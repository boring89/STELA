# Linux 相容層（Linux 人格）

> P7 由自動排程一次做完，使用者已於 2026-10-20 審核。

讓靜態連結的 x86-64 Linux 程式（musl）直接在 STELA 上跑：同一條 `syscall` 指令進 kernel，kernel 看行程的「人格」決定用哪一套號碼表。架構決定見 `docs/architecture.md` §4 的 H（Linux 只做 syscall 翻譯，先支援 musl 靜態程式）。STELA 自己的 ABI 見 `docs/syscall.md`。

程式碼：

```
abi/src/lib.rs                note（STELA ELF note）、spawn（FORCE_*）、tty、object_kind、編號 33–36
elfload/src/lib.rs            notes()、Layout::Packed、program_headers_address()
kernel/src/program.rs         人格判定、頁面權限合併、spawn_linux
kernel/src/task/process.rs    Personality、Process::linux()
kernel/src/linux/nr.rs        Linux 號碼（純邏輯）
kernel/src/linux/errno.rs     errno 與 abi::Error 的對應（純邏輯）
kernel/src/linux/fd.rs        FdTable（純邏輯）
kernel/src/linux/path.rs      合成根目錄的路徑（純邏輯）
kernel/src/linux/stack.rs     初始堆疊：argv、envp、auxv（純邏輯）
kernel/src/linux/state.rs     State、OpenFile
kernel/src/linux/sys.rs       分派與雜項 syscall
kernel/src/linux/files.rs     檔案 syscall
kernel/src/linux/memory.rs    brk、mmap、munmap、mprotect
kernel/src/object/ldisc.rs    行模式（純邏輯）
kernel/src/object/tty.rs      Tty 物件
kernel/src/vfs/tmpfs.rs       Tmp 卷宗（記憶體檔案系統，純邏輯 + 後端）
kernel/src/initrd.rs          Tree：initrd 的資料夾樹
frame/src/task/mod.rs         FS base（set_fs_base、switch_to）
user/libstela/src/tty.rs      tty 的包裝
linux/tests/*.c               測試程式（GPL-3.0-or-later）
```

## 1. 比喻：同一個櫃台，兩本手冊

櫃台（`syscall` 進入點）只有一個。STELA 的程式拿的是 STELA 的申請表（RAX = STELA 編號、結果分兩格），Linux 的程式拿的是 Linux 的申請表（RAX = Linux 編號、結果只有一格，負數代表錯誤）。承辦人看申請人的**識別證**（行程的人格）決定翻哪一本手冊。

識別證是在入場時發的：程式檔裡有一張 **STELA 貼紙**（ELF 的 `PT_NOTE`，名字 `STELA`）就是 STELA 程式，沒有就當成 Linux 程式。libstela 會自動把貼紙貼在每個 STELA 程式上。

Linux 程式以為自己在一台 Linux 電腦上：有 `/`、有檔案描述符 0、1、2、有 `/tmp`、有終端機。這些都是 kernel 在翻譯時「演」出來的：`/Data/a.txt` 其實是 Data 卷宗的 `a.txt`，fd 1 其實是一個 `Tty` 物件。

## 2. 人格（personality）

| | STELA | Linux |
|---|---|---|
| 判定 | ELF 有 `PT_NOTE`，其中 name `STELA`、type 1（`abi::note`） | 沒有 |
| 強制 | `ProcessSpawn` 的 count 參數高 32 位元：`abi::spawn::FORCE_STELA`（bit 33） | `FORCE_LINUX`（bit 32）；兩個都設 → `InvalidArgument` |
| 回傳 | RAX = 狀態、RDX = 值 | RAX = 值或 `-errno`；其他暫存器（含 RDX）不變（`SyscallFrame::set_rax`） |
| 參數 | 主 port 的 `BOOTSTRAP_ARGS` | 堆疊上的 argc／argv／envp／auxv（§6） |
| 狀態 | handle 表 | handle 表 + `linux::State`（fd 表、cwd、brk、tty） |

