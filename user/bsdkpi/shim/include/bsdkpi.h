/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: MIT
 *
 * bsdkpi (P14 K13, docs/bsdkpi.md): the part of FreeBSD's kernel
 * programming interface that if_em (sys/dev/e1000 of FreeBSD 14.3) uses,
 * for a STELA user program. Every FreeBSD header the driver includes is
 * this one. Written for STELA (not copied from FreeBSD), except the
 * FreeBSD headers included at the end (net/iflib.h, net/if_media.h,
 * dev/pci/pcireg.h, sys/queue.h: BSD licensed, in ../../freebsd/).
 *
 * The model: one thread runs the driver (bsdkpi_* entry points called
 * from Rust, src/ffi.rs); locks are no-ops, callouts and tasks run from
 * that thread's loop, DMA memory and the registers come from STELA's
 * Device interface.
 */
#ifndef BSDKPI_H
#define BSDKPI_H

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>
#include <stdarg.h>
#include <limits.h>

#define _KERNEL 1
#define __FreeBSD__ 14
#define __FreeBSD_version 1403000

/* ---- basic types (sys/types.h) ---- */
typedef uint8_t u_int8_t;
typedef uint16_t u_int16_t;
typedef uint32_t u_int32_t;
typedef uint64_t u_int64_t;
typedef unsigned char u_char;
typedef unsigned short u_short;
typedef unsigned int u_int;
typedef unsigned long u_long;
typedef long ssize_t;
typedef char *caddr_t;
typedef const char *c_caddr_t;
typedef uint64_t bus_addr_t;
typedef uint64_t bus_size_t;
typedef uint64_t vm_paddr_t;
typedef uint64_t vm_offset_t;
typedef int64_t off_t;
typedef int64_t sbintime_t;
typedef int64_t time_t;
typedef uint32_t in_addr_t;
typedef uint16_t in_port_t;
typedef int boolean_t;
typedef uint32_t uint;
typedef uint64_t rman_res_t;
typedef int64_t quad_t;
typedef uint64_t u_quad_t;
typedef uint32_t pid_t;

#define __unused __attribute__((unused))
#define __packed __attribute__((packed))
#define __aligned(x) __attribute__((aligned(x)))
#define __predict_false(x) __builtin_expect(!!(x), 0)
#define __predict_true(x) __builtin_expect(!!(x), 1)
#define __inline inline
#define __DECONST(type, var) ((type)(uintptr_t)(const void *)(var))
#define __containerof(x, s, m) ((s *)((char *)(x) - offsetof(s, m)))
#define __printflike(a, b) __attribute__((format(printf, a, b)))
#define __FBSDID(s)
#define __BEGIN_DECLS
#define __END_DECLS
#define CACHE_LINE_SIZE 64
#define __offsetof(t, m) offsetof(t, m)
#define __bitcount32(x) __builtin_popcount(x)
#define __restrict restrict
#define __used __attribute__((used))
#define __diagused __unused
#define __result_use_check
#define __nonnull(x)

#ifndef NULL
#define NULL ((void *)0)
#endif
#define TRUE 1
#define FALSE 0

