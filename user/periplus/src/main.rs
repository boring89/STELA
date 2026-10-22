// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Periplus (`Periplus.app`, working name; P11, `docs/apps.md` §4): a
//! small web browser. HTTP only (no TLS), a subset of HTML
//! (`periplus::parse`), one monospaced font.
//!
//! ```text
//!  [<] [>] [Reload] [ http://host/path                          ]
//!  ┌──────────────────────────────────────────────────────────┬─┐
//!  │ the page, laid out in rows (headings and <b> bold, links  │▲│
//!  │ underlined; the selected link inverted)                   │ │
//!  └──────────────────────────────────────────────────────────┴─┘
//!  status: Loaded http://host/path (12 KiB)
//! ```
//!
//! Addresses: `http://host[:port]/path`, `https://...` (P12, TLS 1.3),
//! `stela:home` (built in),
//! `Volume:/path` or `file:Volume:/path` (local files; `.html`/`.htm` are
//! HTML). Started without an address it shows the home page with an
//! empty address field ready for typing. Keys: Ctrl-L a new address
//! (Enter goes, Escape puts the old one back), Tab /
//! Shift-Tab the next / previous link, Enter follows it, Alt-Left /
//! Alt-Right back and forward, Ctrl-R reload, arrows and Page Up/Down
//! scroll. A click on a link follows it. It prints `periplus: loaded URL
//! (title "...")` for the tests.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use abi::key::{KeyCode, KeyEvent};
use abi::window::{AUTO, flags};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use libstela::fs::{self, open};
use libstela::ipc::{FOREVER, handle_close};
use libstela::path::{self, Location};
use libstela::{http, println};
use periplus::{Page, Row};
use toolbox::control::{Button, FieldResult, ScrollBar, TextField};
use toolbox::draw::{Canvas, Font, Rect, color, font_for};
use toolbox::event::{Event, MouseKind};
use toolbox::menu::MenuBar;
use toolbox::window::{Connection, Window};

libstela::entry!(main);

const COLS: i32 = 90;
const ROWS: i32 = 26;
const PAD: i32 = 8;
const BAR: i32 = 16;
/// Largest page read.
const MAX: usize = 4 << 20;

/// The built-in home page.
const HOME: &str = "<html><head><title>STELA</title></head><body>\
<h1>Periplus</h1>\
<p>A small browser for STELA: HTTP and HTTPS, a subset of HTML, one font.</p>\
<p>Type an address with Ctrl-L. Tab and Enter follow links; Alt-Left goes back.</p>\
<h2>Places</h2><ul>\
<li><a href=\"System:/doc/guide.html\">STELA 使用手冊</a>, <a href=\"System:/doc/guide.en.html\">STELA User Guide</a></li>\
<li><a href=\"Data:/welcome.txt\">Data:/welcome.txt</a> (a file on the Data volume)</li>\
<li><a href=\"http://10.0.2.2:8642/\">http://10.0.2.2:8642/</a> (the host, under QEMU's user networking)</li>\
<li><a href=\"http://example.com/\">http://example.com/</a>, <a href=\"https://example.com/\">https://example.com/</a></li>\
</ul></body></html>";

/// What drawing the page needs.
struct View<'a> {
    rows: &'a [Row],
    scroll: usize,
    focus: Option<usize>,
    area: Rect,
    line: i32,
    page: usize,
    bar: ScrollBar,
}

impl View<'_> {
    /// The rows (headings and <b> bold, links underlined, the selected one
    /// inverted) and the scroll bar.
    fn draw(&self, c: &mut Canvas, font: &Font) {
        let a = self.area;
        c.fill(a, color::WHITE);
        c.frame(a, color::BLACK);
        let saved = c.clip();
        c.set_clip(saved.intersect(&a.inset(1)));
        for (i, row) in self
            .rows
            .iter()
            .skip(self.scroll)
            .take(self.page + 1)
            .enumerate()
        {
            let y = a.y + 2 + i as i32 * self.line;
            for span in row {
                let x = a.x + 4 + span.col as i32 * font.width;
                let focused = span.link.is_some() && span.link == self.focus;
                let (fg, bg) = match (span.link, focused) {
                    (Some(_), true) => (color::WHITE, Some(color::HIGHLIGHT)),
                    (Some(_), false) => (color::HIGHLIGHT, None),
                    _ => (color::BLACK, None),
                };
                c.text(x, y, &span.text, font, fg, bg);
                if span.bold {
                    // Bold: the text again one pixel to the right.
                    c.text(x + 1, y, &span.text, font, fg, None);
                }
                if span.link.is_some() {
                    let w = font.text_width(&span.text);
                    c.hline(x, y + font.height - 1, w, fg);
                }
            }
        }
        c.set_clip(saved);
        self.bar.draw(c);
    }
}

