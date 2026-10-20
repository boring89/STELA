/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#ifndef EMU_MAC_MAC_H
#define EMU_MAC_MAC_H

#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>

#include "fsm.h"

/* CPU clock of the Mac IIci (25 MHz); the sound chip and the VIAs count from it. */
#define MAC_CPU_HZ 25000000u

/*
 * Machine model of a Macintosh IIci (68030, no FPU, 32-bit addressing).
 *
 * Think of it as the "motherboard" around the CPU in emu/m68k: it decides
 * which guest address reaches RAM, the ROM or a chip. The model never opens a
 * file or draws a pixel; the program that embeds it hands in the ROM and RAM
 * buffers and reads the results back.
 *
 *   0x00000000 RAM (or the ROM, while the reset "overlay" is on)
 *   0x40000000 ROM, mirrored every rom_size bytes up to 0x4FFFFFFF
 *   0x50000000 I/O chips (VIA1, VIA2/RBV, SCC, SCSI, ASC, SWIM ...)
 *
 * Memory map: "Guide to the Macintosh Family Hardware", 2nd ed., ch. 3 (IIci
 * has ROM at 0x40800000 and I/O at 0x50F00000), and Inside Macintosh: Devices.
 */

#define MAC_ROM_BASE 0x40000000u
#define MAC_ROM_END 0x50000000u
#define MAC_IO_BASE 0x50000000u
#define MAC_IO_END 0x60000000u
/* Slot $E: the on-board video's declaration ROM is the tail of the main ROM. */
#define MAC_SLOT_E_BASE 0xFE000000u
#define MAC_SLOT_E_END 0xFF000000u

/*
 * Native-call stubs (emu/mac/stub.s): a small read-write window in the ROM
 * address range that holds the code and data of the .Disk driver.
 */
#define MAC_STUB_BASE 0x4F000000u
#define MAC_STUB_SIZE 0x2000u
#define MAC_FSM_STUB 0x800u             /* offset of emu/mac/fsm.s in the stub window */
#define MAC_STUB_ADBOP 0x60u
#define MAC_STUB_INJECT 0xA0u       /* offset of the input injection routine */
#define MAC_STUB_PACKET 0x700u      /* the packet handed to the ROM's handler */        /* offset of the ADBOp replacement */
#define MAC_STUB_DRIVER 0x100u      /* offset of the driver header */
#define MAC_STUB_DRIVE_RECORD 0x900u    /* one 0x20-byte record per drive */
#define MAC_STUB_DRIVE_RECORD_SIZE 0x20u

/* A disk the Mac sees, backed by the host or the kernel. Offsets and lengths
 * are in bytes; both callbacks return 0 on success and -1 on failure. */
typedef struct
{
    int (*read)(void *ctx, uint32_t offset, void *buf, uint32_t len);
    int (*write)(void *ctx, uint32_t offset, const void *buf, uint32_t len);
    void *ctx;
    uint32_t blocks;            /* size in 512-byte blocks; 0 = no disk */
    bool read_only;
} mac_disk_t;

/*
 * How the Mac OS ended: Shut Down switches the power off through the RBV (VIA2 port B bit 2, v2PowerOff,
 * active low; Guide to the Macintosh Family Hardware, "VIA2"), Restart starts the ROM again. mac_run stops
 * when either happens; the host decides what to do (the kernel: ACPI off or reset).
 */
typedef enum
{
    MAC_POWER_NONE = 0,
    MAC_POWER_OFF,
    MAC_POWER_RESTART
} mac_power_t;

/* Drives the Mac sees: drive 8 is mac->disk, drives 9.. are mac->more_disks. */
#define MAC_MAX_DISKS 4
#define MAC_FIRST_DRIVE 8

#define MAC_UNIMPL_LOG_SIZE 2048
#define MAC_TRACE_SIZE 4096
#define MAC_FLOW_SIZE 1024       /* ROM blocks remembered in first-seen order */
#define MAC_FLOW_SHIFT 6         /* block = 64 bytes */

