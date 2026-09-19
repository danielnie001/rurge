# 阶段 2 / M1「出站地基与 HTTP / SOCKS5 上游」设计

> 状态：已与项目所有者逐节确认（2026-09-19）。本文件是 M1a / M1b 两份实施计划的依据；与阶段 2 总设计（`2026-09-19-phase2-outbound-groups-design.md`）不一致处以本文件为准，并在第 11 节登记。Surge 行为以 2026-09 版手册为准（`policies/parameters` `policies/tls` `policies/http` `policies/socks5` `profile/keystore` `tools/http-api` 各页已逐条核对）。

## 1. 目标与范围

### 1.1 目标

- 让 `[Proxy]` 里的 `http` `https` `socks5` `socks5-tls` 策略真正可用（TCP），包括两级及以上的 `underlying-proxy` 链、六个 TLS 参数与 p12 客户端证书。
- 建好后面七个里程碑共用的地基：类型化的策略参数、带 socket 选项的连接层、TLS 层、出站工厂、能装下真实出站的注册表、跨代稳定的注册表单元与选择表、干构建。
- 补全 `select` 组的控制面：经 HTTP API 读取与切换，切换立即生效并持久化。
- 建好三层测试的基础设施：可编排的回环假上游、对 sing-box 的互操作夹具。

### 1.2 范围内（PRD 编号）

| 需求 | 本里程碑覆盖的部分 |
| ---- | ------------------ |
| FR-CFG-11 | `[Keystore]` 的 `p12`：引用校验、Base64 校验、构建期解码、用于 `client-cert` |
| FR-OUT-03 | `interface` `allow-other-interface` `ip-version` `tos` `underlying-proxy` 生效；其余通用参数（含 `tfo`，见 5.1）解析并校验取值 |
| FR-OUT-04 | 六个 TLS 参数（Shadow TLS 属 M2） |
| FR-OUT-05 | `http` `https` `socks5` `socks5-tls` 的 TCP |
| FR-OUT-08 | TCP 的链式代理；底层可以是策略或组；代理主机名远程解析 |
| FR-OUT-09 | 出站网卡绑定与 `allow-other-interface` |
| FR-GRP-05（部分） | 嵌套 `select` 组照常解析；环与空组的语义到 M3 才改（见 1.3） |
| FR-GRP-06（部分） | `select` 的选择经 API 切换并按 Profile 持久化 |
| FR-DNS-07 | `use-local-host-item-for-proxy` |
| 清单 10.4 | `GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET` / `POST /v1/policy_groups/select` |

### 1.3 范围外

| 项 | 去向 |
| -- | ---- |
| `dns-follow-interface`（FR-DNS-10） | M5。M1 解析并告警"尚未生效"（理由见 11 节 C2） |
| 出站按指纹跨代复用 | M2（M1 的四种协议没有长生命周期状态） |
| `udp-relay`、UDP 载体、`Outbound::connect_udp` | M5（`DirectConnector` 的 UDP 载体随 M4 的 WireGuard） |
| `test-url` `test-timeout` 的使用、请求记录的计时字段 | M3 |
| 组的环降级为告警、空组回退 DIRECT、自动组算法 | M3（M1 保持阶段 1 的行为：环是 `E0009`，空组与自动组按现状处理） |
| "策略存在但不可用 → REJECT" | M3（订阅导入项才需要；主配置里的构建失败是配置错误，见 6.4） |

### 1.4 子里程碑

| 子里程碑 | crate | 内容 | 验收 |
| -------- | ----- | ---- | ---- |
| **M1a 配置与出站库** | `rurge-config` `rurge-net` `rurge-proto` `rurge-platform`（外加 `rurge-dns` `rurge-engine` 里因 `ConnectOpts` 变化而必须同步的几行） | `spec` 模块与诊断码、Keystore 校验、`rurge_net::socket` 与竞速的 `DirectConnector`、TLS 层、p12 解码、`http(s)` / `socks5(-tls)` 出站、`rurge_proto::testing` 假上游 | 库级测试全过；除 DIRECT 的建连由顺序尝试改为竞速外，`rurge run` 的行为不变（能力表未翻转，四种协议仍 `W0007` + REJECT） |
| **M1b 装配与控制面** | `rurge-policy` `rurge-engine` `rurge-inbound` `rurge-dns` `rurge-api` bin、`tests/interop` | 工厂、注册表、`RegistryCell` / `ChainConnector`、`SelectionTable`、引擎拨号改造、绝对 URI 转发接线、干构建、四个端点、能力表翻转、互操作夹具与 CI、文档 | 第 9 节验收标准全部通过 |

## 2. 已确认的技术决策

| 编号 | 决策 |
| ---- | ---- |
| T1 | "出站怎样够到自己的服务器"这一抽象**演进现有的 `rurge_net::Connector`**，不新增 `rurge_proto::Dialer`：签名本来就吻合，DoH / DoT / 内部 HTTP 客户端 / 资源下载已经都接受 `Arc<dyn Connector>`，而工作区里已有一个 `rurge_inbound::Dialer`（入站→引擎边界） |
| T2 | Windows 的网卡绑定用"绑定该网卡的源地址"，不引入 unsafe（项目所有者 2026-09-19 决定，关闭总设计 Q1）；差异登记到兼容性清单 |
| T3 | 六个 API 端点里手册没有给出响应示例的，按社区已知形状实现并在 `docs/api/phase2.md` 标"暂定"（项目所有者 2026-09-19 决定） |
| T4 | `http` / `https` 上游对明文 HTTP 默认按**绝对 URI 转发**（手册：`always-use-connect` 默认 false），M1 就做，不降级成"只会 CONNECT" |
| T5 | 主配置里策略的**构建失败是配置错误**：`check` 报错、`run` 拒绝启动、`reload` 保留旧一代 |
| T6 | `capabilities::current()` 在 **M1b** 才加入四种协议，避免"`W0007` 消失但运行期仍 REJECT"的空窗 |
| T7 | PKCS#12 用 `p12-keystore` **0.2**（MIT / Apache-2.0；源码核实：支持旧式 RC2-40 / 3DES 与 PBES2-AES256，自带 writer 可在测试里现场生成 p12）。不用 0.3：它换到了新一代 RustCrypto，会给 `Cargo.lock` 新增约 47 个 crate 版本，其中 `cms` 与 `pkcs12` 还是预发布版；0.2 建在工作区已在用的那一代（`sha2 0.10` 等）上，没有预发布依赖；socket 选项用 `socket2` 0.6（已在 `Cargo.lock`，不新增传递依赖）；随机串用 `getrandom`（已在 `Cargo.lock`） |
| T8 | `Outbound` 在 M1 只增加 TCP 需要的方法；UDP 方法到 M5 以"带默认实现的新方法"加入，不破坏既有实现 |