- note：namesz 6、descsz 4、type 1、`"STELA\0"` 補到 8、descriptor = 版本 1（u32 LE），共 24 bytes。libstela 的 `static STELA_NOTE`（`#[used]`、`link_section = ".note.stela"`），`user/user.ld` 用 `KEEP` 放在 rodata 段並給它自己的 `PT_NOTE` 標頭。
- kernel 自己的測試程式（selftest 手組的 ELF）一律用 `program::spawn`（強制 STELA）；`spawn_with(…, None)` 才自動判定。
- 分派：`kernel::syscall::handle` 一開始看 `current_process().personality()`，Linux 就交給 `linux::dispatch(frame, process)`（行程以值傳入：`exit` 不返回，不能在堆疊上留一份 `Arc`）。

## 3. 載入

```
 elfload::Elf::parse_with(image, Layout::Packed)
   Packed：段可以不對齊頁、可以和前一段共用一頁（GNU ld 的一般輸出），但位元組不重疊、要遞增
 每一頁的權限 = 涵蓋它的所有段的聯集（R + RW → RW）；聯集是 W+X → 拒絕（BadSegment）
 先把所有頁映射成清零的頁框，再把各段的檔案內容複製進去
```

musl 的程式（`-static -no-pie -Wl,-z,max-page-size=4096`）是四個 PT_LOAD：標頭 R、text R-X、rodata R、data RW（從頁中間開始，RELRO）。

## 4. 號碼表（`linux/nr.rs`，Linux 的 `syscall_64.tbl`）

| 類別 | 號碼 | 做法 |
|---|---|---|
| 結束 | `exit` 60、`exit_group` 231 | `exit_process(code & 0xFF)`（單執行緒，兩者相同） |
| 檔案 | `read` 0、`write` 1、`open` 2、`close` 3、`stat` 4、`fstat` 5、`lstat` 6、`lseek` 8、`readv` 19、`writev` 20、`access` 21、`dup` 32、`dup2` 33、`fcntl` 72、`getcwd` 79、`chdir` 80、`rename` 82、`mkdir` 83、`rmdir` 84、`unlink` 87、`getdents64` 217、`openat` 257、`mkdirat` 258、`newfstatat` 262、`unlinkat` 263、`renameat` 264、`faccessat` 269 | §5、§7 |
| 連結 | `readlink` 89、`readlinkat` 267 | 永遠 `ENOENT`（沒有符號連結） |
| 等待 | `poll` 7、`select` 23、`pselect6` 270、`ppoll` 271 | §5.3（P11） |
| 終端機 | `ioctl` 16 | tty：`TCGETS`、`TCSETS(W/F)`（ICANON／ECHO／ISIG）、`TIOCGWINSZ`（25×80）、`TIOCGPGRP`／`TIOCSPGRP`；其他 fd → `ENOTTY` |
| 記憶體 | `mmap` 9、`mprotect` 10、`munmap` 11、`brk` 12 | §8 |
| 訊號 | `rt_sigaction` 13、`rt_sigprocmask` 14 | 只回 0（舊值讀成預設）；訊號永遠不遞送 |
| 時間 | `nanosleep` 35、`gettimeofday` 96、`clock_gettime` 228、`clock_nanosleep` 230 | REALTIME = `wall_clock_ns()`（`docs/time.md` §5.5）；MONOTONIC、BOOTTIME、CPUTIME 等 = `now_ns()` |
| 身分 | `getpid` 39、`gettid` 186（都是行程 id）、`getppid` 110（固定 1）、`getuid` 102 等（0）、`uname` 63 | sysname `STELA`、nodename `stela`、release = 版本、machine `x86_64` |
| TLS | `arch_prctl` 158、`set_tid_address` 218 | §9 |
| 同步 | `futex` 202 | `FUTEX_WAIT`：值不同 → `EAGAIN`；相同 → 睡到逾時（沒有逾時就睡到被 kill）；`FUTEX_WAKE` → 0 |
| 亂數 | `getrandom` 318 | splitmix64 混合時鐘——**不安全**，不可當密碼學亂數 |
| 不做 | `pipe` 22、`pipe2` 293、`clone` 56、`fork` 57、`vfork` 58、`execve` 59、`clone3` 435 | `ENOSYS` |
| 其他 | | `ENOSYS`，每個號碼第一次印一行 `linux: process N (name): system call X is not supported (ENOSYS)` |

errno（`linux/errno.rs`）與 `abi::Error` 的對應：`NotFound` → `ENOENT`、`Exists` → `EEXIST`、`NotDir` → `ENOTDIR`、`IsDir` → `EISDIR`、`NotEmpty` → `ENOTEMPTY`、`ReadOnly` → `EROFS`、`NoSpace` → `ENOSPC`、`IoError` → `EIO`、`BadAddress` → `EFAULT`、`InvalidArgument` → `EINVAL`、`OutOfMemory` → `ENOMEM`……

