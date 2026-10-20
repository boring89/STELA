#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Create a zero-filled disk image for the storage tests.

Usage: make_test_disk.py IMAGE SIZE_MIB

The image starts with the magic "STELATEST", which the self test looks
for before it writes to a disk. Only regular files under build/ are
written; anything else (a block device, a path outside build/) is refused.
"""
import os
import stat
import sys


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    path, size_mib = sys.argv[1], int(sys.argv[2])
    real = os.path.realpath(path)
    build = os.path.realpath("build")
    if not real.startswith(build + os.sep):
        sys.exit(f"make_test_disk: {path} is not under build/; refused")
    if os.path.exists(real) and not stat.S_ISREG(os.stat(real).st_mode):
        sys.exit(f"make_test_disk: {path} is not a regular file; refused")
    with open(real, "wb") as f:
        f.truncate(size_mib << 20)
        f.write(b"STELATEST")


if __name__ == "__main__":
    main()
