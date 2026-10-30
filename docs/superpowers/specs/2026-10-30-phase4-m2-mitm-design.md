# 阶段 4 / M2「MITM」细化设计

| 项       | 内容 |
| -------- | ---- |
| 日期     | 2026-10-30 |
| 状态     | 草案，已过构建者检查：一个全新的子代理只读本文与代码，提了五件，都已并进正文——RSA 的 CA 签名慢（第 6 节）、虚拟网卡上的名字（第 8.2 节）、上一代的 `Authority` 取不到（第 5.4 节）、EC 私钥与 `Debug` 的指纹与空的来源名单（第 4.3、4.4、5.2 节）、重载不在阻塞线程上（第 5.3 节）。已决事项 M2-D1 ～ D25 是在项目所有者"自己决定、继续做"的常设授权下定的（代项目所有者作的裁定），所有者可以改。执行交给 Pi：三份计划（M2a、M2b、M2c），由协调的会话逐个任务调 Pi 做、逐个任务评审（第 2 节） |
| 上位设计 | [阶段 4 总设计](2026-10-29-phase4-http-engine-design.md) 第 1.4、3.2、3.7、4、5.2、6.1、6.4、6.5、7、9.1、10、12、13、14、17 节；[M1 细化设计](2026-10-29-phase4-m1-session-entry-http-engine-design.md)（本文在 M1 的嗅探与 HTTP/1.1 引擎上加东西，先读它第 20、21 节的订正）。不一致处以本文为准，订正列在第 20 节 |
| 需求     | FR-HTTP-02（`h2 = true` 时两侧的 HTTP/2、`:authority`，M2b）；FR-HTTP-03（`[MITM]` 九个键、按主机名签发并缓存叶证书、靠嗅探到的 SNI 认出的 TLS 连接上不带端口的项不看端口、`client-source-address`、`skip-server-cert-verify`、证书固定的提示、CA 经 `ca-p12` / `ca-keystore-name`、`rurge mitm ca generate`、`GET /v1/mitm/ca`）；FR-HTTP-13 的 `mitm`（`rewrite` 随 M3、`capture` 随 M4）；FR-HTTP-14（`http-api-tls`，M2c）；NFR-03 的"MITM CA 私钥文件只属主可读"（M2c）。FR-HTTP-03 里 QUIC 的那一半与 FR-HTTP-04（`auto-quic-block`）在 M5 |
| 清单     | 1.x 节 L36（`[MITM] # note` 这样的节头，所有者的 Q3）、L54（`[MITM]`）；2.x 节 L166（`http-api-tls`）；7.1 节 L565（HTTPS 默认不透明）；7.2 节 L576 ～ L588（九个键、CA 的生成与使用、证书固定的提示、手册没有的键）；10.x 节 L860（`rurge mitm ca`）；11.x 节 L871（功能开关）、L901（`GET /v1/mitm/ca`）、L904（`http-api-tls`）。行号是 2026-10-30（`10d5925`）的 |
| 前置     | M1（M1a、M1b）做完，本地 `main` = `10d5925`；`origin/main` = `41c35d4`（M1a 与 M1b 的 47 个提交等项目所有者推送，推送之后读那次 CI）。M2 在分支 `phase4-m2` 上，从 `10d5925` 起 |
| 调研     | 总设计的三份调研（`docs/reports/2026-10-29-phase4-*.md`）；手册五页（`http/mitm.html`、`profile/host-list.html`、`profile/keystore.html`、`tools/http-api.html`、`profile/general.html`，2026-10-30 重新取的文本，下文的引文出自它们）；本机 cargo 注册表里锁定版本的源码：rcgen 0.14.7（`src/lib.rs` 的 `Issuer::from_ca_cert_der` 与 `KeyIdMethod::from_x509`：CA 证书有 SKI 时签发者的密钥标识照抄它，没有时取 CA 公钥的 SHA-256 的前 20 字节；`src/certificate.rs` 的 `write_extensions`：只有 `IsCa::Ca` 写 SKI 与 BasicConstraints，AKI 只在 `use_authority_key_identifier_extension` 为真时写；`src/key_pair.rs` 的 `TryFrom<&PrivateKeyDer>`：ring 后端只收 PKCS#8，依次试 Ed25519、P-256、P-384、RSA）、p12-keystore 0.2.1（`src/keystore.rs`：`Pkcs12Writer` 能选 3DES 加密与 HMAC-SHA1 的 MAC）、rustls 0.23.43（`ServerConfig` 是 `Clone`，`session_storage` 是 `Arc<dyn StoresServerSessions>`；对端的警报以 `Error::AlertReceived` 报出）、x509-parser 0.18.1（缺省特性为空，`verify` 特性要 ring）、hyper 1.11.1 的 HTTP/2 两侧（`server::conn::http2::Builder::new(exec)`；`client::conn::http2::SendRequest` 可克隆、有 `is_ready` / `is_closed`；`src/proto/h2/mod.rs` 的 `PipeToSendStream` 在流的窗口为 0 时不再 poll 应答正文，正文出错时重置这个流；`src/proto/h2/server.rs` 里客户端重置流时服务的 future 被丢掉，服务返回错误时以 `INTERNAL_ERROR` 重置这个流） |

M2 让 `[MITM]` 生效。判为 TLS 的可嗅探会话在名单命中、来源允许、有可用的 CA、开关开着时不再按原样转发：rurge 用 `[MITM]` 的 CA 当场为 SNI（没有 SNI 时为连接的目标）签一张叶证书，先与客户端完成 TLS 握手，再把解密之后的每个请求交给 M1b 的 HTTP 引擎——每个请求照旧各自求规则、选策略、借或拨上游连接、各有一条记录，只是上游一侧多一层 TLS、URL 以 `https://` 开头、协议是 `HTTPS`。解密之后，`URL-REGEX`、`USER-AGENT` 等 HTTP 类规则对 HTTPS 请求第一次生效，M3 的重写与 M4 的抓包也建在这上面。

M2 还做：`[MITM]` 的九个键类型化（`auto-quic-block` 只读进来，效果在 M5）；`mitm` 开关生效并持久化（`state.json` 第 2 版）；`GET /v1/mitm/ca`；握手失败与证书固定的提示（日志、记录、计数）；`h2 = true` 时两侧的 HTTP/2（M2b）；`rurge mitm ca generate` / `export` 与 `http-api-tls`（M2c）。

**M2 几乎没有平台代码**：改的是 `rurge-config`、`rurge-proto`、`rurge-net`、`rurge-http`、`rurge-engine`、`rurge-api` 与可执行文件里不带 `cfg` 的部分。唯一的例外是 M2c 写私钥文件时 `#[cfg(unix)]` 的那一支（用标准库设 `0600`，同 `crates/rurge-dns/src/fakeip/file.rs`），开发机上编译不到，只有 CI 编译——M2 做完之后请所有者推送一次（连同 M1），读回那次 CI。

## 1. 范围

| 做 | 不做（去向） |
| -- | ------------ |
| `[MITM]` 九个键类型化：两张主机名单、生效的名单、来源名单、CA 的三个键、`skip-server-cert-verify`、`h2`、`auto-quic-block`；从 `W0016` 的名单里拿掉（第 4 节，M2a） | `auto-quic-block` 的效果（M5）；`[MITM] # note` 这样的节头怎样读（所有者的 Q3，本阶段不改节头的解析） |
| CA：`ca-keystore-name` 优先、否则 `ca-p12` 与 `ca-passphrase`；解码、检查、出问题时告警且 MITM 不生效；`rurge check` 看得到（第 5 节，M2a） | `p12-keystore` 升到 0.4.x（所有者的 Q4，M2 留在 0.2.1，第 20 节第 1 条）；把 CA 装进系统的信任库（所有者的 Q5） |
| 叶证书的签发、缓存与 TLS 的会话缓存（第 6 节，M2a） | — |
| 何时解密：开关、CA、来源、名单（SNI 或连接的目标）、ALPN；可嗅探的会话随名单扩大（第 7 节，M2a） | 不可嗅探的端口上靠 SNI 解密（不做：只有可嗅探的会话嗅探，第 7.6 节登记） |
| 与客户端的握手、失败的分类、证书固定的提示、承载连接的记录、限频、计数（第 8 节，M2a） | 自动把疑似证书固定的主机排除出名单（不做，总设计第 7.4 节） |
| 解密之后的请求：`https://` 的 URL、`HTTPS`、对上游的 TLS 与校验、失败的 502、升级经 TLS（第 9 节，M2a） | 上游要客户端证书（`[MITM]` 没有这个键，清单 L588） |
| 两侧的 HTTP/2 与版本转换、`:authority`、HTTP/2 上的终止与看门狗（第 10 节，M2b） | 宣告扩展 `CONNECT`（RFC 8441，不做，总设计 P4-D11）；明文 HTTP/2 先验（不做，同 M1） |
| `mitm` 开关生效与持久化，`state.json` 第 2 版（第 11 节，M2a） | `rewrite`、`capture` 生效（M3、M4；在那之前照旧读作关、`POST` 答 501） |
| `GET /v1/mitm/ca`；`/v1/rurge/http` 加解密的四个数（第 12 节，M2a） | — |
| `rurge mitm ca generate` / `export`（第 13 节，M2c） | 自动安装（Q5） |
| `http-api-tls` 与命令行的 HTTPS（第 14 节，M2c） | `--remote` 指向开了 `http-api-tls` 的实例（不做，第 14.3 节登记） |
| 手工清单 `docs/acceptance/phase4-manual.md` 新建，含 M1 与 M2 的行为（M2c） | 手工验收本身（归所有者） |

## 2. 计划与执行

三份计划，依次做。后一份在前一份合并之后开工，它的计划也在那时对着合并之后的代码写（免得计划之间漂移，同 M1）：

- `docs/superpowers/plans/2026-10-30-phase4-m2a-mitm-http1-plan.md`
- M2b（HTTP/2）与 M2c（CA 工具、`http-api-tls` 与收尾）的计划日后写，文件名同样的格式。

M2a 的任务：

| 任务 | 内容 |
| ---- | ---- |
| 0 | 开工前核对：第 19 节的各项对着当时的代码核一遍，出入记进「执行期修正记录」；量一次基线 |
| 1 | `rurge-config`：`[MITM]` 类型化（`MitmConfig`、`SourceList`）、`HostList::matches_sni` 与按文字去掉的生效名单、从 `W0016` 的名单拿掉、`Debug` 与 `secrets.rs`、语料库快照；诊断码 `W0039`（第 4 节） |
| 2 | `rurge-proto`：`decode_pkcs12`、`origin_client_config`；`rurge-net`：`union_root_store`；`rurge-http`：CA 的载入与检查（`Authority`）、`mitm::check`；引擎的 `load_checked` 接上，不在阻塞线程上调它的地方（已知重载一处）挪进 `spawn_blocking`（第 5、9.3 节） |
| 3 | `rurge-http`：叶证书与缓存、服务端配置的模板、与客户端的握手和失败的分类（第 6、8.1、8.2 节） |
| 4 | `rurge-http`：解密的连接上的请求——`Conn` 的上游 TLS、`https://` 的 URL 与 `HTTPS`、对上游的握手与校验、失败的答复、升级经 TLS（第 9 节） |
| 5 | 引擎：每一代的 `Mitm`（`EngineShared` 记最后一个生效过的 `Authority`，同样的材料沿用它）、开关与 `state.json` 第 2 版、启动时还原、可嗅探的扩大、解密的分派、承载连接的记录、提示与限频、计数（第 7、8.3 ～ 8.5、11 节） |
| 6 | API：`/v1/features/mitm` 生效、`GET /v1/mitm/ca`、`/v1/rurge/http` 的 `mitm`（第 12 节） |
| 7 | 端到端用例（第 17 节第 3 条） |
| 8 | 文档收尾（M2a 的部分，第 18 节） |

M2b 的任务（计划日后写，这里是大纲）：核对；hyper 的 HTTP/2 执行器与服务端；上游的 HTTP/2 与池里共享的连接；版本转换与 `:authority`；HTTP/2 上的终止、看门狗、拒绝与停止；升级只走 HTTP/1.1；四种组合的端到端用例；文档。

M2c 的任务（同上）：核对；`rurge mitm ca generate` / `export`；`http-api-tls` 的服务端；命令行的 HTTPS；`W0040`；手工清单；M2 收尾的文档（`docs/status.md`、清单、两份 README、需求文档第 7 节的状态一句、`CLAUDE.md`）。

