# 測試

STELA 的測試分三層：host 上的單元測試（純邏輯）、QEMU 開機的整合測試（serial 輸出、QMP 打字與點滑鼠、截圖檢查），以及 P11 加的 fuzz（host）與壓力測試（本機）。`make test` 跑 host 測試、fuzz 與所有 QEMU 測試，CI 也跑同一組；需要使用者自己的檔案（Mac ROM）或很久的測試只在本機跑。

## 1. 比喻：三種檢查

- 單元測試像檢查每一個零件的尺寸（`cargo test`：B-tree、TCP 狀態機、路徑解析）。
- QEMU 測試像把整台機器組起來開機，照著劇本操作，看螢幕與 serial 有沒有出現預期的字。
- fuzz 像把零件丟進搖晃的箱子：輸入是**故意弄壞的**合法資料，只檢查「不會壞掉」——不 panic、不卡住；壓力測試則是讓機器同時做很多事，事後檢查資源有沒有漏掉。

## 2. 測試目標

`make test` 依序跑下表「進 make test」的目標（P12 起約 15 分鐘）；其他的只在本機。每個 QEMU 測試用 `tools/qemu_test.py`：等 serial 出現某一行就經 QMP 打字（`--type`，`{up}`、`{tab}` 等鍵）、點滑鼠（`--click`）、下 QMP 指令（`--qmp`），檢查預期的行**依序**出現（`--expect`）、截圖（`--screendump`）後用 `tools/fb_check.py` 檢查畫面；`--log` 把 serial 存到 `build/test-*.log`。

| 目標 | 進 make test | 內容 |
|---|---|---|
| `test-host` | ✓ | 圖檔與 `assets/generated/` 一致；`HOST_CRATES` 的 `cargo test` 與 `HOST_LIB_CRATES` 的 `cargo test --lib`（純邏輯：B-tree、eHFS、FAT、GPT、TCP、ACPI 表、排程佇列、工具箱、Periplus…） |
| `test-fuzz` | ✓ | §3：11 個解析器的 fuzz，最多 60 秒 |
| `test-qemu` | ✓ | selftest kernel：鍵盤（QMP 打 `stela`）、滑鼠、使用者程式、IPC、行程、`selftest: OK`、exit code 33 |
| `test-qemu-smp` | ✓ | 同一個 kernel、`-smp 4`：每顆 CPU 的 tick、跨 CPU 喚醒與 kill |
| `test-qemu-min` | ✓ | 最低硬體：`-cpu Conroe,-nx -m 128M`（P12 起；之前 256M）、單核，加 AHCI 測試磁碟跑 fstest |
| `test-textmode` | ✓ | `boot.cfg` = `textmode`、`nousb`、`verbose`、`waitkey`：init 直接開 shell |
| `test-disk-image` | ✓ | `make_disk_image.py` 的映像當唯一磁碟：桌面、`mkdir`、`shutdown`、host fsck、第二次開機讀回 |
| `test-net` | ✓ | host 的 HTTP 伺服器；e1000、e1000e、virtio-net 各一次：`ifconfig`、`ping`、DNS、`fetch`、Linux 的 socket |
| `test-tls` | ✓ | P12：host 的 HTTPS（TLS 1.3）伺服器、測試根憑證；`fetch https://` 拿到檔案，名字不對／過期／自簽／不信任的根都被拒（`docs/tls.md` §5） |
| `test-pkg` | ✓ | P12：簽過章的 index 與兩個測試套件經 HTTPS；`pkg update/search/install/list/remove`、shell 跑裝好的程式（`docs/pkg.md` §5） |
| `test-setup` | ✓ | P12：`boot.cfg` = `setup`、一顆從沒格式化的磁碟；Setup.app 走完每一頁（繁體中文、格式化成 Data），之後 Mouseion 啟動；host 檢查（`docs/setup.md` §5） |
| `test-sound` | ✓ | intel-hda 錄成 wav：`tone` 1 秒約 440 Hz |
| `test-window` | ✓ | 桌面、terminal、`run echo_win`；截圖檢查選單列、標題列與文字；P12：同樣的桌面在 `-m 128M` 再開一次 |
| `test-storage` | ✓ | AHCI（GPT：FAT ESP + eHFS）與 NVMe（4Kn）兩次開機的持久性、fstest、host fsck |
| `test-shell` | ✓ | shell 指令一個接一個（`-smp 2`）：檔案、程式、歷史與游標、管線、`cp -r`、`find`、`date`、萬用字元、Tab 補全、背景工作 |
| `test-pipe` | ✓ | 管線、`>`／`>>`／`<`、Linux 程式進管線、背景工作的 `kill` 與 `fg` |
| `test-shutdown` | ✓ | `shutdown`（QEMU 自己結束、fsck 乾淨）與 `reboot` |
| `test-linux` | ✓ | Linux 人格：`hello`、`files`、`memory`、`time`、`nosys`、`tty`、`poll`、`pipe`；P12：`threads`、`procs`、`misc`、Rust std 的 `hello-std`、`signals`（含 Ctrl-C）（`docs/linux.md` §13–§14） |
| `test-usb` | ✓ | xHCI：USB 鍵盤、滑鼠、hub、熱插拔、USB 隨身碟（FAT 讀寫、拔掉時卸載） |
| `test-panic` | ✓ | 故意 panic：截圖檢查裂開的石碑與最後幾行 log |
| `test-mouseion` | ✓ | 雙擊卷宗、文字檔（Kalamos）、Initrd、Terminal.app（P12 起是第二列第一個：Setup.app 排在前面）；截圖檢查 |
| `test-editor` | ✓ | Kalamos：打中英文、Alt-S 存檔、重開讀回 |
| `test-browser` | ✓ | Periplus：host 的頁面、連結、重新導向、chunked |
| `test-classic` | 本機 | 使用者的 Mac ROM 與 System（`~/mac/`，只讀，複製進 `build/`）：開到 Finder、截圖、電源與第二顆磁碟 |
| `test-ehfs-compat` | 本機 | 與 68k_HAL 的 eHFS 工具交叉檢查 |
| `bench` | 本機 | 量測（記憶體、syscall、執行緒切換、eHFS、TCP），`build/bench.md`（`docs/bench.md`） |
| `test-stress` | 本機 | §4：壓力測試與資源洩漏檢查（`-smp 4`） |
| `test-ports` | 本機 | P12：`~/stela-ports` 的每個 port 建置、打包、在 QEMU 跑它的 `test.sh`，最後 `index`（`docs/ports.md`） |

