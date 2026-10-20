/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#ifndef EMU_M68K_FREESTANDING_MATH_H
#define EMU_M68K_FREESTANDING_MATH_H

/* Only what Musashi's FPU emulation calls; see freestanding/libc.c. */
double sin(double x);
double cos(double x);

#endif
