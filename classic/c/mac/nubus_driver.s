| Copyright (C) 2026 YuanChi Hsieh
| SPDX-License-Identifier: GPL-3.0-or-later
|
| Video driver of the virtual NuBus card (emu/mac/nubus_video.c). The
| declaration ROM carries it as the card's sMacOS68020 driver; the Slot
| Manager copies it into the system heap, so the code is position
| independent. Assembled with: m68k-linux-gnu-as -m68030; the bytes are kept
| in emu/mac/nubus_driver_code.h (regenerate with tools/gen_mac_stub.py; the
| host test checks that they match this file).
|
| The $712x words are emulated opcodes (emu/mac/mac.h): the real work of
| Open, Control and Status is native. Driver structure: Inside Macintosh:
| Devices, "Device Manager" (driver header, DCE); slot interrupts: Designing
| Cards and Drivers for the Macintosh Family, "Slot Manager" (SIntInstall).
|
| DCE fields used: dCtlStorage +20, dCtlSlot +40, dCtlDevBase +42.
| Parameter block: ioTrap +6, ioResult +16, csCode +26, csParam +28.
|
    .text
driver:
    .word   0x4C00                  | drvrFlags: needs lock, control, status
    .word   0                       | drvrDelay
    .word   0                       | drvrEMask
    .word   0                       | drvrMenu
    .word   drv_open-driver
    .word   drv_prime-driver
    .word   drv_control-driver
    .word   drv_status-driver
    .word   drv_close-driver
    .byte   name_end-name_start
name_start:
    .ascii  ".Display_Video_Apple_68kHAL"
name_end:
    .align  2

| Open: install a slot interrupt handler for the card's vertical blanking
| (an SQElem in the system heap, kept in dCtlStorage), then let the native
| part set up the card (A1 = DCE).
drv_open:
    movem.l %d1-%d2/%a0-%a2,-(%sp)
    movea.l %a1,%a2
    moveq   #16,%d0
    .word   0xA71E                  | _NewPtrSysClear: A0 = 16 cleared bytes
    tst.l   %d0
    bne.s   open_done
    move.l  %a0,20(%a2)             | dCtlStorage = the SQElem
    move.w  #6,4(%a0)               | sqType = sIQType
    lea     drv_interrupt(%pc),%a1
    move.l  %a1,8(%a0)              | sqAddr
    moveq   #0,%d0
    move.b  40(%a2),%d0             | dCtlSlot
    move.l  %d0,12(%a0)             | sqParm = the slot number
    .word   0xA075                  | _SIntInstall (A0 = SQElem, D0 = slot)
    movea.l %a2,%a1
    .word   0x712A                  | native open: D0 = result
open_done:
    movem.l (%sp)+,%d1-%d2/%a0-%a2
    rts

drv_prime:
    moveq   #-17,%d0                | controlErr: there is nothing to read or write
    bra.s   drv_finish

drv_control:
    .word   0x712B                  | native: D0 = result
    bra.s   drv_finish

drv_status:
    .word   0x712C                  | native: D0 = result

| Immediate calls (ioTrap bit 9) return with RTS, queued calls through the
| Device Manager's IODone vector in low memory (0x8FC), A1 = DCE, D0 = result.
drv_finish:
    move.w  %d0,16(%a0)             | ioResult
    btst    #1,6(%a0)               | bit 9 of the ioTrap word
    bne.s   drv_return
    movea.l 0x8FC,%a0
    jmp     (%a0)
drv_return:
    rts

| Close: remove the interrupt handler; the native part stops the card's
| interrupts.
drv_close:
    movem.l %d1-%d2/%a0-%a2,-(%sp)
    movea.l %a1,%a2
    .word   0x712E                  | native close
    movea.l 20(%a2),%a0
    moveq   #0,%d0
    move.b  40(%a2),%d0
    .word   0xA076                  | _SIntRemove (A0 = SQElem, D0 = slot)
    movem.l (%sp)+,%d1-%d2/%a0-%a2
    moveq   #0,%d0
    rts

| The slot interrupt handler (A1 = sqParm = the slot): acknowledge the card's
| interrupt, run the VBL tasks of this slot through the jump vector JVBLTask
| (low memory 0xD28, D0 = slot) and report D0 = 1, "handled".
drv_interrupt:
    .word   0x712D                  | native: clear the card's interrupt
    move.l  %a1,%d0
    movea.l 0xD28,%a0
    jsr     (%a0)
    moveq   #1,%d0
    rts
