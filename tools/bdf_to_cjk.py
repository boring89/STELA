#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Convert a 12-pixel BDF font into STELA's glyph file for the toolbox
(P11 K39, docs/toolbox.md section 6.3).

Usage: bdf_to_cjk.py FONT.bdf OUT.bin

Only code points from U+0080 to U+FFFF are kept (ASCII is drawn with
Spleen). The file, little endian:

    "STF1"                      magic
    u32 count                   glyphs
    u16 12                      rows a glyph has
    u16 28                      bytes a record has
    count records, sorted by code point:
        u16 code point
        u8  advance in pixels (6: one cell, 12: two cells)
        u8  0
        12 x u16 rows, top first; bit 15 is the leftmost pixel

Each glyph is placed in a 12 x 12 box from the font's bounding box: the
baseline is FONT_ASCENT rows down, BBX offsets move it.
"""
import struct
import sys

ROWS = 12


def parse(path):
    glyphs = {}
    ascent = 10
    with open(path, encoding="latin-1") as f:
        lines = iter(f.read().splitlines())
    for line in lines:
        if line.startswith("FONT_ASCENT"):
            ascent = int(line.split()[1])
        if not line.startswith("STARTCHAR"):
            continue
        cp, adv, bbx, bitmap = None, 12, (0, 0, 0, 0), []
        for line in lines:
            if line.startswith("ENCODING"):
                cp = int(line.split()[1])
            elif line.startswith("DWIDTH"):
                adv = int(line.split()[1])
            elif line.startswith("BBX"):
                bbx = tuple(int(v) for v in line.split()[1:5])
            elif line.startswith("BITMAP"):
                for line in lines:
                    if line.startswith("ENDCHAR"):
                        break
                    bitmap.append(line.strip())
                break
        if cp is None or not (0x80 <= cp <= 0xFFFF):
            continue
        w, h, xoff, yoff = bbx
        rows = [0] * ROWS
        top = ascent - (yoff + h)
        for i, hexrow in enumerate(bitmap[:h]):
            if not hexrow:
                continue
            bits = int(hexrow, 16)
            nbits = len(hexrow) * 4
            for x in range(w):
                if bits >> (nbits - 1 - x) & 1:
                    px, py = xoff + x, top + i
                    if 0 <= px < 16 and 0 <= py < ROWS:
                        rows[py] |= 0x8000 >> px
        glyphs[cp] = (max(1, min(adv, 12)), rows)
    return glyphs


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    glyphs = parse(sys.argv[1])
    out = bytearray(b"STF1")
    out += struct.pack("<IHH", len(glyphs), ROWS, 4 + 2 * ROWS)
    for cp in sorted(glyphs):
        adv, rows = glyphs[cp]
        out += struct.pack("<HBB", cp, adv, 0)
        out += struct.pack("<12H", *rows)
    with open(sys.argv[2], "wb") as f:
        f.write(out)
    print(f"bdf_to_cjk: {len(glyphs)} glyphs, {len(out)} bytes")


if __name__ == "__main__":
    main()
