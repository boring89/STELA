# System call ABI 與 libstela

> P3 由自動排程一次做完，使用者已於 2026-10-20 審核。P4 的修改（handle／port／memory／input 的呼叫、移除 `DebugWrite`、libstela 經 console 服務印字）使用者已於 2026-10-20 審核，見 `docs/ipc.md`。

使用者程式怎麼請 kernel 辦事：暫存器怎麼放、有哪些呼叫、錯誤怎麼回報、kernel 怎麼進出，以及使用者程式用的執行期 libstela。行程、執行緒與位址空間見 `docs/process.md`。

程式碼：`abi/`（共用的編號與型別，host 測試）、`frame/src/arch/x86_64/syscall.rs`（MSR 與進入點組語）、`frame/src/syscall.rs`（`SyscallFrame`、`set_handler`）、`kernel/src/syscall.rs`（分派與每個呼叫）、`user/libstela/`。

## 1. 比喻：櫃台的叫號鈴

使用者程式在櫃台外（ring 3），不能自己走進辦公室。它把**申請單**填在固定的格子裡（暫存器：RAX 寫「要辦什麼」，RDI、RSI… 寫細節），按**叫號鈴**（`syscall` 指令）。CPU 立刻把它帶進辦公室的**固定入口**（IA32_LSTAR 指的那段程式），入口的職員先換上辦公室的識別證（`swapgs`）、改用辦公室的桌子（kernel 堆疊），把申請單影印一份（`SyscallFrame`），交給承辦人（`kernel::syscall::handle`）。承辦人把結果寫在兩個格子：**RAX = 成功或失敗的代碼，RDX = 結果**，再從原路送回（`sysretq`）。

承辦人不相信申請單上的任何地址：要讀使用者記憶體時，先查使用者的檔案櫃（分頁表）確認那一頁真的是他的，才去影印（`AddressSpace::read`）。

## 2. 名詞

| 名詞 | 意思 |
|---|---|
| `syscall`／`sysret` | x86-64 的快速系統呼叫指令（Intel SDM Vol. 2B "SYSCALL"、"SYSRET"；Vol. 3A §5.8.8） |
| MSR | Model-Specific Register，`rdmsr`／`wrmsr` 存取的 CPU 設定 |
| IA32_STAR／LSTAR／FMASK | syscall 用的三個 MSR：段選擇子、進入點、進入時要清掉的 RFLAGS 位元 |
| ABI | Application Binary Interface：二進位層面的約定（哪個暫存器放什麼、編號是多少） |
| canonical 位址 | 48 位元虛擬位址時，bit 63–47 必須全部等於 bit 47；中間是不能用的洞 |

## 3. 呼叫慣例（`abi` crate）

```
 進：  RAX = 編號（abi::Syscall）   RDI RSI RDX R10 R8 R9 = 參數 1–6
 出：  RAX = 狀態（0 = OK，否則 abi::Error 的值）
       RDX = 回傳值（狀態 OK 時才有意義）
 會壞：RCX、R11（CPU 用它們存返回位址與 RFLAGS）；其他暫存器都保留
```

第 4 個參數用 R10 而不是 System V 的 RCX，因為 `syscall` 會覆寫 RCX（Linux 也是這樣）。「狀態 + 值」分兩個暫存器（而不是 Linux 的負數 errno），讓 64 位元的回傳值（例如時間、之後的位址）不必和錯誤碼搶空間。

### 3.1 編號（`abi::Syscall`）

