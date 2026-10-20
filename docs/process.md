# 行程、執行緒與使用者模式

> P3 由自動排程一次做完，**使用者尚未審核**；之後修改前請先帶使用者過一遍這份文件。

這份文件說明 STELA 怎麼讓「很多件事輪流做」：kernel 執行緒、排程與搶佔、context 切換、每個行程自己的位址空間、進出 ring 3、使用者程式出錯時怎麼收拾，以及第一個使用者程式怎麼從 initrd 載入。syscall 的 ABI 與 libstela 見 `docs/syscall.md`。

程式碼：

```
frame/src/task/mod.rs           TaskContext、switch_to、enter_user、搶佔與使用者例外的回呼
frame/src/task/stack.rs         kernel 堆疊（動態區的 slot，下面有 guard）
frame/src/arch/x86_64/context.rs  switch 的組語、trampoline、FXSAVE、enter_user 的 iretq
frame/src/arch/x86_64/gdt.rs    user 段、TSS RSP0
frame/src/arch/x86_64/exceptions.rs  來自 ring 3 時 swapgs、交給 user_fault
frame/src/mm/user.rs            AddressSpace（每行程的 PML4）
frame/src/mm/mod.rs             initrd()
kernel/src/task/                Thread、排程器、睡眠清單、Process、wait_process
kernel/src/initrd.rs            ustar 解析（host 測試）
kernel/src/program.rs           從 initrd 載入 ELF、建立行程
boot/src/main.rs                load_initrd
```

## 1. 比喻：一間只有一張辦公桌的事務所

CPU 是**一張辦公桌**，執行緒是**幾個輪流用桌子的員工**。

- 每個員工有自己的**資料夾**（kernel 堆疊與存好的暫存器 = TCB 裡的 context）。換人時，前一個人把桌上的東西收進他的資料夾，下一個人把他的資料夾攤開——這就是 **context 切換**。
- **排班表**（run queue）決定下一個輪到誰；每人一次最多用 10 分鐘（**時間片** 10 ms）。時間到了，**鬧鐘**（APIC timer）響，就算他不想讓也得讓——這是**搶佔**。
- 有人說「我 30 分鐘後再來」就去**休息室**（睡眠清單），時間到了回到排班表尾端。
- 沒人要用桌子時，**值班的**（idle 執行緒）坐在那裡打盹（`hlt`），有人來就讓位。
- 員工屬於不同的**客戶**（行程）。每個客戶有自己的**檔案櫃**（位址空間）；換到另一個客戶的員工時，要換一組檔案櫃（寫 CR3）。事務所自己的文件（kernel）在每個檔案櫃的上半部都一樣。
- 客戶（使用者程式）只能在**櫃台外**（ring 3）活動，要辦事得按鈴（`syscall`）。在櫃台外打破東西（例外）只會被請出去（行程被殺），事務所照常營業。
- 員工離職（執行緒結束）時不能自己把自己的桌子搬走——他還坐在上面。所以由**下一個坐上來的人**清掉他的東西（Zombie 回收）。

## 2. 名詞

| 名詞 | 意思 |
|---|---|
| 執行緒（thread） | 一條獨立的執行流：自己的堆疊與暫存器。STELA 是 1:1：每個使用者執行緒對應一個 kernel 執行緒（同一個 TCB、同一個 kernel 堆疊） |
| 行程（process） | 一個位址空間 + 在裡面跑的執行緒 |
| TCB | Thread Control Block，kernel 記錄一個執行緒的結構（`kernel::task::Thread`） |
| context | 執行緒停下來時要保存的 CPU 狀態：callee-saved 暫存器、RSP，使用者執行緒再加 FPU/SSE |
| 搶佔（preemption） | 不等執行緒自己讓出，由計時器中斷強迫換人 |
| ring 0／ring 3 | CPU 的特權等級：kernel 在 0，使用者程式在 3；ring 3 不能執行特權指令、不能碰 U/S=0 的頁 |
| TSS RSP0 | ring 3 被中斷時，CPU 自動換到的 kernel 堆疊頂 |
| `swapgs` | 交換 GS_BASE 與 IA32_KERNEL_GS_BASE；進出 kernel 時用它找回 per-CPU 區 |
| Zombie | 已結束、但堆疊與 TCB 還沒釋放的執行緒 |
| initrd | initial RAM disk：loader 讀進記憶體的一個 ustar 封存，裡面是第一批使用者程式 |

## 3. 分層：誰管什麼

