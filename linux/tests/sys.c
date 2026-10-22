/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Linux personality test (P13 K9): the smaller calls - uname's release,
 * prctl names, priorities, clock_getres, statfs/fstatfs, statx, sendfile,
 * copy_file_range, memfd_create, flock, sched_setaffinity, socket
 * options and the network-interface ioctls, executable file modes.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <net/if.h>
#include <sched.h>
#include <stdio.h>
#include <string.h>
#include <sys/file.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <sys/sendfile.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/statfs.h>
#include <sys/utsname.h>
#include <time.h>
#include <unistd.h>

#define CHECK(c, what) do { if (!(c)) { printf("sys: FAILED: %s (errno %d)\n", what, errno); return 1; } } while (0)

int main(void)
{
    struct utsname u;
    CHECK(uname(&u) == 0 && strstr(u.release, "-stela"), "uname release");
    int a, b, c;
    CHECK(sscanf(u.release, "%d.%d.%d", &a, &b, &c) == 3, "release numbers");
    printf("sys: release %s\n", u.release);

    char name[16] = {0};
    CHECK(prctl(PR_SET_NAME, "worker", 0, 0, 0) == 0, "PR_SET_NAME");
    CHECK(prctl(PR_GET_NAME, name, 0, 0, 0) == 0 && strcmp(name, "worker") == 0, "PR_GET_NAME");
    CHECK(setpriority(PRIO_PROCESS, 0, 5) == 0, "setpriority");
    errno = 0;
    CHECK(getpriority(PRIO_PROCESS, 0) == 5 && errno == 0, "getpriority");
    struct timespec res;
    CHECK(clock_getres(CLOCK_MONOTONIC, &res) == 0 && res.tv_sec == 0 && res.tv_nsec > 0, "clock_getres");
    printf("sys: name, priority and clock resolution work\n");

    struct statfs fs;
    CHECK(statfs(".", &fs) == 0 && fs.f_bsize == 4096 && fs.f_blocks > 0, "statfs");
    int fd = open("sys-a.txt", O_RDWR | O_CREAT | O_TRUNC, 0644);
    CHECK(fd >= 0 && write(fd, "copied by sendfile\n", 19) == 19, "write");
    CHECK(fstatfs(fd, &fs) == 0 && fs.f_namelen == 255, "fstatfs");
    struct statx sx;
    CHECK(statx(AT_FDCWD, "sys-a.txt", 0, STATX_BASIC_STATS, &sx) == 0 && sx.stx_size == 19
          && S_ISREG(sx.stx_mode), "statx");
    struct stat st;
    CHECK(stat("sys-a.txt", &st) == 0 && (st.st_mode & S_IXUSR), "executable mode");
    CHECK(flock(fd, LOCK_EX) == 0 && flock(fd, LOCK_UN) == 0, "flock");
    printf("sys: statfs, statx and flock work\n");

    int out = open("sys-b.txt", O_RDWR | O_CREAT | O_TRUNC, 0644);
    off_t off = 0;
    CHECK(sendfile(out, fd, &off, 19) == 19 && off == 19, "sendfile");
    lseek(fd, 0, SEEK_SET);
    CHECK(copy_file_range(fd, NULL, out, NULL, 7, 0) == 7, "copy_file_range");
    char buf[32] = {0};
    CHECK(pread(out, buf, 26, 0) == 26 && memcmp(buf, "copied by sendfile\ncopied ", 26) == 0, "copied bytes");
    int m = memfd_create("scratch", MFD_CLOEXEC);
    CHECK(m >= 0 && write(m, "in memory", 9) == 9 && pread(m, buf, 9, 0) == 9, "memfd_create");
    close(fd);
    close(out);
    close(m);
    unlink("sys-a.txt");
    unlink("sys-b.txt");
    printf("sys: sendfile, copy_file_range and memfd work\n");

    cpu_set_t set;
    CPU_ZERO(&set);
    CPU_SET(0, &set);
    CHECK(sched_setaffinity(0, sizeof set, &set) == 0, "sched_setaffinity");
    CPU_ZERO(&set);
    CHECK(sched_setaffinity(0, sizeof set, &set) == -1 && errno == EINVAL, "empty affinity");

    int s = socket(AF_INET, SOCK_DGRAM, 0);
    int one = 1, got = 0;
    socklen_t len = sizeof got;
    CHECK(setsockopt(s, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one) == 0, "setsockopt");
    CHECK(getsockopt(s, SOL_SOCKET, SO_REUSEADDR, &got, &len) == 0 && got == 1, "getsockopt");
    struct ifreq reqs[4];
    struct ifconf ifc = {.ifc_len = sizeof reqs, .ifc_req = reqs};
    CHECK(ioctl(s, SIOCGIFCONF, &ifc) == 0 && ifc.ifc_len >= (int) sizeof reqs[0], "SIOCGIFCONF");
    CHECK(strcmp(reqs[0].ifr_name, "lo") == 0, "lo first");
    struct ifreq r = {0};
    strcpy(r.ifr_name, "lo");
    CHECK(ioctl(s, SIOCGIFFLAGS, &r) == 0 && (r.ifr_flags & IFF_LOOPBACK), "SIOCGIFFLAGS");
    CHECK(ioctl(s, SIOCGIFADDR, &r) == 0, "SIOCGIFADDR");
    printf("sys: %d interface(s), lo is %s\n", ifc.ifc_len / (int) sizeof reqs[0],
           (r.ifr_addr.sa_data[2] & 0xFF) == 127 ? "127.0.0.1" : "?");
    close(s);
    printf("sys: OK\n");
    return 0;
}
