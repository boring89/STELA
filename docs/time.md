# 時間：PIT 校準、APIC timer 與單調時鐘

> P2 由自動排程一次做完，使用者已於 2026-10-20 審核。

程式碼：`frame/src/time/`（`calc.rs` 純邏輯、`mod.rs`）、`frame/src/arch/x86_64/pit.rs`、`apic.rs` 的 timer 暫存器。中斷的部分見 `docs/irq.md`。

## 1. 比喻：節拍器與碼錶

kernel 需要兩種時間：

- **節拍器**（tick）：每 1 ms「滴」一聲，叫醒 CPU。之後的排程器靠它切換執行緒；現在 `sleep_ms` 靠它醒來。這是 **local APIC timer**。
- **碼錶**（`now_ns()`）：隨時看一眼就知道開機到現在幾奈秒，而且不會倒退。最好的碼錶是 CPU 的 **TSC**。

問題是：節拍器和碼錶的「刻度」每台電腦不一樣（APIC timer 跟著匯流排頻率、TSC 跟著 CPU 頻率），沒人告訴我們。所以開機時拿一個**頻率已知的老時鐘（PIT）**來對時：讓 PIT 走 10 ms，看節拍器和碼錶各走了多少格。

## 2. 名詞

| 名詞 | 意思 |
|---|---|
| PIT | Intel 8254 可程式計時器，1981 年的 PC 就有；輸入時脈固定 **1,193,182 Hz** |
| APIC timer | local APIC 內建的倒數計時器，倒數到 0 送一個中斷；頻率 = 匯流排（或晶振）頻率 ÷ 除數 |
| TSC | Time-Stamp Counter：CPU 內每個時脈加一的 64 位元計數器，`rdtsc` 讀取 |
| invariant TSC | 不隨省電狀態、變頻而改變速度的 TSC（`CPUID.80000007H:EDX[8]`） |
| tick | 計時器中斷一次；STELA 是 1000 Hz，一個 tick = 1 ms |
| 單調（monotonic） | 只會增加或不變，不會倒退（不受「調時間」影響） |

## 3. PIT 校準（OSDev "Programmable Interval Timer"、"APIC timer"）

### 3.1 用 channel 2，不用中斷

PIT 有 3 個 channel。channel 2 本來接 PC 喇叭，好處是：它的 **gate**（開始／暫停）由軟體控制（port `0x61` bit 0），輸出可以讀回（port `0x61` bit 5），不需要中斷。bit 1 會把輸出接到喇叭，校準時保持 0（不出聲）。

```
port 0x43 ← 0xB0      channel 2、先低後高位元組、mode 0、二進位
port 0x42 ← 0x9C      11932 的低位元組
   ── 取樣 start（APIC 目前計數、TSC）──
port 0x42 ← 0x2E      高位元組：寫入後開始倒數，輸出變低
   … 輪詢 port 0x61 bit 5，直到輸出變高（倒數到 0）…
   ── 取樣 end ──
```

1,193,182 Hz ÷ 100 = **11932** 個 PIT 時脈 = 10 ms。mode 0 = "interrupt on terminal count"：倒數到 0 時輸出從低變高（Intel 8254 datasheet）。

如果輸出一直不變高（有些新平台把 8254 關掉了），輪詢約 1000 萬次後放棄並 panic，而不是永遠卡住。

### 3.2 同時量兩個東西

在這 10 ms 裡：

- APIC timer：除數 16、one-shot、初始計數 `0xFFFF_FFFF`，開始前後讀「目前計數」（往下數），差就是走了幾格；
- TSC：開始前後 `rdtsc`，差就是走了幾格。

做 **3 次取中位數**（各自取）：一次被干擾（例如 QEMU 的主機忙）不會影響結果。換算：

```
頻率 = 走的格數 × 1,193,182 ÷ 11932        （u128 計算，不會溢位）
```

QEMU（無 KVM）實測：

```
APIC timer：每 10 ms 約 625,130 格 → 62.513 MHz（÷16 之前是 1 GHz，QEMU 的 APIC 匯流排）
TSC       ：約 3696 MHz（主機 CPU 3.7 GHz）
```

## 4. APIC timer：1000 Hz（Intel SDM Vol. 3A "APIC Timer"）

```
0x3E0  divide configuration   0b0011 = ÷16
0x320  LVT timer              bit 17 = 週期模式、bits 0–7 = 向量 48
0x380  initial count          頻率 ÷ 1000 = 62,513（QEMU）
0x390  current count          （校準時讀）
```

