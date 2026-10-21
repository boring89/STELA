# 視窗伺服器與桌面

> P8 由自動排程一次做完，使用者已於 2026-10-20 審核。

`user/windowserver` 擁有畫面、鍵盤與滑鼠，把各個程式（client）的視窗合成到畫面上；`user/terminal` 是第一個 client：視窗版的 shell。畫面的取得見 `docs/display.md`，client 用的函式庫見 `docs/toolbox.md`。

程式碼：

```
abi/src/window.rs              協定：標籤、旗標、訊息的編碼與解碼（host 測試）
abi/src/key.rs                 MouseEvent（8 bytes）、button
user/windowserver/src/compose.rs  視窗堆疊、框的幾何、命中測試、髒矩形、選單、雙擊（純邏輯，host 測試）
user/windowserver/src/server.rs   伺服器：畫面、合成、輸入、請求
user/terminal/src/grid.rs      文字格（純邏輯，host 測試）
user/terminal/src/main.rs      terminal
user/init                      有 framebuffer 就啟動 windowserver terminal
frame/src/input/mouse.rs       PS/2 滑鼠的封包解碼（host 測試）
frame/src/arch/x86_64/i8042.rs 第二個 port（滑鼠，IRQ 12）
```

## 1. 比喻：舞台監督

舞台（畫面）只有一個，演員（程式）各自在自己的**畫板**（Memory，視窗大小 × 4 bytes）上畫，畫好了舉手說「這一塊畫好了」（`DAMAGE`）。舞台監督（視窗伺服器）負責把畫板依前後順序擺上舞台、畫外框與標題、決定觀眾的手指（滑鼠）指到誰、麥克風（鍵盤）給誰（焦點）。

演員彼此看不到對方的畫板；只有舞台監督看得到全部。

## 2. 開機流程

```
 kernel ──► Initrd:/init（[2] = Framebuffer）
 init：[2] 是 Framebuffer（HandleKind）？
   是 ──► Initrd:/windowserver terminal（[1] console、[2] Framebuffer、[3] Data 的根）
          結束就 1 秒後再啟動
   否 ──► 文字模式：Initrd:/shell（P6 的流程）
 windowserver：MemoryMap([2]) → InputAttach(主 port) → 畫桌面
               → 啟動參數裡的程式（Initrd:/terminal），[2] = 自己主 port 的 SEND
 terminal：連線、建視窗、TtyCreate(MASTER)、啟動 Initrd:/shell（[1] = tty、[2] = terminal 的主 port）
 shell：SHELL_READY → terminal；terminal 回 WINDOW_SERVER（夾帶視窗伺服器的 SEND）
```

selftest 版的 kernel 不跑 init，所以 `make test-qemu` 仍是文字模式。

P9 起 init 啟動的是 `windowserver mouseion`：Mouseion 畫桌面（`DESKTOP` 視窗）並打開 `Initrd:/Terminal.app`（terminal 包成 bundle），流程的其餘部分不變（`docs/mouseion.md` §2）。

## 3. 協定（`abi::window`，經 port 的訊息）

**版本 2（P11，使用者尚未審核）**：每個 client 有自己的請求 port，伺服器**從訊息來的 port 認出 client**，不再看訊息內容（版本 1 的 client id 可以被冒用）。

```
 client                                     server
   │ CONNECT（word 0 = VERSION 2，夾帶回覆 port 的 SEND）
   │ ──────────────► 伺服器主 port（client 的 [2]）
   │                 建一個新 port P，WaitSet 加入 P（key = id）
   │                 與回覆 port（key = id | 1<<32，只等 CLOSED）
   │ ◄────────────── CONNECTED（client id、螢幕…，夾帶 P 的 SEND）
   │ 之後的請求都送到 P
```

client 的 `[2]` 是伺服器主 port 的 `SEND`，主 port 只收 `CONNECT` 與 kernel 的輸入事件（送到主 port 的其他請求被丟掉）。回覆與事件送到 client 的**主 port**。每個請求的第一個 word 保留（0）；版本不符的 `CONNECT` 得到 `ERROR`（`NotSupported`）。client 結束時它的主 port 關閉，伺服器的 WaitSet 立刻看到回覆 port 的 `CLOSED`（`docs/ipc.md` §13.3），移除它的視窗——**`PING` 拿掉了**（0x188 不再使用）。數字一律 little endian；座標是 i32 螢幕像素；視窗位置指**內容區**的左上角（框與標題列由伺服器畫在外面）；像素是 u32 `0x00RRGGBB`。

