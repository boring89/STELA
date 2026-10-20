// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The boot information block: everything the STELA loader learned from the
//! firmware, in a fixed `#[repr(C)]` layout shared by the loader (`boot/`)
//! and the kernel.
//!
//! The loader fills one [`BootInfo`] in memory that stays in use after
//! `ExitBootServices` and passes the kernel its address in the higher-half
//! direct map (HHDM). Every address stored *inside* the block is physical;
//! the kernel adds [`BootInfo::hhdm_base`] to reach it.
//!
//! The structure is versioned: the kernel refuses a block whose
//! [`BootInfo::magic`] or [`BootInfo::version`] it does not recognise.

#![no_std]
#![forbid(unsafe_code)]

/// `"STELABI\0"` read as a little-endian `u64`.
pub const MAGIC: u64 = u64::from_le_bytes(*b"STELABI\0");

/// Layout version; bump on every incompatible change to any structure here.
pub const VERSION: u32 = 3;

/// Capacity of [`BootInfo::regions`]. The loader merges adjacent regions of
/// the same kind, so firmware memory maps of a few hundred entries fit.
pub const MAX_REGIONS: usize = 256;

/// Capacity of [`BootInfo::cmdline`] in bytes.
pub const CMDLINE_SIZE: usize = 256;

/// Size of a page frame, the unit of [`MemoryRegion::page_count`].
pub const PAGE_SIZE: u64 = 4096;

/// Virtual address the kernel image is linked at: the last 2 GiB.
pub const KERNEL_BASE: u64 = 0xFFFF_FFFF_8000_0000;

/// Virtual address of physical address 0 in the higher-half direct map.
pub const HHDM_BASE: u64 = 0xFFFF_8000_0000_0000;

/// What a range of physical memory is used for, as seen by the kernel.
///
/// The UEFI memory types are folded into these (UEFI Specification 2.10,
/// 7.2.1 "EFI_BOOT_SERVICES.AllocatePages()", Table 7-6 "Memory Type Usage
/// after ExitBootServices()").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum MemoryKind {
    /// Free RAM the kernel may allocate (UEFI conventional memory and
    /// boot-services code/data).
    Usable = 1,
    /// RAM holding the kernel image, the boot information, the loader's page
    /// tables and the initial stack (UEFI loader code/data). In use.
    KernelAndBoot = 2,
    /// Never to be touched (firmware runtime services, reserved, unusable).
    Reserved = 3,
    /// ACPI tables; usable once the kernel has parsed them.
    AcpiReclaimable = 4,
    /// ACPI non-volatile storage; must be preserved.
    AcpiNvs = 5,
    /// Memory-mapped I/O reported by the firmware.
    Mmio = 6,
}

impl MemoryKind {
    /// Decodes a raw [`MemoryRegion::kind`] value.
    pub const fn from_raw(raw: u32) -> Option<MemoryKind> {
        match raw {
            1 => Some(MemoryKind::Usable),
            2 => Some(MemoryKind::KernelAndBoot),
            3 => Some(MemoryKind::Reserved),
            4 => Some(MemoryKind::AcpiReclaimable),
            5 => Some(MemoryKind::AcpiNvs),
            6 => Some(MemoryKind::Mmio),
            _ => None,
        }
    }
}

/// One contiguous range of physical memory of a single kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct MemoryRegion {
    /// A [`MemoryKind`] as `u32`; stored raw so that a bad value cannot
    /// create an invalid enum. Read it with [`MemoryRegion::kind`].
    pub kind: u32,
    /// Always zero; keeps the layout explicit.
    pub reserved: u32,
    /// Physical start address, page aligned.
    pub phys_start: u64,
    /// Length in 4 KiB pages.
    pub page_count: u64,
}

impl MemoryRegion {
    /// An all-zero entry (kind 0 is not a valid [`MemoryKind`]).
    pub const EMPTY: MemoryRegion = MemoryRegion {
        kind: 0,
        reserved: 0,
        phys_start: 0,
        page_count: 0,
    };

    /// Creates a region.
    pub const fn new(kind: MemoryKind, phys_start: u64, page_count: u64) -> MemoryRegion {
        MemoryRegion {
            kind: kind as u32,
            reserved: 0,
            phys_start,
            page_count,
        }
    }

    /// The decoded kind; `None` if the raw value is unknown.
    pub const fn kind(&self) -> Option<MemoryKind> {
        MemoryKind::from_raw(self.kind)
    }

