/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#include "mac/mac.h"

#include <stddef.h>

#include "m68k/vm.h"
#include "mac/nubus_driver_code.h"
#include "shim/k_lib.h"

/*
 * A video card for NuBus slot $C (and a second one in $D) that exists only in software (see
 * docs/nubus_video.md).
 *
 * The Mac learns about a card from its declaration ROM: a list of
 * "sResources" at the top of the slot's address space that says what the
 * card is (a display), where its video RAM is, which pixel depths it has and
 * which driver to use. That ROM is built here when the card is attached. The
 * driver (nubus_driver.s) is a few 68k instructions around emulated opcodes;
 * the real work of its Control and Status calls is done by the functions
 * below.
 *
 * Formats: "Designing Cards and Drivers for the Macintosh Family", 3rd ed.,
 * ch. 8 (declaration ROM, format block, sResources, CRC) and ch. 9 (video
 * cards: the VPBlock, mode lists, the csCodes of video drivers); "Display
 * Device Driver Guide" for the Display Manager calls (cscGetNextResolution
 * and friends). Basilisk II's slot_rom.cpp and video.cpp (GPL-2.0-or-later)
 * were used as a reference for what Mac OS really asks for; no code is taken
 * from them.
 */

const mac_card_res_t mac_card_resolutions[] = {
    {640, 480},   {800, 600},   {1024, 768},  {1152, 870},  {1280, 720},
    {1280, 960},  {1280, 1024}, {1440, 900},  {1600, 1200}, {1920, 1080},
    {1920, 1200}, {2560, 1440}, {3840, 2160},
};
const uint32_t mac_card_resolution_count =
    sizeof mac_card_resolutions / sizeof mac_card_resolutions[0];

static const uint8_t depth_bits[MAC_CARD_DEPTHS] = {1, 2, 4, 8, 16, 32};

/* sResource ids and entry types (Designing Cards, ch. 8 and 9). */
#define SRSRC_BOARD 0x01
#define SRSRC_VIDEO 0x80
#define E_TYPE 1                /* sRsrcType */
#define E_NAME 2                /* sRsrcName */
#define E_DRVR_DIR 4            /* sRsrcDrvrDir */
#define E_FLAGS 7               /* sRsrcFlags */
#define E_HW_DEV_ID 8           /* sRsrcHWDevId */
#define E_MINOR_LENGTH 11      /* MinorLength: the boot code reads it (ROM 0x40800C10) */
#define E_MAJOR_BASE 12         /* MajorBaseOS: offset in the slot's super space */
#define E_MAJOR_LENGTH 13
#define E_BOARD_ID 32
#define E_VENDOR_INFO 36
#define E_GAMMA_DIR 64
#define E_PRAM_INIT 33        /* PRAMInitData: the slot's parameter RAM when it is new (ROM 0x40806202) */
#define E_VENDOR_ID 1
#define E_REV_LEVEL 3
#define E_PART_NUM 4
#define E_DATE 5
#define S_MAC_OS_68020 2        /* driver directory: the 68020 driver */
#define M_VID_PARAMS 1          /* mode list: the VPBlock */
#define M_PAGE_CNT 3
#define M_DEV_TYPE 4            /* 0 = color table, 2 = direct color */
#define F_OPEN_AT_START 0x02    /* sRsrcFlags: the Slot Manager opens the driver at startup */
#define F_32BIT_MODE 0x04       /* sRsrcFlags: dCtlDevBase is a 32-bit address */
#define END_OF_LIST 0xFF

#define CAT_BOARD 1
#define CAT_DISPLAY 3
#define TYP_VIDEO 1
#define DRSW_APPLE 1
#define DRHW_68KHAL 0x6848      /* 'hH': our own hardware id */
#define BOARD_ID 0x6848

#define TEST_PATTERN 0x5A932BC7u
#define BYTE_LANES_ALL 0x0F     /* the ROM uses all four byte lanes */
#define FORMAT_APPLE 1
#define ROM_REVISION 1

/* VPBlock (Designing Cards, "The video parameters"): offsets after its length word. */
#define VP_SIZE 46
#define VP_BASE_OFFSET 0
#define VP_ROW_BYTES 4
#define VP_BOUNDS 6
#define VP_VERSION 14
#define VP_PACK_TYPE 16
#define VP_PACK_SIZE 18
#define VP_HRES 22
#define VP_VRES 26
#define VP_PIXEL_TYPE 30
#define VP_PIXEL_SIZE 32
#define VP_CMP_COUNT 34
#define VP_CMP_SIZE 36
#define VP_PLANE_BYTES 38
#define DPI_72 0x00480000u      /* 72 dpi as a 16.16 fixed point number */


typedef struct
{
    uint8_t *b;
    uint32_t p;
} writer_t;

static void w8(writer_t *w, uint32_t v)
{
    if (w->p < MAC_CARD_ROM_MAX)
        w->b[w->p] = (uint8_t)v;
    w->p++;
}

static void w16(writer_t *w, uint32_t v)
{
    w8(w, v >> 8);
    w8(w, v);
}

static void w32(writer_t *w, uint32_t v)
{
    w16(w, v >> 16);
    w16(w, v);
}

/* An sResource entry: id and a 24-bit value. */
static void entry(writer_t *w, uint32_t id, uint32_t value)
{
    w8(w, id);
    w8(w, value >> 16);
    w8(w, value >> 8);
    w8(w, value);
}

/* An sResource entry that points to 'target': the 24-bit offset from the entry itself. */
static void entry_offset(writer_t *w, uint32_t id, uint32_t target)
{
    entry(w, id, (target - w->p) & 0xFFFFFF);
}

static void end_of_list(writer_t *w)
{
    entry(w, END_OF_LIST, 0);
}

