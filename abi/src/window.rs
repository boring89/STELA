// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The window server protocol (`docs/window.md`): messages between
//! applications and `user/windowserver`, over ports.
//!
//! ```text
//!  client                                   server (its main port is the
//!    │ CONNECT (version; carries the reply    client's bootstrap [2])
//!    │          port)                         │
//!    │──────────────────────────────────────►│ makes a port for the client
//!    │◄──────── CONNECTED (client id, screen size; carries that port)
//!    │ (every request now goes to that port)  │
//!    │ CREATE_WINDOW (size, flags, title)    │
//!    │──────────────────────────────────────►│ makes a Memory w*h*4
//!    │◄──────── WINDOW_CREATED (carries the Memory)
//!    │ (draws into it)                        │
//!    │ DAMAGE (window, rect)                  │
//!    │──────────────────────────────────────►│ composites, shows
//!    │◄──────── KEY / MOUSE / WINDOW_EVENT / MENU_SELECTED
//! ```
//!
//! Protocol version 2 (P11): each client has its own request port, and
//! the server knows the client by the port a request came on; the first
//! word of every request is reserved (0). Numbers are little endian; coordinates are i32 in screen
//! pixels, window positions name the top-left corner of the content (the
//! server draws the frame around it). Pixels are u32 `0x00RRGGBB`.

use crate::key::{KEY_EVENT_SIZE, KeyEvent};
use crate::{MESSAGE_DATA, Message};

/// The protocol version a client gives in `CONNECT`.
pub const VERSION: u32 = 2;

/// Client to server.
pub mod request {
    /// To the server's main port: the protocol `VERSION`; carries a SEND
    /// handle to the client's reply port.
    pub const CONNECT: u32 = 0x100;
    /// 0, width, height, flags, x, y (`AUTO` = server decides), title
    /// (UTF-8, the rest of the data).
    pub const CREATE_WINDOW: u32 = 0x101;
    /// 0, window, x, y, width, height: show that part again.
    pub const DAMAGE: u32 = 0x102;
    /// 0, window, x, y.
    pub const MOVE_WINDOW: u32 = 0x103;
    /// 0, window, width, height (a new Memory comes back).
    pub const RESIZE_WINDOW: u32 = 0x104;
    /// 0, window, title.
    pub const SET_TITLE: u32 = 0x105;
    /// 0, length; carries a Memory with the menus as text: one menu per
    /// line, `Title\tItem\tItem...`; an item `-` is a separator.
    pub const SET_MENUBAR: u32 = 0x106;
    /// 0, window.
    pub const CLOSE_WINDOW: u32 = 0x107;
    /// 0, window: bring it to the front and give it the keyboard.
    pub const FOCUS_WINDOW: u32 = 0x108;
}

/// Server to client.
pub mod event {
    /// client id, screen width, screen height, scale, menu bar height;
    /// carries a SEND handle to the client's own request port.
    pub const CONNECTED: u32 = 0x180;
    /// window, width, height; carries the window's Memory (width *
    /// height * 4 bytes, rows of `0x00RRGGBB`).
    pub const WINDOW_CREATED: u32 = 0x181;
    /// window, width, height; carries the new Memory.
    pub const WINDOW_RESIZED: u32 = 0x182;
    /// window, then a `KeyEvent` (4 bytes).
    pub const KEY: u32 = 0x183;
    /// window, x, y (in the content), buttons, kind (`mouse::*`), clicks
    /// (1, or 2 for a double click).
    pub const MOUSE: u32 = 0x184;
    /// window, kind (`window_event::*`), x, y, width, height.
    pub const WINDOW_EVENT: u32 = 0x185;
    /// menu index, item index (from 0; the STELA menu is not counted).
    pub const MENU_SELECTED: u32 = 0x186;
    /// the request's tag, an `abi::Error` value.
    pub const ERROR: u32 = 0x187;
    // 0x188 was PING (version 1): the server now sees a client's reply
    // port close at once.
}

/// `CREATE_WINDOW` flags.
pub mod flags {
    /// A title bar (drag to move).
    pub const TITLE_BAR: u32 = 1 << 0;
    /// A close box in the title bar.
    pub const CLOSE_BOX: u32 = 1 << 1;
    /// A grow box in the bottom-right corner.
    pub const RESIZABLE: u32 = 1 << 2;
    /// The desktop: no frame, always at the back, covering the screen
    /// below the menu bar (Mouseion's icons).
    pub const DESKTOP: u32 = 1 << 3;
    /// Every pointer move over the content comes as `MOUSE` (`MOVE`), not
    /// only those with a button held (P11; Classic.app).
    pub const WANT_MOTION: u32 = 1 << 4;
    /// The server's pointer is not drawn over the content (the window
    /// draws its own; P11).
    pub const HIDE_CURSOR_INSIDE: u32 = 1 << 5;
    /// A normal document window.
    pub const DOCUMENT: u32 = TITLE_BAR | CLOSE_BOX | RESIZABLE;
    /// Every defined flag.
    pub const ALL: u32 =
        TITLE_BAR | CLOSE_BOX | RESIZABLE | DESKTOP | WANT_MOTION | HIDE_CURSOR_INSIDE;
}

