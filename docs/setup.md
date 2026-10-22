# Setup.app：第一次開機（P12）

> P12 由自動排程一次做完，使用者已於 2026-10-22 審核。

第一次開機時，在 Mouseion 之前先出現 Setup.app，一頁一頁問：語言、時區、Data 卷宗、網路、套件、Classic、顯示（P14 K2），最後寫下「設定過了」並啟動 Mouseion。程式碼：`user/setup`（Setup.app）、`user/toolbox/src/i18n.rs`（兩種語言的字串）、`user/init`（決定要不要跑）、`kernel/src/vfs/mount.rs` 的 `format_candidates`／`format_data` 與 syscall 65 `VolumeFormat`。

## 1. 比喻：新家的入住手續

搬進新家（新的 STELA）時，管理員先帶你填幾張表：用哪種語言、住在哪個時區、東西放哪個倉庫（Data 卷宗；沒有倉庫的話，可以把一間**從沒用過的空房間**改成倉庫——但得親手寫下房間號碼，免得改錯）、網路通不通、要不要先搬幾件家具進來（套件）、有沒有舊家的東西（Classic 的 ROM 與 System）。填完了，管理員在門口貼一張「已入住」（`Data:/.stela/setup-done`），下次就直接進門（Mouseion）。

## 2. 什麼時候跑

```
 init（有 framebuffer、不是文字模式時）
   boot.cfg 有 setup？                       ──► Setup.app
   有 Data 卷宗但沒有 Data:/.stela/setup-done？ ──► Setup.app
   否則                                       ──► Mouseion（照舊）
 init 給視窗伺服器的程式清單：["windowserver", "Setup.app"] 或 ["windowserver", "mouseion"]
 Setup 結束時自己啟動 Mouseion（和視窗伺服器一樣的 handle 與 POWER 能力）
```

- 完全沒有 Data 卷宗又沒有 `setup` 選項時不跑（沒有地方記住設定；QEMU 沒有磁碟的測試也不受影響）。
- 測試用的磁碟映像（`make_test_images.py` 的 ahci.img、`make_disk_image.py --setup-done`）都已經有 `setup-done`，其他桌面測試不會停在 Setup。`make disk-image` 做的實機映像**沒有**，第一次開機就會跑 Setup。
- Mouseion 從 P12 起不顯示名稱以 `.` 開頭的項目（`.Trash`、`.stela`、`.pkg` 都是系統自己的資料夾）。

## 3. 語言：`toolbox::i18n`

- `Language::{English, TraditionalChinese}`；`tr("Shut Down")` 用英文原文當鍵查表，沒有翻譯就回原文；`tr_menu("Open^O")` 保留快捷鍵。
- 選擇存在 `Data:/.stela/language`（`en`、`zh-TW`），`i18n::load()` 讀它。目前翻譯：Setup 自己的所有文字、Mouseion 的選單（計畫：先只翻這兩個）。
- 中文字用 P11 的 Fusion Pixel 字型（`docs/toolbox.md` §6.3）；kernel 的主控台訊息仍是英文。

## 4. 每一頁

全部用鍵盤：方向鍵選、空白鍵勾選、Enter 下一步（或執行）、Esc 上一步。每一頁在主控台印 `setup: page 名稱`（測試等它）。

| 頁 | 做什麼 |
|---|---|
| language | English／繁體中文 |
| timezone | 與 UTC 的時差（`+08:00`；`abi::time::parse_offset` 檢查），結束時寫 `Data:/.stela/timezone` |
| data | 有 Data：顯示大小與剩餘空間。沒有：列出可以變成 Data 的地方（§4.1），選一個、**打它的完整名稱**確認、Enter 才格式化 |
| network | DHCP 的位址、DNS；`pkg.conf` 有伺服器時跑一次 `pkg update`（HTTPS + 驗簽），顯示結果 |
| packages | index 的套件（`pkgcore::ops::index`），空白鍵勾選、Enter 安裝（和 `pkg install` 同一段程式，進度印在主控台） |
| classic | 其他卷宗（不是 Data、Initrd、Tmp）最上層的 `*.rom`、`*.img`、`*.dsk`、`*.hfs`，勾選後複製到 `Data:/Classic/`（`docs/classic.md`） |
| display（P14 K2） | 三行，上下選、左右改：介面倍數（自動／1x／2x，下次開機起）、熄屏時間（永不、1、5、10、30、60 分鐘，預設 10）、桌面圖樣（素色、回紋、波浪）；Enter 印 `setup: display scale=… blank=… pattern=…` |
| done | 寫 `language`、`timezone`、`display`（`scale=auto\|1\|2`、`blank=分鐘`）、`desktop`（`pattern=…`）、`setup-done`，關視窗，啟動 Mouseion |

### 4.1 格式化：`VolumeFormat`（syscall 65）

```
 op 0（列出，誰都可以）：已經有 Data → 空的清單
   每顆不是 USB 的磁碟：GPT（或 MBR）的每個分割區（ESP 除外），沒有分割表的整顆磁碟
   第一個 4 KiB 全是 0（從沒格式化過）的才算
   名稱：磁碟名 + "." + 分割區序號（ahci0p1.1），整顆磁碟就是磁碟名
 op 1（格式化，要 POWER 能力）：名稱必須還在清單裡、第一塊仍然全 0（否則 Busy）
   ehfs::format（卷宗名 Data、亂數 UUID、現在的時間）→ 照開機時的規則掛上（成為 Data）
```

- 安全規則：**只有從沒用過的地方**（全 0）會被提出、會被格式化；有任何資料的分割區、ESP、USB 磁碟、已經有 Data 時一律不會。實機上也一樣：Setup 不會自動格式化任何東西，一定要使用者選擇並打出名稱。
- 測試只用 `build/images/setup.img`（64 MiB，一個從沒格式化的 GPT 分割區）。

## 5. 測試

| 測試 | 內容 |
|---|---|
| host `toolbox::i18n` | 語言代碼、`tr`、`tr_menu` 保留快捷鍵、沒有翻譯的回原文 |
| `make test-setup` | `boot.cfg` = `setup`、只有 `setup.img`：init 印 `first start: Setup.app`；QMP 按鍵：↓ Enter（繁體中文）、Enter（時區 +08:00）、打 `ahci0p1.1` Enter（格式化：`vfs: ahci0p1.1 formatted as Data`、掛上）、Enter、network 顯示「套件伺服器：沒有設定」（中文）、Enter（沒有套件）、Enter（沒有 Classic 映像）、Enter → display 頁：Right、Down、Right、Down、Right、Enter（`scale=1 blank=30 pattern=meander`）→ Enter → `setup: done`、`mouseion: ready`；host 的 fsck、讀出 `.stela/language` = `zh-TW`、`setup-done`、`display`、`desktop`；P14 K2：同一顆磁碟用一般的 ESP 再開機，`windowserver: scale 1`（設定檔生效；2560 寬本來是 2） |

套件頁的安裝和 `pkg install` 是同一段程式（`make test-pkg`）；新格式化的 Data 沒有 `pkg.conf`，所以 test-setup 的套件頁是空的（計畫外的簡化）。

## 6. 已知限制

- 只有鍵盤操作（滑鼠點選沒有）。
- 時區只有固定的 UTC 時差（`docs/time.md` §5.6）。
- 新的 Data 卷宗沒有 `pkg.conf`：要使用者自己寫伺服器（還沒有正式的套件伺服器）。
- Classic 頁只看其他卷宗的最上層、用副檔名判斷，不檢查內容。
- 格式化只做 eHFS「Data」；不能重新分割、不能縮小或刪除分割區。
