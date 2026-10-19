# STELA

STELA is an operating system written from scratch in Rust for x86-64 PCs with UEFI firmware.
It inherits the design philosophy of the classic Macintosh — simplicity, directness, a spatial
file manager — and adds what a modern operating system needs. It depends on no Apple component;
a classic Macintosh environment (the user's own ROM and System running on a 68k interpreter)
is planned as an optional, isolated compatibility layer, next to a Linux compatibility layer.

STELA is the successor of [68k_HAL](https://github.com/boring89/68k_HAL) (C, archived at v1.0),
whose experience it reuses and whose code it does not.

## Status

Early (phase P1 of [the plan](docs/architecture.md)): STELA's own UEFI boot loader loads the
kernel ELF, builds its page tables (higher-half kernel and a direct map of physical memory) and
hands over a boot information block. The kernel sets up its GDT/TSS and exception handlers, a
bitmap physical frame allocator, its page tables and a heap, and prints to the serial port and
to a framebuffer text console (Spleen font); a kernel panic shows a panic screen. There are no
interrupts, timers, processes or drivers yet. See [docs/boot.md](docs/boot.md) and
[docs/frame.md](docs/frame.md).

## Building

Requirements: `rustup` (the pinned toolchain and targets are installed automatically from
`rust-toolchain.toml`), `qemu-system-x86`, `ovmf`, `python3`, `make`.

```bash
make        # build the loader and the kernel into build/esp/
make run    # boot it in QEMU with OVMF, serial on stdio
make test   # host unit tests + QEMU self tests + a screen check
```

## License

GNU General Public License v3.0 or later. See [LICENSE](LICENSE).

The Spleen font in `third_party/spleen/` is by Frederic Cambus and is under the BSD 2-Clause
license (see [third_party/spleen/LICENSE](third_party/spleen/LICENSE)).

## Author

**YuanChi Hsieh** — GitHub [@boring89](https://github.com/boring89)
