#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Run a command while HTTPS servers (TLS 1.3 only) serve a folder (make
test-tls, make test-pkg; docs/tls.md §5).

Usage: with_https.py DIR CERTDIR PORT -- COMMAND...

Four servers on 127.0.0.1 (QEMU's user networking reaches them as
10.0.2.2), all with the key CERTDIR/good.key:
    PORT      good.pem     (signed by the test root, for 10.0.2.2)
    PORT + 1  wrong.pem    (for another name)
    PORT + 2  expired.pem  (out of date)
    PORT + 3  other.pem    (self-signed, not from the test root)
They stop when the command ends; its exit status is returned.
"""
import functools
import http.server
import os
import ssl
import subprocess
import sys
import threading


class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


def serve(folder, certdir, port, cert):
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_3
    context.load_cert_chain(os.path.join(certdir, cert), os.path.join(certdir, "good.key"))
    handler = functools.partial(Quiet, directory=folder)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), handler)
    server.socket = context.wrap_socket(server.socket, server_side=True)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def main():
    if len(sys.argv) < 6 or sys.argv[4] != "--":
        sys.exit(__doc__)
    folder, certdir, port = sys.argv[1], sys.argv[2], int(sys.argv[3])
    servers = [serve(folder, certdir, port + i, c)
               for i, c in enumerate(["good.pem", "wrong.pem", "expired.pem", "other.pem"])]
    try:
        code = subprocess.call(sys.argv[5:])
    finally:
        for s in servers:
            s.shutdown()
    sys.exit(code)


if __name__ == "__main__":
    main()
