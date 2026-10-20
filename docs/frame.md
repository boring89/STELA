# frame：kernel 與硬體之間的那一層

> P1 由自動排程一次做完，使用者已於 2026-10-20 審核。

`frame/` 是 STELA 唯一可以寫 `unsafe` 的 kernel crate（framekernel 架構）。kernel 與之後的驅動都是 `#![forbid(unsafe_code)]`，只能透過 frame 提供的 safe API 碰硬體。

## 1. 比喻：配電盤

kernel 像住戶，frame 像大樓的配電盤：危險的高壓線（直接寫記憶體位址、CPU 暫存器、I/O port）全部關在配電盤裡，住戶只看得到插座（safe 函式）。插座的設計要保證「不管怎麼插都不會觸電」——也就是 safe API 不能讓呼叫者造成未定義行為（UB）。

## 2. 模組與邊界

```
frame/src/
  lib.rs              init()、halt()、breakpoint()、report_panic()、entry_point!
  arch/x86_64/        只在 target_os = "none" 編譯
    io.rs             in/out 指令（crate 私有）
    gdt.rs            GDT 與 TSS
    idt.rs            IDT
    exceptions.rs     32 個例外的組語 stub 與 Rust 分派
  serial.rs           COM1
  qemu.rs             isa-debug-exit
  trap.rs             TrapFrame 與例外報告格式（純邏輯，host 測試）
  sync.rs             SpinLock（host 測試）、RacyCell
  mm/
    bitmap.rs         位元圖（純邏輯，host 測試）
    frames.rs         頁框配置邏輯（純邏輯，host 測試）
    pmm.rs            全域 PMM、PhysFrame
    paging.rs         kernel 端的 map/unmap/translate
    heap.rs           #[global_allocator]
  console/
    font.rs           Spleen 字型（純邏輯，host 測試）
    text.rs           文字格線與捲動（純邏輯，host 測試）
    fb.rs             framebuffer canvas、主控台、panic 畫面
    mod.rs            kprint!／kprintln!
```

規則：碰硬體的程式碼用 `#[cfg(target_os = "none")]` 隔開，純邏輯放在可以在 host `cargo test` 的模組。frame 在 host 上能編（`make test-host`）。

kernel 啟動時只要：

```rust
frame::entry_point!(kernel_main);      // 產生 #[unsafe(no_mangle)] _start
fn kernel_main(info: &'static BootInfo) -> ! {
    frame::init(info);                 // serial → GDT/TSS/IDT → PMM → 分頁 → heap → 畫面
    kprintln!("STELA kernel {}", ...);
    ...
}
```

`entry_point!` 存在的原因：`#[unsafe(no_mangle)]` 屬於 `unsafe_code` lint，kernel 的 `forbid` 不允許；由外部 crate 的巨集展開的程式碼不受 kernel 的 lint 限制。

## 3. Serial（COM1）

16550 相容 UART，I/O port `0x3F8`（OSDev Wiki "Serial Ports"）。初始化：關中斷 → DLAB=1 設除數 3（115200/3 = 38400 baud）→ 8N1 → 開 FIFO → DTR+RTS。輸出時輪詢 line status 的 bit 5（傳送暫存器空）。`\n` 轉成 `\r\n`。

serial 沒有鎖（每次寫一個位元組是原子的），所以 panic 時一定能印。這是測試的主要輸出（`-serial stdio`）。

## 4. GDT 與 TSS

### 4.1 為什麼 64 位元還要 GDT

64 位元模式幾乎關掉了分段（base 一律 0、不檢查 limit），但 CPU 仍然要：

- 一個 L=1 的程式碼段，才知道自己在跑 64 位元程式碼；
- 一個資料段給 SS；
- 一個 TSS，裡面的 **IST**（Interrupt Stack Table）讓特定例外換到已知完好的堆疊。

UEFI 的 GDT 在 boot services 記憶體裡，`ExitBootServices` 之後可能被覆蓋，所以 kernel 要換成自己的。

