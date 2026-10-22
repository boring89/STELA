# 網路：網卡驅動、協定堆疊、socket

> P10 由自動排程一次做完，使用者已於 2026-10-21 審核。

## 1. 比喻：一層一層的信封

網路封包像套了好幾層的信封。最外層寫「實體地址」（MAC，網卡的出廠號碼），裡面一層寫「網際網路地址」（IP），再裡面是「哪個程式」（port）和內容：

```
 Ethernet   [目的 MAC | 來源 MAC | 類型]          ← 同一條線上的網卡之間
   ARP      「10.0.2.2 的 MAC 是誰？」              ← 用 IP 查 MAC
   IPv4     [版本/長度 | 協定 | 來源 IP | 目的 IP | 檢查碼 | 內容]
     ICMP   ping（echo 請求／回覆）
     UDP    [來源 port | 目的 port | 長度 | 檢查碼 | 資料]
       DHCP 向路由器要一個 IP；DNS 把名字換成 IP
     TCP    可靠的位元組流：序號、確認、視窗、重傳
```

QEMU 的 `-netdev user`（slirp）模擬一個小型家用網路：我們（DHCP 配給）10.0.2.15、閘道（也是 host 本身）10.0.2.2、DNS 10.0.2.3（轉送給 host 的解析器）。

## 2. 分層與檔案

```
 使用者程式：ifconfig、ping、fetch（user/）、Linux 程式（BSD socket）
      │ libstela::net（Socket、resolve、http_get）     │ kernel/src/linux/socket.rs
      ▼ syscall 39–46                                 ▼
 kernel/src/net.rs：Socket 物件、net 執行緒、等待
      │
 net/（純邏輯，host 測試）：Stack ─ wire（封包格式）、dhcp、tcp（狀態機）
      │ 一個個 Ethernet frame
 drivers/src/net/：e1000.rs（e1000、e1000e/I219）、virtio.rs；NetDevice trait
      │ DMA 環、MMIO、MSI
 frame：MmioRegion（8/16/32 位元）、DmaBuffer、irq
```

一次只用**第一張**網卡（`drivers::net::probe` 依 PCI 順序找到的第一張能用的）。開機選項 `nonet` 不啟動網路。

## 3. 驅動（`drivers/src/net/`）

`NetDevice` trait：`name`、`mac`、`link_up`、`send(frame)`、`receive(buf) -> Option<len>`、`interrupts()`。中斷處理程式只喚醒 net 執行緒（`set_waiter`）；封包永遠在執行緒裡取，協定程式不在中斷內執行。

### 3.1 Intel e1000 / e1000e（`e1000.rs`）

資料來源：Intel 8254x Software Developer's Manual（暫存器、描述子）、82574 與 I219 datasheet／specification update、FreeBSD `em_flush_desc_rings`、68k_HAL 的 `drivers/e1000e`。

| PCI id | 家族 | 備註 |
|---|---|---|
| 8086:100E（QEMU `e1000`）、100F、1004、107C | `E1000` | 82540EM 沒有 MSI → **輪詢**（net 執行緒每 5 ms 看一次；P11：只在 link up 而且有 socket 開著時，否則 100 ms） |
| 8086:10D3（QEMU `e1000e`，q35 的預設網卡） | `E1000e { pch: false }` | MSI |
| 8086:15B7（P520 的 I219-LM）、15B8、15B9、156F、1570、15D7、15D8、15E3、153A/B、155A、1559、15A0–15A3、15D6、15BB–15BE | `E1000e { pch: true }` | MSI；I219 的 ring flush 與 errata |

```
 一頁 DMA：32 個接收描述子（16 bytes）+ 8 個傳送描述子
 DMA 緩衝：32 + 8 個 2048 bytes
 接收描述子：address(8) length(2) checksum(2) status(1) errors(1) special(2)
 傳送描述子：address(8) length(2) cso(1) command(1) status(1) css(1) special(2)
```

