# 測試

STELA 的測試分三層：host 上的單元測試（純邏輯）、QEMU 開機的整合測試（serial 輸出、QMP 打字與點滑鼠、截圖檢查），以及 P11 加的 fuzz（host）。`make test` 跑 host 測試、fuzz 與所有 QEMU 測試，CI 也跑同一組；需要使用者自己的檔案（Mac ROM）或很久的測試只在本機跑。

## 1. 比喻：三種檢查

- 單元測試像檢查每一個零件的尺寸（`cargo test`：B-tree、TCP 狀態機、路徑解析）。
- QEMU 測試像把整台機器組起來開機，照著劇本操作，看螢幕與 serial 有沒有出現預期的字。
- fuzz 像把零件丟進搖晃的箱子：輸入是**故意弄壞的**合法資料，只檢查「不會壞掉」——不 panic、不卡住；壓力測試則是讓機器同時做很多事，事後檢查資源有沒有漏掉。

## 2. 測試目標

`make test` 依序跑下表「進 make test」的目標（約 8 分鐘）；其他的只在本機。每個 QEMU 測試用 `tools/qemu_test.py`：等 serial 出現某一行就經 QMP 打字（`--type`，`{up}`、`{tab}` 等鍵）、點滑鼠（`--click`）、下 QMP 指令（`--qmp`），檢查預期的行**依序**出現（`--expect`）、截圖（`--screendump`）後用 `tools/fb_check.py` 檢查畫面；`--log` 把 serial 存到 `build/test-*.log`。

| 目標 | 進 make test | 內容 |
|---|---|---|
| `test-host` | ✓ | 圖檔與 `assets/generated/` 一致；`HOST_CRATES` 的 `cargo test` 與 `HOST_LIB_CRATES` 的 `cargo test --lib`（純邏輯：B-tree、eHFS、FAT、GPT、TCP、ACPI 表、排程佇列、工具箱、Periplus…） |
| `test-fuzz` | ✓ | §3：11 個解析器的 fuzz，最多 60 秒 |
| `test-qemu` | ✓ | selftest kernel：鍵盤（QMP 打 `stela`）、滑鼠、使用者程式、IPC、行程、`selftest: OK`、exit code 33 |
| `test-qemu-smp` | ✓ | 同一個 kernel、`-smp 4`：每顆 CPU 的 tick、跨 CPU 喚醒與 kill |
| `test-qemu-min` | ✓ | 最低硬體：`-cpu Conroe,-nx -m 256M`、單核，加 AHCI 測試磁碟跑 fstest |
| `test-textmode` | ✓ | `boot.cfg` = `textmode`、`nousb`、`verbose`、`waitkey`：init 直接開 shell |
| `test-disk-image` | ✓ | `make_disk_image.py` 的映像當唯一磁碟：桌面、`mkdir`、`shutdown`、host fsck、第二次開機讀回 |
| `test-net` | ✓ | host 的 HTTP 伺服器；e1000、e1000e、virtio-net 各一次：`ifconfig`、`ping`、DNS、`fetch`、Linux 的 socket |
| `test-sound` | ✓ | intel-hda 錄成 wav：`tone` 1 秒約 440 Hz |
| `test-window` | ✓ | 桌面、terminal、`run echo_win`；截圖檢查選單列、標題列與文字 |
| `test-storage` | ✓ | AHCI（GPT：FAT ESP + eHFS）與 NVMe（4Kn）兩次開機的持久性、fstest、host fsck |
| `test-shell` | ✓ | shell 指令一個接一個（`-smp 2`）：檔案、程式、歷史與游標、管線、`cp -r`、`find`、`date`、萬用字元、Tab 補全、背景工作 |
| `test-pipe` | ✓ | 管線、`>`／`>>`／`<`、Linux 程式進管線、背景工作的 `kill` 與 `fg` |
| `test-shutdown` | ✓ | `shutdown`（QEMU 自己結束、fsck 乾淨）與 `reboot` |
| `test-linux` | ✓ | Linux 人格：`hello`、`files`、`memory`、`time`、`nosys`、`tty` |
| `test-usb` | ✓ | xHCI：USB 鍵盤、滑鼠、hub、熱插拔、USB 隨身碟（FAT 讀寫、拔掉時卸載） |
| `test-panic` | ✓ | 故意 panic：截圖檢查裂開的石碑與最後幾行 log |
| `test-mouseion` | ✓ | 雙擊卷宗、文字檔（Viewer）、Initrd、Terminal.app；截圖檢查 |
| `test-editor` | ✓ | Kalamos：打中英文、Alt-S 存檔、重開讀回 |
| `test-browser` | ✓ | Periplus：host 的頁面、連結、重新導向、chunked |
| `test-classic` | 本機 | 使用者的 Mac ROM 與 System（`~/mac/`，只讀，複製進 `build/`）：開到 Finder、截圖、電源與第二顆磁碟 |
| `test-ehfs-compat` | 本機 | 與 68k_HAL 的 eHFS 工具交叉檢查 |
| `bench` | 本機 | 量測（記憶體、syscall、執行緒切換、eHFS、TCP），`build/bench.md`（`docs/bench.md`） |

## 3. fuzz（`make test-fuzz`，P11 K45，使用者尚未審核）

```text
 種子（合法的輸入） ──► 變異 1–8 次：翻一個位元、亂數位元組、邊界值（0、0x7F、0x80、
                        0xFF、0xFFFF、0xFFFFFFFF…，1/2/4/8 位元組、兩種位元組順序）、
                        截斷、插入、重複一段、刪一段、複製一段
                    ──► 目標（catch_unwind 裡）
 監視執行緒：10 秒沒有進展 → 「卡住」；panic → 輸入存到 build/fuzz-<目標>.bin，exit 1
```

