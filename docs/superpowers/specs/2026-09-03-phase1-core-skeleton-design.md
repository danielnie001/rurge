# 阶段 1「核心骨架」设计文档

| 项       | 内容                                                                                  |
| -------- | ------------------------------------------------------------------------------------- |
| 日期     | 2026-09-03                                                                            |
| 状态     | 草案，待评审                                                                          |
| 对应需求 | [需求文档](../../requirements.md) 第 7 节「阶段 1」；FR 编号见各节                     |
| 兼容基线 | [Surge 兼容性清单](../../surge-compatibility-matrix.md)，Surge Mac 6.9 / iOS 5.22 手册 |

## 1. 目标与范围

### 1.1 阶段目标

交付一个能加载真实 Surge 配置、提供 HTTP / SOCKS5 代理、按规则在 DIRECT 与 REJECT 之间分流、可通过 CLI 与 HTTP API 控制的跨平台守护进程。它是后续所有阶段的地基：配置模型、规则引擎、DNS、连接流水线、观测与控制面的接口在这一阶段定型。

### 1.2 范围内（引用需求文档编号）

| 领域           | 需求                                                                                           |
| -------------- | ---------------------------------------------------------------------------------------------- |
| 配置           | FR-CFG-01 ～ 09、12、14 ～ 17、19                                                              |
| 入站与系统集成 | FR-IN-01 ～ 05                                                                                 |
| 出站           | FR-OUT-01、02                                                                                  |
| 规则           | FR-RULE-01（除 PROCESS-NAME / SCRIPT / DEVICE-NAME / MAC-ADDRESS）、02 ～ 09、15               |
| DNS            | FR-DNS-01 ～ 06、11（API 部分）                                                                |
| HTTP           | FR-HTTP-10（基础）、12                                                                         |
| 观测与控制     | FR-OBS-01、02（骨架）、04（`run` `check` `reload` `stop`）、05（基础）、09（基础）、10 |

### 1.3 范围外

出站代理协议、策略组算法、TUN、HTTP 引擎（重写 / MITM / 抓包）、脚本、模块、Dashboard、网关。它们的配置行在阶段 1 会被**完整解析并校验语法**，但不产生行为（见 4.6）。

### 1.4 里程碑拆分

阶段 1 体量大，拆成四个可独立验收的里程碑，每个里程碑一份实施计划：

| 里程碑            | 内容                                                                                                                        | 可验收的产出                                      |
| ----------------- | --------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------- |
| M1 配置解析       | workspace 骨架、`rurge-config`、`rurge check`                                                                           | 对语料库中的配置输出诊断；快照测试                |
| M2 规则引擎与 DNS | `rurge-rules`、`rurge-dns`、GeoIP / ASN、外部资源管理                                                                   | `rurge rule match` 开发命令；黄金测试与属性测试 |
| M3 连接流水线     | `rurge-engine`、`rurge-inbound`、`rurge-proto`（DIRECT / REJECT）、`rurge-policy`（注册表与 select 决策）、请求记录 | `rurge run` 可作为 HTTP / SOCKS5 代理按规则分流 |
| M4 控制面与平台   | `rurge-api`、`rurge-platform`（系统代理、目录、服务）、CLI 全部阶段 1 命令、状态持久化、三平台 CI                       | 阶段 1 验收标准全部通过                           |

## 2. 技术选型

| 领域         | 选择                                                                                                         | 理由                                                                                                             |
| ------------ | ------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------- |
| 语言与工具链 | Rust stable，edition 2024，`rust-toolchain.toml` 固定 channel 为 stable；MSRV 跟随 edition 2024 的最低版本 | 纯 Rust 优先（NFR-07）                                                                                           |
| 异步运行时   | tokio（多线程运行时）+ tokio-util                                                                            | 事实标准，生态完整                                                                                               |
| TLS          | rustls 0.23 + tokio-rustls；根证书用`rustls-native-certs`（系统根，兼容企业 CA）并以 `webpki-roots` 兜底 | 纯 Rust，可控                                                                                                    |
| HTTP         | hyper 1.x + hyper-util；API 服务用 axum；内部 HTTP 客户端基于 hyper 自建，连接器可插拔                       | DoH、外部资源下载、后续策略测试与`$httpClient` 都需要经由 rurge 自己的出站路径，不能用无法定制连接的通用客户端 |
| DNS 报文     | hickory-proto（报文编解码、记录类型）                                                                        | 只用它的协议层，查询策略（并发、重试、缓存）自研以匹配 Surge 语义                                                |
| GeoIP / ASN  | maxminddb（mmap 读取 GeoLite2 Country / ASN）                                                                | 与 Surge 使用同一数据格式                                                                                        |
| 正则         | fancy-regex（回溯引擎，支持环视与反向引用），内部对无高级特性的表达式自动走 regex 快路径                     | Surge 使用 ICU 风格正则，社区规则里常见环视；纯`regex` crate 不支持                                            |
| 通配匹配     | 自研小型匹配器：`*` `?`（Host List、USER-AGENT、DEVICE-NAME、SSID）与 `[...]`（DOMAIN-WILDCARD）       | 语义与手册逐条对应，避免第三方 glob 的分隔符规则干扰                                                             |
| IP 前缀查找  | prefix-trie                                                                                                  | 维护活跃，支持 v4 / v6 最长前缀匹配                                                                              |
| 日志         | tracing + tracing-subscriber（env-filter、fmt）+ tracing-appender（滚动文件）                                | 结构化字段、按连接关联                                                                                           |
| CLI          | clap（derive）                                                                                               |                                                                                                                  |
| 序列化       | serde + serde_json；状态文件 JSON                                                                            |                                                                                                                  |
| 文件监视     | notify                                                                                                       | 本地规则集 / hosts / 配置变化                                                                                    |
| 配置热切换   | arc-swap                                                                                                     | 不可变配置对象原子替换（AR-04）                                                                                  |
| 错误         | 库 crate 用 thiserror，bin 用 anyhow                                                                         |                                                                                                                  |
| 测试         | insta（快照）、proptest（属性）、criterion（基准）                                                           | NFR-08                                                                                                           |

