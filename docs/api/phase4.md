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

## M3b　Body Rewrite 与 Map Local

`[Body Rewrite]` 的正则形式与 `[Map Local]` 生效了（设计第 9、10 节，计划 P9 ～ P19）：每个请求在 URL Rewrite 之后先过 Map Local（命中就由 rurge 答复、一个字节也不拨，第 6 步），再过请求一侧的 Body Rewrite（第 7 步）；应答在应答一侧的 Header Rewrite 之后过 Body Rewrite（第 12 步）。两个节的读法在 `rurge-config`：式子先用 `regex` 的多行与 CRLF 编、编不了再用 fancy-regex，jq 的行给 `W0042` 并跳过那一行，Map Local 的路径来自下载到的文本时是 `W0033`，两节也从 `W0016` 的名单拿掉了。正文这一侧：请求至多 32 MiB、应答至多 10 MiB 才缓冲，`gzip`、`x-gzip` 与 `deflate` 解压之后才改，正文必须是严格的 UTF-8；chunked、带 `Expect: 100-continue` 的与 HTTP/2 上没有长度的请求原样转发。Map Local 的数据有文件、URL、`text`、`base64` 与 `tiny-gif` 五种，URL 的那种在控制面是外部资源的新一类 `data`。计数 `body` 与 `mapLocal` 开始计（两个键 M3a 就定下了，M3-D25）。jq 的 Body Rewrite 只有 `W0042`；`br` 与 `zstd` 的解码、对上游收窄 `Accept-Encoding` 都不在这一构建里（M3c 等总设计 Q4）。

**这一节写的是本地提交的行为：M3b 的提交在本地，M1a、M1b、M2a、M2b、M2c 与 M3a 的也一样，CI 都没有见过。**

本节订正上面「M3a」一节的三句（规矩是历史的段落不逐处改、由后写的段落订正）：`/v1/features/rewrite` 那一节里"（后两个到 M3b 才有效果）"（第 386 行），以及 `body` 与 `mapLocal` 两条后面各一句"（到 M3b 之前一直是 0）"（第 402、403 行）。从 M3b 起，`[Body Rewrite]` 与 `[Map Local]` 也随 `rewrite` 开关（关着时四个节一概不生效，见「M3a」一节的 `/v1/features/rewrite`），`body` 与 `mapLocal` 照下面数。

### 请求记录的备注

每个起作用的与每个跳过的重写各留一条备注（`notes`）。备注里只有规则的 URL 式子（`{式子}` 是 `Pattern::source` 照写的原文），没有 URL、查询串、正文、头的值，也没有 Map Local 的数据、路径与来源。

**起作用的（`Done`）**

```
Body Rewrite: {request|response} by {式子}
Map Local: {file|text|base64|tiny-gif} by {式子}
```

`Body Rewrite` 那一句里的中段是规则的方向（`request` 或 `response`）；一条规则真的改了正文才留这一条，配上而没有改动的不留。`Map Local` 那一句里的类型是 `MapData::kind()`：文件与 URL 的数据都是 `file`，其余照写 `text`、`base64`、`tiny-gif`。

**跳过的（`Skipped`，每一条 `skipped` 加一）**

```
Body Rewrite skipped: {式子}: {原因}
Map Local skipped: {式子}: the regular expression hit its backtracking limit
```

`{原因}` 是下面之一（照 `crates/rurge-http/src/rewrite/body.rs` 的字面）：

```
the body is not valid UTF-8
the body is over 10 MiB
the decoded body is over 32 MiB
the decoded body is over 10 MiB
content-encoding {名字} is not supported
the content-encoding is not supported
the body cannot be decoded as {名字}
the request body is chunked
the request expects 100-continue
the request body has no length
the regular expression hit its backtracking limit
```

解码失败时**每一条**配上的规则各留一条同样的原因；正文不是严格的 UTF-8 时同样每条一条。一条规则里有几对时，有一对回溯超限就整条作废（正文回到这一条规则之前的样子）、留一条 `the regular expression hit its backtracking limit`，接着求下一条规则。Map Local 的式子在比 URL 时回溯超限的那一条也留一句、接着比下一行。

**失败的（`Failed`）**

```
Body Rewrite failed: {式子}: the request body is over 32 MiB
Map Local failed: {式子}: the file cannot be read
Map Local failed: {式子}: the data is not available yet
```

请求正文超过 32 MiB 时只有**第一条**配上的规则留一条，`failed` 加一。Map Local 的两种各有一条记录，见「记录里 Map Local 的失败」。

### `GET /v1/rurge/http` 的 `rewrite` 里的 `body` 与 `mapLocal`

两个数都自进程启动起累计、重载不清零（同 M3a 那一节的四个）：

- `body`：正文被 Body Rewrite 改过的请求。一个请求只数一次——请求与应答两个方向都改也只数一次（设计第 12 节，计划 P13）。
- `mapLocal`：由 Map Local 答复的请求，`HEAD` 与 204、304 的答复在内；打不开数据、答 502 的不算。

`skipped` 照旧是每一条 `Skipped` 加一（M3b 的这些也算在其中）。`failed` 多两种：请求正文超过 32 MiB、Map Local 的数据读不到（答 502 的那两种）。下面这些**不算** `failed`：缓冲应答时源站停住或出错（那是上游正文的失败，一条 502 的记录）、读 Map Local 的文件读到一半出错（这个请求已经算在 `mapLocal` 里）、缓冲请求时客户端停住或出错（一条失败的记录）。

两个键的形状与 M3a 那一节的例子一字不差（`{"url":0,"header":0,"body":0,"mapLocal":0,"skipped":0,"failed":0}`），只有 `body` 与 `mapLocal` 从这里起会有非零值（`skipped` 与 `failed` 从 M3a 起就在数）；用例 `body_and_map_local_counts_reach_the_api_shape`（`crates/rurge-engine/tests/rewrite.rs`）、`counters_count_and_snapshot`（`crates/rurge-http/src/rewrite/mod.rs`）。

### 记录里 Map Local 的失败

Map Local 的规则命中了、数据却给不出来时，答 502 的错误页（`show-error-page` 关着时什么也不答、关掉连接；HTTP/2 上是重置这个流），会话以失败结束，`failed` 加一，这一条 `Failed` 备注的 `{原因}` 与记录是同一件事的两种说法：

- **文件打不开、读不到元数据或不是普通文件**（例如是个目录）：记录是 `Map Local: cannot read {路径}`（`{路径}` 是 `Path::display`），备注的原因写 `the file cannot be read`，错误页的说明是 `The file a Map Local rule names cannot be read.`
- **URL 的数据还没下载好**：记录是 `Map Local: the data from {来源} is not available yet`，备注的原因写 `the data is not available yet`，错误页的说明是 `The data a Map Local rule names has not been downloaded yet.` `{来源}` 是 `rurge_config::remote::origin` 的结果（`scheme://host[:port]/…`，末尾的 `/…` 是照写的省略号，例如 `https://d.test/…`），不出现的是用户名、密码、路径与查询串。

**读到一半出错**不是 502：答复已经开始，文件在那之后读不了、变短了或停住了。记录是 `Map Local: reading {路径} failed: {错误}`、`Map Local: {路径} became shorter while it was read` 或 `Map Local: reading {路径} stalled`（`{错误}` 是那次读的错误），会话以失败结束，客户端收到的正文以错误终止；`mapLocal` 已经数过这个请求，`failed` **不再**加，也不另留备注（计划 P15、P16，设计第 22 节第 21 条）。

### 那一行 WARN 与一行 DEBUG

只有请求一侧、只有正文**不能缓冲**时才有（三种原因见「正文不缓冲、缓冲不成与 trailer」）。每条配上的规则：

- 一定有一行 DEBUG，消息 `request body not rewritten: {原因}`，字段 `host`（会话的目标主机）与 `pattern`（规则的 URL 式子）；
- 另有一行 WARN，消息相同、只有字段 `pattern`，只在 `should_warn` 放行时出现：同一条规则（这一代里的这一行）第一次总是放行，之后一小时之内不再放行（`crates/rurge-http/src/rewrite/body.rs` 的 `BodyRule` 里 `warned` 一格记着上一次的时刻，`WARN_EVERY` 是 3600 秒）。它属于一代的重写，重载之后重新计时。

日志里没有 URL、查询串、正文与头的值。正文的改写跑在阻塞线程上（M3-D21），那个任务 panic 时另有一行 ERROR `body rewrite task failed: {错误}`，正文照原样发出去、不留备注、不计数。

### 外部资源的 `data`

`[Map Local]` 的 `data` 以 `http://` 或 `https://` 开头时是一个 URL：加载时就读成 `url::Url`，挂在这一代的资源管理器上，种类是新的一类 `data`（`ResourceKind::MapLocal`，设计第 10.3 节，计划 P19）。`GET /v1/external_resources` 照常列出它（字段与别的种类一样，`type` 是 `data`），`POST /v1/external_resources/update` 可以按键更新它，`"all"` 也更新它。

- 列表里那一项的 `path`（标签）是 `map local data {键的前 8 位} ({来源})`，`{来源}` 只到 `rurge_config::remote::origin`（`crates/rurge-engine/src/runtime.rs` 的 `build_rewrite`，修正记录第 67 行）。URL 是凭据，不进日志、不进标签的其余部分。
- 同一个 URL 写在几行里只有一项：第一次登记定下种类与标签（用例 `the_same_map_local_url_is_one_resource`）。
- 它挂在这一代的资源管理器上，所以 `rewrite` 开关关着时照样下载（`build_rewrite` 的文档注释，设计第 10.3 节）；不联网的一代（资源管理器是 offline 的）从不下载——数据目录里有这个 URL 早先下载的副本时照那份副本答（用例 `offline_mode_uses_the_cache_only`），没有副本时才答 `Map Local: the data from {来源} is not available yet`。
- 登记时的更新间隔是 `None`，即资源管理器缺省的 86 400 秒（一天）：在线的一代照这个间隔在后台再下载它，与别的外部资源一样；大小上限照资源管理器的缺省（64 MiB）。

用例 `map_local_urls_are_listed_as_data`、`map_local_url_data_that_never_came_answers_502`、`the_same_map_local_url_is_one_resource`（都在 `crates/rurge-engine/tests/rewrite.rs`）。

### 大小的边界与超过时的样子

两个上限都按二进制的兆字节算（M3-D20）：请求 32 MiB（33,554,432 字节）、应答 10 MiB（10,485,760 字节），备注里写的就是 `32 MiB` 与 `10 MiB`（`crates/rurge-http/src/rewrite/body.rs` 的 `REQUEST` 与 `RESPONSE`）。

- **请求超过上限**：`Content-Length` 的下界已经超过时一个帧也不读；否则一帧一帧读、越过上限即停。两种都关掉连接（HTTP/2 上重置这个流），`failed` 加一，记录 `Body Rewrite: the request body is over 32 MiB`，第一条配上的规则留 `Body Rewrite failed: {式子}: the request body is over 32 MiB`。
- **应答超过 10 MiB 不算失败**：每条配上的规则留 `Body Rewrite skipped: {式子}: the body is over 10 MiB`，已经读到的字节先发出去、其余照原样接着流过（`ReadAhead::resumed`），没有 WARN，`failed` 不动。
- **解压**：每一层都以"上限加一个字节"为界地读，解开之后越过上限是 `the decoded body is over 32 MiB` 或 `the decoded body is over 10 MiB`（请求与应答各用各的上限文字）。

