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
//! word of every request is reserved (0). Version 3 (P13 K13): every
//! coordinate and size is in **logical** pixels; `CONNECTED` gives the
//! scale (physical pixels per logical one, 1 or 2) and the logical screen;
//! a window's Memory is its logical size times the scale in each
//! direction. Numbers are little endian; coordinates are i32 in screen
//! pixels, window positions name the top-left corner of the content (the
//! server draws the frame around it). Pixels are u32 `0x00RRGGBB`.

use crate::key::{KEY_EVENT_SIZE, KeyEvent};
use crate::{MESSAGE_DATA, Message};

/// The protocol version a client gives in `CONNECT`.
pub const VERSION: u32 = 3;

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
    /// line, `Title\tItem\tItem...`; an item `-` is a separator; an item
    /// ending in `^K` (a letter or digit) has the keyboard shortcut
    /// Alt-K (Alt is Command; P11 K43).
    pub const SET_MENUBAR: u32 = 0x106;
    /// 0, window.
    pub const CLOSE_WINDOW: u32 = 0x107;
    /// 0, window: bring it to the front and give it the keyboard.
    pub const FOCUS_WINDOW: u32 = 0x108;
    /// 0, length: the system clipboard becomes this text (UTF-8, at most
    /// `CLIPBOARD_MAX` bytes; P11); carries a Memory with it (none when
    /// the length is 0: the clipboard is emptied).
    pub const SET_CLIPBOARD: u32 = 0x109;
    /// 0: the server answers with `CLIPBOARD`.
    pub const GET_CLIPBOARD: u32 = 0x10A;
    /// 0: the client is done (toolbox: when its `Connection` is dropped);
    /// its windows close and the server forgets it, as when its program
    /// ends (P11 K46: a program that connected again and again left a
    /// client behind each time).
    pub const DISCONNECT: u32 = 0x10B;
    /// P13 K2 (`docs/ime.md`): the sender becomes the input method. Only a
    /// client that connected through the server's IME port (the program
    /// the server starts as its IME) may; one at a time.
    pub const REGISTER_IME: u32 = 0x10C;
    /// P13 K2: the IME's answer to an `IME_KEY` (or to `IME_RESET`, seq
    /// 0): [`ImeHeader`], then the text (committed, composed, candidates)
    /// in the message or, with [`ime::MEMORY`], in a carried Memory.
    pub const IME_RESULT: u32 = 0x10D;
    /// P13 K2: where the text cursor of a window is (window, x, y, h in
    /// content coordinates): the composition is drawn there.
    pub const SET_CARET: u32 = 0x10E;
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
    /// length: the clipboard's text (P11); carries a Memory with it when
    /// the length is not 0.
    pub const CLIPBOARD: u32 = 0x188;
    /// P13 K2, to the IME: a key for the focused window (seq, `KeyEvent`).
    pub const IME_KEY: u32 = 0x189;
    /// P13 K2, to the focused window: text the IME committed (window,
    /// length, UTF-8; longer text comes in several).
    pub const TEXT_INPUT: u32 = 0x18A;
    /// P13 K2, to the IME: the focus moved; drop what is being composed
    /// and answer with an `IME_RESULT` of seq 0.
    pub const IME_RESET: u32 = 0x18B;
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
    /// The zoom box was clicked (P11 K43): the server moved the window and
    /// asks for this size (width, height); answer with `RESIZE_WINDOW`.
    pub const ZOOM_REQUESTED: u32 = 6;
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

/// Most bytes the clipboard holds.
pub const CLIPBOARD_MAX: usize = 1 << 20;

/// `SET_CLIPBOARD` (the caller adds the Memory handle when `len` > 0).
pub fn set_clipboard(len: u32) -> Message {
    build(request::SET_CLIPBOARD, &[0, len], &[])
}

/// `GET_CLIPBOARD`.
pub fn get_clipboard() -> Message {
    build(request::GET_CLIPBOARD, &[0], &[])
}