## 3. Workspace 与 crate 边界

在需求文档 3.2 的基础上增加一个 `rurge-engine` crate 承载连接流水线与运行时状态，避免把编排逻辑塞进二进制 crate 或某个领域 crate。阶段 1 只创建本阶段需要的 crate，不预建空壳。

```
rurge/
├── Cargo.toml                 # workspace，统一 lints / 依赖版本
├── rust-toolchain.toml
├── crates/
│   ├── rurge-config/          # M1
│   ├── rurge-rules/           # M2
│   ├── rurge-dns/             # M2
│   ├── rurge-net/             # M2：内部 HTTP 客户端、连接器 trait、外部资源管理器
│   ├── rurge-policy/          # M3
│   ├── rurge-proto/           # M3：DIRECT / REJECT 出站
│   ├── rurge-inbound/         # M3：HTTP / SOCKS5 监听
│   ├── rurge-engine/          # M3：会话流水线、请求记录、流量统计、运行时状态、重载
│   ├── rurge-api/             # M4
│   ├── rurge-platform/        # M4：系统代理、目录、hosts 路径、服务安装
│   └── rurge/                 # bin：CLI 与守护进程组装
├── tests/corpus/              # 兼容性语料库（脱敏 Surge 配置与规则集）
└── docs/
```

依赖方向（只能向下）：

```
rurge (bin) → rurge-api → rurge-engine → { rurge-inbound, rurge-proto, rurge-policy, rurge-rules, rurge-dns }
                                         rurge-rules → rurge-net, rurge-config
                                         rurge-dns   → rurge-net, rurge-config
                                         rurge-policy → rurge-config
                                         rurge-net   → rurge-config
rurge-platform 只被 rurge-engine 与 bin 依赖；rurge-config 不依赖任何内部 crate
```

每个 crate 的公共接口在 5 ～ 11 节定义；实现细节可变，接口变更需要更新本文档。

## 4. 核心数据模型（`rurge-config`）

### 4.1 两层模型

1. **`Profile`（文本层）**：忠实保留文件结构。`Vec<Section>`，每个 `Section { name, kind: KeyValue | Ordered | Unknown, entries: Vec<Entry> }`，`Entry { raw, span: Span { file, line }, disabled: bool, origin: Origin }`。`origin` 记录该行来自主文件、某个 include 文件还是模块。未识别的节与键在这一层原样保留（FR-CFG-02）。
2. **`Config`（语义层）**：从 `Profile` 派生的强类型对象，只包含 rurge 理解的内容，附带 `Vec<Diagnostic>`。运行时只读 `Config`，用 `Arc<Config>` 共享。

### 4.2 语义层主要类型

