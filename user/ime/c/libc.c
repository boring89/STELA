/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

/*
 * The C library libchewing needs, for the IME (docs/ime.md). Every name
 * has an ime_ prefix (the headers in include/ rename the standard ones),
 * so the same code links into the STELA program and into the host tests
 * without clashing with the host's C library.
 *
 * Files live in memory: the Rust side registers the dictionary files it
 * read from Initrd:/ime/chewing/ (ime_vfs_add, read-only, not copied)
 * and the user-phrase file; whatever libchewing writes stays in the table
 * and the Rust side picks it up with ime_vfs_get to save it on Data.
 * Memory comes from the Rust allocator (ime_rust_alloc/ime_rust_free).
 */

#include <ctype.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>

/* Provided by user/ime/src/ffi.rs. */
void *ime_rust_alloc(size_t size);
void ime_rust_free(void *p, size_t size);
void ime_rust_abort(const char *msg) __attribute__((noreturn));

int ime_errno;

/* ---- memory ---- */

/* A 16-byte header in front of each block keeps its size. */
#define HDR 16

void *ime_malloc(size_t size)
{
    unsigned char *p;

    if (size > ((size_t) 1 << 40))
        return NULL;
    p = ime_rust_alloc(size + HDR);
    if (!p)
        return NULL;
    *(size_t *) p = size;
    return p + HDR;
}

void ime_free(void *ptr)
{
    unsigned char *p = ptr;

    if (!p)
        return;
    p -= HDR;
    ime_rust_free(p, *(size_t *) p + HDR);
}

void *ime_calloc(size_t n, size_t size)
{
    void *p;

    if (size && n > ((size_t) -1) / size)
        return NULL;
    p = ime_malloc(n * size);
    if (p)
        memset(p, 0, n * size);
    return p;
}

void *ime_realloc(void *ptr, size_t size)
{
    void *n;
    size_t old;

    if (!ptr)
        return ime_malloc(size);
    old = *(size_t *) ((unsigned char *) ptr - HDR);
    n = ime_malloc(size);
    if (!n)
        return NULL;
    memcpy(n, ptr, old < size ? old : size);
    ime_free(ptr);
    return n;
}

void ime_exit(int status)
{
    (void) status;
    ime_rust_abort("libchewing called exit()");
}

void ime_assert_fail(const char *expr, const char *file, int line)
{
    (void) file;
    (void) line;
    ime_rust_abort(expr);
}

char *ime_getenv(const char *name)
{
    (void) name;
    return NULL;
}

int ime_abs(int x)
{
    return x < 0 ? -x : x;
}

int ime_atoi(const char *s)
{
    int neg = 0, v = 0;

    while (ime_isspace((unsigned char) *s))
        s++;
    if (*s == '-' || *s == '+')
        neg = *s++ == '-';
    while (ime_isdigit((unsigned char) *s))
        v = v * 10 + (*s++ - '0');
    return neg ? -v : v;
}

void *ime_bsearch(const void *key, const void *base, size_t n, size_t size,
                  int (*cmp)(const void *, const void *))
{
    const unsigned char *a = base;
    size_t lo = 0, hi = n;

    while (lo < hi) {
        size_t mid = lo + (hi - lo) / 2;
        int c = cmp(key, a + mid * size);

        if (c == 0)
            return (void *) (a + mid * size);
        if (c < 0)
            hi = mid;
        else
            lo = mid + 1;
    }
    return NULL;
}

/* Insertion sort over a gap sequence (Shell sort): small arrays only. */
void ime_qsort(void *base, size_t n, size_t size, int (*cmp)(const void *, const void *))
{
    unsigned char *a = base, tmp[256], *t = tmp;
    size_t gap, i, j;

    if (size > sizeof(tmp)) {
        t = ime_malloc(size);
        if (!t)
            ime_rust_abort("qsort: no memory");
    }
    for (gap = n / 2; gap > 0; gap /= 2) {
        for (i = gap; i < n; i++) {
            memcpy(t, a + i * size, size);
            for (j = i; j >= gap && cmp(a + (j - gap) * size, t) > 0; j -= gap)
                memcpy(a + j * size, a + (j - gap) * size, size);
            memcpy(a + j * size, t, size);
        }
    }
    if (t != tmp)
        ime_free(t);
}

