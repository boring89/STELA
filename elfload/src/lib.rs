// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! A minimal ELF64 reader: just enough to load the STELA kernel and
//! statically linked programs.
//!
//! Only the file header, the program headers of type `PT_LOAD` and the
//! notes of `PT_NOTE` segments are read; sections, symbols and relocations
//! are ignored (everything is a statically linked `ET_EXEC` at a fixed
//! address; P13 K5: with [`Layout::Packed`], also `ET_DYN` — a
//! position-independent program or a shared library, which the loader
//! places at a base of its choosing — and the `PT_INTERP` path). Field offsets follow the "ELF-64
//! Object File Format" version 1.5 and the System V ABI AMD64 Architecture
//! Processor Supplement (`EM_X86_64` = 62).
//!
//! All checks happen in [`Elf::parse`]; once it succeeds every segment's file
//! bytes lie inside the input and no address computation overflows.

#![no_std]
#![forbid(unsafe_code)]

/// Size of a page; segments must start on a page boundary.
pub const PAGE_SIZE: u64 = 4096;

// e_ident (ELF-64 spec, Table 2 "ELF Identification").
const ELFMAG: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const EV_CURRENT: u8 = 1;

/// `e_type`: executable file.
const ET_EXEC: u16 = 2;
/// `e_type`: shared object (a PIE program or a library; P13 K5).
const ET_DYN: u16 = 3;
/// `e_machine`: AMD x86-64.
const EM_X86_64: u16 = 62;

/// Size of the file header (`Elf64_Ehdr`).
const EHDR_SIZE: usize = 64;
/// Size of one program header (`Elf64_Phdr`).
const PHDR_SIZE: usize = 56;

/// `p_type`: loadable segment.
const PT_LOAD: u32 = 1;
/// `p_type`: the program interpreter's path (P13 K5).
const PT_INTERP: u32 = 3;
/// `p_type`: notes (System V ABI, "Note Section").
const PT_NOTE: u32 = 4;

/// Why a file was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Shorter than its headers claim.
    Truncated,
    /// Does not start with `\x7FELF`.
    BadMagic,
    /// Not a 64-bit, little-endian, version 1 file.
    UnsupportedFormat,
    /// Not an `ET_EXEC` for x86-64 (or `ET_DYN` with `Layout::Packed`).
    UnsupportedType,
    /// A program header is malformed (sizes, alignment, overflow, overlap).
    BadSegment,
    /// There is no `PT_LOAD` segment.
    NoSegments,
    /// The entry point is not inside an executable segment.
    BadEntry,
}

/// Segment permission bits (`p_flags`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flags(pub u32);

impl Flags {
    /// `PF_X`: executable.
    pub const EXECUTE: u32 = 1;
    /// `PF_W`: writable.
    pub const WRITE: u32 = 2;
    /// `PF_R`: readable.
    pub const READ: u32 = 4;

    /// Whether the segment may be executed.
    pub const fn executable(self) -> bool {
        self.0 & Flags::EXECUTE != 0
    }

    /// Whether the segment may be written.
    pub const fn writable(self) -> bool {
        self.0 & Flags::WRITE != 0
    }
}

/// One `PT_LOAD` segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    /// Virtual address of the first byte; page aligned.
    pub vaddr: u64,
    /// Size in memory; the bytes after `file_size` are zero (`.bss`).
    pub mem_size: u64,
    /// Offset of the segment's bytes in the file.
    pub file_offset: u64,
    /// Number of bytes stored in the file.
    pub file_size: u64,
    /// Permissions.
    pub flags: Flags,
}

impl Segment {
    /// One past the last virtual address of the segment.
    pub const fn vaddr_end(&self) -> u64 {
        self.vaddr + self.mem_size
    }
}

/// A validated ELF64 executable.
#[derive(Clone, Copy, Debug)]
pub struct Elf<'a> {
    data: &'a [u8],
    dynamic: bool,
    entry: u64,
    phoff: usize,
    phnum: usize,
    phentsize: usize,
}

/// How strictly [`Elf::parse_with`] checks the `PT_LOAD` segments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Every segment starts on a page and no two share a page (the kernel
    /// and STELA programs, linked so that each page has one permission).
    PageAligned,
    /// Segments may start anywhere and share a page with the previous one
    /// (what GNU ld makes for ordinary Linux executables); they must still
    /// be in ascending order without overlapping bytes. The loader gives
    /// a shared page the union of the permissions.
    Packed,
}

