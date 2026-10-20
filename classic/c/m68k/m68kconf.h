/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#ifndef EMU_M68K_M68KCONF_H
#define EMU_M68K_M68KCONF_H

/*
 * Musashi configuration for 68k_HAL. Selected with -DMUSASHI_CNF (see
 * Makefile). Every option in the stock third_party/musashi/m68kconf.h is
 * wrapped in #ifndef, so the overrides below are defined first and the
 * stock file supplies the remaining defaults.
 *
 * Option values: 0 = off, 1 = on, 2 = specify handler (link to the macro).
 */

/* 68040 support is not needed (target is 68030) and pulls in more FPU code. */
#define M68K_EMULATE_040            0

/* 68030 on-chip PMMU: the Mac ROM programs TC/CRP/TT registers (emu/mac). */
#define M68K_EMULATE_PMMU           1

/* Raise address errors on odd word/long accesses (68000 behavior). */
#define M68K_EMULATE_ADDRESS_ERROR  1

/*
 * Called before every instruction. Used to intercept A-line ($Axxx) opcodes,
 * which Musashi would otherwise turn into a vector 10 exception.
 */
#define M68K_INSTRUCTION_HOOK       2
#define M68K_INSTRUCTION_CALLBACK(pc) m68k_vm_instr_hook(pc)

#ifdef __cplusplus
extern "C" {
#endif
void m68k_vm_instr_hook(unsigned int pc);
#ifdef __cplusplus
}
#endif

#include "../../musashi/m68kconf.h"

#endif /* EMU_M68K_M68KCONF_H */
