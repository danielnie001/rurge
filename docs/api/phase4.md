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

## M2b　MITM（HTTP/2）

`[MITM] h2 = true` 生效了：解密的连接上，客户端用 ALPN 选了 `h2` 的，rurge 用 hyper 的 HTTP/2 服务端讲它的请求；对上游的 TLS 同时给 `h2` 与 `http/1.1`，选了 `h2` 的上游连接在同一条客户端连接的同一个键下共享给并发的请求。两侧的版本可以不同，请求在两种形式之间转换（设计第 10 节，计划 P1 ～ P18）。**HTTP/1.1 上“关掉整条连接”的终止、看门狗、拒绝与停止，在 HTTP/2 上改成“重置这个流”。** 没有新的端点，也没有新的计数：`/v1/rurge/http`、`/v1/features/mitm`、`GET /v1/mitm/ca` 与 `state.json` 的形状都照「M2a」。

**这一节写的是本地提交的行为：M2b 的提交也在本地，M1a、M1b、M2a 与 M2b 的提交 CI 都没有见过。**「M2a」一节「没有覆盖的」里有一条写着“HTTP/2 的解密（M2b）：只给 `h2` 的客户端照原样转发，`h2 = true` 不生效”——那是 M2a 收尾时的状态，M2b 之后以本节为准。

### 什么时候讲 HTTP/2

- **客户端一侧**：「M2a」的五条都对得上，并且 `[MITM] h2 = true`（缺省 false，见兼容性清单 7.2 节那一行）。这时服务端的 ALPN 是 `[h2, http/1.1]`，两种都给的客户端拿到 `h2`（rustls 按服务端列表的次序选）；`h2 = false` 时仍是 `[http/1.1]`，只给 `h2` 的客户端照原样转发（`a_client_offering_only_h2_is_passed_through`）。
- **上游一侧各自协商**：解密的连接（不论客户端一侧是哪个版本）对上游的 ALPN 是 `[h2, http/1.1]`，源站选哪个就讲哪个；`h2 = false` 时对上游也只给 `http/1.1`。所以四种组合都可能：客户端 h2 而上游 h1、客户端 h1 而上游 h2 都在用例里（`four_combinations_of_http1_and_h2_are_decrypted`，`crates/rurge-engine/tests/mitm.rs`）。
- **明文的连接只讲 HTTP/1.1**：h2c（不论升级还是先验知识）没有做；`h2 = true` 不影响经 HTTP / SOCKS5 监听器与虚拟网卡来的明文会话（设计第 1 节）。
- 客户端一侧的上限：一条连接至多 100 个并发流，不发 keep-alive 的 PING，不宣告扩展 `CONNECT`（总设计 P4-D11）。用例 `the_h2_server_offers_100_streams_and_no_extended_connect`。

### 请求记录

解密的连接上每个请求一条记录，形状与规矩同「M2a」（`protocol` 是 `https`、`url` 以 `https://` 开头、`sni` 是 ClientHello 里那个名字）。HTTP/2 加的三件事：

- **URL 与规则看 `:authority`**：会话的 `http_host` 与记录的 `url` 取 `:authority`（FR-HTTP-02，设计第 10.3 节），规则看到的主机名也是它；转到 HTTP/1.1 的上游时按 RFC 9113 第 8.3.1 节把 `:authority` 写进 `Host`、请求目标改成源站形式。用例 `the_authority_of_an_h2_request_names_it_and_becomes_host`、`an_h2_request_keeps_its_authority_and_te_trailers_to_an_h2_origin`。
- **客户端取消一个流**：原因写 `client cancelled the request`（HTTP/1.1 上是 `client closed the connection`）。客户端重置这个流、或者整条连接关了，都是它；同一条连接上别的流照常。用例 `a_stream_the_client_resets_is_cancelled_and_the_connection_stays`。
- **`cookie` 与连接专用的头**：HTTP/2 拆开的多个 `cookie` 字段转到 HTTP/1.1 时合成一个（用 `; ` 连接，RFC 9113 第 8.2.3 节）；转到 HTTP/2 的那一侧去掉 `Host`、`Transfer-Encoding` 与其余连接专用的头，`TE` 只留 `trailers`，原来声明过才留。用例 `the_cookies_of_an_h2_request_are_joined_for_an_http1_upstream` 与 `crates/rurge-http/src/version.rs` 的四条单元用例，经引擎的那一条是 `the_cookies_of_an_h2_client_reach_an_http1_origin_as_one_field`。

### `CONNECT` 的 501