**执行的约定**（三份计划开头重申，同 M1）：协调的会话逐个任务开 Pi 的会话、把任务的全文与要用的接口交给它；Pi 做完写报告、提交；协调者读改动、跑门禁、评审，有问题回到同一个 Pi 会话修正，再评审，过了才派下一个任务。每个任务先写失败的用例、看到红，再实现到绿；门禁是 `CLAUDE.md` 的全量门禁；提交说明的标题用中文。计划没有写到、要做决定的，Pi 停下来报告，不自己定。

做完 M2c 之后：项目所有者推送（M1 与 M2 一起），读回那次 CI 的注解与任务摘要（M2c 有只在 CI 上编译的 `#[cfg(unix)]` 分支）；再往后是 M3（重写与 Map Local）。M2a、M2b 之后不单独停下来等推送：没有平台代码。

## 3. crate 与接口

**不加新包、不下载**：M2 用到的 rcgen 0.14.7、x509-parser 0.18.1、time 0.3.45、lru 0.18、rustls 0.23.43、tokio-rustls 0.26.4、p12-keystore 0.2.1、webpki-roots 1.0.9 都已在 `Cargo.lock` 里、源码在本机注册表里。`rurge-http` 新增正式依赖 rcgen（开 `x509-parser` 特性，ring 后端会顺带打开 `x509-parser/verify`）、x509-parser、time、lru、rustls、tokio-rustls；工作区的依赖表加 `x509-parser = "0.18"` 与 `time = "0.3"` 两行（今天是传递依赖）。`Cargo.lock` 的包数应当不变（今天 520 个），任务 1、2 各核一次。依赖方向不加新的工作区边：`rurge-http → rurge-proto` 与 `rurge-engine → rurge-http` 在 M1 就有；M2c 加 `rurge`（可执行文件）`→ rurge-http`（`generate` 在 `rurge-http` 里，第 13 节）。

| crate | 改动 |
| ----- | ---- |
| `rurge-config` | 新 `mitm.rs`：`MitmConfig`、`SourceList`；`hostlist.rs`：`HostList::matches_sni`、`HostList::without`；`deferred.rs` 去掉 `MITM`；`config.rs`：`Config.mitm`；`diagnostic.rs`：`W0039`（M2a）、`W0040`（M2c） |
| `rurge-proto` | `keystore::decode_pkcs12(label, base64, password)`（`decode_p12` 改为调它）；`transport::tls::origin_client_config(roots, insecure, alpn)`：用模块私有的 `Verifier` 建解密的上游的客户端配置，校验器本身照旧私有 |
| `rurge-net` | `tls::union_root_store()`：系统的根 ∪ `webpki-roots`，进程里读一次 |
| `rurge-http` | 新 `mitm` 模块：`Authority`（CA、叶证书缓存、服务端的会话缓存）、`Mitm`（一代配置里的名单、来源、开关以外的条件、两份 TLS 配置）、`check`、与客户端的握手；`engine.rs`：`Conn` 加上游的 TLS、`https://` 的 URL、`HTTPS`、`connect()` 里对上游的握手（M2a）；HTTP/2 两侧（M2b）；`generate`（M2c） |
| `rurge-engine` | `Runtime.mitm`；`EngineShared` 的开关、提示的限频与最后一个生效过的 `Authority`（第 5.4 节）；`sniffable_session` 的扩大；`serve_sniffed` 判为 TLS 的一支先问要不要解密；`serve_mitm`；`load_checked` 加 `rurge_http::mitm::check`；`state.rs` 第 2 版；`Engine::switch` / `set_switch` / `restore_switches`；`Engine::mitm_ca_der`；计数（M2a） |
| `rurge-api` | `/v1/features/mitm` 生效；`GET /v1/mitm/ca`；`/v1/rurge/http` 的 `mitm`（M2a）；`http-api-tls` 的监听器（M2c） |
| `rurge`（可执行文件） | 启动时还原开关；重载时的 `load_checked` 挪进 `spawn_blocking`（第 5.3 节）（M2a）；`rurge mitm ca generate` / `export`、API 客户端的 HTTPS（M2c） |

接口的形状（细节以计划为准）：

```rust
// rurge-config（M2a）
pub struct MitmConfig {
    pub hostname: HostList,            // 缺省端口 443
    pub hostname_disabled: HostList,   // 缺省端口 443
    pub effective: HostList,           // hostname 去掉 hostname_disabled 里文字相同的项（第 4.2 节）
    pub ca_p12: Option<String>,        // Debug 里藏起来
    pub ca_passphrase: Option<String>, // Debug 里藏起来
    pub ca_keystore_name: Option<String>,
    pub skip_server_cert_verify: bool, // 缺省 false
    pub h2: bool,                      // 缺省 false；M2b 起生效
    pub client_source_address: Option<SourceList>, // None：不写等于全部
    pub auto_quic_block: bool,         // 缺省 true；M5 起生效
    pub spans: MitmSpans,              // 各键所在的行，诊断用
}
impl HostList {
    /// 同 `matches`，只是不带端口的项不看端口（有 SNI 时，第 7.3 节）
    pub fn matches_sni(&self, host: &HostName, port: u16) -> Option<bool>;
}

// rurge-http（M2a）
pub struct Authority { /* CA 的证书链与签名密钥、叶证书缓存、服务端的会话缓存、材料的指纹 */ }
impl Authority {
    /// None：`[MITM]` 没有写 CA；Err：写了但不能用（第 5.2 节，消息里没有凭据）
    pub fn load(cfg: &MitmConfig, keystore: &[KeystoreItem]) -> Result<Option<Authority>, CaProblem>;
    pub fn same_material(&self, cfg: &MitmConfig, keystore: &[KeystoreItem]) -> bool; // 第 5.4 节
    /// 第 6 节：签名在阻塞的线程上做，同一个名字同时只签一次
    pub async fn leaf(&self, name: &HostName) -> Result<Arc<CertifiedKey>, LeafError>;
    pub fn ca_der(&self) -> &[u8];
}
pub struct Mitm { /* Arc<Authority>、生效的名单、来源名单、h2、对客户端与对上游的 TLS 配置 */ }
impl Mitm {
    pub fn build(cfg: &MitmConfig, authority: Arc<Authority>, roots: Arc<RootCertStore>) -> Result<Mitm, CaProblem>;
    pub fn decides(&self, facts: &TlsFacts) -> Decision; // 第 7 节，开关由引擎先问
    pub fn widens(&self, src: Source, host: &HostName, port: u16) -> bool; // 第 7.6 节
    pub async fn accept(&self, client: Prefixed<BoxedStream>, name: &HostName, wait: Duration)
        -> Result<Accepted, HandshakeFailure>; // 第 8 节
}
pub fn check(config: &Config) -> Diagnostics; // W0039，加载时（第 5.3 节）
```

`Conn`（M1b）加一个字段 `origin: Option<OriginTls>`：`Some` 表示这是解密的连接，请求的 URL 用 `https://`、协议是 `HTTPS`、`connect()` 在拨好的流上先做对上游的 TLS 握手（第 9 节）。`OriginTls` 带对上游的客户端配置、SNI 与校验用的名字、握手的时限。

## 4. `[MITM]` 的读法（M2a）

### 4.1 九个键

`[MITM]` 是键值节（`crates/rurge-config/src/text/mod.rs` 的 `KEY_VALUE_SECTIONS` 里已有），照 `[General]` 的写法读：取第一个 `[MITM]` 节（`Profile::section`），一行一个键，同一个键写了两次以后写的为准，键名不分大小写。

| 键 | 类型与缺省 | 读不懂时 |
| -- | ---------- | -------- |
| `hostname` | Host List，缺省端口 443 | 非法项 `W0005`，忽略那一项（手册："An invalid entry in the list produces a profile warning instead of a parse error"） |
| `hostname-disabled` | Host List，缺省端口 443 | 同上 |
| `ca-p12` | 字符串（Base64 的 PKCS#12） | 不在这里检查，见第 5 节 |
| `ca-passphrase` | 字符串；可以为空（总设计第 2.3 节 3.23：Surge 不收空口令，rurge 收，登记） | — |
| `ca-keystore-name` | `[Keystore]` 里 p12 条目的名字 | 不在这里检查，见第 5 节 |
| `skip-server-cert-verify` | 布尔，缺省 false | `W0012`，取缺省 |
| `h2` | 布尔，缺省 false（M2b 起生效；M2a 读进来、不告警） | `W0012`，取缺省 |
| `client-source-address` | 来源名单（第 4.3 节）；不写等于全部 | 非法项 `W0005`，忽略那一项 |
| `auto-quic-block` | 布尔，缺省 true（M5 起生效；在那之前读进来、不告警：缺省就是真，写它的人多半只是照抄示例） | `W0012`，取缺省 |

别的键 `W0001`（未知键），包括常见误写的 `tcp-connection`（`CLAUDE.md`「事实来源」：`[MITM]` 没有这个键；清单 L588）。

### 4.2 生效的名单

手册："Entries in this list are removed from the effective `hostname` list." 读法（M2-D3）：`hostname` 的一项，在 `hostname-disabled` 里有一项与它**文字相同**时被去掉；文字相同指 `-` 的有无相同、`-` 后面的文字去掉首尾空白之后不分大小写相同（`*.Example.com` 与 `*.example.com` 相同，`example.com` 与 `example.com:443` 不同）。不按"匹配的范围"去掉：`hostname-disabled` 是 Surge 的界面用来"暂时停用某一项"的（手册），它写的就是 `hostname` 里那一项的原文。`hostname-disabled` 里有、`hostname` 里没有的项不告警。生效的名单保留 `hostname` 的次序（"Items are matched in order"）。

### 4.3 来源名单

手册：逗号分隔；单个 IP 或 CIDR，IPv4 与 IPv6；`-` 开头排除；不写等于 `0.0.0.0/0, ::/0`；Mac 6.1.0 起可以写 MAC 地址。读法：

- 每项去掉首尾空白，`-` 开头的是排除；剩下的先当 IP 或 CIDR 读（IP 读成 `/32` 或 `/128`），再当 MAC 读（六组两位十六进制，`:` 或 `-` 分隔，不分大小写）；都不是的 `W0005`、忽略。
- 名单按顺序匹配，第一项决定（同 Host List）；一项也不命中时**不允许**（写了名单就是只对名单里的来源）。
- 值为空（去掉空白之后什么也没有）的按没写算，不告警：否则 `client-source-address =` 这样一行会让 MITM 对所有来源悄悄不生效（构建者检查提出）。名单里的项都非法时不这样算——那时有逐项的 `W0005`，名单是空的，谁也不允许。
- MAC 的项读进来，但**在阶段 7 之前不命中任何来源**（rurge 还不知道来源设备的 MAC，清单 L583 的"4 / 7"）：有这样的项时整个键一条 `W0029`（"MAC address entries match no client in this version"），不逐项告警。

### 4.4 凭据

`ca-p12` 与 `ca-passphrase` 早就在 `redact.rs` 的 `SECRET_KEYS` 里（`GET /v1/profiles/current` 的脱敏视图藏它们），`[Keystore]` 的 p12 条目也早就藏着。M2a 新的是类型化之后的 `MitmConfig`：它的 `Debug` 只写这两个键有没有、不写值（同 `KeystoreItem` 的写法）；`crates/rurge-config/tests/secrets.rs` 的配置里已有 `[MITM]` 的两处标记（第 108、109 行），用例加断言：`Config` 的 `Debug` 里没有它们。CA 的私钥只在 `Authority` 里（第 5 节），`Authority` 的 `Debug` 只写 CA 证书的 subject 与**证书**（DER）的 SHA-256 指纹的前几位。第 5.4 节的材料指纹（由 Base64 与口令算出）不进 `Debug`、日志与 API：它的前缀能让人不经 PKCS#12 的密钥派生就离线验证对口令的猜测（构建者检查提出）。

## 5. CA（M2a）

### 5.1 来源

- `ca-keystore-name` 写了：用 `[Keystore]` 里那个名字的条目，**不回落到 `ca-p12`**（手册："When set, it takes precedence over `ca-p12`."）。没有那个条目、或它不是 p12 类型（`type` 省略时有 `password` 的算 p12，手册），是"出问题"（第 5.2 节），**不是** `E0020`：总设计把 CA 的一切问题都定成告警、MITM 不生效、配置的其余部分照常（M2-D4）。
- 否则 `ca-p12` 与 `ca-passphrase`（没写口令按空口令）。
- 都没写：没有 CA。这时 `hostname` 的生效名单不空就告警一次（"`[MITM]` lists hosts to decrypt but has no CA"），名单空着就什么也不说。
- 解码：`rurge_proto::keystore::decode_pkcs12(label, base64, password)`——今天的 `decode_p12` 拆出来的一段，`decode_p12` 改为调它；`label` 是写进错误消息的名字（`[Keystore]` 条目的名字，或 `ca-p12`）。得到证书链（与私钥配对的那张在前）与 PKCS#8 的私钥。仍用 `p12-keystore` 0.2.1（第 20 节第 1 条）。