## 3. fuzz（`make test-fuzz`，P11 K45，使用者已於 2026-10-22 審核）

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

## 4. 壓力測試（`make test-stress`，本機，P11 K46，使用者已於 2026-10-22 審核）

一般 kernel、`-smp 4`、AHCI 測試磁碟、user 網路（host 的 `tools/with_http.py` serve `build/www/stress.bin`，256 KiB 亂數）；桌面的 shell 打 `stress http://10.0.2.2:PORT/stress.bin`。`user/stress` 每一輪：

```text
 procs    100 個行程（Initrd:/bin/true）一起啟動，再一個一個 wait，exit code 都是 0
 files    Data:/stress 建 1000 個檔（各 100 位元組）、列出檢查、全部刪掉、刪資料夾
 pipe     64 MiB 經過一條 pipe：寫的執行緒（64 KiB 一塊、計數的花樣），這個執行緒讀並逐位元組檢查
 tcp      50 個執行緒，各一條 TCP 連線同時下載同一個檔，長度都一樣
 windows  20 個視窗，每個填色、畫方塊、damage，20 次；然後關掉
```

- 跑三輪：前兩輪暖機（kernel 的區塊快取、兩邊 heap 的空閒清單、handle 表、ARP 表），第三輪結束後與第二輪結束時比較——空閒頁框（容許 64 頁）、kernel heap 使用量（容許 64 KiB）、handle 表（一次複製 600 個 handle 拿到的最高槽位不能變高：表只有在沒有空槽時才長，600 比任何一段同時拿著的都多）、執行中的行程數（不能多：沒有殭屍）。每次量之前等 2.5 秒（關掉的 TCP 連線要過 TIME_WAIT 的 2 秒）。超過就印 `stress: LEAK` 並 exit 1。
- 每個部分印 `stress: round N part OK (ms)`，第二輪起加上這一段的頁框與 kernel heap 變化；最後 `stress: OK`。
- 不進 `make test`／CI：要 5–7 分鐘（TCG 下 1000 個檔的建刪要 35–70 秒：每次都是一個 eHFS 交易、讀寫 16 KiB 的節點並算 CRC），而且 50 條連線在慢的機器上可能逾時。

### 4.1 找到的問題（P11 K46）

第一次跑：第二輪比第一輪多了 256 頁框、80 KB kernel heap、一個 handle 槽。改成三輪、每段分開量之後：

| 段 | 每輪（修之前） | 原因與修法 | 修之後 |
|---|---|---|---|
| 頁框 | +256（只在第一次的兩輪比較） | stress 自己的 heap 長了一塊（1 MiB），不是洩漏；改成比第二、三輪 | +0 |
| `procs` | +59 KB／100 個行程 | 行程表的紀錄用 `Weak<Process>`：`Weak` 讓整個 `Process` 的配置在最後一個 `Arc` 之後還留著。`process::finish` 時把它換成空的 `Weak` | +13.7 KB（每個約 137 bytes：名字與 exit code，已知問題「Exited 紀錄永遠不清」，process handle 只記 id，要回收得先改 handle） |
| `windows` | +11 KB／連線、handle 槽 +1 | toolbox 的 `Connection` 沒有 `Drop`：伺服器只在程式結束時才清 client，同一個程式反覆連線就每次留下一個（port、佇列、紀錄）。新的請求 `DISCONNECT`（0x10B，`docs/window.md`），`Connection` 被 drop 時送 | +16 bytes、handle 不變 |
| `files`、`pipe`、`tcp` | 幾十 bytes 以內 | — | — |