struct Browser {
    conn: Connection,
    window: Window,
    font: &'static Font,
    address: TextField,
    editing: bool,
    back: Button,
    forward: Button,
    reload: Button,
    url: String,
    page: Page,
    rows: Vec<Row>,
    scroll: usize,
    /// The selected link (Tab), if any.
    focus: Option<usize>,
    status: String,
    history: Vec<String>,
    /// Where in `history` the page is.
    at: usize,
}

impl Browser {
    fn line(&self) -> i32 {
        self.font.height + 2
    }

    /// The page's rectangle (scroll bar included).
    fn area(&self) -> Rect {
        let top = self.address.rect.bottom() + PAD;
        Rect::new(
            PAD,
            top,
            self.window.width as i32 - 2 * PAD,
            self.window.height as i32 - top - self.line() - 2 * PAD,
        )
    }

    fn cols(&self) -> usize {
        ((self.area().w - BAR - 8) / self.font.width).max(8) as usize
    }

    fn page_rows(&self) -> usize {
        ((self.area().h - 4) / self.line()).max(1) as usize
    }

    fn bar(&self) -> ScrollBar {
        let a = self.area();
        ScrollBar {
            rect: Rect::new(a.right() - BAR, a.y, BAR, a.h),
            value: self.scroll,
            total: self.rows.len(),
            page: self.page_rows(),
        }
    }

    /// Reads `url`: the page, a description for the status line, and the
    /// address it came from (after redirects).
    fn fetch(url: &str) -> Result<(Page, String, String), String> {
        if url == "stela:home" {
            return Ok((
                periplus::parse(HOME),
                String::from("the built-in page"),
                String::from(url),
            ));
        }
        if url.starts_with("http://") || url.starts_with("https://") {
            // https:// (P12): TLS 1.3 with the system's trusted roots.
            let r = http::get(url, 10_000_000_000).map_err(|e| String::from(e.name()))?;
            if r.status >= 400 {
                return Err(format!("the server answered {}", r.status));
            }
            let text = String::from_utf8_lossy(&r.body[..r.body.len().min(MAX)]).into_owned();
            let page = if r.is_html() {
                periplus::parse(&text)
            } else {
                periplus::plain(&text)
            };
            let what = format!("{} KiB, status {}", r.body.len().div_ceil(1024), r.status);
            return Ok((page, what, r.url));
        }
        // A local file.
        let name = url.strip_prefix("file:").unwrap_or(url);
        let loc = path::resolve(&Location::root("Data"), name.trim_start_matches("//"))
            .map_err(|_| String::from("not an address"))?;
        let root = fs::volume_open(&loc.volume).map_err(|e| String::from(e.name()))?;
        let h = fs::open(root, &loc.relative(), open::READ);
        let _ = handle_close(root);
        let h = h.map_err(|e| String::from(e.name()))?;
        let size = fs::stat(h).map(|s| s.size as usize).unwrap_or(0).min(MAX);
        let mut data = alloc::vec![0u8; size];
        let mut done = 0;
        while done < size {
            match fs::read(h, &mut data[done..], done as u64) {
                Ok(0) | Err(_) => break,
                Ok(n) => done += n,
            }
        }
        let _ = handle_close(h);
        data.truncate(done);
        let text = String::from_utf8_lossy(&data).into_owned();
        let lower = loc.name().to_ascii_lowercase();
        let page = if lower.ends_with(".html") || lower.ends_with(".htm") {
            periplus::parse(&text)
        } else {
            periplus::plain(&text)
        };
        Ok((
            page,
            format!("{} KiB, a local file", done.div_ceil(1024)),
            format!("{loc}"),
        ))
    }

    /// Goes to `url`; `remember` puts it in the history.
    fn go(&mut self, url: &str, remember: bool) {
        let url = String::from(url.trim());
        self.status = format!("Loading {url}...");
        self.draw();
        println!("periplus: loading {url}");
        match Browser::fetch(&url) {
            Ok((page, what, url)) => {
                println!("periplus: loaded {url} (title \"{}\")", page.title);
                self.status = format!("{url}: {what}");
                let title = if page.title.is_empty() {
                    String::from("Periplus")
                } else {
                    page.title.clone()
                };
                let _ = self.conn.set_title(&self.window, &title);
                self.page = page;
                self.layout();
                self.scroll = 0;
                self.focus = None;
                self.url = url.clone();
                self.address.text = url.clone();
                self.address.cursor = url.chars().count();
                if remember {
                    self.history.truncate(self.at + 1);
                    self.history.push(url);
                    self.at = self.history.len() - 1;
                }
            }
            Err(why) => {
                println!("periplus: {url}: {why}");
                self.status = format!("{url}: {why}");
            }
        }
    }

