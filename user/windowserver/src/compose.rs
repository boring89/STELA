// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The window server's model, without any system call: the window stack,
//! the frame geometry, hit testing, damage (dirty rectangles), menus and
//! double clicks. Pure logic, tested on the host (`docs/window.md`).

use abi::window::flags;
use alloc::string::String;
use alloc::vec::Vec;
use toolbox::draw::Rect;

/// Sizes of the server's decorations, from the interface font.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    /// The menu bar's height.
    pub menubar: i32,
    /// A title bar's height.
    pub title: i32,
    /// The frame's outline.
    pub border: i32,
    /// The close box's side.
    pub close: i32,
    /// The grow box's side.
    pub grow: i32,
}

impl Metrics {
    /// The sizes for a font `font_height` pixels high.
    pub const fn for_font(font_height: i32) -> Metrics {
        Metrics {
            menubar: font_height + 8,
            title: font_height + 6,
            border: 1,
            close: font_height - 2,
            grow: font_height,
        }
    }
}

/// One window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    /// Its id (unique while the server runs).
    pub id: u32,
    /// The client that made it.
    pub client: u32,
    /// The content area in screen coordinates.
    pub content: Rect,
    /// `abi::window::flags`.
    pub flags: u32,
    /// The title.
    pub title: String,
}

impl Window {
    /// Whether this is the desktop window.
    pub fn is_desktop(&self) -> bool {
        self.flags & flags::DESKTOP != 0
    }

    /// The whole window: content, title bar and border.
    pub fn frame(&self, m: &Metrics) -> Rect {
        if self.is_desktop() {
            return self.content;
        }
        let title = if self.flags & flags::TITLE_BAR != 0 {
            m.title
        } else {
            0
        };
        Rect::new(
            self.content.x - m.border,
            self.content.y - title - m.border,
            self.content.w + 2 * m.border,
            self.content.h + title + 2 * m.border,
        )
    }

    /// The title bar (empty without one).
    pub fn title_bar(&self, m: &Metrics) -> Rect {
        if self.is_desktop() || self.flags & flags::TITLE_BAR == 0 {
            return Rect::default();
        }
        Rect::new(
            self.content.x,
            self.content.y - m.title,
            self.content.w,
            m.title,
        )
    }

    /// The close box (empty without one).
    pub fn close_box(&self, m: &Metrics) -> Rect {
        let bar = self.title_bar(m);
        if bar.is_empty() || self.flags & flags::CLOSE_BOX == 0 {
            return Rect::default();
        }
        Rect::new(bar.x + 8, bar.y + (bar.h - m.close) / 2, m.close, m.close)
    }

    /// The grow box, over the content's bottom-right corner (empty when
    /// the window cannot be resized).
    pub fn grow_box(&self, m: &Metrics) -> Rect {
        if self.is_desktop() || self.flags & flags::RESIZABLE == 0 {
            return Rect::default();
        }
        Rect::new(
            self.content.right() - m.grow,
            self.content.bottom() - m.grow,
            m.grow,
            m.grow,
        )
    }
}

/// What is under a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The menu bar.
    MenuBar,
    /// Nothing (the bare desktop).
    Nothing,
    /// A title bar (drag to move).
    Title(u32),
    /// A close box.
    Close(u32),
    /// A grow box.
    Grow(u32),
    /// A window's content (also the desktop window).
    Content(u32),
    /// The border.
    Border(u32),
}

/// The windows, back to front. A desktop window always stays at the
/// back.
#[derive(Debug, Default)]
pub struct Stack {
    windows: Vec<Window>,
}

impl Stack {
    /// No windows.
    pub const fn new() -> Stack {
        Stack {
            windows: Vec::new(),
        }
    }

    /// Back to front.
    pub fn iter(&self) -> impl Iterator<Item = &Window> {
        self.windows.iter()
    }

