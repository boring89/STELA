#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Rough check that a QEMU screendump (binary PPM, P6) shows console text.

Usage: fb_check.py <screen.ppm> [--min-ink 0.001] [--max-ink 0.3]

The most common colour is taken as the background; "ink" is every other
pixel. The screen passes if it is not blank (ink fraction at least
--min-ink) and not noise (at most --max-ink), and if the ink is spread over
several text rows (at least 3 separate bands of rows with ink).
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
    args = ap.parse_args()

    width, height, pixels = read_ppm(args.ppm)
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


if __name__ == "__main__":
    sys.exit(main())
