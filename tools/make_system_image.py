#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Make the System volume's image (P14 K4, docs/system.md): an eHFS
volume called System holding everything under ROOT.

Usage: make_system_image.py ROOT IMAGE MIB EHFS_TOOLS

IMAGE (under build/, a regular file or none) becomes MIB MiB; the volume
leaves the last 4 KiB block free for the image's trailer (the version,
SHA-256 and signature, docs/system.md §3; written by tools/sysimage.py).
"""
import os
import subprocess
import sys

TRAILER = 4096


def run(*args):
    subprocess.run(args, check=True, stdout=subprocess.DEVNULL)


def main():
    if len(sys.argv) != 5:
        sys.exit(__doc__)
    root, image, mib, tools = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
    if not os.path.abspath(image).startswith(os.path.abspath("build") + os.sep):
        sys.exit(f"make_system_image: {image}: only images under build/")
    if os.path.exists(image) and not os.path.isfile(image):
        sys.exit(f"make_system_image: {image}: not a regular file")
    if os.path.exists(image):
        os.unlink(image)
    size = mib << 20
    with open(image, "wb") as f:
        f.truncate(size)
    sectors = (size - TRAILER) // 512
    run(os.path.join(tools, "mkfs.ehfs"), "-L", "System", "--sectors", str(sectors), image)
    cp = os.path.join(tools, "ehfs-cp")
    count = 0
    for top, dirs, files in os.walk(root):
        dirs.sort()
        rel = os.path.relpath(top, root)
        inside = "/" if rel == "." else "/" + rel.replace(os.sep, "/")
        for d in dirs:
            run(cp, "--sectors", str(sectors), image, "mkdir", inside.rstrip("/") + "/" + d)
        for name in sorted(files):
            run(cp, "--sectors", str(sectors), image, "in", os.path.join(top, name),
                inside.rstrip("/") + "/" + name)
            count += 1
    print(f"make_system_image: {image}: {count} files, {mib} MiB")


if __name__ == "__main__":
    main()
