# 輸入法（注音）

> P13 A（K1–K4）由自動排程一次做完，**使用者尚未審核**。

STELA 的第一個輸入法：注音（大千鍵盤配置），引擎是 [libchewing](https://github.com/chewing/libchewing)（新酷音，C，LGPL-2.1-or-later）。輸入法是一個獨立的使用者程式 `Initrd:/ime`，視窗伺服器把鍵盤事件先給它，再把它送出的字給焦點視窗。

程式碼：

```
user/ime/build.rs          用 gcc 編 libchewing（build/chewing/src，make chewing 解開的）＋ c/libc.c → libchewing_c.a
user/ime/c/config.h        libchewing 的 config.h（hash 使用者詞庫，不用 sqlite3）；也給 host 的 init_database
user/ime/c/include/        墊片標頭（stdio.h、stdlib.h、string.h…）：每個名字改成 ime_ 開頭
user/ime/c/libc.c          墊片 C 函式庫：記憶體內的檔案表、printf／fscanf 子集、malloc 經 Rust 配置
user/ime/src/ffi.rs        唯一有 unsafe 的檔案：libchewing 的宣告、墊片的 hook、安全的 Chewing
user/ime/src/lib.rs        Ime：按鍵 → 送出的字、組字、候選（host 測試）
user/ime/src/main.rs       服務：連線、REGISTER_IME、回答 IME_KEY；`ime type KEYS`
abi/src/window.rs          協定：REGISTER_IME、IME_RESULT、SET_CARET、IME_KEY、TEXT_INPUT、IME_RESET
user/windowserver/         按鍵的路由、組字區與候選窗、選單列的「注／英」
user/toolbox/              Event::Text、TextField／TextView 的 insert_text 與 caret、Connection::set_caret
```

## 1. 比喻：口譯員

鍵盤（使用者）說的是「ㄋㄧˇ ㄏㄠˇ」，應用程式只聽得懂「你好」。視窗伺服器（舞台監督）在中間安排一位口譯員（IME）：每個按鍵先交給口譯員，口譯員說「這個鍵我收下了」（還在組字）或「這個鍵不是給我的」（照原樣交給演員），組好了就把整句「你好」交出去。組字的過程寫在舞台監督舉的小白板上（組字區、候選窗），演員看不到，也不需要知道。

## 2. 為什麼是 libchewing 0.5.1、為什麼是原生程式

- libchewing 0.6 起改用 Rust 重寫，需要一串 crate（違反「不加新 crate」）；**0.5.1 是最後一版純 C**，字典格式也穩定。
- 計畫的決定是「原生優先，libc 需求太多就改 Linux 人格」。實際量過：hash 後端（不開 sqlite3）的 libchewing 只用到約 60 個 libc 函式（字串、`malloc`、`fopen`/`fread`/`fwrite`/`fseek`、`fscanf` 的 `%d %hu %s`、`snprintf`/`asprintf`、`qsort`/`bsearch`、`open`/`lseek`/`mmap`、`access`/`rename`/`unlink`）。寫一個 800 行的墊片（`c/libc.c`）比走 Linux 人格 + pipe 省事，也讓 IME 能直接用視窗協定。
- 墊片的每個名字都改成 `ime_` 開頭（標頭裡 `#define fopen ime_fopen`），所以**同一份 C 程式碼**連進 STELA 的程式，也連進 host 的 `cargo test`，不會和 host 的 C 函式庫撞名。`memcpy`、`memmove`、`memset`、`memcmp` 例外：STELA 由 Rust 的 compiler_builtins 提供，host 由 C 函式庫提供（gcc 自己也會產生這些呼叫）。

### 2.1 記憶體內的檔案

libchewing 用路徑讀字典（`/sys/dictionary.dat`，用 `open` + `mmap`）、讀寫使用者詞庫（`/user/uhash.dat`，用 `fopen`）。墊片沒有真的檔案系統，只有一張 16 格的表：

```
 Rust（main.rs）                          C（c/libc.c）
 讀 Initrd:/chewing/*.dat ── ime_vfs_add ──► files[]：路徑、指標、長度、cap、dirty
 讀 Data:/.stela/ime/uhash.dat ─ ime_vfs_add ─►   cap = 0：bytes 屬於 Rust（唯讀，不複製）
                                              fwrite 第一次寫入時複製成自己的（own()）
 每個按鍵之後 ime_vfs_get(/user/uhash.dat) ◄── dirty = 1 就存回 Data
```

`mmap` 直接傳回表裡的指標（字典 3.7 MB 只在 Rust 的 Vec 裡一份）。`malloc` 前面放 16 bytes 記大小，呼叫 Rust 的 `alloc`（`ime_rust_alloc`／`ime_rust_free`），所以 C 與 Rust 共用一個 heap。

檔案表是全域的，所以一個行程同時只能有一個 `Chewing`（`IN_USE`）；`Drop` 時清空檔案表（`ime_vfs_reset`），因為表裡的指標指向即將釋放的 Vec。

### 2.2 除錯

`IME_DEBUG_CFLAGS="-fsanitize=address -g" RUSTFLAGS="-C link-arg=-lasan" cargo test --lib -p ime --no-run`，再用 `LD_PRELOAD=$(gcc -print-file-name=libasan.so)` 執行測試：C 的部分有 AddressSanitizer。K1 用它找到一個 `own()` 沒把現有長度算進容量的溢位。

## 3. 字典

`make chewing`（`Makefile`）：

1. tarball 從 `~/stela-ports/sources/` 複製（`CHEWING_TARBALL=`），沒有就從 GitHub 下載；檢查 SHA-256（`third_party/README.md`）。
2. 解開到 `build/chewing/src`。
3. 用 host 的 gcc 編 libchewing 自己的工具 `init_database`（配 `user/ime/c/config.h`），把 `phone.cin`（字的注音）與 `tsi.src`（詞與詞頻）轉成 `dictionary.dat`（1.2 MB）與 `index_tree.dat`（2.5 MB）。
4. 連同 `pinyin.tab`、`swkb.dat`、`symbols.dat` 放進 initrd 的 `chewing/`。

**計畫外的決定**：計畫寫 `Initrd:/ime/chewing/`，但 `Initrd:/ime` 是程式本身（initrd 的同一層不能同時有檔案 `ime` 與資料夾 `ime/`），所以字典放在 `Initrd:/chewing/`。

使用者詞庫：libchewing 每送出一個詞就記一筆（hash 後端，每筆 125 bytes），IME 存到 `Data:/.stela/ime/uhash.dat`（資料夾沒有就建）；下次開機讀回來，常用的詞會排前面。

## 4. 協定（`abi::window`，P13 K2）

版本仍是 2：都是新增的訊息，舊的 client 不受影響。

| 訊息 | 方向 | 內容 |
|---|---|---|
| `REGISTER_IME` 0x10C | IME → 伺服器 | 0。只有經 **IME port** 連線的 client 可以（否則 `AccessDenied`），同時只能一個（`Busy`） |
| `IME_RESULT` 0x10D | IME → 伺服器 | 0、seq、flags（`CONSUMED` 1、`CHINESE` 2、`MEMORY` 4）、cursor、commit 長度、preedit 長度、候選長度、page \| pages<<16；然後是文字（commit + preedit + 以 `\n` 分隔的候選），≤ 80 bytes 放在訊息裡，否則放在夾帶的 Memory（`MEMORY`） |
| `SET_CARET` 0x10E | client → 伺服器 | 0、window、x、y、h（內容座標）：文字游標在哪裡 |
| `IME_KEY` 0x189 | 伺服器 → IME | seq、`KeyEvent` |
| `TEXT_INPUT` 0x18A | 伺服器 → client | window、長度、UTF-8（每則最多 104 bytes，切在字元邊界；更長的分幾則） |
| `IME_RESET` 0x18B | 伺服器 → IME | 焦點換了：丟掉組字，回一個 seq 0 的 `IME_RESULT` |

### 4.1 誰可以當 IME

IME 看得到每一個按鍵（包括密碼），所以不能讓任何程式登記。伺服器開機時建一個**專用的 IME port**，啟動 `Initrd:/ime` 時把這個 port 的 SEND 當它的 `[2]`；從這個 port 進來的 `CONNECT` 建立的 client 帶 `may_ime`，只有它能 `REGISTER_IME`。一般程式的 `[2]` 是伺服器的主 port，就算送 `REGISTER_IME` 也被拒絕。

### 4.2 按鍵的路徑

```
 kernel ─INPUT_KEY─► 伺服器
                     ├─ Alt + 選單快捷鍵 → MENU_SELECTED（照舊，IME 不經手）
                     ├─ 沒有 IME，或 IME 是英文模式、沒在組字、沒有待回的鍵，
                     │  而且不是 Ctrl-Space → KEY 直接給焦點視窗（和 P12 一樣）
                     └─ 否則：seq+1，記下（seq、鍵、當時的焦點視窗），IME_KEY 給 IME
 IME：Ime::key → IME_RESULT(seq, CONSUMED?, commit, preedit, 候選)
 伺服器：取出 seq 那一筆（更舊而 IME 跳過的照原樣送出）
         commit 不空 → TEXT_INPUT 給那個視窗
         沒有 CONSUMED → KEY 給那個視窗
         更新組字區、候選窗、選單列的「注／英」
```

- **順序**：鍵在 IME 那裡排隊，伺服器收到回答才送出，所以「組字中的鍵」與「直接送的鍵」不會亂序（有待回的鍵時，之後的鍵也一律經過 IME）。
- **逾時**：最舊的待回鍵等了 1 秒，伺服器印 `the IME does not answer`、把排隊的鍵照原樣送出、不再用 IME（`drop_ime`）。IME 結束時也一樣。
- **英文模式不經過 IME**：英文模式的延遲與 P12 完全一樣，所有既有的測試（打 ASCII）不受影響。代價是 Ctrl-Space 永遠被 IME 拿走（程式收不到 Ctrl-Space）。
- **焦點換了**：伺服器清掉畫面上的組字，送 `IME_RESET`；IME 丟掉組到一半的字（不送出）。

### 4.3 IME 怎麼決定（`Ime::key`）

| 情況 | 結果 |
|---|---|
| 放開鍵 | 組字中吞掉，否則照送 |
| Ctrl-Space | 切換中／英；組到一半的字先送出（`toggle`） |
| Alt、Meta、Ctrl + 字母等 libchewing 沒用的鍵 | 組字中吞掉，否則照送 |
| 沒在組字時的編輯鍵（Enter、Backspace、方向鍵、Tab…）、英文模式的任何鍵 | 照送（不經 libchewing） |
| 其他 | 給 libchewing；`chewing_keystroke_CheckIgnore` 說它不要且沒送出字 → 照送 |

libchewing 的設定：每頁 9 個候選、組字區最多 16 字、空白鍵選字（`spaceAsSelection`）、Esc 清掉全部、選字後游標右移、選字從游標往後找（`phraseChoiceRearward`）。Shift + 字母輸入英文字母（libchewing 內建）。

## 5. 畫面

- **組字區**：游標（`SET_CARET`）正下方一個羊皮紙底、黑框的小方塊：組字的文字、底線、游標的直線。
- **候選窗**：組字區下面：`1.你 2.妳 … 9.柅  1/4`（有陰影）。放不下就整組移到游標上方；左右夾在畫面內、不蓋選單列（`compose::ime_boxes`，host 測試）。
- **狀態**：選單列右邊、時鐘左邊的「注」或「英」（有 IME 時才畫）。
- 沒有收過 `SET_CARET` 的視窗：組字區在內容的左下角。

## 6. 應用程式這一端（P13 K3）

- toolbox：`Event::Text { window, text }`（`TEXT_INPUT`）；`TextField::insert_text`、`TextField::caret`；`TextView::insert_text`、`TextView::caret`（以格子算，中文字兩格）；`Connection::set_caret`。
- 接收的程式：Kalamos（TextView，每次重畫時送 `SET_CARET`）、Periplus（網址列）、對話框（`ask_text`、存檔對話框的檔名）、Echo.app。
- **terminal**：`TEXT_INPUT` 轉成新的 `tag::INPUT_TEXT`（10，資料是 UTF-8）送給 shell；shell 在提示符時把每個字插進行編輯器，執行程式時原樣轉給 tty；kernel 的 tty 收到 `INPUT_TEXT` 就 `feed_str`（如同打字）。所以 shell 與 Linux 程式從 tty 讀到 UTF-8。terminal 在游標移動時送 `SET_CARET`（格子的位置）。
- 計畫寫「terminal 收到 `TEXT_INPUT` 就直接寫進 tty」；但 shell 在提示符時是從 `INPUT_KEY` 自己做行編輯（不經 tty），直接寫 tty 的話提示符下打不了中文，所以多了 `INPUT_TEXT`（**計畫外的決定**）。

## 7. 測試

| 測試 | 內容 |
|---|---|
| host `ime`（`cargo test --lib -p ime`，用 `build/chewing/data` 的字典） | `su3cl3` + Enter 送出「你好」、組字中顯示注音、Backspace／Esc、Down 開候選並以 `2` 選第二個、閒置與英文模式的鍵照送、Ctrl-Space 切換並送出組到一半的字、使用者詞庫寫出並由第二個 context 讀回、同時只能一個 context、reset |
| host `abi::window` | IME 訊息的編號與往返、`TEXT_INPUT` 切在字元邊界 |
| host `windowserver::compose` | `ime_boxes`：跟著游標、右邊界、底部翻到上方、不蓋選單列 |
| host `toolbox` | `Event::Text` 的解碼；`TextView`／`TextField` 的 `insert_text` 與 `caret`（中文字兩格） |
| `make test-shell` | `ime type su3cl3` 印 `ime: 你好`（freestanding 的 libchewing 在 STELA 裡跑） |
| `make test-ime` | Kalamos：Ctrl-Space（`ime: 注`）、`su3` + Down（`ime: candidates`，伺服器印候選窗的位置，**截圖**檢查那裡是羊皮紙底、有一列字）、`1`、`cl3`、Enter、Alt-S、Ctrl-Q、Ctrl-Space（`ime: 英`）、`shutdown`；host 讀回 `/ime.txt` 是「你好」（6 bytes）、fsck 乾淨 |

`qemu_test.py` 多了 `--screenshot TRIGGER FILE`（QMP `screendump`；之後的動作可以在同一行觸發，因為截圖本身不會印東西）。IME 只在切換（`ime: 注`／`ime: 英`）與開候選窗（`ime: candidates`）時印一行，**不印任何打的字**。

## 8. 已知限制

- 只有大千配置的注音；沒有拼音、倚天等配置的選擇介面（libchewing 有，沒接）；沒有設定畫面。
- 組字區與候選窗用單一字型（12px 的中文點陣字，16x32 字型時 2 倍放大）；候選窗不能用滑鼠點。
- Ctrl-Space 一律給 IME（Classic.app 收不到）；每個視窗沒有各自的中英狀態（全系統一個）。
- 使用者詞庫每次改變就整個寫回（`uhash.dat`，每筆 125 bytes）；沒有匯入／匯出。
- IME 程式結束不會自動重啟（伺服器改回直送）。
- libchewing 0.5.1 是 2016 年的版本；之後的修正與詞庫更新（Rust 版）沒有。
