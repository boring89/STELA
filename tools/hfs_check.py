#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Checks that an HFS volume image was unmounted cleanly (make
test-classic: the Mac's Shut Down flushed its disk).

Usage: hfs_check.py <volume.img>

The master directory block is at byte 1024: signature "BD" (drSigWord),
then drAtrb at offset 10, whose bit 8 (kHFSVolumeUnmountedBit) the File
Manager sets when it unmounts the volume and clears when it mounts it
(Inside Macintosh: Files, "Master Directory Blocks").
"""
import struct
import sys


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    with open(sys.argv[1], "rb") as f:
        mdb = f.read(1024 + 512)[1024:]
    if len(mdb) < 12 or mdb[0:2] != b"BD":
        print("hfs_check: FAIL no HFS master directory block", file=sys.stderr)
        return 1
    (atrb,) = struct.unpack(">H", mdb[10:12])
    name_len = mdb[36]
    name = mdb[37:37 + min(name_len, 27)].decode("mac_roman")
    print(f"hfs_check: \"{name}\" attributes {atrb:#06x}")
    if not atrb & 0x0100:
        print("hfs_check: FAIL the volume was not unmounted cleanly", file=sys.stderr)
        return 1
    print("hfs_check: OK (unmounted cleanly)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