/// One note of a `PT_NOTE` segment (System V ABI "Note Section"): a
/// 12-byte header (namesz, descsz, type), the name, the descriptor, each
/// padded to 4 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Note<'a> {
    /// The owner's name without the terminating NUL (`b"GNU"`, `b"STELA"`).
    pub name: &'a [u8],
    /// The note type.
    pub kind: u32,
    /// The descriptor bytes.
    pub desc: &'a [u8],
}

/// The notes in `data` (the bytes of a `PT_NOTE` segment); stops at the
/// first malformed one.
pub fn notes(data: &[u8]) -> impl Iterator<Item = Note<'_>> {
    let mut at = 0usize;
    core::iter::from_fn(move || {
        let namesz = read_u32(data, at).ok()? as usize;
        let descsz = read_u32(data, at + 4).ok()? as usize;
        let kind = read_u32(data, at + 8).ok()?;
        let name_at = at + 12;
        let desc_at = name_at.checked_add(namesz.next_multiple_of(4))?;
        let end = desc_at.checked_add(descsz.next_multiple_of(4))?;
        let name = data.get(name_at..name_at + namesz)?;
        let desc = data.get(desc_at..desc_at + descsz)?;
        at = end;
        let name = name.strip_suffix(&[0]).unwrap_or(name);
        Some(Note { name, kind, desc })
    })
}

fn read_u16(data: &[u8], at: usize) -> Result<u16, Error> {
    let bytes = data.get(at..at + 2).ok_or(Error::Truncated)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], at: usize) -> Result<u32, Error> {
    let bytes = data.get(at..at + 4).ok_or(Error::Truncated)?;
    let mut buf = [0; 4];
    buf.copy_from_slice(bytes);
    Ok(u32::from_le_bytes(buf))
}

fn read_u64(data: &[u8], at: usize) -> Result<u64, Error> {
    let bytes = data.get(at..at + 8).ok_or(Error::Truncated)?;
    let mut buf = [0; 8];
    buf.copy_from_slice(bytes);
    Ok(u64::from_le_bytes(buf))
}