### 5.2 检查

下面任一条不成立就是"出问题"：一条 `W0039`（消息以 "MITM is off: " 开头，写明是哪一种），这一代没有 `Mitm`，MITM 不生效，配置的其余部分照常。消息里没有 Base64、口令与密钥。

1. Base64 解得开（`ca-p12` 或 `[Keystore]` 条目的 `base64`；后者在 `[Keystore]` 本身读的时候已经由 `E0021` 管，这里不重复）。
2. PKCS#12 解得开（口令对、格式 0.2.1 认得）；里面有私钥与配对的证书。
3. 那张证书是 CA：BasicConstraints 的 `cA` 为真；写了 KeyUsage 的，含 `keyCertSign`。
4. 现在在它的有效期里（`notBefore ≤ 现在 < notAfter`）。
5. 私钥 rcgen 的 ring 后端签得了：PKCS#8 的 ECDSA P-256 / P-384、2048 ～ 8192 位的 RSA、Ed25519（`KeyPair::try_from`）。P-521、DSA、PKCS#1 形式的不行。**EC 私钥的 PKCS#8 里没有带公钥的也不行**：RFC 5915 说公钥可以省，ring 0.17.14 却要它在（`src/ec/suite_b.rs` 第 199、200 行："The RFC says it is optional, but we require it to be present"；rcgen 0.14.7 经 `EcdsaKeyPair::from_pkcs8` 走到这里，构建者检查查出）。不另外补上公钥（那要改写 DER），登记（第 16 节）；消息带 ring 给的原因，使用者能照着用 OpenSSL 重新导出。
6. **试签一张**：用这把钥匙、经 `Issuer::from_ca_cert_der` 为保留名 `rurge-ca-check.invalid` 签一张叶证书（第 6 节的参数），再用 rustls 的 `WebPkiServerVerifier`、根只有这张 CA，按这个名字校验它。不过就是出问题，消息带校验器给的原因。这一步一次查掉几种难逐个列举的坏法：CA 的 subject 里有多值的 RDN 或重复的属性类型时 rcgen 重建出的签发者名字与 subject 不再逐字节相同（总设计第 7.2 节），证书与私钥不配对，签名算法 webpki 不认。

Ed25519 的 CA 过得了这几条，但多数浏览器不认 Ed25519 签的证书：不当作出问题，登记（第 16 节）。

### 5.3 在哪里检查

`rurge_http::mitm::check(&Config) -> Diagnostics` 做第 5.1、5.2 节的全部，接进引擎的 `load_checked`（"`run`、重载、`rurge check` 与 `POST /v1/profiles/check` 读配置的那一个办法"，`crates/rurge-engine/src/outbounds.rs`）：命令行的检查与 API 的检查都看得到 `W0039`。PKCS#12 的解码要做 PBKDF 的迭代，次数由文件定、没有上限，所以不能占异步的工作线程，`load_checked` 要在阻塞的线程上调。启动（`profile/boot.rs`）是在阻塞线程上调的。**重载不是**：`crates/rurge/src/cli/run.rs` 的 `reload`（今天第 487 行起）是异步函数，直接调它（构建者检查查出）。M2a 把这一处挪进 `spawn_blocking`。其余的调用处由任务 0 逐个核（第 19 节 V14），不在阻塞线程上的一并挪。

每一代配置建 `Runtime` 时（第 5.4 节），材料变了才再解一次。两次之间不传对象：`load_checked` 的结果是 `rurge-config` 的类型，装不下 `Authority`。`Runtime::build` 里的解码同样放进 `spawn_blocking`。

### 5.4 每一代与"同样的材料"

`Authority` 带一个材料的指纹：所用的 Base64 文字与口令（中间隔一个分隔字节）的 SHA-256，只用于比较，不进 `Debug`、日志与 API（第 4.4 节）。

`EngineShared` 里记着**最后一个生效过的 `Authority`**（M2-D5）：
- 一代配置换上时（`Engine::swap_runtime`），它有 `Mitm` 就记下那个 `Arc<Authority>`；没有就不动，记着的那个留着。
- 建新的一代时（`Runtime::build`），先算新配置的材料指纹。与记着的那个相同就**沿用同一个 `Arc<Authority>`**，叶证书缓存与服务端的会话缓存跟着它；不同就解码新建（总设计："条目变了的重载换 CA，已缓存的叶证书作废"）。
- 中间隔着一代没有可用的 CA、之后又改回同样的材料，照样沿用。
- 只在换上时记，不在建时记：建好而没有换上的一代（重载在后面失败了）不会留下它的 `Authority`。
- `http-api-tls` 用的"最后一个可用的 `Authority`"也是这一个（第 14.1 节）。

`Runtime::build` 里的 `opts.shared.cell.load()` 取到的是上一代的策略注册表（`RegistryCell`，在 `rurge-policy` 里），不是上一代的 `Runtime`，拿不到 `Authority`（构建者检查查出），所以要另记在 `EngineShared` 里。

`Mitm`（名单、来源、`h2`、`skip-server-cert-verify`、两份 TLS 配置）每一代都新建：这些重载能改，而且建起来便宜。已经解密的连接握着自己那一代的 `Mitm` 与叶证书，重载不打断它们。

### 5.5 发给客户端的链

叶证书 + PKCS#12 里与私钥配对的那张起、`private_key_chain` 给出的整条链（自签的根 CA 时就是叶 + CA；使用者的 MITM CA 是某个根签的中间 CA 时，客户端也能建链）。

## 6. 叶证书（M2a）

| 项 | 取值 |
| -- | ---- |
| 密钥 | 每张各自新生成的 ECDSA P-256（总设计 P4-D8） |
| 签名算法 | 随 CA 的密钥：P-256 → ECDSA-SHA256，P-384 → ECDSA-SHA384，RSA → RSA-PKCS1-SHA256，Ed25519 → Ed25519 |
| 序列号 | 16 个随机字节，最高位清零 |
| subject | CN = 名字（64 字符以内时；超过时不写 CN） |
| SAN | 名字是域名时一个 DNS 名，是 IP 时一个 IP 地址 |
| 扩展 | EKU serverAuth；KeyUsage digitalSignature；**AKI 只在 CA 证书有 SKI 时写**（照抄那个 SKI；没有 SKI 时 rcgen 会按 CA 公钥算一个，与谁也对不上，Windows 的建链按 AKI 找签发者，宁可不写，M2-D6）；叶不写 SKI、不写 BasicConstraints |
| 有效期 | `notBefore` = 现在回拨 2 天与 CA 的 `notBefore` 两者中晚的那个（叶证书的有效期不早于 CA 的，免得某些平台上嵌套检查不过）；`notAfter` = `notBefore` + 47 天与 CA 的 `notAfter` 两者中早的那个 |

**名字**是 SNI；没有 SNI 时是连接的目标（`CONNECT` 的主机、SOCKS5 的域名、fake 地址反查到的域名，都没有时是 IP，总设计第 2.3 节 3.8）。SNI 写成 IP 的（不合 RFC 6066，但有客户端这样做）照 IP 签。

**签名不在异步的工作线程上做**（M2-D7，构建者检查提出）。签一张叶证书的大头是用 CA 的私钥签名：
- P-256 的 CA 签一次在几十微秒的量级；
- RSA 的 CA 在毫秒的量级，4096 位以上的要几毫秒到几十毫秒。

这些是量级的估计，没有在这台机器上量过。从 Surge 迁来的 CA 多半是 RSA 的（推断，没有核实）。所以签名一律放进 `spawn_blocking`，不按密钥类型分；叶证书自己的 P-256 密钥在同一个阻塞任务里生成。

**同一个名字同时只签一次**。缓存里每个名字一个槽（`tokio::sync::OnceCell` 一类）：先到的连接签，同时到的等它的结果。浏览器第一次访问一个主机时会同时开好几条连接，所以同时到是常态，不是偶发的竞争。签失败时槽留空，这条连接的握手按"别的失败"算（第 8.2 节，原因写 "cannot issue a certificate for …"），下一个连接再试。

**缓存**按名字（不含端口）放在 `Authority` 里，是一个 LRU（`lru`，1000 个槽）。锁只在取槽、换槽时持有，签名时不持有。取的时候槽里的叶证书剩余寿命不足总长的三分之一，就换一个空槽、重签；已经拿到旧槽的连接照用旧的那张，它还没有过期。

**TLS 的会话恢复**：`Authority` 里一份 `ServerSessionMemoryCache`（1024 条，rustls 缺省每份配置 256 条），这个 `Authority` 的每条解密连接的服务端配置共用它（第 8.1 节）；恢复要求 SNI 相同（rustls 的做法）。

## 7. 何时解密（M2a）

### 7.1 条件

一个判为 TLS 的可嗅探会话，下面几条依次都成立才解密（总设计第 7.1 节，M2-D8）：

1. `mitm` 开关开着（第 11 节）；
2. 这一代有 `Mitm`（`[MITM]` 有可用的 CA，第 5 节）；
3. 来源允许（第 7.2 节）；
4. 名单命中（第 7.3 节）；
5. ALPN 合得上（第 7.4 节）。

不解密的照 M1 按原样转发、带着 SNI 与 `HTTPS` 求规则。只有第 5 条不成立时写一行 DEBUG（名字、客户端给的 ALPN）：那是名单命中了却解不了，使用者可能会奇怪；别的情形是绝大多数 TLS 会话的常态，不写。条件取会话判为 TLS 那一刻在用的那一代配置与开关。

### 7.2 来源

- 经 HTTP 与 SOCKS5 监听器来的：来源地址是回环地址（`127.0.0.0/8`、`::1`）的**一律按 `127.0.0.1` 比**，别的按它自己的地址比（IPv4 映射的 IPv6 地址先还原成 IPv4）。
- 经虚拟网卡来的：一律按 `127.0.0.1` 比——阶段 3 的虚拟网卡只接本机的流量（网关模式在阶段 7）。
- 照手册"`127.0.0.1` should be included if you want to enable MITM for the current device"（总设计第 2.3 节 3.9）。写了 `::1` 的项因此不命中本机的会话（登记）；经本机的局域网地址连到监听器的本机应用按那个局域网地址比（登记）。

### 7.3 名单

- **有 SNI**：拿 SNI 比生效的名单，用 `HostList::matches_sni`：同 `matches`（第一个命中的项决定，`-` 的项排除），只是**不带端口的项不看端口**；写了 `:端口` 的项照旧要端口相同，`:0` 的项不看端口。照手册"For TLS or QUIC connections identified by the sniffed SNI, a bare entry matches regardless of the connection's actual port."
- **没有 SNI**：拿连接的目标比，用今天的 `matches`：不带端口的项只对 443（缺省端口）。`<ip-address>` 一类的项照 IP 比。
- 有 SNI 时只比 SNI，不再比连接的目标（总设计第 7.1 节第 4 条）。

### 7.4 ALPN

客户端没给 ALPN，或给的里面有 `http/1.1`：合得上，对客户端选 `http/1.1`（没给时不选）。只给了 `h2`（或别的）：M2a 合不上；**M2b 起**照总设计：含 `h2` 且 `h2 = true` 也合得上。rustls 的服务端在客户端给了 ALPN 而一个也对不上时会拒绝握手，所以合不上的不解密（按原样转发），不是解密之后失败。

### 7.5 `always-raw-tcp-*` 与 `force-http-engine-hosts`

`always-raw-tcp-*` 命中的会话不嗅探，因此不解密（总设计第 2.3 节 3.12）。`force-http-engine-hosts` 强制进来、判为 TLS 的会话同样先问第 7.1 节：解密就解密，不解密就照 M1 按原样转发。

### 7.6 可嗅探的会话随名单扩大

M1 的可嗅探的条件（`crates/rurge-engine/src/serve.rs` 的 `sniffable_session`）里端口那一条是"80、443 或命中 `force-http-engine-hosts`"。M2 再加一种（M2-D9）：**`mitm` 开关开着、这一代有 `Mitm`、来源允许（第 7.2 节），并且**下面任一条成立：

- 端口是生效名单里某一个**非排除**项写明的端口（`example.com:8443` 让所有目标的 8443 都可嗅探——经虚拟网卡来、目标是 IP 的会话只有嗅探了才看得到 SNI）；
- 连接的目标按生效名单（`matches`，缺省端口 443）命中——`:0` 的项由此只让**它命中的目标**的全部端口可嗅探。

这订正了总设计第 5.2 节"`:0` 的项让全部端口都算"（第 20 节第 2 条）：照那样写，名单里只要有一项 `example.com:0`，所有主机的所有端口都要嗅探，服务器先说话的协议（SMTP、IMAP、MySQL 等）在任何端口上都要晚 15 秒才通。`*:0` 照样让全部端口可嗅探——那是使用者明说的。