    /// One past the last byte of the region.
    pub const fn phys_end(&self) -> u64 {
        self.phys_start + self.page_count * PAGE_SIZE
    }
}

/// Byte order of a 32-bit framebuffer pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum PixelFormat {
    /// No usable framebuffer (none found, or a format STELA cannot draw).
    None = 0,
    /// Byte 0 red, byte 1 green, byte 2 blue, byte 3 reserved
    /// (UEFI `PixelRedGreenBlueReserved8BitPerColor`).
    Rgb32 = 1,
    /// Byte 0 blue, byte 1 green, byte 2 red, byte 3 reserved
    /// (UEFI `PixelBlueGreenRedReserved8BitPerColor`).
    Bgr32 = 2,
}

impl PixelFormat {
    /// Decodes a raw [`Framebuffer::format`] value.
    pub const fn from_raw(raw: u32) -> PixelFormat {
        match raw {
            1 => PixelFormat::Rgb32,
            2 => PixelFormat::Bgr32,
            _ => PixelFormat::None,
        }
    }
}

/// The linear framebuffer left set up by the firmware's Graphics Output
/// Protocol (GOP).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Framebuffer {
    /// Physical address of pixel (0, 0); 0 if there is no framebuffer.
    pub phys_addr: u64,
    /// Size in bytes.
    pub size: u64,
    /// Visible width in pixels.
    pub width: u32,
    /// Visible height in pixels.
    pub height: u32,
    /// Bytes from the start of one row to the start of the next.
    pub pitch: u32,
    /// A [`PixelFormat`] as `u32`; read it with [`Framebuffer::format`].
    pub format: u32,
}

impl Framebuffer {
    /// No framebuffer.
    pub const NONE: Framebuffer = Framebuffer {
        phys_addr: 0,
        size: 0,
        width: 0,
        height: 0,
        pitch: 0,
        format: PixelFormat::None as u32,
    };

    /// The decoded pixel format.
    pub const fn format(&self) -> PixelFormat {
        PixelFormat::from_raw(self.format)
    }

    /// Whether there is a framebuffer the kernel can draw on.
    pub const fn is_present(&self) -> bool {
        self.phys_addr != 0 && !matches!(self.format(), PixelFormat::None)
    }
}

/// Everything the loader hands to the kernel.
#[derive(Clone, Debug)]
#[repr(C)]
pub struct BootInfo {
    /// Must be [`MAGIC`].
    pub magic: u64,
    /// Must be [`VERSION`].
    pub version: u32,
    /// Number of valid entries at the start of [`BootInfo::regions`].
    pub region_count: u32,
    /// Virtual address of physical 0 in the higher-half direct map.
    pub hhdm_base: u64,
    /// Bytes of physical address space covered by the direct map.
    pub hhdm_size: u64,
    /// Physical address of the top-level page table (the value in CR3).
    pub page_table_root: u64,
    /// Physical address of the kernel image (all `PT_LOAD` segments,
    /// contiguous).
    pub kernel_phys_base: u64,
    /// Size of the kernel image in bytes, page aligned.
    pub kernel_size: u64,
    /// Virtual address the kernel image starts at.
    pub kernel_virt_base: u64,
    /// Physical address of the initial kernel stack's lowest byte.
    pub stack_phys_base: u64,
    /// Size of the initial kernel stack in bytes.
    pub stack_size: u64,
    /// Physical address of the ACPI RSDP; 0 if the firmware has none.
    pub rsdp_phys: u64,
    /// Non-zero if the firmware memory map had more regions than fit; the
    /// kernel must then treat memory above the last region as unknown.
    pub regions_truncated: u32,
    /// Non-zero if the loader enabled no-execute page protection
    /// (`IA32_EFER.NXE`).
    pub nx_enabled: u32,
    /// The firmware framebuffer.
    pub framebuffer: Framebuffer,
    /// Physical address of the initial RAM disk (`\STELA\initrd.tar`, a
    /// ustar archive), page aligned; 0 if there is none.
    pub initrd_phys: u64,
    /// Size of the initial RAM disk in bytes; 0 if there is none.
    pub initrd_size: u64,
    /// Physical address of one page below 1 MiB that the loader reserved
    /// for the application processors' start-up code (a SIPI vector names
    /// a 4 KiB page below 1 MiB); 0 if none could be had.
    pub ap_trampoline_phys: u64,
    /// Number of valid bytes at the start of [`BootInfo::cmdline`].
    pub cmdline_len: u32,
    /// Always zero; keeps the layout explicit.
    pub reserved: u32,
    /// The boot options from `\STELA\boot.cfg`, normalised by
    /// [`parse_config`] into space-separated `key` or `key=value` words.
    pub cmdline: [u8; CMDLINE_SIZE],
    /// The physical memory map, sorted by address; only the first
    /// [`BootInfo::region_count`] entries are valid.
    pub regions: [MemoryRegion; MAX_REGIONS],
}

