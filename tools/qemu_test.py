#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Boot STELA in QEMU and check its serial output.

Usage: qemu_test.py [--timeout S] --expect TEXT [--expect TEXT ...] -- <qemu command>

The expected strings must appear on the serial console in the given order.
QEMU is killed as soon as the last one is seen, or after the timeout.
"""
import argparse
import subprocess
import sys
import time


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--timeout", type=float, default=60.0)
    ap.add_argument("--expect", action="append", required=True)
    ap.add_argument("qemu", nargs=argparse.REMAINDER)
    args = ap.parse_args()
    cmd = args.qemu[1:] if args.qemu and args.qemu[0] == "--" else args.qemu

    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            stdin=subprocess.DEVNULL)
    deadline = time.monotonic() + args.timeout
    pending = list(args.expect)
    log = []
    try:
        while pending and time.monotonic() < deadline:
            line = proc.stdout.readline()
            if not line:
                break
            text = line.decode("utf-8", "replace").rstrip("\r\n")
            log.append(text)
            if pending[0] in text:
                pending.pop(0)
    finally:
        proc.kill()
        proc.wait()

    if pending:
        print("\n".join(log))
        print(f"qemu_test: missing expected output: {pending[0]!r}", file=sys.stderr)
        return 1
    print(f"qemu_test: OK ({len(args.expect)} expected lines seen)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
