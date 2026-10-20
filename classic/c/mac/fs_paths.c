/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#include "fs_paths.h"
#include "mac_roman.h"

#include "shim/k_lib.h"
#include "shim/k_lib.h"

#define UNIX_TO_MAC 2082844800u     /* seconds from 1 Jan 1904 to 1 Jan 1970 */

/* Extensions shown as SimpleText documents ('TEXT', 'ttxt'); everything else is '????'. */
static const char *const text_extensions[] = {
    "txt", "c", "h", "s", "asm", "md", "lua", "cfg", "conf", "sh", "py", "mk", "json", "csv", "log", "ini",
};

static bool hidden(const char *name)
{
    return name[0] == '.' || name[0] == '\0' || k_strlen(name) >= MAC_FS_HOST_NAME;
}

static char lower(char c)
{
    return c >= 'A' && c <= 'Z' ? (char)(c + 'a' - 'A') : c;
}

static bool is_text(const char *name)
{
    const char *dot = NULL;

    for (const char *s = name; *s != '\0'; s++)
    {
        if (*s == '.')
            dot = s;
    }
    if (dot == NULL)
        return false;
    for (size_t i = 0; i < sizeof text_extensions / sizeof text_extensions[0]; i++)
    {
        const char *e = text_extensions[i];
        const char *s = dot + 1;

        while (*e != '\0' && lower(*s) == *e)
        {
            e++;
            s++;
        }
        if (*e == '\0' && *s == '\0')
            return true;
    }
    return false;
}

void mac_fs_paths_mac_name(const char *host_name, uint32_t id, uint8_t out[MAC_FS_NAME_MAX + 1])
{
    uint8_t buf[256];
    uint32_t n = 0;
    bool changed = false;

    const uint8_t *end = (const uint8_t *)host_name + k_strlen(host_name);

    for (const uint8_t *s = (const uint8_t *)host_name; s < end && n < sizeof buf;)
    {
        uint32_t u = mac_roman_utf8_next(&s, end);
        int roman = mac_roman_from_unicode(u);
        uint8_t c = roman < 0 ? '_' : (uint8_t)roman;

        if (roman < 0)
        {
            changed = true;     /* a character Mac Roman does not have */
        }
        else if (c == ':')
        {
            c = '/';            /* ':' separates the Mac's path names */
        }
        else if (c < 0x20 || c == 0x7F)
        {
            c = '_';
            changed = true;
        }
        buf[n++] = c;
    }
    if (n > MAC_FS_NAME_MAX)
        changed = true;
    if (changed)
    {
        char suffix[12];
        uint32_t len = 0;
        char digits[10];
        uint32_t d = 0;

        do
        {
            digits[d++] = (char)('0' + id % 10);
            id /= 10;
        } while (id != 0 && d < sizeof digits);
        suffix[len++] = '#';
        while (d > 0)
            suffix[len++] = digits[--d];
        if (n > MAC_FS_NAME_MAX - len)
            n = MAC_FS_NAME_MAX - len;
        k_memcpy(buf + n, suffix, len);
        n += len;
    }
    out[0] = (uint8_t)n;
    k_memcpy(out + 1, buf, n);
}

/* The host path of an item; 0, or -1 (unknown ID, path too long). */
static int path_of(mac_fs_paths_t *p, uint32_t id, char *buf, uint32_t size)
{
    uint32_t chain[64];
    uint32_t n = 0;

    while (id != MAC_FS_ROOT_ID)
    {
        uint32_t i = id - MAC_FS_FIRST_ID;

        if (id < MAC_FS_FIRST_ID || i >= p->count || n == 64)
            return -1;
        chain[n++] = i;
        id = p->item[i].parent;
    }

    uint32_t len = 0;

    while (n > 0)
    {
        const char *name = p->item[chain[--n]].name;
        uint32_t l = (uint32_t)k_strlen(name);

        if (len + l + 2 > size)
            return -1;
        if (len > 0)
            buf[len++] = '/';
        k_memcpy(buf + len, name, l);
        len += l;
    }
    buf[len] = '\0';
    return 0;
}

static void join(char *out, const char *dir, const char *name)
{
    uint32_t len = (uint32_t)k_strlen(dir);

    k_memcpy(out, dir, len);
    if (len > 0)
        out[len++] = '/';
    k_memcpy(out + len, name, k_strlen(name) + 1);
}

/* The ID of name in folder parent: the one it had, or a new one (0 if the table is full). */
static uint32_t id_of(mac_fs_paths_t *p, uint32_t parent, const char *name)
{
    for (uint32_t i = 0; i < p->count; i++)
    {
        if (p->item[i].parent == parent && k_strcmp(p->item[i].name, name) == 0)
            return MAC_FS_FIRST_ID + i;
    }
    if (p->count == MAC_FS_PATH_ITEMS)
        return 0;
    p->item[p->count].parent = parent;
    k_memcpy(p->item[p->count].name, name, k_strlen(name) + 1);
    p->count++;
    return MAC_FS_FIRST_ID + p->count - 1;
}

