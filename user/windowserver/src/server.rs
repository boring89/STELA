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
use libstela::ipc::{Mapping, handle_close, input_attach, memory_create, port_receive, port_send};
use libstela::process::{self, pass};
use libstela::{Error, Handle, Message, Rights, bootstrap, println, tag};
use toolbox::draw::{Canvas, Font, Rect, color, font_for};
use windowserver::compose::{
    Clicks, Damage, Menu, Metrics, Part, Stack, Window, menu_box, menu_item_at, menu_titles,
    parse_menus,
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

/// How often clients are checked for having gone away.
const PING_NS: u64 = 1_000_000_000;
/// The label at the left of the menu bar.
const LABEL: &str = "STELA";

/// A connected application.
struct Client {
    id: u32,
    reply: Handle,
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
    next_client: u32,
    next_window: u32,
    focus: Option<u32>,
    cursor: (i32, i32),
    buttons: u8,
    drag: Option<Drag>,
    clicks: Clicks,
    damage: Damage,
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
        {
            let mut canvas = Canvas::new(&mut self.back, w, h, w as usize);
            canvas.set_clip(r);
            canvas.fill(r, color::DESKTOP);
        }
        let windows: Vec<Window> = self.stack.iter().cloned().collect();
        for win in &windows {
            let frame = win.frame(&m);
            if !frame.overlaps(&r) {
                continue;
            }
            let active = self.focus == Some(win.id);
            let pressed =
                matches!(self.drag, Some(Drag::Close { id, inside: true }) if id == win.id);
            if !win.is_desktop() {
                let mut canvas = Canvas::new(&mut self.back, w, h, w as usize);
                canvas.set_clip(r);
                draw_frame(&mut canvas, win, &m, font, active, pressed);
            }
            // The content, row by row from the client's buffer.
            let part = win.content.intersect(&r);
            if let Some(surface) = self.surfaces.get(&win.id)
                && !part.is_empty()
            {
                for y in part.y..part.bottom() {
                    let src =
                        ((y - win.content.y) * surface.width + (part.x - win.content.x)) as usize;
                    let dst = (y * self.width + part.x) as usize;
                    let row = &mut self.back[dst..dst + part.w as usize];
                    if surface.mapping.read_words(src, row).is_err() {
                        row.fill(color::BLACK);
                    }
                }
            }
            let grow = win.grow_box(&m);
            if !grow.is_empty() && grow.overlaps(&r) {
                let mut canvas = Canvas::new(&mut self.back, w, h, w as usize);
                canvas.set_clip(r);
                draw_grow_box(&mut canvas, grow);
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
        if cursor.overlaps(&r) {
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
        if let Some(id) = self.focus
            && let Some(client) = self.window_owner(id)
        {
            self.send(client, &proto::key(id, &key));
        }
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
            _ if moved => self.drag_to(x, y, e.buttons),
            _ => {}
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

    fn request(&mut self, msg: &Message) {
        if msg.tag == request::CONNECT {
            self.connect(msg);
            return;
        }
        let client = proto::word(msg, 0);
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
            _ => {
                release(msg);
                Ok(())
            }
        };
        if let Err(e) = result {
            self.reply_error(client, msg.tag, e);
        }
    }

    fn connect(&mut self, msg: &Message) {
        let Some(&reply) = msg.carried().first() else {
            return;
        };
        let id = self.next_client;
        self.next_client += 1;
        self.clients.push(Client {
            id,
            reply,
            menus: Vec::new(),
        });
        let m = proto::connected(
            id,
            self.width as u32,
            self.height as u32,
            self.scale as u32,
            self.m.menubar as u32,
        );
        self.send(id, &m);
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

    fn close_window(&mut self, client: u32, msg: &Message) -> Result<(), Error> {
        let id = proto::word(msg, 1);
        self.owned(client, id)?;
        if let Some(w) = self.stack.remove(id) {
            let frame = w.frame(&self.m);
            self.mark(frame);
        }
        self.surfaces.remove(&id);
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

    fn ping(&mut self) {
        let ids: Vec<u32> = self.clients.iter().map(|c| c.id).collect();
        for id in ids {
            self.send(id, &proto::ping());
        }
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
        c.text(area.x + 2 * font.width, y + 2, item, font, fg, None);
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
            pass(
                bootstrap::CWD,
                Rights::READ | Rights::WRITE | Rights::DUPLICATE,
            ),
        ];
        let base = name.rsplit('/').next().unwrap_or(name);
        match process::spawn(file.0, &[base], handles) {
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
        next_client: 1,
        next_window: 1,
        focus: None,
        cursor: (width as i32 / 2, height as i32 / 2),
        buttons: 0,
        drag: None,
        clicks: Clicks::default(),
        damage: Damage::new(),
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
    let mut msg = Message::new();
    let mut last_ping = libstela::now_ns();
    loop {
        let wait = if server.damage.is_empty() { PING_NS } else { 0 };
        let mut handled = 0;
        let mut timeout = wait;
        while handled < 64 {
            match port_receive(bootstrap::MAIN_PORT, &mut msg, timeout) {
                Ok(()) => {}
                Err(Error::TimedOut) => break,
                Err(_) => return 1,
            }
            timeout = 0;
            handled += 1;
            match msg.tag {
                tag::INPUT_KEY => {
                    if let Some(key) = KeyEvent::decode(msg.payload()) {
                        server.key(key);
                    }
                }
                tag::INPUT_MOUSE => {
                    if let Some(e) = MouseEvent::decode(msg.payload()) {
                        server.pointer(e);
                    }
                }
                _ => server.request(&msg),
            }
        }
        let now = libstela::now_ns();
        if now.saturating_sub(last_ping) >= PING_NS {
            last_ping = now;
            server.ping();
        }
        server.flush();
    }
}
