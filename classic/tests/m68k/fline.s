| Copyright (C) 2026 YuanChi Hsieh
| SPDX-License-Identifier: GPL-3.0-or-later
|
| The guest has no FPU: an F-line opcode (here an FNOP, $F280 $0000) raises
| vector 11 (address 0x2C). The handler stores the stacked PC in D6 and the
| format/vector word in D5, then skips the instruction and returns.
| Expected: D7=0xF11E, D0=7, D6 = address of the F-line opcode,
| D5=0x2C (format 0, vector offset 11*4).
    .text
    lea     handler(%pc),%a0
    move.l  %a0,0x2C
    lea     fop(%pc),%a1
    move.l  %a1,%d4
fop:
    .word   0xF280
    .word   0x0000
    moveq   #7,%d0
    stop    #0x2700

handler:
    move.l  2(%sp),%d6
    moveq   #0,%d5
    move.w  6(%sp),%d5
    addq.l  #4,2(%sp)
    move.l  #0xF11E,%d7
    rte