/* A C string, padded to an even length. */
static void cstring(writer_t *w, const char *s)
{
    do
    {
        w8(w, (uint8_t)*s);
    } while (*s++ != '\0');
    if (w->p & 1)
        w8(w, 0);
}


/*
 * The deepest depth (index) a resolution offers. 32 bits per pixel at 3840x2160 (15360 bytes per row) crashes
 * System 7.5.3 when it switches to it (the heap is overwritten with pixel values; 2560x1440 at 32 bits, 10240
 * bytes per row, works), so rows longer than that stop at 16 bits.
 */
#define MAX_ROW_BYTES_32 10240u

static uint32_t max_depth(uint32_t res)
{
    return mac_card_resolutions[res].width * 4u <= MAX_ROW_BYTES_32 ? MAC_CARD_DEPTHS - 1 : MAC_CARD_DEPTHS - 2;
}


static uint32_t row_bytes(uint32_t res, uint32_t depth)
{
    /* Rounded up to whole longs: QuickDraw wants even row lengths, 4 is faster. */
    return (mac_card_resolutions[res].width * depth_bits[depth] + 31) / 32 * 4;
}


uint32_t mac_card_row_bytes(const mac_t *mac, uint32_t index, uint32_t *bits_per_pixel)
{
    if (index >= MAC_CARDS || !mac->card[index].present)
        return 0;
    if (bits_per_pixel != NULL)
        *bits_per_pixel = depth_bits[mac->card[index].depth];
    return row_bytes(mac->card[index].res, mac->card[index].depth);
}


static void put16(uint8_t *p, uint32_t v)
{
    p[0] = (uint8_t)(v >> 8);
    p[1] = (uint8_t)v;
}

static void put32(uint8_t *p, uint32_t v)
{
    put16(p, v >> 16);
    put16(p + 2, v);
}


/* Fill in the VPBlock at 'vp' (after its length) for a resolution and depth. */
static void fill_vpblock(uint8_t *vp, uint32_t res, uint32_t depth)
{
    bool direct = depth >= 4;

    k_memset(vp, 0, VP_SIZE);
    put32(vp + VP_BASE_OFFSET, 0);
    put16(vp + VP_ROW_BYTES, row_bytes(res, depth));
    put16(vp + VP_BOUNDS + 4, mac_card_resolutions[res].height);   /* bottom */
    put16(vp + VP_BOUNDS + 6, mac_card_resolutions[res].width);    /* right */
    put32(vp + VP_HRES, DPI_72);
    put32(vp + VP_VRES, DPI_72);
    put16(vp + VP_PIXEL_TYPE, direct ? 16 : 0);    /* RGBDirect or chunky indexed */
    put16(vp + VP_PIXEL_SIZE, depth_bits[depth]);
    put16(vp + VP_CMP_COUNT, direct ? 3 : 1);
    put16(vp + VP_CMP_SIZE, depth == 4 ? 5 : depth == 5 ? 8 : depth_bits[depth]);
}


/* The ROM's CRC (Designing Cards, ch. 8): rotate the sum left one bit, add the next byte; the CRC field reads as 0. */
static uint32_t rom_crc(const uint8_t *rom, uint32_t size)
{
    uint32_t crc = 0;

    for (uint32_t i = 0; i < size; i++)
    {
        uint8_t b = (i >= size - 12 && i < size - 8) ? 0 : rom[i];

        crc = ((crc << 1) | (crc >> 31)) + b;
    }
    return crc;
}


static void update_crc(mac_card_t *card)
{
    put32(card->rom + card->rom_size - 12, rom_crc(card->rom, card->rom_size));
}


/*
 * The VPBlocks of the ROM describe the current resolution (the ROM has room for one per depth). The deepest
 * mode list is the last entry of the video sResource: where the resolution does not offer it, the entry
 * becomes the end of the list, and the Monitors control panel does not show that depth.
 */
static void update_vpblocks(mac_card_t *card)
{
    for (uint32_t d = 0; d < MAC_CARD_DEPTHS; d++)
        fill_vpblock(card->rom + card->vpblock[d] + 4, card->res, d);

    card->rom[card->last_mode_entry] = max_depth(card->res) == MAC_CARD_DEPTHS - 1
                                           ? MAC_CARD_MODE_1BIT + MAC_CARD_DEPTHS - 1 : END_OF_LIST;

    update_crc(card);
}


