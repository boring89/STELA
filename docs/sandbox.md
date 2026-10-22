# 沙箱與 powerbox

> P13 E（K15–K17）由自動排程一次做完，使用者已於 2026-10-22 審核。

從 Mouseion 啟動的程式不再看得到整個 `Data`：它只有自己的**容器**（一個資料夾）；要別的檔案，得讓使用者在**視窗伺服器**畫的對話框（powerbox）裡選，或從 Mouseion 拖給它、雙擊文件讓 Mouseion 交給它。程式拿到的是**那一個檔案的 handle**，不是路徑，也不是卷宗。

程式碼：

```
abi/src/lib.rs              capability::VOLUMES（6）、MORE、bootstrap::DOCUMENT（[6]）；VolumeOpen／List／OpenById 多一個能力參數
abi/src/window.rs           POWERBOX、DROP_FILE（請求）、POWERBOX_RESULT、DROPPED（事件）、powerbox::*
kernel/src/fsyscall.rs      volume_open／volume_list／volume_open_by_id 檢查 VOLUMES（Initrd 例外）
kernel/src/program.rs       CAPABILITIES 一則四個、多的接下一則；Linux 程式沒有 VOLUMES 就受限（confined）
kernel/src/linux/files.rs   reachable：受限的程式只到得了 [3]、[4] 以下；path::within（host 測試）
user/libstela               fs::has_volumes、process::inherited（加 VOLUMES）／sandboxed／document
user/mouseion               run_sandboxed、container、TRUSTED、open_with 交 handle、drop_outside
user/windowserver           powerbox（啟動 Initrd:/powerbox）、drop_file
user/powerbox               powerbox 程式：toolbox 的開檔／存檔對話框，把選的檔案送給程式
user/toolbox                dialog::pick_open／pick_save（Picked）、file_dialog、Event::Dropped、Connection::wait_message
user/kalamos、user/viewer    收 [6]、DROPPED；Kalamos 經 handle 存檔
user/sandboxtest            make test-sandbox 的 Data:/Sandbox.app
```

## 1. 比喻：旅館房間

以前每個程式都拿到整棟大樓的萬能鑰匙（`Data` 的根目錄）。現在 Mouseion 讓每個程式住進自己的**房間**（容器）：房間裡隨便它放東西；大樓的其他地方它進不去，連有哪些樓層（卷宗清單）都看不到。要一份別處的文件時，它請**櫃檯**（視窗伺服器）幫忙：櫃檯拿出一張只有住客本人能看的清單（powerbox 對話框，由櫃檯畫，程式碰不到），住客選了哪一份，櫃檯就把**那一份的影本鑰匙**（File handle）交給它。大廳的公告欄（`Initrd`：唯讀的系統檔案、字型、憑證）誰都能看。

```
   程式（容器裡）                    視窗伺服器（VOLUMES）            powerbox（VOLUMES）
      │ POWERBOX（夾帶回覆 port）──────►│
      │                                  │ 啟動 Initrd:/powerbox ─────────►│ 畫開檔對話框
      │                                  │                                  │ 使用者選 Data:/welcome.txt
      │◄──────────── POWERBOX_RESULT（夾帶 File handle：READ[|WRITE]）─────┘ 開檔、複製、送出
      │ 讀／寫那一個檔案
```

## 2. 誰在沙箱裡

| 程式 | 怎麼啟動 | `VOLUMES` | `[3]` |
|---|---|---|---|
| init、視窗伺服器、Mouseion、IME、Setup | kernel → init → 伺服器 | 有（init 從 kernel 拿到，`inherited()` 往下傳） | 照舊 |
| Terminal、Classic、Setup（`Initrd:/` 或 P14 K4 起 `System:/` 裡的，`TRUSTED`） | Mouseion | 有 | 照舊（所在的資料夾） |
| shell 與它啟動的一切 | Terminal、文字模式的 init | 有（**開發者模式**：shell 的程式維持 P12 的行為） | 照舊 |
| 其他從 Mouseion 啟動的程式（bundle、單一 ELF、Linux 程式、雙擊文件開的 Kalamos／Viewer） | Mouseion | **沒有** | `Data:/Programs/NAME/container/` |
| e1000u | init | 沒有（只有 `DEVICES`） | 照舊 |

`NAME` 是 bundle 的名字（`Kalamos.app` → `Kalamos`）或檔名；資料夾不存在就建（沒有 `Data` 時建在 `Tmp`，重開機就沒了）。同名的程式共用一個容器（同一個程式下次啟動還看得到上次存的東西）。

沙箱裡的程式拿到的 handle：

| | 內容 |
|---|---|
| `[3]` | 容器（READ、WRITE、MANAGE：裡面隨它） |
| `[4]` | bundle 本身，或單一程式所在的資料夾；**只有 READ** |
| `[6]`（`bootstrap::DOCUMENT`） | Mouseion 的「用某程式開啟」交給它的檔案（READ，可寫的卷宗再加 WRITE）；參數仍是路徑，只用來顯示名字 |
| 能力 | `NETWORK`、`INIT`（`process::sandboxed()`）；**沒有 `VOLUMES`** |

