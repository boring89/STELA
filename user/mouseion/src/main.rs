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
//!  run it; .txt → Kalamos.app (P11; other documents → Viewer.app);
//!  drag an icon: within its window → move it, onto a folder, volume or another window → move (same volume) or copy
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
    self, DropTarget, FolderState, Grid, ICON_ATTR, Kind, STATE_ATTR, View, bundle_name,
    decode_icon, display_name, ellipsize, encode_icon,
};
use toolbox::control::ScrollBar;
use toolbox::dialog;
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
    /// Last change (ns since 1970, UTC; 0 when unknown).
    mtime: u64,
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
    /// The items selected (P11: several, Shift-click or a band).
    selected: Vec<usize>,
    /// The content rectangle on the screen.
    rect: Rect,
    /// Lines of text for a Note.
    note: Vec<String>,
    /// The order of the items (P11 K43: the list view's column titles).
    sort: model::SortBy,
}

/// A press that may turn into a drag.
#[derive(Clone, Copy, Debug)]
struct Press {
    window: u32,
    index: usize,
    start: (i32, i32),
    /// Where the pointer is now (the outlines are drawn there).
    at: (i32, i32),
    moved: bool,
}

/// A name being typed (P11).
#[derive(Clone, Debug)]
struct Editing {
    window: u32,
    index: usize,
    text: String,
}

/// A selection band: its window, where it started, where it is now.
type Band = (u32, (i32, i32), (i32, i32));

/// The list of windows to open again (`Data:/.mouseion`).
const OPEN_LIST: &str = ".mouseion";

fn pane_window_id(p: &Pane) -> u32 {
    p.window.id
}

/// A folder's window state: the extended attribute, or the old place in
/// the Finder information (moved to the attribute, the Finder
/// information cleared for the Mac; P11).
fn read_state(h: Handle) -> Option<FolderState> {
    if let Ok(v) = fs::get_attr(h, STATE_ATTR)
        && let Ok(b) = <[u8; 32]>::try_from(v.as_slice())
    {
        return FolderState::decode(&b);
    }
    let old = fs::get_info(h).ok().and_then(|b| FolderState::decode(&b))?;
    if fs::set_attr(h, STATE_ATTR, &old.encode()).is_ok() {
        let _ = fs::set_info(h, &[0; 32]);
    }
    Some(old)
}

/// An item's icon position, the same way.
fn read_icon(h: Handle) -> Option<(i32, i32)> {
    if let Ok(v) = fs::get_attr(h, ICON_ATTR)
        && let Ok(b) = <[u8; 32]>::try_from(v.as_slice())
    {
        return decode_icon(&b);
    }
    let info = fs::get_info(h).ok()?;
    let pos = decode_icon(&info)?;
    if fs::set_attr(h, ICON_ATTR, &info).is_ok() {
        let _ = fs::set_info(h, &[0; 32]);
    }
    Some(pos)
}

/// Removes `name` from `dir`, with everything inside when it is a folder.
/// The id the desktop's trash icon has (no volume has it).
const TRASH_ID: u64 = u64::MAX;

/// `volume`'s trash folder (`.Trash` at its root), made when `make`.
fn trash_folder(volume: &str, make: bool) -> Option<Owned> {
    let root = Owned(fs::volume_open(volume).ok()?);
    let mut flags = open::READ | open::WRITE | open::DIRECTORY;
    if make {
        flags |= open::CREATE;
    }
    fs::open(root.0, ".Trash", flags).ok().map(Owned)
}

fn remove_tree(dir: Handle, name: &str, is_dir: bool) -> Result<(), Error> {
    if is_dir {
        let sub = Owned(fs::open(dir, name, open::READ | open::WRITE)?);
        for e in fs::entries(sub.0)? {
            remove_tree(sub.0, e.name(), e.kind_is_dir())?;
        }
    }
    fs::remove(dir, name)
}

