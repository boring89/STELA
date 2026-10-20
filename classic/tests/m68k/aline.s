| Copyright (C) 2026 YuanChi Hsieh
| SPDX-License-Identifier: GPL-3.0-or-later
|
| A-line traps. Expected: the VM records 0xA9F4 then 0xA000 and continues:
| D0=1 (before), D1=2 (between), D2=3 (after).
    .text
    moveq   #1,%d0
    .short  0xA9F4
    moveq   #2,%d1
    .short  0xA000
    moveq   #3,%d2
    stop    #0x2700
