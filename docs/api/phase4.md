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

`requests` 是走到选路的请求数（设计第 11 节，计划 P19）：嗅探到的连接、监听器的绝对形式与虚拟网卡三种入口都走 `ConnUpstreams::route`，在那里加一，**被拒的请求也算**（`REJECT` 一族是拨号时才生效的）。自进程启动起累计，重载不清零。`sniffed` 的五个数的意思不变（见「M1a」）。

**不算的：一个请求也没走到选路的连接**，包括两类。一是读不出请求头的：发半个头就关、读不懂、请求头超时。二是**目的地**是反查不到名字的 fake 地址的连接：`serve_http` 在读任何请求之前就以 `fake address not known` 结束承载连接（`crates/rurge-engine/src/serve.rs` 的 `Turned::Unknown`）。这两种都在承载连接的记录里（见下）。

**算的：HTTP 监听器上绝对形式的请求，请求行里的主机是反查不到名字的 fake 地址**——`rurge-http` 先 `route`（计数就在那里），再看反查的结果（`crates/rurge-http/src/engine.rs` 的 `prepared.unknown`）；那个请求有自己的记录，结果是 `fake address not known`（`show-error-page` 开着时答 502，关着时不答就关），承载连接不另记。两条路次序不同，出处是设计第 21 节第 10 条与延后第 3 行。

用例：`the_requests_are_counted`（`crates/rurge-engine/tests/http.rs`：嗅探到的连接上两个请求、监听器上一个被 `REJECT` 的请求，`requests` 是 3）、`the_sniffs_are_counted`（那段里的一个请求）与 `rurge_http_counts_start_at_zero`（零与键序，`crates/rurge-api/tests/api.rs`）。

### 请求记录

引擎里的明文 HTTP **一个请求一条记录**（不再是一条连接一条，设计第 10.1 节）。记录（`RequestRecord`）与它的 JSON（`RequestJson`）在 `protocol` 之后多三项，次序就是 `method`、`url`、`statusCode`：

- `method`：请求的方法名（`GET`、`POST` 等）；
- `url`：去掉 userinfo、**留着查询串**（总设计 Q1 的临时做法），源站形式的请求由 `Host` 头重建（`http://主机[:端口]/路径`），绝对形式的就是请求行里的那个，主机是 fake 地址时先换成它的名字；源站形式的请求没有 `Host` 头（或读不懂）时，用连接的目标拼（`crates/rurge-http/src/engine.rs` 的 `form_of` 最后一支）；嗅探到的连接上的 `CONNECT` 没有 URL，是 `null`；
- `statusCode`：答给客户端的状态码，按拒绝的种类分开（`crates/rurge-http/src/engine.rs` 的 `answer`）：`REJECT` 与 `REJECT-NO-DROP` 开着 `show-error-page-for-reject` 时是 403 页，关着时不答就关、是 `null`；`REJECT-TINYGIF` 不看开关，答那一像素的 GIF、记 200；`REJECT-DROP` 停一会儿再关、是 `null`；失败与等应答头超时的错误页（502、504）要 `show-error-page` 开着，关着时不答就关、是 `null`。

不是 HTTP 请求的会话，三项都是 `null`。用例 `a_request_record_has_its_method_url_and_status`（200、403 与一个被关连接而 `statusCode` 为 `null` 的请求）、`a_url_with_userinfo_is_recorded_without_it`，以及 `crates/rurge-api/tests/api.rs` 的 `requests_carry_the_method_the_url_and_the_status_code`（键序与 `null`）。

另外四处与以前不同：

- `remoteAddr`：借来的连接是那条连接拨号时记下的地址（设计第 8.4 节），所以经引擎的 HTTP 请求**借来的连接也有**——这是 `docs/api/phase1.md`「`remoteAddr`」一条里“复用已有连接的会话没有”在 HTTP 上的那一半；用例 `a_borrowed_connection_records_where_it_went`。
- `connectMs`：借来的连接是从请求开始到借到为止；真拨号照旧。`firstByteMs`：从就绪（自己拨好或借到）到读到第一个应答字节。
- `up` / `down`：这个请求在所用上游连接上写出与读到的字节（请求头与正文、应答头与正文，升级之后的转发也算），与按原样转发的会话一样**只数上游一侧**；客户端一侧的头不计入任何一条（设计第 13 节第 5 条）。用例 `each_request_counts_its_own_bytes`（两个应答 100 与 5000 字节、借同一条连接）。