### 空的正文与解压

`Content-Encoding` 每一行以 `,` 分项、去掉首尾空白，空的项与 `identity` 去掉，从最后一项往前逐层解（设计第 9.4 节）。名字不分大小写：`gzip` 与 `x-gzip` 用 `MultiGzDecoder`，`deflate` 看头两个字节是不是 zlib 头（是就 `ZlibDecoder`，否则 `DeflateDecoder`）。

一层 `gzip`、`x-gzip` 或 `deflate` 的输入是空的时解成空的、不经解码器（`decode` 的文档注释；修正记录第 40 行，协调者的裁定）：`MultiGzDecoder` 会把零字节叫作截断的头，而手册的 `^$` 写法本来就该在这种空正文上成立。`br`、`zstd`、`compress` 与别的名字照旧以 `content-encoding {名字} is not supported` 跳过，空不空都一样；名字不是 RFC 9110 的 token（或那一行不是可见的 ASCII）时是 `the content-encoding is not supported`。解不开的层是 `the body cannot be decoded as {名字}`。

### 正文不缓冲、缓冲不成与 trailer

**不配请求正文的规则**：要升级的请求与嗅探的连接上客户端发来的 `CONNECT` 不求请求正文的规则，也没有备注（`crates/rurge-http/src/engine.rs` 的 `needs_h1` 那一段）。配上之后的三种不缓冲，各留一条 `Skipped`、一行 DEBUG、可能一行 WARN：

- 带 `Transfer-Encoding` 头的（不论它的值是什么）：`the request body is chunked`；
- 带值是 `100-continue`（不分大小写）的 `Expect` 头：`the request expects 100-continue`；
- 客户端讲 HTTP/2、正文没有结束、`size_hint().exact()` 是 `None`：`the request body has no length`。

这三种的正文照原样转发。缓冲的请求读到客户端停住或出错时，记录是 `request body stalled` 或 `request body failed: {错误}`，连接关掉（HTTP/2 上重置这个流），不另留备注、`failed` 不动。

**不配应答正文的规则**：`HEAD` 的应答、1xx、204 与 304 不求应答正文的规则，也没有备注。缓冲应答时源站停住或出错，经 `failed` 答 502（错误页的说明分别是 `The upstream server stopped sending the response body.` 与 `The upstream server's response body failed.`），记录是 `upstream body stalled` 或 `upstream body failed: {错误}`；这一代的错误页关着时不答、关掉连接（HTTP/2 上是重置这个流）。这两种都不算 `failed`，那条上游连接也不回池。

**改了正文之后**（两个方向一样）：去掉 `Content-Encoding` 与 `Transfer-Encoding`，写新的 `Content-Length`（改成空的也是，`0`），trailer 不要；不把正文压回去（设计第 9.6 节）。配上而没有改动的，照原样交回读到的字节与 trailer（连同压缩）。

**trailer**：到 HTTP/1.1 客户端的 trailer 一个也不转，有没有规则都一样——引擎把 `Trailer` 当逐跳的头去掉（`crates/rurge-http/src/headers.rs` 的 `HOP_BY_HOP`），hyper 的 HTTP/1.1 编码器只写 `Trailer` 里声明过的；两端都讲 HTTP/2 时照转。这从 M1b 起就是这样，不是 M3b 加的（修正记录第 20 行；兼容性清单那一句的订正是任务 9 的事）。

**Map Local 的 204 与 304**：数据不读（文件不开、URL 的数据不问），也不带 `Content-Length`。`HEAD` 照样开文件或取数据、量出 `Content-Length`，不发正文。

### 没有覆盖的

- **jq、`br`、`zstd`**：jq 的 Body Rewrite 只有 `W0042`（这一行跳过），`br` 与 `zstd` 的应答不改写（备注是 `content-encoding br is not supported` 或 `content-encoding zstd is not supported`）；对上游收窄 `Accept-Encoding`、fancy-regex 的升级都归项目所有者（设计第 21 节 Q4、M3c）。
- **不把改写过的正文压回去**：改过的应答是明文，`Content-Encoding` 去掉了（设计第 9.6 节）。
- **没有手工验收**：`docs/acceptance/phase4-manual.md` 的 M3 一节一项也没有做过（M3b 任务 8 加的）；**M3b 的提交都在本地，CI 没有见过**。
- **全部证据是回环上的用例**：源站是测试自己的 HTTP / TLS 服务器，客户端是测试自己写的；没有经真浏览器、没有对真实的站点试过。
- **没有观察过 Surge 在这些地方的实际行为**，照设计与手册写：`deflate` 的两种头怎么判、空的 `Content-Encoding` 层怎么算、改写过的正文的 `Content-Length` 与 trailer 的形状、Map Local 的 `Content-Type` 表之外的名字（一律 `application/octet-stream`）。

## M4a　抓包

抓包生效了（设计第 4 ～ 9 节，计划 P2 ～ P15）：一个引擎一份只在内存里的存储，装着经 HTTP 引擎的请求的请求头、应答头与两个正文的副本——按块记账、有两个上限、从最早的一条挤掉、存不下时截断或丢掉；开关抓到的进 `default`，命中的 `always-capture` 进那一行写的名字；`/v1/features/capture` 与 `/v1/rurge/capture/…` 六个端点读出它，其中一个把会话导出成 HAR 1.2；`rurge run` 多了 `--capture-size` 与 `--capture-body-size`（各有环境变量）。抓包缺省**关着**：关着时存储是一条空队列（写了 `always-capture` 而命中的除外，见下），请求照旧走，不抄正文，也不包应答正文（设计第 5.1 节）。

**这一节写的是本地提交的行为：M4a 的提交在本地，M1a、M1b、M2a、M2b、M2c、M3a 与 M3b 的也一样，CI 都没有见过。**

### `/v1/features/capture`

`GET` 给 `{"enabled":bool}`。`POST {"enabled":bool}` 给 `{}`，并把开关写进 `state.json`（`version` 是 2，`features.capture` 是布尔）；开关在请求走到 `ConnUpstreams::route` 时读（见下面），所以切换对之后走到那里的请求生效，已经过了那一处的请求照它当时的样子走完，开着的连接不因此断掉（设计第 5.1 节）。

缺省是**关**（没设过时；`Engine::capture_on` 的初值、设计第 5.1 节）。启动时 `state.json` 里记着的值由 `Engine::restore_switches` 装回开关，不写盘。

`POST` 之后有一行 INFO（`crates/rurge-api/src/routes/features.rs`），原文与字段照代码：

```
enabled=false capture switched via http-api
```

即消息 `capture switched via http-api`，字段 `enabled`（`true` / `false`）。与 `mitm`、`rewrite` 同一个写法。

每个请求在 `ConnUpstreams::route` 里读一次开关（`crates/rurge-engine/src/http.rs` 的两个分支都调 `Engine::capture_for`），所以切换只对之后走到这一处的请求管用；关掉之后已经存进去的条目照样读得到，直到被挤掉、被清除或进程退出。

用例 `the_capture_switch_is_read_switched_and_kept`（`crates/rurge-api/tests/api.rs`：缺省关、切换、`state.json` 的 `version` 2 与 `features.capture`）、`the_capture_switch_is_kept_in_state_json_and_restored`（`crates/rurge-engine/tests/capture.rs`：写进 `state.json`，下一次起引擎装回来）、`nothing_is_captured_while_the_switch_is_off` 与 `the_switch_captures_into_default`（同文件：关着时一条也不存、开着时进 `default`）、`turning_the_switch_off_keeps_what_was_captured`（同文件：关掉之后已经存着的还在）。

### 什么请求被抓

- **开关抓到的进 `default`**（`rurge_http::capture::DEFAULT_SESSION`，设计第 5.3 节）。
- **`always-capture=<名字>` 在开关关着时也抓**：命中的那个请求存进那个名字的会话（设计第 5.2 节）。名字照写的原样，区分大小写；开关开着时命中的仍进它的名字，不进 `default`。
- **`always-capture=default`** 与开关抓到的是同一个会话（登记，设计第 4.6 节）。
- **空的 `always-capture=`**：`rurge check` 给一条 `` `always-capture` needs a capture session name; the parameter is ignored ``（`W0012`），这一行**照常进规则**；引擎读它时去掉首尾空白、空的不填（`crates/rurge-engine/src/engine.rs` 的 `always_capture`）。不带 `=` 的 `always-capture` 照旧是未知参数 `W0003`。
- **`FINAL` 上的照样起作用**（用例 `always_capture_on_final_captures_too`，`crates/rurge-engine/tests/capture.rs`）。
- **命中 `RULE-SET`、`DOMAIN-SET` 与逻辑规则时**，读的是那一行顶层规则在 `[Rule]` 里的参数（`decision.matched` 就是那一行的下标）；规则集文件里的行上写的 `always-capture` **不读**（设计第 5.2 节；任务 0 的核对 V4）。
- **只在求规则时**：出站模式是直连、出站模式是全局代理而全局策略设了且认得、或会话属于 `external` 程序时，`choose_policy_with` 不求规则就定了策略，抓包的槽是空的——这些请求只有在开关开着时进 `default`，`always-capture` 不起作用（用例 `the_direct_mode_reads_no_rule_and_so_no_always_capture`）。全局代理而全局策略没设或不认得时，`choose_policy_with` 告警一次（WARN `outbound mode is proxy but the global policy is unset or unknown; routing by rules`）、照规则走，`always-capture` 照读（`crates/rurge-engine/src/engine.rs`）。
- **只抓本来就经 HTTP 引擎的请求**（M4-D8）：`always-capture` 不让一条连接进引擎、也不让 TLS 被解密；要抓原样转发的连接，另写 `force-http-engine-hosts` 或 `[MITM]`。

抓不到的（设计第 5.4 节，一条也不存）：

- 没有走到 `ConnUpstreams::route` 的：HTTP/2 上的 `CONNECT`（答 501）、hyper 自己答 400 的坏请求、请求头超时、只发了半个头、经 HTTP 监听器来的不是 `http://` 绝对形式的请求、`route` 还没有回来 hyper 就丢下的请求；
- 承载请求的连接自己的记录（握手失败、握手之后一个请求也没有）：那不是请求；
- 不经 HTTP 引擎的：原样转发的 TCP、不解密的 TLS、UDP 与 QUIC、DNS。

### 抓到什么

**请求在两个取样点取**（设计第 6.1 节，M4-D11，计划 P9）：

