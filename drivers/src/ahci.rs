// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! AHCI (Serial ATA AHCI 1.3.1 Specification): SATA disks behind a PCI
//! class 01/06/01 controller (`docs/storage.md`).
//!
//! ```text
//!  ABAR (BAR5)  0x00 CAP  0x04 GHC  0x08 IS  0x0C PI       global registers
//!               0x100 + 0x80 * port:  CLB FB IS IE CMD TFD SIG SSTS SERR CI
//!
//!  per port, one DMA page:
//!   0x000  command list (32 headers x 32 bytes)
//!   0x400  received FIS area (256 bytes)
//!  command tables, 256 bytes per slot: command FIS (64 bytes), ATAPI
//!  (16), reserved, 0x80 PRDT (one 16-byte entry)
//!  a 64 KiB DMA data buffer per slot the requests copy through
//! ```
//!
//! One request at a time per port, but a request issues up to CAP.NCS + 1
//! commands at once (P11: every command slot, 64 KiB each): a large
//! transfer, or several blocks of `read_many`/`write_many`, is split into
//! waves; the thread sleeps until the port's interrupt (MSI) or a 10 ms
//! re-check sees the wave done, 5 seconds at most.

use block::{BLOCK_SIZE, BlockError};

/// H2D register FIS type (Serial ATA Revision 3.0, 10.3.4).
pub const FIS_TYPE_REG_H2D: u8 = 0x27;
/// IDENTIFY DEVICE (ACS-3 7.12).
pub const ATA_IDENTIFY: u8 = 0xEC;
/// READ DMA EXT (48-bit LBA).
pub const ATA_READ_DMA_EXT: u8 = 0x25;
/// WRITE DMA EXT.
pub const ATA_WRITE_DMA_EXT: u8 = 0x35;
/// FLUSH CACHE EXT.
pub const ATA_FLUSH_EXT: u8 = 0xEA;

/// A host-to-device register FIS for `command` on `lba`, `count` sectors
/// (20 bytes, the 5 DWORDs a command header's CFL counts).
pub fn command_fis(command: u8, lba: u64, count: u16) -> [u8; 20] {
    let mut fis = [0u8; 20];
    fis[0] = FIS_TYPE_REG_H2D;
    fis[1] = 0x80; // C: this is a command
    fis[2] = command;
    fis[4] = lba as u8;
    fis[5] = (lba >> 8) as u8;
    fis[6] = (lba >> 16) as u8;
    fis[7] = 0x40; // device: LBA mode
    fis[8] = (lba >> 24) as u8;
    fis[9] = (lba >> 32) as u8;
    fis[10] = (lba >> 40) as u8;
    fis[12] = count as u8;
    fis[13] = (count >> 8) as u8;
    fis
}

/// The command slots a controller has: CAP.NCS + 1 (AHCI 3.1.1, bits
/// 12:8).
pub fn slot_count(cap: u32) -> usize {
    ((cap >> 8) & 0x1F) as usize + 1
}

/// DWORD 0 of a command header: FIS length 5 DWORDs, write direction,
/// PRDT entries (AHCI 4.2.2).
pub fn header_dw0(write: bool, prdt_entries: u16) -> u32 {
    5 | if write { 1 << 6 } else { 0 } | u32::from(prdt_entries) << 16
}

/// What IDENTIFY DEVICE says about the disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    /// Addressable logical sectors (48-bit).
    pub sectors: u64,
    /// Logical sector size in bytes.
    pub sector_size: u32,
}

/// Reads the 256 words of IDENTIFY DEVICE data (ACS-3 7.12.7): words
/// 100-103 are the 48-bit sector count (word 60-61 the 28-bit one when
/// 48-bit addressing is missing, word 83 bit 10); word 106 bit 12 says
/// words 117-118 hold the logical sector size in words.
pub fn parse_identify(data: &[u8; 512]) -> Identity {
    let word = |i: usize| u16::from_le_bytes([data[2 * i], data[2 * i + 1]]);
    let lba48 = word(83) & (1 << 10) != 0;
    let sectors = if lba48 {
        (0..4).fold(0u64, |acc, i| acc | u64::from(word(100 + i)) << (16 * i))
    } else {
        u64::from(word(60)) | u64::from(word(61)) << 16
    };
    let w106 = word(106);
    let sector_size = if w106 & 0xC000 == 0x4000 && w106 & (1 << 12) != 0 {
        (u32::from(word(117)) | u32::from(word(118)) << 16) * 2
    } else {
        512
    };
    Identity {
        sectors,
        sector_size,
    }
}