```rust
pub struct Config {
    pub general: General,                 // 全部 [General] 键的强类型表示
    pub policies: Vec<ProxyPolicy>,       // [Proxy]
    pub groups: Vec<PolicyGroup>,         // [Proxy Group]
    pub rules: Vec<Rule>,                 // [Rule]，已解析
    pub rulesets: Vec<InlineRuleset>,     // [Ruleset <name>]
    pub hosts: Vec<HostEntry>,            // [Host]
    pub keystore: Vec<KeystoreItem>,      // [Keystore]（阶段 2 使用，阶段 1 解析）
    pub deferred: DeferredSections,       // 已解析但本阶段无行为的节：MITM、URL/Header/Body Rewrite、Map Local、Script、Panel、SSID Setting、Port Forwarding、WireGuard、DHCP、Snell Server、MTProto、Testing
    pub managed: Option<ManagedConfig>,   // #!MANAGED-CONFIG
    pub source: SourceInfo,               // 主文件路径、include 文件列表、加载时间、内容哈希
}

pub enum HostName { Domain(String), V4(Ipv4Addr), V6(Ipv6Addr) }

pub struct HostList(Vec<HostListEntry>);           // FR-CFG-12
pub struct HostListEntry { negate: bool, pattern: HostPattern, port: PortSpec }
pub enum HostPattern { Wildcard(Glob), AnyIp, AnyV4, AnyV6, SimpleHostname }
pub enum PortSpec { Default, All, Port(u16) }

pub struct ProxyPolicy {
    pub name: String,
    pub kind: PolicyKind,                 // 16 种协议关键字 + 内置别名类型的枚举；未知关键字是错误
    pub server: Option<HostName>, pub port: Option<u16>,
    pub positional: Vec<String>,          // 如 http 的 username/password 位置参数
    pub params: ParamMap,                 // 保序、大小写不敏感的 key=value；阶段 2 逐协议做强类型校验
    pub span: Span,
}

pub struct PolicyGroup { name, kind: GroupKind, members: Vec<String>, params: ParamMap, conditions: Vec<(SubnetExpr, String)>, span }

pub struct Rule { kind: RuleKind, policy: PolicyRef, params: RuleParams, span }
pub enum RuleKind {
    Domain(String), DomainSuffix(String), DomainKeyword(String), DomainWildcard(Glob),
    DomainSet(ResourceRef), IpCidr(Ipv4Net), IpCidr6(Ipv6Net), GeoIp(CountryCode), IpAsn(u32),
    UserAgent(Glob), UrlRegex(Regex), ProcessName(ProcessPattern), DestPort(PortExpr), SrcPort(PortExpr),
    InPort(PortExpr), SrcIp(IpOrNet), DeviceName(Glob), MacAddress(MacAddr), Protocol(ProtocolKind),
    HostnameType(HostnameType), Subnet(SubnetExpr), CellularRadio(String), CellularCarrier(String),
    And(Vec<SubRule>), Or(Vec<SubRule>), Not(Box<SubRule>), Script(String), RuleSet(RuleSetRef), Final,
}
pub struct RuleParams { no_resolve, dns_failed, extended_matching, pre_matching, notification_text, notification_interval, update_interval, requires_resolve, always_capture, unknown: Vec<String> }
pub enum PolicyRef { Builtin(Builtin), Named(String), Device(String) }   // Device = Ponte，加载时告警并视为 REJECT

pub enum HostEntry { Ip { pattern, addrs: Vec<IpAddr> }, Alias { pattern, target }, Server { pattern, servers: Vec<DnsUpstream> }, System { pattern, mode: SystemMode }, Script { pattern, name }, Set { set: ResourceRef, value: Box<HostValue> } }
```

`General` 是一个字段齐全的结构体：每个手册键一个字段，类型精确（枚举、`HostList`、`Vec<Listener>`、`Duration` 等），平台不适用的键也保留字段以便日志说明"已忽略"。旧键在解析时迁移到新字段并记一条 `Diagnostic::Info`。

### 4.3 诊断

```rust
pub struct Diagnostic { severity: Error | Warning | Info, code: &'static str, message: String, span: Option<Span>, hint: Option<String> }
```

规则（FR-CFG-03）：

- **错误**（拒绝生效）：语法无法解析的行属于有序节且无法跳过、缺少启用的 `FINAL`、规则引用不存在的策略或组、策略组循环引用、`[Proxy]` 未知协议关键字、重定义内置策略名（`DIRECT` 除外）、`#!include` 目标不存在、Requirement 表达式语法错误、`http-listen` 地址不是 IP 字面量。
- **警告**（继续加载）：未知键 / 节 / 规则参数、平台不适用项、Host List 非法项、外部资源暂不可达、协议尚未实现、旧键迁移提示、iOS 专属内置策略被视为 DIRECT。
- 每条诊断带稳定的 `code`（如 `E0102`），便于测试与文档引用。

## 5. 配置解析（M1，`rurge-config`）

### 5.1 流程

```
读取主文件 → 行级预处理（#!MANAGED-CONFIG、#!REQUIREMENT / 简写、行尾 //!REQUIREMENT）
→ 节切分（[Section]，命名节 [Ruleset x] / [WireGuard x] / [Tailscale x]）
→ #!include 展开（本地 / 多文件 / 通配命名节 / 远程 URL 经外部资源管理器取缓存）
→ 逐节解析为语义对象（值拆分：逗号分隔、引号、转义、行内注释）
→ 旧键迁移 → 交叉校验（策略引用、组循环、FINAL）→ Config + Diagnostics
```

- 注释：行首 `#` `;` `//`；行内注释仅当分隔符前有空白（FR-CFG-01）。
- 引号：`"..."` 内 `\"` `\\` 转义；引号值内的逗号不作分隔。
- 值拆分器是共享工具：`split_list(&str) -> Vec<Field>`，处理引号与括号嵌套（逻辑规则、WireGuard `peer=(...)`）。
- Requirement 表达式：独立的小型解析器 + 求值器（变量、比较 / 逻辑 / 字符串运算符、引号），环境由 `Environment { core_version, system, system_version, device_model, language, device_name }` 提供；不满足的行标记 `disabled=true` 并保留在文本层。
- 模块叠加、`{{{参数}}}`、`%APPEND%` / `%INSERT%` 在阶段 5 实现，但文本层的 `Origin::Module` 与叠加入口现在预留。
- 远程 `#!include` 与托管配置的下载、缓存、`interval` / `strict` 由 `rurge-net` 的外部资源管理器提供（M2），M1 先实现本地 include，远程 include 在 M2 接入。

