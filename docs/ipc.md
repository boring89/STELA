# IPC 與 handle

> P4 由自動排程一次做完，使用者已於 2026-10-20 審核。

行程之間怎麼溝通、怎麼指稱 kernel 裡的東西：kernel 物件、handle 與權限、port 與訊息、handle 的轉移、共享記憶體、每個行程一出生就有的 handle、console 服務、鍵盤事件的投遞，以及等待佇列。架構決定見 `docs/architecture.md` §4 的 C（IPC）、D（handle + 權限）、I（能力式安全）。syscall 的暫存器慣例見 `docs/syscall.md`。

程式碼：

```
abi/src/lib.rs                 Handle、Rights、Message、tag、bootstrap、map、編號與錯誤
abi/src/key.rs                 KeyCode、Modifiers、KeyEvent（4 bytes 編碼）、key_to_char
kernel/src/object/handle.rs    HandleTable<T>（純邏輯，host 測試）
kernel/src/object/regions.rs   映射區的區間配置器（純邏輯，host 測試）
kernel/src/object/port.rs      Port、QueuedMessage
kernel/src/object/mod.rs       KernelObject
kernel/src/task/queue.rs       WaitQueue（純邏輯，host 測試）
kernel/src/task/sched.rs       block_until、current_killed
kernel/src/task/process.rs     每個行程的 handle 表、主 port、映射區
kernel/src/syscall.rs          handle／port／memory／input 的 syscall
kernel/src/console.rs          console 服務
kernel/src/input.rs            input-router
frame/src/mm/shared.rs         SharedMemory（頁框集合）
frame/src/mm/user.rs           AddressSpace::map_shared／unmap_shared
user/libstela/src/ipc.rs       使用者端的包裝與 Mapping
user/ipc_server、ipc_client、echo
```

## 1. 比喻：郵局與鑰匙圈

- **kernel 物件**是郵局裡的東西：信箱（port）、儲物櫃（共享記憶體）。行程自己摸不到，只能請櫃台（syscall）幫忙。
- **handle** 是行程鑰匙圈上的一把鑰匙，上面刻著號碼（slot 索引）和**版次**（generation）。鑰匙圈每個行程一個（handle 表），別人的鑰匙對你沒用。
- 每把鑰匙有**權限**：這把只能投信（`SEND`）、那把可以開信箱收信（`RECEIVE`）、可以再打一把（`DUPLICATE`）、可以寄給別人（`TRANSFER`）。打新鑰匙只能打「權限一樣或更少」的。
- **信**（訊息）是固定大小的小卡片：最多 112 bytes 的字，加上最多 4 把**夾帶的鑰匙**。夾帶的鑰匙從寄件人的鑰匙圈拿下來，收件人收信時才掛到自己的圈上。
- 大東西不塞進信裡：放進**儲物櫃**（共享記憶體），把儲物櫃的鑰匙夾在信裡寄過去，對方打開同一個櫃子。
- 信箱的主人搬走（行程結束）時，信箱封起來：再投信會被退（`PeerClosed`）。

## 2. 名詞

| 名詞 | 意思 |
|---|---|
| kernel 物件 | kernel 裡有狀態、可以被 handle 指到的東西：`Port`、`Memory`（P5、P6 再加 Volume／Directory／File、Process／Thread） |
| handle | 32 位元的值：低 24 位元 = handle 表的索引，高 8 位元 = 那個 slot 的 generation |
| 權限（rights） | handle 上的位元，決定能對物件做什麼 |
| port | 訊息佇列：很多個送方、一個收的那一邊（N:1） |
| 轉移（transfer） | handle 跟著訊息從送方的表移到收方的表 |
| 能力（capability） | 「持有 handle = 有權做這件事」的安全模型；沒有全域名稱可以繞過 |

## 3. handle 與 handle 表

### 3.1 `abi::Handle`

```
 31        24 23                              0
 ┌──────────┬────────────────────────────────┐
 │generation│            index                │
 └──────────┴────────────────────────────────┘
 例：0x0200_0003 = slot 3、第 2 版
```

- generation 從 1 開始，所以 `0` 永遠無效（`Handle::NONE`）。
- slot 被釋放時 generation + 1（255 之後回到 1，跳過 0）。舊的值再拿來用 → 版次不符 → `BadHandle`，不會誤指到這個 slot 的新物件（ABA 問題）。

