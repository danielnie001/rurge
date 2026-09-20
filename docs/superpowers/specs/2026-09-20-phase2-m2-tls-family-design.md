# 阶段 2 / M2「TLS 族」设计

> 状态：已与项目所有者逐节确认（2026-09-20）。本文件是 M2a / M2b / M2c 三份实施计划的依据；与阶段 2 总设计（`2026-09-19-phase2-outbound-groups-design.md`）不一致处以本文件为准，并在第 14 节登记。Surge 行为以 2026-09 版手册为准（`policies/tls` `policies/trojan` `policies/vmess` `policies/anytls` 各页已核对）。各协议线上格式的逐字节细节在写对应计划时对照参考实现的源码与协议文档核对后写进计划；本文件定结构、接口、语义与安全要求。

## 1. 目标与范围

### 1.1 目标

- 让 `[Proxy]` 里的 `trojan` `vmess`（AEAD 握手）`anytls` 策略真正可用（TCP），各自可叠加手册允许的传输层：TLS、WebSocket、Shadow TLS v2 / v3。
- **项目所有者日常使用的是 Trojan**：Trojan 最先落地、最先合并。
- 兑现总设计 5.5 / D8：重载时按指纹复用出站，与某条策略无关的重载不打断它的连接池。
- 结掉总设计的风险 A / Q2（Shadow TLS v3 的实现路径）。

### 1.2 范围内（PRD 编号）

| 需求 | 本里程碑覆盖的部分 |
| ---- | ------------------ |
| FR-OUT-04 | Shadow TLS v2 / v3（`shadow-tls-password` `shadow-tls-sni` `shadow-tls-version`），可叠加在所有 TCP 类出站上 |
| FR-OUT-05 | `trojan`（可带 `ws`）、`vmess`（AEAD；可带 `tls`、`ws`）、`anytls`（会话复用、padding 方案）的 TCP |
| FR-OUT-08 | 三种协议都能作链的任意一跳（沿用 M1 的 `ChainConnector`，无新机制） |
| FR-OBS-01 | 新协议的凭据及其派生物不出现在错误、日志、API、`Debug` 输出里 |
| AR-04 | 重载时按指纹复用出站；被复用的出站经 `ResolverCell` 看到新一代解析器 |

### 1.3 范围外

| 项 | 去向 |
| -- | ---- |
| 三种协议的 UDP（trojan 的 `0x03`、vmess 的 UDP 命令、anytls 的 udp-over-tcp v2） | M5 |
| VMess 旧式（非 AEAD）握手 | M8，与总设计一致（见 4.3） |
| uTLS 指纹伪装、Trojan-Go 的 mux、WebSocket early data | Surge 手册里没有，不做 |
| Shadow TLS 与 QUIC 类 / WireGuard 组合的配置错误 | 判断函数在 M2c 落地；那些协议的 spec 出现时（M4 / M7）接上 |
| 链底下的 REJECT 到不了 `dial_internal` 的 reject 旁路；链深超限时两处说法不一致 | M3（M1b 终审的延后事项，去向不变） |

### 1.4 三份计划

按顺序执行、各自合并。每份计划开工前不再另写设计，只写计划。

| 计划 | 内容 | 合并后的可见成果 |
| ---- | ---- | ---------------- |
| **M2a　Trojan 优先** | 传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 层、`trojan`（可带 `ws`）、M1b 承接事项（第 9 节）、sing-box 互操作（trojan、trojan + ws）、能力表翻转 `trojan`、文档（含 `docs/acceptance/phase2-manual.md` 的 Trojan 一节） | 手写 `[Proxy]` 的 Trojan 节点 + `select` 组可日常使用 |
| **M2b　VMess / AnyTLS** | `vmess`（AEAD；可带 `tls` / `ws`）、`anytls`（会话复用、padding）、`ResolverCell` 与 `publish_generation`、按指纹复用出站、互操作（sing-box：vmess / anytls；xray：只测 vmess）、能力表翻转 `vmess` `anytls` | 三种 TLS 族协议齐备；无关的重载不打断连接池 |
| **M2c　Shadow TLS** | v2 与 v3、接到所有 TCP 类出站（届时把 M1 的 http / socks5 出站迁到 `Stack`）、互操作（sing-box 的 shadowtls 入站）、三个 Shadow TLS 参数的 `W0029` 退役 | 总设计验收标准第 5 条 |

## 2. 已确认的决定

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| M2-D1 | 优先级 | 项目所有者主要使用 Trojan：Trojan 单独成为第一份计划（2026-09-20） |
| M2-D2 | 计划的拆分 | 一份设计、三份计划（M2a / M2b / M2c），各自合并 |
| M2-D3 | Shadow TLS v3 的实现路径（总设计风险 A / Q2） | **stock rustls，不 fork、不打补丁、不用 unsafe**：两遍构造 ClientHello（附录 A）。2026-09-20 的一次性 spike 已验证：rustls 0.23.43 上 50 轮"带签名 SessionID"的 TLS 1.3 握手全部完成 |
| M2-D4 | VMess 旧握手 | 留在 M8；M2b 里没写 `vmess-aead=true` 的 `vmess` 行按"协议未实现"处理（4.3） |
| M2-D5 | 互操作的参考实现 | sing-box 1.14.1（CI 已有的固定二进制）覆盖全部；**xray 只用于 vmess**（M2b 引入，固定版本 + SHA256）：VMess 由它那一脉定义，sing-box 的实现是重写，手写的编解码需要两个独立实现作证 |
| M2-D6 | WebSocket 的实现 | `tokio-tungstenite`（总设计第 2 节），`default-features = false`，不启用它的 TLS 特性 |
| M2-D7 | 脱敏 | `ws-headers`、`ws-path`、`shadow-tls-password` 进 `SECRET_PARAMS`（整值抹掉，也不进 `lineHash`）：经 CDN 的节点常把路径当共享密钥，该模块的既定原则是宁可多抹 |
| M2-D8 | 手工验收清单 | `docs/acceptance/phase2-manual.md` 提前到 M2a 创建（总设计原定 M8），先放 Trojan 一节 |
| M2-D9 | AnyTLS 开流 | 不等 `cmdSYNACK` 就返回流（6.3） |
| M2-D10 | Shadow TLS 伪装握手的证书 | 照常校验；六个 TLS 参数只作用于里层 TLS（5.3） |

## 3. crate 改动一览

| crate | 改动 | 计划 |
| ----- | ---- | ---- |
| `rurge-config` | `spec::{ws, trojan, vmess, anytls, shadow_tls}`；`ProtoSpec` 增三个变体；`read_tls` 校验 `server-cert-verify-name`；socket 选项 × `underlying-proxy` 的 `W0028`；`SECRET_PARAMS` 增三项 | a / b / c |
| `rurge-proto` | `transport::{stack, ws, shadow_tls}`；`trojan` `vmess` `anytls` 三个出站模块；惰性请求头的 `LazyHead` 流；`testing::{ws, trojan, vmess, anytls, shadow_tls}` | a / b / c |
| `rurge-policy` | `PolicyRegistry::build(.., previous)` 与指纹；`OutboundFactory::environment()` | b |
| `rurge-engine` | `EngineFactory` 认识三种协议；`ResolverCell`（进 `EngineShared`）；`publish_registry` → `publish_generation`；`tests/outbounds.rs` 的端到端用例 | a / b / c |
| `rurge`（bin） | 能力表翻转；CLI 用例 | a / b |
| `tests/interop` | sing-box 的 trojan / vmess / anytls / shadowtls 入站；xray 的 vmess 入站；CI 安装并校验 xray | a / b / c |

