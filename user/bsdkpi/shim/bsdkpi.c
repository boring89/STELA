/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: MIT
 *
 * bsdkpi (P14 K13-K14, docs/bsdkpi.md): the FreeBSD kernel functions
 * if_em calls, and a small iflib (one transmit and one receive queue,
 * one buffer per descriptor, legacy or MSI interrupts), for a STELA user
 * program. The program's Rust half (src/ffi.rs) gives the bsdkpi_host_*
 * functions and calls the bsdkpi_em_* ones; everything runs in its one
 * thread.
 */
#include <bsdkpi.h>

/* ---- what the Rust half gives (src/ffi.rs) ---- */
void bsdkpi_host_print(const char *s, size_t n);
void *bsdkpi_host_alloc(size_t n);
void bsdkpi_host_free(void *p, size_t n);
void *bsdkpi_host_dma(size_t bytes, uint64_t *phys);
uint64_t bsdkpi_host_now_us(void);
void bsdkpi_host_delay_us(uint64_t us);
uint32_t bsdkpi_host_pci_read(int reg, int width);
void bsdkpi_host_pci_write(int reg, uint32_t val, int width);
void *bsdkpi_host_bar(int index, uint64_t *size);
void bsdkpi_host_input(const void *frame, size_t len);
void bsdkpi_host_link(int up, uint64_t baudrate);

/* ---- libc ---- */
void *memcpy(void *d, const void *s, size_t n)
{
	uint8_t *dd = d;
	const uint8_t *ss = s;
	while (n--)
		*dd++ = *ss++;
	return d;
}

void *memmove(void *d, const void *s, size_t n)
{
	uint8_t *dd = d;
	const uint8_t *ss = s;
	if (dd < ss) {
		while (n--)
			*dd++ = *ss++;
	} else {
		while (n--)
			dd[n] = ss[n];
	}
	return d;
}

void *memset(void *d, int c, size_t n)
{
	uint8_t *dd = d;
	while (n--)
		*dd++ = (uint8_t)c;
	return d;
}

int memcmp(const void *a, const void *b, size_t n)
{
	const uint8_t *x = a, *y = b;
	for (; n; n--, x++, y++)
		if (*x != *y)
			return *x - *y;
	return 0;
}

size_t strlen(const char *s)
{
	size_t n = 0;
	while (s[n])
		n++;
	return n;
}

int strcmp(const char *a, const char *b)
{
	while (*a && *a == *b)
		a++, b++;
	return (uint8_t)*a - (uint8_t)*b;
}

int strncmp(const char *a, const char *b, size_t n)
{
	for (; n; n--, a++, b++) {
		if (*a != *b || !*a)
			return (uint8_t)*a - (uint8_t)*b;
	}
	return 0;
}

char *strncpy(char *d, const char *s, size_t n)
{
	size_t i = 0;
	for (; i < n && s[i]; i++)
		d[i] = s[i];
	for (; i < n; i++)
		d[i] = 0;
	return d;
}

size_t strlcpy(char *d, const char *s, size_t n)
{
	size_t len = strlen(s);
	if (n) {
		size_t c = len < n - 1 ? len : n - 1;
		memcpy(d, s, c);
		d[c] = 0;
	}
	return len;
}

/* A small vsnprintf: %d %i %u %x %X %p %s %c %%, with l, ll, z, h, #, 0
 * and a width. */