/* ---- sys/param.h, sys/systm.h ---- */
#define hz 1000
extern volatile uint64_t ticks;
#define NBBY 8
#define MAXPATHLEN 1024
#define PAGE_SIZE 4096
#define PAGE_MASK (PAGE_SIZE - 1)
#define MCLBYTES 2048
#define MJUMPAGESIZE PAGE_SIZE
#define MJUM9BYTES (9 * 1024)
#define MJUM16BYTES (16 * 1024)
#define MSIZE 256
#define MHLEN 160
#define MLEN 200
#define nitems(x) (sizeof(x) / sizeof((x)[0]))
#define howmany(x, y) (((x) + ((y) - 1)) / (y))
#define roundup2(x, y) (((x) + ((y) - 1)) & (~((y) - 1)))
#define rounddown2(x, y) ((x) & ~((y) - 1))
#define powerof2(x) ((((x) - 1) & (x)) == 0)
#define min(a, b) ((a) < (b) ? (a) : (b))
#define max(a, b) ((a) > (b) ? (a) : (b))
#define imin(a, b) min(a, b)
#define imax(a, b) max(a, b)
#define MIN(a, b) min(a, b)
#define MAX(a, b) max(a, b)
#define bitcount32(x) __builtin_popcount(x)
#define ffs(x) __builtin_ffs(x)
#define fls(x) ((x) ? 32 - __builtin_clz(x) : 0)
#define KASSERT(e, m) do { (void)0; } while (0)
#define MPASS(e) do { (void)0; } while (0)
#define CTASSERT(e) _Static_assert(e, #e)
#define __assert_unreachable() __builtin_unreachable()
#define INVARIANTS_unused
#define bootverbose 0
#define mp_ncpus 1
#define curcpu 0
#define CPU_FIRST() 0
#define CPU_NEXT(i) 0

#define EINVAL 22
#define ENOMEM 12
#define ENXIO 6
#define EIO 5
#define EBUSY 16
#define EFBIG 27
#define ENOBUFS 55
#define EOPNOTSUPP 45
#define ETIMEDOUT 60
#define EAGAIN 35
#define ENOTSUP EOPNOTSUPP
#define ENOENT 2
#define ENODEV 19
#define EPERM 1
#define E2BIG 7
#define EEXIST 17
#define EBADMSG 89

int printf(const char *fmt, ...) __printflike(1, 2);
int snprintf(char *buf, size_t n, const char *fmt, ...) __printflike(3, 4);
int vsnprintf(char *buf, size_t n, const char *fmt, va_list ap);
void panic(const char *fmt, ...) __printflike(1, 2) __attribute__((noreturn));
void *memcpy(void *d, const void *s, size_t n);
void *memmove(void *d, const void *s, size_t n);
void *memset(void *d, int c, size_t n);
int memcmp(const void *a, const void *b, size_t n);
size_t strlen(const char *s);
int strcmp(const char *a, const char *b);
int strncmp(const char *a, const char *b, size_t n);
char *strncpy(char *d, const char *s, size_t n);
size_t strlcpy(char *d, const char *s, size_t n);
#define bcopy(s, d, n) memmove((d), (s), (n))
#define bzero(d, n) memset((d), 0, (n))
#define bcmp(a, b, n) memcmp((a), (b), (n))
void DELAY(int usec);
int pause(const char *wmesg, int timo);
uint32_t arc4random(void);
void arc4rand(void *p, unsigned n, int reseed);
#define log(level, ...) printf(__VA_ARGS__)
#define LOG_WARNING 4
#define LOG_ERR 3
#define LOG_INFO 6
#define LOG_NOTICE 5

/* ---- sys/endian.h ---- */
#define htole16(x) ((uint16_t)(x))
#define htole32(x) ((uint32_t)(x))
#define htole64(x) ((uint64_t)(x))
#define le16toh(x) ((uint16_t)(x))
#define le32toh(x) ((uint32_t)(x))
#define le64toh(x) ((uint64_t)(x))
#define htobe16(x) __builtin_bswap16(x)
#define htobe32(x) __builtin_bswap32(x)
#define be16toh(x) __builtin_bswap16(x)
#define be32toh(x) __builtin_bswap32(x)
#define htons(x) __builtin_bswap16(x)
#define ntohs(x) __builtin_bswap16(x)
#define htonl(x) __builtin_bswap32(x)
#define ntohl(x) __builtin_bswap32(x)
#define bswap16(x) __builtin_bswap16(x)
#define bswap32(x) __builtin_bswap32(x)

/* ---- atomics (machine/atomic.h), single thread ---- */
#define atomic_add_int(p, v) (*(p) += (v))
#define atomic_subtract_int(p, v) (*(p) -= (v))
#define atomic_set_int(p, v) (*(p) |= (v))
#define atomic_clear_int(p, v) (*(p) &= ~(v))
#define atomic_load_acq_int(p) (*(p))
#define atomic_store_rel_int(p, v) (*(p) = (v))
#define atomic_add_64(p, v) (*(p) += (v))
#define wmb() __asm__ __volatile__("" ::: "memory")
#define rmb() __asm__ __volatile__("" ::: "memory")
#define mb() __asm__ __volatile__("" ::: "memory")

