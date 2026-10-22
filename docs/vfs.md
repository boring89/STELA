# VFS：卷宗、資料夾與檔案

> P5 由自動排程一次做完，使用者已於 2026-10-20 審核。

使用者程式怎麼碰到檔案：卷宗各自為根（`Data:/docs/a.txt`）、所有操作都相對於一個**資料夾 handle**、每個項目有**持久 ID**。架構決定見 `docs/architecture.md` §4 的 E（磁碟區各自為根、持久 ID、大小寫保留但不敏感）、D（handle + 權限）、I（能力）。

程式碼：

```
abi/src/lib.rs               編號 16–26、open 旗標、DirEntry／Stat／VolumeInfo、錯誤 14–21、bootstrap::CWD
kernel/src/vfs/mod.rs        模組說明
kernel/src/vfs/path.rs       相對路徑的切分（純邏輯，host 測試）
kernel/src/vfs/target.rs     FileSystem trait、Volume、Node、掛載表、EhfsFs、FatFs
kernel/src/vfs/mount.rs      開機時的掛載
kernel/src/fsyscall.rs       檔案 syscall
kernel/src/object/mod.rs     KernelObject::Directory／File
user/libstela/src/fs.rs      使用者端包裝
user/fstest/                 測試程式
```

## 1. 比喻：每個抽屜各有一把鑰匙

Classic Mac 的桌面上，每個磁碟區是獨立的一顆圖示（沒有 Unix 那種把所有磁碟掛在 `/` 底下的單一樹）。STELA 也一樣：`Data`、`Boot`、`Archive` 各自是根。

程式拿到的不是「路徑字串」，而是**某個資料夾的鑰匙**（handle）。開檔案永遠是「用這把鑰匙開它底下的某某」——沒有鑰匙就到不了。程式啟動時拿到一把「工作目錄」的鑰匙（handle `[3]`，預設是 `Data` 的根）；要別的地方，得有人把鑰匙交給它（或用 `VolumeOpen` 依名字開某個卷宗的根）。

## 2. 模型

```
 VOLUMES ── Arc<Volume> { index 0, "Boot",    FatFs  }   FAT，唯讀
         ├─ Arc<Volume> { index 1, "Data",    EhfsFs }   eHFS，讀寫
         └─ Arc<Volume> { index 2, "Archive", EhfsFs }
 handle ──► KernelObject::Directory(Node) / File(Node)
            Node { volume: Arc<Volume>, id: 持久 ID }
```

- `FileSystem` trait（kernel 的後端介面）：`root`、`lookup(dir, name)`、`get(id)`、`list(dir)`、`read`、`write`、`create`、`remove`、`rename`、`set_size`、`read_only`、`space`、`kind`、`unmount`。
- **Node 以持久 ID 指向項目**：改名、搬移後 handle 仍然有效；項目被刪除後，用它的操作回 `NotFound`。
- eHFS 的 ID 是 CNID（真的持久，跨開機不變）；FAT 沒有 inode，`FatFs` 在看到項目時配發 ID（這次開機內穩定）。
- 名稱：大小寫保留、比較不敏感（eHFS：NFC + case folding；FAT：ASCII）。

## 3. 路徑

- `DirOpen` 的路徑是**相對於資料夾 handle** 的：以 `/` 分隔，空的與 `.` 跳過，**`..` 拒絕**（`InvalidArgument`）——kernel 不往上走；`..` 由程式自己以字面方式處理（P6 的 libstela 路徑解析）。
- 完整的寫法 `Volume:/a/b` 是使用者介面的慣例（shell），由 libstela 拆成「`VolumeOpen("Volume")` + 相對路徑」。
- 名稱上限 255 bytes；路徑上限 4096 bytes。

## 4. syscall（接 P4 的 16 起）

| 編號 | 名稱 | 參數 | 回傳 | 權限 |
|---|---|---|---|---|
| 16 | `DirOpen` | dir, path_ptr, path_len, flags | handle | dir 要 READ；建立要 WRITE |
| 17 | `DirRead` | dir, cookie, &mut DirEntry | 下一個 cookie（≠ 0）、0 = 到底 | READ |
| 18 | `DirRemove` | dir, name_ptr, name_len | 0 | WRITE |
| 19 | `DirRename` | dir, old_ptr, old_len, new_dir, new_ptr, new_len | 0 | 兩個都 WRITE；同一卷宗 |
| 20 | `FileRead` | file, buf, len, offset | 讀到的 bytes | READ |
| 21 | `FileWrite` | file, buf, len, offset | 寫入的 bytes | WRITE |
| 22 | `FileStat` | file 或 dir, &mut Stat | 0 | — |
| 23 | `FileSetSize` | file, size | 0 | WRITE |
| 24 | `VolumeList` | index, &mut VolumeInfo | 1／0 | — |
| 25 | `VolumeOpen` | name_ptr, name_len | 根資料夾 handle | — |
| 26 | `VolumeOpenById` | dir, id | handle | dir 要 READ；新 handle 的權限 ⊆ dir 的 |

