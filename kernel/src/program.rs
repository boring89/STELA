// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Starting user programs: an ELF executable (read from a file through the
//! VFS; the initrd is the `Initrd` volume) is copied into a new process's
//! address space, a stack is mapped below `STACK_TOP`, its arguments are
//! queued on its main port (STELA programs) or written on its stack (Linux
//! programs, `docs/linux.md`), and the first thread starts at the entry
//! point. A program with the STELA ELF note (`abi::note`) gets the STELA
//! personality, any other the Linux one.
//!
//! ```text
//!  0x0000_0000_0040_0000   the program's PT_LOAD segments (W^X)
//!        ...
//!  0x0000_7FFF_FFDF_F000   guard page (not mapped)
//!  0x0000_7FFF_FFE0_0000   stack, 1 MiB, mapped at start
//!  0x0000_7FFF_FFF0_0000   STACK_TOP (initial RSP = STACK_TOP - 8)
//! ```

use crate::object::port::QueuedMessage;
use crate::object::tty::Tty;
use crate::object::{Entry, KernelObject};
use crate::task::{self, Personality, Process, ProcessId, SpawnError};
use crate::vfs;
use abi::layout::{STACK_GUARD, STACK_SIZE, STACK_TOP};
use abi::{Message, tag};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use elfload::{Elf, Layout};
use frame::mm::paging::MapError;
use frame::mm::pmm;
use frame::mm::user::{AddressSpace, UserFlags};

/// Why a program could not be started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// No such volume or file (or not a file).
    NotFound,
    /// Not a valid ELF executable for STELA.
    BadElf(elfload::Error),
    /// A segment is outside the program area (below the stack guard), or
    /// both writable and executable.
    BadSegment,
    /// Out of physical memory.
    OutOfMemory,
}

impl From<MapError> for LoadError {
    fn from(e: MapError) -> LoadError {
        match e {
            MapError::OutOfMemory => LoadError::OutOfMemory,
            _ => LoadError::BadSegment,
        }
    }
}

/// Maps fresh zeroed frames at `start..end` (page aligned).
fn map_zeroed(
    space: &AddressSpace,
    start: u64,
    end: u64,
    flags: UserFlags,
) -> Result<(), LoadError> {
    let mut page = start;
    while page < end {
        let frame = pmm::alloc_zeroed_frame().ok_or(LoadError::OutOfMemory)?;
        space.map(page, frame, flags)?;
        page += 4096;
    }
    Ok(())
}

/// Where a position-independent Linux program (`ET_DYN`) is loaded
/// (P13 K5, `docs/linux.md` §16; no ASLR).
pub const PIE_BASE: u64 = 0x5555_5555_0000;
/// Where the program interpreter (the dynamic linker) is loaded.
pub const INTERP_BASE: u64 = 0x7F00_0000_0000;

/// Copies the segments of `elf` into `space` and maps the stack. A page
/// that two segments share (Linux programs) gets both permissions; a page
/// that would be writable and executable is refused (W^X).
pub(crate) fn load(space: &AddressSpace, elf: &Elf) -> Result<(), LoadError> {
    load_segments(space, elf, 0)?;
    map_stack(space)
}

/// The stack below `STACK_TOP`.
fn map_stack(space: &AddressSpace) -> Result<(), LoadError> {
    let stack = UserFlags {
        writable: true,
        executable: false,
    };
    map_zeroed(space, STACK_TOP - STACK_SIZE, STACK_TOP, stack)
}

/// Copies the segments of `elf`, moved up by `bias` (0 for `ET_EXEC`).
fn load_segments(space: &AddressSpace, elf: &Elf, bias: u64) -> Result<(), LoadError> {
    let mut pages: BTreeMap<u64, UserFlags> = BTreeMap::new();
    for segment in elf.segments() {
        let start = segment
            .vaddr
            .checked_add(bias)
            .ok_or(LoadError::BadSegment)?
            & !0xFFF;
        let end = (segment.vaddr_end() + bias).next_multiple_of(4096);
        if start == 0 || end > STACK_GUARD {
            return Err(LoadError::BadSegment);
        }
        for page in (start..end).step_by(4096) {
            let flags = pages.entry(page).or_insert(UserFlags {
                writable: false,
                executable: false,
            });
            flags.writable |= segment.flags.writable();
            flags.executable |= segment.flags.executable();
            if flags.writable && flags.executable {
                return Err(LoadError::BadSegment);
            }
        }
    }
    // Zeroed frames: .bss (mem_size beyond file_size) needs nothing more.
    for (&page, &flags) in &pages {
        map_zeroed(space, page, page + 4096, flags)?;
    }
    for segment in elf.segments() {
        space
            .load(segment.vaddr + bias, elf.segment_data(&segment))
            .map_err(|_| LoadError::BadSegment)?;
    }
    Ok(())
}

