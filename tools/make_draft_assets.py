#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Draws STELA's DRAFT icons as simple geometry and writes them as
assets/<name>-<size>.ppm (colour, P6) and .pbm (1-bit, P4), for sizes
16, 32, 64 and 128.

DRAFT — to be replaced by the user's own artwork. These are placeholder
shapes made from rectangles, triangles and ellipses (no existing work was
traced); `assets/README.md` explains how to replace them.

Colours (black-figure pottery): terracotta #C8763C, glaze black #1A1410,
parchment white #F2E8D5; #FF00FF in a PPM means transparent.

Usage: make_draft_assets.py OUTDIR
"""
import os
import sys

TERRACOTTA = (0xC8, 0x76, 0x3C)
BLACK = (0x1A, 0x14, 0x10)
WHITE = (0xF2, 0xE8, 0xD5)
DARK = (0x8E, 0x4A, 0x22)   # shaded terracotta
CLEAR = (0xFF, 0x00, 0xFF)  # transparent
SIZES = (16, 32, 64, 128)


class Canvas:
    """A size x size image; shapes take coordinates in 0..1."""

    def __init__(self, size):
        self.n = size
        self.px = [[CLEAR] * size for _ in range(size)]

    def _each(self, inside, colour):
        n = self.n
        for y in range(n):
            for x in range(n):
                if inside((x + 0.5) / n, (y + 0.5) / n):
                    self.px[y][x] = colour

    def rect(self, x0, y0, x1, y1, colour):
        self._each(lambda x, y: x0 <= x < x1 and y0 <= y < y1, colour)

    def ellipse(self, cx, cy, rx, ry, colour):
        self._each(lambda x, y: ((x - cx) / rx) ** 2 + ((y - cy) / ry) ** 2 <= 1, colour)

    def poly(self, points, colour):
        def inside(x, y):
            hit = False
            j = len(points) - 1
            for i, (xi, yi) in enumerate(points):
                xj, yj = points[j]
                if (yi > y) != (yj > y) and x < (xj - xi) * (y - yi) / (yj - yi) + xi:
                    hit = not hit
                j = i
            return hit
        self._each(inside, colour)

    def hline(self, x0, x1, y, colour, thick=1):
        """A horizontal line `thick` pixels high (at least one) at y."""
        n = self.n
        row = min(n - 1, int(y * n))
        for dy in range(max(1, thick * n // 64)):
            r = row + dy
            if 0 <= r < n:
                for x in range(int(x0 * n), min(n, int(x1 * n + 0.5))):
                    self.px[r][x] = colour

    def vline(self, x, y0, y1, colour, thick=1):
        n = self.n
        col = min(n - 1, int(x * n))
        for dx in range(max(1, thick * n // 64)):
            c = col + dx
            if 0 <= c < n:
                for y in range(int(y0 * n), min(n, int(y1 * n + 0.5))):
                    self.px[y][c] = colour

    def outline(self):
        """Black on every opaque pixel that touches transparency or the
        edge (the glaze line around a figure)."""
        n = self.n
        old = [row[:] for row in self.px]
        for y in range(n):
            for x in range(n):
                if old[y][x] == CLEAR:
                    continue
                for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                    a, b = x + dx, y + dy
                    if not (0 <= a < n and 0 <= b < n) or old[b][a] == CLEAR:
                        self.px[y][x] = BLACK
                        break


def stela_shape(c, cracked=False):
    # Base step, slab, pediment with acroteria, carved lines.
    c.rect(0.22, 0.86, 0.78, 0.96, DARK)
    c.rect(0.30, 0.26, 0.70, 0.87, TERRACOTTA)
    c.poly([(0.26, 0.27), (0.50, 0.08), (0.74, 0.27)], TERRACOTTA)
    c.ellipse(0.50, 0.19, 0.05, 0.04, WHITE)
    for i in range(6):
        c.hline(0.36, 0.64, 0.36 + i * 0.075, BLACK)
    if cracked:
        # A zigzag crack down the middle, cleared out.
        pts = [(0.47, 0.05), (0.55, 0.30), (0.45, 0.50), (0.56, 0.70), (0.48, 0.98)]
        left = [(x - 0.035, y) for x, y in pts]
        right = [(x + 0.035, y) for x, y in reversed(pts)]
        c.poly(left + right, CLEAR)
    c.outline()


def stela(c):
    stela_shape(c)


def stela_cracked(c):
    stela_shape(c, cracked=True)


def temple(c):
    c.rect(0.10, 0.86, 0.90, 0.95, DARK)          # stylobate
    c.rect(0.14, 0.80, 0.86, 0.87, TERRACOTTA)
    c.poly([(0.10, 0.34), (0.50, 0.10), (0.90, 0.34)], TERRACOTTA)  # pediment
    c.rect(0.12, 0.33, 0.88, 0.42, WHITE)          # architrave
    for i in range(4):
        x = 0.20 + i * 0.18
        c.rect(x, 0.42, x + 0.08, 0.80, TERRACOTTA)  # columns
    c.outline()
    c.hline(0.12, 0.88, 0.375, BLACK)


def scroll(c):
    c.rect(0.18, 0.22, 0.82, 0.78, WHITE)
    c.ellipse(0.18, 0.50, 0.07, 0.30, TERRACOTTA)
    c.ellipse(0.82, 0.50, 0.07, 0.30, TERRACOTTA)
    for i in range(5):
        c.hline(0.30, 0.70, 0.32 + i * 0.09, BLACK)
    c.outline()


def kibotos(c):
    c.rect(0.10, 0.30, 0.90, 0.40, DARK)           # lid
    c.rect(0.12, 0.40, 0.88, 0.86, TERRACOTTA)     # chest
    c.rect(0.12, 0.54, 0.88, 0.68, BLACK)          # band
    # A meander-like band: white squares with black notches.
    for i in range(6):
        x = 0.15 + i * 0.12
        c.rect(x, 0.565, x + 0.08, 0.655, WHITE)
        c.rect(x + 0.03, 0.565, x + 0.05, 0.62, BLACK)
    c.rect(0.16, 0.86, 0.24, 0.94, DARK)           # feet
    c.rect(0.76, 0.86, 0.84, 0.94, DARK)
    c.outline()


def volume(c):
    c.rect(0.08, 0.30, 0.92, 0.74, TERRACOTTA)
    c.rect(0.08, 0.62, 0.92, 0.74, DARK)
    c.rect(0.16, 0.44, 0.56, 0.52, BLACK)          # slot
    c.ellipse(0.80, 0.48, 0.05, 0.05, WHITE)       # light
    c.outline()


def app(c):
    # A small stela with a seal.
    c.rect(0.28, 0.88, 0.72, 0.96, DARK)
    c.rect(0.34, 0.30, 0.66, 0.89, TERRACOTTA)
    c.poly([(0.30, 0.31), (0.50, 0.12), (0.70, 0.31)], TERRACOTTA)
    c.ellipse(0.62, 0.70, 0.17, 0.17, WHITE)
    c.ellipse(0.62, 0.70, 0.10, 0.10, TERRACOTTA)
    c.outline()


ICONS = {
    "stela": stela,
    "stela-cracked": stela_cracked,
    "temple": temple,
    "scroll": scroll,
    "kibotos": kibotos,
    "volume": volume,
    "app": app,
}

COMMENT = b"# DRAFT - to be replaced by the user's own artwork (assets/README.md)\n"


def write_ppm(path, c):
    with open(path, "wb") as f:
        f.write(b"P6\n" + COMMENT + f"{c.n} {c.n}\n255\n".encode())
        f.write(bytes(v for row in c.px for p in row for v in p))


def write_pbm(path, c):
    """1-bit: set (black) for every pixel that is not transparent or
    parchment white (so the white details stay white)."""
    with open(path, "wb") as f:
        f.write(b"P4\n" + COMMENT + f"{c.n} {c.n}\n".encode())
        for row in c.px:
            bits = [1 if p not in (CLEAR, WHITE) else 0 for p in row]
            out = bytearray((len(bits) + 7) // 8)
            for i, b in enumerate(bits):
                if b:
                    out[i // 8] |= 0x80 >> (i % 8)
            f.write(bytes(out))


def main():
    out = sys.argv[1]
    os.makedirs(out, exist_ok=True)
    for name, draw in ICONS.items():
        for size in SIZES:
            c = Canvas(size)
            draw(c)
            write_ppm(os.path.join(out, f"{name}-{size}.ppm"), c)
            write_pbm(os.path.join(out, f"{name}-{size}.pbm"), c)


if __name__ == "__main__":
    main()
