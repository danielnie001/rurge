# 阶段 1 / M3「连接流水线」设计

> 状态：已与项目所有者逐节确认（2026-09-05）。本文件是 M3a / M3b 两份实施计划的依据；与阶段 1 设计文档（`2026-09-03-phase1-core-skeleton-design.md` 第 8、11 ～ 13 节）不一致处以本文件为准，并在第 11 节登记。

## 1. 目标与范围

### 1.1 阶段目标

- `rurge run -c <conf>` 作为前台守护进程，按 Surge 配置提供 HTTP / SOCKS5 代理，把每个连接经规则引擎分流到 `DIRECT` 或 `REJECT` 系策略。
- 建立后续阶段的插入点：阶段 2 的代理协议只需实现 `Outbound`；阶段 4 的 HTTP 引擎只需替换 `HttpListener` 中「dial → 转发」这一段；M4 的 API 只读写 `Engine` 暴露的记录、统计与状态。

### 1.2 范围内（PRD 编号）

| 编号 | 内容 | 子里程碑 |
| --- | --- | --- |
| FR-IN-01 | `http-listen` 多监听器、`password@` Basic 认证、CONNECT 与明文转发、默认端口 6152 | M3a |
| FR-IN-02 | `socks5-listen`、无认证、CONNECT；UDP ASSOCIATE 回「不支持」 | M3a |
| FR-IN-03 | `proxy-restricted-to-lan`；默认监听 `127.0.0.1` | M3a |
| FR-IN-05 | 元数据采集：`IN-PORT`、`SRC-IP`、`SRC-PORT`、`HOSTNAME-TYPE`；协议识别（SNI） | M3a（SNI：M3b） |
| FR-OUT-01 | `DIRECT`；`REJECT` 系四种的连接层与 HTTP 响应；自动升级阈值 | M3a（升级：M3b） |
| FR-OUT-02 | 别名类型 `direct` / `reject*` | M3a |
| FR-HTTP-10（基础） | 请求记录：环形缓冲、活动索引、字段、终止 | M3b |
| FR-HTTP-12 | `show-error-page`、`show-error-page-for-reject`、1px GIF | M3a（连接失败的错误页：M3b） |
| FR-OBS-01 | 日志级别映射、stdout、结构化字段、凭据不落日志；滚动文件 | M3a（文件：M3b） |
| FR-OBS-04（部分） | `rurge run` | M3a |
| FR-OBS-05（基础） | 按策略 / 监听器的流量计数与实时速率 | M3b |
| FR-OBS-10 | 日志与请求记录以会话 id 关联 | M3a 日志字段，M3b 记录 |
| FR-DNS-04（部分） | `encrypted-dns-follow-outbound-mode`：DNS 连接走流水线、`PROTOCOL,DOH/DOT/DNS` 可匹配、防环回退 | M3b |
| AR-03 / AR-04 | 每连接一个任务；配置不可变、重载原子切换 | M3a / M3b |

### 1.3 范围外

系统代理与 HTTP API（M4）；代理协议、策略组算法、连通性测试、UDP 中继（阶段 2）；TUN 与进程识别（阶段 3）；HTTP 引擎、MITM、重写（阶段 4）；`rurge reload` / `stop` 命令（依赖 M4 的控制通道）。

### 1.4 子里程碑

| 子里程碑 | 内容 | 验收产出 |
| --- | --- | --- |
| M3a 跑起来 | `rurge-proto`（`Outbound`、`Direct`、`Reject`）、`rurge-policy`（注册表、select 持久选择只读）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine` 最小流水线（dial / relay / 会话日志）、`rurge run` 前台运行 + stdout 日志 | 本地集成测试：HTTP / CONNECT / SOCKS5 经 DIRECT 通过，REJECT 四种行为符合第 8 节；手工：浏览器 / `curl -x` 按规则上网 |
| M3b 可运维 | 请求记录与流量统计、SNI 嗅探、空闲超时、REJECT 自动升级、连接失败错误页、优雅退出、热重载（SIGHUP / `--watch`）、`encrypted-dns-follow-outbound-mode`、`--log-file` | 请求记录断言；重载后新会话用新配置；relay 吞吐基准 |

每个子里程碑一份实施计划，独立分支、独立验收。

## 2. 已确认的技术决策

| 编号 | 决策 | 理由 |
| --- | --- | --- |
| D1 | HTTP 代理监听基于 hyper 1（`server::conn::http1` + `with_upgrades()`；明文转发用 `client::conn::http1`） | 请求行 / 头 / chunked / keep-alive / 100-continue 交给 hyper；已是 workspace 依赖；阶段 4 的 HTTP 引擎在同一 service 里插入 |
| D2 | 入站只负责协议语义，引擎负责规则 / 策略 / 出站 / 转发 / 记录；二者以 `Dialer` trait 相接 | HTTP、SOCKS5、阶段 3 的 TUN 都是「拿到目标 → dial → 转发」 |
| D3 | M3 的出站只有 `Direct`（包 `rurge-net` 的 happy-eyeballs 连接器）与 `Reject`；策略组只实现 `select` 的持久选择，其他组取第一个成员并告警 W0008；未实现协议 → REJECT + 加载告警（阶段 1 设计 D1 / D2） | 避免用户以为走了代理实际直连 |
| D4 | `state.json` 的 schema 在 M3 定义，M3 只读，M4 写 | select 组需要它，API 才有写入来源 |
| D5 | 出站模式初值来自 `--outbound-mode`（默认 `rule`），M4 起改为 `state.json` 优先 | M3 没有运行时修改入口 |
| D6 | 日志用 `tracing-subscriber`，级别映射沿用阶段 1 设计第 12 节 | |
| D7 | M3a 明文 HTTP 每请求一条出站连接，不做连接池 | 简单正确；连接池留给阶段 4 |

## 3. crate 边界与依赖方向

```
rurge (bin: run)
  └─ rurge-engine ── rurge-inbound ── rurge-proto（仅 RejectKind）
        │                 └─ hyper
        ├─ rurge-policy ── rurge-proto
        ├─ rurge-dns ── rurge-rules ── rurge-net ── rurge-config
        └─ rurge-platform（仅 dirs / dns，经 bin 注入）
