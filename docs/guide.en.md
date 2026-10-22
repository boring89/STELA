# STELA User Guide

> Written by the P13 scheduled run (I, K24); **not yet reviewed**. Chinese version: `docs/guide.md`. Inside STELA, Periplus's home page links to it (`System:/doc/guide.en.html`).

STELA is an operating system written from scratch for x86-64 PCs with UEFI: simple and spatial like the Classic Mac OS, with the features of a modern system. This guide shows how to use it. Every "Try it" can be done in QEMU: run `make run` in the source tree (no Data volume; the prompt is `Initrd:/>`), or make a disk with a Data volume with `make disk-image` (`docs/p520.md`).

## Starting up and Setup

While STELA starts, a stela (its logo) stands in the middle of the screen. Then the desktop appears: the menu bar at the top, the volumes' icons on the right, and a Terminal at the top left.

- The first time STELA starts on a new disk, **Setup.app** comes first and asks, page by page: the language (English, Traditional Chinese), the time zone, the Data volume (without one, pick a partition and type its full name to format it), the network, packages to install, and Classic files to copy; then the desktop starts. Setup is used with the keyboard only: arrows to choose, Space to tick, Enter for the next page, Esc to go back.
- Volumes: `Initrd` (the programs that start the system, read only), `System` (applications, tools, fonts, this guide; read only), `Data` (your files), `Boot` (the FAT partition it started from, read only), `Tmp` (memory, gone at shutdown), and any USB stick you plug in.
- Boot options live in `STELA/boot.cfg` on the ESP, one a line; see Troubleshooting.

Try it: `make run`, wait for the stela to give way to the desktop, and type `vols` in the Terminal.

## The desktop and Mouseion

**Mouseion** is STELA's file manager (named after the ancient academy of Alexandria). It remembers where every folder window was, its size and where its icons sit: close it, open it again, and everything is where you left it.

