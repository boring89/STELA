# 使用者空間：libstela、行程、init 與 shell

> P6 由自動排程一次做完，使用者已於 2026-10-20 審核。

開機之後使用者看到的東西：init 啟動 shell，shell 讀鍵盤、跑指令、啟動程式。以及寫一個 STELA 程式需要知道的：libstela 的 API、heap、參數、從檔案啟動行程。IPC 與 handle 見 `docs/ipc.md`，檔案與卷宗見 `docs/vfs.md`，syscall 慣例見 `docs/syscall.md`。

程式碼：

```
abi/src/lib.rs               編號 27–31、Rights::KILL、tag::BOOTSTRAP_ARGS／SHELL_READY、EXIT_KILLED、sysinfo
kernel/src/psyscall.rs       ProcessSpawn、ProcessWait、ProcessKill、ProcessSelf、SysInfo
kernel/src/program.rs        ELF 載入、參數訊息、spawn_path（一律經 VFS）
kernel/src/task/process.rs   行程表（名稱、weak 參考、狀態）、wait_until、list
kernel/src/vfs/target.rs     InitrdFs（initrd 掛成 Initrd 卷宗）
user/libstela/src/           heap、sync、process、path、line、fs、ipc
user/init、user/shell        init 與 shell
user/proctest                行程測試程式
tools/qemu_test.py           --type：經 QMP 打字
```

## 1. 比喻：大樓的管理員與櫃台

- **init** 是大樓管理員：一大早第一個到，打開櫃台（啟動 shell），把電話線（鍵盤）接到櫃台；櫃台人員下班（shell 結束）就再找一個來。
- **shell** 是櫃台：聽你說一句話（一行指令），照做或幫你叫人（`run` 啟動程式）；叫來的人辦事時電話轉給他，辦完再轉回來。
- **handle** 是鑰匙（`docs/ipc.md`）：管理員把「工作目錄」（Data 的根）的鑰匙交給櫃台，櫃台再把同一把鑰匙的複製品交給它叫來的每個人。

## 2. 行程 syscall（接 P5 的 27 起）

| 編號 | 名稱 | 參數 | 回傳 |
|---|---|---|---|
| 27 | `ProcessSpawn` | file, &Message, &[Handle; n], n（≤ 6，P11；之前 ≤ 3）, &[u32; 2m], m（≤ 4，P11：能力，§2.3） | Process handle（`WAIT|KILL|DUPLICATE|TRANSFER`） |
| 28 | `ProcessWait` | process, timeout_ns | exit code（i64）；`TimedOut` |
| 29 | `ProcessKill` | process | 0；被 kill 的行程以 `EXIT_KILLED`（137）結束 |
| 30 | `ProcessSelf` | — | 自己的 Process handle（`KILL|DUPLICATE|TRANSFER`，沒有 `WAIT`） |
| 31 | `SysInfo` | kind, buf, len | 數量 |

- `ProcessSpawn`：file 要 `READ` 的 File handle（資料夾 → `IsDir`）；kernel 讀整個檔案（≤ 32 MiB）、先確認是 ELF，再**搬走** handles（各要 `TRANSFER`，`Handle::NONE` 表示空）當新行程的 `[1]`–`[3]`，訊息（tag 強制成 `BOOTSTRAP_ARGS`；它夾帶的 handle 也搬走）放進新行程主 port 的佇列，然後才啟動第一個執行緒。行程名稱 = 檔名。
- Process handle 只記**行程 id**，不讓行程的記憶體活著：行程結束就回收；結束碼留在行程表裡，所以 `ProcessWait` 可以重複呼叫。
- `ProcessWait` 等在行程表的 waiter 清單上（`block_until`，可逾時）；等待者自己被 kill 時放棄。
- `ProcessKill` 自己 → 等於 `ProcessExit(137)`。
- `SysInfo`：`PROCESSES`（每筆 48 bytes：pid、執行緒數、狀態 1 執行中／2 已結束、exit code、名稱 24 bytes；回傳總筆數，可能多於放得下的）、`MEMORY`（PMM 總數／空閒頁框、kernel heap 大小／已用）、`UPTIME`（奈秒、tick）、`FRAMEBUFFER`（`docs/display.md`）、`TIME`（6，P10：牆上時鐘，1970 年起的奈秒，libstela `process::wall_clock_ns`）、`CMDLINE`（5，P10：開機選項的位元組，回傳長度；`abi::sysinfo::cmdline_flag` 查選項，libstela 的 `process::boot_flag`；`docs/p520.md` §3）、`CPUS`（7）與 `PROCESS_CPUS`（8）（`docs/smp.md`）、`HARDWARE`（9，P11：硬體報告的文字，回傳整份長度；libstela `process::hardware`，`hwinfo` 程式與 Mouseion 的 About STELA；`docs/hwinfo.md`）。