1. **离开 rurge 时**：要发往上游的请求，在第 5 步 `headers::strip` 之后、`absolute_form` / `origin_form`、`fit` 与 `write_host` 之前取一份。头是 Header Rewrite 之后、去掉逐跳头之后的；URL 是 URL Rewrite 之后的（`header` 模式换过目标的是新的 URL）；版本是**客户端**的版本（讲 HTTP/2 的客户端记 `HTTP/2.0`，不记转成 HTTP/1.1 之后的样子）。
2. **求规则时**：没有发往上游的请求（被拒绝、URL Rewrite 当场答复、Map Local、Header Rewrite 改了框架头而失败、请求正文超过 Body Rewrite 的上限而断开、拨号失败、终止……）记的是它求规则时的样子：`route` 回来之后马上取一份，URL 是记录里的 URL（客户端问的），头照 `headers::strip` 去掉逐跳的头。

走到第 1 个取样点的请求以第 1 份为准。两份里都**没有 rurge 自己的转发头**：对上游 HTTP 代理的 `Proxy-Authorization` 由 `absolute_form` 加在两个取样点之后；客户端交给 rurge 监听器的 `Proxy-Authorization`（`password@` 写的口令）在取样之前就被去掉了（用例 `the_captured_request_is_the_one_that_leaves_without_rurges_forward_headers`，`crates/rurge-http/src/engine.rs` 的测试模块）。

**应答头是客户端看到的那个**：状态码、版本与头（应答一侧的 Header Rewrite 与 Body Rewrite 之后、去掉逐跳头之后），没有 hyper 写出应答时自己加的头（`date`、HTTP/1.1 的 `content-length` / `transfer-encoding` 等）。版本记客户端看到的（M4-D28）：讲 HTTP/2 的客户端连接上一律 `HTTP/2.0`。rurge 自己答复的（错误页、拒绝页、`302` / `307`、Map Local）与上游来的一样记。

**两个正文各自按块抄**：请求正文抄的是发往上游的字节（Body Rewrite 改过的是改过之后的），应答正文抄的是交给客户端的帧；trailer 一个也不抄。

**存进去的时刻**（设计第 6.4 节；照 `crates/rurge-http/src/capture/slot.rs` 的 `CaptureSlot`）：

- **升级与隧道**（应答是 101，或嗅探到的连接上的 `CONNECT` 得到 2xx）：交出应答头的那一刻，之后的字节不抄，条目的结局是 `upgraded`；
- **其余有应答的**：应答正文交完或被 hyper 丢下、**而且**这个请求的记录已经结束，两者后到的那一刻；
- **没有应答的**（`handle` 返回 `Err(Close)`，或 hyper 在应答之前丢下了这个请求——HTTP/2 上客户端重置了流）：知道没有应答、而且记录已经结束，两者后到的那一刻。

`REJECT-DROP` 先结束记录、再停留 `drop_hold`（缺省 30 秒）才交回 `Err(Close)`：条目在**记录结束时**就存进去，不等停留完，`total` 与记录一样只有几毫秒、不含停留（设计 M4-D29，计划 P8：由 `when_finished` 的回调认出，用例 `a_dropped_request_is_captured_before_its_hold_ends`）。

**被 `REJECT` 一族的规则拒绝的请求**：开着 `show-error-page-for-reject` 时条目的应答就是那一页（403），关着（缺省）时拒绝不答话、条目**没有应答**（`response` 是 `null`；修正记录第 79 行 ③，用例 `a_request_a_reject_rule_refuses_is_captured` 给的是 `show-error-page-for-reject = true`）。

**时间**（设计第 6.5 节，计划 P9 ④）：`connect` 从句柄建好到上游连接可用（借来的连接是借到的那一刻）、`dns` 是 `Trail` 记的解析、`wait` 从连接可用到第一个上游字节、`receive` 从第一个上游字节到结束、`total` 从句柄建好到结束；拿不到的是 `null`。`reused` 是这个请求借了池里的或共享的连接。API 与 HAR 都向下取整到毫秒（`Duration::as_millis`），用例 `a_capture_session_lists_its_entries_newest_first` 与 `an_entry_is_shown_whole_with_four_headers_masked` 给的是 35.999 毫秒与 12.999 毫秒这种不整的时长，改成四舍五入时它们会红（修正记录第 53、54 行）。

一条在**借来的连接上没有接下、在新拨的连接上再发一次**的请求（重发），`reused` 仍是真、`connect` 也是借的那一刻——`connect_time` 只有第一次的算，两者一致（计划 P9 ④，登记）。

**`remoteAddr`**：`Trail` 的对端，就是记录的 `remoteAddr`（借来的连接是拨它时记下的）；一个字节也没有拨的（拒绝、Map Local、当场答复的 `302`）是 `null`。

**`outcome`** 是 `completed` / `upgraded` / `rejected` / `failed`；**`rejectKind`** 是被拒的请求的 REJECT 种类（`REJECT`、`REJECT-TINYGIF` 一类，照 `RejectKind::name()`），不是拒绝的 `null`。**`error`** 与请求记录的 `error` 是同一句（计划 P5：`set_error` 写的那一句，没有时是失败自己的消息）；**被拒绝的请求也可能有 `error`**。**`notes`** 是记录的备注——M3 的重写备注在这里。

**遮住**（设计第 6.6 节，M4-D15）：`authorization`、`proxy-authorization`、`cookie`、`set-cookie` 四个头的值在 API 与 HAR 里写 `***`，**存储里保留原值**；正文与 URL（含查询串）照给。头值里不是 UTF-8 的字节在 API 里按 U+FFFD 显示，存储里照原样。抓到的值算不算"API 输出"的凭据是所有者的 Q1（设计第 17 节）。

**rurge 自己的凭据从不进抓包**：带口令的监听器（`http-listen = password@…`）与带凭据的上游 HTTP 代理，条目的请求头里**根本没有** `proxy-authorization`（用例 `a_listener_password_is_not_captured`、`an_upstream_proxy_credential_is_not_captured`，`crates/rurge-engine/tests/capture.rs`；口令在监听器的入站、`arm` 的头与 `leaving` 的头三处被去掉，修正记录第 73、80 行）。

### 两个上限与命令行

两个上限是 rurge 专有的运行时选项，只走命令行参数与环境变量（FR-CFG-17，设计第 4.2 节，计划 P13），**只有 `rurge run` 有**：

| 参数 | 环境变量 | 缺省 |
| ---- | -------- | ---- |
| `--capture-size <SIZE>` | `RURGE_CAPTURE_SIZE` | 100 MiB（104,857,600 字节） |
| `--capture-body-size <SIZE>` | `RURGE_CAPTURE_BODY_SIZE` | 5 MiB（5,242,880 字节） |

写法是十进制的字节数，后面可以跟一个不分大小写的 `k`、`m`、`g`（乘 1024、1024²、1024³）；不是数字、带符号、带小数点、带空白与超出 `u64` 的都是错的。`0` 合法：总量 0 什么也存不进（每条都丢掉）；单条 0 只存头（用例 `a_body_limit_of_zero_keeps_the_heads_only`）。

`--capture-size <SIZE>` 的帮助（`crates/rurge/src/cli/run.rs` 的文档注释，源码里折成两行）：

```
Keep at most this much of the captured requests in memory: bytes, or a number with k, m or g (default 100m)
```

`--capture-body-size <SIZE>` 的帮助：

```
Keep at most this much of each captured body (default 5m)
```

写错的两种，照 `parse_size` 的原文（反引号里是写的原样）：

```
invalid size `{写的}`: expected a number of bytes, optionally followed by k, m or g
size `{写的}` is too large
```

两者都由 clap 的 `value_parser` 报出，`rurge run` **以 2 退出**、不启动（用例 `bad_capture_sizes_are_refused` 用 `12x` 与 `17179869184g` 各试一次，后者是乘积溢出）。用例 `sizes_are_bytes_with_an_optional_binary_suffix` 与 `the_body_size_is_lowered_to_the_capture_size` 是 `crates/rurge/src/cli/run.rs` 的单元用例；`the_capture_limits_come_from_the_options_or_the_variables`（`crates/rurge/tests/cli.rs`）看的是经 API 读回来的两个数。

**单条大于总量时降到总量**。使用者写了单条（参数或环境变量）而它大于总量时，启动时一行 WARN 说降成了多少：

```
the capture body size ({单条} bytes) is larger than the capture size; lowered to {总量} bytes
```

这一行在存储建好的时候写（`crates/rurge/src/cli/run.rs` 里 `CaptureStore::new(limits)` 之前的那一句），排在两行 `listening on` 与 `api on` **之前**——没有这条告警时 `api on` 是启动输出的第三行（同一个文件里起 API 那一段的注释；修正记录第 63、69 行）。单条是缺省值、被使用者写的总量压下去时照样降，**不告警**（使用者只想要一个小一点的总量）。用例 `a_body_size_over_the_capture_size_is_lowered_with_a_warning` 自己起进程、从第一行读起（因为它要断言的正是 `spawn_command` 会丢掉的那些行）。

**记账**（设计第 4.3 节，计划 P6）：一个条目的账是请求头与应答头的每一行"名字 + 值 + 32"字节、URL、方法、规则、每个策略、每条备注与 `error` 的字节、两个正文**存下的**字节，再加一个固定的 `ENTRY_OVERHEAD`。这个数今天 **7168**，是量出来改的：512 时装满的存储比基线多出总量的 1.5207 倍，改成 7168 之后是 1.0266（修正记录第 76 行，量法见下面「量到的内存」）。它的注释写着它是量出来的、量的是什么样的条目，而且**只对量的那种形状成立**——头的行数多得多的条目，实际的占用仍比账上的多。

于是：

- **挤掉**：存不下时从队列最前面（最早存进去的）一条一条拿掉，**不分会话**；拿掉的条目正被 API 读着时，内存到读完才放掉（用例 `the_oldest_entries_go_first_whatever_their_session`、`an_evicted_entry_lives_on_while_it_is_held`）。
- **截断**：一个正文超过单条上限时只存开头的那么多字节（`Truncated::Limit`）；总量挤不出地方时从那一块起不再存（`Truncated::Budget`）。两种都照样记着见过的全部字节数（`size`）。
- **丢掉**：一个条目的头存不进去（总量太小）时整条不要（`dropped` 加一）。一条最小的条目的账也在 7168 字节以上，所以 `--capture-size` 小于它时**什么也存不下**（照实记，不加判断；用例 `an_entry_whose_head_does_not_fit_is_dropped`）。
- **四个数** `stored`、`evicted`、`dropped`、`truncated` 自引擎启动起累计，**重载与清除都不清零**（用例 `clearing_one_session_or_all_keeps_the_four_counts`）。

**存进去的条目两侧的头是复制出来的**，不与 hyper 的读缓冲共享：hyper 1.11.1 解 HTTP/1 的头时，值是那条连接读缓冲的切片（`hyper-1.11.1/src/proto/h1/role.rs`），克隆只加引用计数，每条存着的条目会把那条连接的缓冲留在内存里，账就不对了。`Pending::commit` 在拿存储的锁之前把两侧的头设成各自一次分配的副本（`crates/rurge-http/src/capture/store.rs` 的 `owned`，修正记录第 80、81 行；用例 `the_stored_headers_hold_none_of_the_buffer_they_were_read_from`）。进行中的条目的头在存进去时才记账（设计第 4.3 节的例外）。

