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
- **Classic 環境（P10）**：`classic/` 是 STELA 原生程式（Rust 包裝 + C 核心經 FFI，不是 Linux 相容程式；`docs/architecture.md` H 的註記）。
- **識別**：標誌是一塊石碑（stela），panic 畫面是裂開的石碑，檔案管理員叫 Mouseion，主題是古希臘；P9 做了機制與**草稿**圖示（`assets/`、`docs/identity.md`），正式的圖由使用者畫。

## 建置與執行

```bash
make            # 建置 loader、kernel、使用者程式與 initrd → build/esp/（EFI/BOOT/BOOTX64.EFI、STELA/kernel.elf、STELA/initrd.tar）
make chewing    # P13：libchewing 0.5.1（LGPL-2.1，從 ~/stela-ports/sources/ 複製或下載，檢查 SHA-256）解開到 build/chewing/src，host 的 init_database 造字典 → build/chewing/data（Initrd:/chewing/）
make musl       # 複製 68k_HAL 已下載的 musl-1.2.5.tar.gz、檢查 SHA-256、建在 build/musl/（MIT；只用來建 Linux 測試程式；P13 起也建 libc.so，--syslibdir 指在 build 裡）
make linux-tests # linux/tests/*.c（hello、files、memory、time、tty、nosys、net、poll、pipe、threads、signals、procs、misc；P13：dyn（PIE + libdyntest.so）與 linux/lib/ld-musl-x86_64.so.1（musl 的 libc.so）） → build/user/linux/（musl-gcc -static），與 Rust std 的 linux/rust/hello-std（make linux-rust：x86_64-unknown-linux-musl、relocation-model=static），打包進 initrd 的 linux/
make user       # 建置 user/ 的程式（hello、spin、fault、ipc_server、ipc_client、keyecho、fstest、proctest、init、shell、windowserver、terminal、echo_win、mouseion、viewer、ifconfig、ping、fetch、tone、hwinfo、bench、kalamos、monitor、periplus、stress、setup、ime、e1000u、powerbox、sandboxtest）與 classic/（Classic.app）→ build/user/；user/bin 的命令列工具與 pkg → build/user/bin/（initrd 的 bin/）
make classic    # 同 make user（Classic.app 的 C 由 classic/build.rs 用 gcc 編）
make initrd     # 把 build/user/ 排成 build/initrd-root/（linux/、Terminal.app 等 bundle）再打包成 build/initrd.tar（GNU tar --format=ustar）
make run        # QEMU + OVMF 開機（一般 kernel：init → shell，提示符 Initrd:/> ），serial 輸出到 stdio
make test       # host 單元測試 + QEMU selftest + 最低硬體 selftest + 桌面、儲存、shell、關機、Linux、USB、panic 畫面、Mouseion 測試，修改後必須通過
make disk-image # 實機用的 build/stela.img（GPT：FAT ESP 含 boot.cfg + eHFS Data，DATA_MIB= 預設 4096；只產生檔案，寫進隨身碟由使用者自己做，docs/p520.md）
make ehfs-tools # host 的 mkfs.ehfs、fsck.ehfs、ehfs-cp、ehfs-ls → build/host/ehfs/
make assets     # assets/*.ppm、*.pbm → assets/generated/（Rust 資料；換圖後執行）
make draft-assets # 重畫草稿圖示（會覆蓋 assets/；使用者的圖放進去之後不要跑）
make test-classic # 本機專用：MAC_ROM、MAC_DISK（預設 ~/mac/，只讀）複製進 build/images/classic-data.img 的 Data:/Classic/，開機五次：Finder 截圖（放大 2 倍）、聲音、800x600 卡 + disk2（hfsutils 的 hformat 造空白 HFS）、FSM 卷宗、Restart → Shut Down 與 System 卷宗正常卸載（tools/hfs_check.py）（不在 make test／CI）
make bench      # 無 KVM 的量測（kernel 大小、開機時間、閒置記憶體、syscall、切換、eHFS、TCP）→ build/bench.md；值得留的貼進 docs/bench.md（P11）
make test-alpine # 本機專用（P13）：~/stela-ports/sources/alpine/ 的 Alpine musl、busybox 套件（只解開在 build/）：Alpine 的 ld-musl + 動態連結的 busybox 在 STELA 跑（docs/linux.md §16）
make test-ehfs-compat  # 本機專用：與 68k_HAL 的 eHFS 工具交叉檢查（不在 make test／CI）
make test-fuzz  # host 的 fuzz（tools/fuzz：eHFS、FAT、GPT、MBR、ustar、網路、HTTP、DNS、HTML、設定檔、路徑；最多 60 秒；進 make test）（docs/testing.md §3）
make test-stress # 本機專用：-smp 4 的壓力測試（100 行程、1000 檔、64 MiB pipe、50 條 TCP、20 視窗，三輪），檢查頁框、kernel heap、handle、行程回到原值（docs/testing.md §4）
make test-ports # 本機專用（P12）：~/stela-ports（PORTS_DIR=）的每個 port 建置、打包、在 QEMU 跑 test.sh、產 index（docs/ports.md）
make test-tls test-pkg test-setup  # P12：HTTPS、pkg、Setup.app（都在 make test 裡）
make clippy     # cargo clippy（loader、kernel 端 crate 含 selftest feature、使用者程式、host 測試），警告視為錯誤
make fmt        # cargo fmt
make clean
```

