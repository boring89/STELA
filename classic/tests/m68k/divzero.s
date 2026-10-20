| Copyright (C) 2026 YuanChi Hsieh
| SPDX-License-Identifier: GPL-3.0-or-later
|
| Divide by zero raises vector 5 (address 0x14). The handler sets D7 and
| returns with RTE. Expected: D7=0xDEAD, D0=7 (the instruction after DIVU
| ran), D1 unchanged.
    .text
    lea     handler(%pc),%a0
    move.l  %a0,0x14
    moveq   #7,%d1
    moveq   #0,%d2
    divu    %d2,%d1                 | exception
    moveq   #7,%d0
    stop    #0x2700

handler:
    move.l  #0xDEAD,%d7
    rte
