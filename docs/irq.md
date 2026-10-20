# 中斷：ACPI MADT、APIC、per-CPU 與鎖

> P2 由自動排程一次做完，**使用者尚未審核**。之後修改前請先帶使用者過一遍這份文件。

這份文件說明 STELA 怎麼讓硬體「打斷」CPU：從 ACPI 表找出中斷控制器、關掉舊的 8259、打開 APIC、把 256 個向量都接上處理程式，以及「中斷與鎖」的規則。計時器見 `docs/time.md`，鍵盤見 `docs/keyboard.md`。

程式碼：`frame/src/acpi/`、`frame/src/arch/x86_64/{pic,apic,ioapic,msr,interrupts,exceptions,idt}.rs`、`frame/src/irq.rs`、`frame/src/cpu.rs`、`frame/src/mm/mmio.rs`、`frame/src/sync.rs`。

## 1. 比喻：公司的總機與櫃台

P1 的 kernel 像一個只會照稿子念的人：它從頭做到尾，不接電話。P2 讓它接電話。

- **裝置**（鍵盤、計時器）是打電話來的客人。
- **I/O APIC** 是公司的**總機**：外面的線路（中斷線）都接到它，它照一張轉接表決定「這條線響了，轉給哪一個員工、說哪個代號」。
- **local APIC** 是每個 CPU 桌上的**電話**：響了就叫 CPU 放下手邊的事去處理；處理完 CPU 要按一下「掛斷」（EOI），電話才會再響下一通。
- **向量（vector）**是代號 0–255，CPU 拿它查 **IDT**（員工手冊）找到該做什麼。
- **8259** 是舊公司留下的老總機，我們把它的線全部拔掉（遮蔽），免得兩台總機同時響。
- **ACPI MADT** 是大樓的**線路圖**：告訴我們總機在哪個位址、有幾台、哪條線其實接到哪個孔。

## 2. 名詞

| 名詞 | 意思 |
|---|---|
| 中斷（interrupt） | 硬體要求 CPU 立刻處理某件事；CPU 存好現場，跳去處理程式，處理完回來 |
| IF | RFLAGS 的 bit 9。IF=0 時 CPU 不接受一般中斷（`cli` 設 0、`sti` 設 1） |
| 向量 | 0–255 的編號；0–31 是 CPU 例外，其他給中斷 |
| IDT | 256 個「門」（gate），每個門記錄向量對應的處理程式位址 |
| PIC／8259 | 1981 年 IBM PC 的中斷控制器，兩顆串接共 16 條線（IRQ 0–15） |
| APIC | Advanced PIC。每顆 CPU 一個 **local APIC**，主機板上有一或多個 **I/O APIC** |
| IRQ | 傳統 ISA 中斷線編號 0–15（0 = PIT、1 = 鍵盤…） |
| GSI | Global System Interrupt，I/O APIC 的輸入腳編號（ACPI 的說法） |
| EOI | End Of Interrupt：告訴 local APIC「這個中斷處理完了」 |
| MMIO | Memory-Mapped I/O：裝置的暫存器出現在某個實體位址，用讀寫記憶體的指令操作 |
| MSR | Model-Specific Register，用 `rdmsr`／`wrmsr` 存取的 CPU 設定暫存器 |
| BSP／AP | 開機時執行的那顆 CPU（bootstrap processor）／其他 CPU（application processor） |

## 3. 開機順序

`frame::init`（`frame/src/lib.rs`）：

```
serial → GDT/TSS/IDT（256 個 gate）→ PMM → 分頁 → heap → MMIO → 畫面
  → ACPI：找 MADT
  → irq::init：8259 remap + 全遮蔽 → local APIC → per-CPU（GS_BASE）→ I/O APIC 全遮蔽
  → input::init：i8042 鍵盤，IRQ 1 → 向量 33
  → time::init：PIT 校準 → APIC timer 1000 Hz（向量 48）
  → sti（開中斷）
```

