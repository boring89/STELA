# Linux 相容層（Linux 人格）

> P7 由自動排程一次做完，使用者已於 2026-10-20 審核。P11 的部分（poll、pipe、`/etc`）與 **P12 的行程模型（執行緒、fork、execve、wait、訊號、pty、`/proc`、檔案 mmap，§11–§12）自動排程完成，使用者尚未審核**。

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
kernel/src/linux/memory.rs    brk、mmap（匿名與檔案）、munmap、mprotect
kernel/src/linux/process.rs   clone、fork、execve、wait4／waitid、exit（P12）
kernel/src/linux/futex.rs     futex 的等待佇列（P12）
kernel/src/linux/signal.rs    訊號的純邏輯：號碼、預設動作、sigaction、siginfo、rt_sigframe（P12，host 測試）
kernel/src/linux/signals.rs   送、遞送、rt_sigreturn、訊號 syscall、ITIMER_REAL 的計時執行緒（P12）
kernel/src/linux/pty.rs       /dev/ptmx 與 /dev/pts/N（P12）
kernel/src/linux/procfs.rs    /proc 檔案的內容（P12，純邏輯）
frame/src/syscall.rs          SyscallFrame（P12：全部暫存器）、UserFrame
frame/src/trap.rs             UserRegs（P12）
frame/src/random.rs           ChaCha20 產生器、RDSEED／RDRAND／TSC 抖動（P12）
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
| 結束 | `exit` 60、`exit_group` 231 | P12：`exit` 只結束呼叫的執行緒（最後一個執行緒的碼就是行程的碼），`exit_group` 結束全部（§11） |
| 行程 | `clone` 56、`fork` 57、`vfork` 58、`execve` 59、`wait4` 61、`waitid` 247、`execveat` 322、`clone3` 435 | §11（P12） |
| 檔案 | `read` 0、`write` 1、`open` 2、`close` 3、`stat` 4、`fstat` 5、`lstat` 6、`lseek` 8、`pread64` 17、`pwrite64` 18、`readv` 19、`writev` 20、`access` 21、`dup` 32、`dup2` 33、`fcntl` 72、`getcwd` 79、`chdir` 80、`fchdir` 81、`rename` 82、`mkdir` 83、`rmdir` 84、`unlink` 87、`getdents64` 217、`openat` 257、`mkdirat` 258、`newfstatat` 262、`unlinkat` 263、`renameat` 264、`faccessat` 269、`dup3` 292、`faccessat2` 439 | §5、§7 |
| 檔案（P12） | `truncate` 76、`ftruncate` 77、`fsync` 74、`fdatasync` 75、`sync` 162、`syncfs` 306 | 改大小；同步都是 0（區塊快取寫透） |
| 權限與時間（P12） | `chmod` 90、`fchmod` 91、`fchmodat` 268、`chown` 92 等、`utime` 132、`utimes` 235、`utimensat` 280、`umask` 95 | 不存 mode、owner，不能設時間：路徑存在就回 0；`umask` 記住並回舊值 |
| 連結 | `readlink` 89、`readlinkat` 267；`link` 86、`symlink` 88、`linkat`、`symlinkat` | P12：只有 `/proc/self/exe`、`/proc/self/fd/N` 是連結，其他存在的路徑 `EINVAL`；建立連結 `EPERM` |
| 鎖 | `fcntl` 的 `F_GETLK`、`F_SETLK`、`F_SETLKW`、OFD 版本 | P12：一律成功，`F_GETLK` 回沒有鎖 |
| 等待 | `poll` 7、`select` 23、`pselect6` 270、`ppoll` 271 | §5.3（P11） |
| 終端機 | `ioctl` 16 | tty：`TCGETS`、`TCSETS(W/F)`、`TIOCGWINSZ`、`TIOCSWINSZ`（P12，送 SIGWINCH）、`TIOCGPGRP`／`TIOCSPGRP`（P12：前景行程群組）、`TIOCSCTTY`、`TIOCNOTTY`；pty master：`TIOCGPTN`、`TIOCSPTLCK`（§5.4）；任何 fd：`FIONREAD`、`FIONBIO`；其他 → `ENOTTY` |
| 記憶體 | `mmap` 9、`mprotect` 10、`munmap` 11、`brk` 12、`madvise` 28（0） | §8 |
| 訊號 | `rt_sigaction` 13、`rt_sigprocmask` 14、`rt_sigreturn` 15、`pause` 34、`getitimer` 36、`alarm` 37、`setitimer` 38、`kill` 62、`rt_sigpending` 127、`rt_sigsuspend` 130、`sigaltstack` 131、`tkill` 200、`tgkill` 234 | §12（P12） |
| 時間 | `nanosleep` 35、`gettimeofday` 96、`times` 100、`clock_gettime` 228、`clock_nanosleep` 230 | REALTIME = `wall_clock_ns()`（`docs/time.md` §5.5）；MONOTONIC 等 = `now_ns()`；P12：sleep 可被訊號打斷（`EINTR`，寫回剩餘時間）；`times` 回 0 的 CPU 時間、開機後的 tick（100 Hz） |
| 身分 | `getpid` 39、`gettid` 186、`getppid` 110、`getuid` 102 等（0）、`setuid`／`setgid`（0）、`uname` 63 | P12：`gettid` 是執行緒 id（主執行緒 = pid）、`getppid` 是真的父行程（STELA 啟動的是 1） |
| 行程群組（P12） | `setpgid` 109、`getpgrp` 111、`setsid` 112、`getpgid` 121、`getsid` 124 | §11.5 |
| 排程（P12） | `sched_yield` 24、`sched_getaffinity` 204（行程所在的那顆）、`sched_setaffinity`（0）、`sched_getscheduler`／`getparam`（SCHED_OTHER）、`getcpu` 309 | 執行緒不換 CPU（`docs/smp.md`） |
| 資源（P12） | `getrlimit` 97、`setrlimit` 160（忽略）、`prlimit64` 302、`getrusage` 98（0）、`sysinfo` 99 | 固定值：STACK 1 MiB、NOFILE 256、NPROC 1024、其他無限 |
| TLS | `arch_prctl` 158、`set_tid_address` 218、`set_robust_list` 273（0） | §9 |
| 同步 | `futex` 202 | P12：真的等待佇列（§11.2） |
| 亂數 | `getrandom` 318 | P12：`frame::random`（ChaCha20，金鑰來自 RDSEED／RDRAND，沒有就用 TSC 抖動；`docs/tls.md` §2）；一次最多 1 MiB |
| pipe（P11） | `pipe` 22、`pipe2` 293 | `OpenFile::PipeRead`／`PipeWrite`（`docs/ipc.md` §14）；P12：讀端全關時寫入送 SIGPIPE |
| 不做 | `mremap` 25、`statx` 332、`rseq` 334、`membarrier` 324 | `ENOSYS`，不印訊息（musl 與 Rust std 會探測，退回別的做法） |
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
- **Ctrl-C**（ISIG）：清掉這一行、echo `^C\n`、送 **SIGINT** 給登記在這個 tty、屬於前景行程群組（`TIOCSPGRP` 設的；沒設時是全部）的 Linux 行程（P12；預設動作結束，exit code 130；P11 以前是直接以 137 結束）。
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

