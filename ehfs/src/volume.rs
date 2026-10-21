// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The volume: superblock, blocks, transactions and the journal, and the
//! allocation bitmap (68k_HAL `docs/ehfs.md` section 5 and 11.2).
//!
//! Every metadata change (superblock, bitmap, B-tree nodes) belongs to a
//! transaction: [`Volume::begin`], the changes (kept in memory),
//! [`Volume::commit`]. The journal (CNID 7) is one run of blocks:
//!
//! ```text
//!  j+0  header: "eHFSjrnl", sequence of the last transaction written home, CRC32C
//!  j+1  descriptor: "eHFStxn\0", sequence, count, target block numbers
//!  j+2  .. the new contents of the targets
//!  j+2+count  commit: "eHFScmt\0", sequence, count, CRC32C of descriptor and contents
//! ```
//!
//! Commit writes the transaction, then every block home, then the header.
//! Mount finds a complete transaction newer than the header and writes it
//! home again. File data is written straight to its blocks before the
//! transaction that gives them to the file (ordered mode).

use crate::Error;
use crate::crc32c::crc32c;
use crate::format::*;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use block::BlockDevice;

const BITS_PER_BLOCK: u64 = (BLOCK_SIZE * 8) as u64;

/// One metadata block changed by the open transaction.
type Changed = (u64, Box<[u8; BLOCK_SIZE]>);

/// A mounted (or being formatted) eHFS volume on a block device whose
/// block 0 is the volume's block 0.
pub struct Volume<D> {
    pub(crate) dev: D,
    pub(crate) blocks: u64,
    read_only: bool,
    pub(crate) sb: Vec<u8>,
    pub(crate) alloc_hint: u64,
    in_txn: bool,
    pub(crate) sb_dirty: bool,
    txn: Vec<Changed>,
    journal_sequence: u64,
}

fn io<T>(r: Result<T, block::BlockError>) -> Result<T, Error> {
    r.map_err(|_| Error::Io)
}

/// Whether `sb` (3072 bytes) is a superblock this code can use.
pub(crate) fn sb_check(sb: &[u8]) -> Result<(), Error> {
    if &sb[..8] != MAGIC || le32(sb, sb::CRC) != crc32c(0, &sb[..sb::CRC]) {
        return Err(Error::Corrupt);
    }
    if le32(sb, sb::VERSION) != VERSION
        || le32(sb, sb::BLOCK_SIZE) != BLOCK_SIZE as u32
        || le32(sb, sb::INCOMPAT) != 0
        || le32(sb, sb::NODE_SIZE) != NODE_SIZE as u32
    {
        return Err(Error::Unsupported);
    }
    Ok(())
}

/// Stores the superblock CRC.
pub(crate) fn sb_seal(sb: &mut [u8]) {
    let crc = crc32c(0, &sb[..sb::CRC]);
    put32(sb, sb::CRC, crc);
}

impl<D: BlockDevice> Volume<D> {
    /// A volume object for formatting: nothing read yet.
    pub(crate) fn raw(dev: D, blocks: u64) -> Volume<D> {
        Volume {
            dev,
            blocks,
            read_only: false,
            sb: vec![0; SB_SIZE],
            alloc_hint: 0,
            in_txn: false,
            sb_dirty: false,
            txn: Vec::new(),
            journal_sequence: 0,
        }
    }

    /// Mounts the volume on `dev`: checks the superblock, replays a
    /// committed transaction left in the journal (read-only: reads through
    /// it instead) and, for writing, marks the volume mounted.
    pub fn mount(dev: D, read_only: bool) -> Result<Volume<D>, Error> {
        let read_only = read_only || dev.read_only();
        let mut v = Volume::raw(dev, 1);
        v.read_only = read_only;
        v.load_sb()?;
        v.blocks = v.sb_get64(sb::BLOCK_COUNT);
        if v.blocks < 16 || v.blocks > v.dev.block_count() {
            return Err(Error::Corrupt);
        }
        let sequence = v.journal_pending()?;
        if !v.txn.is_empty() {
            if read_only {
                // Read through: the transaction's blocks stay in memory.
                v.load_sb()?;
            } else {
                let txn = core::mem::take(&mut v.txn);
                for (n, data) in &txn {
                    v.dev_write(*n, &data[..])?;
                }
                v.write_journal_header(sequence)?;
                v.journal_sequence = sequence;
                v.load_sb()?;
            }
        }
        if !read_only {
            v.begin()?;
            v.sb_set32(sb::STATE, state::MOUNTED);
            v.commit()?;
        }
        Ok(v)
    }

