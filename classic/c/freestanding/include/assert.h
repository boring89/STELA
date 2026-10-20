/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#ifndef EMU_M68K_FREESTANDING_ASSERT_H
#define EMU_M68K_FREESTANDING_ASSERT_H

/*
 * Freestanding stand-in for the libc headers Musashi includes. Only used
 * when Musashi is built into the kernel (-isystem in the Makefile).
 */
#define assert(cond) ((void)0)

#endif
