/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

/* Stand-in <unistd.h> for libchewing in the IME (user/ime/c/libc.c). */

#ifndef IME_SHIM_UNISTD_H
#define IME_SHIM_UNISTD_H

#include <sys/types.h>
#define R_OK 4
#define W_OK 2
#define X_OK 1
#define F_OK 0
#ifndef SEEK_SET
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
#endif
#define access ime_access
#define close ime_close
#define lseek ime_lseek
#define unlink ime_unlink
#define getpagesize ime_getpagesize
int ime_access(const char *path, int mode);
int ime_close(int fd);
off_t ime_lseek(int fd, off_t off, int whence);
int ime_unlink(const char *path);
int ime_getpagesize(void);

#endif
