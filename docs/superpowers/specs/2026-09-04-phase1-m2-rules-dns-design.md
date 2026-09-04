# 阶段 1 里程碑 M2「规则引擎与 DNS」设计文档

- 日期：2026-09-04
- 状态：草案，待项目所有者评审
- 上游文档：`docs/requirements.md`（PRD）、`docs/surge-compatibility-matrix.md`、`docs/superpowers/specs/2026-09-03-phase1-core-skeleton-design.md`（阶段 1 设计，下称「阶段 1 设计」）
- 手册基线：Surge 官方手册 2026-09 版（Surge Mac 6.9 / iOS 5.22）。本文引用的手册页面：`rules/ruleset.html`、`rules/domain.html`、`rules/ip.html`、`rules/overview.html`、`rules/logical.html`、`rules/final.html`、`dns/overview.html`、`dns/dns-server.html`、`dns/encrypted-dns.html`、`dns/local-dns-mapping.html`、`dns/advanced.html`、`profile/general.html`

本文细化阶段 1 设计第 6、7 节，接口签名以本文为准；与阶段 1 设计不一致之处在第 12 节列出。

## 1. 目标与范围

### 1.1 目标

M2 交付 rurge 的规则引擎、DNS 客户端与外部资源管理，使 M3 的连接流水线只需组装即可按规则分流。M2 结束时：

- 任意通过 `rurge check` 的配置都能构建出 `RuleEngine`，对给定会话信息返回与 Surge 一致的决策；规则集（内部 / 内联 / 本地文件 / URL）与 GeoIP / ASN 库可加载、可热更新。
- `Resolver` 能通过 UDP / TCP / DoH / DoT 上游解析域名，`[Host]`、系统 hosts、缓存、重试与 AAAA 抑制行为与手册一致。
- 两个离线开发命令 `rurge rule match` 与 `rurge dns lookup` 可用于验收和日后排障。

### 1.2 范围内（PRD 编号）

| 编号 | 内容 | 所属子里程碑 |
| --- | --- | --- |
| FR-RULE-01 | 29 种规则类型的匹配（`PROCESS-NAME` 匹配器实现，进程信息由 M3 提供；`SCRIPT` `DEVICE-NAME` `MAC-ADDRESS` `SUBNET` `CELLULAR-*` 按 4.5 处理） | M2a |
| FR-RULE-02 | 10 个规则参数在匹配时的语义 | M2a |
| FR-RULE-03 | 评估语义：顺序首个命中、pre-matching 提取、出站模式旁路、域名规则不触发 DNS、IP 规则按需解析、A / AAAA 选取、DNS 失败与 `dns-failed`、逻辑规则、FINAL 取最后 | M2a |
| FR-RULE-04 | 规则集：内部 / 内联 / 文件 / URL、嵌套 8 层与循环拒绝、文件格式、上限 1,000,000、行级参数作用于整集 | M2a |
| FR-RULE-05 | 规则集索引：域名精确 / 后缀索引、IP 前缀树、ASN 常数时间 | M2a |
| FR-RULE-06 | GeoIP / ASN：mmdb 读取、`geoip-maxmind-url`（tar.gz 或 mmdb）、自动更新与 `disable-geoip-db-auto-update`、库日期可查 | M2a |
| FR-RULE-07 | `extended-matching` 的匹配语义（SNI / Host 由 M3 的嗅探填入 `SessionInfo`） | M2a |
| FR-RULE-09 | 出站模式作为 `evaluate` 的输入（运行时切换与持久化在 M3 / M4） | M2a |
| FR-RULE-15 | 子规则命中信息（`Decision.sub_rule`，M3 写日志） | M2a |
| FR-CFG-14 | 外部资源管理器：下载、磁盘缓存、`update-interval`、失败退避、本地文件监视、强制更新 | M2a |
| FR-CFG-15（部分） | 数据目录布局（第 9 节）与 `rurge-platform::dirs` | M2a |
| FR-DNS-01 | 内部 DNS 客户端全部行为 | M2b |
| FR-DNS-02 | 特殊主机名 | M2b |
| FR-DNS-03 | `dns-server` 语法与语义 | M2b |
| FR-DNS-04（阶段 1 部分） | `https://` `tls://` `tcp://` 上游、引导豁免、`encrypted-dns-skip-cert-verification` | M2b |
| FR-DNS-05 | `[Host]` 映射链（`script:` 阶段 5，`force-syslib` 在 M2 等同 `syslib`） | M2b |
| FR-DNS-06 | 系统 hosts 文件 | M2b |
| FR-DNS-11（部分） | `dns lookup` 的离线形式；缓存快照与延迟测试接口（API 端点 M4） | M2b |
| NFR-01 / NFR-02 | 10 万条规则集下单次匹配 p99 < 50 µs；DNS 缓存命中 < 1 ms；外部资源下载不阻塞启动 | M2a / M2b |

### 1.3 范围外

- fake-IP 应答器、`hijack-dns`、`always-real-ip` 的运行时行为（M3，FR-DNS-08）。
- `encrypted-dns-follow-outbound-mode` 的走规则能力（M3 注入连接器后自然获得，见 7.7）。
- `h3://` `quic://` 上游（阶段 2）：M2 解析后告警忽略。
- `use-local-host-item-for-proxy`（阶段 2）、`dns-follow-interface`（阶段 2）、`[SSID Setting]` DNS 覆盖（阶段 3）。
- 网络变化检测（M3 / M4 触发 `Resolver::on_network_change`）。
- 临时规则、规则命中计数 API、`rule explain`（阶段 6；本设计只预留计数器与跟踪数据）。
- HTTP API 端点（M4）。

### 1.4 子里程碑

| 子里程碑 | 内容 | 验收产出 |
| --- | --- | --- |
| M2a 规则 | `rurge-net`（HTTP 客户端、连接器、资源管理器）、`rurge-rules`（引擎、索引、规则集、GeoIP / ASN）、`rurge-platform::dirs`、`rurge-config` 补充（`SessionInfo`、FINAL 与 DOMAIN-SET 修正）、`rurge rule match` | 黄金测试、属性测试、基准；`rurge rule match` 对语料库输出预期决策 |
| M2b DNS | `rurge-dns`（上游、并发查询、缓存、`[Host]`、hosts 文件、特殊主机名）、`rurge-platform::dns`、`rurge dns lookup` | 本地模拟上游集成测试（UDP / TCP / DoT / DoH）；`rurge dns lookup` 可观测缓存与重试 |

每个子里程碑一份实施计划，独立分支、独立验收。

## 2. 技术选型（M2 新增依赖）