### 5.4 pty（P12，`linux/pty.rs`）

`open("/dev/ptmx")` 建一個有 master 的 `Tty`（和終端機程式用 `TtyCreate(MASTER)` 拿到的是同一種物件），編號 N 是最小的沒在用的；`/dev/pts/N` 開出它的程式端（一般的 tty 描述符，開的行程登記成它的使用者，Ctrl-C 送得到）。

```
 master fd：read  = 程式的輸出（O_NONBLOCK → EAGAIN）
            write = 打字（經行模式：編輯、echo、^C → SIGINT）
            ioctl TIOCGPTN → N；TIOCSPTLCK → 0（不鎖）
 musl：posix_openpt = open("/dev/ptmx")、grantpt 什麼都不做、unlockpt = TIOCSPTLCK、ptsname = TIOCGPTN
```

沒人持有 `Tty` 之後 N 可以重用。master 關掉時程式端不會收到 EOF／EIO（限制）。

### 5.2 shell 怎麼跑程式

```
 run prog args
   [1] 是 tty？（HandleKind）── 是：沿用（在終端機程式裡，P8）
                              └ 否：TtyCreate(0)，沒有 master，輸出直接印
   ProcessSpawn(prog, args, [tty, 無, 目前資料夾])
   每 20 ms：ProcessWait 逾時 → 看主 port 的按鍵：
       Ctrl-C → Linux 程式：送進 tty（→ SIGINT，P12）；2 秒內第二次、或 STELA 程式 → ProcessKill
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
              envp[]                         HOME=/Data、PATH=（P12：每個套件的 /Data/Programs/名稱/bin，`docs/pkg.md`）:/Initrd/linux:/Initrd、TERM=dumb、PWD=<cwd>、TZ；execve 用呼叫者給的 envp
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
 /dev               null、zero、tty、urandom、random、ptmx、pts/N（P12）
 /etc/resolv.conf   合成的（P11）：nameserver <DHCP 給的 DNS>（還沒有位址時是空檔）
 /proc              合成的（P12，§7.3）
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

### 7.3 `/proc`（P12，`path.rs` 的 `Proc`、`procfs.rs`、`files.rs`）

```
 /proc/cpuinfo、meminfo、uptime        全部的 CPU、PMM 的總數與空閒、開機秒數
 /proc/self（= /proc/<自己的 pid>）
     exe       連結：程式的 Linux 路徑（shell 啟動的是 [4] + 名字；execve 後是新程式的路徑）
     maps      程式、[heap]（brk 區）、映射區、[stack]
     stat      proc(5) 的 52 個欄位（不知道的是 0）
     status    Name、State、Pid、PPid、Threads…
     cmdline   程式名
     fd/       開著的描述符；fd/N 是連結（檔案的路徑、/dev/tty、pipe:[N]、socket:[N]）
 /proc/<別的 pid>   stat、status、cmdline（ps 用）；exe 也可以；maps、fd 只給自己
```

內容在 `open` 時產生（`OpenFile::Text`），和 `/etc/resolv.conf` 一樣。`execve("/proc/self/exe")` 換成程式的路徑。

### 7.2 fd 表（`linux/fd.rs`）

`FdTable<Arc<OpenFile>>`：最低可用號碼、上限 256（`EMFILE`）；`dup`／`dup2`／`dup3`／`fcntl(F_DUPFD…)` 共用同一個 `OpenFile`（所以共用 offset，和 Linux 一樣）；`FD_CLOEXEC` 每個 fd 自己的，P12 起 `execve` 關掉它們（`take_cloexec`）；`fork` 的子行程拿到同一批 `OpenFile`（共用 offset）。P12 新增 `OpenFile::PtyMaster`、`Random`。`OpenFile`：`Tty`、`Null`、`Zero`、`File { node, offset, append, readable, writable, flags }`、`Dir { node, path, cursor }`。fd 直接持有 VFS 的 `Node`（不經 handle 表）。行程結束時 `State::release()` 關掉全部。

## 8. 記憶體（`linux/memory.rs`）

```
 0x40_0000           程式（PT_LOAD）
 程式結尾（頁對齊）   brk 區：brk(addr) 往上長、往下縮，最多 256 MiB
 MAP_BASE 起         mmap 匿名映射（和 MemoryMap 共用 Regions，每段後一頁 guard）
 STACK_TOP 以下       1 MiB 堆疊