impl<'a> Elf<'a> {
    /// Checks the headers and every `PT_LOAD` segment.
    ///
    /// Requirements beyond a well-formed file: segments start on a page
    /// boundary, appear in ascending address order and do not share a page,
    /// and the entry point lies in an executable segment.
    pub fn parse(data: &'a [u8]) -> Result<Elf<'a>, Error> {
        Elf::parse_with(data, Layout::PageAligned)
    }

    /// Like [`Elf::parse`], with the segment rules of `layout`.
    pub fn parse_with(data: &'a [u8], layout: Layout) -> Result<Elf<'a>, Error> {
        let ident = data.get(..16).ok_or(Error::Truncated)?;
        if ident[..4] != ELFMAG {
            return Err(Error::BadMagic);
        }
        if ident[4] != ELFCLASS64 || ident[5] != ELFDATA2LSB || ident[6] != EV_CURRENT {
            return Err(Error::UnsupportedFormat);
        }
        if data.len() < EHDR_SIZE {
            return Err(Error::Truncated);
        }
        // Elf64_Ehdr: e_type @16, e_machine @18, e_entry @24, e_phoff @32,
        // e_phentsize @54, e_phnum @56.
        let dynamic = match read_u16(data, 16)? {
            ET_EXEC => false,
            ET_DYN if layout == Layout::Packed => true,
            _ => return Err(Error::UnsupportedType),
        };
        if read_u16(data, 18)? != EM_X86_64 {
            return Err(Error::UnsupportedType);
        }
        let entry = read_u64(data, 24)?;
        let phoff = usize::try_from(read_u64(data, 32)?).map_err(|_| Error::Truncated)?;
        let phentsize = usize::from(read_u16(data, 54)?);
        let phnum = usize::from(read_u16(data, 56)?);
        if phentsize < PHDR_SIZE {
            return Err(Error::BadSegment);
        }
        let table_end = phentsize
            .checked_mul(phnum)
            .and_then(|size| size.checked_add(phoff))
            .ok_or(Error::Truncated)?;
        if table_end > data.len() {
            return Err(Error::Truncated);
        }

        let elf = Elf {
            data,
            dynamic,
            entry,
            phoff,
            phnum,
            phentsize,
        };
        elf.validate_segments(layout)?;
        Ok(elf)
    }

    fn validate_segments(&self, layout: Layout) -> Result<(), Error> {
        let mut previous_end: Option<u64> = None;
        let mut entry_ok = false;
        for index in 0..self.phnum {
            let Some(segment) = self.segment_at(index)? else {
                continue;
            };
            let file_end = segment
                .file_offset
                .checked_add(segment.file_size)
                .ok_or(Error::BadSegment)?;
            if file_end > self.data.len() as u64
                || segment.file_size > segment.mem_size
                || segment.mem_size == 0
                || (layout == Layout::PageAligned && segment.vaddr % PAGE_SIZE != 0)
                || segment
                    .vaddr
                    .checked_add(segment.mem_size)
                    .is_none_or(|end| end.checked_next_multiple_of(PAGE_SIZE).is_none())
            {
                return Err(Error::BadSegment);
            }
            if let Some(end) = previous_end {
                let limit = match layout {
                    Layout::PageAligned => end.next_multiple_of(PAGE_SIZE),
                    Layout::Packed => end,
                };
                if segment.vaddr < limit {
                    return Err(Error::BadSegment);
                }
            }
            previous_end = Some(segment.vaddr_end());
            if segment.flags.executable()
                && (segment.vaddr..segment.vaddr_end()).contains(&self.entry)
            {
                entry_ok = true;
            }
        }
        if previous_end.is_none() {
            return Err(Error::NoSegments);
        }
        if !entry_ok {
            return Err(Error::BadEntry);
        }
        Ok(())
    }

    /// Reads program header `index`; `None` if it is not `PT_LOAD`.
    fn segment_at(&self, index: usize) -> Result<Option<Segment>, Error> {
        let at = self.phoff + index * self.phentsize;
        // Elf64_Phdr: p_type @0, p_flags @4, p_offset @8, p_vaddr @16,
        // p_filesz @32, p_memsz @40.
        if read_u32(self.data, at)? != PT_LOAD {
            return Ok(None);
        }
        Ok(Some(Segment {
            flags: Flags(read_u32(self.data, at + 4)?),
            file_offset: read_u64(self.data, at + 8)?,
            vaddr: read_u64(self.data, at + 16)?,
            file_size: read_u64(self.data, at + 32)?,
            mem_size: read_u64(self.data, at + 40)?,
        }))
    }

    /// The entry point's virtual address (relative to the load base for
    /// an `ET_DYN` file).
    pub const fn entry(&self) -> u64 {
        self.entry
    }

    /// Whether it is `ET_DYN` (placed at a base the loader chooses; its
    /// addresses are offsets from that base).
    pub const fn is_dynamic(&self) -> bool {
        self.dynamic
    }

    /// The program interpreter (`PT_INTERP`, without the NULs), if any and
    /// valid UTF-8.
    pub fn interpreter(&self) -> Option<&'a str> {
        (0..self.phnum).find_map(|index| {
            let at = self.phoff + index * self.phentsize;
            if read_u32(self.data, at).ok()? != PT_INTERP {
                return None;
            }
            let offset = usize::try_from(read_u64(self.data, at + 8).ok()?).ok()?;
            let size = usize::try_from(read_u64(self.data, at + 32).ok()?).ok()?;
            let bytes = self.data.get(offset..offset.checked_add(size)?)?;
            core::str::from_utf8(bytes)
                .ok()
                .map(|p| p.trim_end_matches('\0'))
        })
    }

    /// Number of program headers and the size of each (for the auxiliary
    /// vector of a Linux program: AT_PHNUM, AT_PHENT).
    pub const fn program_headers(&self) -> (usize, usize) {
        (self.phnum, self.phentsize)
    }

    /// The virtual address of the program header table, when a `PT_LOAD`
    /// segment's file bytes contain it (AT_PHDR).
    pub fn program_headers_address(&self) -> Option<u64> {
        let start = self.phoff as u64;
        let end = start + (self.phnum * self.phentsize) as u64;
        self.segments().find_map(|s| {
            (s.file_offset <= start && end <= s.file_offset + s.file_size)
                .then(|| s.vaddr + (start - s.file_offset))
        })
    }

    /// The notes of every `PT_NOTE` segment.
    pub fn notes(&self) -> impl Iterator<Item = Note<'a>> + '_ {
        (0..self.phnum).flat_map(move |index| {
            let at = self.phoff + index * self.phentsize;
            let bytes = (read_u32(self.data, at) == Ok(PT_NOTE))
                .then(|| {
                    let offset = read_u64(self.data, at + 8).ok()? as usize;
                    let size = read_u64(self.data, at + 32).ok()? as usize;
                    self.data.get(offset..offset.checked_add(size)?)
                })
                .flatten()
                .unwrap_or(&[]);
            notes(bytes)
        })
    }

    /// The `PT_LOAD` segments in file order (which [`Elf::parse`] checked to
    /// be ascending by address).
    pub fn segments(&self) -> impl Iterator<Item = Segment> + '_ {
        (0..self.phnum).filter_map(|index| self.segment_at(index).ok().flatten())
    }

