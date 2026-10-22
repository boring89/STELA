/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

/* Stand-in <ctype.h> for libchewing in the IME (user/ime/c/libc.c). */

#ifndef IME_SHIM_CTYPE_H
#define IME_SHIM_CTYPE_H

#define isalpha ime_isalpha
#define isdigit ime_isdigit
#define isprint ime_isprint
#define isupper ime_isupper
#define islower ime_islower
#define isspace ime_isspace
#define tolower ime_tolower
#define toupper ime_toupper
int ime_isalpha(int c);
int ime_isdigit(int c);
int ime_isprint(int c);
int ime_isupper(int c);
int ime_islower(int c);
int ime_isspace(int c);
int ime_tolower(int c);
int ime_toupper(int c);

#endif
