// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The window server proper: the screen, the clients, the event loop.

use abi::key::{KeyCode, KeyEvent, MouseEvent, button};
use abi::sysinfo::{FORMAT_RGBX, FramebufferInfo};
use abi::window::{self as proto, event, flags, mouse, request, window_event};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use libstela::fs::{self, open};
use libstela::ipc::{
    Mapping, handle_close, handle_duplicate, input_attach, memory_create, port_create,
    port_receive, port_send,
};
use libstela::process::{self, pass};
use libstela::wait::{self, WaitSet};
use libstela::{Error, Handle, Message, Rights, bootstrap, println, tag};
use toolbox::draw::{Canvas, Font, Rect, color, font_for};
use windowserver::compose::{
    Clicks, Damage, Menu, Metrics, Part, Stack, Window, backdrop_rows, find_shortcut, ime_boxes,
    menu_box, menu_item_at, menu_titles, parse_audio, parse_display, parse_menus, shortcut,
    visible_parts, volume_at, volume_panel,
};

/// The pointer: `X` outline, `.` fill, space transparent (STELA's own
/// drawing).
const ARROW: [&str; 17] = [
    "X          ",
    "XX         ",
    "X.X        ",
    "X..X       ",
    "X...X      ",
    "X....X     ",
    "X.....X    ",
    "X......X   ",
    "X.......X  ",
    "X........X ",
    "X.....XXXXX",
    "X..X..X    ",
    "X.X X..X   ",
    "XX  X..X   ",
    "X    X..X  ",
    "     X..X  ",
    "      XX   ",
];

/// Wait-set keys: 0 is the main port, a client id its request port, the
/// id with this bit its reply port (watched for closing).
const REPLY_KEY: u64 = 1 << 32;
/// The wait-set key of the IME port (P13 K2): `CONNECT`s that come on it
/// may register as the input method.
const IME_PORT_KEY: u64 = 1 << 33;
/// How long a key waits for the IME before it goes to the window anyway
/// (and the IME is dropped).
const IME_TIMEOUT_NS: u64 = 1_000_000_000;
/// The IME program the server starts (`docs/ime.md`).
const IME_PROGRAM: &str = "ime";
/// The label at the left of the menu bar.
const LABEL: &str = "STELA";

/// A connected application.
struct Client {
    id: u32,
    /// SEND to the client's main port (replies and events).
    reply: Handle,
    /// The client's own request port (we receive; it has a SEND).
    port: Handle,
    menus: Vec<Menu>,
    /// Connected through the IME port: may send `REGISTER_IME`.
    may_ime: bool,
}

/// A key the IME has not answered yet.
struct PendingKey {
    seq: u32,
    key: KeyEvent,
    /// The window that had the focus when the key came.
    window: u32,
    sent: u64,
}

/// The registered input method (P13 K2) and what it is composing.
struct Ime {
    client: u32,
    chinese: bool,
    seq: u32,
    pending: VecDeque<PendingKey>,
    preedit: String,
    cursor: usize,
    candidates: Vec<String>,
    page: u16,
    pages: u16,
    /// Where the composition and the candidates were drawn last.
    boxes: (Rect, Rect),
}

impl Ime {
    fn composing(&self) -> bool {
        !self.preedit.is_empty() || !self.candidates.is_empty()
    }
}

/// A window's pixels, mapped read-only (the client draws them).
struct Surface {
    mapping: Mapping,
    width: i32,
}

/// What a held button is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    /// Moving window `id`: the outline's frame and the grab offset.
    Move {
        id: u32,
        outline: Rect,
        dx: i32,
        dy: i32,
    },
    /// Resizing window `id` with an outline.
    Grow { id: u32, outline: Rect },
    /// Pressing the close box of `id`.
    Close { id: u32, inside: bool },
    /// A press in window `id`'s content: moves and the release go to it.
    Content { id: u32 },
    /// A menu is pulled down.
    Menu { index: usize, hover: Option<usize> },
}

struct Server {
    fb: Mapping,
    fb_stride: usize,
    swap_rb: bool,
    width: i32,
    height: i32,
    back: Vec<u32>,
    font: &'static Font,
    m: Metrics,
    /// The pointer's size factor (2 with the 16x32 font).
    scale: i32,
    /// The protocol scale (P13 K13): physical pixels per logical one.
    /// Clients speak logical pixels; everything in here is physical.
    ui: i32,
    stack: Stack,
    surfaces: BTreeMap<u32, Surface>,
    clients: Vec<Client>,
    set: WaitSet,
    next_client: u32,
    next_window: u32,
    focus: Option<u32>,
    cursor: (i32, i32),
    buttons: u8,
    drag: Option<Drag>,
    clicks: Clicks,
    damage: Damage,
    /// The system clipboard (P11): UTF-8 text.
    clipboard: Vec<u8>,
    /// The menu bar's clock (`HH:MM`, local time; P11 K40) and when it
    /// changes next (`now_ns`).
    clock: String,
    next_clock: u64,
    /// Zoomed windows: where their content was before (P11 K43).
    zoomed: BTreeMap<u32, Rect>,
    /// The port the IME program connects through (P13 K2).
    ime_port: Handle,
    /// The input method, once it registered.
    ime: Option<Ime>,
    /// Where each window's text cursor is (`SET_CARET`: content x, y, h).
    carets: BTreeMap<u32, (i32, i32, i32)>,
    /// The sound output's master volume and mute (P13 K19), if there is
    /// a sound card; the speaker in the menu bar, its panel open, the
    /// slider held.
    volume: Option<(u8, bool)>,
    speaker: Rect,
    volume_open: bool,
    volume_drag: bool,
    /// The desktop (P13 K20): its colour, and one period of its rows when
    /// it has a pattern (`compose::backdrop_rows`).
    backdrop: u32,
    backdrop_rows: Vec<Vec<u32>>,
    /// P13 K21: when the last key or mouse event came (ns), how long
    /// without one blanks the screen (0: never; `Data:/.stela/display`),
    /// and whether it is blank now.
    last_input: u64,
    blank_after: u64,
    blanked: bool,
    /// P13 K22: Alt-Tab's list (the windows front first) and the one
    /// chosen, while Alt is held.
    switcher: Option<(Vec<u32>, usize)>,
}

fn screen_rect(s: &Server) -> Rect {
    Rect::new(0, 0, s.width, s.height)
}

/// A handle closed when dropped.
struct Owned(Handle);

impl Drop for Owned {
    fn drop(&mut self) {
        let _ = handle_close(self.0);
    }
}

impl Server {
    fn screen(&self) -> Rect {
        screen_rect(self)
    }

    fn mark(&mut self, r: Rect) {
        let screen = self.screen();
        self.damage.add(r, screen);
    }

    fn cursor_rect(&self) -> Rect {
        Rect::new(
            self.cursor.0,
            self.cursor.1,
            11 * self.scale,
            ARROW.len() as i32 * self.scale,
        )
    }

    fn client(&self, id: u32) -> Option<&Client> {
        self.clients.iter().find(|c| c.id == id)
    }

