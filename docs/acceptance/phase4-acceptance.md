# 阶段 4 需求逐句对照代码

## 怎么读

### 六列（M6 设计第 4.2 节）

| 列 | 内容 |
| --- | --- |
| 需求 | 需求编号；M6 设计第 3.2、3.4 节的两张小表写 `总设计第 1.2 节（FR-…）` / `总设计第 1.5 节（FR-…）`，括号里是这一句算在哪条需求下（M6 设计第 11 节），M6 设计第 3.3 节的表写原来的编号与出处 |
| 这一句 | 同阶段 1 |
| 结论 | M6 设计第 4.4 节的八种之一 |
| 代码 | 真正**作用于**这件事的地方，`路径::函数`；只有解析器接受它而没有东西用它的，不算"做了" |
| 用例 | 路径与用例名；读过它的断言，断言的是这一句说的行为，不是"跑通了"；只在某个平台上编译的，写明平台 |
| 真机上 | M6 设计第 4.3 节 |

### "真机上"一列（M6-D3）

阶段 4 的多数句子，回环上的用例证明了逻辑：测试自己的明文与 TLS 服务器、rustls 写的测试客户端、只信测试自己生成的 CA（总设计 P4-D26）。真浏览器与真站点的握手与 ALPN、真源站的证书链、系统与浏览器的信任库、证书固定的真客户端，要在真机上看（手工清单开头那一段）。这一格写：

- 手工清单的编号，写成 `节/编号`（如 `M2/W1`、`M4/P13`）：清单的 M1 与 M2 两节都有 W1、W2、M1、M2、L1、L2、P1 ～ P3，光写编号分不出是哪一节（M6-D9）；一个平台一个或几个；
- `—`：L1 的用例已经证明了这一句的全部，不需要真机（例如解析、诊断、纯函数、只在回环上就有意义的行为）；
- `看不出`：要真机才证得了，但在真机上也没有一项手工操作看得出它；括号里一句话说为什么；
- `缺`：要真机而手工清单里没有对应项、又不是"看不出"的，任务 6b 补项（M6 设计第 7 节）。

阶段 4 没有 L2 特权用例（总设计第 14 节没有设），这一格不写 L2。

### 八种结论（M6-D4）

阶段 1 的七种照用（把"不属于阶段 1"换成"不属于阶段 4"），定义照 `phase1-acceptance.md`「怎么读」，抄在这里，并把阶段 3 / M6 执行期的两条订正（阶段 3 / M6 设计第 15 节）写进去：

- `做了`：代码在，也有一条会因这一句不成立而红的用例。
- `做了，补了用例`：代码在，读的时候没有这样的用例（把代码改坏一行，所有现有用例照样通过）；补了，并且在改坏的代码上看到它红。用例格后面括号里是补它的提交。
- `补上了`：这一句没做，或只做了一部分，读的时候当场补上；先红后绿，一处一个提交。
- `没有做完（已登记）`：要先有设计才能做，没有做；缺什么、使用者会看到什么写在代码列，登记进「没有做完的」。
- `有意不同（已登记）`：rurge 有意做成了别的样子，代码列写差异登记在哪（兼容性清单的哪一节或哪份设计），用例列写 rurge 现在做的是什么。
- `不属于阶段 4`：结论后面括号里用一句话说为什么——这一句自己的话写明了后面的阶段；或它所在的需求行的阶段栏列了几个阶段（或写着"各阶段""逐阶段补齐"），而登记把这一句放在后面的那一部分；或这一句说的是项目怎么做事，不是程序做什么。没有什么要项目所有者定的。
- `需求文字有问题`：结论后面括号里用一句话说为什么——需求行的阶段栏给这一句的是阶段 4，这一句自己没有说别的，而兼容性清单或某份设计把它登记在后面的阶段；或这一句的字同手册、同另一条需求（或同一条里的另一句）不符。这些由项目所有者定，进「需求文字的问题」一节。

**两者的分界**（阶段 3 / M6 订正第 1、2 条）：阶段栏只写 4 的需求行上，一句话自己没有说别的阶段，而登记把它放在后面的——例如 FR-HTTP-09 的"→ 脚本"、FR-HTTP-07 的"先于脚本"（脚本在阶段 5，FR-SCR），FR-HTTP-11 的"通过 … Dashboard 查看"（Dashboard 在阶段 6）——记 `需求文字有问题`，不记 `不属于阶段 4`。只有这一句自己写明了后面的阶段、或需求行的阶段栏列了几个阶段（FR-HTTP-10 的"1 基础 / 4 完整"、FR-CFG-06、FR-CFG-18、FR-OBS-02）而登记把这一句放在后面的（例如 FR-HTTP-10 来源里的"设备"——网关模式在阶段 7，阶段 3 的对照已经这样记），才记 `不属于阶段 4`。照阶段 1 的话，这两种之间有一处是判断、不是规则：判不了的，看登记；同一类的句子由协调者在评审时统一。

另加一种：

- `做了，本机没有能红的用例`：代码在，作用于这一句，但没有一条缺省的用例会因这一句不成立而红，在这台开发机上也补不出来。有两种：它只在 Linux 或 macOS 上编译（`cfg(target_os = "linux")`、`cfg(target_os = "macos")`、`cfg(unix)`），本机编译不到；或它在本机编译，但缺省的用例照规矩从不走到它——阶段 4 里可能的是读系统信任库的那一层（解密的上游信"系统的根"，测试的上游只信测试的 CA）与只有真浏览器、真站点才走到的路径。代码一格写明那段代码与原因，用例一格写"—"或断言不到这一句的那几条用例；这些句子另列一节「本机没有能红的用例的」，给以后能在那个平台上跑的助手（或所有者）补。

**逻辑钉住了、系统的效果没有钉住的，记 `做了`**（阶段 1 与阶段 3 的先例）：L1 的用例把作用于这一句的逻辑一直钉到系统的边界，改坏那段逻辑会有用例红，就记 `做了`；例如 `rurge mitm ca generate` 打印的装 CA 的命令，有用例逐字断言打印的文字的，记 `做了`，那些命令在真机上照做有没有效，是"真机上"一格的事。只有作用于这一句的代码整段都没有缺省用例走到时，才用 `做了，本机没有能红的用例`。

已经有一条只在 Linux / macOS 上编译、会因这一句而红的用例的，记 `做了`，用例一格写"只在 Linux 与 macOS 上编译"与下面一段的 CI 证据。

**用例一格里 CI 的证据**（M6-D28）：一条用例在某个平台的 CI 上"通过"，只凭那个平台的 `check` 任务全绿的一次运行（测试一步成功，每条用例都过了）；不全绿的任务里"失败摘要没有点名它"不写成"通过"——公开能读到的只有各步的结论与失败的注解（`.github/scripts/report-test-failures.py` 只列失败的），那样的"通过"是推出来的。两次运行可以拼起来、各取全绿的平台，条件是两个提交之间这条用例编译与运行的东西都没有改：`20efc16` 与 `a262d4c` 之间只改了 `crates/rurge-tun/src/tests/dispatch.rs`（`rurge-tun` 的测试模块，不进别的 crate 的构建），所以：

- `20efc16` 上就有的用例（`git grep` 那个提交）写"CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901）"；
- 其中只在 Unix 上编译的写"只在 Linux 与 macOS 上编译；CI：Linux 运行 41402787066、macOS 运行 41411445901 通过"；
- `20efc16` 上没有的写"CI 没有见过"；带 `#[ignore]` 的写"CI 编译、不运行"。

这说的是那条用例在 `a262d4c` 的代码上通过过；`a262d4c` 之后改过的代码（M4b 任务 11、M5、M6）CI 没有见过，任务 8 读回那次运行之后改写（M6 设计第 6 节）。

（其余由任务 7 写）

## 结论

（任务 7 写）

## 第一部分：需求逐句对照代码

