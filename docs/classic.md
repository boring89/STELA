# Classic 環境（Classic.app）

> P10 由自動排程一次做完，使用者已於 2026-10-21 審核。

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
5. 結束：關閉框或 File > Quit → 寫 PRAM、關視窗、exit 0。Mac 的 Special > Shut Down → 同上；Restart → 重新讀檔、重新開機（P11 K26 確認並加了測試，§12）。

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

- 內建視訊 640x480、1 位元（System 的預設；Monitors 控制台可以改 2/4/8 位元，renderer 支援）；P11 起可以改用 NuBus 顯示卡（§8，大解析度、16/32 位元）。P11 K24 起視窗可以整數放大（§10）。
- P11：視窗設 `WANT_MOTION` 與 `HIDE_CURSOR_INSIDE`（`docs/window.md` §3）：沒按按鈕的移動也送進來，Mac 的游標一直跟著；指標在視窗內時 STELA 的箭頭不畫，只看到 Mac 的游標。座標直接對應（沒有 Mac 的滑鼠加速）。
- 磁碟：`System.img`，P11 K25 起加一顆 `disk2`（§11；`.Disk` 支援到 drive 11，但設定只有一個 `disk2`）。P11 起 Data 卷宗經 FSM 給 Mac（§7）；只有 Data 一顆，沒有 Desktop Manager（Finder 自己在 Data 的根建 `Desktop` 檔）。不能在 Mac 執行中換磁碟（沒有退出／插入）。
- 聲音（ASC）已接到 `Audio`（S 完成後補上，`--test-beep` 驗證 SysBeep）；最近鄰的重新取樣、沒有音量控制；Classic.app 開著時別的程式（`tone`）拿不到音效（`Busy`）。
- 序列埠、網路、SCSI 裝置都沒有。
- 時鐘當成 UTC（STELA 的 RTC 也當成 UTC，`docs/time.md` §5.5），所以 Mac 的時間比台灣時間慢 8 小時。
- 效能：TCG 下約 22 fps（比真的 IIci 慢約 3 倍）；實機（KVM 或 P520）應該可以到 60 fps，沒有驗證。
- Classic.app 一次一個（C 的全域狀態是每個行程一份，所以兩個 Classic.app 行程其實可以同時跑，只是會用同一個 `System.img`——不要這樣做）。
- P11 起設定在 `classic.cfg`（§9）；開機選項 `classic=off` 仍然有效。

## 7. Data 卷宗：File System Manager 外掛（P11 K9，使用者尚未審核）

**比喻**：玻璃櫃裡的舊電腦本來只有一顆自己的硬碟（`System.img`）。現在在它的背面接一條線到 STELA 的 Data 卷宗：舊電腦以為那是一顆「網路磁碟」，它的檔案櫃管理員（File Manager）每次要找檔案、讀寫，就經這條線問 STELA。

```
 Mac 的 File Manager ─► FSM ─► c/mac/fsm.s／fsm.c（68k_HAL 的外掛，已複製）
                                   │ mac_fs_ops_t（get、lookup、child、read、volume、
                                   │              create、remove、move、set_info、write、set_length）
                                   ▼ ffi.rs 的 extern "C" trampoline（ctx = Box<dyn MacFs>）
                     classic::hostfs::HostFs（純邏輯，host 測試）
                                   │ Store trait
                                   ▼
                     main.rs 的 datafs::DataStore：libstela::fs，從 Data 的根 handle
```

