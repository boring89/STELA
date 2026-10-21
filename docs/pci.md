# PCI：列舉、BAR、capability 與 MSI

> P5 由自動排程一次做完，使用者已於 2026-10-20 審核。

STELA 怎麼找到 PC 上的裝置（儲存控制器、網卡、顯示卡……），以及怎麼讓它們發中斷。架構決定見 `docs/architecture.md` §4 的 F（驅動在 kernel 內、safe Rust、只透過 frame 碰硬體）。

程式碼：

```
frame/src/acpi/tables.rs     parse_mcfg（純邏輯，host 測試）
frame/src/acpi/mod.rs        acpi::mcfg()：開機時順便找 MCFG
frame/src/pci.rs             legacy_read32/write32：0xCF8/0xCFC
frame/src/irq.rs             alloc_vector()
drivers/src/pci/mod.rs       ConfigSpace trait、scan、bar、capabilities、enable_msi（純邏輯，host 用假匯流排測）
drivers/src/pci/system.rs    ECAM／legacy 存取、devices()、map_bar、enable_bus_master、enable_msi（含 MSI-X）
```

## 1. 比喻：大樓的住戶名冊

PCI 是一棟大樓：**匯流排（bus）**是樓層，每層 32 個**房間（device）**，每間最多 8 個**住戶（function）**。每個住戶門口有一張 256 bytes（PCI Express 是 4 KiB）的**名牌**（configuration space）：寫著是誰（vendor/device id）、做什麼的（class）、要用哪些空間（BAR）、有什麼特殊本事（capability 清單）。

- 讀名牌的方法有兩種：老方法是對管理室打兩通電話（I/O port 0xCF8 寫地址、0xCFC 讀資料）；新方法是整棟大樓的名牌都影印在一面牆上（**ECAM**：一段記憶體位址，ACPI 的 **MCFG** 表告訴我們牆在哪）。
- 有的房間是**樓梯間**（PCI-to-PCI bridge），名牌上寫著「往上是第幾樓」（secondary bus），列舉要跟著走上去。
- **BAR** 是住戶向大樓要的「信箱」：一段記憶體位址（或 I/O port），驅動透過它跟裝置說話（MMIO）。
- **MSI** 是住戶按門鈴的方式：不拉中斷線，而是往一個特定位址寫一個值，CPU 的 local APIC 收到就當成中斷。

## 2. 名詞

| 名詞 | 意思 |
|---|---|
| BDF | bus:device.function，例如 `00:1f.2` |
| ECAM | Enhanced Configuration Access Mechanism：bus `b`、device `d`、function `f` 的設定空間在 `base + (b << 20 | d << 15 | f << 12)`（PCI Express Base Spec §7.2.2） |
| MCFG | ACPI 表，列出 ECAM 視窗：base、segment、起訖 bus（PCI Firmware Spec 3.0 §4.1.2） |
| BAR | Base Address Register（0x10–0x24）：裝置的記憶體或 I/O 視窗 |
| capability | 設定空間裡的鏈結串列（0x34 起），每項 `id, next`；MSI = 0x05、PCIe = 0x10、MSI-X = 0x11 |
| MSI | Message Signaled Interrupt：裝置寫 `address`（0xFEE…）= `data`（向量）就是中斷 |

## 3. 設定空間存取

- 開機時 `acpi::init` 在 XSDT 裡也找 `MCFG`（`parse_mcfg`：表頭後 8 bytes 保留，接著 16 bytes 一項）。QEMU q35：`0xE000_0000`（OVMF 設定）、segment 0、bus 0–255。
- 有 segment 0 從 bus 0 起的視窗 → ECAM：每個 bus 第一次被讀時 `mmio::map(base + bus << 20, 1 MiB)`（不快取），之後一直留著。
- 沒有 MCFG → legacy：`frame::pci::legacy_read32/write32`（Configuration Mechanism #1：`0x8000_0000 | bus << 16 | dev << 11 | fn << 8 | reg`，PCI Local Bus Spec 3.0 §3.2.2.3.2），在一把鎖下寫地址再讀寫資料，只到 256 bytes。
- 一律 32 位元對齊存取；16／8 位元欄位用讀-改-寫（`read16`、`write16`、`read8`）。
- selftest 檢查 ECAM 與 legacy port 讀到的頭幾個暫存器相同。

## 4. 列舉（`scan`，OSDev Wiki "PCI" 的 Recursive Scan）

```
 scan_bus(b)：for device 0..32
     function 0 不存在（vendor 0xFFFF）→ 下一個
     header type bit 7 = 多功能 → function 0..8，否則只有 0
     class 06/04（PCI bridge，header type 1）→ scan_bus(secondary bus，0x19)
 00:00.0 是多功能的 host bridge → 每個 function f 是 bus f 的根
```