初始化（8254x SDM §14.5）：bus master、BAR0 → IMC 全關、RCTL=0、TCTL=PSP → CTRL.RST（MAC reset，不碰 PHY）→ CTRL.SLU|ASDE（自動協商）→ RAL0/RAH0 的 MAC（RAH.AV 要是 1）→ MTA 清 0 → 接收環（RDT = 31）、傳送環 → e1000e 設 TXDCTL → TIPG（IPGT 10／8、IPGR1 8、IPGR2 6）→ TCTL（EN、PSP、CT 15、COLD 64、RTLC）→ RCTL（EN、BAM 廣播、SECRC 去掉 CRC、2048 bytes）→ MSI（有的話 IMS = LSC|RXDMT0|RXO|RXT0）。I219：reset 之前先用我們的傳送環推一個假描述子（PCI 0xE4 的 FLUSH_DESC_REQUIRED），再短暫開接收；之後 IOSFPC、TARC0 的 errata。

接收：`rx_next` 的描述子 status 有 DD 就取出（EOP 且沒有錯誤才交出去），清空後寫 RDT 還給網卡。傳送：`tx_next` 的描述子沒用過或已 DD 才能用（否則等 100 ms → `Busy`），複製 frame、寫描述子（EOP|IFCS|RS）、TDT + 1。

### 3.2 virtio-net（`virtio.rs`）

VIRTIO 1.1 的 **modern** 介面（§4.1.4 PCI capabilities、§2.6 split virtqueue、§5.1 網路裝置）；PCI 1AF4:1041 或 transitional 1AF4:1000。

```
 vendor capability（id 0x09）：cfg_type 1 common、2 notify（+ notify_off_multiplier）、3 ISR、4 device
 common config：feature select/值 0x00–0x0C、msix_config 0x10、device_status 0x14、
   queue_select 0x16、queue_size 0x18、queue_msix_vector 0x1A、queue_enable 0x1C、
   queue_notify_off 0x1E、queue_desc 0x20、queue_driver 0x28、queue_device 0x30
 每個佇列一頁 DMA：32 個描述子（0）、available ring（512）、used ring（2048）
 佇列 0 收、佇列 1 送；每個緩衝前 12 bytes 的 virtio_net_hdr（全 0：沒有 offload）
```

每個欄位用自己的寬度存取（§4.1.3.1），所以 `frame::mm::mmio::MmioRegion` 加了 `read_u8`、`read_u16`、`write_u16`。初始化：reset（status 0）→ ACKNOWLEDGE|DRIVER → 協商（一定要 `VIRTIO_F_VERSION_1`，另外接受 `MAC`、`STATUS`）→ FEATURES_OK（讀回確認）→ 讀 MAC → 兩個佇列（大小 32、三個位址、MSI-X vector 0）→ DRIVER_OK → 32 個接收緩衝全部放進 available ring、notify。MSI-X：`enable_msi` 設好 entry 0，兩個佇列都指向它（`msix_config` = 無）。

### 3.3 MSI 與輪詢

| 網卡 | 中斷 | net 執行緒的睡眠 |
|---|---|---|
| e1000（82540EM） | 沒有 MSI | 5 ms（P11：有 socket 而且 link up 時；否則 100 ms） |
| e1000e、I219 | MSI | 100 ms（計時器的步長），中斷提早叫醒 |
| virtio-net | MSI-X entry 0 | 同上 |

## 4. 協定堆疊（`net/`，純邏輯）

沒有 I/O、沒有時鐘：`Stack::receive(frame, now_ms)`、`Stack::poll(now_ms)`、`Stack::take_output()`。整個堆疊在 host 上用「兩個堆疊接在一條模擬的線上」測試。

| 模組 | 內容 |
|---|---|
| `wire` | Internet checksum（RFC 1071）、pseudo header、Ethernet（補到 60 bytes）、ARP（RFC 826）、IPv4（RFC 791：不接受分片、DF、TTL 64）、ICMP echo（RFC 792）、UDP（RFC 768）、TCP（RFC 793，含 MSS 選項） |
| `dhcp` | DISCOVER／REQUEST 的組裝、OFFER／ACK／NAK 的解析（RFC 2131、2132：53 類型、50 要求的 IP、54 伺服器、55 參數表、1 遮罩、3 路由器、6 DNS、51 租期） |
| `tcp` | 一條連線的 TCB 與狀態機（下面 §4.2） |
| `stack` | 路由、ARP 快取與等待佇列、DHCP client、socket 表 |

