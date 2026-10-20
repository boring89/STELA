# USB：xHCI、hub、HID 鍵盤與滑鼠

> P8 由自動排程一次做完，**使用者尚未審核**；之後修改前請先帶使用者過一遍這份文件。

沒有 PS/2 的新電腦也能用鍵盤與滑鼠：xHCI 主控制器、USB 2.0 hub、HID boot protocol 的鍵盤與滑鼠、熱插拔。設計參考 68k_HAL 的 `docs/usb.md`（只參考設計與數值，程式碼重寫），規格是 *eXtensible Host Controller Interface for USB 1.2*（以下「xHCI §」）、*USB 2.0 Specification*（「USB §」）、*Device Class Definition for HID 1.11*、*HID Usage Tables 1.12*。

程式碼：

```
drivers/src/usb/mod.rs    SETUP 封包、描述子、hub 的 port 狀態、速度、interrupt 間隔（純邏輯，host 測試）
drivers/src/usb/hid.rs    boot 鍵盤報告 → KeyEvent、滑鼠報告 → MouseEvent（純邏輯，host 測試）
drivers/src/usb/trb.rs    TRB 的編碼、生產者／消費者的環狀位置（純邏輯，host 測試）
drivers/src/usb/xhci.rs   控制器：啟動、命令、控制傳輸、列舉、HID、hub、熱插拔
kernel/src/usb.rs         找控制器、給它一個 kernel 執行緒（usb）
```

## 1. 比喻：郵局與小紙條

USB 是一棵樹：根是主控制器（PCI 裝置），樹枝是 hub，樹葉是裝置。只有主控制器能開口，裝置永遠是被問的一方。

xHCI 控制器像一個郵局：軟體把要辦的事寫在 16 bytes 的**小紙條**（TRB）上，放進**環狀的信箱**（ring），按**門鈴**（doorbell）；郵局把做完的事寫在另一個環（事件環）並響鈴（MSI 中斷）。每張紙條有一個 **cycle 位元**表示「這張現在歸誰」：生產者每繞一圈就把「有效」的意義翻轉一次，所以不需要頭尾指標暫存器。

```
 軟體                                          xHCI
 命令環 ──[Enable Slot][Address Device]…────▶   （doorbell 0）
 EP0 環 ──[Setup][Data][Status]…─────────────▶   （doorbell slot，target 1）
 中斷環 ──[Normal]（等一份報告）──────────────▶   （doorbell slot，target DCI）
 事件環 ◀──[Command Completion][Transfer][Port Status Change]── MSI
```

## 2. 名詞

| 名詞 | 意思 |
|---|---|
| TRB | Transfer Request Block：16 bytes（parameter u64、status u32、control u32），control 的 bit 0 是 cycle、bits 15:10 是類型 |
| slot | 控制器替每個裝置配的編號（Enable Slot），1 起 |
| device context | 控制器維護的裝置狀態：slot context + 31 個 endpoint context；DCBAA[slot] 指向它 |
| input context | 軟體填好、給 Address Device／Configure Endpoint／Evaluate Context 的版本；開頭是 input control context（drop／add 旗標） |
| DCI | device context index：endpoint 號 × 2 + IN；EP0 是 1 |
| route string | 從 root port 往下，每層 hub 一個 4 位元的 port 編號 |
| boot protocol | HID 的簡化報告格式（鍵盤 8 bytes、滑鼠 3–4 bytes），BIOS 也看得懂 |

## 3. 啟動（`xhci::probe` → `start`，xHCI §4.2）

1. PCI 找 class 0C／03／30 的第一個裝置；映射 BAR 0、bus master。
2. 讀 capability：CAPLENGTH（operational 的位移）、HCSPARAMS1（MaxSlots、MaxPorts）、HCSPARAMS2（scratchpad 數）、HCCPARAMS1（CSZ：context 是 32 還是 64 bytes；xECP）、DBOFF、RTSOFF。
3. **向韌體取回控制權**（xHCI §7.1 USB Legacy Support）：xECP 鏈裡 id 1 的 capability：設 OS-owned（bit 24）、等 BIOS-owned（bit 16）清掉、關掉 SMI。QEMU 沒有這個 capability。
4. 停止（USBCMD.R/S = 0，等 HCH）、重設（HCRST，等它與 CNR 清掉）、CONFIG.MaxSlotsEn = min(MaxSlots, 32)。
5. DCBAA（一頁）；scratchpad（控制器要求的數量，各一頁，陣列放 DCBAA[0]）；命令環（一頁 256 個 TRB，最後一個是 Link）→ CRCR（RCS = 1）；事件環（一頁 256 個）與 ERST（一個 segment）→ ERSTSZ、ERDP、ERSTBA。
6. 中斷：MSI（`pci::system::enable_msi`）、IMAN.IE、IMOD = 1000（250 µs）、USBCMD.INTE；然後 R/S = 1。沒有電的 port（PPC）給電。
7. kernel 的 `usb::start` 開一個 `usb` 執行緒跑 `xhci::run`。