`open` 旗標：`READ` 1、`WRITE` 2、`CREATE` 4、`TRUNCATE` 8、`DIRECTORY` 16、`EXCLUSIVE` 32。

- 新 handle 的權限 = 要求的（沒有指定 READ/WRITE 時給 READ）∩ 父資料夾 handle 的；**唯讀卷宗上要 WRITE → `ReadOnly`**。另外一律有 `DUPLICATE|TRANSFER`（可以交給別的程式）。
- 存在的是資料夾就給 Directory handle（不論有沒有 `DIRECTORY` 旗標）；有 `DIRECTORY` 旗標但是檔案 → `NotDir`。
- `CREATE|EXCLUSIVE` 遇到已存在 → `Exists`；`TRUNCATE` 要 WRITE。
- `DirRead` 以 **cookie** 列舉（P11，使用者已於 2026-10-22 審核）：0 = 從頭，每次回傳下一個的 cookie，0 = 到底；cookie 對程式是不透明的。eHFS：cookie = 上一個項目的 CNID，kernel 記著它的名字（`EhfsFs::cursors`，最多 256 個），下一次從 B-tree 的下界（parent, 名字）往後走一個（`Volume::list_after`）——整個資料夾列完是 O(n)（以前每次重列整個資料夾，O(n²)）；上一個項目被刪掉了也能接下去。其他後端（FAT、Initrd、Tmp）用預設實作：cookie = 位置 + 1，每次仍重列（它們的資料夾小）。libstela：`fs::read_dir(dir, cookie) -> Option<(DirEntry, next)>`、`fs::entries(dir)`。
- 名稱與資料一律經 `AddressSpace::read/write` 複製（`FileRead`／`FileWrite` 每次 64 KiB）。

錯誤（接 P4 的 13）：14 `NotFound`、15 `Exists`、16 `NotDir`、17 `IsDir`、18 `NotEmpty`、19 `ReadOnly`、20 `NoSpace`、21 `IoError`（裝置錯誤或卷宗損壞）。

### 4.1 結構

```
 DirEntry（288 bytes）：id u64、size u64、mtime u64、kind u32（1 檔案、2 資料夾）、name_len u32、name [u8; 256]
 Stat（48 bytes）：id、size、ctime、mtime（u64）、kind、volume、flags（bit 0 唯讀）、reserved（u32）
 VolumeInfo（96 bytes）：index、flags（bit 0 唯讀）、kind（1 eHFS、2 FAT、3 initrd）、name_len（u32）、total、free（u64）、name [u8; 64]
```

時間是 1970 起的奈秒（UTC）；P6.5 起來自 CMOS RTC（`frame::time::wall_clock_ns`，`docs/time.md` §5.5），讀不到 RTC 時是「2026-01-01 + 開機以來」。

### 4.2 Finder 資訊（P9）

| 編號 | 名稱 | 參數 | 權限 |
|---|---|---|---|
| 37 | `FileGetInfo` | 檔案或資料夾, &mut [u8; 32] | — |
| 38 | `FileSetInfo` | 檔案或資料夾, &[u8; 32] | WRITE + `MANAGE`（P11）；唯讀卷宗 → `ReadOnly` |

每個項目 32 bytes 的 Finder 資訊（`abi::FINDER_INFO_SIZE`），kernel 不解讀：eHFS 存在目錄記錄裡（格式本來就有，`docs/ehfs.md`），`Tmp` 存在記憶體；FAT 與 Initrd 回 `NotSupported`。後端：`FileSystem::get_info`／`set_info`（預設 `NotSupported`）。P14 K2：kernel 只看一個位元——`fdFlags`（bytes 8–9，大端序）的 `kNameLocked` 0x1000（`abi::finder`）：`DirRemove`、`DirRename` 與 Linux 的 `unlinkat`、`renameat` 遇到鎖定的項目回 `AccessDenied`（`FileSystem::check_unlocked`）。Mouseion 的配置（視窗矩形、圖示位置）見 `docs/mouseion.md`。