## 3. crate 改动一览

```
rurge-config    + spec/（PolicySpec 等）、诊断码 E0018–E0022 / W0028–W0029、Keystore Base64 校验；+ base64 依赖
rurge-net       + socket.rs（SocketOpts、SocketHook、竞速连接）、tls.rs（根证书装载抽成公共函数）
                  connector.rs：ConnectOpts 去掉 prefer_v6；DirectConnector 带 SocketOpts 与 SocketHook；+ socket2
rurge-proto     + transport/tls.rs、keystore.rs、http.rs、socks5.rs、testing/（feature "testing"）
                  outbound.rs：http_forward()、OutboundError 三个新变体；Direct 改用新的 DirectConnector
rurge-platform  + socket.rs（bind_interface / set_tos 两个自由函数）；+ socket2
rurge-dns       + Resolver::host_lookup
rurge-policy    + factory.rs（OutboundFactory、BuildError）、cell.rs（RegistryCell、ChainConnector）、
                  selections.rs：SelectionTable；registry.rs：Entry 重构、Resolution.note / terminal
rurge-inbound   session.rs：Dialed.forward；http.rs：forward() 支持绝对 URI 转发
rurge-engine    + outbounds.rs（EngineFactory、dry_build）；engine.rs：拨号改造、select / 视图方法；runtime.rs：装配
rurge-api       routes/policies.rs：四个端点
rurge (bin)     PlatformSockets 适配器注入；capabilities 翻转；check / run / reload 调 dry_build
tests/interop   ★ 新的仅测试用工作区成员（publish = false）
```

依赖方向不变（总设计第 3 节）。`rurge-platform` 仍不依赖任何内部 crate：平台函数是自由函数，实现 `SocketHook` 的适配器放在 bin（与 `SystemProxyManager` 同样的做法）。

## 4. 配置层（`rurge-config::spec`）

### 4.1 入口与数据模型

```rust
pub struct SpecEnv<'a> {
    pub keystore: &'a [KeystoreItem],
    /// 名字 → 是策略、组还是内置（校验 underlying-proxy 用）。
    pub lookup: &'a dyn Fn(&str) -> Option<NameKind>,
}
pub fn to_spec(policy: &ProxyPolicy, env: &SpecEnv<'_>) -> SpecOutcome;
pub struct SpecOutcome {
    pub spec: Option<PolicySpec>,          // 有错误级诊断时为 None
    pub diagnostics: Vec<Diagnostic>,
    pub inert: Vec<&'static str>,          // 出现了的"已解析未生效"参数名 → 调用方按参数名去重后报 W0029
    pub ios_only: Vec<&'static str>,       // 出现了的 iOS 专属参数名 → W0004
}

pub struct PolicySpec { pub name: String, pub kind: PolicyKind, pub server: Option<HostName>, pub port: Option<u16>,
                        pub common: CommonOpts, pub proto: ProtoSpec, pub span: Span }
pub enum ProtoSpec { Direct, Reject(Builtin), Http(HttpSpec), Socks5(Socks5Spec) }  // 随里程碑增长

pub struct CommonOpts {
    pub interface: Option<String>,
    pub allow_other_interface: bool,
    pub dns_follow_interface: bool,
    pub no_error_alert: bool,
    pub ip_version: IpVersion,            // Dual | V4Only | V6Only | PreferV4 | PreferV6
    pub tfo: bool,
    pub tos: u8,                          // 十进制或 0x 十六进制
    pub ecn: Tristate,                    // Auto | On | Off
    pub block_quic: Tristate,
    pub test_url: Option<String>,
    pub test_timeout: Option<Duration>,
    pub test_udp: Option<UdpTest>,
    pub underlying_proxy: Option<String>,
}
pub struct TlsOpts {
    pub skip_cert_verify: bool,
    pub sni: Sni,                         // Default | Off | Name(String)
    pub verify_name: Option<String>,
    pub fingerprint_sha256: Option<[u8; 32]>,
    pub alpn: Vec<String>,
    pub client_cert: Option<String>,      // [Keystore] 条目名
}
pub struct HttpSpec { pub tls: Option<TlsOpts>, pub username: Option<String>, pub password: Option<String>,
                      pub always_use_connect: bool, pub headers: Vec<HeaderTemplate> }
pub struct Socks5Spec { pub tls: Option<TlsOpts>, pub username: Option<String>, pub password: Option<String>,
                        pub udp_relay: bool }
```

- `to_spec` 对**没有 spec 定义的类型**返回空的 `SpecOutcome`，它们的参数原样不动，直到各自的里程碑。M1 覆盖：`direct` / `reject*` 别名（它们也接受通用参数，FR-OUT-02）、`http` `https` `socks5` `socks5-tls`。
- `validate()` 对每个策略调用 `to_spec`，结果存进新字段 `Config.specs: Vec<PolicySpec>`，并提供 `Config::spec(name) -> Option<&PolicySpec>`。`ConfigSummary` 不变（不引起语料库快照变动）。
- `ProxyPolicy` 不变；API 的策略详情与 `redact_profile` 仍基于原始定义行。
- `username` / `password` 位置写法与命名写法都接受；两者同时出现时命名的优先。重复的命名参数取第一个。
- `reject*` 别名上的通用参数只校验取值，既不生效也不产生"不适用"告警（FR-OUT-02 只要求接受它们）。

### 4.2 `ParamReader`

包一层 `&ParamMap` 与位置参数：类型化读取（`bool` `str` `u8_tos` `enumeration` `hex32` `list`…）时记下读过的键，取值非法就地产生 `E0018`；`finish()` 对没读过的键告警。它补上目前 `[Proxy]` 参数完全没有校验的缺口，后续每个协议的 spec 都用它。

### 4.3 诊断码（永不重编号；只定义 M1 用到的）

| 码 | 含义 | 例 |
| -- | ---- | -- |
| `E0018` | 已知参数取值非法 | `tos=300`、`ip-version=v5`、指纹不是 64 位十六进制、`underlying-proxy=REJECT` |
| `E0019` | `underlying-proxy` 成环 | 静态引用图上检测：策略 → 它的 `underlying-proxy`；组 → 它的每个成员 |
| `E0020` | Keystore 引用无效 | `client-cert` 指向不存在的条目，或条目类型不是 `p12` |
| `E0021` | Keystore 条目的 `base64` 不是合法 Base64 | |
| `E0022` | 策略无法构建（干构建，6.4） | p12 解不开、密码错误 |
| `W0028` | 参数对该策略类型不适用，忽略 | `sni` 写在 `http` 上；`underlying-proxy` `ecn` `no-error-alert` 写在 `direct` 上 |
| `W0029` | 参数已解析但本版本尚未生效 | `udp-relay` `tfo` `test-url` `test-timeout` `test-udp` `block-quic` `ecn` `dns-follow-interface` `shadow-tls-password` `shadow-tls-sni` `shadow-tls-version`；布尔参数只在取值为 `true` 时报；每个参数名每次加载只报一次 |

