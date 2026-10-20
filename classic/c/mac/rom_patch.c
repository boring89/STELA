/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#include "mac/mac.h"

/*
 * ROM patches for the Mac IIci ROM (checksum 368CADFE).
 *
 * The ROM probes the real chips (VIAs, SCC, SCSI ...) and sizes the RAM
 * itself. Instead of imitating all of that hardware we overwrite a few
 * routines so that they skip the probing and call native code through
 * emulated opcodes. The idea and the choice of places follow Basilisk II's
 * rom_patches.cpp (GPL); offsets were checked against a disassembly of
 * this ROM.
 *
 * All offsets are from the start of the ROM image.
 */

#define ROM_CHECKSUM 0x368CADFE

#define OP_NOP 0x4E71
#define OP_JMP_A5 0x4ED5
#define OP_JMP_ABS_L 0x4EF9

/* Early reset code: "move #$2700,SR; reset; ..." through the first hardware
 * detection. Offset 0xBA is where the ROM carries on with the results. */
#define PATCH_RESET 0x8C
#define RESUME_AFTER_RESET 0x408000BA

/* "jmp GetHardwareInfo" after the resume point: the RESET emulated opcode
 * has already provided its results in the registers. */
#define PATCH_GET_HW_INFO 0xC2

/* "lea ...,A6; jmp InitHW": initialization of the VIAs, SCC and friends. */
#define PATCH_INIT_HW 0xCE
#define INIT_HW_WORDS 4

/* The routine that talks to the clock chip (RTC and parameter RAM) through
 * VIA1 port B. It is entered with a return address in A5, the command in D1
 * and the data in D2, and answers in D2. */
#define PATCH_CLKNOMEM 0xB1E4

/* GetHardwareInfo: "tst.b D2; beq probe". D2 = 0 asks the routine to find the
 * model by poking at the hardware; the emulated opcode makes it ask for the
 * IIci instead, so the table lookup below it answers. */
#define PATCH_HWINFO_TEST 0x2F1E

/* _SwapMMUMode: switches the 68030 PMMU between 24 and 32 bit addressing. We
 * always run in 32-bit mode with translation off, so the trap just reports
 * "32-bit" ("moveq #1,D0; rts"). */
#define PATCH_SWAP_MMU_MODE 0x4BD8
#define OP_MOVEQ_1_D0 0x7001
#define OP_RTS 0x4E75

/* ADB (keyboard and mouse bus) reinitialization waits, with interrupts on,
 * for the VIA shift-register state machine to finish scanning the bus. There
 * is no ADB hardware here, so the wait ("btst #5,(349,A3); bne") is skipped. */
#define PATCH_ADB_WAIT 0xA8E6
#define ADB_WAIT_WORDS 4

/* MMU setup: after loading the root pointer the ROM turns translation on with
 * "pmove (A3),tc" (two places). The 24-bit mapping is never needed, so
 * translation stays off. */
#define PATCH_MMU_ENABLE_1 0x4384
#define PATCH_MMU_ENABLE_2 0x438C

/* InitMMU builds the translation tables unless a flag in its frame says the
 * machine has no use for them: "cmpi.b #1,(-26,A6); bne build". Turn the
 * compare into "cmp.b D0,D0" so the flag always reads as set. */
#define PATCH_INIT_MMU 0x430A
#define OP_CMP_B_D0_D0 0xB000

/* SCSI Manager: "wait for the target to raise BSY" with a timeout of minutes
 * at our speed. Nothing is on the bus, so answer "timeout" (D0 = 2, flags set
 * by tst.w) at once. */
#define PATCH_SCSI_WAIT_BSY 0x7860
#define OP_MOVEQ_2_D0 0x7002
#define OP_TST_W_D0 0x4A40

/* _SCSIDispatch handler: the SCSI Manager. The bus is empty, so the whole
 * manager is replaced by a native one that reports "no target answered". */
#define PATCH_SCSI_DISPATCH 0x7460

/* InitDrivers (ROM offset 0x1100) opens .Sony and .Sound, then starts to load
 * the serial drivers with "subq.w #4,sp; move.l #'SERD',-(sp)". Those 8 bytes
 * become "jsr install; nop": the stub code adds the .Disk driver and performs
 * the two displaced instructions. */
#define PATCH_INSTALL_DRIVERS 0x1166
#define OP_JSR_ABS_L 0x4EB9

/* After InitMMU the ROM takes MemTop and BufPtr from the frame at A4
 * ("movea.l (-20,A4),A6" at 0x10E and the next two instructions). Its own
 * values come from memory probing that does not apply to our flat RAM, so the
 * emulated opcode puts the right ones in the frame and then does the
 * displaced "movea.l (-20,A4),A6" itself. */
#define PATCH_BOOT_GLOBS 0x10E

/* UniversalInfo entries of the IIci (two, see mac.c): the hardware flags at
 * +16 say that an FPU may be present (bit 28). Without it the System would
 * try an FNOP and the guest raises "no coprocessor" exceptions it does not
 * expect. */
#define IICI_ENTRY_1 0x36C0
#define IICI_ENTRY_2 0x3700
#define ENTRY_HWCFG 16
#define HWCFG_FPU_PRESENT 0x10000000u

/* _ADBOp ($A07C): the routine for the trap starts at ROM offset 0xA3DC. It is
 * replaced by a jump to the stub that answers every ADB command with "no
 * device". */