**不带端口的项不让别的端口可嗅探**：手册说 `example.com` 这一项对非标准端口上看得见 SNI 的 TLS 也解密；rurge 只嗅探可嗅探的会话（总设计第 5.2 节），8443 上没有写 `:8443` 或 `:0` 的连接不嗅探、看不到 SNI、不解密。登记（第 16 节）；要解密非标准端口就写明端口或 `:0`。

## 8. 与客户端的握手（M2a）

### 8.1 服务端配置

`Mitm` 建一份模板（`rustls::ServerConfig`，ring 的密码套件，TLS 1.2 与 1.3，不要客户端证书，不收 0-RTT，ALPN 照第 7.4 节——M2a 只有 `http/1.1`，M2b 起 `h2 = true` 时是 `h2`、`http/1.1`），`session_storage` 指向 `Authority` 的会话缓存。每条解密的连接克隆一份模板，换上一个只给出这条连接的叶证书的 `cert_resolver`（`ServerConfig` 是 `Clone`；克隆共享会话缓存的 `Arc`）。不用"按 SNI 选证书"的共用解析器：没有 SNI 的连接要按连接的目标签，解析器看不到目标。

### 8.2 握手

- 用 tokio-rustls 的 `TlsAcceptor` 在 `Prefixed<BoxedStream>` 上握手：嗅探读过的 ClientHello 还在前缀里，rustls 从头读到它（总设计第 3.2 节，不用 `LazyConfigAcceptor`）。
- 时限 30 秒（引擎的 `HANDSHAKE_TIMEOUT`，监听器的握手用的同一个值）；与停止竞速，停止时不计数、不记录、不写日志（同嗅探被停止）。
- 失败的分类（M2-D11）：

| 情形 | 日志 | 计数 | 承载连接的记录 |
| ---- | ---- | ---- | -------------- |
| 客户端发来 `unknown_ca`、`bad_certificate` 或 `certificate_unknown` 警报 | WARN "the client does not trust the certificate rurge issued for `<名字>`"，提示在客户端装上并信任 MITM 的 CA，或在 `hostname` 里写 `-<名字>` 排除它（这几种警报分不清"没装 CA"与"证书固定"，总设计第 7.4 节）；限频（第 8.4 节） | `untrusted` | `Failed`，原因写警报的名字 |
| 握手中途客户端断开、超时、别的警报与协议错误 | INFO 一行，带名字与原因 | `failed` | `Failed`，原因 |

- 握手成功：`decrypted` 加一，连接交给 HTTP 引擎（第 9 节）。
- **经虚拟网卡来的会话**（`ListenerKind::Tun`）：本节与第 8.3 节的 WARN / INFO 行不带名字与端口，写成 "… for a host reached through the virtual interface (the name is in the DEBUG log and the request record)"；名字另起一行 DEBUG。照 `CLAUDE.md`「日志的分级」与阶段 3 的 M3-D26：虚拟网卡上的名字与地址只在 DEBUG 与请求记录里（构建者检查提出）。限频照旧按名字算（第 8.4 节），记录照旧带名字。经 HTTP 与 SOCKS5 监听器来的会话照上表带名字。

### 8.3 证书固定的提示

手册："If a client completes the TLS handshake but disconnects without sending a request, Surge logs a hint that the host is likely protected by certificate pinning." 条件（M2-D11）：

1. 与客户端的握手完成了；
2. 从客户端读到的**明文字节数是 0**（包在 TLS 流外面数，第 19 节 V9）；
3. 连接是因为客户端关闭（读到 `close_notify`、EOF 或连接被重置）而结束的——不是请求头的时限到了、不是停止、不是终止。

成立时：WARN "`<名字>` is likely protected by certificate pinning: the client finished the TLS handshake and closed without a request; exclude it with `-<名字>` in `[MITM] hostname`"（限频）；`noRequest` 加一；承载连接一条记录，`Failed`，原因写 "no request after the TLS handshake (likely certificate pinning)"。

不自动把这个主机排除（总设计第 7.4 节）。浏览器的预连接（握手之后没用上就关掉）也满足这几条，会触发这条提示——手册的写法是"likely"，Surge 想必一样；限频让它不刷屏，登记（第 16 节）。

### 8.4 限频

第 8.2 节的 WARN 与第 8.3 节的提示**每种、每个名字一小时至多一行 WARN**，其余照样写，但在 DEBUG（M2-D11）。记在 `EngineShared` 里（活得比一代配置长）：名字到上次写 WARN 的时刻，至多 1024 个名字，满了整个清空。计数与记录不限。

### 8.5 计数

`/v1/rurge/http` 加一个对象 `mitm`（第 12.3 节）：`decrypted`（与客户端的握手完成的连接数）、`untrusted`、`failed`（第 8.2 节）、`noRequest`（第 8.3 节）。自进程启动起累计，重载不清零（同 M1 的计数）。被停止打断的不计。

## 9. 解密之后的请求（M2a，`rurge-http`）

### 9.1 每个请求

照 M1b 的流程（M1 细化设计第 8 节），不同的只有：

- **URL** 以 `https://` 开头；端口是 443 时省略（今天 `origin_authority` 只省略 80，要看 scheme），重建的规则同 M1b（请求行、`Host`、连接的目标）。
- **协议**是 `HTTPS`（今天 `prepare` 写死 `ProtocolKind::Http`）；`sni` 沿用连接的。`PROTOCOL,HTTPS`、`URL-REGEX` 对 `https://` 的 URL、`USER-AGENT` 由此对 HTTPS 请求生效。
- **不以绝对形式交给 HTTP 代理**（`forwardable` 为假）：策略是 HTTP 代理时经 `CONNECT` 隧道连到目标，再在隧道里做 TLS。
- 嗅探到的连接上客户端发来的绝对形式请求与 `CONNECT`（客户端在解密的连接里还对它自己的代理说话）照 M1b 原样转给连接的目标。

### 9.2 对上游的 TLS

- 拨号照旧经 `Upstreams::dial`（引擎的选路与拨号，`CLAUDE.md`「出站套接字只有一处建法」）。拨好的流在 `connect()` 里先包一层计量（M1b 的 `Metered`，记的是线上的字节），再做 TLS 握手，再在上面做 HTTP/1.1 的握手。
- **SNI 与校验用的名字**：客户端的 SNI；客户端没给时是连接的目标；目标是 IP 时不发 SNI、按 IP 校验（总设计第 7.5 节）。一条解密的连接上的 SNI 不变，`OriginTls` 在连接进来时定好。
- **ALPN**：M2a 只给 `http/1.1`；M2b 照第 10 节。
- **时限**：30 秒（同第 8.2 节），超时同拨号失败。
- **客户端配置**：每一代两份（`skip-server-cert-verify` 只有一个值，所以实际一份，M2b 多一份只给 `http/1.1` 的，第 10.4 节），由 `rurge_proto::transport::tls::origin_client_config` 建：缺省按证书链与名字校验，根见第 9.3 节；`skip-server-cert-verify = true` 时用 `Verifier` 的 `Insecure` 模式——不校验链与名字，握手里的签名照常校验（手册："This relaxes verification of the real server only"）。不带客户端证书；会话恢复用 rustls 客户端的缺省（每份配置 256 条）。
- **失败**：上游的握手失败（证书不合法、名字不对、协议错误、对端要客户端证书而 rurge 没有）、超时：这个请求失败，502 错误页（`show-error-page` 开着时），记录的原因写 "upstream TLS handshake failed: …"（rustls 的原因，例如 `invalid peer certificate: UnknownIssuer`）。客户端那一侧的握手早已完成，不改成 TLS 错误（总设计第 2.3 节 3.7）。

### 9.3 上游校验的根

`rurge_net::tls::union_root_store()`：系统的根 ∪ `webpki-roots`，进程里读一次（`OnceLock`，同今天的 `root_store()`）。理由见总设计第 7.5 节（Windows 的根存储按需填充）；阶段 2 的出站 TLS 照旧用 `root_store()`。`EngineShared.roots` 有值时（"Tests bring their own CA this way"）用它——测试的源站用测试自己的 CA（第 17 节）。要不要改用调用系统校验接口的 `rustls-platform-verifier` 归所有者（总设计 Q4）。

### 9.4 池

池键不加 TLS 的参数（M2-D12）：池属于一条客户端连接（总设计 P4-D6），一条客户端连接上的请求要么都解密、要么都不解密，同一条解密的连接上 SNI 不变，键里已有的目标、出站链与代次足够区分。M3 的 URL Rewrite `header` 模式能把 `http` 改成 `https`、能换目标，那时把 scheme 与 SNI 加进键（总设计第 6.4 节已经写了键含 TLS 的参数）。

### 9.5 升级

WebSocket 的 101 与隧道里 `CONNECT` 的 2xx 之后两侧透传（M1b）。解密的连接上两侧都是 TLS 流：M1b 的 `promote` 里按 `TokioIo<Metered<BoxedStream>>` 取回上游的流那一支对不上（多了一层 TLS），走通用的那一支（hyper 的 `Upgraded`）——这样可以，升级之后的字节按明文计（由引擎的 `relay` 计），登记。

### 9.6 记录与日志

每个请求的记录同 M1b，`protocol` 是 `https`、`url` 以 `https://` 开头、`sni` 是连接的。请求的日志行同 M1b（级别同今天的会话，路径只在 DEBUG、不写查询串）。承载连接的记录只在第 8.2、8.3 节的情形与 M1b 的"一个请求也没有走到选路"时有。

## 10. HTTP/2（M2b）

只在 `h2 = true` 时（缺省 false）。M2b 的计划写之前对着 hyper 1.11.1 的源码再核一遍下面的事实（第 19 节 V16）。

### 10.1 对客户端

- ALPN 选到 `h2` 的解密连接用 hyper 的 `server::conn::http2`，执行器是把任务放到引擎任务跟踪器上的一个小类型（实现 `hyper::rt::Executor`），停止时等得到它们。
- **并发上限 100 个流**（M2-D14；总设计第 6.1 节"并发上限在 M2 定，hyper 的缺省 200 偏高"；RFC 9113 建议不少于 100）。不开 keep-alive 的 PING；装 `TokioTimer`。
- **空闲**：hyper 的 HTTP/2 服务端没有"没有请求时多久关"的时限。没有请求在途满 `idle_timeout` 时 `graceful_shutdown`（发 GOAWAY）。
- 不宣告扩展 `CONNECT`（不调 `enable_connect_protocol`，总设计 P4-D11）：浏览器为 WebSocket 另开一条只给 `http/1.1` 的连接。HTTP/2 上客户端发来的 `CONNECT`（RFC 9113 第 8.5 节的隧道）答 501、记录写原因（登记）。

### 10.2 对上游

- 解密的连接（不论客户端一侧是哪个版本）对上游的 ALPN 给 `h2`、`http/1.1`，各自协商（总设计 P4-D11）。
- 上游选了 `h2`：hyper 的 `client::conn::http2`（执行器同上）。**池里同一个键至多一条 HTTP/2 连接，共享给并发的请求**（`SendRequest` 克隆着用；`is_closed` 或收到 GOAWAY 之后不再分配新的流），不独占、不放回；第一次连上之前并发的几个请求各自拨号，之后的共享已有的那条（M2-D15）。
- 上游选了 `http/1.1`：照 M1b 的独占连接，至多 4 条空闲、90 秒。客户端一侧是 HTTP/2 时并发的请求各占一条上游连接，数目不另设上限（登记）。
- **字节**：HTTP/1.1 的上游照 M1b（换连接的计量，记线上的字节）；HTTP/2 的上游是共享的，换不了连接一层的计量，一个请求的 `up` / `down` 只算它的请求正文与应答正文（登记，M2-D15）。

### 10.3 版本转换

- `:authority` 与 `Host`：HTTP/2 的请求以 `:authority` 为主机（FR-HTTP-02"`:authority` 提取"），会话信息的 `http_host` 与 URL 取它；转到 HTTP/1.1 的上游时没有 `Host` 就按 `:authority` 补上、请求目标改成源站形式；HTTP/1.1 的请求转到 HTTP/2 的上游时 URI 改成 `https://` 的绝对形式（hyper 的 HTTP/2 客户端拿它写 `:authority`）。
- HTTP/2 拆开的多个 `cookie` 字段转到 HTTP/1.1 时用 `; ` 连成一个（RFC 9113 第 8.2.3 节）。
- 转到 HTTP/2 的一侧去掉连接专用的头（`Connection` 与它列出的、`Keep-Alive`、`Proxy-Connection`、`Transfer-Encoding`、`Upgrade`），`TE` 只留 `trailers`：h2 遇到它们会报错。M1b 的 `headers::strip` 已经去掉其中的大部分，M2b 补齐两个方向。
- 应答的版本由 hyper 随客户端一侧定；状态码、头与正文照转，trailers 照 M1b（声明过才转）。

