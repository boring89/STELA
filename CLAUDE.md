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
- **識別**：標誌是一塊石碑（stela），panic 畫面是裂開的石碑，檔案管理員叫 Mouseion，主題是古希臘；P9 做了機制與**草稿**圖示（`assets/`、`docs/identity.md`），正式的圖由使用者畫。

## 建置與執行

```bash
make            # 建置 loader、kernel、使用者程式與 initrd → build/esp/（EFI/BOOT/BOOTX64.EFI、STELA/kernel.elf、STELA/initrd.tar）
make musl       # 複製 68k_HAL 已下載的 musl-1.2.5.tar.gz、檢查 SHA-256、建在 build/musl/（MIT；只用來建 Linux 測試程式）
make linux-tests # linux/tests/*.c → build/user/linux/（musl-gcc -static），打包進 initrd 的 linux/
make user       # 建置 user/ 的程式（hello、spin、fault、ipc_server、ipc_client、echo、fstest、proctest、init、shell、windowserver、terminal、echo_win、mouseion、viewer）→ build/user/
make initrd     # 把 build/user/ 排成 build/initrd-root/（linux/、Terminal.app 等 bundle）再打包成 build/initrd.tar（GNU tar --format=ustar）
make run        # QEMU + OVMF 開機（一般 kernel：init → shell，提示符 Initrd:/> ），serial 輸出到 stdio
make test       # host 單元測試 + QEMU selftest + 最低硬體 selftest + 桌面、儲存、shell、關機、Linux、USB、panic 畫面、Mouseion 測試，修改後必須通過
make ehfs-tools # host 的 mkfs.ehfs、fsck.ehfs、ehfs-cp、ehfs-ls → build/host/ehfs/
make assets     # assets/*.ppm、*.pbm → assets/generated/（Rust 資料；換圖後執行）
make draft-assets # 重畫草稿圖示（會覆蓋 assets/；使用者的圖放進去之後不要跑）
make test-ehfs-compat  # 本機專用：與 68k_HAL 的 eHFS 工具交叉檢查（不在 make test／CI）
make clippy     # cargo clippy（loader、kernel 端 crate 含 selftest feature、使用者程式、host 測試），警告視為錯誤
make fmt        # cargo fmt
make clean
```

