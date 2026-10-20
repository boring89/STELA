| Copyright (C) 2026 YuanChi Hsieh
| SPDX-License-Identifier: GPL-3.0-or-later
|
| BSR/RTS and stack use. Initial SP is 0x8000 (set by the test).
| D0 = double(21) = 42, D1 = 0x11111111 restored from the stack,
| A7 = 0x8000 again after balanced pushes/pops.
    .text
    moveq   #21,%d0
    bsr     double
    move.l  #0x11111111,%d1
    move.l  %d1,-(%sp)
    moveq   #0,%d1
    move.l  (%sp)+,%d1
    stop    #0x2700

double:
    add.l   %d0,%d0
    rts
