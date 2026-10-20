# STELA's icons — DRAFT

**DRAFT — to be replaced by the user's own artwork.** Every image in this folder is a
placeholder drawn by `tools/make_draft_assets.py` from rectangles, triangles and ellipses
(nothing traced from any existing work). The real logo and icons are to be drawn by the
user; the system reads whatever is here.

## Files

`NAME-SIZE.ppm` (colour, binary P6, 8 bits per channel) and `NAME-SIZE.pbm` (1-bit,
binary P4) for SIZE 16, 32, 64 and 128 (square). In the PPM, **`#FF00FF` is transparent**.
In the PBM, 1 (black) is ink.

| NAME | What | Used by |
|---|---|---|
| `stela` | the logo: a Greek stela (pediment, carved lines, base) | boot screen (128) |
| `stela-cracked` | the same stela, cracked down the middle | kernel panic screen (128) |
| `temple` | Mouseion (the file manager) | Mouseion's About, its menu |
| `scroll` | a document | files in Mouseion |
| `kibotos` | a folder: a Greek wooden chest with a meander band | folders in Mouseion |
| `volume` | a disk | volumes on the desktop |
| `app` | a program: a small stela with a seal | `.app` bundles and programs |

Colours (black-figure pottery): terracotta `#C8763C`, glaze black `#1A1410`, parchment
white `#F2E8D5` (the drafts also use a shaded terracotta `#8E4A22`). Any colours work; up
to 255 per image.

## Replacing them

1. Draw the new images at the sizes above (any program that saves binary PPM/PBM, or
   export PNG and convert, e.g. `pnmtopng`'s counterpart `pngtopnm`), keeping the names.
   Mark transparent pixels `#FF00FF` in the PPM.
2. Put them here, replacing the drafts (and remove the DRAFT notes from this file).
3. `make assets` — runs `tools/img_to_rs.py`, which rewrites `assets/generated/`
   (Rust data; do not edit by hand).
4. `make test` (the host tests check every icon decodes to its size and that
   `assets/generated/` matches the images).

`make draft-assets` would overwrite this folder with the drafts again — do not run it
after replacing them.
