// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! NVMe (NVM Express Base Specification 1.4): PCI class 01/08/02 SSDs
//! (`docs/storage.md`).
//!
//! ```text
//!  BAR0  0x00 CAP (MQES, DSTRD, TO)   0x14 CC   0x1C CSTS   0x24 AQA
//!        0x28 ASQ   0x30 ACQ          0x1000 + (2y [+1]) * (4 << DSTRD): doorbells
//!
//!  admin queue pair (16 entries)      I/O queue pair 1 (32 entries)
//!  submission entry 64 bytes:  DW0 opcode | CID << 16, DW1 NSID,
//!                              DW6-9 PRP1/PRP2, DW10-15 command
//!  completion entry 16 bytes:  DW2 SQ head | SQ id, DW3 CID | phase (bit 16) | status
//! ```
//!
//! A completion is new when its phase bit equals the phase the host
//! expects; the expected phase flips each time the queue wraps. One I/O
//! command in flight (one 4 KiB page, PRP1 only); the thread sleeps until
//! the MSI or a 10 ms re-check sees the completion.

use block::{BLOCK_SIZE, BlockError};

/// Admin opcodes (1.4, figure 139).
pub mod admin {
    /// Create I/O Submission Queue.
    pub const CREATE_SQ: u8 = 0x01;
    /// Create I/O Completion Queue.
    pub const CREATE_CQ: u8 = 0x05;
    /// Identify.
    pub const IDENTIFY: u8 = 0x06;
}

/// NVM command set opcodes (1.4, figure 346).
pub mod io {
    /// Flush.
    pub const FLUSH: u8 = 0x00;
    /// Write.
    pub const WRITE: u8 = 0x01;
    /// Read.
    pub const READ: u8 = 0x02;
}

/// A 64-byte submission queue entry.
pub fn command(opcode: u8, cid: u16, nsid: u32, prp1: u64, cdw: [u32; 6]) -> [u8; 64] {
    let mut e = [0u8; 64];
    e[0..4].copy_from_slice(&(u32::from(opcode) | u32::from(cid) << 16).to_le_bytes());
    e[4..8].copy_from_slice(&nsid.to_le_bytes());
    e[24..32].copy_from_slice(&prp1.to_le_bytes());
    for (i, dw) in cdw.iter().enumerate() {
        e[40 + 4 * i..44 + 4 * i].copy_from_slice(&dw.to_le_bytes());
    }
    e
}

/// What Identify Namespace says (1.4, figure 247): size in logical blocks
/// (bytes 0-7) and the size of the formatted LBA format (FLBAS at byte 26,
/// bits 3:0, selects one of the LBA formats at byte 128 + 4i, whose bits
/// 23:16 are log2 of the block size).
pub fn parse_namespace(data: &[u8]) -> Option<(u64, u32)> {
    let size = u64::from_le_bytes(data.get(0..8)?.try_into().ok()?);
    let format = usize::from(*data.get(26)? & 0xF);
    let lbaf = u32::from_le_bytes(
        data.get(128 + 4 * format..132 + 4 * format)?
            .try_into()
            .ok()?,
    );
    let shift = (lbaf >> 16) & 0xFF;
    if !(9..=12).contains(&shift) {
        return None;
    }
    Some((size, 1 << shift))
}

/// Blocks of 4 KiB in `lbas` blocks of `lba_size` bytes, and the LBAs of
/// one 4 KiB block.
pub fn geometry(lbas: u64, lba_size: u32) -> Result<(u64, u64), BlockError> {
    let per_block = BLOCK_SIZE as u64 / u64::from(lba_size);
    if per_block == 0 {
        return Err(BlockError::Io);
    }
    Ok((lbas / per_block, per_block))
}

#[cfg(target_os = "none")]
pub use system::probe;

#[cfg(target_os = "none")]
mod system {
    use super::*;
    use crate::pci::Function;
    use crate::pci::system as pci;
    use crate::sync::{SleepLock, spin_for, wait_for, wake};
    use alloc::boxed::Box;
    use alloc::string::String;
    use alloc::sync::Arc;
    use alloc::vec::Vec;
    use block::BlockDevice;
    use core::sync::atomic::{AtomicU64, Ordering};
    use frame::mm::dma::DmaBuffer;
    use frame::mm::mmio::MmioRegion;
    use frame::sync::SpinLock;
    use frame::trap::TrapFrame;

