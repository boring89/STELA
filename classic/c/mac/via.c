/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#include "mac/mac.h"

/*
 * MOS 6522 Versatile Interface Adapter (VIA): two 8-bit ports, two timers, a
 * shift register and an interrupt controller. The Mac uses VIA1 for the
 * ROM overlay, the clock chip lines, ADB and the 60 Hz tick.
 *
 * Think of the interrupt part as a row of doorbell buttons (IFR, one per
 * source) with a mute switch for each (IER). The CPU hears one bell, the
 * "any unmuted button is pressed" line. Register map and behavior: MOS
 * 6522 data sheet; "Guide to the Macintosh Family Hardware", ch. 3.
 */

#define REG_ORB 0
#define REG_ORA 1
#define REG_DDRB 2
#define REG_DDRA 3
#define REG_T1CL 4
#define REG_T1CH 5
#define REG_T1LL 6
#define REG_T1LH 7
#define REG_T2CL 8
#define REG_T2CH 9
#define REG_SR 10
#define REG_ACR 11
#define REG_PCR 12
#define REG_IFR 13
#define REG_IER 14
#define REG_ORA_NH 15

#define IFR_ANY 0x80
#define ACR_T1_FREE_RUN 0x40
#define IER_SET 0x80


/* IFR bit 7 is derived: set while an enabled interrupt is pending. */
bool mac_via_irq(const mac_via_t *via)
{
    return (via->ifr & via->ier & 0x7F) != 0;
}


void mac_via_set_flag(mac_via_t *via, uint8_t mask)
{
    via->ifr |= mask;
}


uint32_t mac_via_read(mac_via_t *via, uint32_t reg)
{
    switch (reg)
    {
    case REG_ORB:
        return (via->orb & via->ddrb) | (via->in_b & ~via->ddrb);
    case REG_ORA:
    case REG_ORA_NH:
        via->ifr &= ~(VIA_INT_CA1 | VIA_INT_CA2);
        return (via->ora & via->ddra) | (via->in_a & ~via->ddra);
    case REG_DDRB:
        return via->ddrb;
    case REG_DDRA:
        return via->ddra;
    case REG_T1CL:
        via->ifr &= ~VIA_INT_T1;
        return via->t1_counter & 0xFF;
    case REG_T1CH:
        return via->t1_counter >> 8;
    case REG_T1LL:
        return via->t1_latch & 0xFF;
    case REG_T1LH:
        return via->t1_latch >> 8;
    case REG_T2CL:
        via->ifr &= ~VIA_INT_T2;
        return via->t2_counter & 0xFF;
    case REG_T2CH:
        return via->t2_counter >> 8;
    case REG_SR:
        via->ifr &= ~VIA_INT_SR;
        return via->sr;
    case REG_ACR:
        return via->acr;
    case REG_PCR:
        return via->pcr;
    case REG_IFR:
        return via->ifr | (mac_via_irq(via) ? IFR_ANY : 0);
    default:
        return via->ier | IER_SET;
    }
}


void mac_via_write(mac_via_t *via, uint32_t reg, uint32_t value)
{
    uint8_t v = value & 0xFF;

    switch (reg)
    {
    case REG_ORB:
        via->orb = v;
        break;
    case REG_ORA:
    case REG_ORA_NH:
        via->ora = v;
        via->ifr &= ~(VIA_INT_CA1 | VIA_INT_CA2);
        break;
    case REG_DDRB:
        via->ddrb = v;
        break;
    case REG_DDRA:
        via->ddra = v;
        break;
    case REG_T1CL:
    case REG_T1LL:
        via->t1_latch = (via->t1_latch & 0xFF00) | v;
        break;
    case REG_T1CH:
        /* Writing the high byte loads the counter and starts the timer. */
        via->t1_latch = (via->t1_latch & 0x00FF) | (v << 8);
        via->t1_counter = via->t1_latch;
        via->t1_running = true;
        via->ifr &= ~VIA_INT_T1;
        break;
    case REG_T1LH:
        via->t1_latch = (via->t1_latch & 0x00FF) | (v << 8);
        via->ifr &= ~VIA_INT_T1;
        break;
    case REG_T2CL:
        via->t2_latch = (via->t2_latch & 0xFF00) | v;
        break;
    case REG_T2CH:
        via->t2_latch = (via->t2_latch & 0x00FF) | (v << 8);
        via->t2_counter = via->t2_latch;
        via->t2_running = true;
        via->ifr &= ~VIA_INT_T2;
        break;
    case REG_SR:
        via->sr = v;
        via->ifr &= ~VIA_INT_SR;
        break;
    case REG_ACR:
        via->acr = v;
        break;
    case REG_PCR:
        via->pcr = v;
        break;
    case REG_IFR:
        via->ifr &= ~(v & 0x7F);
        break;
    default:
        if (v & IER_SET)
        {
            via->ier |= v & 0x7F;
        }
        else
        {
            via->ier &= ~v;
        }
        break;
    }
}


void mac_via_advance(mac_via_t *via, uint32_t ticks)
{
    if (via->t1_running)
    {
        if (ticks > via->t1_counter)
        {
            /* Timed out: the counter then keeps counting from the latch
             * (free run) or from 0xFFFF (one shot). */
            uint32_t rest = ticks - via->t1_counter - 1;

            via->ifr |= VIA_INT_T1;
            if (via->acr & ACR_T1_FREE_RUN)
            {
                uint32_t period = via->t1_latch + 2u;

                via->t1_counter = (uint16_t)(via->t1_latch - rest % period);
            }
            else
            {
                via->t1_running = false;
                via->t1_counter = (uint16_t)(0xFFFF - rest);
            }
        }
        else
        {
            via->t1_counter = (uint16_t)(via->t1_counter - ticks);
        }
    }
    else
    {
        via->t1_counter = (uint16_t)(via->t1_counter - ticks);
    }

    if (via->t2_running)
    {
        if (ticks > via->t2_counter)
        {
            via->ifr |= VIA_INT_T2;
            via->t2_running = false;
            via->t2_counter = (uint16_t)(0xFFFF - (ticks - via->t2_counter - 1));
        }
        else
        {
            via->t2_counter = (uint16_t)(via->t2_counter - ticks);
        }
    }
    else
    {
        via->t2_counter = (uint16_t)(via->t2_counter - ticks);
    }
}
