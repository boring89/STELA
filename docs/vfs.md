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
| 17 | `DirRead` | dir, index, &mut DirEntry | 1 = 有、0 = 到底 | READ |
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
- `DirRead` 以 index 列舉（每次呼叫都重新列出整個資料夾——簡單但大資料夾是 O(n²)）。
- 名稱與資料一律經 `AddressSpace::read/write` 複製（`FileRead`／`FileWrite` 每次 64 KiB）。

錯誤（接 P4 的 13）：14 `NotFound`、15 `Exists`、16 `NotDir`、17 `IsDir`、18 `NotEmpty`、19 `ReadOnly`、20 `NoSpace`、21 `IoError`（裝置錯誤或卷宗損壞）。

### 4.1 結構

```
 DirEntry（288 bytes）：id u64、size u64、mtime u64、kind u32（1 檔案、2 資料夾）、name_len u32、name [u8; 256]
 Stat（48 bytes）：id、size、ctime、mtime（u64）、kind、volume、flags（bit 0 唯讀）、reserved（u32）
 VolumeInfo（96 bytes）：index、flags（bit 0 唯讀）、kind（1 eHFS、2 FAT、3 initrd）、name_len（u32）、total、free（u64）、name [u8; 64]
```

時間是 1970 起的奈秒（UTC）；P6.5 起來自 CMOS RTC（`frame::time::wall_clock_ns`，`docs/time.md` §5.5），讀不到 RTC 時是「2026-01-01 + 開機以來」。

## 5. 初始 handle `[3]`

`bootstrap::CWD`：kernel 建立行程時，如果有 `Data` 卷宗，`[3]` 是它的根（`READ|WRITE|DUPLICATE|TRANSFER`）；沒有就空（`BadHandle`）。P6 的 `ProcessSpawn` 讓父行程指定 `[1]`–`[3]`。

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

- 每個卷宗的裝置是 `CachedDevice<Partition<Disk>>`（eHFS 256 塊、FAT 64 塊的快取，`docs/storage.md` §8）。
- QEMU 的 `fat:rw:` ESP 沒有 GPT（vvfat 是 MBR）→ 不掛載；`make run` 與一般的 `make test-qemu` 沒有卷宗（`vfs: no volumes`），程式照常從 initrd 啟動。
- 關機：selftest 結束前 `vfs::unmount_all()`（eHFS 標成 clean 並 flush）。P6.5 起一般 kernel 的 `SystemPower`（shell 的 `shutdown`／`reboot`）也先 `unmount_all()`（`docs/acpi.md`）；直接關掉 QEMU 不會壞（有日誌），只是 fsck 會說「沒有正常卸載」。

### 6.1 `Initrd`（P6）

initrd 也是一個卷宗：`InitrdFs`，唯讀，ustar 的檔案都在根目錄（id = 2 + 成員序號）；`mount_all` 最後掛上。程式（包括 kernel 自己啟動的 init）一律從檔案載入（`docs/userland.md` §2.1）。

## 7. 測試

| 測試 | 內容 |
|---|---|
| host `abi` | 編號、錯誤值、`DirEntry`／`Stat`／`VolumeInfo` 的大小與位移、`CWD`、`open::ALL` |
| host `kernel::vfs::path` | 切分、`.`／空的、`..` 拒絕、非 UTF-8、太長、單一名稱 |
| selftest "file systems" | 列出卷宗與空間；有 Boot 就直接用後端讀 `HELLO.TXT`、建立 → `ReadOnly`；有 Data 就跑 `fstest`（exit 0） |
| `fstest`（使用者程式） | `VolumeList`；`Data:/fstest-boots` 開機計數（`fstest: boot N`，第二次開機看到 2 = 持久）；建資料夾（`EXCLUSIVE`）、建檔、寫 10000 bytes、讀回、stat、讀到結尾、不分大小寫開啟、`EXCLUSIVE` 失敗、不存在、唯讀 handle 不能寫、改名、by-id 開啟、列目錄、`set_size`、非空資料夾不能刪、刪除後 by-id 的 handle 失效；Boot：讀 `HELLO.TXT`、唯讀卷宗建立 → `ReadOnly`、巢狀路徑 `EFI/BOOT/nested.txt` |
| `make test-storage` | 上面兩次開機 + host 的 `fsck.ehfs` |

## 8. 已知限制

- 沒有 `..`、沒有符號連結、沒有檔案鎖、沒有 mmap；沒有「開著的檔案被刪除仍可讀」的語意。
- 沒有 System 卷宗與 A/B、沒有配額、沒有快照、沒有延伸屬性／資源 fork 的 syscall（`docs/ehfs.md` §7）。
- 一個卷宗同時只做一個操作（eHFS 的 `SleepMutex`）。
- `VolumeOpen` 任何程式都能依名字開任何卷宗（之後的沙箱要收緊：`docs/architecture.md` §4.1 的 powerbox）。
