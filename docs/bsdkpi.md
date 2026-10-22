# FreeBSD 驅動相容層（bsdkpi）

> P14 D（K13–K16）由自動排程一次做完，**使用者尚未審核**。

STELA 自己寫的驅動只涵蓋 QEMU 與 P520 的幾張卡。FreeBSD 有大量成熟、授權相容（BSD）的驅動，Wi-Fi 與 GPU 之外的網卡、儲存卡大多是「讀寫暫存器、管 DMA ring、處理中斷」這種形狀。bsdkpi 的做法：**FreeBSD 的驅動原始碼一個字不改**，在它下面墊一層「看起來像 FreeBSD kernel」的函式（KPI，Kernel Programming Interface），整個東西編成一個 STELA 的**使用者程式**，經 P13 的使用者空間驅動介面（`docs/devices.md`）碰硬體。

第一個例子是 `bsdnet`：FreeBSD 14.3 的 `if_em`（Intel e1000／e1000e 系列），在 QEMU 的 82574L（MSI）與 82540EM（輪詢）上 DHCP、ping、fetch 都通。K15 又把 FreeBSD 的 Intel Wi-Fi 驅動 `iwm` 與 `net80211` 的標頭編進來，但**只編得過、只跑 probe**（見 §6）。

程式碼：

```
user/bsdkpi/
  freebsd/            FreeBSD 14.3（release/14.3.0）的原檔，不改；README.md 記來源路徑、tag、授權、SHA256SUMS.*
    dev/e1000/        if_em 與 Intel 共用碼（43 檔）
    dev/iwm/          Intel Wi-Fi 7000–9000（38 檔，K15，compile-only）
    net80211/         802.11 層的標頭（33 個，K15；沒有 .c）
    net/ sys/ dev/pci/  iflib.h、if_media.h、queue.h、pcireg.h
  shim/               墊片（MIT，STELA 自己的）
    include/bsdkpi.h  FreeBSD 的型別、巨集、函式宣告；include/sys/*.h 等 FreeBSD 標頭名都只是轉到它
    include/bsdkpi_wifi.h  Wi-Fi 才需要的（-DBSDKPI_WIFI 才看得到）
    bsdkpi.c          libc 子集、malloc、callout、PCI、bus_space、資源、迷你 iflib、bsdkpi_em_*、bsdkpi_probe
    wifi.c            mbufq、ether_sprintf、msleep／wakeup、bsdkpi_iwm_probe
    wifi_stubs.c      iwm 呼叫但墊片還沒有的 61 個函式：每個都 panic（K15）
  build.rs            gcc 編全部 C → libbsdkpi_c.a（FreeBSD 的檔案 -w，墊片 -Werror）
  src/ffi.rs          C 與 Rust 的邊界：bsdkpi_host_*（墊片呼叫）與 bsdkpi_em_*／iwm 的包裝；bsdnet 唯一的 unsafe
  src/main.rs         找卡、attach、事件迴圈、--probe
user/init             開機選項 usernet=bsd 時啟動 Initrd:/bsdnet（只給它 DEVICES）
```

## 1. 比喻：同聲傳譯

FreeBSD 的驅動是一位只會說「FreeBSD 話」的技師：他要 `bus_space_read_4` 讀暫存器、`malloc(M_DEVBUF)` 要記憶體、`callout_reset` 定鬧鐘、把收到的封包交給 `iflib`。墊片是坐在旁邊的口譯：技師每說一句 FreeBSD 話，口譯就翻成 STELA 的做法（讀 `DeviceMapBar` 映射進來的記憶體、Rust 的 heap、事件迴圈裡的計時器、`NetDeliver` 給 kernel）。技師不用改口，口譯只要學會**這位技師實際會說的那些話**——不是整本 FreeBSD kernel 字典。

```
  FreeBSD if_em（原樣）
     │  bus_space_*、malloc、callout、pci_read_config、iflib 的 ifdi_* 方法 …
     ▼
  shim/bsdkpi.c（C，MIT）      ← 迷你 iflib：一組 TX／RX ring，一個描述子一個 2 KiB buffer
     │  bsdkpi_host_*（C ABI）
     ▼
  src/ffi.rs（Rust，unsafe 只在這）
     │  libstela::device::{Device, Bar, NetTap}
     ▼
  kernel：DeviceOpen／MapBar／Dma／Irq、NetAttach／Deliver／Take（docs/devices.md）
```

## 2. 墊片的範圍

只做 `if_em` 用到的；沒用到的不做（不是 FreeBSD 的通用相容）。