static uint32_t build_rom(mac_card_t *card)
{
    writer_t w = {card->rom, 0};
    uint32_t board_type, board_name, vendor_id, rev_level, part_num, date;
    uint32_t vendor_info, pram_init, board, video_type, video_name, driver, driver_dir;
    uint32_t gamma, gamma_dir, modes[MAC_CARD_DEPTHS], video, directory;
    uint32_t major_base, major_length;

    /* The board sResource: what the card is called and who made it. */
    board_type = w.p;
    w16(&w, CAT_BOARD);
    w16(&w, 0);
    w16(&w, 0);
    w16(&w, 0);
    board_name = w.p;
    cstring(&w, "68k_HAL Video Card");
    vendor_id = w.p;
    cstring(&w, "68k_HAL");
    rev_level = w.p;
    cstring(&w, "1.0");
    part_num = w.p;
    cstring(&w, "68KHAL-VIDEO");
    date = w.p;
    cstring(&w, "2026-10-18");

    vendor_info = w.p;
    entry_offset(&w, E_VENDOR_ID, vendor_id);
    entry_offset(&w, E_REV_LEVEL, rev_level);
    entry_offset(&w, E_PART_NUM, part_num);
    entry_offset(&w, E_DATE, date);
    end_of_list(&w);

    /*
     * The slot's parameter RAM starts from this block (length, two reserved bytes, vendorUse1-6). The
     * ROM's boot code takes vendorUse1 as the video mode to start in (ROM 0x40800DE0, sReadPRAMRec), so
     * the card starts in the depth the host chose.
     */
    pram_init = w.p;
    w32(&w, 12);
    w16(&w, 0);
    w8(&w, MAC_CARD_MODE_1BIT + card->depth);
    for (uint32_t i = 0; i < 5; i++)
        w8(&w, 0);

    board = w.p;
    entry_offset(&w, E_TYPE, board_type);
    entry_offset(&w, E_NAME, board_name);
    entry(&w, E_BOARD_ID, BOARD_ID);
    entry_offset(&w, E_PRAM_INIT, pram_init);
    entry_offset(&w, E_VENDOR_INFO, vendor_info);
    end_of_list(&w);

    /* The video sResource. */
    video_type = w.p;
    w16(&w, CAT_DISPLAY);
    w16(&w, TYP_VIDEO);
    w16(&w, DRSW_APPLE);
    w16(&w, DRHW_68KHAL);
    video_name = w.p;
    cstring(&w, "Display_Video_Apple_68kHAL");

    /* The driver: a block (length including itself, then the DRVR). */
    driver = w.p;
    w32(&w, 4 + sizeof mac_nubus_driver_code);
    for (uint32_t i = 0; i < sizeof mac_nubus_driver_code; i++)
        w8(&w, mac_nubus_driver_code[i]);
    if (w.p & 1)
        w8(&w, 0);
    driver_dir = w.p;
    entry_offset(&w, S_MAC_OS_68020, driver);
    end_of_list(&w);

    /* A linear gamma table: a block (length, resource id, name) around a
     * GammaTbl (version, type, formula size, channels, entries, bits, data). */
    gamma = w.p;
    w32(&w, 0);                     /* length, filled in below */
    w16(&w, 0);
    cstring(&w, "Linear Gamma");
    card->gamma = w.p;
    w16(&w, 0);
    w16(&w, 0);
    w16(&w, 0);
    w16(&w, 1);
    w16(&w, 256);
    w16(&w, 8);
    for (uint32_t i = 0; i < 256; i++)
        w8(&w, i);
    put32(card->rom + gamma, w.p - gamma);
    gamma_dir = w.p;
    entry_offset(&w, 0x80, gamma);
    end_of_list(&w);

    /* Where the video RAM is: offset 0 of the slot's super space, 32 MiB. */
    major_base = w.p;
    w32(&w, 0);
    major_length = w.p;
    w32(&w, MAC_CARD_VRAM_SIZE);

    /* One VPBlock and mode list per depth. */
    for (uint32_t d = 0; d < MAC_CARD_DEPTHS; d++)
    {
        card->vpblock[d] = w.p;
        w32(&w, 4 + VP_SIZE);
        for (uint32_t i = 0; i < VP_SIZE; i++)
            w8(&w, 0);
    }
    for (uint32_t d = 0; d < MAC_CARD_DEPTHS; d++)
    {
        modes[d] = w.p;
        entry_offset(&w, M_VID_PARAMS, card->vpblock[d]);
        entry(&w, M_PAGE_CNT, 1);
        entry(&w, M_DEV_TYPE, d >= 4 ? 2 : 0);
        end_of_list(&w);
    }

    video = w.p;
    entry_offset(&w, E_TYPE, video_type);
    entry_offset(&w, E_NAME, video_name);
    entry_offset(&w, E_DRVR_DIR, driver_dir);
    entry(&w, E_FLAGS, F_OPEN_AT_START | F_32BIT_MODE);
    entry(&w, E_HW_DEV_ID, 1);
    entry_offset(&w, E_MINOR_LENGTH, major_length);
    entry_offset(&w, E_MAJOR_BASE, major_base);
    entry_offset(&w, E_MAJOR_LENGTH, major_length);
    entry_offset(&w, E_GAMMA_DIR, gamma_dir);
    for (uint32_t d = 0; d < MAC_CARD_DEPTHS; d++)
    {
        card->last_mode_entry = w.p;
        entry_offset(&w, MAC_CARD_MODE_1BIT + d, modes[d]);
    }
    end_of_list(&w);

    directory = w.p;
    entry_offset(&w, SRSRC_BOARD, board);
    entry_offset(&w, SRSRC_VIDEO, video);
    end_of_list(&w);

    /* The format block: the last 20 bytes of the ROM. */
    entry_offset(&w, 0, directory);
    w32(&w, w.p + 16);              /* length of the whole ROM */
    w32(&w, 0);                     /* CRC, filled in by update_crc */
    w8(&w, ROM_REVISION);
    w8(&w, FORMAT_APPLE);
    w32(&w, TEST_PATTERN);
    w8(&w, 0);
    w8(&w, BYTE_LANES_ALL);
    return w.p;
}


static uint32_t rom_base(const mac_card_t *card)
{
    return MAC_CARD_ROM_END_OF(card->slot) - card->rom_size;
}


/* Every color table entry gray: what a card shows before the system sets its colors. */
static void gray_clut(mac_card_t *card)
{
    for (uint32_t i = 0; i < 256; i++)
    {
        card->clut[i][0] = card->clut[i][1] = card->clut[i][2] = 0x80;
    }
    card->all_dirty = true;
}


int mac_card_find_resolution(uint32_t width, uint32_t height)
{
    for (uint32_t i = 0; i < mac_card_resolution_count; i++)
    {
        if (mac_card_resolutions[i].width == width && mac_card_resolutions[i].height == height)
            return (int)i;
    }
    return -1;
}


