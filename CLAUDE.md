# CLAUDE.md

STELA：從零開始的獨立作業系統（Rust，x86-64，UEFI），繼承 Classic Mac OS 的設計哲學（簡單、易用、空間式檔案管理），加上現代作業系統的功能。前身 68k_HAL（C，已封存於 v1.0）的經驗與設計（例如 eHFS）會帶過來，但程式碼全部重寫；STELA **不依賴任何 Apple 元件**。「STELA」是名稱，不是縮寫，一律大寫。

## 回應規則

- 使用繁體中文回應；程式碼、註解、commit message 使用英文。
- 回應保持精簡：說明做了什麼、為什麼，不重複貼出整段程式碼。

## 基礎決策（2026-10-19）

- **語言**：Rust，stable 工具鏈（`rust-toolchain.toml` 釘版本），不用 nightly；需要 nightly 的功能出現時先討論。
- **架構**：framekernel——單一位址空間，所有 `unsafe` 關在 `frame/`（接觸硬體的框架 crate）；驅動、檔案系統、排程都用 safe Rust。
- **平台**：只做 x86-64、只支援 UEFI（沒有 32 位元、BIOS、Multiboot）。`arch/` 的邊界從第一天畫清楚，但不實作第二個架構。
- **開機**：自己的 UEFI loader（`boot/`，`uefi` crate），不用 Limine／GRUB。
- **使用者程式**：STELA 自己的 syscall ABI 是主要介面；Linux 等其他系統的相容是相容層，與 Classic 環境同級。
- **Classic 環境**：縮減版 68k_HAL（Musashi 直譯 + Mac 機器模型）跑在使用者空間行程，呈現方式類似 Windows 7 的 XP Mode；執行使用者自己的 ROM 與 System，是可選功能。
- **識別**：標誌是一塊石碑（stela），panic 畫面是裂開的石碑，檔案管理員叫 Mouseion，主題是古希臘；有畫面之後再做。

## 建置與執行

```bash
make            # 建置 loader 與 kernel → build/esp/（EFI/BOOT/BOOTX64.EFI、STELA/kernel.elf）
make run        # QEMU + OVMF 開機（一般 kernel），serial 輸出到 stdio
make test       # host 單元測試 + QEMU selftest + 最低硬體 selftest + 畫面檢查，修改後必須通過
make clippy     # cargo clippy（loader、kernel 端 crate 含 selftest feature、host 測試），警告視為錯誤
make fmt        # cargo fmt
make clean
```

- 需要的套件：`rustup`（目標 `x86_64-unknown-none`、`x86_64-unknown-uefi`，元件 `rust-src`、`llvm-tools`）、`qemu-system-x86`、`ovmf`（`/usr/share/OVMF/OVMF_CODE_4M.fd`）。
- cargo 的輸出在 `build/target/`（`.cargo/config.toml`）。
- CI：`.github/workflows/ci.yml` 在 push 到 main 與 PR 時執行 `cargo fmt --check`、`make clippy` 與 `make test`。
- `make test` 的組成：`test-host`（`HOST_CRATES` = bootinfo、elfload、frame 的 `cargo test`）、`test-qemu`（`selftest` feature 的 kernel，放在 `build/esp-test/`，selftest 印 `input: waiting` 時 `tools/qemu_test.py --send-keys` 經 QMP `send-key` 打 `stela` + Enter，檢查 `input: OK`、`selftest: OK` 與 QEMU exit code 33）、`test-qemu-min`（同一個 kernel，`-cpu Conroe,-nx -m 256M`：無 NX、無 invariant TSC）、`test-fb`（一般 kernel 開機、QMP 打 `h i ret`、看到 `uptime:` 後 screendump，`tools/fb_check.py` 檢查畫面有字）。`make run` 永遠跑沒有 selftest 的 kernel（打字會 echo，Enter 印 uptime）。
- QEMU 的 TCG 不支援 invariant TSC，`make test` 只測得到 tick 退路；TSC 路徑要在本機用 `-enable-kvm -cpu host,+invtsc` 手動驗證（`docs/time.md` §5.3）。

### 測試

