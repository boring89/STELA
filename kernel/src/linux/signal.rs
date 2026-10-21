// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Linux signals, the pure part (`docs/linux.md` §12): numbers, default
//! actions, `struct sigaction`, `siginfo_t`, and the signal frame a
//! handler runs on (`struct rt_sigframe`, Linux
//! `arch/x86/include/asm/sigframe.h`, `uapi/asm/sigcontext.h`).
//!
//! ```text
//!  user stack before the signal (rsp)
//!  ── 128-byte red zone (System V ABI §3.2.2), left alone
//!  ── FXSAVE image, 512 bytes, 64-aligned      ◄── uc_mcontext.fpstate
//!  ── struct rt_sigframe (440 bytes), so that the handler starts with
//!     rsp ≡ 8 (mod 16), as after a call:
//!       +0    pretcode   = sa_restorer (the handler's return address)
//!       +8    ucontext   uc_flags, uc_link, uc_stack (24 bytes),
//!       +48              uc_mcontext: r8..r15 rdi rsi rbp rbx rdx rax rcx
//!                        rsp rip eflags, cs/gs/fs/ss, err, trapno,
//!                        oldmask, cr2, fpstate, 8 reserved words
//!       +304             uc_sigmask (the mask to restore)
//!       +312  siginfo    128 bytes
//!  handler(sig, &siginfo, &ucontext); `ret` → restorer → rt_sigreturn
//! ```

use frame::trap::UserRegs;

#[allow(missing_docs)]
pub mod sig {
    pub const SIGHUP: u32 = 1;
    pub const SIGINT: u32 = 2;
    pub const SIGQUIT: u32 = 3;
    pub const SIGILL: u32 = 4;
    pub const SIGTRAP: u32 = 5;
    pub const SIGABRT: u32 = 6;
    pub const SIGBUS: u32 = 7;
    pub const SIGFPE: u32 = 8;
    pub const SIGKILL: u32 = 9;
    pub const SIGUSR1: u32 = 10;
    pub const SIGSEGV: u32 = 11;
    pub const SIGUSR2: u32 = 12;
    pub const SIGPIPE: u32 = 13;
    pub const SIGALRM: u32 = 14;
    pub const SIGTERM: u32 = 15;
    pub const SIGCHLD: u32 = 17;
    pub const SIGCONT: u32 = 18;
    pub const SIGSTOP: u32 = 19;
    pub const SIGTSTP: u32 = 20;
    pub const SIGTTIN: u32 = 21;
    pub const SIGTTOU: u32 = 22;
    pub const SIGURG: u32 = 23;
    pub const SIGWINCH: u32 = 28;
}

/// Highest signal number (signals are 1..=64).
pub const NSIG: u32 = 64;

/// `sa_handler` values.
pub const SIG_DFL: u64 = 0;
/// Ignore the signal.
pub const SIG_IGN: u64 = 1;

/// `sa_flags` bits (Linux `uapi/asm-generic/signal-defs.h`, x86 `signal.h`).
pub const SA_NOCLDSTOP: u64 = 1;
/// Siginfo handler (three arguments); STELA always passes all three.
pub const SA_SIGINFO: u64 = 4;
/// `sa_restorer` is valid.
pub const SA_RESTORER: u64 = 0x0400_0000;
/// Run on the alternate stack (recorded, not used).
pub const SA_ONSTACK: u64 = 0x0800_0000;
/// Restart interrupted system calls.
pub const SA_RESTART: u64 = 0x1000_0000;
/// Do not block the signal in its own handler.
pub const SA_NODEFER: u64 = 0x4000_0000;
/// Back to `SIG_DFL` after one delivery.
pub const SA_RESETHAND: u64 = 0x8000_0000;

/// `si_code` values.
pub const SI_USER: i32 = 0;
/// Sent by the kernel.
pub const SI_KERNEL: i32 = 0x80;
/// Sent by `tkill`/`tgkill`.
pub const SI_TKILL: i32 = -6;
/// SIGCHLD: the child exited.
pub const CLD_EXITED: i32 = 1;
/// SIGCHLD: the child was killed by a signal.
pub const CLD_KILLED: i32 = 2;
/// SIGSEGV: address not mapped.
pub const SEGV_MAPERR: i32 = 1;
/// SIGSEGV: no permission.
pub const SEGV_ACCERR: i32 = 2;

/// The mask bit of signal `sig` (1..=64).
pub const fn bit(sig: u32) -> u64 {
    1 << (sig - 1)
}

