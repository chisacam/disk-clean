//! Parallel disk-usage measurement.

use rayon::prelude::*;
use std::collections::HashSet;
use std::fs::{self, Metadata};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct Usage {
    /// Bytes allocated on disk (`st_blocks`), so sparse files are not inflated.
    pub bytes: u64,
    pub files: u64,
    /// Entries that could not be read, so the size is a lower bound.
    /// Usually for lack of Full Disk Access, but any error counts.
    pub unreadable: u64,
}

impl Usage {
    pub fn add(&mut self, other: Usage) {
        self.bytes += other.bytes;
        self.files += other.files;
        self.unreadable += other.unreadable;
    }
}

impl std::iter::Sum for Usage {
    fn sum<I: Iterator<Item = Usage>>(iter: I) -> Self {
        iter.fold(Usage::default(), |mut acc, u| {
            acc.add(u);
            acc
        })
    }
}

/// Inodes already counted in this run, so a hard-linked file is counted once.
#[derive(Default)]
pub struct Seen(Mutex<HashSet<(u64, u64)>>);

impl Seen {
    fn first(&self, md: &Metadata) -> bool {
        md.nlink() <= 1 || self.0.lock().unwrap().insert((md.dev(), md.ino()))
    }
}

fn allocated(md: &Metadata) -> u64 {
    md.blocks() * 512
}

const ATTEMPTS: usize = 8;

/// Repeats a call macOS interrupted (EINTR). Reads inside other apps'
/// containers get interrupted now and then, more so under parallel load.
pub fn retry<T>(mut f: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut attempt = 1;
    loop {
        match f() {
            Err(e) if e.kind() == io::ErrorKind::Interrupted && attempt < ATTEMPTS => {
                if *DEBUG {
                    eprintln!("disk-clean: interrupted, retrying ({attempt})");
                }
                attempt += 1;
            }
            other => return other,
        }
    }
}

/// A directory's entries, read again from the start if interrupted.
/// `ReadDir` stops at its first error, which would silently drop the rest of the folder.
pub fn list(dir: &Path) -> io::Result<Vec<fs::DirEntry>> {
    retry(|| fs::read_dir(dir)?.collect())
}

/// `symlink_metadata`, retried if interrupted.
pub fn lstat(path: &Path) -> io::Result<Metadata> {
    retry(|| fs::symlink_metadata(path))
}

static DEBUG: LazyLock<bool> = LazyLock::new(|| std::env::var_os("DISK_CLEAN_DEBUG").is_some());

/// Counts a read failure instead of dropping it, so a short measurement says so.
/// A path that vanished meanwhile is not a failure: there is nothing left to count.
fn unreadable(path: &Path, e: &io::Error) -> u64 {
    if e.kind() == io::ErrorKind::NotFound {
        return 0;
    }
    if *DEBUG {
        eprintln!("disk-clean: {}: {e}", path.display());
    }
    1
}

fn file_usage(md: &Metadata, seen: &Seen) -> Usage {
    if seen.first(md) {
        Usage { bytes: allocated(md), files: 1, unreadable: 0 }
    } else {
        Usage::default()
    }
}

#[derive(Default)]
struct Totals {
    bytes: AtomicU64,
    files: AtomicU64,
    unreadable: AtomicU64,
}

/// Measures a file or a directory tree without following symlinks or leaving its volume.
pub fn measure(path: &Path, seen: &Seen) -> Usage {
    let md = match lstat(path) {
        Ok(md) => md,
        Err(e) => return Usage { unreadable: unreadable(path, &e), ..Usage::default() },
    };
    if !md.is_dir() {
        return file_usage(&md, seen);
    }
    let totals = Totals::default();
    walk(path, md.dev(), seen, &totals);
    Usage {
        bytes: totals.bytes.into_inner() + allocated(&md),
        files: totals.files.into_inner(),
        unreadable: totals.unreadable.into_inner(),
    }
}

fn walk(dir: &Path, dev: u64, seen: &Seen, totals: &Totals) {
    let entries = match list(dir) {
        Ok(entries) => entries,
        Err(e) => {
            totals.unreadable.fetch_add(unreadable(dir, &e), Relaxed);
            return;
        }
    };
    let mut subdirs = Vec::new();
    let mut here = Usage::default();
    for entry in entries {
        // DirEntry::metadata does not traverse symlinks.
        let md = match retry(|| entry.metadata()) {
            Ok(md) => md,
            Err(e) => {
                here.unreadable += unreadable(&entry.path(), &e);
                continue;
            }
        };
        if md.is_dir() {
            if md.dev() == dev {
                here.bytes += allocated(&md);
                subdirs.push(entry.path());
            }
        } else {
            here.add(file_usage(&md, seen));
        }
    }
    totals.bytes.fetch_add(here.bytes, Relaxed);
    totals.files.fetch_add(here.files, Relaxed);
    totals.unreadable.fetch_add(here.unreadable, Relaxed);
    subdirs.par_iter().for_each(|d| walk(d, dev, seen, totals));
}

