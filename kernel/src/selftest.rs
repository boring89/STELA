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
use core::sync::atomic::{AtomicU64, Ordering};
use frame::bootinfo::BootInfo;
use frame::input::{self, InputEvent};
use frame::mm::paging::{self, Flags};
use frame::mm::{DYNAMIC_BASE, HEAP_SIZE, MMIO_BASE, heap, mmio, pmm};
use frame::sync::{IrqGuard, SpinLock, interrupts_enabled};
use frame::trap::{Report, TrapFrame};
use frame::{cpu, irq, kprintln};

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
        ("locks and interrupts", locks),
        ("mmio mapping", mmio_mapping),
        ("acpi madt", acpi_madt),
        ("interrupt controllers", interrupt_controllers),
        ("interrupt dispatch", interrupt_dispatch),
        ("keyboard", keyboard),
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

fn locks(_: &'static BootInfo) {
    // frame::init enabled interrupts; guards turn them off and the
    // outermost one turns them back on.
    assert!(interrupts_enabled());
    let before = interrupts_enabled();
    {
        let _outer = IrqGuard::new();
        assert!(!interrupts_enabled());
        {
            let _inner = IrqGuard::new();
            assert!(!interrupts_enabled());
        }
        assert!(!interrupts_enabled());
    }
    assert_eq!(interrupts_enabled(), before);
    let lock = SpinLock::new(1);
    {
        let guard = lock.lock();
        assert!(!interrupts_enabled());
        assert_eq!(*guard, 1);
        assert!(lock.try_lock().is_none());
    }
    assert_eq!(interrupts_enabled(), before);
    assert!(lock.try_lock().is_some());
}

fn mmio_mapping(info: &'static BootInfo) {
    // RAM is refused.
    assert!(matches!(
        mmio::map(info.kernel_phys_base, 4096),
        Err(mmio::MmioError::Ram)
    ));
    assert!(matches!(
        mmio::map(0xFEE0_0000, 0),
        Err(mmio::MmioError::BadRange)
    ));
    let fb = &info.framebuffer;
    if !fb.is_present() {
        kprintln!("selftest:   no framebuffer; skipping the aliasing check");
        return;
    }
    // Map the first page of the framebuffer twice (at an unaligned offset
    // too): a write through one mapping is visible through the other.
    let a = mmio::map(fb.phys_addr, 4096).expect("map failed");
    let b = mmio::map(fb.phys_addr + 8, 16).expect("map failed");
    assert!(a.virt() >= MMIO_BASE && b.virt() >= MMIO_BASE);
    assert_ne!(a.virt() & !0xFFF, b.virt() & !0xFFF);
    assert_eq!(paging::translate(a.virt()), Some(fb.phys_addr));
    assert_eq!(paging::translate(b.virt()), Some(fb.phys_addr + 8));
    let saved = a.read_u32(8);
    a.write_u32(8, 0x0012_3456);
    assert_eq!(b.read_u32(0), 0x0012_3456);
    b.write_u32(0, saved);
    assert_eq!(a.read_u32(8), saved);
    // MMIO pages cannot be unmapped as if they were frames.
    assert!(paging::unmap(a.virt() & !0xFFF).is_err());
    kprintln!(
        "selftest:   framebuffer {:#x} mapped at {:#x}",
        fb.phys_addr,
        a.virt()
    );
}

fn acpi_madt(_: &'static BootInfo) {
    let madt = frame::acpi::madt();
    assert_ne!(madt.local_apic_address, 0);
    assert!(!madt.io_apics.is_empty(), "no I/O APIC");
    // CPUID.01H:EBX[31:24] is the initial APIC id of the running processor
    // (Intel SDM Vol. 2A "CPUID"), which must be an enabled MADT processor.
    let bsp = (core::arch::x86_64::__cpuid(1).ebx >> 24) as u8;
    assert!(
        madt.processors
            .iter()
            .any(|p| p.enabled && p.apic_id == bsp),
        "BSP APIC id {bsp} not in the MADT"
    );
    for p in &madt.processors {
        kprintln!(
            "selftest:   cpu uid {} apic id {}{}",
            p.processor_id,
            p.apic_id,
            if p.enabled { "" } else { " (disabled)" }
        );
    }
    for a in &madt.io_apics {
        kprintln!(
            "selftest:   I/O APIC {} at {:#x}, GSI base {}",
            a.id,
            a.address,
            a.gsi_base
        );
    }
    for o in &madt.overrides {
        kprintln!(
            "selftest:   IRQ {} -> GSI {} ({:?}, {:?})",
            o.source,
            o.gsi,
            o.polarity(),
            o.trigger()
        );
    }
    // QEMU wires the PIT (IRQ 0) to I/O APIC input 2.
    let (gsi, _, _) = madt.isa_irq(0);
    kprintln!("selftest:   ISA IRQ 0 arrives at GSI {gsi}");
}