/// Signals that can be neither caught, blocked nor ignored.
pub const UNBLOCKABLE: u64 = bit(sig::SIGKILL) | bit(sig::SIGSTOP);

/// What a signal does without a handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefaultAction {
    /// The process ends (exit code 128 + signal).
    Terminate,
    /// Nothing (also the stop and continue signals: no job control).
    Ignore,
}

/// The default action of `sig` (signal(7)).
pub fn default_action(sig: u32) -> DefaultAction {
    use sig::*;
    match sig {
        SIGCHLD | SIGCONT | SIGURG | SIGWINCH | SIGSTOP | SIGTSTP | SIGTTIN | SIGTTOU => {
            DefaultAction::Ignore
        }
        // Real-time signals and the rest terminate.
        _ => DefaultAction::Terminate,
    }
}

/// Whether `sig` is a valid signal number.
pub fn valid(sig: u64) -> bool {
    (1..=u64::from(NSIG)).contains(&sig)
}

/// The kernel's `struct sigaction` (what `rt_sigaction` passes): handler,
/// flags, restorer, mask, 8 bytes each.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Action {
    /// `SIG_DFL`, `SIG_IGN` or the handler's address.
    pub handler: u64,
    /// `SA_*`.
    pub flags: u64,
    /// Where the handler returns (musl's `__restore_rt`).
    pub restorer: u64,
    /// Signals blocked while the handler runs.
    pub mask: u64,
}

impl Action {
    /// Decodes the 32 bytes of a `struct sigaction`.
    pub fn from_bytes(b: &[u8; 32]) -> Action {
        let word = |i: usize| {
            let mut w = [0u8; 8];
            w.copy_from_slice(&b[i * 8..i * 8 + 8]);
            u64::from_le_bytes(w)
        };
        Action {
            handler: word(0),
            flags: word(1),
            restorer: word(2),
            mask: word(3),
        }
    }

    /// Encodes it.
    pub fn to_bytes(&self) -> [u8; 32] {
        let mut b = [0u8; 32];
        for (i, w) in [self.handler, self.flags, self.restorer, self.mask]
            .iter()
            .enumerate()
        {
            b[i * 8..i * 8 + 8].copy_from_slice(&w.to_le_bytes());
        }
        b
    }

    /// Whether the signal is caught by a handler.
    pub fn is_handler(&self) -> bool {
        self.handler != SIG_DFL && self.handler != SIG_IGN
    }

    /// Whether delivering `sig` with this action does nothing.
    pub fn ignores(&self, sig: u32) -> bool {
        self.handler == SIG_IGN
            || (self.handler == SIG_DFL && default_action(sig) == DefaultAction::Ignore)
    }
}

/// What a `siginfo_t` says about one signal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Info {
    /// The signal.
    pub signo: u32,
    /// `si_code`.
    pub code: i32,
    /// Sender's pid (kill, SIGCHLD).
    pub pid: u32,
    /// Child's exit status or signal (SIGCHLD).
    pub status: i32,
    /// Faulting address (SIGSEGV, SIGBUS, SIGILL, SIGFPE).
    pub addr: u64,
}

impl Info {
    /// The 128-byte `siginfo_t` (x86-64 layout: signo, errno, code, then
    /// the union at offset 16).
    pub fn to_bytes(&self) -> [u8; 128] {
        let mut b = [0u8; 128];
        b[0..4].copy_from_slice(&self.signo.to_le_bytes());
        b[8..12].copy_from_slice(&self.code.to_le_bytes());
        if self.addr != 0 {
            b[16..24].copy_from_slice(&self.addr.to_le_bytes());
        } else {
            b[16..20].copy_from_slice(&self.pid.to_le_bytes());
            // si_uid 0 at 20; si_status at 24 (SIGCHLD).
            b[24..28].copy_from_slice(&self.status.to_le_bytes());
        }
        b
    }
}

/// Offset of the `ucontext` in the frame.
pub const UC: usize = 8;
/// Offset of `uc_mcontext`.
pub const MCONTEXT: usize = UC + 40;
/// Offset of `uc_sigmask`.
pub const SIGMASK: usize = UC + 296;
/// Offset of the `siginfo`.
pub const INFO: usize = UC + 304;
/// Size of `struct rt_sigframe`.
pub const FRAME_SIZE: usize = INFO + 128;
/// Size of the FXSAVE image after it.
pub const FPSTATE_SIZE: usize = 512;
/// The red zone below the interrupted stack pointer.
pub const RED_ZONE: u64 = 128;

