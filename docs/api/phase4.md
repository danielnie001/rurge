# rurge HTTP API（阶段 4）

阶段 4 / M1a（嗅探与规则之前的 SNI，设计 `docs/superpowers/specs/2026-10-29-phase4-m1-session-entry-http-engine-design.md` 的第 4 ～ 7、11、12 节，计划 `docs/superpowers/plans/2026-10-29-phase4-m1a-sniffing-plan.md`）**新增一个端点** `GET /v1/rurge/http`，改了请求记录里 `sni` 与 `protocol` 的取值，并且没有新增日志行（见「日志」）。鉴权（`X-Key`）、错误体（`{"error":"<message>"}`）与无内容的成功响应（`{}`）沿用阶段 1 的约定，见 `docs/api/phase1.md`；本页只记录变了的与新增的部分。阶段 4 / M1b（明文 HTTP 引擎，设计第 8 ～ 12 节，计划 `docs/superpowers/plans/2026-10-29-phase4-m1b-http-engine-plan.md`）给同一个端点加了 `requests`，给请求记录加了 `method`、`url`、`statusCode`，改了 `remoteAddr`、`connectMs`、`firstByteMs` 与 `up` / `down` 在 HTTP 请求上的含义，并给每个请求加了一行会话日志（见「M1b　明文 HTTP 引擎」）。

**这一份写的是本地提交的行为：M1a 与 M1b 的提交都在本地，CI 没有见过。** 下面每一处行为都只在一台 Windows 11 x86-64 开发机上的一次运行里由用例断言过（一台机器上的一次运行，不是 CI），没有手工验收；阶段 4 的手工清单从 M2 起建，M1 的行为随它写进去。

## M1a　嗅探与规则之前的 SNI

可嗅探的会话（经 SOCKS5、HTTP 监听器的 `CONNECT` 与虚拟网卡来的 TCP 会话，端口 80、443 或命中 `force-http-engine-hosts`，其余条件见「可嗅探的条件与等多久」）在答复之后先读头一批字节，判出 TLS、HTTP 或别的，再带着 SNI 与协议求规则、拨号。判为 HTTP 的会话在 M1a 照原样转发、协议记 `http`，进 HTTP 引擎随 M1b；判为 TLS 的照原样转发，解密随 M2 的 MITM。

### `GET /v1/rurge/http`

`/v1/rurge/…` 是总设计给 rurge 专有接口定的前缀，Surge 没有这个端点（设计第 11 节，M1-D24）。它要钥匙，与别的端点在同一层：没有钥匙回 401 `{"error":"unauthorized"}`。

应答的形状（`crates/rurge-api/src/routes/rurge_http.rs`）：

```json
{"sniffed":{"tls":0,"http":0,"other":0,"timeout":0,"closed":0}}
```

键的次序就是 `tls`、`http`、`other`、`timeout`、`closed`；用例 `rurge_http_counts_start_at_zero`（`crates/rurge-api/tests/api.rs`）断言正文的字节恰好是这一串，另外断言不带钥匙时是 401。

每个数是一个嗅探结局的次数（设计第 7.5 节）：

- `tls`：读到完整的 ClientHello，判为 TLS；
- `http`：判为明文 HTTP；
- `other`：判为别的（读不懂的 ClientHello、不是方法记号的开头、`PRI` 一类）；
- `timeout`：等满时限（15 秒）还没有判出来；
- `closed`：等满时限之前客户端关闭了写端，或读出错（计划 P4：读出错也计在这里）。

嗅探被停止（虚拟网卡停、守护进程退出）打断的不计——它没有结局。自进程启动起累计，重载不清零。M1a 只有 `sniffed`；M1b 加 `requests`（走到选路的请求数，设计第 11 节）。

### 请求记录

可嗅探的会话的 `sni` 与 `protocol` 在求规则之前就定了（设计第 7.1、7.2 节）：判为 TLS 的，`sni` 是 ClientHello 里第一个 `host_name`（转成小写），`protocol` 是 `https`；判为 HTTP 的，`protocol` 是 `http`；判为别的、超时与关闭的不设协议。

记录里这两个键各取自哪里，照 `crates/rurge-engine/src/observe.rs` 的 `record_of` 的那两行：

- `sni` 是 `s.sni.clone().or_else(|| h.sni())`：先取会话信息里的（嗅探读出的完整 SNI），没有时取句柄侧槽里的（不可嗅探的会话由转发循环从首个客户端数据块至多 8 KiB 里认出 SNI 时写进去的）；
- `protocol` 是 `h.protocol().or(s.protocol)`：先取句柄侧槽里的（转发循环的首块嗅探写的），没有时取会话信息里的。

