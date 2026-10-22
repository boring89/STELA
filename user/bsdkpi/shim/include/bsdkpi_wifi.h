/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: MIT
 *
 * bsdkpi: what FreeBSD's net80211 headers and iwm need on
 * top of bsdkpi.h (P14 K15, docs/bsdkpi.md). Compile-only: the driver
 * links into bsdnet but is not attached; the functions it needs from
 * net80211 and the kernel are stubs (shim/wifi_stubs.c).
 */
#ifndef BSDKPI_WIFI_H
#define BSDKPI_WIFI_H

#define IFNAMSIZ 16
#define roundup(x, y) ((((x) + ((y) - 1)) / (y)) * (y))
#define NBBY 8
#define setbit(a, i) (((unsigned char *)(a))[(i) / NBBY] |= 1 << ((i) % NBBY))
#define clrbit(a, i) (((unsigned char *)(a))[(i) / NBBY] &= ~(1 << ((i) % NBBY)))
#define isset(a, i) (((const unsigned char *)(a))[(i) / NBBY] & (1 << ((i) % NBBY)))
#define isclr(a, i) (!isset(a, i))

struct timeval { time_t tv_sec; long tv_usec; };
struct timespec { time_t tv_sec; long tv_nsec; };

/* linker sets: nothing collects them here */
#define SET_DECLARE(set, ptype) extern ptype *__CONCAT(__start_set_, set)
#define TEXT_SET(set, sym)
#define DATA_SET(set, sym)
#ifndef __CONCAT
#define __CONCAT1(x, y) x##y
#define __CONCAT(x, y) __CONCAT1(x, y)
#endif

/* mbufs */
#define M_PROTO1 0x00001000
#define M_PROTO2 0x00002000
#define M_PROTO3 0x00004000
#define M_PROTO4 0x00008000
#define M_PROTO5 0x00010000
#define M_PROTO6 0x00020000
#define M_PROTO7 0x00040000
#define M_PROTO8 0x00080000
#define M_PROTO9 0x00100000
#define M_PROTO10 0x00200000
#define M_PROTO11 0x00400000
#define M_PROTO12 0x00800000
#define MJUMPAGESIZE 4096
#define MJUM9BYTES (9 * 1024)
#define MJUM16BYTES (16 * 1024)
#define MCLBYTES 2048
#define MHLEN 160
#define mtodo(m, o) ((void *)(((m)->m_data) + (o)))
struct mbuf *m_getjcl(int how, short type, int flags, int size);
struct mbuf *m_collapse(struct mbuf *m, int how, int maxfrags);
struct mbuf *m_defrag(struct mbuf *m, int how);
struct mbufq {
	struct mbuf *mq_head;
	struct mbuf *mq_tail;
	int mq_len;
	int mq_maxlen;
};
void mbufq_init(struct mbufq *mq, int maxlen);
int mbufq_enqueue(struct mbufq *mq, struct mbuf *m);
struct mbuf *mbufq_dequeue(struct mbufq *mq);
void mbufq_drain(struct mbufq *mq);
#define mbufq_len(mq) ((mq)->mq_len)
#define mbufq_full(mq) ((mq)->mq_len >= (mq)->mq_maxlen)
#define mbufq_first(mq) ((mq)->mq_head)

/* sleeping (one thread: these never wait) */
int msleep(void *chan, struct mtx *m, int pri, const char *wmesg, int timo);
void wakeup(void *chan);
void wakeup_one(void *chan);
#define PCATCH 0x100

/* firmware(9) */
struct firmware {
	const char *name;
	const void *data;
	size_t datasize;
	unsigned version;
};
#define FIRMWARE_UNLOAD 0x0001
const struct firmware *firmware_get(const char *name);
void firmware_put(const struct firmware *fw, int flags);

/* config_intrhook */
struct intr_config_hook {
	void (*ich_func)(void *);
	void *ich_arg;
};
int config_intrhook_establish(struct intr_config_hook *hook);
void config_intrhook_disestablish(struct intr_config_hook *hook);