### 会话入口与 HTTP 引擎（FR-HTTP-01、02）

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| FR-HTTP-01 | 「HTTP 引擎：HTTP/1.1 请求 / 响应解析与转发」 | 做了 | `crates/rurge-http/src/engine.rs::serve_connection`、`crates/rurge-http/src/engine.rs::serve`、`crates/rurge-http/src/engine.rs::Requests::handle` | `crates/rurge-http/src/engine.rs`：`a_request_goes_through_and_its_handle_finishes`（一个请求走完、句柄完成）；`crates/rurge-engine/tests/http.rs`：`a_sniffed_http_connection_has_a_record_per_request`（嗅探到的连接上两个请求各一条记录）、`every_entry_brings_its_requests_into_the_engine`（四种入口）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M1/W1、M1/W2、M1/M1、M1/M2、M1/L1、M1/L2 |
| FR-HTTP-01 | 「keep-alive」 | 做了 | `crates/rurge-http/src/engine.rs::serve`（借池里的连接）、`crates/rurge-http/src/engine.rs::complete`（还回池）、`crates/rurge-http/src/pool.rs::Pool` | `crates/rurge-http/src/engine.rs`：`two_requests_on_one_connection_share_an_upstream_connection`（第二条请求借同一条上游连接）；`crates/rurge-engine/tests/http.rs`：`a_sniffed_http_connection_has_a_record_per_request`（同上，两条各一条记录）、`a_body_after_a_304_does_not_poison_the_next_request`（重对报告第 3 条：上游在 304 后多送字节，第二个请求仍拿到 200；rurge 的下一个请求不借那条连接：没有正文的应答要等下一个请求来了才把上游连接放回池里，计划延后第 12 行）（`cc8216b`）；前两条 CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901），`a_body_after_a_304_does_not_poison_the_next_request`：CI 没有见过 | M1/P2、M1/W1、M1/M1、M1/L1 |
| FR-HTTP-01 | 「chunked」 | 做了 | `crates/rurge-http/src/engine.rs::serve`（正文流式；长度没人说的正文补 chunked 框架） | `crates/rurge-engine/tests/http.rs`：`chunked_bodies_go_both_ways`（chunked 请求与应答的字节两个方向原样）、`a_pipelined_request_after_a_chunked_body_is_its_own_request`（重对报告第 9 条：chunked 正文之后紧跟的 `GET` 是自己的请求，正文整块、两条记录）（`cc8216b`）；`crates/rurge-http/src/engine.rs`：`a_chunked_answer_counts_every_frame`（chunked 应答逐帧计数）；前两条 CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901），补的那一条：CI 没有见过 | — |
| FR-HTTP-01 | 「WebSocket 升级透传」 | 做了 | `crates/rurge-http/src/engine.rs::serve`、`crates/rurge-http/src/engine.rs::wants_upgrade`、`crates/rurge-http/src/engine.rs::promote` | `crates/rurge-http/src/engine.rs`：`a_websocket_upgrade_is_relayed_and_counted_once`（升级之后两个方向的字节原样、只计一次）、`a_websocket_upgrade_goes_through_tls_and_is_counted_once`（解密连接上的升级）；`crates/rurge-engine/tests/http.rs`：`a_websocket_goes_through_every_entry`（四种入口）、`stopping_the_adapter_ends_an_upgraded_relay`（适配器停止结束升级后的转发）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| FR-HTTP-01 | 「80 / 443 协议嗅探」 | 做了 | `crates/rurge-engine/src/serve.rs::sniffable_session`、`crates/rurge-engine/src/serve.rs::sniffable_target` | `crates/rurge-engine/tests/sniff.rs`：`which_sessions_are_sniffable`（80 / 443 与别的端口、三个名单、fake 地址）、`a_paused_engine_sniffs_nothing`、`a_session_that_pre_matching_refuses_is_not_sniffed`；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M1/P1、M1/P3、M1/W1、M1/M1、M1/L1 |
| FR-HTTP-01 | 「`force-http-engine-hosts`」 | 做了 | `crates/rurge-engine/src/serve.rs::sniffable_target`（`force_http_engine_hosts.matches`，缺省端口 80）、`crates/rurge-engine/src/serve.rs::forces_http_engine` | `crates/rurge-engine/tests/sniff.rs`：`force_http_engine_hosts_sniff_another_port`（8080 命中名单才嗅探）；`crates/rurge-engine/tests/http.rs`：`force_http_engine_hosts_bring_another_port_into_the_engine`（命中后进引擎）；`crates/rurge-config/src/hostlist.rs`：`force_http_engine_hosts_examples`（带端口、`*`、`-` 排除、`:0`）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M1/P3 |
| FR-HTTP-01 | 「`always-raw-tcp-hosts`」 | 做了 | `crates/rurge-engine/src/serve.rs::raw_listed`（`always_raw_tcp_hosts.matches`）、`crates/rurge-engine/src/serve.rs::sniffable_target` | `crates/rurge-engine/tests/sniff.rs`：`always_raw_tcp_hosts_keep_the_dial_first`（命中名单照今天的次序：先拨号，拒绝在答复之前）、`always_raw_tcp_hosts_and_keywords_are_noted`（记录里的原因）；`crates/rurge-config/src/hostlist.rs`：`force_http_engine_hosts_examples`（`-` 排除与端口）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M1/P3 |
| FR-HTTP-01 | 「`always-raw-tcp-keywords`」 | 做了 | `crates/rurge-engine/src/serve.rs::raw_listed`（关键字只比名字、不分大小写） | `crates/rurge-engine/tests/sniff.rs`：`always_raw_tcp_keywords_ignore_case`（大小写不同照样命中）、`always_raw_tcp_hosts_and_keywords_are_noted`；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M1/P3 |
| FR-HTTP-02 | 「HTTP/2：MITM 后与客户端…使用 HTTP/2（`h2=true`）」 | 做了 | `crates/rurge-http/src/mitm/decrypt.rs::server_config`（`h2` 为真时服务端的 ALPN 是 `[h2, http/1.1]`，否则 `[http/1.1]`）、`crates/rurge-http/src/mitm/decrypt.rs::Mitm::decides`（`h2` 为真时只给 `h2` 的客户端也解密）、`crates/rurge-engine/src/serve.rs::serve_sniffed`（把 `accepted.h2` 交给连接）、`crates/rurge-http/src/engine.rs::serve_connection` 与 `crates/rurge-http/src/engine.rs::serve_h2`（`conn.h2` 为真时用 hyper 的 HTTP/2 服务端，并发上限是 rurge 的 100） | `crates/rurge-http/src/mitm/decrypt.rs`：`with_h2_an_h2_only_client_is_decrypted`（`h2` 开着时只给 `h2` 的与两种都给的客户端都解密，两个都不给的照原样转发）、`a_client_is_served_http1_or_nothing`（`h2` 关着时服务端只给 `http/1.1`：什么都不给的客户端不协商，两种都给的拿到 `http/1.1`）；`crates/rurge-engine/tests/mitm.rs`：`four_combinations_of_http1_and_h2_are_decrypted`（客户端与上游四种组合都解密，客户端一侧给 `h2` 时握手落在 `h2`）、`an_h2_only_client_is_decrypted_when_h2_is_on`、`a_client_offering_only_h2_is_passed_through`（`h2` 关着时）、`each_h2_stream_follows_its_own_rule`；`crates/rurge-http/src/engine.rs`：`the_h2_server_offers_100_streams_and_no_extended_connect`（`max_concurrent_send_streams` 是 100、不宣告扩展 `CONNECT`）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901）。重对报告第 4 条（MITM 的 HTTP/2 连接上客户端读得慢时内存不涨）没有用例：照读到的，hyper 1.11.1 把应答正文交给 h2 之前先等这个流的发送容量（`hyper-1.11.1/src/proto/h2/mod.rs` 的 `PipeToSendStream`，手里至多压一块），h2 每个流的发送缓冲至多约 400 KiB（`hyper-1.11.1/src/proto/h2/server.rs` 第 39 行），应答正文是流式转发的，客户端不给窗口时 rurge 就不再读上游——上游是 HTTP/2 时停在 rurge 给它的接收窗口以内（每个流 2 MiB、连接 5 MiB，`hyper-1.11.1/src/proto/h2/client.rs` 第 48、49 行），是 HTTP/1.1 时停在 hyper 的读缓冲与套接字的缓冲以内；命中 `[Body Rewrite]` 的应答另先攒至多 10 MiB（M3b），抓包开着时另存进抓包的存储（单条正文至多 5 MiB、总量 100 MiB，M4a）；一直不读、到了空闲时限的，这个请求失败、上游被放掉（`an_h2_client_that_stops_reading_one_stream_fails_only_that_request`）。读得慢的客户端让 rurge 在途攒多少没有量过（延后第 13 行）。重对报告第 6 条（格式不对的 HTTP/2 请求只重置它自己的流）没有用例：客户端一侧的 HTTP/2 由 h2 0.4.19 讲，带连接专用头的请求（`h2-0.4.19/src/codec/framed_read.rs` 第 173 ～ 177 行）与缺伪头的请求（`h2-0.4.19/src/server.rs` 的 `convert_poll_message`）都只以 `PROTOCOL_ERROR` 重置那一个流、不发 GOAWAY，与 Surge 修过之后一样；rurge 在这一层没有自己的代码，h2 与 hyper 的客户端都不肯发这种请求 | M2/W3、M2/M3、M2/L3 |
| FR-HTTP-02 | 「…/ 上游使用 HTTP/2…」 | 做了 | `crates/rurge-http/src/mitm/decrypt.rs::Mitm::build`（对上游的 TLS 同时给 `h2` 与 `http/1.1`，`h2` 为假时只给 `http/1.1`）、`crates/rurge-http/src/engine.rs::start_h2`（hyper 的 HTTP/2 客户端）、`crates/rurge-http/src/engine.rs::Shared::borrow_h2` 与 `crates/rurge-http/src/engine.rs::Shared::share`（一个键在一条客户端连接里至多一条，克隆着共享）、`crates/rurge-http/src/version.rs::fit`（两侧版本之间转换） | `crates/rurge-engine/tests/mitm.rs`：`four_combinations_of_http1_and_h2_are_decrypted`（上游一侧给 `h2` 时源站收到 `HTTP/2.0`）、`concurrent_h2_streams_share_one_upstream_connection`（六个请求一条上游连接）；`crates/rurge-http/src/engine.rs`：`an_h2_origin_is_spoken_to_in_h2`（源站记到 `HTTP/2.0`）、`requests_of_one_key_share_one_h2_connection`、`another_key_gets_another_h2_connection`、`an_h2_connection_that_went_away_is_not_lent_again`、`the_bytes_of_requests_on_a_shared_h2_connection_are_their_bodies`（共享连接上每条记录只算正文）、`an_upgrade_dials_http1_where_the_origin_speaks_h2`（升级拨 `http/1.1`）、`an_upstream_h2_stream_reset_mid_body_reaches_the_client_as_internal_error`（上游中途重置只截断这个流、连接继续）、`an_upstream_h2_connection_that_ends_mid_body_truncates_the_client`（重对报告第 7 条：上游连接在应答中途关掉，客户端正文出错、记录是 `Failed`）（`5a99dc9`）；前八条 CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901），补的那一条：CI 没有见过 | M2/W3、M2/M3、M2/L3 |
| FR-HTTP-02 | 「`:authority` 提取」 | 做了 | `crates/rurge-http/src/engine.rs::form_of`（HTTP/2 请求以 URI 的 authority 拼 URL、`http_host` 取它）、`crates/rurge-http/src/engine.rs::prepare`（同一处交给会话）、`crates/rurge-http/src/version.rs::Form::of` 与 `crates/rurge-http/src/version.rs::fit`（转到 HTTP/1.1 时把 `:authority` 写进 `Host`，转到 HTTP/2 时把 authority 写进 `https://` URI）、`crates/rurge-rules/src/matcher.rs::domain_targets`（`extended-matching` 时 `http_host` 是域名目标之一） | `crates/rurge-http/src/engine.rs`：`the_authority_of_an_h2_request_names_it_and_becomes_host`（`:authority` 进 `url` 与会话；上游是 HTTP/1.1 时写进 `Host`）、`an_h2_request_keeps_its_authority_and_te_trailers_to_an_h2_origin`（转到 h2 时仍是那个 authority、`TE: trailers` 留住）；`crates/rurge-http/src/version.rs`：`a_form_takes_the_uri_then_host_then_the_target`、`an_h2_request_is_put_into_http1_form`、`an_http1_request_is_put_into_h2_form`；`crates/rurge-engine/tests/mitm.rs`：`each_h2_stream_follows_its_own_rule`（规则按 `:authority` 取到的主机名各选一条）、`the_cookies_of_an_h2_client_reach_an_http1_origin_as_one_field`；`crates/rurge-rules/src/matcher.rs`：`extended_matching_uses_sni_and_host`、`url_regex_with_extended_matching_rewrites_host`（`URL-REGEX` 带 `extended-matching` 时也拿主机换成 `http_host` 的 URL 去试）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M2/W3、M2/M3、M2/L3 |

