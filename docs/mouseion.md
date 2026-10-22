# Mouseion：空間式檔案管理員

> P9 由自動排程一次做完，使用者已於 2026-10-20 審核。名稱（選單、Viewer）都是暫定的，使用者之後決定 STELA 自己的命名。

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

### 3.1 P11：延伸屬性（使用者已於 2026-10-22 審核）

Mouseion 的資料不再放在 Finder 資訊（那 32 bytes 照 HFS 的原意留給 Mac 的 type/creator/flags/位置，Classic 環境的 FSM 卷宗用它，`docs/classic.md` §7），改放**延伸屬性**（`FileGetAttr`／`FileSetAttr`，`docs/vfs.md` §4.5）：

| 屬性 | 內容（同上面 32 bytes 的編碼） |
|---|---|
| `stela.mouseion` | 資料夾的視窗狀態 |
| `stela.mouseion.icon` | 項目的圖示位置——**資料夾也有了**（以前 32 bytes 放不下兩份） |

- 讀：先讀屬性；沒有就讀 Finder 資訊，是 Mouseion 的舊格式（`MI`）就寫進屬性、把 Finder 資訊清成 0（**遷移**），之後不再寫 Finder 資訊。
- 開機重開視窗：開著的資料夾視窗清單存在 `Data:/.mouseion`（一行 `卷宗<TAB>id<TAB>路徑`，`model::format_open_list`／`parse_open_list`），開／關資料夾視窗時寫，Mouseion 啟動時（開了 Terminal 之後）依序打開。

## 4. 操作

| 動作 | 結果 |
|---|---|
| 雙擊卷宗 | 它的根資料夾視窗（`Name:/`） |
| 雙擊資料夾 | 它的視窗；已經開著就移到前面（一個資料夾只有一個視窗） |
| 雙擊 `Name.app` | 執行它（`ProcessSpawn` 資料夾 → 裡面的 `exec`）：`[1]` console、`[2]` 視窗伺服器、`[3]` bundle 所在的資料夾 |
| 雙擊 ELF 檔（開頭 `\x7FELF`） | 執行它 |
| 雙擊 `.txt` | `Kalamos.app 路徑`（P11 K34，編輯器，`docs/apps.md` §2） |
| 雙擊其他檔案（不是程式） | `Viewer.app 路徑` |
| 拖曳圖示，放在同一個視窗的空白處 | 移到那一格（存圖示位置） |
| 拖到資料夾或卷宗圖示上 | 移進去（同卷宗：`DirRename`）或複製（不同卷宗：檔案讀寫、資料夾遞迴） |
| 拖到別的資料夾視窗的空白處 | 同上，並把圖示放在放開的地方 |
| 捲軸 | 點箭頭一行、軌道一頁；只有垂直 |

- **P11 的新操作**：
  - **多選**：Shift-點一下加入／拿掉；在資料夾視窗的空白處拖出一個框（畫外框），放開時框到的都選起來（`model::band_select`）。拖曳時移動整個選取（同一個視窗內一起位移，到別處一個個搬或複製）。
  - **拖曳影子**：拖曳中在來源視窗畫出被拖的圖示的外框，跟著指標（半透明不做）；拖到別的視窗時影子只畫在來源視窗內。
  - **改名**：點一下已經是唯一選取的項目的**名稱** → 名稱處變成文字框（白底黑框、`|` 是游標）；打字、Backspace，Enter 改名（`DirRename`），Escape 取消，點別處也算確定。名稱不能有 `/`、`:`。
  - **刪除**：Delete 鍵 → 問「Delete "x"?」（多個時「Delete these N items?」，說明沒有垃圾桶）；Enter 刪除（資料夾連裡面全部），Escape 留著。K32 起用 toolbox 的 `dialog::alert`（`Keep`／`Delete`，Delete 是預設）。失敗（例如唯讀卷宗）印 `mouseion: delete x: …`。
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

### 5.x 垃圾桶與清單排序（P11 K43）

- **垃圾桶**：桌面右下角的 `Trash` 圖示。把項目拖到上面 = 搬到**它所在卷宗**的 `.Trash/`（卷宗根目錄的資料夾，沒有就建；同名的加數字；同一卷宗內的 rename，所以很快；唯讀卷宗沒有垃圾桶）。雙擊 Trash 打開 `Data:/.Trash` 的視窗（可以把東西拖出來）。**Special > Empty Trash...** 先問（`alert`），再把每個卷宗的 `.Trash` 裡的東西連同資料夾內容刪掉，印 `mouseion: emptied the trash (N items)`。Delete 鍵仍是直接刪除（有確認）。
- **清單檢視的欄位**：第一列是 Name、Size、Kind、Date 的標題，點一個就依它排序（名稱不分大小寫、資料夾在前；大小大的在前；種類；日期新的在前；`model::compare`、`column_at`，host 測試），目前的欄位加底線；Date 欄是本地時間（`docs/time.md` §5.6）。印 `mouseion: sorted by Size`。
- 都沒有自動測試（要拖放與點擊座標）；排序有 host 測試。

