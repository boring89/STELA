// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Monitor (`Monitor.app`, working name; P11, `docs/apps.md` §3): what
//! the system is doing, again every second.
//!
//! ```text
//!  Processes                                        [ End Process ]
//!    pid  name                threads  cpu  state
//!    ...  (a list; click one to select it)
//!  Memory   used / total, kernel heap used / size
//!  CPUs     timer ticks per second, each (a sign of life, not a load)
//!  Network  card, address, frames and bytes in and out
//!  Up       hours:minutes:seconds
//! ```
//!
//! End Process asks, then ends the selected process (`ProcessOpen` +
//! `ProcessKill`); it needs the `POWER` capability, which Mouseion's
//! Special > Monitor gives it. init, the window server, Mouseion and the
//! monitor itself are not ended from here.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use abi::window::{AUTO, flags};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use libstela::{Error, net, println, process};
use toolbox::control::{Button, List};
use toolbox::dialog;
use toolbox::draw::{Font, Rect, color, font_for};
use toolbox::event::{Event, MouseKind};
use toolbox::menu::MenuBar;
use toolbox::window::{Connection, Window};

libstela::entry!(main);

const COLS: i32 = 64;
const LIST_ROWS: i32 = 14;
const PAD: i32 = 12;
/// Processes that are not ended from here.
const PROTECTED: [&str; 4] = ["init", "windowserver", "mouseion", "monitor"];

struct Monitor {
    conn: Connection,
    window: Window,
    font: &'static Font,
    list: List,
    /// The pids of the list's lines.
    pids: Vec<(u64, String)>,
    end: Button,
    lines: Vec<String>,
    /// Each processor's ticks last time, and when.
    ticks: Vec<u64>,
    at: u64,
}

impl Monitor {
    fn line(&self) -> i32 {
        self.font.height + 4
    }

    /// Reads everything again.
    fn refresh(&mut self) {
        let selected = self
            .list
            .selected
            .and_then(|i| self.pids.get(i))
            .map(|p| p.0);
        let cpus: Vec<(u64, u64)> = process::process_cpus();
        let list = process::list().unwrap_or_default();
        self.pids.clear();
        let mut items = Vec::new();
        for p in list.iter().filter(|p| p.state == 1) {
            let cpu = cpus
                .iter()
                .find(|c| c.0 == p.pid)
                .map_or(String::from("-"), |c| format!("{}", c.1));
            items.push(format!(
                "{:>5}  {:<20} {:>7}  {:>3}  running",
                p.pid,
                p.name(),
                p.threads,
                cpu
            ));
            self.pids.push((p.pid, String::from(p.name())));
        }
        let ended = list.iter().filter(|p| p.state != 1).count();
        self.list.items = items;
        self.list.selected = selected.and_then(|pid| self.pids.iter().position(|p| p.0 == pid));
        let mut lines = Vec::new();
        lines.push(format!("{} running, {ended} ended", self.pids.len()));
        if let Ok(m) = process::memory() {
            lines.push(format!(
                "Memory   {} of {} MiB used; kernel heap {} of {} KiB",
                (m.total_frames - m.free_frames) * 4 / 1024,
                m.total_frames * 4 / 1024,
                m.heap_used / 1024,
                m.heap_size / 1024
            ));
        }
        let now = libstela::now_ns();
        let ticks = process::cpus();
        let span = now.saturating_sub(self.at).max(1);
        let rates: Vec<String> = ticks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let before = self.ticks.get(i).copied().unwrap_or(*t);
                format!("cpu{i} {}/s", (t - before) * 1_000_000_000 / span)
            })
            .collect();
        lines.push(format!("CPUs     {}", rates.join("  ")));
        self.ticks = ticks;
        self.at = now;
        match net::info() {
            Some(n) if n.configured => lines.push(format!(
                "Network  {} {}: in {} frames ({} KiB), out {} ({} KiB)",
                n.driver_name(),
                net::ip_text(n.ip),
                n.rx_packets,
                n.rx_bytes / 1024,
                n.tx_packets,
                n.tx_bytes / 1024
            )),
            Some(n) => lines.push(format!("Network  {}: no address", n.driver_name())),
            None => lines.push(String::from("Network  no card")),
        }
        let up = now / 1_000_000_000;
        lines.push(format!(
            "Up       {}:{:02}:{:02}",
            up / 3600,
            up / 60 % 60,
            up % 60
        ));
        self.lines = lines;
    }

    fn draw(&mut self) {
        let font = self.font;
        let line = self.line();
        {
            let bounds = self.window.bounds();
            let mut c = self.window.canvas();
            c.fill(bounds, color::PARCHMENT);
            c.text(PAD, PAD + 4, "Processes", font, color::BLACK, None);
            let head = format!(
                "{:>5}  {:<20} {:>7}  {:>3}  state",
                "pid", "name", "threads", "cpu"
            );
            c.text(PAD + 4, PAD + 2 * line, &head, font, color::GREY, None);
            self.list.draw(&mut c, font, line);
            c.frame(self.list.rect.inset(-1), color::BLACK);
            self.end.draw(&mut c, font);
            let mut y = self.list.rect.bottom() + line / 2;
            for l in &self.lines {
                c.text(PAD, y, l, font, color::BLACK, None);
                y += line;
            }
        }
        let _ = self.conn.damage_all(&self.window);
    }

    /// End Process: asks, then ends the selected process.
    fn end_selected(&mut self) {
        let Some((pid, name)) = self.list.selected.and_then(|i| self.pids.get(i)).cloned() else {
            return;
        };
        if PROTECTED.iter().any(|p| p.eq_ignore_ascii_case(&name)) {
            let text = format!("\"{name}\" is part of the system and is not ended here.");
            let _ = dialog::alert(&mut self.conn, "Monitor", &text, &["OK"]);
            return;
        }
        let question = format!("End \"{name}\" (process {pid})? Unsaved work in it is lost.");
        if dialog::alert(&mut self.conn, "Monitor", &question, &["Cancel", "End"]) != Some(1) {
            return;
        }
        let result = process::open(pid).and_then(|h| {
            let r = process::kill(h);
            let _ = libstela::ipc::handle_close(h);
            r
        });
        match result {
            Ok(()) => println!("monitor: ended {pid} ({name})"),
            Err(e) => {
                let text = match e {
                    Error::AccessDenied => String::from(
                        "The monitor may not end processes. Start it from Mouseion's Special menu.",
                    ),
                    e => format!("Cannot end \"{name}\": {}.", e.name()),
                };
                let _ = dialog::alert(&mut self.conn, "Monitor", &text, &["OK"]);
            }
        }
    }
}