### 2.3 能力（capability，P11，使用者已於 2026-10-22 審核）

**比喻**：大樓的總電源開關、對講機總機、對外電話線各有一把專用鑰匙。管理員（init）一開始就拿到全部三把（kernel 給的），只把需要的那幾把**複製**給該用的人；沒拿到鑰匙的人就算知道開關在哪也打不開（`AccessDenied`）。

| kind | 名稱 | 守著什麼 | 誰拿到 |
|---|---|---|---|
| 1 | `POWER` | `SystemPower`（`docs/acpi.md`） | init → 視窗伺服器 → 它啟動的程式（Mouseion：特別 → 關機／重新開機） |
| 2 | `INPUT` | `InputAttach` | init → 視窗伺服器；文字模式的 shell → 它 `run` 的程式（`echo`） |
| 3 | `NETWORK` | `SocketCreate`；Linux 的 `socket()`（沒有 → `EACCES`；Linux 程式不留 handle，`linux::State::network` 記著） | init → 所有人（預設繼承，現在的行為） |
| 4 | `INIT` | 不是 kernel 的能力：init 主 port 的 `SEND`，送 `POWER_REQUEST` | init 做的，所有人預設繼承 |
| 5 | `DEVICES` | `DeviceOpen`、`NetAttach`（P13，`docs/devices.md`） | init → 它啟動的驅動（e1000u） |
| 6 | `VOLUMES` | `VolumeOpen`（`Initrd` 例外）、`VolumeList`、`VolumeOpenById`（P13，`docs/sandbox.md`） | init → 所有人預設繼承（`inherited()`）；**Mouseion 啟動的程式沒有**（容器） |

- kernel 物件 `KernelObject::Capability(kind)`，handle 的權限只有 `DUPLICATE|TRANSFER`；`HandleKind` = 12。持有就是許可，kernel 只檢查種類（`object::check_capability`）。
- **傳遞**：`ProcessSpawn` 的第 5、6 個參數是 (handle, kind) 對（最多 4 個，搬走，要 `TRANSFER`）；kernel 檢查 Capability 的種類和宣稱的相符、`INIT` 必須是有 `SEND` 的 port，否則 `InvalidArgument`。kernel 在 `BOOTSTRAP_ARGS` 之後**一定**再排一則 `CAPABILITIES`（tag 7；data = 每個 handle 的 kind，u32 LE；沒有就是空的），都在程式開始前放好，所以 libstela 的 `take_args` 不等待地收兩則就好。
- kernel 啟動 init 時給 `POWER`、`INPUT`、`NETWORK`（`program::spawn_path_with`；P13 起還有 `DEVICES`、`VOLUMES`）；selftest 啟動的程式什麼都沒有（fstest 例外：`VOLUMES`）。
- P13：一個程式最多六個能力；`CAPABILITIES` 一則四個，多的接下一則（前一則的 data 以 `capability::MORE` 結尾，`docs/sandbox.md` §3）。
- libstela：`process::capability(kind)`、`set_capability(kind, h)`（init 登記自己做的 `INIT`）、`pass_capability(kind)`（複製一份給子行程）、`inherited()`（`NETWORK` + `INIT` 的複製；P13 再加 `VOLUMES`）、`sandboxed()`（P13：沒有 `VOLUMES`）、`spawn_with(file, args, handles, &[(kind, h)])`；`spawn` = `spawn_with(…, &inherited())`。**`POWER` 與 `INPUT` 不會自動往下傳**。
- `process::shutdown()`／`reboot()`（`power(kind)`）：有 `POWER` 就直接 `SystemPower(kind, power)`；否則有 `INIT` 就送 `POWER_REQUEST`（data = kind，夾帶回覆 port）給 init，等 10 秒的回覆（失敗時 init 回 `Error` 值；成功就沒有回覆，機器已經關了）；兩個都沒有 → `AccessDenied`。所以 shell 的 `shutdown` 不必持有 `POWER`：它請 init 做。
- init 等子行程時用 WaitSet 同時等主 port（`POWER_REQUEST`）與子行程（`EXITED`）。
- libstela 的 `ipc::input_attach` 與 `net::Socket::*` 自動帶上自己的能力（沒有就送 0 → `AccessDenied`）。
- `process::spawn_with(file, args, &[h1..h6], caps)`：P11 起 `[4]` = 程式所在的資料夾（shell 的 `find_program` 一併找出來；Mouseion 給 bundle／程式所在的資料夾，Viewer 與 Terminal 給 Initrd 的根）。
- 資料夾 handle 的 `MANAGE` 權限（`docs/ipc.md` §3.3）：`process::pass_dir(h)` 複製一份給子行程，盡量保留 READ／WRITE／MANAGE（拿不到就退一級），取代各處的 `pass(cwd, READ|WRITE|DUPLICATE)`（那樣會拿掉 MANAGE，唯讀的 Initrd 還會因為沒有 WRITE 而複製失敗）。

