# 儲存：區塊層、DMA、AHCI、NVMe、快取、GPT、FAT

> P5 由自動排程一次做完，使用者已於 2026-10-20 審核。

磁碟怎麼被讀寫：從「一個 4 KiB 區塊」的抽象，到 SATA（AHCI）與 NVMe 控制器的命令，到分割表與 ESP 的 FAT。檔案系統 eHFS 見 `docs/ehfs.md`，卷宗與檔案 syscall 見 `docs/vfs.md`。

程式碼：

```
block/src/lib.rs            BlockDevice trait、check_request、Arc/& 的轉接
block/src/partition.rs      Partition<D>：一段區塊的窗
block/src/gpt.rs            GPT（512 與 4096 扇區）
block/src/crc32.rs          CRC-32（GPT 用）
block/src/mem.rs            MemDisk（測試用，可模擬斷電）
frame/src/mm/dma.rs         DmaBuffer：連續、清零的頁框
drivers/src/sync.rs         排程 hooks、wait_for、SleepLock、SleepMutex、spin_for
drivers/src/ahci.rs         AHCI 驅動
drivers/src/nvme.rs         NVMe 驅動
kernel/src/block.rs         開機時 probe，磁碟清單
kernel/src/fs/cache.rs      區塊快取（LRU、write-through）
kernel/src/fs/fat.rs        FAT16/32 唯讀
tools/make_test_disk.py     tools/make_test_images.py   測試映像（只在 build/）
```

## 1. 比喻：倉庫與搬運工

- **區塊裝置**是一排編了號的**箱子**（每箱 4 KiB）。檔案系統只會說「給我第 1234 箱」「把這些放進第 88 箱」。
- **驅動**是搬運工：它知道這座倉庫（SATA 碟、NVMe SSD）要怎麼填單子（命令）、單子放哪（命令清單／佇列）、按哪個鈴（doorbell、PxCI）。
- **DMA** 是倉庫自己的輸送帶：控制器直接把資料搬進記憶體（不經過 CPU），所以要給它**實體位址**、而且那段記憶體在搬的時候不能被挪作他用。
- **中斷**是搬完時的鈴聲；等貨的人先去睡（`block`），鈴響了被叫醒。
- **快取**是門口的暫存架：最近用過的箱子放一份在記憶體，下次直接拿。
- **GPT** 是倉庫的分區圖：哪一段箱子是 ESP（開機用的 FAT）、哪一段是 eHFS 的 Data。

## 2. `BlockDevice`（`block` crate）

```rust
pub trait BlockDevice {
    fn block_count(&self) -> u64;                                  // 4 KiB 區塊數
    fn read(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError>;   // buf 是 4096 的倍數
    fn write(&self, block: u64, buf: &[u8]) -> Result<(), BlockError>;
    fn read_only(&self) -> bool { false }
    fn flush(&self) -> Result<(), BlockError> { Ok(()) }
}
```

- 永遠 4 KiB；驅動自己換算扇區（AHCI 512、NVMe 512 或 4096）。
- **同步**：回來時資料已在裝置上（寫）或在 buffer 裡（讀）。
- 為什麼是獨立的小 crate：kernel、drivers、ehfs、host 工具都要用它，但彼此不該相依（避免循環）。`no_std`、host 測試。
- `Partition<D>`：`start..start+count` 的窗，越界 `OutOfRange`；唯讀分割區寫入 `ReadOnly`。
- kernel 裡的磁碟型別：`Disk = Arc<dyn BlockDevice + Send + Sync>`。

## 3. DMA 緩衝區（`frame::mm::dma`）

- `DmaBuffer::new(pages)`：PMM 的連續頁框（`alloc_contiguous`）、清零；`phys()` 給裝置、`read/write(offset, bytes)` 以複製存取、`read_u32/u64`、`write_u32/u64` 是 volatile（描述子、完成佇列）。drop 時還給 PMM——驅動要先確定裝置不再用它。
- 不交出 `&[u8]`：裝置隨時可能寫，Rust 的參考會是錯的假設。
- x86 的 DMA 與快取是一致的（snooping，Intel SDM Vol. 3A §12.3），不必 flush。實體位址沒保證 < 4 GiB；目前 QEMU 的記憶體都在下面（AHCI 的 `CAP.S64A` 也是 yes）。

## 4. 等待：中斷 + 重檢查（`drivers::sync`）

drivers 不能依賴 kernel（kernel 依賴 drivers），所以 kernel 在開機時註冊 hooks：

```rust
drivers::sync::set_hooks(Hooks { current, block_until, wake, now_ns })
```

