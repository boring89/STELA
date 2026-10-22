// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Setup.app: the first start of STELA (P12, `docs/setup.md`). init starts
//! it (through the window server) instead of Mouseion when the Data volume
//! has no `.stela/setup-done` (or with the boot option `setup`).
//!
//! ```text
//!  Language ─► Time zone ─► Data volume ─► Network ─► Packages ─► Classic ─► Display ─► Done
//!  English/繁體中文  +08:00     found, or format an     DHCP, DNS,   from the    ROM and     setup-done,
//!                              empty place (type its   the package  signed      System      language,
//!                              name to confirm)        server       index       images      timezone;
//!                                                                                           Mouseion
//!  Display (P14 K2): the interface scale, when the screen sleeps, the
//!  desktop's pattern (Data:/.stela/display, Data:/.stela/desktop).
//! ```
//!
//! Everything is done with the keyboard: arrows choose, Space marks,
//! Enter goes on (or acts), Esc goes back. Each page prints
//! `setup: page NAME` on the console (tests wait for it).

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use abi::key::KeyCode;
use abi::window::flags;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use libstela::fs::{self, open};
use libstela::ipc::{FOREVER, handle_close};
use libstela::process::{self, pass};
use libstela::{Rights, bootstrap, println};
use pkgcore::ops;
use toolbox::control::{FieldResult, TextField};
use toolbox::draw::{Rect, color, font_for};
use toolbox::event::Event;
use toolbox::i18n::{self, Language, tr};
use toolbox::window::Connection;

libstela::entry!(main);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    Language,
    TimeZone,
    Install,
    Data,
    Network,
    Packages,
    Classic,
    Display,
    Done,
}

impl Page {
    fn name(self) -> &'static str {
        match self {
            Page::Language => "language",
            Page::TimeZone => "timezone",
            Page::Install => "install",
            Page::Data => "data",
            Page::Network => "network",
            Page::Packages => "packages",
            Page::Classic => "classic",
            Page::Display => "display",
            Page::Done => "done",
        }
    }

    fn next(self) -> Page {
        match self {
            Page::Language => Page::TimeZone,
            Page::TimeZone => Page::Install,
            Page::Install => Page::Data,
            Page::Data => Page::Network,
            Page::Network => Page::Packages,
            Page::Packages => Page::Classic,
            Page::Classic => Page::Display,
            Page::Display | Page::Done => Page::Done,
        }
    }

    fn back(self) -> Page {
        match self {
            Page::Language | Page::TimeZone => Page::Language,
            Page::Install => Page::TimeZone,
            Page::Data => Page::Install,
            Page::Network => Page::Data,
            Page::Packages => Page::Network,
            Page::Classic => Page::Packages,
            Page::Display => Page::Classic,
            Page::Done => Page::Display,
        }
    }
}

/// What the pages know.
struct Setup {
    page: Page,
    language: Language,
    zone: TextField,
    zone_minutes: i32,
    /// Places that can become Data: (name, bytes).
    candidates: Vec<(String, u64)>,
    /// Whole empty disks STELA can be installed on (P14 K7): (name, bytes).
    disks: Vec<(String, u64)>,
    confirm: TextField,
    /// The network page's lines (made when it is shown).
    network: Vec<String>,
    /// The index's packages and which are marked.
    packages: Vec<(pkgcore::Package, bool)>,
    /// Images found for Classic: (Volume:path, marked).
    images: Vec<(String, bool)>,
    /// The selected line of the page's list.
    cursor: usize,
    /// The last thing that happened (a result or an error).
    status: String,
    /// The display page (P14 K2): the scale (0 automatic, 1, 2), the
    /// index in [`BLANK_MINUTES`], the desktop's pattern.
    scale: u32,
    blank: usize,
    pattern: u32,
}

/// The choices of "sleep after" (0: never); 10 is the window server's
/// default.
const BLANK_MINUTES: [u32; 6] = [0, 1, 5, 10, 30, 60];