int mac_card_attach(mac_t *mac, uint8_t *vram, uint32_t res, uint32_t depth)
{
    uint32_t index = 0;

    while (index < MAC_CARDS && mac->card[index].present)
        index++;
    if (index == MAC_CARDS || vram == NULL || res >= mac_card_resolution_count || depth > max_depth(res))
        return -1;

    mac_card_t *card = &mac->card[index];

    k_memset(card, 0, sizeof *card);
    card->slot = MAC_CARD_SLOT + index;
    card->vram = vram;
    card->res = card->preferred_res = res;
    card->depth = card->preferred_depth = depth;
    card->rom_size = build_rom(card);
    if (card->rom_size > MAC_CARD_ROM_MAX)
        return -1;

    /* The ROM was built from offset 0; the Mac reads it from the end of the slot space down. */
    update_vpblocks(card);
    gray_clut(card);
    card->present = true;
    return 0;
}


static uint32_t bus_get(const uint8_t *p, int size)
{
    uint32_t v = 0;

    for (int i = 0; i < size; i++)
        v = (v << 8) | p[i];
    return v;
}


/* The card whose slot spaces hold 'addr' (super space 0xs0000000 or standard space 0xFsxxxxxx). */
static mac_card_t *card_at(mac_t *mac, uint32_t addr)
{
    uint32_t slot = (addr >> 28) == 0xF ? (addr >> 24) & 0xF : addr >> 28;

    for (uint32_t i = 0; i < MAC_CARDS; i++)
    {
        if (mac->card[i].present && mac->card[i].slot == slot)
            return &mac->card[i];
    }
    return NULL;
}


bool mac_card_read(mac_t *mac, uint32_t addr, int size, uint32_t *value)
{
    mac_card_t *card = card_at(mac, addr);

    if (card == NULL)
        return false;

    if (addr - MAC_CARD_VRAM_BASE_OF(card->slot) < MAC_CARD_VRAM_SIZE)
    {
        uint32_t off = addr - MAC_CARD_VRAM_BASE_OF(card->slot);

        *value = off + (uint32_t)size <= MAC_CARD_VRAM_SIZE ? bus_get(card->vram + off, size) : 0;
        return true;
    }
    if (addr >= rom_base(card) && addr < MAC_CARD_ROM_END_OF(card->slot))
    {
        uint32_t off = addr - rom_base(card);

        *value = off + (uint32_t)size <= card->rom_size ? bus_get(card->rom + off, size) : 0;
        return true;
    }
    return false;
}


bool mac_card_write(mac_t *mac, uint32_t addr, int size, uint32_t value)
{
    mac_card_t *card = card_at(mac, addr);

    if (card == NULL)
        return false;

    uint32_t off = addr - MAC_CARD_VRAM_BASE_OF(card->slot);

    if (off >= MAC_CARD_VRAM_SIZE)
        return false;

    if (off + (uint32_t)size > MAC_CARD_VRAM_SIZE)
        return true;

    for (int i = size - 1; i >= 0; i--)
    {
        card->vram[off + (uint32_t)i] = (uint8_t)value;
        value >>= 8;
    }
    uint32_t first = off >> MAC_CARD_DIRTY_SHIFT;
    uint32_t last = (off + (uint32_t)size - 1) >> MAC_CARD_DIRTY_SHIFT;

    card->dirty[first >> 5] |= 1u << (first & 31);
    card->dirty[last >> 5] |= 1u << (last & 31);
    if (mac->vram_hook != NULL)
        mac->vram_hook(mac->vram_hook_ctx, (uint32_t)(card - mac->card), off, (uint32_t)size);
    return true;
}


/* ---- The driver's Control and Status calls ---- */

/* Parameter block and DCE fields (Inside Macintosh: Devices). */
#define PB_CS_CODE 26
#define PB_CS_PARAM 28
#define DCE_REFNUM 24
#define DCE_SLOT 40
#define DCE_DEV_BASE 42

/* Result codes. */
#define NO_ERR 0
#define CONTROL_ERR (-17)
#define STATUS_ERR (-18)
#define PARAM_ERR (-50)

/* Control codes and status codes of video drivers (Designing Cards, ch. 9; Display Device Driver Guide). */
#define CSC_SET_MODE 2
#define CSC_SET_ENTRIES 3
#define CSC_SET_GAMMA 4
#define CSC_GRAY_PAGE 5
#define CSC_SET_GRAY 6
#define CSC_SET_INTERRUPT 7
#define CSC_DIRECT_SET_ENTRIES 8
#define CSC_SET_DEFAULT_MODE 9
#define CSC_SWITCH_MODE 10
#define CSC_SAVE_PREFERRED 16

#define CSC_GET_MODE 2
#define CSC_GET_ENTRIES 3
#define CSC_GET_PAGES 4
#define CSC_GET_BASE_ADDR 5
#define CSC_GET_GRAY 6
#define CSC_GET_INTERRUPT 7
#define CSC_GET_GAMMA 8
#define CSC_GET_DEFAULT_MODE 9
#define CSC_GET_CUR_MODE 10
#define CSC_GET_CONNECTION 12
#define CSC_GET_MODE_TIMING 13
#define CSC_GET_MODE_BASE_ADDR 14
#define CSC_GET_PREFERRED 16
#define CSC_GET_NEXT_RESOLUTION 17
#define CSC_GET_VIDEO_PARAMETERS 18
#define CSC_GET_MULTI_CONNECT 28

/* VDSwitchInfo / VDPageInfo: csMode (word), csData (long), csPage (word), csBaseAddr (long). */
#define CS_MODE 0
#define CS_DATA 2
#define CS_PAGE 6
#define CS_BASE_ADDR 8
/* VDSetEntryRecord: csTable (pointer to ColorSpecs: value, red, green, blue words), csStart, csCount. */
#define CS_TABLE 0
#define CS_START 4
#define CS_COUNT 6

