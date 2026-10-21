# Third-party material

| What | Where | License | Use |
|---|---|---|---|
| Spleen font 8x16, 16x32 (Frederic Cambus) | `spleen/` | BSD-2-Clause (`spleen/LICENSE`) | the console and UI text |
| Fusion Pixel 12px Mono zh_TW 2026.10.16 (TakWolf; glyphs from Ark Pixel, Cubic 11, Galmuri) | `fusion-pixel/` (converted glyphs) | SIL Open Font License 1.1 (`fusion-pixel/OFL.txt`, `fusion-pixel/LICENSES/`) | characters beyond ASCII in the toolbox (P11) |
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

## Fusion Pixel (P11 K39)

- Source: the official GitHub release `TakWolf/fusion-pixel-font` 2026.10.16,
  `fusion-pixel-font-12px-monospaced-bdf-v2026.10.16.zip`, SHA-256
  `76ed4adde0cc43ae8312041960d586c130c75a9d0f7c1c64a7cace7e4130c358` (downloaded once to
  `build/downloads/`, not committed; the user allowed the download on 2026-10-21).
- Converted with `python3 -I tools/bdf_to_cjk.py fusion-pixel-12px-monospaced-zh_tw.bdf
  third_party/fusion-pixel/fusion-pixel-12-zh_tw.bin`: the code points U+0080–U+FFFF the font
  has (35,701 glyphs, about 1 MB), 12 rows each; the format is in the script and in
  `user/toolbox/src/cjk.rs`. The file goes into the initrd as `fonts/` and the toolbox reads it
  when a program first draws such a character.
- The OFL allows embedding and redistribution with the copyright notice and the license; the
  font's name is not changed (Reserved Font Name). `OFL.txt` and the licenses of the fonts
  Fusion Pixel draws on (`LICENSES/ark-pixel`, `cubic-11`, `galmuri`, all OFL 1.1) are kept
  as they came.
