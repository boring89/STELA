# 量測：`make bench`（P11 K31，P12 第二筆；使用者已於 2026-10-22 審核）

**比喻**：`docs/architecture.md` §3 訂了幾個目標（kernel 小於 2 MiB、無 KVM 的 QEMU 五秒內到桌面、桌面閒置時 64 MiB 以內）。量尺要先做出來，每個階段量一次，才知道是變好還是變差。

## 1. 怎麼量

`make bench`（不在 `make test`）：

1. 一般 kernel、AHCI 測試磁碟（`build/images/ahci-bench.img`，有 Data 卷宗）、QEMU 的 user networking（e1000e），**不開 KVM**、單核心、512 MiB。
2. host 用 `tools/with_http.py` 提供 `build/www/bench.bin`（4 MiB 的 0）。
3. `qemu_test.py --times`：log 的每一行前面加上「QEMU 啟動後幾秒」（host 的時鐘）。
4. shell 出現後打 `run bench http://10.0.2.2:8642/bench.bin`；`bench` 程式（`user/bench`）一項一行印結果，最後 `bench: done`。
5. `tools/bench_report.py` 把 log 變成 Markdown 表格（`build/bench.md`，也印出來）；值得留的結果手動貼到下面 §3。

| 項目 | 怎麼量 |
|---|---|
| kernel.elf | `build/esp/STELA/kernel.elf` 的檔案大小 |
| 開機到桌面 | serial 上第一次出現 `STELA boot loader` 到 `mouseion: ready` 的 host 時間（另列從 QEMU 啟動算起，含 OVMF） |
| 開機里程碑 | loader、`boot: starting the kernel`、`STELA kernel`、`windowserver:`、`mouseion: ready`、`STELA shell` 第一次出現的時間 |
| 閒置記憶體 | `bench` 一開始的 `SysInfo(MEMORY)`：PMM 用掉的頁框 × 4 KiB（kernel、heap、所有行程與共享記憶體；這時桌面閒著，只多了 bench 自己）。另外列「扣掉 heap 沒用到的部分」（P11 的 heap 開機就拿 32 MiB；P12 起開機 4 MiB、按需成長，這一欄只差 heap 裡沒用的那一點） |
| syscall | `ClockNowNs` 連續 100,000 次的平均（進出 kernel 一次的成本） |
| 執行緒切換 | 同一行程的兩個執行緒用兩個 port 互丟訊息 10,000 來回（= 20,000 次切換）；每秒幾次。包含 port 的訊息複製，所以是「IPC + 切換」 |
| eHFS | `Data:/bench.tmp` 每次 64 KiB 循序寫 16 MiB、再讀回來（之後刪掉）；MB/s = 10^6 bytes/s |
| TCP | 從 host 下載 4 MiB（HTTP/1.0，連線到結束）；MB/s |

## 2. 怎麼看

- **TCG 的數字不代表實機**：QEMU 不開 KVM 是逐指令翻譯，CPU 密集的部分（syscall、切換、eHFS 的計算）比實機慢一個數量級以上；I/O（AHCI、e1000e）是 QEMU 模擬的裝置，也不是真的磁碟與網卡速度。它的用處是**同一台 host、不同版本之間比較**。
- 開機時間主要花在 OVMF（約 2 秒）；STELA 自己從 loader 到桌面不到半秒。
- **P12 的目標**（`docs/architecture.md` §3.1）：桌面閒置 ≤ 32 MiB + 畫面緩衝（寬 × 高 × 4 × 2）。2560×1600 時畫面緩衝約 31 MiB，目標約 63 MiB；P12 量到 57.0 MiB（§3）。
- **P11 時記憶體超過 64 MiB 的原因**（P12 已處理）：QEMU 的 OVMF 給 2560x1600 的畫面，一張全螢幕的畫布就是 2560 × 1600 × 4 = 16 MiB——視窗伺服器的合成緩衝、Mouseion 的桌面視窗各一張，terminal 的視窗約 4 MiB；再加上預留的 32 MiB heap（只用了不到 1 MiB）。在 1280x1024 的螢幕上畫布只有 5 MiB。要達到目標，可以讓 heap 按需成長、桌面不用一張全螢幕的畫布（之後的工作，沒有在 P11 做）。
- 計畫寫「兩個 kernel 執行緒 ping-pong」量 context switch；這裡用使用者空間的兩個執行緒與 port（不需要 selftest kernel，也量到 syscall 與 IPC 的實際路徑）——計畫外的決定。

## 3. 結果

（每次階段結束時加一節，舊的留著比較）

### 2026-10-22（P12 K16）

