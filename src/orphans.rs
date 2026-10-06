//! Containers that outlived their app.
//!
//! Deleting an app leaves `~/Library/Containers/<bundle id>` behind, with
//! everything the app downloaded. A container counts as left over only when
//! every source agrees the app is gone: the bundle path recorded in the
//! container's metadata no longer exists, LaunchServices has no app under that
//! bundle id, and neither does Spotlight.

use crate::walk::{Seen, Usage, list, lstat, measure, retry};
use plist::Value;
use rayon::prelude::*;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

const METADATA: &str = ".com.apple.containermanagerd.metadata.plist";

pub struct Orphan {
    pub id: String,
    /// Where the app was when it last ran.
    pub app: PathBuf,
    pub container: PathBuf,
    pub usage: Usage,
    /// The app's own folders under `/var/folders`, and its Application Scripts folder.
    pub extra: Vec<(PathBuf, Usage)>,
    pub last_used: Option<SystemTime>,
}

struct Meta {
    id: String,
    app: Option<PathBuf>,
    /// `DARWIN_USER_{CACHE,TEMP,DIR}` as the container recorded them.
    darwin: Vec<PathBuf>,
}

fn lookup<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().try_fold(v, |v, k| v.as_dictionary()?.get(k))
}

fn read_meta(container: &Path) -> Option<Meta> {
    let bytes = retry(|| fs::read(container.join(METADATA))).ok()?;
    let v = Value::from_reader(io::Cursor::new(bytes)).ok()?;
    let id = lookup(&v, &["MCMMetadataIdentifier"])?.as_string()?.to_string();
    let params = lookup(&v, &["MCMMetadataInfo", "SandboxProfileDataValidationInfo", "Parameters"]);
    let path = |key: &str| {
        params
            .and_then(|p| lookup(p, &[key]))
            .and_then(Value::as_string)
            .map(PathBuf::from)
    };
    Some(Meta {
        id,
        app: path("application_bundle"),
        darwin: ["application_darwin_cache_dir", "application_darwin_temp_dir", "application_darwin_user_dir"]
            .iter()
            .filter_map(|k| path(k))
            .collect(),
    })
}

/// Bundle ids are reverse-DNS; anything else is not ours to judge.
fn plain_id(id: &str) -> bool {
    !id.is_empty()
        && id.contains('.')
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// The recorded app path if the container is left over, judged by `installed`.
fn left_over(dir_name: &str, meta: &Meta, installed: impl Fn(&str) -> bool) -> Option<PathBuf> {
    if meta.id != dir_name || dir_name.starts_with("com.apple.") || !plain_id(dir_name) {
        return None;
    }
    let app = meta.app.as_ref()?;
    // An app on a disk that is not plugged in looks exactly like a deleted one.
    if !app.is_absolute() || app.starts_with("/Volumes") {
        return None;
    }
    match lstat(app) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        _ => return None,
    }
    if installed(&meta.id) {
        return None;
    }
    Some(app.clone())
}

fn installed(id: &str) -> bool {
    !registered_apps(id).is_empty() || spotlight(id)
}

/// Whether Spotlight knows an app with this bundle id. Unsure counts as yes.
fn spotlight(id: &str) -> bool {
    let query = format!("kMDItemCFBundleIdentifier == '{id}'");
    match Command::new("mdfind").arg(query).output() {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .any(|l| Path::new(l).exists()),
        _ => true,
    }
}

#[cfg(target_os = "macos")]
mod launch_services {
    use std::ffi::{OsStr, c_void};
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::ptr::null;

    type CFTypeRef = *const c_void;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithBytes(alloc: CFTypeRef, bytes: *const u8, len: isize, encoding: u32, external: u8) -> CFTypeRef;
        fn CFArrayGetCount(array: CFTypeRef) -> isize;
        fn CFArrayGetValueAtIndex(array: CFTypeRef, idx: isize) -> CFTypeRef;
        fn CFURLGetFileSystemRepresentation(url: CFTypeRef, resolve: u8, buf: *mut u8, max: isize) -> u8;
        fn CFRelease(cf: CFTypeRef);
    }

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn LSCopyApplicationURLsForBundleIdentifier(id: CFTypeRef, err: *mut CFTypeRef) -> CFTypeRef;
    }

    const UTF8: u32 = 0x0800_0100;

    /// Apps LaunchServices has registered under this bundle id that are still on disk.
    pub fn registered_apps(id: &str) -> Vec<PathBuf> {
        let mut out = Vec::new();
        // SAFETY: every object created or copied here is released before returning,
        // and the array is only indexed below its count.
        unsafe {
            let cf_id = CFStringCreateWithBytes(null(), id.as_ptr(), id.len() as isize, UTF8, 0);
            if cf_id.is_null() {
                return out;
            }
            let mut err: CFTypeRef = null();
            let urls = LSCopyApplicationURLsForBundleIdentifier(cf_id, &mut err);
            CFRelease(cf_id);
            if !err.is_null() {
                CFRelease(err);
            }
            if urls.is_null() {
                return out;
            }
            for i in 0..CFArrayGetCount(urls) {
                let mut buf = [0u8; 4096];
                let url = CFArrayGetValueAtIndex(urls, i);
                if CFURLGetFileSystemRepresentation(url, 1, buf.as_mut_ptr(), buf.len() as isize) != 0 {
                    let len = buf.iter().position(|&b| b == 0).unwrap_or(0);
                    out.push(PathBuf::from(OsStr::from_bytes(&buf[..len])));
                }
            }
            CFRelease(urls);
        }
        out.retain(|p| p.exists());
        out
    }
}

