// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The global descriptor table (GDT) and the task-state segment (TSS).
//!
//! In 64-bit mode segmentation is mostly switched off, but the processor
//! still needs a code segment with the L (64-bit) bit, a data segment for
//! SS, and a TSS whose interrupt stack table (IST) gives the double-fault
//! handler a known-good stack (Intel SDM Vol. 3A 3.4.5 "Segment
//! Descriptors", 5.2.1 "Code-Segment Descriptor in 64-bit Mode",
//! 7.2.3 "TSS Descriptor in 64-bit mode", 7.7 "Task Management in 64-bit
//! Mode", Figure 7-11 "64-Bit TSS Format").
//!
//! ```text
//!  index  selector  descriptor
//!  0      0x00      null
//!  1      0x08      kernel code  (64-bit, DPL 0)
//!  2      0x10      kernel data  (DPL 0)
//!  3      0x18      user data    (DPL 3; selector with RPL 3: 0x1B)
//!  4      0x20      user code    (64-bit, DPL 3; with RPL 3: 0x23)
//!  5-6    0x28      TSS          (16-byte system descriptor)
//! ```
//!
//! The order is the one `syscall`/`sysret` require (Vol. 2B "SYSCALL",
//! "SYSRET"; Vol. 3A 5.8.8 "Fast System Calls in 64-Bit Mode"): with
//! IA32_STAR[47:32] = 0x08, `syscall` loads CS = 0x08 and SS = 0x10; with
//! IA32_STAR[63:48] = 0x10, `sysretq` loads SS = 0x10 + 8 = 0x18 and CS =
//! 0x10 + 16 = 0x20, both with RPL 3.

use crate::sync::RacyCell;
use core::arch::asm;
use core::mem::size_of;

/// Selector of the kernel code segment.
pub(crate) const KERNEL_CODE: u16 = 0x08;
/// Selector of the kernel data segment.
pub(crate) const KERNEL_DATA: u16 = 0x10;
/// Selector of the user data segment, with RPL 3.
pub(crate) const USER_DATA: u16 = 0x18 | 3;
/// Selector of the user code segment, with RPL 3.
pub(crate) const USER_CODE: u16 = 0x20 | 3;
/// Selector of the TSS.
const TSS_SELECTOR: u16 = 0x28;

/// IST slot (1-based, as stored in an IDT gate) used for double faults.
pub(crate) const DOUBLE_FAULT_IST: u8 = 1;

/// Kernel code: base 0, limit 0xFFFFF, G=1, L=1, D=0, P=1, DPL=0, S=1,
/// type 0xA (execute/read).
const CODE_DESCRIPTOR: u64 = 0x00AF_9A00_0000_FFFF;
/// Kernel data: base 0, limit 0xFFFFF, G=1, D/B=1, P=1, DPL=0, S=1,
/// type 0x2 (read/write).
const DATA_DESCRIPTOR: u64 = 0x00CF_9200_0000_FFFF;
/// User data: as kernel data with DPL=3 (bits 45-46).
const USER_DATA_DESCRIPTOR: u64 = 0x00CF_F200_0000_FFFF;
/// User code: as kernel code with DPL=3.
const USER_CODE_DESCRIPTOR: u64 = 0x00AF_FA00_0000_FFFF;

/// The 64-bit TSS (Figure 7-11): RSP0 (the stack for interrupts that
/// arrive in ring 3, set on every context switch) and the IST.
#[repr(C, packed(4))]
struct Tss {
    reserved0: u32,
    /// Stack pointers loaded on a privilege change to ring 0-2.
    rsp: [u64; 3],
    reserved1: u64,
    /// Interrupt stack table: IST1..IST7.
    ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    /// Offset of the I/O permission bitmap; `size_of::<Tss>()` means none.
    iomap_base: u16,
}

const _: () = assert!(size_of::<Tss>() == 104);

#[repr(C, align(16))]
struct Gdt([u64; 7]);

#[repr(C, align(16))]
struct Stack([u8; DOUBLE_FAULT_STACK_SIZE]);

const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024;

