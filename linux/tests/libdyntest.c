/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * A shared library for the dynamic-linking test (P13 K5, linux/tests/
 * dyn.c): the dynamic linker maps it from Initrd:/linux/lib with mmap
 * (a private copy of the file, MAP_FIXED, PROT_EXEC). Its data and its
 * thread-local variable check the relocations.
 */

static int calls;
__thread int dyntest_tls = 40;

int dyntest_add(int a, int b)
{
    calls++;
    return a + b;
}

int dyntest_calls(void)
{
    return calls + dyntest_tls;
}