/* Display Manager ids: "no more resolutions" and "give me the first one". */
#define RES_ID_END 0xFFFFFFFDu
#define RES_ID_FIRST 0xFFFFFFFEu

/* cscGetConnection: a "modeless" display (any mode is fine), all modes valid and safe. */
#define DISPLAY_MODELESS 8
#define CONNECT_FLAGS 0x43
#define TIMING_DECL 0x6465636Cu     /* 'decl': the timing comes from the declaration ROM */
#define TIMING_VALID_SAFE_SHOWN 0x0B
#define TIMING_DEFAULT 0x04


static uint32_t rd8(mac_t *mac, uint32_t a) { return mac_bus_read(mac, a, 1); }
static uint32_t rd16(mac_t *mac, uint32_t a) { return mac_bus_read(mac, a, 2); }
static uint32_t rd32(mac_t *mac, uint32_t a) { return mac_bus_read(mac, a, 4); }
static void wr8(mac_t *mac, uint32_t a, uint32_t v) { mac_bus_write(mac, a, 1, v); }
static void wr16(mac_t *mac, uint32_t a, uint32_t v) { mac_bus_write(mac, a, 2, v); }
static void wr32(mac_t *mac, uint32_t a, uint32_t v) { mac_bus_write(mac, a, 4, v); }


static bool valid_mode(uint32_t mode, uint32_t res)
{
    return mode >= MAC_CARD_MODE_1BIT && mode <= MAC_CARD_MODE_1BIT + max_depth(res);
}


static int res_from_id(uint32_t id)
{
    return id >= MAC_CARD_RES_ID_BASE && id - MAC_CARD_RES_ID_BASE < mac_card_resolution_count
               ? (int)(id - MAC_CARD_RES_ID_BASE) : -1;
}


static void switch_mode(mac_card_t *card, uint32_t res, uint32_t depth)
{
    if (res == card->res && depth == card->depth)
        return;

    card->res = res;
    card->depth = depth;
    update_vpblocks(card);
    gray_clut(card);
    card->mode_changes++;
}


static void set_entries(mac_t *mac, mac_card_t *card, uint32_t param)
{
    uint32_t table = rd32(mac, param + CS_TABLE);
    uint32_t start = rd16(mac, param + CS_START);
    uint32_t count = rd16(mac, param + CS_COUNT);

    for (uint32_t i = 0; i <= count; i++, table += 8)
    {
        uint32_t index = start == 0xFFFF ? rd16(mac, table) & 0xFF : start + i;
        uint32_t r = rd16(mac, table + 2) >> 8;
        uint32_t g = rd16(mac, table + 4) >> 8;
        uint32_t b = rd16(mac, table + 6) >> 8;

        if (card->gray)
        {
            /* Luminance (Rec. 601 weights, as fractions of 65536). */
            r = g = b = (r * 19595 + g * 38470 + b * 7471) >> 16;
        }
        card->clut[index][0] = (uint8_t)r;
        card->clut[index][1] = (uint8_t)g;
        card->clut[index][2] = (uint8_t)b;
    }
    card->all_dirty = true;
}


static int entries_ok(mac_t *mac, uint32_t param)
{
    uint32_t start = rd16(mac, param + CS_START);
    uint32_t count = rd16(mac, param + CS_COUNT);

    if (rd32(mac, param + CS_TABLE) == 0 || count > 255 || (start != 0xFFFF && start + count > 255))
        return PARAM_ERR;
    return NO_ERR;
}


/* cscGrayPage: the 50% gray dither of the desktop before anything is drawn. */
static void gray_page(mac_card_t *card)
{
    static const uint32_t pattern[MAC_CARD_DEPTHS] = {
        0xAAAAAAAAu, 0xCCCCCCCCu, 0xF0F0F0F0u, 0xFF00FF00u, 0xFFFF0000u, 0xFFFFFFFFu,
    };
    uint32_t rb = row_bytes(card->res, card->depth);
    uint32_t h = mac_card_resolutions[card->res].height;
    uint32_t pat = pattern[card->depth];

    for (uint32_t y = 0; y < h; y++)
    {
        uint8_t *row = card->vram + y * rb;
        uint32_t p = pat;

        for (uint32_t x = 0; x < rb; x += 4)
        {
            put32(row + x, p);
            if (card->depth == 5)
                p = ~p;     /* 32 bits: one pixel per long, so alternate them */
        }
        pat = ~pat;
    }
    if (card->depth >= 4)
    {
        for (uint32_t i = 0; i < 256; i++)
            card->clut[i][0] = card->clut[i][1] = card->clut[i][2] = (uint8_t)i;
    }
    card->all_dirty = true;
}