/// `MOUSE` kinds.
pub mod mouse {
    /// The pointer moved.
    pub const MOVE: u32 = 1;
    /// A button went down.
    pub const DOWN: u32 = 2;
    /// A button went up.
    pub const UP: u32 = 3;
}

/// `WINDOW_EVENT` kinds.
pub mod window_event {
    /// The window got the keyboard (it is in front).
    pub const FOCUS: u32 = 1;
    /// It lost the keyboard.
    pub const BLUR: u32 = 2;
    /// The user moved it (x, y are the new content position).
    pub const MOVED: u32 = 3;
    /// The user asked for a new size (width, height); answer with
    /// `RESIZE_WINDOW`.
    pub const RESIZE_REQUESTED: u32 = 4;
    /// The close box was clicked.
    pub const CLOSE_REQUESTED: u32 = 5;
}

/// `x`/`y` of `CREATE_WINDOW`: let the server place the window.
pub const AUTO: i32 = i32::MIN;

/// Longest title, in bytes.
pub const TITLE_MAX: usize = MESSAGE_DATA - 24;

/// Builds message data from 32-bit words and a trailing byte string.
fn build(tag: u32, words: &[u32], tail: &[u8]) -> Message {
    let mut data = [0u8; MESSAGE_DATA];
    let mut at = 0;
    for w in words {
        data[at..at + 4].copy_from_slice(&w.to_le_bytes());
        at += 4;
    }
    let n = tail.len().min(MESSAGE_DATA - at);
    data[at..at + n].copy_from_slice(&tail[..n]);
    Message::with_data(tag, &data[..at + n])
}