```

- `brk(0)` 或範圍外 → 回目前的 break（失敗的表示法）；長大時逐頁映射清零頁框（配不到就不動），縮小時釋放。musl 的 `sbrk(n)` 只接受 n = 0（musl 的設計），malloc（mallocng）用 brk 與 mmap。
- `mmap`：`MAP_PRIVATE|MAP_ANONYMOUS`；P12 起也接受檔案：`MAP_PRIVATE`（讀進新頁，寫入不回檔案）與沒有 `PROT_WRITE` 的 `MAP_SHARED`（一樣是複本，看不出差別）；可寫的 `MAP_SHARED` → `ENOSYS`；offset 要頁對齊；檔案結尾之後是 0。`MAP_FIXED` 不支援（`ENOSYS`），`PROT_EXEC` → `EACCES`；kernel 選位址，立刻映射全部頁（沒有需求分頁）；`PROT_NONE` 映射成唯讀（STELA 沒有「映射了但不能讀」的頁）。
- `munmap`：只在映射區內；釋放範圍內的頁；整段都放掉時把位址範圍還給 Regions（部分 munmap 只釋放頁，位址範圍留著）。
- `mprotect`：加執行權 → `EINVAL`；其他就改每一頁的可寫位元（`AddressSpace::protect`，新增）；沒映射的頁略過。

## 9. TLS：FS base

musl 啟動時 `arch_prctl(ARCH_SET_FS, tp)` 把執行緒指標放進 FS base（`%fs:0` 指向自己），`errno`、`__thread` 變數都從這裡找。

- `frame::task::set_fs_base(base)`：檢查 `base < USER_END`（非 canonical 的值會讓 `wrmsr` 在 kernel 裡 #GP），存進 `TaskContext.fs_base`，並立刻 `wrmsr IA32_FS_BASE`（0xC000_0100，Intel SDM Vol. 4 Table 2-2）。
- `switch_to`：切到有 FPU 區的（使用者）執行緒時 `wrmsr IA32_FS_BASE ← next.fs_base`（STELA 程式是 0），所以行程之間看不到彼此的 FS。切出時不讀回：CR4.FSGSBASE 沒開，使用者不能自己改（`wrfsbase` 會 #UD），context 裡的值永遠是對的。
- `ARCH_GET_FS` 讀 context 的值；`ARCH_SET_GS`／`GET_GS` → `EINVAL`。

## 10. initrd 的資料夾（`initrd.rs` 的 `Tree`）

P6 的 `InitrdFs` 把所有檔案放在根目錄；P7 起建一棵樹：成員路徑 `a/b/c` 變成資料夾 `a`、`a/b` 與檔案 `c`（不管 archive 有沒有目錄成員），id：根 1，其他依出現順序 2、3、……；同路徑後出現的檔案取代前面的。Makefile 打包時 `tar --sort=name … $(USER_PROGRAMS) linux`，所以 `Initrd:/linux/hello` 是 musl 的 hello。shell 的 `run linux/hello`：目前資料夾找不到時，非絕對、沒有卷宗名的路徑也到 `Initrd:/` 找。

## 11. 行程模型（P12）

> 比喻：P7 的 Linux 程式是「一個人的辦公室」：一個員工、不能找幫手、不能開分公司。P12 起它可以**請幫手**（執行緒：同一個檔案櫃、各自的桌子）、**開分公司**（fork：把整個檔案櫃影印一份給新的客戶）、**換掉整個業務**（execve：同一個客戶編號、清空檔案櫃、照新的手冊重來），總公司（父行程）也會**收到分公司結束的通知**（wait、SIGCHLD）。

```
 Linux 行程 = STELA 的 Process（位址空間、handle 表）+ linux::State（行程共享的）
   State：fd 表、cwd、brk、tty、threads[]、actions[64]、futex 等待者、
          parent、pgid、sid、children[]、child_waiters、term_signal、last_exit、
          umask、itimer、exe
 Linux 執行緒 = STELA 的 Thread（kernel 堆疊、FPU、FS base）+ LinuxThread
   LinuxThread：tid、clear_child_tid、signals { mask, pending, info[64],
                saved_mask, restart, altstack, trap }、deliverable（原子旗標）

 clone(CLONE_VM|CLONE_THREAD|…) ──► spawn_user_regs(同一個行程)      §11.1
 fork / vfork / clone(SIGCHLD)  ──► clone_user() ─► create_fork ─► 子行程   §11.3
 execve                          ──► 新 AddressSpace ─► swap_user          §11.4
 子行程結束 ──► process_ended ──► 父行程：children[] 記狀態、叫醒 wait4、SIGCHLD   §11.5
```

### 11.0 frame 的改動：完整的使用者暫存器

訊號（handler 要拿到被打斷的全部暫存器、rt_sigreturn 要全部還原）、`fork`（子行程從同一個指令繼續，所有暫存器都一樣）、`clone`（新執行緒除了 RAX 與 RSP 都和呼叫者一樣）、`execve`（全部換掉）都需要「回 ring 3 時的整組暫存器」。

- `syscall` 進入點現在把 15 個通用暫存器全部存進 `SyscallFrame`（160 bytes，多一個「完整還原」旗標與一個補齊用的字）。平常照舊 `sysretq`（RCX = RIP、R11 = RFLAGS）；kernel 用 `set_regs` 換掉暫存器時，旗標讓 stub 改走 `iretq`，連 RCX、R11 也還原（`sysret` 做不到）。
- `UserRegs`（`frame::trap`，純資料）：`sanitized()` 檢查 RIP、RSP 在使用者半部（`iretq`／`sysretq` 到非 canonical 位址會在 ring 0 出錯），RFLAGS 只保留使用者能改的位元（CF PF AF ZF SF TF DF OF AC ID，Intel SDM Vol. 1 §3.4.3），IF 與 bit 1 一定是 1。
- user_return hook 收到 `UserFrame`（syscall 或中斷／例外的框架），可以讀、可以換；使用者例外的 handler 可以「改了暫存器之後返回」（轉去訊號 handler），不一定要結束。
- `enter_user_regs(&UserRegs)`：新執行緒用整組暫存器進 ring 3；`fpu_state()`／`set_fpu_state()`（FXSAVE 格式；MXCSR 的保留位元先清掉，不然 FXRSTOR #GP）；`TaskContext::copy_fpu_from_current()`（clone、fork 的子執行緒繼承 FPU 狀態）。
- `AddressSpace::clone_user()`：走使用者半部的表，每個自己的頁配新頁框複製（共享記憶體與裝置頁照原樣映射、物件 clone）；`swap_user(&other)`：交換兩個位址空間的 PML4 項目 0–255（以及共享／裝置的清單），在用的那個重載 CR3 清 TLB。

### 11.1 執行緒：`clone`、`exit`、`exit_group`

musl 的 `pthread_create` 用 `CLONE_VM|CLONE_FS|CLONE_FILES|CLONE_SIGHAND|CLONE_THREAD|CLONE_SYSVSEM|CLONE_SETTLS|CLONE_PARENT_SETTID|CLONE_CHILD_CLEARTID|CLONE_DETACHED`。STELA 只接受這一組裡的旗標（必須有 VM、SIGHAND、THREAD；其他 → `EINVAL`）：

```
 tid = 行程 id 計數器的下一個號碼（和 pid 不會撞；主執行緒的 tid = pid）
 LinuxThread::new(tid, 呼叫者的訊號遮罩)
 CHILD_CLEARTID → clear_child_tid = ctid；PARENT_SETTID、CHILD_SETTID → 寫 tid
 暫存器 = 呼叫者的（RCX、R11 是 syscall 留下的），RAX = 0，RSP = 給的堆疊
 FS base = tls（SETTLS）或呼叫者的
 spawn_user_regs(同一個行程)：行程親和，同一顆 CPU
