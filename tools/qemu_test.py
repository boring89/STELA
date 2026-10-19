#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Boot STELA in QEMU and check its serial output.

Usage: qemu_test.py [--timeout S] [--exit-code N] [--screendump FILE]
                    --expect TEXT [--expect TEXT ...] -- <qemu command>

The expected strings must appear on the serial console in the given order.
Without --exit-code, QEMU is killed as soon as the last one is seen. With
--exit-code, QEMU must then exit by itself (the self-test kernel uses the
isa-debug-exit device) with status N. Either way the timeout applies.

With --screendump, a QEMU monitor socket is added to the command, and once
the last expected line is seen the screen is saved to FILE (PPM) before QEMU
is killed. Not combinable with --exit-code.
"""
import argparse
import os
import socket
import subprocess
import sys
import time


def screendump(monitor, path, deadline):
    """Ask QEMU's human monitor to save the screen, and wait for the file."""
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        sock.connect(monitor)
        sock.settimeout(max(1.0, deadline - time.monotonic()))
        sock.sendall(f"screendump {path}\n".encode())
        # The command completes before the monitor prints its next prompt.
        reply = b""
        while reply.count(b"(qemu)") < 2:
            chunk = sock.recv(4096)
            if not chunk:
                break
            reply += chunk


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--timeout", type=float, default=60.0)
    ap.add_argument("--exit-code", type=int)
    ap.add_argument("--screendump")
    ap.add_argument("--expect", action="append", required=True)
    ap.add_argument("qemu", nargs=argparse.REMAINDER)
    args = ap.parse_args()
    cmd = args.qemu[1:] if args.qemu and args.qemu[0] == "--" else args.qemu
    monitor = None
    if args.screendump:
        if args.exit_code is not None:
            ap.error("--screendump and --exit-code exclude each other")
        monitor = os.path.abspath(args.screendump) + ".sock"
        if os.path.exists(monitor):
            os.unlink(monitor)
        if os.path.exists(args.screendump):
            os.unlink(args.screendump)
        cmd = cmd + ["-monitor", f"unix:{monitor},server=on,wait=off"]

    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            stdin=subprocess.DEVNULL)
    deadline = time.monotonic() + args.timeout
    pending = list(args.expect)
    log = []
    status = None
    try:
        while time.monotonic() < deadline:
            if not pending and args.exit_code is None:
                break
            line = proc.stdout.readline()
            if not line:
                status = proc.wait()
                break
            text = line.decode("utf-8", "replace").rstrip("\r\n")
            log.append(text)
            if pending and pending[0] in text:
                pending.pop(0)
        if not pending and monitor:
            screendump(monitor, os.path.abspath(args.screendump), deadline)
    finally:
        if monitor and os.path.exists(monitor):
            os.unlink(monitor)
        if status is None:
            proc.kill()
            proc.wait()

    if pending:
        print("\n".join(log))
        print(f"qemu_test: missing expected output: {pending[0]!r}", file=sys.stderr)
        return 1
    if args.screendump and not os.path.exists(args.screendump):
        print(f"qemu_test: no screendump written to {args.screendump}", file=sys.stderr)
        return 1
    if args.exit_code is not None and status != args.exit_code:
        print("\n".join(log))
        print(f"qemu_test: QEMU exit status {status}, expected {args.exit_code}",
              file=sys.stderr)
        return 1
    print(f"qemu_test: OK ({len(args.expect)} expected lines seen"
          + (f", exit status {status})" if args.exit_code is not None else ")"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