typedef struct
{
    uint32_t addr;
    uint32_t pc;
    uint32_t value;     /* value written; 0 for reads */
    uint8_t size;
    bool write;
    uint32_t count;     /* times this exact access (address, pc, r/w) happened */
} mac_access_t;

typedef struct
{
    uint32_t pc;
    uint16_t opcode;
} mac_trace_t;

/* MOS 6522 VIA (see via.c). */
typedef struct
{
    uint8_t ora, orb, ddra, ddrb;
    uint8_t acr, pcr, ifr, ier, sr;
    uint16_t t1_latch, t2_latch;
    uint16_t t1_counter, t2_counter;
    bool t1_running;            /* T1 has been started and not yet timed out */
    bool t2_running;
    uint8_t in_a, in_b;         /* levels the outside world drives on the pins */
} mac_via_t;

/* RBV: the Mac IIci's replacement for VIA2, with the video control bits. */
typedef struct
{
    bool vblank_latched;        /* the internal video interrupt is pending */
    bool sound_irq;             /* the sound chip asks for data */
    uint8_t sier;               /* slot interrupt enable (register 0x12) */
    uint8_t rier;               /* interrupt enable (register 0x13) */
    uint8_t depth;              /* bits 0-2 of register 0x10: 0=1 bpp .. 3=8 bpp */
    uint8_t sense;              /* monitor sense lines reported in bits 3-5 of 0x10 */
    uint8_t slot_irq;           /* NuBus slot interrupts pending: bit 0 = slot $9 .. bit 5 = $E (only $C-$E exist on a IIci) */
    uint8_t other[0x20];        /* registers without special behavior */
} mac_rbv_t;

/* Apple Sound Chip, FIFO mode only (see asc.c). */
typedef struct
{
    uint8_t mode, control, fifo_mode, status, wave_control, volume, clock;
    uint8_t other[0x80];
    uint8_t wave[0x800];        /* wave table RAM */
    uint8_t fifo[2][0x400];
    uint16_t fifo_head[2];
    uint16_t fifo_count[2];
    uint32_t phase;             /* fraction of a sample period passed (units of 1/CPU_HZ) */
} mac_asc_t;

/* Bt-style color lookup table of the built-in video (the DAC). */
typedef struct
{
    uint8_t rgb[256][3];
    uint8_t index;
    uint8_t phase;              /* next component of the entry: 0=R 1=G 2=B */
} mac_dac_t;

/* NCR 5380 SCSI controller with an empty bus (see scsi.c). */
typedef struct
{
    uint8_t reg[8];             /* values the ROM wrote (read-write ones) */
} mac_scsi_t;

/* Apple IWM floppy controller, reduced to what the ROM's probe needs. */
typedef struct
{
    uint8_t lines;      /* bit n = state of IWM line n (see iwm.c) */
    uint8_t mode;       /* mode register, 5 bits */
} mac_iwm_t;


/*
 * The virtual NuBus video card in slot $C (see nubus_video.c and
 * docs/nubus_video.md): its declaration ROM sits at the top of the slot's
 * standard space (0xFCxxxxxx), its video RAM in the slot's super space
 * (0xC0000000). $C is the first of the IIci's three NuBus slots ($C-$E):
 * the Slot Manager keeps parameter RAM only for those (slot $9 gets
 * smSlotOOBErr from sPutPRAMRec, ROM 0x40806230).
 */
