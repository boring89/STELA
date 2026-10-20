/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#ifndef EMU_M68K_FREESTANDING_STDIO_H
#define EMU_M68K_FREESTANDING_STDIO_H

#include <stdarg.h>
#include <stddef.h>

/* Only what Musashi calls; see freestanding/libc.c. */
typedef struct emu_file FILE;

extern FILE *stderr;

int fprintf(FILE *stream, const char *format, ...);
int vfprintf(FILE *stream, const char *format, va_list ap);
int sprintf(char *str, const char *format, ...);
int sscanf(const char *str, const char *format, ...);

#endif