## 5. 終端機：`Tty` 物件（`object/tty.rs`、`object/ldisc.rs`）

```
   鍵盤（InputAttach 到 tty）／master 的 TtyWrite／PortSend INPUT_KEY
        │
        ▼
   行模式（ldisc）：編輯、echo、^C ──► 可讀的位元組 ──► Linux read(0)
        │ echo
        ▼
   輸出 ◄── Linux write(1)、PortSend CONSOLE_WRITE（STELA 程式的 [1]）
        ├─ 沒有 master：直接印到 kernel 主控台（kprint!）
        └─ 有 master：存著等 master 的 TtyRead（終端機程式，P8）
```

- **canonical 模式**（預設，ICANON）：可見字元加進這一行並 echo；Backspace（0x08、0x7F）刪一個字元（UTF-8）並 echo `"\b \b"`；Ctrl-U 清整行；Enter（`\r` 或 `\n`）把這一行加 `\n` 變成可讀；Ctrl-D：行是空的 → 一次 end of file（`read` 回 0），否則把這一行（不加 `\n`）變成可讀；其他控制字元忽略（Tab 保留）。一行最多 4096 bytes，可讀的最多 16 KiB。
- **raw 模式**（`TCSETS` 清掉 ICANON）：每個字元立刻可讀。
- **Ctrl-C**（ISIG）：清掉這一行、echo `^C\n`、把登記在這個 tty 的 Linux 行程以 `EXIT_KILLED`（137）結束。
- `read` 阻塞到有東西可讀（一次最多一行）；被 kill 時放棄。
- STELA 程式用 tty 當 `[1]` 時什麼都不用改：libstela 的 `console_write` 送 `CONSOLE_WRITE` 給 `[1]`，kernel 看到是 tty 就印（**同步**，不像 console 服務是非同步的）。

### 5.1 syscall（接 P6.5 的 32）

| 編號 | 名稱 | 參數 | 回傳 |
|---|---|---|---|
| 33 | `TtyCreate` | flags（`abi::tty::MASTER`） | 低 32 位元 = 程式端 handle（`SEND|DUPLICATE|TRANSFER`），高 32 位元 = master handle（`READ|WRITE|DUPLICATE|TRANSFER`；沒有 MASTER 時是 0） |
| 34 | `TtyRead` | master, buf, len（≤ 4096）, timeout_ns | 讀到的輸出 bytes；`TimedOut` |
| 35 | `TtyWrite` | master, buf, len（≤ 4096） | 當成打字（UTF-8）送進行模式 |
| 36 | `HandleKind` | handle | `abi::object_kind`：1 port、2 memory、3 資料夾、4 檔案、5 行程、6 tty、7 tty master |

- `PortSend` 的目標可以是 tty（要 `SEND`）：`CONSOLE_WRITE` → 輸出，`INPUT_KEY` → 按鍵轉成字元（`key_to_char`）送進行模式；不收夾帶的 handle（`InvalidArgument`）。
- `InputAttach` 的目標可以是 tty：input-router 把每個鍵的字元送進它（`kernel::input::Target::Tty`）。
- libstela：`tty::create()`、`tty::create_with_master()`、`tty::read`、`tty::write`、`ipc::handle_kind`。

### 5.3 `poll`／`select`（P11，使用者尚未審核）

`kernel/src/linux/poll.rs`，用 kernel 的 WaitSet（`docs/ipc.md` §13）：

```
 loop：
   每個 fd 現在的事件：tty（POLLIN 有一行可讀、POLLOUT 永遠）、
                      socket（READABLE → POLLIN、WRITABLE → POLLOUT、CLOSED → POLLHUP）、
                      其他（檔案、資料夾、/dev、/etc）永遠 POLLIN|POLLOUT
   fd < 0 → 略過；不存在的 fd → POLLNVAL（select 則整個 EBADF）
   有 → 寫回 revents，回傳個數
   時間到 → 0
   把 tty 與 socket 放進一個臨時的 WaitSet，等到有變化或時間到
```