### MITM（FR-HTTP-03、04、14）

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| FR-HTTP-03 | 「`[MITM]` 九个键」之 `hostname` | 做了 | `crates/rurge-config/src/mitm.rs::parse_mitm`（`hostname` 与 `hostname-disabled` 都由 `crates/rurge-config/src/general.rs::parse_host_list` 读成 Host List、缺省端口 443）、`crates/rurge-http/src/mitm/decrypt.rs::Mitm::decides`（有 SNI 时按 `crates/rurge-config/src/hostlist.rs::HostList::matches_sni` 比，没有 SNI 时按 `matches` 比连接的目标） | `crates/rurge-config/src/mitm.rs`：`the_nine_keys_are_read_and_default`（`hostname = *.example.com, example.com:8443` 读成两项、缺省端口是 443）、`hostname_disabled_removes_entries_written_the_same`（大小写与 `-` 不计、端口不同的不去掉）；`crates/rurge-config/src/hostlist.rs`：`mitm_hostname_example_and_special_tokens`（手册那一份 `[MITM] hostname` 名单：三项排除、`*`、443 命中 8443 不命中）、`matches_sni_ignores_the_port_of_a_bare_entry`（带端口与 `:0` 的两支）；`crates/rurge-http/src/mitm/decrypt.rs`：`decides_lists_first_then_source_name_and_alpn`（命中的名字 `Decrypt`、排除项 `NotListed`）；`crates/rurge-engine/tests/mitm.rs`：`a_listed_host_is_decrypted_through_socks5`（SOCKS5 上命中的名字被解密）、`exclusions_disabled_entries_and_sources_pass_through`（排除项与停用项按原样转发）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M2/W2、M2/M2、M2/L2、M2/P3、M2/P4 |
| FR-HTTP-03 | 「`[MITM]` 九个键」之 `hostname-disabled` | 做了 | `crates/rurge-config/src/mitm.rs::parse_mitm`（`hostname-disabled` 也是 Host List；`hostname` 里与它文字相同的项被去掉，`crates/rurge-config/src/hostlist.rs::HostList::without`） | `crates/rurge-config/src/mitm.rs`：`hostname_disabled_removes_entries_written_the_same`（`*.Example.com` 去掉 `*.example.com`、`b.com:443` 去掉 `example.com:443`、`hostname` 没有的项不告警）；`crates/rurge-engine/tests/mitm.rs`：`exclusions_disabled_entries_and_sources_pass_through`（被停用的项不解密、客户端看到源站自己的证书）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M2/P3 |
| FR-HTTP-03 | 「`[MITM]` 九个键」之 `ca-p12` | 做了 | `crates/rurge-config/src/mitm.rs::parse_mitm`（`ca-p12` 按原样留下）、`crates/rurge-http/src/mitm/authority.rs::Authority::load`（Base64 与 PKCS#12 解码，`crates/rurge-proto/src/keystore.rs::decode_pkcs12`）、`crates/rurge-http/src/mitm/decrypt.rs::Mitm::build` | `crates/rurge-http/src/mitm/authority.rs`：`good_cas_of_every_supported_key_load`（P-256、P-384、RSA 2048、Ed25519 四种 CA 都载得进）、`every_ca_problem_is_named_and_quotes_no_secret`（Base64 坏、口令错、没有私钥、不是 CA 等都各一条 `W0039`，消息里没有材料与口令）；`crates/rurge-engine/tests/mitm.rs`：`a_listed_host_is_decrypted_through_socks5`（这份 CA 签出叶证书并完成握手）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M2/W1、M2/M1、M2/L1、M2/P6 |
| FR-HTTP-03 | 「`[MITM]` 九个键」之 `ca-passphrase` | 做了 | `crates/rurge-config/src/mitm.rs::parse_mitm`（写了空串也留下）、`crates/rurge-http/src/mitm/authority.rs::Authority::load`（不写按空口令） | `crates/rurge-config/src/mitm.rs`：`an_empty_passphrase_is_kept`（`ca-passphrase =` 留下空串）、`the_debug_shows_neither_the_p12_nor_the_passphrase`（`Debug` 只写有没有）；`crates/rurge-config/tests/secrets.rs`：`the_debug_of_a_loaded_profile_holds_no_secret`（整份 `Config` 的 `Debug` 里没有 `[MITM]` 的两个标记）；`crates/rurge-http/src/mitm/authority.rs`：`an_empty_passphrase_opens_a_p12_written_with_one`（空口令的 p12 打得开）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M2/W1、M2/M1、M2/L1、M2/P6 |
| FR-HTTP-03 | 「`[MITM]` 九个键」之 `ca-keystore-name` | 做了 | `crates/rurge-config/src/mitm.rs::parse_mitm`（键留下）、`crates/rurge-http/src/mitm/authority.rs::Authority::load`（优先于 `ca-p12`、不回落；名字不在 `[Keystore]`、或那一项不是 p12 都各是 `W0039`） | `crates/rurge-http/src/mitm/authority.rs`：`a_keystore_item_wins_and_does_not_fall_back`（条目优先，即使 `ca-p12` 根本不是 p12；名字不在时不回落）、`a_keystore_item_that_is_not_pkcs12_is_a_problem`（`type=openssh-private-key` 的项报「不是 PKCS#12 项」）、`a_keystore_item_with_bad_base64_is_left_to_e0021`（Base64 坏只报 `E0021`、不重复 `W0039`）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| FR-HTTP-03 | 「`[MITM]` 九个键」之 `skip-server-cert-verify` | 做了 | `crates/rurge-config/src/mitm.rs::parse_mitm`（键读成布尔）、`crates/rurge-http/src/mitm/decrypt.rs::Mitm::build` 与 `crates/rurge-proto/src/transport/tls.rs::origin_client_config`（为真时走 `Insecure`、只放过链与名字的校验）。为假时对上游信的根是 `crates/rurge-net/src/tls.rs::union_root_store`（系统的根 ∪ `webpki-roots`），由 `crates/rurge-engine/src/runtime.rs::build_mitm` 在 `EngineShared.roots` 为 `None` 时读，缺省的用例走过这一支（`an_untrusted_origin_is_a_502_and_skip_server_cert_verify_lets_it_through` 不给 roots）。`webpki-roots` 那一半由 `the_union_holds_every_webpki_root_and_is_shared` 钉住；系统的根那一半读的是本机的信任库，是系统的效果，没有断言；`build_mitm` 交给上游一侧的正是这一份并集，也没有用例单独钉住（那条 502 的用例断言的是源站的 CA 不被信，不是信的是哪些根）。作用于这一句的逻辑（为真时不校验、为假时校验）有会红的用例，照设计第 4.4 节「逻辑钉住了、系统的效果没有钉住的，记 `做了`」记 `做了`（协调者的裁定，修正记录第 19 行） | `crates/rurge-proto/src/transport/tls.rs`：`origin_client_config_verifies_unless_insecure`（不信另一个 CA 的根时失败、不校验时通过）；`crates/rurge-engine/tests/mitm.rs`：`an_untrusted_origin_is_a_502_and_skip_server_cert_verify_lets_it_through`（缺省时 502、原因带 `UnknownIssuer`；为真时 200、正文到达）；`crates/rurge-net/src/tls.rs`：`the_union_holds_every_webpki_root_and_is_shared`（`webpki-roots` 的每一个根都在并集里，并集只读一次；系统的根那一半没有断言）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M2/P2 |
| FR-HTTP-03 | 「`[MITM]` 九个键」之 `h2` | 做了 | 同「会话入口与 HTTP 引擎（FR-HTTP-01、02）」的「「HTTP/2：MITM 后与客户端…使用 HTTP/2（`h2=true`）」」一行 | 同上 | M2/W3、M2/M3、M2/L3 |
| FR-HTTP-03 | 「`[MITM]` 九个键」之 `client-source-address` | 做了 | `crates/rurge-config/src/mitm.rs::SourceList::parse`（逗号分隔、IP / CIDR / `-` 排除 / MAC；MAC 项在阶段 7 前不命中、整键一条 `W0029`）、`crates/rurge-http/src/mitm/decrypt.rs::source_of`（回环与虚拟网卡的来源按 `127.0.0.1` 比）、`crates/rurge-http/src/mitm/decrypt.rs::Mitm::blocked`（一项也不命中即不允许） | `crates/rurge-config/src/mitm.rs`：`the_source_list_reads_ips_cidrs_exclusions_and_macs`（IP、CIDR、`-` 排除、IPv4 映射、MAC 都读进来；MAC 只告警一次）、`an_empty_source_list_is_as_if_unwritten`（空值按没写算、全非法则谁也不允许）；`crates/rurge-http/src/mitm/decrypt.rs`：`decides_lists_first_then_source_name_and_alpn`（名单不取时 `SourceBlocked`）、`source_of_maps_loopback_and_the_virtual_interface_to_127_0_0_1`；`crates/rurge-engine/tests/mitm.rs`：`the_local_machine_is_compared_as_127_0_0_1`（`127.0.0.1` 解、`::1` 不解、SOCKS5 与虚拟网卡都按 `127.0.0.1`）、`exclusions_disabled_entries_and_sources_pass_through`；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M2/P3、M2/P5 |
| FR-HTTP-03 | 「`[MITM]` 九个键」之 `auto-quic-block` | 待读（任务 5） | 同「MITM（FR-HTTP-03、04、14）」的「「命中 MITM 列表时阻断 UDP 使客户端回落」」一行 | 同上 | |
| FR-HTTP-03 | 「按主机名签发…叶子证书」 | 做了 | `crates/rurge-http/src/mitm/authority.rs::Authority::issue`（每张各自一把新的 P-256；名字超过 64 个字符不写 CN；SAN 按域名或 IP；EKU serverAuth、KU digitalSignature；AKI 只在 CA 有 SKI 时写；序列号 16 个随机字节、最高位清零；有效期照 CA 收窄）、`crates/rurge-http/src/mitm/authority.rs::Authority::leaf`、`crates/rurge-http/src/mitm/decrypt.rs::Mitm::handshake`（把这张证书装进每条连接的 `cert_resolver`） | `crates/rurge-http/src/mitm/authority.rs`：`a_leaf_says_what_the_design_says`（SAN、CN、EKU、KU、AKI 等于 CA 的 SKI、叶没有 SKI 与 BasicConstraints、序列号编码后至多 16 字节且首字节最高位为 0；四种 CA 的签名算法各对；**2026-11-03 改了序列号那一条断言，改了两次**：原来断 `len() == 16`，而 DER 丢掉序列号开头每一个为 0 的字节，编码成 15 字节（约 1/256）或更短（约 1/65536）时偶发红；`fc972b8` 改成「15 或 16 字节」，更短的那一种照样红；`c5f33e8` 改成「至多 16 字节」（最高位清零时编码之后不会长过 16 字节，不清零时约一半是 17 字节）；**这一条改过，CI 没有见过**）、`a_leaf_lives_47_days_within_its_ca`（47 天、不越过 CA 的两端）、`a_ca_without_a_subject_key_identifier_gives_leaves_without_an_authority_key_identifier`（CA 没有 SKI 时不写 AKI）；`crates/rurge-http/src/mitm/decrypt.rs`：`without_sni_the_leaf_names_the_target`（没有 SNI 时按目标签、IP 是 IP 的 SAN）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M2/W2、M2/M2、M2/L2、M2/P5 |
| FR-HTTP-03 | 「…并缓存叶子证书」 | 做了 | `crates/rurge-http/src/mitm/authority.rs::Authority::leaf`（每个名字一个槽、同时到的等同一张；LRU 1000 个槽；剩余寿命不足三分之一就换槽重签）、`crates/rurge-engine/src/runtime.rs::build_mitm`（材料相同的重载沿用同一个 `Authority`，叶证书缓存跟着走） | `crates/rurge-http/src/mitm/authority.rs`：`one_name_one_leaf_until_a_third_of_its_life_is_left`（同一名字同一张、签一次；剩三分之一时换）、`the_thousand_and_first_name_pushes_out_the_oldest`（第 1001 个名字把最早的挤掉、重签）、`concurrent_requests_for_one_name_sign_once`（16 个并发只签一次、拿到同一张）；`crates/rurge-engine/tests/mitm.rs`：`a_reload_with_the_same_material_keeps_the_leaves`（同材料的重载沿用同一个 `Authority`、缓存里的叶证书是同一张）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | 看不出（缓存是进程内的，手工看不出同一张叶证书被复用；用例经签名次数与同一张证书的指针断言） |
| FR-HTTP-03 | 「靠嗅探到的 SNI 认出的 TLS…连接，不带端口的项不看端口」 | 做了 | `crates/rurge-http/src/mitm/decrypt.rs::Mitm::decides`（有 SNI 时比 `crates/rurge-config/src/hostlist.rs::HostList::matches_sni`）、`crates/rurge-config/src/hostlist.rs::HostList::matches_sni`（不带端口的项不看端口，带端口的项照旧要端口相同，`:0` 不看端口） | `crates/rurge-config/src/hostlist.rs`：`matches_sni_ignores_the_port_of_a_bare_entry`（`example.com` 在 8443 命中、`example.com:8443` 在 443 不命中、`example.com:0` 在所有端口命中、排除项）；`crates/rurge-http/src/mitm/decrypt.rs`：`decides_lists_first_then_source_name_and_alpn`（有 SNI 8443 也 `Decrypt`、没有 SNI 的 8443 `NotListed`、没有 SNI 的 443 `Decrypt`）；`crates/rurge-engine/tests/mitm.rs`：`without_sni_the_target_decides_on_443_only`、`a_written_port_makes_its_port_sniffable`、`a_bare_entry_does_not_sniff_another_port`、`a_zero_port_entry_widens_its_own_target_only`；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M2/W2、M2/M2、M2/L2、M2/P3、M2/P4 |
| FR-HTTP-03 | 「…与 QUIC 连接，不带端口的项不看端口」 | 待读（任务 5） | | | |
| FR-HTTP-03 | 「`client-source-address`」 | 做了 | 同「MITM（FR-HTTP-03、04、14）」的「「`[MITM]` 九个键」之 `client-source-address`」」一行 | 同上 | M2/P3、M2/P5 |
| FR-HTTP-03 | 「`skip-server-cert-verify`」 | 做了 | 同「MITM（FR-HTTP-03、04、14）」的「「`[MITM]` 九个键」之 `skip-server-cert-verify`」」一行 | 同上 | M2/P2 |
| FR-HTTP-03 | 「证书固定提示日志」 | 做了 | `crates/rurge-engine/src/serve.rs::serve_http`（`pinning`：握手完成、读到的明文字节是 0、客户端关的）、`crates/rurge-engine/src/serve.rs::log_pinning`、`crates/rurge-engine/src/mitm.rs::MitmShared::warn_now`（每个名字每小时至多一行 WARN，其余 DEBUG） | `crates/rurge-engine/tests/mitm.rs`：`a_client_that_closes_after_the_handshake_gets_the_pinning_hint`（一行 WARN、第二次同一名字只有 DEBUG、两条失败记录、`noRequest` 计 2）、`a_request_head_that_times_out_is_no_pinning_hint`（等请求头超时不算）、`a_stop_after_the_handshake_is_no_pinning_hint`（停止不算）、`through_the_virtual_interface_the_warnings_name_no_host`（经虚拟网卡的名字只在 DEBUG 与记录里）、`an_untrusting_client_is_warned_once_an_hour_and_counted_each_time`（不信任的 WARN 同样限频）；`crates/rurge-engine/src/mitm.rs`：`a_hint_is_warned_again_only_after_an_hour`、`a_full_table_is_cleared_before_a_new_key`（限频表）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M2/P1 |
| FR-HTTP-03 | 「CA 通过 `ca-p12`…提供」 | 做了 | 同「MITM（FR-HTTP-03、04、14）」的「「`[MITM]` 九个键」之 `ca-p12`」」一行 | 同上 | M2/W1、M2/M1、M2/L1、M2/P6 |
| FR-HTTP-03 | 「…/ `ca-keystore-name` 提供」 | 做了 | 同「MITM（FR-HTTP-03、04、14）」的「「`[MITM]` 九个键」之 `ca-keystore-name`」」一行 | 同上 | — |
| FR-HTTP-03 | 「`rurge mitm ca generate` 生成 CA」 | 做了 | `crates/rurge/src/cli/mitm.rs::generate_ca`、`crates/rurge-http/src/mitm/generate.rs::generate`（P-256、十年、回拨两天、BasicConstraints 与 KeyUsage 关键、SKI；四个固定名字的文件；只建新文件、失败时删掉本次写出的；写回读一次 p12） | `crates/rurge-http/src/mitm/generate.rs`：`a_generated_ca_is_p256_backdated_and_ten_years_long`（P-256、回拨两天、十年、两个扩展关键、SKI）、`the_p12_is_3des_with_a_sha1_mac_and_reads_back`（3DES 与 SHA1 的 MAC、读得回）、`the_conf_is_a_mitm_section_that_loads`（写出的 `[MITM]` 节能加载）、`certificate_pem_is_the_standard_text`（PEM 是标准文本）；`crates/rurge/tests/cli.rs`：`mitm_ca_generate_writes_four_files_and_prints_no_secret`（四个文件都写好、PEM 与 DER 是同一张、`conf` 写进 Profile 能加载；2026-11-03 这条用例为 NFR-03 之 Windows 一行另加了一条断言（`f485306`），这一行引的断言没有改）、`mitm_ca_generate_does_not_overwrite_or_create_a_directory`（目录不存在或任一文件已存在都不写）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M2/W1、M2/M1、M2/L1 |
| FR-HTTP-03 | 「…并给出各平台信任安装指引」 | 做了 | `crates/rurge/src/cli/mitm.rs::announce`（四个文件的路径、把 `rurge-mitm.conf` 抄进 Profile 的说明；Windows `certutil -user -addstore Root`、macOS `security add-trusted-cert`、Debian / Ubuntu `update-ca-certificates`、Fedora / RHEL `update-ca-trust`、Arch `trust anchor`、NSS 库、Firefox、Android、iOS；不打印口令与私钥；rurge 自己不装） | `crates/rurge/tests/cli.rs`：`mitm_ca_generate_writes_four_files_and_prints_no_secret`（断言九行安装文字各有它的那一段——`certutil -user -addstore Root`、`security add-trusted-cert`、`update-ca-certificates`、`update-ca-trust`、`trust anchor`、`sql:$HOME/.pki/nssdb`、`Firefox`、`Android`、`iOS`——与「to use it: copy the lines of rurge-mitm.conf into your profile」一句，不是逐字断言整行；并断言输出里没有口令与私钥。2026-11-03 这条用例为 NFR-03 之 Windows 一行另加了一条断言（`f485306`），这一行引的断言没有改）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M2/W1、M2/M1、M2/L1、M2/P5 |
| FR-HTTP-03 | 「`GET /v1/mitm/ca` 导出 DER」 | 做了 | `crates/rurge-api/src/routes/mitm.rs::ca`（当前一代 CA 证书的 DER、`Content-Type: application/x-x509-ca-cert`、`Content-Disposition: attachment; filename="rurge-ca.cer"`；没有可用的 CA 时 404） | `crates/rurge-api/tests/api.rs`：`the_mitm_ca_is_given_in_der`（状态码、两个头、正文与配置里的 CA 逐字节相同、`?x-key=` 也认）、`the_mitm_ca_is_a_404_without_a_usable_ca`（404、正文 `{"error":"no usable MITM CA"}`）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M2/W4、M2/M4、M2/L4 |
| FR-HTTP-04 | 「`auto-quic-block`：解析 QUIC Initial 的 SNI」 | 待读（任务 5） | | | |
| FR-HTTP-04 | 「命中 MITM 列表时阻断 UDP 使客户端回落」 | 待读（任务 5） | | | |
| FR-HTTP-14 | 「`http-api-tls`：API 使用 MITM CA 签发的证书提供 HTTPS」 | 做了 | `crates/rurge-api/src/tls.rs::serve`（监听器外面套一层 TLS：按 SNI、没有 SNI 时按这条连接进来的本地地址签叶证书，ALPN 只有 `http/1.1`，同时在握手的至多 64 条、每条 30 秒；没有可用的 CA 时不启动）、`crates/rurge-http/src/mitm/api.rs`（API 的 TLS 服务端配置与客户端的根）、`crates/rurge-engine/src/engine.rs::api_tls`（每条新连接问一次，给最后一个生效过的 `Authority`）、`crates/rurge/src/cli/api_client.rs`（`status` / `reload` / `stop` 带 `-c` 时走 HTTPS，根只有 Profile 的 CA、按地址校验） | `crates/rurge-api/tests/api.rs`：`the_api_is_served_over_tls_with_the_mitm_ca`（同一个 API 的应答经 HTTPS、叶证书的 SAN 是 `127.0.0.1`、签发者是测试 CA、ALPN 是 `http/1.1`、同一个端口不讲明文）、`serve_tls_without_a_ca_answers_nothing`（没有 CA 时每条握手都失败、不退明文）；`crates/rurge/tests/run/mitm.rs`：`status_and_stop_reach_an_api_served_over_https`（守护进程打印 `api on https://`、`rurge status` 第一行是 `rurge at https://127.0.0.1:<端口>`、`stop` 退出 0、`--remote` 明文连不上）、`without_a_ca_the_api_is_not_started_and_rurge_runs`（WARN 一行、没有 `api on`、进程照常跑）；`crates/rurge-engine/tests/mitm.rs`：`api_tls_is_the_last_ca_a_generation_had`（重载换 CA 后新连接用新的、用不了时留旧的）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M2/W4、M2/M4、M2/L4、M2/P6 |

