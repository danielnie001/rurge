# rurge HTTP API（阶段 1）

rurge 在 `[General] http-api = <key>@<ip>:<port>` 指定的地址上提供 Surge 兼容的 HTTP API。本页记录阶段 1（M4a / M4b，阶段 1 补遗（M5a）加的 `GET /v1/geoip`，以及阶段 1 补遗（M5b）加的 `GET /v1/external_resources` 与 `POST /v1/external_resources/update`）实现的端点与 rurge 暂定的 JSON 结构；手册未定义的结构在阶段 6 与真实 Surge 对齐时可能调整（见 `docs/surge-compatibility-matrix.md` 第 10.4 节）。

## 鉴权与错误

- 每个请求带 `X-Key: <key>` 头或 `?x-key=<key>` 查询参数；比较为常量时间。查询参数取字面值，不做百分号解码，因此只适用于 ASCII 密钥；含 `%` 等特殊字符的密钥请用 `X-Key` 头。
- 失败：`401 {"error":"unauthorized"}`。同一来源 IP 10 分钟内 5 次失败 → 之后 10 分钟内一律 `403 {"error":"banned"}`；封禁表最多跟踪 1024 个来源。
- 所有错误都是 `{"error":"<message>"}`：400（参数、JSON 请求体或查询字符串错误）、401、403、404（未知端点 / 未知请求 id / 未知功能名 / 未知的 Profile 名）、405（`{"error":"method not allowed"}`，请求方法与已注册路径不匹配，如对 `/v1/outbound` 发 `DELETE`）、409（不可终止的内部会话）、500（内部错误）、501（本阶段未实现）。
- 无内容的成功响应是 `{}`。字段名 camelCase。手册给了应答形状的端点（如 `{"mode":"rule"}`、`{"policy":"ProxyA"}`、`{"enabled":true}`、`GET /v1/external_resources` 的 `defines`）照手册的字段写；手册没有给形状的端点——`GET /v1/policies`、`GET /v1/dns` 一类——用的是 rurge 暂定的字段名（`policy-groups`、`dnsCache` 等）：手册那一页只写了 "List all policies." 与 "Obtain the current DNS cache content."，没有这两个词；到阶段 6 与真实 Surge 对齐。
- `http-api` 绑定到非环回地址时启动时打 WARN；改动 `http-api` 需重启 rurge（重载只警告，继续用旧地址 / 密钥）。
- **API 连接的收尾（阶段 1 补遗（M5b））**：服务端放手一条连接时——答完并决定关、对端先关、出错、优雅退出——不是立刻关：先关写端，再读掉并丢弃对端还在发的字节，直到对端关闭、读出错、丢弃满 256 KiB，或从放手起过了 1 秒（一个期限管整件事，不是每次读一个），然后才关；同时处在这一步里的至多 64 条，满了的当场关，与此前每条连接都是的样子相同。读掉的字节不解析、不保存，这一步也不记日志。为的是：不读请求正文的端点——下面「端点」表里"请求"一格为空的 `POST`：`/v1/dns/flush`、`/v1/profiles/reload`、`/v1/stop`——和在读正文之前就被拒绝的请求（`401`、`403`、`404`、`405`）只凭请求头就答复；正文若在这之后才到，它到的是一条已经关了的连接，系统回一个 reset，而 reset 会让客户端的系统把还没读的应答扔掉——此前在 Windows 上，这样的客户端读到的是连接被中止（`ConnectionAborted`，os error 10053）、一个字节也读不到，尽管请求已经执行。Python 的 `http.client`——以及 `urllib.request` 和经 urllib3 的 `requests`——就把请求头与正文分开发：读 CPython v3.13.0 的 `Lib/http/client.py`，`_send_output` 用一次 `send` 发请求头、对正文的每一块各用一次；`requests` 走的 urllib3 2.7.0 里，`HTTPConnection.request` 在 `endheaders()` 之后逐块 `send`（读源码得出，没有运行）。现在这七种应答——上面三个端点各一，加 `401`、`403`、`404`、`405`——客户端都读得到，连同连接的结尾：在一台 Windows 11 x86-64 的开发机上，对着一个把请求头与正文分开写的客户端量过，改动之前读不到、之后读到。不发正文的客户端，和把请求头与正文放进一次写里发的客户端，不受影响；读正文的端点（`POST /v1/log/level` 一类）本来就不受影响；`POST /v1/profiles/check` 现在也读正文（它在正文到齐之前不答复），不在不读正文的端点之列；rurge 自己的 `rurge reload` / `rurge stop` 属把请求头与正文放进一次写里的那一种（读 hyper 的源码得出，没有量过）。请求怎样被读、鉴权与应答都没有动。没有补上的有四处：
  - 超过 256 KiB 之后才读应答的客户端仍可能丢应答：服务端带着没读完的字节关连接，之后到的字节引来 reset，收到而没读的应答是否活下来是客户端系统的事。在同一台机器上，发 300 KiB 之后立刻读或 0.5 秒后读，50 次里 50 次得到应答和连接的结尾；发 8 MiB 之后立刻读，10 次里 10 次同；发 8 MiB 之后 1.5 秒才读，10 次里 10 次什么也没有、一个 reset。API 的请求正文都是小的 JSON，没有已知的客户端会这样；读正文的端点收到超过 2 MiB 的正文时，在越过上限的那一刻就答 400，剩下的字节还在路上，同样的事也落在它们身上（在 `POST /v1/profiles/check` 上量到：超过上限 64 KiB 或 1 MiB 的发送者读得到那个 400，超过 6 MiB、边写边读的发送者在 Windows 上读到的是连接被中止，丢了 400）。
  - 64 个位置由所有对端共用：一个对端连得比每秒 64 条快、又从不关，就占满它们，别的连接当场被关，与此前一样；被封的对端的连接也占位置。
  - 一个已得到应答却不关连接的空闲客户端（面板的长连接一类）使每次停止多等至多 1 秒，在 5 秒的关闭宽限之内：量到停止的等待由改动之前的 0.10 秒变成 1.10 秒。
  - Linux 与 macOS 没有量过，改动之前与之后都没有：自动化用例在那两个系统上断言的是同一件事，但它们在改动之前红不红不知道（手工清单 `docs/acceptance/phase1-manual.md` 各有一行）。1 秒、256 KiB 与 64 条是写死的常量。

## 端点