/* ---- malloc(9) ---- */
struct malloc_type { const char *name; };
#define MALLOC_DEFINE(t, s, l) struct malloc_type t[1] = { { s } }
#define MALLOC_DECLARE(t) extern struct malloc_type t[1]
extern struct malloc_type M_DEVBUF[1];
#define M_NOWAIT 0x0001
#define M_WAITOK 0x0002
#define M_ZERO 0x0100
void *malloc(size_t size, struct malloc_type *type, int flags);
void free(void *p, struct malloc_type *type);
void *mallocarray(size_t n, size_t size, struct malloc_type *type, int flags);

/* ---- locks: one thread, so nothing to do ---- */
struct mtx { const char *name; };
struct sx { const char *name; };
struct lock_object { const char *name; };
#define MTX_DEF 0
#define MTX_SPIN 1
#define mtx_init(m, n, t, o) ((m)->name = (n))
#define mtx_destroy(m) ((void)(m))
#define mtx_lock(m) ((void)(m))
#define mtx_unlock(m) ((void)(m))
#define mtx_assert(m, w) ((void)(m))
#define mtx_initialized(m) 1
#define sx_init(s, n) ((s)->name = (n))
#define sx_xlock(s) ((void)(s))
#define sx_xunlock(s) ((void)(s))
#define sx_assert(s, w) ((void)(s))
#define SX_XLOCKED 1
#define MA_OWNED 1

/* ---- kobj, devices (sys/bus.h, sys/kobj.h) ---- */
typedef void (*kobjop_t)(void);
typedef struct kobj_method { const char *name; kobjop_t func; } kobj_method_t;
typedef kobj_method_t device_method_t;
#define DEVMETHOD(name, func) { #name, (kobjop_t)(func) }
#define DEVMETHOD_END { NULL, NULL }
typedef struct kobj_class {
	const char *name;
	const kobj_method_t *methods;
	size_t size;
} driver_t;
typedef driver_t *kobj_class_t;
/* A driver module is a pointer bsdkpi_driver_NAME the shim looks at. */
#define DRIVER_MODULE(name, bus, driver, evh, arg) \
	driver_t *bsdkpi_driver_##name = &(driver)
#define MODULE_DEPEND(a, b, c, d, e)
#define MODULE_VERSION(a, b)
#define MODULE_PNP_INFO(d, b, u, t, n)
#define DECLARE_MODULE(a, b, c, d)
#define EARLY_DRIVER_MODULE(a, b, c, d, e, f)
struct bsdkpi_device;
typedef struct bsdkpi_device *device_t;
void *device_get_softc(device_t dev);
void device_set_desc(device_t dev, const char *desc);
void device_set_desc_copy(device_t dev, const char *desc);
const char *device_get_nameunit(device_t dev);
const char *device_get_name(device_t dev);
int device_get_unit(device_t dev);
int device_printf(device_t dev, const char *fmt, ...) __printflike(2, 3);
#define BUS_PROBE_DEFAULT (-20)
#define bus_generic_print_child NULL
#define bus_generic_driver_added NULL
typedef int driver_filter_t(void *);
typedef void driver_intr_t(void *);
#define FILTER_STRAY 0x01
#define FILTER_HANDLED 0x02
#define FILTER_SCHEDULE_THREAD 0x04
#define INTR_TYPE_NET 4
#define INTR_MPSAFE 512

