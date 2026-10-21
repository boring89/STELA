/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Linux personality test: what STELA does not do fails with ENOSYS (fork
 * and execve work since P12: linux/tests/procs.c), and links are refused
 * (EPERM).
 */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>

int main(void)
{
    if (syscall(SYS_io_setup, 1, NULL) != -1 || errno != ENOSYS) {
        printf("nosys: FAIL io_setup\n");
        return 1;
    }
    if (syscall(SYS_mremap, NULL, 4096, 8192, 0) != -1 || errno != ENOSYS) {
        printf("nosys: FAIL mremap\n");
        return 1;
    }
    if (symlink("/tmp/a", "/tmp/b") != -1 || errno != EPERM) {
        printf("nosys: FAIL symlink\n");
        return 1;
    }
    printf("nosys: OK\n");
    return 0;
}