QEMU（q35、OVMF）實際輸出：

```
acpi: local APIC 0xfee00000, 1 processor(s), 1 I/O APIC(s), 5 override(s)
ioapic: 0 at 0xfec00000, GSI 0..24
i8042: PS/2 keyboard on IRQ 1 (vector 33)
time: APIC timer 62.513 MHz (/16), 1000 Hz; TSC 3696 MHz (PIT, not invariant)
STELA kernel 0.1.0
```

## 4. ACPI：找到 MADT

### 4.1 表的鏈（ACPI 6.5 §5.2.5–5.2.8）

韌體把很多描述硬體的「表」留在記憶體裡。loader 只交給 kernel 一個起點：RSDP 的實體位址（`BootInfo::rsdp_phys`）。

```
RSDP（"RSD PTR "）
 ├─ revision ≥ 2 → XSDT（項目 8 bytes）
 └─ revision 0   → RSDT（項目 4 bytes）
                     ├─ "FACP"
                     ├─ "APIC"  ← MADT，STELA 只解析這一張
                     ├─ "HPET"
                     └─ …
```

每張表的前 36 bytes 是共同的 header：簽名（4）、長度（4）、版本、**checksum**……。checksum 的規則：整張表（`length` 個 bytes）加起來模 256 必須是 0。RSDP 有兩個 checksum：前 20 bytes 一個（ACPI 1.0 的部分），整個 36 bytes 一個（extended）。

### 4.2 怎麼讀

- 純邏輯在 `frame/src/acpi/tables.rs`：給它一段 `&[u8]`，回傳結構或錯誤（`BadSignature`、`BadChecksum`、`BadLength`、`BadEntry`）。host 測試用手工合成的表（含 checksum 錯誤、長度不合、截斷的項目、長度 0 的項目——那會讓迴圈永遠不前進）。
- target 端（`acpi/mod.rs`）透過 HHDM 讀實體記憶體：先讀 36 bytes 的 header 拿到 `length`，`length` 不在 36..=1 MiB 就當壞表，再切出整張表檢查。
- 安全性：拒絕 HHDM 之外、以及與 Usable／KernelAndBoot 重疊的範圍（那些記憶體 Rust 可能正在寫，建立 `&[u8]` 會和可變參考重疊，是 UB）。ACPI 表在 `AcpiReclaimable`／`AcpiNvs`，STELA 不會寫。

### 4.3 MADT 的內容（§5.2.12）

```
offset 36  local APIC 位址（u32）
offset 40  flags：bit 0 PCAT_COMPAT = 也有兩顆 8259
offset 44  項目：[type][length][…]，一個接一個
```

| type | 名稱 | STELA 存的欄位 |
|---|---|---|
| 0 | Processor Local APIC | ACPI 處理器 UID、APIC id、enabled、online capable |
| 1 | I/O APIC | id、位址、GSI base |
| 2 | Interrupt Source Override | ISA IRQ → GSI、極性、觸發方式 |
| 4 | Local APIC NMI | 哪顆 CPU（0xFF = 全部）、LINT0/1、flags |
| 5 | Local APIC Address Override | 64 位元的 local APIC 位址（取代 offset 36） |
| 其他 | | 略過 |

QEMU q35 的 MADT：1 個處理器（APIC id 0）、I/O APIC 0 在 `0xFEC0_0000`（GSI 0–23）、override：

```
IRQ 0  → GSI 2   (ActiveHigh, Edge)    ← PIT 接在 I/O APIC 的第 2 腳，幾乎所有 PC 都這樣
IRQ 5  → GSI 5   (ActiveHigh, Level)
IRQ 9  → GSI 9   (ActiveHigh, Level)
IRQ 10 → GSI 10  (ActiveHigh, Level)
IRQ 11 → GSI 11  (ActiveHigh, Level)
```