/* ---- ctype (ASCII) ---- */

int ime_isdigit(int c) { return c >= '0' && c <= '9'; }
int ime_isupper(int c) { return c >= 'A' && c <= 'Z'; }
int ime_islower(int c) { return c >= 'a' && c <= 'z'; }
int ime_isalpha(int c) { return ime_isupper(c) || ime_islower(c); }
int ime_isprint(int c) { return c >= 0x20 && c < 0x7f; }
int ime_isspace(int c) { return c == ' ' || (c >= '\t' && c <= '\r'); }
int ime_tolower(int c) { return ime_isupper(c) ? c + 32 : c; }
int ime_toupper(int c) { return ime_islower(c) ? c - 32 : c; }

/* ---- strings ---- */

void *ime_memchr(const void *s, int c, size_t n)
{
    const unsigned char *p = s;

    for (; n; n--, p++)
        if (*p == (unsigned char) c)
            return (void *) p;
    return NULL;
}

size_t ime_strlen(const char *s)
{
    size_t n = 0;

    while (s[n])
        n++;
    return n;
}

int ime_strcmp(const char *a, const char *b)
{
    while (*a && *a == *b)
        a++, b++;
    return (unsigned char) *a - (unsigned char) *b;
}

int ime_strncmp(const char *a, const char *b, size_t n)
{
    for (; n; n--, a++, b++) {
        if (*a != *b)
            return (unsigned char) *a - (unsigned char) *b;
        if (!*a)
            return 0;
    }
    return 0;
}

char *ime_strcpy(char *d, const char *s)
{
    char *r = d;

    while ((*d++ = *s++))
        ;
    return r;
}

char *ime_strncpy(char *d, const char *s, size_t n)
{
    size_t i = 0;

    for (; i < n && s[i]; i++)
        d[i] = s[i];
    for (; i < n; i++)
        d[i] = 0;
    return d;
}

char *ime_strcat(char *d, const char *s)
{
    ime_strcpy(d + ime_strlen(d), s);
    return d;
}

char *ime_strchr(const char *s, int c)
{
    for (;; s++) {
        if (*s == (char) c)
            return (char *) s;
        if (!*s)
            return NULL;
    }
}

char *ime_strrchr(const char *s, int c)
{
    const char *r = NULL;

    for (;; s++) {
        if (*s == (char) c)
            r = s;
        if (!*s)
            return (char *) r;
    }
}

char *ime_strstr(const char *h, const char *n)
{
    size_t len = ime_strlen(n);

    for (; *h; h++)
        if (!ime_strncmp(h, n, len))
            return (char *) h;
    return len ? NULL : (char *) h;
}

size_t ime_strspn(const char *s, const char *accept)
{
    size_t n = 0;

    while (s[n] && ime_strchr(accept, s[n]))
        n++;
    return n;
}

size_t ime_strcspn(const char *s, const char *reject)
{
    size_t n = 0;

    while (s[n] && !ime_strchr(reject, s[n]))
        n++;
    return n;
}

char *ime_strpbrk(const char *s, const char *accept)
{
    s += ime_strcspn(s, accept);
    return *s ? (char *) s : NULL;
}

char *ime_strdup(const char *s)
{
    size_t n = ime_strlen(s) + 1;
    char *d = ime_malloc(n);

    if (d)
        memcpy(d, s, n);
    return d;
}

char *ime_strtok_r(char *s, const char *delim, char **save)
{
    char *end;

    if (!s)
        s = *save;
    s += ime_strspn(s, delim);
    if (!*s) {
        *save = s;
        return NULL;
    }
    end = s + ime_strcspn(s, delim);
    if (*end)
        *end++ = 0;
    *save = end;
    return s;
}

