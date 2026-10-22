// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The connection to the window server and windows (the toolbox's Window
//! Manager). `CONNECT` goes to the server's main port and gives this
//! client its own request port (protocol version 2); replies and events
//! arrive on the program's main port; events that come while the toolbox
//! waits for a reply are kept for [`Connection::next_event`].

use crate::draw::{Canvas, Rect};
use crate::event::Event;
use abi::window::{self as proto, event};
use alloc::collections::VecDeque;
use libstela::ipc::{
    FOREVER, Mapping, handle_close, handle_duplicate, memory_create, port_receive, port_send,
};
use libstela::{Error, Handle, Message, Rights, bootstrap};

/// How long to wait for the server's replies.
const REPLY_TIMEOUT: u64 = 5_000_000_000;

/// A connection to the window server.
pub struct Connection {
    /// This client's request port (from `CONNECTED`).
    server: Handle,
    client: u32,
    /// The screen's size.
    pub screen: (u32, u32),
    /// The menu bar's height.
    pub menubar: u32,
    /// Physical pixels per logical one (P13 K13): the screen size and
    /// every coordinate are logical.
    pub scale: u32,
    pending: VecDeque<Message>,
    /// Events a dialog took and gave back (`put_back`), returned first.
    returned: VecDeque<Event>,
}

impl Drop for Connection {
    /// Tells the server this client is done (its windows close) and lets
    /// go of the request port.
    fn drop(&mut self) {
        if self.server != Handle::NONE {
            let _ = port_send(
                self.server,
                &Message::with_data(abi::window::request::DISCONNECT, &[0; 4]),
            );
            let _ = handle_close(self.server);
        }
    }
}

impl Connection {
    /// Connects through the bootstrap handle `[2]` (what the window server
    /// gives the programs it starts).
    pub fn open() -> Result<Connection, Error> {
        Connection::open_with(bootstrap::BOOTSTRAP)
    }

    /// Connects to the server whose main port `server` sends to. Replies
    /// and events come to this program's main port.
    pub fn open_with(server: Handle) -> Result<Connection, Error> {
        let reply = handle_duplicate(bootstrap::MAIN_PORT, Rights::SEND | Rights::TRANSFER)?;
        let mut msg = proto::connect();
        msg.push_handle(reply);
        if let Err(e) = port_send(server, &msg) {
            let _ = handle_close(reply);
            return Err(e);
        }
        let mut conn = Connection {
            server: Handle::NONE,
            client: 0,
            screen: (0, 0),
            menubar: 0,
            scale: 1,
            pending: VecDeque::new(),
            returned: VecDeque::new(),
        };
        let m = conn.wait_for(event::CONNECTED)?;
        let Some(&port) = m.carried().first() else {
            return Err(Error::InvalidArgument);
        };
        conn.server = port;
        conn.client = proto::word(&m, 0);
        conn.screen = (proto::word(&m, 1), proto::word(&m, 2));
        conn.scale = proto::word(&m, 3).clamp(1, 4);
        conn.menubar = proto::word(&m, 4);
        Ok(conn)
    }

    /// The client id the server gave.
    pub fn client(&self) -> u32 {
        self.client
    }

    /// Sends a request.
    pub fn send(&self, msg: &Message) -> Result<(), Error> {
        loop {
            match port_send(self.server, msg) {
                Err(Error::Full) => libstela::yield_now(),
                r => return r,
            }
        }
    }

    /// Waits for the message with `tag` (an `ERROR` for the request fails
    /// it), keeping everything else for later.
    fn wait_for(&mut self, tag: u32) -> Result<Message, Error> {
        let deadline = libstela::now_ns() + REPLY_TIMEOUT;
        loop {
            let now = libstela::now_ns();
            if now >= deadline {
                return Err(Error::TimedOut);
            }
            let mut msg = Message::new();
            port_receive(bootstrap::MAIN_PORT, &mut msg, deadline - now)?;
            if msg.tag == tag {
                return Ok(msg);
            }
            if msg.tag == event::ERROR && proto::word(&msg, 0) != 0 {
                let code = u64::from(proto::word(&msg, 1));
                return Err(Error::from_raw(code).unwrap_or(Error::InvalidArgument));
            }
            self.pending.push_back(msg);
        }
    }