- `tools/fuzz`（host 的 binary crate，workspace 成員）：`cargo build --profile fuzz`——`fuzz` profile 繼承 release（最佳化）但保留 overflow 與 debug 檢查、panic 會 unwind（才能被抓到並回報輸入）。亂數（xorshift64*）的種子固定（目標名稱的長度；`FUZZ_SEED=n` 可以換一組），所以失敗會重現。
- 用法：`build/target/fuzz/fuzz BUILD_DIR [TIME_PERCENT [TARGET]]`；每個目標做完它的次數或到它的時間上限就停（時間乘上 TIME_PERCENT/100），總共最多約 60 秒；TARGET 只跑那一個。
- 磁碟映像不複製：`Overlay` 是種子映像上面一層「改過的區塊」（`BTreeMap<區塊, 4 KiB>`），實作 `BlockDevice`；先用沒改過的映像跑一次、記下讀過的區塊（中繼資料），之後只改這些區塊。eHFS 的 B-tree 節點（16 KiB，前 4 位元組是後面的 CRC-32C）與超級區塊有 CRC：四次有一次把改過的節點與超級區塊**重新蓋 CRC**，變異才進得到解析的程式碼；GPT 一半的時候重算表頭與分割區陣列的 CRC-32。
- 網路一半的時候重算 IPv4 表頭與 TCP／UDP／ICMP 的 checksum；另外十六次有一次讓兩個 `net::stack::Stack` 真的對話（ARP、ping、UDP、TCP 連線、兩邊送資料、關閉），中間的封包隨機丟掉、重送、變異，時間隨機前進（重傳計時器），40 回合。

| 目標 | 種子 | 跑什麼 | 次數／上限 |
|---|---|---|---|
| `ehfs` | 程式裡造的 8 MiB 卷宗（三層資料夾、空到 300 KB 的檔案、resource fork、屬性、60 個名字、64 區塊的 journal） | 唯讀掛載、走訪（深 8 層、500 項）、讀 fork 與屬性；十六次有一次可寫掛載、建檔、卸載 | 2 萬／25 s |
| `fat` | `build/images/fat-test.img`（mtools 的 FAT32） | 掛載、走訪、`lookup`、讀檔、`resolve` | 2 萬／15 s |
| `gpt` | `build/images/gpt-test.img`（sgdisk） | `gpt::read`、`blocks_of` | 3 萬／5 s |
| `mbr` | 兩個分割區的 MBR、保護性 MBR | `mbr::read` | 10 萬／2 s |
| `ustar` | 程式裡造的 tar（資料夾、bundle、用 prefix 欄的長名字） | `entries`、`find`、`Tree::new` 與走訪 | 5 萬／2 s |
| `net` | 一段乾淨對話的所有封包＋DHCP offer／ack | `parse_eth`／`arp`／`ipv4`／`echo`／`udp`／`tcp`、`dhcp::parse`；餵給有 listener 的 stack 與 DHCP 中的 stack；對話 | 10 萬／4 s |
| `http` | 200（HTML）、302、chunked、一個 URL | `parse_head`、`dechunk`、`is_html`、`join`、`net::parse_response`、`parse_url` | 10 萬／2 s |
| `dns` | 有 CNAME 與 A 的回答、查詢、名字 | `dns_answer`、`dns_query` | 10 萬／2 s |
| `html` | 有各種標籤、實體、表格、中文的頁面 | Periplus 的 `parse`、`layout`（80 欄與窄欄）、`decode`、`tokens`、`plain` | 2 萬／2 s |
| `config` | `boot.cfg`、`classic.cfg`、時區字串 | `bootinfo::parse_config`、`Config::parse`、`time::parse_offset`、`KeyEvent`／`MouseEvent::decode` | 10 萬／2 s |
| `path` | 路徑、管線、有 Tab 的行 | `path::resolve`／`glob`／`split_last`／`common_prefix`、`pipeline::parse`、`LineEditor`（字元與方向鍵混著打） | 10 萬／2 s |

- toolbox 的 CJK 字型平常經 STELA 的檔案 syscall 載入；host 上不能發 STELA 的 syscall，所以 fuzz 先用 `cjk::set_font` 給它 repo 裡的字型檔。
- eHFS 每次 lookup 都讀 16 KiB 的節點並逐位元組算 CRC-32C（沒有節點快取），一個輸入約 1.5 ms，25 秒約 1.6 萬次（略少於 2 萬）。

### 3.1 找到的問題（當場修了）

1. `abi::time::parse_offset`：`+` 後面接一個 3 位元組的字（`+中a`）時，`&rest[..2]` 切在字元中間而 panic。時區檔 `Data:/.stela/timezone` 是使用者可寫的，kernel 的 Linux 人格（`TZ`）也讀它——這會讓 kernel panic。修法：不是 ASCII 就回 `None`（abi 的 host 測試加了這個例子）。
2. eHFS 的 B-tree：節點的 CRC 對，但 count／offset 表／記錄長度指到節點外面時，`key_of`、`value_of` 越界 panic（一顆壞掉的磁碟就能讓 kernel panic）。修法：`read_node` 在 CRC 之後檢查節點**結構完整**（`btree::well_formed`：記錄都在 `DATA`..`FREE` 之間且不重疊、offset 表放得下、index 節點的值至少 8 位元組放得下子節點編號），不完整就是 `Corrupt`。

