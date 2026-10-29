# 阶段 4 / M1「会话入口与明文 HTTP 引擎」细化设计

| 项       | 内容 |
| -------- | ---- |
| 日期     | 2026-10-29 |
| 状态     | 草案。已决事项 M1-D1 ～ D27 是在项目所有者"自己决定、继续做"的常设授权下定的（代项目所有者作的裁定），所有者可以改。执行交给 Pi：两份计划（M1a、M1b），由协调的会话逐个任务调 Pi 做、逐个任务评审（第 2 节） |
| 上位设计 | [阶段 4 总设计](2026-10-29-phase4-http-engine-design.md) 第 1.4、4、5、6、9.1、13、14 节。不一致处以本文为准，订正列在第 17 节 |
| 需求     | FR-HTTP-01（80 / 443 的嗅探、`force-http-engine-hosts`、`always-raw-tcp-hosts` / `always-raw-tcp-keywords`）；FR-RULE-07 / 08 的拨号之前部分（规则之前就有 SNI 与协议）；FR-RULE-01 的 HTTP 类（引擎里的每个请求各自求规则）；FR-HTTP-10 的一部分（每个请求一条记录：方法、URL、状态码、所用上游连接的 `remoteAddr`） |
| 清单     | 1.x 节 L131（Host List 的缺省端口）；2.x 节 L172 ～ L174（三个设置）；3.2 节 L265（`extended-matching`）；3.5 节 L314（规则集文件里一行上的 `extended-matching`）；7.1 节 L562 ～ L570。行号是 2026-10-29（`3ec732e`）的 |
| 前置     | 阶段 3 做完（M1 ～ M6）；阶段 4 总设计提交为 `3ec732e`。`origin/main` = `41c35d4`，CI 运行 40824893108（2026-10-29）三个平台全绿；本地 `main` 领先 5 个文档提交，等项目所有者推送 |
| 调研     | 总设计的三份调研（`docs/reports/2026-10-29-phase4-*.md`）；本机 cargo 注册表里锁定版本的源码：hyper 1.11.1 的 `proto/h1/conn.rs`（第 210 ～ 233 行 `header_read_timeout` 从读一个头开始计时，空闲的 keep-alive 连接也算；第 487 ～ 504 行 `mid_message_detect_eof`：请求在途、正文已读完、缓冲空着时客户端的 EOF 让连接以"消息不完整"结束，在途的服务 future 随之被丢掉）与 `client/conn/http1.rs`（第 184 ～ 191 行 `is_ready` / `is_closed`；第 259 ～ 290 行 `try_send_request`：请求还没写出时把它还回来）；手册 `profile/general.html`（三个设置、两个错误页设置）、`policies/reject.html`（REJECT 一族与预匹配）、`rules/ruleset.html`（一行上的 `extended-matching`）、`rules/protocol-and-network.html`（`PROTOCOL`） |

M1 做阶段 4 的入口与明文的一半：80、443 与 `force-http-engine-hosts` 命中的 TCP 会话先答后读、先嗅探再求规则，于是 SNI 与协议在规则之前就有，`extended-matching` 与 `PROTOCOL,HTTPS` 对真实会话第一次生效（清单 L265，连同规则集文件里一行上的标志、L314，阶段 1 两句没做完的其中一句）；判为 HTTP 的连接进一个 hyper 写的明文 HTTP/1.1 引擎，每个请求各自求规则、选策略、借或拨上游连接，各有一条带方法、URL、状态码与远端地址的记录；HTTP 监听器的绝对形式请求不再自己转发，交给同一个引擎（今天的 `forward()` 退役）。手册里规则为什么要等第一个包，原话是 "Due to the extensive range of properties that Surge's rule system can evaluate, rule determination can normally only occur after receiving the first TCP packet."（`policies/reject.html`）。

**M1 没有平台代码**：改的是 `rurge-http`（新）、`rurge-inbound`、`rurge-tun` 的 `session.rs`、`rurge-rules`、`rurge-engine`、`rurge-net`、`rurge-api` 里不带 `cfg` 的部分，开发机上全部编译、全部跑；没有等 CI 的停点。

## 1. 范围

| 做 | 不做（去向） |
| -- | ------------ |
| 新 crate `rurge-http` 的第一部分：前缀流、TLS ClientHello 的读取（记录的重组与握手消息的解析分开，第二半给 M5 的 QUIC 复用）、HTTP 开头的判断、嗅探（第 6 节，M1a） | QUIC Initial 里的 SNI（M5）；明文 HTTP/2 先验进引擎（不做：照原样转发，总设计 P4-D11） |
| 可嗅探的判定：80 / 443、`force-http-engine-hosts`、`always-raw-tcp-*`；预匹配会拒、引擎暂停时不嗅探（第 4 节，M1a） | `[MITM] hostname` 写明的端口进可嗅探的条件（M2） |
| 三种入口（SOCKS5、HTTP 监听器的 `CONNECT`、虚拟网卡）对可嗅探的会话先答后交（第 5 节，M1a） | 不可嗅探的会话：一行不变 |
| 嗅探之后按原样转发的会话：规则之前带 SNI 与协议；规则集文件里一行上的 `extended-matching`（第 7 节，M1a） | 不可嗅探的端口上的 SNI 进规则（不做：只有可嗅探的会话嗅探，总设计第 5.2 节） |
| 判为 HTTP 的连接：M1a 先照原样转发、协议记 `HTTP`；M1b 进 HTTP 引擎（第 7.2、8 节） | 解密（M2）；HTTP/2（M2） |
| 明文 HTTP/1.1 引擎：keep-alive、chunked、每个请求求规则选策略、每条客户端连接一个上游连接池、REJECT 一族逐请求答复、502 / 504 错误页、WebSocket 与隧道里的 `CONNECT` 透传（第 8 节，M1b） | 四种重写与 Map Local（M3）；抓包（M4） |
| HTTP 监听器的绝对形式请求改走引擎，主机是 fake 地址时反查回域名（R27），`forward()` 退役（第 9 节，M1b） | `Via`、`X-Forwarded-For`（不加，总设计第 6.5 节） |
| 每个请求一条记录：方法、URL（去掉 userinfo）、状态码、`remoteAddr`（借来的连接同样有）；承载请求的连接只在一个请求也没有走到求规则时有一条（第 10 节，M1b） | 各段耗时、正文字节数、备注里的重写与放过的原因（M4）；记录容量够不够（M4 量） |
| 计数：嗅探的五种结局（M1a）与请求数（M1b），`GET /v1/rurge/http`（第 11 节） | 解密与握手失败的计数（M2）、缓冲放弃的计数（M3） |
| — | 上游连接跨客户端连接共享（不做，总设计 P4-D6）；引擎里逐请求求值的开销基准（M5） |

## 2. 计划与执行

两份计划，先 M1a 后 M1b。M1b 在 M1a 合并之后开工，它的计划也在那时对着合并之后的代码写（免得两份计划之间漂移）：

- `docs/superpowers/plans/2026-10-29-phase4-m1a-sniffing-plan.md`
- `docs/superpowers/plans/2026-10-29-phase4-m1b-http-engine-plan.md`

M1a 的任务：

| 任务 | 内容 |
| ---- | ---- |
| 0 | 开工前核对：第 16 节的各项对着当时的代码核一遍，出入记进「执行期修正记录」 |
| 1 | `rurge-http` 建 crate；前缀流 `Prefixed`（第 6.1 节） |
| 2 | ClientHello 的读取与 HTTP 开头的判断；`sniff`（等待、64 KiB、五种结局）（第 6.2 ～ 6.4 节） |
| 3 | `rurge-rules`：规则集文件里一行上的 `extended-matching`（L314）；预匹配的不计数探询 `pre_refuses`（第 4.3、7.3 节） |
| 4 | `rurge-inbound`：`Dialer` 的 `sniffable` 与 `serve`（带缺省实现）、`ServeOpts`、通用的停留 `hold_dropped`；SOCKS5 与 `CONNECT` 先答后交（第 5 节） |
| 5 | `rurge-tun`：可嗅探的 TCP 会话交给 `serve`（第 5.3 节） |
| 6 | 引擎：`sniffable` 与 `serve`（嗅探 → 带 SNI 与协议的会话 → 拨号 → 前缀与转发；停留；停止）；记录里的 SNI；计数（第 4、7 节） |
| 7 | 引擎的端到端用例：三种入口 × 嗅探的结局，`extended-matching` 命中 SNI，拨号之前的 `PROTOCOL,HTTPS`，三个设置，预匹配，暂停（第 14 节） |
| 8 | API：`GET /v1/rurge/http` 的嗅探计数（第 11 节） |
| 9 | 文档收尾（第 15 节） |

M1b 的任务：

| 任务 | 内容 |
| ---- | ---- |
| 0 | 开工前核对（同上） |
| 1 | `rurge-net`：`Trail::set_peer`；`rurge-inbound`：错误页正文的错误类型放宽（第 8.7 节）；`rurge-http`：上游连接池（第 8.5 节） |
| 2 | `rurge-http`：HTTP/1.1 引擎的骨架：每个连接一个 hyper 服务、`Upstreams`、每个请求的会话信息、逐跳的头、拒绝与失败的答复、时限、守卫（第 8.1 ～ 8.4、8.7、8.8、8.10、8.11 节） |
| 3 | `rurge-http`：升级与隧道里的 `CONNECT`；重发的规则（第 8.6、8.9 节） |
| 4 | 引擎：`connect_session` 拆成选路与拨号两步；`Upstreams` 的实现；`serve` 把判为 HTTP 的连接交给引擎；承载连接的记录（第 8.3、8.4、10.2 节） |
| 5 | 引擎与 API：记录的新字段、`RequestJson` 的新键、每个请求的日志行（第 10、12 节） |
| 6 | HTTP 监听器：绝对形式请求交给 `Dialer::plain_requests`、R27、`forward()` 退役、它的用例搬到引擎（第 9 节） |
| 7 | 端到端用例：四种入口上每个请求各自命中规则、连接复用（服务器数连接）、借来的连接的 `remoteAddr`、REJECT 逐请求、WebSocket 回显、chunked、keep-alive、重载与出口表变化之后不借旧连接（第 14 节） |
| 8 | 计数 `requests`；文档收尾（第 11、15 节） |

