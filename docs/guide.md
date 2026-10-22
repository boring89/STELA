# STELA 使用手冊

> P13 I（K24）由自動排程寫成，使用者已於 2026-10-22 審核。英文版：`docs/guide.en.md`。STELA 裡也看得到：Periplus 的首頁有連結（`Initrd:/doc/guide.html`）。

STELA 是一套從零寫起的作業系統（x86-64、UEFI）：像 Classic Mac OS 一樣簡單、用空間的方式管檔案，同時有現代系統的功能。這份手冊說明怎麼用它。每一段最後的「試試看」都可以在 QEMU 裡重現：在原始碼目錄打 `make run`（沒有 Data 卷宗，提示符是 `Initrd:/>`），或用 `make disk-image` 做一顆有 Data 卷宗的映像（`docs/p520.md`）。

## 開機與 Setup

開機時畫面中間是一塊石碑（STELA 的標誌）。接著出現桌面：上面是選單列，右邊是卷宗的圖示，左上角開著一個 Terminal。

- 第一次用一顆新磁碟開機時，**Setup.app** 會先出來，一頁一頁問：語言（English、繁體中文）、時區、Data 卷宗（沒有時選一個分割區，打它的完整名稱確認後格式化）、網路、要裝的套件、要複製進來的 Classic 檔案，最後啟動桌面。Setup 只用鍵盤：方向鍵選、空白鍵勾選、Enter 下一步、Esc 上一步。
- 卷宗：`Initrd`（系統本身，唯讀）、`Data`（你的檔案）、`Boot`（開機用的 FAT 分割區，唯讀）、`Tmp`（記憶體，關機就沒了），插上的 USB 隨身碟也會出現。
- 開機選項寫在 ESP 的 `STELA/boot.cfg`（一行一個），見「疑難排解」。

試試看：`make run`，等石碑換成桌面，在 Terminal 打 `vols` 看有哪些卷宗。

## 桌面與 Mouseion

**Mouseion** 是 STELA 的檔案管理員（名字來自古代亞歷山卓的學院）。它記得每個資料夾視窗的位置、大小與圖示的擺放：關掉再打開，一切都在原來的地方。

- **雙擊**卷宗或資料夾打開它的視窗；雙擊 `.app` 的程式（bundle）或程式檔執行它；雙擊文字檔用 Kalamos 打開，其他文件用 Viewer。
- **拖曳**圖示：在同一個視窗裡是移動位置；拖到資料夾、卷宗或別的視窗是搬移（同一個卷宗）或複製（不同卷宗）；拖到**垃圾桶**是丟掉（Special > Empty Trash... 才真的刪）；拖到別的程式的視窗（例如 Kalamos）就是把那個檔案交給它。
- 點一下選取，Shift-點選加選，在空白處拉框選多個；點已選取項目的名字可以改名。
- **選單**：File > New Folder（Alt-N）、Open（Alt-O）、Find...（Alt-F：找名字含有某些字的項目，雙擊結果打開它所在的資料夾）、Get Info（Alt-I）、Rename...、Set Type...（Mac 的 type 與 creator）、Close（Alt-W）；View > Icons／List（清單的欄位標題可以點來排序）；Special > Terminal、Monitor、Sleep Display（立刻熄螢幕）、Empty Trash...、Restart、Shut Down；Mouseion > Preferences...（桌面圖樣：素色、回紋、波浪）。
- **視窗**：拖標題列移動，拖右下角改大小，左上的方塊關閉，右上的方塊（zoom box）放大到合適的大小再按一次回來。
- **鍵盤**：Alt 加字母是選單項目的快捷鍵（選單上寫的 `^N` 就是 Alt-N）；Alt-M 用鍵盤打開選單（左右換選單、上下換項目、Enter 選、Escape 收回）；Alt-Tab 在視窗之間切換（按著 Alt 再按 Tab 選，放開 Alt 就切過去）。
- **選單列右邊**：喇叭（點開調音量與靜音）、注音輸入的「注／英」、時鐘。
- 一段時間沒有操作（預設 10 分鐘，`Data:/.stela/display` 的 `blank=分鐘`，0 是不熄）螢幕會變黑；按任何鍵或動滑鼠就回來（那一下不會變成點擊）。