    fn layout(&mut self) {
        self.rows = periplus::layout(&self.page, self.cols());
    }

    fn back(&mut self) {
        if self.at > 0 {
            self.at -= 1;
            let url = self.history[self.at].clone();
            self.go(&url, false);
        }
    }

    fn forward(&mut self) {
        if self.at + 1 < self.history.len() {
            self.at += 1;
            let url = self.history[self.at].clone();
            self.go(&url, false);
        }
    }

    fn follow(&mut self, link: usize) {
        if let Some(href) = self.page.links.get(link) {
            let url = http::join(&self.url, href);
            self.go(&url, true);
        }
    }

    /// Selects the next (or previous) link and scrolls to it.
    fn tab(&mut self, back: bool) {
        let n = self.page.links.len();
        if n == 0 {
            return;
        }
        let next = match (self.focus, back) {
            (None, false) => 0,
            (None, true) => n - 1,
            (Some(i), false) => (i + 1) % n,
            (Some(i), true) => (i + n - 1) % n,
        };
        self.focus = Some(next);
        if let Some(row) = periplus::row_of_link(&self.rows, next) {
            let page = self.page_rows();
            if row < self.scroll || row >= self.scroll + page {
                self.scroll = row.saturating_sub(page / 3);
            }
        }
    }

    fn scroll_by(&mut self, rows: i64) {
        let mut bar = self.bar();
        bar.scroll(rows);
        self.scroll = bar.value;
    }

    fn draw(&mut self) {
        let font = self.font;
        let view = View {
            rows: &self.rows,
            scroll: self.scroll,
            focus: self.focus,
            area: self.area(),
            line: self.line(),
            page: self.page_rows(),
            bar: self.bar(),
        };
        let status_y = self.window.height as i32 - self.line() - PAD / 2;
        let status: String = self
            .status
            .chars()
            .take((self.window.width as i32 / font.width - 2).max(0) as usize)
            .collect();
        {
            let bounds = self.window.bounds();
            let mut c = self.window.canvas();
            c.fill(bounds, color::PARCHMENT);
            self.back.draw(&mut c, font);
            self.forward.draw(&mut c, font);
            self.reload.draw(&mut c, font);
            self.address.draw(&mut c, font, self.editing);
            view.draw(&mut c, font);
            c.text(PAD, status_y, &status, font, color::GREY, None);
        }
        let _ = self.conn.damage_all(&self.window);
    }

    /// Ctrl-L, File > Open Address: an empty address field takes the
    /// typing (Escape puts the address back).
    fn edit_address(&mut self) {
        self.editing = true;
        self.address.text.clear();
        self.address.cursor = 0;
    }

    /// Text the IME committed (P13 K3): into the address while editing it.
    fn text(&mut self, text: &str) {
        if self.editing {
            self.address.insert_text(text);
        }
    }

    fn key(&mut self, key: &KeyEvent) {
        if !key.pressed {
            return;
        }
        let m = key.modifiers;
        if m.ctrl && key.code == KeyCode::L {
            self.edit_address();
            return;
        }
        if self.editing {
            match key.code {
                KeyCode::Escape => {
                    self.editing = false;
                    self.address.text = self.url.clone();
                }
                _ => {
                    if self.address.key(key) == FieldResult::Enter {
                        self.editing = false;
                        let url = self.address.text.clone();
                        self.go(&url, true);
                    }
                }
            }
            return;
        }
        let page = self.page_rows() as i64;
        match key.code {
            KeyCode::Left if m.alt => self.back(),
            KeyCode::Right if m.alt => self.forward(),
            KeyCode::R if m.ctrl => {
                let url = self.url.clone();
                self.go(&url, false);
            }
            KeyCode::F5 => {
                let url = self.url.clone();
                self.go(&url, false);
            }
            KeyCode::Tab => self.tab(m.shift),
            KeyCode::Enter => {
                if let Some(link) = self.focus {
                    self.follow(link);
                }
            }
            KeyCode::Up => self.scroll_by(-1),
            KeyCode::Down => self.scroll_by(1),
            KeyCode::PageUp => self.scroll_by(-(page - 1).max(1)),
            KeyCode::PageDown | KeyCode::Space => self.scroll_by((page - 1).max(1)),
            KeyCode::Home => self.scroll = 0,
            KeyCode::End => self.scroll_by(i64::MAX / 2),
            _ => {}
        }
    }

