//! Deleting at a terminal: pick, see the plan, confirm.
//!
//! Without a terminal nothing is deleted here. Agents make a plan with
//! `disk-clean plan` and a person approves `disk-clean apply`.

use crate::fmt::{rpad, size};
use crate::plan::{self, Job};
use crate::scan::{self, Item, Opts};
use crate::{Outcome, Refused};
use anyhow::Result;
use console::style;
use dialoguer::{Confirm, MultiSelect};
use std::io::{self, IsTerminal};
use std::path::Path;

pub const NO_TERMINAL: &str = "터미널 밖에서는 clean 이 지우지 않습니다. `disk-clean plan <ID…>` 로 계획을 만들고, 사람이 확인한 뒤 `disk-clean apply <계획 ID>` 로 실행하세요.";

pub fn run(items: Vec<Item>, ids: &[String], dry_run: bool, opts: &Opts, home: &Path) -> Result<Outcome> {
    let terminal = io::stdin().is_terminal();
    if !dry_run && !terminal {
        return Err(Refused(NO_TERMINAL.into()).into());
    }
    let selected = if !ids.is_empty() {
        scan::select(&items, ids)?
    } else if terminal {
        pick(&items)?
    } else {
        return Err(Refused("지울 항목 ID 를 지정하세요. ID 는 `disk-clean scan` 에 나옵니다.".into()).into());
    };
    if selected.is_empty() {
        println!("고른 항목이 없습니다.");
        return Ok(Outcome::Done);
    }

    let plan = plan::make(&selected, opts, home);
    plan::print(&plan, home);
    if dry_run {
        println!("{}", style("dry-run: 아무것도 지우지 않았습니다.").green());
        return Ok(Outcome::Done);
    }
    if plan.items.is_empty() {
        return Err(Refused("고른 항목에 지울 수 있는 경로가 없습니다.".into()).into());
    }
    let go = Confirm::new()
        .with_prompt(format!("{} 를 삭제합니다. 되돌릴 수 없습니다. 계속할까요?", size(plan.total_bytes)))
        .default(false)
        .interact()?;
    if !go {
        println!("취소했습니다.");
        return Ok(Outcome::Done);
    }
    let jobs = plan
        .items
        .iter()
        .flat_map(|i| i.paths.iter().map(|p| Job { path: p.path.clone(), bytes: Some(p.bytes) }))
        .collect();
    let item_ids = plan.items.iter().map(|i| i.id.clone()).collect();
    let report = plan::execute("clean", None, item_ids, jobs, plan.refused.clone(), plan.total_bytes, home);
    plan::print_report(&report, false, home);
    Ok(report.outcome())
}

fn pick(items: &[Item]) -> Result<Vec<&Item>> {
    let mut sorted: Vec<&Item> = items
        .iter()
        .filter(|i| i.cleanable && i.usage.bytes > 0 && !i.paths.is_empty())
        .collect();
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
