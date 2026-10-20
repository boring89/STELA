# Third-party material

| What | Where | License | Use |
|---|---|---|---|
| Spleen font 8x16, 16x32 (Frederic Cambus) | `spleen/` | BSD-2-Clause (`spleen/LICENSE`) | the console and UI text |
| musl 1.2.5 | **not in the repository**; built in `build/musl/` by `make musl` | MIT | only to build the Linux test programs (`linux/tests/`) |

## musl

- Tarball: `musl-1.2.5.tar.gz`, SHA-256
  `a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4` (the value musl
  publishes for the 1.2.5 release; the `.asc` signature sits next to the tarball in
  68k_HAL's downloads). The Makefile checks it (`MUSL_SHA256`) before building.
- Locally the tarball is copied from `~/文件/68k_HAL/third_party/downloads/musl/` (override
  with `make MUSL_TARBALL=/path/to/musl-1.2.5.tar.gz`) to `build/downloads/`; nothing is
  downloaded. CI fetches it from musl.libc.org once and caches it.
- Build: `./configure --prefix=build/musl/install --target=x86_64 --disable-shared`, `make`,
  `make install`; the programs are compiled with `build/musl/install/bin/musl-gcc -static
  -no-pie`. No musl source or binary is committed; the test programs in the initrd contain
  statically linked musl code (MIT).