### 4.4 `FilePath`（P11，使用者已於 2026-10-22 審核）

| 編號 | 名稱 | 參數 | 回傳 | 權限 |
|---|---|---|---|---|
| 53 | `FilePath` | 檔案或資料夾, buf, len | 路徑的總長度（寫進 buf 的最多 len bytes） | READ |

回傳卷宗**裡面**的路徑：根是 `/`，其他是 `/a/b`（UTF-8，沒有 NUL）。kernel 從項目往上走：`FileSystem::parent(id)`——eHFS 用目錄記錄裡的父 CNID（`Entry::parent`）；其他後端（FAT、Initrd、Tmp）用預設實作，從根做廣度優先搜尋（最多看 10 萬個資料夾）。`vfs::path_of(volume, id)` 也給 Linux 人格算 cwd（`docs/linux.md` §7）。libstela `fs::path_of(h)`（buf 不夠時放大重試）。注意：有某個資料夾的 handle 就能知道它上面各層的名字（不能打開它們）。

### 4.5 延伸屬性（P11，使用者已於 2026-10-22 審核）

| 編號 | 名稱 | 參數 | 回傳 | 權限 |
|---|---|---|---|---|
| 58 | `FileGetAttr` | 項目, name, name_len, buf, len | 值的總長度（寫進 buf 最多 len）；沒有 → `NotFound` | READ |
| 59 | `FileSetAttr` | 項目, name, name_len, value, len | 0；len 0 = 刪除 | WRITE + `MANAGE`；P14 K3：`stela.*` 見下 |
| 60 | `FileListAttr` | 項目, buf, len | 名稱的總長度（每個名稱後面一個 NUL） | READ |

名稱 1–255 bytes 的 UTF-8（不含 NUL，`abi::ATTR_NAME_MAX`），值最多 3 KiB（`ATTR_VALUE_MAX`，eHFS 的上限）。後端：eHFS 的 attributes B-tree（`attr_get`／`attr_set`／`attr_remove`／`attr_names`，格式本來就有）；`Tmp` 存在每個節點的 `BTreeMap`；FAT、Initrd → `NotSupported`。libstela `fs::get_attr`、`set_attr`（空值刪除）、`list_attrs`。用途：Mouseion 的視窗狀態與圖示位置（`docs/mouseion.md` §3.1）。fstest 第 38 步：沒有 → `NotFound`、設定、3000 bytes 的、讀回、列出、太大 → `InvalidArgument`、刪除。

**命名空間（P14 K3，自動排程一次做完，使用者尚未審核）**：`stela.OWNER` 與 `stela.OWNER.…`（`abi::attr::owner`，host 測試）只有程式 OWNER 能寫（**讀都可以**），只要 handle 有 `WRITE`（不需要 `MANAGE`，所以容器裡的程式也能在自己的檔案上記東西）；其他名稱照舊要 `WRITE` + `MANAGE`。「程式是誰」是 kernel 記的：`ProcessSpawn` 時 bundle 的名稱（`Kalamos.app` → `Kalamos`）或檔名，**不是**參數的第一個字（那是啟動者給的，可以亂取；行程名稱仍照舊取它），比較時不分大小寫（`Process::program`）。`Initrd` 裡有同名程式（檔案或 `NAME.app`）時，只有從 `Initrd` 讀進來的那一份算（Data 上自己放一個叫 `mouseion` 的程式寫不了 `stela.mouseion.*`）。kernel 自己建的行程（init 由 `spawn_path` 從 Initrd 啟動，也記下）之外沒有記錄的一律拒絕。Linux 人格照舊不暴露延伸屬性。fstest 第 40 步：寫 `stela.mouseion.icon` → `AccessDenied`（fstest 有 MANAGE 也不行）、`user.note` 可以；第 38 步改用自己的 `stela.fstest`。**計畫外的決定**：計畫說「行程名稱（bundle 名）」——行程名稱是參數的第一個字、可以偽造，所以另外記程式的名字與來源。

### 4.3 bundle（P9）

`ProcessSpawn` 也接受 `Name.app` 資料夾的 handle：執行裡面的 `exec`（`program::BUNDLE_EXEC`），行程名稱是參數的第一個字（沒有參數時用 bundle 名稱 `Name`）。其他資料夾仍是 `IsDir`。P9 起行程名稱一律取參數的第一個字（≤ 24 bytes），不再是檔名。initrd 裡的 `Terminal.app`、`Echo.app`（之後 `Viewer.app`）由 Makefile 的 `BUNDLES` 產生（`exec` + `info.txt`：名稱、版本）。