- `classic_host_ops` 多一個 `fs`（`mac_fs_ops_t *`，NULL = 沒有）；`classic_start` 在 `mac_reset` 前 `mac_fsm_attach`。Finder 開始執行後，模型在安全的時間點注入 `install`（Gestalt、AddDrive、InstallFS、PBVolumeMount），桌面上出現「Data」（`classic: the Data volume is on the Mac's desktop`；`classic_fs_status` 給 install 的結果、是否掛上、處理過的請求數）。
- **ID**：Mac 的 ID = STELA 的持久 ID（eHFS 的 CNID；兩邊的根都是 2，其他 ≥ 16；放不進 32 位元的看不到）。
- **名字**：UTF-8 ↔ Mac Roman（`c/mac/mac_roman.c` 經 FFI：`ffi::mac_roman_char`／`mac_roman_byte`）；STELA 名字裡的 `:` 在 Mac 上顯示成 `/`（Mac 的 `/` 存成 `:`）；Mac Roman 沒有的字元換成 `_`，換過或超過 31 字元的名字結尾加 `#<ID>`（查詢時也認得這個尾巴）。Mac 給的名字有控制字元、`:` 或以 `.` 開頭 → `bdNamErr`。
- **看不到的**：`.` 開頭的名字，以及根目錄的 `Classic` 資料夾（裡面是 Mac 正在用的 `System.img`、ROM、PRAM——讓 Mac 改到自己的開機磁碟會壞掉）。
- **Finder 資訊**：32 bytes 照 HFS 的原意存 FInfo + FXInfo（檔案）或 DInfo + DXInfo（資料夾），經 `FileGetInfo`／`FileSetInfo`（要 `MANAGE`；Classic.app 拿 `VolumeOpen("Data")` 的根，有 MANAGE）。Mouseion 自己的格式（`MI` 開頭）或全 0 時，Mac 看到的是 0；檔案的 type/creator 預設依副檔名（`.txt`、`.md`、`.c`、`.rs`… → `TEXT`／`ttxt`，SimpleText 的文件；其他 `????`）。**和 Mouseion 共用欄位**：Mac 寫過的項目，Mouseion 的圖示位置／視窗狀態會被蓋掉（E 部分的 K22 把 Mouseion 的狀態搬到延伸屬性）。
- **resource fork**：STELA 的檔案 API 沒有 resource fork，所以存在根目錄的隱藏資料夾 `.rsrc/<ID>`（第一次寫入時建立；項目刪除時一起刪）。
- **日期**：STELA 的奈秒（1970 起）↔ Mac 的秒（1904 起，差 2082844800）；卷宗大小與剩餘空間最多顯示 2 GB（System 7.5 的上限）。
- **錯誤對照**：Exists → `dupFNErr`、NotFound → `fnfErr`、NotDir → `dirNFErr`、NoSpace → `dskFulErr`、NotEmpty → `fBsyErr`、ReadOnly／AccessDenied → `wPrErr`、搬進自己 → `badMovErr`、其他 → `ioErr`。
- **同時寫**：Mac 寫 Data 時 STELA 的其他程式照樣能改（沒有 68k_HAL 的「Mac 執行時唯讀」）；每個呼叫都是重新 open 再 close，所以 STELA 端的改名、刪除 Mac 下次列目錄時就看到（最後一次的列表有快取，Mac 自己改過就清掉）。
- **執行緒**：回呼在 Musashi 執行中同步呼叫（`Mac::frame` 裡），`libstela::fs` 的 syscall 會讓 Classic.app 的主執行緒等一下（計畫的決定，68k_HAL 也是這樣）。
- `LogFs`（main.rs）把建立、刪除、搬移與每個檔案第一次的讀取印到 console（`classic: fs: created …`）。

### 7.1 測試

- host：`hostfs` 的測試用記憶體裡的 `Store`：名字兩個方向（`:`／`/`、é ↔ 0x8E、中文 → `_` + `#ID`、超過 31 字元、壞名字）、根與列表（隱藏 `Classic` 與 `.` 開頭的、`TEXT`／`ttxt`、日期、讀、2 GB 上限）、`#ID` 的查詢、寫入（建立、重名、兩個 fork、改長度、Finder 資訊、搬移與改名、搬進自己、非空資料夾、刪除連同 resource fork）、Mouseion 的資訊不給 Mac。
- `make test-classic`（本機）第三次開機：`run Classic.app --test-fs`：桌面出現三秒後由 Classic.app 自己送鍵盤事件給 Mac——打 `data`（Finder 依名字選取）、Command-O（打開 Data 的視窗）、約 3 秒後 Command-N（新資料夾）——要看到 `classic: fs: created untitled folder`；Classic.app 結束後 shell 的 `ls Data:/` 看得到 `untitled folder`，`shutdown` 後 host 的 `ehfs-ls` 也看得到、`fsck.ehfs` 乾淨。

## 8. NuBus 顯示卡（P11 K11，使用者尚未審核）