依赖方向不变：协议代码全部在 `rurge-proto`（总设计第 3 节：TLS 族不单独成 crate）；`rurge-policy` 仍不依赖任何协议实现；平台代码仍只在 `rurge-platform`。

**新依赖**（均在 NFR-11 的许可证白名单内，MIT / Apache-2.0）：

| 依赖 | 用途 | 计划 |
| ---- | ---- | ---- |
| `tokio-tungstenite`（带 `tungstenite`）、`futures-util`（若尚未直接依赖） | WebSocket 客户端握手与帧 | a |
| `ring`（已有）`aes`（已有）`md-5` `sha3` `crc32fast`（已有） | VMess AEAD | b |
| `md-5`（同上）`tokio-util`（已有） | AnyTLS 的 `padding-md5`；`tokio-util` 供 `PollSender` | b |
| `hmac` `sha1` | Shadow TLS 的 HMAC-SHA1 | c |

`sha2`（SHA224 / SHA256）已是依赖。写计划时核对每个 crate 在 `Cargo.lock` 里新增的条目数，超出预期的先停下来报告（M1a 的 `p12-keystore` 先例）。

## 4. 配置层（`rurge-config::spec`）

### 4.1 数据模型

```rust
pub enum ProtoSpec {
    Direct, Reject(Builtin), Http(HttpSpec), Socks5(Socks5Spec),
    Trojan(TrojanSpec),   // M2a
    Vmess(VmessSpec),     // M2b
    AnyTls(AnyTlsSpec),   // M2b
}

pub struct WsOpts { pub path: String /* 默认 "/" */, pub headers: Vec<(String, String)> }

pub struct TrojanSpec { pub tls: TlsOpts, pub password: String, pub ws: Option<WsOpts> }

pub enum VmessCipher { Aes128Gcm /* 默认 */, ChaCha20Poly1305 }
pub struct VmessSpec { pub uuid: [u8; 16], pub cipher: VmessCipher,
                       pub tls: Option<TlsOpts>, pub ws: Option<WsOpts> }

pub struct AnyTlsSpec { pub tls: TlsOpts, pub password: String, pub reuse: bool /* 默认 true */ }

// M2c：挂在 PolicySpec 上，对所有 TCP 类协议有效
pub enum ShadowTlsVersion { V2 /* 默认 */, V3 }
pub struct ShadowTlsOpts { pub password: String, pub sni: Option<String>, pub version: ShadowTlsVersion }
pub struct PolicySpec { /* 现有字段 */ pub shadow_tls: Option<ShadowTlsOpts> }
```

spec 类型沿用 M1 的约定派生 `Debug`（测试断言要用；日志与诊断从不打印 spec）；持有凭据派生物的出站对象不实现 `Debug`。

### 4.2 校验（沿用现有诊断码，不新增）

| 情况 | 处理 |
| ---- | ---- |
| `trojan` / `anytls` 缺 `password`；`vmess` 缺 `username` | `E0018`。trojan 的 `password` 只接受命名写法（手册如此）；位置值不读，按多余的位置参数报 `W0001`。当初的另一半理由"`redact_profile` 只对 `http` / `socks5` 系抹位置凭据，接受位置口令会留下脱敏漏洞"在终审修复后不再成立（脱敏已覆盖所有写作 `type, server, port` 的类型），按手册的那一半不变。anytls 的同一句留给 M2b 的计划核对 |
| `vmess` 的 `username` 不是合法 UUID；`encrypt-method` 不是手册列的两个值 | `E0018`（文本不回显取值） |
| `ws-path` 不以 `/` 开头，或含控制字符 / 空白 | `E0018` |
| `ws-headers`：按 `\|` 切分、每项按第一个 `:` 切名值；名字不是 HTTP token，或值含 HTAB 以外的控制字符 | `E0018`（与 M1 的 `headers=` 同一规则、同一套函数）；`Connection` / `Upgrade` / `Sec-WebSocket-*` 由握手自己写，出现时 `W0012` 并忽略 |
| `ws=false`（或没写）却写了 `ws-path` / `ws-headers` | `W0028` |
| `vmess` 的 `tls=false` 却写了 TLS 参数 | `W0028`（复用 `refuse_tls`） |
| `shadow-tls-version` 不是 `2` / `3`；`version=3` 缺 `shadow-tls-sni`；`shadow-tls-sni` 不是主机名 | `E0018`（M2c） |
| Shadow TLS 写在非 TCP 协议上（QUIC 类 / WireGuard） | `E0018`（M2c 落判断函数；相应协议的 spec 出现时接上） |
| 只写了 `shadow-tls-sni` / `shadow-tls-version` 而没有 `shadow-tls-password` | `W0028`（没有密码即未启用） |

M2a、M2b 期间 Shadow TLS 三个参数仍按 M1 的约定处理（`W0029`：解析但尚未生效）；M2c 起生效，`W0029` 对它们退役。

### 4.3 VMess 旧握手

Surge 的 `vmess-aead` 默认 `false`，即默认是旧式 MD5 鉴权握手；而主流服务端基本只认 AEAD（xray 已移除旧握手，v2fly 默认关闭）。M2b：

