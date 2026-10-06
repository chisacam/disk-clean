use crate::fmt::{pad, rpad, shorten, size, tilde};
use crate::rules::Safety;
use crate::scan::Item;
use crate::system::System;
use crate::whitelist::Whitelist;
use console::{Color, style};
use std::cmp::Reverse;
use std::path::Path;
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

/// Items smaller than this are hidden unless `--all` is given.
const MIN_SHOW: u64 = 10 << 20;
const SIZE_WIDTH: usize = 9;

pub const FDA_HELP: &str = "시스템 설정 › 개인정보 보호 및 보안 › 전체 디스크 접근 권한에서 지금 쓰는 터미널 앱을 켜세요.";

fn color(s: Safety) -> Color {
    match s {
        Safety::Safe => Color::Green,
        Safety::Regenerable => Color::Yellow,
        Safety::Leftover => Color::Magenta,
        Safety::Review => Color::Blue,
    }
}

pub fn print_scan(items: &[Item], sys: &System, wl: &Whitelist, home: &Path, all: bool, elapsed: Duration) {
    if let Some(d) = &sys.disk {
        println!(
            "{}  {} 중 {} 사용 · {} 남음  {}",
            style("디스크").bold(),
            size(d.total),
            size(d.total.saturating_sub(d.free)),
            size(d.free),
            style(sys.disk_basis).dim()
        );
    }
    if sys.fda == Some(false) {
        println!("{}", style("전체 디스크 접근 권한 없음 — 휴지통·메일·메시지·Safari 는 측정되지 않습니다.").yellow());
        println!("  {}", style(FDA_HELP).dim());
    }

    for w in &wl.warnings {
        println!("{}", style(format!("whitelist 경고: {w}")).yellow());
    }
    let width = console::Term::stdout().size_checked().map(|(_, cols)| cols as usize);
    let mut hidden = 0;
    for safety in [Safety::Safe, Safety::Regenerable, Safety::Leftover, Safety::Review] {
        let section: Vec<&Item> = items.iter().filter(|i| i.safety == safety).collect();
        let shown: Vec<&Item> = section.iter().copied().filter(|i| all || i.usage.bytes >= MIN_SHOW).collect();
        hidden += section.len() - shown.len();
        if shown.is_empty() {
            continue;
        }
        // Review items overlap (containers include their caches), so they get no total.
        let total = match safety {
            Safety::Review => String::new(),
            _ => format!("  합계 {}", size(section.iter().map(|i| i.usage.bytes).sum())),
        };
        println!();
        println!("{}{}", style(format!("■ {}", safety.title())).bold().fg(color(safety)), style(total).bold());
        // `clean` refuses review items, so their ids are not worth a column.
        let id_width = match safety {
            Safety::Review => 0,
            _ => shown.iter().map(|i| i.id.width()).max().unwrap_or(0).min(44),
        };
        let indent = 4 + SIZE_WIDTH + 2 + id_width + 2;
        let room = width.map_or(usize::MAX, |w| w.saturating_sub(indent).max(30));
        for group in groups(&shown) {
            println!("  {}  {}", style(&group[0].group).bold(), style(group[0].about).dim());
            for item in group {
                let id = match safety {
                    Safety::Review => String::new(),
                    _ => pad(&item.id, id_width) + "  ",
                };
                println!(
                    "    {}  {}{}",
                    rpad(&size(item.usage.bytes), SIZE_WIDTH),
                    style(id).cyan(),
                    shorten(&item.shown, room)
                );
                for note in &item.notes {
                    println!("{}{}", " ".repeat(indent), style(note).yellow());
                }
                if item.cleanable && item.usage.unreadable > 0 {
                    println!("{}{}", " ".repeat(indent), style(short_note(item.usage.unreadable)).yellow());
                }
            }
        }
    }

    println!();
    println!("{}", style("■ 시스템 — 정보만, 지우지 않음").bold());
    for e in &sys.entries {
        let value = match (e.count, e.usage) {
            (Some(n), _) => format!("{n}개"),
            (None, Some(u)) if u.bytes == 0 && u.unreadable > 0 => "읽지 못함".into(),
            (None, Some(u)) => size(u.bytes),
            (None, None) => "확인 못 함".into(),
        };
        let mut note = String::new();
        if let Some(p) = &e.path
            && p.to_str() != Some(e.label)
        {
            note = format!("{} — ", tilde(p, home));
        }
        note += e.note;
        if e.usage.is_some_and(|u| u.unreadable > 0 && u.bytes > 0) {
            note += " (일부를 읽지 못해 실제는 더 큼)";
        }
        println!("  {}  {}  {}", pad(e.label, 14), rpad(&value, SIZE_WIDTH), style(note).dim());
    }

    println!();
    let unreadable: u64 = items.iter().map(|i| i.usage.unreadable).sum();
    if unreadable > 0 {
        println!("{}", style(format!("읽지 못한 곳 {unreadable}곳 — 그만큼 실제보다 적게 잡혔을 수 있습니다.")).yellow());
    }
    let protected: usize = items.iter().map(|i| i.protected).sum();
    if protected > 0 {
        println!("{}", style(format!("whitelist 로 보호해 뺀 곳 {protected}곳 (disk-clean whitelist 로 확인)")).dim());
    }
    if hidden > 0 {
        println!("{}", style(format!("10 MiB 미만 항목 {hidden}개는 숨김 (--all)")).dim());
    }
    println!(
        "{}",
        style(format!(
            "측정 {:.1}초 · 지우기: disk-clean clean (목록에서 고르기) 또는 disk-clean clean <ID>…  · 미리 보기: --dry-run",
            elapsed.as_secs_f64()
        ))
        .dim()
    );
}