### 10.4 升级只走 HTTP/1.1

带 `Upgrade` 的请求（只可能来自 HTTP/1.1 的客户端连接）要一条 HTTP/1.1 的上游：这样的拨号用只给 `http/1.1` 的那份客户端配置，池键里与别的请求分开（M2-D15）。

### 10.5 终止、看门狗、拒绝与停止

HTTP/2 上一条连接承载许多请求，M1b 里"关掉这条连接"的做法都要改成"重置这个流"（M2-D17）：

- **终止**一个请求（`POST /v1/requests/kill`、请求的令牌）：它的应答正文出错，hyper 重置这个流；还在等应答头时服务返回错误，hyper 以 `INTERNAL_ERROR` 重置这个流。连接与别的流照常。
- **看门狗**（客户端不读）：HTTP/2 有流量控制，客户端不读某个流时只有那个流的窗口到 0，hyper 不再 poll 它的应答正文。看门狗照 M1b 的条件到时记 `Failed("client stopped reading")`、放掉这个请求的上游（独占的 HTTP/1.1 连接关掉；共享的 HTTP/2 连接上丢掉这个应答的正文，hyper 重置上游的那个流），**不取消 `Conn::abort`**（那会关掉整条客户端连接）；客户端一侧的流在 hyper 下一次 poll 它时、或连接关闭时才被重置（登记）。
- **拒绝**：`REJECT` 有错误页时照答（在这个流上）；没有错误页时重置这个流，不关连接。`REJECT-DROP` 让这个流一直没有应答，停到 `drop_hold` 或客户端先重置它，再重置。
- **停止**：`graceful_shutdown`（GOAWAY），在途的流照 M1b 等它们自己的令牌。

## 11. 功能开关与 `state.json` 第 2 版（M2a）

### 11.1 开关

- `mitm`：`Engine` 里一个原子的布尔；`GET /v1/features/mitm` 读它、`POST` 设它并写进 `state.json`（经已有的 `StateStore::update`，写失败只记日志，同 `set_mode`）。切换立即对新的连接生效：已经解密的连接不因为关掉它而中断，之后判为 TLS 的连接不解密（总设计第 10.1 节）。
- `rewrite`、`capture`：**照旧读作关、`POST` 答 501，到 M3、M4**（M2-D18）：在它们有效果之前让它们能开，会让人以为开着就生效了。`scripting` 照旧到阶段 5。
- 缺省（没设过时）：`mitm` 开（总设计 P4-D19）。

### 11.2 `state.json` 第 2 版

- `STATE_VERSION` 改成 2。`Features` 的 `mitm`、`rewrite`、`capture`、`scripting` 改成 `Option<bool>`（没设过 / 开 / 关），序列化时**没设过的不写**（`skip_serializing_if`）；`system_proxy` 与 `enhanced_mode` 照旧是布尔。
- 读第 1 版的文件时把这四个当作没设过，不论文件里写的是什么（阶段 1 ～ 3 的代码从来不能把它们设成真，旧文件里的 `false` 不是使用者的选择，总设计第 10.1 节）。之后第一次写盘时写成第 2 版。
- **旧版本读新文件**：没设过的不写，所以文件里只有使用者设过的值，都是布尔，阶段 3 的 `rurge` 读得懂（它的 `Features` 带 `#[serde(default)]`），不会把整个文件当成坏的、改名成 `state.json.broken`（那会丢掉系统代理的备份）。**降级再升级**：旧版本写回文件时会把版本号照读到的写回（2），把没设过的写成 `false`；之后的新版本读到的是"设过、关"，`mitm` 因此是关的。登记（第 16 节）；使用者经 API 再打开即可。
- 启动时可执行文件在 `engine.attach_state(store)` 之后调 `engine.restore_switches(&state.features)`。

## 12. API（M2a）

### 12.1 `/v1/features/mitm`

`GET` 给 `{"enabled":bool}`（开关的值，不论 CA 能不能用——CA 的状态看 `GET /v1/mitm/ca` 与告警）；`POST {"enabled":bool}` 给 `{}`，INFO 一行 "mitm switched via http-api"。

### 12.2 `GET /v1/mitm/ca`

当前这一代的 CA 证书（与私钥配对的那一张）的 DER，不含私钥（手册："DER binary format (Certificate only, no private key included)"）；`Content-Type: application/x-x509-ca-cert`，`Content-Disposition: attachment; filename="rurge-ca.cer"`（浏览器直接下载、系统认得扩展名）。没有可用的 CA 时 404，正文同别的 404 的形状（`{"error":"…"}`）。钥匙可以放在查询参数里（`?x-key=`，阶段 1 起就认），浏览器能直接下载。

### 12.3 `/v1/rurge/http`

加 `mitm`，键的次序固定：

```json
{"sniffed":{"tls":0,"http":0,"other":0,"timeout":0,"closed":0},"requests":0,"mitm":{"decrypted":0,"untrusted":0,"failed":0,"noRequest":0}}
```

## 13. `rurge mitm ca generate` / `export`（M2c）

照总设计第 7.6 节，补上细节（M2-D20）：

- `rurge mitm ca generate --out <目录>`：ECDSA P-256 的 CA，有效期 10 年，`notBefore` 回拨 2 天（时钟不准的设备上刚生成的 CA 也能用，同叶证书），`BasicConstraints CA:TRUE, pathLen 0`（关键）、`KeyUsage keyCertSign, cRLSign`（关键）、SKI；CN 是 "rurge MITM CA <生成的日期>"，O 是 "rurge"。口令 24 个随机的字母与数字。写四个文件，名字固定：`rurge-ca.pem`、`rurge-ca.cer`（DER）——公开；`rurge-ca.p12`（3DES 加密、HMAC-SHA1 的 MAC）与 `rurge-mitm.conf`（`[MITM]` 节头与 `ca-p12`、`ca-passphrase` 两行）——**只属主可读**。
- "只属主可读"：Unix 上建文件时就是 `0600`（标准库的 `OpenOptionsExt::mode`，同 `crates/rurge-dns/src/fakeip/file.rs`）；Windows 上不另设 ACL，文件随所在目录的权限，输出里提醒把目录设成只有自己能读（总设计"要不要另设 ACL 由 M2 的细化设计定"：不设——要调 Windows 的安全接口，`unsafe` 只许在 `rurge-platform`，为一个提醒得到的保护加一个放开的函数不值，登记）。
- 四个文件有任何一个已存在就什么也不写、报错（不覆盖）；目录不存在就报错（不建）。
- 标准输出打印四个文件的路径、把 `rurge-mitm.conf` 的内容贴进配置的说明、各平台装 CA 的步骤（Windows `certutil -user -addstore Root`、macOS `security add-trusted-cert`、Linux 各发行版的系统库与 NSS 库、Firefox、Android 与 iOS 的说明）；**不打印私钥与口令**（凭据的规矩），rurge 自己不装。
- 生成之后自己读回一次 `rurge-ca.p12`（`decode_pkcs12`），读不回就报错并删掉写出的文件。
- `rurge mitm ca export -c <配置> [--out <文件>] [--der]`：从配置里读出 CA 证书（不含私钥）。不给 `--out` 时把 PEM 打到标准输出（证书是公开的）；`--der` 只能与 `--out` 一起；不覆盖。CA 不可用时报错，消息同 `W0039`。
- 实现在 `rurge-http`（`mitm::generate`，rcgen 与 p12-keystore 都在那里），可执行文件只管参数、文件与输出。

## 14. `http-api-tls`（M2c）

### 14.1 服务端

- `http-api-tls = true` 时 API 的监听器外面套一层 TLS（总设计第 7.8 节）：每条连接按 SNI、没有 SNI 时按这条连接进来的本地地址签叶证书（同第 6 节，同一个 `Authority` 的缓存）——手册："Surge will use the CA certificate of MITM to generate the server certificate for the corresponding access address." ALPN 只有 `http/1.1`（API 是 axum 的 HTTP/1.1）。
- **握手不占接受连接的循环**：接受之后在跟踪器上的任务里握手，握好的连接经一个通道交给 axum 的 `Listener::accept`；同时在握手的至多 64 个（同 `LINGER_MAX`），多出来的直接关；每个握手 30 秒（M2-D21）。握手失败 DEBUG 一行。
- 关闭照旧由 `Closing` 在 TCP 一层分阶段做（M4 的 `linger.rs`），TLS 包在它外面；丢掉 TLS 流时不发 `close_notify`（API 的应答都有长度，登记）。
- **没有可用的 CA**：API 不启动，WARN 一行（总设计第 2.3 节 3.22：不退回明文）；`rurge check` 给 `W0040`（"http-api-tls is on but the MITM CA cannot be used: the API will not start"）。重载让 CA 不可用时，API 照旧用 `EngineShared` 里最后一个生效过的 `Authority`（第 5.4 节），WARN 一行（API 不因为一次重载停下）。

### 14.2 命令行

`rurge status` / `reload` / `stop` 等经 `-c <配置>`（或 `--data-dir` 里托管配置的副本）找到 API 的，看到 `http-api-tls = true` 时走 HTTPS：根只有配置里的 CA 证书（解 `[MITM]` 的 CA，同第 5 节），按 API 的地址（IP）校验——叶证书带那个 IP 的 SAN。CA 解不开时报错，消息同 `W0039`。

### 14.3 `--remote`

`--remote <地址> --key` 不读配置、不知道 CA，照旧走明文：对开了 `http-api-tls` 的实例连不上（握手失败）。不加新参数（M2-D21），登记；有需要时再加一个给 CA 证书文件的参数。

## 15. 日志与凭据

- 第 8.2 ～ 8.4 节的行在 WARN / INFO / DEBUG。
  - 经 HTTP 与 SOCKS5 监听器来的会话：带名字（SNI 或连接的目标）与端口。
  - 经虚拟网卡来的会话：INFO 及以上不带名字与端口，名字另起一行 DEBUG（第 8.2 节）。
  - 都不带来源地址与进程。依据是 `CLAUDE.md`「日志的分级」：进程与网络只在 DEBUG；虚拟网卡上的名字与地址只在 DEBUG（M3-D26）。CA 出问题在加载时以诊断出现（`W0039`），运行中不再每个连接重复；名单命中而这一代没有 CA 时每个连接至多一行 DEBUG。
- 凭据：`ca-p12`、`ca-passphrase` 与 CA 的私钥不进日志、错误文本、API 输出与 `Debug`（第 4.4 节）；`p12-keystore` 的错误文本照阶段 2 的 `decode_p12` 带进消息（不含口令与材料）。`GET /v1/mitm/ca` 与 `export` 只给证书。
- 用户流量：解密之后的请求的记录带完整的 `https://` URL（含查询串、去掉 userinfo），经 `/v1/requests/*` 出去——总设计 Q1 归所有者，照它的临时做法；日志不写查询串、请求头与正文。

## 16. 行为的变化（登记）

M2 的文档任务把下面各条写进兼容性清单（`docs/surge-compatibility-matrix.md`，多数在 7.2 节），没有观察过 Surge 的实际行为的照实写：

1. 不带端口的项只在可嗅探的端口上靠 SNI 解密（第 7.6 节）。
2. `:0` 的项只让它命中的目标的全部端口可嗅探（第 7.6 节）。
3. `client-source-address`：
   - 本机的会话一律按 `127.0.0.1` 比，`::1` 的项不命中它们；
   - 经局域网地址连到监听器的本机应用按那个地址比；
   - MAC 的项在阶段 7 之前不命中；
   - 值为空按没写算。

   见第 4.3、7.2 节。