- `vmess-aead=true` → 产生 `VmessSpec`。
- 否则 → 不产生 spec，加载时 `W0007`，文本点明原因：`` `vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet ``；运行期按 REJECT 处理，会话日志 `policy protocol not implemented: vmess (legacy handshake)`。M8 实现旧握手时这条随 `W0007` 一起退役。

### 4.4 脱敏与 `lineHash`

`SECRET_PARAMS` 增加 `ws-headers`、`ws-path`（M2a）与 `shadow-tls-password`（M2c）。`GET /v1/profiles/current`、`GET /v1/policies/detail` 与 `lineHash` 自动继承（`lineHash` 取脱敏后的定义，M1b 的 P6）。

### 4.5 默认 ALPN

`trojan`、`anytls`、`vmess` + `tls` 默认都**不带 ALPN**（与 M1 对 `https` 的处理一致），写了 `alpn` 才带。这也保证 `ws=true` 时不会被协商成 h2。未与真实 Surge 核对，登记进清单。

## 5. 传输层（`rurge_proto::transport`）

### 5.1 传输阶梯 `Stack`（M2a）

```rust
pub struct Stack {
    connector: Arc<dyn Connector>,
    server: Target,
    shadow_tls: Option<ShadowTlsClient>,   // M2c
    tls: Option<TlsClient>,
    ws: Option<WsClient>,
}
impl Stack {
    /// connect → shadow-tls → tls → ws，固定顺序（总设计 5.4）。
    pub async fn open(&self, opts: &ConnectOpts) -> Result<BoxedStream, OutboundError>;
}
```

- 超时：调用方把"整条阶梯 + 协议握手"包进**一个** `tokio::time::timeout(opts.timeout, ..)`，与 M1 的两个出站一致。
- 每层的错误带自己的前缀：`tls: …`（M1 已有）、`ws: …`、`shadow-tls: …`；连接层的 `io::Error` 原样上抛（`ChainConnector` 的 `via <名字>:` 前缀不变）。
- M2a 由 `trojan` 使用；M2b 给 `vmess` / `anytls`；M2c 把 M1 的 `http` / `socks5` 出站也迁过来（那时它们必须接上 Shadow TLS，才值得动）。

### 5.2 WebSocket（`transport::ws`，M2a）

- `tokio_tungstenite::client_async_with_config` 在任意 `BoxedStream` 上做客户端握手；TLS 是我们自己的下一层，所以不启用它的 TLS 特性。
- 请求：`GET <ws-path> HTTP/1.1`；`Host` 取 `ws-headers` 里的 `Host`，否则取策略的服务器主机名（端口非 80 / 443 时带端口）；其余 `ws-headers` 原样带上；`Upgrade` / `Connection` / `Sec-WebSocket-Key` / `Sec-WebSocket-Version` 由库生成，应答的 101 与 `Sec-WebSocket-Accept` 由库校验。`Host` 的缺省取值未与真实 Surge 核对，登记进清单。
- 适配器 `WsByteStream`（消息流 → 字节流，实现 `AsyncRead + AsyncWrite`）：
  - 读：Binary 帧的负载按序拼接；Text 帧视为协议错误；Ping 由库应答，适配器负责把应答刷出去；Close 视为 EOF。
  - 写：每次 `poll_write` 发一个 Binary 帧；`poll_shutdown` 发 Close 并刷出。
- 入站帧与消息的大小上限 ≤ 1 MiB，出站每帧 ≤ 64 KiB，超限视为协议错误；tungstenite 要求请求 URI 带 `ws://` scheme、且 `Host` `Connection` `Upgrade` `Sec-WebSocket-Version` `Sec-WebSocket-Key` 五个握手头各恰好一个，它的错误文本会引用头的值，所以一律按变体映射成固定文本。
- 错误文本：`ws: handshake failed: HTTP <状态码>`（只有状态码，对端把握手请求回了非 101 响应）；握手期间的其它错误统一为 `ws: handshake failed`；建立之后，协议错误统一为 `ws: protocol error`，对端发文本帧是 `ws: the server sent a text frame`，单帧或重组后的消息超过入站上限是 `ws: the server sent a frame larger than the limit`，写入或刷出发生在连接已经关闭之后是 `ws: the connection is closed`。tungstenite 自己的 `Display` 文本可能引用头的值，因此从不转发，一律按错误变体（或消息种类）映射成上面的固定文本；唯一的例外是 `WsError::Io`，它本来就是一个 I/O 错误，原样作为 `OutboundError::Io`（或超时时的 `OutboundError::Timeout`）继续走标准出站错误路径，不带 `ws:` 前缀。

### 5.3 Shadow TLS（`transport::shadow_tls`，M2c）

一个模块三件东西：

1. **TLS 记录读写器**：5 字节头 + 负载，长度先校验后分配，单条记录的上限按 TLS 规范（16 KiB + 扩展余量）。
2. **自己驱动的伪装握手**：直接驱动 `rustls::ClientConnection` 的 `read_tls` / `write_tls` / `process_new_packets`，不经 tokio-rustls——Shadow TLS 本来就要在记录层拦截、校验、改写服务端的字节之后才能交给 TLS 栈。
3. **握手后的帧化字节流**：应用数据装进 ApplicationData 记录（`0x17 0x03 0x03 <len>`），实现 `AsyncRead + AsyncWrite`。里层的真实 TLS（例如 trojan 自己的 TLS）就跑在这条字节流上。

**v2**：与伪装站点做一次真实的 TLS 握手，SNI 取 `shadow-tls-sni`（没写则用策略的有效 SNI 名；未核对，登记）；握手期间对收到的全部服务端字节做 HMAC-SHA1（密钥为 `shadow-tls-password`）；握手后客户端的第一帧带 8 字节摘要前缀，之后是普通帧。

**v3**：

- ClientHello 的 32 字节 SessionID：前 28 字节随机，后 4 字节是 HMAC-SHA1(password) 对"不含 5 字节记录头、且这 4 字节置零的 ClientHello"的摘要前 4 字节。做法见附录 A。
- 从 ServerHello 取 ServerRandom；握手期间服务端发来的每条 ApplicationData 记录带 4 字节 HMAC 前缀、负载与 `SHA256(password ‖ ServerRandom)` 异或：逐条验证、剥掉前缀、异或还原、改回记录长度，再交给 rustls。
- 数据阶段：双向每帧 `(5 字节记录头)(4 字节 HMAC)(负载)`；客户端方向的 HMAC 链以 `ServerRandom ‖ "C"` 起始，服务端方向以 `ServerRandom ‖ "S"` 起始；每帧把负载喂进 HMAC、取 4 字节放在帧首、再把这 4 字节喂回去。
- 要求伪装站点支持 TLS 1.3；ServerHello 不是 TLS 1.3 时以 `shadow-tls: the handshake server does not support TLS 1.3` 失败。
- 服务端发来的握手期记录没有正确的 HMAC（对端不是 Shadow TLS v3 服务端，或密码不对）：按参考实现的做法体面收尾后，以 `shadow-tls: the server did not authenticate itself` 失败。

**伪装握手的证书照常校验**（用出站的根证书库）。`skip-cert-verify`、`sni`、`server-cert-verify-name`、指纹、`alpn`、`client-cert` 六个参数只作用于里层的真实 TLS，不作用于伪装握手：一个对坏证书视而不见的客户端本身就是特征。测试与互操作经已有的 `EngineFactory::with_roots` 注入自己的根。

**与各协议的组合**：`connect → shadow-tls → tls → ws → 协议`。对 `http` / `socks5` / `vmess`（无 `tls`）这类没有里层 TLS 的组合，帧化字节流上直接跑协议。

逐字节细节（v2 摘要覆盖的确切字节范围、v3 HMAC 链的起始值与"体面收尾"的具体动作）在写 M2c 计划时对照参考实现（`ihciah/shadow-tls` 的协议文档与源码）逐项核对；第 1 层测试用依此推导、带出处说明的向量。

## 6. 协议（`rurge-proto`）

### 6.1 惰性请求头 `LazyHead`（M2a，trojan 与 vmess 共用）

`connect_tcp` 返回时还不知道第一段负载。返回的流先存着请求头：

- 第一次 `poll_write`：请求头与首段负载合成一次写出——避免"单独一个几十字节的首记录"这一已知的流量特征。
- 读在请求头未发出时先等一个宽限期 `HEAD_GRACE`（100 ms）：期间若有一次写，请求头与首段负载一起发出，并唤醒挂在计时器上的读；宽限期内一直没有写（SSH、SMTP、FTP 这类服务端先说话的协议）才把请求头单独刷出。原因：转发循环从隧道建立的那一刻起就在轮询"读"，早于客户端的第一段字节到达；按"应用先读就立刻单独发出"的原始设计，请求头会几乎总是单独发出，合并的意图落空。
- `poll_shutdown` 之前若请求头仍未发出，同样先刷出。

### 6.2 Trojan（M2a）

