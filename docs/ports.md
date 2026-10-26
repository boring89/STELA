# Ports：在 STELA 上跑的 Linux 程式（P12、P13）

> P12 由自動排程一次做完，使用者已於 2026-10-22 審核。P13 的 K7–K8（§6）同日審核。

ports repo（暫名 `stela-ports`，和 STELA 分開的 git repo，本機在 `~/stela-ports`，還沒有放上 GitHub）收集「把一個現成的 Linux 程式編成 STELA 能跑的樣子」的配方：在哪裡拿原始碼、怎麼編、怎麼打包、在 STELA 裡怎麼測。編出來的套件由 `pkg` 安裝（`docs/pkg.md`）。

## 1. 比喻：食譜書

STELA 本身是一間廚房（kernel 與 Linux 相容層，`docs/linux.md`）。ports repo 是**食譜書**：每道菜（port）一頁，寫著材料從哪來（原始碼的網址與 SHA-256）、怎麼做（`build.sh`）、做好怎麼裝盒（`package`：一個 ustar 檔）、怎麼試吃（`test.sh`：在 QEMU 裡的 STELA 打指令、看輸出）。食譜書本身是 0BSD（誰都能抄），每道菜的材料各有自己的授權（寫在 `port.toml`）。

## 2. 一個 port 的樣子

```
ports/lua/
  port.toml     name = "lua"、version = "5.4.7"、license = "MIT"、
                source = "https://…" 或 copy-from = "~/…" 或 local = "crate"、
                sha256 = "…"、install = "bin"、depends = []、stela = "0.1.0"、
                description、homepage
  build.sh      在解開的原始碼裡執行（sh -e）：SRC、OUT、MUSL_GCC、STELA_ROOT、JOBS、PORT
                把要裝的東西放進 $OUT：bin/、lib/、share/，或 Name.app/
  test.sh       $ 指令（在 STELA 的 shell 打）、> 一行必須出現的輸出、
                = 上一個程式的 exit code、! QEMU 按鍵（例如 ctrl-q）
  files/        測試用的檔案，放到 Data:/test/
  patches/      *.patch（patch -p1）
```

編譯的規則（STELA 的 Linux 相容層只載入靜態、非 PIE 的 ELF）：

```
C：     $MUSL_GCC -O2 -static -no-pie -fno-pie -Wl,-z,max-page-size=4096
Rust：  --target x86_64-unknown-linux-musl
        RUSTFLAGS="-C relocation-model=static -C target-feature=+crt-static"
        （Rust 的 musl 目標預設是 static-pie，ET_DYN，STELA 不載入）
```

`MUSL_GCC` 是 STELA 的 `build/musl/install/bin/musl-gcc`（`make musl`）。

## 3. `tools/portbuild.py`

| 指令 | 做什麼 |
|---|---|
| `fetch NAME` | 下載（`source`）或複製（`copy-from`）到 `sources/`，檢查 SHA-256 |
| `build NAME` | 解開到 `work/NAME/src`、套 patch、跑 `build.sh`，輸出在 `work/NAME/out`；log 在 `work/NAME/build.log` |
| `package NAME` | `dist/NAME-VERSION-x86_64.tar`：ustar、owner 0、mtime 0；最上層是 `PKGINFO`（name、version、description、license、install、depends）與 `$OUT` 的內容 |
| `test NAME` | 複製 STELA 的 `build/images/ahci.img`，用 `ehfs-cp` 把套件解到 `Data:/Programs/NAME/`（依賴的套件也是）、`files/` 到 `Data:/test/`，開 QEMU（STELA 的 `build/esp`），依 `test.sh` 經 `tools/qemu_test.py` 打字與檢查 |
| `index` | `dist/index.json`：每個套件的名稱、版本、描述、授權、依賴、大小、SHA-256、檔名、URL（`PORTS_URL` 前綴）；K12 起加 ed25519 簽章（`docs/pkg.md`） |
| `all NAME` | build、package、test |
| `list` | 列出 port |

`NAME` 可以是 `all`。STELA 這邊的 `make test-ports`（**本機專用**，不進 `make test`、不進 CI）：先做好 `musl esp test-images ehfs-tools`，再 `portbuild.py all all`。

## 4. 第一批 port