| 用途 | crate | 说明 |
| --- | --- | --- |
| 异步运行时 | `tokio` 1（features：rt-multi-thread, net, time, fs, sync, macros, io-util） | 全部 crate 共用 |
| HTTP 客户端 | `hyper` 1 + `hyper-util`（client-legacy, http1, http2）+ `http-body-util` + `bytes` | 自建客户端以便注入连接器；不用 reqwest（无法注入自定义连接建立） |
| TLS | `rustls` 0.23 + `tokio-rustls` 0.26 + `hyper-rustls` 0.27 + `rustls-native-certs` 0.8（回退 `webpki-roots`） | 系统根证书优先；`encrypted-dns-skip-cert-verification` 用自定义 verifier |
| URL | `url` 2 | 资源与上游 URL 解析 |
| 哈希 / 压缩 | `sha2` 0.10、`flate2` 1 + `tar` 0.4 | 缓存目录名；GeoIP tar.gz 解包 |
| 文件监视 | `notify` 8（稳定版）+ 自写 500 ms 防抖 | 本地规则集 / hosts 文件 |
| 原子替换 | `arc-swap` 1 | 规则集与 GeoIP 热更新 |
| IP 前缀树 | `prefix-trie` 0.10 | v4 / v6 各一棵 |
| mmdb | `maxminddb` 0.30 | 内存映射或字节数组打开 |
| DNS 报文 | `hickory-proto` 0.26 | 只用报文编解码，不用其 resolver（需要自定义并发 / 重试语义） |
| 缓存 | `lru` 0.18 | DNS LRU；自写 TTL 与乐观刷新 |
| 系统 DNS 发现 | Windows：`ipconfig` 0.3；Unix：`resolv-conf` 0.7；IPv6 探测：`if-addrs` 0.15 | 只在 `rurge-platform` 中出现（AR-02） |
| 日志 | `tracing` 0.1 | 运行时告警与调试输出 |
| 测试 | `proptest`、`rcgen` 0.14（自签证书）、`criterion` 0.8（基准，独立 CI job）、`toml`（黄金用例文件） | |

版本在实施计划中锁定；升级需通过 CI。

## 3. crate 边界与依赖方向

```
rurge (bin) ──► rurge-dns ──► rurge-rules ──► rurge-net ──► rurge-config
     │                                                          ▲
     └─────────► rurge-platform（无内部依赖）                    │
                 （bin 用适配器把 platform 函数实现为 dns / net 的 trait）
```

| crate | 职责 | 公共接口所在节 |
| --- | --- | --- |
| `rurge-config`（补充） | `SessionInfo` 及枚举；`Config::proxy_hostnames()`；FINAL / DOMAIN-SET 修正 | 4.1 |
| `rurge-net` | `Connector` trait 与 `DirectConnector`；`HttpClient`；`ResourceManager` | 5 |
| `rurge-rules` | 索引、`CompiledSet` 与 `SetRegistry`、`GeoDb` 与更新器、`RuleEngine`、`PreMatchingSet`、`LazyResolver` trait | 6 |
| `rurge-dns` | `Resolver`、上游、并发查询、缓存、`HostMap`、`SystemDns` trait | 7 |
| `rurge-platform` | `dirs`（M2a）、`dns`（M2b）：系统 DNS 服务器、搜索域、hosts 路径、IPv6 探测 | 8 |
| `rurge`（bin） | `rule match`、`dns lookup` 子命令；运行时选项；平台适配器 | 10 |

与阶段 1 设计第 3 节的差异：`rurge-dns` 依赖 `rurge-rules`（原为并列）。原因：`[Host]` 的 `DOMAIN-SET:` / `RULE-SET:` 键需要规则集索引，且 `LazyResolver` trait 由 `rurge-rules` 定义、`rurge-dns` 实现。`rurge-rules` 不依赖 `rurge-dns`，方向仍是单向。

## 4. `rurge-config` 补充

### 4.1 会话信息（新增 `session.rs`）

```rust
pub struct SessionInfo {
    pub src: SocketAddr,
    pub in_port: u16,
    pub listener: ListenerKind,          // Http | Socks5 | Tun | Forward | Internal（rurge 自身发起，如 DoH）
    pub dst_host: HostName,              // Domain | Ipv4 | Ipv6（已小写、已去尾点）
    pub dst_port: u16,
    pub transport: Transport,            // Tcp | Udp
    pub protocol: Option<ProtocolKind>,  // 嗅探结果；None = 未知
    pub sni: Option<String>,
    pub http_host: Option<String>,
    pub user_agent: Option<String>,
    pub url: Option<String>,             // 仅 HTTP 明文或 MITM 后可得
    pub process: Option<ProcessInfo>,    // M3 填充：{ name, path }
    pub device: Option<DeviceInfo>,      // 阶段 7 填充：{ name, mac }
}
impl SessionInfo { pub fn hostname_type(&self) -> HostnameType; }  // IPv4 | IPv6 | Domain | Simple（无点）
```

`HostName::parse` 已存在；M2 增加 `HostName::as_domain(&self) -> Option<&str>` 与 `as_ip(&self) -> Option<IpAddr>`。

### 4.2 修正项（来自 M1 延后事项）

- 生效 FINAL：手册「多条 FINAL 取最后一条」。`Config` 增加 `effective_final(&self) -> Option<usize>`（最后一条 FINAL 的下标）；W0019「FINAL 之后的规则被忽略」改为以最后一条 FINAL 为锚点；此前的重复 FINAL 各报一条 W0019 变体「earlier FINAL is ignored」。
- `DOMAIN-SET,<name>`：不再解析为内联 `[Ruleset]`（手册只允许 RULE-SET 引用内联集）；无法解析为文件或 URL 时按 `File` 相对路径处理，运行时找不到文件再告警。
- `Config::proxy_hostnames(&self) -> HashSet<String>`：全部 `[Proxy]` 策略的 server 字段中的域名（小写），供 `Resolver` 跳过 `[Host]`。

## 5. `rurge-net`

### 5.1 连接器

```rust
pub struct Target { pub host: HostName, pub port: u16 }
pub struct ConnectOpts { pub timeout: Duration /* 默认 10 s */, pub prefer_v6: bool }
pub trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send {}
pub type BoxedStream = Box<dyn AsyncStream>;
pub trait Connector: Send + Sync {
    fn connect<'a>(&'a self, target: &'a Target, opts: &'a ConnectOpts) -> BoxFuture<'a, io::Result<BoxedStream>>;
}
pub trait Resolve: Send + Sync {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>>;
}
pub struct SystemResolve;                              // tokio::net::lookup_host
pub struct DirectConnector { resolver: Arc<dyn Resolve> }
```

`DirectConnector`：IP 字面量直接连接；域名经 `Resolve` 得到地址列表，按「v4 / v6 交错、每个地址独立超时 = 总超时 / 地址数（下限 2 s）」顺序尝试，任一成功即返回（简化版 Happy Eyeballs，不并发以避免半开连接）。M3 的引擎用「走流水线的连接器」替换它，接口不变。

### 5.2 HTTP 客户端

```rust
pub struct HttpClient { /* hyper-util legacy client + 自定义 connector 服务 + rustls */ }
pub struct HttpClientConfig { pub user_agent: String /* "rurge/<ver>" */, pub skip_cert_verification: bool, pub connect_timeout: Duration }
pub struct RequestOpts { pub timeout: Duration /* 30 s */, pub max_body: u64 /* 64 MiB */, pub headers: Vec<(HeaderName, HeaderValue)>, pub follow_redirects: u8 /* 5 */ }
pub struct Response { pub status: StatusCode, pub headers: HeaderMap, pub body: Bytes }
impl HttpClient {
    pub fn new(connector: Arc<dyn Connector>, cfg: HttpClientConfig) -> Result<HttpClient, HttpError>;
    pub async fn get(&self, url: &Url, opts: &RequestOpts) -> Result<Response, HttpError>;
    pub async fn post(&self, url: &Url, body: Bytes, opts: &RequestOpts) -> Result<Response, HttpError>;
    pub async fn send(&self, req: http::Request<Full<Bytes>>, timeout: Duration) -> Result<http::Response<Incoming>, HttpError>;  // DoH 与大文件用
}
pub enum HttpError { InvalidUrl, Connect(io::Error), Tls(String), Timeout, TooLarge, Status(StatusCode), Protocol(String) }
```

