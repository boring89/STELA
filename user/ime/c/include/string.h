/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

/* Stand-in <string.h> for libchewing in the IME (user/ime/c/libc.c). */

#ifndef IME_SHIM_STRING_H
#define IME_SHIM_STRING_H

#include <stddef.h>
/* memcpy, memmove, memset and memcmp are the real ones: Rust's
   compiler_builtins in STELA, the C library on the host (gcc emits calls
   to them on its own as well). */
void *memcpy(void *d, const void *s, size_t n);
void *memmove(void *d, const void *s, size_t n);
void *memset(void *d, int c, size_t n);
int memcmp(const void *a, const void *b, size_t n);
#define memchr ime_memchr
#define strlen ime_strlen
#define strcmp ime_strcmp
#define strncmp ime_strncmp
#define strcpy ime_strcpy
#define strncpy ime_strncpy
#define strcat ime_strcat
#define strchr ime_strchr
#define strrchr ime_strrchr
#define strstr ime_strstr
#define strpbrk ime_strpbrk
#define strcspn ime_strcspn
#define strspn ime_strspn
#define strdup ime_strdup
#define strtok ime_strtok
#define strtok_r ime_strtok_r
void *ime_memchr(const void *s, int c, size_t n);
size_t ime_strlen(const char *s);
int ime_strcmp(const char *a, const char *b);
int ime_strncmp(const char *a, const char *b, size_t n);
char *ime_strcpy(char *d, const char *s);
char *ime_strncpy(char *d, const char *s, size_t n);
char *ime_strcat(char *d, const char *s);
char *ime_strchr(const char *s, int c);
char *ime_strrchr(const char *s, int c);
char *ime_strstr(const char *h, const char *n);
char *ime_strpbrk(const char *s, const char *accept);
size_t ime_strcspn(const char *s, const char *reject);
size_t ime_strspn(const char *s, const char *accept);
char *ime_strdup(const char *s);
char *ime_strtok(char *s, const char *delim);
char *ime_strtok_r(char *s, const char *delim, char **save);

#endif
