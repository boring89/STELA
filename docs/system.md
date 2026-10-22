# System 卷宗與更新

> P14（B，K4–K8）由自動排程一次做完，**使用者尚未審核**。

STELA 的系統程式分兩處：開機一定要有的放在 **initrd**（和 kernel 一起從 ESP 載入），其他放在一個唯讀的 eHFS 卷宗 **`System`**。整個 System 卷宗是一個映像檔；更新就是換掉整個映像（之後的 A/B 兩槽，§4 起）。程式碼：`tools/make_system_image.py`、Makefile 的 `system-image`、`kernel/src/vfs/mount.rs`（`mount_system`）、`libstela::fs::read_system`／`open_system`、shell 的 `SEARCH_PATH`、Mouseion 的 `system_program`。

## 1. 比喻：書店的櫥窗與倉庫

initrd 是**櫥窗**：店一開門就要擺好的東西（收銀機、門牌、店員）——init、shell、終端機、視窗伺服器、Mouseion、Setup、輸入法、憑證。System 是**倉庫**：架上大部分的書（應用程式、命令列工具、中文字型、字典、手冊、Linux 的函式庫）。倉庫上鎖（唯讀），進貨就是整箱換掉。倉庫打不開時店還是能開（只有櫥窗裡的東西），只是書少了。

## 2. 內容（`make system-image` → `build/system.img`）

```
System（eHFS，卷宗名 System，預設 24 MiB：SYSTEM_IMG_MIB=）
  Programs/   Echo.app Viewer.app Classic.app Kalamos.app Monitor.app Periplus.app
              ifconfig ping fetch tone bench
  bin/        echo cat grep wc head tail sort uniq find du df date sleep touch stat hexdump tar true false
  fonts/      fusion-pixel-12-zh_tw.bin（中文字型，約 1 MB）
  chewing/    libchewing 的字典（輸入法）
  doc/        guide.html、guide.en.html（使用手冊）
  linux/lib/  ld-musl-x86_64.so.1（musl 的 libc.so）、libdyntest.so
  VERSION     版本（一行；`make system-image VERSION=0.2.1` 可改）
  （最後 4 KiB 留給映像的尾端資料，§3）
```

initrd 剩下（P13 是 13.9 MB，P14 K4 起約 4.9 MB）：`init`、`shell`、`windowserver`、`mouseion`、`ime`、`powerbox`、`e1000u`、`hwinfo`、`Terminal.app`、`Setup.app`、`bin/pkg`、`certs/`、selftest 與測試用的程式（`hello`、`spin`、`fault`、`ipc_server`、`ipc_client`、`keyecho`、`fstest`、`proctest`、`stress`）與 `linux/` 的測試程式（selftest 要跑 `linux/hello`、`threads`、`procs`；**`linux/lib/` 不在 initrd**）。

- `tools/make_system_image.py`：`mkfs.ehfs -L System`（`--sectors` 少最後 4 KiB）、`ehfs-cp mkdir`／`in` 逐一放進去；只接受 `build/` 底下的一般檔案。
- **計畫外的決定**：計畫寫「`Programs/`（所有 .app 與 bin 工具）」、「`certs/`」——命令列工具放在另一個資料夾 `System:/bin/`（和 `Initrd:/bin/` 對稱，Mouseion 的 `Programs` 視窗不會塞滿 20 個工具）；憑證只留在 initrd（TLS 一定要有，且小），System 不再放一份。`hwinfo` 依計畫留在 initrd（根目錄，P11 起就在那裡，不是 `bin/`）。

### 2.1 掛載

```
mount_all：每顆磁碟的每個分割區（或整顆）：
   區塊 0 是 eHFS 且卷宗名是 "System"（ehfs::label，不用掛載就讀得到）→ 先記下來
   其他照舊（第一個可寫的 eHFS 是 Data）
 然後 Initrd、Tmp，最後第一個 System：唯讀掛載，名稱 System（其他的印 another System volume; not mounted）
vfs: mounted Boot (FAT, ro, 39 MiB), Data (eHFS, rw, 86 MiB), Initrd (initrd, ro, 4790 KiB), Tmp (memory, rw, 16 MiB), System (eHFS, ro, 23 MiB)
```

- System 最後掛，所以卷宗清單（與 Mouseion 桌面的直條）的順序是 Boot、Data、Initrd、Tmp、System——以前的位置都不變。
- 唯讀：kernel 不寫它（`ehfs::Volume::mount(_, true)`，日誌也不重做到磁碟上），所以沒有正常卸載的問題；寫入一律 `ReadOnly`。
- K5 起由 GPT 的分割區類型選槽（§4）；整顆磁碟、卷宗名是 System 的映像（QEMU 測試這樣接）照樣可以。

### 2.2 誰去 System 找東西