### 重写与 Map Local（FR-HTTP-05 ～ 09）

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| FR-HTTP-05 | 「URL Rewrite：`header`」 | 做了 | `crates/rurge-http/src/rewrite/url.rs::NewUrl::parse`（新 URL 的读法与它预备好的三样请求目标）、`crates/rurge-http/src/rewrite/url.rs::retarget_request`（请求行与 `Host` 换到新目标、保持请求原来的形式）、`crates/rurge-http/src/rewrite/url.rs::evaluate`（`UrlMode::Header`）、`crates/rurge-http/src/engine.rs::Requests::handle`（答复之前换目标、不重选策略、按新主机换对上游 TLS） | `crates/rurge-engine/tests/rewrite.rs`：`the_manual_url_rewrite_header_example`（手册第一例：经监听器与 SOCKS5 两条路，源站收到的请求行是 `GET /s?q=1`、`Host` 是 `www.google.com:{端口}`，两条记录的备注、`rule` 与 `policy` 各各对上、`url` 计 2）；`crates/rurge-http/src/rewrite/url.rs`：`retargeting_keeps_the_form_and_sets_host`（绝对形式保持绝对、源站形式保留路径与查询串、`Host` 换成新 authority、HTTP/2 客户端无 `Host` 时不加）；`crates/rurge-http/src/engine.rs`：`a_changed_host_is_written_back`（改过的 `Host` 在发出之前写回）；`crates/rurge-engine/tests/rewrite.rs`：`rewrites_on_a_decrypted_https_request`（解密请求被换到另一个 TLS 源站）、`http_rewritten_to_https_reaches_a_tls_origin_without_a_ca`（明文改写为 `https` 不需要 CA）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M3/W6、M3/M6、M3/L6 |
| FR-HTTP-05 | 「…`302`」 | 做了 | `crates/rurge-http/src/rewrite/url.rs::evaluate`（`UrlMode::Found`：`Location` 能当头值时答 `302`）、`crates/rurge-http/src/rewrite/url.rs::redirect`（只有 `Location` 与 `Content-Length: 0` 的应答，连接不关）、`crates/rurge-http/src/engine.rs::Requests::handle`（记状态码、结束会话、不拨号） | `crates/rurge-engine/tests/rewrite.rs`：`the_manual_url_rewrite_302_and_307_examples`（302 那一半：应答是 `HTTP/1.1 302`、`Location` 是 `https://yach.me/`、`Content-Length` 是 0、选定的守卫零连接、记录状态码 302 与备注、`url` 计 1）；`crates/rurge-http/src/rewrite/url.rs`：`only_the_first_header_rule_applies`（同一次求值里 `302` 的状态码与 `Location`）；`crates/rurge-http/src/engine.rs`：`a_redirect_is_answered_without_a_dial`（不拨号）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M3/W6、M3/M6、M3/L6 |
| FR-HTTP-05 | 「…`307`」 | 做了 | `crates/rurge-http/src/rewrite/url.rs::evaluate`（`UrlMode::Temporary`：答 `307`，其余同 `302`）、`crates/rurge-http/src/rewrite/url.rs::redirect`、`crates/rurge-http/src/engine.rs::Requests::handle` | `crates/rurge-engine/tests/rewrite.rs`：`the_manual_url_rewrite_302_and_307_examples`（307 那一半：应答是 `HTTP/1.1 307`、`Location` 与 `Content-Length: 0`、守卫零连接、记录状态码 307 与备注、`url` 计 1）；`crates/rurge-http/src/rewrite/url.rs`：`redirects_put_the_gateway_in_and_give_up_without_one`（`307` 那一支的 `Location` 与状态码）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M3/W6、M3/M6、M3/L6 |
| FR-HTTP-05 | 「…`reject`」 | 做了 | `crates/rurge-http/src/rewrite/url.rs::evaluate`（`UrlMode::Reject`）、`crates/rurge-http/src/engine.rs::Requests::handle`（`pages.reject` 为真时答 403 的页、页里有 `The request was rejected by a URL Rewrite rule.`；为假时关连接；记录是 `Rejected("REJECT")`） | `crates/rurge-engine/tests/rewrite.rs`：`the_manual_url_rewrite_reject_example`（页开着答 403 且页里有那一句、关着读不到字节、守卫零连接、记录是 `Rejected("REJECT")`、`url` 计 1）；`crates/rurge-http/src/rewrite/url.rs`：`reject_answers_and_ends_the_rules`（`reject` 之后不再求后面的规则）；`crates/rurge-http/src/engine.rs`：`a_url_rewrite_reject_answers_like_reject`（同一个答复）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M3/W6、M3/M6、M3/L6 |
| FR-HTTP-05 | 「首个 header 规则生效」 | 做了 | `crates/rurge-http/src/rewrite/url.rs::evaluate`（一条 `header` 已经生效之后的 `header` 规则不再求、也不留备注） | `crates/rurge-http/src/rewrite/url.rs`：`only_the_first_header_rule_applies`（两条 `header` 加一条 `302`：备注恰好是第一条 `header` 与那条 `302` 两条、第二条 `header` 一条也没有，结果 URL 是第一条的）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| FR-HTTP-05 | 「结果 URL 合法性」 | 做了 | `crates/rurge-http/src/rewrite/url.rs::NewUrl::parse`（scheme 不是 `http` / `https` / `ws` / `wss`、主机空、端口读不成或是 0、三样请求目标造不成、`https` / `wss` 的主机当不了 TLS 名字时交 `None`）、`crates/rurge-http/src/rewrite/url.rs::evaluate`（交 `None` 时留 `Skipped`、这条不算第一条、接着求下一条） | `crates/rurge-http/src/rewrite/url.rs`：`which_rewritten_urls_are_valid`（`ftp://`、`http:///x`、`:0`、`:65536`、`/relative`、`a.test`、空串、`https://a..b/` 都不合法；`ws` / `wss`、缺省端口、空的端口、用户名口令与 `#` 之后的去掉、`http://a..b` 可以）；`an_invalid_header_result_does_not_count_as_the_first`（第一条结果不是合法 URL 时留 `Skipped`、第二条 `header` 生效）；`a_location_that_cannot_be_a_header_value_is_skipped`（`Location` 不能当头值时同一条规则跳过）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| FR-HTTP-05 | 「`{{{GATEWAY_ADDRESS}}}`」 | 做了 | `crates/rurge-http/src/rewrite/url.rs::evaluate`（`rule.gateway` 为真时只在 `302` / `307` 上把占位换成 `gateway()` 的地址、没有时留 `Skipped` 接着求下一条）、`crates/rurge-http/src/engine.rs::Upstreams::gateway` 与 `crates/rurge-engine/src/http.rs::ConnUpstreams::gateway`（网络表上第一个 IPv4 路由器，经 `EngineShared.network`） | `crates/rurge-http/src/rewrite/url.rs`：`redirects_put_the_gateway_in_and_give_up_without_one`（有网关时 `Location` 是 `http://127.0.0.2/login`、网关只问一次；没有网关时留 `Skipped` 并求到下一条 `302`）；`crates/rurge-engine/tests/rewrite.rs`：`a_redirect_carries_the_first_ipv4_gateway`（IPv6 路由器排在前面时仍取 IPv4 的 `127.0.0.2`；没有主接口时这条跳过、请求照原样到源站、`skipped` 计 1）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| FR-HTTP-05 | 「对 Host…重建的 URL 也匹配」 | 做了 | `crates/rurge-http/src/rewrite/url.rs::candidates`（第二个候选是用 `Host` 头的值按 `{scheme}://{Host}{路径与查询串}` 重建的；与请求行的 URL 相同的不重复）、`crates/rurge-http/src/engine.rs::Requests::handle`（把 `req.headers().get(HOST)` 交给 `candidates`） | `crates/rurge-http/src/rewrite/url.rs`：`candidates_are_the_url_the_host_and_the_target`（三个候选的次序、相同的不重复、`Host` 不是 UTF-8 或为空时不加、端口随 scheme 的缺省写不写）；`crates/rurge-engine/tests/rewrite.rs`：`the_manual_matching_details_example`（请求行指向守卫、`Host` 是 `ad.com`，按 `Host` 重建的 URL 命中 `reject`、守卫零连接）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| FR-HTTP-05 | 「对…连接主机名重建的 URL 也匹配」 | 做了 | `crates/rurge-http/src/rewrite/url.rs::candidates`（第三个候选是用连接的目标主机与端口重建的；与前面的相同的不重复） | `crates/rurge-http/src/rewrite/url.rs`：`candidates_are_the_url_the_host_and_the_target`（三个候选的顺序，第三个就是连接目标重建的那一个，端口不是 scheme 缺省时写端口，相同的不重复）；`crates/rurge-engine/tests/rewrite.rs`：`the_manual_matching_details_example`（同上一行）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| FR-HTTP-06 | 「Header Rewrite：方向」 | 做了，补了用例 | `crates/rurge-config/src/rewrite.rs::parse_header`（`http-request` / `http-response` 不分大小写；省略时当 `http-request`）、`crates/rurge-http/src/rewrite/header.rs::evaluate`（`rule.direction != direction` 的规则不作用、看下一个）、`crates/rurge-http/src/engine.rs::Requests::handle`（请求一侧与应答一侧各求一次） | `crates/rurge-config/src/rewrite.rs`：`header_rewrite_reads_the_manual_examples`（四行手册例子与省略方向一行的 `direction` 依次是 Request/Request/Request/Response/Request）；`crates/rurge-http/src/rewrite/header.rs`：`a_rule_of_one_direction_leaves_the_other_alone`（请求规则在 Response 方向不改一个头、应答规则在 Request 方向也不改；`5679d6c` 补；**这一条改过，CI 没有见过**）；`the_manual_header_rewrite_examples`（请求三条与应答一条各作用各的）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M3/W6、M3/M6、M3/L6 |
| FR-HTTP-06 | 「四种动作」之 `header-add` | 做了 | `crates/rurge-http/src/rewrite/header.rs::evaluate`（`HeaderRuleAction::Add`：`headers.append`，同名已有也追一行） | `crates/rurge-http/src/rewrite/header.rs`：`the_four_actions`（`DNT: 0` 之外再加一行 `DNT: 1`，两行都在、`changed` 为真）；`crates/rurge-engine/tests/rewrite.rs`：`the_manual_header_rewrite_examples`（手册的 `header-add DNT 1`：源站恰收到一个 `DNT: 1`）、`the_manual_header_rewrite_regex_example` 之外的省略方向那一 rig（`header-add DNT 1` 不带方向时也作用在请求上）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M3/W6、M3/M6、M3/L6 |
| FR-HTTP-06 | 「四种动作」之 `header-del` | 做了 | `crates/rurge-http/src/rewrite/header.rs::evaluate`（`HeaderRuleAction::Del`：`headers.remove`，同名的行一行也不留） | `crates/rurge-http/src/rewrite/header.rs`：`the_four_actions`（两条 `Cookie` 与一条别的字段：`Cookie` 一行不剩、别的留住）；`crates/rurge-engine/tests/rewrite.rs`：`the_manual_header_rewrite_examples`（`header-del Cookie` 把客户端两条 `Cookie` 都去掉、源站一个也看不到）、`a_changed_host_is_written_back` 里 `header-del Host` 的那一支在 `crates/rurge-http/src/engine.rs`：`deleting_host_does_not_take`；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M3/W6、M3/M6、M3/L6 |
| FR-HTTP-06 | 「四种动作」之 `header-replace` | 做了 | `crates/rurge-http/src/rewrite/header.rs::evaluate`（`HeaderRuleAction::Replace`：`OccupiedEntry::insert`，同名的几行换成一行、留在第一行的位置；没有这个名字时什么也不做） | `crates/rurge-http/src/rewrite/header.rs`：`the_four_actions`（没有 `User-Agent` 时 `changed` 为假、备注为空；有两条 `User-Agent` 时换成一行 `Unknown`、这个字段在表里的位置不变）；`crates/rurge-engine/tests/rewrite.rs`：`the_manual_header_rewrite_examples`（`header-replace User-Agent Unknown`：源站收到的 `user-agent` 是 `Unknown`）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M3/W6、M3/M6、M3/L6 |
| FR-HTTP-06 | 「四种动作」之 `header-replace-regex` | 做了 | `crates/rurge-http/src/rewrite/header.rs::evaluate`（`HeaderRuleAction::ReplaceRegex`：每行的全部匹配都换，只要一行换出的值不能进头就跳过这一条；值不是 UTF-8 的行留着） | `crates/rurge-http/src/rewrite/header.rs`：`the_four_actions`（手册的 `User-Agent` 一例 `Safari` -> `Chrome`，逐字断言前后两条）、`a_replace_regex_result_that_is_no_header_value_leaves_the_value`（换出 `0x01` 时值不变、`changed` 为假、留一条 `Skipped`）；`crates/rurge-engine/tests/rewrite.rs`：`the_manual_header_rewrite_regex_example`（源站收到的 `user-agent` 是手册 After 那一串）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M3/W6、M3/M6、M3/L6 |
| FR-HTTP-06 | 「顺序生效」 | 做了 | `crates/rurge-http/src/rewrite/header.rs::evaluate`（按 `rewrite.header` 写的次序逐条求值、表在原地改，每一条都见到前一条留下的；框架头变了就到此为止）、`crates/rurge-config/src/rewrite.rs::parse_header`（按写的次序收集） | `crates/rurge-http/src/rewrite/header.rs`：`the_four_actions`（`header-del DNT` 之后 `header-add DNT 1`：最终只有一行 `DNT: 1`；次序倒过来会不同）、`framing_changes_are_found`（框架头变了之后后面的规则不再作用）、`a_changed_host_is_the_last_line_and_a_removed_one_comes_back`（`Host` 的加与删各按次序）；`crates/rurge-engine/tests/rewrite.rs`：`the_manual_header_rewrite_examples`（del 在 add 之前的那一 rig：客户端 `DNT: 0` 去掉、恰好一个 `DNT: 1`）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M3/W6、M3/M6、M3/L6 |
| FR-HTTP-06 | 「改动消息框架头则拒绝…」 | 做了 | `crates/rurge-http/src/rewrite/header.rs::evaluate`（`framing_lines` 前后比：`Content-Length` 或 `Transfer-Encoding` 变了就留一条 `Failed`、返回 `framing`、后面的规则不再求）、`crates/rurge-config/src/rewrite.rs::parse_header`（这样的行给 `W0041` 并保留）、`crates/rurge-config/src/diagnostic.rs::codes::W_REWRITE_FRAMING` | `crates/rurge-config/src/rewrite.rs`：`framing_headers_are_kept_with_w0041`（两行都在，两条 `W0041` 的正文与 hint 逐字）；`crates/rurge-http/src/rewrite/header.rs`：`framing_changes_are_found`（删 `Transfer-Encoding`、加 `Content-Length` 都算框架变了、后面的规则不再求；同值的 `header-replace` 不算；本来没有这个头时删掉也不算）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| FR-HTTP-06 | 「…并使请求失败」 | 做了 | `crates/rurge-http/src/engine.rs::Requests::handle`（请求一侧 `out.framing` 为真时先 `failed()`、再 `failed` 页或关连接、这个请求不拨号；应答一侧丢掉上游的应答、那条上游连接不回池）、`crates/rurge-http/src/engine.rs::failed` 与 `crates/rurge-http/src/engine.rs::RESPONSE_FRAMING_LOST` | `crates/rurge-http/src/engine.rs`：`a_request_whose_framing_changes_fails_before_the_dial`（请求一侧：502 页里有「…so the request was not sent.」、`dials()` 是 0、记录 `Failed("Header Rewrite changed Content-Length or Transfer-Encoding")`、`header` 与 `failed` 各计 1；页关着时不答就关）、`a_response_whose_framing_changes_fails`（应答一侧：502 页里有「…of the response, so the response was not passed on.」且没有「the request was not sent」、那条上游连接不回池、下一条请求另拨一次）；`crates/rurge-engine/tests/rewrite.rs`：`the_manual_header_rewrite_framing_limit`（502、源站一次也没被问、记录 `Failed`、`failed` 计 1）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| FR-HTTP-07 | 「Body Rewrite：正则多对替换」 | 待读（任务 3） | | | |
| FR-HTTP-07 | 「按行锚定」 | 待读（任务 3） | | | |
| FR-HTTP-07 | 「`^$` 生成正文」 | 待读（任务 3） | | | |
| FR-HTTP-07 | 「UTF-8 检查」 | 待读（任务 3） | | | |
| FR-HTTP-07 | 「jq 表达式」 | 待读（任务 3） | | | |
| FR-HTTP-07 | 「大小限制（请求 32 MB 断开…）」 | 待读（任务 3） | | | |
| FR-HTTP-07 | 「（…响应 10 MB 直通）」 | 待读（任务 3） | | | |
| FR-HTTP-07 | 「chunked…限制」 | 待读（任务 3） | | | |
| FR-HTTP-07 | 「…`Expect` 限制」 | 待读（任务 3） | | | |
| FR-HTTP-07 | 「解压…」之 `gzip`、`x-gzip`、`deflate` | 待读（任务 3） | | | |
| FR-HTTP-07 | 「解压…」之 `br`、`zstd` | 待读（任务 3） | | | |
| FR-HTTP-07 | 「…与 `Content-Length` 重算」 | 待读（任务 3） | | | |
| FR-HTTP-07 | 「先于脚本」 | 待读（任务 3） | | | |
| FR-HTTP-08 | 「Map Local：四种 `data-type`」之 `file` | 待读（任务 3） | | | |
| FR-HTTP-08 | 「Map Local：四种 `data-type`」之 `text` | 待读（任务 3） | | | |
| FR-HTTP-08 | 「Map Local：四种 `data-type`」之 `tiny-gif` | 待读（任务 3） | | | |
| FR-HTTP-08 | 「Map Local：四种 `data-type`」之 `base64` | 待读（任务 3） | | | |
| FR-HTTP-08（总设计第 1.5 节） | 「`data` 路径…」 | 待读（任务 3） | | | |
| FR-HTTP-08 | 「…/ URL…」 | 待读（任务 3） | | | |
| FR-HTTP-08 | 「…/ 文本…」 | 待读（任务 3） | 同「重写与 Map Local（FR-HTTP-05 ～ 09）」的「「Map Local：四种 `data-type`」之 `text`」」一行 | 同上 | |
| FR-HTTP-08 | 「…/ Base64」 | 待读（任务 3） | 同「重写与 Map Local（FR-HTTP-05 ～ 09）」的「「Map Local：四种 `data-type`」之 `base64`」」一行 | 同上 | |
| FR-HTTP-08 | 「`header` 两种形式」之 `a:b\|c:d` | 待读（任务 3） | | | |
| FR-HTTP-08 | 「`header` 两种形式」之 Base64 编码的多行头 | 待读（任务 3） | | | |
| FR-HTTP-08 | 「`status-code` 200–999」 | 待读（任务 3） | | | |
| FR-HTTP-08 | 「`Content-Type` 补全」 | 待读（任务 3） | | | |
| FR-HTTP-08 | 「需 Rewrite 功能开启」 | 待读（任务 3） | | | |
| FR-HTTP-09 | 「流水线顺序 Header Rewrite → URL Rewrite → Body Rewrite…」 | 待读（任务 3） | | | |
| FR-HTTP-09 | 「…→ 脚本」 | 待读（任务 3） | | | |
| FR-HTTP-09 | 「Map Local 短路」 | 待读（任务 3） | | | |
| FR-HTTP-09 | 「每请求 / 响应最多一个脚本」 | 待读（任务 3） | | | |