/// Word `i` of a message's data (0 if missing).
pub fn word(msg: &Message, i: usize) -> u32 {
    let p = msg.payload();
    p.get(i * 4..i * 4 + 4)
        .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Word `i` as a signed number.
pub fn signed(msg: &Message, i: usize) -> i32 {
    word(msg, i) as i32
}

/// The data after the first `words` words (a title, text).
pub fn tail(msg: &Message, words: usize) -> &[u8] {
    msg.payload().get(words * 4..).unwrap_or(&[])
}

/// A title cut to [`TITLE_MAX`] bytes at a character boundary.
fn title_bytes(title: &str) -> &[u8] {
    let mut n = title.len().min(TITLE_MAX);
    while !title.is_char_boundary(n) {
        n -= 1;
    }
    &title.as_bytes()[..n]
}

/// `CONNECT` (the caller adds the reply port handle).
pub fn connect() -> Message {
    build(request::CONNECT, &[VERSION], &[])
}

/// `CREATE_WINDOW`.
pub fn create_window(w: u32, h: u32, flags: u32, x: i32, y: i32, title: &str) -> Message {
    build(
        request::CREATE_WINDOW,
        &[0, w, h, flags, x as u32, y as u32],
        title_bytes(title),
    )
}

/// `DAMAGE`.
pub fn damage(window: u32, x: i32, y: i32, w: u32, h: u32) -> Message {
    build(request::DAMAGE, &[0, window, x as u32, y as u32, w, h], &[])
}

/// `MOVE_WINDOW`.
pub fn move_window(window: u32, x: i32, y: i32) -> Message {
    build(request::MOVE_WINDOW, &[0, window, x as u32, y as u32], &[])
}

/// `RESIZE_WINDOW`.
pub fn resize_window(window: u32, w: u32, h: u32) -> Message {
    build(request::RESIZE_WINDOW, &[0, window, w, h], &[])
}

/// `SET_TITLE`.
pub fn set_title(window: u32, title: &str) -> Message {
    build(request::SET_TITLE, &[0, window], title_bytes(title))
}

/// `SET_MENUBAR` (the caller adds the Memory handle).
pub fn set_menubar(len: u32) -> Message {
    build(request::SET_MENUBAR, &[0, len], &[])
}

/// `CLOSE_WINDOW`.
pub fn close_window(window: u32) -> Message {
    build(request::CLOSE_WINDOW, &[0, window], &[])
}

/// `FOCUS_WINDOW`.
pub fn focus_window(window: u32) -> Message {
    build(request::FOCUS_WINDOW, &[0, window], &[])
}

/// `CONNECTED`.
pub fn connected(client: u32, w: u32, h: u32, scale: u32, menubar: u32) -> Message {
    build(event::CONNECTED, &[client, w, h, scale, menubar], &[])
}

/// `WINDOW_CREATED` or `WINDOW_RESIZED` (the caller adds the Memory).
pub fn window_memory(tag: u32, window: u32, w: u32, h: u32) -> Message {
    build(tag, &[window, w, h], &[])
}

/// `KEY`.
pub fn key(window: u32, event: &KeyEvent) -> Message {
    build(event::KEY, &[window], &event.encode())
}

/// The key event of a `KEY` message.
pub fn key_of(msg: &Message) -> Option<KeyEvent> {
    let t = tail(msg, 1);
    KeyEvent::decode(t.get(..KEY_EVENT_SIZE)?)
}

/// `MOUSE`.
pub fn mouse_event(window: u32, x: i32, y: i32, buttons: u8, kind: u32, clicks: u32) -> Message {
    build(
        event::MOUSE,
        &[window, x as u32, y as u32, u32::from(buttons), kind, clicks],
        &[],
    )
}

/// `WINDOW_EVENT`.
pub fn window_event(window: u32, kind: u32, x: i32, y: i32, w: u32, h: u32) -> Message {
    build(
        event::WINDOW_EVENT,
        &[window, kind, x as u32, y as u32, w, h],
        &[],
    )
}

/// `MENU_SELECTED`.
pub fn menu_selected(menu: u32, item: u32) -> Message {
    build(event::MENU_SELECTED, &[menu, item], &[])
}

/// `ERROR`.
pub fn error(request: u32, error: u64) -> Message {
    build(event::ERROR, &[request, error as u32], &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::{KeyCode, Modifiers};

    #[test]
    fn values_are_fixed() {
        assert_eq!((request::CONNECT, request::FOCUS_WINDOW), (0x100, 0x108));
        assert_eq!((event::CONNECTED, event::ERROR), (0x180, 0x187));
        assert_eq!(VERSION, 2);
        assert_eq!(word(&connect(), 0), VERSION);
        assert_eq!(flags::DOCUMENT, 7);
        assert_eq!(flags::ALL, 63);
        assert_eq!((flags::WANT_MOTION, flags::HIDE_CURSOR_INSIDE), (16, 32));
        assert_eq!((mouse::MOVE, mouse::DOWN, mouse::UP), (1, 2, 3));
        assert_eq!(window_event::CLOSE_REQUESTED, 5);
    }

    #[test]
    fn requests_round_trip() {
        let m = create_window(640, 480, flags::DOCUMENT, AUTO, -20, "Terminal");
        assert_eq!(m.tag, request::CREATE_WINDOW);
        assert_eq!(
            (word(&m, 0), word(&m, 1), word(&m, 2), word(&m, 3)),
            (0, 640, 480, 7)
        );
        assert_eq!((signed(&m, 4), signed(&m, 5)), (AUTO, -20));
        assert_eq!(tail(&m, 6), b"Terminal");
        let m = damage(2, 10, 20, 30, 40);
        assert_eq!(
            (0..6).map(|i| word(&m, i)).collect::<heapless_vec::V>(),
            heapless_vec::V::from([0, 2, 10, 20, 30, 40])
        );
        assert_eq!(word(&m, 9), 0);
    }

    #[test]
    fn long_titles_are_cut_at_characters() {
        let long = "é".repeat(100);
        let m = set_title(2, &long);
        let t = tail(&m, 2);
        assert!(t.len() <= TITLE_MAX);
        assert!(core::str::from_utf8(t).is_ok());
    }

    #[test]
    fn key_events() {
        let e = KeyEvent {
            code: KeyCode::A,
            pressed: true,
            modifiers: Modifiers::from_raw(1),
        };
        let m = key(9, &e);
        assert_eq!(word(&m, 0), 9);
        assert_eq!(key_of(&m), Some(e));
        let m = mouse_event(4, -3, 7, 1, mouse::DOWN, 2);
        assert_eq!(
            (
                signed(&m, 1),
                signed(&m, 2),
                word(&m, 3),
                word(&m, 4),
                word(&m, 5)
            ),
            (-3, 7, 1, 2, 2)
        );
    }

    /// A tiny fixed vector for comparing words without alloc.
    mod heapless_vec {
        #[derive(Debug, PartialEq, Eq)]
        pub struct V([u32; 6], usize);
        impl From<[u32; 6]> for V {
            fn from(a: [u32; 6]) -> V {
                V(a, 6)
            }
        }
        impl FromIterator<u32> for V {
            fn from_iter<I: IntoIterator<Item = u32>>(it: I) -> V {
                let mut v = V([0; 6], 0);
                for x in it {
                    v.0[v.1] = x;
                    v.1 += 1;
                }
                v
            }
        }
    }
}