/* ---- resources (sys/rman.h, machine/resource.h) ---- */
#define SYS_RES_IRQ 1
#define SYS_RES_IOPORT 4
#define SYS_RES_MEMORY 3
#define RF_ACTIVE 0x0002
#define RF_SHAREABLE 0x0004
typedef uintptr_t bus_space_tag_t;
typedef uintptr_t bus_space_handle_t;
struct resource {
	int type;
	int rid;
	bus_space_tag_t tag;
	bus_space_handle_t handle;
	uint64_t size;
};
struct resource *bus_alloc_resource_any(device_t dev, int type, int *rid, unsigned flags);
int bus_release_resource(device_t dev, int type, int rid, struct resource *r);
#define rman_get_bustag(r) ((r)->tag)
#define rman_get_bushandle(r) ((r)->handle)
#define rman_get_rid(r) ((r)->rid)
#define rman_get_start(r) ((rman_res_t)(r)->handle)
#define rman_get_size(r) ((r)->size)

/* ---- bus_space (machine/bus.h): memory BARs mapped into this program ---- */
static inline uint8_t bus_space_read_1(bus_space_tag_t t, bus_space_handle_t h, bus_size_t o)
{ (void)t; return *(volatile uint8_t *)(h + o); }
static inline uint16_t bus_space_read_2(bus_space_tag_t t, bus_space_handle_t h, bus_size_t o)
{ (void)t; return *(volatile uint16_t *)(h + o); }
static inline uint32_t bus_space_read_4(bus_space_tag_t t, bus_space_handle_t h, bus_size_t o)
{ (void)t; return *(volatile uint32_t *)(h + o); }
static inline void bus_space_write_1(bus_space_tag_t t, bus_space_handle_t h, bus_size_t o, uint8_t v)
{ (void)t; *(volatile uint8_t *)(h + o) = v; }
static inline void bus_space_write_2(bus_space_tag_t t, bus_space_handle_t h, bus_size_t o, uint16_t v)
{ (void)t; *(volatile uint16_t *)(h + o) = v; }
static inline void bus_space_write_4(bus_space_tag_t t, bus_space_handle_t h, bus_size_t o, uint32_t v)
{ (void)t; *(volatile uint32_t *)(h + o) = v; }
#define BUS_SPACE_MAXADDR 0xFFFFFFFFFFFFFFFFUL
#define BUS_SPACE_MAXADDR_32BIT 0xFFFFFFFFUL
#define BUS_SPACE_MAXSIZE_32BIT 0xFFFFFFFFUL

/* ---- bus_dma (machine/bus.h) ---- */
typedef struct bus_dma_segment { bus_addr_t ds_addr; bus_size_t ds_len; } bus_dma_segment_t;
typedef struct bsdkpi_dma_tag *bus_dma_tag_t;
typedef struct bsdkpi_dma_map *bus_dmamap_t;
#define BUS_DMA_NOWAIT 1
#define BUS_DMASYNC_PREREAD 1
#define BUS_DMASYNC_POSTREAD 2
#define BUS_DMASYNC_PREWRITE 4
#define BUS_DMASYNC_POSTWRITE 8
#define bus_dmamap_sync(t, m, op) do { (void)(t); (void)(m); __asm__ __volatile__("" ::: "memory"); } while (0)

/* ---- PCI (dev/pci/pcivar.h) ---- */
uint32_t pci_read_config(device_t dev, int reg, int width);
void pci_write_config(device_t dev, int reg, uint32_t val, int width);
uint16_t pci_get_vendor(device_t dev);
uint16_t pci_get_device(device_t dev);
uint16_t pci_get_subvendor(device_t dev);
uint16_t pci_get_subdevice(device_t dev);
uint8_t pci_get_revid(device_t dev);
int pci_find_cap(device_t dev, int cap, int *capreg);
int pci_msix_table_bar(device_t dev);
int pci_msix_count(device_t dev);
int pci_has_pm(device_t dev);
void pci_enable_pme(device_t dev);
void pci_enable_busmaster(device_t dev);
#define pci_get_class(d) 2
#define pci_get_subclass(d) 0