沿用的码：不认识的参数 `W0001`；iOS 专属的 `hybrid` 用 `W0004`（每次加载一次）；`underlying-proxy` 指向不存在的名字用 `E0007`。`skip-cert-verify=true` 与指纹同时出现时指纹优先，并以 `W0012` 告警。`underlying-proxy = DIRECT` 合法，等同于没有写。

### 4.4 `[Keystore]`

每个条目的 `base64` 在加载期解码校验（`E0021`，条目自己的行号），解码结果不保留。`client-cert` 的引用在 `to_spec` 里校验（`E0020`）。p12 的解析在构建期（5.3）。

### 4.5 防误伤

spec 校验会让以前能加载的配置新增错误。M1a 增加一个测试：对 `tests/corpus/valid` 全量加载，断言**不新增任何错误级诊断**（NFR-04）；新增的告警体现在快照里，逐条审阅。

## 5. 连接层与协议层（M1a）

### 5.1 `rurge_net::socket` 与 `DirectConnector`

```rust
pub struct SocketOpts {
    pub interface: Option<String>,
    pub allow_other_interface: bool,
    pub ip_version: IpVersion,
    /// 交错排序时哪一族在前（来自 `[General] ipv6`）；`prefer-*` / `*-only` 时不看它。
    pub v6_first: bool,
    pub tos: u8,
}
pub enum Family { V4, V6 }

/// 平台相关的两件事；实现经 bin 注入（AR-02）。
pub trait SocketHook: Send + Sync {
    fn bind_interface(&self, socket: &socket2::Socket, interface: &str, family: Family) -> io::Result<()>;
    fn set_tos(&self, socket: &socket2::Socket, family: Family, tos: u8) -> io::Result<()>;
}
pub struct NoopSocketHook;   // 两个方法都不做事；check / rule match / dns lookup 与测试用

// connector.rs
pub struct ConnectOpts { pub timeout: Duration }                       // 去掉 prefer_v6
impl DirectConnector {
    pub fn new(resolver: Arc<dyn Resolve>) -> DirectConnector;         // 默认 SocketOpts + NoopSocketHook
    pub fn with_opts(resolver: Arc<dyn Resolve>, opts: SocketOpts, hook: Arc<dyn SocketHook>) -> DirectConnector;
}
```

- **建连**：`socket2::Socket` → 非阻塞 → `set_tos`（非 0 时；失败只记 debug，不影响连接）→ `bind_interface`（有 `interface` 时）→ 交给 tokio 连接 → `TCP_NODELAY`。目标是 IP 字面量时不按 `ip-version` 过滤（手册：该参数只在主机名是域名时有意义）。`bind_interface` 失败：`allow_other_interface=true` 则 WARN 一次并不绑定继续，否则这次连接以该错误失败。
- **竞速**（取代现有的顺序尝试）：

| `ip-version` | 行为 |
| ------------ | ---- |
| `dual` | 按地址族交错排序（哪一族在前由 `v6_first` 决定）；每 250 ms 发起下一个尝试，先成功者胜，其余取消 |
| `prefer-v4` / `prefer-v6` | 先只在偏好族内交错尝试；3 秒仍未建立则把另一族也加入竞速（手册） |
| `v4-only` / `v6-only` | 过滤掉另一族；没有可用地址即失败 |

  DNS 解析与全部尝试共用 `ConnectOpts.timeout` 这一个总预算；全部失败时返回最后一个错误。"发起一次连接"在内部是可注入的函数，竞速逻辑因此能用暂停的时钟做确定性测试。
- `[General] ipv6` 原先经 `ConnectOpts.prefer_v6` 传入，现在由装配方写进 `SocketOpts.v6_first`（`ipv6=true` 时 v6 在前，否则 v4 在前），排序与阶段 1 一致。去掉 `ConnectOpts.prefer_v6` 牵涉的构造点随之同步修改：`rurge-net/src/http.rs`、`rurge-dns/src/upstream/tcp.rs`、`rurge-dns/src/bootstrap.rs`（它自己的连接器也调用 `interleave`，改为固定 v4 在前，即现状）、`rurge-engine/src/engine.rs` 两处、`rurge-proto/src/direct.rs` 的测试。
- `rurge_net::tls::root_store() -> Arc<RootCertStore>`：把 `http.rs` 里内联的"系统根 + webpki 兜底"抽成公共函数，内部 HTTP 客户端与出站的 TLS 层共用。

**平台实现**（`rurge-platform::socket`，自由函数）：

| 平台 | `bind_interface` | `set_tos` |
| ---- | ---------------- | --------- |
| Linux | `SO_BINDTODEVICE`（`socket2::Socket::bind_device`） | `set_tos_v4` / `set_tclass_v6` |
| macOS | `IP_BOUND_IF` / `IPV6_BOUND_IF`（`bind_device_by_index_v4/v6`；网卡名 → 索引取自 `if_addrs::Interface::index`，不调 unsafe 的 `if_nametoindex`） | `set_tos_v4` / `set_tclass_v6` |
| Windows | 用 `if-addrs` 找该网卡（按友好名称，如 `Wi-Fi`）在对应地址族上的第一个非环回、非链路本地地址，绑定为源地址；找不到 → `AddrNotAvailable` | `set_tos_v4`；IPv6 无封装，`Ok(())` 并记 debug |

**TCP Fast Open**：`socket2` 0.6 对任何平台都没有 TFO 的安全封装（已查源码），按"缺封装即不支持、不为此引入 unsafe"的规则，`tfo` 在 M1 三平台都不生效：参数照常解析，`tfo=true` 归入 `W0029`，`SocketOpts` 与 `SocketHook` 里不出现 TFO。

### 5.2 `Outbound` 的变化