```
 kernel（safe Rust，政策）                    frame（unsafe，機制）
 ┌─────────────────────────────────┐         ┌──────────────────────────────────────┐
 │ Thread / Scheduler / Process    │         │ TaskContext（rsp、kernel 堆疊、FXSAVE 區）│
 │ run queue、睡眠清單、blocked     │ switch_to│ switch_to：存 → 換堆疊 → 載入          │
 │ schedule()：選下一個             ├────────►│ thread_start（trampoline 之後）        │
 │ tick()：時間片、喚醒睡眠者        │◄────────┤ time::set_tick_hook                  │
 │ preempt()                       │◄────────┤ irq::dispatch：EOI 後看 need_resched  │
 │ user_fault()：殺行程             │◄────────┤ 例外來自 ring 3                       │
 │ syscall::handle                 │◄────────┤ syscall 進入點                        │
 │ program::spawn：ELF → 位址空間    ├────────►│ AddressSpace（map、load、activate）    │
 └─────────────────────────────────┘ enter_user└──────────────────────────────────────┘
```

frame 不知道「下一個該誰」，kernel 不碰暫存器與分頁表。kernel 需要被 frame 叫到的地方（tick、搶佔、使用者例外、syscall）都用「註冊回呼」：`frame::time::set_tick_hook`、`frame::task::set_scheduler`、`frame::task::set_user_fault_handler`、`frame::syscall::set_handler`，和 `irq::register` 一樣是一般的 `fn`。

## 4. 執行緒與 TCB

### 4.1 `Thread`（`kernel/src/task/sched.rs`）

| 欄位 | 意思 |
|---|---|
| `id` | 全域遞增的 u64（`AtomicU64`），不重用；行程 id 另一組 |
| `name` | `"main"`、`"idle"`、`"echo"`、`"user"`… |
| `state` | `Ready`／`Running`／`Sleeping(wake_at)`／`Blocked`／`Zombie` |
| `priority` | `u8`，存著但沒有政策（單一等級 round-robin） |
| `slice` | 這次時間片還剩幾個 tick（從 10 開始） |
| `wake_pending` | 還沒 `block()` 就先被 `wake()` 了（見 §6.3） |
| `context` | `Arc<frame::task::TaskContext>` |
| `start` | kernel 執行緒要跑的閉包（第一次執行時取出） |
| `user_start` | 使用者執行緒的 (entry, rsp, arg) |
| `process` | `Option<Arc<Process>>`，kernel 執行緒是 `None` |

每顆 CPU 一個 `Scheduler`（`CPUS[cpu id]`，現在只有 BSP 用）：`current`、`ready`（RunQueue）、`sleeping`（SleepList）、`blocked`、`idle`、`zombies`。TCB 全部是 `Box<Thread>`，在容器之間搬只是搬一個指標。

```
            spawn / wake / 睡醒 / 被搶佔 / yield
                 │
                 ▼
   ┌────────► ready ──── schedule() 取最前面 ───► running
   │                                              │ │ │
   │   sleeping ◄──── sleep_ns ──────────────────┘ │ │
   │   blocked  ◄──── block ────────────────────────┘ │
   │   zombie   ◄──── exit ───────────────────────────┘
   │      └──► 下一個執行緒在切換之後釋放
   └── idle：沒有 ready 時才跑，永遠不進 ready
```

### 4.2 `TaskContext`（`frame/src/task/mod.rs`）

```rust
pub struct TaskContext {
    rsp: UnsafeCell<u64>,                     // 暫停時的 RSP
    stack: Option<KernelStack>,               // boot 執行緒是 None（用 loader 的堆疊）
    fpu: Option<Box<UnsafeCell<FxArea>>>,     // 使用者執行緒才有：512 bytes、16 對齊
}
```

它用 `Arc` 共用：kernel 的 TCB 拿一份，**per-CPU 區的 `current_task` 也拿一份**。所以「正在跑的那個執行緒的堆疊」不可能被釋放，不管 kernel 對自己那份做了什麼——這是 Zombie 回收在記憶體安全上的保證（§8）。

### 4.3 kernel 堆疊（`frame/src/task/stack.rs`）

```
 0xFFFF_FF10_0000_0000 + slot × 64 KiB      （動態區 + 64 GiB）
 ┌──────────────────────┬──────────────────────┐
 │ guard 32 KiB（不映射） │ 堆疊 32 KiB（8 個頁框）│ ◄── top（初始 RSP）
 └──────────────────────┴──────────────────────┘
```

32 KiB 從 PMM 拿 8 個頁框一頁一頁映射；下面 32 KiB 不映射當 guard：堆疊溢位會 page fault，而 page fault 推不進同一個堆疊，變成 double fault，在 IST1 的堆疊上印出 panic。slot 最多 16384 個（1 GiB 位址空間），釋放的 slot 重用。

## 5. context 切換

### 5.1 `switch_to(next)` 的步驟

