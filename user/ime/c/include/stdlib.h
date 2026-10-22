/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

/* Stand-in <stdlib.h> for libchewing in the IME (user/ime/c/libc.c). */

#ifndef IME_SHIM_STDLIB_H
#define IME_SHIM_STDLIB_H

#include <stddef.h>
#define malloc ime_malloc
#define calloc ime_calloc
#define realloc ime_realloc
#define free ime_free
#define exit ime_exit
#define abort() ime_exit(134)
#define getenv ime_getenv
#define qsort ime_qsort
#define atoi ime_atoi
#define abs ime_abs
#define bsearch ime_bsearch
void *ime_malloc(size_t size);
void *ime_calloc(size_t n, size_t size);
void *ime_realloc(void *p, size_t size);
void ime_free(void *p);
void ime_exit(int status) __attribute__((noreturn));
char *ime_getenv(const char *name);
void ime_qsort(void *base, size_t n, size_t size, int (*cmp)(const void *, const void *));
int ime_atoi(const char *s);
int ime_abs(int x);
void *ime_bsearch(const void *key, const void *base, size_t n, size_t size,
                  int (*cmp)(const void *, const void *));

#endif
