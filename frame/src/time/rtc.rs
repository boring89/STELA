// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Decoding the CMOS real-time clock's registers into a date and time, and
//! turning that into seconds since 1970. Pure logic, tested on the host;
//! the port access is in `arch/x86_64/cmos.rs`.
//!
//! The RTC (Motorola MC146818 and its descendants; OSDev Wiki "CMOS",
//! "RTC") keeps the time in registers 0x00 seconds, 0x02 minutes, 0x04
//! hours, 0x07 day of month, 0x08 month, 0x09 year (two digits) and, on
//! some machines, a century register whose index the ACPI FADT gives.
//! Status register B (0x0B) says how the values are written:
//!
//! ```text
//!  bit 2  binary (else BCD: 0x59 means 59)
//!  bit 1  24-hour (else 12-hour, with bit 7 of the hour = PM)
//! ```
//!
//! STELA treats the RTC as UTC (no time zones).

/// Status register B bit 2: values are binary, not BCD.
pub const STATUS_B_BINARY: u8 = 1 << 2;
/// Status register B bit 1: 24-hour mode.
pub const STATUS_B_24_HOUR: u8 = 1 << 1;
/// In 12-hour mode, bit 7 of the hour register means PM.
const HOUR_PM: u8 = 1 << 7;

/// The raw registers, as read from the CMOS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Registers {
    /// Register 0x00.
    pub second: u8,
    /// Register 0x02.
    pub minute: u8,
    /// Register 0x04.
    pub hour: u8,
    /// Register 0x07.
    pub day: u8,
    /// Register 0x08.
    pub month: u8,
    /// Register 0x09.
    pub year: u8,
    /// The FADT's century register, or 0 when there is none.
    pub century: u8,
    /// Register 0x0B.
    pub status_b: u8,
}

/// A date and time (UTC).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    /// Full year, for example 2026.
    pub year: u32,
    /// 1-12.
    pub month: u32,
    /// 1-31.
    pub day: u32,
    /// 0-23.
    pub hour: u32,
    /// 0-59.
    pub minute: u32,
    /// 0-59.
    pub second: u32,
}

fn from_bcd(value: u8) -> Option<u8> {
    if value & 0x0F > 9 || value >> 4 > 9 {
        return None;
    }
    Some((value >> 4) * 10 + (value & 0x0F))
}

fn leap_year(year: u32) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    const DAYS: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    DAYS[(month - 1) as usize] + u32::from(month == 2 && leap_year(year))
}

/// Decodes the registers; `None` if any value is out of range (a dead
/// battery or a read during an update).
pub fn decode(regs: &Registers) -> Option<DateTime> {
    let binary = regs.status_b & STATUS_B_BINARY != 0;
    let twenty_four = regs.status_b & STATUS_B_24_HOUR != 0;
    let pm = !twenty_four && regs.hour & HOUR_PM != 0;
    let hour_raw = if twenty_four {
        regs.hour
    } else {
        regs.hour & !HOUR_PM
    };
    let convert = |v: u8| if binary { Some(v) } else { from_bcd(v) };
    let second = convert(regs.second)?;
    let minute = convert(regs.minute)?;
    let mut hour = convert(hour_raw)?;
    let day = convert(regs.day)?;
    let month = convert(regs.month)?;
    let year = convert(regs.year)?;
    // A century register that does not decode is ignored.
    let century = if regs.century == 0 {
        0
    } else {
        convert(regs.century).unwrap_or(0)
    };
    if !twenty_four {
        // 12 AM is hour 0, 12 PM is hour 12.
        if !(1..=12).contains(&hour) {
            return None;
        }
        hour = hour % 12 + if pm { 12 } else { 0 };
    }
    if year > 99 {
        return None;
    }
    let full_year = if (19..=21).contains(&century) {
        u32::from(century) * 100 + u32::from(year)
    } else {
        // No century register: the RTC is assumed to be in 20xx.
        2000 + u32::from(year)
    };
    let (month, day) = (u32::from(month), u32::from(day));
    if second > 59
        || minute > 59
        || hour > 23
        || !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(full_year, month)
    {
        return None;
    }
    Some(DateTime {
        year: full_year,
        month,
        day,
        hour: u32::from(hour),
        minute: u32::from(minute),
        second: u32::from(second),
    })
}