### 4.1 Stack

- **路由**：同一個子網路直接送，否則送閘道；廣播（255.255.255.255 或子網路廣播）用廣播 MAC。
- **ARP**：快取 64 項、5 分鐘；查不到就把 IP 封包放進等待佇列並送 ARP 請求，每秒重問、3 次後放棄（丟掉封包）。收到回覆（或別人問我們）就學起來，把等待的封包送出去。
- **DHCP**：DISCOVER → OFFER → REQUEST → ACK，每 2 秒重送；NAK 就重來。沒有續租（租期很長，重新開機才再要）。
- **socket**：最多 256 個；UDP（佇列 32 個 datagram）、ICMP echo（依 identifier 收回覆）、TCP（未連線、listener、連線）。暫時 port 從 49152 起。
- 送到沒人聽的 TCP port 回 RST；UDP 就丟掉。
- `take_events()`：自從上次呼叫有沒有「socket 可能在等的事」發生（資料、新連線、狀態改變），kernel 用它決定要不要叫醒等待者。

### 4.2 TCP（`tcp.rs`）

```
 主動開啟：SynSent ──SYN+ACK──► Established
 被動開啟：（listener 收到 SYN）SynReceived ──ACK──► Established
 我們先關：Established ─FIN─► FinWait1 ─ACK─► FinWait2 ─FIN─► TimeWait ─2 秒─► Closed
 對方先關：Established ─FIN─► CloseWait ─（close）FIN─► LastAck ─ACK─► Closed
```

- 序號比較都是模 2³² 的（`lt`／`le`）。
- **重傳**：逾時加倍到 8 秒、最多 8 次（之後 `TimedOut`）；逾時時 go-back-N：退回最舊未確認的位元組重送；FIN 也會重送。

### 4.3 P11：RTT、壅塞控制、快速重傳、延遲 ACK、重組（使用者已於 2026-10-22 審核）

**比喻**：寄包裹時先估計郵差來回要多久（RTT），等太久才補寄（RTO）；一開始只寄幾個試試路況，順了就越寄越多（慢啟動），掉件就減半（壅塞避免）；對方說「我一直在等第 N 號」三次，就馬上補寄第 N 號，不必等逾時（快速重傳）；收件方不必每收一件就回條，每兩件或等一下再回（延遲 ACK）；先到的後面幾號先收著，等前面的到了一起交出去（重組）。

| 機制 | 做法 |
|---|---|
| RTT（RFC 6298） | 一次量一個段（握手的 SYN、之後新送出的資料）；重傳過的不量（Karn）。第一個樣本 R：SRTT = R、RTTVAR = R/2；之後 RTTVAR = ¾RTTVAR + ¼|SRTT−R|、SRTT = ⅞SRTT + ⅛R；RTO = SRTT + max(1 ms, 4·RTTVAR)，夾在 1 s（RFC 的下限）到 8 s；有進展的 ACK 讓退避過的 RTO 回到估計值 |
| 慢啟動／壅塞避免（RFC 5681） | 初始 cwnd = 3 段（MSS 1460 時 4380 bytes），ssthresh = 65535；新 ACK：cwnd < ssthresh 時加 min(確認量, MSS)，否則加 MSS²/cwnd；送出量 ≤ min(對方視窗, cwnd) |
| 逾時 | ssthresh = max(在途/2, 2 MSS)、cwnd = 1 段、go-back-N |
| 快速重傳／快速恢復 | 第 3 個重複 ACK（同一個 ack、沒有資料、視窗沒變、有在途資料）：ssthresh 減半、立刻重送最舊的段、cwnd = ssthresh + 3 MSS；之後每個重複 ACK cwnd + MSS；部分 ACK 再補送下一個洞（RFC 6582 的精神）；確認到恢復點時 cwnd = ssthresh |
| 延遲 ACK（RFC 1122 4.2.3.2） | 按順序的資料：每第 2 段立刻 ACK，否則 200 ms 內（`ACK_DELAY_MS`，net 執行緒 100 ms 一步，所以實際 ≤ 300 ms）；亂序、填補了洞、FIN、舊資料重來、視窗重新打開：立刻 ACK；我們送出的任何段都帶 ACK，所以有資料要送時就順便確認了 |
| 重組 | 洞後面的段（在接收視窗內、總量不超過 64 KiB 緩衝）存在 `ooo`（依序號排序）；洞補上時接著交出去；洞後面的 FIN 記著（`fin_at`），資料到齊才算 |