| 編號 | 名稱 | 參數 | 回傳值 | 說明 |
|---|---|---|---|---|
| 0 | `ProcessExit` | code | —（不返回） | 結束整個行程（所有執行緒），exit code = `code as i64` |
| 1 | `ThreadExit` | value | —（不返回） | 結束呼叫的執行緒，value 給 `ThreadJoin`（P11）；最後一個執行緒結束時行程以 0 結束 |
| 2 | `ThreadCreate` | entry, stack_top, arg | Thread handle | 同一行程的新執行緒：RIP = entry、RDI = arg、RSP = `(stack_top & !15) − 8`；P11 起回 handle（`WAIT | DUPLICATE | TRANSFER`，`docs/process.md` §8.1），之前是執行緒 id |
| 3 | `Yield` | — | 0 | 讓別的執行緒先跑 |
| 4 | `SleepNs` | ns | 0 | 至少睡 `ns` 奈秒（tick 解析度：1 ms） |
| 5 | `ClockNowNs` | — | 開機以來的奈秒 | 單調；沒有 invariant TSC 時解析度 1 ms |
| 6 | —（`DebugWrite`，P4 移除） | | | 編號保留不用，回 `InvalidSyscall`；印字改成送訊息到 console 服務（`docs/ipc.md` §7） |
| 7 | `HandleClose` | h | 0 | 見 `docs/ipc.md` §10 |
| 8 | `HandleDuplicate` | h, rights | 新 handle | 權限只能減少 |
| 9 | `PortCreate` | — | handle | |
| 10 | `PortSend` | port, &Message | 0 | 不阻塞；`Full`／`PeerClosed` |
| 11 | `PortReceive` | port, &mut Message, timeout_ns | 0 | 0 = 不等，`u64::MAX` = 永遠 |
| 12 | `MemoryCreate` | size | handle | 4 KiB 倍數，≤ 64 MiB |
| 13 | `MemoryMap` | memory, flags | 位址 | 在 `MAP_BASE..MAP_END` |
| 14 | `MemoryUnmap` | addr | 0 | |
| 15 | `InputAttach` | port, input | 0 | 鍵盤事件送到 port；P11 起要 `INPUT` 能力（`docs/userland.md` §2.3） |
| 16–26 | `DirOpen`…`VolumeOpenById` | | | 檔案與卷宗，見 `docs/vfs.md` §4（P5，使用者已於 2026-10-20 審核） |
| 27–31 | `ProcessSpawn`、`ProcessWait`、`ProcessKill`、`ProcessSelf`、`SysInfo` | | | 行程，見 `docs/userland.md` §2（P6，使用者已於 2026-10-20 審核） |
| 32 | `SystemPower` | kind | —（成功不返回） | 卸載卷宗後關機（0）或重新開機（1），見 `docs/acpi.md`（P6.5，使用者已於 2026-10-20 審核） |
| 33–36 | `TtyCreate`、`TtyRead`、`TtyWrite`、`HandleKind` | | | 終端機與 handle 種類，見 `docs/linux.md` §5（P7，使用者已於 2026-10-20 審核） |
| 37–38 | `FileGetInfo`、`FileSetInfo` | 檔案或資料夾, 32 bytes | 0 | Finder 資訊，見 `docs/vfs.md` §4.2（P9，使用者已於 2026-10-20 審核） |
| 39–46 | `SocketCreate`、`SocketBind`、`SocketConnect`、`SocketListen`、`SocketAccept`、`SocketSend`、`SocketRecv`、`NetInfo` | | | 網路，見 `docs/net.md` §6（P10，使用者已於 2026-10-21 審核） |
| 47–48 | `AudioOpen`、`AudioClose` | | | 音效，見 `docs/sound.md` §4（P10，使用者已於 2026-10-21 審核） |
| 49–52 | `WaitSetCreate`、`WaitSetAdd`、`WaitSetRemove`、`WaitSetWait` | | | 同時等多個物件，見 `docs/ipc.md` §13（P11，使用者尚未審核） |
| 53 | `FilePath` | 檔案或資料夾, buf, len | 路徑長度 | 卷宗內的路徑，見 `docs/vfs.md` §4.4（P11） |
| 54–56 | `PipeCreate`、`PipeRead`、`PipeWrite` | | | pipe，見 `docs/ipc.md` §14（P11） |
| 58–60 | `FileGetAttr`、`FileSetAttr`、`FileListAttr` | | | 延伸屬性，見 `docs/vfs.md` §4.5（P11） |
| 57 | `TtySetSize` | master, 欄, 列 | 0 | 給 Linux 的 `TIOCGWINSZ`，見 `docs/window.md` §7（P11） |
| 61 | `ThreadJoin` | thread, timeout_ns | 執行緒的 value | 等執行緒結束（`u64::MAX` 永遠等；`TimedOut`）；跟著行程結束的 → `EXIT_KILLED`，見 `docs/process.md` §8.1（P11） |
| 62 | `ProcessOpen` | pid, power | 行程 handle（`WAIT | KILL | DUPLICATE`） | 用 pid 拿任何執行中的行程，要 `POWER` 能力；見 `docs/apps.md` §3（P11） |

