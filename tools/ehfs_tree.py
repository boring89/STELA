#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Copy a folder tree into an eHFS image (P14 K11: make test-alpine-rust).

Usage: ehfs_tree.py EHFS_TOOLS IMAGE SRC DEST [--offset SECTORS]

Every folder under SRC is made under DEST (an eHFS path, made too) and
every regular file copied; symbolic links are followed when they point
at a file (eHFS has no links), others are skipped. IMAGE must be a
regular file under build/ (ehfs-cp refuses anything else too).
"""
import os
import subprocess
import sys


def main():
    args = sys.argv[1:]
    offset = []
    if "--offset" in args:
        i = args.index("--offset")
        offset = ["--offset", args[i + 1]]
        del args[i:i + 2]
    if len(args) != 4:
        sys.exit(__doc__)
    tools, image, src, dest = args
    if not os.path.abspath(image).startswith(os.path.abspath("build") + os.sep):
        sys.exit(f"ehfs_tree: {image}: only images under build/")
    cp = os.path.join(tools, "ehfs-cp")

    def run(*a):
        subprocess.run([cp, *offset, image, *a], check=True, stdout=subprocess.DEVNULL)

    parts = [p for p in dest.split("/") if p]
    for i in range(len(parts)):
        subprocess.run([cp, *offset, image, "mkdir", "/" + "/".join(parts[:i + 1])],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    count = 0
    for top, dirs, files in os.walk(src):
        dirs.sort()
        rel = os.path.relpath(top, src)
        inside = "/" + "/".join(parts + ([] if rel == "." else rel.split(os.sep)))
        for d in dirs:
            if os.path.islink(os.path.join(top, d)):
                continue
            run("mkdir", inside.rstrip("/") + "/" + d)
        for name in sorted(files):
            path = os.path.join(top, name)
            if not os.path.isfile(path):
                continue
            run("in", path, inside.rstrip("/") + "/" + name)
            count += 1
    print(f"ehfs_tree: {count} files into {dest}")


if __name__ == "__main__":
    main()