/// The sectors of blocks `block..block + n` on a disk with `sector_size`
/// bytes per sector: (first LBA, sector count).
pub fn sectors_for(block: u64, n: u64, sector_size: u32) -> Result<(u64, u64), BlockError> {
    let per_block = (BLOCK_SIZE as u64) / u64::from(sector_size);
    if per_block == 0 || !(BLOCK_SIZE as u64).is_multiple_of(u64::from(sector_size)) {
        return Err(BlockError::Io);
    }
    Ok((block * per_block, n * per_block))
}

#[cfg(target_os = "none")]
pub use system::probe;

#[cfg(target_os = "none")]
mod system {
    use super::*;
    use crate::pci::Function;
    use crate::pci::system::{self as pci, InterruptError};
    use crate::sync::{SleepLock, spin_for, wait_for, wake};
    use alloc::boxed::Box;
    use alloc::string::String;
    use alloc::sync::Arc;
    use alloc::vec::Vec;
    use block::BlockDevice;
    use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    use frame::mm::dma::DmaBuffer;
    use frame::mm::mmio::MmioRegion;
    use frame::sync::SpinLock;
    use frame::trap::TrapFrame;

    const CAP: usize = 0x00;
    const GHC: usize = 0x04;
    const IS: usize = 0x08;
    const PI: usize = 0x0C;
    const GHC_AE: u32 = 1 << 31;
    const GHC_IE: u32 = 1 << 1;
    const GHC_HR: u32 = 1 << 0;

    const PX_CLB: usize = 0x00;
    const PX_CLBU: usize = 0x04;
    const PX_FB: usize = 0x08;
    const PX_FBU: usize = 0x0C;
    const PX_IS: usize = 0x10;
    const PX_IE: usize = 0x14;
    const PX_CMD: usize = 0x18;
    const PX_TFD: usize = 0x20;
    const PX_SIG: usize = 0x24;
    const PX_SSTS: usize = 0x28;
    const PX_SERR: usize = 0x30;
    const PX_CI: usize = 0x38;
    const CMD_ST: u32 = 1 << 0;
    const CMD_FRE: u32 = 1 << 4;
    const CMD_FR: u32 = 1 << 14;
    const CMD_CR: u32 = 1 << 15;
    const TFD_ERR: u32 = 1 << 0;
    const TFD_DRQ: u32 = 1 << 3;
    const TFD_BSY: u32 = 1 << 7;
    /// PxIS: task file error.
    const IS_TFES: u32 = 1 << 30;
    /// SATA disk signature (AHCI 3.3.9).
    const SIG_ATA: u32 = 0x0000_0101;

    /// Bytes moved per command (16 blocks).
    const CHUNK: usize = 64 * 1024;
    /// Bytes of a command table (FIS, ATAPI, PRDT with one entry).
    const TABLE: usize = 256;

    /// One command of a wave: the FIS, the direction and the bytes.
    struct Job {
        fis: [u8; 20],
        write: bool,
        bytes: usize,
    }
    const TIMEOUT_NS: u64 = 5_000_000_000;

    struct Port {
        hba: Arc<MmioRegion>,
        base: usize,
        /// Command list and FIS area.
        page: DmaBuffer,
        /// One command table per slot.
        tables: DmaBuffer,
        /// One 64 KiB data buffer per slot.
        data: Vec<DmaBuffer>,
        identity: Identity,
        lock: SleepLock,
        /// Thread waiting for the current command (0: none).
        waiter: AtomicU64,
        /// PxIS bits the interrupt handler collected.
        status: AtomicU32,
    }

    impl Port {
        fn read(&self, reg: usize) -> u32 {
            self.hba.read_u32(self.base + reg)
        }

        fn write(&self, reg: usize, value: u32) {
            self.hba.write_u32(self.base + reg, value)
        }

        /// Runs one command on slot 0 and waits for it.
        fn run(&self, fis: &[u8; 20], write: bool, bytes: usize) -> Result<(), BlockError> {
            self.run_wave(&[Job {
                fis: *fis,
                write,
                bytes,
            }])
        }