/// A signal frame ready to be copied to the user stack.
pub struct Placed {
    /// Where the frame starts: the handler's stack pointer.
    pub rsp: u64,
    /// The bytes from `rsp` up to the end of the FXSAVE image.
    pub bytes: alloc::vec::Vec<u8>,
}

/// The order of the general registers in `struct sigcontext`.
fn mcontext_words(r: &UserRegs) -> [u64; 18] {
    [
        r.r8, r.r9, r.r10, r.r11, r.r12, r.r13, r.r14, r.r15, r.rdi, r.rsi, r.rbp, r.rbx, r.rdx,
        r.rax, r.rcx, r.rsp, r.rip, r.rflags,
    ]
}

/// Fault details saved in the frame's mcontext.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Trap {
    /// Exception vector (`trapno`).
    pub trapno: u64,
    /// Error code (`err`).
    pub err: u64,
    /// CR2 (`cr2`).
    pub cr2: u64,
}

/// Builds the frame for a handler of `info.signo` interrupting `regs`:
/// `mask` is the mask to restore on return, `fpu` the FXSAVE image,
/// `restorer` the return address. `stack_top` is where to build it (the
/// interrupted RSP, or the top of an alternate stack).
pub fn build(
    regs: &UserRegs,
    stack_top: u64,
    info: &Info,
    mask: u64,
    fpu: &[u8; 512],
    restorer: u64,
    trap: Trap,
) -> Placed {
    let below = stack_top.wrapping_sub(RED_ZONE);
    let fpstate = below.wrapping_sub(FPSTATE_SIZE as u64) & !63;
    let rsp = (fpstate.wrapping_sub(FRAME_SIZE as u64) & !15).wrapping_sub(8);
    let len = (fpstate - rsp) as usize + FPSTATE_SIZE;
    let mut b = alloc::vec![0u8; len];
    b[0..8].copy_from_slice(&restorer.to_le_bytes());
    // uc_flags 0, uc_link 0, uc_stack: ss_flags SS_DISABLE (2).
    b[UC + 24..UC + 28].copy_from_slice(&2u32.to_le_bytes());
    for (i, w) in mcontext_words(regs).iter().enumerate() {
        let at = MCONTEXT + i * 8;
        b[at..at + 8].copy_from_slice(&w.to_le_bytes());
    }
    // cs = 0x33 as Linux reports it (user code), ss = 0x2b; STELA's
    // selectors differ and are restored by the kernel, not from here.
    b[MCONTEXT + 144..MCONTEXT + 146].copy_from_slice(&0x33u16.to_le_bytes());
    b[MCONTEXT + 150..MCONTEXT + 152].copy_from_slice(&0x2Bu16.to_le_bytes());
    let words = [trap.err, trap.trapno, mask, trap.cr2, fpstate];
    for (i, w) in words.iter().enumerate() {
        let at = MCONTEXT + 152 + i * 8;
        b[at..at + 8].copy_from_slice(&w.to_le_bytes());
    }
    b[SIGMASK..SIGMASK + 8].copy_from_slice(&mask.to_le_bytes());
    b[INFO..INFO + 128].copy_from_slice(&info.to_bytes());
    let fp = (fpstate - rsp) as usize;
    b[fp..fp + FPSTATE_SIZE].copy_from_slice(fpu);
    Placed { rsp, bytes: b }
}

/// What `rt_sigreturn` restores from a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Restored {
    /// The registers (to be sanitized by the caller).
    pub regs: UserRegs,
    /// The signal mask.
    pub mask: u64,
    /// The FXSAVE image's address (0: none).
    pub fpstate: u64,
}

/// Decodes the `FRAME_SIZE` bytes of a frame (starting at `pretcode`).
pub fn parse(b: &[u8]) -> Option<Restored> {
    if b.len() < FRAME_SIZE {
        return None;
    }
    let word = |at: usize| {
        let mut w = [0u8; 8];
        w.copy_from_slice(&b[at..at + 8]);
        u64::from_le_bytes(w)
    };
    let m = |i: usize| word(MCONTEXT + i * 8);
    let regs = UserRegs {
        r8: m(0),
        r9: m(1),
        r10: m(2),
        r11: m(3),
        r12: m(4),
        r13: m(5),
        r14: m(6),
        r15: m(7),
        rdi: m(8),
        rsi: m(9),
        rbp: m(10),
        rbx: m(11),
        rdx: m(12),
        rax: m(13),
        rcx: m(14),
        rsp: m(15),
        rip: m(16),
        rflags: m(17),
    };
    Some(Restored {
        regs,
        mask: word(SIGMASK),
        fpstate: word(MCONTEXT + 152 + 4 * 8),
    })
}