### 3.2 `HandleTable<T>`（`kernel/src/object/handle.rs`）

```
 slots  [0] gen 1  Port   SEND|RECEIVE|DUPLICATE|TRANSFER   ← 主 port
        [1] gen 1  Port   SEND|DUPLICATE|TRANSFER           ← console
        [2] gen 1  （空，保留）                              ← 沒有 bootstrap
        [3] gen 2  Memory READ|WRITE|MAP|...                ← 重用過的 slot
 free   [..]  可重用的 slot，後進先出
```

- `Vec<Slot>` + 空閒清單；上限 1024 個（`OutOfHandles`）。對物件型別是泛型，host 用 `u32` 測。
- `with_initial`：初始 handle 依序放在 slot 0、1、2；**空的初始 slot 保留不重用**，所以 `bootstrap::BOOTSTRAP` 這個值不會指到別的東西。
- `get_with(h, rights)`、`remove(h)`、`duplicate(h, rights)`（原 handle 要有 `DUPLICATE`，新權限必須是子集，否則 `AccessDenied`）、`room()`、`drain()`（行程結束）。
- 每個 `Process` 有一個 `SpinLock<HandleTable<KernelObject>>`。

### 3.3 `abi::Rights`

| 位元 | 名稱 | 用途 |
|---|---|---|
| 0 | `DUPLICATE` | `HandleDuplicate` |
| 1 | `TRANSFER` | 可以夾在訊息裡送出 |
| 2 | `SEND` | `PortSend`、`InputAttach` |
| 3 | `RECEIVE` | `PortReceive` |
| 4 | `READ` | 映射記憶體（唯讀或可寫都要） |
| 5 | `WRITE` | 可寫映射 |
| 6 | `MAP` | `MemoryMap` |
| 7 | `WAIT` | 保留（P6 的 `ProcessWait`；P11 的 `WaitSetWait`） |
| 8 | `KILL` | `ProcessKill`（P6） |
| 9 | `MANAGE` | `FileSetInfo`（P11）：改檔案與資料夾的中繼資料；`VolumeOpen` 與可寫卷宗的根有，`DirOpen` 從有它的資料夾往下繼承，`HandleDuplicate` 可以拿掉 |

未定義的位元 → `InvalidArgument`。

### 3.4 `KernelObject`（`kernel/src/object/mod.rs`）

```rust
pub enum KernelObject {
    Port(Arc<Port>),
    Memory(SharedMemory),   // 內部就是 Arc，clone 是共享
}
```

物件靠參考計數活著：handle 表、排隊中的訊息、kernel 的結構（例如 console 服務、input-router 的目標）各持有一份；最後一份放掉時釋放。syscall 的檢查順序：handle 不存在 → `BadHandle`；物件型別不對 → `WrongType`；權限不夠 → `AccessDenied`。

## 4. port 與訊息

### 4.1 `abi::Message`（144 bytes，`#[repr(C)]`）

```
 offset  0  tag            u32   訊息的意思（abi::tag）
         4  len            u32   data 的有效長度（≤ 112）
         8  handles        u32   handle_slots 的有效個數（≤ 4）
        12  reserved       u32   0
        16  data           [u8; 112]
       128  handle_slots   [Handle; 4]
       144  （大小）
```

計畫原本寫 128 bytes 的結構再加 handle；實際把 handle 放在尾端，整個 144 bytes。`abi` 用編譯期斷言與 host 測試鎖住位移（`to_bytes`／`from_bytes` 是 kernel 複製使用者記憶體時用的小端序編碼，和 repr(C) 在 x86-64 上一致）。

### 4.2 `Port`（`kernel/src/object/port.rs`）

```
 送方 ──PortSend──┐                         ┌──PortReceive── 收方
 送方 ──PortSend──┼──► [msg][msg][msg] ─────┤  （空就等，可以逾時）
 kernel ──send────┘     最多 64 則           │
                                            └── waiters：WaitQueue
```

