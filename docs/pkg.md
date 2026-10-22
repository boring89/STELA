# pkg：套件（P12）

> P12 由自動排程一次做完，使用者已於 2026-10-22 審核。

`pkg` 從套件伺服器安裝程式：ports repo（`docs/ports.md`）編好的 tar 檔，加上一份有簽章的清單（index）。程式碼：`user/pkgcore`（邏輯，也給 Setup.app 用）、`user/pkg`（命令列，`Initrd:/bin/pkg`）、`tools/pkgsign.py`、`tools/make_test_packages.py`。

## 1. 比喻：郵購目錄

伺服器是一家郵購公司。**目錄**（`index.json`）列出每樣商品的名稱、版本、價錢（大小）、說明，以及每個包裹的**指紋**（SHA-256）。目錄本身蓋著公司的**鋼印**（ed25519 簽章，`index.json.sig`）；STELA 出廠時就帶著這家公司鋼印的樣本（`Initrd:/certs/pkg.pub`，公鑰）。

```
 pkg update：拿目錄與鋼印 ──► 用 pkg.pub 驗鋼印 ──► 對了才收下（Data:/.pkg/index.json）
 pkg install：照目錄訂貨 ──► 包裹的指紋和目錄寫的一樣？ ──► 拆箱放進 Data:/Programs/名稱/
```

就算有人在路上掉包（或伺服器被入侵、換了包裹），指紋對不上；就算有人連目錄一起換，鋼印對不上。HTTPS（`docs/tls.md`）另外保護路上的內容。

## 2. 指令

```
pkg update               拿新的 index（Data:/.stela/pkg.conf 的伺服器），驗簽章
pkg list                 已安裝的（名稱 版本）
pkg search [字]          index 裡名稱或說明有這個字的
pkg install 名稱...      連同依賴一起裝（依賴先裝；已安裝的跳過）
pkg install 檔案.tar     裝本機的套件檔（相對於目前資料夾，或 Volume:路徑）
pkg remove 名稱          刪掉 Data:/Programs/名稱/
pkg upgrade              update 之後，版本和 index 不同的重裝
```

## 3. 檔案

```
Data:/.stela/pkg.conf        server = https://伺服器/路徑/     （必須）
                             ca = Volume:/檔案.der             （可選：多信任的根憑證）
Data:/.pkg/index.json        驗過簽章的 index
Data:/.pkg/installed.json    {"format": 1, "installed": [{"name", "version"}, ...]}
Data:/Programs/名稱/          套件的內容（PKGINFO、bin/、lib/、share/ 或 Name.app/）
Initrd:/certs/pkg.pub        index 的公鑰（hex，32 bytes）——目前是測試鍵（§4）
```

- index 的格式（`portbuild.py index`、`pkgsign.py index` 產生）：`{"format": 1, "packages": [{"name", "version", "description", "license", "install", "depends", "size", "sha256", "file", "url"}]}`；`pkgcore::json` 是一個小的 JSON 解析器（物件、陣列、字串含 `\uXXXX`、整數、true/false/null，深度 ≤ 32）。
- 套件檔：ustar，最上層是 `PKGINFO`（`name = …`、`version`、`description`、`license`、`install`、`depends`，空白分隔）。解開時拒絕絕對路徑、`..`、連結與裝置（`tar_entries`）。
- 名稱只能是英數字與 `-`、`_`、`.`（不是 `.`、`..`），所以 `Data:/Programs/名稱` 不會跑到別的地方。

### 3.1 找得到裝好的程式

- STELA 的 shell：打名字時依序找目前資料夾、`Initrd:/`、`Initrd:/bin/`，最後是 **`Data:/Programs/*/bin/`**（套件名稱的順序）。計畫的選擇是「搜尋路徑」，不把檔案複製到一個共同的 `bin`。
- Linux 程式的 `PATH`：kernel 在啟動時列出每個有 `bin` 的套件：`/Data/Programs/名稱/bin:…:/Initrd/linux:/Initrd`（`docs/linux.md` §6）。
- `install = "app"` 的套件把 `Name.app/` 放在 `Data:/Programs/名稱/` 裡，Mouseion 打開那個資料夾就看得到。