### 5.2 `rurge check`

`rurge check -c <path> [--json] [--platform windows|linux|macos]`：加载并输出诊断（人类可读或 JSON），退出码：0 = 无错误（有警告也为 0）；1 = 有警告且指定了 `--strict`；2 = 有错误或文件无法读取。`--platform` 允许在一个平台上按另一个平台的语义校验。

## 6. 规则引擎（M2，`rurge-rules`）

### 6.1 接口

```rust
pub struct RuleEngine { /* 由 Config 与已加载的规则集、GeoIP 构建 */ }
impl RuleEngine {
    pub async fn evaluate(&self, session: &SessionInfo, resolver: &dyn LazyResolver) -> Decision;
    pub fn pre_matching_set(&self) -> &PreMatchingSet;   // 阶段 3 在 DNS / SYN 层使用
    pub fn rules(&self) -> &[CompiledRule];              // API GET /v1/rules
}
pub struct Decision { policy: PolicyRef, matched: Option<RuleRef>, reason: Reason, dns: Option<DnsResult>, notes: Vec<String> }
pub enum Reason { OutboundModeDirect, OutboundModeProxy, PreMatched, Rule, Final, DnsFailedFallback, DnsFailed }
```

`SessionInfo`（定义在 `rurge-config` 供各 crate 共用）：

```rust
pub struct SessionInfo {
    pub src: SocketAddr, pub in_port: u16, pub listener: ListenerKind /* Http | Socks5 | Tun | Forward */,
    pub dst_host: HostName, pub dst_port: u16, pub transport: Tcp | Udp,
    pub protocol: ProtocolHint /* Http | Https | Tcp | Udp | Quic | Stun | ... */,
    pub sni: Option<String>, pub http_host: Option<String>, pub user_agent: Option<String>, pub url: Option<String>,
    pub process: Option<ProcessInfo>, pub device: Option<DeviceInfo>,   // 阶段 3 / 7 填充
}
```

`LazyResolver` 只有一个方法 `async fn resolve(&self, host: &str) -> Result<DnsResult, DnsError>`，引擎在遇到第一个需要 IP 的规则时调用一次，结果缓存在本次 `evaluate` 内。

### 6.2 匹配器与索引

| 规则                                              | 实现                                                                                                                                                                 |
| ------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| DOMAIN / DOMAIN-SUFFIX                            | 主机名按标签反转后放入一棵后缀 trie（`example.com` → `com → example`），一次遍历同时得到精确与后缀命中；规则集里的域名条目也进这棵树并记录所属规则集与规则索引 |
| DOMAIN-KEYWORD                                    | 线性子串匹配（数量通常很少）                                                                                                                                         |
| DOMAIN-WILDCARD / USER-AGENT / DEVICE-NAME        | 自研 glob（大小写规则按手册：域名不敏感，UA / 设备名敏感）                                                                                                           |
| IP-CIDR / IP-CIDR6                                | prefix-trie，两棵（v4 / v6）                                                                                                                                         |
| GEOIP / IP-ASN                                    | maxminddb 查询，结果在本次评估内缓存                                                                                                                                 |
| URL-REGEX                                         | fancy-regex；`extended-matching` 时再对 SNI / Host 替换主机部分后的 URL 各匹配一次                                                                                 |
| DEST-PORT / SRC-PORT / IN-PORT                    | `PortExpr { Single, Range, Cmp }`                                                                                                                                  |
| SRC-IP                                            | 单地址精确或 prefix-trie                                                                                                                                             |
| PROTOCOL / HOSTNAME-TYPE                          | 枚举比较                                                                                                                                                             |
| SUBNET / CELLULAR-*                               | 阶段 1 无网络环境信息：SUBNET 视为不匹配并在首次评估时告警一次；CELLULAR-* 永不匹配                                                                                  |
| AND / OR / NOT                                    | 递归求值，短路；深度上限 10 在解析时检查                                                                                                                             |
| SCRIPT / PROCESS-NAME / DEVICE-NAME / MAC-ADDRESS | 阶段 1 视为不匹配（保留规则位置与计数）                                                                                                                              |
| RULE-SET / DOMAIN-SET                             | 见 6.3                                                                                                                                                               |
| FINAL                                             | 总是命中；`dns-failed`                                                                                                                                             |

规则表按顺序编译为 `Vec<CompiledRule>`；域名与 IP 索引是**跨规则的加速结构**：查询索引得到候选规则索引集合，再按规则顺序取最小索引，保证"自上而下首个命中"的语义不变。逻辑规则、关键词、正则等线性规则与索引命中一起按索引排序求值。

### 6.3 规则集与外部资源