char *ime_strtok(char *s, const char *delim)
{
    static char *save;

    return ime_strtok_r(s, delim, &save);
}

/* ---- formatted output ---- */

struct out {
    char *buf;
    size_t cap;   /* bytes the buffer holds, the terminating 0 included */
    size_t len;   /* bytes produced (may exceed cap) */
};

static void put(struct out *o, char c)
{
    if (o->len + 1 < o->cap)
        o->buf[o->len] = c;
    o->len++;
}

int ime_vsnprintf(char *s, size_t n, const char *fmt, va_list ap)
{
    struct out o = { s, n, 0 };

    for (; *fmt; fmt++) {
        char digits[24], *str, pad = ' ';
        int left = 0, alt = 0, plus = 0, space = 0, width = 0, prec = -1, lng = 0;
        int i, dlen, neg = 0, base = 10, upper = 0;
        unsigned long long v;
        const char *prefix = "";

        if (*fmt != '%') {
            put(&o, *fmt);
            continue;
        }
        for (fmt++;; fmt++) {
            if (*fmt == '-')
                left = 1;
            else if (*fmt == '0')
                pad = '0';
            else if (*fmt == '#')
                alt = 1;
            else if (*fmt == '+')
                plus = 1;
            else if (*fmt == ' ')
                space = 1;
            else
                break;
        }
        if (*fmt == '*') {
            width = va_arg(ap, int);
            fmt++;
        }
        while (ime_isdigit((unsigned char) *fmt))
            width = width * 10 + (*fmt++ - '0');
        if (*fmt == '.') {
            prec = 0;
            fmt++;
            if (*fmt == '*') {
                prec = va_arg(ap, int);
                fmt++;
            }
            while (ime_isdigit((unsigned char) *fmt))
                prec = prec * 10 + (*fmt++ - '0');
        }
        for (;; fmt++) {
            if (*fmt == 'l' || *fmt == 'z' || *fmt == 'j' || *fmt == 't')
                lng++;
            else if (*fmt != 'h')
                break;
        }
        switch (*fmt) {
        case 's':
            str = va_arg(ap, char *);
            if (!str)
                str = "(null)";
            dlen = 0;
            while (str[dlen] && (prec < 0 || dlen < prec))
                dlen++;
            if (!left)
                for (i = dlen; i < width; i++)
                    put(&o, ' ');
            for (i = 0; i < dlen; i++)
                put(&o, str[i]);
            if (left)
                for (i = dlen; i < width; i++)
                    put(&o, ' ');
            continue;
        case 'c':
            put(&o, (char) va_arg(ap, int));
            continue;
        case '%':
            put(&o, '%');
            continue;
        case 'd':
        case 'i':
            {
                long long x = lng ? va_arg(ap, long long) : va_arg(ap, int);

                neg = x < 0;
                v = neg ? -(unsigned long long) x : (unsigned long long) x;
                prefix = neg ? "-" : plus ? "+" : space ? " " : "";
            }
            break;
        case 'p':
            v = (uintptr_t) va_arg(ap, void *);
            base = 16;
            prefix = "0x";
            break;
        case 'X':
            upper = 1;
            /* fall through */
        case 'x':
        case 'o':
        case 'u':
            v = lng ? va_arg(ap, unsigned long long) : va_arg(ap, unsigned int);
            base = *fmt == 'u' ? 10 : *fmt == 'o' ? 8 : 16;
            if (alt && v && base == 16)
                prefix = upper ? "0X" : "0x";
            break;
        default:
            /* Unknown conversion: print it as it is. */
            put(&o, '%');
            if (*fmt)
                put(&o, *fmt);
            else
                fmt--;
            continue;
        }
        dlen = 0;
        do {
            int d = (int) (v % base);

            digits[dlen++] = (char) (d < 10 ? '0' + d : (upper ? 'A' : 'a') + d - 10);
            v /= base;
        } while (v);
        while (dlen < prec)
            digits[dlen++] = '0';
        width -= dlen + (int) ime_strlen(prefix);
        if (!left && pad == ' ')
            for (; width > 0; width--)
                put(&o, ' ');
        for (str = (char *) prefix; *str; str++)
            put(&o, *str);
        if (!left)
            for (; width > 0; width--)
                put(&o, '0');
        while (dlen)
            put(&o, digits[--dlen]);
        for (; width > 0; width--)
            put(&o, ' ');
    }
    if (o.cap)
        o.buf[o.len < o.cap ? o.len : o.cap - 1] = 0;
    return o.len > INT_MAX ? -1 : (int) o.len;
}

