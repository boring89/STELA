# Classic 環境（Classic.app）

> P10 由自動排程一次做完，**使用者尚未審核**；之後修改前請先帶使用者過一遍這份文件。

Classic 環境讓一台 **Macintosh IIci**（68030、25 MHz）在 STELA 的視窗裡執行使用者自己的 ROM 與 System 7。它是縮減版的 68k_HAL：68k 核心是 Musashi，機器模型是 68k_HAL 的 `emu/mac`，兩者都是 C，從 68k_HAL 複製進 `classic/`（使用者授權的唯一例外，`classic/README.md`）。外面包一層 Rust，透過一般的使用者空間 API（視窗、檔案、事件、時鐘）和 STELA 打交道。

## 1. 比喻：在玻璃櫃裡放一台舊電腦

Musashi 是「CPU 晶片」，`emu/mac` 是「主機板」（決定哪個位址是 RAM、ROM、VIA、影像 RAM），而 Classic.app 是**玻璃櫃**：把主機板的螢幕接到視窗、把鍵盤滑鼠的訊號從視窗事件接進去、把硬碟接到 `Data:/Classic/System.img` 這個檔案。舊電腦完全不知道自己在櫃子裡；STELA 也不需要為它加任何特別的 syscall。

```
 ┌─────────── Classic.app（一個 STELA 行程）───────────────────────────┐
 │  main.rs（Rust）                                                    │
 │    ├─ toolbox：視窗（640x480）、事件 ──────────┐                    │
 │    ├─ libstela::fs：ROM、System.img、pram       │                    │
 │    └─ ffi.rs（唯一有 unsafe 的檔案）            │                    │
 │         │ classic_start / classic_frame / classic_render ...        │
 │  ───────┼─────────────────────────── C（gcc -ffreestanding）──────  │
 │         ▼                                                           │
 │  c/shim/host.c ── 一台 mac_t ── c/mac（VIA、RBV、DAC、ADB、.Disk…） │
 │                                   └─ c/m68k/vm.c ── musashi/（68030）│
 └─────────────────────────────────────────────────────────────────────┘
```

## 2. 檔案

| 位置 | 內容 |
|---|---|
| `classic/musashi/` | Musashi 4.10（MIT），**沒改**；設定經 `c/m68k/m68kconf.h`（`MUSASHI_CNF`）：68030、PMMU、位址錯誤、指令 hook |
| `classic/c/m68k/` | 68k_HAL 的 `emu/m68k`：Musashi 的薄包裝（平坦 RAM 或機器模型的 bus、A-line trap、F-line） |
| `classic/c/mac/` | 68k_HAL 的 `emu/mac`：Mac IIci 的機器模型、ROM 修補、`.Disk` 驅動、ADB 注入、NuBus 卡、FSM（後兩者有編進去但沒用到） |
| `classic/c/freestanding/` | 給 Musashi 的最小 libc 替身（`sin`、`fprintf`… 都是空的或呼叫 `m68k_vm_fatal`） |
| `classic/c/shim/k_lib.[ch]` | 68k_HAL kernel 函式庫的 `k_memcpy` 等，改用 memcpy/memset/memcmp（STELA 建置時由 Rust 的 compiler_builtins 提供） |
| `classic/c/shim/host.[ch]` | **`classic_host_ops`** 與一台靜態的 `mac_t`：Rust 只呼叫這裡的函式，不需要知道 `mac_t` 的配置 |
| `classic/build.rs` | 用 build machine 的 gcc 編 `m68kmake`、產生 `m68kops.c/h`，再把全部 C 編成 `libclassic_c.a`；也指定 `user/user.ld` |
| `classic/src/ffi.rs` | C 函式的宣告與安全包裝：`Vm`（裸 68k，給測試）、`Mac`、`MacDisk` trait、`m68k_vm_fatal` |
| `classic/src/keys.rs` | STELA 的 `KeyCode` → ADB 鍵碼 |
| `classic/src/screen.rs` | 找出畫面改變的列（damage 用）、Mac 的時鐘（1904 年起的秒） |
| `classic/src/sound.rs` | Mac 的聲音（8 位元無號單聲道、約 22 kHz）轉成 48 kHz 16 位元雙聲道（最近鄰，跨呼叫保留相位） |
| `classic/src/main.rs` | Classic.app 本體 |
| `classic/tests/m68k/` | 68k 測試程式（`m68k-linux-gnu-as` 組譯） |

### 2.1 建置

`build.rs` 依 `TARGET` 選旗標：

