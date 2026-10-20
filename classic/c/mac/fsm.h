/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#ifndef EMU_MAC_FSM_H
#define EMU_MAC_FSM_H

#include <stdbool.h>
#include <stdint.h>

/*
 * An external file system for the Mac OS (System 7.5's File System Manager,
 * docs/fsm.md): a volume whose files come from the host through the
 * callbacks below, not from blocks of a disk. The 68k half is emu/mac/fsm.s,
 * the File Manager's side is emu/mac/fsm.c. Up to MAC_FSM_VOLUMES volumes;
 * one without the writing callbacks is read-only.
 */

#define MAC_FS_ROOT_PARENT 1u   /* the root folder's parent ID */
#define MAC_FS_ROOT_ID 2u       /* the root folder's ID */
#define MAC_FS_FIRST_ID 16u     /* IDs below this are reserved (fsUsrCNID) */
#define MAC_FS_NAME_MAX 31u

/* One file or folder as the Mac sees it. */
typedef struct
{
    uint32_t id, parent;
    bool is_dir;
    uint8_t name[MAC_FS_NAME_MAX + 1];  /* Pascal string, Mac Roman */
    uint32_t data_length, rsrc_length;  /* files */
    uint32_t valence;                   /* folders: how many items inside */
    uint8_t finfo[16];                  /* FInfo (files) or DInfo (folders) */
    uint8_t fxinfo[16];                 /* FXInfo or DXInfo */
    uint32_t created, modified;         /* seconds since 1904 */
    uint32_t backed_up;
    bool locked;                        /* files: may not be changed */
} mac_fs_item_t;

/* A volume's size and contents. */
typedef struct
{
    uint64_t total_bytes, free_bytes;
    uint32_t files, folders;
    bool read_only;
} mac_fs_volume_t;

/* Mac OS errors the writing callbacks return (Inside Macintosh: Files, "Result Codes"). */
#define MAC_FS_DSK_FUL_ERR (-34)
#define MAC_FS_IO_ERR (-36)
#define MAC_FS_BD_NAM_ERR (-37)
#define MAC_FS_FNF_ERR (-43)
#define MAC_FS_WPR_ERR (-44)
#define MAC_FS_FLCKD_ERR (-45)
#define MAC_FS_FBSY_ERR (-47)
#define MAC_FS_DUP_FN_ERR (-48)
#define MAC_FS_DIR_NF_ERR (-120)
#define MAC_FS_BAD_MOV_ERR (-122)

/*
 * The host's side. Every call returns 0 (or the byte count for read) on
 * success, -1 if there is no such item (or the read fails).
 */
typedef struct
{
    void *ctx;
    const char *volume_name;    /* Mac Roman, at most 27 characters */
    int (*get)(void *ctx, uint32_t id, mac_fs_item_t *item);
    /* The item called name (Mac Roman, length bytes; letters compared without case) in folder parent. */
    int (*lookup)(void *ctx, uint32_t parent, const uint8_t *name, uint32_t length, mac_fs_item_t *item);
    /* Item number index (from 1) of folder parent. */
    int (*child)(void *ctx, uint32_t parent, uint32_t index, mac_fs_item_t *item);
    int32_t (*read)(void *ctx, uint32_t id, bool resource_fork, uint32_t offset, void *buffer, uint32_t length);

    /* Size, free space and counts (NULL: counted from the root folder, read-only). 0, or -1. */
    int (*volume)(void *ctx, mac_fs_volume_t *volume);

    /*
     * Writing; NULL for a read-only volume. Each returns 0 (write: the byte
     * count) or one of the MAC_FS_..._ERR codes. Names are Mac Roman.
     */
    int (*create)(void *ctx, uint32_t parent, const uint8_t *name, uint32_t length, bool is_dir,
                  mac_fs_item_t *item);
    /* A file, or an empty folder (MAC_FS_FBSY_ERR if it is not). */
    int (*remove)(void *ctx, uint32_t id);
    /* Into folder new_parent, called name (NULL: the name it has). */
    int (*move)(void *ctx, uint32_t id, uint32_t new_parent, const uint8_t *name, uint32_t length);
    /* finfo, fxinfo, the three dates and locked of item->id. */
    int (*set_info)(void *ctx, const mac_fs_item_t *item);
    int32_t (*write)(void *ctx, uint32_t id, bool resource_fork, uint32_t offset, const void *buffer,
                     uint32_t length);
    int (*set_length)(void *ctx, uint32_t id, bool resource_fork, uint32_t length);
} mac_fs_ops_t;

#define MAC_FSM_VOLUMES 2       /* volumes one model shows */
#define MAC_FSM_FRAMES 4        /* nested calls (the install's VolumeMount calls the HFS procedure) */
#define MAC_FSM_CALLS 32        /* trap calls one File Manager request may make */

/* One call of the 68k half in progress (fsm.c). */
typedef struct
{
    uint8_t kind;
    uint16_t select;
    uint32_t vcb, pb, globals;
    uint32_t count;             /* trap calls made so far */
    uint32_t next;              /* the next one while replaying */
    uint32_t d0[MAC_FSM_CALLS], a0[MAC_FSM_CALLS];
} mac_fsm_frame_t;

/* One volume of ours. */
typedef struct
{
    const mac_fs_ops_t *ops;
    bool mounted;
    uint32_t vcb;
    int16_t drive, vrefnum;
    int16_t mount_error;        /* its PBVolumeMount's result */
} mac_fsm_volume_t;

typedef struct
{
    mac_fsm_volume_t volume[MAC_FSM_VOLUMES];
    uint32_t volumes;           /* how many are attached (0: no file system) */
    bool install_pending;       /* inject the install once the Finder runs */
    uint32_t data;              /* the plug-in's block in the system heap (FSDRec, drives, scratch) */
    int16_t install_error;      /* the install's result: 0, an OSErr, or -1/-2 no FSM 1.2, -3 no memory */
    bool overflow;              /* calls nested deeper than MAC_FSM_FRAMES */
    uint32_t restore_addr, restore_value;   /* a long to put back after the call being made */
    uint32_t depth;
    mac_fsm_frame_t frame[MAC_FSM_FRAMES];
    uint32_t requests;          /* File Manager requests served */
    struct
    {
        uint16_t select;
        int16_t result;
        uint8_t permission;     /* ioPermssn (opens) */
    } log[64];                  /* the last requests, for diagnostics (index = request number % 64) */
} mac_fsm_t;

#endif