- HTTP/1.1 与 HTTP/2 通过 ALPN 协商；每个 (scheme, host, port) 一个连接池（hyper-util 内置）。
- `get` / `post` 只支持 `http` / `https`；重定向只跟随 GET，最多 5 次，跨 scheme 降级（https→http）拒绝。
- `send`（原设计为 `stream`）接收调用方已构造好的 `Request`，只按 `timeout` 发送并返回未读取的流式响应；不经过 `get` / `post` 内部的 `build()`（不设 scheme 校验、不设 User-Agent、不设 `max_body`、不跟随重定向），调用方（DoH 等）自行提供 User-Agent 与响应体大小上限。
- 根证书：`rustls-native-certs` 加载失败时回退 `webpki-roots` 并告警一次。
- 没有代理支持：M3 起通过连接器实现「走策略」，客户端本身不感知。

### 5.3 外部资源管理器

```rust
pub enum ResourceSource { Url(Url), File(PathBuf) }
pub struct ResourceSpec { pub source: ResourceSource, pub update_interval: Option<i64> /* None → 86400；负数禁用 */ }
pub struct ResourceManager { /* root, client, 任务表 */ }
impl ResourceManager {
    pub fn new(root: PathBuf, client: Arc<HttpClient>) -> Arc<ResourceManager>;
    pub fn get(&self, spec: &ResourceSpec) -> ResourceHandle;     // 同一 source 共享一个条目；多个 spec 取最小正 interval
    pub async fn wait_initial(&self, timeout: Duration) -> Vec<ResourceStatus>;   // 开发命令与测试用：等待首轮抓取
    pub fn force_update(&self, source: &ResourceSource) -> bool;
    pub fn statuses(&self) -> Vec<ResourceStatus>;                 // M4 API 用
}
pub struct ResourceHandle { /* Arc 内部条目 */ }
impl ResourceHandle {
    pub fn current(&self) -> ResourceState;
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64>;  // 版本号递增即变化
}
pub enum ResourceState {
    Missing,                                                     // 无缓存且尚未成功
    Available { data: Arc<Bytes>, version: u64, fetched_at: SystemTime, stale: bool },
    Failed { last_error: String, since: SystemTime, cached: Option<Arc<Bytes>> },
}
pub struct ResourceStatus { pub source: ResourceSource, pub state: &'static str, pub version: u64, pub fetched_at: Option<SystemTime>, pub next_refresh: Option<SystemTime>, pub last_error: Option<String> }
```

行为：

1. **磁盘缓存**：`<root>/resources/<sha256(url) 的小写 hex>/data` 与 `meta.json`（`url`、`etag`、`last_modified`、`fetched_at`、`update_interval`）。启动时有缓存立即进入 `Available`（`stale = 已过期`），不阻塞（NFR-02）；无缓存进入 `Missing` 并立刻抓取。
2. **刷新调度**：每个 URL 一个 tokio 任务；到期后抓取；成功则写临时文件后 `rename` 原子替换并广播新版本；`update_interval < 0` 只在无缓存时抓取一次。刷新时间加 ±10% 抖动，避免整点风暴。
3. **条件请求**：有 `etag` 发 `If-None-Match`，有 `last_modified` 发 `If-Modified-Since`；304 只更新 `fetched_at`。手册未提及，属于 rurge 内部优化，不影响兼容性。
4. **失败退避**：从 60 s 起指数退避到 3600 s；退避期间状态为 `Failed { cached }`，缓存数据继续可用。任何非 2xx / 304、超时、体积超限都算失败。
5. **本地文件**：相对路径以主配置所在目录为基准（与 M1 `ParseCtx.base_dir` 一致）；读取失败进入 `Failed`；用 `notify` 监视父目录，500 ms 防抖后重读并广播。文件被删除进入 `Failed { cached: 上次内容 }`。
6. **强制更新**：`force_update` 取消当前等待、立即抓取一次；M4 的 API 调用它。
7. **上限**：单资源 64 MiB；管理器内条目数无上限（由配置决定）。
8. **生命周期**：`ResourceManager` 不提供单条目退休（retire）接口；约定为「一个配置代数一个 `ResourceManager`」——重载时构建新管理器并丢弃旧的，缓存文件从磁盘重新读取，旧管理器的后台任务在 60 秒内感知到自身被丢弃后退出。

`GeoDb` 的两个数据库也通过 `ResourceManager` 抓取（`update_interval = 7 天`），见 6.4。

## 6. `rurge-rules`

### 6.1 索引结构

**域名索引 `DomainIndex`**（每个 `CompiledSet` 一份）：

- 键为「标签反转后用 `.` 连接」的小写字符串（`www.example.com` → `com.example.www`），存放在按字典序排序的 `Vec<Box<str>>` 中，配两组平行数组 `exact: Vec<u32>` 与 `suffix: Vec<u32>`（值为条目下标，`u32::MAX` 表示无）。
- 查询 `a.b.c`：生成 `c`、`c.b`、`c.b.a` 三个反转前缀，各做一次二分查找；`suffix` 命中任一即后缀命中，`exact` 只看完整键。复杂度 O(k · log n)，k 为标签数。
- 选择排序数组而非 trie：1,000,000 条时内存约 60 MB（trie 的 HashMap 节点约 3 倍），构建 O(n log n) 一次性完成，查询无指针跳转。若基准显示不足再换 `fst`（第 13 节）。
- 内联 `[Ruleset]` 与顶层 `[Rule]` 不建索引：顶层规则数通常 < 2,000，逐条 O(1) / O(log n) 判定即可，且保持「自上而下首个命中」语义无需合并索引（与阶段 1 设计 6.2「跨规则加速结构」的差异见第 12 节）。

**IP 索引 `IpIndex`**：`prefix_trie::PrefixMap<Ipv4Net, u32>` 与 `PrefixMap<Ipv6Net, u32>`，值为条目下标；查询取最长前缀匹配（集合内任一命中即命中，取最长前缀用于日志）。SRC-IP 规则同样用它（顶层单条规则直接比较）。

**ASN**：`maxminddb` 查询 O(1)，每次 `evaluate` 内缓存 country 与 asn 结果。

### 6.2 规则集

```rust
pub struct CompiledSet {
    pub name: String,                     // 显示名：SYSTEM / LAN / 内联名 / 文件名 / URL
    pub domains: DomainIndex,
    pub ips: IpIndex,
    pub linear: Vec<CompiledSubRule>,     // 关键词、通配、正则、端口、协议、逻辑、嵌套集等
    pub entries: Vec<Box<str>>,           // 每条原文，供 Sub-rule matched 日志
    pub needs_dns: bool,                  // 含 IP 类条目（不含 no-resolve 行）
    pub version: u64,
}
pub struct SetHandle(Arc<ArcSwap<CompiledSet>>);
pub struct SetRegistry { /* ResourceRef → SetHandle；后台任务在资源变化时重编译并 swap */ }
impl SetRegistry {
    pub fn build(cfg: &Config, resources: &Arc<ResourceManager>, base_dir: &Path) -> (Arc<SetRegistry>, Diagnostics);
    pub fn get(&self, r: &ResourceRef, kind: SetKind /* RuleSet | DomainSet */) -> SetHandle;
    pub fn statuses(&self) -> Vec<SetStatus>;
}
```

