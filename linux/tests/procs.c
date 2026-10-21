/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Linux personality test (P12): processes. fork and wait (exit status,
 * getppid), a pipe between parent and child, execve of another program,
 * a #! script, execve errors, posix_spawn, vfork, WNOHANG, ECHILD,
 * process groups, and many forks in a row. Prints "procs: OK".
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <spawn.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

extern char **environ;

static int fail(const char *what)
{
    printf("procs: FAIL %s (errno %d)\n", what, errno);
    return 1;
}

static int exited_with(pid_t pid, int code)
{
    int status;
    if (waitpid(pid, &status, 0) != pid)
        return 0;
    return WIFEXITED(status) && WEXITSTATUS(status) == code;
}

int main(void)
{
    pid_t me = getpid();

    /* fork, exit status, getppid. */
    pid_t pid = fork();
    if (pid < 0)
        return fail("fork");
    if (pid == 0)
        _exit(getppid() == me ? 5 : 6);
    if (!exited_with(pid, 5))
        return fail("fork exit status");

    /* A pipe between the two; the child's memory is a copy. */
    int fds[2];
    int shared = 1;
    if (pipe(fds))
        return fail("pipe");
    pid = fork();
    if (pid == 0) {
        close(fds[0]);
        shared = 2;
        const char *msg = "hello from the child";
        write(fds[1], msg, strlen(msg));
        _exit(0);
    }
    close(fds[1]);
    char buf[64] = {0};
    ssize_t n = read(fds[0], buf, sizeof buf - 1);
    close(fds[0]);
    if (n <= 0 || strcmp(buf, "hello from the child") || shared != 1)
        return fail("pipe from child");
    if (!exited_with(pid, 0))
        return fail("pipe child");
    printf("procs: child said: %s\n", buf);

    /* execve errors leave the caller as it was. */
    char *argv[] = {"hello", "from-exec", NULL};
    if (execve("/Initrd/linux/no-such", argv, environ) != -1 || errno != ENOENT)
        return fail("execve ENOENT");
    if (execve("/Initrd/linux", argv, environ) != -1 || errno != EACCES)
        return fail("execve EACCES");

    /* execve in a child. */
    pid = fork();
    if (pid == 0) {
        execve("/Initrd/linux/hello", argv, environ);
        _exit(99);
    }
    if (!exited_with(pid, 3))
        return fail("execve hello");

    /* A #! script. */
    int f = open("/tmp/script", O_CREAT | O_WRONLY | O_TRUNC, 0755);
    if (f < 0)
        return fail("create script");
    const char *script = "#!/Initrd/linux/hello one\n";
    write(f, script, strlen(script));
    close(f);
    pid = fork();
    if (pid == 0) {
        char *sargv[] = {"script", "two", NULL};
        execve("/tmp/script", sargv, environ);
        _exit(98);
    }
    if (!exited_with(pid, 3))
        return fail("script");

    /* posix_spawn (musl: clone with CLONE_VM|CLONE_VFORK). */
    char *pargv[] = {"hello", "spawned", NULL};
    if (posix_spawn(&pid, "/Initrd/linux/hello", NULL, NULL, pargv, environ))
        return fail("posix_spawn");
    if (!exited_with(pid, 3))
        return fail("posix_spawn status");
    if (posix_spawn(&pid, "/Initrd/linux/missing", NULL, NULL, pargv, environ) != ENOENT)
        return fail("posix_spawn ENOENT");

    /* vfork. */
    pid = vfork();
    if (pid == 0)
        _exit(9);
    if (!exited_with(pid, 9))
        return fail("vfork");

    /* WNOHANG while the child runs; its own process group. */
    pid = fork();
    if (pid == 0) {
        setpgid(0, 0);
        struct timespec t = {0, 100 * 1000 * 1000};
        nanosleep(&t, NULL);
        _exit(getpgrp() == getpid() ? 7 : 8);
    }
    int status;
    if (waitpid(pid, &status, WNOHANG) != 0)
        return fail("WNOHANG");
    if (!exited_with(pid, 7))
        return fail("process group");
    if (getpgrp() != me)
        return fail("own group");

    /* Many in a row. */
    for (int i = 0; i < 20; i++) {
        pid = fork();
        if (pid == 0)
            _exit(i);
        if (!exited_with(pid, i))
            return fail("fork loop");
    }
    if (wait(&status) != -1 || errno != ECHILD)
        return fail("ECHILD");

    printf("procs: OK\n");
    return 0;
}