/// The patterns, in the page's order.
const PATTERNS: [u32; 3] = [
    abi::window::backdrop::PLAIN,
    abi::window::backdrop::MEANDER,
    abi::window::backdrop::WAVES,
];

impl Setup {
    fn new() -> Setup {
        let mut zone = TextField::new(Rect::new(0, 0, 1, 1), 6);
        zone.text = String::from("+08:00");
        zone.cursor = 6;
        Setup {
            page: Page::Language,
            language: Language::English,
            zone,
            zone_minutes: abi::time::DEFAULT_OFFSET_MINUTES,
            candidates: Vec::new(),
            disks: Vec::new(),
            confirm: TextField::new(Rect::new(0, 0, 1, 1), 32),
            network: Vec::new(),
            packages: Vec::new(),
            images: Vec::new(),
            cursor: 0,
            status: String::new(),
            scale: 0,
            blank: 3,
            pattern: abi::window::backdrop::PLAIN,
        }
    }

    /// Prepares `page` and shows it.
    fn enter(&mut self, page: Page) {
        // P14 K7: the install page only when there is a disk for it (and a
        // System volume to copy); otherwise on in the same direction.
        if page == Page::Install {
            self.disks = fs::install_candidates().unwrap_or_default();
            if self.disks.is_empty() || fs::volume_open("System").is_err() {
                let skip = if self.page == Page::Data {
                    Page::TimeZone
                } else {
                    Page::Data
                };
                self.page = page;
                return self.enter(skip);
            }
            for (name, bytes) in &self.disks {
                println!("setup: can install on: {name} ({} MiB)", bytes >> 20);
            }
            self.confirm.text.clear();
            self.confirm.cursor = 0;
        }
        self.page = page;
        self.cursor = 0;
        self.status.clear();
        match page {
            Page::Data => {
                if !has_data() {
                    self.candidates = fs::format_candidates().unwrap_or_default();
                    for (name, bytes) in &self.candidates {
                        println!("setup: can become Data: {name} ({} MiB)", bytes >> 20);
                    }
                }
            }
            Page::Network => self.check_network(),
            Page::Packages => {
                self.packages = ops::index()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|p| (p, false))
                    .collect();
            }
            Page::Classic => self.images = classic_images(),
            _ => {}
        }
        println!("setup: page {}", page.name());
    }

    fn check_network(&mut self) {
        let mut lines = Vec::new();
        match libstela::net::wait_configured(5_000_000_000) {
            Some(info) => {
                lines.push(format!(
                    "{}: {}",
                    tr("Address"),
                    libstela::net::ip_text(info.ip)
                ));
                lines.push(format!(
                    "{}: {}",
                    tr("DNS server"),
                    libstela::net::ip_text(info.dns)
                ));
            }
            None => lines.push(format!("{}: -", tr("Address"))),
        }
        let line = match ops::conf() {
            (None, _) => format!("{}: {}", tr("Package server"), tr("not configured")),
            (Some(server), _) => match ops::update() {
                Ok(n) => format!(
                    "{}: {server} ({}, {n})",
                    tr("Package server"),
                    tr("reachable")
                ),
                Err(e) => format!("{}: {server}: {}", tr("Package server"), e.text()),
            },
        };
        println!("setup: {line}");
        lines.push(line);
        self.network = lines;
    }

    /// The page's text lines (and the marked line).
    fn lines(&self) -> (String, Vec<String>) {
        let mut out = Vec::new();
        let title = match self.page {
            Page::Language => {
                out.push(String::from(tr(
                    "Choose a language with the arrow keys, then press Enter.",
                )));
                out.push(String::new());
                for (i, l) in [Language::English, Language::TraditionalChinese]
                    .iter()
                    .enumerate()
                {
                    let mark = if i == self.cursor { ">" } else { " " };
                    out.push(format!("{mark} {}", l.name()));
                }
                tr("Language")
            }
            Page::TimeZone => {
                out.push(String::from(tr(
                    "Type the offset from UTC (for example +08:00), then press Enter.",
                )));
                out.push(String::new());
                out.push(format!("UTC{}_", self.zone.text));
                tr("Time zone")
            }
            Page::Install => {
                out.push(String::from(tr(
                    "STELA can be copied to a whole empty disk (ESP, System, Data).",
                )));
                out.push(String::new());
                for (i, (name, bytes)) in self.disks.iter().enumerate() {
                    let mark = if i == self.cursor { ">" } else { " " };
                    out.push(format!("{mark} {name} ({} MiB)", bytes >> 20));
                }
                out.push(String::new());
                out.push(String::from(tr(
                    "Type the disk's name to install there, or press Enter to go on.",
                )));
                out.push(format!("{}_", self.confirm.text));
                tr("Install to disk")
            }
            Page::Data => {
                out.push(String::from(tr(
                    "Your files and programs live on the Data volume.",
                )));
                out.push(String::new());
                if let Some(info) = data_info() {
                    out.push(format!("{} {info}", tr("Found the Data volume:")));
                } else if self.candidates.is_empty() {
                    out.push(String::from(tr("Nothing can become a Data volume.")));
                } else {
                    out.push(String::from(tr(
                        "No Data volume. Empty places that can become one:",
                    )));
                    for (i, (name, bytes)) in self.candidates.iter().enumerate() {
                        let mark = if i == self.cursor { ">" } else { " " };
                        out.push(format!("{mark} {name} ({} MiB)", bytes >> 20));
                    }
                    out.push(String::new());
                    out.push(String::from(tr(
                        "Erases it. Type its name to confirm, then press Enter.",
                    )));
                    out.push(format!("{}_", self.confirm.text));
                }
                tr("Data volume")
            }
            Page::Network => {
                out.extend(self.network.iter().cloned());
                tr("Network")
            }
            Page::Packages => {
                out.push(String::from(tr(
                    "Space selects, Enter installs the selected ones (or goes on).",
                )));
                out.push(String::new());
                let installed = ops::installed();
                for (i, (p, marked)) in self.packages.iter().enumerate() {
                    let mark = if i == self.cursor { ">" } else { " " };
                    let state = if installed.iter().any(|(n, _)| n == &p.name) {
                        tr("installed")
                    } else if *marked {
                        "[x]"
                    } else {
                        "[ ]"
                    };
                    out.push(format!(
                        "{mark} {state} {} {} - {}",
                        p.name, p.version, p.description
                    ));
                }
                tr("Packages")
            }
            Page::Classic => {
                if self.images.is_empty() {
                    out.push(String::from(tr("None found. Press Enter.")));
                } else {
                    out.push(String::from(tr(
                        "Mac ROM and System disk images on other volumes:",
                    )));
                    out.push(String::from(tr(
                        "Space selects, Enter copies them to Data:/Classic/ (or goes on).",
                    )));
                    out.push(String::new());
                    for (i, (path, marked)) in self.images.iter().enumerate() {
                        let mark = if i == self.cursor { ">" } else { " " };
                        let x = if *marked { "[x]" } else { "[ ]" };
                        out.push(format!("{mark} {x} {path}"));
                    }
                }
                tr("Classic")
            }
            Page::Display => {
                out.push(String::from(tr(
                    "Up and down choose a line, left and right change it.",
                )));
                out.push(String::new());
                let scale = match self.scale {
                    0 => String::from(tr("automatic")),
                    n => format!("{n}x"),
                };
                let blank = match BLANK_MINUTES[self.blank] {
                    0 => String::from(tr("never")),
                    m => format!("{m} {}", tr("minutes")),
                };
                let pattern = tr(abi::window::backdrop::name(self.pattern));
                let rows = [
                    format!(
                        "{}: < {scale} > {}",
                        tr("Interface scale"),
                        tr("(from the next start)")
                    ),
                    format!("{}: < {blank} >", tr("Sleep after")),
                    format!("{}: < {pattern} >", tr("Desktop pattern")),
                ];
                for (i, row) in rows.into_iter().enumerate() {
                    let mark = if i == self.cursor { ">" } else { " " };
                    out.push(format!("{mark} {row}"));
                }
                tr("Display")
            }
            Page::Done => {
                out.push(String::from(tr("STELA is ready. Press Enter to start.")));
                tr("Done")
            }
        };
        if !self.status.is_empty() {
            out.push(String::new());
            out.push(self.status.clone());
        }
        (format!("{} - {title}", tr("Welcome to STELA")), out)
    }

    /// How many lines the page's list has.
    fn list_len(&self) -> usize {
        match self.page {
            Page::Language => 2,
            Page::Install => self.disks.len(),
            Page::Data => self.candidates.len(),
            Page::Packages => self.packages.len(),
            Page::Classic => self.images.len(),
            Page::Display => 3,
            _ => 0,
        }
    }

    /// Left (`forward` false) or Right on the display page.
    fn change(&mut self, forward: bool) {
        let step = |at: usize, n: usize| {
            if forward {
                (at + 1) % n
            } else {
                (at + n - 1) % n
            }
        };
        match self.cursor {
            0 => self.scale = step(self.scale as usize, 3) as u32,
            1 => self.blank = step(self.blank, BLANK_MINUTES.len()),
            _ => {
                let at = PATTERNS
                    .iter()
                    .position(|&p| p == self.pattern)
                    .unwrap_or(0);
                self.pattern = PATTERNS[step(at, PATTERNS.len())];
            }
        }
    }

    /// Enter on the page: act, or go on. Returns `true` when Setup is
    /// done.
    fn enter_key(&mut self) -> bool {
        match self.page {
            Page::Language => {
                self.language = if self.cursor == 1 {
                    Language::TraditionalChinese
                } else {
                    Language::English
                };
                i18n::set(self.language);
                println!("setup: language {}", self.language.code());
            }
            Page::TimeZone => match abi::time::parse_offset(&self.zone.text) {
                Some(m) => {
                    self.zone_minutes = m;
                    println!("setup: time zone {}", self.zone.text);
                }
                None => {
                    self.status = format!("{}: UTC{}", tr("Error"), self.zone.text);
                    return false;
                }
            },
            Page::Install if !self.confirm.text.is_empty() => {
                let Some((name, _)) = self.disks.get(self.cursor).cloned() else {
                    return false;
                };
                if self.confirm.text != name {
                    self.status = format!("{}: {}", tr("Error"), name);
                    return false;
                }
                println!("setup: installing on {name}");
                self.status = match fs::install_to(&name) {
                    Ok(()) => {
                        println!("setup: installed on {name}");
                        self.disks.retain(|(n, _)| *n != name);
                        format!(
                            "{name}: {}",
                            tr("installed; start from it without the installer")
                        )
                    }
                    Err(e) => format!("{}: {}", tr("Error"), e.name()),
                };
                self.confirm.text.clear();
                self.confirm.cursor = 0;
                return false;
            }
            Page::Data if !has_data() && !self.candidates.is_empty() => {
                let Some((name, _)) = self.candidates.get(self.cursor).cloned() else {
                    return false;
                };
                if self.confirm.text != name {
                    self.status = format!("{}: {}", tr("Error"), name);
                    return false;
                }
                println!("setup: formatting {name}");
                match fs::format_volume(&name) {
                    Ok(()) => {
                        println!("setup: {name} is the Data volume");
                        self.confirm.text.clear();
                        self.confirm.cursor = 0;
                        self.status = format!("{name}: Data");
                    }
                    Err(e) => self.status = format!("{}: {}", tr("Error"), e.name()),
                }
                return false;
            }
            Page::Packages if self.packages.iter().any(|(_, m)| *m) => {
                let names: Vec<String> = self
                    .packages
                    .iter()
                    .filter(|(_, m)| *m)
                    .map(|(p, _)| p.name.clone())
                    .collect();
                let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                match ops::install(&refs, |step| println!("setup: {step}")) {
                    Ok(done) => {
                        for p in &done {
                            println!("setup: installed {} {}", p.name, p.version);
                        }
                        self.status = format!("{} {}", tr("installed"), names.join(", "));
                    }
                    Err(e) => self.status = format!("{}: {}", tr("Error"), e.text()),
                }
                for p in self.packages.iter_mut() {
                    p.1 = false;
                }
                return false;
            }
            Page::Classic if self.images.iter().any(|(_, m)| *m) => {
                let mut copied = 0;
                for (path, marked) in self.images.iter_mut() {
                    if !*marked {
                        continue;
                    }
                    let name = path.rsplit(['/', ':']).next().unwrap_or("image");
                    let r = fs::read_path(path)
                        .and_then(|d| ops::write_file(&format!("Data:/Classic/{name}"), &d));
                    match r {
                        Ok(()) => copied += 1,
                        Err(e) => self.status = format!("{}: {path}: {}", tr("Error"), e.name()),
                    }
                    *marked = false;
                }
                if copied > 0 {
                    self.status = format!("Data:/Classic/ ({copied})");
                }
                return false;
            }
            Page::Display => {
                let scale = match self.scale {
                    0 => String::from("auto"),
                    n => format!("{n}"),
                };
                println!(
                    "setup: display scale={scale} blank={} pattern={}",
                    BLANK_MINUTES[self.blank],
                    abi::window::backdrop::name(self.pattern)
                );
            }
            Page::Done => return true,
            _ => {}
        }
        let next = self.page.next();
        self.enter(next);
        false
    }
}