- `ResourceRef { Internal(System | Lan), Inline(name), File(path), Url(url) }`，解析顺序与手册一致。
- 内部集 `SYSTEM` / `LAN` 内容按清单 3.5 内置为常量。
- 外部集由 `rurge-net::ResourceManager` 负责：磁盘缓存目录 `<data>/resources/<sha256(url)>`，`update-interval`（默认 86400，负数禁用），失败按 1 分钟起指数退避（上限 1 小时），本地文件用 notify 监视；更新完成后重新编译该规则集并原子替换（引擎持有 `ArcSwap<CompiledSet>`）。
- 文件格式校验：非法行跳过并告警；`FINAL` 与 `pre-matching` 出现在集合文件里是错误；上限 1,000,000 条。
- 嵌套（内联集引用其他集）：加载时做拓扑排序，循环报错，深度 8。
- 大集合：域名条目超过 1000 时仍用内存 trie（Rust 内存占用可控，不引入磁盘数据库；若基准显示内存超标再改），IP 条目一律进 prefix-trie。

### 6.4 GeoIP / ASN 管理器

- 数据目录 `<data>/geoip/GeoLite2-Country.mmdb` 与 `GeoLite2-ASN.mmdb`。
- Country 库来源：`geoip-maxmind-url`（接受 tar.gz 或 mmdb）；未配置时使用 rurge 内置默认 URL（构建时常量，M2 实施前确认镜像的许可与稳定性）。ASN 库来源：rurge 专有参数 `--geoip-asn-url` / 环境变量，同样有默认镜像。
- 首次运行无库时：GEOIP / IP-ASN 规则视为不匹配并告警，后台下载完成后热加载；`disable-geoip-db-auto-update` 语义与手册一致；库文件日期通过 API / CLI 可查。

### 6.5 出站模式

`OutboundMode { Direct, Proxy(global_policy), Rule }` 由 `rurge-engine` 的运行时状态提供；引擎 `evaluate` 的第一步检查模式，`Direct` / `Proxy` 直接返回，不触发 DNS。

## 7. DNS（M2，`rurge-dns`）

### 7.1 接口

```rust
pub struct Resolver { /* 上游列表、缓存、Host 映射、系统解析器 */ }
impl Resolver {
    pub async fn lookup(&self, host: &str, opts: LookupOpts) -> Result<DnsResult, DnsError>;
    pub fn flush(&self);
    pub fn cache_snapshot(&self) -> Vec<CacheEntry>;      // GET /v1/dns
    pub async fn measure_delay(&self) -> Vec<(Upstream, Duration)>;   // POST /v1/test/dns_delay
}
pub struct DnsResult { v4: Vec<Ipv4Addr>, v6: Vec<Ipv6Addr>, ttl: Duration, source: Source /* Cache | Upstream(name) | Host | System | Literal */ }
```

### 7.2 查询流程

1. IP 字面量直接返回；尾部 `.` 剥离并禁用搜索域。
2. `[Host]` 链：按顺序首个命中；IP 映射直接返回；别名重启查询（上限 8 次防环）；`server:` 指定上游；`server:system` / `syslib` 走系统解析（`tokio::net::lookup_host`，阶段 3 增强模式下改为向系统配置的 DNS 服务器转发）；`DOMAIN-SET:` / `RULE-SET:` 键用规则集索引匹配。代理服务器主机名在 Config 中标记，不经过 `[Host]`。
3. 简单主机名：追加系统首个搜索域后交系统解析；`.local` 交系统解析。
4. 缓存：键为域名，值为 A / AAAA 记录与到期时间；命中且未过期直接返回；过期则返回旧值并触发后台刷新（乐观缓存）；LRU 上限默认 2000。
5. 上游查询：向全部上游并发发送；1 秒无应答重发；5 次后失败；`ipv6=true` 且本机有 IPv6 时并发 A 与 AAAA；连续 5 次 AAAA 超时后抑制 AAAA 直到网络变化或 flush；空应答语义按手册。
6. 上游类型：UDP（默认）、`tcp://`（持久连接，配置后 UDP 上游只用于引导）、`https://`（DoH，经 `rurge-net` HTTP 客户端，HTTP/2 优先）、`tls://`（DoT，持久 TLS 连接）；`h3://` 与 `quic://` 阶段 2。引导豁免：加密上游 URL 中的主机名只由传统上游解析。
7. `encrypted-dns-follow-outbound-mode`：阶段 1 加密 DNS 连接只走 DIRECT（因为没有代理策略），但连接经由 `rurge-engine` 的会话流水线创建，规则可以看到 `PROTOCOL,DOH` 等会话；阶段 2 自然获得走代理的能力。

### 7.3 系统 hosts

`read-etc-hosts` 默认 true：读取平台 hosts 文件，条目追加在 `[Host]` 之后，用 notify 监视变化。

## 8. 连接流水线（M3，`rurge-engine` 等）

### 8.1 组件

