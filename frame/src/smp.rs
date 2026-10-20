// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Starting the application processors (`docs/smp.md`).
//!
//! An AP wakes up in real mode at the page a Startup IPI names (Intel SDM
//! Vol. 3A 9.4.4 "MP Initialization Example", 11.6.1 "Interrupt Command
//! Register"; OSDev Wiki "SMP"). The loader reserved four pages below
//! 640 KiB (`BootInfo::ap_trampoline_phys`):
//!
//! ```text
//!  page 0  the trampoline: 16-bit → 32-bit (temporary GDT, PAE, CR3 =
//!          page 1, EFER.LME/NXE, paging) → 64-bit → jump to `ap_entry`
//!          on its own kernel stack; parameters at the end of the page
//!  page 1  temporary PML4: the kernel's 512 entries, plus entry 0 →
//!  page 2  PDPT: entry 0 →
//!  page 3  PD: entry 0 = the first 2 MiB identity mapped (the trampoline)
//! ```
//!
//! One AP at a time: INIT, 10 ms, SIPI, 200 µs, SIPI (if it has not
//! started yet), then up to 100 ms for it to report in. `ap_entry` gives
//! the AP the kernel's page tables, its own GDT/TSS, the IDT, the MSRs
//! (PAT, syscall), SMEP, SSE, its per-CPU area, its local APIC and timer,
//! its boot context, and calls the kernel's hook (its scheduler).

use crate::arch::x86_64::{apic, context, gdt, idt, pat, syscall};
use crate::mm::paging;
use crate::sync::{Once, SpinLock};
use crate::task::KernelStack;
use bootinfo::HHDM_BASE;
use core::arch::global_asm;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

global_asm!(
    r#"
    .section .text.stela_ap, "ax"
    .global stela_ap_start
    .global stela_ap_end
    .global stela_ap_pm_target
    .global stela_ap_base32
    .global stela_ap_lm_target
    .global stela_ap_gdtr
    .global stela_ap_params
    .code16
stela_ap_start:
    cli
    cld
    movw %cs, %ax
    movw %ax, %ds
    lgdtl (stela_ap_gdtr - stela_ap_start)
    movl %cr0, %eax
    orl $1, %eax
    movl %eax, %cr0
    .byte 0x66, 0xEA
stela_ap_pm_target:
    .long 0
    .word 0x08
    .code32
stela_ap_pm:
    movw $0x10, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %ss
    .byte 0xBB
stela_ap_base32:
    .long 0
    movl %cr4, %eax
    orl $0x20, %eax
    movl %eax, %cr4
    movl (stela_ap_params - stela_ap_start)(%ebx), %eax
    movl %eax, %cr3
    movl $0xC0000080, %ecx
    rdmsr
    orl (stela_ap_params - stela_ap_start + 4)(%ebx), %eax
    wrmsr
    movl %cr0, %eax
    orl $0x80010001, %eax
    movl %eax, %cr0
    .byte 0xEA
stela_ap_lm_target:
    .long 0
    .word 0x18
    .code64
stela_ap_lm:
    movw $0x10, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %ss
    movl %ebx, %ebx
    movq (stela_ap_params - stela_ap_start + 8)(%rbx), %rsp
    movq (stela_ap_params - stela_ap_start + 24)(%rbx), %rdi
    movq (stela_ap_params - stela_ap_start + 16)(%rbx), %rax
    xorl %ebp, %ebp
    pushq $0
    jmp *%rax
    .balign 8
stela_ap_gdt:
    .quad 0
    .quad 0x00CF9A000000FFFF
    .quad 0x00CF92000000FFFF
    .quad 0x00AF9A000000FFFF
stela_ap_gdtr:
    .word 31
    .long 0
    .balign 8
stela_ap_params:
    .long 0
    .long 0
    .quad 0
    .quad 0
    .quad 0
stela_ap_end:
    .code64
    .text
"#,
    options(att_syntax)
);

unsafe extern "C" {
    static stela_ap_start: u8;
    static stela_ap_end: u8;
    static stela_ap_pm_target: u8;
    static stela_ap_base32: u8;
    static stela_ap_lm_target: u8;
    static stela_ap_gdtr: u8;
    static stela_ap_params: u8;
}

