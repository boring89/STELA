// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! terminal: a window with a text grid, running the shell on a tty
//! (`docs/window.md` §7).
//!
//! ```text
//!  window server ── KEY ──► terminal ── INPUT_KEY ──► shell's main port
//!                                                     (the shell passes
//!                                                     keys on to the tty
//!                                                     while a program runs)
//!  shell, its programs ── print to [1] = the tty ──► master
//!  output thread: TtyRead(master) ──► OUTPUT message to our main port
//!                                 └─► a copy to the console (serial)
//!  main thread: grid.write(output) → redraw the changed rows → DAMAGE
//! ```
//!
//! The copy on the console keeps the serial log complete (the tests read
//! the shell's output there). When the shell ends, a new one starts.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use abi::key::KeyEvent;
use abi::window::{AUTO, flags};
use libstela::fs::{self, open};
use libstela::ipc::{FOREVER, handle_close, handle_duplicate, port_send};
use libstela::process::{self, pass};
use libstela::{
    Handle, MESSAGE_DATA, Message, Rights, ThreadStack, bootstrap, console_write, println, tag, tty,
};
use terminal::grid::Grid;
use toolbox::draw::{Font, Rect, color, font_for};
use toolbox::event::Event;
use toolbox::window::{Connection, Window};

libstela::entry!(main);

/// Our own message: tty output for the main thread.
const OUTPUT: u32 = 0x7E00_0001;
/// Columns and rows at start.
const COLS: usize = 80;
const ROWS: usize = 25;
/// Space around the grid.
const PAD: i32 = 4;

static OUTPUT_STACK: ThreadStack<16384> = ThreadStack::new();

/// The output thread: the tty's output, to our main port and the console.
fn pump(master: u64) {
    let master = Handle::from_raw(master);
    let mut buf = [0u8; 4096];
    let Ok(to_main) = handle_duplicate(bootstrap::MAIN_PORT, Rights::SEND) else {
        return;
    };
    loop {
        let n = match tty::read(master, &mut buf, FOREVER) {
            Ok(n) => n,
            Err(_) => return,
        };
        let _ = console_write(&buf[..n]);
        for chunk in buf[..n].chunks(MESSAGE_DATA) {
            let msg = Message::with_data(OUTPUT, chunk);
            while port_send(to_main, &msg) == Err(libstela::Error::Full) {
                libstela::yield_now();
            }
        }
    }
}

struct Terminal {
    conn: Connection,
    window: Window,
    grid: Grid,
    font: &'static Font,
    slave: Handle,
    shell: Option<Handle>,
    shell_port: Option<Handle>,
    active: bool,
}

impl Terminal {
    /// Draws the changed rows (and the cursor) and tells the server.
    fn redraw(&mut self) {
        let rows = self.grid.take_dirty();
        if rows.is_empty() {
            return;
        }
        let (cols, _) = self.grid.size();
        let font = self.font;
        let (cx, cy) = self.grid.cursor;
        let mut damaged = Rect::default();
        {
            let mut canvas = self.window.canvas();
            for &row in &rows {
                let y = PAD + row as i32 * font.height;
                let line = Rect::new(0, y, cols as i32 * font.width + 2 * PAD, font.height);
                canvas.fill(line, color::PARCHMENT);
                for (col, &c) in self.grid.row(row).iter().enumerate() {
                    let x = PAD + col as i32 * font.width;
                    if row == cy && col == cx.min(cols - 1) && self.active {
                        canvas.char(x, y, c, font, color::PARCHMENT, Some(color::BLACK));
                    } else if c != ' ' {
                        canvas.char(x, y, c, font, color::BLACK, None);
                    }
                }
                damaged = damaged.union(&line);
            }
        }
        let _ = self.conn.damage(&self.window, damaged);
    }

    fn redraw_all(&mut self) {
        {
            let bounds = self.window.bounds();
            let mut canvas = self.window.canvas();
            canvas.fill(bounds, color::PARCHMENT);
        }
        self.grid.mark_all();
        self.redraw();
        let _ = self.conn.damage_all(&self.window);
    }

    /// The grid size that fits a content area.
    fn fit(&self, w: u32, h: u32) -> (usize, usize) {
        (
            ((w as i32 - 2 * PAD) / self.font.width).max(10) as usize,
            ((h as i32 - 2 * PAD) / self.font.height).max(3) as usize,
        )
    }

    fn start_shell(&mut self) {
        let Ok(initrd) = fs::volume_open("Initrd") else {
            return;
        };
        let program = fs::open(initrd, "shell", open::READ);
        let _ = handle_close(initrd);
        let Ok(program) = program else {
            println!("terminal: no shell in Initrd:/");
            return;
        };
        let handles = [
            pass(self.slave, Rights::SEND | Rights::DUPLICATE),
            pass(bootstrap::MAIN_PORT, Rights::SEND),
            pass(
                bootstrap::CWD,
                Rights::READ | Rights::WRITE | Rights::DUPLICATE,
            ),
        ];
        match process::spawn(program, &["shell"], handles) {
            Ok(p) => self.shell = Some(p),
            Err(e) => println!("terminal: cannot start the shell: {}", e.name()),
        }
        let _ = handle_close(program);
    }