int vsnprintf(char *buf, size_t n, const char *fmt, va_list ap)
{
	size_t at = 0;
#define PUT(c) do { if (at + 1 < n) buf[at] = (c); at++; } while (0)
	for (; *fmt; fmt++) {
		if (*fmt != '%') {
			PUT(*fmt);
			continue;
		}
		fmt++;
		int alt = 0, zero = 0, width = 0, longs = 0;
		for (;; fmt++) {
			if (*fmt == '#')
				alt = 1;
			else if (*fmt == '0')
				zero = 1;
			else
				break;
		}
		while (*fmt >= '0' && *fmt <= '9')
			width = width * 10 + (*fmt++ - '0');
		while (*fmt == 'l' || *fmt == 'z' || *fmt == 'j' || *fmt == 'q') {
			longs++;
			fmt++;
		}
		while (*fmt == 'h')
			fmt++;
		char tmp[32];
		int len = 0, neg = 0;
		unsigned long long v = 0;
		switch (*fmt) {
		case 'd':
		case 'i': {
			long long s = longs ? va_arg(ap, long long) : va_arg(ap, int);
			if (s < 0) {
				neg = 1;
				v = (unsigned long long)(-s);
			} else {
				v = (unsigned long long)s;
			}
			goto decimal;
		}
		case 'u':
			v = longs ? va_arg(ap, unsigned long long) : va_arg(ap, unsigned);
		decimal:
			do {
				tmp[len++] = (char)('0' + v % 10);
				v /= 10;
			} while (v);
			if (neg)
				tmp[len++] = '-';
			break;
		case 'p':
			alt = 1;
			longs = 2;
			/* fall through */
		case 'x':
		case 'X':
			v = longs ? va_arg(ap, unsigned long long) : va_arg(ap, unsigned);
			do {
				int d = (int)(v & 15);
				tmp[len++] = (char)(d < 10 ? '0' + d : (*fmt == 'X' ? 'A' : 'a') + d - 10);
				v >>= 4;
			} while (v);
			if (alt) {
				tmp[len++] = 'x';
				tmp[len++] = '0';
			}
			break;
		case 's': {
			const char *s = va_arg(ap, const char *);
			if (!s)
				s = "(null)";
			for (int i = (int)strlen(s); i < width; i++)
				PUT(' ');
			while (*s)
				PUT(*s++);
			continue;
		}
		case 'c':
			PUT((char)va_arg(ap, int));
			continue;
		case '%':
			PUT('%');
			continue;
		default:
			PUT('?');
			continue;
		}
		for (int i = len; i < width; i++)
			PUT(zero ? '0' : ' ');
		while (len)
			PUT(tmp[--len]);
	}
	if (n)
		buf[at < n ? at : n - 1] = 0;
#undef PUT
	return (int)at;
}

int snprintf(char *buf, size_t n, const char *fmt, ...)
{
	va_list ap;
	va_start(ap, fmt);
	int r = vsnprintf(buf, n, fmt, ap);
	va_end(ap);
	return r;
}

static void vprint(const char *prefix, const char *fmt, va_list ap)
{
	char buf[256];
	size_t p = strlen(prefix);
	memcpy(buf, prefix, p);
	vsnprintf(buf + p, sizeof buf - p, fmt, ap);
	bsdkpi_host_print(buf, strlen(buf));
}

int printf(const char *fmt, ...)
{
	va_list ap;
	va_start(ap, fmt);
	vprint("", fmt, ap);
	va_end(ap);
	return 0;
}

void panic(const char *fmt, ...)
{
	va_list ap;
	va_start(ap, fmt);
	vprint("bsdnet: panic: ", fmt, ap);
	va_end(ap);
	for (;;)
		bsdkpi_host_delay_us(1000000);
}

/* ---- malloc(9): a header with the size ---- */
struct malloc_type M_DEVBUF[1] = { { "devbuf" } };

void *malloc(size_t size, struct malloc_type *type, int flags)
{
	(void)type;
	size_t total = size + 16;
	uint64_t *p = bsdkpi_host_alloc(total);
	if (!p)
		return NULL;
	p[0] = total;
	if (flags & M_ZERO)
		memset(p + 2, 0, size);
	return p + 2;
}

void free(void *ptr, struct malloc_type *type)
{
	(void)type;
	if (!ptr)
		return;
	uint64_t *p = (uint64_t *)ptr - 2;
	bsdkpi_host_free(p, p[0]);
}

void *mallocarray(size_t n, size_t size, struct malloc_type *type, int flags)
{
	return malloc(n * size, type, flags);
}

/* ---- time ---- */
volatile uint64_t ticks;


void DELAY(int usec)
{
	bsdkpi_host_delay_us(usec > 0 ? (uint64_t)usec : 0);
}

int pause(const char *wmesg, int timo)
{
	(void)wmesg;
	bsdkpi_host_delay_us((uint64_t)(timo > 0 ? timo : 1) * 1000);
	return 0;
}

