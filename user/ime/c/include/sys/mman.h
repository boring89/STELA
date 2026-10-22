/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

/* Stand-in <sys/mman.h> for libchewing in the IME (user/ime/c/libc.c). */

#ifndef IME_SHIM_SYS_MMAN_H
#define IME_SHIM_SYS_MMAN_H

#include <sys/types.h>
#define PROT_READ 1
#define PROT_WRITE 2
#define MAP_SHARED 1
#define MAP_PRIVATE 2
#define MAP_FAILED ((void *) -1)
#define mmap ime_mmap
#define munmap ime_munmap
void *ime_mmap(void *addr, size_t len, int prot, int flags, int fd, off_t off);
int ime_munmap(void *addr, size_t len);

#endif