### 5.y P13 K15–K16：程式在容器裡跑（使用者已於 2026-10-22 審核）

Mouseion 啟動的程式（bundle、單一程式、Linux 程式，雙擊文件開的 Kalamos／Viewer）沒有 `VOLUMES`：`[3]` 是 `Data:/Programs/NAME/container/`、`[4]` 唯讀（bundle 本身或所在的資料夾），印 `mouseion: started NAME (sandboxed)`；雙擊的文件以 `[6]` 交出去。`Initrd:/` 的 Terminal、Classic、Setup 例外（`TRUSTED`）。檔案拖到別的程式的視窗時，Mouseion 送 `DROP_FILE` 給視窗伺服器（`mouseion: dropped PATH at X,Y`），那個程式收到 `DROPPED`。細節見 `docs/sandbox.md`。

### 5.z P13 K20–K21：Find、改名與 type／creator、桌面圖樣、Sleep Display（使用者已於 2026-10-22 審核）

- **File > Find...**（Alt-F）：問一行字（`dialog::ask_text_shown`，對話框出現時印 `mouseion: find on VOLUME`），在**前面那個資料夾視窗的卷宗**（沒有就 `Data`）裡找名字含有它的項目（不分大小寫；廣度優先、跳過 `.` 開頭的、最多看 2000 個資料夾、最多 100 個結果），印 `mouseion: found N for "TEXT" on VOLUME`，結果放在一個資訊視窗：每行「路徑  大小  日期」；**雙擊一行打開它所在的資料夾**（`Pane::found`）。
- **File > Rename...**：用對話框改選取項目的名字（原本「點選取項目的名字」的改名照舊）。**File > Set Type...**：檔案的 Mac type 與 creator（Finder 資訊的前 8 bytes，`TEXT ttxt` 這樣兩個四字元的字，`model::parse_type_creator`）；**Get Info** 多一行 `type:`。計畫寫「Get Info 可改名、改 type/creator」——Get Info 仍是唯讀的資訊視窗，改的動作放在 File 選單（工具箱沒有可編輯的資訊視窗）。
- **Mouseion > Preferences...**：桌面圖樣（Plain、Meander 回紋、Waves 波浪；`toolbox::backdrop`，草稿）存到 `Data:/.stela/desktop`（`pattern=…`，`color=RRGGBB` 可以手改），送 `SET_BACKDROP` 讓視窗伺服器換；Mouseion 自己的圖示直條用同一個圖樣、對齊同一個螢幕格線。
- **Special > Sleep Display**：送 `BLANK`，視窗伺服器立刻把螢幕變黑（`docs/window.md` §12）。

### 5.w P14 K2：Get Info 的編輯、鎖定、備註、bundle 的圖示（自動排程一次做完，**使用者尚未審核**）

- **Get Info**（Alt-I）的資訊視窗多三行：`comment:`（延伸屬性 `stela.mouseion.comment`，UTF-8，最多 3 KiB）、`locked: yes/no`、提示 `N: name  T: type  C: comment  L: lock`。視窗在前面時按 **N** 改名、**T** 改 type／creator、**C** 改備註（都是 `dialog::ask_text`，出現前印 `mouseion: asking for …`）、**L** 鎖定／解鎖（`mouseion: NAME locked`／`unlocked`）；改完資訊視窗重畫、顯示那個資料夾的視窗重新列出。計畫寫「Get Info 可編輯」——工具箱沒有多欄位的編輯視窗，所以用按鍵叫出對話框（**計畫外的簡化**）。資訊視窗自己留一個資料夾的 handle（從卷宗的根依路徑打開，所以帶 `MANAGE`；`VolumeOpenById` 給的 handle 永遠沒有 `MANAGE`），資料夾視窗先關掉也能改。
- **鎖定**：Finder 資訊的 `fdFlags`（bytes 8–9，大端序）的 `kNameLocked` 0x1000（`abi::finder`）。HFS 真正的「locked」是目錄記錄的 `filFlags` bit 0，eHFS 沒有那一欄，所以借用 Finder 旗標（Classic 的 FSM 卷宗看得到同一個位元）。**kernel 擋**：`DirRemove`、`DirRename`（含搬進垃圾桶）與 Linux 的 `unlinkat`、`renameat`（來源與被取代的目標）遇到鎖定的項目回 `AccessDenied`（`FileSystem::check_unlocked`）；內容照樣可以寫（計畫只要求不能刪與改名）。
- **P13 的 File > Set Type... 修正**：它用 `VolumeOpenById` 打開檔案，沒有 `MANAGE`，`FileSetInfo` 一定失敗（沒有測試所以沒發現）；P14 K2 改成從卷宗的根依名字打開（`open_folder_path` + `open_named`）。
- **bundle 的圖示**：`Name.app/icon-32.ppm`（1x）與 `icon-64.ppm`（2x）——小螢幕（圖示 32 以下）是 `icon-16`／`icon-32`，也就是邏輯大小與兩倍。binary PPM（P6，8 位元，`#FF00FF` 透明，和 `assets/` 一樣；`toolbox::icons::parse_ppm`，host 測試），大小不對就不用。2x 沒有時把 1x 放大兩倍。每個 bundle 讀一次、記在記憶體（`Mouseion::pictures`，以資料夾路徑 + id 為鍵；換了圖要重開 Mouseion），印 `mouseion: NAME has its own icon`。只在圖示檢視；清單檢視仍是 16 的 `app`。Initrd 的 Echo.app 用 stela 的草稿圖當例子（Makefile 的 `BUNDLE_ICONS`）。
- 測試：`fstest`（鎖定的檔案刪不掉、改不了名，解鎖後可以）；`make test-mouseion` 的第三次開機（乾淨的磁碟）：Data → 點 welcome.txt → Alt-I → L → C（打 `hello note`）→ N（改名被拒 `access denied`）→ L；第一次開機打開 Initrd 時印 `Echo.app has its own icon`；host `toolbox::icons` 的 PPM 解析與透明。

