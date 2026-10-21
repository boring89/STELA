# STELA

STELA is an operating system written from scratch in Rust for x86-64 PCs with UEFI firmware.
It inherits the design philosophy of the classic Macintosh — simplicity, directness, a spatial
file manager — and adds what a modern operating system needs. It depends on no Apple component;
a classic Macintosh environment (the user's own ROM and System running on a 68k interpreter)
is an optional, isolated compatibility layer, next to a Linux compatibility layer.

STELA is the successor of [68k_HAL](https://github.com/boring89/68k_HAL) (C, archived at v1.0),
whose experience it reuses and whose code it does not — except the Classic environment, which
is a reduced 68k_HAL by definition (see [classic/README.md](classic/README.md)).

## Status

Early (phase P11 of [the plan](docs/architecture.md); P11 is not reviewed yet): STELA's own UEFI boot loader loads the
kernel ELF and an initial RAM disk (a ustar archive of user programs), builds its page tables
(higher-half kernel and a direct map of physical memory) and hands over a boot information
block. The kernel sets up its GDT/TSS and exception handlers, a bitmap physical frame
allocator, its page tables and a heap, and prints to the serial port and to a framebuffer text
console (Spleen font); a kernel panic shows a panic screen. It reads the ACPI MADT, masks the
legacy 8259s and runs on the local and I/O APICs, with a per-CPU area, a 1000 Hz APIC timer
calibrated against the PIT, a monotonic clock (TSC when invariant) and a PS/2 keyboard feeding
an input event queue. It has kernel threads with a preemptive round-robin scheduler (10 ms time
slices, sleeping, blocking), processes with their own address spaces, ring 3 user mode entered
through `iretq` and left through `syscall`/`sysret`, and a small user runtime (libstela).
Processes hold capabilities: handles with rights in a per-process table, to ports (message
queues carrying small messages and handles) and shared memory; output goes through a console
service and keyboard events arrive as messages. It enumerates PCI (ECAM, MSI/MSI-X), drives
AHCI and NVMe disks, reads GPT and FAT, and has its own file system, eHFS (journaled B-trees,
persistent file ids, case-insensitive Unicode names; on-disk compatible with 68k_HAL's eHFS),
with volumes as separate roots (`Data:/docs/a.txt`) reached through folder handles; host tools
`mkfs.ehfs`, `fsck.ehfs`, `ehfs-cp` and `ehfs-ls` work on image files. Programs are started
from files through process handles (spawn, wait, kill, arguments) and have a heap. `make run`
boots into `init`, which starts the shell: `help` lists its commands (`ls`, `cat`, `cp`, `mv`,
`mkdir`, `rm`, `cd`, `pwd`, `run`, `ps`, `mem`, `uptime`, `vols`, `shutdown`, `reboot`); a
faulting program is killed and the system goes on. The date comes from the CMOS real-time
clock; `shutdown` unmounts the volumes and turns the machine off through ACPI (FADT and the
DSDT's `\_S5`). Static x86-64 Linux programs (musl) run with a Linux personality: Linux
system calls are translated onto STELA's objects (descriptors, a synthetic root over the volumes
with `/tmp` and `/dev`, ttys with a line discipline, brk and anonymous mmap, TLS). `make run`
boots into a desktop: a window server in user space (dirty-rectangle compositing on the
write-combining framebuffer, menu bar, window frames, pointer, focus) with a terminal window
running the shell, and a small UI toolbox (drawing, windows, events, menus, controls). Input
comes from PS/2 and from USB (xHCI, hubs, HID boot keyboards and mice, hot plugging). The
desktop belongs to Mouseion, a spatial file manager: one icon per volume, one window per
folder that remembers its place (in eHFS's Finder information), double clicks to open folders,
run `.app` bundles and view text, drag and drop to move or copy. The boot screen shows a stela
and a kernel panic a cracked one (all icons are drafts for now). P10 adds: a bootable disk
image for real machines with boot options in `STELA/boot.cfg`; the Classic environment
(`Classic.app`: Musashi and 68k_HAL's Macintosh IIci model in a window, booting the user's own
ROM and System 7 to the Finder); networking (e1000, e1000e/I219 and virtio-net drivers, a
protocol stack with DHCP, ARP, ICMP, UDP and TCP, socket system calls and BSD sockets for Linux
programs, `ifconfig`, `ping`, `fetch`); sound output (Intel HD Audio, `tone`); and SMP (the
application processors start and run processes, each process on one processor). P11 adds:
waiting on several objects at once (wait sets; Linux `poll`/`select`), capabilities for power,
input and the network, pipes with shell pipelines, redirections, background jobs, line editing
and history, ANSI colours and scrollback in the terminal, extended attributes, thread handles
and join; Mouseion renames, deletes and selects several items and reopens its windows; the
Classic environment gets the Data volume as a Mac disk, a NuBus video card, integer scaling, a
second disk and `classic.cfg`; faster storage (several commands at once, read-ahead) and TCP
(RTT, congestion control, fast retransmit); a hardware report (`hwinfo`), the boot options
`verbose` and `waitkey`, `make bench`; and everyday programs: a clipboard, dialogs and a
multi-line text view in the toolbox, command-line tools in `Initrd:/bin/` (`grep`, `sort`,
`find`, `tar`...), a text editor (Kalamos), a system monitor (Monitor) and a small HTTP-only web
browser (Periplus) — working names. Later in P11: text beyond ASCII (UTF-8 drawn with the
Fusion Pixel 12px font, Chinese characters taking two cells), a local time zone, USB sticks
and disks (Bulk-Only mass storage, MBR, FAT writing; volumes appear and disappear while
running), a trash, zoom box, list sorting and menu shortcuts on the desktop, Tab completion and
`*`/`?` wildcards in the shell, a fuzzer for the parsers (`make test-fuzz`) and a stress test
(`make test-stress`). See
[docs/boot.md](docs/boot.md), [docs/frame.md](docs/frame.md), [docs/irq.md](docs/irq.md),
[docs/time.md](docs/time.md), [docs/keyboard.md](docs/keyboard.md),
[docs/process.md](docs/process.md), [docs/syscall.md](docs/syscall.md),
[docs/ipc.md](docs/ipc.md), [docs/pci.md](docs/pci.md), [docs/storage.md](docs/storage.md),
[docs/ehfs.md](docs/ehfs.md), [docs/vfs.md](docs/vfs.md), [docs/userland.md](docs/userland.md),
[docs/acpi.md](docs/acpi.md), [docs/linux.md](docs/linux.md), [docs/display.md](docs/display.md),
[docs/window.md](docs/window.md), [docs/toolbox.md](docs/toolbox.md), [docs/usb.md](docs/usb.md),
[docs/mouseion.md](docs/mouseion.md), [docs/identity.md](docs/identity.md),
[docs/p520.md](docs/p520.md), [docs/classic.md](docs/classic.md), [docs/net.md](docs/net.md),
[docs/sound.md](docs/sound.md), [docs/smp.md](docs/smp.md), [docs/hwinfo.md](docs/hwinfo.md),
[docs/bench.md](docs/bench.md), [docs/apps.md](docs/apps.md) and
[docs/testing.md](docs/testing.md).

## Building

Requirements: `rustup` (the pinned toolchain and targets are installed automatically from
`rust-toolchain.toml`), `qemu-system-x86`, `ovmf`, `python3`, `make`, GNU `tar`, `mtools`,
`gdisk` (the last two make test disk images), `gcc` (builds musl, MIT, for the Linux test
programs, and the Classic environment's C code; the musl tarball is copied, not downloaded: see
[third_party/README.md](third_party/README.md)), `binutils-m68k-linux-gnu` (the Classic
environment's 68k test programs); for the local `make test-classic` also `hfsutils` (it makes
a blank HFS disk image).

```bash
make        # build the loader, the kernel, the user programs and the initrd into build/esp/
make run    # boot it in QEMU with OVMF into the desktop (Mouseion and a terminal), serial on stdio
make test   # host unit tests + QEMU self tests (keys typed via QMP) + a screen check + disk tests
make ehfs-tools  # mkfs.ehfs, fsck.ehfs, ehfs-cp, ehfs-ls for image files, in build/host/ehfs/
make disk-image  # build/stela.img for a USB stick (writing it is your own step: docs/p520.md)
make test-classic  # local only: boots your own Mac IIci ROM and System to the Finder
make bench  # measurements without KVM (docs/bench.md)
make test-fuzz    # the parsers fuzzed on the host (part of make test, about a minute)
make test-stress  # local only: many processes, files, pipes, connections and windows; leaks checked
```

## License

GNU General Public License v3.0 or later. See [LICENSE](LICENSE).

The Fusion Pixel font's glyphs in `third_party/fusion-pixel/` (by TakWolf, with glyphs from
Ark Pixel, Cubic 11 and Galmuri) are under the SIL Open Font License 1.1 (see
[third_party/README.md](third_party/README.md)).

musl is not part of the repository; the Linux test programs in the initrd are linked with it
(MIT). The Spleen font in `third_party/spleen/` is by Frederic Cambus and is under the BSD 2-Clause
license (see [third_party/spleen/LICENSE](third_party/spleen/LICENSE)).

The Classic environment's `classic/musashi/` is Karl Stenerud's Musashi (MIT) with John R.
Hauser's SoftFloat (see `classic/musashi/softfloat/README.txt`); `classic/c/` comes from 68k_HAL
(GPL-3.0-or-later). No Apple ROM, System software, fonts or headers are in the repository.

`ehfs/src/unicode_data.rs` is generated from the Unicode Character Database 15.0.0 and carries
the Unicode License V3 (included in the file).

## Author

**YuanChi Hsieh** — GitHub [@boring89](https://github.com/boring89)
