#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""Turn the log of make bench into a Markdown report (docs/bench.md).

Usage: bench_report.py LOG KERNEL_ELF

LOG is qemu_test.py's --log written with --times (every line starts with
the seconds since QEMU started); KERNEL_ELF is the kernel that ran. The
report has the date, the QEMU version, the targets of docs/architecture.md
section 3 with what was measured, the boot milestones and the lines of the
bench program.
"""
import datetime
import os
import re
import subprocess
import sys

# Serial lines whose first appearance marks a stage of the boot.
MILESTONES = [
    ("loader starts", "STELA boot loader"),
    ("kernel starts", "boot: starting the kernel"),
    ("kernel ready", "STELA kernel "),
    ("window server ready", "windowserver: "),
    ("desktop (Mouseion ready)", "mouseion: ready"),
    ("shell prompt", "STELA shell"),
]

LINE = re.compile(r"^\[\s*([0-9.]+)\] (.*)$")


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    lines = []
    with open(sys.argv[1], encoding="utf-8", errors="replace") as f:
        for raw in f:
            m = LINE.match(raw.rstrip("\n"))
            if m:
                lines.append((float(m.group(1)), m.group(2)))
    first = {}
    for label, text in MILESTONES:
        for t, line in lines:
            if text in line:
                first[label] = t
                break
    bench = {}
    for _, line in lines:
        if line.startswith("bench: ") and line != "bench: done":
            key = line[len("bench: "):].split(" ", 1)[0]
            bench[key] = line[len("bench: "):]
    kernel = os.path.getsize(sys.argv[2])
    qemu = subprocess.run(["qemu-system-x86_64", "--version"], capture_output=True,
                          text=True).stdout.splitlines()
    desktop = first.get("desktop (Mouseion ready)")
    loader = first.get("loader starts")
    used = heap_idle = None
    m = re.search(r"memory used (\d+) KiB.*kernel heap (\d+) KiB used of (\d+) KiB",
                  bench.get("memory", ""))
    if m:
        used = int(m.group(1))
        # The heap's frames are taken at boot whether used or not.
        heap_idle = int(m.group(3)) - int(m.group(2))

    out = []
    out.append(f"## {datetime.date.today().isoformat()}")
    out.append("")
    out.append(f"QEMU: {qemu[0] if qemu else '?'} (TCG, no KVM; `-machine q35 -m 512M`, one "
               "processor, e1000e on user networking, AHCI disk)")
    out.append("")
    out.append("| target (docs/architecture.md §3) | goal | measured |")
    out.append("|---|---|---|")
    out.append(f"| kernel.elf | ≤ 2 MiB | {kernel / 1048576:.2f} MiB ({kernel} bytes) |")
    if desktop is not None and loader is not None:
        out.append(f"| boot to the desktop, no KVM | ≤ 5 s | {desktop - loader:.2f} s from the "
                   f"loader ({desktop:.2f} s from QEMU's start, firmware included) |")
    else:
        out.append("| boot to the desktop, no KVM | ≤ 5 s | (not seen) |")
    if used is not None:
        out.append(f"| memory on the idle desktop (all frames in use: kernel, heap, "
                   f"window server, Mouseion, terminal, shell) | ≤ 64 MiB | {used / 1024:.1f} MiB; "
                   f"{(used - heap_idle) / 1024:.1f} MiB without the heap's unused "
                   f"{heap_idle / 1024:.1f} MiB |")
    out.append("")
    out.append("| boot milestone | seconds since QEMU started |")
    out.append("|---|---|")
    for label, _ in MILESTONES:
        t = first.get(label)
        out.append(f"| {label} | {t:.2f} |" if t is not None else f"| {label} | — |")
    out.append("")
    out.append("| measurement | result |")
    out.append("|---|---|")
    for key in ("memory", "syscall", "switch", "ehfs", "tcp"):
        if key in bench:
            out.append(f"| {key} | {bench[key][len(key):].strip()} |")
        else:
            out.append(f"| {key} | (not measured) |")
    print("\n".join(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
