/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Linux personality test: clocks and sleeping.
 */
#include <stdio.h>
#include <sys/time.h>
#include <time.h>

int main(void)
{
    struct timespec a, b, nap = {0, 20 * 1000 * 1000};
    struct timeval tv;
    long long elapsed;
    time_t now = time(NULL);
    struct tm *tm = gmtime(&now);

    if (clock_gettime(CLOCK_MONOTONIC, &a) != 0 || nanosleep(&nap, NULL) != 0 ||
        clock_gettime(CLOCK_MONOTONIC, &b) != 0) {
        printf("time: FAIL clocks\n");
        return 1;
    }
    elapsed = (b.tv_sec - a.tv_sec) * 1000000000LL + (b.tv_nsec - a.tv_nsec);
    printf("time: nanosleep(20 ms) took %lld us\n", elapsed / 1000);
    if (elapsed < 20 * 1000 * 1000) {
        printf("time: FAIL monotonic clock\n");
        return 1;
    }
    if (gettimeofday(&tv, NULL) != 0 || tm == NULL || tm->tm_year + 1900 < 2024 ||
        tv.tv_sec + 5 < now) {
        printf("time: FAIL wall clock\n");
        return 1;
    }
    printf("time: %04d-%02d-%02d %02d:%02d UTC\n", tm->tm_year + 1900, tm->tm_mon + 1,
           tm->tm_mday, tm->tm_hour, tm->tm_min);
    printf("time: OK\n");
    return 0;
}