週期模式：數到 0 就送中斷，並自動從 initial count 重來。所以每 62,513 格（= 1 ms）一個中斷。

向量 48 的 handler（`timer_interrupt`）：全域 `TICKS += 1`、`cpu::current().tick()`（每顆 CPU 自己的計數），再呼叫 kernel 用 `time::set_tick_hook` 註冊的函式（P3：排程器的時間片與睡眠清單，`docs/process.md` §6.2）。EOI 由 `irq::dispatch` 代做。

## 5. TSC 與 `now_ns()`

### 5.1 TSC 的頻率從哪來

依序：

1. `CPUID.15H`：EAX、EBX、ECX 都非 0 → TSC Hz = ECX（晶振 Hz）× EBX ÷ EAX；
2. `CPUID.16H`：EAX = 處理器基頻（MHz）→ × 1,000,000（有 invariant TSC 的處理器上，TSC 就以基頻跑）；
3. 都沒有 → 用 §3 的 PIT 校準值。

QEMU 與這台主機的 KVM（Xeon W-2135）都沒有回報 15H/16H，所以用 PIT；開機訊息會標出來源（`CPUID` 或 `PIT`）。

### 5.2 有 invariant TSC

```
now_ns() = (rdtsc() − 開機時的 TSC) × 1,000,000,000 ÷ TSC Hz      （u128）
```

解析度約 0.3 ns（3.7 GHz），不需要中斷就能讀。用 u128 是因為 TSC 差 × 10⁹ 在幾秒後就超過 u64。

### 5.3 沒有 invariant TSC → 退回 tick

舊 CPU 的 TSC 會隨變頻改速度，不能當時鐘。這時：

```
now_ns() = ticks() × 1,000,000        （解析度 1 ms）
```

QEMU 的 TCG（純軟體模擬）**不支援** invariant TSC（`-cpu max`、`+invtsc` 都一樣，QEMU 會警告 "TCG doesn't support requested feature"），所以 `make test` 的兩種設定都走這條退路。有 invariant TSC 的路徑在本機用 `-enable-kvm -cpu host,+invtsc` 手動驗證過：`TSC 3700 MHz (PIT, invariant)`、`sleep_ms(50)` 用 TSC 量是 50 ms。

### 5.4 API

| 函式 | 說明 |
|---|---|
| `time::ticks()` | 計時器啟動以來的 tick 數 |
| `time::now_ns()` | 開機以來的奈秒（單調） |
| `time::sleep_ms(ms)` | 等到 `ticks() ≥ 開始 + ms + 1`（保證至少 ms 毫秒），中間 `sti; hlt`；IF=0 時呼叫會 panic（永遠醒不來） |
| `time::tsc_hz()`、`tsc_invariant()` | TSC 頻率（校準或 CPUID）、是否 invariant |
| `time::TICK_HZ` | 1000 |

## 6. 測試

| 測試 | 內容 |
|---|---|
| host `time::calc` | 中位數、頻率換算（含 0 與大數）、CPUID 15H/16H 的組合、`Clock::ns` |
| selftest "timer tick" | 最多 1000 次 `hlt` 內 tick 增加 ≥ 10；per-CPU tick 與向量 48 的計數也增加 |
| selftest "clock and sleep" | `now_ns` 不倒退；`sleep_ms(50)` 至少 50 tick；invariant TSC 時 TSC 量出 40–60 ms 並印 `tsc: <MHz> MHz`，否則印 `tsc: not invariant` |
| selftest "keyboard" | 等按鍵最多 5 秒（`ticks()`） |

PIT 與 APIC timer 在 QEMU 都跟著虛擬時鐘走，所以即使主機很忙也一致；12 個 QEMU 同時跑的壓力測試都通過。

## 7. 已知限制與待辦

- 只有週期模式；沒有 tickless、TSC-deadline 模式、HPET。
- 沒有 invariant TSC 時 `now_ns()` 只有 1 ms 解析度，而且**少算的 tick 補不回來**：中斷關著的時間超過 1 ms（例如持主控台的鎖捲動整個 framebuffer），中間的 tick 會合併成一個，uptime 會變慢。
- 只校準一次；TSC 頻率在沒有 invariant TSC 的機器上可能隨變頻偏掉（但那時 `now_ns` 不用它）。
- 依賴 8254 PIT；沒有 PIT 的平台會 panic（之後可用 CPUID 15H 的晶振頻率或 HPET 校準）。
- AP 啟動後每顆 CPU 要各自設定自己的 APIC timer（每顆 CPU 的 local APIC 是獨立的）；校準結果可共用。
