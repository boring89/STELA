/* Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * User programs are linked at abi::layout::PROGRAM_BASE (4 MiB). Each
 * output segment starts on a 4 KiB page so the kernel can map it with its
 * own permissions (W^X):
 *   text   R-X   code
 *   rodata R--   constants
 *   data   RW-   initialised data and .bss
 * The STELA note (libstela's .note.stela) sits in rodata and has its own
 * PT_NOTE header: the kernel runs programs without it as Linux programs.
 */
ENTRY(_start)

PHDRS
{
    text   PT_LOAD FLAGS(5);   /* PF_R | PF_X */
    rodata PT_LOAD FLAGS(4);   /* PF_R */
    data   PT_LOAD FLAGS(6);   /* PF_R | PF_W */
    note   PT_NOTE FLAGS(4);
}

SECTIONS
{
    . = 0x400000;

    .text : ALIGN(4K)
    {
        *(.text .text.*)
    } :text

    . = ALIGN(4K);
    .rodata :
    {
        *(.rodata .rodata.*)
    } :rodata
    .note.stela : ALIGN(4)
    {
        KEEP(*(.note.stela))
    } :rodata :note

    . = ALIGN(4K);
    .data :
    {
        *(.data .data.*)
        *(.got .got.*)
    } :data
    .bss :
    {
        *(.bss .bss.*)
        *(COMMON)
    } :data

    /DISCARD/ :
    {
        *(.eh_frame .eh_frame_hdr)
        *(.comment)
        *(.note .note.*)
    }
}