#define MAC_CARD_SLOT 0xC                       /* the first card; a second one goes into $D */
#define MAC_CARDS 2
#define MAC_CARD_VRAM_BASE_OF(slot) ((uint32_t)(slot) << 28)
#define MAC_CARD_ROM_END_OF(slot) (0xF1000000u + ((uint32_t)(slot) << 24))  /* slot $C: last byte 0xFCFFFFFF */
#define MAC_CARD_SLOT_IRQ_OF(slot) (1u << ((slot) - 9))  /* the slot's bit in the RBV's slot interrupt flags */
#define MAC_CARD_VRAM_BASE MAC_CARD_VRAM_BASE_OF(MAC_CARD_SLOT)
#define MAC_CARD_ROM_END MAC_CARD_ROM_END_OF(MAC_CARD_SLOT)
#define MAC_CARD_VRAM_SIZE 0x2000000u          /* 32 MiB: 3840x2160 at 32 bits fits */
#define MAC_CARD_ROM_MAX 4096u
#define MAC_CARD_DIRTY_SHIFT 10                 /* video RAM changes are tracked per KiB */
#define MAC_CARD_DIRTY_WORDS (MAC_CARD_VRAM_SIZE >> MAC_CARD_DIRTY_SHIFT >> 5)
#define MAC_CARD_DEPTHS 6                       /* 1, 2, 4, 8, 16 and 32 bits per pixel */
#define MAC_CARD_MODE_1BIT 0x80                 /* Apple's mode number of the first depth */
#define MAC_CARD_RES_ID_BASE 0x80               /* display mode id of the first resolution */

typedef struct
{
    uint16_t width, height;
} mac_card_res_t;

/* The resolutions the card offers, smallest first (nubus_video.c). */
extern const mac_card_res_t mac_card_resolutions[];
extern const uint32_t mac_card_resolution_count;

typedef struct
{
    bool present;
    uint32_t slot;              /* $C or $D */
    uint8_t *vram;              /* MAC_CARD_VRAM_SIZE bytes, set by the host */
    uint8_t rom[MAC_CARD_ROM_MAX];  /* the declaration ROM, its last byte at MAC_CARD_ROM_END - 1 */
    uint32_t rom_size;
    uint32_t vpblock[MAC_CARD_DEPTHS];  /* offsets in rom[] of the VPBlock of each depth */
    uint32_t gamma;             /* offset in rom[] of the gamma table (GammaTbl) */
    uint32_t last_mode_entry;   /* offset in rom[] of the deepest mode list's entry */
    uint32_t res;               /* index in mac_card_resolutions */
    uint32_t depth;             /* 0..5 = 1..32 bits per pixel */
    uint32_t preferred_res, preferred_depth;
    uint8_t clut[256][3];
    bool gray;                  /* luminance mapping (cscSetGray) */
    bool irq_enabled;           /* vertical blanking interrupts on (cscSetInterrupt) */
    bool irq_pending;
    bool all_dirty;             /* the whole picture must be converted again */
    uint32_t dirty[MAC_CARD_DIRTY_WORDS];   /* bit n = KiB n of the video RAM was written */
    uint32_t mode_changes;      /* counts resolution and depth switches (for the host) */
    uint16_t refnum;            /* the driver's reference number once it is open (0 = not yet) */
} mac_card_t;