```
 1. assert 中斷關著（切到一半被中斷，兩個 context 都會壞）
 2. 從 per-CPU 拿出 prev（目前的 Arc）；prev == next 就放回去、返回
 3. prev 有 FPU 區 → fxsave；next 有 → fxrstor
 4. next 有自己的堆疊 → TSS.RSP0 = per-CPU.kernel_rsp = 它的堆疊頂
 5. per-CPU.current_task = next；per-CPU.previous_task = prev（讓 prev 的堆疊活到切完）
 6. stela_switch_context(&prev.rsp, next.rsp)
 7. （在 next 的堆疊上）drop per-CPU.previous_task
```

### 5.2 組語（`arch/x86_64/context.rs`）

System V ABI 規定 rbx、rbp、r12–r15 是 callee-saved：被呼叫的函式要保持它們不變；其他暫存器呼叫者本來就假設會被弄亂。所以切換只要存這 6 個加 RSP：

```asm
stela_switch_context:        ; rdi = &prev.rsp，rsi = next 的 rsp
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    mov [rdi], rsp           ; 存起來
    mov rsp, rsi             ; 換堆疊——從這裡起就是 next 了
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    ret                      ; 回到 next 上次呼叫 switch 的地方
```

暫停中的堆疊（低位址在上）：

```
 saved rsp ─► r15 r14 r13 r12 rbx rbp  返回位址
              └──────── pop ────────┘ └─ ret 從這裡繼續
```

### 5.3 新執行緒的第一次

`TaskContext::new(entry, arg, fpu)` 在新堆疊頂端放一個「假裝曾經切走過」的框架：

```
 top − 64: r15=0 r14=0 r13=arg r12=entry rbx=0 rbp=0  ret=stela_thread_trampoline  0
```

第一次被切到時，`pop` 把 entry 放進 r12、arg 放進 r13，`ret` 跳到 trampoline：

```asm
stela_thread_trampoline:
    xor ebp, ebp             ; 堆疊回溯到此為止
    mov rdi, r12
    mov rsi, r13
    and rsp, -16             ; call 時 RSP 要 16 對齊
    call thread_start        ; frame：drop previous_task → 開中斷 → entry(arg)
    ud2
```

kernel 的 entry 一律是 `thread_entry`：先 `reap()`，然後若是 kernel 執行緒就取出閉包執行、最後 `exit()`；若是使用者執行緒就 `enter_user(entry, rsp, arg)`。kernel 不能用 unsafe 把整數轉回閉包，所以閉包存在 TCB 的 `start`，`thread_entry` 從「目前的執行緒」取出來。

### 5.4 FPU/SSE

kernel 的編譯目標 `x86_64-unknown-none` 是 soft-float，**kernel 自己從不碰 x87/SSE 暫存器**，所以：

- `frame::init` 設 CR0.MP=1、EM=0、TS=0，CR4.OSFXSR（bit 9）、OSXMMEXCPT（bit 10）（Intel SDM Vol. 3A §2.5、§13.1.3）：使用者程式可以用 SSE，SIMD 例外報成 #XM。
- 只有使用者執行緒有 FXSAVE 區（`TaskContext::new(.., fpu: true)`），切換時 eager 存／載（Vol. 1 §10.5）。kernel 執行緒沒有：它們跑的時候 FPU 裡留著上一個使用者執行緒的狀態，不會被弄亂。
- 新區域的初值 = FNINIT 之後：FCW = 0x037F（offset 0）、MXCSR = 0x1F80（offset 24，全部 SSE 例外遮蔽）。
- 不做 XSAVE/AVX（硬體基準是 SSE2）。

## 6. 排程

### 6.1 `schedule()`

```
 IrqGuard（關中斷）
 拿 per-CPU 排程器的鎖：
   prev = current.take()，依 prev.state 放回：
     Running/Ready → ready 尾（idle 則放回 idle 槽）
     Sleeping(t)   → sleeping（依 t 排序）
     Blocked       → blocked
     Zombie        → zombies
   next = ready.pop() 或 idle
   next.state = Running、slice = 10
   next 是使用者執行緒 → next.process.space().activate()（CR3 不同才寫）
 放鎖
 frame::task::switch_to(next.context)
 （回來之後）reap()
```

呼叫點：`yield_now`、`sleep_ns`、`block`、`exit`、`exit_process`、使用者例外，以及搶佔（§6.2）。切換本身在放鎖之後、中斷仍關著時做；單顆 CPU 上這樣就沒有別的東西能插進來。

kernel 執行緒不寫 CR3，沿用上一個行程的位址空間——kernel 半部在每個位址空間都一樣，所以沒差；只是那個位址空間被釋放時要先換掉（§7.4）。

### 6.2 時間片與搶佔