- 需要的套件：`rustup`（目標 `x86_64-unknown-none`、`x86_64-unknown-uefi`，元件 `rust-src`、`llvm-tools`）、`qemu-system-x86`、`ovmf`（`/usr/share/OVMF/OVMF_CODE_4M.fd`）、`mtools`、`gdisk`（測試映像）、`gcc`（建 musl 與 Linux 測試程式）。musl 的 tarball 不下載：預設從 `~/文件/68k_HAL/third_party/downloads/musl/` 複製（`MUSL_TARBALL=` 可改），CI 從 musl.libc.org 抓一次並快取（`third_party/README.md`）。
- cargo 的輸出在 `build/target/`（`.cargo/config.toml`）。
- CI：`.github/workflows/ci.yml` 在 push 到 main 與 PR 時執行 `cargo fmt --check`、`make clippy` 與 `make test`。
- `make test` 的組成：`test-host`（依賴 `make initrd` 與 `host-images`（mtools／sgdisk 做的 FAT、GPT 映像）；`HOST_CRATES` = abi、block、bootinfo、drivers、ehfs、ehfs-tools、elfload、frame、libstela、toolbox 的 `cargo test`，加上 `HOST_LIB_CRATES` = kernel、windowserver、terminal、mouseion、viewer 的 `cargo test --lib`；另外檢查 `assets/generated/` 與圖檔一致）、`test-qemu`（`selftest` feature 的 kernel，放在 `build/esp-test/`，selftest 印 `input: waiting` 時 `tools/qemu_test.py --send-keys` 經 QMP `send-key` 打 `stela` + Enter、印 `mouse: waiting` 時 `--mouse` 經 `input-send-event` 移動 PS/2 滑鼠並按左鍵，依序檢查 `input: OK`、`mouse: OK`、內建使用者程式與 initrd 程式（hello、子執行緒、fault、spin）的輸出、`ipc_client: OK`、`proctest: OK`、`selftest: OK` 與 QEMU exit code 33）、`test-qemu-min`（同一個 kernel，`-cpu Conroe,-nx -m 256M`：無 NX、無 invariant TSC、無 SMEP；也接 AHCI 測試磁碟跑 fstest）、`test-window`（一般 kernel 開機進桌面：視窗伺服器 + terminal + shell；經 QMP 打 `uptime`、`run echo_win`、在 echo_win 打一行，screendump 後 `tools/fb_check.py --expect-rect/--expect-text` 檢查桌面、選單列、兩個視窗的標題列與文字）、`test-storage`（`make test-images` 在 `build/images/` 造 AHCI（GPT：FAT ESP + eHFS Data）與 NVMe（4Kn，eHFS Archive）兩顆磁碟，selftest 開機兩次檢查持久性與 fstest，最後 host 的 `fsck.ehfs` 檢查）、`test-shell`（一般 kernel + AHCI 測試磁碟，`qemu_test.py --type` 依序打 shell 指令並檢查輸出）、`test-shutdown`（一般 kernel + AHCI 測試磁碟：shell 打 `shutdown`，QEMU 自己結束（status 0），host 的 fsck 確認正常卸載；再開機打 `reboot`）、`test-linux`（一般 kernel + AHCI 測試磁碟：shell 依序 `run linux/hello a b`、`files`、`memory`、`time`、`nosys`、`tty`（打一行字）並檢查輸出與 exit code）、`test-panic`（selftest kernel + 多一個 `panic-test` 檔的 initrd：故意 panic，截圖檢查裂碑的 panic 畫面）、`test-mouseion`（一般 kernel + 乾淨的 AHCI 測試磁碟：`qemu_test.py --click` 雙擊 Data、welcome.txt（viewer）、Initrd、Terminal.app，截圖檢查卷宗圖示與新視窗）、`test-usb`（一般 kernel、`-machine q35,i8042=off`、`qemu-xhci` + usb-kbd + usb-mouse + usb-hub（後面一個 usb-kbd）：USB 鍵盤打字到 shell、滑鼠報告、`qemu_test.py --qmp` 的 `device_del`／`device_add` 熱插拔 root port 與 hub port）。shell、Linux、關機與 USB 測試都經過桌面（terminal 把 shell 的輸出鏡像到 serial）。`qemu_test.py --log FILE` 把 serial 輸出存檔（`make test-qemu` 寫 `build/test-qemu.log`）。`make run` 永遠跑沒有 selftest 的 kernel（開機畫面的石碑 → init 啟動視窗伺服器與 Mouseion，Mouseion 打開一個 terminal，裡面是 shell；`help` 列出指令，`run Echo.app` 開一個視窗；shell 的輸出也鏡像到 serial）。
- QEMU 的 TCG 不支援 invariant TSC，`make test` 只測得到 tick 退路；TSC 路徑要在本機用 `-enable-kvm -cpu host,+invtsc` 手動驗證（`docs/time.md` §5.3）。

### 測試

- 不碰硬體的 crate（純邏輯：資料結構、檔案系統、協定解析）要能在 host 上以 `cargo test` 測試；硬體相關的 crate 把邏輯與硬體存取分開，讓邏輯的部分可以在 host 測。
- 需要硬體的測試放在 kernel 的 selftest（`kernel/src/selftest.rs`，`selftest` feature；QEMU + OVMF 開機，檢查 serial 輸出，結束時經 `isa-debug-exit` 回報 exit code）。
- 需要使用者自己的檔案（Mac ROM、磁碟映像）的測試只放在本機專用目標，不進 `make test` 或 CI。
- 測試映像只放在 `build/` 底下；造映像與 eHFS 的工具拒絕一般檔案以外的目標（不可對實體磁碟操作）。

## 目錄結構（規劃，隨進度更新）

