# 程式：命令列工具、編輯器、監視器、瀏覽器（P11 H，使用者尚未審核）

P11 的 H 部分讓 STELA 有日常用得到的程式。名稱都是**暫名**（使用者之後決定）。

| 程式 | 在哪裡 | 怎麼開 | 一句話 |
|---|---|---|---|
| 命令列工具（§1） | `Initrd:/bin/` | shell 直接打名字 | `grep`、`sort`、`find`、`tar`… 可以用 `\|` 串 |
| `Kalamos.app`（§2） | `Initrd:/` | Mouseion 雙擊 `.txt`、`run Kalamos.app 檔案` | 純文字編輯器 |
| `Monitor.app`（§3） | `Initrd:/` | Mouseion 的 Special > Monitor、`Monitor.app &` | 行程、記憶體、網路；結束行程 |
| `Periplus.app`（§4） | `Initrd:/` | `run Periplus.app [網址]` | HTTP 與（P12 起）HTTPS 的小瀏覽器 |

它們都用 toolbox 的新元件（`docs/toolbox.md` §6.1–6.2：`TextView`、對話框）與系統剪貼簿（`docs/window.md` §3）。

## 1. 命令列工具（`Initrd:/bin/`，P11 K33）

**比喻**：shell 的內建指令像瑞士刀上的幾把小刀；`Initrd:/bin/` 是工具箱，每樣工具一個程式，可以用 `|` 串起來（`docs/userland.md` §6.1）。

### 1.1 怎麼找到程式

shell 先看內建指令（`ls`、`cd`、`cat file`、`cp`、`mv`、`mkdir`、`rm`、`ps`、`vols`…），不是內建的就當程式找，依序：

1. 工作資料夾（或寫了路徑時照路徑）；
2. `Initrd:/`（`hwinfo`、`ifconfig`、`ping`、`fetch`、`tone`、`bench` 等）；
3. `Initrd:/bin/`（下面這些工具）。

管線（`a | b`）的每一段都是程式，所以 `cat x | grep y` 的 `cat` 是 `Initrd:/bin/cat`；單獨打 `cat file` 仍是內建的。以前 initrd 根目錄的鍵盤 echo（文字模式的測試程式）改名 `keyecho`，`echo` 現在是印參數的工具。

### 1.2 工具

| 工具 | 用法 | 說明 |
|---|---|---|
| `echo` | `echo [-n] 文字…` | 印參數（空白隔開）；`-n` 不換行 |
| `cat` | `cat [檔案…]` | 依序印出檔案；沒有檔案就印輸入（`[5]`） |
| `grep` | `grep [-inv] 文字 [檔案…]` | 含有文字的行；`-i` 不分大小寫、`-n` 加行號、`-v` 反過來；有印出東西 exit 0，沒有 1，用錯 2 |
| `wc` | `wc [-lwc] [檔案…]` | 行、字、位元組；沒有選項時 `wc: L lines, W words, B bytes`，有選項時只印那幾個數字（行、字、位元組的順序） |
| `head`／`tail` | `head [-n N] [檔案…]` | 前／後 N 行（預設 10；`-n5` 也可以） |
| `sort` | `sort [-nr] [檔案…]` | 排序；`-n` 依開頭的數字（同數字再依文字）、`-r` 反過來 |
| `uniq` | `uniq [-c] [檔案…]` | 相鄰重複的行只留一行；`-c` 前面加次數 |
| `find` | `find [資料夾] [-name 樣式]` | 資料夾底下的每一項（遞迴，最深 64 層），一行一個完整路徑；`-name` 用 `*`、`?` 比對名稱 |
| `du` | `du [項目…]` | 資料夾底下所有檔案的大小（KiB；預設 `.`） |
| `df` | `df` | 卷宗、檔案系統、大小、剩餘（同 shell 的 `vols`） |
| `date` | `date` | 日期時間（UTC；RTC 當成 UTC，`docs/time.md` §5.5） |
| `sleep` | `sleep 秒` | 等一下（可以有小數：`0.5`） |
| `touch` | `touch 檔案…` | 不存在就建一個空檔（沒有改時間的 syscall） |
| `stat` | `stat 項目…` | 種類、大小、id、唯讀、建立與修改時間 |
| `hexdump` | `hexdump [-n N] [檔案…]` | 16 進位與文字對照，每行 16 bytes；`-n` 只看前 N bytes |
| `tar` | `tar t 檔案`、`tar x 檔案 [資料夾]` | ustar：列出成員；解開到資料夾（預設 `.`；含 `..` 或絕對路徑的成員略過） |
| `true`／`false` | | exit 0／1 |

