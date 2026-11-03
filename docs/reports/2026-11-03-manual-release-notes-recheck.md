# 手册与发行说明重对报告：2026-11-03

| 项       | 内容 |
| -------- | ---- |
| 日期     | 2026-11-03 |
| 对应     | 阶段 4 总设计（[`docs/superpowers/specs/2026-10-29-phase4-http-engine-design.md`](../superpowers/specs/2026-10-29-phase4-http-engine-design.md)）第 1.5 节："M6 重对一次发行说明"；手册基线重对计划延后第 8 行："下一次重对从这里开始"。是阶段 4 / M6 细化设计（[`docs/superpowers/specs/2026-11-03-phase4-m6-acceptance-design.md`](../superpowers/specs/2026-11-03-phase4-m6-acceptance-design.md)，下面写"M6 设计"）的调研。手册一半的做法照 2026-10-28 的复查报告（[`2026-10-28-manual-6.11-recheck.md`](2026-10-28-manual-6.11-recheck.md)，下面写"10-28 报告"）第 1 节与基线报告（[`2026-10-14-manual-baseline-recheck.md`](2026-10-14-manual-baseline-recheck.md)）的附录 B |
| 结论     | **手册**：目录照旧是那 87 页；10-28 之后改了 11 页，新内容都标着 iOS 6.0.0+ / Mac 6.11.0+——手册现在至少描述到 Surge iOS 6.0.0 / Mac 6.11.0，10-28 时标着 iOS 5.24.0+ 的地方也改成了 iOS 6.0.0+。阶段 4 读的六页（`http/` 下的）与 10-14 的基线逐字节相同。**发行说明**：官方频道 2026-10-13 ～ 2026-11-02 的帖子里，与阶段 4 有关的 12 条（第 3 节）；与阶段 4 无关的列在第 4 节，去向项目所有者。与阶段 4 有关、手册新写的两件：抓包过滤器（第 2.1 节）、QUIC 的识别与阻断只对增强模式的流量（第 2.3 节） |

只读：取手册首页与 87 页的 HTML、算文本的摘要、读改过的 11 页；取官方频道公开的网页两页。没有观察过 Surge 的行为。手册与发行说明的文字不进仓库：下面只引被对照的那一句（带页面路径或帖子号）。

## 1. 做法

1. **手册**，照 10-28 报告第 1 节：取首页 `https://manual.nssurge.com/` 的目录，收集 `data-path`，去掉首页自己是 87 页，与基线报告附录 A 的清单逐项相同；逐页取 HTML，经基线报告附录 B 的脚本变成文本、算 SHA-256；拿基线报告附录 A 比，10-28 改过的四页拿 10-28 报告第 2 节的右栏比。取于 2026-11-03（UTC 06 时）。76 页的摘要与比的值相同，也说明取文本的办法与前两轮逐字节一致。
2. **改了的 11 页**，与 10-14 的基线文本逐行比（基线的文本留在那一轮的工作区里）。10-28 的文本没有留下，所以 10-28 改过的四页（`policies/parameters.html`、`policies/wireguard.html`、`policies/tailscale.html`、`tools/http-api.html`）是对 10-14 比、再扣掉 10-28 报告第 3 节已经记了的。
3. **发行说明**：官方频道（Telegram 的 SurgeTestFlight，公开可读）的网页 `https://t.me/s/SurgeTestFlight` 与它的上一页 `?before=1802`，帖子 1779 ～ 1822（2026-10-05 ～ 2026-11-02，UTC）；读的是 2026-10-13 起的（帖子 1793 起），正文逐字取下来、不是摘要。帖子 1811、1816 没有正文（图片或视频），1817、1818、1820 不在这两页上。测试版的说明（`#Beta`、`#TestFlight`、"Beta Updates"）与正式版（`#Release`）分开说：测试版的条目进了哪个正式版，这一轮没有查。

## 2. 手册：10-28 之后改了的 11 页

