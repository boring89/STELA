/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

/* Stand-in <stdio.h> for libchewing in the IME (user/ime/c/libc.c). */

#ifndef IME_SHIM_STDIO_H
#define IME_SHIM_STDIO_H

#include <stdarg.h>
#include <stddef.h>
typedef struct ime_file FILE;
#define EOF (-1)
#ifndef SEEK_SET
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
#endif
#define fopen ime_fopen
#define fclose ime_fclose
#define fflush ime_fflush
#define fread ime_fread
#define fwrite ime_fwrite
#define fseek ime_fseek
#define ftell ime_ftell
#define fgets ime_fgets
#define fgetc ime_fgetc
#define getc ime_fgetc
#define feof ime_feof
#define fscanf ime_fscanf
#define fprintf ime_fprintf
#define vfprintf ime_vfprintf
#define printf ime_printf
#define sprintf ime_sprintf
#define snprintf ime_snprintf
#define vsnprintf ime_vsnprintf
#define asprintf ime_asprintf
#define rename ime_rename
#define stderr ((FILE *) 0)
#define stdout ((FILE *) 0)
FILE *ime_fopen(const char *path, const char *mode);
int ime_fclose(FILE *f);
int ime_fflush(FILE *f);
size_t ime_fread(void *buf, size_t size, size_t n, FILE *f);
size_t ime_fwrite(const void *buf, size_t size, size_t n, FILE *f);
int ime_fseek(FILE *f, long off, int whence);
long ime_ftell(FILE *f);
char *ime_fgets(char *s, int n, FILE *f);
int ime_fgetc(FILE *f);
int ime_feof(FILE *f);
int ime_fscanf(FILE *f, const char *fmt, ...);
int ime_fprintf(FILE *f, const char *fmt, ...);
int ime_vfprintf(FILE *f, const char *fmt, va_list ap);
int ime_printf(const char *fmt, ...);
int ime_sprintf(char *s, const char *fmt, ...);
int ime_snprintf(char *s, size_t n, const char *fmt, ...);
int ime_vsnprintf(char *s, size_t n, const char *fmt, va_list ap);
int ime_asprintf(char **out, const char *fmt, ...);
int ime_rename(const char *from, const char *to);

#endif
