// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Time: a 1000 Hz tick from the local APIC timer and a monotonic clock.
//!
//! ```text
//!  boot: PIT channel 2 counts 10 ms ──► how far did the APIC timer and
//!        (3 times, median)               the TSC get in that window?
//!                                             │
//!            ┌────────────────────────────────┴──────────────────┐
//!            ▼                                                   ▼
//!  APIC timer, periodic, /16,                     TSC Hz (CPUID 15H/16H if
//!  initial count = Hz / 1000                      reported, else calibrated)
//!            │ vector 48, every 1 ms                             │
//!            ▼                                                   ▼
//!  ticks() += 1 (global and per CPU)          now_ns() = TSC since boot / Hz
//!                                             (only with an invariant TSC;
//!                                              otherwise ticks() * 1 ms)
//! ```
//!
//! An invariant TSC (CPUID.80000007H:EDX[8]) runs at a constant rate in
//! every power state, so it is a fine-grained clock; without it the TSC may
//! change speed and only the tick count is trusted.

pub mod calc;

#[cfg(target_os = "none")]
pub use target::{now_ns, set_tick_hook, sleep_ms, ticks, tsc_hz, tsc_invariant};

#[cfg(target_os = "none")]
pub(crate) use target::init;

/// Timer interrupts per second.
pub const TICK_HZ: u64 = 1000;

#[cfg(target_os = "none")]
mod target {
    use super::TICK_HZ;
    use super::calc::{self, Clock};
    use crate::arch::x86_64::{apic, pit};
    use crate::irq;
    use crate::sync::Once;
    use crate::trap::TrapFrame;
    use core::arch::asm;
    use core::arch::x86_64::{__cpuid, __cpuid_count};
    use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    /// PIT clocks in the 10 ms calibration window.
    const WINDOW: u16 = (pit::PIT_HZ / 100) as u16;
    /// APIC timer divide configuration 0b0011: divide by 16 (Intel SDM
    /// Vol. 3A "Divide Configuration Register").
    const DIVIDE_BY_16: u32 = 0b0011;
    /// LVT timer bits 17-18 = 01: periodic mode.
    const PERIODIC: u32 = 1 << 17;

    static TICKS: AtomicU64 = AtomicU64::new(0);
    /// The TSC clock, when the TSC is invariant.
    static TSC_CLOCK: Once<Clock> = Once::new();
    static TSC_HZ: AtomicU64 = AtomicU64::new(0);
    static TSC_INVARIANT: AtomicBool = AtomicBool::new(false);

    fn rdtsc() -> u64 {
        let (low, high): (u32, u32);
        // SAFETY: `rdtsc` only reads the time-stamp counter (Vol. 2B
        // "RDTSC"); CR4.TSD is clear, so it is allowed.
        unsafe {
            asm!("rdtsc", out("eax") low, out("edx") high, options(nomem, nostack, preserves_flags))
        };
        (u64::from(high) << 32) | u64::from(low)
    }

    /// CPUID.80000007H:EDX[8]: invariant TSC.
    fn invariant_tsc() -> bool {
        __cpuid(0x8000_0000).eax >= 0x8000_0007 && __cpuid(0x8000_0007).edx & (1 << 8) != 0
    }

    /// The TSC frequency reported by CPUID leaves 15H/16H, if any.
    fn cpuid_tsc_hz() -> Option<u64> {
        let max = __cpuid(0).eax;
        let leaf15 = (max >= 0x15).then(|| {
            let r = __cpuid_count(0x15, 0);
            (r.eax, r.ebx, r.ecx)
        });
        let leaf16 = (max >= 0x16).then(|| __cpuid_count(0x16, 0).eax & 0xFFFF);
        calc::tsc_hz_from_cpuid(leaf15, leaf16)
    }

    /// One calibration run: (APIC timer counts, TSC counts) in the window.
    fn calibrate_once() -> (u64, u64) {
        apic::write(apic::LVT_TIMER, apic::LVT_MASKED);
        apic::write(apic::TIMER_DIVIDE, DIVIDE_BY_16);
        apic::write(apic::TIMER_INITIAL, u32::MAX);
        let Some((start, end)) =
            pit::measure(WINDOW, || (apic::read(apic::TIMER_CURRENT), rdtsc()))
        else {
            panic!("time: the PIT does not count; cannot calibrate the timers");
        };
        apic::write(apic::TIMER_INITIAL, 0);
        (
            u64::from(start.0.wrapping_sub(end.0)),
            end.1.wrapping_sub(start.1),
        )
    }

