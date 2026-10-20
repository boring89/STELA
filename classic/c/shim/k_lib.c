/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#include "shim/k_lib.h"

void *memcpy(void *dest, const void *src, size_t size);
void *memset(void *dest, int value, size_t size);
int memcmp(const void *a, const void *b, size_t size);


void *k_memcpy(void *dest, const void *src, uint32_t size)
{
    return memcpy(dest, src, size);
}


void *k_memset(void *dest, uint8_t value, uint32_t size)
{
    return memset(dest, value, size);
}


int k_memcmp(const void *a, const void *b, uint32_t size)
{
    return memcmp(a, b, size);
}


size_t k_strlen(const char *str)
{
    size_t n = 0;
    while (str[n] != '\0')
    {
        n++;
    }
    return n;
}


int k_strcmp(const char *a, const char *b)
{
    while (*a != '\0' && *a == *b)
    {
        a++;
        b++;
    }
    return (unsigned char)*a - (unsigned char)*b;
}
