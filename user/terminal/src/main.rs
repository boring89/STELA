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
//!  one thread, a wait set: the main port (window events, SHELL_READY),
//!  the master (output) and the shell process (its end)
//!      output: TtyRead(master) → grid.write → redraw the changed rows →
//!              DAMAGE, and a copy to the console (serial)
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
use libstela::wait::{self, Ready, WaitSet};
use libstela::{Error, Handle, Message, Rights, bootstrap, console_write, println, tag, tty};
use terminal::grid::{self, Grid};

/// The 16 ANSI colours (SGR 30-37, 90-97), toned for the parchment
/// background.
const PALETTE: [u32; 16] = [
    0x1A_1410, 0xA0_281E, 0x3C_6E28, 0xA0_7814, 0x2E_4A62, 0x82_3C78, 0x28_6E78, 0xD8_CDB8,
    0x6E_645A, 0xD2_463C, 0x5A_9646, 0xC8_A028, 0x4A_78A0, 0xAA_5AA0, 0x46_A0AA, 0xF2_E8D5,
];

/// The colour of palette entry `index` (`grid::DEFAULT`: `default`).
fn ansi(index: u8, default: u32) -> u32 {
    PALETTE.get(usize::from(index)).copied().unwrap_or(default)
}
use toolbox::draw::{Font, Rect, color, font_for};
use toolbox::event::{Event, MouseKind};
use toolbox::window::{Connection, Window};

libstela::entry!(main);

/// Wait-set keys.
const KEY_EVENTS: u64 = 0;
const KEY_OUTPUT: u64 = 1;
const KEY_SHELL: u64 = 2;
/// Columns and rows at start.
const COLS: usize = 80;
const ROWS: usize = 25;
/// Space around the grid.
const PAD: i32 = 4;

struct Terminal {
    conn: Connection,
    window: Window,
    grid: Grid,
    font: &'static Font,
    slave: Handle,
    master: Handle,
    set: WaitSet,
    shell: Option<Handle>,
    shell_port: Option<Handle>,
    active: bool,
    /// A mouse selection (P11): where it started and where it is, as
    /// (col, row) of the view; `selecting` while the button is down.
    selection: Option<((usize, usize), (usize, usize))>,
    selecting: bool,
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
                // The cursor only on the live screen.
                let live = self.grid.scrolled() == 0;
                for (col, cell) in self.grid.view_row(row).iter().enumerate() {
                    // The right half of a wide character: drawn with it.
                    if cell.c == grid::TAIL {
                        continue;
                    }
                    let x = PAD + col as i32 * font.width;
                    let (fg, bg) = (ansi(cell.fg, color::BLACK), ansi(cell.bg, color::PARCHMENT));
                    let chosen = self
                        .selection
                        .is_some_and(|(a, b)| grid::selected(a, b, (col, row)));
                    if chosen {
                        canvas.char(x, y, cell.c, font, color::PARCHMENT, Some(color::HIGHLIGHT));
                    } else if live && row == cy && col == cx.min(cols - 1) && self.active {
                        canvas.char(x, y, cell.c, font, bg, Some(fg));
                    } else if cell.bg != grid::DEFAULT {
                        canvas.char(x, y, cell.c, font, fg, Some(bg));
                    } else if cell.c != ' ' {
                        canvas.char(x, y, cell.c, font, fg, None);
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
            process::pass_dir(bootstrap::CWD),
        ];
        match process::spawn(program, &["shell"], handles) {
            Ok(p) => {
                let _ = self.set.add(p, KEY_SHELL, wait::EXITED);
                self.shell = Some(p);
            }
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
            // A process stays in a set until removed (it is known by id).
            let _ = self.set.remove(shell);
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

    /// The cell under (x, y), clamped to the grid.
    fn cell_at(&self, x: i32, y: i32) -> (usize, usize) {
        let (cols, rows) = self.grid.size();
        (
            ((x - PAD).max(0) / self.font.width).min(cols as i32 - 1) as usize,
            ((y - PAD).max(0) / self.font.height).min(rows as i32 - 1) as usize,
        )
    }

    /// The mouse selects cells (P11): press, drag, release.
    fn mouse(&mut self, x: i32, y: i32, kind: MouseKind) {
        let at = self.cell_at(x, y);
        let before = self.selection;
        match kind {
            MouseKind::Down => {
                self.selection = Some((at, at));
                self.selecting = true;
            }
            MouseKind::Move if self.selecting => {
                if let Some((a, _)) = self.selection {
                    self.selection = Some((a, at));
                }
            }
            MouseKind::Up => {
                self.selecting = false;
                // A click without a drag selects nothing.
                if self.selection.is_some_and(|(a, b)| a == b) {
                    self.selection = None;
                }
            }
            MouseKind::Move => {}
        }
        if self.selection != before {
            self.grid.mark_all();
            self.redraw();
        }
    }

    /// Edit > Copy: the selected text to the clipboard.
    fn copy(&mut self) {
        if let Some((a, b)) = self.selection {
            let text = self.grid.text_between(a, b);
            let _ = self.conn.set_clipboard(&text);
        }
    }

    /// Edit > Paste: the clipboard's text, typed (as key presses, to the
    /// shell or the program reading the terminal).
    fn paste(&mut self) {
        let Ok(text) = self.conn.clipboard() else {
            return;
        };
        for c in text.chars() {
            if let Some(key) = abi::key::key_for_char(c) {
                self.key(&key);
            }
        }
    }

    fn key(&mut self, key: &KeyEvent) {
        // Shift+PgUp/PgDn: the scrollback (P11).
        use abi::key::KeyCode;
        if key.modifiers.shift && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
            if key.pressed {
                let page = self.grid.size().1 as isize - 1;
                let lines = if key.code == KeyCode::PageUp {
                    page
                } else {
                    -page
                };
                if self.grid.scroll_by(lines) {
                    self.redraw();
                }
            }
            return;
        }
        if let Some(port) = self.shell_port {
            let msg = Message::with_data(tag::INPUT_KEY, &key.encode());
            let _ = port_send(port, &msg);
        }
    }

    /// Takes all the tty output there is: into the grid and to the
    /// console.
    fn output(&mut self) {
        let mut buf = [0u8; 4096];
        while let Ok(n) = tty::read(self.master, &mut buf, 0) {
            if n == 0 {
                break;
            }
            let _ = console_write(&buf[..n]);
            self.grid.write(&buf[..n]);
        }
        self.redraw();
    }

    /// Handles one event; `false` when the terminal should end.
    fn event(&mut self, event: Event) -> bool {
        match event {
            Event::Other(msg) if msg.tag == tag::SHELL_READY => {
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
                    if let Some(old) = self.shell_port.replace(port) {
                        let _ = handle_close(old);
                    }
                }
            }
            Event::Other(msg) => {
                for &h in msg.carried() {
                    let _ = handle_close(h);
                }
            }
            Event::Key { key, .. } => {
                if key.pressed && self.selection.take().is_some() {
                    self.grid.mark_all();
                    self.redraw();
                }
                self.key(&key)
            }
            Event::Mouse { x, y, kind, .. } => self.mouse(x, y, kind),
            Event::Menu(1, 0) => self.copy(),
            Event::Menu(1, 1) => self.paste(),
            Event::Focus(_) => {
                self.active = true;
                self.redraw_all();
            }
            Event::Blur(_) => {
                self.active = false;
                self.redraw_all();
            }
            Event::ResizeRequested(_, w, h) => self.resize(w, h),
            Event::Menu(0, 0) => {
                let (cols, rows) = self.grid.size();
                self.grid = Grid::new(cols, rows);
                self.redraw_all();
            }
            Event::CloseRequested(_) => {
                if let Some(shell) = self.shell.take() {
                    let _ = process::kill(shell);
                }
                return false;
            }
            _ => {}
        }
        true
    }