```

- `exit`（60）：寫 0 到 `clear_child_tid` 並 `FUTEX_WAKE` 一個（`pthread_join` 在等這個字）、記下 `last_exit`、從 `threads` 移除、結束這個執行緒。最後一個執行緒結束時行程以 `last_exit` 結束（沒有別的 exit code 的話）。
- `exit_group`（231）：`exit_process`，所有執行緒結束。
- `clone3`（435）：讀 `struct clone_args`（flags、child_tid、parent_tid、exit_signal、stack + stack_size、tls），轉成 `clone`。

### 11.2 futex（`linux/futex.rs`）

每個行程一張等待者清單（使用者位址、bitset、執行緒）。「值還是 X」的檢查與排隊在清單的鎖裡做，喚醒者也拿同一把鎖，所以「檢查完、睡著前被叫醒」不會遺失：

```
 FUTEX_WAIT(addr, val, timeout)        FUTEX_WAKE(addr, n)
 鎖住；*addr != val → EAGAIN            （喚醒者先改了 *addr）
 排隊 (addr, 我)；放鎖                   鎖住；拿走最多 n 個 addr 的等待者；放鎖
 block_until(期限) ◄────────────────────  wake 每一個
 我還在清單裡？ 期限到 → ETIMEDOUT；被訊號 → EINTR；否則 0
```

`WAIT`（相對逾時）、`WAIT_BITSET`（絕對時間，`FUTEX_CLOCK_REALTIME` 時是牆上時間）、`WAKE`、`WAKE_BITSET`、`REQUEUE`、`CMP_REQUEUE`；`PRIVATE` 旗標不看（沒有跨行程共享記憶體的 futex）；`WAKE_OP`、PI 類 → `ENOSYS`。

### 11.3 `fork`、`vfork`、行程的 `clone`

```
 space = p.space().clone_user()           全部複製（沒有 copy-on-write）
 maps  = p.maps() 的複本                   映射區的區間
 State::fork_copy：同一批 OpenFile（共用 offset）、cwd、brk、tty、看得到的卷宗、
                   訊號動作、pgid、sid、umask、exe；沒有執行緒、子行程、計時器
 handle [1]–[6] 照抄（Linux 程式不直接用它們）
 子行程的執行緒：tid = pid、暫存器 = 呼叫者的、RAX = 0、FS base 一樣、FPU 一樣
 tty.add_user(子 pid)；父行程 children += 子行程
```

- `vfork` 和 `fork` 一樣（記憶體是複本，父行程不必停；子行程只會 exec 或 `_exit`）。
- 沒有 `CLONE_THREAD` 的 `clone` 都當 fork：musl 的 `posix_spawn` 用 `CLONE_VM|CLONE_VFORK|SIGCHLD`，子行程在呼叫者給的堆疊上跑，失敗時經 pipe 回報——記憶體是複本也照樣運作。
- 子行程的 CPU 由 round robin 決定（不一定和父行程同一顆）。
- 計畫寫的「孤兒交給 pid 1」：父行程先結束時，子行程結束的通知沒有人收（找不到父行程就算了），效果一樣。

### 11.4 `execve`、`execveat`

```
 讀路徑、argv、envp（各最多 4096 個、合計 256 KiB）
 /proc/self/exe → 程式的路徑；AT_EMPTY_PATH → 那個 fd 的檔案
 讀檔：前兩個 byte 是 "#!" → 直譯器 + 一個參數（一行最多 255 bytes，stack::shebang）
       argv = [直譯器, 參數?, 腳本路徑, 原本的 argv[1..]]（一層，不支援直譯器又是腳本）
 解析 ELF（失敗 ENOEXEC）；有 STELA note → ENOEXEC（STELA 程式不能原地取代 Linux 程式）
 新的 AddressSpace：載入、初始堆疊（給的 envp）
 ──── 到這裡為止失敗都回錯誤，行程不變 ────
 其他執行緒標 killed，讓出 CPU 直到只剩自己
 swap_user（舊的半部隨暫存的 AddressSpace 釋放）、映射區清空、brk 重設
 O_CLOEXEC 的 fd 關掉；有 handler 的訊號改回預設（忽略的保持）；futex 清單清空
 這個執行緒變成唯一的執行緒，tid = pid；exe = 新路徑
 FS base = 0、FPU 重設、暫存器全 0、RIP = entry、RSP = 新堆疊（經 iretq）
