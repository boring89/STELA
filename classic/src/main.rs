// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Classic.app: a Macintosh IIci in a window (`docs/classic.md`).
//!
//! ```text
//!  Data:/Classic/ROM         the Mac IIci ROM (368CADFE), the user's own
//!  Data:/Classic/System.img  the System disk (HFS image), read and written
//!  Data:/Classic/pram        the parameter RAM (256 bytes; made when the
//!                            Mac OS changes it)
//!  Data:/Classic/classic.cfg the settings (classic::config; optional)
//!  Data:/Classic/<disk2>     a second disk (HFS image; classic.cfg's disk2)
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
//!
//! `Classic.app --test-fs` (make test-classic): once the desktop is shown,
//! the Finder is driven from the keyboard (type "data", Command-O,
//! Command-N: a new folder in Data), and Classic.app ends.
//!
//! The Mac also gets the `Data` volume itself (P11): the File System
//! Manager plug-in of `c/mac/fsm.c` with `classic::hostfs` over
//! `datafs::DataStore`; the `Classic` folder (the running disk) is hidden
//! from it.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

mod datafs;

use abi::key::KeyCode;
use abi::window::{AUTO, flags};
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use classic::config::Config;
use classic::ffi::{Card, Mac, MacDisk, MacFs, Power, StartError, card_depth, card_resolution};
use classic::hostfs::HostFs;
use classic::keys::adb_code;
use classic::screen::{Screen, mac_seconds, scale_rows};
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

/// The ROM's size (the IIci ROM is 512 KiB).
const ROM_SIZE: usize = 512 * 1024;
/// One frame of real time.
const FRAME_NS: u64 = 1_000_000_000 / 60;
/// The parameter RAM is saved at most this often.
const PRAM_EVERY_FRAMES: u32 = 300;

/// A disk of the Mac: an image file on a STELA volume.
struct FileDisk {
    file: Handle,
    blocks: u32,
    read_only: bool,
}

impl FileDisk {
    /// `Data:/Classic/<name>`, writable unless `read_only`.
    fn open(name: &str, read_only: bool) -> Result<FileDisk, Error> {
        let flags = if read_only {
            open::READ
        } else {
            open::READ | open::WRITE
        };
        let file = open_classic(name, flags)?;
        let size = fs::stat(file).map(|s| s.size).unwrap_or(0);
        Ok(FileDisk {
            file,
            blocks: (size / 512).min(u64::from(u32::MAX)) as u32,
            read_only,
        })
    }

    fn kib(&self) -> u32 {
        self.blocks / 2
    }
}

impl MacDisk for FileDisk {
    fn blocks(&self) -> u32 {
        self.blocks
    }