    fn resize(&mut self, w: u32, h: u32) {
        let (cols, rows) = self.fit(w, h);
        let (w, h) = (
            cols as u32 * self.font.width as u32 + 2 * PAD as u32,
            rows as u32 * self.font.height as u32 + 2 * PAD as u32,
        );
        if self.conn.resize(&mut self.window, w, h).is_ok() {
            self.grid.resize(cols, rows);
            let _ = tty::set_size(self.master, cols as u16, rows as u16);
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
    let _ = conn.set_menubar("Terminal\tClear\nEdit\tCopy\tPaste\n");
    let (slave, master) = match tty::create_with_master() {
        Ok(pair) => pair,
        Err(e) => {
            println!("terminal: no tty: {}", e.name());
            return 1;
        }
    };
    let _ = tty::set_size(master, COLS as u16, ROWS as u16);
    let set = match WaitSet::new() {
        Ok(set) => set,
        Err(e) => {
            println!("terminal: no wait set: {}", e.name());
            return 1;
        }
    };
    if set
        .add(bootstrap::MAIN_PORT, KEY_EVENTS, wait::READABLE)
        .is_err()
        || set.add(master, KEY_OUTPUT, wait::READABLE).is_err()
    {
        println!("terminal: cannot wait on the port and the tty");
        return 1;
    }
    let mut t = Terminal {
        conn,
        window,
        grid: Grid::new(COLS, ROWS),
        font,
        slave,
        master,
        set,
        shell: None,
        shell_port: None,
        active: true,
        selection: None,
        selecting: false,
    };
    t.redraw_all();
    t.start_shell();
    let mut ready = [Ready::default(); 4];
    loop {
        // Events kept while the toolbox waited for a reply come first.
        while t.conn.has_pending() {
            if let Some(e) = t.conn.next_event(0)
                && !t.event(e)
            {
                return 0;
            }
        }
        let n = match t.set.wait(&mut ready, FOREVER) {
            Ok(n) => n,
            Err(Error::TimedOut) => 0,
            Err(e) => {
                println!("terminal: wait failed: {}", e.name());
                return 1;
            }
        };
        for r in &ready[..n] {
            match r.key {
                KEY_OUTPUT => t.output(),
                KEY_SHELL => t.check_shell(),
                _ => {
                    while let Some(e) = t.conn.next_event(0) {
                        if !t.event(e) {
                            return 0;
                        }
                    }
                }
            }
        }
    }
}
