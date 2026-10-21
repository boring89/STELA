#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Boot STELA in QEMU and check its serial output.

Usage: qemu_test.py [--timeout S] [--exit-code N] [--screendump FILE]
                    [--send-keys TRIGGER KEYS] [--type TRIGGER TEXT]
                    [--mouse TRIGGER DX DY BUTTON] [--log FILE] [--times]
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

--mouse TRIGGER DX DY BUTTON moves the mouse by (DX, DY) and clicks BUTTON
(left, right, middle or none) through QMP input-send-event when TRIGGER
appears; --qmp TRIGGER COMMAND JSON runs any QMP command (device_add,
device_del, input-send-event with a device...); --click TRIGGER X Y COUNT
moves the pointer to screen position (X, Y) (by way of the top-left
corner) and clicks the left button COUNT times. The order of firing is
--send-keys, --type, --mouse, --qmp, --click, each in the order given.
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


SHIFTED = {":": "semicolon", "_": "minus", "!": "1", "?": "slash", "\"": "apostrophe",
           "|": "backslash", ">": "dot", "<": "comma", "&": "7"}
PLAIN = {" ": "spc", "/": "slash", ".": "dot", "-": "minus", ";": "semicolon",
         ",": "comma", "=": "equal", "\n": "ret", "'": "apostrophe"}


NAMED = {"up", "down", "left", "right", "home", "end", "delete", "pgup", "pgdn", "esc"}


def text_to_keys(text):
    """QKeyCode names that type `text` on a US layout; `{up}`, `{left}`,
    `{home}`... are those keys."""
    keys = []
    text = text.replace("\\n", "\n")
    i = 0
    while i < len(text):
        if text[i] == "{" and "}" in text[i:]:
            name = text[i + 1:text.index("}", i)]
            if name in NAMED:
                keys.append(name)
                i += len(name) + 2
                continue
        c = text[i]
        i += 1
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


def move(qmp, dx, dy):
    """Moves the mouse by (dx, dy) in steps of at most 100: QEMU's PS/2
    mouse sends only a few packets (127 each at most) per event."""
    while dx or dy:
        sx = max(-100, min(100, dx))
        sy = max(-100, min(100, dy))
        qmp.execute("input-send-event", {"events": [
            {"type": "rel", "data": {"axis": "x", "value": sx}},
            {"type": "rel", "data": {"axis": "y", "value": sy}}]})
        dx -= sx
        dy -= sy
        time.sleep(0.03)


def press(qmp, button, count):
    """Clicks `button` `count` times (2: a double click)."""
    for _ in range(count):
        for down in (True, False):
            qmp.execute("input-send-event", {"events": [
                {"type": "btn", "data": {"down": down, "button": button}}]})
            time.sleep(0.05)


def mouse(path, dx, dy, button, deadline):
    """Moves the mouse by (dx, dy) and clicks `button` ("left", "right",
    "middle" or "none") with QMP input-send-event."""
    qmp = Qmp(path, deadline)
    try:
        move(qmp, dx, dy)
        time.sleep(0.1)
        if button != "none":
            press(qmp, button, 1)
    finally:
        qmp.close()


def click(path, x, y, count, deadline):
    """Moves the pointer to (x, y) on the screen (first to the top-left
    corner, where it stops) and clicks the left button `count` times."""
    qmp = Qmp(path, deadline)
    try:
        move(qmp, -4000, -4000)
        move(qmp, x, y)
        time.sleep(0.2)
        press(qmp, "left", count)
    finally:
        qmp.close()


def command(path, name, arguments, deadline):
    """Runs one QMP command (device_add, device_del, input-send-event...)."""
    qmp = Qmp(path, deadline)
    try:
        qmp.execute(name, arguments)
    finally:
        qmp.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--timeout", type=float, default=60.0)
    ap.add_argument("--exit-code", type=int)
    ap.add_argument("--screendump")
    class Ordered(argparse.Action):
        """Keeps every action option in one list, in command-line order."""

        def __call__(self, parser, namespace, values, option_string=None):
            namespace.actions.append((self.dest, values))

    ap.set_defaults(actions=[])
    ap.add_argument("--send-keys", nargs=2, action=Ordered, dest="send_keys",
                    metavar=("TRIGGER", "KEYS"))
    ap.add_argument("--type", nargs=2, action=Ordered, dest="type",
                    metavar=("TRIGGER", "TEXT"))
    ap.add_argument("--mouse", nargs=4, action=Ordered, dest="mouse",
                    metavar=("TRIGGER", "DX", "DY", "BUTTON"))
    ap.add_argument("--qmp", nargs=3, action=Ordered, dest="qmp",
                    metavar=("TRIGGER", "COMMAND", "JSON"))
    ap.add_argument("--click", nargs=4, action=Ordered, dest="click",
                    metavar=("TRIGGER", "X", "Y", "COUNT"))
    ap.add_argument("--expect", action="append", required=True)
    ap.add_argument("--log", help="also write the serial output to this file")
    ap.add_argument("--times", action="store_true",
                    help="start every logged line with the seconds since QEMU started "
                         "(make bench)")
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
    # (trigger, action) in the order given on the command line (P11:
    # before, all --send-keys came first, then --type, --mouse, --qmp,
    # --click): keys, ("mouse", dx, dy, button), ("qmp", command,
    # arguments) or ("click", x, y, count).
    typing = []
    for kind, v in args.actions:
        if kind == "send_keys":
            typing.append((v[0], v[1].split()))
        elif kind == "type":
            typing.append((v[0], text_to_keys(v[1])))
        elif kind == "mouse":
            typing.append((v[0], ("mouse", int(v[1]), int(v[2]), v[3])))
        elif kind == "qmp":
            typing.append((v[0], ("qmp", v[1], json.loads(v[2]))))
        else:
            typing.append((v[0], ("click", int(v[1]), int(v[2]), int(v[3]))))
    if typing:
        qmp = os.path.abspath(os.path.join("build", f"qmp-{os.getpid()}.sock"))
        if os.path.exists(qmp):
            os.unlink(qmp)
        cmd = cmd + ["-qmp", f"unix:{qmp},server=on,wait=off"]

    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            stdin=subprocess.DEVNULL)
    started = time.monotonic()
    deadline = started + args.timeout
    pending = list(args.expect)
    log = []
    status = None
    try:
        state = {"eof": False}
        for text in serial_lines(proc, deadline, state):
            if args.times:
                log.append(f"[{time.monotonic() - started:9.3f}] {text}")
            else:
                log.append(text)
            if qmp and typing and typing[0][0] in text:
                action = typing.pop(0)[1]
                if isinstance(action, tuple) and action[0] == "qmp":
                    command(qmp, action[1], action[2], deadline)
                elif isinstance(action, tuple) and action[0] == "click":
                    click(qmp, action[1], action[2], action[3], deadline)
                elif isinstance(action, tuple):
                    mouse(qmp, action[1], action[2], action[3], deadline)
                else:
                    send_keys(qmp, action, deadline)
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
    except (OSError, RuntimeError) as e:
        log.append(f"qemu_test: QMP failed: {e!r}")
        pending = pending or ["(QMP failure)"]
    finally:
        if monitor and os.path.exists(monitor):
            os.unlink(monitor)
        if qmp and os.path.exists(qmp):
            os.unlink(qmp)
        if status is None:
            proc.kill()
            proc.wait()

    if args.log:
        with open(args.log, "w", encoding="utf-8") as f:
            f.write("\n".join(log) + "\n")
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
