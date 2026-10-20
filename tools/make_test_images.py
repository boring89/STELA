#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Create the disk images the host tests and the storage test use.

Usage: make_test_images.py host DIR
           fat-test.img   FAT32 file system (mtools) with known files
           gpt-test.img   GPT made by sgdisk: ESP + "Data"

Only regular files under build/ are created or written; anything else
(a block device, a path outside build/) is refused. The tools run on the
image files only (mtools with -i, sgdisk on a file path).
"""
import os
import stat
import subprocess
import sys

MIB = 1 << 20


def checked(path):
    """The real path of `path`, which must be (or become) a regular file
    under build/."""
    real = os.path.realpath(path)
    build = os.path.realpath("build")
    if not real.startswith(build + os.sep):
        sys.exit(f"make_test_images: {path} is not under build/; refused")
    if os.path.exists(real) and not stat.S_ISREG(os.stat(real).st_mode):
        sys.exit(f"make_test_images: {path} is not a regular file; refused")
    return real


def run(*args):
    subprocess.run(args, check=True, stdout=subprocess.DEVNULL)


def pattern(size):
    """Bytes that differ in every position modulo 251, so misplaced
    clusters show."""
    return bytes((i * 7 + i // 4096) % 251 for i in range(size))


def make_fat(path, size_mib, label):
    """A FAT32 image holding the files the tests read."""
    img = checked(path)
    if os.path.exists(img):
        os.remove(img)
    with open(img, "wb") as f:
        f.truncate(size_mib * MIB)
    run("mformat", "-i", img, "-F", "-v", label, "::")
    work = checked(os.path.join(os.path.dirname(img), "fat-files"))
    os.makedirs(work, exist_ok=True)
    files = {
        "HELLO.TXT": b"hello from FAT\n",
        "A long file name.txt": b"long names work\n",
        "big.bin": pattern(300 * 1024),
    }
    for name, data in files.items():
        with open(os.path.join(work, name), "wb") as f:
            f.write(data)
        run("mcopy", "-i", img, os.path.join(work, name), "::/" + name)
    run("mmd", "-i", img, "::/EFI")
    run("mmd", "-i", img, "::/EFI/BOOT")
    run("mcopy", "-i", img, os.path.join(work, "HELLO.TXT"), "::/EFI/BOOT/nested.txt")


def make_gpt(path, size_mib):
    img = checked(path)
    if os.path.exists(img):
        os.remove(img)
    with open(img, "wb") as f:
        f.truncate(size_mib * MIB)
    run("sgdisk", "-n", "1:2048:+16M", "-t", "1:EF00", "-c", "1:EFI system",
        "-n", "2:0:0", "-t", "2:8300", "-c", "2:Data", img)


def main():
    if len(sys.argv) != 3 or sys.argv[1] != "host":
        sys.exit(__doc__)
    out = sys.argv[2]
    os.makedirs(os.path.dirname(checked(os.path.join(out, "x"))), exist_ok=True)
    make_fat(os.path.join(out, "fat-test.img"), 64, "BOOT")
    make_gpt(os.path.join(out, "gpt-test.img"), 64)


if __name__ == "__main__":
    main()