沒有 override 的 ISA IRQ：GSI = IRQ、active high、edge（`Madt::isa_irq`）。override 的 flags（MPS INTI flags）：bits 0–1 極性（00 = 跟匯流排 = ISA 的 high、01 high、11 low），bits 2–3 觸發（00 = 跟匯流排 = ISA 的 edge、01 edge、11 level）。

找不到 MADT → panic（STELA 不做 8259 退路，T3）。

## 5. 關掉 8259（OSDev "8259 PIC"）

為什麼不直接遮蔽就好？8259 即使全部遮蔽，偶爾還是會送出「假的」IRQ 7／15。PC 開機時 IRQ 0–7 的預設向量是 8–15，正好是 CPU 例外（8 = double fault）。所以先 **remap**：

```
ICW1 0x11 → 0x20、0xA0      開始初始化，之後會有 ICW4
ICW2 0x20 → 0x21            主片 IRQ 0–7  → 向量 32–39
     0x28 → 0xA1            從片 IRQ 8–15 → 向量 40–47
ICW3 0x04 → 0x21            從片接在主片的 IRQ 2
     0x02 → 0xA1            從片的身分 = 2
ICW4 0x01                   8086 模式
OCW1 0xFF → 0x21、0xA1      全部遮蔽
```

每個寫入之間寫一次 port `0x80`（約 1 µs 的延遲，給老晶片時間）。另外 local APIC 的 LINT0（8259 接到 CPU 的那支腳）也遮蔽，所以 8259 的訊號根本到不了 CPU。selftest 檢查兩個遮蔽暫存器讀回 `0xFFFF`。

## 6. local APIC（Intel SDM Vol. 3A "Advanced Programmable Interrupt Controller (APIC)"）

### 6.1 位址與模式

- 每顆 CPU 都在**同一個實體位址**（預設 `0xFEE0_0000`）看到**自己的** local APIC。
- 位址與開關在 MSR `IA32_APIC_BASE`（`0x1B`）：bit 11 = 啟用，bit 10 = x2APIC 模式，bits 12+ = 位址。
- STELA 只用 **xAPIC**（MMIO 存取）。如果韌體開了 x2APIC，要先關掉 APIC 再以 xAPIC 打開（Vol. 3A "x2APIC State Transitions" 規定 x2APIC 不能直接回 xAPIC）。
- 先檢查 `CPUID.01H:EDX[9]`（有 APIC），沒有就 panic。
- MADT 的位址和 MSR 不同時，以 MSR 為準（CPU 實際用的）並印警告。

### 6.2 暫存器與 STELA 的設定

```
offset  暫存器                      STELA 設成
0x020   ID（bits 24–31）           （讀）BSP = 0
0x030   version（bits 16–23：最大 LVT 編號）
0x080   TPR 工作優先權              0：接受所有向量
0x0B0   EOI                         處理完一個中斷就寫 0
0x0F0   SVR spurious vector         0x1FF：bit 8 軟體啟用 + 向量 0xFF
0x280   ESR 錯誤狀態                寫兩次 0 清除
0x300   ICR 低／0x310 高            送 IPI（selftest 的 self-IPI）
0x320   LVT timer                   先遮蔽，time.rs 再設
0x330   LVT thermal、0x340 perf     遮蔽（version 說有才寫）
0x350   LVT LINT0、0x360 LINT1      遮蔽（8259 與 NMI 腳）
0x370   LVT error                   遮蔽
```

LVT = Local Vector Table，local APIC 自己的中斷來源（計時器、溫度、兩支外部腳…）；bit 16 = 遮蔽。順序有講究：**先寫 SVR 啟用**，因為 APIC 在軟體停用時 LVT 的遮蔽位元改不動。

### 6.3 EOI 與 spurious