    fn press(&mut self, x: i32, y: i32) {
        self.back.press(x, y);
        self.forward.press(x, y);
        self.reload.press(x, y);
        if self.address.rect.contains(x, y) {
            self.editing = true;
            return;
        }
        self.editing = false;
        let mut bar = self.bar();
        if bar.rect.contains(x, y) {
            if bar.click(x, y) {
                self.scroll = bar.value;
            }
            return;
        }
        let a = self.area();
        if !a.contains(x, y) {
            return;
        }
        let row = self.scroll + ((y - a.y - 2).max(0) / self.line()) as usize;
        let col = ((x - a.x - 4).max(0) / self.font.width) as usize;
        if let Some(link) = self.rows.get(row).and_then(|r| periplus::link_at(r, col)) {
            self.focus = Some(link);
            self.follow(link);
        }
    }

    fn release(&mut self, x: i32, y: i32) {
        if self.back.release(x, y) {
            self.back();
        } else if self.forward.release(x, y) {
            self.forward();
        } else if self.reload.release(x, y) {
            let url = self.url.clone();
            self.go(&url, false);
        }
    }
}

fn main() -> i64 {
    let args = libstela::process::args();
    let mut conn = match Connection::open() {
        Ok(c) => c,
        Err(e) => {
            println!("periplus: no window server: {}", e.name());
            return 1;
        }
    };
    let font = font_for(conn.screen.0);
    let line = font.height + 2;
    let w = COLS * font.width + 2 * PAD + BAR + 8;
    let h = ROWS * line + 3 * line + 4 * PAD;
    let Ok(window) =
        conn.create_window(w as u32, h as u32, flags::DOCUMENT, AUTO, AUTO, "Periplus")
    else {
        return 1;
    };
    let mut menus = MenuBar::new();
    menus.add("File", &["Open Address...^L", "Reload^R", "-", "Close^W"]);
    menus.add("Go", &["Back", "Forward", "Home"]);
    let _ = conn.set_menubar(&menus.to_text());
    let bh = font.height + 10;
    let bw = |label: &str| (label.chars().count() as i32 + 2) * font.width;
    let back = Button::new(Rect::new(PAD, PAD, bw("<"), bh), "<");
    let forward = Button::new(Rect::new(back.rect.right() + 4, PAD, bw(">"), bh), ">");
    let reload = Button::new(
        Rect::new(forward.rect.right() + 4, PAD, bw("Reload"), bh),
        "Reload",
    );
    let field_x = reload.rect.right() + 8;
    let address = TextField::new(Rect::new(field_x, PAD, w - field_x - PAD, bh), 512);
    let mut b = Browser {
        conn,
        window,
        font,
        address,
        editing: false,
        back,
        forward,
        reload,
        url: String::new(),
        page: Page::default(),
        rows: Vec::new(),
        scroll: 0,
        focus: None,
        status: String::new(),
        history: Vec::new(),
        at: 0,
    };
    let start = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| String::from("stela:home"));
    b.go(&start, true);
    // Started without an address: the address field takes the typing.
    if args.get(1).is_none() {
        b.edit_address();
    }
    b.draw();
    loop {
        let Some(event) = b.conn.next_event(FOREVER) else {
            continue;
        };
        match event {
            Event::Key { key, .. } => b.key(&key),
            Event::Text { text, .. } => b.text(&text),
            Event::Mouse { x, y, kind, .. } => match kind {
                MouseKind::Down => b.press(x, y),
                MouseKind::Up => b.release(x, y),
                MouseKind::Move => continue,
            },
            Event::Menu(0, 0) => b.edit_address(),
            Event::Menu(0, 1) => {
                let url = b.url.clone();
                b.go(&url, false);
            }
            Event::Menu(0, 3) | Event::CloseRequested(_) => break,
            Event::Menu(1, 0) => b.back(),
            Event::Menu(1, 1) => b.forward(),
            Event::Menu(1, 2) => b.go("stela:home", true),
            Event::ResizeRequested(_, w, h) => {
                if b.conn.resize(&mut b.window, w, h).is_ok() {
                    b.address.rect.w = w as i32 - b.address.rect.x - PAD;
                    b.layout();
                }
            }
            _ => continue,
        }
        if !b.conn.has_pending() {
            b.draw();
        }
    }
    let Browser { conn, window, .. } = b;
    let _ = conn.close(window);
    0
}
