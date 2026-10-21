// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The hardware report (P11, `docs/hwinfo.md`): what STELA found, as
//! text, for `SysInfo(HARDWARE)`, the `hwinfo` program and Mouseion's
//! About STELA. The first lines are a summary with fixed prefixes
//! (`cpu:`, `processors:`, `memory:`, `display:`), then one section per
//! kind of hardware:
//!
//! ```text
//!  STELA 0.1.0
//!  cpu: QEMU Virtual CPU version 2.5+ (GenuineIntel, family 6 model 6 stepping 3)
//!  ...
//!  [memory map]   [acpi]   [pci]   [usb]   [disks]   [volumes]
//!  [network]   [audio]
//! ```
//!
//! The formatting helpers are pure (host tests); [`report`] collects the
//! data on the target.

use alloc::format;
use alloc::string::String;

/// `bytes` in the largest unit that keeps a whole number in front:
/// `512 B`, `64 KiB`, `1.5 MiB` (one decimal below 10), `128 MiB`.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut unit = 0;
    let mut scaled = bytes;
    let mut tenths = 0;
    while scaled >= 1024 && unit + 1 < UNITS.len() {
        tenths = (scaled % 1024) * 10 / 1024;
        scaled /= 1024;
        unit += 1;
    }
    if unit > 0 && scaled < 10 && tenths > 0 {
        format!("{scaled}.{tenths} {}", UNITS[unit])
    } else {
        format!("{scaled} {}", UNITS[unit])
    }
}

/// The name of a `bootinfo::MemoryKind` value.
pub fn region_kind(kind: u32) -> &'static str {
    match kind {
        1 => "usable",
        2 => "kernel and loader",
        3 => "reserved",
        4 => "ACPI reclaimable",
        5 => "ACPI NVS",
        6 => "MMIO",
        _ => "unknown",
    }
}

/// The name of an `abi::volume_kind` value.
pub fn volume_kind(kind: u32) -> &'static str {
    match kind {
        abi::volume_kind::EHFS => "eHFS",
        abi::volume_kind::FAT => "FAT",
        abi::volume_kind::INITRD => "initrd",
        abi::volume_kind::TMP => "memory",
        _ => "?",
    }
}

/// `a.b.c.d`.
pub fn ipv4(a: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", a[0], a[1], a[2], a[3])
}

/// The prefix length of a subnet mask.
pub fn prefix(mask: [u8; 4]) -> u32 {
    u32::from_be_bytes(mask).leading_ones()
}

/// `52:54:00:12:34:56`.
pub fn mac(m: [u8; 6]) -> String {
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        m[0], m[1], m[2], m[3], m[4], m[5]
    )
}

/// An ACPI signature as text (`?` for bytes that are not printable).
pub fn signature(s: [u8; 4]) -> String {
    s.iter()
        .map(|&b| if b.is_ascii_graphic() { b as char } else { '?' })
        .collect()
}

/// The network line: driver, MAC, link, address and counters.
pub fn network_line(info: &abi::net::NetInfo) -> String {
    let address = if info.configured {
        format!(
            "{}/{} gateway {} dns {}",
            ipv4(info.ip),
            prefix(info.mask),
            ipv4(info.gateway),
            ipv4(info.dns)
        )
    } else {
        String::from("no address")
    };
    format!(
        "{} {} link {}, {}; received {} frames ({}), sent {} frames ({})",
        info.driver_name(),
        mac(info.mac),
        if info.link { "up" } else { "down" },
        address,
        info.rx_packets,
        size(info.rx_bytes),
        info.tx_packets,
        size(info.tx_bytes)
    )
}

#[cfg(target_os = "none")]
pub use target::report;

#[cfg(target_os = "none")]
mod target {
    use super::*;
    use core::fmt::Write;

    /// The whole report.
    pub fn report() -> String {
        let mut out = String::new();
        // Writing to a String does not fail.
        let _ = write_report(&mut out);
        out
    }