```rust
pub struct Port { owner: ProcessId, inner: SpinLock<Inner> }
struct Inner { queue: VecDeque<QueuedMessage>, waiters: WaitQueue, closed: bool }
pub struct QueuedMessage { msg: Message, objects: Vec<Entry> }  // 夾帶的物件跟著排隊
```

- **N:1**：一個 port 物件，送與收靠權限區分。`PortCreate` 回 `SEND|RECEIVE|DUPLICATE|TRANSFER`，要給別人投信就 `HandleDuplicate(h, SEND|TRANSFER)` 再送過去。
- **送**：佇列滿（64）→ `Full`，**不阻塞**；關閉 → `PeerClosed`；否則放進佇列尾、從 waiters 取一個叫醒。
- **收**（`receive(deadline, room)`）：

```
 loop:
   被 kill 了？→ 放棄（NotSupported；這個值不會回到使用者模式）
   鎖住：佇列有東西 → 夾帶的 handle 數 > room？→ OutOfHandles（訊息留著）
                    → 取出回傳
         已關閉 → PeerClosed
         到期了 → TimedOut
         把自己加進 waiters
   block_until(deadline)        ← 送方的 wake 或時間到
   把自己從 waiters 移除
   （回到 loop：訊息可能被別的收方搶走、可能到期、可能被 kill）
```

- 逾時：`timeout_ns = 0` 是「不等」（deadline = 現在）；`u64::MAX` 是永遠等；其他是 `now + timeout`。
- **擁有者**：port 記住建立它的行程 id（kernel 的是 0）。行程結束時（最後一個執行緒被回收）`Process::release` 把它的 handle 全部放掉，並把它建立的每個 port `close()`：之後送 → `PeerClosed`；收的一方（若接收權限已經轉給別的行程）先收完剩下的訊息，再得到 `PeerClosed`；在等的收方會被叫醒。

### 4.3 handle 的轉移

```
 PortSend：                                    PortReceive：
 1. 讀進 144 bytes，len ≤ 112、handles ≤ 4     1. 要 RECEIVE；msg 指標可寫
 2. port 可以收嗎（check_send）                 2. room = min(表的空位, 4)
 3. 夾帶的 handle：不重複、都在表裡、            3. 取訊息（夾帶數 > room → OutOfHandles，
    都有 TRANSFER（任何一個不行就整個失敗，         訊息留在佇列）
    表不動）                                    4. 物件放進自己的表，新值填回 handle_slots
 4. 從表移除，物件放進 QueuedMessage            5. 寫回使用者記憶體
 5. 排進佇列
```

- 送出後，送方原本的 handle 值失效（`BadHandle`）——`ipc_client` 檢查這一點。
- 訊息沒被收就被丟掉（port 被 drop）時，夾帶的物件隨 `QueuedMessage` 一起放掉。
- 兩個邊界情況（文件化的簡化）：(a) 第 2 步與第 5 步之間另一個送方把佇列送滿，夾帶的 handle 會隨失敗的訊息被放掉；(b) 收方第 2 步算好空位後、第 4 步之前，同行程的另一個執行緒把表填滿，多出的物件被放掉、回 `OutOfHandles`。

## 5. 共享記憶體

### 5.1 物件

`frame::mm::shared::SharedMemory`：一組清零的頁框（不必連續），`Arc` 包著；最後一份 clone 放掉時頁框還給 PMM。大小是 4 KiB 的倍數，上限 64 MiB（`abi::MEMORY_MAX`），建立時就全部配好（eager）。

### 5.2 映射（`AddressSpace::map_shared`）

```
 行程 A 的頁表                     SharedMemory（3 頁）            行程 B 的頁表
 0x1000_0000_0000 ─┐              ┌── 頁框 0x1234_5000 ──┐         ┌─ 0x1000_0000_0000
 0x1000_0000_1000 ─┼──────────────┼── 頁框 0x0FF0_2000 ──┼─────────┼─ 0x1000_0000_1000
 0x1000_0000_2000 ─┘              └── 頁框 0x2001_7000 ──┘         └─ 0x1000_0000_2000
   葉子 PTE 帶 bit 9（SHARED）                                       各自還持有一份 clone
```

