#[cfg(not(unix))]
compile_error!("disk-clean supports macOS and Linux only.");

mod clean;
mod dev;
mod fmt;
mod guard;
mod orphans;
mod plan;
mod report;
mod rules;
mod scan;
mod state;
mod system;
mod top;
mod walk;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use console::style;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

/// 디스크를 무엇이 차지하는지(macOS 의 '시스템 데이터'·'문서' 포함) 보여 주고,
/// 다시 만들 수 있는 것만 골라 지웁니다. macOS·Linux 용.
///
/// 종료 코드: 0 성공 · 1 오류 · 2 일부 실패 · 3 거부(안전장치가 막음)
#[derive(Parser)]
#[command(name = "disk-clean", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// 알려진 위치를 재서 분류별로 보여 줍니다 (읽기 전용, 기본 명령)
    Scan {
        #[command(flatten)]
        scan: ScanArgs,
        /// 10 MiB 미만 항목도 보여 줍니다
        #[arg(long)]
        all: bool,
        /// 결과를 JSON 으로 (크기는 바이트)
        #[arg(long)]
        json: bool,
    },
    /// 큰 폴더·파일 순위를 보여 줍니다 (읽기 전용)
    Top {
        /// 기본: 홈 폴더
        path: Option<PathBuf>,
        /// 단계마다 보여 줄 개수
        #[arg(short = 'n', long, default_value_t = 15)]
        count: usize,
        /// 몇 단계까지 펼칠지
        #[arg(short, long, default_value_t = 1)]
        depth: usize,
        #[arg(long)]
        json: bool,
    },
    /// 지울 계획을 만들어 저장합니다. 아무것도 지우지 않습니다
    Plan {
        /// 지울 항목 ID (scan 에 표시). 상위 ID 는 그 아래 항목을 모두 포함
        #[arg(required = true)]
        ids: Vec<String>,
        #[command(flatten)]
        scan: ScanArgs,
        #[arg(long)]
        json: bool,
    },
    /// 계획을 실행합니다: 계획에 적힌 경로만, 다시 재서 확인한 뒤 지웁니다 (되돌릴 수 없음)
    Apply {
        /// plan 이 출력한 계획 ID
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// 지금까지 지운 기록을 보여 줍니다
    Log {
        /// 최근 몇 건
        #[arg(short = 'n', long, default_value_t = 20)]
        count: usize,
        #[arg(long)]
        json: bool,
    },
    /// 터미널에서 골라 지웁니다 (확인 후). 터미널이 아니면 지우지 않습니다
    Clean {
        /// 지울 항목 ID. 생략하면 목록에서 고릅니다
        ids: Vec<String>,
        /// 지울 경로만 보여 주고 아무것도 지우지 않습니다
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        scan: ScanArgs,
    },
}

impl Command {
    fn json(&self) -> bool {
        match self {
            Command::Scan { json, .. }
            | Command::Top { json, .. }
            | Command::Plan { json, .. }
            | Command::Apply { json, .. }
            | Command::Log { json, .. } => *json,
            Command::Clean { .. } => false,
        }
    }
}

#[derive(Args, Default)]
struct ScanArgs {
    /// 개발 산출물을 찾을 폴더 (여러 번 지정 가능). 기본: 홈에서 숨김 폴더·Library 등을 뺀 곳
    #[arg(long = "dev-root", value_name = "PATH")]
    dev_roots: Vec<PathBuf>,
    /// 개발 산출물(node_modules, target, .terraform …) 검색을 건너뜁니다
    #[arg(long)]
    no_dev: bool,
    /// 이 일수보다 오래 바뀌지 않은 개발 산출물만
    #[arg(long, value_name = "DAYS")]
    older_than: Option<u64>,
}

impl ScanArgs {
    fn opts(self) -> Result<scan::Opts> {
        let dev_roots = self
            .dev_roots
            .iter()
            .map(|p| std::path::absolute(p).with_context(|| format!("{} 를 찾을 수 없습니다", p.display())))
            .collect::<Result<_>>()?;
        Ok(scan::Opts { dev_roots, no_dev: self.no_dev, older_than: self.older_than })
    }
}

/// A safety check said no. Exit code 3, so a caller can tell it from a crash.
#[derive(Debug)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