| crate             | 内容                                                                                                                                                                                                                                                                                                                                                                      |
| ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `rurge-inbound` | `HttpListener`：HTTP/1.1 代理，`CONNECT` 隧道、绝对 URI 的明文请求转发、Basic 认证（`password@`）、keep-alive 与请求级复用（一个客户端连接上的多个明文请求可能命中不同规则与目标，逐请求处理）；`Socks5Listener`：RFC 1928 无认证、`CONNECT` 命令，`UDP ASSOCIATE` 返回不支持（阶段 2）。两者产出 `Inbound { session: SessionInfo, stream: InboundStream }` |
| `rurge-proto`   | `Outbound` trait：`async fn connect_tcp(&self, target, opts) -> Result<Box<dyn AsyncStream>>`；阶段 1 实现 `Direct`（带 `ip-version` 基本语义与 happy-eyeballs 式并发连接）与 `Reject { kind }`                                                                                                                                                                 |
| `rurge-policy`  | `PolicyRegistry`：按名字解析策略 / 组 / 别名；组的**决策**在阶段 1 只实现 `select`（持久化选择，否则第一个成员）；其他组类型阶段 1 采用临时策略「第一个成员」并记一条警告，阶段 2 替换为真实算法；组循环在配置校验期报错                                                                                                                                        |
| `rurge-engine`  | `Engine`：持有 `ArcSwap<Runtime>`（`Runtime` = Config + RuleEngine + Resolver + PolicyRegistry + Outbounds），`handle(inbound)` 执行流水线，请求记录、流量统计、运行时状态、`reload(new_config)`                                                                                                                                                                |

### 8.2 流水线（每个入站会话一个 tokio 任务）

```
1. 建立 SessionInfo（来源、目标、监听端口、协议提示；HTTP 代理请求附带 url / host / user-agent）
2. 协议嗅探：CONNECT 隧道的首个客户端片段做 TLS ClientHello 解析得到 SNI（用于 extended-matching 与 HTTPS 识别）；不消费数据，回填后原样转发
3. 出站模式判断
4. RuleEngine.evaluate（按需 DNS）
5. PolicyRegistry.resolve(decision.policy) → 具体 Outbound；未实现的协议 → RejectUnsupported
6. 出站建立（DIRECT：解析目标、并发连接、绑定选项预留）；REJECT：按类型生成响应 / 断开 / 丢弃
7. 双向转发（tokio::io::copy_bidirectional 变体，统计上下行字节，空闲超时）
8. 请求记录状态从 Active → Completed / Failed / Rejected；写日志
```

### 8.3 REJECT 行为（FR-OUT-01、FR-HTTP-12）

| 策略           | 明文 HTTP                                                                                     | CONNECT / 原始 TCP |
| -------------- | --------------------------------------------------------------------------------------------- | ------------------ |
| REJECT         | `show-error-page-for-reject=true` 时返回 rurge 错误页（HTML，说明命中的规则），否则直接关闭 | 关闭连接           |
| REJECT-TINYGIF | 返回 200 + 1px 透明 GIF                                                                       | 关闭连接           |
| REJECT-DROP    | 不响应，保持连接直到客户端超时或空闲超时（可配，默认 30 秒）                                  | 同左               |
| REJECT-NO-DROP | 同 REJECT，但不参与自动升级                                                                   | 同左               |

自动升级：同一目标主机 30 秒内触发 REJECT / REJECT-TINYGIF 达 50 次后，后续按 REJECT-DROP 处理（滑动窗口计数器，按主机名或 IP 键控）。RST 限频保护属于阶段 3 的 SYN 层实现。

`show-error-page=true` 时，明文 HTTP 请求的连接失败（DNS 失败、连接超时、协议未实现）返回错误页；CONNECT 请求返回 502。

### 8.4 请求记录与流量统计（FR-HTTP-10、FR-OBS-05、FR-OBS-10）

- `RequestRecord { id: u64, started_at, state, session: SessionInfo 摘要, rule: Option<String>, policy_chain: Vec<String>, remote_addr, bytes_up, bytes_down, timings { dns, connect, first_byte, total }, notes, error }`。
- 环形缓冲，容量默认 1000（CLI 参数可调）；活动请求单独索引以支持 `kill`。
- 流量：总计、按策略、按监听器的原子计数器；每秒采样计算实时速率；`GET /v1/traffic` 输出。
- 日志中每条与会话相关的记录带 `session_id` 字段，与请求记录 id 一致。

### 8.5 未实现功能的处理（阶段 1 的关键约定）

| 情况                                                  | 行为                                                                                                                    |
| ----------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| 规则命中尚未实现的代理协议                            | 连接以`REJECT` 处理，请求记录 `error = "policy protocol not implemented: <type>"`，加载配置时对每个此类策略告警一次 |
| 规则命中自动类型策略组                                | 用第一个成员（递归解析），加载时告警"组算法将在阶段 2 实现"                                                             |
| `DEVICE:<name>` 策略                                | 视为 REJECT，加载时告警                                                                                                 |
| iOS 专属内置策略                                      | 视为 DIRECT，加载时告警                                                                                                 |
| 需要增强模式才有意义的规则（SUBNET、PROCESS-NAME 等） | 不匹配，首次求值告警一次                                                                                                |

选择 REJECT 而不是 DIRECT 是为了避免用户以为流量走了代理实际却直连泄露，与 Surge 对 iOS 上 `external` 策略的处理一致。

## 9. 系统代理与平台层（M4，`rurge-platform`）

