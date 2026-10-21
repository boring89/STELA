# 硬體報告：`SysInfo(HARDWARE)` 與 `hwinfo`（P11 K28，使用者尚未審核）

**比喻**：實機測試時最常問的是「STELA 到底看到了什麼？」——哪顆 CPU、多少記憶體、哪些 PCI 裝置、磁碟怎麼分割、網卡有沒有 link。以前要翻開機時捲過去的 serial 訊息；現在 kernel 把它看到的整理成一份「體檢報告」，任何程式都能要，`hwinfo > Data:/hwinfo.txt` 就能存下來帶回去看。

## 1. 介面

- `SysInfo(HARDWARE = 9, buf, len) -> 長度`：UTF-8 文字，一行一項；回傳**整份**的長度（可能比 `len` 大，這時只寫了前 `len` bytes，用更大的緩衝再要一次）。libstela `process::hardware() -> Result<String, Error>`（從 16 KiB 開始，不夠就用回傳的長度重來）。
- 每次呼叫都重新產生（`kernel/src/hwinfo.rs` 的 `report()`），所以 USB 熱插拔、網路計數、可用記憶體都是當下的值。
- `hwinfo`（initrd 裡的程式）：印出整份。Mouseion 的 **Mouseion > About STELA** 取開頭的摘要行（`cpu:`、`processors:`、`memory:`、`display:`）加上開機到現在的時間，放進資訊視窗（太長的截到 40 字）。

## 2. 內容

```
STELA 0.1.0
cpu: <品牌字串> (<vendor>, family F model M stepping S)
cpu features: sse2 sse3 ... smep nx invariant-tsc hypervisor
processors: N online
memory: 502 MiB usable, 466 MiB free
kernel heap: 2.1 MiB of 32 MiB used
display: 2560x1600, pitch 10240 bytes, BGRX 8:8:8, 15 MiB at 0x80000000
timer: APIC 1000 Hz; TSC 3696 MHz, not invariant

[memory map] 34 regions        起點、大小、種類（usable、kernel and loader、reserved、ACPI reclaimable、ACPI NVS、MMIO）
[acpi] 8 tables                XSDT 列的每個表：簽名、長度、實體位址
[pci] 7 functions              BDF、vendor:device、class.subclass.prog_if、種類
[usb] 2 devices                port 路徑、vendor:product、keyboard/mouse/hub/other、速度、slot
[disks] 1                      每顆磁碟的大小，GPT 的每個分割區（編號、名稱、大小、起始 sector、型別：EFI system 或 GUID）
[volumes] 4                    卷宗名稱、檔案系統、唯讀、大小與剩餘
[network]                      driver、MAC、link、IP/前綴、閘道、DNS、收送的 frame 數與 bytes
[audio]                        HDA 的 PCI 位址、codec 位址、codec 的 vendor:device、輸出 pin 與 converter
```

## 3. 資料從哪裡來

| 項目 | 來源 |
|---|---|
| CPU | `frame::cpuid`（新，純邏輯 + host 測試）：leaf 0 的 vendor（EBX、EDX、ECX）、leaf 1 EAX 的 family/model/stepping（family 0FH 加 extended family；family 06H／0FH 把 extended model 放前面，Intel SDM Vol. 2A「CPUID」Figure 3-6）、80000002H–80000004H 的品牌字串、特徵位元表 `FEATURES`（leaf 1 ECX/EDX、leaf 7 EBX、80000001H EDX bit 20 = NX、80000007H EDX bit 8 = invariant TSC）。只讀存在的 leaf（leaf 0 / 80000000H 的最大值）。 |
| 處理器數、記憶體、heap | `frame::cpu::online`、`pmm::stats`、`heap::stats` |
| 顯示 | loader 的 `BootInfo.framebuffer`（`frame::boot_framebuffer`）；GOP 的模式編號 loader 沒有傳，所以沒有 |
| 記憶體表 | `BootInfo` 的區段（`frame::memory_regions`；合併後、最多 256 個） |
| ACPI | `frame::acpi::table_list`：`init` 時把 XSDT（或 RSDT）列的每個表頭讀一次記下 |
| PCI | `drivers::pci::system::devices`（開機時掃過的） |
| USB | `xhci::devices`（P11 起 `DeviceInfo` 多 `ids`、`speed`：列舉時讀到的 device descriptor） |
| 磁碟 | `block::disks` + `block::gpt::read` |
| 卷宗 | `vfs::volumes` 的 `space`、`kind`、`read_only` |
| 網路 | `net::info`（`NetInfo` 的內容；計數是 P11 新加的） |
| 音效 | `audio::description`：`Hda::description`（probe 時組好；codec 的 vendor id 是根節點的參數 00H，HDA 規格 7.3.4.1） |

格式化的輔助函式（`size`、`region_kind`、`volume_kind`、`prefix`、`mac`、`signature`、`network_line`）是純邏輯，host 測試（`kernel/src/hwinfo.rs`）。

## 4. 測試

- host：`frame::cpuid`（family/model 的組合、假的 CPUID 表解出 vendor、品牌、特徵；沒有延伸 leaf 的舊 CPU）、`kernel::hwinfo`（大小的單位與小數、名稱、網路那一行）、`abi`（`NetInfo` 80 bytes 往返）。
- `make test-shell`：打 `hwinfo`，要看到 `cpu: `、`processors: 2 online`、`[acpi]`、`FACP`、q35 的 host bridge `8086:29c0 class 06.00.00 host bridge`、`[disks] 2`（QEMU 的 `fat:rw` ESP 與 AHCI 測試磁碟）與 GPT 的 `2 "Data"`、`[network]` 與 `hwinfo exited with code 0`。
- `make test-net`：`ifconfig` 多一行 `received … sent …`。

## 5. 限制

- 文字格式給人看，不是穩定的機器介面（程式要用的值仍有各自的 `SysInfo` 種類）；Mouseion 只依賴開頭幾行的前綴。
- 沒有 SMBIOS（主機板、BIOS 版本）、沒有每顆 CPU 各自的資訊（只讀執行 `report` 的那顆）、沒有 NVMe／AHCI 的型號字串（IDENTIFY 的 model number）、沒有 PCI 名稱資料庫（只有 class 的大分類）。
