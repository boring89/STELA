/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

/*
 * libchewing's config.h for STELA (the values its configure script finds
 * on a POSIX system with the hash user-phrase back end, without sqlite3).
 * Used for the IME (freestanding, with the stand-in C library of
 * c/include) and for the host tool init_database that builds the
 * dictionary (Makefile, `make chewing`).
 */

#ifndef STELA_CHEWING_CONFIG_H
#define STELA_CHEWING_CONFIG_H

#define HAVE_ASPRINTF 1
#define HAVE_GETPAGESIZE 1
#define HAVE_INTTYPES_H 1
#define HAVE_MALLOC 1
#define HAVE_MMAP 1
#define HAVE_STDINT_H 1
#define HAVE_STDLIB_H 1
#define HAVE_STRING_H 1
#define HAVE_STRTOK_R 1
#define HAVE_SYS_STAT_H 1
#define HAVE_SYS_TYPES_H 1
#define HAVE_UNISTD_H 1
#define LIBCHEWING_BINARY_VERSION "1.0.0"
#define LIBCHEWING_VERSION "0.5.1"
#define PACKAGE "libchewing"
#define PACKAGE_VERSION "0.5.1"
#define STDC_HEADERS 1
#define UNDER_POSIX 1
#define VERSION "0.5.1"

#endif