| 方法 | 路径 | 请求 | 响应 |
| --- | --- | --- | --- |
| GET | `/v1/outbound` | | `{"mode":"direct"\|"proxy"\|"rule"}` |
| POST | `/v1/outbound` | `{"mode":…}` | `{}`；`proxy` 且未设全局策略 → 400 |
| GET | `/v1/outbound/global` | | `{"policy":"<name>"\|null}` |
| POST | `/v1/outbound/global` | `{"policy":"<name>"}`（空串清除） | `{}`；未知策略 → 400；出站模式为 `proxy` 时用空串清空 → 400（先切到 direct / rule） |
| GET | `/v1/policies` | | `{"proxies":[…5 个内置策略 + 配置策略],"policy-groups":[…]}` |
| GET | `/v1/rules` | | `{"rules":[{"index":0,"rule":"DOMAIN,…","hits":3}]}`；`RULE-SET` / `DOMAIN-SET` 的 URL 写了用户名口令时，`rule` 的文本里显示为 `https://***@host/…`（阶段 2 / M8c）。列出的是编译后的规则表，不列第一条 `FINAL` 之后的规则（阶段 1 补遗（M5b））：匹配在第一条启用的 `FINAL` 处结束，策略与选项取最后一条的——这是 rurge 对手册三句话的读法，推断的，没有在 Surge 上观察过（兼容性清单 3.1 节 `FINAL` 一行）——，所以表里是第一条 `FINAL` 之前的规则，最后一条是给出策略的那条 `FINAL`；`index` 是规则在 Profile 里的位置，所以会跳号：Profile 的规则依次是 `DOMAIN,before.test,DIRECT`、`FINAL,Pick`、`DOMAIN,between.test,REJECT`、`DOMAIN,ads.test,REJECT`、`FINAL,DIRECT` 时，列出的是 `index` 为 0 与 4 的两条 |
| GET | `/v1/requests/recent?limit=N` | 默认 100；上限是 `--request-log-size` 的环形缓冲容量；`limit=0` → 400 `limit must be at least 1` | `{"requests":[Request…]}`（最新在前） |
| GET | `/v1/requests/active` | | 同上 |
| POST | `/v1/requests/kill` | `{"id":N}` | `{}`；404 不在进行中；409 `not killable: internal session`；UDP 流（`transport` 为 `udp`）同样可结束 |
| GET | `/v1/traffic` | | 见下「Traffic」 |
| GET | `/v1/dns` | | `{"dnsCache":[Cache…],"upstreams":[…],"bootstrap":[…]}` |
| POST | `/v1/dns/flush` | | `{}` |
| POST | `/v1/test/dns_delay` | `{"name":"<host>"}`，缺省为 `internet-test-url` 的主机 | `{"delays":[{"upstream":"udp://…","ms":12,"error":null}]}` |
| GET | `/v1/geoip?ip=<地址>` | | `{"address":"198.51.100.23","country":"SE","asn":64500,"organization":"Example Network","geoip-db-date":1760000000,"asn-db-date":1760000000}`；见下「GeoIP」 |
| GET | `/v1/external_resources` | | `{"defines":[{"path":…,"type":…,"key":…,"local":…,"ready":…,"updatedAt":…,"updating":…,"fromModule":null}]}`，另有 `error`、`suspended` 与集合的三个数；见下「外部资源」（Surge iOS 5.23 / Mac 6.10 起的端点） |
| POST | `/v1/external_resources/update` | `{"key":"<key>"}`；`"all"` 是每一个下载得到的资源 | `{"<key>":"success"\|"<原因>"}`，每个更新了的键一项；键不存在 → 400；至多等 60 秒；见下「外部资源」（Surge iOS 5.23 / Mac 6.10 起的端点） |
| GET | `/v1/profiles/current?sensitive=0\|1` | | `text/plain`（手册说应答总是 JSON，rurge 的形状随阶段 6 对齐）；返回的是生效的主文本——托管 Profile 的副本生效时是数据目录里下载的副本（阶段 1 补遗（M5b），见下「Profile 的当前文本与检查」）。`sensitive=1` 给出原文，默认（`sensitive=0`）给出脱敏后的文本：行数与行尾 CRLF 原样保留，认得出的秘密换成 `***`。**脱敏是尽力而为，不是保证**——藏什么、有意不藏什么、已知漏什么，见下面「脱敏后的配置」 |
| POST | `/v1/profiles/reload` | | `{"ok":true,"errors":0,"warnings":1,"listenersRebound":false}`；解析失败或构建 `Runtime` 失败时 `ok:false`，运行中的配置不变；重绑监听器失败时也是 `ok:false`，但新配置这时已经生效，只是监听器归零，直到下一次重载成功为止（下一次重载会无条件重试绑定） |
| POST | `/v1/profiles/check` | 可带 `{"name":"<Profile 名>"}` | `{"ok":…,"errors":N,"warnings":N,"diagnostics":[Diagnostic…],"error":null\|"<首个错误>"}`（校验磁盘上的当前配置，不影响运行；`Diagnostic` 与 `rurge check --json` 相同，每项另有 `hint`——字符串或 `null`：阶段 1 补遗（M5a）起多数诊断码有缺省建议，见兼容性清单 10.3 节 `--check` 一行）。阶段 1 补遗（M5b）起应答另有 `error`，请求可带 `name`：不是正在运行的那份 Profile 的名字 → 404 `{"error":"profile not found"}`；正文不是 JSON 对象、`name` 不是字符串或正文超过 2 MiB → 400；见下「Profile 的当前文本与检查」 |
| POST | `/v1/log/level` | `{"level":"verbose"\|"debug"\|"info"\|"notify"\|"warning"\|"error"}` | `{}` |
| GET | `/v1/features/{system_proxy\|enhanced_mode\|mitm\|capture\|rewrite\|scripting}` | | `system_proxy` 返回真实状态 `{"enabled":bool}`；其余恒 `{"enabled":false}` |
| POST | `/v1/features/{name}` | `{"enabled":bool}` | `system_proxy`：成功 `{}`；失败 500 `{"error":"<原因>"}`（例如不支持的 Linux 桌面会带 `export http_proxy=…` 提示）；其余功能 501 |
| GET | `/v1/modules` | | `{"enabled":[],"available":[]}` |
| GET | `/v1/scripting` | | `{"scripts":[]}` |
| GET | `/v1/events` | | `{"events":[]}` |
| POST | `/v1/stop` | | `{}`，随后进程退出（退出码 0） |

### Request

```json
{"id":12,"listener":"http","transport":"tcp","src":"127.0.0.1:51234","listenPort":6152,"dst":"example.com:443","remoteAddr":"203.0.113.7:8443","rule":"DOMAIN-SUFFIX,example.com,Proxy","policy":["Proxy","HK"],"sni":"example.com","protocol":"https","up":1234,"down":56789,"startedMs":1757200000000,"elapsedMs":812,"connectMs":35,"firstByteMs":120,"dnsMs":14,"status":"completed","rejectKind":null,"error":null,"notes":[]}
```

`listener` ∈ `http` `socks5` `tun` `forward` `internal`；`status` ∈ `active` `completed` `rejected` `failed`；`rejectKind` 在 `rejected` 时是 `REJECT` / `REJECT-DROP` / `REJECT-NO-DROP` / `REJECT-TINYGIF`；`protocol` 是嗅探到的协议小写名或 `null`。`src` 与 `dst` 里，IPv4 映射的 IPv6 地址（`::ffff:a.b.c.d`：双栈监听上的 IPv4 客户端，或写成这种形式的目标）显示为它指的 IPv4 地址 `a.b.c.d`（阶段 2 / M8c；规则与出站看到的也是它）。

`transport` 是 `tcp` 或 `udp`（阶段 2 / M5a 起：经 SOCKS5 UDP ASSOCIATE 进来的每条 UDP 流各有一条记录，`dst` 是它的目标，`up` / `down` 是载荷字节数）。`connectMs` 是会话开始到出站就绪的毫秒数（规则匹配、DNS、`evaluate-before-use` 的等待与 `smart` 组换成员的重试都在内），`firstByteMs` 是出站就绪到收到第一个上游字节的毫秒数；还没有对应时刻（被拒绝、拨号失败、还没收到数据）时为 `null`，测试会话（`rule` 为 `policy test`）两者恒为 `null`（阶段 2 / M3c 起）。

阶段 1 补遗（M5a）起每条记录多四个键：`listenPort`（紧跟 `src`）、`remoteAddr`（紧跟 `dst`）、`dnsMs`（紧跟 `firstByteMs`）与最后的 `notes`。测试会话（`rule` 为 `policy test`）的 `listenPort`、`remoteAddr`、`dnsMs` 恒为 `null`。