HTTP/2 上的 `CONNECT`（RFC 9113 第 8.5 节的隧道）不转发：答 `501`，正文是 `CONNECT over HTTP/2 is not supported`（不是错误页），**这个请求有自己的记录**（结局失败、原因就是那一句），不选路、不拨号（计划 P6）。承载连接的记录照 M2a 的规矩（这个请求走到了选路，所以连接不另记）。用例 `a_connect_over_h2_is_a_501_with_a_record`（`crates/rurge-http/src/engine.rs`）与 `a_connect_over_h2_has_one_record_and_a_501`（`crates/rurge-engine/tests/mitm.rs`）。**带非零 `content-length` 的 `CONNECT` 没有记录**：hyper 在服务它之前就把那个流重置了。

### 上游的连接与字节

- **共享**：上游选了 `h2` 时，同一个键在**一条客户端连接**里至多一条 HTTP/2 连接（键就是 M1b 的池键 `PoolKey`：这一代的配置与策略注册表、选到的策略与链、目标、能不能以绝对形式交给 HTTP 代理、拨号时的出口表；TLS 的参数不进键，M2-D12），`SendRequest` 克隆着用、共享给并发的请求；`is_closed()` 或收到源站的 GOAWAY 之后不再借出。不跨客户端连接共享，也不进 M1b 的池。用例 `requests_of_one_key_share_one_h2_connection`、`another_key_gets_another_h2_connection`、`an_h2_connection_that_went_away_is_not_lent_again`，经引擎的 `concurrent_h2_streams_share_one_upstream_connection`。
- **客户端 HTTP/2 而源站只讲 HTTP/1.1 时，并发的请求各占一条上游连接，数目不另设上限**（登记，设计第 10.2 节）。
- **升级与隧道只拨 HTTP/1.1**：带 `Upgrade` 的请求与 `CONNECT` 用只给 `http/1.1` 的那份客户端配置拨号，源站讲 h2 时也这样（设计第 10.4 节）。用例 `an_upgrade_dials_http1_where_the_origin_speaks_h2`、`a_websocket_from_an_http1_client_goes_over_http1_where_the_origin_speaks_h2`。
- **字节**：HTTP/1.1 的上游照 M1b（换连接的计量，记线上的字节）。共享的 HTTP/2 上游换不了连接一层的计量，所以设成不记之后连接一层什么也不记，**每条记录的 `up` / `down` 只算它自己的正文**（请求正文与应答正文，trailers 不计）。例外是**拨这条连接的那一个请求**：计量是在握手之后才关掉的，在那之前走过的字节都算在它头上——TLS 握手与 HTTP/2 的前言，可能还有 SETTINGS 的交换与 TLS 1.3 的会话票据，**不是准确的数**（设计第 16 节第 10 条、第 23 节第 21 条）。用例 `the_bytes_of_requests_on_a_shared_h2_connection_are_their_bodies`（拨号那个请求只断言 `down` 至少 5）。
- **被源站拒绝的流再发一次**：源站以 `REFUSED_STREAM` 拒绝、或以 GOAWAY 说没有处理过的流（RFC 9113 第 8.7 节），**借来的连接上、幂等且没有正文**的请求在新拨的连接上再发一次，另写一行 DEBUG：`an upstream refused the request; sending it again on a new connection`。别的照 M1b 答 502。用例 `a_request_the_origin_refuses_is_sent_again_once`。

### 空闲、停止与 GOAWAY 之后的一秒

- **空闲**：客户端一侧的 HTTP/2 连接在**没有请求在途**、且过了 `Conn::idle`（运行时的空闲时限，同「M1b」的 `idle_timeout`）时发 GOAWAY（`graceful_shutdown`）；之后从没有请求在途时算起，至多再等一秒（`GOAWAY_GRACE`，照 Go 的 `net/http` HTTP/2 服务端发 GOAWAY 之后的 `goAwayTimeout`），没关的连接就丢掉，另写一行 DEBUG：`an HTTP/2 client did not close its connection within a second of GOAWAY; dropping it`。HTTP/1.1 的 keep-alive 照 M1b（`head_wait`），所以**两种版本“空闲”的口径不同**：HTTP/1.1 等的是下一次请求头，HTTP/2 等的是“没有请求在途”。用例 `an_idle_h2_connection_is_sent_goaway`、`a_silent_h2_client_is_dropped_after_idle_and_the_grace`（什么都不发、不回 PING 的客户端）。
- **代价**：很慢的机器上，一个规矩的客户端也可能在那一秒里没走完平静关而被切——那时它的流已经关了，被切的只是连接（任务 1 报告的顾虑 3）。
- **停止**：`graceful_shutdown`（GOAWAY），在途的流等它们自己的令牌；一个也不在途、或不读的那个流让停止迟迟不结束的连接，至多一秒之后被丢掉（`a_stop_ends_an_h2_connection_with_goaway`、`a_stop_while_an_h2_client_does_not_read_ends_the_connection`）。
- 在途的计数是守卫加一、丢掉减一；“客户端不读”与“终止一个客户端不读的流”之后，在途数要等 hyper 下一次 poll 那个流、或者连接关掉才减——这不是缺陷，连接因此在那一小段里不会被判为空闲（设计第 10.5 节的登记）。