| FreeBSD 的東西 | 墊片怎麼做 |
|---|---|
| `printf`、`snprintf`、`mem*`、`str*` | 自己的小 libc（`%d %u %x %s %c %p %l… %z…`），印到 serial／console |
| `malloc(9)`／`free` | Rust 的 global allocator（`bsdkpi_host_alloc`，16 bytes 對齊）；free 要大小，所以前面藏一個 header |
| `mtx`、`sx`、`taskqueue`、`grouptask` | 全部是空的或立刻執行：**整個驅動在一個執行緒**，沒有競爭（§3） |
| `callout(9)` | 一張清單；事件迴圈每次醒來跑到期的（`bsdkpi_em_tick`） |
| `DELAY`、`pause` | 1 ms 以上睡、以下忙等 |
| `pci_read_config`／`pci_write_config`、`pci_find_cap` | `DeviceConfig`（kernel 不讓寫 BAR） |
| `bus_alloc_resource_any(SYS_RES_MEMORY)`、`bus_space_*` | `DeviceMapBar`；`bus_space_handle_t` 就是映射的位址，讀寫是 volatile |
| `SYS_RES_IOPORT` | **假的**：一塊 16 bytes 的記憶體。Device 介面沒有 port I/O；82540 的 attach 要一個 I/O BAR 但只在特定的 PHY 重設才用（QEMU 用不到） |
| `SYS_RES_IRQ`、MSI | `DeviceIrq` → 主 port 收 `DEVICE_IRQ`；沒有 MSI 的卡每 5 ms 輪詢 |
| DMA（iflib 配好給驅動） | `DeviceDma`：描述子 ring 與 buffer 各一塊實體連續記憶體 |
| `sysctl`、`SYSCTL_ADD_*`、`TUNABLE_*`、`sbuf` | 什麼都不做（回傳空的 oid） |
| `ifnet`、`if_get*`／`if_set*`、`ifmedia` | 一個 `struct ifnet`，只有 MTU、旗標、MAC；媒體表不記 |
| `DRIVER_MODULE(name, …)` | 變成全域指標 `bsdkpi_driver_<name>`；`method(drv, "ifdi_init")` 依名字在 kobj 方法表裡找函式 |
| iflib（`iflib_device_register` 那一大套） | **迷你 iflib**：一組 TX、一組 RX queue；`ifdi_attach_pre` → 配 ring → `ifdi_{tx,rx}_queues_alloc` → `ifdi_attach_post` → `ifdi_init`；收送經驅動的 `if_txrx` 函式表（`ift_rxd_available`、`ift_rxd_pkt_get`、`ift_rxd_refill`、`ift_txd_encap`、`ift_txd_flush`、`ift_txd_credits_update`、`ift_legacy_intr`） |
| mbuf | 結構有（標頭要），但 em 經 iflib 時看不到 mbuf；封包是平的 2 KiB buffer，複製一次 |

迷你 iflib 的資料流：

```
 收：DEVICE_IRQ 或 5 ms ─► ift_legacy_intr ─► ift_rxd_available／pkt_get ─► 複製出 buffer ─► bsdkpi_host_input ─► NetDeliver
                                                                         └► ift_rxd_refill（同一個 buffer 還給卡）
 送：NET_TX_READY ─► NetTake ─► bsdkpi_em_transmit：複製進 TX buffer ─► ift_txd_encap ─► ift_txd_flush
                               ring 剩不到 8 個時先 ift_txd_credits_update 回收，還是滿就留到下一輪
 計時：每輪 run_callouts；每 0.5 s ifdi_timer + ifdi_update_admin_status（連線狀態 → bsdkpi_host_link）
```

數字：TX／RX 各 256 個描述子、每個 2 KiB buffer（共 1 MiB DMA）；一次最多收 64 個 frame 就回到事件迴圈。

## 3. 執行緒與鎖：為什麼全部可以是空的

FreeBSD 的驅動到處拿鎖，因為它的中斷 handler、taskqueue、ioctl 跑在不同的執行緒、甚至不同的 CPU。bsdnet **只有一個執行緒**：

```
loop {
    port_receive(MAIN_PORT, timeout = MSI ? 20 ms : 5 ms)   // DEVICE_IRQ、NET_TX_READY 或逾時
    bsdkpi_em_interrupt()   // 中斷原因、收、回收、重新打開中斷、需要時更新連線
    bsdkpi_em_tick()        // callout、iflib 的計時器
    while NetTake 有 frame { bsdkpi_em_transmit }
}
```

同一時間只有一段 FreeBSD 程式在跑，所以 `mtx_lock` 等於什麼都不做是正確的；`taskqueue_enqueue` 直接呼叫 task 也正確（不會在 handler 裡遞迴，因為 em 的 task 不會再排自己）。代價：不能用多核、一個慢的操作（例如 PHY 重設的 `msec_delay`）會讓收送停那麼久。

