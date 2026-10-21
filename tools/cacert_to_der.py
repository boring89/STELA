#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Turn a PEM bundle of certificates into DER certificates put one after
another (what libstela's tls::Roots reads; docs/tls.md §3).

Usage: cacert_to_der.py IN.pem OUT.der

Every "-----BEGIN CERTIFICATE-----" block is base64-decoded; nothing else
in the file (comments, the curl header) is kept.
"""
import base64
import sys


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    out = bytearray()
    count = 0
    block = None
    with open(sys.argv[1], encoding="ascii", errors="replace") as f:
        for line in f:
            line = line.strip()
            if line == "-----BEGIN CERTIFICATE-----":
                block = []
            elif line == "-----END CERTIFICATE-----" and block is not None:
                der = base64.b64decode("".join(block))
                if not der or der[0] != 0x30:
                    sys.exit(f"cacert_to_der: certificate {count + 1} is not DER")
                out += der
                count += 1
                block = None
            elif block is not None:
                block.append(line)
    if count == 0:
        sys.exit("cacert_to_der: no certificates")
    with open(sys.argv[2], "wb") as f:
        f.write(out)
    print(f"cacert_to_der: {count} certificates, {len(out)} bytes")


if __name__ == "__main__":
    main()