别的字段没有变：`notes`、`rule`、`policy`、`dst`、`pid`、`processPath`、`listenPort`、`dnsMs` 照旧，`notes` 的四种写法也没有新的。

### 承载连接的记录

进引擎的客户端连接自己那个句柄建好时不登记（不在活动列表里，结束时不自动写日志与记录）：连接结束时**一个请求也没有走到选路**才补一条记录——发半个头就关、读不懂、请求头超时，以及连接的目的地是反查不到名字的 fake 地址（这一种在读请求之前就结束）——结局是失败、原因照实写（如 `client closed the connection`、`fake address not known`），会话日志的级别照旧（失败在 INFO，不带路径）。走到过选路的连接不另记：它的请求各有记录。所以 `requests` 为零的连接不一定没有记录。HTTP 监听器上绝对形式的请求里反查不到名字的 fake 地址**不在此列**：它有自己的记录（见上面 `requests` 一节）。用例 `a_connection_that_never_reaches_the_rules_leaves_one_failed_record`、`a_connection_whose_requests_were_routed_leaves_no_record_of_its_own`、`the_carrier_connection_is_recorded_only_when_nothing_was_routed`。

### 日志

每个请求一行会话日志（还是 `log_session` 那三行），级别照旧：完成的在 DEBUG，被拒与失败的在 INFO（设计第 12 节）。字段照旧，在 `elapsed_ms` 之后加 `method` 与 `status`（不是 HTTP 的会话两个值都是空的），完成的那一行再加 `url`：写成 `scheme://主机[:端口]/路径`，**不带查询串与片段**（userinfo 本来就没有）；头与正文在任何级别都不进日志（总设计 P4-D25）。用例 `the_request_log_line_shows_no_query_and_no_userinfo`（`crates/rurge-engine/tests/http.rs`），字段的次序由 `crates/rurge-engine/tests/pipeline.rs` 的 `SESSION_FIELDS` 钉住。

### 没有覆盖的

- 没有手工验收；**M1b 的提交都在本地，M1a 与 M1b 的提交 CI 都没有见过**，上面每一条只在一台机器上的一次运行里证明过，也没有在真机上跑过（M1 没有平台代码）。
- 四种重写与 Map Local（M3）、抓包视图与记录里的各段耗时（M4）、HTTPS 的解密与 HTTP/2（M2）都还没有做：本页写的只是明文 HTTP 的那一层。
- 不改请求头：不加 `Via`、`X-Forwarded-For`（设计第 8.7 节）。
- 记录里的 `url` 带着查询串出 API 是总设计 Q1 的临时做法（设计第 19 节把最终取法留给项目所有者）。

## M2a　MITM（HTTP/1.1）

`[MITM]` 生效了：判为 TLS 的可嗅探会话在开关开着、有可用的 CA、来源允许、名单命中、ALPN 合得上时，rurge 用 `[MITM]` 的 CA 当场签一张叶证书、与客户端握手，把解密之后的 HTTP/1.1 请求交给 M1b 的引擎（设计第 7.1 节，计划 P18）。这里的每个请求照旧各自求规则、选策略、借或拨上游连接、各有一条记录，只是上游一侧多一层 TLS、URL 以 `https://` 开头、协议是 `HTTPS`（见「M1b」）。HTTP/2 随 M2b，CA 的命令行工具与 `http-api-tls` 随 M2c（见「没有覆盖的」）。

**这一节写的是本地提交的行为：M2a 的提交也在本地，CI 没有见过。**

### `/v1/features/mitm`

`GET` 给 `{"enabled":bool}`——开关的值，不论 CA 能不能用（CA 的状态看 `GET /v1/mitm/ca` 与加载时的 `W0039`）。`POST {"enabled":bool}` 给 `{}`，并把开关写进 `state.json`（`version` 是 2，`features.mitm` 是布尔）；切换立即对新的连接生效，已经解密的连接不因为关掉它而中断（设计第 11.1 节）。

缺省是**开**（没设过时；总设计 P4-D19）。启动时 `state.json` 里记着的值由 `Engine::restore_switches` 装回开关，不写盘：一个第 1 版的文件在一次启动之后仍是它原来的字节（`crates/rurge-engine/src/engine.rs`）。

`POST` 之后有一行 INFO（`crates/rurge-api/src/routes/features.rs`），原文与字段照代码：

```
enabled=false mitm switched via http-api
```

即消息 `mitm switched via http-api`，字段 `enabled`（`true` / `false`）。

用例 `the_mitm_switch_is_read_switched_and_kept`（缺省开、切换、`state.json` 的 `version` 2 与 `features.mitm`）、`a_daemon_restores_the_mitm_switch_from_state_json`（`crates/rurge/tests/cli.rs`：起守护进程之后读得到文件里记着的值）。