- 不碰硬體的 crate（純邏輯：資料結構、檔案系統、協定解析）要能在 host 上以 `cargo test` 測試；硬體相關的 crate 把邏輯與硬體存取分開，讓邏輯的部分可以在 host 測。
- 需要硬體的測試放在 kernel 的 selftest（`kernel/src/selftest.rs`，`selftest` feature；QEMU + OVMF 開機，檢查 serial 輸出，結束時經 `isa-debug-exit` 回報 exit code）。
- 需要使用者自己的檔案（Mac ROM、磁碟映像）的測試只放在本機專用目標，不進 `make test` 或 CI。

## 目錄結構（規劃，隨進度更新）

```
boot/          UEFI loader（x86_64-unknown-uefi）：讀 kernel.elf、建分頁表、取得記憶體表與 framebuffer、交棒（docs/boot.md）
bootinfo/      loader 與 kernel 共用的 BootInfo 結構（no_std，host 測試）
elfload/       最小 ELF64 解析（只處理 PT_LOAD，host 測試）
frame/         框架 crate：所有 unsafe 的家，對外只提供 safe API（docs/frame.md）
  src/arch/x86_64/  架構相關：port I/O、MSR、GDT/TSS、IDT、256 個向量的 stub、8259、local/I/O APIC、PIT、i8042
  src/acpi/         RSDP/XSDT/MADT 解析（tables.rs 純邏輯，host 測試）（docs/irq.md）
  src/irq.rs        向量配置、irq::register、分派與 EOI（docs/irq.md）
  src/cpu.rs        per-CPU 區（GS_BASE）
  src/time/         APIC timer 1000 Hz、now_ns、sleep_ms（docs/time.md）
  src/input/        鍵盤解碼、KeyCode、輸入事件佇列（docs/keyboard.md）
  src/mm/           頁框配置（bitmap）、kernel 分頁、MMIO 映射、heap
  src/console/      字型、文字主控台、framebuffer、panic 畫面
kernel/        kernel 主體（safe Rust，x86_64-unknown-none，linker.ld）：之後的記憶體、行程、排程、VFS、syscall
drivers/       裝置驅動（safe Rust，透過 frame 的 API 碰硬體）（尚未建立）
third_party/   第三方資料（spleen/：Spleen 字型，BSD-2-Clause）
docs/          設計文件（每個元件完成後寫 docs/<元件>.md）
tools/         開發工具與測試腳本（qemu_test.py、fb_check.py、font_to_rs.py）
```

## 程式風格

- 每個新的 `.rs` 檔開頭加上授權標頭：
  ```rust
  // Copyright (C) 2026 YuanChi Hsieh
  // SPDX-License-Identifier: GPL-3.0-or-later
  ```
- `cargo fmt` 的預設格式；`cargo clippy` 零警告（`make clippy`）。
- `unsafe` 只能出現在 `frame/`，以及 `boot/` 的兩處（`exit_boot_services` 與跳進 kernel 的 inline asm；`boot/` 用 `#![deny(unsafe_code)]` 加局部 `#[allow(unsafe_code)]`）；其他 crate 以 `#![forbid(unsafe_code)]` 禁止。每個 `unsafe` 區塊前要有 `// SAFETY:` 註解說明為什麼安全。
- 不用 `unwrap()`／`expect()` 處理可能失敗的情況；錯誤用 `Result` 傳回。無法恢復的錯誤用 `panic!`（kernel 的 panic handler 負責顯示）。
- 與既有程式碼保持一致；不要只為了風格而修改無關的程式碼。

## Commit 格式

`<crate 或路徑>: <小寫開頭的簡短描述>`，例如：

```
boot: print the banner on the UEFI console
frame/paging: map the higher half
```

## 工作規則