    /// Number of windows.
    pub fn len(&self) -> usize {
        self.windows.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// Adds `w` in front (a desktop window goes to the back).
    pub fn add(&mut self, w: Window) {
        if w.is_desktop() {
            self.windows.insert(0, w);
        } else {
            self.windows.push(w);
        }
    }

    /// The window `id`.
    pub fn get(&self, id: u32) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    /// The window `id`, to change it.
    pub fn get_mut(&mut self, id: u32) -> Option<&mut Window> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    /// Removes the window `id`.
    pub fn remove(&mut self, id: u32) -> Option<Window> {
        let at = self.windows.iter().position(|w| w.id == id)?;
        Some(self.windows.remove(at))
    }

    /// Removes every window of `client`; returns them.
    pub fn remove_client(&mut self, client: u32) -> Vec<Window> {
        let (gone, kept) = core::mem::take(&mut self.windows)
            .into_iter()
            .partition(|w| w.client == client);
        self.windows = kept;
        gone
    }

    /// Brings `id` to the front (not a desktop); whether it moved.
    pub fn raise(&mut self, id: u32) -> bool {
        let Some(at) = self.windows.iter().position(|w| w.id == id) else {
            return false;
        };
        if self.windows[at].is_desktop() || at + 1 == self.windows.len() {
            return false;
        }
        let w = self.windows.remove(at);
        self.windows.push(w);
        true
    }

    /// The front window (the desktop only when it is alone).
    pub fn front(&self) -> Option<&Window> {
        self.windows.last()
    }

    /// What is at (x, y), front first.
    pub fn hit(&self, x: i32, y: i32, m: &Metrics) -> Part {
        if y < m.menubar {
            return Part::MenuBar;
        }
        for w in self.windows.iter().rev() {
            if !w.frame(m).contains(x, y) {
                continue;
            }
            if w.close_box(m).contains(x, y) {
                return Part::Close(w.id);
            }
            if w.grow_box(m).contains(x, y) {
                return Part::Grow(w.id);
            }
            if w.title_bar(m).contains(x, y) {
                return Part::Title(w.id);
            }
            if w.content.contains(x, y) {
                return Part::Content(w.id);
            }
            return Part::Border(w.id);
        }
        Part::Nothing
    }

    /// Where a new window of size (w, h) goes when the client lets the
    /// server decide: cascaded from the top left, 30 pixels per window.
    pub fn place(&self, m: &Metrics, screen: Rect) -> (i32, i32) {
        let n = self.windows.iter().filter(|w| !w.is_desktop()).count() as i32 % 8;
        let x = screen.x + 40 + 30 * n;
        let y = screen.y + m.menubar + m.title + 30 + 30 * n;
        (x, y)
    }
}

/// Dirty rectangles waiting to be redrawn.
#[derive(Debug, Default)]
pub struct Damage {
    rects: Vec<Rect>,
}

/// Most separate rectangles kept; beyond that they are merged into one.
pub const DAMAGE_MAX: usize = 16;

impl Damage {
    /// Nothing dirty.
    pub const fn new() -> Damage {
        Damage { rects: Vec::new() }
    }

    /// Marks `r` (clipped to `screen`) dirty. Overlapping rectangles are
    /// merged when the union is not much bigger than the two.
    pub fn add(&mut self, r: Rect, screen: Rect) {
        let mut r = r.intersect(&screen);
        if r.is_empty() {
            return;
        }
        // Absorb every rectangle that overlaps or touches the new one.
        loop {
            let near = self.rects.iter().position(|o| {
                let grown = Rect::new(o.x - 1, o.y - 1, o.w + 2, o.h + 2);
                grown.overlaps(&r) && o.union(&r).area() <= (o.area() + r.area()) * 2
            });
            match near {
                Some(i) => r = r.union(&self.rects.swap_remove(i)),
                None => break,
            }
        }
        self.rects.push(r);
        if self.rects.len() > DAMAGE_MAX {
            let all = self.rects.iter().fold(Rect::default(), |a, b| a.union(b));
            self.rects.clear();
            self.rects.push(all);
        }
    }