**执行的约定**（两份计划开头重申，同阶段 3 的 M4d ～ M6）：协调的会话逐个任务开 Pi 的会话、把任务的全文与要用的接口交给它；Pi 做完写报告、提交；协调者读改动、跑门禁、评审，有问题回到同一个 Pi 会话修正，再评审，过了才派下一个任务。每个任务先写失败的用例、看到红，再实现到绿；门禁是 `CLAUDE.md` 的全量门禁；提交说明的标题用中文。计划没有写到、要做决定的地方：能按本文与总设计的原则定的，执行者自己定，记进修正记录并写"执行者的裁定"；去向写着"项目所有者"的、或会改变本文已决事项的，停下来列出。仓库的规矩（`CLAUDE.md` 的「必须遵守的规矩」「测试约定」「这台开发机」）一律适用。

做完 M1b 之后：项目所有者推送，读回那次 CI 的注解与任务摘要（M1a 之后不单独停下来等推送：没有平台代码）；再往后是 M2（MITM）。

## 3. crate 与接口

不加包：M1 用到的 `hyper`、`hyper-util`、`http`、`http-body`、`http-body-util`、`bytes`、`tokio`、`tokio-util` 都已在工作区里（任务 0 核 `http-body` 是不是工作区依赖，不是的话照 `rurge-inbound` 的写法引用）。新的依赖边只有两条：`rurge-http → { rurge-config, rurge-net, rurge-inbound, rurge-proto }`（`rurge-proto` 只为 `RejectKind`）与 `rurge-engine → rurge-http`。`rurge-tun` 不依赖 `rurge-http`：虚拟网卡把流交给 `Dialer::serve`，嗅探在引擎里做。`rurge-http` 是 `forbid(unsafe_code)`。

| crate | 改动 |
| ----- | ---- |
| `rurge-http`（新） | M1a：`prefix`（`Prefixed<S>`）、`sniff`（`judge`、`sniff`、`Sniffed`、`ClientHello`）、`tls`（记录的重组、握手消息里 ClientHello 的解析）。M1b：`engine`（每个连接的 hyper 服务、每个请求的流水线、`Upstreams`）、`pool`（上游连接池）、`headers`（逐跳的头，从 `rurge-inbound` 的 `http.rs` 搬来） |
| `rurge-inbound` | `Dialer` 加三个带缺省实现的方法：`sniffable`、`serve`（M1a）与 `plain_requests`（M1b）；`ServeOpts`、`hold_dropped`（M1a）；`PlainRequests`（M1b）；SOCKS5 与 `CONNECT` 的先答后交（M1a）；HTTP 监听器的绝对形式请求交给 `plain_requests`，`forward()`、`absolute_form`、`origin_form` 退役（M1b）；`ResponseBody` 的错误类型放宽（M1b） |
| `rurge-tun` | `session::dial_and_relay` 对可嗅探的会话改调 `serve`（M1a） |
| `rurge-rules` | `CompiledSet` 一行上的 `extended-matching`（M1a）；`RuleEngine::pre_refuses`（M1a） |
| `rurge-net` | `Trail::set_peer`（M1b） |
| `rurge-config` | `SessionInfo` 的 `url` 一段注释改写（第 8.3 节，M1b）；不加字段 |
| `rurge-engine` | `Dialer::sniffable` 与 `serve` 的实现、嗅探之后的分派、计数（M1a）；`EngineShared.sniff_wait`（M1a）；`connect_session` 拆成选路与拨号、`Upstreams` 与 `PlainRequests` 的实现、承载连接的记录、记录的新字段与日志行（M1b）；`record_of` 的 SNI 取法（M1a） |
| `rurge-api` | `GET /v1/rurge/http`（M1a 起）；`RequestJson` 的三个新键（M1b） |

接口的形状（细节以计划为准）：

```rust
// rurge-inbound（M1a）
pub struct ServeOpts {
    /// 交出连接的一方停下时取消（虚拟网卡）；监听器给 None
    pub stop: Option<CancellationToken>,
    pub drop_hold: Duration,
    pub linger: Duration,
}
pub trait Dialer: Send + Sync {
    // …今天的四个方法不变…
    /// 这个会话要不要先答后读（第 4 节）。缺省 false。
    fn sniffable(&self, session: &SessionInfo) -> bool { false }
    /// 接手一个已经答过"成功"的可嗅探会话，直到它结束（第 7 节）。
    /// 缺省：dial → relay；REJECT-DROP 时 hold_dropped。
    fn serve<'a>(&'a self, session: SessionInfo, client: BoxedStream, opts: ServeOpts) -> BoxFuture<'a, ()> { … }
    // M1b：
    /// HTTP 监听器的一条客户端连接上，绝对形式的请求交给谁（第 9 节）。缺省 None。
    fn plain_requests(&self, conn: ClientConn) -> Option<Arc<dyn PlainRequests>> { None }
}

// rurge-http（M1b）
pub trait Upstreams: Send + Sync + 'static {
    /// 池里连接的键（第 8.5 节）：引擎给出，只要求能比较相等
    type Key: PartialEq + Send + Sync + 'static;
    /// 一个选好了路、还没拨号的请求
    type Route: Send + 'static;
    /// 一个请求：建它的记录、求规则、选策略；或拒绝、失败（句柄已结束）
    fn route(&self, request: RequestFacts) -> BoxFuture<'_, Routed<Self::Route, Self::Key>>;
    /// 真拨号，连同拨号时的记账（第 8.4 节）
    fn dial(&self, route: Self::Route, handle: Arc<SessionHandle>) -> BoxFuture<'_, Result<Upstream, DialError>>;
    /// 升级之后两侧的转发，直到结束（第 8.9 节）
    fn relay(&self, client: BoxedStream, upstream: BoxedStream, handle: Arc<SessionHandle>) -> BoxFuture<'static, ()>;
}
```

`Upstream` 是拨好的流、`Dialed.forward` 那组头（以绝对形式交给 HTTP 代理时要加的）与拨号时 `Trail` 记下的远端地址。`Routed` 是"走"（句柄、路、键、转发的方式）、"拒绝"（句柄与 `RejectKind`）、"失败"（句柄、`FailKind` 与原因）三种。

## 4. 可嗅探的判定（M1a）

### 4.1 条件

`Engine::sniffable(session)` 对下面的全部成立时为真（总设计第 5.2 节，加两条）：

1. 会话是 TCP，经 HTTP 监听器的 `CONNECT`、SOCKS5 或虚拟网卡来（引擎自己的会话从不经过监听器）；
2. 引擎没有暂停（托管 Profile 的 `strict`，`is_suspended()`）——暂停时照今天的路拒绝，客户端拿到今天的应答码与错误页，记录照今天；
3. 预匹配不拒它（第 4.3 节）；
4. 目标不命中 `always-raw-tcp-hosts`（`HostList::matches(主机, 端口) == Some(true)`）与 `always-raw-tcp-keywords`（第 4.2 节）；
5. 目标端口是 80 或 443，或者目标命中 `force-http-engine-hosts`（`HostList::matches` 为 `Some(true)`；这个名单的缺省端口是 80，`host:0` 是全部端口，`*:0` 是全部）。

"目标"是连接的目标：`CONNECT` 与 SOCKS5 给的主机、虚拟网卡上 fake 地址背后的名字或真实 IP。经 HTTP 监听器与 SOCKS5 来的目标是 fake 地址时（客户端用 rurge 的 DNS 拿到它，再经代理端口连），比名单之前先用 `FakeIpPool::name_of`（不租用）换成它背后的名字——拨号时 `turn_back` 照旧租用。`always-raw-tcp-*` 先于一切（总设计第 2.3 节 3.12）。设置读自现在这一代（`runtime()`）。

### 4.2 `always-raw-tcp-keywords`

手册："Any hostname containing one of the keywords will skip protocol sniffing"。只对名字比（IP 不是 hostname）；不分大小写：名字按约定是小写的，关键字在比较时转成小写（解析时不改它，`rurge check` 与脱敏视图照写的显示）。

### 4.3 预匹配

SOCKS5 与 `CONNECT` 的可嗅探会话先答"成功"再求规则，带 `pre-matching` 的 REJECT 规则就会晚到答过之后才拒——今天它们在答复之前就拒（规则求值时的预匹配先行，`RuleEngine::pre_pass`）。手册说预匹配在 "the DNS resolution and TCP SYN phases" 生效、为的是"低开销地快速拒绝"；代理端口上没有 SYN，最接近的是"答复之前"。所以 `sniffable` 先问一次：规则模式下、预匹配的规则拒这个会话（照它现在的样子，没有 SNI）时，`sniffable` 为假，会话照今天的路走，由拨号里的规则求值照今天拒绝、记录、计入升级（`escalation`）。

为了不让同一个会话的命中数加两次，`rurge-rules` 加一个不计数的探询 `RuleEngine::pre_refuses(session) -> bool`：同 `pre_match(session, PreMatchLayer::Syn)` 的判断，不加 `hits`、不留记录。虚拟网卡的会话在 SYN 时已经问过预匹配（阶段 3 / M3c），这一问对它们照例为假。

## 5. 先答后交（M1a）

### 5.1 SOCKS5

读完请求、组好会话信息之后：`sniffable` 为真时先写成功应答（与今天的字节相同：`BND.ADDR` / `BND.PORT` 是 `0.0.0.0:0`），再 `serve(session, 流, ServeOpts { stop: None, drop_hold: 监听器的 drop_hold, linger: 0 })`；写应答失败时直接结束（还没有句柄，没有记录）。为假时照今天：先拨号、再答成功或失败。

### 5.2 `CONNECT`

端口照旧必须写明（否则 400）。`sniffable` 为真时：在引擎的任务跟踪器上起任务等升级、升级之后 `serve(session, 升级之后的流, ServeOpts { stop: None, … })`，并立刻答 200；升级失败时结束（没有句柄，DEBUG 一行）。为假时照今天。

### 5.3 虚拟网卡