- 共享頁的葉子 PTE 多設 **bit 9**：Intel SDM Vol. 3A Table 4-19 的 bit 11:9 是處理器忽略、給軟體用的位元。位址空間被丟掉時 `free_tables` 看到 bit 9 就**不**把那個頁框還給 PMM（它屬於 `SharedMemory`），只釋放分頁表。
- `AddressSpace` 自己記一份 `(起點, SharedMemory)` 清單，所以只要還映射著，頁框就不會被釋放（frame 層保證，kernel 不必小心）。`unmap_shared(起點)` 清掉 PTE 並交回那份 clone。
- 一般的 `unmap` 拒絕共享頁；`map` 失敗時把頁框還給 PMM（以前會漏掉）。
- 可執行映射不允許（`UserFlags::executable = false`）。

### 5.3 映射區（`abi::layout`）

```
 0x0000_0000_0040_0000   程式
 0x0000_1000_0000_0000   MAP_BASE：MemoryMap 從這裡往上找
       …                 每段之後留一頁不映射（guard），越界就 page fault
 0x0000_7000_0000_0000   MAP_END
 0x0000_7FFF_FFDF_F000   堆疊的 guard 頁
```

`kernel::object::regions::Regions`：`BTreeMap<起點, 大小>`，first-fit，每段後面多算一頁 guard；kernel 選位址，程式不能指定。

### 5.4 syscall

| 呼叫 | 檢查 | 結果 |
|---|---|---|
| `MemoryCreate(size)` | size > 0、4 KiB 倍數、≤ 64 MiB | handle：`READ|WRITE|MAP|DUPLICATE|TRANSFER` |
| `MemoryMap(h, flags)` | `MAP|READ`，`map::WRITE` 再要 `WRITE`；未定義的 flag → `InvalidArgument` | 位址；區間用完 → `AlreadyMapped` |
| `MemoryUnmap(addr)` | addr 必須是某次 `MemoryMap` 回傳的起點 | 否則 `InvalidArgument` |

libstela 的 `ipc::Mapping` 包住映射：`read(offset, buf)`／`write(offset, data)` 以複製存取（另一個行程隨時可能寫同一塊，所以不交出 Rust 參考），drop 時自動 unmap。

## 6. 初始 handle（`abi::bootstrap`）

| slot | 常數 | 內容 |
|---|---|---|
| `[0]` | `MAIN_PORT` = `0x0100_0000` | 自己的主 port（`RECEIVE|SEND|DUPLICATE|TRANSFER`），由 `Process` 建立並持有 |
| `[1]` | `CONSOLE` = `0x0100_0001` | console 服務的 `SEND|DUPLICATE|TRANSFER` |
| `[2]` | `BOOTSTRAP` = `0x0100_0002` | 啟動者給的 handle（`program::spawn_process(.., Some(entry))`）；沒有就空（`BadHandle`） |

P5 加了 `[3]`（`CWD`，工作目錄，`docs/vfs.md` §5）；P6 起 `ProcessSpawn` 由父行程指定 `[1]`–`[3]`（`docs/userland.md` §2），主 port 的第一則訊息是 `BOOTSTRAP_ARGS`（參數）。P11 起最多 `[1]`–`[6]`（`[4]` `PROGRAM` 程式所在的資料夾、`[5]` `STDIN`、`[6]` 保留；`bootstrap::COUNT` = 7），第二則訊息是 `CAPABILITIES`（`docs/userland.md` §2.3）。

## 7. console 服務（`kernel/src/console.rs`）

- kernel 執行緒 `console` 擁有一個 port（擁有者 0），`receive(None, 4)` 迴圈：`tag == CONSOLE_WRITE` 的資料以 UTF-8 印到 `kprint!`（無效序列印成 U+FFFD），其他 tag 忽略、夾帶的 handle 放掉。
- `kernel_main` 在 `task::init` 之後、任何行程之前 `console::start()`。
- libstela：`console_write(bytes)` 切成 112 bytes 一則送到 `[1]`；`Full` 時 `yield_now()` 再送（不丟字）。`debug_print!`／`debug_println!` 先格式化進 256 bytes 的緩衝區（一行通常是一則訊息，不會被別的執行緒插進來）。
- 輸出是**非同步**的：訊息排進佇列，console 執行緒輪到時才印。因為 `send` 會把 console 執行緒放進 ready 佇列尾，程式結束後 kernel 印的「exited with code」通常在它的輸出之後（測試依賴這個順序，至今穩定）。
- `DebugWrite`（編號 6）移除：編號保留不用，`Syscall::from_raw(6)` 回 `None` → `InvalidSyscall`。