- 需要的套件：`rustup`（目標 `x86_64-unknown-none`、`x86_64-unknown-uefi`、`x86_64-unknown-linux-musl`（P12，Rust std 的 Linux 測試程式；`rust-toolchain.toml` 列了，CI 自動裝），元件 `rust-src`、`llvm-tools`）、`openssl`（P12 的 test-tls 造測試憑證）、Python 的 `cryptography`（P12 的 `tools/pkgsign.py`）、`qemu-system-x86`、`ovmf`（`/usr/share/OVMF/OVMF_CODE_4M.fd`）、`mtools`、`gdisk`（測試映像）、`gcc`（建 musl、Linux 測試程式與 Classic 環境的 C）、`binutils-m68k-linux-gnu`（Classic 的 68k 測試程式）；本機的 `make test-classic` 另外要 `hfsutils`（`hformat` 造空白 HFS 映像）。musl 的 tarball 不下載：預設從 `~/文件/68k_HAL/third_party/downloads/musl/` 複製（`MUSL_TARBALL=` 可改），CI 從 musl.libc.org 抓一次並快取（`third_party/README.md`）。
- cargo 的輸出在 `build/target/`（`.cargo/config.toml`）。
- CI：`.github/workflows/ci.yml` 在 push 到 main 與 PR 時執行 `cargo fmt --check`、`make clippy` 與 `make test`。
- `make test` 的組成：`test-host`（依賴 `make initrd` 與 `host-images`（mtools／sgdisk 做的 FAT、GPT 映像）；`HOST_CRATES` = abi、block、bootinfo、drivers、ehfs、ehfs-tools、elfload、frame、libstela、net、toolbox、pkgcore（P12）的 `cargo test`，加上 `HOST_LIB_CRATES` = kernel、windowserver、terminal、mouseion、viewer、classic、bin（cli）、periplus、ime（P13，要 `make chewing` 的字典）的 `cargo test --lib`；另外檢查 `assets/generated/` 與圖檔一致）、`test-fuzz`（P11 K45：`cargo build --profile fuzz -p fuzz` 後跑 `build/target/fuzz/fuzz build`，11 個目標各自有次數與時間上限、總共最多 60 秒；panic 或卡住 10 秒就失敗，輸入存到 `build/fuzz-<目標>.bin`）、`test-qemu`（`selftest` feature 的 kernel，放在 `build/esp-test/`，selftest 印 `input: waiting` 時 `tools/qemu_test.py --send-keys` 經 QMP `send-key` 打 `stela` + Enter、印 `mouse: waiting` 時 `--mouse` 經 `input-send-event` 移動 PS/2 滑鼠並按左鍵，依序檢查 `input: OK`、`mouse: OK`、內建使用者程式與 initrd 程式（hello、子執行緒、fault、spin）的輸出、`ipc_client: OK`、`proctest: OK`、`selftest: OK` 與 QEMU exit code 33）、`test-qemu-smp`（同一個 selftest kernel、`-smp 4`：4 顆上線、selftest "smp"（每顆的 tick、跨 CPU 的 wake、每顆一個 spinner、跨 CPU kill））、`test-qemu-min`（同一個 kernel，`-cpu Conroe,-nx -m 128M`（P12 起，之前 256M）、單核：無 NX、無 invariant TSC、無 SMEP；也接 AHCI 測試磁碟跑 fstest）、`test-textmode`（一般 kernel、`boot.cfg` = `textmode` + `nousb` + `verbose` + `waitkey`：每步的 `boot: … done`、QMP 回答 8 次 `press any key`、init 直接開 shell、不啟動 xHCI）、`test-disk-image`（`make_disk_image.py` 做 Data 128 MiB 的映像當唯一的 AHCI 磁碟開機：桌面、shell `mkdir` 後 `shutdown`、host fsck、第二次開機讀回）、`test-sound`（QEMU intel-hda + hda-duplex 錄成 `build/out.wav`：shell `tone` 後 `shutdown`，`tools/check_wav.py` 檢查 1 秒約 440 Hz）、`test-net`（`tools/with_http.py` 在 host serve `build/www`，e1000、e1000e、virtio-net 各開機一次：`ifconfig`、`ping 10.0.2.2`、host 有 DNS 時 `ifconfig resolve example.com`、`fetch`、`run linux/net`）、`test-tls`（P12：`tools/with_https.py` 起 TLS 1.3 伺服器、`tools/make_test_certs.py` 的測試根；`fetch --ca` 拿到檔案，名字錯、過期、自簽、只信 Mozilla 根時都拒絕）、`test-pkg`（P12：測試套件 + 測試鍵簽的 index 經 HTTPS；pkg update/search/install/list/remove、跑裝好的程式）、`test-setup`（P12：`boot.cfg` = `setup` + 一顆沒格式化的磁碟；Setup.app 繁體中文、格式化成 Data、走完每頁、Mouseion 啟動）、`test-window`（一般 kernel 開機進桌面：視窗伺服器 + terminal + shell；經 QMP 打 `uptime`、`run echo_win`、在 echo_win 打一行、Ctrl-C／Ctrl-V 經剪貼簿貼回、P13 Alt-Tab 兩次，screendump 後 `tools/fb_check.py --expect-rect/--expect-text` 檢查桌面、選單列、兩個視窗的標題列與文字；P12：同樣的桌面在 `-m 128M` 再開一次）、`test-storage`（`make test-images` 在 `build/images/` 造 AHCI（GPT：FAT ESP + eHFS Data）與 NVMe（4Kn，eHFS Archive）兩顆磁碟，selftest 開機兩次檢查持久性與 fstest，最後 host 的 `fsck.ehfs` 檢查）、`test-shell`（一般 kernel + AHCI 測試磁碟、`-smp 2`，`qemu_test.py --type` 依序打 shell 指令並檢查輸出；P11：歷史與游標、`hwinfo`、`Initrd:/bin` 工具的管線、`cp -r`、`find`、`date`、萬用字元 `echo /lx/*s`、Tab 補全（`{tab}`）、背景的 Monitor）、`test-pipe`（P11：shell 的管線、`>`、`>>`、`<`、Linux 程式接 pipe、背景工作 `jobs`／`kill`／`fg`）、`test-shutdown`（一般 kernel + AHCI 測試磁碟：shell 打 `shutdown`，QEMU 自己結束（status 0），host 的 fsck 確認正常卸載；再開機打 `reboot`）、`test-linux`（一般 kernel + AHCI 測試磁碟：shell 依序 `run linux/hello a b`、`files`、`memory`、`time`、`nosys`、`tty`（打一行字）、`poll`、`pipe`、P12：`threads`、`procs`、`misc`、`hello-std`、`signals`（QMP 送 Ctrl-C）並檢查輸出與 exit code）、`test-panic`（selftest kernel + 多一個 `panic-test` 檔的 initrd：故意 panic，截圖檢查裂碑的 panic 畫面與底部的最後 10 行訊息）、`test-mouseion`（一般 kernel + 乾淨的 AHCI 測試磁碟：`qemu_test.py --click` 雙擊 Data、welcome.txt（Kalamos）、Initrd、刪除 Classic.app 被拒、Terminal.app（P12 起第二列第一個），截圖檢查卷宗圖示與新視窗；第二次開機重開上次的視窗）、`test-sandbox`（P13：測試磁碟加 `Data:/Sandbox.app`，Mouseion 雙擊後它在容器裡跑：卷宗被拒、Initrd 可以、經 powerbox（Down 四次、Enter）拿到 `Data:/welcome.txt`；鍵盤選單選 File > Find...、桌面回紋；第二次開機 `blank=2s` 熄屏截圖全黑、按鍵叫醒）、`test-editor`（P11：Kalamos 打字、Ctrl-S、Ctrl-Q、shutdown，host `cmp` 存的檔）、`test-ime`（P13：Kalamos 裡 Ctrl-Space、`su3` + Down 開候選窗（`--screenshot` 截圖檢查）、選字、`cl3` Enter、存檔，host `cmp` 是「你好」）、`test-browser`（P11：host serve `build/www`，Periplus 打網址（轉址）、Tab+Enter 跟連結、Alt-Left 返回；P13：打開 `Initrd:/doc/guide.html`）、`test-usb`（一般 kernel、`-machine q35,i8042=off`、`qemu-xhci` + usb-kbd + usb-mouse + usb-hub（後面一個 usb-kbd）：USB 鍵盤打字到 shell、滑鼠報告、`qemu_test.py --qmp` 的 `device_del`／`device_add` 熱插拔 root port 與 hub port；P11：hub 上的 usb-storage（mtools 做的 FAT 隨身碟映像）`cat STICK:/hello.txt`、拔掉（卸載、Mouseion 收到 VolumeWatch）、插另一支、寫檔後 host 用 mtools 讀回）。shell、Linux、關機與 USB 測試都經過桌面（terminal 把 shell 的輸出鏡像到 serial）。`qemu_test.py --log FILE` 把 serial 輸出存檔（`make test-qemu` 寫 `build/test-qemu.log`）。`make run` 永遠跑沒有 selftest 的 kernel（開機畫面的石碑 → init 啟動視窗伺服器與 Mouseion，Mouseion 打開一個 terminal，裡面是 shell；`help` 列出指令，`run Echo.app` 開一個視窗；shell 的輸出也鏡像到 serial）。
- QEMU 的 TCG 不支援 invariant TSC，`make test` 只測得到 tick 退路；TSC 路徑要在本機用 `-enable-kvm -cpu host,+invtsc` 手動驗證（`docs/time.md` §5.3）。

