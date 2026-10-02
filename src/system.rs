//! Space macOS itself holds: volume totals, snapshots, swap, temporary folders.

use crate::walk::{Seen, Usage, measure};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Disk {
    pub total: u64,
    pub free: u64,
}

/// Size and free space of the APFS container holding `path`, as `df` shows them.
pub fn disk(path: &Path) -> Option<Disk> {
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let bs = st.f_bsize as u64;
    Some(Disk { total: st.f_blocks * bs, free: st.f_bavail * bs })
}

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
fn full_disk_access(home: &Path) -> Option<bool> {
    for p in [".Trash", "Library/Safari", "Library/Mail"] {
        match crate::walk::retry(|| std::fs::read_dir(home.join(p))) {
            Ok(_) => return Some(true),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Some(false),
            Err(_) => continue,
        }
    }
    None
}

pub struct System {
    pub disk: Option<Disk>,
    pub snapshots: Option<usize>,
    /// `/private/var/vm`: the sleep image and swap files.
    pub vm: Usage,
    /// This user's `/var/folders/…` directory, holding `$TMPDIR` and the per-user cache.
    pub temp: Option<(PathBuf, Usage)>,
    pub fda: Option<bool>,
}

impl System {
    pub fn read(home: &Path) -> Self {
        let seen = Seen::default();
        let temp = std::env::temp_dir()
            .parent()
            .filter(|p| p.starts_with("/var/folders") || p.starts_with("/private/var/folders"))
            .map(|p| (p.to_path_buf(), measure(p, &seen)));
        System {
            disk: disk(home),
            snapshots: local_snapshots(),
            vm: measure(Path::new("/private/var/vm"), &seen),
            temp,
            fda: full_disk_access(home),
        }
    }
}