        /// Runs `jobs` (at most one per slot; job i uses slot i and its
        /// data buffer) at once and waits for all of them.
        fn run_wave(&self, jobs: &[Job]) -> Result<(), BlockError> {
            if jobs.is_empty() || jobs.len() > self.data.len() {
                return Err(BlockError::Io);
            }
            for (slot, job) in jobs.iter().enumerate() {
                let dw0 = header_dw0(job.write, if job.bytes > 0 { 1 } else { 0 });
                let at = slot * TABLE;
                let table = self.tables.phys() + at as u64;
                let ok = (|| {
                    self.page.write_u32(32 * slot, dw0)?;
                    self.page.write_u32(32 * slot + 4, 0)?;
                    self.page.write_u32(32 * slot + 8, table as u32)?;
                    self.page.write_u32(32 * slot + 12, (table >> 32) as u32)?;
                    self.tables.write(at, &job.fis)?;
                    if job.bytes > 0 {
                        let data = self.data[slot].phys();
                        self.tables.write_u32(at + 0x80, data as u32)?;
                        self.tables.write_u32(at + 0x84, (data >> 32) as u32)?;
                        self.tables.write_u32(at + 0x88, 0)?;
                        // Byte count - 1; bit 31: interrupt on completion.
                        self.tables
                            .write_u32(at + 0x8C, (job.bytes as u32 - 1) | 1 << 31)?;
                    }
                    Ok::<(), frame::mm::dma::OutOfRange>(())
                })();
                ok.map_err(|_| BlockError::Io)?;
            }
            let mask = if jobs.len() == 32 {
                u32::MAX
            } else {
                (1u32 << jobs.len()) - 1
            };
            // The device must not be busy (AHCI 5.6.2.1).
            if !wait_for(
                TIMEOUT_NS,
                |_| {},
                || self.read(PX_TFD) & (TFD_BSY | TFD_DRQ) == 0,
            ) {
                return Err(BlockError::Timeout);
            }
            self.status.store(0, Ordering::Release);
            let done = wait_for(
                TIMEOUT_NS,
                |me| {
                    self.waiter.store(me, Ordering::Release);
                    self.write(PX_CI, mask);
                },
                || self.read(PX_CI) & mask == 0 || self.read(PX_IS) & IS_TFES != 0,
            );
            self.waiter.store(0, Ordering::Release);
            let is = self.read(PX_IS) | self.status.swap(0, Ordering::AcqRel);
            self.write(PX_IS, is);
            if !done {
                return Err(BlockError::Timeout);
            }
            if is & IS_TFES != 0 || self.read(PX_TFD) & TFD_ERR != 0 {
                return Err(BlockError::Io);
            }
            Ok(())
        }

        /// Called from the interrupt handler with the port's PxIS bits
        /// (already cleared in the register).
        fn interrupt(&self, is: u32) {
            self.status.fetch_or(is, Ordering::AcqRel);
            wake(self.waiter.load(Ordering::Acquire));
        }
    }

    /// A SATA disk on an AHCI port.
    pub struct AhciDisk {
        port: Arc<Port>,
        name: String,
    }

    impl AhciDisk {
        /// "ahci0p1" style name: controller and port.
        pub fn name(&self) -> &str {
            &self.name
        }