impl BootInfo {
    /// An empty block with the magic and version filled in.
    pub const fn new() -> BootInfo {
        BootInfo {
            magic: MAGIC,
            version: VERSION,
            region_count: 0,
            hhdm_base: HHDM_BASE,
            hhdm_size: 0,
            page_table_root: 0,
            kernel_phys_base: 0,
            kernel_size: 0,
            kernel_virt_base: KERNEL_BASE,
            stack_phys_base: 0,
            stack_size: 0,
            rsdp_phys: 0,
            regions_truncated: 0,
            nx_enabled: 0,
            framebuffer: Framebuffer::NONE,
            initrd_phys: 0,
            initrd_size: 0,
            ap_trampoline_phys: 0,
            cmdline_len: 0,
            reserved: 0,
            cmdline: [0; CMDLINE_SIZE],
            regions: [MemoryRegion::EMPTY; MAX_REGIONS],
        }
    }

    /// Whether the magic and version match this build.
    pub const fn is_valid(&self) -> bool {
        self.magic == MAGIC && self.version == VERSION && self.region_count as usize <= MAX_REGIONS
    }

    /// The valid part of the memory map.
    pub fn regions(&self) -> &[MemoryRegion] {
        let count = (self.region_count as usize).min(MAX_REGIONS);
        &self.regions[..count]
    }

    /// The valid part of the command line.
    pub fn cmdline(&self) -> &[u8] {
        let len = (self.cmdline_len as usize).min(CMDLINE_SIZE);
        &self.cmdline[..len]
    }

    /// Stores the boot options parsed from a `boot.cfg` file.
    pub fn set_config(&mut self, config: &[u8]) {
        self.cmdline_len = parse_config(config, &mut self.cmdline) as u32;
    }

    /// Appends a region, merging it into the previous one when it has the
    /// same kind and starts where that one ends. Returns `false` (and sets
    /// [`BootInfo::regions_truncated`]) if the table is full.
    pub fn push_region(&mut self, region: MemoryRegion) -> bool {
        if region.page_count == 0 {
            return true;
        }
        let count = self.region_count as usize;
        if let Some(last) = count.checked_sub(1).map(|i| &mut self.regions[i])
            && last.kind == region.kind
            && last.phys_end() == region.phys_start
        {
            last.page_count += region.page_count;
            return true;
        }
        if count >= MAX_REGIONS {
            self.regions_truncated = 1;
            return false;
        }
        self.regions[count] = region;
        self.region_count += 1;
        true
    }
}

/// Turns the text of a `boot.cfg` file into a command line: one option per
/// line (`key` or `key=value`), `#` starts a comment, blank space around
/// an option is dropped, and options are joined with single spaces into
/// `out`. Options with characters other than printable ASCII, and options
/// that no longer fit, are left out. Returns the number of bytes written.
pub fn parse_config(config: &[u8], out: &mut [u8]) -> usize {
    let mut len = 0;
    for line in config.split(|&b| b == b'\n') {
        let line = match line.iter().position(|&b| b == b'#') {
            Some(i) => &line[..i],
            None => line,
        };
        let line = line.trim_ascii();
        if line.is_empty() || !line.iter().all(|&b| b.is_ascii_graphic()) {
            continue;
        }
        let needed = line.len() + usize::from(len > 0);
        if len + needed > out.len() {
            continue;
        }
        if len > 0 {
            out[len] = b' ';
            len += 1;
        }
        out[len..len + line.len()].copy_from_slice(line);
        len += line.len();
    }
    len
}

impl Default for BootInfo {
    fn default() -> BootInfo {
        BootInfo::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, offset_of, size_of};

