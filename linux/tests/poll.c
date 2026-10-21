/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * poll, ppoll, select and pselect on STELA's Linux personality
 * (docs/linux.md): the terminal is writable at once and readable only
 * after a line is typed, /dev/null is always ready, a closed descriptor
 * is POLLNVAL, timeouts take their time, and /etc/resolv.conf exists.
 * Prints "poll: type a line", waits with poll(-1) for it, then
 * "poll: you typed: ..." and "poll: OK".
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <string.h>
#include <sys/select.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

static int fail(const char *what)
{
    printf("poll: FAIL %s (errno %d)\n", what, errno);
    return 1;
}

static long long now_ms(void)
{
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (long long)t.tv_sec * 1000 + t.tv_nsec / 1000000;
}

int main(void)
{
    struct pollfd p[4];
    p[0].fd = 1;
    p[0].events = POLLOUT;
    if (poll(p, 1, 0) != 1 || !(p[0].revents & POLLOUT))
        return fail("stdout writable");

    p[0].fd = 0;
    p[0].events = POLLIN;
    long long start = now_ms();
    if (poll(p, 1, 50) != 0 || p[0].revents != 0)
        return fail("stdin not readable yet");
    if (now_ms() - start < 50)
        return fail("poll timeout too short");

    int null = open("/dev/null", O_RDONLY);
    if (null < 0)
        return fail("open /dev/null");
    p[0].fd = null;
    p[0].events = POLLIN;
    p[1].fd = -1;
    p[1].events = POLLIN;
    p[2].fd = 99;
    p[2].events = POLLIN;
    if (poll(p, 3, 0) != 2 || !(p[0].revents & POLLIN) || p[1].revents != 0 ||
        p[2].revents != POLLNVAL)
        return fail("null, negative and closed descriptors");

    struct timespec ts = {0, 20 * 1000000};
    p[0].fd = 0;
    p[0].events = POLLIN;
    start = now_ms();
    if (ppoll(p, 1, &ts, NULL) != 0 || now_ms() - start < 20)
        return fail("ppoll timeout");

    fd_set r, w;
    FD_ZERO(&r);
    FD_ZERO(&w);
    FD_SET(0, &r);
    FD_SET(1, &w);
    struct timeval tv = {0, 30000};
    start = now_ms();
    if (select(2, &r, &w, NULL, &tv) != 1 || FD_ISSET(0, &r) || !FD_ISSET(1, &w))
        return fail("select");
    FD_ZERO(&r);
    FD_SET(0, &r);
    tv.tv_sec = 0;
    tv.tv_usec = 30000;
    start = now_ms();
    if (select(1, &r, NULL, NULL, &tv) != 0 || now_ms() - start < 30)
        return fail("select timeout");
    FD_ZERO(&r);
    FD_SET(50, &r);
    if (select(51, &r, NULL, NULL, &tv) != -1 || errno != EBADF)
        return fail("select on a closed descriptor");
    FD_ZERO(&w);
    FD_SET(1, &w);
    ts.tv_nsec = 0;
    if (pselect(2, NULL, &w, NULL, &ts, NULL) != 1)
        return fail("pselect");

    struct stat st;
    if (stat("/etc/resolv.conf", &st) != 0 || !S_ISREG(st.st_mode))
        return fail("stat /etc/resolv.conf");
    int conf = open("/etc/resolv.conf", O_RDONLY);
    char text[128];
    ssize_t n = conf < 0 ? -1 : read(conf, text, sizeof text - 1);
    if (n < 0 || n != st.st_size)
        return fail("read /etc/resolv.conf");
    text[n] = '\0';
    if (n > 0 && strncmp(text, "nameserver ", 11) != 0)
        return fail("resolv.conf contents");
    close(conf);
    if (open("/etc/resolv.conf", O_WRONLY) >= 0 || errno != EROFS)
        return fail("resolv.conf is read-only");

    printf("poll: type a line\n");
    fflush(stdout);
    p[0].fd = 0;
    p[0].events = POLLIN;
    if (poll(p, 1, -1) != 1 || !(p[0].revents & POLLIN))
        return fail("stdin readable");
    char line[128];
    n = read(0, line, sizeof line - 1);
    if (n <= 0)
        return fail("read a line");
    line[n] = '\0';
    printf("poll: you typed: %s", line);
    printf("poll: OK\n");
    return 0;
}