```rust
pub trait Outbound: Send + Sync {
    fn name(&self) -> &str;
    fn connect_tcp<'a>(&'a self, target: &'a Target, opts: &'a ConnectOpts)
        -> BoxFuture<'a, Result<BoxedStream, OutboundError>>;
    /// 接受绝对 URI 形式明文请求的 HTTP 代理（`always-use-connect = false`）。
    fn http_forward(&self) -> Option<&dyn HttpForward> { None }
}
pub trait HttpForward: Send + Sync {
    /// 只连到代理本身（TCP，`https` 再加 TLS），不发 CONNECT。
    fn connect<'a>(&'a self, opts: &'a ConnectOpts) -> BoxFuture<'a, Result<BoxedStream, OutboundError>>;
    /// 为一次请求渲染好的 `Proxy-Authorization` 与 `headers`。
    fn request_headers(&self) -> Vec<(String, String)>;
}
pub enum OutboundError { Reject(..), Unsupported(..), Dns(..), Io(..), Timeout,
                         Proxy(String), Tls(String), Unavailable(String) }   // 后三个为新增
```

`Proxy` 的文本形如 `http proxy answered 407 Proxy Authentication Required`、`socks5: authentication failed`；任何错误文本与日志都不含凭据。`Direct` 改为建立在带 `SocketOpts` 的 `DirectConnector` 上，`direct` 别名策略因此各自拥有自己的 socket 选项。

### 5.3 TLS 层（`transport::tls`）与 Keystore 解码

```rust
pub struct TlsClient { /* Arc<rustls::ClientConfig> + ServerName */ }
impl TlsClient {
    pub fn build(opts: &TlsOpts, server: &HostName, default_alpn: &[&str],
                 client_cert: Option<ClientIdentity>, roots: Arc<RootCertStore>) -> Result<TlsClient, BuildError>;
    pub async fn wrap(&self, stream: BoxedStream) -> io::Result<BoxedStream>;
}
pub fn decode_p12(item: &KeystoreItem) -> Result<ClientIdentity, BuildError>;   // keystore.rs，经 p12-keystore
```

- 构建期完成一切可以提前做的事（编出 `ClientConfig`、解码 p12），拨号期只做握手。`BuildError { message }` 定义在 `rurge-proto`（协议的构造函数在 M1a 就要用到它），`rurge-policy::factory` 在 M1b 直接复用这个类型。
- 自定义校验器三种模式，**都照常校验握手签名**：标准（`verify_name` 存在时用它而不是 SNI 名做链校验）；指纹（对叶子证书 DER 取 SHA-256，常数时间比较，**取代**标准 X.509 校验——手册原文）；不校验。优先级：指纹 > `skip-cert-verify` > 标准。
- `sni = off` 关闭 SNI 扩展；`sni = <name>` 改发该名字；都没写时发代理主机名，代理主机是 IP 字面量则不发 SNI。
- `alpn` 未写时用协议给的默认值（`http` / `socks5` 系列为空）。
- 根证书可注入（测试传自签 CA；生产传 `rurge_net::tls::root_store()`）。

### 5.4 `http` / `https`

- **CONNECT**：`CONNECT host:port HTTP/1.1` + `Host: host:port` + 有凭据时 `Proxy-Authorization: Basic …` + `headers`。IPv6 目标写成 `[addr]:port`。响应头上限 16 KiB，整个握手受 `ConnectOpts.timeout` 约束；2xx 成功，其余 → `Proxy(..)`；响应头之后已读入的字节原样留给隧道。
- **`headers`**：分号分隔；配置的头**替换**同名的原有头，包括 `Host`（手册）。占位符 `<random-string(n)>` 与 `<random-string(min-max)>` 每次连接渲染一次，字符集 `A–Z a–z 0–9 - _`，随机数来自 `getrandom`。模板在加载期解析，格式错误是 `E0018`。
- **转发模式**：`always_use_connect = false`（默认）时 `http_forward()` 返回 `Some`。`connect` 只建立到代理的 TCP（+ TLS）；`request_headers()` 给出 Basic 鉴权与渲染后的 `headers`。

### 5.5 `socks5` / `socks5-tls`

RFC 1928 / 1929。有凭据时提供"无鉴权 + 用户名密码"两种方法，否则只提供无鉴权；服务器选了不在提供列表里的方法 → `Proxy(..)`。目标是域名就用 ATYP = domain（远程解析；超过 255 字节是错误），是 IP 就用对应的 ATYP。应答码映射成可读文本（`socks5: connection refused` 等）。`udp-relay` 解析保留，`W0029`。

### 5.6 `rurge_proto::testing`（cargo feature `testing`）

可编排的回环假上游，供本 crate 的测试、引擎的集成测试与后续里程碑使用：`FakeHttpProxy`（CONNECT 与绝对 URI 两种请求；可要求 Basic 鉴权；可编排状态码、超长响应头、延迟、在响应头后追加字节；记录收到的请求行与请求头）、`FakeSocks5`（可要求用户名密码；可编排应答码、截断、延迟；记录收到的 ATYP 与地址）、`TlsAcceptorFixture`（`rcgen` 现签 CA 与叶子证书，可选要求客户端证书，记录收到的 SNI 与 ALPN）、`echo_server()`。全部回环 + 端口 0。

## 6. 装配（M1b）

### 6.1 工厂（`rurge-policy::factory`，由 `rurge-engine::outbounds::EngineFactory` 实现）

```rust
pub trait OutboundFactory: Send + Sync {
    /// 没有 `underlying-proxy` 的策略用它拨号（带该策略自己的 socket 选项）。
    fn direct_connector(&self, common: &CommonOpts) -> Arc<dyn Connector>;
    /// 同步、不碰网络。
    fn build(&self, spec: &PolicySpec, connector: Arc<dyn Connector>) -> Result<OutboundRef, BuildError>;
}
pub use rurge_proto::BuildError;       // { pub message: String }，定义在 rurge-proto（5.3）
```

`EngineFactory` 持有：解析器、`Arc<dyn SocketHook>`、Keystore 条目、根证书、由 `[General] ipv6` 折算的默认地址族偏好。`rurge-policy` 的单元测试用假工厂与假出站。

### 6.2 注册表、`RegistryCell` 与 `ChainConnector`

```rust
enum Entry {
    Alias(Terminal),                       // 不带参数的 direct / reject* 别名 → 共享的 DIRECT / REJECT
    Outbound { outbound: OutboundRef, proxy: bool },   // 带 socket 选项的 direct 别名，或构建好的代理
    Unsupported { kind: PolicyKind },      // 协议尚未实现 → REJECT（W0007）
    Group { kind: GroupKind, members: Vec<String> },
}
pub struct Resolution {
    pub chain: Vec<String>,
    pub outbound: OutboundRef,
    pub terminal: TerminalKind,            // Direct | Reject | Proxy
    pub note: Option<Note>,                // Unsupported(关键字)；M3 起还有 Unavailable(原因)
}
pub struct RegistryCell(ArcSwapOption<PolicyRegistry>);      // 引擎持有，跨代稳定
pub struct ChainConnector { cell: Arc<RegistryCell>, name: String }   // impl Connector
```