/// Whether the Data volume is mounted.
fn has_data() -> bool {
    match fs::volume_open("Data") {
        Ok(h) => {
            let _ = handle_close(h);
            true
        }
        Err(_) => false,
    }
}

/// "Data (eHFS, N MiB free of M MiB)".
fn data_info() -> Option<String> {
    let mut i = 0;
    while let Ok(Some(v)) = fs::volume_list(i) {
        if v.name() == "Data" {
            return Some(format!(
                "Data ({} MiB, {} MiB free)",
                v.total >> 20,
                v.free >> 20
            ));
        }
        i += 1;
    }
    None
}

/// ROM and disk images on the volumes other than Data, Initrd and Tmp
/// (their top folders): `*.rom`, `*.img`, `*.dsk`, `*.hfs`.
fn classic_images() -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Ok(Some(v)) = fs::volume_list(i) {
        i += 1;
        let name = v.name().to_string();
        if matches!(name.as_str(), "Data" | "Initrd" | "Tmp") {
            continue;
        }
        let Ok(root) = fs::volume_open(&name) else {
            continue;
        };
        for e in fs::entries(root).unwrap_or_default() {
            let n = e.name().to_ascii_lowercase();
            if [".rom", ".img", ".dsk", ".hfs"]
                .iter()
                .any(|x| n.ends_with(x))
            {
                out.push((format!("{name}:/{}", e.name()), false));
            }
        }
        let _ = handle_close(root);
    }
    out
}