**比喻**：IIci 主機板上的顯示晶片只能接 13 吋的 Apple 螢幕（640x480）。真的 Mac 要大螢幕就在 NuBus 插槽插一張顯示卡；卡上有一張「名片」（declaration ROM）告訴 Mac 自己有哪些模式。我們插的是一張**虛擬的卡**（68k_HAL 的 `c/mac/nubus_video.c`，已複製）：名片與驅動程式都在 C 裡，畫面記憶體是 Classic.app 的一塊 32 MiB。

- `classic_host_ops` 多 `card_vram`（`CLASSIC_CARD_VRAM_SIZE` = 32 MiB，NULL = 只有內建視訊）、`card_res`（卡的解析度清單的 index：640x480、800x600、1024x768、1152x870、1280x720、1280x960、1280x1024、1440x900、1600x1200、1920x1080、1920x1200、2560x1440、3840x2160）、`card_depth`（0–5 = 1、2、4、8、16、32 位元）；`classic_start` 在 `mac_reset` 前 `mac_card_attach`（slot `$C`）。有卡時內建視訊回報「沒有螢幕」，卡是唯一的畫面。
- Rust：`ffi::Card { resolution, depth }`、`card_resolution(w, h)`、`card_depth(bits)`；`Mac::start` 的 `card` 參數（`None` = 內建視訊，仍是預設）。
- Classic.app 的參數 `resolution=WxH`、`depth=N`（預設 8 位元；K12 起也可以寫在 `classic.cfg`）；卡沒有那個解析度時退回內建視訊並說明。
- **解析度切換**：Mac 的 Monitors 控制台換模式時，`mac_screen_size` 跟著變；主迴圈每個影格比較，變了就 `RESIZE_WINDOW`（toolbox `Connection::resize`）、重建 `Screen` 與畫面緩衝，印 `classic: screen WxH`。
- 畫面：`mac_render_changes` 有卡時畫卡（只轉換被寫過的列，`MAC_CARD_DIRTY_SHIFT` 每 KiB 一個位元），沒卡時畫內建視訊（每次整張）。

### 8.1 測試

- host：`card_modes`（640x480 = 0、800x600 = 1、1024x768 = 2、1280x960 = 5、沒有的 → None；8 位元 = 3、32 位元 = 5）。
- `make test-classic`（本機）的第三次開機：`classic.cfg` 寫 `ram=32`、`resolution=800x600`、`depth=1`、`scale=1`（K12、K24），要看到 `classic: screen 800x600 (scale 1)` 與 Finder 的桌面；截圖檢查 640 以右的選單列（白）與 480 以下的桌面格紋。

## 9. `Data:/Classic/classic.cfg`（P11 K12，使用者尚未審核）

一行一個 `key=value`，`#` 之後是註解，前後空白不計；沒有檔案就是預設值。Classic.app 的同樣格式的參數（`run Classic.app resolution=800x600`）蓋過檔案。看不懂的行印出來（`classic: classic.cfg: line N: …（ignored）`）後略過。解析是純邏輯（`classic::config`，host 測試）。

| 設定 | 預設 | 意思 |
|---|---|---|
| `ram=16` | 16 | Mac 的 RAM（MiB），8–1024 |
| `resolution=800x600` | （沒有：內建視訊 640x480） | 用 NuBus 顯示卡（§8）的這個模式；`builtin` = 內建視訊 |
| `depth=8` | 8 | 卡的色深：1、2、4、8、16、32 |
| `scale=2` | （沒有：螢幕寬 ≥ 1920 且放大後放得下時 2，否則 1） | 視窗像素／Mac 像素（§10） |
| `fs=off` | on | 不把 Data 卷宗給 Mac（§7） |
| `disk2=Applications.img` | （沒有） | `Data:/Classic/` 裡的第二顆磁碟（§11）；`,ro` 唯讀、`,rw` 讀寫（預設） |

```
# Data:/Classic/classic.cfg
ram=32
resolution=1024x768
depth=8
```

## 10. 整數放大（P11 K24，使用者尚未審核）

**比喻**：2560 寬的螢幕上，640x480 的 Mac 只有郵票大小。把每個 Mac 像素畫成 2x2 的方塊（像把照片放大到兩倍，不做平滑），視窗就變成 1280x960，字也看得清楚。