```
abi/           kernel 與使用者程式共用的 syscall ABI：編號、錯誤碼、位址配置、Handle／Rights／Message、key、DirEntry／Stat／VolumeInfo（no_std，host 測試）（docs/syscall.md、docs/ipc.md、docs/vfs.md）
block/         BlockDevice trait、Partition、GPT、CRC-32、MemDisk（no_std，host 測試）（docs/storage.md）
boot/          UEFI loader（x86_64-unknown-uefi）：讀 kernel.elf 與 initrd.tar、建分頁表、取得記憶體表與 framebuffer、交棒（docs/boot.md）
bootinfo/      loader 與 kernel 共用的 BootInfo 結構（版本 3：含 boot.cfg 的 cmdline 與 AP trampoline 頁，no_std，host 測試）
elfload/       最小 ELF64 解析（只處理 PT_LOAD，host 測試）
ehfs/          eHFS（與 68k_HAL 的 eHFS v1 格式相容；no_std，host 測試）（docs/ehfs.md）
frame/         框架 crate：所有 unsafe 的家，對外只提供 safe API（docs/frame.md）
  src/arch/x86_64/  架構相關：port I/O、MSR、GDT/TSS、IDT、256 個向量的 stub、8259、local/I/O APIC、PIT、i8042、CMOS RTC（cmos.rs）、
                    context 切換與 enter_user（context.rs）、syscall 進入點與 MSR（syscall.rs）
  src/acpi/         RSDP/XSDT/MADT/MCFG 解析（tables.rs）、FADT（fadt.rs）、DSDT 的 \_S5 掃描（aml.rs）（純邏輯，host 測試）（docs/irq.md、docs/pci.md、docs/acpi.md）
  src/power.rs      ACPI 關機（S5）、重新開機（reset 暫存器 → 8042 → triple fault）（docs/acpi.md）
  src/pci.rs        PCI 設定空間的 legacy ports（docs/pci.md）
  src/irq.rs        向量配置、irq::register、分派與 EOI（docs/irq.md）
  src/cpu.rs        per-CPU 區（GS_BASE；kernel_rsp、need_resched、目前的 context）
  src/task/         TaskContext、switch_to、kernel 堆疊、enter_user、搶佔與使用者例外的回呼、FS base（docs/process.md、docs/linux.md）
  src/syscall.rs    SyscallFrame、set_handler（docs/syscall.md）
  src/time/         APIC timer 1000 Hz、now_ns、sleep_ms、RTC 解碼（rtc.rs）與 wall_clock_ns（docs/time.md）
  src/input/        鍵盤解碼、PS/2 滑鼠封包（mouse.rs）、輸入事件佇列、notify hook（docs/keyboard.md、docs/window.md）
  src/mm/           頁框配置（bitmap）、kernel 分頁、MMIO 映射、heap（16 MiB）、行程位址空間（user.rs；含 map_device）、共享記憶體（shared.rs）、DMA（dma.rs）、可給使用者映射的裝置記憶體（device.rs）、initrd()
  src/console/      字型、文字主控台、framebuffer（交給視窗伺服器：FramebufferDevice、hand_over）、panic 畫面（docs/display.md）
kernel/        kernel 主體（safe Rust，x86_64-unknown-none，linker.ld）；lib（host 測試純邏輯）+ bin（入口與 selftest）
  src/task/         Thread、排程器、run queue／睡眠清單／WaitQueue（queue.rs，host 測試）、block_until、Process、wait_process（docs/process.md）
  src/object/       handle 表（handle.rs）、映射區配置（regions.rs）、行模式（ldisc.rs）（host 測試）、Port、Tty（tty.rs）、KernelObject（docs/ipc.md、docs/linux.md）
  src/console.rs    console 服務；src/input.rs  input-router（docs/ipc.md）
  src/block.rs      磁碟 probe；src/fs/  區塊快取（cache.rs）、FAT 唯讀（fat.rs）（docs/storage.md）
  src/vfs/          卷宗、資料夾／檔案 Node、eHFS、FAT、Initrd（資料夾樹）與 Tmp（tmpfs.rs，記憶體）後端、掛載（docs/vfs.md）；src/fsyscall.rs  檔案 syscall
  src/linux/        Linux 人格：號碼（nr）、errno、fd 表、合成根目錄的路徑、初始堆疊（host 測試）；分派（sys）、檔案（files）、brk／mmap（memory）（docs/linux.md）
  src/psyscall.rs   行程 syscall（ProcessSpawn 等）、SysInfo、SystemPower（docs/userland.md、docs/acpi.md）
  src/power.rs      卸載卷宗後關機／重新開機（docs/acpi.md）
  src/syscall.rs    syscall 分派（docs/syscall.md、docs/ipc.md）
  src/initrd.rs     ustar 解析與資料夾樹（host 測試）
  src/program.rs    從檔案（經 VFS）載入 ELF、人格判定（STELA note）、參數訊息或 Linux 初始堆疊、建立行程
user/          使用者程式（x86_64-unknown-none，user.ld 連結在 0x40_0000）（docs/syscall.md §5）
  libstela/         執行期：entry!（含參數）、STELA ELF note、syscall 包裝、ipc、fs、process、tty、heap（global allocator）、path、line、print!／println!（經 console 服務）、ThreadStack/spawn、panic handler（docs/userland.md）
  hello/ spin/ fault/ ipc_server/ ipc_client/ fstest/ proctest/  initrd 裡的測試程式；echo/  鍵盤 echo（文字模式）
  windowserver/     視窗伺服器：compose.rs（純邏輯，host 測試）、server.rs（docs/window.md）
  toolbox/          UI 工具箱 lib：draw、window、event、menu、control（docs/toolbox.md）
  terminal/         視窗版的終端機：grid.rs（純邏輯，host 測試）、tty master、shell（docs/window.md §7）
  echo_win/         工具箱的範例（Echo.app）
  mouseion/         空間式檔案管理員：model.rs（純邏輯，host 測試）、桌面、資料夾視窗、拖放（docs/mouseion.md）
  viewer/           Viewer.app：唯讀的文字檢視
  init/ shell/      開機後的 init（有 framebuffer 就跑 windowserver mouseion）與 shell（docs/userland.md）
linux/tests/   Linux 人格的測試程式（C，musl 靜態連結）（docs/linux.md）
drivers/       裝置驅動（safe Rust，透過 frame 的 API 碰硬體）：PCI、AHCI、NVMe、USB（usb/：描述子、HID、TRB 純邏輯 host 測試；xhci.rs）、sync（排程 hooks、SleepLock、SleepMutex）（docs/pci.md、docs/storage.md、docs/usb.md）
third_party/   第三方資料（spleen/：Spleen 字型，BSD-2-Clause；README.md：清單與 musl 的 SHA-256）
docs/          設計文件（每個元件完成後寫 docs/<元件>.md）
assets/        圖示（PPM／PBM，**草稿**）與產生的 Rust 資料（generated/）（docs/identity.md）
tools/         開發工具與測試腳本（qemu_test.py、fb_check.py、font_to_rs.py、make_test_images.py、make_test_disk.py、gen_casefold.py、ehfs_compat.py、make_draft_assets.py、img_to_rs.py）
  ehfs/             host 工具 crate：mkfs.ehfs、fsck.ehfs、ehfs-cp、ehfs-ls
```

