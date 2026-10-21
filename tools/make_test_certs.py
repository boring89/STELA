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

Usage: make_test_certs.py DIR
"""
import os
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
    ec = ["-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-nodes"]
    run("openssl", "req", "-x509", *ec, "-keyout", p("ca.key"), "-out", p("ca.pem"),
        "-days", "3650", "-subj", "/CN=STELA test root", "-addext",
        "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign")
    run("openssl", "x509", "-in", p("ca.pem"), "-outform", "DER", "-out", p("ca.der"))
    run("openssl", "genpkey", "-algorithm", "EC", "-pkeyopt",
        "ec_paramgen_curve:prime256v1", "-out", p("good.key"))

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
    leaf("expired", "DNS:10.0.2.2,IP:10.0.2.2",
         ("-not_before", "20200101000000Z", "-not_after", "20210101000000Z"))
    run("openssl", "req", "-x509", "-key", p("good.key"), "-out", p("other.pem"), "-days", "825",
        "-subj", "/CN=stela other", "-addext", "subjectAltName=DNS:10.0.2.2,IP:10.0.2.2")
    print(f"make_test_certs: {d}")


if __name__ == "__main__":
    main()