| 平台    | 设置                                                                                                                                                                                                                            | 恢复                                                                              |
| ------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------- |
| Windows | `HKCU\...\Internet Settings` 的 `ProxyEnable` / `ProxyServer`（`http=...;https=...;socks=...`）/ `ProxyOverride`（`skip-proxy` + `exclude-simple-hostnames` → `<local>`），随后 `InternetSetOption` 通知刷新 | 启动时把原值存入`<data>/state.json`，退出、`suspend` 或下次启动发现残留时恢复 |
| macOS   | 对所有活动网络服务调用`networksetup -setwebproxy / -setsecurewebproxy / -setsocksfirewallproxy / -setproxybypassdomains`（需要管理员权限，文档说明；阶段 6 评估 SystemConfiguration 直写）                                    | 同上                                                                              |
| Linux   | GNOME：`gsettings set org.gnome.system.proxy ...`；KDE：`kwriteconfig6`；其他桌面：打印 `http_proxy` / `https_proxy` / `no_proxy` 提示                                                                                | 同上                                                                              |

其他平台职责：默认目录（需求文档第 6 节）、hosts 路径、进程信号处理、服务安装（`rurge service install/uninstall` 生成 systemd unit / launchd plist / Windows 服务注册，M4 的 P1）。

## 10. 控制面（M4，`rurge-api` 与 CLI）

### 10.1 HTTP API

- axum 服务，监听 `http-api` 指定地址；未配置则不启动。鉴权中间件接受 `X-Key` 头或 `?x-key=`；连续 5 次错误鉴权后封禁来源 IP 10 分钟（阶段 6 完善 `security ban`）。
- 阶段 1 端点与响应：

| 端点                                                                                                                                                   | 阶段 1 行为                                                                                 |
| ------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------- |
| `GET/POST /v1/outbound`、`/v1/outbound/global`                                                                                                     | 读写运行时状态并持久化                                                                      |
| `GET /v1/policies`                                                                                                                                   | `{"proxies":[...],"policy-groups":[...]}`（结构为暂定，见 10.3）                          |
| `GET /v1/policies/detail?policy_name=`                                                                                                               | 返回策略行的解析结果                                                                        |
| `GET /v1/policy_groups`、`GET/POST /v1/policy_groups/select`                                                                                       | 组列表与 select 决策                                                                        |
| `GET /v1/requests/recent`、`/v1/requests/active`、`POST /v1/requests/kill`                                                                       | 请求记录                                                                                    |
| `GET /v1/profiles/current?sensitive=`、`POST /v1/profiles/reload`、`GET /v1/profiles`、`POST /v1/profiles/switch`、`POST /v1/profiles/check` | 配置管理                                                                                    |
| `GET /v1/dns`、`POST /v1/dns/flush`、`POST /v1/test/dns_delay`                                                                                   | DNS                                                                                         |
| `GET /v1/rules`                                                                                                                                      | 规则列表（含来源、是否禁用、命中次数）                                                      |
| `GET /v1/traffic`                                                                                                                                    | 流量                                                                                        |
| `POST /v1/log/level`                                                                                                                                 | 会话内日志级别                                                                              |
| `GET/POST /v1/features/system_proxy`                                                                                                                 | 系统代理开关                                                                                |
| `GET /v1/features/{mitm,capture,rewrite,scripting,enhanced_mode}`                                                                                    | 返回`{"enabled":false}`；POST 返回 501 与 `{"error":"not implemented in this version"}` |
| `GET /v1/modules`、`GET /v1/scripting`、`GET /v1/events`                                                                                         | 返回空列表，保证面板不报错                                                                  |
| `POST /v1/stop`                                                                                                                                      | 停止引擎并退出进程                                                                          |

### 10.2 CLI（阶段 1）

```
rurge run   -c <conf> [--config-dir] [--data-dir] [--system-proxy] [--listen-http ADDR] [--listen-socks5 ADDR] [--log-file PATH] [--watch]
rurge check -c <conf> [--json] [--strict] [--platform ...]
rurge reload | stop | status      # 通过本机 API（地址与密钥来自当前配置或 --remote/--key）
rurge rule match <host[:port]> [--url] [--src] [--protocol] ...   # M2 开发命令，阶段 6 正式化
rurge service install|uninstall   # M4，P1
rurge version
```

### 10.3 与 Surge 未文档化响应结构的关系

`/v1/policies`、`/v1/requests/*`、`/v1/policy_groups` 等端点的 JSON 结构手册未定义。阶段 1 采用自定义但稳定的结构并在 `docs/api/` 记录；阶段 6 用真实 Surge 实例与第三方面板的请求样本校准。字段一旦对齐 Surge，以 Surge 为准，本阶段的结构不做兼容承诺。

## 11. 运行时状态与重载