## 8. 鍵盤事件（`kernel/src/input.rs`）

```
 IRQ 1 ─► frame::input 佇列 ─► notify hook：wake(input-router)
                                       │
          input-router：pop ───────────┘──► port.send(INPUT_KEY 訊息)
```

- 中斷處理程式不碰 port（不必在中斷裡拿 port 的鎖），只叫醒 `input-router` 執行緒；它取出事件、編成 `KeyEvent::encode()`（4 bytes：KeyCode u16 LE、pressed、modifiers 位元）送到目前的目標。
- `InputAttach(h)`：h 要 `SEND`；一次一個訂閱者，後者取代前者；送滿就丟；目標 port 關閉就清掉。
- `KeyCode`／`Modifiers`／`KeyEvent`／`key_to_char` 搬到 `abi::key`（frame 的解碼器與 libstela 共用）；KeyCode 的編碼是 `KeyCode::NAMED` 的索引（只能往後加），未命名的鍵 = `0x8000 | 掃描碼`。
- `user/echo`：attach 主 port、印 `echo: ready…`、收事件、`key_to_char`、印字（控制字元以 `^X`），Enter 印 `uptime:`。kernel 的 echo 迴圈與 `kernel: ready` 刪除。P6 起開機後是 shell（`docs/userland.md`），echo 用 `run echo` 跑、Escape 結束（P11 K33 起程式改名 `keyecho`：`echo` 是 `Initrd:/bin/echo`，印參數）。
- selftest 不啟動 input-router（鍵盤測試直接讀 `frame::input`）；鍵盤事件的投遞由 `make test-fb` 與 `make test-shell` 經 QMP 打字測（P4–P5 是 echo，P6 起是 shell）。

## 9. 等待與喚醒

### 9.1 `WaitQueue`（`kernel/src/task/queue.rs`）

執行緒 id 的 FIFO：`add`（不重複）、`pop`、`remove`、`drain`。純邏輯，host 測試。

### 9.2 `block_until(deadline)`（`kernel/src/task/sched.rs`）

```
 deadline = None   → block()（在 blocked 清單）
 deadline = Some(t)→ 狀態 Sleeping(t)、wakeable = true，進睡眠清單
     tick 到期：timed_out = true，回 ready            → 回傳 false
     wake(id)：從睡眠清單拿出（只拿 wakeable 的），回 ready → 回傳 true
     已經有 wake_pending：不睡，直接回傳 true
```

執行緒同時「在睡眠清單」又「登記在 port 的 WaitQueue」：先到的一方把它放回 ready，另一方的登記由 `receive` 醒來後自己 `remove`。`sleep_ns` 的普通睡眠不是 wakeable，`wake` 不會提早叫醒它。

### 9.3 被 kill 時

`task::kill` 會把被 kill 行程的 blocked／sleeping 執行緒搬回 ready（`docs/process.md` §14）。阻塞點醒來後**先**檢查 `task::current_killed()`，是就放棄等待、讓 syscall 返回，執行緒在回 ring 3 的路上（`user_return`）結束。selftest 的 "ipc programs" 對一個停在 `PortReceive` 的 `ipc_server` 做 kill：exit code 99、沒有 Zombie、PMM 頁框數回到原值。

## 10. syscall（編號接 P3）

| 編號 | 名稱 | 參數 | 回傳 |
|---|---|---|---|
| 7 | `HandleClose` | h | 0 |
| 8 | `HandleDuplicate` | h, rights | 新 handle |
| 9 | `PortCreate` | — | handle |
| 10 | `PortSend` | port, &Message | 0 |
| 11 | `PortReceive` | port, &mut Message, timeout_ns | 0 |
| 12 | `MemoryCreate` | size | handle |
| 13 | `MemoryMap` | memory, flags | 位址 |
| 14 | `MemoryUnmap` | addr | 0 |
| 15 | `InputAttach` | port, input（P11：`capability::INPUT`） | 0 |

