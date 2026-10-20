/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#include "m68k/trap.h"

#include <stddef.h>

static m68k_trap_handler_t handlers[M68K_TRAP_COUNT];
static m68k_trap_record_t log_ring[M68K_TRAP_LOG_SIZE];
static uint32_t total;


void m68k_trap_clear_log(void)
{
    total = 0;
}


void m68k_trap_init(void)
{
    for (uint32_t i = 0; i < M68K_TRAP_COUNT; i++)
    {
        handlers[i] = NULL;
    }
    m68k_trap_clear_log();
}


int m68k_trap_set_handler(uint16_t number, m68k_trap_handler_t handler)
{
    if (number >= M68K_TRAP_COUNT)
    {
        return -1;
    }
    handlers[number] = handler;
    return 0;
}


void m68k_trap_dispatch(uint16_t opcode, uint32_t pc)
{
    m68k_trap_handler_t handler = handlers[opcode & (M68K_TRAP_COUNT - 1)];

    log_ring[total % M68K_TRAP_LOG_SIZE].opcode = opcode;
    log_ring[total % M68K_TRAP_LOG_SIZE].pc = pc;
    total++;

    if (handler != NULL)
    {
        handler(opcode, pc);
    }
}


uint32_t m68k_trap_total(void)
{
    return total;
}


uint32_t m68k_trap_log_length(void)
{
    return total < M68K_TRAP_LOG_SIZE ? total : M68K_TRAP_LOG_SIZE;
}


int m68k_trap_log_get(uint32_t index, m68k_trap_record_t *out)
{
    uint32_t length = m68k_trap_log_length();

    if (index >= length)
    {
        return -1;
    }
    *out = log_ring[(total - length + index) % M68K_TRAP_LOG_SIZE];
    return 0;
}
