# Third-party material

| What | Where | License | Use |
|---|---|---|---|
| Spleen font 8x16, 16x32 (Frederic Cambus) | `spleen/` | BSD-2-Clause (`spleen/LICENSE`) | the console and UI text |
| Fusion Pixel 12px Mono zh_TW 2026.10.16 (TakWolf; glyphs from Ark Pixel, Cubic 11, Galmuri) | `fusion-pixel/` (converted glyphs) | SIL Open Font License 1.1 (`fusion-pixel/OFL.txt`, `fusion-pixel/LICENSES/`) | characters beyond ASCII in the toolbox (P11) |
| FreeBSD 14.3 `sys/dev/e1000` (if_em), `net/iflib.h`, `net/if_media.h`, `dev/pci/pcireg.h`, `sys/queue.h` | `user/bsdkpi/freebsd/` (unchanged; source and SHA-256 in its README) | BSD-2-Clause, BSD-3-Clause | bsdnet: FreeBSD's driver on STELA (P14 K13–K14, `docs/bsdkpi.md`) |
| libchewing 0.5.1 (P13) | **not in the repository**; unpacked in `build/chewing/` by `make chewing` | LGPL-2.1-or-later | the Zhuyin input method (`user/ime`, `docs/ime.md`) |
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

## libchewing (P13 K1)

- Tarball: the official GitHub release `chewing/libchewing` v0.5.1,
  `libchewing-0.5.1.tar.bz2`, SHA-256
  `9708c63415fa6034435c0f38100e7d30d0e1bac927f67bec6dfeb3fef016172b` (downloaded
  2026-10-22, allowed in P13_PLAN.md; the Makefile checks it, `CHEWING_SHA256`). 0.5.1 is
  the last release written in C; later ones are Rust with crates of their own.
- Locally the tarball is copied from `~/stela-ports/sources/` (`CHEWING_TARBALL=`), or else
  `make chewing` downloads it to `build/downloads/`; CI fetches it once and caches it.
- `make chewing` unpacks it into `build/chewing/src` and builds libchewing's host tool
  `init_database` (with `user/ime/c/config.h`), which turns `data/phone.cin` and
  `data/tsi.src` into `dictionary.dat` and `index_tree.dat` in `build/chewing/data`;
  `pinyin.tab`, `swkb.dat` and `symbols.dat` are copied as they are. The five files go
  into the initrd as `chewing/` (about 3.7 MB).
- `user/ime/build.rs` compiles the library (the hash user-phrase back end, no sqlite3)
  freestanding against STELA's stand-in C library (`user/ime/c/`), unchanged.
- License: LGPL-2.1-or-later (`COPYING` in the tarball). The IME links it statically into a
  GPL-3.0-or-later program, which the LGPL allows (GPL-3.0 is a later version the LGPL lets
  the library be used under); the library's source is the published tarball above and
  the build steps are the Makefile's.

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

## Mozilla CA certificates (P12 K10)

- Source: curl's conversion of Mozilla's root store, `https://curl.se/ca/cacert.pem`,
  "Certificate data from Mozilla as of: Tue Oct 20 13:32:40 2026 GMT", 118 certificates,
  200,116 bytes, SHA-256 `796f1389bfea13c0ed78f663b765baaff54a508e5483786bd6ece73c5e3e3cbe`
  (equal to curl's published `cacert.pem.sha256`). Downloaded 2026-10-22 (allowed in
  P12_PLAN.md); committed as `third_party/mozilla-ca/cacert.pem`.
- License: MPL-2.0 (the file's header). It is a separate data file, distributed unchanged
  next to GPL-3.0-or-later code, which the MPL allows.
- `make initrd` converts it with `tools/cacert_to_der.py` into `Initrd:/certs/cacert.der` (the
  DER certificates one after another), which `libstela::tls` reads (`docs/tls.md`).
- To update: download the new `cacert.pem`, check the SHA-256 curl publishes, replace the file
  and this paragraph.

## Rust crates for TLS (P12 K10)

Added for `libstela::tls` (`docs/tls.md`), from crates.io, versions locked in `Cargo.lock`.
Direct dependencies of `user/libstela`:

| crate | version | license | use |
|---|---|---|---|
| embedded-tls | 0.19.0 | Apache-2.0 | TLS 1.3 client, `no_std`; features `alloc`, `rsa`, `p384` (its `rustpki` certificate checks) |
| embedded-io | 0.7.1 | MIT OR Apache-2.0 | the read/write traits embedded-tls uses on a socket |
| rand_core | 0.6.4 | MIT OR Apache-2.0 | the random-number trait (the kernel's `Random` behind it) |
| sha2 | 0.10.9 | MIT OR Apache-2.0 | named only to turn on `force-soft` (no SSE in the user target) |

What they pull in (all MIT OR Apache-2.0 unless noted): the RustCrypto crates aead, aes,
aes-gcm, cipher, ctr, ghash, polyval, universal-hash, digest, block-buffer, crypto-common,
hkdf, hmac, p256, p384, ecdsa, elliptic-curve, primeorder, sec1, rfc6979, signature,
crypto-bigint, ff, group, base16ct, base64ct, der (0.7 and 0.8), der_derive, const-oid, spki,
pkcs1, pkcs8, rsa, num-bigint-dig, zeroize, subtle (BSD-3-Clause), generic-array (MIT),
typenum, opaque-debug, inout, cpufeatures, libc; and heapless, hash32, stable_deref_trait,
portable-atomic, byteorder (Unlicense OR MIT), num-integer, num-iter, num-traits, rand,
rand_chacha, ppv-lite86, zerocopy (BSD-2-Clause OR Apache-2.0 OR MIT), smallvec, spin (MIT),
lazy_static, libm (MIT), time, time-core, deranged, num-conv, powerfmt, autocfg,
version_check, syn. All are compatible with GPL-3.0-or-later. The AES and POLYVAL crates are
built with `--cfg aes_force_soft --cfg polyval_force_soft` (`.cargo/config.toml`): their
AES-NI/CLMUL code needs SSE, which the `x86_64-unknown-none` target does not have.

## Rust crates for package signatures (P12 K12)

`user/pkgcore` checks the ed25519 signature of the package index (`docs/pkg.md` §4):

| crate | version | license | use |
|---|---|---|---|
| ed25519-dalek | 2.2.0 | BSD-3-Clause | ed25519 signature check (`verify_strict`, RFC 8032) |
| curve25519-dalek | 4.1.3 | BSD-3-Clause | the curve arithmetic under it (serial backend: `--cfg curve25519_dalek_backend="serial"`) |
| curve25519-dalek-derive | 0.1.1 | MIT OR Apache-2.0 | its build-time macros |
| ed25519 | 2.2.3 | Apache-2.0 OR MIT | the signature type |
| fiat-crypto | 0.2.9 | MIT OR Apache-2.0 OR BSD-1-Clause | verified field arithmetic |
| rustc_version, semver | 0.4.1, 1.0.28 | MIT OR Apache-2.0 | build scripts |

sha2 (already listed) gives the SHA-256 of downloaded packages. All are compatible with
GPL-3.0-or-later. The signing side is `tools/pkgsign.py` (Python's `cryptography`, on the
build machine only).
