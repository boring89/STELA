| Copyright (C) 2026 YuanChi Hsieh
| SPDX-License-Identifier: GPL-3.0-or-later
|
| Native-call stubs of the Macintosh model, mapped at MAC_STUB_BASE
| (0x4F000000). Assembled with: m68k-linux-gnu-as -m68030; the bytes are kept
| in emu/mac/stub_code.h (regenerate with tools/gen_mac_stub.py; the host test
| checks that they match this file).
|
| The $71xx words are "emulated opcodes": illegal 68k instructions that
| emu/mac/mac.c intercepts to run native code (see emu/mac/mac.h).
|
| 0x000  install      called by the patched ROM instead of its code at 0x1166
| 0x060  adbop        replaces the ADBOp trap ($A07C)
| 0x0A0  inject       runs the ROM's mouse and keyboard handlers for queued input
| 0x100  .Disk driver header (a DRVR as the Device Manager expects it)
| 0x1F0  name of the driver as a Pascal string
|
    .text
    .equ    ROM_INIT_DRIVERS_END, 0x40801182    | the RTS of InitDrivers

| ---------------------------------------------------------------------------
| install: add the .Disk driver to the unit table and open it.
| Entered with JSR from ROM offset 0x1166, where the ROM was about to load the
| serial drivers ("subq.w #4,sp; move.l #'SERD',-(sp)"). Their code drives the
| SCC and waits for its interrupts, which this machine does not have, so it
| is left out: install ends the routine with the ROM's own RTS at 0x1182.
| ---------------------------------------------------------------------------
install:
    movem.l %d0-%d7/%a0-%a6,-(%sp)
    move.w  #-50,%d0                | driver reference number
    .word   0xA43D                  | _DrvrInstallRsrvMem
    tst.w   %d0
    bne.s   install_done
    movea.l 0x11C,%a0               | UTableBase: one handle per unit,
    movea.l 49*4(%a0),%a0           | index = ~refnum = 49
    movea.l (%a0),%a1               | A1 = the DCE
    .word   0x710A                  | native: fill in the DCE (driver pointer, flags)
    suba.w  #50,%sp                 | an IOParam block for _Open
    movea.l %sp,%a0
    clr.b   27(%a0)                 | ioPermssn
    lea     driver_name(%pc),%a1
    move.l  %a1,18(%a0)             | ioNamePtr
    .word   0xA000                  | _Open
    adda.w  #50,%sp
install_done:
    movem.l (%sp)+,%d0-%d7/%a0-%a6
    addq.l  #4,%sp                  | drop our return address and finish
    jmp     ROM_INIT_DRIVERS_END    | InitDrivers like the ROM would, but
                                    | without the serial drivers (no SCC)

| ---------------------------------------------------------------------------
| adbop: the ROM talks to keyboards and mice through the VIA and an interrupt
| driven state machine. There is no ADB hardware, so every ADB command is
| answered at once with "no data" (an empty block): ADBOp is called with
| A0 -> { data pointer, completion routine, completion data }, D0 = command;
| the completion routine gets A0 = the data block (first byte = length),
| A1 = itself, A2 = the completion data, D0 = the command.
| ---------------------------------------------------------------------------
    .org    0x60
adbop:
    movem.l %d1-%d3/%a0-%a2,-(%sp)
    movea.l 4(%a0),%a1              | completion routine
    movea.l 8(%a0),%a2              | its data
    move.l  %a1,%d3
    beq.s   adbop_done
    lea     adbop_empty(%pc),%a0
    jsr     (%a1)
adbop_done:
    movem.l (%sp)+,%d1-%d3/%a0-%a2
    moveq   #0,%d0
    rts
adbop_empty:
    .word   0

| ---------------------------------------------------------------------------
| inject: entered like a level 1 interrupt (the model pushes a format 0 frame,
| see mac.c) when keyboard or mouse input is waiting. The emulated opcode
| takes the next queued packet and sets up what the ADB Manager passes to a
| device handler: D1 = 1 (0 = nothing left), A1 = the handler, A2 = its data
| area, D0 = the Talk register 0 command, A0 = a block { length, byte 1,
| byte 2 } like the ones the ADB hardware would deliver.
| ---------------------------------------------------------------------------
    .org    0xA0
inject:
    movem.l %d0-%d7/%a0-%a6,-(%sp)
inject_next:
    .word   0x7114
    tst.l   %d1
    beq.s   inject_done
    jsr     (%a1)
    bra.s   inject_next
inject_done:
    movem.l (%sp)+,%d0-%d7/%a0-%a6
    rte

| ---------------------------------------------------------------------------
| The .Disk driver (Inside Macintosh: Devices, "Device Manager", driver header)
| ---------------------------------------------------------------------------
    .org    0x100
driver:
    .word   0x6F00                  | drvrFlags: read, write, control, status, needs lock and time
    .word   60                      | drvrDelay (ticks between periodic calls)
    .word   0                       | drvrEMask
    .word   0                       | drvrMenu
    .word   drv_open-driver
    .word   drv_prime-driver
    .word   drv_control-driver
    .word   drv_status-driver
    .word   drv_close-driver
driver_name:
    .byte   5
    .ascii  ".Disk"
    .align  2

| Open: the native part fills in a drive record and the drive queue entry for
| each drive (A0 = qLink of the record, D0 = drive number << 16 | refnum;
| D1 = 0 when there are no more drives); _AddDrive links it into the drive queue.
drv_open:
    .word   0x7110                  | native: start with the first drive
    nop                             | (the model runs one emulated opcode per instruction)
drv_open_next:
    .word   0x7115                  | native: the next drive's record
    tst.l   %d1
    beq.s   drv_open_done
    .word   0xA04E                  | _AddDrive
    bra.s   drv_open_next
drv_open_done:
    moveq   #0,%d0
    rts

drv_prime:
    .word   0x7111
    bra.s   drv_finish

drv_control:
    .word   0x7112
    bra.s   drv_finish

drv_status:
    .word   0x7113

| Immediate calls (ioTrap bit 9) return with RTS, queued calls through the
| Device Manager's IODone vector in low memory (0x8FC), A1 = DCE, D0 = result.
drv_finish:
    btst    #9,6(%a0)
    bne.s   drv_return
    movea.l 0x8FC,%a0
    jmp     (%a0)
drv_return:
    rts

drv_close:
    moveq   #-24,%d0                | closeErr: a disk driver is never closed
    rts