握手本来就先完成了。`session::dial_and_relay` 组好会话信息之后：`sniffable` 为真时 `serve(会话, 流, ServeOpts { stop: Some(life.session_stop()), drop_hold, linger })`，fake 地址的租约照旧握到服务结束；为假时照今天（拨号、转发与停止的竞速、`hold_in_silence`）。

### 5.4 `linger`

监听器给 0：停留之后关写端就放手，同今天监听器的"停到时限、关连接"。虚拟网卡给它的 `linger`（今天缺省 60 秒）：关写端之后读了就扔、至多等这么久等应用关它那一侧，与 `close_and_wait_for_the_app` 相同：应用在这段时间里关了它那一侧的，放手时不被重置；到时仍开着的，放手时被重置（`TunStream` 被丢掉时就这样做，`crates/rurge-tun/src/tcp/stream.rs`）。

## 6. 读与判（M1a，`rurge-http`）

### 6.1 前缀流

`Prefixed<S>`：读进来的字节留在缓冲里；`fill()` 再读一次、追加到缓冲；判完之后它作为普通的流交出去，读先给缓冲里的字节、再读底下的流，写与关写端直通。按原样转发时它就是 `relay` 的客户端一侧，第一次读给出整个前缀，一个字节也不丢。

### 6.2 判断

`judge(已读的字节) -> NeedMore | Tls(ClientHello) | Http | Other`，照总设计第 5.4 节的表：

- **TLS**：`0x16 0x03 0x00..=0x04` 开头、握手类型 `0x01`。先把连续的握手记录（类型 `0x16`）的片段拼起来，再从拼好的字节里解析握手消息：类型 1、24 位的长度，凑齐 4 + 长度个字节才解析。两步分开写：第二步 `parse_client_hello(握手消息)` 只吃握手字节，M5 从 QUIC 的 CRYPTO 帧里重组出的字节直接交给它。中途来了别的记录类型、长度不合理、读的总量超过 64 KiB，判为 `Other`。解析只取两个扩展：`server_name`（第一个 `host_name` 项；不是 ASCII 的当作没有；转成小写，规则要求小写）与 `application_layer_protocol_negotiation`（全部协议名）；其余字段只用来跳过。读不懂的 ClientHello 判为 `Other`。
- **HTTP**：1 ～ 20 个 `A-Z`、`-`、`_` 组成的方法记号后跟一个空格，且不是 `PRI`。字节不够、到目前为止都合格时是 `NeedMore`；第一个不合格的字节就是 `Other`。
- `PRI `（明文的 HTTP/2 先验）：`Other`，按原样转发。
- 其它：`Other`。

### 6.3 等待

`sniff(&mut Prefixed, wait) -> Sniffed`：`judge` 说 `NeedMore` 就再读，直到判出来、`wait` 到、客户端关闭写端或读出错。五种结局：

| 结局 | 去向 |
| ---- | ---- |
| `Tls(ClientHello)` | 第 7 节 |
| `Http` | M1a：按原样转发、协议 `HTTP`；M1b：HTTP 引擎 |
| `Other` | 按原样转发 |
| `Timeout` | 按原样转发 |
| `Closed`：读到 EOF 之前没判出来 | 按原样转发（已读的照样写给上游） |

读出错（客户端重置）时 `sniff` 返回 `Closed` 并带上"出错"，引擎不再拨号，什么也不记——这时还没有句柄，客户端已经不在了。

**等多久**：15 秒，定稿（总设计第 5.4 节的理由）。只供测试的选项：`EngineShared.sniff_wait`，`EngineShared::new` 给 15 秒，用例把它调短；不进命令行与环境变量。

### 6.4 `force-http-engine-hosts` 命中的会话

M1b 起，被它强制的会话只看是不是 TLS：不是 TLS 的（`Http`、`Other`、`Closed`）一律进 HTTP 引擎（hyper 解析不了时答 400 并关连接）；TLS 的照第 7 节按原样转发（解密只有 MITM 名单能定，M2）；`Timeout` 按原样转发——15 秒里一个字节也不发的多半是服务器先说话的协议。M1a 里它们与别的可嗅探会话一样处理（`Http` 也按原样转发）。

## 7. 嗅探之后（M1a）

### 7.1 `Engine::serve`

1. 包成 `Prefixed`，`sniff` 与停止竞速：`opts.stop` 被取消或引擎的会话根令牌被取消（守护进程退出），立刻返回——这时没有句柄，放掉流（虚拟网卡上是 RST）。
2. 组会话信息：TLS 时 `sni` 取 ClientHello 的、`protocol` 是 `HTTPS`；`Http`（M1a）时 `protocol` 是 `HTTP`；其余不设协议（同今天按原样转发的 TCP 会话，记录里 `protocol` 是 `null`）。
3. `self.dial(会话)`：今天的拨号原样不变（fake 地址反查、进程查找、建句柄、规则、策略、拨号、smart、`Trail`）。规则在这里第一次看到 SNI 与协议。拨号不与停止竞速（同虚拟网卡今天的做法：拨号握着会话的句柄，半途丢掉会让记录一直开着）。
4. 拨通：`relay(前缀流, 上游, 句柄)`，与停止竞速；停止时取消会话的令牌、再等转发结束（同今天的虚拟网卡）。
5. `Reject { kind: Drop }`：`hold_dropped(前缀流, opts)`（第 7.4 节）。别的拒绝与失败：放掉流（TCP 上是关闭，虚拟网卡上是 RST）。

### 7.2 规则看见什么

按原样转发的会话求一次规则，会话信息里已有：SNI（TLS 时）、协议（`HTTPS`、M1a 的 `HTTP`，或不设）、连接的目标。`extended-matching` 的域名规则因此比 SNI（`domain_targets`），`PROTOCOL,HTTPS` 在拨号之前就能命中；`PROTOCOL,TCP` 照旧按传输层匹配全部 TCP 会话（手册："`PROTOCOL,TCP` also matches HTTP, HTTPS"，`rules/protocol-and-network.html`）。进 HTTP 引擎的连接不在连接一层求规则（第 8 节）。

记录：`record_of` 的 `sni` 改成先取会话信息里的、没有时取句柄侧槽里的（`relay` 首块嗅探写的），这样完整读出的 SNI 不会被首块的 8 KiB 限制丢掉。`relay` 的首块嗅探不动：它仍给不可嗅探的会话（如 8443 上的 TLS）记 SNI。

### 7.3 规则集文件里一行上的 `extended-matching`（L314）

手册："Note that if any domain rule inside the set file already carries the flag, it is applied to all domain rules of that set."（`rules/ruleset.html`）。`CompiledSet::compile` 记一个整集的标志：集合里任何一行域名规则（`DOMAIN`、`DOMAIN-SUFFIX`、`DOMAIN-KEYWORD`、`DOMAIN-WILDCARD`）带了 `extended-matching`，整集的域名规则都带——域名索引里的 `DOMAIN` 与 `DOMAIN-SUFFIX` 行用 `extended || 整集`，逐条求值的域名行用 `extended || 整集 || 这一行`。`DOMAIN-SET` 的行没有参数，照旧只看引用它的那一行。`rurge rule match --sni` 的结果随之改变（清单 L314 记下的那个试法）。

### 7.4 REJECT-DROP 的停留

`hold_dropped(流, opts)`（`rurge-inbound`，引擎与 `serve` 的缺省实现共用）：读了就扔，直到读到 EOF 或出错、停止、或 `drop_hold` 到；之后关写端，再读了就扔至多 `linger`，然后放手；停止时立刻放手。

对虚拟网卡上可嗅探的会话，这与今天的 `hold_in_silence`（"谁也不读不写"，应用重置或协议栈的定时器结束连接时提前结束）有两处不同：应用发来的照收照扔（接收窗口不会填满），应用关它那一侧（FIN）时提前结束。两处都是"握着的东西更少"的一侧，记进第 13 节。不可嗅探的会话照旧用 `hold_in_silence`。

### 7.5 计数

引擎里五个计数（原子量），每次嗅探结束加一个：`tls`、`http`、`other`、`timeout`、`closed`（含读出错）。经 `GET /v1/rurge/http` 读（第 11 节）。

## 8. HTTP 引擎（M1b）

### 8.1 连接

`serve` 判为 HTTP（与第 6.4 节被强制进来的）时，引擎为这条客户端连接准备：`turn_back`（fake 地址反查回名字，租约握到连接结束；反查不到时承载连接记一条失败 `fake address not known`、关连接）、进程查找一次（`look_up_owner`，连接上所有请求共用这一个进程槽）、`quic_adaptive.on_tcp` 一次、连接的令牌（会话根令牌的孩子）、一个不登记的承载连接句柄（第 10.2 节），再把前缀流交给 `rurge-http` 的 HTTP/1.1 服务：

- hyper `server::conn::http1`：`preserve_header_case(true)`、`timer(TokioTimer)`、`header_read_timeout(idle_timeout)`、`keep_alive(true)`、`with_upgrades()`；`half_close` 保持缺省的关。
- `idle_timeout` 是今天转发的空闲时限（`--idle-timeout`，缺省 600 秒）。hyper 的这个时限从开始读一个头算起，空闲的 keep-alive 连接也算，所以它同时是客户端一侧 keep-alive 的空闲上限：与同一条连接按原样转发时一样长。
- M1 只有 HTTP/1.1：明文 HTTP/2 先验照原样转发（第 6.2 节），解密之后的 HTTP/2 在 M2。

### 8.2 每个请求的流程

```
请求头读完
→ 组会话信息（第 8.3 节）
→ Upstreams::route：建句柄、求规则、选策略            —— REJECT 一族与失败在这里答复（第 8.8 节）
→ 池里有键相同、可用的连接 → 借；没有 → Upstreams::dial（第 8.4、8.5 节）
→ 改写请求行与逐跳的头（第 8.7 节），发出；正文流式
→ 应答头到达：记状态码；改逐跳的头
→ 应答正文流式交给客户端；读完 → 句柄结束、连接放回池里
```

正文缺省流式（hyper 的正文有背压）。M3 的重写插在"发出"之前与"应答头到达"之后（总设计第 6.2 节），本里程碑不留空的钩子。

### 8.3 每个请求的会话信息

由 `rurge-http` 组（总设计第 6.3 节）：

