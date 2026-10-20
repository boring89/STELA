# 顯示：GOP 模式、PAT write-combining、Framebuffer 物件

> P8 由自動排程一次做完，**使用者尚未審核**；之後修改前請先帶使用者過一遍這份文件。

畫面怎麼從 kernel 的文字主控台交給使用者空間的視窗伺服器。架構決定見 `docs/architecture.md` §4 的 G（kernel 只提供 framebuffer 與輸入事件，視窗伺服器與繪圖在使用者空間）。視窗伺服器本身見 `docs/window.md`，畫圖的函式見 `docs/toolbox.md`。

程式碼：

```
boot/src/main.rs                 set_largest_mode：選最大的橫向 GOP 模式
frame/src/arch/x86_64/pat.rs     PAT：entry 4 = write-combining
frame/src/mm/device.rs           DeviceRange（可以給使用者映射的裝置記憶體）
frame/src/mm/user.rs             AddressSpace::map_device／unmap_device／is_write_combining
frame/src/console/fb.rs          FramebufferDevice、framebuffer()、hand_over()、ink_rows()
kernel/src/object/mod.rs         KernelObject::Framebuffer
kernel/src/syscall.rs            MemoryMap 對 Framebuffer、InputAttach 的政策
kernel/src/input.rs              set_owner、may_attach
kernel/src/psyscall.rs           SysInfo(FRAMEBUFFER)
kernel/src/main.rs               init 的 [2] = Framebuffer
abi/src/lib.rs                   sysinfo::FramebufferInfo、object_kind::FRAMEBUFFER
```

## 1. 比喻：黑板交班

開機時 kernel 拿著粉筆在黑板（framebuffer）上寫開機訊息（文字主控台）。視窗伺服器來了，kernel 把**黑板的鑰匙**（Framebuffer handle）交給它，從此 kernel 不再在黑板上寫字（只寫進 serial 的記事本）；除非出大事（panic），kernel 會衝上去把整塊黑板蓋掉畫裂開的石碑。

**write-combining** 像是寫黑板時先在手上累積一整句再一次寫上去，而不是一筆一筆跑過去：對 framebuffer 這種只寫不讀、記憶體又在顯示卡上的區域，快很多。

## 2. GOP 模式（loader）

`set_largest_mode`：走過 GOP 的模式清單（UEFI 2.10 §12.9 QueryMode），取**面積最大、寬 > 高**、像素格式 RGB 或 BGR（32 位元）的，`SetMode`；失敗就留在目前模式。

- 只取橫向：OVMF 也列出 2048×2048 這種正方形模式（計畫外的決定）。
- QEMU（OVMF、std VGA）：2560×1600，pitch 10240。
- 沒有設定檔（計畫 D1：固定選最大）。

## 3. PAT（`pat.rs`，Intel SDM Vol. 3A §13.12）

```
 entry  PAT PCD PWT   開機值     STELA
 0      0   0   0     WB (06)    WB          一般記憶體
 1      0   0   1     WT (04)    WT
 2      0   1   0     UC- (07)   UC-
 3      0   1   1     UC (00)    UC          map_device（MMIO）
 4      1   0   0     WB         WC (01) ←   framebuffer
 5..7                 WT UC- UC  不變
```

- `CPUID.01H:EDX[16]` 有 PAT 才設（所有 x86-64 都有）；MSR `IA32_PAT`（0x277）的 byte 4 改成 0x01。
- entry 4 以前沒有任何頁使用，所以改它不會讓同一頁有兩種記憶體型別。
- 4 KiB 頁的 PTE bit 7 是 PAT 位元：PAT=1、PCD=0、PWT=0 → entry 4。
- kernel 自己的 framebuffer 主控台仍走 HHDM（write-back）；同一塊實體記憶體有 WB（HHDM）與 WC（使用者）兩種映射，是已知的 aliasing（實務上 framebuffer 沒問題；記在限制）。

## 4. 裝置記憶體的映射

