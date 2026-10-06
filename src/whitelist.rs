//! Paths disk-clean never deletes, as Mole's `~/.config/mole/whitelist` does.
//!
//! The file is `$XDG_CONFIG_HOME/disk-clean/whitelist`, else
//! `~/.config/disk-clean/whitelist`: one path or glob per line, `#` comments,
//! `~`, `$HOME` and `${HOME}` expanded. A path is protected when it matches a
//! pattern, when it is inside a protected folder, or when it holds one: a
//! folder is deleted whole, so deleting it would take the protected path too.
//!
//! A few built-in patterns are always on. Removing them breaks things rather
//! than costing a rebuild (the list and the reasons follow Mole's
//! `SAFETY_WHITELIST_PATTERNS`).

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

/// Always protected, whatever the file says: (pattern, why).
#[cfg(target_os = "macos")]
const BUILTIN: &[(&str, &str)] = &[
    ("~/Library/Caches/CloudKit*", "iCloud 동기화가 쓰는 캐시"),
    ("~/Library/Caches/pypoetry/virtualenvs*", "Poetry 프로젝트가 쓰는 가상 환경"),
    ("~/Library/Caches/org.R-project.R/R/renv*", "renv 가 프로젝트 라이브러리에 링크하는 패키지"),
];
#[cfg(not(target_os = "macos"))]
const BUILTIN: &[(&str, &str)] = &[
    ("~/.cache/pypoetry/virtualenvs*", "Poetry 프로젝트가 쓰는 가상 환경"),
    ("~/.cache/R/renv*", "renv 가 프로젝트 라이브러리에 링크하는 패키지"),
];

const HEADER: &str = "\
# disk-clean whitelist: 여기 적은 경로는 지우지 않습니다.
# 한 줄에 하나. ~ 와 $HOME 을 쓸 수 있고, * ? [...] glob 도 됩니다.
# 폴더를 적으면 그 안쪽도, 그 폴더를 품은 상위 폴더도 지우지 않습니다.
";

#[derive(Clone, Debug, serde::Serialize)]
pub struct Pattern {
    /// As written in the file, or the built-in form.
    pub raw: String,
    /// Expanded, absolute, without a trailing slash.
    pub path: String,
    pub glob: bool,
    /// Why a built-in pattern is there; `None` for the user's own.
    pub builtin: Option<&'static str>,
}

pub struct Whitelist {
    pub file: PathBuf,
    pub patterns: Vec<Pattern>,
    /// Lines that were not used, and why.
    pub warnings: Vec<String>,
}

pub fn file(home: &Path) -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    base.join("disk-clean/whitelist")
}

/// Expands a pattern to an absolute path, or says why it cannot be used.
fn expand(raw: &str, home: &Path) -> Result<String, String> {
    let home = home.to_string_lossy();
    let mut s = raw.trim().to_string();
    if s == "~" || s.starts_with("~/") {
        s = format!("{home}{}", &s[1..]);
    }
    s = s.replace("${HOME}", &home).replace("$HOME", &home);
    if s.chars().any(char::is_control) {
        return Err(format!("제어 문자가 있음: {raw}"));
    }
    if !s.starts_with('/') {
        return Err(format!("절대 경로가 아님: {raw}"));
    }
    if s.split('/').any(|c| c == "..") {
        return Err(format!("`..` 은 쓸 수 없음: {raw}"));
    }
    if s.contains("//") {
        return Err(format!("`//` 가 있음: {raw}"));
    }
    while s.len() > 1 && s.ends_with('/') {
        s.pop();
    }
    if s == "/" {
        return Err(format!("`/` 전체는 보호할 수 없음: {raw}"));
    }
    Ok(s)
}

fn has_glob(s: &str) -> bool {
    s.contains(['*', '?', '['])
}

