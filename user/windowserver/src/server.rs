// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The window server proper: the screen, the clients, the event loop.

use abi::key::{KeyEvent, MouseEvent, button};
use abi::sysinfo::{FORMAT_RGBX, FramebufferInfo};
use abi::window::{self as proto, event, flags, mouse, request, window_event};
use alloc::collections::BTreeMap;
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
    Clicks, Damage, Menu, Metrics, Part, Stack, Window, find_shortcut, menu_box, menu_item_at,
    menu_titles, parse_menus, shortcut, visible_parts,
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
    scale: i32,
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

    /// Sends `msg` to client `id`; a client that went away is removed.
    fn send(&mut self, id: u32, msg: &Message) {
        let Some(reply) = self.client(id).map(|c| c.reply) else {
            return;
        };
        if port_send(reply, msg) == Err(Error::PeerClosed) {
            self.drop_client(id);
        }
    }

    fn drop_client(&mut self, id: u32) {
        let Some(at) = self.clients.iter().position(|c| c.id == id) else {
            return;
        };
        let client = self.clients.remove(at);
        let _ = self.set.remove(client.port);
        let _ = self.set.remove(client.reply);
        // Requests still queued go with the port (and what they carry).
        let _ = handle_close(client.port);
        let _ = handle_close(client.reply);
        for w in self.stack.remove_client(id) {
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
            canvas.fill(part, color::DESKTOP);
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
        for r in self.damage.take() {
            self.compose(r);
            self.present(r);
        }
    }

    // ---- input ----

    fn key(&mut self, key: KeyEvent) {
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
        if let Some(id) = self.focus
            && let Some(client) = self.window_owner(id)
        {
            self.send(client, &proto::key(id, &key));
        }
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
    fn main_message(&mut self, msg: &Message) {
        match msg.tag {
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
            request::CONNECT => self.connect(msg),
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
    fn connect(&mut self, msg: &Message) {
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
        });
        let mut m = proto::connected(
            id,
            self.width as u32,
            self.height as u32,
            self.scale as u32,
            self.m.menubar as u32,
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
            Rect::new(0, self.m.menubar, self.width, self.height - self.m.menubar)
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

    fn close_window(&mut self, client: u32, msg: &Message) -> Result<(), Error> {
        let id = proto::word(msg, 1);
        self.owned(client, id)?;
        if let Some(w) = self.stack.remove(id) {
            let frame = w.frame(&self.m);
            self.mark(frame);
        }
        self.surfaces.remove(&id);
        self.zoomed.remove(&id);
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
    fn drain(&mut self, port: Handle, client: Option<u32>, max: usize) -> Result<(), Error> {
        let mut msg = Message::new();
        for _ in 0..max {
            match port_receive(port, &mut msg, 0) {
                Ok(()) => {}
                Err(Error::TimedOut) => return Ok(()),
                Err(e) => return Err(e),
            }
            match client {
                None => self.main_message(&msg),
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
    let font = font_for(width);
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
    };
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
        let timeout = if server.damage.is_empty() {
            server.next_clock.saturating_sub(now)
        } else {
            0
        };
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
                if server.drain(bootstrap::MAIN_PORT, None, 64).is_err() {
                    return 1;
                }
                continue;
            }
            let id = (r.key & 0xFFFF_FFFF) as u32;
            if r.key & REPLY_KEY != 0 {
                // The client's main port closed: it ended.
                server.drop_client(id);
            } else if let Some(port) = server.client(id).map(|c| c.port) {
                let _ = server.drain(port, Some(id), 64);
            }
        }
        server.flush();
    }
}
