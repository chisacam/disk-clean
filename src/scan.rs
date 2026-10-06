//! Measures every known location and turns it into items that can be shown or cleaned.

use crate::dev::{self, Found, Kind};
use crate::fmt::{date, size, tilde};
use crate::orphans::{self, Orphan};
use crate::rules::{self, Mode, Rule, Safety};
use crate::walk::{Seen, Usage, list, lstat, measure};
use crate::Refused;
use anyhow::Result;
use rayon::prelude::*;
use std::cmp::Reverse;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Entries at least this big get their own line, so one bulky cache can be picked alone.
const SPLIT_MIN: u64 = 256 << 20;
/// Review entries below this are folded into one line.
const REPORT_MIN: u64 = 1 << 30;
/// Project artifacts at least this big are listed one by one.
const ARTIFACT_MIN: u64 = 1 << 30;

#[derive(Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Opts {
    pub dev_roots: Vec<PathBuf>,
    pub no_dev: bool,
    pub older_than: Option<u64>,
}

pub struct Item {
    /// What `clean` accepts on the command line.
    pub id: String,
    /// Heading the item is listed under.
    pub group: String,
    pub about: &'static str,
    pub safety: Safety,
    pub cleanable: bool,
    pub preselect: bool,
    pub shown: String,
    /// Removed as a whole by `clean`.
    pub paths: Vec<PathBuf>,
    pub usage: Usage,
    pub notes: Vec<String>,
    /// Checked again right before deletion.
    pub recheck: Recheck,
}

pub enum Recheck {
    None,
    /// Each path must still look like this kind of build output.
    Artifact(&'static Kind),
    /// The container must still have no app.
    Leftover(PathBuf),
}

pub fn scan(home: &Path, opts: &Opts) -> Vec<Item> {
    let seen = Seen::default();
    let leftovers = orphans::find(home, &seen);
    // Shown once, as leftovers, not again under containers or their caches.
    let taken: Vec<PathBuf> = leftovers.iter().map(|o| o.container.clone()).collect();
    let (mut items, dev) = rayon::join(
        || rule_items(home, &seen, &taken),
        || if opts.no_dev { Vec::new() } else { dev_items(home, opts, &seen) },
    );
    items.extend(leftovers.iter().map(|o| leftover_item(home, o)));
    items.extend(dev);
    items
}

fn leftover_item(home: &Path, o: &Orphan) -> Item {
    let mut paths = vec![o.container.clone()];
    let mut usage = o.usage;
    let mut notes = vec![format!(
        "앱 {} 없음 · LaunchServices·Spotlight 에도 없음 · 메타데이터 갱신 {}",
        o.app.display(),
        o.last_used.map_or_else(|| "?".into(), date)
    )];
    for (path, u) in &o.extra {
        paths.push(path.clone());
        usage.add(*u);
        if u.bytes > 0 {
            notes.push(format!("+ {} ({})", tilde(path, home), size(u.bytes)));
        }
    }
    Item {
        id: format!("leftover/{}", o.id),
        group: "지운 앱의 컨테이너".into(),
        about: "앱을 지워도 macOS 는 데이터를 남김. 받은 리소스·설정·로컬 세이브가 함께 지워짐",
        safety: Safety::Leftover,
        cleanable: true,
        preselect: false,
        shown: tilde(&o.container, home),
        paths,
        usage,
        notes,
        recheck: Recheck::Leftover(o.container.clone()),
    }
}

fn hit(id: &str, arg: &str) -> bool {
    id == arg || id.strip_prefix(arg).is_some_and(|r| r.starts_with('/') || r.starts_with(':'))
}

/// The cleanable items named by `ids`; an id also names everything below it
/// (`caches` takes `caches/Homebrew`, `dev:.terraform` takes `dev:.terraform:eks`).
pub fn select<'a>(items: &'a [Item], ids: &[String]) -> Result<Vec<&'a Item>> {
    let live = |i: &&Item| i.usage.bytes > 0 && !i.paths.is_empty();
    let mut out: Vec<&Item> = Vec::new();
    for arg in ids {
        let found: Vec<&Item> = items.iter().filter(live).filter(|i| i.cleanable && hit(&i.id, arg)).collect();
        if found.is_empty() {
            if items.iter().any(|i| !i.cleanable && hit(&i.id, arg)) {
                return Err(Refused(format!("`{arg}` 는 직접 판단할 항목이라 지우지 않습니다. 앱 안에서 정리하세요.")).into());
            }
            return Err(Refused(format!(
                "`{arg}` 에 해당하는 항목이 없습니다(비어 있거나 ID 가 다름). `disk-clean scan` 으로 확인하세요."
            ))
            .into());
        }
        for item in found {
            if !out.iter().any(|o| std::ptr::eq(*o, item)) {
                out.push(item);
            }
        }
    }
    Ok(out)
}

struct Unit {
    label: Option<String>,
    shown: PathBuf,
    paths: Vec<PathBuf>,
}