```
usb: xhci0 (0000:00:03.0): xHCI 1.0, 8 ports, 32 slots, 32-byte contexts, vector 51
```

## 4. 單一執行緒的服務迴圈（`run`）

```
 啟動後 100 ms：掃每個 CCS = 1 的 port → attach_root
 loop：
   service：取完事件環（Port Status Change → 記下 port；Transfer → HID 報告）
            處理有變的 root port（插上 → 列舉；拔掉 → detach）
            每 300 ms 輪詢 hub 的所有 port
   等 MSI 或 50 ms
```

- 命令與控制傳輸在等待時**自己處理事件環**（`wait_event`）：不是在等的事件照常分派（HID 報告照樣進輸入佇列），所以列舉途中鍵盤不會停。
- 中斷處理程式只清 IMAN.IP、叫醒執行緒，不碰環。
- ERDP 每處理一個事件就前進，並寫 EHB（bit 3）清掉 busy。

## 5. 環（`trb.rs`）

- `Producer`：寫到最後一個可用的位置後，把 Link TRB 以**舊的** cycle 交出去，自己從頭開始、cycle 翻轉（Link 的 TC 旗標叫控制器也翻）。新環的 Link 一開始 cycle 0（還不屬於控制器）。
- `Consumer`（事件環）：cycle 相同的才是新的；到尾端回到 0、翻轉。
- 每個 TRB 整塊 16 bytes 寫入；x86 的 DMA 與快取一致，寫完按門鈴即可。

## 6. 列舉（`enumerate`、`setup_device`）

```
 Enable Slot ──► slot
 配：output context、input context、EP0 環、一頁資料緩衝；DCBAA[slot] = output
 input：add A0|A1；slot context（route、speed、context entries 1、root port、TT）；
        EP0 context（type 4 control、CErr 3、max packet = 速度的預設值、TR dequeue | DCS、avg TRB 8）
 Address Device（BSR 0：控制器送 SET_ADDRESS）
 GET_DESCRIPTOR(device, 8) ──► bMaxPacketSize0（SuperSpeed：2^n）；不同就 Evaluate Context（add A1）
 GET_DESCRIPTOR(device, 18)、(configuration, 9) ──► wTotalLength、(configuration, 全部 ≤ 1024)
 SET_CONFIGURATION
 hub（class 9）──► §8；HID boot（class 3、subclass 1、protocol 1/2）──► §7；其他只印出來
```

- 控制傳輸：Setup TRB（immediate data、transfer type 0/2/3）、Data TRB（DIR、ISP）、Status TRB（方向與資料相反；沒有資料就 IN；IOC），按 doorbell(slot, 1)，等 status TRB 的 Transfer Event（成功或 short packet）。
- 失敗就 Disable Slot、印原因。

```
usb: 5: keyboard (0627:0001, high speed)
usb: 6: mouse (0627:0001, high speed)
usb: 7.1: keyboard (0627:0001, full speed)
usb: 7: hub (0409:55aa, full speed)
```

裝置的名字是路徑：root port，接著每層 hub 的 port（`7.1`）。QEMU 的 `qemu-xhci`：xHCI port 1–4 是 USB 3、5–8 是 USB 2，所以 `-device usb-kbd` 出現在 port 5。

## 7. HID（`hid.rs`）

- `SET_PROTOCOL(boot)`、`SET_IDLE(0)`（失敗不管）；Configure Endpoint：add A0 | 1 << DCI、slot 的 context entries = DCI、endpoint context：type 7（interrupt IN）、CErr 3、max packet、**interval**（低／全速：bInterval 是 frame（1 ms = 8 microframe），interval = log2(8 × bInterval)，限制在 3–10；高速：bInterval − 1；xHCI §6.2.3.6）、TR dequeue、avg TRB length 與 max ESIT payload = max packet。
- 一次一個 Normal TRB（IOC、ISP）；完成時讀報告、再排一個、按 doorbell。
- 鍵盤：8 bytes（修飾鍵位元、保留、最多 6 個按著的 usage）。和上一份比：先放開、再修飾鍵變化、最後按下（所以 Ctrl+C 是 Ctrl 先、C 後）；全部是 1（ErrorRollOver）不變；Caps Lock 自己記著翻轉。usage → `abi::key::KeyCode`（Keyboard/Keypad page 0x07：0x04–0x1D 字母、0x1E–0x27 數字、0x28 Enter……），沒名字的鍵是 `KeyCode::Unknown(0x200 | usage)`。和 PS/2 同一套 `KeyEvent`，送進 `frame::input`，上面的層不知道鍵盤是哪一種。
- 滑鼠：按鈕、dx、dy（i8，USB 的 dy 本來就是往下為正）、可選的滾輪 → `MouseEvent`。
- 每個 HID 裝置第一份報告到時印 `usb: <路徑>: first report`（測試用）。
- 出錯（stall）就不再輪詢這個 endpoint（沒有 Reset Endpoint 的復原）。