- `wait_for(timeout, register, done)`：`register(自己的 thread id)`（驅動在這裡記下等待者並按門鈴）→ 迴圈：`done()` 就回；否則 `block_until(min(now + 10 ms, 期限))`。中斷處理程式只做「清狀態、`wake(等待者)`」。**中斷掉了只會慢 10 ms，不會錯**。期限 5 秒。
- `SleepLock`：可以「拿著睡」的鎖（`SpinLock` 會關中斷，不能拿著等 I/O）：一個 busy 旗標 + 等待者清單；每個裝置一把，一次一個請求。
- `SleepMutex<T>`：`SleepLock` + `SpinLock<Option<T>>`，`with(|v| ...)` 時把值「拿出來」用、用完放回去——不需要 `unsafe`。eHFS 卷宗用它。
- `spin_for(ns, cond)`：初始化時的短等待（控制器重設），用時間而不是迴圈次數（TCG 下 MMIO 讀很慢，迴圈次數會變成幾十秒）。

## 5. AHCI（Serial ATA AHCI 1.3.1）

```
 ABAR（BAR5）  0x00 CAP  0x04 GHC  0x08 IS  0x0C PI
               0x100 + 0x80·port：CLB FB IS IE CMD TFD SIG SSTS SERR CI

 每個 port 一頁 DMA：
   0x000  command list（32 個 32 bytes 的 header；只用 slot 0）
   0x400  received FIS（256 bytes）
   0x500  command table：command FIS（64）、ATAPI（16）、保留、0x80 PRDT（一項）
 另外 64 KiB 的資料緩衝區
```

1. `map_bar(5)`、bus master、`GHC.AE`、`GHC.HR`（重設，≤ 1 秒）、再 `AE`。
2. MSI（`enable_msi`），`IS` 清掉，`GHC.IE`。
3. 每個 `PI` 裡的 port：等 `SSTS.DET = 3`（≤ 10 ms，沒裝置就跳過）→ 停掉引擎（清 ST、FRE，等 CR、FR 清，AHCI §10.1.2）→ 設 CLB/FB → 清 SERR、IS → **開 FRE** → 等 `TFD` 不忙（裝置的第一個 D2H FIS 帶著 signature）→ `SIG == 0x101`（SATA 碟；ATAPI 跳過）→ 開 ST → `PxIE`。
4. IDENTIFY（0xEC）：48 位元 LBA 的扇區數（word 100–103）、邏輯扇區大小（word 106/117–118）。

命令（slot 0）：header DW0 = `CFL 5 | W << 6 | PRDTL << 16`、CTBA；H2D FIS（type 0x27、C=1、命令、LBA 48 位元、device 0x40、count）；PRDT 一項（DBA、DBC = bytes−1、I）。READ DMA EXT 0x25、WRITE DMA EXT 0x35、FLUSH CACHE EXT 0xEA。一次最多 16 個區塊（64 KiB）。送出：`PxCI = 1`，等 `CI` bit 0 清掉或 `IS.TFES`（task file error）。

中斷處理（AHCI §10.7.2.1 的順序）：對 `IS` 裡每個 port **先清 `PxIS`、再清 `IS`**，把 PxIS 位元交給等待的請求並 `wake`。順序反了會一直重新觸發（實作時真的遇到中斷風暴把開機卡住）。

## 6. NVMe（NVM Express Base 1.4）

```
 BAR0  0x00 CAP（MQES、DSTRD、TO）  0x14 CC  0x1C CSTS  0x24 AQA  0x28 ASQ  0x30 ACQ
       0x1000 + (2y [+1])·(4 << DSTRD)：SQ y tail／CQ y head doorbell
 submission 64 bytes：DW0 opcode | CID << 16、DW1 NSID、DW6–9 PRP1/2、DW10–15
 completion 16 bytes：DW2 SQ head、DW3 CID | phase(bit 16) | status(31:17)
```

1. `CC.EN = 0`、等 `CSTS.RDY = 0`（`CAP.TO` × 500 ms）。
2. admin 佇列（16 項）：AQA、ASQ、ACQ；`CC = IOCQES 4 | IOSQES 6 | EN`（4 KiB 頁、NVM 命令集），等 RDY。
3. MSI/MSI-X；Identify Controller（CNS 1：namespace 數在 byte 516）。
4. Create I/O CQ 1（32 項、IEN、vector 0）、Create I/O SQ 1。
5. 每個 namespace：Identify Namespace（NSZE、FLBAS → LBA format 的 LBADS）；支援 512 與 4096 bytes。

讀寫一次一個 4 KiB 區塊（PRP1 一頁）：寫 SQ entry → `wait_for` 的 register 裡按 SQ tail doorbell → 等 CQ head 那項的 **phase 位元**等於預期（每繞一圈預期值翻轉）→ 前進 head、按 CQ head doorbell → 檢查 CID 與 status。

## 7. 開機：`kernel::block::probe`

設 hooks → `ahci::probe()` + `nvme::probe()` → 名稱 `ahci0p1`（控制器 0 的 port 1）、`nvme0n1`。QEMU 的 `-drive …fat:rw:` ESP 也是 AHCI 的 port 0（`ahci0p0`）：**selftest 只寫開頭有 `STELATEST` 的磁碟**。

