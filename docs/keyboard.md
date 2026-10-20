# 鍵盤與輸入事件：i8042、掃描碼 set 1、KeyCode、事件佇列

> P2 由自動排程一次做完，使用者已於 2026-10-20 審核。P4 的修改（`KeyCode`／`KeyEvent`／`key_to_char` 搬到 `abi::key`、notify hook、input-router 與使用者程式 echo 取代 kernel 主迴圈，§7）使用者已於 2026-10-20 審核。

程式碼：`frame/src/arch/x86_64/i8042.rs`（控制器）、`frame/src/input/keyboard.rs`（解碼，純邏輯）、`abi/src/key.rs`（KeyCode、KeyEvent 與其 4 bytes 編碼、字元，純邏輯；P4 從 frame 搬來，使用者程式也用）、`frame/src/input/queue.rs`（ring buffer，純邏輯）、`frame/src/input/mod.rs`（中斷處理、全域佇列、notify hook）、`kernel/src/input.rs`（input-router）、`user/echo`、`tools/qemu_test.py`（QMP 打字）。

## 1. 比喻：打字員、翻譯與信箱

- **鍵盤**每按一下、放開一下，就丟出一兩個位元組（**掃描碼**），只說「第幾號鍵」，不說是什麼字。
- **i8042 控制器**是收件窗口：收到位元組放在窗口（port `0x60`），同時按鈴（IRQ 1）。
- **解碼器**是翻譯：把「0x1F」翻成「S 鍵按下」，並記住 Shift 有沒有按著。
- **事件佇列**是信箱：中斷處理程式把「S 鍵按下」投進去，主迴圈有空時再拿出來。信箱滿了就丟掉最舊的信。
- **`key_to_char`** 是最後一步：依 US 鍵盤配置，把「S 鍵按下 + Shift」變成 `'S'`。

```
 鍵盤 ──► i8042 (0x60) ──IRQ 1──► I/O APIC ──向量 33──► keyboard_interrupt
                                                         │ 讀 0x60
                                                         ▼
                                                   Decoder（set 1）
                                                         │ KeyEvent
                                                         ▼
                                     EventQueue<InputEvent, 256>  ◄── 之後 USB HID（P8）也推這裡
                                                         │ pop()
                                                         ▼
                     （P4）input-router → INPUT_KEY 訊息 → user/echo：key_to_char → 印
```

## 2. 名詞

| 名詞 | 意思 |
|---|---|
| PS/2 | IBM PS/2（1987）的鍵盤／滑鼠介面；現在多數主機板用 USB，但韌體常模擬 |
| i8042 | PS/2 控制器晶片（現在整合在晶片組裡），兩個 port：1 = 鍵盤、2 = 滑鼠 |
| 掃描碼（scan code） | 鍵盤送出的「鍵號」；有 set 1、2、3 三種編碼 |
| make／break code | 按下時送的碼／放開時送的碼（set 1：break = make \| 0x80） |
| translation | i8042 把鍵盤原生的 set 2 翻成舊的 set 1 再交給 CPU（預設開啟） |
| ring buffer | 頭尾相接的固定大小陣列，寫到底就繞回開頭 |

## 3. i8042 控制器（OSDev "8042 PS/2 Controller"）

```
port  讀        寫
0x60  資料      資料（給裝置，或命令的參數）
0x64  狀態      對控制器的命令

狀態 bit 0  輸出緩衝區滿（0x60 有一個位元組等著讀）
     bit 1  輸入緩衝區滿（先別寫）
     bit 5  這個位元組來自 port 2（滑鼠）

設定位元組（命令 0x20 讀、0x60 寫）
     bit 0  port 1 中斷（IRQ 1）       bit 4  port 1 時脈關閉
     bit 1  port 2 中斷（IRQ 12）      bit 5  port 2 時脈關閉
     bit 6  translation（set 2 → set 1）
```

`i8042::init` 的步驟：