### 终止、看门狗与拒绝：只重置这个流

HTTP/2 上一条连接承载许多请求，所以 M1b 里“关掉这条连接”的做法在这里都改成“重置这个流”，同一条连接上别的流与之后的请求照常（设计第 10.5 节，M2-D17）。**重置的原因码一律是 `INTERNAL_ERROR`**：终止（`POST /v1/requests/kill` 与请求的令牌，记录的结局是 `killed`）、看门狗到时（`client stopped reading`）、没有错误页的 `REJECT`、`REJECT-DROP`，以及上游正文出错——上游的原因码不转给客户端（hyper 按错误链里的原因码重置客户端的流，而 `REFUSED_STREAM` 那样的原因码会让客户端以为请求没有处理过、可以重发；见设计第 23 节第 20 条）。

用例（`crates/rurge-http/src/engine.rs` 的测试模块）：`a_killed_h2_stream_is_reset_and_the_others_go_on`、`a_killed_h2_stream_whose_body_flows_is_reset_and_its_upstream_let_go`、`an_h2_client_that_stops_reading_one_stream_fails_only_that_request`、`a_rejected_h2_request_without_a_page_resets_only_its_stream`、`a_dropped_h2_request_is_held_then_reset`、`an_upstream_h2_stream_reset_mid_body_reaches_the_client_as_internal_error`。`REJECT` 有错误页时照答在那个流上。

### 已知限制

**上游在应答的 END_STREAM 之前以 `RST_STREAM(NO_ERROR)` 结束这个流时，rurge 记的是完成。** hyper 把这样结束的应答正文当成正常读完（它按 RFC 7540 第 8.1 节“提前应答”的读法处理），引擎因此 `complete`、记 `Completed`；应答**不带 `content-length`** 时，客户端看到的是一个被截断却正常结尾的正文（带的话客户端能从长度不符看出来）。RFC 9113 第 8.1 节只许服务端在完整的应答之后这样做，所以这是对端不合规时的行为。M2b 不识别它（计划的「延后事项」第 2 行；协调者的裁定）。

### 没有覆盖的

- **h2c**（明文的 HTTP/2，不论升级还是先验知识）：明文的连接只讲 HTTP/1.1。
- **扩展 `CONNECT`**（RFC 8441，HTTP/2 上的 WebSocket）：不宣告，所以浏览器为 WebSocket 另开一条只给 `http/1.1` 的连接。
- **HTTP/2 的服务端推送与优先级**、**对上游的 PING 保活**，以及**跨客户端连接共享上游的 HTTP/2 连接**。
- `rurge mitm ca generate` / `export`、`http-api-tls` 与阶段 4 的手工验收清单（M2c）；`README.md`、`README_en.md` 与 `docs/requirements.md` 也在 M2c 收尾时改。
- 没有手工验收；**M2b 的提交都在本地，M1a、M1b、M2a 与 M2b 的提交 CI 都没有见过**，上面每一条只在一台机器上的一次运行里证明过（M2b 没有平台代码）。

## M2c　CA 工具与 `http-api-tls`

M2 的最后三件事：`rurge mitm ca generate` 与 `rurge mitm ca export` 两个命令、`http-api-tls` 生效（API 讲 HTTPS），与“`http-api-tls` 开着而没有可用的 CA”的那条诊断 `W0040`。没有新的端点；`GET /v1/mitm/ca` 照旧，HTTPS 上也取得到（设计第 13、14 节，计划 P6 ～ P14）。

**这一节写的是本地提交的行为：M2c 的提交也在本地，M1a、M1b、M2a、M2b 与 M2c 的提交 CI 都没有见过。**「M2b」一节「没有覆盖的」里有一条写着“`rurge mitm ca generate` / `export`、`http-api-tls` 与阶段 4 的手工验收清单（M2c）”——那是 M2b 收尾时的状态，M2c 之后以本节为准（手工验收清单仍是 `docs/acceptance/phase4-manual.md`，一项也没有做）。

### `rurge mitm ca generate --out <DIR>`

