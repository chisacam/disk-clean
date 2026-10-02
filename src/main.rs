mod clean;
mod dev;
mod fmt;
mod orphans;
mod report;
mod rules;
mod scan;
mod system;
mod top;
mod walk;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use console::style;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

/// macOS 의 '시스템 데이터'·'문서' 용량이 어디서 왔는지 보여 주고,
/// 다시 만들 수 있는 것만 골라 지웁니다.
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
    },
    /// 고른 항목을 삭제합니다 (확인 후, 되돌릴 수 없음)
    Clean {
        /// 지울 항목 ID (scan 에 표시). 생략하면 목록에서 고릅니다
        ids: Vec<String>,
        /// 지울 경로만 보여 주고 아무것도 지우지 않습니다
        #[arg(long)]
        dry_run: bool,
        /// 확인 없이 지웁니다
        #[arg(short, long)]
        yes: bool,
        #[command(flatten)]
        scan: ScanArgs,
    },
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

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{} {e:#}", style("오류:").red().bold());
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME 이 설정돼 있지 않습니다")?);
    let command = cli.command.unwrap_or(Command::Scan { scan: ScanArgs::default(), all: false });
    match command {
        Command::Scan { scan, all } => {
            let start = Instant::now();
            eprintln!("{}", style("재는 중…").dim());
            let items = scan::scan(&home, &scan.opts()?);
            let sys = system::System::read(&home);
            report::print_scan(&items, &sys, &home, all, start.elapsed());
        }
        Command::Top { path, count, depth } => {
            let path = std::path::absolute(path.unwrap_or_else(|| home.clone()))?;
            top::run(&path, count, depth, &home);
        }
        Command::Clean { ids, dry_run, yes, scan } => {
            eprintln!("{}", style("재는 중…").dim());
            let items = scan::scan(&home, &scan.opts()?);
            clean::run(items, &ids, &clean::Opts { dry_run, yes }, &home)?;
        }
    }
    Ok(())
}