錯誤（接 P3 的 1–5）：6 `BadHandle`、7 `AccessDenied`、8 `WrongType`、9 `Full`、10 `TimedOut`、11 `PeerClosed`、12 `OutOfHandles`、13 `AlreadyMapped`。

## 11. 測試

| 測試 | 內容 |
|---|---|
| host `abi` | 編號與錯誤值、Handle 的位元配置與 bootstrap 值、Rights 位元、Message 的 layout（144、data 在 16、handle 在 128）與編碼往返、KeyEvent 編碼往返、`key_to_char` |
| host `kernel::object::handle` | 插入／取得／移除、generation 換版與 255→1、權限檢查與 duplicate 只能減少、1024 上限、初始 slot 保留 |
| host `kernel::object::regions` | first-fit、guard 頁、空洞重用、區域滿 |
| host `kernel::task::queue` | WaitQueue 的順序、不重複、remove、drain |
| selftest "system calls" | 重新組譯的內建程式：經 `PortSend` 到 `[1]` 印字（kernel 指標、未映射指標 → `BadAddress`）、編號 6 與 999 → `InvalidSyscall`、`HandleClose(0)` → `BadHandle` |
| selftest "ports" | kernel 內：送滿 64 → `Full`、FIFO、不等與 5 ms 逾時（至少 5 ms）、永遠等與長 deadline 的收方都被送方提早叫醒、close 叫醒收方（`PeerClosed`）、關閉後先收完剩下的 |
| selftest "ipc programs" | `ipc_server` + `ipc_client`（bootstrap = server 主 port 的 SEND）：請求／回覆（夾回覆 port）、送出的 handle 失效、權限（沒有 SEND → `AccessDenied`、duplicate 不能加權限、壞 handle）、送滿 → `Full`、逾時（0 與 10 ms）、共享記憶體 3 頁往返（server map、加總、寫回最後一個 byte）、記憶體的錯誤參數、server 結束後送 → `PeerClosed`；跑兩次，第二次前後 PMM 頁框數相同；再對停在 `PortReceive` 的 server 做 kill |
| `make test-fb` | 一般 kernel：P4–P5 是 hello → echo，打 `h i ret`，看到 `uptime:`；P6 起打進 shell（`docs/userland.md` §8） |

## 12. 已知限制與待辦

- P11 加了 WaitSet（§13）同時等多個物件；沒有「收到回覆才返回」的 call 語意（client 自己建回覆 port）。
- 訊息夾帶自己所在 port 的 handle 會形成 `Arc` 循環：只要那則訊息沒被收，port 就不會被釋放（擁有者結束時只 close、不清佇列）。
- §4.3 的兩個競爭邊界會放掉夾帶的 handle。
- console 服務任何持有 `[1]` 的程式都能印；輸出是非同步的。
- P7：`PortSend` 與 `InputAttach` 的目標也可以是 `Tty`（`docs/linux.md` §5）。
- P11：`InputAttach` 要 `INPUT` 能力（`docs/userland.md` §2.3），只有 init、視窗伺服器、文字模式的 shell 與它 run 的程式有。
- 共享記憶體是 eager（建立時配好全部頁框），沒有需求分頁；映射區不回收空的中間分頁表。
- 行程表的 `Exited` 狀態仍然永遠不清（P6 的 Process handle 再處理）。

## 13. WaitSet：同時等多個物件（P11）

> P11 由自動排程一次做完，**使用者尚未審核**。

### 13.1 比喻：總機的燈板

以前一個執行緒只能守著一個信箱（`PortReceive`）；要同時等鍵盤與 tty 輸出，只好多開一個執行緒（terminal 的輸出執行緒）。WaitSet 是**總機的燈板**：把幾個信箱、插座、終端機、行程登記到板子上，每個配一個號碼（key）；坐在板子前面等，任何一盞燈亮就醒來，看是哪幾個號碼亮了，再去處理。

### 13.2 syscall

| 編號 | 名稱 | 參數 | 回傳 |
|---|---|---|---|
| 49 | `WaitSetCreate` | — | handle（`WAIT|DUPLICATE|TRANSFER`） |
| 50 | `WaitSetAdd` | set, handle, key, mask | 0；已在集合裡就更新 key 與 mask |
| 51 | `WaitSetRemove` | set, handle | 0；不在集合裡 → `NotFound` |
| 52 | `WaitSetWait` | set, &mut [Ready], max, timeout_ns | 就緒的個數（≥ 1）；逾時 → `TimedOut` |

