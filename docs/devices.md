# 使用者空間驅動介面

> P13 C（K11–K12）由自動排程一次做完，**使用者尚未審核**。

STELA 的驅動原本都在 kernel（`drivers/`，safe Rust，經 frame 碰硬體）。這個介面讓一個**使用者程式**驅動一張 PCI 卡：kernel 把那張卡「借」給它，給它看設定空間、把暫存器（BAR）映射給它、給它 DMA 用的連續記憶體、把中斷轉成訊息。第一個例子是 `user/e1000u`（kernel 的 e1000 驅動改寫成使用者程式），之後 FreeBSD 驅動的相容層要接在這裡。

程式碼：

```
abi/src/lib.rs            Syscall 66–74、capability::DEVICES、tag::DEVICE_IRQ／NET_TX_READY、object_kind 16／17
frame/src/mm/device.rs    bar_range：檢查過不是 RAM 的 BAR 範圍（DeviceRange）
frame/src/mm/shared.rs    SharedMemory::new_contiguous、phys（DMA）
drivers/src/pci/          parse_location（BB:DD.F）、is_claimed（kernel 驅動 map_bar 過的）
kernel/src/device.rs      Device、NetTap 物件與 syscall、devirq 執行緒
user/libstela/src/device.rs  Device、Bar（volatile 存取，唯一的 unsafe）、NetTap
user/e1000u/              使用者空間的 e1000／e1000e 驅動
user/init                 開機選項 usernet 時啟動 e1000u（只給它 DEVICES）
```

## 1. 比喻：出借工具

kernel 是工具房管理員。一般人（程式）不能進工具房；拿著「工具證」（`DEVICES` 能力）的人可以借一台機器（`DeviceOpen`）：借期內只有他能用（一台一個借用人），管理員自己在用的不借（kernel 驅動已經 `map_bar` 的卡）、拆了會出事的不借（橋接器、顯示卡）。借出去時附上操作面板（BAR 映射）、一塊專用的工作台（DMA 記憶體，知道它的實體位址才能告訴機器去哪裡拿東西）、一個鈴（中斷 → 訊息）。還回來（handle 關掉、程式結束）時管理員把機器斷電（bus master 關、MSI 關）。

但這台機器能自己搬東西（DMA, bus master），而工具房沒有門禁（**沒有 IOMMU**）：它能搬到任何地方。所以借機器的人等於被信任——**持有 `Device` 的程式等於持有整台機器**。工具證只有 init 發，只發給它啟動的驅動。

## 2. 呼叫（`abi::Syscall`）

| 呼叫 | 內容 |
|---|---|
| `DeviceOpen(path, len, devices)` 66 | `BB:DD.F` 或 `SSSS:BB:DD.F`；需要 `DEVICES` 能力（`AccessDenied`）；class 06（橋）、03（顯示）`AccessDenied`；kernel 驅動用的 `Busy`；同時只借一個人（`Busy`） |
| `DeviceConfig(device, offset, size \| WRITE, value)` 67 | 讀寫 1／2／4 bytes 的設定空間（offset < 0x1000、對齊）；**不能寫 BAR**（0x10–0x27：搬動 BAR 可能讓它蓋到 RAM） |
| `DeviceMapBar(device, bar, size_ptr)` 68 | 把記憶體 BAR 映射進程式（uncached、不可執行），寫回大小，打開 memory decoding；`frame::mm::device::bar_range` 拒絕和 RAM 重疊的範圍 |
| `DeviceDma(device, pages, phys_ptr)` 69 | 1–1024 頁實體連續、清零的記憶體（`Memory` handle），寫回實體位址；打開 bus master |
| `DeviceIrq(device, port)` 70 | 卡的 MSI（沒有就 MSI-X 的第 0 項）指到 kernel 配的向量；每次中斷送 `tag::DEVICE_IRQ`（向量、累積次數）到 `port` |
| `DeviceReset(device)` 71 | bus master、MSI、MSI-X 關掉，放掉向量 |
| `NetAttach(port, mac_ptr, devices)` 72 | 程式裡的網卡變成 kernel 網路的網卡（`NetTap`） |
| `NetDeliver(tap, buf, len)` 73 | 網卡收到的 frame 交給 kernel |
| `NetTake(tap, buf, len)` 74 | kernel 要送的下一個 frame（0：沒有）；有東西要送時 kernel 送一則 `tag::NET_TX_READY` |

`Device` handle 關掉（或程式結束）時 kernel 自動 reset 並放掉向量；`NetTap` 關掉時就像 USB 網卡拔掉（`docs/usb.md` §13：網路變回沒有網卡）。

## 3. 中斷

