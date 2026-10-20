/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

/*
 * The few libc functions Musashi references, for the kernel build (the host
 * tests use the real libc; this directory is not part of the host build).
 *
 * They are all reached only from Musashi's FPU emulation (m68kfpu.c), which
 * is always compiled in: sin/cos for FSIN/FCOS, sscanf/sprintf for packed
 * decimal constants, and fprintf/exit for its "unimplemented instruction"
 * error. None of that is supported in the kernel: the functions that would
 * need a real implementation stop the machine through m68k_vm_fatal().
 */

#include "m68k/vm.h"

#include <stdarg.h>
#include <stddef.h>

typedef struct emu_file FILE;

FILE *stderr;


double sin(double x)
{
    (void)x;
    m68k_vm_fatal("m68k: FSIN is not supported");
}


double cos(double x)
{
    (void)x;
    m68k_vm_fatal("m68k: FCOS is not supported");
}


void exit(int status)
{
    (void)status;
    m68k_vm_fatal("m68k: emulator error (see m68kfpu.c fatalerror)");
}


/* Diagnostics are dropped: emu/ has no console. */
int fprintf(FILE *stream, const char *format, ...)
{
    (void)stream;
    (void)format;
    return 0;
}


int vfprintf(FILE *stream, const char *format, va_list ap)
{
    (void)stream;
    (void)format;
    (void)ap;
    return 0;
}


int sprintf(char *str, const char *format, ...)
{
    (void)format;
    str[0] = '\0';
    return 0;
}


int sscanf(const char *str, const char *format, ...)
{
    (void)str;
    (void)format;
    return 0;
}