- **`listenPort`**：会话进来的监听端口，数字；经 SOCKS5 UDP ASSOCIATE 进来的 UDP 流是 SOCKS5 监听端的 TCP 端口。rurge 自己发起的会话——内部 DNS 会话（`listener` 为 `internal`）与连通性测试——没有监听端口，是 `null`。
- **`remoteAddr`**：这条会话**自己**开的那条 TCP 连接连到的地址，`ip:port`，IPv6 地址带方括号，IPv4 映射的地址（`::ffff:a.b.c.d`）显示为 IPv4；链式策略（`underlying-proxy`）记的是第一跳，`smart` 组换过成员时是最后连上的那个成员的地址。会话没有开过这样一条连接时是 `null`。具体到各种策略（TCP 会话）：
  - DIRECT 是连上的那个地址（地址族竞速里赢的那一个）；`http` / `https`、`socks5` / `socks5-tls`、`trojan`、`vmess`、`ss`（含 simple-obfs 与 Shadow TLS 之后）是代理服务器的地址；`external` 是 `127.0.0.1:<local-port>`，程序的本机端口（程序自己再连到哪里，rurge 不知道）。
  - 复用已有连接或隧道的会话没有：`snell` 与 `anytls` 取自池里的连接是 `null`（取出的 `snell` 连接已失效、拨号返回之后才换新连接重发的会话也是 `null`，尽管它自己开了 socket），`h2-connect` 与 HTTP/2 的 `trust-tunnel` 共用一条连接的其余流是 `null`（只有真的拨了新连接的那一条会话有），`ssh` 只有建立（或重建）那条 SSH 会话的会话有，`wireguard` 是 `null`（隧道由设备任务建立）；QUIC 族（`hysteria2`、`tuic` / `tuic-v5`、`masque`、`trust-tunnel` 的 `h3=true`）走 UDP，是 `null`。例外：底层策略的 UDP 载体是当场开的一条 TCP 连接时（带 `udp-relay=true` 的 `socks5` / `socks5-tls`、`external`、`trojan`、v3 起的 `snell`、没有空闲会话的 `anytls`；`wireguard` 与 `vmess` 不是），碰巧打开那条 QUIC 连接的会话带着底层代理服务器的地址（`external` 是程序的本机端口），共用这条 QUIC 连接的会话是 `null`——读代码得出，没有用例。
  - 被拒绝的会话（`REJECT` 一族）与 rurge 不实现的协议（`W0007`）：`null`。所有 UDP 会话——UDP 流与内部 UDP 会话，失败的也算——一律 `null`：UDP 没有单一的远端，而经代理的载体只由第一条用到它的流打开，只让那一条带上地址反而误导。
  - 内部 DNS 会话（`encrypted-dns-follow-outbound-mode`）与别的 TCP 会话同一条规则。DoT / DoH / `tcp://` 上游的会话：直连时是 DNS 服务器的地址（规则选了 DIRECT；或规则选了 REJECT、链底是 REJECT 的代理、以域名配置的代理，rurge 告警后改走直连，见 `docs/api/phase2.md` 的「跟随出站模式下 DNS 的内部会话」一节）；规则把它交给以地址配置的代理时是代理服务器的地址（链式策略是第一跳）；代理走复用的连接或隧道时是 `null`。DoQ / DoH3 的 QUIC 载体是 UDP，`null`。

  这是登记的限制（任务局部变量只在会话自己的任务里可见），补全随阶段 4 的 FR-HTTP-10 完整部分。以上各情形里有用例钉住的是：DIRECT（HTTP 与 SOCKS5 两种入站）、经 `socks5` 的上游、链式策略的第一跳、`smart` 组换成员（记连上的那个成员；先连上又失败、换到的成员没连上时是 `null`）、被拒绝的会话、`external`、内部 DNS 会话（`tcp://` 上游的三种：直连时是服务器的地址，经以地址配置的 `http` 代理时是代理的地址，因代理以域名配置而改走直连时是服务器的地址；SOCKS5 UDP 关联上的 DoQ 载体是 `null`）与 UDP 流（`null`）；其余的是读代码得出的，没有用例：`http`（内部 DNS 会话经它的那一例除外）、`https`、`socks5-tls`、`trojan`、`vmess`、`ss` 与上面测过的 `socks5` 走连接器里同一条路径，各自没有用例，取自池的 `snell` / `anytls` 连接、共用一条连接的 `h2-connect` 与 HTTP/2 `trust-tunnel` 的其余流、`ssh`、`wireguard`、QUIC 族、DoT / DoH 的内部会话、因 REJECT 改走直连的内部会话（含链底是 REJECT 的代理）、底层 UDP 载体是 TCP 连接的那个例外也是。
- **`dnsMs`**：这条会话拨号期间花在域名解析上的毫秒数，只算 rurge 自己的解析器——规则要的解析（`IP-CIDR` 一类遇到域名）与出站自己的解析（含以域名配置的代理服务器）合计；`[Host]` 与缓存命中的也算一次接近 0 的查找（可能是 0），等另一条会话在同一个名字上已在途的查询的时间也算，被外层超时打断的查找记它已花的时间。目标是地址、名字交给代理去解析、或隧道自己的 `dns-server` 解析了它时，会话没有做过查找，是 `null`，不是 0。查找在结束时才记下，所以卡在一次慢解析里的进行中会话显示 `null`。UDP 流照记；解析器向上游发的查询、查找自己开的连接不算会话的。
- **`notes`**：备注，字符串数组，没有时 `[]`。现在只有子规则命中那一句 ``Sub-rule matched: <行> (in <集合>)``（兼容性清单 3.3 节）。`error` 里已登记的说明（组环、空组兜底、`policy does not support UDP; sent through DIRECT` 一类，见 `docs/api/phase2.md`）仍在 `error` 里，不搬进 `notes`。

**暂停时被拒的会话**（阶段 1 补遗（M5b））：托管 Profile 带 `strict`、它的更新到期而最近一次尝试失败（或离线）时，引擎拒绝**新**会话——启动时已经到期、本次运行还没有一次尝试有结果的也一样，从第一个会话起就拒绝（兼容性清单 1.3 节"托管 Profile 的 `strict`"一行）——：已经打开的照常，监听端、API 与连通性测试不受影响。被拒的会话在记录里是 `status` 为 `failed`、`error` 为 `refused: the managed profile requires an update (strict)`、`rule` 为 `null`、`policy` 为 `[]`——拒绝发生在规则、任何查找与任何组之前，所以没有规则也没有策略。更新为什么没成功的原因不进会话的记录、错误页或日志行，只在 WARN 日志（`managed profile: the update that strict requires has not succeeded; new sessions are refused until it does`，字段 `error` 是原因）和 `GET /v1/external_resources` 里托管 Profile 那一项的 `error` 与 `suspended` 里。HTTP 监听端对客户端回 `502 Bad Gateway` 和 rurge 的错误页（标题 `Connection failed`，页里写着那句 `error`；`show-error-page = false` 时直接关连接，没有页）；SOCKS5 监听端回应答码 `0x05`；经 SOCKS5 UDP ASSOCIATE 来的 UDP 流以失败结束（同一句 `error`），它的数据报被丢弃，5 秒后同一条流的下一个数据报再试，所以暂停一结束它就恢复——代价是暂停期间每个活跃的 UDP 目标每 5 秒一条失败记录；`encrypted-dns-follow-outbound-mode` 下 rurge 自己的内部 DNS 会话（`listener` 为 `internal`）同样被拒。暂停只在内存里，不进 `state.json`，重载不动它；更新成功后至多一秒（主循环每秒看一眼）解除，日志写 INFO `managed profile: new sessions are accepted again`。

### Traffic

```json
{"startTime":1757200000.5,"total":{"in":123,"out":45,"inCurrentSpeed":0,"outCurrentSpeed":0},"connector":{"DIRECT":{"in":123,"out":45}},"listener":{"http":{"in":123,"out":45},"socks5":{"in":0,"out":0}}}
```

`in` 是下行（远端 → 客户端）字节，`out` 是上行；速度是最近两次采样之间走的字节数，每秒采样一次，所以平时就是最近一秒的字节数——机器休眠错过的采样不补（阶段 2 / M8c）：醒来后只采一次，第一次读到的是休眠前后这一整段里走的字节，随后恢复为每秒的字节数（此前醒来后连发的补采会把它冲成 0）；`startTime` 是 API 启动时的 Unix 时间戳，**秒**（`f64`，可带小数）——设计文档 §5.2 / 开放问题 Q1 曾写作毫秒，以本页为准。

`total` 统计所有会话，包括仍在进行中的会话与 rurge 自己的内部 DNS 拨号（与 `inCurrentSpeed` / `outCurrentSpeed` 同源，长传输期间不会出现「总量 0 但速度非 0」）；`connector` 与 `listener` 只统计已结束的会话，且 `listener` 在阶段 1 只有 `http` / `socks5` 两项。因此这三个层级不应相互对得上，不要拿它们互相校验。

### Cache

```json
{"domain":"example.com","data":["93.184.216.34"],"expiresTime":1757200060.2,"server":"udp://1.1.1.1:53","stale":false,"negative":false}
```

`expiresTime` 与 `startTime` 同单位（Unix 秒，`f64`）；条目没有剩余 TTL 信息时为 `null`。

### GeoIP

阶段 1 补遗（M5a）起，`GET /v1/geoip?ip=<地址>`（Surge iOS 5.23 / Mac 6.10 起的端点）用 `GEOIP`、`IP-ASN` 规则在用的两个库查一个地址；更新器换上新文件之后立刻是新的。`ip` 是 IPv4 或 IPv6 地址。

```json
{"address":"198.51.100.23","country":"SE","asn":64500,"organization":"Example Network","geoip-db-date":1760000000,"asn-db-date":1760000000}
```

- `address`：解析之后的样子（`2001:0218:0:0:0:0:0:1` 回显 `2001:218::1`）；IPv4 映射的地址（`::ffff:198.51.100.23`）按它指的 IPv4 地址查，回显也是 IPv4。
- `country`：国家库给的 ISO 3166 代码；`asn`：数字；`organization`：ASN 库里的组织名。库里没有这个地址的项，或对应的库不在（数据目录的 `geoip/` 里没有那个文件）时是 `null`。
- `geoip-db-date`、`asn-db-date`：两个库自己带的构建时间（MMDB 元数据的 `build_epoch`），UNIX 秒；库不在时是 `null`，地址在库里没有项时日期仍在。手册只说是"the UNIX timestamps of the databases in use"，没有说是构建时间还是下载时间——登记的差异。
- 错误：`ip` 不是 IP 地址（含空串）→ 400 `{"error":"ip is not a valid IP address"}`，不回显取值；缺 `ip` → 400，正文是查询串拒绝的原文（含 `` `ip` ``）；鉴权同其它端点（没有 `X-Key` → 401）。

### 外部资源