### 測試

- 不碰硬體的 crate（純邏輯：資料結構、檔案系統、協定解析）要能在 host 上以 `cargo test` 測試；硬體相關的 crate 把邏輯與硬體存取分開，讓邏輯的部分可以在 host 測。
- 需要硬體的測試放在 kernel 的 selftest（`kernel/src/selftest.rs`，`selftest` feature；QEMU + OVMF 開機，檢查 serial 輸出，結束時經 `isa-debug-exit` 回報 exit code）。
- 需要使用者自己的檔案（Mac ROM、磁碟映像）的測試只放在本機專用目標，不進 `make test` 或 CI。
- 測試映像只放在 `build/` 底下；造映像與 eHFS 的工具拒絕一般檔案以外的目標（不可對實體磁碟操作）。

## 目錄結構（規劃，隨進度更新）

```
abi/           kernel 與使用者程式共用的 syscall ABI：編號、錯誤碼、位址配置、Handle／Rights／Message、key、DirEntry／Stat／VolumeInfo（no_std，host 測試）（docs/syscall.md、docs/ipc.md、docs/vfs.md）
block/         BlockDevice trait、Partition、GPT、MBR（P11）、CRC-32、MemDisk（no_std，host 測試）（docs/storage.md）
boot/          UEFI loader（x86_64-unknown-uefi）：讀 kernel.elf 與 initrd.tar、建分頁表、取得記憶體表與 framebuffer、交棒（docs/boot.md）
bootinfo/      loader 與 kernel 共用的 BootInfo 結構（版本 3：含 boot.cfg 的 cmdline 與 AP trampoline 頁，no_std，host 測試）
elfload/       最小 ELF64 解析（只處理 PT_LOAD，host 測試）
ehfs/          eHFS（與 68k_HAL 的 eHFS v1 格式相容；no_std，host 測試）（docs/ehfs.md）
net/           網路協定（純邏輯，no_std，host 測試）：wire（Ethernet/ARP/IPv4/ICMP/UDP/TCP 封包）、dhcp、tcp（狀態機）、stack（路由、ARP、DHCP client、socket）（docs/net.md）
frame/         框架 crate：所有 unsafe 的家，對外只提供 safe API（docs/frame.md、docs/smp.md）
  src/arch/x86_64/  架構相關：port I/O、MSR、GDT/TSS、IDT、256 個向量的 stub、8259、local/I/O APIC、PIT、i8042、CMOS RTC（cmos.rs）、
                    context 切換與 enter_user（context.rs）、syscall 進入點與 MSR（syscall.rs）
  src/acpi/         RSDP/XSDT/MADT/MCFG 解析（tables.rs）、FADT（fadt.rs）、DSDT 的 \_S5 掃描（aml.rs）（純邏輯，host 測試）（docs/irq.md、docs/pci.md、docs/acpi.md）
  src/power.rs      ACPI 關機（S5）、重新開機（reset 暫存器 → 8042 → triple fault）（docs/acpi.md）
  src/pci.rs        PCI 設定空間的 legacy ports（docs/pci.md）
  src/irq.rs        向量配置、irq::register、分派與 EOI（docs/irq.md）
  src/cpu.rs        per-CPU 區（GS_BASE；kernel_rsp、need_resched、目前的 context、TSS）、apic_id_of、ticks_of、kick（reschedule IPI）
  src/smp.rs        啟動 AP（trampoline、INIT-SIPI-SIPI、ap_entry）、TLB shootdown（docs/smp.md）
  src/task/         TaskContext、switch_to、kernel 堆疊、enter_user、搶佔與使用者例外的回呼、FS base（docs/process.md、docs/linux.md）
  src/syscall.rs    SyscallFrame、set_handler（docs/syscall.md）
  src/time/         APIC timer 1000 Hz、now_ns、sleep_ms、RTC 解碼（rtc.rs）與 wall_clock_ns（docs/time.md）
  src/cpuid.rs      CPUID 解碼（品牌、family/model、特徵；純邏輯 host 測試；docs/hwinfo.md）
  src/input/        鍵盤解碼、PS/2 滑鼠封包（mouse.rs）、輸入事件佇列、notify hook（docs/keyboard.md、docs/window.md）
  src/mm/           頁框配置（bitmap）、kernel 分頁、MMIO 映射、heap（32 MiB）、行程位址空間（user.rs；含 map_device）、共享記憶體（shared.rs）、DMA（dma.rs）、可給使用者映射的裝置記憶體（device.rs）、initrd()
  src/console/      字型、文字主控台、framebuffer（交給視窗伺服器：FramebufferDevice、hand_over）、panic 畫面（含最後 10 行，ring.rs）、開機步驟 step（verbose／waitkey）（docs/display.md、docs/p520.md §3.1）
kernel/        kernel 主體（safe Rust，x86_64-unknown-none，linker.ld）；lib（host 測試純邏輯）+ bin（入口與 selftest）
  src/task/         Thread、排程器、run queue／睡眠清單／WaitQueue（queue.rs，host 測試）、block_until、Process、wait_process（docs/process.md）
  src/object/       handle 表（handle.rs）、映射區配置（regions.rs）、行模式（ldisc.rs）（host 測試）、Port、Tty（tty.rs）、WaitSet（waitset.rs）、Pipe（pipe.rs）、Thread 的結束紀錄（thread.rs）、KernelObject（docs/ipc.md、docs/linux.md、docs/process.md §8.1）
  src/console.rs    console 服務；src/input.rs  input-router（docs/ipc.md）
  src/block.rs      磁碟 probe 與 USB 磁碟的掛上／拿掉；src/fs/  區塊快取（cache.rs）、FAT（fat.rs；USB 上可寫，P11）（docs/storage.md、docs/vfs.md §6.2）
  src/vfs/          卷宗、資料夾／檔案 Node、eHFS、FAT、Initrd（資料夾樹）與 Tmp（tmpfs.rs，記憶體）後端、掛載（docs/vfs.md）；src/fsyscall.rs  檔案 syscall
  src/linux/        Linux 人格：號碼（nr）、errno、fd 表、合成根目錄的路徑、初始堆疊（host 測試）；分派（sys）、檔案（files）、brk／mmap（memory）、BSD socket（socket）（docs/linux.md）
  src/net.rs        網路：net 執行緒、Socket 物件、socket syscall（docs/net.md）
  src/audio.rs      音效：audio 執行緒、Audio 物件（會話，P13 混音與音量）、AudioOpen/AudioClose/AudioSetVolume/AudioMaster；src/audiofeed.rs  餵 DMA 的帳與混音（host 測試）（docs/sound.md）
  src/device.rs     P13：使用者空間驅動的 Device、NetTap 物件與 syscall、devirq 執行緒（docs/devices.md）
  src/psyscall.rs   行程 syscall（ProcessSpawn、ProcessOpen 等）、SysInfo、SystemPower（docs/userland.md、docs/acpi.md）
  src/hwinfo.rs     硬體報告 SysInfo(HARDWARE)（格式化 host 測試；docs/hwinfo.md）
  src/power.rs      卸載卷宗後關機／重新開機（docs/acpi.md）
  src/syscall.rs    syscall 分派（docs/syscall.md、docs/ipc.md）
  src/initrd.rs     ustar 解析與資料夾樹（host 測試）
  src/program.rs    從檔案（經 VFS）載入 ELF、人格判定（STELA note）、參數訊息或 Linux 初始堆疊、建立行程
user/          使用者程式（x86_64-unknown-none，user.ld 連結在 0x40_0000）（docs/syscall.md §5）
  libstela/         執行期：entry!（含參數）、STELA ELF note、syscall 包裝、ipc、fs、process、tty、net（socket、DNS）、http（GET、轉址、chunked、連結解析）、audio（共用環）、wait、pipe、pipeline、io、heap（global allocator）、path、line、print!／println!（經 console 服務）、ThreadStack/spawn/JoinHandle、panic handler（docs/userland.md）
  hello/ spin/ fault/ ipc_server/ ipc_client/ fstest/ proctest/  initrd 裡的測試程式；stress/  壓力測試（make test-stress）；echo/  鍵盤 echo（文字模式，程式名 keyecho）
  ifconfig/ ping/ fetch/  網路工具（shell 直接打名字；docs/net.md §6）；tone/  方波（docs/sound.md）；hwinfo/  硬體報告；bench/  量測（docs/bench.md）
  bin/              命令列工具（Initrd:/bin/：echo cat grep wc head tail sort uniq find du df date sleep touch stat hexdump tar true false）與 lib cli（純邏輯 host 測試）（docs/apps.md §1）
  pkgcore/ pkg/     P12：套件邏輯（JSON、index、ed25519、ustar、依賴；host 測試）與 Initrd:/bin/pkg（docs/pkg.md）
  setup/            P12：Setup.app，第一次開機（docs/setup.md）
  powerbox/         P13：視窗伺服器替容器裡的程式開的檔案對話框（Initrd:/powerbox）；sandboxtest/  make test-sandbox 的 Data:/Sandbox.app（不在 initrd）（docs/sandbox.md）
  windowserver/     視窗伺服器：compose.rs（純邏輯，host 測試）、server.rs（docs/window.md）
  toolbox/          UI 工具箱 lib：draw、window（含剪貼簿）、event、menu（快捷鍵）、control、text（TextView）、dialog（alert、ask_text、開檔／存檔）、cjk（ASCII 以外的字，Initrd:/fonts/ 的 Fusion Pixel 12px）（docs/toolbox.md）
  terminal/         視窗版的終端機：grid.rs（純邏輯，host 測試）、tty master、shell（docs/window.md §7）
  echo_win/         工具箱的範例（Echo.app）
  mouseion/         空間式檔案管理員：model.rs（純邏輯，host 測試）、桌面、資料夾視窗、拖放（docs/mouseion.md）
  viewer/           Viewer.app：唯讀的文字檢視
  kalamos/ monitor/ periplus/  Kalamos.app（編輯器）、Monitor.app（監視器）、Periplus.app（只有 HTTP 的瀏覽器；lib 是 HTML 解析與排版，host 測試）（暫名；docs/apps.md）
  e1000u/           P13：使用者空間的 e1000／e1000e 驅動（Device 介面的例子；開機選項 usernet 時 init 啟動它）（docs/devices.md）
  ime/              P13：注音輸入法（libchewing 經 FFI；c/ 是墊片 libc 與 config.h，src/ffi.rs 唯一的 unsafe；視窗伺服器啟動它）（docs/ime.md）
  init/ shell/      開機後的 init（有 framebuffer 就跑 windowserver mouseion）與 shell（docs/userland.md）
classic/       Classic 環境（Classic.app，STELA 原生程式；docs/classic.md、classic/README.md）
  musashi/          Musashi（MIT，從 68k_HAL 複製，不改）
  c/                68k_HAL 的 emu/m68k、emu/mac、freestanding/（複製，commit ceb904d）與 shim/（k_lib、host：classic_host_ops 與一台 mac_t）
  build.rs          gcc 編 m68kmake → m68kops.c、全部 C → libclassic_c.a；user.ld
  src/              ffi.rs（唯一的 unsafe）、keys.rs（ADB 鍵碼）、screen.rs（改變的列、Mac 時鐘）、main.rs（Classic.app）、tests.rs（tests/m68k 在 host 跑）
linux/tests/   Linux 人格的測試程式（C，musl 靜態連結）（docs/linux.md）
linux/rust/    P12：Rust std 的 Linux 測試程式（hello-std，自己的 workspace，x86_64-unknown-linux-musl）
drivers/       裝置驅動（safe Rust，透過 frame 的 API 碰硬體）：PCI、AHCI、NVMe、USB（usb/：描述子、HID、TRB、Bulk-Only 大量儲存（storage.rs，P11）、USB 乙太（net.rs：ECM／NCM／RNDIS／AX88179，P13）純邏輯 host 測試；xhci.rs）、網卡（net/：e1000.rs（e1000、e1000e/I219）、virtio.rs）、HDA 音效（hda.rs：codec 路徑純邏輯 host 測試）、sync（排程 hooks、SleepLock、SleepMutex）（docs/pci.md、docs/storage.md、docs/usb.md、docs/net.md、docs/sound.md）
third_party/   第三方資料（spleen/：Spleen 字型，BSD-2-Clause；fusion-pixel/：Fusion Pixel 12px 轉成的字形檔，SIL OFL 1.1（P11）；mozilla-ca/：curl.se 的 cacert.pem，MPL-2.0（P12）；README.md：清單、SHA-256、TLS 與簽章用的 crate）
docs/          設計文件（每個元件完成後寫 docs/<元件>.md）；P13：使用手冊 guide.md（中文）、guide.en.md，`make initrd` 轉成 Initrd:/doc/*.html
assets/        圖示（PPM／PBM，**草稿**）與產生的 Rust 資料（generated/）（docs/identity.md）
tools/         開發工具與測試腳本（qemu_test.py、fb_check.py、with_http.py、with_https.py、check_wav.py、hfs_check.py、bench_report.py、font_to_rs.py、bdf_to_cjk.py、make_test_images.py、make_disk_image.py、boot.cfg（映像的預設開機選項）、make_test_disk.py、gen_casefold.py、ehfs_compat.py、make_draft_assets.py、img_to_rs.py；P12：cacert_to_der.py、make_test_certs.py、pkgsign.py、pkg.pub（測試公鑰，要換）、make_test_packages.py；P13：md_to_html.py（使用手冊 → Initrd:/doc/））
~/stela-ports/ P12：ports repo（暫名，另一個 git repo，還沒上 GitHub）：port.toml、build.sh、test.sh、tools/portbuild.py；8 個 port，P13 加 7 個函式庫（build-depends）與 python3、git、vim、nano、curl、file、less（docs/ports.md §6）
  ehfs/             host 工具 crate：mkfs.ehfs、fsck.ehfs、ehfs-cp、ehfs-ls
  fuzz/             host 的 fuzz（make test-fuzz；docs/testing.md §3）
```