P7 起沒有 STELA ELF note 的程式用 **Linux 人格**：同一個進入點，號碼與回傳慣例是 Linux 的（`docs/linux.md`）。

編號就是 ABI：`abi` 的 host 測試把每個值鎖住。未知編號 → `InvalidSyscall`。

### 3.2 錯誤（`abi::Error`）

| 值 | 名稱 | 何時 |
|---|---|---|
| 1 | `InvalidSyscall` | RAX 不是任何呼叫 |
| 2 | `InvalidArgument` | 參數超出範圍：`ThreadCreate` 的 entry ≥ `USER_END`、stack_top 不在 16..=`USER_END`；訊息的 len > 112 或 handles > 4；未定義的權限／flag 位元 |
| 3 | `BadAddress` | 指標所指的範圍有任何一頁不是已映射的使用者頁（kernel 位址、未映射、跨出使用者半部、溢位） |
| 4 | `OutOfMemory` | kernel 配不到記憶體（例如新執行緒的 kernel 堆疊） |
| 5 | `NotSupported` | 合法但這裡不能做（kernel 執行緒沒有行程；行程正在結束時 `ThreadCreate`） |
| 6–13 | `BadHandle`、`AccessDenied`、`WrongType`、`Full`、`TimedOut`、`PeerClosed`、`OutOfHandles`、`AlreadyMapped` | 見 `docs/ipc.md` §10 |
| 14–21 | `NotFound`、`Exists`、`NotDir`、`IsDir`、`NotEmpty`、`ReadOnly`、`NoSpace`、`IoError` | 見 `docs/vfs.md` §4 |
| 22–25 | `ConnectionRefused`、`ConnectionReset`、`NetworkDown`、`AddressInUse` | 見 `docs/net.md` §6（P10） |
| 26 | `Busy` | 裝置已被別的程式開啟（`AudioOpen`，`docs/sound.md`） |

`abi::encode(Result) -> (rax, rdx)` 給 kernel、`abi::decode(rax, rdx) -> Result` 給使用者；未知的非 0 狀態解成 `InvalidSyscall`。

### 3.3 位址配置（`abi::layout`）

| 常數 | 值 | |
|---|---|---|
| `PROGRAM_BASE` | `0x0000_0000_0040_0000` | 程式的連結位址（`user/user.ld`） |
| `STACK_GUARD` | `0x0000_7FFF_FFDF_F000` | 不映射的 guard 頁 |
| `STACK_TOP` − `STACK_SIZE` | `0x0000_7FFF_FFE0_0000` | 主堆疊底（1 MiB） |
| `STACK_TOP` | `0x0000_7FFF_FFF0_0000` | 主堆疊頂；初始 RSP = `STACK_TOP − 8` |
| `USER_END` | `0x0000_8000_0000_0000` | 使用者半部結尾（不含） |
| `MAP_BASE`／`MAP_END` | `0x0000_1000_0000_0000`／`0x0000_7000_0000_0000` | `MemoryMap` 的映射區（`docs/ipc.md` §5.3） |

不變量（4 KiB 對齊、順序）在 `abi` 裡用編譯期 `const` 斷言檢查。

## 4. 進入點（`frame/src/arch/x86_64/syscall.rs`）

### 4.1 `syscall` 指令做了什麼（Vol. 2B "SYSCALL"）