### 请求记录与抓包（FR-HTTP-10、11，FR-RULE-14）

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| FR-HTTP-10 | 「请求记录：最近请求…与活动请求」之经 HTTP 引擎的每个请求各一条 | 待读（任务 4） | | | |
| FR-HTTP-10 | 「字段包含…目标…状态…」之经引擎的请求的方法、URL 与状态码 | 待读（任务 4） | | | |
| FR-HTTP-10（阶段 1 对照第 410 行；阶段 3 对照第 250 行） | 「…来源（…进程…）」之经引擎的每个请求 | 待读（任务 4） | | | |
| FR-HTTP-10（阶段 1 对照第 410 行；阶段 3 对照第 251 行） | 「…来源（…设备）」 | 待读（任务 4） | | | |
| FR-HTTP-10（阶段 1 对照第 417 行；阶段 2 验收第 527 行；总设计第 1.5 节） | 「…远端地址」之复用的连接、隧道与 UDP 流 | 待读（任务 4） | | | |
| FR-HTTP-10 | 「…上下行流量」之两个正文的字节数 | 待读（任务 4） | | | |
| FR-HTTP-10 | 「…各阶段耗时」之 `receiveMs` | 待读（任务 4） | | | |
| FR-HTTP-10（总设计第 1.5 节） | 「…各阶段耗时」之 `dnsMs` 在解析途中 | 待读（任务 4） | | | |
| FR-HTTP-10（阶段 1 对照第 421 行） | 「…备注（重写…）」 | 待读（任务 4） | | | |
| FR-HTTP-10（阶段 1 对照第 421 行） | 「…备注（…脚本…）」 | 待读（任务 4） | | | |
| FR-HTTP-10（阶段 1 对照第 421 行） | 「…备注（…直通原因）」 | 待读（任务 4） | | | |
| FR-HTTP-10 | 「终止活动请求」之经引擎的请求 | 待读（任务 4） | | | |
| FR-HTTP-11 | 「抓包：请求 / 响应头…保存」 | 待读（任务 4） | | | |
| FR-HTTP-11 | 「…与正文保存」 | 待读（任务 4） | | | |
| FR-HTTP-11 | 「（总容量…可配）」 | 待读（任务 4） | | | |
| FR-HTTP-11 | 「（…单条上限可配）」 | 待读（任务 4） | | | |
| FR-HTTP-11 | 「命名会话」 | 待读（任务 4） | | | |
| FR-HTTP-11 | 「`always-capture`」 | 待读（任务 4） | 同「请求记录与抓包（FR-HTTP-10、11，FR-RULE-14）」的「「`always-capture=<session>`：强制抓包…」」一行 | 同上 | |
| FR-HTTP-11 | 「通过 API…查看」 | 待读（任务 4） | | | |
| FR-HTTP-11 | 「通过…Dashboard 查看」 | 待读（任务 4） | | | |
| FR-RULE-14 | 「`always-capture=<session>`：强制抓包…」 | 待读（任务 4） | | | |
| FR-RULE-14 | 「…并归入命名会话」 | 待读（任务 4） | | | |