- `src`、`in_port`、`local`、`listener`、`transport`、`sni` 沿用连接的；`protocol` 是 `HTTP`。
- **目标是连接的目标**（`dst_host`、`dst_port` 不变）。例外只有经 HTTP 监听器来的绝对形式请求：目标是 URL 的主机与端口（第 9 节）。
- `url`：绝对形式取请求行；源站形式用 `http://` + `Host` 头 + 路径与查询串重建，没有 `Host` 时用连接的目标；都去掉 userinfo。
- `http_host`：`Host` 头的主机部分（不带端口），转成小写；没有时不设。
- `user_agent`：`User-Agent` 头，照原样。
- 方法不进 `SessionInfo`（规则不看它）：放在句柄的侧槽里，进记录与日志（第 10 节）。

`SessionInfo.url` 上"`listener == Http` 且 `url` 有值就是监听器在转发明文请求"的注释（`crates/rurge-config/src/session.rs` 第 74 ～ 79 行）改写：以后每个引擎里的请求都有 URL，以绝对形式交给 HTTP 代理与否由引擎显式给出（第 8.4 节），不再从这两个字段推断。

### 8.4 选路与拨号

`Engine::connect_session` 拆成两步，今天的会话照旧两步连着走：

- **选路** `route_session`：暂停的检查、`choose_policy`（规则在这里）、`resolve_ready`、记策略链与说明、目标与 `use-local-host-item-for-proxy` 钉住的地址、`may_forward`、smart 组的尝试名单（`smart::attempts`）。
- **拨号** `dial_route`：尝试的循环与今天一样——`mark_connected`、`book.used`、`smart::watch`、`Trail` 记下的对端地址与 DNS 耗时，失败的映射与升级（`escalation`）也一样。

`may_forward` 不再由 `listener == Http && url.is_some()` 推断：按原样转发的会话恒为假（它们是隧道）；引擎里的请求是 `http://`、没有被 `[Host]` 钉住地址、且不是"嗅探到的连接上客户端对它自己的代理发的绝对形式请求"（第 8.9 节）时为真——这时策略是 HTTP 代理就以绝对形式交给它（`Dialed.forward` 的那组头照旧在拨号时渲染一次）。

引擎实现 `Upstreams` 的是**每条客户端连接一个对象**（握着 `Arc<Engine>`——引擎里已有的 `me: Weak<Engine>` 升级而来——、连接的令牌、进程槽、连接的会话信息）：

- `route`：快照这一代与它的注册表（`snapshot()`）；建请求的句柄：编号照旧、令牌是**连接令牌的孩子**、进程槽是连接的、结束时照旧写会话日志与记录（`new_handle_with` 加一个给定父令牌的变体）；`route_session`；算出池键。
- `dial`：在请求自己的任务里、在句柄的 `Trail` 之下调 `dial_route`（`Trail` 的作用域是任务局部的），把 `Trail` 记下的对端地址随流交回，池里的连接存着它。
- 借到池里的连接时由 `rurge-http` 做两件事：`handle.mark_connected()`；`handle.trail().set_peer(那条连接拨号时的远端地址)`（新方法，`rurge-net`）。smart 组什么也不告诉（`book.used` 与 `smart::watch` 只属于真拨号：借来的连接上一个请求的成败说的是那条早就连上的连接，不是成员现在连不连得上）。

### 8.5 上游连接池

`rurge-http` 的 `Pool<K>`，**属于一条客户端连接**（总设计 P4-D6）：

- **键**由引擎给（`Upstreams::Key`，只要求 `PartialEq`）：这一代（`Arc<Runtime>` 按指针）、注册表（按指针）、第一个尝试的出站（按指针，连同它的策略链）、目标（主机、端口，钉住的地址照算）、`may_forward`、出口表（`SocketHook::egress_route()` 给的 `Route`，没有时 `None`）。重载换了代或注册表、`select` 换了成员、出口表变了，旧连接的键就不再相等，不再借出（CLAUDE.md「出站套接字只有一处建法」对活得比一次会话长的套接字的要求）。键里的 `Arc` 让旧的一代多活到这些空闲连接被关掉为止。
- **取**：按键线性找（至多 4 条），跳过并关掉 `is_closed()` 的，取第一条 `is_ready()` 的。
- **还**：应答正文读完、hyper 的 `SendRequest` 没有关闭时放回；正文没读完（客户端走了、出错）、升级了的、任一侧说了 `Connection: close` 的不放回（hyper 的客户端遇到 `close` 会自己关，`is_closed()` 为真）。
- **上限**：每条客户端连接至多 4 条空闲连接，多了关最早放回的；空闲超过 90 秒的在下一次取或还时关掉（不另设定时器；客户端连接自己在空闲时限到时连同池一起结束）。
- 上游连接的 hyper 客户端任务在引擎的任务跟踪器上起，`SendRequest` 被丢掉或对端关闭时结束。
- 上游的字节计数：池里的连接包一层计数，"当前请求"可换——HTTP/1.1 上一条连接同一时刻只给一个请求用，借出时换成借它的请求的句柄、还回时清空；读到字节时 `mark_first_byte`。升级之后清空，字节由 `relay` 计（第 8.9 节）。
- `TCP_NODELAY`：上游的套接字由引擎经 `rurge-net` 的统一建法建（`new_socket`），任务 0 核它是不是已经开着；没开的话在建法里开，池不自己建套接字。

### 8.6 重发

hyper 的 `try_send_request` 在请求还没写出时把它还回来（`TrySendError::take_message`）：

- 借来的连接、请求**还没写出**就失败：换一条新拨的连接重发一次，任何方法都行（一个字节也没发出去）。
- 借来的连接、请求**已经写出**、应答头到达之前就"消息不完整"（hyper 的 `is_incomplete_message`）：只有幂等的方法（`GET`、`HEAD`、`OPTIONS`、`TRACE`、`PUT`、`DELETE`，RFC 9110 第 9.2.2 节）、没有请求正文时重发一次。这一条是 rurge 自己的：hyper-util 的 `retry_canceled_requests` 只重发还没写出的请求（上一条），hyper 也不把写出了的请求还回来（`TrySendError::take_message` 只给还没写出的），所以引擎在借来的连接上发没有正文的幂等请求时自己留一份请求头（方法、URI、版本、头）。
- 其余（新拨的连接上失败、非幂等、有正文、第二次又失败）：502。

### 8.7 头

- 逐跳的头两个方向都去掉：今天 `rurge-inbound` 的名单与做法（`Connection` 与它列出的名字、`Keep-Alive`、`Proxy-Connection`、`TE`、`Trailer`、`Transfer-Encoding`、`Upgrade`、`Proxy-Authenticate`、`Proxy-Authorization`）搬到 `rurge-http`，改两处：升级的请求（第 8.9 节）保留 `Connection: upgrade` 与 `Upgrade`；`Proxy-Authorization` 只在经 HTTP 监听器来的请求上去掉（监听器消费了它），嗅探到的连接上原样转发（那是给客户端自己的代理的，总设计第 6.5 节）。
- `Host`：源站形式的请求照客户端写的转发；经监听器来的绝对形式请求照今天：交给源站时 `Host` 换成目标的主机与端口（RFC 9112 第 3.2.2 节），以绝对形式交给 HTTP 代理时照今天的 `absolute_form`。
- 不加 `Via`、`X-Forwarded-For`。头名的拼写：两侧都开 `preserve_header_case`，原拼写原样转出（总设计第 6.5 节）。
- `rurge-inbound` 的 `ResponseBody` 是 `BoxBody<Bytes, hyper::Error>`，引擎自己的正文包装（第 8.10 节的空闲时限）要能报自己的错：错误类型放宽成 `Box<dyn std::error::Error + Send + Sync>`，错误页与 200 应答随之改 `map_err`。

### 8.8 拒绝与失败

| `route` 的结果 | 答复（所有入口一样） |
| -------------- | -------------------- |
| `REJECT`、`REJECT-NO-DROP` | `show-error-page-for-reject` 开着时 403 错误页，否则关连接（手册："Show an error webpage for the REJECT policy if the request is a plain HTTP request."；今天 HTTP 监听器的做法） |
| `REJECT-TINYGIF` | 200 与 1 像素的 GIF（今天的做法） |
| `REJECT-DROP` | 停到 `drop_hold` 再关连接；请求的令牌被取消时提前结束。客户端先关、且请求正文已读完时，hyper 丢掉在途的服务 future（`mid_message_detect_eof`），守卫结束句柄（第 8.11 节）；请求正文没读完时 hyper 不再读这条连接，停到时限 |
| 失败（DNS、连接、`fake address not known`） | `show-error-page` 开着时 502 错误页（今天的 `failure_response`：标题 `Connection failed`，按 `FailKind` 的三句），否则关连接 |
| 等应答头超时 | `show-error-page` 开着时 504 错误页（标题 `Gateway timeout`），否则关连接 |

`show-error-page` 与 `show-error-page-for-reject` 读自这个请求选路时的那一代。

### 8.9 升级与隧道里的 `CONNECT`

- 带 `Upgrade` 的请求（WebSocket）照普通请求走完选路与拨号，上游答 101 时：在拆开请求之前取客户端一侧的 `hyper::upgrade::on(&mut 请求)`、在拆开应答之前取上游一侧的，在引擎的任务跟踪器上起任务等两侧升级完成、`Upstreams::relay`（今天的 `relay::pump`：空闲时限、计数、结局）；101 交给客户端。这条上游连接不回池。
- 嗅探到的连接上客户端发 `CONNECT`（客户端在对它自己的代理说话，连接的目标就是那个代理）：`CONNECT` 照原样转给连接的目标，2xx 之后同上透传；目标不是 2xx 时照原样把应答交给客户端。这样的连接上的绝对形式请求同样照原样（绝对形式）转给连接的目标，`may_forward` 为假。
- 升级之后的转发计入这个请求的记录；池连接的计数层在升级时清空当前请求，免得字节记两次。

### 8.10 时限