`visited[256]` 防止迴圈。結果依 BDF 排序。開機印：

```
pci: ECAM at 0xe0000000, buses 0-255, 7 function(s)
pci: 0000:00:00.0 8086:29c0 class 06/00/00 host bridge
pci: 0000:00:03.0 1b36:0010 class 01/08/02 NVMe controller
pci: 0000:00:1f.2 8086:2922 class 01/06/01 AHCI controller
```

### 4.1 每一個 bus（P12，使用者尚未審核）

遞迴掃描只找得到「從 bus 0 經過橋」接到的 bus。有好幾個根 complex 的機器（P520：NVMe 在 bus 0x17、GTX 在 bus 0x65，`docs/p520.md` §6.5）有沒有橋指向的 bus。所以遞迴之後，0–255 每個還沒看過的 bus 再掃一次（OSDev Wiki "PCI" 的 Brute Force Scan；`visited` 避免重複）。先用 `ConfigSpace::present` 問 32 個裝置的 function 0 有沒有回應——`Access` 經舊式的 CF8h/CFCh port 問（ECAM 的視窗一個 bus 要映射 1 MiB、而且映射不回收，空的 bus 不值得）；有回應才 `scan_bus`。host 測試：沒有橋指向的 bus 0x17、0x65 上的裝置也找得到，橋後面的 bus 只掃一次。

## 5. BAR（`bar`，PCI Local Bus Spec 3.0 §6.2.5.1）

```
 讀原值 → 寫 0xFFFF_FFFF → 讀回（沒接線的位元是 0）→ 寫回原值
 bit 0 = 1：I/O，大小 = !(mask & !3) & 0xFFFF + 1
 bit 2:1 = 2：64 位元記憶體，下一個 BAR 是高 32 位元
 bit 3：prefetchable
 記憶體大小 = !(mask & !0xF) + 1（64 位元時含高半）
```

量大小時先關掉 command 的 memory/IO decode，量完還原（量的時候位址是 0xFFFF…，不能讓裝置回應）。`map_bar(f, i)` = `mmio::map(addr, size)` + 開 memory decode。

## 6. MSI 與 MSI-X

- `irq::alloc_vector()`：從 `FIRST_FREE_VECTOR`（50）往上發到 0xEF，不回收。
- MSI capability（§6.8.1）：

```
 +2 message control：bit 0 enable、bits 6:4 multiple message enable、bit 7 64 位元
 +4 address（低 32）  +8 address 高 32（64 位元時） +8/+12 data
 address = 0xFEE0_0000 | apic_id << 12（Intel SDM Vol. 3A §11.11.1：fixed、physical）
 data    = vector（§11.11.2：edge、fixed delivery）
```

  一個 message、開啟，並在 command 設 INTx disable。
- MSI-X（§6.8.2）：QEMU 的 NVMe 只有 MSI-X。table 在某個 BAR（`+4` 的低 3 位元是 BIR，其餘是 offset）；只設 entry 0（address、data、vector control 的 mask 清掉），message control bit 15 enable、bit 14 function mask 清掉。table 的 MMIO 映射永久保留。
- `system::enable_msi(f, handler)`：配向量 → `irq::register` → MSI 優先，否則 MSI-X。沒有就回 `NoMsi`（驅動改用輪詢）。目的地是目前的 CPU（BSP）。

## 7. 測試

| 測試 | 內容 |
|---|---|
| host `frame::acpi::tables` | MCFG 解析、長度錯、bus 範圍反過來 |
| host `drivers::pci` | 假匯流排：bridge 後面的 bus、多功能、單功能的 function 1 不掃；32／64 位元記憶體 BAR 與 I/O BAR 的大小、沒實作的 BAR、command 還原；capability 走訪（含迴圈上限 48）、MSI 欄位 |
| selftest "pci" | ECAM 開著、host bridge 在 bus 0、ECAM 與 legacy 一致、AHCI（00:1f.2）的 BAR5 是記憶體、有 MSI capability、`alloc_vector` 連號 |

## 8. 已知限制

- 只有 segment 0；不處理 hot-plug、SR-IOV、bridge 的視窗重新配置（相信韌體的配置）。
- MSI 只用一個 message；MSI-X 只用 entry 0；向量不回收。
- 設定空間的寫入（bus master、BAR）和 MMIO 一樣是 safe API：裝置做 DMA 可以寫任何記憶體，驅動是被信任的 kernel 程式碼（沒有 IOMMU）。
