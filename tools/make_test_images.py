#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Create the disk images the host tests and the storage test use.

Usage: make_test_images.py host DIR
           fat-test.img   FAT32 file system (mtools) with known files
           gpt-test.img   GPT made by sgdisk: ESP + "Data"
       make_test_images.py storage DIR EHFS_TOOLS
           ahci.img       128 MiB, 512-byte sectors: GPT with an ESP (FAT32,
                          the files of fat-test.img) and an eHFS "Data"
           nvme.img       64 MiB, 4096-byte sectors: GPT with an eHFS
                          "Archive"
       Both start with the test-disk magic "STELATEST" (in the protective
       MBR's boot code area), which the self test looks for.

Only regular files under build/ are created or written; anything else
(a block device, a path outside build/) is refused. The tools run on the
image files only (mtools with -i, sgdisk on a file path).
"""
import os
import stat
import struct
import subprocess
import sys
import uuid
import zlib

MIB = 1 << 20


def checked(path):
    """The real path of `path`, which must be (or become) a regular file
    under build/."""
    real = os.path.realpath(path)
    build = os.path.realpath("build")
    if not real.startswith(build + os.sep):
        sys.exit(f"make_test_images: {path} is not under build/; refused")
    if os.path.exists(real) and not stat.S_ISREG(os.stat(real).st_mode):
        sys.exit(f"make_test_images: {path} is not a regular file; refused")
    return real


def run(*args):
    subprocess.run(args, check=True, stdout=subprocess.DEVNULL)


def pattern(size):
    """Bytes that differ in every position modulo 251, so misplaced
    clusters show."""
    return bytes((i * 7 + i // 4096) % 251 for i in range(size))


def make_fat(path, size_mib, label):
    """A FAT32 image holding the files the tests read."""
    img = checked(path)
    if os.path.exists(img):
        os.remove(img)
    with open(img, "wb") as f:
        f.truncate(size_mib * MIB)
    run("mformat", "-i", img, "-F", "-v", label, "::")
    work = os.path.join(os.path.dirname(img), "fat-files")
    os.makedirs(work, exist_ok=True)
    files = {
        "HELLO.TXT": b"hello from FAT\n",
        "A long file name.txt": b"long names work\n",
        "big.bin": pattern(300 * 1024),
    }
    for name, data in files.items():
        with open(os.path.join(work, name), "wb") as f:
            f.write(data)
        run("mcopy", "-i", img, os.path.join(work, name), "::/" + name)
    run("mmd", "-i", img, "::/EFI")
    run("mmd", "-i", img, "::/EFI/BOOT")
    run("mcopy", "-i", img, os.path.join(work, "HELLO.TXT"), "::/EFI/BOOT/nested.txt")


def make_gpt(path, size_mib):
    img = checked(path)
    if os.path.exists(img):
        os.remove(img)
    with open(img, "wb") as f:
        f.truncate(size_mib * MIB)
    run("sgdisk", "-n", "1:2048:+16M", "-t", "1:EF00", "-c", "1:EFI system",
        "-n", "2:0:0", "-t", "2:8300", "-c", "2:Data", img)


ESP_TYPE = uuid.UUID("C12A7328-F81F-11D2-BA4B-00A0C93EC93B")
DATA_TYPE = uuid.UUID("0FC63DAF-8483-4772-8E79-3D69D8477DE4")


def write_gpt(img, sector, size, parts, magic=b"STELATEST"):
    """A GPT (UEFI 2.10 section 5.3) on the image file, with `sector`-byte
    sectors: protective MBR, primary header and entries, backup entries
    and header. `parts` is a list of (type UUID, first LBA, last LBA,
    name). `magic` goes at the start of the MBR's boot code area (the
    self test writes raw blocks only on disks that carry the test-disk
    magic; a real disk image passes b"")."""
    sectors = size // sector
    entries = bytearray(128 * 128)
    for i, (kind, first, last, name) in enumerate(parts):
        e = struct.pack("<16s16sQQQ72s", kind.bytes_le, uuid.uuid4().bytes_le, first, last, 0,
                        name.encode("utf-16-le"))
        entries[128 * i:128 * (i + 1)] = e
    entry_crc = zlib.crc32(entries) & 0xFFFFFFFF
    entry_sectors = len(entries) // sector + (len(entries) % sector != 0)
    disk_guid = uuid.uuid4().bytes_le

    def header(my, alternate, entries_lba):
        h = bytearray(struct.pack("<8sIIIIQQQQ16sQIII", b"EFI PART", 0x10000, 92, 0, 0, my, alternate,
                                  2 + entry_sectors, sectors - 2 - entry_sectors, disk_guid, entries_lba,
                                  128, 128, entry_crc))
        h[16:20] = struct.pack("<I", zlib.crc32(h) & 0xFFFFFFFF)
        return bytes(h) + bytes(sector - 92)

    with open(img, "r+b") as f:
        mbr = bytearray(sector)
        mbr[0:len(magic)] = magic
        # One protective partition of type 0xEE over the whole disk.
        mbr[446:462] = struct.pack("<BBBBBBBBII", 0, 0, 2, 0, 0xEE, 0xFF, 0xFF, 0xFF, 1,
                                   min(sectors - 1, 0xFFFFFFFF))
        mbr[510:512] = b"\x55\xAA"
        f.seek(0)
        f.write(mbr)
        f.seek(sector)
        f.write(header(1, sectors - 1, 2))
        f.seek(2 * sector)
        f.write(entries)
        f.seek((sectors - 1 - entry_sectors) * sector)
        f.write(entries)
        f.seek((sectors - 1) * sector)
        f.write(header(sectors - 1, 1, sectors - 1 - entry_sectors))


def make_storage(out, tools):
    """The two disks of make test-storage."""
    mkfs = os.path.join(tools, "mkfs.ehfs")
    cp = os.path.join(tools, "ehfs-cp")
    work = os.path.dirname(checked(os.path.join(out, "x")))
    os.makedirs(work, exist_ok=True)

    # AHCI: 128 MiB, 512-byte sectors; ESP 40 MiB at 1 MiB; Data after it.
    ahci = checked(os.path.join(out, "ahci.img"))
    size = 128 * MIB
    with open(ahci, "wb") as f:
        f.truncate(size)
    esp_first, esp_last = 2048, 2048 + 40 * 2048 - 1
    data_first, data_last = esp_last + 1, size // 512 - 34 - 1
    write_gpt(ahci, 512, size, [(ESP_TYPE, esp_first, esp_last, "EFI system"),
                                (DATA_TYPE, data_first, data_last, "Data")])
    fat = checked(os.path.join(out, "esp.img"))
    make_fat(fat, 40, "BOOT")
    with open(fat, "rb") as src, open(ahci, "r+b") as dst:
        dst.seek(esp_first * 512)
        dst.write(src.read())
    sectors = (data_last + 1 - data_first) // 8 * 8
    run(mkfs, "-L", "Data", "--offset", str(data_first), "--sectors", str(sectors), ahci)
    welcome = os.path.join(work, "welcome.txt")
    with open(welcome, "wb") as f:
        f.write(b"Welcome to STELA's Data volume.\n")
    run(cp, "--offset", str(data_first), ahci, "in", welcome, "/welcome.txt")

    # NVMe: 64 MiB, 4096-byte sectors; one eHFS partition at 1 MiB.
    nvme = checked(os.path.join(out, "nvme.img"))
    size = 64 * MIB
    with open(nvme, "wb") as f:
        f.truncate(size)
    first, last = 256, size // 4096 - 6 - 1
    write_gpt(nvme, 4096, size, [(DATA_TYPE, first, last, "Archive")])
    run(mkfs, "-L", "Archive", "--offset", str(first * 8), "--sectors", str((last + 1 - first) * 8), nvme)


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "host":
        out = sys.argv[2]
        os.makedirs(os.path.dirname(checked(os.path.join(out, "x"))), exist_ok=True)
        make_fat(os.path.join(out, "fat-test.img"), 64, "BOOT")
        make_gpt(os.path.join(out, "gpt-test.img"), 64)
    elif len(sys.argv) == 4 and sys.argv[1] == "storage":
        make_storage(sys.argv[2], sys.argv[3])
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