static uint32_t random_state = 0x2545F491;

uint32_t arc4random(void)
{
	random_state ^= random_state << 13;
	random_state ^= random_state >> 17;
	random_state ^= random_state << 5;
	return random_state ^ (uint32_t)bsdkpi_host_now_us();
}

void arc4rand(void *p, unsigned n, int reseed)
{
	(void)reseed;
	uint8_t *b = p;
	while (n--)
		*b++ = (uint8_t)arc4random();
}

/* ---- callouts and tasks ---- */
static struct callout *callouts;
struct taskqueue *taskqueue_thread;
struct taskqueue *taskqueue_fast;

void callout_init(struct callout *c, int mpsafe)
{
	(void)mpsafe;
	memset(c, 0, sizeof *c);
}

int callout_stop(struct callout *c)
{
	int was = c->active;
	c->active = 0;
	for (struct callout **p = &callouts; *p; p = &(*p)->next) {
		if (*p == c) {
			*p = c->next;
			break;
		}
	}
	return was;
}

int callout_reset(struct callout *c, int t, void (*func)(void *), void *arg)
{
	int was = callout_stop(c);
	c->func = func;
	c->arg = arg;
	c->when = bsdkpi_host_now_us() / 1000 + (uint64_t)(t > 0 ? t : 1);
	c->active = 1;
	c->next = callouts;
	callouts = c;
	return was;
}

static void run_callouts(void)
{
	uint64_t now = bsdkpi_host_now_us() / 1000;
	ticks = now;
	for (int again = 1; again;) {
		again = 0;
		for (struct callout *c = callouts; c; c = c->next) {
			if (c->active && c->when <= now) {
				callout_stop(c);
				c->func(c->arg);
				again = 1;
				break;
			}
		}
	}
}

int taskqueue_enqueue(struct taskqueue *q, struct task *t)
{
	(void)q;
	t->func(t->arg, 1);
	return 0;
}

/* ---- the device ---- */
struct bsdkpi_device {
	void *softc;
	uint16_t vendor;
	uint16_t device;
	uint16_t subvendor;
	uint16_t subdevice;
	uint8_t revid;
	const char *desc;
	struct resource res[4];
	int nres;
};

void *device_get_softc(device_t dev) { return dev->softc; }
void device_set_desc(device_t dev, const char *desc) { dev->desc = desc; }
void device_set_desc_copy(device_t dev, const char *desc) { dev->desc = desc; }
const char *device_get_nameunit(device_t dev) { (void)dev; return "em0"; }
const char *device_get_name(device_t dev) { (void)dev; return "em"; }
int device_get_unit(device_t dev) { (void)dev; return 0; }

int device_printf(device_t dev, const char *fmt, ...)
{
	(void)dev;
	va_list ap;
	va_start(ap, fmt);
	vprint("bsdnet: em0: ", fmt, ap);
	va_end(ap);
	return 0;
}

struct sbuf bsdkpi_no_sbuf;
static struct sysctl_ctx_list no_sysctl_ctx;
static struct sysctl_oid no_sysctl_oid;
struct sysctl_ctx_list *device_get_sysctl_ctx(device_t dev) { (void)dev; return &no_sysctl_ctx; }
struct sysctl_oid *device_get_sysctl_tree(device_t dev) { (void)dev; return &no_sysctl_oid; }
int sysctl_handle_int(SYSCTL_HANDLER_ARGS) { (void)oidp; (void)arg1; (void)arg2; (void)req; return 0; }
int sysctl_handle_string(SYSCTL_HANDLER_ARGS) { (void)oidp; (void)arg1; (void)arg2; (void)req; return 0; }
int sysctl_wire_old_buffer(struct sysctl_req *req, size_t len) { (void)req; (void)len; return 0; }

