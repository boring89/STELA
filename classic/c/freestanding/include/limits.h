/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#ifndef EMU_M68K_FREESTANDING_LIMITS_H
#define EMU_M68K_FREESTANDING_LIMITS_H

/* The compiler's own limits, without the libc header GCC would chain to. */
#define CHAR_BIT __CHAR_BIT__
#define UCHAR_MAX (__SCHAR_MAX__ * 2 + 1)
#define USHRT_MAX (__SHRT_MAX__ * 2 + 1)
#define INT_MAX __INT_MAX__
#define INT_MIN (-INT_MAX - 1)
#define UINT_MAX (INT_MAX * 2U + 1U)

#endif
