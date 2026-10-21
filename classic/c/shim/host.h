/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

/*
 * The Classic environment's interface to the Macintosh model: one machine
 * (a static mac_t), started with the ROM, RAM and video RAM the Rust side
 * owns and a classic_host_ops with the callbacks for the disk and the log.
 * Every function the Rust side calls (classic/src/ffi.rs) is here, with
 * plain argument types, so that Rust never needs the layout of mac_t.
 */

#ifndef CLASSIC_SHIM_HOST_H
#define CLASSIC_SHIM_HOST_H

#include <stdbool.h>
#include <stdint.h>

/* Disks besides the System disk (drives 9..; mac/mac.h MAC_MAX_DISKS - 1). */
#define CLASSIC_MORE_DISKS 3

typedef struct
{
    /* The System disk: byte offsets and lengths (multiples of 512);
     * return 0 on success, -1 on failure. */
    int (*disk_read)(void *ctx, uint32_t offset, void *buf, uint32_t len);
    int (*disk_write)(void *ctx, uint32_t offset, const void *buf, uint32_t len);
    void *ctx;
    uint32_t disk_blocks;       /* 512-byte blocks; 0 = no disk */
    bool disk_read_only;
    /* The sound chip's output: unsigned 8-bit mono samples at 'rate' Hz,
     * a few hundred at a time; NULL = discard. */
    void (*audio)(void *ctx, const uint8_t *samples, uint32_t count, uint32_t rate);
    void *audio_ctx;
    /* An external file system volume for the File System Manager
     * (mac/fsm.h; P11): a STELA folder as a Mac volume. NULL = none. It
     * must stay valid until the next classic_start. */
    const void *fs;
    /* The NuBus video card in slot $C (mac/mac.h; P11): its video RAM
     * (CLASSIC_CARD_VRAM_SIZE bytes; NULL = the built-in video only), the
     * resolution it starts in (an index of the card's list,
     * classic_card_resolution) and the depth (0-5 = 1, 2, 4, 8, 16, 32
     * bits). */
    uint8_t *card_vram;
    uint32_t card_res;
    uint32_t card_depth;
    /* More disks (P11): drives 9, 10 and 11, through disk_read and
     * disk_write with their own ctx; blocks 0 = none. */
    struct
    {
        void *ctx;
        uint32_t blocks;
        bool read_only;
    } more_disks[CLASSIC_MORE_DISKS];
} classic_host_ops;

/* Size of the card's video RAM. */
#define CLASSIC_CARD_VRAM_SIZE 0x2000000u

/* The index of resolution width x height in the card's list, or -1. */
int classic_card_resolution(uint32_t width, uint32_t height);

/* Results of classic_start. */
#define CLASSIC_OK 0
#define CLASSIC_BAD_ROM -1      /* not the Mac IIci ROM 368CADFE */
#define CLASSIC_BAD_ARGS -2     /* sizes the model refuses */

/* Size of the built-in video RAM the caller provides. */
#define CLASSIC_VRAM_SIZE 0x80000u

/*
 * Patches 'rom' (a private copy, changed in place), attaches ROM, RAM and
 * video RAM, the disk of 'ops' (copied), the parameter RAM (256 bytes) and
 * the clock (seconds since 1 January 1904), and resets the machine. The
 * buffers must stay valid until the next classic_start.
 */
int classic_start(uint8_t *rom, uint32_t rom_size, uint8_t *ram, uint32_t ram_size,
                  uint8_t *vram, const classic_host_ops *ops, const uint8_t *xpram,
                  uint32_t rtc_seconds);

/* Runs one 60 Hz frame of the 25 MHz CPU; returns the power request
 * (0 none, 1 shut down, 2 restart). */
int classic_frame(void);

/* The screen size (640x480 for the built-in video). */
void classic_screen_size(uint32_t *width, uint32_t *height);

/* Draws the screen into 'out' (width * height 0x00RRGGBB); returns the rows
 * drawn or -1. */
int classic_render(uint32_t *out);

/* Input: the pointer to (x, y) with the button state; an ADB key. */
void classic_mouse(int x, int y, bool button);
void classic_key(uint8_t adb_code, bool down);

/* Copies the parameter RAM to 'out' (256 bytes); returns whether it changed
 * since the last call. */
bool classic_xpram(uint8_t *out);

/* Milestones: 1 once the Finder runs (low memory CurApName, $910, is
 * "Finder"). */
uint32_t classic_milestones(void);

/* Makes the running system beep (SysBeep), as an application would; for
 * tests, on an idle desktop. Returns 0, or -1 if a call is under way. */
int classic_sysbeep(void);

/* The mounted volumes (the File Manager's VCB queue, Inside Macintosh:
 * Files): for each, up to 'max', the drive number and the name (Mac Roman,
 * NUL-terminated, at most 27 bytes). Returns how many there are. */
int classic_volumes(uint16_t *drives, char (*names)[28], int max);

/* Instructions executed so far. */
uint32_t classic_instructions(void);

/* The external file system's state: the install's result (0, an OSErr,
 * -1/-2 no FSM 1.2, -3 no memory; 1 not yet), whether volume 0 is
 * mounted, and the File Manager requests served. */
void classic_fs_status(int32_t *install, bool *mounted, uint32_t *requests);

#endif