- 构建：`PolicyRegistry::build(cfg, factory, cell, selections)`。对每个有 spec 的策略：有 `underlying-proxy`（且不是 `DIRECT`）→ `ChainConnector`，否则 → `factory.direct_connector(&spec.common)`，再 `factory.build(..)`。策略链的表示不变（`!unsupported:<kw>` 标记、流量统计取最后一个非标记项）。
- `ChainConnector::connect(target, opts)`：从 cell 取**当前一代**注册表，解析 `name`（是组就跟随组的当前选择），调那个出站的 `connect_tcp(target, opts)`——`target` 是本策略的服务器，主机名原样传过去（远程解析，FR-OUT-08）；`OutboundError` 映射成带 `via <name>: …` 前缀的 `io::Error`。它按名字解析而不绑定某一代，M2 做跨代复用时不必再改；重载后名字消失则这次拨号失败并给出明确错误。
- 引擎在换代时 `cell.store(新一代)`，在 `Drop` 时清空 cell，打断 cell ↔ 注册表的引用环。
- M1 里组成员是静态的，环已在加载期由 `E0019` 拦住；运行期的深度兜底留给 M3（订阅才会引入动态成员）。

### 6.3 `SelectionTable`

引擎持有的共享可变表（`RwLock<HashMap<组名, 成员名>>`），取代"把选择烤进每一代注册表"；`GroupSelections` 保留为装载 / 保存用的快照类型。`resolve` 每次读表；成员已消失的旧选择照旧回落到第一个成员。同一 Profile 的重载沿用同一张表。

```rust
impl Engine {
    pub fn groups_view(&self) -> Vec<GroupView>;                    // 名字、类型、hidden、成员（名字、是否组、类型描述）、当前生效成员
    pub fn policy_detail(&self, name: &str) -> Option<String>;      // 脱敏后的定义行
    pub fn group_selection(&self, group: &str) -> Result<String, SelectError>;
    pub async fn select_group(&self, group: &str, member: &str) -> Result<(), SelectError>;
}
```

`select_group` 校验（组存在、是 `select` 类型、成员在组内）→ 更新表 → `StateStore::update` 写 `group_selections[profile][group]`。这补上了目前不存在的写入路径。

### 6.4 干构建与"构建失败即配置错误"

`rurge_engine::outbounds::dry_build(cfg: &Config) -> Diagnostics`：用 `NoopSocketHook` 与一个永不被调用的解析器构造 `EngineFactory`（该构造用真实根证书，见第 13 节），对 `cfg.specs` 逐个 `build` 后丢弃，把 `BuildError` 转成 `E0022`（策略自己的行号）。统一入口 `rurge_engine::outbounds::load_checked(path, opts)`（= `rurge_config::config::load` + 追加 `dry_build` 的诊断）供四个调用点使用：`rurge check`、`run`、`reload`、`POST /v1/profiles/check`——有错误则 `run` 以退出码 2 拒绝启动、`reload` 保留旧一代（第 13 节 P7）。`Runtime::build` 里真正的构建失败因此只剩防御意义，直接返回错误。

### 6.5 引擎拨号

- `ConnectOpts { timeout: CONNECT_TIMEOUT }`。
- **FR-DNS-07**：`resolution.terminal == Proxy`、目标是域名且 `use-local-host-item-for-proxy = true` 时，调用新增的 `Resolver::host_lookup(name) -> Option<HostLookup>`（对现有 `HostMap::lookup` 的两行委托，不发网络查询）；命中 `HostAction::Ips` 就把第一个 IP 而不是域名交给代理，别名 / 指定服务器类条目不改变目标。
- **绝对 URI 转发**：`rurge_inbound::Dialed` 增加 `forward: Option<Vec<(String, String)>>`。会话是 HTTP 入站的明文请求（`listener == Http` 且 `session.url` 有值）且终端出站的 `http_forward()` 为 `Some` 时，引擎走 `HttpForward::connect` 并带回 `request_headers()`。入站的 `forward()` 见到 `Some` 就**不做** `origin_form()`：保留绝对 URI、照常去掉 hop-by-hop 头、把这些头套上（替换同名头）再发。经 SOCKS5 入站进来的明文 HTTP 无从识别，照常走 CONNECT。
- 错误映射：`Proxy` / `Tls` → `FailKind::Connect` 并保留文本；`Unavailable` 在 M1 不会产生。
- 请求记录的字段 M1 不变。`dial_internal`（DNS 跟随出站模式）不需要改动：代理出站对它同样可用。

### 6.6 API（`rurge-api`）

| 端点 | 响应 | 来源 |
| ---- | ---- | ---- |
| `GET /v1/policies/detail?policy_name=X` | `{"X": "<脱敏后的定义行>"}`；内置策略的值是它自己的名字；未知 → 404 | 暂定 |
| `GET /v1/policy_groups` | `{"<组名>": [{"name", "typeDescription", "isGroup", "enabled", "lineHash"}, …], …}`，成员顺序同配置；`enabled` 恒为 `true`；`lineHash` 是脱敏后定义行 SHA-256 的前 16 个十六进制字符（第 13 节 P6 的修订） | 暂定 |
| `GET /v1/policy_groups/select?group_name=G` | `{"policy": "<当前生效的成员>"}`；未知组 → 404 | 手册 |
| `POST /v1/policy_groups/select` | 请求 `{"group_name", "policy"}` → `{}`；组或成员无效、不是 `select` 组 → 400 | 手册（请求体） |

鉴权、错误体、`{}` 成功体沿用阶段 1 的约定。形状登记在新建的 `docs/api/phase2.md`，"暂定"的逐项标出。

### 6.7 bin

`PlatformSockets`（`impl SocketHook`，委托给 `rurge_platform::socket`）经 `StackOptions` 注入；`capabilities::current()` 加入 `Http` `Https` `Socks5` `Socks5Tls`；`check` 在 `load()` 后追加干构建的诊断再计数与输出。

## 7. 错误处理

