# TLS：HTTPS（P12）

> P12 由自動排程一次做完，使用者已於 2026-10-22 審核。

STELA 的程式現在可以用 `https://`：`fetch`、Periplus、`pkg`（`docs/pkg.md`）都經 libstela 的 `tls` 模組。TLS 1.3 的協定由 `embedded-tls`（crates.io，Apache-2.0）做，密碼學是 RustCrypto 的 crate；STELA 自己做的是：亂數的來源（kernel 的 `Random`）、socket 的轉接、從 Mozilla 的根憑證清單挑出對的那一張、以及資料怎麼放進 initrd。

程式碼：

```
frame/src/random.rs           ChaCha20 產生器、RDSEED／RDRAND／TSC 抖動（§2）
kernel/src/syscall.rs         Random（64）
user/libstela/src/tls.rs      DER 小工具、Roots、Verifier、request（§3、§4）
user/libstela/src/net.rs      http_get_raw_with：https:// 轉給 tls::request
user/libstela/src/http.rs     get_with、https:// 的連結合併
user/fetch/src/main.rs        --ca 檔案
tools/cacert_to_der.py        PEM → DER（make initrd）
tools/make_test_certs.py      測試用的根與伺服器憑證（openssl）
tools/with_https.py           host 上的 HTTPS 伺服器（Python ssl，只有 TLS 1.3）
third_party/mozilla-ca/       cacert.pem（MPL-2.0，curl 的 Mozilla 清單）
```

## 1. 比喻：信封與印章

HTTP 是**明信片**：路上每個人都看得到內容，也可以偷改。TLS 把信放進**只有收件人打得開的信封**（加密），而且收件人得先出示**身分證**（憑證）：身分證上有名字（網址的主機名）、有效期限，以及**發證單位的印章**（簽章）。發證單位本身也有身分證，蓋著更上層的印章……一路往上到幾個大家事先約定好要相信的**根**（root CA）。STELA 帶著 Mozilla 整理的那一份根的名單（118 張）。

```
 github.com 的憑證 ──蓋章者──► Sectigo 的中繼憑證 ──蓋章者──► USERTrust ECC 根（在名單裡）
   名字對嗎？日期對嗎？              章對嗎？                         章對嗎？
```

另外每次連線要一把新的**臨時鑰匙**（金鑰交換，P-256 ECDH）：它要從真正的亂數產生，不然別人猜得到——這就是 P12 的 `getrandom` 從「時鐘混一混」換成真的產生器的原因。

## 2. 亂數：`frame::random`

```
 熵：RDSEED（CPUID.07H:EBX[18]）→ RDRAND（CPUID.01H:ECX[30]）→ TSC 抖動
     （Intel 的 DRNG 文件 §5.2.1：CF=0 表示還沒有值，重試 10 次）
       │ 32 bytes：第一次用到時、之後每給出 1 MiB
       ▼
 ChaCha20 的金鑰 ──► 一個個 64-byte 區塊（RFC 8439 §2.3）──► 呼叫者
       ▲                         │
       └── 每次請求之後換成下一個區塊的前 32 bytes（fast key erasure：
           就算之後金鑰外洩，也推不回之前給出去的）
```

- TSC 抖動：4096 次取樣，每次在一個長度隨 TSC 變的迴圈之後讀 TSC，差值的低位元混進 ChaCha 的狀態；快取、中斷、時脈的細微變化讓它不可預測。QEMU 的 TCG 預設 CPU 沒有 RDRAND，所以測試走這條路；P520（Xeon W-2135）有 RDSEED。
- 使用者：`Random(buf, len)`（syscall 64，一次最多 64 KiB，`libstela::sys::random`）；Linux 的 `getrandom`、`/dev/urandom`、AT_RANDOM（`docs/linux.md`）。
- host 測試：RFC 8439 §2.3.2 的測試向量、同一個種子給出同樣的輸出、換金鑰後不重複、1 MiB 後要求重新播種。

## 3. 信任的根：`Initrd:/certs/cacert.der`

- `third_party/mozilla-ca/cacert.pem`：curl 從 Mozilla 的根憑證庫轉出的 PEM（2026-10-20 的資料，118 張，SHA-256 與 curl 公布的相同；`third_party/README.md`）。
- `make initrd` 用 `tools/cacert_to_der.py` 把每個 `BEGIN CERTIFICATE` 區塊轉成 DER，一張接一張放進 `certs/cacert.der`（約 150 KiB）。
- `tls::Roots`：讀這個檔（或 `fetch --ca 檔案` 給的別的檔），`split_certificates` 依 DER 的長度切開。

### 3.1 選根（`Verifier`，`tls.rs`）

embedded-tls 的 `CertVerifier` 一次只接受**一張**根。STELA 的 `Verifier` 先挑出那一張：