### 功能开关（FR-HTTP-13）

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| FR-HTTP-13 | 「功能开关 `mitm`…运行时切换并持久化」 | 做了 | `crates/rurge-engine/src/state.rs`（`STATE_VERSION` 是 2、`Features` 的 `mitm` 是可省的 `bool`、没设过不写、第 1 版读作没设过）、`crates/rurge-engine/src/engine.rs::set_mitm` 与 `crates/rurge-engine/src/engine.rs::restore_switches`、`crates/rurge-api/src/routes/features.rs::mitm`（`GET` / `POST`，经 `StateStore::update` 写进 `state.json`） | `crates/rurge-api/tests/api.rs`：`the_mitm_switch_is_read_switched_and_kept`（缺省开、`POST` 切换、`state.json` 的 `version` 2 与 `features.mitm`）；`crates/rurge/tests/cli.rs`：`a_daemon_restores_the_mitm_switch_from_state_json`（守护进程从 `state.json` 的值装回、重启之后照旧）；`crates/rurge-engine/tests/mitm.rs`：`the_switch_is_restored_from_state_json`；`crates/rurge-engine/src/state.rs`：`a_version_1_file_leaves_the_four_switches_unset`、`a_version_2_file_is_what_phase_3_reads`、`loads_defaults_selections_and_tolerates_garbage`；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| FR-HTTP-13 | 「…`rewrite`…」 | 做了 | `crates/rurge-engine/src/engine.rs::rewrite_on`（初值真）、`Engine::set_rewrite`（写进 `state.json`）、`Engine::restore_switches`（启动时读回）、`crates/rurge-engine/src/state.rs`（`STATE_VERSION` 是 2、`Features` 的 `rewrite` 是可省的布尔）、`crates/rurge-api/src/routes/features.rs::get_feature` 与 `set_feature`（`GET` / `POST /v1/features/rewrite`）、`crates/rurge-engine/src/http.rs::ConnUpstreams::route`（每个请求读一次，关着时不给这一代的重写） | `crates/rurge-api/tests/api.rs`：`the_rewrite_switch_is_read_switched_and_kept`（缺省开、`POST` 切、`state.json` 的 `version` 2 与 `features.rewrite`）；`crates/rurge/tests/cli.rs`：`a_daemon_restores_the_rewrite_switch_from_state_json`（守护进程照 `state.json` 的值装回）；`crates/rurge-engine/tests/rewrite.rs`：`the_rewrite_switch_turns_every_rewrite_off`（关着时 `header` 与 `302` 都不作用、计数与备注都不动、再开回来又作用）；`crates/rurge-engine/src/state.rs`：`a_version_1_file_leaves_the_four_switches_unset`、`a_version_2_file_is_what_phase_3_reads`；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M3/P7 |
| FR-HTTP-13 | 「…`capture`…」 | 待读（任务 4） | | | |

### 配置与观测（FR-CFG-06、FR-CFG-18、FR-OBS-02）

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| FR-CFG-06（兼容性清单 2.1 节；阶段 1 对照第 80 行） | 「…各键随所属功能阶段生效」之 `http-api-tls` | 做了 | 同「MITM（FR-HTTP-03、04、14）」的「「`http-api-tls`：API 使用 MITM CA 签发的证书提供 HTTPS」」一行 | 同上 | M2/W4、M2/M4、M2/L4、M2/P6 |
| FR-CFG-06（兼容性清单 2.1 节） | 「…各键随所属功能阶段生效」之 `force-http-engine-hosts` | 做了 | 同「会话入口与 HTTP 引擎（FR-HTTP-01、02）」的「「`force-http-engine-hosts`」」一行 | 同上 | M1/P3 |
| FR-CFG-06（兼容性清单 2.1 节） | 「…各键随所属功能阶段生效」之 `always-raw-tcp-hosts` | 做了 | 同「会话入口与 HTTP 引擎（FR-HTTP-01、02）」的「「`always-raw-tcp-hosts`」」一行 | 同上 | M1/P3 |
| FR-CFG-06（兼容性清单 2.1 节） | 「…各键随所属功能阶段生效」之 `always-raw-tcp-keywords` | 做了 | 同「会话入口与 HTTP 引擎（FR-HTTP-01、02）」的「「`always-raw-tcp-keywords`」」一行 | 同上 | M1/P3 |
| FR-CFG-06（兼容性清单 2.1 节） | 「…各键随所属功能阶段生效」之 `show-error-page`（经引擎的请求上） | 做了 | `crates/rurge-engine/src/http.rs::ConnUpstreams::route`（把 `Pages { error, reject }` 交给引擎）、`crates/rurge-http/src/engine.rs::failed`、`crates/rurge-http/src/engine.rs::timed_out` | `crates/rurge-engine/tests/http.rs`：`the_listener_answers_refusals_and_failures_as_the_profile_says`（失败答 502 页）、`refusals_are_answered_on_every_entry`（四种入口）、`a_late_response_head_is_a_504`（等应答头超时答 504 页）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| FR-CFG-06（兼容性清单 2.1 节） | 「…各键随所属功能阶段生效」之 `show-error-page-for-reject`（经引擎的请求上） | 做了 | `crates/rurge-engine/src/http.rs::ConnUpstreams::route`、`crates/rurge-http/src/engine.rs::answer` | `crates/rurge-engine/tests/http.rs`：`refusals_are_answered_on_every_entry`（`REJECT` 开着答 403 页、关着不答就关；`REJECT-TINYGIF` 不看这个设置答 GIF）、`the_listener_answers_refusals_and_failures_as_the_profile_says`；`crates/rurge-engine/tests/pipeline.rs`：`reject_rules_close_serve_gifs_or_render_pages`；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| FR-CFG-18（阶段 1 对照第 162 行） | 「Header Rewrite 省略方向」 | 做了 | `crates/rurge-config/src/rewrite.rs::parse_header`（第一个词不是 `http-request` / `http-response` 时方向当 `Direction::Request`） | `crates/rurge-config/src/rewrite.rs`：`header_rewrite_reads_the_manual_examples`（`^http://example.com header-add DNT 1` 的 `direction` 是 Request）；`crates/rurge-engine/tests/rewrite.rs`：`the_manual_header_rewrite_examples`（省略方向的那一 rig 作用在请求上、源站收到 `DNT: 1`）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 4 / M1a 加的（`docs/api/phase4.md` 的「M1a」一节） | 做了 | `crates/rurge-api/src/routes/rurge_http.rs::http`、`crates/rurge-engine/src/serve.rs::SniffCounts` | `crates/rurge-api/tests/api.rs`：`rurge_http_counts_start_at_zero`（`sniffed` 五个键按次序、零与 401）；`crates/rurge-engine/tests/sniff.rs`：`the_sniffs_are_counted`（五种结局各记一次）、`a_stop_during_the_wait_ends_a_tun_session_at_once`（被停止的嗅探不计；`f9fc0ba` 补了计数断言）、`which_sessions_are_sniffable`（可嗅探的条件）、`a_server_first_protocol_passes_after_the_wait`（等满时限之后照原样转发、记录不带协议；用例把时限调到 300 毫秒；缺省的 15 秒没有用例等满它，见 `docs/api/phase4.md`「M1a」的「没有覆盖的」第 2 条）、`a_plain_http_session_is_marked_http`、`neither_http_nor_tls_is_noted`、`a_sniff_that_waits_out_its_time_is_noted`、`a_client_that_closes_during_the_wait_has_no_note`；`crates/rurge-engine/tests/http.rs`：`a_tls_connection_is_still_relayed_as_it_is`（记录的 `sni` 与 `protocol=https`）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901），`f9fc0ba` 的计数断言：CI 没有见过 | — |
| FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 4 / M1b 加的（`docs/api/phase4.md` 的「M1b」一节） | 做了 | `crates/rurge-api/src/routes/rurge_http.rs::http`（`requests`）、`crates/rurge-engine/src/http.rs::ConnUpstreams::route`（计数）、`crates/rurge-api/src/routes/requests.rs::RequestJson`（`method` / `url` / `statusCode`）、`crates/rurge-engine/src/observe.rs::record_of` | `crates/rurge-engine/tests/http.rs`：`the_requests_are_counted`（拒绝也算）、`a_connection_that_never_reaches_the_rules_leaves_one_failed_record`、`a_connection_whose_requests_were_routed_leaves_no_record_of_its_own`、`the_carrier_connection_is_recorded_only_when_nothing_was_routed`（承载连接的记录）、`an_unknown_fake_address_in_a_plain_request_is_a_502`（先求规则、`statusCode=502`、`requests=1`；`f9fc0ba`、`d40ddcf` 补）、`a_request_record_has_its_method_url_and_status`（200 / 403 / `null`）、`a_late_response_head_is_a_504`（504 / `null`）、`a_url_with_userinfo_is_recorded_without_it`、`a_borrowed_connection_records_where_it_went`（借来的 `remoteAddr`、`connectMs`、`firstByteMs`；`f9fc0ba` 补）、`each_request_counts_its_own_bytes`（`up` / `down`、借来的连接）、`the_request_log_line_shows_no_query_and_no_userinfo`（日志）；`crates/rurge-http/src/engine.rs`：`the_session_of_a_request_says_what_the_rules_see`（`url` 由 `Host` 拼出）、`a_connect_on_a_sniffed_connection_goes_to_its_target`（嗅探到的 `CONNECT` 没有 URL）、`a_request_without_host_is_named_by_its_target`（没有 `Host` 时用连接的目标拼）；`crates/rurge-api/tests/api.rs`：`requests_carry_the_method_the_url_and_the_status_code`（键序与 `null`）、`rurge_http_counts_start_at_zero`；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901），`f9fc0ba` 与 `d40ddcf` 的补：CI 没有见过 | M1/W1、M1/W2、M1/M1、M1/M2、M1/L1、M1/L2 |
| FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 4 / M2a 加的（`docs/api/phase4.md` 的「M2a」一节） | 做了 | `crates/rurge-api/src/routes/features.rs::mitm`（`GET` / `POST`）、`crates/rurge-api/src/routes/mitm.rs::ca`（DER、两个头、没有 CA 时 404）、`crates/rurge-api/src/routes/rurge_http.rs::http`（`mitm` 四个数）、`crates/rurge-engine/src/serve.rs` 的 `MitmCounts` 与三类日志行、`crates/rurge-engine/src/state.rs`（第 2 版） | `crates/rurge-api/tests/api.rs`：`the_mitm_switch_is_read_switched_and_kept`（`{"enabled":bool}`、`state.json` 的 `version` 2 与 `features.mitm`）、`the_mitm_ca_is_given_in_der`（两个头逐字、正文是 CA 的 DER、`?x-key=` 也认）、`the_mitm_ca_is_a_404_without_a_usable_ca`（`{"error":"no usable MITM CA"}`）、`rurge_http_counts_start_at_zero`（`mitm` 四个键与次序、原始字节逐字）；`crates/rurge-engine/tests/mitm.rs`：`the_mitm_counts_survive_a_reload`、`a_stop_during_the_handshake_counts_nothing`、`a_client_that_closes_after_the_handshake_gets_the_pinning_hint`、`an_untrusting_client_is_warned_once_an_hour_and_counted_each_time`、`through_the_virtual_interface_the_warnings_name_no_host`、`a_handshake_the_client_abandons_is_failed_at_info`、`a_listed_host_is_decrypted_through_socks5`、`every_entry_decrypts_a_listed_host`、`two_requests_of_one_decrypted_connection_follow_their_own_rules`；`crates/rurge-engine/src/state.rs`：`a_version_1_file_leaves_the_four_switches_unset`、`a_version_2_file_is_what_phase_3_reads`；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 4 / M2b 加的（`docs/api/phase4.md` 的「M2b」一节） | 做了 | HTTP/2 没有新端点；`crates/rurge-http/src/engine.rs::serve_h2`、`crates/rurge-http/src/engine.rs::Requests::handle`（终止、看门狗、拒绝、`REJECT-DROP` 只重置这一个流）、`crates/rurge-http/src/engine.rs::prepare` 与 `crates/rurge-http/src/version.rs::fit`（`:authority`、`cookie` 合并成一个、连接专用的头）、记录的 `up` / `down`（共享的 h2 上游只算正文）与取消的原因 `client cancelled the request` | `crates/rurge-http/src/engine.rs`：`a_killed_h2_stream_is_reset_and_the_others_go_on`、`an_h2_client_that_stops_reading_one_stream_fails_only_that_request`（`client stopped reading`、别的流照常）、`a_rejected_h2_request_without_a_page_resets_only_its_stream`、`a_dropped_h2_request_is_held_then_reset`、`an_upstream_h2_stream_reset_mid_body_reaches_the_client_as_internal_error`、`a_stream_the_client_resets_is_cancelled_and_the_connection_stays`（原因 `client cancelled the request`）、`the_bytes_of_requests_on_a_shared_h2_connection_are_their_bodies`、`a_connect_over_h2_is_a_501_with_a_record`、`the_cookies_of_an_h2_request_are_joined_for_an_http1_upstream`、`an_h2_request_keeps_its_authority_and_te_trailers_to_an_h2_origin`、`an_idle_h2_connection_is_sent_goaway`、`a_stop_ends_an_h2_connection_with_goaway`；`crates/rurge-engine/tests/mitm.rs`：`each_h2_stream_follows_its_own_rule`、`a_connect_over_h2_has_one_record_and_a_501`、`the_cookies_of_an_h2_client_reach_an_http1_origin_as_one_field`、`concurrent_h2_streams_share_one_upstream_connection`；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 4 / M2c 加的（`docs/api/phase4.md` 的「M2c」一节） | 做了 | 没有新端点；`http-api-tls` 的代码同「MITM（FR-HTTP-03、04、14）」的「「`http-api-tls`：API 使用 MITM CA 签发的证书提供 HTTPS」」一行（`crates/rurge-api/src/tls.rs`）；`W0040` 在 `crates/rurge-http/src/mitm/authority.rs::check` 与 `crates/rurge-config/src/diagnostic.rs::W_API_TLS_NO_CA` | `crates/rurge-api/tests/api.rs`：`the_api_is_served_over_tls_with_the_mitm_ca`、`serve_tls_without_a_ca_answers_nothing`；`crates/rurge/tests/run/mitm.rs`：`status_and_stop_reach_an_api_served_over_https`、`without_a_ca_the_api_is_not_started_and_rurge_runs`；`crates/rurge/tests/cli.rs`：`check_says_the_api_does_not_start_without_a_usable_ca`（`W0040` 的正文与 `hint` 逐字）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M2/W4、M2/M4、M2/L4、M2/P6 |
| FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 4 / M3a 加的（`docs/api/phase4.md` 的「M3a」一节） | 做了 | `crates/rurge-api/src/routes/features.rs::get_feature` 与 `set_feature`（`GET` / `POST /v1/features/rewrite`，`set_feature` 里另有一行 INFO）、`crates/rurge-api/src/routes/rurge_http.rs::RewriteJson` 与 `http`（`rewrite` 六个键的次序）、`crates/rurge-http/src/rewrite/mod.rs::RewriteCounters` 与 `RewriteCounts`（六个计数）、`crates/rurge-engine/src/engine.rs::http_counts`；备注措辞在 `crates/rurge-http/src/rewrite/url.rs::evaluate` 与 `header.rs::evaluate`，失败页在 `crates/rurge-http/src/engine.rs::failed` 与 `RESPONSE_FRAMING_LOST` | `crates/rurge-api/tests/api.rs`：`rurge_http_counts_start_at_zero`（正文与原始字节里 `rewrite` 六个键的次序与初值逐字）、`the_rewrite_switch_is_read_switched_and_kept`（`{"enabled":bool}`、`POST` 的 `{}`、`state.json` 的 `version` 2 与 `features.rewrite`）；`crates/rurge-engine/tests/rewrite.rs`：`notes_and_counts_reach_the_api_shape`（六个数与备注的正文）、`counts_survive_a_reload`；`POST` 之后的 INFO 那一行 `rewrite switched via http-api` 的代码在 `set_feature`，没有断言它形状的用例（`rurge-api` 的用例没有抓日志的装置，补一条要加依赖，按「一条用例补得上」不成立，不补）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 4 / M3b 加的（`docs/api/phase4.md` 的「M3b」一节） | 待读（任务 3） | | | |
| FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 4 / M4a 加的（`docs/api/phase4.md` 的「M4a」一节） | 待读（任务 4） | | | |
| FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 4 / M4b 加的（`docs/api/phase4.md` 的「M4b」一节） | 待读（任务 4） | | | |
| FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 4 / M5a 加的（`docs/api/phase4.md` 的「M5a」一节） | 待读（任务 5） | | | |

