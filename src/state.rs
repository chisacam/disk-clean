//! Where plans and the audit log live: `$XDG_STATE_HOME/disk-clean`, else `~/.local/state/disk-clean`.
//!
//! Not under `~/Library/Logs`: the `logs` rule empties that folder.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn dir(home: &Path) -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".local/state"));
    base.join("disk-clean")
}

pub fn plans(home: &Path) -> PathBuf {
    dir(home).join("plans")
}

fn audit_file(dir: &Path) -> PathBuf {
    dir.join("audit.jsonl")
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// FNV-1a: stable across builds, unlike `DefaultHasher`. Not a secret, only a fingerprint.
pub fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

/// Who ran this, as far as the environment tells: every harness marker present.
///
/// Harnesses nest (pi started from a Claude Code shell inherits `CLAUDECODE`),
/// so no single marker is the caller. pi sets `PI_SESSION_ID` afresh for each
/// command it runs, which makes it the nearer one, so it comes first.
pub fn caller() -> String {
    let mut found = Vec::new();
    if let Some(id) = std::env::var_os("PI_SESSION_ID") {
        found.push(format!("pi:{}", id.to_string_lossy()));
    }
    if std::env::var_os("CLAUDECODE").is_some() {
        found.push("claude-code".into());
    }
    if found.is_empty() {
        found.push(if std::io::stdin().is_terminal() { "terminal" } else { "unknown" }.into());
    }
    found.join("+")
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Deleted {
    pub path: PathBuf,
    /// Measured just before deletion; absent when it was not measured per path.
    pub bytes: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Skipped {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Serialize, Deserialize)]
pub struct Entry {
    pub time: u64,
    /// `apply` or `clean`.
    pub action: String,
    pub by: String,
    pub plan: Option<String>,
    pub items: Vec<String>,
    pub deleted: Vec<Deleted>,
    /// Refused by the checks; nothing was attempted.
    pub refused: Vec<Skipped>,
    /// Attempted, and the deletion itself failed.
    pub failed: Vec<Skipped>,
    pub free_before: Option<u64>,
    pub free_after: Option<u64>,
}

/// Appends one line. A deletion that cannot be logged is reported, never silent.
pub fn record(dir: &Path, entry: &Entry) -> Result<()> {
    let path = audit_file(dir);
    fs::create_dir_all(path.parent().unwrap())?;
    let mut line = serde_json::to_vec(entry)?;
    line.push(b'\n');
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(&line))
        .with_context(|| format!("감사 로그를 쓰지 못했습니다: {}", path.display()))
}

/// The last `n` entries, oldest first. Unreadable lines are skipped and counted.
pub fn history(dir: &Path, n: usize) -> Result<(Vec<Entry>, usize)> {
    let path = audit_file(dir);
    let file = match fs::File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
        Err(e) => return Err(e).with_context(|| path.display().to_string()),
    };
    let mut entries = Vec::new();
    let mut bad = 0;
    for line in BufReader::new(file).lines() {
        match serde_json::from_str::<Entry>(&line?) {
            Ok(e) => entries.push(e),
            Err(_) => bad += 1,
        }
    }
    let skip = entries.len().saturating_sub(n);
    Ok((entries.into_iter().skip(skip).collect(), bad))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_is_stable() {
        assert_eq!(fnv64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn audit_round_trip() {
        let t = tempfile::tempdir().unwrap();
        for i in 0..3 {
            let entry = Entry {
                time: i,
                action: "apply".into(),
                by: "test".into(),
                plan: Some(format!("p{i}")),
                items: vec!["caches".into()],
                deleted: vec![Deleted { path: "/x".into(), bytes: Some(1) }],
                refused: Vec::new(),
                failed: Vec::new(),
                free_before: Some(1),
                free_after: Some(2),
            };
            record(t.path(), &entry).unwrap();
        }
        let (last, bad) = history(t.path(), 2).unwrap();
        assert_eq!(bad, 0);
        assert_eq!(last.iter().map(|e| e.time).collect::<Vec<_>>(), [1, 2]);
    }
}