`Tcb::next_timer()` 給下一個計時器（重傳或延遲 ACK）的時間；`rto()`、`srtt()`、`cwnd` 給測試與診斷。
- **視窗**：接收緩衝 64 KiB，宣告的視窗是剩下的空間；對方視窗是 0 時，沒有在途的資料就送 1 byte 探測。讀走資料讓視窗從 < MSS 變回 ≥ MSS 時主動送一個 ACK。
- MSS：我們宣告 1460；對方沒說就用 536。
- TIME-WAIT 縮短成 2 秒。
- 應用程式 close 之後連線留著直到協定做完（FIN 交換完、TIME-WAIT 結束）才釋放 slot；`shutdown` 只送 FIN、還能讀。

## 5. kernel（`kernel/src/net.rs`）

```
 網卡中斷（或沒有 MSI 時每 5 ms）─► net 執行緒：
     收下所有 frame ─► stack.receive；stack.poll（計時器）；
     關掉最後一個 handle 已經不在的 socket；frame 交給網卡；
     有事件就叫醒所有在 socket 呼叫裡等待的執行緒
 socket 呼叫 ─► 上鎖、堆疊操作、frame 送出；要等的話：
     加入等待者、放鎖、睡（被叫醒或到期限）、醒來重新檢查
```

- 堆疊在一把 SpinLock 後面（操作都很短、不會睡）。
- `Socket` 是 kernel 物件（`KernelObject::Socket`，`object_kind::SOCKET` = 9）；最後一個 handle 關掉時（`Drop`）把 id 放進 `CLOSING`，由 net 執行緒關（handle 可能在持有堆疊鎖的時候被丟掉，所以不直接關）。
- 開機時 `kernel::net::start()`（`usb::start` 之後）啟動執行緒並開始 DHCP；拿到位址時印 `net: DHCP: 10.0.2.15, gateway 10.0.2.2, DNS 10.0.2.3`。
- selftest 先用原始 frame 測網卡（`net: raw frames work`），再啟動堆疊等 DHCP（`net: stack configured`）。

## 6. 系統呼叫與 libstela

| 號碼 | 呼叫 | 參數 | 回傳 |
|---|---|---|---|
| 39 | `SocketCreate` | kind（`net::UDP` 1、`TCP` 2、`ICMP` 3）, network（P11：`capability::NETWORK` 的 handle，沒有 → `AccessDenied`） | handle（READ、WRITE、DUPLICATE、TRANSFER） |
| 40 | `SocketBind` | socket, port（0：任意） | port |
| 41 | `SocketConnect` | socket, addr（`net::pack`）, timeout ns | 0；TCP 等到連上，UDP 設預設目的地 |
| 42 | `SocketListen` | socket, backlog（1–64） | 0 |
| 43 | `SocketAccept` | socket, timeout | 新 handle |
| 44 | `SocketSend` | socket, buf, len, addr | bytes；TCP 等到有空間（回傳放進去的量）；UDP 一個 datagram；ICMP 送 echo 請求（addr 的 port 是序號） |
| 45 | `SocketRecv` | socket, buf, len, timeout, &mut addr | bytes（TCP 0 = 結束）；UDP/ICMP 一個 datagram，來源寫進 addr（8 bytes） |
| 46 | `NetInfo` | &mut [u8; 80] | 1（0 = 沒有網卡）：driver 名稱、MAC、link、configured、IP、遮罩、閘道、DNS；P11 K28 起 48–79 是收／送的 frame 數與 bytes（u64，net 執行緒在 `card.receive`／`card.send` 成功時累加；`ifconfig` 印出來，`docs/hwinfo.md`）。之前是 48 bytes |