| 时限 | 值 | 到了 |
| ---- | -- | ---- |
| 客户端一侧读一个请求头（含 keep-alive 的空闲） | 嗅探进引擎的连接：`idle_timeout`（缺省 600 秒）；经 HTTP 监听器来的：照旧 `handshake_timeout`（缺省 30 秒，第 9 节） | hyper 关连接 |
| 拨号 | 今天的 `CONNECT_TIMEOUT` | 502 |
| 等上游的应答头 | `idle_timeout` | 504 |
| 请求与应答的正文：两帧之间 | `idle_timeout` | 关这条客户端连接与这条上游连接，句柄以失败结束 |
| 池里的空闲连接 | 90 秒 | 关 |

### 8.11 终止、停止与守卫

- 每个请求的句柄的令牌是连接令牌的孩子。`POST /v1/requests/kill` 终止一个请求：取消它的令牌；HTTP/1.1 上这关掉这条客户端连接（hyper 在一条连接上只能中止整条）。终止连接令牌（虚拟网卡停止、守护进程退出）终止连接上所有的请求。
- 每个请求的 future 带一个守卫：future 被丢掉（hyper 在客户端走了时丢掉它）而句柄还没结束时，守卫照今天 `forward()` 的 `handled` 守卫结束句柄（被终止的算终止，否则算失败），记录不会一直开着。
- 停止：取消连接令牌；hyper 的连接 `graceful_shutdown()`、再等它结束。请求的 future 在拨号之外的每一步都与令牌竞速；拨号不竞速（同第 7.1 节），拨完看到令牌已取消就结束。

## 9. HTTP 监听器的绝对形式请求（M1b）

今天 `forward()` 每个请求拨一次号、不复用、去掉 `Upgrade`。以后：

- 每条客户端连接开始时，监听器问 `dialer.plain_requests(ClientConn { src, in_port, local })`，引擎给一个这条连接专用的对象（同第 8.4 节的每连接对象：一个池、一个进程槽——经代理端口来的同一个套接字，进程查一次——、连接的令牌）；`PlainRequests::handle(请求) -> Result<应答, Close>`。缺省 `None`：监听器答 502（只有测试的假拨号器会这样）。
- 监听器只剩：鉴权（`Proxy-Authorization` 验过就去掉，不往上游转）、`CONNECT`、拒绝不是 `http://` 的绝对形式请求（400，照今天）。
- 目标是 URL 的主机与端口（缺省 80）。主机是 fake 地址时，`route` 先反查回域名：请求行的主机、`Host` 头里的同一个地址、随之的 `url` 与 `http_host` 都换成名字（总设计第 6.8 节；它第 2 节的表里 M4-D24 那一行："引擎把请求行与 `Host` 里的 fake 地址换回域名"），再求规则、拨号与转发，租约握到这个请求结束（R27，阶段 3 / M4 设计 M4-D24 留给本阶段；今天只换了 `dst_host`，见 `docs/status.md` 阶段 3 的 M3c 一节）；反查不到：502，`fake address not known`。
- 客户端一侧读请求头的时限照旧是监听器自己的 `header_read_timeout(handshake_timeout)`（缺省 30 秒，`Engine::listener_opts`），它也是这条路上 keep-alive 的空闲上限；第 8.1、8.10 节的 `idle_timeout` 只说嗅探进引擎的连接。代理端口是"前门"，慢慢发头的客户端照今天 30 秒就断开。
- 走第 8.2 节的同一条流程：每个请求一条记录、池、REJECT 一族与错误页、WebSocket（今天被去掉的 `Upgrade` 以后能用）。
- `forward()`、`absolute_form`、`origin_form` 与它们在 `rurge-inbound` 里的用例退役：用例照它们断言的行为在引擎一层重写（源站形式与 `Host`、以绝对形式交给 HTTP 代理与那组头、HTTP 代理答 407、逐跳的头、错误页、终止）。

## 10. 请求记录（M1b）

### 10.1 每个请求一条

记录（`RequestRecord`）与 `RequestJson` 加三项，紧跟 `protocol`：`method`（方法名）、`url`（去掉 userinfo，含查询串，与 Q1 相交，第 19 节）、`statusCode`（上游应答的状态码；引擎自己答的拒绝与错误页是它答的码；没有应答时 `null`）。不是 HTTP 请求的会话三项都是 `null`。

- `remoteAddr`：真拨号的照旧取 `Trail`；借来的连接是那条连接拨号时记下的地址（第 8.4 节）——这解决 `docs/api/phase1.md`「`remoteAddr`」一条登记的"复用已有连接的会话没有"在 HTTP 上的那一半。
- `up` / `down`：这个请求在所用上游连接上写出与读到的字节（请求头与正文、应答头与正文；升级之后的转发也算），与按原样转发的会话一样只数上游一侧。客户端一侧的头不计入任何一条（登记）。
- `connectMs`：真拨号照旧；借来的连接是从请求开始到借到为止。`firstByteMs`：从就绪到读到第一个应答字节。
- `listener` 照旧是入口（`http`、`socks5`、`tun`）。

### 10.2 承载请求的连接

进引擎的客户端连接自己的句柄建好时**不登记**（不在活动列表里、结束时不自动写日志与记录）。连接结束时，**一个请求也没有走到选路**（hyper 读不出请求头答了 400、请求头超时、客户端发了半个头就关、fake 地址反查不到）才补写这一条：结局是失败、原因照实写，会话日志照今天的级别（失败的在 INFO）。走到过选路的连接不另记：它的请求各有记录（总设计第 9.1 节）。

### 10.3 不变的

记录的容量照旧是 `--request-log-size`；按原样转发的会话的记录照旧一个连接一条；`GET /v1/traffic` 的总量照旧由记录的 `up` / `down` 加起来（HTTP 的请求同样是会话）。

## 11. 计数与 API

`GET /v1/rurge/http`（rurge 专有，`/v1/rurge/…` 是总设计给 rurge 专有接口定的前缀；形状写进 `docs/api/phase4.md`）：

```json
{"sniffed":{"tls":0,"http":0,"other":0,"timeout":0,"closed":0},"requests":0}
```

M1a 有 `sniffed`，M1b 加 `requests`（走到选路的请求数）；M2 起加解密与握手失败，M3 加缓冲放弃。自进程启动起累计，重载不清零。

## 12. 日志与个人信息

- 每个请求一行会话日志（今天的 `log_session`），级别照旧：完成的在 DEBUG，被拒与失败的在 INFO；字段照旧，再加 `method` 与 `status`（状态码）。路径只在 DEBUG 的行里，写成 `scheme://主机[:端口]/路径`，**不写查询串与 userinfo**；请求头与正文在任何级别都不进日志（总设计 P4-D25）。
- 嗅探不另写日志：结局在计数里，协议与 SNI 在记录与会话日志里。`CONNECT` 升级失败、SOCKS5 写应答失败（都还没有句柄）各一行 DEBUG，不带地址。
- 虚拟网卡上的名字与地址照旧只在 DEBUG；进程与网络的信息照旧只在 DEBUG（CLAUDE.md「日志的分级」）。
- 凭据：没有新的带凭据的字段。`Proxy-Authorization` 照旧不进日志、错误与 API。

## 13. 行为的变化（登记）

照总设计第 18 节的去向登记进清单（第 15 节）：

1. **先答后读**：可嗅探的 SOCKS5 与 `CONNECT` 会话拨号失败时，客户端拿到一个随即关闭的隧道，不再是 `0x02` / `0x04` / `0x05` 应答码或错误页（总设计风险 A）。预匹配会拒、引擎暂停时不嗅探，那两种拒绝照旧在答复之前（第 4 节）。
2. **15 秒**：80、443 与 `force-http-engine-hosts` 命中的端口上，服务器先说话的协议晚 15 秒才通（总设计风险 B）。
3. **虚拟网卡上可嗅探会话的 REJECT-DROP**：应用发来的照收照扔，应用关它那一侧时提前结束（第 7.4 节）。
4. **HTTP 监听器**：上游连接复用（每条客户端连接一个池）；WebSocket 经它能用；进程每条客户端连接查一次（以前每个请求一次）；等应答头超时答 504（以前没有这个时限）。
5. **记录**：经引擎的明文 HTTP 一个请求一条，不再一条连接一条；记录的 `up` / `down` 只数上游一侧。
6. **hyper 的限制**（总设计第 6.9 节）：头的顺序、记录里头名的原拼写、1xx 不转发（101 除外，`100 Continue` 由 hyper 答）、折行头与非 UTF-8 的请求目标答 400、非 ASCII 的原因短语变空、trailers 只在声明过时转发、被短路的请求在客户端还在上传时关连接。

## 14. 测试

沿用的规矩（`CLAUDE.md`「测试约定」与 `docs/testing.md`）：只用回环与端口 0；失败时也不离开回环与临时目录；缺省的 `cargo test` 不改系统设置、不建网卡；有界等待；固定等待只许"什么都没发生"、注释写等的是什么与余量；新写的计时用例提交前单独循环跑二十次以上；起 `rurge` 的用例经 `rurge_command()` / `rurge_process()` 并给 `--data-dir`。

