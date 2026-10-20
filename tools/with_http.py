#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Run a command while an HTTP server serves a folder (make test-net).

Usage: with_http.py DIR PORT -- COMMAND...

The server listens on 127.0.0.1:PORT (QEMU's user networking reaches it
as 10.0.2.2:PORT) and stops when the command ends; the command's exit
status is returned. HOST_DNS=yes is put in the command's environment when
this machine can resolve example.com (the DNS check of the test is then
done; without a network it is skipped).
"""
import functools
import http.server
import os
import socket
import subprocess
import sys
import threading


class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


def main():
    if len(sys.argv) < 5 or sys.argv[3] != "--":
        sys.exit(__doc__)
    folder, port = sys.argv[1], int(sys.argv[2])
    handler = functools.partial(Quiet, directory=folder)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    env = dict(os.environ)
    try:
        socket.setdefaulttimeout(3)
        socket.gethostbyname("example.com")
        env["HOST_DNS"] = "yes"
        print("with_http: this machine resolves names: the DNS check runs")
    except OSError:
        env.pop("HOST_DNS", None)
        print("with_http: no DNS on this machine: the DNS check is skipped")
    try:
        code = subprocess.call(sys.argv[4:], env=env)
    finally:
        server.shutdown()
    sys.exit(code)


if __name__ == "__main__":
    main()