/* ---- sysctl: accepted and ignored ---- */
struct sysctl_oid { void *oid_arg1; intmax_t oid_arg2; };
struct sysctl_ctx_list { int unused; };
struct sysctl_oid_list { int unused; };
struct sysctl_req { void *newptr; size_t newlen; };
#define SYSCTL_HANDLER_ARGS struct sysctl_oid *oidp, void *arg1, intmax_t arg2, struct sysctl_req *req
#define SYSCTL_NODE(...) struct sysctl_oid_list bsdkpi_unused_node_##__LINE__ __unused
#define SYSCTL_INT(...)
#define SYSCTL_UINT(...)
#define SYSCTL_BOOL(...)
#define SYSCTL_DECL(...)
#define SYSCTL_ADD_PROC(...) ((struct sysctl_oid *)0)
#define SYSCTL_ADD_NODE(...) ((struct sysctl_oid *)0)
#define SYSCTL_ADD_INT(...) ((struct sysctl_oid *)0)
#define SYSCTL_ADD_UINT(...) ((struct sysctl_oid *)0)
#define SYSCTL_ADD_ULONG(...) ((struct sysctl_oid *)0)
#define SYSCTL_ADD_UQUAD(...) ((struct sysctl_oid *)0)
#define SYSCTL_ADD_U32(...) ((struct sysctl_oid *)0)
#define SYSCTL_ADD_U64(...) ((struct sysctl_oid *)0)
#define SYSCTL_CHILDREN(o) ((struct sysctl_oid_list *)0)
#define SYSCTL_STATIC_CHILDREN(...) ((struct sysctl_oid_list *)0)
#define CTLFLAG_RD 0
#define CTLFLAG_RW 0
#define CTLFLAG_RDTUN 0
#define CTLFLAG_RWTUN 0
#define CTLFLAG_MPSAFE 0
#define CTLFLAG_NEEDGIANT 0
#define CTLTYPE_INT 0
#define CTLTYPE_UINT 0
#define CTLTYPE_STRING 0
#define CTLTYPE_U32 0
#define CTLTYPE_U64 0
#define OID_AUTO (-1)
#define TUNABLE_INT(...)
#define TUNABLE_INT_FETCH(n, p)
int sysctl_handle_int(SYSCTL_HANDLER_ARGS);
int sysctl_handle_string(SYSCTL_HANDLER_ARGS);
int sysctl_wire_old_buffer(struct sysctl_req *req, size_t len);
struct sysctl_ctx_list *device_get_sysctl_ctx(device_t dev);
struct sysctl_oid *device_get_sysctl_tree(device_t dev);
struct sbuf { int unused; };
extern struct sbuf bsdkpi_no_sbuf;
#define sbuf_new_for_sysctl(...) (&bsdkpi_no_sbuf)
#define sbuf_printf(...) 0
#define sbuf_finish(s) 0
#define sbuf_delete(s) ((void)0)
#define sbuf_cat(...) 0
#define sbuf_new_auto() (&bsdkpi_no_sbuf)
#define sbuf_len(s) 0
#define sbuf_data(s) ""
#define sbuf_trim(s) 0

/* ---- callouts and tasks: run from the driver thread's loop ---- */
struct callout {
	void (*func)(void *);
	void *arg;
	uint64_t when;
	int active;
	struct callout *next;
};
void callout_init(struct callout *c, int mpsafe);
#define callout_init_mtx(c, m, f) callout_init((c), 1)
int callout_reset(struct callout *c, int ticks, void (*func)(void *), void *arg);
int callout_stop(struct callout *c);
#define callout_drain(c) callout_stop(c)
#define callout_pending(c) ((c)->active)
struct task {
	void (*func)(void *, int);
	void *arg;
	int pending;
};
#define TASK_INIT(t, p, f, a) do { (t)->func = (f); (t)->arg = (a); (t)->pending = 0; } while (0)
struct taskqueue;
int taskqueue_enqueue(struct taskqueue *q, struct task *t);
#define taskqueue_drain(q, t) ((void)0)
#define taskqueue_free(q) ((void)0)
extern struct taskqueue *taskqueue_thread;
extern struct taskqueue *taskqueue_fast;
struct grouptask {
	struct task gt_task;
	void *gt_uniq;
};
#define GROUPTASK_INIT(gt, p, f, a) TASK_INIT(&(gt)->gt_task, p, f, a)
struct taskqgroup;
#define DEVICE_POLLING_unused