fn interrupt_controllers(_: &'static BootInfo) {
    assert_eq!(irq::legacy_pic_masks(), 0xFFFF, "8259 not fully masked");
    let cpu = cpu::current();
    assert_eq!(cpu.id(), 0);
    assert_eq!(cpu::online(), 1);
    // CPUID.01H:EBX[31:24]: initial APIC id of this processor.
    let initial = core::arch::x86_64::__cpuid(1).ebx >> 24;
    assert_eq!(cpu.apic_id(), initial);
    kprintln!("selftest:   BSP APIC id {}", cpu.apic_id());
    assert!(interrupts_enabled());
}

/// Test vectors in the free range.
const TEST_VECTOR: u8 = 0x50;
const UNHANDLED_VECTOR: u8 = 0x51;

static TEST_HITS: AtomicU64 = AtomicU64::new(0);

fn test_handler(frame: &TrapFrame) {
    assert_eq!(frame.vector, u64::from(TEST_VECTOR));
    // Handlers run with interrupts off (interrupt gate).
    assert!(!interrupts_enabled());
    TEST_HITS.fetch_add(1, Ordering::Relaxed);
}

fn interrupt_dispatch(_: &'static BootInfo) {
    assert_eq!(irq::register(3, test_handler), Err(irq::IrqError::Reserved));
    assert_eq!(
        irq::register(irq::SPURIOUS_VECTOR, test_handler),
        Err(irq::IrqError::Reserved)
    );
    irq::register(TEST_VECTOR, test_handler).expect("register failed");
    assert_eq!(
        irq::register(TEST_VECTOR, test_handler),
        Err(irq::IrqError::Busy)
    );

    // A self-IPI with interrupts on arrives at once; the handler runs and
    // the EOI lets the next one in.
    let before = TEST_HITS.load(Ordering::Relaxed);
    for _ in 0..3 {
        irq::self_ipi(TEST_VECTOR);
    }
    assert_eq!(TEST_HITS.load(Ordering::Relaxed), before + 3);
    assert_eq!(irq::count(TEST_VECTOR), 3);

    // With interrupts off it stays pending; `sti; hlt` then wakes up for
    // it. The guard is forgotten so interrupts stay off until the `sti`.
    let guard = IrqGuard::new();
    irq::self_ipi(TEST_VECTOR);
    assert_eq!(TEST_HITS.load(Ordering::Relaxed), before + 3);
    core::mem::forget(guard);
    frame::halt_until_interrupt();
    assert_eq!(TEST_HITS.load(Ordering::Relaxed), before + 4);
    assert!(interrupts_enabled());

    // A vector without a handler is reported and acknowledged.
    irq::self_ipi(UNHANDLED_VECTOR);
    irq::self_ipi(UNHANDLED_VECTOR);
    assert_eq!(irq::count(UNHANDLED_VECTOR), 2);
    irq::unregister(TEST_VECTOR);
    irq::self_ipi(TEST_VECTOR);
    assert_eq!(TEST_HITS.load(Ordering::Relaxed), before + 4);
}

/// What `tools/qemu_test.py --send-keys` types after `input: waiting`.
const TYPED: &str = "stela\n";

fn keyboard(_: &'static BootInfo) {
    assert!(input::keyboard_present(), "no PS/2 keyboard");
    // IRQ 1 is routed to vector 33 and unmasked (bit 16 clear).
    let entry = irq::isa_entry(1).expect("IRQ 1 has no I/O APIC input");
    assert_eq!(entry & 0x1_00FF, u32::from(irq::isa_vector(1)));
    while input::pop().is_some() {}
    kprintln!("input: waiting");
    let mut text = alloc::string::String::new();
    while text.chars().count() < TYPED.chars().count() {
        let Some(InputEvent::Key(event)) = input::pop() else {
            frame::halt_until_interrupt();
            continue;
        };
        if let Some(ch) = input::key_to_char(&event) {
            kprintln!("key: {:?} {:?}", event.code, ch);
            text.push(ch);
        }
    }
    assert_eq!(text, TYPED);
    assert_eq!(input::dropped(), 0);
    kprintln!("input: OK");
}
