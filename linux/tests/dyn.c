/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Linux personality test (P13 K5): a dynamically linked, position-
 * independent program. The kernel loads it at the PIE base and its
 * interpreter, /lib/ld-musl-x86_64.so.1 (musl's libc.so, Initrd:/linux/
 * lib), which maps libdyntest.so. dlopen and dlsym find the library's
 * function again; malloc and printf come from the shared libc.
 */
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int dyntest_add(int a, int b);
int dyntest_calls(void);

int main(int argc, char **argv)
{
    (void) argc;
    printf("dyn: hello from a dynamic program (%s)\n", argv[0]);
    printf("dyn: pie %s\n", (unsigned long) &main >= 0x100000000000UL ? "yes" : "no");
    printf("dyn: 2 + 3 = %d\n", dyntest_add(2, 3));
    void *h = dlopen("libdyntest.so", RTLD_NOW);
    if (!h) {
        printf("dyn: dlopen failed: %s\n", dlerror());
        return 1;
    }
    int (*add)(int, int) = (int (*)(int, int)) dlsym(h, "dyntest_add");
    if (!add) {
        printf("dyn: dlsym failed\n");
        return 1;
    }
    int sum = add(4, 5);
    printf("dyn: dlsym add(4, 5) = %d, calls %d\n", sum, dyntest_calls());
    char *p = malloc(1 << 20);
    if (!p)
        return 1;
    memset(p, 7, 1 << 20);
    printf("dyn: malloc ok %d\n", p[12345]);
    free(p);
    return 0;
}
