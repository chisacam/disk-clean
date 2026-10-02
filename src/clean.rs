//! Deleting what the user picked, after showing exactly what that is.

use crate::fmt::{rpad, size, tilde};
use crate::orphans;
use crate::scan::{Item, Recheck};
use crate::system;
use crate::walk::{list, lstat, retry};
use anyhow::{Context, Result, bail, ensure};
use console::style;
use dialoguer::{Confirm, MultiSelect};
use rayon::prelude::*;
use std::fs;
use std::io::{self, IsTerminal};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

pub struct Opts {
    pub dry_run: bool,
    pub yes: bool,
}

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

pub fn run(items: Vec<Item>, ids: &[String], opts: &Opts, home: &Path) -> Result<()> {
    let (cleanable, review): (Vec<Item>, Vec<Item>) = items
        .into_iter()
        .filter(|i| i.usage.bytes > 0 && !i.paths.is_empty())
        .partition(|i| i.cleanable);
    let selected = if !ids.is_empty() {
        by_ids(&cleanable, &review, ids)?
    } else if io::stdin().is_terminal() {
        pick(&cleanable)?
    } else {
        bail!("지울 항목 ID 를 지정하세요. ID 는 `disk-clean scan` 에 나옵니다.");
    };
    if selected.is_empty() {
        println!("고른 항목이 없습니다.");
        return Ok(());
    }

    let roots = Roots { home: home.to_path_buf(), temp: orphans::temp_root() };
    // Judged once per item, before any of its paths is touched: deleting a
    // container first would otherwise make its other folders look unowned.
    let verdicts: Vec<Result<()>> = selected.iter().map(|i| item_check(i)).collect();

    let total: u64 = selected.iter().map(|i| i.usage.bytes).sum();
    let count: usize = selected.iter().map(|i| i.paths.len()).sum();
    println!("{}", style("지울 항목").bold());
    for (item, verdict) in selected.iter().zip(&verdicts) {
        println!("  {}  {}  {}", rpad(&size(item.usage.bytes), 9), style(&item.id).cyan(), item.shown);
        for note in &item.notes {
            println!("               {}", style(note).yellow());
        }
        if item.usage.unreadable > 0 {
            println!("               {}", style(crate::report::short_note(item.usage.unreadable)).yellow());
        }
        if let Err(e) = verdict {
            println!("               {}  {}", style("건너뜀").red(), style(format!("{e:#}")).dim());
            continue;
        }
        if opts.dry_run {
            // The same checks the real run makes, so the preview does not promise more than it will do.
            for (k, p) in item.paths.iter().enumerate() {
                match path_check(item, p, &roots) {
                    Err(e) => println!("               {}  {}", style("건너뜀").red(), style(format!("{}: {e:#}", tilde(p, home))).dim()),
                    Ok(()) if k < 20 => println!("               {}", style(tilde(p, home)).dim()),
                    Ok(()) => {}
                }
            }
            if item.paths.len() > 20 {
                println!("               {}", style(format!("… 외 {}개", item.paths.len() - 20)).dim());
            }
        }
    }
    println!("  합계 {} · 경로 {count}개", size(total));
    if opts.dry_run {
        println!("{}", style("dry-run: 아무것도 지우지 않았습니다.").green());
        return Ok(());
    }
    if !opts.yes {
        ensure!(io::stdin().is_terminal(), "확인을 받을 수 없습니다. 지우려면 --yes 를 붙이세요.");
        let go = Confirm::new()
            .with_prompt(format!("{} 를 삭제합니다. 되돌릴 수 없습니다. 계속할까요?", size(total)))
            .default(false)
            .interact()?;
        if !go {
            println!("취소했습니다.");
            return Ok(());
        }
    }

    let before = system::disk(home).map(|d| d.free);
    let mut jobs: Vec<(&Item, &PathBuf)> = Vec::new();
    let mut failures: Vec<(&PathBuf, String)> = Vec::new();
    for (item, verdict) in selected.iter().zip(&verdicts) {
        match verdict {
            Ok(()) => jobs.extend(item.paths.iter().map(|p| (*item, p))),
            Err(e) => failures.extend(item.paths.iter().map(|p| (p, format!("{e:#}")))),
        }
    }
    failures.par_extend(
        jobs.par_iter()
            .filter_map(|&(item, path)| delete(item, path, &roots).err().map(|e| (path, format!("{e:#}")))),
    );
    let after = system::disk(home).map(|d| d.free);

    let label = if failures.is_empty() { style("완료:").green() } else { style("일부 실패:").yellow() };
    println!("{} 경로 {count}개 중 {}개를 지웠습니다.", label.bold(), count - failures.len());
    if let (Some(b), Some(a)) = (before, after) {
        let gained = a.saturating_sub(b);
        println!(
            "  디스크 남은 공간 {} → {} (+{}, statfs 로 측정) · 지우기 전에 잰 크기 {}",
            size(b),
            size(a),
            size(gained),
            size(total)
        );
        if failures.is_empty() && gained < total / 2 {
            println!(
                "  {}",
                style("늘어난 공간이 잰 크기보다 적습니다. 앱이 열어 둔 파일은 앱을 닫아야, 하드 링크·APFS 복제본은 다른 사본까지 없어져야 공간이 돌아옵니다.").dim()
            );
        }
    }
    if !failures.is_empty() {
        println!("{}", style(format!("실패 {}건", failures.len())).red().bold());
        for (path, err) in failures.iter().take(10) {
            println!("  {}: {err}", tilde(path, home));
        }
        if failures.len() > 10 {
            println!("  … 외 {}건", failures.len() - 10);
        }
        if failures.iter().any(|(_, e)| e.contains("Operation not permitted")) {
            println!("  {}", style(crate::report::FDA_HELP).dim());
        }
    }
    Ok(())
}