```
狀態讀到 0xFF       → 匯流排浮空 = 沒有控制器，回報錯誤（kernel 繼續，只是沒有鍵盤）
0xAD、0xA7         停用兩個 port
flush              丟掉緩衝區裡的殘留
0x20 → config      讀設定，清掉兩個中斷位元，0x60 寫回
0xAA → 0x55?       控制器自我測試（有些晶片會因此重設，所以之後再寫一次設定）
0xAB → 0x00?       port 1 測試
0xAE               啟用 port 1
鍵盤 0xF4 → 0xFA?  開始掃描（通常本來就開著）
config |= IRQ1 | translation | port 2 時脈關；清 IRQ12 與 port 1 時脈關 → 寫回
```

**踩過的坑**：設定位元組是在 port 1 停用（0xAD）之後讀的，所以它的 bit 4（port 1 時脈關閉）是 1；最後寫回時如果沒清掉，鍵盤就又被關掉了——QEMU 收得到 `send-key`，但 kernel 一個位元組也收不到。

所有輪詢最多 100,000 次（實機約 0.1 秒）就放棄，不會卡死。

之後 `input::init` 註冊向量 33、經 I/O APIC 路由 IRQ 1，再清一次輸出緩衝區：IRQ 1 是**邊緣觸發**，如果路由好之前就有位元組進來，那次「按鈴」已經錯過；只要位元組一直留在窗口，控制器就不會再按鈴。

中斷處理程式：只要狀態 bit 0 是 1 就讀 `0x60`（bit 5 是 1 的位元組來自滑鼠，丟掉），每個位元組交給解碼器。

## 4. 掃描碼 set 1（OSDev "PS/2 Keyboard"）

QEMU 的鍵盤原生是 set 2，i8042 的 translation 把它翻成 set 1。STELA **不切換** set（T7）。

```
按 S、放開 S：             1F  9F
按 Shift、A、放開：         2A  1E  9E  AA          → 'A'
方向鍵上（按、放）：        E0 48  E0 C8            ← E0 = 延伸鍵前綴
右 Alt：                    E0 38
PrintScreen（按）：         E0 2A  E0 37            ← E0 2A 是「假 Shift」，忽略
Pause（只有按，沒有放開）： E1 1D 45 E1 9D C5
```

`Decoder::feed(byte)` 的狀態：`extended`（上一個是 E0）、`pause_left`（E1 之後還要吞幾個位元組）、8 個修飾鍵各自的按下狀態（左右分開記，對外合併成 `shift`、`ctrl`、`alt`、`meta`）、Caps Lock（按下時切換）。

忽略的位元組：`0x00`、`0xEE`、`0xFA`（ACK）、`0xFC`–`0xFF`（錯誤、resend）。**不**忽略 `0xAA`：它雖然是鍵盤自我測試成功的回覆，但在 set 1 也是「左 Shift 放開」。

## 5. KeyCode、KeyEvent、key_to_char

```rust
pub struct KeyEvent {
    pub code: KeyCode,        // 鍵的「位置名稱」（US 配置），不是字元
    pub pressed: bool,        // 按下 / 放開
    pub modifiers: Modifiers, // 這個事件之後的 shift、ctrl、alt、meta、caps_lock
}
```

`KeyCode` 是 STELA 自己的列舉：`A`…`Z`、`Digit0`…`Digit9`、`F1`…`F12`、`Enter`、`LeftShift`、`Up`…（88 個有名字的鍵），用鍵的位置命名，與掃描碼無關——之後 USB HID 的 usage code 也轉成同一套。數字鍵台、多媒體鍵目前是 `Unknown(make)`（E0 開頭的加 0x100）。

`key_to_char(&event) -> Option<char>`（只看「按下」）：

| 鍵 | 結果 |
|---|---|
| 字母 | Shift XOR Caps Lock → 大寫；**Ctrl + 字母 → 控制字元**（Ctrl-C = 0x03） |
| 數字與符號 | 有 Shift 時用 US 配置的上排字元（`1` → `!`、`/` → `?`） |
| Enter／Tab／Backspace／Escape | `\n`、`\t`、0x08、0x1B |
| 其他（F 鍵、方向鍵…） | `None` |