- `scale`（`classic.cfg` 或參數，1 或 2）；沒設時 `Config::scale_for(螢幕, Mac 解析度)`：螢幕寬 ≥ 1920 **而且**放大後的視窗放得下（寬 ≤ 螢幕寬、高 + 64（選單列與標題列，`config::ROOM`）≤ 螢幕高）才是 2，否則 1。明確寫的值一律照做。
- 畫面：Mac 的畫面緩衝仍是 Mac 的大小；改變的列（`Screen::update`）用 `screen::scale_rows` 最近鄰放大（每個像素重複 scale 次、每列重複 scale 次）再複製進視窗，damage 的矩形也乘上 scale。scale 1 直接複製（不經過放大緩衝）。
- 滑鼠：視窗座標除以 scale 再給 Mac。
- Monitors 換解析度時重新算 scale（自動時可能從 2 變 1），視窗改成 Mac 解析度 × scale；印 `classic: screen WxH (scale N)`。
- 測試：host 的 `rows_are_enlarged`（2x2 → 4x4、3 倍、1 倍不變、超出畫面的列略過）、`defaults`（2560x1600 → 2、1920x1080 上 1024x768 → 1、2560x1600 上 1280x800 → 1）。本機 `make test-classic` 的第一次開機沒有 cfg：QEMU 的 2560x1600 → `classic: screen 640x480 (scale 2)`，截圖檢查放大的選單列與 2x2 方格的桌面（`fb_check.py --expect-pattern …,2`）；第三次開機的 cfg 加 `scale=1`（800x600 的卡、原來的檢查）。

## 11. 第二顆磁碟（P11 K25，使用者尚未審核）

**比喻**：IIci 的機殼裡可以多裝一顆硬碟。`disk2=Applications.img` 就是把 `Data:/Classic/Applications.img` 當成第二顆硬碟接上，Mac 開機後桌面上多一個卷宗。

- `.Disk` 驅動（`c/mac/disk.c`，68k_HAL 複製來的）本來就有 drive 8–11（`MAC_MAX_DISKS` = 4）：Open 時把 `blocks > 0` 的每一顆 `_AddDrive` 進 drive queue，Mac 開機時 File Manager 掛上每一顆的 HFS 卷宗。這次只是把 host 端接上：
  - `classic_host_ops.more_disks[3]`（`ctx`、`blocks`、`read_only`；讀寫用同一組 `disk_read`／`disk_write`，靠 `ctx` 分辨），`classic_start` 抄進 `mac.more_disks`。
  - Rust：`Mac::start(rom, ram, disks: Vec<Box<dyn MacDisk>>, …)`，`disks[0]` 是 System（drive 8），之後最多 3 顆（drive 9–11）；空的或太多 → `BadArguments`。
  - Classic.app：`FileDisk::open(name, read_only)`（唯讀時只用 READ 開檔，Mac 寫入會被 `.Disk` 拒絕）；打不開就印 `classic: no second disk: …` 照樣開機。
- `classic_volumes`（C shim，新寫的）：走 File Manager 的 VCB 佇列（low memory `VCBQHdr` 的 qHead `$358`；VCB 的 `vcbVN` 在 44、`vcbDrvNum` 在 72，Inside Macintosh: Files），回傳每個已掛載卷宗的 drive 與名稱；`Mac::volumes()`。Finder 的桌面出現時 Classic.app 印 `classic: volume "名稱" (drive N)`。
- 映像要是**整顆 HFS 卷宗**（沒有分割表，和 `System.img` 一樣；第 1024 byte 是 `BD`）。host 上可以用 hfsutils 做：`truncate -s 100M Applications.img; hformat -l Applications Applications.img`，再 `ehfs-cp` 進 `Data:/Classic/`。
- 測試：本機 `make test-classic` 的第三次開機（800x600 卡）加 `disk2=Applications.img,ro`：host 用 `hformat` 在 `build/images/` 造 4 MiB 的空白 HFS（`HOME` 指到 `build/images`，hfsutils 的「目前卷宗」紀錄不碰真的家目錄），要看到 `classic: disk2 Applications.img (4096 KiB, read only)` 與 `classic: volume "Applications" (drive 9)`。需要 hfsutils（只有 `test-classic`）。