    /// Marks the volume clean (when mounted for writing) and flushes the
    /// device.
    pub fn unmount(&mut self) -> Result<(), Error> {
        if self.read_only {
            return Ok(());
        }
        self.begin()?;
        self.sb_set32(sb::STATE, state::CLEAN);
        self.commit()?;
        io(self.dev.flush())
    }

    /// Whether the volume is mounted read-only.
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// Blocks in the volume.
    pub fn block_count(&self) -> u64 {
        self.blocks
    }

    /// Free blocks.
    pub fn free_blocks(&self) -> u64 {
        self.sb_get64(sb::FREE_BLOCKS)
    }

    /// The volume name.
    pub fn name(&self) -> alloc::string::String {
        let n = usize::from(self.sb[sb::NAME]);
        alloc::string::String::from_utf8_lossy(&self.sb[sb::NAME + 1..sb::NAME + 1 + n]).into()
    }

    /// The state field (clean, mounted, errors) as found at mount.
    pub fn state(&self) -> u32 {
        self.sb_get32(sb::STATE)
    }

    /// The volume UUID.
    pub fn uuid(&self) -> [u8; 16] {
        let mut u = [0u8; 16];
        u.copy_from_slice(&self.sb[sb::UUID..sb::UUID + 16]);
        u
    }

    /// Files and folders (without the root).
    pub fn counts(&self) -> (u64, u64) {
        (
            self.sb_get64(sb::FILE_COUNT),
            self.sb_get64(sb::FOLDER_COUNT),
        )
    }

    /// The device.
    pub fn device(&self) -> &D {
        &self.dev
    }

    // ---- Superblock fields ----

    pub(crate) fn sb_get64(&self, at: usize) -> u64 {
        le64(&self.sb, at)
    }

    pub(crate) fn sb_set64(&mut self, at: usize, v: u64) {
        put64(&mut self.sb, at, v);
        self.sb_dirty = true;
    }

    pub(crate) fn sb_get32(&self, at: usize) -> u32 {
        le32(&self.sb, at)
    }

    pub(crate) fn sb_set32(&mut self, at: usize, v: u32) {
        put32(&mut self.sb, at, v);
        self.sb_dirty = true;
    }

    /// A special file's fork descriptor (in the superblock).
    pub(crate) fn sb_fork(&self, which: Special) -> &[u8] {
        let at = sb::FORKS + which as usize * FORK_SIZE;
        &self.sb[at..at + FORK_SIZE]
    }

    pub(crate) fn sb_fork_mut(&mut self, which: Special) -> &mut [u8] {
        self.sb_dirty = true;
        let at = sb::FORKS + which as usize * FORK_SIZE;
        &mut self.sb[at..at + FORK_SIZE]
    }

    // ---- The device ----

    pub(crate) fn dev_read(&self, n: u64, buf: &mut [u8]) -> Result<(), Error> {
        if n >= self.blocks {
            return Err(Error::Invalid);
        }
        io(self.dev.read(n, buf))
    }

    pub(crate) fn dev_write(&self, n: u64, buf: &[u8]) -> Result<(), Error> {
        if n >= self.blocks {
            return Err(Error::Invalid);
        }
        io(self.dev.write(n, buf))
    }

    /// Reads runs of data blocks (first block, offset in `buf`, bytes) in
    /// one `read_many`.
    pub(crate) fn dev_read_runs(
        &self,
        runs: &[(u64, usize, usize)],
        buf: &mut [u8],
    ) -> Result<(), Error> {
        if !self.txn.is_empty() {
            // A transaction may hold newer copies: block by block.
            for &(block, offset, len) in runs {
                for i in 0..len / BLOCK_SIZE {
                    let at = offset + i * BLOCK_SIZE;
                    self.read_block(block + i as u64, &mut buf[at..at + BLOCK_SIZE])?;
                }
            }
            return Ok(());
        }
        let mut requests: Vec<(u64, &mut [u8])> = Vec::with_capacity(runs.len());
        let mut rest: &mut [u8] = buf;
        let mut at = 0;
        for &(block, offset, len) in runs {
            if block + (len / BLOCK_SIZE) as u64 > self.blocks || offset < at {
                return Err(Error::Invalid);
            }
            let (_, tail) = core::mem::take(&mut rest).split_at_mut(offset - at);
            let (piece, tail) = tail.split_at_mut(len);
            requests.push((block, piece));
            rest = tail;
            at = offset + len;
        }
        io(self.dev.read_many(&mut requests))
    }