- 每個中斷處理完都要寫 EOI，local APIC 才會送同一或更低優先權的下一個中斷。`irq::dispatch` 在 handler 回來後代為寫入，handler 不用管。
- **spurious interrupt**（向量 0xFF）：中斷在 CPU 接受的瞬間消失時，APIC 送這個代號。它不算「進行中」，所以**不能** EOI（Vol. 3A "Spurious Interrupt"）；`dispatch` 只計數就返回。

## 7. MMIO 映射（`frame::mm::mmio`）

APIC 的暫存器不是 RAM。HHDM 用 write-back 快取映射，對暫存器是錯的：讀可能拿到快取裡的舊值，寫可能被延後或合併。所以另外映射：

```rust
let regs: MmioRegion = mmio::map(0xFEE0_0000, 4096)?;
regs.write_u32(0x0B0, 0);           // EOI
let id = regs.read_u32(0x020) >> 24;
```

- 虛擬位址在動態區的**上半部**（MMIO 視窗，`0xFFFF_FF40_0000_0000` 起，256 GiB），用 bump 指標往後配，**不回收**（裝置在開機時映射一次）。
- 分頁項目設 **PCD + PWT**（加上開機預設的 PAT，選到 PAT 第 3 項 = UC，完全不快取）、NX、可寫。
- `MmioRegion` 和 `PhysFrame` 是不同的型別：裝置記憶體不是頁框，永遠不會還給 PMM。`paging::map/unmap` 拒絕 MMIO 視窗，所以 safe code 不能把 MMIO 頁當頁框拿走。
- `read_u32`／`write_u32` 用 `read_volatile`／`write_volatile`（編譯器不能省略或合併），offset 不是 4 的倍數或超出範圍就 panic。
- 與記憶體表中 RAM（Usable、KernelAndBoot、ACPI 兩種）重疊的範圍會被拒絕，否則 safe code 能繞過 Rust 的所有權寫 RAM。
- 實際位址：framebuffer 的測試映射在 `0xFFFF_FF40_0000_0000`、local APIC、I/O APIC 依序在後面。

## 8. I/O APIC（Intel 82093AA datasheet §3.0）

### 8.1 間接存取

I/O APIC 只露出兩個暫存器：寫索引到 **IOREGSEL**（offset 0x00），再讀寫 **IOWIN**（offset 0x10）。「選 + 讀寫」是兩個動作，所以每個 I/O APIC 用一把 `SpinLock` 保護。

```
索引 0x01        version：bits 16–23 = 最後一個輸入的編號（QEMU：23 → 24 個輸入）
索引 0x10 + 2n   第 n 個輸入的 redirection entry 低 32 位元
索引 0x11 + 2n   高 32 位元
```

### 8.2 Redirection entry

```
低  bits 0–7   向量
    bits 8–10  遞送模式（000 = fixed）
    bit 11     目的模式（0 = physical：用 APIC id）
    bit 13     極性（1 = active low）
    bit 15     觸發（1 = level）
    bit 16     遮蔽
高  bits 24–31 目的 APIC id
```

`init` 把每個輸入都設成遮蔽（韌體可能留下開著的）。`irq::enable_isa(irq)`：查 MADT 得到 GSI／極性／觸發 → 找到有那個 GSI 的 I/O APIC → 先寫高半（目的 = 目前 CPU 的 APIC id），再寫低半（向量 32 + irq，同時解除遮蔽）。QEMU 裡鍵盤的項目讀回 `0x21`（向量 33、edge、active high、沒遮蔽）。

## 9. 向量配置與 `irq::register`

```
向量      用途
0–31      CPU 例外（P1；breakpoint 返回，其餘 panic）
32–47     傳統 ISA IRQ 0–15（經 I/O APIC）：IRQ n → 32 + n；目前只有 33（鍵盤）
48        local APIC timer
49        保留給 IPI（之後 SMP）
50–254    給驅動（FIRST_FREE_VECTOR = 50）
255       spurious（不 EOI）
```

### 9.1 IDT 與 stub

