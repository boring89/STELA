/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Linux personality test (P12): the smaller calls. getrandom and
 * /dev/urandom, a pseudo-terminal (posix_openpt, ptsname, both
 * directions), mmap of a file, /proc (self/exe, maps, fd, cpuinfo,
 * meminfo, stat), pread/pwrite, ftruncate, record locks, umask, chmod,
 * rlimits, sysinfo, times, FIONREAD, dup3, fchdir. Prints "misc: OK".
 */
#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/random.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/sysinfo.h>
#include <sys/times.h>
#include <unistd.h>

static int fail(const char *what)
{
    printf("misc: FAIL %s (errno %d)\n", what, errno);
    return 1;
}

static int contains(const char *path, const char *text)
{
    static char buf[16384];
    int fd = open(path, O_RDONLY);
    if (fd < 0)
        return 0;
    ssize_t n = read(fd, buf, sizeof buf - 1);
    close(fd);
    if (n <= 0)
        return 0;
    buf[n] = 0;
    return strstr(buf, text) != NULL;
}

int main(void)
{
    /* Random bytes. */
    unsigned char a[32], b[32];
    if (getrandom(a, sizeof a, 0) != sizeof a || getrandom(b, sizeof b, 0) != sizeof b)
        return fail("getrandom");
    if (!memcmp(a, b, sizeof a))
        return fail("getrandom repeats");
    int fd = open("/dev/urandom", O_RDONLY);
    if (fd < 0 || read(fd, a, sizeof a) != sizeof a || !memcmp(a, b, sizeof a))
        return fail("/dev/urandom");
    close(fd);

    /* A pseudo-terminal. */
    int master = posix_openpt(O_RDWR | O_NOCTTY);
    if (master < 0 || grantpt(master) || unlockpt(master))
        return fail("posix_openpt");
    char *name = ptsname(master);
    if (!name || strncmp(name, "/dev/pts/", 9))
        return fail("ptsname");
    int slave = open(name, O_RDWR);
    if (slave < 0 || !isatty(slave))
        return fail("open slave");
    write(master, "typed\n", 6);
    char line[64] = {0};
    ssize_t n = read(slave, line, sizeof line - 1);
    if (n != 6 || strcmp(line, "typed\n"))
        return fail("master to slave");
    int avail = 0;
    write(slave, "shown", 5);
    if (ioctl(master, FIONREAD, &avail) || avail <= 0)
        return fail("FIONREAD on the master");
    /* The echo of "typed" comes first, then the output. */
    char out[128] = {0};
    size_t got = 0;
    while (got < sizeof out - 1 && !strstr(out, "shown")) {
        n = read(master, out + got, sizeof out - 1 - got);
        if (n <= 0)
            return fail("read master");
        got += n;
    }
    close(slave);
    close(master);
    printf("misc: pty %s works\n", name);

    /* mmap of a file: a private copy. */
    fd = open("/tmp/misc.txt", O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0 || write(fd, "mapped file contents", 20) != 20)
        return fail("create file");
    char *m = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, 0);
    if (m == MAP_FAILED || memcmp(m, "mapped file contents", 20) || m[20] != 0)
        return fail("mmap file");
    m[0] = 'M';
    char check[8] = {0};
    if (pread(fd, check, 6, 0) != 6 || strcmp(check, "mapped"))
        return fail("private mapping wrote the file");
    munmap(m, 4096);
    char *r = mmap(NULL, 20, PROT_READ, MAP_SHARED, fd, 0);
    if (r == MAP_FAILED || r[7] != 'f')
        return fail("read-only shared mapping");
    munmap(r, 20);

    /* pread, pwrite, ftruncate, locks. */
    if (pwrite(fd, "MAPPED", 6, 0) != 6 || pread(fd, check, 6, 0) != 6 || strcmp(check, "MAPPED"))
        return fail("pwrite");
    if (lseek(fd, 0, SEEK_CUR) != 20)
        return fail("pread moved the offset");
    if (ftruncate(fd, 6))
        return fail("ftruncate");
    struct stat st;
    if (fstat(fd, &st) || st.st_size != 6)
        return fail("size after ftruncate");
    struct flock lk = {.l_type = F_WRLCK, .l_whence = SEEK_SET};
    if (fcntl(fd, F_SETLK, &lk) || fcntl(fd, F_GETLK, &lk) || lk.l_type != F_UNLCK)
        return fail("locks");
    if (fsync(fd) || fdatasync(fd))
        return fail("fsync");
    int copy = dup3(fd, 20, O_CLOEXEC);
    if (copy != 20 || !(fcntl(20, F_GETFD) & FD_CLOEXEC))
        return fail("dup3");
    close(copy);
    close(fd);
    if (chmod("/tmp/misc.txt", 0600) || chmod("/tmp/none", 0600) != -1 || errno != ENOENT)
        return fail("chmod");
    if (umask(077) != 022 || umask(022) != 077)
        return fail("umask");
    unlink("/tmp/misc.txt");

    /* /proc. */
    char exe[256] = {0};
    if (readlink("/proc/self/exe", exe, sizeof exe - 1) <= 0 || !strstr(exe, "misc"))
        return fail("/proc/self/exe");
    if (!contains("/proc/self/maps", "[stack]") || !contains("/proc/cpuinfo", "processor"))
        return fail("/proc maps or cpuinfo");
    if (!contains("/proc/meminfo", "MemTotal:") || !contains("/proc/self/status", "Name:\tmisc"))
        return fail("/proc meminfo or status");
    DIR *d = opendir("/proc/self/fd");
    int fds = 0;
    struct dirent *e;
    while (d && (e = readdir(d)))
        if (e->d_name[0] != '.')
            fds++;
    if (!d || fds < 3)
        return fail("/proc/self/fd");
    closedir(d);
    char link[64] = {0};
    if (readlink("/proc/self/fd/1", link, sizeof link - 1) <= 0)
        return fail("/proc/self/fd/1");
    printf("misc: exe %s, fd 1 is %s\n", exe, link);

    /* Limits and system information. */
    struct rlimit rl;
    if (getrlimit(RLIMIT_NOFILE, &rl) || rl.rlim_cur != 256)
        return fail("getrlimit");
    struct sysinfo si;
    if (sysinfo(&si) || si.totalram == 0 || si.procs == 0)
        return fail("sysinfo");
    struct tms t;
    if (times(&t) == (clock_t)-1)
        return fail("times");
    int dirfd = open("/tmp", O_RDONLY | O_DIRECTORY);
    if (dirfd < 0 || fchdir(dirfd))
        return fail("fchdir");
    char cwd[64];
    if (!getcwd(cwd, sizeof cwd) || strcmp(cwd, "/tmp"))
        return fail("cwd after fchdir");
    printf("misc: OK\n");
    return 0;
}
