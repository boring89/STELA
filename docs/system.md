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

## 7. 測試（K4 的部分）

| 測試 | 內容 |
|---|---|
| host `ehfs` | `ehfs::label` 不掛載讀出卷宗名、不是 eHFS → `None` |
| 每個一般 kernel 的 QEMU 測試 | `QEMU_FLAGS` 多一顆 `build/system.img`（`SYSTEM_DRIVE`：`ide.2`、`snapshot=on`，所以並排的測試共用、從不寫入）；selftest kernel 的測試（`QEMU_TEST_FLAGS`）不接 |
| `make test-mouseion` | 5 個卷宗；Initrd 視窗裡刪 `Setup.app` 被拒、雙擊 `Terminal.app`；第四次開機雙擊桌面的 System → `Programs` → `Echo.app has its own icon` |
| `make test-sandbox` | 容器裡的程式 `VolumeOpen("System")` 可以 |
| `make test-setup` | 第一次開機沒有 System：`mouseion: no System volume` |
| `make test-shell`、`test-linux`、`test-browser` 等 | 工具、`linux/dyn`（`System:/linux/lib`）、`System:/doc/guide.html` 都從 System 來 |

## 9. 已知限制（K4）

- System 映像每次 `make` 都重做（`mkfs` 的 UUID 與時間是新的，內容相同時位元組也不同）。
- 整顆磁碟形式的 System 只看卷宗名——任何叫 System 的 eHFS 都會被唯讀掛上（K5 加簽章檢查）。
