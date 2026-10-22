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
use frame::mm::user::UserFlags;
use frame::mm::{DYNAMIC_BASE, HEAP_INITIAL, HEAP_MAX, MMIO_BASE, heap, mmio, pmm};
use frame::sync::{IrqGuard, SpinLock, interrupts_enabled};
use frame::trap::{Report, TrapFrame};
use frame::{cpu, irq, kprintln, time};
use kernel::task;

/// Set by the panic screen test: the panic handler halts instead of
/// leaving QEMU.
static PANIC_TEST: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Whether this is the panic screen test.
pub fn panic_test() -> bool {
    PANIC_TEST.load(Ordering::Acquire)
}

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
        ("copy on write", copy_on_write),
        ("heap", heap),
        ("locks and interrupts", locks),
        ("mmio mapping", mmio_mapping),
        ("acpi madt", acpi_madt),
        ("acpi fadt", acpi_fadt),
        ("interrupt controllers", interrupt_controllers),
        ("interrupt dispatch", interrupt_dispatch),
        ("timer tick", timer_tick),
        ("clock and sleep", clock_and_sleep),
        ("real-time clock", real_time_clock),
        ("keyboard", keyboard),
        ("mouse", mouse),
        ("kernel threads", kernel_threads),
        ("thread reaping", thread_reaping),
        ("preemption", preemption),
        ("sleep order", sleep_order),
        ("user mode faults", user_mode_faults),
        ("system calls", system_calls),
        ("elf loading", elf_loading),
        ("initrd programs", initrd_programs),
        ("ports", ports),
        ("ipc programs", ipc_programs),
        ("pci", pci),
        ("block devices", block_devices),
        ("network card", network_card),
        ("network stack", network_stack),
        ("file systems", file_systems),
        ("processes", processes),
        ("linux personality", linux_personality),
        ("smp", smp),
        ("framebuffer object", framebuffer_object),
    ];
    // Let the service threads (console) reach their first wait, so that
    // their start-up allocations do not land in the middle of a test.
    task::sleep_ms(1);
    // The boot screen stays a second, then the text console comes back.
    task::sleep_ms(1000);
    frame::console::end_boot_screen();
    // `make test-panic` puts a file `panic-test` in the initrd: panic on
    // purpose and leave the panic screen up.
    if kernel::vfs::find("Initrd").is_some_and(|v| v.fs.lookup(v.fs.root(), "panic-test").is_ok()) {
        PANIC_TEST.store(true, Ordering::Release);
        panic!("selftest: the panic screen test (a deliberate panic)");
    }
    for (name, test) in tests {
        kprintln!("selftest: {name} ...");
        test(info);
        kprintln!("selftest: {name} ok");
    }
    kernel::vfs::unmount_all();
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
    // make esp-test writes a boot.cfg with this option (docs/p520.md).
    assert!(frame::boot_flag("selftest"), "boot.cfg not read");
    assert!(!frame::boot_flag("nosmp"));
    let trampoline = frame::ap_trampoline_phys();
    assert!(
        trampoline != 0 && trampoline < 0xA_0000 && trampoline.is_multiple_of(4096),
        "bad AP trampoline page {trampoline:#x}"
    );
    kprintln!(
        "boot: options \"{}\", AP trampoline at {:#x}",
        core::str::from_utf8(frame::cmdline()).unwrap_or("?"),
        trampoline
    );
}

/// The free frame count once it reaches `expected`, or after 200 ms: a
/// process whose end `wait_process` reported may still be freeing its
/// memory on another processor (`docs/smp.md`).
/// The free frame count once it stops changing (two readings 5 ms apart
/// agree).
fn stable_frames() -> usize {
    let mut last = pmm::stats().0;
    for _ in 0..100 {
        task::sleep_ms(5);
        let now = pmm::stats().0;
        if now == last {
            break;
        }
        last = now;
    }
    last
}

fn settled_frames(expected: usize) -> usize {
    for _ in 0..200 {
        if pmm::stats().0 == expected {
            break;
        }
        task::sleep_ms(1);
    }
    pmm::stats().0
}