/* bus_dma(9) with tags and maps */
typedef int bus_dma_filter_t(void *, bus_addr_t);
typedef void bus_dmamap_callback_t(void *, bus_dma_segment_t *, int, int);
typedef void bus_dma_lock_t(void *, int);
int bus_dma_tag_create(bus_dma_tag_t parent, bus_size_t alignment, bus_addr_t boundary,
    bus_addr_t lowaddr, bus_addr_t highaddr, bus_dma_filter_t *filter, void *filterarg,
    bus_size_t maxsize, int nsegments, bus_size_t maxsegsz, int flags,
    bus_dma_lock_t *lockfunc, void *lockfuncarg, bus_dma_tag_t *dmat);
int bus_dma_tag_destroy(bus_dma_tag_t dmat);
int bus_dmamap_create(bus_dma_tag_t dmat, int flags, bus_dmamap_t *mapp);
int bus_dmamap_destroy(bus_dma_tag_t dmat, bus_dmamap_t map);
int bus_dmamem_alloc(bus_dma_tag_t dmat, void **vaddr, int flags, bus_dmamap_t *mapp);
void bus_dmamem_free(bus_dma_tag_t dmat, void *vaddr, bus_dmamap_t map);
int bus_dmamap_load(bus_dma_tag_t dmat, bus_dmamap_t map, void *buf, bus_size_t buflen,
    bus_dmamap_callback_t *callback, void *arg, int flags);
int bus_dmamap_load_mbuf_sg(bus_dma_tag_t dmat, bus_dmamap_t map, struct mbuf *m,
    bus_dma_segment_t *segs, int *nsegs, int flags);
void bus_dmamap_unload(bus_dma_tag_t dmat, bus_dmamap_t map);
#define BUS_SPACE_MAXADDR_32BIT 0xFFFFFFFFUL
#define BUS_SPACE_MAXADDR 0xFFFFFFFFFFFFFFFFUL
#define BUS_DMA_NOWAIT 0x01
#define BUS_DMA_WAITOK 0x00
#define BUS_DMA_COHERENT 0x04
#define BUS_DMA_ZERO 0x08
#define BUS_DMA_ALLOCNOW 0x02

/* time */
void getmicrotime(struct timeval *tv);
void microtime(struct timeval *tv);
void getbinuptime(void *bt);
int ppsratecheck(struct timeval *lasttime, int *curpps, int maxpps);
int ratecheck(struct timeval *lasttime, const struct timeval *mininterval);
#define SBT_1MS ((sbintime_t)1 << 32 / 1000)


#define ENETDOWN 50
#ifndef EWOULDBLOCK
#define EWOULDBLOCK 35
#endif
void wakeup_one(void *chan);
int bsdkpi_iwm_probe(uint16_t vendor, uint16_t device, char *desc, size_t n);
#define M_COPYALL 1000000000
struct mbuf *m_copym(struct mbuf *m, int off, int len, int how);
char *ether_sprintf(const u_char *ap);
void if_inc_counter(if_t ifp, ift_counter cnt, int64_t inc);
extern int ifqmaxlen;
int pci_alloc_msi(device_t dev, int *count);
int pci_release_msi(device_t dev);
int pci_msi_count(device_t dev);
int bus_setup_intr(device_t dev, struct resource *r, int flags, driver_filter_t *filter,
    driver_intr_t *handler, void *arg, void **cookiep);
int bus_teardown_intr(device_t dev, struct resource *r, void *cookie);
bus_dma_tag_t bus_get_dma_tag(device_t dev);
typedef void (*taskqueue_enqueue_fn)(void *context);
struct taskqueue *taskqueue_create(const char *name, int mflags, taskqueue_enqueue_fn enqueue, void *context);
void taskqueue_thread_enqueue(void *context);
int taskqueue_start_threads(struct taskqueue **tqp, int count, int pri, const char *name, ...);
int taskqueue_cancel(struct taskqueue *queue, struct task *task, u_int *pendp);
void taskqueue_drain_all(struct taskqueue *queue);

#define BUS_SPACE_BARRIER_READ 0x01
#define BUS_SPACE_BARRIER_WRITE 0x02
#define bus_space_barrier(t, h, o, l, f) __asm__ __volatile__("mfence" ::: "memory")

#endif
