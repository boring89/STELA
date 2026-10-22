/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

/* Stand-in <assert.h> for libchewing in the IME (user/ime/c/libc.c). */

#ifndef IME_SHIM_ASSERT_H
#define IME_SHIM_ASSERT_H

void ime_assert_fail(const char *expr, const char *file, int line);
#define assert(e) ((e) ? (void) 0 : ime_assert_fail(#e, __FILE__, __LINE__))

#endif