### 总设计第 1.2 节另列的

这两句取自兼容性清单 3.1 节 `USER-AGENT`、`URL-REGEX` 两行；阶段 1 只对 HTTP 监听器转发的明文请求读过它们（阶段 1 对照第 181、182 行），这里读的是经 HTTP 引擎的每个请求（M6 设计第 3.2 节）。两句都按 FR-RULE-01 算 P0。

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| 总设计第 1.2 节（FR-RULE-01） | 「`USER-AGENT`：HTTP User-Agent，`*` `?` 通配，区分大小写」（3.1 节）之引擎里的每个请求 | 做了 | `crates/rurge-rules/src/matcher.rs::Matcher::eval`（`Matcher::UserAgent`）、`crates/rurge-http/src/engine.rs::prepare`（把 `User-Agent` 放进会话） | `crates/rurge-rules/src/matcher.rs`：`port_source_protocol_and_hostname_type_rules`（`USER-AGENT,Mozilla*` 命中、大小写不同不命中）；`crates/rurge-http/src/engine.rs`：`the_session_of_a_request_says_what_the_rules_see`（引擎里的请求带着 `User-Agent`）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| 总设计第 1.2 节（FR-RULE-01） | 「`URL-REGEX`：在完整 URL 里搜索的正则…」（3.1 节）之引擎里的每个请求 | 做了 | `crates/rurge-rules/src/matcher.rs::Matcher::eval`（`Matcher::UrlRegex`）、`crates/rurge-http/src/engine.rs::prepare`（把 URL 放进会话） | `crates/rurge-engine/tests/http.rs`：`two_requests_of_one_connection_follow_their_own_rules`（同一条客户端连接上两个请求按 `URL-REGEX` 各走一条策略）；`crates/rurge-engine/tests/pipeline.rs`：`keep_alive_requests_are_dialed_one_by_one`；`crates/rurge-rules/src/matcher.rs`：`url_regex_without_an_anchor_matches_anywhere_in_the_url`；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M1/P2 |

### 总设计第 1.5 节的遗留

这几句取自总设计第 1.5 节的表（M6 设计第 3.4 节与第 15 节第 1 条）；那张表别的行并进了别的表，出处写在那一行的「需求」一格里。

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| 总设计第 1.5 节（FR-HTTP-01） | 「经监听器来的绝对形式明文请求里，主机是 fake 地址时保留着它」 | 做了 | `crates/rurge-engine/src/http.rs::ConnUpstreams::named`、`crates/rurge-inbound/src/http.rs::handle`、`crates/rurge-http/src/engine.rs::prepare` | `crates/rurge-engine/tests/http.rs`：`a_fake_address_in_a_plain_request_is_turned_back_into_its_name`（请求行与 `Host` 里的 fake 地址换回域名再求规则与转发，反查不到时 502）；`crates/rurge-http/src/engine.rs`：`a_fake_address_is_turned_back_into_its_name`（会话的 `dst_host`、`url`、`http_host` 与上游的 `Host` 都换成名字）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| 总设计第 1.5 节（FR-OUT-01） | 「HTTP 一侧的 `REJECT-DROP` 看不到客户端提前关闭」 | 有意不同（已登记） | `crates/rurge-http/src/engine.rs::answer`（`RejectKind::Drop`：请求正文读完后客户端先关会提前结束停留，没读完时停到 `drop_hold`；差异登记在 M1 细化设计第 20 节第 8 条与兼容性清单 4.1 节 `REJECT-DROP` 一行） | `crates/rurge-http/src/engine.rs`：`a_dropped_request_is_held_then_closed`（请求正文读完、客户端不走，停满 `drop_hold` 再关）、`a_dropped_h2_request_is_held_then_reset`（HTTP/2 上停满再重置这个流）、`a_dropped_request_ends_early_when_the_client_closes`（请求正文读完后客户端关掉写端，停留提前结束、远早于 `drop_hold` 就 EOF）（`3fc8347`）；前两条 CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901），新补的：CI 没有见过 | — |
| 总设计第 1.5 节（FR-RULE-07） | 「QUIC 里的 SNI、跨几个包的 ClientHello…」 | 待读（任务 5） | | | |
| 总设计第 1.5 节（FR-RULE-07） | 「…按 SNI 匹配规则」 | 待读（任务 5） | | | |

### 阶段 1 ～ 3 推过来的

