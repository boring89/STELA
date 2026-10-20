# classic/: the Classic environment

A Macintosh IIci in a STELA window (`docs/classic.md`). The C code here is
**copied from 68k_HAL** (`~/文件/68k_HAL`, commit `ceb904d`, "README,
CLAUDE.md: 68k_HAL is archived at v1.0"), the only exception to STELA's
rule of not copying 68k_HAL's C code (the user agreed on 2026-10-20: the
Classic environment is by definition a reduced 68k_HAL). Every file keeps
its license header.

| Here | From 68k_HAL | License | Changes |
|---|---|---|---|
| `musashi/` | `third_party/musashi/` | MIT (Karl Stenerud; `musashi/LICENSE`); `softfloat/` is John R. Hauser's SoftFloat 2 (its own notice in `softfloat/README.txt`, as in MAME) | none (configured through `c/m68k/m68kconf.h`, `MUSASHI_CNF`) |
| `c/m68k/` | `emu/m68k/` (without `freestanding/`) | GPL-3.0-or-later | include paths |
| `c/freestanding/` | `emu/m68k/freestanding/` | GPL-3.0-or-later | `divdi3.c` left out (only the 32-bit build needed it) |
| `c/mac/` | `emu/mac/` | GPL-3.0-or-later | include paths |
| `tests/m68k/` | `tests/m68k/` | GPL-3.0-or-later | none |

Include paths: `emu/m68k/…` → `m68k/…`, `emu/mac/…` → `mac/…`,
`third_party/musashi/m68k.h` → `m68k.h`, `third_party/musashi/m68kconf.h` →
`../../musashi/m68kconf.h`, `kernel/lib/k_memory.h` and `k_string.h` →
`shim/k_lib.h`. Comments still name the 68k_HAL paths.

STELA's own files: `c/shim/` (the `k_memcpy`… helpers of 68k_HAL's kernel
library, over memcpy/memset/memcmp), `build.rs`, `src/`.

`build.rs` builds Musashi's `m68kmake` with the build machine's gcc,
generates `m68kops.c`/`.h`, and compiles everything with gcc into
`libclassic_c.a` (freestanding for `x86_64-unknown-none`, against the host
libc for the host tests). Only `src/ffi.rs` has `unsafe` code.

Nothing from Apple is here: the ROM and the System disk are the user's own
files, read at run time from `Data:/Classic/` (never in the repository, the
tests or CI).
