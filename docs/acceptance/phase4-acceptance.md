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
| FR-HTTP-02 | 「HTTP/2：MITM 后与客户端…使用 HTTP/2（`h2=true`）」 | 待读（任务 2） | | | |
| FR-HTTP-02 | 「…/ 上游使用 HTTP/2…」 | 待读（任务 2） | | | |
| FR-HTTP-02 | 「`:authority` 提取」 | 待读（任务 2） | | | |

### MITM（FR-HTTP-03、04、14）

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| FR-HTTP-03 | 「`[MITM]` 九个键」之 `hostname` | 待读（任务 2） | | | |
| FR-HTTP-03 | 「`[MITM]` 九个键」之 `hostname-disabled` | 待读（任务 2） | | | |
| FR-HTTP-03 | 「`[MITM]` 九个键」之 `ca-p12` | 待读（任务 2） | | | |
| FR-HTTP-03 | 「`[MITM]` 九个键」之 `ca-passphrase` | 待读（任务 2） | | | |
| FR-HTTP-03 | 「`[MITM]` 九个键」之 `ca-keystore-name` | 待读（任务 2） | | | |
| FR-HTTP-03 | 「`[MITM]` 九个键」之 `skip-server-cert-verify` | 待读（任务 2） | | | |
| FR-HTTP-03 | 「`[MITM]` 九个键」之 `h2` | 待读（任务 2） | 同「会话入口与 HTTP 引擎（FR-HTTP-01、02）」的「「HTTP/2：MITM 后与客户端…使用 HTTP/2（`h2=true`）」」一行 | 同上 | |
| FR-HTTP-03 | 「`[MITM]` 九个键」之 `client-source-address` | 待读（任务 2） | | | |
| FR-HTTP-03 | 「`[MITM]` 九个键」之 `auto-quic-block` | 待读（任务 5） | 同「MITM（FR-HTTP-03、04、14）」的「「命中 MITM 列表时阻断 UDP 使客户端回落」」一行 | 同上 | |
| FR-HTTP-03 | 「按主机名签发…叶子证书」 | 待读（任务 2） | | | |
| FR-HTTP-03 | 「…并缓存叶子证书」 | 待读（任务 2） | | | |
| FR-HTTP-03 | 「靠嗅探到的 SNI 认出的 TLS…连接，不带端口的项不看端口」 | 待读（任务 2） | | | |
| FR-HTTP-03 | 「…与 QUIC 连接，不带端口的项不看端口」 | 待读（任务 5） | | | |
| FR-HTTP-03 | 「`client-source-address`」 | 待读（任务 2） | 同「MITM（FR-HTTP-03、04、14）」的「「`[MITM]` 九个键」之 `client-source-address`」」一行 | 同上 | |
| FR-HTTP-03 | 「`skip-server-cert-verify`」 | 待读（任务 2） | 同「MITM（FR-HTTP-03、04、14）」的「「`[MITM]` 九个键」之 `skip-server-cert-verify`」」一行 | 同上 | |
| FR-HTTP-03 | 「证书固定提示日志」 | 待读（任务 2） | | | |
| FR-HTTP-03 | 「CA 通过 `ca-p12`…提供」 | 待读（任务 2） | 同「MITM（FR-HTTP-03、04、14）」的「「`[MITM]` 九个键」之 `ca-p12`」」一行 | 同上 | |
| FR-HTTP-03 | 「…/ `ca-keystore-name` 提供」 | 待读（任务 2） | 同「MITM（FR-HTTP-03、04、14）」的「「`[MITM]` 九个键」之 `ca-keystore-name`」」一行 | 同上 | |
| FR-HTTP-03 | 「`rurge mitm ca generate` 生成 CA」 | 待读（任务 2） | | | |
| FR-HTTP-03 | 「…并给出各平台信任安装指引」 | 待读（任务 2） | | | |
| FR-HTTP-03 | 「`GET /v1/mitm/ca` 导出 DER」 | 待读（任务 2） | | | |
| FR-HTTP-04 | 「`auto-quic-block`：解析 QUIC Initial 的 SNI」 | 待读（任务 5） | | | |
| FR-HTTP-04 | 「命中 MITM 列表时阻断 UDP 使客户端回落」 | 待读（任务 5） | | | |
| FR-HTTP-14 | 「`http-api-tls`：API 使用 MITM CA 签发的证书提供 HTTPS」 | 待读（任务 2） | | | |