```
 RCX    ← RIP（下一條指令）
 R11    ← RFLAGS
 RFLAGS ← RFLAGS & ~IA32_FMASK
 CS     ← IA32_STAR[47:32]（0x08）     SS ← IA32_STAR[47:32] + 8（0x10）
 RIP    ← IA32_LSTAR
 RSP    不變！（還是使用者的堆疊）
```

### 4.2 MSR 設定（`syscall::init`，在 `frame::init` 裡）

| MSR | 編號 | 值 | 說明 |
|---|---|---|---|
| IA32_EFER | `0xC000_0080` | \|= bit 0（SCE） | 允許 `syscall`／`sysret` |
| IA32_STAR | `0xC000_0081` | `0x0010_0008_0000_0000` | [47:32] = 0x08：syscall 的 CS（SS = 0x10）；[63:48] = 0x10：sysret 的基底 |
| IA32_LSTAR | `0xC000_0082` | `stela_syscall_entry` | 64 位元進入點 |
| IA32_FMASK | `0xC000_0084` | `0x700` | 清 TF（bit 8）、IF（bit 9）、DF（bit 10）：不單步、換好堆疊前不接中斷、字串指令往前 |
| IA32_KERNEL_GS_BASE | `0xC000_0102` | 0 | 使用者的 GS_BASE，第一次 `swapgs` 前 |

`sysretq` 載入 SS = 基底 + 8 = 0x18、CS = 基底 + 16 = 0x20，RPL 強制為 3（0x1B、0x23）——所以 GDT 必須是 user data 在前、user code 在後（`docs/process.md` §9.1）。

### 4.3 進入與返回

```asm
stela_syscall_entry:
    swapgs                          ; GS_BASE = 這顆 CPU 的 PerCpu
    mov gs:[32], rsp                ; user_rsp_scratch = 使用者 RSP
    mov rsp, gs:[24]                ; kernel_rsp = 目前執行緒的 kernel 堆疊頂
    push qword ptr gs:[32]          ; ┐
    push rcx                        ; │ SyscallFrame（80 bytes，低位址在下面那行）
    push r11                        ; │
    push rax                        ; │
    push rdi                        ; │
    push rsi                        ; │
    push rdx                        ; │
    push r10                        ; │
    push r8                         ; │
    push r9                         ; ┘
    mov rdi, rsp
    cld
    call syscall::dispatch          ; 開中斷 → kernel 的 handler → （RCX 非 canonical 就當 #GP）
    cli
    pop r9 / r8 / r10 / rdx / rsi / rdi / rax / r11 / rcx
    pop rsp                         ; 使用者 RSP
    swapgs
    sysretq
```

```
 SyscallFrame（低位址在上）
 rsp → r9 r8 r10 rdx rsi rdi rax  rflags(r11) rip(rcx) rsp(使用者)
       └────── pub：kernel 讀參數、寫 rax/rdx ──┘ └──── 私有：只能讀 ────┘
```

- 堆疊頂 16 對齊，推 10 個（80 bytes）之後仍 16 對齊，`call` 符合 System V。
- `gs:[24]`、`gs:[32]` 是 `PerCpu` 的固定 offset，`cpu.rs` 用 `offset_of!` 在編譯期檢查。
- 使用者的 callee-saved 暫存器（rbx、rbp、r12–r15）由 Rust 程式碼本身保留（System V 規則），切換執行緒時也由 `switch_to` 保存。其他參數暫存器從 frame 還原成使用者原本的值，所以 kernel 的值不會漏出去。selftest 檢查 rbx 跨過 `SleepNs`（會切換執行緒）仍保留。
- handler 執行時中斷開著：syscall 可以被搶佔、可以睡、可以切換；`ProcessExit`／`ThreadExit` 不返回（這個 kernel 堆疊之後被回收）。
- `rip`／`rsp`／`rflags` 是私有欄位：kernel 不能把使用者送去任意的地方。`sysret` 回到非 canonical 的 RCX 會在 **ring 0、使用者 RSP 已載入**時 #GP（Intel 的行為，著名的漏洞來源）；RCX 只會是使用者 `syscall` 的下一條指令，唯一的情況是 `syscall` 放在使用者半部的最後兩個 bytes——`dispatch` 檢查 `rip >= USER_END` 就當成 #GP 殺掉行程。

