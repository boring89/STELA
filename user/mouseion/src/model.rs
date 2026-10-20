// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mouseion's model, without system calls (`docs/mouseion.md`): what the
//! Finder information of folders and files holds, where icons go, what an
//! item is, and where a dragged icon lands. Pure logic, tested on the
//! host.
//!
//! ```text
//!  folder (32 bytes)                      file (32 bytes)
//!   0  "MI"  magic                         0  "MI"
//!   2  1     version                       2  1
//!   3  kind  1 = folder state              3  2 = icon position
//!   4  x y w h      i16 x 4 (screen)       4  x y   i16 x 2 (in the window)
//!  12  scroll x y   i16 x 2                8  reserved (0)
//!  16  view         0 icons, 1 list
//!  17  reserved (0)
//! ```

use alloc::string::String;
use toolbox::draw::Rect;

/// The first two bytes of Mouseion's Finder information.
pub const MAGIC: [u8; 2] = *b"MI";
/// The layout version.
pub const VERSION: u8 = 1;
const KIND_FOLDER: u8 = 1;
const KIND_ICON: u8 = 2;

/// How a folder window shows its items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// Icons in a grid.
    Icons,
    /// One line per item.
    List,
}

/// What a folder remembers about its window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FolderState {
    /// The window's content rectangle on the screen.
    pub rect: Rect,
    /// How far it is scrolled.
    pub scroll: (i16, i16),
    /// Icons or list.
    pub view: View,
}

fn i16_at(b: &[u8; 32], at: usize) -> i16 {
    i16::from_le_bytes([b[at], b[at + 1]])
}

fn put_i16(b: &mut [u8; 32], at: usize, v: i32) {
    let v = v.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

impl FolderState {
    /// The 32 bytes stored with the folder.
    pub fn encode(&self) -> [u8; 32] {
        let mut b = [0u8; 32];
        b[..2].copy_from_slice(&MAGIC);
        b[2] = VERSION;
        b[3] = KIND_FOLDER;
        put_i16(&mut b, 4, self.rect.x);
        put_i16(&mut b, 6, self.rect.y);
        put_i16(&mut b, 8, self.rect.w);
        put_i16(&mut b, 10, self.rect.h);
        put_i16(&mut b, 12, i32::from(self.scroll.0));
        put_i16(&mut b, 14, i32::from(self.scroll.1));
        b[16] = match self.view {
            View::Icons => 0,
            View::List => 1,
        };
        b
    }

    /// What `b` says, if it is Mouseion's folder state.
    pub fn decode(b: &[u8; 32]) -> Option<FolderState> {
        if b[..2] != MAGIC || b[2] != VERSION || b[3] != KIND_FOLDER {
            return None;
        }
        let rect = Rect::new(
            i32::from(i16_at(b, 4)),
            i32::from(i16_at(b, 6)),
            i32::from(i16_at(b, 8)),
            i32::from(i16_at(b, 10)),
        );
        if rect.w <= 0 || rect.h <= 0 {
            return None;
        }
        Some(FolderState {
            rect,
            scroll: (i16_at(b, 12), i16_at(b, 14)),
            view: if b[16] == 1 { View::List } else { View::Icons },
        })
    }
}

/// A file's (or folder's, as seen in its parent) icon position.
pub fn encode_icon(x: i32, y: i32) -> [u8; 32] {
    let mut b = [0u8; 32];
    b[..2].copy_from_slice(&MAGIC);
    b[2] = VERSION;
    b[3] = KIND_ICON;
    put_i16(&mut b, 4, x);
    put_i16(&mut b, 6, y);
    b
}

/// The icon position in `b`, if it holds one.
pub fn decode_icon(b: &[u8; 32]) -> Option<(i32, i32)> {
    (b[..2] == MAGIC && b[2] == VERSION && b[3] == KIND_ICON)
        .then(|| (i32::from(i16_at(b, 4)), i32::from(i16_at(b, 6))))
}

/// Where a folder keeps its icon position: a folder's Finder information
/// holds its window state, so its own icon position cannot go there too;
/// Mouseion keeps folder icons in the grid (`docs/mouseion.md` §3).
pub fn keeps_icon_position(is_dir: bool) -> bool {
    !is_dir
}

/// What an item is to Mouseion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A folder: opens a window.
    Folder,
    /// A `Name.app` folder: runs.
    Bundle,
    /// A text file (`.txt`): opens in the viewer.
    Text,
    /// Any other file (programs are recognised when opened).
    Document,
}