shell 的 `cp` 多了 `cp -r 來源 目的`：整個資料夾（遞迴，最深 64 層；目的在來源裡面時拒絕），印 `N file(s), B bytes copied`。

### 1.3 結構

- 一個套件 `user/bin`：lib `cli`（`src/lib.rs`，純邏輯，host 測試：選項解析 `Args`、`count`、`lines`、`matches`、`sort`、`uniq`、`glob`、`date`（1970 起的奈秒 → 年月日，Howard Hinnant 的公式）、`hex_line`、`tar_entries`（POSIX ustar 標頭：名稱 0–100、大小 124–136 八進位、型別 156、prefix 345–500））與 `cli::sys`（STELA 上：參數、從 `[3]` 解析路徑（`fs::path_of` + 卷宗名稱 → `path::Location`）、讀檔、讀輸入（檔案或 `[5]`）、`工具: 訊息` 的錯誤）；每個工具是 `src/bin/` 的一個小程式。
- Makefile 的 `BIN_TOOLS` 列出它們；`make user` 建好放 `build/user/bin/`，`make initrd` 放進 initrd 的 `bin/`。
- 計畫寫「每個工具一個小 crate」；做成一個套件多個 bin（一樣是各自獨立的程式，少了十幾個幾乎一樣的 Cargo.toml）——計畫外的決定。

### 1.4 測試

- host：`cargo test -p bin --lib`（選項、計數、排序、uniq、glob、日期（含閏日）、hexdump 的一行、ustar 的解析與壞標頭）。
- `make test-shell`：`cat /welcome.txt | grep -i stela | wc -l`、`grep -n -i stela /welcome.txt`（`1:Welcome to STELA's Data volume.`）、`cp -r Initrd:/linux /lx`（`9 file(s), …`）、`find /lx -name n*`（`Data:/lx/net`、`Data:/lx/nosys`）、`date`（`… UTC`）。`make test-pipe` 的 `wc` 也是這個工具。

### 1.5 限制

- 沒有萬用字元展開（shell 不展開 `*`；`find -name` 自己比對）、沒有環境變數、沒有 `PATH` 設定（搜尋順序固定）。
- 工具一次讀完整個輸入（`sort`、`tail` 本來就要；`grep`、`wc` 也是），很大的輸入會用很多記憶體。
- `grep` 只找子字串（沒有正規表示式）；`sort` 只依整行或開頭的整數。

## 2. 文字編輯器 `Kalamos.app`（P11 K34）

**比喻**：Viewer 是看書，Kalamos（κάλαμος，蘆葦筆）是寫字——同樣的紙，多了一枝筆和橡皮擦。

```
 Kalamos.app [檔案]     檔案（Volume:/路徑，或從工作資料夾算）；不存在就是新文件
 File   New  Open...  Save  Save As...  -  Close      Ctrl-N  Ctrl-O  Ctrl-S  Ctrl-Shift-S  Ctrl-Q/W
 Edit   Undo  -  Cut  Copy  Paste  Select All         Ctrl-Z  Ctrl-X  Ctrl-C  Ctrl-V  Ctrl-A
 View   Small Font（8x16）  Large Font（16x32）
```

- 一個視窗一份文件（80 欄 × 30 列起，可以改大小；文字自動折行），內容是 toolbox 的 `TextView`（`docs/toolbox.md` §6.1）：游標、Shift 選取、滑鼠拖曳選取、捲軸、PgUp/PgDn、Tab = 4 個空白。
- UTF-8 純文字、`\n` 換行（讀進來的 `\r` 會留著）；最大 4 MiB。
- 標題是檔名，有未存的修改時前面加 `* `（計畫寫 `•`，但視窗伺服器的字型只有 ASCII——計畫外的決定）。
- Save：寫回原檔（`CREATE | TRUNCATE`）；沒有檔名（New 之後）或 Save As 時用 `dialog::save_file`。Open 用 `dialog::open_file`。
- New、Open、Close（含關閉框）遇到未存的修改先問：`alert`「Save the changes to "x"?」Cancel／Don't Save／Save（Save 是預設）。
- Undo 是一步（連續打字算一步，`docs/toolbox.md` §6.1），再按一次是重做。剪貼簿是系統的（`docs/window.md` §3）。
- Mouseion 雙擊 `.txt` 開 Kalamos（以前是 Viewer；Viewer 保留，給其他文件與唯讀檢視）。
- 印到 `[1]`（測試用）：`kalamos: opened X (N bytes)`、`kalamos: new X`、`kalamos: modified`（第一次變成未存）、`kalamos: saved X (N bytes)`、`kalamos: closed`。