## 程式風格

- 每個新的 `.rs` 檔開頭加上授權標頭：
  ```rust
  // Copyright (C) 2026 YuanChi Hsieh
  // SPDX-License-Identifier: GPL-3.0-or-later
  ```
- `cargo fmt` 的預設格式；`cargo clippy` 零警告（`make clippy`）。
- `unsafe` 只能出現在 `frame/`，以及 `boot/` 的兩處（`exit_boot_services` 與跳進 kernel 的 inline asm；`boot/` 用 `#![deny(unsafe_code)]` 加局部 `#[allow(unsafe_code)]`）、`user/libstela`（`syscall` 指令、`ThreadStack`、`ipc::Mapping` 的複製、`heap` 與 `sync::SpinMutex`、P13 的 `device::Bar`（使用者空間驅動的 volatile 暫存器存取），使用者程式唯一需要 unsafe 的地方）、`user/fault`（故意讀位址 0 的一處 inline asm）、`user/ime/src/ffi.rs`（P13：IME 呼叫 libchewing 的 FFI，同 classic 的規則）、`classic/src/ffi.rs`（Classic 環境呼叫 C 的 FFI；`classic` 的 lib 用 `#![deny(unsafe_code)]`，只有這個檔案 `#![allow(unsafe_code)]`）；其他 crate 以 `#![forbid(unsafe_code)]` 禁止。每個 `unsafe` 區塊前要有 `// SAFETY:` 註解說明為什麼安全。
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
- 68k_HAL 的程式碼在 `~/文件/68k_HAL`（唯讀的參考）；可以參考它的設計與文件，不要複製 C 程式碼進來。唯一的例外是 `classic/`（使用者 2026-10-20 同意：Musashi、emu/m68k、emu/mac、tests/m68k，見 `classic/README.md`）。

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
- P10（自動排程一次做完，使用者已審核（2026-10-21））：實機映像（`docs/p520.md`）、Classic 環境（`docs/classic.md`）、網路（`docs/net.md`）、音效（`docs/sound.md`）、SMP（`docs/smp.md`）
- P12（自動排程一次做完，**使用者尚未審核**）：Linux 相容層補齊（執行緒、fork、execve、wait、訊號、pty、/proc；`docs/linux.md` §11–§12）、ports（`docs/ports.md`）、TLS（`docs/tls.md`）、pkg 與 Setup（`docs/pkg.md`、`docs/setup.md`）、規模（`docs/bench.md`、`docs/architecture.md` §3.1）、PCI 全 bus 掃描與 P520 相容清單（`docs/pci.md` §4.1、`docs/p520.md` §6.5）
- P11（自動排程一次做完，**使用者尚未審核**）：WaitSet 與 poll（`docs/ipc.md` §13、`docs/linux.md`）、安全收緊（能力、MANAGE、`docs/userland.md` §2.3）、Classic 的 FSM／NuBus／整數放大／多磁碟／cfg／關機測試（`docs/classic.md` §7–12）、效能（`docs/storage.md`、`docs/net.md` §4.3、`docs/window.md`）、Pipe 與 shell（`docs/ipc.md` §14、`docs/userland.md` §6）、terminal（`docs/window.md` §7）、延伸屬性與 Mouseion 完善（`docs/vfs.md` §4.5、`docs/mouseion.md`）、Thread 與 join（`docs/process.md` §8.1）、硬體報告（`docs/hwinfo.md`）、開機診斷（`docs/p520.md` §3.1）、量測（`docs/bench.md`）、工具箱與剪貼簿（`docs/toolbox.md` §6.1–6.2）、命令列工具與應用程式（`docs/apps.md`）、中文顯示（`docs/toolbox.md` §6.3）、時區（`docs/time.md` §5.6）、USB 儲存與 FAT 寫入、VolumeWatch（`docs/usb.md` §12、`docs/vfs.md` §6.2）、垃圾桶與排序、zoom box、選單快捷鍵（`docs/mouseion.md`、`docs/window.md`）、shell 的 Tab 補全與萬用字元（`docs/userland.md` §6.3）、fuzz 與壓力測試（`docs/testing.md`）