fn hit(id: &str, arg: &str) -> bool {
    id == arg || id.strip_prefix(arg).is_some_and(|r| r.starts_with('/') || r.starts_with(':'))
}

fn by_ids<'a>(cleanable: &'a [Item], review: &[Item], ids: &[String]) -> Result<Vec<&'a Item>> {
    let mut out: Vec<&Item> = Vec::new();
    for arg in ids {
        let found: Vec<&Item> = cleanable.iter().filter(|i| hit(&i.id, arg)).collect();
        if found.is_empty() {
            if review.iter().any(|i| hit(&i.id, arg)) {
                bail!("`{arg}` 는 직접 판단할 항목이라 지우지 않습니다. 앱 안에서 정리하세요.");
            }
            bail!("`{arg}` 에 해당하는 항목이 없습니다(비어 있거나 ID 가 다름). `disk-clean scan` 으로 확인하세요.");
        }
        for item in found {
            if !out.iter().any(|o| std::ptr::eq(*o, item)) {
                out.push(item);
            }
        }
    }
    Ok(out)
}

fn pick(items: &[Item]) -> Result<Vec<&Item>> {
    let mut sorted: Vec<&Item> = items.iter().collect();
    // Biggest first: what frees the most should not sit below the scroll.
    sorted.sort_by_key(|i| std::cmp::Reverse(i.usage.bytes));
    let labels: Vec<String> = sorted
        .iter()
        .map(|i| format!("{}  [{}] {}  {}", rpad(&size(i.usage.bytes), 9), i.safety.tag(), i.id, i.shown))
        .collect();
    let defaults: Vec<bool> = sorted.iter().map(|i| i.preselect).collect();
    let chosen = MultiSelect::new()
        .with_prompt("지울 항목 (스페이스: 선택, 엔터: 확정, Esc: 취소)")
        .items(&labels)
        .defaults(&defaults)
        .max_length(20)
        .interact_opt()?;
    Ok(chosen.unwrap_or_default().into_iter().map(|k| sorted[k]).collect())
}

/// Where `clean` may delete at all.
struct Roots {
    home: PathBuf,
    /// This user's `/var/folders/xx/<id>`, resolved.
    temp: Option<PathBuf>,
}

fn item_check(item: &Item) -> Result<()> {
    if let Recheck::Leftover(container) = &item.recheck {
        ensure!(orphans::confirm(container), "앱이 다시 보여 건너뜀 (설치돼 있거나 판정할 수 없음)");
    }
    Ok(())
}

fn path_check(item: &Item, path: &Path, roots: &Roots) -> Result<()> {
    check(path, roots)?;
    if let Recheck::Artifact(kind) = item.recheck {
        ensure!(kind.matches(path), "{} 로 보이지 않아 건너뜀", kind.name);
    }
    Ok(())
}

fn delete(item: &Item, path: &Path, roots: &Roots) -> Result<()> {
    path_check(item, path, roots)?;
    remove(path)?;
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

fn remove(path: &Path) -> io::Result<()> {
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
    fn ids_match_whole_segments() {
        assert!(hit("caches", "caches"));
        assert!(hit("caches/Homebrew", "caches"));
        assert!(hit("dev:.terraform:eks", "dev:.terraform"));
        assert!(!hit("caches-old", "caches"));
        assert!(!hit("dev:.terraformx", "dev:.terraform"));
    }

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