### `GET /v1/rurge/capture`

存储此刻的样子（设计第 7.1 节）：`limits`（两个上限）、`used`（记着的字节：队列里的条目加进行中的预留）、`entries`（队列里的条目数）与四个数。键的次序就是声明 `StatusJson` 的次序：

```json
{"limits":{"total":104857600,"body":5242880},"used":0,"entries":0,"stored":0,"evicted":0,"dropped":0,"truncated":0}
```

开关的状态读 `/v1/features/capture`；这里的两个上限是**这一个进程**用的那对（`rurge run` 按参数换掉过就是换掉之后的）。用例 `the_capture_status_says_the_limits_and_the_counts`（`crates/rurge-api/tests/api.rs`）。

### `GET /v1/rurge/capture/sessions`

只有**有条目**的会话，按名字排（`BTreeMap` 的次序）；`bytes` 是这个会话的条目记着的字节：

```json
[{"name":"default","entries":12,"bytes":34567}]
```

一个条目也没有时是 `[]`。用例 `capture_sessions_are_listed_by_name`。

### `GET /v1/rurge/capture/sessions/{name}`

这个会话的条目**由新到旧**，至多 `limit` 条。`limit` 照 `/v1/requests/recent`：缺省 100（`DEFAULT_LIMIT`），`0` 答 400 `limit must be at least 1`。会话没有一条条目（从来不存在、被挤掉、被清除）时答 404 `` unknown capture session `{名字}` ``。会话名里的百分号由 axum 解开，所以名字带空格、斜杠或中文都够得到（用例 `odd_session_names_are_reached_through_the_path`）。

摘要的键照 `SummaryJson` 的声明次序：

```json
[{"id":42,"startedMs":1761984000000,"method":"GET","url":"http://example.test:8080/a?b=1","status":200,"outcome":"completed","requestBodySize":0,"responseBodySize":5,"truncated":false,"totalMs":4}]
```

`url` 是抓到的那一个（`CONNECT` 是 `host:port`，见下）；`status` 是应答的状态码，没有应答时 `null`；`outcome` 是 `completed` / `upgraded` / `rejected` / `failed`；`requestBodySize` 与 `responseBodySize` 是**见过的**字节数（不是存下的）；`truncated` 是两个正文有一个被截断；`totalMs` 向下取整。用例 `a_capture_session_lists_its_entries_newest_first`。

### `GET /v1/rurge/capture/entries/{id}`

一条的全部。两个参数按代码的次序读：**先 `?decode`**（只认 `true` 与 `false`，缺省 `false`，别的答 400 `decode must be true or false`），**再**把 `id` 读成 `u64`（读不了答 400 `entry id must be a number`）；两个都写错时报的是 `decode` 那一句（修正记录第 50 行 ③）。没有这一条答 404 `unknown capture entry {id}`。

正文是一张 `json!` 建的对象，键按 `serde_json` 的 `Map`（`BTreeMap`）排，也就是**字母序**，不是设计的例子里的次序；`timings` 是结构体，里面是声明的次序：

```json
{"error":null,"id":42,"notes":[],"outcome":"completed","policy":["DIRECT"],"rejectKind":null,"remoteAddr":"127.0.0.1:8080","request":{"body":{"base64":"","size":0,"stored":0,"truncated":null},"headers":[{"name":"host","value":"example.test:8080"},{"name":"cookie","value":"***"}],"method":"GET","url":"http://example.test:8080/a?b=1","version":"HTTP/1.1"},"response":{"body":{"base64":"aGVsbG8=","size":5,"stored":5,"truncated":null},"headers":[{"name":"content-type","value":"text/plain"}],"status":200,"version":"HTTP/1.1"},"rule":"DOMAIN,example.test,DIRECT","session":"default","startedMs":1761984000000,"timings":{"connectMs":1,"dnsMs":null,"waitMs":2,"receiveMs":1,"totalMs":4,"reused":false}}
```

- `version` 是 `format!("{:?}", v)` 的写法：`HTTP/1.1`、`HTTP/2.0`。
- `headers` 照 `HeaderMap::iter` 的次序，每一项 `{"name","value"}`，名字是小写；四个凭据头的值写 `***`；值里不是 UTF-8 的字节按 U+FFFD 显示（存储里照原样）。
- `body`：`size` 是见过的字节、`stored` 是存下的、`truncated` 是 `null` / `"limit"` / `"budget"`、`base64` 是存下的字节（`STANDARD` 编码）。没有应答时 `response` 是 `null`（不是少一个键）。
- `timings` 的五个时长拿不到时是 `null`，`totalMs` 总有；`reused` 布尔。
- `remoteAddr` 是 `SocketAddr` 的文字（IPv6 带方括号），一个字节也没有拨的是 `null`。
- `rejectKind` 不是拒绝的 `null`；`error` 没有时 `null`；`rule` 没有时 `null`；`policy` 与 `notes` 是数组，可以是空的。

**`?decode=true`** 时两个 `body` 各多两个键：`decoded` 与 `notDecoded`（都在字母序里，`decoded` 在 `base64` 之后、`notDecoded` 在 `size` 之前）。三种情形（设计第 7.5 节，计划 P7）：

- 解开了（`Ok(Some(b))`）：`decoded` 是 `true`、`base64` 换成解开之后的字节、`notDecoded` 是 `null`；
- 没有要解的编码（`Ok(None)`）：`decoded` 是 `false`、`notDecoded` 是 `null`，`base64` 不动；没有要解的层的正文被截断了也走这一支，有要解的层而被截断的是下一支（`the body was truncated`）；
- 解不开（`Err(r)`）：`decoded` 是 `false`、`notDecoded` 是 `r` 的那一句。

`notDecoded` 的五句照 `NotDecoded` 的 `Display`：

```
the body was truncated
content-encoding {名字} is not supported
the content-encoding is not supported
the body cannot be decoded as {名字}
the decoded body is over {上限} bytes
```

判的次序照 `Body::decoded`（`crates/rurge-http/src/capture/mod.rs`）：先读 `content-encoding` 的各层（空的项与 `identity` 不算层），那一行不是可见的 ASCII 时就是第三句；没有要解的层是上面的 `Ok(None)`；有层而正文被截断的，不管层叫什么都是第一句；其余的从最后一层往前解，第一个解不了的那一层定是哪一句——`br` 与 `zstd` 是第二句（`content-encoding br is not supported`），名字不是 RFC 9110 的 token 时是第三句，数据解不开是第四句，解开之后超过单条上限时是第五句（上限是这一个进程的单条上限）。用例 `decode_undoes_gzip_and_says_why_it_cannot`、`what_cannot_be_decoded_says_why`。

### `POST /v1/rurge/capture/clear`

请求体：

- **没有请求体，或全是空白**：清除全部；
- **`{}`**（一个 JSON 对象、没有 `session`）：清除全部；
- **`{"session":"<名字>"}`**：只清这个会话；**这个会话没有条目时 `{"cleared":0}`，不答 404**；
- **别的**（不是对象、`session` 不是字符串、正文读不了）：400 `the body must be a JSON object; "session", when given, must be a string`（常量 `BAD_BODY`；这一句里不引用请求发来的东西）。

应答是 `{"cleared":N}`（N 是清掉的条数）。进行中的条目不受影响（之后照常存进去）；四个数不清零；正被 API 或 HAR 握着的条目内存到读完、写完才放掉。用例 `clearing_takes_one_session_or_all`、`an_export_holds_its_entries_while_they_are_cleared`。

### `GET /v1/rurge/capture/sessions/{name}/har`

这个会话导出的 HAR 1.2（设计第 8 节，计划 P12），`Content-Type: application/json`，条目**由旧到新**。没有这个会话时是同一个 404 `` unknown capture session `{名字}` ``。

`log` 的外层（`crates/rurge-api/src/har.rs` 的 `document`）：

```json
{"log":{"version":"1.2","creator":{"name":"rurge","version":"0.1.0"},"pages":[],"entries":[ …
```

`creator.version` 是 `env!("CARGO_PKG_VERSION")`，也就是 `rurge-api` 的版本（这一构建里是 `0.1.0`）；`pages` 永远是空的——rurge 没有"页面"这个概念。

一条的映射照设计第 8.2 节的表，写成一张表就是：

| HAR | 取自 |
| --- | ---- |
| `startedDateTime` | 条目的 `started`，RFC 3339、毫秒、UTC（`YYYY-MM-DDTHH:MM:SS.mmmZ`） |
| `time` | 下面 `timings` 里不是 `-1` 的各项之和 |
| `request.method` / `url` / `httpVersion` | 抓到的请求；`CONNECT` 的 `url` 是 `host:port`；版本写 `HTTP/1.1`、`HTTP/2.0` |
| `request.cookies` / `response.cookies` | `[]`：不拆（`cookie` 与 `set-cookie` 遮住了，拆开会漏出它们） |
| `request.headers` / `response.headers` | 头，四个凭据头遮住；没有应答时 `[]` |
| `request.queryString` | URL 的查询串拆成 `{"name","value"}`；方法是 `CONNECT` 时是 `[]`；URL 读不了时也是 `[]` |
| `request.postData` | 请求正文**见过的字节数大于 0 时才有这个键**：`mimeType` 取 `content-type`（没有时空串）；正文没有截断、没有 `content-encoding`、是 UTF-8 时 `text` 是它，否则 `text` 是 Base64 并另写 `"_encoding":"base64"`（HAR 1.2 的 `postData` 没有 `encoding`）；截断的再加 `comment` |
| `request.headersSize` / `bodySize` | `-1` / 请求正文见过的字节数 |
| `response.status` / `statusText` / `httpVersion` | 状态码 / `canonical_reason`（没有时空串）/ 版本；**没有应答时**是 `0`、`""`、`""` |
| `response.content` | 见下 |
| `response.redirectURL` | `location` 头，没有时空串 |
| `response.headersSize` / `bodySize` | `-1` / 应答正文见过的字节数；没有应答时 `bodySize` 是 `-1` |
| `cache` | `{}` |
| `timings` | 见下 |
| `serverIPAddress` | `remoteAddr` 的 IP；**没有时不写这个键** |
| `_rurgeId` / `_session` / `_outcome` / `_notes` | 条目的 ID、会话、结局、备注 |
| `_rejectKind` / `_error` | 被拒的种类与失败的说明；**没有时不写这个键** |

没有应答的条目照 mitmproxy 的写法：`response` 是 `status` `0`、`statusText` `""`、`httpVersion` `""`、`headers` `[]`、`content` 只有 `{"size":0,"mimeType":""}`、`redirectURL` `""`、`bodySize` `-1`（用例 `har_says_no_response_as_mitmproxy_does`）。

**`response.content`**：能解开的（同上面 `?decode`，只是这里总是解）是 `size` = 解开之后的字节数、`compression` = `size − bodySize`（可以是负的）、`text` 是解开之后的；没有编码的与解不开的都是 `size` = 存下的字节数、`text` 是存下的字节。`text` 是 UTF-8 时照写，否则 Base64 并加 `"encoding":"base64"`（**应答一侧的键名是 `encoding`**，请求一侧才是 rurge 自己的 `_encoding`）；`mimeType` 取 `content-type`。**`comment`** 有两句，都有时以 `"; "` 接起来，**截断的那一句在前**：

