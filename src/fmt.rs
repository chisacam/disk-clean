use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use unicode_width::UnicodeWidthStr;

/// Formats a byte count in binary units, the way `du -h` does.
pub fn size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    match unit {
        0 => format!("{n} B"),
        _ if v >= 100.0 => format!("{v:.0} {}", UNITS[unit]),
        _ => format!("{v:.1} {}", UNITS[unit]),
    }
}

/// Left-aligns to a display width; Hangul takes two columns.
pub fn pad(s: &str, width: usize) -> String {
    format!("{s}{}", " ".repeat(width.saturating_sub(s.width())))
}

pub fn rpad(s: &str, width: usize) -> String {
    format!("{}{s}", " ".repeat(width.saturating_sub(s.width())))
}

pub fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// `YYYY-MM-DD` in local time.
pub fn date(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as libc::time_t;
    // SAFETY: localtime_r only writes into the tm we own.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return "?".into();
    }
    format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday)
}

/// Shortens from the middle so both the start and the leaf of a path stay visible.
pub fn shorten(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    let head = keep / 3;
    let tail = keep - head;
    let start: String = chars[..head].iter().collect();
    let end: String = chars[chars.len() - tail..].iter().collect();
    format!("{start}…{end}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(1023), "1023 B");
        assert_eq!(size(1536), "1.5 KiB");
        assert_eq!(size(90 << 30), "90.0 GiB");
        assert_eq!(size(495 << 30), "495 GiB");
    }

    #[test]
    fn pads_hangul_as_two_columns() {
        assert_eq!(pad("캐시", 6), "캐시  ");
        assert_eq!(rpad("1 B", 5), "  1 B");
    }

    #[test]
    fn shortens_from_the_middle() {
        assert_eq!(shorten("abcdef", 10), "abcdef");
        let s = shorten("~/work/acme/infra/terraform/environments/dev/.terraform", 30);
        assert_eq!(s.chars().count(), 30);
        assert!(s.starts_with("~/work") && s.ends_with("dev/.terraform"));
    }
}
