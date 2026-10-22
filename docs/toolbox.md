# UI 工具箱（toolbox）

> P8 由自動排程一次做完，使用者已於 2026-10-20 審核。

寫視窗程式用的函式庫 `user/toolbox`。模組的切分照 Classic Mac OS 的 Manager：`draw`（QuickDraw）、`window`（Window Manager）、`event`（Event Manager）、`menu`（Menu Manager）、`control`（Control Manager）。協定見 `docs/window.md`。

## 1. 比喻：畫家的工具箱

視窗伺服器給你一塊畫布（視窗的 Memory）。工具箱裡有：尺和筆（`draw`：矩形、線、字）、郵差（`window::Connection`：跟伺服器講話）、信件分類（`event`：把收到的訊息變成 `Event`）、菜單（`menu`）、現成的零件（`control`：按鈕、輸入框、捲軸、清單）。

## 2. `draw`

| 項目 | 說明 |
|---|---|
| `Color` | u32 `0x00RRGGBB`；`color::*`：`BLACK`、`PARCHMENT`、`TERRACOTTA`、`STONE`、`DESKTOP`、`HIGHLIGHT`、`WHITE`、`GREY` |
| `Rect` | x、y、w、h；`intersect`、`union`、`contains`、`offset`、`inset`、`overlaps`、`area`、`right`、`bottom` |
| `Font` | `SMALL`（Spleen 8x16）、`LARGE`（16x32），`include!` 共用 `third_party/spleen/*.rs`（BSD-2-Clause）；`font_for(screen_width)`：≥ 1920 用 LARGE；ASCII 以外顯示 `?` |
| `Canvas` | 對 `&mut [u32]`（stride 可大於寬）：`fill`、`hline`、`vline`、`frame`、`line`（Bresenham）、`bitmap`（1-bit，MSB 在左）、`image`（32-bit，可指定透明色）、`copy_from`、`char`、`text`；一切都裁切在畫布與 `set_clip` 的矩形內 |

## 3. `window`

```rust
let mut conn = Connection::open()?;                 // 經 [2] 連線，回覆到主 port
let mut win = conn.create_window(400, 300, flags::DOCUMENT, AUTO, AUTO, "Hello")?;
{
    let mut c = win.canvas();                       // 直接畫在共享的 Memory 上
    c.fill(win.bounds(), color::PARCHMENT);
    c.text(8, 8, "hello", font_for(conn.screen.0), color::BLACK, None);
}
conn.damage_all(&win)?;                             // 請伺服器重畫
loop {
    match conn.next_event(FOREVER) { … }
}
```

- `Connection::open()`／`open_with(handle)`：送 `CONNECT`（版本 2，夾帶主 port 的 SEND）到伺服器主 port、等 `CONNECTED`（5 秒），之後的請求都送到 `CONNECTED` 夾帶的這個 client 自己的請求 port（P11，`docs/window.md` §3）；等回覆時收到的其他訊息存起來，之後由 `next_event` 依序交出（`has_pending()` 告訴自己用 WaitSet 等主 port 的程式先處理它們）。
- `create_window`、`resize`（等 `WINDOW_CREATED`／`WINDOW_RESIZED`）、`damage`、`damage_all`、`set_title`、`move_window`、`focus`、`close`、`set_menubar(text)`（建 Memory、寫入、夾帶送出）。
- `Window::canvas()`：`ipc::Mapping::words_mut()` 直接給 `&mut [u32]`——libstela 裡唯一的那處 `unsafe`（SAFETY：這塊 Memory 只有這個程式寫，伺服器只讀；別的行程讀到的最多是畫到一半的像素）。toolbox 本身 `#![forbid(unsafe_code)]`（計畫 D4 原本允許 toolbox 一處 unsafe，改放在 libstela，和其他 unsafe 放一起）。
- 視窗的 Memory handle 在映射後就關掉（映射本身留住頁框）。

## 4. `event`

`Event::from_message`：`Key { window, key }`、`Mouse { window, x, y, buttons, kind: Move|Down|Up, clicks }`、`Focus`、`Blur`、`Moved`、`ResizeRequested`、`CloseRequested`、`Menu(menu, item)`、`Error(tag, code)`、`Other(Message)`（不是視窗事件的訊息：`SHELL_READY`、程式自己的訊息）。`window()` 取出事件所屬的視窗。