### 2.1 程式一律從檔案啟動

P3–P5 的 kernel 有「從 initrd 生行程」的特例；P6 起 initrd 掛成唯讀卷宗 **`Initrd`**（`InitrdFs`：ustar 的檔案都在根目錄，id = 2 + 成員序號），kernel 自己啟動程式也走 VFS（`program::spawn_path("Initrd", "init", …)`）。

## 3. 參數（`BOOTSTRAP_ARGS`）

```
 新行程主 port 的第一則訊息（kernel 在程式開始前放好）：
   tag BOOTSTRAP_ARGS
   ≤ 112 bytes：data = "prog\0arg1\0arg2"（NUL 分隔，第一個是程式名）
   更長：handle_slots[0] = Memory 物件（資料在裡面），data = 長度（u64 LE）
 kernel 自己啟動的程式：只有程式名
```

libstela 的 `entry!` 在呼叫 `main` 前 `take_args()`：從主 port 不等待地收一則（一定是它），存起來；`process::args()` 回傳 `Vec<String>`。長參數就 map 那個 Memory 讀出來、關掉 handle。P11 起接著再收第二則 `CAPABILITIES`（§2.3）。

## 4. libstela

| 模組 | 內容 |
|---|---|
| `entry!(main)` | `_start`：`take_args()` → `main()` → `exit(code)` |
| `print!`／`println!`（= `debug_print!`／`debug_println!`） | 經 console 服務印字（`docs/ipc.md` §7） |
| `heap` | `#[global_allocator]`：`alloc` 可用（`Vec`、`String`、`Box`） |
| `sync::SpinMutex` | 使用者空間的鎖（搶不到就 `yield`） |
| `ipc` | handle、port、memory、`Mapping`、`input_attach` |
| `fs` | 檔案與卷宗（`docs/vfs.md`） |
| `process` | `spawn(file, args, [h1, h2, h3])`、`spawn_with(…, caps)`、`capability(kind)`、`pass(h, rights)`（複製一份可轉移的）、`standard_handles`、`wait`、`wait_forever`、`kill`、`this`、`list`、`memory`、`uptime`、`args`、`shutdown`／`reboot`（§2.3） |
| `path` | `Location { volume, parts }`、`resolve(cwd, input)` |
| `line` | `LineEditor`：`feed(char) -> Edit` |
| P11 新增 | `wait`（WaitSet）、`pipe`、`pipeline`（解析）、`io`（`[5]` 標準輸入）、`http`（GET、轉址、chunked、連結解析，`docs/apps.md` §4）；`process::hardware`、`process::open`（`ProcessOpen`）；`spawn` 回 `JoinHandle`（`docs/process.md` §8.1） |

### 4.1 heap

```
 第一次配置：MemoryCreate(1 MiB) → MemoryMap → 整塊放進 free list
 free list：依位址排序，[size | next] 寫在空閒區塊本身
 配置：first fit；使用者指標前 16 bytes 記 [區塊起點, 區塊大小]（對齊 > 16 時中間有空隙）
       剩下 ≥ 32 bytes 就切開
 釋放：讀那 16 bytes → 插回 free list，和前後相鄰的合併
 不夠：再 MemoryCreate 一塊（至少 1 MiB，或請求的大小）
```

一把 `SpinMutex` 保護（多執行緒程式也能用）。映射後 Memory 的 handle 就關掉（映射本身留住頁框）。記憶體不還給 kernel（行程結束時一起回收）。