    fn write_report(o: &mut String) -> core::fmt::Result {
        writeln!(o, "STELA {}", env!("CARGO_PKG_VERSION"))?;
        let cpu = frame::cpuid::info();
        let brand = if cpu.brand.is_empty() {
            "(no brand string)"
        } else {
            &cpu.brand
        };
        writeln!(
            o,
            "cpu: {brand} ({}, family {} model {} stepping {})",
            cpu.vendor, cpu.family, cpu.model, cpu.stepping
        )?;
        writeln!(o, "cpu features: {}", cpu.features.join(" "))?;
        writeln!(o, "processors: {} online", frame::cpu::online())?;
        let (free, total) = frame::mm::pmm::stats();
        writeln!(
            o,
            "memory: {} usable, {} free",
            size(total as u64 * 4096),
            size(free as u64 * 4096)
        )?;
        let heap = frame::mm::heap::stats();
        writeln!(
            o,
            "kernel heap: {} of {} used",
            size(heap.used as u64),
            size(heap.size as u64)
        )?;
        let fb = frame::boot_framebuffer();
        if !fb.is_present() {
            writeln!(o, "display: none (text only)")?;
        } else {
            writeln!(
                o,
                "display: {}x{}, pitch {} bytes, {}, {} at {:#x}",
                fb.width,
                fb.height,
                fb.pitch,
                match fb.format() {
                    frame::bootinfo::PixelFormat::Bgr32 => "BGRX 8:8:8",
                    _ => "RGBX 8:8:8",
                },
                size(fb.size),
                fb.phys_addr
            )?;
        }
        let tsc = frame::time::tsc_hz();
        writeln!(
            o,
            "timer: APIC 1000 Hz; TSC {} MHz{}",
            tsc / 1_000_000,
            if frame::time::tsc_invariant() {
                ", invariant (the clock)"
            } else {
                ", not invariant"
            }
        )?;

        let regions = frame::memory_regions();
        writeln!(o, "\n[memory map] {} regions", regions.len())?;
        for r in regions {
            writeln!(
                o,
                "  {:#014x}  {:>10}  {}",
                r.phys_start,
                size(r.page_count * 4096),
                region_kind(r.kind)
            )?;
        }

        let tables = frame::acpi::table_list();
        writeln!(o, "\n[acpi] {} tables", tables.len())?;
        for (sig, len, phys) in &tables {
            writeln!(o, "  {} {len} bytes at {phys:#x}", signature(*sig))?;
        }

        let pci = drivers::pci::system::devices();
        writeln!(o, "\n[pci] {} functions", pci.len())?;
        for f in pci {
            writeln!(
                o,
                "  {} {:04x}:{:04x} class {:02x}.{:02x}.{:02x} {}",
                f.address,
                f.vendor,
                f.device,
                f.class,
                f.subclass,
                f.prog_if,
                f.kind()
            )?;
        }

        let usb = drivers::usb::xhci::devices();
        writeln!(o, "\n[usb] {} devices", usb.len())?;
        for d in &usb {
            writeln!(
                o,
                "  port {} {:04x}:{:04x} {} ({} speed, slot {})",
                d.path, d.ids.0, d.ids.1, d.kind, d.speed, d.slot
            )?;
        }

        let disks = crate::block::disks();
        writeln!(o, "\n[disks] {}", disks.len())?;
        for (name, disk) in &disks {
            writeln!(
                o,
                "  {name}: {} ({} blocks of 4 KiB){}",
                size(disk.block_count() * 4096),
                disk.block_count(),
                if disk.read_only() { ", read only" } else { "" }
            )?;
            match block::gpt::read(&**disk) {
                Ok(gpt) => {
                    for p in &gpt.partitions {
                        let ss = u64::from(gpt.sector_size);
                        let bytes = (p.last_lba + 1).saturating_sub(p.first_lba) * ss;
                        let kind = if p.kind == block::gpt::Guid::ESP {
                            String::from("EFI system")
                        } else {
                            format!("{}", p.kind)
                        };
                        writeln!(
                            o,
                            "    {} \"{}\" {} from sector {} ({kind})",
                            p.index + 1,
                            p.name,
                            size(bytes),
                            p.first_lba
                        )?;
                    }
                }
                Err(_) => writeln!(o, "    no GPT")?,
            }
        }

        let volumes = crate::vfs::volumes();
        writeln!(o, "\n[volumes] {}", volumes.len())?;
        for v in &volumes {
            let (total, free) = v.fs.space();
            writeln!(
                o,
                "  {} {}{} {} ({} free)",
                v.name,
                volume_kind(v.fs.kind()),
                if v.fs.read_only() { " ro" } else { "" },
                size(total),
                size(free)
            )?;
        }

        writeln!(o, "\n[network]")?;
        match crate::net::info() {
            Some(info) => writeln!(o, "  {}", network_line(&info))?,
            None => writeln!(o, "  no card (or nonet)")?,
        }

        writeln!(o, "\n[audio]")?;
        match crate::audio::description() {
            Some(d) => writeln!(o, "  hda {d}")?,
            None => writeln!(o, "  no HDA controller")?,
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(512), "512 B");
        assert_eq!(size(64 * 1024), "64 KiB");
        assert_eq!(size(1536 * 1024), "1.5 MiB");
        assert_eq!(size(128 << 20), "128 MiB");
        assert_eq!(size(502 << 20), "502 MiB");
        assert_eq!(size(3 << 30), "3 GiB");
        assert_eq!(size(u64::MAX), "16777215 TiB");
    }

    #[test]
    fn names() {
        assert_eq!(region_kind(1), "usable");
        assert_eq!(region_kind(9), "unknown");
        assert_eq!(volume_kind(abi::volume_kind::EHFS), "eHFS");
        assert_eq!(signature(*b"FACP"), "FACP");
        assert_eq!(signature([b'A', 0, b'C', 0xFF]), "A?C?");
        assert_eq!(prefix([255, 255, 255, 0]), 24);
        assert_eq!(prefix([0; 4]), 0);
        assert_eq!(mac([0x52, 0x54, 0, 0x12, 0x34, 0x56]), "52:54:00:12:34:56");
    }

    #[test]
    fn network() {
        let mut info = abi::net::NetInfo {
            driver: *b"e1000e\0\0\0\0\0\0\0\0\0\0",
            mac: [0x52, 0x54, 0, 0x12, 0x34, 0x56],
            link: true,
            configured: true,
            ip: [10, 0, 2, 15],
            mask: [255, 255, 255, 0],
            gateway: [10, 0, 2, 2],
            dns: [10, 0, 2, 3],
            rx_packets: 3,
            rx_bytes: 2048,
            tx_packets: 4,
            tx_bytes: 300,
        };
        assert_eq!(
            network_line(&info),
            "e1000e 52:54:00:12:34:56 link up, 10.0.2.15/24 gateway 10.0.2.2 dns 10.0.2.3; \
             received 3 frames (2 KiB), sent 4 frames (300 B)"
        );
        info.configured = false;
        info.link = false;
        assert!(network_line(&info).contains("link down, no address"));
    }
}