/// `CLIPBOARD` (the server adds the Memory handle when `len` > 0).
pub fn clipboard(len: u32) -> Message {
    build(event::CLIPBOARD, &[len], &[])
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

/// The flags of an `IME_RESULT` (P13 K2).
pub mod ime {
    /// The IME took the key: it does not go to the window.
    pub const CONSUMED: u32 = 1 << 0;
    /// Chinese mode (the menu bar shows 注; English: 英).
    pub const CHINESE: u32 = 1 << 1;
    /// The text is in the carried Memory, not in the message.
    pub const MEMORY: u32 = 1 << 2;
}

/// The words of an `IME_RESULT` before its text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImeHeader {
    /// The `IME_KEY` answered (0: after `IME_RESET`, or on its own).
    pub seq: u32,
    /// [`ime`] flags.
    pub flags: u32,
    /// The cursor in the composed text, in characters.
    pub cursor: u32,
    /// Bytes of committed text (first in the text).
    pub commit_len: u32,
    /// Bytes of composed text (next).
    pub preedit_len: u32,
    /// Bytes of candidates, separated by `\n` (last).
    pub candidates_len: u32,
    /// The candidate page shown.
    pub page: u16,
    /// The number of candidate pages.
    pub pages: u16,
}

/// Bytes of text an `IME_RESULT` holds itself.
pub const IME_INLINE_MAX: usize = MESSAGE_DATA - 32;

impl ImeHeader {
    /// Bytes of text.
    pub fn text_len(&self) -> usize {
        self.commit_len as usize + self.preedit_len as usize + self.candidates_len as usize
    }
}

/// `REGISTER_IME`.
pub fn register_ime() -> Message {
    build(request::REGISTER_IME, &[0], &[])
}

/// `IME_RESULT` with the text in the message (`text` at most
/// [`IME_INLINE_MAX`] bytes; without [`ime::MEMORY`]), or only the header
/// (the caller adds the Memory).
pub fn ime_result(h: &ImeHeader, text: &[u8]) -> Message {
    build(
        request::IME_RESULT,
        &[
            0,
            h.seq,
            h.flags,
            h.cursor,
            h.commit_len,
            h.preedit_len,
            h.candidates_len,
            u32::from(h.page) | (u32::from(h.pages) << 16),
        ],
        text,
    )
}

/// The header of an `IME_RESULT`.
pub fn ime_header_of(msg: &Message) -> ImeHeader {
    let pages = word(msg, 7);
    ImeHeader {
        seq: word(msg, 1),
        flags: word(msg, 2),
        cursor: word(msg, 3),
        commit_len: word(msg, 4),
        preedit_len: word(msg, 5),
        candidates_len: word(msg, 6),
        page: pages as u16,
        pages: (pages >> 16) as u16,
    }
}

/// The text of an `IME_RESULT` that holds it itself.
pub fn ime_text_of(msg: &Message) -> &[u8] {
    tail(msg, 8)
}

/// `SET_CARET`.
pub fn set_caret(window: u32, x: i32, y: i32, h: u32) -> Message {
    build(request::SET_CARET, &[0, window, x as u32, y as u32, h], &[])
}

/// `IME_KEY`.
pub fn ime_key(seq: u32, event: &KeyEvent) -> Message {
    build(event::IME_KEY, &[seq], &event.encode())
}

/// The sequence number and key of an `IME_KEY`.
pub fn ime_key_of(msg: &Message) -> Option<(u32, KeyEvent)> {
    let t = tail(msg, 1);
    Some((word(msg, 0), KeyEvent::decode(t.get(..KEY_EVENT_SIZE)?)?))
}

/// `IME_RESET`.
pub fn ime_reset() -> Message {
    build(event::IME_RESET, &[0], &[])
}

/// Bytes of text one `TEXT_INPUT` holds.
pub const TEXT_INPUT_MAX: usize = MESSAGE_DATA - 8;

/// `TEXT_INPUT` messages for `text`, cut at character boundaries.
pub fn text_input(window: u32, text: &str) -> impl Iterator<Item = Message> + '_ {
    let mut rest = text;
    core::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let mut n = rest.len().min(TEXT_INPUT_MAX);
        while !rest.is_char_boundary(n) {
            n -= 1;
        }
        let (chunk, more) = rest.split_at(n);
        rest = more;
        Some(build(
            event::TEXT_INPUT,
            &[window, chunk.len() as u32],
            chunk.as_bytes(),
        ))
    })
}