## 5. `menu`

`MenuBar::add(title, &[items])` → index；`to_text()` 產生 `SET_MENUBAR` 的文字；`item(menu, item)` 查回項目文字。選單的 index 從 0 起（左邊的 STELA 字樣不算）。

## 6. `control`

| 控制項 | 狀態 | 輸入 | 畫 |
|---|---|---|---|
| `Button` | rect、label、pressed | `press(x, y)`、`release(x, y) -> 是否點擊`（按下與放開都在裡面） | 圓角框、置中標籤，按下時反黑 |
| `TextField` | rect、text、cursor、max | `key(&KeyEvent) -> None|Changed|Enter`：字元插入、Backspace、左右鍵 | 白底黑框、插入點 |
| `ScrollBar` | rect、value、total、page | `click(x, y)`：箭頭 ±1、軌道 ±1 頁；`scroll(delta)` | 兩個箭頭、軌道、滑塊（大小依比例） |
| `List` | rect、items、selected、top | `click(x, y, line) -> Option<index>` | 白底、選取反白 |

### 6.1 `text`：多行文字（P11 K32，使用者已於 2026-10-22 審核）

**比喻**：`TextField` 是一張便條，`TextView` 是一本筆記本——可以很多行、會自動換行、可以選一段、可以捲。Classic Mac OS 的 TextEdit 也是這個角色。

- `TextBuffer`（純邏輯）：`text`、`cursor`（byte 位置，永遠在字元邊界）、`anchor`（選取的另一端）、`goal`（上下移動要回到的欄）、一步的 undo。
  - 排版：`rows(cols)` 把文字切成「列」：每個邏輯行（`\n` 分開）按 `cols` 個字元折行；空行也是一列。`position(rows, at)` → (列, 欄)；`index_at(rows, 列, 欄)` 反過來（超出就停在列尾）。折行點屬於下一列，行尾屬於本列。
  - 編輯：`insert`（取代選取）、`backspace`、`delete_forward`、`cut`、`select_all`、`set_text`（開新文件，清掉 undo）。
  - 移動：`move_cursor(Left|Right|Up|Down|Home|End, extend, cols)`；Up／Down 記住目標欄（經過短行再回到長行時回到原來的欄）；不帶 Shift 時左右鍵在選取上會跳到選取的頭或尾。
  - **復原**：一步，但連續打字算一步、連續刪除算一步（換種類、移動游標、貼上都會開新的一步）；undo 之後再 undo 是重做。`is_dirty`／`mark_saved` 給編輯器判斷「未存」。
- `TextView`：`rect`（含右邊 16 像素的捲軸）、`buffer`、`scroll`（第一個顯示的列）、`read_only`。`key`（方向鍵、Home/End、PgUp/PgDn、Shift 選取、Delete、Backspace、Enter、Tab = 4 個空白、Ctrl-A 全選）→ `Ignored|Moved|Changed`，之後自動捲到游標；`press(x, y, shift)`／`drag`／`release`（滑鼠選取；捲軸的點擊）；`scroll_by`；`draw`（選取反白、插入點、捲軸）。字元寬一律一格（等寬字型）；`\t` 畫成一個空白。
- 剪貼簿不在 `TextView` 裡：程式的 Edit 選單用 `selected_text`／`cut`／`insert` 與 `Connection::set_clipboard`／`clipboard`。

### 6.2 `dialog`：對話框（P11 K32）

每個對話框是程式自己的一個視窗（標題列 + 關閉框，伺服器決定位置），自己跑一個事件迴圈直到有答案；這段時間**其他視窗的事件先收起來，結束後 `Connection::put_back` 還回去**（下一次 `next_event` 先拿到它們），所以不會掉事件。Escape 或關閉框 = 取消。