- **内部集**：`SYSTEM` 与 `LAN` 的内容按手册 `rules/ruleset.html`「Internal Rule Sets」列表以常量内置（20 条与 14 条），随手册版本更新。
- **文件格式（RULE-SET）**：整行注释 `#` `//` `;`；空行忽略；每行经 `rurge_config::rule::parse_subrule` 解析；行级参数 `no-resolve` `extended-matching` 允许；`FINAL` 与 `pre-matching` 出现即为非法行；非法行跳过并告警一次（汇总数量）。
- **文件格式（DOMAIN-SET）**：`.example.com` = 后缀（含自身），`example.com` = 精确；注释 `#` `//`；含 `*`、空白或非法字符的行跳过并告警。
- **嵌套**：集合文件与内联集中的 `RULE-SET` / `DOMAIN-SET` 行解析为 `SubRuleKind::Set(SetHandle)`；`SetRegistry::build` 用 DFS 检测循环并限制深度 8，超出的引用行按不匹配处理并告警。手册只对内联集明确允许嵌套，对外部文件未说明；rurge 一并支持并登记为 🟡。
- **上限**：超过 1,000,000 条时丢弃其余并告警（手册说「at most」，rurge 选择截断而非拒绝整集，登记 🟡）。
- **同一资源不可同时作两种集**：`RULE-SET,x` 与 `DOMAIN-SET,x` 引用同一 source 时，第二种引用报错（M1 已有校验的沿用运行时兜底）。
- **行级参数作用于整集**：`no-resolve` 使集合 `needs_dns = false` 且所有 IP 类条目只在目标已是 IP 时匹配；`extended-matching` 使集合内域名条目也匹配 SNI / Host。
- **热更新**：`ResourceHandle::subscribe` 变化 → 后台任务重编译 → `ArcSwap::store`；引擎无需重建；编译失败保留旧集并告警。
- **`Missing` 资源**：视为空集；首次评估告警一次；资源到达后自动生效。

### 6.3 匹配器

`CompiledRule { index: usize, matcher: Matcher, policy: PolicyRef, params: RuleParams, raw: String, hits: AtomicU64, warned: AtomicBool }`。`Matcher::eval(&self, s: &SessionInfo, ctx: &mut EvalCtx) -> Verdict`，`Verdict = Match | NoMatch | NeedsResolve`。`EvalCtx` 持有本次评估的已解析地址、GeoIP / ASN 缓存与跟踪缓冲。

| 规则 | 判定 |
| --- | --- |
| DOMAIN / DOMAIN-SUFFIX / DOMAIN-KEYWORD / DOMAIN-WILDCARD | 对 `dst_host` 的域名形式（IP 目标 → NoMatch）；`extended-matching` 时再对 `sni`、`http_host` 各试一次。大小写不敏感 |
| DOMAIN-SET / RULE-SET | 先查域名索引与不需要 DNS 的线性条目（不触发 DNS）；无命中且 `needs_dns` 且目标为域名且无 `no-resolve` → NeedsResolve；有地址后查 IP 索引 |
| IP-CIDR / IP-CIDR6 / GEOIP / IP-ASN | 目标为 IP 直接判定；目标为域名且本次评估尚未解析：`no-resolve` → NoMatch（跳过），否则 NeedsResolve；若地址已由更早的规则解析出来，`no-resolve` 规则照常按该地址判定（手册：只跳过「尚未解析」的域名目标）。记录选取：IP-CIDR 取首个 IPv4；IP-CIDR6 取首个 IPv6；GEOIP / IP-ASN 取首个 IPv4，无则首个 IPv6。GEOIP 国家码不区分大小写；库缺失 → NoMatch + 告警一次 |
| URL-REGEX | 对 `url` 匹配；`extended-matching` 时把 URL 主机部分替换为 SNI / Host 再各匹配一次；`url` 为 None → NoMatch |
| USER-AGENT | glob（大小写敏感）对 `user_agent`；None → NoMatch |
| DEST-PORT / SRC-PORT / IN-PORT | `PortExpr::matches` |
| SRC-IP | `IpNet::contains(src.ip())` |
| PROTOCOL | `protocol == Some(kind)` |
| HOSTNAME-TYPE | `session.hostname_type()` |
| PROCESS-NAME | 三种模式对 `process`：名称 glob 对文件名；路径 glob 对全路径；前缀对全路径。Windows 路径统一为反斜杠、大小写不敏感；None → NoMatch |
| AND / OR / NOT | 递归；AND 遇 NoMatch 短路，OR 遇 Match 短路；子规则 NeedsResolve 向上传递（先解析再重评） |
| SUBNET | NoMatch，首次评估告警一次（阶段 1 无网络环境信息） |
| CELLULAR-RADIO / CELLULAR-CARRIER | 永不匹配（无告警，手册语义） |
| SCRIPT / DEVICE-NAME / MAC-ADDRESS | NoMatch，首次评估告警一次 |
| FINAL | 见 6.5 |

### 6.4 GeoIP / ASN

```rust
pub struct GeoDb { country: ArcSwap<Option<Reader<Vec<u8>>>>, asn: ArcSwap<Option<Reader<Vec<u8>>>> }
impl GeoDb {
    pub fn open(dir: &Path) -> (Arc<GeoDb>, Vec<Diagnostic>);   // 缺文件不报错，返回 Info
    pub fn country(&self, ip: IpAddr) -> Option<CountryCode /* [u8; 2] 大写 */>;
    pub fn asn(&self, ip: IpAddr) -> Option<u32>;
    pub fn info(&self) -> GeoDbInfo { country_epoch: Option<u64>, asn_epoch: Option<u64>, country_path, asn_path }
}
pub struct GeoUpdater;   // GeoUpdater::spawn(geo, resources, GeoUrls { country, asn }, auto_update: bool)
```

- 目录 `<data>/geoip/`；文件 `GeoLite2-Country.mmdb`、`GeoLite2-ASN.mmdb`。
- Country 来源：`geoip-maxmind-url`（配置），缺省为构建常量 `https://github.com/P3TERX/GeoLite.mmdb/releases/latest/download/GeoLite2-Country.mmdb`；ASN 来源：rurge 专有运行时选项 `--geoip-asn-url` / `RURGE_GEOIP_ASN_URL`，缺省 `.../GeoLite2-ASN.mmdb`。两者都可用 `--geoip-url` / `RURGE_GEOIP_URL` 覆盖（命令行优先于配置文件；FR-CFG-17）。
- 接受 `.mmdb` 或 `.tar.gz`（解包时取第一个以 `.mmdb` 结尾的成员）；下载后先用 `maxminddb` 打开校验（数据库类型须含 `Country` / `ASN`），再写 `.tmp` 并 `rename`，随后 swap。
- 更新周期 7 天（手册未给数值）；`disable-geoip-db-auto-update = true` 时只在文件缺失时抓取一次；`force_update` 可手动触发（M4 API）。
- 许可：GeoLite2 数据受 MaxMind EULA 约束，rurge 不内置数据库，README 与首次下载日志注明归属（「This product includes GeoLite2 data created by MaxMind」）。
- 无库时 GEOIP / IP-ASN 规则 NoMatch，首次评估告警一次；库到达后热加载。