- 线上格式：`hex(SHA224(password))`（56 字节）`CRLF` `0x01`（CONNECT）`ATYP(1 / 3 / 4) ADDR PORT` `CRLF`，随后就是负载；服务端没有应答。
- 构建期算好 SHA224 的十六进制串；出站对象里只存它、不存口令。
- 叠层：`Stack { tls: Some, ws: 可选 }`；TLS 必有（`trojan` 没有明文形态）。
- **密码错误在连接期发现不了**：服务端会把认不出的连接转给它的回落站点，客户端只会在转发阶段看到连接被关或一段 HTTP。这是协议性质，如实登记。
- 目标主机名经 `hostname::to_ascii`（6.5）。
- `testing::trojan`：TLS 接入 + 请求头解析；可编排：哈希不对时像真实回落那样回一段 HTTP 400 再关、记录收到的请求、把流接到指定的回环目标。带 `ws` 的变体复用 `testing::ws`。

### 6.3 AnyTLS（M2b）

- TLS 之后的鉴权帧：`SHA256(password)`（32 字节）`‖ padding0 长度（u16 BE）‖ padding0`。
- 会话层帧：`命令(1) ‖ streamId(u32 BE) ‖ 长度(u16 BE) ‖ 数据`；命令号 0–10 按协议文档（`cmdWaste` `cmdSYN` `cmdPSH` `cmdFIN` `cmdSettings` `cmdAlert` `cmdUpdatePaddingScheme` `cmdSYNACK` `cmdHeartRequest` `cmdHeartResponse` `cmdServerSettings`）。
- 新会话先发 `cmdSettings`：`v=2`、`client=rurge/<版本>`、`padding-md5=<当前方案的 md5>`；收到 `cmdServerSettings` 才启用 v2 的语义（对 v1 服务端自然降级）。
- 开流：`cmdSYN`，接着第一个 `cmdPSH` 里是 SOCKS 风格的目标地址。**不等 `cmdSYNACK` 就返回流**：首段负载可以跟着 SYN 一起走，省一个往返，对 v1 / v2 服务端都成立；v2 的 SYNACK 带错误文本时，在该流的第一次读上以 `anytls: <untrusted_text>` 失败并关闭该流。
- `cmdAlert`：记 WARN（文本经 `untrusted_text`）并关闭会话。`cmdHeartRequest`：回 `cmdHeartResponse`；客户端不主动发心跳。
- **padding**：按方案处理每个会话的前 `stop` 个包（按方案给的长度切分 / 补足，`c` 标记处若已无用户数据则停止），填充用 `cmdWaste` 帧。默认方案取协议文档里的那份。方案属于出站对象、它的所有会话共享；`cmdUpdatePaddingScheme` 到达时解析并做有界校验（条目数与取值上限，写计划时定），通过则原子替换，不合法就保留旧方案并记一条 WARN。
- **会话复用**：与参考实现一致，一条会话同一时刻只承载一个流（相当于 HTTP/1.1 的 keep-alive，不是并发多路），因此没有跨流的队头阻塞。流正常结束（任一方的 `cmdFIN`、事件循环无错）→ 会话带时间戳回空闲池；**没有半关闭**：客户端或服务端任一侧发出 `cmdFIN` 就结束整条流的双向传输，不需要等对端回应。取用时选 `Seq` 最大（最新）的那条；回收任务每 30 秒清一次空闲超过 60 秒的会话（协议文档给的下限）。`reuse=false`：流结束就关会话，不进池。
- 池与回收任务归出站对象所有；最后一个 `Arc` 释放时回收任务中止、空闲会话关闭（总设计 5.5）。

### 6.4 VMess（M2b）

- **AEAD 请求头**：`cmdKey = MD5(uuid ‖ "c48619fe-8f02-49e0-b9e9-edf763e17e21")`；16 字节 AuthID 是 AES-128 对 `时间戳(8) ‖ 随机(4) ‖ CRC32(4)` 的单块加密（密钥由 KDF 从 `cmdKey` 派生）；头长度与头本体分别用 AES-128-GCM 密封，密钥与 nonce 由"嵌套 HMAC-SHA256"的 VMess KDF 从 `cmdKey`、AuthID、8 字节连接 nonce 派生，AAD 为 AuthID。线上顺序：`AuthID(16) ‖ 密封的长度(18) ‖ nonce(8) ‖ 密封的头`。
- 头本体：版本、body IV / Key、应答校验字节 V、选项、`填充长度 | 加密方式`、保留、命令（`0x01` TCP）、端口、地址（ATYP 1 / 2 / 3）、随机填充、FNV1a-32 校验。
- 选项：只开 ChunkStream + ChunkMasking（SHAKE128 掩码长度）——所有服务端都接受的组合；不开 GlobalPadding 与 AuthenticatedLength。Surge 的实际取值未公开，登记进清单。
- 数据分块：`长度(2，掩码) ‖ AEAD(负载)`；nonce = `计数(2，BE) ‖ iv[2..12]`；`encrypt-method` 决定 AES-128-GCM 还是 ChaCha20-Poly1305（后者的密钥为 `MD5(key) ‖ MD5(MD5(key))`）；空块表示流结束。
- 应答头：密钥 / IV 取自 body Key / IV 的 SHA256 前 16 字节，同样 AEAD 密封（长度、本体各一次）；校验 V 字节；应答里的动态端口指令忽略（与现行客户端一致）。
- 请求头经 `LazyHead` 与首段负载合并（6.1）。
- 时间戳取当前时间 ±30 秒内的随机值。本机时钟偏差超过服务端容忍（约 120 秒）时服务端直接断开，客户端分辨不出原因——写进文档的排错一节。
- 叠层：`Stack { tls: 可选（tls=true）, ws: 可选 }`。
- 逐字节细节在写 M2b 计划时对照 v2fly / sing-vmess 的源码核对；第 1 层测试用从参考实现抓取、带出处说明的向量（KDF、头密封、一段分块流）。

### 6.5 目标主机名

三种协议都把目标主机名写进二进制的长度前缀字段，没有文本注入面；仍统一走 M1b 的 `hostname::to_ascii`（IDN → A-label，只允许 `[A-Za-z0-9._-]`，长度 ≤ 255）。一条规则、清单里一处登记：任何出站都不会向远端说出一个规则引擎没见过的名字。

## 7. 出站的生命周期：重载时按指纹复用（M2b，总设计 5.5 / D8）

### 7.1 指纹与复用

```rust
// rurge-policy
impl PolicyRegistry {
    pub fn build(cfg: &Config, factory: &dyn OutboundFactory, cell: &Arc<RegistryCell>,
                 selections: Arc<SelectionTable>, previous: Option<&PolicyRegistry>)
        -> Result<PolicyRegistry, BuildError>;
}
pub trait OutboundFactory: Send + Sync {
    // 现有的 direct_connector / build 不变
    /// 出站在构建期按值捕获的、随配置代际变化的工厂输入；变了就全部重建。
    fn environment(&self) -> String;
}
```

- 指纹 = 去掉 `span` 的 `PolicySpec`（行号会因无关的增删而变，不能算进去）+ 被引用的 Keystore 条目的内容摘要 + `factory.environment()`。
- 注册表的每个出站条目记着自己的指纹。构建新一代时，名字相同且指纹相等 → 沿用上一代的同一个 `Arc`；否则新建。M1 的 `http` / `socks5` 按同一规则复用（统一规则，不为"无状态"开特例）。
- 连接器的指纹不必另算：直连的 socket 选项与 `underlying-proxy` 的名字都在 `PolicySpec.common` 里；链式连接器经跨代稳定的 `RegistryCell` 按名字解析。
- 干构建（`dry_build`）不传 `previous`、不复用、不留任何后台任务。
- 旧一代里没被复用的出站，随最后一个持有它的会话结束而释放（AR-04）；它的后台任务随之中止。

