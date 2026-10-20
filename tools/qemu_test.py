#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Boot STELA in QEMU and check its serial output.

Usage: qemu_test.py [--timeout S] [--exit-code N] [--screendump FILE]
                    [--send-keys TRIGGER KEYS]
                    --expect TEXT [--expect TEXT ...] -- <qemu command>

The expected strings must appear on the serial console in the given order.
Without --exit-code, QEMU is killed as soon as the last one is seen. With
--exit-code, QEMU must then exit by itself (the self-test kernel uses the
isa-debug-exit device) with status N. Either way the timeout applies.

With --screendump, a QEMU monitor socket is added to the command, and once
the last expected line is seen the screen is saved to FILE (PPM) before QEMU
is killed. Not combinable with --exit-code.

With --send-keys, a QMP socket is added to the command, and when a serial
line containing TRIGGER appears, the space-separated QEMU key names in KEYS
(QKeyCode names: "s", "ret", "shift", ...; "shift-a" presses both) are
typed one after another with the QMP command send-key. The keyboard self
test uses this. --type TRIGGER TEXT does the same with plain text ("\n" is
Enter). Both may be given several times: each fires once, in the order
given, when its trigger appears after the previous one fired.
"""
import argparse
import json
import os
import select
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


def serial_lines(proc, deadline, state):
    """Yields QEMU's output line by line until it exits or the deadline
    passes, without blocking past the deadline (a hung guest prints
    nothing). Sets state["eof"] when QEMU closed its output."""
    buffer = b""
    fd = proc.stdout.fileno()
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return
        ready, _, _ = select.select([fd], [], [], remaining)
        if not ready:
            return
        chunk = os.read(fd, 4096)
        if not chunk:
            state["eof"] = True
            if buffer:
                yield buffer.decode("utf-8", "replace").rstrip("\r")
            return
        buffer += chunk
        while b"\n" in buffer:
            line, buffer = buffer.split(b"\n", 1)
            yield line.decode("utf-8", "replace").rstrip("\r")


class Qmp:
    """A minimal QMP client (QEMU Machine Protocol, docs/interop/qmp-spec)."""

    def __init__(self, path, deadline):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(max(1.0, deadline - time.monotonic()))
        self.sock.connect(path)
        self.buffer = b""
        self.reply()  # the greeting
        self.execute("qmp_capabilities")

    def reply(self):
        """The next message that is not an asynchronous event."""
        while True:
            while b"\n" not in self.buffer:
                chunk = self.sock.recv(4096)
                if not chunk:
                    raise ConnectionError("QMP connection closed")
                self.buffer += chunk
            line, self.buffer = self.buffer.split(b"\n", 1)
            message = json.loads(line)
            if "event" not in message:
                return message

    def execute(self, command, arguments=None):
        request = {"execute": command}
        if arguments is not None:
            request["arguments"] = arguments
        self.sock.sendall(json.dumps(request).encode() + b"\n")
        message = self.reply()
        if "error" in message:
            raise RuntimeError(f"QMP {command}: {message['error']}")
        return message.get("return")

    def close(self):
        self.sock.close()


SHIFTED = {":": "semicolon", "_": "minus", "!": "1", "?": "slash", "\"": "apostrophe"}
PLAIN = {" ": "spc", "/": "slash", ".": "dot", "-": "minus", ";": "semicolon",
         ",": "comma", "=": "equal", "\n": "ret", "'": "apostrophe"}


def text_to_keys(text):
    """QKeyCode names that type `text` on a US layout."""
    keys = []
    for c in text.replace("\\n", "\n"):
        if c.islower() or c.isdigit():
            keys.append(c)
        elif c.isupper():
            keys.append("shift-" + c.lower())
        elif c in PLAIN:
            keys.append(PLAIN[c])
        elif c in SHIFTED:
            keys.append("shift-" + SHIFTED[c])
        else:
            raise ValueError(f"cannot type {c!r}")
    return keys


def send_keys(path, keys, deadline):
    """Types `keys` (QKeyCode names, "shift-x" for two at once), one
    press-and-release at a time."""
    qmp = Qmp(path, deadline)
    try:
        for key in keys:
            qmp.execute("send-key", {"keys": [{"type": "qcode", "data": k}
                                              for k in key.split("-") if k],
                                     "hold-time": 50})
            # Let the release happen before the next press.
            time.sleep(0.15)
    finally:
        qmp.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--timeout", type=float, default=60.0)
    ap.add_argument("--exit-code", type=int)
    ap.add_argument("--screendump")
    ap.add_argument("--send-keys", nargs=2, action="append", default=[],
                    metavar=("TRIGGER", "KEYS"))
    ap.add_argument("--type", nargs=2, action="append", default=[],
                    metavar=("TRIGGER", "TEXT"))
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

    qmp = None
    # (trigger, keys) in firing order: --send-keys first, then --type.
    typing = [(t, k.split()) for t, k in args.send_keys]
    typing += [(t, text_to_keys(x)) for t, x in args.type]
    if typing:
        qmp = os.path.abspath(os.path.join("build", f"qmp-{os.getpid()}.sock"))
        if os.path.exists(qmp):
            os.unlink(qmp)
        cmd = cmd + ["-qmp", f"unix:{qmp},server=on,wait=off"]

    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            stdin=subprocess.DEVNULL)
    deadline = time.monotonic() + args.timeout
    pending = list(args.expect)
    log = []
    status = None
    try:
        state = {"eof": False}
        for text in serial_lines(proc, deadline, state):
            log.append(text)
            if qmp and typing and typing[0][0] in text:
                send_keys(qmp, typing.pop(0)[1], deadline)
            if pending and pending[0] in text:
                pending.pop(0)
            if not pending and args.exit_code is None:
                break
        else:
            if state["eof"]:
                try:
                    status = proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    pass
        if not pending and monitor:
            screendump(monitor, os.path.abspath(args.screendump), deadline)
    finally:
        if monitor and os.path.exists(monitor):
            os.unlink(monitor)
        if qmp and os.path.exists(qmp):
            os.unlink(qmp)
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
