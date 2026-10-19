# STELA

STELA is an operating system written from scratch in Rust for x86-64 PCs with UEFI firmware.
It inherits the design philosophy of the classic Macintosh — simplicity, directness, a spatial
file manager — and adds what a modern operating system needs. It depends on no Apple component;
a classic Macintosh environment (the user's own ROM and System running on a 68k interpreter)
is planned as an optional, isolated compatibility layer, next to a Linux compatibility layer.

STELA is the successor of [68k_HAL](https://github.com/boring89/68k_HAL) (C, archived at v1.0),
whose experience it reuses and whose code it does not.

## Status

Early: a UEFI boot loader that prints a banner. Nothing else yet.

## Building

Requirements: `rustup` (the pinned toolchain and targets are installed automatically from
`rust-toolchain.toml`), `qemu-system-x86`, `ovmf`, `python3`, `make`.

```bash
make        # build the loader into build/esp/
make run    # boot it in QEMU with OVMF, serial on stdio
make test   # host unit tests + a QEMU boot test
```

## License

GNU General Public License v3.0 or later. See [LICENSE](LICENSE).

## Author

**YuanChi Hsieh** — GitHub [@boring89](https://github.com/boring89)