4. `ca-passphrase` 可以为空（总设计第 2.3 节 3.23）。
5. `ca-keystore-name` 指向不存在或不是 p12 的条目：告警、MITM 不生效，不回落到 `ca-p12`（第 5.1 节）。
6. Ed25519 的 CA 能用，但多数浏览器不认它签的证书（第 5.2 节）；P-521 与 DSA 的 CA 用不了。
7. `p12-keystore` 0.2.1 读不了的 PKCS#12 写法（总设计第 7.2 节列的几种）：告警、MITM 不生效（第 20 节第 1 条）。
8. 证书固定的提示会被浏览器的预连接触发；每个名字每小时至多一行 WARN（第 8.3、8.4 节）。
9. 升级（WebSocket）之后的字节按明文计（第 9.5 节）。
10. HTTP/2：并发 100 个流；上游的 HTTP/2 连接共享、请求的字节只算正文；客户端一侧 HTTP/2 而上游 HTTP/1.1 时上游连接数不设上限；HTTP/2 上的 `CONNECT` 答 501；看门狗到时客户端一侧的流要等下一次 poll 才重置（第 10 节）。
11. 降级再升级之后 `mitm` 是关的（第 11.2 节）。
12. `http-api-tls`：不发 `close_notify`；`--remote` 连不上开了它的实例；重载让 CA 不可用时 API 沿用之前的 CA（第 14 节）。
13. `rurge mitm ca generate` 在 Windows 上不设 ACL（第 13 节）。
14. EC 私钥的 PKCS#8 里没有公钥的 CA 用不了：ring 要公钥在，结果是 `W0039`，消息带 ring 的原因（第 5.2 节第 5 条）。
15. 经虚拟网卡来的会话，握手失败与证书固定提示的 WARN / INFO 行不带名字，名字在 DEBUG 与记录里（第 8.2 节）。

## 17. 测试

沿用的规矩（`CLAUDE.md`「测试约定」与 `docs/testing.md`）：只用回环与端口 0；失败时也不离开回环与临时目录；缺省的 `cargo test` 不改系统设置；有界等待；"什么都没发生"的等待注释写等的是什么与余量；新写的计时用例提交前单独循环跑二十次以上。**测试从不把任何 CA 装进系统或浏览器的信任库**（总设计 P4-D26）：测试的客户端是 rustls 的客户端，根只有测试自己生成的 MITM CA；源站是回环上的 TLS 测试服务器，证书来自测试自己的另一个 CA，经 `EngineShared.roots` 交给引擎做上游校验。测试用的 CA 与 PKCS#12 在内存里或用例的临时目录里生成（rcgen 与 p12-keystore 的写入），从不读、不写用例目录之外的证书。

1. **单元**（`rurge-config`）：九个键与读不懂时的诊断；`hostname-disabled` 按文字去掉（大小写、`-`、端口不同的不去掉）；`matches_sni` 与 `matches` 的区别（不带端口的项、`:端口`、`:0`、排除项在前）；来源名单（IP、CIDR、排除、MAC、非法项、IPv4 映射）；`W0029` 只一条；`Debug` 里没有两个凭据（`secrets.rs`）；`[MITM]` 不再出现在 `W0016` 里（语料库快照）。
2. **单元**（`rurge-http`）：CA 的载入——好的 CA（P-256、P-384、RSA 2048、Ed25519）、`[Keystore]` 来源与 `ca-p12` 来源、优先级、空口令；出问题的每一种（Base64 坏、口令错、没有私钥、不是 CA、KeyUsage 没有 `keyCertSign`、过期与未生效、P-521 的钥匙、试签不过）各一条，断言消息说的是哪一种、消息里没有口令与 Base64；多值 RDN 的 CA 用锁文件里已有的 `yasna`（rcgen 带来的）手写 DER 造，造不出来就照实登记成没有用例。叶证书的字段用 x509-parser 读回（SAN、CN、EKU、KU、AKI 等于 CA 的 SKI、CA 没有 SKI 时没有 AKI、没有 SKI 与 BasicConstraints、序列号、有效期的两个边界）；缓存（同一个名字同一张、第 1001 个名字挤掉最早的、剩余寿命不足三分之一时重签——时钟经只供测试的注入；同一个名字并发取只签一次、都拿到同一张——签名次数经只供测试的计数）；与客户端的握手：ALPN 的几种（没有、`http/1.1`、只有 `h2`）、没有 SNI 时按目标签（IP 的 SAN）、三种证书类警报与别的失败的分类（不信任这个 CA 的 rustls 客户端会发 `unknown_ca`）。
3. **端到端**（引擎，M2a）：客户端经 SOCKS5、`CONNECT` 与内存设备上的虚拟网卡进来，连到回环上的 TLS 源站，`[MITM] hostname` 命中：每个请求一条记录（`https`、`https://` 的 URL、方法、状态码），规则按 `URL-REGEX` 对同一条解密连接上的两个请求各选一条策略（其中一条 `REJECT`：TLS 里的 403 错误页）；源站的证书不受信 → 502 与原因；`skip-server-cert-verify` → 通；排除项、`hostname-disabled`、来源名单（本机按 `127.0.0.1`）、开关关掉 → 按原样转发；只给 `h2` 的客户端在 `h2 = false` 时按原样转发；写明端口与 `:0` 让非标准端口可嗅探、不带端口的项不让；握手之后不发请求就关 → WARN 的提示（`LogCapture`）、记录与 `noRequest`，第二次同一个名字在 DEBUG；不信任的客户端 → WARN 与 `untrusted`；同样的两种情形经虚拟网卡来时 WARN 里没有名字、DEBUG 行里有；重载换了 CA → 新的连接拿到新 CA 签的证书，没换 → 同一张叶证书（缓存跟着 `Authority` 走）；解密的连接上 WebSocket 的回显。主机名照规矩：`[Host]` 钉到 `127.0.0.1`，另加一条按地址的规则作第二道防线。
4. **API**：`/v1/features/mitm` 的读与切换、写进 `state.json`（第 2 版）、重启（用新的 `StateStore` 读同一个文件）之后照用；`GET /v1/mitm/ca` 的 DER 与配置里的 CA 相同、类型与文件名、没有 CA 时 404、`?x-key=`；`/v1/rurge/http` 的正文字节恰好是第 12.3 节那一串。`state.json` 第 1 版的文件里写着 `"mitm":false` 时读作没设过、开着。
5. **M2b**：两侧 HTTP/1.1 与 HTTP/2 的四种组合；`:authority` 进规则与 URL；`cookie` 合并；并发的流共享一条上游的 HTTP/2 连接（测试源站数连接）；终止一个流、别的流照常；`REJECT` 没有页时重置这个流；客户端不读一个流时看门狗到时只结束那个请求；空闲之后 GOAWAY；升级的请求拨 HTTP/1.1 的上游。
6. **M2c**：`generate` 写出四个文件、读回 p12（3DES / SHA1）、用它签一张叶证书并完成一次回环上的握手；Unix 上两个私密文件是 `0600`（CI 上跑）；已存在的文件与不存在的目录报错且什么也不写；标准输出里没有口令与私钥；`export` 的 PEM 与 DER；`http-api-tls`：命令行经 HTTPS 的 `status`、没有 CA 时 API 不启动与 `W0040`。

## 18. 文档

| 文档 | 改动 | 时机 |
| ---- | ---- | ---- |
| `docs/api/phase4.md` | M2a：`/v1/features/mitm`、`GET /v1/mitm/ca`、`/v1/rurge/http` 的 `mitm`、解密的请求的记录与日志、握手失败与提示的日志行；M2b：HTTP/2 的记录与终止；M2c：`http-api-tls` | 各自收尾 |
| `docs/testing.md` | MITM 的测试约定（不碰信任库、测试的 CA、`EngineShared.roots`、证书的生成只在内存与临时目录）；M2 的用例过滤词 | M2a 起 |
| `docs/surge-compatibility-matrix.md` | L54、L565、7.2 节各行、L871、L901（M2a）；L166、L860、L904、L585（M2c）；第 16 节的登记 | 各自收尾 |
| `docs/acceptance/phase4-manual.md` | 新建：三个平台的真实浏览器与命令行工具经三种入口上网（M1 的行为）；装好 CA 之后解密真实站点、HTTP/1.1 与 HTTP/2、证书固定的应用触发提示、`GET /v1/mitm/ca` 在浏览器里下载、`rurge mitm ca generate` 与各平台的安装步骤、`http-api-tls` | M2c |
| `README.md`、`README_en.md` | 状态、特性表、路线图（两份一致） | M2c 收尾 |
| `docs/requirements.md` | 第 7 节阶段 4 的状态一句；FR 的字面不改 | M2c 收尾 |
| `docs/status.md` | 阶段 4 下 M2a、M2b、M2c 各一节 | 各自收尾 |
| `CLAUDE.md` | 「现在在哪里」 | 各自收尾 |
| `docs/README.md` | 本文与各份计划各一项 | 随本文与计划 |

## 19. 写计划时要核对的事项

| 编号 | 核对什么 |
| ---- | -------- |
| V1 | `[General]` 怎样读键值行（`parse_general` 的循环、`bool_or`、`parse_host_list`、`Profile::section` 与停用的行），`[MITM]` 照它写 |
| V2 | `deferred.rs` 的名单与它的用例（第 67、85、90 行一带）；`config.rs` 里提到 `[MITM]` 的全部用例（今天第 2156、2603、6248 ～ 6258 行一带：`[MITM]` 里写 `x = 1` 的那条以后是 `W0001`，断言"不生效的节"的那几条要拿掉 `[MITM]`）；语料库快照 `corpus__corpus__kitchen-sink.snap` 里那一行；`secrets.rs` 第 539、540 行 `#!MACOS-ONLY` 前缀的 `[MITM]` 行怎样读 |
| V3 | `secrets.rs` 里 `[MITM]` 的两处标记与它怎样查 `Debug`；`redact.rs` 的 `SECRET_KEYS` |
| V4 | `decode_p12` 的全部调用处；`ClientIdentity` 的链的次序 |
| V5 | rcgen 0.14.7：`Issuer::from_ca_cert_der`、`CertificateParams` 的字段（`use_authority_key_identifier_extension`、`serial_number`、`not_before` 的类型）、`SanType::IpAddress`、`KeyPair::try_from`；开了 `x509-parser` 特性之后 `Cargo.lock` 不变 |
| V6 | x509-parser 0.18.1 读 BasicConstraints、KeyUsage、SKI、有效期与 subject 的接口 |
| V7 | rustls 0.23.43：`ServerConfig` 的克隆与 `cert_resolver`、`ServerSessionMemoryCache::new`、`WebPkiServerVerifier::builder_with_provider`、`Error::AlertReceived` 经 tokio-rustls 的 `io::Error` 怎样取回、`ServerConnection` 是不是 `Sync`（`BoxedStream` 的约束） |
| V8 | tokio-rustls 0.26.4：`TlsAcceptor::accept` 对 `Prefixed<BoxedStream>`；服务端与客户端的 `TlsStream` 能不能装进 `BoxedStream` |
| V9 | `rurge-http` 的 `connect`、`promote`（升级时的 `downcast` 与通用的那一支）、`form_of` 与 `origin_authority`、`prepare` 的 `ProtocolKind::Http`、`Served` 带回的东西（第 8.3 节要的"读到的明文字节数"与"怎样结束的"） |
| V10 | 引擎：`sniffable_session`、`serve_sniffed` 判为 TLS 的那一支、`serve_http` 的承载记录、`SniffCounts` / `HttpCounts`、`EngineShared.roots`、`HANDSHAKE_TIMEOUT` |
| V11 | `Runtime::build` 与 `Engine::swap_runtime`：`Mitm` 放进 `Runtime` 的位置；`EngineShared` 记最后一个生效过的 `Authority` 的位置（`opts.shared.cell` 是策略注册表，不是上一代的 `Runtime`，第 5.4 节）；`Runtime::build` 是不是异步的，解码怎样进 `spawn_blocking` |
| V12 | `StateStore` 与 `Features` 的全部用处（`cli/enhanced`、`cli/sysproxy` 只用 `system_proxy` 与 `enhanced_mode`）；`run.rs` 里 `attach_state` 之后还原的位置 |
| V13 | `routes/features.rs`；`/v1/rurge/http` 字节恰好的那条用例；`auth` 对 `?x-key=` 的处理；404 的正文形状 |
| V14 | `load_checked` 的调用处（`profile/boot.rs`、`profile/verdict.rs`、`subscriptions.rs`、`cli/run.rs`）逐个看是不是在阻塞的线程上：已知 `cli/run.rs` 的 `reload` 不是（第 5.3 节） |
| V15 | 测试夹具：`TlsFixture`、`TestServer` 的 TLS 版、引擎用例的 `geo_urls` 与 `roots`、内存设备上的虚拟网卡、`LogCapture` |
| V16 | （M2b）hyper 1.11.1 的 HTTP/2：`server::conn::http2::Builder` 的接口与缺省、`client::conn::http2` 的 `SendRequest`（`Clone`、`ready`、`is_ready`、`is_closed`）、`proto/h2/server.rs` 里客户端重置流时服务的 future 被丢与服务返回错误时的重置码、`proto/h2/mod.rs` 的 `PipeToSendStream` 在窗口为 0 时不 poll 正文、HTTP/2 客户端对 URI 与 `Host` 头的要求、对连接专用头的拒绝；行号写进计划 |
| V17 | （M2c）axum 0.8 的 `serve::Listener` 对 IO 类型的要求；`Lingering` / `Closing` 能不能被 TLS 包住；`ApiClient` 换 HTTPS 的连接器 |
| V18 | （M2c）p12-keystore 0.2.1 的 `Pkcs12Writer` 对证书包与密钥包各用什么算法 |
| V19 | `rurge-http` 的 tokio 特性够不够用 `spawn_blocking` 与 `sync::OnceCell`（工作区的 tokio 开了哪些特性）；ring 拒绝不带公钥的 EC 私钥时给的原因经 rcgen 怎样带出来；经虚拟网卡来的会话在 `rurge-http` 与引擎里怎样认出（`ListenerKind::Tun`），日志行怎样分两种写法 |