impl Whitelist {
    pub fn load(home: &Path) -> Self {
        let file = file(home);
        let mut wl = Whitelist { file: file.clone(), patterns: Vec::new(), warnings: Vec::new() };
        for (raw, why) in BUILTIN {
            let path = expand(raw, home).expect("built-in patterns are valid");
            wl.patterns.push(Pattern { raw: raw.to_string(), glob: has_glob(&path), path, builtin: Some(why) });
        }
        let text = match fs::read_to_string(&file) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return wl,
            Err(e) => {
                wl.warnings.push(format!("whitelist 를 읽지 못함({}): {e}", file.display()));
                return wl;
            }
        };
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            match expand(line, home) {
                Ok(path) if wl.patterns.iter().any(|p| p.path == path) => {}
                Ok(path) => wl.patterns.push(Pattern { raw: line.to_string(), glob: has_glob(&path), path, builtin: None }),
                Err(why) => wl.warnings.push(why),
            }
        }
        wl
    }

    /// The pattern that protects `target`, if any.
    pub fn protects(&self, target: &Path) -> Option<&Pattern> {
        let mut t = target.to_string_lossy().into_owned();
        while t.len() > 1 && t.ends_with('/') {
            t.pop();
        }
        self.patterns.iter().find(|p| {
            t == p.path
                || (p.glob && glob(&p.path, &t))
                // `target` holds the protected path; deleting it would take that too.
                || p.path.strip_prefix(t.as_str()).is_some_and(|rest| rest.starts_with('/'))
                // `target` is inside a protected folder.
                || (!p.glob && t.strip_prefix(p.path.as_str()).is_some_and(|rest| rest.starts_with('/')))
        })
    }
}