生成一张新的 CA（ECDSA P-256、10 年、`notBefore` 回拨 2 天、`BasicConstraints CA:TRUE pathLen:0` 与 `KeyUsage keyCertSign,cRLSign` 都关键、带 SKI，CN 是 `rurge MITM CA <生成的日期>`、O 是 `rurge`），往 `<DIR>` 里写四个文件：

| 文件 | 内容 |
| ---- | ---- |
| `rurge-ca.pem` | 证书（PEM），给客户端信任用 |
| `rurge-ca.cer` | 同一张证书（DER），有的导入方要这一种 |
| `rurge-ca.p12` | 证书与它的私钥（PKCS#12：3DES 加密、HMAC-SHA1 的 MAC，口令 24 个随机的字母与数字） |
| `rurge-mitm.conf` | 一个 `[MITM]` 节，`ca-p12` 是那份 PKCS#12 的 Base64、`ca-passphrase` 是口令 |

- **`<DIR>` 要已经存在**：不是一个已存在的目录就报 `{dir} is not a directory; rurge does not create it`，以 2 退出，什么也不写。
- **不覆盖**：四个路径里有任何一个已存在（坏掉的符号链接也算）就报 `{path} already exists; rurge does not overwrite it`，以 2 退出，什么也不写。
- **次序**：① 查目录 → ② 查四个路径 → ③ 生成 → ④ 依次写 pem、cer、p12、conf（每个都只新建）→ ⑤ 把 `rurge-ca.p12` 读回来交给 `Authority::load`（解码之外还查它是不是 CA、能不能签）→ ⑥ 打印。③ ～ ⑤ 失败以 **1** 退出：先把这一次已经写出的文件删掉，再写 `error: …`（⑤ 的消息是 `{path} does not read back as a CA rurge can sign with: {原因}; the files written are removed`），这一次没有写出的与本来就有的文件不动。
- **私密文件的权限**：`rurge-ca.p12` 与 `rurge-mitm.conf` 在 Unix 上建文件时就是 `0600`（标准库的 `OpenOptionsExt::mode`）；**Windows 上不设**（M2-D20），输出里提醒把它放在只有自己能读的目录里。`0600` 那一段带 `#[cfg(unix)]`，这台 Windows 开发机上编译不到，第一次编译它的是推送之后的 CI。
- **输出**：四个文件的路径与用途、把 `rurge-mitm.conf` 的内容贴进 Profile 的说明、各平台装 CA 的步骤（Windows `certutil -user -addstore`、macOS `security add-trusted-cert`、Debian / Fedora / Arch 的系统库、NSS、Firefox、Android、iOS）。**不打印口令、PKCS#12 与私钥**（设计第 13、15 节）。全文（逐字）在计划 P6，这里不重抄；它打印的安装命令 rurge 自己不执行，用例也只断言输出里有这些文字。

### `rurge mitm ca export -c <FILE> [--out <FILE>] [--der] [--platform <P>] [--data-dir <DIR>]`

从 Profile 里读出 CA 的**证书**（不含私钥）。只读 `[MITM]` 与 `[Keystore]`，Profile 别处的错误不管。`--platform` 与 `--data-dir` 同 `rurge status`：托管配置的 `[MITM]` 在数据目录里那份副本里。

- 给 `-c` 读到底：`Ok(Some(ca))` 就用它；`Ok(None)` 报 `the profile's [MITM] names no CA (ca-p12 or ca-keystore-name)`；CA 用不了报 `W0039` 的那一句消息。这三种都以 **2** 退出。
- 不给 `--out`：把证书的 PEM 打到标准输出，别的什么也不打，以 0 退出。
- 给 `--out`：先看它是否已存在（是 → `{path} already exists; rurge does not overwrite it`，以 2 退出），再以 `create_new` 写 PEM（或 `--der` 时写 DER），之后打印一行 `wrote the CA certificate to {path}`。写失败以 **1** 退出，并删掉没写完的那个文件。`--der` 只能与 `--out` 一起。

### `W0040`

```
warning[W0040]: http-api-tls is on but there is no usable MITM CA: the API does not start with this profile
  hint: configure a usable MITM CA, or turn http-api-tls off
```

条件：`http-api-tls = true`、Profile 写了 `http-api`，而 `[MITM]` 的 CA 用不了（`Authority::load` 是 `Err` 或 `Ok(None)`，不论 `W0039` 报没报）。没有 `http-api` 就没有要起的 API，不报。**不带位置**。