## 6. Viewer（`Viewer.app`，暫名）

`Viewer 路徑`：`Volume:/path` 或相對工作資料夾；最多 1 MiB；80 欄折行（Tab 到 8 的倍數、控制字元略過、壞 UTF-8 顯示 U+FFFD）；視窗標題 = 檔名；方向鍵、Page Up／Down、Home／End、捲軸捲動；File > Close 或關閉鈕結束。開檔時在 serial 印 `viewer: <路徑>: N lines`。

## 7. 測試

P11：host `model`——開著的視窗清單的格式往返與壞行、框選（`band`、`band_select`）、資料夾也存圖示位置；`make test-mouseion`——打開 Initrd:/ 後點 Classic.app（`mouseion: selected Classic.app`）、Delete → `mouseion: asking: Delete "Classic.app"?`、Enter → `delete Classic.app: read-only volume`（Initrd 唯讀）；第二次開機上次開的 Data:/ 與 Initrd:/ 自己打開。`tools/qemu_test.py` 的 `--type`、`--click` 等改成依命令列的順序觸發（以前全部 `--type` 在 `--click` 前面）。


| 測試 | 內容 |
|---|---|
| host `mouseion::model` | 資料夾狀態的編碼往返、壞的 magic／版本、夾住過大的值、圖示位置、種類與 bundle 名稱、縮短名稱、格線（位置、格子、放置、吸附）、拖放目標（圖示、空白、桌面、外面） |
| host `viewer` | 折行、Tab、CRLF、壞 UTF-8、空檔 |
| `fstest`（selftest 與 test-storage） | `FileGetInfo`／`FileSetInfo` 往返、資料夾、FAT 回 `NotSupported` |
| `make test-mouseion` | 一般 kernel + AHCI 測試磁碟（乾淨的那份）：`--click`（先移到左上角，再以 100 像素一步移過去：QEMU 的 PS/2 滑鼠一個事件只送幾個封包）雙擊 Data → `mouseion: opened Data:/` → 雙擊 `welcome.txt` → `viewer: Data:/welcome.txt` → 雙擊 Initrd → 雙擊 Initrd 視窗裡的 `Terminal.app` → 第二個 `STELA shell`；截圖：卷宗圖示（Boot 的 terracotta）、桌面色、新 terminal 的作用中標題列 |

### 7.x P12 的改動（使用者已於 2026-10-22 審核）

- 桌面視窗只是右邊放圖示的直條（`docs/window.md` §11）。
- 名稱以 `.` 開頭的項目不顯示（`.Trash`、`.stela`、`.pkg` 是系統自己的資料夾）。
- 選單依 `Data:/.stela/language` 用英文或繁體中文（`toolbox::i18n`，`docs/setup.md` §3）。
- 第一次開機時 Setup.app 先跑，Mouseion 由它啟動（`docs/setup.md` §2）。

## 8. 已知限制

- P11 起有拖曳影子（只是外框，只在來源視窗內）、多選、刪除（沒有垃圾桶，直接刪）、改名；沒有複製／貼上、復原。
- 視窗內的捲動只有垂直。
- 開機重開視窗在 Terminal 之後開，所以 Terminal 的視窗可能蓋在它們上面。
- bundle 的自訂圖示（P14 K2）只在圖示檢視、讀一次就記住（換圖要重開 Mouseion）。
- Get Info 的編輯是按鍵叫出對話框（不是可直接編輯的欄位）；鎖定只擋刪除、改名與搬移，不擋寫入。
- 跨卷宗複製把整個檔案讀進記憶體；同名檔案已存在時失敗（`Exists`）。
- 前後順序是 Mouseion 自己記的（`FOCUS` 事件），可能和視窗伺服器的不同（例如別的程式的視窗）。
- P13：Find 只找名字（不找內容）、一次一個卷宗、結果不會自己更新；Set Type 沒有選單式的 type 清單；桌面圖樣只有三種（草稿）、顏色只能手改檔案。
- 有 `MANAGE` 的程式都能用 `FileSetAttr` 改 Mouseion 的屬性（屬性沒有依程式分開）。