typedef struct
{
    uint8_t *ram;
    uint32_t ram_size;
    const uint8_t *rom;
    uint32_t rom_size;

    bool overlay;               /* ROM shown at address 0 (set at reset) */

    /* Diagnostics. */
    bool trace_enabled;
    uint32_t break_lo, break_hi;    /* stop before running an instruction here */
    bool break_hit;
    void (*pc_hook)(void *ctx, uint32_t pc); /* debugging: called before every instruction (tools/mac_run -L) */
    void *pc_hook_ctx;
    /* measurements (tools/mac_run -H): called after a write of size bytes at offset of card's video RAM */
    void (*vram_hook)(void *ctx, uint32_t card, uint32_t offset, uint32_t size);
    void *vram_hook_ctx;
    uint32_t watch_lo, watch_hi;    /* debugging: stop after the instruction that writes RAM here (0 = off) */
    uint32_t insn_count;
    mac_trace_t trace[MAC_TRACE_SIZE];
    uint32_t trace_next;        /* total entries written, ring index = % size */
    uint32_t flow_block[MAC_FLOW_SIZE];     /* ROM block numbers, first-seen order */
    uint32_t flow_insn[MAC_FLOW_SIZE];      /* instruction count at first sight */
    uint32_t flow_count;
    uint32_t pmmu_pc[16];       /* the last PMMU instructions: address, */
    uint16_t pmmu_ext[16];      /* extension word */
    uint32_t pmmu_total;
    mac_access_t unimpl[MAC_UNIMPL_LOG_SIZE];
    uint32_t unimpl_total;      /* all accesses */
    uint32_t unimpl_used;       /* distinct entries in unimpl[] */

    uint8_t xpram[256];         /* parameter RAM kept by the clock chip */
    bool xpram_dirty;           /* the Mac OS changed a byte of xpram (the host saves it and clears this) */
    uint32_t clk_log[1024];      /* first clock chip commands: cmd << 8 | data, bit 31 = read */
    uint32_t clk_log_len;
    uint32_t rtc_seconds;       /* seconds since 1 Jan 1904: set by the host at the start, then counted by the model */
    uint32_t second_ticks;      /* 60 Hz ticks since rtc_seconds last advanced */

    /* mac_sysbeep: the CPU state to restore when the injected call returns. */
    bool call_active, call_returned;
    uint32_t call_return_pc;
    uint32_t call_regs[20];

    uint8_t stub[MAC_STUB_SIZE];
    mac_disk_t disk;            /* drive 8; set by the host after mac_init() */
    mac_disk_t more_disks[MAC_MAX_DISKS - 1];   /* drives 9.. (blocks == 0: absent) */
    uint8_t disk_cursor;        /* next drive the Open routine adds */
    uint8_t power_request;      /* mac_power_t: what the Mac OS asked for at its end */
    uint32_t reset_count;       /* passes through the ROM's start (2 = a restart) */
    uint8_t mount_pending;      /* bit n: drive 8 + n still has to be mounted (see poll_mount) */
    uint32_t mount_poll;

    /* Queued keyboard and mouse packets (see mac_mouse_move, mac_key). */
    uint8_t input[64][4];       /* device (2 = keyboard, 3 = mouse), 2 data bytes */
    uint32_t input_head, input_count;
    bool injecting;
    int mouse_x, mouse_y;       /* where the model thinks the pointer is */
    mac_iwm_t iwm;
    mac_scsi_t scsi;
    mac_rbv_t rbv;
    mac_dac_t dac;
    mac_asc_t asc;
    /*
     * Sound output: called with the unsigned 8-bit mono samples the sound chip
     * played since the last call (a few hundred at a time) and their rate in
     * Hz. NULL = discard.
     */
    void (*audio)(void *ctx, const uint8_t *samples, uint32_t count, uint32_t rate);
    void *audio_ctx;
    uint8_t *vram;              /* video RAM, set by the host */
    uint32_t vram_size;
    uint32_t vram_base;         /* guest address of the first byte */

    mac_card_t card[MAC_CARDS]; /* the NuBus video cards (mac_card_attach): slot $C, then $D */

    mac_via_t via1;
    uint32_t via_rem;           /* CPU cycles not yet given to the VIAs (< 32) */
    uint32_t frame_cycles;      /* CPU cycles since the last 60 Hz tick */

    mac_fsm_t fsm;              /* the external file system (emu/mac/fsm.c) */
} mac_t;

/*
 * Attach 'ram' and 'rom' to the model and select it in the CPU core. Returns
 * 0, or -1 for bad arguments (the ROM must be a power of two, >= 64 KiB).
 */
int mac_init(mac_t *mac, uint8_t *ram, uint32_t ram_size,
             const uint8_t *rom, uint32_t rom_size);

/*
 * Patch a private copy of the Mac IIci ROM (checksum 368CADFE) so that it
 * skips the hardware probing and calls into this model instead; see
 * emu/mac/rom_patch.c. 'rom' is modified in place. Returns 0, or -1 if the
 * ROM is not the supported one.
 */