### 4.2 路徑（`path::resolve`，純文字）

| 輸入 | 結果（目前在 `Data:/docs/notes`） |
|---|---|
| `a.txt` | `Data:/docs/notes/a.txt` |
| `../x` | `Data:/docs/x` |
| `../../../..` | `Data:/`（不會走出卷宗的根） |
| `/top` | `Data:/top` |
| `Boot:/EFI/./BOOT` | `Boot:/EFI/BOOT` |
| `Initrd:` | `Initrd:/` |

冒號只在第一個 `/` 之前才算卷宗名（`a/b:c` 是名字）。kernel 的 `DirOpen` 不接受 `..`，所以 `..` 一定在這裡先解掉。

### 4.3 行編輯（`line::LineEditor`）

可見字元加入（上限 200）、Backspace（0x08／0x7F）刪一個、Ctrl-U 清整行、Tab 回傳 `Edit::Complete`（P11 K44 起；以前當空白，§6.3）、Enter 完成；其他控制字元忽略。回傳要顯示的東西（`Echo(c)`、`Erase(n)`、`Done(line)`），呼叫者自己印——shell 用 `"\x08 \x08"` 擦掉（主控台 P6 起認得 backspace：游標退一格）。

## 5. init（`user/init`）

```
 home = VolumeOpen("Data")，沒有就 VolumeOpen("Initrd")
 program = Initrd:/shell
 loop:
   shell = ProcessSpawn(program, ["shell"], [console, init 主 port 的 SEND, home])
   收 SHELL_READY（≤ 5 秒）→ InputAttach(訊息帶來的 shell 主 port SEND)
   ProcessWait(shell) → 印 exit code → 重來
```

kernel 的 `init` 執行緒只做兩件事：啟動 input-router、跑 `Initrd:/init`（結束的話 1 秒後再跑）。

## 6. shell（`user/shell`）

- 提示符是目前位置：`Data:/> `（沒有 Data 時 `Initrd:/> `）。啟動時從 `[3]` 的 `FileStat.volume` 找出是哪個卷宗。
- 啟動後送 `SHELL_READY`（夾帶自己主 port 的 SEND）到 `[2]`；沒有 `[2]` 就自己 `InputAttach`。
- 每個指令用到的 handle 用完就關（`Owned` 在 drop 時 `HandleClose`），不會累積到 1024 個。

| 指令 | 說明 |
|---|---|
| `ls [path]` | 列出資料夾（大小、資料夾名後加 `/`、總數）；檔案就印它自己 |
| `cat file` | 印出檔案（UTF-8） |
| `cp from to` | 讀進 heap、`CREATE|TRUNCATE` 寫出 |
| `mv from to` | `DirRename`（同一卷宗） |
| `mkdir path`、`rm path` | `DirOpen(CREATE|DIRECTORY|EXCLUSIVE)`、`DirRemove` |
| `cd [path]`、`pwd` | 沒有 path 回到卷宗的根；確認是資料夾才換 |
| `run prog [args]` | 先找目前資料夾（或給的路徑），再找 `Initrd:/`（P7 起相對路徑如 `linux/hello` 也找）；handles = tty（P7：自己的 `[1]` 是 tty 就沿用，否則 `TtyCreate` 一個印到主控台的）、無、目前資料夾；前景等待，每 20 ms 看一次鍵盤：Ctrl-C → `ProcessKill`，其他鍵送給 tty（`docs/linux.md` §5.2）；結束後 `InputAttach` 拿回鍵盤、印 `prog exited with code N` |
| `ps`、`mem`、`uptime`、`vols` | `SysInfo` 與 `VolumeList` |
| `hwinfo`（程式，P11） | `SysInfo(HARDWARE)` 的整份報告（`docs/hwinfo.md`）；`hwinfo > 檔案` 存下來 |
| 其他名稱（P11 K33） | 不是內建指令就當程式找：工作資料夾 → `Initrd:/` → `Initrd:/bin/` → P14 K4：`System:/Programs/` → `System:/bin/`（`SEARCH_PATH`，`docs/system.md` §2.2；命令列工具 P14 起在 `System:/bin/`：`echo`、`cat`、`grep`、`wc`、`head`、`tail`、`sort`、`uniq`、`find`、`du`、`df`、`date`、`sleep`、`touch`、`stat`、`hexdump`、`tar`、`true`、`false`；`docs/apps.md` §1）；`cp -r` 複製整個資料夾 |
| `shutdown`、`reboot` | `SystemPower`（P6.5，`docs/acpi.md`）：卸載卷宗後關機／重新開機 |