/* ---- events, ddb, led, eventhandler: nothing ---- */
#define EVENTHANDLER_REGISTER(...) ((void *)0)
#define EVENTHANDLER_DEREGISTER(...) ((void)0)
#define DB_COMMAND(...) static void bsdkpi_unused_db(void)
struct cdev;
#define led_create(f, a, n) ((struct cdev *)0)
#define led_destroy(c) ((void)0)
#define SYSINIT(...)

/* ---- counters ---- */
typedef uint64_t *counter_u64_t;
#define counter_u64_add(c, v) ((void)(c), (void)(v))

/* ---- mbufs (sys/mbuf.h): one buffer each, no chains of clusters ---- */
struct pkthdr {
	struct ifnet *rcvif;
	int len;
	uint32_t csum_flags;
	uint32_t csum_data;
	uint16_t tso_segsz;
	uint16_t ether_vtag;
	uint32_t flowid;
};
struct mbuf {
	struct mbuf *m_next;
	struct mbuf *m_nextpkt;
	char *m_data;
	int m_len;
	int m_flags;
	struct pkthdr m_pkthdr;
	char *m_buf;
	int m_size;
	bus_addr_t m_phys;
};
#define M_PKTHDR 0x0002
#define M_VLANTAG 0x0080
#define M_BCAST 0x0010
#define M_MCAST 0x0020
#define M_HASHTYPE_OPAQUE 0
#define M_HASHTYPE_SET(m, t) ((void)(m))
#define MT_DATA 1
#define mtod(m, t) ((t)((m)->m_data))
#define CSUM_IP 0x0001
#define CSUM_TCP 0x0002
#define CSUM_UDP 0x0004
#define CSUM_TSO 0x0010
#define CSUM_IP_TSO CSUM_TSO
#define CSUM_IP6_TSO 0x0020
#define CSUM_IP6_TCP 0x0040
#define CSUM_IP6_UDP 0x0080
#define CSUM_IP_CHECKED 0x0100
#define CSUM_IP_VALID 0x0200
#define CSUM_DATA_VALID 0x0400
#define CSUM_PSEUDO_HDR 0x0800
#define CSUM_L3_CALC CSUM_IP_CHECKED
#define CSUM_L3_VALID CSUM_IP_VALID
#define CSUM_L4_CALC CSUM_DATA_VALID
#define CSUM_L4_VALID CSUM_PSEUDO_HDR
#define CSUM_IP_SCTP 0x1000
#define CSUM_IP6_SCTP 0x2000
#define CSUM_SCTP_VALID 0x4000
#define CSUM_OFFLOAD (CSUM_IP | CSUM_TCP | CSUM_UDP)
#define CSUM_IP_UDP CSUM_UDP
#define CSUM_IP_TCP CSUM_TCP
#define CSUM_SCTP CSUM_IP_SCTP
#define M_HASHTYPE_RSS_TCP_IPV4 1
#define M_HASHTYPE_RSS_IPV4 2
#define M_HASHTYPE_RSS_TCP_IPV6 3
#define M_HASHTYPE_RSS_IPV6_EX 4
#define M_HASHTYPE_RSS_IPV6 5
#define M_HASHTYPE_RSS_TCP_IPV6_EX 6
#define M_HASHTYPE_RSS_UDP_IPV4 7
#define M_HASHTYPE_RSS_UDP_IPV6 8
#define M_HASHTYPE_RSS_UDP_IPV6_EX 9
#define M_HASHTYPE_NONE 0
void m_freem(struct mbuf *m);
struct mbuf *m_getcl(int how, short type, int flags);
struct mbuf *m_get(int how, short type);
void m_adj(struct mbuf *m, int len);
struct mbuf *m_pullup(struct mbuf *m, int len);
void m_copydata(const struct mbuf *m, int off, int len, caddr_t cp);