```
 APIC timer（1 ms） → irq::dispatch → timer_interrupt
                                       ticks += 1
                                       tick hook = kernel::task::tick()：
                                         睡眠清單裡到期的 → ready
                                         current 是 idle 且 ready 非空 → need_resched
                                         否則 slice −1；到 0 → 重設 10，ready 非空 → need_resched
                    → EOI
                    → per-CPU.need_resched？ → 清掉 → frame::task::preempt() → kernel schedule()
```

`need_resched` 在 **EOI 之後**才看：local APIC 已經可以送下一個中斷，被切走的執行緒的中斷框架（`TrapFrame`）留在它自己的 kernel 堆疊上，等它下次被切回來，`schedule()` 返回、`dispatch` 返回、stub `iretq`，就從被打斷的地方繼續。

誰會被搶佔：中斷開著的所有程式碼——使用者程式、kernel 執行緒、syscall 處理中（`frame::syscall::dispatch` 一進來就開中斷）。`SpinLock` 持鎖期間中斷是關的，所以**持鎖時不會被搶佔**，也不會帶著鎖切換。

QEMU 實測：一個從不讓出的 spinner 旁邊，主執行緒 `sleep_ms(10)` 5 次都在 100 tick 內回來；主執行緒自己忙 30 ms 不讓出，spinner 也還是有跑。

### 6.3 睡眠、阻塞、喚醒

- `sleep_ns(ns)`：`wake_at = now_ns() + ns`（沒有 invariant TSC 時 `now_ns` 是整數 tick，可能比實際時間慢最多 1 tick，所以再加 1 ms，保證至少睡滿）；狀態設 `Sleeping(wake_at)` 後 `schedule()`。tick hook 每 1 ms 檢查清單頭。同一時間到期的依先來後到（`SleepList` 的 host 測試）。
- `block()` / `wake(id)`：給「等某件事」用（現在是 `wait_process`，之後 IPC）。經典的競賽是「檢查條件 → 還沒睡著 → 別人已經叫醒」：

```
 等待者                          喚醒者
 檢查：還沒結束
 登記成 waiter
                                 結束了 → wake(等待者)
 block() → 睡著，永遠沒人叫醒   ← 錯
```

  解法：`wake` 對一個還在跑（或在 ready 裡）的執行緒設 `wake_pending`；`block` 看到 `wake_pending` 就清掉、直接返回；`wait_process` 迴圈重新檢查。`block` 的「決定要睡」與 `schedule` 之間關著中斷，所以單顆 CPU 上 `wake` 不會插在中間；SMP 時要改成持排程器的鎖跨過切換。

### 6.4 idle

每顆 CPU 一個 idle 執行緒（`TaskContext::new(idle_entry, ..)`，有自己的堆疊）：ready 是空的就 `sti; hlt`，否則 `yield_now()`。它不在 ready 裡，`schedule()` 找不到別人才選它；tick hook 看到 idle 在跑而 ready 非空就要求重排，所以它永遠可被搶佔。

### 6.5 boot 執行緒

`kernel::task::init()` 把正在跑的 `kernel_main`（loader 給的 64 KiB 堆疊）包成執行緒 `"main"`。`make run` 時它啟動 `init` 執行緒後 `exit()`；它沒有自己的 `KernelStack`，所以回收時沒有東西要釋放。selftest 時它就是跑測試的那個執行緒。

## 7. 位址空間（`frame/src/mm/user.rs`）

### 7.1 結構

```
 行程的 PML4                         kernel 的 PML4（loader 建的）
 ┌──────────────┐                    ┌──────────────┐
 │ 0..255 自己的 │──► 自己的 PDPT/PD/PT │ 0..255 空的   │
 ├──────────────┤                    ├──────────────┤
 │ 256..511     │──► 和 kernel 共用同一批下層表 ◄──┤ 256..511     │
 └──────────────┘                    └──────────────┘
```

- `AddressSpace::new()`：從 PMM 配一個清零的頁框當 PML4，把 kernel PML4 的項目 256–511 複製過來（共用下層表）。之後 kernel 在自己半部加的映射（heap、kernel 堆疊、MMIO），因為都落在**已存在**的 PML4 項目底下，所有位址空間自動看得到。
- 因此 `paging::map/unmap` 一律走 `KERNEL_ROOT`（開機時記下的 loader PML4），不管 CR3 現在是誰的。`translate` 仍走目前的 CR3。
- 限制：kernel 半部不能再新增 PML4 項目（新增只會出現在 kernel 的 PML4，不在已存在的行程裡）。目前用到的 256（HHDM）…、510（動態區＋MMIO）、511（kernel 映像）開機時就都在了。

### 7.2 映射