pub fn short_note(unreadable: u64) -> String {
    format!("⚠ 읽지 못한 곳 {unreadable}곳 — 실제 크기는 이보다 큼")
}

pub fn scan_json(items: &[Item], sys: &System, wl: &Whitelist, elapsed: Duration) -> serde_json::Value {
    let mut sorted: Vec<&Item> = items.iter().collect();
    sorted.sort_by_key(|i| (i.safety, Reverse(i.usage.bytes)));
    serde_json::json!({
        "schema": "disk-clean/scan/v1",
        "disk": sys.disk.as_ref().map(|d| serde_json::json!({
            "total": d.total,
            "free": d.free,
            "used": d.total.saturating_sub(d.free),
            "basis": sys.disk_basis,
        })),
        "full_disk_access": sys.fda,
        "items": sorted.iter().map(|i| serde_json::json!({
            "id": i.id,
            "group": i.group,
            "about": i.about,
            "safety": i.safety,
            "cleanable": i.cleanable,
            "bytes": i.usage.bytes,
            "files": i.usage.files,
            "unreadable": i.usage.unreadable,
            "shown": i.shown,
            "notes": i.notes,
            "paths": i.paths,
            "protected": i.protected,
        })).collect::<Vec<_>>(),
        "whitelist": {
            "file": wl.file,
            "patterns": wl.patterns,
            "warnings": wl.warnings,
        },
        "system": sys.entries.iter().map(|e| serde_json::json!({
            "label": e.label,
            "path": e.path,
            "bytes": e.usage.map(|u| u.bytes),
            "unreadable": e.usage.map(|u| u.unreadable),
            "count": e.count,
            "note": e.note,
        })).collect::<Vec<_>>(),
        "unreadable": items.iter().map(|i| i.usage.unreadable).sum::<u64>(),
        "elapsed_ms": elapsed.as_millis() as u64,
    })
}

pub fn print_whitelist(wl: &Whitelist, changed: Option<(&str, Vec<String>)>, json: bool, home: &Path) {
    if json {
        let mut out = serde_json::json!({
            "schema": "disk-clean/whitelist/v1",
            "file": wl.file,
            "patterns": wl.patterns,
            "warnings": wl.warnings,
        });
        if let Some((what, lines)) = &changed {
            out[if *what == "추가" { "added" } else { "removed" }] = serde_json::json!(lines);
        }
        println!("{out}");
        return;
    }
    if let Some((what, lines)) = &changed {
        if lines.is_empty() {
            println!("바뀐 것 없음 (이미 있음)");
        }
        for l in lines {
            println!("{} {l}", style(format!("{what}:")).green());
        }
        println!();
    }
    println!("{}  {}", style("whitelist").bold(), style(tilde(&wl.file, home)).dim());
    for p in &wl.patterns {
        match p.builtin {
            Some(why) => println!("  {}  {}", pad(&p.raw, 44), style(format!("기본 보호 — {why}")).dim()),
            None => println!("  {}", p.raw),
        }
    }
    if wl.patterns.iter().all(|p| p.builtin.is_some()) {
        println!("  {}", style("(직접 추가한 것 없음 — disk-clean whitelist add <경로>)").dim());
    }
    for w in &wl.warnings {
        println!("{}", style(format!("경고: {w}")).yellow());
    }
}

/// Items grouped by heading, biggest group first.
fn groups<'a>(items: &[&'a Item]) -> Vec<Vec<&'a Item>> {
    let mut out: Vec<Vec<&Item>> = Vec::new();
    for &item in items {
        match out.iter_mut().find(|g| g[0].group == item.group) {
            Some(g) => g.push(item),
            None => out.push(vec![item]),
        }
    }
    for g in &mut out {
        g.sort_by_key(|i| Reverse(i.usage.bytes));
    }
    out.sort_by_key(|g| Reverse(g.iter().map(|i| i.usage.bytes).sum::<u64>()));
    out
}