### 6.5 规则引擎

```rust
pub trait LazyResolver: Send + Sync {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<ResolvedAddrs, ResolveError>>;
}
pub struct ResolvedAddrs { pub v4: Vec<Ipv4Addr>, pub v6: Vec<Ipv6Addr> }
pub enum OutboundMode { Direct, Proxy(PolicyRef), Rule }

pub struct RuleEngine { rules: Vec<CompiledRule>, final_pos: usize, geo: Arc<dyn GeoLookup>, pre: PreMatchingSet, registry: Option<Arc<SetRegistry>> }
impl RuleEngine {
    pub fn build(cfg: &Config, sets: &dyn SetLookup, geo: Arc<dyn GeoLookup>) -> Result<RuleEngine, BuildError>;
    // 持有 registry 存活直到引擎被丢弃，重载不因调用方忘记单独保留 Arc 而在 60 s 内停止（F5）：
    pub fn build_with_registry(cfg: &Config, registry: Arc<SetRegistry>, geo: Arc<dyn GeoLookup>) -> Result<RuleEngine, BuildError>;
    pub async fn evaluate(&self, s: &SessionInfo, mode: OutboundMode, r: &dyn LazyResolver) -> Decision;
    pub async fn evaluate_traced(&self, s: &SessionInfo, mode: OutboundMode, r: &dyn LazyResolver) -> (Decision, Vec<TraceStep>);
    pub fn pre_matching(&self) -> &PreMatchingSet;
    pub fn pre_match_domain(&self, host: &str, port: u16) -> Option<PreMatch>;
    pub fn pre_match_ip(&self, ip: IpAddr, port: u16) -> Option<PreMatch>;
    pub fn rules(&self) -> &[CompiledRule];
    pub fn registry(&self) -> Option<&Arc<SetRegistry>>;
}
pub struct Decision {
    pub outcome: Outcome,                 // Policy(PolicyRef) | DnsFailed
    pub reason: Reason,                   // OutboundModeDirect | OutboundModeProxy | Rule | Final | DnsFailedFallback | DnsFailed
    pub matched: Option<usize>,           // 命中规则下标（Final 时为 FINAL 下标）
    pub sub_rule: Option<SubRuleHit>,     // { set_name, entry }，FR-RULE-15
    pub resolved: Option<ResolvedAddrs>,  // 评估中解析过则带回，M3 复用避免二次解析
    pub notes: Vec<String>,               // 运行时告警（如资源缺失）
}
pub struct TraceStep { pub rule: usize, pub verdict: &'static str, pub elapsed: Duration }
```

评估算法：

1. `mode == Direct` → `Policy(DIRECT)`；`mode == Proxy(p)` → `Policy(p)`；均不触发 DNS，不计数。
2. 按下标遍历 `rules[..=final_index]`（最后一条 FINAL 之后的规则不参与）。对每条：
   - `Match` → 计数、返回 `Policy(rule.policy)`，`reason = Rule`。
   - `NoMatch` → 下一条。
   - `NeedsResolve` → 若本次尚未解析，调用 `LazyResolver::resolve(dst_host)` 一次；成功则存入 `EvalCtx` 并重评本条（所需地址族为空时该条按 NoMatch 处理）；失败（含 `EmptyAnswer`，见 Q1）则：FINAL 带 `dns-failed` → `Policy(final.policy)`，`reason = DnsFailedFallback`；否则 `Outcome::DnsFailed`（M3 关闭连接并记录）。
3. 走到 FINAL → `Policy(final.policy)`，`reason = Final`。
4. 多条 FINAL 取最后一条（4.2）；其前的其他 FINAL 在构建时被移除并计入诊断。

`PreMatchingSet`：由带 `pre-matching` 的规则（M1 已校验其策略为 REJECT 族）构建，暴露 `match_domain(host: &str, port: u16) -> Option<RejectKind>` 与 `match_ip(ip: IpAddr, port: u16) -> Option<RejectKind>`，只使用不需要 DNS 的判定（域名规则对域名、IP 规则对 IP、端口、逻辑组合、集合的相应部分）。M3 在 DNS 应答与 SYN 阶段调用。

命中计数：`CompiledRule.hits`（`AtomicU64`）供阶段 6 的 `rule-usage`；`Decision.notes` 中的告警由 M3 写日志，引擎本身只用 `tracing::warn!` 记录一次性告警。

## 7. `rurge-dns`

### 7.1 接口

```rust
pub struct Resolver { /* upstreams, bootstrap, cache, hosts, sets, system, opts */ }
pub struct ResolverConfig {
    pub servers: Vec<DnsServer>, pub encrypted: Vec<EncryptedDns>, pub skip_cert_verification: bool,
    pub ipv6: bool, pub hosts: Vec<HostEntry>, pub read_etc_hosts: bool, pub proxy_hostnames: HashSet<String>,
    pub cache_capacity: usize /* 默认 2000，--dns-cache-size 覆盖 */,
}
impl ResolverConfig { pub fn from_config(cfg: &Config) -> ResolverConfig; }
pub struct ResolverDeps { pub connector: Arc<dyn Connector>, pub http: Arc<HttpClient>, pub sets: Arc<SetRegistry>, pub system: Arc<dyn SystemDns>, pub resources: Arc<ResourceManager> }
pub trait SystemDns: Send + Sync {
    fn servers(&self) -> Vec<SocketAddr>;      // 端口 53
    fn search_domains(&self) -> Vec<String>;
    fn hosts_path(&self) -> Option<PathBuf>;
    fn has_ipv6(&self) -> bool;
}
impl Resolver {
    pub fn new(cfg: ResolverConfig, deps: ResolverDeps) -> (Arc<Resolver>, Diagnostics);
    pub async fn lookup(&self, host: &str, opts: LookupOpts) -> Result<DnsResult, DnsError>;
    pub fn flush(&self);
    pub fn on_network_change(&self);           // flush + 重置 AAAA 抑制 + 重建 UDP socket + 重读系统 DNS
    pub fn cache_snapshot(&self) -> Vec<CacheEntry>;
    pub async fn measure_delay(&self) -> Vec<UpstreamDelay>;   // 对每个上游解析固定域名一次，记录耗时或错误
}
pub struct LookupOpts { pub bypass_cache: bool, pub want_v6: Option<bool> /* None = 按配置 */ }
pub struct DnsResult { pub v4: Vec<Ipv4Addr>, pub v6: Vec<Ipv6Addr>, pub ttl: Duration, pub source: Source, pub elapsed: Duration }
pub enum Source { Literal, Host(HostKind /* Ip | Alias | Server | System | EtcHosts */), Cache { stale: bool }, Upstream(String), System }
pub enum DnsError { Timeout, EmptyAnswer, AllFailed(Vec<(String, String)>), Bootstrap(String), NoUpstream, AliasLoop(String), Unsupported(String) }
pub struct CacheEntry { pub name: String, pub v4: Vec<Ipv4Addr>, pub v6: Vec<Ipv6Addr>, pub expires_in: Option<Duration>, pub stale: bool, pub source: String }
```

