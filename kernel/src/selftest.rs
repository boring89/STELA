// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Self tests run at boot by the `selftest` build (`make test-qemu`). Each
//! test panics on failure; the panic handler then exits QEMU with a failure
//! status. When all pass, `selftest: OK` is printed and QEMU exits with 33.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::{self, Write};
use core::hint::black_box;
use frame::bootinfo::BootInfo;
use frame::kprintln;
use frame::mm::paging::{self, Flags};
use frame::mm::{DYNAMIC_BASE, HEAP_SIZE, heap, pmm};
use frame::trap::{Report, TrapFrame};

/// A named test; it panics on failure.
type Test = (&'static str, fn(&'static BootInfo));

/// Runs every test and exits QEMU.
pub fn run(info: &'static BootInfo) -> ! {
    let tests: &[Test] = &[
        ("boot information", boot_information),
        ("breakpoint exception", breakpoint),
        ("exception report format", report_format),
        ("frame allocator", frame_allocator),
        ("page mapping", page_mapping),
        ("heap", heap),
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

fn frame_allocator(_: &'static BootInfo) {
    let (free_before, total) = pmm::stats();
    assert!(free_before > 0 && free_before < total);
    let a = pmm::alloc_frame().expect("no frame");
    let b = pmm::alloc_zeroed_frame().expect("no frame");
    assert_ne!(a.addr(), b.addr());
    assert!(a.addr().is_multiple_of(4096) && a.addr() != 0);
    assert_eq!(pmm::stats().0, free_before - 2);
    let run = pmm::alloc_contiguous(16).expect("no contiguous frames");
    assert_eq!(run.count(), 16);
    assert_eq!(pmm::stats().0, free_before - 18);
    pmm::free_frame(a);
    pmm::free_frame(b);
    pmm::free_contiguous(run);
    assert_eq!(pmm::stats().0, free_before);
    kprintln!("selftest:   {} of {} frames free", free_before, total);
}

fn page_mapping(info: &'static BootInfo) {
    // The kernel image is mapped at its link address...
    assert_eq!(
        paging::translate(info.kernel_virt_base),
        Some(info.kernel_phys_base)
    );
    // ...the HHDM maps physical memory...
    assert_eq!(
        paging::translate(info.hhdm_base + 0x1234_5678),
        Some(0x1234_5678)
    );
    // ...and the loader's identity map is gone.
    assert_eq!(paging::translate(info.kernel_phys_base), None);
    assert_eq!(paging::translate(0x1000), None);

    let virt = DYNAMIC_BASE + (1 << 30);
    let frame = pmm::alloc_frame().expect("no frame");
    let phys = frame.addr();
    paging::map(virt, frame, Flags::DATA).expect("map failed");
    assert_eq!(paging::translate(virt + 0x10), Some(phys + 0x10));
    let again = pmm::alloc_frame().expect("no frame");
    assert!(matches!(
        paging::map(virt, again, Flags::DATA),
        Err(paging::MapError::AlreadyMapped)
    ));
    let frame = paging::unmap(virt).expect("unmap failed");
    assert_eq!(frame.addr(), phys);
    assert_eq!(paging::translate(virt), None);
    assert!(paging::unmap(virt).is_err());
    pmm::free_frame(frame);
    // Outside the dynamic region: refused.
    let outside = pmm::alloc_frame().expect("no frame");
    assert!(matches!(
        paging::map(info.kernel_virt_base, outside, Flags::DATA),
        Err(paging::MapError::OutOfRegion)
    ));
}

fn heap(_: &'static BootInfo) {
    let before = heap::stats();
    assert_eq!(before.size, HEAP_SIZE as usize);
    {
        // black_box keeps the optimiser from removing the allocations.
        let numbers: Vec<u64> = black_box((1..=10_000).collect());
        assert_eq!(numbers.iter().sum::<u64>(), 50_005_000);
        let mut text = String::new();
        write!(text, "STELA {}", 42).expect("format failed");
        text.push_str(" heap");
        assert_eq!(black_box(&text).as_str(), "STELA 42 heap");
        let big: Box<[u8]> = black_box(vec![7u8; 256 * 1024].into_boxed_slice());
        assert_eq!(big[256 * 1024 - 1], 7);
        let during = heap::stats();
        assert!(during.used >= before.used + 80_000 + 256 * 1024);
        kprintln!(
            "selftest:   heap {} KiB, {} KiB in use",
            during.size / 1024,
            during.used / 1024
        );
    }
    assert_eq!(heap::stats().used, before.used);
}
