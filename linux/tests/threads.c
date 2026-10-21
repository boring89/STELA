/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Linux personality test (P12): POSIX threads on clone + futex.
 * Mutex and condition variable, ten threads adding up, join values,
 * detached threads, thread-local variables, thread ids, sched_yield,
 * a futex wait that times out, exit() of a thread vs exit_group.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <pthread.h>
#include <sched.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

/* linux/futex.h (no kernel headers with musl alone). */
#define FUTEX_WAIT_PRIVATE 128
#define FUTEX_WAKE_PRIVATE 129

#define THREADS 10
#define ROUNDS 2000

static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t cond = PTHREAD_COND_INITIALIZER;
static long total;
static int ready;
static __thread int mine = 1;
static volatile int detached_done;

static int fail(const char *what)
{
    printf("threads: FAIL %s\n", what);
    return 1;
}

static void *adder(void *arg)
{
    long n = (long)arg;
    for (int i = 0; i < ROUNDS; i++) {
        pthread_mutex_lock(&lock);
        total += n;
        pthread_mutex_unlock(&lock);
        if (i % 500 == 0)
            sched_yield();
    }
    mine = (int)n * 100;
    return (void *)(n * 3);
}

static void *waiter(void *arg)
{
    (void)arg;
    pthread_mutex_lock(&lock);
    while (!ready)
        pthread_cond_wait(&cond, &lock);
    pthread_mutex_unlock(&lock);
    return (void *)(long)syscall(SYS_gettid);
}

static void *detached(void *arg)
{
    (void)arg;
    pthread_mutex_lock(&lock);
    detached_done = 1;
    pthread_cond_broadcast(&cond);
    pthread_mutex_unlock(&lock);
    return NULL;
}

int main(void)
{
    pthread_t t[THREADS];
    long expect = 0;

    for (long i = 0; i < THREADS; i++) {
        if (pthread_create(&t[i], NULL, adder, (void *)(i + 1)))
            return fail("create");
        expect += (i + 1) * ROUNDS;
    }
    for (long i = 0; i < THREADS; i++) {
        void *ret;
        if (pthread_join(t[i], &ret) || (long)ret != (i + 1) * 3)
            return fail("join value");
    }
    if (total != expect)
        return fail("sum");
    if (mine != 1)
        return fail("thread-local");
    printf("threads: %d threads added up to %ld\n", THREADS, total);

    /* A condition variable: the waiter sleeps until signalled. */
    pthread_t w;
    if (pthread_create(&w, NULL, waiter, NULL))
        return fail("create waiter");
    struct timespec pause = {0, 20 * 1000 * 1000};
    nanosleep(&pause, NULL);
    pthread_mutex_lock(&lock);
    ready = 1;
    pthread_cond_signal(&cond);
    pthread_mutex_unlock(&lock);
    void *tid;
    if (pthread_join(w, &tid))
        return fail("join waiter");
    long me = syscall(SYS_gettid);
    if ((long)tid == me || (long)tid <= 0 || me != getpid())
        return fail("thread ids");

    /* A detached thread. */
    pthread_attr_t attr;
    pthread_attr_init(&attr);
    pthread_attr_setdetachstate(&attr, PTHREAD_CREATE_DETACHED);
    pthread_t d;
    if (pthread_create(&d, &attr, detached, NULL))
        return fail("create detached");
    pthread_mutex_lock(&lock);
    while (!detached_done)
        pthread_cond_wait(&cond, &lock);
    pthread_mutex_unlock(&lock);

    /* FUTEX_WAIT with a timeout, and a value that differs. */
    int word = 5;
    struct timespec tmo = {0, 30 * 1000 * 1000};
    struct timespec t0, t1;
    clock_gettime(CLOCK_MONOTONIC, &t0);
    long r = syscall(SYS_futex, &word, FUTEX_WAIT_PRIVATE, 5, &tmo, NULL, 0);
    clock_gettime(CLOCK_MONOTONIC, &t1);
    long ms = (t1.tv_sec - t0.tv_sec) * 1000 + (t1.tv_nsec - t0.tv_nsec) / 1000000;
    if (r != -1 || errno != ETIMEDOUT || ms < 30)
        return fail("futex timeout");
    if (syscall(SYS_futex, &word, FUTEX_WAIT_PRIVATE, 6, &tmo, NULL, 0) != -1 || errno != EAGAIN)
        return fail("futex value");
    if (syscall(SYS_futex, &word, FUTEX_WAKE_PRIVATE, 1, NULL, NULL, 0) != 0)
        return fail("futex wake nobody");

    cpu_set_t set;
    if (sched_getaffinity(0, sizeof set, &set) || CPU_COUNT(&set) != 1)
        return fail("affinity");

    printf("threads: OK\n");
    /* exit_group: ends with this code even though nothing else runs. */
    return 4;
}