    #[test]
    fn layout_is_stable() {
        assert_eq!(size_of::<MemoryRegion>(), 24);
        assert_eq!(align_of::<MemoryRegion>(), 8);
        assert_eq!(size_of::<Framebuffer>(), 32);
        assert_eq!(offset_of!(BootInfo, magic), 0);
        assert_eq!(offset_of!(BootInfo, version), 8);
        assert_eq!(offset_of!(BootInfo, region_count), 12);
        assert_eq!(offset_of!(BootInfo, hhdm_base), 16);
        assert_eq!(offset_of!(BootInfo, framebuffer), 96);
        assert_eq!(offset_of!(BootInfo, initrd_phys), 128);
        assert_eq!(offset_of!(BootInfo, initrd_size), 136);
        assert_eq!(offset_of!(BootInfo, ap_trampoline_phys), 144);
        assert_eq!(offset_of!(BootInfo, cmdline_len), 152);
        assert_eq!(offset_of!(BootInfo, cmdline), 160);
        assert_eq!(offset_of!(BootInfo, regions), 416);
        assert_eq!(size_of::<BootInfo>(), 416 + 24 * MAX_REGIONS);
        assert_eq!(align_of::<BootInfo>(), 8);
    }

    #[test]
    fn magic_reads_as_text() {
        assert_eq!(&MAGIC.to_le_bytes(), b"STELABI\0");
    }

    #[test]
    fn enums_round_trip() {
        for kind in [
            MemoryKind::Usable,
            MemoryKind::KernelAndBoot,
            MemoryKind::Reserved,
            MemoryKind::AcpiReclaimable,
            MemoryKind::AcpiNvs,
            MemoryKind::Mmio,
        ] {
            assert_eq!(MemoryKind::from_raw(kind as u32), Some(kind));
        }
        assert_eq!(MemoryKind::from_raw(0), None);
        assert_eq!(MemoryKind::from_raw(7), None);
        assert_eq!(PixelFormat::from_raw(1), PixelFormat::Rgb32);
        assert_eq!(PixelFormat::from_raw(2), PixelFormat::Bgr32);
        assert_eq!(PixelFormat::from_raw(9), PixelFormat::None);
    }

    #[test]
    fn new_block_is_valid() {
        let info = BootInfo::new();
        assert!(info.is_valid());
        assert_eq!(info.version, 3);
        assert!(info.cmdline().is_empty());
        assert_eq!(info.ap_trampoline_phys, 0);
        assert_eq!((info.initrd_phys, info.initrd_size), (0, 0));
        assert!(info.regions().is_empty());
        assert!(!info.framebuffer.is_present());
    }

    #[test]
    fn push_region_merges_neighbours() {
        let mut info = BootInfo::new();
        assert!(info.push_region(MemoryRegion::new(MemoryKind::Usable, 0x1000, 2)));
        assert!(info.push_region(MemoryRegion::new(MemoryKind::Usable, 0x3000, 1)));
        // Gap: not merged.
        assert!(info.push_region(MemoryRegion::new(MemoryKind::Usable, 0x8000, 1)));
        // Different kind: not merged.
        assert!(info.push_region(MemoryRegion::new(MemoryKind::Reserved, 0x9000, 1)));
        // Empty: ignored.
        assert!(info.push_region(MemoryRegion::new(MemoryKind::Reserved, 0xA000, 0)));
        assert_eq!(
            info.regions(),
            &[
                MemoryRegion::new(MemoryKind::Usable, 0x1000, 3),
                MemoryRegion::new(MemoryKind::Usable, 0x8000, 1),
                MemoryRegion::new(MemoryKind::Reserved, 0x9000, 1),
            ]
        );
    }

    #[test]
    fn push_region_reports_overflow() {
        let mut info = BootInfo::new();
        for i in 0..MAX_REGIONS as u64 {
            let kind = if i % 2 == 0 {
                MemoryKind::Usable
            } else {
                MemoryKind::Reserved
            };
            assert!(info.push_region(MemoryRegion::new(kind, i * PAGE_SIZE, 1)));
        }
        assert_eq!(info.regions_truncated, 0);
        assert!(!info.push_region(MemoryRegion::new(MemoryKind::Mmio, 1 << 40, 1)));
        assert_eq!(info.regions_truncated, 1);
        assert_eq!(info.regions().len(), MAX_REGIONS);
    }

    #[test]
    fn config_becomes_a_command_line() {
        let mut info = BootInfo::new();
        info.set_config(
            b"# STELA boot options\r\ntextmode\n\n  nousb  # no xHCI\nclassic=off\nbad\x01word\n",
        );
        assert_eq!(info.cmdline(), b"textmode nousb classic=off");
    }

    #[test]
    fn config_overflow_drops_options() {
        let mut out = [0u8; 8];
        assert_eq!(parse_config(b"abc\nlonger-than-eight\nde", &mut out), 6);
        assert_eq!(&out[..6], b"abc de");
        assert_eq!(parse_config(b"", &mut out), 0);
    }
}
