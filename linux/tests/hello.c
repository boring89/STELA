/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Linux personality test: printf through stdio, the arguments, uname,
 * thread-local storage (musl sets the FS base with arch_prctl), and the
 * exit code (3).
 */
#include <stdio.h>
#include <string.h>
#include <sys/utsname.h>

static __thread int tls_value = 5;

int main(int argc, char **argv)
{
    struct utsname u;

    printf("hello from linux: %d args\n", argc);
    for (int i = 0; i < argc; i++)
        printf("  argv[%d] = %s\n", i, argv[i]);
    if (uname(&u) != 0) {
        printf("hello: uname failed\n");
        return 1;
    }
    printf("hello: uname %s %s %s\n", u.sysname, u.release, u.machine);
    tls_value += 2;
    printf("hello: thread-local value %d\n", tls_value);
    if (strcmp(u.sysname, "STELA") != 0 || tls_value != 7)
        return 1;
    return 3;
}