## 20. 对总设计的订正

1. 第 3.2 节、第 3.7 节与 P4-D24"`p12-keystore` 升到 0.4.x……M2 的第一个任务做" → **M2 留在 0.2.1**（M2-D2）。理由：升级要下载新的包，而"本阶段新引入的包与 `p12-keystore` 升到 0.4.x，同意吗"是所有者的 Q4、还没有回答；本会话的规矩是不下载、不安装。代价照实：0.2.1 读不了的几种写法（总设计第 7.2 节）落到"出问题"；迭代次数没有上限（总设计说的拒绝服务的口子）对 `ca-p12` 同样存在——这与阶段 2 起 `[Keystore]` 的 p12 条目一样，`ca-p12` 来自托管配置时提供者本来就能改全部的规则与代理。所有者回答 Q4 之后另起任务升级（第 22 节）。
2. 第 5.2 节"`:0` 的项让全部端口都算" → 只让它命中的目标的全部端口可嗅探；另加"只在来源允许时"（第 7.6 节，M2-D9）。
3. 第 4 节 `rurge-proto` 一行"`transport/tls.rs` 里的校验器……公开给 `rurge-http` 用" → 校验器照旧私有，公开的是建客户端配置的函数 `origin_client_config`（第 9.2 节）：`rurge-http` 只要配置，不要校验器本身。
4. 第 6.1 节"并发上限在 M2 定" → 100（第 10.1 节，M2-D14）。
5. 第 7.2 节没有说 `ca-keystore-name` 指向不存在的条目怎么办 → 告警、MITM 不生效、不回落到 `ca-p12`、不用 `E0020`（第 5.1 节，M2-D4）。
6. 第 7.3 节的表 → AKI 只在 CA 有 SKI 时写；`notBefore` 不早于 CA 的 `notBefore`；签名算法随 CA 的密钥（第 6 节，M2-D6）。第 7.3 节"链：叶 + CA" → 叶 + PKCS#12 里的整条链（第 5.5 节）。
7. 第 7.3 节"缓存……容量 1000 条（M2 定）" → 1000 条；服务端的会话缓存 1024 条（第 6 节，M2-D7）。
8. 第 7.4 节的 WARN 与提示 → 每种、每个名字一小时至多一行 WARN（第 8.4 节，M2-D11）；提示的条件加"读到的明文字节数是 0、不是时限或停止"（第 8.3 节）。
9. 第 7.6 节"要不要另设 ACL 由 M2 的细化设计定" → 不设（第 13 节，M2-D20）；文件名、CN、`notBefore` 回拨与读回（同上）。
10. 第 7.8 节 → 握手不占接受的循环、至多 64 个在途；重载让 CA 不可用时沿用之前的；`--remote` 不变（第 14 节，M2-D21）。
11. 第 10.1 节"`mitm`、`rewrite`、`capture`：`GET` 读真实状态、`POST` 切换" → M2 只让 `mitm` 生效，`rewrite`、`capture` 到 M3、M4（第 11.1 节，M2-D18）。`state.json` 第 2 版的读写细节（没设过的不写、降级再升级的那一处，第 11.2 节）。
12. 第 13 节"计数：……解密的连接、握手失败" → `/v1/rurge/http` 的 `mitm` 四个数（第 12.3 节，M2-D19）。
13. 第 6.4 节"键：……TLS 的参数" → M2 的键不加 TLS 的参数，M3 加（第 9.4 节，M2-D12）。
14. 第 1.4 节"M2 也可能把 `http-api-tls` 与 CA 工具拆出去" → 拆成三份计划，HTTP/2 单独一份（第 2 节，M2-D1）。

## 21. 已决事项

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| M2-D1 | 计划与执行 | 一份设计、三份计划（M2a HTTP/1.1 上的解密 → M2b HTTP/2 → M2c CA 工具、`http-api-tls` 与收尾），后一份在前一份合并之后对着代码写；逐个任务调 Pi、逐个任务评审；M2c 有只在 CI 上编译的 `#[cfg(unix)]` 分支，M2 做完请所有者推送一次（连同 M1）、读 CI（第 2 节） |
| M2-D2 | 依赖 | 不加新包、不下载；`rurge-http` 正式依赖 rcgen（`x509-parser` 特性）、x509-parser、time、lru、rustls、tokio-rustls；工作区依赖表加 `x509-parser`、`time` 两行；`p12-keystore` 留在 0.2.1；`Cargo.lock` 的包数不变；M2c 加 `rurge → rurge-http`（第 3 节） |
| M2-D3 | `[MITM]` 的读法 | 照 `[General]`：第一个节、后写的键为准；名单项与来源项非法 `W0005`、布尔 `W0012`、别的键 `W0001`；`hostname-disabled` 按文字去掉；来源名单按顺序、不命中即不允许、值为空按没写算、MAC 项阶段 7 之前不命中且整键一条 `W0029`；空口令收；`Debug` 藏两个凭据（第 4 节） |
| M2-D4 | CA 的来源与检查 | `ca-keystore-name` 优先、不回落；六条检查（含试签一张并用 webpki 校验）；出问题 `W0039`、这一代没有 `Mitm`；有名单没有 CA 也告警；检查接进 `load_checked`（第 5.1 ～ 5.3 节） |
| M2-D5 | 每一代与同样的材料 | `EngineShared` 记最后一个生效过的 `Authority`（换上时记，不在建时记）；新一代的材料（Base64 与口令）指纹与它相同就沿用（叶证书与会话缓存跟着它），不同就新建；`Mitm` 每一代新建；已解密的连接不因重载中断（第 5.4 节） |
| M2-D6 | 叶证书 | 总设计第 7.3 节，加：AKI 只在 CA 有 SKI 时写；`notBefore` 不早于 CA 的；签名算法随 CA 的密钥；链是叶 + PKCS#12 的整条链；没有 SNI 时按连接的目标签（第 5.5、6 节） |
| M2-D7 | 签名与缓存 | 签名一律在 `spawn_blocking` 里；同一个名字同时只签一次（每名一个槽）；LRU 1000 个槽、剩余不足三分之一换槽重签；服务端会话缓存 1024 条、同一个 `Authority` 的连接共用（第 6 节） |
| M2-D8 | 何时解密 | 开关 → CA → 来源 → 名单 → ALPN；本机的会话按 `127.0.0.1`；有 SNI 时 `matches_sni`、没有时按目标 `matches`；M2a 的 ALPN 只认 `http/1.1`，M2b 起照总设计；只在 ALPN 合不上时 DEBUG 一行（第 7.1 ～ 7.5 节） |
| M2-D9 | 可嗅探的扩大 | 开关开、有 `Mitm`、来源允许，且端口是某个非排除项写明的端口、或目标按名单命中；`:0` 只扩它命中的目标；不带端口的项不扩（第 7.6 节） |
| M2-D10 | 对客户端的配置与握手 | 每一代一份模板、每条连接克隆并固定叶证书；TLS 1.2 与 1.3、不要客户端证书、不收 0-RTT；在前缀流上握手；30 秒；与停止竞速（第 8.1、8.2 节） |
| M2-D11 | 握手失败与提示 | 三种证书类警报 WARN 与 `untrusted`；别的 INFO 与 `failed`；提示要握手完成、明文 0 字节、客户端关闭；WARN 每种每名字每小时一行，其余 DEBUG；承载连接各有记录（第 8.2 ～ 8.4 节） |
| M2-D12 | 解密之后的请求 | `https://` 的 URL（443 省略）、`HTTPS`、不以绝对形式交给 HTTP 代理；对上游的 SNI 是客户端的 SNI 或目标、IP 不发；`origin_client_config`；30 秒；失败 502 带原因；池键不加 TLS 的参数；升级走通用的那一支、字节按明文计（第 9 节） |
| M2-D13 | 上游校验的根 | 系统 ∪ `webpki-roots`（`union_root_store`，进程里读一次）；`EngineShared.roots` 有值时用它（第 9.3 节） |
| M2-D14 | HTTP/2 的服务端（M2b） | 跟踪器上的执行器；100 个流；没有 PING；空闲 `idle_timeout` 之后 GOAWAY；不宣告扩展 `CONNECT`；HTTP/2 上的 `CONNECT` 答 501（第 10.1 节） |
| M2-D15 | HTTP/2 的上游（M2b） | ALPN 两个都给；同一键至多一条共享的 HTTP/2 连接；HTTP/1.1 的上游照 M1b；升级的请求只给 `http/1.1`、键里分开；HTTP/2 上游的字节只算正文（第 10.2、10.4 节） |
| M2-D16 | 版本转换（M2b） | `:authority` 为主机；补 `Host` 与改请求目标的形式；`cookie` 合并；去掉连接专用的头、`TE` 只留 `trailers`（第 10.3 节） |
| M2-D17 | HTTP/2 上的终止（M2b） | 终止重置这个流；看门狗不取消整条连接；`REJECT` 没有页时重置流；`REJECT-DROP` 停住这个流；停止发 GOAWAY（第 10.5 节） |
| M2-D18 | 开关与 `state.json` | 只有 `mitm` 生效，缺省开；`rewrite`、`capture` 到 M3、M4；第 2 版：四个开关三态、没设过的不写、读第 1 版当作没设过；启动时还原（第 11 节） |
| M2-D19 | API | `/v1/features/mitm`；`GET /v1/mitm/ca`（DER、`application/x-x509-ca-cert`、`rurge-ca.cer`、没有时 404）；`/v1/rurge/http` 加 `mitm` 四个数（第 12 节） |
| M2-D20 | `rurge mitm ca`（M2c） | P-256、10 年、回拨 2 天、关键的 BasicConstraints 与 KeyUsage、SKI；四个固定名字的文件；Unix `0600`、Windows 不设 ACL；不覆盖不建目录；读回；`export` 的 PEM 可以打到标准输出（第 13 节） |
| M2-D21 | `http-api-tls`（M2c） | 叶证书按 SNI 或本地地址；握手不占接受的循环、至多 64 个、30 秒；没有 CA 时 API 不启动与 `W0040`；重载让 CA 不可用时沿用之前的；命令行经 `-c` 时信配置里的 CA；`--remote` 不变（第 14 节） |
| M2-D22 | 诊断码 | `W0039`（MITM 的 CA 用不了，不设缺省建议：消息说的是哪一种问题，每处各不相同）；`W0040`（`http-api-tls` 开着而 CA 用不了，缺省建议"配置可用的 MITM CA，或关掉 `http-api-tls`"）；各在 `codes::hint` 的表里做决定 |
| M2-D23 | 日志 | 第 15 节：经监听器来的会话带名字与端口；经虚拟网卡来的会话 INFO 及以上不带名字与端口，名字另起 DEBUG 一行（M3-D26）；都不带来源与进程；CA 的问题只在加载时 |
| M2-D24 | 测试 | 不碰信任库；测试的 CA 在内存与临时目录里生成；源站经 `EngineShared.roots`；时钟注入；第 17 节的清单 |
| M2-D25 | 文档 | 第 18 节的表；手工清单在 M2c 新建 |

## 22. 留给项目所有者的

| 问题 | 与 M2 的关系 |
| ---- | ------------ |
| 总设计 Q4：新包与 `p12-keystore` 升到 0.4.x；`webpki-roots` 的许可；`rustls-platform-verifier` | M2 留在 0.2.1（第 20 节第 1 条）：读不了的写法落到"出问题"，迭代次数没有上限的口子对 `ca-p12` 与 `[Keystore]` 一样存在。解密的上游用"系统 ∪ `webpki-roots`"，`webpki-roots` 多了一处用法。所有者同意升级之后另起任务：升级、逐种重核总设计第 7.2 节列的写法、能靠打开底层包的特性补上的补 |
| 总设计 Q1：用户流量的内容算不算 API 输出 | M2 起解密之后的 HTTPS 请求也逐个记录、带完整的 `https://` URL（含查询串）——HTTPS 的查询串里令牌更常见。照总设计的临时做法做 |
| 总设计 Q3：`[MITM] # note` 这样的节头 | M2 让 `[MITM]` 生效之后，这一行读错的后果是 MITM 的键落进上一节（`W0001` 一串），MITM 悄悄不生效。本阶段不改节头的解析 |
| 总设计 Q5：把 CA 自动装进系统的信任库 | M2c 只打印步骤 |
| 经虚拟网卡来的会话的名字在 INFO 及以上（M3b 计划延后第 1 行，即 M3c 计划延后第 6 行那一项） | M2 新加的握手失败与证书固定提示的行照 M3-D26 写：经虚拟网卡来的会话在 WARN / INFO 里不带名字，名字在 DEBUG 与记录里。对这类会话的使用者，提示因此弱一些。所有者若在那一项上决定 INFO 及以上可以带名字，这几行随之改 |
| 总设计 Q2、Q6 | 与 M2 无直接关系，照旧开着 |

