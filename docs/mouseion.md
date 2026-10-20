# Mouseion：空間式檔案管理員

> P9 由自動排程一次做完，**使用者尚未審核**；之後修改前請先帶使用者過一遍這份文件。名稱（選單、Viewer）都是暫定的，使用者之後決定 STELA 自己的命名。

Mouseion（Μουσεῖον，亞歷山卓的「繆思之殿」，圖書館所在）是 STELA 的 Finder：桌面上每個卷宗一個圖示、每個資料夾一個視窗，**而且每個視窗記得自己在哪裡**。架構決定見 `docs/architecture.md` §1.1（空間式檔案管理）。視窗的機制見 `docs/window.md`，圖示見 `docs/identity.md`。

程式碼：

```
user/mouseion/src/model.rs   Finder 資訊的編碼、格線、種類、bundle、拖放目標（純邏輯，host 測試）
user/mouseion/src/main.rs    Mouseion
user/viewer/                 Viewer.app：唯讀的文字檢視（lib.rs 的 wrap 有 host 測試）
abi、kernel、libstela         FileGetInfo／FileSetInfo（37、38）、ProcessSpawn 的 bundle（docs/vfs.md §4.2、§4.3）
```

## 1. 比喻：書房裡的抽屜櫃

「空間式」的意思是：一個資料夾就是一個**實體的抽屜**，只有一個，打開就在它上次的位置、上次的大小，裡面的東西也擺在上次放的地方。不會出現「同一個資料夾開了兩個視窗」或「每次打開都在不同位置」。所以位置要存在**資料夾自己身上**（eHFS 每個項目有 32 bytes 的 Finder 資訊），而不是存在某個設定檔裡：把資料夾搬到別的地方，它的視窗位置跟著走。

## 2. 開機與畫面

```
 init ──► windowserver mouseion
 Mouseion：選單列 → DESKTOP 視窗（選單列以下整個畫面，沒有框，永遠在最後面）
           右邊一列卷宗圖示（VolumeList 的順序：Boot、Data、Initrd、Tmp…）
           → 啟動 Initrd:/Terminal.app（目前的「啟動項目」）
```

- 圖示 64×64（螢幕寬 ≥ 1920，`toolbox::icons::size_for`），格子 160×112（圖示 + 名稱）；字型 16x32。
- 卷宗用 `volume` 圖示，資料夾 `kibotos`，bundle `app`，文件 `scroll`（`docs/identity.md`）。
- 選取：點一下，圖示變暗、名稱反白。

## 3. 空間記憶：Finder 資訊的配置（`model.rs`）

```
 資料夾（32 bytes）                      檔案（32 bytes）
  0  "MI"  magic                          0  "MI"
  2  1     版本                           2  1
  3  1     種類：資料夾狀態               3  2     種類：圖示位置
  4  x y w h   i16 × 4（螢幕座標，內容區） 4  x y   i16 × 2（視窗內容座標）
 12  捲動 x y  i16 × 2                     8  保留（0）
 16  檢視 0 圖示、1 清單
 17  保留（0）
```

- 打開資料夾時 `FileGetInfo` → 有效（magic、版本、種類、寬高 > 0）就用它的矩形、捲動與檢視；否則從 (120, 選單列 + 100) 起疊放、6×3 格大小。
- 存回（`FileSetInfo`）的時機：打開時、移動（`MOVED`）、改大小、捲動、切換檢視、關閉、重新開機／關機前。
- 檔案的圖示位置在拖放後存；列出資料夾時有位置的放在原處，其他依序填空格（`Grid::place`）。
- **資料夾自己的圖示位置不存**：資料夾的 Finder 資訊已經放它的視窗狀態（32 bytes 放不下兩份）；資料夾圖示一律排在格子裡（記在限制）。
- FAT 與 Initrd 不存（`NotSupported`，Mouseion 不理會）；`Tmp` 存在記憶體（關機就沒了）。
- 數值超出 i16 會被夾住，不會繞回。

## 4. 操作

| 動作 | 結果 |
|---|---|
| 雙擊卷宗 | 它的根資料夾視窗（`Name:/`） |
| 雙擊資料夾 | 它的視窗；已經開著就移到前面（一個資料夾只有一個視窗） |
| 雙擊 `Name.app` | 執行它（`ProcessSpawn` 資料夾 → 裡面的 `exec`）：`[1]` console、`[2]` 視窗伺服器、`[3]` bundle 所在的資料夾 |
| 雙擊 ELF 檔（開頭 `\x7FELF`） | 執行它 |
| 雙擊 `.txt` 或其他檔案 | `Viewer.app 路徑` |
| 拖曳圖示，放在同一個視窗的空白處 | 移到那一格（存圖示位置） |
| 拖到資料夾或卷宗圖示上 | 移進去（同卷宗：`DirRename`）或複製（不同卷宗：檔案讀寫、資料夾遞迴） |
| 拖到別的資料夾視窗的空白處 | 同上，並把圖示放在放開的地方 |
| 捲軸 | 點箭頭一行、軌道一頁；只有垂直 |