int mac_rom_patch(uint8_t *rom, uint32_t size);

/* Emulated opcodes ($71xx are illegal 68k instructions, so no real code
 * uses them; the patched ROM places them where native code must run). */
#define MAC_EMUL_OP_BASE 0x7100
#define MAC_EMUL_OP_LAST 0x713E
#define MAC_EMUL_OP_RESET 0x7103
#define MAC_EMUL_OP_CLKNOMEM 0x7104
#define MAC_EMUL_OP_HWINFO 0x7105
#define MAC_EMUL_OP_PATCH_BOOT_GLOBS 0x7107
#define MAC_EMUL_OP_INSTALL_DRIVERS 0x710A
#define MAC_EMUL_OP_INPUT_POP 0x7114
#define MAC_EMUL_OP_DISK_OPEN 0x7110
#define MAC_EMUL_OP_DISK_PRIME 0x7111
#define MAC_EMUL_OP_DISK_CONTROL 0x7112
#define MAC_EMUL_OP_DISK_STATUS 0x7113
#define MAC_EMUL_OP_DISK_NEXT 0x7115
#define MAC_EMUL_OP_SCSI_DISPATCH 0x7128
#define MAC_EMUL_OP_SLOT_PROBE 0x7129
#define MAC_EMUL_OP_CARD_OPEN 0x712A
#define MAC_EMUL_OP_CARD_CONTROL 0x712B
#define MAC_EMUL_OP_CARD_STATUS 0x712C
#define MAC_EMUL_OP_CARD_IRQ 0x712D
#define MAC_EMUL_OP_CARD_CLOSE 0x712E
#define MAC_EMUL_OP_FSM_INSTALL 0x713A
#define MAC_EMUL_OP_FSM_HFS 0x713B
#define MAC_EMUL_OP_FSM_COMM 0x713C
#define MAC_EMUL_OP_FSM_STEP 0x713D
#define MAC_EMUL_OP_FSM_RESULT 0x713E

/*
 * The external file system (emu/mac/fsm.c, docs/fsm.md). mac_fsm_attach before
 * mac_reset, once per volume (up to MAC_FSM_VOLUMES; -1 when there is no
 * room): once the Finder runs, the volumes are mounted in that order. The
 * rest is for mac.c.
 */
int mac_fsm_attach(mac_t *mac, const mac_fs_ops_t *ops);
void mac_fsm_reset(mac_t *mac);
void mac_fsm_op(mac_t *mac, uint16_t op);
uint32_t mac_fsm_install_code(void);
uint32_t mac_fsm_install_return(void);

/* Native handlers of the .Disk driver (emu/mac/disk.c). They work on the
 * guest state of the emulated opcode that called them. */
void mac_disk_install(mac_t *mac);
void mac_disk_open(mac_t *mac);
void mac_disk_next(mac_t *mac);
void mac_disk_prime(mac_t *mac);
void mac_disk_control(mac_t *mac);
void mac_disk_status(mac_t *mac);

/* VIA registers: 'reg' is 0-15. */
uint32_t mac_via_read(mac_via_t *via, uint32_t reg);
void mac_via_write(mac_via_t *via, uint32_t reg, uint32_t value);
/* Advance the timers by 'ticks' VIA clock cycles. */
void mac_via_advance(mac_via_t *via, uint32_t ticks);
/* Raise an interrupt flag (VIA_INT_*). */
void mac_via_set_flag(mac_via_t *via, uint8_t mask);
/* True while the IRQ output is active. */
bool mac_via_irq(const mac_via_t *via);

#define VIA_INT_CA2 0x01
#define VIA_INT_CA1 0x02
#define VIA_INT_SR 0x04
#define VIA_INT_T2 0x20
#define VIA_INT_T1 0x40