QEMU: QEMU emulator version 10.2.1 (Debian 1:10.2.1+ds-1ubuntu3.2) (TCG, no KVM; `-machine q35 -m 512M`, one processor, e1000e on user networking, AHCI disk)

| target (docs/architecture.md §3, §3.1) | goal | measured |
|---|---|---|
| kernel.elf | ≤ 2 MiB | 1.19 MiB (1251232 bytes) |
| system image (loader + kernel + initrd on the ESP) | ≤ 25 MiB | 9.5 MiB (9961888 bytes; the initrd 8.4 MiB) |
| boot to the desktop, no KVM | ≤ 5 s | 0.78 s from the loader (2.83 s from QEMU's start, firmware included) |
| memory on the idle desktop | ≤ 32 MiB + 2 × 2560 × 1600 × 4 ≈ 63 MiB | 57.0 MiB; 53.5 MiB without the heap's unused 3.6 MiB |
| smallest memory | boots to the desktop in 128 MiB | yes: `make test-qemu-min` (`-m 128M`) and the 128 MiB run of `make test-window` |

| boot milestone | seconds since QEMU started |
|---|---|
| loader starts | 2.04 |
| kernel starts | 2.24 |
| kernel ready | 2.73 |
| window server ready | 2.80 |
| desktop (Mouseion ready) | 2.83 |
| shell prompt | 2.86 |

| measurement | result |
|---|---|
| memory | used 58404 KiB of 523216 KiB, kernel heap 458 KiB used of 4096 KiB |
| syscall | 510 ns (ClockNowNs, 100000 calls) |
| switch | 170940 per second (two threads, port ping-pong, 20000 switches) |
| ehfs | write 12.9 MB/s, read 69.9 MB/s (16384 KiB) |
| tcp | 45.1 MB/s (4194509 bytes) |

- 閒置記憶體 93.0 → 57.0 MiB：kernel heap 只映射用到的（開機 4 MiB，P11 是 32 MiB 一次拿走），Mouseion 的桌面視窗只剩右邊的圖示直條（約 192 × 1568 × 4 ≈ 1.2 MiB，P11 是 16 MiB 的全螢幕畫布，`docs/window.md` §11）。
- 開機變慢約 0.35 秒（loader → 桌面 0.43 → 0.78 秒）：幾乎都在掛載卷宗這一步（`vfs: ahci0p0 …` 到 `vfs: mounted` 約 0.3 秒）。PCI 掃全部 256 個 bus 只花約 10 ms；把初始 heap 暫時改回 32 MiB 量過一樣慢，所以不是 heap 的成長。原因還沒查到（記在 CLAUDE.md 的已知問題）。
- syscall 比 P11 慢約 19%（430 → 510 ns）、切換約 9%（186915 → 170940 次／秒）。可能的原因（沒有分開量）：syscall 的進出多存／取 10 個暫存器（Linux 訊號需要完整的使用者狀態，`docs/syscall.md` §4.3），每次回 ring 3 前的檢查多看一個欄位；TCG 的數字本來也有幾 % 的起伏。

### 2026-10-21（P11 K31）

QEMU: QEMU emulator version 10.2.1 (Debian 1:10.2.1+ds-1ubuntu3.2) (TCG, no KVM; `-machine q35 -m 512M`, one processor, e1000e on user networking, AHCI disk)

| target (docs/architecture.md §3) | goal | measured |
|---|---|---|
| kernel.elf | ≤ 2 MiB | 0.98 MiB (1025552 bytes) |
| boot to the desktop, no KVM | ≤ 5 s | 0.43 s from the loader (2.37 s from QEMU's start, firmware included) |
| memory on the idle desktop (all frames in use: kernel, heap, window server, Mouseion, terminal, shell) | ≤ 64 MiB | 93.0 MiB; 61.2 MiB without the heap's unused 31.8 MiB |

| boot milestone | seconds since QEMU started |
|---|---|
| loader starts | 1.94 |
| kernel starts | 2.06 |
| kernel ready | 2.27 |
| window server ready | 2.33 |
| desktop (Mouseion ready) | 2.37 |
| shell prompt | 2.43 |

| measurement | result |
|---|---|
| memory | used 95192 KiB of 523216 KiB, kernel heap 225 KiB used of 32768 KiB |
| syscall | 430 ns (ClockNowNs, 100000 calls) |
| switch | 186915 per second (two threads, port ping-pong, 20000 switches) |
| ehfs | write 12.2 MB/s, read 64.2 MB/s (16384 KiB) |
| tcp | 47.1 MB/s (4194509 bytes) |