## 3. `VOLUMES` 能力（K15）

`VolumeOpen`、`VolumeList`、`VolumeOpenById` 多一個參數：程式的 `VOLUMES` 能力 handle（libstela 自動帶上，沒有就送 0 → `AccessDenied`）。

- **`VolumeOpen("Initrd")` 與（P14 K4）`VolumeOpen("System")` 不需要能力**：Initrd 是唯讀的系統檔案（字型 `Initrd:/fonts/`、憑證 `Initrd:/certs/`、程式），每個 toolbox 程式都要讀字型，`fetch`／Periplus 的 TLS 要讀憑證；裡面沒有使用者的東西。
- **`VolumeOpenById` 需要能力**：id 可以指到卷宗上任何地方（例如根目錄的 id），拿著容器的 handle 就能用 id 跳出去。Mouseion、Classic 的 FSM、fstest 用它，都有 `VOLUMES`。
- `DirOpen` 照舊（相對 handle；`..` 和 `/` 本來就不接受，`docs/vfs.md`），所以拿著容器的 handle 只到得了容器以下。
- `FilePath` 仍會說出上面各層的名字（P11 的已知問題），但到不了那些資料夾。

**一個程式最多六個能力**（`capability::MAX` = 6）：視窗伺服器有 POWER、INPUT、NETWORK、INIT、VOLUMES 五個，而一則訊息只能夾四個 handle（`MESSAGE_HANDLES`）。所以 kernel 把 `CAPABILITIES` 拆成幾則：每則四個，除了最後一則，data 的最後一個 kind 是 `capability::MORE`（`u32::MAX`），libstela 的 `take_args` 看到就再收一則。之前（P11–P13 K11）第五個能力會被默默丟掉（init 剛好四個，所以沒出事）。

## 4. Linux 程式的受限根（K15）

Linux 程式不留能力 handle，kernel 在啟動時記下有沒有 `VOLUMES`。沒有的話（從 Mouseion 啟動的 Linux 程式），`linux::State::confined` 記著 `[3]`、`[4]` 的 Linux 路徑（例如 `/Data/Programs/vim/container`），路徑解析時：

- 卷宗上的路徑必須是其中一個資料夾或在它以下（`path::within`），否則 `ENOENT`；連 `/Data` 本身都 `ENOENT`。
- `/` 的列表不列任何卷宗，只有 `tmp`、`dev`、`etc`、`proc`。
- `/tmp`、`/lib`、`/usr/lib`（函式庫資料夾）、`/dev`、`/etc`、`/proc` 照舊。
- fork 與 execve 留著同樣的限制。

shell 啟動的 Linux 程式有 `VOLUMES`，看得到 `[3]`、`[4]` 的整個卷宗（P11 起的行為）。

## 5. powerbox（K16）

toolbox 的 `dialog::pick_open(conn, title, start)`、`pick_save(conn, title, start, name)` 回傳 `Picked { location, file }`：

- 程式有 `VOLUMES`：照舊用自己的對話框（`open_file`／`save_file`），再自己開檔（存檔：`CREATE`，READ|WRITE；開檔：能寫就 READ|WRITE，否則 READ）。
- 沒有：送 `POWERBOX`（mode、"title\0name\0start"，夾帶自己主 port 的 SEND），等 `POWERBOX_RESULT`（最多 30 分鐘；其他事件先留著）。

視窗伺服器收到 `POWERBOX`：啟動 `Initrd:/powerbox`，參數 `open|save TITLE NAME START`，`[2]` = 伺服器的 port（它是普通的 client）、`[6]` = 請求夾帶的回覆 port、能力只有 `VOLUMES`；印 `windowserver: powerbox (open) for client N`。

`powerbox` 程式：用 toolbox 的同一個對話框（`dialog::file_dialog`，畫好時印 `powerbox: open dialog shown`），使用者選好後開檔（存檔就建），複製成 READ（|WRITE）| DUPLICATE | TRANSFER——**沒有 MANAGE**——夾在 `POWERBOX_RESULT`（PICKED、路徑）送到 `[6]`，印 `powerbox: picked Data:/welcome.txt`；取消、出錯都送 CANCELLED。

對話框是另一個程式畫的另一個視窗：沙箱裡的程式碰不到它的內容（也就看不到卷宗清單）。

## 6. 拖放與「用某程式開啟」（K16）

- **雙擊文件**：Mouseion 的 `open_with` 開那個檔案，把 handle 當 `[6]` 交給 Kalamos（`.txt`）或 Viewer（其他），兩者都在容器裡跑；參數照舊是路徑（`kalamos: opened Data:/welcome.txt`）。Kalamos 存檔寫回同一個 handle（`FileSetSize(0)` 後寫），不用路徑。
- **拖到別的程式的視窗**：Mouseion 的 `drop_at` 落點不在自己的視窗時（`DropTarget::Nowhere`），開那個檔案、送 `DROP_FILE`（螢幕座標、路徑，夾帶檔案）給伺服器；伺服器找出那一點的視窗內容（`Part::Content`），屬於別的 client 就送 `DROPPED`（視窗、內容座標、路徑，夾帶檔案），印 `windowserver: dropped PATH on window N`。toolbox 解成 `Event::Dropped { window, x, y, file, path }`。Kalamos（沒存的改變先問）與 Viewer 打開它。