| 東西 | 先找 | 再找 |
|---|---|---|
| shell 的程式（`run x`、直接打名字、Tab 補全） | 工作資料夾 → `Initrd:/` → `Initrd:/bin/` | `System:/Programs/` → `System:/bin/` → `Data:/Programs/NAME/bin/`（P12） |
| Mouseion 的 Kalamos／Viewer／Monitor（`system_program`） | `System:/Programs/NAME` | `Initrd:/NAME` |
| 中文字型（`toolbox::cjk`）、輸入法的字典 | `System:/fonts/`、`System:/chewing/` | `Initrd:/…`（`libstela::fs::read_system`） |
| Linux 的 `/lib`、`/usr/lib`（`linux::path::LIB_FOLDERS`） | `Data:/Programs/lib` | `System:/linux/lib` → `Initrd:/linux/lib` |
| Periplus 首頁的手冊連結 | `System:/doc/guide.html` | — |

- **沙箱**：`VolumeOpen("System")` 和 `Initrd` 一樣不需要 `VOLUMES`（唯讀的系統檔案，`docs/sandbox.md`）；Linux 程式可以把 `System:/linux/lib` 的檔案映射成可執行。
- **信任**：從 System 讀進來的程式和 Initrd 的一樣算「系統的」（`program::is_system_volume`：延伸屬性的命名空間 `docs/vfs.md` §4.5；Mouseion 的 `TRUSTED`：Classic.app 在 System 也照舊有全部卷宗）。

### 2.3 沒有 System 時

開得到桌面：init、視窗伺服器、Mouseion、Terminal、Setup、shell、輸入法都在 initrd。Mouseion 印 `mouseion: no System volume` 並開一個說明的資訊視窗（在 Terminal 之前開，鍵盤仍在 Terminal）；沒有應用程式、命令列工具（shell 的內建指令還在）、中文字型（中文字畫成空白）、輸入法的字典（IME 印找不到字典就結束，視窗伺服器重啟一次後放棄）。`make test-textmode`、`make test-setup` 的第一次開機、`make test-disk-image` 都是沒有 System 的開機。

## 3. 映像的尾端：版本、SHA-256、簽章（P14 K5）

```
 System 映像： [ eHFS 卷宗（N 個 4 KiB 區塊）][ 尾端：1 個區塊 ]
 尾端：  0   "STELASYS"
         8   格式（u32 LE，1）
         16  資料長度（u64 LE）= N × 4096
         24  前面所有位元組的 SHA-256
         56  版本長度（u8），57… 版本（UTF-8，≤ 64）
         128 bytes 0..128 的 ed25519 簽章
```

- **找得到尾端**：映像可能比槽小（24 MiB 的映像放在 64 MiB 的槽），所以尾端不是「槽的最後一塊」，而是 eHFS 卷宗之後的那一塊——kernel 從區塊 0 的 superblock 讀卷宗的區塊數 N（`ehfs::block_count`），尾端在第 N 塊。`make_system_image.py` 讓 mkfs 少用最後 4 KiB，`tools/sysimage.py sign` 寫尾端。
- **簽章**：ed25519（RFC 8032），簽的是尾端的前 128 bytes（含 SHA-256 與版本），所以「簽章對 + SHA-256 對」= 內容是簽的人做的。金鑰是 pkg 的**測試鑰匙**（`tools/pkg.pub`，私鑰由固定種子產生，任何人都能簽）；kernel 把 `tools/pkg.pub` 編進去（`system.rs` 的 `KEY_HEX`）。**正式發布前要換**（和 pkg 一樣，`docs/pkg.md` §6）。
- **什麼時候算 SHA-256**：每次開機都驗尾端的簽章（一次 ed25519，很快）；整個映像的 SHA-256（24 MiB，TCG 下約 1–2 秒）只在「slot 檔說這是更新後還沒成功開機過（`new`）」、「上次開機沒有到桌面（嘗試次數 > 0）」、`boot.cfg` 的 `verify`、或換到另一槽時才算（計畫的決定）。
- 純邏輯在新的 workspace crate **`sysimage`**（`no_std`，host 測試；kernel 與之後的 `stela-update` 共用）。它用的 `ed25519-dalek` 2 與 `sha2` 0.10 是 P12 的 pkgcore 已經在用的同一版本，**不是新的第三方 crate**（kernel 第一次直接依賴它們）。

## 4. 兩個槽與 slot 檔（P14 K5）

```
 磁碟（make disk-image）：ESP │ System A │ System B │ Data
                               STELA System 類型（docs/storage.md §9）
 ESP:/STELA/slot（一行）："a 0"      用 A，0 次失敗的嘗試
                           "b 1 new"  用 B，1 次還沒到桌面的開機，更新後還沒成功過
```

開機時（`kernel/src/system.rs` 的 `choose`，邏輯在 `sysimage::plan`／`after_start`）：