| 页 | 10-28（基线报告附录 A，或 10-28 报告第 2 节的右栏） | 2026-11-03 | 改了什么 |
| -- | -------------- | ---------- | -------- |
| `profile/general.html` | `365c6303284d171914d2b5e92e7bbc0a65b1c5a80e9c3ae9b5bd92e3868375e4` | `65d8bf36537b0ccfb10a310516471acf955bbaf35e2be60c27881b690ae48682` | `http-api-mcp`；`socks5-listen` 的用户名与密码；SOCKS5 服务收 UDP ASSOCIATE（第 2.4 节） |
| `rules/domain.html` | `5b5829808aa98aba3d6ac3fedc05f425ec719576dd0c24d24b0c419c46bd3467` | `bafd72f25345c1b0888a832c678287749fb36a2ba5ef14fe68c23f7e87f55f8d` | 规则集与域名集不再有条数上限（第 2.6 节） |
| `rules/ruleset.html` | `934195afb2ce2c5d5bc3612e5b11b00768ba4b54af2753be751a8326fb827348` | `f068239e2d1689cfa84a5806f6807042efe60174635d7df98940b4112fc3cf31` | 同上 |
| `policies/parameters.html` | `a6d9f9b03e4a5cce29eadf1ec8109b206f5dd4c936775a24da2c28abde88d439` | `d1db3cb4633df35a3d6dccf14d8df0dd2689eae443a07eecd0e46322812d753a` | `block-quic = adaptive` 的细节（第 2.5 节） |
| `policies/udp.html` | `2bc67edf714a7bdebded6ab3ff2e9e058b5ec4ba24830d2ceede1e2f537c74f1` | `6a66596cecd943d2ce13bb2e3f31de8d36f29818856feb3d08879cf8afd353fc` | 新的一节 SOCKS5 Inbound UDP（第 2.3 节） |
| `policies/wireguard.html` | `bb9ca785407463b4b12bb1e44feab2551ce2f92e2f321c09e6ee15b34b5e2660` | `a4d7b8422182e17c44c04f9ae68005ad236fc8ea35c9c4cf79a69eab3cd6a03a` | `accept-incoming` 改名 `allow-peer-access`；Apple 屏幕共享（第 2.7 节） |
| `policies/tailscale.html` | `649ec971d02bef6bc7b7cedf5b82544ca60a36e8bff54dc97ded2f11925d57a7` | `d29321ab628a7dcc985fd934b499392235cc28556a0e36e10f3fb497cebc8931` | 同上 |
| `features/gateway.html` | `1778c2446dd41780b979617f926f407dfeaf0befecddbae776ef19746bf6f5e2` | `ead313ca574336d50b1df1fd0e5ea7594b3969b43557a3985e0e221a9dbe9c06` | Disable UDP Fast Path（第 2.8 节） |
| `tools/testing.html` | `d6469f8ab18001cb4981ef1259120992f449e2a189335d046d5eecd61903d815` | `9f1bb9f5ac5520fbe5ebb1bd0e26ce03fcdbcaf3e5b3d70eeed27634ba301493` | `upload-size-limit` 只收整数的字节数（第 2.9 节） |
| `tools/cli.html` | `798c73c140e74ff0ba56584091bec92719538803b07cd41a29ff7d4118cc195e` | `2f761081bd58f46a036bb90478b122d69909a0d8192b7f62b1218c8402b03be2` | `capture-filter`；按节改 Profile（第 2.2 节） |
| `tools/http-api.html` | `258a433df059fc75043edd8ad3ef9f25f296f81be1a410227383d7435f8d6167` | `b804c1a359940189daed0c37caf623ec0469d596456cfa15b8b58296315b43b5` | 抓包过滤器、MCP、`Authorization: Bearer`、按节改 Profile、`testBytes`（第 2.1 节） |

其余 76 页的摘要照旧是基线报告附录 A 的值（10-28 改过的四页在上表）。下一次复查拿附录 A 与上表的右栏比。阶段 4 读的六页——`http/overview.html`、`http/mitm.html`、`http/url-rewrite.html`、`http/header-rewrite.html`、`http/body-rewrite.html`、`http/map-local.html`——都在没有改的 76 页里，与 10-14 的基线逐字节相同；`rules/http.html`（`USER-AGENT`、`URL-REGEX`）、`rules/overview.html`、`rules/protocol-and-network.html` 也没有改。

### 2.1 `tools/http-api.html`：抓包过滤器、MCP 与另几处