阶段 1 补遗（M5b）起，`GET /v1/external_resources` 列出 rurge 下载或读取的外部资源，`POST /v1/external_resources/update` 立刻更新其中一个或全部。这是 Surge iOS 5.23 / Mac 6.10 起的两个端点（手册 `tools/http-api.html`；与手册的差异见下面的表和兼容性清单 10.4 节）。两个端点都要 `X-Key`，引擎因 `strict` 拒绝会话时也照常应答。没有对应的 CLI 子命令（阶段 6）。

**列表。** 例子里的键是例子地址的 SHA-256（托管 Profile 是 `https://provider.example/surge/profile.conf` 的，规则集是 `https://sets.example/rules.list` 的，GeoIP 库是 `https://geo.example/country.mmdb` 的）：

```json
{"defines":[
{"path":"managed profile (https://provider.example/…)","type":"profile","key":"d775041d9c0eb39635b2af13c467b6b63615ed529d1fce8378659eba977f5b8f","local":false,"ready":true,"updatedAt":1760000000,"updating":false,"fromModule":null},
{"path":"https://sets.example/rules.list","type":"ruleset","key":"2482fa3056438fe3af4f5001345a9fb087645067a0280f98ce4835b64bb40032","local":false,"ready":true,"updatedAt":1760000000,"updating":false,"fromModule":null,"skipped":2,"truncated":0,"unknownParams":2},
{"path":"GeoLite2-Country.mmdb","type":"geoip","key":"681e0219e99b208ea9fd34996cff3506e385fd4a116b0b70afe67ec1d4a2ea56","local":false,"ready":false,"updatedAt":0,"updating":false,"fromModule":null,"error":"http status 404"}
]}
```