/* RBV registers: 'reg' is the byte offset in the chip, 0-31. */
uint32_t mac_rbv_read(mac_rbv_t *rbv, uint32_t reg, bool vblank);
void mac_rbv_write(mac_rbv_t *rbv, uint32_t reg, uint32_t value);
bool mac_rbv_irq(const mac_rbv_t *rbv);
/* The sound chip's interrupt line (RBV interrupt flag bit 4). */
void mac_rbv_set_sound_irq(mac_rbv_t *rbv, bool asserted);
/* A video frame starts: the internal video interrupt becomes pending (if
 * enabled). */
void mac_rbv_vblank(mac_rbv_t *rbv);
#define RBV_SENSE_13_RGB 6      /* Apple 13" RGB, 640x480 */
#define RBV_SENSE_NONE 7        /* no monitor connected */
#define RBV_SIER_SLOTS 0x3F     /* slot interrupt enable bits of slots $9-$E */

/* DAC registers: the address register at offset 0, data at offset 4. */
uint32_t mac_dac_access(mac_dac_t *dac, uint32_t offset, bool write,
                        uint32_t value);

/* Draw the screen into 'out' (width * height 0x00RRGGBB pixels) from the
 * video RAM, depth and color table. Returns 0, or -1 if there is no video
 * RAM. */
#define MAC_SCREEN_W 640
#define MAC_SCREEN_H 480
int mac_render(const mac_t *mac, uint32_t *out);

/* ASC registers: one byte access at 'offset' from 0x50F14000. */
uint32_t mac_asc_access(mac_asc_t *asc, uint32_t offset, bool write, uint32_t value);
bool mac_asc_irq(const mac_asc_t *asc);
uint32_t mac_asc_rate(const mac_asc_t *asc);
/* Let the chip play for 'cycles' CPU cycles; calls 'audio' with what it played. */
void mac_asc_advance(mac_asc_t *asc, uint32_t cycles,
                     void (*audio)(void *ctx, const uint8_t *samples, uint32_t count, uint32_t rate),
                     void *ctx);

/* SCSI registers: one byte access at 'offset' from 0x50F10000. */
uint32_t mac_scsi_access(mac_scsi_t *scsi, uint32_t offset, bool write,
                         uint32_t value);

/* IWM registers: one byte access at 'offset' from 0x50F16000. */
uint32_t mac_iwm_access(mac_iwm_t *iwm, uint32_t offset, bool write,
                        uint32_t value);

/*
 * Make the running system call SysBeep, as an application would: the CPU
 * state is saved, "move.w #3,-(sp); _SysBeep" runs from the stub window, and
 * the state comes back when it returns (at the start of a later mac_run).
 * For tests of the sound chip; the system may be in the middle of anything,
 * so this is only meant for an idle desktop. Returns 0, or -1 if a call is
 * already under way.
 */
int mac_sysbeep(mac_t *mac);

/*
 * Plug a NuBus video card into the next free slot, $C then $D (after
 * mac_init, before mac_reset; at most MAC_CARDS):
 * 'vram' is MAC_CARD_VRAM_SIZE bytes; 'res' indexes mac_card_resolutions and
 * is the resolution the card starts in, 'depth' (0-5 = 1, 2, 4, 8, 16, 32
 * bits) the depth the Mac starts in. With the card the built-in video
 * reports "no monitor" and stays off, so the card is the only screen.
 * Returns 0, or -1 for bad arguments or when every slot is taken.
 */
int mac_card_attach(mac_t *mac, uint8_t *vram, uint32_t res, uint32_t depth);

/* Native parts of the card's driver (nubus_video.c). */
void mac_card_open(mac_t *mac);
void mac_card_control(mac_t *mac);
void mac_card_status(mac_t *mac);
void mac_card_close(mac_t *mac);
void mac_card_irq_ack(mac_t *mac);
/* The card's vertical blanking: once per 60 Hz frame. */
void mac_card_vblank(mac_t *mac);
/* Bus accesses that belong to the card: true if 'addr' is its ROM or video RAM. */
bool mac_card_read(mac_t *mac, uint32_t addr, int size, uint32_t *value);
bool mac_card_write(mac_t *mac, uint32_t addr, int size, uint32_t value);
/* Index of the resolution w x h in mac_card_resolutions, or -1. */
int mac_card_find_resolution(uint32_t width, uint32_t height);

