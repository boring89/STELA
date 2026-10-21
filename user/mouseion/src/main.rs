// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mouseion: STELA's spatial file manager (`docs/mouseion.md`).
//!
//! ```text
//!  the desktop window (DESKTOP flag): one icon per volume, on the right
//!  one window per folder; a folder remembers its window (rectangle,
//!  scroll, view) and each file its icon position, in their Finder
//!  information (eHFS; FAT and the initrd do not keep it)
//!  double click: folder or volume → its window; Name.app or a program →
//!  run it; .txt → Viewer.app; drag an icon: within its window → move it,
//!  onto a folder, volume or another window → move (same volume) or copy
//! ```
//!
//! At start Mouseion also opens `Initrd:/Terminal.app` (the startup item
//! until there is a Startup Items folder).

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use abi::window::{AUTO, flags};
use abi::{DirEntry, kind};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use libstela::fs::{self, open};
use libstela::ipc::{FOREVER, handle_close};
use libstela::process::{self, pass};
use libstela::{Error, Handle, Rights, bootstrap, println};
use mouseion::model::{
    self, DropTarget, FolderState, Grid, Kind, View, bundle_name, decode_icon, display_name,
    ellipsize, encode_icon,
};
use toolbox::control::ScrollBar;
use toolbox::draw::{Font, Rect, color, font_for};
use toolbox::event::{Event, MouseKind};
use toolbox::icons::{self, Icon};
use toolbox::menu::MenuBar;
use toolbox::window::{Connection, Window};

libstela::entry!(main);

/// A handle closed when dropped.
struct Owned(Handle);

impl Drop for Owned {
    fn drop(&mut self) {
        let _ = handle_close(self.0);
    }
}

/// One icon in a window.
#[derive(Clone, Debug)]
struct Item {
    name: String,
    id: u64,
    is_dir: bool,
    size: u64,
    kind: Kind,
    /// Its cell's top-left corner in the window's content (unscrolled).
    pos: (i32, i32),
    /// A volume on the desktop.
    volume: bool,
}

/// What a window shows.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Place {
    /// The volumes.
    Desktop,
    /// A folder: its volume, persistent id and path (`Data:/docs`).
    Folder {
        volume: String,
        id: u64,
        path: String,
    },
    /// An information window (Get Info, About).
    Note,
}

struct Pane {
    window: Window,
    place: Place,
    /// The folder handle, kept open (`READ`, and `WRITE` when allowed).
    handle: Option<Owned>,
    items: Vec<Item>,
    view: View,
    scroll: i32,
    selected: Option<usize>,
    /// The content rectangle on the screen.
    rect: Rect,
    /// Lines of text for a Note.
    note: Vec<String>,
}

/// A press that may turn into a drag.
#[derive(Clone, Copy, Debug)]
struct Press {
    window: u32,
    index: usize,
    start: (i32, i32),
    moved: bool,
}

struct Mouseion {
    conn: Connection,
    font: &'static Font,
    icon_size: i32,
    cell: (i32, i32),
    margin: i32,
    panes: Vec<Pane>,
    /// Window ids, front first (as far as Mouseion knows).
    order: Vec<u32>,
    press: Option<Press>,
    menus: MenuBar,
}

const LIST_LINE_EXTRA: i32 = 8;
const SCROLL_WIDTH: i32 = 24;

fn describe(e: Error) -> &'static str {
    match e {
        Error::NotFound => "not found",
        e => e.name(),
    }
}

/// The entries of the folder `h`.
fn list(h: Handle) -> Vec<DirEntry> {
    fs::entries(h).unwrap_or_default()
}

impl Mouseion {
    fn pane(&self, window: u32) -> Option<usize> {
        self.panes.iter().position(|p| p.window.id == window)
    }