    /// Whether anything is dirty.
    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    /// Takes the dirty rectangles.
    pub fn take(&mut self) -> Vec<Rect> {
        core::mem::take(&mut self.rects)
    }
}

/// One menu: a title and its items (`-` is a separator).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Menu {
    /// The title in the menu bar.
    pub title: String,
    /// The items.
    pub items: Vec<String>,
}

/// `r` without `cut`: up to four rectangles (above, below, left, right
/// of the cut), none if `cut` covers it.
pub fn subtract(r: Rect, cut: &Rect) -> Vec<Rect> {
    let c = r.intersect(cut);
    if c.is_empty() {
        return alloc::vec![r];
    }
    let mut out = Vec::new();
    let mut push = |x: Rect| {
        if !x.is_empty() {
            out.push(x);
        }
    };
    push(Rect::new(r.x, r.y, r.w, c.y - r.y));
    push(Rect::new(r.x, c.bottom(), r.w, r.bottom() - c.bottom()));
    push(Rect::new(r.x, c.y, c.x - r.x, c.h));
    push(Rect::new(c.right(), c.y, r.right() - c.right(), c.h));
    out
}

/// The parts of `r` inside `frames[index]` that no later (nearer) frame
/// covers: what of that window shows in `r` (occlusion culling, P11).
/// With `index` = `frames.len()`, the parts of `r` no frame covers.
pub fn visible_parts(r: Rect, frames: &[Rect], index: usize) -> Vec<Rect> {
    let start = match frames.get(index) {
        Some(f) => r.intersect(f),
        None => r,
    };
    if start.is_empty() {
        return Vec::new();
    }
    let mut parts = alloc::vec![start];
    let nearer = if index < frames.len() {
        &frames[index + 1..]
    } else {
        frames
    };
    for cut in nearer {
        parts = parts.into_iter().flat_map(|p| subtract(p, cut)).collect();
        if parts.is_empty() {
            break;
        }
    }
    parts
}

/// Parses the `SET_MENUBAR` text: one menu per line, `Title\tItem...`.
pub fn parse_menus(text: &str) -> Vec<Menu> {
    text.lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let mut parts = line.split('\t');
            Menu {
                title: String::from(parts.next().unwrap_or("")),
                items: parts.map(String::from).collect(),
            }
        })
        .collect()
}

/// Where menu titles sit in the menu bar: (x, width) of each, after a
/// leading label of `label_width` pixels.
pub fn menu_titles(menus: &[Menu], char_width: i32, label_width: i32) -> Vec<(i32, i32)> {
    let mut x = 8 + label_width + 2 * char_width;
    menus
        .iter()
        .map(|m| {
            let w = (m.title.chars().count() as i32 + 2) * char_width;
            let at = (x, w);
            x += w;
            at
        })
        .collect()
}

/// The pulled-down menu's rectangle, below its title at `title_x`.
pub fn menu_box(menu: &Menu, title_x: i32, m: &Metrics, char_width: i32, line: i32) -> Rect {
    let longest = menu
        .items
        .iter()
        .map(|i| i.chars().count())
        .max()
        .unwrap_or(0) as i32;
    Rect::new(
        title_x,
        m.menubar - 1,
        (longest + 4) * char_width,
        menu.items.len() as i32 * line + 4,
    )
}

/// The item under y in a pulled-down menu (separators are not items).
pub fn menu_item_at(menu: &Menu, area: &Rect, x: i32, y: i32, line: i32) -> Option<usize> {
    if !area.contains(x, y) {
        return None;
    }
    let i = ((y - area.y - 2) / line) as usize;
    menu.items.get(i).filter(|t| *t != "-").map(|_| i)
}

/// Double-click detection: a second press within `interval_ns` and a few
/// pixels of the first.
#[derive(Debug, Default)]
pub struct Clicks {
    last: Option<(u64, i32, i32)>,
    count: u32,
}

