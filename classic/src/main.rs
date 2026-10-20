// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Classic.app: a Macintosh IIci in a window (`docs/classic.md`).
//!
//! ```text
//!  Data:/Classic/ROM         the Mac IIci ROM (368CADFE), the user's own
//!  Data:/Classic/System.img  the System disk (HFS image), read and written
//!  Data:/Classic/pram        the parameter RAM (256 bytes; made when the
//!                            Mac OS changes it)
//! ```
//!
//! The main loop runs the Mac one 60 Hz frame at a time (416,000 cycles
//! of the 25 MHz 68030), handles the window's events in between, and every
//! second frame converts the picture and copies the rows that changed into
//! the window; the sound chip's output goes to the sound output when there
//! is one. Ahead of real time it sleeps; behind, it yields.
//!
//! `Classic.app --test-beep` (make test-classic): once the Finder's desktop
//! is shown, the Mac beeps, and two seconds later Classic.app ends.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use abi::window::{AUTO, flags};
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use classic::ffi::{Mac, MacDisk, Power, StartError};
use classic::keys::adb_code;
use classic::screen::{Screen, mac_seconds};
use classic::sound::Resampler;
use libstela::audio::Audio;
use libstela::fs::{self, open};
use libstela::ipc::handle_close;
use libstela::{Error, Handle, println, process};
use toolbox::draw::Rect;
use toolbox::event::{Event, MouseKind};
use toolbox::menu::MenuBar;
use toolbox::window::{Connection, Window};

libstela::entry!(main);

/// The Mac's RAM.
const RAM_SIZE: u32 = 16 << 20;
/// The ROM's size (the IIci ROM is 512 KiB).
const ROM_SIZE: usize = 512 * 1024;
/// One frame of real time.
const FRAME_NS: u64 = 1_000_000_000 / 60;
/// The parameter RAM is saved at most this often.
const PRAM_EVERY_FRAMES: u32 = 300;

/// The System disk: an image file on a STELA volume.
struct FileDisk {
    file: Handle,
    blocks: u32,
}

impl MacDisk for FileDisk {
    fn blocks(&self) -> u32 {
        self.blocks
    }

    fn read_only(&self) -> bool {
        false
    }

    fn read(&mut self, offset: u32, buf: &mut [u8]) -> bool {
        let mut done = 0;
        while done < buf.len() {
            match fs::read(self.file, &mut buf[done..], u64::from(offset) + done as u64) {
                Ok(0) | Err(_) => return false,
                Ok(n) => done += n,
            }
        }
        true
    }

    fn write(&mut self, offset: u32, data: &[u8]) -> bool {
        let mut done = 0;
        while done < data.len() {
            match fs::write(self.file, &data[done..], u64::from(offset) + done as u64) {
                Ok(0) | Err(_) => return false,
                Ok(n) => done += n,
            }
        }
        true
    }
}

impl Drop for FileDisk {
    fn drop(&mut self) {
        let _ = handle_close(self.file);
    }
}

/// Opens `Data:/Classic/<name>`.
fn open_classic(name: &str, flags: u64) -> Result<Handle, Error> {
    let data = fs::volume_open("Data")?;
    let h = fs::open(data, &alloc::format!("Classic/{name}"), flags);
    let _ = handle_close(data);
    h
}

/// The whole of a file (at most `max` bytes).
fn read_all(name: &str, max: usize) -> Result<Vec<u8>, Error> {
    let h = open_classic(name, open::READ)?;
    let size = fs::stat(h).map(|s| s.size as usize).unwrap_or(0).min(max);
    let mut data = vec![0u8; size];
    let mut done = 0;
    while done < size {
        match fs::read(h, &mut data[done..], done as u64) {
            Ok(0) | Err(_) => break,
            Ok(n) => done += n,
        }
    }
    let _ = handle_close(h);
    data.truncate(done);
    Ok(data)
}