### `GET /v1/mitm/ca`

这一代的 CA 证书（与私钥配对的那一张）的 DER，不含私钥。两个头照代码逐字（`crates/rurge-api/src/routes/mitm.rs`）：

- `Content-Type: application/x-x509-ca-cert`
- `Content-Disposition: attachment; filename="rurge-ca.cer"`

**“可用的 CA”是这一代有 `Mitm`**：`[MITM]` 写了 CA、六条检查都过、`Mitm::build` 成功（`Engine::mitm_ca_der` 读的是这一代的 `Runtime.mitm`，`crates/rurge-engine/src/engine.rs`）。没有时是 404，正文同别的 404 的形状：

```json
{"error":"no usable MITM CA"}
```

钥匙可以放在查询参数里（`?x-key=`，阶段 1 起就认），浏览器能直接下载。用例 `the_mitm_ca_is_given_in_der`（两个头、正文逐字节是 CA 的 DER、`?x-key=`）、`the_mitm_ca_is_a_404_without_a_usable_ca`。

### `GET /v1/rurge/http` 的 `mitm`

应答在 `requests` 之后多一个对象，键的次序固定（设计第 12.3 节，计划 P23）：

```json
{"sniffed":{"tls":0,"http":0,"other":0,"timeout":0,"closed":0},"requests":0,"mitm":{"decrypted":0,"untrusted":0,"failed":0,"noRequest":0}}
```

正文的字节恰好是这一串，用例 `rurge_http_counts_start_at_zero`（`crates/rurge-api/tests/api.rs`）写着它。四个数：

- `decrypted`：与客户端的握手完成时（计划 P23）；
- `untrusted`：客户端的警报是 `unknown_ca`、`bad_certificate` 或 `certificate_unknown` 之一时（计划 P11 的分类），握手失败时加一；
- `failed`：别的握手失败（含客户端中途放手、时限到了），按失败的原因记，加一在握手失败时；
- `noRequest`：握手完成、`serve_connection` 返回、一个请求也没有走到选路、读到的明文字节是 0、客户端关的、没有被停止（计划 P21 与设计第 8.3 节）；
- 被停止打断的握手不计——它没有结局（设计第 8.2 节）。

自进程启动起累计，**重载不清零**（计数器在 `Engine` 里，不在 `Runtime` 里）。用例 `the_mitm_counts_survive_a_reload`、`a_stop_during_the_handshake_counts_nothing`（`crates/rurge-engine/tests/mitm.rs`）。

### 请求记录

解密的连接上每个请求一条记录，同 M1b 的形状与规矩（见「M1b」的「请求记录」一节）。与明文的三处不同：

- `protocol` 是 `https`；
- `url` 以 `https://` 开头。**每个请求都是**，包括客户端在解密的连接里发来的绝对形式请求：请求行里的 scheme 不看（计划 P13）；`Host` 写了端口的照写，只有兜底的连接目标在 443 时省略端口（同 M1b 的 `form_of`）；
- `sni` 是 ClientHello 里那个名字，同 M1a（见「M1a」的「请求记录」）。

用例 `a_listed_host_is_decrypted_through_socks5`（`protocol` 是 `https`、`url` 是 `https://origin.test/a`）、`every_entry_decrypts_a_listed_host`（三种入口，每个的 `url` 都是 `https://`）与 `two_requests_of_one_decrypted_connection_follow_their_own_rules`（两个请求各有记录、各照自己的规则）。

### 承载连接的记录

解密的连接自己那个句柄同 M1b：建好时不登记，走到过选路的请求各有记录、连接不另记。补一条记录的三处（计划 P21，原因照原文）：

- 客户端不信任 rurge 签的证书，握手失败：`the client does not trust the certificate rurge issued (alert <警报的名字>)`；
- 别的握手失败：`TLS handshake with the client failed: <原因>`；
- 证书固定的提示（收上四条再加上读到的明文字节是 0 那个连接）：`no request after the TLS handshake (likely certificate pinning)`。

三处的结局都是失败（会话日志照旧在 INFO），另有一个请求也没走到选路但不符合提示的上四条时，原因是 hyper 给的，同 M1b。用例 `a_client_that_closes_after_the_handshake_gets_the_pinning_hint`、`a_handshake_the_client_abandons_is_failed_at_info`、`through_the_virtual_interface_the_warnings_name_no_host`。

### 日志