| 情形 | 行为 |
| ---- | ---- |
| 参数取值非法 / 引用无效 / 环 / 构建失败 | 加载错误（`E0018`–`E0022`）：`check` 退出 2，`run` 拒绝启动，`reload` 保留旧一代 |
| 代理拒绝（407、SOCKS5 非 0 应答、方法不被接受） | 会话失败，`error` 是 `Proxy(..)` 的文本；HTTP 入站回 502、SOCKS5 入站回对应应答码（沿用阶段 1 的映射） |
| TLS 握手 / 校验失败 | `Tls(..)`，文本含校验失败的原因，不含证书内容 |
| `interface` 不可用 | `allow-other-interface=false` → 连接失败；`true` → WARN（每个策略一次）并用默认网卡 |
| 链上某一跳失败 | 错误文本带 `via <name>:` 前缀，可看出是哪一跳 |
| 握手超时 | `Timeout`；总预算是 `CONNECT_TIMEOUT`（10 秒），链上各跳共用 |

## 8. 测试策略

**M1a（三平台 `cargo test`）**

- spec：逐参数的表驱动用例（合法 / 非法 / 不认识 / 不适用 / 未生效）；手册示例行做黄金用例；语料库"不新增错误"测试（4.5）。
- 竞速：注入"发起一次连接"的函数 + `tokio::time::pause()`，确定性地测 250 ms 交错、3 秒回退、先成者胜、取消其余、总预算；真实 socket 只测回环连接与假 `SocketHook` 的调用记录（含 `allow-other-interface` 两种结果）。需要 `[::1]` 的用例在绑定失败时跳过。
- TLS：名字不匹配失败、`server-cert-verify-name`、`sni=off`（服务端断言没收到 SNI）、自定义 SNI、指纹对 / 错、不校验、ALPN、用 p12 做双向 TLS。p12 fixture 两份（OpenSSL 3 默认加密与 `-legacy`），测试专用密钥，随附生成命令。
- HTTP / SOCKS5：对 5.6 的假上游覆盖成功、需要 / 拒绝鉴权、非 2xx、超长响应头、应答截断、超时（暂停的时钟）、残留字节、IPv6 目标、域名远程解析、`headers` 与随机串占位（按正则断言）、转发模式的请求头。
- 平台函数：Windows 的"按网卡名找源地址"用注入的地址表做单元测试；不修改本机任何网络设置。

**M1b**

- 注册表单测（假工厂）：各类 `Entry`、`Resolution.terminal` / `note`；`ChainConnector` 在两次拨号之间切换组选择，断言入口节点随之变化；`SelectionTable` 语义。
- 引擎进程内端到端（沿用 `tests/pipeline.rs` 的写法）：CONNECT、绝对 URI 转发（假上游断言收到绝对 URI 与 `Proxy-Authorization`）、SOCKS5、两级链、`use-local-host-item-for-proxy`、`select_group` 对下一条连接生效且写进 `state.json`、重启后保留；**rurge → rurge**（引擎 A 的上游是引擎 B 的入站，两份独立实现互相验证）。
- API：四个端点含错误路径。CLI：坏 p12 → `rurge check` 带行号退出 2；`http` / `socks5` 不再出 `W0007`；`run` 对构建失败的配置以退出码 2 拒绝启动。
- **互操作**（`tests/interop`，仅测试用的工作区成员）：夹具按 `RURGE_TEST_SING_BOX` 或 `PATH` 找 sing-box，把 http / socks / mixed 入站（含用户名密码与 TLS，证书由 `rcgen` 现签）渲染到临时目录，作为回环子进程拉起，等端口就绪，`Drop` 时杀掉并等待退出。缺二进制 → 打印说明并跳过；`RURGE_INTEROP_REQUIRED=1` 时缺了就失败。用例在库层面直接构造出站（经 `EngineFactory`），经 sing-box 连到回环的回显 / HTTP 源站。
- **CI**：三平台安装固定版本的 sing-box 并校验 SHA256，设 `RURGE_INTEROP_REQUIRED=1`。仓库目前领先 origin 176 个提交且从未推送，这一步在首次推送前无法验证，实施计划里如实标注。

安全约束不变：只用回环、不碰公网、不改本机系统代理、不注册真实服务。

## 9. 验收标准

1. 一份含 `http` `https` `socks5` `socks5-tls` 策略的真实配置，`rurge run` 后经 HTTP 与 SOCKS5 入站的流量按规则走对应上游；两级 `underlying-proxy` 链（含底层是 `select` 组）转发通过。
2. 明文 HTTP 经 `http` 上游按绝对 URI 转发，`always-use-connect=true` 时改走 CONNECT。
3. 六个 TLS 参数与 p12 客户端证书各有测试；`interface` / `allow-other-interface` / `ip-version` 三种模式各有测试。
4. `POST /v1/policy_groups/select` 切换后下一条连接生效，重启后保留；其余三个端点返回登记的形状。
5. `rurge check` 对参数非法、引用无效、成环、p12 解不开分别给出带行号的 `E0018`–`E0022`；语料库不新增错误。
6. 对 sing-box 的互操作测试通过（本机装了就跑）。
7. `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace` 全绿。

## 10. 兼容性清单需登记的差异

| 项 | 内容 |
| -- | ---- |
| `interface`（Windows） | 取网卡的友好名称；以绑定该网卡的源地址实现；网卡在对应地址族上没有地址则视为不可用；被改成弱主机模型的接口上可能不生效 |
| `tfo` | 解析并校验，`W0029`，三平台都不生效（`socket2` 没有 TFO 的安全封装，不为此引入 unsafe） |
| `tos` | Windows 上对 IPv6 不生效 |
| `dns-follow-interface` | 解析，`W0029`，M5 生效 |
| `udp-relay` `test-url` `test-timeout` `test-udp` `block-quic` `ecn` | 解析并校验取值，`W0029`，随后续里程碑生效 |
| `ip-version` 用于 `direct` 别名 | 同样作用于对目标的解析与连接（手册只描述了到代理服务器的连接） |
| `/v1/policies/detail`、`/v1/policy_groups` | 响应形状暂定 |
| `http` / `https` / `socks5` / `socks5-tls`、通用参数、TLS 参数、Keystore 各行 | 状态改为已实现；`W0007` 的备注改为"随阶段 2 各里程碑逐协议移除" |

## 11. 对阶段 2 总设计的订正（同一提交里改总设计的文字）

