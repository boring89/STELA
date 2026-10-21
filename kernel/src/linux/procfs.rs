// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The text of the generated `/proc` files (P12, `docs/linux.md` §7.3),
//! in the formats Linux programs parse (`proc(5)`). Pure logic, tested on
//! the host; `files.rs` fills in the numbers.

use alloc::format;
use alloc::string::String;

/// `/proc/cpuinfo`: one paragraph per processor.
pub fn cpuinfo(cpus: usize, vendor: &str, brand: &str, mhz: u64) -> String {
    let mut s = String::new();
    for i in 0..cpus.max(1) {
        s.push_str(&format!(
            "processor\t: {i}\nvendor_id\t: {vendor}\nmodel name\t: {brand}\ncpu MHz\t\t: {mhz}.000\n\
             cpu cores\t: {cpus}\nflags\t\t: fpu tsc msr pae cx8 apic sep cmov pat clflush mmx fxsr sse sse2 syscall nx lm\n\n"
        ));
    }
    s
}

/// `/proc/meminfo` (sizes in bytes, shown in KiB).
pub fn meminfo(total: u64, free: u64) -> String {
    format!(
        "MemTotal:       {:>8} kB\nMemFree:        {:>8} kB\nMemAvailable:   {:>8} kB\n\
         Buffers:               0 kB\nCached:                0 kB\nSwapTotal:             0 kB\nSwapFree:              0 kB\n",
        total / 1024,
        free / 1024,
        free / 1024
    )
}

/// `/proc/uptime`: seconds since boot (and the same as idle time).
pub fn uptime(ns: u64) -> String {
    let cs = ns / 10_000_000;
    format!(
        "{}.{:02} {}.{:02}\n",
        cs / 100,
        cs % 100,
        cs / 100,
        cs % 100
    )
}

/// One line of `/proc/<pid>/maps`.
pub fn maps_line(start: u64, end: u64, write: bool, exec: bool, name: &str) -> String {
    let perms = format!(
        "r{}{}p",
        if write { 'w' } else { '-' },
        if exec { 'x' } else { '-' }
    );
    if name.is_empty() {
        format!("{start:08x}-{end:08x} {perms} 00000000 00:00 0\n")
    } else {
        format!("{start:08x}-{end:08x} {perms} 00000000 00:00 0          {name}\n")
    }
}

/// What `/proc/<pid>/stat` and `status` say about a process.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcInfo {
    /// Process id.
    pub pid: u64,
    /// Program name.
    pub name: String,
    /// `R` running, `S` sleeping, `Z` ended (zombie).
    pub state: char,
    /// Parent's pid.
    pub ppid: u64,
    /// Process group.
    pub pgid: u64,
    /// Session.
    pub sid: u64,
    /// Threads.
    pub threads: u64,
    /// Resident memory in pages.
    pub pages: u64,
}

/// `/proc/<pid>/stat`: the 52 fields of `proc(5)`, unknown ones 0.
pub fn stat(p: &ProcInfo) -> String {
    // pid (comm) state ppid pgrp session tty_nr tpgid flags minflt
    // cminflt majflt cmajflt utime stime cutime cstime priority nice
    // num_threads itrealvalue starttime vsize rss ...
    let mut s = format!(
        "{} ({}) {} {} {} {} 0 -1 0 0 0 0 0 0 0 0 0 20 0 {} 0 0 {} {}",
        p.pid,
        p.name,
        p.state,
        p.ppid,
        p.pgid,
        p.sid,
        p.threads,
        p.pages * 4096,
        p.pages
    );
    // Fields 25..52.
    for _ in 25..=52 {
        s.push_str(" 0");
    }
    s.push('\n');
    s
}

/// `/proc/<pid>/status` (the lines programs usually read).
pub fn status(p: &ProcInfo) -> String {
    let state = match p.state {
        'R' => "R (running)",
        'Z' => "Z (zombie)",
        _ => "S (sleeping)",
    };
    format!(
        "Name:\t{}\nState:\t{}\nTgid:\t{}\nPid:\t{}\nPPid:\t{}\nUid:\t0\t0\t0\t0\nGid:\t0\t0\t0\t0\n\
         VmRSS:\t{} kB\nThreads:\t{}\n",
        p.name,
        state,
        p.pid,
        p.pid,
        p.ppid,
        p.pages * 4,
        p.threads
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        let c = cpuinfo(2, "GenuineIntel", "QEMU Virtual CPU", 2000);
        assert_eq!(c.matches("processor\t:").count(), 2);
        assert!(c.contains("model name\t: QEMU Virtual CPU\n"));
        let m = meminfo(512 << 20, 300 << 20);
        assert!(m.starts_with("MemTotal:         524288 kB\n"));
        assert!(m.contains("MemAvailable:     307200 kB\n"));
        assert_eq!(uptime(12_345_000_000), "12.34 12.34\n");
        assert_eq!(
            maps_line(0x40_0000, 0x40_1000, false, true, "/Initrd/linux/hello"),
            "00400000-00401000 r-xp 00000000 00:00 0          /Initrd/linux/hello\n"
        );
        assert_eq!(
            maps_line(0x1000_0000_0000, 0x1000_0000_2000, true, false, ""),
            "100000000000-100000002000 rw-p 00000000 00:00 0\n"
        );
        let p = ProcInfo {
            pid: 7,
            name: String::from("dash"),
            state: 'S',
            ppid: 1,
            pgid: 7,
            sid: 7,
            threads: 1,
            pages: 10,
        };
        let s = stat(&p);
        assert!(s.starts_with("7 (dash) S 1 7 7 0 -1 "));
        assert_eq!(s.trim_end().split(' ').count(), 52);
        let st = status(&p);
        assert!(st.contains("PPid:\t1\n") && st.contains("State:\tS (sleeping)\n"));
    }
}