1. **单元**（`rurge-http`）：前缀流的回放（多次 `fill`、读的缓冲比前缀小）；ClientHello：用 rustls 的客户端（`ClientConnection::write_tls`）产生真实的 ClientHello，单个记录、切成任意块喂进去、拆成几个记录、大于一个记录（用例里手工造，带一个填充扩展：本机的 rustls 0.23.43 只开了 ring，它的密钥交换组只有 X25519、P-256 与 P-384，没有 ML-KEM，rustls 产生的 ClientHello 只有几百字节）、超过 64 KiB、中途来了别的记录类型、SNI 是大写、没有 SNI、ALPN 几种；HTTP 开头：每个方法、`PRI`、小写、21 个字符、只来了一半；`sniff` 的五种结局（超时用调短的时限）。池：键、上限、`is_closed`、90 秒（用可控的时刻）。
2. **规则**（`rurge-rules`）：L314 的三种行（索引里的 `DOMAIN` / `DOMAIN-SUFFIX`、逐条的 `DOMAIN-KEYWORD`）与"一行带、整集带"；`pre_refuses` 不加命中数。
3. **端到端**（引擎，M1a）：客户端经 SOCKS5、`CONNECT`、内存设备上的虚拟网卡进来，连到回环上的 TLS 服务器（`TlsFixture`）、明文服务器与"服务器先说话"的服务器：带 `extended-matching` 的 `DOMAIN` 规则按 SNI 命中（规则把它送到第二个上游，以此判断）；`PROTOCOL,HTTPS` 在拨号之前命中（REJECT，客户端看到隧道被关）；`always-raw-tcp-hosts` / `-keywords` 让会话照今天的路走（SOCKS5 的应答在拨号之后才来）；`force-http-engine-hosts` 让 8080 上的明文进嗅探；预匹配与暂停时照今天拒绝；服务器先说话的协议在调短的时限之后通；前缀一个字节不丢（回显）；记录里的 SNI 与协议；计数。
4. **端到端**（引擎，M1b）：四种入口（加 HTTP 监听器的绝对形式）上：一条客户端连接上的两个请求按 `URL-REGEX` 各走一条策略；同一个键的请求复用上游连接（数连接的测试服务器：`TestServer` 今天不数连接，计划里加一个数接受次数的回环服务器）；借来的连接的 `remoteAddr`；REJECT 一族逐请求的四种答复；502、504 与错误页的两个设置；WebSocket 经回显服务器往返；chunked 的请求与应答；`Expect: 100-continue`；重载与出口表变化之后不借旧连接；重发的两种情形；终止一个请求；承载连接的记录（发半个头就关）。
5. **只供测试的选项**：`EngineShared.sniff_wait` 调短；"什么都没发生"的等待照规矩写注释。
6. **现有的用例**：嗅探在 M1a 任务 6 生效之后，经 SOCKS5、`CONNECT` 与虚拟网卡连 80、443 的现有用例里，讲"拨号之后才答"的（应答码、错误页、拒绝时的重置）与客户端不先发字节就等结果的，改用不嗅探的端口（8080、8443），断言不改：它们讲的是不可嗅探的会话照今天走的那条路。客户端先发字节的照过。先答后读的新行为由第 3 条的新用例断言。M1b 同样处理"一条 `CONNECT :80` 隧道一条记录"一类的用例（M1b 的计划点名）。

## 15. 文档

| 文档 | 改动 | 时机 |
| ---- | ---- | ---- |
| `docs/surge-compatibility-matrix.md` | L131（`force-http-engine-hosts` 的缺省端口在运行时生效）；L172 ～ L174（三个设置生效，写 rurge 的做法：关键字不分大小写、只比名字）；L265（SNI 在规则之前，`Host` 在引擎的请求上）；L314（做完）；7.1 节各行（嗅探、引擎、"明文 HTTP 代理转发"一行改写：池、WebSocket、504）；第 13 节的各条登记；附录的统计随改动的行 | M1a、M1b 各自收尾 |
| `docs/api/phase4.md` | 新建：`GET /v1/rurge/http`；记录的三个新键与 `remoteAddr`、`up` / `down` 在 HTTP 请求上的含义；每个请求的日志行在哪一级说什么 | M1a 建，M1b 加 |
| `docs/api/phase1.md` | 「`remoteAddr`」一条里"复用已有连接的会话没有"补一句：经引擎的 HTTP 请求借来的连接有（指到 `phase4.md`） | M1b |
| `docs/testing.md` | M1 的用例过滤词；`sniff_wait` 的测试选项；数连接的测试服务器 | M1a、M1b |
| `README.md`、`README_en.md` | 状态、特性表、路线图（两份一致） | M1b 收尾 |
| `docs/requirements.md` | 第 7 节阶段 4 的状态一句；FR 的字面不改（阶段栏是总设计 Q6，归项目所有者） | M1b 收尾 |
| `docs/status.md` | 阶段 4 下新的 M1a、M1b 两节（做了什么、没做的、必须知道的事、去向写着项目所有者的） | 各自收尾 |
| `CLAUDE.md` | 「现在在哪里」 | 各自收尾 |
| `docs/README.md` | 本文与两份计划各一项 | 随本文与计划 |

## 16. 写计划时要核对的事项

| 编号 | 核对什么 |
| ---- | -------- |
| V1 | `Dialer` 的全部实现（引擎、`rurge-inbound` 的 `FakeDialer` 与 `UdpEcho`、`rurge_tun::testing::FakeDialer`、`rurge-tun` 用例里的 `Ends` 与 `Gated`）：三个新方法都有缺省实现，它们不用改 |
| V2 | SOCKS5 的成功应答的字节（今天是 `[5,0,0,1,0,0,0,0,0,0]`）；`CONNECT` 的升级与任务跟踪器的写法；`ListenerOpts` 的 `drop_hold` |
| V3 | `rurge-tun` 的 `dial_and_relay`、`Life::session_stop`、`Holds` 与 fake 地址的租约在哪里握着 |
| V4 | `FakeIpPool::name_of` 与 `lease` 的签名；引擎拿到 fake 池的路径；`turn_back` 的返回值（租约）与它对 `Tun` 的不做 |
| V5 | `RuleEngine::pre_match` 的写法，`pre_refuses` 照它不加 `hits`；`Engine::mode()` 与 `is_suspended()` |
| V6 | `CompiledSet::compile` 与 `eval` 里域名索引与逐条两处取 `extended` 的地方；`set.rs` 现有的那条用例（第 1246 ～ 1272 行） |
| V7 | `HostList::matches` 的返回值与排除项；`always_raw_tcp_keywords` 的类型 |
| V8 | rustls 的客户端怎样在测试里产生 ClientHello 的字节（`ClientConnection::new` + `write_tls`），缺省的密钥交换组与 ClientHello 的大小 |
| V9 | `EngineShared` 的构造处（`EngineShared::new` 与用例的夹具），`sniff_wait` 加在哪里不碰别的用例 |
| V10 | `record_of` 的 `sni` 与 `protocol`；`RequestJson` 的键序与 `docs/api/phase1.md` 的例子 |
| V11 | `connect_session` 拆开之后两步之间要传的东西（`Resolution`、目标、钉住的地址、`may_forward`、尝试名单、smart 的书）；`new_handle_with` 与 `SessionHandle::new_with_slot` 的令牌参数 |
| V12 | `SocketHook::egress_route()` 在引擎里怎样拿到；`new_socket` 有没有开 `TCP_NODELAY` |
| V13 | hyper 1.11.1 的 `server::conn::http1::Builder` 与 `client::conn::http1` 的接口（`graceful_shutdown`、`with_upgrades`、`try_send_request`、`TrySendError`）；`hyper::upgrade::on` 对请求与应答两侧 |
| V14 | `responses.rs` 的全部用处（`ResponseBody` 换错误类型）；`failure_response` 的三句 |
| V15 | `TestServer`、`TlsFixture` 的接口；数连接的回环服务器放在 `rurge-net::testing` 还是引擎的夹具 |
| V16 | 引擎的任务跟踪器（`tracker()`）与会话根令牌（`sessions_root`）的可见性 |

## 17. 对总设计的订正

1. 第 5.2 节的条件加两条：预匹配会拒、引擎暂停时不嗅探（第 4.1、4.3 节，M1-D2）。`always-raw-tcp-keywords` 不分大小写、只比名字（第 4.2 节）。
2. 第 5.4 节"值在 M1 的细化设计里定稿" → 15 秒；只供测试的选项是 `EngineShared.sniff_wait`（第 6.3 节，M1-D5）。第 5.4 节的四种结局加第五种"客户端关闭写端之前没判出来"（`Closed`），读出错时不拨号（第 6.3 节）。
3. 第 5.5 节"别的、超时：协议是 `TCP`" → 不设协议，同今天按原样转发的 TCP 会话（记录里是 `null`；`PROTOCOL,TCP` 照旧按传输层匹配）（第 7.1 节）。
4. 第 5.5 节"HTTP/1.x → HTTP 引擎"在 M1a 里先按原样转发、协议记 `HTTP`，M1b 才进引擎（第 6.3 节）。
5. 第 6.1 节请求头读取的时限 → `idle_timeout`（缺省 600 秒），同时是 keep-alive 的空闲上限（第 8.1 节）。第 6.7 节"客户端一侧 keep-alive 的空闲时限、等上游应答头的时限在 M1 定" → 第 8.10 节的表。
6. 第 6.4 节"每条客户端连接至多留几条空闲的上游连接（M1 定）" → 4 条、空闲 90 秒；"smart 组对借来的连接上的请求怎样记，M1 的细化设计读 smart 的代码定" → 什么也不告诉（第 8.4、8.5 节，M1-D14、D15）。
7. 第 6.4 节"幂等的请求在'借到的空闲连接一写就断'时重发一次" → 分两种：还没写出的任何方法都重发一次；写出了的只有幂等、没有正文的重发一次（第 8.6 节，M1-D16）。
8. 第 6.7 节 REJECT-DROP"M1 的细化设计找办法……找不到就照旧固定停到时限" → 找到了一半：请求正文读完时客户端先关会结束停留（hyper 的 `mid_message_detect_eof`）；没读完时停到时限（第 8.8 节）。
9. 第 6.8 节"以绝对形式交给 HTTP 代理……只对 `http://` 的请求、策略是 HTTP 代理时用" → 再加两个条件：没有被 `[Host]` 钉住地址（今天就这样）、不是嗅探到的连接上客户端对它自己的代理发的绝对形式请求（第 8.4、8.9 节）。
10. 第 9.1 节"加上……请求与应答的正文字节数" → 正文字节数随 M4；M1 的 `up` / `down` 是上游一侧的全部字节（含头）。"握手与头的开销不计入任何一条" → 上游一侧的头计入请求的 `up` / `down`，客户端一侧的不计（第 10.1 节，M1-D23）。
11. 第 13 节"计数（在 `GET /v1/traffic` 或新端点里，M1 定）" → `GET /v1/rurge/http`（第 11 节，M1-D24）。
12. 第 4 节 `rurge-inbound` 一行"`Dialer` 加'把客户端的流整个交给引擎'的入口" → 三个方法 `sniffable`、`serve`、`plain_requests`（第 3 节）。

