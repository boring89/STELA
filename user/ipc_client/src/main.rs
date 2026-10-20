// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The IPC test client: its bootstrap handle `[2]` is a `SEND` handle to
//! `ipc_server`'s main port. Runs every check in turn, prints
//! `ipc_client: OK` and exits with 0, or exits with the number of the
//! check that failed.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use libstela::ipc::{
    Mapping, handle_close, handle_duplicate, memory_create, port_create, port_receive, port_send,
};
use libstela::{Error, Handle, Message, Rights, bootstrap, debug_println};

libstela::entry!(main);

const PING: u32 = 100;
const PONG: u32 = 101;
const SUM: u32 = 102;
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

    // 6. Shared memory: 3 pages filled here, summed by the server, which
    // writes the last byte back.
    const SIZE: u64 = 3 * 4096;
    let memory = memory_create(SIZE).map_err(|_| 9i64)?;
    let mapping = Mapping::new(memory, SIZE, true).map_err(|_| 9i64)?;
    let mut expected = 0u64;
    let mut page = [0u8; 4096];
    for p in 0..3u64 {
        for (i, b) in page.iter_mut().enumerate() {
            *b = (i as u64 + p) as u8;
            expected += u64::from(*b);
        }
        mapping.write(p * 4096, &page).map_err(|_| 9i64)?;
    }
    let shared = handle_duplicate(
        memory,
        Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
    )
    .map_err(|_| 9i64)?;
    let mut msg_out = Message::with_data(SUM, &SIZE.to_le_bytes());
    let send_only = handle_duplicate(reply, Rights::SEND | Rights::TRANSFER).map_err(|_| 9i64)?;
    msg_out.push_handle(send_only);
    msg_out.push_handle(shared);
    port_send(bootstrap::BOOTSTRAP, &msg_out).map_err(|_| 9i64)?;
    let got = port_receive(reply, &mut msg, SECOND);
    let sum = msg.payload().get(..8).map(|b| {
        let mut w = [0u8; 8];
        w.copy_from_slice(b);
        u64::from_le_bytes(w)
    });
    let mut last = [0u8];
    mapping.read(SIZE - 1, &mut last).map_err(|_| 9i64)?;
    check(
        9,
        got.is_ok() && msg.tag == SUM && sum == Some(expected) && last[0] == 0xA5,
        "shared memory",
    )?;
    // Bad requests: an unaligned size, a port handle, a second unmap.
    check(
        10,
        memory_create(100) == Err(Error::InvalidArgument)
            && libstela::ipc::memory_map(reply, false) == Err(Error::WrongType)
            && libstela::ipc::memory_unmap(0x1234_5000) == Err(Error::InvalidArgument),
        "memory errors",
    )?;
    drop(mapping);
    let _ = handle_close(memory);

    // 7. The server quits; then its port is closed.
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