    fn icon_for(&self, item: &Item) -> Option<&'static Icon> {
        let name = if item.volume {
            "volume"
        } else {
            item.kind.icon()
        };
        icons::find(name, self.icon_size as u32)
    }

    fn grid(&self, pane: &Pane) -> Grid {
        Grid::for_width(pane.rect.w - SCROLL_WIDTH, self.cell, self.margin)
    }

    fn line(&self) -> i32 {
        self.font.height + LIST_LINE_EXTRA
    }

    /// The rectangle of item `i` in content coordinates (scrolled).
    fn item_rect(&self, pane: &Pane, i: usize) -> Rect {
        match pane.view {
            View::Icons => {
                let (x, y) = pane.items[i].pos;
                Rect::new(x, y - pane.scroll, self.cell.0, self.cell.1)
            }
            View::List => Rect::new(
                0,
                self.margin + i as i32 * self.line() - pane.scroll,
                pane.rect.w - SCROLL_WIDTH,
                self.line(),
            ),
        }
    }

    /// The height of everything in the pane.
    fn content_height(&self, pane: &Pane) -> i32 {
        match pane.view {
            View::Icons => {
                pane.items
                    .iter()
                    .map(|i| i.pos.1 + self.cell.1)
                    .max()
                    .unwrap_or(0)
                    + self.margin
            }
            View::List => 2 * self.margin + pane.items.len() as i32 * self.line(),
        }
    }

    fn scrollbar(&self, pane: &Pane) -> ScrollBar {
        let line = self.line();
        ScrollBar {
            rect: Rect::new(
                pane.rect.w - SCROLL_WIDTH,
                0,
                SCROLL_WIDTH,
                pane.rect.h - SCROLL_WIDTH,
            ),
            value: (pane.scroll / line) as usize,
            total: (self.content_height(pane) / line + 1) as usize,
            page: (pane.rect.h / line) as usize,
        }
    }

    // ---- drawing ----

    fn draw(&mut self, index: usize) {
        let font = self.font;
        let line = self.line();
        let cell = self.cell;
        let icon_size = self.icon_size;
        let rects: Vec<Rect> = (0..self.panes[index].items.len())
            .map(|i| self.item_rect(&self.panes[index], i))
            .collect();
        let icons: Vec<Option<&'static Icon>> = self.panes[index]
            .items
            .iter()
            .map(|i| self.icon_for(i))
            .collect();
        let small: Vec<Option<&'static Icon>> = self.panes[index]
            .items
            .iter()
            .map(|i| icons::find(if i.volume { "volume" } else { i.kind.icon() }, 16))
            .collect();
        let bar = self.scrollbar(&self.panes[index]);
        let pane = &mut self.panes[index];
        let bounds = pane.window.bounds();
        let desktop = pane.place == Place::Desktop;
        {
            let mut c = pane.window.canvas();
            c.fill(
                bounds,
                if desktop {
                    color::DESKTOP
                } else {
                    color::PARCHMENT
                },
            );
            if pane.place == Place::Note {
                for (i, text) in pane.note.iter().enumerate() {
                    c.text(16, 16 + i as i32 * line, text, font, color::BLACK, None);
                }
                if let Some(icon) = icons::find("temple", icon_size as u32) {
                    c.icon(bounds.w - icon_size - 16, 16, icon, false);
                }
            }
            for (i, item) in pane.items.iter().enumerate() {
                let r = rects[i];
                if !r.overlaps(&bounds) {
                    continue;
                }
                let selected = pane.selected == Some(i);
                let label = display_name(&item.name, item.is_dir);
                match pane.view {
                    View::Icons => {
                        if let Some(icon) = icons[i] {
                            c.icon(r.x + (cell.0 - icon_size) / 2, r.y + 4, icon, selected);
                        }
                        let text = ellipsize(label, (cell.0 / font.width) as usize);
                        let tw = font.text_width(&text);
                        let tx = r.x + (cell.0 - tw) / 2;
                        let ty = r.y + icon_size + 8;
                        let (fg, bg) = match (selected, desktop) {
                            (true, _) => (color::PARCHMENT, Some(color::HIGHLIGHT)),
                            (false, true) => (color::BLACK, Some(color::PARCHMENT)),
                            (false, false) => (color::BLACK, None),
                        };
                        if let Some(bg) = bg {
                            c.fill(Rect::new(tx - 2, ty, tw + 4, font.height), bg);
                        }
                        c.text(tx, ty, &text, font, fg, None);
                    }
                    View::List => {
                        let (fg, bg) = if selected {
                            (color::PARCHMENT, Some(color::HIGHLIGHT))
                        } else {
                            (color::BLACK, None)
                        };
                        if let Some(bg) = bg {
                            c.fill(r, bg);
                        }
                        if let Some(icon) = small[i] {
                            c.icon(r.x + 8, r.y + (line - 16) / 2, icon, selected);
                        }
                        let ty = r.y + (line - font.height) / 2;
                        c.text(r.x + 32, ty, &ellipsize(label, 24), font, fg, None);
                        let size = if item.is_dir {
                            String::from("-")
                        } else {
                            format!("{} B", item.size)
                        };
                        c.text(r.x + 32 + 26 * font.width, ty, &size, font, fg, None);
                        c.text(
                            r.x + 32 + 40 * font.width,
                            ty,
                            item.kind.word(),
                            font,
                            fg,
                            None,
                        );
                    }
                }
            }
            if !desktop && pane.place != Place::Note {
                bar.draw(&mut c);
            }
        }
        let _ = self.conn.damage_all(&self.panes[index].window);
    }

    // ---- folders ----

    /// Reads the folder of pane `index` again.
    fn reload(&mut self, index: usize) {
        let Some(h) = self.panes[index].handle.as_ref().map(|h| h.0) else {
            return;
        };
        let entries = list(h);
        let keeps = matches!(&self.panes[index].place, Place::Folder { .. });
        let mut stored = Vec::new();
        for e in &entries {
            let pos = if keeps && !e.kind_is_dir() {
                fs::open_by_id(h, e.id)
                    .ok()
                    .map(Owned)
                    .and_then(|f| fs::get_info(f.0).ok())
                    .and_then(|b| decode_icon(&b))
            } else {
                None
            };
            stored.push(pos);
        }
        let grid = self.grid(&self.panes[index]);
        let places = grid.place(&stored);
        let previous = self.panes[index]
            .selected
            .and_then(|s| self.panes[index].items.get(s).map(|i| i.id));
        let items: Vec<Item> = entries
            .iter()
            .zip(places)
            .map(|(e, pos)| Item {
                name: String::from(e.name()),
                id: e.id,
                is_dir: e.kind_is_dir(),
                size: e.size,
                kind: Kind::of(e.name(), e.kind_is_dir()),
                pos,
                volume: false,
            })
            .collect();
        let pane = &mut self.panes[index];
        pane.selected = previous.and_then(|id| items.iter().position(|i| i.id == id));
        pane.items = items;
        self.draw(index);
    }

    /// The volumes, on the desktop's right edge.
    fn load_desktop(&mut self, index: usize) {
        let mut items = Vec::new();
        let mut i = 0;
        while let Ok(Some(v)) = fs::volume_list(i) {
            items.push(Item {
                name: String::from(v.name()),
                id: u64::from(v.index),
                is_dir: true,
                size: v.total,
                kind: Kind::Folder,
                pos: (0, 0),
                volume: true,
            });
            i += 1;
        }
        let width = self.panes[index].rect.w;
        for (n, item) in items.iter_mut().enumerate() {
            item.pos = (
                width - self.cell.0 - self.margin,
                self.margin + n as i32 * self.cell.1,
            );
        }
        self.panes[index].items = items;
        self.draw(index);
    }

    /// Opens (or brings forward) the window of a folder.
    fn open_folder(&mut self, volume: &str, id: u64, path: String, title: &str) {
        if let Some(i) = self.panes.iter().position(
            |p| matches!(&p.place, Place::Folder { volume: v, id: d, .. } if v == volume && *d == id),
        ) {
            let _ = self.conn.focus(&self.panes[i].window);
            return;
        }
        let Ok(root) = fs::volume_open(volume) else {
            return;
        };
        let root = Owned(root);
        let handle = fs::open_by_id(root.0, id).map(Owned);
        let Ok(handle) = handle else {
            println!("mouseion: cannot open {path}");
            return;
        };
        let state = fs::get_info(handle.0)
            .ok()
            .and_then(|b| FolderState::decode(&b));
        let n = self.panes.len() as i32;
        let (rect, view, scroll) = match state {
            Some(s) => (s.rect, s.view, i32::from(s.scroll.1)),
            None => (
                Rect::new(
                    120 + 40 * n,
                    self.conn.menubar as i32 + 100 + 40 * n,
                    6 * self.cell.0 + 2 * self.margin + SCROLL_WIDTH,
                    3 * self.cell.1 + 2 * self.margin,
                ),
                View::Icons,
                0,
            ),
        };
        let w = rect.w.clamp(200, self.conn.screen.0 as i32 - 40);
        let h = rect.h.clamp(120, self.conn.screen.1 as i32 - 80);
        let window = match self.conn.create_window(
            w as u32,
            h as u32,
            flags::DOCUMENT,
            rect.x,
            rect.y,
            title,
        ) {
            Ok(win) => win,
            Err(e) => {
                println!("mouseion: no window for {path}: {}", describe(e));
                return;
            }
        };
        let id_w = window.id;
        self.panes.push(Pane {
            window,
            place: Place::Folder {
                volume: String::from(volume),
                id,
                path,
            },
            handle: Some(handle),
            items: Vec::new(),
            view,
            scroll,
            selected: None,
            rect: Rect::new(rect.x, rect.y, w, h),
            note: Vec::new(),
        });
        self.order.insert(0, id_w);
        let index = self.panes.len() - 1;
        self.reload(index);
        self.save_state(index);
        println!(
            "mouseion: opened {}",
            match &self.panes[index].place {
                Place::Folder { path, .. } => path.as_str(),
                _ => "",
            }
        );
    }

    /// Stores pane `index`'s window state in its folder's Finder
    /// information (ignored where the volume does not keep it).
    fn save_state(&self, index: usize) {
        let pane = &self.panes[index];
        if !matches!(pane.place, Place::Folder { .. }) {
            return;
        }
        let Some(h) = &pane.handle else {
            return;
        };
        let state = FolderState {
            rect: pane.rect,
            scroll: (0, pane.scroll.clamp(0, i32::from(i16::MAX)) as i16),
            view: pane.view,
        };
        let _ = fs::set_info(h.0, &state.encode());
    }

    fn close_pane(&mut self, index: usize) {
        self.save_state(index);
        let pane = self.panes.remove(index);
        self.order.retain(|&w| w != pane.window.id);
        let _ = self.conn.close(pane.window);
    }

    // ---- opening and running ----

    /// Runs `file` (a program file or a bundle folder) with `args`; its
    /// working folder is `cwd`, and `home` the folder it is in (its
    /// `[4]`).
    fn run(&self, file: Handle, args: &[&str], cwd: Handle, home: Handle) {
        let handles = [
            pass(bootstrap::CONSOLE, Rights::SEND | Rights::DUPLICATE),
            pass(bootstrap::BOOTSTRAP, Rights::SEND | Rights::DUPLICATE),
            process::pass_dir(cwd),
            process::pass_dir(home),
        ];
        match process::spawn_with(file, args, &handles, &process::inherited()) {
            Ok(p) => {
                println!("mouseion: started {}", args[0]);
                let _ = handle_close(p);
            }
            Err(e) => println!("mouseion: cannot run {}: {}", args[0], describe(e)),
        }
    }

    /// Opens item `i` of pane `index` (a double click or File > Open).
    fn open_item(&mut self, index: usize, i: usize) {
        let pane = &self.panes[index];
        let Some(item) = pane.items.get(i).cloned() else {
            return;
        };
        if item.volume {
            let Ok(root) = fs::volume_open(&item.name) else {
                return;
            };
            let root = Owned(root);
            let Ok(st) = fs::stat(root.0) else {
                return;
            };
            self.open_folder(&item.name, st.id, format!("{}:/", item.name), &item.name);
            return;
        }
        let Place::Folder { volume, path, .. } = pane.place.clone() else {
            return;
        };
        let Some(folder) = pane.handle.as_ref().map(|h| h.0) else {
            return;
        };
        let child_path = if path.ends_with('/') {
            format!("{path}{}", item.name)
        } else {
            format!("{path}/{}", item.name)
        };
        match item.kind {
            Kind::Folder => self.open_folder(&volume, item.id, child_path, &item.name),
            Kind::Bundle => {
                if let Ok(h) = fs::open_by_id(folder, item.id) {
                    let h = Owned(h);
                    let name = bundle_name(&item.name).unwrap_or(&item.name);
                    self.run(h.0, &[name], folder, folder);
                }
            }
            Kind::Text => self.open_with_viewer(&child_path, folder),
            Kind::Document => {
                let Ok(h) = fs::open_by_id(folder, item.id) else {
                    return;
                };
                let h = Owned(h);
                let mut magic = [0u8; 4];
                if fs::read(h.0, &mut magic, 0) == Ok(4) && &magic == b"\x7FELF" {
                    self.run(h.0, &[&item.name], folder, folder);
                } else {
                    self.open_with_viewer(&child_path, folder);
                }
            }
        }
    }

    fn open_with_viewer(&self, path: &str, cwd: Handle) {
        let Ok(initrd) = fs::volume_open("Initrd") else {
            return;
        };
        let initrd = Owned(initrd);
        if let Ok(viewer) = fs::open(initrd.0, "Viewer.app", open::READ) {
            let viewer = Owned(viewer);
            self.run(viewer.0, &["Viewer", path], cwd, initrd.0);
        }
    }

    /// Starts a terminal (the startup item, Special > Terminal).
    fn terminal(&self) {
        let Ok(initrd) = fs::volume_open("Initrd") else {
            return;
        };
        let initrd = Owned(initrd);
        if let Ok(t) = fs::open(initrd.0, "Terminal.app", open::READ) {
            let t = Owned(t);
            self.run(t.0, &["Terminal"], bootstrap::CWD, initrd.0);
        }
    }

    // ---- moving and copying ----

    /// Moves (same volume) or copies item `i` of pane `from` into the
    /// folder `to` (a handle), at icon position `pos` if given.
    fn transfer(
        &mut self,
        from: usize,
        i: usize,
        to: Handle,
        to_volume: &str,
        pos: Option<(i32, i32)>,
    ) {
        let Some(item) = self.panes[from].items.get(i).cloned() else {
            return;
        };
        let (Some(src), Place::Folder { volume, .. }) = (
            self.panes[from].handle.as_ref().map(|h| h.0),
            self.panes[from].place.clone(),
        ) else {
            return;
        };
        let result = if volume.eq_ignore_ascii_case(to_volume) {
            fs::rename(src, &item.name, to, &item.name)
        } else {
            copy(src, &item.name, item.is_dir, to)
        };
        if let Err(e) = result {
            println!("mouseion: {}: {}", item.name, describe(e));
            return;
        }
        if let Some((x, y)) = pos
            && !item.is_dir
            && let Ok(h) = fs::open(to, &item.name, open::READ | open::WRITE)
        {
            let _ = fs::set_info(h, &encode_icon(x, y));
            let _ = handle_close(h);
        }
        let all: Vec<usize> = (0..self.panes.len()).collect();
        for p in all {
            if matches!(self.panes[p].place, Place::Folder { .. }) {
                self.reload(p);
            }
        }
    }

    fn drop_at(&mut self, press: Press, x: i32, y: i32) {
        let Some(from) = self.pane(press.window) else {
            return;
        };
        let screen = (self.panes[from].rect.x + x, self.panes[from].rect.y + y);
        // Front first; the desktop last.
        let mut front: Vec<usize> = self.order.iter().filter_map(|&w| self.pane(w)).collect();
        front.sort_by_key(|&p| self.panes[p].place == Place::Desktop);
        let rects: Vec<Rect> = front.iter().map(|&p| self.panes[p].rect).collect();
        let target = model::drop_target(screen.0, screen.1, &rects, |w| {
            let p = &self.panes[front[w]];
            (0..p.items.len())
                .map(|i| {
                    (
                        self.item_rect(p, i),
                        p.items[i].is_dir && p.items[i].kind != Kind::Bundle,
                    )
                })
                .collect()
        });
        match target {
            DropTarget::OnIcon { window, index } => {
                let to = front[window];
                if to == from && index == press.index {
                    return;
                }
                let item = self.panes[to].items[index].clone();
                let (handle, volume) = if item.volume {
                    match fs::volume_open(&item.name) {
                        Ok(h) => (Owned(h), item.name.clone()),
                        Err(_) => return,
                    }
                } else {
                    let (Some(h), Place::Folder { volume, .. }) = (
                        self.panes[to].handle.as_ref().map(|h| h.0),
                        self.panes[to].place.clone(),
                    ) else {
                        return;
                    };
                    match fs::open_by_id(h, item.id) {
                        Ok(f) => (Owned(f), volume),
                        Err(_) => return,
                    }
                };
                self.transfer(from, press.index, handle.0, &volume, None);
            }
            DropTarget::InWindow { window, x, y } => {
                let to = front[window];
                let grid = self.grid(&self.panes[to]);
                let pos = grid.snap(
                    x - self.cell.0 / 2,
                    y + self.panes[to].scroll - self.cell.1 / 2,
                );
                if to == from {
                    // Moved within its window: a new icon position.
                    let pane = &mut self.panes[from];
                    if pane.view != View::Icons || pane.place == Place::Desktop {
                        return;
                    }
                    let Some(item) = pane.items.get_mut(press.index) else {
                        return;
                    };
                    item.pos = pos;
                    if model::keeps_icon_position(item.is_dir)
                        && let Some(h) = pane.handle.as_ref()
                        && let Ok(f) = fs::open_by_id(h.0, item.id)
                    {
                        let _ = fs::set_info(f, &encode_icon(pos.0, pos.1));
                        let _ = handle_close(f);
                    }
                    self.draw(from);
                    return;
                }
                let (Some(h), Place::Folder { volume, .. }) = (
                    self.panes[to].handle.as_ref().map(|h| h.0),
                    self.panes[to].place.clone(),
                ) else {
                    return;
                };
                self.transfer(from, press.index, h, &volume, Some(pos));
            }
            DropTarget::Nowhere => {}
        }
    }

    // ---- input ----

    fn hit(&self, index: usize, x: i32, y: i32) -> Option<usize> {
        let pane = &self.panes[index];
        (0..pane.items.len()).find(|&i| {
            let r = self.item_rect(pane, i);
            match pane.view {
                View::Icons => {
                    let icon = Rect::new(
                        r.x + (self.cell.0 - self.icon_size) / 2,
                        r.y + 4,
                        self.icon_size,
                        self.icon_size,
                    );
                    icon.contains(x, y)
                        || Rect::new(r.x, r.y + self.icon_size + 8, self.cell.0, self.font.height)
                            .contains(x, y)
                }
                View::List => r.contains(x, y),
            }
        })
    }

    fn mouse(&mut self, window: u32, x: i32, y: i32, kind: MouseKind, clicks: u32) {
        let Some(index) = self.pane(window) else {
            return;
        };
        match kind {
            MouseKind::Down => {
                if self.panes[index].place != Place::Desktop
                    && self.panes[index].place != Place::Note
                {
                    let mut bar = self.scrollbar(&self.panes[index]);
                    if bar.rect.contains(x, y) {
                        if bar.click(x, y) {
                            self.panes[index].scroll = bar.value as i32 * self.line();
                            self.draw(index);
                            self.save_state(index);
                        }
                        return;
                    }
                }
                let hit = self.hit(index, x, y);
                if self.panes[index].selected != hit {
                    self.panes[index].selected = hit;
                    self.draw(index);
                }
                if let Some(i) = hit {
                    if clicks >= 2 {
                        self.press = None;
                        self.open_item(index, i);
                    } else {
                        self.press = Some(Press {
                            window,
                            index: i,
                            start: (x, y),
                            moved: false,
                        });
                    }
                }
            }
            MouseKind::Move => {
                if let Some(p) = self.press.as_mut()
                    && p.window == window
                    && ((x - p.start.0).abs() > 4 || (y - p.start.1).abs() > 4)
                {
                    p.moved = true;
                }
            }
            MouseKind::Up => {
                if let Some(p) = self.press.take()
                    && p.window == window
                    && p.moved
                {
                    self.drop_at(p, x, y);
                }
            }
        }
    }

    fn front_folder(&self) -> Option<usize> {
        self.order
            .iter()
            .filter_map(|&w| self.pane(w))
            .find(|&p| matches!(self.panes[p].place, Place::Folder { .. }))
    }

    fn front_note(&self) -> Option<usize> {
        self.order
            .first()
            .and_then(|&w| self.pane(w))
            .filter(|&p| self.panes[p].place == Place::Note)
    }

    fn note(&mut self, title: &str, lines: Vec<String>) {
        let w = 40 * self.font.width + self.icon_size + 48;
        let h = (lines.len() as i32 + 2) * self.line() + 16;
        let Ok(window) = self.conn.create_window(
            w as u32,
            h.max(self.icon_size + 48) as u32,
            flags::TITLE_BAR | flags::CLOSE_BOX,
            AUTO,
            AUTO,
            title,
        ) else {
            return;
        };
        let id = window.id;
        self.panes.push(Pane {
            window,
            place: Place::Note,
            handle: None,
            items: Vec::new(),
            view: View::Icons,
            scroll: 0,
            selected: None,
            rect: Rect::new(0, 0, w, h),
            note: lines,
        });
        self.order.insert(0, id);
        let index = self.panes.len() - 1;
        self.draw(index);
    }

    fn menu(&mut self, menu: u32, item: u32) {
        let choice = self.menus.item(menu, item).map(String::from);
        match choice.as_deref() {
            Some("About Mouseion") => self.note(
                "About Mouseion",
                vec![
                    String::from("Mouseion"),
                    String::from("STELA's file manager"),
                    format!("version {}", env!("CARGO_PKG_VERSION")),
                    String::from("(icons: drafts)"),
                ],
            ),
            Some("New Folder") => {
                if let Some(p) = self.front_folder()
                    && let Some(h) = self.panes[p].handle.as_ref().map(|h| h.0)
                {
                    for n in 1..100 {
                        let name = if n == 1 {
                            String::from("untitled folder")
                        } else {
                            format!("untitled folder {n}")
                        };
                        match fs::open(h, &name, open::CREATE | open::DIRECTORY | open::EXCLUSIVE) {
                            Ok(f) => {
                                let _ = handle_close(f);
                                break;
                            }
                            Err(Error::Exists) => continue,
                            Err(e) => {
                                println!("mouseion: new folder: {}", describe(e));
                                break;
                            }
                        }
                    }
                    self.reload(p);
                }
            }
            Some("Open") => {
                if let Some(p) = self.order.first().and_then(|&w| self.pane(w))
                    && let Some(i) = self.panes[p].selected
                {
                    self.open_item(p, i);
                }
            }
            Some("Get Info") => {
                if let Some(p) = self.order.first().and_then(|&w| self.pane(w))
                    && let Some(i) = self.panes[p].selected
                {
                    let item = self.panes[p].items[i].clone();
                    let where_ = match &self.panes[p].place {
                        Place::Folder { path, .. } => path.clone(),
                        _ => String::from("the desktop"),
                    };
                    let kind = if item.volume {
                        "volume"
                    } else {
                        item.kind.word()
                    };
                    self.note(
                        &format!("{} Info", display_name(&item.name, item.is_dir)),
                        vec![
                            format!("name:  {}", item.name),
                            format!("kind:  {kind}"),
                            format!("size:  {} bytes", item.size),
                            format!("where: {where_}"),
                            format!("id:    {}", item.id),
                        ],
                    );
                }
            }
            Some("Close") => {
                if let Some(p) = self.front_note().or_else(|| self.front_folder()) {
                    self.close_pane(p);
                }
            }
            Some(view @ ("Icons" | "List")) => {
                if let Some(p) = self.front_folder() {
                    self.panes[p].view = if view == "List" {
                        View::List
                    } else {
                        View::Icons
                    };
                    self.panes[p].scroll = 0;
                    self.draw(p);
                    self.save_state(p);
                }
            }
            Some("Terminal") => self.terminal(),
            Some("Restart") => {
                let all: Vec<usize> = (0..self.panes.len()).collect();
                for p in all {
                    self.save_state(p);
                }
                println!("mouseion: restart: {}", describe(process::reboot()));
            }
            Some("Shut Down") => {
                let all: Vec<usize> = (0..self.panes.len()).collect();
                for p in all {
                    self.save_state(p);
                }
                println!("mouseion: shut down: {}", describe(process::shutdown()));
            }
            _ => {}
        }
    }

    fn event(&mut self, e: Event) {
        match e {
            Event::Mouse {
                window,
                x,
                y,
                kind,
                clicks,
                ..
            } => self.mouse(window, x, y, kind, clicks),
            Event::Focus(w) => {
                self.order.retain(|&o| o != w);
                self.order.insert(0, w);
            }
            Event::Moved(w, x, y) => {
                if let Some(p) = self.pane(w) {
                    self.panes[p].rect.x = x;
                    self.panes[p].rect.y = y;
                    self.save_state(p);
                }
            }
            Event::ResizeRequested(w, width, height) => {
                if let Some(p) = self.pane(w) {
                    let pane = &mut self.panes[p];
                    if self.conn.resize(&mut pane.window, width, height).is_ok() {
                        pane.rect.w = width as i32;
                        pane.rect.h = height as i32;
                        self.reload(p);
                        self.save_state(p);
                    }
                }
            }
            Event::CloseRequested(w) => {
                if let Some(p) = self.pane(w)
                    && self.panes[p].place != Place::Desktop
                {
                    self.close_pane(p);
                }
            }
            Event::Menu(menu, item) => self.menu(menu, item),
            Event::Key { key, .. } => {
                // Enter opens the selection; Ctrl-W closes the window.
                match abi::key::key_to_char(&key) {
                    Some('\n') => self.menu(1, 1),
                    Some('\u{17}') => self.menu(1, 4),
                    _ => {}
                }
            }
            Event::Other(msg) => {
                for &h in msg.carried() {
                    let _ = handle_close(h);
                }
            }
            _ => {}
        }
    }
}