```
 卡的 MSI ─► kernel 向量 N 的 handler：PENDING[N] += 1，叫醒 devirq（不碰卡）
 devirq 執行緒：每個有次數的向量 → DEVICE_IRQ（N、次數）送到登記的 port
 程式：收到 DEVICE_IRQ → 自己讀卡的中斷原因暫存器（e1000：讀 ICR 即確認）
```

計畫的決定：handler 只說「向量 N 響了」，不讀卡的暫存器（kernel 不知道每種卡怎麼確認中斷）；MSI 是 edge 觸發，不會因為沒確認而一直響。訊息經一個 kernel 執行緒送出（中斷裡不配置記憶體、不拿 port 的鎖）。devirq 最多 50 ms 醒一次，萬一叫醒漏了也只晚這麼久。

## 4. 網卡：`NetTap`

```
 kernel 的 net 執行緒 ── NetDevice::send ─► TX 佇列 ─► 空變非空時 NET_TX_READY ─► 程式 NetTake 拿走、送給卡
 程式：卡收到 frame ─► NetDeliver ─► RX 佇列 ─► 叫醒 net 執行緒 ─► stack
```

`NetTap` 走和 USB 網卡一樣的路（`kernel::net::usb_attached`）：PCI 有線網卡在用時不會換掉它；所以要用使用者空間的驅動，kernel 不能先把那張卡拿走——開機選項 **`usernet`**：kernel 不 probe PCI 網卡（e1000 沒被 `map_bar`，所以借得出去），init 啟動 `Initrd:/e1000u`。

## 5. `user/e1000u`

kernel 的 `drivers/src/net/e1000.rs` 的使用者版：在 bus 0 上找 kernel 借得出來的 82540EM（QEMU 的 e1000）或 82574（e1000e）；reset 與啟動的順序、數值和 kernel 驅動一樣（8254x SDM §13–14）；描述子與緩衝放在 `DeviceDma` 的記憶體（經 `Mapping` 讀寫）、暫存器經 `Bar::read_u32`／`write_u32`。有 MSI（e1000e）就用 `DeviceIrq`、等 `DEVICE_IRQ`，沒有（e1000）就每 5 ms 看一次。主迴圈：收到的 frame 一個個 `NetDeliver`，kernel 的 frame `NetTake` 到沒有為止。

`libstela::device::Bar` 是使用者程式唯一直接碰硬體的地方：32-bit 的 volatile 讀寫，檢查對齊與 BAR 的大小（超出就讀 0、不寫）。這是 libstela 裡新增的 `unsafe`（和 syscall、heap 一樣只在 libstela，`CLAUDE.md` 的 unsafe 規則）。

## 6. 測試

| 測試 | 內容 |
|---|---|
| host `drivers::pci` | `parse_location` |
| host `abi` | 編號 66–74、`DEVICES` = 5、`MAX` = 5、tag 11／12、`device::WRITE` |
| selftest／proctest | 沒有 `DEVICES` 的程式 `DeviceOpen`、`NetAttach` 都是 `AccessDenied`；拿 port 冒充能力也是 |
| `make test-net-user` | 開機選項 `usernet`：e1000e（`MSI vector N`、`first interrupt`）與 e1000（`polled`）各一次，DHCP、ping、fetch 都經過 e1000u |

## 7. FreeBSD 相容層要接在這裡

`docs/architecture.md` 規劃的 FreeBSD 驅動相容層（Wi-Fi、GPU）會是使用者程式：它的 `bus_space`／`bus_dma`／`pci_*`／中斷 API 對應到這裡的 `DeviceConfig`、`DeviceMapBar`、`DeviceDma`、`DeviceIrq`；網路驅動經 `NetTap` 接上 kernel 的網路。還缺的：I/O port BAR、多個 MSI-X 向量、DMA 的分散表（現在只有一整塊連續的）、DMA 記憶體的快取屬性、USB 裝置的出借、IOMMU（有了才能放寬信任）。

## 8. 已知限制

- 沒有 IOMMU：`DEVICES` 等於整台機器的權限。
- 只有 PCI（USB 裝置不能借）；只有記憶體 BAR（沒有 I/O port）；一個裝置一個向量（MSI 一個 message，或 MSI-X 第 0 項）；向量與 MMIO 映射的虛擬位址不回收（和 kernel 驅動一樣）。
- `DeviceDma` 一次最多 1024 頁、一定實體連續；記憶體的 handle 交出去後 kernel 不知道卡還在不在用它（程式結束時先 reset 卡，再放記憶體）。
- `NetTap` 一次一張；kernel 的佇列各 256 個 frame，滿了就丟。
- e1000u 只認 bus 0 的 82540EM 與 82574，沒有 I219 的特殊處理（kernel 驅動有）。