| 函式 | 內容 | 回傳 |
|---|---|---|
| `alert(conn, title, text, buttons)` | 文字（48 欄自動換行，`wrap`）與右下角的按鈕，**最後一個是預設**（粗框、Enter） | 按鈕的 index；取消 `None` |
| `ask_text(conn, title, prompt, default)` | 提示與一個 `TextField`（預設文字、游標在尾端）；Cancel／OK | OK 或 Enter → `Some(text)` |
| `open_file(conn, title, start)` | 上面是目前位置（`Volumes` 或 `Data:/a/b`），下面是清單：卷宗（`Data:`）；資料夾裡第一行 `../`，接著資料夾（`名稱/`）再檔案，各自依名稱（不分大小寫，`sort_entries`）；以 `.` 開頭的不列。雙擊或 Enter：資料夾 → 進去，檔案 → 選它；上下鍵移動 | `Some(Location)` |
| `save_file(conn, title, start, name)` | 同上，多一個檔名欄；Save 回傳目前資料夾 + 檔名（檔名不能空、不能有 `/`、`:`） | `Some(Location)` |

Mouseion 的刪除確認改用 `alert`（`Keep`／`Delete`，Delete 是預設），以前是自己的資訊視窗加 Enter／Escape。

### 6.3 `cjk`：ASCII 以外的字（P11 K39，使用者已於 2026-10-22 審核）

**比喻**：Spleen 是一本只有英文字母的字帖；Fusion Pixel 是一本厚厚的中日韓字典，平常放在書架上（`System:/fonts/`，P14 K4 前是 `Initrd:/fonts/`），第一次遇到不認識的字才翻開。

- 字型：Fusion Pixel 12px Mono zh_TW（OFL 1.1，`third_party/fusion-pixel/`、`third_party/README.md`）。`tools/bdf_to_cjk.py` 把 BDF 轉成 `STF1` 檔：U+0080–U+FFFF 的 35,701 字，每字 28 bytes（碼點、寬 6 或 12 像素、12 列 × 16 位元），依碼點排序（二分搜尋），約 1 MB；放進 initrd 的 `fonts/`（P14 K4 起放在 System 卷宗的 `fonts/`，initrd 不再有）。
- **載入**：`cjk::font()` 第一次被叫到時從 `System:/fonts/fusion-pixel-12-zh_tw.bin`（沒有就 `Initrd:/fonts/`，`libstela::fs::read_system`）讀進 heap（每個畫到非 ASCII 字的程式一份，約 1 MB；沒有檔案就一直是 `None`）。ASCII 從來不問它。
- **寬度**：12 像素寬的字（中日韓）佔**兩格**，6 像素的佔一格；`cjk::cells(c)`、`cjk::width(text)`；`Font::text_width` 用它。
- **畫**：`Canvas::char` 回傳佔了幾格：ASCII 照舊（Spleen）；其他查 Fusion Pixel，8x16 字型時 1 倍（12x12 放在 16 像素高的格子中間），16x32 時 2 倍最近鄰（24x24）；兩種字型都沒有的字畫一個空心框（計畫的 ▯）。`Canvas::text` 依格數前進。
- **用到的地方**：`TextBuffer` 的折行、位置與欄（`rows`／`position`／`index_at` 以格計：寬字兩欄，點到寬字的右半也算那個字）；`TextView` 畫字；`TextField` 的游標位置；terminal 的格子（寬字在第一格、第二格放 `grid::TAIL`，不畫；放不下就換行；複製時略過）；Periplus 的排版（`periplus::cells`）；Mouseion 與所有用 `text` 的地方自動跟著。
- **kernel 主控台與 panic 畫面仍只有 ASCII**（不把 1 MB 的表放進 kernel）。
- 沒有中文輸入法：中文來自檔名與檔案內容。
- 測試：host `cjk`（查表、真的字型檔：`你` 兩格、第二列的筆畫）、`draw`（`中a` 的寬度、置中的像素、缺字的框）、`text`（寬字的折行與欄位換算）、terminal `grid`（寬字、換行、複製）、`periplus`；`make test-shell` 的 `ls /` 印出測試映像裡的 `中文.txt`（`make_test_images.py` 放了一個中文檔名、中文內容的檔）。
- 計畫要擴充 `fb_check.py --expect-text` 比對點陣；這裡改用 host 端以真的字型檔畫字檢查像素（更精確、不依賴視窗位置）——計畫外的決定。

## 7. 範例：`user/echo_win`