P9：`run Name.app` 執行 bundle（`docs/vfs.md` §4.3）。
| `help`、`exit` | `exit` 後 init 會再啟動一個 |

程式若自己 `InputAttach`（例如 `run keyecho`，P11 K33 以前叫 `echo`），它結束前 shell 收不到鍵盤（Ctrl-C 也無效）；keyecho 以 Escape 結束。

```
STELA shell; type help for the commands
Data:/> vols
Boot       FAT     ro  40314 KiB, 0 KiB free
Data       eHFS    rw  89068 KiB, 87244 KiB free
Initrd     initrd  ro  380 KiB, 0 KiB free
Data:/> run hello
hello from user mode
hello from a second thread (argument 7)
hello: thread returned 42 within 4000 us
hello exited with code 0
Data:/> ps
  pid  name             threads  state
    1  init                   1  running
    2  shell                  1  running
(1 ended)
```

### 6.1 管線、重新導向、背景工作（P11，使用者已於 2026-10-22 審核）

**比喻**：以前櫃台一次只叫一個人辦事；現在可以排一條生產線，前一個人做完的東西直接用輸送帶（pipe）交給下一個人；原料可以從倉庫（檔案）搬來，成品可以直接入庫；不急的工作可以叫人在後面慢慢做（背景），櫃台繼續接下一位。

```
 hello | wc                 hello 的 [1] = pipe 的寫端，wc 的 [5] = 讀端
 hello > out.txt            shell 建一條 pipe 給 hello 當 [1]，自己把讀端倒進檔案
 hello >> out.txt           同上，從檔案結尾接著寫
 wc < out.txt               shell 建一條 pipe 給 wc 當 [5]，自己把檔案灌進寫端
 run linux/hello x | wc     Linux 程式：[1] 是 pipe 寫端時 fd 1、2 就是它；[5] 是 fd 0
 proctest wait &            背景：印 [1] proctest wait，提示符馬上回來
 jobs / fg N / kill N       列出（結束的列一次就移除）、拉到前景等、全部 kill
```

- 解析：`libstela::pipeline`（純邏輯，host 測試）：`|`、`<`、`>`、`>>`、`&` 前後不必空白；`<` 只能給第一個程式、`>`／`>>` 只能給最後一個、`&` 必須在行尾。有這些符號的行，**每一段都是程式**（不是內建指令；開頭的 `run` 可寫可不寫）；沒有的行照舊。
- `[5]`（`bootstrap::STDIN`）：P11 定為標準輸入（pipe 讀端或檔案）。libstela `io::has_stdin()`、`io::read(buf)`（pipe → `PipeRead`，檔案 → `FileRead` 記著位置）；輸出仍是 `[1]`——pipe 的寫端收 `CONSOLE_WRITE`（`docs/ipc.md` §14），所以 `println!` 不用改。
- 前景：shell 一個迴圈同時做四件事——把 `<` 的檔案一塊塊灌進 pipe（不等）、把 `>` 的 pipe 倒進檔案、每 10 ms 看最後一段結束沒、把鍵盤轉給 tty（Ctrl-C kill 每一段）。最後一段結束、輸出也倒完後，其他段給 1 秒，還沒結束就 kill；印 `prog exited with code N`（最後一段的）。
- 背景：不能有 `<`／`>`（shell 不在旁邊灌與倒）；輸出照樣到 terminal；背景工作拿不到鍵盤。
- 新程式 `wc`：數 `[5]` 的行、字、位元組（`wc: L lines, W words, B bytes`）；沒有輸入就說明後結束（1）。

### 6.2 行編輯與歷史（P11，使用者已於 2026-10-22 審核）

`libstela::line::LineEditor`（純邏輯，host 測試）：游標可以在行中間——打字插入、Left／Right、Home／End（Ctrl-A／Ctrl-E）、Backspace 與 Delete、Ctrl-K（刪到行尾）、Ctrl-U（整行）；Up／Down 翻歷史（翻的時候正在打的那行記著，Down 翻過最新的就回來）。編輯器回傳**要印的字串**，只用 Backspace（游標左移一格、不擦）與空白：例如在中間插入 `x` 印 `x` + 後面的字 + 同樣多個 Backspace——任何 terminal（STELA 的文字格、kernel 的主控台、serial）都顯示得對，不需要 ANSI 的游標控制。