    /// The bytes of `segment` stored in the file (`file_size` long).
    pub fn segment_data(&self, segment: &Segment) -> &'a [u8] {
        // parse() checked that the range lies inside the file.
        let start = segment.file_offset as usize;
        let end = start + segment.file_size as usize;
        self.data.get(start..end).unwrap_or(&[])
    }

    /// The page-aligned virtual range covering all segments:
    /// `(first address, one past the last)`.
    pub fn image_span(&self) -> (u64, u64) {
        let mut start = u64::MAX;
        let mut end = 0;
        for segment in self.segments() {
            start = start.min(segment.vaddr);
            end = end.max(segment.vaddr_end());
        }
        (start, end.next_multiple_of(PAGE_SIZE))
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    const BASE: u64 = 0xFFFF_FFFF_8000_0000;

    struct Phdr {
        p_type: u32,
        flags: u32,
        offset: u64,
        vaddr: u64,
        filesz: u64,
        memsz: u64,
    }

    fn load(flags: u32, offset: u64, vaddr: u64, filesz: u64, memsz: u64) -> Phdr {
        Phdr {
            p_type: PT_LOAD,
            flags,
            offset,
            vaddr,
            filesz,
            memsz,
        }
    }

    /// Builds an ELF file with the given program headers, padded to `len`
    /// bytes; byte `i` of the payload area is `i as u8`.
    fn build(entry: u64, phdrs: &[Phdr], len: usize) -> Vec<u8> {
        let mut f: Vec<u8> = (0..len).map(|i| i as u8).collect();
        f[..64].fill(0);
        f[..4].copy_from_slice(&ELFMAG);
        f[4] = ELFCLASS64;
        f[5] = ELFDATA2LSB;
        f[6] = EV_CURRENT;
        f[16..18].copy_from_slice(&ET_EXEC.to_le_bytes());
        f[18..20].copy_from_slice(&EM_X86_64.to_le_bytes());
        f[20..24].copy_from_slice(&1u32.to_le_bytes());
        f[24..32].copy_from_slice(&entry.to_le_bytes());
        f[32..40].copy_from_slice(&64u64.to_le_bytes());
        f[52..54].copy_from_slice(&64u16.to_le_bytes());
        f[54..56].copy_from_slice(&(PHDR_SIZE as u16).to_le_bytes());
        f[56..58].copy_from_slice(&(phdrs.len() as u16).to_le_bytes());
        for (i, p) in phdrs.iter().enumerate() {
            let at = 64 + i * PHDR_SIZE;
            let h = &mut f[at..at + PHDR_SIZE];
            h.fill(0);
            h[0..4].copy_from_slice(&p.p_type.to_le_bytes());
            h[4..8].copy_from_slice(&p.flags.to_le_bytes());
            h[8..16].copy_from_slice(&p.offset.to_le_bytes());
            h[16..24].copy_from_slice(&p.vaddr.to_le_bytes());
            h[24..32].copy_from_slice(&p.vaddr.to_le_bytes());
            h[32..40].copy_from_slice(&p.filesz.to_le_bytes());
            h[40..48].copy_from_slice(&p.memsz.to_le_bytes());
            h[48..56].copy_from_slice(&PAGE_SIZE.to_le_bytes());
        }
        f
    }

    fn kernel_like() -> Vec<u8> {
        build(
            BASE + 0x10,
            &[
                load(5, 0x1000, BASE, 0x900, 0x900),
                Phdr {
                    p_type: 4, // PT_NOTE: ignored
                    flags: 4,
                    offset: 0,
                    vaddr: 0,
                    filesz: 0,
                    memsz: 0,
                },
                load(4, 0x2000, BASE + 0x1000, 0x100, 0x100),
                load(6, 0x3000, BASE + 0x2000, 0x10, 0x5000),
            ],
            0x3010,
        )
    }

    #[test]
    fn parses_kernel_like_file() {
        let file = kernel_like();
        let elf = Elf::parse(&file).unwrap();
        assert_eq!(elf.entry(), BASE + 0x10);
        let segments: Vec<Segment> = elf.segments().collect();
        assert_eq!(segments.len(), 3);
        assert!(segments[0].flags.executable() && !segments[0].flags.writable());
        assert!(!segments[1].flags.executable() && !segments[1].flags.writable());
        assert!(segments[2].flags.writable() && !segments[2].flags.executable());
        assert_eq!(elf.segment_data(&segments[1]).len(), 0x100);
        assert_eq!(elf.segment_data(&segments[1])[1], 0x01);
        assert_eq!(elf.image_span(), (BASE, BASE + 0x7000));
    }

    #[test]
    fn rejects_bad_identification() {
        let mut file = kernel_like();
        file[0] = 0;
        assert_eq!(Elf::parse(&file).unwrap_err(), Error::BadMagic);
        let mut file = kernel_like();
        file[4] = 1; // 32-bit
        assert_eq!(Elf::parse(&file).unwrap_err(), Error::UnsupportedFormat);
        let mut file = kernel_like();
        file[18] = 3; // EM_386
        assert_eq!(Elf::parse(&file).unwrap_err(), Error::UnsupportedType);
        assert_eq!(Elf::parse(&[0x7F, b'E']).unwrap_err(), Error::Truncated);
    }

    #[test]
    fn rejects_truncated_segment() {
        let file = build(BASE, &[load(5, 0x1000, BASE, 0x900, 0x900)], 0x1800);
        assert_eq!(Elf::parse(&file).unwrap_err(), Error::BadSegment);
    }

    #[test]
    fn rejects_truncated_program_headers() {
        let mut file = kernel_like();
        file[56..58].copy_from_slice(&1000u16.to_le_bytes());
        file.truncate(0x400);
        assert_eq!(Elf::parse(&file).unwrap_err(), Error::Truncated);
    }

    #[test]
    fn rejects_unaligned_overlapping_or_unordered_segments() {
        let unaligned = build(BASE + 8, &[load(5, 0x100, BASE + 8, 0x10, 0x10)], 0x200);
        assert_eq!(Elf::parse(&unaligned).unwrap_err(), Error::BadSegment);
        let shared_page = build(
            BASE,
            &[
                load(5, 0x100, BASE, 0x10, 0x10),
                load(6, 0x100, BASE + 0x1000, 0x10, 0x10),
                load(4, 0x100, BASE + 0x1000, 0x10, 0x10),
            ],
            0x400,
        );
        assert_eq!(Elf::parse(&shared_page).unwrap_err(), Error::BadSegment);
        let unordered = build(
            BASE,
            &[
                load(5, 0x100, BASE, 0x10, 0x10),
                load(6, 0x100, BASE - 0x1000, 0x10, 0x10),
            ],
            0x400,
        );
        assert_eq!(Elf::parse(&unordered).unwrap_err(), Error::BadSegment);
        let filesz_too_big = build(BASE, &[load(5, 0x100, BASE, 0x20, 0x10)], 0x400);
        assert_eq!(Elf::parse(&filesz_too_big).unwrap_err(), Error::BadSegment);
        let wraps = build(BASE, &[load(5, 0x100, BASE, 0x10, u64::MAX)], 0x400);
        assert_eq!(Elf::parse(&wraps).unwrap_err(), Error::BadSegment);
    }

    #[test]
    fn packed_layout_allows_shared_pages() {
        // Text at 0x401000, data starting in the middle of the next page
        // and sharing it with rodata: what GNU ld makes for Linux.
        let phdrs = [
            load(5, 0x1000, 0x40_1000, 0x800, 0x800),
            load(4, 0x2000, 0x40_2000, 0x100, 0x100),
            load(6, 0x2e10, 0x40_2e10, 0x100, 0x400),
        ];
        let file = build(0x40_1000, &phdrs, 0x3000);
        assert_eq!(Elf::parse(&file).err(), Some(Error::BadSegment));
        let elf = Elf::parse_with(&file, Layout::Packed).unwrap();
        assert_eq!(elf.segments().count(), 3);
        // Overlapping bytes are still refused.
        let phdrs = [
            load(5, 0x1000, 0x40_1000, 0x800, 0x800),
            load(6, 0x1000, 0x40_17ff, 0x10, 0x10),
        ];
        let file = build(0x40_1000, &phdrs, 0x3000);
        assert_eq!(
            Elf::parse_with(&file, Layout::Packed).err(),
            Some(Error::BadSegment)
        );
    }

    #[test]
    fn notes_and_program_header_address() {
        let note_at = 0x200usize;
        let mut note = Vec::new();
        for (name, kind, desc) in [
            (&b"GNU\0"[..], 3u32, &[1u8, 2, 3][..]),
            (b"STELA\0", 1, &[1, 0, 0, 0]),
        ] {
            note.extend_from_slice(&(name.len() as u32).to_le_bytes());
            note.extend_from_slice(&(desc.len() as u32).to_le_bytes());
            note.extend_from_slice(&kind.to_le_bytes());
            note.extend_from_slice(name);
            note.resize(note.len().next_multiple_of(4), 0);
            note.extend_from_slice(desc);
            note.resize(note.len().next_multiple_of(4), 0);
        }
        let phdrs = [
            load(5, 0, 0x40_0000, 0x1000, 0x1000),
            Phdr {
                p_type: PT_NOTE,
                flags: 4,
                offset: note_at as u64,
                vaddr: 0x40_0200,
                filesz: note.len() as u64,
                memsz: note.len() as u64,
            },
        ];
        let mut file = build(0x40_0100, &phdrs, 0x1000);
        file[note_at..note_at + note.len()].copy_from_slice(&note);
        let elf = Elf::parse(&file).unwrap();
        let found: Vec<_> = elf.notes().collect();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, b"GNU");
        assert_eq!((found[0].kind, found[0].desc), (3, &[1u8, 2, 3][..]));
        assert_eq!(found[1].name, b"STELA");
        assert_eq!(elf.program_headers(), (2, PHDR_SIZE));
        assert_eq!(elf.program_headers_address(), Some(0x40_0040));
        // Truncated notes stop the iteration.
        assert_eq!(notes(&note[..10]).count(), 0);
        assert_eq!(notes(&note[..note.len() - 4]).count(), 1);
    }

    #[test]
    fn rejects_entry_outside_code() {
        let data_entry = build(
            BASE + 0x1000,
            &[
                load(5, 0x100, BASE, 0x10, 0x10),
                load(6, 0x100, BASE + 0x1000, 0x10, 0x10),
            ],
            0x400,
        );
        assert_eq!(Elf::parse(&data_entry).unwrap_err(), Error::BadEntry);
        let none = build(BASE, &[], 0x400);
        assert_eq!(Elf::parse(&none).unwrap_err(), Error::NoSegments);
    }

    /// Parses the real kernel if `make` has built it; skipped otherwise.
    #[test]
    fn parses_built_kernel_if_present() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../build/target/x86_64-unknown-none/release/kernel"
        );
        let Ok(file) = std::fs::read(path) else {
            return;
        };
        let elf = Elf::parse(&file).unwrap();
        let (start, _) = elf.image_span();
        assert_eq!(start, BASE);
        assert!(
            elf.segments()
                .all(|s| !(s.flags.writable() && s.flags.executable()))
        );
    }

    #[test]
    fn dynamic_programs_and_their_interpreter() {
        // A PIE: ET_DYN at vaddr 0, an interpreter path in the file.
        let mut f = build(
            0x1010,
            &[
                Phdr {
                    p_type: PT_INTERP,
                    flags: 4,
                    offset: 0x200,
                    vaddr: 0x200,
                    filesz: 28,
                    memsz: 28,
                },
                load(4, 0, 0, 0x300, 0x300),
                load(5, 0x1000, 0x1000, 0x100, 0x100),
            ],
            0x1100,
        );
        f[16..18].copy_from_slice(&ET_DYN.to_le_bytes());
        f[0x200..0x21C].copy_from_slice(b"/lib/ld-musl-x86_64.so.1\0\0\0\0");
        let elf = Elf::parse_with(&f, Layout::Packed).expect("an ET_DYN with Packed");
        assert!(elf.is_dynamic());
        assert_eq!(elf.interpreter(), Some("/lib/ld-musl-x86_64.so.1"));
        assert_eq!(elf.image_span(), (0, 0x2000));
        // Not with the strict layout (the kernel, STELA programs).
        assert_eq!(Elf::parse(&f).err(), Some(Error::UnsupportedType));
        // A static program has no interpreter.
        let s = kernel_like();
        assert!(!Elf::parse(&s).expect("static").is_dynamic());
        assert_eq!(Elf::parse(&s).expect("static").interpreter(), None);
    }
}