不做：數字鍵台、LED（Caps Lock 燈）、按住重複（typematic）、其他鍵盤配置。

## 6. 事件佇列（`frame::input`）

```rust
pub enum InputEvent { Key(KeyEvent) }    // 之後加滑鼠等
input::push(event);                      // 驅動／中斷處理程式
input::pop() -> Option<InputEvent>;      // kernel 主迴圈
input::dropped() -> u64;                 // 滿了被丟掉的數量
```

`EventQueue<T, N>`：`head`（最舊的位置）、`len`。push 寫在 `(head + len) % N`；滿了先把 `head` 往前推一格（丟最舊）並計數。容量 256。

```
N = 4，push 1 2 3 4 5：
  [1 2 3 4] head=0 len=4  → push 5 時丟 1 → [5 2 3 4] head=1 len=4，dropped=1
  pop → 2, 3, 4, 5
```

鎖：佇列是 `SpinLock`。中斷處理程式在 IF=0 時 push；主迴圈 pop 時 SpinLock 會關中斷，所以中斷不可能在主迴圈持鎖時進來 push（`docs/irq.md` §11）。

## 7. 事件的去向（P4 起）

P2–P3 時 kernel 的 `init` 執行緒每 4 ms 輪詢佇列並 echo。P4 起：`frame::input::push` 之後呼叫 kernel 註冊的 notify hook（中斷裡，只叫醒 `input-router` 執行緒）；router 把事件編成 4 bytes 的 `INPUT_KEY` 訊息送到 `InputAttach` 的 port；使用者程式 `user/echo` 收到後 `key_to_char`、印字（控制字元印成 `^X`），Enter 印 `uptime: 0.307 s`。細節見 `docs/ipc.md` §8。selftest 不啟動 router（"keyboard" 測試直接 `pop`）。

## 8. 測試

| 測試 | 內容 |
|---|---|
| host `input::keyboard` | make/break、"stela\n"、Shift／右 Shift／Caps Lock、Ctrl 字母、E0 鍵、PrintScreen 與 Pause、回覆位元組、Enter/Tab/Backspace/Escape |
| host `input::queue` | FIFO、滿了丟最舊、與 `VecDeque` 模型比對 1000 輪 |
| selftest "keyboard" | IRQ 1 的 I/O APIC 項目 = 向量 33 且沒遮蔽；印 `input: waiting`；5 秒內收到 `stela\n`、沒有丟事件；印 `key: S 's'` … `input: OK` |
| `make test-fb` | 一般 kernel：P6 起看到 `STELA shell` 後打 `uptime`（`qemu_test.py --type`），等 `uptime:` 再 screendump；`make test-shell` 打一整串 shell 指令 |

`tools/qemu_test.py --send-keys TRIGGER KEYS`：在 QEMU 命令加上 `-qmp unix:build/qmp-<pid>.sock,server=on,wait=off`；serial 出現 TRIGGER 那一行時，連上 QMP（QEMU Machine Protocol，JSON）、送 `qmp_capabilities`，再對每個鍵送 `send-key`（QKeyCode 名稱，例如 `s`、`ret`；按住 50 ms、間隔 150 ms）。腳本讀 serial 改用 `select` 加逾時，guest 卡住不印字時也會在時限內失敗。

## 9. 已知限制與待辦

- 很多新電腦（尤其筆電、沒有 legacy 支援的 UEFI 機器）沒有 i8042，或只靠 SMM 模擬：那時 `i8042: no PS/2 keyboard`，要等 P8 的 USB HID。ACPI FADT 的 `IAPC_BOOT_ARCH` bit 1（有沒有 8042）沒有檢查。
- 只有 US 配置；沒有數字鍵台、LED、按住重複、滑鼠（port 2 關閉）。
- 沒有重設鍵盤（0xFF）；假設韌體留下的狀態是正常的。
- 一次只有一個 `InputAttach` 的接收者；之後由 shell（P6）、視窗伺服器（P8）決定前景。