- STELA（`x86_64-unknown-none`）：`gcc -std=gnu11 -O2 -ffreestanding -nostdinc -fno-stack-protector -fno-pic -fno-pie -mno-red-zone`，標頭只有 `c/freestanding/include` 與 gcc 自己的（`gcc -print-file-name=include`：stddef.h、stdint.h…）。C 程式碼可以用 SSE（使用者執行緒有 FXSAVE，`docs/process.md`）；Rust 端是 soft-float，但兩邊之間不傳浮點數。
- host 測試：一般的 gcc 與 libc（`-fPIC`）。
- 我們自己的 C（`c/m68k`、`c/mac`、`c/shim`、`c/freestanding`）加 `-Wall -Wextra -Werror`；Musashi 用 `-w`（和 68k_HAL 一樣，它的風格過不了我們的警告）。
- **計畫外的決定**：計畫核准了 `cc` crate，但實際上直接用 `std::process::Command` 呼叫 `gcc` 與 `ar`（`cc` 不在本機的 cargo 快取，而直接呼叫只要十幾行，不多一個相依）。

### 2.2 從 68k_HAL 複製後的改動

- include 路徑（`emu/m68k/…` → `m68k/…` 等，`classic/README.md`）。
- `freestanding/divdi3.c` 沒複製（只有 32 位元建置需要）。
- **`c/mac/video.c` 的修正**：內建視訊在 1/2/4 位元時，調色盤要讀 256 項的**最上面**幾項（1 位元：254 白、255 黑）；68k_HAL 讀第 `index` 項，白色變成開機畫面留下的灰（158）。68k_HAL 從 P9 起改用 NuBus 卡，所以沒發現。

## 3. `classic_host_ops` 與執行流程

```c
typedef struct {
    int (*disk_read)(void *ctx, uint32_t offset, void *buf, uint32_t len);
    int (*disk_write)(void *ctx, uint32_t offset, const void *buf, uint32_t len);
    void *ctx;
    uint32_t disk_blocks;
    bool disk_read_only;
    void (*audio)(void *ctx, const uint8_t *samples, uint32_t count, uint32_t rate);
    void *audio_ctx;
} classic_host_ops;
```

`emu/mac` 本來就設計成「不開檔、不畫圖」：ROM、RAM、影像 RAM 都由嵌入它的程式交進去，結果再讀出來。所以需要回呼的只有磁碟與聲音（`audio`、`audio_ctx`：ASC 播出的 8 位元樣本；`ffi.rs` 的 trampoline 收進一個緩衝，最多約 1 秒，主迴圈每個影格取走）；Rust 端的 `MacDisk` trait 由 `FileDisk`（`fs::read`／`fs::write` 讀寫 `System.img`）實作，`ffi.rs` 的兩個 `extern "C"` trampoline 把 `ctx` 轉回 `Box<dyn MacDisk>`。log 走 Rust 的 `println!`（C 端沒有輸出）。

Classic.app 的流程：

1. 讀 `Data:/Classic/ROM`（必須 512 KiB；`mac_rom_patch` 只認 IIci 的 368CADFE）、開 `Data:/Classic/System.img`（讀寫）、讀 `Data:/Classic/pram`（256 bytes，沒有就全 0）。缺任何一個就印出清楚的訊息、結束（exit 1）。開機選項 `classic=off` 也讓它直接結束。
2. `Mac::start`：RAM 16 MiB、影像 RAM 512 KiB（`0xFBB08000`，ROM 的 UniversalInfo 表）、時鐘 = `SysInfo(TIME)`（P10 新增：牆上時鐘）換算成 1904 年起的秒、`mac_reset`。
3. 開一個 640x480 的視窗（標題列 + 關閉框，不可縮放），選單 File > Quit。
4. 主迴圈，每一圈：
   - 收完所有待處理的事件（`next_event(0)`）：鍵 → `keys::adb_code` → `mac_key`；滑鼠 → `mac_mouse_warp(x, y)` + `mac_mouse_move(0, 0, 按鈕)`（視窗座標直接對應 Mac 的螢幕座標，不經過 Mac 的滑鼠加速）。
   - 跑一個 60 Hz 的影格：`mac_run(416000)`（25 MHz ÷ 60）。
   - 每兩個影格（約 30 fps）`mac_render_changes` 畫出整張 640x480（內建視訊每次都整張畫），`Screen::update` 和上一張比較，只把改變的列（相隔 8 列內合併成一段）複製進視窗的 Memory 並 damage。
   - 每 30 影格看一次低記憶體的 `CurApName`（`$910`）是不是 "Finder"，第一次是就印 `classic: finder: …`；三秒後印 `classic: finder desktop shown`（`make test-classic` 在這時截圖）。
   - 聲音：每個影格把 ASC 的樣本（`take_audio`）經 `sound::Resampler` 轉成 48 kHz 雙聲道寫進 `Audio`（P10 的 S；開不到——沒有音效卡或別的程式在用——就丟掉）；環放不下的丟掉（模擬比真實時間慢，寧可斷音也不要越積越久的延遲）。
   - 每 300 影格，PRAM 若被改過（`xpram_dirty`）就寫回 `Data:/Classic/pram`。
   - 比真實時間快就 `sleep`，慢就 `yield`（QEMU TCG 下大約只有 22 fps，所以一直在 yield）。
