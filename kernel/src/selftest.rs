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
use frame::mm::{DYNAMIC_BASE, HEAP_SIZE, MMIO_BASE, heap, mmio, pmm};
use frame::sync::{IrqGuard, SpinLock, interrupts_enabled};
use frame::trap::{Report, TrapFrame};
use frame::{cpu, irq, kprintln, time};
use kernel::task;

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
        ("timer tick", timer_tick),
        ("clock and sleep", clock_and_sleep),
        ("keyboard", keyboard),
        ("kernel threads", kernel_threads),
        ("thread reaping", thread_reaping),
        ("preemption", preemption),
        ("sleep order", sleep_order),
        ("user mode faults", user_mode_faults),
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
    // Everything (address spaces, page tables, kernel stacks) went back.
    assert_eq!(pmm::stats().0, free, "process memory not freed");
    assert!(interrupts_enabled());
}