/// Whether a directory entry is a folder.
trait IsDir {
    fn kind_is_dir(&self) -> bool;
}

impl IsDir for DirEntry {
    fn kind_is_dir(&self) -> bool {
        self.kind == kind::DIRECTORY
    }
}

/// Copies `name` from folder `src` into folder `dst` (folders with what
/// they hold).
fn copy(src: Handle, name: &str, is_dir: bool, dst: Handle) -> Result<(), Error> {
    if is_dir {
        let from = Owned(fs::open(src, name, open::READ)?);
        let to = Owned(fs::open(
            dst,
            name,
            open::READ | open::WRITE | open::CREATE | open::DIRECTORY | open::EXCLUSIVE,
        )?);
        for e in list(from.0) {
            copy(from.0, e.name(), e.kind_is_dir(), to.0)?;
        }
        return Ok(());
    }
    let from = Owned(fs::open(src, name, open::READ)?);
    let size = fs::stat(from.0)?.size as usize;
    let mut data = vec![0u8; size];
    let n = fs::read(from.0, &mut data, 0)?;
    let to = Owned(fs::open(
        dst,
        name,
        open::READ | open::WRITE | open::CREATE | open::EXCLUSIVE,
    )?);
    fs::write(to.0, &data[..n], 0)?;
    Ok(())
}