### 7.2 `ResolverCell`

每次重载都会重建整个解析器（`[Host]`、上游、缓存策略都可能变了），而被复用的出站里的 `DirectConnector` 还攥着上一代的解析器。解法：

- `EngineShared` 增加 `resolver: Arc<ResolverCell>`；`ResolverCell` 实现 `rurge_net::Resolve`，内部以 `ArcSwap` 指向当前一代的解析器。
- `EngineFactory` 造的所有连接器都拿 `ResolverCell`，不再直接拿某一代的解析器。
- 它与注册表在同一个发布点一起切换：`Engine::publish_registry` 扩成 `publish_generation(registry, resolver)`；"注册表必须出自引擎自己的 cell"的 `assert!` 保留（M1b 终审的裁定不变）。
- `environment()` 目前只含按值捕获的 `[General]` 项（写计划时核对 `EngineFactory` 的字段后定稿）；经 cell 动态读取的东西（解析器）不进指纹。

### 7.3 用例

- 改一条无关策略后重载：anytls 的空闲会话仍在池里，出站 `Arc` 指针相等。
- 改它自己的任一参数（或它引用的 Keystore 条目的内容）后重载：出站换新，旧池随旧对象释放。
- 重载改了解析器配置后：被复用的出站按新一代解析。
- 链式会话进行中重载：在途会话继续用旧出站，新拨号走新一代（第 9 节第 2 条，M2a 先以"不复用"的语义落地，M2b 复核）。

## 8. 引擎、bin 与 API

- `EngineFactory::build` 认识三种新协议：`ProtoSpec::Trojan`（M2a）、`Vmess` / `AnyTls`（M2b）；Shadow TLS 在 M2c 由 `Stack` 统一接上。`dry_build` 自动覆盖它们（构建期可失败的点：p12、TLS 配置、Shadow TLS 的 provider 自检）。
- `Engine::dial_internal` 的防环回退不需要改：它按"要在本机解析其服务器名的那一跳"判断，与协议无关；M2a 加一条"以域名配置的 trojan 命中 DNS 会话"的端到端用例钉住这一点。
- bin：能力表逐协议翻转（M2a：`Trojan`；M2b：`Vmess`、`AnyTls`）。翻转是让死路径变活的时刻——每次翻转的任务里，回头核对设计承诺过的守卫是否真的存在（M1b 的 Critical 就漏在这里）。
- `rurge-api`：没有新端点；`policies/detail` 与 `profiles/current` 的脱敏随 4.4 自动变化；`docs/api/phase2.md` 更新脱敏名单与会话日志的新错误文本。

## 9. M1b 承接事项

| # | 事项 | 处理 | 计划 |
| - | ---- | ---- | ---- |
| 1 | `server-cert-verify-name` 只在标准校验分支解析 | `read_tls` 按主机名校验（非法 `E0018`）；`TlsClient::build` 在选择校验模式之前解析；与 `server-cert-fingerprint-sha256` 或 `skip-cert-verify` 同时出现时给一条 `W0012`（"ignored because …"，比照现有的那条） | a |
| 2 | 两个缺失用例 | (a) 链式会话进行中重载：在途会话继续用旧一代的出站，新拨号走新一代；(b) 保存的选择指向新一代里已不存在的成员：回落到第一个成员，用例钉住 | a |
| 3 | `interface` / `allow-other-interface` / `tos` / `ip-version` 与 `underlying-proxy` 同时出现时静默失效 | 加载时 `W0028`：`` `interface` has no effect on a policy with `underlying-proxy`; ignored ``；运行期行为不变 | a |
| 4 | "防环回退对『服务器名能被 `[Host]` 回答』的代理偏保守" | **前提不成立，关闭**：解析器把代理服务器主机名排除在 `[Host]` 之外（`rurge-dns` 的 `proxy_hostnames`，清单 6.3，有用例钉着）；`use-local-host-item-for-proxy` 管的是目标主机名。以域名配置的代理永远需要一次真实查询，旁路是精确的。订正 M1b 计划「延后事项」表里的那一行 | a（文档） |
| 5 | `publish_registry` 的 `assert!` | 保留；M2b 把它扩成 `publish_generation`（7.2） | b |

## 10. 错误处理、可观测性与安全

| 层 | 策略 |
| -- | ---- |
| 配置 | 4.2 的分级；错误时 `run` 拒绝启动、`reload` 保留旧一代（不变） |
| 构建 | `BuildError` → `E0022`（不变）；新的构建期失败点：Shadow TLS 的 provider 自检 |
| 拨号 | `OutboundError::{Proxy, Tls, Io, Timeout}` → 请求记录的 `error`；文本前缀 `trojan:` `vmess:` `anytls:` `ws:` `shadow-tls:` |
| 对端文本 | 一律经 `untrusted_text`（去控制字符、有界） |
| 凭据 | 口令、SHA224 / SHA256 哈希、UUID、`cmdKey`、Shadow TLS 的 HMAC 与异或密钥——永不出现在错误、日志、诊断、API 与 `Debug` 输出里 |
| 缓冲 | 长度先校验后分配：ws 帧 / 消息上限、anytls 帧 ≤ 65535（格式所限）与 padding 方案的上限、vmess 分块与 TLS 记录的规范上限 |
| 等待 | 整条阶梯一个超时；anytls 回收任务定时；没有无界等待；任一方向以错误结束时，另一方向随之结束（M2b：转发循环的 flush 与跨方向取消两处修正） |
| 后台任务 | anytls 的回收任务与会话读循环：随所属对象释放而中止；panic 由任务边界隔离并记 ERROR |

## 11. 测试策略（总设计第 13 节的三层；全部安全约束不变）

**第 1 层　向量与属性测试**：trojan 的 SHA224；`WsByteStream` 与 TLS 记录编解码的属性测试（任意切片方式喂入，结果一致）；VMess 的 KDF / 头密封 / 分块流（参考实现抓取的向量，fixture 带出处）；anytls 的 padding 方案解析与 md5；Shadow TLS 的 HMAC 链；附录 A 的自检用例（rustls 升级的绊线）。

**第 2 层　回环假服务端**：`rurge_proto::testing::{ws, trojan, vmess, anytls, shadow_tls}`（cargo feature `testing`）。覆盖成功与失败路径：口令 / UUID 错、握手截断、超时、超大帧、`cmdAlert`、带错误的 SYNACK、padding 方案更新、空闲会话的复用与过期（暂停的 tokio 时钟，不用真实 sleep 当同步手段）。引擎端到端（`crates/rurge-engine/tests/outbounds.rs`）：每种协议经真实流水线、经 `underlying-proxy` 的链、以域名配置的 trojan 命中 DNS 会话的防环、第 7.3 节的复用用例。

**第 3 层　互操作**（`tests/interop`）：

| 计划 | 参考实现 | 用例 |
| ---- | -------- | ---- |
| a | sing-box 1.14.1 | trojan；trojan + ws |
| b | sing-box 1.14.1；xray（固定版本 + SHA256，写计划时选定） | vmess（± tls、± ws）、anytls；xray：vmess（± ws） |
| c | sing-box 1.14.1 | 前置 shadowtls v2 / v3 入站的 trojan |