这张表只列没有并进别的表的句子；并进别的表的（阶段 1 对照第 80、162、410、417、421 行，阶段 2 验收第 527 行，阶段 3 对照第 250、251 行），出处写在那一行的「需求」一格里；同一句在几份文档里都推过来的（阶段 2 验收第 564 行、阶段 3 对照第 247 行），并进这张表里阶段 1 的那一行（M6 设计第 3.3 节）。

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| FR-CFG-12（阶段 1 对照第 126 行） | 「供…MITM `hostname` 等共用」 | 做了 | `crates/rurge-config/src/general.rs::parse_host_list`（`[MITM] hostname` 与 `force-http-engine-hosts` 等键共用的那一个）、`crates/rurge-config/src/mitm.rs::parse_mitm`（用它把 `hostname` 读成 `HostList`、缺省端口 443） | `crates/rurge-config/src/mitm.rs`：`the_nine_keys_are_read_and_default`（`hostname` 读出两项、`default_port` 是 443）、`hostname_disabled_removes_entries_written_the_same`；`crates/rurge-config/src/hostlist.rs`：`mitm_hostname_example_and_special_tokens`（手册的 `[MITM] hostname` 例子与四个特殊记号）、`force_http_engine_hosts_examples`（另一个用它的键、同一种读法）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| FR-CFG-14（阶段 1 对照第 131 行） | 「…Map Local 数据」 | 待读（任务 3） | | | |
| FR-RULE-02（阶段 1 对照第 204 行） | 「…参数」之 `extended-matching` | 做了 | `crates/rurge-rules/src/matcher.rs::domain_targets`（`extended` 时把 `sni` 与 `http_host` 加进域名目标）、`crates/rurge-rules/src/matcher.rs::Matcher::eval` | `crates/rurge-rules/src/matcher.rs`：`extended_matching_uses_sni_and_host`；`crates/rurge-engine/tests/sniff.rs`：`extended_matching_rules_a_tls_session_by_its_sni`、`extended_matching_rules_a_tls_session_by_its_sni_on_every_entry`、`without_extended_matching_the_sni_decides_nothing`；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M1/P1 |
| FR-RULE-02（阶段 1 对照第 210 行） | 「…参数」之 `always-capture` | 待读（任务 4） | 同「请求记录与抓包（FR-HTTP-10、11，FR-RULE-14）」的「「`always-capture=<session>`：强制抓包…」」一行 | 同上 | |
| FR-RULE-04（阶段 1 对照第 232 行；总设计第 1.5 节） | 「…文件格式约束」之行内可带 `extended-matching` | 做了 | `crates/rurge-rules/src/matcher.rs::Matcher::eval`（行内 `extended-matching` 让域名规则拿 SNI 与 `Host` 比） | `crates/rurge-rules/src/matcher.rs`：`extended_matching_uses_sni_and_host`；`crates/rurge-engine/tests/sniff.rs`：`extended_matching_rules_a_tls_session_by_its_sni`；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M1/P1 |
| FR-RULE-04（阶段 1 对照第 241 行） | 「…行级参数作用于整集」之 `extended-matching` | 做了 | `crates/rurge-rules/src/set.rs::CompiledSet::compile` | `crates/rurge-rules/src/set.rs`：`a_flag_on_one_domain_line_applies_to_every_domain_line`、`extended_matching_applies_to_domain_entries`、`without_a_flag_a_rule_set_ignores_the_sni`、`a_flag_on_a_rule_that_is_not_a_domain_rule_does_not_spread`；`crates/rurge-engine/tests/sniff.rs`：`a_flag_on_one_line_of_a_rule_set_matches_the_sni`（真实规则集文件里一行上的标志作用于整集）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| FR-RULE-07（阶段 1 对照第 257 行；阶段 2 验收第 564 行；总设计第 1.5 节） | 「`extended-matching`：从 TLS ClientHello 提取 SNI」 | 做了 | `crates/rurge-http/src/tls.rs::parse_client_hello`、`crates/rurge-http/src/sniff.rs::judge`、`crates/rurge-engine/src/serve.rs::serve_sniffed` | `crates/rurge-http/src/tls.rs`：`a_whole_hello_is_tls_with_its_sni_and_alpn`、`an_upper_case_sni_is_lowered`、`a_hello_over_several_records_is_put_together`、`a_hello_larger_than_a_record_is_read_to_its_end`；`crates/rurge-engine/tests/sniff.rs`：`extended_matching_rules_a_tls_session_by_its_sni`（规则之前就有 SNI）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M1/P1 |
| FR-RULE-07（阶段 1 对照第 258 行；阶段 3 对照第 247 行） | 「…从 HTTP/1.1 Host…提取主机名」 | 做了 | `crates/rurge-http/src/engine.rs::prepare`（`http_host` 取 `Host` 的主机、小写）、`crates/rurge-rules/src/matcher.rs::domain_targets` | `crates/rurge-http/src/engine.rs`：`the_session_of_a_request_says_what_the_rules_see`（`Host: Target.Test:8080` 而连接目标是 `target.test:80` 时，`url` 带着 8080 与原来的大小写、`http_host` 是小写的 `target.test`）；`crates/rurge-engine/tests/http.rs`：`every_entry_brings_its_requests_into_the_engine`（经虚拟网卡来的两个请求各一条记录，`listener` 是 TUN、`method` 是 `GET`、`url` 是 `http://target.test/a` 与 `/b`、`statusCode` 是 200）；`crates/rurge-rules/src/matcher.rs`：`extended_matching_uses_sni_and_host`（域名规则拿 `Host` 比）（三种入口走同一个 `prepare`，见 `docs/api/phase4.md`「M1b」的 `requests` 一节）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M1/W1、M1/W2、M1/M1、M1/M2、M1/L1、M1/L2 |
| FR-RULE-07（阶段 1 对照第 259 行） | 「…HTTP/2 `:authority`…提取主机名」 | 做了 | 同「会话入口与 HTTP 引擎（FR-HTTP-01、02）」的「「`:authority` 提取」」一行 | 同上 | M2/W3、M2/M3、M2/L3 |
| FR-RULE-08（阶段 1 对照第 261 行；阶段 2 验收第 564 行） | 「协议识别：HTTP」之 CONNECT 隧道或 SOCKS5 连接里的明文 HTTP | 做了 | `crates/rurge-engine/src/serve.rs::serve_sniffed`（判为 HTTP 时 `session.protocol = Some(ProtocolKind::Http)`） | `crates/rurge-engine/tests/sniff.rs`：`a_plain_http_session_is_marked_http`（`PROTOCOL,HTTP` 规则命中、协议记 `http`）；`crates/rurge-rules/src/matcher.rs`：`port_source_protocol_and_hostname_type_rules`（`PROTOCOL,HTTP` 对 HTTPS 会话不命中）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M1/W1、M1/W2、M1/M1、M1/M2、M1/L1、M1/L2 |
| FR-RULE-08（阶段 1 对照第 262 行） | 「…HTTPS（TLS）」 | 做了 | `crates/rurge-engine/src/serve.rs::serve_sniffed`（判为 TLS 时 `session.protocol = Some(ProtocolKind::Https)`） | `crates/rurge-engine/tests/sniff.rs`：`protocol_https_refuses_before_the_dial`（`PROTOCOL,HTTPS` 在拨号之前命中、协议记 `https`）；`crates/rurge-rules/src/matcher.rs`：`port_source_protocol_and_hostname_type_rules`；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M1/P1 |
| FR-CFG-11（阶段 2 验收第 574 行） | 「…被…`ca-keystore-name`…引用」 | 做了 | 同「MITM（FR-HTTP-03、04、14）」的「「`[MITM]` 九个键」之 `ca-keystore-name`」」一行 | 同上 | — |

### 非功能需求

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| NFR-01 | 「目标值（阶段 1 与阶段 3 建立基准后校准）」之经 HTTP 引擎的路径 | 待读（任务 5） | | | |
| NFR-03 | 「MITM CA 私钥与 Keystore 材料文件权限仅属主可读」之 Unix | 做了 | `crates/rurge/src/cli/mitm.rs::restrict`（`#[cfg(unix)]`：私密文件以 `0600` 建出） | `crates/rurge/tests/cli.rs`：`mitm_ca_generate_keeps_the_secrets_private`（`rurge-ca.p12` 与 `rurge-mitm.conf` 的权限位 `& 0o777` 是 `0600`；**只在 Linux 与 macOS 上编译**；CI：Linux 运行 41402787066、macOS 运行 41411445901 通过） | — |
| NFR-03 | 「MITM CA 私钥与 Keystore 材料文件权限仅属主可读」之 Windows | 有意不同（已登记） | `crates/rurge/src/cli/mitm.rs::restrict`（`#[cfg(not(unix))]` 是空操作，不另设 ACL；权限随文件所在目录）与 `crates/rurge/src/cli/mitm.rs::announce`（打印「rurge sets no permissions of its own on Windows, so keep them in a folder only you can read.」）；差异登记在 M2-D20 与兼容性清单 7.2 节「CA 生成」一行 | `crates/rurge/tests/cli.rs`：`mitm_ca_generate_writes_four_files_and_prints_no_secret`（跑在 Windows 上，`announce` 走 `cfg(not(unix))` 那一支；**2026-11-03 加了一条断言**：输出里有「rurge sets no permissions of its own on Windows」，在 Unix 上换成「they were made readable by you alone (0600)」（`f485306`）；Unix 那一支本机没有运行过）；**这一条改过，CI 没有见过**（改之前的样子 CI 三平台通过：Linux、Windows 运行 41402787066，macOS 运行 41411445901） | 看不出（Windows 上 rurge 不另设 ACL、权限完全随所在目录，rurge mitm ca generate 只印一句提醒；真机上没有一项手工操作能看出「仅属主可读」） |
| NFR-07 | 「平台特定代码隔离在 `rurge-platform` 与 `rurge-tun`」之阶段 4 加的 | 待读（任务 5） | | | |
| NFR-07 | 「CI 在三平台构建与测试」之阶段 4 的代码 | 待读（任务 5） | | | |
| NFR-11 | 「依赖许可证白名单（MIT / Apache-2.0 / BSD / ISC / MPL-2.0 等）」之阶段 4 用到的外部包 | 待读（任务 5） | | | |

### 数字

（任务 7 写）

### 没有做完的

| # | 需求（优先级） | 这一句 | 缺什么、使用者今天看到什么 | 去向 |
| --- | --- | --- | --- | --- |

### 需求文字的问题

| # | 需求（优先级） | 这一句 | 问题 | 去向 |
| --- | --- | --- | --- | --- |

### 有意不同的

| 需求 | 这一句 | 登记在 |
| --- | --- | --- |
| FR-OUT-01 | 「HTTP 一侧的 `REJECT-DROP` 看不到客户端提前关闭」 | M1 细化设计第 20 节第 8 条；兼容性清单 4.1 节 `REJECT-DROP` 一行 |
| NFR-03 | 「MITM CA 私钥与 Keystore 材料文件权限仅属主可读」之 Windows | M2-D20；兼容性清单 7.2 节「CA 生成」一行 |

### 本机没有能红的用例的

| 需求 | 这一句 | 代码与为什么本机没有能红的用例（它的 cfg，或缺省的用例走不到的那一层） | 要补的用例 |
| --- | --- | --- | --- |

## 第二部分：总设计第 15 节七条验收标准

（任务 7 写）

### 手册的重写示例与黄金用例（第 1 条）

| 页 | 示例（原文） | 用例 | 用例断言了什么 |
| --- | --- | --- | --- |
| http/url-rewrite.html | `^http://www\.google\.cn http://www.google.com header`、`^http://yachen\.com https://yach.me 302`、`^http://ad\.com/ad\.png _ reject` | | |
| http/url-rewrite.html | `^http://www\.google\.cn http://www.google.com header` | | |
| http/url-rewrite.html | `^http://yachen\.com https://yach.me 302` | | |
| http/url-rewrite.html | `^http://yachen\.com https://yach.me 307` | | |
| http/url-rewrite.html | `^http://ad\.com/ad\.png _ reject` | | |
| http/header-rewrite.html | `http-request ^http://example.com header-add DNT 1`、`http-request ^http://example.com header-del Cookie`、`http-request ^http://example.com header-replace User-Agent Unknown`、`http-response ^http://example.com header-replace-regex Date 2022 2023` | | |
| http/header-rewrite.html | `^http://example.com header-add DNT 1` | | |
| http/header-rewrite.html | `http-request ^http://example.com header-add DNT 1` | | |
| http/header-rewrite.html | `http-request ^http://example.com header-del DNT` | | |
| http/header-rewrite.html | `http-request ^http://example.com header-replace DNT 1` | | |
| http/header-rewrite.html | `^http://example.com header-del DNT`、`^http://example.com header-add DNT 1` | | |
| http/header-rewrite.html | `http-request ^http://example.com header-replace-regex User-Agent Safari Chrome` | | |
| http/body-rewrite.html | `http-request ^http(s)?://example\.com value abc`、`http-response ^http(s)?://example\.com documents Surge` | | |
| http/body-rewrite.html | `http-response ^https?://example\.com/ regex replacement` | | |
| http/body-rewrite.html | `http-response ^https?://example\.com/ regex1 replacement1 regex2 replacement2`、`http-response ^https?://example\.com/ regex1 replacement1 regex2 replacement2 regex3 replacement3`、`…` | | |
| http/body-rewrite.html | `http-request-jq url-pattern jq-expression`、`http-response-jq url-pattern jq-expression` | | |
| http/body-rewrite.html | `http-response-jq ^http://httpbingo.org/anything '.headers \|= with_entries(select(.key \| test("^X-") \| not))'` | | |
| http/map-local.html | `^http://surgetest\.com/json data-type=text data="{}" status-code=500`、`^http://surgetest\.com/gif data-type=tiny-gif status-code=200`、`^http://surgetest\.com/file data-type=file data="data/map-local.json" header="a:b\|foo:bar"`、`^http://surgetest\.com/base64 data="dGVzdA==" data-type=base64` | | |
| http/url-rewrite.html | "Besides the request-line URL, Surge also tries matching with the URL rebuilt from the `Host` header value and from the underlying connection's hostname." | | |
| http/header-rewrite.html | "Header Rewrite cannot change the message framing. If a rewrite changes the `Content-Length` or `Transfer-Encoding` headers, the modification is rejected and the request fails." | | |
| http/body-rewrite.html | "The replacement supports capture group references such as `$1`." | | |
| http/body-rewrite.html | "The `^` and `$` anchors match at the beginning and end of each line of the body." | | |
| http/body-rewrite.html | "Even if the original request does not contain a body, new content may still be generated through body rewrite, such as using the `^$` expression." | | |
| http/body-rewrite.html | "The body must be valid UTF-8 text for regex-based rewriting." | | |
| http/map-local.html | "You can use `data-type=text data=""` to return an empty result." | | |

## 靠手工验收的

（任务 7 写）

## 靠 CI 的

（任务 7 写）

## 留给项目所有者的

（任务 7 写）