```
{存下的字节数} of {见过的字节数} bytes stored
not decoded: {NotDecoded 的 Display}
```

**`timings`**（设计第 8.3 节，M4-D27）先把 `Trail` 记的解析时间压进它所在的那一段：连上了的 `d = min(dns, connect)`，没连上的 `d = min(dns, total)`。各项（`ms(x)` 是向下取整到毫秒，减法都用 `saturating_sub`）：

| HAR | 条件 | 值 |
| --- | ---- | -- |
| `blocked` | 借了连接的（`reused`）而 `connect` 有值 | `ms(connect − d)` |
| | 别的 | `-1` |
| `dns` | `d` 有值 | `ms(d)` |
| | 没有 | `-1` |
| `connect` | 没有借连接而 `connect` 有值 | `ms(connect − d)` |
| | 别的 | `-1` |
| `ssl` | 总是 | `-1`（握手的时间在 `connect` 里，不单列） |
| `send` | 总是 | `0`（rurge 不单独量发请求的时间，它在 `wait` 里） |
| `wait` | `connect` 与 `wait` 都有 | `ms(wait)` |
| | 只有 `connect` | `ms(total − connect)` |
| | 没有 `connect` | `ms(total − d)` |
| `receive` | `receive` 有值 | `ms(receive)` |
| | 没有 | `0` |

`time` 是上面不是 `-1` 的各项之和，所以它恰好等于各项相加（各项先向下取整，`totalMs − time` 因此在 0 到 3 之间）。借了连接的条目 `blocked` + `dns` + `wait` + `receive` 合起来是 `total`，新拨的是 `dns` + `connect` + `wait` + `receive`，没有连上的只有 `dns` 与 `wait`。用例 `har_timings_add_up_to_the_total`（`crates/rurge-api/src/har.rs` 的测试）、`a_session_is_exported_as_har_oldest_first`、`har_bodies_are_text_or_base64_and_say_what_was_left_out`、`har_splits_query_strings_and_gives_connect_none`、`har_masks_the_four_headers_and_leaves_cookies_empty`（`crates/rurge-api/tests/api.rs`）。

`entries` **一条一条写出**（设计第 8.4 节）：处理函数先取会话的条目快照（一份 `Arc` 的 `Vec`，空的答 404），交给 `axum::body::Body::from_stream` 一条流；每条在流被 poll 到它时才序列化成 JSON，所以除了快照，导出过程只握一条的字节，不在内存里拼整个文件。快照里的 `Arc` 让导出期间被挤掉或被清除的条目完整到写完（用例 `an_export_holds_its_entries_while_they_are_cleared`）。

一条的键次序与上面 `entries/{id}` 同理是字母序（`json!` 的 `BTreeMap`），与上面那张表的次序不同——表说的是取哪一项。

### 量到的内存

量的办法（设计第 4.5 节，计划 P15，任务 8）：**调试构建**、`rurge-http` 的**库内**用例、两条各单独跑一次；峰值工作集由操作系统读（Windows 上是起一个 `powershell` 读本进程的 `PeakWorkingSet64`，与 `crates/rurge-tun/src/tests/bench/peak.rs` 的写法相同）。两条带 `#[ignore]`，缺省的 `cargo test` 不跑：

- `capture::memory::an_empty_store_is_the_baseline`：只建一份空的存储，作基线；
- `capture::memory::a_full_store_at_the_default_limits`：按缺省的上限（100 MiB / 5 MiB）把存储装满，条目像真实的页面请求——请求头与应答头各 14 行（名字约 12 字节、值约 40 字节）、URL 约 80 字节、一条备注、请求正文空、应答正文 4 KiB（四块，每块 1 KiB）；每 1024 条里有一条 6 MiB 的应答正文（64 KiB 一块，截断在 5 MiB）；一直存到累计挤掉的条数超过已存条数的十分之一。

三次运行的数（`memory-task-8.log`、`memory-task-8-fix-1.log`，**一台机器上的一次运行，不是 CI**）：

| 运行 | 基线峰值 | 装满的峰值 | 多出的 = 装满 − 基线 | 多出的 / 100 MiB |
| ---- | -------- | ---------- | -------------------- | ---------------- |
| `ENTRY_OVERHEAD` 是 512 时 | 17,477,632 | 176,930,816（9216 条，kept 8192，`used` 99,917,824） | 159,453,184 | 1.5207 |
| 改成 7168 之后 | 17,477,632 | 125,124,608（6144 条，kept 5345，`used` 104,856,567） | 107,646,976 | 1.0266 |
| 修正轮 1 之后（修正记录第 81 行） | 17,473,536 | 131,170,304（同样是 6144 条、kept 5345、`used` 104,856,567） | 113,696,768 | 1.0843 |

第一次的比值超过 1.25，所以照计划 P15 把 `ENTRY_OVERHEAD` 从 512 改成 7168（512 + (159,453,184 − 104,857,600) / 8192 = 7176.5，取 64 的倍数是 7168）；改完的 1.0266 与修正轮 1 之后的 1.0843 都没有再超过 1.25，所以不再改（修正记录第 76、81 行）。最后两次装满分出约 6 MB，**原因没有查过**（两次各只跑了一次，修正记录第 82 行），不写成是"存进去的条目复制自己的头"的代价——那一次的复制一个顶替一个，每条存着的分配个数与改之前一样。

**缺省的上限不因此改**（100 MiB / 5 MiB，M4-D7）：要不要改归项目所有者（设计第 17 节）。

### 日志与诊断

抓包只多两行日志（设计第 9 节）：开关切换的 INFO（`capture switched via http-api`）与单条上限被降到总量的启动 WARN。条目的任何内容——头、正文、URL、查询串——**不进任何日志**，任何一级都不；用例 `captured_content_never_reaches_the_log` 在 TRACE 一级（`LogCapture::everything()`）跑一个抓包有内容的请求，再在抓到的日志里找那些字。

诊断不设新码：空的 `always-capture=` 那条是既有的 `W0012`（`W_INVALID_VALUE`，同一个码也用在别的值上，例如写错的 `loglevel`；设计第 9 节）。

### 登记的几件

- **抓包开着时**，经引擎的请求多一份内存里的副本与按块记账；**关着时引擎照旧**（`capture` 是 `None`，不包应答正文）——设计第 11 节第 1 条。
- **`always-capture` 不让连接进 HTTP 引擎、也不让 TLS 被解密**（M4-D8，设计第 11 节第 2 条）；它只在求规则时起作用。
- **抓到的请求头去掉了逐跳的头与 rurge 自己的转发头**；抓到的应答头没有 hyper 写出时加的头；头的次序是 `HeaderMap` 的，不一定是线上的次序（设计第 11 节第 3 条）。
- **`default` 会话与使用者写的 `always-capture=default` 是同一个**（设计第 11 节第 4 条）。
- **HAR 导出期间被挤掉或被清除的条目，内存到导出写完才放掉**；进行中的条目的头不记账（设计第 11 节第 5 条）。
- **抓到的 `CONNECT` 的 `url` 是 `host:port`，而记录里它的 `url` 是 `null`**（设计第 11 节第 9 条，M4-D26，API 与 HAR 同）。
- **抓到的应答的版本是客户端看到的**，可能与上游的不同：讲 HTTP/2 的客户端连接上一律 `HTTP/2.0`（设计第 11 节第 10 条，M4-D28）。
- **条目的 `error` 与请求记录的 `error` 是同一句**（计划 P5）：`set_error` 写的那一句，没有时是失败自己的消息；**被拒绝的请求也可能有 `error`**。
- **`REJECT-DROP` 的条目在记录结束时存进去**，不等 `drop_hold` 的停留，`total` 不含停留（计划 P8，同设计 M4-D29）。
- **借来的连接上没有接下、在新拨的连接上再发一次的请求**：`reused` 仍是真，`total` 里的 `connect` 也是借的那一刻——两者一致（计划 P9 ④）。

### 没有覆盖的

- **读一条与 HAR 的解开不认 `br` 与 `zstd`**：`?decode=true` 给 `notDecoded` 是 `content-encoding br is not supported` 一类，HAR 的 `content.comment` 是 `not decoded: …`，正文照存下的（压缩着的）字节给。它们随 M3c、等总设计 Q4。
- **Dashboard 在阶段 6**：这一构建只有 API，没有界面。
- **M4b 的事不在这一节**：`receiveMs` 之外记录的新字段、九种直通的原因、`remoteAddr` 的新口径、`dnsMs` 在途（设计第 10 节）。
- **没有手工验收**：`docs/acceptance/phase4-manual.md` 的 M4 一节一项也没有做过（M4a 任务 9a 加的）。
- **M4a 的提交都在本地，CI 没有见过**。
- **全部证据是回环上的用例**：源站与客户端都是测试自己的，没有经真浏览器、没有对真实的站点试过；导出的 HAR 没有被任何浏览器导入过。
- **没有观察过 Surge 的抓包在这些地方的实际行为**，照设计与手册写：会话怎么分、`default` 的名字、`always-capture` 起作用的时机、条目的 ID、HAR 里 rurge 自己的 `_` 键。

## M4b　请求记录的完整

记录补全了（设计第 10、11 节，计划 P3 ～ P13）：每条记录多三个键——`receiveMs`、`requestBodyBytes` 与 `responseBodyBytes`；可嗅探的会话没有进 HTTP 引擎、或判为 TLS 而 `[MITM]` 的名单命中却没有解密时，备注写九种原因之一；`remoteAddr` 改成「这条会话离开 rurge 时所经的那条连接或载体的远端」，复用的 TCP 载体、QUIC 一族、WireGuard 与 UDP 流都有了这个地址；`dnsMs` 在解析途中也有数。这一节写 API 与记录看得见的部分，抓包那一半见上一节。

**这一节写的是本地提交的行为：M4b 的提交在本地，M1a ～ M4a 的也一样，CI 都没有见过。**

### 三个新键

`/v1/requests/recent` 与 `/v1/requests/active` 的每条记录多三个键（设计第 10.1 节，M4-D21）：

- **`receiveMs`**（紧跟 `dnsMs`）：从收到第一个上游字节到会话结束的毫秒数；会话还没有结束、或从没有收到过上游的字节时是 `null`。它与 `connectMs`、`firstByteMs` 合起来是总设计第 9.1 节的「取连接、等应答头、收正文」三段。每种会话都有，不只是 HTTP 请求。
- **`requestBodyBytes`**：一个 HTTP 请求的请求正文发往上游的字节数。
- **`responseBodyBytes`**：应答正文交给客户端的字节数，包括 rurge 自己答复的（错误页、拒绝页、`[Map Local]` 的答复）。

后两个紧跟 `statusCode`（`RequestJson` 的字段次序就是 JSON 里键的次序）；不是 HTTP 请求的会话（`method` 是 `null`）两个都是 `null`。`up` / `down` 照旧是上游一侧的全部字节、含头，与这两个数无关。

