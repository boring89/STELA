#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""The System image's trailer (P14 K5, docs/system.md §3; the format is
sysimage/src/lib.rs).

    sysimage.py keygen KEYFILE PUBFILE       the TEST System key pair
    sysimage.py sign IMAGE KEYFILE VERSION   writes the trailer into the
                                             last 4 KiB of IMAGE
    sysimage.py show IMAGE                   prints the trailer
    sysimage.py corrupt IMAGE                flips a byte before the trailer
                                             (make test-update)
    sysimage.py badsig IMAGE                 flips a byte of the signed part of
                                             the trailer (make test-update)

KEYFILE is a 32-byte seed in hex. The System key is not the package
key (tools/pkg.pub): a leaked package key must not be able to sign a
bootable System. `keygen` makes the TEST System key: its seed is SHA-256
of a fixed sentence, so its public half tools/system.pub (committed; the
kernel trusts it, Initrd:/certs/system.pub) and the private key every
build makes always match. Anyone can sign with it: replace it (a key
made with Ed25519PrivateKey.generate(), kept only on the releaser's
machine) before releasing. IMAGE must be a regular file under build/.
"""
import hashlib
import os
import struct
import sys

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

TEST_SEED = hashlib.sha256(b"STELA test System image signing key - replace before use").digest()

BLOCK = 4096
MAGIC = b"STELASYS"
SIGNED = 128


def checked(path):
    if not os.path.abspath(path).startswith(os.path.abspath("build") + os.sep):
        sys.exit(f"sysimage: {path}: only images under build/")
    if not os.path.isfile(path):
        sys.exit(f"sysimage: {path}: not a regular file")
    return path


def sign(image, keyfile, version):
    with open(keyfile) as f:
        key = Ed25519PrivateKey.from_private_bytes(bytes.fromhex(f.read().strip()))
    size = os.path.getsize(image)
    if size < 2 * BLOCK or size % BLOCK:
        sys.exit(f"sysimage: {image}: not a whole number of blocks")
    data_len = size - BLOCK
    h = hashlib.sha256()
    with open(image, "rb") as f:
        left = data_len
        while left:
            chunk = f.read(min(left, 1 << 20))
            h.update(chunk)
            left -= len(chunk)
    v = version.encode()
    if len(v) > 64:
        sys.exit("sysimage: the version is longer than 64 bytes")
    t = bytearray(BLOCK)
    t[0:8] = MAGIC
    t[8:12] = struct.pack("<I", 1)
    t[16:24] = struct.pack("<Q", data_len)
    t[24:56] = h.digest()
    t[56] = len(v)
    t[57:57 + len(v)] = v
    t[SIGNED:SIGNED + 64] = key.sign(bytes(t[:SIGNED]))
    with open(image, "r+b") as f:
        f.seek(data_len)
        f.write(t)
    print(f"sysimage: {image}: version {version}, {data_len} bytes signed")


def show(image):
    with open(image, "rb") as f:
        f.seek(-BLOCK, os.SEEK_END)
        t = f.read(BLOCK)
    if t[:8] != MAGIC:
        sys.exit(f"sysimage: {image}: no trailer")
    data_len = struct.unpack("<Q", t[16:24])[0]
    print(f"version {t[57:57 + t[56]].decode()}, {data_len} bytes, sha256 {t[24:56].hex()}")


def corrupt(image):
    with open(image, "r+b") as f:
        f.seek(BLOCK * 3)
        b = f.read(1)
        f.seek(BLOCK * 3)
        f.write(bytes([b[0] ^ 0xFF]))


def badsig(image):
    with open(image, "r+b") as f:
        f.seek(-BLOCK + 57, os.SEEK_END)
        b = f.read(1)
        f.seek(-BLOCK + 57, os.SEEK_END)
        f.write(bytes([b[0] ^ 0x01]))


def keygen(keyfile, pubfile):
    key = Ed25519PrivateKey.from_private_bytes(TEST_SEED)
    os.makedirs(os.path.dirname(os.path.abspath(keyfile)), exist_ok=True)
    with open(keyfile, "w") as f:
        f.write(TEST_SEED.hex() + "\n")
    raw = key.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
    with open(pubfile, "w") as f:
        f.write(raw.hex() + "\n")


def main():
    a = sys.argv[1:]
    if len(a) == 3 and a[0] == "keygen":
        keygen(a[1], a[2])
    elif len(a) == 4 and a[0] == "sign":
        sign(checked(a[1]), a[2], a[3])
    elif len(a) == 2 and a[0] == "show":
        show(a[1])
    elif len(a) == 2 and a[0] == "corrupt":
        corrupt(checked(a[1]))
    elif len(a) == 2 and a[0] == "badsig":
        badsig(checked(a[1]))
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