/// The application processors (`docs/smp.md`), when there are any: every
/// processor's timer ticks; wake-ups cross processors both ways; one
/// spinning process per processor (round robin); killing them from the
/// boot processor ends them where they run.
fn smp(_: &'static BootInfo) {
    static BLOCKER: AtomicU64 = AtomicU64::new(0);
    static WOKEN: AtomicU64 = AtomicU64::new(0);
    let n = cpu::online();
    if n < 2 {
        kprintln!("smp: one processor (skipped)");
        return;
    }
    let before: Vec<u64> = (0..n).map(cpu::ticks_of).collect();
    task::sleep_ms(50);
    for (i, b) in before.iter().enumerate() {
        assert!(
            cpu::ticks_of(i) > b + 10,
            "processor {i}'s timer does not tick"
        );
    }
    // A thread on processor 1 wakes the main thread (processor 0).
    let me = task::current_id();
    task::spawn_on(1, "smp-waker", move || {
        task::sleep_ms(20);
        task::wake(me);
    })
    .expect("spawn_on");
    let start = time::now_ns();
    assert!(
        task::block_until(Some(start + 2_000_000_000)),
        "not woken from processor 1"
    );
    // The main thread wakes a thread blocked on processor 1.
    let id = task::spawn_on(1, "smp-sleeper", || {
        assert_eq!(cpu::current().id(), 1, "spawn_on(1) ran elsewhere");
        BLOCKER.store(task::current_id(), Ordering::Release);
        task::block();
        WOKEN.store(1, Ordering::Release);
    })
    .expect("spawn_on");
    while BLOCKER.load(Ordering::Acquire) != id {
        task::sleep_ms(1);
    }
    task::sleep_ms(20);
    task::wake(id);
    let deadline = time::now_ns() + 1_000_000_000;
    while WOKEN.load(Ordering::Acquire) == 0 {
        assert!(
            time::now_ns() < deadline,
            "the thread on processor 1 was not woken"
        );
        task::sleep_ms(1);
    }
    // One spinner (jmp $) per processor.
    let flags = UserFlags {
        writable: false,
        executable: true,
    };
    let mut spinners = Vec::new();
    for i in 0..n {
        let process = task::process::create("spinner").expect("no address space");
        let page = pmm::alloc_zeroed_frame().expect("no frame");
        process.space().map(CODE_BASE, page, flags).expect("map");
        process
            .space()
            .load(CODE_BASE, &[0xEB, 0xFE])
            .expect("load");
        task::spawn_user(&process, CODE_BASE, STACK_TOP - 8, 0).expect("spawn");
        kprintln!(
            "smp:   spinner {i}: process {} on processor {}",
            process.id(),
            process.cpu()
        );
        spinners.push(process);
    }
    let mut cpus: Vec<usize> = spinners.iter().map(|p| p.cpu()).collect();
    cpus.sort_unstable();
    cpus.dedup();
    assert_eq!(cpus.len(), n, "the spinners share processors");
    let ticks = time::ticks();
    task::sleep_ms(30);
    assert!(time::ticks() - ticks < 100, "main starved by the spinners");
    for p in &spinners {
        assert_eq!(task::process::exit_status(p.id()), None);
        task::kill(p, 99);
    }
    for p in spinners {
        let pid = p.id();
        drop(p);
        assert_eq!(task::wait_process(pid), Some(99), "spinner {pid}");
    }
    kprintln!("smp: {n} processors: ticks, wake both ways, {n} spinners killed across processors");
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

/// P13 K6: a cloned address space shares the frames until one side
/// writes; the counts go back to nothing and every frame comes back.
fn copy_on_write(_: &'static BootInfo) {
    use frame::mm::cow;
    use frame::mm::user::AddressSpace;
    let rw = UserFlags {
        writable: true,
        executable: false,
    };
    let ro = UserFlags {
        writable: false,
        executable: false,
    };
    let (free_before, _) = pmm::stats();
    let shared_before = cow::shared_frames();
    {
        let va = 0x40_0000;
        let parent = AddressSpace::new().expect("an address space");
        let frame = pmm::alloc_zeroed_frame().expect("a frame");
        parent.map(va, frame, rw).expect("map rw");
        let frame = pmm::alloc_zeroed_frame().expect("a frame");
        parent.map(va + 4096, frame, ro).expect("map ro");
        parent.write(va, b"parent").expect("write");
        let before_clone = pmm::stats().0;
        let child = parent.clone_user().expect("clone");
        // The same frames, counted twice; only page tables were new.
        assert_eq!(parent.translate(va), child.translate(va));
        assert_eq!(parent.translate(va + 4096), child.translate(va + 4096));
        assert_eq!(cow::shared_frames(), shared_before + 2);
        assert!(before_clone - pmm::stats().0 <= 4, "fork copied pages");
        // A shared page that was writable passes the write check (the
        // write copies it); the read-only one does not.
        assert!(child.check(va, 6, true));
        assert!(!child.check(va + 4096, 1, true));
        // The child writes: it gets its own copy, the parent keeps its.
        child.write(va, b"child!").expect("child write");
        assert_ne!(parent.translate(va), child.translate(va));
        let mut buf = [0u8; 6];
        parent.read(va, &mut buf).expect("read");
        assert_eq!(&buf, b"parent");
        child.read(va, &mut buf).expect("read");
        assert_eq!(&buf, b"child!");
        assert_eq!(cow::shared_frames(), shared_before + 1);
        // A read-only page stays read-only.
        assert!(child.write(va + 4096, b"x").is_err());
        // The parent is the last user of its frame: no copy, the same frame.
        let mine = parent.translate(va);
        assert!(parent.resolve_cow(va));
        assert_eq!(parent.translate(va), mine);
        assert!(!parent.resolve_cow(va), "not shared any more");
        drop(child);
        assert_eq!(cow::shared_frames(), shared_before);
    }
    assert_eq!(pmm::stats().0, free_before, "frames leaked");
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
    assert!(before.size >= HEAP_INITIAL as usize && before.size <= HEAP_MAX as usize);
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
    // P12: an allocation larger than what is free makes the heap grow
    // (1 MiB steps, at most HEAP_MAX); it does not shrink afterwards.
    let size = heap::stats().free + (2 << 20);
    {
        let big: Box<[u8]> = black_box(vec![9u8; size].into_boxed_slice());
        assert_eq!(big[size - 1], 9);
        let grown = heap::stats();
        assert!(grown.size > before.size && grown.size <= HEAP_MAX as usize);
        kprintln!("selftest:   the heap grew to {} KiB", grown.size / 1024);
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

fn acpi_fadt(_: &'static BootInfo) {
    // QEMU q35 has a FADT with a PM1a control block and a plain \_S5.
    let Some(fadt) = frame::acpi::fadt() else {
        panic!("no FADT");
    };
    assert_ne!(fadt.pm1a_control, 0, "no PM1a control block");
    let Some((a, b)) = frame::acpi::s5() else {
        panic!("no \\_S5 in the DSDT");
    };
    assert!(frame::power::can_shutdown());
    kprintln!(
        "selftest:   PM1a {:#x}, SLP_TYP {a}/{b}, DSDT {} bytes, reset register {:?}",
        fadt.pm1a_control,
        frame::acpi::dsdt().map_or(0, <[u8]>::len),
        fadt.reset
    );
}

fn interrupt_controllers(_: &'static BootInfo) {
    assert_eq!(irq::legacy_pic_masks(), 0xFFFF, "8259 not fully masked");
    let cpu = cpu::current();
    assert_eq!(cpu.id(), 0);
    // Every enabled processor of the MADT is started (docs/smp.md).
    let enabled = frame::acpi::madt()
        .processors
        .iter()
        .filter(|p| p.enabled)
        .count();
    let expected = if frame::boot_flag("nosmp") {
        1
    } else {
        enabled.min(cpu::MAX_CPUS)
    };
    assert_eq!(cpu::online(), expected);
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
    let deadline = time::ticks() + 5 * time::TICK_HZ;
    while text.chars().count() < TYPED.chars().count() {
        let Some(InputEvent::Key(event)) = input::pop() else {
            assert!(
                time::ticks() < deadline,
                "no keys within 5 s (typed so far: {text:?})"
            );
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

fn mouse(_: &'static BootInfo) {
    assert!(input::mouse_present(), "no PS/2 mouse");
    let entry = irq::isa_entry(12).expect("IRQ 12 has no I/O APIC input");
    assert_eq!(entry & 0x1_00FF, u32::from(irq::isa_vector(12)));
    while input::pop().is_some() {}
    // qemu_test.py moves the mouse right 40 and down 30 and clicks.
    kprintln!("mouse: waiting");
    let (mut x, mut y, mut clicked) = (0i32, 0i32, false);
    let deadline = time::ticks() + 5 * time::TICK_HZ;
    while !(x >= 40 && y >= 30 && clicked) {
        let Some(InputEvent::Mouse(event)) = input::pop() else {
            assert!(
                time::ticks() < deadline,
                "no mouse within 5 s (so far {x},{y}, clicked {clicked})"
            );
            frame::halt_until_interrupt();
            continue;
        };
        x += i32::from(event.dx);
        y += i32::from(event.dy);
        clicked |= event.buttons & abi::key::button::LEFT != 0;
    }
    kprintln!("selftest:   mouse moved {x},{y}, left button");
    kprintln!("mouse: OK");
}

fn timer_tick(_: &'static BootInfo) {
    let start = time::ticks();
    // At 1000 Hz, 10 ticks take 10 ms; each `hlt` ends at the next
    // interrupt, so 1000 halts are plenty unless the timer is dead.
    for _ in 0..1000 {
        if time::ticks() >= start + 10 {
            break;
        }
        frame::halt_until_interrupt();
    }
    assert!(time::ticks() >= start + 10, "the timer does not tick");
    assert!(cpu::current().ticks() >= 10);
    assert!(irq::count(irq::TIMER_VECTOR) >= 10);
}

fn clock_and_sleep(_: &'static BootInfo) {
    let a = time::now_ns();
    let b = time::now_ns();
    assert!(b >= a, "clock went backwards");
    let start_ticks = time::ticks();
    let start = time::now_ns();
    time::sleep_ms(50);
    let elapsed_ticks = time::ticks() - start_ticks;
    let elapsed_ns = time::now_ns() - start;
    assert!(elapsed_ticks >= 50, "slept only {elapsed_ticks} ticks");
    if time::tsc_invariant() {
        kprintln!("tsc: {} MHz", time::tsc_hz() / 1_000_000);
        // The tick (APIC timer) and the TSC were calibrated against the
        // same PIT window, so they must agree within 20%.
        let ms = elapsed_ns / 1_000_000;
        kprintln!("selftest:   sleep_ms(50) took {ms} ms by the TSC");
        assert!((40..=60).contains(&ms), "sleep_ms(50) took {ms} ms");
    } else {
        kprintln!("tsc: not invariant");
        assert!(elapsed_ns >= 50_000_000);
    }
}

fn real_time_clock(_: &'static BootInfo) {
    // QEMU's RTC follows the host's clock (-rtc base=utc).
    let Some(t) = time::boot_time() else {
        panic!("the RTC could not be read");
    };
    assert!(
        (2024..=2100).contains(&t.year),
        "the RTC says year {}",
        t.year
    );
    let a = time::wall_clock_ns();
    time::sleep_ms(2);
    let b = time::wall_clock_ns();
    assert!(b >= a + 2_000_000, "the wall clock does not advance");
    assert!(kernel::fs::now() / 1_000_000_000 >= t.unix_seconds());
    let fadt = frame::acpi::fadt();
    kprintln!(
        "selftest:   {:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC, century register {:#x}",
        t.year,
        t.month,
        t.day,
        t.hour,
        t.minute,
        t.second,
        fadt.map_or(0, |f| f.century)
    );
}

/// Whose turn it is in the ping-pong test (0 or 1), and the shared count.
static TURN: AtomicU64 = AtomicU64::new(0);
static COUNT: AtomicU64 = AtomicU64::new(0);
static DONE: AtomicU64 = AtomicU64::new(0);
/// Set when a thread saw the other one's update between two of its own.
static ALTERNATED: AtomicU64 = AtomicU64::new(0);

const ROUNDS: u64 = 1000;

/// One side of the ping-pong: waits for its turn (yielding), bumps the
/// count, hands the turn over.
fn ping_pong(me: u64) {
    for round in 0..ROUNDS {
        while TURN.load(Ordering::Acquire) != me {
            task::yield_now();
        }
        // Strict alternation: before my n-th increment, both sides have
        // done exactly n (if I go first) or n + 1 / n increments.
        let count = COUNT.load(Ordering::Relaxed);
        if count == 2 * round + me {
            ALTERNATED.fetch_add(1, Ordering::Relaxed);
        }
        COUNT.store(count + 1, Ordering::Relaxed);
        TURN.store(1 - me, Ordering::Release);
    }
    DONE.fetch_add(1, Ordering::Release);
}

fn kernel_threads(_: &'static BootInfo) {
    assert_eq!(task::current_name(), "main");
    let main = task::current_id();
    let a = task::spawn("ping", || ping_pong(0)).expect("spawn failed");
    let b = task::spawn("pong", || ping_pong(1)).expect("spawn failed");
    assert!(a != main && b != main && a != b);
    assert_eq!(task::stats().ready, 2);
    let mut yields = 0u64;
    while DONE.load(Ordering::Acquire) < 2 {
        task::yield_now();
        yields += 1;
        assert!(yields < 1_000_000, "ping-pong did not finish");
    }
    assert_eq!(COUNT.load(Ordering::Relaxed), 2 * ROUNDS);
    assert_eq!(ALTERNATED.load(Ordering::Relaxed), 2 * ROUNDS);
    assert_eq!(task::current_id(), main);
    kprintln!(
        "selftest:   {} increments alternated, main yielded {yields} times",
        COUNT.load(Ordering::Relaxed)
    );
}

static EXITED: AtomicU64 = AtomicU64::new(0);

fn thread_reaping(_: &'static BootInfo) {
    let spawn_some = |n: u64| {
        let start = EXITED.load(Ordering::Acquire);
        for _ in 0..n {
            task::spawn("short", || {
                EXITED.fetch_add(1, Ordering::Release);
            })
            .expect("spawn failed");
        }
        while EXITED.load(Ordering::Acquire) < start + n {
            task::yield_now();
        }
        // The last one is freed by the switch back to us.
        task::yield_now();
    };
    // Warm up: the first stacks may create page tables, and the queues
    // grow their capacity; both stay.
    spawn_some(16);
    let (free, _) = pmm::stats();
    let heap = heap::stats().used;
    spawn_some(16);
    assert_eq!(task::stats().zombies, 0);
    assert_eq!(pmm::stats().0, free, "kernel stacks not freed");
    assert_eq!(heap::stats().used, heap, "thread control blocks not freed");
    kprintln!("selftest:   16 threads exited and freed, {free} frames free");
}

static SPIN_STOP: AtomicU64 = AtomicU64::new(0);
static SPINS: AtomicU64 = AtomicU64::new(0);
static SPIN_EXITED: AtomicU64 = AtomicU64::new(0);

fn preemption(_: &'static BootInfo) {
    // A thread that never yields: only the timer can take the processor
    // away from it.
    task::spawn("spinner", || {
        while SPIN_STOP.load(Ordering::Acquire) == 0 {
            SPINS.fetch_add(1, Ordering::Relaxed);
            core::hint::spin_loop();
        }
        SPIN_EXITED.store(1, Ordering::Release);
    })
    .expect("spawn failed");
    // Let it start, then sleep: main must come back within 100 ms although
    // the spinner keeps the processor busy.
    task::yield_now();
    for _ in 0..5 {
        let start = time::ticks();
        task::sleep_ms(10);
        let took = time::ticks() - start;
        assert!(
            (10..100).contains(&took),
            "sleep_ms(10) took {took} ticks next to a spinner"
        );
    }
    let spins = SPINS.load(Ordering::Relaxed);
    assert!(spins > 0, "the spinner never ran");
    // Without yielding, main still gets time slices.
    let start = time::ticks();
    while time::ticks() < start + 30 {
        core::hint::spin_loop();
    }
    assert!(
        SPINS.load(Ordering::Relaxed) > spins,
        "main was never preempted"
    );
    SPIN_STOP.store(1, Ordering::Release);
    while SPIN_EXITED.load(Ordering::Acquire) == 0 {
        task::yield_now();
    }
    kprintln!(
        "selftest:   spinner looped {} times",
        SPINS.load(Ordering::Relaxed)
    );
}

static WOKEN: SpinLock<Vec<u64>> = SpinLock::new(Vec::new());

fn sleep_order(_: &'static BootInfo) {
    for ms in [30u64, 10, 20, 0] {
        task::spawn("sleeper", move || {
            let start = time::now_ns();
            task::sleep_ms(ms);
            let slept = time::now_ns() - start;
            assert!(slept >= ms * 1_000_000, "sleep_ms({ms}) took {slept} ns");
            WOKEN.lock().push(ms);
        })
        .expect("spawn failed");
    }
    let deadline = time::ticks() + 1000;
    while WOKEN.lock().len() < 4 {
        assert!(time::ticks() < deadline, "sleepers did not wake up");
        task::sleep_ms(1);
    }
    assert_eq!(*WOKEN.lock(), [0, 10, 20, 30]);
    // Let the last one finish exiting.
    task::sleep_ms(1);
    assert_eq!(task::stats().sleeping, 0);
}

/// Where the built-in test programs are placed.
const CODE_BASE: u64 = 0x40_0000;
const STACK_TOP: u64 = 0x7FFF_FFF0_0000;

/// Runs `code` as a one-page user program in a new process and returns its
/// exit code.
fn run_user_code(name: &str, code: &[u8]) -> i64 {
    let process = task::process::create(name).expect("no address space");
    let space = process.space();
    let page = pmm::alloc_zeroed_frame().expect("no frame");
    let code_flags = UserFlags {
        writable: false,
        executable: true,
    };
    space.map(CODE_BASE, page, code_flags).expect("map code");
    space.load(CODE_BASE, code).expect("load code");
    let stack = pmm::alloc_zeroed_frame().expect("no frame");
    let stack_flags = UserFlags {
        writable: true,
        executable: false,
    };
    space
        .map(STACK_TOP - 4096, stack, stack_flags)
        .expect("map stack");
    // The kernel cannot write the code page as a user (it is read-only),
    // and addresses outside the user half are refused.
    assert!(space.write(CODE_BASE, &[0]).is_err());
    assert!(space.check(STACK_TOP - 16, 16, true));
    assert!(!space.check(STACK_TOP, 1, false));
    assert!(!space.check(u64::MAX - 4, 8, false));
    task::spawn_user(&process, CODE_BASE, STACK_TOP - 8, 0).expect("spawn_user failed");
    let pid = process.id();
    drop(process);
    task::wait_process(pid).expect("unknown process")
}

fn user_mode_faults(_: &'static BootInfo) {
    // Warm up the paths that allocate once (process table, page tables of
    // the kernel stack slots).
    assert_eq!(run_user_code("ud2", &[0x0F, 0x0B]), 128 + 6);
    let (free, _) = pmm::stats();
    // ud2: #UD in ring 3 -> the process is killed with 128 + 6.
    assert_eq!(run_user_code("ud2", &[0x0F, 0x0B]), 128 + 6);
    // mov al, [0]: #PF -> 128 + 14.
    assert_eq!(
        run_user_code("null", &[0x8A, 0x04, 0x25, 0, 0, 0, 0]),
        128 + 14
    );
    // A kernel address: #PF (the page is supervisor-only) -> 128 + 14.
    let mut code = vec![0x48, 0xA1]; // mov rax, [moffs64]
    code.extend_from_slice(&frame::bootinfo::KERNEL_BASE.to_le_bytes());
    assert_eq!(run_user_code("kernel", &code), 128 + 14);
    // hlt is privileged: #GP -> 128 + 13.
    assert_eq!(run_user_code("hlt", &[0xF4]), 128 + 13);
    // A loop that never ends is preempted; main keeps running.
    let process = task::process::create("loop").expect("no address space");
    let page = pmm::alloc_zeroed_frame().expect("no frame");
    let flags = UserFlags {
        writable: false,
        executable: true,
    };
    process.space().map(CODE_BASE, page, flags).expect("map");
    process
        .space()
        .load(CODE_BASE, &[0xEB, 0xFE])
        .expect("load"); // jmp $
    task::spawn_user(&process, CODE_BASE, STACK_TOP - 8, 0).expect("spawn");
    let start = time::ticks();
    task::sleep_ms(30);
    assert!(time::ticks() - start < 100, "main starved by a user loop");
    assert_eq!(task::process::exit_status(process.id()), None);
    task::kill(&process, 99);
    let pid = process.id();
    drop(process);
    assert_eq!(task::wait_process(pid), Some(99));
    // A thread killed while it sleeps inside a system call ends on its way
    // back to user mode, with nothing left behind on its kernel stack.
    // loop: mov eax, 4 (SleepNs); mov edi, 1000000; syscall; jmp loop
    let process = task::process::create("sleeper").expect("no address space");
    let page = pmm::alloc_zeroed_frame().expect("no frame");
    process.space().map(CODE_BASE, page, flags).expect("map");
    let code = [
        0xB8, 0x04, 0, 0, 0, 0xBF, 0x40, 0x42, 0x0F, 0x00, 0x0F, 0x05, 0xEB, 0xF2,
    ];
    process.space().load(CODE_BASE, &code).expect("load");
    task::spawn_user(&process, CODE_BASE, STACK_TOP - 8, 0).expect("spawn");
    task::sleep_ms(20);
    task::kill(&process, 77);
    let pid = process.id();
    drop(process);
    assert_eq!(task::wait_process(pid), Some(77));
    // Everything (address spaces, page tables, kernel stacks) went back.
    assert_eq!(settled_frames(free), free, "process memory not freed");
    assert!(interrupts_enabled());
}

/// A user program that exercises the system calls and exits with 42 if
/// every result is as expected (1 otherwise). Assembled with GNU as
/// (`.intel_syntax noprefix`) from:
///
/// ```text
/// start:
///     mov rbx, 0x1122334455667788
///     mov eax, 5
///     syscall
///     test rax, rax
///     jnz fail
///     mov r12, rdx
///     mov eax, 10
///     mov edi, 0x01000001
///     lea rsi, [rip + msg]
///     syscall
///     test rax, rax
///     jnz fail
///     mov eax, 10
///     mov edi, 0x01000001
///     mov rsi, 0xffffffff80000000
///     syscall
///     cmp rax, 3
///     jne fail
///     mov eax, 10
///     mov edi, 0x01000001
///     mov esi, 0x10000000
///     syscall
///     cmp rax, 3
///     jne fail
///     mov eax, 6
///     syscall
///     cmp rax, 1
///     jne fail
///     mov eax, 999
///     syscall
///     cmp rax, 1
///     jne fail
///     mov eax, 7
///     xor edi, edi
///     syscall
///     cmp rax, 6
///     jne fail
///     mov eax, 3
///     syscall
///     test rax, rax
///     jnz fail
///     mov eax, 4
///     mov edi, 2000000
///     syscall
///     test rax, rax
///     jnz fail
///     mov eax, 5
///     syscall
///     sub rdx, r12
///     cmp rdx, 2000000
///     jb fail
///     mov rax, 0x1122334455667788
///     cmp rbx, rax
///     jne fail
///     xor eax, eax
///     mov edi, 42
///     syscall
///     ud2
/// fail:
///     xor eax, eax
///     mov edi, 1
///     syscall
///     ud2
/// msg:
///     .long 1, msg_text_end - msg_text, 0, 0
/// msg_text:
///     .ascii "user: hello from built-in user code\n"
/// msg_text_end:
///     .fill 112 - (msg_text_end - msg_text), 1, 0
///     .long 0, 0, 0, 0
/// ```
const SYSCALL_TEST: &[u8] = &[
    0x48, 0xBB, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0xB8, 0x05, 0x00, 0x00, 0x00, 0x0F,
    0x05, 0x48, 0x85, 0xC0, 0x0F, 0x85, 0xC6, 0x00, 0x00, 0x00, 0x49, 0x89, 0xD4, 0xB8, 0x0A, 0x00,
    0x00, 0x00, 0xBF, 0x01, 0x00, 0x00, 0x01, 0x48, 0x8D, 0x35, 0xBD, 0x00, 0x00, 0x00, 0x0F, 0x05,
    0x48, 0x85, 0xC0, 0x0F, 0x85, 0xA7, 0x00, 0x00, 0x00, 0xB8, 0x0A, 0x00, 0x00, 0x00, 0xBF, 0x01,
    0x00, 0x00, 0x01, 0x48, 0xC7, 0xC6, 0x00, 0x00, 0x00, 0x80, 0x0F, 0x05, 0x48, 0x83, 0xF8, 0x03,
    0x0F, 0x85, 0x8A, 0x00, 0x00, 0x00, 0xB8, 0x0A, 0x00, 0x00, 0x00, 0xBF, 0x01, 0x00, 0x00, 0x01,
    0xBE, 0x00, 0x00, 0x00, 0x10, 0x0F, 0x05, 0x48, 0x83, 0xF8, 0x03, 0x75, 0x73, 0xB8, 0x06, 0x00,
    0x00, 0x00, 0x0F, 0x05, 0x48, 0x83, 0xF8, 0x01, 0x75, 0x66, 0xB8, 0xE7, 0x03, 0x00, 0x00, 0x0F,
    0x05, 0x48, 0x83, 0xF8, 0x01, 0x75, 0x59, 0xB8, 0x07, 0x00, 0x00, 0x00, 0x31, 0xFF, 0x0F, 0x05,
    0x48, 0x83, 0xF8, 0x06, 0x75, 0x4A, 0xB8, 0x03, 0x00, 0x00, 0x00, 0x0F, 0x05, 0x48, 0x85, 0xC0,
    0x75, 0x3E, 0xB8, 0x04, 0x00, 0x00, 0x00, 0xBF, 0x80, 0x84, 0x1E, 0x00, 0x0F, 0x05, 0x48, 0x85,
    0xC0, 0x75, 0x2D, 0xB8, 0x05, 0x00, 0x00, 0x00, 0x0F, 0x05, 0x4C, 0x29, 0xE2, 0x48, 0x81, 0xFA,
    0x80, 0x84, 0x1E, 0x00, 0x72, 0x1A, 0x48, 0xB8, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11,
    0x48, 0x39, 0xC3, 0x75, 0x0B, 0x31, 0xC0, 0xBF, 0x2A, 0x00, 0x00, 0x00, 0x0F, 0x05, 0x0F, 0x0B,
    0x31, 0xC0, 0xBF, 0x01, 0x00, 0x00, 0x00, 0x0F, 0x05, 0x0F, 0x0B, 0x01, 0x00, 0x00, 0x00, 0x24,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x75, 0x73, 0x65, 0x72, 0x3A,
    0x20, 0x68, 0x65, 0x6C, 0x6C, 0x6F, 0x20, 0x66, 0x72, 0x6F, 0x6D, 0x20, 0x62, 0x75, 0x69, 0x6C,
    0x74, 0x2D, 0x69, 0x6E, 0x20, 0x75, 0x73, 0x65, 0x72, 0x20, 0x63, 0x6F, 0x64, 0x65, 0x0A, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

fn system_calls(_: &'static BootInfo) {
    // ClockNowNs, PortSend to the console (good, kernel pointer, unmapped
    // pointer), the removed DebugWrite and an unknown number
    // (InvalidSyscall), HandleClose of a bad handle, Yield, SleepNs, rbx
    // preserved, ProcessExit.
    assert_eq!(run_user_code("syscalls", SYSCALL_TEST), 42);
    kprintln!(
        "selftest:   SMEP {}",
        if frame::arch::x86_64::smep_enabled() {
            "on"
        } else {
            "not available"
        }
    );
}

/// A minimal ELF executable: one PT_LOAD segment at 0x40_0000 holding
/// `code` (ELF-64 Object File Format 1.5, "File header", "Program header
/// table"), with permissions `flags` (PF_X = 1, PF_W = 2, PF_R = 4).
fn make_elf(code: &[u8], flags: u32) -> Vec<u8> {
    const HEADERS: usize = 64 + 56;
    let mut elf = vec![0u8; HEADERS];
    elf[..8].copy_from_slice(&[0x7F, b'E', b'L', b'F', 2, 1, 1, 0]);
    elf[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    elf[18..20].copy_from_slice(&62u16.to_le_bytes()); // EM_X86_64
    elf[20..24].copy_from_slice(&1u32.to_le_bytes());
    elf[24..32].copy_from_slice(&CODE_BASE.to_le_bytes()); // e_entry
    elf[32..40].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
    elf[52..54].copy_from_slice(&64u16.to_le_bytes()); // e_ehsize
    elf[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
    elf[56..58].copy_from_slice(&1u16.to_le_bytes()); // e_phnum
    let ph = &mut elf[64..];
    ph[..4].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
    ph[4..8].copy_from_slice(&flags.to_le_bytes());
    ph[8..16].copy_from_slice(&(HEADERS as u64).to_le_bytes()); // p_offset
    ph[16..24].copy_from_slice(&CODE_BASE.to_le_bytes()); // p_vaddr
    ph[32..40].copy_from_slice(&(code.len() as u64).to_le_bytes()); // p_filesz
    ph[40..48].copy_from_slice(&(code.len() as u64 + 8192).to_le_bytes()); // p_memsz
    elf.extend_from_slice(code);
    elf
}

fn elf_loading(_: &'static BootInfo) {
    use kernel::program::{self, LoadError};
    let image = make_elf(SYSCALL_TEST, 4 | 1);
    // Warm up, then check that a whole program's memory (segments, 1 MiB
    // stack, page tables) comes back.
    let pid = program::spawn("elf", &image).expect("spawn failed");
    assert_eq!(task::wait_process(pid), Some(42));
    let (free, _) = pmm::stats();
    let pid = program::spawn("elf", &image).expect("spawn failed");
    assert_eq!(task::wait_process(pid), Some(42));
    assert_eq!(settled_frames(free), free, "program memory not freed");
    // Refused: writable and executable, not ELF, missing from the initrd.
    assert_eq!(
        program::spawn("wx", &make_elf(SYSCALL_TEST, 7)),
        Err(LoadError::BadSegment)
    );
    assert!(matches!(
        program::spawn("junk", b"not an elf"),
        Err(LoadError::BadElf(_))
    ));
    assert_eq!(settled_frames(free), free, "failed loads leaked memory");
    assert_eq!(
        program::spawn_path("Initrd", "no-such-program", None).err(),
        Some(LoadError::NotFound)
    );
    // An unknown process id.
    assert_eq!(task::wait_process(u64::MAX), None);
    kprintln!("selftest:   initrd {} bytes", frame::mm::initrd().len());
}

/// Starts program `name` from the Initrd volume; returns its id.
fn initrd_program(name: &str) -> u64 {
    match kernel::program::spawn_path("Initrd", name, None) {
        Ok(p) => p.id(),
        Err(e) => panic!("cannot start {name}: {e:?}"),
    }
}

fn initrd_programs(_: &'static BootInfo) {
    assert!(!frame::mm::initrd().is_empty(), "no initrd");
    let (free, _) = pmm::stats();
    // hello: prints from two threads, joins the second, exits with 0.
    let pid = initrd_program("hello");
    assert_eq!(task::wait_process(pid), Some(0));
    // fault: reads address 0 -> page fault -> 128 + 14; the kernel goes on.
    let pid = initrd_program("fault");
    assert_eq!(task::wait_process(pid), Some(128 + 14));
    // spin: 200 ms without yielding; main still gets to run.
    let pid = initrd_program("spin");
    task::yield_now();
    for _ in 0..5 {
        let start = time::ticks();
        task::sleep_ms(10);
        let took = time::ticks() - start;
        assert!(took < 100, "main slept {took} ticks next to spin");
    }
    assert_eq!(task::wait_process(pid), Some(0));
    // Every process's memory came back.
    assert_eq!(task::stats().zombies, 0);
    assert_eq!(settled_frames(free), free, "program memory not freed");
}

fn ports(_: &'static BootInfo) {
    use abi::{Error, Message, PORT_QUEUE_DEPTH};
    use alloc::sync::Arc;
    use kernel::object::port::{KERNEL_OWNER, Port, QueuedMessage};
    let msg = |tag: u32| QueuedMessage::plain(Message::with_data(tag, b"x"));
    // The console service waits on its port all the time.
    let blocked = task::stats().blocked;
    let port = Port::new(KERNEL_OWNER);
    // Fills up to the depth, then Full; first in, first out.
    for i in 0..PORT_QUEUE_DEPTH as u32 {
        port.send(msg(i)).expect("send");
    }
    assert_eq!(port.send(msg(999)).err(), Some(Error::Full));
    for i in 0..PORT_QUEUE_DEPTH as u32 {
        assert_eq!(port.receive(None, 0).ok().map(|m| m.msg.tag), Some(i));
    }
    // Empty: no wait, then a 5 ms wait.
    let now = time::now_ns();
    assert_eq!(port.receive(Some(now), 0).err(), Some(Error::TimedOut));
    let start = time::now_ns();
    assert_eq!(
        port.receive(Some(start + 5_000_000), 0).err(),
        Some(Error::TimedOut)
    );
    assert!(time::now_ns() - start >= 5_000_000, "timed out early");
    assert_eq!(task::stats().sleeping, 0);
    // A receiver blocked forever, and one with a long deadline, are woken
    // by sends (the second leaves the sleep list early).
    static GOT: AtomicU64 = AtomicU64::new(0);
    for deadline in [None, Some(time::now_ns() + 10_000_000_000)] {
        GOT.store(0, Ordering::SeqCst);
        let p = Arc::clone(&port);
        task::spawn("receiver", move || {
            let tag = p.receive(deadline, 0).map_or(1, |m| m.msg.tag);
            GOT.store(u64::from(tag), Ordering::SeqCst);
        })
        .expect("spawn");
        task::sleep_ms(5);
        assert_eq!(GOT.load(Ordering::SeqCst), 0);
        let sent_at = time::now_ns();
        port.send(msg(7)).expect("send");
        while GOT.load(Ordering::SeqCst) == 0 {
            task::sleep_ms(1);
        }
        assert_eq!(GOT.load(Ordering::SeqCst), 7);
        assert!(time::now_ns() - sent_at < 1_000_000_000, "woken late");
    }
    // Closing wakes a blocked receiver with PeerClosed; queued messages
    // are still delivered first.
    GOT.store(0, Ordering::SeqCst);
    let p = Arc::clone(&port);
    task::spawn("receiver", move || {
        let closed = p.receive(None, 0).err() == Some(Error::PeerClosed);
        GOT.store(if closed { 2 } else { 1 }, Ordering::SeqCst);
    })
    .expect("spawn");
    task::sleep_ms(5);
    port.close();
    while GOT.load(Ordering::SeqCst) == 0 {
        task::sleep_ms(1);
    }
    assert_eq!(GOT.load(Ordering::SeqCst), 2);
    let port = Port::new(KERNEL_OWNER);
    port.send(msg(5)).expect("send");
    port.close();
    assert_eq!(port.send(msg(6)).err(), Some(Error::PeerClosed));
    assert_eq!(port.receive(None, 0).ok().map(|m| m.msg.tag), Some(5));
    assert_eq!(port.receive(None, 0).err(), Some(Error::PeerClosed));
    // No receiver is left blocked. The console service may be busy (not
    // blocked) at either moment: with SMP, programs on other processors
    // keep it printing (docs/smp.md).
    let settled = (0..200).any(|_| {
        if task::stats().blocked <= blocked.max(1) {
            return true;
        }
        task::sleep_ms(1);
        false
    });
    assert!(settled, "a receiver is still blocked");
}

/// Runs ipc_server and ipc_client (bootstrap: SEND to the server's main
/// port) and checks that both exit with 0.
fn run_ipc_pair() {
    use abi::Rights;
    use alloc::sync::Arc;
    use kernel::object::{Entry, KernelObject};
    use kernel::program::spawn_path;
    let server = spawn_path("Initrd", "ipc_server", None).expect("cannot start ipc_server");
    let to_server = Entry {
        object: KernelObject::Port(Arc::clone(server.main_port())),
        rights: Rights::SEND,
    };
    let client =
        spawn_path("Initrd", "ipc_client", Some(to_server)).expect("cannot start ipc_client");
    let (server, client) = (server.id(), client.id());
    assert_eq!(task::wait_process(client), Some(0), "ipc_client failed");
    assert_eq!(task::wait_process(server), Some(0), "ipc_server failed");
}

fn ipc_programs(_: &'static BootInfo) {
    use kernel::program::spawn_path;
    run_ipc_pair();
    let (free, _) = pmm::stats();
    run_ipc_pair();
    // A server blocked in PortReceive is killed: it ends with the kill's
    // exit code and leaves nothing behind.
    let server = spawn_path("Initrd", "ipc_server", None).expect("cannot start ipc_server");
    task::sleep_ms(20);
    assert_eq!(task::process::exit_status(server.id()), None);
    task::kill(&server, 99);
    let pid = server.id();
    drop(server);
    assert_eq!(task::wait_process(pid), Some(99));
    task::sleep_ms(1);
    assert_eq!(task::stats().zombies, 0);
    assert_eq!(settled_frames(free), free, "ipc memory not freed");
}

fn pci(_: &'static BootInfo) {
    use drivers::pci::{self, Bar, system};
    let devices = system::devices();
    assert!(system::uses_ecam(), "QEMU q35 has an MCFG");
    let host = devices
        .iter()
        .find(|f| f.class == 0x06 && f.subclass == 0x00)
        .expect("no host bridge");
    assert_eq!(host.address.bus, 0);
    // ECAM and the legacy ports agree on the first registers.
    let at = host.address;
    for offset in [0u16, 8, 0x2C] {
        assert_eq!(
            system::config().read32(at, offset),
            frame::pci::legacy_read32(at.bus, at.device, at.function, offset as u8)
        );
    }
    let ahci = devices
        .iter()
        .find(|f| f.is(0x01, 0x06, 0x01))
        .expect("no AHCI controller (q35 has one at 00:1f.2)");
    // ABAR is BAR5, a 32-bit memory BAR.
    assert!(matches!(
        pci::bar(system::config(), ahci, 5),
        Some(Bar::Memory { size, .. }) if size >= 0x1000
    ));
    assert!(pci::find_capability(system::config(), ahci.address, pci::capability::MSI).is_some());
    let first = frame::irq::alloc_vector().expect("no vector");
    let second = frame::irq::alloc_vector().expect("no vector");
    assert!(first >= frame::irq::FIRST_FREE_VECTOR && second == first + 1);
}

/// The first bytes of a disk image made for the storage tests
/// (`make test-storage`); only such disks are written.
const TEST_DISK_MAGIC: &[u8] = b"STELATEST";

/// The first network card (QEMU's default NIC with user networking, or
/// the one a test adds): a DHCP DISCOVER built by the `net` crate goes
/// out raw, and slirp's OFFER must come back within 5 seconds. Skipped
/// without a card.
fn network_card(_: &'static BootInfo) {
    use net::wire;
    let Some(card) = drivers::net::probe() else {
        kprintln!("net: no network card (skipped)");
        return;
    };
    let mac = card.mac();
    let xid = 0x5354_454C;
    let discover = net::dhcp::build(net::dhcp::DISCOVER, xid, mac, None, None);
    let udp = wire::build_udp(wire::ANY, wire::BROADCAST_IP, 68, 67, &discover);
    let ip = wire::build_ipv4(wire::ANY, wire::BROADCAST_IP, wire::PROTO_UDP, 1, &udp);
    let eth = wire::build_eth(wire::BROADCAST_MAC, mac, wire::ETHERTYPE_IPV4, &ip);
    card.send(&eth).expect("sending a frame");
    let deadline = time::now_ns() + 5_000_000_000;
    let mut buf = [0u8; 2048];
    let mut frames = 0;
    loop {
        while let Some(n) = card.receive(&mut buf) {
            frames += 1;
            let offer = wire::parse_eth(&buf[..n])
                .filter(|e| e.ethertype == wire::ETHERTYPE_IPV4)
                .and_then(|e| wire::parse_ipv4(e.payload))
                .filter(|p| p.proto == wire::PROTO_UDP)
                .and_then(|p| {
                    wire::parse_udp(p.src, p.dst, p.payload)
                        .filter(|u| u.dst_port == 68)
                        .and_then(|u| net::dhcp::parse(u.data, mac))
                });
            if let Some(o) = offer
                && o.xid == xid
                && o.kind == net::dhcp::OFFER
            {
                let [a, b, c, d] = o.your_ip;
                kprintln!(
                    "net: raw frames work: DHCP offer of {a}.{b}.{c}.{d} after {frames} frame(s)"
                );
                return;
            }
        }
        assert!(
            time::now_ns() < deadline,
            "no DHCP offer within 5 s ({frames} frames received)"
        );
        task::sleep_ms(10);
    }
}

/// The stack on the card: DHCP must give the interface an address
/// within 10 seconds (slirp: 10.0.2.15). Skipped without a card.
fn network_stack(_: &'static BootInfo) {
    kernel::net::start();
    if drivers::net::probe().is_none() {
        return;
    }
    assert!(
        kernel::net::wait_configured(10_000_000_000),
        "no address from DHCP"
    );
    kprintln!("net: stack configured");
}

fn block_devices(_: &'static BootInfo) {
    use block::BLOCK_SIZE;
    let disks = kernel::block::disks();
    kprintln!("selftest:   {} disk(s)", disks.len());
    for (name, disk) in disks {
        let mut first = vec![0u8; BLOCK_SIZE];
        disk.read(0, &mut first).expect("read block 0");
        if !first.starts_with(TEST_DISK_MAGIC) {
            kprintln!("selftest:   {name}: not a test disk; only read");
            continue;
        }
        let blocks = disk.block_count();
        assert!(blocks >= 64, "test disk too small");
        // 20 blocks (more than one 64 KiB AHCI command) written and read
        // back at block 8.
        let pattern: Vec<u8> = (0..20 * BLOCK_SIZE)
            .map(|i| (i / 7 + i / BLOCK_SIZE) as u8)
            .collect();
        disk.write(8, &pattern).expect("write");
        let mut back = vec![0u8; pattern.len()];
        disk.read(8, &mut back).expect("read back");
        assert!(back == pattern, "{name}: data read back differs");
        assert_eq!(
            disk.read(blocks, &mut first),
            Err(block::BlockError::OutOfRange)
        );
        // The boot counter in block 64 (between the partition table and
        // the first partition at 1 MiB) survives a restart.
        let last = 64;
        let mut counter = vec![0u8; BLOCK_SIZE];
        disk.read(last, &mut counter).expect("read last block");
        let boots = if counter.starts_with(b"BOOTS") {
            u64::from_le_bytes(counter[8..16].try_into().expect("8 bytes")) + 1
        } else {
            1
        };
        counter[..8].copy_from_slice(b"BOOTS\0\0\0");
        counter[8..16].copy_from_slice(&boots.to_le_bytes());
        disk.write(last, &counter).expect("write last block");
        disk.flush().expect("flush");
        kprintln!("storage: {name} boot {boots}");
    }
}

fn file_systems(_: &'static BootInfo) {
    let volumes = kernel::vfs::volumes();
    for v in &volumes {
        let (total, free) = v.fs.space();
        kprintln!(
            "selftest:   volume {} \"{}\": {} KiB, {} KiB free",
            v.index,
            v.name,
            total / 1024,
            free / 1024
        );
    }
    if let Some(boot) = kernel::vfs::find("boot") {
        let fs = &boot.fs;
        let hello = fs.lookup(fs.root(), "hello.txt").expect("Boot:/HELLO.TXT");
        let mut buf = [0u8; 32];
        let n = fs.read(hello.id, 0, &mut buf).expect("read");
        assert_eq!(&buf[..n], b"hello from FAT\n");
        assert_eq!(
            fs.create(fs.root(), "x", false).err(),
            Some(abi::Error::ReadOnly)
        );
    }
    if kernel::vfs::find("Data").is_none() {
        kprintln!("selftest:   no Data volume; fstest not run");
        return;
    }
    // P13 K15: fstest opens the volumes (VOLUMES).
    let volumes = alloc::vec![(
        abi::capability::VOLUMES,
        kernel::object::capability(abi::capability::VOLUMES),
    )];
    let pid = match kernel::program::spawn_path_with("Initrd", "fstest", None, volumes) {
        Ok(p) => p.id(),
        Err(e) => panic!("cannot start fstest: {e:?}"),
    };
    assert_eq!(task::wait_process(pid), Some(0), "fstest failed");
}

fn processes(_: &'static BootInfo) {
    // proctest starts copies of itself through ProcessSpawn, waits, kills
    // and uses the heap. Warm up once, then check that everything came
    // back.
    let pid = initrd_program("proctest");
    assert_eq!(task::wait_process(pid), Some(0), "proctest failed");
    task::sleep_ms(1);
    let (free, _) = pmm::stats();
    let pid = initrd_program("proctest");
    assert_eq!(task::wait_process(pid), Some(0), "proctest failed");
    task::sleep_ms(1);
    assert_eq!(task::stats().zombies, 0);
    assert_eq!(settled_frames(free), free, "process memory not freed");
}

/// A Linux program in raw machine code (no STELA note, so the kernel runs
/// it with the Linux personality):
///
/// ```text
///     lea  rsi, [rip + msg]
///     mov  edi, 1                 ; write(1, msg, len)
///     mov  edx, len
///     mov  eax, 1
///     syscall
///     cmp  rax, len               ; all written
///     jne  fail
///     cmp  rdx, len               ; Linux leaves RDX alone
///     jne  fail
///     mov  eax, 206               ; io_setup() -> -ENOSYS
///     syscall
///     neg  rax
///     mov  rdi, rax               ; exit_group(38)
///     mov  eax, 231
///     syscall
/// fail:
///     mov  edi, 1                 ; exit_group(1)
///     mov  eax, 231
///     syscall
/// msg: "linux: hello from raw system calls\n"
/// ```
const LINUX_TEST: &[u8] = &[
    0x48, 0x8D, 0x35, 0x3D, 0x00, 0x00, 0x00, 0xBF, 0x01, 0x00, 0x00, 0x00, 0xBA, 0x23, 0x00, 0x00,
    0x00, 0xB8, 0x01, 0x00, 0x00, 0x00, 0x0F, 0x05, 0x48, 0x83, 0xF8, 0x23, 0x75, 0x1A, 0x48, 0x83,
    0xFA, 0x23, 0x75, 0x14, 0xB8, 0xCE, 0x00, 0x00, 0x00, 0x0F, 0x05, 0x48, 0xF7, 0xD8, 0x48, 0x89,
    0xC7, 0xB8, 0xE7, 0x00, 0x00, 0x00, 0x0F, 0x05, 0xBF, 0x01, 0x00, 0x00, 0x00, 0xB8, 0xE7, 0x00,
    0x00, 0x00, 0x0F, 0x05, 0x6C, 0x69, 0x6E, 0x75, 0x78, 0x3A, 0x20, 0x68, 0x65, 0x6C, 0x6C, 0x6F,
    0x20, 0x66, 0x72, 0x6F, 0x6D, 0x20, 0x72, 0x61, 0x77, 0x20, 0x73, 0x79, 0x73, 0x74, 0x65, 0x6D,
    0x20, 0x63, 0x61, 0x6C, 0x6C, 0x73, 0x0A,
];

fn linux_personality(_: &'static BootInfo) {
    use kernel::program;
    use kernel::task::Personality;
    // Detection: programs built with libstela carry the STELA note.
    let initrd = kernel::vfs::find("Initrd").expect("no Initrd volume");
    let hello = initrd
        .fs
        .lookup(initrd.fs.root(), "hello")
        .expect("no hello in the initrd");
    let (_, image) = program::read_program(&initrd, hello.id).expect("cannot read hello");
    let elf = elfload::Elf::parse_with(&image, elfload::Layout::Packed).expect("bad hello");
    assert_eq!(program::personality_of(&elf), Personality::Stela);
    let raw = make_elf(LINUX_TEST, 4 | 1);
    let elf = elfload::Elf::parse_with(&raw, elfload::Layout::Packed).expect("bad raw ELF");
    assert_eq!(program::personality_of(&elf), Personality::Linux);
    // write to the tty, RDX preserved, ENOSYS, exit_group; twice, and the
    // second run gives back every frame (descriptors, tty, memory).
    let pid = program::spawn_with("linux-raw", &raw, None).expect("spawn failed");
    assert_eq!(task::wait_process(pid), Some(38));
    // With several processors the ended thread may be freed a little
    // after the exit is recorded (by the next thread on its processor).
    let free = stable_frames();
    let pid = program::spawn_with("linux-raw", &raw, None).expect("spawn failed");
    assert_eq!(task::wait_process(pid), Some(38));
    assert_eq!(settled_frames(free), free, "a Linux process leaked memory");
    // Forced personality: the same code as a STELA program makes STELA
    // calls (1 = ThreadExit), so it ends with code 0 without printing.
    let pid = program::spawn_with("forced", &raw, Some(Personality::Stela)).expect("spawn failed");
    assert_eq!(task::wait_process(pid), Some(0));
    // A musl program from Initrd:/linux: stdio, uname, TLS, exit code 3;
    // its memory comes back.
    let run = || match program::spawn_path("Initrd", "linux/hello", None) {
        Ok(p) => {
            assert_eq!(p.personality(), Personality::Linux);
            let pid = p.id();
            drop(p);
            task::wait_process(pid)
        }
        Err(e) => panic!("cannot start linux/hello: {e:?}"),
    };
    assert_eq!(run(), Some(3));
    let free = stable_frames();
    assert_eq!(run(), Some(3));
    assert_eq!(settled_frames(free), free, "a musl program leaked memory");
    // P12: threads (clone, futex) and processes (fork, execve, wait,
    // #!): their memory comes back too.
    // Started like the shell does: `[4]` is the program's folder, so
    // `/Initrd` is visible (for execve) even when `Data` is mounted.
    let run_path = |name: &str| {
        let folder = initrd
            .fs
            .lookup(initrd.fs.root(), "linux")
            .expect("no linux folder");
        let file = initrd.fs.lookup(folder.id, name).expect("no program");
        let (_, image) = program::read_program(&initrd, file.id).expect("cannot read");
        let mut initial = task::process::standard_initial(None);
        initial[3] = Some(kernel::object::Entry {
            object: kernel::object::KernelObject::Directory(kernel::vfs::Node {
                volume: initrd.clone(),
                id: folder.id,
            }),
            rights: abi::Rights::READ,
        });
        match program::spawn_process(name, &image, initial, None, Vec::new(), None, None) {
            Ok(p) => {
                let pid = p.id();
                drop(p);
                task::wait_process(pid)
            }
            Err(e) => panic!("cannot start {name}: {e:?}"),
        }
    };
    assert_eq!(run_path("threads"), Some(4));
    assert_eq!(run_path("procs"), Some(0));
    let free = stable_frames();
    assert_eq!(run_path("threads"), Some(4));
    assert_eq!(run_path("procs"), Some(0));
    assert_eq!(
        settled_frames(free),
        free,
        "Linux threads or processes leaked memory"
    );
}

fn framebuffer_object(_: &'static BootInfo) {
    let Some(fb) = frame::console::framebuffer() else {
        panic!("no framebuffer");
    };
    // The text console has drawn the boot messages (what test-fb checked
    // on a screendump before the desktop took the screen).
    let rows = frame::console::ink_rows();
    kprintln!("selftest:   the console drew on {rows} pixel rows");
    assert!(rows >= 3 * 32, "the console drew too little ({rows} rows)");
    assert!(
        fb.width >= 640 && fb.height >= 480,
        "{}x{}",
        fb.width,
        fb.height
    );
    assert!(u64::from(fb.pitch) * u64::from(fb.height) <= fb.range.size());
    // Mapped into a process write-combining (PAT entry 4), the pages are
    // the framebuffer's and come back unmapped.
    let process = task::process::create("fbmap").expect("no address space");
    let at = 0x2000_0000_0000;
    process
        .space()
        .map_device(at, &fb.range, true, true)
        .expect("cannot map the framebuffer");
    assert_eq!(process.space().translate(at), Some(fb.range.phys()));
    assert!(
        process.space().is_write_combining(at),
        "not write-combining"
    );
    let last = at + fb.range.size() - 4096;
    assert_eq!(
        process.space().translate(last),
        Some(fb.range.phys() + fb.range.size() - 4096)
    );
    let range = process.space().unmap_device(at).expect("not mapped");
    assert_eq!(range, fb.range);
    assert_eq!(process.space().translate(at), None);
    kprintln!(
        "selftest:   {}x{} pitch {} at {:#x}, {} pages, write-combining",
        fb.width,
        fb.height,
        fb.pitch,
        fb.range.phys(),
        fb.range.pages()
    );
}