夹具的安全守卫照旧：只监听 `127.0.0.1`、所有目标是回环 IP 字面量、配置里绝不出现 `set_system_proxy` / `tun` / `auto_route`（xray 的配置同样只含回环入站与 `freedom` 出站）；本机缺二进制 → 打印说明并跳过；CI 设 `RURGE_INTEROP_REQUIRED=1`。**不在开发者的机器上下载或安装任何参考二进制**：互操作由首次推送后的 CI 证明。

测试只用回环 + 端口 0 + 有界等待；不碰公网、不改本机系统代理、不注册服务。每个任务结束的门禁不变。

## 12. 验收标准

1. `trojan`（± ws）、`vmess`（AEAD，± tls，± ws）、`anytls`（`reuse` 两种取值）对回环假服务端的成功与失败路径通过；对 sing-box 转发通过；vmess 另对 xray 转发通过。
2. Shadow TLS v2、v3 各对回环假服务端与 sing-box 的 shadowtls 入站转发通过；附录 A 的自检有用例钉住。
3. 六个 TLS 参数在新协议上各有一条用例（沿用 M1 的 TLS 夹具）。
4. 三种协议各能作链的入口与出口（`underlying-proxy`）。
5. 重载复用的四条用例（7.3）通过。
6. 第 9 节的承接事项全部落地。
7. 凭据不外泄的断言覆盖三种协议的错误文本、`policies/detail`、`profiles/current` 与日志。
8. fmt / clippy 零警告 / `cargo test --workspace` 全绿；CI（含互操作 job）在首次推送后为绿。
9. 需要真实公网节点的项目进 `docs/acceptance/phase2-manual.md`，由项目所有者手工验收。

## 13. 兼容性清单需登记的差异

| 位置 | 内容 |
| ---- | ---- |
| 4.2 `trojan` | M2a 已实现（TCP）；密码错误在连接期无法识别（协议性质）；默认不带 ALPN（未核对）；UDP 属 M5 |
| 4.2 `vmess` | M2b 已实现 AEAD 握手；没写 `vmess-aead=true` 的行在 M8 之前按 `W0007` + REJECT 处理；只开 ChunkStream + ChunkMasking（Surge 的取值未公开）；时钟偏差导致的断开无法识别 |
| 4.2 `anytls` | M2b 已实现；一条会话同一时刻一个流（与参考实现一致）；空闲 60 秒回收；不等 SYNACK |
| 4.4 Shadow TLS 三行 | M2c 已实现；v3 用 stock rustls 两遍构造 ClientHello，伪装握手只提供 X25519、要求 TLS 1.3；伪装握手照常校验证书、不受六个 TLS 参数影响；`shadow-tls-sni` 缺省取策略的有效 SNI（v2，未核对） |
| 4.6 `ws` `ws-path` `ws-headers` | `Host` 缺省取服务器主机名（未核对）；`ws-path` / `ws-headers` 的字符限制；不支持 early data |
| 4.3 `interface` `allow-other-interface` `tos` `ip-version` | 与 `underlying-proxy` 同时出现时 `W0028` |
| 4.4 `server-cert-verify-name` | 配置期校验；与指纹 / `skip-cert-verify` 同时出现时 `W0012` |
| 10.4 `profiles/current`、`policies/detail` | 脱敏名单增加 `ws-headers` `ws-path` `shadow-tls-password` |
| `W0007` 那一行 | 逐协议移除：M2a `trojan`；M2b `vmess`（AEAD）`anytls` |
| 所有出站 | 目标主机名的字母表规则同样适用于三种新协议 |

## 14. 对其它文档的订正

| 文档 | 订正 | 时机 |
| ---- | ---- | ---- |
| 阶段 2 总设计 1.4 | M2 行注明拆成 M2a / M2b / M2c，Trojan 最先 | 随本文件 |
| 阶段 2 总设计第 2 节 | Shadow TLS 一行：v2 与 v3 都在 stock rustls 上实现（v3 用两遍构造 ClientHello） | 随本文件 |
| 阶段 2 总设计第 13 节 | M2 行：xray 只用于 vmess；trojan + ws 由 sing-box 覆盖 | 随本文件 |
| 阶段 2 总设计第 15 / 16 节 | 风险 A、Q2 改为"已决（M2-D3）" | 随本文件 |
| 阶段 2 总设计第 17 节 | `docs/acceptance/phase2-manual.md` 的创建时机改为 M2a | 随本文件 |
| M1b 计划「延后事项」表 | 第 9 节第 4 条对应的那一行改为"前提不成立，关闭" | M2a 的文档任务 |
| `CLAUDE.md` | 「先读这些文档」加入本文件；状态一节随各计划收尾更新 | 随本文件；各计划收尾 |

## 15. 写计划时必须核对的事项

| 编号 | 事项 | 计划 |
| ---- | ---- | ---- |
| V1 | `tokio-tungstenite` / `tungstenite` 的确切版本、`client_async_with_config` 对自带 `http::Request` 要求调用方提供哪些头、`WebSocketConfig` 的上限字段名与取值（5.2 的帧 / 消息上限）、给 `Cargo.lock` 新增的条目数（已核对：M2a 计划 P1–P3、P7 / P10） | a |
| V2 | sing-box 1.14.1 的 trojan 入站与 ws 传输的配置写法（发布版是否需要额外的构建标签）（已核对：M2a 计划 P1–P3、P7 / P10） | a |
| V3 | VMess AEAD 的逐字节格式与 KDF 标签串（对照 v2fly / sing-vmess 源码）；向量的出处（已核对：M2b 计划 P1 / P5 / P9 / P10） | b |
| V4 | AnyTLS v2 的逐字节格式、默认 padding 方案、`padding-md5` 的算法（对照 anytls-go 的协议文档与源码）；`cmdUpdatePaddingScheme` 的有界校验取值（6.3：条目数与单项长度的上限）（已核对：M2b 计划 P1 / P5 / P9 / P10） | b |
| V5 | xray 的固定版本、三个平台的包名与 SHA256、只含回环入站的最小配置（已核对：M2b 计划 P1 / P5 / P9 / P10） | b |
| V6 | `EngineFactory` 按值捕获的字段清单 → `environment()` 的内容（已核对：M2b 计划 P1 / P5 / P9 / P10） | b |
| V7 | Shadow TLS v2 摘要覆盖的确切字节范围、v3 HMAC 链的起始值与帧格式、"体面收尾"的动作（对照 `ihciah/shadow-tls` 的文档与源码）；sing-box shadowtls 入站的配置写法 | c |
| V8 | 附录 A 的两条假设在所用的 rustls 版本上仍成立（spike 的断言即检查项） | c |

## 16. 任务草图

**M2a（约 8 个任务）**：① 配置层：`WsOpts` / `TrojanSpec`、脱敏、承接事项 1 与 3 → ② `transport::Stack` + `transport::ws` + `testing::ws` → ③ `LazyHead` + trojan 出站 + `testing::trojan` → ④ 引擎装配与端到端用例（含链、防环）→ ⑤ 承接的两个缺失用例 → ⑥ 能力表翻转与 CLI 用例 → ⑦ 互操作（sing-box：trojan、trojan + ws）→ ⑧ 文档（清单、README、CLAUDE.md、`phase2-manual.md`、M1b 计划表的订正）。

