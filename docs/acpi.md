# ACPI 的 FADT、`\_S5` 與關機、重新開機

> P6.5 由自動排程一次做完，**使用者尚未審核**；之後修改前請先帶使用者過一遍這份文件。

讓 STELA 自己把電腦關掉或重新開機：從 ACPI 表格找出電源管理暫存器在哪裡、關機要寫什麼數字，先把檔案系統收好，再請晶片組斷電。MADT（中斷）與 MCFG（PCI）見 `docs/irq.md`、`docs/pci.md`；RTC 見 `docs/time.md` §5.5。

程式碼：

```
frame/src/acpi/fadt.rs     FADT 解析（純邏輯，host 測試）
frame/src/acpi/aml.rs      DSDT 裡 \_S5 的最小 AML 掃描（純邏輯，host 測試）
frame/src/acpi/mod.rs      找表格：fadt()、dsdt()、s5()
frame/src/power.rs         shutdown()、reboot()、can_shutdown()（port I/O，unsafe 在這裡）
kernel/src/power.rs        先 vfs::unmount_all() 再交給 frame
kernel/src/psyscall.rs     SystemPower syscall（32）
user/libstela/src/process.rs  process::shutdown()、reboot()
user/shell                 shutdown、reboot 指令
```

## 1. 比喻：大樓的總開關

ACPI 表格是大樓的**配電圖**：FADT 寫著「總開關在幾號配電箱（PM1a 控制暫存器的 I/O port）」；DSDT 是一本用 AML（一種小程式語言）寫的**操作手冊**，裡面有一頁 `\_S5`：「要完全斷電，請把開關撥到第 N 格」。

STELA 不讀整本手冊（那需要一個 AML 直譯器，幾萬行），只翻到 `\_S5` 那一頁抄下數字。關機前先叫每個卷宗**把帳記完、蓋上「正常結束」的章**（eHFS 標成 clean 並 flush），再撥開關。

## 2. 名詞

| 名詞 | 意思 |
|---|---|
| FADT | Fixed ACPI Description Table（簽名 `FACP`）：固定位置的電源管理資訊 |
| DSDT | Differentiated System Description Table：用 AML 描述的裝置與方法 |
| AML | ACPI Machine Language：DSDT／SSDT 的位元組碼 |
| S5 | 「soft off」：電源管理的睡眠狀態之一，等於關機（S3 是待命、S4 是休眠） |
| SLP_TYP | 寫進 PM1 控制暫存器 bit 12:10 的睡眠類型；每台機器不同，所以要從 `\_S5` 讀 |
| SLP_EN | PM1 控制暫存器 bit 13：寫 1 就進入 SLP_TYP 指定的狀態 |
| SCI_EN | PM1 控制暫存器 bit 0：機器在 ACPI 模式（不是舊的 APM／legacy 模式） |
| GAS | Generic Address Structure：12 bytes 的「位址 + 位址空間（記憶體／I/O）」 |

## 3. FADT（ACPI 6.5 §5.2.9，Table 5.9）

| offset | 欄位 | 用途 |
|---|---|---|
| 40 | DSDT（u32） | DSDT 的實體位址 |
| 48 | SMI_CMD（u32） | 寫 ACPI_ENABLE 到這個 port → 韌體切到 ACPI 模式 |
| 52 | ACPI_ENABLE（u8） | |
| 64、68 | PM1a_CNT_BLK、PM1b_CNT_BLK（u32） | PM1 控制暫存器的 I/O port（b 通常是 0） |
| 108 | CENTURY（u8） | RTC 的世紀暫存器編號（`docs/time.md` §5.5） |
| 109 | IAPC_BOOT_ARCH（u16） | bit 1：有 8042 鍵盤控制器 |
| 112 | Flags（u32） | bit 10 RESET_REG_SUP |
| 116 | RESET_REG（GAS） | 重新開機暫存器 |
| 128 | RESET_VALUE（u8） | 要寫的值 |
| 140 | X_DSDT（u64） | ACPI 2.0+；非 0 時優先 |
| 172、184 | X_PM1a/b_CNT_BLK（GAS） | 舊欄位是 0 時才用（只接受 I/O 空間） |

- ACPI 1.0 的 FADT 只有 116 bytes（到 RESET_REG 之前），之後的欄位當成沒有；`IAPC_BOOT_ARCH` 缺席時假設有 8042。
- RESET_REG 只接受 I/O（space 1）與記憶體（space 0）；PCI 設定空間（space 2）等不支援。
- FADT 或 DSDT 壞掉只印警告（沒有就不能 ACPI 關機、沒有世紀暫存器），不像 MADT 是必要的。
- QEMU q35：`acpi: FADT PM1a 0x604, S5 yes, century 0x32, reset yes, 8042 yes`；reset 暫存器是 port `0xCF9`、值 `0x0F`；DSDT 8457 bytes；SLP_TYP 0/0。

## 4. `\_S5` 的掃描（`acpi/aml.rs`）

韌體幾乎都把 `\_S5` 寫成一個常數 Package：

```
 08            NameOp
 [5C]          可有可無的根前綴 '\'
 5F 53 35 5F   "_S5_"
 12            PackageOp
 pp [pp..]     PkgLength：第一個 byte 的 bit 7-6 = 後面還有幾個 byte（0-3）
 nn            NumElements
 0A tt | tt    SLP_TYPa：BytePrefix + 一個 byte，或 ZeroOp（00）／OneOp（01）
 0A tt | tt    SLP_TYPb
```