uint32_t pci_read_config(device_t dev, int reg, int width) { (void)dev; return bsdkpi_host_pci_read(reg, width); }
void pci_write_config(device_t dev, int reg, uint32_t val, int width) { (void)dev; bsdkpi_host_pci_write(reg, val, width); }
uint16_t pci_get_vendor(device_t dev) { return dev->vendor; }
uint16_t pci_get_device(device_t dev) { return dev->device; }
uint16_t pci_get_subvendor(device_t dev) { return dev->subvendor; }
uint16_t pci_get_subdevice(device_t dev) { return dev->subdevice; }
uint8_t pci_get_revid(device_t dev) { return dev->revid; }
int pci_msix_table_bar(device_t dev) { (void)dev; return -1; }
int pci_msix_count(device_t dev) { (void)dev; return 0; }
int pci_has_pm(device_t dev) { (void)dev; return 0; }
void pci_enable_pme(device_t dev) { (void)dev; }
void pci_enable_busmaster(device_t dev) { (void)dev; }

int pci_find_cap(device_t dev, int cap, int *capreg)
{
	(void)dev;
	if (!(bsdkpi_host_pci_read(PCIR_STATUS, 2) & PCIM_STATUS_CAPPRESENT))
		return ENXIO;
	int p = (int)(bsdkpi_host_pci_read(PCIR_CAP_PTR, 1) & 0xFC);
	for (int n = 0; p && n < 48; n++) {
		if ((int)bsdkpi_host_pci_read(p, 1) == cap) {
			if (capreg)
				*capreg = p;
			return 0;
		}
		p = (int)(bsdkpi_host_pci_read(p + 1, 1) & 0xFC);
	}
	return ENOENT;
}

struct resource *bus_alloc_resource_any(device_t dev, int type, int *rid, unsigned flags)
{
	(void)flags;
	if (dev->nres >= 4)
		return NULL;
	if (type == SYS_RES_IOPORT) {
		/* No port I/O on STELA's Device interface: the old chips' I/O
		 * BAR (E1000_WRITE_REG_IO, only for 82541/82547 workarounds)
		 * writes into this word and nowhere else. */
		static uint32_t no_ioport[4];
		struct resource *r = &dev->res[dev->nres++];
		r->type = type;
		r->rid = *rid;
		r->tag = 0;
		r->handle = (bus_space_handle_t)no_ioport;
		r->size = sizeof no_ioport;
		return r;
	}
	if (type != SYS_RES_MEMORY)
		return NULL;
	int bar = (*rid - PCIR_BAR(0)) / 4;
	uint64_t size = 0;
	void *base = bsdkpi_host_bar(bar, &size);
	if (!base)
		return NULL;
	struct resource *r = &dev->res[dev->nres++];
	r->type = type;
	r->rid = *rid;
	r->tag = 0;
	r->handle = (bus_space_handle_t)base;
	r->size = size;
	return r;
}

int bus_release_resource(device_t dev, int type, int rid, struct resource *r)
{
	(void)dev; (void)type; (void)rid; (void)r;
	return 0;
}

/* ---- mbufs (only where em uses them) ---- */
void m_freem(struct mbuf *m) { (void)m; }
struct mbuf *m_getcl(int how, short type, int flags) { (void)how; (void)type; (void)flags; return NULL; }
struct mbuf *m_get(int how, short type) { (void)how; (void)type; return NULL; }
void m_adj(struct mbuf *m, int len) { (void)m; (void)len; }
struct mbuf *m_pullup(struct mbuf *m, int len) { (void)len; return m; }
void m_copydata(const struct mbuf *m, int off, int len, caddr_t cp) { memcpy(cp, m->m_data + off, (size_t)len); }

/* ---- the interface ---- */
struct ifnet {
	if_ctx_t ctx;
	int flags;
	int drvflags;
	int capenable;
	int mtu;
	uint8_t lladdr[ETHER_ADDR_LEN];
};

uint64_t if_get_counter_default(if_t ifp, ift_counter c) { (void)ifp; (void)c; return 0; }
int if_getmtu(if_t ifp) { return ifp->mtu; }
int if_getflags(if_t ifp) { return ifp->flags; }
int if_getdrvflags(if_t ifp) { return ifp->drvflags; }
int if_getcapenable(if_t ifp) { return ifp->capenable; }
void if_setcapenablebit(if_t ifp, int set, int clear) { ifp->capenable = (ifp->capenable | set) & ~clear; }
uint8_t *if_getlladdr(if_t ifp) { return ifp->lladdr; }
int if_vlantrunkinuse(if_t ifp) { (void)ifp; return 0; }
void if_setsendqready(if_t ifp) { (void)ifp; }
void if_setsendqlen(if_t ifp, int len) { (void)ifp; (void)len; }
unsigned if_foreach_llmaddr(if_t ifp, iflladdr_cb_t cb, void *arg) { (void)ifp; (void)cb; (void)arg; return 0; }
unsigned if_llmaddr_count(if_t ifp) { (void)ifp; return 0; }