## 5. 平行測試（`make test -jN`，P13 K23，使用者已於 2026-10-22 審核）

**比喻**：原本一間考場一次只考一科；現在同時開好幾間考場。前提是考生不能共用同一份考卷、同一支電話：每個 QEMU 測試要有自己的東西。

`make test` 先跑 `test-host` 與 `test-fuzz`，再用一個子 make 跑 `test-qemu-all`（= `QEMU_TESTS` 的每一個）。沒有 `-j` 時順序和以前一樣；`make -j6 test` 時 QEMU 測試並排跑（子 make 繼承 jobserver）。為了能並排，P13 改了這些共用的東西：

| 共用的 | 問題 | 改成 |
|---|---|---|
| `build/OVMF_VARS.fd`（pflash，可寫） | QEMU 鎖住檔案，第二個 QEMU 打不開 | `snapshot=on`：寫入進暫存檔，同一個檔案可以同時給很多個 QEMU（測試不需要留下 UEFI 變數） |
| HTTP／HTTPS 的 port | test-net 與 test-browser 都用 8642；test-tls 與 test-pkg 都用 8643–8646 | 各自的 port：`NET_PORT` 8642、`TLS_PORT` 8643（+1..+3 由它算出來）、`BROWSER_PORT` 8647、`USERNET_PORT` 8650、`PKG_PORT` 8653；都能在命令列改（`make test NET_PORT=9642 …`，兩個 worktree 同時測時用） |
| `build/www` | 幾個測試同時寫 `hello.txt` 等檔案 | `build/www-user`、`www-tls`、`www-pkg`、`www-browser`；test-pkg 的憑證在 `build/tls-pkg` |
| `build/images/ahci.img` | test-storage 直接寫它，其他測試同時在複製它 | test-storage 用自己的 `ahci-storage.img`、`nvme-storage.img`；`ahci.img` 保持剛做好的樣子 |
| `build/esp-scale1` | test-window 與 test-mouseion 都 `rm -rf` 再複製 | test-mouseion 用 `build/esp-scale1-m` |

已經各自分開的：每個測試的 log、截圖（monitor socket 跟著截圖的名字）、QMP socket（`build/qmp-PID.sock`）、其他 ESP 副本與磁碟複本。ESP 本身（`fat:rw:build/esp`）可以同時給很多個 QEMU 讀（STELA 不寫 Boot）。P14 K4：System 映像（`build/system.img`）用 `snapshot=on` 接（`SYSTEM_DRIVE`），寫入只進暫存檔，大家共用同一份。

**CI**（`.github/workflows/ci.yml`）：`host` job 跑 fmt、clippy、`make test-host test-fuzz`；`qemu` job 是三個 shard 的 matrix，各跑 `make -j2 test-qemu-shard N=… SHARDS=3`（測試名稱的 `cksum` 模 3；分組不平均：9／5／10 個）。

**時間**（本機 12 核、TCG，`make clean` 之後，含建置）：`make clean && make -j6 test` 249 秒（約 4 分鐘，24 個 QEMU 目標、37 次開機全過；那時另一個 worktree 同時在跑序列的 `make test`，所以單獨跑會更快）；序列的 `make clean && make test` 約 15 分鐘（這次的 worktree 執行）。計畫的目標「總 wall time ≤ 6 分鐘」在本機達成；CI（4 核的 runner、每個 shard `-j2`）沒有量到。

計畫外的決定：

- port 用**每個目標固定、不同的號碼**，不是計畫寫的 `--port 0` 自動選 + `{HTTP_PORT}` 替換：測試打的網址與預期的輸出都直接用 make 變數，比讓 `qemu_test.py` 讀 port 檔、替換字串簡單；代價是 host 上那些 port 被別的程式佔住時會失敗（可以在命令列換）。
- 沒有把每個目標的東西都搬進 `build/run/<目標>/`：只把真的共用的（上表）分開，其他本來就各自的名字不動（改動小，log 與截圖的位置不變）。
- `test` 改成先跑 host 的測試、再用子 make 跑 QEMU 的（計畫「test-host 先」）。
- shard 用名稱的 cksum（計畫「依名稱 hash 分組」），沒有依時間平衡。

已知限制：CI 的設定沒有在 GitHub 上跑過（這次不能 push）；`-j` 太大時 TCG 變慢，逾時（多數 90–180 秒）可能不夠；本機專用的目標（test-classic、test-ports、test-stress、bench）沒有改（它們不在 `make test` 裡，彼此也不並排跑）。