/// Writes the choices and `setup-done`.
fn finish(s: &Setup) -> Result<(), abi::Error> {
    ops::write_file(i18n::LANGUAGE_FILE, s.language.code().as_bytes())?;
    let m = s.zone_minutes;
    let sign = if m < 0 { '-' } else { '+' };
    let zone = format!("{sign}{:02}:{:02}\n", m.abs() / 60, m.abs() % 60);
    ops::write_file("Data:/.stela/timezone", zone.as_bytes())?;
    // P14 K2: the display page; the window server reads these at start.
    let scale = match s.scale {
        0 => String::from("auto"),
        n => format!("{n}"),
    };
    let display = format!("scale={scale}\nblank={}\n", BLANK_MINUTES[s.blank]);
    ops::write_file("Data:/.stela/display", display.as_bytes())?;
    let desktop = format!("pattern={}\n", abi::window::backdrop::name(s.pattern));
    ops::write_file("Data:/.stela/desktop", desktop.as_bytes())?;
    ops::write_file("Data:/.stela/setup-done", b"1\n")
}

/// Starts Mouseion the way the window server does.
fn start_mouseion() {
    let Ok(initrd) = fs::volume_open("Initrd") else {
        return;
    };
    let file = fs::open(initrd, "mouseion", open::READ);
    let _ = handle_close(initrd);
    let Ok(file) = file else {
        println!("setup: no Initrd:/mouseion");
        return;
    };
    let handles = [
        pass(bootstrap::CONSOLE, Rights::SEND | Rights::DUPLICATE),
        pass(
            bootstrap::BOOTSTRAP,
            Rights::SEND | Rights::DUPLICATE | Rights::TRANSFER,
        ),
        process::pass_dir(bootstrap::CWD),
    ];
    let mut caps = process::inherited();
    if let Some(p) = process::pass_capability(abi::capability::POWER) {
        caps.push((abi::capability::POWER, p));
    }
    match process::spawn_with(file, &["mouseion"], &handles, &caps) {
        Ok(p) => {
            let _ = handle_close(p);
        }
        Err(e) => println!("setup: cannot start Mouseion: {}", e.name()),
    }
    let _ = handle_close(file);
}