    /// Reads block `n` (the transaction's copy when it changed it).
    pub(crate) fn read_block(&self, n: u64, buf: &mut [u8]) -> Result<(), Error> {
        if let Some((_, data)) = self.txn.iter().find(|(t, _)| *t == n) {
            buf.copy_from_slice(&data[..]);
            return Ok(());
        }
        self.dev_read(n, buf)
    }

    /// Changes metadata block `n` inside the transaction.
    pub(crate) fn write_meta(&mut self, n: u64, buf: &[u8]) -> Result<(), Error> {
        if self.read_only {
            return Err(Error::ReadOnly);
        }
        if !self.in_txn || n >= self.blocks {
            return Err(Error::Invalid);
        }
        if let Some((_, data)) = self.txn.iter_mut().find(|(t, _)| *t == n) {
            data.copy_from_slice(buf);
            return Ok(());
        }
        if self.txn.len() == TXN_BLOCKS {
            return Err(Error::TxnTooLarge);
        }
        let mut data = Box::new([0u8; BLOCK_SIZE]);
        data.copy_from_slice(buf);
        self.txn.push((n, data));
        Ok(())
    }

    /// Writes a data block straight to the device (file contents).
    pub(crate) fn write_data(&mut self, n: u64, buf: &[u8]) -> Result<(), Error> {
        if self.read_only {
            return Err(Error::ReadOnly);
        }
        self.dev_write(n, buf)
    }

    // ---- Superblock ----

    fn load_sb(&mut self) -> Result<(), Error> {
        let mut b = vec![0u8; BLOCK_SIZE];
        self.read_block(0, &mut b)?;
        self.sb.copy_from_slice(&b[SB_OFFSET..SB_OFFSET + SB_SIZE]);
        sb_check(&self.sb)
    }

    /// Block 0 as it is to be written: 1024 zero bytes, then the sealed
    /// superblock.
    pub(crate) fn sb_block(&self) -> Vec<u8> {
        let mut b = vec![0u8; BLOCK_SIZE];
        b[SB_OFFSET..].copy_from_slice(&self.sb);
        sb_seal(&mut b[SB_OFFSET..]);
        b
    }

    // ---- Transactions ----

    /// Starts a transaction.
    pub(crate) fn begin(&mut self) -> Result<(), Error> {
        if self.read_only {
            return Err(Error::ReadOnly);
        }
        if self.in_txn {
            return Err(Error::Invalid);
        }
        self.in_txn = true;
        self.sb_dirty = false;
        self.txn.clear();
        Ok(())
    }

    fn journal_start(&self) -> u64 {
        le64(self.sb_fork(Special::Journal), fork::EXTENT)
    }

    fn journal_blocks(&self) -> u64 {
        le64(self.sb_fork(Special::Journal), fork::EXTENT + 8)
    }

    fn write_journal_header(&self, sequence: u64) -> Result<(), Error> {
        let mut b = vec![0u8; BLOCK_SIZE];
        b[..8].copy_from_slice(journal::MAGIC);
        put64(&mut b, journal::HDR_SEQUENCE, sequence);
        let crc = crc32c(0, &b[..journal::HDR_CRC]);
        put32(&mut b, journal::HDR_CRC, crc);
        self.dev_write(self.journal_start(), &b)
    }

    /// Commits the transaction: journal, home, journal header.
    pub(crate) fn commit(&mut self) -> Result<(), Error> {
        if !self.in_txn {
            return Err(Error::Invalid);
        }
        if self.sb_dirty {
            let b = self.sb_block();
            if let Err(e) = self.write_meta(0, &b) {
                self.abort();
                return Err(e);
            }
        }
        if self.txn.is_empty() {
            self.in_txn = false;
            return Ok(());
        }
        let count = self.txn.len();
        if count > journal::TXN_MAX || count as u64 + 3 > self.journal_blocks() {
            self.abort();
            return Err(Error::TxnTooLarge);
        }
        let j = self.journal_start();
        let sequence = self.journal_sequence + 1;
        let result = (|| {
            let mut b = vec![0u8; BLOCK_SIZE];
            b[..8].copy_from_slice(journal::TXN_MAGIC);
            put64(&mut b, journal::TXN_SEQUENCE, sequence);
            put32(&mut b, journal::TXN_COUNT, count as u32);
            for (i, (n, _)) in self.txn.iter().enumerate() {
                put64(&mut b, journal::TXN_TARGETS + 8 * i, *n);
            }
            let mut crc = crc32c(0, &b);
            self.dev_write(j + 1, &b)?;
            for (i, (_, data)) in self.txn.iter().enumerate() {
                crc = crc32c(crc, &data[..]);
                self.dev_write(j + 2 + i as u64, &data[..])?;
            }
            b.fill(0);
            b[..8].copy_from_slice(journal::COMMIT_MAGIC);
            put64(&mut b, journal::COMMIT_SEQUENCE, sequence);
            put32(&mut b, journal::COMMIT_COUNT, count as u32);
            put32(&mut b, journal::COMMIT_CRC, crc);
            self.dev_write(j + 2 + count as u64, &b)?;
            for (n, data) in &self.txn {
                self.dev_write(*n, &data[..])?;
            }
            self.write_journal_header(sequence)
        })();
        self.txn.clear();
        self.in_txn = false;
        if let Err(e) = result {
            // The disk is in some state the next mount repairs; this mount
            // must not go on writing.
            self.read_only = true;
            return Err(e);
        }
        self.journal_sequence = sequence;
        self.sb_dirty = false;
        Ok(())
    }

