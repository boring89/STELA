#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Check a WAV file QEMU recorded (-audiodev wav) for make test-sound.

Usage: check_wav.py FILE MIN_SECONDS HZ

Passes if the file holds at least MIN_SECONDS of audio that is not
silent, and the loud part's zero crossings give about HZ (within 10%;
HZ 0 skips the frequency check).
Only the RIFF/WAVE PCM 16-bit format QEMU writes is read.
"""
import struct
import sys


def main():
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    path, min_seconds, hz = sys.argv[1], float(sys.argv[2]), float(sys.argv[3])
    data = open(path, "rb").read()
    if data[0:4] != b"RIFF" or data[8:12] != b"WAVE":
        sys.exit(f"check_wav: {path}: not a WAV file")
    # Chunks after the header: "fmt " then "data".
    pos, fmt, samples = 12, None, None
    while pos + 8 <= len(data):
        kind, size = data[pos:pos + 4], struct.unpack_from("<I", data, pos + 4)[0]
        body = data[pos + 8:pos + 8 + size]
        if kind == b"fmt ":
            fmt = struct.unpack_from("<HHIIHH", body)
        elif kind == b"data":
            # QEMU killed early leaves size 0: take the rest of the file.
            samples = body if size else data[pos + 8:]
        pos += 8 + size + (size & 1)
    if fmt is None or samples is None:
        sys.exit(f"check_wav: {path}: no fmt or data chunk")
    tag, channels, rate, _, _, bits = fmt
    if tag != 1 or bits != 16:
        sys.exit(f"check_wav: {path}: not 16-bit PCM")
    frames = len(samples) // (2 * channels)
    left = [struct.unpack_from("<h", samples, i * 2 * channels)[0] for i in range(frames)]
    loud = [i for i, v in enumerate(left) if abs(v) > 1000]
    print(f"check_wav: {path}: {rate} Hz, {channels} channel(s), {frames / rate:.2f} s, "
          f"{len(loud) / rate:.2f} s loud")
    if not loud or len(loud) < min_seconds * rate * 0.9:
        sys.exit("check_wav: FAIL (silent or too short)")
    if hz == 0:
        print("check_wav: OK")
        return
    part = left[loud[0]:loud[-1] + 1]
    crossings = sum(1 for a, b in zip(part, part[1:]) if (a < 0) != (b < 0))
    measured = crossings / 2 / (len(part) / rate)
    print(f"check_wav: about {measured:.0f} Hz")
    if abs(measured - hz) > hz * 0.1:
        sys.exit(f"check_wav: FAIL (expected about {hz:.0f} Hz)")
    print("check_wav: OK")


if __name__ == "__main__":
    main()