/* ---- sockets, ifnet (net/if.h, net/if_var.h) ---- */
#define ETHER_ADDR_LEN 6
#define ETHER_HDR_LEN 14
#define ETHER_CRC_LEN 4
#define ETHER_MIN_LEN 64
#define ETHER_MAX_LEN 1518
#define ETHER_VLAN_ENCAP_LEN 4
#define ETHERMTU 1500
#define ETHERTYPE_IP 0x0800
#define ETHERTYPE_IPV6 0x86DD
#define ETHERTYPE_VLAN 0x8100
#define ETHER_IS_MULTICAST(a) ((a)[0] & 1)
#define ETHER_BPF_MTAP(ifp, m) ((void)0)
struct ether_header { uint8_t ether_dhost[6]; uint8_t ether_shost[6]; uint16_t ether_type; } __packed;
struct ether_vlan_header {
	uint8_t evl_dhost[6]; uint8_t evl_shost[6]; uint16_t evl_encap_proto; uint16_t evl_tag; uint16_t evl_proto;
} __packed;
struct ifnet;
typedef struct ifnet *if_t;
struct sockaddr_dl;
#define IFF_UP 0x1
#define IFF_BROADCAST 0x2
#define IFF_PROMISC 0x100
#define IFF_ALLMULTI 0x200
#define IFF_MULTICAST 0x8000
#define IFF_DRV_RUNNING 0x40
#define IFF_DRV_OACTIVE 0x400
#define IFCAP_RXCSUM 0x00001
#define IFCAP_TXCSUM 0x00002
#define IFCAP_NETCONS 0x00004
#define IFCAP_VLAN_MTU 0x00008
#define IFCAP_VLAN_HWTAGGING 0x00010
#define IFCAP_JUMBO_MTU 0x00020
#define IFCAP_POLLING 0x00040
#define IFCAP_VLAN_HWCSUM 0x00080
#define IFCAP_TSO4 0x00100
#define IFCAP_TSO6 0x00200
#define IFCAP_LRO 0x00400
#define IFCAP_WOL_UCAST 0x00800
#define IFCAP_WOL_MCAST 0x01000
#define IFCAP_WOL_MAGIC 0x02000
#define IFCAP_TOE4 0x04000
#define IFCAP_TOE6 0x08000
#define IFCAP_VLAN_HWFILTER 0x10000
#define IFCAP_VLAN_HWTSO 0x40000
#define IFCAP_LINKSTATE 0x80000
#define IFCAP_NETMAP 0x100000
#define IFCAP_RXCSUM_IPV6 0x200000
#define IFCAP_TXCSUM_IPV6 0x400000
#define IFCAP_HWSTATS 0x800000
#define IFCAP_HWCSUM (IFCAP_RXCSUM | IFCAP_TXCSUM)
#define IFCAP_TSO (IFCAP_TSO4 | IFCAP_TSO6)
#define IFCAP_WOL (IFCAP_WOL_UCAST | IFCAP_WOL_MCAST | IFCAP_WOL_MAGIC)
#define IFCAP_HWCSUM_IPV6 (IFCAP_RXCSUM_IPV6 | IFCAP_TXCSUM_IPV6)
#define LINK_STATE_UNKNOWN 0
#define LINK_STATE_DOWN 1
#define LINK_STATE_UP 2
#define IF_Mbps(x) ((uint64_t)(x) * 1000 * 1000)
#define IF_Gbps(x) ((uint64_t)(x) * 1000 * 1000 * 1000)
typedef enum {
	IFCOUNTER_IPACKETS, IFCOUNTER_IERRORS, IFCOUNTER_OPACKETS, IFCOUNTER_OERRORS,
	IFCOUNTER_COLLISIONS, IFCOUNTER_IBYTES, IFCOUNTER_OBYTES, IFCOUNTER_IMCASTS,
	IFCOUNTER_OMCASTS, IFCOUNTER_IQDROPS, IFCOUNTER_OQDROPS, IFCOUNTER_NOPROTO,
	IFCOUNTERS
} ift_counter;
uint64_t if_get_counter_default(if_t ifp, ift_counter c);
int if_getmtu(if_t ifp);
int if_getflags(if_t ifp);
int if_getdrvflags(if_t ifp);
int if_getcapenable(if_t ifp);
void if_setcapenablebit(if_t ifp, int set, int clear);
uint8_t *if_getlladdr(if_t ifp);
int if_vlantrunkinuse(if_t ifp);
void if_setsendqready(if_t ifp);
void if_setsendqlen(if_t ifp, int len);
typedef unsigned iflladdr_cb_t(void *, struct sockaddr_dl *, unsigned);
unsigned if_foreach_llmaddr(if_t ifp, iflladdr_cb_t cb, void *arg);
unsigned if_llmaddr_count(if_t ifp);
#define LLADDR(sdl) ((uint8_t *)(sdl))
struct ether_addr { uint8_t octet[6]; };
void ether_gen_addr(if_t ifp, struct ether_addr *hwaddr);
#define VLAN_CAPABILITIES(ifp) ((void)0)
#define EVL_VLID_MASK 0x0FFF
#define EVL_VLANOFTAG(tag) ((tag) & EVL_VLID_MASK)
#define ETHER_VLAN_ENCAP_LEN 4
#define IFQ_MAXLEN 50
struct ifmediareq {
	char ifm_name[16];
	int ifm_current;
	int ifm_mask;
	int ifm_status;
	int ifm_active;
	int ifm_count;
	int *ifm_ulist;
};
#define SIOCSIFMTU 1
#define SIOCSIFCAP 2