| 字段 | rurge | 与手册的差异 |
| --- | --- | --- |
| `path` | 资源叫什么。规则集与域名集是写下的地址，其中的用户名口令显示为 `***`，路径与查询串照写的样子（与 `GET /v1/rules` 和日志里一样）；订阅（`policy-path`）是 ``policy-path of `<组名>` ``，地址是凭据，不显示；两个 GeoIP 库是文件名；托管 Profile 是 ``managed profile (<scheme>://<host>[:port]/…)``；远程 `#!include` 是 ``remote include <键的前 8 位> (<scheme>://<host>[:port]/…)``；本地文件是它的路径 | 手册的例子是资源的 URL。托管 Profile 与远程 include 的链接整个是凭据（服务商发的链接带令牌），日志、诊断与这个端点都只显示 `scheme://host[:port]/…` |
| `type` | `ruleset`、`domainset`、`policy-group`（`policy-path` 订阅），另有 rurge 自己的三个：`geoip`（两个 GeoIP 库）、`profile`（托管 Profile）、`include`（远程 `#!include`） | 手册："One of `ruleset`, `domainset`, `script`, `policy-group` and `data`."；rurge 没有 `script` 与 `data`（脚本在阶段 5，Map Local 的数据在阶段 4） |
| `key` | 地址的 SHA-256，64 位小写十六进制（本地文件取 `file:<路径>` 的 SHA-256），也是缓存目录 `<数据目录>/resources/<key>/` 的名字。在列表里不唯一：一个地址既是 Profile 的来源又是规则集时，列出两项（`type` 不同）、一个键 | 手册只说是资源的标识、更新端点用它（例子是 32 位十六进制）；算法是 rurge 自己的 |
| `local` | 资源是本地文件 | 同手册 |
| `ready` | rurge 手里有这份资源的内容；最近一次更新失败而留着旧内容的也算 | 手册："Whether the resource content is available." |
| `updatedAt` | 最近一次成功的**检查**的 UNIX 秒，从没有过是 `0`。服务端答"没有变化"（HTTP 304，或内容相同的 200）也算一次成功。本地文件没有这一项 | 手册："The UNIX timestamp of the last successful download, or 0 if the resource has never been downloaded."；本地文件没有这一项同手册（"Local resources have no `updatedAt` field."） |
| `updating` | 有一次下载正在进行，或它带来的内容正在检查；已经被要求而还没开始的尝试不算 | 手册没有解释这个字段 |
| `fromModule` | 恒为 `null`，到有模块（阶段 5）为止 | 手册："The name of the module that references the resource, or null if it's from the profile." |
| `error` | 只在最近一次更新或解析失败、而且那是最近发生的事时才有；文字见下 | 手册："Present only if the last update or parsing failed." |
| `suspended` | rurge 自己的：引擎因 `strict` 拒绝会话时，那一行写了 `strict=true` 的托管 Profile 那一项是 `true`，别处没有这个键——列表里另有一项 `profile`（一次失败的重载之后 rurge 去下载的那一份，见下「顺序与范围」）时，它也没有 | 手册没有 |
| `skipped`、`truncated`、`unknownParams` | rurge 自己的：只在 `ruleset` 与 `domainset` 的项上，见下「集合里没有用上的行」 | 手册没有 |

**顺序与范围。** 先是 Profile 的来源（托管 Profile 在前，再是远程 `#!include`，按被跟踪的次序；在用的一代之外还被跟着的来源——一个还没被采用的新版本提到的，或一次失败的重载之后 rurge 去下载的（磁盘上的 Profile 点名而从没下载过的，兼容性清单 1.3 节）——`type` 照它在 Profile 里的身份：是 `include` 的排在这些之后，是 `profile` 的排在在用的那一份托管 Profile 之后、各份 `#!include` 之前（列表里于是有两项 `profile`，在用的那一份在前）；第一份副本被采用之前 `ready` 是 `false`，下载失败时带 `error`；失败的重载之后跟上的那些从不带 `suspended`），然后是当前这一代的资源，按 `path`、再按 `key`；手册没有定义顺序。监视着的 hosts 文件不是外部资源，不列，也不能按键更新。

**`error` 的文字。** 都是 rurge 自己的话，不含链接的任何部分（链接至多以 `scheme://host[:port]/…` 出现）。使用者自己的本地文件的路径可以在里面，两处：本地的规则集、域名集或 `policy-path` 文件读不了时（`cannot read <路径>: …`），和 `does not load:` 后面接的那条错误说到一个本地文件时（比如本地 Profile 自己 include 的文件读不了）；读得到这个端点的人拿着 API key。

| 文字 | 什么时候 |
| --- | --- |
| `http status N` | 服务端答了 200 与 304 以外的状态 N |
| `timeout`、`connect: <原因>`、`tls: <原因>`、`too many redirects`、`body exceeds N bytes`、`protocol: <原因>`、`invalid url: <原因>` | 下载本身失败；一份资源的正文至多 64 MiB。`protocol:` 是请求发出之后出的错——连接中途断了、应答不合 HTTP、正文读到一半断了——，或从 `https://` 被重定向到 `http://`（`protocol: refusing to redirect from https to http`）；`invalid url:` 是地址（重定向之后的也算）不是 `http(s)`（`invalid url: unsupported scheme`），或 HTTP 库读不成地址。除了括号里这两句固定的话，这两类冒号后面是 HTTP 库的话，rurge 不往里放链接 |
| `does not load: <首个错误的文字>`，错误在下载到的文件里时接 ` (line N)`；`does not load: the local profile cannot be read (<io 错误的种类>)` | 托管 Profile 或远程 include 下载到的内容与本地 Profile 一起加载不了（常见的是一页登录页或报错页：缺 `FINAL`）。不采用，缓存里上一份照旧生效，日志是 WARN `downloaded resource not adopted` |
| `the profile no longer refers to this address` | 下载到的内容属于一个 Profile 现在已经不提的地址 |
| `lacks the section it is included into` | 远程 include 下载到的内容没有它被 include 进的那个节；通配命名节（`[Ruleset *]` 等）下，是写下来连一个节都没有的正文（一页登录页、空的正文） |
| `the downloaded file is not UTF-8 text` | 下载到的不是 UTF-8 文本 |
| `waiting for the sources this version refers to` | 新版本提到一个还没试过下载的来源，在等它试完第一次再一并采用；这不算失败 |
| `cannot write the cache: <系统的错误文本>` | 检查通过了而缓存写不进去（数据目录写满或只读）；这次尝试按失败算，什么也不采用 |
| `the server answered "not modified" but nothing is cached` | 服务端对一个没有带校验器的请求答了 304，而手里没有这份资源；按一次失败的下载算 |
| `offline mode: no cached copy` | `--no-network` 而缓存里没有这份资源；不发出任何请求 |
| `the check of the downloaded copy did not finish` | 对下载到的内容的检查自己出了故障、没有做完；这次尝试按失败算，什么也不采用 |
| `cannot read <路径>: <系统的错误文本>` | 本地文件——规则集、域名集或 `policy-path` 指的文件——读不了；路径是 rurge 去读的那一个 |
| `the update has not finished`、`offline mode: the update cannot be attempted` | 只在带 `suspended` 的那一项托管 Profile 上：会话因 `strict` 被拒、而它自己没有失败的更新可说时，说被拒的原因——本次运行的第一次尝试结束之前，是等待结束时的原因或 `the update has not finished`；`--no-network` 时是后一句，那时从不尝试。它自己失败的更新排在前面 |

更新成功之后至多一秒，这一项仍带 `suspended` 与原因，直到主循环的下一眼；反过来，一次重载去掉了那一行的 `strict` 之后的至多一秒里，会话仍被拒，而列表里没有一项带 `suspended`。

**集合里没有用上的行。** `ruleset` 与 `domainset` 的项多三个数：`skipped`（既不是规则也不是名字的行，不在集合里；下载到的文本里点名本地文件的行也算）、`truncated`（超过一百万条上限的有效行，不在集合里）、`unknownParams`（带着不认识的参数的规则行：行在集合里，参数不在；域名集没有参数，恒为 0）。一行只算一类，行号从 1 起，注释行与空行也算行。它们是集合上一次编译所依据的内容的数：更新之后到集合轮到编译之前（同一代的集合一次编一个），仍是更新之前内容的数；没有编译过的集合是三个 0。一份有几行读不了的集合是载入了的，所以这三个数不让 `error` 出现。同样的数在日志里是 WARN `set loaded with lines that are not used`（启动之后载入的集合，三个数不全为零且与上一次载入的不同时才说），在诊断里是 `W0023`、`W0024` 与 `W0003`（兼容性清单 3.5 节）。

**更新。** 请求 `{"key":"<key>"}` 更新那一个键，`{"key":"all"}` 更新每一个下载得到的资源：托管 Profile、远程 `#!include`、规则集、域名集、订阅与两个 GeoIP 库，每个地址只更新一次，下载同时发出（不设并发上限，与冷启动相同）。本地文件不在 `all` 里，同手册："Local resources are skipped."；按键点名一个本地文件时，要求重读它并答 `"success"`——这个 `"success"` 的意思是"已经要求重读"，文件读不了时也是它，原因在列表里那一项的 `error` 上（手册没有说这种情形）。应答是一个对象，把每个更新了的键映射到 `"success"` 或原因（原因是上面 `error` 的那些话，另有只在这里出现的三句：`the resource is no longer kept`——列表里刚才还有、这一刻已经不在的键，比如其间来了一次重载；`the resource is no longer updated`——那一项的下载任务已经结束，或等待当中这个来源被撤掉；`the update did not finish`——那个键的更新没有带着答案回来），与手册相同："The response maps each updated key to `"success"` or an error message."；对象不保持列表的顺序。

```json
{"d775041d9c0eb39635b2af13c467b6b63615ed529d1fce8378659eba977f5b8f":"success","681e0219e99b208ea9fd34996cff3506e385fd4a116b0b70afe67ec1d4a2ea56":"http status 404"}
```

- **等多久。** 请求等着那些尝试做完，整个请求至多 60 秒，到时还没答的键是 `"timed out"`，别的键保留各自的答案。它只等尝试本身，不等之后的事：不等集合的重新编译，也不等托管 Profile 更新之后的重载（守护进程在更新成功后约一秒重载，应答不等它）。调用方走了，等待就此结束，下载不受影响。
- **离线。** `--no-network` 时每个下载得到的资源的答案都是 `offline mode: nothing is downloaded`，不发出任何请求。
- **新版本在等别的来源。** 托管 Profile 的新版本提到一个从没下载过的来源（比如多了一行远程 `#!include`）时，不会先采用缺着那一块的版本：它等那个来源试完第一次再一并采用，其间那个键的答案是 `waiting for the sources this version refers to`。
- **同一个地址在两处。** 一个地址既是 Profile 的来源又是规则集时是一个键，两边都更新，答案是两边的第一个错误（Profile 的在先）。
- **400。** 正文不是带字符串 `key` 的 JSON 对象 → `{"error":"the body must be a JSON object with a string \"key\""}`；键不存在（含空串）→ `{"error":"no external resource has this key"}`，手册："If the key is not found, a 400 error is returned."。两句都是固定的，不重复请求里的任何东西。

**链接里的用户名口令。** 下载得到的资源——规则集、域名集、`policy-path` 订阅、两个 GeoIP 库、托管 Profile 与远程 `#!include`——的链接里写了用户名口令（`https://user:pass@host/…`）时，rurge 把它们百分号解码后作为 `Authorization: Basic …` 发出，并从请求的目标里去掉（阶段 1 补遗（M5b）起；手册没有写明 Surge 发不发，代项目所有者作的裁定）。它们只带到同一来源（方案、主机、端口都相同）的重定向上；`http://` 的链接上它们在网络上是明文，rurge 不拒绝也不警告。细节与代价见兼容性清单 3.1 节 `RULE-SET` 一行。

**界限，都没有补上的。**

- `all` 包括托管 Profile：本地文件在上次更新之后被改过时，它生效到下一次成功的检查为止，"全部更新"就是这样一次检查——约一秒后守护进程重载到下载的副本，那处改动不再生效。
- 更新请求等着的时候来了重载（托管 Profile 变了，一秒后重载，而 `all` 里的集合还在下载）：换代时 rurge 对旧一代刚检查过或正在检查的资源再问一次，免得键已答 `"success"` 而在用的一代仍是旧内容；请求在另一代下结束时，让在用的一代再看一次。够不到的：新一代已经在下载的资源（留给那次尝试，它可能在服务端变之前就发出了）、被丢掉的请求（客户端走了）、时钟被拨回后的检查；这几种情形下，在用的一代要等有人再看一次，或资源自己的下一次刷新。
- 列表里的数落后于下载：一个更新后立刻读列表的客户端，可能看到新的 `updatedAt` 配着集合旧的三个数。
- 一次失败的强制尝试使条目自己的退避加倍（Profile 的来源至多 600 秒，别的至多 3600 秒）：服务端挂着的时候一再要求更新，会把守护进程自己的下一次重试往后推；服务端好了之后要求的更新立即通过。

### Profile 的当前文本与检查

**`GET /v1/profiles/current`** 返回的是**生效的主文本**（阶段 1 补遗（M5b））：托管 Profile 的副本生效时是数据目录里下载的副本，不是指向它的本地文件；本地文件生效时——它在上次更新之后被改过，生效到下一次成功的检查为止——是本地文件。哪一份在生效，每次加载时选，`rurge check` 的 `I0004` 说（规则见兼容性清单 1.3 节）。文本是在请求的那一刻从文件里读的，所以在重载之前、以及一次失败的重载之后，显示的是已经写在磁盘上的文本，同对一个改了而没重载的本地文件；读缓存里的副本时不查摘要，损坏的副本照原样显示。缓存里的副本被删掉之后答 500 `{"error":"cannot read the profile: <系统的错误文本>"}`（文本里没有路径），直到下一次成功的更新把副本写回。脱敏的规则见下面「脱敏后的配置」。

**`POST /v1/profiles/check`**（手册：Mac 4.0.6+）检查一份 Profile，不影响运行中的配置。

- **应答。** `{"ok":true,"errors":0,"warnings":1,"diagnostics":[…],"error":null}`：五个键，`error` 总是写出。`ok`、`errors`、`warnings`、`diagnostics` 的名字、类型与意思没变；`error` 是阶段 1 补遗（M5b）照手册加的：手册写 "If the profile is invalid, an error is returned. Otherwise, the `error` field is null."。Profile 没有严重程度为 error 的诊断时它是 `null`（告警不算）；否则是一个字符串：`diagnostics` 里第一条错误的一行文字 `error[<码>] <文件>:<行>: <说明>`，还有别的错误时接 ` (and N more)`。**这个措辞是 rurge 自己的**：手册只写了"an error is returned"，没有写错误是什么样子。读了且检查了的 Profile 一律答 200（有错误时 `ok` 为 `false`），读不了的仍答 500 `{"error":"cannot load the profile: <原因>"}`。
- **请求。** 没有正文、正文只有 JSON 空白、或是不带 `name` 的 JSON 对象（别的键忽略），检查正在运行的那份 Profile。`name` 是字符串时，与正在运行的 Profile 的文件名、以及去掉最后一个扩展名的文件名比较（`config.conf` 与 `config`；完全相等，不折叠大小写，从不当路径用；托管 Profile 时是本地文件的名字）：相符才检查，不相符是 404 `{"error":"profile not found"}`，什么也没加载。为什么拒绝而不是忽略：照手册的例子读（`{"name": "Profile2"}`；旁边的 `GET /v1/profiles` 是"Get all available profile names."），Surge 用 `name` 在几份 Profile 里选一份检查（没有在 Surge 上核对），rurge 只有一份（`-c` 给的文件），忽略一个不认识的名字的话，应答说的就是一份没被问到的 Profile。按 `name` 在几份 Profile 里选一份，随 `GET /v1/profiles` 与 `POST /v1/profiles/switch` 在阶段 6。
- **400。** 正文不是 JSON 对象，或 `name` 不是字符串，或正文大到读不了（超过 2 MiB）：`{"error":"the body must be a JSON object; \"name\", when given, must be a string"}`，一句固定的话。404 与 400 的文字都不重复请求里的任何东西，都在加载 Profile 之前给出；请求的 Content-Type 不看（脚本的 HTTP 库怎样标注正文，这里不管）。
- **读正文。** 这个端点读正文：在正文到齐之前不答复（此前只凭请求头就答复）。发来的正文远超 2 MiB 的发送者，在 Windows 上可能丢掉 400——那是上面「API 连接的收尾」的 256 KiB 界限，不是这个端点新有的，读正文的端点都一样。
- **读什么。** 磁盘上的本地文件，连同数据目录里已下载的 Profile 来源（托管 Profile 的副本、远程 `#!include` 的副本）与已缓存的订阅，读法同守护进程；从不联网，对数据目录只读不写。还没有下载的来源报 `W0022`（``managed profile not downloaded yet`` 与 ``remote include not downloaded yet``，有原因时接 ``: <原因>``）。自阶段 2 / M1 起含干构建，见 `docs/api/phase2.md`。

### 脱敏后的配置

`GET /v1/profiles/current` 不带 `sensitive=1` 时给出的文本。阶段 2 / M8c 重写了这套规则，下面写的是代码现在的做法（`crates/rurge-config/src/redact.rs`）；同一套规则里管策略定义的那一半也用于 `GET /v1/policies/detail` 与 `GET /v1/policy_groups` 的 `lineHash`（`docs/api/phase2.md`）。

**先说它不是什么。** 脱敏是尽力而为，不是"没有秘密会出现"的保证：它是一张"藏什么"的清单，照各个解析器读一行的方式去认，清单之外的键、读法之外的写法它认不出。能取到这个视图的人拿着 API key，本来就能用 `sensitive=1` 取到原文——所以它防的是一张截图、一次粘贴、一份贴出去的报告，不是防 API 的调用方。**把这份文本给别人看之前，自己再读一遍**，对照下面"有意照原样显示的"与"已知没有覆盖的"两段。清单是照 Surge 手册里放秘密的地方列的——口令、密钥、订阅与托管配置的链接、URL 里的用户名口令——，而且只是"目前所知"：两轮修正之后它还漏着手册里的一个键（`[MTProto]` 的 `secret`，已补上）。这些地方之外不要指望它：规则集 URL 的查询串里带 token、秘密写在自定义的头或重写规则里、配置里混着为别的客户端写的行，都会照原样显示。要不要换成"只显示已知无害内容"的视图是留给项目所有者的问题，还没有决定（M8c 计划 `docs/superpowers/plans/2026-10-09-phase2-m8c-leftovers-acceptance-plan.md`「延后事项」第 51 行）。

**怎样读一行。** 照节解析器的顺序：行首的空白（连同字节顺序标记与零宽字符）、注释标记（`#`（后面不是 `!`）、`;`、`//`，连着几个也算）与行首的要求指令（`#!MACOS-ONLY`、`#!REQUIREMENT <表达式>` 等）原样保留；行尾的行内注释与要求指令照解析器的切法切开；剩下的内容按它的键或它的定义脱敏。被注释掉的行与行尾注释里的文字不是原样放过，而是照"没有被注释"那样再脱敏一遍——旧口令常留在那里；注释里再套注释跟到四层，更深的只按名字搜索。秘密键的取值为空而行尾有注释时，注释的文字整个换成 `***`（以 `#` 开头的口令会被解析器当成注释）。行数、行的顺序与行尾的 CRLF 不变。

**整行键**（`key = value`，整个取值换成 `***`）：`password` `ca-passphrase` `ca-p12` `private-key` `psk` `pre-shared-key` `preshared-key` `token` `auth-key` `proxy-test-url` `internet-test-url` `geoip-maxmind-url` `secret` `passphrase`。键名不分大小写，也不计 `-` 与 `_`（`PrivateKey`、`Pre_Shared_Key` 都算）。键是分隔符前面那段文字的**结尾**：分隔符是第一个 `=`（或全角的 `＝`），以及它之前的 `:`（或 `：`）；键的前面可以另有文字，只要键名从头开始，或接在空白、逗号、`(`、引号或任何非 ASCII 字符之后；`=` 之前有顶层逗号的不是键行。另有四类键各有各的处理：

| 键 | 处理 |
| --- | --- |
| `http-api` `external-controller-access` | 取值在最后一个 `@` 处切开，之前的换成 `***`；没有 `@` 时整个换掉 |
| `http-listen` `socks5-listen` | 逐项去掉最后一个 `@` 之前的部分 |
| `wifi-access-http-auth` | `user:***`；没有 `:` 时整个换掉 |
| `dns-server` `encrypted-dns-server` `doh-server` | 逐项去掉 URL 里的用户名口令（全角的 `＠` 也算）。只在键的前面除空白外没有别的文字时才算键行，否则（`[SSID Setting]` 的行）按策略定义读 |

**行内参数**（`name=value`，取值换成 `***`）：`password` `passphrase` `psk` `private-key` `pre-shared-key` `preshared-key` `base64` `token` `uuid` `username` `headers` `ws-headers` `ws-path` `shadow-tls-password` `salamander-password` `gecko-password` `policy-path` `external-policy-modifier` `test-url` `url` `args` `argument` `obfs-host` `obfs-uri`。参数照解析器的读法认：整项写在引号里的 `"password=x"` 是 `password` 参数；括号组（`[WireGuard]` 的 `peer = (…)`）逐项处理；取值到解析器自己的第一个顶层逗号为止——`"` 或 `'` 在值里任何位置都会开启一段引号（`"` 内 `\` 转义下一个字符），`(` / `)` 分组，引号内与括号内的逗号属于取值，引号或括号没闭合时到行尾。读不成这样一项的文字，以及注释，再按名字搜索一遍：名字前面是行首、空白、逗号、`(`、引号或非 ASCII 字符，后面是 `=`、`＝`、`:`、`：`（中间可隔一个收尾的引号）。其中几个是有意宽藏的：`username` 连带藏起无害的 SSH 用户名；`headers` / `ws-headers` 连 header 名也不保留；`ws-path` 与 `obfs-uri` 在按路径分流的前置后面就是共享密钥；`obfs-host` 可能认得出用户；`policy-path`、`test-url` 与组行上的旧参数 `url` 常带订阅的 token；`external-policy-modifier` 能设任何参数；`external` 的 `args` 与 `[Script]` 的 `argument` 里常有口令或 cookie。

**位置凭据**：类型带服务器与端口的策略行（`http` `https` `socks5` `socks5-tls` `ss` `snell` `vmess` `trojan` `anytls` `ssh` 等——`wireguard`、`tailscale`、`external` 与内置策略的别名以外的全部类型），以及第三个字段是端口号的任何行（为别的客户端写的行，其类型 rurge 不认识），第四个字段起凡不是 `name=value` 具名参数的字段一律换成 `***`。含 `=` 的字段，要 `=` 前面是一个参数名、后面非空且不全是 `=` 才算具名参数（`tfo=true` 保留；带填充的 Base64 `aHVudGVyMg==` 换掉；`sni=` 这样值为空的也被换掉）；以引号开头的字段整个算一个位置值。类型带服务器的行上，端口位置不是数字的字段也换掉（漏了逗号的 `8080 alice`）。类型后面漏了逗号的行（`http h.example, 8080, …`），只在拆开之后第三个字段是端口号时才这样读。全角的 `，` 在这里当逗号。`http` `https` `socks5` `socks5-tls` 与 `h2-connect` 的位置凭据是解析器接受的写法；其余类型这个位置本就是多余参数（`W0001`），按偏安全一侧一并换掉。

**URL**：任何位置的 `scheme://authority` 里，authority 中最后一个 `@` 之前的部分换成 `***`（路径、查询串与片段里的 `@` 不动）。scheme 不是 `http` `https` `h3` `quic` `tls` `tcp` 这六种之一的 URL——分享链接 `trojan://…`、`ss://…`、`vmess://…` 把秘密放在哪里各有各的写法——从 `://` 之后到下一个空白整段换成 `***`。scheme 指 `://` 前面紧挨着的那串字母、数字、`+`、`-`、`.` 里从第一个字母起的部分（scheme 以字母开头，RFC 3986）：`1password://vault/item` 显示成 `1password://***`，`3http://u:p@h.test/x` 显示成 `3http://***@h.test/x`；`://` 前面没有这样一串的（正则 `^https?://`、`^[a-z]+://`、`^.+://`）不算带 scheme 的 URL，只藏 authority 里的用户名口令。注释里那六种 scheme 的 URL 只显示 scheme 与 authority，其后换成 `/***`。

**`#!` 行**按形状：`#!MANAGED-CONFIG` 之后只显示写成 `interval=<数字>`、`strict=<字母>` 的词，其余每个词换成 `***`（指令名写错的，如 `#!MANAGED_CONFIG`、`#MANAGED-CONFIG`，同样处理）；`#!include` 按目标，含 `://` 的换成 `***`，本地路径保留；别的 `#!` 行里含 `://` 的词换成 `***`。

例子（每一对都是 `redact.rs` 里的用例钉住的）：

| 配置里的行 | 显示为 |
| --- | --- |
| `http-api = s3cret@127.0.0.1:6171` | `http-api = ***@127.0.0.1:6171` |
| `wifi-access-http-auth = alice:hunter2` | `wifi-access-http-auth = alice:***` |
| `encrypted-dns-server = https://al1ce:pw@dns.example/dns-query, quic://dns.example` | `encrypted-dns-server = https://***@dns.example/dns-query, quic://dns.example` |
| `ProxyA = https, 1.2.3.4, 443, carol, trustno1` | `ProxyA = https, 1.2.3.4, 443, ***, ***` |
| `P = http h.example, 8080, alice, hunter2` | `P = http h.example, 8080, ***, ***` |
| `peer = (pre-shared-key = PSK1, endpoint = 1.2.3.4:51820)` | `peer = (pre-shared-key = ***, endpoint = 1.2.3.4:51820)` |
| `G = url-test, A, B, url=http://t.test/?token=t0k3n, interval=600` | `G = url-test, A, B, url=***, interval=600` |
| `# old: ca-passphrase = hunter2` | `# old: ca-passphrase = ***` |
| `PosC = socks5, s.test, 1080, alice, hunter2 # expires=2027-01` | `PosC = socks5, s.test, 1080, ***, *** # expires=2027-01` |
| `RULE-SET,https://al1ce:hunter2@rules.example/a.list,Proxy` | `RULE-SET,https://***@rules.example/a.list,Proxy` |
| `trojan://hunter2@t.example:443?sni=t.example#HK` | `trojan://***` |
| `# 订阅: https://sub.example/api/v1/client/subscribe?token=TOKEN` | `# 订阅: https://sub.example/***` |
| `#!MANAGED-CONFIG https://provider.test/link/t0k3n?surge=4 interval=43200 strict=true` | `#!MANAGED-CONFIG *** interval=43200 strict=true` |
| `#!include a.conf, https://h.example/p?token=t0k3n` | `#!include a.conf, ***` |

**有意多藏的**（不是缺陷，换来的是少漏）：名字以清单里的键结尾的策略、组或脚本——名叫 `HK Token` 的策略，整行显示成 `HK Token = ***`；散文里 `password:`、`token:`、`url:`、`args:` 之后的文字（`# see url: https://x.example/y` 显示成 `# see url: ***`）；以协议类型开头、第三个词恰是端口号的注释；注释里 URL 的路径；scheme 不是那六种的整个 URL——URL 重写里指向 App 的目标也在内（`myapp://item/$1` 显示成 `myapp://***`）——这一段一直藏到下一个空白，紧跟的逗号也在内：`external, exec=ssh://git@h.test/x, local-port=1080` 显示成 `external, exec=ssh://*** local-port=1080`；`://` 前面恰好是一串以字母开头的 scheme 字符的正则——`^\w+://example\.com/x` 显示成 `^\w+://***`，`w+` 被读成了 scheme。

**有意照原样显示的**（登记现状，不是改动的承诺；要不要藏、怎样藏由项目所有者定，同一张「延后事项」的第 43 行）：

- 不是注释的行上，规则集与域名集的 URL，连同路径与查询串。规则集在规则列表、请求记录与日志里到处以 URL 命名（与 Surge 一样），只在这里藏起来没有用；**查询串里的 token 因此看得见**。URL 里的用户名口令仍然藏。
- `encrypted-dns-server` 各项 URL 的路径与查询串：它们是上游名字的一部分。
- `[General]` 里 rurge 不认识的键的取值。
- `[Script]` 的 `script-path` 的 URL（用户名口令除外）。
- `[URL Rewrite]`、`[Body Rewrite]` 各行的文字，`[Header Rewrite]` 写进去的头值，`[Map Local]` 的 `header=` / `data=`。

**已知没有覆盖的。** 位置口令里带 `=`（`P = http, h.example, 8080, alice, Xk9=mQ2p` 的最后一个字段，对解析器和对脱敏都读成参数）；名字本身就是整行键的策略（`DNS-Server = trojan, …, password=…`）；没有任何分隔符的秘密键（`ca-passphrase hunter2`）；行内参数名里以 `_` 代 `-`（`policy_path=…`）；为别的客户端写的格式（Loon 的 `peers=[{…}]`、Clash 的 YAML、Quantumult X 的节、整份 Base64 的订阅）；折成几行的 Base64；滑进 `sni=`、`interface=` 这类不是秘密的参数里的秘密；URL 的用户信息里带不加引号逗号的口令（`https://user:pa,ss@dns.example/…`）；带空白又不加引号的要求表达式后面跟一个键（`#!REQUIREMENT CORE_VERSION >= 20 http-api = KEY@…`）；`external` 策略写在 `exec=` 里的口令（`exec="sshpass -p hunter2 ssh …"`）；等等。逐条列在 M8c 计划「延后事项」的第 58 ～ 77 行。三轮修正与两次对抗性复审之后，第二位复审者的判断是：这样一张清单对这种格式做不成可靠的，写法没有边界（同表第 51 行）——所以上面那句"不是保证"要按字面读。

## 系统代理

M4b 起 `system_proxy` 已生效（见上面「端点」表）；这里记录地址取值、`skip-proxy` 转换与生命周期。

- **地址**：http / https 取第一个 `http-listen`；`set-system-socks-proxy = true`（默认）时 socks 取第一个 `socks5-listen`，为 false 时不设置 socks。监听地址是通配地址时换成**同族**回环：`0.0.0.0` → `127.0.0.1`，`::` → `::1`（`[::]` 在 Windows 上默认只监听 v6，`127.0.0.1` 连不上）。一个 http/socks5 监听器都没有时视为「未启用」，`POST` 打开会失败。
- **`skip-proxy` → 系统绕过列表**：采用 macOS 语义。取反项（`-host`）与 `<…>` 特殊记号（`<ip-address>` 等）整条丢弃；端口一律丢弃（`host:port` 只留主机部分）；CIDR 网段保留 `/前缀`、主机位清零（如 `192.168.1.5/16` → `192.168.0.0/16`），单地址网段（`/32`、`/128`）写成裸地址。macOS 与 Linux（GNOME `ignore-hosts` / KDE 逗号拼接的 `NoProxyFor`）原样使用这份列表；Windows 的 `ProxyOverride` 没有 CIDR 语法，把 IPv4 网段展开成通配符模式（`10.0.0.0/8` → `10.*`；`172.16.0.0/12` 展开成 16 条 `172.16.*`…`172.31.*`；单条 `/1`、`/9`、`/17` 或 `/25` 网段最多展开到 128 条，没有上限），IPv6 字面量加中括号（`::1` → `[::1]`），IPv6 网段无法表示、丢弃。`exclude-simple-hostnames` 只在 Windows 生效（追加 `<local>`）；macOS 经 `networksetup` 无法设置，`true` 时 WARN 并忽略这一项，其余设置照常应用；Linux 无对应项。
- **`state.json` 备份与崩溃恢复**：开启前先 `snapshot()` 当前系统设置，连同 `features.system_proxy = true` 一起写入 `state.json`（先于 `apply`，这样崩溃后仍能找回原始设置；已经存在的备份不会被覆盖——它就是最初的原值）；`apply` 失败会尽力 `restore` 并清空这两个字段。关闭或优雅退出 → `restore` 并清空。启动时若 `system_proxy_backup` 非空（上一次异常退出留下的）→ 先 `restore` 并打一条 WARN，再按本次是否带 `--system-proxy` 决定要不要重新开启（详见下一条）；`state.json` 的写入是尽力而为（原子写但不 `fsync`），写失败只记日志，不影响调用方。
- **崩溃恢复不依赖本次能否启动**：任何一次 `rurge run` 都做这件事，**不需要**带 `--system-proxy`——只要 `state.json` 里有非空的 `system_proxy_backup` 就恢复。这一步紧跟打开 `state.json` 之后，**早于**监听器绑定与 `http-api` 绑定：上一次崩溃留下的端口往往正是本次绑不上的原因，所以即使本次以 `error: cannot bind listener`（或 `cannot bind http-api`）退出 1，崩溃前的系统设置也已经放回去了；否则机器会一直指着一个死掉的端口，而且每次重试都卡在同一处。排在恢复之前的只剩加载配置、打开日志文件、解析运行时参数（建数据目录）和取数据目录的实例锁（见下面"一台机器只跑一个实例"一条）这几步：实例锁被另一个还在运行的实例占着时退出 1——那份备份属于它，本来就不该由别的进程恢复；数据目录建不出来时退出 2，那里也就没有 `state.json` 可恢复；配置加载不了或日志文件打不开时退出 2，自阶段 2 / M8c 起在退出之前另做一次恢复，见下一条（此前要等下一次能走到这一步的启动）。
- **配置加载不了时也恢复（阶段 2 / M8c）**：`rurge run` 在日志建起来之前就结束的三个出口——配置读不了、配置有错误、配置能加载但 `--log-file`（`RURGE_LOG_FILE`）打不开——都以 2 退出，退出之前都先试一次恢复。此前这三个出口不恢复：配置坏着的时候，机器一直指着一个死掉的端口，直到把配置修好。条件依次是：数据目录（照 `--data-dir` / `RURGE_DATA_DIR` / 平台缺省取，**不创建**）里有 `state.json`；**拿到**实例锁——别的实例持着，或者锁文件打不开、锁不了，都不做、也不出声（与正常启动不同：正常启动锁不了时不带锁照跑；这里进程反正要退出，备份留给走得更远的那次启动）；`state.json` 里有备份（只读地看：读不了或解析不了的文件原样留着，不像正常启动那样把它挪开，也不出声）。然后取后端、恢复、清掉备份。结果写到标准错误（这时还没有日志），不改变退出码，是下面四句之一：
  - `warning: a previous run left the system proxy pointing at rurge; the saved settings were restored`——已恢复，备份已清掉。
  - `warning: a previous run left the system proxy pointing at rurge; the saved settings were restored, but state.json could not be updated: the next start restores them again`——已恢复，但 `state.json` 没有写成：下一次启动会再恢复一遍，盖掉其间对系统设置的改动。
  - `warning: cannot restore the system proxy: <原因>; the saved settings stay in state.json and are restored on the next start`——恢复失败；`<原因>` 是后端自己的错误文本（macOS / Linux 上是失败的命令，它的参数里最后一个 `@` 之前的部分显示为 `***`）。
  - `warning: <后端的报错>; the system proxy was not restored`——取不到后端（只在测试钩子 `RURGE_SYSTEM_PROXY_BACKEND` 的取值不对时）。

  输出的先后：配置有错误时诊断在前、告警在后；配置读不了时告警在前、`error: cannot read …` 在后；日志文件打不开时告警在前、`error: cannot open log file <路径>` 在后（日志目录列不了时，日志库自己先往标准错误打一行）。没有 `state.json` 的目录里什么也不多出来（连 `rurge.lock` 也没有），不存在的数据目录不被创建；恢复期间这个进程持着 `rurge.lock`。自动化用例只经测试用的文件后端：真实的后端（Windows 注册表、macOS `networksetup`、Linux 的 `gsettings` / `kwriteconfig`）没有在这条路径上跑过，手工的一项列在 `docs/acceptance/phase1-manual.md`。
- **带 `--system-proxy` 时一台机器只跑一个实例**：备份存在各自的 `--data-dir` 的 `state.json` 里。两个实例用**不同**的数据目录先后开启系统代理时，第二个实例会把第一个实例刚写进去的地址当成「原始设置」快照下来；两个都退出后，系统设置停在一个已经失效的地址上，而两份 `state.json` 里都没有真正的原值可以恢复。共用同一个 `--data-dir` 的情况由实例锁挡住：每次 `rurge run`（不限于带 `--system-proxy` 的）都会在打开 `state.json` **之前**对 `<data-dir>/rurge.lock` 取独占锁，同一数据目录上的第二个实例在碰任何东西之前就以 `error: another rurge instance is already running with the data directory <路径>` 退出 1——既不改系统设置，也不动 `state.json`。锁由操作系统在进程死亡时释放，所以崩溃之后的下一次启动照常拿到锁并恢复备份；反过来说，握着锁时在 `state.json` 里看到的备份必定属于一个已经死掉的运行，而不是一个还活着的实例。锁文件从不删除——删除会与另一个进程的打开竞争。文件系统不支持加锁（或锁文件打不开）时，rurge 记一条 WARN 后不带这项检查继续启动：不能因为文件系统的限制把守护进程整个丢掉。
- **运行期不巡检**：rurge 只在开启、重载跟随、关闭这三个时刻写系统设置。运行期间别的程序（或用户自己）改了系统代理，rurge 既不会察觉也不会重新写回去；退出时 rurge 把**开启那一刻**的快照原样写回，覆盖掉期间的任何改动。
- **`RURGE_SYSTEM_PROXY_BACKEND` 是测试钩子**：取值只能是 `file:<path>`，把「系统代理」换成对一个 JSON 文件的读写（端到端测试用，见设计文档 §15 的 P5）。其它任何取值都直接报错退出，不会静默退回真实后端——否则测试里的一个拼写错误就会改动开发机的真实设置。这个变量不是给用户用的。
- **重载跟随**：`POST /v1/profiles/reload`（或 SIGHUP / `--watch`）之后，如果系统代理开着，rurge 会用新配置重新计算地址并 `apply`（监听地址或 `skip-proxy` 变了就能看出来）。如果重载后没有任何可用的监听器，rurge **不会**顺带关闭系统代理——那等于替用户把流量静默改成直连——而是打一条 WARN，系统设置继续指向（已经失效的）旧地址，直到下一次重载成功或者进程退出。
- **启动输出顺序**：`listening on …`（每个监听器一行）→ `api on http://<addr>`（配置了 `http-api` 时）→ `system proxy enabled: <地址描述>`（带 `--system-proxy` 启动且成功开启时；地址描述形如 `http 127.0.0.1:6152` 或 `http 127.0.0.1:6152, socks 127.0.0.1:6153`）→ 汇总行 `rurge <version> running: …`。开启失败会改为 stderr 打印 `error: cannot enable the system proxy: <原因>` 并以退出码 1 结束，不打印汇总行。

## CLI 客户端

`rurge reload | stop | status [-c <conf>] [--remote host:port] [--key <key>] [--data-dir <dir>] [--platform <p>]`，`status` 另有 `--json`。地址 / 密钥：`--remote` + `--key`（或 `RURGE_API_KEY`）→ `-c` 配置的 `http-api`（只解析）→ 都未提供时退出 2，并打印 `http-api is not configured; add "http-api = <key>@127.0.0.1:6171" to [General] or pass --remote/--key (reload can also be triggered by SIGHUP or --watch)`。退出码：2 参数缺失或鉴权失败（401 / 403），1 连不上、非 2xx，或 `reload` 语义失败（HTTP 200 但 `ok:false`），0 成功。成功时 `reload` 打印 `reloaded: <错误数> error(s), <告警数> warning(s)`，`stop` 打印 `stop requested`，`status` 打印四行（出站模式与全局策略、策略与规则的个数与活动请求数、流量）；连不上时打印 `error: cannot reach rurge at http://<地址>: <原因>`，`reload` 失败时打印 `reload failed: <错误数> error(s); the running configuration is unchanged`。

阶段 1 补遗（M5b）起三个命令多一个 `--data-dir <dir>`（环境变量 `RURGE_DATA_DIR`，缺省是平台的数据目录）：`-c` 的 Profile 读来是为了找到 `http-api`，而托管 Profile 生效时 `http-api` 写在数据目录里下载的副本中，所以三个命令照 `rurge run` 的读法读那份副本（`--remote` 与 `--key` 盖过 Profile 里的值）。它们读 Profile 时从不联网，也从不创建或写数据目录。副本还没下载、本地文件又只有一行 `#!MANAGED-CONFIG` 时找不到 `http-api`：照旧退出 2 并打印上面那句 `http-api is not configured; …`，数据目录不会因此出现；找到了而连不上（守护进程没有在运行）时退出 1，打印 `error: cannot reach rurge at http://127.0.0.1:<端口>: <原因>`。`rurge service install` 不加载 Profile，不受影响。
