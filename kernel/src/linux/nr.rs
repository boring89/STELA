// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Linux x86-64 system-call numbers (`arch/x86/entry/syscalls/
//! syscall_64.tbl` in the Linux sources, the same values as musl's
//! `arch/x86_64/bits/syscall.h.in`). Only the ones STELA knows.

#![allow(missing_docs)]

pub const READ: u64 = 0;
pub const WRITE: u64 = 1;
pub const OPEN: u64 = 2;
pub const CLOSE: u64 = 3;
pub const STAT: u64 = 4;
pub const FSTAT: u64 = 5;
pub const LSTAT: u64 = 6;
pub const LSEEK: u64 = 8;
pub const MMAP: u64 = 9;
pub const MPROTECT: u64 = 10;
pub const MUNMAP: u64 = 11;
pub const BRK: u64 = 12;
pub const RT_SIGACTION: u64 = 13;
pub const RT_SIGPROCMASK: u64 = 14;
pub const IOCTL: u64 = 16;
pub const READV: u64 = 19;
pub const WRITEV: u64 = 20;
pub const ACCESS: u64 = 21;
pub const PIPE: u64 = 22;
pub const DUP: u64 = 32;
pub const DUP2: u64 = 33;
pub const NANOSLEEP: u64 = 35;
pub const GETPID: u64 = 39;
pub const CLONE: u64 = 56;
pub const FORK: u64 = 57;
pub const VFORK: u64 = 58;
pub const EXECVE: u64 = 59;
pub const EXIT: u64 = 60;
pub const UNAME: u64 = 63;
pub const FCNTL: u64 = 72;
pub const GETCWD: u64 = 79;
pub const CHDIR: u64 = 80;
pub const RENAME: u64 = 82;
pub const MKDIR: u64 = 83;
pub const RMDIR: u64 = 84;
pub const UNLINK: u64 = 87;
pub const READLINK: u64 = 89;
pub const GETTIMEOFDAY: u64 = 96;
pub const GETUID: u64 = 102;
pub const GETGID: u64 = 104;
pub const GETEUID: u64 = 107;
pub const GETEGID: u64 = 108;
pub const GETPPID: u64 = 110;
pub const ARCH_PRCTL: u64 = 158;
pub const GETTID: u64 = 186;
pub const FUTEX: u64 = 202;
pub const GETDENTS64: u64 = 217;
pub const SET_TID_ADDRESS: u64 = 218;
pub const CLOCK_GETTIME: u64 = 228;
pub const CLOCK_NANOSLEEP: u64 = 230;
pub const EXIT_GROUP: u64 = 231;
pub const OPENAT: u64 = 257;
pub const MKDIRAT: u64 = 258;
pub const NEWFSTATAT: u64 = 262;
pub const UNLINKAT: u64 = 263;
pub const RENAMEAT: u64 = 264;
pub const READLINKAT: u64 = 267;
pub const FACCESSAT: u64 = 269;
pub const PIPE2: u64 = 293;
pub const GETRANDOM: u64 = 318;
pub const CLONE3: u64 = 435;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_match_linux() {
        // A few from each range, checked against syscall_64.tbl.
        assert_eq!((READ, WRITE, OPEN, CLOSE), (0, 1, 2, 3));
        assert_eq!((MMAP, BRK, IOCTL, WRITEV), (9, 12, 16, 20));
        assert_eq!((CLONE, FORK, EXECVE, EXIT), (56, 57, 59, 60));
        assert_eq!((UNAME, FCNTL, GETCWD, UNLINK), (63, 72, 79, 87));
        assert_eq!(
            (ARCH_PRCTL, GETTID, FUTEX, GETDENTS64),
            (158, 186, 202, 217)
        );
        assert_eq!(
            (CLOCK_GETTIME, EXIT_GROUP, OPENAT, NEWFSTATAT),
            (228, 231, 257, 262)
        );
        assert_eq!((PIPE2, GETRANDOM, CLONE3), (293, 318, 435));
    }
}