## 4. 簽章：`tools/pkgsign.py`

```
pkgsign.py keygen KEYFILE PUBFILE    測試鍵
pkgsign.py sign KEYFILE INDEX        INDEX.sig（hex 的 64-byte ed25519 簽章）
pkgsign.py index KEYFILE DIR URL     DIR/*.tar 的 index.json + .sig
```

- 驗證：`ed25519-dalek` 2（BSD-3-Clause）的 `verify_strict`（RFC 8032；host 測試用 RFC 8032 §7.1 的測試向量）。使用者空間沒有 SSE，curve25519-dalek 用 `--cfg curve25519_dalek_backend="serial"`（`.cargo/config.toml`）。
- **測試鍵**：`keygen` 的私鑰是一句固定句子的 SHA-256，所以 repo 裡的 `tools/pkg.pub`（進 initrd 的 `certs/pkg.pub`）和每次建置在 `build/pkg-signing.key` 產生的私鑰永遠配對，CI 的 `make test-pkg` 才能簽。**任何人都能用它簽**——真正對外提供套件之前，使用者要自己用 `cryptography` 的 `Ed25519PrivateKey.generate()` 產生正式的鑰匙（私鑰不進 repo），換掉 `tools/pkg.pub`。
- ports repo：`PKG_SIGNING_KEY=鑰匙檔 portbuild.py index` 也會寫 `dist/index.json.sig`。

## 5. 測試

| 測試 | 內容 |
|---|---|
| host `pkgcore` | JSON（往返、跳脫、代理對、錯誤、深度）、index（解析、壞名字、格式版本）、依賴順序（已安裝的跳過、不認得的、循環）、hex、SHA-256（"abc"）、ed25519（RFC 8032 測試 2、改一個 byte 就不對）、ustar（PKGINFO、資料夾、`..`、連結、checksum）、pkg.conf、installed.json |
| `make test-pkg` | `make_test_packages.py`：`greet`（bin/greet = Linux 的 hello）與 `greetdoc`（share/hello.txt，依賴 greet）；`pkgsign.py index` 用測試鍵簽；`with_https.py` 提供（測試根簽的憑證，Data 映像裡放 `test-ca.der` 與 `.stela/pkg.conf`）。shell：`pkg update`（2 個）、`pkg search greet`、`pkg install greetdoc`（先裝 greet）、`pkg list`、`run greet installed`（shell 從 `Data:/Programs/greet/bin` 找到）、`cat` 套件的檔案、`pkg remove greetdoc`、`pkg install nothere`（沒有這個套件）；host 的 fsck |
| `make test-ports`（本機） | 真的 ports 也可以用：`PKG_SIGNING_KEY=… portbuild.py index`，把 `dist/` 放到 HTTPS 伺服器（`docs/ports.md`） |

## 6. 已知限制

- 沒有正式的套件伺服器（`pkg.conf` 要自己寫），公鑰是測試鍵。
- System 映像用另一把鑰匙（`tools/system.pub`，P14；`docs/system.md` §3）：套件鑰匙外洩也簽不出會被開機的 System。兩把都是測試鍵。
- index 沒有版本號與過期時間：舊的（簽過的）index 可以被重送（rollback）；`upgrade` 只比較版本字串是否不同。
- 安裝不是交易式的：中途失敗留下一半的資料夾（下次 install 會先整個刪掉再解）；同時跑兩個 pkg 會互相覆蓋 `installed.json`。
- 沒有「誰依賴我」：`remove` 不檢查別的套件是否需要它。
- 套件全部在記憶體裡下載與解開（大小受程式的 heap 限制）。