| 函式 | 說明 |
|---|---|
| `map(virt, PhysFrame, UserFlags)` | `virt < 0x0000_8000_0000_0000`、4 KiB 對齊；中間表 P\|W\|U，葉子 P\|U（\|W）（\|NX，有 NX 才設）；吃掉頁框的所有權憑證 |
| `unmap(virt) -> PhysFrame` | 還回憑證 |
| `translate(virt)` | 已映射的使用者頁才有值 |
| `check(virt, len, write)` | 每一頁都已映射、U/S=1（寫入時還要 W=1）、不溢位、不超出使用者半部 |
| `read(virt, &mut buf)`、`write(virt, &buf)` | copy_from_user／copy_to_user：先 `check` 全部，再一頁一頁**經 HHDM** 複製 |
| `load(virt, &buf)` | 同 `write` 但不要求 W（載入程式到唯讀頁） |
| `activate()` | CR3 ≠ 自己才寫 CR3 |
| `Drop` | 若是目前的 CR3，先切回 kernel 的 PML4；再走 0..255 的表，把所有葉子頁框、中間表、PML4 還給 PMM |

kernel **從不直接解參考使用者位址**：壞指標在檢查時就變成錯誤值（`BadAddress`），不會在 kernel 裡 page fault，也不需要 SMAP 的 `stac`/`clac`。U/S 位元的規則見 Vol. 3A §4.6。

### 7.3 使用者的配置（`abi::layout`）

```
 0x0000_0000_0040_0000   程式（user/user.ld）：text R-X、rodata R--、data RW-，各自 4 KiB 對齊
       …
 0x0000_7FFF_FFDF_F000   guard 頁（不映射；堆疊溢位 → page fault → 行程被殺）
 0x0000_7FFF_FFE0_0000   主執行緒的堆疊 1 MiB，載入時全部映射（清零）
 0x0000_7FFF_FFF0_0000   STACK_TOP；初始 RSP = STACK_TOP − 8（像剛被 call 進來）
 0x0000_8000_0000_0000   USER_END（非 canonical 的開始）
```

其他執行緒的堆疊由程式自己提供（`ThreadCreate(entry, stack_top, arg)`；libstela 用 static 的 `ThreadStack<N>`）。

`hello` 一個行程實際用掉的頁框：程式段 8 頁（text 2、rodata 1、data 5——data 裡有 16 KiB 的子執行緒堆疊）、主堆疊 256 頁、分頁表 7 頁（PML4 + 程式那邊 PDPT/PD/PT + 堆疊那邊 PDPT/PD/PT），共 271 頁（約 1.06 MiB）；另外每個執行緒 8 頁 kernel 堆疊。

### 7.4 位址空間的壽命

`Process` 擁有 `AddressSpace`；只有它的執行緒持有 `Arc<Process>`（行程表只記狀態）。最後一個執行緒被回收時 `Arc` 歸零 → `AddressSpace::drop`。這時 CR3 可能還指著它（例如切到的是 kernel 執行緒），`drop` 會先換回 kernel 的 PML4。

## 8. Zombie 回收

執行緒不能釋放自己正站著的堆疊。STELA 分兩層處理：

```
 執行緒 A exit()：state = Zombie → schedule()
    A 的 Box<Thread> 進 zombies；per-CPU.current_task = B；previous_task = A 的 TaskContext
    ── switch ──
 執行緒 B（在自己的堆疊上）：
    frame：drop previous_task      （A 的 Arc 少一份）
    kernel：reap()                 → drop A 的 Box<Thread>（最後一份 Arc → KernelStack::drop：
                                      unmap 8 頁、還給 PMM、slot 還回去）
                                   → A 屬於行程 P：P.thread_reaped()，最後一個 → 行程結束
```

就算 kernel 寫錯、提早 drop 了 A 的 TCB，per-CPU 的那份 `Arc` 也讓堆疊撐到切換完成——記憶體安全不依賴 kernel 的政策正確。selftest：16 個執行緒結束後 PMM 的 free 數與 heap 用量都回到原值。

## 9. 使用者模式

### 9.1 GDT（Vol. 3A §3.4.5、§5.2.1）

```
 索引  selector  描述子                          值
 0     0x00      null
 1     0x08      kernel code（L=1，DPL 0）       0x00AF_9A00_0000_FFFF
 2     0x10      kernel data（DPL 0）            0x00CF_9200_0000_FFFF
 3     0x18      user data（DPL 3）→ 0x1B        0x00CF_F200_0000_FFFF
 4     0x20      user code（L=1，DPL 3）→ 0x23   0x00AF_FA00_0000_FFFF
 5-6   0x28      TSS（16 bytes）
```

user data 在 user code **前面**，是 `sysret` 的規定（`docs/syscall.md` §4）。selector 的低兩位是 RPL：使用者用 0x1B／0x23。

### 9.2 第一次進 ring 3：`frame::task::enter_user(entry, rsp, arg)`