握手结束的时候有三类行（`crates/rurge-engine/src/serve.rs` 的 `log_untrusted`、`log_pinning`、`log_handshake_failed`）。经监听器来的会话带名字与 `port` 字段，经虚拟网卡来的不带名字与端口、名字另起一行 DEBUG（设计第 8.2、15 节）。句子照代码逐字：

**不信任**（WARN，另带 `alert` 字段）：

- 经监听器：`the client does not trust the certificate rurge issued for <名字>: trust rurge's MITM CA on the client, or exclude the host with -<名字> in [MITM] hostname`；
- 经虚拟网卡：`the client does not trust the certificate rurge issued for a host reached through the virtual interface (the name is in the DEBUG log and the request record): trust rurge's MITM CA on the client, or exclude the host in [MITM] hostname`，另 DEBUG 一行（经监听器的句子）带名字。

**证书固定的提示**（WARN）：

- 经监听器：`<名字> is likely protected by certificate pinning: the client finished the TLS handshake and closed without a request; exclude it with -<名字> in [MITM] hostname`；
- 经虚拟网卡：`a host reached through the virtual interface is likely protected by certificate pinning (the name is in the DEBUG log and the request record): the client finished the TLS handshake and closed without a request`，另 DEBUG 一行（经监听器的句子）带名字。

**别的握手失败**（INFO）：

- 经监听器：`the TLS handshake with the client failed for <名字>: <原因>`；
- 经虚拟网卡：`the TLS handshake with the client failed for a host reached through the virtual interface: <原因>`，另 DEBUG 一行（经监听器的句子）带名字。

**限频**（设计第 8.4 节，计划 P22）：两种 WARN（不信任、证书固定）**每种、每个名字一小时至多一行**；被挡下的那一次写 DEBUG，句子同它顶替的 WARN（经监听器的那一种，带名字），经监听器的还带 `port`、不信任的还带 `alert`；经虚拟网卡的 DEBUG 行这两个字段都不带。表里至多 1024 个键，满了整个清空。INFO 那一行不限频；计数与记录不论限频与否都有。

用例 `an_untrusting_client_is_warned_once_an_hour_and_counted_each_time`（同名字第二回只有 DEBUG、`alert unknown_ca`）、`through_the_virtual_interface_the_warnings_name_no_host`（三条 WARN / INFO 不带名字与端口、三条 DEBUG 各带一个名字且不带 `port`）、`a_request_head_that_times_out_is_no_pinning_hint`、`a_stop_after_the_handshake_is_no_pinning_hint`。限频的两个边界只有单元用例：`a_hint_is_warned_again_only_after_an_hour`、`a_full_table_is_cleared_before_a_new_key`（`crates/rurge-engine/src/mitm.rs`）。

### `state.json` 第 2 版

- `version` 是 2（`STATE_VERSION`）。
- `[MITM]` 与后面几个阶段的四个开关——`mitm`、`capture`、`rewrite`、`scripting`——是可省的 `bool`：没设过的不写（`skip_serializing_if`）。`system_proxy` 与 `enhanced_mode` 照旧是布尔。
- 第 1 版文件里的 `"mitm": false` 读作**没设过**（于是 `mitm` 开着）：阶段 1 ～ 3 的代码从来不能把它们设成真，那个 `false` 不是使用者的选择（设计第 11.2 节）。`version` 只在内存里改成 2，下一次写盘时写成第 2 版。
- 阶段 3 的代码读得懂第 2 版的文件（没设过的不写、在的都是布尔），不会把整个文件当成坏的。**降级再升级之后 `mitm` 是关的**：旧版本写回文件时把没设过的写成 `false`，新版本读到的是“设过、关”（设计第 16 节第 11 条）；使用者经 API 再打开即可。

用例 `loads_defaults_selections_and_tolerates_garbage`、`a_version_1_file_leaves_the_four_switches_unset`、`a_version_2_file_is_what_phase_3_reads`（`crates/rurge-engine/src/state.rs`）。

### 没有覆盖的

- HTTP/2 的解密（M2b）：只给 `h2` 的客户端照原样转发，`h2 = true` 不生效。
- `rurge mitm ca generate` / `export`、`http-api-tls` 与阶段 4 的手工验收清单（M2c）。
- 没有把 CA 装进任何信任库、没有解密过真实的站点、没有经真浏览器试过：全部证据是回环上的用例。
- `auto-quic-block` 读进来、不生效（M5）。
- 证书固定判断里两项“没有被停止”的条件没有能红的用例（M2a 记录第 50 行、M2a 延后第 3 行）。