### 4.2 內容（Intel SDM Vol. 3A §3.4.5、§5.2.1、§7.2.3、§7.7）

```
索引  selector  描述子                              值
0     0x00      null
1     0x08      kernel code：P、DPL0、type A、L、G   0x00AF_9A00_0000_FFFF
2     0x10      kernel data：P、DPL0、type 2、D/B、G 0x00CF_9200_0000_FFFF
3-4   0x18      64 位元 TSS（16 bytes，type 9）      執行時填入 TSS 位址
```

TSS 是 104 bytes（Figure 7-11）：`rsp[0..3]`（之後有 user mode 時用 RSP0）、`ist[1..8]`、I/O bitmap 位移（設成 104 = 沒有 bitmap）。IST1 指向 16 KiB 的靜態堆疊，給 double fault 用：如果 kernel 堆疊溢位，page fault 推不進堆疊會變成 double fault，這時一定要換堆疊才能印出訊息。

載入：`lgdt` → 用 far return（`push 0x08; push 標籤; retfq`）重新載入 CS（CS 不能用 `mov` 寫）→ `mov` DS/ES/FS/GS/SS = 0x10 → `ltr 0x18`。

## 5. IDT 與例外

### 5.1 IDT（Vol. 3A §6.10、§6.14.1）

256 個 16-byte gate。每個 gate：handler 位址（拆三段）、selector 0x08、IST 索引、屬性 `0x8E`（存在、DPL0、64 位元 interrupt gate——進入時自動關中斷）。

P1 只填 0–31（CPU 例外）；32–255 不存在，意外的中斷會變成 #GP → panic。向量 8（double fault）用 IST1。

### 5.2 Stub 與 TrapFrame

CPU 進入例外時自動推入 SS、RSP、RFLAGS、CS、RIP，部分例外（8、10–14、17、21、29、30；Vol. 3A Table 6-1）再推一個 error code。為了讓 Rust 看到一致的結構，每個向量有一段組語 stub：

```
沒有 error code 的向量：push 0 ; push 向量 ; jmp common
有 error code 的向量：            push 向量 ; jmp common
common：push 15 個通用暫存器 → mov rdi, rsp → call exception_dispatch
        → pop 回來 → add rsp, 16 → iretq
```

堆疊（低位址在上）就是 `frame::trap::TrapFrame`：

```
rsp → r15 r14 r13 r12 r11 r10 r9 r8 rbp rdi rsi rdx rcx rbx rax
      vector  error_code  rip  cs  rflags  rsp  ss        （共 22 個 u64 = 176 bytes）
```

對齊：64 位元模式下 CPU 推入前先把 RSP 對齊 16（Vol. 3A §6.14.2）；176 是 16 的倍數，所以 `call` 時 RSP 也對齊 16，符合 System V ABI。

### 5.3 處理

- 向量 3（`int3`，breakpoint）：計數後返回，`iretq` 回到 `int3` 的下一條指令。`frame::breakpoint()` 用它測試整條路徑。
- 其他：讀 CR2（page fault 的位址），`panic!` 印出：

```
exception 6 (#UD invalid opcode), error code 0x0
  RIP=0xffffffff800014a4 CS=0x0008 RFLAGS=0x00010002
  RSP=0xffff80001de6bf40 SS=0x0010 CR2=0x0000000000000000
  RAX=… RBX=… RCX=… RDX=…
  …
```

## 6. 實體記憶體（PMM）

### 6.1 比喻與原理

實體記憶體切成 4 KiB 的「頁框」，像停車場的格子。bitmap 是門口的看板：一格一個位元，1 = 有車（使用中），0 = 空位。

```
頁框號   0 1 2 3 4 5 6 7 8 9 …
bitmap   1 1 0 0 0 1 0 0 1 1 …
             └─ alloc_frame() 找到第一個 0，設成 1，回傳 2 × 4096 = 0x2000
```

### 6.2 建立（`mm::frames::FrameAllocator::new`）