**在哪里数的**（M4-D30）：记录在句柄 `finish` 的那一刻拍下，之后句柄上的计数进不了记录。所以两个正文在引擎把每一帧交出去时、记录结束之前数：请求正文在 `Idle` 交出每一帧时数（两种版本都数）；应答正文在 `Tail` 与 `LocalFile` 交出每一帧时、`complete()` 之前数；rurge 自己的答复在建好应答、`finish` 之前按应答正文的长度记下。这不在 M4a 包住应答正文的那一层：那里总漏掉最后一帧、自己的答复总是 0。

四种不普通的情形：

- **`HEAD`**：hyper 1.11.1 的 HTTP/1.1 服务端不写 `HEAD` 应答的正文，HTTP/2 服务端照样发出去。所以 rurge 自己的页答 `HEAD` 时，HTTP/1.1 上 `responseBodyBytes` 是 0、HTTP/2 上是页的长度（照实数，不改 hyper 的行为，没有用例，登记）；上游与 `[Map Local]` 对 `HEAD` 的应答本来没有正文，两种版本都是 0。用例 `a_head_answer_counts_no_body_on_http1`。
- **升级之后**（应答是 `101`，或嗅探到的连接上的 `CONNECT` 得到 2xx）：之后的字节不算正文，算在 `up` / `down`；`responseBodyBytes` 只是升级之前那一小段应答的正文（通常没有）。用例 `an_upgrade_counts_no_body`。
- **源站提前作答、请求正文还在上传**：`requestBodyBytes` 只到记录结束的那一刻，之后接着上传的进不了记录（设计第 11 节第 11 条）；没有用例（登记）。
- **客户端在应答正文中途走掉**：`responseBodyBytes` 是到那一刻交给它的字节数，不多于源站写的。用例 `a_client_that_leaves_mid_body_counts_what_was_handed_over`。

两个数是 `u64`（`AtomicU64::fetch_add` 累加；要 2^64 字节才会溢出回绕，实际到不了）。用例：`the_body_counters_add_up_apart_from_the_bytes`、`first_byte_at_is_counted_from_the_start`（`crates/rurge-inbound/src/session.rs`）、`a_finished_record_has_its_receive_time_and_its_body_bytes`、`a_session_that_is_no_http_request_has_no_body_bytes`、`the_receive_time_is_none_without_a_first_byte`（`crates/rurge-engine/src/observe.rs`）、`the_new_keys_sit_where_the_design_puts_them`（`crates/rurge-api/src/routes/requests.rs`，键序），引擎里的 `the_request_body_sent_upstream_is_counted_before_the_record_ends`、`a_one_frame_answer_is_counted_before_the_record_ends`、`a_chunked_answer_counts_every_frame`、`rurges_own_answers_count_their_length`、`map_local_answers_count_their_length`、`an_h2_connect_501_counts_its_page`、`an_h2_client_counts_its_bodies_too`，与经 API 的端到端 `the_request_records_carry_the_three_new_keys`（`crates/rurge-api/tests/api.rs`：一个 9 字节的 `POST` 与一个不可嗅探端口上的 `CONNECT`，后者两个正文是 `null`）。

### 直通的原因

可嗅探的会话没有进 HTTP 引擎、或判为 TLS 而 `[MITM]` 的名单命中却没有解密时，记录的 `notes` 写一句（设计第 10.2 节，M4-D22）。九句照 `crates/rurge-engine/src/serve.rs` 的私有常量：

| 情形 | 备注 |
| ---- | ---- |
| `always-raw-tcp-hosts` 命中 | `Passed through: always-raw-tcp-hosts` |
| `always-raw-tcp-keywords` 命中 | `Passed through: always-raw-tcp-keywords` |
| 嗅探判为既不是 HTTP 也不是 TLS（`force-http-engine-hosts` 没有把它留在引擎里） | `Passed through: neither HTTP nor TLS` |
| 嗅探等到了时限 | `Passed through: sniffing timed out` |
| 名单命中、`mitm` 开关关着 | `Not decrypted: MITM is off` |
| 名单命中、这一代没有可用的 CA | `Not decrypted: no usable CA` |
| 名单命中、客户端给的协议（ALPN）rurge 都不解密 | `Not decrypted: the client offers no protocol rurge decrypts` |
| 名单命中、来源不在 `client-source-address` 里 | `Not decrypted: the client is not in client-source-address` |
| 名单命中、名字是 rustls 不收的、签不了证书 | `Not decrypted: no certificate can be made for this name` |

**判断的次序**是名单 → 开关 → CA → 来源 → 名字 → ALPN（设计第 10.2 节，计划 P6 ③）：名单没有命中的 TLS 原样转发是 TLS 的常态，不写，来源不允许而名单也没有命中的同样不写。`Mitm::decides` 的次序是名单 → 来源 → 名字 → ALPN，交回五种（`Decrypt`、`NotListed`、`SourceBlocked`、`NoCertificate`、`AlpnMismatch`）；解不解密与 M4b 之前相同，只有标签变了。

**什么时候写**：在拨号、建好这条会话的句柄时写（`crates/rurge-engine/src/engine.rs` 的 `dial_noted` 与 `PassedBy`），一条会话至多一句；之后被任何规则拒绝的照样带着它。客户端在嗅探时关掉的（`Sniffed::Closed`）没有表示任何协议，不写；暂停的引擎、fake 地址背后没有名字的不写；嗅探等到时限的照写（`force-http-engine-hosts` 命中也照旧拨号）。预匹配排在名单之后：名单没有命中、最先成立的是预匹配的才不写，命中 `always-raw-tcp-*` 而又被预匹配的规则拒绝的照写那一句（M4-D31，设计第 18 节第 11 条，计划 P6 ⑤ 的登记）。不可嗅探的端口上命中名单的照写（名单在端口之前）。备注里没有名字、地址、凭据，也没有别的变量。

用例：`crates/rurge-engine/tests/sniff.rs` 的 `always_raw_tcp_hosts_and_keywords_are_noted`、`a_listed_raw_host_on_a_port_that_is_never_sniffed_is_noted_still`、`a_listed_raw_host_that_the_rules_reject_keeps_its_note`、`neither_http_nor_tls_is_noted`、`a_sniff_that_waits_out_its_time_is_noted`、`a_client_that_closes_during_the_wait_has_no_note`、`a_paused_engine_writes_no_reason`、`pre_matching_writes_no_reason_unless_a_list_came_first`、`an_unlisted_tls_session_has_no_note`，与 `crates/rurge-engine/tests/mitm.rs` 的 `a_listed_session_with_mitm_off_is_noted`、`a_listed_session_without_a_usable_ca_is_noted`、`a_listed_session_from_a_source_not_allowed_is_noted`、`a_listed_session_whose_alpn_rurge_does_not_decrypt_is_noted`、`a_listed_name_no_certificate_can_be_made_for_is_noted`、`a_decrypted_session_has_no_reason`。

### `remoteAddr`

**新口径**（M4-D23）：这条会话离开 rurge 时所经的那条连接或载体的远端——会话自己开的、从池里借来的、与别的会话共用的都算；链式策略照旧是第一跳。`docs/api/phase1.md` 第 62 ～ 64 行那一段写的「会话**自己**开的那条 TCP 连接」、复用与隧道与 UDP 一律 `null`，是 M4b 之前的口径；M4b 起照新口径，那一段作为历史不改（订正在这一节）。

各载体实际记下的（设计第 10.3 节的表，按任务 5 ～ 7 实际做的写）：

| 载体 | M4b 起记录什么 |
| ---- | ---- |
| 直连的 TCP 会话 | 拨通的那个地址（阶段 1 起就是这样）。 |
| 经代理的 TCP 会话 | 第一跳（代理服务器的地址）。 |
| 经代理、第一跳连上之后才失败的 TCP 会话 | 留着第一跳的地址：`dial_route` 在最后一次尝试失败之后不清会话的 `trail`（修正记录第 36、41 行）。 |
| `snell` 池里的连接 | 池里的连接带着拨它时的对端，借出时写进会话；取出的连接已失效、转发时换新连接重发的，写的是新连接的远端（重拨在会话的 `trail` 下跑）。 |
| `anytls` 池里的会话 | 同上，会话带着对端。 |
| `h2-connect` 与 HTTP/2 的 `trust-tunnel` 共用的连接 | 连接带着对端，借用的每个流写进自己的会话。 |
| `ssh` 的会话 | 会话带着对端，每条通道都写。 |
| QUIC 一族（`hysteria2`、`tuic` / `tuic-v5`、MASQUE 的 TCP、`trust-tunnel` 的 `h3=true`） | 借出时写那条连接此刻的远端：直连的是服务端的地址，跳端口的是取出那一刻的端口，经底层代理的是承载的远端（例如 socks5 的 UDP 中继）。**从不是替身 `192.0.2.1:443`**，也不用 quinn 的 `remote_address()`（它在跳端口时是第一个端口、经承载时是替身；设计第 18 节第 14 条、计划 P10 ①，做法见修正记录第 52、53 行）。 |
| `hysteria2`、`tuic` 的 UDP | 读此刻绑定的那条连接的远端；没有绑定时是 `null`（修正记录第 54 行）。 |
| `wireguard` | 覆盖这个目标的那个对端的载体的远端：直连是它的端点，经底层代理的是承载的远端（修正记录第 55 行）。 |
| UDP 流 | 载体只有一个远端的（`ss`、socks5 的中继、`trojan`、`snell`、`anytls`）是那个载体的远端；QUIC 一族与 `wireguard` 见上面两格。DIRECT 的是这条流第一个数据报发往的地址；`external` 是本机的中继 `127.0.0.1:<端口>`。 |
| 内部的 UDP 会话（DoQ、DoH3 的载体） | 同上：直连的是服务端的地址，经 socks5 的是中继（计划 P8 ④，修正记录第 36 行）。 |
| 没开成载体的 UDP 流与内部 UDP 会话 | `null`：两处 `clear_peer` 把载体的拨号所经的第一跳（socks5 的控制连接）清掉——那不是这条会话的远端（修正记录第 36 行）。 |

**两种失败不一样**：经代理、第一跳连上之后才失败的 TCP 会话留着第一跳的地址，没开成载体的 UDP 流与内部 UDP 会话是 `null`——数据没有走过任何一跳（协调者评审任务 5 时记下的，修正记录第 36、41 行）。

**照旧是 `null` 的**（登记，P8 ②）：`vmess`、`h2-connect` 与 MASQUE 的 **UDP**——它们按目标在第一次发送时才在另一个任务里开连接，那时会话的 `Trail` 不在；被拒绝的会话；rurge 不实现的协议。