`net::pack(ip, port)` = IP 的 4 bytes（大端）在 bit 47..16、port 在 bit 15..0。新錯誤：`ConnectionRefused` 22、`ConnectionReset` 23、`NetworkDown` 24（沒有網卡或還沒有位址）、`AddressInUse` 25。

libstela `net`：`Socket::{udp, tcp, icmp, bind, connect, listen, accept, send, send_all, recv}`（Drop 關 handle）、`info()`、`wait_configured()`、`resolve(name)`（IP 字面值或 DNS：UDP 到 DHCP 給的伺服器，A 記錄，處理壓縮指標，3 次 × 2 秒）、`parse_url`、`http_get(url)`（HTTP/1.0、`Connection: close`，讀到伺服器關閉為止）。

程式（initrd，shell 直接打名字就會執行）：

| 指令 | 做什麼 |
|---|---|
| `ifconfig` | 網卡、MAC、link、IP／遮罩／閘道／DNS（還沒 DHCP 就等 5 秒） |
| `ifconfig resolve 名字` | DNS 查詢 |
| `ping host [次數]` | ICMP echo，每秒一次，印往返時間 |
| `fetch [--ca 根.der] http(s)://host[:port]/path [檔案]` | HTTP GET（P12 起也有 HTTPS，`docs/tls.md`；`--ca` 另外信任的根憑證），印出內容或存到檔案（相對於目前資料夾） |

## 7. Linux 人格的 socket（`kernel/src/linux/socket.rs`）

只有 `AF_INET` + `SOCK_STREAM`／`SOCK_DGRAM`（`SOCK_NONBLOCK`、`SOCK_CLOEXEC` 旗標有效）。descriptor 是 `OpenFile::Socket`，`read`/`write` = recv/send，`fstat` 回 `S_IFSOCK`。

| Linux 呼叫 | 號碼 | 對應 |
|---|---|---|
| `socket` | 41 | `net::create` |
| `connect` | 42 | `net::connect`（阻塞） |
| `accept`／`accept4` | 43／288 | `net::accept`，寫回對方位址 |
| `sendto`／`recvfrom` | 44／45 | `net::send`／`net::recv`（`MSG_DONTWAIT`、非阻塞 → `EAGAIN`） |
| `sendmsg`／`recvmsg` | 46／47 | P11：iovec 合成一次 send／一次 recv 分散到 iovec；`msg_name` 是位址；不支援 control 資料（`EOPNOTSUPP`） |
| `shutdown` | 48 | `SHUT_WR`/`SHUT_RDWR` 送 FIN；`SHUT_RD` 什麼都不做 |
| `bind`／`listen` | 49／50 | 只看 port |
| `getsockname`／`getpeername` | 51／52 | |
| `setsockopt`／`getsockopt` | 54／55 | 忽略／一律 0（`SO_ERROR` = 沒有錯誤） |

錯誤：`ECONNREFUSED`、`ECONNRESET`、`ENETDOWN`、`EADDRINUSE`、`ENOTSOCK`、`EAFNOSUPPORT`、`EPROTONOSUPPORT`。

## 8. 測試

