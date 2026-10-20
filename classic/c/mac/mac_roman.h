/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#ifndef EMU_MAC_MAC_ROMAN_H
#define EMU_MAC_MAC_ROMAN_H

#include <stdint.h>

/*
 * Mac Roman, the Mac's character set for names, and UTF-8. Bytes below 0x80
 * are ASCII; each byte from 0x80 is one Unicode character (all precomposed,
 * so a converted name is already in NFC). Table: tools/gen_mac_roman.py.
 */

/* The Unicode character of Mac Roman byte c. */
uint32_t mac_roman_to_unicode(uint8_t c);

/* The Mac Roman byte of Unicode character u, or -1 when Mac Roman has none. */
int mac_roman_from_unicode(uint32_t u);

/*
 * The next character of a UTF-8 string, from *s up to end; *s moves past it.
 * Returns the character, or 0xFFFFFFFF for a malformed sequence (one byte is
 * skipped).
 */
uint32_t mac_roman_utf8_next(const uint8_t **s, const uint8_t *end);

/* Mac Roman byte c as UTF-8 in out (1 to 3 bytes); returns the length. */
uint32_t mac_roman_to_utf8(uint8_t c, uint8_t out[3]);

#endif
