#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Rough checks of a QEMU screendump (binary PPM, P6).

Usage: fb_check.py <screen.ppm> [--min-ink 0.001] [--max-ink 0.3]
                   [--expect-rect X,Y,W,H,#RRGGBB] ...
                   [--expect-text X,Y,W,H,#RRGGBB,BANDS] ...
                   [--expect-pattern X,Y,W,H,#RRGGBB,#RRGGBB] ...

Without rectangle options (the text console): the most common colour is
taken as the background; "ink" is every other pixel. The screen passes if
it is not blank (ink fraction at least --min-ink) and not noise (at most
--max-ink), and if the ink is spread over several text rows (at least 3
separate bands of rows with ink).

--expect-rect: the most common colour inside the rectangle must be the
given one (the desktop, a menu bar, a title bar). --expect-text: inside
the rectangle, the pixels that are not the given background colour must
form at least BANDS separate bands of rows (lines of text in a window).
--expect-pattern: the rectangle is a checkerboard of the two colours
(each pixel differs from its right and lower neighbours: the Mac's 50%
gray desktop pattern).
"""
import argparse
import sys
from collections import Counter


def read_ppm(path):
    with open(path, "rb") as f:
        data = f.read()
    # Header: "P6", width, height, maxval, separated by whitespace.
    fields = []
    pos = 0
    while len(fields) < 4:
        while data[pos:pos + 1].isspace():
            pos += 1
        if data[pos:pos + 1] == b"#":
            pos = data.index(b"\n", pos) + 1
            continue
        end = pos
        while not data[end:end + 1].isspace():
            end += 1
        fields.append(data[pos:end])
        pos = end
    pos += 1
    if fields[0] != b"P6" or int(fields[3]) != 255:
        sys.exit(f"{path}: not an 8-bit binary PPM")
    width, height = int(fields[1]), int(fields[2])
    pixels = data[pos:pos + width * height * 3]
    if len(pixels) != width * height * 3:
        sys.exit(f"{path}: truncated")
    return width, height, pixels


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("ppm")
    ap.add_argument("--min-ink", type=float, default=0.001)
    ap.add_argument("--max-ink", type=float, default=0.3)
    ap.add_argument("--expect-rect", action="append", default=[])
    ap.add_argument("--expect-text", action="append", default=[])
    ap.add_argument("--expect-pattern", action="append", default=[])
    args = ap.parse_args()

    width, height, pixels = read_ppm(args.ppm)
    if args.expect_rect or args.expect_text or args.expect_pattern:
        return check_rects(args, width, height, pixels)
    colours = [pixels[i:i + 3] for i in range(0, len(pixels), 3)]
    background, _ = Counter(colours).most_common(1)[0]
    ink_rows = [any(colours[y * width + x] != background for x in range(width))
                for y in range(height)]
    ink = sum(1 for c in colours if c != background) / len(colours)
    bands = sum(1 for y in range(height) if ink_rows[y] and (y == 0 or not ink_rows[y - 1]))

    print(f"fb_check: {width}x{height}, background #{background.hex()}, "
          f"ink {ink:.2%}, {bands} text bands")
    if not args.min_ink <= ink <= args.max_ink:
        print("fb_check: FAIL (blank or noisy screen)", file=sys.stderr)
        return 1
    if bands < 3:
        print("fb_check: FAIL (fewer than 3 lines of text)", file=sys.stderr)
        return 1
    print("fb_check: OK")
    return 0


def parse_rect(spec, extra):
    parts = spec.split(",")
    if len(parts) != 5 + extra:
        sys.exit(f"bad rectangle {spec!r}")
    x, y, w, h = (int(p) for p in parts[:4])
    colour = bytes.fromhex(parts[4].lstrip("#"))
    return x, y, w, h, colour, parts[5:]


def region(width, height, pixels, x, y, w, h):
    """The rows of a rectangle (clipped to the screen), as lists of RGB."""
    rows = []
    for py in range(max(0, y), min(height, y + h)):
        row = pixels[(py * width + max(0, x)) * 3:(py * width + min(width, x + w)) * 3]
        rows.append([row[i:i + 3] for i in range(0, len(row), 3)])
    return rows


def check_rects(args, width, height, pixels):
    ok = True
    for spec in args.expect_rect:
        x, y, w, h, colour, _ = parse_rect(spec, 0)
        counts = Counter(c for row in region(width, height, pixels, x, y, w, h) for c in row)
        if not counts:
            print(f"fb_check: FAIL rectangle {spec} is off the screen", file=sys.stderr)
            ok = False
            continue
        common, n = counts.most_common(1)[0]
        total = sum(counts.values())
        print(f"fb_check: {x},{y} {w}x{h}: mostly #{common.hex()} ({n / total:.0%})")
        if common != colour:
            print(f"fb_check: FAIL expected #{colour.hex()} there", file=sys.stderr)
            ok = False
    for spec in args.expect_text:
        x, y, w, h, colour, extra = parse_rect(spec, 1)
        rows = region(width, height, pixels, x, y, w, h)
        inked = [any(c != colour for c in row) for row in rows]
        bands = sum(1 for i, r in enumerate(inked) if r and (i == 0 or not inked[i - 1]))
        print(f"fb_check: {x},{y} {w}x{h}: {bands} bands of text on #{colour.hex()}")
        if bands < int(extra[0]):
            print(f"fb_check: FAIL expected at least {extra[0]} lines", file=sys.stderr)
            ok = False
    for spec in args.expect_pattern:
        x, y, w, h, first, extra = parse_rect(spec, 1)
        second = bytes.fromhex(extra[0].lstrip("#"))
        rows = region(width, height, pixels, x, y, w, h)
        cells = sum(len(r) for r in rows)
        good = 0
        if rows and rows[0]:
            # The colour of the top left pixel fixes the phase.
            phase = 0 if rows[0][0] == first else 1
            good = sum(1 for j, row in enumerate(rows) for i, c in enumerate(row)
                       if c == (first if (i + j + phase) % 2 == 0 else second))
        print(f"fb_check: {x},{y} {w}x{h}: checkerboard of #{first.hex()} and #{second.hex()} "
              f"{good / max(cells, 1):.0%}")
        if cells == 0 or good != cells:
            print("fb_check: FAIL expected the checkerboard there", file=sys.stderr)
            ok = False
    print("fb_check: OK" if ok else "fb_check: FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
