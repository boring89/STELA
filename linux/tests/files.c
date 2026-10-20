/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Linux personality test: files and folders in /tmp (the kernel's memory
 * file system) and in the working directory, the synthetic root, /dev.
 */
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define CHECK(cond, what)                                                   \
    do {                                                                    \
        if (!(cond)) {                                                      \
            printf("files: FAIL %s (errno %d)\n", what, errno);             \
            return 1;                                                       \
        }                                                                   \
    } while (0)

static int has_entry(const char *dir, const char *name)
{
    DIR *d = opendir(dir);
    struct dirent *e;
    int found = 0;

    if (d == NULL)
        return 0;
    while ((e = readdir(d)) != NULL)
        if (strcmp(e->d_name, name) == 0)
            found = 1;
    closedir(d);
    return found;
}

static int file_round_trip(const char *dir)
{
    char path[256], moved[256], buf[64];
    struct stat st;
    int fd;

    snprintf(path, sizeof path, "%s/files-test", dir);
    CHECK(mkdir(path, 0755) == 0, "mkdir");
    CHECK(mkdir(path, 0755) == -1 && errno == EEXIST, "mkdir twice");
    snprintf(path, sizeof path, "%s/files-test/a.txt", dir);
    fd = open(path, O_CREAT | O_RDWR | O_TRUNC, 0644);
    CHECK(fd >= 0, "create");
    CHECK(write(fd, "hello, stela\n", 13) == 13, "write");
    CHECK(lseek(fd, 7, SEEK_SET) == 7, "lseek");
    CHECK(read(fd, buf, sizeof buf) == 6 && memcmp(buf, "stela\n", 6) == 0, "read back");
    CHECK(fstat(fd, &st) == 0 && st.st_size == 13 && S_ISREG(st.st_mode), "fstat");
    CHECK(close(fd) == 0, "close");
    CHECK(close(fd) == -1 && errno == EBADF, "close twice");
    snprintf(moved, sizeof moved, "%s/files-test/b.txt", dir);
    CHECK(rename(path, moved) == 0, "rename");
    CHECK(stat(path, &st) == -1 && errno == ENOENT, "old name gone");
    CHECK(stat(moved, &st) == 0 && st.st_size == 13, "stat new name");
    snprintf(path, sizeof path, "%s/files-test", dir);
    CHECK(has_entry(path, "b.txt"), "getdents");
    CHECK(rmdir(path) == -1 && errno == ENOTEMPTY, "rmdir non-empty");
    CHECK(unlink(moved) == 0, "unlink");
    CHECK(rmdir(path) == 0, "rmdir");
    CHECK(access(path, F_OK) == -1 && errno == ENOENT, "removed");
    return 0;
}

int main(void)
{
    char cwd[256], buf[4];
    int fd;

    CHECK(getcwd(cwd, sizeof cwd) != NULL, "getcwd");
    printf("files: working directory %s\n", cwd);
    if (file_round_trip("/tmp") != 0 || file_round_trip(".") != 0)
        return 1;
    CHECK(has_entry("/", "Initrd") && has_entry("/", "tmp") && has_entry("/", "dev"), "root");
    CHECK(has_entry("/dev", "null"), "/dev");
    fd = open("/Initrd/linux/hello", O_RDONLY);
    CHECK(fd >= 0 && read(fd, buf, 4) == 4 && memcmp(buf, "\177ELF", 4) == 0, "read a program");
    close(fd);
    CHECK(open("/Initrd/x", O_CREAT | O_WRONLY, 0644) == -1 && errno == EROFS, "read-only volume");
    fd = open("/dev/null", O_WRONLY);
    CHECK(fd >= 0 && write(fd, "x", 1) == 1, "/dev/null");
    close(fd);
    CHECK(chdir("/tmp") == 0 && getcwd(cwd, sizeof cwd) && strcmp(cwd, "/tmp") == 0, "chdir");
    fd = open("rel.txt", O_CREAT | O_WRONLY, 0644);
    CHECK(fd >= 0 && dup2(fd, 7) == 7 && write(7, "ab", 2) == 2, "relative path, dup2");
    close(fd);
    close(7);
    CHECK(unlink("/tmp/rel.txt") == 0, "unlink relative");
    printf("files: OK\n");
    return 0;
}