| 請求（client → server） | 內容 | 回覆 |
|---|---|---|
| `CONNECT` 0x100 | `VERSION`；夾帶回覆 port 的 SEND（送到伺服器主 port） | `CONNECTED`：client id、螢幕寬高、scale、選單列高度；夾帶這個 client 的請求 port 的 SEND |
| `CREATE_WINDOW` 0x101 | 0、w、h、flags、x、y（`AUTO` = 伺服器決定）、標題（≤ 88 bytes UTF-8） | `WINDOW_CREATED`：window id、w、h，夾帶 Memory（w×h×4） |
| `DAMAGE` 0x102 | 0、window、x、y、w、h（視窗座標） | — |
| `MOVE_WINDOW` 0x103 | 0、window、x、y | — |
| `RESIZE_WINDOW` 0x104 | 0、window、w、h | `WINDOW_RESIZED`（新的 Memory） |
| `SET_TITLE` 0x105 | 0、window、標題 | — |
| `SET_MENUBAR` 0x106 | 0、長度，夾帶 Memory：每行一個選單 `Title\tItem\tItem…`，`-` 是分隔線 | — |
| `CLOSE_WINDOW` 0x107 | 0、window | — |
| `FOCUS_WINDOW` 0x108 | 0、window | — |

| 事件（server → client） | 內容 |
|---|---|
| `KEY` 0x183 | window、`KeyEvent`（4 bytes） |
| `MOUSE` 0x184 | window、x、y（內容座標）、buttons、kind（1 移動 2 按下 3 放開）、clicks（2 = 雙擊） |
| `WINDOW_EVENT` 0x185 | window、kind（1 FOCUS、2 BLUR、3 MOVED、4 RESIZE_REQUESTED、5 CLOSE_REQUESTED）、x、y、w、h |
| `MENU_SELECTED` 0x186 | 選單 index、項目 index |
| `ERROR` 0x187 | 請求的 tag、`abi::Error` |

旗標：`TITLE_BAR` 1、`CLOSE_BOX` 2、`RESIZABLE` 4、`DESKTOP` 8（沒有框、永遠在最後面、蓋滿選單列以下的整個畫面：給 P9 的 Mouseion 畫桌面圖示）；`DOCUMENT` = 1|2|4。

## 4. 合成（`compose.rs`、`server.rs`）

```
 back buffer（伺服器自己的記憶體，和畫面一樣大，Vec<u32>）
 每個髒矩形 r：
   1. 填桌面色
   2. 視窗由後往前：框（1 px 黑邊）→ 標題列（作用中：terracotta + 兩條黑線；非作用中：石色）
      → 標題（置中、羊皮紙底）→ 關閉鈕 → 內容（從 client 的 Memory 一列一列複製，read_words）
      → grow box（蓋在內容右下角）
   3. 選單列（羊皮紙、底線、STELA 字樣（粗體）、焦點 client 的選單標題）
   4. 拉下的選單（陰影、項目、反白）
   5. 拖曳中的虛線外框
 present(r)：back buffer 的 r 複製到 framebuffer（RGBX 時交換 R、B），再把游標畫在上面
```

- **髒矩形**（`Damage`）：新的矩形和重疊或相鄰、而且合併後面積不超過兩者和的兩倍的舊矩形合併；超過 16 個就全部合成一個。
- 不讀 framebuffer（WC 讀很慢）：back buffer 就是「存底圖」，游標移動只要把舊位置與新位置當髒矩形。
- 游標：11×17 的箭頭（自己畫的 1-bit 圖，黑框白底），16x32 字型時放大 2 倍。
- client 的 Memory 伺服器以唯讀映射、用 `Mapping::read_words` 複製（client 隨時可能在寫）。

### 4.1 主迴圈（P11）

```
 WaitSet：主 port（key 0，READABLE）＋ 每個 client 的請求 port（key = id）＋ 回覆 port（key = id | 1<<32，CLOSED）
 loop：
   wait（有髒矩形時不等，否則永遠）
   key 0     → 主 port 最多 64 則：INPUT_KEY／INPUT_MOUSE／CONNECT
   key id    → 那個 client 的 port 最多 64 則請求
   CLOSED    → 移除 client（它的視窗、port、選單）
   flush（合成、顯示）
```

## 5. 輸入

- `InputAttach(主 port)`：kernel 送 `INPUT_KEY`、`INPUT_MOUSE`（PS/2 滑鼠，`docs/keyboard.md` 的 i8042 第二個 port：設定預設值、開啟回報、IRQ 12；3 bytes 封包，bit 3 重新同步）。
- 鍵盤：給焦點視窗的 client（`KEY`）。
- 滑鼠左鍵按下，看指到哪裡（`Stack::hit`）：
  - 選單列：焦點 client 的選單標題 → 拉下選單；移動時反白項目、換選單；放開在項目上 → `MENU_SELECTED`。
  - 標題列：取得焦點、拖曳**虛線外框**（Classic 風格，不即時移動），放開時移動並送 `MOVED`。
  - 關閉鈕：按著時反黑，放開仍在框內 → `CLOSE_REQUESTED`（client 自己決定要不要關）。
  - grow box：拖外框，放開送 `RESIZE_REQUESTED(w, h)`，client 回 `RESIZE_WINDOW`。
  - 內容：取得焦點、送 `MOUSE DOWN`（含雙擊判定：500 ms、4 像素內）；按著時的移動與放開都送給這個視窗（capture）。
- 焦點：點到的視窗，或新建的視窗；換焦點送 `BLUR`／`FOCUS`，重畫兩個標題列與選單列。`DESKTOP` 視窗不會被提到前面。
- 新視窗的位置（`AUTO`）：從左上角 (40, 選單列 + 標題列 + 30) 起，每個往右下 30 像素（8 個一輪）。