    const CC: usize = 0x14;
    const CSTS: usize = 0x1C;
    const AQA: usize = 0x24;
    const ASQ: usize = 0x28;
    const ACQ: usize = 0x30;
    const ADMIN_DEPTH: u16 = 16;
    const IO_DEPTH: u16 = 32;
    const TIMEOUT_NS: u64 = 5_000_000_000;

    /// A submission/completion queue pair.
    struct Queue {
        id: u16,
        depth: u16,
        sq: DmaBuffer,
        cq: DmaBuffer,
        tail: u16,
        head: u16,
        phase: bool,
        cid: u16,
    }

    impl Queue {
        fn new(id: u16, depth: u16) -> Option<Queue> {
            Some(Queue {
                id,
                depth,
                sq: DmaBuffer::new(usize::from(depth) * 64 / 4096 + 1)?,
                cq: DmaBuffer::new(usize::from(depth) * 16 / 4096 + 1)?,
                tail: 0,
                head: 0,
                phase: true,
                cid: 0,
            })
        }
    }

    struct Controller {
        regs: MmioRegion,
        stride: usize,
        admin: SpinLock<Queue>,
        io: SpinLock<Queue>,
        lock: SleepLock,
        data: DmaBuffer,
        waiter: AtomicU64,
    }

    impl Controller {
        fn reg32(&self, at: usize) -> u32 {
            self.regs.read_u32(at)
        }

        fn reg64(&self, at: usize) -> u64 {
            u64::from(self.regs.read_u32(at)) | u64::from(self.regs.read_u32(at + 4)) << 32
        }

        fn set64(&self, at: usize, value: u64) {
            self.regs.write_u32(at, value as u32);
            self.regs.write_u32(at + 4, (value >> 32) as u32);
        }

        fn doorbell(&self, queue: u16, completion: bool, value: u16) {
            let index = 2 * usize::from(queue) + usize::from(completion);
            self.regs
                .write_u32(0x1000 + index * self.stride, u32::from(value));
        }

        /// Submits a command and waits for its completion; returns the
        /// completion's DW0, or the status as an error.
        fn submit(
            &self,
            queue: &SpinLock<Queue>,
            opcode: u8,
            nsid: u32,
            prp1: u64,
            cdw: [u32; 6],
        ) -> Result<u32, BlockError> {
            let (cid, qid) = {
                let mut q = queue.lock();
                q.cid = q.cid.wrapping_add(1);
                let entry = command(opcode, q.cid, nsid, prp1, cdw);
                q.sq.write(usize::from(q.tail) * 64, &entry)
                    .map_err(|_| BlockError::Io)?;
                q.tail = (q.tail + 1) % q.depth;
                (q.cid, q.id)
            };
            let done = wait_for(
                TIMEOUT_NS,
                |me| {
                    self.waiter.store(me, Ordering::Release);
                    let tail = queue.lock().tail;
                    self.doorbell(qid, false, tail);
                },
                || {
                    let q = queue.lock();
                    let dw3 = q.cq.read_u32(usize::from(q.head) * 16 + 12).unwrap_or(0);
                    (dw3 >> 16 & 1 != 0) == q.phase
                },
            );
            self.waiter.store(0, Ordering::Release);
            if !done {
                return Err(BlockError::Timeout);
            }
            let mut q = queue.lock();
            let at = usize::from(q.head) * 16;
            let dw0 = q.cq.read_u32(at).unwrap_or(0);
            let dw3 = q.cq.read_u32(at + 12).unwrap_or(0);
            q.head = (q.head + 1) % q.depth;
            if q.head == 0 {
                q.phase = !q.phase;
            }
            let head = q.head;
            drop(q);
            self.doorbell(qid, true, head);
            let status = dw3 >> 17;
            if dw3 as u16 != cid || status != 0 {
                return Err(BlockError::Io);
            }
            Ok(dw0)
        }
    }

    /// An NVMe namespace.
    pub struct NvmeDisk {
        controller: Arc<Controller>,
        nsid: u32,
        blocks: u64,
        lbas_per_block: u64,
    }

    impl NvmeDisk {
        fn io(&self, opcode: u8, block: u64) -> Result<(), BlockError> {
            let lba = block * self.lbas_per_block;
            let count = (self.lbas_per_block - 1) as u32;
            self.controller.submit(
                &self.controller.io,
                opcode,
                self.nsid,
                self.controller.data.phys(),
                [lba as u32, (lba >> 32) as u32, count, 0, 0, 0],
            )?;
            Ok(())
        }
    }