另有两件事要项目所有者做：**推送**（M1 与 M2 一起，M2c 做完之后；推送之后读回那次 CI）；**手工验收**（`docs/acceptance/phase4-manual.md` 在 M2c 新建；阶段 1、2、3 的清单照旧一项也没有做）。

所有者可以改的裁定：本文 M2-D1 ～ D25，尤其 M2-D2（不升级 `p12-keystore`）、M2-D4（不回落到 `ca-p12`、不用 `E0020`）、M2-D9（`:0` 只扩它命中的目标、不带端口的项不扩）、M2-D11（提示的限频）、M2-D18（`rewrite`、`capture` 到 M3、M4 才能开）与 M2-D21（`--remote` 不变）。

## 23. 计划与执行期的订正

写计划与执行时对本文正文的改动记在这里，一条一段：改了正文哪一处、以哪一处为准。正文不逐处改，读正文之前先读这一节。

**写 M2a 计划时**（`docs/superpowers/plans/2026-10-30-phase4-m2a-mitm-http1-plan.md`，以它的「计划期决定」为准）：

1. 第 2 节 M2a 的任务表：任务 5 拆成两个。任务 5 是每一代的 `Mitm`、开关与 `state.json`、可嗅探的扩大、解密的分派；任务 6 是握手失败与证书固定提示的日志、限频与计数。API、端到端、文档顺延为任务 7、8、9（计划 P24）。
2. 第 3 节与第 5.1 节的 `decode_pkcs12(label, base64, password)` → `decode_pkcs12(label, der, password)`，另加 `base64_bytes(text)`：Base64 那一步由各来源自己做，`[Keystore]` 条目今天的两种消息逐字不变（计划 P4）。
3. 第 3 节的 `Engine::switch` / `set_switch` → `mitm_on` / `set_mitm`：M2a 只有一个能切的开关，按名字写，`rewrite`、`capture` 在 M3、M4 各加自己的（计划 P20）。
4. 第 5.1 节"没有 CA 而生效的名单不空就告警一次"：这一条也是 `W0039`，同样以 "MITM is off: " 开头，在 `hostname` 那一行（计划 P5 第 1 条）。
5. 第 5.2 节第 5 条"消息带 ring 给的原因"与第 16 节第 14 条的同一句：做不到。rcgen 0.14.7 的 `KeyPair::try_from` 依次试 Ed25519、P-256、P-384、RSA，全不成时只返回 `CouldNotParseKeyPair`，ring 的原因丢了。消息改为列出 rurge 签得了的几种，并写明"EC 的 PKCS#8 要带公钥"（计划 P1、P5 第 11 条）。
6. 第 5.2 节第 6 条"试签一次查掉几种坏法"：多值 RDN 在试签之前就被 rcgen 的 `Issuer::from_ca_cert_der` 拒绝（`DistinguishedName::from_name`），是单独的一条消息；重复的属性类型（rcgen 把它们合并，签发者名字因此与 subject 不同）、证书与私钥不配对、签名算法不认，仍在试签时查出（计划 P5 第 12、13 条）。
7. 第 5.4 节"材料相同就沿用"：另加"且现在仍在 CA 的有效期里"（`Authority::usable_now`）。过期的不沿用，重新载入就得到"过期"的问题（计划 P16）。
8. 第 9.1 节"URL 以 `https://` 开头"：解密的连接上每个请求的 URL 都是 `https://`，包括客户端在解密的连接里发来的绝对形式请求。请求行里的 scheme 不看，请求照 M1b 去连接的目标（计划 P13）。
9. 第 17 节第 2 条"多值 RDN 的 CA 用 `yasna` 手写 DER 造"：改为由协调者用本机的 OpenSSL 生成五组夹具——RSA 2048 的私钥、P-521、不带公钥的 EC 私钥、多值 RDN、重复的属性类型——以 Base64 写进 `crates/rurge-http/src/mitm/fixtures.rs`。不带 SKI 的 CA 用 rcgen 造（计划 P6）。

**执行 M2a 时**：由 M2a 的任务 9 补上（计划「执行期修正记录」里改了本文正文意思的行）。

10. 第 3 节 `MitmConfig` 的 `spans`：注释写"各键所在的行"，实际只记三处——`hostname`、`ca-p12`、`ca-keystore-name`，`MitmSpans` 也只有这三个字段。它们是 `W0039` 报在的那三行；其余六个键的行没有一条诊断报在那一行上，记下来只有无人读的字段（执行记录第 20 行）。
11. 第 5.2 节第 4 条 `notBefore ≤ 现在 < notAfter`：上界含在内。过期是 `now > not_after`，`Authority::usable_now` 是 `not_before ≤ now ≤ not_after`（RFC 5280 4.1.2.5；rustls-webpki 只在 `time > not_after` 时报 `CertExpired`）。到期的那一刻仍可用（执行记录第 28 行 ④）。
12. 第 3 节 `pub async fn leaf(&self, name: &HostName)`：接收者是 `self: &Arc<Self>`——签名与叶证书密钥的生成在 `spawn_blocking` 里（M2-D7），闭包要 `'static`，借来的 `&self` 进不去（执行记录第 29 行）。
13. 第 5.4 节"一代配置换上时（`Engine::swap_runtime`），它有 `Mitm` 就记下"：第一代不经过 `swap_runtime`，由 `Engine::new` 在存下注册表与解析器之后自己记，仍是"换上时记，不在建时记"；`publish_generation` 只在 `swap_runtime` 里被调（计划 P17 说它在 `Engine::new` 里也被调，不对）（执行记录第 41 行）。
14. 第 3 节列的 `rurge-http` 新增正式依赖漏了两个包：`sha2`（材料的指纹，第 5.4 节）与 `ring`（叶证书序列号的随机数，第 6 节），都是锁文件里已有的包、`Cargo.lock` 的包数不变。这一条是写计划时定的，当时没有记进上面"写 M2a 计划时"那一段（计划「全局约束」的依赖一条）。

**写 M2b 计划时**（`docs/superpowers/plans/2026-10-30-phase4-m2b-mitm-http2-plan.md`，以它的「计划期决定」为准）：

15. 第 10.1、10.5 节的 GOAWAY：发了 GOAWAY（空闲或停止）之后，连接至多再等 1 秒（从发 GOAWAY 起、没有请求在途时计），之后丢掉。h2 的平静关要等客户端回应它发的 PING 才发最后的 GOAWAY，hyper 在握手中途只记下"要关"，而可执行文件收尾时等任务不设时限：不回 PING 或一直不发前言的客户端会让停止一直等下去（计划 P5）。
16. 第 10.2 节"HTTP/2 上游……一个请求的 `up` / `down` 只算它的请求正文与应答正文"：拨这条连接的那一个请求另带握手之前记下的字节（TLS 握手与 HTTP/2 的前言，同 M2a 记 TLS 握手的做法）；握手之后连接一层不再记，之后的请求只记正文（计划 P11）。
17. 第 10.4 节"池键里与别的请求分开"：不分开。池里本来只有 HTTP/1.1 的连接，升级与隧道跳过共享的 HTTP/2 连接、从池里借或用只给 `http/1.1` 的配置拨就够了（计划 P12）。
18. 第 10.5 节看门狗"不取消 `Conn::abort`"：停止时（连接的令牌取消）仍取消——整条连接本来就要走，不取消的话一个不读的流会让停止一直等下去；只终止一个请求、或看门狗到时，才只放掉这个请求的上游（计划 P14）。
19. 第 10.2 节加一条、第 3 节与 M2-D2 的依赖列表加一个包：上游以 `REFUSED_STREAM` 拒绝、或以 GOAWAY 说没有处理的流（RFC 9113 第 8.7 节），对借来的连接上、幂等且没有正文的请求在新拨的连接上再发一次，同 M1b 对"连接在应答之前断了"的重发；为了认出这两种错误，`rurge-http` 正式依赖 `h2`（锁文件里已有，包数不变）（计划 P13）。

**执行 M2b 时**：由 M2b 的任务 6 补上（计划 `docs/superpowers/plans/2026-10-30-phase4-m2b-mitm-http2-plan.md`，「执行期修正记录」里改了本文正文意思的行）。

20. 第 10.5 节终止一条"它的应答正文出错，hyper 重置这个流"只写了重置、没有写原因码：终止、看门狗到时、没有错误页的 `REJECT`、`REJECT-DROP` 与上游正文出错，客户端一侧的那个流一律以 `INTERNAL_ERROR` 重置，上游的原因码不转给客户端（讲 HTTP/1.1 的客户端不受影响：hyper 的 HTTP/1.1 一侧遇到正文出错就关掉连接，没有原因码可言）（执行记录第 48、52 行）。
21. 第 16 节第 10 条的"请求的字节只算正文"与第 16 条（第 10.2 节"一个请求的 `up` / `down` 只算它的请求正文与应答正文"）里"拨这条连接的那一个请求另带握手之前记下的字节"：那一段不是"握手之前记下的"、也不是准确的数——计量是在 `start_h2` 返回之后才设成 `None` 的，在那之前走过的字节都算在它头上，除了 TLS 握手与 HTTP/2 的前言，可能还有 SETTINGS 的交换与 TLS 1.3 的会话票据，多少看时机（执行记录第 42 行）。

**写 M2c 计划时**（`docs/superpowers/plans/2026-10-31-phase4-m2c-ca-tools-api-tls-plan.md`，以它的「计划期决定」为准）：

22. 第 14.1 节 `W0040` 的消息改为 "http-api-tls is on but there is no usable MITM CA: the API does not start with this profile"：原文 "the MITM CA cannot be used" 漏了没有写 CA 的那一种；重载时这一条也会报出来，而那时 API 还在用之前的 CA，所以写成"不会以这份 Profile 启动"。另：只在 Profile 有 `http-api` 时报；`[Keystore]` 那一项的 Base64 坏了、已经报了 `E0021`（因此没有 `W0039`）时照样报（计划 P2）。
23. 第 13 节末条说 `generate` 放在 `rurge-http` 是因为"rcgen 与 p12-keystore 都在那里"，这不准：`rurge-http` 今天不依赖 `p12-keystore`（`decode_pkcs12` 在 `rurge-proto`），`base64` 只是它的开发依赖。M2c 给 `rurge-http` 加 `p12-keystore`、把 `base64` 挪进正式依赖；`rurge-api` 与可执行文件另加 `rustls`、`tokio-rustls`；`rurge-api` 的开发依赖加 `rurge-http`。都在锁文件里，包数不变。第 3 节的依赖列表随之（计划 P5）。
24. 第 13 节"生成之后自己读回一次 `rurge-ca.p12`（`decode_pkcs12`）"：读回用 `Authority::load`，解码之外还查它是不是 CA、能不能签（计划 P6）。
25. 第 13 节 `rurge mitm ca export` 的参数另有 `--platform` 与 `--data-dir`，同 `rurge status`：托管的 Profile 的 `[MITM]` 在数据目录里的副本里（计划 P7）。
26. 第 14.1 节"API 照旧用最后一个生效过的 `Authority`"：API 每接一条连接问一次（`Engine::api_tls`），所以重载换了 CA 之后，新的连接由新的 CA 签——命令行经 `-c` 读的也正是新的 CA。第 3 节"`http-api-tls` 的监听器"的入口是另一个函数 `serve_tls`，`serve` 不变；有没有可用的 CA 由可执行文件在起 API 之前问（计划 P9、P11、P12）。
27. 第 14.1 节"重载让 CA 不可用时……WARN 一行"：重载时关于 API 另有两种告警——`http-api-tls` 本身在重载里改了，与启动时没有 CA、重载之后有了。`http-api` 变了时"保留现在的地址与钥匙"那一句只在 API 真的在跑时说：`http-api-tls` 开着而没有 CA 时 API 没有起（计划 P13）。

**执行 M2c 时**：由 M2c 的任务 8 补上（计划「执行期修正记录」里改了本文正文意思的行）。