P1 只有 0–31 有 gate；P2 起 256 個全部存在。32–255 的 stub 用組語 `.rept 224` 產生，每個放在 16 bytes 對齊的位置：

```
stela_irq_stubs + (n − 32) × 16:
    push 0          ; 沒有 error code，補 0（2 bytes）
    push n          ; 向量（n < 128：2 bytes；n ≥ 128：5 bytes，否則會被當成負數）
    jmp  common     ; 和例外共用（5 bytes）
```

最長 12 bytes ≤ 16，所以 Rust 端可以直接用 `起點 + (n − 32) × 16` 算出每個 stub 的位址；`stubs()` 在開機時檢查總長度剛好 224 × 16（最後一個也要補齊，第一次寫時就是漏了這個）。common stub 存好 15 個暫存器、組成 `TrapFrame`，呼叫 `trap_dispatch`：向量 ≥ 32 交給 `irq::dispatch`，否則照 P1 的例外處理。所有 gate 都是 interrupt gate，進入時 CPU 自動清 IF，所以 handler 執行時中斷是關的。

### 9.2 註冊與分派

```rust
fn on_key(frame: &TrapFrame) { … }                // 一般的 fn，不是 closure
irq::register(irq::isa_vector(1), on_key)?;       // 32–254；例外或 0xFF → Reserved，已有 → Busy
irq::enable_isa(1)?;                              // I/O APIC 路由並解除遮蔽
```

`dispatch(frame)`：計數 → 0xFF 就返回 → 從表（`SpinLock<[Option<fn(&TrapFrame)>; 256]>`）複製出 handler、放鎖 → 呼叫 → **EOI**。沒有 handler 的向量印一次 `irq: vector N has no handler (reported once)` 後照樣 EOI，不 panic。

`irq::self_ipi(vector)`：用 local APIC 的 ICR 送中斷給自己（destination shorthand = self、fixed、assert）。selftest 用它測整條路：handler 被叫到、`frame.vector` 正確、handler 內 IF=0、EOI 後下一個能進來。

## 10. per-CPU（`frame::cpu`）

每顆 CPU 需要自己的資料（現在：編號、APIC id、tick 數；之後：目前的執行緒、排程佇列）。x86-64 的慣例是用 **GS 段的 base**：

```
IA32_GS_BASE（MSR 0xC000_0101）──► PerCpu {
                                      self_ptr ──┐  ← 第一個欄位存自己的位址
                                      id          │
                                      apic_id     │
                                      ticks       │
                                    } ◄──────────┘
frame::cpu::current()：mov rax, gs:[0]  → &'static PerCpu
```

- 一條 `mov rax, gs:[0]` 就拿到一般指標，不用每次 `rdmsr`（慢）。
- 現在只有 BSP 一份（heap 配置後 leak）；`init_this_cpu(id, apic_id)` 是每顆 CPU 自己呼叫的設計，P10 啟動 AP 時各建一份。
- P3 有使用者模式時要加 `swapgs` 與 `IA32_KERNEL_GS_BASE`（使用者程式也能設 GS）。
- selftest：`current().apic_id()` 等於 `CPUID.01H:EBX[31:24]`（CPU 自己回報的初始 APIC id），`id() == 0`、`online() == 1`。

## 11. 鎖與中斷

### 11.1 為什麼持鎖要關中斷

單顆 CPU 也會死結：

```
主迴圈：input::pop() 拿到 QUEUE 的鎖
   ── 鍵盤中斷 ──► handler：input::push() 想拿 QUEUE 的鎖 → 等主迴圈放鎖
                    但主迴圈被這個 handler 打斷了，永遠不會繼續 → 卡死
```

所以 `SpinLock::lock()`：

```
1. 記下 IF（pushfq）
2. cli
3. 搶鎖（compare_exchange）
   …持鎖期間中斷關著…
4. guard drop：放鎖 → 若第 1 步 IF=1 則 sti
```