| 编号 | 总设计原文 | 订正 |
| ---- | ---------- | ---- |
| C1 | 5.1–5.3 的 `Dialer` / `DirectDialer` / `ChainDialer` / `InterfaceBinder` / `DialCtx` | 分别落为演进后的 `rurge_net::Connector`、`DirectConnector`、`ChainConnector`、`SocketHook`、`ConnectOpts`（T1） |
| C2 | M1 含 `dns-follow-interface`（FR-DNS-10） | 移到 M5：DNS 的 UDP 上游是每个上游一个共享的已连接 socket，按策略换网卡需要另一组 socket 与按网卡分区的缓存，等 M5 的 UDP socket 工厂就位再做 |
| C3 | 5.6："构建失败的策略……解析为 REJECT……加载时 WARN 一次"，同时"`rurge check`……作为带行号的错误输出" | 两句不一致。主配置里的构建失败是配置错误（T5）；"存在但不可用 → REJECT"只用于 M3 的订阅导入项 |
| C4 | 第 10 节："六个端点经 `Control` trait 的扩展提供" | 经 `Engine` 的方法提供（与现有的 `set_global_policy` 一致）；`Control` 只管守护进程级命令 |
| C5 | 8.1：请求记录新增协议、拨号路径与计时字段 | 推迟到 M3（测试与 `smart` 才需要） |
| C6 | 5.5 按指纹复用（未写里程碑） | M2 起（M1 的协议无长生命周期状态）；M1 先建好它依赖的 `RegistryCell` |
| C7 | 1.4 的 M1 一行未列绝对 URI 转发 | 加入（T4） |
| C8 | 第 16 节 Q1（Windows `IP_UNICAST_IF`） | 已决：绑定源地址，不引入 unsafe（T2） |

## 12. 开放问题

| 编号 | 问题 | 处理 |
| ---- | ---- | ---- |
| O1 | `socket2` 对 macOS `IP_BOUND_IF`、Linux `TCP_FASTOPEN_CONNECT` 的安全封装在 0.6 里的确切名字与 cfg 条件 | **已结（写 M1a 计划时查了 0.6.5 源码）**：`bind_device`（Linux / Android / Fuchsia）、`bind_device_by_index_v4/v6`（Apple 各平台等）、`set_tos_v4`（含 Windows）、`set_tclass_v6`（不含 Windows）都在 `all` feature 下；**没有任何 TFO 封装** → 见 5.1 |
| O2 | `/v1/policies/detail`、`/v1/policy_groups` 的真实响应 | 拿到 Surge 实例的样本后对齐；此前标"暂定" |
| O3 | sing-box 的固定版本与三平台发布包的 SHA256 | **已结（P2）**：固定 sing-box **1.14.1**（2026-09-15 发布的稳定版）。三个包的 SHA256：`sing-box-1.14.1-linux-amd64.tar.gz` = `12cb2816b52febb356f6a885b740cc8758c3f30b8ae0ca8edba80f0d2d35343f`；`sing-box-1.14.1-windows-amd64.zip` = `5197f16d492d93202dc623622149a6ed040f8eca263128f91d603f2b901baa89`；`sing-box-1.14.1-darwin-arm64.tar.gz` = `b9024642ef7b4848252df5469b7f60ef3c18bb5e217a16a0934f0174f8ad11b4`（GitHub 发布 API `releases/tags/v1.14.1` 各资产的 `digest` 字段；`macos-latest` 是 arm64） |
| O4 | hyper 的 HTTP/1 客户端是否原样发送绝对 URI 的请求行 | **已结（P1）**：不需要手写请求行。入站把请求 URI 重建成 `http://<host[:port]><path?query>`（去掉 userinfo）后原样交给 hyper；hyper 1.11.1 的 `client::conn::http1::SendRequest::send_request` 在 `proto/h1/role.rs` 里把请求 URI（`msg.head.subject.1`）原样写出，其文档写明发给 HTTP 代理时用 absolute-form |

## 13. M1b 实施期的订正

本节登记 M1b 计划的「计划期决定」（`global-constraints.md` P4–P12）里与本文件第 6 节文字不同的地方，以及第 6.1–6.5 节的代码草图与实现（`crates/rurge-policy/src/{registry,cell,selections,factory}.rs`、`crates/rurge-engine/src/{outbounds,shared,views}.rs`）核对后发现的出入。逐条对应实现的提交见 `docs/superpowers/plans/2026-09-19-phase2-m1b-assembly-control-plane-plan.md` 末尾「执行期修正记录」。