### 4.4 kernel 端（`kernel/src/syscall.rs`）

```rust
fn handle(frame: &mut SyscallFrame) {
    let [a0, a1, a2, ..] = frame.args();
    let result = match Syscall::from_raw(frame.number()) {
        Some(Syscall::ProcessExit) => task::exit_process(a0 as i64),   // !
        Some(Syscall::ThreadExit) => task::exit_with(a0),              // !
        Some(Syscall::ThreadCreate) => thread_create(a0, a1, a2),
        Some(Syscall::Yield) => { task::yield_now(); Ok(0) }
        Some(Syscall::SleepNs) => { task::sleep_ns(a0); Ok(0) }
        Some(Syscall::ClockNowNs) => Ok(frame::time::now_ns()),
        Some(Syscall::HandleClose) => handle_close(a0),
        // ... HandleDuplicate、Port*、Memory*、InputAttach（docs/ipc.md）
        None => Err(Error::InvalidSyscall),
    };
    let (status, value) = abi::encode(result);
    frame.set_return(status, value);
}
```

讀使用者記憶體一律經 `AddressSpace::read`／`write`（走表檢查每一頁，`docs/process.md` §7.2），例如 `PortSend` 把 144 bytes 的訊息複製進來。

## 5. libstela（`user/libstela/`）

使用者程式的執行期，`#![no_std]`。**使用者程式唯一需要的 `unsafe` 在這裡**：`sys.rs` 的 `syscall` inline asm、`ThreadStack` 的 `Sync` 與起始資料，以及 `ipc::Mapping` 對共享記憶體的複製。

| 項目 | 說明 |
|---|---|
| `entry!(main)` | 產生 `#[unsafe(no_mangle)] extern "C" fn _start() -> !`：呼叫 `main() -> i64`，以回傳值 `exit`。由外部巨集產生，所以程式本身可以 `#![forbid(unsafe_code)]`（和 frame 的 `entry_point!` 同一招） |
| `exit(code)`、`thread_exit()` | `-> !` |
| `yield_now()`、`sleep_ns(ns)`、`sleep_ms(ms)`、`now_ns()` | |
| `console_write(&[u8]) -> Result<usize, Error>` | 112 bytes 一則 `CONSOLE_WRITE` 訊息送到 `[1]`；`Full` 時讓出再送 |
| `debug_print!`、`debug_println!` | 格式化進 256 bytes 的堆疊緩衝區再 `console_write`（一行不會被別的執行緒插進來）；超過 256 bytes 就分次送 |
| `ipc::*` | `handle_close`、`handle_duplicate`、`port_create`、`port_send`、`port_receive`（`FOREVER`）、`memory_create`、`memory_map`、`memory_unmap`、`Mapping`、`input_attach`（`docs/ipc.md`） |
| `pub use abi` | 程式可以用 `libstela::abi::key` 等型別 |
| `ThreadStack<N>` + `spawn(entry: fn(u64) -> u64, arg, &'static ThreadStack<N>) -> Result<JoinHandle, Error>` | 放在 `static` 的堆疊，占用中再用 → `InvalidArgument`。`(entry, arg)` 存在 ThreadStack 開頭，`ThreadCreate` 的 arg 是 ThreadStack 的位址；新執行緒從 libstela 的 `thread_start` 開始，讀出來呼叫 `entry(arg)`，回來就 `thread_exit(回傳值)`。P11：`JoinHandle::join()`／`join_timeout(ns)` 拿到回傳值，之後堆疊可以再用；`handle()` 可以放進 WaitSet（`docs/process.md` §8.1） |
| `sys::syscall0..4`、`syscall6` | 原始呼叫：`inlateout("rax")`、`in("rdi"/"rsi"/"r10"/"r8"/"r9")`、`inlateout("rdx")`、`lateout("rcx"/"r11")` |
| `fs::*` | 檔案與卷宗（`docs/vfs.md`） |
| `process::*`、`heap`、`path`、`line`、`print!`／`println!` | 行程、參數、heap、路徑、行編輯（`docs/userland.md`）；`entry!` 在 `main` 前收下參數訊息 |
| panic handler | `debug_println!("panic: {info}")` → `exit(101)` |

