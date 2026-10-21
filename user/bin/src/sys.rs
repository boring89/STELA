// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The tools' side of STELA: arguments, paths from the working folder
//! (`[3]`), files, the input (`[5]` or the files named), errors.

use alloc::string::String;
use alloc::vec::Vec;
use libstela::fs::{self, open};
use libstela::ipc::handle_close;
use libstela::path::{self, Location};
use libstela::{Error, Handle, bootstrap, io, process};

/// A handle closed when dropped.
pub struct Owned(pub Handle);

impl Drop for Owned {
    fn drop(&mut self) {
        let _ = handle_close(self.0);
    }
}

/// The arguments after the program's name.
pub fn args() -> Vec<String> {
    process::args().into_iter().skip(1).collect()
}

/// An error's words.
pub fn describe(e: Error) -> &'static str {
    match e {
        Error::NotFound => "not found",
        Error::AccessDenied => "not allowed",
        Error::ReadOnly => "read-only volume",
        Error::IsDir => "is a folder",
        Error::NotDir => "not a folder",
        Error::Exists => "already exists",
        e => e.name(),
    }
}

/// The working folder (`[3]`) as a place.
pub fn cwd() -> Location {
    let volume = fs::stat(bootstrap::CWD)
        .ok()
        .and_then(|st| fs::volume_by_index(st.volume))
        .map_or_else(|| String::from("Data"), |v| String::from(v.name()));
    let mut loc = Location::root(&volume);
    if let Ok(p) = fs::path_of(bootstrap::CWD) {
        loc.parts = p
            .split('/')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
    }
    loc
}

/// Where `input` leads from the working folder.
pub fn resolve(input: &str) -> Result<Location, String> {
    path::resolve(&cwd(), input).map_err(|_| alloc::format!("{input}: not a path"))
}

/// Opens a place with `flags`.
pub fn open_at(loc: &Location, flags: u64) -> Result<Owned, Error> {
    let root = Owned(fs::volume_open(&loc.volume)?);
    if loc.parts.is_empty() && flags & open::CREATE == 0 {
        return Ok(root);
    }
    fs::open(root.0, &loc.relative(), flags).map(Owned)
}

/// Opens `input` (a path as typed) with `flags`; the error names it.
pub fn open_path(input: &str, flags: u64) -> Result<(Owned, Location), String> {
    let loc = resolve(input)?;
    let h = open_at(&loc, flags).map_err(|e| alloc::format!("{loc}: {}", describe(e)))?;
    Ok((h, loc))
}

/// Everything in file `h`.
pub fn read_all(h: Handle) -> Result<Vec<u8>, Error> {
    let size = fs::stat(h)?.size as usize;
    let mut data = alloc::vec![0u8; size];
    let mut done = 0;
    while done < size {
        match fs::read(h, &mut data[done..], done as u64)? {
            0 => break,
            n => done += n,
        }
    }
    data.truncate(done);
    Ok(data)
}

/// Writes all of `data` at `offset`.
pub fn write_all(h: Handle, data: &[u8], offset: u64) -> Result<(), Error> {
    let mut done = 0;
    while done < data.len() {
        match fs::write(h, &data[done..], offset + done as u64)? {
            0 => return Err(Error::IoError),
            n => done += n,
        }
    }
    Ok(())
}

/// The file at `input`, whole.
pub fn read_file(input: &str) -> Result<Vec<u8>, String> {
    let (h, loc) = open_path(input, open::READ)?;
    read_all(h.0).map_err(|e| alloc::format!("{loc}: {}", describe(e)))
}

/// The tool's input: the files named (one after another), else `[5]`.
pub fn read_input(files: &[String]) -> Result<Vec<u8>, String> {
    if !files.is_empty() {
        let mut all = Vec::new();
        for f in files {
            all.extend(read_file(f)?);
        }
        return Ok(all);
    }
    if !io::has_stdin() {
        return Err(String::from(
            "no input (name a file, or use it after | or with <)",
        ));
    }
    let mut all = Vec::new();
    let mut buf = alloc::vec![0u8; 4096];
    loop {
        match io::read(&mut buf) {
            Ok(0) => break,
            Ok(n) => all.extend_from_slice(&buf[..n]),
            Err(e) => return Err(String::from(describe(e))),
        }
    }
    Ok(all)
}

/// The input as text (bad UTF-8 replaced).
pub fn read_text(files: &[String]) -> Result<String, String> {
    read_input(files).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Prints `tool: message` and gives the exit code 1.
pub fn fail(tool: &str, message: &str) -> i64 {
    libstela::println!("{tool}: {message}");
    1
}
