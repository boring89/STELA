| Copyright (C) 2026 YuanChi Hsieh
| SPDX-License-Identifier: GPL-3.0-or-later
|
| Branches and loops. D0 = 1+2+...+10 = 55 (DBRA loop),
| D1 = 3 (BNE countdown), D2 = 1 (BEQ taken), D3 = 0 (BNE not taken).
    .text
    moveq   #0,%d0
    moveq   #10,%d4
1:  add.l   %d4,%d0
    subq.l  #1,%d4
    bne     1b

    moveq   #0,%d1
    moveq   #2,%d5
2:  addq.l  #1,%d1
    dbra    %d5,2b                  | runs 3 times (D5 = 2,1,0)

    moveq   #0,%d2
    moveq   #0,%d6
    tst.l   %d6
    beq     3f
    moveq   #99,%d2                 | skipped
3:  addq.l  #1,%d2

    moveq   #0,%d3
    tst.l   %d6
    bne     4f
    bra     5f
4:  moveq   #99,%d3                 | skipped
5:  stop    #0x2700