- 拖放的判定（`model::drop_target`）：視窗伺服器把整個按住的過程都送給按下的那個視窗（capture，座標可以在視窗外），Mouseion 把座標換成螢幕座標，依自己知道的前後順序（`FOCUS` 事件；桌面最後）找放在哪個視窗、哪個圖示上。拖曳中沒有畫出圖示的影子（記在限制）。
- 雙擊判定由視窗伺服器做（500 ms、4 像素）。

### 4.1 選單（英文暫名）

| 選單 | 項目 |
|---|---|
| Mouseion | About Mouseion（資訊視窗） |
| File | New Folder（在最前面的資料夾視窗建 `untitled folder`、`untitled folder 2`…）、Open（開啟選取的）、Get Info（名稱、種類、大小、位置、id 的資訊視窗）、Close（關最前面的資訊視窗或資料夾視窗） |
| Edit | （空：沒有剪貼簿） |
| View | Icons、List（最前面的資料夾視窗；清單是一行一個：小圖示、名稱、大小、種類） |
| Special | Terminal（再開一個 terminal）、Restart、Shut Down（先存所有視窗狀態，再 `SystemPower`） |

鍵盤：Enter = Open，Ctrl-W = Close。沒有垃圾桶、搜尋、別名（計畫 M1）。

## 5. bundle

```
 Name.app/
   exec        ELF（STELA 或 Linux 程式）
   info.txt    第一行名稱、第二行版本（可選）
   icon-32.ppm、icon-64.ppm   可選：自己的圖示（Mouseion 目前還沒讀，都用 app 圖示）
```

- Mouseion 把 `.app` 資料夾當一個圖示（名稱不顯示 `.app`），雙擊執行；kernel 的 `ProcessSpawn` 接受 bundle 資料夾（`docs/vfs.md` §4.3），所以 shell 的 `run Name.app` 也行。
- initrd 裡的 `Terminal.app`、`Echo.app`、`Viewer.app` 由 Makefile 的 `BUNDLES` 產生。

## 6. Viewer（`Viewer.app`，暫名）

`Viewer 路徑`：`Volume:/path` 或相對工作資料夾；最多 1 MiB；80 欄折行（Tab 到 8 的倍數、控制字元略過、壞 UTF-8 顯示 U+FFFD）；視窗標題 = 檔名；方向鍵、Page Up／Down、Home／End、捲軸捲動；File > Close 或關閉鈕結束。開檔時在 serial 印 `viewer: <路徑>: N lines`。

## 7. 測試

| 測試 | 內容 |
|---|---|
| host `mouseion::model` | 資料夾狀態的編碼往返、壞的 magic／版本、夾住過大的值、圖示位置、種類與 bundle 名稱、縮短名稱、格線（位置、格子、放置、吸附）、拖放目標（圖示、空白、桌面、外面） |
| host `viewer` | 折行、Tab、CRLF、壞 UTF-8、空檔 |
| `fstest`（selftest 與 test-storage） | `FileGetInfo`／`FileSetInfo` 往返、資料夾、FAT 回 `NotSupported` |
| `make test-mouseion` | 一般 kernel + AHCI 測試磁碟（乾淨的那份）：`--click`（先移到左上角，再以 100 像素一步移過去：QEMU 的 PS/2 滑鼠一個事件只送幾個封包）雙擊 Data → `mouseion: opened Data:/` → 雙擊 `welcome.txt` → `viewer: Data:/welcome.txt` → 雙擊 Initrd → 雙擊 Initrd 視窗裡的 `Terminal.app` → 第二個 `STELA shell`；截圖：卷宗圖示（Boot 的 terracotta）、桌面色、新 terminal 的作用中標題列 |

## 8. 已知限制

- 拖曳中看不到圖示的影子；不能一次選多個；沒有複製／貼上、刪除（沒有垃圾桶）、改名（Get Info 只看）。
- 資料夾圖示的位置不保存（只有檔案的）；視窗內的捲動只有垂直。
- 開機時不重新打開上次開著的視窗。
- bundle 的自訂圖示（`icon-*.ppm`）還沒讀。
- 跨卷宗複製把整個檔案讀進記憶體；同名檔案已存在時失敗（`Exists`）。
- 前後順序是 Mouseion 自己記的（`FOCUS` 事件），可能和視窗伺服器的不同（例如別的程式的視窗）。
- 所有程式都能用 `FileSetInfo` 改 Mouseion 的記錄（沒有分隔各程式的 Finder 資訊）。