/* Size of the Mac's screen now: the first card's mode, or the built-in 640x480. */
void mac_screen_size(const mac_t *mac, uint32_t *width, uint32_t *height);

/* Size and picture of card 'index' (0 = slot $C, 1 = $D), as mac_screen_size and mac_render_changes.
 * Return -1 if there is no such card. */
int mac_card_screen_size(const mac_t *mac, uint32_t index, uint32_t *width, uint32_t *height);

/* Bytes per row of card 'index''s picture and its bits per pixel (0 if there is no such card). */
uint32_t mac_card_row_bytes(const mac_t *mac, uint32_t index, uint32_t *bits_per_pixel);
int mac_card_render(mac_t *mac, uint32_t index, uint32_t *out, uint8_t *changed);

/*
 * Where the Mac puts card 'index' on its desktop: the top left corner of its GDevice's rectangle (the
 * main screen is at 0,0; the Monitors control panel moves the others). Returns 0, or -1 if the system has
 * no GDevice for the card (yet).
 */
int mac_card_position(mac_t *mac, uint32_t index, int32_t *x, int32_t *y);

/*
 * Draw the rows of the screen that changed since the last call into 'out'
 * (width * height 0x00RRGGBB pixels of mac_screen_size; a new size means
 * that everything is drawn). 'changed', if not NULL, gets one byte per row:
 * 1 = drawn. Returns the number of rows drawn, or -1 if there is no video
 * RAM. The built-in video is always drawn whole.
 */
int mac_render_changes(mac_t *mac, uint32_t *out, uint8_t *changed);

/* Power-on reset: overlay on, CPU reads SSP/PC from the ROM. */
int mac_reset(mac_t *mac);

/* Run for at most 'cycles' CPU cycles; returns the cycles used. */
int mac_run(mac_t *mac, int cycles);

/* The bus as the CPU sees it (also used by tests). */
uint32_t mac_bus_read(mac_t *mac, uint32_t addr, int size);
void mac_bus_write(mac_t *mac, uint32_t addr, int size, uint32_t value);

/*
 * Mouse and keyboard. There is no ADB hardware; instead the packets the ADB
 * devices would send are queued, and the model calls the ROM's own mouse and
 * keyboard handlers with them like an interrupt does (see stub.s, inject).
 *
 * mac_mouse_move: relative motion in pixels and the button state (changes in
 * the button are sent even without motion). mac_key: an ADB key code (0-0x7F,
 * "Inside Macintosh: Devices", ADB keyboard) and whether it goes down.
 */
void mac_mouse_warp(mac_t *mac, int x, int y);
void mac_mouse_move(mac_t *mac, int dx, int dy, bool button);

/*
 * Put the pointer at screen position (x, y) at once, without the mouse
 * acceleration that makes absolute positioning with relative moves
 * unreliable: the cursor position in the system's low memory (RawMouse,
 * MTemp, Mouse; Inside Macintosh: Toolbox Essentials / Mac OS Technote
 * "Moving the cursor") is set and the cursor task told to redraw (CrsrNew).
 * For scripted tests; the button state is sent with mac_mouse_move(0, 0).
 */
void mac_mouse_warp(mac_t *mac, int x, int y);
void mac_key(mac_t *mac, uint8_t adb_code, bool down);

/* The keyboard's power key (System 7 asks whether to shut down). */
void mac_power_key(mac_t *mac, bool down);

/* True once the ROM's ADB Manager has a handler for 'device'; queued input
 * is only injected after that. */
bool mac_adb_ready(mac_t *mac, uint8_t device);

/* Number of unimplemented I/O accesses seen (may exceed the log size). */
uint32_t mac_unimpl_total(const mac_t *mac);

#endif
