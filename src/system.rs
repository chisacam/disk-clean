//! Space the operating system itself holds: volume totals, snapshots, swap, logs, temporary folders.
//! Reported only; never cleaned.

use crate::walk::{Seen, Usage, measure};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub struct Disk {
    pub total: u64,
    pub free: u64,
}

/// Size and free space of the filesystem holding `path`, as `df` shows them.
pub fn disk(path: &Path) -> Option<Disk> {
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let bs = st.f_bsize as u64;
    Some(Disk { total: st.f_blocks * bs, free: st.f_bavail * bs })
}

/// One line of the system section.
pub struct Entry {
    pub label: &'static str,
    pub path: Option<PathBuf>,
    /// Measured size, for a folder or file.
    pub usage: Option<Usage>,
    /// A count, for things that are not sizes (snapshots).
    pub count: Option<usize>,
    pub note: &'static str,
}

pub struct System {
    pub disk: Option<Disk>,
    /// What `disk` covers, in words.
    pub disk_basis: &'static str,
    pub fda: Option<bool>,
    pub entries: Vec<Entry>,
}

impl System {
    pub fn read(home: &Path) -> Self {
        System {
            disk: disk(home),
            disk_basis: platform::DISK_BASIS,
            fda: platform::full_disk_access(home),
            entries: platform::entries(&Seen::default()),
        }
    }
}

fn measured(label: &'static str, path: &Path, note: &'static str, seen: &Seen) -> Entry {
    Entry { label, path: Some(path.to_path_buf()), usage: Some(measure(path, seen)), count: None, note }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{Entry, measured};
    use crate::walk::Seen;
    use std::path::Path;
    use std::process::Command;

    pub const DISK_BASIS: &str = "APFS 컨테이너 기준 — macOS·Preboot·Recovery 볼륨 포함";

    /// Time Machine and OS-update snapshots of the startup volume.
    fn local_snapshots() -> Option<usize> {
        let out = Command::new("tmutil").args(["listlocalsnapshots", "/"]).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        Some(text.lines().filter(|l| l.trim_start().starts_with("com.apple")).count())
    }

    /// Whether this process may read folders macOS guards behind Full Disk Access.
    pub fn full_disk_access(home: &Path) -> Option<bool> {
        for p in [".Trash", "Library/Safari", "Library/Mail"] {
            match crate::walk::retry(|| std::fs::read_dir(home.join(p))) {
                Ok(_) => return Some(true),
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Some(false),
                Err(_) => continue,
            }
        }
        None
    }

    pub fn entries(seen: &Seen) -> Vec<Entry> {
        let mut out = vec![
            Entry {
                label: "로컬 스냅샷",
                path: None,
                usage: None,
                count: local_snapshots(),
                note: "공간이 모자라면 macOS 가 알아서 지움",
            },
            measured("/private/var/vm", Path::new("/private/var/vm"), "절전 이미지·스왑 파일 — 정상", seen),
        ];
        // $TMPDIR is /var/folders/xx/<id>/T/; its parent also holds the per-user cache.
        if let Some(temp) = std::env::temp_dir()
            .parent()
            .filter(|p| p.starts_with("/var/folders") || p.starts_with("/private/var/folders"))
        {
            out.push(measured("임시 폴더", temp, "재부팅 때 macOS 가 정리", seen));
        }
        out
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::{Entry, measured};
    use crate::walk::{Seen, lstat};
    use std::path::Path;

    pub const DISK_BASIS: &str = "홈 폴더가 있는 파일시스템 기준";

    /// Full Disk Access is a macOS permission.
    pub fn full_disk_access(_home: &Path) -> Option<bool> {
        None
    }

    pub fn entries(seen: &Seen) -> Vec<Entry> {
        let candidates: [(&'static str, &str, &'static str); 5] = [
            ("systemd 저널", "/var/log/journal", "sudo journalctl --vacuum-time=2weeks 로 줄일 수 있음"),
            ("/tmp", "/tmp", "재부팅이나 systemd-tmpfiles 가 정리"),
            ("/var/tmp", "/var/tmp", "systemd-tmpfiles 가 오래된 것을 정리"),
            ("스왑 파일", "/swapfile", "스왑 — 정상"),
            ("스왑 파일", "/swap.img", "스왑 — 정상"),
        ];
        candidates
            .into_iter()
            .filter(|(_, p, _)| lstat(Path::new(p)).is_ok())
            .map(|(label, p, note)| measured(label, Path::new(p), note, seen))
            .collect()
    }
}