### 2.1 測試

`make test-editor`（加進 `make test`）：一般 kernel + AHCI 測試磁碟；shell `run Kalamos.app /notes.txt`（新文件）→ QMP 打兩行字 → `kalamos: modified` 後 Ctrl-S → `kalamos: saved Data:/notes.txt (36 bytes)` → Ctrl-Q → `Kalamos exited with code 0` → shell `shutdown`；host 用 `ehfs-cp out` 拿出 `/notes.txt` 與預期的內容 `cmp`，`fsck.ehfs` 確認正常卸載。`make test-mouseion` 雙擊 welcome.txt 看到 `kalamos: opened Data:/welcome.txt`。

### 2.2 限制

- 沒有尋找／取代、多份文件、自動存檔、行號、語法顏色；Shift-點選不會延伸選取（視窗伺服器的滑鼠事件沒有帶修飾鍵）。
- 非 ASCII 的字可以讀寫，但字型只畫得出 ASCII（其他字是方框）。
- 對話框的互動沒有自動測試。

## 3. 系統監視器 `Monitor.app`（P11 K35）

**比喻**：儀表板。每秒看一次引擎（行程）、油箱（記憶體）、轉速（處理器）、里程（網路收送），必要時熄掉一個不聽話的引擎。

```
 Processes                                        [ End Process ]
   pid  name                 threads  cpu  state      ← List，點一下選取
 N running, M ended
 Memory   用掉 / 全部 MiB；kernel heap 用掉 / 大小 KiB
 CPUs     cpu0 1000/s  cpu1 1000/s                  ← 每顆每秒處理的 timer tick
 Network  e1000e 10.0.2.15: in N frames (K KiB), out …
 Up       時:分:秒
```

- 資料：`SysInfo(PROCESSES)`（執行中的；結束的只計數）、`PROCESS_CPUS`（每個行程在哪顆）、`MEMORY`、`CPUS`、`NetInfo`（K28 的計數）、`ClockNowNs`。每秒重讀一次（`next_event` 的逾時），選取依 pid 保留。
- **CPU 欄不是使用率**：kernel 沒有記錄忙／閒的時間，每顆 CPU 每秒都處理約 1000 個 timer tick；這個數字只表示那顆還活著（計畫寫「≈ 使用率的替代」）。真正的使用率要 kernel 記 idle 執行緒的時間（之後的工作）。
- **End Process**（按鈕或 File > End Process）：先問「End "x" (process N)?」（`alert`，Cancel／End），再 `ProcessOpen(pid, power)` + `ProcessKill`，印 `monitor: ended N (x)`。init、windowserver、mouseion 與 Monitor 自己不能從這裡結束（提示說明）。
- **新 syscall `ProcessOpen`（62，P11）**：`ProcessOpen(pid, power) -> process`（`WAIT | KILL | DUPLICATE`）：用 pid 拿到任何執行中行程的 handle，要持有 `POWER` 能力（`docs/userland.md` §2.3）；不在執行 → `NotFound`。只有 Mouseion 的 **Special > Monitor** 啟動的 Monitor 拿得到 `POWER`（Mouseion 從視窗伺服器那裡有）；從 shell 啟動的 Monitor 只能看、按 End Process 會說明沒有權限。libstela `process::open(pid)`。
- 印到 `[1]`：第一次畫好後 `monitor: showing N processes`。

### 3.1 測試

- `make test-shell` 最後打 `Monitor.app &`（背景），看到 `monitor: showing`。
- proctest 第 25 步：沒有 `POWER` 時 `process::open(1)` 與用 port 冒充的 `ProcessOpen` 都是 `AccessDenied`。
- End Process 的互動沒有自動測試（要點選清單與按鈕）。

### 3.2 About STELA

Mouseion 的 **Mouseion > About STELA**（K28）：版本、CPU、處理器數、記憶體、畫面、開機多久——資料來自硬體報告的摘要行（`docs/hwinfo.md`）。

## 4. 瀏覽器 `Periplus.app`（P11 K36）

**比喻**：periplus（περίπλους）是古希臘的航海指南——沿著海岸一站一站寫下港口。Periplus 也是這樣：一頁一頁沿著連結走，記住來時的路（上一頁／下一頁）。

```
 [<] [>] [Reload] [ http://host/path                            ]
 ┌────────────────────────────────────────────────────────────┬─┐
 │ 頁面：標題與 <b> 粗體，連結有底線（Tab 選到的反白）          │▲│
 └────────────────────────────────────────────────────────────┴─┘
 http://host/path: 12 KiB, status 200                ← 狀態列
```