/// Writes the parameter RAM file.
fn save_pram(xpram: &[u8; 256]) {
    match open_classic("pram", open::READ | open::WRITE | open::CREATE) {
        Ok(h) => {
            if fs::write(h, xpram, 0).is_err() {
                println!("classic: cannot write Data:/Classic/pram");
            }
            let _ = handle_close(h);
        }
        Err(e) => println!("classic: cannot create Data:/Classic/pram: {}", e.name()),
    }
}

/// Starts the Mac from the files; prints why it cannot.
fn start() -> Option<Mac> {
    let rom = match read_all("ROM", ROM_SIZE) {
        Ok(r) if r.len() == ROM_SIZE => r,
        Ok(r) => {
            println!(
                "classic: Data:/Classic/ROM has {} bytes; the Mac IIci ROM has {ROM_SIZE}",
                r.len()
            );
            return None;
        }
        Err(e) => {
            println!(
                "classic: no ROM: put your Mac IIci ROM at Data:/Classic/ROM ({})",
                e.name()
            );
            return None;
        }
    };
    let file = match open_classic("System.img", open::READ | open::WRITE) {
        Ok(h) => h,
        Err(e) => {
            println!(
                "classic: no System disk: put an HFS disk image at Data:/Classic/System.img ({})",
                e.name()
            );
            return None;
        }
    };
    let size = fs::stat(file).map(|s| s.size).unwrap_or(0);
    let blocks = (size / 512).min(u64::from(u32::MAX)) as u32;
    let disk = FileDisk { file, blocks };
    let mut xpram = [0u8; 256];
    match read_all("pram", 256) {
        Ok(p) if p.len() == 256 => xpram.copy_from_slice(&p),
        Ok(_) => {
            println!("classic: Data:/Classic/pram is not 256 bytes; starting with a blank one")
        }
        Err(_) => {}
    }
    let clock = mac_seconds(process::wall_clock_ns().unwrap_or(0));
    match Mac::start(
        rom.into_boxed_slice(),
        RAM_SIZE,
        Box::new(disk),
        &xpram,
        clock,
    ) {
        Ok(mac) => {
            println!(
                "classic: started (ROM 368CADFE, {} MiB, disk {} KiB)",
                RAM_SIZE >> 20,
                size / 1024
            );
            Some(mac)
        }
        Err(StartError::BadRom) => {
            println!("classic: Data:/Classic/ROM is not the Mac IIci ROM (checksum 368CADFE)");
            None
        }
        Err(e) => {
            println!("classic: cannot start the Mac: {e:?}");
            None
        }
    }
}

/// Copies the changed rows of `picture` into the window and damages them.
fn show(conn: &Connection, window: &mut Window, screen: &mut Screen, picture: &[u32]) {
    let bands = screen.update(picture, 8);
    if bands.is_empty() {
        return;
    }
    let w = screen.width;
    {
        let mut c = window.canvas();
        for &(first, count) in &bands {
            c.copy_from(
                0,
                first as i32,
                picture,
                w,
                Rect::new(0, first as i32, w as i32, count as i32),
            );
        }
    }
    for (first, count) in bands {
        let _ = conn.damage(window, Rect::new(0, first as i32, w as i32, count as i32));
    }
}