- `poll` 的負逾時 = 永遠；`ppoll`／`pselect6` 用 timespec，signal mask 忽略；`select` 的 timeval 不寫回剩餘時間。最多 1024 個 fd。
- 訊號永遠不遞送，所以只有被 kill 時才有 `EINTR`（行程正在結束）。
- 沒有可以等的物件（全是檔案）又沒有就緒的 → 每 100 ms 醒一次看是否被 kill，直到時間到。

### 5.2 shell 怎麼跑程式

```
 run prog args
   [1] 是 tty？（HandleKind）── 是：沿用（在終端機程式裡，P8）
                              └ 否：TtyCreate(0)，沒有 master，輸出直接印
   ProcessSpawn(prog, args, [tty, 無, 目前資料夾])
   每 20 ms：ProcessWait 逾時 → 看主 port 的按鍵：
       Ctrl-C → ProcessKill（shell 自己處理，不送進 tty）
       其他   → PortSend(INPUT_KEY) 到 tty（Linux 程式從 fd 0 讀到）
```

Linux 程式的 tty：kernel 在 `spawn_linux` 時看 `[1]`：是 Tty 就用它，否則自己建一個沒有 master 的（kernel 自己啟動的 Linux 程式）。fd 0、1、2 都指向同一個 `OpenFile::Tty`；`/dev/tty` 也是它。tty 把行程登記起來（Ctrl-C 用）。

## 6. 初始堆疊（`linux/stack.rs`，System V ABI AMD64 §3.4.1）

```
 STACK_TOP ─► 字串："prog\0a\0b\0" "HOME=/Data\0" ...
              16 bytes 亂數                 ← AT_RANDOM
              （補到 16 對齊）
              AT_NULL 0
              AT_RANDOM ptr、AT_SECURE 0、AT_EGID 0 … AT_PAGESZ 4096、AT_PHNUM、AT_PHENT、AT_PHDR
              0
              envp[]                         HOME=/Data、PATH=/Initrd/linux:/Initrd、TERM=dumb、PWD=<cwd>
              0
              argv[]                         來自 BOOTSTRAP_ARGS（資料或 Memory 物件）
 rsp ───────► argc                           16 對齊
```

- `AT_PHDR` = 含有程式標頭表的 PT_LOAD 的位址 + (e_phoff − p_offset)（`Elf::program_headers_address`）；musl 靠它找 `PT_TLS`。
- 堆疊和 STELA 程式一樣：1 MiB，頂端 `STACK_TOP`，下面一頁 guard。

## 7. 路徑：合成的根目錄（`linux/path.rs`、`linux/files.rs`）

```
 /                  合成的：每個卷宗一個資料夾（Tmp 卷宗顯示成 tmp）、dev
 /Data/docs/a.txt   Data 卷宗的 docs/a.txt
 /Initrd/linux      Initrd 卷宗的 linux 資料夾（initrd 的 ustar 子資料夾，§10）
 /tmp/x             Tmp 卷宗的 x（記憶體檔案系統，§7.1）
 /dev               null、zero、tty
 /etc/resolv.conf   合成的（P11）：nameserver <DHCP 給的 DNS>（還沒有位址時是空檔）
```

