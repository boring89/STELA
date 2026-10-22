/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: MIT
 *
 * bsdkpi (P14 K15, docs/bsdkpi.md): the functions FreeBSD's iwm calls
 * that the shim does not have yet, from net80211 and the kernel. Wi-Fi
 * is compile-only: bsdnet only runs iwm's probe, so none of these is
 * reached; each panics with its name if one is. The list is what
 * `nm -u` leaves undefined after linking iwm with bsdkpi.c and wifi.c;
 * a function that gets a real body moves to wifi.c.
 *
 * No headers on purpose: the stubs do not match the prototypes.
 */
void panic(const char *fmt, ...) __attribute__((noreturn));

#define STUB(name) \
	void name(void); \
	void name(void) { panic("bsdkpi: %s is not in the shim (Wi-Fi is compile-only)", #name); }

STUB(_ieee80211_find_rxnode)
STUB(_ieee80211_free_node)
STUB(_ieee80211_ref_node)
STUB(bus_dma_tag_create)
STUB(bus_dma_tag_destroy)
STUB(bus_dmamap_create)
STUB(bus_dmamap_destroy)
STUB(bus_dmamap_load)
STUB(bus_dmamap_load_mbuf_sg)
STUB(bus_dmamap_unload)
STUB(bus_dmamem_alloc)
STUB(bus_dmamem_free)
STUB(bus_get_dma_tag)
STUB(bus_setup_intr)
STUB(bus_teardown_intr)
STUB(config_intrhook_disestablish)
STUB(config_intrhook_establish)
STUB(firmware_get)
STUB(firmware_put)
STUB(ieee80211_add_channel)
STUB(ieee80211_add_rates)
STUB(ieee80211_add_rx_params)
STUB(ieee80211_add_ssid)
STUB(ieee80211_add_xrates)
STUB(ieee80211_beacon_miss)
STUB(ieee80211_cancel_scan)
STUB(ieee80211_chan2ieee)
STUB(ieee80211_crypto_encap)
STUB(ieee80211_ieee2mhz)
STUB(ieee80211_ifattach)
STUB(ieee80211_ifdetach)
STUB(ieee80211_input_mimo)
STUB(ieee80211_input_mimo_all)
STUB(ieee80211_media_change)
STUB(ieee80211_media_status)
STUB(ieee80211_mhz2ieee)
STUB(ieee80211_notify_radio)
STUB(ieee80211_radiotap_attach)
STUB(ieee80211_radiotap_tx)
STUB(ieee80211_ratectl_init)
STUB(ieee80211_restart_all)
STUB(ieee80211_resume_all)
STUB(ieee80211_scan_done)
STUB(ieee80211_start_all)
STUB(ieee80211_suspend_all)
STUB(ieee80211_tx_complete)
STUB(ieee80211_vap_attach)
STUB(ieee80211_vap_detach)
STUB(ieee80211_vap_setup)
STUB(ieee80211_wme_ic_getparams)
STUB(if_inc_counter)
STUB(m_collapse)
STUB(m_copym)
STUB(m_getjcl)
STUB(pci_alloc_msi)
STUB(pci_release_msi)
STUB(taskqueue_cancel)
STUB(taskqueue_create)
STUB(taskqueue_drain_all)
STUB(taskqueue_start_threads)
STUB(taskqueue_thread_enqueue)