**程式在自己的房間裡跑**：從 Mouseion 啟動的程式只看得到自己的「容器」資料夾（`Data:/Programs/程式名/container/`）。要讓它開別的檔案，用它的 File > Open（出來的對話框是系統畫的，你選哪個它才拿到哪個）、把檔案拖進它的視窗，或在 Mouseion 雙擊文件。Terminal、Classic、Setup 與 shell 啟動的程式不受這個限制。細節見 `docs/sandbox.md`。

試試看（有 Data 的映像）：雙擊右邊的 Data，再雙擊 `welcome.txt`——Kalamos 打開它；把它拖到垃圾桶，再從垃圾桶拖回來。

## Terminal 與 shell

Terminal 視窗裡是 STELA 的 shell，提示符是目前的位置（`Data:/>`）。

| 指令 | 做什麼 |
|---|---|
| `ls [路徑]`、`cd [路徑]`、`pwd` | 列出資料夾、換資料夾、目前在哪 |
| `cat 檔案`、`cp 來源 目的`（`cp -r` 整個資料夾）、`mv`、`mkdir`、`rm` | 檔案操作 |
| `run 程式 [參數]` | 執行程式或 `Name.app`（目前資料夾，然後 `Initrd:/`）；Ctrl-C 停止它；直接打程式名字也可以 |
| `ps`、`mem`、`uptime`、`vols`、`hwinfo` | 行程、記憶體、開機多久、卷宗、硬體 |
| `ifconfig`、`ping`、`fetch` | 網路 |
| `tone [Hz [ms]]` | 發出一個音 |
| `shutdown`、`reboot` | 關機、重新開機 |
| `help` | 全部的指令 |

- **路徑**：`Volume:/a/b`、`/a/b`（這個卷宗）或 `a/b`，可以用 `.` 與 `..`。
- **管線與重新導向**：`a | b`、`a > 檔案`、`a >> 檔案`、`a < 檔案`；後面加 `&` 在背景跑，`jobs`、`fg N`、`kill N` 管理背景工作（只接程式，不接內建指令）。
- 上下鍵叫回之前的指令、左右鍵移動游標；**Tab** 補全名字；`*`、`?` 比對名字（`ls *.txt`）。
- `Initrd:/bin/` 有一般的命令列工具：`echo cat grep wc head tail sort uniq find du df date sleep touch stat hexdump tar true false`。
- Terminal 支援顏色（SGR）；拖曳選取文字後 Edit > Copy 放進剪貼簿，Edit > Paste 把剪貼簿的文字打給 shell（Terminal 裡的 Ctrl-C 是停止程式）。

試試看：`make run`，在 Terminal 打 `help`、`ls /bin`、`echo hello | wc -c`、`run Echo.app`。

## 安裝軟體

**pkg** 從套件伺服器安裝軟體（簽章驗證過才裝）：

```
pkg update               拿新的目錄（Data:/.stela/pkg.conf 的 server = https://…）
pkg search 字            找
pkg install 名稱         安裝（連同它需要的）
pkg list                 已安裝的
pkg remove 名稱          移除
```

裝好的東西在 `Data:/Programs/名稱/`；shell 會在 `Data:/Programs/*/bin/` 找程式。現在的簽章鑰匙是**測試用的**，正式的套件伺服器還沒有（`docs/pkg.md`）。

**ports**：python3、git、vim、nano、curl、sqlite、lua、tcc、busybox 等 22 個軟體已經能在 STELA 上建置並執行（Linux 相容層，`docs/ports.md`）。

## 應用程式

- **Kalamos**（文字編輯器）：File > New（Ctrl-N）、Open...（Ctrl-O）、Save（Ctrl-S）、Save As...（Ctrl-Shift-S）、Close（Ctrl-Q/W）；Edit 有 Undo、Cut、Copy、Paste、Select All；View 換字型大小。沒存就關會先問。
- **Periplus**（瀏覽器）：Ctrl-L 打網址、Enter 前往；Tab／Shift-Tab 選連結、Enter 跟過去；Alt-Left／Alt-Right 上一頁／下一頁；支援 HTTP 與 HTTPS（TLS 1.3），HTML 只有一部分。本機檔案用 `Data:/路徑` 或 `Initrd:/doc/guide.html` 這樣的位址。
- **Monitor**（系統監視器，Special > Monitor）：行程、記憶體、CPU；可以結束行程。
- **Viewer**：唯讀的文字檢視。
- **Echo**：工具箱的範例程式。