## 程式風格

- 每個新的 `.rs` 檔開頭加上授權標頭：
  ```rust
  // Copyright (C) 2026 YuanChi Hsieh
  // SPDX-License-Identifier: GPL-3.0-or-later
  ```
- `cargo fmt` 的預設格式；`cargo clippy` 零警告（`make clippy`）。
- `unsafe` 只能出現在 `frame/`，以及 `boot/` 的兩處（`exit_boot_services` 與跳進 kernel 的 inline asm；`boot/` 用 `#![deny(unsafe_code)]` 加局部 `#[allow(unsafe_code)]`）、`user/libstela`（`syscall` 指令、`ThreadStack`、`ipc::Mapping` 的複製、`heap` 與 `sync::SpinMutex`，使用者程式唯一需要 unsafe 的地方）、`user/fault`（故意讀位址 0 的一處 inline asm）；其他 crate 以 `#![forbid(unsafe_code)]` 禁止。每個 `unsafe` 區塊前要有 `// SAFETY:` 註解說明為什麼安全。
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
- P3（自動排程一次做完，使用者已審核（2026-10-20））：執行緒、排程、context 切換、位址空間、使用者模式、initrd（`docs/process.md`）、syscall ABI 與 libstela（`docs/syscall.md`）
- P4（自動排程一次做完，使用者已審核（2026-10-20））：IPC 與 handle（`docs/ipc.md`）
- P5（自動排程一次做完，使用者已審核（2026-10-20））：PCI、儲存、eHFS、VFS（`docs/pci.md`、`docs/storage.md`、`docs/ehfs.md`、`docs/vfs.md`）
- P6（自動排程一次做完，使用者已審核（2026-10-20））：libstela、init、shell（`docs/userland.md`）
- P6.5（自動排程一次做完，使用者已審核（2026-10-20））：RTC 與關機（`docs/time.md` §5.5、`docs/acpi.md`）
- P7（自動排程一次做完，使用者已審核（2026-10-20））：Linux 相容層（`docs/linux.md`）
- P8（自動排程一次做完，使用者已審核（2026-10-20））：USB、顯示、視窗伺服器、工具箱（`docs/usb.md`、`docs/display.md`、`docs/window.md`、`docs/toolbox.md`）
- P9（自動排程一次做完，使用者已審核（2026-10-20））：Mouseion 與識別（`docs/mouseion.md`、`docs/identity.md`）

## 已知問題（尚未修正）