## 18. 已决事项

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| M1-D1 | 计划与执行 | 一份设计、两份计划（M1a 嗅探与规则之前的 SNI → M1b 明文 HTTP 引擎）；协调的会话逐个任务调 Pi、逐个任务评审；没有平台代码、没有等 CI 的停点；M1b 做完请所有者推送一次、读 CI（第 2 节） |
| M1-D2 | 可嗅探的判定 | TCP、经三种入口；不暂停；预匹配不拒；不命中 `always-raw-tcp-*`；端口 80 / 443 或命中 `force-http-engine-hosts`；fake 地址先 `name_of`（第 4.1 节） |
| M1-D3 | 预匹配的探询 | `RuleEngine::pre_refuses`，同 `Syn` 层的判断、不计数；拒时不嗅探、照今天拒（第 4.3 节） |
| M1-D4 | 先答后交 | SOCKS5 的应答字节不变；`CONNECT` 先答 200、升级之后交；虚拟网卡不变；监听器的 `linger` 为 0（第 5 节） |
| M1-D5 | 等待 | 15 秒；`EngineShared.sniff_wait` 只供测试调短（第 6.3 节） |
| M1-D6 | 判断 | 完整的 ClientHello（记录重组与握手解析分开、≤ 64 KiB、SNI 取第一个 `host_name`、转小写、不是 ASCII 当作没有）；方法记号加空格、`PRI` 除外；读不懂的判为别的（第 6.2 节） |
| M1-D7 | 判完之后 | TLS 按原样转发、带 SNI 与 `HTTPS`；HTTP 在 M1a 按原样转发、M1b 进引擎；别的、超时、关闭按原样转发、不设协议；读出错时不拨号；`force-http-engine-hosts` 强制的非 TLS 进引擎、超时按原样转发（第 6.3、6.4 节） |
| M1-D8 | `serve` | 嗅探与停止竞速；今天的 `dial` 原样做；拨号不竞速；转发与停止竞速（第 7.1 节） |
| M1-D9 | 记录里的 SNI | 先取会话信息的、再取侧槽的；`relay` 的首块嗅探不动（第 7.2 节） |
| M1-D10 | L314 | 整集的标志：一行带、整集的域名规则都带；`DOMAIN-SET` 照旧（第 7.3 节） |
| M1-D11 | 嗅探之后的 REJECT-DROP | `hold_dropped`：读了就扔到 EOF、出错、停止或 `drop_hold`，再关写端、读了就扔至多 `linger`；虚拟网卡上的两处不同登记（第 7.4 节） |
| M1-D12 | 引擎的连接 | hyper HTTP/1.1；`header_read_timeout` = `idle_timeout`；保留头名拼写；升级；连接一层反查 fake 地址、查进程、通知 `quic_adaptive` 各一次（第 8.1 节） |
| M1-D13 | 每个请求的会话信息 | 目标是连接的目标（监听器的绝对形式除外）；URL 重建；`http_host` 小写；方法在句柄的侧槽；`url` 一段注释改写（第 8.3 节） |
| M1-D14 | 池 | 每条客户端连接一个；引擎给的键（代、注册表、第一个尝试的出站、目标、`may_forward`、出口表）；至多 4 条空闲、90 秒；`is_ready` / `is_closed`；升级的不回池；上游的字节记到当前请求（第 8.5 节） |
| M1-D15 | 选路与拨号 | `connect_session` 拆成 `route_session` 与 `dial_route`；`may_forward` 显式；借来的连接 `mark_connected` 与 `Trail::set_peer`，smart 什么也不告诉（第 8.4 节） |
| M1-D16 | 重发 | 还没写出的一次、任何方法；写出了的只有幂等且没有正文的一次；其余 502（第 8.6 节） |
| M1-D17 | 头 | 名单从 `rurge-inbound` 搬来；升级的请求保留 `Upgrade`；`Proxy-Authorization` 只在监听器的请求上去掉；`Host` 照今天；不加 `Via` / `X-Forwarded-For`；`ResponseBody` 的错误类型放宽（第 8.7 节） |
| M1-D18 | 拒绝与失败 | 所有入口一样：REJECT 看 `show-error-page-for-reject`、TINYGIF 给 GIF、DROP 停留、失败 502、等应答头超时 504，后两者看 `show-error-page`（第 8.8 节） |
| M1-D19 | 升级与隧道 | 101 / 2xx 之后两侧升级、跟踪器上的任务里 `relay`；隧道里的 `CONNECT` 与绝对形式请求原样转给连接的目标（第 8.9 节） |
| M1-D20 | 时限 | 第 8.10 节的表 |
| M1-D21 | 终止与停止 | 请求的令牌是连接令牌的孩子；终止一个请求关掉这条 HTTP/1.1 连接；守卫结束被丢掉的请求；停止时 `graceful_shutdown`，拨号不竞速（第 8.11 节） |
| M1-D22 | HTTP 监听器 | `Dialer::plain_requests` 每条客户端连接一个对象；`forward()` 退役、用例搬到引擎；R27 反查、租约握到请求结束；缺省 `None` 时 502（第 9 节） |
| M1-D23 | 记录 | 每个请求一条，加 `method`、`url`、`statusCode`；`remoteAddr` 含借来的连接；`up` / `down` 是上游一侧的字节；承载连接只在没有请求走到选路时补一条（第 10 节） |
| M1-D24 | 计数 | `GET /v1/rurge/http`：五种嗅探结局（M1a）与请求数（M1b）（第 11 节） |
| M1-D25 | 日志 | 每个请求一行，级别同今天；加方法与状态码；路径只在 DEBUG、不写查询串与 userinfo；嗅探不另写（第 12 节） |
| M1-D26 | 依赖 | 不加包；新边 `rurge-http → { rurge-config, rurge-net, rurge-inbound, rurge-proto }` 与 `rurge-engine → rurge-http`；`rurge-tun` 不依赖 `rurge-http`（第 3 节） |
| M1-D27 | 测试 | 真实的 rustls ClientHello 切块喂；数连接的回环服务器；嗅探时限可调短；不碰信任库；讲"拨号之后才答"的现有用例改用不嗅探的端口（第 14 节） |

## 19. 留给项目所有者的

| 问题 | 与 M1 的关系 |
| ---- | ------------ |
| 总设计 Q1：用户流量的内容算不算 API 输出 | M1b 起每条 HTTP 请求的记录带完整的 URL（含查询串，去掉 userinfo），经 `/v1/requests/*` 出去；日志不写查询串。照总设计的临时做法做 |
| 总设计 Q6：需求文档的阶段栏 | FR-RULE-07 / 08 的拨号之前部分在 M1a 做了，阶段栏不改，由验收文档说明 |
| 总设计 Q2 ～ Q5 | 与 M1 无直接关系，照旧开着 |

另有两件事要项目所有者做：**推送**（M1b 做完之后，读回那次 CI）；**手工验收**（阶段 4 的手工清单从 M2 起建，M1 的行为——浏览器经三种入口上网、80 端口的明文请求逐个记录——随 M2 的那一份一起写进去；阶段 1、2、3 的清单照旧一项也没有做过）。

所有者可以改的裁定：本文 M1-D1 ～ D27，尤其 M1-D3（预匹配会拒时不嗅探：多一次规则的探询）、M1-D5（15 秒）、M1-D7（读不懂的 ClientHello 判为别的、不设协议）、M1-D11（虚拟网卡上停留的两处不同）、M1-D14（每条客户端连接 4 条空闲、90 秒）与 M1-D23（`up` / `down` 只数上游一侧）。

## 20. 写计划时的订正

M1a 的计划（`docs/superpowers/plans/2026-10-29-phase4-m1a-sniffing-plan.md`）对着 `3ec732e` 写，改了本文下面几处。以计划为准，理由在计划「计划期决定」的对应行。M1b 的订正在 M1b 的计划写好时补在这里。

1. **第 3 节的依赖边分两步加**（计划 P2）：M1a 的 `rurge-http` 只依赖 `tokio`（另有只供用例的 `rustls`），`rurge-config`、`rurge-net`、`rurge-inbound`、`rurge-proto` 四条边在 M1b 用到时再加。`rurge-engine → rurge-http` 在 M1a 加。M1-D26 的"不加包"不变。
2. **第 6.2 节的 `NeedMore` 带一个数**（计划 P3）："判出来至少要多少字节"。`sniff` 不到这个数不再判，一个字节一个字节发来的客户端因此不会让解析从头重来。这个数大于 64 KiB 时判 `Other`。
3. **第 6.3 节的读出错是第六种结局 `Failed`**（计划 P4），不是"带上出错的 `Closed`"。计数算在 `closed` 里，同第 7.5 节。
4. **第 4.1 节：fake 地址背后没有名字的不嗅探**（计划 P5）。经监听器来、目标是 rurge 地址空间里的 IP、`name_of` 给不出名字的会话，照今天的路走：拨号时以 `fake address not known` 失败，客户端拿到今天的失败应答。
5. **第 4.3 节的探询写细**（计划 P6）：
   - 只对经 HTTP 监听器与 SOCKS5 来的会话问；
   - 只在拨号时会走到规则时问：模式是规则，或模式是代理而全局策略不可用；
   - 看的是 fake 地址换成名字之后的会话；
   - `choose_policy_with` 里"外部程序的会话一律直连"的一支，探询时不认（它要等进程，`sniffable` 是同步的）：探询拒了这样的会话，只是不嗅探它。
6. **第 7.3 节的域名规则是五种**（计划 P7）：加上规则集文件里的 `DOMAIN-SET` 一行。它带标志时整集都带；整集带时它也带，求值时把标志传给它引用的域名集，同今天的写法。原文"`DOMAIN-SET` 的行没有参数"说的是域名集文件的行（`SetLine::Domain`），它们照旧只看引用它的那一行。
7. **第 12 节"SOCKS5 写应答失败一行 DEBUG"不另写**（计划 P8）：`handle` 照今天把错误返回给监听器，由监听器已有的那一行 DEBUG 记下。`CONNECT` 升级失败的一行照写，不带地址。
8. **第 7.1 节的停止多一处**（计划 P10）：引擎里的 `hold_dropped` 也与会话根令牌竞速，守护进程退出时不再停到 `drop_hold`。`rurge-inbound` 缺省的 `serve` 没有会话根令牌，只看 `opts.stop`。
9. **第 11 节的类型放在引擎里**（计划 P16、P19）：
   - `HttpCounts` 与 `SniffedCounts` 定义在 `rurge-engine` 的新模块 `serve.rs` 里，由 `Engine::http_counts()` 读，`rurge-api` 不认识 `rurge-http` 的类型。
   - 引擎里已有的 `sniff.rs`（`relay` 的首块嗅探与 STUN）不动。
