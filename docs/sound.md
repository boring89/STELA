# 音效：Intel HD Audio 與 Audio 物件

> P10 由自動排程一次做完，**使用者尚未審核**；之後修改前請先帶使用者過一遍這份文件。

## 1. 比喻：一條水管

程式把數字（取樣，sample）倒進水桶（和 kernel 共用的環形緩衝區）；kernel 的 audio 執行緒把桶裡的水舀進抽水機前面的水槽（DMA 的循環緩衝區）；**HDA 控制器**像抽水機，用 DMA 自己把水槽裡的數字一個一個抽走，經過 HDA 連結送到 **codec**（裡面的 DAC 把數字變成電壓），再經過音量（amp）到插孔（pin）。

```
 程式 ──寫──► 共用環（64 KiB）──audio 執行緒每 5 ms──► DMA 循環緩衝（64 KiB = 2 × 32 KiB）
                                                        │ 控制器讀（LPIB = 讀到哪）
                                                        ▼
                                     HDA 連結 ──► codec：DAC ─►（mixer）─► pin ─► 喇叭
```

格式固定：**48 kHz、16 位元有號、雙聲道**（每個 frame 4 bytes，每秒 192,000 bytes）。stream format 字 `0x0011`（HDA 規格 3.7.1：48 kHz 基礎、16 位元、2 聲道）。

## 2. 驅動（`drivers/src/hda.rs`）

資料來源：Intel High Definition Audio Specification 1.0a；68k_HAL 的 `drivers/hda` 與 `docs/sound.md`。

1. PCI class 04/03 的第一個控制器：bus master、BAR0。
2. **重置控制器**：`GCTL.CRST` 寫 0、等它變 0，寫 1、等它變 1；等 2 ms 讓 codec 報到，`STATESTS` 的第一個位元就是要用的 codec。
3. **命令**：用**立即命令暫存器**（規格 3.4.3）：等 `ICIS.ICB`（bit 0）清掉 → 寫 `ICOI` → 設 ICB → 等 `ICIS.IRV`（bit 1）→ 讀 `ICII` → 寫 1 清 IRV。不用 CORB/RIRB 環（68k_HAL 發現 QEMU 透過環只回應第一個命令）。
4. **找輸出路徑**（`find_output`，純邏輯，經 `Codec` trait，host 用假 codec 測）：根節點 → audio function group（type 1）→ widget 們；找 pin 的 configuration default 不是「沒有連接」（bits 31:30 ≠ 1）且裝置是 line out／speaker／headphone（bits 23:20 = 0–2），沿著 connection list（短格式每個回應 4 個 7 位元、長格式 2 個 15 位元）找到 audio output，直接或經過一層 mixer／selector。
5. **準備路徑**（`prepare`）：function group 與路徑上的節點電源 D0；amp 取消靜音設 0 dB（output amp `0xB000 | 0dB`、每個 input amp `0x7000 | index << 8 | 0dB`，0 dB 的步數來自 amp capabilities）；pin 選對的輸入；DAC 設 stream tag 1、channel 0、格式；pin control 開輸出（0x40）、EAPD（0x02）。
6. **輸出 stream**：descriptor 在 `0x80 + 輸入 stream 數 × 0x20`；SRST 1→0 重置；BDL（一頁 DMA）兩項，各 32 KiB、IOC；CBL = 64 KiB、LVI = 1、FMT、BDLPL/U；stream tag 寫在 CTL 的第三個 byte。開啟時設 `RUN`，關閉時清掉。

驅動提供 `start`、`stop`、`position`（LPIB）、`write(offset, data)`（環繞）、`silence`。

QEMU 的 `intel-hda` + `hda-duplex`：codec 0，pin 3 接 converter 2（開機時印 `hda: 0000:00:03.0: codec 0, output pin 3 from converter 2, stream at 0x100`）。

## 3. kernel：Audio 物件與 audio 執行緒（`kernel/src/audio.rs`、`audiofeed.rs`）

