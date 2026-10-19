// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The register state saved when an exception arrives, and its printed form.
//!
//! The layout matches the stubs in `arch/x86_64/exceptions.rs`: the stub
//! pushes the vector number (and a zero when the processor pushes no error
//! code) on top of the frame the processor pushed (Intel SDM Vol. 3A 6.14.2
//! "64-Bit Mode Stack Frame", Figure 6-9), then the 15 general-purpose
//! registers. This module is pure data and builds on the host for tests.

use core::fmt;

/// Registers at the time of an exception, lowest address first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
#[allow(missing_docs)]
pub struct TrapFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    /// Exception vector (0–31).
    pub vector: u64,
    /// The error code the processor pushed, or 0 for vectors without one.
    pub error_code: u64,
    // Pushed by the processor:
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

/// The mnemonic and name of an exception vector (Intel SDM Vol. 3A 6.3.1,
/// Table 6-1 "Protected-Mode Exceptions and Interrupts").
pub const fn exception_name(vector: u64) -> &'static str {
    match vector {
        0 => "#DE divide error",
        1 => "#DB debug",
        2 => "NMI",
        3 => "#BP breakpoint",
        4 => "#OF overflow",
        5 => "#BR bound range exceeded",
        6 => "#UD invalid opcode",
        7 => "#NM device not available",
        8 => "#DF double fault",
        9 => "coprocessor segment overrun",
        10 => "#TS invalid TSS",
        11 => "#NP segment not present",
        12 => "#SS stack-segment fault",
        13 => "#GP general protection",
        14 => "#PF page fault",
        16 => "#MF x87 floating-point error",
        17 => "#AC alignment check",
        18 => "#MC machine check",
        19 => "#XM SIMD floating-point exception",
        20 => "#VE virtualization exception",
        21 => "#CP control protection",
        28 => "#HV hypervisor injection",
        29 => "#VC VMM communication",
        30 => "#SX security exception",
        _ => "reserved",
    }
}

/// An exception as printed in a panic message.
#[derive(Clone, Copy, Debug)]
pub struct Report<'a> {
    /// The saved registers.
    pub frame: &'a TrapFrame,
    /// CR2 (the faulting address of a page fault).
    pub cr2: u64,
}

impl fmt::Display for Report<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let t = self.frame;
        writeln!(
            f,
            "exception {} ({}), error code {:#x}",
            t.vector,
            exception_name(t.vector),
            t.error_code
        )?;
        writeln!(
            f,
            "  RIP={:#018x} CS={:#06x} RFLAGS={:#010x}",
            t.rip, t.cs, t.rflags
        )?;
        writeln!(
            f,
            "  RSP={:#018x} SS={:#06x} CR2={:#018x}",
            t.rsp, t.ss, self.cr2
        )?;
        writeln!(
            f,
            "  RAX={:#018x} RBX={:#018x} RCX={:#018x} RDX={:#018x}",
            t.rax, t.rbx, t.rcx, t.rdx
        )?;
        writeln!(
            f,
            "  RSI={:#018x} RDI={:#018x} RBP={:#018x} R8 ={:#018x}",
            t.rsi, t.rdi, t.rbp, t.r8
        )?;
        writeln!(
            f,
            "  R9 ={:#018x} R10={:#018x} R11={:#018x} R12={:#018x}",
            t.r9, t.r10, t.r11, t.r12
        )?;
        write!(
            f,
            "  R13={:#018x} R14={:#018x} R15={:#018x}",
            t.r13, t.r14, t.r15
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    #[test]
    fn frame_layout_matches_the_stubs() {
        // 15 registers + vector + error code + 5 words pushed by the CPU.
        assert_eq!(size_of::<TrapFrame>(), 22 * 8);
        assert_eq!(core::mem::offset_of!(TrapFrame, vector), 15 * 8);
        assert_eq!(core::mem::offset_of!(TrapFrame, rip), 17 * 8);
    }

    #[test]
    fn report_shows_vector_and_registers() {
        let frame = TrapFrame {
            vector: 14,
            error_code: 2,
            rip: 0xFFFF_FFFF_8000_1234,
            cs: 0x08,
            rflags: 0x202,
            rsp: 0xFFFF_8000_0001_0000,
            ss: 0x10,
            rax: 0xAA,
            ..TrapFrame::default()
        };
        let text = std::format!(
            "{}",
            Report {
                frame: &frame,
                cr2: 0xDEAD_B000
            }
        );
        assert!(text.starts_with("exception 14 (#PF page fault), error code 0x2\n"));
        assert!(text.contains("RIP=0xffffffff80001234 CS=0x0008 RFLAGS=0x00000202"));
        assert!(text.contains("CR2=0x00000000deadb000"));
        assert!(text.contains("RAX=0x00000000000000aa"));
    }

    #[test]
    fn names_cover_reserved_vectors() {
        assert_eq!(exception_name(15), "reserved");
        assert_eq!(exception_name(31), "reserved");
        assert_eq!(exception_name(8), "#DF double fault");
    }
}
