/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#ifndef EMU_M68K_VM_H
#define EMU_M68K_VM_H

#include <stdint.h>
#include <stdbool.h>

/*
 * Thin wrapper around the Musashi 68k core (third_party/musashi).
 *
 * Musashi keeps all CPU state in global variables, so only one VM can exist
 * at a time. m68k_vm_init() replaces any previous VM.
 *
 * Guest memory is a flat byte array starting at guest address 0. The CPU is
 * big-endian; accesses outside the array read as 0 and writes are dropped.
 */

typedef enum
{
    M68K_VM_CPU_68000,
    M68K_VM_CPU_68030,
} m68k_vm_cpu_t;

typedef enum
{
    M68K_VM_REG_D0 = 0,     /* D0..D7 = 0..7 */
    M68K_VM_REG_A0 = 8,     /* A0..A7 = 8..15 */
    M68K_VM_REG_PC = 16,
    M68K_VM_REG_SR = 17,
    M68K_VM_REG_USP = 18,
    M68K_VM_REG_ISP = 19,
} m68k_vm_reg_t;


/*
 * Optional memory bus. When set, every guest access (by the CPU or by
 * the m68k_vm_read and m68k_vm_write functions) goes through it instead of the flat RAM array, so a
 * machine can map RAM, ROM and devices at its own addresses. 'size' is 1, 2
 * or 4 bytes; the bus must handle unaligned and out-of-range addresses
 * itself. Pass NULL to go back to flat RAM.
 */
typedef struct
{
    uint32_t (*read)(void *ctx, uint32_t addr, int size);
    void (*write)(void *ctx, uint32_t addr, int size, uint32_t value);
    void *ctx;
} m68k_vm_bus_t;

void m68k_vm_set_bus(const m68k_vm_bus_t *bus);

/*
 * Optional hook called before every instruction with its address. It may
 * inspect or change the guest state, including PC (to skip or replace the
 * instruction). Pass NULL to remove it.
 */
typedef void (*m68k_vm_insn_hook_t)(void *ctx, uint32_t pc);
void m68k_vm_set_insn_hook(m68k_vm_insn_hook_t hook, void *ctx);

/*
 * By default A-line opcodes are consumed by the trap table (trap.h). A
 * guest that has its own trap dispatcher (the Mac ROM) wants the CPU to take
 * the line-A exception instead: pass false.
 */
void m68k_vm_set_aline_traps(bool intercept);

/* Reset the CPU, which loads SSP and PC from the vectors at address 0 and 4
 * as the bus presents them. Unlike m68k_vm_reset() it writes nothing. */
int m68k_vm_reset_cpu(void);

/* Raise interrupt priority level 'level' (0 = none, 1..7). */
void m68k_vm_set_irq(int level);

/* Attach guest RAM and select the CPU model. Returns 0, or -1 on bad args. */
int m68k_vm_init(uint8_t *ram, uint32_t size, m68k_vm_cpu_t cpu);

/* Copy data into guest RAM. Returns 0, or -1 if it does not fit. */
int m68k_vm_load(uint32_t addr, const void *data, uint32_t len);

/*
 * Write the reset vectors (initial SSP at 0, initial PC at 4) and reset the
 * CPU, which then starts in supervisor mode at 'entry' with SP = 'sp'.
 */
int m68k_vm_reset(uint32_t entry, uint32_t sp);

/*
 * Run for at most 'cycles' clock cycles, or until the program executes STOP.
 * Returns the number of cycles used.
 */
int m68k_vm_run(int cycles);

/* Ask the running m68k_vm_run() to return after the current instruction. */
void m68k_vm_stop(void);

/* True once the program has executed a STOP instruction since the reset. */
bool m68k_vm_halted(void);

uint32_t m68k_vm_get_reg(m68k_vm_reg_t reg);
void m68k_vm_set_reg(m68k_vm_reg_t reg, uint32_t value);

/* Direct guest memory access (big-endian). Out of range: read 0, write ignored. */
uint32_t m68k_vm_read8(uint32_t addr);
uint32_t m68k_vm_read16(uint32_t addr);
uint32_t m68k_vm_read32(uint32_t addr);
void m68k_vm_write8(uint32_t addr, uint32_t value);
void m68k_vm_write16(uint32_t addr, uint32_t value);
void m68k_vm_write32(uint32_t addr, uint32_t value);

/*
 * Optional callback invoked with the address of every byte written to guest
 * RAM (by the CPU or by m68k_vm_write*). Tests use it to see which memory a
 * program touched. Pass NULL to remove it.
 */
typedef void (*m68k_vm_write_watch_t)(uint32_t addr);
void m68k_vm_set_write_watch(m68k_vm_write_watch_t watch);

/*
 * Called when the emulator cannot continue (see freestanding/libc.c). Not
 * defined here: the program that embeds the VM provides it, and it must not
 * return. Only referenced by the freestanding build.
 */
void m68k_vm_fatal(const char *message) __attribute__((noreturn));

#endif
