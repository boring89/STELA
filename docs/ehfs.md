# eHFS

> P5 由自動排程一次做完，使用者已於 2026-10-20 審核。

eHFS 是 STELA 的原生檔案系統（`docs/architecture.md` §4 的 E）。磁碟格式**與 68k_HAL 的 eHFS 第 1 版完全相容**：同一個卷宗在兩個系統都能讀寫。**格式的權威來源是 68k_HAL 的 `docs/ehfs.md`**（以及它的 `kernel/fs/ehfs/format.h`）；這份文件是摘要、Rust 實作的結構，以及和 68k_HAL 實作的差異。程式碼全部重寫（Rust），沒有複製 C 程式碼。

程式碼：

```
ehfs/src/format.rs        磁碟格式的所有偏移與常數（對照 format.h）
ehfs/src/crc32c.rs        CRC-32C（Castagnoli）
ehfs/src/volume.rs        Volume：superblock、交易、日誌、mount/unmount、bitmap
ehfs/src/btree.rs         B+-tree
ehfs/src/catalog.rs       catalog：Entry、lookup/get/list、建立、刪除、搬移
ehfs/src/fork.rs          fork 的 extent（inline 8 段 + extents 樹）
ehfs/src/file.rs          檔案操作（各自的交易）、延伸屬性
ehfs/src/mkfs.rs          format
ehfs/src/check.rs         fsck
ehfs/src/name.rs          名稱：合法性、比較形式（NFC + simple case folding）
ehfs/src/unicode_data.rs  Unicode 15.0.0 的表（tools/gen_casefold.py 產生）
tools/ehfs/               mkfs.ehfs、fsck.ehfs、ehfs-cp、ehfs-ls（host）
tools/ehfs_compat.py      與 68k_HAL 工具的交叉檢查（本機）
```

## 1. 比喻：有草稿本的卡片櫃

eHFS 像 Classic Mac 的 HFS：一個**卡片櫃**（B-tree 目錄），每個檔案有固定的**編號**（CNID，改名搬家都不變），每張卡片有兩個**口袋**（data fork、resource fork）與 Finder 資訊。不同的是每一欄都放大（64 位元），而且**改卡片前先寫在草稿本上**（日誌）：寫到一半停電，下次開機照草稿本重做一次，卡片櫃永遠不會只改一半；每張卡片還有**驗算碼**（CRC-32C），被弄壞看得出來。

## 2. 格式摘要（細節以 68k_HAL `docs/ehfs.md` 為準）

```
 區塊 0      0–1023 保留；1024–4095 superblock（3072 bytes，小端序，CRC-32C 在 3068）
 1 ..        allocation bitmap（一個位元一個 4 KiB 區塊）
 ..          日誌（卷宗的 1/256，260–16384 塊）
 ..          catalog（32 節點）、extents（8）、attributes（8）B-tree 的檔案
 最後一塊    superblock 的副本（byte 1024）
```

- superblock：magic `"eHFS\0\r\n\x1a"`、版本 1、區塊大小 4096、區塊數、空閒數、下一個 CNID、檔案／資料夾數、compat／ro_compat／incompat（不認得 incompat 就拒絕掛載）、狀態（clean／mounted／errors）、UUID、時間、6 個特殊檔案的 fork 描述（144 bytes：長度、區塊數、8 段 extent）、卷宗名、Unicode 版本（1240，`0x0F0000`）、節點大小（1244，16384）。
- CNID：1 根的父親、2 根、3–8 特殊檔案、16 起配給檔案；不重用；不超過 2³²−1（E5，Mac 端是 32 位元）。
- B-tree 節點 16 KiB：32 bytes 標頭（CRC、種類、高度、筆數、前後節點、空閒起點），紀錄從前往後（u16 鍵長、鍵、u16 值長、值），偏移表從尾端往前。節點 0 是標頭節點（根、深度、節點數、空閒數、筆數、首尾 leaf、節點使用 bitmap）。**鍵裡的整數是大端序**，三棵樹都用位元組比較。
- catalog：鍵 = 父 CNID（8 bytes BE）+ 名稱的比較形式；thread 紀錄的鍵只有 CNID（排在該資料夾的子項目前）。檔案紀錄含兩個 fork 描述與原名（UTF-8）。
- extents：鍵 = CNID（BE）+ fork（0/1）+ 起始檔案區塊（BE），值 = 8 段。
- attributes：鍵 = CNID（BE）+ 名稱（原樣、分大小寫）；值 = u16 種類 1、u32 長度、內容（≤ 3 KiB）。
- 日誌：header（"eHFSjrnl"、已寫回的序號、CRC）＋一個交易：描述區塊（"eHFStxn\0"、序號、數量、目標區塊號）、內容、提交區塊（"eHFScmt\0"、序號、數量、CRC-32C 涵蓋描述與內容）。

