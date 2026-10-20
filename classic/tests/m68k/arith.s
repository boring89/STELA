| Copyright (C) 2026 YuanChi Hsieh
| SPDX-License-Identifier: GPL-3.0-or-later
|
| Arithmetic and CCR flags. SR is copied into D4..D7 after each operation.
| D4: 0x7FFFFFFF+1  -> N,V set        (CCR=0x0A)
| D5: 0xFFFFFFFF+1  -> Z,C,X set      (CCR=0x13)
| D6: 1-2           -> N,C,X set      (CCR=0x19)
| D7: cmp 5,5       -> Z set, X kept  (CCR=0x14)
    .text
    move.l  #0x7FFFFFFF,%d0
    addq.l  #1,%d0
    move    %sr,%d4
    moveq   #-1,%d1
    addq.l  #1,%d1
    move    %sr,%d5
    moveq   #1,%d2
    subq.l  #2,%d2
    move    %sr,%d6
    moveq   #5,%d3
    cmp.l   #5,%d3
    move    %sr,%d7
    stop    #0x2700