/// The text of a `TEXT_INPUT` (invalid UTF-8 is cut off).
pub fn text_of(msg: &Message) -> &str {
    let t = tail(msg, 2);
    let t = &t[..t.len().min(word(msg, 1) as usize)];
    match core::str::from_utf8(t) {
        Ok(s) => s,
        Err(e) => core::str::from_utf8(&t[..e.valid_up_to()]).unwrap_or(""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::{KeyCode, Modifiers};
    extern crate std;
    use std::string::String;
    use std::vec::Vec;

    #[test]
    fn ime_messages_round_trip() {
        assert_eq!(
            (
                request::REGISTER_IME,
                request::IME_RESULT,
                request::SET_CARET
            ),
            (0x10C, 0x10D, 0x10E)
        );
        assert_eq!(
            (event::IME_KEY, event::TEXT_INPUT, event::IME_RESET),
            (0x189, 0x18A, 0x18B)
        );
        assert_eq!(word(&register_ime(), 0), 0);
        let h = ImeHeader {
            seq: 7,
            flags: ime::CONSUMED | ime::CHINESE,
            cursor: 2,
            commit_len: 3,
            preedit_len: 6,
            candidates_len: 0,
            page: 1,
            pages: 3,
        };
        let m = ime_result(&h, "你好嗎".as_bytes());
        assert_eq!(m.tag, request::IME_RESULT);
        assert_eq!(word(&m, 0), 0);
        assert_eq!(ime_header_of(&m), h);
        assert_eq!(h.text_len(), 9);
        assert_eq!(ime_text_of(&m), "你好嗎".as_bytes());
        assert_eq!(IME_INLINE_MAX, 80);

        let key = KeyEvent {
            code: KeyCode::S,
            pressed: true,
            modifiers: Modifiers::default(),
        };
        assert_eq!(ime_key_of(&ime_key(9, &key)), Some((9, key)));
        let c = set_caret(4, -1, 20, 16);
        assert_eq!((word(&c, 0), word(&c, 1), signed(&c, 2)), (0, 4, -1));
        assert_eq!((signed(&c, 3), word(&c, 4)), (20, 16));
    }

    #[test]
    fn text_input_is_cut_at_characters() {
        let one: Vec<Message> = text_input(3, "你好").collect();
        assert_eq!(one.len(), 1);
        assert_eq!(word(&one[0], 0), 3);
        assert_eq!(text_of(&one[0]), "你好");
        // 40 three-byte characters: 104 bytes hold 34 of them.
        let long: String = core::iter::repeat_n('字', 40).collect();
        let parts: Vec<Message> = text_input(3, &long).collect();
        assert_eq!(parts.len(), 2);
        let joined: String = parts.iter().map(text_of).collect();
        assert_eq!(joined, long);
        assert_eq!(text_of(&parts[0]).len(), 102);
        assert_eq!(text_input(3, "").count(), 0);
    }

    #[test]
    fn values_are_fixed() {
        assert_eq!((request::CONNECT, request::FOCUS_WINDOW), (0x100, 0x108));
        assert_eq!((event::CONNECTED, event::ERROR), (0x180, 0x187));
        assert_eq!(VERSION, 3);
        assert_eq!(word(&connect(), 0), VERSION);
        assert_eq!(flags::DOCUMENT, 7);
        assert_eq!(flags::ALL, 63);
        assert_eq!((flags::WANT_MOTION, flags::HIDE_CURSOR_INSIDE), (16, 32));
        assert_eq!((mouse::MOVE, mouse::DOWN, mouse::UP), (1, 2, 3));
        assert_eq!(window_event::CLOSE_REQUESTED, 5);
        assert_eq!(
            (
                request::SET_CLIPBOARD,
                request::GET_CLIPBOARD,
                event::CLIPBOARD
            ),
            (0x109, 0x10A, 0x188)
        );
        assert_eq!(CLIPBOARD_MAX, 1 << 20);
        let m = set_clipboard(42);
        assert_eq!(
            (m.tag, word(&m, 0), word(&m, 1)),
            (request::SET_CLIPBOARD, 0, 42)
        );
        assert_eq!(word(&clipboard(7), 0), 7);
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