/// A Linux program in memory (P13 K5): where it starts and what its
/// auxiliary vector says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinuxImage {
    /// Where the first thread starts: the interpreter's entry, or the
    /// program's.
    pub start: u64,
    /// `AT_ENTRY`: the program's entry point.
    pub entry: u64,
    /// `AT_PHDR`: the program headers in memory (0: not mapped).
    pub phdr: u64,
    /// `AT_PHNUM`, `AT_PHENT`.
    pub phnum: u64,
    /// `AT_PHENT`.
    pub phent: u64,
    /// `AT_BASE`: the interpreter's base (0: none).
    pub base: u64,
    /// The end of the program's pages (the `brk` area starts there).
    pub image_end: u64,
}

/// Loads a Linux program: an `ET_EXEC` where it says, an `ET_DYN` at
/// [`PIE_BASE`]; its `PT_INTERP` (under `/lib` or `/usr/lib`, read with
/// `interpreter`) at [`INTERP_BASE`]; and the stack.
pub(crate) fn load_linux(
    space: &AddressSpace,
    elf: &Elf,
    interpreter: impl Fn(&str) -> Option<Vec<u8>>,
) -> Result<LinuxImage, LoadError> {
    let bias = if elf.is_dynamic() { PIE_BASE } else { 0 };
    load_segments(space, elf, bias)?;
    let (phnum, phent) = elf.program_headers();
    let (_, end) = elf.image_span();
    let mut image = LinuxImage {
        start: elf.entry() + bias,
        entry: elf.entry() + bias,
        phdr: elf.program_headers_address().map_or(0, |a| a + bias),
        phnum: phnum as u64,
        phent: phent as u64,
        base: 0,
        image_end: end + bias,
    };
    if let Some(path) = elf.interpreter() {
        let bytes = interpreter(path).ok_or(LoadError::NotFound)?;
        let interp = Elf::parse_with(&bytes, Layout::Packed).map_err(LoadError::BadElf)?;
        if !interp.is_dynamic() || interp.interpreter().is_some() {
            return Err(LoadError::BadElf(elfload::Error::UnsupportedType));
        }
        load_segments(space, &interp, INTERP_BASE)?;
        image.base = INTERP_BASE;
        image.start = interp.entry() + INTERP_BASE;
    }
    map_stack(space)?;
    Ok(image)
}

/// Starts the ELF executable `image` as a new STELA process named `name`
/// (with or without the note: the kernel's own test programs have none),
/// with the standard bootstrap handles.
pub fn spawn(name: &str, image: &[u8]) -> Result<ProcessId, LoadError> {
    spawn_with(name, image, Some(Personality::Stela))
}

/// Like [`spawn`], with the given personality (`None`: from the note).
pub fn spawn_with(
    name: &str,
    image: &[u8],
    personality: Option<Personality>,
) -> Result<ProcessId, LoadError> {
    spawn_process(
        name,
        image,
        task::process::standard_initial(None),
        None,
        Vec::new(),
        personality,
        None,
    )
    .map(|p| p.id())
}

/// The personality a program gets: STELA if it carries the STELA note.
pub fn personality_of(elf: &Elf) -> Personality {
    if elf
        .notes()
        .any(|n| n.name == abi::note::NAME && n.kind == abi::note::ABI)
    {
        Personality::Stela
    } else {
        Personality::Linux
    }
}