void ether_gen_addr(if_t ifp, struct ether_addr *hwaddr)
{
	(void)ifp;
	uint8_t a[6] = { 0x58, 0x9c, 0xfc, 0x00, 0x00, 0x01 };
	arc4rand(a + 3, 3, 0);
	memcpy(hwaddr->octet, a, 6);
}

void ifmedia_add(struct ifmedia *ifm, int mword, int data, void *aux) { (void)ifm; (void)mword; (void)data; (void)aux; }
void ifmedia_set(struct ifmedia *ifm, int mword) { (void)ifm; (void)mword; }

/* ---- iflib ---- */
#define RX_BUF 2048
#define NDESC 256

struct iflib_ctx {
	struct bsdkpi_device dev;
	if_shared_ctx_t sctx;
	struct if_softc_ctx scctx;
	struct ifnet ifnet;
	struct ifmedia media;
	void *softc;
	/* the one transmit queue */
	caddr_t txd;
	uint64_t txd_phys;
	uint8_t *txbuf;
	uint64_t txbuf_phys;
	qidx_t tx_pidx;
	int tx_inuse;
	/* the one receive queue */
	caddr_t rxd;
	uint64_t rxd_phys;
	uint8_t *rxbuf;
	uint64_t rxbuf_phys;
	qidx_t rx_cidx;
	qidx_t rx_pidx;
	int rx_free;
	/* the admin task wants to run */
	int admin;
	int link;
	uint64_t next_timer;
};

static struct iflib_ctx the_ctx;

void *iflib_get_softc(if_ctx_t ctx) { return ctx->softc; }
device_t iflib_get_dev(if_ctx_t ctx) { return &ctx->dev; }
if_t iflib_get_ifp(if_ctx_t ctx) { return &ctx->ifnet; }
struct ifmedia *iflib_get_media(if_ctx_t ctx) { return &ctx->media; }
if_softc_ctx_t iflib_get_softc_ctx(if_ctx_t ctx) { return &ctx->scctx; }
if_shared_ctx_t iflib_get_sctx(if_ctx_t ctx) { return ctx->sctx; }
uint32_t iflib_get_rx_mbuf_sz(if_ctx_t ctx) { (void)ctx; return RX_BUF; }
void iflib_set_mac(if_ctx_t ctx, uint8_t mac[ETHER_ADDR_LEN]) { memcpy(ctx->ifnet.lladdr, mac, ETHER_ADDR_LEN); }
void iflib_admin_intr_deferred(if_ctx_t ctx) { ctx->admin = 1; }
void iflib_request_reset(if_ctx_t ctx) { (void)ctx; }
struct sx *iflib_ctx_lock_get(if_ctx_t ctx) { (void)ctx; static struct sx s; return &s; }

void iflib_link_state_change(if_ctx_t ctx, int state, uint64_t baudrate)
{
	int up = state == LINK_STATE_UP;
	if (up != ctx->link) {
		ctx->link = up;
		bsdkpi_host_link(up, baudrate);
	}
}

int iflib_irq_alloc_generic(if_ctx_t ctx, if_irq_t irq, int rid, iflib_intr_type_t type,
    driver_filter_t *filter, void *filter_arg, int qid, const char *name)
{
	(void)ctx; (void)irq; (void)rid; (void)type; (void)filter; (void)filter_arg; (void)qid; (void)name;
	return 0;
}

void iflib_softirq_alloc_generic(if_ctx_t ctx, if_irq_t irq, iflib_intr_type_t type, void *arg, int qid,
    const char *name)
{
	(void)ctx; (void)irq; (void)type; (void)arg; (void)qid; (void)name;
}

void iflib_irq_free(if_ctx_t ctx, if_irq_t irq) { (void)ctx; (void)irq; }