    /// `msg` with its words `words` changed by `f` (handles kept).
    fn rewrite(msg: &Message, words: &[usize], f: impl Fn(u32) -> u32) -> Message {
        let mut data = msg.payload().to_vec();
        for &i in words {
            if let Some(b) = data.get_mut(i * 4..i * 4 + 4) {
                let v = f(u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                b.copy_from_slice(&v.to_le_bytes());
            }
        }
        let mut out = Message::with_data(msg.tag, &data);
        for &h in msg.carried() {
            out.push_handle(h);
        }
        out
    }

    /// An event or reply in logical pixels (P13 K13).
    fn to_logical(&self, msg: &Message) -> Message {
        let ui = self.ui;
        if ui == 1 {
            return *msg;
        }
        let down = |v: u32| ((v as i32) / ui) as u32;
        match msg.tag {
            event::MOUSE => Self::rewrite(msg, &[1, 2], down),
            event::WINDOW_EVENT => Self::rewrite(msg, &[2, 3, 4, 5], down),
            event::WINDOW_CREATED | event::WINDOW_RESIZED => Self::rewrite(msg, &[1, 2], down),
            event::DROPPED => Self::rewrite(msg, &[1, 2], down),
            _ => *msg,
        }
    }

    /// A request in physical pixels (P13 K13).
    fn to_physical(&self, msg: &Message) -> Message {
        let ui = self.ui;
        if ui == 1 {
            return *msg;
        }
        let up = |v: u32| ((v as i32).saturating_mul(ui)) as u32;
        match msg.tag {
            request::CREATE_WINDOW => {
                // AUTO places stay AUTO; a desktop window of width 1 means
                // the whole screen.
                let desktop = proto::word(msg, 3) & flags::DESKTOP != 0;
                let mut words = alloc::vec![2usize];
                if !(desktop && proto::word(msg, 1) <= 1) {
                    words.push(1);
                }
                if proto::signed(msg, 4) != proto::AUTO {
                    words.push(4);
                }
                if proto::signed(msg, 5) != proto::AUTO {
                    words.push(5);
                }
                Self::rewrite(msg, &words, up)
            }
            request::DAMAGE => Self::rewrite(msg, &[2, 3, 4, 5], up),
            request::MOVE_WINDOW | request::RESIZE_WINDOW => Self::rewrite(msg, &[2, 3], up),
            request::SET_CARET => Self::rewrite(msg, &[2, 3, 4], up),
            request::DROP_FILE => Self::rewrite(msg, &[1, 2], up),
            _ => *msg,
        }
    }

    /// Sends `msg` to client `id`; a client that went away is removed.
    fn send(&mut self, id: u32, msg: &Message) {
        let Some(reply) = self.client(id).map(|c| c.reply) else {
            return;
        };
        let msg = &self.to_logical(msg);
        if port_send(reply, msg) == Err(Error::PeerClosed) {
            self.drop_client(id);
        }
    }

    fn drop_client(&mut self, id: u32) {
        let Some(at) = self.clients.iter().position(|c| c.id == id) else {
            return;
        };
        let client = self.clients.remove(at);
        if self.ime.as_ref().is_some_and(|i| i.client == id) {
            self.drop_ime();
        }
        let _ = self.set.remove(client.port);
        let _ = self.set.remove(client.reply);
        // Requests still queued go with the port (and what they carry).
        let _ = handle_close(client.port);
        let _ = handle_close(client.reply);
        for w in self.stack.remove_client(id) {
            self.carets.remove(&w.id);
            let frame = w.frame(&self.m);
            self.surfaces.remove(&w.id);
            self.mark(frame);
        }
        if self.focus.is_some_and(|f| self.stack.get(f).is_none()) {
            self.focus = None;
            let front = self.stack.front().map(|w| w.id);
            self.set_focus(front);
        }
        self.mark_menubar();
    }

    /// Reads the clock again and schedules the next change (the next
    /// minute).
    fn update_clock(&mut self) {
        let now = libstela::now_ns();
        match libstela::time::local_ns() {
            Some(local) => {
                self.clock = libstela::time::hours_minutes(local);
                let into_minute = local.rem_euclid(60_000_000_000) as u64;
                self.next_clock = now + (60_000_000_000 - into_minute) + 1_000_000;
            }
            None => self.next_clock = now + 60_000_000_000,
        }
        self.mark_menubar();
    }

    fn mark_menubar(&mut self) {
        let r = Rect::new(0, 0, self.width, self.m.menubar);
        self.mark(r);
    }

    fn window_owner(&self, id: u32) -> Option<u32> {
        self.stack.get(id).map(|w| w.client)
    }

    /// Gives the keyboard to `id` (and brings it to the front).
    fn set_focus(&mut self, id: Option<u32>) {
        if let Some(id) = id
            && self.stack.raise(id)
            && let Some(w) = self.stack.get(id)
        {
            let frame = w.frame(&self.m);
            self.mark(frame);
        }
        if self.focus == id {
            return;
        }
        if let Some(old) = self.focus.take()
            && let Some(w) = self.stack.get(old)
        {
            let (bar, client) = (w.title_bar(&self.m), w.client);
            self.mark(bar);
            self.send(
                client,
                &proto::window_event(old, window_event::BLUR, 0, 0, 0, 0),
            );
        }
        self.focus = id;
        self.reset_ime();
        if let Some(id) = id
            && let Some(w) = self.stack.get(id)
        {
            let (bar, client) = (w.title_bar(&self.m), w.client);
            self.mark(bar);
            self.send(
                client,
                &proto::window_event(id, window_event::FOCUS, 0, 0, 0, 0),
            );
        }
        self.mark_menubar();
    }

    /// The menus shown now: those of the focused window's client.
    fn menus(&self) -> &[Menu] {
        self.focus
            .and_then(|f| self.window_owner(f))
            .and_then(|c| self.client(c))
            .map_or(&[], |c| c.menus.as_slice())
    }

    fn label_width(&self) -> i32 {
        self.font.text_width(LABEL)
    }

    fn line_height(&self) -> i32 {
        self.font.height + 4
    }

    /// The open menu's box, if one is pulled down.
    fn open_menu_box(&self) -> Option<Rect> {
        let Some(Drag::Menu { index, .. }) = self.drag else {
            return None;
        };
        let menus = self.menus();
        let titles = menu_titles(menus, self.font.width, self.label_width());
        let menu = menus.get(index)?;
        Some(menu_box(
            menu,
            titles[index].0,
            &self.m,
            self.font.width,
            self.line_height(),
        ))
    }

    // ---- drawing ----

    /// Redraws `r` of the back buffer.
    fn compose(&mut self, r: Rect) {
        let (w, h) = (self.width as u32, self.height as u32);
        let font = self.font;
        let m = self.m;
        let windows: Vec<Window> = self.stack.iter().cloned().collect();
        // Occlusion culling (P11): every window (and the desktop colour)
        // is drawn only where no nearer window covers it.
        let frames: Vec<Rect> = windows.iter().map(|win| win.frame(&m)).collect();
        for part in visible_parts(r, &frames, frames.len()) {
            let mut canvas = Canvas::new(&mut self.back, w, h, w as usize);
            canvas.set_clip(part);
            if self.backdrop_rows.is_empty() {
                canvas.fill(part, self.backdrop);
                continue;
            }
            // P13 K20: the pattern's rows.
            let n = self.backdrop_rows.len();
            for y in part.y.max(0)..part.bottom().min(self.height) {
                let row = &self.backdrop_rows[y as usize % n];
                let (x0, x1) = (
                    part.x.max(0) as usize,
                    part.right().min(self.width) as usize,
                );
                let at = y as usize * w as usize;
                self.back[at + x0..at + x1].copy_from_slice(&row[x0..x1]);
            }
        }
        for (i, win) in windows.iter().enumerate() {
            let active = self.focus == Some(win.id);
            let pressed =
                matches!(self.drag, Some(Drag::Close { id, inside: true }) if id == win.id);
            for clip in visible_parts(r, &frames, i) {
                if !win.is_desktop() {
                    let mut canvas = Canvas::new(&mut self.back, w, h, w as usize);
                    canvas.set_clip(clip);
                    draw_frame(&mut canvas, win, &m, font, active, pressed);
                }
                // The content, row by row from the client's buffer.
                let part = win.content.intersect(&clip);
                if let Some(surface) = self.surfaces.get(&win.id)
                    && !part.is_empty()
                {
                    for y in part.y..part.bottom() {
                        let src = ((y - win.content.y) * surface.width + (part.x - win.content.x))
                            as usize;
                        let dst = (y * self.width + part.x) as usize;
                        let row = &mut self.back[dst..dst + part.w as usize];
                        if surface.mapping.read_words(src, row).is_err() {
                            row.fill(color::BLACK);
                        }
                    }
                }
                let grow = win.grow_box(&m);
                if !grow.is_empty() && grow.overlaps(&clip) {
                    let mut canvas = Canvas::new(&mut self.back, w, h, w as usize);
                    canvas.set_clip(clip);
                    draw_grow_box(&mut canvas, grow);
                }
            }
        }
        let menubar = Rect::new(0, 0, self.width, m.menubar);
        let menu_area = self.open_menu_box();
        let open = match self.drag {
            Some(Drag::Menu { index, hover }) => Some((index, hover)),
            _ => None,
        };
        let menus: Vec<Menu> = self.menus().to_vec();
        let label_width = self.label_width();
        let line = self.line_height();
        let outline = match self.drag {
            Some(Drag::Move { outline, .. } | Drag::Grow { outline, .. }) => Some(outline),
            _ => None,
        };
        let clock = self.clock.clone();
        let (ime_text, ime_list) = self.ime_layout();
        let switcher_area = self.switcher_box();
        let switcher = self.switcher.as_ref().map(|(ids, at)| {
            let titles: Vec<String> = ids
                .iter()
                .map(|&id| {
                    self.stack
                        .get(id)
                        .map(|w| w.title.clone())
                        .unwrap_or_default()
                })
                .collect();
            (titles, *at)
        });
        self.speaker = self.speaker_rect();
        let (volume, speaker, volume_open) = (self.volume, self.speaker, self.volume_open);
        let ime_status = self.ime.as_ref().map(|i| i.chinese);
        let ime_draw = self.ime.as_ref().filter(|i| i.composing()).map(|i| {
            (
                i.preedit.clone(),
                i.cursor,
                if i.candidates.is_empty() {
                    String::new()
                } else {
                    candidate_line(i)
                },
            )
        });
        let mut canvas = Canvas::new(&mut self.back, w, h, w as usize);
        canvas.set_clip(r);
        if menubar.overlaps(&r) {
            draw_menubar(
                &mut canvas,
                &m,
                font,
                &menus,
                open.map(|o| o.0),
                label_width,
            );
            // The clock at the right end (P11 K40).
            let x = w as i32 - font.text_width(&clock) - 2 * font.width;
            canvas.text(
                x,
                (m.menubar - font.height) / 2,
                &clock,
                font,
                color::BLACK,
                None,
            );
            // The IME's mode left of the clock (P13 K2): 注 or 英.
            let mut left = x;
            if let Some(chinese) = ime_status {
                let mark = if chinese { "注" } else { "英" };
                let mx = x - font.text_width(mark) - 2 * font.width;
                canvas.text(
                    mx,
                    (m.menubar - font.height) / 2,
                    mark,
                    font,
                    color::BLACK,
                    None,
                );
                left = mx;
            }
            // The speaker (P13 K19), left of that.
            if let Some((_, muted)) = volume {
                draw_speaker(&mut canvas, speaker, muted);
            }
            let _ = left;
        }
        if volume_open && let Some((level, muted)) = volume {
            let (panel, track, mute) = volume_panel(speaker, font.width, font.height + 4, w as i32);
            if panel.overlaps(&r) {
                canvas.fill(panel.offset(3, 3), color::BLACK);
                canvas.fill(panel, color::PARCHMENT);
                canvas.frame(panel, color::BLACK);
                let label = alloc::format!("Volume {level}");
                canvas.text(
                    panel.x + font.width,
                    panel.y + 4,
                    &label,
                    font,
                    color::BLACK,
                    None,
                );
                canvas.fill(track, color::STONE);
                canvas.frame(track, color::BLACK);
                let knob_x = track.x + track.w * i32::from(level) / 100;
                canvas.fill(
                    Rect::new(knob_x - 4, track.y - 6, 8, track.h + 12),
                    color::TERRACOTTA,
                );
                canvas.fill(mute, color::WHITE);
                canvas.frame(mute, color::BLACK);
                if muted {
                    canvas.line(
                        mute.x,
                        mute.y,
                        mute.right() - 1,
                        mute.bottom() - 1,
                        color::BLACK,
                    );
                    canvas.line(
                        mute.right() - 1,
                        mute.y,
                        mute.x,
                        mute.bottom() - 1,
                        color::BLACK,
                    );
                }
                canvas.text(
                    mute.right() + font.width,
                    mute.y,
                    "Mute",
                    font,
                    color::BLACK,
                    None,
                );
            }
        }
        // The composition under the caret, the candidates under it.
        if let Some((preedit, cursor, list)) = ime_draw {
            if ime_text.overlaps(&r) {
                canvas.fill(ime_text, color::PARCHMENT);
                canvas.frame(ime_text, color::BLACK);
                let (tx, ty) = (ime_text.x + 4, ime_text.y + 4);
                canvas.text(tx, ty, &preedit, font, color::BLACK, None);
                let tw = font.text_width(&preedit);
                canvas.hline(tx, ty + font.height - 1, tw, color::BLACK);
                let before: String = preedit.chars().take(cursor).collect();
                let cx = tx + font.text_width(&before);
                canvas.vline(cx, ty, font.height, color::BLACK);
            }
            if !list.is_empty() && ime_list.overlaps(&r) {
                canvas.fill(ime_list.offset(2, 2), color::BLACK);
                canvas.fill(ime_list, color::PARCHMENT);
                canvas.frame(ime_list, color::BLACK);
                canvas.text(
                    ime_list.x + 4,
                    ime_list.y + 4,
                    &list,
                    font,
                    color::BLACK,
                    None,
                );
            }
        }
        if let (Some(area), Some((index, hover))) = (menu_area, open)
            && area.overlaps(&r)
            && let Some(menu) = menus.get(index)
        {
            draw_menu(&mut canvas, menu, area, font, line, hover);
        }
        if let Some(outline) = outline {
            draw_outline(&mut canvas, outline);
        }
        // P13 K22: Alt-Tab's list.
        if let (Some(area), Some((ids, at))) = (switcher_area, switcher)
            && area.overlaps(&r)
        {
            canvas.fill(area.offset(3, 3), color::BLACK);
            canvas.fill(area, color::PARCHMENT);
            canvas.frame(area, color::BLACK);
            for (i, title) in ids.iter().enumerate() {
                let row = Rect::new(area.x + 4, area.y + 8 + i as i32 * line, area.w - 8, line);
                let (fg, bg) = if i == at {
                    canvas.fill(row, color::HIGHLIGHT);
                    (color::WHITE, Some(color::HIGHLIGHT))
                } else {
                    (color::BLACK, None)
                };
                canvas.text(row.x + 2 * font.width, row.y + 2, title, font, fg, bg);
            }
        }
    }

    /// Copies `r` of the back buffer to the screen, with the pointer.
    fn present(&mut self, r: Rect) {
        let r = r.intersect(&self.screen());
        if r.is_empty() {
            return;
        }
        let stride = self.fb_stride;
        let width = self.width;
        let swap = self.swap_rb;
        let cursor = self.cursor_rect();
        let (cx, cy) = self.cursor;
        let scale = self.scale;
        let hidden = self.hides_cursor(cx, cy);
        let Ok(fb) = self.fb.words_mut() else {
            return;
        };
        for y in r.y..r.bottom() {
            let src = &self.back[(y * width + r.x) as usize..(y * width + r.right()) as usize];
            let dst = &mut fb
                [y as usize * stride + r.x as usize..y as usize * stride + r.right() as usize];
            if swap {
                for (d, s) in dst.iter_mut().zip(src) {
                    *d = (s & 0x00FF00) | ((s >> 16) & 0xFF) | ((s & 0xFF) << 16);
                }
            } else {
                dst.copy_from_slice(src);
            }
        }
        if cursor.overlaps(&r) && !hidden {
            for (ry, row) in ARROW.iter().enumerate() {
                for (rx, c) in row.bytes().enumerate() {
                    let pixel = match c {
                        b'X' => color::BLACK,
                        b'.' => color::WHITE,
                        _ => continue,
                    };
                    for sy in 0..scale {
                        for sx in 0..scale {
                            let x = cx + rx as i32 * scale + sx;
                            let y = cy + ry as i32 * scale + sy;
                            if r.contains(x, y) {
                                fb[y as usize * stride + x as usize] = pixel;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Redraws and shows everything dirty.
    fn flush(&mut self) {
        if self.blanked {
            // Drawn again whole when it wakes.
            let _ = self.damage.take();
            return;
        }
        for r in self.damage.take() {
            self.compose(r);
            self.present(r);
        }
    }

    /// Blanks the screen (P13 K21: no input for `blank_after`, or
    /// `BLANK`): black until the next key or mouse event.
    fn blank(&mut self) {
        if self.blanked {
            return;
        }
        self.blanked = true;
        if let Ok(fb) = self.fb.words_mut() {
            fb.fill(0);
        }
        println!("windowserver: display asleep");
    }

    /// A key or mouse event came: true if it only woke the screen (it is
    /// not passed on, as on a Mac).
    fn wake(&mut self) -> bool {
        self.last_input = libstela::now_ns();
        if !self.blanked {
            return false;
        }
        self.blanked = false;
        let all = self.screen();
        self.mark(all);
        println!("windowserver: display awake");
        true
    }

    /// When the screen blanks if nothing comes (0: never).
    fn blank_deadline(&self) -> u64 {
        if self.blanked || self.blank_after == 0 {
            0
        } else {
            self.last_input.saturating_add(self.blank_after)
        }
    }

    // ---- input ----

    /// Alt-Tab (P13 K22): with Alt held, Tab (Shift-Tab back) goes through
    /// the windows, front first (focusing raises, so that is the order
    /// they were used in); letting go of Alt focuses the one chosen.
    /// Whether the key was this.
    fn switch_key(&mut self, key: &KeyEvent) -> bool {
        let alt = matches!(key.code, KeyCode::LeftAlt | KeyCode::RightAlt);
        if let Some((ids, at)) = self.switcher.clone() {
            if alt && !key.pressed {
                self.mark_switcher();
                self.switcher = None;
                if let Some(&id) = ids.get(at) {
                    let title = self
                        .stack
                        .get(id)
                        .map(|w| w.title.clone())
                        .unwrap_or_default();
                    println!("windowserver: switched to {title}");
                    self.set_focus(Some(id));
                }
                return true;
            }
            if key.pressed && key.code == KeyCode::Tab {
                let n = ids.len();
                let at = if key.modifiers.shift {
                    (at + n - 1) % n
                } else {
                    (at + 1) % n
                };
                self.switcher = Some((ids, at));
                self.mark_switcher();
                return true;
            }
            if key.pressed && key.code == KeyCode::Escape {
                self.mark_switcher();
                self.switcher = None;
                return true;
            }
            return !alt;
        }
        if !(key.pressed && key.modifiers.alt && key.code == KeyCode::Tab) {
            return false;
        }
        let mut ids: Vec<u32> = self
            .stack
            .iter()
            .filter(|w| !w.is_desktop())
            .map(|w| w.id)
            .collect();
        ids.reverse();
        if ids.len() < 2 {
            return true;
        }
        let at = if key.modifiers.shift {
            ids.len() - 1
        } else {
            1
        };
        self.switcher = Some((ids, at));
        self.mark_switcher();
        true
    }

    /// Where Alt-Tab's list is drawn: the middle of the screen.
    fn switcher_box(&self) -> Option<Rect> {
        let (ids, _) = self.switcher.as_ref()?;
        let font = self.font;
        let line = self.line_height();
        let widest = ids
            .iter()
            .filter_map(|&id| self.stack.get(id))
            .map(|w| font.text_width(&w.title))
            .max()
            .unwrap_or(0)
            .max(16 * font.width);
        let (w, h) = (widest + 4 * font.width, ids.len() as i32 * line + 16);
        Some(Rect::new((self.width - w) / 2, (self.height - h) / 2, w, h))
    }

    fn mark_switcher(&mut self) {
        if let Some(r) = self.switcher_box() {
            self.mark(r.inset(-4));
        }
    }

    /// The menus from the keyboard (P13 K22): Alt-M pulls down the first
    /// one; with a menu down the arrows move through
    /// the menus and their items, Enter chooses, Escape puts it back.
    /// Whether the key was this.
    fn menu_key(&mut self, key: &KeyEvent) -> bool {
        if !key.pressed {
            return matches!(self.drag, Some(Drag::Menu { .. }));
        }
        let n = self.menus().len();
        if n == 0 {
            return false;
        }
        let Some(Drag::Menu { index, hover }) = self.drag else {
            // Only Alt-M: Alt with the arrows belongs to the programs
            // (Periplus goes back and forward with them).
            let opens = key.modifiers.alt && !key.modifiers.ctrl && key.code == KeyCode::M;
            if opens {
                let first = self.first_item(0);
                self.pull_down(0, first);
            }
            return opens;
        };
        match key.code {
            KeyCode::Left | KeyCode::Right => {
                let next = if key.code == KeyCode::Left {
                    (index + n - 1) % n
                } else {
                    (index + 1) % n
                };
                let first = self.first_item(next);
                self.pull_down(next, first);
            }
            KeyCode::Up | KeyCode::Down => {
                let items = self.menus().get(index).map_or(0, |m| m.items.len());
                let mut at = hover;
                for _ in 0..items {
                    let i = match (at, key.code) {
                        (None, KeyCode::Up) => items.saturating_sub(1),
                        (None, _) => 0,
                        (Some(i), KeyCode::Up) => (i + items - 1) % items,
                        (Some(i), _) => (i + 1) % items,
                    };
                    at = Some(i);
                    if self.selectable(index, i) {
                        break;
                    }
                }
                self.pull_down(index, at);
            }
            KeyCode::Enter => {
                self.put_back_menu();
                if let (Some(item), Some(f)) = (hover, self.focus)
                    && let Some(client) = self.window_owner(f)
                {
                    self.send(client, &proto::menu_selected(index as u32, item as u32));
                }
            }
            KeyCode::Escape => self.put_back_menu(),
            _ => {}
        }
        true
    }

    /// Whether item `i` of menu `index` may be chosen (not a line).
    fn selectable(&self, index: usize, i: usize) -> bool {
        self.menus()
            .get(index)
            .and_then(|m| m.items.get(i))
            .is_some_and(|t| t != "-")
    }

    /// The first item of menu `index` that may be chosen.
    fn first_item(&self, index: usize) -> Option<usize> {
        let items = self.menus().get(index).map_or(0, |m| m.items.len());
        (0..items).find(|&i| self.selectable(index, i))
    }

    /// Pulls menu `index` down with `hover` lit.
    fn pull_down(&mut self, index: usize, hover: Option<usize>) {
        if let Some(area) = self.open_menu_box() {
            self.mark(area);
        }
        self.drag = Some(Drag::Menu { index, hover });
        if let Some(area) = self.open_menu_box() {
            self.mark(area);
        }
        self.mark_menubar();
    }

    /// Puts the pulled-down menu back.
    fn put_back_menu(&mut self) {
        if let Some(area) = self.open_menu_box() {
            self.mark(area);
        }
        self.drag = None;
        self.mark_menubar();
    }

    fn key(&mut self, key: KeyEvent) {
        // P13 K22: Alt-Tab, and the menus from the keyboard.
        if self.switch_key(&key) || self.menu_key(&key) {
            return;
        }
        // Alt + a letter that one of the focused client's menu items has
        // as its shortcut chooses that item (P11 K43).
        if key.pressed
            && key.modifiers.alt
            && !key.modifiers.ctrl
            && let Some(c) = abi::key::key_to_char(&KeyEvent {
                modifiers: abi::key::Modifiers::default(),
                ..key
            })
            && c.is_ascii_alphanumeric()
            && let Some((menu, item)) = find_shortcut(self.menus(), c)
            && let Some(client) = self.focus.and_then(|f| self.window_owner(f))
        {
            self.send(client, &proto::menu_selected(menu as u32, item as u32));
            return;
        }
        let Some(id) = self.focus else {
            return;
        };
        // P13 K2: the IME sees the key first, unless it is in English
        // mode with nothing composed and nothing waiting (then only
        // Ctrl-Space, which switches, goes to it).
        let toggle = key.code == abi::key::KeyCode::Space
            && key.modifiers.ctrl
            && !key.modifiers.alt
            && !key.modifiers.shift;
        let now = libstela::now_ns();
        let owner = self.window_owner(id);
        let route = match &mut self.ime {
            Some(ime)
                if owner != Some(ime.client)
                    && (ime.chinese || ime.composing() || !ime.pending.is_empty() || toggle) =>
            {
                ime.seq = ime.seq.wrapping_add(1).max(1);
                ime.pending.push_back(PendingKey {
                    seq: ime.seq,
                    key,
                    window: id,
                    sent: now,
                });
                Some((ime.client, ime.seq))
            }
            _ => None,
        };
        match route {
            Some((client, seq)) => self.send(client, &proto::ime_key(seq, &key)),
            None => self.deliver_key(id, key),
        }
    }

    /// Sends a key to window `id`'s client.
    fn deliver_key(&mut self, id: u32, key: KeyEvent) {
        if let Some(client) = self.window_owner(id) {
            self.send(client, &proto::key(id, &key));
        }
    }

    /// Sends committed text to window `id`'s client (`TEXT_INPUT`).
    fn deliver_text(&mut self, id: u32, text: &str) {
        if let Some(client) = self.window_owner(id) {
            let msgs: Vec<Message> = proto::text_input(id, text).collect();
            for m in &msgs {
                self.send(client, m);
            }
        }
    }

    /// The IME went away (or stopped answering): the keys it holds go to
    /// their windows as they are, the composition disappears.
    fn drop_ime(&mut self) {
        let Some(ime) = self.ime.take() else {
            return;
        };
        let (a, b) = ime.boxes;
        self.mark(a);
        self.mark(b);
        self.mark_menubar();
        for p in ime.pending {
            self.deliver_key(p.window, p.key);
        }
    }

    /// The focus moved: the IME drops what it composes.
    fn reset_ime(&mut self) {
        let Some(ime) = &mut self.ime else {
            return;
        };
        let client = ime.client;
        if !ime.composing() && ime.pending.is_empty() {
            return;
        }
        ime.preedit.clear();
        ime.candidates.clear();
        let (a, b) = ime.boxes;
        self.mark(a);
        self.mark(b);
        self.send(client, &proto::ime_reset());
    }

    /// When the oldest key sent to the IME times out (0: none waiting).
    fn ime_deadline(&self) -> u64 {
        self.ime
            .as_ref()
            .and_then(|i| i.pending.front())
            .map_or(0, |p| p.sent + IME_TIMEOUT_NS)
    }

    /// `REGISTER_IME`: only through the IME port, one at a time.
    fn register_ime(&mut self, client: u32) -> Result<(), Error> {
        if !self.client(client).is_some_and(|c| c.may_ime) {
            return Err(Error::AccessDenied);
        }
        if self.ime.is_some() {
            return Err(Error::Busy);
        }
        self.ime = Some(Ime {
            client,
            chinese: false,
            seq: 0,
            pending: VecDeque::new(),
            preedit: String::new(),
            cursor: 0,
            candidates: Vec::new(),
            page: 0,
            pages: 0,
            boxes: (Rect::new(0, 0, 0, 0), Rect::new(0, 0, 0, 0)),
        });
        self.mark_menubar();
        println!("windowserver: IME registered");
        Ok(())
    }

    /// `IME_RESULT`: the answer to a key (or a reset).
    fn ime_result(&mut self, client: u32, msg: &Message) -> Result<(), Error> {
        let memory = msg.carried().first().map(|&m| Owned(m));
        if !self.ime.as_ref().is_some_and(|i| i.client == client) {
            return Err(Error::AccessDenied);
        }
        let h = proto::ime_header_of(msg);
        let len = h.text_len();
        let text = if h.flags & proto::ime::MEMORY != 0 {
            if len > 64 * 1024 {
                return Err(Error::InvalidArgument);
            }
            let memory = memory.ok_or(Error::InvalidArgument)?;
            let size = (len as u64).next_multiple_of(4096).max(4096);
            let map = Mapping::new(memory.0, size, false)?;
            let mut t = vec![0u8; len];
            map.read(0, &mut t)?;
            t
        } else {
            let t = proto::ime_text_of(msg);
            t[..len.min(t.len())].to_vec()
        };
        let cut = |a: usize, b: usize| {
            String::from_utf8_lossy(text.get(a..b.min(text.len())).unwrap_or(&[])).into_owned()
        };
        let c = h.commit_len as usize;
        let p = c + h.preedit_len as usize;
        let commit = cut(0, c);
        let preedit = cut(c, p);
        let list = cut(p, len);
        // The keys answered: this one (and any older one the IME skipped,
        // which goes on as it is).
        let mut answered = None;
        let mut skipped = Vec::new();
        if let Some(ime) = &mut self.ime {
            if h.seq != 0 {
                while let Some(front) = ime.pending.front() {
                    if front.seq.wrapping_sub(h.seq) as i32 > 0 {
                        break;
                    }
                    let k = ime.pending.pop_front().ok_or(Error::NotFound)?;
                    if k.seq == h.seq {
                        answered = Some(k);
                        break;
                    }
                    skipped.push(k);
                }
            }
            ime.chinese = h.flags & proto::ime::CHINESE != 0;
            ime.preedit = preedit;
            ime.cursor = h.cursor as usize;
            ime.candidates = if list.is_empty() {
                Vec::new()
            } else {
                list.split('\n').map(String::from).collect()
            };
            ime.page = h.page;
            ime.pages = h.pages;
        }
        for k in skipped {
            self.deliver_key(k.window, k.key);
        }
        if let Some(k) = answered {
            if !commit.is_empty() {
                self.deliver_text(k.window, &commit);
            }
            if h.flags & proto::ime::CONSUMED == 0 {
                self.deliver_key(k.window, k.key);
            }
        }
        self.mark_ime();
        self.mark_menubar();
        Ok(())
    }

    /// `SET_CARET`.
    fn set_caret(&mut self, client: u32, msg: &Message) -> Result<(), Error> {
        let id = proto::word(msg, 1);
        self.owned(client, id)?;
        let h = (proto::word(msg, 4) as i32).clamp(1, 256);
        self.carets
            .insert(id, (proto::signed(msg, 2), proto::signed(msg, 3), h));
        if self.ime.as_ref().is_some_and(Ime::composing) && self.focus == Some(id) {
            self.mark_ime();
        }
        Ok(())
    }

    /// The composition's boxes now (empty when nothing is composed).
    fn ime_layout(&self) -> (Rect, Rect) {
        let none = (Rect::new(0, 0, 0, 0), Rect::new(0, 0, 0, 0));
        let Some(ime) = self.ime.as_ref().filter(|i| i.composing()) else {
            return none;
        };
        let Some(win) = self.focus.and_then(|f| self.stack.get(f)) else {
            return none;
        };
        let (cx, cy, ch) = self.carets.get(&win.id).copied().unwrap_or((
            4,
            win.content.h - self.font.height - 4,
            self.font.height,
        ));
        let caret = Rect::new(win.content.x + cx, win.content.y + cy, 1, ch);
        let text_w = self.font.text_width(&ime.preedit) + self.font.width;
        let list_w = if ime.candidates.is_empty() {
            0
        } else {
            self.font.text_width(&candidate_line(ime))
        };
        ime_boxes(
            caret,
            text_w,
            list_w,
            self.font.height,
            self.screen(),
            self.m.menubar,
        )
    }

    /// Where the speaker goes in the menu bar: left of the IME's mark (or
    /// of the clock).
    fn speaker_rect(&self) -> Rect {
        let f = self.font;
        let mut x = self.width - f.text_width(&self.clock) - 2 * f.width;
        if self.ime.is_some() {
            x -= f.text_width("注") + 2 * f.width;
        }
        let side = self.m.menubar - 8;
        Rect::new(x - side - 2 * f.width, 4, side, side)
    }

    /// The volume panel's rectangles, while it is open.
    fn volume_layout(&self) -> (Rect, Rect, Rect) {
        volume_panel(
            self.speaker,
            self.font.width,
            self.font.height + 4,
            self.width,
        )
    }

    /// Sets the master volume and mute (the sound driver, through POWER)
    /// and keeps them in `Data:/.stela/audio`.
    fn set_volume(&mut self, level: u8, muted: bool, save: bool) {
        self.volume = Some((level, muted));
        let _ = libstela::audio::set_master(level, muted);
        let (panel, ..) = self.volume_layout();
        self.mark(panel);
        self.mark_menubar();
        if save {
            println!(
                "windowserver: volume {level}{}",
                if muted { ", muted" } else { "" }
            );
            let text = alloc::format!("volume={level}\nmute={}\n", u8::from(muted));
            let _ = save_setting("audio", text.as_bytes());
        }
    }

    /// A press on the volume panel or the speaker; true if it was one.
    fn volume_press(&mut self, x: i32, y: i32) -> bool {
        let Some((level, muted)) = self.volume else {
            return false;
        };
        if self.volume_open {
            let (panel, track, mute) = self.volume_layout();
            if panel.contains(x, y) {
                if mute.inset(-4).contains(x, y) {
                    self.set_volume(level, !muted, true);
                } else if track.inset(-8).contains(x, y) {
                    self.volume_drag = true;
                    self.set_volume(volume_at(track, x), muted, false);
                }
                return true;
            }
            self.volume_open = false;
            self.mark(panel);
        }
        if self.speaker.contains(x, y) {
            println!("windowserver: volume panel");
            self.volume_open = true;
            let (panel, ..) = self.volume_layout();
            self.mark(panel);
            return true;
        }
        false
    }

    /// Marks the old and the new place of the composition.
    fn mark_ime(&mut self) {
        let new = self.ime_layout();
        let old = match &mut self.ime {
            Some(ime) => core::mem::replace(&mut ime.boxes, new),
            None => return,
        };
        if old.1.is_empty() && !new.1.is_empty() {
            // For the tests (`make test-ime`): where to look.
            let r = new.1;
            println!(
                "windowserver: IME candidates at {},{},{},{}",
                r.x, r.y, r.w, r.h
            );
        }
        self.mark(old.0);
        self.mark(old.1);
        self.mark(new.0);
        self.mark(new.1);
    }

    /// The zoom box (P11 K43): the window fills the screen below the menu
    /// bar, or goes back to where and how big it was; the server moves it
    /// and asks its client for the size (`ZOOM_REQUESTED`).
    fn zoom(&mut self, id: u32) {
        let Some(w) = self.stack.get(id) else {
            return;
        };
        let (before, client) = (w.frame(&self.m), w.client);
        let m = self.m;
        let target = match self.zoomed.remove(&id) {
            Some(saved) => saved,
            None => {
                self.zoomed.insert(id, w.content);
                Rect::new(
                    m.border,
                    m.menubar + m.title + m.border,
                    self.width - 2 * m.border,
                    self.height - m.menubar - m.title - 2 * m.border,
                )
            }
        };
        if let Some(w) = self.stack.get_mut(id) {
            w.content.x = target.x;
            w.content.y = target.y;
        }
        self.mark(before);
        if let Some(w) = self.stack.get(id) {
            let after = w.frame(&self.m);
            self.mark(after);
        }
        self.send(
            client,
            &proto::window_event(
                id,
                window_event::ZOOM_REQUESTED,
                target.x,
                target.y,
                target.w as u32,
                target.h as u32,
            ),
        );
    }

    fn pointer(&mut self, e: MouseEvent) {
        let old = self.cursor_rect();
        let (x, y) = (
            (self.cursor.0 + i32::from(e.dx)).clamp(0, self.width - 1),
            (self.cursor.1 + i32::from(e.dy)).clamp(0, self.height - 1),
        );
        let moved = (x, y) != self.cursor;
        self.cursor = (x, y);
        if moved {
            self.mark(old);
            let new = self.cursor_rect();
            self.mark(new);
        }
        let was = self.buttons & button::LEFT != 0;
        let down = e.buttons & button::LEFT != 0;
        self.buttons = e.buttons;
        match (was, down) {
            (false, true) => self.press(x, y, e.buttons),
            (true, false) => self.release(x, y, e.buttons),
            _ if moved && self.drag.is_none() => self.hover(x, y, e.buttons),
            _ if moved => self.drag_to(x, y, e.buttons),
            _ => {}
        }
        // The pointer may have gone into or out of a window that hides it.
        if moved && self.hides_cursor(old.x, old.y) != self.hides_cursor(x, y) {
            let new = self.cursor_rect();
            self.mark(old);
            self.mark(new);
        }
    }

    /// The window whose content is at (x, y) and has `flag`, if any.
    fn content_with(&self, x: i32, y: i32, flag: u32) -> Option<u32> {
        match self.stack.hit(x, y, &self.m) {
            Part::Content(id) if self.stack.get(id).is_some_and(|w| w.flags & flag != 0) => {
                Some(id)
            }
            _ => None,
        }
    }

    /// Whether the pointer at (x, y) is not drawn (`HIDE_CURSOR_INSIDE`).
    fn hides_cursor(&self, x: i32, y: i32) -> bool {
        self.drag.is_none() && self.content_with(x, y, flags::HIDE_CURSOR_INSIDE).is_some()
    }

    /// A move with no button held: to the window under the pointer if it
    /// asked for every move (`WANT_MOTION`).
    fn hover(&mut self, x: i32, y: i32, buttons: u8) {
        if let Some(id) = self.content_with(x, y, flags::WANT_MOTION) {
            let (cx, cy) = self.content_point(id, x, y);
            if let Some(client) = self.window_owner(id) {
                self.send(
                    client,
                    &proto::mouse_event(id, cx, cy, buttons, mouse::MOVE, 0),
                );
            }
        }
    }

    fn content_point(&self, id: u32, x: i32, y: i32) -> (i32, i32) {
        self.stack
            .get(id)
            .map_or((x, y), |w| (x - w.content.x, y - w.content.y))
    }

    fn press(&mut self, x: i32, y: i32, buttons: u8) {
        if self.volume_press(x, y) {
            return;
        }
        let part = self.stack.hit(x, y, &self.m);
        match part {
            Part::MenuBar => {
                let titles = menu_titles(self.menus(), self.font.width, self.label_width());
                if let Some(index) = titles.iter().position(|&(tx, tw)| x >= tx && x < tx + tw) {
                    self.drag = Some(Drag::Menu { index, hover: None });
                    self.mark_menubar();
                    if let Some(area) = self.open_menu_box() {
                        self.mark(area);
                    }
                }
            }
            Part::Title(id) => {
                self.set_focus(Some(id));
                if let Some(w) = self.stack.get(id) {
                    let outline = w.frame(&self.m);
                    self.drag = Some(Drag::Move {
                        id,
                        outline,
                        dx: x - outline.x,
                        dy: y - outline.y,
                    });
                    self.mark_outline(outline);
                }
            }
            Part::Zoom(id) => {
                self.set_focus(Some(id));
                self.zoom(id);
            }
            Part::Close(id) => {
                self.set_focus(Some(id));
                self.drag = Some(Drag::Close { id, inside: true });
                self.mark_close(id);
            }
            Part::Grow(id) => {
                self.set_focus(Some(id));
                if let Some(w) = self.stack.get(id) {
                    let outline = w.frame(&self.m);
                    self.drag = Some(Drag::Grow { id, outline });
                    self.mark_outline(outline);
                }
            }
            Part::Content(id) | Part::Border(id) => {
                if self.focus != Some(id) {
                    self.set_focus(Some(id));
                }
                let clicks = self.clicks.press(libstela::now_ns(), x, y);
                let (cx, cy) = self.content_point(id, x, y);
                if let Some(client) = self.window_owner(id) {
                    self.send(
                        client,
                        &proto::mouse_event(id, cx, cy, buttons, mouse::DOWN, clicks),
                    );
                }
                self.drag = Some(Drag::Content { id });
            }
            Part::Nothing => {}
        }
    }

    fn mark_outline(&mut self, r: Rect) {
        for edge in [
            Rect::new(r.x, r.y, r.w, 1),
            Rect::new(r.x, r.bottom() - 1, r.w, 1),
            Rect::new(r.x, r.y, 1, r.h),
            Rect::new(r.right() - 1, r.y, 1, r.h),
        ] {
            self.mark(edge);
        }
    }

    fn mark_close(&mut self, id: u32) {
        if let Some(w) = self.stack.get(id) {
            let r = w.close_box(&self.m);
            self.mark(r);
        }
    }

    fn drag_to(&mut self, x: i32, y: i32, buttons: u8) {
        if self.volume_drag
            && let Some((_, muted)) = self.volume
        {
            let (_, track, _) = self.volume_layout();
            self.set_volume(volume_at(track, x), muted, false);
            return;
        }
        match self.drag {
            Some(Drag::Move {
                id,
                outline,
                dx,
                dy,
            }) => {
                let new = Rect::new(x - dx, (y - dy).max(self.m.menubar), outline.w, outline.h);
                self.mark_outline(outline);
                self.mark_outline(new);
                self.drag = Some(Drag::Move {
                    id,
                    outline: new,
                    dx,
                    dy,
                });
            }
            Some(Drag::Grow { id, outline }) => {
                let min = 8 * self.m.grow;
                let new = Rect::new(
                    outline.x,
                    outline.y,
                    (x - outline.x + 4).max(min),
                    (y - outline.y + 4).max(min),
                );
                self.mark_outline(outline);
                self.mark_outline(new);
                self.drag = Some(Drag::Grow { id, outline: new });
            }
            Some(Drag::Close { id, inside }) => {
                let now = self
                    .stack
                    .get(id)
                    .is_some_and(|w| w.close_box(&self.m).contains(x, y));
                if now != inside {
                    self.drag = Some(Drag::Close { id, inside: now });
                    self.mark_close(id);
                }
            }
            Some(Drag::Content { id }) => {
                let (cx, cy) = self.content_point(id, x, y);
                if let Some(client) = self.window_owner(id) {
                    self.send(
                        client,
                        &proto::mouse_event(id, cx, cy, buttons, mouse::MOVE, 0),
                    );
                }
            }
            Some(Drag::Menu { index, hover }) => {
                let titles = menu_titles(self.menus(), self.font.width, self.label_width());
                if y < self.m.menubar
                    && let Some(new) = titles.iter().position(|&(tx, tw)| x >= tx && x < tx + tw)
                    && new != index
                {
                    if let Some(area) = self.open_menu_box() {
                        self.mark(area);
                    }
                    self.drag = Some(Drag::Menu {
                        index: new,
                        hover: None,
                    });
                    self.mark_menubar();
                    if let Some(area) = self.open_menu_box() {
                        self.mark(area);
                    }
                    return;
                }
                let item = self.open_menu_box().and_then(|area| {
                    self.menus()
                        .get(index)
                        .and_then(|m| menu_item_at(m, &area, x, y, self.line_height()))
                });
                if item != hover {
                    self.drag = Some(Drag::Menu { index, hover: item });
                    if let Some(area) = self.open_menu_box() {
                        self.mark(area);
                    }
                }
            }
            None => {}
        }
    }

    fn release(&mut self, x: i32, y: i32, buttons: u8) {
        if self.volume_drag {
            self.volume_drag = false;
            if let Some((level, muted)) = self.volume {
                self.set_volume(level, muted, true);
            }
            return;
        }
        self.drag_to(x, y, buttons | button::LEFT);
        let drag = self.drag.take();
        match drag {
            Some(Drag::Move { id, outline, .. }) => {
                self.mark_outline(outline);
                let m = self.m;
                if let Some(w) = self.stack.get_mut(id) {
                    let old = w.frame(&m);
                    let delta = (outline.x - old.x, outline.y - old.y);
                    w.content = w.content.offset(delta.0, delta.1);
                    let (new, content, client) = (w.frame(&m), w.content, w.client);
                    self.mark(old);
                    self.mark(new);
                    self.send(
                        client,
                        &proto::window_event(id, window_event::MOVED, content.x, content.y, 0, 0),
                    );
                }
            }
            Some(Drag::Grow { id, outline }) => {
                self.mark_outline(outline);
                if let Some(w) = self.stack.get(id) {
                    let frame = w.frame(&self.m);
                    let width = (w.content.w + outline.w - frame.w).max(1) as u32;
                    let height = (w.content.h + outline.h - frame.h).max(1) as u32;
                    let client = w.client;
                    self.send(
                        client,
                        &proto::window_event(
                            id,
                            window_event::RESIZE_REQUESTED,
                            0,
                            0,
                            width,
                            height,
                        ),
                    );
                }
            }
            Some(Drag::Close { id, inside }) => {
                self.mark_close(id);
                if inside && let Some(client) = self.window_owner(id) {
                    self.send(
                        client,
                        &proto::window_event(id, window_event::CLOSE_REQUESTED, 0, 0, 0, 0),
                    );
                }
            }
            Some(Drag::Content { id }) => {
                let (cx, cy) = self.content_point(id, x, y);
                if let Some(client) = self.window_owner(id) {
                    self.send(
                        client,
                        &proto::mouse_event(id, cx, cy, buttons, mouse::UP, 1),
                    );
                }
            }
            Some(Drag::Menu { index, hover }) => {
                // Close the menu first (its box is still computed from it).
                self.drag = Some(Drag::Menu { index, hover });
                if let Some(area) = self.open_menu_box() {
                    self.mark(area);
                }
                self.drag = None;
                self.mark_menubar();
                if let (Some(item), Some(f)) = (hover, self.focus)
                    && let Some(client) = self.window_owner(f)
                {
                    self.send(client, &proto::menu_selected(index as u32, item as u32));
                }
            }
            None => {}
        }
    }

    // ---- requests ----

    fn reply_error(&mut self, client: u32, request: u32, e: Error) {
        self.send(client, &proto::error(request, e.raw()));
    }

    /// A message on the main port: input, or a new client.
    fn main_message(&mut self, msg: &Message, from_ime_port: bool) {
        if from_ime_port {
            // Only CONNECT comes here (P13 K2).
            if msg.tag == request::CONNECT {
                self.connect(msg, true);
            } else {
                release(msg);
            }
            return;
        }
        match msg.tag {
            // P13 K21: the event that wakes the screen goes no further.
            tag::INPUT_KEY | tag::INPUT_MOUSE if self.wake() => {}
            tag::INPUT_KEY => {
                if let Some(key) = KeyEvent::decode(msg.payload()) {
                    self.key(key);
                }
            }
            tag::INPUT_MOUSE => {
                if let Some(e) = MouseEvent::decode(msg.payload()) {
                    self.pointer(e);
                }
            }
            request::CONNECT => self.connect(msg, false),
            // Requests belong on the client's own port.
            _ => release(msg),
        }
    }

    /// A request from `client` (known by the port it came on).
    fn request(&mut self, client: u32, msg: &Message) {
        if self.client(client).is_none() {
            release(msg);
            return;
        }
        let msg = &self.to_physical(msg);
        let result = match msg.tag {
            request::CREATE_WINDOW => self.create_window(client, msg),
            request::DAMAGE => self.damage_window(client, msg),
            request::MOVE_WINDOW => self.move_window(client, msg),
            request::RESIZE_WINDOW => self.resize_window(client, msg),
            request::SET_TITLE => self.set_title(client, msg),
            request::SET_MENUBAR => self.set_menubar(client, msg),
            request::CLOSE_WINDOW => self.close_window(client, msg),
            request::FOCUS_WINDOW => self.focus_window(client, msg),
            request::SET_CLIPBOARD => self.set_clipboard(msg),
            request::GET_CLIPBOARD => self.get_clipboard(client),
            request::REGISTER_IME => self.register_ime(client),
            request::IME_RESULT => self.ime_result(client, msg),
            request::SET_CARET => self.set_caret(client, msg),
            request::POWERBOX => self.powerbox(client, msg),
            request::BLANK => {
                self.blank();
                Ok(())
            }
            request::SET_BACKDROP => {
                self.set_backdrop(proto::word(msg, 1), proto::word(msg, 2));
                Ok(())
            }
            request::DROP_FILE => self.drop_file(client, msg),
            request::DISCONNECT => {
                self.drop_client(client);
                Ok(())
            }
            _ => {
                // CONNECT again, or something unknown.
                release(msg);
                Ok(())
            }
        };
        if let Err(e) = result {
            self.reply_error(client, msg.tag, e);
        }
    }

    /// `CONNECT`: a request port for the new client, watched with its
    /// reply port.
    fn connect(&mut self, msg: &Message, may_ime: bool) {
        let Some(&reply) = msg.carried().first() else {
            release(msg);
            return;
        };
        if proto::word(msg, 0) != proto::VERSION {
            let _ = port_send(
                reply,
                &proto::error(request::CONNECT, Error::NotSupported.raw()),
            );
            release(msg);
            return;
        }
        let Ok(port) = port_create() else {
            release(msg);
            return;
        };
        // TRANSFER: it travels in CONNECTED.
        let give = handle_duplicate(port, Rights::SEND | Rights::TRANSFER);
        let id = self.next_client;
        let added = give.is_ok()
            && self.set.add(port, u64::from(id), wait::READABLE).is_ok()
            && self
                .set
                .add(reply, u64::from(id) | REPLY_KEY, wait::CLOSED)
                .is_ok();
        let Ok(give) = give.and_then(|g| if added { Ok(g) } else { Err(Error::Full) }) else {
            let _ = self.set.remove(port);
            let _ = handle_close(port);
            release(msg);
            return;
        };
        self.next_client += 1;
        self.clients.push(Client {
            id,
            reply,
            port,
            menus: Vec::new(),
            may_ime,
        });
        // P13 K13: the screen in logical pixels and the scale.
        let ui = self.ui;
        let mut m = proto::connected(
            id,
            (self.width / ui) as u32,
            (self.height / ui) as u32,
            ui as u32,
            (self.m.menubar / ui) as u32,
        );
        m.push_handle(give);
        if port_send(reply, &m).is_err() {
            let _ = handle_close(give);
            self.drop_client(id);
        }
    }

    /// The window `id` if `client` owns it.
    fn owned(&self, client: u32, id: u32) -> Result<&Window, Error> {
        match self.stack.get(id) {
            Some(w) if w.client == client => Ok(w),
            Some(_) => Err(Error::AccessDenied),
            None => Err(Error::NotFound),
        }
    }

    /// A new buffer of `w` x `h` pixels: the server's read-only mapping
    /// and the handle for the client.
    fn surface(&self, w: i32, h: i32) -> Result<(Surface, Handle), Error> {
        let size = (w as u64 * h as u64 * 4).next_multiple_of(4096);
        let memory = memory_create(size)?;
        match Mapping::new(memory, size, false) {
            Ok(mapping) => Ok((Surface { mapping, width: w }, memory)),
            Err(e) => {
                let _ = handle_close(memory);
                Err(e)
            }
        }
    }

    fn create_window(&mut self, client: u32, msg: &Message) -> Result<(), Error> {
        let fl = proto::word(msg, 3);
        if fl & !flags::ALL != 0 {
            return Err(Error::InvalidArgument);
        }
        let title = String::from_utf8_lossy(proto::tail(msg, 6)).into_owned();
        let content = if fl & flags::DESKTOP != 0 {
            // P12 (docs/window.md §11): a desktop window as wide as asked
            // (Mouseion: the strip of volume icons) sits at the right
            // edge, full height; the rest of the desktop is the server's
            // background colour, without a canvas of its own. Width 1 (or
            // the screen's): the whole screen, as before.
            let w = proto::word(msg, 1) as i32;
            let w = if w > 1 && w < self.width {
                w
            } else {
                self.width
            };
            Rect::new(
                self.width - w,
                self.m.menubar,
                w,
                self.height - self.m.menubar,
            )
        } else {
            let (w, h) = (proto::word(msg, 1) as i32, proto::word(msg, 2) as i32);
            if !(1..=self.width).contains(&w) || !(1..=self.height).contains(&h) {
                return Err(Error::InvalidArgument);
            }
            let (mut x, mut y) = (proto::signed(msg, 4), proto::signed(msg, 5));
            if x == proto::AUTO || y == proto::AUTO {
                (x, y) = self.stack.place(&self.m, self.screen());
            }
            Rect::new(x, y.max(self.m.menubar + self.m.title), w, h)
        };
        let (surface, memory) = self.surface(content.w, content.h)?;
        let id = self.next_window;
        self.next_window += 1;
        let mut reply = proto::window_memory(
            event::WINDOW_CREATED,
            id,
            content.w as u32,
            content.h as u32,
        );
        reply.push_handle(memory);
        self.surfaces.insert(id, surface);
        let window = Window {
            id,
            client,
            content,
            flags: fl,
            title,
        };
        let frame = window.frame(&self.m);
        self.stack.add(window);
        self.send(client, &reply);
        self.mark(frame);
        let focus = if fl & flags::DESKTOP != 0 && self.stack.len() > 1 {
            self.focus
        } else {
            Some(id)
        };
        self.set_focus(focus);
        Ok(())
    }

    fn damage_window(&mut self, client: u32, msg: &Message) -> Result<(), Error> {
        let w = self.owned(client, proto::word(msg, 1))?;
        let r = Rect::new(
            proto::signed(msg, 2),
            proto::signed(msg, 3),
            proto::word(msg, 4) as i32,
            proto::word(msg, 5) as i32,
        )
        .intersect(&Rect::new(0, 0, w.content.w, w.content.h))
        .offset(w.content.x, w.content.y);
        self.mark(r);
        Ok(())
    }

    fn move_window(&mut self, client: u32, msg: &Message) -> Result<(), Error> {
        let id = proto::word(msg, 1);
        self.owned(client, id)?;
        let m = self.m;
        let (x, y) = (
            proto::signed(msg, 2),
            proto::signed(msg, 3).max(m.menubar + m.title),
        );
        if let Some(w) = self.stack.get_mut(id)
            && !w.is_desktop()
        {
            let old = w.frame(&m);
            w.content = Rect::new(x, y, w.content.w, w.content.h);
            let new = w.frame(&m);
            self.mark(old);
            self.mark(new);
        }
        Ok(())
    }

    fn resize_window(&mut self, client: u32, msg: &Message) -> Result<(), Error> {
        let id = proto::word(msg, 1);
        if self.owned(client, id)?.is_desktop() {
            return Err(Error::NotSupported);
        }
        let (w, h) = (proto::word(msg, 2) as i32, proto::word(msg, 3) as i32);
        if !(1..=self.width).contains(&w) || !(1..=self.height).contains(&h) {
            return Err(Error::InvalidArgument);
        }
        let (surface, memory) = self.surface(w, h)?;
        let m = self.m;
        let Some(win) = self.stack.get_mut(id) else {
            return Err(Error::NotFound);
        };
        let old = win.frame(&m);
        win.content = Rect::new(win.content.x, win.content.y, w, h);
        let new = win.frame(&m);
        self.surfaces.insert(id, surface);
        let mut reply = proto::window_memory(event::WINDOW_RESIZED, id, w as u32, h as u32);
        reply.push_handle(memory);
        self.send(client, &reply);
        self.mark(old);
        self.mark(new);
        Ok(())
    }

    fn set_title(&mut self, client: u32, msg: &Message) -> Result<(), Error> {
        let id = proto::word(msg, 1);
        self.owned(client, id)?;
        let m = self.m;
        let title = String::from_utf8_lossy(proto::tail(msg, 2)).into_owned();
        if let Some(w) = self.stack.get_mut(id) {
            w.title = title;
            let bar = w.title_bar(&m);
            self.mark(bar);
        }
        Ok(())
    }

    fn set_menubar(&mut self, client: u32, msg: &Message) -> Result<(), Error> {
        let Some(&memory) = msg.carried().first() else {
            return Err(Error::InvalidArgument);
        };
        let memory = Owned(memory);
        let len = proto::word(msg, 1) as usize;
        if len > 64 * 1024 {
            return Err(Error::InvalidArgument);
        }
        let size = (len as u64).next_multiple_of(4096).max(4096);
        let map = Mapping::new(memory.0, size, false)?;
        let mut text = vec![0u8; len];
        map.read(0, &mut text)?;
        let menus = parse_menus(&String::from_utf8_lossy(&text));
        if let Some(c) = self.clients.iter_mut().find(|c| c.id == client) {
            c.menus = menus;
        }
        self.mark_menubar();
        Ok(())
    }

    /// `SET_CLIPBOARD`: keeps a copy of the text.
    fn set_clipboard(&mut self, msg: &Message) -> Result<(), Error> {
        let len = proto::word(msg, 1) as usize;
        let memory = msg.carried().first().map(|&m| Owned(m));
        if len > proto::CLIPBOARD_MAX {
            return Err(Error::InvalidArgument);
        }
        let mut text = vec![0u8; len];
        if len > 0 {
            let memory = memory.ok_or(Error::InvalidArgument)?;
            let size = (len as u64).next_multiple_of(4096);
            let map = Mapping::new(memory.0, size, false)?;
            map.read(0, &mut text)?;
        }
        self.clipboard = text;
        Ok(())
    }

    /// `GET_CLIPBOARD`: a copy of the text in a new Memory.
    fn get_clipboard(&mut self, client: u32) -> Result<(), Error> {
        let len = self.clipboard.len();
        let mut reply = proto::clipboard(len as u32);
        if len > 0 {
            let size = (len as u64).next_multiple_of(4096);
            let memory = memory_create(size)?;
            {
                let map = Mapping::new(memory, size, true)?;
                map.write(0, &self.clipboard)?;
            }
            reply.push_handle(memory);
        }
        self.send(client, &reply);
        Ok(())
    }

    /// The desktop's pattern and colour (0: the usual one; P13 K20).
    fn set_backdrop(&mut self, pattern: u32, colour: u32) {
        self.backdrop = if colour == 0 { color::DESKTOP } else { colour };
        self.backdrop_rows = backdrop_rows(pattern, self.backdrop, self.width as usize, self.ui);
        println!(
            "windowserver: desktop {}",
            abi::window::backdrop::name(pattern)
        );
        let all = self.screen();
        self.mark(all);
    }

    /// `POWERBOX` (P13 K16, `docs/sandbox.md`): the file dialog for a
    /// program that may not open the volumes itself. The server starts
    /// `Initrd:/powerbox` with `VOLUMES` and the SEND to the client's port
    /// that came with the request (its `[6]`); it shows the dialog and
    /// sends the client the file.
    fn powerbox(&mut self, client: u32, msg: &Message) -> Result<(), Error> {
        // The client's SEND to its own port comes with the request.
        let Some(&reply) = msg.carried().first() else {
            return Err(Error::InvalidArgument);
        };
        // Moved to the powerbox (closed here if it cannot start).
        let reply = Owned(reply);
        let (mode, [title, name, start]) = proto::powerbox_of(msg);
        let mode = if mode == proto::powerbox::SAVE {
            "save"
        } else {
            "open"
        };
        let initrd = Owned(fs::volume_open("Initrd")?);
        let file = Owned(fs::open(initrd.0, POWERBOX_PROGRAM, open::READ)?);
        let handles = [
            pass(bootstrap::CONSOLE, Rights::SEND | Rights::DUPLICATE),
            pass(
                bootstrap::MAIN_PORT,
                Rights::SEND | Rights::DUPLICATE | Rights::TRANSFER,
            ),
            process::pass_dir(bootstrap::CWD),
            Handle::NONE,
            Handle::NONE,
            reply.0,
        ];
        let caps: Vec<(u32, Handle)> = process::pass_capability(abi::capability::VOLUMES)
            .map(|v| (abi::capability::VOLUMES, v))
            .into_iter()
            .collect();
        let p = process::spawn_with(
            file.0,
            &[POWERBOX_PROGRAM, mode, title, name, start],
            &handles,
            &caps,
        )?;
        core::mem::forget(reply);
        let _ = handle_close(p);
        println!("windowserver: powerbox ({mode}) for client {client}");
        Ok(())
    }

    /// `DROP_FILE` (P13 K16): the file Mouseion dropped goes to the window
    /// under (x, y) as `DROPPED`, if another client's.
    fn drop_file(&mut self, client: u32, msg: &Message) -> Result<(), Error> {
        let (x, y) = (proto::signed(msg, 1), proto::signed(msg, 2));
        let target = match self.stack.hit(x, y, &self.m) {
            Part::Content(id) => self.window_owner(id).map(|c| (id, c)),
            _ => None,
        };
        let Some((id, owner)) = target.filter(|&(_, c)| c != client) else {
            release(msg);
            return Ok(());
        };
        let Some(&file) = msg.carried().first() else {
            return Ok(());
        };
        let (cx, cy) = self.content_point(id, x, y);
        let path = proto::tail(msg, 3);
        let mut out = proto::dropped(id, cx, cy, path);
        out.push_handle(file);
        println!(
            "windowserver: dropped {} on window {id}",
            String::from_utf8_lossy(path)
        );
        self.send(owner, &out);
        Ok(())
    }

    fn close_window(&mut self, client: u32, msg: &Message) -> Result<(), Error> {
        let id = proto::word(msg, 1);
        self.owned(client, id)?;
        if let Some(w) = self.stack.remove(id) {
            let frame = w.frame(&self.m);
            self.mark(frame);
        }
        self.surfaces.remove(&id);
        self.zoomed.remove(&id);
        self.carets.remove(&id);
        if matches!(self.drag, Some(Drag::Move { id: d, .. } | Drag::Grow { id: d, .. } | Drag::Close { id: d, .. } | Drag::Content { id: d }) if d == id)
        {
            self.drag = None;
        }
        if self.focus == Some(id) {
            self.focus = None;
            let front = self.stack.front().map(|w| w.id);
            self.set_focus(front);
        }
        Ok(())
    }

    fn focus_window(&mut self, client: u32, msg: &Message) -> Result<(), Error> {
        let id = proto::word(msg, 1);
        self.owned(client, id)?;
        self.set_focus(Some(id));
        Ok(())
    }

    /// Handles up to `max` messages from `port` (the main port, or
    /// client `client`'s).
    fn drain(
        &mut self,
        port: Handle,
        client: Option<u32>,
        max: usize,
        from_ime_port: bool,
    ) -> Result<(), Error> {
        let mut msg = Message::new();
        for _ in 0..max {
            match port_receive(port, &mut msg, 0) {
                Ok(()) => {}
                Err(Error::TimedOut) => return Ok(()),
                Err(e) => return Err(e),
            }
            match client {
                None => self.main_message(&msg, from_ime_port),
                Some(id) => {
                    self.request(id, &msg);
                    if self.client(id).is_none() {
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }
}

/// Closes the handles a message we do not use carries.
fn release(msg: &Message) {
    for &h in msg.carried() {
        let _ = handle_close(h);
    }
}

fn draw_frame(c: &mut Canvas, w: &Window, m: &Metrics, font: &Font, active: bool, pressed: bool) {
    let frame = w.frame(m);
    c.frame(frame, color::BLACK);
    let bar = w.title_bar(m);
    if bar.is_empty() {
        return;
    }
    c.fill(
        bar,
        if active {
            color::TERRACOTTA
        } else {
            color::STONE
        },
    );
    c.hline(bar.x, bar.bottom() - 1, bar.w, color::BLACK);
    if active {
        // Two thin glaze lines along the bar, as on a painted band.
        let y = bar.y + bar.h / 2;
        c.hline(bar.x + 4, y - 3, bar.w - 8, color::BLACK);
        c.hline(bar.x + 4, y + 2, bar.w - 8, color::BLACK);
    }
    let tw = font.text_width(&w.title).min(bar.w - 4 * m.close);
    let tx = bar.x + (bar.w - tw) / 2;
    let ty = bar.y + (bar.h - font.height) / 2;
    c.fill(
        Rect::new(tx - 6, bar.y + 1, tw + 12, bar.h - 2),
        color::PARCHMENT,
    );
    let mut clip = c.clip();
    clip = clip.intersect(&Rect::new(tx, bar.y, tw, bar.h));
    let saved = c.clip();
    c.set_clip(clip);
    c.text(tx, ty, &w.title, font, color::BLACK, None);
    c.set_clip(saved);
    let close = w.close_box(m);
    if !close.is_empty() {
        c.fill(
            close.inset(-2),
            if active {
                color::TERRACOTTA
            } else {
                color::STONE
            },
        );
        c.fill(
            close,
            if pressed {
                color::BLACK
            } else {
                color::PARCHMENT
            },
        );
        c.frame(close, color::BLACK);
    }
    // The zoom box (P11 K43): a box with a smaller one in its corner.
    let zoom = w.zoom_box(m);
    if !zoom.is_empty() {
        c.fill(
            zoom.inset(-2),
            if active {
                color::TERRACOTTA
            } else {
                color::STONE
            },
        );
        c.fill(zoom, color::PARCHMENT);
        c.frame(zoom, color::BLACK);
        c.frame(
            Rect::new(zoom.x, zoom.y, zoom.w / 2 + 1, zoom.h / 2 + 1),
            color::BLACK,
        );
    }
}

fn draw_grow_box(c: &mut Canvas, r: Rect) {
    c.fill(r, color::PARCHMENT);
    c.frame(r, color::BLACK);
    let q = r.w / 4;
    c.frame(Rect::new(r.x + q, r.y + q, r.w / 2, r.h / 2), color::BLACK);
    c.frame(
        Rect::new(r.x + q / 2, r.y + q / 2, r.w / 3, r.h / 3),
        color::BLACK,
    );
}

fn draw_menubar(
    c: &mut Canvas,
    m: &Metrics,
    font: &Font,
    menus: &[Menu],
    open: Option<usize>,
    label_width: i32,
) {
    let (w, _) = c.size();
    let bar = Rect::new(0, 0, w, m.menubar);
    c.fill(bar, color::PARCHMENT);
    c.hline(0, m.menubar - 1, w, color::BLACK);
    let ty = (m.menubar - font.height) / 2;
    // The label, drawn twice one pixel apart: bold.
    c.text(8, ty, LABEL, font, color::BLACK, None);
    c.text(9, ty, LABEL, font, color::BLACK, None);
    let _ = label_width;
    for (i, (menu, (x, tw))) in menus
        .iter()
        .zip(menu_titles(menus, font.width, font.text_width(LABEL)))
        .enumerate()
    {
        if open == Some(i) {
            c.fill(Rect::new(x, 0, tw, m.menubar - 1), color::HIGHLIGHT);
            c.text(
                x + font.width,
                ty,
                &menu.title,
                font,
                color::PARCHMENT,
                None,
            );
        } else {
            c.text(x + font.width, ty, &menu.title, font, color::BLACK, None);
        }
    }
}

fn draw_menu(
    c: &mut Canvas,
    menu: &Menu,
    area: Rect,
    font: &Font,
    line: i32,
    hover: Option<usize>,
) {
    c.fill(area.offset(3, 3), color::BLACK);
    c.fill(area, color::PARCHMENT);
    c.frame(area, color::BLACK);
    for (i, item) in menu.items.iter().enumerate() {
        let y = area.y + 2 + i as i32 * line;
        if item == "-" {
            c.hline(area.x + 2, y + line / 2, area.w - 4, color::GREY);
            continue;
        }
        let (fg, bg) = if hover == Some(i) {
            (color::PARCHMENT, Some(color::HIGHLIGHT))
        } else {
            (color::BLACK, None)
        };
        if let Some(bg) = bg {
            c.fill(Rect::new(area.x + 1, y, area.w - 2, line), bg);
        }
        let (label, key) = shortcut(item);
        c.text(area.x + 2 * font.width, y + 2, label, font, fg, None);
        if let Some(k) = key {
            // The shortcut at the right: Alt is Command (P11 K43).
            let hint = alloc::format!("Alt-{k}");
            let x = area.right() - font.text_width(&hint) - font.width;
            c.text(x, y + 2, &hint, font, fg, None);
        }
    }
}

/// The menu bar's speaker (P13 K19): a box and a cone, crossed out when
/// muted.
fn draw_speaker(c: &mut Canvas, r: Rect, muted: bool) {
    let body = Rect::new(r.x + r.w / 6, r.y + r.h / 3, r.w / 4, r.h / 3);
    c.fill(body, color::BLACK);
    let cone_x = body.right();
    for i in 0..r.w / 3 {
        let half = r.h / 6 + i * (r.h / 3) / (r.w / 3).max(1);
        c.vline(cone_x + i, r.y + r.h / 2 - half, 2 * half, color::BLACK);
    }
    if muted {
        c.line(r.x, r.y, r.right() - 1, r.bottom() - 1, color::HIGHLIGHT);
        c.line(r.x + 1, r.y, r.right(), r.bottom() - 1, color::HIGHLIGHT);
    } else {
        let wave = cone_x + r.w / 3 + 2;
        c.vline(wave, r.y + r.h / 3, r.h / 3, color::BLACK);
        c.vline(wave + 3, r.y + r.h / 4, r.h / 2, color::BLACK);
    }
}

/// Writes `Data:/.stela/NAME` (made if needed).
fn save_setting(name: &str, data: &[u8]) -> Result<(), Error> {
    let root = Owned(fs::volume_open("Data")?);
    let dir = Owned(fs::open(
        root.0,
        ".stela",
        open::READ | open::WRITE | open::CREATE | open::DIRECTORY,
    )?);
    let file = Owned(fs::open(
        dir.0,
        name,
        open::READ | open::WRITE | open::CREATE | open::TRUNCATE,
    )?);
    fs::write(file.0, data, 0).map(|_| ())
}

fn draw_outline(c: &mut Canvas, r: Rect) {
    // A dotted outline (every other pixel).
    for x in (r.x..r.right()).step_by(2) {
        c.pixel(x, r.y, color::BLACK);
        c.pixel(x, r.bottom() - 1, color::BLACK);
    }
    for y in (r.y..r.bottom()).step_by(2) {
        c.pixel(r.x, y, color::BLACK);
        c.pixel(r.right() - 1, y, color::BLACK);
    }
}

/// Starts the programs named in the arguments, from `Initrd:/`, with the
/// server's main port as their bootstrap handle.
fn start_programs(names: &[String]) {
    let Ok(initrd) = fs::volume_open("Initrd") else {
        return;
    };
    let initrd = Owned(initrd);
    for name in names {
        let Ok(file) = fs::open(initrd.0, name, open::READ) else {
            println!("windowserver: no {name} in Initrd:/");
            continue;
        };
        let file = Owned(file);
        let handles = [
            pass(bootstrap::CONSOLE, Rights::SEND | Rights::DUPLICATE),
            pass(
                bootstrap::MAIN_PORT,
                Rights::SEND | Rights::DUPLICATE | Rights::TRANSFER,
            ),
            process::pass_dir(bootstrap::CWD),
        ];
        let base = name.rsplit('/').next().unwrap_or(name);
        let base = base.strip_suffix(".app").unwrap_or(base);
        // The first programs (Mouseion) may turn the machine off.
        let mut caps = process::inherited();
        if let Some(p) = process::pass_capability(abi::capability::POWER) {
            caps.push((abi::capability::POWER, p));
        }
        match process::spawn_with(file.0, &[base], &handles, &caps) {
            Ok(p) => {
                let _ = handle_close(p);
            }
            Err(e) => println!("windowserver: cannot start {name}: {}", e.name()),
        }
    }
}

/// The display sleeps after 10 minutes without input unless
/// `Data:/.stela/display` says otherwise (P13 K21).
const DEFAULT_BLANK_NS: u64 = 10 * 60 * 1_000_000_000;

/// The powerbox's program in `Initrd:/` (P13 K16).
const POWERBOX_PROGRAM: &str = "powerbox";

/// Starts `Initrd:/ime` (if it is there) with a SEND to the IME port as
/// its bootstrap handle (P13 K2).
fn start_ime(port: Handle) {
    let Ok(initrd) = fs::volume_open("Initrd") else {
        return;
    };
    let initrd = Owned(initrd);
    let Ok(file) = fs::open(initrd.0, IME_PROGRAM, open::READ) else {
        return;
    };
    let file = Owned(file);
    let handles = [
        pass(bootstrap::CONSOLE, Rights::SEND | Rights::DUPLICATE),
        pass(port, Rights::SEND | Rights::DUPLICATE),
        process::pass_dir(bootstrap::CWD),
    ];
    match process::spawn_with(file.0, &[IME_PROGRAM], &handles, &process::inherited()) {
        Ok(p) => {
            let _ = handle_close(p);
        }
        Err(e) => println!("windowserver: cannot start the IME: {}", e.name()),
    }
}

/// The candidate list as one line: `1.你 2.妳 … 1/3`.
fn candidate_line(ime: &Ime) -> String {
    let mut line = String::new();
    for (i, c) in ime.candidates.iter().enumerate() {
        if i > 0 {
            line.push(' ');
        }
        line.push_str(&alloc::format!("{}.{c}", (i + 1) % 10));
    }
    if ime.pages > 1 {
        line.push_str(&alloc::format!("  {}/{}", ime.page + 1, ime.pages));
    }
    line
}

pub fn main() -> i64 {
    let Some(info) = process::framebuffer_info() else {
        println!("windowserver: no framebuffer");
        return 1;
    };
    let FramebufferInfo {
        width,
        height,
        pitch,
        format,
        size,
        ..
    } = info;
    println!("windowserver: scale {}", info.scale.clamp(1, 2));
    let fb = match Mapping::new(bootstrap::BOOTSTRAP, u64::from(size), true) {
        Ok(fb) => fb,
        Err(e) => {
            println!(
                "windowserver: cannot map the framebuffer ([2]): {}",
                e.name()
            );
            return 1;
        }
    };
    let set = match WaitSet::new() {
        Ok(set) => set,
        Err(e) => {
            println!("windowserver: no wait set: {}", e.name());
            return 1;
        }
    };
    if let Err(e) = set.add(bootstrap::MAIN_PORT, 0, wait::READABLE) {
        println!("windowserver: cannot wait on the main port: {}", e.name());
        return 1;
    }
    // P13 K13: the scale the kernel chose (boot option scale=, or 2 from
    // 2560 pixels wide); at 2 the server's own text is the 16x32 font,
    // as the clients' is.
    let ui = info.scale.clamp(1, 2) as i32;
    let font = if ui == 2 {
        &toolbox::draw::LARGE
    } else {
        font_for(width)
    };
    let mut server = Server {
        fb,
        fb_stride: pitch as usize / 4,
        swap_rb: format == FORMAT_RGBX,
        width: width as i32,
        height: height as i32,
        back: vec![color::DESKTOP; width as usize * height as usize],
        font,
        m: Metrics::for_font(font.height),
        scale: if font.height >= 32 { 2 } else { 1 },
        ui,
        stack: Stack::new(),
        surfaces: BTreeMap::new(),
        clients: Vec::new(),
        set,
        next_client: 1,
        next_window: 1,
        focus: None,
        cursor: (width as i32 / 2, height as i32 / 2),
        buttons: 0,
        drag: None,
        clicks: Clicks::default(),
        damage: Damage::new(),
        clipboard: Vec::new(),
        clock: String::new(),
        next_clock: 0,
        zoomed: BTreeMap::new(),
        ime_port: Handle::NONE,
        ime: None,
        carets: BTreeMap::new(),
        volume: libstela::audio::master().map(|(v, m, _)| (v, m)),
        speaker: Rect::new(0, 0, 0, 0),
        volume_open: false,
        volume_drag: false,
        backdrop: color::DESKTOP,
        backdrop_rows: Vec::new(),
        last_input: libstela::now_ns(),
        blank_after: DEFAULT_BLANK_NS,
        blanked: false,
        switcher: None,
    };
    // P13 K21: the display's sleep (blank=MINUTES, or Ns for seconds).
    if let Ok(text) = fs::read_path("Data:/.stela/display")
        && let Some(after) = parse_display(&String::from_utf8_lossy(&text))
    {
        server.blank_after = after;
    }
    // P13 K20: the desktop's pattern.
    if let Ok(text) = fs::read_path("Data:/.stela/desktop") {
        let (pattern, colour) = toolbox::backdrop::parse(&String::from_utf8_lossy(&text));
        server.set_backdrop(pattern, colour.unwrap_or(0));
    }
    // P13 K19: the volume kept from last time.
    if server.volume.is_some()
        && let Ok(text) = fs::read_path("Data:/.stela/audio")
    {
        let (level, muted) = parse_audio(&String::from_utf8_lossy(&text));
        server.set_volume(level, muted, false);
    }
    // P13 K2: the IME's own port, and the IME.
    match port_create() {
        Ok(port) => {
            if server.set.add(port, IME_PORT_KEY, wait::READABLE).is_ok() {
                server.ime_port = port;
                start_ime(port);
            } else {
                let _ = handle_close(port);
            }
        }
        Err(e) => println!("windowserver: no IME port: {}", e.name()),
    }
    if let Err(e) = input_attach(bootstrap::MAIN_PORT) {
        println!(
            "windowserver: cannot take the keyboard and mouse: {}",
            e.name()
        );
    }
    let all = server.screen();
    server.mark(all);
    server.flush();
    println!("windowserver: {width}x{height}, ready");
    let args = process::args();
    start_programs(args.get(1..).unwrap_or(&[]));
    let mut ready = [wait::Ready::default(); 16];
    loop {
        let now = libstela::now_ns();
        if now >= server.next_clock {
            server.update_clock();
        }
        // P13 K2: an IME that does not answer in time is dropped (the
        // keys it holds go on).
        let deadline = server.ime_deadline();
        if deadline != 0 && now >= deadline {
            println!("windowserver: the IME does not answer; keys go straight to the windows");
            server.drop_ime();
        }
        let deadline = server.ime_deadline();
        let mut timeout = if server.damage.is_empty() {
            server.next_clock.saturating_sub(now)
        } else {
            0
        };
        if deadline != 0 {
            timeout = timeout.min(deadline.saturating_sub(now));
        }
        // P13 K21: the display's sleep.
        let blank = server.blank_deadline();
        if blank != 0 {
            if now >= blank {
                server.blank();
            } else {
                timeout = timeout.min(blank - now);
            }
        }
        let n = match server.set.wait(&mut ready, timeout) {
            Ok(n) => n,
            Err(Error::TimedOut) => 0,
            Err(e) => {
                println!("windowserver: wait failed: {}", e.name());
                return 1;
            }
        };
        for r in &ready[..n] {
            if r.key == 0 {
                if server.drain(bootstrap::MAIN_PORT, None, 64, false).is_err() {
                    return 1;
                }
                continue;
            }
            if r.key == IME_PORT_KEY {
                let port = server.ime_port;
                let _ = server.drain(port, None, 16, true);
                continue;
            }
            let id = (r.key & 0xFFFF_FFFF) as u32;
            if r.key & REPLY_KEY != 0 {
                // The client's main port closed: it ended.
                server.drop_client(id);
            } else if let Some(port) = server.client(id).map(|c| c.port) {
                let _ = server.drain(port, Some(id), 64, false);
            }
        }
        server.flush();
    }
}