    /// The kernel's per-tick function (the scheduler's clock).
    static TICK_HOOK: Once<fn()> = Once::new();

    /// Registers a function called on every timer tick, inside the
    /// interrupt (interrupts disabled; it must be short). Only the first
    /// registration counts.
    pub fn set_tick_hook(hook: fn()) {
        let _ = TICK_HOOK.set(hook);
    }

    fn timer_interrupt(_: &TrapFrame) {
        TICKS.fetch_add(1, Ordering::Relaxed);
        crate::cpu::current().tick();
        if let Some(hook) = TICK_HOOK.get() {
            hook();
        }
    }

    /// Calibrates the APIC timer and the TSC against the PIT, then starts
    /// the 1000 Hz tick. Called by `frame::init` with interrupts disabled.
    pub(crate) fn init() {
        let runs = [calibrate_once(), calibrate_once(), calibrate_once()];
        let apic_counts = calc::median3(runs.map(|r| r.0));
        let tsc_counts = calc::median3(runs.map(|r| r.1));
        let apic_hz = calc::frequency(apic_counts, u64::from(WINDOW), pit::PIT_HZ);
        let initial = apic_hz / TICK_HZ;
        assert!(
            initial > 0 && initial <= u64::from(u32::MAX),
            "time: APIC timer calibration failed ({apic_counts} counts in 10 ms)"
        );
        let invariant = invariant_tsc();
        let (tsc_hz, source) = match cpuid_tsc_hz() {
            Some(hz) => (hz, "CPUID"),
            None => (
                calc::frequency(tsc_counts, u64::from(WINDOW), pit::PIT_HZ),
                "PIT",
            ),
        };
        TSC_HZ.store(tsc_hz, Ordering::Relaxed);
        TSC_INVARIANT.store(invariant, Ordering::Relaxed);
        if invariant && tsc_hz > 0 {
            let _ = TSC_CLOCK.set(Clock {
                base: rdtsc(),
                hz: tsc_hz,
            });
        }
        if let Err(e) = irq::register(irq::TIMER_VECTOR, timer_interrupt) {
            panic!("time: cannot register the timer vector: {e:?}");
        }
        apic::write(apic::TIMER_DIVIDE, DIVIDE_BY_16);
        apic::write(apic::LVT_TIMER, PERIODIC | u32::from(irq::TIMER_VECTOR));
        apic::write(apic::TIMER_INITIAL, initial as u32);
        crate::kprintln!(
            "time: APIC timer {}.{:03} MHz (/16), {} Hz; TSC {} MHz ({}, {})",
            apic_hz / 1_000_000,
            apic_hz / 1_000 % 1_000,
            TICK_HZ,
            tsc_hz / 1_000_000,
            source,
            if invariant {
                "invariant"
            } else {
                "not invariant"
            }
        );
    }

    /// Timer ticks since the timer started (1 ms each).
    pub fn ticks() -> u64 {
        TICKS.load(Ordering::Relaxed)
    }

    /// Nanoseconds since the clock started: from the TSC when it is
    /// invariant, else from the tick count (1 ms resolution). Never goes
    /// backwards.
    pub fn now_ns() -> u64 {
        match TSC_CLOCK.get() {
            Some(clock) => clock.ns(rdtsc()),
            None => ticks() * (1_000_000_000 / TICK_HZ),
        }
    }

    /// The TSC frequency in Hz (from CPUID or the calibration).
    pub fn tsc_hz() -> u64 {
        TSC_HZ.load(Ordering::Relaxed)
    }

    /// Whether the TSC is invariant, so that [`now_ns`] uses it.
    pub fn tsc_invariant() -> bool {
        TSC_INVARIANT.load(Ordering::Relaxed)
    }

    /// Waits at least `ms` milliseconds, halting between timer ticks.
    /// Interrupts must be enabled (they are after `frame::init`).
    pub fn sleep_ms(ms: u64) {
        assert!(
            crate::sync::interrupts_enabled(),
            "time: sleep_ms with interrupts disabled would never wake up"
        );
        // +1: the current tick may be about to end.
        let target = ticks() + ms + 1;
        while ticks() < target {
            crate::halt_until_interrupt();
        }
    }
}
