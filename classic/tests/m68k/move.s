| Copyright (C) 2026 YuanChi Hsieh
| SPDX-License-Identifier: GPL-3.0-or-later
|
| MOVE variants. Expected: D0=0x12345678 D1=0xFFFFFF80 D2=0x1234FFEE
| D3=0x000000AB D4=0x12345678 (via memory at A0=0x2000, stored big-endian).
    .text
    move.l  #0x12345678,%d0
    moveq   #-128,%d1               | sign-extended to 32 bits
    move.l  #0x12345678,%d2
    move.w  #0xFFEE,%d2             | only the low word changes
    moveq   #0,%d3
    move.b  #0xAB,%d3
    lea     0x2000,%a0
    move.l  %d0,(%a0)               | store, then reload through memory
    move.l  (%a0),%d4
    stop    #0x2700