## 8. hub（USB §11）

- GET_DESCRIPTOR（class，type 0x29）→ port 數、bPwrOn2PwrGood（× 2 ms）。
- Configure Endpoint（只 add A0）：slot context 的 Hub 位元（bit 26）與 Number of Ports——讓控制器知道這是 hub。
- 每個 port SET_FEATURE(PORT_POWER)，等 power good（至少 20 ms）。
- **輪詢**（不用 hub 的狀態變化 interrupt endpoint：簡單，代價是最多 300 ms 的延遲）：每個 port GET_STATUS；連線變化就 CLEAR_FEATURE(C_PORT_CONNECTION)；有東西又還沒有裝置 → SET_FEATURE(PORT_RESET)、每 10 ms 看一次直到 reset 完成（最多 500 ms）、CLEAR_FEATURE(C_PORT_RESET)、從狀態取得速度（bit 9 低速、bit 10 高速、否則全速）→ 列舉：route string = hub 的 route | port << (4 × hub 的深度)、root port 同 hub、低／全速裝置接在高速 hub 後面時 TT = (hub slot, port)。深度上限 5。
- 拔掉 hub 會遞迴移除後面的裝置。

## 9. 熱插拔

- root port：Port Status Change 事件只記下 port；服務迴圈讀 PORTSC、清掉變化位元（寫回時只保留 RO／RW 位元，不碰 PED 與其他 RW1C 位元：`PORT_KEEP`）；接上 → reset（USB 2 port；USB 3 自己訓練）→ 列舉；拔掉 → detach。
- detach：先孩子、再 Disable Slot、DCBAA[slot] = 0、放掉 DMA 記憶體、印 `usb: <路徑>: removed`。
- `xhci::devices()`：目前的裝置清單（路徑、種類、slot），給之後的工具與測試。

## 10. 測試

| 測試 | 內容 |
|---|---|
| host `drivers::usb` | SETUP 封包（標準、HID、hub）、device／configuration 描述子（含零長度描述子不會無限迴圈）、interval、速度、hub port 狀態 |
| host `drivers::usb::hid` | usage 對應、打字（放開、Shift 與數字同一份報告、Ctrl+C、一起放開、ErrorRollOver、Caps Lock、沒名字的鍵）、滑鼠 |
| host `drivers::usb::trb` | 命令、Setup／Data／Status、Link、Normal 的編碼、事件欄位、生產者經 Link 繞回、消費者翻轉 |
| `make test-usb` | 一般 kernel、`-machine q35,i8042=off`（沒有 PS/2）、`qemu-xhci` + `usb-kbd` + `usb-mouse` + `usb-hub`（後面一個 `usb-kbd`）：等 hub 列舉完 → QMP 打 `uptime`，terminal 裡的 shell 回 `uptime:`（USB 鍵盤 → 視窗伺服器 → terminal → shell）→ `input-send-event` 移動滑鼠 → `usb: 6: first report` → `device_del` 滑鼠（`usb: 6: removed`）→ `device_add` 滑鼠（`usb: 8: mouse`）→ `device_del` hub 後面的鍵盤（`usb: 7.1: removed`）→ `device_add` 到 hub 的 port 2（`usb: 7.2: keyboard`） |

`tools/qemu_test.py --qmp TRIGGER COMMAND JSON` 送任意 QMP 命令。

## 11. 已知限制

- 只用第一個 xHCI 控制器；沒有 EHCI／OHCI／UHCI。
- 只有 USB 2.0 hub 的 port 列舉（SuperSpeed hub、多 TT、hub 的 interrupt endpoint 都沒有；QEMU 的 `usb-hub` 是全速 hub，所以 TT 的路徑沒測到）。
- 只有 HID boot protocol（不解析 report descriptor：多媒體鍵、絕對座標的平板、遊戲手把都不支援）；沒有 LED（Caps Lock 燈）、按住重複。
- 沒有 USB 儲存、音訊、isochronous、電源管理（U1/U2、suspend）。
- endpoint stall 不復原；控制傳輸一次 4 KiB 以內；裝置路徑最多 5 層。
- 沒有在實機上驗證。