/// Offsets of the parameters (`stela_ap_params`): temporary CR3 (u32),
/// EFER bits to set (u32), stack top (u64), entry (u64), argument (u64).
const PARAM_CR3: usize = 0;
const PARAM_EFER: usize = 4;
const PARAM_STACK: usize = 8;
const PARAM_ENTRY: usize = 16;
const PARAM_ARG: usize = 24;

/// IA32_EFER.LME (bit 8) and NXE (bit 11), Vol. 4 Table 2-2.
const EFER_LME: u32 = 1 << 8;
const EFER_NXE: u32 = 1 << 11;

/// The kernel's hook for a started AP (its scheduler; never returns).
static AP_HOOK: Once<fn() -> !> = Once::new();
/// Set by an AP once it is set up.
static STARTED: AtomicBool = AtomicBool::new(false);
/// The kernel's page table root, for the APs.
static KERNEL_CR3: AtomicU64 = AtomicU64::new(0);

/// Registers the function each application processor runs once it is set
/// up (with interrupts disabled; it must enable them). Only the first
/// registration counts.
pub fn set_ap_entry(hook: fn() -> !) {
    let _ = AP_HOOK.set(hook);
}

fn offset(sym: &u8) -> usize {
    // SAFETY: only the addresses of the linker symbols are taken.
    sym as *const u8 as usize - unsafe { &stela_ap_start as *const u8 as usize }
}

/// The trampoline's code and data, as linked.
fn trampoline_bytes() -> &'static [u8] {
    // SAFETY: `stela_ap_start..stela_ap_end` is the trampoline in the
    // kernel's text, mapped and never written.
    unsafe {
        let start = &stela_ap_start as *const u8;
        let len = &stela_ap_end as *const u8 as usize - start as usize;
        core::slice::from_raw_parts(start, len)
    }
}

/// Writes `data` at physical address `phys` (through the direct map).
///
/// # Safety
///
/// The range must be memory the kernel owns and nothing else uses.
unsafe fn poke(phys: u64, data: &[u8]) {
    // SAFETY: the caller's guarantee; the HHDM maps all physical memory.
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), (HHDM_BASE + phys) as *mut u8, data.len())
    };
}

/// Starts every enabled processor of the MADT (at most `cpu::MAX_CPUS`).
/// Call once, from the boot processor, with interrupts enabled (the
/// waits use the clock) and the kernel's AP hook registered. Returns the
/// number of processors online.
pub fn start_aps() -> usize {
    if crate::boot_flag("nosmp") {
        crate::kprintln!("smp: off (boot option nosmp)");
        return crate::cpu::online();
    }
    let page = crate::ap_trampoline_phys();
    if page == 0 {
        crate::kprintln!("smp: no trampoline page from the loader; one processor");
        return crate::cpu::online();
    }
    let me = crate::cpu::current().apic_id();
    let others: alloc::vec::Vec<u32> = crate::acpi::madt()
        .processors
        .iter()
        .filter(|p| p.enabled && u32::from(p.apic_id) != me)
        .map(|p| u32::from(p.apic_id))
        .collect();
    if others.is_empty() {
        return crate::cpu::online();
    }
    if let Err(e) = crate::irq::register(crate::irq::IPI_VECTOR, reschedule_ipi) {
        crate::kprintln!("smp: no reschedule vector ({e:?}); one processor");
        return crate::cpu::online();
    }
    if let Err(e) = crate::irq::register(SHOOTDOWN_VECTOR, shootdown_ipi) {
        crate::kprintln!("smp: no shootdown vector ({e:?}); one processor");
        return crate::cpu::online();
    }
    KERNEL_CR3.store(paging::kernel_root(), Ordering::Release);
    prepare_tables(page);
    for apic_id in others {
        let index = crate::cpu::online();
        if index >= crate::cpu::MAX_CPUS {
            crate::kprintln!(
                "smp: more than {} processors; the rest stay off",
                crate::cpu::MAX_CPUS
            );
            break;
        }
        if !start_one(page, apic_id, index) {
            crate::kprintln!("smp: processor with APIC id {apic_id} did not start");
        }
    }
    let n = crate::cpu::online();
    crate::kprintln!("smp: {n} processor(s) online");
    n
}

