/* Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The kernel is linked at the start of the last 2 GiB of the address space
 * (bootinfo::KERNEL_BASE). Each output segment starts on a 4 KiB page so the
 * loader can map it with its own permissions (W^X):
 *   text   R-X   code
 *   rodata R--   constants, unwind tables
 *   data   RW-   initialised data and .bss
 */
ENTRY(_start)

PHDRS
{
    text   PT_LOAD FLAGS(5);   /* PF_R | PF_X */
    rodata PT_LOAD FLAGS(4);   /* PF_R */
    data   PT_LOAD FLAGS(6);   /* PF_R | PF_W */
}

SECTIONS
{
    . = 0xFFFFFFFF80000000;
    __kernel_start = .;

    .text : ALIGN(4K)
    {
        *(.text .text.*)
    } :text

    . = ALIGN(4K);
    .rodata :
    {
        *(.rodata .rodata.*)
    } :rodata
    .eh_frame_hdr : { *(.eh_frame_hdr) } :rodata
    .eh_frame : { KEEP(*(.eh_frame)) } :rodata

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

    . = ALIGN(4K);
    __kernel_end = .;

    /DISCARD/ :
    {
        *(.comment)
        *(.note .note.*)
    }
}
