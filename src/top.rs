//! The biggest folders and files under a path, to find what nothing else names.

use crate::fmt::{rpad, size, tilde};
use crate::rules;
use crate::walk::{Node, Seen, tree};
use console::style;
use std::path::Path;
use std::time::Instant;

pub fn run(path: &Path, keep: usize, depth: usize, home: &Path) {
    let start = Instant::now();
    let root = tree(path, depth.max(1), keep, &Seen::default());
    println!(
        "{}  {} · 파일 {}개 · {:.1}초",
        style(tilde(path, home)).bold(),
        size(root.usage.bytes),
        root.usage.files,
        start.elapsed().as_secs_f64()
    );
    // Below the first level, hide what is under 0.5% of the whole.
    print(&root, 1, root.usage.bytes, root.usage.bytes / 200);
    if root.usage.unreadable > 0 {
        println!(
            "{}",
            style(format!("읽지 못한 곳 {}곳 — 실제는 더 클 수 있습니다.", root.usage.unreadable)).yellow()
        );
        println!("  {}", style(crate::report::FDA_HELP).dim());
    }
}

fn print(node: &Node, level: usize, total: u64, min: u64) {
    let indent = "  ".repeat(level);
    for child in &node.children {
        if level > 1 && child.usage.bytes < min {
            continue;
        }
        let pct = child.usage.bytes as f64 * 100.0 / total.max(1) as f64;
        let name = if child.is_dir { format!("{}/", child.name) } else { child.name.clone() };
        let hint = rules::hint(&child.name).map(|h| format!("  {}", style(h).dim())).unwrap_or_default();
        println!("{indent}{} {:>5.1}%  {name}{hint}", rpad(&size(child.usage.bytes), 9), pct);
        print(child, level + 1, total, min);
    }
    let (n, bytes) = node.rest;
    if n > 0 && (level == 1 || bytes >= min) {
        println!("{indent}{}         {}", rpad(&size(bytes), 9), style(format!("그 외 {n}개")).dim());
    }
}
