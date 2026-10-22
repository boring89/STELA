# SMP：啟動其他處理器、跨 CPU 排程

> P10 由自動排程一次做完，使用者已於 2026-10-21 審核。

## 1. 比喻：事務所多開了幾張辦公桌

P3 的事務所只有一張辦公桌（BSP），每件案子（執行緒）輪流上桌。P10 把其他辦公桌（AP，application processor）也打開。規矩很簡單：

- **一個客戶（行程）的所有案子都在同一張桌子處理**（行程親和）：桌子之間不搬案子，所以每張桌子只看自己的抽屜（per-CPU 的排程器），也不必通知別張桌子「這位客戶的檔案改了」（使用者位址空間的 TLB 只在一顆 CPU 上）。
- 別張桌子要動你的抽屜（叫醒你的一件案子、殺掉一位客戶），要先拿**你那張桌子的鎖**，做完再**按你的鈴**（IPI），讓你回頭看一眼。
- kernel 自己的公文（kernel 執行緒）預設都在第一張桌子。

## 2. 啟動 AP（`frame/src/smp.rs`）

AP 開機時是 real mode，從 Startup IPI 指定的頁開始執行（Intel SDM Vol. 3A §9.4.4 "MP Initialization Example"、§11.6.1 ICR；OSDev Wiki "SMP"）。loader 在 640 KiB 以下保留了 **4 頁**（`BootInfo::ap_trampoline_phys`；K1 原本只保留 1 頁，K15 改成 4 頁）：

```
 頁 0  trampoline：16 位元 → 32 位元（暫時的 GDT、PAE、CR3 = 頁 1、EFER.LME/NXE、分頁）→ 64 位元
       → 換到自己的 kernel 堆疊，跳到 Rust 的 ap_entry；參數放在頁尾
 頁 1  暫時的 PML4：kernel PML4 的 512 項，加上第 0 項 →
 頁 2  PDPT：第 0 項 →
 頁 3  PD：第 0 項 = 前 2 MiB 的 identity map（trampoline 自己）
```

- trampoline 是 `global_asm!`（AT&T 語法，`.code16`／`.code32`／`.code64`），在 kernel 映像裡；BSP 把它複製到頁 0 後**修補**：兩個遠跳的 32 位元目標（頁位址 + 偏移）、32 位元段用的基底（`mov ebx, imm32`）、暫時 GDT 的基底。參數：暫時 CR3、要設的 EFER 位元（LME，BSP 開了 NX 就加 NXE——kernel 的分頁表用 bit 63，沒開 NXE 那是保留位元）、堆疊頂、`ap_entry` 位址、CPU 編號。
- 32 位元段只能載 32 位元的 CR3，所以先用頁 1–3 的暫時表（都在 1 MiB 以下）；有 identity map 的第 0 項，開分頁後下一個指令還找得到；複製來的 kernel 半部讓 64 位元段可以直接跳進 higher half 的 `ap_entry`。
- 一次一顆：**INIT**（ICR delivery mode 101、level assert）→ 10 ms → **SIPI**（110，vector = 頁號）→ 200 µs → 還沒報到就再一次 SIPI → 最多等 100 ms。等待用 `time::now_ns`（所以要在開中斷之後做）。
- `ap_entry`（關中斷）：載入 kernel 的 CR3 → 自己的 GDT、TSS、double-fault 堆疊（`gdt::init_ap`：TSS 描述子 `ltr` 之後是 busy，不能共用）→ IDT（共用）→ PAT（和 BSP 一樣，Vol. 3A §11.12.4）→ local APIC（`apic::init_ap`；暫存器已經由 BSP 映射）→ per-CPU 區（GS_BASE）→ syscall MSR → SMEP → SSE → boot context → APIC timer（用 BSP 的校準值，1000 Hz）→ 報到 → kernel 的 hook。
- kernel 的 hook（`task::ap_main`）：建立這顆 CPU 的排程器（目前的程式碼變成執行緒 `ap-main`，加一個 idle 執行緒），然後 `exit()`：之後這顆 CPU 只有 idle，等工作來。
- 每顆最多 16 顆（`cpu::MAX_CPUS`，排程器的陣列大小）。開機訊息 `smp: 4 processor(s) online`。開機選項 `nosmp` 不啟動 AP。

## 3. per-CPU 的東西

| 東西 | 怎麼做 |
|---|---|
| GDT/TSS | BSP 用 static 的；AP 各自 leak 一份（`PerCpu::tss` 記位址，`set_kernel_stack` 寫自己的 TSS 的 RSP0） |
| per-CPU 區 | `cpu::init_this_cpu(id, apic_id, tss)`；`cpu::apic_id_of(id)`、`cpu::ticks_of(id)` 給別顆 CPU 讀 |
| 時鐘 | 每顆都有 APIC timer；**全域的 tick（`time::ticks`、`now_ns`）只算 BSP 的**，否則 4 顆會讓時間快 4 倍 |
| 排程器 | `CPUS[id]`，每顆一把 SpinLock（P3 起結構就是 per-CPU） |
| IPI | 向量 49（`irq::IPI_VECTOR`）＝ reschedule：handler 只設 `need_resched`，中斷結束時切換（idle 也會被叫醒）；向量 0xFD ＝ TLB shootdown |

