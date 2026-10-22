#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""RELEASE_NOTES.md for `make release` (P14 K17, docs/release.md).

    release_notes.py VERSION OUT [SHA256SUMS]

Made from the repository, nothing typed by hand but docs/release.md's
"已知限制" section: a warning that everything was verified only in QEMU,
the components (CLAUDE.md's collaboration list, which says which were
reviewed), what each design document is about (its title and first
paragraph), the known limits, and the files with their SHA-256.
"""
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WARNING = (
    "> **Only verified in QEMU** (with OVMF, TCG). Nothing in this release has been "
    "tested on real hardware: drivers for real machines (I219, AX88179, HDA codecs, "
    "USB sticks, HiDPI panels) are unverified, and the System image and packages are "
    "signed with **test keys** that anyone can sign with. Do not install it on a disk "
    "whose contents you need.\n"
)


def read(path):
    with open(os.path.join(ROOT, path), encoding="utf-8") as f:
        return f.read()


def first_paragraph(text):
    """The document's title and its first paragraph of prose (not a quote,
    a heading, a list, a table or code)."""
    title = None
    para = []
    in_code = False
    for line in text.splitlines():
        if line.startswith("```"):
            in_code = not in_code
            if para:
                break
            continue
        if in_code:
            continue
        if title is None and line.startswith("# "):
            title = line[2:].strip()
            continue
        if not line.strip():
            if para:
                break
            continue
        if line.startswith((">", "#", "|", "-", "*", "1.", "    ")):
            if para:
                break
            continue
        para.append(line.strip())
    return title or "", " ".join(para)


def collaboration_list():
    lines = read("CLAUDE.md").splitlines()
    out = []
    taking = False
    for line in lines:
        if line.startswith("協作模式的元件"):
            taking = True
            continue
        if taking:
            if line.startswith("- "):
                out.append(line)
            elif line.strip() and not line.startswith("  "):
                break
    return out


def section(text, heading):
    """The body of `## heading` (up to the next `## `)."""
    m = re.search(r"^## " + re.escape(heading) + r"[^\n]*\n(.*?)(?=^## |\Z)", text, re.M | re.S)
    return m.group(1).strip() if m else ""


def main():
    if len(sys.argv) not in (3, 4):
        sys.exit(__doc__)
    version, out = sys.argv[1], sys.argv[2]
    parts = [f"# STELA {version}\n", WARNING]
    parts.append("## Components\n\nFrom `CLAUDE.md` (\"使用者已審核\" = reviewed by the author; "
                 "\"使用者尚未審核\" = not yet).\n")
    parts.append("\n".join(collaboration_list()) + "\n")
    parts.append("## Design documents\n\nThe first paragraph of each document in `docs/`.\n")
    for name in sorted(os.listdir(os.path.join(ROOT, "docs"))):
        if not name.endswith(".md") or name == "release.md":
            continue
        title, para = first_paragraph(read(os.path.join("docs", name)))
        parts.append(f"- **{title}** (`docs/{name}`): {para}" if para else f"- **{title}** (`docs/{name}`)")
    limits = section(read("docs/release.md"), "已知限制")
    if not limits:
        sys.exit("release_notes: docs/release.md has no 已知限制 section")
    parts.append("\n## Known limits\n\n" + limits + "\n")
    if len(sys.argv) == 4:
        with open(sys.argv[3], encoding="utf-8") as f:
            sums = f.read().strip()
        parts.append("## Files (SHA-256)\n\n```\n" + sums + "\n```\n")
    with open(out, "w", encoding="utf-8") as f:
        f.write("\n".join(parts))
    print(f"release_notes: {out}")


if __name__ == "__main__":
    main()