| port | 版本 | 授權 | 原始碼 | 測試 |
|---|---|---|---|---|
| lua | 5.4.7 | MIT | 68k_HAL 的下載目錄（複製） | 函式、遞迴、coroutine、檔案 I/O、`os.time` |
| sqlite | 3.54.1（amalgamation 3540100） | public domain | 同上 | 建表、插入、聚合、排序（`-init` 讀 SQL 檔） |
| kilo | 0.0.1 | BSD-2-Clause | 同上 | 開檔（raw 模式、ANSI）、Ctrl-Q 離開 |
| dash | 0.5.12 | BSD-3-Clause | 同上 | 迴圈、函式、管線、`$(...)`、子 shell、跑別的程式 |
| make | 4.4.1 | GPL-3.0-or-later | 同上 | 依賴、自動變數、函式；recipe 由 dash 執行（預設 shell 改成 `/Data/Programs/dash/bin/dash`） |
| tcc | 0.9.27 | LGPL-2.1-or-later | 同上 | 在 STELA 裡 `tcc -static hello.c -o /tmp/hello` 再執行 |
| busybox | 1.36.1 | GPL-2.0-only | busybox.net（下載，SHA-256 `b8cc24c9…de314`，和官方 `.sha256` 一致） | ash 跑腳本：sort、awk、sed、grep、tar + gzip、md5sum、xargs |
| rust-hello | 0.1.0 | 0BSD | port 自己的 `crate/` | std::thread、mpsc、std::fs |

- BusyBox 從 `allnoconfig` 開始，只打開 `applets.txt` 列的 applet（需要 mount、網路設定、模組、裝置或 Linux 標頭的都不開）；`FEATURE_SH_STANDALONE` + `PREFER_APPLETS`：ash 直接跑內建的 applet（經 `/proc/self/exe` 重新執行自己，P12 的 execve 支援這個別名）。
- tcc 的路徑全部編死在 `/Data/Programs/tcc`（`include/` 是 musl 的標頭、`lib/` 是 musl 的 `crt*.o` 與 `libc.a`、`lib/tcc/` 是 tcc 自己的）。STELA 只跑靜態程式，所以要 `tcc -static`。
- 授權：GPL-2.0-only 的 BusyBox 是獨立的程式（套件），不連結進 STELA，和 STELA 的 GPL-3.0-or-later 沒有衝突（計畫的授權事項）。

## 5. 已知限制

- 套件只有 x86-64；除了 python3（動態連結 musl 的 libc.so，§6.2）都是靜態 musl；函式庫 port 只有靜態的 `.a`。
- `test` 用 STELA 的測試磁碟映像（128 MiB）；大的套件（tcc 的 libc.a）放得下，但不能太多個一起。
- 原始碼大多從 68k_HAL 的下載目錄複製（計畫的授權事項）；之後要改成各自的官方網址（`source = ` + SHA-256）。

## 6. P13：函式庫 port 與更多程式（K7–K8，使用者已於 2026-10-22 審核）

### 6.1 建置期依賴

`port.toml` 多了 `build-depends = ["ncurses"]`：建置前先確定那些 port 已經建好（沒有就先建），再把它們（與它們的依賴、`depends`）的 `include/`、`lib/` 合併到 `work/NAME/deps`，以 `$DEPS` 給 `build.sh`（`-I$DEPS/include -L$DEPS/lib`）。`depends` 仍是執行期的依賴（測試時一起裝進 `Data:/Programs/`）。

函式庫 port 的套件裝到 `Data:/Programs/NAME/{lib,include}`（tcc 在 STELA 裡也能用）；動態連結的 `/lib` 會找 `Data:/Programs/lib`（`docs/linux.md` §16.3），不是這些資料夾。

| port | 版本 | 授權 | 內容與測試 |
|---|---|---|---|
| zlib | 1.3.1 | Zlib | `libz.a`、minigzip；測試程式壓縮往返、CRC-32 |
| bzip2 | 1.0.8 | bzip2-1.0.6 | `libbz2.a`、bzip2；壓縮並 `-t` 檢查 |
| xz | 5.6.3 | 0BSD | `liblzma.a`、xz（單執行緒）；壓縮並 `-t` 檢查 |
| libffi | 3.4.6 | MIT | `libffi.a`；`ffi_call` 整數與浮點；closure 需要匿名可執行記憶體（STELA 拒絕），不能用 |
| ncurses | 6.5 | X11 | `libncurses.a`（寬字元、terminfo 在同一個庫）；**沒有 terminfo 資料庫**：dumb、vt100、vt220、linux、xterm、xterm-256color、ansi、screen 編進庫裡（`--with-fallbacks`，用建置機的 tic）；cross 時 configure 猜 bool 是 int，改成告訴它原生的答案（stdbool、1 byte） |
| readline | 8.2 | GPL-3.0-or-later | `libreadline.a`、`libhistory.a`（build-depends ncurses）；測試打一行字 |
| mbedtls | 3.6.2 | Apache-2.0 | `libmbedtls.a`、`libmbedx509.a`、`libmbedcrypto.a`；SHA-256、AES-GCM 往返與竄改 |
| sqlite（P12 的） | 3.54.1 | public domain | 也打包 `libsqlite3.a` 與 `sqlite3.h`（python3 用） |

### 6.2 程式

