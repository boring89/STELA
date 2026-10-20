#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Cross-check STELA's eHFS with 68k_HAL's (local only).

Usage: ehfs_compat.py STELA_TOOLS HAL_TOOLS WORKDIR

1. STELA's mkfs.ehfs and ehfs-cp make an image (folders, UTF-8 names, a
   multi-megabyte file); 68k_HAL's fsck.ehfs must find it clean and its
   ehfs-cp must read the files back unchanged.
2. 68k_HAL's mkfs.ehfs and ehfs-cp make an image; STELA's fsck.ehfs must
   find it clean, and STELA's ehfs-cp must read the files back and write
   more, after which 68k_HAL's fsck.ehfs must still find it clean.

Only image files under build/ are made or touched (both tool sets refuse
anything but regular files). Skipped when 68k_HAL's tools are missing.
"""
import os
import subprocess
import sys


def run(*args):
    r = subprocess.run(args, capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"ehfs_compat: {' '.join(args)} failed:\n{r.stdout}{r.stderr}")
    return r.stdout


def main():
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    stela, hal, work = sys.argv[1:]
    if not os.path.exists(os.path.join(hal, "fsck.ehfs")):
        print(f"ehfs_compat: no 68k_HAL tools in {hal}; skipped")
        return
    work = os.path.realpath(work)
    if not work.startswith(os.path.realpath("build") + os.sep):
        sys.exit("ehfs_compat: the work directory must be under build/")
    os.makedirs(work, exist_ok=True)
    files = {
        "small.txt": b"hello from STELA\n",
        "big.bin": bytes((i * 13 + i // 4096) % 256 for i in range(3 << 20)),
        "Ünïcödé näme.txt": "名前 and accents\n".encode(),
    }
    for name, data in files.items():
        with open(os.path.join(work, name), "wb") as f:
            f.write(data)

    def check_files(cp, image):
        for name, data in files.items():
            out = os.path.join(work, "out.bin")
            run(cp, image, "out", f"/docs/{name}", out)
            with open(out, "rb") as f:
                if f.read() != data:
                    sys.exit(f"ehfs_compat: {name} differs after the round trip ({cp})")

    # 1. Made by STELA, checked and read by 68k_HAL.
    image = os.path.join(work, "stela.img")
    if os.path.exists(image):
        os.remove(image)
    run(os.path.join(stela, "mkfs.ehfs"), "-L", "Data", "--size", "64", image)
    run(os.path.join(stela, "ehfs-cp"), image, "mkdir", "/docs")
    for name in files:
        run(os.path.join(stela, "ehfs-cp"), image, "in", os.path.join(work, name), f"/docs/{name}")
    print(run(os.path.join(hal, "fsck.ehfs"), image).strip())
    check_files(os.path.join(hal, "ehfs-cp"), image)
    print("ehfs_compat: 68k_HAL reads STELA's volume")

    # 2. Made by 68k_HAL, checked, read and changed by STELA.
    image = os.path.join(work, "hal.img")
    with open(image, "wb") as f:
        f.truncate(64 << 20)
    run(os.path.join(hal, "mkfs.ehfs"), "-L", "Data", image)
    run(os.path.join(hal, "ehfs-cp"), image, "mkdir", "/docs")
    for name in files:
        run(os.path.join(hal, "ehfs-cp"), image, "in", os.path.join(work, name), f"/docs/{name}")
    print(run(os.path.join(stela, "fsck.ehfs"), image).strip())
    check_files(os.path.join(stela, "ehfs-cp"), image)
    run(os.path.join(stela, "ehfs-cp"), image, "mkdir", "/from stela")
    run(os.path.join(stela, "ehfs-cp"), image, "in", os.path.join(work, "big.bin"), "/from stela/copy.bin")
    run(os.path.join(stela, "ehfs-cp"), image, "rm", "/docs/small.txt")
    print(run(os.path.join(hal, "fsck.ehfs"), image).strip())
    out = os.path.join(work, "out.bin")
    run(os.path.join(hal, "ehfs-cp"), image, "out", "/from stela/copy.bin", out)
    with open(out, "rb") as f:
        if f.read() != files["big.bin"]:
            sys.exit("ehfs_compat: 68k_HAL reads a different copy.bin")
    print("ehfs_compat: STELA reads and writes 68k_HAL's volume; OK")


main()