不可嗅探的会话照旧：SNI 与协议只在出站建立之后的首块嗅探里认出，决定策略时规则看不见它们。

可嗅探的会话在被嗅探完、拨号时才建句柄与记录：嗅探中途被停止的会话没有记录（被拒绝、拨号失败的会话有记录，同今天）。读出错的会话不拨号，也没有记录。

### 日志

嗅探不另写日志行：结局在计数里，协议与 SNI 在记录与会话日志里（设计第 12 节）。

- `CONNECT` 的会话在升级失败、还没有会话时写一行 DEBUG `CONNECT upgrade failed before the session began`，带 `listener = "http"` 与 `error`，不带地址（计划 P8）。
- SOCKS5 写成功应答失败时照今天由监听器已有的那一行 DEBUG（`socks5 session ended with an error`，带 `source`）记下，不另写（计划 P8）：可嗅探的会话这时还没有句柄。

虚拟网卡上的名字与地址照旧只在 DEBUG。

### 可嗅探的条件与等多久

可嗅探的条件是：TCP、经三种入口（`CONNECT`、SOCKS5、虚拟网卡）、引擎没有暂停、预匹配不拒（只对经代理端口来的会话问一次，且不计数）、不命中 `always-raw-tcp-hosts` 与 `always-raw-tcp-keywords`、端口是 80 或 443 或命中 `force-http-engine-hosts`（这个名单的缺省端口是 80）；经代理端口来、目标是 rurge 地址空间里的 fake 地址时，先换回它背后的名字再比。等多久：15 秒（`EngineShared.sniff_wait` 只供测试调短，不进命令行与环境变量）。全文与登记的差异见兼容性清单 7.1 节与设计第 4、6、13 节，这里不重抄。

### 没有覆盖的

- 没有手工验收；**M1a 的提交都在本地，CI 没有见过**，上面每一条只在一台机器上的一次运行里证明过。
- `timeout` 那一个数只在把 `EngineShared.sniff_wait` 调短的用例里出现过；没有用例等满缺省的 15 秒，用缺省的 15 秒、不调短的只有一条（`a_stop_during_the_wait_ends_a_tun_session_at_once`），它让停止落在等待之中，证明的是停止、不是时限。
- 只有 HTTP 监听器与 SOCKS5 的应答次序改了；`CONNECT` 的 502 错误页、SOCKS5 的失败应答码在可嗅探的会话上不再出现，那两种失败现在是一个随即关闭的隧道（设计第 13 节第 1 条，登记在兼容性清单 7.1 节），没有在真机上观察过。

## M1b　明文 HTTP 引擎

嗅探判为 HTTP 的连接与 `force-http-engine-hosts` 强制进来的非 TLS 连接进 `rurge-http` 的 HTTP/1.1 引擎；HTTP 监听器的绝对形式请求也交给同一个引擎，今天转发它的 `rurge-inbound` 的那段代码（`forward()`）退役。每个请求各自求规则、选策略、借或拨上游连接、各有一条记录；上游连接在同一个策略、同一代配置、同一出口表时复用，池属于一条客户端连接，**不跨客户端连接共享**（总设计 P4-D6）。时限、拒绝与失败的答复、升级（WebSocket）与嗅探到的连接上的 `CONNECT`、两种重发的写法见设计第 8 节，全文与登记的差异见兼容性清单 7.1 节；这里只写 API、记录与会话日志看得见的部分。

### `GET /v1/rurge/http` 的 `requests`

应答多一个键，在 `sniffed` 之后（键序）：

```json
{"sniffed":{"tls":0,"http":0,"other":0,"timeout":0,"closed":0},"requests":0}
```

`requests` 是走到选路的请求数（设计第 11 节，计划 P19）：嗅探到的连接、监听器的绝对形式与虚拟网卡三种入口都走 `ConnUpstreams::route`，在那里加一，**被拒的请求也算**（`REJECT` 一族是拨号时才生效的）；读不出请求头的连接（发半个头就关、请求头超时、假地址反查不到）不算——它在承载连接的记录里（见下）。自进程启动起累计，重载不清零。`sniffed` 的五个数的意思不变（见「M1a」）。用例：`the_requests_are_counted`（`crates/rurge-engine/tests/http.rs`：嗅探到的连接上两个请求、监听器上一个被 `REJECT` 的请求，`requests` 是 3）、`the_sniffs_are_counted`（那段里的一个请求）与 `rurge_http_counts_start_at_zero`（零与键序，`crates/rurge-api/tests/api.rs`）。

### 请求记录