impl Kind {
    /// The kind of an item called `name`.
    pub fn of(name: &str, is_dir: bool) -> Kind {
        if is_dir {
            if bundle_name(name).is_some() {
                Kind::Bundle
            } else {
                Kind::Folder
            }
        } else if has_extension(name, "txt") {
            Kind::Text
        } else {
            Kind::Document
        }
    }

    /// The icon it shows.
    pub fn icon(self) -> &'static str {
        match self {
            Kind::Folder => "kibotos",
            Kind::Bundle => "app",
            Kind::Text | Kind::Document => "scroll",
        }
    }

    /// The word the list view shows.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Folder => "folder",
            Kind::Bundle => "application",
            Kind::Text => "text",
            Kind::Document => "document",
        }
    }
}

fn has_extension(name: &str, ext: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(stem, e)| !stem.is_empty() && e.eq_ignore_ascii_case(ext))
}

/// The bundle's name without `.app`, if `folder` is one.
pub fn bundle_name(folder: &str) -> Option<&str> {
    let cut = folder.len().checked_sub(4)?;
    (cut > 0 && folder.is_char_boundary(cut) && folder[cut..].eq_ignore_ascii_case(".app"))
        .then(|| &folder[..cut])
}

/// The name an icon shows: bundles without `.app`.
pub fn display_name(name: &str, is_dir: bool) -> &str {
    if is_dir {
        bundle_name(name).unwrap_or(name)
    } else {
        name
    }
}

/// `name` cut to `max` characters with a trailing `…` when longer.
pub fn ellipsize(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        return String::from(name);
    }
    let mut s: String = name.chars().take(max.saturating_sub(1)).collect();
    s.push('~');
    s
}

/// The icon grid of a window: cells of `cell` (width, height) from
/// (`margin`, `margin`), `cols` per row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    /// Cell width and height.
    pub cell: (i32, i32),
    /// Columns.
    pub cols: i32,
    /// Space around.
    pub margin: i32,
}

impl Grid {
    /// The grid that fits `width` pixels.
    pub fn for_width(width: i32, cell: (i32, i32), margin: i32) -> Grid {
        Grid {
            cell,
            cols: ((width - 2 * margin) / cell.0).max(1),
            margin,
        }
    }

    /// The top-left corner of cell `i`.
    pub fn position(&self, i: usize) -> (i32, i32) {
        let i = i as i32;
        (
            self.margin + (i % self.cols) * self.cell.0,
            self.margin + (i / self.cols) * self.cell.1,
        )
    }

    /// The cell a position falls in.
    pub fn cell_at(&self, x: i32, y: i32) -> usize {
        let col = ((x - self.margin).max(0) / self.cell.0).min(self.cols - 1);
        let row = (y - self.margin).max(0) / self.cell.1;
        (row * self.cols + col) as usize
    }

    /// Places `n` items: those with a stored position keep it, the others
    /// take the free cells in order. Returns the position of each.
    pub fn place(&self, stored: &[Option<(i32, i32)>]) -> alloc::vec::Vec<(i32, i32)> {
        let mut taken: alloc::vec::Vec<usize> = stored
            .iter()
            .flatten()
            .map(|&(x, y)| self.cell_at(x, y))
            .collect();
        let mut next = 0;
        stored
            .iter()
            .map(|s| match s {
                Some(p) => *p,
                None => {
                    while taken.contains(&next) {
                        next += 1;
                    }
                    taken.push(next);
                    self.position(next)
                }
            })
            .collect()
    }

    /// Snaps a dropped icon to the nearest cell.
    pub fn snap(&self, x: i32, y: i32) -> (i32, i32) {
        self.position(self.cell_at(x + self.cell.0 / 2, y + self.cell.1 / 2))
    }
}

/// Where a dragged icon was dropped (window coordinates are content
/// coordinates; windows are front first).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropTarget {
    /// On the icon `index` of window `window` (a folder or volume: into
    /// it).
    OnIcon {
        /// Which window (index in the list given).
        window: usize,
        /// Which icon.
        index: usize,
    },
    /// On the empty space of window `window` (into its folder) at (x, y).
    InWindow {
        /// Which window.
        window: usize,
        /// Content x.
        x: i32,
        /// Content y.
        y: i32,
    },
    /// Nowhere useful.
    Nowhere,
}