```
 boot.cfg system=a|b？ ── 有 ──► 先試那一槽（slot 檔不動）
        │ 沒有（auto）
 讀 slot 檔（沒有就當 "a 0"）
 嘗試次數 ≥ 2？ ── 是 ──► 換另一槽（印 slot X did not reach the desktop 2 times）
 先試的槽：簽章（必要時 + SHA-256）── 不對 ──► 另一槽（一定算 SHA-256），印 not used (原因)
 掛上的槽寫回 slot 檔：同一槽 → 嘗試 +1（保留 new）；換了槽 → "X 1"
 Mouseion 起來後呼叫 BootOk（syscall 77，要 POWER）→ "X 0"（拿掉 new），印 system: slot X confirmed
```

- 所以「開機但沒到桌面」兩次（例如更新後的 System 讓視窗伺服器當掉）就自動退回另一槽。計畫寫「loader 或 kernel 記嘗試中」——loader 不動 slot 檔，全部在 kernel（計畫的決定）。
- **寫 ESP**：`Boot` 卷宗對程式唯讀，kernel 自己經 `FatFs::kernel_write` 寫 `STELA/slot`（建立或覆寫整個檔，快取是 write-through，回來時已在磁碟上）。QEMU 測試的 `fat:rw:` ESP 不掛載（不是 GPT），那時沒有 slot 檔，只看卷宗名。
- **沒有槽時**：整顆磁碟（或任何分割區）上卷宗名是 System 的 eHFS 照 K4 的方式掛，但也要簽章對（`system: ahci0 (version 0.1.0)`）。
- **`SystemStatus`**（syscall 78，誰都可以）：`mounted=a slot=a 0 a=0.1.0 b=-`（掛上的槽、slot 檔、兩槽裡簽章對的映像的版本；`-` = 沒有）。

## 6. 更新：`stela-update`（P14 K6）

```
 stela-update status                  running slot a; slot a: 0.1.0, slot b: -
 stela-update check [--file IMAGE]    映像（或套件伺服器 index 的 system 項）的版本，驗過簽章與 SHA-256
 stela-update apply [--file IMAGE]    寫進沒有掛上的那一槽，下次開機用它
 stela-update rollback                下次開機用另一槽（有尚未生效的切換就取消它）
```

```
 Mouseion：Special > Software Update…（ask_text：檔案；空白 = 套件伺服器；rollback）
   └► Initrd:/bin/stela-update apply --file F --window（帶 POWER）
        讀整個映像 → 尾端的簽章（Initrd:/certs/pkg.pub）+ SHA-256，不對就一個 byte 都不寫
        PartitionWrite(offset, 1 MiB) × N（syscall 79，要 POWER；只寫沒掛上的槽）
            offset 0 那次：slot 檔若指著正要寫的槽，先指回掛著的槽（寫到一半斷電也不會開到半個映像）
        SystemUse(0)（syscall 80）：kernel 把那一槽整個再驗一次（簽章 + SHA-256）→ slot 檔 "b 0 new"
        視窗：「Restart now? Enter: restart, Esc: later」→ reboot
```

- **只能從 Mouseion 用**：`apply`、`rollback` 要 `POWER`，shell 沒有（`status`、`check` 誰都可以）。計畫寫「`stela-update`（要 POWER）」與「Mouseion 的 Software Update… 呼叫它」——指令行版照樣在 `Initrd:/bin/`，但從 shell 執行會說 `no POWER capability`。
- **從套件伺服器**：`index.json` 可以多一項 `"system": {"version", "url", "sha256", "size"}`（`pkgcore::parse_system`，host 測試）；`stela-update apply`（不給 `--file`）先做 `pkg update`（驗 index 的簽章）、下載 `url`、比對 index 的 SHA-256、再照上面驗映像。整個映像放在記憶體裡（16 MiB）。**沒有自動測試**（`make test-update` 只用檔案）。
- **rollback**：slot 檔指著掛著的槽 → 換另一槽（kernel 整個驗過才換，另一槽壞了就拒絕：`the other slot has no good System image`）；slot 檔已經指著另一槽（剛 apply 還沒重開）→ 指回掛著的槽（取消）。
- 計畫外的決定：多了 `SystemUse`（syscall 80）——寫完後由 kernel 再驗一次並改 slot 檔，程式不能直接寫 slot 檔（ESP 對程式唯讀）；`PartitionWrite` 的參數是 (offset, buf, len, power)，沒有名字（只能寫「另一槽」，不能指定分割區）。

### 6.1 安裝到磁碟（P14 K7）

Setup 的 install 頁把 ESP、System A（掛著的槽的映像）、空的 System B、空的 Data 寫到一整顆空白的磁碟（`vfs::mount::install`，`docs/setup.md` §4.2）。所以「安裝」也只是複製簽好的 System 映像，和更新是同一套格式。

