# 發布（release）

> P14 E（K17–K18）由自動排程一次做完，**使用者尚未審核**。這一輪只寫好做法與工具，**沒有 tag、沒有上傳**。

一次發布是把某個 commit 的 STELA 做成一組可以下載的檔案：能直接寫進隨身碟開機的磁碟映像、單獨的 System 映像（給 `stela-update`）、ESP 的檔案、所有 port 的套件與簽好的 index，加上 SHA-256 清單與自動產生的說明。

## 1. 版本

- repo 根目錄的 `VERSION`（一行，`0.2.0`）是 STELA 的版本；Cargo workspace 的 `version`（`Cargo.toml` 的 `[workspace.package]`）要一樣——kernel 的開機訊息、`uname -r`（`0.2.0-stela`）、`hwinfo`、Mouseion 的 About 都用 Cargo 的版本，System 映像的尾端與 `VERSION` 檔用 `VERSION`。`make release` 檢查兩者相同。
- 版本號照 SemVer 的形狀但還沒有相容的承諾（0.x）：每個 P<n> 完成並審核後加次版本（0.2 → 0.3），只修 bug 的發布加修訂號（0.2.0 → 0.2.1）。
- `make test-update` 自己用 `VERSION` 的修訂號 +1 做「下一版」，不需要改。

## 2. `make release`

```
make release            → build/release/stela-0.2.0/
  stela.img             磁碟映像：GPT = ESP（loader、kernel、initrd、boot.cfg 範例）+ System A（system.img）+ System B（空）+ Data（RELEASE_DATA_MIB，預設 512 MiB，空的：第一次開機跑 Setup）
  system.img            System 卷宗映像（16 MiB，尾端有版本、SHA-256、System 鑰匙的簽章；docs/system.md §3）
  esp/                  ESP 的檔案（EFI/BOOT/BOOTX64.EFI、STELA/kernel.elf、initrd.tar、boot.cfg）
  ports/                ~/stela-ports/dist 的 *.tar、index.json、index.json.sig（套件鑰匙簽）
  SHA256SUMS            上面每個檔案的 SHA-256（最後一行是 RELEASE_NOTES.md）
  RELEASE_NOTES.md      tools/release_notes.py 產生
```

步驟（`Makefile` 的 `release`）：

1. `VERSION` 與 Cargo 的版本不同就停。
2. 建 `esp`（kernel、initrd、System 映像），`tools/make_disk_image.py` 做磁碟映像——**不放使用者的 Mac 檔**（`make disk-image` 會在找到 `~/mac/` 時放進 `Data:/Classic/`，release 不經過那條路），做完再用 `ehfs-ls` 確認 Data 裡沒有 Classic。
3. `~/stela-ports/dist`（`PORTS_DIR=` 可改）沒有套件就先 `portbuild.py all all`（約 15 分鐘）；index 已經用 `tools/pkg.pub` 的鑰匙簽好、而且正好列出 dist 裡每個 `.tar` 與它的 SHA-256（`pkgsign.py verify`）就照用（ports 自己的 release）；否則重做 index 並用套件的**測試**鑰匙簽（`tools/pkg.pub` 換成正式的之後這條路會失敗，這是故意的）。`RELEASE_SYSTEM=檔案` 用別處簽好的 System 映像代替 `build/system.img`（版本要等於 `VERSION`）。
4. 找到任何 `*.key` 或 `*signing*` 檔就停（私鑰不能進 release）。
5. SHA256SUMS → RELEASE_NOTES.md（含 SHA256SUMS 的內容）→ 把 RELEASE_NOTES.md 的 SHA-256 補進 SHA256SUMS。

`RELEASE_NOTES.md` 的內容全部從 repo 來：開頭固定的「只在 QEMU 驗證、測試鑰匙」警語；`CLAUDE.md` 的協作清單（哪些元件、使用者是否審核過）；`docs/` 每份文件的標題與第一段；本文件最後的「已知限制」；檔案清單。要改說明就改那些來源，不要改產生的檔案。

## 3. tag 與上傳的規則（這一輪不做）