1. bitmap 涵蓋到「最高的 Usable 區段結尾」（不是記憶體表的最高位址——QEMU 有個 1 TiB 處的 Reserved 洞，會讓 bitmap 變成 32 MiB）。QEMU 512 MiB：130,804 個頁框，bitmap 16,352 bytes（4 頁）。
2. bitmap 本身放在最大的 Usable 區段開頭（跳過頁框 0），透過 HHDM 存取。
3. 全部設 1 → 清掉 Usable 區段 → 把 bitmap 自己與頁框 0 設回 1。`KernelAndBoot`（kernel、loader 分頁表、堆疊、BootInfo）、ACPI、Reserved 一律維持使用中。

結果：512 MiB 的 QEMU 有 128,384 個空頁框（約 501 MiB）。

### 6.3 API（`frame::mm::pmm`）

| 函式 | 說明 |
|---|---|
| `alloc_frame() -> Option<PhysFrame>` | 從上次的位置往後找第一個空位（word 為單位跳過全滿的 64 格） |
| `alloc_zeroed_frame()` | 同上，並透過 HHDM 清零 |
| `free_frame(PhysFrame)` | 重複釋放會 panic |
| `alloc_contiguous(n) -> Option<PhysFrames>` | first-fit 線性搜尋（簡單版） |
| `stats() -> (free, total)` | |

`PhysFrame` 是不能複製、不能在 frame 外建立的「所有權憑證」：拿到它的人才能釋放它，因此 safe 的 `free_frame` 不會讓兩個人用到同一個頁框。

## 7. 分頁（kernel 端）

loader 建好的分頁表由 kernel 接手（格式見 `docs/boot.md` §7）。kernel 透過 HHDM 讀寫分頁表（實體位址 `p` 的表在 `HHDM_BASE + p`）。

- `paging::init`：清掉 PML4[0..256]（loader 的 identity map），重寫 CR3 讓 TLB 失效（Vol. 3A §4.10.4.1）。
- `map(virt, PhysFrame, Flags)`：只允許在**動態區** `0xFFFF_FF00_0000_0000..+512 GiB`（PML4[510]），所以 safe code 動不到 kernel 映像與 HHDM。缺的中間表從 PMM 配置清零的頁框。映射吃掉 `PhysFrame`。
- `unmap(virt) -> PhysFrame`：清掉項目、`invlpg`，把頁框的憑證還回來。
- `translate(virt)`：任何位址，會處理 2 MiB／1 GiB 大頁。
- NX：只有 loader 開了 `EFER.NXE`（`BootInfo::nx_enabled`）才設 bit 63。

```
虛擬位址                       內容
0xFFFF_8000_0000_0000 ...      HHDM
0xFFFF_FF00_0000_0000 ...      動態區：heap（4 MiB），之後是其他映射
0xFFFF_FFFF_8000_0000 ...      kernel 映像
```

## 8. Kernel heap

`HEAP_BASE = 0xFFFF_FF00_0000_0000`，固定 4 MiB：從 PMM 拿 1024 個頁框（不需要連續）一頁一頁映射上去，交給 `linked_list_allocator::Heap`（MIT/Apache-2.0，關掉預設 feature 所以不帶 `spinning_top`），外面包 frame 自己的 `SpinLock`，註冊成 `#[global_allocator]`。之後 kernel 就能用 `Vec`、`String`、`Box`。`heap::stats()` 回報大小、已用、可用。

## 9. 鎖

`frame::sync::SpinLock<T>`：一個 `AtomicBool`，`lock()` 用 compare-exchange 搶，搶不到就 `spin_loop`；回傳的 guard 在 drop 時解鎖。P1 只有一顆 CPU、沒有中斷，所以其實不會搶；之後中斷處理程式也要拿鎖時（P2），必須在持鎖期間關中斷，否則同一顆 CPU 會自己卡死。