- 只有 BSP，沒有啟動 AP（P10）；只支援 xAPIC，沒有 8259 退路（沒有 APIC 或 MADT 就 panic）；LINT1（NMI）遮蔽。
- MMIO 映射的虛擬位址只增不減、不回收；中斷處理程式表用一把 SpinLock（SMP 時可改 atomic）。
- 計時器只有週期模式 1000 Hz；依賴 8254 PIT 校準；沒有 invariant TSC 時 `now_ns()` 只有 1 ms 解析度，且長時間關中斷（例如主控台捲動）會少算 tick。
- 鍵盤與滑鼠：PS/2（i8042，兩個 port）與 USB HID boot protocol；US 配置；沒有數字鍵台、LED、按住重複、滾輪的用途、滑鼠加速度（docs/keyboard.md、docs/usb.md §11）。
- PAT entry 4 = WC：視窗伺服器的 framebuffer 映射是 write-combining；kernel 自己的文字主控台仍經 HHDM（write-back，捲動要讀 framebuffer），同一塊記憶體有兩種別名（docs/display.md §8）。
- loader 選面積最大的橫向 GOP 模式（OVMF：2560×1600）；不支援 Bitmask 像素格式；HiDPI scale 固定 1。
- 記憶體表合併後最多 256 個區段（`regions_truncated`）；`KernelAndBoot`（含 loader 自己）與 identity map 用過的分頁表不回收。
- kernel heap 固定 16 MiB（P5 從 4 MiB 加大）；空掉的中間分頁表不回收；`alloc_contiguous` 是線性 first-fit。
- 排程：單一等級 round-robin（10 ms 時間片），`priority` 只存不用；`block`/`wake` 的競賽只靠「單顆 CPU + 關中斷」，SMP 時要重做；沒有 tickless；睡眠解析度 1 ms。
- 位址空間只有 4 KiB 頁、沒有需求分頁／mmap；kernel 半部不能新增 PML4 項目（行程的 PML4 是建立時複製的）；使用者 FPU 只有 FXSAVE（無 XSAVE/AVX）；沒有 SMAP（kernel 不直接解參考使用者指標）。
- NMI／#MC 若剛好落在 `swapgs` 前後或 `sysret` 前會用錯 GS／堆疊（它們沒有 IST）；行程表的 `Exited` 狀態永遠不清。
- 使用者空間：沒有 Thread 物件；shell 沒有管線、重新導向、背景工作、歷史；heap 不還記憶體（docs/userland.md §9）。
- Mouseion：拖曳沒有影子、不能多選、沒有刪除與改名、資料夾圖示位置不存、開機不重開上次的視窗；圖示全是草稿（docs/mouseion.md §8、docs/identity.md）。
- 視窗：client 以 id 辨識（可冒用）、沒有 port set；拖外框不即時移動；選單一層；client 結束最多 1 秒後才被發現；terminal 沒有 ANSI 顏色與捲回（docs/window.md §10）。
- USB：只有第一個 xHCI、USB 2.0 hub 的 port 輪詢、HID boot protocol；沒有儲存、音訊、電源管理；stall 不復原；未在實機驗證（docs/usb.md §11）。
- 儲存：一次一個請求（AHCI 一個 slot、NVMe 每次一個 4 KiB 區塊）；MSI 一個 message、MSI-X 只用 entry 0、向量不回收；FAT 唯讀、沒有 FAT12、不讀 MBR；RTC 只在開機讀一次、當成 UTC（docs/time.md §5.5）；`DirRead` 每次重新列出（O(n²)）；一個卷宗一次一個操作（docs/storage.md §12、docs/vfs.md §8）。
- 電源：只認常數 Package 的 `\_S5`（沒有 AML 直譯器、不看 SSDT、不跑 `\_PTS`）；`SystemPower` 任何程式都能呼叫；關機前不通知程式（docs/acpi.md §9）。
- Linux 人格：只有靜態非 PIE 的 musl 程式；單執行緒（沒有 clone、fork、execve、pipe、訊號遞送）；Linux 程式不受能力限制（看得到所有卷宗）；cwd 只從 `[3]` 的卷宗根開始；`getrandom` 不安全；`PROT_NONE` 可讀（docs/linux.md §13）。
- IPC：沒有 port set；訊息夾帶自己 port 的 handle 會形成 Arc 循環；送滿／表滿的競爭邊界會放掉夾帶的 handle；任何有 port SEND 的程式都能 `InputAttach` 搶鍵盤；共享記憶體是 eager（`docs/ipc.md` §12）。