**M2b（约 11 个任务）**：① vmess 配置（含 4.3）→ ② vmess 编解码与向量 → ③ vmess 出站与假服务端 → ④ anytls 配置 → ⑤ anytls 会话层与 padding → ⑥ anytls 的池、出站与假服务端 → ⑦ `ResolverCell` 与 `publish_generation` → ⑧ 按指纹复用 → ⑨ 引擎装配、端到端、能力表翻转 → ⑩ 互操作（sing-box + xray，CI）→ ⑪ 文档。

**M2c（约 7 个任务）**：① 配置与约束 → ② TLS 记录编解码与帧化流 → ③ v2 → ④ v3（附录 A）→ ⑤ 假服务端、接入 `Stack`、迁移 http / socks5 出站 → ⑥ 互操作 → ⑦ 文档。

## 17. M2a 实施期的订正

本节登记 M2a 计划的「计划期决定」（`global-constraints.md` P1–P14）里与本文件文字不同的地方，以及实施期核对源码（`crates/rurge-proto/src/{trojan,transport/ws,transport/lazy_head}.rs`、`crates/rurge-config/src/spec/{tls,ws,trojan}.rs`）后发现的出入。逐条对应实现的提交见 `docs/superpowers/plans/2026-09-20-phase2-m2a-trojan-plan.md` 末尾「执行期修正记录」。

| 编号 | 设计原文 | 订正 |
| ---- | -------- | ---- |
| P5 | 4.1："凭据字段的 `Debug` 输出一律抹掉（与 `HttpSpec` 的做法一致）" | spec 类型沿用 M1 的约定派生 `Debug`（测试断言要用；日志与诊断从不打印 spec）；持有凭据派生物的出站对象不实现 `Debug` |
| P4 | 4.2 第一行："`password` 接受位置参数（沿用 `read_credentials` 的"命名优先于位置"）" | trojan 的 `password` 只接受命名写法（手册如此）；位置值不读，按多余的位置参数报 `W0001`。计划期的另一半理由（`redact_profile` 只对 `http` / `socks5` 系抹位置凭据）在终审修复后不再成立：脱敏已覆盖所有写作 `type, server, port` 的类型；决定按手册不变。anytls 的同一句留给 M2b 的计划核对 |
| P2 | 4.2 `ws-headers` 一行只写 `E0018`（与 M1 的 `headers=` 同一规则、同一套函数） | 追加：`Connection` / `Upgrade` / `Sec-WebSocket-*` 由握手自己写，出现时 `W0012` 并忽略 |
| P14 | 6.1："若应用先读（SSH、SMTP、FTP 这类服务端先说话的协议）：第一次 `poll_read` 之前先把请求头单独刷出，否则死锁" | 读在请求头未发出时先等 `HEAD_GRACE`（100 ms）；期间有写 → 头与首段负载一起发出并唤醒挂起的读；一直没有写 → 头单独发出。原因：转发循环从隧道建立起就在轮询读，远早于客户端第一段字节到达，按设计原文请求头会几乎总是单独发出，合并的意图落空 |
| P7 | 5.2："帧与消息的大小上限取有界值（写计划时定具体数字；量级为 1 MiB）" | 入站帧与消息 ≤ 1 MiB，出站每帧 ≤ 64 KiB；并补一句（P2 / P3）：tungstenite 要求请求 URI 带 `ws://` scheme、五个握手头各恰好一个；它的错误文本会引用头的值，所以一律按变体映射成固定文本 |
| 任务 2 | 5.2 只说"写：每次 `poll_write` 发一个 Binary 帧"，未说明成功返回是否意味着字节已到达下一层 | `WsByteStream` 是写穿的：`poll_write` 在报告成功之前会驱动 tungstenite 自己的写缓冲一并刷出（进而推动下层，如内层 TLS），调用方不需要、工作区里也没有任何调用方会再显式 `flush` |
| 任务 3 | 6.1 只说"第一次 `poll_write`：请求头与首段负载合成一次写出"，未说明请求头一旦开始发送之后能否继续并入后续的写 | `LazyHead` 一旦开始发送请求头（第一次内层 `poll_write`）就不再增长：`coalesced` 是"这些字节已随请求头发出"的唯一依据，之后任何一次读或写都不会让同一段负载被再发送一次 |
| 任务 8 | 5.2 错误文本一句写着 `ws: <untrusted_text(库的错误文本)>`，暗示握手 / 运行期错误会转发 tungstenite 自己的（经 `untrusted_text` 处理过的）文本 | 代码里没有这个机制：`transport/ws.rs` 从不调用 `untrusted_text`，每个 tungstenite 错误变体都映射成固定文本（`ws: handshake failed`，非 101 响应时带 `: HTTP <状态码>`；`ws: protocol error`；`ws: the server sent a text frame`；`ws: the server sent a frame larger than the limit`；`ws: the connection is closed`），唯一的例外是 `WsError::Io`，它原样作为普通 I/O 错误继续走 `OutboundError::Io` / `Timeout`，不带 `ws:` 前缀。Task 8 评审发现 |

实施中发现的新出入由各任务追加。

## 18. M2b 实施期的订正

本节登记 M2b 计划的「计划期决定」（`global-constraints.md` P2、P3、P5、P6、P7、P13、P16、P17、P18）里与本文件文字不同的地方，以及实施期新发现的两处出入。逐条对应实现的提交见 `docs/superpowers/plans/2026-09-20-phase2-m2b-vmess-anytls-plan.md` 末尾「执行期修正记录」。