Rust 端的 `ffi::HOST`（`SpinMutex<Option<Host>>`）放 Device、映射與 NetTap；C 呼叫 host 函式時鎖一下，從來不會在鎖住時再進 C，所以不會自己鎖死。

## 4. 信任模型

bsdnet 拿著 `DEVICES` 能力（`docs/devices.md` §1）：**沒有 IOMMU，能叫卡做 DMA 的程式等於持有整台機器**。這和 kernel 裡的驅動一樣信任，只是：

- 驅動的 bug（越界寫、空指標）只會讓 bsdnet 這個行程死掉（page fault → 行程結束 → kernel 自動 `DeviceReset`、`NetTap` 關掉、網路變成沒有網卡），不會讓 kernel panic。
- 但卡的 DMA 可以寫任何實體記憶體：一個寫錯 DMA 位址的驅動還是可能毀掉整台機器。墊片給驅動的位址只來自 `DeviceDma`，驅動自己算錯就沒辦法擋。
- 只有 init 給它 `DEVICES`，而且只在開機選項 `usernet=bsd` 時才啟動它；shell 啟動的 `bsdnet --probe` 沒有 DEVICES（probe 不碰卡）。

授權：FreeBSD 的檔案是 BSD-2-Clause／BSD-3-Clause／ISC（`iwm` 的 Intel 部分是「BSD 或 GPLv2」雙授權，我們用 BSD 那一邊），墊片 MIT，Rust 程式 GPL-3.0-or-later；整個 bsdnet 以 GPL-3.0-or-later 發布沒有衝突。`freebsd/README.md` 與 `third_party/README.md` 有清單。

## 5. 怎麼加一個驅動

以「另一張 iflib 的網卡」（例如 `igc`、`ixgbe`）為例：

1. **取檔案**：從 FreeBSD 的 GitHub 鏡像 `freebsd/freebsd-src` 的某個 release tag 拿 `sys/dev/<驅動>/` 整個目錄放進 `freebsd/dev/<驅動>/`，**不改內容**；在 `freebsd/README.md` 的表加一列（路徑、tag、授權），`(cd freebsd/dev/<驅動> && sha256sum *) > freebsd/SHA256SUMS.<驅動>`。
2. **編**：`build.rs` 的來源清單加那個資料夾（各組可以有自己的 `-I`／`-D`）。編不過時看第一個錯誤：
   - `fatal error: sys/xxx.h`：在 `shim/include/` 加一個只有 `#include <bsdkpi.h>` 的同名檔。
   - 缺型別、巨集：加進 `bsdkpi.h`（或只給那個驅動的 `bsdkpi_<組>.h`，用 `-D` 打開）。FreeBSD 的真標頭能直接用的（只有型別與常數、沒有 kernel 內部的）就整個拿來放 `freebsd/`，例如 `net/iflib.h`、`if_media.h`。
   - FreeBSD 的檔案一律 `-w`；墊片自己的檔 `-Wall -Wextra -Werror`。
3. **連結**：`nm -u` 列出驅動要、墊片還沒有的函式。真的會被呼叫到的寫進墊片；暫時不會的可以像 `wifi_stubs.c` 一樣 panic 並印名字（執行時一碰到就知道缺什麼）。
4. **接上**：在 `bsdkpi.c` 寫 `bsdkpi_<驅動>_attach`（用 `method(bsdkpi_driver_<名字>, "...")` 找方法）；`src/ffi.rs` 加 `extern "C"` 宣告與安全的包裝；`main.rs` 的 `find` 認得那張卡。
5. **測試**：QEMU 有那張卡就加進 `make test-net-user` 的 bsd 那幾輪；沒有就只能靠 probe 測（像 iwm 的 `bsdnet --probe`）並在文件標「實機未驗證」。

非 iflib 的網卡（直接用 `ifnet` 的 `if_transmit`、`if_input`）要再補一套 `ifnet` 與 mbuf 的函式（真的 mbuf 鏈、`m_getcl`、`bus_dmamap_load_mbuf_sg`）——K15 的 `bsdkpi_wifi.h` 已有宣告，但都還是 stub。

## 6. Wi-Fi：K15 做到哪、筆電上還差什麼

K15 的範圍是「**編得過並連結進 bsdnet**，執行時沒有裝置就不載入」：