    impl BlockDevice for NvmeDisk {
        fn block_count(&self) -> u64 {
            self.blocks
        }

        fn read(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
            let n = block::check_request(self.blocks, block, buf.len())?;
            let _guard = self.controller.lock.lock();
            for i in 0..n {
                self.io(io::READ, block + i)?;
                let at = i as usize * BLOCK_SIZE;
                self.controller
                    .data
                    .read(0, &mut buf[at..at + BLOCK_SIZE])
                    .map_err(|_| BlockError::Io)?;
            }
            Ok(())
        }

        fn write(&self, block: u64, buf: &[u8]) -> Result<(), BlockError> {
            let n = block::check_request(self.blocks, block, buf.len())?;
            let _guard = self.controller.lock.lock();
            for i in 0..n {
                let at = i as usize * BLOCK_SIZE;
                self.controller
                    .data
                    .write(0, &buf[at..at + BLOCK_SIZE])
                    .map_err(|_| BlockError::Io)?;
                self.io(io::WRITE, block + i)?;
            }
            Ok(())
        }

        fn flush(&self) -> Result<(), BlockError> {
            let _guard = self.controller.lock.lock();
            self.controller
                .submit(&self.controller.io, io::FLUSH, self.nsid, 0, [0; 6])
                .map(|_| ())
        }
    }

    static CONTROLLERS: SpinLock<Vec<Arc<Controller>>> = SpinLock::new(Vec::new());

    fn interrupt(_: &TrapFrame) {
        for c in CONTROLLERS.lock().iter() {
            wake(c.waiter.load(Ordering::Acquire));
        }
    }

