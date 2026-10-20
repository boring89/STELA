// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Linux error numbers (`include/uapi/asm-generic/errno-base.h` and
//! `errno.h`) and the mapping from STELA's `abi::Error`. A Linux system
//! call returns `-errno` in RAX.

#![allow(missing_docs)]

use abi::Error;

pub const EPERM: i64 = 1;
pub const ENOENT: i64 = 2;
pub const ESRCH: i64 = 3;
pub const EINTR: i64 = 4;
pub const EIO: i64 = 5;
pub const EBADF: i64 = 9;
pub const ECHILD: i64 = 10;
pub const EAGAIN: i64 = 11;
pub const ENOMEM: i64 = 12;
pub const EACCES: i64 = 13;
pub const EFAULT: i64 = 14;
pub const EBUSY: i64 = 16;
pub const EEXIST: i64 = 17;
pub const EXDEV: i64 = 18;
pub const ENOTDIR: i64 = 20;
pub const EISDIR: i64 = 21;
pub const EINVAL: i64 = 22;
pub const EMFILE: i64 = 24;
pub const ENOTTY: i64 = 25;
pub const EFBIG: i64 = 27;
pub const ENOSPC: i64 = 28;
pub const ESPIPE: i64 = 29;
pub const EROFS: i64 = 30;
pub const ERANGE: i64 = 34;
pub const ENAMETOOLONG: i64 = 36;
pub const ENOSYS: i64 = 38;
pub const ENOTEMPTY: i64 = 39;
pub const ETIMEDOUT: i64 = 110;

/// The errno for a STELA error.
pub fn from_error(e: Error) -> i64 {
    match e {
        Error::InvalidSyscall => ENOSYS,
        Error::InvalidArgument => EINVAL,
        Error::BadAddress => EFAULT,
        Error::OutOfMemory => ENOMEM,
        Error::NotSupported => ENOSYS,
        Error::BadHandle | Error::WrongType => EBADF,
        Error::AccessDenied => EACCES,
        Error::Full => EAGAIN,
        Error::TimedOut => ETIMEDOUT,
        Error::PeerClosed => EIO,
        Error::OutOfHandles => EMFILE,
        Error::AlreadyMapped => ENOMEM,
        Error::NotFound => ENOENT,
        Error::Exists => EEXIST,
        Error::NotDir => ENOTDIR,
        Error::IsDir => EISDIR,
        Error::NotEmpty => ENOTEMPTY,
        Error::ReadOnly => EROFS,
        Error::NoSpace => ENOSPC,
        Error::IoError => EIO,
    }
}

/// The value a Linux system call puts in RAX: the result, or `-errno`.
pub fn encode(result: Result<u64, i64>) -> u64 {
    match result {
        Ok(v) => v,
        Err(errno) => (-errno) as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_and_mapping() {
        assert_eq!((ENOENT, EBADF, EINVAL, ENOSYS, ENOTTY), (2, 9, 22, 38, 25));
        assert_eq!(from_error(Error::NotFound), ENOENT);
        assert_eq!(from_error(Error::ReadOnly), EROFS);
        assert_eq!(from_error(Error::NotEmpty), ENOTEMPTY);
        assert_eq!(encode(Ok(5)), 5);
        assert_eq!(encode(Err(ENOSYS)), (-38i64) as u64);
        assert_eq!(encode(Err(ENOENT)) as i64, -2);
    }
}