记录读 `Trail` 的对端时不再分 TCP 与 UDP。用例：`crates/rurge-engine/src/observe.rs` 的 `a_udp_record_reads_the_trail_as_a_tcp_record_does`（原来是 `the_record_of_a_udp_session_has_no_remote_address`）、`tests/udp.rs` 的 `a_direct_flow_records_where_its_datagrams_go`、`tests/pipeline.rs` 的 `a_carrier_over_a_socks5_association_records_the_relay`、`tests/outbounds_snell.rs` 的 `a_pooled_snell_connection_gives_its_record_the_servers_address`、`a_pooled_snell_connection_through_a_socks5_proxy_records_the_first_hop` 与 `a_redialled_snell_session_records_the_new_connection`、`tests/outbounds_tls_family.rs` 的 `a_pooled_anytls_session_gives_its_record_the_servers_address`、`tests/outbounds_http2.rs` 的 `every_stream_on_a_shared_h2_connection_records_its_address`、`tests/outbounds_ssh.rs` 的 `every_channel_on_an_ssh_session_records_its_address`、`tests/outbounds_hysteria2.rs` 的 `every_stream_on_a_quic_connection_records_the_servers_address`、`a_hysteria2_udp_flow_records_the_server` 与 `hysteria2_through_a_socks5_relay_records_the_relay`、`tests/outbounds_tuic.rs` 的 `every_stream_on_a_tuic_connection_records_the_servers_address` 与 `a_tuic_udp_flow_records_the_server`、`tests/outbounds_masque.rs` 的 `every_stream_on_a_masque_connection_records_the_servers_address` 与 `a_masque_udp_flow_records_no_far_end`、`tests/outbounds_wireguard.rs` 的 `a_session_through_wireguard_records_its_peers_endpoint`、`a_udp_flow_through_wireguard_records_its_peers_endpoint` 与 `a_session_records_the_peer_its_destination_routes_to`，以及 `crates/rurge-net` 的 `quic/mod.rs` 的 `a_carried_path_far_end_is_its_carriers_not_the_stand_in`、`quic/hop.rs` 的 `the_current_port_follows_the_hops`。引擎与协议那几层断言的地址都是回环上夹具的地址；单元那一层（`Trail` 的用例）用文档地址当哨兵，不拨号。

### `dnsMs` 在解析途中

`dnsMs`（紧跟 `firstByteMs`）：规则与出站花在名字查找上的时间，两者合在一起；一次还没有结束的查找也计到读记录的此刻，所以进行中的会话的 `dnsMs` 随时间增长，结束之后是合计（设计第 10.4 节，M4-D24）。被打断的查找记它已经花的时间。没有做过查找（目标是地址、或代理自己解析名字）时是 `null`。规则与拨号里的查找在出站就绪之前，落在 `connectMs` 那一段里；出站就绪之前 `connectMs` 是 `null`、`dnsMs` 可以已经有数；出站就绪之后的查找也算进 `dnsMs`（例如 snell 的池连接失效、拨号返回之后才换新连接重发时，重拨在会话的 `trail` 下跑，设计第 18 节第 16 条、计划 P9 ②），所以 `dnsMs` 不一定不大于 `connectMs`。

`Trail` 的 `dns()` 含在途的查找；新的查找用 `Lookup::begin()`，它在开始时登记、被丢下时加上花的时间并注销。`trail::current()` 只在 `follow` 里有。用例：`crates/rurge-net/src/trail.rs` 的 `a_lookup_under_way_counts_up_to_now`、`current_is_the_trail_followed_now`、`a_lookup_begun_outside_a_follow_notes_nothing` 与 `a_lookup_ends_on_the_trail_it_began_on`，`crates/rurge-dns/src/resolver.rs` 的 `a_lookup_under_way_shows_on_the_trail_while_it_waits`，`crates/rurge-engine/tests/pipeline.rs` 的 `an_active_session_shows_the_time_its_lookup_has_taken_so_far`，与现有的 `a_lookup_cut_short_is_noted_for_the_time_it_took`（`crates/rurge-dns/src/resolver.rs`）。

### 量到的一条记录

量的办法（设计第 10.5 节，计划 P12，任务 8）：**调试构建**、`rurge-engine` 的**库内**用例、两条各单独跑一次；峰值工作集由操作系统读（Windows 上是起一个 `powershell` 读本进程的 `PeakWorkingSet64`，Linux 读 `/proc/<pid>/status` 的 `VmHWM`，与 `crates/rurge-http/src/capture/memory.rs` 的做法相同）。两条带 `#[ignore]`，缺省的 `cargo test` 不跑：

```text
cargo test -j 8 --workspace --lib -- observe::memory::<名字> --ignored --exact --nocapture
```

- `observe::memory::an_empty_request_log_is_the_baseline`：只建一份空的 `RequestLog::new(100_000)`，作基线。空表的 `VecDeque::with_capacity` 只分配、不写，没写过的页不进工作集。
- `observe::memory::a_full_request_log_of_http_requests`：容量 100 000 条，造 110 000 个像真实的 HTTP 请求的句柄（`dst` 连 `:443` 是 32 字节、`url` 87 ～ 92 字节、`sni` 23 字节、规则文字 42 字节、两项的策略链、一条 53 字节的备注，设了方法与状态、记了连接与第一个字节、两个正文的计数），照引擎的路 `mark_active` → `finish` → `record_finished`，装满之后再进 10 000 条、挤掉最早的；先读峰值、再打印 `records held`（`log.len()`，不复制记录）。

两次运行的数（`memory-task-8.log` 与协调者评审时的那一次——那一次没有日志文件，数记在修正记录第 67 行；**一台机器上的一次运行，不是 CI**）：

| 运行 | 空表的峰值 | 装满的峰值 | 多出的 = 装满 − 空表 | 一条 = 多出的 / 100 000 |
| ---- | --------- | --------- | -------------------- | ---------------------- |
| 任务 8 | 17,702,912 | 126,230,528（`records held: 100000`） | 108,527,616 | 1085.3 |
| 协调者评审时 | 17,649,664 | 125,829,120（`records held: 100000`） | 108,179,456 | 1081.8 |

由多出的直接算：1 000 条（`--request-log-size` 的缺省）约 1.03 MiB、10 000 条约 10.3 MiB、100 000 条约 103 MiB；两次运行到这个精度相同（协调者那一次为约 103.2 MiB）。这个数里含着环里那一格 `RequestRecord` 本身与它在堆上的字符串和向量（修正记录第 67 行）。**1088 字节一条不是量到的数**：计划 P12 让把一条的字节数取整到 8 的倍数，任务 8 的 1085.3 取整是 1088（修正记录第 62 行）；协调者那一次的 1081.8 向上取整也是 1088、取最近是 1080（第 67 行）。

**缺省（`--request-log-size` 1 000）不因此改**（M4-D25）：要不要改归项目所有者（设计第 17 节）。

### 没有覆盖的

- **M4b 的提交都在本地，CI 没有见过**；**没有手工验收**：`docs/acceptance/phase4-manual.md` 的 M4 一节，M4b 的项 2026-11-02 加在那里，一项也没有做过。
- **全部证据是回环上的用例**：服务端是测试自己的或假的对端（假 snell、假 anytls、假 hysteria2、假 tuic、假 MASQUE 与 `FakeWgPeer` 一类），没有真的代理服务端；读的是 rurge 自己写下的记录，没有经真实的浏览器。手工清单里的那几项就是为此。
- **照旧 `null` 的三种**（P8 ②）：MASQUE、`h2`（`h2-connect`）与 `vmess` 的 UDP——按目标在另一个任务里懒开，登记（设计第 10.3 节）。
- **HTTP/2 上 `HEAD` 的应答、rurge 自己的页数页的长度**：没有用例（P5 ③，登记，设计第 18 节第 15 条）。
- **三处 `bad_request` 没有用例**：第 5 步 `absolute_form` / `origin_form` 失败、`fit` 失败、重发时 `fit` 失败（修正记录第 18 行）。
- **源站提前作答、请求正文还在上传的那一种没有用例**（设计第 11 节第 11 条，登记）。
- **跳端口的 hysteria2 在引擎那一层没有用例**：只有 `rurge-net` 的 `the_current_port_follows_the_hops` 钉住单元那一层（修正记录第 60 行）。
- **WireGuard 的 `far_end_for` 在 `connect` 之后读、不与换载体在同一把锁里**：碰上正在换载体的会话可能记下前一个载体的远端；这仍是「记下时的那一跳」（M4-D23），不改（修正记录第 60 行）。
- **`external` 的 UDP 只断言到「回环上、不是目标」**：中继的端口是那个程序自己绑的，用例不知道（修正记录第 39 行）。
- **没有观察过 Surge 在这些地方的实际行为**，照设计与手册写：九句备注的文字、`receiveMs` 与两个正文的口径、`remoteAddr` 在新口径下的取值。

## M1 ～ M4 推送之后的 CI（2026-11-02）

**本节订正上面各节里“CI 没有见过”的说法**（规矩是历史的段落不逐处改、由后写的段落订正）。M1a ～ M4b 的提交推送了（`20efc16`），CI 运行 41402787066 在三个平台上编译了它们，fmt 与 clippy 都过，阶段 4 的用例在三个平台上都通过。那一次运行没有全绿：失败的是 macOS 上一条阶段 3 的 `rurge-tun` 用例，原因在用例的时序假设，修正与其后的运行见 `docs/status.md`「M1 ～ M4 推送之后的 CI」。

- 开头那一句（“这一份写的是本地提交的行为：……CI 没有见过。”）、各节开头“这一节写的是本地提交的行为：……”那一句，与各节「没有覆盖的」里说提交在本地、CI 没有见过的那一条，都不再成立：各节引的用例 CI 在三个平台上都跑过、没有失败（带 `#[cfg(unix)]` 的只在 Linux 与 macOS 上，见本节第二条；带 `#[ignore]` 的 CI 不跑，见第三条）。
- 「M2c」一节带 `#[cfg(unix)]` 的 `0600`（`restrict` 与 `mitm_ca_generate_keeps_the_secrets_private`）在 CI 的 Linux 与 macOS 上第一次被编译，那条用例在两个平台上通过。
- 照旧成立的：全部证据是回环上的用例；没有手工验收（`docs/acceptance/phase4-manual.md` 六十二项）；没有经真浏览器、没有对真实的站点试过，也没有把 CA 装进任何信任库。量抓包内存与记录大小的两份 `memory.rs` 里，Linux 与其余平台的两支在 CI 上只编译、没有运行：调用它们的只有带 `#[ignore]` 的用例。以上由测试一步的结论与失败摘要推出，逐条的结果在要登录才能读的任务日志里。

## M5a　QUIC 里的 SNI 与 `auto-quic-block`

M5a 让 QUIC 流在求规则之前有 SNI，并让 `[MITM]` 的 `auto-quic-block` 生效（设计第 4 ～ 6 节，计划 P3 ～ P10）。一条 UDP 流的第一个数据报是客户端的 QUIC Initial 时，rurge 从它与后面几个数据报里的 Initial 包读出 CRYPTO 帧、按偏移拼出 ClientHello（设计第 4 节）；拼齐之前持住这条流，至多 300 毫秒、16 个数据报（第 5 节）；读出的 SNI 在求规则之前写进会话，所以带 `extended-matching` 的域名规则从此按 SNI 命中 QUIC 流，请求记录里的 `sni` 也有了值。`GET /v1/rurge/http` 因此多一个 `quic`。这一节写 API、记录与日志里看得见的部分；读不成与凑不齐时的几件、以及没有做的事，见下面「没有覆盖的」。

