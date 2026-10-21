// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Local time (P11 K40, `docs/time.md` §5.6): the wall clock plus the
//! offset in `Data:/.stela/timezone` (`+08:00`; +08:00 when there is no
//! file). Dates are the proleptic Gregorian calendar; the formatting is
//! pure and tested on the host.

use abi::time::{DEFAULT_OFFSET_MINUTES, TIMEZONE_FILE, parse_offset};
use alloc::format;
use alloc::string::String;

/// The year, month, day, hour, minute and second of `unix_ns`
/// (nanoseconds since 1970-01-01 00:00), by Howard Hinnant's
/// days-from-civil algorithm run backwards.
pub fn civil(unix_ns: i64) -> (i64, u32, u32, u32, u32, u32) {
    let secs = unix_ns.div_euclid(1_000_000_000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (
        y,
        m,
        d,
        (rem / 3600) as u32,
        (rem / 60 % 60) as u32,
        (rem % 60) as u32,
    )
}

/// `YYYY-MM-DD HH:MM:SS`.
pub fn format(unix_ns: i64) -> String {
    let (y, mo, d, h, mi, s) = civil(unix_ns);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// `HH:MM`.
pub fn hours_minutes(unix_ns: i64) -> String {
    let (_, _, _, h, mi, _) = civil(unix_ns);
    format!("{h:02}:{mi:02}")
}

/// The offset from UTC in minutes: `Data:/.stela/timezone`, or +08:00.
pub fn offset_minutes() -> i32 {
    read_setting()
        .and_then(|t| parse_offset(&t))
        .unwrap_or(DEFAULT_OFFSET_MINUTES)
}

/// The offset as `+08:00`.
pub fn offset_text(minutes: i32) -> String {
    String::from_utf8_lossy(&abi::time::offset_text(minutes)).into_owned()
}

/// The local time: nanoseconds since 1970 in the local zone (`None`
/// without a clock).
pub fn local_ns() -> Option<i64> {
    let utc = crate::process::wall_clock_ns().ok()? as i64;
    Some(utc + i64::from(offset_minutes()) * 60_000_000_000)
}

/// `utc_ns` (a file's time) in the local zone.
pub fn to_local(utc_ns: u64) -> i64 {
    utc_ns as i64 + i64::from(offset_minutes()) * 60_000_000_000
}

fn read_setting() -> Option<String> {
    use crate::fs::{self, open};
    let root = fs::volume_open("Data").ok()?;
    let h = fs::open(root, TIMEZONE_FILE, open::READ);
    let _ = crate::ipc::handle_close(root);
    let h = h.ok()?;
    let mut buf = [0u8; 32];
    let n = fs::read(h, &mut buf, 0).unwrap_or(0);
    let _ = crate::ipc::handle_close(h);
    core::str::from_utf8(&buf[..n]).ok().map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(format(0), "1970-01-01 00:00:00");
        // 2026-10-21 08:13:05 UTC.
        assert_eq!(format(1_792_570_385 * 1_000_000_000), "2026-10-21 08:13:05");
        assert_eq!(format(951_782_400 * 1_000_000_000), "2000-02-29 00:00:00");
        // Before 1970 (a negative offset applied to 1970-01-01).
        assert_eq!(format(-3600 * 1_000_000_000), "1969-12-31 23:00:00");
        assert_eq!(hours_minutes((8 * 3600 + 5 * 60) * 1_000_000_000), "08:05");
        assert_eq!(offset_text(480), "+08:00");
    }
}