- **抓包过滤器**（与阶段 4 有关）："Obtain or change the filters that decide which requests are recorded. `in-memory` applies to the recent requests kept while HTTP Capture is off, and `on-disk` applies to an HTTP Capture session."——端点 `GET /v1/capture/filters` 与 `POST /v1/capture/filters`，过滤器的键是 `httpOnly`、`hideCrashReporterRequests`、`hideAppleRequests`、`hideUDP`、`type`（`off`、`with-keywords`、`without-keywords`、`pattern`）、`keywords`、`disabledKeywords`。rurge 今天：没有这两个端点；抓包存什么由 `capture` 开关与规则的 `always-capture` 定（阶段 4 / M4a），没有过滤器。**去向**：M6 设计第 3.5 节（登记，不做）。
- `http-api-mcp` 与 `POST /v1/mcp`："The endpoint is off by default; enable it with `http-api-mcp = true`."；MCP 一节里的 "With `http-api-tls`, the client must trust the MITM CA certificate"（与 rurge 的 `http-api-tls` 一致：叶证书由 `[MITM]` 的 CA 签，阶段 4 / M2c）。rurge 今天：没有 MCP；`http-api-mcp` 是不认识的键。
- 鉴权多一种写法："Clients that only support bearer tokens, such as MCP clients, may send the key as `Authorization: Bearer examplekey` instead."。rurge 今天：只认 `X-Key` 头与 `x-key` 查询参数（FR-OBS-02）。
- 按节读写 Profile：`GET /v1/profiles/sections`、`GET /v1/profiles/section?name=…`、`POST /v1/profiles/section`。rurge 今天：没有。
- `testBytes`："Each policy under `connector` also reports `testBytes`"。rurge 今天：没有这个字段。
- `include_external=true` 的版本标记从 iOS 5.24.0+ 改成 iOS 6.0.0+（内容在 10-28 报告第 3.4 节）。

### 2.2 `tools/cli.html`：`capture-filter` 与按节改 Profile

"`capture-filter` inspects and changes the filters that decide which requests are recorded."（与第 2.1 节的端点同一件事）；`surge-cli profile section list` / `get` / `set`。rurge 今天：命令行没有 `capture-filter` 与 `profile`（CLI 的 `feature` 一类命令在阶段 6）。

### 2.3 `policies/udp.html`：SOCKS5 Inbound UDP

新的一节，写 SOCKS5 服务收 UDP ASSOCIATE 之后的样子：每个关联一个中继端口、只收客户端那个地址来的数据报；"The policy is selected once per association by rule-matching the first datagram's destination"；域名目标按名字匹配；一个关联只用一个地址族；不支持分片（`FRAG != 0` 丢掉）。与阶段 4 有关的一句：**"QUIC detection and blocking apply only to Enhanced Mode traffic, not to UDP ASSOCIATE associations."**

rurge 今天（读代码，没有用例核过）：SOCKS5 的 UDP ASSOCIATE 从阶段 2 / M5a 起就有（兼容性清单 2.1 节 `socks5-listen` 一行）；它的流经 `crates/rurge-engine/src/udp.rs` 的 `serve` → `run_flow`：第一个数据报照样识别 QUIC 与 STUN（阶段 3 / M5）、读 ClientHello（阶段 4 / M5a）、按 `block-quic`（`quic_verdict`）阻断，`auto-quic-block` 是否也作用于它们没有读到。也就是 rurge 对 UDP ASSOCIATE 的流做 QUIC 的识别与阻断，与手册新写的这一句不同。每个关联里"第一个数据报定策略、之后的照用"一条，rurge 是每个目标各一条流、各自求规则（同一个关联里发往不同目标的数据报各走各的），也不同。**去向**：M6 设计第 3.5 节——QUIC 那一句在读阶段 4 / M5 那一片时核实、登记；关联的其余几条不是阶段 4 的事，列给所有者（第 4 节）。

### 2.4 `profile/general.html`

- `http-api-mcp`（iOS 6.0.0+ Mac 6.11.0+），同第 2.1 节。
- `socks5-listen` 可以带用户名与密码："With a `username:password@` prefix, e.g. `user:pass@0.0.0.0:6153`, clients must authenticate with the SOCKS5 username/password method; SOCKS4 clients are refused on that listener."（Mac 6.11.0+；原来写的是不支持认证）。rurge 今天：`socks5-listen` 不支持密码（兼容性清单 2.1 节那一行写着"不支持密码"）。
- SOCKS5 服务收 UDP ASSOCIATE（"The SOCKS5 service accepts the CONNECT and UDP ASSOCIATE commands (and SOCKS4/4a CONNECT)."）。rurge 今天：收 CONNECT 与 UDP ASSOCIATE；SOCKS4 没有核。