static GDT: RacyCell<Gdt> = RacyCell::new(Gdt([
    0,
    CODE_DESCRIPTOR,
    DATA_DESCRIPTOR,
    USER_DATA_DESCRIPTOR,
    USER_CODE_DESCRIPTOR,
    0,
    0,
]));
static TSS: RacyCell<Tss> = RacyCell::new(Tss {
    reserved0: 0,
    rsp: [0; 3],
    reserved1: 0,
    ist: [0; 7],
    reserved2: 0,
    reserved3: 0,
    iomap_base: size_of::<Tss>() as u16,
});
static DOUBLE_FAULT_STACK: RacyCell<Stack> = RacyCell::new(Stack([0; DOUBLE_FAULT_STACK_SIZE]));

/// The operand of `lgdt` and `lidt`: limit (size - 1) and linear base
/// (Intel SDM Vol. 3A 3.5.1, Figure 3-11).
#[repr(C, packed(2))]
pub(crate) struct DescriptorTablePointer {
    pub(crate) limit: u16,
    pub(crate) base: u64,
}

/// The two halves of a 64-bit TSS descriptor (Figure 7-4): type 0x9
/// (available 64-bit TSS), present, DPL 0, byte granularity.
const fn tss_descriptor(base: u64, limit: u64) -> (u64, u64) {
    let low = (limit & 0xFFFF)
        | ((base & 0xFF_FFFF) << 16)
        | (0x9 << 40)
        | (1 << 47)
        | (((limit >> 16) & 0xF) << 48)
        | (((base >> 24) & 0xFF) << 56);
    (low, base >> 32)
}

/// Sets the TSS's RSP0, the stack the processor switches to when an
/// interrupt or exception arrives in ring 3 (Vol. 3A 6.14.4 "Stack Switching
/// in IA-32e Mode"). Called on every context switch.
pub(crate) fn set_kernel_stack(top: u64) {
    let tss = TSS.get();
    // SAFETY: the TSS is a static; RSP0 is only read by the processor on a
    // privilege change, which cannot happen on this processor while it runs
    // this kernel code. The packed field is written unaligned, without
    // creating a reference.
    unsafe {
        core::ptr::addr_of_mut!((*tss).rsp)
            .cast::<u64>()
            .write_unaligned(top)
    };
}

/// Loads the GDT and the TSS and reloads every segment register.
///
/// # Safety
///
/// Must be called once, on the boot processor, before interrupts are
/// enabled and before anything else uses `GDT` or `TSS`.
pub(crate) unsafe fn init() {
    let tss = TSS.get();
    let gdt = GDT.get();
    let stack_top = DOUBLE_FAULT_STACK.get() as u64 + DOUBLE_FAULT_STACK_SIZE as u64;
    // SAFETY: the caller guarantees single-threaded first use, so these are
    // the only accesses to TSS and GDT. Fields of the packed TSS are written
    // whole (no references to unaligned fields are created).
    unsafe {
        let mut ist = [0; 7];
        ist[usize::from(DOUBLE_FAULT_IST) - 1] = stack_top;
        (*tss).ist = ist;
        let (low, high) = tss_descriptor(tss as u64, size_of::<Tss>() as u64 - 1);
        (*gdt).0[5] = low;
        (*gdt).0[6] = high;
    }
    let pointer = DescriptorTablePointer {
        limit: (size_of::<Gdt>() - 1) as u16,
        base: gdt as u64,
    };
    // SAFETY: the GDT is a static, so it outlives the processor's use of it.
    // Its code and data descriptors match what the running code already
    // assumes (flat 64-bit), so reloading the segment registers does not
    // change any address. CS cannot be written with `mov`; a far return pops
    // the new CS:RIP (Intel SDM Vol. 2B "RET": far return). The TSS
    // descriptor is valid and not yet busy, as `ltr` requires.
    unsafe {
        asm!("lgdt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
        asm!(
            "push {code}",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "retfq",
            "2:",
            code = const KERNEL_CODE as u64,
            tmp = lateout(reg) _,
            options(preserves_flags),
        );
        asm!(
            "mov ds, {0:x}",
            "mov es, {0:x}",
            "mov fs, {0:x}",
            "mov gs, {0:x}",
            "mov ss, {0:x}",
            in(reg) KERNEL_DATA,
            options(nostack, preserves_flags),
        );
        asm!("ltr {0:x}", in(reg) TSS_SELECTOR, options(nostack, preserves_flags));
    }
}