/// Checks that `image` is an executable STELA can load (either layout).
pub fn check_image(image: &[u8]) -> Result<(), LoadError> {
    Elf::parse_with(image, Layout::Packed)
        .map(|_| ())
        .map_err(LoadError::BadElf)
}

/// The arguments in a `BOOTSTRAP_ARGS` message: NUL-separated in its data,
/// or in the `Memory` object it carries (the data is then the length).
pub fn message_args(args: &QueuedMessage) -> Vec<Vec<u8>> {
    let bytes = match args.objects.first().and_then(|e| e.object.as_memory()) {
        Some(memory) => {
            let mut len = [0u8; 8];
            let n = args.msg.payload().len().min(8);
            len[..n].copy_from_slice(&args.msg.payload()[..n]);
            let len = u64::from_le_bytes(len).min(memory.size()) as usize;
            let mut data = vec![0u8; len];
            if memory.read(0, &mut data).is_err() {
                data.clear();
            }
            data
        }
        None => args.msg.payload().to_vec(),
    };
    bytes
        .split(|&b| b == 0)
        .filter(|a| !a.is_empty())
        .map(<[u8]>::to_vec)
        .collect()
}

/// The arguments message of a program started with just its name.
pub fn name_args(name: &str) -> QueuedMessage {
    QueuedMessage::plain(Message::with_data(tag::BOOTSTRAP_ARGS, name.as_bytes()))
}

/// Starts the ELF executable `image` as a new process named `name`, with
/// `initial` as its handles `[1]`..`[3]` and `args` (default: just the
/// name) as its arguments: queued on its main port as its
/// `BOOTSTRAP_ARGS` message (STELA), or on its stack (Linux). The
/// personality is `force`, or what [`personality_of`] says.
pub fn spawn_process(
    name: &str,
    image: &[u8],
    initial: task::process::Initial,
    args: Option<QueuedMessage>,
    capabilities: Vec<(u32, Entry)>,
    force: Option<Personality>,
    program: Option<(&str, bool)>,
) -> Result<Arc<Process>, LoadError> {
    let elf = Elf::parse_with(image, Layout::Packed).map_err(LoadError::BadElf)?;
    let personality = force.unwrap_or_else(|| personality_of(&elf));
    let args = args.unwrap_or_else(|| name_args(name));
    if personality == Personality::Linux {
        // A Linux program keeps no handles for its capabilities: only
        // whether it may use the network, and (P13 K15) see the volumes.
        let has = |k: u32| capabilities.iter().any(|(kind, _)| *kind == k);
        let network = has(abi::capability::NETWORK);
        let volumes = has(abi::capability::VOLUMES);
        let jit = has(abi::capability::JIT);
        return spawn_linux(name, &elf, initial, &args, network, volumes, jit);
    }
    let process = task::process::create_with(name, initial, None)?;
    if let Some((program, system)) = program {
        process.set_program(program, system);
    }
    // On failure the process has no thread; dropping it frees what was
    // mapped and ends it with code -1.
    load(process.space(), &elf)?;
    if process.main_port().send(args).is_err() {
        return Err(LoadError::OutOfMemory);
    }
    for msg in capabilities_messages(capabilities) {
        if process.main_port().send(msg).is_err() {
            return Err(LoadError::OutOfMemory);
        }
    }
    start(&process, elf.entry(), STACK_TOP - 8)
}

/// The `CAPABILITIES` messages: the kinds in the data, the handles
/// carried; four to a message, the data of all but the last ending in
/// `capability::MORE` (P13 K15).
fn capabilities_messages(capabilities: Vec<(u32, Entry)>) -> Vec<QueuedMessage> {
    let mut messages = Vec::new();
    let mut data = Vec::new();
    let mut objects = Vec::new();
    for (kind, entry) in capabilities {
        if objects.len() == abi::MESSAGE_HANDLES {
            data.extend_from_slice(&abi::capability::MORE.to_le_bytes());
            messages.push(QueuedMessage {
                msg: Message::with_data(tag::CAPABILITIES, &data),
                objects: core::mem::take(&mut objects),
            });
            data.clear();
        }
        data.extend_from_slice(&kind.to_le_bytes());
        objects.push(entry);
    }
    messages.push(QueuedMessage {
        msg: Message::with_data(tag::CAPABILITIES, &data),
        objects,
    });
    messages
}