### 2.5 `policies/parameters.html`：`block-quic = adaptive` 的细节

10-28 报告第 3.1 节之后又多了：起表的说法改成 "If a TCP connection arrives between the first QUIC packet and 1 second after the block"；"during the first 10 seconds after Surge starts taking over traffic, QUIC stays blocked even if no TCP connection is observed, and the result is not remembered."；"Adaptive mode always blocks QUIC to these hostnames."（`tether.edge.apple`、`mask.icloud.com` 这类只收 QUIC 的系统中继）；版本标记改成 iOS 6.0.0+。rurge 今天：`adaptive` 照 6.11 的手册做在阶段 3 / M5，没有接管后头 10 秒的那一条、也没有总是阻断的名单。不是阶段 4 的事（第 4 节）。

### 2.6 `rules/domain.html`、`rules/ruleset.html`：没有条数上限了

"Earlier versions limit a set to 1,000,000 entries; there is no entry limit since iOS 6.0.0+ Mac 6.11.0+."。rurge 今天：超过 1,000,000 条截断并告警（`crates/rurge-rules/src/set_format.rs` 的 `MAX_ENTRIES`；FR-RULE-04 写着"上限 1,000,000"）。不是阶段 4 的事（第 4 节）。

### 2.7 `policies/wireguard.html`、`policies/tailscale.html`

`allow-peer-access`（"The earlier name `accept-incoming` is still accepted."）；启用后没有 `keepalive` 的 peer 用 25 秒（WireGuard）；Apple 屏幕共享的 TCP 5900 与 UDP 5900–5909 交给系统、不经规则（"These flows do not go through rule matching and do not appear in the request list."）；版本标记改成 iOS 6.0.0+。rurge 今天：`[WireGuard]` 里认 `accept-incoming`（阶段 3 / M5 让它能加载、说明不生效）；`allow-peer-access` 是不认识的键；`[Tailscale]` 整节是 `W0016`。不是阶段 4 的事（第 4 节）。

### 2.8 `features/gateway.html`

"To turn off the fast path for all devices, enable Disable UDP Fast Path in the VM Gateway tab of the gateway mode configuration."（Mac 6.11.0+，界面里的选项，不是 Profile 的键）。网关模式在阶段 7。

### 2.9 `tools/testing.html`

`upload-size-limit` 从"size, default: 1GB"改成"integer, default: 1000000000"，"Unit suffixes such as `GB` are not supported."。rurge 今天：代码里没有这个键（`git grep` 查不到）。不是阶段 4 的事（第 4 节）。

## 3. 发行说明：与阶段 4 有关的

