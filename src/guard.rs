//! What may be deleted, and deleting it.

use crate::orphans;
use crate::scan::{Item, Recheck};
use crate::walk::{list, lstat, retry};
use anyhow::{Context, Result, ensure};
use rayon::prelude::*;
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

/// Never removed, whatever a rule or a bug points at. Rules empty some of these; none is deleted itself.
const PROTECTED: &[&str] = &[
    "Library",
    "Library/Caches",
    "Library/Containers",
    "Library/Group Containers",
    "Library/Application Support",
    "Library/Logs",
    "Library/Developer",
    "Desktop",
    "Documents",
    "Downloads",
    "Movies",
    "Music",
    "Pictures",
    ".Trash",
    ".cache",
    ".cargo",
    ".gradle",
    ".m2",
    ".npm",
    "go",
];

/// Where anything may be deleted at all.
pub struct Roots {
    home: PathBuf,
    /// This user's `/var/folders/xx/<id>`, resolved.
    temp: Option<PathBuf>,
}

impl Roots {
    pub fn new(home: &Path) -> Self {
        Roots { home: home.to_path_buf(), temp: orphans::temp_root() }
    }
}

pub fn item_check(item: &Item) -> Result<()> {
    if let Recheck::Leftover(container) = &item.recheck {
        ensure!(orphans::confirm(container), "앱이 다시 보여 건너뜀 (설치돼 있거나 판정할 수 없음)");
    }
    Ok(())
}

pub fn path_check(item: &Item, path: &Path, roots: &Roots) -> Result<()> {
    check(path, roots)?;
    if let Recheck::Artifact(kind) = item.recheck {
        ensure!(kind.matches(path), "{} 로 보이지 않아 건너뜀", kind.name);
    }
    Ok(())
}

/// Refuses anything outside the home folder, too shallow, protected, or reached through a symlink.
/// Under the temp root, only an app's own `C/<id>`, `T/<id>` or `0/<id>` is allowed.
fn check(path: &Path, roots: &Roots) -> Result<()> {
    let in_temp = roots.temp.as_deref().and_then(|t| path.strip_prefix(t).ok());
    let rel = match in_temp {
        Some(rel) => {
            let parts: Vec<Component> = rel.components().collect();
            let app_dir = parts.len() == 2
                && matches!(parts[0], Component::Normal(d) if ["C", "T", "0"].iter().any(|x| d == *x))
                && matches!(parts[1], Component::Normal(_));
            ensure!(app_dir, "임시 폴더의 앱별 폴더가 아니라 건너뜀");
            rel
        }
        None => path.strip_prefix(&roots.home).context("홈 폴더 밖이라 건너뜀")?,
    };
    ensure!(rel.components().all(|c| matches!(c, Component::Normal(_))), "경로가 이상해 건너뜀");
    if in_temp.is_none() {
        ensure!(rel.components().count() >= 2, "너무 상위 경로라 건너뜀");
        ensure!(!PROTECTED.iter().any(|p| rel == Path::new(p)), "보호된 경로라 건너뜀");
    }
    let parent = path.parent().context("상위 폴더가 없음")?;
    ensure!(fs::canonicalize(parent)? == parent, "상위 경로에 심볼릭 링크가 있어 건너뜀");
    Ok(())
}

pub fn remove(path: &Path) -> io::Result<()> {
    let md = lstat(path)?;
    if !md.is_dir() {
        return retry(|| fs::remove_file(path));
    }
    // An interrupted attempt may have finished the job; gone is what we wanted.
    let remove_tree = || match fs::remove_dir_all(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    };
    match retry(remove_tree) {
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            make_writable(path);
            retry(remove_tree)
        }
        other => other,
    }
}

/// Go's module cache and some installers leave read-only directories, whose entries cannot be unlinked.
fn make_writable(dir: &Path) {
    let Ok(md) = lstat(dir) else { return };
    if !md.is_dir() {
        return;
    }
    let mode = md.mode() & 0o7777;
    if md.uid() == unsafe { libc::getuid() } && mode & 0o700 != 0o700 {
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(mode | 0o700));
    }
    let Ok(entries) = list(dir) else { return };
    let subdirs: Vec<PathBuf> = entries
        .into_iter()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    subdirs.par_iter().for_each(|d| make_writable(d));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_refuses_dangerous_paths() {
        let t = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(t.path()).unwrap();
        let home = base.join("home");
        let temp = base.join("folders");
        fs::create_dir_all(home.join("Library/Caches/app")).unwrap();
        fs::create_dir_all(home.join("real/x")).unwrap();
        fs::create_dir_all(temp.join("C/com.x.game")).unwrap();
        std::os::unix::fs::symlink(home.join("real"), home.join("link")).unwrap();
        let roots = Roots { home: home.clone(), temp: Some(temp.clone()) };

        assert!(check(&home.join("Library/Caches/app"), &roots).is_ok());
        assert!(check(&home.join("Library/Caches"), &roots).is_err(), "protected");
        assert!(check(&home.join("Documents"), &roots).is_err(), "too shallow");
        assert!(check(&home.join("link/x"), &roots).is_err(), "through a symlink");
        assert!(check(Path::new("/tmp/x/y"), &roots).is_err(), "outside home");
        assert!(check(&home.join("Library/Caches/../Caches/app"), &roots).is_err(), "dot-dot");
        assert!(check(&temp.join("C/com.x.game"), &roots).is_ok(), "an app's cache folder");
        assert!(check(&temp.join("C"), &roots).is_err(), "the whole cache folder");
        assert!(check(&temp.join("X/com.x.game"), &roots).is_err(), "not C, T or 0");
        assert!(check(&temp.join("C/com.x.game/sub"), &roots).is_err(), "deeper than the app folder");
    }

    #[test]
    fn removes_read_only_trees() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("mod");
        fs::create_dir_all(dir.join("pkg@v1")).unwrap();
        fs::write(dir.join("pkg@v1/a.go"), b"x").unwrap();
        fs::set_permissions(dir.join("pkg@v1"), fs::Permissions::from_mode(0o555)).unwrap();
        remove(&dir).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn removing_a_symlink_keeps_its_target() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("keep")).unwrap();
        fs::write(t.path().join("keep/f"), b"x").unwrap();
        std::os::unix::fs::symlink(t.path().join("keep"), t.path().join("link")).unwrap();
        remove(&t.path().join("link")).unwrap();
        assert!(t.path().join("keep/f").exists());
    }
}