/// LaunchServices is macOS's; elsewhere there is nothing to ask.
#[cfg(not(target_os = "macos"))]
mod launch_services {
    pub fn registered_apps(_id: &str) -> Vec<std::path::PathBuf> {
        Vec::new()
    }
}

use launch_services::registered_apps;

/// The current user's `/var/folders/xx/<id>` directory, resolved.
pub fn temp_root() -> Option<PathBuf> {
    let root = fs::canonicalize(std::env::temp_dir().parent()?).ok()?;
    root.starts_with("/private/var/folders").then_some(root)
}

/// An app's folder under the temp root: `C/<id>`, `T/<id>` or `0/<id>`.
fn darwin_dir(path: &Path, id: &str, temp: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(temp) else { return false };
    let parts: Vec<&OsStr> = rel.iter().collect();
    parts.len() == 2 && ["C", "T", "0"].iter().any(|d| parts[0] == *d) && parts[1] == id
}

pub fn find(home: &Path, seen: &Seen) -> Vec<Orphan> {
    // App containers and their metadata are macOS's.
    if !cfg!(target_os = "macos") {
        return Vec::new();
    }
    let root = home.join("Library/Containers");
    let Ok(entries) = list(&root) else { return Vec::new() };
    let temp = temp_root();
    let dirs: Vec<(PathBuf, String)> = entries
        .into_iter()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| (e.path(), e.file_name().to_string_lossy().into_owned()))
        .collect();
    dirs.into_par_iter()
        .filter_map(|(container, name)| {
            let meta = read_meta(&container)?;
            let app = left_over(&name, &meta, installed)?;
            let mut extra: Vec<PathBuf> = meta
                .darwin
                .iter()
                .filter(|p| temp.as_deref().is_some_and(|t| darwin_dir(p, &meta.id, t)))
                .cloned()
                .collect();
            extra.push(home.join("Library/Application Scripts").join(&meta.id));
            extra.retain(|p| lstat(p).is_ok_and(|m| m.is_dir()));
            Some(Orphan {
                usage: measure(&container, seen),
                extra: extra
                    .into_iter()
                    .map(|p| {
                        let usage = measure(&p, seen);
                        (p, usage)
                    })
                    .collect(),
                last_used: lstat(&container.join(METADATA)).and_then(|m| m.modified()).ok(),
                id: meta.id,
                app,
                container,
            })
        })
        .collect()
}

/// Asked again right before deleting: is this container still left over?
pub fn confirm(container: &Path) -> bool {
    let Some(name) = container.file_name().and_then(OsStr::to_str) else { return false };
    read_meta(container).is_some_and(|meta| left_over(name, &meta, installed).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str, app: Option<&Path>) -> Meta {
        Meta { id: id.into(), app: app.map(Path::to_path_buf), darwin: Vec::new() }
    }

    #[test]
    fn left_over_only_when_every_source_agrees() {
        let t = tempfile::tempdir().unwrap();
        let gone = t.path().join("Gone.app");
        let here = t.path().join("Here.app");
        fs::create_dir(&here).unwrap();
        let nobody = |_: &str| false;

        assert_eq!(left_over("com.x.game", &meta("com.x.game", Some(&gone)), nobody), Some(gone.clone()));
        // The app is still where it was.
        assert_eq!(left_over("com.x.game", &meta("com.x.game", Some(&here)), nobody), None);
        // Moved elsewhere, and LaunchServices or Spotlight found it.
        assert_eq!(left_over("com.x.game", &meta("com.x.game", Some(&gone)), |_| true), None);
        // Nothing recorded: cannot tell.
        assert_eq!(left_over("com.x.game", &meta("com.x.game", None), nobody), None);
        // On a disk that may just be unplugged.
        let ext = Path::new("/Volumes/Games/Gone.app");
        assert_eq!(left_over("com.x.game", &meta("com.x.game", Some(ext)), nobody), None);
        // macOS's own, a folder whose metadata names someone else, an odd id.
        assert_eq!(left_over("com.apple.x", &meta("com.apple.x", Some(&gone)), nobody), None);
        assert_eq!(left_over("com.x.game", &meta("com.y.other", Some(&gone)), nobody), None);
        assert_eq!(left_over("x'y.z", &meta("x'y.z", Some(&gone)), nobody), None);
    }

    #[test]
    fn darwin_dirs_are_the_apps_own() {
        let temp = Path::new("/private/var/folders/q4/abc");
        assert!(darwin_dir(&temp.join("C/com.x.game"), "com.x.game", temp));
        assert!(darwin_dir(&temp.join("T/com.x.game"), "com.x.game", temp));
        assert!(!darwin_dir(&temp.join("C"), "com.x.game", temp));
        assert!(!darwin_dir(&temp.join("C/com.y.other"), "com.x.game", temp));
        assert!(!darwin_dir(&temp.join("X/com.x.game"), "com.x.game", temp));
        assert!(!darwin_dir(Path::new("/tmp/C/com.x.game"), "com.x.game", temp));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn launch_services_knows_finder() {
        // Control for the FFI: an app every Mac has.
        assert!(!registered_apps("com.apple.finder").is_empty());
        assert!(registered_apps("invalid.disk-clean.nothing").is_empty());
    }
}