/// Pages 1-3: the temporary page tables.
fn prepare_tables(page: u64) {
    let pml4 = page + 0x1000;
    let pdpt = page + 0x2000;
    let pd = page + 0x3000;
    let mut table = [0u8; 4096];
    // SAFETY: the kernel's PML4 is mapped through the HHDM and only read.
    unsafe {
        core::ptr::copy_nonoverlapping(
            (HHDM_BASE + paging::kernel_root()) as *const u8,
            table.as_mut_ptr(),
            4096,
        )
    };
    // Present, writable (Vol. 3A Table 4-15..4-17); PS for the 2 MiB page.
    table[0..8].copy_from_slice(&(pdpt | 0x3).to_le_bytes());
    let mut p = [0u8; 4096];
    p[0..8].copy_from_slice(&(pd | 0x3).to_le_bytes());
    let mut d = [0u8; 4096];
    d[0..8].copy_from_slice(&(0x83u64).to_le_bytes());
    // SAFETY: pages 1-3 of the loader's reservation, used by nothing else.
    unsafe {
        poke(pml4, &table);
        poke(pdpt, &p);
        poke(pd, &d);
    }
}

/// Copies and patches the trampoline for processor `index`, sends
/// INIT-SIPI-SIPI to `apic_id` and waits for it.
fn start_one(page: u64, apic_id: u32, index: usize) -> bool {
    let Some(stack) = KernelStack::new() else {
        return false;
    };
    let stack_top = stack.top();
    // The AP's first context runs on this stack for good.
    core::mem::forget(stack);
    let mut code = alloc::vec::Vec::from(trampoline_bytes());
    let base = page as u32;
    let put32 = |code: &mut alloc::vec::Vec<u8>, at: usize, v: u32| {
        code[at..at + 4].copy_from_slice(&v.to_le_bytes())
    };
    // SAFETY: only the symbols' addresses are taken.
    let (pm, b32, lm, gdtr, params) = unsafe {
        (
            offset(&stela_ap_pm_target),
            offset(&stela_ap_base32),
            offset(&stela_ap_lm_target),
            offset(&stela_ap_gdtr),
            offset(&stela_ap_params),
        )
    };
    // The far jumps' targets follow their operands: 6 bytes after the
    // 32-bit one (imm32 + selector), 6 after the 64-bit one.
    put32(&mut code, pm, base + (pm + 6) as u32);
    put32(&mut code, b32, base);
    put32(&mut code, lm, base + (lm + 6) as u32);
    // The GDT starts 32 bytes before its pointer.
    put32(&mut code, gdtr + 2, base + (gdtr - 32) as u32);
    let nx = paging::nx();
    put32(&mut code, params + PARAM_CR3, base + 0x1000);
    put32(
        &mut code,
        params + PARAM_EFER,
        EFER_LME | if nx { EFER_NXE } else { 0 },
    );
    code[params + PARAM_STACK..params + PARAM_STACK + 8].copy_from_slice(&stack_top.to_le_bytes());
    code[params + PARAM_ENTRY..params + PARAM_ENTRY + 8]
        .copy_from_slice(&(ap_entry as *const () as u64).to_le_bytes());
    code[params + PARAM_ARG..params + PARAM_ARG + 8].copy_from_slice(&(index as u64).to_le_bytes());
    // SAFETY: page 0 of the loader's reservation, used by nothing else
    // (the previous AP has finished with it: it reported in).
    unsafe { poke(page, &code) };
    STARTED.store(false, Ordering::Release);
    // INIT (delivery mode 101, level assert), then two Startup IPIs
    // (delivery mode 110, vector = page number), Vol. 3A 9.4.4.1.
    apic::send_icr(apic_id, 0x4500);
    wait_ns(10_000_000, || false);
    let vector = (page >> 12) as u32;
    apic::send_icr(apic_id, 0x4600 | vector);
    if !wait_ns(200_000, || STARTED.load(Ordering::Acquire)) {
        apic::send_icr(apic_id, 0x4600 | vector);
    }
    wait_ns(100_000_000, || STARTED.load(Ordering::Acquire))
}

/// Spins until `done()` or `ns` passed.
fn wait_ns(ns: u64, done: impl Fn() -> bool) -> bool {
    let end = crate::time::now_ns() + ns;
    while crate::time::now_ns() < end {
        if done() {
            return true;
        }
        core::hint::spin_loop();
    }
    done()
}