    /// Forgets the transaction's changes (the superblock is read back).
    pub(crate) fn abort(&mut self) {
        self.txn.clear();
        self.in_txn = false;
        if self.sb_dirty {
            let mut b = vec![0u8; BLOCK_SIZE];
            if self.dev_read(0, &mut b).is_ok() {
                self.sb.copy_from_slice(&b[SB_OFFSET..SB_OFFSET + SB_SIZE]);
            }
        }
        self.sb_dirty = false;
    }

    /// Commits when `r` is Ok, aborts otherwise.
    pub(crate) fn finish<T>(&mut self, r: Result<T, Error>) -> Result<T, Error> {
        match r {
            Ok(v) => {
                self.commit()?;
                Ok(v)
            }
            Err(e) => {
                self.abort();
                Err(e)
            }
        }
    }

    /// A complete, intact transaction newer than the journal header: its
    /// blocks are loaded into `self.txn`. Returns the sequence it would
    /// have.
    fn journal_pending(&mut self) -> Result<u64, Error> {
        let j = self.journal_start();
        let size = self.journal_blocks();
        self.txn.clear();
        if size < 4 || j + size > self.blocks {
            return Err(Error::Corrupt);
        }
        let mut b = vec![0u8; BLOCK_SIZE];
        self.dev_read(j, &mut b)?;
        if &b[..8] != journal::MAGIC
            || le32(&b, journal::HDR_CRC) != crc32c(0, &b[..journal::HDR_CRC])
        {
            return Err(Error::Corrupt);
        }
        self.journal_sequence = le64(&b, journal::HDR_SEQUENCE);
        let sequence = self.journal_sequence + 1;
        self.dev_read(j + 1, &mut b)?;
        let count = le32(&b, journal::TXN_COUNT) as usize;
        if &b[..8] != journal::TXN_MAGIC
            || le64(&b, journal::TXN_SEQUENCE) != sequence
            || count == 0
            || count > TXN_BLOCKS
            || count as u64 + 3 > size
        {
            return Ok(sequence);
        }
        let mut crc = crc32c(0, &b);
        let mut targets = Vec::with_capacity(count);
        for i in 0..count {
            let t = le64(&b, journal::TXN_TARGETS + 8 * i);
            if t >= self.blocks {
                return Ok(sequence);
            }
            targets.push(t);
        }
        let mut txn = Vec::with_capacity(count);
        for (i, &t) in targets.iter().enumerate() {
            let mut data = Box::new([0u8; BLOCK_SIZE]);
            self.dev_read(j + 2 + i as u64, &mut data[..])?;
            crc = crc32c(crc, &data[..]);
            txn.push((t, data));
        }
        self.dev_read(j + 2 + count as u64, &mut b)?;
        if &b[..8] != journal::COMMIT_MAGIC
            || le64(&b, journal::COMMIT_SEQUENCE) != sequence
            || le32(&b, journal::COMMIT_COUNT) as usize != count
            || le32(&b, journal::COMMIT_CRC) != crc
        {
            // The crash came before the commit block: never happened.
            return Ok(sequence);
        }
        self.txn = txn;
        Ok(sequence)
    }

    // ---- Bitmap ----

    /// Block of the bitmap that holds the bit of block `n`.
    fn bitmap_block(&self, n: u64) -> Result<u64, Error> {
        if n >= self.blocks {
            return Err(Error::Invalid);
        }
        inline_block(self.sb_fork(Special::Bitmap), n / BITS_PER_BLOCK)
    }