fn start(process: &Arc<Process>, entry: u64, rsp: u64) -> Result<Arc<Process>, LoadError> {
    match task::spawn_user(process, entry, rsp, 0) {
        Ok(_) => Ok(Arc::clone(process)),
        Err(SpawnError::OutOfMemory | SpawnError::Exiting) => Err(LoadError::OutOfMemory),
    }
}

/// Starts a Linux program: its terminal is the tty given as `[1]`, or a
/// new one that prints on the kernel console; its working directory is
/// the volume of `[3]`; the arguments go on its stack.
fn spawn_linux(
    name: &str,
    elf: &Elf,
    initial: task::process::Initial,
    args: &QueuedMessage,
    network: bool,
    volumes: bool,
    jit: bool,
) -> Result<Arc<Process>, LoadError> {
    let tty = match &initial[0] {
        Some(Entry {
            object: KernelObject::Tty(tty),
            ..
        }) => Arc::clone(tty),
        _ => Tty::new(false),
    };
    // A pipe as [1] is the output, a pipe or file as [5] the input (P11).
    let stdout = match &initial[0] {
        Some(Entry {
            object: KernelObject::PipeWrite(end),
            ..
        }) => Some(crate::linux::OpenFile::PipeWrite(
            Arc::clone(end),
            core::sync::atomic::AtomicBool::new(false),
        )),
        _ => None,
    };
    let stdin = match &initial[4] {
        Some(Entry {
            object: KernelObject::PipeRead(end),
            ..
        }) => Some(crate::linux::OpenFile::PipeRead(
            Arc::clone(end),
            core::sync::atomic::AtomicBool::new(false),
        )),
        Some(Entry {
            object: KernelObject::File(node),
            ..
        }) => Some(crate::linux::OpenFile::File {
            node: node.clone(),
            state: frame::sync::SpinLock::new(crate::linux::FileState {
                offset: 0,
                append: false,
                readable: true,
                writable: false,
                flags: 0,
            }),
        }),
        _ => None,
    };
    let folder = |e: &Option<Entry>| match e {
        Some(Entry {
            object: KernelObject::Directory(node),
            ..
        }) => Some(node.clone()),
        _ => None,
    };
    // The working directory: [3], down to its own folder (P11).
    let cwd = match folder(&initial[2]) {
        Some(node) => linux_path(&node),
        None if vfs::find("Data").is_some() => String::from("/Data"),
        None => String::from("/Initrd"),
    };
    let program_folder = folder(&initial[3]).map(|n| linux_path(&n));
    // It sees the volumes of [3] and [4] (and /tmp, /dev, /etc).
    let given: Vec<String> = [folder(&initial[2]), folder(&initial[3])]
        .into_iter()
        .flatten()
        .map(|n| linux_path(&n))
        .collect();
    let mut seen: Vec<String> = [folder(&initial[2]), folder(&initial[3])]
        .into_iter()
        .flatten()
        .map(|n| n.volume.name.clone())
        .collect();
    if seen.is_empty() {
        seen.push(String::from(&cwd[1..]));
    }
    seen.dedup();
    let mut state = crate::linux::State::with_streams(Arc::clone(&tty), stdin, stdout, cwd, 0);
    state.network = network;
    state.jit = jit;
    state.volumes = seen;
    // P13 K15 (docs/sandbox.md): without VOLUMES only the folders [3] and
    // [4] and what is below them (and /tmp, /lib, /dev, /etc, /proc).
    if !volumes {
        state.confined = Some(given);
    }
    let process = task::process::create_with(name, initial, Some(state))?;
    let image = load_linux(process.space(), elf, crate::linux::lib_image)?;
    crate::linux::set_image(&process, &image);
    let argv = message_args(args);
    let stack = crate::linux::initial_stack(&image, &argv, None, process.linux());
    process
        .space()
        .write(stack.rsp, &stack.bytes)
        .map_err(|_| LoadError::OutOfMemory)?;
    tty.add_user(process.id());
    // Its own process group and session; its main thread's id is its pid
    // (P12, docs/linux.md §11).
    let main = crate::linux::LinuxThread::new(process.id(), 0);
    if let Some(linux) = process.linux() {
        linux
            .pgid
            .store(process.id(), core::sync::atomic::Ordering::Relaxed);
        linux
            .sid
            .store(process.id(), core::sync::atomic::Ordering::Relaxed);
        // /proc/self/exe: the program's folder ([4]) and its name.
        *linux.exe.lock() = match &program_folder {
            Some(folder) => alloc::format!("{}/{name}", folder.trim_end_matches('/')),
            None => alloc::format!("/{name}"),
        };
        linux.threads.lock().push(Arc::clone(&main));
    }
    match task::spawn_user_linux(&process, image.start, stack.rsp, main) {
        Ok(_) => Ok(process),
        Err(SpawnError::OutOfMemory | SpawnError::Exiting) => Err(LoadError::OutOfMemory),
    }
}

