/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Linux personality test (P12): signals. Handlers through raise and
 * kill, the mask (blocked signals stay pending), SIGALRM from alarm and
 * setitimer, sigsuspend, SIGCHLD and wait, SIGPIPE's default action,
 * EINTR without SA_RESTART and the restart with it, a SIGSEGV handler and
 * the default for SIGSEGV, ignored signals; at the end it waits for
 * Ctrl-C (SIGINT from the terminal). Prints "signals: OK".
 */
#define _GNU_SOURCE
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static volatile sig_atomic_t usr1, alrm, chld, intr, info_ok;
/* An address nothing is mapped at (hidden from the compiler's checks). */
static volatile int *volatile bad = (int *)16;

static int fail(const char *what)
{
    printf("signals: FAIL %s (errno %d)\n", what, errno);
    return 1;
}

static void on_usr1(int sig, siginfo_t *info, void *uc)
{
    (void)uc;
    usr1++;
    if (sig == SIGUSR1 && info->si_signo == SIGUSR1 && info->si_pid == getpid())
        info_ok = 1;
}

static void on_alrm(int sig)
{
    (void)sig;
    alrm++;
}

static void on_chld(int sig)
{
    (void)sig;
    chld++;
}

static void on_int(int sig)
{
    (void)sig;
    intr++;
}

static void on_segv(int sig, siginfo_t *info, void *uc)
{
    (void)uc;
    /* The fault address was 16; leave without returning to it. */
    _exit(sig == SIGSEGV && info->si_addr == (void *)16 ? 7 : 8);
}

static void handle(int sig, void (*f)(int), int flags)
{
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = f;
    sa.sa_flags = flags;
    sigemptyset(&sa.sa_mask);
    sigaction(sig, &sa, NULL);
}

static int child_status(pid_t pid)
{
    int status;
    if (waitpid(pid, &status, 0) != pid)
        return -1;
    return status;
}

int main(void)
{
    /* A siginfo handler, through raise (tkill) and kill. */
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_sigaction = on_usr1;
    sa.sa_flags = SA_SIGINFO;
    sigaction(SIGUSR1, &sa, NULL);
    raise(SIGUSR1);
    kill(getpid(), SIGUSR1);
    if (usr1 != 2 || !info_ok)
        return fail("SIGUSR1 handler");

    /* Blocked: pending until unblocked. */
    sigset_t set, old;
    sigemptyset(&set);
    sigaddset(&set, SIGUSR1);
    sigprocmask(SIG_BLOCK, &set, &old);
    raise(SIGUSR1);
    sigset_t pending;
    sigpending(&pending);
    if (usr1 != 2 || !sigismember(&pending, SIGUSR1))
        return fail("blocked signal");
    sigprocmask(SIG_SETMASK, &old, NULL);
    if (usr1 != 3)
        return fail("unblocked signal");

    /* Ignored. */
    signal(SIGUSR2, SIG_IGN);
    raise(SIGUSR2);

    /* SIGALRM: alarm and pause; setitimer and sigsuspend. */
    handle(SIGALRM, on_alrm, 0);
    alarm(1);
    pause();
    if (alrm != 1)
        return fail("alarm");
    struct itimerval it = {{0, 0}, {0, 50 * 1000}};
    sigprocmask(SIG_BLOCK, (sigemptyset(&set), sigaddset(&set, SIGALRM), &set), &old);
    setitimer(ITIMER_REAL, &it, NULL);
    sigset_t wait_mask;
    sigemptyset(&wait_mask);
    sigsuspend(&wait_mask);
    sigprocmask(SIG_SETMASK, &old, NULL);
    if (alrm != 2)
        return fail("setitimer");
    printf("signals: handlers, mask and timers work\n");

    /* SIGCHLD and the child's status. */
    handle(SIGCHLD, on_chld, SA_RESTART);
    pid_t pid = fork();
    if (pid == 0)
        _exit(4);
    int status = child_status(pid);
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 4 || chld != 1)
        return fail("SIGCHLD");

    /* SIGPIPE ends a writer whose reader is gone. */
    int fds[2];
    pipe(fds);
    pid = fork();
    if (pid == 0) {
        close(fds[0]);
        write(fds[1], "x", 1);
        _exit(0);
    }
    close(fds[0]);
    close(fds[1]);
    status = child_status(pid);
    if (!WIFSIGNALED(status) || WTERMSIG(status) != SIGPIPE)
        return fail("SIGPIPE");

    /* EINTR: a read interrupted by SIGALRM without SA_RESTART. */
    pipe(fds);
    handle(SIGALRM, on_alrm, 0);
    struct itimerval shot = {{0, 0}, {0, 30 * 1000}};
    setitimer(ITIMER_REAL, &shot, NULL);
    char c;
    if (read(fds[0], &c, 1) != -1 || errno != EINTR || alrm != 3)
        return fail("EINTR");
    /* With SA_RESTART the read goes on until the child writes. */
    handle(SIGALRM, on_alrm, SA_RESTART);
    pid = fork();
    if (pid == 0) {
        struct timespec t = {0, 150 * 1000 * 1000};
        nanosleep(&t, NULL);
        write(fds[1], "r", 1);
        _exit(0);
    }
    setitimer(ITIMER_REAL, &shot, NULL);
    if (read(fds[0], &c, 1) != 1 || c != 'r' || alrm != 4)
        return fail("SA_RESTART");
    child_status(pid);
    close(fds[0]);
    close(fds[1]);

    /* A SIGSEGV handler; and the default action. */
    pid = fork();
    if (pid == 0) {
        struct sigaction s;
        memset(&s, 0, sizeof s);
        s.sa_sigaction = on_segv;
        s.sa_flags = SA_SIGINFO;
        sigaction(SIGSEGV, &s, NULL);
        *bad = 1;
        _exit(9);
    }
    status = child_status(pid);
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 7)
        return fail("SIGSEGV handler");
    pid = fork();
    if (pid == 0) {
        *bad = 1;
        _exit(9);
    }
    status = child_status(pid);
    if (!WIFSIGNALED(status) || WTERMSIG(status) != SIGSEGV)
        return fail("SIGSEGV default");
    /* SIGTERM from the parent. */
    pid = fork();
    if (pid == 0) {
        for (;;)
            pause();
    }
    kill(pid, SIGTERM);
    status = child_status(pid);
    if (!WIFSIGNALED(status) || WTERMSIG(status) != SIGTERM)
        return fail("SIGTERM");
    printf("signals: children, pipes, faults and restarts work\n");

    /* Ctrl-C on the terminal. */
    handle(SIGINT, on_int, 0);
    printf("signals: press Ctrl-C\n");
    fflush(stdout);
    while (!intr)
        pause();
    printf("signals: got SIGINT\n");
    printf("signals: OK\n");
    return 0;
}
