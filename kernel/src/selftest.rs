// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Self tests run at boot by the `selftest` build (`make test-qemu`). Each
//! test panics on failure; the panic handler then exits QEMU with a failure
//! status. When all pass, `selftest: OK` is printed and QEMU exits with 33.

use core::fmt::{self, Write};
use frame::bootinfo::BootInfo;
use frame::kprintln;
use frame::trap::{Report, TrapFrame};

/// A named test; it panics on failure.
type Test = (&'static str, fn(&'static BootInfo));

/// Runs every test and exits QEMU.
pub fn run(info: &'static BootInfo) -> ! {
    let tests: &[Test] = &[
        ("boot information", boot_information),
        ("breakpoint exception", breakpoint),
        ("exception report format", report_format),
    ];
    for (name, test) in tests {
        kprintln!("selftest: {name} ...");
        test(info);
        kprintln!("selftest: {name} ok");
    }
    kprintln!("selftest: OK");
    frame::qemu::exit(frame::qemu::ExitCode::Success);
}

fn boot_information(info: &'static BootInfo) {
    assert!(info.is_valid());
    assert!(info.region_count > 0, "empty memory map");
    assert_eq!(info.hhdm_base, frame::bootinfo::HHDM_BASE);
    assert_eq!(info.kernel_virt_base, frame::bootinfo::KERNEL_BASE);
    let sorted = info
        .regions()
        .windows(2)
        .all(|w| w[0].phys_end() <= w[1].phys_start);
    assert!(sorted, "memory map not sorted or overlapping");
}

fn breakpoint(_: &'static BootInfo) {
    let before = frame::breakpoint_count();
    frame::breakpoint();
    frame::breakpoint();
    assert_eq!(frame::breakpoint_count(), before + 2);
}

/// A fixed-size text buffer; formatting fails when it is full.
struct Buffer {
    bytes: [u8; 1024],
    len: usize,
}

impl Write for Buffer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len + s.len();
        self.bytes
            .get_mut(self.len..end)
            .ok_or(fmt::Error)?
            .copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

fn report_format(_: &'static BootInfo) {
    let frame = TrapFrame {
        vector: 14,
        error_code: 0b11,
        rip: 0xFFFF_FFFF_8000_0042,
        cs: 0x08,
        rsp: 0xFFFF_8000_0000_1000,
        ss: 0x10,
        ..TrapFrame::default()
    };
    let mut buffer = Buffer {
        bytes: [0; 1024],
        len: 0,
    };
    write!(
        buffer,
        "{}",
        Report {
            frame: &frame,
            cr2: 0x1000
        }
    )
    .expect("report too long");
    let text = core::str::from_utf8(&buffer.bytes[..buffer.len]).expect("report not UTF-8");
    for needle in [
        "exception 14 (#PF page fault), error code 0x3",
        "RIP=0xffffffff80000042",
        "CR2=0x0000000000001000",
        "RSP=0xffff800000001000 SS=0x0010",
    ] {
        assert!(text.contains(needle), "report lacks {needle:?}:\n{text}");
    }
}