pub enum Outcome {
    Done,
    /// Some paths were deleted or attempted, not all.
    Partial,
    /// Nothing was attempted: every path was refused.
    Refused,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.command.as_ref().is_some_and(Command::json);
    match run(cli) {
        Ok(Outcome::Done) => ExitCode::SUCCESS,
        Ok(Outcome::Partial) => ExitCode::from(2),
        Ok(Outcome::Refused) => ExitCode::from(3),
        Err(e) => {
            let refused = e.downcast_ref::<Refused>().is_some();
            if json {
                let out = serde_json::json!({
                    "schema": "disk-clean/error/v1",
                    "kind": if refused { "refused" } else { "error" },
                    "error": format!("{e:#}"),
                });
                println!("{out}");
            } else {
                let label = if refused { "거부:" } else { "오류:" };
                eprintln!("{} {e:#}", style(label).red().bold());
            }
            ExitCode::from(if refused { 3 } else { 1 })
        }
    }
}

fn measuring(json: bool) {
    if !json {
        eprintln!("{}", style("재는 중…").dim());
    }
}

fn run(cli: Cli) -> Result<Outcome> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME 이 설정돼 있지 않습니다")?);
    let command = cli.command.unwrap_or(Command::Scan { scan: ScanArgs::default(), all: false, json: false });
    match command {
        Command::Scan { scan, all, json } => {
            let start = Instant::now();
            measuring(json);
            let items = scan::scan(&home, &scan.opts()?);
            let sys = system::System::read(&home);
            if json {
                println!("{}", report::scan_json(&items, &sys, start.elapsed()));
            } else {
                report::print_scan(&items, &sys, &home, all, start.elapsed());
            }
        }
        Command::Top { path, count, depth, json } => {
            let path = std::path::absolute(path.unwrap_or_else(|| home.clone()))?;
            top::run(&path, count, depth, json, &home);
        }
        Command::Plan { ids, scan, json } => {
            measuring(json);
            let opts = scan.opts()?;
            let items = scan::scan(&home, &opts);
            let selected = scan::select(&items, &ids)?;
            let plan = plan::make(&selected, &opts, &home);
            anyhow::ensure!(
                !plan.items.is_empty(),
                Refused(format!(
                    "고른 항목에 지울 수 있는 경로가 없습니다: {}",
                    plan.refused.iter().map(|s| s.reason.as_str()).collect::<Vec<_>>().join(" / ")
                ))
            );
            let file = plan::save(&plan, &home)?;
            if json {
                let mut out = serde_json::to_value(&plan)?;
                out["file"] = serde_json::json!(file);
                out["apply"] = serde_json::json!(format!("disk-clean apply {}", plan.id));
                println!("{out}");
            } else {
                plan::print(&plan, &home);
                println!();
                println!("{}", style(format!("저장: {} (1시간 유효, 한 번만 실행)", fmt::tilde(&file, &home))).dim());
                println!("실행: {}", style(format!("disk-clean apply {}", plan.id)).bold());
            }
        }
        Command::Apply { id, json } => return plan::apply(&id, &home, json),
        Command::Log { count, json } => {
            let (entries, bad) = state::history(&state::dir(&home), count)?;
            if json {
                println!("{}", serde_json::json!({ "schema": "disk-clean/log/v1", "entries": entries, "unreadable_lines": bad }));
            } else {
                print_log(&entries, bad);
            }
        }
        Command::Clean { ids, dry_run, scan } => {
            measuring(false);
            let opts = scan.opts()?;
            let items = scan::scan(&home, &opts);
            return clean::run(items, &ids, dry_run, &opts, &home);
        }
    }
    Ok(Outcome::Done)
}

fn print_log(entries: &[state::Entry], bad: usize) {
    if entries.is_empty() {
        println!("지운 기록이 없습니다.");
    }
    for e in entries {
        let bytes: u64 = e.deleted.iter().filter_map(|d| d.bytes).sum();
        let freed = match (e.free_before, e.free_after) {
            (Some(b), Some(a)) => format!(" · 남은 공간 +{}", fmt::size(a.saturating_sub(b))),
            _ => String::new(),
        };
        println!(
            "{}  {:<6} {}  {}  지움 {}곳 {}{freed}{}",
            fmt::datetime(e.time),
            e.action,
            e.by,
            e.plan.as_deref().unwrap_or("-"),
            e.deleted.len(),
            fmt::size(bytes),
            match (e.refused.len(), e.failed.len()) {
                (0, 0) => String::new(),
                (r, f) => format!(" · 거부 {r} · 실패 {f}"),
            }
        );
        println!("    {}", style(e.items.join(", ")).dim());
    }
    if bad > 0 {
        println!("{}", style(format!("읽을 수 없는 기록 {bad}줄")).yellow());
    }
}