```
 檢查：entry、rsp 在使用者半部；目前的 context 有自己的 kernel 堆疊與 FPU 區
 TSS.RSP0 = per-CPU.kernel_rsp = 這個執行緒的 kernel 堆疊頂
 cli
 swapgs                    ; 使用者的 GS_BASE = 0，per-CPU 放到 KERNEL_GS_BASE
 push 0x1B                 ; SS
 push rsp                  ; 使用者 RSP
 push 0x202                ; RFLAGS：IF=1（bit 1 永遠是 1）
 push 0x23                 ; CS
 push entry                ; RIP
 rdi = arg，其他通用暫存器全部清零（不洩漏 kernel 的值）
 iretq                     ; Vol. 3A §6.14.3
```

`iretq` 一次換掉 CS（特權等級）、RIP、RFLAGS、SS、RSP。kernel 堆疊上的東西從此作廢：下一次 ring 3 被中斷時，CPU 從 RSP0（堆疊頂）開始推。

### 9.3 從 ring 3 回到 kernel

- **中斷／例外**：CPU 看到特權等級變了，從 TSS.RSP0 載入 kernel 堆疊，推 SS、RSP、RFLAGS、CS、RIP（Vol. 3A §6.14.4）。共用 stub 一進來先看推入的 CS（`[rsp + 24]`，在 vector 與 error code 上面）：

  ```asm
  test qword ptr [rsp + 24], 3
  jz 1f
  swapgs                 ; 來自 ring 3：換回 per-CPU 的 GS
  1:
  ```

  返回前（`add rsp, 16` 之後 CS 在 `[rsp + 8]`）同樣判斷，回 ring 3 才 `swapgs`。

- **syscall**：見 `docs/syscall.md` §4。

RSP0 每次 `switch_to` 都改成新執行緒的堆疊頂，所以每個使用者執行緒被中斷時都落在自己的 kernel 堆疊上。

### 9.4 per-CPU 與 GS

```
 在 kernel 裡：GS_BASE = &PerCpu，KERNEL_GS_BASE = 使用者的（0）
 在 ring 3：  GS_BASE = 0，        KERNEL_GS_BASE = &PerCpu
```

每條進入 kernel 的路（中斷、例外、syscall）都在第一件事 `swapgs`，每條回 ring 3 的路最後一件事 `swapgs`。kernel 裡的程式碼（包括被中斷的 kernel 程式碼）永遠看到 per-CPU 的 GS，所以 `cpu::current()`（`mov rax, gs:[0]`）照舊。使用者程式現在沒有辦法改 GS_BASE（沒開 CR4.FSGSBASE、沒有對應的 syscall）。

PerCpu 新增的欄位（`frame/src/cpu.rs`）：

| offset | 欄位 | 用途 |
|---|---|---|
| 24 | `kernel_rsp` | syscall 進入點載入的 kernel 堆疊（組語用固定 offset，編譯期檢查） |
| 32 | `user_rsp_scratch` | syscall 進入點暫存使用者 RSP |
| | `need_resched` | tick hook 設、`irq::dispatch` 清 |
| | `current_task`、`previous_task` | §4.2、§8 |

### 9.5 SMEP

`CPUID.(EAX=07H,ECX=0):EBX[7]` 有就設 CR4.SMEP（bit 20）：ring 0 不能執行 U/S=1 的頁（Vol. 3A §4.6.1），kernel 被騙去跳到使用者程式碼時會 page fault 而不是執行。QEMU 的預設 CPU 與 Conroe 都沒有 SMEP（selftest 印 `SMEP not available`）。SMAP 之後再說（kernel 不直接碰使用者位址，§7.2）。

## 10. 使用者例外 → 殺行程

```
 ring 3 #PF/#UD/#GP… → stub（swapgs）→ trap_dispatch：CS & 3 == 3
   → frame::task::user_fault → kernel user_fault(frame, cr2)：
       印：process 12 (fault): exception 14 (#PF page fault), error code 0x4 at RIP=0x400015 RSP=0x7fffffefffd0 CR2=0x0; killed
       exit_process(128 + vector)
         kill(process)：exit code（第一個決定的算數）；ready／sleeping／blocked 裡屬於它的執行緒 → zombies；
                        目前的執行緒 → Zombie
         schedule()（永不返回）
```

ring 0 的例外照舊 panic。exit code 的慣例：`128 + 向量`（#UD 134、#GP 141、#PF 142），`ProcessExit(code)` 自己給，使用者程式 panic 是 101，執行緒全部自己結束是 0，載入失敗（從沒跑過）是 −1。

注意 `exit_process` 永不返回，堆疊上的區域變數不會被 drop——所以它在 `schedule()` 之前先 drop 自己拿的 `Arc<Process>`（第一次寫的時候漏了，selftest 抓到每個被殺的行程漏 9 個頁框）。