/* The device methods if_em points at; only the driver's register method
 * is used (bsdkpi_em_attach does what iflib_device_attach would). */
int iflib_device_probe(device_t dev) { (void)dev; return 0; }
int iflib_device_attach(device_t dev) { (void)dev; return 0; }
int iflib_device_detach(device_t dev) { (void)dev; return 0; }
int iflib_device_shutdown(device_t dev) { (void)dev; return 0; }
int iflib_device_suspend(device_t dev) { (void)dev; return 0; }
int iflib_device_resume(device_t dev) { (void)dev; return 0; }

/* A driver's method by name ("ifdi_init"), or NULL. */
static kobjop_t method(const driver_t *drv, const char *name)
{
	for (const kobj_method_t *m = drv->methods; m && m->name; m++)
		if (!strcmp(m->name, name))
			return m->func;
	return NULL;
}

#define IFDI(name, type) ((type)method(ctx->sctx->isc_driver, name))

void IFDI_INTR_DISABLE(if_ctx_t ctx)
{
	void (*f)(if_ctx_t) = IFDI("ifdi_intr_disable", void (*)(if_ctx_t));
	if (f)
		f(ctx);
}

void IFDI_INTR_ENABLE(if_ctx_t ctx)
{
	void (*f)(if_ctx_t) = IFDI("ifdi_intr_enable", void (*)(if_ctx_t));
	if (f)
		f(ctx);
}

static void call_void(if_ctx_t ctx, const char *name)
{
	void (*f)(if_ctx_t) = IFDI(name, void (*)(if_ctx_t));
	if (f)
		f(ctx);
}

static int call_int(if_ctx_t ctx, const char *name)
{
	int (*f)(if_ctx_t) = IFDI(name, int (*)(if_ctx_t));
	return f ? f(ctx) : 0;
}

/* Gives receive descriptors [from, to) their buffers again (iflib's
 * free list refill) and tells the card. */
static void refill(if_ctx_t ctx, int count)
{
	if_txrx_t t = ctx->scctx.isc_txrx;
	int n = ctx->scctx.isc_nrxd[0];
	uint64_t paddrs[32];
	qidx_t idxs[32];
	while (count > 0) {
		int k = count < 32 ? count : 32;
		for (int i = 0; i < k; i++) {
			int j = (ctx->rx_pidx + i) % n;
			idxs[i] = (qidx_t)j;
			paddrs[i] = ctx->rxbuf_phys + (uint64_t)j * RX_BUF;
		}
		struct if_rxd_update iru = {
			.iru_paddrs = paddrs, .iru_idxs = idxs, .iru_pidx = ctx->rx_pidx,
			.iru_qsidx = 0, .iru_count = (uint16_t)k, .iru_buf_size = RX_BUF, .iru_flidx = 0,
		};
		t->ift_rxd_refill(ctx->softc, &iru);
		ctx->rx_pidx = (qidx_t)((ctx->rx_pidx + k) % n);
		count -= k;
	}
	t->ift_rxd_flush(ctx->softc, 0, 0, ctx->rx_pidx);
}

/* Finds and attaches the card: the em driver's register method, then
 * what iflib_device_register does. Returns 0 or an errno. */
