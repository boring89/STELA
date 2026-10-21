// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! fstest: exercises the file system calls on the `Data` volume (its
//! working directory, bootstrap handle `[3]`) and reads the `Boot` volume.
//! Counts its runs in `Data:/fstest-boots` to show that writes survive a
//! restart: prints `fstest: boot N`. Exits with 0 after `fstest: OK`, or
//! with the number of the failed check.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use libstela::fs::{self, open};
use libstela::{Error, Handle, bootstrap, debug_println};

libstela::entry!(main);

fn check(step: i64, ok: bool, what: &str) -> Result<(), i64> {
    if ok {
        Ok(())
    } else {
        debug_println!("fstest: {what} FAILED");
        Err(step)
    }
}

fn pattern(i: usize) -> u8 {
    (i * 7 + i / 4096) as u8
}

fn run() -> Result<(), i64> {
    let data = bootstrap::CWD;
    // Volumes.
    let mut names = [[0u8; 64]; 8];
    let mut count = 0;
    while let Ok(Some(v)) = fs::volume_list(count as u64) {
        debug_println!(
            "fstest: volume {} {} {} MiB {}",
            v.index,
            v.name(),
            v.total >> 20,
            if v.flags & 1 != 0 { "ro" } else { "rw" }
        );
        if count < names.len() {
            names[count] = v.name;
        }
        count += 1;
    }
    check(1, count >= 1, "volume list")?;

    // The boot counter.
    let counter = fs::open(
        data,
        "fstest-boots",
        open::READ | open::WRITE | open::CREATE,
    )
    .map_err(|_| 2i64)?;
    let mut b = [0u8; 8];
    let n = fs::read(counter, &mut b, 0).map_err(|_| 2i64)?;
    let boots = if n == 8 { u64::from_le_bytes(b) + 1 } else { 1 };
    fs::write(counter, &boots.to_le_bytes(), 0).map_err(|_| 2i64)?;
    debug_println!("fstest: boot {boots}");

    // A folder and a file in it, written across blocks and read back.
    let _ = remove_tree(data);
    let dir = fs::open(
        data,
        "fstest",
        open::READ | open::WRITE | open::CREATE | open::DIRECTORY | open::EXCLUSIVE,
    )
    .map_err(|_| 3i64)?;
    let file =
        fs::open(dir, "hello.txt", open::READ | open::WRITE | open::CREATE).map_err(|_| 4i64)?;
    let mut buf = [0u8; 10_000];
    for (i, x) in buf.iter_mut().enumerate() {
        *x = pattern(i);
    }
    check(5, fs::write(file, &buf, 0) == Ok(buf.len()), "write")?;
    let mut back = [0u8; 10_000];
    check(6, fs::read(file, &mut back, 0) == Ok(back.len()), "read")?;
    check(7, back == buf, "read back")?;
    // Finder information (FileGetInfo/FileSetInfo): zero at first, then
    // what was stored.
    check(30, fs::get_info(file) == Ok([0; 32]), "fresh Finder info")?;
    let mut info = [0u8; 32];
    info[..6].copy_from_slice(b"MOUSEI");
    info[31] = 0xA5;
    check(31, fs::set_info(file, &info).is_ok(), "set Finder info")?;
    check(32, fs::get_info(file) == Ok(info), "Finder info back")?;
    check(33, fs::set_info(dir, &info).is_ok(), "folder Finder info")?;
    // Extended attributes (P11).
    check(
        38,
        fs::get_attr(file, "stela.test") == Err(Error::NotFound),
        "no attribute yet",
    )?;
    check(
        38,
        fs::set_attr(file, "stela.test", b"value one").is_ok(),
        "set an attribute",
    )?;
    check(
        38,
        fs::set_attr(dir, "stela.dir", &[7u8; 3000]).is_ok(),
        "a big one",
    )?;
    let got = fs::get_attr(file, "stela.test").map_err(|_| 38i64)?;
    check(38, got.as_slice() == b"value one", "attribute back")?;
    check(
        38,
        fs::get_attr(dir, "stela.dir").map(|v| v.len()) == Ok(3000),
        "big one back",
    )?;
    check(
        38,
        fs::list_attrs(file).map(|l| l.len() == 1 && l[0] == "stela.test") == Ok(true),
        "list",
    )?;
    check(
        38,
        fs::set_attr(file, "x", &[0u8; 4000]) == Err(Error::InvalidArgument),
        "too big",
    )?;
    check(38, fs::set_attr(file, "stela.test", &[]).is_ok(), "remove")?;
    check(
        38,
        fs::get_attr(file, "stela.test") == Err(Error::NotFound),
        "removed",
    )?;
    // Paths from the parents (FilePath, P11).
    check(
        35,
        fs::path_of(file).as_deref() == Ok("/fstest/hello.txt"),
        "path of a file",
    )?;
    check(
        35,
        fs::path_of(data).as_deref() == Ok("/"),
        "path of the root",
    )?;
    // Without MANAGE (P11) the metadata stays.
    let plain =
        libstela::ipc::handle_duplicate(file, libstela::Rights::READ | libstela::Rights::WRITE)
            .map_err(|_| 34i64)?;
    check(
        34,
        fs::set_info(plain, &[0; 32]) == Err(Error::AccessDenied),
        "Finder info without MANAGE",
    )?;
    let _ = libstela::ipc::handle_close(plain);
    let st = fs::stat(file).map_err(|_| 8i64)?;
    check(
        8,
        st.size == 10_000 && st.kind == libstela::abi::kind::FILE,
        "stat",
    )?;
    check(
        9,
        fs::read(file, &mut back, 9_990) == Ok(10),
        "read at the end",
    )?;
    // Names ignore case; a second exclusive create fails.
    let again = fs::open(dir, "HELLO.TXT", open::READ).map_err(|_| 10i64)?;
    check(
        10,
        fs::stat(again).map(|s| s.id) == Ok(st.id),
        "case-insensitive name",
    )?;
    check(
        11,
        fs::open(dir, "hello.txt", open::CREATE | open::EXCLUSIVE) == Err(Error::Exists),
        "exclusive create",
    )?;
    check(
        12,
        fs::open(dir, "missing", open::READ) == Err(Error::NotFound),
        "missing file",
    )?;
    // A read-only handle cannot write.
    check(
        13,
        fs::write(again, b"x", 0) == Err(Error::AccessDenied),
        "write through a read handle",
    )?;
    // Rename, then open by id, then list.
    fs::rename(dir, "hello.txt", dir, "renamed.txt").map_err(|_| 14i64)?;
    let by_id = fs::open_by_id(data, st.id).map_err(|_| 15i64)?;
    check(
        15,
        fs::stat(by_id).map(|s| s.size) == Ok(10_000),
        "open by id",
    )?;
    fs::open(dir, "sub", open::CREATE | open::DIRECTORY | open::WRITE).map_err(|_| 16i64)?;
    let mut listed = 0;
    let mut cookie = 0;
    while let Ok(Some((e, next))) = fs::read_dir(dir, cookie) {
        debug_println!("fstest:   {} ({} bytes)", e.name(), e.size);
        listed += 1;
        cookie = next;
    }
    check(16, listed == 2, "list")?;
    // A listing goes on after the item it stopped at is removed (the
    // cookie remembers its name; P11). In key order "gone" comes first.
    let gone = fs::open(dir, "gone", open::CREATE | open::WRITE).map_err(|_| 37i64)?;
    let _ = libstela::ipc::handle_close(gone);
    check(
        37,
        fs::entries(dir).map(|e| e.len()) == Ok(3),
        "three entries",
    )?;
    let (first, cookie) = fs::read_dir(dir, 0).ok().flatten().ok_or(37i64)?;
    check(37, first.name() == "gone", "key order")?;
    fs::remove(dir, "gone").map_err(|_| 37i64)?;
    let mut rest = 0;
    let mut cookie = cookie;
    while let Ok(Some((e, next))) = fs::read_dir(dir, cookie) {
        let want = ["renamed.txt", "sub"].get(rest).copied();
        check(37, Some(e.name()) == want, "listing after a removal")?;
        rest += 1;
        cookie = next;
    }
    check(37, rest == 2, "the rest of the listing")?;
    check(
        17,
        fs::set_size(file, 100).is_ok() && fs::stat(file).map(|s| s.size) == Ok(100),
        "set size",
    )?;
    check(
        18,
        fs::remove(data, "fstest") == Err(Error::NotEmpty),
        "remove a full folder",
    )?;
    fs::remove(dir, "renamed.txt").map_err(|_| 19i64)?;
    fs::remove(dir, "sub").map_err(|_| 19i64)?;
    fs::remove(data, "fstest").map_err(|_| 19i64)?;
    check(20, fs::stat(by_id).is_err(), "removed file")?;

    // The Boot volume (FAT, read only), when there is one.
    if let Ok(boot) = fs::volume_open("Boot") {
        let hello = fs::open(boot, "HELLO.TXT", open::READ).map_err(|_| 21i64)?;
        let mut text = [0u8; 64];
        let n = fs::read(hello, &mut text, 0).map_err(|_| 21i64)?;
        check(21, &text[..n] == b"hello from FAT\n", "Boot:/HELLO.TXT")?;
        check(
            22,
            fs::open(boot, "new", open::CREATE | open::WRITE) == Err(Error::ReadOnly),
            "read-only volume",
        )?;
        check(
            34,
            fs::get_info(hello) == Err(Error::NotSupported),
            "no Finder info on FAT",
        )?;
        let nested = fs::open(boot, "EFI/BOOT/nested.txt", open::READ).map_err(|_| 23i64)?;
        check(
            23,
            fs::stat(nested).map(|s| s.size) == Ok(15),
            "nested FAT path",
        )?;
        check(
            36,
            fs::path_of(nested).is_ok_and(|p| p.eq_ignore_ascii_case("/EFI/BOOT/nested.txt")),
            "path on FAT",
        )?;
    }
    Ok(())
}

/// Removes what an interrupted earlier run left in `Data:/fstest`.
fn remove_tree(data: Handle) -> Result<(), Error> {
    let dir = fs::open(data, "fstest", open::READ | open::WRITE)?;
    while let Some((e, _)) = fs::read_dir(dir, 0)? {
        fs::remove(dir, e.name())?;
    }
    fs::remove(data, "fstest")
}

fn main() -> i64 {
    match run() {
        Ok(()) => {
            debug_println!("fstest: OK");
            0
        }
        Err(step) => step,
    }
}
