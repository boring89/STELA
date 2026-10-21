#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Create a disk image that boots STELA on a real UEFI machine
(docs/p520.md).

Usage: make_disk_image.py [--setup-done] OUT ESP_DIR EHFS_TOOLS [DATA_MIB]

    OUT         the image, a regular file under build/ (made fresh)
    ESP_DIR     a directory with the files of the EFI System Partition
                (EFI/BOOT/BOOTX64.EFI, STELA/kernel.elf, STELA/initrd.tar,
                STELA/boot.cfg)
    EHFS_TOOLS  the directory with mkfs.ehfs and ehfs-cp
    DATA_MIB    size of the eHFS Data volume in MiB (default 4096)

Layout (512-byte sectors, partitions aligned to 1 MiB):

    LBA 0        protective MBR (no test-disk magic: the self test never
                 writes raw blocks on this disk)
    LBA 1..33    GPT header and entries
    1 MiB        "EFI system" (FAT32, 64 MiB): the files of ESP_DIR
    65 MiB       "Data" (eHFS): Welcome.txt
    end - 33     backup GPT entries and header

The image is sparse where nothing is written. Only regular files under
build/ are created or written; anything else (a block device, a path
outside build/) is refused, and the tools run on image files only
(mtools with -i, mkfs.ehfs and ehfs-cp on a file path). Writing the image
to a USB stick is left to the user (docs/p520.md section 2).
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from make_test_images import DATA_TYPE, ESP_TYPE, MIB, checked, run, write_gpt  # noqa: E402

ESP_MIB = 64
SECTOR = 512

WELCOME = b"""Welcome to STELA.

This is the Data volume: your programs and documents live here.
The boot options are in Boot:/STELA/boot.cfg (docs/p520.md).
"""


def copy_tree(img, src, dest="::"):
    """Copies the directory `src` into the FAT image `img` at `dest`."""
    for name in sorted(os.listdir(src)):
        path = os.path.join(src, name)
        target = dest.rstrip("/") + "/" + name
        if os.path.isdir(path):
            run("mmd", "-i", img, target)
            copy_tree(img, path, target)
        else:
            run("mcopy", "-i", img, path, target)


def main():
    # --setup-done (P12, tests only): the Data volume says Setup.app has
    # run, so the first boot goes straight to the desktop.
    args = [a for a in sys.argv[1:] if a != "--setup-done"]
    setup_done = len(args) != len(sys.argv) - 1
    if len(args) not in (3, 4):
        sys.exit(__doc__)
    out, esp_dir, tools = args[0:3]
    data_mib = int(args[3]) if len(args) == 4 else 4096
    if data_mib < 16:
        sys.exit("make_disk_image: the Data volume needs at least 16 MiB")
    for f in ("EFI/BOOT/BOOTX64.EFI", "STELA/kernel.elf", "STELA/initrd.tar"):
        if not os.path.isfile(os.path.join(esp_dir, f)):
            sys.exit(f"make_disk_image: {esp_dir}/{f} is missing (run make first)")

    img = checked(out)
    work = os.path.join(os.path.dirname(img), "disk-image-work")
    os.makedirs(work, exist_ok=True)
    if os.path.exists(img):
        os.remove(img)

    esp_first = MIB // SECTOR
    esp_last = esp_first + ESP_MIB * MIB // SECTOR - 1
    data_first = esp_last + 1
    data_last = data_first + data_mib * MIB // SECTOR - 1
    # The backup GPT: 32 sectors of entries and the header.
    size = (data_last + 1 + 33) * SECTOR
    with open(img, "wb") as f:
        f.truncate(size)
    write_gpt(img, SECTOR, size, [(ESP_TYPE, esp_first, esp_last, "EFI system"),
                                  (DATA_TYPE, data_first, data_last, "Data")], magic=b"")

    # The ESP is made as an image of its own and copied in.
    esp = checked(os.path.join(work, "esp.img"))
    if os.path.exists(esp):
        os.remove(esp)
    with open(esp, "wb") as f:
        f.truncate(ESP_MIB * MIB)
    run("mformat", "-i", esp, "-F", "-v", "STELA", "::")
    copy_tree(esp, esp_dir)
    with open(esp, "rb") as src, open(img, "r+b") as dst:
        dst.seek(esp_first * SECTOR)
        dst.write(src.read())
    os.remove(esp)

    sectors = (data_last + 1 - data_first) // 8 * 8
    run(os.path.join(tools, "mkfs.ehfs"), "-L", "Data", "--offset", str(data_first),
        "--sectors", str(sectors), img)
    welcome = os.path.join(work, "Welcome.txt")
    with open(welcome, "wb") as f:
        f.write(WELCOME)
    run(os.path.join(tools, "ehfs-cp"), "--offset", str(data_first), img, "in", welcome,
        "/Welcome.txt")
    if setup_done:
        done = os.path.join(work, "setup-done")
        with open(done, "wb") as f:
            f.write(b"1\n")
        run(os.path.join(tools, "ehfs-cp"), "--offset", str(data_first), img, "mkdir", "/.stela")
        run(os.path.join(tools, "ehfs-cp"), "--offset", str(data_first), img, "in", done,
            "/.stela/setup-done")
    print(f"make_disk_image: {out}: {size // MIB} MiB (ESP {ESP_MIB} MiB at LBA {esp_first}, "
          f"Data {data_mib} MiB at LBA {data_first})")


if __name__ == "__main__":
    main()
