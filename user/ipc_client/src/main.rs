// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The IPC test client: its bootstrap handle `[2]` is a `SEND` handle to
//! `ipc_server`'s main port. Runs every check in turn, prints
//! `ipc_client: OK` and exits with 0, or exits with the number of the
//! check that failed.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use libstela::ipc::{handle_close, handle_duplicate, port_create, port_receive, port_send};
use libstela::{Error, Handle, Message, Rights, bootstrap, debug_println};

libstela::entry!(main);

const PING: u32 = 100;
const PONG: u32 = 101;
const QUIT: u32 = 103;
const BYE: u32 = 104;

/// One second.
const SECOND: u64 = 1_000_000_000;

/// Sends `msg` to the server with a fresh `SEND` duplicate of `reply` as
/// its first handle. Returns the duplicate's (now dead) value.
fn request(reply: Handle, mut msg: Message) -> Result<Handle, Error> {
    let send_only = handle_duplicate(reply, Rights::SEND | Rights::TRANSFER)?;
    msg.push_handle(send_only);
    port_send(bootstrap::BOOTSTRAP, &msg)?;
    Ok(send_only)
}

fn check(step: i64, ok: bool, what: &str) -> Result<(), i64> {
    if ok {
        debug_println!("ipc_client: {what} ok");
        Ok(())
    } else {
        debug_println!("ipc_client: {what} FAILED");
        Err(step)
    }
}

fn run() -> Result<(), i64> {
    let reply = port_create().map_err(|_| 1i64)?;
    let mut msg = Message::new();

    // 1. Request and reply.
    let sent = request(reply, Message::with_data(PING, b"stela")).map_err(|_| 2i64)?;
    let got = port_receive(reply, &mut msg, SECOND);
    check(
        2,
        got.is_ok() && msg.tag == PONG && msg.payload() == b"alets",
        "request and reply",
    )?;

    // 2. A handle that was sent is gone from this process.
    check(
        3,
        handle_close(sent) == Err(Error::BadHandle),
        "moved handle",
    )?;

    // 3. Rights: no duplicate with more rights; no send without SEND.
    let receive_only = handle_duplicate(reply, Rights::RECEIVE).map_err(|_| 4i64)?;
    check(
        4,
        port_send(receive_only, &Message::new()) == Err(Error::AccessDenied)
            && handle_duplicate(receive_only, Rights::RECEIVE) == Err(Error::AccessDenied)
            && port_receive(Handle::NONE, &mut msg, 0) == Err(Error::BadHandle),
        "rights",
    )?;
    let _ = handle_close(receive_only);

    // 4. A full queue.
    let own = port_create().map_err(|_| 5i64)?;
    let mut sent = 0;
    while port_send(own, &Message::with_data(1, b"x")).is_ok() {
        sent += 1;
        if sent > 1000 {
            break;
        }
    }
    check(
        5,
        sent == libstela::PORT_QUEUE_DEPTH && port_send(own, &Message::new()) == Err(Error::Full),
        "full queue",
    )?;
    let _ = handle_close(own);

    // 5. Timeouts: an empty port, without waiting and with 10 ms.
    let start = libstela::now_ns();
    let polled = port_receive(reply, &mut msg, 0);
    let waited = port_receive(reply, &mut msg, 10_000_000);
    let took = libstela::now_ns() - start;
    check(
        6,
        polled == Err(Error::TimedOut) && waited == Err(Error::TimedOut) && took >= 10_000_000,
        "timeouts",
    )?;

    // 6. The server quits; then its port is closed.
    request(reply, Message::with_data(QUIT, b"")).map_err(|_| 7i64)?;
    let got = port_receive(reply, &mut msg, SECOND);
    check(7, got.is_ok() && msg.tag == BYE, "quit")?;
    let mut closed = false;
    for _ in 0..1000 {
        match port_send(bootstrap::BOOTSTRAP, &Message::new()) {
            Err(Error::PeerClosed) => {
                closed = true;
                break;
            }
            _ => libstela::sleep_ms(1),
        }
    }
    check(8, closed, "peer closed")?;
    Ok(())
}

fn main() -> i64 {
    match run() {
        Ok(()) => {
            debug_println!("ipc_client: OK");
            0
        }
        Err(step) => step,
    }
}