int bsdkpi_em_attach(uint16_t vendor, uint16_t device, uint16_t subvendor, uint16_t subdevice,
    uint8_t revid, uint8_t mac[ETHER_ADDR_LEN])
{
	extern driver_t *bsdkpi_driver_em;
	if_ctx_t ctx = &the_ctx;
	memset(ctx, 0, sizeof *ctx);
	ctx->dev.vendor = vendor;
	ctx->dev.device = device;
	ctx->dev.subvendor = subvendor;
	ctx->dev.subdevice = subdevice;
	ctx->dev.revid = revid;
	void *(*reg)(device_t) = (void *(*)(device_t))method(bsdkpi_driver_em, "device_register");
	if (!reg)
		return ENXIO;
	ctx->sctx = reg(&ctx->dev);
	const pci_vendor_info_t *v = ctx->sctx->isc_vendor_info;
	for (; v->pvi_vendor_id; v++)
		if (v->pvi_vendor_id == vendor && v->pvi_device_id == device)
			break;
	if (!v->pvi_vendor_id)
		return ENXIO;
	printf("bsdnet: %s\n", v->pvi_name);
	ctx->softc = malloc(ctx->sctx->isc_driver->size, M_DEVBUF, M_ZERO);
	ctx->dev.softc = ctx->softc;
	ctx->ifnet.ctx = ctx;
	ctx->ifnet.mtu = ETHERMTU;
	ctx->ifnet.flags = IFF_UP | IFF_BROADCAST | IFF_MULTICAST;
	ctx->scctx.isc_ntxd[0] = NDESC;
	ctx->scctx.isc_nrxd[0] = NDESC;
	ctx->scctx.isc_ntxqsets = ctx->scctx.isc_nrxqsets = 1;
	ctx->scctx.isc_ntxqsets_max = ctx->scctx.isc_nrxqsets_max = 1;
	ctx->scctx.isc_intr = IFLIB_INTR_MSI;
	ctx->scctx.isc_media = &ctx->media;
	int err = call_int(ctx, "ifdi_attach_pre");
	if (err) {
		printf("bsdnet: attach_pre failed: %d\n", err);
		return err;
	}
	/* em may change the counts; one buffer per descriptor. */
	ctx->scctx.isc_ntxqsets = ctx->scctx.isc_nrxqsets = 1;
	int ntxd = ctx->scctx.isc_ntxd[0], nrxd = ctx->scctx.isc_nrxd[0];
	ctx->txd = bsdkpi_host_dma(roundup2(ctx->scctx.isc_txqsizes[0], PAGE_SIZE), &ctx->txd_phys);
	ctx->rxd = bsdkpi_host_dma(roundup2(ctx->scctx.isc_rxqsizes[0], PAGE_SIZE), &ctx->rxd_phys);
	ctx->txbuf = bsdkpi_host_dma((size_t)ntxd * RX_BUF, &ctx->txbuf_phys);
	ctx->rxbuf = bsdkpi_host_dma((size_t)nrxd * RX_BUF, &ctx->rxbuf_phys);
	if (!ctx->txd || !ctx->rxd || !ctx->txbuf || !ctx->rxbuf)
		return ENOMEM;
	int (*qalloc)(if_ctx_t, caddr_t *, uint64_t *, int, int);
	qalloc = IFDI("ifdi_tx_queues_alloc", int (*)(if_ctx_t, caddr_t *, uint64_t *, int, int));
	if (!qalloc || (err = qalloc(ctx, &ctx->txd, &ctx->txd_phys, 1, 1)))
		return err ? err : ENXIO;
	qalloc = IFDI("ifdi_rx_queues_alloc", int (*)(if_ctx_t, caddr_t *, uint64_t *, int, int));
	if (!qalloc || (err = qalloc(ctx, &ctx->rxd, &ctx->rxd_phys, 1, 1)))
		return err ? err : ENXIO;
	if ((err = call_int(ctx, "ifdi_attach_post"))) {
		printf("bsdnet: attach_post failed: %d\n", err);
		return err;
	}
	/* "ifconfig up": iflib_init_locked. */
	IFDI_INTR_DISABLE(ctx);
	ctx->ifnet.drvflags = IFF_DRV_RUNNING;
	call_void(ctx, "ifdi_init");
	ctx->rx_pidx = 0;
	refill(ctx, nrxd - 1);
	ctx->rx_cidx = 0;
	IFDI_INTR_ENABLE(ctx);
	call_void(ctx, "ifdi_update_admin_status");
	memcpy(mac, ctx->ifnet.lladdr, ETHER_ADDR_LEN);
	return 0;
}