        /// Reads (`reads`) or writes (`writes`) the requests, 64 KiB per
        /// command, a slot's worth of commands at a time.
        fn transfer(
            &self,
            reads: &mut [(u64, &mut [u8])],
            writes: &[(u64, &[u8])],
        ) -> Result<(), BlockError> {
            let size = self.port.identity.sector_size;
            let count = self.block_count();
            // The pieces: (request, byte offset in it, first block, blocks).
            let mut pieces: Vec<(usize, usize, u64, u64)> = Vec::new();
            let lens: Vec<(u64, usize)> = if writes.is_empty() {
                reads.iter().map(|(b, buf)| (*b, buf.len())).collect()
            } else {
                writes.iter().map(|(b, buf)| (*b, buf.len())).collect()
            };
            for (r, &(block, len)) in lens.iter().enumerate() {
                let n = block::check_request(count, block, len)?;
                let mut done = 0;
                while done < n {
                    let blocks = (n - done).min((CHUNK / BLOCK_SIZE) as u64);
                    pieces.push((r, done as usize * BLOCK_SIZE, block + done, blocks));
                    done += blocks;
                }
            }
            let write = !writes.is_empty();
            let _guard = self.port.lock.lock();
            for wave in pieces.chunks(self.port.data.len()) {
                let mut jobs = Vec::with_capacity(wave.len());
                for (slot, &(r, at, block, blocks)) in wave.iter().enumerate() {
                    let bytes = blocks as usize * BLOCK_SIZE;
                    let (lba, sectors) = sectors_for(block, blocks, size)?;
                    if write {
                        self.port.data[slot]
                            .write(0, &writes[r].1[at..at + bytes])
                            .map_err(|_| BlockError::Io)?;
                    }
                    let command = if write {
                        ATA_WRITE_DMA_EXT
                    } else {
                        ATA_READ_DMA_EXT
                    };
                    jobs.push(Job {
                        fis: command_fis(command, lba, sectors as u16),
                        write,
                        bytes,
                    });
                }
                self.port.run_wave(&jobs)?;
                if !write {
                    for (slot, &(r, at, _, blocks)) in wave.iter().enumerate() {
                        let bytes = blocks as usize * BLOCK_SIZE;
                        self.port.data[slot]
                            .read(0, &mut reads[r].1[at..at + bytes])
                            .map_err(|_| BlockError::Io)?;
                    }
                }
            }
            Ok(())
        }
    }

    impl BlockDevice for AhciDisk {
        fn block_count(&self) -> u64 {
            let id = self.port.identity;
            id.sectors * u64::from(id.sector_size) / BLOCK_SIZE as u64
        }

        fn read(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
            self.transfer(&mut [(block, buf)], &[])
        }

        fn write(&self, block: u64, buf: &[u8]) -> Result<(), BlockError> {
            self.transfer(&mut [], &[(block, buf)])
        }

        fn read_many(&self, requests: &mut [(u64, &mut [u8])]) -> Result<(), BlockError> {
            self.transfer(requests, &[])
        }

        fn write_many(&self, requests: &[(u64, &[u8])]) -> Result<(), BlockError> {
            if requests.is_empty() {
                return Ok(());
            }
            self.transfer(&mut [], requests)
        }

        fn flush(&self) -> Result<(), BlockError> {
            let _guard = self.port.lock.lock();
            self.port.run(&command_fis(ATA_FLUSH_EXT, 0, 0), false, 0)
        }
    }

    /// Every port with a disk, for the interrupt handler.
    static PORTS: SpinLock<Vec<Arc<Port>>> = SpinLock::new(Vec::new());
    /// The controllers' global registers, for the interrupt handler.
    static HBAS: SpinLock<Vec<Arc<MmioRegion>>> = SpinLock::new(Vec::new());

    /// The MSI handler: for every port whose bit is set in IS, clears its
    /// PxIS (first, so that it does not raise IS again: AHCI 10.7.2.1),
    /// hands the bits to the waiting request, then clears IS.
    fn interrupt(_: &TrapFrame) {
        let hbas = HBAS.lock().clone();
        let ports = PORTS.lock().clone();
        for hba in &hbas {
            let is = hba.read_u32(IS);
            for index in (0..32).filter(|i| is & (1 << i) != 0) {
                let base = 0x100 + 0x80 * index;
                let port_is = hba.read_u32(base + PX_IS);
                hba.write_u32(base + PX_IS, port_is);
                if let Some(port) = ports
                    .iter()
                    .find(|p| Arc::ptr_eq(&p.hba, hba) && p.base == base)
                {
                    port.interrupt(port_is);
                }
            }
            hba.write_u32(IS, is);
        }
    }

    /// Stops a port's command and FIS engines (AHCI 10.1.2).
    fn stop(hba: &MmioRegion, base: usize) -> bool {
        let cmd = hba.read_u32(base + PX_CMD);
        hba.write_u32(base + PX_CMD, cmd & !(CMD_ST | CMD_FRE));
        spin_for(500_000_000, || {
            hba.read_u32(base + PX_CMD) & (CMD_CR | CMD_FR) == 0
        })
    }