- **`rurge check`**：写在**标准输出**，每条诊断一行；有位置的带 `<文件>:<行>`，有 `hint` 的下面另起一行 `  hint: …`。诊断按位置排序，没有位置的排在前面，所以 `W0040` 在带位置的 `W0039` 之前。
- **`rurge run`**：写在**标准错误**，不带位置，同样按位置排过序；它与下面 P12 那一行 WARN 同时出现（起监听器之前打印诊断，起 API 的那一段再报一行 WARN）。`rurge run` 的日志行在**标准输出**。
- **重载**：重载加载的那一代也带这条诊断，`rurge run` 在重载那一步同样把它打在标准错误上（那时 API 仍在跑，用的还是之前那个 CA，见下面 P13 的 `api_ca_note`）。

### `http-api-tls = true`

API 的监听器外面套一层 TLS：每条连接按客户端的 SNI 签叶证书，没有 SNI 时按这条连接进来的**本地地址**（监听在 `::` 时 IPv4 的客户端连进来是映射地址，证书上还原成 IPv4）。证书由 `[MITM]` 的 CA 当场签，与解密的连接共用同一个叶证书缓存与会话缓存。ALPN **只有 `http/1.1`**，不收 0-RTT，不要客户端证书。

- **握手不占接受连接的循环**：接受之后在跟踪器上的任务里握手，握好的连接经一个通道交给 `axum` 的 `Listener::accept`，所以一个连上不说话的客户端挡不住下一条连接。同时在握手的至多 **64** 个（`TLS_HANDSHAKES`，与 `LINGER_MAX` 同数），多出来的当场关掉（DEBUG 一行 `http-api: too many TLS handshakes at once; a connection is closed`，带来源地址）；每个握手 **30 秒**（`TLS_HANDSHAKE_TIME`，M2-D21），到时关掉（DEBUG `http-api: a TLS handshake did not finish in time`）。握手失败也在 DEBUG（`http-api: a TLS handshake failed`，带原因）。超出的那些连接还要照 `Closing` 的规矩逗留，所以一个反复连接、从不握完手的客户端同时占 `TLS_HANDSHAKES + LINGER_MAX`（128）条——这是监听器这一侧的数，交给 `axum` 的连接不在其中。
- **没有可用的 CA**：API **不启动**，WARN 一行，rurge 照常跑，**从不退回明文**：
  `http-api-tls is on but there is no usable MITM CA: the API is not started (it never falls back to plain HTTP)`。
- **起来了**：标准输出一行 `api on https://{addr}`（明文仍是 `api on http://{addr}`），INFO 的 `http-api listening` 多一个字段 `tls = true`。
- **用哪个 CA**：每条新连接问一次 `Engine::api_tls()`，它是**最后一个生效过的** `Authority`（`MitmShared::last_live`）。所以重载换了 CA 之后新的连接由新的 CA 签——命令行经 `-c` 读的也正是新的；重载让 CA 用不了时 API 照旧、用的是之前那个。
- **不发 `close_notify`**：丢掉一条 TLS 流时不写它（登记）；hyper 平静地关一条连接时经 `poll_shutdown` 发它。
- **重载时关于 API 的告警**（每一句一行 WARN，都在新的一代换上时或之后打）：
  - `http-api` 变了：API 在跑 → `http-api changed in the profile; the API keeps its current address and key until rurge restarts`；API 没起而启动时有 `http-api` → `http-api changed in the profile; the API is not started until rurge restarts`；启动时没有 `http-api` → `http-api was added to the profile; the API is not started until rurge restarts`。
  - `http-api-tls` 变了（启动时有 `http-api`）：API 在跑且原来是 HTTPS → `http-api-tls changed in the profile; the API keeps serving HTTPS until rurge restarts`；API 在跑且原来是明文 → `http-api-tls changed in the profile; the API keeps serving plain HTTP until rurge restarts`；API 没起 → `http-api-tls changed in the profile; the API is not started until rurge restarts`。
  - 换上的这一代**没有**可用的 CA，而 API 在跑、讲的是 HTTPS → `http-api-tls is on but this profile has no usable MITM CA: the API goes on with the last CA it had`。
  - API 因没有 CA 而没起、启动时 `http-api-tls` 是开的，而换上的这一代**有**可用的 CA → `the profile has a usable MITM CA now: the API that http-api-tls kept from starting starts when rurge restarts`。
- **`GET /v1/mitm/ca`** 照旧，HTTPS 上也能取（客户端信任那张 CA 就行）。

### 命令行：`rurge status` / `reload` / `stop`

