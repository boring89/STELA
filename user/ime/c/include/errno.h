/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

/* Stand-in <errno.h> for libchewing in the IME (user/ime/c/libc.c). */

#ifndef IME_SHIM_ERRNO_H
#define IME_SHIM_ERRNO_H

#define errno ime_errno
extern int ime_errno;
#define ENOENT 2
#define EIO 5
#define ENOMEM 12
#define EACCES 13
#define EEXIST 17
#define EINVAL 22

#endif