```

| crate | 职责 | 依赖 |
| --- | --- | --- |
| `rurge-proto` | `Outbound` trait、`Direct`、`Reject`、`OutboundError`、`RejectKind` | rurge-net、rurge-config、rurge-dns（`Direct` 用 `Resolver` 作 `Resolve`） |
| `rurge-policy` | `PolicyRegistry`：名字 → `Resolution`；`GroupSelections` | rurge-proto、rurge-config |
| `rurge-inbound` | `Dialer` / `Dialed` / `DialError`；`HttpListener`、`Socks5Listener`；来源限制 | rurge-net、rurge-config、rurge-proto、hyper、http、bytes、base64（Basic 认证） |
| `rurge-engine` | `Runtime`、`Engine`（`Dialer` 实现）、`relay`、`SessionHandle`、会话日志；M3b：`RequestLog`、`TrafficStats`、`reload`、优雅退出 | 以上全部 + rurge-rules + arc-swap + tracing |
| `rurge`（bin） | `run` 子命令、日志初始化、平台适配器注入 | rurge-engine、rurge-platform、tracing-subscriber |

平台特定代码仍只在 `rurge-platform`（AR-02）；M3 不新增平台代码。

## 4. `rurge-proto`

```rust
pub struct ConnectOpts { pub timeout: Duration /* 10 s */, pub prefer_v6: bool }  // 实现：直接复用 rurge_net::connector::ConnectOpts
pub trait Outbound: Send + Sync {
    fn name(&self) -> &str;                                   // "DIRECT" / "REJECT-TINYGIF" / 策略名
    fn connect_tcp<'a>(&'a self, target: &'a Target, opts: &'a ConnectOpts)
        -> BoxFuture<'a, Result<BoxedStream, OutboundError>>;
}
#[derive(Clone, Copy, PartialEq, Eq)] pub enum RejectKind { Reject, Drop, NoDrop, TinyGif }
pub enum OutboundError { Reject(RejectKind), Unsupported(String), Dns(String), Io(std::io::Error), Timeout }
pub struct Direct { connector: DirectConnector /* Resolve = Arc<Resolver> */ }
pub struct Reject { kind: RejectKind }   // connect_tcp 立即返回 Err(Reject(kind))
```

- `Direct::connect_tcp`：`Target` 为 IP 字面量时直接连接；域名经 `Resolver` 解析（规则评估已解析过时命中缓存），`interleave(addrs, prefer_v6)` 后逐个尝试，整体受 `timeout` 约束；成功返回 `BoxedStream`。`ip-version` 等通用参数属于阶段 2（FR-OUT-03），M3 只按 `general.ipv6` 决定 `prefer_v6`。
- `RejectKind` 与 `rurge_config::policy::Builtin` 的 `Reject* ` 变体一一对应；`Builtin::Cellular` 等 iOS 专属内置视为 `DIRECT`。
- 阶段 2 的每种协议实现 `Outbound`；`underlying-proxy` 链在阶段 2 通过组合 `Outbound` 实现。

## 5. `rurge-policy`

```rust
pub struct GroupSelections(HashMap<String /* group */, String /* member */>);   // state.json 中当前 profile 的选择
pub struct Resolution { pub chain: Vec<String>, pub outbound: Arc<dyn Outbound> }
pub struct PolicyRegistry { .. }
impl PolicyRegistry {
    pub fn build(cfg: &Config, selections: &GroupSelections, resolver: Arc<Resolver>) -> PolicyRegistry;  // W0007 / W0008 / W0009 / W0010 已由 M1 加载器发出，注册表不再返回诊断
    pub fn resolve(&self, policy: &PolicyRef) -> Resolution;
    pub fn names(&self) -> Vec<String>;            // M4 的 /v1/policies
}
```

解析规则（递归，深度上限 16；组循环已在 M1 校验期报 E0009，不可能到达）：

| `PolicyRef` | 结果 |
| --- | --- |
| `Builtin::Direct` | `Direct` |
| `Builtin::Reject*` | 对应 `Reject { kind }` |
| `Builtin::Cellular / CellularOnly / Hybrid / NoHybrid` | `Direct`，加载时告警一次（复用 M1 的平台告警码） |
| `Named(n)` → `[Proxy]` 别名类型 `direct` / `reject*` | 同内置 |
| `Named(n)` → 代理策略（阶段 2 协议） | `Reject { kind: Reject }`，`chain` 末尾标注 `!unsupported:<type>`；加载时每个策略告警一次 `W_POLICY_NOT_IMPLEMENTED`（新增码，见第 11 节） |
| `Named(n)` → `select` 组 | `selections[n]` 若是成员则用它，否则第一个成员；递归 |
| `Named(n)` → 其他组类型 | 第一个成员，加载时告警 W0008；递归 |
| `Named(n)` 未定义 | 不可能（M1 报 E0007）；防御性地按 REJECT 处理并记 ERROR 日志 |
| `Device(_)` | `Reject { kind: Reject }`，加载时告警 W0010 |

`chain` 记录解析路径（如 `["Proxy", "HK", "!unsupported:ss"]`），写入会话日志与请求记录（`policy_chain`）。`build` 预先为每个名字构造 `Arc<dyn Outbound>`（`Direct` 全局共享一个实例），`resolve` 只做查表与递归，无分配热点。

## 6. `rurge-inbound`

### 6.1 与引擎的接口

```rust
pub trait Dialer: Send + Sync {
    fn dial<'a>(&'a self, session: SessionInfo) -> BoxFuture<'a, Result<Dialed, DialError>>;
    fn relay<'a>(&'a self, client: BoxedStream, upstream: BoxedStream, handle: Arc<SessionHandle>) -> BoxFuture<'a, ()>;
}
pub struct Dialed { pub stream: BoxedStream, pub handle: Arc<SessionHandle> }
pub enum DialError {
    Reject { kind: RejectKind, rule: Option<String>, handle: Arc<SessionHandle> },
    Failed { message: String, rule: Option<String>, handle: Arc<SessionHandle> },
}
```

`SessionHandle` 由 `rurge-inbound` 定义（`id`、`rule`、`policy_chain`、原子上下行计数、`finish(outcome)`），引擎持有它写日志 / 记录；入站在 HTTP 明文转发中用它累计请求 / 响应体字节。

### 6.2 `HttpListener`

- `bind(addr: SocketAddr, dialer: Arc<dyn Dialer>, opts: ListenerOpts) -> io::Result<Running>`；`Running { local_addr, task: JoinHandle }`，`Drop` 停止 accept。
- 每个连接：来源限制检查 → `hyper::server::conn::http1::Builder::serve_connection(io, service).with_upgrades()`。
- 认证：`Listener.password` 为 `Some` 时要求 `Proxy-Authorization: Basic base64(user:pass)`，只比较密码部分（用户名任意；见第 12 节 Q1），失败回 `407` + `Proxy-Authenticate: Basic realm="rurge"`。
- `CONNECT host:port`：组 `SessionInfo`（`dst` 来自 authority，`protocol = None`）→ `dial` → 成功回 `200 Connection Established`，`hyper::upgrade::on(req)` 拿到裸流 → `dialer.relay(client, upstream, handle)`；`DialError` 按第 8 节响应。
- 绝对 URI 明文请求：`SessionInfo { protocol: Some(Http), url, http_host, user_agent, dst 来自 URI 的 host:port（默认 80）}` → 每请求 `dial` → 在出站流上 `hyper::client::conn::http1::handshake` → 请求改写：URI 改 origin-form、删除 `Proxy-Authorization` / `Proxy-Connection`、保留 `Host`；响应原样回写；`Connection: close` 由 hyper 处理；一个客户端连接上的多个请求分别 dial（可能命中不同规则），出站连接随响应结束关闭。
- 非绝对 URI 且非 CONNECT 的请求（浏览器直接访问代理端口）：`400`。
- 请求 / 响应体大小经 `SessionHandle` 计数；`Content-Length` 与 chunked 由 hyper 处理。
- 实施订正（M3a）：明文转发的头处理按 RFC 7230 而非原文的「保留 `Host` / 只删 `Proxy-*`」——`Host` 无条件用请求目标的 authority 覆盖（§5.4，且丢弃 userinfo），请求与响应两个方向都剥离逐跳头（`Connection` 列出的 token 及 `Connection` / `Keep-Alive` / `Proxy-Connection` / `TE` / `Trailer` / `Transfer-Encoding` / `Upgrade` / `Proxy-Authenticate` / `Proxy-Authorization`，§6.1；帧头由 hyper 按 body 自行补齐）；只转发 `http` scheme 的绝对 URI，其余回 400；CONNECT 目标缺端口回 400（不再默认 443）；`session.url` 重建时丢弃 `user:pass@`。另：连接的 `header_read_timeout` 设为握手超时 30 s（`ListenerOpts::handshake_timeout`），且必须显式 `.timer(hyper_util::rt::TokioTimer::new())`——不装 `Timer` 时 hyper 会静默丢弃默认的 30 s 头部超时。

### 6.3 `Socks5Listener`

- RFC 1928：方法协商只接受 `0x00`（无认证），否则回 `0xFF` 断开；请求 `CONNECT`（`0x01`）支持 IPv4 / 域名 / IPv6 三种地址类型；`BIND` / `UDP ASSOCIATE` 回 `0x07`（命令不支持）。
- `dial` 成功回 `0x00` + 本地绑定地址（出站流的本地地址，取不到时 `0.0.0.0:0`）→ `relay`；失败按第 8 节回 `0x02` / `0x04` / `0x05`。
- 实施订正（M3a）：「方法协商 + 读 CONNECT 请求」整段包在 `handshake_timeout`（30 s）里，超时按 `io::ErrorKind::TimedOut` 结束且不回应答；拨号与 relay 不在超时范围内。`ATYP=0x03` 且域名长度为 0 时回 `0x01`（general failure）并且不拨号。

### 6.4 来源限制与监听生命周期

- `proxy-restricted-to-lan = true`（默认）且监听地址非回环时：来源必须是回环、私有（RFC 1918）、链路本地、ULA 之一，否则关闭连接并按来源 IP 限频（每分钟一条）记 WARN。监听回环地址时不检查。
- 每个监听器一个 accept 循环 + `JoinSet`：会话 panic 由 `join_next` 捕获记 ERROR；accept 错误记 WARN 并退避 50 ms 后继续。
- 监听地址解析：`Listener.addr` 按字面绑定（IPv6 写 `[::1]:6152`）；端口 0 合法，`Running.local_addr` 给出实际端口。
- 实施订正（M3b）：`serve` 增一个停止令牌；收到后先 `drop` 监听 socket 并触发一个 `closed` 门闩（`Running::wait_closed()` 可等它——重载重绑前必须确认旧 socket 已关，Windows 无 `SO_REUSEADDR`），再排空 `JoinSet` 里的在飞会话。`Running::stop()` 只停这一个监听器的 accept（不影响其他监听器），`join()` 等 accept 循环退出且会话排空完毕。

## 7. `rurge-engine`

### 7.1 Runtime 与 Engine

```rust
pub struct Runtime {
    pub config: Arc<Config>,
    pub rules: RuleEngine,
    pub resolver: Arc<Resolver>,
    pub policies: PolicyRegistry,
    pub stack: Stack,                 // 资源管理器 / 规则集注册表 / GeoIP（M2a）
    pub outbound_mode: OutboundMode,
}
pub struct Engine { runtime: ArcSwap<Runtime>, next_session: AtomicU64, /* M3b: records, traffic */ }
impl Engine {
    pub fn new(runtime: Runtime) -> Arc<Engine>;
    pub fn runtime(&self) -> Arc<Runtime>;
    pub async fn bind_listeners(self: &Arc<Self>) -> io::Result<Vec<Running>>;   // 按 config.general.http_listen / socks5_listen
}
impl Dialer for Engine { .. }
```

- `Stack` 及其构建自 bin 迁入 `rurge_engine::stack`，平台 DNS 由 `StackOptions.system` 注入。

### 7.2 dial 流水线

1. 取 `runtime` 快照（会话期间不变）；分配 `session_id`；建 `SessionHandle`（Active）。
2. 出站模式：`Direct` → `PolicyRef::Builtin(Direct)`；`Proxy(p)` → `p`；`Rule` → `rules.evaluate(&session, mode, resolver).await`：`Outcome::DnsFailed` → `DialError::Failed("dns lookup failed")`；`Outcome::Policy(p)` → 记 `matched` 规则原文与 `sub_rule`。
3. `policies.resolve(&p)` → `Resolution { chain, outbound }`；`chain` 写入 handle。
4. `outbound.connect_tcp(&Target::new(dst_host, dst_port), &ConnectOpts { timeout: 10 s, prefer_v6: config.general.ipv6 }).await`：`Err(Reject(kind))` → `DialError::Reject`（handle 结束为 Rejected）；`Err(Unsupported(t))` → `DialError::Reject { kind: Reject }` 且 handle 的 `error = "policy protocol not implemented: <t>"`；`Err(Dns | Io | Timeout)` → `DialError::Failed`（handle 结束为 Failed）。
5. 成功 → `Dialed { stream, handle }`，handle 记 `connect` 耗时与远端地址。

### 7.3 relay 与会话日志

- `relay`：`copy_bidirectional` 的变体，每方向一个 8 KB 缓冲，字节数写入 handle 的原子计数；一方向 EOF 时对另一方向 `shutdown`；M3b 加空闲超时（两个方向都无数据超过阈值则关闭）。
- 实施订正（M3a）：所谓「变体」的实现是把 `Counting` 包在 upstream 一侧后交给 `tokio::io::copy_bidirectional`——写 upstream 计 `up`、读 upstream 计 `down`，计数随字节移动而累加，因此复制中途失败时已传字节仍保留在 handle 上。另：`SessionHandle` 新增 `error` 字段，`Rejected` 日志带上它（未实现协议的文案见第 8 节）。
- `SessionHandle::finish` 只触发一次：写一条日志 —— `Completed` 为 DEBUG（`loglevel = info` 可见），`Failed` / `Rejected` 为 INFO；字段 `session`、`listener`、`src`、`dst`、`rule`、`policy`（chain 用 ` > ` 连接）、`up`、`down`、`elapsed_ms`、`error`。
- M3b：`RequestLog`（环形缓冲，默认 1000，`--request-log-size`；活动索引支持 `kill`）与 `TrafficStats`（总计、按策略、按监听器的原子计数；每秒采样速率）从同一份 handle 数据填充；M4 的 `GET /v1/requests/*`、`/v1/traffic` 只读它们。
- 实施订正（M3b）：relay 换成手写可中断双向泵（`relay.rs::pump`），不是 `copy_bidirectional` 的变体——`tokio::io::split` 拆成两个方向，各自跑一个独立的 `copy_half`（`tokio::join!`），每次 `read` 与 `write_all` 都与会话令牌的子令牌竞速，因此一个方向卡在 `write_all` 上不会挡住另一方向（无队头阻塞）；另起一个 `idle_watchdog` 任务，两个方向都无字节移动超过 `idle`（默认 600 s，`--idle-timeout` 覆盖）时取消该子令牌。结束原因三选一并分别记录：`kill` → `Failed("killed")`；父令牌取消（优雅退出）→ `Completed`；空闲计时器触发 → `Completed`。
- 实施订正（M3b 修复波）：空闲超时只覆盖经 `pump` 的会话（CONNECT / SOCKS5）。明文 HTTP 转发不经过 `pump`，因此 `--idle-timeout` 对它不适用——保活等待由 hyper 自身的 `header_read_timeout` 覆盖，单次交换的寿命由上游连接自身约束（阶段 4 自有 HTTP 引擎后统一）。该路径仍然监听会话令牌：驱动上游连接的任务与 `send_request` 都与令牌竞速（令牌优先），因此 `kill` → `Failed("killed")`、优雅退出的 `cancel_sessions` → `Completed`，与 `pump` 的结束原因阶梯一致；已登记进兼容性清单的 `idle-timeout` 行。

### 7.4 M3b 的引擎扩展

- **SNI 嗅探**：CONNECT 与 SOCKS5 成功 dial 之前，对客户端首个片段 `peek`（≤ 300 ms 或 16 KB）解析 TLS ClientHello 的 SNI，回填 `session.sni` 与 `protocol = Https`；数据不消费，原样转发；HTTP 明文请求不嗅探。
- **REJECT 自动升级**：按目标主机（域名或 IP）键控的滑动窗口计数器，30 s 内 REJECT / TINYGIF 达 50 次后该主机后续按 DROP；NO-DROP 不计入也不受影响。
- **热重载**：`Engine::reload(config) -> Result<Diagnostics, LoadError>`：加载失败保留旧 Runtime 并返回诊断；成功则构建新 Runtime（DNS 相关键与 `[Host]` 未变时复用旧 `Resolver`，否则新建并 `flush` 旧的；`Stack` 每代新建，规则集与 GeoIP 依赖磁盘缓存）→ `ArcSwap::store` → 监听地址集合变化时重建监听器 → 记一条 INFO。触发：Unix SIGHUP；`--watch` 监视主配置与 `#!include` 文件（去抖 500 ms）。
- **优雅退出**：Ctrl-C / SIGTERM → 停止 accept → 等活动会话最多 5 s → 退出；第二次 Ctrl-C 立即退出。
- **`encrypted-dns-follow-outbound-mode`**：为 `Resolver` 注入「走流水线的连接器」——DNS 上游连接组成 `SessionInfo { protocol: Some(Doh / Dot / Dns) }` 经 dial 分流；防环：DNS 会话内部的解析用 `Bootstrap`（不再进入规则引擎），命中的策略若是域名配置的代理则告警并回退 DIRECT（阶段 2 有代理后才可能触发）。
- 实施订正（M3b）：SNI 嗅探不是「dial 前 peek」，而是 `pump` 里客户端→上游方向 `copy_half` 的首个非空 chunk 钩子（≤ 8 KiB，即 relay 的读缓冲大小）——HTTP 明文转发不经过 `pump`，因此不嗅探；只解析单个 ClientHello，跨 TCP 段的分片本段解析不到就放弃、不做拼接；结果只填 `session.sni` / `protocol` 供请求记录与日志观测，路由仍按 CONNECT / SOCKS5 的目标主机，基于 SNI 的路由留给阶段 4。REJECT 自动升级按 `count >= 50` 判定，即第 50 次拒绝本身就已按 DROP 处理（不是第 51 次才升级）；计数以目标主机字符串为键的滑动窗口（30 s），超过 4096 个不同主机时清理空闲条目。热重载没有做「DNS 相关键未变则复用旧 Resolver」的优化——`Runtime::build` 每代都完整重跑 `build_stack`（资源 → 规则集 → GeoIP → Resolver），因此每次重载都清空 DNS 缓存。`encrypted-dns-follow-outbound-mode` 的连接器挂在 `Resolver` 自带的 `BootstrapConnector` 内层：上游主机名由 Bootstrap 用明文 UDP 先解析，流水线侧的连接器只会看到 IP 目标（域名分支只是防御性兜底，正常路径不会走到），因此域名规则不会匹配上游服务器的主机名，且这类内部会话的 `SRC-IP` / `IN-PORT` 是 `SessionInfo::tcp` 的占位默认值（`127.0.0.1:0` / `0`），`kill` 对其无效（DNS 路径不监听取消令牌）；被规则 REJECT 时回退到直连以保证 DNS 不因规则配置整体失效；UDP 上游不经过 `Connector`，不受影响。
- 实施订正（M3b 修复波）：重建监听器的判据是**监听器配置面**（`Engine::listener_specs` 的地址集合、`password@` / `allow-wifi-access` 认证、`proxy-restricted-to-lan`、`show-error-page` 与 `show-error-page-for-reject`）任一变化，而不只是「监听地址集合变化」。`ListenerOpts` 在 `bind` 时定型、之后不再刷新，只比地址会让密码轮换与来源限制在重载后静默不生效，而日志仍记 `profile reloaded`。整条重绑序列（`stop` → `wait_closed`（上界 `REBIND_WAIT` = 2 s）→ 后台 `join` 排空 → `bind_listeners`）收进 `Engine::rebind_listeners`；重绑失败会留下「零监听器」的退化态，因此 `rurge run` 在监听器列表为空时无条件重试绑定，并把失败记为 ERROR。REJECT 自动升级的 4096 主机上限是硬上限：表满且清理不出空位时，新主机不被记录（因而不会升级），与 `listener::warn_due` 同一模式。

## 8. REJECT 语义与错误处理

| 情况 | 明文 HTTP | CONNECT | SOCKS5 |
| --- | --- | --- | --- |
| REJECT / REJECT-NO-DROP | `show-error-page-for-reject = true` → `403` + rurge 错误页（HTML，写明命中规则与策略链），否则直接关闭 | 关闭连接 | 回 `0x02`（规则不允许）后关闭 |
| REJECT-TINYGIF | `200` + `image/gif`，1×1 透明 GIF（43 字节） | 关闭 | 回 `0x02` 后关闭 |
| REJECT-DROP | 不响应，保持连接直到客户端关闭或 30 s | 同左 | 同左（不回应答） |
| 未实现协议的策略 / `DEVICE:` | 同 REJECT，`error = "policy protocol not implemented: <type>"` | 同 REJECT | 同 REJECT |
| DNS 失败 / 连接失败 / 超时 | `show-error-page = true` → `502` + 错误页，否则关闭 | M3a 关闭；M3b `502` | `0x04` 主机不可达 / `0x05` 连接被拒 |
| Basic 认证失败 | `407` + `Proxy-Authenticate: Basic realm="rurge"` | 同左 | 不适用 |
| 非 CONNECT 命令 | 不适用 | 不适用 | `0x07` |
| 来源不允许（restricted-to-lan） | 关闭 | 关闭 | 关闭 |

- 错误页：内嵌 HTML 模板（英文，含 `rurge`、规则原文、策略链、会话 id），`Content-Type: text/html; charset=utf-8`，不引用外部资源。
- 配置错误：`run` 启动时打印诊断（与 `check` 同格式）并退出 2；告警只在启动时打印一次。
- 会话层错误转成 handle 的 `error` 与一条 INFO 日志；panic 由 `JoinSet` 边界隔离并记 ERROR。
- 实施订正（M3a）：REJECT-DROP 的「直到客户端关闭或 30 s」在 SOCKS5 侧两句都成立，HTTP 侧只实现了后半句——hyper 的服务内观察不到客户端关闭，只能固定保持到超时；阶段 4 换成 rurge 自有 HTTP 引擎后统一（已登记进兼容性清单 4.1 表）。
- 实施订正（M3b）：CONNECT 隧道 dial 失败（DNS / 连接 / 超时）现在也回 `502 Bad Gateway` + 错误页（受 `show-error-page` 控制），补齐了上表中「CONNECT：M3a 关闭；M3b 502」的差距；REJECT 自动升级的判定细节见 §7.4 的实施订正。

## 9. 运行时状态、日志与 `rurge run`

### 9.1 `state.json`

数据目录下的 `state.json`（M3 只读；M4 写入采用临时文件 + 重命名）：

```json
{
  "version": 1,
  "outbound_mode": "rule",
  "global_policy": null,
  "features": { "system_proxy": false, "enhanced_mode": false, "mitm": false, "capture": false, "rewrite": false, "scripting": false },
  "group_selections": { "<profile 文件名>": { "<组名>": "<成员名>" } },
  "system_proxy_backup": null,
  "current_profile": null
}
```

M3a 读取 `group_selections[<当前 profile 文件名>]`（缺失或解析失败 → 空选择，记一条 WARN）；`outbound_mode` 在 M4 起才优先于 CLI。

### 9.2 日志

- `loglevel`：`verbose` → TRACE、`info` → DEBUG、`notify` → INFO（默认）、`warning` → WARN；`--log-level` / `RURGE_LOG_LEVEL` 覆盖，额外接受 `debug`、`error`。
- 输出 stdout，TTY 时着色；每条会话日志带第 7.3 节字段；`password@`、策略密码、API key 不进入日志（`Listener` 的 `Debug` 实现脱敏）。
- M3b：`--log-file <path>` 按天滚动，保留 7 个。
- 实施订正（M3b）：`--log-file` 用 `tracing-appender` 的 `rolling::Builder`（`Rotation::DAILY` + `max_log_files(7)`）配 `tracing_appender::non_blocking`；文件层与 stdout 层同时挂载（不是二选一），`WorkerGuard` 随进程存活以保证退出前的日志不丢；文件层未加 `with_target(false)`，格式与 stdout 略有差异。

### 9.3 `rurge run`

```
rurge run -c <conf> [--outbound-mode direct|proxy=<policy>|rule] [--log-level <level>] [--platform <p>]
          [--data-dir <dir>] [--no-network] [--geoip-url <url>] [--geoip-asn-url <url>] [--dns-cache-size <n>]
          （M3b）[--watch] [--log-file <path>] [--request-log-size <n>]
```

启动顺序：加载配置（错误 → 诊断 + 退出 2）→ `build_stack`（`wait = 0`，外部资源后台加载，规则集就位后由注册表热切换）→ 读 `state.json` → 构建 `Runtime` / `Engine` → 绑定监听（失败 → 退出 1；每个监听打印 `listening on http://127.0.0.1:6152` / `socks5://...`）→ 打印摘要（策略数、规则数、出站模式）→ 等 Ctrl-C → 退出 0（M3a 立即；M3b 优雅退出）。

超时常量（M3a）：连接 10 s；REJECT-DROP 保持 30 s；M3b 空闲超时默认 10 分钟（Q2）。

实施订正（M3b）：命令行实际还有 `--idle-timeout <secs>`（默认 600，即 Q2 的 10 分钟；未在上面的示意中列出）。主循环用一次 `select!` 在两个分支间选择——一个等 Ctrl-C（Windows）或 Ctrl-C/SIGTERM（Unix）触发退出，一个等 SIGHUP（Unix）或 `--watch` 的文件事件触发 `reload`；两个信号流都在循环外只构造一次（`tokio::signal::unix::signal` / `windows::ctrl_c()`），而不是每轮循环重新 `await` 一次性的 `ctrl_c()`，否则重载耗时的几秒内到达的信号会被新注册的监听丢弃。退出序列：`stop_accepting` → `tracker().close()` → 等待所有监听器 `join()` 与 `tracker().wait()`；期间再收到一次 Ctrl-C 立即强制退出，否则最多等 `GRACE`（5 s）后 `cancel_sessions()` 强制结束在飞会话。

## 10. 测试策略

| 层 | 内容 |
| --- | --- |
| 单元 | SOCKS5 编解码；HTTP 代理头改写；`PolicyRegistry::resolve` 表驱动（内置 / 别名 / select 持久与首成员 / 嵌套 / 未实现协议 / `DEVICE:`）；`Reject` 出站；来源限制谓词；日志级别映射；`state.json` 解析与缺省 |
| 集成（`rurge-engine/tests`，全部 127.0.0.1） | 配置文本 + `rurge_net::testing::TestServer`（明文与 TLS 目标站）+ `rurge_dns::testing::MockDns`（IP 规则解析）启动 `Engine`：明文 HTTP 经代理取到目标响应且 handle 含规则 / 策略 / 字节数；CONNECT 隧道内 TLS 握手；SOCKS5 CONNECT；REJECT 四种行为；407 与正确凭据；未实现策略 → REJECT + 告警；出站模式 direct / proxy；DNS 失败；同一 keep-alive 连接上两个请求分别命中 DIRECT 与 REJECT；M3b：请求记录字段、自动升级、重载后新会话用新配置、优雅退出 |
| CLI | 子进程 `rurge run -c t.conf --no-network`（`http-listen = 127.0.0.1:0`），解析 `listening on` 取端口，经代理请求 `TestServer`，结束子进程；配置错误退出 2；端口占用退出 1 |
| 基准（M3b） | 回环 relay 吞吐（criterion，CI 只编译），对应 NFR-01 单核 ≥ 1 Gbps |
| 手工验收 | 浏览器 / `curl -x http://127.0.0.1:6152` 与 `--socks5 127.0.0.1:6153` 按规则上网；三平台 CI 绿 |

测试不访问公网。

## 11. 兼容性清单需登记的差异（实施时更新 `docs/surge-compatibility-matrix.md`）

| 项 | 状态 | 说明 |
| --- | --- | --- |
| REJECT-DROP 保持时长 | 🟡 | rurge 最多 30 s；Surge 直到客户端超时 |
| `proxy-restricted-to-lan` 判定 | 🟡 | rurge 按回环 / 私有 / 链路本地 / ULA；手册原文是「只接受当前子网的设备」 |
| `http-listen` 的 `password@` | 🟡 | Basic 认证只比较密码，用户名任意（手册只给出 `[password@]address[:port]`） |
| 错误页内容 | 🟡 | rurge 自己的 HTML |
| 出站模式初值 | 🟡 | M3 来自 `--outbound-mode`；M4 起 `state.json` 优先 |
| SOCKS5 对 REJECT 的应答码 | 🟡 | 回 `0x02`；手册未说明 |
| 未实现协议的策略 | 🟡 | 按 REJECT + 告警 `W_POLICY_NOT_IMPLEMENTED`（新增诊断码，实施时分配下一个 W 号） |
| 明文 HTTP 出站不复用连接 | 🟡 | 每请求一条连接（阶段 4 引入连接池） |

## 12. 开放问题

| 编号 | 问题 | 处理 |
| --- | --- | --- |
| Q1 | `http-listen` 的 `password@` 对应 Basic 认证时用户名是否任意 | 已核对手册（2026-09-05）：语法为 `[password@]address[:port]`，只有密码、未提用户名；rurge 只比较 Basic 凭据的密码部分，用户名任意，登记 🟡 |
| Q2 | 空闲超时默认值（手册未说明） | M3b 拟 10 分钟，可用 `--idle-timeout` 覆盖 |
| Q3 | IPv6 监听地址是否自动双栈 | 按字面绑定，不自动展开；需要双栈时配置两个监听器 |
| Q4 | 明文 HTTP 请求的 `Via` / `X-Forwarded-For` 头 | 不添加（Surge 不添加） |

## 13. 与后续阶段的接缝

- **M4**：`Engine` 暴露 `runtime()`、`request_log()`、`traffic()`、`reload()`、`set_outbound_mode()`、`policies().select()`；API 只调用这些方法；`state.json` 的写入与系统代理开关在 M4；`rurge reload` / `stop` 经 API。
- **阶段 2**：协议策略实现 `Outbound`；`PolicyRegistry` 的组决策换成真实算法（url-test / fallback / load-balance / smart / subnet）；`ConnectOpts` 扩展 14 个通用参数；UDP 经新的 `connect_udp`。
- **阶段 3**：TUN 入站实现 `Dialer` 的调用方，复用 dial / relay；进程识别填 `SessionInfo.process`。
- **阶段 4**：`HttpListener` 的「dial → hyper 客户端转发」段替换为 HTTP 引擎（MITM → 重写 → 脚本 → Map Local）。
