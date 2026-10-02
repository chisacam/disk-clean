use crate::fmt::{pad, rpad, shorten, size, tilde};
use crate::rules::Safety;
use crate::scan::Item;
use crate::system::System;
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

pub fn print_scan(items: &[Item], sys: &System, home: &Path, all: bool, elapsed: Duration) {
    if let Some(d) = &sys.disk {
        println!(
            "{}  {} 중 {} 사용 · {} 남음  {}",
            style("디스크").bold(),
            size(d.total),
            size(d.total.saturating_sub(d.free)),
            size(d.free),
            style("APFS 컨테이너 기준 — macOS·Preboot·Recovery 볼륨 포함").dim()
        );
    }
    if sys.fda == Some(false) {
        println!("{}", style("전체 디스크 접근 권한 없음 — 휴지통·메일·메시지·Safari 는 측정되지 않습니다.").yellow());
        println!("  {}", style(FDA_HELP).dim());
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
    let snapshots = match sys.snapshots {
        Some(n) => format!("{n}개"),
        None => "확인 못 함".into(),
    };
    println!("  {}  {}  {}", pad("로컬 스냅샷", 14), rpad(&snapshots, SIZE_WIDTH), style("공간이 모자라면 macOS 가 알아서 지움").dim());
    println!("  {}  {}  {}", pad("/private/var/vm", 14), rpad(&size(sys.vm.bytes), SIZE_WIDTH), style("절전 이미지·스왑 파일 — 정상").dim());
    if let Some((path, usage)) = &sys.temp {
        println!(
            "  {}  {}  {}",
            pad("임시 폴더", 14),
            rpad(&size(usage.bytes), SIZE_WIDTH),
            style(format!("{} — 재부팅 때 macOS 가 정리", tilde(path, home))).dim()
        );
    }

    println!();
    let unreadable: u64 = items.iter().map(|i| i.usage.unreadable).sum();
    if unreadable > 0 {
        println!("{}", style(format!("읽지 못한 곳 {unreadable}곳 — 그만큼 실제보다 적게 잡혔을 수 있습니다.")).yellow());
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