```

pid、ppid、cwd、pgid、訊號遮罩與 pending、tty、看得到的卷宗不變。行程名稱（`ps`）不變（限制）。

### 11.5 `wait4`、`waitid`、行程群組

- 子行程的最後一個執行緒被回收時（`Process::thread_reaped` → `linux::process_ended`）：找到父行程，在它的 `children` 記下 (exit code, 結束它的訊號)，叫醒 `child_waiters`，送 SIGCHLD（`si_code` CLD_EXITED／CLD_KILLED）。父行程把 SIGCHLD 設成 `SIG_IGN` 或有 `SA_NOCLDWAIT` → 直接移除（不留 zombie）。
- `wait4(pid, status, options)`：`-1` 任何子行程、`>0` 那一個、`0` 同一群組、`<-1` 群組 `-pid`；有結束的 → 移除紀錄、寫狀態字（`exit code << 8`；被訊號結束是訊號號碼）、回 pid；沒有 → `WNOHANG` 回 0，否則等；完全沒有符合的子行程 → `ECHILD`；被訊號打斷 → `EINTR`（`SA_RESTART` 時重做）。`WUNTRACED` 不看（沒有停止的行程）。
- `waitid(P_ALL/P_PID/P_PGID, id, infop, WEXITED|WNOHANG|WNOWAIT)`：`siginfo` 的 si_pid、si_status、si_code。
- 結束的子行程在 STELA 的行程表裡本來就留著（`Exited(code)`）；`children` 的紀錄就是 Linux 的 zombie，`/proc/N/stat` 的狀態是 `Z`。
- Linux 程式被訊號結束時，STELA 的 exit code 是 128 + 訊號（SIGINT 130、SIGKILL 137、SIGSEGV 139），`term_signal` 記下訊號給父行程的 `wait4`。
- 行程群組：STELA 啟動的 Linux 程式自己是一個群組與 session（pgid = sid = pid），父行程是 1；fork 繼承；`setpgid(pid, pgid)` 只能改自己或自己的子行程（session leader 不能改）；`setsid` 建新的。tty 的前景群組由 `TIOCSPGRP` 設，Ctrl-C 只送給它（§5）。

## 12. 訊號（P12，`signal.rs`、`signals.rs`）

> 比喻：訊號是**貼在員工桌上的便條**。誰都可以貼（kill、終端機的 ^C、kernel 自己），但員工只在**回到櫃台前**（回 ring 3 之前）才看便條；正在等東西（阻塞在 syscall）的員工會被叫醒去看。看到便條：交代過怎麼處理（handler）就先去處理、處理完回到原來的工作（rt_sigreturn）；沒交代就照公司規定（預設動作：多半是下班＝行程結束）。

```
 kill / tkill / tgkill / ^C / 例外 / 子行程結束 / alarm / SIGPIPE / SIGWINCH
       │
       ▼ send_thread / send_process：pending |= bit、info[sig] = …、wake(執行緒)
 阻塞中的 syscall ── current_killed()（有可遞送的訊號也算）──► 放棄等待
       │                                                   → EINTR（或記下要重做）
       ▼
 user_return（syscall、中斷、例外回 ring 3 之前）── deliver：
       取最低的 pending & !mask
       ├── 忽略（SIG_IGN、預設忽略的）──────► 下一個；全沒了且有被打斷的 syscall → 重做
       ├── 預設：結束 ──────────────────────► term_signal、exit_process(128 + sig)
       └── handler ─────────────────────────► 在使用者堆疊推 rt_sigframe、RIP = handler
 handler 的 ret → sa_restorer（musl 的 __restore_rt：mov rax, 15; syscall）
       ▼
 rt_sigreturn：從框架還原暫存器、遮罩、FPU（經 iretq）
```

### 12.1 送

- 送給執行緒（`tkill`、`tgkill`、`raise`、例外、SIGPIPE）：設 pending、記 `siginfo`、更新 `deliverable`（`pending & !mask != 0`，原子旗標，排程器不必拿鎖就能看）、`wake`。
- 送給行程（`kill`、^C、SIGCHLD、SIGALRM、SIGWINCH）：第一個沒遮蔽它的執行緒；如果那個執行緒會立刻拿到、而且它被忽略，直接丟掉；全都遮蔽了 → 第一個執行緒的 pending（解除遮蔽時才遞送）。
- `kill(pid, sig)`：`pid > 0` 那個行程；`0` 與 `-1` 自己的群組；`< -1` 群組 `-pid`；`sig = 0` 只檢查存在。目標不是 Linux 程式（STELA 程式）→ 預設動作是結束的訊號就以 128 + sig 結束它。
- SIGKILL 與 SIGSTOP 不能被攔、不能被遮蔽（`rt_sigaction` 回 `EINVAL`、遮罩會去掉它們）。

### 12.2 打斷阻塞中的 syscall

kernel 的每個阻塞點醒來後本來就會看 `current_killed()`（行程被 kill 時放棄等待）；P12 起它對 Linux 執行緒也包括「有可遞送的訊號」，所以 tty、pipe、socket、wait4、futex、poll、nanosleep、pause 都會醒來。分派器結束時（`signals::interrupted`）：會阻塞的 syscall 回 `EINTR` 或 `ENOSYS`（kernel 的等待被叫停時回 `NotSupported`）、而且有可遞送的訊號 → 改回 `EINTR`，並記下 syscall 號碼。遞送時：

- handler 有 `SA_RESTART` 且這個 syscall 可以重做 → 框架裡存的暫存器是 RAX = 號碼、RIP − 2（回到 `syscall` 指令），handler 回來後重新執行；
- 沒有 `SA_RESTART`，或是 `nanosleep`、`clock_nanosleep`、`pause`、`rt_sigsuspend`、`poll`、`ppoll`、`select`、`pselect6`（signal(7)：永遠不重做）→ `EINTR`；
- 訊號被忽略（或被別的執行緒拿走）→ 重做。

自己造成的錯誤不算被打斷：寫到讀端全關的 pipe 回 `EPIPE` 並送 SIGPIPE，不會被重做（第一次寫的時候犯過這個錯：無限重做）。

### 12.3 訊號框架（`struct rt_sigframe`，Linux `arch/x86/include/asm/sigframe.h`）

```
 被打斷時的 RSP
 ── 128 bytes red zone（System V ABI §3.2.2）
 ── FXSAVE 映像 512 bytes，64 對齊        ◄── uc_mcontext.fpstate
 ── rt_sigframe 440 bytes，使 handler 開始時 RSP ≡ 8 (mod 16)（像剛被 call）
      +0    pretcode = sa_restorer
      +8    ucontext：uc_flags、uc_link、uc_stack（24 bytes）
      +48             uc_mcontext：r8–r15 rdi rsi rbp rbx rdx rax rcx rsp rip eflags、
                      cs/gs/fs/ss、err、trapno、oldmask、cr2、fpstate、8 個保留字
      +304            uc_sigmask（要還原的遮罩）
      +312  siginfo   128 bytes
 handler(sig, &siginfo, &ucontext)：RDI、RSI、RDX；RAX = 0；DF 清掉
