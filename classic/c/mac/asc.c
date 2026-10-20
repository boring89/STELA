/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#include "mac.h"

#include "shim/k_lib.h"

/*
 * The Apple Sound Chip (ASC), reduced to its FIFO mode: the one the
 * Sound Manager uses for SysBeep and sampled sounds. Register layout
 * (Guide to the Macintosh Family Hardware, ch. 6, and the layout used by
 * MAME's apple/asc.cpp, which the details marked "(guess)" below follow):
 *
 *   0x000-0x3FF  FIFO A (or wave table 0/1 in wave table mode)
 *   0x400-0x7FF  FIFO B (wave tables 2/3)
 *   0x800  version      the original ASC reads 0
 *   0x801  mode         0 = off, 1 = FIFO, 2 = wave table
 *   0x802  control      bit 0: analog output, bit 1: stereo (not used here)
 *   0x803  FIFO mode    bit 7: clear both FIFOs
 *   0x804  FIFO status  bit 0: A half empty, bit 1: A full,
 *                       bit 2: B half empty, bit 3: B full; reading clears
 *                       them. The Sound Manager's fill loop (found by
 *                       disassembling System 7.5.3) writes bytes until bit 3
 *                       shows up, so the "full" flags are set when a FIFO
 *                       fills; MAME's apple/asc.cpp has the same layout.
 *   0x806  volume
 *   0x807  clock rate
 *
 * In FIFO mode the chip takes one byte from FIFO A and one from FIFO B for
 * every sample period and plays them as unsigned 8-bit samples (0x80 =
 * silence). The software refills the FIFOs when the "half empty" flag raises
 * the sound interrupt (RBV interrupt flag bit 4). Wave table mode is not
 * emulated; its writes only change the stored bytes.
 */

#define ASC_VERSION 0x800
#define ASC_MODE 0x801
#define ASC_CONTROL 0x802
#define ASC_FIFO_MODE 0x803
#define ASC_FIFO_STATUS 0x804
#define ASC_WAVE_CONTROL 0x805
#define ASC_VOLUME 0x806
#define ASC_CLOCK 0x807

#define MODE_FIFO 1
#define FIFO_SIZE 0x400
#define FIFO_HALF (FIFO_SIZE / 2)

#define STATUS_A_HALF 0x01
#define STATUS_A_FULL 0x02
#define STATUS_B_HALF 0x04
#define STATUS_B_FULL 0x08
#define STATUS_IRQ_MASK (STATUS_A_HALF | STATUS_B_HALF)

/* Sample rates for the clock register (guess: 0 is the Mac's 22254.5 Hz; the others are the usual halves/doubles). */
static uint32_t sample_rate(uint8_t clock)
{
    switch (clock & 3)
    {
    case 2:
        return 44100;
    case 3:
        return 11127;
    default:
        return 22254;
    }
}


uint32_t mac_asc_rate(const mac_asc_t *asc)
{
    return sample_rate(asc->clock);
}


bool mac_asc_irq(const mac_asc_t *asc)
{
    /* Only "half empty" interrupts; the "full" flags are for polling. */
    return (asc->status & STATUS_IRQ_MASK) != 0;
}


static void fifo_push(mac_asc_t *asc, int which, uint8_t value)
{
    if (asc->fifo_count[which] >= FIFO_SIZE)
    {
        return;     /* full: the byte is lost */
    }
    asc->fifo[which][(asc->fifo_head[which] + asc->fifo_count[which]) % FIFO_SIZE] = value;
    asc->fifo_count[which]++;

    if (asc->fifo_count[which] == FIFO_SIZE)
    {
        asc->status |= which == 0 ? STATUS_A_FULL : STATUS_B_FULL;
    }
}


/* Take the next sample of a FIFO (0x80 when it is empty); sets the status flags. */
static uint8_t fifo_pop(mac_asc_t *asc, int which)
{
    uint8_t value = 0x80;

    if (asc->fifo_count[which] > 0)
    {
        value = asc->fifo[which][asc->fifo_head[which]];
        asc->fifo_head[which] = (uint16_t)((asc->fifo_head[which] + 1) % FIFO_SIZE);
        asc->fifo_count[which]--;

        if (asc->fifo_count[which] == FIFO_HALF - 1)
        {
            asc->status |= which == 0 ? STATUS_A_HALF : STATUS_B_HALF;
        }
    }
    return value;
}


uint32_t mac_asc_access(mac_asc_t *asc, uint32_t offset, bool write, uint32_t value)
{
    offset &= 0xFFF;

    if (offset < 0x800)
    {
        int which = offset >= FIFO_SIZE ? 1 : 0;

        if (asc->mode == MODE_FIFO)
        {
            if (write)
            {
                fifo_push(asc, which, (uint8_t)value);
                return 0;
            }
            return 0;       /* the FIFOs cannot be read back */
        }
        if (write)
        {
            asc->wave[offset] = (uint8_t)value;
            return 0;
        }
        return asc->wave[offset];
    }

    switch (offset)
    {
    case ASC_VERSION:
        return 0;
    case ASC_MODE:
        if (write)
        {
            asc->mode = (uint8_t)value;
            asc->phase = 0;
        }
        return asc->mode;
    case ASC_CONTROL:
        if (write)
        {
            asc->control = (uint8_t)value;
        }
        return asc->control;
    case ASC_FIFO_MODE:
        if (write)
        {
            asc->fifo_mode = (uint8_t)value;

            if (value & 0x80)
            {
                k_memset(asc->fifo_count, 0, sizeof asc->fifo_count);
                k_memset(asc->fifo_head, 0, sizeof asc->fifo_head);
            }
        }
        return asc->fifo_mode;
    case ASC_FIFO_STATUS:
        if (!write)
        {
            uint8_t status = asc->status;

            asc->status = 0;
            return status;
        }
        return 0;
    case ASC_WAVE_CONTROL:
        if (write)
        {
            asc->wave_control = (uint8_t)value;
        }
        return asc->wave_control;
    case ASC_VOLUME:
        if (write)
        {
            asc->volume = (uint8_t)value;
        }
        return asc->volume;
    case ASC_CLOCK:
        if (write)
        {
            asc->clock = (uint8_t)value;
        }
        return asc->clock;
    default:
        return write ? 0 : asc->other[offset & 0x7F];
    }
}


void mac_asc_advance(mac_asc_t *asc, uint32_t cycles, void (*audio)(void *ctx, const uint8_t *samples,
                     uint32_t count, uint32_t rate), void *ctx)
{
    if (asc->mode != MODE_FIFO)
    {
        return;
    }

    /* One sample every MAC_CPU_HZ / rate cycles; asc->phase is the fractional position. */
    uint32_t rate = sample_rate(asc->clock);
    uint64_t phase = asc->phase + (uint64_t)cycles * rate;
    uint8_t out[512];
    uint32_t n = 0;

    while (phase >= MAC_CPU_HZ)
    {
        phase -= MAC_CPU_HZ;

        uint8_t a = fifo_pop(asc, 0);
        uint8_t b = fifo_pop(asc, 1);

        /* Mono output: the average of the channels that have data, so a mono
         * sound in FIFO A (B silent) is not halved. */
        uint8_t sample = asc->fifo_count[1] == 0 && b == 0x80 ? a : (uint8_t)(((uint32_t)a + b) / 2);

        if (n < sizeof out)
        {
            out[n++] = sample;
        }
    }
    asc->phase = (uint32_t)phase;

    if (n > 0 && audio != NULL)
    {
        audio(ctx, out, n, rate);
    }
}