- `.`、`..` 在 kernel 以字面處理（`normalize(cwd, path)`，`..` 在根停住）；沒有符號連結。
- 相對路徑相對於 cwd，或 `*at` 的 dirfd（它的 `OpenFile::Dir` 記著自己的 Linux 路徑）；`AT_FDCWD` = −100。
- cwd 一開始是 `[3]` 那個資料夾（P11：包括它在卷宗裡的子路徑，`vfs::path_of`；shell 在 `Data:/t` 裡 `run` 時 cwd 是 `/Data/t`）；沒有 `[3]` 時 `/Data`，再沒有 `/Initrd`。
- **P11：合成根只顯示 `[3]` 與 `[4]`（程式所在資料夾）的卷宗**，加上 `tmp`、`dev`、`etc`；其他卷宗（`Boot`、`Archive`…）看不到也打不開（`ENOENT`；`linux::State::volumes`）。沒有 `[3]`/`[4]` 時只有 cwd 的卷宗。shell 從 `Data:/` 跑 `linux/files` 時看得到 `Data`（`[3]`）與 `Initrd`（`[4]` = `Initrd:/linux`）。
- `open`：`O_CREAT`（`O_EXCL` → `EEXIST`）、`O_TRUNC`、`O_APPEND`、`O_DIRECTORY`、`O_CLOEXEC`；資料夾以寫入開 → `EISDIR`；唯讀卷宗寫入 → `EROFS`；在 `/` 或 `/dev` 建新名字 → `EACCES`。
- `rename`：同一卷宗（否則 `EXDEV`）；目標是檔案就先刪掉（Linux 的取代語意），是資料夾 → `EISDIR`。
- `stat`：`st_dev` = 卷宗 index + 1、`st_ino` = 持久 ID、mode 資料夾 `040755`、檔案 `100644`、裝置與 tty `020666`／`020620`；時間來自後端。
- `getdents64`：`.`、`..` 在前；每次呼叫重新列出（和 `DirRead` 一樣 O(n²)）；cursor 存在 `OpenFile::Dir`；`lseek(fd, 0, SEEK_SET)` 重來。
- `/etc` 只有 `resolv.conf`（唯讀，`open` 時才產生內容；寫入 → `EROFS`）；其他名字 → `ENOENT`（musl 找 `/etc/hosts` 時就是這樣，然後走 DNS）。
- P11：`socket()` 要行程啟動時拿到 `NETWORK` 能力（`docs/userland.md` §2.3），否則 `EACCES`。
- Linux 程式在看得到的卷宗裡不受 handle 權限限制（P11 前看得到所有卷宗）；之後的沙箱可以再收緊到 `[3]` 那個資料夾以下（`docs/architecture.md` §4.1）。

### 7.1 `Tmp`（`vfs/tmpfs.rs`）

kernel 記憶體裡的檔案系統，也是一個一般卷宗（`vols` 看得到：`Tmp memory rw`，`abi::volume_kind::TMP` = 4）。`TmpTree`：`BTreeMap<id, Node>`，檔案是 `Vec<u8>`，全部檔案合計 ≤ 16 MiB（`NoSpace`）；名稱大小寫保留、比較不分 ASCII 大小寫；不能把資料夾搬進自己裡面。關機就沒了。

### 7.2 fd 表（`linux/fd.rs`）

`FdTable<Arc<OpenFile>>`：最低可用號碼、上限 256（`EMFILE`）；`dup`／`dup2`／`fcntl(F_DUPFD…)` 共用同一個 `OpenFile`（所以共用 offset，和 Linux 一樣）；`FD_CLOEXEC` 每個 fd 自己的（沒有 execve，只是存著）。`OpenFile`：`Tty`、`Null`、`Zero`、`File { node, offset, append, readable, writable, flags }`、`Dir { node, path, cursor }`。fd 直接持有 VFS 的 `Node`（不經 handle 表）。行程結束時 `State::release()` 關掉全部。

## 8. 記憶體（`linux/memory.rs`）

```
 0x40_0000           程式（PT_LOAD）
 程式結尾（頁對齊）   brk 區：brk(addr) 往上長、往下縮，最多 256 MiB
 MAP_BASE 起         mmap 匿名映射（和 MemoryMap 共用 Regions，每段後一頁 guard）
 STACK_TOP 以下       1 MiB 堆疊
```

- `brk(0)` 或範圍外 → 回目前的 break（失敗的表示法）；長大時逐頁映射清零頁框（配不到就不動），縮小時釋放。musl 的 `sbrk(n)` 只接受 n = 0（musl 的設計），malloc（mallocng）用 brk 與 mmap。
- `mmap`：只接受 `MAP_PRIVATE|MAP_ANONYMOUS`（否則 `ENOSYS`），`MAP_FIXED` 不支援（`ENOSYS`），`PROT_EXEC` → `EACCES`；kernel 選位址，立刻映射全部頁（沒有需求分頁）；`PROT_NONE` 映射成唯讀（STELA 沒有「映射了但不能讀」的頁）。
- `munmap`：只在映射區內；釋放範圍內的頁；整段都放掉時把位址範圍還給 Regions（部分 munmap 只釋放頁，位址範圍留著）。
- `mprotect`：加執行權 → `EINVAL`；其他就改每一頁的可寫位元（`AddressSpace::protect`，新增）；沒映射的頁略過。

## 9. TLS：FS base

musl 啟動時 `arch_prctl(ARCH_SET_FS, tp)` 把執行緒指標放進 FS base（`%fs:0` 指向自己），`errno`、`__thread` 變數都從這裡找。