/// The Linux path of a folder: `/Volume/a/b` (`/tmp/...` for `Tmp`).
fn linux_path(node: &vfs::Node) -> String {
    let top = if node.volume.name == "Tmp" {
        "tmp"
    } else {
        node.volume.name.as_str()
    };
    match vfs::path_of(&node.volume, node.id) {
        Ok(p) if p != "/" => alloc::format!("/{top}{p}"),
        _ => alloc::format!("/{top}"),
    }
}

/// The program file inside a bundle.
pub const BUNDLE_EXEC: &str = "exec";

/// The name of a bundle folder without its `.app` (ASCII case ignored),
/// or `None` if the folder is not a bundle.
pub fn bundle_name(folder: &str) -> Option<&str> {
    let cut = folder.len().checked_sub(4)?;
    (folder.is_char_boundary(cut) && folder[cut..].eq_ignore_ascii_case(".app") && cut > 0)
        .then(|| &folder[..cut])
}

/// Largest program file `read_program` takes.
pub const PROGRAM_MAX: u64 = 32 << 20;

/// The contents and name of file `id` on `volume` (a program to start).
pub fn read_program(volume: &vfs::Volume, id: u64) -> Result<(String, Vec<u8>), LoadError> {
    let info = volume.fs.get(id).map_err(|_| LoadError::NotFound)?;
    if info.is_dir || info.size > PROGRAM_MAX {
        return Err(LoadError::NotFound);
    }
    let mut image = vec![0u8; info.size as usize];
    let n = volume
        .fs
        .read(id, 0, &mut image)
        .map_err(|_| LoadError::NotFound)?;
    image.truncate(n);
    Ok((info.name, image))
}

/// Starts the program at `path` (relative to the root) on the volume
/// called `volume`, with the standard handles and `bootstrap` as `[2]`.
/// Programs are always read through the VFS (the initrd is the `Initrd`
/// volume).
pub fn spawn_path(
    volume: &str,
    path: &str,
    bootstrap: Option<Entry>,
) -> Result<Arc<Process>, LoadError> {
    spawn_path_with(volume, path, bootstrap, Vec::new())
}

/// Like [`spawn_path`], with `capabilities` in its `CAPABILITIES`
/// message.
pub fn spawn_path_with(
    volume: &str,
    path: &str,
    bootstrap: Option<Entry>,
    capabilities: Vec<(u32, Entry)>,
) -> Result<Arc<Process>, LoadError> {
    let volume = vfs::find(volume).ok_or(LoadError::NotFound)?;
    let mut id = volume.fs.root();
    for part in path.split('/').filter(|p| !p.is_empty()) {
        id = volume
            .fs
            .lookup(id, part)
            .map_err(|_| LoadError::NotFound)?
            .id;
    }
    let (name, image) = read_program(&volume, id)?;
    let system = is_system_volume(&volume);
    spawn_process(
        &name,
        &image,
        task::process::standard_initial(bootstrap),
        None,
        capabilities,
        None,
        Some((&name, system)),
    )
}

/// Whether programs read from `volume` are the system's own (P14 K3: the
/// read-only `Initrd`; K4: and `System`).
pub fn is_system_volume(volume: &vfs::Volume) -> bool {
    volume.name == "Initrd" || volume.name == vfs::mount::SYSTEM
}