- 直接在 `main` 上開發，**不建立 branch 或 PR**（除非使用者要求）。一個任務對應一個小 commit。
- **不要自行 commit 或 push**：完成並通過測試後提出 commit 訊息，使用者審核 diff 確認後才執行。
- **不要猜使用者的想法，有問題就問**：需求不明確、有多種合理解讀，或需要使用者做決定時，先提問再動手。不要自行補上使用者沒說的需求。
- 一次只做被要求的任務；發現其他問題時列出來，不要順便修改。
- 涉及多個檔案或架構的變更，先提出計畫，等使用者確認後再實作。
- 修改後必須執行 `make test` 並通過；新功能要附上測試。
- 硬體相關的數值（暫存器、descriptor 位元、UEFI 結構）要註明出處（Intel SDM、UEFI Specification、OSDev Wiki），不確定時明說。
- 第三方 crate：只用授權相容 GPL-3.0-or-later 的（MIT／Apache-2.0／BSD 等）；不引入 GPL-2.0-only 的程式碼。每加一個相依先說明用途與授權。
- 不要加入任何 Apple ROM、System Software 或其他受版權保護的內容。使用者的 ROM 與磁碟映像（`~/mac/`）只能讀取，不可複製進 repo、不可 commit、不可進 `make test` 或 CI。
- 68k_HAL 的程式碼在 `~/文件/68k_HAL`（唯讀的參考）；可以參考它的設計與文件，不要複製 C 程式碼進來。

## 開發模式

依任務性質分成兩種模式。不確定屬於哪一種時，先問使用者。

### 修正模式：既有程式碼的 bug 修正與整理

Claude 依照上方工作規則直接實作。

### 協作模式：新元件

使用者負責架構決策，Claude 負責說明原理、列出決策點並實作。使用者可以設計架構，但不熟悉底層知識，所以**理解優先於進度**。

1. **概念**：先說明這個元件要解決什麼問題、需要哪些背景知識。
2. **決策點**：每一步列出需要決定的事項（選項、取捨、Claude 的建議與理由、潛在問題），由使用者決定；不要替使用者決定架構。
3. **一次一個函式**：使用者決定後由 Claude 實作，並說明：
   - 用途：它在整個元件中負責什麼、誰會呼叫它
   - 原理：背後的硬體或演算法原理，附上出處（Intel SDM 章節、UEFI 規格、OSDev Wiki 頁面）
   - 關鍵的程式碼行在做什麼（Rust 的語法或慣用寫法第一次出現時也要解釋）
4. **確認**：使用者理解並確認後，才進行下一個函式。不要一次寫完整個元件。
5. **測試**：每個函式都要有對應的測試，並且通過 `make test`。
6. **紀錄**：元件完成後，把原理與設計決策整理到 `docs/<元件名稱>.md`，作為之後查閱的知識庫。

解釋方式：先用比喻再說明實際運作；專有名詞第一次出現時要解釋；多用 ASCII 圖和本專案的實際數字；一次一個概念，保持簡短。

協作模式的元件（之後新增的加入此清單）：
- P1（自動排程一次做完，使用者已審核（2026-10-20））：開機載入器（`docs/boot.md`）、frame 的 GDT/IDT/分頁/記憶體/主控台（`docs/frame.md`）
- P2（自動排程一次做完，使用者已審核（2026-10-20））：ACPI MADT、APIC 與中斷、per-CPU、鎖（`docs/irq.md`）、計時器與時鐘（`docs/time.md`）、PS/2 鍵盤與輸入事件（`docs/keyboard.md`）

## 已知問題（尚未修正）

- 只有 BSP，沒有啟動 AP（P10）；只支援 xAPIC，沒有 8259 退路（沒有 APIC 或 MADT 就 panic）；LINT1（NMI）遮蔽。
- MMIO 映射的虛擬位址只增不減、不回收；中斷處理程式表用一把 SpinLock（SMP 時可改 atomic）。
- 計時器只有週期模式 1000 Hz；依賴 8254 PIT 校準；沒有 invariant TSC 時 `now_ns()` 只有 1 ms 解析度，且長時間關中斷（例如主控台捲動）會少算 tick。
- 鍵盤：只有 PS/2（i8042）、US 配置；沒有數字鍵台、LED、按住重複、滑鼠；新電腦可能沒有 i8042（要等 P8 的 USB HID）。
- 沒有設定 PAT：framebuffer 是 write-back 而不是 write-combining，捲動要讀 framebuffer，實機上會慢。
- loader 不切換 GOP 模式（OVMF 預設 1280x800）；不支援 Bitmask 像素格式。
- 記憶體表合併後最多 256 個區段（`regions_truncated`）；`KernelAndBoot`（含 loader 自己）與 identity map 用過的分頁表不回收。
- kernel heap 固定 4 MiB；空掉的中間分頁表不回收；`alloc_contiguous` 是線性 first-fit。