**这一节写的是本地提交的行为：M5a 的提交在本地，CI 没有见过。**

### `GET /v1/rurge/http` 的 `quic`

应答在 `rewrite` 之后多一个对象，键的次序固定（设计第 5.4、7 节，计划 P11）：

```json
"quic":{"hello":0,"incomplete":0,"unreadable":0,"blocked":0}
```

一条认出是 QUIC、走到建句柄的流在下面三个里**恰好计一次**，另有第四个（设计第 5.4 节的表）：

- `hello`：ClientHello 拼齐了，而且 `rurge_http::tls::parse_client_hello` 读得出——**有没有 SNI 都算**（客户端按地址连的也算）。被 `auto-quic-block` 阻断的流都在这里。
- `incomplete`：300 毫秒到了，或持着的数据报到了 16 个，还没有拼齐。
- `unreadable`：读者放弃了，或拼齐了而 `parse_client_hello` 读不出。
- `blocked`：被 `auto-quic-block` 阻断的流（都是 `hello` 里的）。

关联在持住时结束的流一个也不计（见「等多久、持几个」）。四个数自进程启动起累计，**重载不清零**：计数器是 `crates/rurge-engine/src/serve.rs` 的 `QuicCounts`（四个 `AtomicU64`，与 `SniffCounts` 并排），在引擎里紧跟着 `sniff_counts`。用例 `the_quic_counts_go_out_under_their_names`（`crates/rurge-api/src/routes/rurge_http.rs` 的测试模块：四个键的名字与次序逐字节）与 `rurge_http_counts_start_at_zero`（`crates/rurge-api/tests/api.rs`：`rewrite` 之后紧跟这一串）。

### 记录：`sni` 与两种备注

**形状不变**（设计第 5.3、7 节）：`/v1/requests/recent` 与 `/v1/requests/active` 的键一个也没有增减。变的只有两处：

- **`sni`**：QUIC 流读到了 ClientHello 时是它连的名字（`parse_client_hello` 答的那个，小写）；没有 SNI、或没有读到时照旧是 `null`。
- **`notes`**：没有读到时多一句，一条流至多一句。

两种备注照 `crates/rurge-engine/src/udp.rs` 的原文逐字（`add_note`；第 5.4 节的表）：

```text
No SNI: the QUIC ClientHello did not arrive whole
No SNI: the QUIC ClientHello could not be read
```

第一句是 `incomplete` 那一类（300 毫秒到了、或 16 个数据报还没有拼齐），第二句是 `unreadable` 那一类（读者放弃了、或拼齐了而读不出）。**读成了的不写备注**（M5-D10）：没有 SNI 的 ClientHello 也读得成、也计进 `hello`，那时记录里只有 `sni: null`，没有备注。备注在建句柄之前就定了、建句柄时写；之后被规则拒绝的照样带着它。

### 被 `auto-quic-block` 阻断的流

被阻断的流的记录（设计第 6.2 节，M5-D12）：

- `status` 是 `rejected`、`rejectKind` 是 `REJECT`（结局 `Rejected(RejectKind::Reject)`）；
- `error` 是 `QUIC blocked by auto-quic-block`（常数 `AUTO_QUIC_BLOCKED`，`crates/rurge-engine/src/udp.rs`）；
- 规则文字与策略链照旧：阻断放在规则与 `REJECT` 之后，使用者看得到这条流命中了哪一行、本来要走哪条策略；
- `notes` 里没有这一句：说明在 `error` 里，与备注是两个字段。

这一句与 `block-quic` 的 `QUIC blocked` 不同，两者一读就分得开。虚拟网卡上每次丢掉的数据报各回一个 ICMP「管理上禁止」，SOCKS5 上只能丢（同 `block-quic`）。

**阻断的条件**（设计第 6.1 节，M5-D11）：下面三条都成立才阻断，实现在 `crates/rurge-engine/src/serve.rs` 的 `auto_quic_blocks`：

1. 这条流的 ClientHello 读到了（上面 `hello` 那一类）；
2. 这一代配置的 `[MITM] auto-quic-block` 为真（缺省真）；
3. **回落之后的 TCP 会被嗅探并解密**——同一个目标、同一个端口上的一条 TCP 会话，带着同一个 ClientHello 去掉 ALPN，会被 `sniffable_session` 嗅探、被 `Engine::decrypting` 解密。两步都是 TCP 一侧今天的那段代码（入口、名字、`always-raw-tcp-hosts`、`always-raw-tcp-keywords` 与端口那几步抽成 `sniffable_target`，两侧共用，条件不会分叉），所以名单、`mitm` 开关、可用的 CA、`client-source-address`、`always-raw-tcp-hosts` 与 `always-raw-tcp-keywords`、以及端口（80、443、`force-http-engine-hosts` 与 `[MITM]` 放宽到的）都对 QUIC 流同样算数。ALPN 不看：QUIC 的是 `h3`，回落到 TCP 之后客户端给的是 `h2` 与 `http/1.1`，那时由 TCP 一侧照常判断。

两处因此与手册不同（登记在兼容性清单的 `auto-quic-block` 一行）：名单里同时写了 `always-raw-tcp-hosts` 的主机**不阻断**（回落之后的 TCP 反正不解开，阻断只让客户端慢）；名单里**不带端口**的项只在这个端口本来就嗅探时算数——例如 HTTP/3 在 8443、名单写 `example.com` 而不带端口，不阻断。要阻断就写明 `:8443` 或 `:0`，两侧一起生效。

**没有 SNI 的 ClientHello**（设计第 6.4 节，M5-D14）：名单按目标比，不带端口的项只对 443——同 TCP 上没有 SNI 的 TLS。

**与 `block-quic` 的关系**（设计第 6.3 节，M5-D13）：两者各自独立，任一个要阻断就阻断；`auto-quic-block` 先判，两者都要阻断时 `error` 写它。全局的 `block-quic = always-allow` **不放过** `auto-quic-block` 要阻断的流（`always-allow` 说的是各策略的 `block-quic` 参数，手册没有说 `[MITM]` 的这个键）；不想阻断名单里主机的 QUIC，写 `auto-quic-block = false`。被 `auto-quic-block` 阻断的流不碰自适应的表（它每一次都阻断，不是试探）；`block-quic` 那一段本身没有改。

用例：`crates/rurge-engine/tests/mitm.rs`（经 SOCKS5）的 `auto_quic_block_blocks_quic_to_a_listed_host`、`auto_quic_block_off_lets_quic_through`、`with_mitm_off_quic_goes_through`、`without_a_usable_ca_quic_goes_through`、`a_source_mitm_does_not_take_lets_quic_through`、`a_negated_entry_lets_quic_through`、`a_raw_tcp_host_is_not_blocked`、`a_bare_entry_does_not_block_quic_on_8443`、`a_port_entry_blocks_only_its_port`、`an_incomplete_hello_is_not_blocked`、`a_hello_without_sni_is_judged_by_its_target`、`always_allow_does_not_stop_auto_quic_block`、`with_block_quic_too_the_record_names_auto_quic_block`、`a_reject_rule_comes_before_auto_quic_block`，与 `crates/rurge-engine/tests/tun.rs`（经内存设备上的虚拟网卡）的 `auto_quic_block_answers_each_held_datagram_with_icmp`。

### 等多久、持几个

两个常数在 `crates/rurge-engine/src/udp.rs`，**没有旋钮**（M5-D7）：

- `HELLO_WAIT`：300 毫秒，**从第一个数据报起**；
- `HELLO_DATAGRAMS`：16，第一个算在内。

拼齐了立刻放，所以 ClientHello 装得进第一个数据报的流一点也不多等；跨两个数据报的只多等两个数据报到达的间隔。300 毫秒与 16 个只管凑不齐的：300 毫秒同 sing-box 的缺省、在 Xray 的 200 毫秒与 mihomo 的 3 秒之间，远小于客户端 1 秒的 PTO；16 个装得下 16 KiB 的 ClientHello，也挡住一个只发别的包的客户端让持着的队列一直长（每个关联 1 MiB 的 `QUEUED_BYTES` 照旧管着全部）。

持着的时候**没有句柄、没有记录**，同 TCP 的嗅探；满了 16 个之后的与别的数据报照旧排在流的队列里，放了之后按原来的次序发出去（`forward` 先发持着的，再收队列）。关联在持住时结束的，这条流**不留记录、不计数**。

### 日志

只多两行 DEBUG，一条流至多一行（设计第 5.4 节；`destination` 是这条流的目标，`why` 是读者放弃的原因）：

```rust
tracing::debug!(host = %destination.host, port = destination.port, "udp: the QUIC ClientHello did not come whole within 300 ms or 16 datagrams; the rules see no SNI");
tracing::debug!(host = %destination.host, port = destination.port, why, "udp: the QUIC ClientHello could not be read; the rules see no SNI");
```

读不成的原因**只进 DEBUG**（`why`），不进备注、记录与 API。九种：

```text
the client closed the connection
a frame an Initial may not carry
a frame cut short
CRYPTO data beyond 64 KiB
CRYPTO data that changed when it came again
CRYPTO data in more than 32 pieces
the first handshake message is no ClientHello
a ClientHello longer than 64 KiB
a ClientHello that does not parse
```

前八种是 `rurge-net/src/quic/initial.rs` 里那几个常数的原文，最后一种是拼齐了而 `parse_client_hello` 读不出的（`udp.rs` 自己写的）。名字与地址只进 DEBUG：INFO 与以上的会话日志行不变。

### 没有覆盖的

- **M5a 的提交都在本地，CI 没有见过**；**没有手工验收**：`docs/acceptance/phase4-manual.md` 的 M5 一节（W13、W14、M13、M14、L13、L14）2026-11-02 加在那里，一项也没有做过。
- **全部证据是回环上的用例**：QUIC 的数据报是用例自己造的或 quinn 的客户端在回环上真发出的，没有一个 QUIC 握手完成过；没有经真浏览器、没有对真实的 HTTP/3 站点试过。
- **设计第 4.7 节的四件没有做**（M5-D15）：从没有拼齐的 ClientHello 里取 SNI（Surge iOS 5.17.0 说有）、gQUIC 的 SNI（Surge iOS 5.14.6 说有）、Retry 之后的第二把密钥（持住时还没有到服务端，放出去之后不再读）、ECH（读到的是外层的 SNI）。前两件要不要做归项目所有者（设计第 15 节）。
- **放行的 QUIC 流发往没有人收的回环端口**，用例因此**不断言说明为空**（计划 P12、设计第 16 节第 2 条）：Windows 上可能写上一个「连接被重置」一类的说明。被阻断的流断言的是 `error` 恰好那一句。
- **没有观察过 Surge 在这些地方的实际行为**，照设计与手册写：`quic` 四个数的口径、两种备注与那一句说明的文字、持住的两个上限、阻断跟着 TCP 一侧的读法。
