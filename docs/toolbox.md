# UI 工具箱（toolbox）

> P8 由自動排程一次做完，**使用者尚未審核**；之後修改前請先帶使用者過一遍這份文件。

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

- `Connection::open()`／`open_with(handle)`：送 `CONNECT`（夾帶主 port 的 SEND）、等 `CONNECTED`（5 秒）；等回覆時收到的其他訊息存起來，之後由 `next_event` 依序交出；`PING` 直接略過。
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

## 7. 範例：`user/echo_win`

視窗（放在畫面右邊）：上面一個 `TextField`，Enter 把一行加到下面的 `List`（並印到 `[1]`：`echo_win: <行>`），`Clear` 按鈕與 `File > Clear` 清空，`File > Quit` 或關閉鈕結束。在 terminal 裡 `run echo_win`。

## 8. 測試

host `toolbox`：`draw`（矩形運算、填色與裁切、stride 外的像素不動、線、框、1-bit 與 32-bit 圖、複製、文字、字型選擇、缺字）、`event`（各種事件的解碼）、`menu`（文字與查詢）、`control`（按鈕、輸入框、捲軸、清單）。`window` 要 syscall，在 `make test-window` 裡經 terminal 與 echo_win 測。

## 9. 已知限制

- 只有 ASCII 字型；沒有比例字、粗體（選單列的 STELA 是畫兩次）、抗鋸齒。
- 沒有對話框、捲動文字區的現成控制項（viewer 自己做）、焦點在控制項之間的切換（Tab）。
- `Connection` 等回覆時其他事件存在記憶體裡沒有上限。