static int control(mac_t *mac, mac_card_t *card, uint32_t code, uint32_t param, uint32_t dce)
{
    uint32_t base = MAC_CARD_VRAM_BASE_OF(card->slot);

    switch (code)
    {
    case CSC_SET_MODE:
    {
        uint32_t mode = rd16(mac, param + CS_MODE);

        wr32(mac, param + CS_BASE_ADDR, base);
        if (!valid_mode(mode, card->res) || rd16(mac, param + CS_PAGE) != 0)
            return PARAM_ERR;
        switch_mode(card, card->res, mode - MAC_CARD_MODE_1BIT);
        return NO_ERR;
    }
    case CSC_SET_ENTRIES:
    case CSC_DIRECT_SET_ENTRIES:
        if ((code == CSC_SET_ENTRIES) == (card->depth >= 4))
            return CONTROL_ERR;
        if (entries_ok(mac, param) != NO_ERR)
            return PARAM_ERR;
        set_entries(mac, card, param);
        return NO_ERR;
    case CSC_SET_GAMMA:
        return NO_ERR;      /* the gamma is always linear */
    case CSC_GRAY_PAGE:
        if (rd16(mac, param + CS_PAGE) != 0)
            return PARAM_ERR;
        gray_page(card);
        return NO_ERR;
    case CSC_SET_GRAY:
        card->gray = rd8(mac, param + CS_MODE) != 0;
        return NO_ERR;
    case CSC_SET_INTERRUPT:
        card->irq_enabled = rd8(mac, param + CS_MODE) == 0;
        if (!card->irq_enabled)
        {
            card->irq_pending = false;
            mac->rbv.slot_irq &= (uint8_t)~MAC_CARD_SLOT_IRQ_OF(card->slot);
        }
        return NO_ERR;
    case CSC_SET_DEFAULT_MODE:
        if (valid_mode(rd8(mac, param + CS_MODE), card->res))
            card->preferred_depth = rd8(mac, param + CS_MODE) - MAC_CARD_MODE_1BIT;
        return NO_ERR;
    case CSC_SWITCH_MODE:
    {
        uint32_t mode = rd16(mac, param + CS_MODE);
        int res = res_from_id(rd32(mac, param + CS_DATA));

        wr32(mac, param + CS_BASE_ADDR, base);
        if (res < 0 || !valid_mode(mode, (uint32_t)res) || rd16(mac, param + CS_PAGE) != 0)
            return PARAM_ERR;
        switch_mode(card, (uint32_t)res, mode - MAC_CARD_MODE_1BIT);
        wr32(mac, dce + DCE_DEV_BASE, base);
        return NO_ERR;
    }
    case CSC_SAVE_PREFERRED:
    {
        int res = res_from_id(rd32(mac, param + CS_DATA));
        uint32_t mode = rd16(mac, param + CS_MODE);

        if (res < 0 || !valid_mode(mode, (uint32_t)res))
            return PARAM_ERR;
        card->preferred_res = (uint32_t)res;
        card->preferred_depth = mode - MAC_CARD_MODE_1BIT;
        return NO_ERR;
    }
    default:
        return CONTROL_ERR;
    }
}


static void write_resolution(mac_t *mac, uint32_t param, uint32_t res)
{
    /* VDResolutionInfo: previous id, id, width, height, refresh (16.16), max depth mode, flags. */
    wr32(mac, param + 4, MAC_CARD_RES_ID_BASE + res);
    wr32(mac, param + 8, mac_card_resolutions[res].width);
    wr32(mac, param + 12, mac_card_resolutions[res].height);
    wr32(mac, param + 16, 60u << 16);
    wr16(mac, param + 20, MAC_CARD_MODE_1BIT + max_depth(res));
    wr32(mac, param + 22, 0);
}


static void write_connection(mac_t *mac, uint32_t info)
{
    /* VDDisplayConnectInfo: type (word), tagged type and data (bytes), flags, component. */
    wr16(mac, info, DISPLAY_MODELESS);
    wr8(mac, info + 2, 0);
    wr8(mac, info + 3, 0);
    wr32(mac, info + 4, CONNECT_FLAGS);
    wr32(mac, info + 8, 0);
}


