/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#ifndef EMU_MAC_FS_PATHS_H
#define EMU_MAC_FS_PATHS_H

#include <stdbool.h>
#include <stdint.h>

#include "fsm.h"

/*
 * A host folder tree that is reached by paths (the HAL's VFS, a folder of the
 * host running tools/mac_run), turned into the ID-based items of
 * mac_fs_ops_t. Paths are relative to the tree's top: "" is the top, "a/b"
 * below it. The Mac sees each host name (UTF-8) in Mac Roman: ':' as '/',
 * characters Mac Roman does not have as '_'; a changed or shortened name ends in
 * "#<ID>" so that it stays unique. Names starting with '.' are hidden.
 * Read-only.
 */

typedef struct
{
    void *ctx;
    /*
     * Entry number index (from 0) of folder path: its name (NUL-terminated,
     * at most name_size - 1 bytes). 0, or -1 past the end.
     */
    int (*entry)(void *ctx, const char *path, uint32_t index, char *name, uint32_t name_size);
    /* Whether path is a folder, its size and modification time (seconds since 1970). 0, or -1. */
    int (*stat)(void *ctx, const char *path, bool *is_dir, uint32_t *size, uint32_t *mtime);
    /* Bytes read, or -1. */
    int32_t (*read)(void *ctx, const char *path, uint32_t offset, void *buffer, uint32_t length);
} mac_fs_host_t;

#define MAC_FS_PATH_ITEMS 2048      /* items the Mac has seen (each keeps its ID) */
#define MAC_FS_HOST_NAME 64         /* longest host name kept (longer ones are hidden) */
#define MAC_FS_PATH_MAX 512

typedef struct
{
    mac_fs_ops_t ops;               /* give &paths->ops to mac_fsm_attach */
    const mac_fs_host_t *host;
    uint32_t count;
    struct
    {
        uint32_t parent;
        char name[MAC_FS_HOST_NAME];
    } item[MAC_FS_PATH_ITEMS];      /* ID = MAC_FS_FIRST_ID + index */
} mac_fs_paths_t;

/* Set up paths for host; volume_name is the Mac's name of the volume (kept, not copied). */
void mac_fs_paths_init(mac_fs_paths_t *paths, const mac_fs_host_t *host, const char *volume_name);

/*
 * The Mac name of a host name (see above) as a Pascal string; id is used for
 * the "#<ID>" ending. Exposed for the tests.
 */
void mac_fs_paths_mac_name(const char *host_name, uint32_t id, uint8_t out[MAC_FS_NAME_MAX + 1]);

#endif