## 11. 行程與 `wait_process`（`kernel/src/task/process.rs`）

```rust
pub struct Process {
    id, name,
    space: AddressSpace,
    live: AtomicUsize,               // 建立了、還沒回收的執行緒數
    exit_code: SpinLock<Option<i64>>,
}
static TABLE: SpinLock<BTreeMap<ProcessId, Status>>;   // Running { waiters } | Exited(code)
```

- `process::create(name)`、`spawn_user(&process, entry, rsp, arg)`（行程正在結束就拒絕）。
- 最後一個執行緒被回收 → `finish(id, code)`：表裡改成 `Exited`、叫醒所有 waiter。
- `wait_process(id) -> Option<i64>`：kernel 內部的（不是 syscall）；`Exited` 就回傳，否則登記成 waiter 後 `block()`，醒來再檢查。狀態會留著，再等一次還是同一個值。未知 id 回 `None`。
- `exit_status(id)`：不等待，只看。
- 從沒有執行緒的行程（載入失敗）drop 時以 −1 結束，等它的人不會永遠等。

## 12. initrd 與程式載入

### 12.1 打包（Makefile）

```
make user     cargo build --release -p hello -p spin -p fault --target x86_64-unknown-none
              → build/user/{hello,spin,fault}
make initrd   tar --format=ustar --owner=0 --group=0 --numeric-owner --mtime=@0
                  -cf build/initrd.tar -C build/user hello spin fault
make / esp    複製到 build/esp/STELA/initrd.tar（esp-test 也是）
```

固定 owner 與 mtime，同樣的程式就是同樣的 bytes。目前 80 KiB（三個程式 18–35 KiB，加 header 與結尾）。

### 12.2 loader

`load_initrd()` 用 `uefi::fs` 讀 `\STELA\initrd.tar`（和 kernel.elf 一樣），複製到 `LOADER_DATA` 頁面（kernel 看到的是 `KernelAndBoot`，PMM 不會拿去用），把實體位址與大小放進 `BootInfo`（版本 2，`docs/boot.md` §9）。沒有這個檔案不是錯誤：欄位為 0，印 `boot: no \STELA\initrd.tar`。

```
boot: kernel at 0x1de43000 (132 KiB), entry 0xffffffff80004ae8
boot: initrd at 0x1de2f000 (81920 bytes)
```

kernel 端 `frame::mm::initrd()` 檢查範圍落在某個 KernelAndBoot 區段與 HHDM 內，才把它當 `&'static [u8]`。

### 12.3 ustar（`kernel/src/initrd.rs`）

```
 512-byte header   offset  大小  欄位
                   0       100   name（短於 100 時 NUL 結尾）
                   124     12    size，八進位 ASCII
                   148     8     checksum：header 所有 bytes 的和，這 8 個 bytes 當作空白；八進位
                   156     1     type：'0' 或 NUL = 檔案，'5' = 目錄
                   257     6     magic "ustar\0"（GNU 的舊格式是 "ustar "，前 5 個字一樣）
                   345     155   prefix：路徑 = prefix + "/" + name
 接著是檔案內容，補 0 到 512 的倍數；兩個全 0 的 block 代表結束
```

`entries(archive)` 一個一個走（壞的 header → 錯誤並停止）、`find(archive, path)` 找一般檔案。去掉開頭的 `./`、目錄名稱結尾的 `/`。host 測試：手工合成的封存（含 prefix、目錄、600 bytes 的檔案）、checksum／magic 錯、資料或 header 被截斷、空封存，以及 `build/initrd.tar`（`make test-host` 依賴 `make initrd`，所以一定測到真的那個）。

### 12.4 ELF → 行程（`kernel/src/program.rs`）

```
 spawn(name, image)：
   elfload::Elf::parse（ET_EXEC、x86-64、段頁對齊、遞增不重疊、entry 在可執行段）
   process::create(name)
   每個 PT_LOAD：
     不可 W+X；不可從 0 開始；結尾不可超過 stack guard
     [vaddr, 結尾向上取頁) 每頁配清零頁框、依 flags 映射（W、NX）
     AddressSpace::load 複製檔案內容（.bss 已經是 0）
   映射 1 MiB 堆疊
   spawn_user(process, entry, STACK_TOP − 8, 0)
 spawn_from_initrd(name) = initrd::find(frame::mm::initrd(), name) → spawn
```

錯誤：`NotFound`、`BadElf(elfload::Error)`、`BadSegment`、`OutOfMemory`。失敗時行程沒有執行緒，drop 時把已映射的都還回去（selftest 檢查失敗的載入不漏頁框）。

### 12.5 `make run` 的開機流程

```
 kernel_main（boot 執行緒）→ task::init → syscall::init → spawn("init") → exit()
 init：spawn_from_initrd("hello") → wait_process → 印 exit code → echo 迴圈
```

