# STELA 架構規劃

決定日期：2026-10-19／20（使用者「照建議」）。這份文件是之後每個階段的依據；改變任何決定時先改這裡。

## 1. 三個總原則

1. **繼承 Classic Macintosh 的架構風格**：組織原則與使用者可見的概念，不是實作方式。
2. **極度精簡**：以 Pentium 4 等級的效能為基準（見 §3），單核心必須順。
3. **系統與使用者空間完全隔離**（iOS 式）：系統唯讀、整體替換；應用程式與使用者資料在另一個磁碟區；應用程式彼此隔離、不能改系統。

### 1.1 繼承什麼、不繼承什麼

| 繼承 | 不繼承 |
|---|---|
| 以「Manager」切分子系統（File、Event、Window…各有小而明確的 API） | 合作式多工 |
| 應用程式是事件驅動（從事件佇列取事件） | 沒有記憶體保護 |
| 磁碟區各自為根；檔案有類型與持久 ID | 全系統共用一個堆積 |
| 應用程式自帶資源（圖示、字串、視窗版面） | 使用者打補丁改系統（INIT／extension） |
| 空間式檔案管理（Mouseion）；單一使用者 | |

## 2. 分層

```
 使用者空間
 ┌──────────────────────────────────────────────────────────────┐
 │ Mouseion │ 原生應用程式 │ Classic 環境 │ Linux 相容層的程式    │
 │          │             │ (Musashi, C) │                     │
 ├──────────┴─────────────┴──────────────┴─────────────────────┤
 │ 視窗伺服器 / UI 工具箱 (Rust)                                  │
 ├──────────────────────────────────────────────────────────────┤
 │ libstela：syscall 包裝、執行期 (Rust)   │ Linux 相容層 (musl) │
 └──────────────────────────────────────────────────────────────┘
          ▲ STELA syscall ABI                 ▲ Linux syscall 翻譯
 kernel（單一位址空間，safe Rust）
 ┌──────────────────────────────────────────────────────────────┐
 │ syscall 分派 │ 行程/執行緒/排程 │ IPC │ VFS │ 記憶體 (VMM/heap) │
 ├──────────────────────────────────────────────────────────────┤
 │ drivers/：PCI、儲存 (AHCI/NVMe)、USB (xHCI/HID)、網路、顯示、音效 │
 │ fs/：eHFS (原生)、FAT (ESP)、ISO9660                          │
 ├──────────────────────────────────────────────────────────────┤
 │ frame/（所有 unsafe）：分頁、中斷、APIC、timer、context 切換、   │
 │        syscall 進入點、port I/O、MMIO、SMP 啟動、主控台          │
 └──────────────────────────────────────────────────────────────┘
 boot/：UEFI loader → bootinfo → kernel
```

## 3. 硬體基準（「Pentium 4 等級」）

真正的 Pentium 4 沒有 UEFI，所以這是效能與指令集的基準，不是目標硬體。

| 項目 | 值 |
|---|---|
| 指令集 | x86-64-v1（只有 SSE2；不用 SSSE3/SSE4/AVX）；編譯目標維持預設 `x86-64` |
| CPU 功能 | NX、1 GiB 大頁、x2APIC、TSC deadline 等一律 CPUID 偵測，缺了就退回 |
| 核心數 | 單核心就必須順；SMP 是加分 |
| 記憶體 | 以 512 MiB 設計，256 MiB 要能開機 |
| 圖形 | 純軟體繪圖，無 GPU 加速 |
| 韌體 | 只支援 UEFI |

可量測目標：kernel 映像 ≤ 2 MiB；無 KVM 的 QEMU 開機到桌面 ≤ 5 秒；桌面閒置時 kernel + 視窗伺服器 + Mouseion ≤ 64 MiB；相依 crate 以「必要」為原則。`make test` 要加一條最小 CPU 模型（例如 QEMU `-cpu Conroe`）、不開 KVM、`-m 256M` 的開機測試。

## 4. 架構決定

- **A 執行模型**：行程有獨立位址空間；有執行緒；有 kernel 執行緒。理由：Classic 環境與 Linux 相容層跑 C 程式，硬體隔離是唯一防線。
- **B 多核心**：資料結構從第一天假設 SMP（per-CPU 區域、spin lock 帶中斷關閉），但先只啟動 BSP，AP 啟動放後面。
- **C IPC**：kernel 提供 port 與固定大小的小訊息（可夾帶 handle），大資料走共享記憶體。事件佇列是 per-process 的 port。
- **D syscall ABI**：handle + 權限位元（不是整數 fd）；`syscall` 指令；Linux 相容層把 fd 對應到 handle 表。
- **E 檔案系統**：磁碟區各自為根；每個檔案有持久 ID；擴充屬性；路徑分隔符 `/`；大小寫保留但不敏感。eHFS 延伸。
- **F 驅動**：kernel 內 safe Rust，只透過 frame 的 API 碰 MMIO／DMA；大型驅動（GPU）之後可例外放使用者空間。驅動永遠不開放第三方。
- **G 圖形**：kernel 只提供 framebuffer／顯示模式與輸入事件；視窗伺服器與繪圖在使用者空間；髒矩形合成、不做全畫面 alpha、不做動畫與透明效果。
- **H 相容層**：Linux 只做 syscall 翻譯，先支援 musl 靜態程式；Classic 環境以 Linux 相容層的程式形式存在，不另養 C 執行期。
- **I 安全**：單使用者 + handle 式能力 + 應用程式沙箱。

### 4.1 應用程式（3-A、3-C）

- 每個應用程式至少一個執行緒，可以自己再開；事件迴圈從自己的 port 取事件。
- 應用程式是一個 bundle 目錄，拖進 Data 就算安裝；執行時只能寫自己的容器；要碰使用者的文件，只能透過 Mouseion 或開檔對話框交出 handle（powerbox）。文件屬於使用者，任何應用程式可開。

### 4.2 磁碟區配置（3-B）

```
ESP      FAT      loader + kernel.elf（只有開機需要的東西）
System   eHFS 唯讀  系統程式、視窗伺服器、Mouseion、字型、驅動資料
                   整個磁碟區是一個映像，更新 = 換掉整個映像（A/B 兩份）
Data     eHFS 可寫  應用程式、使用者的文件、設定、每個應用程式的容器
```

kernel 放 ESP（loader 不必懂 eHFS）。Mouseion 顯示 System 磁碟區但唯讀。

### 4.3 延伸機制（3-D）

沒有 INIT。第三方只能提供使用者空間的「服務」行程（有 IPC 介面，沒有 kernel 權限）。

## 5. 階段

```
P1  boot + frame 基礎 + 主控台
P2  中斷與時間：APIC、timer、鍵盤、per-CPU
P3  行程：位址空間、執行緒、排程、context 切換、syscall 進入點、第一個使用者程式
P4  IPC + handle 表                            （C、D、I 落地）
P5  儲存與 VFS：PCI、AHCI/NVMe、FAT、eHFS、VFS   （E 落地）
P6  使用者空間：libstela、init、shell、ELF 載入
P7  Linux 相容層（musl 靜態）                   （H）
P8  顯示與輸入：USB HID、視窗伺服器、UI 工具箱   （G）
P9  Mouseion + 識別（石碑、panic 畫面、圖示）
P10 Classic 環境、網路、音效、SMP 啟動 AP
```

## 6. 待修（P1 審核時）

- P1 無條件設定 `IA32_EFER.NXE`；要改成 CPUID 偵測（`CPUID.80000001H:EDX[20]`），沒有 NX 就不設 NX 位元。