/// Resolves a home-relative pattern, returning each match with the name `*` stood for.
fn expand(home: &Path, pattern: &str) -> Vec<(PathBuf, Option<String>)> {
    let mut found = vec![(home.to_path_buf(), None)];
    for part in pattern.split('/') {
        found = found
            .into_iter()
            .flat_map(|(dir, star)| {
                if part == "*" {
                    entries(&dir)
                        .into_iter()
                        .filter(|(p, _)| p.is_dir())
                        .map(|(p, name)| (p, Some(name)))
                        .collect::<Vec<_>>()
                } else {
                    vec![(dir.join(part), star)]
                }
            })
            .collect();
    }
    // A symlinked target is skipped rather than followed.
    found.retain(|(p, _)| lstat(p).is_ok_and(|m| m.is_dir() || m.is_file()));
    found
}

/// Entries of a directory without symlinks, since removing a link frees nothing.
fn entries(dir: &Path) -> Vec<(PathBuf, String)> {
    let Ok(entries) = list(dir) else { return Vec::new() };
    entries
        .into_iter()
        .filter(|e| e.file_type().is_ok_and(|t| !t.is_symlink()))
        .map(|e| (e.path(), e.file_name().to_string_lossy().into_owned()))
        .collect()
}

fn units(home: &Path, rule: &Rule, taken: &[PathBuf]) -> (Vec<Unit>, usize) {
    let mut out = Vec::new();
    let mut excluded = 0;
    let skip = |name: &str| rule.exclude.prefixes.iter().any(|p| name.starts_with(p));
    let is_taken = |p: &Path| taken.iter().any(|t| p.starts_with(t));
    for pattern in rule.targets {
        for (target, star) in expand(home, pattern) {
            if is_taken(&target) {
                continue;
            }
            match (rule.mode, star) {
                (Mode::Whole | Mode::ReportSelf, _) => out.push(Unit {
                    label: None,
                    shown: target.clone(),
                    paths: vec![target],
                }),
                (Mode::Contents, Some(name)) => {
                    if skip(&name) {
                        excluded += 1;
                        continue;
                    }
                    let paths = entries(&target).into_iter().map(|(p, _)| p).collect();
                    out.push(Unit { label: Some(name), shown: target, paths });
                }
                (Mode::Contents | Mode::ReportChildren, _) => {
                    for (path, name) in entries(&target) {
                        if is_taken(&path) {
                            continue;
                        }
                        if skip(&name) {
                            excluded += 1;
                            continue;
                        }
                        out.push(Unit { label: Some(name), shown: path.clone(), paths: vec![path] });
                    }
                }
            }
        }
    }
    (out, excluded)
}

fn rule_items(home: &Path, seen: &Seen, taken: &[PathBuf]) -> Vec<Item> {
    let rules: Vec<&'static Rule> = rules::rules().collect();
    let expanded: Vec<_> = rules.into_par_iter().map(|r| (r, units(home, r, taken))).collect();
    expanded
        .into_par_iter()
        .flat_map_iter(|(rule, (units, excluded))| {
            let measured: Vec<(Unit, Usage)> = units
                .into_par_iter()
                .map(|u| {
                    let usage = u.paths.par_iter().map(|p| measure(p, seen)).sum();
                    (u, usage)
                })
                .collect();
            group(home, rule, measured, excluded)
        })
        .collect()
}

fn group(home: &Path, rule: &'static Rule, mut measured: Vec<(Unit, Usage)>, excluded: usize) -> Vec<Item> {
    let item = |id: String, shown: String| Item {
        id,
        group: rule.name.to_string(),
        about: rule.about,
        safety: rule.safety,
        cleanable: rule.cleanable(),
        preselect: rule.preselect,
        shown,
        paths: Vec::new(),
        usage: Usage::default(),
        notes: Vec::new(),
        recheck: Recheck::None,
    };
    measured.sort_by_key(|(_, u)| Reverse(u.bytes));
    let mut items = Vec::new();
    match rule.mode {
        Mode::Whole => {
            if measured.is_empty() {
                return items;
            }
            let mut shown = tilde(&measured[0].0.shown, home);
            if measured.len() > 1 {
                shown += &format!(" 외 {}곳", measured.len() - 1);
            }
            let mut it = item(rule.id.into(), shown);
            for (unit, usage) in measured {
                it.paths.extend(unit.paths);
                it.usage.add(usage);
            }
            items.push(it);
        }
        Mode::ReportSelf => {
            for (unit, usage) in measured {
                let leaf = unit.shown.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let mut it = item(format!("{}/{leaf}", rule.id), tilde(&unit.shown, home));
                it.paths = unit.paths;
                it.usage = usage;
                items.push(it);
            }
        }
        Mode::Contents | Mode::ReportChildren => {
            let min = if rule.cleanable() { SPLIT_MIN } else { REPORT_MIN };
            let (big, rest): (Vec<_>, Vec<_>) = measured.into_iter().partition(|(_, u)| u.bytes >= min);
            for (unit, usage) in big {
                let label = unit.label.unwrap_or_default();
                let mut it = item(format!("{}/{label}", rule.id), tilde(&unit.shown, home));
                if let Some(h) = rules::hint(&label) {
                    it.notes.push(h.to_string());
                }
                it.paths = unit.paths;
                it.usage = usage;
                items.push(it);
            }
            if !rest.is_empty() || excluded > 0 {
                let place = format!("~/{}", rule.targets[0]);
                let shown = if items.is_empty() { place } else { format!("그 외 {}개 · {place}", rest.len()) };
                let mut it = item(rule.id.into(), shown);
                for (unit, usage) in rest {
                    // Nothing to free, and usually not removable either (TCC).
                    if usage.bytes > 0 {
                        it.paths.extend(unit.paths);
                    }
                    it.usage.add(usage);
                }
                if excluded > 0 {
                    let prefixes = rule.exclude.prefixes.iter().map(|p| format!("{p}*")).collect::<Vec<_>>().join(", ");
                    it.notes.push(format!("{prefixes} {excluded}개는 건드리지 않음 ({})", rule.exclude.why));
                }
                items.push(it);
            }
        }
    }
    items
}

