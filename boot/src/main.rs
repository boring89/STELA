// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! STELA's UEFI boot loader.
//!
//! 1. Read `\STELA\kernel.elf` from the partition the loader was started from.
//! 2. Copy its `PT_LOAD` segments into one block of fresh pages.
//!    Read `\STELA\initrd.tar` (if present) into fresh pages as well.
//! 3. Record the GOP framebuffer and the ACPI RSDP.
//! 4. Build the kernel's page tables: the higher-half direct map (HHDM),
//!    the kernel at its link address, and an identity map of physical memory
//!    so the loader survives the switch of CR3.
//! 5. Exit boot services, convert the final memory map into the boot
//!    information block, switch to the new page tables and stack, and jump to
//!    the kernel's entry point.
//!
//! The only `unsafe` code is the call to `exit_boot_services` and the final
//! jump (see `docs/boot.md`).

#![no_std]
#![no_main]
#![deny(unsafe_code)]

extern crate alloc;

mod paging;

use alloc::boxed::Box;
use alloc::vec;
use bootinfo::{BootInfo, Framebuffer, MemoryKind, MemoryRegion, PixelFormat};
use elfload::Elf;
use paging::{NO_EXECUTE, PAGE_2M, PAGE_4K, PageTables, WRITABLE};
use uefi::boot::MemoryType;
use uefi::mem::memory_map::{MemoryMap, MemoryMapMut};
use uefi::prelude::*;
use uefi::proto::console::gop::{self, GraphicsOutput};
use uefi::table::cfg::ConfigTableEntry;
use uefi::{CStr16, cstr16, println};

/// Where the kernel lives on the boot partition.
const KERNEL_PATH: &CStr16 = cstr16!("\\STELA\\kernel.elf");
/// The initial RAM disk: the first user programs, as a ustar archive.
const INITRD_PATH: &CStr16 = cstr16!("\\STELA\\initrd.tar");
/// Boot options, one per line (`docs/p520.md`); optional.
const CONFIG_PATH: &CStr16 = cstr16!("\\STELA\\boot.cfg");

/// Size of the kernel's initial stack.
const STACK_SIZE: usize = 64 * 1024;

/// The direct map always covers at least the first 4 GiB, where the firmware
/// places MMIO such as the framebuffer even when the memory map omits it.
const MIN_HHDM_SIZE: u64 = 4 << 30;

/// One page, the unit the loader allocates in so that allocations are page
/// aligned and come straight from `AllocatePages` (type `LOADER_DATA`).
#[derive(Clone)]
#[repr(C, align(4096))]
struct Page([u8; 4096]);

/// Allocates `count` zeroed pages that are never freed.
fn alloc_pages(count: usize) -> &'static mut [Page] {
    vec![Page([0; 4096]); count].leak()
}

/// Copies `data` into `pages` starting at byte `offset`.
fn write_bytes(pages: &mut [Page], mut offset: usize, mut data: &[u8]) {
    while !data.is_empty() {
        let page = &mut pages[offset / 4096].0;
        let start = offset % 4096;
        let n = (4096 - start).min(data.len());
        page[start..start + n].copy_from_slice(&data[..n]);
        offset += n;
        data = &data[n..];
    }
}