int ime_snprintf(char *s, size_t n, const char *fmt, ...)
{
    va_list ap;
    int r;

    va_start(ap, fmt);
    r = ime_vsnprintf(s, n, fmt, ap);
    va_end(ap);
    return r;
}

int ime_sprintf(char *s, const char *fmt, ...)
{
    va_list ap;
    int r;

    va_start(ap, fmt);
    r = ime_vsnprintf(s, (size_t) INT_MAX, fmt, ap);
    va_end(ap);
    return r;
}

int ime_asprintf(char **out, const char *fmt, ...)
{
    va_list ap;
    int n;

    va_start(ap, fmt);
    n = ime_vsnprintf(NULL, 0, fmt, ap);
    va_end(ap);
    if (n < 0)
        return -1;
    *out = ime_malloc((size_t) n + 1);
    if (!*out)
        return -1;
    va_start(ap, fmt);
    ime_vsnprintf(*out, (size_t) n + 1, fmt, ap);
    va_end(ap);
    return n;
}

/* No console: libchewing only prints in its debug builds. */
int ime_vfprintf(FILE *f, const char *fmt, va_list ap)
{
    (void) f;
    return ime_vsnprintf(NULL, 0, fmt, ap);
}

int ime_fprintf(FILE *f, const char *fmt, ...)
{
    va_list ap;
    int r;

    va_start(ap, fmt);
    r = ime_vfprintf(f, fmt, ap);
    va_end(ap);
    return r;
}

int ime_printf(const char *fmt, ...)
{
    va_list ap;
    int r;

    va_start(ap, fmt);
    r = ime_vfprintf(NULL, fmt, ap);
    va_end(ap);
    return r;
}

/* ---- the files in memory ---- */

#define MAX_FILES 16
#define MAX_PATH 160

struct vfile {
    char path[MAX_PATH];  /* empty: a free slot */
    unsigned char *data;
    size_t len;
    size_t cap;           /* 0: data belongs to the Rust side (read-only) */
    int dirty;
};

static struct vfile files[MAX_FILES];

struct ime_file {
    struct vfile *v;
    size_t pos;
    int eof;
    int used;
};

static struct ime_file streams[MAX_FILES];

static struct vfile *lookup(const char *path)
{
    int i;

    for (i = 0; i < MAX_FILES; i++)
        if (files[i].path[0] && !ime_strcmp(files[i].path, path))
            return &files[i];
    return NULL;
}

static struct vfile *create(const char *path)
{
    int i;

    if (ime_strlen(path) >= MAX_PATH)
        return NULL;
    for (i = 0; i < MAX_FILES; i++) {
        if (!files[i].path[0]) {
            ime_strcpy(files[i].path, path);
            files[i].data = NULL;
            files[i].len = files[i].cap = 0;
            files[i].dirty = 0;
            return &files[i];
        }
    }
    return NULL;
}

static void drop_data(struct vfile *v)
{
    if (v->cap)
        ime_free(v->data);
    v->data = NULL;
    v->len = v->cap = 0;
}

/* Called by Rust: a read-only file whose bytes stay owned by Rust. */
int ime_vfs_add(const char *path, const void *data, size_t len)
{
    struct vfile *v = lookup(path);

    if (!v)
        v = create(path);
    if (!v)
        return -1;
    drop_data(v);
    v->data = (unsigned char *) data;
    v->len = len;
    v->dirty = 0;
    return 0;
}

