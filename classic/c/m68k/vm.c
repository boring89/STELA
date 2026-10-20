/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#include "m68k/vm.h"

#include <stddef.h>

#include "m68k/trap.h"
#include "shim/k_lib.h"
#include "m68k.h"

/* STOP #imm opcode (M68000 Programmer's Reference Manual, STOP). */
#define OPCODE_STOP 0x4E72

/* A-line opcodes are $Axxx (M68000 Programmer's Reference Manual, 8.2). */
#define OPCODE_LINE_MASK 0xF000
#define OPCODE_LINE_A 0xA000
#define OPCODE_LINE_F 0xF000

/* Line 1111 emulator exception: vector 11 (M68000 PRM, Table 8-1). */
#define VECTOR_LINE_F 11
#define SR_SUPERVISOR 0x2000
#define SR_TRACE 0x8000

/*
 * F-line opcodes carry a coprocessor id in bits 11-9 (M68030 User's Manual,
 * 6.1): 0 is the on-chip PMMU, which Musashi emulates; 1 is the FPU, which
 * the guest does not have. Any other id is also treated as absent.
 */
#define OPCODE_CPID_MASK 0x0E00

#define MAX_TRAP_CHAIN 65536

static uint8_t *vm_ram;
static uint32_t vm_ram_size;
static bool vm_halted;
static bool vm_core_initialized;
static m68k_vm_write_watch_t vm_write_watch;
static m68k_vm_bus_t vm_bus;
static bool vm_has_bus;
static bool vm_stop_requested;
static bool vm_aline_intercept = true;
static m68k_vm_insn_hook_t vm_insn_hook;
static void *vm_insn_hook_ctx;


uint32_t m68k_vm_read8(uint32_t addr)
{
    if (vm_has_bus)
    {
        return vm_bus.read(vm_bus.ctx, addr, 1);
    }
    if (addr >= vm_ram_size)
    {
        return 0;
    }
    return vm_ram[addr];
}


uint32_t m68k_vm_read16(uint32_t addr)
{
    if (vm_has_bus)
    {
        return vm_bus.read(vm_bus.ctx, addr, 2);
    }
    return (m68k_vm_read8(addr) << 8) | m68k_vm_read8(addr + 1);
}


uint32_t m68k_vm_read32(uint32_t addr)
{
    if (vm_has_bus)
    {
        return vm_bus.read(vm_bus.ctx, addr, 4);
    }
    return (m68k_vm_read16(addr) << 16) | m68k_vm_read16(addr + 2);
}


void m68k_vm_write8(uint32_t addr, uint32_t value)
{
    if (vm_has_bus)
    {
        vm_bus.write(vm_bus.ctx, addr, 1, value);
        return;
    }
    if (addr < vm_ram_size)
    {
        vm_ram[addr] = (uint8_t)value;
        if (vm_write_watch != NULL)
        {
            vm_write_watch(addr);
        }
    }
}


void m68k_vm_write16(uint32_t addr, uint32_t value)
{
    if (vm_has_bus)
    {
        vm_bus.write(vm_bus.ctx, addr, 2, value);
        return;
    }
    m68k_vm_write8(addr, value >> 8);
    m68k_vm_write8(addr + 1, value);
}


void m68k_vm_write32(uint32_t addr, uint32_t value)
{
    if (vm_has_bus)
    {
        vm_bus.write(vm_bus.ctx, addr, 4, value);
        return;
    }
    m68k_vm_write16(addr, value >> 16);
    m68k_vm_write16(addr + 2, value);
}


/* Memory callbacks required by Musashi. */
unsigned int m68k_read_memory_8(unsigned int address)
{
    return m68k_vm_read8(address);
}


unsigned int m68k_read_memory_16(unsigned int address)
{
    return m68k_vm_read16(address);
}


unsigned int m68k_read_memory_32(unsigned int address)
{
    return m68k_vm_read32(address);
}


void m68k_write_memory_8(unsigned int address, unsigned int value)
{
    m68k_vm_write8(address, value);
}


void m68k_write_memory_16(unsigned int address, unsigned int value)
{
    m68k_vm_write16(address, value);
}


void m68k_write_memory_32(unsigned int address, unsigned int value)
{
    m68k_vm_write32(address, value);
}


/*
 * Raise the line 1111 exception by hand, as a 68030 without FPU does: push a
 * format 0 frame (SR, PC of the F-line opcode, format/vector word; M68030 User's
 * Manual, 8.4) on the supervisor stack and jump through the vector table.
 * Musashi cannot be told to take this exception from outside, and it would
 * otherwise enter its FPU code (see docs/m68k.md).
 */
static void raise_line_f(uint32_t pc)
{
    uint32_t sr = m68k_get_reg(NULL, M68K_REG_SR);
    uint32_t sp;

    /* Switching to supervisor mode also switches to the supervisor stack. */
    m68k_set_reg(M68K_REG_SR, (sr | SR_SUPERVISOR) & ~SR_TRACE);
    sp = m68k_get_reg(NULL, M68K_REG_SP) - 8;
    m68k_vm_write16(sp, sr);
    m68k_vm_write32(sp + 2, pc);
    m68k_vm_write16(sp + 6, VECTOR_LINE_F * 4);
    m68k_set_reg(M68K_REG_SP, sp);
    m68k_set_reg(M68K_REG_PC,
                 m68k_vm_read32(m68k_get_reg(NULL, M68K_REG_VBR) +
                                VECTOR_LINE_F * 4));
}


static bool is_line_a(uint16_t opcode)
{
    return vm_aline_intercept && (opcode & OPCODE_LINE_MASK) == OPCODE_LINE_A;
}


