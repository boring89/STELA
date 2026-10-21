/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Linux personality test: the terminal. Prints its size and a coloured
 * word, reads one line (canonical mode: the kernel echoes and edits it)
 * and prints it back.
 */
#include <fcntl.h>
#include <stdio.h>
#include <sys/ioctl.h>
#include <string.h>
#include <termios.h>
#include <unistd.h>

int main(void)
{
    char line[200];
    struct termios t;
    int null = open("/dev/null", O_RDONLY);

    if (!isatty(0) || !isatty(1) || isatty(null) || tcgetattr(0, &t) != 0 ||
        !(t.c_lflag & ICANON)) {
        printf("tty: FAIL terminal\n");
        return 1;
    }
    struct winsize w;
    if (ioctl(1, TIOCGWINSZ, &w) != 0) {
        printf("tty: FAIL TIOCGWINSZ\n");
        return 1;
    }
    /* The terminal's size (P11: what it set with TtySetSize). */
    printf("tty: %d rows, %d columns\n", w.ws_row, w.ws_col);
    /* A coloured word: the terminal shows it red (ANSI SGR 31). */
    printf("tty: \033[31mred\033[0m and plain\n");
    printf("tty: type a line\n");
    fflush(stdout);
    if (fgets(line, sizeof line, stdin) == NULL) {
        printf("tty: end of file\n");
        return 1;
    }
    line[strcspn(line, "\n")] = 0;
    printf("tty: you typed: %s\n", line);
    return 0;
}
