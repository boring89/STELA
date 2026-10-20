// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The IPC test server: answers requests on its main port until it is told
//! to quit. Each request carries a reply port (a `SEND` handle) in its
//! first handle slot.
//!
//! | tag | request | reply |
//! |---|---|---|
//! | `PING` | any data | `PONG` with the data reversed |
//! | `SUM` | size (u64), memory handle in slot 1 | `SUM` with the sum of the bytes (u64); writes `0xA5` to the last byte |
//! | `QUIT` | — | `BYE`, then the server exits with 0 |

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use libstela::ipc::{FOREVER, Mapping, handle_close, port_receive, port_send};
use libstela::{Handle, Message, bootstrap, debug_println};

libstela::entry!(main);

const PING: u32 = 100;
const PONG: u32 = 101;
const SUM: u32 = 102;
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
            SUM => {
                let sum = sum_memory(&msg).unwrap_or(u64::MAX);
                reply(reply_to, &Message::with_data(SUM, &sum.to_le_bytes()));
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

/// Maps the memory of a `SUM` request, adds up its bytes and marks the
/// last one.
fn sum_memory(msg: &Message) -> Option<u64> {
    let &memory = msg.carried().get(1)?;
    let size = u64::from_le_bytes(msg.payload().get(..8)?.try_into().ok()?);
    let mapping = Mapping::new(memory, size, true).ok()?;
    let mut sum = 0u64;
    let mut page = [0u8; 4096];
    let mut at = 0;
    while at < size {
        mapping.read(at, &mut page).ok()?;
        sum += page.iter().map(|&b| u64::from(b)).sum::<u64>();
        at += 4096;
    }
    mapping.write(size - 1, &[0xA5]).ok()?;
    drop(mapping);
    let _ = handle_close(memory);
    Some(sum)
}

const fn abi_len() -> usize {
    libstela::MESSAGE_DATA
}