static int status(mac_t *mac, mac_card_t *card, uint32_t code, uint32_t param)
{
    uint32_t base = MAC_CARD_VRAM_BASE_OF(card->slot);

    switch (code)
    {
    case CSC_GET_MODE:
        wr16(mac, param + CS_MODE, MAC_CARD_MODE_1BIT + card->depth);
        wr16(mac, param + CS_PAGE, 0);
        wr32(mac, param + CS_BASE_ADDR, base);
        return NO_ERR;
    case CSC_GET_ENTRIES:
    {
        uint32_t table = rd32(mac, param + CS_TABLE);
        uint32_t start = rd16(mac, param + CS_START);
        uint32_t count = rd16(mac, param + CS_COUNT);

        if (entries_ok(mac, param) != NO_ERR)
            return PARAM_ERR;
        for (uint32_t i = 0; i <= count; i++, table += 8)
        {
            uint32_t index = start == 0xFFFF ? rd16(mac, table) & 0xFF : start + i;

            for (uint32_t c = 0; c < 3; c++)
                wr16(mac, table + 2 + 2 * c, card->clut[index][c] * 0x0101u);
        }
        return NO_ERR;
    }
    case CSC_GET_PAGES:
        wr16(mac, param + CS_PAGE, 1);
        return NO_ERR;
    case CSC_GET_BASE_ADDR:
        wr32(mac, param + CS_BASE_ADDR, base);
        return rd16(mac, param + CS_PAGE) == 0 ? NO_ERR : PARAM_ERR;
    case CSC_GET_GRAY:
        wr8(mac, param + CS_MODE, card->gray ? 1 : 0);
        return NO_ERR;
    case CSC_GET_INTERRUPT:
        wr8(mac, param + CS_MODE, card->irq_enabled ? 0 : 1);
        return NO_ERR;
    case CSC_GET_GAMMA:
        wr32(mac, param, rom_base(card) + card->gamma);
        return NO_ERR;
    case CSC_GET_DEFAULT_MODE:
        wr8(mac, param + CS_MODE, MAC_CARD_MODE_1BIT + card->preferred_depth);
        return NO_ERR;
    case CSC_GET_CUR_MODE:
        wr16(mac, param + CS_MODE, MAC_CARD_MODE_1BIT + card->depth);
        wr32(mac, param + CS_DATA, MAC_CARD_RES_ID_BASE + card->res);
        wr16(mac, param + CS_PAGE, 0);
        wr32(mac, param + CS_BASE_ADDR, base);
        return NO_ERR;
    case CSC_GET_CONNECTION:
        write_connection(mac, param);
        return NO_ERR;
    case CSC_GET_MODE_TIMING:
    {
        /* VDTimingInfo: mode id, reserved, format, data, flags. */
        uint32_t id = rd32(mac, param);
        int res = res_from_id(id);

        if (res < 0)
            return PARAM_ERR;
        wr32(mac, param + 8, TIMING_DECL);
        wr32(mac, param + 12, 0);
        wr32(mac, param + 16, TIMING_VALID_SAFE_SHOWN |
                                  ((uint32_t)res == card->preferred_res ? TIMING_DEFAULT : 0));
        return NO_ERR;
    }
    case CSC_GET_MODE_BASE_ADDR:
        wr32(mac, param + CS_BASE_ADDR, base);
        return NO_ERR;
    case CSC_GET_PREFERRED:
        wr16(mac, param + CS_MODE, MAC_CARD_MODE_1BIT + card->preferred_depth);
        wr32(mac, param + CS_DATA, MAC_CARD_RES_ID_BASE + card->preferred_res);
        return NO_ERR;
    case CSC_GET_NEXT_RESOLUTION:
    {
        uint32_t previous = rd32(mac, param);
        int res;

        if (previous == 0)
            res = (int)card->res;
        else if (previous == RES_ID_FIRST)
            res = 0;
        else if ((res = res_from_id(previous)) < 0)
            return PARAM_ERR;
        else if ((uint32_t)++res >= mac_card_resolution_count)
        {
            wr32(mac, param + 4, RES_ID_END);
            return NO_ERR;
        }
        write_resolution(mac, param, (uint32_t)res);
        return NO_ERR;
    }
    case CSC_GET_VIDEO_PARAMETERS:
    {
        /* VDVideoParametersInfo: mode id, depth mode (word), VPBlock pointer, page count, device type. */
        int res = res_from_id(rd32(mac, param));
        uint32_t mode = rd16(mac, param + 4);
        uint32_t vp = rd32(mac, param + 6);
        uint8_t block[VP_SIZE];

        if (res < 0 || !valid_mode(mode, (uint32_t)res))
            return PARAM_ERR;
        fill_vpblock(block, (uint32_t)res, mode - MAC_CARD_MODE_1BIT);
        for (uint32_t i = 0; i < 42; i++)       /* the VPBlock without its reserved long */
            wr8(mac, vp + i, block[i]);
        wr32(mac, param + 10, 1);
        wr32(mac, param + 14, mode - MAC_CARD_MODE_1BIT >= 4 ? 2 : 0);
        return NO_ERR;
    }
    case CSC_GET_MULTI_CONNECT:
    {
        uint32_t which = rd32(mac, param);

        if (which == 0xFFFFFFFFu)
        {
            wr32(mac, param, 1);
            return NO_ERR;
        }
        if (which != 1)
            return PARAM_ERR;
        write_connection(mac, param + 4);
        return NO_ERR;
    }
    default:
        return STATUS_ERR;
    }
}


/* The card of the driver whose DCE is 'dce' (dCtlSlot); the first card if the slot is unknown. */
static mac_card_t *card_of_dce(mac_t *mac, uint32_t dce)
{
    uint32_t slot = rd8(mac, dce + DCE_SLOT);

    for (uint32_t i = 0; i < MAC_CARDS; i++)
    {
        if (mac->card[i].present && mac->card[i].slot == slot)
            return &mac->card[i];
    }
    return &mac->card[0];
}


void mac_card_open(mac_t *mac)
{
    uint32_t dce = m68k_vm_get_reg(M68K_VM_REG_A0 + 1);
    mac_card_t *card = card_of_dce(mac, dce);

    /* The video RAM's 32-bit address (the Slot Manager should have set it from MajorBaseOS already). */
    wr32(mac, dce + DCE_DEV_BASE, MAC_CARD_VRAM_BASE_OF(card->slot));
    card->refnum = (uint16_t)rd16(mac, dce + DCE_REFNUM);
    card->irq_enabled = true;
    m68k_vm_set_reg(M68K_VM_REG_D0, NO_ERR);
}


static void stop_interrupts(mac_t *mac, mac_card_t *card)
{
    card->irq_enabled = false;
    card->irq_pending = false;
    mac->rbv.slot_irq &= (uint8_t)~MAC_CARD_SLOT_IRQ_OF(card->slot);
}


void mac_card_close(mac_t *mac)
{
    stop_interrupts(mac, card_of_dce(mac, m68k_vm_get_reg(M68K_VM_REG_A0 + 1)));
}


static void driver_call(mac_t *mac, bool is_control)
{
    uint32_t pb = m68k_vm_get_reg(M68K_VM_REG_A0);
    uint32_t dce = m68k_vm_get_reg(M68K_VM_REG_A0 + 1);
    uint32_t code = rd16(mac, pb + PB_CS_CODE);
    uint32_t param = rd32(mac, pb + PB_CS_PARAM);
    mac_card_t *card = card_of_dce(mac, dce);
    int result = is_control ? control(mac, card, code, param, dce) : status(mac, card, code, param);

    m68k_vm_set_reg(M68K_VM_REG_D0, (uint32_t)result);
}


void mac_card_control(mac_t *mac)
{
    driver_call(mac, true);
}


void mac_card_status(mac_t *mac)
{
    driver_call(mac, false);
}


void mac_card_vblank(mac_t *mac)
{
    for (uint32_t i = 0; i < MAC_CARDS; i++)
    {
        mac_card_t *card = &mac->card[i];

        if (card->present && card->irq_enabled)
        {
            card->irq_pending = true;
            mac->rbv.slot_irq |= (uint8_t)MAC_CARD_SLOT_IRQ_OF(card->slot);
        }
    }
}