- **带 `-c <FILE>`**（或 `--data-dir` 里托管配置的副本）找到 API 的：`http-api-tls = true` 时走 HTTPS，根**只有** Profile 里那张 CA，按 API 的地址（IP）校验证书（叶证书带那个 IP 的 SAN）。`rurge status` 的第一行因此是 `rurge at https://127.0.0.1:<端口>`。
- **CA 用不了**：以 **2** 退出，消息是 `W0040` 那一句（`[MITM]` 一个 CA 也没写时）或 `W0039` 的消息（写了一个用不了的）。这与 `NOT_CONFIGURED` 是同一类错误：读不出要连哪里，一个请求也不发。
- **`--remote <地址> --key`**：不读 Profile、不知道 CA，照旧讲明文。对开着 `http-api-tls` 的实例因此连不上：服务端的握手失败、把连接关掉，命令行报 `error: cannot reach rurge at http://<地址>: …`、以 **1** 退出（M2-D21；没有加指定 CA 文件的参数，登记）。

### 没有覆盖的

- **HTTPS 的 API 只讲 `http/1.1`**：没有 HTTP/2（也没有 ALPN 的 `h2`）。
- **`--remote` 没有给 CA 文件的参数**：对开了 `http-api-tls` 的实例只能从本机带 `-c` 用。
- **没有把 CA 装进任何信任库**：`generate` 只打印步骤；测试的客户端一律是 rustls 的、根只有测试自己的 CA，全部证据在回环上。
- **`0600` 那一段只有 CI 会编译**（这台 Windows 开发机上编译不到）；Windows 上不设权限是登记的取舍。
- 没有手工验收，也没有经真浏览器试过；**M2c 的提交都在本地，CI 没有见过**。

## M3a　URL Rewrite 与 Header Rewrite

`[URL Rewrite]` 与 `[Header Rewrite]` 生效了：每个请求在选定策略之后依次过请求一侧的 Header Rewrite 与 URL Rewrite（`302` / `307` / `reject` 在这里答复，`header` 换掉拨号的目标而不重选策略），应答过应答一侧的 Header Rewrite；改过的 `Host` 在发出之前写回；换了目标的请求带着自己的对上游 TLS（设计第 6 ～ 8 节，计划 P15 ～ P18）。两个节不再是 `W0016` 的名单（`[Replica]` 认得并忽略、给 `I0005`；写明 `Content-Length` 或 `Transfer-Encoding` 的行保留、给 `W0041`），`rewrite` 开关与计数也做了（设计第 11、12 节）。`[Body Rewrite]` 与 `[Map Local]` 随 M3b，照旧列在 `W0016` 里。

**这一节写的是本地提交的行为：M3a 的提交在本地，M1a、M1b、M2a、M2b 与 M2c 的也一样，CI 都没有见过。**

### `/v1/features/rewrite`

`GET` 给 `{"enabled":bool}`。`POST {"enabled":bool}` 给 `{}`，并把开关写进 `state.json`（`version` 是 2，`features.rewrite` 是布尔）；切换立即对**下一个请求**生效，已经走到选路的请求照它开始时的样子走完，开着的连接不因此断掉（设计第 11 节）。

缺省是**开**（没设过时；总设计 P4-D19、M3-D24）。启动时 `state.json` 里记着的值由 `Engine::restore_switches` 装回开关，不写盘。

`POST` 之后有一行 INFO（`crates/rurge-api/src/routes/features.rs`），原文与字段照代码：

```
enabled=false rewrite switched via http-api
```

即消息 `rewrite switched via http-api`，字段 `enabled`（`true` / `false`）。

关着时四个节一概不生效：`[URL Rewrite]`、`[Header Rewrite]`、`[Body Rewrite]` 与 `[Map Local]`（后两个到 M3b 才有效果）；计数一个也不动，备注一条也不留。开关在 `ConnUpstreams::route` 里读（`crates/rurge-engine/src/http.rs`），所以每个请求各读一次。

用例 `the_rewrite_switch_is_read_switched_and_kept`（缺省开、切换、`state.json` 的 `version` 2 与 `features.rewrite`）、`a_daemon_restores_the_rewrite_switch_from_state_json`（`crates/rurge/tests/cli.rs`：起守护进程之后读得到文件里记着的值）、`the_rewrite_switch_turns_every_rewrite_off`（关着时 `header` 规则与 `302` 规则都不生效，请求照原样到源站）。

### `GET /v1/rurge/http` 的 `rewrite`

应答在 `mitm` 之后多一个对象，键的次序固定（设计第 12 节，计划 P18）：

```json
{"sniffed":{"tls":0,"http":0,"other":0,"timeout":0,"closed":0},"requests":0,"mitm":{"decrypted":0,"untrusted":0,"failed":0,"noRequest":0},"rewrite":{"url":0,"header":0,"body":0,"mapLocal":0,"skipped":0,"failed":0}}
```