視窗（放在畫面右邊）：上面一個 `TextField`，Enter 把一行加到下面的 `List`（並印到 `[1]`：`echo_win: <行>`），`Clear` 按鈕與 `File > Clear` 清空，`File > Quit` 或關閉鈕結束。在 terminal 裡 `run echo_win`。P11：`Edit > Copy`（或 Ctrl-C）把選取的那行（沒有就最後一行）放進剪貼簿、印 `echo_win: copied <行>`；`Edit > Paste`（或 Ctrl-V）把剪貼簿的文字加進輸入框。

## 8. 測試

host `toolbox`：`draw`（矩形運算、填色與裁切、stride 外的像素不動、線、框、1-bit 與 32-bit 圖、複製、文字、字型選擇、缺字）、`event`（各種事件的解碼）、`menu`（文字與查詢）、`control`（按鈕、輸入框、捲軸、清單）、P11 的 `text`（折行、位置換算、打字與復原的分組、選取、上下鍵的目標欄、UTF-8、捲動、滑鼠選取）與 `dialog`（換行、檔案清單的排序與標籤）。`window` 要 syscall，在 `make test-window` 裡經 terminal 與 echo_win 測；P11 起 test-window 在 Echo 裡 Ctrl-C 複製、Ctrl-V 貼回再 Enter，檢查 `echo_win: copied hello` 與第二次的 `echo_win: hello`（剪貼簿經過視窗伺服器的來回）。對話框的互動沒有自動測試（Mouseion 刪除的確認在 `test-mouseion` 經過 `alert` 的 Enter）。

## 9. 已知限制

- 只有 ASCII 字型；沒有比例字、粗體（選單列的 STELA 是畫兩次）、抗鋸齒。
- P11 起有對話框與 `TextView`；沒有焦點在控制項之間的切換（Tab）、非模態的對話框、對話框的鍵盤捷徑（除了 Enter／Escape）。
- 剪貼簿只有純文字；`TextField` 沒有選取（貼上要程式自己做）。
- `Connection` 等回覆時其他事件存在記憶體裡沒有上限。

## P13：輸入法（使用者已於 2026-10-22 審核）

- `Event::Text { window, text }`：IME 送出的字（`TEXT_INPUT`），當作打字插入。
- `TextField::insert_text`、`TextView::insert_text`：插入一串字（唯讀的 TextView 不收）；`TextField::caret(font)`、`TextView::caret()`：游標在視窗座標的位置與高度（中文字算兩格）。
- `Connection::set_caret(window, x, y, h)`：告訴伺服器文字游標在哪裡，組字區畫在它下面。Kalamos 每次重畫、terminal 在游標移動時送。
- 細節見 `docs/ime.md`。

## 7. P13：scale（HiDPI 2x，使用者已於 2026-10-22 審核）

- `Connection::scale`（`CONNECTED` 給的 1 或 2）；`Connection::screen`、`menubar` 是邏輯像素。
- `Window::scale`：緩衝是 `width * scale` × `height * scale`；`Window::canvas()` 給一個 `Canvas::scaled`。
- `Canvas` 的方法都吃邏輯座標；`size`、`bounds`、`clip` 傳回邏輯值；`pixels`、`stride` 是實體緩衝。scale 2 時：8x16 的字改用 16x32 的字型畫在 2 倍的位置（`char_phys`）；`Canvas::icon` 用兩倍大的圖（沒有就放大）；其他一律放大成 2×2 的方塊。`Canvas::image_phys` 不放大（圖示用）。
- 程式不用改：版面用 `font.width`／`font.height` 與邏輯座標排，`font_for(conn.screen.0)` 在 2x 的 2560 螢幕（邏輯 1280）給 8x16。

## 8. P13：powerbox、拖放、桌面圖樣（使用者已於 2026-10-22 審核）

- `dialog::pick_open`／`pick_save` 回傳 `Picked { location, file }`：有 `VOLUMES` 的程式用自己的對話框再開檔，沒有的經視窗伺服器的 powerbox（`docs/sandbox.md` §5）；`dialog::file_dialog`、`ask_text_shown` 多一個「對話框出現了」的回呼（測試等它才打字）；`open_location`。
- `Event::Dropped { window, x, y, file, path }`：Mouseion 拖過來的檔案（`docs/sandbox.md` §6）；`Connection::wait_message(tag, request, timeout)` 等某一則訊息、其他的留著。
- `backdrop`：桌面的圖樣（`on`、`line_color`、`parse`、`paint`；`docs/window.md` §12）。