/// A loader error: a message for the console and the status to return.
struct Fail(&'static str, Status);

impl Fail {
    fn new(message: &'static str) -> Fail {
        Fail(message, Status::LOAD_ERROR)
    }
}

impl From<uefi::Error> for Fail {
    fn from(e: uefi::Error) -> Fail {
        Fail("firmware call failed", e.status())
    }
}

#[entry]
fn main() -> Status {
    if let Err(e) = uefi::helpers::init() {
        return e.status();
    }
    println!("STELA boot loader {}", env!("CARGO_PKG_VERSION"));
    match boot() {
        Ok(never) => match never {},
        Err(Fail(message, status)) => {
            println!("boot: {message} ({status:?})");
            status
        }
    }
}

/// The kernel image after copying.
struct LoadedKernel {
    entry: u64,
    virt_base: u64,
    phys_base: u64,
    size: u64,
}

fn boot() -> Result<core::convert::Infallible, Fail> {
    let nx = nx_supported();
    let mut tables = PageTables::new();
    let kernel = load_kernel(&mut tables, nx)?;
    println!(
        "boot: kernel at {:#x} ({} KiB), entry {:#x}",
        kernel.phys_base,
        kernel.size / 1024,
        kernel.entry
    );

    let (initrd_phys, initrd_size) = load_initrd()?;
    if initrd_size > 0 {
        println!("boot: initrd at {initrd_phys:#x} ({initrd_size} bytes)");
    } else {
        println!(r"boot: no \STELA\initrd.tar");
    }

    let config = read_config();
    let trampoline = reserve_trampoline();

    let rsdp = find_rsdp();
    let stack = alloc_pages(STACK_SIZE / 4096);
    let stack_phys = stack.as_ptr() as u64;

    let mut info = Box::new(BootInfo::new());
    info.kernel_phys_base = kernel.phys_base;
    info.kernel_size = kernel.size;
    info.kernel_virt_base = kernel.virt_base;
    info.stack_phys_base = stack_phys;
    info.stack_size = STACK_SIZE as u64;
    info.rsdp_phys = rsdp;
    info.nx_enabled = u32::from(nx);
    info.initrd_phys = initrd_phys;
    info.initrd_size = initrd_size;
    info.ap_trampoline_phys = trampoline;
    info.set_config(&config);
    if !info.cmdline().is_empty() {
        println!(
            "boot: options {}",
            core::str::from_utf8(info.cmdline()).unwrap_or("?")
        );
    }

    // The direct map covers all RAM the firmware reports (usable, in use,
    // ACPI), the framebuffer, and at least 4 GiB. Reserved and MMIO ranges
    // above that are left out: QEMU, for one, reports a 12 GiB reserved hole
    // at 1012 GiB, which would cost 4 MiB of page tables per mapping.
    let framebuffer = framebuffer();
    let memory_top = uefi::boot::memory_map(MemoryType::LOADER_DATA)?
        .entries()
        .filter(|d| !matches!(memory_kind(d.ty), MemoryKind::Reserved | MemoryKind::Mmio))
        .map(|d| d.phys_start + d.page_count * PAGE_4K)
        .max()
        .unwrap_or(0);
    let hhdm_size = memory_top
        .max(framebuffer.phys_addr + framebuffer.size)
        .max(MIN_HHDM_SIZE)
        .next_multiple_of(PAGE_2M);
    info.framebuffer = framebuffer;
    info.hhdm_size = hhdm_size;

    let nx_flag = if nx { NO_EXECUTE } else { 0 };
    tables
        .map_linear_2m(bootinfo::HHDM_BASE, hhdm_size, WRITABLE | nx_flag)
        .map_err(|_| Fail::new("cannot build the direct map"))?;
    // The loader keeps running at its physical address for a few instructions
    // after loading CR3, so physical memory stays identity mapped (and
    // executable). The kernel removes this mapping.
    tables
        .map_linear_2m(0, hhdm_size, WRITABLE)
        .map_err(|_| Fail::new("cannot build the identity map"))?;
    info.page_table_root = tables.root();
    println!(
        "boot: framebuffer {}x{}, RSDP {:#x}, direct map {} MiB, {} page tables, NX {}",
        framebuffer.width,
        framebuffer.height,
        rsdp,
        hhdm_size >> 20,
        tables.page_count(),
        if nx { "on" } else { "off" }
    );
    println!("boot: starting the kernel");

    let handoff = Handoff {
        page_table_root: tables.root(),
        stack_top: bootinfo::HHDM_BASE + stack_phys + STACK_SIZE as u64,
        entry: kernel.entry,
        info_virt: bootinfo::HHDM_BASE + (&*info as *const BootInfo as u64),
        nx,
    };
    let info = Box::leak(info);

    // SAFETY: nothing borrowed from boot services is used after this point:
    // the kernel image, page tables, stack and boot information were
    // allocated as LOADER_DATA, which the firmware leaves alone, and no
    // protocol is open (the GOP was closed in `framebuffer`).
    #[allow(unsafe_code)]
    let mut memory_map = unsafe { uefi::boot::exit_boot_services(None) };

    memory_map.sort();
    for descriptor in memory_map.entries() {
        info.push_region(MemoryRegion::new(
            memory_kind(descriptor.ty),
            descriptor.phys_start,
            descriptor.page_count,
        ));
    }
    handoff.jump()
}

/// Whether the processor supports the execute-disable bit:
/// `CPUID.80000001H:EDX[20]` (Intel SDM Vol. 2A "CPUID", Table 3-8).
fn nx_supported() -> bool {
    use core::arch::x86_64::__cpuid;
    __cpuid(0x8000_0000).eax >= 0x8000_0001 && __cpuid(0x8000_0001).edx & (1 << 20) != 0
}

/// Reads and parses the kernel, copies its segments into one block of pages
/// and maps them at their link addresses.
fn load_kernel(tables: &mut PageTables, nx: bool) -> Result<LoadedKernel, Fail> {
    let fs = uefi::boot::get_image_file_system(uefi::boot::image_handle())?;
    let file = uefi::fs::FileSystem::new(fs)
        .read(KERNEL_PATH)
        .map_err(|_| Fail(r"cannot read \STELA\kernel.elf", Status::NOT_FOUND))?;
    let elf = Elf::parse(&file).map_err(|_| Fail::new("kernel.elf is not a valid kernel"))?;
    let (virt_base, virt_end) = elf.image_span();
    if virt_base < bootinfo::KERNEL_BASE {
        return Err(Fail::new("kernel.elf is not linked in the higher half"));
    }
    let size = virt_end - virt_base;
    let image = alloc_pages((size / PAGE_4K) as usize);
    let phys_base = image.as_ptr() as u64;

    for segment in elf.segments() {
        let offset = segment.vaddr - virt_base;
        // .bss (mem_size beyond file_size) is already zero.
        write_bytes(image, offset as usize, elf.segment_data(&segment));
        let mut flags = 0;
        if segment.flags.writable() {
            flags |= WRITABLE;
        }
        if nx && !segment.flags.executable() {
            flags |= NO_EXECUTE;
        }
        let mut page = segment.vaddr;
        while page < segment.vaddr_end() {
            tables
                .map_4k(page, phys_base + (page - virt_base), flags)
                .map_err(|_| Fail::new("cannot map the kernel"))?;
            page += PAGE_4K;
        }
    }
    Ok(LoadedKernel {
        entry: elf.entry(),
        virt_base,
        phys_base,
        size,
    })
}

/// Reads the initial RAM disk into fresh pages (type `LOADER_DATA`, so the
/// kernel sees them as `KernelAndBoot` and never reuses them). Returns
/// `(0, 0)` when the file does not exist.
fn load_initrd() -> Result<(u64, u64), Fail> {
    let fs = uefi::boot::get_image_file_system(uefi::boot::image_handle())?;
    let Ok(file) = uefi::fs::FileSystem::new(fs).read(INITRD_PATH) else {
        return Ok((0, 0));
    };
    if file.is_empty() {
        return Ok((0, 0));
    }
    let pages = alloc_pages(file.len().div_ceil(4096));
    write_bytes(pages, 0, &file);
    Ok((pages.as_ptr() as u64, file.len() as u64))
}

/// Reads `\\STELA\\boot.cfg`; empty when it does not exist.
fn read_config() -> alloc::vec::Vec<u8> {
    let Ok(fs) = uefi::boot::get_image_file_system(uefi::boot::image_handle()) else {
        return alloc::vec::Vec::new();
    };
    uefi::fs::FileSystem::new(fs)
        .read(CONFIG_PATH)
        .unwrap_or_default()
}

/// Reserves one page below 640 KiB for the application processors'
/// real-mode start-up code: the Startup IPI's vector is the number of a
/// 4 KiB page below 1 MiB (Intel SDM Vol. 3A 10.4.4.1 / 11.4.4.1 "Typical
/// BSP Initialization Sequence"). Type `LOADER_DATA`, so the kernel sees it
/// in use. Returns 0 when the firmware has no such page free.
fn reserve_trampoline() -> u64 {
    uefi::boot::allocate_pages(
        uefi::boot::AllocateType::MaxAddress(0x9_FFFF),
        MemoryType::LOADER_DATA,
        1,
    )
    .map_or(0, |page| page.as_ptr() as u64)
}

/// The ACPI RSDP from the configuration table: ACPI 2.0+ preferred, ACPI 1.0
/// otherwise (UEFI Specification 2.10, 4.6 "EFI Configuration Table").
fn find_rsdp() -> u64 {
    uefi::system::with_config_table(|entries| {
        let find = |guid| entries.iter().find(|e| e.guid == guid);
        find(ConfigTableEntry::ACPI2_GUID)
            .or_else(|| find(ConfigTableEntry::ACPI_GUID))
            .map_or(0, |e| e.address as u64)
    })
}

/// Switches the GOP to its largest landscape mode (by area; wider than
/// high: OVMF also offers squares like 2048x2048) with a
/// 32-bit RGB or BGR framebuffer (UEFI Specification 2.10, 12.9 "Graphics
/// Output Protocol": QueryMode through the mode list, SetMode). Staying in
/// the current mode is fine when it already is the largest or a switch
/// fails.
fn set_largest_mode(gop: &mut GraphicsOutput) {
    let usable = |info: &gop::ModeInfo| {
        let (w, h) = info.resolution();
        w > h
            && matches!(
                info.pixel_format(),
                gop::PixelFormat::Rgb | gop::PixelFormat::Bgr
            )
    };
    let best = gop.modes().filter(|m| usable(m.info())).max_by_key(|m| {
        let (w, h) = m.info().resolution();
        w * h
    });
    let Some(best) = best else {
        return;
    };
    if best.info().resolution() == gop.current_mode_info().resolution() {
        return;
    }
    let (w, h) = best.info().resolution();
    if gop.set_mode(&best).is_err() {
        uefi::println!("boot: cannot switch the display to {w}x{h}; keeping the current mode");
    }
}

/// The GOP framebuffer, after switching to the largest mode.
fn framebuffer() -> Framebuffer {
    let Ok(handle) = uefi::boot::get_handle_for_protocol::<GraphicsOutput>() else {
        return Framebuffer::NONE;
    };
    let Ok(mut gop) = uefi::boot::open_protocol_exclusive::<GraphicsOutput>(handle) else {
        return Framebuffer::NONE;
    };
    set_largest_mode(&mut gop);
    let mode = gop.current_mode_info();
    let format = match mode.pixel_format() {
        gop::PixelFormat::Rgb => PixelFormat::Rgb32,
        gop::PixelFormat::Bgr => PixelFormat::Bgr32,
        // Bitmask formats are rare; BltOnly has no framebuffer at all.
        gop::PixelFormat::Bitmask | gop::PixelFormat::BltOnly => return Framebuffer::NONE,
    };
    let (width, height) = mode.resolution();
    let mut buffer = gop.frame_buffer();
    Framebuffer {
        phys_addr: buffer.as_mut_ptr() as u64,
        size: buffer.size() as u64,
        width: width as u32,
        height: height as u32,
        // Both supported formats use 4 bytes per pixel.
        pitch: (mode.stride() * 4) as u32,
        format: format as u32,
    }
}

/// Folds a UEFI memory type into a [`MemoryKind`] (UEFI Specification 2.10,
/// Table 7-6 "Memory Type Usage after ExitBootServices()").
fn memory_kind(ty: MemoryType) -> MemoryKind {
    match ty {
        MemoryType::CONVENTIONAL
        | MemoryType::BOOT_SERVICES_CODE
        | MemoryType::BOOT_SERVICES_DATA => MemoryKind::Usable,
        MemoryType::LOADER_CODE | MemoryType::LOADER_DATA => MemoryKind::KernelAndBoot,
        MemoryType::ACPI_RECLAIM => MemoryKind::AcpiReclaimable,
        MemoryType::ACPI_NON_VOLATILE => MemoryKind::AcpiNvs,
        MemoryType::MMIO | MemoryType::MMIO_PORT_SPACE => MemoryKind::Mmio,
        _ => MemoryKind::Reserved,
    }
}

/// Everything the final jump needs, computed before boot services end.
struct Handoff {
    page_table_root: u64,
    stack_top: u64,
    entry: u64,
    info_virt: u64,
    nx: bool,
}

impl Handoff {
    /// Switches to the kernel's page tables and stack and jumps to its entry
    /// point with the boot information's address as the first argument.
    #[allow(unsafe_code)]
    fn jump(self) -> ! {
        // IA32_EFER.NXE, bit 11 (Intel SDM Vol. 3A 4.1.1; Vol. 4 Table 2-2).
        let nxe: u64 = if self.nx { 1 << 11 } else { 0 };
        // SAFETY: boot services have ended and the machine is ours. The new
        // page tables identity-map all physical memory (executable), so the
        // instructions after `mov cr3` still resolve; they also map the stack
        // and the boot information at their HHDM addresses and the kernel at
        // its link address. NXE is set before CR3 is loaded because the new
        // tables use bit 63. `push 0` leaves RSP = 8 (mod 16) with a null
        // return address, as at the start of a System V function.
        unsafe {
            core::arch::asm!(
                "cli",
                "mov ecx, 0xC0000080", // IA32_EFER
                "rdmsr",
                "or eax, r11d",
                "wrmsr",
                "mov rax, cr0",
                "or rax, 0x10000", // CR0.WP: honour read-only pages in ring 0
                "mov cr0, rax",
                "mov cr3, r8",
                "mov rsp, r9",
                "xor ebp, ebp",
                "push 0",
                "jmp r10",
                in("rdi") self.info_virt,
                in("r8") self.page_table_root,
                in("r9") self.stack_top,
                in("r10") self.entry,
                in("r11") nxe,
                options(noreturn)
            )
        }
    }
}