## 3. Rust 實作的結構

```rust
pub struct Volume<D: BlockDevice> {   // 卷宗 = D 的區塊 0 起（分割區用 block::Partition）
    sb: Vec<u8>,                       // 記憶體中的 superblock（交易內修改）
    txn: Vec<(u64, Box<[u8; 4096]>)>,  // 交易改過的 metadata 區塊（只配用到的）
    ...
}
```

- **所有 metadata 都經過交易**：`begin` → 修改（`write_meta` 放進 `txn`；`read_block` 先看 `txn`）→ `commit`：寫日誌（描述、內容、提交）→ 寫回原位 → 寫日誌 header。`finish(result)` 成功就 commit、失敗就 abort（superblock 從磁碟讀回）。
- **資料是 ordered**：檔案內容直接寫到它的區塊（`write_data`），**之後**才提交「把這些區塊給這個檔案、改長度」的交易。新區塊沒寫到的部分補 0。大的寫入、截短、刪除切成每個交易最多 256 個檔案區塊。
- mount：檢查 superblock → 找日誌裡完整、比 header 新的交易 → 讀寫掛載就寫回原位、更新 header；**唯讀掛載不寫，交易的區塊留在記憶體裡讀穿**（fsck 用）→ 讀寫掛載時狀態設成 mounted；unmount 設回 clean 並 flush。
- 公開 API（`Volume` 的方法）：`mount`、`unmount`、`lookup(parent, name)`、`get(cnid)`、`list(dir)`、`create`、`rename`、`remove`、`read`、`write`、`set_length`、`set_info`、`attr_set/get/remove/names`、`check`、`format(dev, options)`。
- 記憶體：B-tree 節點每次操作配 16 KiB 的 `Vec`；交易只配實際改到的區塊（68k_HAL 是固定 1 MiB 的陣列）。

### 3.1 B-tree（和 68k_HAL 相同的規則）

- 搜尋走到「分隔鍵不大於鍵的最後一個子節點」（沒有就第一個）。
- 分裂：原有紀錄加上新的一筆依序分到原節點與新節點，各約一半位元組（至少各一筆）；新節點的第一個鍵是分隔鍵；根分裂時新根的最左子節點用**空鍵**。
- 刪除：節點空了才釋放（不合併）；index 節點刪掉第 0 筆時，新的第 0 筆沿用舊鍵（**index 節點的第一個鍵永遠是整棵子樹的下界**）；根只剩一個子節點就降一層。
- 樹的檔案用完時加倍（最多 8 段，在 superblock 的 fork 描述裡）。

### 3.2 名稱（E1、E2）

- 合法：1–255 bytes 的合法 UTF-8，不含 NUL 與 `/`。
- 比較形式：NFC（UAX #15：標準分解、標準排序、標準組合，含韓文音節演算法）再做 simple case folding（CaseFolding.txt 的 C 與 S）；`ß` 不會變 `ss`。
- **表格的來源**：`tools/gen_casefold.py` 讀 Unicode 15.0.0 的 UCD（UnicodeData.txt、CompositionExclusions.txt、CaseFolding.txt），用 SHA-256 確認是正確的檔案，產生 `ehfs/src/unicode_data.rs`（保留 Unicode License V3 全文——寬鬆授權，與 GPL 相容）。沒有下載：檔案取自 68k_HAL 既有的 `third_party/downloads/unicode-15.0.0/`。
- `--compare` 把產生的表與 68k_HAL 的 `unicode_data.h` **逐項比對：922 個 class、2061 個分解、3087 個分解碼點、941 個組合、1454 個 folding，完全相同**。
- 官方 NormalizationTest.txt（Unicode 15.0，約 19,000 行、95,000 個 NFC 檢查）在本機全部通過（`EHFS_NORMALIZATION_TEST=路徑 cargo test -p ehfs`）。

## 4. 和 68k_HAL 實作的差異