- **HTTP 與 HTTPS**：P11 只有 HTTP；P12 起 `https://` 經 libstela 的 TLS 1.3（Mozilla 的根憑證，`docs/tls.md`）。位址：`https://host[:port]/path`、`http://host[:port]/path`、`stela:home`（內建首頁：說明與幾個連結）、`Volume:/path` 或 `file:Volume:/path`（本機檔案；`.html`/`.htm` 當 HTML，其他當純文字）。
- **libstela `http`**（新，純邏輯 host 測試）：`get(url)`：HTTP/1.0 GET、`Host` 標頭、讀到連線結束；`parse_head`（狀態、標頭）、301/302/303/307/308 跟著 `Location` 走（最多 5 次）、`Transfer-Encoding: chunked` 解開（`dechunk`）、`Response::is_html`（`Content-Type`，沒有時看內容開頭）、`join(base, link)`（相對連結、`/` 開頭、`.`／`..`、丟掉 `#`；也適用 `Volume:/`）。`net::http_get_raw` 給它原始回應。
- **HTML 子集**（`user/periplus/src/lib.rs`，純邏輯 host 測試）：容錯的 tokenizer（標籤名稱轉小寫、屬性有沒有引號都行、註解與 `<!…>` 略過、`script`／`style` 的內容略過、實體 `&amp; &lt; &gt; &quot; &apos; &nbsp; &#N; &#xN;`）→ `Page { title, blocks, links }`：`h1`–`h6`（粗體）、`p`／`div` 等區塊、`br`、`hr`、`pre`（保留換行與空白）、`blockquote`（縮排）、`ul`／`ol`／`li`（`*` 或 `1.`，巢狀縮排）、`table`／`tr`／`td`／`th`（一列一行，儲存格用 16 欄的定位點）、`b`／`strong`、`i`／`em`（照常顯示）、`a href`、`img`（顯示 `[alt]`）。其他標籤略過但保留文字。
- **排版**（`layout(page, cols)`）：等寬字型的欄；區塊之間空一行（相鄰的清單項目不空）、單字換行（太長的字切開）、`pre` 不換行（超出就切）。粗體是同一行字往右一像素再畫一次。
- **操作**：Ctrl-L（或 File > Open Address）清空網址列開始打字，Enter 前往、Escape 放回原網址；沒給網址啟動時停在首頁、網址列已在等打字。Tab／Shift-Tab 選下一個／上一個連結（捲到看得到），Enter 前往；點連結前往；Alt-Left／Alt-Right（或按鈕、Go 選單）上一頁／下一頁；Ctrl-R、F5、Reload 重新載入；方向鍵、PgUp/PgDn、空白鍵、Home/End 捲動。標題列是頁面的 `title`。
- 載入是同步的（載入中視窗不回應，狀態列顯示 `Loading…`）；印 `periplus: loading X`、`periplus: loaded X (title "…")`（轉址後的位址）或 `periplus: X: 錯誤`。

### 4.1 測試

- host：`libstela::http`（標頭、chunked、HTML 判斷、連結解析）、`periplus`（實體與 tokenizer、整頁的區塊與連結、換行與連結位置、清單／pre／表格、太長的字）。
- P13 K24：首頁多了使用手冊的連結（`Initrd:/doc/guide.html`、`guide.en.html`，`tools/md_to_html.py` 從 `docs/guide.md`、`guide.en.md` 轉出來）；「HTTP only」的說明改成 HTTP 與 HTTPS（P12 起就有）。`make test-browser` 最後打開中文手冊（`title "STELA 使用手冊"`）。
- `make test-browser`（加進 `make test`）：host 的 `with_http.py` 提供 `build/www`（`sub/index.html` 與 `page2.html`）；shell `run Periplus.app` → 首頁 → 打 `http://10.0.2.2:8642/sub`（host 的伺服器把資料夾轉址到 `/sub/`）→ `loaded …/sub/ (title "Sub page")` → Tab Enter 跟著 `../page2.html` → `(title "Second page")` → Alt-Left 回到 `…/sub/`。

### 4.2 限制

- 沒有 HTTPS、CSS、JavaScript、圖片（只顯示 alt）、表單、cookie、快取；只有一種字型（標題不放大）；非 ASCII 字畫不出來。
- 載入時整個視窗等著（沒有背景載入、不能中止）；回應整個讀進記憶體（最多 4 MiB 拿來顯示）。
- 文字編碼一律當 UTF-8（壞的位元組換成替代字元）。
