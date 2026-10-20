/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#include "mac/mac.h"

#include <stddef.h>

/*
 * The Mac IIci's built-in video: the RBV chip (its video registers), the
 * color lookup table (DAC) and the video RAM.
 *
 * Think of the screen as a grid of numbers in the video RAM. With 8 bits per
 * pixel each number picks one of 256 colors from the lookup table; with fewer
 * bits several pixels share a byte. The ROM's own video driver sets the
 * depth through RBV register 0x10 and writes the table through the DAC; this
 * file only keeps those values and turns the RAM into RGB pixels.
 *
 * RBV register use as seen in the ROM's video driver (declaration ROM at the
 * end of the ROM image): 0x02 interrupt flags with vertical blank in bit 6,
 * 0x10 depth in bits 0-2 and the monitor sense lines in bits 3-5, 0x12
 * interrupt enable. See also "Guide to the Macintosh Family Hardware",
 * "Built-in video".
 */

/*
 * Register use, from the ROM's video driver and slot interrupt code and the
 * Linux m68k header mac_via.h (rSIFR = 0x02, rIFR = 0x03, rSIER = 0x12,
 * rIER = 0x13):
 *   0x02  slot interrupt flags, active low: bit 6 is the built-in video's
 *         vertical blanking (the driver polls it: 1 -> 0 at the blanking)
 *   0x03  interrupt flags, active high: bit 1 = a slot interrupt is pending,
 *         bit 4 = sound chip; bit 7 = any enabled one
 *   0x10  depth in bits 0-2, monitor sense in bits 3-5
 *   0x12  slot interrupt enable (bit 7 set/clear, bit 6 = built-in video);
 *         writing bit 6 again also re-arms the video interrupt
 *   0x13  interrupt enable (bit 7 set/clear)
 */
#define RBV_REG_SIFR 0x02
#define RBV_REG_IFR 0x03
#define RBV_REG_VIDEO 0x10
#define RBV_REG_SIER 0x12
#define RBV_REG_IER 0x13
#define RBV_REG_MASK 0x1F

#define RBV_DEPTH_MASK 0x07
#define RBV_SENSE_SHIFT 3
#define SIFR_VIDEO 0x40
#define SIFR_SLOTS 0x3F         /* bits 0-5: NuBus slots $9-$E */
#define IFR_SLOT 0x02
#define IFR_SOUND 0x10
#define IFR_ANY 0x80
#define SET_BIT 0x80
#define REG_MASK 0x7F


/* Interrupt flags register: computed from the state of the sources. */
static uint8_t rbv_ifr(const mac_rbv_t *rbv)
{
    uint8_t ifr = 0;

    if ((rbv->vblank_latched && (rbv->sier & SIFR_VIDEO)) || (rbv->slot_irq & rbv->sier & SIFR_SLOTS))
    {
        ifr |= IFR_SLOT;
    }
    if (rbv->sound_irq)
    {
        ifr |= IFR_SOUND;
    }
    if (ifr & rbv->rier & REG_MASK)
    {
        ifr |= IFR_ANY;
    }
    return ifr;
}


bool mac_rbv_irq(const mac_rbv_t *rbv)
{
    return (rbv_ifr(rbv) & rbv->rier & REG_MASK) != 0;
}


void mac_rbv_set_sound_irq(mac_rbv_t *rbv, bool asserted)
{
    rbv->sound_irq = asserted;
}


void mac_rbv_vblank(mac_rbv_t *rbv)
{
    rbv->vblank_latched = true;
}


uint32_t mac_rbv_read(mac_rbv_t *rbv, uint32_t reg, bool vblank)
{
    reg &= RBV_REG_MASK;
    switch (reg)
    {
    case RBV_REG_SIFR:
        /* Active low: 0 while the frame is being blanked, 0 for a slot whose card interrupts. */
        return (0xBF | (vblank ? 0 : SIFR_VIDEO)) & ~(uint32_t)rbv->slot_irq;
    case RBV_REG_IFR:
        return rbv_ifr(rbv);
    case RBV_REG_VIDEO:
        return (uint32_t)(rbv->sense << RBV_SENSE_SHIFT) |
               (rbv->depth & RBV_DEPTH_MASK);
    case RBV_REG_SIER:
        return rbv->sier | SET_BIT;
    case RBV_REG_IER:
        return rbv->rier | SET_BIT;
    default:
        return rbv->other[reg];
    }
}


static uint8_t enable_bits(uint8_t old, uint8_t v)
{
    return (v & SET_BIT) ? (old | (v & REG_MASK)) : (old & ~v);
}


void mac_rbv_write(mac_rbv_t *rbv, uint32_t reg, uint32_t value)
{
    uint8_t v = value & 0xFF;

    reg &= RBV_REG_MASK;
    switch (reg)
    {
    case RBV_REG_IFR:
        break;      /* flags follow their sources */
    case RBV_REG_VIDEO:
        rbv->depth = v & RBV_DEPTH_MASK;
        break;
    case RBV_REG_SIER:
        rbv->sier = enable_bits(rbv->sier, v);
        if (v & SIFR_VIDEO)
        {
            rbv->vblank_latched = false;
        }
        break;
    case RBV_REG_IER:
        rbv->rier = enable_bits(rbv->rier, v);
        break;
    default:
        rbv->other[reg] = v;
        break;
    }
}


#define DAC_REG_ADDRESS 0
#define DAC_REG_DATA 4

uint32_t mac_dac_access(mac_dac_t *dac, uint32_t offset, bool write,
                        uint32_t value)
{
    uint32_t reg = offset & 0xC;

    if (reg == DAC_REG_ADDRESS)
    {
        if (write)
        {
            dac->index = (uint8_t)value;
            dac->phase = 0;
            return 0;
        }
        return dac->index;
    }
    if (reg == DAC_REG_DATA)
    {
        uint32_t v = dac->rgb[dac->index][dac->phase];

        if (write)
        {
            dac->rgb[dac->index][dac->phase] = (uint8_t)value;
        }
        /* After blue the index moves on to the next entry. */
        if (++dac->phase == 3)
        {
            dac->phase = 0;
            dac->index++;
        }
        return v;
    }
    return 0;
}


int mac_render(const mac_t *mac, uint32_t *out)
{
    uint32_t bpp = 1u << (mac->rbv.depth & 3);      /* 1, 2, 4 or 8 */
    uint32_t row_bytes = MAC_SCREEN_W * bpp / 8;

    if (mac->vram == NULL || mac->vram_size < row_bytes * MAC_SCREEN_H)
    {
        return -1;
    }
    for (uint32_t y = 0; y < MAC_SCREEN_H; y++)
    {
        const uint8_t *row = mac->vram + y * row_bytes;

        for (uint32_t x = 0; x < MAC_SCREEN_W; x++)
        {
            uint32_t bit = x * bpp;
            uint32_t shift = 8 - bpp - (bit & 7);
            uint32_t index = (row[bit >> 3] >> shift) & ((1u << bpp) - 1);
            const uint8_t *c;

            if (bpp == 8)
            {
                c = mac->dac.rgb[index];
            }
            else
            {
                /* The ROM stores the table of shallow depths at the top of
                 * the 256 entries: index 0 is white, the last is black. */
                c = mac->dac.rgb[(index == (1u << bpp) - 1) ? 255 : index];
            }
            out[y * MAC_SCREEN_W + x] = (uint32_t)c[0] << 16 |
                                        (uint32_t)c[1] << 8 | c[2];
        }
    }
    return 0;
}