| 項目 | 68k_HAL | STELA |
|---|---|---|
| 裝置 | 512 bytes 的扇區，卷宗從某個扇區起 | 4 KiB 的 `BlockDevice`；分割區用 `Partition`（工具的 `--offset` 仍是 512 bytes 扇區） |
| 交易緩衝 | 固定 256 × 4 KiB | 只配改到的區塊 |
| 測試的斷電 | `crash_after`：之後的寫入回 EIO | `MemDisk::cut_power_after`：之後的寫入**靜靜消失**（更像真的斷電） |
| fsck 的 bitmap 檢查 | 每個 bitmap 區塊重走一次所有擁有者（記憶體固定） | 一次建好整個預期的 bitmap（記憶體 = 區塊數 / 8） |
| fsck 順序 | 有錯也會做 bitmap 檢查 | 前面有錯就不做 bitmap 檢查 |
| Linux 工具的 xattr | `ehfs-cp` 把資源 fork、Finder 資訊對應到 xattr | 沒有（只有 data fork）；`ehfs-ls` 另外有 |
| `--device` | 有（明確允許區塊裝置） | **沒有**：目標必須是一般檔案，沒有例外 |
| 時間 | RTC + 單調時鐘 | 還沒有 RTC 驅動：2026-01-01 + 開機以來的時間（`kernel::fs::now`） |

格式上沒有差異。

## 5. 工具（`make ehfs-tools` → `build/host/ehfs/`）

| 工具 | 用法 |
|---|---|
| `mkfs.ehfs` | `mkfs.ehfs [-L 名稱] [--size MiB] [--offset 扇區] [--sectors N] [--journal 區塊] 映像` |
| `fsck.ehfs` | `fsck.ehfs [--offset 扇區] 映像`：只檢查；0 乾淨、1 有問題、2 讀不了；也報告是否正常卸載 |
| `ehfs-cp` | `ehfs-cp [--offset S] 映像 in 本機檔 路徑`、`out 路徑 本機檔`、`ls [路徑]`、`mkdir 路徑`、`rm 路徑`（指令和 68k_HAL 相同） |
| `ehfs-ls` | `ehfs-ls [--offset S] 映像 [路徑]` |

- Cargo 的執行檔名不能有 `.`，所以建出來是 `mkfs-ehfs`、`fsck-ehfs`，`make ehfs-tools` 以 `mkfs.ehfs`、`fsck.ehfs` 的名字放進 `build/host/ehfs/`。
- **只接受一般檔案**（`metadata().is_file()`）：`/dev/sda` 等區塊裝置一律拒絕，沒有覆寫的選項。

## 6. 測試

| 測試 | 內容 |
|---|---|
| host `ehfs` | CRC-32C 標準向量與串接；名稱（大小寫、NFC、韓文、希臘文 σ/ς、ß、組合記號排序、不合法 UTF-8）；mkfs／唯讀與讀寫 mount、狀態、損壞的 superblock、不認得的 incompat；資料夾與檔案、不分大小寫與 NFC 的查詢、改名（只改大小寫）、搬移、資料夾不能搬進自己、不能刪非空資料夾；2000 個檔案（B-tree 分裂）與交錯刪光（降層）；跨區塊與空隙補 0 的讀寫、40 段交錯的碎片檔（進 extents 樹）、1.5 MB（多個交易）、截短與加長、資源 fork、刪除後區塊歸還；延伸屬性（替換、3 KiB 上限、刪除、列出）、Finder 資訊與時間、CNID 不重用；**在交易的每一個寫入點斷電**（重新 mount 後 fsck 乾淨，新檔案要嘛完整要嘛不在，唯讀 mount 也行）；600 步的隨機操作對照模型。每個測試最後跑 fsck |
| host `ehfs-tools` | 拒絕 `/dev/null`、`/`、不存在的路徑與 `--device`；分割區偏移上的 mkfs／寫入，前面的位元組不變 |
| `make test-storage` | kernel 在 AHCI 的 Data 與 NVMe（4Kn）的 Archive 上讀寫，開機兩次，結束後 host 的 `fsck.ehfs` 乾淨且「正常卸載」 |
| `make test-ehfs-compat`（本機） | STELA 的工具做的映像：68k_HAL 的 `fsck.ehfs` 乾淨、`ehfs-cp out` 讀回相同；68k_HAL 做的映像：STELA 的 fsck 乾淨、讀回相同、寫入後 68k_HAL 的 fsck 仍乾淨。另外手動以 68k_HAL 的 fsck 檢查過 STELA kernel 寫過的 Data 分割區：乾淨 |

## 7. 已知限制

- 沒有 syscall 暴露資源 fork、Finder 資訊與延伸屬性（格式支援，API 只在 crate 與工具裡）。
- `list` 一次讀出整個資料夾；P11 起 `list_after(parent, after, max)` 從某個名字之後走 B-tree（`DirRead` 的 cookie 用它，`docs/vfs.md` §4）。
- 一個卷宗一次一個操作（`SleepMutex`）。
- 沒有 RTC，時間戳是「2026-01-01 + 開機以來」。
- 名稱轉換成 Mac Roman 31 字元（68k_HAL 的 FSM 端）與 System 卷宗、A/B 更新不在 STELA 的範圍內（之後）。