#define PATCH_ADBOP 0xA3DC

/* Slot probe in the Slot Manager's init (ROM $5E8A): for slots $E down to $1
 * it reads the byte at the top of the slot's address space ("move.b (A1),D0")
 * and counts the slots that answer. On a real IIci every one of them is
 * empty: the read gives a bus error and the handler resumes at A2, which
 * skips the counting. Our bus never faults, so every slot counted as a card;
 * the built-in video's declaration ROM (the end of the ROM, mapped at slot
 * $E) then became a second source of the video sResources, and the video
 * primary init, which deletes the unused modes through "slot 0" (the
 * built-in video), left its 15 modes in slot $E. The System made a GDevice for
 * each (16 in all), and the Monitors control panel overran its 16-entry
 * list of screens and hung. The emulated opcode resumes at A2 for every slot. */
#define PATCH_SLOT_PROBE 0x5E8A


static void put16(uint8_t *rom, uint32_t off, uint16_t v)
{
    rom[off] = v >> 8;
    rom[off + 1] = (uint8_t)v;
}


static void put32(uint8_t *rom, uint32_t off, uint32_t v)
{
    put16(rom, off, v >> 16);
    put16(rom, off + 2, (uint16_t)v);
}


int mac_rom_patch(uint8_t *rom, uint32_t size)
{
    uint32_t sum;

    if (rom == 0 || size != 0x80000)
    {
        return -1;
    }
    sum = (uint32_t)rom[0] << 24 | rom[1] << 16 | rom[2] << 8 | rom[3];
    if (sum != ROM_CHECKSUM)
    {
        return -1;
    }

    put16(rom, PATCH_RESET, MAC_EMUL_OP_RESET);
    put16(rom, PATCH_RESET + 2, OP_JMP_ABS_L);
    put32(rom, PATCH_RESET + 4, RESUME_AFTER_RESET);

    put16(rom, PATCH_GET_HW_INFO, OP_NOP);
    put16(rom, PATCH_GET_HW_INFO + 2, OP_NOP);

    for (uint32_t i = 0; i < INIT_HW_WORDS; i++)
    {
        put16(rom, PATCH_INIT_HW + 2 * i, OP_NOP);
    }

    put16(rom, PATCH_HWINFO_TEST, MAC_EMUL_OP_HWINFO);
    put16(rom, PATCH_HWINFO_TEST + 2, OP_NOP);

    for (uint32_t i = 0; i < ADB_WAIT_WORDS; i++)
    {
        put16(rom, PATCH_ADB_WAIT + 2 * i, OP_NOP);
    }

    for (uint32_t i = 0; i < 2; i++)
    {
        uint32_t at = (i == 0 ? IICI_ENTRY_1 : IICI_ENTRY_2) + ENTRY_HWCFG;
        uint32_t flags = (uint32_t)rom[at] << 24 | rom[at + 1] << 16 |
                         rom[at + 2] << 8 | rom[at + 3];

        put32(rom, at, flags & ~HWCFG_FPU_PRESENT);
    }

    put16(rom, PATCH_ADBOP, OP_JMP_ABS_L);
    put32(rom, PATCH_ADBOP + 2, MAC_STUB_BASE + MAC_STUB_ADBOP);

    put16(rom, PATCH_BOOT_GLOBS, MAC_EMUL_OP_PATCH_BOOT_GLOBS);
    put16(rom, PATCH_BOOT_GLOBS + 2, OP_NOP);

    put16(rom, PATCH_INSTALL_DRIVERS, OP_JSR_ABS_L);
    put32(rom, PATCH_INSTALL_DRIVERS + 2, MAC_STUB_BASE);
    put16(rom, PATCH_INSTALL_DRIVERS + 6, OP_NOP);

    put16(rom, PATCH_SCSI_DISPATCH, MAC_EMUL_OP_SCSI_DISPATCH);

    put16(rom, PATCH_SLOT_PROBE, MAC_EMUL_OP_SLOT_PROBE);

    put16(rom, PATCH_SCSI_WAIT_BSY, OP_MOVEQ_2_D0);
    put16(rom, PATCH_SCSI_WAIT_BSY + 2, OP_TST_W_D0);
    put16(rom, PATCH_SCSI_WAIT_BSY + 4, OP_RTS);

    put16(rom, PATCH_INIT_MMU, OP_CMP_B_D0_D0);
    put16(rom, PATCH_INIT_MMU + 2, OP_NOP);
    put16(rom, PATCH_INIT_MMU + 4, OP_NOP);

    put16(rom, PATCH_MMU_ENABLE_1, OP_NOP);
    put16(rom, PATCH_MMU_ENABLE_1 + 2, OP_NOP);
    put16(rom, PATCH_MMU_ENABLE_2, OP_NOP);
    put16(rom, PATCH_MMU_ENABLE_2 + 2, OP_NOP);

    put16(rom, PATCH_SWAP_MMU_MODE, OP_MOVEQ_1_D0);
    put16(rom, PATCH_SWAP_MMU_MODE + 2, OP_RTS);

    put16(rom, PATCH_CLKNOMEM, MAC_EMUL_OP_CLKNOMEM);
    put16(rom, PATCH_CLKNOMEM + 2, OP_JMP_A5);
    return 0;
}