## 12. Shut Down、Restart 與「拔插頭」（P11 K26，使用者尚未審核）

**比喻**：Mac 的 Special > Shut Down 是「正常關機」——Mac 自己把快取寫回磁碟、在卷宗上蓋「已正常卸載」的章，才切電源。Classic.app 的關閉框是「拔插頭」——Mac 來不及做這些事。

| 怎麼結束 | Mac 端 | Classic.app |
|---|---|---|
| Special > Shut Down | 結束程式、卸載卷宗（寫回 Mac 自己的快取、HFS 的 MDB 設 `kHFSVolumeUnmountedBit`）、經 RBV（VIA2 port B bit 2）切電源 → `mac_run` 回 `MAC_POWER_OFF` | 寫 PRAM、印 `classic: the Mac was shut down`、關視窗、exit 0（**STELA 不關機**） |
| Special > Restart | 同上，然後跳回 ROM 的起點：`emul_reset` 第二次 → `MAC_POWER_RESTART` | 寫 PRAM、印 `classic: restarting`、丟掉整台機器、重新讀 ROM／`System.img`／`disk2`／pram 與 `classic.cfg`、重新開機（新視窗大小照新的設定） |
| 關閉框、File > Quit | 什麼都沒做（拔插頭） | 寫 PRAM、關視窗、exit 0 |

- **flush**：Classic.app 這一側沒有自己的緩衝——`FileDisk` 的每次寫都直接 `fs::write`（kernel 的 eHFS 快取是 write-through，`docs/storage.md`），FSM 卷宗的每個操作也是直接的 `fs` 呼叫；PRAM 在每種結束方式、重新開機前、以及每 300 影格（有變動時）寫回。所以「flush」只剩 PRAM，三種結束都做了；磁碟映像的 handle 在 `Mac` drop 時關掉。
- **拔插頭的後果**：Mac 自己的快取（System 的 disk cache）沒寫回，`System.img` 的 HFS 卷宗留著「使用中」的標記；下次開機 System 7.5 會先跳「上次沒有正常關機」的警告（要按 OK），並可能檢查磁碟。所以**用 Special > Shut Down 結束比較好**；關閉框沒有改成「請 Mac 關機」（要模擬 Mac 的電源鍵或選單，留給之後）。
- 測試：`Classic.app --test-power`（本機 `make test-classic` 的第五次開機）：Finder 桌面出現三秒後用滑鼠拉下 Special 選單（`SPECIAL_TITLE` (232, 10)；項目 16 列一格：Clean Up Desktop、Empty Trash…、—、Eject Disk、Erase Disk…、—、Restart、Shut Down）放在 Restart 上 → `classic: restarting` → 第二個桌面 → Shut Down → `classic: the Mac was shut down` → shell `shutdown`；host 把 `System.img` 用 `ehfs-cp out` 拿出來，`tools/hfs_check.py` 檢查 MDB 的 `drAtrb` bit 8（已正常卸載）、`fsck.ehfs` 檢查 STELA 的 Data。這次開機用的是**第一次開機前**複製的映像（`classic-power.img`）：前幾次開機都是拔插頭結束，System 7.5 的警告會擋住測試的點擊。Mac 掛上卷宗時就會清掉 bit 8 並寫回 MDB（前幾次開機後是 0x0000），所以檢查通過代表 Mac 的卸載確實寫到了 STELA 的檔案。
- 滑鼠腳本的步驟（`Step::Mouse`）與鍵盤腳本（`Step::Key`，`--test-fs`）共用一個清單，時間以「桌面出現後的影格數」計；Restart 之後清單清空，第二個桌面出現時再排 Shut Down。超過 600 影格還沒有結果就印 `classic: power test: still running`（除錯時用來截圖）。

## P13：介面的 scale

STELA 的介面有 scale（`docs/display.md` §6）時，Classic 的畫面座標是邏輯的：它自己的倍數（`scale_for` 或 `classic.cfg` 的 `scale=`）乘上介面的 scale 最多 2；log 的 `classic: screen WxH (scale N)` 印相乘的結果。2560 螢幕預設 2x：Classic 自己 1 倍、toolbox 放大 2 倍，看起來和以前一樣。