| port | 版本 | 授權 | 說明 | 測試 |
|---|---|---|---|---|
| python3 | 3.12.7 | PSF-2.0 | 所有擴充模組編進程式（`MODULE_BUILDTYPE=static`）：zlib、bz2、lzma、ctypes、sqlite3、_decimal…；沒有 ssl、curses、readline、dbm、tkinter；標準庫去掉 test、idlelib、tkinter、lib2to3、ensurepip 後 12 MB。**動態連結** musl 的 `libc.so`（`/lib/ld-musl-x86_64.so.1`）：ctypes 需要 `dlopen(NULL)`，靜態程式沒有。cross 需要同版本的 build python：先建一個 host 用的 3.12 | `-c print(6*7)`、壓縮模組、sqlite3、`ctypes.CDLL(None).strlen`、hashlib、json、`-m http.server --help` |
| git | 2.47.0 | GPL-2.0-only | zlib、libcurl + Mbed TLS（HTTPS）；沒有 Perl、Python、Tcl、gettext、expat；C17（`thread_local` 在 C23 是關鍵字）；經 shell 的指令（本機傳輸的 `git-upload-pack`）用 **dash port**（`depends = ["dash"]`，編進 `SHELL_PATH`）；`libexec/git-core/` 有 `git-upload-pack`、`git-remote-http(s)` | init、add、commit、`--no-pager log`、`clone --no-hardlinks` |
| vim | 9.1.0800 | Vim | normal 功能、ncurses、只有終端機、不帶 runtime 檔案；cross 時把 configure 跑不了的答案（`vim_cv_*`）傳進去 | ex 模式（`-u NONE -N -es -S script`）寫檔，cat 讀回 |
| nano | 8.2 | GPL-3.0-or-later | 寬字元 ncurses；musl 的 `<sys/vt.h>` 要 kernel 的 `<linux/vt.h>`，build 裡放一個只有兩個值的替身 | `--version` |
| curl | 8.10.1 | curl | Mbed TLS、zlib；`libcurl.a`（git 用）；CA 是 STELA 的 `third_party/mozilla-ca/cacert.pem`（`share/cacert.pem`） | `--version`（mbedTLS/3.6.2）、`file://` 讀檔 |
| file | 5.45 | BSD-2-Clause | 魔數資料庫要同版本的 file 編：build 裡先建一個 host 用的 | 文字檔、PNG |
| less | 668 | GPL-3.0-or-later | ncurses | `-dFX`（dumb 終端機、放得下就結束） |

- libtool 會把 `-static` 吞掉（對它只是「不用共享的 libtool 庫」），P13 起 musl 有 `libc.so`，所以 xz、curl、file 在 make 時用 `-all-static`。
- terminal 只有 SGR 顏色（`docs/window.md` §10），全螢幕的 vim／nano／less 在 STELA 的 terminal 裡畫不對；測試只用不需要游標定址的方式。
- 下載的 tarball（`~/stela-ports/sources/`）與 SHA-256 在各自的 `port.toml`（計畫的授權事項）。

## 7. P14：Rust 的大程式（K10，自動排程一次做完，使用者已於 2026-10-26 審核）

| port | 版本 | 授權 | 來源 | test.sh |
|---|---|---|---|---|
| ripgrep（`rg`） | 15.2.0 | Unlicense OR MIT | crates.io 的 `ripgrep-15.2.0.crate`（SHA-256 `a30750b6…ec71`） | `--version`、`-n --no-heading --sort path fox`（兩個檔案、三行）、找不到 → exit 1 |
| fd | 10.5.0 | MIT OR Apache-2.0 | crates.io 的 `fd-find-10.5.0.crate`（`f35becce…1e4d`） | `--version`、`-e md readme`、`-e txt notes`（`--color never`） |

- `build.sh`：`cargo build --release --locked --target x86_64-unknown-linux-musl`（相依由 crate 自己的 `Cargo.lock` 鎖定，cargo 從 crates.io 抓——計畫允許），`relocation-model=static`、`crt-static`（STELA 載入靜態、非 PIE 的執行檔），`CARGO_PROFILE_RELEASE_DEBUG=false`、`STRIP=symbols`（沒有的話 `rg` 有 36 MB）。兩個都在 musl 上用 jemalloc（C），`CC_x86_64_unknown_linux_musl` 指到 STELA 的 `musl-gcc`。
- `portbuild.py`：`.crate`（就是 tar.gz）也能解開；P14 起測試時也接 STELA 的 `build/system.img`（snapshot），P13 以前的 port 的 test.sh 有用到 `System:` 的東西（字型、工具）時才不會缺。
- kernel 為了它們補的：`madvise(MADV_DONTNEED)` 把私有映射的頁清成 0（jemalloc 開始時檢查這件事，否則印一行警告）；共享的檔案映射不動（Linux 會重新讀檔）。沒有需求分頁，所以頁框不還（計畫寫「真的釋放」——要先有需求分頁，記在限制）。
- `rg --version` 印出的 `rev` 是 ports repo 的 git 版本（建置在那個 repo 的 `work/` 裡，crate 的 build script 讀到了），無害。