impl Clicks {
    /// The longest gap between the two presses of a double click.
    pub const INTERVAL_NS: u64 = 500_000_000;
    /// How far apart they may be.
    pub const SLOP: i32 = 4;

    /// A press at (x, y) at time `now`: 1 for a single click, 2 for the
    /// second of a double click (then it starts over).
    pub fn press(&mut self, now: u64, x: i32, y: i32) -> u32 {
        let double = matches!(self.last, Some((t, lx, ly))
            if now.saturating_sub(t) <= Self::INTERVAL_NS
                && (x - lx).abs() <= Self::SLOP
                && (y - ly).abs() <= Self::SLOP
                && self.count == 1);
        if double {
            self.count = 2;
            self.last = None;
        } else {
            self.count = 1;
            self.last = Some((now, x, y));
        }
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const M: Metrics = Metrics::for_font(16);

    fn win(id: u32, x: i32, y: i32, w: i32, h: i32) -> Window {
        Window {
            id,
            client: id * 10,
            content: Rect::new(x, y, w, h),
            flags: flags::DOCUMENT,
            title: String::from("t"),
        }
    }

    #[test]
    fn geometry() {
        let w = win(1, 100, 100, 200, 150);
        assert_eq!(w.frame(&M), Rect::new(99, 77, 202, 174));
        assert_eq!(w.title_bar(&M), Rect::new(100, 78, 200, 22));
        assert_eq!(w.close_box(&M), Rect::new(108, 82, 14, 14));
        assert_eq!(w.grow_box(&M), Rect::new(284, 234, 16, 16));
        let plain = Window {
            flags: 0,
            ..w.clone()
        };
        assert_eq!(plain.frame(&M), Rect::new(99, 99, 202, 152));
        assert!(plain.close_box(&M).is_empty());
    }

    #[test]
    fn stack_and_hits() {
        let mut s = Stack::new();
        s.add(win(1, 100, 100, 200, 150));
        s.add(win(2, 150, 150, 200, 150));
        s.add(Window {
            flags: flags::DESKTOP,
            ..win(3, 0, 24, 1000, 800)
        });
        assert_eq!(s.iter().map(|w| w.id).collect::<Vec<_>>(), vec![3, 1, 2]);
        assert_eq!(s.hit(160, 200, &M), Part::Content(2));
        assert_eq!(s.hit(120, 120, &M), Part::Content(1));
        assert_eq!(s.hit(110, 85, &M), Part::Close(1));
        assert_eq!(s.hit(200, 85, &M), Part::Title(1));
        assert_eq!(s.hit(345, 295, &M), Part::Grow(2));
        assert_eq!(s.hit(500, 600, &M), Part::Content(3));
        assert_eq!(s.hit(5, 5, &M), Part::MenuBar);
        assert_eq!(s.hit(99, 120, &M), Part::Border(1));
        assert!(s.raise(1));
        assert_eq!(s.hit(160, 200, &M), Part::Content(1));
        assert!(!s.raise(3), "the desktop stays at the back");
        assert_eq!(s.front().map(|w| w.id), Some(1));
        assert_eq!(s.remove_client(10).len(), 1);
        assert_eq!(s.len(), 2);
        assert!(s.remove(2).is_some());
        assert_eq!(s.hit(160, 200, &M), Part::Content(3));
    }

    #[test]
    fn damage_merges() {
        let screen = Rect::new(0, 0, 1000, 1000);
        let mut d = Damage::new();
        d.add(Rect::new(10, 10, 10, 10), screen);
        d.add(Rect::new(15, 15, 10, 10), screen);
        d.add(Rect::new(500, 500, 10, 10), screen);
        d.add(Rect::new(-50, -50, 10, 10), screen); // off screen
        let rects = d.take();
        assert_eq!(rects.len(), 2);
        assert!(rects.contains(&Rect::new(10, 10, 15, 15)));
        assert!(d.is_empty());
        for i in 0..40 {
            d.add(Rect::new(i * 20, i * 20, 5, 5), screen);
        }
        assert!(d.take().len() <= DAMAGE_MAX);
    }

    #[test]
    fn menus() {
        let menus = parse_menus("File\tNew Folder\tOpen\t-\tClose\nView\tIcons\tList\n");
        assert_eq!(menus.len(), 2);
        assert_eq!(menus[0].items, ["New Folder", "Open", "-", "Close"]);
        let titles = menu_titles(&menus, 8, 40);
        assert_eq!(titles, [(64, 48), (112, 48)]);
        let area = menu_box(&menus[0], 64, &M, 8, 20);
        assert_eq!(area, Rect::new(64, 23, 112, 84));
        assert_eq!(menu_item_at(&menus[0], &area, 70, 23 + 2 + 25, 20), Some(1));
        assert_eq!(menu_item_at(&menus[0], &area, 70, 23 + 2 + 45, 20), None); // separator
        assert_eq!(menu_item_at(&menus[0], &area, 10, 30, 20), None);
    }

    #[test]
    fn double_clicks() {
        let mut c = Clicks::default();
        assert_eq!(c.press(0, 10, 10), 1);
        assert_eq!(c.press(300_000_000, 11, 12), 2);
        assert_eq!(c.press(400_000_000, 11, 12), 1);
        assert_eq!(c.press(2_000_000_000, 11, 12), 1);
        assert_eq!(c.press(2_100_000_000, 40, 12), 1);
    }

    #[test]
    fn placement_cascades() {
        let mut s = Stack::new();
        let screen = Rect::new(0, 0, 1280, 800);
        let a = s.place(&M, screen);
        s.add(win(1, a.0, a.1, 10, 10));
        let b = s.place(&M, screen);
        assert_eq!((b.0 - a.0, b.1 - a.1), (30, 30));
    }

    #[test]
    fn rectangles_minus_rectangles() {
        let r = Rect::new(0, 0, 10, 10);
        assert_eq!(subtract(r, &Rect::new(20, 20, 5, 5)), alloc::vec![r]);
        assert!(subtract(r, &Rect::new(-1, -1, 20, 20)).is_empty());
        let parts = subtract(r, &Rect::new(2, 3, 4, 4));
        assert_eq!(parts.len(), 4);
        let area: i64 = parts.iter().map(Rect::area).sum();
        assert_eq!(area, 100 - 16);
        for (i, a) in parts.iter().enumerate() {
            for b in &parts[i + 1..] {
                assert!(!a.overlaps(b));
            }
        }
        // A cut along one edge leaves one piece.
        assert_eq!(
            subtract(r, &Rect::new(0, 0, 10, 4)),
            alloc::vec![Rect::new(0, 4, 10, 6)]
        );
    }

    #[test]
    fn what_shows_of_each_window() {
        // Back to front: a big window, a small one on top of it.
        let frames = [Rect::new(0, 0, 100, 100), Rect::new(10, 10, 20, 20)];
        let dirty = Rect::new(0, 0, 50, 50);
        let back = visible_parts(dirty, &frames, 0);
        let area: i64 = back.iter().map(Rect::area).sum();
        assert_eq!(area, 50 * 50 - 20 * 20);
        assert!(back.iter().all(|p| !p.overlaps(&frames[1])));
        assert_eq!(visible_parts(dirty, &frames, 1), alloc::vec![frames[1]]);
        // Fully covered: nothing to draw.
        assert!(visible_parts(Rect::new(12, 12, 5, 5), &frames, 0).is_empty());
        // The desktop shows only outside every frame.
        let outside = visible_parts(Rect::new(90, 90, 20, 20), &frames, frames.len());
        let area: i64 = outside.iter().map(Rect::area).sum();
        assert_eq!(area, 400 - 100);
    }
}