## 5. 初始 handle `[3]`

`bootstrap::CWD`：kernel 建立行程時，如果有 `Data` 卷宗，`[3]` 是它的根（`READ|WRITE|MANAGE|DUPLICATE|TRANSFER`）；沒有就空（`BadHandle`）。P6 的 `ProcessSpawn` 讓父行程指定 `[1]`–`[3]`；P11 起最多 `[1]`–`[6]`：`[4]`（`bootstrap::PROGRAM`）= 程式所在的資料夾（shell 的 `run` 與 Mouseion 給；Linux 程式看得到它的卷宗），`[5]`（`STDIN`）= 標準輸入（E 部分），`[6]` 保留。

## 6. 掛載（`vfs::mount::mount_all`，開機時）

```
 每顆磁碟：GPT？
   有 → 每個分割區（起點要 4 KiB 對齊）：
          區塊 0 的 byte 1024 是 eHFS magic → eHFS（不看 type GUID）
          type GUID 是 ESP → FAT，名稱 Boot，唯讀
   沒有 → 整顆磁碟的區塊 0 是 eHFS magic → eHFS
 eHFS：掛載（日誌不乾淨就重做），名稱 = 卷宗名；第一個可寫的叫 Data
       （卷宗名不是 Data 時印警告，仍以 Data 掛上）
 重名加 " 2"、" 3"
```

```
vfs: mounted Boot (FAT, ro, 39 MiB), Data (eHFS, rw, 86 MiB), Archive (eHFS, rw, 62 MiB)
```

- 每個卷宗的裝置是 `CachedDevice<Partition<Disk>>`（eHFS 1024 塊（P11）、FAT 64 塊的快取，`docs/storage.md` §8）。
- QEMU 的 `fat:rw:` ESP 沒有 GPT（vvfat 是 MBR）→ 不掛載；`make run` 與一般的 `make test-qemu` 沒有卷宗（`vfs: no volumes`），程式照常從 initrd 啟動。
- 關機：selftest 結束前 `vfs::unmount_all()`（eHFS 標成 clean 並 flush）。P6.5 起一般 kernel 的 `SystemPower`（shell 的 `shutdown`／`reboot`）也先 `unmount_all()`（`docs/acpi.md`）；直接關掉 QEMU 不會壞（有日誌），只是 fsck 會說「沒有正常卸載」。

### 6.2 可移除的磁碟（P11 K41）

- **分割表**：GPT；沒有 GPT 時看 **MBR**（`block::mbr`：四個項目、`55 AA`、保護用的 `EE` 不算；開始的扇區要對齊 4 KiB）；都沒有就整顆。開機時的磁碟也走這條（QEMU 的 `fat:rw` ESP 有一個從扇區 63 開始的 MBR 分割區，對不齊，印一行略過——它本來就不掛）。
- **USB 磁碟**（`mount_removable`）：每個分割區是 eHFS 就用它的卷宗名稱掛（可寫），是 FAT 就用 FAT 的卷標（沒有卷標叫 `USB`，ESP 叫 `EFI`），FAT 目前唯讀；**不會**變成 `Data`。印 `vfs: mounted STICK (FAT, 62 MiB) from usb1`。
- **FAT 寫入（P11 K42）**：USB 上的 FAT 可寫，包括 USB 隨身碟上的 ESP（卷宗名 `EFI`；AHCI／NVMe 上的 ESP `Boot` 仍唯讀；之後要改 `boot.cfg` 得另外做「掛成可寫」的指令）。`kernel/src/fs/fat.rs`：建檔與資料夾（短名：合乎 8.3 的大寫名稱直接用，其他產生 `NAME~N.EXT` 並寫長檔名項目，UTF-16、每項 13 個；資料夾寫 `.` 與 `..`）、寫入（不夠就配 cluster：從上次的位置掃 FAT 找空的、清成 0、接到鏈尾）、截短／加長、刪除（項目標 `E5`、釋放 cluster；資料夾要是空的）、改名與搬移（先建新項目再指向舊資料、拿掉舊項目；搬資料夾時改它的 `..`）；每次改 FAT 都寫每一份 FAT；第一次寫時把 FAT32 的 FSInfo 空間數設成「不知道」（`FFFFFFFF`，規格允許）；時間寫 UTC（FAT 沒有時區）。剩餘空間數一次（之後隨配置與釋放更新）。不做碎片整理、不寫 FAT12。host 測試在 mtools 做的映像上寫、讀回、刪掉後空間回到原值。
- **`VolumeWatch(port)`**（syscall 63，P11 K42）：之後每次卷宗掛上或拿掉，kernel 送 `tag::VOLUME_CHANGED`（第一個 byte 1 = 掛上、2 = 拿掉，接著卷宗名稱）到那個 port（最多 16 個；關掉的 port 會被清掉）。libstela `fs::volume_watch`。Mouseion 用它：重畫桌面的卷宗圖示，拿掉的卷宗的視窗關掉（印 `mouseion: volume STICK came/went`）。
- **卸載**（`unmount_removable` → `vfs::unmount(name)`）：卷宗從清單拿掉（之後開不到），先 flush；已經開著的 handle 還留著那個卷宗，之後的讀寫得到 `IoError`（磁碟不在了）。
- **卷宗編號**：以前是清單裡的位置；P11 起卷宗會來來去去，編號改成不重複的計數（`VolumeInfo::index`、`Stat::volume`）。`VolumeList(i)` 仍是第 i 個；用編號找卷宗要 libstela 的 `fs::volume_by_index`（`cli::sys`、Classic 的 `datafs` 改用它）。

