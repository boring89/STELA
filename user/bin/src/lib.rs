// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! `cli`: what STELA's command-line tools (`Initrd:/bin/`, P11,
//! `docs/apps.md`) share. The text work is pure and tested on the host:
//! options, counting, sorting, matching, dates, hex dumps, ustar headers.
//! On STELA, [`sys`] finds files from the working folder (`[3]`), reads
//! the input (`[5]` or files) and reports errors as `tool: message`.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Parsed arguments: single-letter flags (`-i -n` or `-in`), options with
/// a value (`-n 5`, `-name '*.txt'`) and the rest, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Args {
    /// The flags given.
    pub flags: Vec<char>,
    /// Options with values.
    pub values: BTreeMap<String, String>,
    /// Everything else (files, patterns...).
    pub rest: Vec<String>,
}

impl Args {
    /// Parses `args` (without the program's name). `valued` names the
    /// options that take a value (`"n"`, `"name"`); `allowed` the flags.
    /// `--` ends the options; `-` alone is an operand.
    pub fn parse(args: &[String], allowed: &str, valued: &[&str]) -> Result<Args, String> {
        let mut out = Args::default();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if a == "--" {
                out.rest.extend(it.cloned());
                break;
            }
            let Some(opt) = a.strip_prefix('-').filter(|o| !o.is_empty()) else {
                out.rest.push(a.clone());
                continue;
            };
            if valued.contains(&opt) {
                let v = it.next().ok_or_else(|| format!("-{opt} needs a value"))?;
                out.values.insert(opt.to_string(), v.clone());
                continue;
            }
            // A number right after a valued one-letter option: -n5.
            if let Some(name) = valued.iter().find(|v| v.len() == 1 && opt.starts_with(**v))
                && opt.len() > 1
                && opt[1..].chars().all(|c| c.is_ascii_digit())
            {
                out.values.insert(name.to_string(), opt[1..].to_string());
                continue;
            }
            for c in opt.chars() {
                if !allowed.contains(c) {
                    return Err(format!("unknown option -{c}"));
                }
                out.flags.push(c);
            }
        }
        Ok(out)
    }

    /// Whether flag `c` was given.
    pub fn has(&self, c: char) -> bool {
        self.flags.contains(&c)
    }

    /// The number given to option `name`, or `default`.
    pub fn number(&self, name: &str, default: u64) -> Result<u64, String> {
        match self.values.get(name) {
            Some(v) => v.parse().map_err(|_| format!("-{name}: not a number: {v}")),
            None => Ok(default),
        }
    }
}

/// Lines, words and bytes of `data` (a word is a run of non-blanks).
pub fn count(data: &[u8]) -> (u64, u64, u64) {
    let (mut lines, mut words) = (0, 0);
    let mut in_word = false;
    for &b in data {
        if b == b'\n' {
            lines += 1;
        }
        let space = b.is_ascii_whitespace();
        if !space && !in_word {
            words += 1;
        }
        in_word = !space;
    }
    (lines, words, data.len() as u64)
}

/// The lines of `text` (a last line without `\n` counts; no empty line
/// after a final `\n`).
pub fn lines(text: &str) -> Vec<&str> {
    let mut v: Vec<&str> = text.split('\n').collect();
    if v.last() == Some(&"") {
        v.pop();
    }
    v
}

/// Whether `line` contains `pattern` (both lowered with `ignore_case`).
pub fn matches(line: &str, pattern: &str, ignore_case: bool) -> bool {
    if ignore_case {
        line.to_lowercase().contains(&pattern.to_lowercase())
    } else {
        line.contains(pattern)
    }
}

/// The number at the start of `s` (after blanks; `-` allowed) for a
/// numeric sort; lines without one sort as 0.
fn leading_number(s: &str) -> i64 {
    let t = s.trim_start();
    let (neg, digits) = match t.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, t),
    };
    let n: i64 = digits
        .chars()
        .take_while(char::is_ascii_digit)
        .fold(0i64, |n, c| {
            n.saturating_mul(10)
                .saturating_add(i64::from(c as u8 - b'0'))
        });
    if neg { -n } else { n }
}