struct Mouseion {
    /// Shift is held (Shift-click adds to the selection).
    shift: bool,
    /// A selection band being dragged: window, start, now.
    band: Option<Band>,
    /// A rename in progress.
    editing: Option<Editing>,
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
            // Row 0 holds the column titles (P11 K43).
            View::List => Rect::new(
                0,
                self.margin + (i as i32 + 1) * self.line() - pane.scroll,
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
            View::List => 2 * self.margin + (pane.items.len() as i32 + 1) * self.line(),
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
        let window_id = self.panes[index].window.id;
        let editing = self.editing.clone().filter(|e| e.window == window_id);
        let mut outlines = Vec::new();
        if let Some((w, a, b)) = self.band
            && w == window_id
        {
            outlines.push(model::band(a, b));
        }
        if let Some(p) = self.press
            && p.window == window_id
            && p.moved
        {
            let (dx, dy) = (p.at.0 - p.start.0, p.at.1 - p.start.1);
            for &i in &self.panes[index].selected {
                outlines.push(rects[i].offset(dx, dy));
            }
        }
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
            // The list view's column titles (click one to sort; P11 K43).
            if pane.view == View::List && !desktop && pane.place != Place::Note {
                let y = self.margin - pane.scroll + (line - font.height) / 2;
                for (title, by, col) in model::COLUMNS {
                    let x = 32 + col as i32 * font.width;
                    c.text(
                        x,
                        y,
                        title,
                        font,
                        if by == pane.sort {
                            color::BLACK
                        } else {
                            color::GREY
                        },
                        None,
                    );
                    if by == pane.sort {
                        c.hline(x, y + font.height - 1, font.text_width(title), color::BLACK);
                    }
                }
            }
            for (i, item) in pane.items.iter().enumerate() {
                let r = rects[i];
                if !r.overlaps(&bounds) {
                    continue;
                }
                let selected = pane.selected.contains(&i);
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
                        // Renaming: a text box with what was typed (P11).
                        if let Some(edit) = editing.as_ref().filter(|e| e.index == i) {
                            let shown = alloc::format!("{}|", edit.text);
                            let w = font.text_width(&shown).max(cell.0 - 8);
                            let bx = r.x + (cell.0 - w) / 2;
                            let b = Rect::new(bx - 2, ty - 1, w + 4, font.height + 2);
                            c.fill(b, color::WHITE);
                            c.frame(b, color::BLACK);
                            c.text(bx, ty, &shown, font, color::BLACK, None);
                            continue;
                        }
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
                            r.x + 32 + 38 * font.width,
                            ty,
                            item.kind.word(),
                            font,
                            fg,
                            None,
                        );
                        if item.mtime != 0 {
                            let when = libstela::time::format(libstela::time::to_local(item.mtime));
                            c.text(r.x + 32 + 50 * font.width, ty, &when[..16], font, fg, None);
                        }
                    }
                }
            }
            if !desktop && pane.place != Place::Note {
                bar.draw(&mut c);
            }
            // The selection band, and the outlines of icons being dragged.
            for r in &outlines {
                c.frame(*r, color::BLACK);
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
            let pos = if keeps {
                fs::open_by_id(h, e.id)
                    .ok()
                    .map(Owned)
                    .and_then(|f| read_icon(f.0))
            } else {
                None
            };
            stored.push(pos);
        }
        let grid = self.grid(&self.panes[index]);
        let places = grid.place(&stored);
        let previous: Vec<u64> = self.panes[index]
            .selected
            .iter()
            .filter_map(|&s| self.panes[index].items.get(s).map(|i| i.id))
            .collect();
        let mut items: Vec<Item> = entries
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
                mtime: e.mtime,
            })
            .collect();
        let by = self.panes[index].sort;
        items.sort_by(|a, b| model::compare(by, sort_key(a), sort_key(b)));
        let pane = &mut self.panes[index];
        pane.selected = previous
            .iter()
            .filter_map(|id| items.iter().position(|i| i.id == *id))
            .collect();
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
                mtime: 0,
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
        // The trash, at the bottom right (P11 K43).
        items.push(Item {
            name: String::from("Trash"),
            id: TRASH_ID,
            is_dir: true,
            size: 0,
            kind: Kind::Folder,
            pos: (
                width - self.cell.0 - self.margin,
                self.panes[index].rect.h - self.cell.1 - self.margin,
            ),
            volume: false,
            mtime: 0,
        });
        self.panes[index].items = items;
        self.draw(index);
    }

    /// Moves the dragged items (the selection, or item `i`) of pane
    /// `from` into the trash of their volume (`.Trash` at its root; P11
    /// K43). A name the trash has gets a number.
    fn move_to_trash(&mut self, from: usize, i: usize) {
        let Place::Folder { volume, .. } = self.panes[from].place.clone() else {
            return;
        };
        let Some(h) = self.panes[from].handle.as_ref().map(|h| h.0) else {
            return;
        };
        let names: Vec<String> = if self.panes[from].selected.contains(&i) {
            self.panes[from]
                .selected
                .iter()
                .filter_map(|&k| self.panes[from].items.get(k).map(|it| it.name.clone()))
                .collect()
        } else {
            self.panes[from]
                .items
                .get(i)
                .map(|it| it.name.clone())
                .into_iter()
                .collect()
        };
        let Some(trash) = trash_folder(&volume, true) else {
            println!("mouseion: {volume} has no trash (read only?)");
            return;
        };
        for name in names {
            let mut target = name.clone();
            let mut n = 2;
            while let Ok(e) = fs::open(trash.0, &target, open::READ) {
                let _ = handle_close(e);
                target = format!("{name} {n}");
                n += 1;
            }
            match fs::rename(h, &name, trash.0, &target) {
                Ok(()) => println!("mouseion: moved {name} to the trash"),
                Err(e) => println!("mouseion: trash {name}: {}", describe(e)),
            }
        }
        self.panes[from].selected.clear();
        self.reload(from);
        self.reload_trash_window();
    }

    /// Special > Empty Trash: asks, then deletes what every volume's trash
    /// holds.
    fn empty_trash(&mut self) {
        let answer = dialog::alert(
            &mut self.conn,
            "Empty Trash",
            "Delete everything in the trash? It is gone for good.",
            &["Cancel", "Empty Trash"],
        );
        if answer != Some(1) {
            return;
        }
        let mut count = 0;
        let mut i = 0;
        while let Ok(Some(v)) = fs::volume_list(i) {
            i += 1;
            let Some(trash) = trash_folder(v.name(), false) else {
                continue;
            };
            for e in fs::entries(trash.0).unwrap_or_default() {
                let dir = e.kind == libstela::abi::kind::DIRECTORY;
                if remove_tree(trash.0, e.name(), dir).is_ok() {
                    count += 1;
                }
            }
        }
        println!("mouseion: emptied the trash ({count} items)");
        self.reload_trash_window();
    }

    fn reload_trash_window(&mut self) {
        if let Some(p) = self.panes.iter().position(
            |p| matches!(&p.place, Place::Folder { path, .. } if path.ends_with("/.Trash")),
        ) {
            self.reload(p);
        }
    }

    /// Opens the trash's window (Data's).
    fn open_trash(&mut self) {
        let Some(trash) = trash_folder("Data", true) else {
            return;
        };
        if let Ok(st) = fs::stat(trash.0) {
            self.open_folder("Data", st.id, String::from("Data:/.Trash"), "Trash");
        }
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
        let state = read_state(handle.0);
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
            selected: Vec::new(),
            rect: Rect::new(rect.x, rect.y, w, h),
            note: Vec::new(),
            sort: model::SortBy::Name,
        });
        self.order.insert(0, id_w);
        let index = self.panes.len() - 1;
        self.reload(index);
        self.save_state(index);
        self.save_open_list();
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
        let _ = fs::set_attr(h.0, STATE_ATTR, &state.encode());
    }

    /// Writes `Data:/.mouseion`: the folder windows open now, opened
    /// again at the next start (P11).
    fn save_open_list(&self) {
        let list: Vec<model::OpenWindow> = self
            .panes
            .iter()
            .filter_map(|p| match &p.place {
                Place::Folder { volume, id, path } => Some((volume.clone(), *id, path.clone())),
                _ => None,
            })
            .collect();
        let Ok(data) = fs::volume_open("Data").map(Owned) else {
            return;
        };
        let flags = open::READ | open::WRITE | open::CREATE | open::TRUNCATE;
        if let Ok(f) = fs::open(data.0, OPEN_LIST, flags).map(Owned) {
            let _ = fs::write(f.0, model::format_open_list(&list).as_bytes(), 0);
        }
    }

    /// Opens the windows `Data:/.mouseion` lists.
    fn reopen(&mut self) {
        let Ok(data) = fs::volume_open("Data").map(Owned) else {
            return;
        };
        let Ok(f) = fs::open(data.0, OPEN_LIST, open::READ).map(Owned) else {
            return;
        };
        let mut buf = vec![0u8; 16 * 1024];
        let n = fs::read(f.0, &mut buf, 0).unwrap_or(0);
        let list = model::parse_open_list(&String::from_utf8_lossy(&buf[..n]));
        for (volume, id, path) in list {
            let title = path
                .trim_end_matches('/')
                .rsplit(['/', ':'])
                .next()
                .filter(|t| !t.is_empty())
                .map_or_else(|| volume.clone(), String::from);
            self.open_folder(&volume, id, path.clone(), &title);
        }
    }

    fn close_pane(&mut self, index: usize) {
        self.save_state(index);
        let pane = self.panes.remove(index);
        let window_id = pane_window_id(&pane);
        self.order.retain(|&w| w != window_id);
        let folder = matches!(pane.place, Place::Folder { .. });
        let _ = self.conn.close(pane.window);
        if folder {
            self.save_open_list();
        }
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
        if pane.place == Place::Desktop && item.id == TRASH_ID && !item.volume {
            self.open_trash();
            return;
        }
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
            // P11: text files open in Kalamos, the editor (docs/apps.md).
            Kind::Text => self.open_with("Kalamos.app", "Kalamos", &child_path, folder),
            Kind::Document => {
                let Ok(h) = fs::open_by_id(folder, item.id) else {
                    return;
                };
                let h = Owned(h);
                let mut magic = [0u8; 4];
                if fs::read(h.0, &mut magic, 0) == Ok(4) && &magic == b"\x7FELF" {
                    self.run(h.0, &[&item.name], folder, folder);
                } else {
                    self.open_with("Viewer.app", "Viewer", &child_path, folder);
                }
            }
        }
    }

    /// Opens `path` with the application `bundle` (in `Initrd:/`).
    fn open_with(&self, bundle: &str, name: &str, path: &str, cwd: Handle) {
        let Ok(initrd) = fs::volume_open("Initrd") else {
            return;
        };
        let initrd = Owned(initrd);
        if let Ok(app) = fs::open(initrd.0, bundle, open::READ) {
            let app = Owned(app);
            self.run(app.0, &[name, path], cwd, initrd.0);
        }
    }

    /// Special > Monitor (P11): the system monitor, with `POWER` so that
    /// it can end processes (`docs/apps.md` §3).
    fn monitor(&self) {
        let Ok(initrd) = fs::volume_open("Initrd") else {
            return;
        };
        let initrd = Owned(initrd);
        let Ok(app) = fs::open(initrd.0, "Monitor.app", open::READ) else {
            return;
        };
        let app = Owned(app);
        let handles = [
            pass(bootstrap::CONSOLE, Rights::SEND | Rights::DUPLICATE),
            pass(bootstrap::BOOTSTRAP, Rights::SEND | Rights::DUPLICATE),
            process::pass_dir(bootstrap::CWD),
            process::pass_dir(initrd.0),
        ];
        let mut caps = process::inherited();
        if let Some(power) = process::pass_capability(libstela::abi::capability::POWER) {
            caps.push((libstela::abi::capability::POWER, power));
        }
        match process::spawn_with(app.0, &["Monitor"], &handles, &caps) {
            Ok(p) => {
                println!("mouseion: started Monitor");
                let _ = handle_close(p);
            }
            Err(e) => println!("mouseion: cannot run Monitor: {}", describe(e)),
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
    /// Moves (or copies) the dragged items: the selection when the
    /// pressed item is in it (P11), else that item.
    fn transfer(
        &mut self,
        from: usize,
        i: usize,
        to: Handle,
        to_volume: &str,
        pos: Option<(i32, i32)>,
    ) {
        let sel = &self.panes[from].selected;
        let indices: Vec<usize> = if sel.contains(&i) {
            sel.clone()
        } else {
            alloc::vec![i]
        };
        let items: Vec<Item> = indices
            .iter()
            .filter_map(|&n| self.panes[from].items.get(n).cloned())
            .collect();
        let Place::Folder {
            id: from_id,
            volume: from_volume,
            ..
        } = self.panes[from].place.clone()
        else {
            return;
        };
        for (n, item) in items.into_iter().enumerate() {
            // Indices change as windows reload: find the source again.
            let Some(f) = self.panes.iter().position(|p| {
                matches!(&p.place, Place::Folder { id, volume, .. } if *id == from_id && *volume == from_volume)
            }) else {
                return;
            };
            let at = pos.map(|(x, y)| (x + n as i32 * 8, y + n as i32 * 8));
            self.transfer_one(f, item, to, to_volume, at);
        }
    }

    fn transfer_one(
        &mut self,
        from: usize,
        item: Item,
        to: Handle,
        to_volume: &str,
        pos: Option<(i32, i32)>,
    ) {
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
            let _ = fs::set_attr(h, ICON_ATTR, &encode_icon(x, y));
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
                if self.panes[to].place == Place::Desktop && item.id == TRASH_ID && !item.volume {
                    self.move_to_trash(from, press.index);
                    return;
                }
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
                    // Moved within its window: new icon positions for the
                    // dragged items (all of the selection, by the same
                    // distance).
                    let pane = &mut self.panes[from];
                    if pane.view != View::Icons || pane.place == Place::Desktop {
                        return;
                    }
                    let Some(pressed) = pane.items.get(press.index).map(|i| i.pos) else {
                        return;
                    };
                    let (dx, dy) = (pos.0 - pressed.0, pos.1 - pressed.1);
                    let moving: Vec<usize> = if pane.selected.contains(&press.index) {
                        pane.selected.clone()
                    } else {
                        alloc::vec![press.index]
                    };
                    for i in moving {
                        let Some(item) = pane.items.get_mut(i) else {
                            continue;
                        };
                        let new = grid.snap(item.pos.0 + dx, item.pos.1 + dy);
                        item.pos = new;
                        if let Some(h) = pane.handle.as_ref()
                            && let Ok(f) = fs::open_by_id(h.0, item.id)
                        {
                            let _ = fs::set_attr(f, ICON_ATTR, &encode_icon(new.0, new.1));
                            let _ = handle_close(f);
                        }
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

    /// Whether (x, y) is on item `i`'s name (icon view).
    fn on_label(&self, index: usize, i: usize, x: i32, y: i32) -> bool {
        let pane = &self.panes[index];
        if pane.view != View::Icons {
            return false;
        }
        let r = self.item_rect(pane, i);
        Rect::new(r.x, r.y + self.icon_size + 8, self.cell.0, self.font.height).contains(x, y)
    }

    fn redraw_editing(&mut self) {
        if let Some(p) = self.editing.as_ref().and_then(|e| self.pane(e.window)) {
            self.draw(p);
        }
    }

    /// Ends a rename: `keep` renames to what was typed.
    fn finish_rename(&mut self, keep: bool) {
        let Some(edit) = self.editing.take() else {
            return;
        };
        let Some(p) = self.pane(edit.window) else {
            return;
        };
        let old = self.panes[p].items.get(edit.index).map(|i| i.name.clone());
        if keep
            && let Some(old) = old
            && !edit.text.is_empty()
            && edit.text != old
            && let Some(h) = self.panes[p].handle.as_ref().map(|h| h.0)
        {
            match fs::rename(h, &old, h, &edit.text) {
                Ok(()) => println!("mouseion: renamed {old} to {}", edit.text),
                Err(e) => println!("mouseion: rename {old}: {}", describe(e)),
            }
        }
        self.reload(p);
    }

    /// Delete: asks about the selection of the front folder window.
    fn ask_delete(&mut self) {
        let Some(p) = self.order.first().and_then(|&w| self.pane(w)) else {
            return;
        };
        let Place::Folder { volume, id, .. } = self.panes[p].place.clone() else {
            return;
        };
        let names: Vec<(String, bool)> = self.panes[p]
            .selected
            .iter()
            .filter_map(|&i| self.panes[p].items.get(i))
            .map(|i| (i.name.clone(), i.is_dir))
            .collect();
        let Some((first, _)) = names.first() else {
            return;
        };
        let question = if names.len() == 1 {
            format!("Delete \"{first}\"?")
        } else {
            format!("Delete these {} items?", names.len())
        };
        println!("mouseion: asking: {question}");
        let text = format!("{question}\nThere is no Trash: it is gone for good.");
        if dialog::alert(&mut self.conn, "Delete", &text, &["Keep", "Delete"]) == Some(1) {
            self.delete(&volume, id, &names);
        }
    }

    /// The answer was yes: deletes `names` in folder `folder` of `volume`.
    fn delete(&mut self, volume: &str, folder: u64, names: &[(String, bool)]) {
        let Some(p) = self.panes.iter().position(
            |p| matches!(&p.place, Place::Folder { volume: v, id, .. } if v == volume && *id == folder),
        ) else {
            return;
        };
        if let Some(h) = self.panes[p].handle.as_ref().map(|h| h.0) {
            for (name, is_dir) in names {
                match remove_tree(h, name, *is_dir) {
                    Ok(()) => println!("mouseion: deleted {name}"),
                    Err(e) => println!("mouseion: delete {name}: {}", describe(e)),
                }
            }
        }
        self.panes[p].selected.clear();
        self.reload(p);
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
                // A column title of the list view: sort by it (P11 K43).
                if self.panes[index].view == View::List
                    && matches!(self.panes[index].place, Place::Folder { .. })
                {
                    let top = self.margin - self.panes[index].scroll;
                    if y >= top && y < top + self.line() {
                        let col = ((x - 32).max(0) / self.font.width) as usize;
                        self.panes[index].sort = model::column_at(col);
                        println!("mouseion: sorted by {:?}", self.panes[index].sort);
                        self.panes[index].selected.clear();
                        self.reload(index);
                        return;
                    }
                }
                let hit = self.hit(index, x, y);
                // A click ends a rename in progress (it is kept).
                if self.editing.is_some() {
                    self.finish_rename(true);
                }
                let folder = matches!(self.panes[index].place, Place::Folder { .. });
                let was_only = hit.is_some()
                    && self.panes[index].selected == hit.into_iter().collect::<Vec<_>>();
                match hit {
                    // Shift-click adds to (or takes from) the selection.
                    Some(i) if self.shift && folder => {
                        let sel = &mut self.panes[index].selected;
                        match sel.iter().position(|&s| s == i) {
                            Some(at) => {
                                sel.remove(at);
                            }
                            None => sel.push(i),
                        }
                        self.draw(index);
                        return;
                    }
                    Some(i) if !self.panes[index].selected.contains(&i) => {
                        self.panes[index].selected = alloc::vec![i];
                        println!("mouseion: selected {}", self.panes[index].items[i].name);
                        self.draw(index);
                    }
                    Some(_) => {}
                    None => {
                        self.panes[index].selected.clear();
                        // A band on the empty space of a folder window.
                        if folder {
                            self.band = Some((window, (x, y), (x, y)));
                        }
                        self.draw(index);
                    }
                }
                if let Some(i) = hit {
                    if clicks >= 2 {
                        self.press = None;
                        self.open_item(index, i);
                    } else if was_only && folder && self.on_label(index, i, x, y) {
                        // A click on the name of the one selected item:
                        // rename it (P11).
                        self.press = None;
                        self.editing = Some(Editing {
                            window,
                            index: i,
                            text: self.panes[index].items[i].name.clone(),
                        });
                        self.draw(index);
                    } else {
                        self.press = Some(Press {
                            window,
                            index: i,
                            start: (x, y),
                            at: (x, y),
                            moved: false,
                        });
                    }
                }
            }
            MouseKind::Move => {
                if let Some(p) = self.press.as_mut()
                    && p.window == window
                {
                    if (x - p.start.0).abs() > 4 || (y - p.start.1).abs() > 4 {
                        p.moved = true;
                    }
                    p.at = (x, y);
                    if p.moved {
                        // The outlines follow the pointer.
                        self.draw(index);
                    }
                }
                if let Some((w, a, _)) = self.band
                    && w == window
                {
                    self.band = Some((w, a, (x, y)));
                    self.draw(index);
                }
            }
            MouseKind::Up => {
                if let Some(p) = self.press.take()
                    && p.window == window
                    && p.moved
                {
                    self.draw(index);
                    self.drop_at(p, x, y);
                }
                if let Some((w, a, _)) = self.band.take()
                    && w == window
                {
                    let rects: Vec<Rect> = (0..self.panes[index].items.len())
                        .map(|i| self.item_rect(&self.panes[index], i))
                        .collect();
                    self.panes[index].selected = model::band_select(&rects, model::band(a, (x, y)));
                    self.draw(index);
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
            selected: Vec::new(),
            rect: Rect::new(0, 0, w, h),
            note: lines,
            sort: model::SortBy::Name,
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
            Some("About STELA") => {
                let lines = about_stela();
                self.note("About STELA", lines);
            }
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
                    && let Some(&i) = self.panes[p].selected.first()
                {
                    self.open_item(p, i);
                }
            }
            Some("Get Info") => {
                if let Some(p) = self.order.first().and_then(|&w| self.pane(w))
                    && let Some(&i) = self.panes[p].selected.first()
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
                            if item.mtime == 0 {
                                String::from("changed: -")
                            } else {
                                // Local time (P11 K40).
                                let offset = libstela::time::offset_minutes();
                                format!(
                                    "changed: {} {}",
                                    libstela::time::format(libstela::time::to_local(item.mtime)),
                                    libstela::time::offset_text(offset)
                                )
                            },
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
            Some("Monitor") => self.monitor(),
            Some("Empty Trash...") => self.empty_trash(),
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
                if let Some(Place::Folder { path, .. }) = self.pane(w).map(|p| &self.panes[p].place)
                {
                    println!("mouseion: front {path}");
                }
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
                use abi::key::KeyCode;
                if matches!(key.code, KeyCode::LeftShift | KeyCode::RightShift) {
                    self.shift = key.pressed;
                    return;
                }
                if !key.pressed {
                    return;
                }
                let c = abi::key::key_to_char(&key);
                // Typing a new name (P11).
                if self.editing.is_some() {
                    match c {
                        Some('\n') => self.finish_rename(true),
                        Some('\u{1b}') => self.finish_rename(false),
                        Some('\u{8}' | '\u{7f}') => {
                            if let Some(e) = self.editing.as_mut() {
                                e.text.pop();
                            }
                            self.redraw_editing();
                        }
                        Some(c) if !c.is_control() && c != '/' && c != ':' => {
                            if let Some(e) = self.editing.as_mut()
                                && e.text.len() < 255
                            {
                                e.text.push(c);
                            }
                            self.redraw_editing();
                        }
                        _ => {}
                    }
                    return;
                }
                if key.code == KeyCode::Delete {
                    self.ask_delete();
                    return;
                }
                // Enter opens the selection; Ctrl-W closes the window.
                match c {
                    Some('\n') => self.menu(1, 1),
                    Some('\u{17}') => self.menu(1, 4),
                    _ => {}
                }
            }
            Event::Other(msg) if msg.tag == libstela::tag::VOLUME_CHANGED => {
                self.volume_changed(msg.payload());
            }
            Event::Other(msg) => {
                for &h in msg.carried() {
                    let _ = handle_close(h);
                }
            }
            _ => {}
        }
    }

    /// A volume came or went (P11 K42, `VolumeWatch`): the desktop shows
    /// the volumes again; the windows of a volume that went are closed.
    fn volume_changed(&mut self, data: &[u8]) {
        let Some((&kind, name)) = data.split_first() else {
            return;
        };
        let name = String::from_utf8_lossy(name).into_owned();
        println!(
            "mouseion: volume {name} {}",
            if kind == 1 { "came" } else { "went" }
        );
        if kind == 2 {
            while let Some(p) = self.panes.iter().position(
                |p| matches!(&p.place, Place::Folder { volume, .. } if volume.eq_ignore_ascii_case(&name)),
            ) {
                self.close_pane(p);
            }
        }
        if let Some(d) = self.panes.iter().position(|p| p.place == Place::Desktop) {
            self.load_desktop(d);
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
    menus.add("Mouseion", &["About Mouseion", "About STELA"]);
    menus.add(
        "File",
        &["New Folder^N", "Open^O", "Get Info^I", "-", "Close^W"],
    );
    menus.add("Edit", &[]);
    menus.add("View", &["Icons", "List"]);
    menus.add(
        "Special",
        &[
            "Terminal",
            "Monitor",
            "Empty Trash...",
            "-",
            "Restart",
            "Shut Down",
        ],
    );
    let _ = conn.set_menubar(&menus.to_text());
    // USB disks come and go (P11 K42).
    if let Err(e) = fs::volume_watch(bootstrap::MAIN_PORT) {
        println!("mouseion: no volume changes: {}", describe(e));
    }
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
        shift: false,
        band: None,
        editing: None,
    };
    let id = desktop.id;
    m.panes.push(Pane {
        window: desktop,
        place: Place::Desktop,
        handle: None,
        items: Vec::new(),
        view: View::Icons,
        scroll: 0,
        selected: Vec::new(),
        rect,
        note: Vec::new(),
        sort: model::SortBy::Name,
    });
    m.order.push(id);
    m.load_desktop(0);
    println!(
        "mouseion: ready, {} volume(s)",
        m.panes[0].items.iter().filter(|i| i.volume).count()
    );
    m.terminal();
    // The folder windows that were open last time (P11).
    m.reopen();
    loop {
        if let Some(e) = m.conn.next_event(FOREVER) {
            m.event(e);
        }
    }
}

/// The About STELA window's lines: the summary at the top of the
/// hardware report (`docs/hwinfo.md`) and the uptime.
fn about_stela() -> Vec<String> {
    let mut lines = Vec::new();
    match process::hardware() {
        Ok(report) => {
            for line in report.lines().take_while(|l| !l.is_empty()) {
                if let Some((key, value)) = line.split_once(": ")
                    && matches!(key, "cpu" | "processors" | "memory" | "display")
                {
                    let mut value = String::from(value);
                    // The window is 40 characters wide.
                    let room = 40 - key.len() - 2;
                    if value.chars().count() > room {
                        value = value.chars().take(room - 3).collect::<String>() + "...";
                    }
                    lines.push(format!("{key}: {value}"));
                } else if line.starts_with("STELA ") {
                    lines.insert(0, String::from(line));
                }
            }
        }
        Err(e) => lines.push(format!("no hardware report: {}", e.name())),
    }
    let up = libstela::now_ns() / 1_000_000_000;
    lines.push(format!(
        "up {}:{:02}:{:02}",
        up / 3600,
        up / 60 % 60,
        up % 60
    ));
    lines
}

/// What sorting looks at in an item.
fn sort_key(i: &Item) -> (&str, bool, u64, &str, u64) {
    (&i.name, i.is_dir, i.size, i.kind.word(), i.mtime)
}