    fn init_port(hba: &Arc<MmioRegion>, index: usize, slots: usize) -> Option<Port> {
        let base = 0x100 + 0x80 * index;
        // After the HBA reset the link comes back up by itself: wait for
        // DET = 3 (device present, communication established).
        if !spin_for(10_000_000, || hba.read_u32(base + PX_SSTS) & 0xF == 3) {
            return None;
        }
        if !stop(hba, base) {
            frame::kprintln!("ahci: port {index}: engines do not stop");
            return None;
        }
        let page = DmaBuffer::new(1)?;
        let tables = DmaBuffer::new((slots * TABLE).div_ceil(4096))?;
        let mut data = Vec::with_capacity(slots);
        for _ in 0..slots {
            data.push(DmaBuffer::new(CHUNK / 4096)?);
        }
        hba.write_u32(base + PX_CLB, page.phys() as u32);
        hba.write_u32(base + PX_CLBU, (page.phys() >> 32) as u32);
        let fis = page.phys() + 0x400;
        hba.write_u32(base + PX_FB, fis as u32);
        hba.write_u32(base + PX_FBU, (fis >> 32) as u32);
        hba.write_u32(base + PX_SERR, 0xFFFF_FFFF);
        hba.write_u32(base + PX_IS, 0xFFFF_FFFF);
        let cmd = hba.read_u32(base + PX_CMD);
        hba.write_u32(base + PX_CMD, cmd | CMD_FRE);
        // The signature arrives with the device's first D2H register FIS,
        // which needs FIS reception (AHCI 3.3.9, 10.1.2).
        let ready = spin_for(1_000_000_000, || {
            hba.read_u32(base + PX_TFD) & (TFD_BSY | TFD_DRQ) == 0
        });
        let sig = hba.read_u32(base + PX_SIG);
        if !ready || sig != SIG_ATA {
            frame::kprintln!("ahci: port {index}: signature {sig:#x}, not a SATA disk; skipped");
            hba.write_u32(base + PX_CMD, cmd & !CMD_FRE);
            return None;
        }
        hba.write_u32(base + PX_CMD, cmd | CMD_FRE | CMD_ST);
        // Interrupts: D2H register FIS, PIO setup, DMA setup, set device
        // bits, and the error bits 22-31.
        hba.write_u32(base + PX_IE, 0xFFC0_000F);
        let mut port = Port {
            hba: Arc::clone(hba),
            base,
            page,
            tables,
            data,
            identity: Identity {
                sectors: 0,
                sector_size: 512,
            },
            lock: SleepLock::new(),
            waiter: AtomicU64::new(0),
            status: AtomicU32::new(0),
        };
        if port
            .run(&command_fis(ATA_IDENTIFY, 0, 0), false, 512)
            .is_err()
        {
            frame::kprintln!("ahci: port {index}: IDENTIFY failed");
            return None;
        }
        let mut id = [0u8; 512];
        port.data[0].read(0, &mut id).ok()?;
        port.identity = parse_identify(&id);
        if !(512..=4096).contains(&port.identity.sector_size)
            || !(BLOCK_SIZE as u32).is_multiple_of(port.identity.sector_size)
        {
            frame::kprintln!(
                "ahci: port {index}: {}-byte sectors not supported",
                port.identity.sector_size
            );
            return None;
        }
        Some(port)
    }

