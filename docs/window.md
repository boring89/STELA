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
| `SET_CLIPBOARD` 0x109（P11） | 0、長度；長度 > 0 時夾帶裝著 UTF-8 文字的 Memory（最多 `CLIPBOARD_MAX` = 1 MiB） | — |
| `GET_CLIPBOARD` 0x10A（P11） | 0 | `CLIPBOARD` |
| `DISCONNECT` 0x10B（P11 K46） | 0 | —（伺服器關掉它的視窗、忘掉這個 client；toolbox 的 `Connection` 被 drop 時送。以前只有程式結束（主 port 關掉）才清，同一個程式反覆連線會每次留下一個 client：`make test-stress` 看到每輪約 11 KB 的 kernel heap） |

| 事件（server → client） | 內容 |
|---|---|
| `KEY` 0x183 | window、`KeyEvent`（4 bytes） |
| `MOUSE` 0x184 | window、x、y（內容座標）、buttons、kind（1 移動 2 按下 3 放開）、clicks（2 = 雙擊） |
| `WINDOW_EVENT` 0x185 | window、kind（1 FOCUS、2 BLUR、3 MOVED、4 RESIZE_REQUESTED、5 CLOSE_REQUESTED）、x、y、w、h |
| `MENU_SELECTED` 0x186 | 選單 index、項目 index |
| `ERROR` 0x187 | 請求的 tag、`abi::Error` |
| `CLIPBOARD` 0x188（P11） | 長度；長度 > 0 時夾帶一份文字的 Memory（client 讀完關掉） |

**選單快捷鍵（P11 K43）**：`SET_MENUBAR` 的項目文字結尾 `^K`（字母或數字）表示 Alt-K（Alt 當 Command）。伺服器在選單右邊畫 `Alt-K`；焦點 client 的選單有這個快捷鍵時，Alt+字母（沒有 Ctrl）不送給程式，改送 `MENU_SELECTED`。其他 Alt 組合照樣送（Classic.app 的 Mac 用 Alt 當 Command 不受影響，它的選單沒有快捷鍵）。toolbox：`MenuBar::item` 傳回去掉 `^K` 的標籤、`menu::shortcut`。標準：New^N、Open^O、Save^S、Close^W、Quit^Q、Undo^Z、Cut^X、Copy^C、Paste^V、Select All^A（Kalamos、Mouseion、Echo、Terminal、Monitor、Periplus、Viewer 都加了）。

**zoom box（P11 K43）**：可改大小的視窗（`RESIZABLE`）標題列右端多一個框中框。按下去：伺服器記住內容區原來的位置與大小、把視窗搬到選單列下的左上角，送 `WINDOW_EVENT` `ZOOM_REQUESTED`（6，x、y、w、h = 整個螢幕減去選單列與框）；再按一次搬回原處、要回原大小。client 跟 `RESIZE_REQUESTED` 一樣回 `RESIZE_WINDOW`（toolbox 把 `ZOOM_REQUESTED` 也解成 `Event::ResizeRequested`，所以程式不用改）；client 可以給不一樣的大小（terminal 對齊格子）。

**系統剪貼簿（P11 K32，使用者尚未審核）**：伺服器自己存一份文字（`Vec<u8>`）；`SET_CLIPBOARD` 從 client 的 Memory 複製進來，`GET_CLIPBOARD` 每次做一塊新的 Memory 放一份給它。只有純文字，沒有「誰擁有」的概念（最後設的贏），伺服器重啟就沒了。toolbox：`Connection::set_clipboard(text)`、`clipboard() -> String`（等 `CLIPBOARD` 時其他事件照樣先存起來）。0x188 以前是 `PING`（P11 K2 拿掉了），現在重新用在 `CLIPBOARD`。

旗標：`TITLE_BAR` 1、`CLOSE_BOX` 2、`RESIZABLE` 4、`DESKTOP` 8（沒有框、永遠在最後面、蓋滿選單列以下的整個畫面：給 P9 的 Mouseion 畫桌面圖示）；`DOCUMENT` = 1|2|4。P11：`WANT_MOTION` 16（沒按按鈕時指標在內容上移動也送 `MOUSE`／`MOVE`，給指標在它下面的那個視窗）、`HIDE_CURSOR_INSIDE` 32（指標在內容上時伺服器不畫箭頭；拖曳中照畫）——Classic.app 兩個都設（Mac 自己畫游標）。

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