## 已知問題（尚未修正）

- SMP（P10）：行程親和、沒有遷移與負載平衡、裝置中斷與 kernel 執行緒都在 BSP、最多 16 顆；**排程器的正確性依賴「執行緒永遠不換 CPU」（沒有 finish_task_switch，docs/smp.md §4）——加入遷移或負載平衡前必須先改排程器的鎖**；只支援 xAPIC，沒有 8259 退路（沒有 APIC 或 MADT 就 panic）；LINT1（NMI）遮蔽（docs/smp.md §9）。
- MMIO 映射的虛擬位址只增不減、不回收；中斷處理程式表用一把 SpinLock（SMP 時可改 atomic）。
- 計時器只有週期模式 1000 Hz；依賴 8254 PIT 校準；沒有 invariant TSC 時 `now_ns()` 只有 1 ms 解析度，且長時間關中斷（例如主控台捲動）會少算 tick。
- 鍵盤與滑鼠：PS/2（i8042，兩個 port）與 USB HID boot protocol；US 配置；沒有數字鍵台、LED、按住重複、滾輪的用途、滑鼠加速度（docs/keyboard.md、docs/usb.md §11）。
- PAT entry 4 = WC：視窗伺服器的 framebuffer 映射是 write-combining；kernel 自己的文字主控台仍經 HHDM（write-back，捲動要讀 framebuffer），同一塊記憶體有兩種別名（docs/display.md §8）。
- loader 選面積最大的橫向 GOP 模式（OVMF：2560×1600）；不支援 Bitmask 像素格式；P13 起介面 scale 1 或 2（`boot.cfg` 的 `scale=`，預設 2560 寬以上是 2；視窗協定 v3 是邏輯像素，docs/display.md §6），Setup 沒有設定的地方（Boot 卷宗唯讀）。
- 記憶體表合併後最多 256 個區段（`regions_truncated`）；`KernelAndBoot`（含 loader 自己）與 identity map 用過的分頁表不回收。
- kernel heap 開機 4 MiB、按需成長到最多 64 MiB、不縮回（P12；之前固定 32 MiB）；空掉的中間分頁表不回收；`alloc_contiguous` 是線性 first-fit；PCI 的 ECAM 每個有裝置的 bus 映射 1 MiB、不回收。
- 排程：每顆 CPU 單一等級 round-robin（10 ms 時間片），`priority` 只存不用；沒有 tickless；睡眠解析度 1 ms。
- 位址空間只有 4 KiB 頁、沒有需求分頁／mmap；kernel 半部不能新增 PML4 項目（行程的 PML4 是建立時複製的）；使用者 FPU 只有 FXSAVE（無 XSAVE/AVX）；沒有 SMAP（kernel 不直接解參考使用者指標）。
- NMI／#MC 若剛好落在 `swapgs` 前後或 `sysret` 前會用錯 GS／堆疊（它們沒有 IST）；行程表的 `Exited` 紀錄永遠不清（P11 K46 起只剩名字與 exit code，約 100 bytes；process handle 只記 id，要改成能回收得先讓 handle 持有紀錄）。
- 使用者空間：沒有單一執行緒的 kill（Thread handle 只能 join）；shell 的管線與重新導向只接程式（不接內建指令）、沒有引號與 `[abc]`；前景工作執行時打的字它沒讀就會遺失；heap 不還記憶體（docs/userland.md §9）。
- 應用程式（P11，暫名）：Periplus 只有 HTTP（沒有 TLS）、HTML 子集、單一字型、同步載入；Kalamos 一份文件一個視窗、字型只有 ASCII、Shift-點選不延伸；Monitor 的 CPU 欄是 tick/s 不是使用率；對話框與剪貼簿只有純文字、對話框互動沒有自動測試（docs/apps.md）。
- 量測：2560x1600 下桌面閒置 P11 是 93 MiB，P12（heap 按需成長、桌面視窗只蓋圖示直條）57 MiB；P12 的開機在掛載卷宗時多了約 0.3 秒（TCG，原因未查明，不是 heap 成長）（目標 32 MiB + 畫面緩衝 ≈ 63 MiB，`docs/architecture.md` §3.1、`docs/bench.md`）。
- TLS（P12）：只有 TLS 1.3、P-256、AES-128-GCM；沒有憑證撤銷檢查；連線被切斷就當作回應結束（docs/tls.md §6）。
- pkg 與 Setup（P12）：`tools/pkg.pub` 是**測試鍵**（固定種子，任何人都能簽），正式的鑰匙與套件伺服器要使用者決定；index 沒有防重送；安裝不是交易式；Setup 只有鍵盤操作、新的 Data 沒有 pkg.conf；Mouseion 的桌面只是右邊的直條（直條外的空白處不能拉框、放東西）（docs/pkg.md §6、docs/setup.md §6、docs/window.md §11）。
- ports（P12、P13）：`~/stela-ports` 只在本機；P12 的原始碼大多從 68k_HAL 的下載目錄複製（P13 的從官方網址下載）；tcc 要 `-static`、用自己編的非 PIC musl 並打了 PLT32 的 patch；python3 動態連結 libc.so、沒有 ssl；全螢幕的 vim／nano／less 在 terminal 畫不對（只有 SGR）（docs/ports.md）。
- Mouseion：拖曳影子只是外框；沒有複製貼上；垃圾桶是每個卷宗的 `.Trash`（P11）；圖示全是草稿（docs/mouseion.md §8、docs/identity.md）。
- 視窗：拖外框不即時移動；選單一層；terminal 只有 SGR 顏色、沒有其他 ANSI 序列（docs/window.md §10）；P13 起有桌面圖樣（三種草稿）、閒置熄屏（填黑，沒有 DPMS；叫醒的那個事件不往下送）、Alt-Tab（只有標題）、鍵盤選單（docs/window.md §12）；Mouseion 的 Find 只找名字、一個卷宗；Setup 沒有顯示頁（熄屏時間、scale 只能改檔案）。
- USB：只有第一個 xHCI、USB 2.0 hub 的 port 輪詢、HID boot protocol；儲存只有 Bulk-Only（沒有 UAS）、沒有音訊、電源管理；stall 不復原；未在實機驗證（docs/usb.md §11）。
- 儲存：一個 port 一次一個請求（但一個請求一次發出多個命令；沒有 NCQ）；MSI 一個 message、MSI-X 只用 entry 0、向量不回收；FAT 在 USB 上可寫（`Boot` 唯讀）、沒有 FAT12；USB 隨身碟只有 Bulk-Only、一次 4 KiB（docs/usb.md §12）；RTC 只在開機讀一次、當成 UTC（docs/time.md §5.5）；FAT／Initrd／Tmp 的 `DirRead` 仍每次重新列出；一個卷宗一次一個操作（docs/storage.md §12、docs/vfs.md §8）。
- 實機：kernel 會把看到的任何 GPT 磁碟上的 eHFS 分割區掛成可寫，包括 P520 上 68k_HAL 磁碟（sda）的 Data 分割區（格式相容，不會損壞，但 Mouseion 會寫入 Finder 資訊）；實機測試時留意或先拔掉（docs/p520.md §1）。
- 電源：只認常數 Package 的 `\_S5`（沒有 AML 直譯器、不看 SSDT、不跑 `\_PTS`）；有 `INIT` 能力的程式（預設所有人）都能請 init 關機；關機前不通知程式（docs/acpi.md §9）。
- Linux 人格（P12 起有執行緒、fork、execve、wait、訊號、pty、/proc；P13 起有動態連結與 PIE，`docs/linux.md` §16）：沒有 ASLR，可執行的映射只限 lib 資料夾與程式自己資料夾的檔案；fork 是 copy-on-write（P13，參考計數 `frame::mm::cow`）、一個行程的執行緒都在同一顆 CPU；epoll 是 level-triggered 的 poll、timerfd 沒有 kernel 計時器、`MAP_SHARED` 只在 msync／munmap／execve／exit 寫回（被殺掉的行程最後的修改會丟）且不同範圍不共用（docs/linux.md §16.7–16.9）；沒有 inotify、signalfd、停止／繼續（job control 的 stop）；`sigaltstack` 只記錄；mode／owner／時間不能改；shell 啟動的 Linux 程式看得到 `[3]`、`[4]` 的整個卷宗（不只那個資料夾以下；Mouseion 啟動的只到那兩個資料夾以下，P13）；`PROT_NONE` 可讀；被訊號或例外結束的 exit code 是 128 + 訊號（docs/linux.md §15）。
- 網路：一次一張網卡（P13：PCI 有線優先，否則 USB 網卡，USB 可熱插拔，`docs/usb.md` §13）；TCP 沒有亂序重組、壅塞控制；沒有 IPv6、分片重組、DHCP 續租；socket 的等待者被任何事件全部叫醒；e1000（82540EM）輪詢；I219 未在實機驗證（docs/net.md §10）。
- 音效：只有輸出、48 kHz 16 位元雙聲道；P13 起多個程式混音、每會話音量與主音量（線性、只在 kernel 的混音裡、codec 的 amp 不動）；輪詢 LPIB；實機 codec 未驗證（docs/sound.md §7）。
- IPC：訊息夾帶自己 port 的 handle 會形成 Arc 循環；送滿／表滿的競爭邊界會放掉夾帶的 handle；共享記憶體是 eager；WaitSet 的行程成員以 id 記，要自己移除（`docs/ipc.md` §12、§13）。
- 能力（P11）：`NETWORK` 與 `INIT` 預設往下傳給所有程式（沙箱還沒有拿掉它們的地方）；P13 的 `VOLUMES`：Mouseion 啟動的程式沒有（容器 `Data:/Programs/NAME/container/`、檔案只經 powerbox、拖放、`[6]`；`Initrd` 不需要它），但 shell 啟動的都有（開發者模式），拖放沒有自動測試、Periplus 不收檔案（docs/sandbox.md §9）；P13 的 `DEVICES` 只有 init 給它啟動的驅動，沒有 IOMMU，所以持有它等於持有整台機器（docs/devices.md）；`FilePath` 讓持有資料夾 handle 的程式知道上面各層的名字（docs/userland.md §2.3、docs/vfs.md §4.4）。
- 中文與時區（P11）：只有 BMP 的 12px 字型（16x32 字型時 2 倍放大）、沒有中文輸入法、kernel 主控台與 panic 畫面只有 ASCII；時區是固定的 UTC 偏移（沒有夏令時間、沒有時區資料庫）（docs/toolbox.md §6.3、docs/time.md §5.6）。
- eHFS 的效能（P11 K45 的觀察）：每次查詢都讀 16 KiB 的 B-tree 節點並逐位元組算 CRC-32C（沒有節點快取）；`make test-stress` 在 TCG 下建刪 1000 個檔約 35 秒（docs/testing.md）。