    fn read_only(&self) -> bool {
        self.read_only
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

/// The settings: `Data:/Classic/classic.cfg` (if there is one), then the
/// arguments of the form `key=value` over it.
fn settings() -> Config {
    let (mut config, problems) = match read_all("classic.cfg", 64 * 1024) {
        Ok(text) => Config::parse(&alloc::string::String::from_utf8_lossy(&text)),
        Err(_) => (Config::default(), Vec::new()),
    };
    for p in problems {
        println!("classic: classic.cfg: {p} (ignored)");
    }
    for arg in process::args().iter().skip(1).filter(|a| a.contains('=')) {
        if let Err(e) = config.set(arg) {
            println!("classic: {e} (ignored)");
        }
    }
    config
}

/// The NuBus card of the settings; `None`: the built-in video.
fn card_of(config: &Config) -> Option<Card> {
    let (w, h) = config.resolution?;
    let Some(resolution) = card_resolution(w, h) else {
        println!("classic: the card has no {w}x{h} mode; the built-in video it is");
        return None;
    };
    let depth = card_depth(config.depth).unwrap_or(3);
    Some(Card { resolution, depth })
}

/// Starts the Mac from the files with the settings; prints why it cannot.
fn start(config: &Config) -> Option<Mac> {
    let card = card_of(config);
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
    let disk = match FileDisk::open("System.img", false) {
        Ok(d) => d,
        Err(e) => {
            println!(
                "classic: no System disk: put an HFS disk image at Data:/Classic/System.img ({})",
                e.name()
            );
            return None;
        }
    };
    let size_kib = disk.kib();
    let mut disks: Vec<Box<dyn MacDisk>> = vec![Box::new(disk)];
    if let Some((name, read_only)) = &config.disk2 {
        match FileDisk::open(name, *read_only) {
            Ok(d) => {
                println!(
                    "classic: disk2 {name} ({} KiB{})",
                    d.kib(),
                    if *read_only { ", read only" } else { "" }
                );
                disks.push(Box::new(d));
            }
            Err(e) => println!(
                "classic: no second disk: Data:/Classic/{name} ({}); going on without it",
                e.name()
            ),
        }
    }
    let mut xpram = [0u8; 256];
    match read_all("pram", 256) {
        Ok(p) if p.len() == 256 => xpram.copy_from_slice(&p),
        Ok(_) => {
            println!("classic: Data:/Classic/pram is not 256 bytes; starting with a blank one")
        }
        Err(_) => {}
    }
    let clock = mac_seconds(process::wall_clock_ns().unwrap_or(0));
    let fs = if config.fs {
        data_volume().map(|fs| (fs, "Data"))
    } else {
        println!("classic: the Data volume is not given to the Mac (fs=off)");
        None
    };
    let ram = config.ram_mib << 20;
    match Mac::start(rom.into_boxed_slice(), ram, disks, fs, card, &xpram, clock) {
        Ok(mac) => {
            println!(
                "classic: started (ROM 368CADFE, {} MiB, disk {size_kib} KiB)",
                config.ram_mib,
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

/// A Mac volume that reports its changes (and first reads) on the
/// console.
struct LogFs<T: MacFs> {
    fs: T,
    read: alloc::collections::BTreeSet<u32>,
}

impl<T: MacFs> LogFs<T> {
    fn name(&mut self, id: u32) -> alloc::string::String {
        self.fs.get(id).map_or_else(
            || alloc::format!("#{id}"),
            |i| {
                i.name
                    .iter()
                    .map(|&b| classic::ffi::mac_roman_char(b))
                    .collect()
            },
        )
    }
}

impl<T: MacFs> MacFs for LogFs<T> {
    fn get(&mut self, id: u32) -> Option<classic::ffi::FsItem> {
        self.fs.get(id)
    }
    fn lookup(&mut self, parent: u32, name: &[u8]) -> Option<classic::ffi::FsItem> {
        self.fs.lookup(parent, name)
    }
    fn child(&mut self, parent: u32, index: u32) -> Option<classic::ffi::FsItem> {
        self.fs.child(parent, index)
    }
    fn read(&mut self, id: u32, rsrc: bool, offset: u32, buf: &mut [u8]) -> Option<usize> {
        if !rsrc && self.read.insert(id) {
            let name = self.name(id);
            println!("classic: fs: read {name}");
        }
        self.fs.read(id, rsrc, offset, buf)
    }
    fn space(&mut self) -> classic::ffi::FsSpace {
        self.fs.space()
    }
    fn create(
        &mut self,
        parent: u32,
        name: &[u8],
        is_dir: bool,
    ) -> Result<classic::ffi::FsItem, i32> {
        let r = self.fs.create(parent, name, is_dir);
        let text: alloc::string::String = name
            .iter()
            .map(|&b| classic::ffi::mac_roman_char(b))
            .collect();
        match &r {
            Ok(_) => println!("classic: fs: created {text}"),
            Err(e) => println!("classic: fs: cannot create {text}: {e}"),
        }
        r
    }
    fn remove(&mut self, id: u32) -> Result<(), i32> {
        let name = self.name(id);
        let r = self.fs.remove(id);
        println!("classic: fs: removed {name}: {r:?}");
        r
    }
    fn move_to(&mut self, id: u32, parent: u32, name: Option<&[u8]>) -> Result<(), i32> {
        let r = self.fs.move_to(id, parent, name);
        let now = self.name(id);
        println!("classic: fs: moved or renamed to {now}: {r:?}");
        r
    }
    fn set_info(&mut self, item: &classic::ffi::FsItem) -> Result<(), i32> {
        self.fs.set_info(item)
    }
    fn write(&mut self, id: u32, rsrc: bool, offset: u32, data: &[u8]) -> Result<usize, i32> {
        self.fs.write(id, rsrc, offset, data)
    }
    fn set_length(&mut self, id: u32, rsrc: bool, length: u32) -> Result<(), i32> {
        self.fs.set_length(id, rsrc, length)
    }
}

/// The `Data` volume for the Mac (its `Classic` folder hidden), if it can
/// be opened.
fn data_volume() -> Option<Box<dyn MacFs>> {
    let root = fs::volume_open("Data").ok()?;
    match datafs::DataStore::new(root) {
        Ok(store) => Some(Box::new(LogFs {
            fs: HostFs::new(store, "Data", &["Classic"]),
            read: alloc::collections::BTreeSet::new(),
        })),
        Err(e) => {
            println!(
                "classic: the Data volume is not given to the Mac: {}",
                e.name()
            );
            None
        }
    }
}

/// The window's picture: the Mac's screen, the rows shown last, and the
/// scale (window pixels per Mac pixel; `docs/classic.md` §10).
struct View {
    screen: Screen,
    picture: Vec<u32>,
    scale: usize,
    /// Enlarged rows on their way to the window.
    wide: Vec<u32>,
}

impl View {
    fn new(w: u32, h: u32, scale: u32) -> View {
        View {
            screen: Screen::new(w as usize, h as usize),
            picture: vec![0u32; (w * h) as usize],
            scale: scale as usize,
            wide: Vec::new(),
        }
    }

    /// Copies the changed rows of the picture into the window, enlarged,
    /// and damages them.
    fn show(&mut self, conn: &Connection, window: &mut Window) {
        let bands = self.screen.update(&self.picture, 8);
        if bands.is_empty() {
            return;
        }
        let (w, s) = (self.screen.width, self.scale);
        {
            let mut c = window.canvas();
            for &(first, count) in &bands {
                if s == 1 {
                    let band = Rect::new(0, first as i32, w as i32, count as i32);
                    c.copy_from(0, first as i32, &self.picture, w, band);
                } else {
                    scale_rows(&self.picture, w, first, count, s, &mut self.wide);
                    let band = Rect::new(0, 0, (w * s) as i32, (count * s) as i32);
                    c.copy_from(0, (first * s) as i32, &self.wide, w * s, band);
                }
            }
        }
        for (first, count) in bands {
            let band = Rect::new(0, (first * s) as i32, (w * s) as i32, (count * s) as i32);
            let _ = conn.damage(window, band);
        }
    }
}

fn main() -> i64 {
    if turned_off() {
        println!("classic: turned off by the boot option classic=off");
        return 1;
    }
    let config = settings();
    let Some(mut mac) = start(&config) else {
        return 1;
    };
    let mut conn = match Connection::open() {
        Ok(c) => c,
        Err(e) => {
            println!("classic: no window server: {}", e.name());
            return 1;
        }
    };
    let (mut w, mut h) = mac.screen_size();
    // P13 K13: the window's own scale times the interface's (the screen
    // here is logical) is at most 2; the log shows the product.
    let ui = conn.scale.max(1);
    let mut scale = config.scale_for(conn.screen, (w, h)).min((2 / ui).max(1));
    println!("classic: screen {w}x{h} (scale {})", scale * ui);
    let Ok(mut window) = conn.create_window(
        w * scale,
        h * scale,
        flags::TITLE_BAR | flags::CLOSE_BOX | flags::WANT_MOTION | flags::HIDE_CURSOR_INSIDE,
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
    let test_fs = process::args().iter().any(|a| a == "--test-fs");
    let test_power = process::args().iter().any(|a| a == "--test-power");
    // The test scripts: (frames after the desktop is shown, step).
    let mut script: Vec<(u64, Step)> = Vec::new();
    if test_fs {
        let mut at = 30;
        for key in [KeyCode::D, KeyCode::A, KeyCode::T, KeyCode::A] {
            script.push((at, Step::Key(key, true)));
            script.push((at + 4, Step::Key(key, false)));
            at += 10;
        }
        for (at, key) in [(at + 20, KeyCode::O), (400, KeyCode::N)] {
            script.push((at, Step::Key(KeyCode::LeftAlt, true)));
            script.push((at + 2, Step::Key(key, true)));
            script.push((at + 6, Step::Key(key, false)));
            script.push((at + 8, Step::Key(KeyCode::LeftAlt, false)));
        }
    }
    // --test-power: Special > Restart on the first desktop, Special > Shut
    // Down on the second.
    let mut restarts = 0;
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
    let mut view = View::new(w, h, scale);
    let start_ns = libstela::now_ns();
    let mut frame: u64 = 0;
    let mut finder = false;
    let mut finder_frame = 0;
    let mut button = false;
    let mut fs_mounted = false;
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
                    let (x, y) = (x / scale as i32, y / scale as i32);
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
        // The Monitors control panel switched the card's resolution: the
        // window follows.
        let (nw, nh) = mac.screen_size();
        let ns = config.scale_for(conn.screen, (nw, nh)).min((2 / ui).max(1));
        if (nw, nh) != (w, h) && conn.resize(&mut window, nw * ns, nh * ns).is_ok() {
            (w, h, scale) = (nw, nh, ns);
            view = View::new(w, h, scale);
            println!("classic: screen {w}x{h} (scale {})", scale * ui);
        }
        if frame.is_multiple_of(2) && mac.render(&mut view.picture) {
            view.show(&conn, &mut window);
        }
        if !fs_mounted && frame.is_multiple_of(30) && mac.fs_status().mounted {
            fs_mounted = true;
            println!("classic: the Data volume is on the Mac's desktop");
        }
        if finder && finder_frame + 180 == frame {
            // Three seconds later the desktop has been drawn and shown
            // (make test-classic saves the screen now).
            let fs = mac.fs_status();
            println!(
                "classic: fs: install {:?}, mounted {}, {} requests",
                fs.install, fs.mounted, fs.requests
            );
            for (drive, name) in mac.volumes() {
                println!("classic: volume \"{name}\" (drive {drive})");
            }
            println!("classic: finder desktop shown");
            if test_beep {
                println!(
                    "classic: beep ({})",
                    if mac.sysbeep() { "sent" } else { "busy" }
                );
            }
        }
        if test_power && finder && finder_frame + 180 == frame {
            let item = if restarts == 0 {
                SPECIAL_RESTART
            } else {
                SPECIAL_SHUT_DOWN
            };
            script = choose_special(item);
        }
        while finder
            && let Some(&(at, step)) = script.first()
            && finder_frame + 180 + at <= frame
        {
            script.remove(0);
            match step {
                Step::Key(key, down) => {
                    if let Some(code) = adb_code(key) {
                        mac.key(code, down);
                    }
                }
                Step::Mouse(x, y, down) => mac.mouse(x, y, down),
            }
            if script.is_empty() {
                println!("classic: test steps sent");
            }
        }
        if test_power && finder && finder_frame + 180 + 600 == frame {
            // Neither a restart nor a shut down came (a failure; make
            // test-classic saves the screen here when debugging).
            println!("classic: power test: still running");
        }
        if test_fs && finder && finder_frame + 180 + 650 == frame {
            println!("classic: fs test checking");
        }
        if test_fs && finder && finder_frame + 180 + 700 == frame {
            let fs = mac.fs_status();
            println!("classic: fs test done ({} requests)", fs.requests);
            let (xpram, _) = mac.xpram();
            save_pram(&xpram);
            let _ = conn.close(window);
            return 0;
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
                restarts += 1;
                script.clear();
                drop(mac);
                let Some(m) = start(&config) else {
                    return 1;
                };
                mac = m;
                finder = false;
                fs_mounted = false;
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

/// A step of a test script: an ADB key, or the mouse at a Mac position
/// with the button up or down.
#[derive(Clone, Copy)]
enum Step {
    Key(KeyCode, bool),
    Mouse(i32, i32, bool),
}

/// The Finder's Special menu (System 7.5, built-in 640x480 or larger):
/// its title in the menu bar, and where its Restart and Shut Down items
/// are (16 rows each, dividers too: Clean Up Desktop, Empty Trash, -,
/// Eject Disk, Erase Disk, -, Restart, Shut Down).
const SPECIAL_TITLE: (i32, i32) = (232, 10);
const SPECIAL_RESTART: i32 = 20 + 6 * 16 + 8;
const SPECIAL_SHUT_DOWN: i32 = 20 + 7 * 16 + 8;

/// Steps that pull down the Special menu and release on the item at row
/// `y`, as a hand on the mouse would.
fn choose_special(y: i32) -> Vec<(u64, Step)> {
    let (x, title) = SPECIAL_TITLE;
    vec![
        (10, Step::Mouse(x, title, false)),
        (20, Step::Mouse(x, title, true)),
        (40, Step::Mouse(x, 40, true)),
        (60, Step::Mouse(x, y, true)),
        (80, Step::Mouse(x, y, false)),
    ]
}

/// Whether the boot options say `classic=off` (`docs/p520.md` §3).
fn turned_off() -> bool {
    abi::sysinfo::cmdline_option(&process::cmdline(), "classic")
        .is_some_and(|v| v.eq_ignore_ascii_case(b"off"))
}
