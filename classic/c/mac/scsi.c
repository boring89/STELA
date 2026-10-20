/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#include "mac/mac.h"

/*
 * NCR 5380 SCSI bus controller, with nothing attached to the bus.
 *
 * The ROM goes through the usual steps to reach a disk: wait for the bus to
 * be free, arbitrate (the chip says "arbitration in progress" and we never
 * lose), select a target ID and wait for it to answer by raising BSY. Nobody
 * answers, so every selection times out and the ROM moves on. Registers are
 * 0x10 bytes apart; layout from the NCR 5380 data sheet ("Guide to the
 * Macintosh Family Hardware", ch. 11, lists the Mac addresses).
 */

#define REG_DATA 0
#define REG_ICR 1               /* initiator command / read: status */
#define REG_MODE 2
#define REG_TCR 3
#define REG_SER 4               /* select enable / read: current bus status */
#define REG_BUS 5               /* bus and status */
#define REG_COUNT 8

#define ICR_SEL 0x04
#define ICR_BSY 0x08
#define ICR_AIP 0x40            /* arbitration in progress */
#define MODE_ARBITRATE 0x01

/* Bus status register: BSY is bit 6, SEL is bit 1. */
#define STATUS_BSY 0x40
#define STATUS_SEL 0x02


uint32_t mac_scsi_access(mac_scsi_t *scsi, uint32_t offset, bool write,
                         uint32_t value)
{
    uint32_t reg = (offset >> 4) & (REG_COUNT - 1);

    if (write)
    {
        scsi->reg[reg] = value & 0xFF;
        return 0;
    }

    switch (reg)
    {
    case REG_ICR:
        /* Arbitration "starts" at once and is never lost. */
        return (scsi->reg[REG_ICR] & ~ICR_AIP) |
               ((scsi->reg[REG_MODE] & MODE_ARBITRATE) ? ICR_AIP : 0);
    case REG_SER:
    {
        /* Only what the ROM itself drives is on the bus. */
        uint8_t icr = scsi->reg[REG_ICR];

        return ((icr & ICR_BSY) ? STATUS_BSY : 0) |
               ((icr & ICR_SEL) ? STATUS_SEL : 0);
    }
    case REG_BUS:
        return 0;
    default:
        return scsi->reg[reg];
    }
}