static bool is_absent_coprocessor(uint16_t opcode)
{
    return (opcode & OPCODE_LINE_MASK) == OPCODE_LINE_F &&
           (opcode & OPCODE_CPID_MASK) != 0;
}


/*
 * Called by Musashi before every instruction (see m68kconf.h).
 *
 * A-line and F-line opcodes are handled here, in a loop, because Musashi would execute
 * whatever sits at the new PC (e.g. a STOP right after a trap) without
 * calling the hook again. The cap keeps a handler that keeps redirecting PC
 * back to a trap from hanging the host; after it, Musashi takes the normal
 * line-A exception.
 */
void m68k_vm_instr_hook(unsigned int pc)
{
    if (vm_insn_hook != NULL)
    {
        vm_insn_hook(vm_insn_hook_ctx, pc);
        pc = m68k_get_reg(NULL, M68K_REG_PC);
    }

    uint16_t opcode = (uint16_t)m68k_vm_read16(pc);

    for (uint32_t i = 0;
         (is_line_a(opcode) || is_absent_coprocessor(opcode)) &&
         i < MAX_TRAP_CHAIN;
         i++)
    {
        if (is_absent_coprocessor(opcode))
        {
            raise_line_f(pc);
            pc = m68k_get_reg(NULL, M68K_REG_PC);
            opcode = (uint16_t)m68k_vm_read16(pc);
            continue;
        }
        /* Skip the opcode first so a handler can redirect PC. */
        m68k_set_reg(M68K_REG_PC, pc + 2);
        m68k_trap_dispatch(opcode, pc);
        pc = m68k_get_reg(NULL, M68K_REG_PC);
        opcode = (uint16_t)m68k_vm_read16(pc);
    }

    if (opcode == OPCODE_STOP)
    {
        /* Let STOP execute, then leave the run loop. */
        vm_halted = true;
        m68k_end_timeslice();
    }
}


void m68k_vm_set_bus(const m68k_vm_bus_t *bus)
{
    vm_has_bus = bus != NULL;
    if (bus != NULL)
    {
        vm_bus = *bus;
    }
}


void m68k_vm_set_insn_hook(m68k_vm_insn_hook_t hook, void *ctx)
{
    vm_insn_hook = hook;
    vm_insn_hook_ctx = ctx;
}


int m68k_vm_reset_cpu(void)
{
    vm_halted = false;
    m68k_trap_clear_log();
    m68k_pulse_reset();
    m68k_execute(1);
    return 0;
}


void m68k_vm_set_aline_traps(bool intercept)
{
    vm_aline_intercept = intercept;
}


void m68k_vm_set_irq(int level)
{
    m68k_set_irq(level);
}


void m68k_vm_set_write_watch(m68k_vm_write_watch_t watch)
{
    vm_write_watch = watch;
}


int m68k_vm_init(uint8_t *ram, uint32_t size, m68k_vm_cpu_t cpu)
{
    if (ram == NULL || size < 8)
    {
        return -1;
    }

    vm_ram = ram;
    vm_ram_size = size;
    vm_halted = false;
    vm_aline_intercept = true;
    m68k_trap_init();

    if (!vm_core_initialized)
    {
        m68k_init();
        vm_core_initialized = true;
    }

    m68k_set_cpu_type(cpu == M68K_VM_CPU_68030 ? M68K_CPU_TYPE_68030
                                               : M68K_CPU_TYPE_68000);
    return 0;
}


int m68k_vm_load(uint32_t addr, const void *data, uint32_t len)
{
    if (addr > vm_ram_size || len > vm_ram_size - addr)
    {
        return -1;
    }
    k_memcpy(vm_ram + addr, data, len);
    return 0;
}


int m68k_vm_reset(uint32_t entry, uint32_t sp)
{
    if (vm_ram == NULL)
    {
        return -1;
    }

    m68k_vm_write32(0, sp);
    m68k_vm_write32(4, entry);
    vm_halted = false;
    m68k_trap_clear_log();
    m68k_pulse_reset();
    /* Musashi charges the reset cycles to the next m68k_execute() call and
     * runs no instruction in it; get that over with now. */
    m68k_execute(1);
    return 0;
}


int m68k_vm_run(int cycles)
{
    int used = 0;

    vm_stop_requested = false;
    while (!vm_halted && !vm_stop_requested && used < cycles)
    {
        used += m68k_execute(cycles - used);
    }
    return used;
}


void m68k_vm_stop(void)
{
    vm_stop_requested = true;
    m68k_end_timeslice();
}


bool m68k_vm_halted(void)
{
    return vm_halted;
}


static m68k_register_t to_musashi_reg(m68k_vm_reg_t reg)
{
    if (reg <= M68K_VM_REG_D0 + 7)
    {
        return (m68k_register_t)(M68K_REG_D0 + (reg - M68K_VM_REG_D0));
    }
    if (reg <= M68K_VM_REG_A0 + 7)
    {
        return (m68k_register_t)(M68K_REG_A0 + (reg - M68K_VM_REG_A0));
    }
    switch (reg)
    {
    case M68K_VM_REG_PC:
        return M68K_REG_PC;
    case M68K_VM_REG_SR:
        return M68K_REG_SR;
    case M68K_VM_REG_USP:
        return M68K_REG_USP;
    default:
        return M68K_REG_ISP;
    }
}


uint32_t m68k_vm_get_reg(m68k_vm_reg_t reg)
{
    return m68k_get_reg(NULL, to_musashi_reg(reg));
}


void m68k_vm_set_reg(m68k_vm_reg_t reg, uint32_t value)
{
    m68k_set_reg(to_musashi_reg(reg), value);
}