/// Where an application processor arrives in 64-bit mode, on its own
/// stack, with the temporary page tables and interrupts disabled.
extern "C" fn ap_entry(index: u64) -> ! {
    // SAFETY: the kernel's tables map everything the kernel uses (the
    // temporary ones only added the identity page, no longer needed).
    unsafe { paging::load_root(KERNEL_CR3.load(Ordering::Acquire)) };
    // SAFETY: once per AP, interrupts off, before its per-CPU area.
    let tss = unsafe { gdt::init_ap() };
    idt::load();
    pat::init_ap();
    let apic_id = apic::init_ap(crate::irq::SPURIOUS_VECTOR);
    crate::cpu::init_this_cpu(index as u32, apic_id, tss);
    syscall::init();
    crate::arch::x86_64::enable_smep();
    context::enable_sse();
    crate::task::init();
    crate::time::init_ap();
    STARTED.store(true, Ordering::Release);
    match AP_HOOK.get() {
        Some(hook) => hook(),
        None => crate::halt(),
    }
}

/// Vector of the TLB shootdown IPI.
pub const SHOOTDOWN_VECTOR: u8 = 0xFD;

/// The reschedule IPI: the handler only asks for a reschedule, which the
/// end of the interrupt carries out (and an idle processor wakes up).
fn reschedule_ipi(_: &crate::trap::TrapFrame) {
    crate::cpu::current().set_need_resched();
}

static SHOOT_LOCK: SpinLock<()> = SpinLock::new(());
static SHOOT_ADDR: AtomicU64 = AtomicU64::new(0);
/// Generation of the latest shootdown request.
static SHOOT_GEN: AtomicU64 = AtomicU64::new(0);
/// The latest generation each processor has carried out.
static SHOOT_ACKED: [AtomicU64; crate::cpu::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::cpu::MAX_CPUS];
static SHOOT_WARNED: AtomicBool = AtomicBool::new(false);

/// Carries out the latest shootdown request, if this processor has not
/// yet. Called by the IPI handler, and by processors that wait for a
/// shootdown of their own (with interrupts off they cannot take the IPI).
fn serve_shootdown() {
    let me = crate::cpu::current().id() as usize;
    let generation = SHOOT_GEN.load(Ordering::Acquire);
    if SHOOT_ACKED[me].load(Ordering::Acquire) < generation {
        paging::invlpg(SHOOT_ADDR.load(Ordering::Acquire));
        SHOOT_ACKED[me].store(generation, Ordering::Release);
    }
}

fn shootdown_ipi(_: &crate::trap::TrapFrame) {
    serve_shootdown();
}

/// Makes every other processor forget its TLB entry for kernel address
/// `virt` (after an unmap in the kernel half), waiting until they did.
/// Requests are serialised; a processor waiting for its turn (or for the
/// answers) serves the current request itself, since it may be running
/// with interrupts disabled. A processor that does not answer within
/// about 50 million spins is given up on (said once) rather than waited
/// for forever.
pub(crate) fn shootdown(virt: u64) {
    let online = crate::cpu::online();
    if online <= 1 {
        return;
    }
    let _guard = loop {
        if let Some(guard) = SHOOT_LOCK.try_lock() {
            break guard;
        }
        serve_shootdown();
        core::hint::spin_loop();
    };
    let me = crate::cpu::current().id() as usize;
    SHOOT_ADDR.store(virt, Ordering::Release);
    let generation = SHOOT_GEN.fetch_add(1, Ordering::AcqRel) + 1;
    SHOOT_ACKED[me].store(generation, Ordering::Release);
    apic::broadcast_ipi(SHOOTDOWN_VECTOR);
    // The clock may not advance with interrupts off: count spins instead.
    let mut spins: u64 = 0;
    while (0..online).any(|c| SHOOT_ACKED[c].load(Ordering::Acquire) < generation) {
        core::hint::spin_loop();
        spins += 1;
        if spins > 50_000_000 {
            if !SHOOT_WARNED.swap(true, Ordering::Relaxed) {
                crate::kprintln!("smp: a TLB shootdown was not answered (reported once)");
            }
            break;
        }
    }
}