- tag 名 `v0.2.0`（`v` + `VERSION`），打在 `make clean && make -j6 test` 通過、使用者審核過的 commit 上；ports repo 打同名的 tag（一次 STELA 發布對應一次 ports 發布）。
- **鑰匙**：發布前兩把測試鑰匙都要換成正式的（`tools/pkg.pub`、`tools/system.pub`；docs/pkg.md §6、docs/system.md §3）。套件的私鑰放 ports repo 的 GitHub secret；**System 的私鑰只放在發布者自己的機器上**，`system.img` 在本機簽好再上傳（CI 不簽 System）。
- 上傳到 GitHub Releases，兩者都寫好了但**從來沒有執行過**（K18）：
  1. ports repo 的 `.github/workflows/build.yml`（`workflow_dispatch`，輸入 STELA 的 tag）：checkout STELA 那個 tag、建所有 port、每個在 QEMU 測、用 secret `PKG_SIGNING_KEY` 簽 index、做成 ports repo 同名 tag 的 **draft** release。第一次跑之前，六個 `copy-from` 的 port 要改成官方網址。
  2. 發布者在本機用正式的 System 鑰匙簽好 `system.img`，在 STELA 建同名 tag 的 draft release 並附上它。
  3. push `v0.2.0` tag：STELA 的 `ci.yml` 先跑 host 與 QEMU 的 job，再跑 `release` job：檢查 tag = `v` + `VERSION`、下載 ports 的 release（沒有就在 CI 裡建）、下載 draft 上的 `system.img`（沒有就用測試鑰匙簽的並警告）、`make release`、把 `stela.img`、`system.img`、`stela-0.2.0-esp.tar`、`stela-0.2.0-ports.tar`、`SHA256SUMS`、`RELEASE_NOTES.md` 傳到 draft。
  4. 發布者檢查 draft 後手動發布。CI 從頭到尾不碰 System 的私鑰。
- 磁碟映像是 raw 檔（約 700 MiB，大部分是空的 Data）；上傳前是否壓縮（`xz`）由發布者決定（還沒決定，見 P14 的待決定）。

## 4. 檢查清單

1. `make clean && make -j6 test`、`make clippy`、`cargo fmt --all -- --check` 通過；本機的 `make test-classic`、`make test-ports`。
2. `VERSION`、`Cargo.toml` 的版本、`CLAUDE.md` 的協作清單（審核狀態）是最新的。
3. 換掉兩把測試鑰匙（§3）。
4. `make release`，看 `RELEASE_NOTES.md`；`make test-release`（本機專用：檢查 SHA256SUMS，用 `stela.img` 的快照在 QEMU 開機到 Setup 的第一頁、System 槽 a 是這個版本）；再手動走完 Setup 到桌面一次。
5. tag、push，讓 CI 的 release job 上傳。

## 已知限制

- **全部只在 QEMU（OVMF、TCG）驗證**；P520 等實機上的 I219、AX88179、HDA codec、HiDPI 螢幕、USB 隨身碟、12 執行緒都沒有驗證過。
- System 映像與套件 index 用的是**測試鑰匙**（固定種子，任何人都能簽）：這個版本的更新與套件驗證沒有安全性可言。
- 實機開機時，kernel 會把看到的任何 GPT 磁碟上的 eHFS 分割區掛成可寫（包括別的系統的資料分割區，docs/p520.md §1）；請只接要給 STELA 的磁碟。
- Wi-Fi 沒有：FreeBSD 的 iwm 只編進去、只跑 probe（docs/bsdkpi.md §6）；網路只有 Intel 有線網卡（e1000 系列、I219）、virtio-net 與幾種 USB 乙太。
- 沒有 ASLR、沒有 IOMMU；持有 `DEVICES` 能力的驅動程式等於持有整台機器（docs/devices.md）。
- Classic 環境需要使用者自己的 Mac ROM 與 System 磁碟，release 不含任何 Apple 的檔案。
- 其他限制見 `CLAUDE.md` 的「已知問題」與各文件的「限制」一節。