`RacyCell<T>`：只在開機單執行緒階段寫入、再交給硬體的 static（GDT、TSS、IDT），每次存取都是 `unsafe` 並說明理由。

## 10. 主控台

### 10.1 字型

Spleen 2.1.0（Frederic Cambus，BSD-2-Clause），只取 ASCII 0x20–0x7E；沒有的字元畫成 `?`。`tools/font_to_rs.py` 把 BDF 轉成 `third_party/spleen/spleen_*.rs`，以 `include!` 編進 frame。每個字形是「高度 × ⌈寬/8⌉ bytes」，最高位元是最左邊的像素：

```
'A'（8x16）   0x00 ........
              0x00 ........
              0x7C .#####..
              0xC6 ##...##.
              …
              0xFE #######.
```

畫面寬 ≥ 1920 用 16x32，否則 8x16（OVMF 預設 1280x800 → 160 欄 × 50 列）。

### 10.2 結構

```
kprintln!  ──►  serial（無鎖）
           └─►  fb::write_str ── try_lock ──► TextConsole（游標、換行、tab、捲動）
                                                   │ Canvas trait
                                                   ▼
                                              FbCanvas（HHDM + framebuffer 實體位址）
```

`TextConsole` 只透過 `Canvas` trait 畫圖，所以 host 測試可以用記憶體裡的假畫布驗證換行與捲動。主控台用 `try_lock`：如果在印字途中 panic，panic 再印字不會卡死（只是那段不上畫面，serial 仍有）。

像素：GOP 回報 `Rgb32`（byte 0 = 紅）或 `Bgr32`（byte 0 = 藍），`FbCanvas::encode` 依此組成 `u32`。捲動是把 framebuffer 整塊往上 `memmove`。

### 10.3 Panic 畫面

`report_panic`：先直接寫 serial（不經主控台的鎖），再畫 panic 畫面——另建一個 canvas（不拿主控台的鎖），清成深紅 #5C1A14，置中畫「S T E L A」「kernel panic」，訊息與位置以左對齊的方塊置中；之後停用主控台以免被覆蓋。訊息先格式化進 4 KiB 的堆疊緩衝區（不碰 heap，因為 heap 的鎖可能正被持有）。裂開的石碑圖之後由使用者設計。

## 11. 測試

| 測試 | 內容 |
|---|---|
| host（`cargo test -p frame`） | bitmap、FrameAllocator（含 double free）、SpinLock（多執行緒）、TrapFrame 位移與報告格式、字型、TextConsole 換行／捲動／tab |
| selftest（`make test-qemu`） | BootInfo 合理、`int3` 兩次被接到並返回、例外報告格式、頁框配置／釋放／連續配置、map/translate/unmap、identity map 已移除、Vec/String/Box 與 heap 統計 |
| `make test-qemu-min` | 同上，`-cpu Conroe,-nx -m 256M`（硬體基準、無 NX） |
| `make test-fb` | 一般 kernel 開機後 QEMU `screendump`，`tools/fb_check.py` 檢查畫面不是全黑、有至少 3 行字 |

selftest 結束時寫 `0x10` 到 `isa-debug-exit`（port `0xF4`），QEMU 以 `(0x10 << 1) | 1 = 33` 結束；panic 時寫 `0x11`（35）。

## 12. 已知限制與待辦

- 單核、沒有中斷、沒有計時器（P2）。
- 沒有設定 PAT，framebuffer 是 write-back 快取而不是 write-combining；捲動要讀 framebuffer，在實機上會慢。之後可以加 PAT／影子緩衝區。
- 畫字逐像素、沒有最佳化（`fill` 已改成逐列連續寫入）。
- 分頁：空掉的中間表不回收；只支援 4 KiB 映射；沒有 MMIO 映射 API（需要時加，型別要與 `PhysFrame` 分開）。
- heap 固定 4 MiB、不會成長。
- `alloc_contiguous` 是線性 first-fit。
- 例外處理一律 panic（除了 breakpoint）；沒有 page fault 的需求分頁。