/* Called by Rust: forgets every file (before a new context). */
void ime_vfs_reset(void)
{
    int i;

    for (i = 0; i < MAX_FILES; i++) {
        drop_data(&files[i]);
        files[i].path[0] = 0;
        streams[i].used = 0;
    }
}

/*
 * Called by Rust: the bytes of a file (NULL if there is none) and whether
 * libchewing changed it since the last call (the flag is cleared).
 */
const void *ime_vfs_get(const char *path, size_t *len, int *dirty)
{
    struct vfile *v = lookup(path);

    if (!v)
        return NULL;
    *len = v->len;
    *dirty = v->dirty;
    v->dirty = 0;
    return v->data ? v->data : (const void *) "";
}

/* Make the file's bytes our own (copy) before the first write. */
static int own(struct vfile *v, size_t need)
{
    unsigned char *n;
    size_t cap;

    if (need < v->len)
        need = v->len;
    if (v->cap >= need && v->cap)
        return 0;
    cap = v->cap ? v->cap : 64;
    while (cap < need)
        cap *= 2;
    n = ime_malloc(cap);
    if (!n)
        return -1;
    if (v->len)
        memcpy(n, v->data, v->len);
    if (v->cap)
        ime_free(v->data);
    v->data = n;
    v->cap = cap;
    return 0;
}

FILE *ime_fopen(const char *path, const char *mode)
{
    struct vfile *v = lookup(path);
    int i;

    if (mode[0] == 'w') {
        if (!v)
            v = create(path);
        if (!v)
            return NULL;
        drop_data(v);
        v->dirty = 1;
    } else if (mode[0] != 'r' || !v) {
        ime_errno = ENOENT;
        return NULL;
    }
    for (i = 0; i < MAX_FILES; i++) {
        if (!streams[i].used) {
            streams[i].used = 1;
            streams[i].v = v;
            streams[i].pos = 0;
            streams[i].eof = 0;
            return &streams[i];
        }
    }
    return NULL;
}

int ime_fclose(FILE *f)
{
    if (f)
        f->used = 0;
    return 0;
}

int ime_fflush(FILE *f)
{
    (void) f;
    return 0;
}

size_t ime_fread(void *buf, size_t size, size_t n, FILE *f)
{
    size_t want, have;

    if (!f || !size)
        return 0;
    want = size * n;
    have = f->pos < f->v->len ? f->v->len - f->pos : 0;
    if (want > have) {
        want = have - have % size;
        f->eof = 1;
    }
    memcpy(buf, f->v->data + f->pos, want);
    f->pos += want;
    return want / size;
}

size_t ime_fwrite(const void *buf, size_t size, size_t n, FILE *f)
{
    size_t len = size * n;

    if (!f || !len)
        return 0;
    if (own(f->v, f->pos + len))
        return 0;
    if (f->pos > f->v->len)
        memset(f->v->data + f->v->len, 0, f->pos - f->v->len);
    memcpy(f->v->data + f->pos, buf, len);
    f->pos += len;
    if (f->pos > f->v->len)
        f->v->len = f->pos;
    f->v->dirty = 1;
    return n;
}

int ime_fseek(FILE *f, long off, int whence)
{
    long base = whence == SEEK_SET ? 0 : whence == SEEK_CUR ? (long) f->pos : (long) f->v->len;

    if (base + off < 0)
        return -1;
    f->pos = (size_t) (base + off);
    f->eof = 0;
    return 0;
}

long ime_ftell(FILE *f)
{
    return (long) f->pos;
}

int ime_fgetc(FILE *f)
{
    if (f->pos >= f->v->len) {
        f->eof = 1;
        return EOF;
    }
    return f->v->data[f->pos++];
}

int ime_feof(FILE *f)
{
    return f->eof;
}

char *ime_fgets(char *s, int n, FILE *f)
{
    int i = 0, c;

    if (n <= 0)
        return NULL;
    while (i < n - 1 && (c = ime_fgetc(f)) != EOF) {
        s[i++] = (char) c;
        if (c == '\n')
            break;
    }
    if (!i)
        return NULL;
    s[i] = 0;
    return s;
}