## 8. 區塊快取（`kernel::fs::cache`）

- `CachedDevice<D>`：固定容量（eHFS 卷宗 256 塊 = 1 MiB、FAT 64 塊），LRU（`BTreeMap` 的使用時間）。
- **write-through**：寫入先到裝置，成功再更新已快取的那份（失敗就丟掉那份，因為裝置上可能是新的也可能是舊的）。eHFS 的一致性靠日誌，快取永遠不比磁碟新。
- 鎖是 `SpinLock`，**I/O 時不持有**：miss 時放鎖、讀裝置、再拿鎖放進去（兩個執行緒同時 miss 會各讀一次，結果相同）。
- 為了快取與 B-tree 的 16 KiB 節點，kernel heap 從 4 MiB 加到 16 MiB（計畫外的調整）。

## 9. GPT（UEFI 2.10 §5.3）

```
 LBA 0   protective MBR（測試映像在開頭放 "STELATEST"）
 LBA 1   header："EFI PART"、header CRC32（欄位清 0 後算）、entries 的 LBA、數量、大小、entries CRC32
 LBA 2…  128 bytes 一項：type GUID、unique GUID、first/last LBA、名稱（UTF-16LE）
```

- 扇區大小：區塊 0 的 byte 512 是 "EFI PART" → 512；否則讀區塊 1 → 4096（4Kn 磁碟，例如測試的 NVMe）。
- `blocks_of`：分割區起點要 4 KiB 對齊（否則不掛載、印警告）；結尾不足一塊的部分捨去（sgdisk 讓最後一個分割區結束在備份表前，不一定對齊）。
- ESP 的 type GUID `C12A7328-F81F-11D2-BA4B-00A0C93EC93B`（磁碟上前三段小端序）。

## 10. FAT（Microsoft FAT spec 1.03），唯讀

- BPB：bytes/sector、sectors/cluster、保留扇區、FAT 數、根目錄項數（FAT16）、總扇區、FAT 大小、根 cluster（FAT32）、卷標。**FAT 類型只看 cluster 數**：< 4085 是 FAT12（不支援）、< 65525 FAT16、其他 FAT32。
- 目錄項 32 bytes；LFN（attr 0x0F）以相反順序排在短名前，13 個 UTF-16 單位一段，checksum 要和短名對得上；短名照 NT 的大小寫旗標（byte 12：0x08 主檔名小寫、0x10 副檔名小寫，mtools 會這樣寫）。
- 名稱比較忽略 ASCII 大小寫；`resolve("EFI/BOOT/x")`；`read_file(entry, offset, buf)` 沿 cluster chain，cluster 可以比 4 KiB 小（測試映像是 512 bytes）。
- ESP 若太小可能是 FAT16，所以也支援 FAT16（計畫只說 FAT32）。

## 11. 測試

| 測試 | 內容 |
|---|---|
| host `block` | 請求檢查、CRC-32 檢查值、MemDisk 讀寫與斷電、Partition 的範圍與唯讀、GPT（自己組的 512 與 4096 扇區表、CRC 壞掉、不對齊）、sgdisk 做的映像 |
| host `drivers` | AHCI 的 FIS／header 編碼、IDENTIFY 解析（48 位元、4 KiB 扇區、舊式 28 位元）、扇區換算；NVMe 的 submission entry、Identify Namespace 解析 |
| host `kernel::fs` | FAT：mtools 做的 FAT32 映像（短名、長名、NT 小寫、子目錄、300 KiB 檔案跨 cluster、從中間讀）；快取：命中／未命中／淘汰、write-through、越界 |
| selftest "block devices" | 每顆磁碟讀區塊 0；測試磁碟（`STELATEST`）寫 20 個區塊（超過一個 64 KiB 的 AHCI 命令）再讀回、越界、block 64 的開機計數 |
| `make test-storage` | AHCI（GPT：FAT ESP + eHFS Data，512 扇區）+ NVMe（GPT：eHFS Archive，`logical_block_size=4096`）開機兩次：`storage: … boot 1/2`、`vfs: mounted …`、fstest；最後 host 的 `fsck.ehfs` 檢查兩個卷宗 |
| `make test-qemu-min` | Conroe、無 NX、256 MiB，也接 AHCI 測試磁碟（自己的副本）跑完 fstest，結束後 fsck |

## 12. 已知限制

- 一次一個請求（AHCI 一個 slot、NVMe 佇列深度實際是 1、每次一個 4 KiB 區塊）；沒有 NCQ、沒有排程。
- 沒有 TRIM、沒有熱插拔、沒有錯誤後的 port 重設；逾時就回錯誤。
- 快取是 write-through、沒有 read-ahead；容量固定。
- FAT 唯讀、沒有 FAT12、不讀 MBR 分割表（QEMU 的 vvfat ESP 不會被掛載）。
- 沒有 USB 儲存（P8）。