/* Visible entries of folder path, counted. */
static uint32_t valence(mac_fs_paths_t *p, const char *path)
{
    char name[MAC_FS_HOST_NAME + 1];
    uint32_t n = 0;

    for (uint32_t i = 0; p->host->entry(p->host->ctx, path, i, name, sizeof name) == 0; i++)
    {
        if (!hidden(name))
            n++;
    }
    return n;
}

static int fill(mac_fs_paths_t *p, uint32_t id, uint32_t parent, const char *name, const char *path,
                mac_fs_item_t *it)
{
    bool is_dir;
    uint32_t size, mtime;

    if (p->host->stat(p->host->ctx, path, &is_dir, &size, &mtime) != 0)
        return -1;
    k_memset(it, 0, sizeof *it);
    it->id = id;
    it->parent = parent;
    it->is_dir = is_dir;
    if (id == MAC_FS_ROOT_ID)
    {
        uint32_t n = (uint32_t)k_strlen(p->ops.volume_name);

        it->name[0] = (uint8_t)(n > 27 ? 27 : n);
        k_memcpy(it->name + 1, p->ops.volume_name, it->name[0]);
    }
    else
    {
        mac_fs_paths_mac_name(name, id, it->name);
    }
    it->created = it->modified = mtime > 0xFFFFFFFFu - UNIX_TO_MAC ? 0xFFFFFFFFu : mtime + UNIX_TO_MAC;
    if (is_dir)
    {
        it->valence = valence(p, path);
    }
    else
    {
        it->data_length = size;
        k_memcpy(it->finfo, is_text(name) ? "TEXTttxt" : "????????", 8);
    }
    return 0;
}

static int op_get(void *ctx, uint32_t id, mac_fs_item_t *item)
{
    mac_fs_paths_t *p = ctx;
    char path[MAC_FS_PATH_MAX];

    if (id == MAC_FS_ROOT_ID)
        return fill(p, id, MAC_FS_ROOT_PARENT, "", "", item);
    if (path_of(p, id, path, sizeof path) != 0)
        return -1;

    uint32_t i = id - MAC_FS_FIRST_ID;

    return fill(p, id, p->item[i].parent, p->item[i].name, path, item);
}

/* Item number index (from 1) of folder parent, skipping hidden names. */
static int op_child(void *ctx, uint32_t parent, uint32_t index, mac_fs_item_t *item)
{
    mac_fs_paths_t *p = ctx;
    char dir[MAC_FS_PATH_MAX], path[MAC_FS_PATH_MAX + MAC_FS_HOST_NAME + 1], name[MAC_FS_HOST_NAME + 1];
    uint32_t seen = 0;

    if (index == 0 || path_of(p, parent, dir, sizeof dir) != 0)
        return -1;
    for (uint32_t i = 0; p->host->entry(p->host->ctx, dir, i, name, sizeof name) == 0; i++)
    {
        if (hidden(name) || ++seen < index)
            continue;

        uint32_t id = id_of(p, parent, name);

        if (id == 0)
            return -1;
        join(path, dir, name);
        return fill(p, id, parent, name, path, item);
    }
    return -1;
}

static int op_lookup(void *ctx, uint32_t parent, const uint8_t *name, uint32_t length, mac_fs_item_t *item)
{
    mac_fs_paths_t *p = ctx;
    char dir[MAC_FS_PATH_MAX], path[MAC_FS_PATH_MAX + MAC_FS_HOST_NAME + 1], host[MAC_FS_HOST_NAME + 1];

    if (path_of(p, parent, dir, sizeof dir) != 0)
        return -1;
    for (uint32_t i = 0; p->host->entry(p->host->ctx, dir, i, host, sizeof host) == 0; i++)
    {
        if (hidden(host))
            continue;

        uint32_t id = id_of(p, parent, host);
        uint8_t mac[MAC_FS_NAME_MAX + 1];
        bool same;

        if (id == 0)
            return -1;
        mac_fs_paths_mac_name(host, id, mac);
        same = mac[0] == length;
        for (uint32_t k = 0; same && k < length; k++)
            same = lower((char)mac[1 + k]) == lower((char)name[k]);
        if (same)
        {
            join(path, dir, host);
            return fill(p, id, parent, host, path, item);
        }
    }
    return -1;
}

static int32_t op_read(void *ctx, uint32_t id, bool resource_fork, uint32_t offset, void *buffer, uint32_t length)
{
    mac_fs_paths_t *p = ctx;
    char path[MAC_FS_PATH_MAX];

    if (resource_fork)
        return 0;               /* host files have no resource fork */
    if (path_of(p, id, path, sizeof path) != 0)
        return -1;
    return p->host->read(p->host->ctx, path, offset, buffer, length);
}

void mac_fs_paths_init(mac_fs_paths_t *paths, const mac_fs_host_t *host, const char *volume_name)
{
    k_memset(paths, 0, sizeof *paths);
    paths->host = host;
    paths->ops.ctx = paths;
    paths->ops.volume_name = volume_name;
    paths->ops.get = op_get;
    paths->ops.lookup = op_lookup;
    paths->ops.child = op_child;
    paths->ops.read = op_read;
}