正文的字节恰好是这一串，用例 `rurge_http_counts_start_at_zero`（`crates/rurge-api/tests/api.rs`）写着它。六个数：

- `url`：有 URL Rewrite 规则起了作用的请求（`header`、`302`、`307`、`reject` 四种模式都算，一个请求至多一次）；
- `header`：头被 Header Rewrite 改过的请求（两个方向，一个请求至多一次）；
- `body`：正文被 Body Rewrite 改过的请求（到 M3b 之前一直是 0）；
- `mapLocal`：由 Map Local 答复的请求（到 M3b 之前一直是 0）；
- `skipped`：留了备注而跳过的改写，每一个计一次；
- `failed`：因为重写而失败的请求（框架头变了，两侧都算）。

自进程启动起累计，**重载不清零**：计数器是 `rurge-http` 的 `RewriteCounters`，一个引擎一份，放在 `EngineShared.rewrite` 里（不在 `Runtime` 里），`Rewrite::build` 每一代拿到同一个。用例 `counts_survive_a_reload`、`notes_and_counts_reach_the_api_shape`（`crates/rurge-engine/tests/rewrite.rs`）。

`body` 与 `mapLocal` 两个键 M3a 就定下了、恒为 0，为的是 M3b 加它们时不改形状（M3-D25）。

### 请求记录的备注

每一个起作用的与每一个跳过的改写各留一条备注，走 `/v1/requests/*` 的 `notes`（设计第 8.5 节，M3-D18）。**备注里从不带头的值、正文、查询串与网关的地址**：`{式子}` 是规则的原文（`Pattern::source` 照写的，不是编译之前改写过的），`header` 那一句里的 URL 只有 scheme、主机、端口与路径，没有查询串。措辞照代码逐字：

**起作用的（`Done`）**

```
URL Rewrite: header by {式子} -> {scheme}://{authority}{路径}
URL Rewrite: 302 by {式子}
URL Rewrite: 307 by {式子}
URL Rewrite: reject by {式子}
Header Rewrite: {request|response} {header-add|header-del|header-replace|header-replace-regex} {名字} by {式子}
```

`header` 那一句里的 URL 是新 URL 的 `note()`：缺省的端口不写、IPv6 带方括号、路径至少是 `/`，查询串去掉。

**跳过的（`Skipped`，每一条 `skipped` 加一）**

```
URL Rewrite skipped: {式子}: the rewritten URL is not a valid http, https, ws or wss URL
URL Rewrite skipped: {式子}: the rewritten Location cannot be a header value
URL Rewrite skipped: {式子}: there is no IPv4 gateway
URL Rewrite skipped: {式子}: the regular expression hit its backtracking limit
Header Rewrite skipped: {式子}: the regular expression hit its backtracking limit
Header Rewrite skipped: {式子}: header-replace-regex made a value a header cannot carry
Header Rewrite skipped: Host cannot be removed
Header Rewrite skipped: the rewritten Host is not a valid HTTP/2 authority
```

三处特别的：

- **`header-del Host`（或任何让 `Host` 一行也不剩的规则）**留两条：那一条规则自己的 `Done`（`Header Rewrite: request header-del Host by {式子}`），与之后放回 `Host` 时的 `Header Rewrite skipped: Host cannot be removed`。请求的头最终没有变，`header` 却数了一次——设计第 12 节"头被 Header Rewrite 改过的请求"那一句对这一种不成立（设计第 22 节第 12 条，修正记录第 36 行）。
- **一条 `header-replace-regex` 规则至多一条 `Skipped`**：同名的几行都失败时留最后失败的那一行的备注，`skipped` 也只数一次（设计第 22 节第 11 条，修正记录第 35 行）。
- **HTTP/2 上游上写不回的 `Host`**：改了 `Host` 而值当不了 `:authority`（例如 `a/b`）时留 `Header Rewrite skipped: the rewritten Host is not a valid HTTP/2 authority`，一个请求至多一条，借来的连接被拒、在第二条连接上重发也一样；这时请求以连接的目标为 authority（修正记录第 44、52 行）。用例 `a_host_that_is_no_authority_is_noted_once_across_a_resend`。同一个 `Host` 用绝对形式交给转发的 HTTP 代理时也不写回（设计第 7.4 节，M3-D30）。

用例 `no_header_value_reaches_the_notes_or_the_log`（`crates/rurge-engine/tests/rewrite.rs`：替换 `Authorization`、URL 带查询串时备注与会话日志里都没有它们）、`notes_never_carry_values_or_queries`（`crates/rurge-http/src/rewrite/mod.rs`）。

