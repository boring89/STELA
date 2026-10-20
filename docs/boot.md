# 開機：UEFI loader 與 bootinfo

> P1 由自動排程一次做完，使用者已於 2026-10-20 審核。

這份文件說明 STELA 從按下電源到 kernel 第一行程式碼之間發生的事：`boot/`（loader）、`elfload/`（ELF 解析）、`bootinfo/`（loader 交給 kernel 的資料）。

## 1. 比喻：搬家

韌體（UEFI）像是房東：它會把房子（電腦）開門、通電、借你工具（檔案系統、顯示、記憶體配置）。loader 是搬家公司：

1. 用房東的工具把家具（`kernel.elf`）從倉庫（ESP 磁碟分割區）搬進來、擺到對的位置；
2. 抄下房子的平面圖（記憶體表）、電箱位置（ACPI RSDP）、電視（framebuffer）；
3. 換好門牌（分頁表：每個虛擬位址對應到哪個實體位址）；
4. 把鑰匙還給房東（`ExitBootServices`）——之後房東的工具都不能用了；
5. 把抄好的筆記（`BootInfo`）放在桌上，叫屋主（kernel）進門。

## 2. 名詞

| 名詞 | 意思 |
|---|---|
| UEFI | 現代 PC 的韌體介面。開機時載入 ESP 上的 `.efi` 程式並提供「boot services」 |
| ESP | EFI System Partition，FAT 格式的分割區。QEMU 用 `build/esp/` 目錄模擬 |
| boot services | 韌體在開機階段提供的函式（配置記憶體、讀檔、找顯示卡）。`ExitBootServices` 之後全部失效 |
| GOP | Graphics Output Protocol，UEFI 的顯示介面；給我們一塊線性 framebuffer |
| RSDP | ACPI 的根指標。P1 只記下位址，不解析 |
| ELF | kernel 檔案的格式。`PT_LOAD` 段 = 要搬進記憶體的區塊 |
| 分頁（paging） | CPU 用四層表把「虛擬位址」翻成「實體位址」 |
| HHDM | Higher-Half Direct Map：把全部實體記憶體原樣映射到高半部的一段虛擬位址，kernel 透過它讀寫任何實體位址 |
| CR3 | 存放最上層分頁表（PML4）實體位址的暫存器 |

## 3. 整體流程

