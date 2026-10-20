// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The IPC test server: answers requests on its main port until it is told
//! to quit. Each request carries a reply port (a `SEND` handle) in its
//! first handle slot.
//!
//! | tag | request | reply |
//! |---|---|---|
//! | `PING` | any data | `PONG` with the data reversed |
//! | `QUIT` | — | `BYE`, then the server exits with 0 |

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use libstela::ipc::{FOREVER, handle_close, port_receive, port_send};
use libstela::{Handle, Message, bootstrap, debug_println};

libstela::entry!(main);

const PING: u32 = 100;
const PONG: u32 = 101;
const QUIT: u32 = 103;
const BYE: u32 = 104;

fn reply(to: Handle, msg: &Message) {
    if let Err(e) = port_send(to, msg) {
        debug_println!("ipc_server: reply failed: {}", e.name());
    }
    let _ = handle_close(to);
}

fn main() -> i64 {
    let mut msg = Message::new();
    loop {
        if let Err(e) = port_receive(bootstrap::MAIN_PORT, &mut msg, FOREVER) {
            debug_println!("ipc_server: receive failed: {}", e.name());
            return 1;
        }
        let Some(&reply_to) = msg.carried().first() else {
            debug_println!("ipc_server: request {} without a reply port", msg.tag);
            continue;
        };
        match msg.tag {
            PING => {
                let mut data = [0u8; abi_len()];
                let payload = msg.payload();
                for (i, b) in payload.iter().rev().enumerate() {
                    data[i] = *b;
                }
                reply(reply_to, &Message::with_data(PONG, &data[..payload.len()]));
            }
            QUIT => {
                reply(reply_to, &Message::with_data(BYE, b""));
                debug_println!("ipc_server: bye");
                return 0;
            }
            other => {
                debug_println!("ipc_server: unknown request {other}");
                let _ = handle_close(reply_to);
            }
        }
    }
}

const fn abi_len() -> usize {
    libstela::MESSAGE_DATA
}