/// The lowest signal in `set`, if any.
pub fn lowest(set: u64) -> Option<u32> {
    (set != 0).then(|| set.trailing_zeros() + 1)
}

/// A `wait4` status word: `code << 8` for an exit, the signal for a kill.
pub fn wait_status(code: i64, signal: Option<u32>) -> u32 {
    match signal {
        Some(s) => s & 0x7F,
        None => ((code as u32) & 0xFF) << 8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_linux() {
        assert_eq!(MCONTEXT, 48);
        assert_eq!(SIGMASK, 304);
        assert_eq!(INFO, 312);
        assert_eq!(FRAME_SIZE, 440);
        // musl's REG_RIP is gregs[16], gregs at uc + 40.
        assert_eq!(MCONTEXT + 16 * 8, UC + 40 + 128);
    }

    #[test]
    fn frame_round_trips_and_aligns() {
        let regs = UserRegs {
            rax: 1,
            rbx: 2,
            rcx: 3,
            rdx: 4,
            rsi: 5,
            rdi: 6,
            rbp: 7,
            rsp: 0x7FFF_FFEF_F123,
            r8: 8,
            r9: 9,
            r10: 10,
            r11: 11,
            r12: 12,
            r13: 13,
            r14: 14,
            r15: 15,
            rip: 0x40_1234,
            rflags: 0x246,
        };
        let info = Info {
            signo: sig::SIGUSR1,
            code: SI_USER,
            pid: 7,
            ..Info::default()
        };
        let mut fpu = [0u8; 512];
        fpu[24] = 0x80;
        let placed = build(
            &regs,
            regs.rsp,
            &info,
            bit(sig::SIGINT),
            &fpu,
            0x40_9000,
            Trap::default(),
        );
        // The handler starts as if called: rsp + 8 is 16-aligned.
        assert_eq!((placed.rsp + 8) % 16, 0);
        assert!(placed.rsp + (placed.bytes.len() as u64) <= regs.rsp - RED_ZONE);
        assert_eq!(&placed.bytes[0..8], &0x40_9000u64.to_le_bytes());
        let back = parse(&placed.bytes).unwrap();
        assert_eq!(back.regs, regs);
        assert_eq!(back.mask, bit(sig::SIGINT));
        assert_eq!(back.fpstate % 64, 0);
        let fp = (back.fpstate - placed.rsp) as usize;
        assert_eq!(placed.bytes[fp + 24], 0x80);
        assert_eq!(&placed.bytes[INFO..INFO + 4], &10u32.to_le_bytes());
        assert_eq!(&placed.bytes[INFO + 16..INFO + 20], &7u32.to_le_bytes());
    }

    #[test]
    fn actions_and_defaults() {
        let a = Action {
            handler: 0x40_1000,
            flags: SA_RESTART | SA_RESTORER,
            restorer: 0x40_2000,
            mask: bit(sig::SIGTERM),
        };
        assert_eq!(Action::from_bytes(&a.to_bytes()), a);
        assert!(a.is_handler());
        assert!(!Action::default().is_handler());
        assert!(Action::default().ignores(sig::SIGCHLD));
        assert!(!Action::default().ignores(sig::SIGINT));
        assert!(
            Action {
                handler: SIG_IGN,
                ..Action::default()
            }
            .ignores(sig::SIGINT)
        );
        assert_eq!(default_action(sig::SIGWINCH), DefaultAction::Ignore);
        assert_eq!(default_action(sig::SIGPIPE), DefaultAction::Terminate);
        assert_eq!(default_action(40), DefaultAction::Terminate);
        assert!(valid(64) && !valid(0) && !valid(65));
    }

    #[test]
    fn helpers() {
        assert_eq!(lowest(0), None);
        assert_eq!(lowest(bit(5) | bit(9)), Some(5));
        assert_eq!(wait_status(3, None), 0x300);
        assert_eq!(wait_status(0, Some(sig::SIGINT)), 2);
        assert_eq!(UNBLOCKABLE, (1 << 8) | (1 << 18));
        let i = Info {
            signo: sig::SIGSEGV,
            code: SEGV_MAPERR,
            addr: 0x1000,
            ..Info::default()
        };
        assert_eq!(&i.to_bytes()[16..24], &0x1000u64.to_le_bytes());
    }
}