- `audio::start()`（開機時，`usb::start` 之後）：probe 控制器、清空 DMA 緩衝、啟動 `audio` 執行緒。
- `AudioOpen` 一次只給一個程式（第二個 → `Busy`）：建立共用記憶體（`abi::audio`：一頁表頭 + 64 KiB 取樣）、清空 DMA、`RUN`，開始餵；回傳 Audio handle（`object_kind::AUDIO` = 10）與共用記憶體的 handle（MAP）。
- **餵**（每 5 ms，`audiofeed::Feeder`，純邏輯 host 測試）：控制器的位置用「讀過的 byte 數」記（LPIB 的差模 64 KiB 累加）；從 `fill` 填到 `hw + 16 KiB`（約 85 ms 的領先）：先拿共用環的取樣（`read_pos..write_pos`），不夠就**補 0**（欠載時播靜音、不停 stream、不重播舊聲音，避免爆音）；執行緒太晚、控制器已經超過填好的部分時，`fill` 跳到控制器的位置。
- 欠載計數（表頭 offset 20）：一段裡環在中途用完才算（程式跟不上）；程式閒著的靜音不算。開頭與結尾各可能算一次。
- `AudioClose`（或最後一個 handle 消失，`AudioSession` 的 `Drop`）：清 `RUN`、清空 DMA。

### 3.1 共用環（`abi::audio`）

```
 offset 0     write position（u32）：程式寫過的 byte 總數
 offset 4     read position（u32）：kernel 拿走的 byte 總數
 offset 8     環的大小（u32，65536）   offset 12  rate（48000）
 offset 16    聲道數（2）               offset 20  欠載次數
 offset 4096  取樣：16 位元有號小端序，左右交錯
```

位置在 2³² 繞回；環裡的資料 = `write - read`，空間 = `size - (write - read)`；第 `p` 個 byte 在 `4096 + p % size`。程式先寫取樣再更新 write position（x86 的儲存不會重排）。

## 4. 系統呼叫與 libstela

| 號碼 | 呼叫 | 參數 | 回傳 |
|---|---|---|---|
| 47 | `AudioOpen` | rate（48000）, channels（2）, &mut [u64; 2] | 0；寫入 Audio handle 與環的 Memory handle；別的值 → `NotSupported`；已被開 → `Busy`（新錯誤 26） |
| 48 | `AudioClose` | audio | 0；停止輸出並關閉 handle |

libstela `audio::Audio`：`open()`（映射環）、`writable()`（frame 數）、`write(&[[i16; 2]])`（放得下多少放多少）、`write_all`（滿了就睡 10 ms）、`queued()`、`drain(timeout)`、`underruns()`；`Drop` 時 `AudioClose`。`audio::square(hz, amplitude, start, frames)` 產生方波。

`tone [Hz [ms]]`（initrd，shell 直接打名字）：預設 440 Hz、1000 ms 的方波，播完印 `tone: done (N underruns)`。

## 5. 測試

| 測試 | 內容 |
|---|---|
| host `drivers::hda` | verb 編碼；假 codec：經 mixer 的路徑、送出的命令（stream、格式、pin、連接選擇、amp）、沒有輸出 pin 時沒有路徑、長格式連接表；BDL 項目 |
| host `kernel::audiofeed` | 維持領先、繞回、執行緒太晚時跳過、環的 byte 數（含 u32 繞回） |
| host `libstela::audio` | 方波 |
| host `abi` | 編號 47–48、錯誤 26、環的大小 |
| `make test-sound` | QEMU `-audiodev wav,id=a0,path=build/out.wav -device intel-hda -device hda-duplex,audiodev=a0`；shell `tone` → `tone: done` → `shutdown`（QEMU 正常結束才會寫好 WAV 檔頭）；`tools/check_wav.py` 要至少 1 秒大聲的資料、過零點算出來約 440 Hz（±10%） |

## 6. 計畫外的決定

- `AudioOpen` 用一個輸出指標同時給兩個 handle（syscall 只有一個回傳值）。
- 新錯誤 `Busy`（26）。
- 欠載只在「環在一段中途用完」時計數。
- `make test-sound` 在 `make test` 裡（約 15 秒）。

## 7. 已知限制

- 只有輸出、只有 48 kHz 16 位元雙聲道；沒有混音（一次一個程式）、音量控制、錄音、HDMI 音訊的選擇（只用第一個 codec 的第一條路徑）。
- 不用中斷：audio 執行緒每 5 ms 輪詢 LPIB（播放中與閒置都在跑）。
- 延遲約 85 ms（領先量）加上程式自己環裡的資料。
- 實機（P520 的 Realtek codec、GTX 1070 的 HDMI 音訊）的路徑選擇沒有驗證。
- Classic 環境的聲音（ASC）還沒接上（`docs/classic.md` §6）。