- **遮擋剔除**（P11，使用者尚未審核）：視窗都是不透明的矩形（框 + 內容），所以每個髒矩形先算出每個視窗**看得到的部分**——它的框 ∩ 髒矩形，再減掉所有比它前面的視窗的框（`compose::subtract`：一個矩形減一個矩形最多剩 4 塊；`visible_parts`）——只畫那些部分；桌面色只填沒有任何視窗的部分。被蓋住的視窗不再整個複製一次內容（以前每個髒矩形都從後往前把每個重疊的視窗畫一遍）。
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
- P11（計畫的 B5）：鍵盤只給焦點視窗的 client、選單列只顯示焦點視窗的 client 的選單——沒有視窗的 client 設了 `SET_MENUBAR` 也不會顯示（存著，等它有視窗）。所以不另外拒絕（Mouseion 先設選單再建桌面視窗）。
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
 一個執行緒，WaitSet 等三樣東西（P11）：
   主 port（key 0）：視窗事件、SHELL_READY
   master（key 1）：TtyRead 到沒有為止 → grid.write → 重畫有變的列 → DAMAGE
                   └─► 同一份送到 console（serial）
   shell 行程（key 2，EXITED）：印 [shell exited…]、啟動新的
```

- 視窗：80×25 格（16x32 字型時 1288×808，四邊 4 像素）、`DOCUMENT`、標題 `Terminal`；選單 `Terminal > Clear`，P11 起還有 `Edit > Copy`、`Edit > Paste`。
- **選取、複製、貼上（P11 K32）**：在格子上按下拖曳選取（按讀的順序從起點到終點，反白；只按一下不拖不算選取，打字會取消選取）；Copy 把選取的文字（每列去掉尾端空白，列之間 `\n`，`Grid::text_between`）放進剪貼簿；Paste 把剪貼簿的文字一個字一個字轉成按鍵（`abi::key::key_for_char`，美式配置的反查；非 ASCII 的字略過）送給 shell，跟打字一樣。
- `TtyCreate(MASTER)`：slave 給 shell 當 `[1]`；master 和主 port 放在同一個 WaitSet（P11；P8 時沒有同時等兩個東西的 syscall，用一個輸出執行緒把 tty 輸出轉成主 port 的訊息，現在拿掉了）。toolbox 等回覆時先收下的事件（`Connection::has_pending()`）在每次等待前處理掉。shell 行程也在集合裡（`EXITED`），不再每 500 ms 檢查一次；行程成員以 id 記，換新 shell 前先 `remove`。
- **serial 鏡像**：輸出也送到 terminal 自己的 `[1]`（console 服務 → serial），所以 `make test-shell`、`test-linux`、`test-shutdown` 在桌面模式下照樣從 serial 檢查 shell 的輸出（鍵盤經 QMP → PS/2 → 視窗伺服器 → terminal → shell）。
- 文字格（`grid.rs`）：UTF-8 可以在任何地方被切開；`\n` 換行（底部捲動）、`\r`、Backspace、Tab（8 的倍數）；記錄改過的列。游標是反白的格子（失去焦點時不畫）。
- **P11（使用者尚未審核）**：每格是 `Cell { c, fg, bg }`；`ESC [ … m`（SGR）設顏色：30–37／90–97 前景、40–47／100–107 背景、1 粗體（前景用亮色）、22、39、49、0 或空的重設；其他 `ESC [ …` 照舊略過。16 色的調色盤配羊皮紙底色調過（`PALETTE`，0 是釉黑、15 是羊皮紙）；預設前景釉黑、背景羊皮紙。
- **捲回**：捲出上緣的行存起來（`SCROLLBACK` = 1000 行）；Shift+PgUp／PgDn 一次一頁，捲回時不畫游標；有新的輸出就跳回最下面。改大小時歷史保留（截斷或補空白到新寬度）。
- **視窗大小**：terminal 開始時與每次改大小後 `TtySetSize(master, 欄, 列)`（syscall 57），Linux 程式的 `TIOCGWINSZ` 拿到的就是它（以前固定 25×80）。
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
| host `windowserver::compose` | P11：矩形相減（不相交、全蓋住、四塊不重疊、沿邊一塊）、每個視窗看得到的部分與桌面；框的幾何、堆疊與命中測試（關閉鈕、grow box、標題列、邊框、桌面視窗留在後面）、髒矩形合併與上限、選單解析與配置、雙擊、自動放置 |
| host `terminal::grid` | 寫入與換行、捲動、控制字元、ESC 序列、UTF-8 切開、改大小；P11：SGR 顏色（前景、背景、粗體變亮、重設、其他序列略過）、捲回（翻頁、到頂、新輸出跳回、1000 行上限） |
| selftest "mouse" | IRQ 12 已接；`qemu_test.py --mouse` 經 QMP `input-send-event` 移動 (40, 30) 並按左鍵 |
| `make test-window` | 一般 kernel：看到 `STELA shell` 打 `uptime`、再 `run echo_win`、在 echo_win 打 `hello`（serial 看到 `echo_win: hello`）；截圖檢查（`fb_check.py --expect-rect/--expect-text`）：桌面色、選單列、terminal 的標題列（非作用中）與文字（≥ 4 行）、echo_win 的標題列（作用中）與內容 |
| `make test-shell`／`test-linux`／`test-shutdown` | 現在都經過桌面（serial 鏡像） |

`make test-fb` 改名為 `make test-window`；文字主控台的檢查搬到 selftest "framebuffer object"（`docs/display.md` §7）。

## 10. 已知限制

- 只有拖外框，不即時移動；沒有視窗最小化、縮放動畫、透明、陰影（選單除外）。
- 選單只有一層，沒有快捷鍵；STELA 字樣不是選單。
- 只有單一滑鼠按鍵（左鍵）有意義；滾輪不處理；沒有加速度。
- terminal 只有 SGR 顏色（P11），沒有游標移動、清除畫面等其他 ANSI 序列；捲回不能用滑鼠滾輪；選字複製沒有。
- P11 起有遮擋剔除，但沒有「只有游標動」的特別處理（游標的舊位置仍從 back buffer 重新合成那一小塊）。

## 11. 桌面視窗只蓋圖示的直條（P12，使用者尚未審核）

P11 的桌面視窗（`DESKTOP` 旗標）蓋滿整個螢幕：Mouseion 為了在右邊畫幾個卷宗圖示，拿了一張全螢幕的畫布（2560×1600 時 16 MiB 的共享記憶體），其中絕大部分只是填滿桌面色。P12 起：

- `CREATE_WINDOW` 帶 `DESKTOP` 旗標時，要求的寬度 `w`（大於 1、小於螢幕寬）就是桌面視窗的寬：它貼在螢幕**右邊**、從選單列下面到底；寬度 1（或螢幕寬）時照舊蓋滿整個螢幕（相容）。
- Mouseion 要求「圖示格寬 + 兩邊的邊距」（2560 寬時約 192 像素）：卷宗與垃圾桶的圖示原本就在右邊，位置不變（測試的點擊座標不用改）。
- 其他的桌面面積沒有視窗：視窗伺服器的合成本來就會把沒有視窗的地方填成桌面色（§4 的遮擋剔除）。
- 計畫的兩個做法裡選了第二個（「退一步只覆蓋圖示直條」）：第一個（`SET_DESKTOP`，把圖示交給視窗伺服器畫）要把 Mouseion 的拖放、選取、改名搬到伺服器，改動太大。代價：在直條以外的桌面上拉框選取、把東西拖到「桌面」的空白處都沒有作用（P11 也沒有放東西到桌面這回事，只有拖到卷宗與垃圾桶）。
- 記憶體：桌面閒置從 93.0 MiB 降到 57.0 MiB（另一半的功勞是 kernel heap 按需成長，`docs/frame.md` §8、`docs/bench.md`）。