```

- 遞送時遮罩 |= `sa_mask`，再加上這個訊號本身（除非 `SA_NODEFER`）；`SA_RESETHAND` 改回預設；框架裡存的是**原本的**遮罩（`rt_sigsuspend` 時是暫時遮罩之前的那個）。
- 框架寫不進使用者堆疊 → 以 SIGSEGV 結束（Linux 的作法）。
- `sigaltstack` 只記錄，handler 永遠在被打斷的堆疊上跑（`SA_ONSTACK` 不看）；Rust std 的「堆疊溢位」handler 因此跑不起來，溢位的程式以 SIGSEGV 結束。
- `rt_sigreturn`：框架 = RSP − 8；暫存器經 `sanitized()`（RIP、RSP 必須在使用者半部，RFLAGS 只留使用者位元），否則 SIGSEGV；FPU 從 fpstate 還原。

### 12.4 例外 → 訊號

| 例外 | 訊號 | si_code、si_addr |
|---|---|---|
| #PF 14 | SIGSEGV | SEGV_MAPERR（不存在）／SEGV_ACCERR（權限），CR2 |
| #GP 13 | SIGSEGV | SI_KERNEL |
| #UD 6 | SIGILL | ILL_ILLOPN，RIP |
| #DE 0、#MF 16、#XM 19 | SIGFPE | FPE_INTDIV（#DE），RIP |
| #DB 1、#BP 3 | SIGTRAP | |
| #SS 12、#AC 17 | SIGBUS | |

有 handler 且沒遮蔽 → 直接在例外的框架上遞送（`fault_signal`）；否則照舊印一行 `process N (name): exception …; killed`，以 128 + 訊號結束（P11 以前是 128 + 向量）。

### 12.5 預設動作與各訊號的來源

| 訊號 | 從哪來 | 預設 |
|---|---|---|
| SIGINT 2 | tty 的 ^C（shell 轉給 Linux 程式） | 結束 |
| SIGPIPE 13 | 寫到讀端全關的 pipe | 結束 |
| SIGALRM 14 | `alarm`、`setitimer(ITIMER_REAL)` | 結束 |
| SIGCHLD 17 | 子行程結束 | 忽略 |
| SIGWINCH 28 | terminal 改視窗大小（`TtySetSize`）、`TIOCSWINSZ` | 忽略 |
| SIGSEGV、SIGILL、SIGFPE、SIGTRAP、SIGBUS | 例外（§12.4） | 結束 |
| 其他（SIGTERM、SIGHUP、SIGUSR1/2、SIGABRT、即時訊號…） | `kill`、`raise` | 結束；SIGCONT、SIGURG、SIGSTOP 類 → 忽略（沒有 job control 的停止） |

### 12.6 計時器（`ITIMER_REAL`）

`alarm`、`setitimer` 把行程放進一張清單，kernel 執行緒 `linux-timer`（第一次用到時建立）睡到最早的期限，到了送 SIGALRM、週期的重新排定。`ITIMER_VIRTUAL`、`ITIMER_PROF` → `EINVAL`。fork 的子行程沒有計時器。

## 13. musl 與測試程式

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
| `nosys` | P12 起：`io_setup`、`mremap` → ENOSYS、`symlink` → EPERM（fork、execve 能用了） | `nosys: OK` |
| `pipe`（P11） | fstat S_IFIFO、poll、依序讀寫、dup2、寫端全關 → 0、沒有讀端 → EPIPE、`pipe2(O_NONBLOCK|O_CLOEXEC)` → EAGAIN、FD_CLOEXEC | `pipe: OK` |
| `poll`（P11） | stdout 立刻可寫、stdin 50 ms 沒東西（等滿）、`/dev/null`、fd −1 略過、關掉的 fd → POLLNVAL、`ppoll` 20 ms、`select`（可寫、30 ms 逾時、關掉的 fd → EBADF）、`pselect`；`/etc/resolv.conf` 是一般檔、內容 `nameserver …`、不能寫；最後 `poll(-1)` 等使用者打一行 | `poll: you typed: …`、`poll: OK` |
| `threads`（P12） | 10 個 pthread 在 mutex 下加總、join 的回傳值、`__thread` 每執行緒不同、condvar、detach、gettid ≠ 主執行緒且主執行緒 tid = pid、futex 逾時（≥ 30 ms）與值不同（EAGAIN）、`sched_getaffinity` 一顆 | `threads: 10 threads added up to 110000`、`threads: OK`，exit 4 |
| `procs`（P12） | fork + exit code + getppid、父子間的 pipe（記憶體是複本）、execve 錯誤（ENOENT、EACCES）、子行程 execve hello、`#!` 腳本（`/tmp/script`）、posix_spawn（成功與 ENOENT）、vfork、WNOHANG、setpgid／getpgrp、連續 20 次 fork、最後 ECHILD | `procs: child said: hello from the child`、`argv[1] = one`、`procs: OK` |
| `signals`（P12） | SA_SIGINFO handler（raise、kill；si_pid）、遮罩與 sigpending、SIG_IGN、alarm + pause、setitimer + sigsuspend、SIGCHLD + waitpid、SIGPIPE 預設結束子行程（WTERMSIG）、沒有 SA_RESTART 的 read → EINTR、有 SA_RESTART 的 read 繼續等到資料、SIGSEGV handler（si_addr = 16）、SIGSEGV 預設、SIGTERM；最後等 Ctrl-C | `signals: got SIGINT`、`signals: OK` |
| `misc`（P12） | getrandom 與 `/dev/urandom`、pty（posix_openpt、ptsname、兩個方向、FIONREAD）、檔案 mmap（私有、唯讀共享）、pread／pwrite、ftruncate、鎖、fsync、dup3、chmod、umask、`/proc`（exe、maps、cpuinfo、meminfo、status、fd/）、getrlimit、sysinfo、times、fchdir | `misc: pty /dev/pts/0 works`、`misc: OK` |
| `hello-std`（P12） | Rust std（`linux/rust/hello-std`，`x86_64-unknown-linux-musl`、`-C relocation-model=static`）：8 個 `std::thread` 在 `Mutex` 下加總、`mpsc`、`std::fs`、`thread::sleep`、`SystemTime`、`available_parallelism` | `hello-std: OK (1 cpu, HOME=/Data)` |