## 4. 排程（`kernel/src/task/sched.rs`）

- **行程親和**：`Process::cpu` 在建立時以 round-robin 指定（`next_cpu`：線上 CPU 數取餘數）；`spawn_user` 把執行緒放進那顆的排程器。kernel 執行緒：`spawn` 放 BSP，`spawn_on(cpu, ..)` 指定。執行緒**永遠不搬家**，沒有負載平衡。
- **跨 CPU 的 wake**：`wake(id)` 先找自己這顆，再依序拿別顆的鎖找；在別顆找到並變成 ready 就 `cpu::kick`（送 IPI 49）。找法和 P3 一樣：正在跑（含「已決定要睡但還沒切走」）→ 改回 Running 或記 `wake_pending`；blocked → ready；可喚醒的 sleeping → ready；ready → `wake_pending`。
- **為什麼不需要 Linux 的 `finish_task_switch`**：計畫原本要「切換後由下一個執行緒放鎖」，因為 Linux 的執行緒可能被別顆 CPU 撿走；STELA 的執行緒只會在自己那顆跑，放鎖之後到 `switch_to` 存好 context 之間，就算別顆把它改成 ready，也只有它自己那顆會去跑它——而那顆正在做這個切換。所以 P3 的「放鎖後、關著中斷切換」在 SMP 下仍然正確（**計畫外的決定**，記在 §8）。
- **`spawn_user` 與 `kill` 的競賽**：「行程是否在結束中」的檢查與放進佇列都在目標 CPU 的排程器鎖下做；`kill` 也拿同一把鎖：要嘛 kill 看到新執行緒，要嘛新執行緒看到 kill。
- **跨 CPU 的 kill**：在行程那顆 CPU 的排程器鎖下：睡著／blocked 的改成 ready，全部標 `killed`；**正在跑的**標 `killed`（如果它剛好「決定要睡還沒切走」就改回 Running，讓它看到 kill），然後送 IPI——它從中斷回 ring 3 時在 `user_return` 結束自己。同一顆 CPU 上的行為和 P3 一樣（目前執行緒直接變 zombie，呼叫者要 `schedule`）。
- **kernel 執行緒與 idle 在 kernel 自己的位址空間跑**（`frame::mm::user::activate_kernel`）：P3 讓它們沿用當時的 CR3；SMP 下，行程結束時最後一個參照可能在別顆 CPU 上放掉，若某顆 CPU 還載著它的 PML4 就會用到已釋放的分頁表。切到沒有行程的執行緒就換回 kernel 的 CR3，保證行程跑完最後一個執行緒的那顆 CPU 不再用它的表。

## 5. TLB shootdown（`frame/src/smp.rs`、`frame/src/mm/paging.rs`）

使用者位址空間因為親和性只在一顆 CPU 上載入，`invlpg` 本地就夠。**kernel 半部**（kernel 堆疊、動態區）是共用的：例如 CPU X 建立執行緒時寫過它的堆疊（`TaskContext::new` 寫初始 frame），X 的 TLB 就記著那個位址；執行緒在 Y 跑完、Y 拆掉映射並釋放頁框，之後同一個堆疊槽被重用時 X 可能還用舊的翻譯寫到別人的頁框。所以 `paging::unmap` 在本地 `invlpg`、放掉分頁鎖之後呼叫 `smp::shootdown(virt)`：

```
 拿 SHOOT_LOCK（try_lock 迴圈；等的時候自己先處理目前的請求）
 SHOOT_ADDR = virt；SHOOT_GEN += 1；自己的 ACKED = gen
 廣播 IPI 0xFD（shorthand：all excluding self）
 等每顆線上 CPU 的 ACKED ≥ gen（最多約 5000 萬次 spin，逾時說一次就放棄）
 IPI handler：invlpg(SHOOT_ADDR)，ACKED[me] = gen
```

關鍵：等鎖或等回應的 CPU 可能關著中斷（例如 `schedule` 之後的 `reap` 釋放 kernel 堆疊），收不到 IPI；所以等待迴圈裡**自己處理**目前的請求（`serve_shootdown`），兩顆同時做 shootdown 才不會互等到逾時（K16 的除錯就是卡在這裡）。

## 6. 系統資訊

- `SysInfo(CPUS)`（7）：每顆線上 CPU 一個 u64（它處理過的 timer tick），回傳 CPU 數；libstela `process::cpus()`。
- `SysInfo(PROCESS_CPUS)`（8）：執行中的行程與它的 CPU（成對的 u64），libstela `process::process_cpus()`。

## 6.1 idle：`mwait`（P14 K19，**使用者尚未審核**）