#[derive(serde::Serialize)]
pub struct Node {
    pub name: String,
    pub is_dir: bool,
    #[serde(flatten)]
    pub usage: Usage,
    /// Largest children first, cut at `keep`.
    pub children: Vec<Node>,
    /// Children cut past `keep`.
    pub rest: Rest,
}

#[derive(Clone, Copy, Default, serde::Serialize)]
pub struct Rest {
    pub count: usize,
    pub bytes: u64,
}

/// Measures `path` once, keeping per-entry sizes for the first `depth` levels.
pub fn tree(path: &Path, depth: usize, keep: usize, seen: &Seen) -> Node {
    let name = path
        .file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    let leaf = |usage: Usage, is_dir: bool| Node {
        name: name.clone(),
        is_dir,
        usage,
        children: Vec::new(),
        rest: Rest::default(),
    };
    let md = match lstat(path) {
        Ok(md) => md,
        Err(e) => return leaf(Usage { unreadable: unreadable(path, &e), ..Usage::default() }, false),
    };
    if !md.is_dir() {
        return leaf(file_usage(&md, seen), false);
    }
    if depth == 0 {
        return leaf(measure(path, seen), true);
    }
    let mut failed = 0;
    let entries: Vec<(PathBuf, Metadata)> = match list(path) {
        Ok(entries) => entries
            .into_iter()
            .filter_map(|e| match retry(|| e.metadata()) {
                Ok(m) => Some((e.path(), m)),
                Err(err) => {
                    failed += unreadable(&e.path(), &err);
                    None
                }
            })
            .filter(|(_, m)| !m.is_dir() || m.dev() == md.dev())
            .collect(),
        Err(e) => return leaf(Usage { unreadable: unreadable(path, &e), ..Usage::default() }, true),
    };
    let mut children: Vec<Node> = entries
        .par_iter()
        .map(|(p, m)| {
            if m.is_dir() {
                tree(p, depth - 1, keep, seen)
            } else {
                Node {
                    name: p.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                    is_dir: false,
                    usage: file_usage(m, seen),
                    children: Vec::new(),
                    rest: Rest::default(),
                }
            }
        })
        .collect();
    let mut usage: Usage = children.iter().map(|c| c.usage).sum();
    usage.bytes += allocated(&md);
    usage.unreadable += failed;
    children.sort_by_key(|c| std::cmp::Reverse(c.usage.bytes));
    let rest = if children.len() > keep {
        let cut = children.split_off(keep);
        Rest { count: cut.len(), bytes: cut.iter().map(|c| c.usage.bytes).sum() }
    } else {
        Rest::default()
    };
    Node { name, is_dir: true, usage, children, rest }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn retry_rides_out_interruptions_but_not_other_errors() {
        let calls = Cell::new(0);
        let flaky = || {
            calls.set(calls.get() + 1);
            if calls.get() < 3 { Err(io::Error::from(io::ErrorKind::Interrupted)) } else { Ok(7) }
        };
        assert_eq!(retry(flaky).unwrap(), 7);
        assert_eq!(calls.get(), 3);

        calls.set(0);
        let denied = || {
            calls.set(calls.get() + 1);
            Err::<(), _>(io::Error::from(io::ErrorKind::PermissionDenied))
        };
        assert!(retry(denied).is_err());
        assert_eq!(calls.get(), 1, "not retried");

        calls.set(0);
        let stuck = || {
            calls.set(calls.get() + 1);
            Err::<(), _>(io::Error::from(io::ErrorKind::Interrupted))
        };
        assert!(retry(stuck).is_err());
        assert_eq!(calls.get(), ATTEMPTS, "gives up");
    }

    #[test]
    fn counts_unreadable_folders_instead_of_dropping_them() {
        // Root reads a mode-000 folder anyway, so there is nothing to observe.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let locked = t.path().join("locked");
        fs::create_dir_all(locked.join("inside")).unwrap();
        fs::write(t.path().join("f"), vec![1u8; 10_000]).unwrap();
        fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
        let usage = measure(t.path(), &Seen::default());
        fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        assert_eq!(usage.unreadable, 1);
        assert!(usage.bytes >= 10_000);
    }
}
