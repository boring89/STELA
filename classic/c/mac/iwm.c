/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#include "mac/mac.h"

/*
 * IWM ("Integrated Woz Machine") floppy controller.
 *
 * Think of eight light switches. Touching address 0x50F16000 + n * 0x200
 * flips switch n / 2 off (n even) or on (n odd); the byte that is read or
 * written there is what the switch settings select. The ROM only tests that
 * a controller is there, by writing the mode register and reading it back
 * from the status register; no drive is attached, so the sense line stays
 * high ("no disk"). Register layout: "Guide to the Macintosh Family
 * Hardware", ch. 10 (IWM); Inside Macintosh: Devices, "Disk driver".
 */

#define IWM_LINE_ENABLE 4       /* motor on */
#define IWM_LINE_Q6 6
#define IWM_LINE_Q7 7

#define IWM_MODE_MASK 0x1F
#define IWM_STATUS_SENSE 0x80
#define IWM_STATUS_ENABLED 0x20


uint32_t mac_iwm_access(mac_iwm_t *iwm, uint32_t offset, bool write,
                        uint32_t value)
{
    uint32_t n = (offset >> 9) & 0xF;
    uint32_t line = n >> 1;
    bool on = (n & 1) != 0;
    bool q6;
    bool q7;

    if (on)
    {
        iwm->lines |= 1u << line;
    }
    else
    {
        iwm->lines &= ~(1u << line);
    }
    q6 = (iwm->lines >> IWM_LINE_Q6) & 1;
    q7 = (iwm->lines >> IWM_LINE_Q7) & 1;

    if (write)
    {
        /* Q7 and Q6 set: the byte goes to the mode register. */
        if (q7 && q6)
        {
            iwm->mode = value & IWM_MODE_MASK;
        }
        return 0;
    }

    if (!q7 && q6)
    {
        /* Status register: mode in bits 0-4, "enabled" in bit 5. */
        return IWM_STATUS_SENSE | iwm->mode |
               (((iwm->lines >> IWM_LINE_ENABLE) & 1) ? IWM_STATUS_ENABLED : 0);
    }
    return 0xFF;
}