fn main() -> i64 {
    let mut conn = match Connection::open() {
        Ok(c) => c,
        Err(e) => {
            println!("mouseion: no window server: {}", describe(e));
            return 1;
        }
    };
    let font = font_for(conn.screen.0);
    let icon_size = icons::size_for(conn.screen.0) as i32;
    let mut menus = MenuBar::new();
    menus.add("Mouseion", &["About Mouseion"]);
    menus.add("File", &["New Folder", "Open", "Get Info", "-", "Close"]);
    menus.add("Edit", &[]);
    menus.add("View", &["Icons", "List"]);
    menus.add("Special", &["Terminal", "-", "Restart", "Shut Down"]);
    let _ = conn.set_menubar(&menus.to_text());
    let desktop = match conn.create_window(1, 1, flags::DESKTOP, 0, 0, "Desktop") {
        Ok(w) => w,
        Err(e) => {
            println!("mouseion: no desktop: {}", describe(e));
            return 1;
        }
    };
    let rect = Rect::new(
        0,
        conn.menubar as i32,
        desktop.width as i32,
        desktop.height as i32,
    );
    let cell = (
        (icon_size * 2).max(10 * font.width),
        icon_size + font.height + 16,
    );
    let mut m = Mouseion {
        conn,
        font,
        icon_size,
        cell,
        margin: 16,
        panes: Vec::new(),
        order: Vec::new(),
        press: None,
        menus,
    };
    let id = desktop.id;
    m.panes.push(Pane {
        window: desktop,
        place: Place::Desktop,
        handle: None,
        items: Vec::new(),
        view: View::Icons,
        scroll: 0,
        selected: None,
        rect,
        note: Vec::new(),
    });
    m.order.push(id);
    m.load_desktop(0);
    println!("mouseion: ready, {} volume(s)", m.panes[0].items.len());
    m.terminal();
    loop {
        if let Some(e) = m.conn.next_event(FOREVER) {
            m.event(e);
        }
    }
}