    /// Whether block `n` is marked in use.
    pub fn block_used(&self, n: u64) -> Result<bool, Error> {
        let b = self.bitmap_block(n)?;
        let mut buf = vec![0u8; BLOCK_SIZE];
        self.read_block(b, &mut buf)?;
        Ok(buf[((n % BITS_PER_BLOCK) / 8) as usize] >> (n % 8) & 1 != 0)
    }

    /// Sets (`used`) or clears the bits of `start..start + count`; every
    /// bit must change.
    fn mark(&mut self, start: u64, count: u64, used: bool) -> Result<(), Error> {
        let mut n = start;
        let mut buf = vec![0u8; BLOCK_SIZE];
        while n < start + count {
            let b = self.bitmap_block(n)?;
            self.read_block(b, &mut buf)?;
            loop {
                let byte = &mut buf[((n % BITS_PER_BLOCK) / 8) as usize];
                let bit = 1u8 << (n % 8);
                if (*byte & bit != 0) == used {
                    return Err(Error::Corrupt);
                }
                *byte ^= bit;
                n += 1;
                if n >= start + count || n.is_multiple_of(BITS_PER_BLOCK) {
                    break;
                }
            }
            self.write_meta(b, &buf)?;
        }
        let free = self.sb_get64(sb::FREE_BLOCKS);
        self.sb_set64(
            sb::FREE_BLOCKS,
            if used { free - count } else { free + count },
        );
        Ok(())
    }

    /// Allocates up to `want` free blocks in one run (at least one),
    /// searching from `near` (wrapping round). Returns (start, count).
    pub(crate) fn alloc(&mut self, near: u64, want: u64) -> Result<(u64, u64), Error> {
        if want == 0 || !self.in_txn {
            return Err(Error::Invalid);
        }
        if self.sb_get64(sb::FREE_BLOCKS) == 0 {
            return Err(Error::NoSpace);
        }
        let near = if near >= self.blocks { 0 } else { near };
        let mut buf = vec![0u8; BLOCK_SIZE];
        for pass in 0..2 {
            let (mut n, end) = if pass == 0 {
                (near, self.blocks)
            } else {
                (0, near)
            };
            while n < end {
                let b = self.bitmap_block(n)?;
                self.read_block(b, &mut buf)?;
                let block_end = ((n / BITS_PER_BLOCK + 1) * BITS_PER_BLOCK).min(end);
                while n < block_end {
                    let byte = buf[((n % BITS_PER_BLOCK) / 8) as usize];
                    if n % 8 == 0 && byte == 0xFF && n + 8 <= block_end {
                        n += 8;
                        continue;
                    }
                    if (byte >> (n % 8)) & 1 == 0 {
                        break;
                    }
                    n += 1;
                }
                if n == block_end {
                    continue;
                }
                // n is free: the run goes on while the bits stay 0.
                let mut len = 0;
                let mut loaded = n / BITS_PER_BLOCK;
                while len < want && n + len < self.blocks {
                    let m = n + len;
                    if m / BITS_PER_BLOCK != loaded {
                        loaded = m / BITS_PER_BLOCK;
                        let b = self.bitmap_block(m)?;
                        self.read_block(b, &mut buf)?;
                    }
                    if (buf[((m % BITS_PER_BLOCK) / 8) as usize] >> (m % 8)) & 1 != 0 {
                        break;
                    }
                    len += 1;
                }
                self.mark(n, len, true)?;
                self.alloc_hint = n + len;
                return Ok((n, len));
            }
        }
        Err(Error::NoSpace)
    }

    /// Frees `start..start + count`.
    pub(crate) fn free(&mut self, start: u64, count: u64) -> Result<(), Error> {
        match start.checked_add(count) {
            Some(end) if self.in_txn && count > 0 && end <= self.blocks => {
                self.mark(start, count, false)
            }
            _ => Err(Error::Invalid),
        }
    }
}

/// The device block of logical block `index` of a fork that has only its
/// inline extents.
pub(crate) fn inline_block(fork_desc: &[u8], index: u64) -> Result<u64, Error> {
    let mut covered = 0;
    for i in 0..INLINE_EXTENTS {
        let start = le64(fork_desc, fork::EXTENT + 16 * i);
        let count = le64(fork_desc, fork::EXTENT + 16 * i + 8);
        if index < covered + count {
            return Ok(start + (index - covered));
        }
        covered += count;
    }
    Err(Error::Corrupt)
}