## 7. 測試

| 測試 | 內容 |
|---|---|
| host `ehfs` | `ehfs::label`、`ehfs::block_count` 不掛載讀出卷宗名與區塊數、不是 eHFS → `None` |
| host `sysimage`（K5） | 尾端的編碼往返、測試鑰匙的簽章（改一個簽過的 byte 就不對、改沒簽的部分沒關係、別的鑰匙不對）、壞的尾端、分段算 SHA-256；slot 檔的文字往返與壞的；`plan`：沒有檔 → A、`new` 或嘗試 > 0 → 整個算、`verify`、兩次失敗 → 另一槽、boot.cfg 指定；更新後的開機、兩次失敗、退回 A、`BootOk`、指定時不寫 |
| host `pkgcore`（K6） | index 的 `system` 項 |
| `make test-update`（K6–K7） | `make_disk_image.py`：System A = 這次的映像、B 空的；Data 放「下一版」（`VERSION` 最後一個數字 +1）、簽章壞掉的一份、這次的映像一份。四次開機，都經 Mouseion 的鍵盤選單（Alt-M → Special → Software Update…）：① apply 下一版 → `wrote 100%`、`the next start uses slot b`、Enter 重開；host 讀 slot 檔 = `b 0 new`。② `slot b (…, checked in full)`、`confirmed`；壞簽章 → `the signature is not STELA's`；rollback → `the next start uses slot a`（Esc：稍後）；再 apply 下一版到 A，`wrote 25%` 時殺掉 QEMU；host：slot 檔 = `b 0`。③ 仍是 `slot b`；rollback → 拒絕（A 只寫了一部分）；apply 這次的映像 → A、重開。④ `slot a (…, checked in full)`、`confirmed`；slot 檔 = `a 0` |
| host `block`（K5、K7） | STELA System／Data 的 GUID 字串；`gpt::write` 寫 512 與 4096 扇區的表再讀回、超出範圍的分割區被拒 |
| `make test-disk-image`（K5） | 映像有 ESP + System A（`build/system.img`）+ 空的 System B + Data：`system: slot a (version …)`、`System (eHFS, ro, …)`、桌面起來後 `system: slot a confirmed`；關機後 host 讀 ESP 的 `STELA/slot` 是 `a 0`（mtools） |
| 每個一般 kernel 的 QEMU 測試 | `QEMU_FLAGS` 多一顆 `build/system.img`（`SYSTEM_DRIVE`：`ide.2`、`snapshot=on`，所以並排的測試共用、從不寫入）；selftest kernel 的測試（`QEMU_TEST_FLAGS`）不接 |
| `make test-mouseion` | 5 個卷宗；Initrd 視窗裡刪 `Setup.app` 被拒、雙擊 `Terminal.app`；第四次開機雙擊桌面的 System → `Programs` → `Echo.app has its own icon` |
| `make test-sandbox` | 容器裡的程式 `VolumeOpen("System")` 可以 |
| `make test-setup` | 第一次開機沒有 System：`mouseion: no System volume`；K7：安裝到第二顆空白磁碟、從它開機（`docs/setup.md` §5） |
| `make test-shell`、`test-linux`、`test-browser` 等 | 工具、`linux/dyn`（`System:/linux/lib`）、`System:/doc/guide.html` 都從 System 來 |

## 8. 已知限制

- System 映像每次 `make` 都重做（`mkfs` 的 UUID 與時間是新的，內容相同時位元組也不同）。
- 整顆磁碟形式的 System 看卷宗名與簽章（K5）。
- 簽章用的是**測試鑰匙**（`tools/pkg.pub`）：任何人都能做出 kernel 接受的 System 映像，正式發布前一定要換（§3）。
- slot 檔放在 ESP（FAT），kernel 寫它時沒有日誌：寫到一半斷電可能留下壞的 slot 檔——壞的或讀不到就當 `a 0`。
- 槽只有兩個、大小固定（`make disk-image` 的 `SYSTEM_MIB`，預設 64 MiB；映像 16 MiB，`SYSTEM_IMG_MIB`）；映像比槽大就裝不進去（`make_disk_image.py` 拒絕、`PartitionWrite` 回 `InvalidArgument`）。
- 更新是整個映像（16 MiB）重寫，沒有差分；從套件伺服器下載時整個放在記憶體裡。
- 「開到桌面」= Mouseion 呼叫 `BootOk`；文字模式（`textmode`）或沒有 Mouseion 的開機永遠不會確認，兩次後就換槽——要用文字模式時請在 `boot.cfg` 寫 `system=a|b`（指定時不計次）。
- 只有 Mouseion（有 `POWER`）能更新；沒有「自動檢查更新」。
- 實機未驗證（OVMF 從新磁碟開機、slot 檔寫在 ESP 上都只在 QEMU）。
