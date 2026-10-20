| Copyright (C) 2026 YuanChi Hsieh
| SPDX-License-Identifier: GPL-3.0-or-later
|
| The 68k half of the external file system (emu/mac/fsm.c, docs/fsm.md): a
| File System Manager (FSM) plug-in whose work is done by native code. Copied
| into the stub window at MAC_FSM_STUB (MAC_STUB_BASE + 0x800). Assembled with
| m68k-linux-gnu-as -m68030; the bytes are kept in emu/mac/fsm_code.h
| (tools/gen_mac_stub.py; make test checks that they match).
|
| The native code cannot call into the Mac OS itself (no nested emulation),
| so every routine here runs a loop: the native "step" either finishes (D5 =
| -1, result in D0) or asks for one trap call (D5 = 0): the arguments for the
| stack are at A6 (D7 bytes, the first word being the result slot when D6 is
| not 0), D0-D4/A0-A4 hold the register arguments and the trap word has been
| written into trap_slot. The call is made, the native "result" keeps D0/A0,
| and the step runs the handler again from the start, replaying the results
| it already has (emu/mac/fsm.c).
|
| 0x000  install   injected once the Finder runs: registers the file system,
|                  adds its drive and mounts its volume; ends with the NOP at 0x02E
| 0x040  hfs       the HFS component's procedure: pascal OSErr (vcb.l,
|                  selectCode.w, paramBlock.l, globalsPtr.l, fsid.w)
| 0x080  comm      the file system's communication procedure: pascal OSErr
|                  (message.w, paramBlock.l, globalsPtr.l)
| 0x090  run       steps until the native code is done
| 0x0B0  call      makes one trap call for a step (D5-D7, A6 as above)
| 0x0E0  trap_slot the trap word of that call

    .text

| ---------------------------------------------------------------------------
install:
    movem.l %d0-%d7/%a0-%a6,-(%sp)
    .word   0x713A                  | native: start the install
    bsr.w   run
    movem.l (%sp)+,%d0-%d7/%a0-%a6
    nop
    nop
    nop
    nop
    .org    0x2E
install_return:
    nop                             | the injected call has returned

| ---------------------------------------------------------------------------
    .org    0x40
hfs:
    movem.l %d1-%d7/%a0-%a6,-(%sp)  | 56 bytes; the arguments start at 56+4(sp)
    .word   0x713B                  | native: start an HFS call
    bsr.w   run
    move.w  %d0,56+4+16(%sp)        | the function result, above the 16 bytes of arguments
    movem.l (%sp)+,%d1-%d7/%a0-%a6
    rtd     #16

| ---------------------------------------------------------------------------
    .org    0x80
comm:
    .word   0x713C                  | native: D0 = the answer to the message
    move.w  %d0,4+10(%sp)
    rtd     #10

| ---------------------------------------------------------------------------
| run: steps until the native code is done.
    .org    0x90
run:
    nop
    .word   0x713D                  | native: step
    tst.l   %d5
    bmi.s   run_done
    bsr.w   call
    nop
    .word   0x713E                  | native: keep the result of the call
    bra.s   run
run_done:
    rts

| ---------------------------------------------------------------------------
    .org    0xB0
call:
    suba.l  %d7,%sp
    moveq   #0,%d5
call_copy:
    cmp.l   %d7,%d5
    bge.s   call_trap
    move.b  (%a6)+,0(%sp,%d5.l)
    addq.l  #1,%d5
    bra.s   call_copy
call_trap:
    bra.s   trap_slot
    .org    0xE0
trap_slot:
    .word   0xA9FF                  | replaced by the native code before each call
    tst.l   %d6
    beq.s   call_done
    move.w  (%sp)+,%d0              | a Pascal function's result
call_done:
    rts