    fn init_controller(number: usize, f: &Function) -> Result<Vec<NvmeDisk>, &'static str> {
        let regs = pci::map_bar(f, 0).map_err(|_| "cannot map BAR0")?;
        pci::enable_bus_master(f);
        let cap = u64::from(regs.read_u32(0)) | u64::from(regs.read_u32(4)) << 32;
        let stride = 4usize << ((cap >> 32) & 0xF);
        let max_entries = (cap & 0xFFFF) as u16 + 1;
        if max_entries < IO_DEPTH || (cap >> 48) & 0xF != 0 {
            return Err("queue size or page size not supported");
        }
        // Disable, wait for CSTS.RDY = 0 (1.4 7.6.1).
        regs.write_u32(CC, 0);
        // CAP.TO: worst case in 500 ms units.
        let timeout = ((cap >> 24) & 0xFF).max(1) * 500_000_000;
        if !spin_for(timeout, || regs.read_u32(CSTS) & 1 == 0) {
            return Err("does not disable");
        }
        let controller = Controller {
            regs,
            stride,
            admin: SpinLock::new(Queue::new(0, ADMIN_DEPTH).ok_or("no memory")?),
            io: SpinLock::new(Queue::new(1, IO_DEPTH).ok_or("no memory")?),
            lock: SleepLock::new(),
            data: DmaBuffer::new(1).ok_or("no memory")?,
            waiter: AtomicU64::new(0),
        };
        let (asq, acq) = {
            let a = controller.admin.lock();
            (a.sq.phys(), a.cq.phys())
        };
        let size = u32::from(ADMIN_DEPTH - 1);
        controller.regs.write_u32(AQA, size << 16 | size);
        controller.set64(ASQ, asq);
        controller.set64(ACQ, acq);
        // Enable: 4 KiB pages, NVM command set, 64-byte SQ and 16-byte CQ
        // entries (IOSQES = 6, IOCQES = 4).
        controller.regs.write_u32(CC, 6 << 16 | 4 << 20 | 1);
        if !spin_for(timeout, || controller.reg32(CSTS) & 3 != 0) || controller.reg32(CSTS) & 2 != 0
        {
            return Err("does not become ready");
        }
        let _ = controller.reg64(0);
        let controller = Arc::new(controller);
        CONTROLLERS.lock().push(Arc::clone(&controller));
        let vector = pci::enable_msi(f, interrupt).ok();
        // Identify controller (CNS 1): number of namespaces at byte 516.
        let data = controller.data.phys();
        controller
            .submit(
                &controller.admin,
                admin::IDENTIFY,
                0,
                data,
                [1, 0, 0, 0, 0, 0],
            )
            .map_err(|_| "identify controller failed")?;
        let mut id = [0u8; 4096];
        controller.data.read(0, &mut id).map_err(|_| "dma")?;
        let namespaces = u32::from_le_bytes([id[516], id[517], id[518], id[519]]);
        // The I/O queues: completion queue 1 (interrupt vector 0, enabled,
        // physically contiguous), then submission queue 1 on it.
        let (sq, cq) = {
            let q = controller.io.lock();
            (q.sq.phys(), q.cq.phys())
        };
        let qsize = u32::from(IO_DEPTH - 1) << 16 | 1;
        controller
            .submit(
                &controller.admin,
                admin::CREATE_CQ,
                0,
                cq,
                [qsize, 0b11, 0, 0, 0, 0],
            )
            .map_err(|_| "create I/O completion queue failed")?;
        controller
            .submit(
                &controller.admin,
                admin::CREATE_SQ,
                0,
                sq,
                [qsize, 1 << 16 | 1, 0, 0, 0, 0],
            )
            .map_err(|_| "create I/O submission queue failed")?;
        let mut disks = Vec::new();
        for nsid in 1..=namespaces.min(16) {
            controller
                .submit(&controller.admin, admin::IDENTIFY, nsid, data, [0; 6])
                .map_err(|_| "identify namespace failed")?;
            controller.data.read(0, &mut id).map_err(|_| "dma")?;
            let Some((lbas, lba_size)) = parse_namespace(&id) else {
                continue;
            };
            if lbas == 0 {
                continue;
            }
            let Ok((blocks, per_block)) = geometry(lbas, lba_size) else {
                frame::kprintln!("nvme: namespace {nsid}: {lba_size}-byte blocks not supported");
                continue;
            };
            frame::kprintln!(
                "nvme{number}n{nsid}: {} blocks of {lba_size} bytes ({} MiB)",
                lbas,
                (blocks * BLOCK_SIZE as u64) >> 20
            );
            disks.push(NvmeDisk {
                controller: Arc::clone(&controller),
                nsid,
                blocks,
                lbas_per_block: per_block,
            });
        }
        frame::kprintln!(
            "nvme: {}: {} namespace(s), doorbell stride {stride}, vector {}",
            f.address,
            namespaces,
            vector.map_or(String::from("none"), |v| alloc::format!("{v}"))
        );
        Ok(disks)
    }

    /// Sets up every NVMe controller and returns its namespaces, with
    /// names.
    pub fn probe() -> Vec<(String, Box<dyn BlockDevice + Send + Sync>)> {
        let mut out: Vec<(String, Box<dyn BlockDevice + Send + Sync>)> = Vec::new();
        let controllers = pci::devices().iter().filter(|f| f.is(0x01, 0x08, 0x02));
        for (number, f) in controllers.enumerate() {
            match init_controller(number, f) {
                Ok(disks) => {
                    for disk in disks {
                        let name = alloc::format!("nvme{number}n{}", disk.nsid);
                        out.push((name, Box::new(disk)));
                    }
                }
                Err(why) => frame::kprintln!("nvme: {}: {why}", f.address),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submission_entry() {
        let e = command(io::READ, 7, 1, 0x1234_5000, [8, 0, 7, 0, 0, 0]);
        assert_eq!(&e[0..4], &[0x02, 0, 7, 0]);
        assert_eq!(&e[4..8], &[1, 0, 0, 0]);
        assert_eq!(&e[24..32], &0x1234_5000u64.to_le_bytes());
        assert_eq!(&e[40..44], &[8, 0, 0, 0]);
        assert_eq!(&e[48..52], &[7, 0, 0, 0]);
    }

    #[test]
    fn namespace_data() {
        let mut ns = [0u8; 4096];
        ns[0..8].copy_from_slice(&16384u64.to_le_bytes());
        ns[26] = 1;
        ns[128..132].copy_from_slice(&(9u32 << 16).to_le_bytes());
        ns[132..136].copy_from_slice(&(12u32 << 16).to_le_bytes());
        assert_eq!(parse_namespace(&ns), Some((16384, 4096)));
        ns[26] = 0;
        assert_eq!(parse_namespace(&ns), Some((16384, 512)));
        ns[130] = 20;
        assert_eq!(parse_namespace(&ns), None);
        assert_eq!(geometry(16384, 512), Ok((2048, 8)));
        assert_eq!(geometry(16384, 4096), Ok((16384, 1)));
    }
}
