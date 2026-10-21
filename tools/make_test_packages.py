#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Make the two packages `make test-pkg` installs (docs/pkg.md §5), in the
format of the ports repository (ustar, PKGINFO first):

    greet-1.0-x86_64.tar      bin/greet: the Linux test program hello
    greetdoc-1.0-x86_64.tar   share/hello.txt; depends on greet

Usage: make_test_packages.py HELLO_BINARY OUTDIR
"""
import io
import os
import sys
import tarfile


def add(t, name, data, mode=0o644):
    info = tarfile.TarInfo(name)
    info.size = len(data)
    info.mode = mode
    info.mtime = 0
    t.addfile(info, io.BytesIO(data))


def package(out, name, depends, files):
    path = os.path.join(out, f"{name}-1.0-x86_64.tar")
    with tarfile.open(path, "w", format=tarfile.USTAR_FORMAT) as t:
        info = (f"name = {name}\nversion = 1.0\ndescription = {name} (a test package)\n"
                f"license = GPL-3.0-or-later\ninstall = bin\ndepends = {' '.join(depends)}\n")
        add(t, "PKGINFO", info.encode())
        for n, data, mode in files:
            add(t, n, data, mode)


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    with open(sys.argv[1], "rb") as f:
        hello = f.read()
    out = sys.argv[2]
    os.makedirs(out, exist_ok=True)
    package(out, "greet", [], [("bin/greet", hello, 0o755)])
    package(out, "greetdoc", ["greet"], [("share/hello.txt", b"hello from a package\n", 0o644)])
    print(f"make_test_packages: {out}")


if __name__ == "__main__":
    main()
