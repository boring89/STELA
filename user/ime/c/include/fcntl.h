/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

/* Stand-in <fcntl.h> for libchewing in the IME (user/ime/c/libc.c). */

#ifndef IME_SHIM_FCNTL_H
#define IME_SHIM_FCNTL_H

#include <sys/types.h>
#define O_RDONLY 0
#define O_WRONLY 1
#define O_RDWR 2
#define O_CREAT 0100
#define O_TRUNC 01000
#define open ime_open
int ime_open(const char *path, int flags, ...);

#endif