/* fscanf with the conversions libchewing uses: %d, %hu, %s. */
int ime_fscanf(FILE *f, const char *fmt, ...)
{
    va_list ap;
    int count = 0, c;

    va_start(ap, fmt);
    for (; *fmt; fmt++) {
        int half = 0;

        if (ime_isspace((unsigned char) *fmt))
            continue;
        if (*fmt != '%')
            break;
        fmt++;
        if (*fmt == 'h') {
            half = 1;
            fmt++;
        }
        do
            c = ime_fgetc(f);
        while (c != EOF && ime_isspace(c));
        if (c == EOF) {
            va_end(ap);
            return count ? count : EOF;
        }
        if (*fmt == 's') {
            char *s = va_arg(ap, char *);

            while (c != EOF && !ime_isspace(c)) {
                *s++ = (char) c;
                c = ime_fgetc(f);
            }
            *s = 0;
        } else if (*fmt == 'd' || *fmt == 'u') {
            long v = 0;
            int neg = 0, any = 0;

            if (c == '-' || c == '+') {
                neg = c == '-';
                c = ime_fgetc(f);
            }
            while (c != EOF && ime_isdigit(c)) {
                v = v * 10 + (c - '0');
                any = 1;
                c = ime_fgetc(f);
            }
            if (!any)
                break;
            if (neg)
                v = -v;
            if (half)
                *va_arg(ap, unsigned short *) = (unsigned short) v;
            else
                *va_arg(ap, int *) = (int) v;
        } else {
            break;
        }
        if (c != EOF)
            f->pos--;
        count++;
    }
    va_end(ap);
    return count;
}

int ime_rename(const char *from, const char *to)
{
    struct vfile *v = lookup(from), *old = lookup(to);

    if (!v || ime_strlen(to) >= MAX_PATH)
        return -1;
    if (old && old != v) {
        drop_data(old);
        old->path[0] = 0;
    }
    ime_strcpy(v->path, to);
    v->dirty = 1;
    return 0;
}

int ime_unlink(const char *path)
{
    struct vfile *v = lookup(path);

    if (!v)
        return -1;
    drop_data(v);
    v->path[0] = 0;
    return 0;
}

int ime_access(const char *path, int mode)
{
    (void) mode;
    return lookup(path) ? 0 : -1;
}

int ime_mkdir(const char *path, mode_t mode)
{
    (void) path;
    (void) mode;
    return 0;
}

/* Descriptors (for plat_mmap): fd = 3 + index into files[]. */
int ime_open(const char *path, int flags, ...)
{
    struct vfile *v = lookup(path);

    (void) flags;
    if (!v)
        return -1;
    return 3 + (int) (v - files);
}

static struct vfile *by_fd(int fd)
{
    if (fd < 3 || fd >= 3 + MAX_FILES || !files[fd - 3].path[0])
        return NULL;
    return &files[fd - 3];
}

int ime_close(int fd)
{
    return by_fd(fd) ? 0 : -1;
}

off_t ime_lseek(int fd, off_t off, int whence)
{
    struct vfile *v = by_fd(fd);

    if (!v)
        return -1;
    /* Only the size is ever asked for (SEEK_END, 0). */
    return whence == SEEK_END ? (off_t) v->len + off : off;
}

/* Read-only "mapping": the bytes themselves. */
void *ime_mmap(void *addr, size_t len, int prot, int flags, int fd, off_t off)
{
    struct vfile *v = by_fd(fd);

    (void) addr;
    (void) prot;
    (void) flags;
    if (!v || off < 0 || (size_t) off + len > v->len)
        return MAP_FAILED;
    return v->data + off;
}

int ime_munmap(void *addr, size_t len)
{
    (void) addr;
    (void) len;
    return 0;
}

int ime_getpagesize(void)
{
    return 4096;
}