引擎里的明文 HTTP **一个请求一条记录**（不再是一条连接一条，设计第 10.1 节）。记录（`RequestRecord`）与它的 JSON（`RequestJson`）在 `protocol` 之后多三项，次序就是 `method`、`url`、`statusCode`：

- `method`：请求的方法名（`GET`、`POST` 等）；
- `url`：去掉 userinfo、**留着查询串**（总设计 Q1 的临时做法），源站形式的请求由 `Host` 头重建（`http://主机[:端口]/路径`），绝对形式的就是请求行里的那个，主机是 fake 地址时先换成它的名字；嗅探到的连接上的 `CONNECT` 没有 URL，是 `null`；
- `statusCode`：答给客户端的状态码；引擎自己答的拒绝页（`REJECT` 一族开着 `show-error-page-for-reject` 时是 403）与错误页（502、504）是它答的码；没答就把连接关了的（页关着的拒绝与失败）是 `null`。

不是 HTTP 请求的会话，三项都是 `null`。用例 `a_request_record_has_its_method_url_and_status`（200、403 与一个被关连接而 `statusCode` 为 `null` 的请求）、`a_url_with_userinfo_is_recorded_without_it`，以及 `crates/rurge-api/tests/api.rs` 的 `requests_carry_the_method_the_url_and_the_status_code`（键序与 `null`）。

另外四处与以前不同：

- `remoteAddr`：借来的连接是那条连接拨号时记下的地址（设计第 8.4 节），所以经引擎的 HTTP 请求**借来的连接也有**——这是 `docs/api/phase1.md`「`remoteAddr`」一条里“复用已有连接的会话没有”在 HTTP 上的那一半；用例 `a_borrowed_connection_records_where_it_went`。
- `connectMs`：借来的连接是从请求开始到借到为止；真拨号照旧。`firstByteMs`：从就绪（自己拨好或借到）到读到第一个应答字节。
- `up` / `down`：这个请求在所用上游连接上写出与读到的字节（请求头与正文、应答头与正文，升级之后的转发也算），与按原样转发的会话一样**只数上游一侧**；客户端一侧的头不计入任何一条（设计第 13 节第 5 条）。用例 `each_request_counts_its_own_bytes`（两个应答 100 与 5000 字节、借同一条连接）。
- `notes`、`rule`、`policy`、`dst`、`pid`、`processPath`、`listenPort`、`dnsMs` 照旧；`notes` 的四种写法没有新的。

### 承载连接的记录

进引擎的客户端连接自己那个句柄建好时不登记（不在活动列表里，结束时不自动写日志与记录）：连接结束时**一个请求也没有走到选路**才补一条记录——发半个头就关、读不懂、请求头超时、fake 地址反查不到这几种，结局是失败、原因照实写（如 `client closed the connection`、`fake address not known`），会话日志的级别照旧（失败在 INFO，不带路径）。走到过选路的连接不另记：它的请求各有记录。所以 `requests` 为零的连接不一定没有记录。用例 `a_connection_that_never_reaches_the_rules_leaves_one_failed_record`、`a_connection_whose_requests_were_routed_leaves_no_record_of_its_own`、`the_carrier_connection_is_recorded_only_when_nothing_was_routed`。

### 日志

每个请求一行会话日志（还是 `log_session` 那三行），级别照旧：完成的在 DEBUG，被拒与失败的在 INFO（设计第 12 节）。字段照旧，在 `elapsed_ms` 之后加 `method` 与 `status`（不是 HTTP 的会话两个值都是空的），完成的那一行再加 `url`：写成 `scheme://主机[:端口]/路径`，**不带查询串与片段**（userinfo 本来就没有）；头与正文在任何级别都不进日志（总设计 P4-D25）。用例 `the_request_log_line_shows_no_query_and_no_userinfo`（`crates/rurge-engine/tests/http.rs`），字段的次序由 `crates/rurge-engine/tests/pipeline.rs` 的 `SESSION_FIELDS` 钉住。

### 没有覆盖的

- 没有手工验收；**M1b 的提交都在本地，M1a 与 M1b 的提交 CI 都没有见过**，上面每一条只在一台机器上的一次运行里证明过，也没有在真机上跑过（M1 没有平台代码）。
- 四种重写与 Map Local（M3）、抓包视图与记录里的各段耗时（M4）、HTTPS 的解密与 HTTP/2（M2）都还没有做：本页写的只是明文 HTTP 的那一层。
- 不改请求头：不加 `Via`、`X-Forwarded-For`（设计第 8.7 节）。
- 记录里的 `url` 带着查询串出 API 是总设计 Q1 的临时做法（设计第 19 节把最终取法留给项目所有者）。