`Ready` 是 16 bytes：key（u64）、events（u32）、0（u32）。`max` 最多 `wait::MAX_READY` = 64；集合最多 `wait::MAX_MEMBERS` = 256 個成員（`Full`）。`mask` 0 = 該物件允許的全部事件；未定義的位元 → `InvalidArgument`。

### 13.3 事件（`abi::wait`）

| 位元 | 名稱 | 誰會有 |
|---|---|---|
| 1 | `READABLE` | port 有訊息；socket 有資料（或串流結束）、listener 有連線；tty 的程式端有一行（或 Ctrl-C）；tty master 有輸出 |
| 2 | `WRITABLE` | socket 可以送；tty master（永遠）；音效的環至少一半是空的 |
| 4 | `CONNECTED` | TCP 連線已建立 |
| 8 | `CLOSED` | port 已關閉（擁有者結束；佇列裡還有訊息也算）；連線結束、被 reset 或失敗 |
| 16 | `EXITED` | 行程已結束；執行緒已結束（Thread handle，P11 K23） |

可以加入的物件：Port、Socket、Tty、TtyMaster、Process、Audio、PipeRead、PipeWrite、Thread；其他（包括 WaitSet 自己）→ `WrongType`。**只有 `SEND` 的 port 只能等 `CLOSED`**（mask 有別的位元 → `AccessDenied`）：用來發現對方行程結束（視窗伺服器用它看 client 的回覆 port，`docs/window.md` §3），但看不到別人信箱裡有沒有信。

### 13.4 kernel 端（`kernel/src/object/waitset.rs`）

```
 WaitSetWait ─► loop：
    被 kill 了？→ 放棄
    複製成員清單
    每個成員：watch(me)        ← 把自己加進物件的 watchers
    每個成員：events() & mask  ← 現在成立的事件
    有 → 每個成員 unwatch(me)，回傳
    到期 → unwatch，TimedOut
    block_until(deadline)      ← 任何成員一變就叫醒所有 watchers
    每個成員 unwatch(me)，回到 loop
```

- **先 watch 再看**：看完到睡之間如果有事件發生，wake 會留下 `wake_pending`，`block_until` 立刻返回（§9.2），不會漏掉。
- 每個物件的 watchers：Port 的 `Inner.watchers`（送信與關閉時全部叫醒；`waiters` 仍只叫醒一個收信的人）、Tty 的 `watchers`（輸出與可讀的輸入）、網路的 `net.waiters`（協定堆疊一有事件就全部叫醒，和 socket syscall 共用）、行程表的 `waiters`（結束時）、音效的 `WATCHERS`（餵資料的執行緒每 5 ms 看一次，環有一半空就叫醒）。
- 成員以 **Weak** 參考：集合不會讓 socket 保持開著或讓 port 活著；物件消失的成員在下一次 wait 時自動移除。**行程例外**：以 id 記（行程表的紀錄永遠在），結束後一直是 `EXITED`，要自己 `WaitSetRemove`（terminal 換新 shell 時就這樣做）。成員的身分用指標比較（`Weak::ptr_eq`；行程用 id），所以 `WaitSetRemove` 給同一個物件的任何 handle 都可以。
- 單一物件的阻塞呼叫（`PortReceive`、`TtyRead`、socket 的 recv…）不變。
- 一個執行緒一次只在一個 WaitSet 等（自然成立：它在 syscall 裡）。

### 13.5 libstela

`libstela::wait::WaitSet`：`new()`、`add(handle, key, mask)`、`remove(handle)`、`wait(&mut [Ready], timeout_ns) -> usize`，drop 時關閉 handle；事件常數 `wait::READABLE` 等。

### 13.6 測試