Rust std 程式的建置（`make linux-rust`，`make linux-tests` 會做）：`rustup target add x86_64-unknown-linux-musl`（`rust-toolchain.toml` 已列，CI 自動裝）；`RUSTFLAGS="-C relocation-model=static -C target-feature=+crt-static -C link-arg=-Wl,-z,max-page-size=4096"`——Rust 的 musl 目標預設產生 static-pie（`ET_DYN`），STELA 只載入 `ET_EXEC`。

## 14. 測試

| 測試 | 內容 |
|---|---|
| host `elfload` | packed 段（共用頁、仍拒絕重疊）、notes 解析（截斷時停止）、程式標頭位址 |
| host `kernel::linux::*` | 號碼、errno 對應與編碼、fd 表（最低號碼、dup/dup2、滿、drain、P12：`take_cloexec`）、路徑（normalize、target、join；P12：`/dev`、`/proc`）、初始堆疊的配置、P12：`#!` 的解析、訊號（框架配置與往返、sigaction 編碼、預設動作、wait 狀態字）、`/proc` 的文字 |
| host `frame` | P12：`UserRegs` 的往返與 RFLAGS 清理、ChaCha20（RFC 8439 §2.3.2 的測試向量）、產生器換金鑰 |
| host `kernel::object::ldisc` | 行、編輯（Backspace、UTF-8、Ctrl-U、控制字元、Tab）、Ctrl-C、Ctrl-D、raw、上限 |
| host `kernel::vfs::tmpfs`、`kernel::initrd` | TmpTree（檔案、資料夾、容量、錯誤、改名迴圈）、initrd 的樹 |
| host `abi` | 編號 33–36、`note::BYTES`、`spawn::FORCE_*`、`tty::MASTER`、`object_kind` |
| selftest "linux personality" | hello 有 note、裸 ELF 沒有；手組的 Linux 機器碼：write 到 tty（`linux: hello from raw system calls`）、RDX 保留、`io_setup` → `-ENOSYS`（P12 以前是 fork）、exit_group(38)，兩次 PMM 回復；強制 STELA 人格（同一段碼變成 `ThreadExit`，exit 0）；`Initrd:/linux/hello`（musl）exit 3，兩次 PMM 回復；P12：`threads`（exit 4）與 `procs`（exit 0）各兩次，PMM 回復（執行緒、fork、execve 不漏頁框） |
| `make test-linux` | 一般 kernel + AHCI 測試磁碟：shell 依序 `run linux/hello a b`、`files`、`memory`、`time`、`nosys`、`tty`（打 `stela rocks`）、`poll`（打 `poll works`）、`pipe`、P12：`threads`、`procs`、`misc`、`hello-std rust`、P13：`dyn`（§16）、`signals`（看到 `press Ctrl-C` 後經 QMP 送 ctrl-c），檢查輸出與 exit code |
| `make test-alpine`（P13，本機專用） | Alpine 3.20 的 `musl-1.2.5-r3.apk`、`busybox-1.36.1-r31.apk`（`~/stela-ports/sources/alpine/`，解開在 `build/alpine`）：Alpine 的 ld-musl 放 `Data:/Programs/lib`、busybox 放 `Data:/alpine`；`busybox echo`、`dyn`（這時也用 Alpine 的 ld-musl）、`busybox uname -s`（`STELA`）、`busybox ls /lib`（`Data:/Programs/lib` 優先） |
| host（P13） | `elfload`：`ET_DYN` 只在 Packed 版面、`PT_INTERP`；`linux::path`：`/lib`、`/usr/lib`；`object::regions`：`reserve`、`containing` |
| `make test-net`（P10） | `run linux/net PORT host`：BSD socket 抓 host 的 HTTP 檔、`getsockname`／`getpeername`／`getsockopt`、UDP `sendto`、關著的 port → `ECONNREFUSED`、`AF_INET6` → `EAFNOSUPPORT`（`docs/net.md` §7） |

## 15. 已知限制

- 網路（P10，`docs/net.md` §7）：只有 `AF_INET` 的 `SOCK_STREAM`／`SOCK_DGRAM`；`socket`、`connect`、`bind`、`listen`、`accept`/`accept4`、`sendto`、`recvfrom`、`sendmsg`/`recvmsg`（P11，沒有 control 資料）、`shutdown`、`getsockname`、`getpeername`、`setsockopt`（忽略）、`getsockopt`（一律 0）；socket 上的 `read`/`write` 等於 recv/send。P11 起 musl 的 `getaddrinfo`（DNS）可用：`/etc/resolv.conf` + UDP + `poll` + `recvmsg`；TCP 的 DNS 退路（`MSG_FASTOPEN` 的 `sendmsg`）不支援。