- 歷史最多 200 行（`HISTORY`），空行與和上一行一樣的不記；每執行一行就整個寫回 `Data:/.shell_history`（沒有 Data 就只在記憶體），shell 啟動時讀回來。
- shell 把 `KeyCode::Left/Right/Up/Down/Home/End/Delete` 對到 `Key`，其他鍵經 `key_to_char`；只看按下。
- 測試：`tools/qemu_test.py` 的 `--type` 文字可以寫 `{up}`、`{left}`、`{home}`、`{end}` 等鍵（P11）；`make test-shell` 最後 `{up}{up}`（再跑一次 `mem`）與 `vls{left}{left}o{end}`（變成 `vols`）。

### 6.3 Tab 補全與萬用字元（P11 K44，使用者已於 2026-10-22 審核）

```text
 Data:/t> cat /welc<Tab>     →  cat /welcome.txt ␣      （唯一：補完，檔案加空白、資料夾加 /）
 Data:/t> ls /lx/t<Tab>      →  （time、tty 沒有更長的共同開頭）
 time  tty                       換行列出，再印一次提示符與這一行
 Data:/t> ls /lx/t
 Data:/t> echo /lx/*s        →  echo /lx/files /lx/nosys
```

- 編輯器：Tab 回傳 `Edit::Complete`（以前當空白）；`word()` 給游標前的字（從上一個空白算起）和「是不是第一個字」，`insert(text)` 像打字一樣插入，`redraw()` 印整行並把游標放回原位（提示符由 shell 印）。
- 候選：第一個字是指令——內建指令（`BUILTINS`）＋目前資料夾、`Initrd:/`、`Initrd:/bin/`、`System:/Programs/`、`System:/bin/` 的程式（檔案與 `Name.app`）；其他字（或第一個字有 `/`、`:`）是路徑——`path::split_last` 切成資料夾與開頭，列那個資料夾裡開頭相同的名字（不分大小寫；`.` 開頭的名字只在打了 `.` 時出現）。一個候選就補完；多個就補到共同的開頭（`path::common_prefix`），沒有可以補的就列出來。
- 萬用字元：命令以外的字，最後一段有 `*` 或 `?`（資料夾的部分沒有）就換成那個資料夾裡符合的名字（`path::glob`：`*` 任意長度、`?` 一個字元、不分大小寫；排序；`.` 開頭的名字只給 `.` 開頭的模式），保留打的資料夾部分（`/lx/*s` → `/lx/files`）；沒有符合的就保留原字（和 sh 一樣）。管線的每一段也展開；`<`、`>` 後面的檔名不展開。
- 不做：`~`（沒有家目錄，`Data:/` 就是）、`alias`、`[abc]`、引號跳脫（shell 沒有引號）。
- 測試：`make test-shell` 的 `echo /lx/*s`、`cat /welc{tab}`、`upt{tab}`（內建指令）、`ls /lx/t{tab}ty`（列出 `time  tty` 後再打完）；`path` 與 `line` 的 host 測試（`glob`、`split_last`、`common_prefix`、`word`、`insert`、`redraw`）。

## 7. 怎麼寫一個程式

```rust
#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;
use libstela::{println, process};

libstela::entry!(main);

fn main() -> i64 {
    let args = process::args();
    println!("hello, {} argument(s)", args.len());
    0
}
```

1. `user/<名稱>/`：`Cargo.toml`（依賴 `libstela`）、`build.rs`（`-T user/user.ld`，照抄 hello）、`src/main.rs`。
2. 加進 workspace（`Cargo.toml` 的 members）與 `Makefile` 的 `USER_PROGRAMS`（打包進 initrd）。
3. `make run`，在 shell 裡 `run <名稱>`；或用 `ehfs-cp` 放進 Data 的映像，從 Data 執行。
4. 用得到的 handle：`[0]` 主 port、`[1]` console、`[2]` 啟動者給的、`[3]` 工作目錄。

## 8. 測試