- `DeviceRange { phys, pages }`：只有 frame 能造（建構子 `pub(crate)`），所以 safe 的 `map_device` 不可能被拿來映射任意實體記憶體。目前唯一的來源是 `frame::console::framebuffer()`（開機 framebuffer，經過與主控台相同的檢查：在 HHDM 內、列數放得下）。
- `AddressSpace::map_device(virt, range, writable, write_combining)`：葉子 PTE = `phys | P | U | (W) | NX | bit 9 | (PAT 或 PCD|PWT)`；bit 9 是「借來的頁」（`docs/ipc.md` §5.2），位址空間拆掉時不會還給 PMM；映射記在 `devices` 清單，`unmap_device(virt)` 拿掉。
- `is_write_combining(virt)`：selftest 用來確認 PTE 選到 entry 4。

## 5. `Framebuffer` 物件

| 項目 | 內容 |
|---|---|
| 物件 | `KernelObject::Framebuffer(Arc<FramebufferDevice>)`：範圍、寬、高、pitch、是否 BGR |
| 誰拿到 | kernel 啟動 `Initrd:/init` 時放進它的 `[2]`（`MAP|READ|WRITE|DUPLICATE|TRANSFER`）；init 交給視窗伺服器的 `[2]` |
| `MemoryMap` | 要 `MAP|READ`（可寫再要 `WRITE`）；在映射區配位址、`map_device(…, write_combining = true)`；成功後 `frame::console::hand_over()`（文字主控台停畫，serial 照舊）並 `input::set_owner(pid)` |
| `MemoryUnmap` | 先試共享記憶體，再試裝置映射 |
| `HandleKind` | 8（`object_kind::FRAMEBUFFER`） |
| `SysInfo(FRAMEBUFFER)`（4） | `FramebufferInfo`（24 bytes）：width、height、pitch、format（1 RGBX、2 BGRX）、size、scale（固定 1）；沒有 framebuffer 回 0 筆 |

### 5.1 主控台的交班

```
 開機 ──► frame::console::fb：文字主控台（Spleen；≥ 1920 寬用 16x32）
 視窗伺服器 MemoryMap(Framebuffer) ──► hand_over()：write_str 不再畫，只到 serial
 panic ──► panic_screen()：不管 hand_over，直接畫整個畫面
```

### 5.2 `InputAttach` 的政策（計畫 D2 的細化）

- 還沒有人映射 framebuffer（文字模式）：任何有 port `SEND` 的程式都能 attach（P4 的行為；文字模式的 shell 要靠它拿回鍵盤）。
- 映射之後：只有映射它的行程（視窗伺服器）能 attach（其他 → `AccessDenied`）；它結束後恢復開放。
- 計畫寫的是「持有 Framebuffer handle 的行程」；實作成「映射它的行程」，效果相同、不必掃 handle 表。

## 6. HiDPI（D5）

`FramebufferInfo::scale` 固定 1，欄位先有。文字：寬 ≥ 1920 用 16x32（`toolbox::draw::font_for`），游標在 16x32 時放大 2 倍。

## 7. 測試

| 測試 | 內容 |
|---|---|
| host `abi` | `FramebufferInfo` 編碼往返、`sysinfo::FRAMEBUFFER`、`object_kind::FRAMEBUFFER` |
| selftest "framebuffer object" | 文字主控台畫了字（`ink_rows()` ≥ 96 列：取代 P6 的 `make test-fb` 截圖檢查）；framebuffer ≥ 640×480；映射進測試行程：`translate` 對到實體位址、PTE 是 write-combining、最後一頁正確、`unmap_device` 還回同一個範圍 |
| `make test-window` | 見 `docs/window.md` §9 |

## 8. 已知限制

- 只有開機 framebuffer（GOP）；不換解析度、沒有多螢幕、沒有 GPU。
- framebuffer 有 WB（HHDM）與 WC（使用者）兩種別名映射。
- kernel 的文字主控台仍是 WB，捲動要讀 framebuffer（實機慢）。
- `scale` 永遠 1。