- P13 起有動態連結與 PIE（§16）；沒有 32 位元（`int 0x80`）、ASLR、`AT_EXECFN`、`LD_LIBRARY_PATH` 以外的搜尋設定（`/etc/ld-musl-x86_64.path` 沒有）。
- 行程模型（P12）：fork 全複製（沒有 copy-on-write：大程式 fork 慢、記憶體加倍）；vfork 不讓父行程等；一個行程的執行緒都在同一顆 CPU；沒有 `MAP_FIXED`、可寫的 `MAP_SHARED`、`mremap`、`epoll`、`inotify`、`eventfd`、`timerfd`、`signalfd`；execve 不能換成 STELA 程式、`#!` 只有一層；行程名稱（`ps`）在 execve 後不變；沒有停止／繼續（SIGSTOP、SIGTSTP、SIGCONT 只是忽略，沒有 job control 的停止）；`sigaltstack` 只記錄；`ITIMER_VIRTUAL`／`PROF`、POSIX 計時器（`timer_create`）沒有；沒有 CPU 時間的統計（`times`、`getrusage` 是 0）；父行程先結束時子行程的狀態沒人收。
- 權限：mode、owner、時間都不能改（`chmod`、`chown`、`utimensat` 成功但什麼都不做）；記錄鎖一律成功。
- pty：master 關掉時程式端不會收到 EOF／SIGHUP。
- Linux 程式在 `[3]`、`[4]` 的卷宗裡不受 handle 權限限制（整個卷宗，不只那個資料夾以下）。
- 時間只有 tick 解析度（沒有 invariant TSC 時 1 ms）。
- tty：VMIN／VTIME 不看；raw 模式下 `read` 有一個字就回。
- `PROT_NONE` 頁可以讀；部分 `munmap` 不歸還位址範圍。
- 輸出到沒有 master 的 tty 是同步 `kprint!`；有 master 時超過 64 KiB 未讀的輸出會被丟掉。

## 16. 動態連結（P13 K5，使用者尚未審核）

### 16.1 比喻

靜態程式像一本把所有引用文獻都抄進來的書；動態連結的程式只寫「見《libc》第 3 章」，開書之前要有人（**動態連結器**，ELF 的 `PT_INTERP`）去書架上把那幾本書拿來、翻到對的頁、把頁碼填好（重定位）。kernel 只負責把「書」與「拿書的人」放上桌，之後的事都是動態連結器在使用者空間做的。

### 16.2 kernel 做的事（`program::load_linux`）

```
 程式 ELF
   ET_EXEC → 照它寫的位址載入（bias 0，和 P12 一樣）
   ET_DYN（PIE）→ 載到 PIE_BASE 0x5555_5555_0000（所有位址 + bias）
   有 PT_INTERP（例如 /lib/ld-musl-x86_64.so.1）：
     從 /lib 或 /usr/lib 讀出來（只接受這兩個地方），必須是 ET_DYN、自己沒有 PT_INTERP
     載到 INTERP_BASE 0x7F00_0000_0000，第一個執行緒從它的 entry 開始
 auxv：AT_PHDR = 程式標頭在記憶體的位址（+ bias）、AT_PHENT、AT_PHNUM、
       AT_BASE = 直譯器的 base（沒有直譯器是 0）、AT_ENTRY = 程式的 entry（+ bias）
 brk：從程式影像的結尾開始；PIE 在 map 區（0x1000_0000_0000–0x7000_0000_0000）裡，
      所以 PIE 與它的 brk 區（256 MiB）先在 Regions 保留起來（Regions::reserve），mmap 不會選到
```

沒有 ASLR（位址固定，方便除錯）；`execve` 走同一條路。

### 16.3 `/lib` 與 `/usr/lib`

合成根多了 `/lib`、`/usr/lib`（同一個地方），依序找：

1. `Data:/Programs/lib/`（套件裝的函式庫；`docs/pkg.md`）
2. `Initrd:/linux/lib/`（`ld-musl-x86_64.so.1` = STELA 自己建的 musl `libc.so`，strip 後約 660 KB；`libdyntest.so` 測試用）

每個 Linux 程式都看得到（不受 `[3]`／`[4]` 卷宗的限制）。`make musl` 現在也建共享版（`--syslibdir` 指到 `build/musl/install/lib`：musl 的 `make install` 預設會在 host 的 `/lib` 建 symlink）。

musl 的動態連結器**就是** libc.so 本身：只連 libc 的程式（Alpine 的 busybox）不需要再載入任何檔案；`libc.musl-x86_64.so.1`、`libpthread.so` 等名字被動態連結器認成自己（musl 的保留名稱），所以不用 symlink。

### 16.4 執行權（W^X 放寬的範圍）

計畫的決定：`PROT_EXEC` 只給 **`MAP_PRIVATE` 的檔案映射**，而且檔案在：

- `Initrd:/linux/lib`（含子資料夾）、`Data:/Programs/lib`、`Data:/Programs/NAME/lib`
- 或程式自己的資料夾（`/proc/self/exe` 所在的資料夾）

`files::may_execute` 判斷。同時可寫又可執行一律拒絕（`EACCES`）；匿名的可執行記憶體拒絕（JIT 之後另議）。這種映射的範圍記在 `Memory::exec`；`mprotect` 只在這些範圍裡可以加 `PROT_EXEC`（動態連結器的 RELRO 是改成唯讀，不需要）。`munmap` 整段時拿掉紀錄；fork 時複製。

### 16.5 `MAP_FIXED`

動態連結器載入一個 `.so` 的方式（musl `map_library`）：先 `mmap(NULL, 整個範圍, 第一段的 prot, MAP_PRIVATE, fd, 0)` 保留位址（超過檔案長度的頁是 0），再對其餘每一段 `mmap(base + 段位址, …, MAP_PRIVATE | MAP_FIXED, fd, 段偏移)`，`.bss` 用 `MAP_FIXED | MAP_ANONYMOUS`。所以 `MAP_FIXED`：

- 落在一個既有映射裡：換掉那些頁（範圍的紀錄不變）
- 落在 map 區的空白處：`Regions::reserve` 保留剛好那一段（含保護頁的規則）
- 其他（重疊兩段、在 map 區外）：`ENOMEM`

### 16.6 測試程式

- `linux/tests/libdyntest.c` → `Initrd:/linux/lib/libdyntest.so`（`-shared -fPIC`；有 `static` 資料與 `__thread` 變數，檢查重定位與 TLS）。
- `linux/tests/dyn.c` → `Initrd:/linux/dyn`：`-fPIE -pie -Wl,--dynamic-linker=/lib/ld-musl-x86_64.so.1 -ldyntest`。印 `pie yes`（`&main` 在 PIE_BASE 以上）、呼叫函式庫、`dlopen`／`dlsym`、`malloc` 1 MiB。
- `make test-alpine`（本機）：Alpine 的 ld-musl 與 busybox，證明不是只和自己建的 musl 相容。
