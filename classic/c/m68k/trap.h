/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#ifndef EMU_M68K_TRAP_H
#define EMU_M68K_TRAP_H

#include <stdint.h>

/*
 * A-line ($Axxx) trap dispatch.
 *
 * Classic Mac OS software calls the Toolbox and OS through A-line opcodes
 * (e.g. $A9F4). The VM intercepts them before the CPU raises its line-A
 * exception: the opcode is recorded, the handler registered for that trap
 * number runs (if any), and execution continues after the opcode.
 *
 * The trap number is the low 12 bits of the opcode.
 */

#define M68K_TRAP_COUNT 0x1000
#define M68K_TRAP_LOG_SIZE 32

typedef struct
{
    uint16_t opcode;    /* full opcode, e.g. 0xA9F4 */
    uint32_t pc;        /* address of the opcode */
} m68k_trap_record_t;

/* 'pc' is the address of the A-line opcode; the VM has already advanced the
 * guest PC past it, so a handler may change PC to redirect execution. */
typedef void (*m68k_trap_handler_t)(uint16_t opcode, uint32_t pc);


/* Remove all handlers and clear the log. */
void m68k_trap_init(void);

/* Clear the log and the counter, keep the handlers. */
void m68k_trap_clear_log(void);

/* Register (or with NULL, remove) a handler. Returns 0, or -1 if
 * 'number' >= M68K_TRAP_COUNT. */
int m68k_trap_set_handler(uint16_t number, m68k_trap_handler_t handler);

/* Record the trap and run its handler. Called by the VM. */
void m68k_trap_dispatch(uint16_t opcode, uint32_t pc);

/* Total traps dispatched since the last clear (may exceed the log size). */
uint32_t m68k_trap_total(void);

/* Number of records currently held in the log (<= M68K_TRAP_LOG_SIZE). */
uint32_t m68k_trap_log_length(void);

/* Read a log entry; index 0 is the oldest one still held. Returns 0, or -1
 * if the index is out of range. */
int m68k_trap_log_get(uint32_t index, m68k_trap_record_t *out);

#endif