| 測試 | 內容 |
|---|---|
| host `libstela` | P11：行編輯（游標移動與中間插入、Home／End／Delete／Ctrl-K、畫面模擬檢查印出的字串）、歷史（上下翻、記著正在打的行、200 行上限、不記重複）；管線的解析（段、`<`、`>>`、`&`、各種錯誤）；路徑解析（相對、`..`、根、卷宗、冒號在名字裡）、行編輯（Backspace、Ctrl-U、上限、控制字元、Tab、UTF-8）、參數切分 |
| host `abi` | 編號 27–31、`KILL`、`ProcessInfo`／`MemInfo` 編碼、`BOOTSTRAP_ARGS`、`EXIT_KILLED` |
| host `frame::console::text` | backspace 退一格、不會小於 0 |
| `make test-pipe`（P11） | 一般 kernel + AHCI 測試磁碟：`hello | wc`（3 行）、`hello > out.txt`、`cat out.txt`、`hello >> out.txt`、`wc < out.txt`（6 行）、`run linux/hello x | wc`（5 行）、`proctest wait &`、`jobs`、`kill 1`（137）、`spin &`、`fg 2` |
| selftest "processes" | `proctest` 跑兩次（第二次前後 PMM 頁框數相同、沒有 Zombie） |
| `proctest`（使用者程式） | heap：1 MiB 的 `Vec`、1000 個 `Box`；自己的參數；從 `Initrd:/proctest` 啟動自己並傳 300 bytes 的參數（走 Memory）→ exit 42、再等一次同樣的值；另一個永遠等的子行程：10 ms 逾時、行程表裡有兩個 proctest、kill → 137；spawn 資料夾 → `IsDir`、wait 檔案 → `WrongType`、wait 自己 → `AccessDenied`；`SysInfo` 的記憶體與時間 |
| `make test-fb` | 一般 kernel：看到 `STELA shell` 後打 `uptime`，看到 `uptime:` 後 screendump |
| `make test-shell` | 一般 kernel + AHCI 測試磁碟：`vols`、`ls Data:/`（`welcome.txt`）、`mkdir t`、`cd t`、`pwd`（`Data:/t`）、`cat nothing`（`cat: Data:/t/nothing: not found`）、`run hello`（輸出與 `exited with code 0`）、`ps`（有 shell）、`mem`、`uptime`；每個指令在前一個指令的那一行出現後才打。P11：歷史與游標（`{up}{up}`、`vls{left}{left}o{end}`）、`hwinfo`、`cat /welcome.txt \| grep -i stela \| wc -l`、`grep -n`、`cp -r`、`find -name`、`date`、萬用字元 `echo /lx/*s`、Tab 補全（`cat /welc{tab}`、`upt{tab}`、`ls /lx/t{tab}ty`）、背景的 `Monitor.app &` |
| P11 的應用程式 | `make test-editor`（Kalamos）、`make test-browser`（Periplus）、`make test-window`（剪貼簿），見 `docs/apps.md` |

`tools/qemu_test.py --type TRIGGER TEXT`：把文字轉成 QKeyCode（大寫與 `:`、`_` 用 shift），可以給很多次，依序在各自的 TRIGGER 出現時打。

## 9. 已知限制

- P11 起 `ThreadCreate` 回 Thread handle、有 `ThreadJoin`（`docs/process.md` §8.1）；沒有單一執行緒的 kill。
- 沒有環境變數；管線與重新導向只接程式（內建指令不行，例如 `ls | wc`；P11 K33 起 `cat`、`echo` 等有程式版，`docs/apps.md` §1）、沒有 `2>`、引號與跳脫、萬用字元展開；背景工作不能重新導向；`run` 的程式拿不到鍵盤（除非它自己 `InputAttach`）。
- 前景工作執行時打的字轉給它的 tty，它沒讀就結束時那些字會遺失（Unix 會留給 shell）。
- shell 一行 200 字元（P11 起有歷史與游標移動、Tab 補全與萬用字元，§6.2、§6.3；沒有搜尋歷史、引號）；`cp` 整個檔案讀進記憶體。
- 行程表的已結束紀錄永遠不清（`ps` 只顯示數量）。
- heap 不把記憶體還給 kernel；沒有 `realloc` 的就地成長（GlobalAlloc 的預設：配新的再複製）。
- P6.5 加了 `shutdown`／`reboot`（`docs/acpi.md`）；關機前不通知其他程式。
- 能力（P11，§2.3）：`NETWORK` 與 `INIT` 預設傳給所有子行程（還沒有拿掉的沙箱）；能力不能撤回（持有的 handle 一直有效）；init 不問使用者就執行 `POWER_REQUEST`。
