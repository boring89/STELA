/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

/* Stand-in <sys/types.h> for libchewing in the IME (user/ime/c/libc.c). */

#ifndef IME_SHIM_SYS_TYPES_H
#define IME_SHIM_SYS_TYPES_H

#include <stddef.h>
#include <stdint.h>
typedef long ssize_t;
typedef long off_t;
typedef unsigned int mode_t;

#endif