| 測試 | 內容 |
|---|---|
| host `net` | checksum（RFC 1071 的例子）、各種封包的組裝與解析（含錯的檢查碼、分片）；TCP：握手、雙向資料、分段、視窗、遺失重傳、亂序保留與重複 ACK、放棄、拒絕、主動／被動關閉、FIN 重傳、RST、序號繞回；P11：延遲 ACK（一段等 200 ms、每兩段立刻）、慢啟動（先 3 段、cwnd 變大）、三個重複 ACK 立刻重送並補洞、RTT 樣本與 RTO（1.5 s 的握手、之後 300 ms）、洞後面的 FIN；Stack：P11 的「慢又會掉的線」（單向 20 ms、每 25 個掉 1 個）上傳 300 KB，20 秒（模擬時間）內完成且資料正確；Stack：ARP 後 ping、ARP 放棄、往閘道送、UDP 雙向與 port 佔用、TCP 連線傳 10 KB 再關、拒絕、遺失段、shutdown 後還能讀、沒有位址時拒絕、DHCP（含重送） |
| host `drivers` | PCI id → 家族；e1000 描述子、MAC；virtio capability、協商、環的配置 |
| host `libstela` | DNS 查詢的格式、回覆（CNAME + 壓縮指標、NXDOMAIN）、URL、HTTP 回應 |
| host `abi` | 編號 39–46、錯誤 22–25、`pack`、`NetInfo` |
| selftest "network card"／"network stack" | 原始的 DHCP DISCOVER → slirp 的 OFFER；堆疊 10 秒內拿到位址（q35 預設的 e1000e；`test-qemu-min` 同樣） |
| `make test-net` | `tools/with_http.py` 在 host 的 127.0.0.1:8642 serve `build/www/`，e1000、e1000e、virtio-net 各開機一次：`ifconfig`（10.0.2.15）、`ping 10.0.2.2 2`、（host 能解析名字時）`ifconfig resolve example.com`、`fetch http://10.0.2.2:8642/hello.txt`、`run linux/net 8642 host`（host 能解析名字時加 `example.com`：musl 的 `getaddrinfo`，P11）；約 1 分鐘 |

手動驗證過：1 MiB 的檔案用 `fetch` 存進 `Tmp:/`（大小正確）。

## 9. 計畫外的決定

- syscall 從 **39** 開始（計畫寫 38，但 38 已經是 P9 的 `FileSetInfo`）。
- 加了 ICMP socket（`net::ICMP`）給 `ping` 用（計畫的 syscall 表沒有 ping 的方式）。
- 加了四個錯誤碼與 `nonet` 開機選項。
- e1000（82540EM）沒有 MSI，改成輪詢。
- `make test-net` 用自己的 `tools/with_http.py`（ThreadingHTTPServer）而不是 `python3 -m http.server`，才能在測試結束時確定停掉伺服器，並順便判斷 host 有沒有 DNS。

## 10. 已知限制

- 一次一張網卡；沒有 IPv6、IP 分片重組、IP 選項、多播、路由表（只有預設閘道）、DHCP 續租。
- TCP：沒有 Nagle、SACK、視窗縮放（接收緩衝 64 KiB 就是上限）、keepalive、ECN；TIME-WAIT 只有 2 秒；RTO 下限 1 s（RFC 6298 的建議；區域網路上的掉件主要靠快速重傳）。
- P11 起 socket 可以放進 WaitSet（`docs/ipc.md` §13），Linux 有 `poll`/`select`（`docs/linux.md` §5.3）與 `sendmsg`/`recvmsg`，musl 的 `getaddrinfo` 經合成的 `/etc/resolv.conf` 可用；等待者仍被「任何事件」全部叫醒再各自檢查（簡單但 socket 多時浪費）；沒有 `epoll`。
- 任何程式都能開 socket（沒有能力限制）。
- e1000 輪詢：P11 起沒有 socket（或沒有 link）時每 100 ms 才看一次——DHCP、ARP 照常（慢一點），開了 socket 的程式第一個封包最多晚 100 ms。
- I219（P520）只依資料手冊與 68k_HAL 的經驗實作，**沒有在實機驗證**（PHY、ULP、Management Engine 都可能讓 link 起不來）。

## P13：USB 網卡與換卡（K10，使用者已於 2026-10-22 審核）

網卡不再只在開機時決定：PCI 有線網卡優先；沒有時 USB 乙太網卡（CDC-ECM、CDC-NCM、RNDIS、AX88179，`docs/usb.md` §13）來了就用、走了就沒有網卡。每次換卡是一個新的 `Stack`（generation +1，DHCP 重來），舊的 socket 一律 `NetworkDown`。網路執行緒在沒有網卡時照樣跑（每 100 ms 看一次有沒有新的網卡）。