- **Double-click** a volume or folder to open its window; a program (a `.app` bundle or a program file) to run it; a text file to open it in Kalamos, other documents in Viewer.
- **Drag** an icon: within its window it moves; onto a folder, a volume or another window it is moved (same volume) or copied (another volume); onto the **Trash** it is thrown away (Special > Empty Trash... deletes for good); onto another program's window (Kalamos, say) the file is handed to that program.
- Click to select, Shift-click to add, drag a band over empty space to select several; click the name of the selected item to rename it.
- **Menus**: File > New Folder (Alt-N), Open (Alt-O), Find... (Alt-F: items whose names contain some text; double-click a result to open its folder), Get Info (Alt-I), Rename..., Set Type... (the Mac's type and creator), Close (Alt-W); View > Icons / List (click a column title of the list to sort); Special > Terminal, Monitor, Sleep Display (the screen goes dark now), Empty Trash..., Restart, Shut Down; Mouseion > Preferences... (the desktop: plain, Greek key, waves).
- **Windows**: drag the title bar to move, the bottom right corner to resize; the box at the top left closes, the zoom box at the top right makes the window just big enough (and back).
- **Keyboard**: Alt with a letter is a menu item's shortcut (`^N` in a menu means Alt-N); Alt-M opens the menus from the keyboard (Left and Right change menus, Up and Down items, Enter chooses, Escape puts it back); Alt-Tab switches windows (hold Alt, press Tab to choose, let go of Alt to switch).
- **On the right of the menu bar**: the speaker (click it for the volume and Mute), the input method's 注／英, the clock.
- After a while without input (10 minutes; `blank=MINUTES` in `Data:/.stela/display`, 0 for never) the screen goes black; any key or mouse movement brings it back (that one does not count as a click).

**Programs live in their own room**: a program Mouseion starts sees only its own container folder (`Data:/Programs/NAME/container/`). To give it another file, use its File > Open (the dialog is drawn by the system: the program gets only the file you pick), drag the file onto its window, or double-click the document in Mouseion. Terminal, Classic, Setup and programs started from the shell are not limited this way. See `docs/sandbox.md`.

Try it (with a Data volume): double-click Data on the right, then `welcome.txt`: Kalamos opens it. Drag it to the Trash and back out.

## Terminal and the shell

The Terminal window runs STELA's shell; the prompt is where you are (`Data:/>`).

| Command | What it does |
|---|---|
| `ls [path]`, `cd [path]`, `pwd` | list a folder, change folders, where am I |
| `cat file`, `cp from to` (`cp -r` for a folder), `mv`, `mkdir`, `rm` | files |
| `run program [args]` | run a program or `Name.app` (from here, then `Initrd:/`, `System:/Programs/`, `System:/bin/`); Ctrl-C stops it; typing a program's name works too |
| `ps`, `mem`, `uptime`, `vols`, `hwinfo` | processes, memory, time since boot, volumes, hardware |
| `ifconfig`, `ping`, `fetch` | network |
| `tone [Hz [ms]]` | play a tone |
| `shutdown`, `reboot` | turn off, restart |
| `help` | every command |

- **Paths**: `Volume:/a/b`, `/a/b` (this volume) or `a/b`, with `.` and `..`.
- **Pipes and redirection**: `a | b`, `a > file`, `a >> file`, `a < file`; `&` at the end runs it in the background; `jobs`, `fg N`, `kill N` (programs only, not built-in commands).
- Up and Down bring back earlier commands, Left and Right move the cursor; **Tab** completes names; `*` and `?` match names (`ls *.txt`).
- `System:/bin/` has the usual tools: `echo cat grep wc head tail sort uniq find du df date sleep touch stat hexdump tar true false`.
- The Terminal shows colours (SGR); drag to select text, Edit > Copy puts it on the clipboard, Edit > Paste types the clipboard to the shell (Ctrl-C in the Terminal stops a program).

Try it: `make run`, then `help`, `ls /bin`, `echo hello | wc -c` and `run Echo.app` in the Terminal.

## Installing software

**pkg** installs software from a package server (only after checking its signature):

```
pkg update               get the catalogue (server = https://... in Data:/.stela/pkg.conf)
pkg search word          search it
pkg install name         install (with what it needs)
pkg list                 what is installed
pkg remove name          remove
```

Packages go to `Data:/Programs/NAME/`; the shell finds programs in `Data:/Programs/*/bin/`. The signing key is a **test key** for now, and there is no official package server yet (`docs/pkg.md`).

**Ports**: 22 programs, among them python3, git, vim, nano, curl, sqlite, lua, tcc and busybox, build and run on STELA through its Linux compatibility (`docs/ports.md`).

## Applications

- **Kalamos** (text editor): File > New (Ctrl-N), Open... (Ctrl-O), Save (Ctrl-S), Save As... (Ctrl-Shift-S), Close (Ctrl-Q/W); Edit has Undo, Cut, Copy, Paste, Select All; View changes the font size. It asks before closing with unsaved changes.
- **Periplus** (web browser): Ctrl-L for an address, Enter to go; Tab / Shift-Tab choose a link, Enter follows it; Alt-Left / Alt-Right go back and forward; HTTP and HTTPS (TLS 1.3), part of HTML. Local files have addresses like `Data:/path` or `System:/doc/guide.en.html`.
- **Monitor** (Special > Monitor): processes, memory, CPUs; it can end a process.
- **Viewer**: shows a text file, read only.
- **Echo**: the toolbox's example program.

Try it: `make run`, type `run Periplus.app`, Tab to "STELA User Guide" on the home page and press Enter.

## The Classic environment

Classic.app runs a 68k Macintosh (a Mac IIci) in a window, with **your own** ROM and System (STELA comes with nothing from Apple):

```
Data:/Classic/ROM          the Mac IIci ROM (512 KiB)
Data:/Classic/System.img   an HFS disk image (System 7.5.3, say)
Data:/Classic/classic.cfg  (optional) ram=32, resolution=800x600, depth=8, scale=2, disk2=...
```

Double-click `System:/Programs/Classic.app` (or `run Classic.app`). Keyboard: **Alt = Command**, Ctrl = Control, the Windows key or right Alt = Option; Backspace is delete. STELA's Data volume appears on the Mac's desktop, so files go back and forth directly. Special > Shut Down on the Mac ends Classic.app. See `docs/classic.md`.

## Linux programs

STELA runs x86-64 Linux programs (static or dynamically linked with musl, PIE): type the name or `run program`. They see `/Data/...`, `/Initrd/...`, `/tmp`, `/dev`, `/proc` and `/lib`, and may use threads, fork, signals, ptys and sockets (`docs/linux.md`).

Try it: `make run`, then `run linux/hello a b` and `run linux/hello-std`.

## Zhuyin input

**Ctrl-Space** switches between Chinese (注 in the menu bar) and English (英). Type on the standard Zhuyin keyboard, for example `su3cl3` for 你好: while composing, the text shows in a small box under the cursor; Down opens the candidates, a number picks one, Enter sends it. Shift with a letter types that letter. The engine is libchewing; the phrases you choose are kept in `Data:/.stela/ime/` (`docs/ime.md`). Kalamos, Periplus, the Terminal and dialogs all take Chinese.

## Network and sound

- The address comes from DHCP at start (wired cards: Intel e1000 / e1000e / I219, virtio-net; USB adapters: CDC ECM / NCM / RNDIS, AX88179). `ifconfig` shows it; try `ping 10.0.2.2` and `fetch https://example.com/`.
- Sound (Intel HD Audio): several programs can play at once; the menu bar's speaker sets the master volume and Mute, and remembers them. Try `tone 440 1000`.

## Shutting down

Special > Shut Down or Restart in Mouseion, or `shutdown` / `reboot` in the shell. The volumes are unmounted cleanly first (no check at the next start).

## Troubleshooting

`STELA/boot.cfg` (on the ESP, one option a line):

| Option | Use |
|---|---|
| `textmode` | no desktop: the shell in text mode |
| `nousb` | no USB (the keyboard through the firmware's PS/2 emulation) |
| `nonet`, `nosmp` | no network card; one processor |
| `verbose`, `waitkey` | a line for every step of the start, and a key to go on after each: when it hangs, look at the last line |
| `scale=1` / `scale=2` | the interface's scale (default: 2 from 2560 pixels wide) |
| `usernet`, `usbrndis` | the user-space e1000 driver; RNDIS first for USB adapters |
| `setup` | run Setup again |

- No desktop: `textmode`; no keyboard: `nousb`; it hangs while starting: `verbose` (then `waitkey`).
- **The panic screen** (a cracked stela with the last 10 lines of messages under it): take a photo of the whole screen, the messages especially, and report it with what you were doing.
- Real machines and the hardware list: `docs/p520.md`.