`Resolver` 实现 `rurge_rules::LazyResolver`（`resolve` = `lookup` 取地址）与 `rurge_net::Resolve`（供 `DirectConnector` 使用，避免 tokio 系统解析）。

### 7.2 上游

```rust
trait Upstream: Send + Sync {
    fn name(&self) -> &str;                  // 显示用，如 "udp://1.1.1.1:53"、"https://dns.example/dns-query"
    async fn query(&self, msg: &Message, deadline: Instant) -> Result<Message, UpstreamError>;
}
```

| 类型 | 实现 |
| --- | --- |
| UDP（`dns-server` 中的 IP[:port]） | 每个上游一个 socket（IPv4 / IPv6 按地址族），请求 ID 随机；应答校验 ID 与问题段；超过 512 字节被截断（TC）时改用 TCP 重发一次 |
| `system` | 启动与 `on_network_change` 时通过 `SystemDns::servers()` 生成一组 UDP 上游；为空则退化为 `tokio::net::lookup_host`（`Source::System`） |
| `tcp://host[:port]`（`dns-server` 或 `encrypted-dns-server`） | 经 `Connector` 建立持久连接，长度前缀帧，多路复用按 ID 配对；断开后下次查询重连；主机名用引导解析 |
| `tls://host[:port]`（DoT，默认 853） | 同 TCP，外加 rustls；SNI = URL 主机名；`skip_cert_verification` 时用不校验的 verifier 并在启动时告警 |
| `https://.../dns-query`（DoH，默认 443） | `HttpClient::stream` 发 `POST`，`Content-Type: application/dns-message`，`Accept` 同；HTTP/2 由 ALPN 决定；非 200 或非 dns-message 响应视为失败 |
| `h3://` `quic://` | 阶段 2；M2 解析后 `Unsupported` 告警并忽略该条 |

**引导（bootstrap）**：URL 形式上游的主机名只由「传统上游」解析：`dns-server` 中的 UDP 服务器，没有则系统 DNS。引导结果单独缓存（TTL 按应答，最短 60 s）。`[Host]` 的 `server:` 加密 URL 同样豁免。

**上游集合选择**：

- 只有 `dns-server` 的 UDP 项 → 普通查询用全部 UDP 上游。
- 配置了任意 `tcp://` / `encrypted-dns-server` → 普通查询只用这些「加密子系统」上游；UDP 项仅作引导与连通性测试（手册 `dns/encrypted-dns.html`）。
- 都没有 → 系统 DNS。
- `ipv6 = false` 时丢弃 IPv6 地址的服务器（M1 已在解析阶段标记，M2 执行）。

### 7.3 并发查询与重试（手册 `dns/overview.html`）

```
attempt = 0
loop:
    向全部选定上游并发发送（A，且 ipv6 && has_ipv6 时同时发 AAAA）
    等待 1 s：
        收到 Valid（NOERROR 且含目标记录）→ 立即返回（AAAA 并行时等两者，见下）
        全部上游都返回 Empty（NOERROR/NXDOMAIN 无记录）→ EmptyAnswer
    attempt += 1；attempt == 5 → 失败：若有上游返回过 Empty 且其余超时 → EmptyAnswer，否则 Timeout/AllFailed
```

- **A / AAAA 并行**：等待两者都到达；若重发定时器触发时只有一种到达，以部分结果完成查询；另一种迟到的应答只更新缓存。
- **AAAA 抑制**：连续 5 次「A 有应答而 AAAA 超时」→ 停发 AAAA，`tracing::warn!` 一次；`flush` 或 `on_network_change` 恢复。
- **首个有效应答获胜**：不同上游的不一致应答不做合并。
- 单个 `Question` 在同一时间只有一个在途查询（in-flight 合并，`HashMap<name, Shared future>`）。

### 7.4 缓存

- `lru::LruCache<String, Entry>`，容量默认 2000；键为小写域名；值含 v4 / v6、`expires_at`、`negative: bool`。
- TTL 取应答记录集合中的最小 TTL；TTL 为 0 不缓存。
- **乐观缓存**：过期条目立即返回（`Source::Cache { stale: true }`）并后台刷新一次；刷新失败保留旧值再等 60 s 才允许下一次刷新。
- **负缓存**：`EmptyAnswer` 缓存 30 s（手册未说明；登记 🟡）。错误不缓存。
- `flush` 清空并重置 AAAA 抑制；`cache_snapshot` 供 `dns lookup --cache` 与 M4 的 `GET /v1/dns`。

### 7.5 `[Host]` 映射链与 hosts 文件

`HostMap::build(entries, sets, system) -> HostMap`；`lookup(name) -> Option<&HostEntryCompiled>`：按配置顺序首个命中；键为精确名、glob（`*`、`?`，手册示例：`*google.com` 匹配 `google.com`、`foo.google.com`、`bargoogle.com`；`*.google.com` 不匹配 `google.com`）、`DOMAIN-SET:`/`RULE-SET:` 集合（只用集合的域名索引与域名类线性条目）。

解析流程（`Resolver::lookup`）：

1. IP 字面量 → `Literal`，不查询。
2. 小写；去尾点并记「禁用搜索域」。
3. `localhost` 与 `*.localhost` → 回环地址（RFC 6761；手册未说明，登记 🟡）。
4. 若 `name ∈ proxy_hostnames` → 跳过第 5 步（手册：代理服务器主机名永不匹配 `[Host]`）。
5. `[Host]` 链：
   - `Ips` → 返回，`ttl = 0`（不缓存，每次直接命中），`Source::Host(Ip)`。
   - `Alias(target)` → 以 `target` 重新从第 1 步开始，最多 8 跳，超出 `AliasLoop`。
   - `Servers(list)` → 用该列表构建（并缓存）的专属上游集合执行 7.3；`system` / `syslib` / `force-syslib` → 系统解析库（`tokio::net::lookup_host`）。
   - `Script` → 构建时告警一次，视为未命中继续。
6. hosts 文件条目（`read_etc_hosts = true` 且文件存在）：解析 `/etc/hosts` 格式（IP 后跟多个名字，`#` 注释），追加在 `[Host]` 之后；文件用 `ResourceManager` 的本地文件监视热更新。
7. `.local` 结尾 → 系统解析库。
8. 单标签名 → 追加系统首个搜索域后交系统 DNS 上游（无搜索域时直接交系统 DNS）。
9. 缓存查找 → 7.4。
10. 上游查询 → 7.2 / 7.3，写缓存。

### 7.6 `dns lookup` 输出

见 10.2。

### 7.7 与 M3 的接缝

- 上游连接全部经 `ResolverDeps.connector`；M2 传 `DirectConnector`。M3 传「走流水线的连接器」并把 `encrypted-dns-follow-outbound-mode` 映射为「是否用流水线连接器」，规则可见 `PROTOCOL,DOH` 等会话；M3 负责防环（DNS 会话自身不再触发解析）。
- fake-IP、`hijack-dns`、`always-real-ip` 在 M3 的应答器中实现，复用 `Resolver::lookup`。
- `on_network_change` 由 M3 / M4 的网络监视调用。

