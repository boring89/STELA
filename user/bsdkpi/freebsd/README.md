# FreeBSD source (BSD-2-Clause / BSD-3-Clause)

Files from FreeBSD, **unchanged**, used by bsdnet (`user/bsdkpi`, P14 K13–K15,
`docs/bsdkpi.md`). Fetched on 2026-10-23 from the GitHub mirror
`freebsd/freebsd-src`, tag **`release/14.3.0`** (tag object
`ab74684c4debaec81dd8a1e57116cbd47f593e04`), path `sys/<path>`:

| here | FreeBSD | license |
|---|---|---|
| `dev/e1000/*` (43 files: the whole directory) | `sys/dev/e1000/` | BSD-2-Clause (Intel; `dev/e1000/LICENSE`) |
| `net/iflib.h`, `net/ifdi_if.m` (reference) | `sys/net/` | BSD-2-Clause |
| `net/if_media.h` | `sys/net/if_media.h` | BSD-2-Clause |
| `dev/pci/pcireg.h` | `sys/dev/pci/pcireg.h` | BSD-2-Clause |
| `sys/queue.h` | `sys/sys/queue.h` | BSD-3-Clause |
| `dev/iwm/*` (38 files: the whole directory; K15, compile-only) | `sys/dev/iwm/` | ISC (OpenBSD's iwm), BSD-2-Clause; the Intel parts are dual BSD/GPLv2 and are used under their BSD (BSD-3-Clause) terms |
| `net80211/*.h` (33 headers, no `.c`; K15) | `sys/net80211/` | BSD-2-Clause (two ISC, one BSD-3-Clause) |

SHA-256 of every file: `SHA256SUMS.e1000` (run in `dev/e1000/`),
`SHA256SUMS.iwm` (run in `dev/iwm/`), `SHA256SUMS.net80211` and
`SHA256SUMS.other`. No firmware: iwm's `iwm*fw` images (Intel's, not free)
are not fetched and not in the repo. To update: fetch the same paths from a newer tag, keep
this table and the sums current, and run `make test-net-user`.

Everything else in `user/bsdkpi` (the shim in `shim/`, the Rust program in
`src/`) is STELA's own: the shim is MIT, the program GPL-3.0-or-later.