fn main() -> i64 {
    let mut conn = match Connection::open() {
        Ok(c) => c,
        Err(e) => {
            println!("setup: no window server: {}", e.name());
            return 1;
        }
    };
    let font = font_for(conn.screen.0);
    let (fw, fh) = (font.width, font.height);
    let (w, h) = (72 * fw + 32, 24 * (fh + 4) + 32);
    let x = (conn.screen.0 as i32 - w) / 2;
    let y = (conn.screen.1 as i32 - h) / 2;
    let Ok(mut window) = conn.create_window(w as u32, h as u32, flags::DOCUMENT, x, y, "Setup")
    else {
        return 1;
    };
    let mut setup = Setup::new();
    setup.enter(Page::Language);
    println!("setup: ready");
    let mut dirty = true;
    loop {
        if dirty {
            let (title, lines) = setup.lines();
            let _ = conn.set_title(&window, &title);
            let bounds = window.bounds();
            let mut c = window.canvas();
            c.fill(bounds, color::PARCHMENT);
            let mut at = 16;
            c.text(16, at, &title, font, color::BLACK, None);
            at += 2 * (fh + 4);
            for line in &lines {
                c.text(16, at, line, font, color::BLACK, None);
                at += fh + 4;
            }
            c.text(
                16,
                h - fh - 16,
                tr("Enter: next   Esc: back"),
                font,
                color::BLACK,
                None,
            );
            let _ = conn.damage_all(&window);
            dirty = false;
        }
        let Some(event) = conn.next_event(FOREVER) else {
            continue;
        };
        let Event::Key { key, .. } = event else {
            continue;
        };
        if !key.pressed {
            continue;
        }
        dirty = true;
        match key.code {
            KeyCode::Enter => {
                if setup.enter_key() {
                    break;
                }
            }
            KeyCode::Escape => {
                let back = setup.page.back();
                setup.enter(back);
            }
            KeyCode::Up => setup.cursor = setup.cursor.saturating_sub(1),
            KeyCode::Down => {
                if setup.cursor + 1 < setup.list_len() {
                    setup.cursor += 1;
                }
            }
            KeyCode::Left | KeyCode::Right if setup.page == Page::Display => {
                setup.change(key.code == KeyCode::Right);
            }
            KeyCode::Space if setup.page == Page::Packages => {
                if let Some(p) = setup.packages.get_mut(setup.cursor) {
                    p.1 = !p.1;
                }
            }
            KeyCode::Space if setup.page == Page::Classic => {
                if let Some(p) = setup.images.get_mut(setup.cursor) {
                    p.1 = !p.1;
                }
            }
            _ => {
                let field = match setup.page {
                    Page::TimeZone => &mut setup.zone,
                    Page::Data | Page::Install => &mut setup.confirm,
                    _ => continue,
                };
                if matches!(field.key(&key), FieldResult::None) {
                    dirty = false;
                }
            }
        }
    }
    match finish(&setup) {
        Ok(()) => println!("setup: done"),
        Err(e) => println!("setup: cannot write the settings: {}", e.name()),
    }
    let _ = conn.close(window);
    start_mouseion();
    0
}