    /// Waits up to `timeout_ns` for the message with `tag` (P13 K16: the
    /// powerbox's answer), keeping everything else for later; `None` when
    /// the time is up or the request `request` failed.
    pub fn wait_message(&mut self, tag: u32, request: u32, timeout_ns: u64) -> Option<Message> {
        let deadline = libstela::now_ns().saturating_add(timeout_ns);
        loop {
            let now = libstela::now_ns();
            if now >= deadline {
                return None;
            }
            let mut msg = Message::new();
            port_receive(bootstrap::MAIN_PORT, &mut msg, deadline - now).ok()?;
            if msg.tag == tag {
                return Some(msg);
            }
            if msg.tag == event::ERROR && proto::word(&msg, 0) == request {
                return None;
            }
            self.pending.push_back(msg);
        }
    }

    /// The next event, waiting up to `timeout_ns` (`FOREVER`); `None` when
    /// the time is up.
    pub fn next_event(&mut self, timeout_ns: u64) -> Option<Event> {
        if let Some(e) = self.returned.pop_front() {
            return Some(e);
        }
        if let Some(msg) = self.pending.pop_front() {
            return Some(Event::from_message(msg));
        }
        let mut msg = Message::new();
        port_receive(bootstrap::MAIN_PORT, &mut msg, timeout_ns).ok()?;
        Some(Event::from_message(msg))
    }

    /// Whether an event is kept from waiting for a reply (a program that
    /// waits on its main port with other objects checks this first).
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty() || !self.returned.is_empty()
    }

    /// Gives events back to be returned again (in this order) before any
    /// other: a dialog keeps the events of the program's other windows
    /// while it runs.
    pub fn put_back(&mut self, events: impl IntoIterator<Item = Event>) {
        for (i, e) in events.into_iter().enumerate() {
            self.returned.insert(i, e);
        }
    }

    /// Puts `text` on the system clipboard (P11; at most
    /// `abi::window::CLIPBOARD_MAX` bytes).
    pub fn set_clipboard(&self, text: &str) -> Result<(), Error> {
        if text.len() > proto::CLIPBOARD_MAX {
            return Err(Error::InvalidArgument);
        }
        let mut msg = proto::set_clipboard(text.len() as u32);
        if !text.is_empty() {
            let size = (text.len() as u64).next_multiple_of(4096);
            let memory = memory_create(size)?;
            {
                let map = Mapping::new(memory, size, true)?;
                map.write(0, text.as_bytes())?;
            }
            msg.push_handle(memory);
        }
        self.send(&msg)
    }

    /// The system clipboard's text (empty when there is none).
    pub fn clipboard(&mut self) -> Result<alloc::string::String, Error> {
        self.send(&proto::get_clipboard())?;
        let reply = self.wait_for(event::CLIPBOARD)?;
        let len = proto::word(&reply, 0) as usize;
        let Some(&memory) = reply.carried().first() else {
            return Ok(alloc::string::String::new());
        };
        let size = (len as u64).next_multiple_of(4096);
        let mut text = alloc::vec![0u8; len.min(proto::CLIPBOARD_MAX)];
        let read = Mapping::new(memory, size, false).and_then(|map| map.read(0, &mut text));
        let _ = handle_close(memory);
        read?;
        Ok(alloc::string::String::from_utf8_lossy(&text).into_owned())
    }

    /// Creates a window of `w` x `h` content pixels at (x, y)
    /// (`abi::window::AUTO`: the server places it).
    pub fn create_window(
        &mut self,
        w: u32,
        h: u32,
        flags: u32,
        x: i32,
        y: i32,
        title: &str,
    ) -> Result<Window, Error> {
        self.send(&proto::create_window(w, h, flags, x, y, title))?;
        let reply = self.wait_for(event::WINDOW_CREATED)?;
        Window::from_reply(&reply, self.scale)
    }

    /// Installs the menu bar text (`toolbox::menu`).
    pub fn set_menubar(&self, text: &str) -> Result<(), Error> {
        let size = (text.len() as u64).next_multiple_of(4096).max(4096);
        let memory = memory_create(size)?;
        {
            let map = Mapping::new(memory, size, true)?;
            map.write(0, text.as_bytes())?;
        }
        let mut msg = proto::set_menubar(text.len() as u32);
        msg.push_handle(memory);
        self.send(&msg)
    }

    /// Shows the part `r` (window coordinates) of `window` again.
    pub fn damage(&self, window: &Window, r: Rect) -> Result<(), Error> {
        let r = r.intersect(&window.bounds());
        if r.is_empty() {
            return Ok(());
        }
        self.send(&proto::damage(window.id, r.x, r.y, r.w as u32, r.h as u32))
    }

    /// Shows all of `window` again.
    pub fn damage_all(&self, window: &Window) -> Result<(), Error> {
        self.damage(window, window.bounds())
    }

    /// Gives `window` a new title.
    pub fn set_title(&self, window: &Window, title: &str) -> Result<(), Error> {
        self.send(&proto::set_title(window.id, title))
    }

    /// Moves `window`'s content to (x, y) on the screen.
    pub fn move_window(&self, window: &Window, x: i32, y: i32) -> Result<(), Error> {
        self.send(&proto::move_window(window.id, x, y))
    }

    /// Tells the server where the window's text cursor is (window
    /// coordinates; P13 K3): the IME draws what is being composed there.
    pub fn set_caret(&self, window: &Window, x: i32, y: i32, h: i32) -> Result<(), Error> {
        self.send(&proto::set_caret(window.id, x, y, h.max(1) as u32))
    }

    /// Brings `window` to the front with the keyboard.
    pub fn focus(&self, window: &Window) -> Result<(), Error> {
        self.send(&proto::focus_window(window.id))
    }

    /// Gives `window` a new size (and a new, blank buffer).
    pub fn resize(&mut self, window: &mut Window, w: u32, h: u32) -> Result<(), Error> {
        self.send(&proto::resize_window(window.id, w, h))?;
        let reply = self.wait_for(event::WINDOW_RESIZED)?;
        *window = Window::from_reply(&reply, self.scale)?;
        Ok(())
    }

    /// Closes `window`.
    pub fn close(&self, window: Window) -> Result<(), Error> {
        self.send(&proto::close_window(window.id))
    }
}