## 8. `rurge-platform`（M2 最小集）

```rust
pub mod dirs {
    pub fn data_dir() -> PathBuf;     // Windows %LOCALAPPDATA%\rurge；Linux $XDG_DATA_HOME/rurge（缺省 ~/.local/share/rurge）；macOS ~/Library/Application Support/rurge
    pub fn config_dir() -> PathBuf;   // Windows %APPDATA%\rurge；Linux $XDG_CONFIG_HOME/rurge；macOS ~/Library/Application Support/rurge/profiles
}
pub mod dns {                          // M2b
    pub fn servers() -> Vec<SocketAddr>;     // Windows：ipconfig crate 读各适配器 DNS；Unix：/etc/resolv.conf 的 nameserver
    pub fn search_domains() -> Vec<String>;  // Windows：适配器 DNS 后缀；Unix：search / domain
    pub fn hosts_path() -> PathBuf;          // Windows %SystemRoot%\System32\drivers\etc\hosts；Unix /etc/hosts
    pub fn has_ipv6() -> bool;               // 任一接口有非链路本地、非回环的全局 IPv6 地址
}
```

服务模式的目录（`/var/lib/rurge`、`/etc/rurge`）在 M4 服务安装时决定。macOS 用 `/etc/resolv.conf`（由系统同步，对主解析器足够）；M4 若需按接口区分再换 SystemConfiguration。全部函数只做读取、不会失败（错误时返回空值并 `tracing::debug!`）。

## 9. 数据目录布局

```
<data>/                      # --data-dir / RURGE_DATA_DIR，缺省 rurge-platform::dirs::data_dir()
├── resources/<sha256>/      # 外部资源缓存：data + meta.json
├── geoip/                   # GeoLite2-Country.mmdb、GeoLite2-ASN.mmdb；两个库都经 ResourceManager 下载，来源 URL 与抓取时间记录在对应的 resources/<sha256>/meta.json 里，geoip/ 目录本身不写 meta.json
└── (M4) state.json, logs/
```

目录不存在时创建；无法创建（只读文件系统）时资源管理器退化为纯内存（每次启动重新抓取）并告警。

## 10. CLI 开发命令（`rurge` bin）

两个命令都是离线的：在当前进程内构建对象，不需要守护进程。运行时选项（FR-CFG-17）：`--data-dir`、`--geoip-url`、`--geoip-asn-url`、`--dns-cache-size`、`--no-network`（只用缓存，不抓取），对应环境变量 `RURGE_DATA_DIR`、`RURGE_GEOIP_URL`、`RURGE_GEOIP_ASN_URL`、`RURGE_DNS_CACHE_SIZE`、`RURGE_NO_NETWORK`。

### 10.1 `rurge rule match`

```
rurge rule match -c <conf> <host[:port]> [--url <url>] [--src <ip:port>] [--in-port <n>] [--protocol <kind>]
                 [--sni <host>] [--http-host <host>] [--user-agent <ua>] [--process <name-or-path>]
                 [--udp] [--listener http|socks5|tun] [--mode direct|proxy=<policy>|rule]
                 [--resolve <ip,...> | --no-dns] [--wait <secs>] [--explain] [--json]
```

- 构建 `Config` → `ResourceManager`（`--wait` 默认 30 s 等待首轮抓取，`--no-network` 时不等待）→ `SetRegistry` → `GeoDb` → `RuleEngine`。
- 解析器：M2a 用 `SystemResolve`（tokio）；M2b 完成后换成 `rurge-dns::Resolver`（同一命令，无需改参数）。`--resolve` 用给定地址代替解析，`--no-dns` 让解析返回错误（验证 `dns-failed` 路径）。
- 输出（文本）：`policy`、`reason`、`rule #<n>: <raw>`、`sub-rule: <entry> (in <set>)`、`resolved: ...`、`notes`；`--explain` 追加逐条 `#<n> <verdict> <raw>`。`--json` 输出同结构对象。
- 退出码：0 有决策；1 `DnsFailed`；2 配置或加载错误。

### 10.2 `rurge dns lookup`

```
rurge dns lookup -c <conf> <name> [--type a|aaaa|both] [--server <spec>...] [--no-cache] [--trace] [--json]
rurge dns cache -c <conf>            # 打印本进程内（仅演示）缓存快照，M4 后改为查询守护进程
```

- `--server` 覆盖配置的上游（同 `dns-server` / `encrypted-dns-server` 语法）。
- 输出：地址列表、`source`、`ttl`、`elapsed`、应答的上游名；`--trace` 打印每次尝试（上游、发送时间、结果）。
- 退出码：0 成功；1 `EmptyAnswer`；2 其他错误。

## 11. 错误处理与日志

| 情形 | 处理 |
| --- | --- |
| 外部资源无法抓取且无缓存 | 空集 + `Decision.notes` + 首次评估 `warn!`；退避重试 |
| 资源内容编译失败（全部行非法） | 保留旧集（或空集）+ `warn!`，状态在 `SetRegistry::statuses` 可见 |
| GeoIP 库缺失 / 损坏 | 规则 NoMatch + 一次性 `warn!`；损坏文件重命名为 `.bad` 并重新下载 |
| DNS 全部上游失败 | `DnsError::AllFailed`（含每个上游的错误）；规则层按 `dns-failed` 处理 |
| 引导失败 | `DnsError::Bootstrap`；该加密上游本轮跳过，其余上游继续 |
| TLS 证书校验失败 | 上游错误；`skip_cert_verification` 时忽略并在启动时 `warn!` 一次 |
| 配置引用不支持的上游（`h3://`） | 构建诊断 W 级 + 忽略该条 |
| panic 隔离 | 资源与更新任务在独立 tokio 任务中；panic 只终止该任务并 `error!`，下次刷新由调度器重新派生 |

日志键：`resource.url`、`set.name`、`rule.index`、`dns.name`、`dns.upstream`，与 AR-05 的事件关联留待 M3 的请求记录。

## 12. 与阶段 1 设计的差异

| 项 | 阶段 1 设计 | 本文 | 原因 |
| --- | --- | --- | --- |
| 依赖方向 | `rurge-dns` 与 `rurge-rules` 并列 | `rurge-dns → rurge-rules` | `[Host]` 集合键与 `LazyResolver` 位置 |
| 域名索引 | 后缀 trie，跨规则合并索引 | 每个集合一份排序数组；顶层规则线性 | 内存与实现复杂度；顶层规则数小 |
| 评估签名 | `evaluate(session, resolver)` | 增加 `mode: OutboundMode` 参数 | 引擎不持有运行时状态（AR-04） |
| `SessionInfo.protocol` | `ProtocolHint` 枚举 | `Option<ProtocolKind>` | 复用 M1 类型 |
| DNS 连接 | 经 `rurge-engine` 流水线 | 经注入的 `Connector`，M2 为 DIRECT | M2 尚无引擎；接口留给 M3 |
| GeoIP 默认源 | 待定（Q1） | P3TERX/GeoLite.mmdb 发布件 | 项目所有者 2026-09-04 决定 |

阶段 1 设计文档保持不变，本文为实现依据；M3 设计时如再调整以最新文档为准。

## 13. 测试策略