    /// Restarts the shell if it ended.
    fn check_shell(&mut self) {
        let Some(shell) = self.shell else {
            return;
        };
        if let Ok(code) = process::wait(shell, 0) {
            let _ = handle_close(shell);
            self.shell = None;
            if let Some(port) = self.shell_port.take() {
                let _ = handle_close(port);
            }
            let line = alloc::format!("[shell exited with code {code}]\n");
            self.grid.write(line.as_bytes());
            let _ = console_write(line.as_bytes());
            self.start_shell();
        }
    }

    fn key(&mut self, key: &KeyEvent) {
        if let Some(port) = self.shell_port {
            let msg = Message::with_data(tag::INPUT_KEY, &key.encode());
            let _ = port_send(port, &msg);
        }
    }

    fn resize(&mut self, w: u32, h: u32) {
        let (cols, rows) = self.fit(w, h);
        let (w, h) = (
            cols as u32 * self.font.width as u32 + 2 * PAD as u32,
            rows as u32 * self.font.height as u32 + 2 * PAD as u32,
        );
        if self.conn.resize(&mut self.window, w, h).is_ok() {
            self.grid.resize(cols, rows);
            self.redraw_all();
        }
    }
}

fn main() -> i64 {
    let mut conn = match Connection::open() {
        Ok(c) => c,
        Err(e) => {
            println!("terminal: no window server: {}", e.name());
            return 1;
        }
    };
    let font = font_for(conn.screen.0);
    let (w, h) = (
        COLS as u32 * font.width as u32 + 2 * PAD as u32,
        ROWS as u32 * font.height as u32 + 2 * PAD as u32,
    );
    let window = match conn.create_window(w, h, flags::DOCUMENT, AUTO, AUTO, "Terminal") {
        Ok(w) => w,
        Err(e) => {
            println!("terminal: cannot make a window: {}", e.name());
            return 1;
        }
    };
    let _ = conn.set_menubar("Terminal\tClear\n");
    let (slave, master) = match tty::create_with_master() {
        Ok(pair) => pair,
        Err(e) => {
            println!("terminal: no tty: {}", e.name());
            return 1;
        }
    };
    if let Err(e) = libstela::spawn(pump, u64::from(master.0), &OUTPUT_STACK) {
        println!("terminal: cannot start the output thread: {}", e.name());
        return 1;
    }
    let mut t = Terminal {
        conn,
        window,
        grid: Grid::new(COLS, ROWS),
        font,
        slave,
        shell: None,
        shell_port: None,
        active: true,
    };
    t.redraw_all();
    t.start_shell();
    loop {
        let event = t.conn.next_event(500_000_000);
        match event {
            Some(Event::Other(msg)) if msg.tag == OUTPUT => {
                t.grid.write(msg.payload());
                // Take whatever else is waiting before drawing.
                t.redraw();
            }
            Some(Event::Other(msg)) if msg.tag == tag::SHELL_READY => {
                if let Some(&port) = msg.carried().first() {
                    // The programs the shell starts get the window server.
                    if let Ok(ws) = handle_duplicate(
                        bootstrap::BOOTSTRAP,
                        Rights::SEND | Rights::DUPLICATE | Rights::TRANSFER,
                    ) {
                        let mut m = Message::with_data(tag::WINDOW_SERVER, b"");
                        m.push_handle(ws);
                        if port_send(port, &m).is_err() {
                            let _ = handle_close(ws);
                        }
                    }
                    if let Some(old) = t.shell_port.replace(port) {
                        let _ = handle_close(old);
                    }
                }
            }
            Some(Event::Other(msg)) => {
                for &h in msg.carried() {
                    let _ = handle_close(h);
                }
            }
            Some(Event::Key { key, .. }) => t.key(&key),
            Some(Event::Focus(_)) => {
                t.active = true;
                t.redraw_all();
            }
            Some(Event::Blur(_)) => {
                t.active = false;
                t.redraw_all();
            }
            Some(Event::ResizeRequested(_, w, h)) => t.resize(w, h),
            Some(Event::Menu(0, 0)) => {
                let (cols, rows) = t.grid.size();
                t.grid = Grid::new(cols, rows);
                t.redraw_all();
            }
            Some(Event::CloseRequested(_)) => {
                if let Some(shell) = t.shell.take() {
                    let _ = process::kill(shell);
                }
                return 0;
            }
            _ => {}
        }
        t.check_shell();
    }
}
