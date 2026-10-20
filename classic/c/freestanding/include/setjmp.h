/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#ifndef EMU_M68K_FREESTANDING_SETJMP_H
#define EMU_M68K_FREESTANDING_SETJMP_H

/*
 * Musashi uses setjmp/longjmp to unwind out of an instruction on an address
 * or bus error. GCC's builtins do this without a libc (GCC manual, "Nonlocal
 * Gotos"): the buffer is five words, and __builtin_longjmp only accepts the
 * value 1, which is the only value Musashi passes.
 */
typedef void *jmp_buf[5];

#define setjmp(env) __builtin_setjmp(env)
#define longjmp(env, val) __builtin_longjmp(env, 1)

#endif