| 层 | 内容 |
| --- | --- |
| 单元 | 每种匹配器；`DomainIndex` / `IpIndex` 构建与查询边界（空集、单条、根域、IPv6 映射地址）；RULE-SET / DOMAIN-SET 文件解析（注释、非法行、上限截断）；`HostMap` 通配示例；hosts 文件解析；DNS 报文构造与校验；缓存 TTL / 乐观刷新 / 负缓存 |
| 属性（proptest） | 随机域名集合与查询：索引结果 == 朴素逐条匹配；随机 CIDR 集合同理；随机 `[Rule]` 序列：引擎决策 == 参考实现（无索引、无短路） |
| 黄金 | 手册中每个规则示例与 `[Host]` 示例作为用例；`tests/corpus/rules/*.toml`（会话属性 → 期望决策）由 `rurge rule match --json` 驱动 |
| 集成（M2a） | 本地 hyper 服务器提供规则集文件（ETag / 304 / 500 / 超时），验证缓存、退避、热更新；临时目录中的本地文件修改触发重编译；GeoIP 用 MaxMind 公开测试库（`GeoIP2-Country-Test.mmdb`、`GeoLite2-ASN-Test.mmdb`，随仓库附许可说明） |
| 集成（M2b） | `tests/support/mock_dns.rs`：本地 UDP + TCP 模拟上游（可编程延迟 / 丢包 / 空应答 / SERVFAIL / 截断）；DoT 用 `tokio-rustls` + `rcgen` 自签证书；DoH 用 hyper 服务器；覆盖 1 s 重发、5 次失败、首个应答获胜、空应答语义、A / AAAA 并行与部分结果、AAAA 抑制与恢复、引导豁免、`[Host]` 链、别名防环 |
| CLI | `assert_cmd`：`rule match` 与 `dns lookup` 的文本与 JSON 输出、退出码；全部用本地资源与 `--no-network` / 本地模拟上游，不访问外网 |
| 基准 | criterion：10 万与 100 万条域名集查询、10 万条 CIDR 查询、完整 `evaluate`（1,000 条顶层规则）、DNS 缓存命中；CI 独立 job 只运行不比较 |

测试不访问公网；默认镜像 URL 的可达性由手工验证与 README 说明覆盖。

## 14. 验收标准

M2a：

1. `cargo test --workspace` 全绿，clippy 零警告，三平台 CI 绿。
2. 语料库配置全部能构建 `RuleEngine`；黄金用例全部通过；属性测试 1,000 轮无反例。
3. 基准：100,000 条域名集单次查询 p99 < 5 µs，`evaluate`（1,000 条顶层规则 + 3 个 10 万条集）p99 < 50 µs（本机数字写入计划的执行记录）。**实测**（本机 Windows，1,000 条顶层 DOMAIN-SUFFIX 规则 + 3 × 10 万条集，全部 miss 落到 FINAL）：`evaluate` 中位数 75.5 µs，超过本条 50 µs 目标；根因是顶层规则按 6.1 的既定取舍（D4）线性逐条比较、不建跨规则索引，miss 场景需扫完全部 1,000 条才落到 FINAL。D4 保持「顶层不建索引」的决定不变，把「必要时给顶层规则加一个索引」列为后续里程碑的待办，本阶段不优化。
4. `rurge rule match` 对 URL 规则集在无网络时使用缓存，有网络时首次抓取并落盘；修改本地规则集文件后 1 s 内再次匹配得到新结果。
5. GeoIP：测试库查询正确；tar.gz 与 mmdb 两种来源都能落盘并热加载。

M2b：

6. 模拟上游集成测试覆盖 7.3 与 7.4 的全部行为。
7. `rurge dns lookup` 经 UDP / TCP / DoT / DoH 模拟上游均能解析，`--trace` 输出显示重发与获胜上游。
8. `[Host]` 全部值形式（`script:` 除外）与 hosts 文件用例通过；修改 hosts 文件后自动生效。
9. `rurge rule match` 切换到 `rurge-dns::Resolver` 后黄金用例仍全部通过。

## 15. 兼容性清单需登记的差异（实施时更新 `docs/surge-compatibility-matrix.md`）

| 项 | 状态 | 说明 |
| --- | --- | --- |
| 外部规则集文件中的 `RULE-SET` / `DOMAIN-SET` 嵌套 | 🟡 | 手册只对内联集说明；rurge 一并支持，深度 8 |
| 集合超过 1,000,000 条 | 🟡 | rurge 截断并告警，Surge 行为未说明 |
| 大集合的磁盘数据库 | 🟡 | rurge 全内存（排序数组） |
| `geoip-maxmind-url` 默认值 | 🟡 | 指向社区镜像而非 nssurge.com |
| ASN 库来源 | 🟡 | Surge 随应用更新；rurge 可配置并自动更新 |
| GeoIP 更新周期 | 🟡 | rurge 7 天，Surge 未说明 |
| 条件请求（ETag） | 🟡 | rurge 内部优化 |
| `read-etc-hosts` | 🟡 | 手册标注 Mac only；rurge 三平台生效 |
| `localhost` / `*.localhost` | 🟡 | rurge 直接回环，手册未说明 |
| 负缓存 30 s | 🟡 | 手册未说明 |
| `server:force-syslib` | 🟡 | M2 等同 `syslib`，M3 区分 |
| `[Host]` `script:` | ⛔（阶段 5） | 构建告警并跳过 |
| `h3://` `quic://` | ⛔（阶段 2） | 告警并忽略 |
| `SUBNET` `SCRIPT` `DEVICE-NAME` `MAC-ADDRESS` | ⛔（后续阶段） | 不匹配并告警一次 |

## 16. 已决事项与开放问题

| 编号 | 事项 | 决定 |
| --- | --- | --- |
| D1 | GeoIP / ASN 默认来源 | 社区镜像 P3TERX/GeoLite.mmdb，可覆盖，不内置数据 |
| D2 | M2 拆分 | 一份设计、两份计划（M2a 规则、M2b DNS） |
| D3 | 域名索引结构 | 排序反转键数组，基准不达标再换 `fst` |
| D4 | 顶层规则是否建索引 | 不建，线性评估 |
| D5 | DNS 报文库 | `hickory-proto` 仅编解码，自写并发 / 重试 |
| D6 | HTTP 客户端 | hyper + 自定义连接器，不用 reqwest |
| D7 | 依赖方向 | `rurge-dns → rurge-rules` |
| D8 | 系统 DNS 发现 | `ipconfig` / `resolv-conf` / `if-addrs`，只在 `rurge-platform` |
| D9 | `ResourceManager` 条目退休 | 不提供单条目退休接口；一个配置代数一个 `ResourceManager`，重载时整体新建并丢弃旧实例（5.3） |
| D10 | `RuleEngine` 与 `SetRegistry` 的生命周期 | `build` 不持有 registry（调用方须自行保活）；新增 `build_with_registry` 持有 `Arc<SetRegistry>` 供常规调用方使用（6.5，F5） |
| Q1 | 空应答是否也触发 `dns-failed` | 暂按「是」（`EmptyAnswer` 视为解析失败）；M3 联调后复核 |
| Q2 | 内联集与外部集共享条目上限时的截断优先级 | 暂按文件顺序截断 |
| Q3 | 排序数组在 100 万条时的实际内存 | M2a 基准后回填本文 |