（ACPI 6.5 §20.2.5 "Term Objects Encoding"、§20.2.4 "Package Length Encoding"、§7.4.2 "\_Sx"。）

- 從 DSDT 的 AML（header 36 bytes 之後）找 `_S5_`，前面要是 NameOp（或 NameOp + `\`），後面要是 PackageOp；否則繼續找下一個。
- ZeroOp、OneOp 的值剛好等於它代表的數字，所以單一 byte 直接當數字。只有一個元素的 Package → SLP_TYPb = SLP_TYPa。值只取低 3 位元。
- 用方法算出來的 `\_S5`、放在 SSDT 的 `\_S5` 找不到 → `can_shutdown()` 為 false → `SystemPower(SHUTDOWN)` 回 `NotSupported`。和 68k_HAL 的 `acpi_find_s5` 同一個做法。

## 5. 關機（`frame::power::shutdown`）

```
 kernel::power::shutdown()
   can_shutdown()?（FADT 有 PM1a、DSDT 有 \_S5）── 否 → NotSupported（什麼都沒動）
   vfs::unmount_all()                ← eHFS：狀態標 CLEAN、commit、flush
   frame::power::shutdown()
     關中斷
     PM1a 的 SCI_EN 是 0？→ 寫 ACPI_ENABLE 到 SMI_CMD，最多等 3 秒
     PM1a ← (PM1a & ~(SLP_TYP|SLP_EN)) | SLP_TYPa << 10 | SLP_EN
     PM1b（若有）同上，用 SLP_TYPb
     等約 1 秒（讀 port 0x80；中斷關著時計時器不走）
     還活著 → Failed
```

- 回傳 `Failed` 時卷宗已經標成 clean 但還掛著；之後再寫入仍有日誌保護（fsck 會看到「clean」但內容一致）。這種情況在支援 `\_S5` 的機器上不該發生。
- QEMU 收到 S5 後自己結束，exit status 0。

## 6. 重新開機（`frame::power::reboot`）

依序嘗試，前一個沒效就下一個：

1. **ACPI reset 暫存器**：I/O → `outb(port, value)`；記憶體 → `mmio::map` 後寫一個 byte（新增 `MmioRegion::write_u8`）。
2. **8042**（FADT 說有 8042，或沒有 FADT）：等 input buffer 空、寫 `0xFE` 到 port `0x64`（脈衝 CPU 的 reset 線；OSDev "Reboot"）。
3. **triple fault**：`lidt` 一個 limit 0 的 IDT、`int3`：例外送不出去 → double fault 也送不出去 → 處理器 shutdown，晶片組重設（Intel SDM Vol. 3A §6.15）。

QEMU 用 `-no-reboot`（Makefile），重設會讓 QEMU 結束、exit status 0。

## 7. syscall 與 shell

| 編號 | 名稱 | 參數 | 回傳 |
|---|---|---|---|
| 32 | `SystemPower` | kind（`abi::power::SHUTDOWN` 0、`REBOOT` 1） | 成功不返回；`NotSupported`（不能 ACPI 關機）、`IoError`（寫了但沒關掉）、`InvalidArgument`（kind 不對） |

- **任何程式都能呼叫**（沒有權限或 handle 擋著）——記在限制裡，之後應該要求某個系統 handle。
- libstela：`process::shutdown()`、`process::reboot()`（只在失敗時返回 `Error`）。
- shell：`shutdown`、`reboot`；失敗時印 `shutdown: NotSupported` 之類。

```
Data:/> shutdown
kernel: volumes unmounted
power: turning off
```

## 8. 測試

| 測試 | 內容 |
|---|---|
| host `frame::acpi::fadt` | 完整 FADT（QEMU 的值）、X_DSDT 優先、X_PM1 退路、ACPI 1.0 的 116 bytes、reset 需要旗標與已知空間、沒有 8042、太短、簽名錯 |
| host `frame::acpi::aml` | BytePrefix、ZeroOp／OneOp、多 byte 的 PkgLength、單一元素、找不到（方法、`_S4_`、截斷、算出來的元素）、第一個不是 Package 時用下一個 |
| host `abi` | `SystemPower` = 32、kind 0／1 |
| selftest "acpi fadt" | 有 FADT、PM1a 非 0、找到 `\_S5`、`can_shutdown()`，印 SLP_TYP 與 reset 暫存器 |
| `make test-shutdown` | 一般 kernel + AHCI 測試磁碟：`mkdir before-shutdown`、`shutdown` → QEMU 自己結束（status 0）→ host 的 `fsck.ehfs` 說「unmounted cleanly」、`ehfs-ls` 看得到 `before-shutdown`；再開一次打 `reboot` → QEMU 結束（status 0）→ fsck 仍然 clean |

## 9. 已知限制

- 沒有 AML 直譯器：只認常數 Package 的 `\_S5`；不跑 `\_PTS`／`\_GTS` 等關機前的方法（大部分機器不需要，少數筆電需要）。
- 不看 SSDT；不支援 S3（待命）、S4（休眠）。
- `SystemPower` 沒有權限控管：任何程式都能關機。
- 關機前不通知使用者空間（程式沒有機會存檔）；不等程式結束，直接卸載卷宗（之後的寫入會失敗或寫進已標 clean 的卷宗）。
- 關機失敗的退路只有回報錯誤；沒有 QEMU／VirtualBox 專用的 port（68k_HAL 有）。
- reset 暫存器在 PCI 設定空間的機器不支援（退到 8042 → triple fault）。