## 7. 測試

| 測試 | 內容 |
|---|---|
| host `abi` | `VOLUMES` = 6、`MAX` = 6、`MORE`、`is_valid`；`POWERBOX`／`DROPPED` 訊息的編碼 |
| host `kernel::linux::path` | `within`：資料夾本身與以下可以、上層與名字相似的不行 |
| selftest／proctest | 沒有 `VOLUMES` 的程式：`VolumeOpen("Data")`、`VolumeList`、`VolumeOpenById` 都 `AccessDenied`，拿 port 冒充也是；`VolumeOpen("Initrd")` 可以；fstest 由 selftest 給 `VOLUMES` |
| `make test-sandbox` | 測試磁碟加 `Data:/Sandbox.app`（`user/sandboxtest`）；Mouseion 雙擊 Data、雙擊 Sandbox.app（`mouseion: started Sandbox (sandboxed)`）；它檢查 `[3]` 是 `/Programs/Sandbox/container`、Data／卷宗清單／by-id 被拒、Initrd 可以、容器可寫、bundle 可讀不可寫；然後 `pick_open` → powerbox 在 `Data:/` 開（`../`、`Programs/`、`Sandbox.app/`、`welcome.txt`：Down 四次、Enter）→ 讀到 `welcome.txt` 的內容、`sandbox: OK` |
| `make test-mouseion` | 雙擊 `welcome.txt` 的 Kalamos 現在在容器裡、經 `[6]` 開檔（輸出不變） |

## 8. 計畫外的決定

- **`Initrd`（P14 K4 起也包括 `System`）不需要 `VOLUMES`**（計畫寫 `VolumeOpen` 需要）：字型與憑證在那裡，所有程式都要讀；它唯讀、沒有使用者的資料。
- **`VolumeOpenById` 也需要 `VOLUMES`**（計畫沒提）：不然拿著容器就能用 id 跳出去。
- **Mouseion 的 `TRUSTED`**：`Initrd:/` 的 Terminal、Classic、Setup 照舊有全部卷宗（終端機的 shell 就是開發者模式；Classic 要 `Data:/Classic` 與整個 Data 的 FSM；Setup 要格式化磁碟）。
- **`OPEN_FILE` 不是一則訊息，而是 `[6]`**（計畫寫「BOOTSTRAP_ARGS 之後的 OPEN_FILE 訊息」）：Mouseion 拿不到新程式主 port 的 SEND，送不了第二則；`[6]` 本來就保留著。
- **powerbox 是伺服器啟動的另一個程式**（計畫寫「伺服器畫對話框」）：對話框直接用 toolbox 的 `file_dialog`，不在伺服器裡重寫一份；信任邊界一樣（只有伺服器和它啟動的 powerbox 有 `VOLUMES`，程式碰不到對話框）。伺服器直接把請求夾帶的回覆 port 交給它，自己不記狀態。
- **回覆 port 由程式自己夾帶**：伺服器手上給 client 的 SEND 沒有 `DUPLICATE`，複製不出來。
- **`[4]`**：bundle 給 bundle 本身（只有程式自己的檔案），單一程式給所在的資料夾；都只有 READ。
- **容器在 `Data:/Programs/NAME/container`，沒有 `Data` 時在 `Tmp`**；同名的程式共用。
- **能力訊息拆成多則**（`MORE`）：見 §3；同時修了第五個能力被丟掉的問題。
- **測試程式不放 initrd**：`sandboxtest` 由 `make test-sandbox` 用 `ehfs-cp` 放進那次測試的磁碟（`NOT_IN_INITRD`）。
- **K15–K17 一個 commit**（`make test-sandbox` 同時驗證三步）。

## 9. 已知限制

- 拖放（`DROP_FILE`／`DROPPED`）沒有自動測試；落點被別的程式的視窗蓋住、但下面是 Mouseion 的視窗時，Mouseion 自己先處理（它不知道上面還有視窗）。
- Periplus 不收 `DROPPED`／`[6]`（它不是 Mouseion 開文件的程式）；不處理 `DROPPED` 的程式會把那個 handle 留到結束。
- 單一程式放在卷宗的最上層時，`[4]` 就是那個卷宗的根（唯讀但整個看得到）。
- 沙箱裡的程式沒有時區（`Data:/.stela/timezone` 讀不到，用 UTC）、`cli::sys::cwd()` 不知道卷宗名稱（當作 `Data`）。
- powerbox 當掉時程式要等 30 分鐘才放棄；程式一次只能開一個 powerbox；伺服器不限制一個程式開幾次。
- Linux 的受限只有 host 測試（`path::within`）與程式碼審查，沒有 QEMU 測試；`/proc/<pid>` 仍看得到別的行程。
- shell 啟動的程式沒有沙箱（開發者模式）；`NETWORK` 與 `INIT` 仍給所有人（`docs/userland.md` §2.3）。