- `<data>/state.json`：`outbound_mode`、`global_policy`、`features`、`group_selections[profile][group]`、`system_proxy_backup`、`current_profile`。写入采用临时文件 + 重命名。
- 重载：`Engine::reload` 加载新配置（失败则保留旧配置并返回诊断）→ 构建新 `Runtime`（复用 DNS 缓存与外部资源缓存，重编译规则）→ `ArcSwap::store` → 新会话使用新配置，旧会话自然结束 → 若监听地址变化则重建监听器 → 记录事件。
- 触发：`POST /v1/profiles/reload`、`rurge reload`、SIGHUP（Unix）、`--watch` 下的文件变化（去抖 500 ms）。

## 12. 日志（FR-OBS-01）

- `loglevel` 映射：`verbose` → TRACE，`info` → DEBUG，`notify` → INFO（默认），`warning` → WARN。API `/v1/log/level` 额外接受 `debug` → DEBUG、`error` → ERROR。
- 输出：stdout（人类可读，TTY 时着色）与可选滚动文件（`--log-file`，按天滚动，保留 7 个）。
- 字段约定：`session`（请求 id）、`rule`、`policy`、`dst`、`src`、`upstream`；凭据（`password@`、API key、策略密码）不进入日志。

## 13. 错误处理

| 层             | 策略                                                                                           |
| -------------- | ---------------------------------------------------------------------------------------------- |
| 配置           | 错误 / 警告分级（4.3）；错误时`run` 拒绝启动、`reload` 保留旧配置                          |
| 会话           | 每个会话独立任务，错误转成请求记录的`error` 与一条 WARN 日志；panic 由任务边界隔离并记 ERROR |
| DNS / 外部资源 | 失败带退避重试；不阻塞启动                                                                     |
| API            | 统一 JSON 错误体`{"error": "..."}`，鉴权失败 401，未实现 501，参数错误 400                   |
| 进程           | 收到 SIGINT / SIGTERM / Ctrl-C 时恢复系统代理、写状态、关闭监听、等待会话最多 5 秒             |

## 14. 测试策略

| 里程碑 | 测试                                                                                                                                                                                     |
| ------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| M1     | 解析器单元测试；insta 快照（语料库每个文件的`Config` 与诊断）；手册中每个配置示例作为用例；旧键迁移用例；Requirement 求值表驱动测试                                                    |
| M2     | 匹配器单元测试；proptest（随机规则集下"索引加速结果 == 朴素顺序求值结果"）；手册规则示例黄金测试；DNS 客户端对本地模拟上游的集成测试（超时、重发、并发、缓存、AAAA 抑制）；[Host] 链用例 |
| M3     | 本地回环集成测试：启动引擎 + 本地目标服务器，HTTP 代理 / SOCKS5 / CONNECT / REJECT 各行为；请求记录断言；`copy_bidirectional` 统计                                                     |
| M4     | API 端到端测试（reqwest 仅用于测试）；系统代理设置的单元测试（对注册表 / 命令封装做 trait 抽象后 mock）；CLI 冒烟                                                                        |
| 全部   | GitHub Actions：ubuntu / windows / macos 矩阵，`cargo fmt --check`、`cargo clippy -D warnings`、`cargo test`；criterion 基准（规则匹配、DNS 缓存）作为独立 job                     |

语料库：`tests/corpus/` 收集脱敏的公开 Surge 配置与规则集，附来源与许可说明；初始由手册示例与自写样例组成，后续持续补充。

## 15. 阶段 1 验收标准（对应需求文档第 7 节）

1. 语料库中的配置全部加载无错误；对故意破坏的配置，`rurge check` 给出准确文件与行号。
2. HTTP 与 SOCKS5 入站经 DIRECT 转发通过本地集成测试；REJECT 四种行为符合 8.3。
3. 规则引擎黄金测试与属性测试全部通过；GeoIP / ASN 查询正确。
4. UDP / TCP / DoH / DoT 上游解析正常，缓存与重试行为可观测。
5. 三平台系统代理开关后，浏览器流量出现在请求记录并按规则分流。
6. 阶段 1 端点全部可用，`rurge status` 能显示模式、策略数、规则数、活动请求数。
7. 三平台 CI 绿；clippy 零警告。

## 16. 已决事项与开放问题

| 编号 | 事项                                      | 决定                                             |
| ---- | ----------------------------------------- | ------------------------------------------------ |
| D1   | 未实现协议的策略如何处理                  | REJECT + 告警（8.5）                             |
| D2   | 自动类型策略组在阶段 1 的决策             | 第一个成员 + 告警，阶段 2 替换                   |
| D3   | 新增`rurge-engine`、`rurge-net` crate | 采纳，更新需求文档 3.2 的 crate 表               |
| D4   | 正则引擎                                  | fancy-regex                                      |
| D5   | 大规则集是否用磁盘数据库                  | 阶段 1 全内存，基准后再定                        |
| D6   | 日志级别映射                              | 见第 12 节                                       |
| D7   | 未文档化 API 响应结构                     | 自定义并标注暂定（10.3）                         |
| Q1   | GeoIP / ASN 默认镜像 URL 与许可           | M2 实施前确认                                    |
| Q2   | macOS 系统代理是否必须 sudo               | M4 实施时验证`networksetup` 权限行为           |
| Q3   | 语料库来源与许可                          | 持续收集；欢迎项目所有者提供自己的配置（脱敏后） |