impl DateTime {
    /// Seconds since 1970-01-01 00:00:00 UTC (years before 1970 give 0).
    pub fn unix_seconds(&self) -> u64 {
        if self.year < 1970 {
            return 0;
        }
        let mut days = 0u64;
        for y in 1970..self.year {
            days += if leap_year(y) { 366 } else { 365 };
        }
        for m in 1..self.month {
            days += u64::from(days_in_month(self.year, m));
        }
        days += u64::from(self.day - 1);
        days * 86_400
            + u64::from(self.hour) * 3_600
            + u64::from(self.minute) * 60
            + u64::from(self.second)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bcd_24(sec: u8, min: u8, hour: u8, day: u8, month: u8, year: u8) -> Registers {
        Registers {
            second: sec,
            minute: min,
            hour,
            day,
            month,
            year,
            century: 0,
            status_b: STATUS_B_24_HOUR,
        }
    }

    #[test]
    fn bcd_24_hour() {
        let t = decode(&bcd_24(0x30, 0x45, 0x17, 0x20, 0x10, 0x26)).unwrap();
        assert_eq!(
            t,
            DateTime {
                year: 2026,
                month: 10,
                day: 20,
                hour: 17,
                minute: 45,
                second: 30
            }
        );
    }

    #[test]
    fn binary_and_century() {
        let regs = Registers {
            second: 5,
            minute: 6,
            hour: 7,
            day: 8,
            month: 9,
            year: 99,
            century: 19,
            status_b: STATUS_B_BINARY | STATUS_B_24_HOUR,
        };
        assert_eq!(decode(&regs).unwrap().year, 1999);
        let regs = Registers {
            century: 0x20,
            status_b: STATUS_B_24_HOUR,
            ..bcd_24(0, 0, 0, 1, 1, 0x26)
        };
        assert_eq!(decode(&regs).unwrap().year, 2026);
        // A garbage century register falls back to 20xx.
        let regs = Registers {
            century: 0xFF,
            ..bcd_24(0, 0, 0, 1, 1, 0x26)
        };
        assert_eq!(decode(&regs).unwrap().year, 2026);
    }

    #[test]
    fn twelve_hour_mode() {
        let mut regs = bcd_24(0, 0, 0x12, 1, 1, 0x26);
        regs.status_b = 0;
        assert_eq!(decode(&regs).unwrap().hour, 0); // 12 AM
        regs.hour = 0x12 | HOUR_PM;
        assert_eq!(decode(&regs).unwrap().hour, 12); // 12 PM
        regs.hour = 0x05 | HOUR_PM;
        assert_eq!(decode(&regs).unwrap().hour, 17);
        regs.hour = 0x00;
        assert_eq!(decode(&regs), None);
    }

    #[test]
    fn invalid_values() {
        assert_eq!(decode(&bcd_24(0x60, 0, 0, 1, 1, 0x26)), None);
        assert_eq!(decode(&bcd_24(0x1A, 0, 0, 1, 1, 0x26)), None);
        assert_eq!(decode(&bcd_24(0, 0, 0x24, 1, 1, 0x26)), None);
        assert_eq!(decode(&bcd_24(0, 0, 0, 0x30, 0x02, 0x26)), None);
        assert_eq!(decode(&bcd_24(0, 0, 0, 0x29, 0x02, 0x26)), None);
        assert!(decode(&bcd_24(0, 0, 0, 0x29, 0x02, 0x28)).is_some());
        assert_eq!(decode(&bcd_24(0, 0, 0, 1, 0x13, 0x26)), None);
        assert_eq!(decode(&bcd_24(0, 0, 0, 0, 1, 0x26)), None);
    }

    #[test]
    fn unix_seconds() {
        let at = |year, month, day, hour, minute, second| {
            DateTime {
                year,
                month,
                day,
                hour,
                minute,
                second,
            }
            .unix_seconds()
        };
        assert_eq!(at(1970, 1, 1, 0, 0, 0), 0);
        assert_eq!(at(2000, 3, 1, 0, 0, 0), 951_868_800);
        assert_eq!(at(2026, 1, 1, 0, 0, 0), 1_767_225_600);
        assert_eq!(at(2026, 10, 20, 9, 30, 15), 1_792_488_615);
        assert_eq!(at(1969, 12, 31, 0, 0, 0), 0);
    }
}