### 重写与 Map Local（FR-HTTP-05 ～ 09）

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| FR-HTTP-05 | 「URL Rewrite：`header`」 | 待读（任务 3） | | | |
| FR-HTTP-05 | 「…`302`」 | 待读（任务 3） | | | |
| FR-HTTP-05 | 「…`307`」 | 待读（任务 3） | | | |
| FR-HTTP-05 | 「…`reject`」 | 待读（任务 3） | | | |
| FR-HTTP-05 | 「首个 header 规则生效」 | 待读（任务 3） | | | |
| FR-HTTP-05 | 「结果 URL 合法性」 | 待读（任务 3） | | | |
| FR-HTTP-05 | 「`{{{GATEWAY_ADDRESS}}}`」 | 待读（任务 3） | | | |
| FR-HTTP-05 | 「对 Host…重建的 URL 也匹配」 | 待读（任务 3） | | | |
| FR-HTTP-05 | 「对…连接主机名重建的 URL 也匹配」 | 待读（任务 3） | | | |
| FR-HTTP-06 | 「Header Rewrite：方向」 | 待读（任务 3） | | | |
| FR-HTTP-06 | 「四种动作」之 `header-add` | 待读（任务 3） | | | |
| FR-HTTP-06 | 「四种动作」之 `header-del` | 待读（任务 3） | | | |
| FR-HTTP-06 | 「四种动作」之 `header-replace` | 待读（任务 3） | | | |
| FR-HTTP-06 | 「四种动作」之 `header-replace-regex` | 待读（任务 3） | | | |
| FR-HTTP-06 | 「顺序生效」 | 待读（任务 3） | | | |
| FR-HTTP-06 | 「改动消息框架头则拒绝…」 | 待读（任务 3） | | | |
| FR-HTTP-06 | 「…并使请求失败」 | 待读（任务 3） | | | |
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
| FR-HTTP-13 | 「功能开关 `mitm`…运行时切换并持久化」 | 待读（任务 2） | | | |
| FR-HTTP-13 | 「…`rewrite`…」 | 待读（任务 3） | | | |
| FR-HTTP-13 | 「…`capture`…」 | 待读（任务 4） | | | |