| # | 帖子 | 原文（被对照的那一句） | rurge 今天 | 去向 |
| - | ---- | ---------------------- | ---------- | ---- |
| 1 | 1796（2026-10-13，Mac 6.10.0-12500，正式版） | "In the previous version, MITM security was strengthened by generating a separate key pair for each distinct domain name. This caused noticeable delays when performing MITM concurrently on a large number of different domains. After evaluation, this change has been reverted." | 每张叶证书各自新生成一把 P-256 的密钥（总设计 P4-D8，阶段 4 / M2）；签一张叶证书的中位数 248.6 µs（阶段 4 / M5b 的 B4：第一遍减第二遍的中位数，含在阻塞线程上签发与两次交接；握手的 p99 与最大两遍都在 25 ～ 27 ms，与签发无关，M5b 计划延后第 2 行；一台机器上 release 的一次运行，不是 CI） | 与 Surge 的做法不同：登记进兼容性清单 7.2 节；要不要改归项目所有者（M6 设计第 13 节） |
| 2 | 1795（2026-10-13，Mac 6.11.0-12510，测试版） | "QUIC hostname detection now works when the TLS ClientHello spans several packets … QUIC v2 and draft-29 are now supported as well. Rules, MITM and request records can now see the hostname of more QUIC connections." | 跨几个数据报拼 ClientHello 做在阶段 4 / M5a；v1、v2 与草案 29 ～ 32 的 Initial 都认（`crates/rurge-net/src/quic/initial.rs` 的 `quic_version`，阶段 3 / M5） | 读阶段 4 / M5 那一片时对着它读（M6 设计第 3.5 节） |
| 3 | 1799、1801（2026-10-19，iOS 5.102.0 (3872)，测试版） | "Fixed requests failing with 503 when a server sends a body after a 304 or another response that should have no body" | 没有读到、没有用例 | 读阶段 4 / M1 那一片（FR-HTTP-01）时对着它读 |
| 4 | 同上 | "Fixed high memory use on MITM HTTP/2 connections when the client reads slowly." | 没有量过读得慢的客户端（M5b 的 B5 量的是空闲连接与一次写完的正文） | 读阶段 4 / M2 那一片（FR-HTTP-02）时对着它读 |
| 5 | 1806、1807（2026-10-21，iOS 5.102.0 (3874)，测试版） | "HTTP proxy: the Proxy-Authorization header is no longer forwarded to the upstream server. It carries the credentials for Surge's own proxy." | 一致：HTTP 监听器收到的请求一律去掉这个头，开着认证时先拿它核对（`crates/rurge-inbound/src/http.rs` 的 `handle`）；引擎只在嗅探到的连接上、且请求没有被重写换目标时照转（`crates/rurge-http/src/headers.rs`，总设计 P4-D5：嗅探到的连接上的代理式请求原样转给连接的目标） | 读阶段 4 / M1 那一片时核实 |
| 6 | 同上 | "HTTP/2: a malformed request now resets only its own stream instead of the whole connection." | 没有读到 | 读阶段 4 / M2 那一片（FR-HTTP-02）时对着它读 |
| 7 | 同上 | "HTTP/2: when the upstream server closes the connection in the middle of a response, Surge now reports it as truncated instead of as a complete response." | 没有读到（请求记录与客户端各看到什么） | 同上 |
| 8 | 同上 | "Fixed HTTP/2 responses with no body (304, 204, or a response to HEAD) not being handled correctly when both a response script and a body rewrite rule matched." | 脚本在阶段 5；Body Rewrite 碰到没有正文的应答时怎样，没有读到 | 读阶段 4 / M3 那一片（FR-HTTP-07）时对着它读 |
| 9 | 同上 | "Fixed pipelined HTTP/1 requests sent after a chunked request body being forwarded as part of that body instead of being handled as separate requests." | 没有读到 | 读阶段 4 / M1 那一片（FR-HTTP-01）时对着它读 |
| 10 | 同上 | "Fixed several edge cases in HTTP/1 connection reuse and closing that could mix up responses or leave connections open." | 说得太笼统，对不上具体的句子 | 只记下 |
| 11 | 同上 | "[iOS] Fixed Map Local data files larger than 2 MB being rejected on iOS. The limit is now 15 MB, as the manual says." | 一致：15 MB 是手册给 iOS 与 tvOS 的，rurge 的平台上 Map Local 的文件没有这个上限（阶段 4 / M3 设计第 8.5 节） | 不改 |
| 12 | 1805（2026-10-21，安全公告） | "Although Surge uses its own TLS protocol stack and MITM engine, it strictly relies on the operating system's certificate trust evaluation mechanisms to validate certificate chains. As a result, Surge benefits from the same certificate trust and revocation protections enforced by the operating system." | 解密的上游由 rustls 自己校验，信"系统的根 ∪ `webpki-roots`"（总设计 P4-D10），不经操作系统的信任评估、不查吊销；操作系统对某个中间 CA 的不信任，rurge 看不到 | 归项目所有者：总设计 Q4 已经问了要不要改用 `rustls-platform-verifier`，这一条是 Q4 的又一条事实 |

帖子 1802、1803（2026-10-19，tvOS）说菜单里能开关 MitM、Rewrite 与 Scripting——与 rurge 的三个开关对得上，没有要做的。

## 4. 发行说明与手册：与阶段 4 无关的

下面这些不在阶段 4 的范围里，这一轮不核代码、不改兼容性清单，列给项目所有者（去向：要不要、在哪个阶段处理它们，是不是另开一轮手册与发行说明的重对把它们落进清单）：