```
OVMF（韌體）
  └─ 載入 \EFI\BOOT\BOOTX64.EFI（stela-boot）
       │ 1. 讀 \STELA\kernel.elf（uefi::fs）
       │ 2. elfload 驗證 → 配置一整塊頁面、複製 PT_LOAD、.bss 已是 0
       │ 3. 配置 64 KiB 堆疊、BootInfo
       │ 4. 記下 GOP framebuffer、RSDP
       │ 5. 建分頁表：HHDM、identity map、kernel 映射
       │ 6. ExitBootServices → 取得最終記憶體表 → 轉成 BootInfo.regions
       │ 7. 設 EFER.NXE、CR0.WP、CR3、RSP，jmp 到 entry
       ▼
kernel `_start(info: &'static BootInfo)`（RDI = BootInfo 的 HHDM 位址）
```

QEMU 實際輸出（512 MiB、OVMF）：

```
STELA boot loader 0.1.0
boot: kernel at 0x1de6b000 (56 KiB), entry 0xffffffff80002560
boot: framebuffer 1280x800, RSDP 0x1f77e014, direct map 4096 MiB, 14 page tables, NX on
boot: starting the kernel
STELA kernel 0.1.0
```

## 4. 虛擬位址配置

x86-64 的虛擬位址是 48 位元，分成兩半；中間是不能用的「非正規」洞。每個 PML4 項目管 512 GiB。

```
0x0000_0000_0000_0000 ┌──────────────────────────┐
                      │ 低半部（使用者空間，P3）   │ loader 暫時在這裡 identity map，
                      │                          │ kernel 啟動後立刻移除
0x0000_7FFF_FFFF_FFFF └──────────────────────────┘
                         （非正規位址，碰到就 #GP）
0xFFFF_8000_0000_0000 ┌──────────────────────────┐ PML4[256]
                      │ HHDM：實體 0.. 的直接映射 │ 2 MiB 大頁，可寫、不可執行
                      │ （QEMU 512 MiB：4 GiB）   │
                      ├──────────────────────────┤
0xFFFF_FF00_0000_0000 │ 動態區（kernel heap…）    │ PML4[510]，見 frame.md
                      ├──────────────────────────┤
0xFFFF_FFFF_8000_0000 │ kernel 映像               │ PML4[511]，4 KiB 頁，W^X
0xFFFF_FFFF_FFFF_FFFF └──────────────────────────┘
```

- **為什麼 kernel 在最後 2 GiB**：`x86_64-unknown-none` 預設 `code-model=kernel`，編譯器假設程式碼與資料位址落在 `-2 GiB..0`，可以用 32 位元有號位移定址，指令較短。
- **為什麼要 HHDM**：kernel 需要讀寫分頁表、PMM 的 bitmap、framebuffer 等「只知道實體位址」的東西。有 HHDM，實體位址 `p` 就在虛擬位址 `0xFFFF_8000_0000_0000 + p`。

## 5. kernel 的 ELF 與 `elfload`

kernel 由 `kernel/linker.ld` 連結成 **ET_EXEC**（固定位址、無重定位）。`.cargo/config.toml` 對 `x86_64-unknown-none` 加了 `-C relocation-model=static`，因為這個 target 預設會產生 static-PIE（需要 loader 套用重定位）。

linker script 把輸出分成三個 `PT_LOAD` 段，各自 4 KiB 對齊，讓 loader 能給不同權限（W^X：可寫的不可執行，可執行的不可寫）：

```
段       權限  實際位址（目前的建置）               大小
text     R-X   0xFFFFFFFF80000000                  40,740 bytes
rodata   R--   0xFFFFFFFF8000A000                  19,088 bytes
data     RW-   0xFFFFFFFF8000F000（含 .bss）        20,912 bytes
```

`elfload/` 只讀必要欄位（ELF-64 Object File Format 1.5；System V ABI AMD64 Supplement：`EM_X86_64 = 62`）：

| 欄位 | 位移 | 檢查 |
|---|---|---|
| `e_ident` | 0 | `7F 'E' 'L' 'F'`、64 位元、little endian、版本 1 |
| `e_type`／`e_machine` | 16／18 | `ET_EXEC`、`EM_X86_64` |
| `e_entry` | 24 | 必須落在可執行的段 |
| `e_phoff`／`e_phentsize`／`e_phnum` | 32／54／56 | 表格在檔案內 |
| `p_type`、`p_flags`、`p_offset`、`p_vaddr`、`p_filesz`、`p_memsz` | 0、4、8、16、32、40 | 段在檔案內、`filesz ≤ memsz`、位址頁對齊、遞增、不共用頁、不溢位 |

loader 把所有段複製進**一整塊連續**的實體頁面（`kernel_phys_base`，大小 `kernel_size`），`.bss`（`memsz` 超過 `filesz` 的部分）因為頁面是清零配置的，所以已經是 0。

## 6. loader 怎麼配置記憶體

loader 用 uefi crate 的 global allocator（feature `global_allocator`）。關鍵技巧：

```rust
#[repr(C, align(4096))]
struct Page([u8; 4096]);
vec![Page([0; 4096]); n].leak()
```

大小是 4096 的倍數、對齊 4096 的配置，uefi crate 會直接呼叫 `AllocatePages(LOADER_DATA)`。於是：

- 拿到的位址就是實體位址（UEFI 在 x86-64 上是 identity map，UEFI Spec 2.10 §2.3.4）；
- 型別是 `LOADER_DATA`，`ExitBootServices` 後韌體不會回收，kernel 看到的是 `KernelAndBoot`（使用中）；
- 全程不需要 `unsafe`（指標轉整數是安全的）。

`boot/` 只有兩處 `unsafe`：`exit_boot_services` 與最後的跳轉（`#![deny(unsafe_code)]` + 局部 `#[allow]`）。

## 7. 分頁表

### 7.1 四層結構（Intel SDM Vol. 3A §4.5）

48 位元虛擬位址拆成 4 個 9 位元索引 + 12 位元頁內位移：

```
 47      39 38      30 29      21 20      12 11         0
┌──────────┬──────────┬──────────┬──────────┬────────────┐
│ PML4 idx │ PDPT idx │  PD idx  │  PT idx  │   offset   │
└──────────┴──────────┴──────────┴──────────┴────────────┘
   CR3 → PML4 → PDPT → PD ──(PS=1: 2 MiB 頁)
                         └→ PT → 4 KiB 頁
```

例：kernel 的 `0xFFFF_FFFF_8000_0000` → PML4 511、PDPT 510、PD 0、PT 0。HHDM 的 `0xFFFF_8000_0000_0000` → PML4 256。

### 7.2 項目的位元（Vol. 3A Tables 4-15～4-20）

| 位元 | 名稱 | 用途 |
|---|---|---|
| 0 | P | 存在 |
| 1 | R/W | 可寫 |
| 7 | PS | 在 PD 裡：這是 2 MiB 大頁 |
| 12–51 | 位址 | 下一層表或頁面的實體位址 |
| 63 | XD／NX | 不可執行（需 `IA32_EFER.NXE = 1`，Vol. 3A §4.1.1） |

中間層一律 `P | R/W`（不設 NX），由最後一層決定權限。

### 7.3 loader 建的三組映射

| 映射 | 範圍 | 頁大小 | 權限 |
|---|---|---|---|
| HHDM | 實體 `0..hhdm_size` → `0xFFFF_8000_0000_0000 + p` | 2 MiB | 可寫、NX |
| identity | 實體 `0..hhdm_size` → 同位址 | 2 MiB | 可寫、可執行 |
| kernel | 每個 `PT_LOAD` 段 → 連結位址 | 4 KiB | 依 `p_flags`（W^X） |

`hhdm_size` = max(RAM 類區段的最高位址, framebuffer 結尾, 4 GiB)，向上取 2 MiB。排除 Reserved／MMIO 是因為 QEMU 在 `0xFD_0000_0000`（1012 GiB）回報 12 GiB 的 Reserved 區（AMD HyperTransport 洞）；算進去會變成 1 TiB 的 HHDM、2056 個分頁表（8 MiB）。最少 4 GiB 是因為 framebuffer 等 MMIO 通常在 4 GiB 以下、而且不一定出現在記憶體表裡。

**為什麼要 identity map**：寫入 CR3 的那一刻，CPU 下一條指令仍從 loader 所在的（低）位址抓。如果新的表沒有映射它，就會 page fault → 沒有 IDT → triple fault 重開機。它不能共用 HHDM 的表，因為 HHDM 設了 NX。kernel 一進來就在 `frame::mm::paging::init` 清掉 PML4[0..256]。

QEMU 512 MiB 的實際數量是 14 個表：PML4 1 + HHDM（PDPT 1 + PD 4）+ identity（PDPT 1 + PD 4）+ kernel（PDPT 1 + PD 1 + PT 1）。

## 8. 交棒

### 8.1 ExitBootServices 之後

`exit_boot_services` 回傳最終的記憶體表（它自己在呼叫前配置好緩衝區）。之後不能再配置記憶體、不能印字（UEFI console 也失效）。loader 只做兩件事：把記憶體表轉進已配置好的 `BootInfo`，然後跳。

UEFI 記憶體型別 → `bootinfo::MemoryKind`（UEFI Spec 2.10 Table 7-6）：

| UEFI | STELA |
|---|---|
| `CONVENTIONAL`、`BOOT_SERVICES_CODE/DATA` | `Usable` |
| `LOADER_CODE/DATA` | `KernelAndBoot`（kernel、分頁表、堆疊、BootInfo、loader 本身） |
| `ACPI_RECLAIM` | `AcpiReclaimable` |
| `ACPI_NON_VOLATILE` | `AcpiNvs` |
| `MMIO`、`MMIO_PORT_SPACE` | `Mmio` |
| 其他（runtime services、reserved、unusable…） | `Reserved` |

相鄰且同類的區段會合併（`BootInfo::push_region`）；QEMU 512 MiB 合併後是 35 個區段。

### 8.2 最後的組合語言

```asm
cli                       ; 沒有 IDT 前不能有中斷
mov ecx, 0xC0000080       ; IA32_EFER
rdmsr
or  eax, r11d             ; NXE（bit 11），CPU 不支援 NX 時 r11 = 0
wrmsr
mov rax, cr0
or  rax, 0x10000          ; CR0.WP：ring 0 也遵守唯讀頁
mov cr0, rax
mov cr3, r8               ; 新分頁表（從這裡起靠 identity map 繼續執行）
mov rsp, r9               ; 新堆疊（HHDM 位址）
xor ebp, ebp
push 0                    ; 假的返回位址：RSP ≡ 8 (mod 16)，符合 System V 函式入口
jmp r10                   ; kernel 的 e_entry；RDI = BootInfo 的 HHDM 位址
```

NX 支援由 `CPUID.80000001H:EDX[20]` 偵測（Vol. 2A "CPUID"）；沒有 NX 時不設 NXE，也不在任何項目設 bit 63（否則那是保留位元，會 page fault）。`BootInfo::nx_enabled` 告訴 kernel。`make test-qemu-min` 用 `-cpu Conroe,-nx` 驗證這條路。

## 9. `BootInfo` 格式（版本 1）

`#[repr(C)]`，所有位址都是**實體位址**（kernel 加上 `hhdm_base` 讀取）。列舉欄位存成 `u32` 原值，用 `kind()`／`format()` 解碼，壞值不會造成 UB。

| 位移 | 欄位 | 說明 |
|---|---|---|
| 0 | `magic: u64` | `"STELABI\0"` |
| 8 | `version: u32` | 1 |
| 12 | `region_count: u32` | `regions` 的有效項目數 |
| 16 | `hhdm_base: u64` | `0xFFFF_8000_0000_0000` |
| 24 | `hhdm_size: u64` | HHDM 涵蓋的位元組數 |
| 32 | `page_table_root: u64` | PML4 的實體位址 |
| 40 | `kernel_phys_base: u64` | kernel 映像的實體位址 |
| 48 | `kernel_size: u64` | 頁對齊 |
| 56 | `kernel_virt_base: u64` | `0xFFFF_FFFF_8000_0000` |
| 64 | `stack_phys_base: u64` | 初始堆疊（64 KiB） |
| 72 | `stack_size: u64` | |
| 80 | `rsdp_phys: u64` | 0 = 沒有 ACPI |
| 88 | `regions_truncated: u32` | 記憶體表超過 256 項 |
| 92 | `nx_enabled: u32` | |
| 96 | `framebuffer` | `phys_addr`、`size`、`width`、`height`、`pitch`、`format`（32 bytes） |
| 128 | `regions[256]` | 每項 24 bytes：`kind: u32`、`reserved: u32`、`phys_start: u64`、`page_count: u64` |

總大小 6,272 bytes。`bootinfo` 的 host 測試鎖住這些位移；改格式時要一起改 `VERSION`。

framebuffer：loader **不切換模式**，用 GOP 目前的模式（OVMF 預設 1280x800）。只接受 32 位元的 RGB／BGR；Bitmask 與 BltOnly 模式回報為「沒有 framebuffer」。

## 10. 測試

- `cargo test -p bootinfo -p elfload`：結構位移、列舉、區段合併；ELF 的各種錯誤輸入，以及（若已建置）真的 `kernel.elf`。
- `make test-qemu`：loader banner → `STELA kernel` → `selftest: OK`、QEMU exit code 33。
- `make test-qemu-min`：同上，`-cpu Conroe,-nx -m 256M`。

## 11. 已知限制與待辦

- 記憶體表最多 256 個區段（合併後）；超過時 `regions_truncated = 1`，kernel 目前只是不使用多出來的部分。
- 不支援 Bitmask 像素格式；不切換 GOP 模式。
- loader 自己（`LOADER_CODE`）與它讀進來又釋放的暫存不會被 kernel 回收（`KernelAndBoot` 一律視為使用中）。
- identity map 用掉的分頁表頁框（約 5 頁）在 kernel 移除映射後沒有回收。
- 只看 `ACPI2_GUID`／`ACPI_GUID`，RSDP 不驗證 checksum（P1 不解析 ACPI）。
- HHDM 只涵蓋 RAM（最少 4 GiB）：4 GiB 以上的 MMIO（例如 GPU 的 64 位元 BAR）不在 HHDM 裡，驅動要用動態區的 MMIO 映射 API（尚未有，見 frame.md §12）。
- 實機的解析度由韌體的 GOP 模式決定；要 2560x1440 得自己列舉並切換 GOP 模式（P8 顯示階段）。
