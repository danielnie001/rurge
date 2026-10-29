# 阶段 4 前期调研：伪造叶证书、`ca-p12`、rustls、正则、jq、正文解码

调研日期 2026-10-29。只读调研：没有安装、下载、运行任何第三方代码，没有跑 cargo，没有改仓库里的文件；唯一写入的是本文件。

本文是阶段 4（HTTP 引擎）设计（`docs/superpowers/specs/2026-10-29-phase4-http-engine-design.md`）的输入，由只读的助手写成，文中的“我”是写报告的助手；收进仓库时只把本机的路径换成了通用的写法，其余照原样。设计怎样取舍见设计第 3 节。


标记约定：**（推断）** = 我自己的判断；**（搜索摘要）** = 只在搜索结果摘要里见过、没有打开原页；**未见** = 找过、没有证据；**本机源码** = 读的是 `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/<crate>-<版本>/` 下的源码。凡写"页面原句"的引号，都是 WebFetch 摘要给出的原文片段（见下一节对它可靠性的说明）。

---

## 来源可靠性

按可靠程度从高到低：

1. **本机源码与本机构建记录（最可靠，逐行读过）**：rcgen 0.14.7、p12-keystore 0.2.1、pkcs5 0.7.1、pkcs12 0.1.0、fancy-regex 0.14.0、regex 1.13.1 / regex-syntax 0.8.11、rustls 0.23.43、tokio-rustls 0.26.4、rustls-native-certs 0.8.4、rustls-webpki 0.103.15、flate2 1.1.10、ring 0.17.14、yasna 0.5.2；rurge 仓库自己的代码（`crates/rurge-proto/src/keystore.rs`、`crates/rurge-proto/src/transport/tls.rs`、`crates/rurge-net/src/tls.rs`、`crates/rurge-config/src/rule.rs`）；以及 `target/debug/.fingerprint/*.json`（用来确认哪些 cargo 特性**真的**打开了，比 `Cargo.lock` 可靠）。
   - 本机版本都落后于 crates.io 上的最新版：rcgen 本机 0.14.7（最新 0.14.10）、p12-keystore 本机 0.2.1（最新 0.4.1）、fancy-regex 本机 0.14.0（最新 0.20.0）。凡标"本机源码"的行为只对这几个版本成立；新版变化我另从各自的 CHANGELOG 读了，单独标出。
2. **Surge 手册的本机快照**（`http_mitm.html.txt`（取法见 `docs/reports/2026-10-29-phase4-manual-recheck.md`）、`http_body-rewrite.html.txt`、`http_url-rewrite.html.txt`、`http_header-rewrite.html.txt`、`http_map-local.html.txt`、`rules_http.html.txt`、`profile_keystore.html.txt`）：这是别的会话取下来的页面文本，我整页读过，与我用 WebFetch 取的摘要一致。手册里**没有任何一处点名正则引擎**（`grep` 过 ICU / NSRegular / PCRE / lookahead / lookbehind 全无）。
3. **WebFetch 取到的原始页面，但经小模型摘要**（引号内是摘要给出的原句，数字与版本号我没有第二来源核对）：mitmproxy 的 `certs.py` / `tlsconfig.py` / `layers/tls.py` / `net/encoding.py` / `http.py` / `CHANGELOG.md` / 文档；hudsucker 的 `rcgen_authority.rs` / `openssl_authority.rs` / `certificate_authority/mod.rs` / `proxy/internal.rs` / `decoder.rs` / `Cargo.toml`；mkcert 的 `cert.go`；Chromium 的 `cert_verify_proc.cc`、`certificate_lifetimes.md`、Chrome Root Store FAQ、`docs/linux/cert_management.md`；Apple 支持页；Android 开发者文档；ICU 用户指南与 `rematch.cpp`；swift-corelibs-foundation 的 `NSRegularExpression.swift`；RFC 8446 §6.2；jaq 手册与 docs.rs 页面；crates.io API 页（各 crate 的版本、日期、许可证、MSRV、下载量、特性表）。
4. **搜索摘要（最弱）**：Firefox 的 Mozilla 支持页（WebFetch 取不到，页面要执行脚本）、Windows Schannel 的各种说法、Apple 论坛与 Sectigo 的说法、Android 14 的 APEX 变化、Squid 邮件列表、各厂商论坛里的 `SEC_ERROR_REUSED_ISSUER_AND_SERIAL` 报告。`developer.apple.com` 对 WebFetch 报"无法验证域名是否安全"，只有 `origin-devforums.apple.com` 的镜像取得到一条。

**已知没有解决的不一致**（读报告的人要知道）：
- mitmproxy：`certs.py` 里 `CERT_EXPIRY = 199 天`（`main` 与 `v12.3.1` 两次取到的一致），而 `CHANGELOG.md` 写 12.2.2（2026-04-12）"Reduce `CERT_EXPIRY` to 199 days"、12.2.3（2026-05-12）"Reduce generated leaf certificate validity from 199 to 197 days"。我没能让这两处对上；（推断）197 是 notAfter = now − 2 天 + 199 天后"从现在起的剩余有效期"，总长仍是 199 天。
- Apple 的"Requirements for trusted certificates"页没有写这些规则是否适用于用户添加的根；"825 天适用于自定义根"这一条来自 mkcert 作者写在源码里的注释，不是 Apple 原文。
- jaq 手册说正则用 `regex-lite`，而 crates.io 上 jaq-std 3.0.4 的依赖是 `regex-bites`（两者同属 rust-lang/regex 仓库的轻量引擎，限制相近）。
- "Surge 用 NSRegularExpression（ICU）"：**没有任何直接证据**（见第 5 节）。仓库里早先的设计文档已经把它当作前提写下了，这是推断不是事实。
- crates.io 上的版本日期（例如 jaq-core 3.1.2 = 2026-10-21）来自 crates.io API 页的 WebFetch 摘要，今天是 2026-10-29，日期与"当前"自洽，但没有第二来源。

---

## 1. 伪造的叶证书：现在的客户端要什么、同类工具怎么做

### 1.1 各客户端对"链到用户安装的根"的叶证书的要求

#### Apple（Safari 与系统 TLS 栈，macOS / iOS）

