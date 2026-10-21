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
pub const POLL: u64 = 7;
pub const LSEEK: u64 = 8;
pub const MMAP: u64 = 9;
pub const MPROTECT: u64 = 10;
pub const MUNMAP: u64 = 11;
pub const BRK: u64 = 12;
pub const RT_SIGACTION: u64 = 13;
pub const RT_SIGPROCMASK: u64 = 14;
pub const RT_SIGRETURN: u64 = 15;
pub const IOCTL: u64 = 16;
pub const PREAD64: u64 = 17;
pub const PWRITE64: u64 = 18;
pub const READV: u64 = 19;
pub const WRITEV: u64 = 20;
pub const ACCESS: u64 = 21;
pub const PIPE: u64 = 22;
pub const SELECT: u64 = 23;
pub const SCHED_YIELD: u64 = 24;
pub const MREMAP: u64 = 25;
pub const MADVISE: u64 = 28;
pub const DUP: u64 = 32;
pub const DUP2: u64 = 33;
pub const PAUSE: u64 = 34;
pub const NANOSLEEP: u64 = 35;
pub const GETITIMER: u64 = 36;
pub const ALARM: u64 = 37;
pub const SETITIMER: u64 = 38;
pub const GETPID: u64 = 39;
pub const SOCKET: u64 = 41;
pub const CONNECT: u64 = 42;
pub const ACCEPT: u64 = 43;
pub const SENDTO: u64 = 44;
pub const RECVFROM: u64 = 45;
pub const SENDMSG: u64 = 46;
pub const RECVMSG: u64 = 47;
pub const SHUTDOWN: u64 = 48;
pub const BIND: u64 = 49;
pub const LISTEN: u64 = 50;
pub const GETSOCKNAME: u64 = 51;
pub const GETPEERNAME: u64 = 52;
pub const SETSOCKOPT: u64 = 54;
pub const GETSOCKOPT: u64 = 55;
pub const CLONE: u64 = 56;
pub const FORK: u64 = 57;
pub const VFORK: u64 = 58;
pub const EXECVE: u64 = 59;
pub const EXIT: u64 = 60;
pub const WAIT4: u64 = 61;
pub const KILL: u64 = 62;
pub const UNAME: u64 = 63;
pub const FCNTL: u64 = 72;
pub const FSYNC: u64 = 74;
pub const FDATASYNC: u64 = 75;
pub const TRUNCATE: u64 = 76;
pub const FTRUNCATE: u64 = 77;
pub const GETCWD: u64 = 79;
pub const CHDIR: u64 = 80;
pub const FCHDIR: u64 = 81;
pub const RENAME: u64 = 82;
pub const MKDIR: u64 = 83;
pub const RMDIR: u64 = 84;
pub const LINK: u64 = 86;
pub const UNLINK: u64 = 87;
pub const SYMLINK: u64 = 88;
pub const READLINK: u64 = 89;
pub const CHMOD: u64 = 90;
pub const FCHMOD: u64 = 91;
pub const CHOWN: u64 = 92;
pub const FCHOWN: u64 = 93;
pub const LCHOWN: u64 = 94;
pub const UMASK: u64 = 95;
pub const GETTIMEOFDAY: u64 = 96;
pub const GETRLIMIT: u64 = 97;
pub const GETRUSAGE: u64 = 98;
pub const SYSINFO: u64 = 99;
pub const TIMES: u64 = 100;
pub const GETUID: u64 = 102;
pub const GETGID: u64 = 104;
pub const SETUID: u64 = 105;
pub const SETGID: u64 = 106;
pub const GETEUID: u64 = 107;
pub const GETEGID: u64 = 108;
pub const SETPGID: u64 = 109;
pub const GETPPID: u64 = 110;
pub const GETPGRP: u64 = 111;
pub const SETSID: u64 = 112;
pub const GETGROUPS: u64 = 115;
pub const GETPGID: u64 = 121;
pub const GETSID: u64 = 124;
pub const RT_SIGPENDING: u64 = 127;
pub const RT_SIGSUSPEND: u64 = 130;
pub const SIGALTSTACK: u64 = 131;
pub const UTIME: u64 = 132;
pub const SCHED_GETPARAM: u64 = 143;
pub const SCHED_SETSCHEDULER: u64 = 144;
pub const SCHED_GETSCHEDULER: u64 = 145;
pub const PRCTL: u64 = 157;
pub const ARCH_PRCTL: u64 = 158;
pub const SETRLIMIT: u64 = 160;
pub const SYNC: u64 = 162;
pub const GETTID: u64 = 186;
pub const TKILL: u64 = 200;
pub const FUTEX: u64 = 202;
pub const SCHED_SETAFFINITY: u64 = 203;
pub const SCHED_GETAFFINITY: u64 = 204;
pub const GETDENTS64: u64 = 217;
pub const SET_TID_ADDRESS: u64 = 218;
pub const CLOCK_GETTIME: u64 = 228;
pub const CLOCK_NANOSLEEP: u64 = 230;
pub const EXIT_GROUP: u64 = 231;
pub const TGKILL: u64 = 234;
pub const UTIMES: u64 = 235;
pub const WAITID: u64 = 247;
pub const OPENAT: u64 = 257;
pub const MKDIRAT: u64 = 258;
pub const FCHOWNAT: u64 = 260;
pub const NEWFSTATAT: u64 = 262;
pub const UNLINKAT: u64 = 263;
pub const RENAMEAT: u64 = 264;
pub const LINKAT: u64 = 265;
pub const SYMLINKAT: u64 = 266;
pub const READLINKAT: u64 = 267;
pub const FCHMODAT: u64 = 268;
pub const FACCESSAT: u64 = 269;
pub const PSELECT6: u64 = 270;
pub const PPOLL: u64 = 271;
pub const SET_ROBUST_LIST: u64 = 273;
pub const GET_ROBUST_LIST: u64 = 274;
pub const UTIMENSAT: u64 = 280;
pub const ACCEPT4: u64 = 288;
pub const DUP3: u64 = 292;
pub const PIPE2: u64 = 293;
pub const PRLIMIT64: u64 = 302;
pub const SYNCFS: u64 = 306;
pub const GETCPU: u64 = 309;
pub const GETRANDOM: u64 = 318;
pub const EXECVEAT: u64 = 322;
pub const MEMBARRIER: u64 = 324;
pub const STATX: u64 = 332;
pub const RSEQ: u64 = 334;
pub const CLONE3: u64 = 435;
pub const FACCESSAT2: u64 = 439;

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
        assert_eq!((POLL, SELECT, PSELECT6, PPOLL), (7, 23, 270, 271));
        assert_eq!((SENDMSG, RECVMSG), (46, 47));
        assert_eq!((RT_SIGRETURN, WAIT4, KILL, TKILL), (15, 61, 62, 200));
        assert_eq!((TGKILL, WAITID, EXECVEAT, PRLIMIT64), (234, 247, 322, 302));
        assert_eq!((RT_SIGSUSPEND, SIGALTSTACK, UTIMENSAT), (130, 131, 280));
    }
}