### 6.1 `Initrd`（P6）與 `Tmp`（P7）

P7 起 `InitrdFs` 是一棵資料夾樹（`initrd::Tree`：成員路徑 `linux/hello` → 資料夾 `linux` 裡的 `hello`），`Tmp` 卷宗是 kernel 記憶體裡的檔案系統（`vfs/tmpfs.rs`，≤ 16 MiB，關機就沒了；Linux 程式的 `/tmp`），見 `docs/linux.md` §7.1、§10。


initrd 也是一個卷宗：`InitrdFs`，唯讀，ustar 的檔案都在根目錄（id = 2 + 成員序號）；`mount_all` 最後掛上。程式（包括 kernel 自己啟動的 init）一律從檔案載入（`docs/userland.md` §2.1）。

## 7. 測試

| 測試 | 內容 |
|---|---|
| host `abi` | 編號、錯誤值、`DirEntry`／`Stat`／`VolumeInfo` 的大小與位移、`CWD`、`open::ALL` |
| fstest 第 35–36 步（P11） | `FilePath`：eHFS 的 `/fstest/hello.txt`、根是 `/`；FAT 的 `/EFI/BOOT/nested.txt`（預設的搜尋） |
| host `ehfs`（P11） | `list_after`：300 個項目一次一個接著列，順序和 `list` 一樣；一次 10 個、從刪掉的名字之後接下去、最後一個之後是空的 |
| fstest 第 37 步（P11） | cookie：列到 `gone`（key 順序第一個）後刪掉它，接著列出 `renamed.txt`、`sub` |
| host `kernel::vfs::path` | 切分、`.`／空的、`..` 拒絕、非 UTF-8、太長、單一名稱 |
| selftest "file systems" | 列出卷宗與空間；有 Boot 就直接用後端讀 `HELLO.TXT`、建立 → `ReadOnly`；有 Data 就跑 `fstest`（exit 0） |
| `fstest`（使用者程式） | `VolumeList`；`Data:/fstest-boots` 開機計數（`fstest: boot N`，第二次開機看到 2 = 持久）；建資料夾（`EXCLUSIVE`）、建檔、寫 10000 bytes、讀回、stat、讀到結尾、不分大小寫開啟、`EXCLUSIVE` 失敗、不存在、唯讀 handle 不能寫、改名、by-id 開啟、列目錄、`set_size`、非空資料夾不能刪、刪除後 by-id 的 handle 失效；Boot：讀 `HELLO.TXT`、唯讀卷宗建立 → `ReadOnly`、巢狀路徑 `EFI/BOOT/nested.txt` |
| `make test-storage` | 上面兩次開機 + host 的 `fsck.ehfs` |

## 8. 已知限制

- 沒有 `..`、沒有符號連結、沒有檔案鎖、沒有 mmap；沒有「開著的檔案被刪除仍可讀」的語意。
- 沒有 System 卷宗與 A/B、沒有配額、沒有快照、沒有延伸屬性／資源 fork 的 syscall（`docs/ehfs.md` §7）。
- 一個卷宗同時只做一個操作（eHFS 的 `SleepMutex`）。
- `VolumeOpen` 任何程式都能依名字開任何卷宗（之後的沙箱要收緊：`docs/architecture.md` §4.1 的 powerbox）。
