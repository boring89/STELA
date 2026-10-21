// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Standard input (P11): the `[5]` a program was given (`bootstrap::STDIN`:
//! a pipe's reading end, or a file); the output is `[1]` (`println!`).

use crate::sync::SpinMutex;
use abi::{Error, bootstrap, object_kind};

/// Where the next read of a file given as input starts.
static OFFSET: SpinMutex<u64> = SpinMutex::new(0);

/// Whether the program has an input (`[5]`).
pub fn has_stdin() -> bool {
    matches!(
        crate::ipc::handle_kind(bootstrap::STDIN),
        Ok(object_kind::PIPE_READ | object_kind::FILE)
    )
}

/// Reads from the input, waiting for some; 0 at its end. `NotSupported`
/// without one.
pub fn read(buf: &mut [u8]) -> Result<usize, Error> {
    match crate::ipc::handle_kind(bootstrap::STDIN)? {
        object_kind::PIPE_READ => crate::pipe::read(bootstrap::STDIN, buf, crate::ipc::FOREVER),
        object_kind::FILE => {
            let mut at = OFFSET.lock();
            let n = crate::fs::read(bootstrap::STDIN, buf, *at)?;
            *at += n as u64;
            Ok(n)
        }
        _ => Err(Error::NotSupported),
    }
}