| 编号 | 设计原文 | 订正 |
| ---- | -------- | ---- |
| P2 | 第 3 节新依赖表："`aes` `aes-gcm` `chacha20poly1305` `hmac` `md-5` `sha3` `crc32fast` \| VMess AEAD" | AEAD 改用 `ring`（rustls 已经把它带进 `Cargo.lock`，零新增条目，汇编实现）而非 `aes-gcm` + `chacha20poly1305`；AuthID 的单块加密用已有的 `aes = "0.8"`；不需要 `hmac`（`hmac` crate 表达不了"HMAC 套 HMAC"，嵌套 HMAC 手写，20 行，向量钉住）；新增的只有 `md-5` `sha3` 及其依赖 `keccak`（`Cargo.lock` 预计新增 3 个条目）；`tokio-util`（工作区已有）随 AnyTLS 一并引入，取 `PollSender` |
| P3 | 6.4 只给出分块格式"长度(2，掩码) ‖ AEAD(负载)"，未定具体的大小上限与流结束语义 | 写：负载 ≤ 16368 字节（密封后 ≤ 2^14）；读：接受 16 ..= 65535 的任何长度；传输层在分块边界上的 EOF 视为流结束（对端没发空块），分块中间的 EOF 是 `UnexpectedEof`；应答头之前就 EOF 是 `vmess: the server closed the connection without answering`（UUID 错、时钟偏差超过约 120 秒都表现为这个，服务端从不说明原因）；选项固定 `0x05`（ChunkStream + ChunkMasking） |
| P5 | 6.3："`cmdUpdatePaddingScheme` 到达时解析并做有界校验（条目数与取值上限，写计划时定）" | 原文 ≤ 8192 字节且是 UTF-8、`stop` ≤ 256、每个包 ≤ 64 项、每项是 `c` 或 `a-b`（1 ≤ 值 ≤ 16384，一条 TLS 记录的上限）；不是 `stop` 也不是包序号的键忽略；任何一条不满足 → 保留旧方案 + 一条 WARN |
| P6 | 6.3 原文"双向 FIN" | AnyTLS 没有半关闭：`cmdFIN` 结束整条流，收到对端的 FIN 不需要回 FIN（协议文档 2025-09 的澄清）；`poll_shutdown` = 发 `cmdFIN` 并让本端的读立刻返回 EOF（sing-box 对没有 `CloseWrite` 的连接就是这么做的）。已直接改写 6.3 正文（本节） |
| P7 | 6.3 未说明会话层的后台任务如何组织、也未提是否有 SYNACK 超时看门狗 | 一条会话一个任务，独占 TLS 流（`tokio::io::split`，读循环与写循环在同一个 `select!` 里）；流句柄经两条有界队列（各 8）与任务通信，写用 `tokio_util::sync::PollSender`；任务每批写完自己 `flush`；多个任务共享一个 `AsyncWrite` 会互相顶掉唤醒者，所以不这么做；空闲时任务照常读（心跳有人答、对端关连接能被发现）；**不实现参考客户端的 3 秒 SYNACK 看门狗**（复用到一条"半死"的空闲会话时由转发阶段的空闲超时兜底） |
| P13 | 4.3 只说"加载时 W0007"，未说明重复出现时报几条、注册表的说明文本如何得来 | 像 `inert` 那样每次加载只报一条（`to_spec` 给 `SpecOutcome` 加 `legacy_vmess: bool`）；运行期文本：成功加载的配置里，没有 spec 的 `vmess` 策略只可能是没写 `vmess-aead=true` 的一种（有错误的配置根本加载不了），所以注册表对这种条目的说明文本直接取 4.3 给出的 `vmess (legacy handshake)` |
| P16 | （承接自 M1b / M2a：转发循环在 `write_all` 之后不 flush，此前的设计文本未处理） | 真缺陷而不只是风格问题：`tokio-rustls` 的 `poll_write` 在 socket 写不动时会在"明文已收下、密文还留在自己缓冲里"的状态下返回成功，若没有下一次写这段尾巴就一直留着；`copy_half` 改为 `write_all` 之后 `flush`（同样与 `stop` 竞争）；同一个任务里顺带修一个相邻的缺口：一个方向以错误结束时，另一个方向不会跟着结束（改为 `tokio::join!` 等两边）——vmess 的"没应答就关"与 anytls 的被拒 / 会话死亡都以读错误的形式出现，没有这一条它们只会表现为卡住 |
| P17 | 4.2 表："`vmess` 的 `tls=false` 却写了 TLS 参数 \| `W0028`（复用 `refuse_tls`）" | 仍是 `W0028`，但不复用 `refuse_tls` 的文本（"does not apply to `vmess` policies" 会误导，因为 `vmess` 本来就可以用 TLS，只是这次没开）：改用 `` `<key>` has no effect without `tls=true`; ignored `` |
| P18 | （无对应设计文字：预检阶段的三处实现细节订正） | ① 只被单元用例用到的 `Pool::len` 标 `#[cfg(test)]`、只有假服务端会发的 `frame::SERVER_SETTINGS` 标 `#[cfg(any(test, feature = "testing"))]`；② `FakeVmess` 拒绝连接时先关写端、再把连接读到头（带着未读字节关连接会变成 RST，客户端拿到的就是 I/O 错误而不是"没应答就关"）；③ 引擎端到端用例里等"anytls 会话回池"的信号改为"会话记录出现"（`get` 带 `Connection: close`，先结束流的是服务端，对端的 FIN 不回，服务端的 FIN 计数在那两条用例里永远是 0） |
| 6（执行期） | 第 10 节"等待"一行原文："整条阶梯一个超时；anytls 回收任务定时；没有无界等待" | 转发循环的两处修正（`flush`、跨方向取消）属于 M2b 的交付；已在第 10 节"等待"一行补一句"任一方向以错误结束时，另一方向随之结束" |
| 8（执行期） | （无对应设计文字：出站复用机制的副作用） | 复用之后，`skip-cert-verify` 的 WARN 不再随每次重载重复：这条 WARN 是 `EngineFactory::build` 的副作用，被复用的出站不再经过 `build`，因此不再告警；首次构建与参数变更后的重建仍照常告警（已裁定接受） |

## 附录 A　Shadow TLS v3：在 stock rustls 上签名 ClientHello

**问题**：v3 要求 SessionID 的后 4 字节是对整个 ClientHello 的 HMAC，而 SessionID 属于握手转录的一部分——发出去之后再改，双方的转录哈希就不一致，握手必然失败；rustls 又没有设置 SessionID 的钩子。

**观察**（读 rustls 0.23.43 源码，`client/hs.rs`）：一个 rustls ClientHello 里的一切随机性只来自两处可替换的公开扩展点——`CryptoProvider::secure_random`（SessionID、client random、扩展顺序的种子）与 `SupportedKxGroup::start()`（key share）；而且 ClientHello 在 `ClientConnection::new` 里同步生成。

**做法**（同一线程、同步、中间没有 await）：

1. 自定义 `CryptoProvider`：`secure_random` 换成"脚本化随机源"（未激活时原样委托给 ring 的随机源），`kx_groups` 只放一个包装了 ring X25519 的"脚本化密钥交换组"。脚本状态放在线程局部变量里。
2. 第一遍（录制）：构造一个用完即弃的 `ClientConnection`；随机源把每次取到的字节记到带子上；密钥交换组启动真实的 X25519、把它**寄存**起来，只把公钥交给这条弃用的连接。取出 ClientHello 的字节。
3. 对"去掉 5 字节记录头、SessionID 后 4 字节置零"的 ClientHello 算 HMAC-SHA1，取前 4 字节；在带子上找到恰好等于 SessionID 的那 32 字节，把后 4 字节换成它。
4. 第二遍（重放）：再构造一次 `ClientConnection`；随机源按带子回放，密钥交换组交出寄存的那个真实密钥交换。得到的 ClientHello 与第一遍只差那 4 个字节，HMAC 因此成立；而它就是这条真实连接自己的 ClientHello，转录一致，握手照常完成。
5. 关闭脚本。之后这条连接里的随机性（以及未激活脚本时的任何连接）都走原来的随机源。

**运行期自检**（任何一条不满足 → `shadow-tls: cannot sign the ClientHello`，绝不发出带错误 HMAC 的连接）：ClientHello 恰好是一条握手记录、SessionID 长 32；SessionID 在带子上恰好出现一次；第二遍恰好用完带子、取走了寄存的密钥交换；两遍的 ClientHello 只在那 4 个字节上不同。这四条同时是单元测试——rustls 升级若破坏了假设，测试直接变红。

**限制**（登记进清单）：伪装握手只提供 X25519 一个密钥交换组（服务端发 HelloRetryRequest 要求别的组时握手失败；主流站点都支持 X25519）；关闭会话恢复；只用于 Shadow TLS 的伪装握手，里层的真实 TLS 与工作区里其它 TLS 用法不受影响（它们不用这个 provider）。

**spike 记录**（2026-09-20，一次性临时工程，未进仓库）：rustls 0.23.43 + ring 0.17.14 + rcgen 0.14.7，离线编译；50 轮，每轮：签名后的 HMAC 校验通过、换密码不通过、与内存里的 rustls TLS 1.3 服务端完整握手成功并收发应用数据；未激活脚本时的连接同样握手成功。