順序重要：先關中斷再搶鎖（否則搶到之後、關中斷之前可能被打斷）；先放鎖再開中斷（`SpinLockGuard` 的 `Drop::drop` 放鎖，之後欄位 `_irq: IrqGuard` 才被 drop 而開中斷）。`try_lock` 失敗時中斷狀態恢復原樣。

`IrqGuard::new()` 只做中斷那一半（存 IF、`cli`，drop 時還原）；巢狀時只有最外層會 `sti`。它不是 `Send`（中斷狀態屬於建立它的 CPU）。host 上沒有中斷，這兩個是 no-op，host 測試照舊。

### 11.2 其他規則

- **中斷處理程式**跑的時候 IF=0（interrupt gate），要短；不能等待（不能 `sleep_ms`）。
- **主控台**：`kprint!` 在中斷裡也能用（serial 沒有鎖；畫面用 `try_lock`，拿不到就略過畫面）。
- **`Once<T>`**（`frame::sync`）：開機時設一次、之後只讀，不需要鎖（local APIC 的暫存器、I/O APIC 清單、TSC 時鐘）。用一個 `AtomicU8`（空 → 寫入中 → 完成）與 Release/Acquire 保證讀者看到完整的值。
- `frame::halt_until_interrupt()` 是 `sti; hlt`：`sti` 的效果延後一條指令，所以中斷不會剛好插在兩者之間而讓 `hlt` 睡過頭（Vol. 2B "STI"）。

## 12. 測試

| 測試 | 內容 |
|---|---|
| host `acpi::tables` | RSDP v0/v2、兩種 checksum、長度、XSDT/RSDT 項目、MADT 各型別、未知型別、壞項目、override 的極性／觸發、`isa_irq`、`io_apic_for` |
| host `sync` | `Once`、`IrqGuard` 巢狀、SpinLock |
| host `mm::frames` | `overlaps_ram` |
| selftest "locks and interrupts" | 開中斷時 IrqGuard／SpinLock 讓 IF=0，drop 後還原 |
| selftest "mmio mapping" | framebuffer 第一頁映射兩次（含非對齊）互相讀寫、`translate`、RAM 被拒、MMIO 頁不能 `unmap` |
| selftest "acpi madt" | local APIC 位址非 0、至少一個 I/O APIC、BSP 的 APIC id 在 MADT 裡且 enabled |
| selftest "interrupt controllers" | 8259 遮蔽 `0xFFFF`、per-CPU 的 APIC id = CPUID |
| selftest "interrupt dispatch" | 向量 0x50 self-IPI 三次、IF=0 時 pending 由 `sti; hlt` 接到、未註冊的 0x51、Reserved／Busy、`unregister` |

## 13. 已知限制與待辦

- 只有 BSP；AP 不啟動（P10）。`irq::enable_isa` 把中斷送給呼叫它的 CPU。
- 只支援 xAPIC，x2APIC 只是被關回 xAPIC；沒有 8259 退路（沒有 APIC 或 MADT 就 panic）。
- MMIO 視窗的虛擬位址只增不減，映射不回收。HHDM 仍以 write-back 涵蓋 `0xFEE0_0000` 等裝置位址（同一實體位址兩種快取屬性）；STELA 從不經 HHDM 碰它們，實機的 MTRR 也把這些範圍設成 UC。
- LINT1（NMI）也遮蔽；MADT 的 NMI 項目有解析但沒用。
- 中斷處理程式表用一把 SpinLock；SMP 時每個中斷都要拿它，之後可改成 atomic 指標表。
- 只處理 ISA IRQ；PCI 的 INTx 路由要 ACPI 的 AML（`_PRT`），之後的驅動改用 MSI。
- 不做巢狀中斷優先權（TPR 永遠 0，handler 執行時 IF=0）。
- 持鎖期間中斷是關的：長時間持鎖（例如主控台捲動整個 framebuffer）會延後中斷，可能讓計時器少算 tick（見 `docs/time.md`）。