5. 結束：關閉框或 File > Quit → 寫 PRAM、關視窗、exit 0。Mac 的 Special > Shut Down → 同上；Restart → 重新讀檔、重新開機。

C 端的狀態都是全域變數（Musashi 一顆 CPU、`vm.c` 一條 bus、`host.c` 一台 `mac_t`），所以一個行程一次只能有一台機器：`Vm::new`／`Mac::start` 在另一台存在時失敗（`VM_IN_USE`）。

## 4. 使用方式

把**你自己的**檔案放進 Data 卷宗（例如用 host 的 `ehfs-cp` 寫進 `make disk-image` 的映像，或之後用 Mouseion 拖進去）：

```
Data:/Classic/ROM          Mac IIci 的 ROM（512 KiB，checksum 368CADFE）
Data:/Classic/System.img   HFS 磁碟映像（例如 System 7.5.3），會被寫入
```

然後雙擊 `Initrd:/Classic.app`，或在 shell 打 `run Classic.app`。

鍵盤：Alt = Command、Ctrl = Control、Windows 鍵／右 Alt = Option；Backspace 是 Mac 的 delete，Insert 是 Help。

## 5. 測試

| 測試 | 內容 |
|---|---|
| `make test`（host） | `cargo test -p classic --lib`：`tests/m68k/*.s` 用 `m68k-linux-gnu-as -m68030` 組譯、在 Musashi 上跑（move、算術旗標、迴圈、副程式、除以零與 F-line 例外、A-line trap 記錄與 handler、cycle 上限、大端序記憶體、一次一台機器）；`Mac::start` 拒絕不是 IIci 的 ROM、另一台機器存在時回 `Busy`；ADB 鍵碼表（範圍、唯一）；改變的列的分段；1904 年的時鐘；重新取樣（跨呼叫維持速率、音量對應） |
| `make test-classic`（**本機專用**，不在 `make test`、CI） | `MAC_ROM`、`MAC_DISK` 預設指向 `~/mac/`（只讀）；`make_disk_image.py` 做 Data 128 MiB 的映像，`ehfs-cp` 把兩個檔**複製**進 `Data:/Classic/`（映像在 `build/images/classic-data.img`）；從映像開機、shell `run Classic.app`、等 `classic: finder desktop shown`；截圖檢查視窗裡 Mac 的選單列（白）與桌面的 50% 格紋（`fb_check.py --expect-pattern`）。第二次開機加 `intel-hda` + `hda-duplex`（錄成 `build/classic.wav`）：`run Classic.app --test-beep`（Finder 出現三秒後 `mac_sysbeep`，再兩秒結束）→ `shutdown` → `check_wav.py` 要有聲音（約 0.6 秒） |

QEMU（TCG，無 KVM）裡 Finder 在約 300 影格、14 秒後出現。

CI 要裝 `binutils-m68k-linux-gnu`（`.github/workflows/ci.yml`）。

## 6. 限制與下一輪

- 只有內建視訊 640x480、1 位元（System 的預設；Monitors 控制台可以改 2/4/8 位元，renderer 支援）。NuBus 顯示卡（大解析度、16/32 位元）、HiDPI 下一輪。
- 滑鼠只在按住按鈕或按下／放開時送事件（視窗伺服器的 `Move` 只在按住時送），所以游標在 Mac 裡只在點擊與拖曳時跟上；沒有隱藏 STELA 的游標。
- 一顆磁碟（`System.img`）；`.Disk` 的多磁碟、FSM 卷宗（把 STELA 的 Data 給 Mac 看）、Applications 卷宗下一輪（程式碼已複製，沒接上）。
- 聲音（ASC）已接到 `Audio`（S 完成後補上，`--test-beep` 驗證 SysBeep）；最近鄰的重新取樣、沒有音量控制；Classic.app 開著時別的程式（`tone`）拿不到音效（`Busy`）。
- 序列埠、網路、SCSI 裝置都沒有。
- 時鐘當成 UTC（STELA 的 RTC 也當成 UTC，`docs/time.md` §5.5），所以 Mac 的時間比台灣時間慢 8 小時。
- 效能：TCG 下約 22 fps（比真的 IIci 慢約 3 倍）；實機（KVM 或 P520）應該可以到 60 fps，沒有驗證。
- Classic.app 一次一個（C 的全域狀態是每個行程一份，所以兩個 Classic.app 行程其實可以同時跑，只是會用同一個 `System.img`——不要這樣做）。
- 沒有 `classic=off` 以外的設定（RAM 固定 16 MiB）。
