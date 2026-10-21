/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Pipes on STELA's Linux personality (P11, docs/linux.md): pipe, read and
 * write in order, poll, dup2, the end of the stream when the writer
 * closes, EPIPE when the reader is gone, pipe2 with O_NONBLOCK (EAGAIN)
 * and fstat's S_IFIFO. Prints "pipe: OK". SIGPIPE is ignored (P12: its
 * default would end the program at the EPIPE write).
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static int fail(const char *what)
{
    printf("pipe: FAIL %s (errno %d)\n", what, errno);
    return 1;
}

int main(void)
{
    int fds[2];
    signal(SIGPIPE, SIG_IGN);
    if (pipe(fds) != 0)
        return fail("pipe");
    struct stat st;
    if (fstat(fds[0], &st) != 0 || !S_ISFIFO(st.st_mode))
        return fail("fstat");
    struct pollfd p = {fds[0], POLLIN, 0};
    if (poll(&p, 1, 0) != 0)
        return fail("empty pipe not readable");
    if (write(fds[1], "hello ", 6) != 6 || write(fds[1], "pipe", 4) != 4)
        return fail("write");
    if (poll(&p, 1, 0) != 1 || !(p.revents & POLLIN))
        return fail("poll readable");
    char buf[64];
    ssize_t n = read(fds[0], buf, sizeof buf);
    if (n != 10 || memcmp(buf, "hello pipe", 10) != 0)
        return fail("read");
    /* dup2 onto 9, write there, close both writers: then the end. */
    if (dup2(fds[1], 9) != 9 || write(9, "!", 1) != 1)
        return fail("dup2");
    close(fds[1]);
    close(9);
    if (read(fds[0], buf, sizeof buf) != 1 || buf[0] != '!')
        return fail("read after dup2");
    if (read(fds[0], buf, sizeof buf) != 0)
        return fail("end of the stream");
    close(fds[0]);
    /* A writer without a reader. */
    if (pipe(fds) != 0)
        return fail("pipe again");
    close(fds[0]);
    if (write(fds[1], "x", 1) != -1 || errno != EPIPE)
        return fail("EPIPE");
    close(fds[1]);
    /* Not waiting. */
    if (pipe2(fds, O_NONBLOCK | O_CLOEXEC) != 0)
        return fail("pipe2");
    if (read(fds[0], buf, 1) != -1 || errno != EAGAIN)
        return fail("EAGAIN");
    if (fcntl(fds[0], F_GETFD) != FD_CLOEXEC)
        return fail("O_CLOEXEC");
    printf("pipe: OK\n");
    return 0;
}
