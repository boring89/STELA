/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

/*
 * The few string and memory helpers the emulator took from 68k_HAL's
 * kernel library (kernel/lib/k_memory.h, k_string.h), here backed by the
 * C library functions: memcpy, memset and memcmp come from Rust's
 * compiler_builtins in the STELA build and from the host's libc in the
 * host tests (classic/README.md).
 */

#ifndef CLASSIC_SHIM_K_LIB_H
#define CLASSIC_SHIM_K_LIB_H

#include <stddef.h>
#include <stdint.h>

void *k_memcpy(void *dest, const void *src, uint32_t size);
void *k_memset(void *dest, uint8_t value, uint32_t size);
int k_memcmp(const void *a, const void *b, uint32_t size);
size_t k_strlen(const char *str);
int k_strcmp(const char *a, const char *b);

#endif