fn dev_items(home: &Path, opts: &Opts, seen: &Seen) -> Vec<Item> {
    let roots = if opts.dev_roots.is_empty() { dev::default_roots(home) } else { opts.dev_roots.clone() };
    let mut found = dev::find(&roots, seen);
    if let Some(days) = opts.older_than {
        let cutoff = SystemTime::now() - Duration::from_secs(days * 86_400);
        found.retain(|f| f.modified.is_some_and(|m| m < cutoff));
    }
    found.sort_by_key(|f| Reverse(f.usage.bytes));

    let mut items = Vec::new();
    let mut ids = HashSet::new();
    for kind in dev::KINDS {
        let mine: Vec<&Found> = found.iter().filter(|f| std::ptr::eq(f.kind, kind)).collect();
        if mine.is_empty() {
            continue;
        }
        let item = |id: String, shown: String| Item {
            id,
            group: kind.name.to_string(),
            about: kind.about,
            safety: Safety::Regenerable,
            cleanable: true,
            preselect: false,
            shown,
            paths: Vec::new(),
            usage: Usage::default(),
            notes: Vec::new(),
            recheck: Recheck::Artifact(kind),
        };
        let (big, rest): (Vec<&Found>, Vec<&Found>) = mine.into_iter().partition(|f| f.usage.bytes >= ARTIFACT_MIN);
        for f in big {
            let project = f.path.parent().and_then(Path::file_name).unwrap_or_default().to_string_lossy();
            let base = format!("dev:{}:{project}", kind.name);
            let mut id = base.clone();
            let mut n = 2;
            while !ids.insert(id.clone()) {
                id = format!("{base}-{n}");
                n += 1;
            }
            let mut it = item(id, tilde(&f.path, home));
            it.notes = notes(f);
            it.paths = vec![f.path.clone()];
            it.usage = f.usage;
            items.push(it);
        }
        if let Some(largest) = rest.first() {
            let shown = format!(
                "{}곳 · 가장 큰 곳 {} ({})",
                rest.len(),
                tilde(&largest.path, home),
                size(largest.usage.bytes)
            );
            let mut it = item(format!("dev:{}", kind.name), shown);
            let nested: usize = rest.iter().map(|f| f.nested).sum();
            if nested > 0 {
                it.notes.push(format!("안에 중첩된 .terraform {nested}개"));
            }
            let orphans = rest.iter().filter(|f| f.orphan).count();
            if orphans > 0 {
                it.notes.push(format!("그중 {orphans}곳은 옆에 .tf 파일이 없음 — 코드를 지운 뒤 남은 것"));
            }
            for f in rest {
                it.paths.push(f.path.clone());
                it.usage.add(f.usage);
            }
            items.push(it);
        }
    }
    items
}

fn notes(f: &Found) -> Vec<String> {
    let mut out = Vec::new();
    if f.nested > 0 {
        out.push(format!(
            "⚠ 안에 중첩된 .terraform {}개 — 모듈 clone 안에서 terraform init 이 실행된 흔적",
            f.nested
        ));
    }
    if f.orphan {
        out.push("옆에 .tf 파일이 없음 — 코드를 지운 뒤 남은 것".to_string());
    }
    if let Some(ws) = &f.workspace {
        out.push(format!("workspace '{ws}' 가 선택돼 있음 — 지우면 default 로 돌아감"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn ids_match_whole_segments() {
        assert!(hit("caches", "caches"));
        assert!(hit("caches/Homebrew", "caches"));
        assert!(hit("dev:.terraform:eks", "dev:.terraform"));
        assert!(!hit("caches-old", "caches"));
        assert!(!hit("dev:.terraformx", "dev:.terraform"));
    }


    #[test]
    fn expand_matches_star_and_skips_symlinks() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path();
        fs::create_dir_all(home.join("C/a/Data/Caches")).unwrap();
        fs::create_dir_all(home.join("C/b/Data")).unwrap();
        fs::create_dir_all(home.join("elsewhere/Data/Caches")).unwrap();
        std::os::unix::fs::symlink(home.join("elsewhere"), home.join("C/c")).unwrap();
        let got = expand(home, "C/*/Data/Caches");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1.as_deref(), Some("a"));
    }
}
