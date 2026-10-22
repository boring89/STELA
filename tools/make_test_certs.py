#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Make the certificates of `make test-tls` (docs/tls.md §5) with the
openssl command, in DIR (under build/):

    ca.key ca.pem ca.der      a test root (EC P-256), trusted with --ca
    good.key good.pem         for 10.0.2.2 (SAN DNS and IP), signed by ca
    wrong.pem                 for wrong.test only (host name check)
    expired.pem               for 10.0.2.2, valid 2020-2021 (date check)
    other.pem                 for 10.0.2.2, self-signed (not from ca)

ca.key, ca.pem, good.key and expired.pem are copied from tools/testcerts/
(P14 K0): an out-of-date certificate needs `openssl x509 -not_before`,
which OpenSSL 3.0 (the CI runner's) does not have. The rest is made here.

Usage: make_test_certs.py DIR
"""
import os
import shutil
import subprocess
import sys


def run(*args):
    subprocess.run(args, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    d = sys.argv[1]
    os.makedirs(d, exist_ok=True)
    p = lambda name: os.path.join(d, name)
    fixtures = os.path.join(os.path.dirname(os.path.abspath(__file__)), "testcerts")
    for name in ["ca.key", "ca.pem", "good.key", "expired.pem"]:
        shutil.copyfile(os.path.join(fixtures, name), p(name))
    run("openssl", "x509", "-in", p("ca.pem"), "-outform", "DER", "-out", p("ca.der"))

    def leaf(name, san, extra=()):
        run("openssl", "req", "-new", "-key", p("good.key"), "-subj", "/CN=stela test server",
            "-out", p(name + ".csr"))
        ext = p(name + ".ext")
        with open(ext, "w") as f:
            f.write(f"subjectAltName={san}\nbasicConstraints=CA:FALSE\n")
        run("openssl", "x509", "-req", "-in", p(name + ".csr"), "-CA", p("ca.pem"),
            "-CAkey", p("ca.key"), "-CAcreateserial", "-extfile", ext, "-out", p(name + ".pem"),
            *extra)

    leaf("good", "DNS:10.0.2.2,IP:10.0.2.2", ("-days", "825"))
    leaf("wrong", "DNS:wrong.test", ("-days", "825"))
    run("openssl", "req", "-x509", "-key", p("good.key"), "-out", p("other.pem"), "-days", "825",
        "-subj", "/CN=stela other", "-addext", "subjectAltName=DNS:10.0.2.2,IP:10.0.2.2")
    print(f"make_test_certs: {d}")


if __name__ == "__main__":
    main()
