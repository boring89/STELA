/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#include "shim/host.h"

#include "mac/mac.h"
#include "shim/k_lib.h"

/* The ROM's tables put the built-in video RAM here (UniversalInfo; the same
 * value as 68k_HAL's kernel/mac/machine.c and tools/mac_run.c). */
#define VRAM_BASE 0xFBB08000u
/* 25 MHz / 60 Hz. */
#define FRAME_CYCLES 416000
/* Low memory CurApName (Str31), Inside Macintosh: Processes. */
#define LM_CURAPNAME 0x910u

static mac_t machine;


int classic_start(uint8_t *rom, uint32_t rom_size, uint8_t *ram, uint32_t ram_size,
                  uint8_t *vram, const classic_host_ops *ops, const uint8_t *xpram,
                  uint32_t rtc_seconds)
{
    if (mac_rom_patch(rom, rom_size) != 0)
    {
        return CLASSIC_BAD_ROM;
    }
    k_memset(&machine, 0, sizeof machine);
    if (mac_init(&machine, ram, ram_size, rom, rom_size) != 0)
    {
        return CLASSIC_BAD_ARGS;
    }
    machine.vram = vram;
    machine.vram_size = CLASSIC_VRAM_SIZE;
    machine.vram_base = VRAM_BASE;
    machine.disk.read = ops->disk_read;
    machine.disk.write = ops->disk_write;
    machine.disk.ctx = ops->ctx;
    machine.disk.blocks = ops->disk_blocks;
    machine.disk.read_only = ops->disk_read_only;
    machine.audio = ops->audio;
    machine.audio_ctx = ops->audio_ctx;
    machine.rtc_seconds = rtc_seconds;
    k_memcpy(machine.xpram, xpram, sizeof machine.xpram);
    machine.xpram_dirty = false;
    if (ops->fs != NULL)
    {
        mac_fsm_attach(&machine, (const mac_fs_ops_t *)ops->fs);
    }
    mac_reset(&machine);
    return CLASSIC_OK;
}


int classic_frame(void)
{
    mac_run(&machine, FRAME_CYCLES);
    return machine.power_request;
}


void classic_screen_size(uint32_t *width, uint32_t *height)
{
    mac_screen_size(&machine, width, height);
}


int classic_render(uint32_t *out)
{
    return mac_render_changes(&machine, out, NULL);
}


void classic_mouse(int x, int y, bool button)
{
    mac_mouse_warp(&machine, x, y);
    mac_mouse_move(&machine, 0, 0, button);
}


void classic_key(uint8_t adb_code, bool down)
{
    mac_key(&machine, adb_code, down);
}


bool classic_xpram(uint8_t *out)
{
    bool dirty = machine.xpram_dirty;

    k_memcpy(out, machine.xpram, sizeof machine.xpram);
    machine.xpram_dirty = false;
    return dirty;
}


uint32_t classic_milestones(void)
{
    uint32_t m = 0;

    if (mac_bus_read(&machine, LM_CURAPNAME, 1) == 6 &&
        mac_bus_read(&machine, LM_CURAPNAME + 1, 4) == 0x46696E64u &&
        mac_bus_read(&machine, LM_CURAPNAME + 5, 2) == 0x6572u)
    {
        m |= 1;
    }
    return m;
}


int classic_sysbeep(void)
{
    return mac_sysbeep(&machine);
}


uint32_t classic_instructions(void)
{
    return machine.insn_count;
}


void classic_fs_status(int32_t *install, bool *mounted, uint32_t *requests)
{
    const mac_fsm_t *fs = &machine.fsm;

    *install = fs->install_pending ? 1 : fs->install_error;
    *mounted = fs->volumes > 0 && fs->volume[0].mounted;
    *requests = fs->requests;
}

