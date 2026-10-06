//! Plans: what will be deleted, written down before anything is.
//!
//! An agent makes a plan and shows it to a person; the person approves
//! `disk-clean apply <id>`, and apply deletes exactly what the plan lists.
//! Anyone who can run commands can also edit the plan file, so apply trusts
//! none of it. The id must still be the hash of the content, every path must
//! be something a fresh scan offers for cleaning under the same category, and
//! each path must be the same file (device and inode) and not much bigger
//! than when it was planned.

use crate::fmt::{rpad, size, tilde};
use crate::guard::{self, Roots};
use crate::rules::Safety;
use crate::scan::{self, Item, Opts};
use crate::state::{self, Deleted, Entry, Skipped};
use crate::system;
use crate::walk::{Seen, lstat, measure};
use crate::{Outcome, Refused};
use anyhow::{Context, Result, anyhow, ensure};
use console::style;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub const SCHEMA: &str = "disk-clean/plan/v1";
/// How long a plan may wait for approval.
const LIFETIME: u64 = 60 * 60;
/// Growth allowed between plan and apply, at least: caches keep filling while a person reads.
const GROWTH_MIN: u64 = 64 << 20;
/// Paths listed per item in human output.
const SHOW_PATHS: usize = 20;

#[derive(Serialize, Deserialize, Clone)]
pub struct Plan {
    pub schema: String,
    /// Hash of everything else in the plan.
    pub id: String,
    pub created_at: u64,
    pub expires_at: u64,
    pub home: PathBuf,
    /// How apply scans again to confirm each path.
    pub scan: Opts,
    pub items: Vec<PlanItem>,
    pub total_bytes: u64,
    /// Paths of the chosen items that already failed the checks; never deleted.
    pub refused: Vec<Skipped>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PlanItem {
    pub id: String,
    pub group: String,
    pub safety: Safety,
    pub notes: Vec<String>,
    pub bytes: u64,
    pub paths: Vec<PlanPath>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PlanPath {
    pub path: PathBuf,
    pub bytes: u64,
    pub files: u64,
    /// Entries that could not be read: the size is a lower bound.
    pub unreadable: u64,
    pub dev: u64,
    pub ino: u64,
}

fn fingerprint(path: &Path) -> Result<PlanPath> {
    ensure!(path.to_str().is_some(), "경로에 UTF-8 이 아닌 문자가 있어 건너뜀");
    let md = lstat(path)?;
    let usage = measure(path, &Seen::default());
    Ok(PlanPath {
        path: path.to_path_buf(),
        bytes: usage.bytes,
        files: usage.files,
        unreadable: usage.unreadable,
        dev: md.dev(),
        ino: md.ino(),
    })
}

fn hash(plan: &Plan) -> String {
    let mut body = plan.clone();
    body.id.clear();
    let bytes = serde_json::to_vec(&body).expect("a plan always serializes");
    format!("{:012x}", state::fnv64(&bytes) & 0xffff_ffff_ffff)
}

fn skipped(path: &Path, e: &anyhow::Error) -> Skipped {
    Skipped { path: path.to_path_buf(), reason: format!("{e:#}") }
}

/// Checks the chosen items and measures each path. Nothing is written.
pub fn make(selected: &[&Item], opts: &Opts, home: &Path) -> Plan {
    let roots = Roots::new(home);
    let mut items = Vec::new();
    let mut refused = Vec::new();
    for item in selected {
        if let Err(e) = guard::item_check(item) {
            refused.extend(item.paths.iter().map(|p| skipped(p, &e)));
            continue;
        }
        let checked: Vec<Result<PlanPath, Skipped>> = item
            .paths
            .par_iter()
            .map(|p| guard::path_check(item, p, &roots).and_then(|()| fingerprint(p)).map_err(|e| skipped(p, &e)))
            .collect();
        let mut paths = Vec::new();
        for c in checked {
            match c {
                Ok(p) => paths.push(p),
                Err(s) => refused.push(s),
            }
        }
        if !paths.is_empty() {
            items.push(PlanItem {
                id: item.id.clone(),
                group: item.group.clone(),
                safety: item.safety,
                notes: item.notes.clone(),
                bytes: paths.iter().map(|p| p.bytes).sum(),
                paths,
            });
        }
    }
    let created_at = state::now();
    let mut plan = Plan {
        schema: SCHEMA.into(),
        id: String::new(),
        created_at,
        expires_at: created_at + LIFETIME,
        home: home.to_path_buf(),
        scan: opts.clone(),
        total_bytes: items.iter().map(|i| i.bytes).sum(),
        items,
        refused,
    };
    plan.id = hash(&plan);
    plan
}

pub fn save(plan: &Plan, home: &Path) -> Result<PathBuf> {
    let dir = state::plans(home);
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", plan.id));
    let tmp = dir.join(format!(".{}.tmp", plan.id));
    fs::write(&tmp, serde_json::to_vec_pretty(plan)?)?;
    fs::rename(&tmp, &path)?;
    Ok(path)
}

pub fn print(plan: &Plan, home: &Path) {
    let count: usize = plan.items.iter().map(|i| i.paths.len()).sum();
    println!(
        "{}  {} · 경로 {count}개",
        style(format!("계획 {}", plan.id)).bold(),
        size(plan.total_bytes)
    );
    for item in &plan.items {
        println!("  {}  {}  {}", rpad(&size(item.bytes), 9), style(&item.id).cyan(), style(item.safety.tag()).dim());
        for note in &item.notes {
            println!("               {}", style(note).yellow());
        }
        let unreadable: u64 = item.paths.iter().map(|p| p.unreadable).sum();
        if unreadable > 0 {
            println!("               {}", style(crate::report::short_note(unreadable)).yellow());
        }
        for p in item.paths.iter().take(SHOW_PATHS) {
            println!("               {}", style(tilde(&p.path, home)).dim());
        }
        if item.paths.len() > SHOW_PATHS {
            println!("               {}", style(format!("… 외 {}개", item.paths.len() - SHOW_PATHS)).dim());
        }
    }
    if !plan.refused.is_empty() {
        println!("  {}", style(format!("건너뜀 {}곳 (지우지 않음)", plan.refused.len())).red());
        for s in plan.refused.iter().take(SHOW_PATHS) {
            println!("               {}: {}", tilde(&s.path, home), style(&s.reason).dim());
        }
    }
}

/// Runs a saved plan once. The plan is consumed whatever happens, so a refused
/// or failed apply needs a fresh plan.
pub fn apply(id: &str, home: &Path, json: bool) -> Result<Outcome> {
    let id = id.trim();
    ensure!(
        !id.is_empty() && id.len() <= 32 && id.chars().all(|c| c.is_ascii_hexdigit()),
        Refused(format!("`{id}` 는 계획 ID 가 아닙니다"))
    );
    let dir = state::plans(home);
    let taken = dir.join(format!("{id}.applying"));
    // Taking the file first means two applies of one plan cannot both run.
    if let Err(e) = fs::rename(dir.join(format!("{id}.json")), &taken) {
        if e.kind() == io::ErrorKind::NotFound {
            return Err(Refused(format!("계획 {id} 가 없습니다. 이미 실행했거나 만든 적 없는 ID 입니다.")).into());
        }
        return Err(e).context("계획 파일을 가져오지 못했습니다");
    }
    let result = run(id, &taken, home, json);
    let used = dir.join("used");
    let _ = fs::create_dir_all(&used).and_then(|()| fs::rename(&taken, used.join(format!("{id}.json"))));
    result
}

fn load(id: &str, file: &Path, home: &Path) -> Result<Plan> {
    let plan: Plan = serde_json::from_slice(&fs::read(file)?)
        .map_err(|e| Refused(format!("계획 파일을 읽을 수 없습니다: {e}")))?;
    ensure!(plan.schema == SCHEMA, Refused(format!("계획 형식이 다릅니다: {}", plan.schema)));
    ensure!(
        plan.id == id && hash(&plan) == id,
        Refused("계획 파일이 만들어진 뒤 바뀌었습니다(ID 와 내용이 맞지 않음). 다시 plan 하세요.".into())
    );
    ensure!(state::now() <= plan.expires_at, Refused("계획이 만료됐습니다(1시간). 다시 plan 하세요.".into()));
    ensure!(plan.home == home, Refused(format!("다른 홈 폴더({})의 계획입니다", plan.home.display())));
    Ok(plan)
}

fn run(id: &str, file: &Path, home: &Path, json: bool) -> Result<Outcome> {
    let plan = load(id, file, home)?;
    if !json {
        eprintln!("{}", style("다시 재서 계획과 대조하는 중…").dim());
    }
    // A path is deleted only if the tool, scanning now, would still offer it.
    let current = scan::scan(home, &plan.scan);
    let by_path: HashMap<&Path, &Item> = current
        .iter()
        .filter(|i| i.cleanable)
        .flat_map(|i| i.paths.iter().map(move |p| (p.as_path(), i)))
        .collect();
    let mut owners: Vec<&Item> = Vec::new();
    for item in plan.items.iter().flat_map(|i| &i.paths).filter_map(|p| by_path.get(p.path.as_path())) {
        if !owners.iter().any(|o| std::ptr::eq(*o, *item)) {
            owners.push(item);
        }
    }
    let owner_ok: HashMap<&str, Result<(), String>> = owners
        .par_iter()
        .map(|i| (i.id.as_str(), guard::item_check(i).map_err(|e| format!("{e:#}"))))
        .collect();
    let roots = Roots::new(home);
    let planned: Vec<(&PlanItem, &PlanPath)> = plan.items.iter().flat_map(|i| i.paths.iter().map(move |p| (i, p))).collect();
    let verdicts: Vec<Result<u64>> = planned
        .par_iter()
        .map(|(item, p)| verify(item, p, &by_path, &owner_ok, &roots))
        .collect();

    let mut jobs = Vec::new();
    let mut refused = Vec::new();
    for ((_, p), verdict) in planned.iter().zip(verdicts) {
        match verdict {
            Ok(bytes) => jobs.push(Job { path: p.path.clone(), bytes: Some(bytes) }),
            Err(e) => refused.push(skipped(&p.path, &e)),
        }
    }
    let items = plan.items.iter().map(|i| i.id.clone()).collect();
    let report = execute("apply", Some(id), items, jobs, refused, plan.total_bytes, home);
    print_report(&report, json, home);
    Ok(report.outcome())
}

fn verify(
    item: &PlanItem,
    p: &PlanPath,
    by_path: &HashMap<&Path, &Item>,
    owner_ok: &HashMap<&str, Result<(), String>>,
    roots: &Roots,
) -> Result<u64> {
    // A fresh scan already leaves whitelisted paths out; name that reason rather than the generic one.
    let owner = match by_path.get(p.path.as_path()) {
        Some(owner) => owner,
        None => return Err(anyhow!(roots.whitelisted(&p.path).unwrap_or_else(|| "지금 다시 재 보니 지울 수 있는 항목이 아님".into()))),
    };
    ensure!(owner.safety == item.safety, "분류가 '{}' 에서 '{}' 로 바뀜", item.safety.tag(), owner.safety.tag());
    if let Some(Err(e)) = owner_ok.get(owner.id.as_str()) {
        return Err(anyhow!("{e}"));
    }
    guard::path_check(owner, &p.path, roots)?;
    let md = lstat(&p.path)?;
    ensure!((md.dev(), md.ino()) == (p.dev, p.ino), "계획 뒤에 다른 파일·폴더로 바뀜");
    let now = measure(&p.path, &Seen::default()).bytes;
    let limit = p.bytes + (p.bytes / 10).max(GROWTH_MIN);
    ensure!(now <= limit, "계획 뒤에 {} → {} 로 커짐. 다시 plan 하세요", size(p.bytes), size(now));
    Ok(now)
}

pub struct Job {
    pub path: PathBuf,
    pub bytes: Option<u64>,
}

pub struct Report {
    pub entry: Entry,
    pub planned_bytes: u64,
    /// Set when the deletion happened but could not be logged.
    pub audit_error: Option<String>,
    pub audit_log: PathBuf,
}

impl Report {
    fn deleted_bytes(&self) -> u64 {
        self.entry.deleted.iter().filter_map(|d| d.bytes).sum()
    }

    pub fn outcome(&self) -> Outcome {
        let e = &self.entry;
        match (e.deleted.is_empty(), e.refused.is_empty() && e.failed.is_empty()) {
            (false, true) => Outcome::Done,
            (true, _) if e.failed.is_empty() => Outcome::Refused,
            _ => Outcome::Partial,
        }
    }
}

/// Deletes the jobs, measures the free space around it, and logs what happened.
pub fn execute(
    action: &str,
    plan: Option<&str>,
    items: Vec<String>,
    jobs: Vec<Job>,
    refused: Vec<Skipped>,
    planned_bytes: u64,
    home: &Path,
) -> Report {
    let free_before = system::disk(home).map(|d| d.free);
    let results: Vec<(Job, io::Result<()>)> = jobs
        .into_par_iter()
        .map(|job| {
            let r = guard::remove(&job.path);
            (job, r)
        })
        .collect();
    let free_after = system::disk(home).map(|d| d.free);
    let mut deleted = Vec::new();
    let mut failed = Vec::new();
    for (job, r) in results {
        match r {
            Ok(()) => deleted.push(Deleted { path: job.path, bytes: job.bytes }),
            Err(e) => failed.push(Skipped { path: job.path, reason: e.to_string() }),
        }
    }
    let entry = Entry {
        time: state::now(),
        action: action.into(),
        by: state::caller(),
        plan: plan.map(str::to_string),
        items,
        deleted,
        refused,
        failed,
        free_before,
        free_after,
    };
    let dir = state::dir(home);
    let audit_error = state::record(&dir, &entry).err().map(|e| format!("{e:#}"));
    Report { entry, planned_bytes, audit_error, audit_log: dir.join("audit.jsonl") }
}

pub fn print_report(report: &Report, json: bool, home: &Path) {
    let e = &report.entry;
    let freed = match (e.free_before, e.free_after) {
        (Some(b), Some(a)) => Some(a as i128 - b as i128),
        _ => None,
    };
    if json {
        let out = serde_json::json!({
            "schema": format!("disk-clean/{}/v1", e.action),
            "plan": e.plan,
            "deleted": e.deleted,
            "refused": e.refused,
            "failed": e.failed,
            "planned_bytes": report.planned_bytes,
            "deleted_bytes": report.deleted_bytes(),
            "free_before": e.free_before,
            "free_after": e.free_after,
            "freed": freed.map(|f| f as i64),
            "audit_log": report.audit_log,
            "audit_error": report.audit_error,
        });
        println!("{out}");
        return;
    }
    let attempted = e.deleted.len() + e.failed.len();
    let label = match report.outcome() {
        Outcome::Done => style("완료:").green(),
        Outcome::Partial => style("일부 실패:").yellow(),
        Outcome::Refused => style("거부:").red(),
    };
    println!(
        "{} 경로 {}개를 지웠습니다 (시도 {attempted} · 검사에서 거부 {}).",
        label.bold(),
        e.deleted.len(),
        e.refused.len()
    );
    if let (Some(b), Some(a)) = (e.free_before, e.free_after) {
        println!(
            "  디스크 남은 공간 {} → {} (+{}, statfs 로 측정) · 지운 경로를 지우기 직전에 잰 크기 {}",
            size(b),
            size(a),
            size(a.saturating_sub(b)),
            size(report.deleted_bytes())
        );
    }
    for (title, list) in [("검사에서 거부", &e.refused), ("삭제 실패", &e.failed)] {
        if list.is_empty() {
            continue;
        }
        println!("{}", style(format!("{title} {}건", list.len())).red().bold());
        for s in list.iter().take(10) {
            println!("  {}: {}", tilde(&s.path, home), s.reason);
        }
        if list.len() > 10 {
            println!("  … 외 {}건", list.len() - 10);
        }
    }
    if e.failed.iter().any(|s| s.reason.contains("Operation not permitted")) {
        println!("  {}", style(crate::report::FDA_HELP).dim());
    }
    match &report.audit_error {
        Some(err) => println!("{}", style(format!("⚠ {err}")).red()),
        None => println!("{}", style(format!("기록: {}", tilde(&report.audit_log, home))).dim()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_in(home: &Path, expires_at: u64) -> Plan {
        let mut plan = Plan {
            schema: SCHEMA.into(),
            id: String::new(),
            created_at: 1,
            expires_at,
            home: home.to_path_buf(),
            scan: Opts::default(),
            items: vec![PlanItem {
                id: "caches".into(),
                group: "앱 캐시".into(),
                safety: Safety::Safe,
                notes: Vec::new(),
                bytes: 1,
                paths: vec![PlanPath {
                    path: home.join("Library/Caches/app"),
                    bytes: 1,
                    files: 1,
                    unreadable: 0,
                    dev: 1,
                    ino: 1,
                }],
            }],
            total_bytes: 1,
            refused: Vec::new(),
        };
        plan.id = hash(&plan);
        plan
    }

    fn refusal(r: Result<Plan>) -> String {
        let e = r.err().expect("refused");
        assert!(e.downcast_ref::<Refused>().is_some(), "{e:#}");
        format!("{e:#}")
    }

    #[test]
    fn load_accepts_only_the_plan_as_written() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path();
        let plan = plan_in(home, state::now() + 60);
        let file = save(&plan, home).unwrap();
        assert!(load(&plan.id, &file, home).is_ok());

        // Another path slipped in after the person saw the plan.
        let text = fs::read_to_string(&file).unwrap().replace("Library/Caches/app", "Documents/thesis");
        fs::write(&file, text).unwrap();
        assert!(refusal(load(&plan.id, &file, home)).contains("바뀌었습니다"));

        // Approved under one id, a different plan saved under it.
        let other = plan_in(home, state::now() + 120);
        fs::write(&file, serde_json::to_vec(&other).unwrap()).unwrap();
        assert!(refusal(load(&plan.id, &file, home)).contains("바뀌었습니다"));
    }

    #[test]
    fn load_refuses_expired_and_foreign_plans() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path();
        let old = plan_in(home, state::now() - 1);
        let file = save(&old, home).unwrap();
        assert!(refusal(load(&old.id, &file, home)).contains("만료"));

        let fresh = plan_in(home, state::now() + 60);
        let file = save(&fresh, home).unwrap();
        assert!(refusal(load(&fresh.id, &file, Path::new("/Users/someone-else"))).contains("다른 홈"));
    }

    #[test]
    fn apply_takes_a_plan_once_and_rejects_odd_ids() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path();
        let is_refused = |r: Result<Outcome>| r.err().is_some_and(|e| e.downcast_ref::<Refused>().is_some());
        assert!(is_refused(apply("../../etc/passwd", home, true)));
        assert!(is_refused(apply("abc123", home, true)), "no such plan");
        // An expired plan is refused and still consumed.
        let old = plan_in(home, state::now() - 1);
        save(&old, home).unwrap();
        assert!(is_refused(apply(&old.id, home, true)));
        assert!(!state::plans(home).join(format!("{}.json", old.id)).exists());
        assert!(state::plans(home).join(format!("used/{}.json", old.id)).exists());
        assert!(is_refused(apply(&old.id, home, true)), "second apply");
    }
}
