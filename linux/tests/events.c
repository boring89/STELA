/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Linux personality test (P13 K6): copy-on-write fork (the child changes
 * a large buffer, the parent's stays), MAP_SHARED of a file (parent and
 * child see each other's writes, munmap writes the file), eventfd (counter
 * and semaphore), timerfd (a periodic timer) and epoll over an eventfd, a
 * pipe and a timerfd.
 */
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/mman.h>
#include <sys/timerfd.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define BIG (4 << 20)

static int fail(const char *what)
{
    printf("events: FAILED: %s (errno %d)\n", what, errno);
    return 1;
}

static int cow(void)
{
    char *buf = malloc(BIG);
    if (!buf)
        return fail("malloc");
    memset(buf, 'p', BIG);
    pid_t pid = fork();
    if (pid < 0)
        return fail("fork");
    if (pid == 0) {
        memset(buf, 'c', BIG);
        _exit(buf[BIG - 1] == 'c' && buf[0] == 'c' ? 0 : 1);
    }
    int status = 0;
    if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status) != 0)
        return fail("cow child");
    for (int i = 0; i < BIG; i += 4096)
        if (buf[i] != 'p')
            return fail("parent's buffer changed");
    free(buf);
    printf("events: copy-on-write fork works\n");
    return 0;
}

static int shared(void)
{
    const char *path = "events-shared.txt";
    int fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0644);
    if (fd < 0)
        return fail("open");
    char page[8192];
    memset(page, '.', sizeof page);
    if (write(fd, page, sizeof page) != (ssize_t) sizeof page)
        return fail("write");
    char *m = mmap(NULL, sizeof page, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (m == MAP_FAILED)
        return fail("mmap shared");
    pid_t pid = fork();
    if (pid == 0) {
        memcpy(m + 4096, "from the child", 14);
        _exit(0);
    }
    int status;
    waitpid(pid, &status, 0);
    if (memcmp(m + 4096, "from the child", 14) != 0)
        return fail("the child's write is not seen");
    memcpy(m, "from the parent", 15);
    if (msync(m, sizeof page, MS_SYNC) != 0)
        return fail("msync");
    if (munmap(m, sizeof page) != 0)
        return fail("munmap");
    char back[16] = {0};
    if (pread(fd, back, 15, 0) != 15 || memcmp(back, "from the parent", 15) != 0)
        return fail("not written back");
    if (pread(fd, back, 14, 4096) != 14 || memcmp(back, "from the child", 14) != 0)
        return fail("the child's part not written back");
    close(fd);
    unlink(path);
    printf("events: MAP_SHARED works\n");
    return 0;
}

static int eventfds(void)
{
    int e = eventfd(3, 0);
    if (e < 0)
        return fail("eventfd");
    uint64_t v = 4;
    if (write(e, &v, 8) != 8)
        return fail("eventfd write");
    if (read(e, &v, 8) != 8 || v != 7)
        return fail("eventfd read");
    int s = eventfd(2, EFD_SEMAPHORE | EFD_NONBLOCK);
    uint64_t a = 0, b = 0, c = 0;
    if (read(s, &a, 8) != 8 || read(s, &b, 8) != 8 || a != 1 || b != 1)
        return fail("semaphore");
    if (read(s, &c, 8) != -1 || errno != EAGAIN)
        return fail("semaphore empty");
    close(e);
    close(s);
    printf("events: eventfd works\n");
    return 0;
}

static int timers(void)
{
    int t = timerfd_create(CLOCK_MONOTONIC, 0);
    if (t < 0)
        return fail("timerfd_create");
    struct itimerspec its = {{0, 20000000}, {0, 20000000}};
    if (timerfd_settime(t, 0, &its, NULL) != 0)
        return fail("timerfd_settime");
    uint64_t n = 0, total = 0;
    while (total < 3) {
        if (read(t, &n, 8) != 8 || n == 0)
            return fail("timerfd read");
        total += n;
    }
    struct itimerspec cur;
    if (timerfd_gettime(t, &cur) != 0 || cur.it_interval.tv_nsec != 20000000)
        return fail("timerfd_gettime");
    close(t);
    printf("events: timerfd works (%llu expirations)\n", (unsigned long long) total);
    return 0;
}

static int epolls(void)
{
    int ep = epoll_create1(EPOLL_CLOEXEC);
    int e = eventfd(0, EFD_NONBLOCK);
    int p[2];
    int t = timerfd_create(CLOCK_MONOTONIC, TFD_NONBLOCK);
    if (ep < 0 || e < 0 || pipe(p) != 0 || t < 0)
        return fail("epoll setup");
    struct epoll_event ev = {.events = EPOLLIN, .data.u64 = 1};
    if (epoll_ctl(ep, EPOLL_CTL_ADD, e, &ev) != 0)
        return fail("epoll_ctl eventfd");
    ev.data.u64 = 2;
    epoll_ctl(ep, EPOLL_CTL_ADD, p[0], &ev);
    ev.data.u64 = 3;
    epoll_ctl(ep, EPOLL_CTL_ADD, t, &ev);
    if (epoll_ctl(ep, EPOLL_CTL_ADD, e, &ev) != -1 || errno != EEXIST)
        return fail("epoll_ctl twice");
    struct epoll_event out[4];
    if (epoll_wait(ep, out, 4, 0) != 0)
        return fail("nothing should be ready");
    uint64_t one = 1;
    write(e, &one, 8);
    write(p[1], "x", 1);
    int n = epoll_wait(ep, out, 4, 1000);
    int seen = 0;
    for (int i = 0; i < n; i++)
        seen |= 1 << out[i].data.u64;
    if (n != 2 || seen != 6)
        return fail("eventfd and pipe ready");
    read(e, &one, 8);
    char c;
    read(p[0], &c, 1);
    struct itimerspec its = {{0, 0}, {0, 30000000}};
    timerfd_settime(t, 0, &its, NULL);
    n = epoll_wait(ep, out, 4, 2000);
    if (n != 1 || out[0].data.u64 != 3)
        return fail("timer ready");
    if (epoll_ctl(ep, EPOLL_CTL_DEL, t, NULL) != 0)
        return fail("epoll_ctl del");
    close(ep);
    printf("events: epoll works\n");
    return 0;
}

int main(void)
{
    if (cow() || shared() || eventfds() || timers() || epolls())
        return 1;
    printf("events: OK\n");
    return 0;
}
