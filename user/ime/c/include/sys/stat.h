/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

/* Stand-in <sys/stat.h> for libchewing in the IME (user/ime/c/libc.c). */

#ifndef IME_SHIM_SYS_STAT_H
#define IME_SHIM_SYS_STAT_H

#include <sys/types.h>
#define S_IRWXU 0700
#define mkdir ime_mkdir
int ime_mkdir(const char *path, mode_t mode);

#endif