/// Sorts `lines` by text, or by their leading number (`numeric`; ties by
/// text), reversed with `reverse`.
pub fn sort(lines: &mut [&str], numeric: bool, reverse: bool) {
    if numeric {
        lines.sort_by(|a, b| leading_number(a).cmp(&leading_number(b)).then(a.cmp(b)));
    } else {
        lines.sort();
    }
    if reverse {
        lines.reverse();
    }
}

/// Adjacent equal lines once each, with how many there were.
pub fn uniq<'a>(lines: &[&'a str]) -> Vec<(usize, &'a str)> {
    let mut out: Vec<(usize, &str)> = Vec::new();
    for &l in lines {
        match out.last_mut() {
            Some((n, last)) if *last == l => *n += 1,
            _ => out.push((1, l)),
        }
    }
    out
}

/// Shell-style matching of a name: `*` any run, `?` one character.
pub fn glob(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// `YYYY-MM-DD HH:MM:SS` (UTC) for nanoseconds since 1970.
pub fn date(unix_ns: u64) -> String {
    let secs = unix_ns / 1_000_000_000;
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// One line of `hexdump`: the offset, 16 bytes in hex and as text.
pub fn hex_line(offset: u64, bytes: &[u8]) -> String {
    let mut s = format!("{offset:08x} ");
    for i in 0..16 {
        if i == 8 {
            s.push(' ');
        }
        match bytes.get(i) {
            Some(b) => s.push_str(&format!(" {b:02x}")),
            None => s.push_str("   "),
        }
    }
    s.push_str("  |");
    for &b in bytes {
        s.push(if (0x20..0x7F).contains(&b) {
            b as char
        } else {
            '.'
        });
    }
    s.push('|');
    s
}

/// A ustar member: its name, size, whether it is a folder, and where its
/// data starts in the archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TarEntry {
    /// The path inside the archive (prefix and name joined).
    pub name: String,
    /// Bytes of data.
    pub size: u64,
    /// A folder (type `5`, or a name ending in `/`).
    pub is_dir: bool,
    /// Offset of the data.
    pub data: usize,
}

/// The members of a ustar archive (POSIX.1-1988 headers of 512 bytes:
/// name 0..100, size 124..136 in octal, type 156, prefix 345..500), up to
/// the two zero blocks; `Err` for a header that does not parse.
pub fn tar_entries(archive: &[u8]) -> Result<Vec<TarEntry>, String> {
    let field = |h: &[u8], a: usize, b: usize| -> String {
        let f = &h[a..b];
        let end = f.iter().position(|&c| c == 0).unwrap_or(f.len());
        String::from_utf8_lossy(&f[..end]).into_owned()
    };
    let mut out = Vec::new();
    let mut at = 0;
    while at + 512 <= archive.len() {
        let h = &archive[at..at + 512];
        if h.iter().all(|&b| b == 0) {
            break;
        }
        let size_text = field(h, 124, 136);
        let size = u64::from_str_radix(size_text.trim(), 8)
            .map_err(|_| format!("bad size at offset {at}"))?;
        let prefix = field(h, 345, 500);
        let name = field(h, 0, 100);
        let name = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let kind = h[156];
        let name = name.trim_start_matches("./").to_string();
        let is_dir = kind == b'5' || name.ends_with('/');
        if !name.is_empty() && name != "." {
            out.push(TarEntry {
                name: name.trim_end_matches('/').to_string(),
                size,
                is_dir,
                data: at + 512,
            });
        }
        at += 512 + (size as usize).next_multiple_of(512);
    }
    Ok(out)
}

#[cfg(target_os = "none")]
pub mod sys;

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn options() {
        let a = Args::parse(
            &strings(&["-in", "-v", "pat", "f1", "--", "-x"]),
            "inv",
            &[],
        )
        .unwrap();
        assert_eq!(a.flags, ['i', 'n', 'v']);
        assert_eq!(a.rest, ["pat", "f1", "-x"]);
        let a = Args::parse(&strings(&["-n", "5", "file"]), "", &["n"]).unwrap();
        assert_eq!(a.number("n", 10), Ok(5));
        assert_eq!(a.rest, ["file"]);
        let a = Args::parse(&strings(&["-n3"]), "", &["n"]).unwrap();
        assert_eq!(a.number("n", 10), Ok(3));
        assert_eq!(
            Args::parse(&strings(&["-q"]), "in", &[]).err().as_deref(),
            Some("unknown option -q")
        );
        assert!(Args::parse(&strings(&["-n"]), "", &["n"]).is_err());
        let a = Args::parse(&strings(&["dir", "-name", "*.txt"]), "", &["name"]).unwrap();
        assert_eq!(a.values.get("name").map(String::as_str), Some("*.txt"));
        assert_eq!(Args::parse(&strings(&["-"]), "", &[]).unwrap().rest, ["-"]);
    }

    #[test]
    fn counting_and_lines() {
        assert_eq!(count(b"one two\nthree\n"), (2, 3, 14));
        assert_eq!(count(b""), (0, 0, 0));
        assert_eq!(lines("a\nb\n"), ["a", "b"]);
        assert_eq!(lines("a\nb"), ["a", "b"]);
        assert!(lines("").is_empty());
        assert!(matches("Welcome to STELA", "stela", true));
        assert!(!matches("Welcome to STELA", "stela", false));
    }

    #[test]
    fn sorting_and_uniq() {
        let mut v = vec!["10 b", "9 a", "-1 z", "x"];
        sort(&mut v, true, false);
        assert_eq!(v, ["-1 z", "x", "9 a", "10 b"]);
        sort(&mut v, false, true);
        assert_eq!(v, ["x", "9 a", "10 b", "-1 z"]);
        assert_eq!(uniq(&["a", "a", "b", "a"]), [(2, "a"), (1, "b"), (1, "a")]);
    }

    #[test]
    fn globs() {
        assert!(glob("*.txt", "notes.txt"));
        assert!(!glob("*.txt", "notes.txt2"));
        assert!(glob("a?c", "abc"));
        assert!(glob("*", ""));
        assert!(glob("*b*c", "aabxxc"));
        assert!(!glob("a*b", "ac"));
    }

    #[test]
    fn dates() {
        assert_eq!(date(0), "1970-01-01 00:00:00");
        // 2026-10-21 08:13:05 UTC.
        assert_eq!(date(1_792_570_385 * 1_000_000_000), "2026-10-21 08:13:05");
        // A leap day.
        assert_eq!(date(951_782_400 * 1_000_000_000), "2000-02-29 00:00:00");
    }

    #[test]
    fn hex() {
        assert_eq!(
            hex_line(16, b"STELA\n"),
            "00000010  53 54 45 4c 41 0a                                 |STELA.|"
        );
    }

    #[test]
    fn tar() {
        let mut a = vec![0u8; 512 * 5];
        let header = |a: &mut [u8], at: usize, name: &str, size: u64, kind: u8| {
            a[at..at + name.len()].copy_from_slice(name.as_bytes());
            let s = format!("{size:011o}");
            a[at + 124..at + 135].copy_from_slice(s.as_bytes());
            a[at + 156] = kind;
        };
        header(&mut a, 0, "./dir/", 0, b'5');
        header(&mut a, 512, "./dir/hi.txt", 3, b'0');
        a[1024..1027].copy_from_slice(b"hi\n");
        let e = tar_entries(&a).unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!((e[0].name.as_str(), e[0].is_dir), ("dir", true));
        assert_eq!(
            (e[1].name.as_str(), e[1].size, e[1].data),
            ("dir/hi.txt", 3, 1024)
        );
        a[512 + 124] = b'9';
        assert!(tar_entries(&a).is_err());
    }
}
