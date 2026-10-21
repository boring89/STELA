/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Linux personality test: what STELA does not do fails with ENOSYS
 * (pipe works since P11: linux/tests/pipe.c).
 */
#include <errno.h>
#include <stdio.h>
#include <unistd.h>

int main(void)
{
    char *argv[] = {"x", NULL};

    if (fork() != -1 || errno != ENOSYS) {
        printf("nosys: FAIL fork\n");
        return 1;
    }
    if (execv("/Initrd/linux/hello", argv) != -1 || errno != ENOSYS) {
        printf("nosys: FAIL execve\n");
        return 1;
    }
    printf("nosys: OK\n");
    return 0;
}