/// Decides where a drop at screen (x, y) lands: `windows` are the content
/// rectangles front first; `icons(w)` gives window `w`'s icon rectangles
/// (content coordinates) and whether each can take items (folders).
pub fn drop_target(
    x: i32,
    y: i32,
    windows: &[Rect],
    icons: impl Fn(usize) -> alloc::vec::Vec<(Rect, bool)>,
) -> DropTarget {
    for (w, r) in windows.iter().enumerate() {
        if !r.contains(x, y) {
            continue;
        }
        let (cx, cy) = (x - r.x, y - r.y);
        for (index, (icon, container)) in icons(w).into_iter().enumerate() {
            if icon.contains(cx, cy) {
                return if container {
                    DropTarget::OnIcon { window: w, index }
                } else {
                    DropTarget::InWindow {
                        window: w,
                        x: cx,
                        y: cy,
                    }
                };
            }
        }
        return DropTarget::InWindow {
            window: w,
            x: cx,
            y: cy,
        };
    }
    DropTarget::Nowhere
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn folder_state_round_trip() {
        let s = FolderState {
            rect: Rect::new(120, -5, 900, 400),
            scroll: (0, 37),
            view: View::List,
        };
        let b = s.encode();
        assert_eq!(&b[..4], b"MI\x01\x01");
        assert_eq!(FolderState::decode(&b), Some(s));
        assert_eq!(FolderState::decode(&[0; 32]), None);
        let mut bad = b;
        bad[2] = 9;
        assert_eq!(FolderState::decode(&bad), None);
        // Huge values are clamped, not wrapped.
        let big = FolderState {
            rect: Rect::new(100_000, 0, 10, 10),
            ..s
        };
        assert_eq!(FolderState::decode(&big.encode()).unwrap().rect.x, 32767);
        // An icon position is not a folder state.
        assert_eq!(FolderState::decode(&encode_icon(1, 2)), None);
    }

    #[test]
    fn icon_positions() {
        assert_eq!(decode_icon(&encode_icon(-40, 300)), Some((-40, 300)));
        assert_eq!(decode_icon(&[0; 32]), None);
        assert!(keeps_icon_position(false));
        assert!(!keeps_icon_position(true));
    }

    #[test]
    fn kinds_and_names() {
        assert_eq!(Kind::of("Terminal.app", true), Kind::Bundle);
        assert_eq!(Kind::of("docs", true), Kind::Folder);
        assert_eq!(Kind::of("a.TXT", false), Kind::Text);
        assert_eq!(Kind::of(".txt", false), Kind::Document);
        assert_eq!(Kind::of("hello", false), Kind::Document);
        assert_eq!(Kind::of("x.app", false), Kind::Document);
        assert_eq!(bundle_name("Viewer.APP"), Some("Viewer"));
        assert_eq!(bundle_name(".app"), None);
        assert_eq!(display_name("Echo.app", true), "Echo");
        assert_eq!(display_name("Echo.app", false), "Echo.app");
        assert_eq!(Kind::Bundle.icon(), "app");
        assert_eq!(ellipsize("abcdefghij", 6), "abcde~");
        assert_eq!(ellipsize("abc", 6), "abc");
    }

    #[test]
    fn grid_layout() {
        let g = Grid::for_width(500, (100, 120), 10);
        assert_eq!(g.cols, 4);
        assert_eq!(g.position(0), (10, 10));
        assert_eq!(g.position(5), (110, 130));
        assert_eq!(g.cell_at(115, 135), 5);
        assert_eq!(g.cell_at(-50, -50), 0);
        // Stored positions stay; the rest fill the free cells.
        let placed = g.place(&[None, Some((10, 10)), None]);
        assert_eq!(placed, vec![(110, 10), (10, 10), (210, 10)]);
        assert_eq!(g.snap(95, 5), (110, 10));
    }

    #[test]
    fn drops() {
        let windows = [Rect::new(100, 100, 300, 200), Rect::new(0, 0, 1000, 800)];
        let icons = |w: usize| {
            if w == 0 {
                vec![
                    (Rect::new(10, 10, 64, 64), true),
                    (Rect::new(100, 10, 64, 64), false),
                ]
            } else {
                vec![(Rect::new(900, 20, 64, 64), true)]
            }
        };
        assert_eq!(
            drop_target(120, 120, &windows, icons),
            DropTarget::OnIcon {
                window: 0,
                index: 0
            }
        );
        assert_eq!(
            drop_target(210, 120, &windows, icons),
            DropTarget::InWindow {
                window: 0,
                x: 110,
                y: 20
            }
        );
        assert_eq!(
            drop_target(920, 40, &windows, icons),
            DropTarget::OnIcon {
                window: 1,
                index: 0
            }
        );
        assert_eq!(
            drop_target(50, 700, &windows, icons),
            DropTarget::InWindow {
                window: 1,
                x: 50,
                y: 700
            }
        );
        assert_eq!(
            drop_target(5000, 5000, &windows, icons),
            DropTarget::Nowhere
        );
    }
}