/// Shell-style match over the whole string: `*` (any run, `/` included, as in
/// `[[ == ]]`), `?` (one character), `[abc]`, `[a-z]`, `[!x]`.
pub fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() {
            match p[pi] {
                '*' => {
                    star = Some((pi, ti));
                    pi += 1;
                    continue;
                }
                '?' => {
                    pi += 1;
                    ti += 1;
                    continue;
                }
                '[' => {
                    if let Some((matched, next)) = class(&p, pi, t[ti])
                        && matched
                    {
                        pi = next;
                        ti += 1;
                        continue;
                    }
                }
                c if c == t[ti] => {
                    pi += 1;
                    ti += 1;
                    continue;
                }
                _ => {}
            }
        }
        match star {
            Some((sp, st)) => {
                pi = sp + 1;
                ti = st + 1;
                star = Some((sp, st + 1));
            }
            None => return false,
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// Matches `c` against the class starting at `p[start] == '['`:
/// whether it matched, and where the pattern continues. `None` if unterminated.
fn class(p: &[char], start: usize, c: char) -> Option<(bool, usize)> {
    let mut i = start + 1;
    let negate = matches!(p.get(i), Some('!' | '^'));
    if negate {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    while i < p.len() && (p[i] != ']' || first) {
        first = false;
        if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
            matched |= p[i] <= c && c <= p[i + 2];
            i += 3;
        } else {
            matched |= p[i] == c;
            i += 1;
        }
    }
    (i < p.len()).then_some((matched != negate, i + 1))
}

/// How a pattern is written to the file: under home as `~/…`.
fn spelled(path: &str, home: &Path) -> String {
    let home = home.to_string_lossy();
    match path.strip_prefix(home.as_ref()) {
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

/// Adds patterns to the file. Returns the lines actually added.
pub fn add(home: &Path, cwd: &Path, raws: &[String]) -> Result<Vec<String>> {
    let wl = Whitelist::load(home);
    let mut added = Vec::new();
    for raw in raws {
        let absolute = if raw.starts_with('/') || raw.starts_with('~') || raw.starts_with('$') {
            raw.clone()
        } else {
            cwd.join(raw).to_string_lossy().into_owned()
        };
        let path = expand(&absolute, home).map_err(|why| anyhow::anyhow!(why))?;
        if wl.patterns.iter().any(|p| p.path == path) || added.iter().any(|a: &String| expand(a, home).as_deref() == Ok(path.as_str())) {
            continue;
        }
        added.push(spelled(&path, home));
    }
    if !added.is_empty() {
        let mut text = fs::read_to_string(&wl.file).unwrap_or_else(|_| HEADER.to_string());
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        for line in &added {
            text.push_str(line);
            text.push('\n');
        }
        write(&wl.file, &text)?;
    }
    Ok(added)
}

/// Removes patterns from the file. Built-in patterns cannot be removed.
pub fn remove(home: &Path, raws: &[String]) -> Result<Vec<String>> {
    let wl = Whitelist::load(home);
    let mut targets = Vec::new();
    for raw in raws {
        let path = expand(raw, home).map_err(|why| anyhow::anyhow!(why))?;
        if let Some(p) = wl.patterns.iter().find(|p| p.path == path && p.builtin.is_some()) {
            return Err(crate::Refused(format!("기본 보호는 뺄 수 없습니다: {} ({})", p.raw, p.builtin.unwrap_or(""))).into());
        }
        if !wl.patterns.iter().any(|p| p.path == path) {
            bail!("whitelist 에 없습니다: {raw}");
        }
        targets.push(path);
    }
    let text = fs::read_to_string(&wl.file).with_context(|| wl.file.display().to_string())?;
    let mut removed = Vec::new();
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| {
            let l = line.trim();
            let hit = !l.is_empty() && !l.starts_with('#') && expand(l, home).is_ok_and(|p| targets.contains(&p));
            if hit {
                removed.push(l.to_string());
            }
            !hit
        })
        .collect();
    write(&wl.file, &(kept.join("\n") + "\n"))?;
    Ok(removed)
}

fn write(file: &Path, text: &str) -> Result<()> {
    fs::create_dir_all(file.parent().unwrap())?;
    let tmp = file.with_extension("tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, file)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_like_the_shell() {
        assert!(glob("/a/b*", "/a/b"));
        assert!(glob("/a/b*", "/a/bc/d"), "* crosses /, as [[ == ]] does");
        assert!(glob("/a/?x", "/a/yx"));
        assert!(!glob("/a/?x", "/a/x"));
        assert!(glob("/a/[bc]", "/a/c"));
        assert!(glob("/a/[a-c]z", "/a/bz"));
        assert!(!glob("/a/[!b]", "/a/b"));
        assert!(glob("/a/*/node_modules", "/a/x/y/node_modules"));
        assert!(!glob("/a/*.log", "/a/x.txt"));
        assert!(!glob("/a/[b", "/a/b"), "an unterminated class matches nothing");
    }

    fn with_file(text: &str) -> (tempfile::TempDir, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().to_path_buf();
        fs::create_dir_all(home.join(".config/disk-clean")).unwrap();
        fs::write(home.join(".config/disk-clean/whitelist"), text).unwrap();
        (t, home)
    }

    #[test]
    fn reads_the_file_and_warns_about_bad_lines() {
        let (_t, home) = with_file("# comment\n\n~/Library/Logs/mole\n$HOME/keep/\n${HOME}/a*\nrelative/path\n/x/../y\n/x//y\n/\n");
        let wl = Whitelist::load(&home);
        let h = home.to_string_lossy();
        let mine: Vec<&str> = wl.patterns.iter().filter(|p| p.builtin.is_none()).map(|p| p.path.as_str()).collect();
        assert_eq!(mine, [format!("{h}/Library/Logs/mole"), format!("{h}/keep"), format!("{h}/a*")]);
        assert_eq!(wl.warnings.len(), 4, "{:?}", wl.warnings);
        assert!(wl.patterns.iter().any(|p| p.builtin.is_some()));
    }

    #[test]
    fn protects_matches_inside_and_holders() {
        let (_t, home) = with_file("~/Library/Logs/mole\n~/Library/Caches/foo/keep\n~/Library/Caches/JetBrains*\n");
        let wl = Whitelist::load(&home);
        let p = |rel: &str| wl.protects(&home.join(rel)).is_some();
        assert!(p("Library/Logs/mole"), "exact");
        assert!(p("Library/Logs/mole/operations.log"), "inside a protected folder");
        assert!(p("Library/Caches/foo"), "holds a protected path");
        assert!(p("Library/Caches/JetBrains2026"), "glob");
        assert!(!p("Library/Logs/other"));
        assert!(!p("Library/Caches/foo2"), "a sibling that shares a prefix");
        assert!(!p("Library/Logs/mole2"));
    }

    #[test]
    fn add_and_remove_keep_comments_and_refuse_builtins() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path();
        let added = add(home, Path::new("/"), &["~/Library/Logs/mole".into(), "~/Library/Logs/mole/".into()]).unwrap();
        assert_eq!(added, ["~/Library/Logs/mole"], "duplicates in one call collapse");
        assert!(add(home, Path::new("/"), &["~/Library/Logs/mole".into()]).unwrap().is_empty());
        let text = fs::read_to_string(file(home)).unwrap();
        assert!(text.starts_with("# disk-clean whitelist"));

        assert_eq!(remove(home, &["~/Library/Logs/mole".into()]).unwrap(), ["~/Library/Logs/mole"]);
        assert!(fs::read_to_string(file(home)).unwrap().starts_with("# disk-clean whitelist"));
        assert!(remove(home, &["~/nope".into()]).is_err());
        let builtin = BUILTIN[0].0.to_string();
        let err = remove(home, &[builtin]).unwrap_err();
        assert!(err.downcast_ref::<crate::Refused>().is_some());
    }
}