    /// Sets up controller `f` and returns its disks.
    fn init_controller(number: usize, f: &Function) -> Vec<AhciDisk> {
        let hba = match pci::map_bar(f, 5) {
            Ok(r) => Arc::new(r),
            Err(e) => {
                frame::kprintln!("ahci: {}: cannot map ABAR: {e:?}", f.address);
                return Vec::new();
            }
        };
        pci::enable_bus_master(f);
        // Reset the HBA (AHCI 10.4.3), then AHCI mode.
        hba.write_u32(GHC, hba.read_u32(GHC) | GHC_AE);
        hba.write_u32(GHC, hba.read_u32(GHC) | GHC_HR);
        if !spin_for(1_000_000_000, || hba.read_u32(GHC) & GHC_HR == 0) {
            frame::kprintln!("ahci: {}: reset does not complete", f.address);
            return Vec::new();
        }
        hba.write_u32(GHC, hba.read_u32(GHC) | GHC_AE);
        let cap = hba.read_u32(CAP);
        let implemented = hba.read_u32(PI);
        let vector = match pci::enable_msi(f, interrupt) {
            Ok(v) => Some(v),
            Err(InterruptError::NoMsi) => {
                frame::kprintln!("ahci: {}: no MSI; polling", f.address);
                None
            }
            Err(e) => {
                frame::kprintln!("ahci: {}: no interrupt ({e:?}); polling", f.address);
                None
            }
        };
        HBAS.lock().push(Arc::clone(&hba));
        hba.write_u32(IS, 0xFFFF_FFFF);
        hba.write_u32(GHC, hba.read_u32(GHC) | GHC_IE);
        let mut disks = Vec::new();
        for index in 0..32 {
            if implemented & (1 << index) == 0 {
                continue;
            }
            let Some(port) = init_port(&hba, index, slot_count(cap)) else {
                continue;
            };
            let port = Arc::new(port);
            PORTS.lock().push(Arc::clone(&port));
            let disk = AhciDisk {
                port,
                name: alloc::format!("ahci{number}p{index}"),
            };
            frame::kprintln!(
                "ahci: {} port {index}: disk, {} sectors of {} bytes ({} MiB)",
                f.address,
                disk.port.identity.sectors,
                disk.port.identity.sector_size,
                (disk.block_count() * BLOCK_SIZE as u64) >> 20
            );
            disks.push(disk);
        }
        frame::kprintln!(
            "ahci: {}: {} port(s), {} command slots, 64-bit DMA {}, vector {}",
            f.address,
            (cap & 0x1F) + 1,
            slot_count(cap),
            if cap & (1 << 31) != 0 { "yes" } else { "no" },
            vector.map_or(alloc::string::String::from("none"), |v| alloc::format!(
                "{v}"
            ))
        );
        disks
    }

    /// Sets up every AHCI controller and returns the disks found, with
    /// their names.
    pub fn probe() -> Vec<(String, Box<dyn BlockDevice + Send + Sync>)> {
        let mut out: Vec<(String, Box<dyn BlockDevice + Send + Sync>)> = Vec::new();
        let controllers = pci::devices().iter().filter(|f| f.is(0x01, 0x06, 0x01));
        for (number, f) in controllers.enumerate() {
            for disk in init_controller(number, f) {
                out.push((String::from(disk.name()), Box::new(disk)));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fis_and_header() {
        let fis = command_fis(ATA_READ_DMA_EXT, 0x0102_0304_0506, 8);
        assert_eq!(
            fis[..14],
            [
                0x27, 0x80, 0x25, 0, 0x06, 0x05, 0x04, 0x40, 0x03, 0x02, 0x01, 0, 8, 0
            ]
        );
        assert_eq!(header_dw0(false, 1), 0x0001_0005);
        assert_eq!(header_dw0(true, 1), 0x0001_0045);
        assert_eq!(slot_count(0x4000_1F05), 32);
        assert_eq!(slot_count(0x0000_0000), 1);
    }

    #[test]
    fn identify_data() {
        let mut data = [0u8; 512];
        let mut set = |w: usize, v: u16| data[2 * w..2 * w + 2].copy_from_slice(&v.to_le_bytes());
        set(83, 1 << 10);
        set(100, 0x0000);
        set(101, 0x0002); // 0x2_0000 sectors = 64 MiB
        let id = parse_identify(&data);
        assert_eq!(
            id,
            Identity {
                sectors: 0x2_0000,
                sector_size: 512
            }
        );
        // 4 KiB logical sectors: word 106 = 0x5000, words 117-118 = 2048.
        let mut data4k = data;
        data4k[212..214].copy_from_slice(&0x5000u16.to_le_bytes());
        data4k[234..236].copy_from_slice(&2048u16.to_le_bytes());
        assert_eq!(parse_identify(&data4k).sector_size, 4096);
        // No 48-bit addressing: words 60-61.
        let mut old = [0u8; 512];
        old[120..122].copy_from_slice(&0x1000u16.to_le_bytes());
        assert_eq!(parse_identify(&old).sectors, 0x1000);
    }

    #[test]
    fn sector_math() {
        assert_eq!(sectors_for(3, 2, 512), Ok((24, 16)));
        assert_eq!(sectors_for(3, 2, 4096), Ok((3, 2)));
        assert_eq!(sectors_for(0, 1, 8192), Err(BlockError::Io));
        assert_eq!(sectors_for(0, 1, 520), Err(BlockError::Io));
    }
}
