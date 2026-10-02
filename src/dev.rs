//! Build and dependency folders inside projects.

use crate::walk::{Seen, Usage, list, lstat, measure};
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

pub struct Kind {
    pub name: &'static str,
    pub about: &'static str,
    /// Given the artifact and its parent, whether this really is a build output.
    check: fn(&Path, &Path) -> bool,
}

impl Kind {
    pub fn matches(&self, dir: &Path) -> bool {
        dir.file_name().and_then(|n| n.to_str()) == Some(self.name)
            && dir.parent().is_some_and(|p| (self.check)(dir, p))
    }
}

fn has(dir: &Path, file: &str) -> bool {
    dir.join(file).is_file()
}

fn has_ext(dir: &Path, ext: &str) -> bool {
    list(dir).is_ok_and(|entries| {
        entries
            .iter()
            .any(|e| e.path().extension().is_some_and(|x| x == ext))
    })
}

fn js(_: &Path, parent: &Path) -> bool {
    has(parent, "package.json")
}

fn venv(dir: &Path, _: &Path) -> bool {
    has(dir, "pyvenv.cfg")
}

fn gradle(_: &Path, parent: &Path) -> bool {
    ["build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts"]
        .iter()
        .any(|f| has(parent, f))
}

fn always(_: &Path, _: &Path) -> bool {
    true
}

/// Next to Terraform code, or left behind after the code was deleted:
/// the folder is git-ignored, so it outlives the `.tf` files.
fn terraform(dir: &Path, parent: &Path) -> bool {
    has_ext(parent, "tf") || orphan_terraform(dir, parent)
}

fn orphan_terraform(dir: &Path, parent: &Path) -> bool {
    !has_ext(parent, "tf")
        && ["providers", "modules", "terraform.tfstate"]
            .iter()
            .any(|f| dir.join(f).exists())
}

pub static KINDS: &[Kind] = &[
    Kind { name: "node_modules", about: "npm·pnpm·yarn install 로 다시 설치", check: js },
    Kind {
        name: "target",
        about: "cargo build·mvn package 로 다시 빌드",
        check: |dir, parent| {
            has(parent, "Cargo.toml") || has(parent, "pom.xml") || has(dir, "CACHEDIR.TAG")
        },
    },
    Kind {
        name: ".terraform",
        about: "terraform init 으로 다시 받음. state 는 그대로지만, 고른 workspace 는 default 로 돌아감",
        check: terraform,
    },
    Kind { name: ".venv", about: "uv sync·pip install 로 다시 설치", check: venv },
    Kind { name: "venv", about: "pip install 로 다시 설치", check: venv },
    Kind { name: ".next", about: "next build 로 다시 만듦", check: js },
    Kind { name: ".nuxt", about: "nuxt build 로 다시 만듦", check: js },
    Kind { name: ".svelte-kit", about: "vite build 로 다시 만듦", check: js },
    Kind { name: ".turbo", about: "turbo 캐시", check: js },
    Kind { name: ".parcel-cache", about: "parcel 캐시", check: js },
    Kind { name: ".angular", about: "angular 빌드 캐시", check: js },
    Kind { name: "build", about: "gradle build 로 다시 빌드", check: gradle },
    Kind { name: ".gradle", about: "gradle 이 다시 만듦", check: gradle },
    Kind {
        name: ".build",
        about: "swift build 로 다시 빌드",
        check: |_, parent| has(parent, "Package.swift"),
    },
    Kind {
        name: ".dart_tool",
        about: "dart pub get 으로 다시 만듦",
        check: |_, parent| has(parent, "pubspec.yaml"),
    },
    Kind { name: ".tox", about: "tox 가 다시 만듦", check: |_, parent| has(parent, "tox.ini") },
    Kind { name: "__pycache__", about: "파이썬이 알아서 다시 만듦", check: always },
    Kind { name: ".pytest_cache", about: "pytest 가 다시 만듦", check: always },
    Kind { name: ".mypy_cache", about: "mypy 가 다시 만듦", check: always },
    Kind { name: ".ruff_cache", about: "ruff 가 다시 만듦", check: always },
];

/// Never searched: version control internals.
const PRUNE: &[&str] = &[".git", ".hg", ".svn"];

pub struct Found {
    pub path: PathBuf,
    pub kind: &'static Kind,
    pub usage: Usage,
    pub modified: Option<SystemTime>,
    /// `.terraform` directories nested inside this one.
    pub nested: usize,
    /// Terraform workspace selected here, other than `default`.
    pub workspace: Option<String>,
    /// A `.terraform` whose Terraform code is gone.
    pub orphan: bool,
}

/// Home folders except hidden ones and those that hold no projects.
///
/// Hidden folders are skipped on purpose: tools install themselves there
/// (`~/.vscode/extensions`, `~/.local/pipx`, `~/.nvm`), and their
/// `node_modules` or virtualenvs are part of the installed tool.
pub fn default_roots(home: &Path) -> Vec<PathBuf> {
    const SKIP: &[&str] = &["Library", "Applications", "Movies", "Music", "Pictures", "Public"];
    let Ok(entries) = list(home) else { return Vec::new() };
    entries
        .into_iter()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            !name.starts_with('.') && !SKIP.contains(&name.as_ref())
        })
        .map(|e| e.path())
        .collect()
}