/* The interrupt handler's acknowledge: A1 = its sqParm, the slot. */
void mac_card_irq_ack(mac_t *mac)
{
    uint32_t slot = m68k_vm_get_reg(M68K_VM_REG_A0 + 1);

    for (uint32_t i = 0; i < MAC_CARDS; i++)
    {
        if (mac->card[i].present && mac->card[i].slot == slot)
        {
            mac->card[i].irq_pending = false;
            mac->rbv.slot_irq &= (uint8_t)~MAC_CARD_SLOT_IRQ_OF(slot);
        }
    }
}


/* ---- The picture ---- */

int mac_card_screen_size(const mac_t *mac, uint32_t index, uint32_t *width, uint32_t *height)
{
    if (index >= MAC_CARDS || !mac->card[index].present)
        return -1;
    *width = mac_card_resolutions[mac->card[index].res].width;
    *height = mac_card_resolutions[mac->card[index].res].height;
    return 0;
}


void mac_screen_size(const mac_t *mac, uint32_t *width, uint32_t *height)
{
    if (mac_card_screen_size(mac, 0, width, height) != 0)
    {
        *width = MAC_SCREEN_W;
        *height = MAC_SCREEN_H;
    }
}


static bool row_dirty(const mac_card_t *card, uint32_t start, uint32_t length)
{
    uint32_t first = start >> MAC_CARD_DIRTY_SHIFT;
    uint32_t last = (start + length - 1) >> MAC_CARD_DIRTY_SHIFT;

    for (uint32_t b = first; b <= last; b++)
    {
        if (card->dirty[b >> 5] & (1u << (b & 31)))
            return true;
    }
    return false;
}


static void convert_row(const mac_card_t *card, const uint8_t *src, uint32_t *out, uint32_t width)
{
    uint32_t bpp = depth_bits[card->depth];

    if (bpp == 32)
    {
        for (uint32_t x = 0; x < width; x++, src += 4)
            out[x] = (uint32_t)src[1] << 16 | (uint32_t)src[2] << 8 | src[3];
    }
    else if (bpp == 16)
    {
        for (uint32_t x = 0; x < width; x++, src += 2)
        {
            uint32_t v = (uint32_t)src[0] << 8 | src[1];
            uint32_t r = (v >> 10) & 31, g = (v >> 5) & 31, b = v & 31;

            out[x] = ((r << 3 | r >> 2) << 16) | ((g << 3 | g >> 2) << 8) | (b << 3 | b >> 2);
        }
    }
    else
    {
        uint32_t mask = (1u << bpp) - 1;

        for (uint32_t x = 0; x < width; x++)
        {
            uint32_t bit = x * bpp;
            uint32_t index = (src[bit >> 3] >> (8 - bpp - (bit & 7))) & mask;
            const uint8_t *c = card->clut[index];

            out[x] = (uint32_t)c[0] << 16 | (uint32_t)c[1] << 8 | c[2];
        }
    }
}


int mac_render_changes(mac_t *mac, uint32_t *out, uint8_t *changed)
{
    if (!mac->card[0].present)
    {
        if (mac_render(mac, out) != 0)
            return -1;
        if (changed != NULL)
            k_memset(changed, 1, MAC_SCREEN_H);
        return MAC_SCREEN_H;
    }
    return mac_card_render(mac, 0, out, changed);
}


int mac_card_render(mac_t *mac, uint32_t index, uint32_t *out, uint8_t *changed)
{
    if (index >= MAC_CARDS || !mac->card[index].present)
        return -1;

    mac_card_t *card = &mac->card[index];

    uint32_t width = mac_card_resolutions[card->res].width;
    uint32_t height = mac_card_resolutions[card->res].height;
    uint32_t rb = row_bytes(card->res, card->depth);
    uint32_t used = (width * depth_bits[card->depth] + 7) / 8;
    int drawn = 0;

    for (uint32_t y = 0; y < height; y++)
    {
        bool draw = card->all_dirty || row_dirty(card, y * rb, used);

        if (draw)
        {
            convert_row(card, card->vram + y * rb, out + y * width, width);
            drawn++;
        }
        if (changed != NULL)
            changed[y] = draw;
    }
    k_memset(card->dirty, 0, sizeof card->dirty);
    card->all_dirty = false;
    return drawn;
}


/* QuickDraw's list of screens: low memory DeviceList, handle -> GDevice; gdRefNum +0, gdNextGD +0x1E,
 * gdRect +0x22 (top, left, bottom, right) (Inside Macintosh: Imaging With QuickDraw, "GDevice"). */
#define LM_DEVICE_LIST 0x8A8
#define GD_REFNUM 0x00
#define GD_NEXT 0x1E
#define GD_RECT 0x22

int mac_card_position(mac_t *mac, uint32_t index, int32_t *x, int32_t *y)
{
    if (index >= MAC_CARDS || !mac->card[index].present || mac->card[index].refnum == 0)
        return -1;

    uint32_t handle = rd32(mac, LM_DEVICE_LIST);

    for (int n = 0; handle != 0 && handle < mac->ram_size && n < 16; n++)
    {
        uint32_t gd = rd32(mac, handle);

        if (gd == 0 || gd >= mac->ram_size)
            return -1;
        if (rd16(mac, gd + GD_REFNUM) == mac->card[index].refnum)
        {
            *y = (int16_t)rd16(mac, gd + GD_RECT);
            *x = (int16_t)rd16(mac, gd + GD_RECT + 2);
            return 0;
        }
        handle = rd32(mac, gd + GD_NEXT);
    }
    return -1;
}