/* Received frames to the host, their descriptors refilled. */
static void receive(if_ctx_t ctx)
{
	if_txrx_t t = ctx->scctx.isc_txrx;
	int n = ctx->scctx.isc_nrxd[0];
	static uint8_t frame[RX_BUF * 8];
	for (int budget = 0; budget < 64; budget++) {
		if (t->ift_rxd_available(ctx->softc, 0, ctx->rx_cidx, 1) <= 0)
			break;
		struct if_rxd_frag frags[8];
		struct if_rxd_info ri;
		memset(&ri, 0, sizeof ri);
		ri.iri_qsidx = 0;
		ri.iri_cidx = ctx->rx_cidx;
		ri.iri_frags = frags;
		ri.iri_ifp = &ctx->ifnet;
		if (t->ift_rxd_pkt_get(ctx->softc, &ri) != 0) {
			ctx->rx_cidx = (qidx_t)((ctx->rx_cidx + (ri.iri_nfrags ? ri.iri_nfrags : 1)) % n);
			refill(ctx, ri.iri_nfrags ? ri.iri_nfrags : 1);
			continue;
		}
		size_t len = 0;
		for (int i = 0; i < ri.iri_nfrags && i < 8; i++) {
			size_t l = frags[i].irf_len;
			if (len + l > sizeof frame)
				break;
			memcpy(frame + len, ctx->rxbuf + (size_t)frags[i].irf_idx * RX_BUF, l);
			len += l;
		}
		ctx->rx_cidx = (qidx_t)((ctx->rx_cidx + ri.iri_nfrags) % n);
		refill(ctx, ri.iri_nfrags);
		if (len)
			bsdkpi_host_input(frame, len);
	}
}

/* Descriptors the card is done with. */
static void reclaim(if_ctx_t ctx)
{
	int done = ctx->scctx.isc_txrx->ift_txd_credits_update(ctx->softc, 0, true);
	if (done > 0)
		ctx->tx_inuse -= done;
	if (ctx->tx_inuse < 0)
		ctx->tx_inuse = 0;
}

/* Sends a frame; 0, or EAGAIN when the ring is full. */
int bsdkpi_em_transmit(const void *data, size_t len)
{
	if_ctx_t ctx = &the_ctx;
	int n = ctx->scctx.isc_ntxd[0];
	if (len > RX_BUF)
		return EINVAL;
	if (ctx->tx_inuse > n - 8)
		reclaim(ctx);
	if (ctx->tx_inuse > n - 8)
		return EAGAIN;
	memcpy(ctx->txbuf + (size_t)ctx->tx_pidx * RX_BUF, data, len);
	bus_dma_segment_t seg = { ctx->txbuf_phys + (uint64_t)ctx->tx_pidx * RX_BUF, len };
	struct if_pkt_info pi;
	memset(&pi, 0, sizeof pi);
	pi.ipi_segs = &seg;
	pi.ipi_nsegs = 1;
	pi.ipi_len = (uint32_t)len;
	pi.ipi_qsidx = 0;
	pi.ipi_pidx = ctx->tx_pidx;
	pi.ipi_flags = IPI_TX_INTR;
	int err = ctx->scctx.isc_txrx->ift_txd_encap(ctx->softc, &pi);
	if (err)
		return err;
	ctx->tx_inuse += pi.ipi_ndescs ? pi.ipi_ndescs : 1;
	ctx->tx_pidx = pi.ipi_new_pidx;
	ctx->scctx.isc_txrx->ift_txd_flush(ctx->softc, 0, ctx->tx_pidx);
	return 0;
}

/* An interrupt (or a poll): the legacy filter, then what its task does. */
void bsdkpi_em_interrupt(void)
{
	if_ctx_t ctx = &the_ctx;
	int r = ctx->scctx.isc_txrx->ift_legacy_intr(ctx->softc);
	receive(ctx);
	reclaim(ctx);
	if (r != FILTER_STRAY)
		IFDI_INTR_ENABLE(ctx);
	if (ctx->admin) {
		ctx->admin = 0;
		call_void(ctx, "ifdi_update_admin_status");
	}
}

/* Every few milliseconds: callouts, and twice a second iflib's timer. */
void bsdkpi_em_tick(void)
{
	if_ctx_t ctx = &the_ctx;
	run_callouts();
	uint64_t now = bsdkpi_host_now_us();
	if (now >= ctx->next_timer) {
		ctx->next_timer = now + 500000;
		void (*timer)(if_ctx_t, uint16_t) = IFDI("ifdi_timer", void (*)(if_ctx_t, uint16_t));
		if (timer)
			timer(ctx, 0);
		call_void(ctx, "ifdi_update_admin_status");
	}
}