```
hello from user mode
hello from a second thread (argument 7)
hello: thread 5 finished within 2000 us
kernel: hello (process 1) exited with code 0
kernel: ready; type to echo, Enter prints the uptime
```

echo 迴圈沒有輸入時 `sleep_ms(4)`（之前是 `hlt`，現在 CPU 讓給別人）。

## 13. 測試

| 測試 | 內容 |
|---|---|
| host `kernel::task::queue` | RunQueue FIFO、round-robin、`take_where` 保持順序；SleepList 依時間排序、同時間 FIFO、`take_where` |
| host `kernel::initrd` | 八進位欄位、合成封存、空封存、各種損壞、真的 `build/initrd.tar`（三個程式都在、都是 ELF） |
| host `abi` | 見 `docs/syscall.md` |
| host `bootinfo` | 版本 2、新欄位位移 |
| selftest "kernel threads" | 兩個執行緒 ping-pong 各 1000 次，嚴格交錯（每次遞增前的計數都是預期值） |
| selftest "thread reaping" | 先暖身 16 個，再 16 個執行緒結束後：沒有 Zombie、PMM free 數與 heap 用量回到原值 |
| selftest "preemption" | spinner 旁 `sleep_ms(10)` 5 次都 10–99 tick；main 忙 30 ms 不讓出時 spinner 仍有進度 |
| selftest "sleep order" | 4 個執行緒睡 30/10/20/0 ms，醒來順序 0/10/20/30，且各自睡滿 |
| selftest "user mode faults" | 內建機器碼：`ud2`→134、讀 0→142、讀 kernel 位址→142（error code 0x5：保護違規、使用者）、`hlt`→141、`jmp $` 被搶佔、`kill`→99；`AddressSpace` 的 check／write 拒絕；結束後 PMM 回到原值 |
| selftest "system calls"／"elf loading" | 見 `docs/syscall.md` |
| selftest "initrd programs" | hello→0（`make test-qemu` 檢查兩個執行緒的輸出）、fault→142、spin→0 且主執行緒 5 次 `sleep_ms(10)` 都 < 100 tick；之後沒有 Zombie、PMM 回到原值 |
| `make test-fb` | 一般 kernel：serial 依序出現 `hello from user mode`、`exited with code 0`、`kernel: ready`，打 `h i ret` 後 `uptime:`，畫面檢查 |
| `make test-qemu-min` | 同 selftest，`-cpu Conroe,-nx -m 256M`（沒有 NX：程式段全部可執行，W^X 只剩 W） |

另外手動做過 6 個 selftest QEMU 並行（各自一份 ESP 副本），都通過。

## 14. 已知限制與待辦

- 只有 BSP；排程器結構上是 per-CPU，但 `block`/`wake` 的競賽只靠「單顆 CPU + 關中斷」處理，SMP 時要重做（持鎖跨切換或 per-thread 狀態 atomic）。
- 單一等級 round-robin；`priority` 只存不用。沒有 tickless：idle 時計時器仍每 1 ms 響。
- 睡眠的解析度是 tick（1 ms）；wake-up 在 tick 才檢查。
- kernel 堆疊固定 32 KiB；slot 的虛擬位址不回收到 PMM 以外（slot 會重用）。kernel 堆疊配置失敗時已映射的頁框會還回去，但 `paging::map` 失敗時那一個頁框會漏掉（只在記憶體用盡時）。
- 位址空間：只有 4 KiB 頁；沒有需求分頁、copy-on-write、`mmap`／`munmap` 類的 syscall；空掉的中間表不回收（只在整個位址空間 drop 時一起釋放）。kernel 半部不能新增 PML4 項目（§7.1）。
- 使用者 FPU 只有 FXSAVE（x87/SSE），沒有 XSAVE/AVX。
- NMI、#MC 若剛好發生在 `swapgs` 前後或 `sysret` 前（使用者 RSP 已載入）會用錯 GS／堆疊；它們沒有 IST（NMI 現在被遮蔽在 LINT1）。之後給 NMI/#MC/#DB 各自的 IST，並在 stub 裡用 GS_BASE 的值判斷而不是 CS。
- 行程結束時，屬於它的執行緒若正在另一顆 CPU 上跑（SMP 之後）不會立刻停下；現在單顆 CPU 沒有這個情況。
- 行程表的 `Exited` 狀態永遠不清（沒有「回收狀態」的動作，P4 有 handle 之後再定）。
- 使用者程式的 `.data`／`.bss` 若是空的，連結出來的空 PT_LOAD 會被 `elfload` 拒絕（目前三個程式都有資料，未實際遇到）。
- `DebugWrite` 是暫時的（P4 之後改成 console 服務的 handle）。