- `frame::task::set_fs_base(base)`：檢查 `base < USER_END`（非 canonical 的值會讓 `wrmsr` 在 kernel 裡 #GP），存進 `TaskContext.fs_base`，並立刻 `wrmsr IA32_FS_BASE`（0xC000_0100，Intel SDM Vol. 4 Table 2-2）。
- `switch_to`：切到有 FPU 區的（使用者）執行緒時 `wrmsr IA32_FS_BASE ← next.fs_base`（STELA 程式是 0），所以行程之間看不到彼此的 FS。切出時不讀回：CR4.FSGSBASE 沒開，使用者不能自己改（`wrfsbase` 會 #UD），context 裡的值永遠是對的。
- `ARCH_GET_FS` 讀 context 的值；`ARCH_SET_GS`／`GET_GS` → `EINVAL`。

## 10. initrd 的資料夾（`initrd.rs` 的 `Tree`）

P6 的 `InitrdFs` 把所有檔案放在根目錄；P7 起建一棵樹：成員路徑 `a/b/c` 變成資料夾 `a`、`a/b` 與檔案 `c`（不管 archive 有沒有目錄成員），id：根 1，其他依出現順序 2、3、……；同路徑後出現的檔案取代前面的。Makefile 打包時 `tar --sort=name … $(USER_PROGRAMS) linux`，所以 `Initrd:/linux/hello` 是 musl 的 hello。shell 的 `run linux/hello`：目前資料夾找不到時，非絕對、沒有卷宗名的路徑也到 `Initrd:/` 找。

## 11. musl 與測試程式

- `make musl`：把 68k_HAL 已下載的 `musl-1.2.5.tar.gz` **複製**到 `build/downloads/`（不下載、不改 68k_HAL；`MUSL_TARBALL=` 可指定別處），檢查 SHA-256 `a9a118bb…c75e4`（`third_party/README.md`），解開到 `build/musl/src`、`./configure --prefix=build/musl/install --target=x86_64 --disable-shared CC=gcc AR=ar RANLIB=ranlib`、`make`、`make install`（約 10 秒）。`--target=x86_64` 會讓 musl 用 `x86_64-ar` 這種跨平台工具名，所以明確指定 `AR`、`RANLIB`。
- `make linux-tests`：`musl-gcc -static -O2 -no-pie -fno-pie -Wall -Wextra -Werror -Wl,-z,max-page-size=4096 -Wl,--build-id=none` → `build/user/linux/`，打包進 initrd 的 `linux/`。
- CI：`actions/cache` 快取 tarball（key = SHA-256），沒有就從 musl.libc.org 抓一次並檢查 SHA-256，`MUSL_TARBALL` 指向它。

| 程式 | 測什麼 | 預期 |
|---|---|---|
| `hello` | printf、argv、uname、`__thread` | `hello from linux: 3 args`、`hello: uname STELA …`、`thread-local value 7`，exit 3 |
| `files` | P11：cwd 是 `/Data/linuxdir`（shell 先 `mkdir`、`cd`）、合成根沒有 `Archive`、`Boot`；`/tmp` 與 cwd：mkdir（兩次 → EEXIST）、建檔、write、lseek、read、fstat、close 兩次 → EBADF、rename、stat、getdents、rmdir 非空 → ENOTEMPTY、unlink、rmdir、access；合成根與 `/dev`；讀 `/Initrd/linux/hello` 的 ELF magic；唯讀卷宗 → EROFS；`/dev/null`；chdir、相對路徑、dup2 | `files: OK` |
| `memory` | 8 MiB malloc、2000 個小 malloc、brk 長大／縮回／太遠、mmap 1 MiB（清零、對齊）、mprotect、munmap、PROT_EXEC 被拒 | `memory: OK` |
| `time` | MONOTONIC + nanosleep(20 ms) ≥ 20 ms、REALTIME 年份 ≥ 2024、gettimeofday | `time: OK` |
| `tty` | isatty（0、1 是，/dev/null 不是）、tcgetattr 有 ICANON、讀一行 | `tty: you typed: …` |
| `nosys` | fork、pipe、execv → -1 且 errno = ENOSYS | `nosys: OK` |
| `poll`（P11） | stdout 立刻可寫、stdin 50 ms 沒東西（等滿）、`/dev/null`、fd −1 略過、關掉的 fd → POLLNVAL、`ppoll` 20 ms、`select`（可寫、30 ms 逾時、關掉的 fd → EBADF）、`pselect`；`/etc/resolv.conf` 是一般檔、內容 `nameserver …`、不能寫；最後 `poll(-1)` 等使用者打一行 | `poll: you typed: …`、`poll: OK` |