pub fn find(roots: &[PathBuf], seen: &Seen) -> Vec<Found> {
    let hits = Mutex::new(Vec::new());
    roots.par_iter().for_each(|r| walk(r, &hits));
    hits.into_inner()
        .unwrap()
        .into_par_iter()
        .map(|(path, kind): (PathBuf, &'static Kind)| {
            let usage = measure(&path, seen);
            let modified = lstat(&path).and_then(|m| m.modified()).ok();
            let (nested, workspace, orphan) = match (kind.name, path.parent()) {
                (".terraform", Some(parent)) => {
                    let workspace = fs::read_to_string(path.join("environment"))
                        .ok()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty() && s != "default");
                    let nested = count_named(&path.join("modules"), ".terraform");
                    (nested, workspace, orphan_terraform(&path, parent))
                }
                _ => (0, None, false),
            };
            Found { path, kind, usage, modified, nested, workspace, orphan }
        })
        .collect()
}

fn walk(dir: &Path, hits: &Mutex<Vec<(PathBuf, &'static Kind)>>) {
    let Ok(entries) = list(dir) else { return };
    let mut subdirs = Vec::new();
    for entry in entries {
        // file_type comes from the directory entry itself and never follows symlinks.
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let path = entry.path();
        if let Some(kind) = KINDS.iter().find(|k| k.name == name) {
            if (kind.check)(&path, dir) {
                hits.lock().unwrap().push((path, kind));
                continue;
            }
            // A global node_modules holds installed packages whose own
            // node_modules look like projects; do not look inside it.
            if name == "node_modules" {
                continue;
            }
        }
        if !PRUNE.contains(&name) {
            subdirs.push(path);
        }
    }
    subdirs.par_iter().for_each(|d| walk(d, hits));
}

fn count_named(dir: &Path, name: &str) -> usize {
    let Ok(entries) = list(dir) else { return 0 };
    let subdirs: Vec<(PathBuf, bool)> = entries
        .into_iter()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| (e.path(), e.file_name() == name))
        .collect();
    subdirs.par_iter().map(|(p, hit)| *hit as usize + count_named(p, name)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(name: &str) -> &'static Kind {
        KINDS.iter().find(|k| k.name == name).unwrap()
    }

    fn mkdir(p: &Path) {
        fs::create_dir_all(p).unwrap();
    }

    fn touch(p: &Path) {
        mkdir(p.parent().unwrap());
        fs::write(p, b"x").unwrap();
    }

    #[test]
    fn finds_artifacts_only_next_to_their_project_file() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        // A real project.
        mkdir(&root.join("app/node_modules/left-pad"));
        touch(&root.join("app/package.json"));
        // A folder that merely has the name.
        mkdir(&root.join("notes/target"));
        // A global install: packages inside carry their own package.json.
        mkdir(&root.join("lib/node_modules/tool/node_modules"));
        touch(&root.join("lib/node_modules/tool/package.json"));
        // Terraform, with a module clone that someone ran init inside.
        touch(&root.join("infra/main.tf"));
        mkdir(&root.join("infra/.terraform/modules/m/env/.terraform"));
        touch(&root.join("infra/.terraform/modules/m/env/main.tf"));
        fs::write(root.join("infra/.terraform/environment"), "prod\n").unwrap();
        // Left behind after the Terraform code was deleted.
        mkdir(&root.join("gone/.terraform/providers"));
        // Named like it, but nothing Terraform made.
        mkdir(&root.join("misc/.terraform/notes"));
        // Version control is never searched.
        mkdir(&root.join("app2/.git/node_modules"));
        touch(&root.join("app2/.git/package.json"));

        let found = find(&[root.to_path_buf()], &Seen::default());
        let mut names: Vec<String> = found
            .iter()
            .map(|f| f.path.strip_prefix(root).unwrap().display().to_string())
            .collect();
        names.sort();
        assert_eq!(names, ["app/node_modules", "gone/.terraform", "infra/.terraform"]);

        let tf = found.iter().find(|f| f.path.ends_with("infra/.terraform")).unwrap();
        assert_eq!(tf.nested, 1);
        assert_eq!(tf.workspace.as_deref(), Some("prod"));
        assert!(!tf.orphan);
        assert!(found.iter().find(|f| f.path.ends_with("gone/.terraform")).unwrap().orphan);
    }

    #[test]
    fn matches_rechecks_the_marker() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("p/target");
        mkdir(&dir);
        assert!(!kind("target").matches(&dir));
        touch(&t.path().join("p/Cargo.toml"));
        assert!(kind("target").matches(&dir));
        assert!(!kind("node_modules").matches(&dir));
    }
}