| 编号 | 设计原文 | 订正 |
| ---- | -------- | ---- |
| P4 | 6.2 / 6.3 说 `RegistryCell` 与 `SelectionTable` "由引擎持有、跨代稳定"，未给出承载它们的具体类型，也未说明它们与 `Runtime` 的关系 | 新增 `rurge_engine::EngineShared { cell: Arc<RegistryCell>, selections: Arc<SelectionTable> }`（`crates/rurge-engine/src/shared.rs`），由调用方在第一次 `Runtime::build` 之前创建，经 `RuntimeOptions.shared` 传入；`Runtime.policies` 改为 `Arc<PolicyRegistry>`；引擎在 `Engine::new` / `swap_runtime` 里把注册表存进 `shared.cell`，`Drop` 时清空；重载时调用方用 `engine.shared()` 构建下一代，`swap_runtime` 断言两者是同一个 cell。原因：注册表在 `Runtime::build` 里构建、早于 `Engine::new`，必须先于引擎存在 |
| P5 | 4.1："`ProxyPolicy` 不变" | `ProxyPolicy` 与 `PolicyGroup` 各加一个字段 `definition: String`（`名字 =` 右边的原文），供 `/v1/policies/detail` 与 `lineHash` 使用；"不变"指 spec 工作不重构它，语料库快照基于 `ConfigSummary`，不受影响 |
| P6 | 6.6 把 `/v1/policies/detail`、`/v1/policy_groups` 的响应标"暂定"，`lineHash` 写作"定义行 SHA-256 的前 16 个十六进制字符"，未说明是否含 `名字 =` 前缀、是否脱敏 | 两个暂定细节定下来：`/v1/policies/detail` 的值是脱敏后的定义（不含 `名字 =` 前缀），脱敏规则同 `GET /v1/profiles/current`；`lineHash` 最初定为 `SHA-256("<名字> = <定义原文>")` 的前 16 个十六进制字符（内置策略对名字取哈希），登记进 `docs/api/phase2.md` |
| P6（修订） | 同上（`lineHash` 建立在未脱敏的定义原文上） | Task 8 评审时修订：`lineHash` 改为对**脱敏后**的定义取哈希——`SHA-256("<名字> = <redact_definition(定义)>")` 的前 16 个十六进制字符，内置策略仍对名字取哈希（`crates/rurge-engine/src/views.rs::line_hash`）。原因：这个哈希经无需额外权限的 API 端点公开，若建立在未脱敏的定义上，持有 API key 的人就能拿它离线核对猜测的凭据，等于让凭据的验证能力离开了进程；名字唯一且参与哈希，脱敏不会让两个成员撞哈希；代价是只改动凭据时 `lineHash` 不变，`docs/api/phase2.md` 已注明这一点及原因 |
| P7 | 6.4："三个调用点：`rurge check`、`POST /v1/profiles/check`、以及 `run` / `reload` 在 `load()` 之后——后者把结果当作加载诊断处理" | 统一成一个入口 `rurge_engine::outbounds::load_checked(path, opts)`（= `rurge_config::config::load` + 追加 `dry_build` 的诊断），`check`、`run`、`reload`、`POST /v1/profiles/check` 四个调用点都改用它。原因：一个入口免得漏掉哪个调用点 |
| P8 | 未展开 `HostName::from_wire` 对含控制字符 / 空白名字的处理，也未说明 IDN 在哪一层转 A-label | 网络来的主机名经 `HostName::from_wire` 进入系统：含控制字符或空白的名字被拒绝，非 ASCII（IDN）原样通过（规则匹配的语义不变）；`http` / `socks5` 出站在写线之前把非 ASCII 域名转成 A-label（`rurge_proto::hostname::to_ascii`，内部用 `idna::domain_to_ascii`）。**执行期收紧**（Task 2 评审）：转换后的结果只允许 ASCII 字母、数字、`-`、`.`、`_`，其它字符一律拒绝——含被 UTS-46 从全角折叠成 `@` `/` `:` 的字符，以及本就放行的普通 ASCII `@`；原样放行 `@` 会让宽松的上游（如 Go 的 URL 解析器）把 `CONNECT a@b.test:443` 读成 userinfo + 主机 `b.test`，客户端就能经上游代理够到 rurge 域名规则从未见过的主机。`rurge_proto::http::wire_host` 随之公开，`valid_target` 定义为 `wire_host(..).is_some()`；`HttpForward` 的 trait 文档相应改写："自己写请求行 / `Host` 头的调用方必须写 `wire_host` 返回的文本，不得写 `target.host`；转发不是自己构造的 URI 的调用方用 `valid_target` 把关"（`crates/rurge-proto/src/http.rs`、`outbound.rs`） |
| P9 | 6.5："FR-DNS-07：… 命中 `HostAction::Ips` 就把第一个 IP 而不是域名交给代理"，未明说命中后这次会话走 CONNECT 还是绝对 URI 转发 | `[Host]` 的 IP 替换命中时，这条会话不走绝对 URI 转发而走 CONNECT（`crates/rurge-engine/src/engine.rs::dial` 里 `pinned` 标记参与 `forward` 的判断）。原因：转发模式下目标由请求 URI 携带，引擎不把目标交给出站；要让代理连到本地指定的 IP 又保留 `Host:`，走隧道是语义正确且最简单的做法 |
| P10 | M1a 设计范围（4.3）未说明 `hybrid` 取值非法时的行为，只说 iOS 专属参数用 `W0004` | `hybrid` 不再校验取值：桌面端一律只报 `W0004`；空的 `interface=` 仍是 `E0018`；`http` / `socks5` 策略上没有 spec 读取的参数照旧报 `W0001`。原因：手册对非法值 / 空值的行为没有任何说明，项目的兼容性原则是平台不适用的配置项解析并忽略，`hybrid` 属于这一类 |
| P11 | 4.4（TLS 参数）与第 7 节（错误处理）未提到 `skip-cert-verify=true` 要不要给出提示 | 真正构建时（非干构建）每个策略打一条运行期 WARN（`crates/rurge-engine/src/outbounds.rs::skips_verification` + `EngineFactory::build`），不新增诊断码。原因：诊断码的新增需要设计评审，这里只是给运维的信号；干构建不打，因为干构建不代表这个策略真的会被拨号使用 |
| P12 | 5.1（`rurge-platform::socket`）未提到网卡表要不要缓存 | 网卡表加 5 秒的进程内缓存（macOS / Windows 每次连接都要查表）。原因：`GetAdaptersAddresses` 是毫秒级调用，每条连接都查一次在高并发下是浪费；代价（登记进「延后事项」）：`Cached::get` 在持锁期间调用阻塞的 `get_if_addrs`，新出现的网卡最多晚 5 秒可见 |
| 任务 4 / 10 | 6.4："用 `NoopSocketHook` 与一个永不被调用的解析器构造 `EngineFactory`"，未提根证书；返回类型写作 `Vec<Diagnostic>` | 干构建的 `EngineFactory` 改用真实根证书（`EngineFactory::dry` 调 `rurge_net::tls::root_store()`），不再用空的 `RootCertStore`；新增 `EngineFactory::with_roots(cfg, resolver, hook, roots)` 供测试注入私有 CA，`EngineFactory::new` 改为调用它并传入真实根证书。原因（Task 10 发现）：rustls 的 `WebPkiServerVerifier::builder` 在空证书库上直接建不起来，会让任何走标准校验的 `https` / `socks5-tls` 策略被干构建误判成 `E0022`，即使它构建后真的能用；Task 4 的用例只让坏 p12（更早失败）和明文 `http` 走过干构建工厂，没有暴露这个缺陷。`dry_build` 的返回类型实为 `rurge_config::Diagnostics`（既有的诊断收集类型），不是裸 `Vec<Diagnostic>`——`load_checked` 要把它并入同为 `Diagnostics` 类型的 `Loaded::diagnostics`，用同一个类型更顺 |
| 6.2 | `enum Entry`、`struct Resolution`、`struct RegistryCell`、`struct ChainConnector` 只给出字段草图；`PolicyRegistry::build(cfg, factory, cell, selections)` 未给完整类型签名 | 与实现核对：字段与设计一致；最终签名为 `PolicyRegistry::build(cfg: &Config, factory: &dyn OutboundFactory, cell: &Arc<RegistryCell>, selections: Arc<SelectionTable>) -> Result<PolicyRegistry, BuildError>`（`crates/rurge-policy/src/registry.rs`）；`Entry`、`Terminal` 是 `registry` 模块内部类型，不对外 `pub`——`PolicyRegistry` 本身是唯一的公开入口 |
| 6.5 | "`rurge_inbound::Dialed` 增加 `forward: Option<Vec<(String, String)>>`"；"调用新增的 `Resolver::host_lookup(name) -> Option<HostLookup>`（对现有 `HostMap::lookup` 的两行委托）" | 与实现一致：`pub struct Dialed { pub stream: BoxedStream, pub handle: Arc<SessionHandle>, pub forward: Option<Vec<(String, String)>> }`（`crates/rurge-inbound/src/session.rs`）；`Resolver::host_lookup(&self, name: &str) -> Option<crate::hosts::HostLookup>` 委托给 `self.hosts.lookup(name)`（一行，非"两行"，纯措辞）（`crates/rurge-dns/src/resolver.rs`） |
