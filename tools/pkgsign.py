#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Sign STELA package indexes with ed25519 (docs/pkg.md §3).

    pkgsign.py keygen KEYFILE PUBFILE   the TEST key pair (see below)
    pkgsign.py sign KEYFILE INDEX       writes INDEX.sig (hex signature)
    pkgsign.py verify PUBFILE INDEX     exit 0 when INDEX.sig is PUBFILE's
                                        and INDEX lists exactly the *.tar
                                        next to it, with their SHA-256
    pkgsign.py index KEYFILE DIR URL    index.json (+ .sig) for DIR/*.tar,
                                        each at URL + file name

KEYFILE holds the 32-byte private seed in hex; PUBFILE gets the public
key in hex (what goes to Initrd:/certs/pkg.pub).

THE TEST KEY: `keygen` does not make a random key: its seed is SHA-256 of
a fixed sentence, so the public key in tools/pkg.pub (committed) and the
private key every build makes (build/pkg-signing.key) always match, and
`make test-pkg` works in CI. Anyone can sign with it, so it must be
replaced by a real key (made with the `cryptography` module's
Ed25519PrivateKey.generate(), kept off the repository) before packages
are served to anyone.
"""
import hashlib
import json
import os
import sys
import tarfile

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives import serialization

TEST_SEED = hashlib.sha256(b"STELA test package signing key - replace before use").digest()


def load_key(path):
    with open(path) as f:
        return Ed25519PrivateKey.from_private_bytes(bytes.fromhex(f.read().strip()))


def public_hex(key):
    raw = key.public_key().public_bytes(serialization.Encoding.Raw,
                                        serialization.PublicFormat.Raw)
    return raw.hex()


def sign(keyfile, index):
    key = load_key(keyfile)
    with open(index, "rb") as f:
        sig = key.sign(f.read())
    with open(index + ".sig", "w") as f:
        f.write(sig.hex() + "\n")


def verify(pubfile, index):
    from cryptography.exceptions import InvalidSignature
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
    try:
        with open(pubfile) as f:
            key = Ed25519PublicKey.from_public_bytes(bytes.fromhex(f.read().strip()))
        with open(index + ".sig") as f:
            sig = bytes.fromhex(f.read().strip())
        with open(index, "rb") as f:
            data = f.read()
        key.verify(sig, data)
        listed = {p["file"]: p["sha256"] for p in json.loads(data)["packages"]}
    except (OSError, ValueError, KeyError, InvalidSignature):
        return False
    folder = os.path.dirname(os.path.abspath(index))
    tars = {n for n in os.listdir(folder) if n.endswith(".tar")}
    if tars != set(listed):
        return False
    for name, digest in listed.items():
        h = hashlib.sha256()
        with open(os.path.join(folder, name), "rb") as f:
            for chunk in iter(lambda: f.read(1 << 20), b""):
                h.update(chunk)
        if h.hexdigest() != digest:
            return False
    return True


def pkginfo(path):
    with tarfile.open(path) as t:
        member = t.getmember("PKGINFO")
        text = t.extractfile(member).read().decode()
    info = {}
    for line in text.splitlines():
        if "=" in line:
            k, v = line.split("=", 1)
            info[k.strip()] = v.strip()
    return info


def index(keyfile, folder, url):
    packages = []
    for name in sorted(os.listdir(folder)):
        if not name.endswith(".tar"):
            continue
        path = os.path.join(folder, name)
        info = pkginfo(path)
        with open(path, "rb") as f:
            data = f.read()
        packages.append({
            "name": info["name"], "version": info["version"],
            "description": info.get("description", ""), "license": info.get("license", ""),
            "install": info.get("install", "bin"),
            "depends": info.get("depends", "").split(),
            "size": len(data), "sha256": hashlib.sha256(data).hexdigest(),
            "file": name, "url": url + name,
        })
    out = os.path.join(folder, "index.json")
    with open(out, "w") as f:
        json.dump({"format": 1, "packages": packages}, f, indent=1, sort_keys=True)
        f.write("\n")
    sign(keyfile, out)
    print(f"pkgsign: {len(packages)} packages in {out}, signed")


def main():
    a = sys.argv[1:]
    if a[:1] == ["keygen"] and len(a) == 3:
        key = Ed25519PrivateKey.from_private_bytes(TEST_SEED)
        os.makedirs(os.path.dirname(os.path.abspath(a[1])), exist_ok=True)
        with open(a[1], "w") as f:
            f.write(TEST_SEED.hex() + "\n")
        with open(a[2], "w") as f:
            f.write(public_hex(key) + "\n")
    elif a[:1] == ["sign"] and len(a) == 3:
        sign(a[1], a[2])
    elif a[:1] == ["verify"] and len(a) == 3:
        sys.exit(0 if verify(a[1], a[2]) else 1)
    elif a[:1] == ["index"] and len(a) == 4:
        index(a[1], a[2], a[3])
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