/* ---- IP headers (netinet/*.h): only for offload; not used here ---- */
struct ip { uint8_t ip_hl_v; uint8_t ip_tos; uint16_t ip_len; uint16_t ip_id; uint16_t ip_off;
	uint8_t ip_ttl; uint8_t ip_p; uint16_t ip_sum; uint32_t ip_src; uint32_t ip_dst; };
struct ip6_hdr { uint32_t ip6_flow; uint16_t ip6_plen; uint8_t ip6_nxt; uint8_t ip6_hlim; uint8_t ip6_src[16]; uint8_t ip6_dst[16]; };
struct tcphdr { uint16_t th_sport; uint16_t th_dport; uint32_t th_seq; uint32_t th_ack; uint8_t th_x2_off; uint8_t th_flags;
	uint16_t th_win; uint16_t th_sum; uint16_t th_urp; };
struct udphdr { uint16_t uh_sport; uint16_t uh_dport; uint16_t uh_ulen; uint16_t uh_sum; };
#define IPPROTO_TCP 6
#define IPPROTO_UDP 17
#define IPPROTO_SCTP 132
#define TH_FIN 0x01
#define TH_SYN 0x02
#define TH_RST 0x04
#define TH_PUSH 0x08
#define TH_ACK 0x10
#define TH_URG 0x20
#define TH_ECE 0x40
#define TH_CWR 0x80

/* ---- smp, priv, misc ---- */
#define CPU_FOREACH(i) for ((i) = 0; (i) < 1; (i)++)
#define bus_get_cpus(...) EOPNOTSUPP
#define INTR_CPUS 0
#define ticks_to_ms(t) (t)

/* The FreeBSD headers used as they are (BSD-2-Clause / BSD-3-Clause). */
#include "../../freebsd/sys/queue.h"
#include "../../freebsd/dev/pci/pcireg.h"
#include "../../freebsd/net/if_media.h"
struct cpuset { int unused; };
typedef struct nvlist nvlist_t;
typedef void task_fn_t(void *context, int pending);
typedef void gtask_fn_t(void *context);
typedef struct cpuset cpuset_t;
#include "../../freebsd/net/iflib.h"

/* ---- the iflib methods the driver calls itself (ifdi_if.m) ---- */
void IFDI_INTR_DISABLE(if_ctx_t ctx);
void IFDI_INTR_ENABLE(if_ctx_t ctx);
#endif
