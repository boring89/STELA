// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Events (the toolbox's Event Manager): what the window server sends,
//! decoded. Messages that are not window events (`SHELL_READY`, an
//! application's own) come through as [`Event::Other`].

use abi::key::KeyEvent;
use abi::window::{self as proto, event, mouse, window_event};
use libstela::Message;

/// What a mouse event was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    /// The pointer moved with a button held.
    Move,
    /// A button went down.
    Down,
    /// It went up.
    Up,
}

/// A decoded event.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A key, for `window`.
    Key {
        /// The window with the keyboard.
        window: u32,
        /// The key.
        key: KeyEvent,
    },
    /// The mouse, in `window`'s content coordinates.
    Mouse {
        /// The window.
        window: u32,
        /// Content x.
        x: i32,
        /// Content y.
        y: i32,
        /// Buttons held (`abi::key::button`).
        buttons: u8,
        /// Move, down or up.
        kind: MouseKind,
        /// 2 for the second press of a double click.
        clicks: u32,
    },
    /// The window got the keyboard.
    Focus(u32),
    /// It lost it.
    Blur(u32),
    /// The user moved it; the new content position.
    Moved(u32, i32, i32),
    /// The user asks for a new size (content width, height).
    ResizeRequested(u32, u32, u32),
    /// The close box was clicked.
    CloseRequested(u32),
    /// A menu item: menu index, item index.
    Menu(u32, u32),
    /// A request failed: its tag and the error.
    Error(u32, u32),
    /// Anything else.
    Other(Message),
}

impl Event {
    /// Decodes a message from the main port.
    pub fn from_message(msg: Message) -> Event {
        let w = |i| proto::word(&msg, i);
        let s = |i| proto::signed(&msg, i);
        match msg.tag {
            event::KEY => match proto::key_of(&msg) {
                Some(key) => Event::Key { window: w(0), key },
                None => Event::Other(msg),
            },
            event::MOUSE => {
                let kind = match w(4) {
                    mouse::DOWN => MouseKind::Down,
                    mouse::UP => MouseKind::Up,
                    _ => MouseKind::Move,
                };
                Event::Mouse {
                    window: w(0),
                    x: s(1),
                    y: s(2),
                    buttons: w(3) as u8,
                    kind,
                    clicks: w(5),
                }
            }
            event::WINDOW_EVENT => match w(1) {
                window_event::FOCUS => Event::Focus(w(0)),
                window_event::BLUR => Event::Blur(w(0)),
                window_event::MOVED => Event::Moved(w(0), s(2), s(3)),
                window_event::RESIZE_REQUESTED => Event::ResizeRequested(w(0), w(4), w(5)),
                window_event::CLOSE_REQUESTED => Event::CloseRequested(w(0)),
                _ => Event::Other(msg),
            },
            event::MENU_SELECTED => Event::Menu(w(0), w(1)),
            event::ERROR => Event::Error(w(0), w(1)),
            _ => Event::Other(msg),
        }
    }

    /// The window the event is about, if any.
    pub fn window(&self) -> Option<u32> {
        match *self {
            Event::Key { window, .. } | Event::Mouse { window, .. } => Some(window),
            Event::Focus(w)
            | Event::Blur(w)
            | Event::Moved(w, ..)
            | Event::ResizeRequested(w, ..)
            | Event::CloseRequested(w) => Some(w),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use abi::key::{KeyCode, Modifiers};

    #[test]
    fn decodes() {
        let key = KeyEvent {
            code: KeyCode::Enter,
            pressed: true,
            modifiers: Modifiers::from_raw(0),
        };
        assert_eq!(
            Event::from_message(proto::key(3, &key)),
            Event::Key { window: 3, key }
        );
        assert_eq!(
            Event::from_message(proto::mouse_event(4, -1, 2, 1, mouse::DOWN, 2)),
            Event::Mouse {
                window: 4,
                x: -1,
                y: 2,
                buttons: 1,
                kind: MouseKind::Down,
                clicks: 2
            }
        );
        let e = Event::from_message(proto::window_event(
            5,
            window_event::RESIZE_REQUESTED,
            0,
            0,
            300,
            200,
        ));
        assert_eq!(e, Event::ResizeRequested(5, 300, 200));
        assert_eq!(e.window(), Some(5));
        assert_eq!(
            Event::from_message(proto::menu_selected(1, 2)),
            Event::Menu(1, 2)
        );
        let other = Message::with_data(abi::tag::SHELL_READY, b"");
        assert!(matches!(Event::from_message(other), Event::Other(_)));
    }
}