/// A window: its id and its pixels (this program draws, the server
/// shows).
pub struct Window {
    /// The server's id.
    pub id: u32,
    /// Content width.
    pub width: u32,
    /// Content height.
    pub height: u32,
    /// Physical pixels per logical one (the buffer is `width * scale` x
    /// `height * scale`).
    pub scale: u32,
    mapping: Mapping,
}

impl Window {
    fn from_reply(reply: &Message, scale: u32) -> Result<Window, Error> {
        let Some(&memory) = reply.carried().first() else {
            return Err(Error::InvalidArgument);
        };
        let (id, width, height) = (
            proto::word(reply, 0),
            proto::word(reply, 1),
            proto::word(reply, 2),
        );
        let s = u64::from(scale);
        let size = (u64::from(width) * s * u64::from(height) * s * 4).next_multiple_of(4096);
        let mapping = Mapping::new(memory, size, true);
        // The mapping keeps the memory; the handle is not needed.
        let _ = handle_close(memory);
        Ok(Window {
            id,
            width,
            height,
            scale,
            mapping: mapping?,
        })
    }

    /// The content rectangle in window coordinates.
    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width as i32, self.height as i32)
    }

    /// Something to draw on (call [`Connection::damage`] after).
    pub fn canvas(&mut self) -> Canvas<'_> {
        let s = self.scale;
        let (w, h) = (self.width * s, self.height * s);
        // A writable mapping always gives its words.
        let words = self.mapping.words_mut().unwrap_or_default();
        Canvas::scaled(words, w, h, w as usize, s)
    }
}

/// Waits forever for the next event.
pub fn wait(conn: &mut Connection) -> Event {
    loop {
        if let Some(e) = conn.next_event(FOREVER) {
            return e;
        }
    }
}