fn main() -> i64 {
    if turned_off() {
        println!("classic: turned off by the boot option classic=off");
        return 1;
    }
    let Some(mut mac) = start() else {
        return 1;
    };
    let mut conn = match Connection::open() {
        Ok(c) => c,
        Err(e) => {
            println!("classic: no window server: {}", e.name());
            return 1;
        }
    };
    let (w, h) = mac.screen_size();
    let Ok(mut window) = conn.create_window(
        w,
        h,
        flags::TITLE_BAR | flags::CLOSE_BOX,
        AUTO,
        AUTO,
        "Classic",
    ) else {
        return 1;
    };
    let mut menus = MenuBar::new();
    menus.add("File", &["Quit"]);
    let _ = conn.set_menubar(&menus.to_text());
    let test_beep = process::args().iter().any(|a| a == "--test-beep");
    // The sound output is optional: without it (no card, or another
    // program has it) the Mac's sound is dropped.
    let audio = match Audio::open() {
        Ok(a) => {
            println!("classic: sound on");
            Some(a)
        }
        Err(e) => {
            println!("classic: no sound ({})", e.name());
            None
        }
    };
    let mut resampler = Resampler::default();
    let mut screen = Screen::new(w as usize, h as usize);
    let mut picture = vec![0u32; (w * h) as usize];
    let start_ns = libstela::now_ns();
    let mut frame: u64 = 0;
    let mut finder = false;
    let mut finder_frame = 0;
    let mut button = false;
    loop {
        // Events first: the input reaches the Mac before the next frame.
        while let Some(event) = conn.next_event(0) {
            match event {
                Event::Key { key, .. } => {
                    if let Some(code) = adb_code(key.code) {
                        mac.key(code, key.pressed);
                    }
                }
                Event::Mouse { x, y, kind, .. } => {
                    button = match kind {
                        MouseKind::Down => true,
                        MouseKind::Up => false,
                        MouseKind::Move => button,
                    };
                    mac.mouse(x.clamp(0, w as i32 - 1), y.clamp(0, h as i32 - 1), button);
                }
                Event::Menu(0, 0) | Event::CloseRequested(_) => {
                    let (xpram, _) = mac.xpram();
                    save_pram(&xpram);
                    println!("classic: closed after {frame} frames");
                    let _ = conn.close(window);
                    return 0;
                }
                _ => {}
            }
        }
        let power = mac.frame();
        frame += 1;
        let (samples, rate) = mac.take_audio();
        if let Some(out) = &audio {
            let frames = resampler.convert(&samples, rate);
            // What does not fit is dropped (the Mac runs slower than real
            // time under emulation; better a gap than a growing delay).
            out.write(&frames);
        }
        if frame.is_multiple_of(2) && mac.render(&mut picture) {
            show(&conn, &mut window, &mut screen, &picture);
        }
        if finder && finder_frame + 180 == frame {
            // Three seconds later the desktop has been drawn and shown
            // (make test-classic saves the screen now).
            println!("classic: finder desktop shown");
            if test_beep {
                println!(
                    "classic: beep ({})",
                    if mac.sysbeep() { "sent" } else { "busy" }
                );
            }
        }
        if test_beep && finder && finder_frame + 300 == frame {
            println!("classic: test done");
            let (xpram, _) = mac.xpram();
            save_pram(&xpram);
            let _ = conn.close(window);
            return 0;
        }
        if !finder && frame.is_multiple_of(30) && mac.finder_running() {
            finder = true;
            finder_frame = frame;
            println!(
                "classic: finder: the Finder runs after {frame} frames, {} ms",
                (libstela::now_ns() - start_ns) / 1_000_000
            );
        }
        if frame.is_multiple_of(u64::from(PRAM_EVERY_FRAMES)) {
            let (xpram, dirty) = mac.xpram();
            if dirty {
                save_pram(&xpram);
            }
        }
        match power {
            Power::None => {}
            Power::Off => {
                let (xpram, _) = mac.xpram();
                save_pram(&xpram);
                println!("classic: the Mac was shut down");
                let _ = conn.close(window);
                return 0;
            }
            Power::Restart => {
                let (xpram, _) = mac.xpram();
                save_pram(&xpram);
                println!("classic: restarting");
                drop(mac);
                let Some(m) = start() else {
                    return 1;
                };
                mac = m;
                finder = false;
                continue;
            }
        }
        // Pace to real time: sleep when ahead, yield when behind.
        let due = start_ns + frame * FRAME_NS;
        let now = libstela::now_ns();
        if now < due {
            libstela::sleep_ns(due - now);
        } else {
            libstela::yield_now();
        }
    }
}

/// Whether the boot options say `classic=off` (`docs/p520.md` §3).
fn turned_off() -> bool {
    abi::sysinfo::cmdline_option(&process::cmdline(), "classic")
        .is_some_and(|v| v.eq_ignore_ascii_case(b"off"))
}
