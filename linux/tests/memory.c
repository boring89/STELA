/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Linux personality test: malloc (musl's mallocng: brk and mmap), the brk
 * system call (musl's sbrk only answers sbrk(0)), anonymous
 * mmap/munmap/mprotect, and that executable mappings are refused.
 */
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <unistd.h>

#define CHECK(cond, what)                                                   \
    do {                                                                    \
        if (!(cond)) {                                                      \
            printf("memory: FAIL %s (errno %d)\n", what, errno);            \
            return 1;                                                       \
        }                                                                   \
    } while (0)

int main(void)
{
    enum { BIG = 8 << 20, SMALL = 2000 };
    static void *small[SMALL];
    unsigned char *big = malloc(BIG);
    char *brk_start, *p;

    CHECK(big != NULL, "malloc 8 MiB");
    memset(big, 0xA5, BIG);
    for (size_t i = 0; i < BIG; i += 4096)
        CHECK(big[i] == 0xA5, "big block contents");
    free(big);
    for (int i = 0; i < SMALL; i++) {
        small[i] = malloc(16 + i % 300);
        CHECK(small[i] != NULL, "small malloc");
        memset(small[i], i & 0xFF, 16);
    }
    for (int i = 0; i < SMALL; i++) {
        CHECK(((unsigned char *)small[i])[15] == (i & 0xFF), "small contents");
        free(small[i]);
    }
    brk_start = (char *)syscall(SYS_brk, 0);
    CHECK(syscall(SYS_brk, brk_start + (64 << 10)) == (long)(brk_start + (64 << 10)), "brk grow");
    memset(brk_start, 1, 64 << 10);
    CHECK(syscall(SYS_brk, brk_start) == (long)brk_start, "brk shrink");
    CHECK(syscall(SYS_brk, brk_start + (1L << 40)) == (long)brk_start, "brk too far");
    p = mmap(NULL, 1 << 20, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    CHECK(p != MAP_FAILED && ((uintptr_t)p & 4095) == 0, "mmap");
    CHECK(p[12345] == 0, "mmap is zeroed");
    memset(p, 7, 1 << 20);
    CHECK(mprotect(p, 4096, PROT_READ) == 0, "mprotect");
    CHECK(munmap(p, 1 << 20) == 0, "munmap");
    CHECK(mmap(NULL, 4096, PROT_READ | PROT_EXEC, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0) == MAP_FAILED,
          "executable mapping refused");
    printf("memory: OK\n");
    return 0;
}
