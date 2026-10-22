/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: MIT
 *
 * bsdkpi (P14 K15, docs/bsdkpi.md): the Wi-Fi half of the shim. FreeBSD's iwm (sys/dev/iwm, unchanged) is compiled and linked
 * into bsdnet, but only its probe runs: attaching needs net80211, the
 * firmware and a supplicant, which are not here (wifi_stubs.c panics
 * if the driver reaches them). The few KPI functions that are simple
 * are real.
 */
#include <bsdkpi.h>

extern driver_t *bsdkpi_driver_iwm;

struct malloc_type M_80211_NODE[1] = { { "80211node" } };
struct malloc_type M_80211_VAP[1] = { { "80211vap" } };
const uint8_t ieee80211broadcastaddr[6] = { 0xff, 0xff, 0xff, 0xff, 0xff, 0xff };
int ifqmaxlen = 50;

/* Whether iwm knows the PCI id: 1 with its description in desc (at most
 * n bytes, NUL-terminated), else 0. */
int bsdkpi_iwm_probe(uint16_t vendor, uint16_t device, char *desc, size_t n)
{
	const char *d = NULL;
	if (bsdkpi_probe(bsdkpi_driver_iwm, vendor, device, &d) > 0 || !d)
		return 0;
	snprintf(desc, n, "%s", d);
	return 1;
}

void mbufq_init(struct mbufq *mq, int maxlen)
{
	mq->mq_head = mq->mq_tail = NULL;
	mq->mq_len = 0;
	mq->mq_maxlen = maxlen;
}

int mbufq_enqueue(struct mbufq *mq, struct mbuf *m)
{
	if (mbufq_full(mq))
		return ENOBUFS;
	m->m_nextpkt = NULL;
	if (mq->mq_tail)
		mq->mq_tail->m_nextpkt = m;
	else
		mq->mq_head = m;
	mq->mq_tail = m;
	mq->mq_len++;
	return 0;
}

struct mbuf *mbufq_dequeue(struct mbufq *mq)
{
	struct mbuf *m = mq->mq_head;
	if (m) {
		mq->mq_head = m->m_nextpkt;
		if (!mq->mq_head)
			mq->mq_tail = NULL;
		m->m_nextpkt = NULL;
		mq->mq_len--;
	}
	return m;
}

void mbufq_drain(struct mbufq *mq)
{
	struct mbuf *m;
	while ((m = mbufq_dequeue(mq)))
		m_freem(m);
}

char *ether_sprintf(const u_char *ap)
{
	static char buf[18];
	snprintf(buf, sizeof buf, "%02x:%02x:%02x:%02x:%02x:%02x", ap[0], ap[1], ap[2], ap[3], ap[4], ap[5]);
	return buf;
}

/* One thread: nothing else can wake a sleeper, so a sleep times out. */
int msleep(void *chan, struct mtx *m, int pri, const char *wmesg, int timo)
{
	(void)chan; (void)m; (void)pri; (void)wmesg;
	if (timo > 0)
		pause("msleep", timo);
	return EWOULDBLOCK;
}

void wakeup(void *chan) { (void)chan; }
void wakeup_one(void *chan) { (void)chan; }