### 失败与答复

**框架头被改（两侧都失败）**：请求一侧的规则改了 `Content-Length` 或 `Transfer-Encoding` 时请求不拨号、不发往上游；应答一侧的则丢掉上游的应答，那条上游连接不回池。两侧都答 502 的错误页（`show-error-page` 关着时关掉连接，HTTP/2 上是重置这个流），页上的两句话不同，照 `crates/rurge-http/src/engine.rs` 逐字：

- 请求一侧：`A Header Rewrite rule changed Content-Length or Transfer-Encoding, so the request was not sent.`
- 应答一侧：`A Header Rewrite rule changed Content-Length or Transfer-Encoding of the response, so the response was not passed on.`

记录里的失败原因两侧都是 `Header Rewrite changed Content-Length or Transfer-Encoding`，备注都是 `Header Rewrite failed: {式子}: Content-Length or Transfer-Encoding changed`，`failed` 各加一；头确实被改过，`header` 也各加一（设计第 12 节；用例 `a_request_whose_framing_changes_fails_before_the_dial`、`a_response_whose_framing_changes_fails`、`the_manual_header_rewrite_framing_limit`）。

**`302` / `307`**：答一个只有 `Location` 与 `Content-Length: 0` 的应答（hyper 自己加的头，例如 `date`，不归 rurge 管），连接照常保持；记录的状态码是 302 或 307，`url` 加一（设计第 6.4 节，M3-D10）。`Location` 不能当头的值时这条规则放弃、留备注、接着求下一条。

**`reject`**：同 `REJECT` 一族对一个请求的答复，只是错误页的说明是 `The request was rejected by a URL Rewrite rule.`：`show-error-page-for-reject` 为真时答 403、标题 `Request rejected`；为假时关掉连接（HTTP/2 上重置这个流）。记录是"被拒"（设计第 6.5 节、第 22 节第 7 条，M3-D27 的拒绝仍由拨号那一侧做）。

### 不过重写的请求

三处（设计第 8.4 节）：

- **嗅探到的连接上客户端发来的 `CONNECT` 与绝对形式的请求**：客户端在对它自己的代理说话，这两种照原样转给连接的目标，一条规则也不求、不计数（M3-D29；用例 `an_absolute_form_request_on_a_sniffed_connection_is_not_rewritten`、`connect_and_absolute_form_on_a_sniffed_connection_are_not_rewritten`）。
- **选定 REJECT 一族的请求**：`ConnUpstreams::route` 看第一条尝试的终点，是 REJECT 就不给这一代的重写——照 M1b 在拨号时被拒绝，不过任何重写、计数不动（M3-D27；用例 `a_rejected_request_is_not_rewritten`）。
- **`rewrite` 开关关着**：见上面 `/v1/features/rewrite`。

### 建不成一代的重写

`build_rewrite`（`crates/rurge-engine/src/runtime.rs`）在每一代建一次；两个节都空时不读根、交 `None`。建不成只关这一代、不关进程，除了一行日志没有别的痕迹（设计第 8.2 节）：

- 读根的阻塞任务没有交回（`shared.roots` 是 `None`，`spawn_blocking(union_root_store)` 交回的是 `Err`：那个任务 panic 了，或者运行时正在关）——DEBUG：`the roots of the origin side cannot be read; rewrites are off for this generation`。系统的信任库读不出**不是**这一行：`union_root_store`（`crates/rurge-net/src/tls.rs`）照样并上 webpki-roots，另写一行 WARN `no native root certificates loaded; the union is webpki-roots alone`，这一代的重写照常建；
- `Rewrite::build` 失败（例如空的根，rustls 报 `NoRootAnchors`）——WARN：`rewrites are off: cannot set up TLS: {e}`。

### 没有覆盖的

- `[Body Rewrite]` 与 `[Map Local]`（M3b）：两节照旧在 `W0016` 里，`rewrite` 的 `body` 与 `mapLocal` 恒为 0。
- jq、`br`、`zstd` 与 `regex` 一侧的 Body Rewrite（M3c，等总设计 Q4）。
- 没有手工验收：`docs/acceptance/phase4-manual.md` 一项也没有做过，M3a 没有往里加条目（M3b 收尾时加）。
- 没有观察过 Surge 在下面这些地方的实际行为，照设计写：换目标的请求仍走选定的策略、新的主机不进 fake 地址池；`header` 模式的 `{{{GATEWAY_ADDRESS}}}` 是普通的字；`wss` 也算合法。
- 全部证据是回环上的用例：没有经真浏览器、没有对真实的站点试过。**M3a 的提交都在本地，CI 没有见过。**
