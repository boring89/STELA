# Spleen

Bitmap fonts by Frederic Cambus, used by STELA's framebuffer console.

- Source: https://github.com/fcambus/spleen, release 2.1.0
  (`https://github.com/fcambus/spleen/releases/download/2.1.0/spleen-2.1.0.tar.gz`,
  downloaded 2026-10-20)
- License: BSD-2-Clause, see [LICENSE](LICENSE) (copied from the release)

| file | SHA-256 |
|---|---|
| `spleen-2.1.0.tar.gz` | `8b47c56f1a6eb858fbcf9e34530557404b02fbb3455e38e64fb84473fd0c372f` |
| `spleen-8x16.bdf` | `b2b05484ba4380c9c6854cc57637235a9fb039c65b5ba38a65289540f8fab7dc` |
| `spleen-16x32.bdf` | `3b6ae73ef1dd3c78bad873697c18b15df82d8e5c0258114fda401556e01d0e67` |
| `LICENSE` | `3cb3f3f5a795547d329828df881c80d923bfc75f65065c44ffa703ad78c678bd` |

`spleen_8x16.rs` and `spleen_16x32.rs` are generated from the BDF files (printable
ASCII, U+0020..U+007E) with:

```bash
python3 tools/font_to_rs.py spleen-2.1.0/spleen-8x16.bdf third_party/spleen/spleen_8x16.rs
python3 tools/font_to_rs.py spleen-2.1.0/spleen-16x32.bdf third_party/spleen/spleen_16x32.rs
```

The BDF files themselves are not kept in the repository.