```
 伺服器送來的鏈：[自己的, 中繼 1, 中繼 2, …]
 從自己的開始往上找：第一張「發證者（issuer）的名字 = 某張根的主體（subject）名字」的
   → 鏈截到這張為止（後面的，例如被別的根交叉簽過的同一張根，丟掉）
   → CertVerifier(那張根)：每一段的簽章、有效期限（牆上時間）、
     主機名（SAN 的 DNS 名稱、萬用字元 *.example.com；沒有 SAN 時看 CN）
 名字以 DER 的位元組直接比對（RFC 5280 的名字比對規則更寬，例如大小寫；實務上一樣）
```

- 名字一樣的根有好幾張（交叉簽署的新舊版本）→ 取清單裡的第一張。
- 截短的方法是改 `CertificateRef.entries`（公開欄位）；**不能重新組一個 `CertificateRef`**：它的原始位元組會不見，之後檢查 CertificateVerify 時拿不到伺服器的憑證（第一版這樣寫，結果 DecodeError，在 host 上用同一個 HTTPS 伺服器重現才找到）。
- 支援的簽章：ECDSA P-256／SHA-256、P-384／SHA-384、RSA PKCS#1 v1.5（SHA-256/384/512）、RSA-PSS（CertificateVerify）。

## 4. 連線：`tls::request`

```
 request(host, ip, port, 請求, roots, timeout)
   Socket::tcp().connect
   TlsConnection（embedded-tls 的 blocking 版）：讀寫各 16.25 KiB 的記錄緩衝
   TlsConfig::new().with_server_name(host)        SNI 與主機名檢查
   CryptoProvider：亂數 = KernelRng（Random）、verifier = Verifier
   open（握手）→ write 請求 → flush → read 直到伺服器關閉
```

- 加密組合：TLS_AES_128_GCM_SHA256；金鑰交換：P-256（embedded-tls 只提供這個群組，常見的伺服器都接受）。
- 伺服器結束連線：`close_notify`，或（Python 等很多伺服器）直接關 TCP——embedded-tls 回 `IoError`；已經收到資料時當成結束（HTTP/1.0 + `Connection: close` 的回應本來就到連線結束為止）。
- 錯誤：憑證或簽章不對 → `AccessDenied`（`fetch` 印 `access denied`）；連線關閉 → `PeerClosed`；逾時 → `TimedOut`；其他 → `IoError`。
- `net::http_get_raw_with(url, timeout, roots)`：`https://` → `tls::request`（`roots` 為 `None` 時讀系統的），`http://` 照舊。`http::get`／`get_with` 跟著轉址，`https://` 的連結合併（`http::join`）也處理了。
- 使用者空間的目標是 soft-float（沒有 SSE），所以 AES、POLYVAL 用 RustCrypto 的可攜實作（`.cargo/config.toml` 的 `--cfg aes_force_soft --cfg polyval_force_soft`），SHA-2 用 `force-soft`。TCG 下一次握手約 1–2 秒。

## 5. 測試

| 測試 | 內容 |
|---|---|
| host `libstela::tls` | DER 長度（短、長、截斷、不定長拒絕）、憑證的 issuer／subject、`Roots` 切開與找候選 |
| host `libstela::http` | `https://` 的相對連結、絕對連結 |
| host `frame::random` | §2 |
| `make test-tls` | `tools/make_test_certs.py` 在 `build/tls/` 用 openssl 造：`good`（10.0.2.2，SAN 有 DNS 與 IP）、`wrong`（wrong.test）、`other`（自簽）；測試根（EC P-256）、伺服器的鑰匙與 `expired`（2020–2021）是 `tools/testcerts/` 的固定夾具（P14 K0：造過期憑證要 `openssl x509 -not_before`，CI 的 OpenSSL 3.0 沒有）；`tools/with_https.py` 在 host 的 8643–8646 起四個只用 TLS 1.3 的伺服器；`ca.der` 經 `ehfs-cp` 放進 Data 映像。shell：`fetch --ca test-ca.der https://10.0.2.2:8643/tls.txt` 拿到 `hello over TLS`；8644（名字不對）、8645（過期）、8646（不是測試根簽的）→ `access denied`；不給 `--ca`（Mozilla 的根）連 8643 → `access denied` |
| 手動（需要網路，不在 `make test`） | QEMU user 網路：`fetch https://example.com/`、`https://www.google.com/`（Google 的鏈尾端是被 GlobalSign 交叉簽的 GTS Root R1，靠 §3.1 的截短）、`https://www.wikipedia.org/`、`https://github.com/` 都 HTTP 200（2026-10-22） |

## 6. 已知限制

- 只有 TLS 1.3（伺服器只有 1.2 的連不上）；只有 P-256 的金鑰交換；只有 AES-128-GCM。
- 沒有憑證撤銷（OCSP、CRL）、沒有 Certificate Transparency；名字比對是位元組比對；同名的根取第一張。
- 沒有 session resumption、沒有 ALPN（只有 HTTP/1.0）。
- 已經收到資料後連線被切斷就當成結束：一個被截斷的回應看不出來（HTTP 層也沒有檢查 Content-Length）。
- 根憑證清單跟著系統（initrd）更新；使用者不能自己加（`fetch --ca` 只對那一次）。