- `sys/dev/iwm/` 的 18 個 `.c` 全部原樣編進 `libbsdkpi_c.a`，經 `wifi.c` 的 `bsdkpi_iwm_probe` 被參考到，所以 iwm 的程式碼真的連結進 `Initrd:/bsdnet`（約 65 KiB 的程式碼與資料）。
- `net80211` 只有標頭（33 個），沒有任何 `.c`：iwm 呼叫的 net80211 函式（`ieee80211_ifattach`、`ieee80211_vap_setup`、`ieee80211_input_mimo`……）與 kernel 函式（`bus_dma_tag_create`、`firmware_get`、`taskqueue_create`……）共 61 個都在 `wifi_stubs.c`，**一被呼叫就 panic**。
- 只有 iwm 的 `device_probe`（`iwm_probe`：查它的 PCI id 表、設描述）真的會跑：`bsdnet --probe 8086:24fd` 印 `iwm: Intel(R) Dual Band Wireless AC 8265`（`make test-net-user` 檢查這行）；bsdnet 找卡時遇到 iwm 認得的 Intel Wi-Fi 就印名字、跳過，不 attach。
- `wpa_supplicant` 2.11 是 ports repo 的一個 port（`~/stela-ports/ports/wpa_supplicant`）：靜態連結 musl、內建的 crypto／TLS（沒有 OpenSSL），只有 `none` 與 `wired` 兩種 driver；`test.sh` 檢查 `wpa_supplicant -v` 與 `wpa_passphrase`（PBKDF2 算出的 PSK 與 host 算的一致）。它不能連任何網路。

要在筆電（例如 Intel 8265）上真的連上 Wi-Fi，還差：

1. **韌體載入**：iwm attach 時 `firmware_get("iwm8265fw")` 要拿到 Intel 的韌體映像（`iwm-8265-36.ucode` 之類，Intel 的授權：可散布、不可修改，**不是自由軟體**）。要決定：韌體放哪（使用者自己放進 `Data:/` 的某個資料夾？System 卷宗不放）、`firmware_get` 怎麼讀（bsdnet 需要那個資料夾的 handle）。這是授權與發布方式的決定，需要使用者決定。
2. **net80211 本體**：把 `sys/net80211/` 需要的 `.c`（`ieee80211.c`、`_input`、`_output`、`_node`、`_proto`、`_scan`、`_scan_sta`、`_sta`、`_crypto`、`_crypto_ccmp`、`_ht`、`_vht`、`_regdomain`、`_freebsd`、`_ratectl`、`_amrr` 等約 20 個，數萬行）編進來，並補它要的 kernel 函式：真的 mbuf 鏈、`ifnet` 的完整 `if_transmit`／`if_input`、`taskqueue` 執行緒（可以在事件迴圈裡跑）、`bus_dma` 的 tag／map、`callout` 已有。802.11 的封包要轉成乙太網路 frame 才能經 `NetTap` 給 kernel（net80211 本來就做這件事：`ieee80211_input` 解封裝後呼叫 `if_input`）。
3. **net80211 的事件與控制**：掃描結果、連上／斷線（`ieee80211_notify_*`）要讓使用者看得到；STELA 現在沒有網路設定的 UI 或 API。
4. **`wpa_supplicant` 的 driver 介面**：FreeBSD 上 wpa_supplicant 用 `driver_bsd.c`：對 `wlan0` 做 `SIOCS80211`／`SIOCG80211` ioctl（設 SSID、金鑰、掃描）、從 routing socket 收事件、經 BPF（`l2_packet_freebsd.c`）收送 EAPOL 封包。STELA 上要選一種做法：(a) bsdnet 開一個 IPC port，寫一個 STELA 自己的 wpa_supplicant driver（`driver_stela.c`）把那些 ioctl 變成訊息；(b) 在 Linux 相容層做 FreeBSD 的 ioctl（不自然）；(c) 不用 wpa_supplicant，用 net80211 的 `ieee80211_crypto` + 自己寫 WPA2-PSK 的四次交握。建議 (a)，但這是架構決定，留給使用者。
5. **實機驗證**：QEMU 沒有 Wi-Fi 卡，以上全部只能在實機測試。

## 7. 限制（已知問題）

- 只有 `if_em`（e1000／e1000e 系列）真的會跑；Wi-Fi 是 compile-only（§6）。
- 一個執行緒、一組 queue；沒有 TSO、checksum offload（驅動回報但墊片不用）、VLAN、多播過濾表（`ifdi_multi_set` 不呼叫）、jumbo frame。
- 封包收送各複製一次；RX buffer 2 KiB。
- 82540 的 I/O BAR 是假的：需要 port I/O 的路徑（某些 PHY 的重設）會讀到 0。
- 沒有 detach、suspend／resume；bsdnet 結束時 kernel 重設卡（`DeviceReset`）。
- `sysctl`、統計計數、LED 都不做。
- 實機未驗證（P520 的 I219 用 kernel 或 e1000u 的驅動）。
