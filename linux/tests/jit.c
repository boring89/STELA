/* Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * jit: generated code (P14 K12, docs/linux.md §19). Writes machine code
 * into anonymous memory, makes it executable and calls it. Without the
 * JIT capability (run, not run --jit) mprotect(PROT_EXEC) and an
 * executable anonymous mmap fail with EACCES; with it they work, but
 * never writable and executable at once. */
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>

int main(void) {
    /* mov eax, 42; add eax, edi; ret */
    static const unsigned char code[] = {0xB8, 0x2A, 0x00, 0x00, 0x00, 0x01, 0xF8, 0xC3};
    unsigned char *p = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED) {
        printf("jit: mmap failed: %s\n", strerror(errno));
        return 1;
    }
    memcpy(p, code, sizeof code);
    void *wx = mmap(NULL, 4096, PROT_READ | PROT_WRITE | PROT_EXEC, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (wx != MAP_FAILED) {
        printf("jit: writable and executable memory was allowed\n");
        return 1;
    }
    int wx_errno = errno;
    if (mprotect(p, 4096, PROT_READ | PROT_EXEC) != 0) {
        printf("jit: no JIT capability (%s)\n", errno == EACCES ? "EACCES" : strerror(errno));
        return 2;
    }
    int (*f)(int) = (int (*)(int))(void *)p;
    int r = f(8);
    printf("jit: generated code returned %d (W+X refused: %s)\n", r,
           wx_errno == EACCES ? "EACCES" : strerror(wx_errno));
    /* Back to writable: no longer executable; a new function. */
    if (mprotect(p, 4096, PROT_READ | PROT_WRITE) != 0 || (p[1] = 7, mprotect(p, 4096, PROT_READ | PROT_EXEC)) != 0) {
        printf("jit: second mprotect failed: %s\n", strerror(errno));
        return 1;
    }
    printf("jit: rewritten code returned %d\n", f(8));
    void *x = mmap(NULL, 4096, PROT_READ | PROT_EXEC, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    printf("jit: executable anonymous mmap %s\n", x == MAP_FAILED ? "refused" : "allowed");
    return 0;
}