| 測試 | 內容 |
|---|---|
| host `abi` | 編號 49–52、`WAIT_SET` = 11、`wait::ALL` = 31、`Ready` 的編碼往返 |
| host `net` | `Stack::readiness`：連線中、已連線可寫、listener 有連線、有資料可讀、對方關閉後 `readable + closed`、UDP 可寫 |
| selftest（`proctest` 的第 15–22 步；P11 K23 加第 29–30 步：Thread handle 進 WaitSet 等到 `EXITED`） | port + tty master（`READABLE`）+ 子行程一起等：不等 → `TimedOut`；送信 → key 1；印到 tty → key 2；kill 子行程 → key 3 `EXITED`；移除兩次 → `NotFound`；另一個執行緒 5 ms 後送信叫醒正在等的主執行緒（至少等了 4 ms）；關掉的 port 自動移出；WaitSet 放進自己 → `WrongType`；未定義的 mask → `InvalidArgument` |

## 14. Pipe（P11，使用者尚未審核）

**比喻**：一條 64 KiB 的水管，一頭灌、一頭接。灌的人在水管滿時等；接的人在水管空時等；灌的人全走了，接的人把剩下的接完就知道「沒了」（讀到 0）；接的人全走了，灌的人會被告知「沒人要」（`PeerClosed`）。

| 編號 | 名稱 | 參數 | 回傳 |
|---|---|---|---|
| 54 | `PipeCreate` | &mut [u32; 2] | 0；寫入讀端（`READ|WAIT|DUPLICATE|TRANSFER`）與寫端（`WRITE|SEND|WAIT|DUPLICATE|TRANSFER`）的 handle |
| 55 | `PipeRead` | 讀端, buf, len（≤ 64 KiB）, timeout_ns | 讀到的 bytes；0 = 結束（所有寫端都關了而且空了）；`TimedOut` |
| 56 | `PipeWrite` | 寫端, buf, len, timeout_ns | 寫進去的 bytes（至少 1）；`PeerClosed`（沒有讀端）；`TimedOut` |

- kernel：`object/pipe.rs`——`Pipe`（`VecDeque<u8>`、`reader`／`writer` 還在不在、讀等待者、寫等待者、watchers），`ReadEnd(Arc<Pipe>)`／`WriteEnd(Arc<Pipe>)` 各是一個 kernel 物件（`KernelObject::PipeRead`／`PipeWrite`，`object_kind` 13、14）。同一端的 handle 共用同一個 `Arc<ReadEnd>`，最後一個 handle 放掉時 `Drop` 把那一端關掉、叫醒所有等待者。
- **`PortSend` 到寫端**：`CONSOLE_WRITE` 的文字寫進 pipe（滿了就等）——所以 `[1]` 是 pipe 寫端的 STELA 程式，`println!` 什麼都不用改（shell 的管線，`docs/userland.md` §6）。
- WaitSet：讀端 `READABLE`（有資料或結束）、`CLOSED`（寫端都關了）；寫端 `WRITABLE`（有空間）、`CLOSED`（讀端都關了）。
- libstela `pipe::create`、`read`、`write`、`write_all`。
- Linux：`pipe`（22）、`pipe2`（293，`O_CLOEXEC`、`O_NONBLOCK`）是 `OpenFile::PipeRead`／`PipeWrite`；`read`／`write`（非阻塞 → `EAGAIN`；沒有讀端 → `EPIPE`，沒有 SIGPIPE）、`poll`（`POLLIN`／`POLLHUP`、`POLLOUT`／`POLLERR`）、`fstat`（`S_IFIFO`）、`dup2`（既有的 fd 表）（`docs/linux.md` §4）。

### 14.1 測試

| 測試 | 內容 |
|---|---|
| host `abi` | 編號 54–56、`PIPE_READ`／`PIPE_WRITE`、`PIPE_BUFFER` |
| selftest（proctest 第 26–28 步） | 依序、空的時候逾時、寫端關掉後 WaitSet 看到 `READABLE|CLOSED` 而且讀到 0、沒有讀端時 `PeerClosed`、子行程的 `[1]` 是 pipe：它的 `debug_println!` 從讀端出來，結束後讀到 0 |
| `make test-linux` | `linux/pipe`：fstat、poll、依序讀寫、dup2、結束、EPIPE、`pipe2(O_NONBLOCK|O_CLOEXEC)` 的 EAGAIN 與 FD_CLOEXEC；`nosys` 不再測 pipe |