- **怎样信任用户的根**：macOS 要在钥匙串里装上并设为"始终信任"（Chrome 的 FAQ 说 Chrome 读 Default 与 System 两个钥匙串并遵守 Always Trust / Never Trust）；iOS 装完描述文件还要到"证书信任设置"里手动打开完全信任（mitmproxy 文档页面原句："On recent iOS versions you also need to enable full trust for the mitmproxy root certificate."）。
- **对叶证书的要求**（[Apple：Requirements for trusted certificates in iOS 13 and macOS 10.15](https://support.apple.com/en-us/103769)，页面日期 2023-11-06，原句经摘要）：
  - RSA 密钥 ≥ 2048 位（对 TLS 服务器证书与签发它的 CA 都一样）；
  - 签名哈希必须属于 SHA-2 家族（SHA-1 不再被信任）；
  - 服务器的 DNS 名必须出现在 **Subject Alternative Name** 里，**CommonName 里的 DNS 名不再被信任**；
  - 2019-07-01 之后签发的服务器证书必须带 **EKU 且含 id-kp-serverAuth**；
  - 2019-07-01 之后签发的服务器证书有效期必须 **≤ 825 天**。
  - ECC 曲线要求：该页没写（未见）。
- **这些规则对用户加的根生效吗**：
  - 825 天：生效。mkcert 的源码注释原话："Certificates last for 2 years and 3 months, which is always less than 825 days, the limit that macOS/iOS apply to all certificates, including custom roots."（[mkcert `cert.go`](https://raw.githubusercontent.com/FiloSottile/mkcert/master/cert.go)）；Apple 开发者论坛与 Sectigo 也报告导入的内部根仍会拒绝超过 825 天的叶证书（搜索摘要）。
  - 398 天：**不生效**。[Apple：About upcoming limits on trusted certificates](https://support.apple.com/en-qa/102028)（页面日期 2023-08-21）："This change will not affect certificates issued from user-added or administrator-added Root CAs."（398 天自 2020-09-01 起对预装根签发的证书生效。）
  - Apple 有没有就 CA/B Forum 的 200 / 100 / 47 天新节奏对用户根作过声明：未见。Pi-hole 论坛 2025-04 的贴子称 iPadOS 18.3.2 拒绝了 30 年有效期的私有根叶证书，并称 Apple 后来的公告漏掉了"用户根例外"那句，没有在 Apple 页面上核实（搜索摘要）：[链接](https://discourse.pi-hole.net/t/apple-clients-do-not-trust-tls-certificates-with-long-validity-periods/79235)。

#### Chrome / Chromium

- **信任哪里的根**（[Chrome Root Store FAQ](https://chromium.googlesource.com/chromium/src/+/main/net/data/ssl/chrome_root_store/faq.md)，摘要）：Windows 读 Local Machine 与 Current User 两个存储（信任与不信任条目都读）；macOS 读 Default 与 System 钥匙串并遵守 Always Trust / Never Trust；Linux 走 NSS 共享库——[Chromium：Linux 证书管理](https://chromium.googlesource.com/chromium/src/+/main/docs/linux/cert_management.md)：缺省 `$HOME/.local/share/pki/nssdb`（自 M146；已有 `$HOME/.pki/nssdb` 时仍用旧库），加根用 `certutil -d sql:… -A -t "C,," -n <名> -i <文件>`，该页没提 `/etc/ssl/certs`。Android 上 Chrome 接受用户装的 CA（搜索摘要，无一手来源）。
- **SAN 必须有**：Chrome 58 起不再用 CommonName 匹配；给"链到本地信任锚"的证书保留回退的企业策略 `EnableCommonNameFallbackForLocalAnchors` 后来也移除了（搜索摘要，版本号说法是 66）：[Chromium 的 Intent to Remove](https://groups.google.com/a/chromium.org/g/security-dev/c/IGT2fLJrAeo/m/csf_1Rh1AwAJ)。
- **有效期规则只对"已知根"**：`net/cert/cert_verify_proc.cc`（[源码](https://chromium.googlesource.com/chromium/src/+/main/net/cert/cert_verify_proc.cc)，摘要）：
  `if (verify_result->is_issued_by_known_root && HasTooLongValidity(*cert)) { ...CERT_STATUS_VALIDITY_TOO_LONG... }`；
  门槛按 notBefore 分：≥ 2020-09-01 → 398 天；≥ 2026-03-15 → 200 天；≥ 2029-03-15 → 47 天（代码里 2027-03-15 那档的天数被摘要省略，NVISO 的表给的是 100 天）。文档 [`net/docs/certificate_lifetimes.md`](https://chromium.googlesource.com/chromium/src/+/main/net/docs/certificate_lifetimes.md) 原句：这条规则"will not apply to locally-operated CAs that have been manually configured"。
- **例外：Android**。NVISO 2026-07-22 的文章报告 Android WebView 149 对 Burp 签发的一年期叶证书报 `net_error -213`（`CERT_VALIDITY_TOO_LONG`），Burp 2026.6 起把叶证书改为 **199 天**（[NVISO](https://blog.nviso.eu/2026/07/22/intercepting-android-webview-traffic-under-new-certificate-validity-requirement-by-chromium/)）。文章没有解释为什么这个 CA 被当成"已知根"；（推断）Chromium 的 Android 验证器按链上公钥哈希是否在已知根表里判断 `is_issued_by_known_root`（[`cert_verify_proc_android.cc`](https://chromium.googlesource.com/chromium/src/+/main/net/cert/cert_verify_proc_android.cc) 的赋值处是 `GetNetTrustAnchorHistogramIdForSPKI(sha256) != 0`），而 App 内的 WebView 不信用户区的 CA，试验者多半把 CA 装进了系统区——这是我的推断，文章没说。
- **内置公钥固定**：`TransportSecurityState::CheckPublicKeyPins(host, is_issued_by_known_root, public_key_hashes)` 把"是否已知根"传进去，另有 `SetEnablePublicKeyPinningBypassForLocalTrustAnchors`（[源码](https://chromium.googlesource.com/chromium/src/+/main/net/http/transport_security_state.cc)）；（推断）链到本地根时 Chrome 绕过内置固定。
- **拒绝时发的 TLS 警报**：`SSLClientSocketImpl::VerifyCertCallback` 里没有设 `*out_alert`（[源码](https://chromium.googlesource.com/chromium/src/+/main/net/socket/ssl_client_socket_impl.cc)），具体警报由 BoringSSL 决定，源里看不到（见 4.3）。

#### Firefox

- 自带证书库（NSS）。用户根进 Firefox 有三条路：
  1. `security.enterprise_roots.enabled`：策略 `ImportEnterpriseRoots` 的说明："Trust certificates that have been added to the operating system certificate store by a user or administrator."，"only works on Windows and macOS"（Firefox 60 起，macOS 自 63；[Firefox 管理文档](https://firefox-admin-docs.mozilla.org/reference/policies/certificates/)）。Firefox 120 的发布说明称在 Windows / macOS / Android 缺省打开，作者 2024-01-09 的更正说实际只导入"用户添加"的根、不导入系统缺省的那批（[joshua.hu](https://joshua.hu/mozilla-firefox-trusting-system-root-stores-qwacs-eu)）。TLS 出错时 Firefox 会自己打开该偏好并重试一次（Mozilla 支持页，搜索摘要）。
  2. 策略 `Certificates → Install`：证书以 trust 串 `CT,CT,` 装进 Firefox 自己的库，DER 或 PEM 皆可（同上管理文档）。
  3. Linux 不读系统库：管理文档把 `ImportEnterpriseRoots` 在 Linux 上的缺失指向 bug 1600509；可行路是导入 profile 的 `cert9.db`、上面第 2 条，或 `p11-kit-trust.so`（搜索摘要）。
- 不对私有根执行 398 / 825 天检查（Sectigo 的说明，搜索摘要）。
- **同一签发者 + 同一序列号但内容不同**会报 `SEC_ERROR_REUSED_ISSUER_AND_SERIAL`：Sophos、Palo Alto 等中间人产品的论坛里都有报告（搜索摘要，[Sophos 贴子](https://community.sophos.com/intercept-x-endpoint/early-access-program/f/feedback-and-issues/132859/sec_error_reused_issuer_and_serial-error-when-using-decrypt-https-websites-using-ssl-tls-in-eap-using-firefox)）。这对 rcgen 的缺省序列号有直接影响，见 2.5。

#### Windows（Schannel：.NET、WinHTTP、curl-schannel 等；Chrome / Edge 读同一批存储）

- 根要进 Local Machine 或 Current User 的"受信任的根证书颁发机构"（mitmproxy 文档给的自动命令：`certutil -addstore root mitmproxy-ca-cert.cer`）。
- **叶证书不要与 CA 共用 Subject Key Identifier**：mitmproxy 10.1.6（2023-12-14）更新说明："Fix compatibility with Windows Schannel clients, which previously got confused by CA and leaf certificate sharing the same [key identifier]"，PR #6549 的原话是 "if CA and leaf share the same Subject Key Identifier, SChannel gets confused"，修法是**叶证书不写 SKI**（[issue #6494 / PR #6549](https://github.com/mitmproxy/mitmproxy/issues/6549)）。
- **吊销检查**：做严格吊销检查的 Schannel 客户端（例如 curl 的 Schannel 后端）遇到链上没有吊销信息会失败，错误是 `CRYPT_E_REVOCATION_OFFLINE` / `CERT_TRUST_REVOCATION_STATUS_UNKNOWN`，curl 的绕法是 `--ssl-revoke-best-effort`（Bright Data 文档，搜索摘要：[链接](https://docs.brightdata.com/general/account/ssl-certificate-windows-schannel)）。mitmproxy 12.0.0（2025-04-29）有一条 "Add CRL entries to dummy cert when the upstream certificate has some."，机制与动机更新说明没写（未见）。
- **Ed25519 叶证书**：只在 Rebex 论坛的一句话里见到"Schannel 不支持"（搜索摘要，无日期，对方是竞品）；微软官方说法未见。ECDSA P-256 / P-384：strongSwan 的页面说 Windows 的 IKEv2 客户端接受这两条曲线、不接受 P-521 与 EdDSA（VPN 场景，不是 TLS；搜索摘要）。
- `rustls-native-certs` 0.8.4 在 Windows 上只读 **Current User 的 `ROOT`** 存储，并过滤掉已过期、EKU 不含 serverAuth 的根（本机源码）。Windows 的根存储是按需惰性填充的（Let's Encrypt 社区的帖子：直接读存储而不走系统 API 就读不到未被触发的根；搜索摘要），所以这一读法在 Windows 上可能漏掉一部分公共根——仓库现在的 `rurge_net::tls::root_store()` 只在读到 0 个根时才补 `webpki-roots`，见 4.2。

#### Android

- 官方文档（[Network security configuration](https://developer.android.com/privacy-and-security/security-config)）原句："By default, secure connections (using protocols like TLS and HTTPS) from all apps trust the pre-installed system CAs, and apps targeting Android 6.0 (API level 23) and lower also trust the user-added CA store by default."——也就是**目标 API ≥ 24 的 App 缺省只信系统库**，要信用户装的 CA 必须在 network security config 里写 `<certificates src="user" />`；`debug-overrides` 只在 `android:debuggable="true"` 时生效。
- 同一页：对 App 自带证书或用户库，证书透明度校验缺省关闭（"By default, the verification is disabled for these cases."）。
- Android 14 起系统根移到 Conscrypt 的 APEX（`/apex/com.android.conscrypt/cacerts`），把用户 CA 注入系统库的做法随之变化（NCC Group，搜索摘要）。
- 钉住：`pin-set` 可带 `expiration`；到期后不再固定（同一页）。

### 1.2 mitmproxy、hudsucker 及其它工具给叶证书与根 CA 的参数

| | mitmproxy（`main` / v12.3.1，MIT） | hudsucker 0.25.0（MIT OR Apache-2.0，2026-07-15） | mkcert（BSD-3，作参照） | Burp（NVISO 2026-07） |
|---|---|---|---|---|
| 叶有效期 | `CERT_EXPIRY = 199 天`；notBefore = now − 2 天（`CERT_VALIDITY_OFFSET`），notAfter = now − 2 天 + 199 天。历史：4.0.4（2018-07）两年；5.1（2020-04）一年；12.2.2 起 199 天 | `TTL_SECS = 365 天`；notBefore = now − 60 秒（`NOT_BEFORE_OFFSET`） | 2 年 3 个月（保证 < 825 天）；notBefore = now（不回拨） | 原来一年，2026.6 起 199 天 |
| 叶密钥 | **复用 CA 的密钥对**（`builder.public_key(cacert.public_key())`，`CertStoreEntry` 的私钥就是 `default_privatekey`） | **复用 CA 的密钥**（`.signed_by(self.issuer.key(), &self.issuer)`；OpenSSL 版是 `set_pubkey(&self.pkey)`） | RSA 2048（`-ecdsa` 时 ECDSA P-256） | 未见 |
| 序列号 | `x509.random_serial_number()` | rcgen 版 `rng().random::<u64>()`；OpenSSL 版 16 字节随机 | 随机，< 2^128 | 未见 |
| 主体 | CN 仅当 < 64 字符才写；可带 O（取自上游证书） | CN = 主机名 | — | — |
| SAN | 上游证书的 CN + SAN + 客户端 SNI（**无 SNI 时用客户端连的本地 IP `sockname[0]`**）+ 服务器地址；无 CN 时 SAN 标 critical；IP 与 DNS 名分别处理 | **只有一个 `DnsName(host)`，没有 `IpAddress` 分支**（IP 主机会被塞进 DNS SAN） | 按名字类型分 DNS / IP / URI / 邮箱 | — |
| EKU | serverAuth（非 critical） | serverAuth | serverAuth（按需加 clientAuth / emailProtection） | — |
| KeyUsage | **不写** | `digitalSignature`（rcgen 写成 critical） | keyEncipherment + digitalSignature | — |
| BasicConstraints | **不写** | 不写 | — | — |
| AKI / SKI | AKI = 取自 CA 的 SKI（CA 没 SKI 时用 CA 公钥算）；**叶不写 SKI**（注释："that SKI is optional for the leaf cert, so we skip that."；原因即上面的 Schannel 问题） | rcgen 版 `use_authority_key_identifier_extension = true`；叶不写 SKI（rcgen 对 `IsCa::NoCa` 不写） | — | — |
| 吊销信息 | 12.0.0 起：上游证书带 CRL 条目时在 dummy cert 里加 CRL 条目 | 无 | 无 | — |
| 签名 | SHA-256 | 跟随 CA 密钥（rcgen） | — | — |
| 根 CA | `create_ca`：RSA（选项 `key_size`，缺省 **2048**）、有效期 `10 * 365` 天、notBefore = now − 2 天；BasicConstraints `ca=True`、**pathLen 不限**（critical）；KeyUsage `keyCertSign` + `cRLSign`（critical）；**EKU serverAuth（非 critical，少见）**；带 SKI；**无 NameConstraints**；SHA-256 | 由调用方给：示例用 `KeyPair::from_pem` + `Issuer::from_ca_cert_pem` 载入现成的 CA（rcgen 0.14 + `x509-parser` 特性） | RSA 3072；10 年；KeyUsage 仅 `certSign`；`MaxPathLenZero`（**pathLen = 0**）；带 SKI（SHA-1）；无 NameConstraints | — |

出处：mitmproxy 的 [`certs.py`](https://github.com/mitmproxy/mitmproxy/blob/main/mitmproxy/certs.py)、[`tlsconfig.py`](https://github.com/mitmproxy/mitmproxy/blob/main/mitmproxy/addons/tlsconfig.py)、[CHANGELOG](https://github.com/mitmproxy/mitmproxy/blob/main/CHANGELOG.md)、[选项文档](https://docs.mitmproxy.org/stable/concepts/options/)；hudsucker 的 [`rcgen_authority.rs`](https://github.com/omjadas/hudsucker/blob/main/src/certificate_authority/rcgen_authority.rs)、[`certificate_authority/mod.rs`](https://github.com/omjadas/hudsucker/blob/main/src/certificate_authority/mod.rs)、[`openssl_authority.rs`](https://github.com/omjadas/hudsucker/blob/main/src/certificate_authority/openssl_authority.rs)、[crates.io](https://crates.io/crates/hudsucker)；mkcert 的 [`cert.go`](https://raw.githubusercontent.com/FiloSottile/mkcert/master/cert.go)。

**Surge 自己**：手册（本机快照 `http_mitm.html.txt`）只说"The key of the new certificate is generated randomly using OpenSSL."——CA 的密钥类型、位数、有效期、名称、叶证书的任何参数都**没写**（未见）；`ca-passphrase` 不能为空（"due to system limitations"）；`h2` 缺省 `false`。Stash 的文档（[stash.wiki](https://stash.wiki/en/http-engine/mitm)）只提 `ca` / `ca-passphrase` 与 `mitm` 主机名列表；Loon、Quantumult X 的公开资料我没找到（未见）。

### 1.3 叶证书怎么缓存

- **mitmproxy**：进程内 `dict`，键是 `(commonname, sans)` 元组，`STORE_CAP = 100`；超过 100 条按**插入顺序**淘汰（`expire_queue`；命中不刷新，所以不是 LRU）。"通配"只出现在**查找用户通过 `--certs` 给的证书**时（`asterisk_forms` 把 `www.example.com` 展开成 `*.example.com` 等去查，`*` 单独那一形式被省略）；**生成**的叶证书没有通配叶。
- **hudsucker**：`moka::future::Cache<Authority, Arc<ServerConfig>>`，容量是构造参数 `cache_size: u64`，`time_to_live` = `TTL_SECS / 2`（约 182.5 天）；键是 `Authority`（**主机 + 端口**），缓存的不是证书而是整个 `ServerConfig`。
- **Surge**：未见。
- 两者都**不生成通配叶证书**，也没有按 eTLD+1 合并（（推断）原因是通配叶对 `a.b.example.com` 这类多级名无效，且会让客户端合并 HTTP/2 连接，见 4.1）。

### 1.4 小结（事实 + 推断，供设计取舍）

- 所有客户端共同的底线：**SAN 里有 DNS 名（IP 主机要 IP SAN）、EKU 含 serverAuth、SHA-2 签名、RSA ≥ 2048 位或 ECDSA**。（hudsucker 对 IP 主机只放 DNS SAN，Chrome 对 IP 地址的 URL 要求 IP SAN，所以那种写法不可取——（推断）。）
- **叶证书总有效期（notAfter − notBefore）**：Apple 对私有根也卡 825 天；Chromium 的 398 / 200 天只管"已知根"，但 Android 上系统区装的 CA 可能被当成已知根（NVISO）；mitmproxy 与 Burp 在 2026 年都落在 **199 天**。（推断）取 ≤ 199 天（含回拨）可以同时满足以上全部；2027-03-15 起 Chromium 的上限降到 100 天，2029-03-15 起 47 天，缓存 TTL 与续期策略要为此留余地。
- **回拨 notBefore**：mitmproxy 回拨 2 天，hudsucker 60 秒，mkcert 不回拨。时钟偏差大的设备（手机）会因"尚未生效"拒绝；mitmproxy 的注释说回拨就是为了"clients with incorrect clocks"。
- **叶密钥**：mitmproxy 与 hudsucker 都复用 CA 的密钥对（省掉每主机生成密钥的开销，RSA 密钥生成慢）。复用有两个已知代价：序列号必须各不相同（Firefox 的 `SEC_ERROR_REUSED_ISSUER_AND_SERIAL`）；叶证书不能与 CA 共享 SKI（Windows Schannel）。再加一个 rcgen 特有的坑：rcgen 缺省的序列号由公钥哈希决定，复用密钥时所有叶序列号相同（见 2.5），必须显式设置。用 ring 生成 ECDSA P-256 很便宜，（推断）每个叶独立生成就不必踩这些坑。
- **根 CA 的约束**：mkcert 用 pathLen = 0，mitmproxy 不限，两者都没有 NameConstraints。rcgen 能写 NameConstraints（critical），但 Surge 的 CA 来自用户自己的 `ca-p12`，rurge 不能替用户改 CA；只有 `rurge mitm ca generate` 自己生成的 CA 才谈得上加 pathLen 与名字约束。
- **吊销信息**：Schannel 类严格客户端缺吊销信息会报错；rcgen 能写 CRL 分发点（`crl_distribution_points`）并签发 CRL（`CertificateRevocationListParams::signed_by`），但不能写 AIA / OCSP（只能用 `custom_extensions` 手拼）。要不要做、做到哪一步是设计决定。

---

## 2. rcgen 0.14：生成 CA、签发叶证书、载入现成 CA

全部来自本机源码 `rcgen-0.14.7`（`MIT OR Apache-2.0`，`rust-version = "1.71"`）；最新版是 0.14.10（2026-08-28，MSRV 1.88，[crates.io](https://crates.io/crates/rcgen)），我没读 0.14.8 ～ 0.14.10 的差异。

### 2.1 本仓库实际编译的特性

先说现状：rcgen 目前在仓库里**只是测试用的依赖**——`rurge-dns`、`rurge-net`、`rurge-proto` 的 `Cargo.toml` 里它要么是 `optional = true`（挂在 `testing` 特性下）、要么是 `[dev-dependencies]`；阶段 4 把它变成生产依赖。`p12-keystore` 则已经是 `rurge-proto` 的普通依赖（`[Keystore]` 的 `p12` 条目用它）。

`target/debug/.fingerprint/rcgen-*/lib-rcgen.json` 记录的启用特性是 `crypto, default, pem, ring`，**没有 `x509-parser`**。`Cargo.lock` 里 rcgen 的依赖表出现了 `x509-parser`，但那不等于编译进来了（原因我没有查；以指纹为准）。后果：`Issuer::from_ca_cert_der` / `from_ca_cert_pem` 在当前构建里**不存在**，要用就得在工作区的 `rcgen` 依赖上加 `features = ["x509-parser"]`；`x509-parser` 0.18.1 已经在依赖树里（p12-keystore 带来的），该特性额外打开它的 `verify`（用 ring），不增加新 crate（（推断）依据 rcgen 的 `Cargo.toml`：`ring = ["crypto","dep:ring","x509-parser?/verify"]`）。hudsucker 0.25.0 也是这样依赖的：`rcgen = { version = "0.14.0", features = ["x509-parser"], optional = true }`。

### 2.2 API 一览（0.14）

- `CertificateParams`（`#[non_exhaustive]`：不能用结构体字面量，只能 `CertificateParams::default()` 或 `CertificateParams::new(sans)` 之后改公开字段）。字段：`not_before` / `not_after`（`time::OffsetDateTime`；**缺省 1975-01-01 ～ 4096-01-01，必须自己改**）、`serial_number: Option<SerialNumber>`、`subject_alt_names: Vec<SanType>`、`distinguished_name`（**缺省 CN = "rcgen self signed cert"**）、`is_ca: IsCa`（`NoCa` / `ExplicitNoCa` / `Ca(BasicConstraints::Unconstrained | Constrained(u8))`）、`key_usages`、`extended_key_usages`、`name_constraints`、`crl_distribution_points`、`custom_extensions`、`use_authority_key_identifier_extension`（缺省 false）、`key_identifier_method`（缺省 `KeyIdMethod::Sha256`，即 RFC 7093 方法 1 取 SHA-256 的前 160 位）。
- `CertificateParams::new(vec!["host"…])`：每个字符串先试 `IpAddr::from_str`，成功就是 `SanType::IpAddress`，否则是 `SanType::DnsName`（`Ia5String`，非 ASCII 会报错，国际化域名要先转 punycode）。
- `KeyPair::generate()` = ECDSA P-256；`KeyPair::generate_for(&alg)`；`serialize_der()`（PKCS#8）/ `serialize_pem()`；实现了 `SigningKey` 与 `PublicKeyData`。
- `Issuer<'a, S: SigningKey>`：`Issuer::new(params, key)`（拿走所有权）、`Issuer::from_params(&params, key)`（借用）、`Issuer::from_ca_cert_der(&CertificateDer, key)` / `from_ca_cert_pem(&str, key)`（需 `x509-parser`）。`CertifiedIssuer::self_signed(params, key)` 一步给出证书与 `Issuer`（可 `Deref` 成 `Issuer`，有 `.pem()` / `.der()`）。
- 签发：`params.self_signed(&key) -> Result<Certificate, Error>`；`params.signed_by(&subject_public_key, &issuer) -> Result<Certificate, Error>`。文档原句："no validation of the `issuer` certificate is performed. Rcgen will not require the certificate to be a CA certificate, or have key usage extensions that allow signing."——它不检查 CA 标志与 keyCertSign。
- 输出：`Certificate::der() -> &CertificateDer<'static>`、`Certificate::pem()`。仓库里 `crates/rurge-proto/src/testing/tls.rs` 已经用这套（`Issuer::new` + `signed_by` + `serialize_der`）给测试造证书。
- 也有：CSR（`serialize_request`）、CRL（`CertificateRevocationListParams::signed_by(&issuer)`）、CRL 分发点（`CrlDistributionPoint`）、名字约束（`NameConstraints { permitted_subtrees, excluded_subtrees }`，写成 critical）、`CidrSubnet`、自定义扩展（`CustomExtension`）。没有 AIA / OCSP 的专门类型。

### 2.3 叶证书里扩展怎么写出来（`certificate.rs` 的 `write_extensions`）

- 顺序：AKI（`use_authority_key_identifier_extension` 为真时）→ SAN → KeyUsage → EKU → NameConstraints → CRL DP → CA 才有的 SKI 与 BasicConstraints → 自定义扩展。
- KeyUsage 写成 **critical**；EKU **非 critical**；SAN 仅当 subject 为空时才 critical；BasicConstraints 与 NameConstraints 都 critical；AKI 非 critical。
- **SKI 只给 `IsCa::Ca` 与 `IsCa::ExplicitNoCa` 写**；`IsCa::NoCa`（缺省）的叶证书**没有 SKI**——恰好是 mitmproxy 为 Schannel 做的选择。`IsCa::ExplicitNoCa` 会写 `BasicConstraints CA:FALSE`（critical）**并**写 SKI。
- AKI 的值：issuer 的 `key_identifier_method` 若是 `PreSpecified(bytes)` 就用这串字节，否则对 issuer 的 SPKI 重新算（取前 20 字节）。`Issuer::from_ca_cert_der` 会把 CA 证书里**现成的 SKI** 读成 `PreSpecified`，所以 AKI 与 CA 的 SKI 一致（mitmproxy 12.2.3 修的正是"自定义 CA 的 SKI 不是公钥 SHA-1 时报 authority and subject key identifier mismatch"）；CA 没有 SKI 时退回 `Sha256` 方法。
- 一个小坑：只设了 `key_usages` 而没有 SAN / EKU / CA / AKI / 名字约束 / 自定义扩展时，整块扩展都不写（`should_write_exts` 不看 `key_usages`）；叶证书一定会有 SAN，不受影响。
- 时间编码：1950 ～ 2049 年用 UTCTime，其余 GeneralizedTime，纳秒会被抹掉（符合 RFC 5280）。

### 2.4 支持的密钥算法（ring 后端）

| 签名算法常量 | 生成密钥 | 载入已有私钥 | 说明 |
|---|---|---|---|
| `PKCS_ECDSA_P256_SHA256` | 能（`KeyPair::generate()` 缺省） | 能（PKCS#8） | |
| `PKCS_ECDSA_P384_SHA384` | 能 | 能（PKCS#8） | |
| `PKCS_ED25519` | 能 | 能 | 客户端兼容性见 1.1（Windows 未见确认） |
| `PKCS_RSA_SHA256` / `SHA384` / `SHA512` | **不能**（`Error::KeyGenerationUnavailable`；注释："Ring doesn't have RSA key generation yet"） | 能（PKCS#8；`TryFrom` 自动识别 RSA 时选 `PKCS_RSA_SHA256`） | ring 的限制：RSA 模数 **≥ 2047 位且 ≤ 4096 位**，公钥指数 ≥ 65537 且不超过 33 位，PKCS#8 只认 v1（`ring-0.17.14/src/rsa/keypair.rs`、`rsa.rs`） |
| `PKCS_ECDSA_P521_*`、RSA 生成（2048 / 3072 / 4096）、ML-DSA | 仅 `aws_lc_rs` 后端 | | 工作区刻意不用 aws-lc-rs（Windows 上要 cmake 与 NASM，见根 `Cargo.toml` 的注释） |
| `PKCS_RSA_PSS_SHA256` | — | — | `pub(crate)`，不导出；源码注释说"not really sure whether the certs we generate actually work. Both openssl and webpki reject them" |

- ring-only 时 `KeyPair::from_pem` / `TryFrom<&[u8]>` / `from_der_and_sign_algo` **只认 PKCS#8**（PEM 里是 `PRIVATE KEY`）；PKCS#1（`RSA PRIVATE KEY`）与 SEC1（`EC PRIVATE KEY`）只有 `aws_lc_rs` 才认。从 p12 取出的私钥本来就是 PKCS#8，不受影响。
- ring 的 `EcdsaKeyPair::from_pkcs8` 要求 PKCS#8 里**带公钥**（`ECPrivateKey` 结构里含 public key，会校验与私钥一致），曲线必须是命名曲线（本机源码 `ring/src/ec/suite_b/ecdsa/signing.rs`）。
- 结论（事实）：用当前依赖，rurge 自己**能生成**的 CA / 叶密钥是 ECDSA P-256 / P-384 与 Ed25519；**能载入并用来签名**的 CA 密钥还包括 2048 ～ 4096 位的 RSA。生成 RSA 密钥需要另一个库（或换 aws-lc-rs），未评估。

### 2.5 序列号

- `serial_number: None` 时 rcgen 用 **`SHA-256(叶证书公钥原始字节)` 的前 20 字节、最高位清零**当序列号（`certificate.rs` 第 413 ～ 428 行）。也就是**同一把公钥签出的所有证书序列号相同**。叶用独立密钥时没问题；若像 mitmproxy / hudsucker 那样复用 CA 密钥当叶密钥，所有叶证书会有相同的 `(issuer, serial)`，正是 Firefox 的 `SEC_ERROR_REUSED_ISSUER_AND_SERIAL` 触发条件。复用密钥时必须显式设置 `serial_number`。
- `SerialNumber` 可由 `u64`（8 字节大端）、`Vec<u8>`、`from_slice(&[u8])` 构造；写出时按**正整数**编码（yasna 的 `write_bigint_bytes(.., true)`，高位为 1 时前面补 0x00）。RFC 5280 限制序列号 ≤ 20 字节：显式给 20 字节且高位为 1 的值会被编码成 21 字节（rcgen 自己的缺省序列号因此清了最高位；0.12.1 的 CHANGELOG 修的是缺省路径上的同一个问题）。hudsucker 用 `u64`，mkcert 用 < 2^128 的随机数；（推断）16 字节随机且清最高位最稳。

### 2.6 载入现成的 CA（证书 + 私钥）

- 私钥：`KeyPair::try_from(&PrivatePkcs8KeyDer)`（或 `KeyPair::from_pem(&str)`、`KeyPair::from_pkcs8_der_and_sign_algo(&pkcs8, &PKCS_RSA_SHA256 | &PKCS_ECDSA_P256_SHA256 | …)`）。
- 证书 + 私钥 → 签发者：`Issuer::from_ca_cert_der(&CertificateDer, key_pair)` / `Issuer::from_ca_cert_pem(&str, key_pair)`（需 `x509-parser` 特性）。它读出 CA 的 subject（重建成 `DistinguishedName`）、`KeyUsage`、SKI（→ `PreSpecified`），文档说"assumes the provided certificate is a CA. It will not check for the presence of the BasicConstraints extension, or perform any other validation."
- 没有 `x509-parser` 特性时的替代：手工构造 `CertificateParams`（DN、`key_identifier_method = KeyIdMethod::PreSpecified(ski)`）再 `Issuer::new(params, key_pair)`——**签发者名字要自己对上 CA 证书的 subject**，容易错。
- **DN 重建会"折叠"重复属性**（源码核实）：`DistinguishedName::from_name` 逐个 RDN 取属性，遇到一个 RDN 里有多个属性直接报 `CouldNotParseCertificate`；而 `DistinguishedName::push` 以属性类型为键，**同一类型出现两次（例如两个 `OU`）时后一个覆盖前一个**。结果是叶证书的 issuer 字段与 CA 证书的 subject 不再逐字节相等，链构建会失败。Surge 自己生成的 CA 的 subject 长什么样未见；用户用 OpenSSL 手工做的 CA 有可能带多个同类型属性。字符串类型（`BmpString` / `Ia5String` / `PrintableString` / `TeletexString` / `UniversalString` / `Utf8String`）会按原类型保留，其它类型（如 `NumericString`、`VisibleString`）会报错。
- 签发者密钥是 RSA 时，叶证书用 `sha256WithRSAEncryption`（`PKCS_RSA_SHA256`）签；是 ECDSA 时跟随曲线（P-256 → SHA-256，P-384 → SHA-384）。叶证书自己的公钥算法与签发算法无关，可以是 P-256。

### 2.7 与 rustls 的衔接

`rustls::crypto::ring::sign::any_supported_type(&PrivateKeyDer)`（本机源码 `rustls-0.23.43/src/crypto/ring/sign.rs`；另有 `any_ecdsa_type`、`any_eddsa_type`）→ `rustls::sign::CertifiedKey::new(vec![叶 DER, (CA DER)], key)`；仓库测试里的 `ImpostorResolver` 就是这样用的（`CertifiedKey::new` 不检查密钥与证书是否匹配，`with_single_cert` 才检查）。

---

## 3. `ca-p12`：PKCS#12 的加密算法与 p12-keystore 的读写能力

### 3.1 Surge 手册说了什么（本机快照 `http_mitm.html.txt`、`profile_keystore.html.txt`）

- `ca-p12`："The CA certificate and private key in PKCS#12 format, base64-encoded."；`ca-passphrase`："It cannot be empty due to system limitations."；`ca-keystore-name`：指向 `[Keystore]` 里 `type=p12` 的条目，"When set, it takes precedence over `ca-p12`"。`[Keystore]` 条目写法 `cert1 = type=p12, base64=<…>, password=123456`。
- 手册**没有写 PBE / MAC 算法**，也没写 Surge 自己导出的 p12 用什么（未见）。手册只说空口令不行是因为 "system limitations"；（推断）这指的是 Apple 的 PKCS#12 导入接口，Surge 在 Apple 平台上多半用它读 `ca-p12`，所以用户能放进去的文件受 Apple 对算法的支持所限（见下）。

### 3.2 各来源的 p12 实际用什么算法

| 来源 | 私钥包 | 证书包 | MAC | 出处 |
|---|---|---|---|---|
| macOS 钥匙串导出（macOS 14.6.1） | `pbeWithSHA1And3-KeyTripleDES-CBC`，迭代 2048 | `pbeWithSHA1And40BitRC2-CBC`，迭代 2048 | `sha1`，迭代 1 | [Mumble 议题 #6653](https://github.com/mumble-voip/mumble/issues/6653)（`openssl pkcs12 -info` 输出） |
| OpenSSL 3.x 缺省 | PBES2：AES-256-CBC + PBKDF2 | 同左 | `-macalg` 缺省 SHA256 | [OpenSSL 3.0 `pkcs12` 手册](https://docs.openssl.org/3.0/man1/openssl-pkcs12/)：“The default encryption algorithm is AES-256-CBC with PBKDF2 for key derivation.”“If not included SHA256 will be used.” 仓库测试夹具 `OPENSSL_MODERN` 的字节里 PBKDF2 的 PRF OID 是 1.2.840.113549.2.9（hmacWithSHA256） |
| OpenSSL 3.x `-legacy`、OpenSSL 1.1.x 缺省 | 3DES | RC2-40 或 3DES | SHA1 | 同上手册：“In the legacy mode, the default algorithm for certificate encryption is RC2_CBC or 3DES_CBC … for private key encryption is 3DES_CBC.” 仓库夹具 `OPENSSL_LEGACY`：RC2-40 证书包 + 3DES 密钥包 + SHA1 MAC |
| Java keytool / JVM（JDK 11+） | PBES2 + AES-256（我的常识） | 同左 | HMAC-SHA256（我的常识） | p12-keystore 的 `writer()` 注释："default values are taken from JVM java.security config file"；缺省值 PBES2/AES-256 + HMAC-SHA256、迭代 10000 |
| Surge 自己导出 | 未见 | 未见 | 未见 | — |

- **OpenSSL 3 的 legacy provider**：RC2、RC4、单 DES、Blowfish、CAST、IDEA、SEED、MD2、MD4、MDC2、RIPEMD160、Whirlpool、PBKDF1 都在 legacy provider 里（[OSSL_PROVIDER-legacy](https://docs.openssl.org/3.0/man7/OSSL_PROVIDER-legacy/)："We can consider this the retirement home of cryptographic algorithms."）。所以 OpenSSL 3 读带 RC2-40 的老 p12（例如钥匙串导出的）要加 `-legacy`；Mumble 议题里说把文件用 AES-256-CBC 重新加密后就不必 `-legacy`。3DES 仍属缺省 provider 是我的常识，手册摘录里没出现。
- **macOS 导入端**：Apple DTS 在 [开发者论坛 779466](https://developer.apple.com/forums/thread/779466)（2025-04，经 `origin-devforums.apple.com` 取得）说："macOS 15 introduced support for a bunch of new algorithms that should allow you to import modern OpenSSL PKCS#12 files."；"If you need to support systems prior to macOS 15 then you'll need to find some other way to import your digital identity."；`security` 命令行"tends to use CDSA code paths, which means it misses out on a lot of the new stuff."。另一条贴子报告 macOS 14.8 钥匙串导入 OpenSSL 3.4.0 生成的 p12 失败（搜索摘要：[论坛 810723](https://developer.apple.com/forums/thread/810723)）。（推断）要让 macOS 15 之前的 Surge 能读 rurge 生成的 `ca-p12`，写出时要选 3DES + SHA1 一族。

### 3.3 p12-keystore 0.2.1（工作区锁定版）能读什么、写什么（本机源码）

**读**

| 项 | 支持 | 不支持（返回 `UnsupportedEncryptionScheme` 等） |
|---|---|---|
| 外层 | PFX v3；MAC 缺失也放行 | 外层 `authSafe` 不是 `data` 类型（公钥保护模式）→ `UnsupportedContentType` |
| MAC | HMAC-**SHA1**、HMAC-**SHA256**（PKCS#12 KDF） | 其余摘要（SHA-224 / 384 / 512 …），例如 `openssl pkcs12 -macalg sha512` |
| 内层容器 | `data`（私钥包）、`encryptedData`（证书包等） | — |
| PBES1（PKCS#12 PBE） | `pbeWithSHAAnd3-KeyTripleDES-CBC`（…12.1.3）、`pbeWithSHAAnd40BitRC2-CBC`（…12.1.6）（特性 `pbes1`，缺省开） | RC2-128（12.1.5）、2-key 3DES（12.1.4）、RC4（12.1.1 / 12.1.2）、MD5 一族（README 明说） |
| PBES2 | 加密：AES-128 / 192 / 256-CBC；KDF：PBKDF2（PRF = HMAC-SHA224 / 256 / 384 / 512）与 scrypt | **PRF = HMAC-SHA1**、**3DES-CBC 作 PBES2 的加密**（见 3.4） |
| 包类型 | `pkcs8ShroudedKeyBag`、`certBag`（x509）、`secretBag` | 明文 `keyBag` 被忽略；`certBag` 里不是 x509（如 sdsi）→ **整个解析报错**；**私钥包没有 `localKeyID` 属性时被悄悄丢掉**（`parse_bags` 里 `if let Some(local_key_id)`），随后 `private_key_chain()` 返回 `None`，rurge 现有代码报 "holds no private key with a certificate" |
| 配对 | 按 `localKeyID` 把私钥配到证书；再按 subject / issuer **字符串**相等往上找签发者，遇到自签名停 | 私钥与证书的 `localKeyID` 对不上就取不出 |
| 口令 | PKCS#12 KDF 用 BMPString（UTF-16BE）+ 两字节结尾的 NUL（`pkcs12-0.1.0/src/kdf.rs`），空串得到 `[0,0]`（（推断）与 OpenSSL 对"空串口令"的处理一致，但与"无口令（NULL）"不同）；PBES2 用 UTF-8 字节 | （推断，未验证）口令里有超出基本多文种平面的字符时 `BmpString::from_utf8` 会拒绝 |

**写**（`KeyStore::writer(password)`）

- `EncryptionAlgorithm`：`PbeWithHmacSha256AndAes256`（PBES2 / PBKDF2-HMAC-SHA256 / AES-256-CBC，盐 32 字节、IV 16 字节）、`PbeWithShaAnd3KeyTripleDesCbc`（3DES）、`PbeWithShaAnd40BitRc4Cbc`（**名字写 RC4，实际写出的 OID 是 RC2-40**：`as_oid()` 返回 `PBE_WITH_SHA_AND_40BIT_RC2_CBC_OID`，加密走 RC2 5 字节密钥）。私钥包与证书包用同一个算法。
- `MacAlgorithm`：`HmacSha1`、`HmacSha256`。缺省 PBES2/AES-256 + HMAC-SHA256，迭代 10000（加密与 MAC 各自可调）。
- 本仓库 `crates/rurge-proto/src/keystore.rs` 的测试代码覆盖了：OpenSSL 现代格式（夹具 `OPENSSL_MODERN`）与 `-legacy` 格式（`OPENSSL_LEGACY`）都要能读，PBES2+SHA256 与 3DES+SHA1 两种写法写出的文件要能被 rustls 用于双向握手。**我只读了测试代码，没有运行**，所以"能读 / 能写"在此是"测试声称"而不是我的观察。

### 3.4 实际构建的两个附加限制

- `pkcs5` 0.7.1 在本仓库构建里只开了 `alloc, pbes2`（`.fingerprint/pkcs5-*/lib-pkcs5.json`），没有 `sha1-insecure`、`3des`、`des-insecure`。所以 **PBES2 且 PBKDF2 的 PRF 是 HMAC-SHA1 的文件**读不了（`pkcs5-0.7.1/src/pbes2/encryption.rs`：`Pbkdf2Prf::HmacWithSha1` 在没有该特性时返回 `UnsupportedAlgorithm{oid: HMAC_WITH_SHA1_OID}`），PBES2 + 3DES 也读不了。（推断）OpenSSL 1.1.x 用 `-keypbe AES-256-CBC` 之类选出 PBES2 时 PRF 缺省是 HMAC-SHA1，会落在这个缺口里；OpenSSL 3 缺省是 HMAC-SHA256，不受影响。我没有对真实文件试过。
- p12-keystore 对攻击者可控的工作量没设上限（0.3.2 才加：迭代数 > 1,000,000、scrypt 内存 > 1 GiB、并行度 > 16 一律拒绝）。`ca-p12` 来自配置文件，托管配置（远程下载的 Profile）里也可能带；解析畸形文件的 CPU / 内存风险属于设计要考虑的点。

### 3.5 p12-keystore 的新版本（读自其 [CHANGELOG](https://github.com/ancwrd1/p12-keystore/blob/main/CHANGELOG.md)，未读源码）

- 0.3.0（2026-06-07）：支持**明文 `keyBag`** 与**私钥和证书之间没有 `localKeyID` 链接**的文件；**破坏性改动**：`PrivateKeyChain::new` 改为 `(local_key_id, key: PrivateKey, certs)`，`key()` 返回 `&PrivateKey`，`chain()` 改名 `certs()`，`local_key_id()` 返回 `&LocalKeyId`；MSRV 1.85。
- 0.3.2（2026-09-14）：验证 SHA-224 ～ SHA-512/256 的 MAC 并可写出；对 MAC / PBES1 / PBKDF2 迭代数 > 1,000,000、scrypt 内存 > 1 GiB、并行度 > 16 一律拒绝；链解析遇到签发者环路时中断（此前会无界增长内存）。
- 0.4.0（2026-10-09）：`Pkcs12Archive::from_pkcs12` 暴露解码后的私钥 / 证书 / 密钥；别名冲突不再覆盖，改加 `#2`、`#3` 后缀。0.4.1（2026-10-13）：含增补字符的主体名可写出。
- 这些版本有没有放开 PBES2 的 HMAC-SHA1 PRF：更新说明里未见。
- 升级会碰到 `crates/rurge-proto/src/keystore.rs` 里的 `chain.key()` / `chain.chain()` 调用（0.3.0 改了名与类型）。

### 3.6 对 `ca-p12` 读取路径的含义（事实 + 推断）

- 读：base64（`STANDARD` 或 `STANDARD_NO_PAD`，仓库已有 `decode_base64`）→ `KeyStore::from_pkcs12(der, passphrase)` → `private_key_chain()` → `chain.key()` 是**解密后的 PKCS#8 DER**（`PrivateKeyInfo`），`chain.chain()[0]` 是 CA 证书。
- PKCS#8 再交给 rcgen / ring 时的限制见 2.4（RSA 2047 ～ 4096 位、PKCS#8 v1、EC 私钥必须带公钥）。
- 写（`rurge mitm ca generate`）：缺省的 PBES2/AES-256 + SHA-256 在 macOS 15 之前的系统上导不进钥匙串、（推断）也读不进 Surge；选 3DES + SHA1（`EncryptionAlgorithm::PbeWithShaAnd3KeyTripleDesCbc` + `MacAlgorithm::HmacSha1`；注意这会把证书包也用 3DES 而不是钥匙串那样的 RC2-40）则（推断）OpenSSL 3 不需要 legacy（3DES 属缺省 provider，RC2-40 才需要），macOS 新旧也都能导。这两点我都没有对真实文件试过。缺省选哪个是设计决定。
- 口令不能为空是 Surge 手册写明的限制；p12-keystore 本身对空串口令能工作（（推断）算出的 MAC 与 OpenSSL 对 `""` 一致），所以"拒绝空口令"是 rurge 要不要照 Surge 办的设计决定，不是库的限制。

---

## 4. rustls 0.23 当 MITM 的服务端与客户端

本机源码：rustls 0.23.43（`Apache-2.0 OR ISC OR MIT`，MSRV 1.71）、tokio-rustls 0.26.4、rustls-native-certs 0.8.4、rustls-webpki 0.103.15。

### 4.1 服务端（面向被拦截的客户端）

**按 SNI 选证书**
- `trait ResolvesServerCert: Debug + Send + Sync { fn resolve(&self, ClientHello<'_>) -> Option<Arc<CertifiedKey>>; }`——同步、不能做 I/O。文档原句："Return `None` to abort the handshake."；实际行为（`server/hs.rs`）：返回 `None` → 发 fatal alert **`access_denied`(49)**，错误文本 "no server certificate chain resolved"。
- `ClientHello<'_>` 提供：`server_name() -> Option<&str>`、`signature_schemes()`、`alpn() -> Option<impl Iterator<Item=&[u8]>>`（`None` = 客户端没发 ALPN 扩展）、`cipher_suites()`、`server_cert_types()`、`client_cert_types()`、`certificate_authorities()`、`named_groups()`。
- SNI 的处理（`process_client_hello`）：DNS 名被**转小写**；**IP 字面量形式的 SNI 当作没有 SNI**（RFC 6066 禁止，rustls 选择"解析成功但当作没发"）；格式非法 → `illegal_parameter`(47)。
- 自带的 `ResolvesServerCertUsingSni`（`server/handy.rs`）：按小写 DNS 名查表，`add()` 会检查证书覆盖该名字；**没有 SNI 就返回 `None`**（"This kind of resolver requires SNI"）。所以要处理无 SNI 的连接必须自己写 resolver。
- `ServerConfig` 构造便宜（rustls 文档："Making one of these is cheap"），按连接新建一个是可行的；重量在 `RootCertStore` 之类的输入上。

**没有 SNI 的 ClientHello**
- `server_name()` 为 `None`。resolver 看不到 CONNECT 目标或 TUN 的原始目的地址，所以要么**每条连接一个带"兜底主机名"的 resolver**，要么先读 ClientHello（见下）再按连接自己的目标名选证书。
- 同类工具：mitmproxy 无 SNI 时用客户端连到的本地 IP（`sockname[0]`）做证书名并加上服务器地址；hudsucker 无 SNI 时用 CONNECT 的 authority（`internal.rs`：`.unwrap_or(&authority)`）。（推断）rurge 的引擎对 CONNECT、SOCKS5 域名、fake-IP 映射都已知道目标主机名，SNI 只用于核对；目标是 IP 时证书要带 IP SAN（rcgen 支持）。

**ALPN 与"把上游的 ALPN 照搬给客户端"**
- ALPN 在 `ServerConfig::alpn_protocols`（"Protocol names we support, most preferred first. If empty we don't do ALPN at all."），不在 resolver 里，所以**一个共享的 `ServerConfig` 无法按连接改 ALPN**。
- 选择规则（`server/hs.rs`）：客户端发了 ALPN → 在服务器列表里按**服务器偏好顺序**找第一个客户端也提供的；没有交集且服务器列表非空 → fatal alert **`no_application_protocol`(120)**；客户端没发 ALPN → 不选。
- 照搬上游需要**先读 ClientHello、再决定 `ServerConfig`**：`tokio_rustls::LazyConfigAcceptor::new(rustls::server::Acceptor::default(), stream).await` → `StartHandshake::client_hello()`（拿到 SNI 与 ALPN 列表）→ 按它去连上游并握手 → 读上游选中的协议 → 构造 `ServerConfig`（`alpn_protocols = vec![上游选的]`，`with_no_client_auth()`，`with_cert_resolver(..)`）→ `start.into_stream(Arc<ServerConfig>)`。tokio-rustls 0.26.4 的 `StartHandshake { accepted, io }` 与 `into_stream_with(config, |conn| …)` 都有（本机源码）。hudsucker 也是用 `LazyConfigAcceptor`，但它不镜像上游：ALPN 固定为 `[h2（仅 http2 特性开启时）, http/1.1]`，按 `Authority` 缓存整个 `ServerConfig`。
- **mitmproxy 的做法**（`tlsconfig.py` 的 `alpn_select_callback`，页面原代码）：客户端已指定 ALPN 且在 offers 里 → 用它；否则上游选的 ALPN 在 offers 里 → 用上游的；上游**拒绝协商**（空串）→ 对客户端也返回"无重叠"以镜像；否则按客户端偏好顺序选 HTTP 一族。选项 `connection_strategy` 缺省 `eager`（"Determine when server connections should be established."），`upstream_cert` 缺省 `True`（"Connect to upstream server to look up certificate details."）；`tls_clienthello` 里 `establish_server_tls_first = conn_context.server.tls and ctx.options.connection_strategy == "eager"`；`layers/tls.py` 里先建上游 TLS，上游失败时只记警告然后**仍继续客户端侧握手**。
- 这强制的顺序（（推断））：客户端 ClientHello → 暂停 → 连上游并完成上游握手（得到 ALPN、证书名）→ 才给客户端发 ServerHello。代价是客户端要等上游 TCP 连接与 TLS 握手，慢上游会让客户端超时；收益是 ALPN、证书的 CN / SAN / O 都与上游一致，不会出现"客户端选了 h2、上游只讲 http/1.1"的错位。若不镜像（hudsucker 路线），客户端先收到 ServerHello，需要在 HTTP 层做 HTTP/2 ↔ HTTP/1.1 的转换。
- Surge 的 `h2` 键缺省 `false`（手册："MITM over HTTP/2 … which can improve the performance of concurrent requests"）：（推断）`h2=false` 时只向客户端通告 `http/1.1`；rustls 里客户端同时提供 `h2` 与 `http/1.1` 而服务器只列 `http/1.1` 是正常选中 `http/1.1`，不会报错。
- 另一个注意（（推断））：叶证书的 SAN 比 SNI 宽（例如照抄上游证书的全部 SAN，mitmproxy 的做法）时，Chrome 等会在 HTTP/2 上合并连接，后续其它主机名的请求会到同一条 TLS 连接上，要按每个请求的 `:authority` / `Host` 分流，不能按连接的 SNI；叶证书只含 SNI 一个名字就不会发生。
- 客户端提供的 ALPN 里一个都不认识（例如只有自定义协议）时，把 `alpn_protocols` 设成空列表可以避免 `no_application_protocol` 警报直接断开。

**会话恢复**
- 缺省（`ServerConfig` 文档）：`session_storage` 内存 256 个会话；`send_tls13_tickets` = 2；`ticketer` 不出票；0-RTT 关（`max_early_data_size = 0`）；`send_half_rtt_data = false`。
- 恢复时 `can_resume` 要求 `resumedata.sni == sni`（RFC 6066 的要求），恢复握手不再发证书。
- **每条连接新建一个 `ServerConfig` 会丢掉恢复缓存**，除非在这些配置之间共享同一个 `Arc<dyn StoresServerSessions>` / `ticketer`。rustls 文档对共享的警告：只有当两个配置的 `verifier`（客户端认证要求）与 `cert_resolver`（服务器身份）等价时才安全。MITM 的各个 `ServerConfig` 都是 `with_no_client_auth()`，（推断）共享是合理的，但 ALPN 不同的配置之间共享时恢复的行为我没有逐行核对。
- 客户端侧缓存的是对**伪造证书**的会话票据；进程重启后缓存为空，客户端会做完整握手。

### 4.2 客户端（面向上游服务器）

- **接受任何证书（`skip-server-cert-verify`）**：`ClientConfig::builder_with_provider(..).with_protocol_versions(..)?.dangerous().with_custom_certificate_verifier(Arc<dyn ServerCertVerifier>)`。`ServerCertVerifier` 要实现 `verify_server_cert`、`verify_tls12_signature`、`verify_tls13_signature`、`supported_verify_schemes`。**仓库里已经有现成的实现**：`crates/rurge-proto/src/transport/tls.rs` 的 `Verifier`，有 `Mode::Standard`（`WebPkiServerVerifier`）、`Mode::Pinned([u8;32])`（叶证书 SHA-256）、`Mode::Insecure`，三种模式下握手签名仍然校验（`verify_tls12_signature` / `verify_tls13_signature` 照常调用 `rustls::crypto::verify_*`），只是跳过链与名字校验。手册对应键：`skip-server-cert-verify`（"This relaxes verification of the real server only; the client side still receives the Surge-generated certificate."）。
- **根证书来源**：仓库的 `rurge_net::tls::root_store()`：`rustls_native_certs::load_native_certs()`，只有读到 **0 个**时才补 `webpki_roots::TLS_SERVER_ROOTS`。
  - `rustls-native-certs` 0.8.4：先看 `SSL_CERT_FILE` / `SSL_CERT_DIR`（设了就只读这两处，不读系统库）；macOS 合并 User / Admin / System 三个域的信任设置（用户 > 管理员 > 系统）；Windows 读 Current User 的 `ROOT` 并过滤时间与 EKU；Linux 靠 `openssl-probe` 找系统 CA 文件。README 的提示框原话：“Instead of this crate, we suggest using rustls-platform-verifier … This crate is still maintained, but mostly for use inside the platform verifier on platforms where no other solution is available.”
  - [`rustls-platform-verifier`](https://github.com/rustls/rustls-platform-verifier) 0.7.1（2026-09-24，`MIT OR Apache-2.0`，MSRV 1.85）：Windows 用系统证书验证 API、macOS / iOS 用 Security.framework、Android 用 Trust Manager、Linux 用 webpki + 系统 CA 文件；吊销（OCSP / CRL）在 Windows / macOS / iOS 有，Linux 没有。能规避"Windows 根库惰性填充读不全"的问题（（推断））；代价是要写自己的 `ServerCertVerifier` 适配与每个平台的集成测试。
  - `webpki-roots` 1.0.9（`CDLA-Permissive-2.0`）：Mozilla 根列表编进二进制，不随系统更新，也读不到用户私有根。（推断）上游验证如果要同时覆盖"企业内部根"与"Windows 惰性根"，取 native ∪ webpki-roots 比"native 读到 0 个才退回"更稳。
- **客户端证书（上游要求 mTLS）**：`ClientConfig::builder()…with_client_auth_cert(Vec<CertificateDer>, PrivateKeyDer)` 或 `with_client_cert_resolver`；仓库的 `ClientIdentity` + `decode_p12` 已经为 `[Keystore]` 的 `client-cert` 做了。Surge 的 `[MITM]` **没有**客户端证书键（本仓库兼容清单也写"手册未定义的键（如 `tcp-connection`、客户端证书）不存在"）。（推断）如果上游向 MITM 代理索要客户端证书，而代理手里没有，握手会失败；对客户端一侧用 `with_no_client_auth()` 则客户端永远不会被索要证书，依赖双向 TLS 的 App 在被解密的主机上会坏。mitmproxy 有选项 `client_certs`（按 `<主机名>.pem` 文件给上游证书）与 11.0.0 起"full mTLS with client certs between client and mitmproxy"；hudsucker 未见。排除这类主机靠 `hostname = -host`。
- **ALPN**：`ClientConfig::alpn_protocols`（空 = 不发扩展）；`check_selected_alpn` 缺省 `true`（检查服务器选的协议在已提供的里面）。要把客户端的 ALPN 交集转给上游，需要按 ALPN 组合（`[h2,http/1.1]` / `[http/1.1]` / `[]`）各备一个 `ClientConfig`，它们可以共享同一个 `Resumption` 存储。
- **SNI**：`ServerName::try_from(host)` 得到 `DnsName` 或 `IpAddress`；IP 不发 SNI（仓库注释："rustls sends no SNI for an IP address"）；`enable_sni` 可关。
- **恢复**：`ClientConfig` 文档写的缺省是 “supports resumption with up to 256 server names, using session ids or tickets, with a max of eight tickets per server”（`Resumption::in_memory_sessions(256)`，含 TLS 1.2 的 session id / ticket）；`Resumption::disabled()` 可关。每个新建的 `ClientConfig` 自带一份内存会话缓存，`Clone` 出来的共享同一份 `Arc`。让"跳过验证"与"严格验证"的配置共用一份缓存是否安全，我没有核对 rustls 在客户端侧设了什么防护；（推断）按"验证模式 × ALPN 集合 × 客户端证书"分开各用一份更稳。

### 4.3 怎样看出客户端拒绝了伪造证书

**RFC 8446 第 6.2 节的警报定义**（[RFC 8446](https://www.rfc-editor.org/rfc/rfc8446.html)，经摘要）：`bad_certificate`(42) "A certificate was corrupt, contained signatures that did not verify correctly, etc."；`unsupported_certificate`(43)；`certificate_revoked`(44)；`certificate_expired`(45) "A certificate has expired or is not currently valid."；`certificate_unknown`(46) "Some other (unspecified) issue arose in processing the certificate, rendering it unacceptable."；`illegal_parameter`(47)；`unknown_ca`(48) "A valid certificate chain or partial chain was received, but the certificate was not accepted because the CA certificate could not be located or could not be matched with a known trust anchor."；`access_denied`(49)；`unrecognized_name`(112)；`no_application_protocol`(120)。

**在 rustls / tokio-rustls 里的表现**
- 客户端发来致命警报：rustls 报 `Error::AlertReceived(AlertDescription::…)`；tokio-rustls 0.26.4 把 `process_new_packets()` 的错误包成 `io::Error::new(io::ErrorKind::InvalidData, rustls_error)`（`common/mod.rs`），可 `downcast_ref::<rustls::Error>()` 取回；`TlsAcceptor::accept` / `StartHandshake::into_stream` 的 future 以此失败。
- 握手期间 TCP 被关（FIN 或 RST，没有警报）：`io::ErrorKind::UnexpectedEof`，消息 "tls handshake eof"；`LazyConfigAcceptor` 读 ClientHello 阶段对端关闭同样给 `UnexpectedEof`。
- 握手**成功**后客户端不发请求就关：第一次 `read` 得到 `Ok(0)`（对端发了 `close_notify`）或 `Err(UnexpectedEof)`（rustls 的“没有 close_notify 就关”，tokio-rustls 原样透出）。这正是 **Surge 手册描述的信号**（`http_mitm.html.txt`）：“If a client completes the TLS handshake but disconnects without sending a request, Surge logs a hint that the host is likely protected by certificate pinning.”
- 服务器侧自己发出的警报（对照）：resolver 返回 `None` → `access_denied`；ALPN 无交集 → `no_application_protocol`；密码套件不兼容 → `handshake_failure`(40)；版本不兼容 → `protocol_version`(70)。

**真实客户端发什么警报**（只有零散证据）：
- Chrome / BoringSSL：`VerifyCertCallback` 没设 `*out_alert`，具体警报在 BoringSSL 里决定，（推断）缺省是 `certificate_unknown`(46)；NVISO 报告 Android WebView 对过长有效期的叶证书在 Burp 里表现为 `unknown_ca`（**该警报的含义在这里不是"CA 没装"**）。
- mitmproxy 在 OpenSSL 的警报文本里匹配 `"unknown ca"`、`"bad certificate"`、`"certificate unknown"` 三种，说明它在真实客户端那里见过这三种。
- 有些 App 的固定校验发生在 TLS 栈之外（握手完成之后）；也有 Apple 平台上的 App 在自定义信任评估里直接取消连接，不发警报（（推断））。同一个警报（`unknown_ca` / `certificate_unknown` / `bad_certificate`）既可能是"根没装/没信任"，也可能是"证书固定"或"有效期过长"，**服务器端无法区分**。

**mitmproxy 怎么报告**（`proxy/layers/tls.py` 的 `ClientTLSLayer.on_handshake_error`，页面原代码）：
- 警报文本含 `unknown ca` / `bad certificate` / `certificate unknown` → `The client does not trust the proxy's certificate for {dest} ({err})`（`dest` 取 SNI，没有 SNI 取服务器地址）；
- `err == "connection closed"` → INFO 级："The client disconnected during the handshake. If this happens consistently for {dest}, this may indicate that the client does not trust the proxy's certificate."；
- `err == "connection closed early"` → 不记日志；
- 其它 → `The client may not trust the proxy's certificate for {dest} ({err})`；
- 无论哪种，都触发 `tls_failed_client` 钩子（`TlsFailedClientHook`）；日志是 `Client TLS handshake failed. {err}`。证书固定本身在这个文件里没被提到。

**hudsucker**：握手失败只记 error 级的 `"Failed to establish TLS connection"`（带底层错误），读不出 ClientHello 记 `"Failed to read TLS client hello"`；不区分原因。

---

## 5. 正则：Surge 的方言、ICU 与 fancy-regex / regex 的差异

### 5.1 关于 Surge 用哪个引擎：证据状态

- **手册没有点名**。本机快照里 `http_url-rewrite.html.txt`、`http_body-rewrite.html.txt`、`http_header-rewrite.html.txt`、`http_map-local.html.txt`、`rules_http.html.txt`、`scripting_*.html.txt`、`policy-groups_policy-including.html.txt` 只写 "regular expression" / "regex"；对 ICU / NSRegularExpression / PCRE 的全文搜索无命中。手册里和语法有关的全部句子：
  - URL Rewrite：“The regular expression is matched against the full request URL. The replacement supports capture group references such as `$1`.”；匹配对象还包括用 Host 头、用底层连接主机名重建的 URL。
  - Body Rewrite：“The replacement supports capture group references such as `$1`. The `^` and `$` anchors match at the beginning and end of each line of the body.”；“The body must be valid UTF-8 text for regex-based rewriting. If it is not, the rule is skipped and a note is added to the request journal.”；第三个参数可连续追加多对"正则 + 替换"；“Even if the original request does not contain a body, new content may still be generated through body rewrite, such as using the `^$` expression.”
  - URL-REGEX 规则：“The rule matches when the pattern is found anywhere in the URL; anchor with `^` when you want a prefix match. The regular expression is case-sensitive.”（`rules_http.html.txt`；仓库 `docs/reports/2026-10-14-manual-baseline-recheck.md` 的 R37 也记了这句）
  - 脚本的 `pattern`：“The line is invalid if the regex does not compile.”
  - Header Rewrite 的 `header-replace-regex`：只给了一个例子（`User-Agent Safari Chrome`），**没有写模板语法**，也没写 `$1`。
- **间接证据**：Surge 跑在 Apple 平台；`$1` 引用与"`^` `$` 逐行锚定"正好是 `NSRegularExpression` 的模板语法与 `anchorsMatchLines` 选项；swift-corelibs-foundation 里 `NSRegularExpression` 的文档注释明说 “The pattern syntax currently supported is that specified by ICU.”（[源码](https://github.com/swiftlang/swift-corelibs-foundation/blob/main/Sources/Foundation/NSRegularExpression.swift)）。Surge 5.9 / 5.14 的更新日志（Telegram 频道）只提到 "URL-REGEX now supports `extended-matching` tags" 与 jq Body Rewrite，没有谈引擎。社区资料我没有找到任何一条直接说"Surge 用 ICU / NSRegularExpression"的（未见）。
- **结论**：“Surge = NSRegularExpression（ICU 语法）”是**合理的推断，没有证实**。仓库的阶段 1 设计、阶段 2 M3a 计划已把它当前提（`docs/superpowers/specs/2026-09-03-phase1-core-skeleton-design.md` 第 53 行，`…/plans/2026-09-23-phase2-m3a-subscriptions-plan.md` 第 56 行）；`rurge_config::rule::Pattern` 目前就是 `fancy_regex::Regex::new(source)`，没有任何自定义 `RegexBuilder` 选项。
- **替换串写法**：手册只写 `$1`，没有出现 `\1`。NSRegularExpression 的模板里反斜杠只转义 `$` 和反斜杠自己（见 5.3），所以 `\1` 在 Surge 里（若确是 NSRegularExpression）会输出字面的 `1`，不会被当成分组引用。

### 5.2 语法特性对照（ICU 以其[用户指南](https://unicode-org.github.io/icu/userguide/strings/regexp.html)为准；fancy-regex 以**本机 0.14.0 源码**为准；regex 以本机 1.13.1 为准）

| 特性 | ICU（NSRegularExpression） | fancy-regex 0.14.0 | regex 1.13 |
|---|---|---|---|
| 先行 `(?=`、`(?!` | 支持 | 支持（走回溯 VM） | 不支持 |
| 后行 `(?<=`、`(?<!` | 支持，但长度必须有界（“must not be unbounded (no * or + operators)”），**可以是有界的变长** | 支持，但**必须定长**：`(?<=a|bc)`、`(?<=a{1,3})` 编译报 `Look-behind assertion without constant size`（`compile.rs`） | 不支持 |
| 反向引用 `\1`、`\k<name>` | 支持 | 支持 | 不支持 |
| 命名组 | `(?<name>…)`；替换里 `${name}` | `(?<name>…)`、`(?P<name>…)`；`\k<name>`、`(?P=name)` | `(?<name>…)`、`(?P<name>…)` |
| 原子组 `(?>…)` | 支持 | 支持 | 不支持 |
| 占有量词 `*+` `++` `?+` | 支持 | 支持（量词后再跟 `+` 就包成原子组） | 不支持 |
| `\Q…\E` | 支持 | **不支持**：`\Q` 报 `InvalidEscape` | 不支持 |
| 内联标志 | `(?ismwx-ismwx:…)`（`w` = Unicode 词边界） | `(?imsUxu)`：`i m s x` 同义，`U` = **交换贪婪/懒惰**（ICU 没有），**`w` 报 `UnknownFlag`** | `i m s R U u x`（`R` = CRLF 模式） |
| `(?#注释)` | 支持 | 支持 | 不支持 |
| `\A`、`\z` | 支持 | 支持 | 支持 |
| `\Z`（末尾行终止符之前） | 支持 | **0.14 不支持**（0.15.0 才加）：报 `InvalidEscape` | 不支持 |
| `\h` / `\H` | **水平空白** / 非水平空白 | **十六进制数字** `[0-9A-Fa-f]` / 非十六进制（Oniguruma 语义）——**静默语义相反** | 不支持（报错） |
| `\v` / `\V` | `\v` = 换行字符（不含 CRLF 序列）；`\V` = 非换行 | `\v` = 垂直制表符 U+000B；`\V` 报错 | `\v` = U+000B |
| `\R`、`\X`、`\N{名}`、`\cX`、`\0ooo` | 支持 | `\R` `\X` `\N` `\c` 报 `InvalidEscape`（0.18 才有 `\R`、`\N`，且 `\N` 的含义是"除换行外任意字符"，不是 Unicode 名）；`\0` 走反向引用分支，未逐项验证 | 不支持 |
| `\x{h…}`、`\uhhhh`、`\Uhhhhhhhh` | 支持 | 支持 | `\x{…}` `\u` `\U` 支持 |
| `\p{…}` / `\P{…}` | ICU 的属性名全集 | 交给 regex-syntax：通用类别、脚本、二值属性；ICU 专有写法不保证 | 同左 |
| `\<` `\>` | 字面 `<` `>` | **词的开始 / 结束边界**（0.16.2 起可用 "oniguruma mode" 开关改成字面）——静默差异 | 不支持 |
| `\G`、`\K` | `\G` 支持；`\K` 文档未列 | 都支持 | 不支持 |
| 条件 `(?(1)…|…)`、递归 | ICU 文档未列 | 条件支持；递归（子程序调用）0.18 起 | 不支持 |
| `.` | 不匹配行终止符（U+000A、U+000B、U+000C、U+000D、U+0085、U+2028、U+2029；CRLF 作为一个整体），`s` 标志才匹配 | 只排除 `\n` | 只排除 `\n` |
| `$` / `^` 的"行"概念（多行模式） | 行终止符同上（U+000A、U+000B、U+000C、U+000D、U+0085、U+2028、U+2029 与 CRLF 整体） | 只认 `\n`；`(?R)` CRLF 模式 0.18 才有 | 只认 `\n`（regex 文档："they only recognize `\n` (assuming CRLF mode is not enabled)"）；`(?mR)` 时认 `\r` `\n` 且不拆 `\r\n` |
| `\s` / `\w` / `\d` | `\s` = `[\t\n\f\r\p{Z}]`；`\w` 含 Alphabetic、Mark、Nd、Pc、U+200C/D；`\d` = Nd | Unicode 感知（委托给 regex） | Unicode 感知；`\s` 含 `\v` 与 U+0085 |
| 资源限制 | 回溯栈缺省 8 MB（超出报错）；可设时间上限 | 回溯次数缺省 **1,000,000**（`RegexBuilder::backtrack_limit`），超出 `RuntimeError::BacktrackLimitExceeded`，另有 `StackOverflow`；不含"fancy"特性的部分委托给 regex（线性）；`delegate_size_limit` / `delegate_dfa_size_limit` 缺省不设 | 线性时间，无回溯 |

fancy-regex 之后的版本（读自其 [CHANGELOG](https://github.com/fancy-regex/fancy-regex/blob/main/CHANGELOG.md)，未读源码；本机 0.14.0 → 最新 0.20.0）：0.15.0 加 `\Z`、`\O`；0.16.2 加"oniguruma mode"开关（`\<` `\>`）；0.17.0 加变长后行断言（需特性开关）、`\b{start}` 等；0.18.0 加 `\N`、`\R`、`(?R)` CRLF 标志、子程序调用、更多 `\p` 别名；0.19.0 加字节模式 / `RegexSet`，**公开类型变成泛型（破坏性）**；0.20.0（2026-10-24）加变长"硬"后行断言、`max_prog_size`。**任何版本都没有 `\Q…\E`、`\X`、也没有把 `\h` 改成水平空白。**

### 5.3 替换串语法

| | 规则 | 出处 |
|---|---|---|
| NSRegularExpression 模板（Foundation 文档注释） | “The replacement is treated as a template, with $0 being replaced by the contents of the matched range, $1 by the contents of the first capture group, and so on. Additional digits beyond the maximum required to represent the number of capture groups will be treated as ordinary characters, as will a $ not followed by digits. Backslash will escape both $ and itself.” 实现：按"表示分组数所需的位数"读 `$` 后的数字（最多 20 位）；编号 ≥ 分组数 → 替换为空串；`$` 后不是数字 → 保留字面 `$` | swift-corelibs-foundation 的 `NSRegularExpression.swift`（`replacementString`）；Apple 的头文件措辞相同（搜索摘要） |
| ICU `appendReplacement`（C API，对照） | `$n`：在不超过分组数的前提下贪心吃数字；`${name}`；`$` 后不是数字也不是 `{` → **错误** `U_REGEX_INVALID_CAPTURE_GROUP_NAME`；反斜杠后任意字符按字面输出，末尾孤立反斜杠被丢弃 | ICU `rematch.cpp`（摘要）。NSRegularExpression 有自己的模板实现，与此处并不完全相同（尤其是裸 `$`） |
| fancy-regex 0.14 `Captures::expand` / `Expander::default()` | `$name` / `${name}`；`$` 后取**最长**的 `[0-9A-Za-z_]+` 作名字：`$1a` 是名为 `1a` 的分组而不是分组 1 后跟字母 a，要写 `${1}a`；`$$` 表示字面 `$`；**不处理反斜杠**；不存在的分组 → 空串 | 本机源码 `lib.rs`（`Captures::expand` 的文档）与 `expand.rs` |
| fancy-regex 的 `Expander::python()` | `\1`、`\g<name>`、`\\`；取最长数字（`\10` = 分组 10） | 本机源码 `expand.rs` |
| regex 1.13 `replace` | 同 `$name` 规则；`$$` 为字面 `$` | 本机源码 `regex/string.rs` |

### 5.4 用 fancy-regex 跑社区模块里的 Surge 正则，会坏在哪里

**响亮地坏（编译期报错，规则被丢弃或整行无效）**
- `\Q…\E`、`\Z`（0.14）、`(?w)`、`\R`、`\X`、`\N{…}`、`\cX`、`\V`；
- 有界变长后行断言，如 `(?<=\d{1,3})`、`(?<=http|https)`。

**静默地坏（编译通过，行为不同，最危险）**
- `\h` / `\H`：ICU 是水平空白，fancy 是十六进制数字；
- `\v`：ICU 是换行类，fancy 是 U+000B；
- `\<` `\>`：ICU 是字面量，fancy 是词边界；
- `.` 与（多行模式下的）`^` `$` 对 `\r`、`\r\n`、U+0085、U+2028、U+2029 的处理：HTTP 正文常用 CRLF，`^foo=(.*)$` 在 ICU 下捕获不含 `\r`，在 fancy 0.14 下会把行尾的 `\r` 吞进捕获；
- 替换串：`$1abc`（fancy 解析成分组 `1abc`，得空串）、`\$`（fancy 得到 `\` 加 `$ref`）、`\\`（fancy 得到两个反斜杠）、数字位数（`$12` 在只有 1 个分组时，NSRegularExpression 读一位数字、保留字面 `2`，fancy 读分组 `12` 得空串）；
- `U` 标志：fancy 把它当"交换贪婪"，ICU 没有这个标志；
- 回溯上限：fancy 超限是**运行时错误**而不是慢，仓库现有的约定是"出错按不匹配处理"（M3a 计划）；Body Rewrite 里对一个最大 10 MB 的响应体做 `replace_all`，超限后"跳过并加备注"还是"当成没匹配"需要设计决定；ICU 则是时间 / 栈上限。

**两个与 Surge 手册直接相关的实现要求**
- “`^` and `$` anchors match at the beginning and end of each line of the body” → Body Rewrite 要在 fancy-regex 里打开多行（`(?m)`，0.15.0 起也可用 `RegexBuilder` 选项）；是否把 URL Rewrite / Header Rewrite 的正则也按多行处理，手册没说（未见）。
- “Even if the original request does not contain a body, new content may still be generated … such as using the `^$` expression” → 空正文也要跑一遍替换（regex / fancy 对空输入匹配 `^$` 没有问题）。

**可选的缓解思路**（供设计取舍，不是推荐）：(a) 自写 ICU 兼容的替换展开器（规则简单：NSRegularExpression 模板语义，约几十行）；(b) 编译前对 ICU 专有语法做一遍改写（`\Q..\E` → 转义；`\h` → `[ \t\p{Zs}]` 等）；(c) 升级到 fancy-regex ≥ 0.18（得到 `\Z`、`\R`、`(?R)`、变长后行断言，但仍无 `\Q..\E`）；(d) 汇集真实社区模块的正则做语料测试——我没有收集到这样的语料（未见）。

---

## 6. 用 Rust 跑 jq：jaq

### 6.1 crate 与版本（crates.io API 页的 WebFetch 摘要，2026-10-29）

| crate | 最新版 | 日期 | 许可 | MSRV（crates.io 元数据） | 说明 |
|---|---|---|---|---|---|
| `jaq-core` | 3.1.2 | 2026-10-21 | MIT | 1.69 | 解释器；依赖 `dyn-clone`、`once_cell`、`typed-arena`（+ 可选 `arbitrary`）；特性 `default = [std]`；累计下载 442 万 |
| `jaq-std` | 3.0.4 | 2026-10-21 | MIT | 1.70 | 标准库；特性 `default = [std, format, log, math, regex, time]`；依赖 `bstr`、可选 `aho-corasick`、`base64`、`jiff`、`libm`、`log`、`regex-bites`、`urlencoding` |
| `jaq-json` | 2.0.3 | 2026-08-28 | MIT | 1.70 | JSON 值 `Val` 与读写；特性 `std`（缺省）、`serde`（→ `serde_core`）、**`sync`**；依赖 `bstr`、`bytes`、`foldhash`、`hifijson`、`indexmap`、`num-bigint`、`num-traits`、`ryu`、`self_cell`… |
| `jaq-all` | 0.4.0 | 2026-10-21 | MIT | 1.70 | “Compile & run jq filters and read & write data … opinionated, high-level API”，再导出前四个，另有 `data::compile` / `data::run` / `compile_with`（docs.rs 页面没给签名，未见） |
| `jaq-fmts` | 0.1.1 | — | — | — | YAML / CBOR / TOML / XML 等格式（jaq-all 依赖），没细看 |

- 仓库：[01mf02/jaq](https://github.com/01mf02/jaq)（MIT，`LICENSE-MIT`），手册 [gedenkt.at/jaq/manual](https://gedenkt.at/jaq/manual/)。3.0.0 在 2026-03-27 发布，之前有 alpha（2025-11）、beta（2025-12）、gamma（2026-03）；老的 `jaq-interpret` 1.5.0 / `jaq-syn` 1.6.0 / `jaq-parse` 1.0.3（2024）已被取代。同一作者；README 的徽章写 "Rust 1.69+"。
- MSRV 都低于工作区的 1.89。

### 6.2 在一个 JSON 值上运行过滤器（jaq-core 3.1.2 的 crate 文档，[docs.rs](https://docs.rs/jaq-core/latest/jaq_core/)）

文档里的示例（我把几行合并了，其余照抄；jaq 是 MIT 许可）：

```rust
use jaq_core::{data, unwrap_valr, Compiler, Ctx, Vars};
use jaq_core::load::{Arena, File, Loader};
use jaq_json::{read, Val};

let input = read::parse_single(&r#"["Hello", "world"]"#.as_bytes()).unwrap();
let program = File { code: ".[]", path: () };
let defs = jaq_core::defs().chain(jaq_std::defs()).chain(jaq_json::defs());
let funs = jaq_core::funs().chain(jaq_std::funs()).chain(jaq_json::funs());
let loader = Loader::new(defs);
let arena = Arena::default();
let modules = loader.load(&arena, program).unwrap();                       // 解析
let filter = Compiler::default().with_funs(funs).compile(modules).unwrap(); // 编译
let ctx = Ctx::<data::JustLut<Val>>::new(&filter.lut, Vars::new([]));        // 运行上下文
let mut out = filter.id.run((ctx, input)).map(unwrap_valr);                   // 输出是迭代器
```

- 输出是一个**迭代器**，每项 `Result<Val, …>`；一个过滤器可以产生 0 个、1 个或多个输出。
- **全局变量（`--arg` 一类）**：`Compiler::with_global_vars(impl IntoIterator<Item = &str>)`——“Assume the existence of global variables with given names. The names all have to start with `$`. For execution, the corresponding values have to be provided via `crate::Ctx::new`.”；运行时 `Vars::new([val, …])` 按声明顺序给值。jaq 命令行有 `--arg`、`--argjson`、`--slurpfile`、`--rawfile`、`--args`（手册 CLI "Variables" 一节）。
- 线程：`jaq_json::Val` 缺省 **`!Send` 且 `!Sync`**（docs.rs 的 “Auto Trait Implementations”：`impl !Send for Val`），因为其 `Arr(Rc<Vec<Val>>)`、`Obj(Rc<Map<Val, Val>>)`。**`jaq-json` 的 `sync` 特性把 `Rc` 换成 `Arc`**（源码：`#[cfg(feature = "sync")] pub use alloc::sync::Arc as Rc;`，且有 `#[cfg(feature = "sync")] #[test] fn val_send_sync()`）。编译产物 `Filter<F>` 的 `Send`/`Sync` 取决于 `Lut<F>`；`data::JustLut<V>` 的文档写 “This type is mostly used for testing.”；`Filter<Native<JustLut<Val>>>` 在启用 `sync` 时是否 `Send + Sync`：docs.rs 没列，**未见**，需要实际编译验证。在 tokio 里使用的稳妥做法（（推断））：整个"解析 JSON → 跑过滤器 → 序列化"放进一个不跨 `.await` 的同步块（`spawn_blocking`），过滤器每次编译或按线程缓存。
- JSON 读写：`jaq_json::read::{parse_single, parse_many, parse_single_num, read_many}`；`Val` 实现 `Display`；`write` 模块有 `write_val` 等宏（是否支持缩进输出：本报告没核对）。**不依赖 `serde_json`**。

### 6.3 与 jq 1.6 / 1.7 的差异（jaq 手册，WebFetch 摘要；整页约 24 万字符，我让它读了前 20 万字符、并按主题追问过几次，摘要没覆盖到的差异可能有漏，"Modules" 一节没读到）

- **数字**：jaq 区分整数、浮点和十进制（decimal）数："A number without a dot and without an exponent is losslessly stored as integer … integers of arbitrary size"；非整数先存成十进制字符串，计算时转成 64 位 IEEE-754。jq 对任何数字都用浮点。对应的差异：数组下标用浮点在 jaq 里是错误；`nan | tojson | fromjson` 在 jaq 里是恒等（NaN），在 jq 里变 `null`；`n / 0`：jq 报错，jaq 在 `n == 0` 时得 `nan`。（（推断）这意味着大整数 ID 在 jaq 里往返不丢精度，与 jq 1.7 保留字面精度的行为接近，与 jq 1.6 不同。）
- **对象键**：jaq 允许任意值作键；jq 只允许字符串。对象用保序 map（`Map`: “Order-preserving map”）。
- **切片与索引**：jaq 对 `null` 切片报错（jq 得 `null`）；若干边角情形报错与否不同。
- **`reduce` / `foreach`**：“The execution of `reduce` and `foreach` differs between `jq` and jaq when `update` yields multiple outputs.”
- **`limit`**：`$n < 0` 时 jq 报错，jaq 不；`range/1,2` 在 jq 里是 `range/3` 的受限版本。
- **`input`**：没有更多输入时 jq 报错，jaq 无输出。
- **字符串**：`@base64d` 在 jaq 里要求整串合法；jq 接受部分合法；jaq 允许自定义 `@name` 格式；`fromjson` 在 jq 里遇到多个 JSON 值报错。
- **不支持**（手册 “Unsupported” 一节）：`$__loc__`、`modulemeta`、`have_literal_numbers`、`have_decnum`、`$JQ_BUILD_CONFIGURATION`、`builtins`、`input_line_number`、SQL 风格的 `INDEX` / `JOIN` / `IN`、流式的 `tostream` / `fromstream` / `truncate_stream`；命令行的 `--stream`、`--seq`、`--ascii-output` 等。
- **jaq 多出来的**：字节串、多种数据格式（YAML / CBOR / TOML / XML）、`halt($exit_code)`、`repl` 等。
- **正则**（`test` / `match` / `capture` / `scan` / `split/2` / `sub` / `gsub`）：手册说 “jaq uses the `regex-lite` crate”，标志 `g n i m s l x`；而 jaq-std 3.0.4 在 crates.io 上的依赖是 `regex-bites`（特性 `regex`）。两者同属 [rust-lang/regex](https://github.com/rust-lang/regex/tree/master/regex-bites) 的"小体积"引擎：[docs.rs](https://docs.rs/regex-bites/latest/regex_bites/) 写 “Unicode character classes of the form `\p{...}` and `\P{...}` are not supported at all”、“Case insensitive searching is limited to ASCII case insensitivity”、“`\d`, `\s` and `\w` only consist of ASCII codepoints”，lookaround / 反向引用未提（（推断）不支持）。真正的 jq 用 Oniguruma（我的常识，手册摘录没提），两者在这里不等价。Surge 手册里唯一的 jq 例子 `test("^X-")` 在 jaq 里没有问题。
- **日期时间**用 `jiff`；手册 “Date & Time” 记的差异：jq 不允许 ISO 8601 字符串里带时区信息；jq 打印 `GMT` 而 jaq 打印 `UTC`。

### 6.4 Surge 手册对 jq 形式的全部约束，和还没定的语义

手册（`http_body-rewrite.html.txt`，iOS 5.14.0+ / Mac 5.9.0+）：`http-request-jq <url 正则> <jq 表达式>`、`http-response-jq …`；正文必须是合法 JSON，否则"跳过并在请求记录加备注"；“If the jq expression produces empty output, the original body is kept.”；“An invalid jq expression produces a warning and the rule is skipped.”；例子 `'.headers |= with_entries(select(.key | test("^X-") | not))'`；与正则形式共享 32 MB 请求 / 10 MB（iOS 1 MB）响应上限。
手册**没写**（未见）：Surge 内嵌的 jq 版本；输出多个值时怎么办（取第一个？拼接？）；输出是否美化（缩进）还是紧凑；数字（大整数、浮点）如何往返；`$ENV` / `env` / `now` / `input` 是否可用；执行的时间 / 内存上限。

### 6.5 其它可选 crate

- [`xq`](https://crates.io/crates/xq) 0.5.0（2026-02-21，MIT，“A reimplementation of jq.”，累计下载 9.5 万，近期 609）：另一个纯 Rust 的 jq 重实现，维护节奏明显比 jaq 慢（0.4.1 与 0.4.3 之间隔了近两年）。
- [`jq-rs`](https://crates.io/crates/jq-rs) 0.4.1（2019-08-17，Apache-2.0/MIT，libjq 绑定）：停更；要编译 C 的 jq。
- `jaq-interpret` / `jaq-syn` / `jaq-parse`（1.x，2024）：被 jaq-core 3.x 取代。
- `jql`、JSONPath、JMESPath 类 crate：是不同的查询语言，不是 jq 子集（crates.io 搜索 "jq" 的前 20 名里没有它们，我没有逐个评估）。

---

## 7. 正文解码：gzip / deflate / br / zstd

### 7.1 Surge 与同类工具怎么处理"改写之后"

- **Surge**（`http_body-rewrite.html.txt`；仓库 R99 也记了）：“When the body is modified, Surge normalizes the related headers automatically: the body is decompressed first if necessary, and `Content-Length` is recalculated.” 手册没写会不会重新压缩、有没有去掉 `Content-Encoding`（未见）；（推断）发出去的是未压缩正文，`Content-Encoding` 被去掉。手册也没写 10 MB（iOS 1 MB）的限制是量压缩前还是解压后的大小（未见）。
- **mitmproxy**（`http.py` 的 `Message.set_content`）：用原有的 `Content-Encoding` **重新编码**（`encoding.encode(value, ce or "identity")`）；遇到未知编码或编码失败就 `del self.headers["content-encoding"]` 并存原始字节；`Content-Length` 仅在没有 `Transfer-Encoding` 头时更新。选项 `anticomp`（缺省 False）：“Try to convince servers to send us un-compressed data.”（即改请求的 `Accept-Encoding`）。`net/encoding.py` 支持 `gzip`、`deflate`、`deflateraw`、`br`、`zstd`；**没有任何解压体积上限**（页面摘要：“The file has no `max_length` parameter or size check.”）。
- **hudsucker**（`decoder.rs`，特性 `decoder`，用 `async-compression`）：支持 `gzip`（`x-gzip`）、`deflate`、`br`、`zstd`、`identity`；解码后**去掉 `Content-Encoding` 与 `Content-Length`**；多重编码（`gzip, deflate`）按相反顺序逐层解；**未知编码直接报错**；`deflate` 只按 zlib 封装解（`ZlibDecoder`），不处理裸 deflate。

### 7.2 "deflate" 到底是哪种

HTTP 的 `deflate` 内容编码按 RFC 9110 §8.4.1.2 是 zlib 封装（RFC 1950）里装 deflate 流（RFC 1951）的数据；实际有的服务器发**裸 deflate**（无 zlib 头与校验和）。mitmproxy 的做法：先 `zlib.decompress(content)`，失败再 `zlib.decompress(content, -15)`，注释 “Some servers may respond with compressed data without a zlib header or checksum.” hudsucker 只做前者。flate2 对应 `read::ZlibDecoder`（zlib 封装）与 `read::DeflateDecoder`（裸）；怎样探测是哪种（比如先看 zlib 头两个字节的 CMF / FLG 校验）要自己写——flate2 本身不探测。RFC 9110 原文我只拿到被摘要截断的片段，措辞请对照 [RFC 9110 §8.4.1.2](https://www.rfc-editor.org/rfc/rfc9110.html#section-8.4.1.2)。

### 7.3 可选的解码库

| 编码 | 纯 Rust 选项 | C 绑定选项 | 备注 |
|---|---|---|---|
| gzip / zlib / deflate | **`flate2` 1.1.10**（`MIT OR Apache-2.0`，MSRV 1.67，累计 7.96 亿下载，2026-08-28）。本机默认特性 `rust_backend`（→ `miniz_oxide` 0.9.1，`MIT OR Zlib OR Apache-2.0`）+ `runtime_detection`；构建指纹（`.fingerprint/flate2-*`）确认只启用了 miniz_oxide 后端。flate2 另有可选的 `zlib-rs` 后端（纯 Rust）；`Cargo.lock` 的 flate2 依赖里虽列了 `zlib-rs`，但没有编译它 | `zlib`（`libz-sys`）等 | `GzDecoder` 只解**第一个** gzip 成员，`MultiGzDecoder` 解全部成员（本机源码文档：“MultiGzDecoder decodes all members of a file and returns Ok(0) once the underlying reader does.”）；仓库 `rurge-rules` 已用 `flate2::read::GzDecoder` 解 GeoIP 下载包 |
| br | **`brotli-decompressor` 6.2.0**（仅解码；`BSD-3-Clause/MIT`；2026-10-25）；**`brotli` 9.1.1**（编码 + 解码；`BSD-3-Clause AND MIT`；MSRV 1.59；2026-10-27，累计 3.09 亿下载） | — | 都是 Dropbox 的 rust-brotli 一族，无 stdlib 依赖；README：“all the advantages of using safe rust (except in the FFI bindings)”；API：`brotli::Decompressor::new(reader, buf_size)`、`BrotliDecompress(..)` |
| zstd | **`ruzstd` 0.9.1**（`MIT`；MSRV 1.87；2026-10-11；累计 7526 万下载）：“fully operational implementation of the decompression portion”，也有压缩器（Uncompressed / Fastest ≈ 级别 1 / Default ≈ 3 / Better ≈ 7 / Best ≈ 11，“does not yet reach the speed, ratio or configurability”）；README：解码在 enwik9 上"about 3.5 times slower"，不易压缩的数据上约 1.4 倍；“currently actively maintained” | **`zstd` 0.14.3**（crates.io API 摘要给的许可是 `BSD-3-Clause`，与我印象中的 MIT 不一致，未核实；MSRV 1.64；2026-10-16；累计 4.5 亿下载）：（推断）编译捆绑的 C 版 zstd，需要 C 编译器 | 流式 API：`ruzstd::decoding::StreamingDecoder` / 低层 `FrameDecoder` |
| 异步适配 | `async-compression` 0.4.51（`MIT OR Apache-2.0`，MSRV 1.83）：tokio / futures 适配器，特性 `gzip` `zlib` `deflate` `brotli` `zstd` | | hudsucker 的 `decoder` 特性用它 |

- 浏览器已经发 zstd：Chrome 123、Firefox 126、Safari 26.3（caniuse / MDN，搜索摘要：[caniuse](https://caniuse.com/zstd)）。所以对 HTTPS 响应，`Content-Encoding: zstd` 会遇到——如果 rurge 不改写 `Accept-Encoding`，要么能解码，要么跳过改写并加备注。
- 正文本来就是整体缓冲的（请求 ≤ 32 MB，响应 ≤ 10 MB / 1 MB），同步解码到 `Vec<u8>` 就够了，不需要异步适配器；未改写的直通路径不解码。
- 解压炸弹防护：mitmproxy 没有；要自己用 `Read::take(limit + 1)` 或按比例限；限制按解压后大小还是压缩前大小，Surge 手册没写（未见）。

### 7.4 "重新压缩"还是"去掉 `Content-Encoding`"

| 做法 | 谁在用 | 代价 |
|---|---|---|
| 解码 → 改写 → 以**未压缩**发出，删 `Content-Encoding`，`Content-Length` = 新长度，删 `Transfer-Encoding: chunked` | hudsucker（删两头不重算）；Surge（（推断）） | 带宽变大；需同时删/改 `ETag`、`Content-MD5` 之类依赖字节的头（Surge 手册只说 "related headers"，未列） |
| 解码 → 改写 → **按原编码重新压缩** | mitmproxy | 需要编码器：flate2（gzip / deflate 有）、`brotli`（有编码器，`brotli-decompressor` 没有）、zstd（ruzstd 编码器等级有限，C 版 `zstd` 完整）；对 `br` 而言重新压缩很慢 |
| 让上游直接发未压缩 | mitmproxy `anticomp` | 改了请求的 `Accept-Encoding`，客户端实际带的头与上游看到的不同 |

---

## 候选 crate 一览

"本机"= 仓库 `Cargo.lock` / 本机 registry 里的版本；"最新"= crates.io API 页摘要（2026-10-29）。

| 名称 | 看到的版本 | 许可 | 用途 | 维护信号 |
|---|---|---|---|---|
| `rcgen` | 本机 0.14.7；最新 0.14.10（2026-08-28） | MIT OR Apache-2.0 | 生成 CA、签发叶证书、CRL | rustls 组织；0.14.x 约每 1 ～ 3 个月一版（0.14.3 = 2025-07 … 0.14.10 = 2026-08）；MSRV 1.71（0.14.7）/ 1.88（0.14.10）；累计 1.3 亿下载。需加 `x509-parser` 特性才有 `Issuer::from_ca_cert_*` |
| `x509-parser` | 本机 0.18.1 | MIT OR Apache-2.0 | rcgen 的 `x509-parser` 特性所需 | 已在依赖树（p12-keystore 带来） |
| `ring` | 本机 0.17.14 | Apache-2.0 AND ISC | 签名、ECDSA / Ed25519 密钥生成；RSA 仅签名 | RSA 私钥 2047 ～ 4096 位、PKCS#8 v1；无 RSA 生成 |
| `p12-keystore` | 本机 0.2.1；最新 0.4.1（2026-10-13） | MIT/Apache-2.0 | 读写 PKCS#12 | 单一作者；2026 年连发 0.3.0-rc3（02-28）…0.3.0（06-07）…0.4.1（10-13）；累计 971 万下载；0.3.0 起 API 破坏性改动，MSRV 1.85 |
| `pkcs5` / `pkcs12` / `cms`（RustCrypto） | 本机 0.7.1 / 0.1.0 / 0.2.3 | Apache-2.0 OR MIT | p12-keystore 的底层 | 本构建中 `pkcs5` 仅 `alloc, pbes2` |
| `rc2` / `des` | 本机 0.8.1 / 0.8.1 | MIT OR Apache-2.0 | PBES1 的 RC2-40 / 3DES | RustCrypto |
| `rustls` | 本机 0.23.43 | Apache-2.0 OR ISC OR MIT | TLS 服务端 / 客户端 | MSRV 1.71 |
| `tokio-rustls` | 本机 0.26.4 | MIT OR Apache-2.0 | `LazyConfigAcceptor`、`TlsConnector` | |
| `rustls-pki-types` | 本机 1.15.1 | MIT OR Apache-2.0 | DER / PEM 类型 | |
| `rustls-webpki` | 本机 0.103.15 | ISC | 证书验证 | |
| `rustls-native-certs` | 本机 0.8.4 | Apache-2.0 OR ISC OR MIT | 系统根 | README 建议改用 platform-verifier；Windows 只读 Current User `ROOT` |
| `rustls-platform-verifier` | 最新 0.7.1（2026-09-24） | MIT OR Apache-2.0 | 用操作系统的证书验证器 | MSRV 1.85；累计 1.99 亿下载；Windows / macOS / iOS 有吊销检查，Linux 没有 |
| `webpki-roots` | 本机 1.0.9 | CDLA-Permissive-2.0 | 编进二进制的 Mozilla 根 | MSRV 1.70 |
| `hudsucker` | 最新 0.25.0（2026-07-15） | MIT OR Apache-2.0 | MITM 代理库（参考实现；带 hyper、moka） | MSRV 1.86；累计 55 万下载；0.24 → 0.25 约两个半月 |
| `fancy-regex` | 本机 0.14.0；最新 0.20.0（2026-10-24） | MIT | 回溯正则（环视、反向引用） | 2025-07 起 0.15 → 0.20 共 6 个次版本；0.19.0 公开类型泛型化（破坏性）；MSRV 1.66 |
| `regex` / `regex-syntax` / `regex-bites` | 本机 1.13.1 / 0.8.11；regex-bites 最新 0.1.6（2025-10-21） | MIT OR Apache-2.0 | 线性正则 / 轻量引擎（jaq 用） | rust-lang |
| `jaq-core` / `jaq-std` / `jaq-json` / `jaq-all` | 最新 3.1.2 / 3.0.4 / 2.0.3 / 0.4.0（2026-10-21 ～ 08-28） | MIT | jq 实现 | 同一作者；3.0.0 = 2026-03-27；累计下载 jaq-core 442 万；`jaq-json` 的 `sync` 特性使 `Val: Send + Sync` |
| `xq` | 最新 0.5.0（2026-02-21） | MIT | 另一个 jq 重实现 | 累计 9.5 万下载；节奏慢 |
| `jq-rs` | 最新 0.4.1（2019-08-17） | Apache-2.0/MIT | libjq 绑定 | 停更 |
| `flate2` | 本机 1.1.10（2026-08-28） | MIT OR Apache-2.0 | gzip / zlib / deflate | MSRV 1.67；累计 7.96 亿下载；本机默认后端 `miniz_oxide` 0.9.1 |
| `brotli` / `brotli-decompressor` | 最新 9.1.1（2026-10-27）/ 6.2.0（2026-10-25） | BSD-3-Clause AND MIT / BSD-3-Clause 或 MIT | br 编解码 / 仅解码 | 近一个月连发多版；累计各约 3.1 亿下载 |
| `ruzstd` | 最新 0.9.1（2026-10-11） | MIT | 纯 Rust zstd 解码 + 简单编码 | MSRV 1.87；README 自称 actively maintained |
| `zstd` | 最新 0.14.3（2026-10-16） | crates.io 摘要写 BSD-3-Clause（未核实） | zstd 的 C 绑定 | （推断）需要 C 编译器；累计 4.5 亿下载 |
| `async-compression` | 最新 0.4.51（2026-10-17） | MIT OR Apache-2.0 | 压缩的异步适配 | MSRV 1.83；几乎每周一版 |
| `lru` | 本机 0.18.4（工作区已有） | MIT | 叶证书缓存的候选 | 已在依赖树 |

---

## 给设计者的开放问题

按"需要谁来决定 / 还缺什么证据"排：

**证书（第 1、2 节）**
1. 叶证书总有效期取多少、notBefore 回拨多少？（事实：Apple 对私有根卡 825 天；Chromium 对"已知根"卡 398 → 200 → 100 → 47 天，Android 系统区的 CA 可能被当成已知根；mitmproxy / Burp 都在 199 天。）续期与缓存 TTL 怎么对上？
2. 叶密钥是每个叶独立生成（ring 的 ECDSA P-256 很便宜），还是像 mitmproxy / hudsucker 那样复用 CA 的密钥？复用就必须显式随机序列号（rcgen 缺省序列号由公钥决定）、叶不写 SKI。
3. `rurge mitm ca generate` 生成什么密钥？ring 只能生成 ECDSA / Ed25519，要生成 RSA 得另找库；ECDSA CA 与 Surge 互通有没有人在真机上试过？（未见）
4. 要不要给 rcgen 打开 `x509-parser` 特性（得到 `Issuer::from_ca_cert_*`）？要不要为 DN 折叠（同类型属性出现两次）这个问题加防护或回退到手工 `CertificateParams`？
5. 发给客户端的链里要不要带 CA 证书？要不要为 Schannel 严格吊销客户端加 CRL 分发点并服务一个空 CRL（rcgen 能签 CRL，但没有 AIA / OCSP 支持）？
6. 目标是 IP 时证书用 IP SAN（hudsucker 没做）；无 SNI 时用 CONNECT / SOCKS5 / fake-IP 得到的主机名。叶证书只放一个名字，还是照抄上游证书的 SAN（mitmproxy）？后者会让客户端合并 HTTP/2 连接，要按每个请求的 `:authority` 分流。
7. 用户 CA 的安装指引覆盖哪些平台与坑：macOS 钥匙串信任设置、iOS 完全信任开关、Windows `certutil`、Linux Chrome 的 NSS 库路径（M146 起变了）、Firefox 在 Linux 不读系统库、Android 7+ 的应用缺省不信用户 CA。

**p12（第 3 节）**
8. 工作区继续用 p12-keystore 0.2.1，还是升到 0.4.1（API 破坏，但支持明文 `keyBag`、无 `localKeyID`、SHA-2 的各种 MAC、迭代数上限）？0.2.1 读不了的格式（PBES2 + HMAC-SHA1 PRF、缺 `localKeyID`、`keyBag`、SHA-384 / 512 MAC）算不算要支持？
9. 写出的 `ca-p12` 缺省用哪种算法？PBES2/AES-256 + SHA-256（现代，macOS 15 起才导得进）还是 3DES + SHA1（老系统通吃）？Surge 自己导出的 p12 是什么算法：我没有样本（未见），最好有人导一个真实的 Surge `.p12` 看 `openssl pkcs12 -info`。
10. 空口令：Surge 要求非空；rurge 对空口令是报错、警告还是放行？

**TLS（第 4 节）**
11. 握手顺序：上游先（mitmproxy 的 `eager`：ALPN 与证书名与上游一致，代价是客户端等上游）还是客户端先（hudsucker：静态 ALPN，之后在 HTTP 层做协议转换）？上游握手失败时是给客户端发警报，还是先完成客户端握手再回 502？
12. 上游验证的根：沿用"native，读到 0 个才退回 webpki-roots"，改成并集，还是换 `rustls-platform-verifier`（解决 Windows 惰性根问题，换来每平台的集成测试）？
13. 证书固定的提示用什么信号：握手期间收到的警报（`unknown_ca` / `bad_certificate` / `certificate_unknown`，但同一警报也可能是"CA 没装"或"有效期过长"）、握手成功后客户端不发请求就关（Surge 手册的做法），还是两者？阈值与日志级别（"进程 / 网络信息只进 DEBUG"的约定要不要扩到主机名）？
14. 上游要求客户端证书（mTLS）时怎么办：不支持（Surge 没有这个键）并提示用户把主机排除出 `hostname`，还是以后扩展？

**正则（第 5 节）**
15. 是否接受"Surge = ICU"这个未证实的前提？接受的话，差异要补到哪一步：替换展开器（NSRegularExpression 模板语义，最小）、`\Q..\E` / `\h` / `\v` / `\<` 改写、多行与行终止符语义、变长后行断言（需升级 fancy-regex）。需要真实社区模块的正则语料来验证，我没有。
16. 回溯上限与 10 MB 响应体：超限算"没匹配"还是"跳过并加备注"？

**jq（第 6 节）**
17. jaq 接受为 jq 子集吗？还缺的信息：Surge 内嵌的 jq 版本、多输出的处理、输出是否缩进、大数字往返、执行预算（jaq 没有内置超时，输出是迭代器，可在输出之间检查期限，但单步计算无法中断）。
18. 在 tokio 里怎么放：`jaq-json` 开 `sync` 特性并验证 `Filter` 的 `Send + Sync`，还是每次在 `spawn_blocking` 里解析编译？

**解码（第 7 节）**
19. 改写后的发出形式：解压后以未压缩发出（Surge 手册的字面意思）并去掉 `Content-Encoding` / 重算 `Content-Length` / 去掉 `Transfer-Encoding`，还是按原编码重新压缩？`ETag` 之类依赖字节的头怎么办？
20. 支持哪些编码：gzip、deflate（zlib + 裸 deflate 探测）、br、zstd（ruzstd 纯 Rust vs `zstd` 的 C 绑定）；多重编码；未知编码时跳过改写并加备注？解压体积上限与"10 MB 限制量的是压缩前还是解压后"。

**范围外但会碰到**：ECH（加密 ClientHello）会让 SNI 不可见；QUIC / HTTP/3（`auto-quic-block`）；HTTP/2 连接合并；这些我没有调研。