### 配置与观测（FR-CFG-06、FR-CFG-18、FR-OBS-02）

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| FR-CFG-06（兼容性清单 2.1 节；阶段 1 对照第 80 行） | 「…各键随所属功能阶段生效」之 `http-api-tls` | 待读（任务 2） | 同「MITM（FR-HTTP-03、04、14）」的「「`http-api-tls`：API 使用 MITM CA 签发的证书提供 HTTPS」」一行 | 同上 | |
| FR-CFG-06（兼容性清单 2.1 节） | 「…各键随所属功能阶段生效」之 `force-http-engine-hosts` | 做了 | 同「会话入口与 HTTP 引擎（FR-HTTP-01、02）」的「「`force-http-engine-hosts`」」一行 | 同上 | M1/P3 |
| FR-CFG-06（兼容性清单 2.1 节） | 「…各键随所属功能阶段生效」之 `always-raw-tcp-hosts` | 做了 | 同「会话入口与 HTTP 引擎（FR-HTTP-01、02）」的「「`always-raw-tcp-hosts`」」一行 | 同上 | M1/P3 |
| FR-CFG-06（兼容性清单 2.1 节） | 「…各键随所属功能阶段生效」之 `always-raw-tcp-keywords` | 做了 | 同「会话入口与 HTTP 引擎（FR-HTTP-01、02）」的「「`always-raw-tcp-keywords`」」一行 | 同上 | M1/P3 |
| FR-CFG-06（兼容性清单 2.1 节） | 「…各键随所属功能阶段生效」之 `show-error-page`（经引擎的请求上） | 做了 | `crates/rurge-engine/src/http.rs::ConnUpstreams::route`（把 `Pages { error, reject }` 交给引擎）、`crates/rurge-http/src/engine.rs::failed`、`crates/rurge-http/src/engine.rs::timed_out` | `crates/rurge-engine/tests/http.rs`：`the_listener_answers_refusals_and_failures_as_the_profile_says`（失败答 502 页）、`refusals_are_answered_on_every_entry`（四种入口）、`a_late_response_head_is_a_504`（等应答头超时答 504 页）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| FR-CFG-06（兼容性清单 2.1 节） | 「…各键随所属功能阶段生效」之 `show-error-page-for-reject`（经引擎的请求上） | 做了 | `crates/rurge-engine/src/http.rs::ConnUpstreams::route`、`crates/rurge-http/src/engine.rs::answer` | `crates/rurge-engine/tests/http.rs`：`refusals_are_answered_on_every_entry`（`REJECT` 开着答 403 页、关着不答就关；`REJECT-TINYGIF` 不看这个设置答 GIF）、`the_listener_answers_refusals_and_failures_as_the_profile_says`；`crates/rurge-engine/tests/pipeline.rs`：`reject_rules_close_serve_gifs_or_render_pages`；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| FR-CFG-18（阶段 1 对照第 162 行） | 「Header Rewrite 省略方向」 | 待读（任务 3） | | | |
| FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 4 / M1a 加的（`docs/api/phase4.md` 的「M1a」一节） | 做了 | `crates/rurge-api/src/routes/rurge_http.rs::http`、`crates/rurge-engine/src/serve.rs::SniffCounts` | `crates/rurge-api/tests/api.rs`：`rurge_http_counts_start_at_zero`（`sniffed` 五个键按次序、零与 401）；`crates/rurge-engine/tests/sniff.rs`：`the_sniffs_are_counted`（五种结局各记一次）、`a_stop_during_the_wait_ends_a_tun_session_at_once`（被停止的嗅探不计；`f9fc0ba` 补了计数断言）、`which_sessions_are_sniffable`（可嗅探的条件）、`a_server_first_protocol_passes_after_the_wait`（等满时限之后照原样转发、记录不带协议；用例把时限调到 300 毫秒；缺省的 15 秒没有用例等满它，见 `docs/api/phase4.md`「M1a」的「没有覆盖的」第 2 条）、`a_plain_http_session_is_marked_http`、`neither_http_nor_tls_is_noted`、`a_sniff_that_waits_out_its_time_is_noted`、`a_client_that_closes_during_the_wait_has_no_note`；`crates/rurge-engine/tests/http.rs`：`a_tls_connection_is_still_relayed_as_it_is`（记录的 `sni` 与 `protocol=https`）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901），`f9fc0ba` 的计数断言：CI 没有见过 | — |
| FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 4 / M1b 加的（`docs/api/phase4.md` 的「M1b」一节） | 做了 | `crates/rurge-api/src/routes/rurge_http.rs::http`（`requests`）、`crates/rurge-engine/src/http.rs::ConnUpstreams::route`（计数）、`crates/rurge-api/src/routes/requests.rs::RequestJson`（`method` / `url` / `statusCode`）、`crates/rurge-engine/src/observe.rs::record_of` | `crates/rurge-engine/tests/http.rs`：`the_requests_are_counted`（拒绝也算）、`a_connection_that_never_reaches_the_rules_leaves_one_failed_record`、`a_connection_whose_requests_were_routed_leaves_no_record_of_its_own`、`the_carrier_connection_is_recorded_only_when_nothing_was_routed`（承载连接的记录）、`an_unknown_fake_address_in_a_plain_request_is_a_502`（先求规则、`statusCode=502`、`requests=1`；`f9fc0ba`、`d40ddcf` 补）、`a_request_record_has_its_method_url_and_status`（200 / 403 / `null`）、`a_late_response_head_is_a_504`（504 / `null`）、`a_url_with_userinfo_is_recorded_without_it`、`a_borrowed_connection_records_where_it_went`（借来的 `remoteAddr`、`connectMs`、`firstByteMs`；`f9fc0ba` 补）、`each_request_counts_its_own_bytes`（`up` / `down`、借来的连接）、`the_request_log_line_shows_no_query_and_no_userinfo`（日志）；`crates/rurge-http/src/engine.rs`：`the_session_of_a_request_says_what_the_rules_see`（`url` 由 `Host` 拼出；嗅探到的 `CONNECT` 是 `null`）、`a_request_without_host_is_named_by_its_target`（没有 `Host` 时用连接的目标拼）；`crates/rurge-api/tests/api.rs`：`requests_carry_the_method_the_url_and_the_status_code`（键序与 `null`）、`rurge_http_counts_start_at_zero`；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901），`f9fc0ba` 与 `d40ddcf` 的补：CI 没有见过 | M1/W1、M1/W2、M1/M1、M1/M2、M1/L1、M1/L2 |
| FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 4 / M2a 加的（`docs/api/phase4.md` 的「M2a」一节） | 待读（任务 2） | | | |
| FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 4 / M2b 加的（`docs/api/phase4.md` 的「M2b」一节） | 待读（任务 2） | | | |
| FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 4 / M2c 加的（`docs/api/phase4.md` 的「M2c」一节） | 待读（任务 2） | | | |
| FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 4 / M3a 加的（`docs/api/phase4.md` 的「M3a」一节） | 待读（任务 3） | | | |
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
| FR-CFG-12（阶段 1 对照第 126 行） | 「供…MITM `hostname` 等共用」 | 待读（任务 2） | | | |
| FR-CFG-14（阶段 1 对照第 131 行） | 「…Map Local 数据」 | 待读（任务 3） | | | |
| FR-RULE-02（阶段 1 对照第 204 行） | 「…参数」之 `extended-matching` | 做了 | `crates/rurge-rules/src/matcher.rs::domain_targets`（`extended` 时把 `sni` 与 `http_host` 加进域名目标）、`crates/rurge-rules/src/matcher.rs::Matcher::eval` | `crates/rurge-rules/src/matcher.rs`：`extended_matching_uses_sni_and_host`；`crates/rurge-engine/tests/sniff.rs`：`extended_matching_rules_a_tls_session_by_its_sni`、`extended_matching_rules_a_tls_session_by_its_sni_on_every_entry`、`without_extended_matching_the_sni_decides_nothing`；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M1/P1 |
| FR-RULE-02（阶段 1 对照第 210 行） | 「…参数」之 `always-capture` | 待读（任务 4） | 同「请求记录与抓包（FR-HTTP-10、11，FR-RULE-14）」的「「`always-capture=<session>`：强制抓包…」」一行 | 同上 | |
| FR-RULE-04（阶段 1 对照第 232 行；总设计第 1.5 节） | 「…文件格式约束」之行内可带 `extended-matching` | 做了 | `crates/rurge-rules/src/matcher.rs::Matcher::eval`（行内 `extended-matching` 让域名规则拿 SNI 与 `Host` 比） | `crates/rurge-rules/src/matcher.rs`：`extended_matching_uses_sni_and_host`；`crates/rurge-engine/tests/sniff.rs`：`extended_matching_rules_a_tls_session_by_its_sni`；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M1/P1 |
| FR-RULE-04（阶段 1 对照第 241 行） | 「…行级参数作用于整集」之 `extended-matching` | 做了 | `crates/rurge-rules/src/set.rs::CompiledSet::compile` | `crates/rurge-rules/src/set.rs`：`a_flag_on_one_domain_line_applies_to_every_domain_line`、`extended_matching_applies_to_domain_entries`、`without_a_flag_a_rule_set_ignores_the_sni`、`a_flag_on_a_rule_that_is_not_a_domain_rule_does_not_spread`；`crates/rurge-engine/tests/sniff.rs`：`a_flag_on_one_line_of_a_rule_set_matches_the_sni`（真实规则集文件里一行上的标志作用于整集）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | — |
| FR-RULE-07（阶段 1 对照第 257 行；阶段 2 验收第 564 行；总设计第 1.5 节） | 「`extended-matching`：从 TLS ClientHello 提取 SNI」 | 做了 | `crates/rurge-http/src/tls.rs::parse_client_hello`、`crates/rurge-http/src/sniff.rs::judge`、`crates/rurge-engine/src/serve.rs::serve_sniffed` | `crates/rurge-http/src/tls.rs`：`a_whole_hello_is_tls_with_its_sni_and_alpn`、`an_upper_case_sni_is_lowered`、`a_hello_over_several_records_is_put_together`、`a_hello_larger_than_a_record_is_read_to_its_end`；`crates/rurge-engine/tests/sniff.rs`：`extended_matching_rules_a_tls_session_by_its_sni`（规则之前就有 SNI）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M1/P1 |
| FR-RULE-07（阶段 1 对照第 258 行；阶段 3 对照第 247 行） | 「…从 HTTP/1.1 Host…提取主机名」 | 做了 | `crates/rurge-http/src/engine.rs::prepare`（`http_host` 取 `Host` 的主机、小写）、`crates/rurge-rules/src/matcher.rs::domain_targets` | `crates/rurge-http/src/engine.rs`：`the_session_of_a_request_says_what_the_rules_see`（`Host: Target.Test:8080` 而连接目标是 `target.test:80` 时，`url` 带着 8080 与原来的大小写、`http_host` 是小写的 `target.test`）；`crates/rurge-engine/tests/http.rs`：`every_entry_brings_its_requests_into_the_engine`（经虚拟网卡来的两个请求各一条记录，`listener` 是 TUN、`method` 是 `GET`、`url` 是 `http://target.test/a` 与 `/b`、`statusCode` 是 200）；`crates/rurge-rules/src/matcher.rs`：`extended_matching_uses_sni_and_host`（域名规则拿 `Host` 比）（三种入口走同一个 `prepare`，见 `docs/api/phase4.md`「M1b」的 `requests` 一节）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M1/W1、M1/W2、M1/M1、M1/M2、M1/L1、M1/L2 |
| FR-RULE-07（阶段 1 对照第 259 行） | 「…HTTP/2 `:authority`…提取主机名」 | 待读（任务 2） | 同「会话入口与 HTTP 引擎（FR-HTTP-01、02）」的「「`:authority` 提取」」一行 | 同上 | |
| FR-RULE-08（阶段 1 对照第 261 行；阶段 2 验收第 564 行） | 「协议识别：HTTP」之 CONNECT 隧道或 SOCKS5 连接里的明文 HTTP | 做了 | `crates/rurge-engine/src/serve.rs::serve_sniffed`（判为 HTTP 时 `session.protocol = Some(ProtocolKind::Http)`） | `crates/rurge-engine/tests/sniff.rs`：`a_plain_http_session_is_marked_http`（`PROTOCOL,HTTP` 规则命中、协议记 `http`）；`crates/rurge-rules/src/matcher.rs`：`port_source_protocol_and_hostname_type_rules`（`PROTOCOL,HTTP` 对 HTTPS 会话不命中）；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M1/W1、M1/W2、M1/M1、M1/M2、M1/L1、M1/L2 |
| FR-RULE-08（阶段 1 对照第 262 行） | 「…HTTPS（TLS）」 | 做了 | `crates/rurge-engine/src/serve.rs::serve_sniffed`（判为 TLS 时 `session.protocol = Some(ProtocolKind::Https)`） | `crates/rurge-engine/tests/sniff.rs`：`protocol_https_refuses_before_the_dial`（`PROTOCOL,HTTPS` 在拨号之前命中、协议记 `https`）；`crates/rurge-rules/src/matcher.rs`：`port_source_protocol_and_hostname_type_rules`；CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901） | M1/P1 |
| FR-CFG-11（阶段 2 验收第 574 行） | 「…被…`ca-keystore-name`…引用」 | 待读（任务 2） | 同「MITM（FR-HTTP-03、04、14）」的「「`[MITM]` 九个键」之 `ca-keystore-name`」」一行 | 同上 | |

### 非功能需求

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| NFR-01 | 「目标值（阶段 1 与阶段 3 建立基准后校准）」之经 HTTP 引擎的路径 | 待读（任务 5） | | | |
| NFR-03 | 「MITM CA 私钥与 Keystore 材料文件权限仅属主可读」之 Unix | 待读（任务 2） | | | |
| NFR-03 | 「MITM CA 私钥与 Keystore 材料文件权限仅属主可读」之 Windows | 待读（任务 2） | | | |
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