沒事做的 CPU 停在 idle 執行緒（`kernel/src/task/sched.rs` 的 `idle_entry`），呼叫 `frame::halt_until_interrupt`。以前是 `sti; hlt`；現在處理器有 MONITOR/MWAIT 時改成：

```
monitor IDLE_LINE        ; 監看一條沒人寫的 cache line（frame/src/arch/x86_64/mod.rs）
sti                      ; 下一個指令執行完才開中斷（interrupt shadow），中斷不會漏在中間
mwait eax=0, ecx=0       ; C1（EAX[7:4]=0、子狀態 0），中斷或那條線被寫就醒
```

比喻：`hlt` 是坐在椅子上打盹，`mwait` 是同一張椅子但告訴 CPU「我在等這個門鈴」，處理器可以照它知道的方式省電（C1 以上的深睡眠要 ACPI 的 `_CST` 才知道代價，STELA 沒有 AML 直譯器，所以只用 C1）。叫醒的方式和 `hlt` 一樣靠中斷（reschedule IPI、timer），所以排程器不用改。

- **偵測**（`frame::cpuid::mwait_c1`，host 測試）：CPUID.01H:ECX[3]（MONITOR/MWAIT）、最高的基本 leaf ≥ 05H；leaf 05H 若列舉了擴充（ECX[0]），EDX[7:4]（C1 的子狀態數）要不是 0——和 Linux 的 `mwait_usable` 同一條規則（出處：Intel SDM Vol. 2A CPUID leaf 05H、Vol. 2B MONITOR／MWAIT）。開機時 BSP 決定、所有 CPU 一樣，serial 印 `idle: mwait` 或 `idle: hlt`。
- **開機選項** `idle=hlt`：不用 mwait（實機上萬一有問題）；`idle=mwait`：只要有 MONITOR 就用（QEMU 的 TCG 在 leaf 05H 報 EDX=0，照規則會選 hlt，測試需要它）。
- **TCG 的行為**：多顆 CPU 時 QEMU 的 `mwait` 什麼都不做（立刻返回），idle 迴圈變成空轉——正確但耗 host 的 CPU；只有 `make test-qemu-smp` 打開它。selftest 的 "timer tick" 原本假設「每次 halt 至少等到下一個中斷」，`mwait` 可能提早醒，所以改成次數上限放寬、只看 tick 有沒有增加。
- 量測：TCG 下看不出省電；實機的差別要在 P520 上量（未驗證）。

## 7. 測試

| 測試 | 內容 |
|---|---|
| selftest "interrupt controllers" | 線上 CPU 數 = MADT 裡 enabled 的處理器數（`nosmp` 時 1） |
| selftest "smp"（只有多顆時） | 每顆的 tick 50 ms 內增加；CPU 1 上的執行緒（`spawn_on`）叫醒 BSP 的主執行緒；主執行緒叫醒在 CPU 1 上 block 的執行緒；每顆一個 spinner 行程（`jmp $`）、round-robin 分到不同 CPU、主執行緒沒被餓到；從 BSP 殺掉全部，`wait_process` 都是 99 |
| `make test-qemu-smp` | selftest 全套在 `-smp 4`（所有行程、IPC、Linux、檔案系統的測試都分散在 4 顆上跑）；P14 起 `-cpu qemu64,+monitor` + `idle=mwait`：四個 idle 都在 `mwait`（`idle: mwait`） |
| `make test-shell` | 改成 `-smp 2`：桌面、terminal、shell 與程式分散在兩顆上 |
| `make test-qemu-min` | 維持單核（Conroe） |

selftest 裡檢查「行程結束後記憶體都回來了」的地方改成等最多 200 ms（`settled_frames`）：`wait_process` 回報結束時，行程的位址空間可能還在別顆 CPU 上釋放中。

## 8. 計畫外的決定

- loader 保留 4 頁（不是 1 頁），放暫時的分頁表。
- 沒有做 `finish_task_switch` 式的放鎖（§4 的理由）。
- kernel 執行緒與 idle 改在 kernel 的位址空間跑。
- shootdown 用「指定位址 + 世代計數 + 等待時自己處理」，不 reload CR3（kernel 頁可能是 global，reload CR3 清不掉）。
- 新的 `SysInfo(PROCESS_CPUS)` 而不是改 `ProcessInfo` 的格式（那會改 ABI）。

## 9. 已知限制

- 沒有執行緒遷移、負載平衡、per-CPU heap、CPU 熱插拔；行程在建立時就決定 CPU（round-robin，不看負載）。
- 裝置中斷（I/O APIC、MSI）都送到 BSP；kernel 執行緒都在 BSP。
- NMI 遮蔽（LINT1）；NMI／#MC 沒有 IST（`docs/process.md` §14）。
- shootdown 逾時時會放棄（印一次警告）而不是死等；只發生在某顆 CPU 長時間關著中斷又不在 shootdown 的等待迴圈裡時。
- 只支援 xAPIC（8 位元 APIC id，最多 16 顆）；實機（P520 的 6 核 12 執行緒）沒有驗證。