10. **第 16 节的核对**：V1 ～ V10、V15、V16 的结论写在计划 P21 与 P18 里（V8：rustls 只开了 ring，没有 ML-KEM，第 14 节第 1 条已照此写）。V11 ～ V14 是 M1b 的，写 M1b 的计划时核。
11. **`CLAUDE.md`「架构」的一句**（计划 P20）："`rurge-http` 到后面的阶段才建"在 M1a 之后不再对。M1a 的收尾把它改成事实；它不是进度，所以不受"`CLAUDE.md` 只改「现在在哪里」"的限制。

M1b 的计划（`docs/superpowers/plans/2026-10-29-phase4-m1b-http-engine-plan.md`）对着 `376cb56` 写，又改了本文下面几处（接着编号）。同样以计划为准，理由在计划「计划期决定」的对应行。

12. **第 8.2、8.4 节与第 3 节的接口草图：选路分成两步**（计划 P1）。`open` 是同步的，建请求的句柄（编号、登记、连接令牌的孩子、连接的进程槽），`rurge-http` 拿到它就建守卫；`route` 是异步的，求规则、选策略、算池键。hyper 在客户端走了时丢掉请求的 future，丢在第一个 `await` 上时，`route` 里建好、登记好的句柄就没有人结束了。`route` 与 `dial` 都不与令牌竞速（同第 7.1 节"拨号不竞速"），返回之后看令牌。
13. **第 8.2 节的流程图与第 8.8 节的表：REJECT 一族是拨号的结果**（计划 P3）。`REJECT` 的出站在拨号时返回 `OutboundError::Reject`，再经 `escalation` 升级，同今天的 `connect_session`；选路只会因暂停、DNS 失败与求值失败拒绝。答复照第 8.8 节的表，不变。
14. **第 3 节的接口草图：交回的拒绝与失败不结束句柄**（计划 P4）。`route_session` 与 `dial_route` 交回 `Refused`，今天的会话由 `connect_session` 照旧结束；引擎里的请求由 `rurge-http` 先设状态码、再结束，记录与会话日志因此带着码。`Upstreams` 的确切形状（`named`、`open`、`route`、`dial`、`relay` 与 `Key`、`Route`、`Lease`）以计划的任务 2 为准。
15. **第 9 节：监听器在一条连接上第一个绝对形式请求到来时才问 `plain_requests`**（计划 P12），不是连接开始时：只发 `CONNECT` 的连接不必查进程、不必建池。
16. **第 8.11 节：守卫的结局多一种**（计划 P18）。连接令牌已取消（停止、守护进程退出）的记 `Completed`，同今天 `forward()` 的 `cancelled_outcome`；被终止的记 `Failed("killed")`；其余记 `Failed("client closed the connection")`。
17. **第 8.3 节的 `http_host`：绝对形式的请求取请求行的主机**（计划 P22）。RFC 9112 第 3.2.2 节：代理收到绝对形式的请求时忽略收到的 `Host`，换成请求目标的主机；今天的 `forward()` 也取 URL 的。源站形式照旧取 `Host` 头。
18. **第 3 节与第 16 节要核对的事项**：`http-body` 不是工作区依赖，`Body`、`Frame` 用 hyper 的再导出（计划 P2）；第 8.5 节的 `TCP_NODELAY` 已经开着（`crates/rurge-net/src/connector.rs:532`），不用改；V11 ～ V16 的结论写在计划 P21 里，`Expect: 100-continue` 读 hyper 源码的结论写在计划 P23 里。

## 21. 执行期的订正

执行时改了本文下面两处。以这里为准，理由在计划「执行期修正记录」的对应行；第 20 节各条的订正照旧有效。

1. **第 6.2 节与第 20 节第 2 条的 `NeedMore` 的数取“握手消息最早能齐的位置”**（协调者的裁定 R2；M1a 记录第 20 行，任务 2 修正第 1 轮）：还不知道长度时是握手的头（4 个字节），知道之后是整条消息（`4 + 24 位长度`）；记录的头没到齐时是 `at + 5 + (wanted - got)`，当前记录没到齐时是 `at + 5 + (wanted - before)`，这个位置超过当前记录的末尾就再加 5（后面至少还有一个记录头）。P3 的用意不变（不到这个数不再判），只是数取这个下界；第 20 节第 2 条写的“至少要多少字节”照这一条理解。为什么：旧规则（当前记录的末尾与消息末尾中小的那个）在一串只带很小负载的记录上每到一个记录边界就把前缀从头接一遍，判的次数随记录数线性增长、总量是记录数的平方——一台机器上的一次 release 运行（只依赖 `rurge-http` 的小程序，不是 CI）量到，负载 1 字节 / 消息 10918 字节：21846 次、776 毫秒（新规则 49 次、2.2 毫秒）。代价是后面某个记录的头不合格时要到那个位置才判 `Other`（结论不变，照旧原样转发）。

2. **第 4.3 节与第 13 节第 1 条：探询时没有会话的进程**（协调者的裁定 R1；M1a 记录第 10 行）：`sniffable` 的预匹配探询手里没有会话的进程（求规则时的 `pre_pass` 有）。能带 `pre-matching` 的规则自己都不是 `PROCESS-NAME`，但带这个标志的 `RULE-SET` 引用的集合里可以有：这样的会话探询时不拒，照可嗅探的路先答“成功”，求规则时才被拒，客户端看到的是一个随即关闭的隧道。第 13 节第 1 条的登记因此以这一条为准——“预匹配会拒时不嗅探”说的是探询看得见的那些拒绝。

3. 任务 4 ～ 8 的修正记录里没有改了本文正文意思的行（M1a 记录第 24 ～ 35 行逐行看过：都是做法、名字、注释与用例的事，或第 20 节已经写下的订正），所以这一节只有上面两条。

M1b 执行时又改了下面几处。以这里为准，理由在 M1b 计划（`docs/superpowers/plans/2026-10-29-phase4-m1b-http-engine-plan.md`）「执行期修正记录」的对应行；第 20 节各条的订正照旧有效。

4. **第 8.4 节：选路也在句柄的 `Trail` 之下跑**（M1b 记录第 8 行，第 21 行订正它的前提）：不只 `dial_route`，`route_session` 也在 `handle.trail().follow(…)` 里，所以选路时规则触发的解析也记进那个请求的 `dns_ms`（第 8.4 节原先只把 `Trail` 写在拨号之下；`crates/rurge-engine/tests/pipeline.rs` 的 `a_rejected_session_has_no_remote_address_but_the_lookups_of_the_rules` 断言规则触发的那次解析记在 `dns_ms` 上）。
5. **第 8.9 节：隧道里的 `CONNECT` 与没要升级的 101**（M1b 记录第 32 行）：所述“照原样转给连接的目标”的两处具体化——`CONNECT` 的 2xx 去掉 `Content-Length`（留着它 hyper 会把整个应答作废、客户端读不到头），没要升级而答 101 的按第 8.6 节的“其余”答 502（记录 `Failed("upstream answered 101 Switching Protocols unasked")`）。
6. **第 8.10 节：等应答头的时限从请求正文的进展起算**（M1b 记录第 28、32 行）：`Idle` 的时限在请求正文第一次被 `poll_frame` 时才起表，每交出一帧或读到末尾重新起算；重发之前 `Progress::restart` 再起一次。新拨号与重拨的时间因此不算成上传停住或应答迟到，慢慢上传的请求不会在正文发完之前拿到 504。
7. **第 8.10、8.11 节与第 3 节的接口：客户端不再读应答时的看门狗**（M1b 记录第 28、36、50 行）：第 8.11 节只写了“被终止的、停止的、其余失败”三种结局，没有说客户端不读时谁来结束连接。实现里 `Conn` 多一个 `abort`（不是令牌的孩子，由持有连接的一方建）：看得见的三件事同时等（`biased`）——`Tail` 的退出令牌、请求的令牌、以及“应答正文没有在等上游而客户端满 `idle_timeout` 没有再取一帧”（上一次 poll 时源头是空的、在等上游，就看门狗只睡、不判，那段等待归 `Tail` 计时）；后一件到时句柄记 `Failed("client stopped reading")` 再 `abort.cancel()`。`rurge_http::serve_connection`（`serve_http` 调的那一个）与监听器自己的 `serve_connection` 都与 `abort.cancelled()` 竞速，先到就丢掉连接（`serve_http` 本身不竞速）；`ClientConn`（第 9 节）因此多一个 `pub abort: CancellationToken`，由监听器建、随它交给引擎。
8. **第 8.11 节：停止时第一个请求头还没读完的连接当场结束**（M1b 记录第 44 行）：hyper 1.11.1 的新连接从 `KA::Busy` 开始，`graceful_shutdown()` 只在读缓冲空着时当场关，否则只关 `KA::Idle` 的连接——半个请求头的连接会一直读到 `header_read_timeout`（嗅探到的连接是 `idle_timeout`，缺省 600 秒）。所以连接令牌先到、而这一条连接上一个请求也没走到选路（`routed() == 0`）时当场结束，不 `graceful_shutdown`；走到过选路的照第 8.11 节优雅关闭。
9. **第 9 节与 R27：fake 地址的租约比“这个请求结束”长**（M1b 记录第 37 行）：租约不挂在监听器的局部变量里，而挂在 `ConnUpstreams` 上，因此活到这条客户端连接真正结束（升级之后转发的任务还握着它）。第 9 节写的“租约握到这个请求结束”以这一条为准。
10. **第 9 节：反查不到名字的 fake 地址先求规则、再答 502**（M1b 记录第 56、60 行）：`rurge-http` 先 `route` 再看反查的结果，所以绝对形式的请求先求过规则（规则的命中数加一、记录带着命中的规则）再答 `fake address not known`；`CONNECT` 那一路（`Dialer::dial` 的 `Turned::Unknown`）在求规则之前就失败，记录没有规则。两路的次序不同，本里程碑没有统一。