## 6. 顏色（黑繪陶）

| 用途 | 顏色 |
|---|---|
| 線條、文字 | 釉黑 `#1A1410` |
| 選單、視窗底 | 羊皮紙 `#F2E8D5` |
| 作用中的標題列 | terracotta `#C8763C` |
| 非作用中的標題列 | 石色 `#D8CDB8` |
| 桌面 | 愛琴海灰藍 `#5B7B86` |
| 反白 | `#2E4A62` |

## 7. terminal（`user/terminal`）

```
 視窗伺服器 ── KEY ──► terminal ── INPUT_KEY ──► shell 的主 port
                                               （shell 跑程式時再轉給 tty）
 shell 與它的程式 ── 印到 [1] = tty ──► master
 輸出執行緒：TtyRead(master) ──► OUTPUT 訊息到 terminal 的主 port
                             └─► 同一份送到 console（serial）
 主執行緒：grid.write(輸出) → 重畫有變的列 → DAMAGE
```

- 視窗：80×25 格（16x32 字型時 1288×808，四邊 4 像素）、`DOCUMENT`、標題 `Terminal`；選單 `Terminal > Clear`。
- `TtyCreate(MASTER)`：slave 給 shell 當 `[1]`；master 由輸出執行緒讀（libstela 的 `ThreadStack` + `spawn`：沒有同時等兩個東西的 syscall，所以用一個執行緒把 tty 輸出變成主 port 的訊息）。
- **serial 鏡像**：輸出也送到 terminal 自己的 `[1]`（console 服務 → serial），所以 `make test-shell`、`test-linux`、`test-shutdown` 在桌面模式下照樣從 serial 檢查 shell 的輸出（鍵盤經 QMP → PS/2 → 視窗伺服器 → terminal → shell）。
- 文字格（`grid.rs`）：UTF-8 可以在任何地方被切開；`\n` 換行（底部捲動）、`\r`、Backspace、Tab（8 的倍數）、`ESC [ … 結尾字元` 略過；記錄改過的列。游標是反白的格子（失去焦點時不畫）。
- shell 結束時印 `[shell exited with code N]` 並啟動新的；關閉鈕：kill shell、結束 terminal。
- `RESIZE_REQUESTED`：換成放得下的格數（至少 10×3）。
- shell 收到 `WINDOW_SERVER`（`abi::tag` 6，夾帶視窗伺服器的 SEND）後，`run` 的程式的 `[2]` 就是它，所以在 terminal 裡 `run echo_win` 會開視窗。

## 8. 其他程式

- `user/echo_win`：工具箱的範例（`docs/toolbox.md` §7），放在畫面右邊。
- `user/echo`：保留給文字模式（自己 `InputAttach`；桌面模式下會被拒絕）。

## 9. 測試

| 測試 | 內容 |
|---|---|
| host `abi::window` | 編號固定、`VERSION` = 2 在 `CONNECT` 的 word 0、請求的編碼往返（word 0 = 0）、標題在字元邊界截斷、KEY／MOUSE |
| host `abi::key` | `MouseEvent` 編碼往返 |
| host `frame::input::mouse` | 封包、負值、重新同步、overflow |
| host `windowserver::compose` | 框的幾何、堆疊與命中測試（關閉鈕、grow box、標題列、邊框、桌面視窗留在後面）、髒矩形合併與上限、選單解析與配置、雙擊、自動放置 |
| host `terminal::grid` | 寫入與換行、捲動、控制字元、ESC 序列、UTF-8 切開、改大小 |
| selftest "mouse" | IRQ 12 已接；`qemu_test.py --mouse` 經 QMP `input-send-event` 移動 (40, 30) 並按左鍵 |
| `make test-window` | 一般 kernel：看到 `STELA shell` 打 `uptime`、再 `run echo_win`、在 echo_win 打 `hello`（serial 看到 `echo_win: hello`）；截圖檢查（`fb_check.py --expect-rect/--expect-text`）：桌面色、選單列、terminal 的標題列（非作用中）與文字（≥ 4 行）、echo_win 的標題列（作用中）與內容 |
| `make test-shell`／`test-linux`／`test-shutdown` | 現在都經過桌面（serial 鏡像） |

`make test-fb` 改名為 `make test-window`；文字主控台的檢查搬到 selftest "framebuffer object"（`docs/display.md` §7）。

## 10. 已知限制

- 只有拖外框，不即時移動；沒有視窗最小化、縮放動畫、透明、陰影（選單除外）。
- 選單只有一層，沒有快捷鍵；STELA 字樣不是選單。
- 只有單一滑鼠按鍵（左鍵）有意義；滾輪不處理；沒有加速度。
- terminal 不支援 ANSI 顏色與游標移動（只略過 ESC 序列）；沒有捲回歷史；選字複製沒有。
- 每次合成都把整個髒矩形重畫（沒有遮擋剔除）；TCG 下大矩形會慢。