試試看：`make run`，打 `run Periplus.app`，在首頁 Tab 到「STELA 使用手冊」按 Enter。

## Classic 環境

Classic.app 在視窗裡跑一台 68k Macintosh（Mac IIci），執行**你自己的** ROM 與 System（STELA 不附任何 Apple 的東西）：

```
Data:/Classic/ROM          Mac IIci 的 ROM（512 KiB）
Data:/Classic/System.img   HFS 磁碟映像（例如 System 7.5.3）
Data:/Classic/classic.cfg  （可選）ram=32、resolution=800x600、depth=8、scale=2、disk2=…
```

雙擊 `Initrd:/Classic.app`（或 `run Classic.app`）。鍵盤：**Alt = Command**、Ctrl = Control、Windows 鍵或右 Alt = Option；Backspace 是 delete。Mac 的桌面上會出現 STELA 的 Data 卷宗，可以直接交換檔案。Mac 裡選 Special > Shut Down 就結束 Classic.app。細節見 `docs/classic.md`。

## Linux 程式

STELA 能執行 x86-64 Linux 的程式（靜態或動態連結 musl 的、PIE）：直接打名字或 `run 程式`。程式看到的檔案系統：`/Data/…`、`/Initrd/…`、`/tmp`、`/dev`、`/proc`、`/lib`。支援執行緒、fork、訊號、pty、socket（`docs/linux.md`）。

試試看：`make run`，打 `run linux/hello a b`、`run linux/hello-std`。

## 注音輸入

按 **Ctrl-Space** 切換中文（選單列顯示「注」）與英文（「英」）。用標準的注音鍵盤打字，例如 `su3cl3` 是「你好」：組字時字出現在游標下的小框裡，按下（↓）開候選窗、數字鍵選字，Enter 送出。Shift 加字母打英文字母。用 libchewing（新酷音的引擎），你選過的詞會記在 `Data:/.stela/ime/`（`docs/ime.md`）。Kalamos、Periplus、Terminal 與對話框都能打中文。

## 網路與聲音

- 開機時自動用 DHCP 拿位址（有線網卡：Intel e1000／e1000e／I219、virtio-net；USB 網卡：CDC ECM／NCM／RNDIS、AX88179）。`ifconfig` 看位址，`ping 10.0.2.2`，`fetch https://example.com/`。
- 聲音（Intel HD Audio）：幾個程式可以同時發聲；選單列的喇叭調主音量與靜音，會記住。`tone 440 1000` 試聽。

## 關機

Mouseion 的 Special > Shut Down 或 Restart，或在 shell 打 `shutdown`、`reboot`。關機前卷宗會正常卸載（下次開機不用檢查）。

## 疑難排解

`STELA/boot.cfg`（ESP 上，一行一個選項）：

| 選項 | 用途 |
|---|---|
| `textmode` | 不開桌面，直接文字模式的 shell |
| `nousb` | 不啟動 USB（鍵盤靠韌體的 PS/2 模擬） |
| `nonet`、`nosmp` | 不啟動網卡、只用一顆 CPU |
| `verbose`、`waitkey` | 每個開機步驟印一行、每步停下來等按鍵：卡住時看最後一行 |
| `scale=1`／`scale=2` | 介面放大（預設：寬 2560 以上是 2） |
| `usernet`、`usbrndis` | 使用者空間的 e1000 驅動、USB 網卡優先用 RNDIS |
| `setup` | 再跑一次 Setup |

- 桌面起不來 → `textmode`；鍵盤沒反應 → `nousb`；開機卡住 → `verbose`（再加 `waitkey`）。
- **panic 畫面**（一塊裂開的石碑，底下是最後 10 行訊息）：拍下整個畫面（特別是底下的訊息），連同你做了什麼一起回報。
- 實機的情況與硬體相容清單見 `docs/p520.md`。
