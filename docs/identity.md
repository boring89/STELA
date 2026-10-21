# 識別：石碑、裂碑與圖示

> P9 由自動排程一次做完，使用者已於 2026-10-20 審核。**所有圖案都是草稿**：使用者會自己畫正式的標誌與圖示；這份文件描述的是機制（格式、尺寸、產生流程、替換方式），不是定稿的設計。

STELA 的標誌是一塊**石碑**（stela）；kernel panic 時畫面上是**裂開的石碑**；檔案管理員叫 **Mouseion**；整體是古希臘主題、黑繪陶的配色（`CLAUDE.md` 的「識別」）。

程式碼與資料：

```
assets/NAME-SIZE.ppm、.pbm      圖示（草稿），assets/README.md
tools/make_draft_assets.py      畫草稿（幾何圖形）
tools/img_to_rs.py              PPM/PBM → Rust 資料（不需要 PIL）
assets/generated/               產生的 Rust 資料（不要手改）
frame/src/console/art.rs        kernel 畫石碑與裂碑（開機與 panic 畫面）
frame/src/console/fb.rs         boot_screen、end_boot_screen、panic_screen
user/toolbox/src/icons.rs       toolbox 的圖示（Mouseion 用）
```

## 1. 比喻：印章與模具

`assets/` 是**模具**（使用者可以換掉的圖），`tools/img_to_rs.py` 是**鑄造**：把模具變成程式裡用的金屬字（Rust 陣列）。kernel 與工具箱只認得鑄好的字，所以換圖不必改任何程式碼：換模具、重新鑄造（`make assets`）、重新建置。

## 2. 圖示

| NAME | 草稿畫的是 | 用在 |
|---|---|---|
| `stela` | 石碑：三角楣（pediment）、頂上的白點、刻字的橫線、底座 | 開機畫面（128，≥ 1920 寬放大 2 倍） |
| `stela-cracked` | 同一塊從中間裂開（鋸齒狀的縫，縫裡清空） | panic 畫面（128） |
| `temple` | 神殿（Mouseion）：三角楣、橫樑、四根柱子、台基 | About Mouseion |
| `scroll` | 卷軸（文件）：羊皮紙、兩端卷起、文字線 | 檔案 |
| `kibotos` | 希臘木箱（κιβωτός，資料夾）：蓋子、箱身、回紋（meander）帶、腳 | 資料夾 |
| `volume` | 磁碟：箱形、凹槽、指示燈 | 卷宗 |
| `app` | 小石碑加一個印章（圓） | bundle、程式 |
| `trash` | （不做：沒有垃圾桶） | |

尺寸：16、32、64、128（正方形）。每個尺寸都分別畫（不是縮放），所以小尺寸也能清楚——草稿是用同一組幾何形狀依比例算出來的，16 像素的效果最差。

## 3. 格式

- **PPM**（binary P6，每色 8 位元）：彩色版；**`#FF00FF` 是透明**（PPM 沒有 alpha）。一張圖最多 255 色（不含透明）。
- **PBM**（binary P4）：1 位元版，1（黑）是墨；草稿的規則是「不透明也不是羊皮紙白的像素」。kernel 與 Mouseion 目前用彩色版；1 位元版給之後的單色顯示與選取時的遮罩（`Canvas::icon_mono`）。
- 註解行 `# DRAFT …` 標明草稿。

### 3.1 產生的 Rust 資料（`tools/img_to_rs.py`）

```rust
Icon {
    name: "stela", size: 128,
    palette: &[0x000000, 0x1A1410, 0xC8763C, …],   // index 0 = 透明
    runs: &[38, 0, 4, 1, 11, 0, …],                 // (個數, 調色盤 index) 一對一對，逐列
    mono: &[0x00, 0x18, …],                         // 每列 size.div_ceil(8) bytes，MSB 在左
}
```

- run-length：草稿有大片同色，128 的圖約 17 KB 的原始碼（4 bpp 的直接編碼要 40 KB 以上）。
- 每個圖一個 `NAME-SIZE.rs`（一個運算式），`icons.rs` 列出全部（`STELA_128` … 與 `ALL`）。toolbox `include!` `icons.rs`；kernel 只 `include!` 兩個 128 的石碑（不把全部圖示放進 kernel）。
- 畫的時候逐 run 解碼，不配置記憶體（panic 畫面也能用）。

## 4. 顏色（黑繪陶）

| | |
|---|---|
| terracotta | `#C8763C`（陶土底色；作用中的標題列） |
| 釉黑 | `#1A1410`（輪廓、文字） |
| 羊皮紙白 | `#F2E8D5`（點綴；選單、視窗底） |
| 暗陶土（草稿用） | `#8E4A22`（陰影、底座） |
| panic 底色 | `#5C1A14`（暗龐貝紅） |
| 開機畫面底色 | `#101014`（主控台的底色） |
| 桌面 | `#5B7B86`（愛琴海灰藍） |

## 5. 開機畫面與 panic 畫面

```
 開機：frame::init 之後 boot_screen()：底色、石碑（置中偏上）、S T E L A
       畫面主控台暫停（訊息照樣到 serial）
       ├─ 視窗伺服器 map framebuffer → 接手（docs/display.md）
       ├─ 一般 kernel 5 秒內沒有人接手（文字模式）→ end_boot_screen()：清畫面、主控台繼續
       └─ selftest kernel：1 秒後 end_boot_screen()
 panic：panic_screen()：底色 #5C1A14、裂碑、S T E L A、kernel panic、訊息與位置
        不管畫面在誰手上都畫；畫完在 serial 印 (the panic screen is up)
```

`make test-panic`：selftest kernel 配一個多了 `panic-test` 檔的 initrd：selftest 一開始看到它就故意 panic（`selftest: the panic screen test`），panic handler 停住而不結束 QEMU；截圖檢查左上角是 panic 底色、石碑左面是 terracotta；P11 起底部有 kernel 的最後 10 行訊息（`docs/p520.md` §3.1），截圖也檢查那一塊有至少 8 行字。

## 6. 替換流程（給使用者）

1. 畫新的圖，存成 `assets/NAME-SIZE.ppm`（透明用 `#FF00FF`）與 `.pbm`，名稱與尺寸照 §2。
2. 刪掉 `assets/README.md` 與 PPM/PBM 註解裡的 DRAFT 字樣（`img_to_rs.py` 產生的檔頭也寫著 DRAFT：改 `tools/img_to_rs.py` 的那兩行）。
3. `make assets`（只跑 `img_to_rs.py`，**不會**覆蓋 `assets/` 的圖）。
4. `make test`：host 測試檢查每個圖都解得出正確大小、`assets/generated/` 與圖檔一致；`make test-panic` 檢查裂碑的位置（若新圖的石碑左面不是 terracotta，要改 Makefile 的 `PANIC_FACE` 與 `MOUSEION_CHECKS`）。
5. 不要再跑 `make draft-assets`（它會把草稿畫回來）。

## 7. 待使用者決定

- 正式的標誌與所有圖示（本階全是草稿）；石碑頂部用三角楣還是棕櫚葉飾（palmette）。
- 選單名稱（`Mouseion`、`File`、`Edit`、`View`、`Special`）、`Viewer` 的名字、STELA 字樣是否變成一個選單（像 Classic 的 Apple 選單）。
- 開機畫面停留多久、要不要進度提示。