| 件 | 出处 | 与 rurge 的关系 |
| -- | ---- | --------------- |
| `block-quic = adaptive` 又改了两次：只对新的握手判断（1799、1801）；"no TCP connection appears within 0.5 seconds of the block, and the app retries over QUIC within 3 seconds"（1821、1822，2026-11-02）；接管后头 10 秒与总是阻断的系统中继（手册，第 2.5 节） | 测试版、手册 | 阶段 3 / M5 照 6.11 的手册做的 `adaptive` 与今天的手册不同 |
| 443 端口上的 DTLS、WireGuard、STUN 等不再当作 QUIC（1799、1801） | 测试版 | 阶段 3 / M5 的识别看的是包的样子、在所有端口上；没有核 |
| SOCKS5 入站的 UDP：一个关联第一个数据报定策略、一个地址族、不支持分片（手册第 2.3 节；1806、1807） | 手册、测试版 | 阶段 2 / M5a 的做法是每个目标一条流、各自求规则 |
| `socks5-listen` 的用户名与密码（手册第 2.4 节） | 手册 | 不支持（阶段 1） |
| 规则集与域名集没有条数上限（手册第 2.6 节；1808、1809） | 手册、测试版 | 上限 1,000,000、截断并告警；FR-RULE-04 的字面写着这个上限 |
| `allow-peer-access`、屏幕共享、`peer-access-passthrough`（加了又去掉）、Tailscale 的出口节点与子网路由（手册第 2.7 节；1797 ～ 1801、1806 ～ 1809） | 手册、测试版 | WireGuard 作服务端随阶段 7；`[Tailscale]` 是远期 |
| `http-api-mcp`、MCP、`Authorization: Bearer`、按节改 Profile、`testBytes`、外部控制能改 Profile（手册第 2.1、2.2 节；1819） | 手册、测试版 | 阶段 6 |
| `upload-size-limit` 只收整数（手册第 2.9 节） | 手册 | 没有这个键 |
| Disable UDP Fast Path（手册第 2.8 节；1808、1809） | 手册、测试版 | 阶段 7 |
| Snell v6 的 UDP 不等回包（1799、1801） | 测试版 | 阶段 2 的 Snell；没有核 |
| SOCKS5 代理拒绝时显示服务器给的原因（1799、1801） | 测试版 | 没有核 |
| Tailscale、Ponte、流量统计、`$httpClient`、`$surge.retestGroup`、iCloud、界面（1797 ～ 1822 各处） | 测试版 | 远期、阶段 5 或界面 |
| Surge iOS 6 的测试版（1812 ～ 1822）："Engine: Tailscale exit node and subnet routes, WireGuard listen port, adaptive QUIC blocking, UDP over SOCKS5 Wi-Fi Access, and no more rule set size limit. All of these are shared with Surge Mac 6.11." | 测试版 | 同上几行 |

## 5. 交给阶段 4 / M6 的

M6 设计第 3.5 节定怎样用：

1. **登记**：抓包过滤器（第 2.1、2.2 节）——兼容性清单加行、验收文档「留给项目所有者的」里一行（需求文档没有这一句，不进「没有做完的」），去向项目所有者（它在阶段 4 的抓包、还是阶段 6 的 API 与命令行）；叶证书的密钥（第 3 节第 1 条）——兼容性清单 7.2 节的说明与「留给项目所有者的」。
2. **读的时候对着它**：第 3 节第 2 ～ 9 条与第 2.3 节 QUIC 那一句，交给读对应那一片的任务；读出与 rurge 不同的，照 M6 设计第 4.5 节当场补或登记。
3. **给所有者的事实**：第 3 节第 12 条并进总设计 Q4；第 4 节整张表。

## 6. 这份报告不能说明的

- Surge 的实际行为：没有观察过。
- 测试版的条目进了哪个正式版、哪一天：没有查。手册里标着 iOS 6.0.0+ / Mac 6.11.0+ 的内容是不是都已经发布：没有查。
- 没有改的 76 页没有逐句重读：它们相对 10-14 的基线没有变，基线报告与 10-28 报告对它们的结论照旧。
- 取文本的办法丢掉的东西照旧（基线报告第 9 节"取文本时丢掉的"）：图片、链接的去处、表格的行列关系、粗体与斜体。
- 2026-10-13 以前的发行说明这一轮没有读（基线报告第 3.3 节读过 6.10.0 的几份）；知识库与博客没有读。
- 第 2.3 节"rurge 今天"那一段是读代码得来的，没有用例核过。