fn main() -> i64 {
    let mut conn = match Connection::open() {
        Ok(c) => c,
        Err(e) => {
            println!("monitor: no window server: {}", e.name());
            return 1;
        }
    };
    let font = font_for(conn.screen.0);
    let line = font.height + 4;
    let w = COLS * font.width + 2 * PAD;
    let list_rect = Rect::new(PAD, PAD + 3 * line, w - 2 * PAD, LIST_ROWS * line);
    let h = list_rect.bottom() + 6 * line + PAD;
    let Ok(window) = conn.create_window(
        w as u32,
        h as u32,
        flags::TITLE_BAR | flags::CLOSE_BOX,
        AUTO,
        AUTO,
        "Monitor",
    ) else {
        return 1;
    };
    let mut menus = MenuBar::new();
    menus.add("File", &["End Process", "-", "Close^W"]);
    let _ = conn.set_menubar(&menus.to_text());
    let bw = 15 * font.width;
    let mut m = Monitor {
        conn,
        window,
        font,
        list: List::new(list_rect, Vec::new()),
        pids: Vec::new(),
        end: Button::new(
            Rect::new(w - PAD - bw, PAD, bw, font.height + 12),
            "End Process",
        ),
        lines: Vec::new(),
        ticks: process::cpus(),
        at: libstela::now_ns(),
    };
    m.refresh();
    m.draw();
    println!("monitor: showing {} processes", m.pids.len());
    let mut next = libstela::now_ns() + 1_000_000_000;
    loop {
        let now = libstela::now_ns();
        if now >= next {
            m.refresh();
            m.draw();
            next = now + 1_000_000_000;
            continue;
        }
        let Some(event) = m.conn.next_event(next - now) else {
            continue;
        };
        match event {
            Event::Mouse { x, y, kind, .. } => {
                match kind {
                    MouseKind::Down => {
                        m.end.press(x, y);
                        let line = m.line();
                        m.list.click(x, y, line);
                    }
                    MouseKind::Up => {
                        if m.end.release(x, y) {
                            m.end_selected();
                            m.refresh();
                        }
                    }
                    MouseKind::Move => {}
                }
                m.draw();
            }
            Event::Menu(0, 0) => {
                m.end_selected();
                m.refresh();
                m.draw();
            }
            Event::Menu(0, 2) | Event::CloseRequested(_) => break,
            _ => {}
        }
    }
    let Monitor { conn, window, .. } = m;
    let _ = conn.close(window);
    0
}
