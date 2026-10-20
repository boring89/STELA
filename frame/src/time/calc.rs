// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Arithmetic for the clocks: turning a calibration into frequencies and
//! counter values into nanoseconds. Pure logic, tested on the host.

/// The median of three measurements (one disturbed run cannot move it).
pub fn median3(mut values: [u64; 3]) -> u64 {
    values.sort_unstable();
    values[1]
}

/// The frequency of a counter that advanced `counted` while a reference
/// clock of `reference_hz` advanced `reference_counted`.
pub fn frequency(counted: u64, reference_counted: u64, reference_hz: u64) -> u64 {
    if reference_counted == 0 {
        return 0;
    }
    (u128::from(counted) * u128::from(reference_hz) / u128::from(reference_counted)) as u64
}

/// The TSC frequency from CPUID leaves 15H and 16H (Intel SDM Vol. 2A
/// "CPUID", "Time Stamp Counter and Nominal Core Crystal Clock Information
/// Leaf" and "Processor Frequency Information Leaf"):
///
/// - 15H: TSC Hz = ECX (crystal Hz) * EBX / EAX, when all three are set;
/// - otherwise 16H EAX: the base frequency in MHz, which is the TSC rate on
///   processors with an invariant TSC.
///
/// `leaf15` is `(eax, ebx, ecx)`; absent leaves are `None`.
pub fn tsc_hz_from_cpuid(leaf15: Option<(u32, u32, u32)>, leaf16_mhz: Option<u32>) -> Option<u64> {
    if let Some((denominator, numerator, crystal)) = leaf15
        && denominator != 0
        && numerator != 0
        && crystal != 0
    {
        return Some(u64::from(crystal) * u64::from(numerator) / u64::from(denominator));
    }
    leaf16_mhz
        .filter(|&mhz| mhz != 0)
        .map(|mhz| u64::from(mhz) * 1_000_000)
}

/// A clock that counts at `hz` from `base`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    /// Counter value at time 0.
    pub base: u64,
    /// Counts per second.
    pub hz: u64,
}

impl Clock {
    /// Nanoseconds since `base` at counter value `now` (u128 inside, so
    /// no overflow for centuries at GHz rates).
    pub fn ns(&self, now: u64) -> u64 {
        if self.hz == 0 {
            return 0;
        }
        (u128::from(now.wrapping_sub(self.base)) * 1_000_000_000 / u128::from(self.hz)) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_of_three() {
        assert_eq!(median3([5, 1, 3]), 3);
        assert_eq!(median3([7, 7, 1]), 7);
        assert_eq!(median3([u64::MAX, 0, 2]), 2);
    }

    #[test]
    fn frequency_from_a_pit_window() {
        // 10 ms of PIT (11932 clocks) while the APIC timer, divided by 16,
        // counted 625,000: 62.5 MHz (a 1 GHz bus).
        let hz = frequency(625_000, 11_932, 1_193_182);
        assert!((62_490_000..=62_510_000).contains(&hz), "{hz}");
        assert_eq!(frequency(1, 0, 1_193_182), 0);
        // No overflow with a 5 GHz TSC over a long window.
        assert_eq!(frequency(5_000_000_000 * 60, 60, 1), 5_000_000_000);
    }

    #[test]
    fn cpuid_leaves() {
        // Skylake-style: crystal not reported, 16H gives 2.4 GHz.
        assert_eq!(
            tsc_hz_from_cpuid(Some((2, 200, 0)), Some(2400)),
            Some(2_400_000_000)
        );
        // Full 15H: 24 MHz crystal * 125 / 1 = 3 GHz.
        assert_eq!(
            tsc_hz_from_cpuid(Some((1, 125, 24_000_000)), Some(2900)),
            Some(3_000_000_000)
        );
        assert_eq!(tsc_hz_from_cpuid(None, None), None);
        assert_eq!(tsc_hz_from_cpuid(Some((0, 0, 0)), Some(0)), None);
    }

    #[test]
    fn clock_to_nanoseconds() {
        let clock = Clock {
            base: 1_000,
            hz: 2_000_000_000,
        };
        assert_eq!(clock.ns(1_000), 0);
        assert_eq!(clock.ns(3_000), 1_000);
        assert_eq!(clock.ns(1_000 + 2_000_000_000 * 3600), 3600 * 1_000_000_000);
        assert_eq!(Clock { base: 0, hz: 0 }.ns(5), 0);
    }
}