### 5.1 程式的建置

- `user/{hello,spin,fault}` 是 workspace 成員；目標 `x86_64-unknown-none`（和 kernel 同一個，soft-float、沒有 red zone），沿用 `.cargo/config.toml` 的 `-C relocation-model=static` → `ET_EXEC`。目標預設的 code-model=kernel 在 `0x40_0000` 也連得起來（位址是 32 位元有號數能表示的正數）。
- 每個程式的 `build.rs` 加 `-T user/user.ld`：`. = 0x400000`，text（R-X）、rodata（R--）、data（RW-，含 .bss 與 .got）三個 PT_LOAD、各自 4 KiB 對齊，丟掉 `.eh_frame`（panic=abort 不需要）。
- 實際大小：hello 35 KiB（data 裡有 16 KiB 的子執行緒堆疊）、spin 18 KiB、fault 18 KiB。

### 5.2 三個程式

| 程式 | 做什麼 | 預期 |
|---|---|---|
| `hello` | 印 `hello from user mode`；`spawn` 子執行緒（arg 7）印 `hello from a second thread (argument 7)`；主執行緒 `sleep_ms(1)` 等它；同一個 ThreadStack 再 `spawn` 必須失敗；印耗時 | exit 0 |
| `spin` | 200 ms 內只呼叫 `ClockNowNs`，不讓出；印 `spin: N loops in 200 ms` | exit 0；期間 kernel 的其他執行緒照常跑 |
| `fault` | 印 `fault: reading address 0`，用 inline asm `mov al, [0]` | page fault → exit 142 |

`fault` 用 inline asm 而不是 `read_volatile(null)`：Rust 裡解參考 null 是未定義行為，編譯器不保證真的去讀；asm 的讀取一定會發生。這是測試程式裡唯一的 `unsafe`（`#![deny(unsafe_code)]` + 局部 `allow`）。

## 6. 測試

| 測試 | 內容 |
|---|---|
| host `abi` | 每個編號與錯誤值固定、`from_raw` 往返與邊界、`encode`/`decode` 往返、未知狀態 → `InvalidSyscall`、layout 的值 |
| selftest "system calls" | 手組的 379 bytes 使用者程式（GNU as 組譯，原始碼在 `kernel/src/selftest.rs` 的註解；P4 重新組譯）：ClockNowNs、`PortSend` 到 console `[1]` 正常（`make test-qemu` 檢查 `user: hello from built-in user code`）／kernel 位址／未映射 → `BadAddress`、編號 6（已移除）與 999 → `InvalidSyscall`、`HandleClose(0)` → `BadHandle`、Yield、SleepNs(2 ms) 後時間至少多 2 ms、rbx 保留、ProcessExit(42)；任何一項不對就 exit 1 |
| selftest "elf loading" | 把上面的程式包成手工 ELF（一個 PT_LOAD，`p_memsz` 多 8 KiB 測 .bss）：exit 42、記憶體回復；W+X 段、非 ELF、initrd 沒有的名稱都被拒 |
| selftest "initrd programs" | hello／fault／spin，見 `docs/process.md` §13 |

## 7. 已知限制與待辦

- 行程 syscall 見 `docs/userland.md`；Thread handle 與 join 見 `docs/process.md` §8.1（P11）。
- `ThreadCreate` 不檢查 stack_top 是否已映射（用到才 page fault → 行程被殺）。P11 起有 join（`docs/process.md` §8.1）；沒有單一執行緒的 kill。
- 沒有 SMAP；kernel 不直接解參考使用者指標，所以目前不需要。沒有 32 位元相容（`int 0x80`、compat mode）的進入點。
- syscall 進入點與 NMI 的交互（`swapgs` 之前、`sysret` 之前的 NMI）見 `docs/process.md` §14。