## 12. 測試

| 測試 | 內容 |
|---|---|
| host `elfload` | packed 段（共用頁、仍拒絕重疊）、notes 解析（截斷時停止）、程式標頭位址 |
| host `kernel::linux::*` | 號碼、errno 對應與編碼、fd 表（最低號碼、dup/dup2、滿、drain）、路徑（normalize、target、join）、初始堆疊的配置 |
| host `kernel::object::ldisc` | 行、編輯（Backspace、UTF-8、Ctrl-U、控制字元、Tab）、Ctrl-C、Ctrl-D、raw、上限 |
| host `kernel::vfs::tmpfs`、`kernel::initrd` | TmpTree（檔案、資料夾、容量、錯誤、改名迴圈）、initrd 的樹 |
| host `abi` | 編號 33–36、`note::BYTES`、`spawn::FORCE_*`、`tty::MASTER`、`object_kind` |
| selftest "linux personality" | hello 有 note、裸 ELF 沒有；手組的 Linux 機器碼：write 到 tty（`linux: hello from raw system calls`）、RDX 保留、fork → `-ENOSYS`、exit_group(38)，兩次 PMM 回復；強制 STELA 人格（同一段碼變成 `ThreadExit`，exit 0）；`Initrd:/linux/hello`（musl）exit 3，兩次 PMM 回復 |
| `make test-linux` | 一般 kernel + AHCI 測試磁碟：shell 依序 `run linux/hello a b`、`files`、`memory`、`time`、`nosys`、`tty`（打 `stela rocks`）、`poll`（打 `poll works`），檢查輸出與 exit code |
| `make test-net`（P10） | `run linux/net PORT host`：BSD socket 抓 host 的 HTTP 檔、`getsockname`／`getpeername`／`getsockopt`、UDP `sendto`、關著的 port → `ECONNREFUSED`、`AF_INET6` → `EAFNOSUPPORT`（`docs/net.md` §7） |

## 13. 已知限制

- 網路（P10，`docs/net.md` §7）：只有 `AF_INET` 的 `SOCK_STREAM`／`SOCK_DGRAM`；`socket`、`connect`、`bind`、`listen`、`accept`/`accept4`、`sendto`、`recvfrom`、`sendmsg`/`recvmsg`（P11，沒有 control 資料）、`shutdown`、`getsockname`、`getpeername`、`setsockopt`（忽略）、`getsockopt`（一律 0）；socket 上的 `read`/`write` 等於 recv/send。P11 起 musl 的 `getaddrinfo`（DNS）可用：`/etc/resolv.conf` + UDP + `poll` + `recvmsg`；TCP 的 DNS 退路（`MSG_FASTOPEN` 的 `sendmsg`）不支援。

- 只有靜態、非 PIE 的 x86-64 程式（`ET_EXEC`）；沒有動態連結、`ET_DYN`、32 位元（`int 0x80`）。
- 單執行緒：沒有 `clone`／執行緒；`futex` 是單執行緒的簡化；沒有 `fork`／`execve`／`pipe`、訊號遞送（`rt_sigaction` 只回 0）、`/proc`、檔案 mmap、`MAP_FIXED`、`mremap`、`epoll`。
- Linux 程式在 `[3]`、`[4]` 的卷宗裡不受 handle 權限限制（整個卷宗，不只那個資料夾以下）。
- `getppid` 是 1；`getrandom` 不安全；時間只有 tick 解析度（沒有 invariant TSC 時 1 ms）。
- tty：一個 tty 的 `TIOCGWINSZ` 永遠 25×80；沒有 job control（`TIOCSPGRP` 什麼都不做）；VMIN／VTIME 不看；raw 模式下 `read` 有一個字就回。
- `PROT_NONE` 頁可以讀；部分 `munmap` 不歸還位址範圍。
- 輸出到沒有 master 的 tty 是同步 `kprint!`；有 master 時超過 64 KiB 未讀的輸出會被丟掉。
