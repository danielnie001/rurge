# 阶段 4 / M2b「MITM：HTTP/2」实施计划

> **执行方式**：交给 Pi，由协调的会话逐个任务派工、逐个任务评审（设计第 2 节、M2-D1；做法同 M1a、M1b、M2a）。
>
> - 协调的会话每个任务开一个 Pi 会话，交给它这个任务的全文与要用的接口（派工文件）。Pi 做完把报告写进派工文件指定的路径，在本地提交。
> - 协调者读改动、重跑门禁、评审。有问题回到同一个 Pi 会话修正，再评审；过了才派下一个任务。
> - 每个任务：先写失败的用例、看到红；再实现到绿；跑门禁；对照本任务的「判据」与「全局约束」逐条自查；最后提交（**只在本地，不推送**）。钉住现有行为的用例（任务里写明“不要求先红”的）可以一写就绿。
> - 与计划的出入记进末尾的「执行期修正记录」，**不改计划的正文**。
>
> 没有等 CI 的停点：M2b 没有平台代码。M2 的三份计划都做完之后由项目所有者推送一次（连同 M1），协调者读回那次 CI（M2-D1）。

**目标**：`[MITM] h2 = true` 生效。解密的连接上，客户端用 ALPN 选了 `h2` 的，rurge 用 hyper 的 HTTP/2 服务端解密它的请求；对上游的 TLS 同时给 `h2` 与 `http/1.1`，选了 `h2` 的上游连接在同一条客户端连接的同一个键下共享给并发的请求。两侧的版本可以不同，请求在两种形式之间转换。HTTP/1.1 上"关掉整条连接"的终止、看门狗、拒绝与停止，在 HTTP/2 上改成"重置这个流"。具体是：

- `rurge-http`：
  - 对客户端：HTTP/2 的服务端（跟踪器上的执行器、100 个流、不发 PING、空闲之后 GOAWAY、GOAWAY 之后有时限）、HTTP/2 上的 `CONNECT` 答 501、流被客户端取消时记录的原因；
  - 版本转换（新文件 `version.rs`）：`:authority` 与 `Host`、`cookie` 合并、连接专用的头、`TE: trailers`；
  - 对上游：`Sender`（HTTP/1.1 或 HTTP/2）、同一个键至多一条共享的 HTTP/2 连接、HTTP/2 上游只记正文的字节、升级与隧道只拨 HTTP/1.1、被上游拒绝的流再发一次；
  - HTTP/2 上的终止与看门狗：放掉这个请求的上游，不关整条客户端连接；
  - `Mitm`：ALPN 认 `h2`、对客户端与对上游的配置随 `h2`、`Accepted` 带出协商到的协议。
- 引擎：把 `Accepted` 的协议交给 `Conn`。
- 端到端用例；文档收尾（M2b 的部分）。

**架构**：没有平台代码，全部在开发机上编译、运行。

- 依赖方向不加新的工作区边。`rurge-http` 加一个外部依赖 `h2`（锁文件里已有，hyper 与 `rurge-proto` 都用它），只用来认出被拒的流（P13）；`rurge-engine` 的开发依赖加 hyper、hyper-util、http-body-util（工作区已有）。
- `rurge-http` 照旧不认识引擎：版本、流、共享的上游连接都在 `rurge-http` 里；引擎只多交一个布尔（客户端一侧是不是 HTTP/2）。
- 共享的 HTTP/2 上游连接与 M1b 的池一样属于一条客户端连接（`Shared`），不跨客户端连接共享（设计第 10.2 节、M2-D15）。

**技术栈**：都已在 `Cargo.lock` 里、源码在本机 cargo 注册表里。**不下载、不加新包**（M2-D2）。

- hyper 1.11.1：`server::conn::http2::Builder`、`client::conn::http2::{Builder, SendRequest}`、`rt::Executor`；hyper-util 0.1：`rt::{TokioIo, TokioTimer, TokioExecutor}`。
- h2 0.4.19：`Error::{reason, is_go_away, is_remote}`、`Reason::REFUSED_STREAM`；用例里另用它的客户端与服务端。
- rustls 0.23.43、tokio-rustls 0.26.4：ALPN 的协商与读回（`alpn_protocol()`）。

**依据**：

- M2 细化设计（下面写"设计"指它）：`docs/superpowers/specs/2026-10-30-phase4-m2-mitm-design.md`。
  - M2b 用到第 10 节、第 16 节第 10 条、第 17 节第 5 条、第 18 节与已决事项 M2-D14 ～ D17。
  - **先读它第 23 节**：第 15 ～ 19 条是本计划改了设计正文的地方（随本计划提交）。
  - 设计与本计划的「计划期决定」不一致的，以计划为准。
- M1b 计划的「计划期决定」（守卫、看门狗、重发的规矩）与 M2a 计划的「计划期决定」、两份计划的执行期修正记录：解密的连接跑在 M1b 的引擎上，规矩照旧。
- RFC 9113（HTTP/2）第 8.2.3 节（`cookie`）、第 8.3.1 节（`:authority` 与 `Host`）、第 8.5 节（`CONNECT`）、第 8.7 节（被拒的请求可以再发）。
- 写计划时对着 `a9e759b`（M2a 合并之后的 `phase4-m2`）核对过的事实写在「计划期决定」里，外部库的事实对着注册表里锁定版本的源码。

## 全局约束

**依赖**

- `rurge-http`：
  - 任务 1 在 `[dev-dependencies]` 加 `h2.workspace = true`（用例要 h2 的客户端读服务端的 SETTINGS）；
  - 任务 2 把它挪到 `[dependencies]`（P13）。
- `rurge-engine`：任务 5 在 `[dev-dependencies]` 加 `hyper.workspace = true`、`hyper-util.workspace = true`、`http-body-util.workspace = true`。
- 不加外部包。`Cargo.lock` 的 `[[package]]` 今天 520 个，每个任务之后照旧 520 个；锁文件里只有 `rurge-http` 与 `rurge-engine` 两项的依赖列表可以变。改了 `Cargo.toml` 之后第一次编译用 `cargo check --offline --workspace --all-targets`：它失败就停下报告——那说明要下载，不许。
- 依赖方向：`rurge-tun`、`rurge-inbound`、`rurge-api`、`rurge-config` 不依赖 `rurge-http`；`rurge-http` 不依赖 `rurge-engine`。
- `unsafe_code`：不加放开的函数，`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 仍是 23 项。

**缺省的 `cargo test` 不碰系统**

- 同 M2a：不建网卡、不改系统的任何设置；不执行 `netsh`、`route`、`ipconfig`、`ip`、`resolvectl`、`networksetup`。
- **测试从不把任何 CA 装进系统或浏览器的信任库**（总设计 P4-D26、M2-D24）。测试的客户端是 rustls 或 hyper / h2 的客户端；源站是回环上的测试服务器，证书来自测试自己的 CA（`TlsFixture`），引擎经 `EngineShared.roots` 信任它。

**测试的约定**

`CLAUDE.md`「测试约定」整节适用。写经引擎的用例之前读 `docs/testing.md`「测试约定（全文）」的阶段 4 各节（M1a、M1b、M2a）。与本计划直接相关的：

- 只用回环与端口 0。经引擎的用例照 M2a：会话的目标是 443 时，规则把它送到一个 SOCKS5 策略（`FakeSocks5`，`connect_to` 指向回环源站）；写了主机名的 Profile 另给第二道防线（`[Host]` 钉到 `127.0.0.1`，另加一条按地址的规则）。
- `rurge-http` 的用例里，客户端一侧的 HTTP/2 不经 TLS：`serve_connection` 拿到的已经是解密之后的流，用例在 `tokio::io::duplex` 上直接讲 HTTP/2（hyper 的 `client::conn::http2`，执行器是 hyper-util 的 `TokioExecutor`）。
- 等待都有时限。固定等待只许"断言什么都没发生"，注释写它等的是什么、余量多大。
- 新写的、带等待的用例提交前单独循环跑二十次以上（任务 1、2、3、5）。
- 抓日志用 `LogCapture`，跑在当前线程的运行时上。
- 测试里手写的 HTTP/1.1 客户端把每个请求——请求头与正文——在一次写里发出（`CLAUDE.md`）。
- **内联的测试模块放在文件的末尾**：`crates/rurge/tests/sockets.rs` 读生产代码，读到第一个 `#[cfg(test)] mod … {` 为止。

**门禁与这台开发机**（每个任务末尾一次；说"通过"之前必跑）

- 门禁：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`。
- 本机 msvc 工具链没有 rustfmt。跑 `cargo fmt` 之前让环境变量 `RUSTFMT` 指向 gnu 工具链里同版本的 rustfmt：用户目录下的 `.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe`。不要为此安装或升级工具链。
- 聚焦地跑用例也经整个工作区：单元测试 `cargo test -j 8 --workspace --lib -- <过滤词>`；集成测试 `cargo test -j 8 --workspace --test <名字> -- <过滤词>`。
- **不用 `-p`**：它另编一份依赖，`target/` 涨得很快，写满过 D: 盘。
- 跑全量之前 `df -h /d`：剩余不足约 15 GiB 就停下报告。
- 测试二进制以 `STATUS_HEAP_CORRUPTION`（本机管控软件注入的钩子 DLL）或 `STATUS_ACCESS_VIOLATION` 退出：单独重跑那一个二进制，报告里写明。`cli` 的用例偶发 `give_up` 20 秒超时：单独重跑 `cli`，把标准错误原样抄进报告。
- rustc 在增量编译里报内部编译器错误：删掉出错的那个目标在 `target/debug/incremental/` 下的目录再编。
- 回环的 TCP / UDP 7、9、13、17、19 号端口在这台机器上开着，CI 上关着：用例不假定任何固定端口的状态。
- 结果照实记：本机的全量运行是"一台机器上的一次运行，不是 CI"；需要参照服务端的互操作用例在本机立刻返回、算在通过的数里，要说出来。

**仓库的规矩**（`CLAUDE.md`「必须遵守的规矩」整节适用；与本计划直接相关的）

- 凭据：请求与应答的头和正文不进日志（同 M1b）；HTTP/2 的错误只按原因写，不带头的内容。
- 日志：新的行只在 DEBUG（被拒的流再发、GOAWAY 之后的时限到了）；hyper 自己的日志在 rurge 里是空操作（没开 `tracing` 特性），h2 的 WARN 只有四处、都不带内容（`m2b` 核对的事实，P3）。
- 出站套接字只有一处建法：对上游的连接一律经 `Upstreams::dial` 建。`rurge-http` 只在拨好的流上做 TLS、HTTP/1.1 与 HTTP/2，不建套接字、不问系统解析库、不建连接器。改动让 `crates/rurge/tests/sockets.rs` 变红就停下报告。
- `rurge-tun` 不改。M1b 的规矩照旧：停止取消正在转发的会话，不取消正在进行的拨号；结束一个请求靠它的令牌与守卫，不靠丢掉 future。
- 文档与报告用写文件 / 改文件的工具改。**永远不要用把一个文件的内容经管道写回同一个文件的命令**（2026-10-27 这样写满过 D: 盘）。
- 计划末尾两张表里已写下的行不改写、不删除；追加新行时**不要把已有的行当成锚点替换掉**（M2a 任务 9 这样删掉过一行，修正记录第 60 行）。

**写法**

- 注释、日志与错误文本用英文，照所在文件的写法：文档注释写"为什么"与前提，句子完整。注释里提到设计或计划，写 `phase 4 M2 design 10.2`、`the phase 4 / M2b plan's P10`。
- 提交说明照 `git log` 的样子写，标题用中文：`feat(http): …`、`feat(inbound): …`、`feat(engine): …`、`test(engine): …`、`docs: …`。正文说做了什么、改了哪些现有用例的期望、为什么。Pi 的提交不带署名行。
- 只在本地提交：不 `push`、不 `pull`、不 `fetch` 远端、不 `amend`、不改写历史。

**不做的事**

- 只运行本仓库构建出来的二进制与 Pi 自己。不下载、不安装任何东西。
- 不手动跑 `rurge run`、`rurge dns lookup`；`rurge service` 只用 `--dry-run`。
- 参考实现：GPL 的（sing-box、mihomo、Xray-core）只取事实，代码不抄。RFC 是公开的事实。
- 不做设计第 1 节「不做」一栏的事，也不做 M2c 的事（`rurge mitm ca generate` / `export`、`http-api-tls`、`W0040`、手工清单），也不做：
  - 明文的 HTTP/2（h2c，不论升级还是先验知识）：明文的连接照 M1b 只讲 HTTP/1.1；
  - 扩展 `CONNECT`（RFC 8441，HTTP/2 上的 WebSocket）：不宣告（总设计 P4-D11）；
  - HTTP/2 的服务端推送、优先级；对上游的 PING 保活；
  - 共享的 HTTP/2 上游连接跨客户端连接。

**计划没有写到、要做决定的地方**

- 能按设计与总设计的原则定的：自己定，记进修正记录，写"执行者的裁定"。
- 去向写着"项目所有者"的（设计第 22 节），或会改变 M2-D1 ～ D25 的：停下来列出，交给协调者。
- 现有用例变红：P17 的表点了名的照计划改，在提交说明里说；没点名的停下来，记进修正记录，不顺手改期望值。

## 评审要点

设计没写透、而最可能咬到使用者的几种输入，与负责它们的任务和用例：

1. **共享的上游连接走掉了**：源站发 GOAWAY、关掉连接，或拒绝一个流（`REFUSED_STREAM`）。
   - 期望：走掉的连接不再借出；被拒的、没有正文的幂等请求在新拨的连接上再发一次，别的照 M1b 答 502；不出现"一个请求撞上刚走掉的连接就失败"。
   - 用例（任务 2）：`an_h2_connection_that_went_away_is_not_lent_again`、`a_request_the_origin_refuses_is_sent_again_once`。
2. **一个流出事拖垮整条连接**：客户端取消一个流、终止一个请求、看门狗到时、`REJECT` 没有页、`REJECT-DROP`。
   - 期望：只那一个流被重置，同一条连接上别的流与之后的请求照常。
   - 用例（任务 1、3）：`a_stream_the_client_resets_is_cancelled_and_the_connection_stays`、`a_killed_h2_stream_is_reset_and_the_others_go_on`、`an_h2_client_that_stops_reading_one_stream_fails_only_that_request`、`a_rejected_h2_request_without_a_page_resets_only_its_stream`、`a_dropped_h2_request_is_held_then_reset`。
3. **停止与空闲挂住**：客户端不回 PING、什么都不发、某个流不读。
   - 期望：空闲之后 GOAWAY；GOAWAY 之后至多一个时限就结束；停止总在有限的时间里结束。
   - 用例（任务 1、3）：`an_idle_h2_connection_is_sent_goaway`、`a_silent_h2_client_is_dropped_after_idle_and_the_grace`、`a_stop_ends_an_h2_connection_with_goaway`、`a_stop_while_an_h2_client_does_not_read_ends_the_connection`。
4. **版本转换丢东西或带错东西**：`:authority`、拆开的 `cookie`、`TE: trailers`、`Host`、`Transfer-Encoding`。
   - 期望：规则与 URL 看到 `:authority`；转到 HTTP/1.1 时 `Host` 是它、`cookie` 合成一个；转到 HTTP/2 时没有 `Host` 与连接专用的头、`TE: trailers` 留着。
   - 用例（任务 1、2）：`the_authority_of_an_h2_request_names_it_and_becomes_host`、`the_cookies_of_an_h2_request_are_joined_for_an_http1_upstream`、`an_h2_request_keeps_its_authority_and_te_trailers_to_an_h2_origin`，`version.rs` 的四条单元用例。
5. **字节记到别人头上**：共享的 HTTP/2 连接上，连接一层的计量把别的请求的字节记给拨号的那一个。
   - 期望：拨号之后连接一层不再记；每个请求记自己的正文。
   - 用例（任务 2）：`the_bytes_of_requests_on_a_shared_h2_connection_are_their_bodies`。
6. **升级与隧道被送到 HTTP/2 的上游**：源站讲 `h2` 时，WebSocket 的升级与解密的连接里的 `CONNECT`。
   - 期望：它们拨只给 `http/1.1` 的连接，升级照 M2a。
   - 用例（任务 2、5）：`an_upgrade_dials_http1_where_the_origin_speaks_h2`、`a_websocket_from_an_http1_client_goes_over_http1_where_the_origin_speaks_h2`。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `crates/rurge-http/src/{exec.rs（新）,version.rs（新）,engine.rs,lib.rs}`、`crates/rurge-http/Cargo.toml`、`crates/rurge-inbound/src/responses.rs`；`Conn` 的构造处（`crates/rurge-engine/src/{serve.rs,engine.rs}`） | 1 | 执行器；版本转换；对客户端的 HTTP/2；在途与空闲；`CONNECT` 的 501；守卫的原因 |
| `crates/rurge-http/src/{engine.rs,mitm/decrypt.rs}`、`crates/rurge-http/Cargo.toml` | 2 | `Sender`；共享的 HTTP/2 上游；转到 HTTP/2；正文的字节；升级与隧道只走 HTTP/1.1；被拒的流再发 |
| `crates/rurge-http/src/engine.rs` | 3 | HTTP/2 上的终止、看门狗、拒绝与停止 |
| `crates/rurge-http/src/mitm/decrypt.rs`、`crates/rurge-engine/src/serve.rs` | 4 | `Mitm` 按 `h2` 生效；引擎交出协议 |
| `crates/rurge-engine/{Cargo.toml,tests/mitm.rs}` | 5 | 端到端 |
| 文档（任务 6 列全） | 6 | 收尾 |

模块的名字与拆法可以照实际调整，调了记进修正记录。

---

## Task 0：开工前核对

**做什么**：对着当时的分支核对下面每一项。有出入的记进「执行期修正记录」，一项一行，写"计划写 X，代码是 Y，按 Y 做"；按记录调整后面的任务。**不改计划的正文。** 没有出入也记一行。

1. **工作树**：`git status --short` 空；当前分支 `phase4-m2`；`a9e759b`（M2a 合并之后）在历史里，之后只多了本计划的提交。
2. **读**：
   - 设计第 10 节、第 16 节第 10 条、第 17 节第 5 条、第 18、21 ～ 23 节（第 23 节先读）；
   - 本计划的「计划期决定」与 P17 的表；
   - `docs/status.md` 阶段 4 / M2a 一节；
   - M1b 计划「计划期决定」里讲守卫（P18）、看门狗与重发的几行。
3. **名字与签名**：用 `git grep -n` 逐项找到、记下行号；与「计划期决定」写的不同的照实记。
   - `crates/rurge-http/src/engine.rs`：`Conn`；`Upstreams`；`ReqBody`；`Pooled`；`Shared`；`Requests::{new, handle, prepare, answer, failed, timed_out, connect, start, redial, retry, upstream_failed, promote}`；`DialError`；`Sent`；`send`；`wants_upgrade`；`head_of`；`with_empty_body`；`form_of`；`default_port`；`origin_authority`；`Outing`；`origin_form`；`Idle`；`Progress`；`Tail::{new, watch_the_client, complete, fail, stop, poll_frame}`；`Guard::{new, outcome, retire}`；`Served`；`serve_connection`。测试模块：`Fake`、`Rig`、`rig_head_wait`、`rig_tls`、`rig_made`、`origin_tls`、`streaming_origin`、`holding_origin`、`wait_until`、`BIG_BODY`。
   - `crates/rurge-http/src/mitm/decrypt.rs`：`Mitm::{build, decides, accept, handshake}`、`OriginTls`、`Accepted`、`server_config`、`Watched`；测试模块的 `cfg`、`rig_of`、`client_config`、`handshake`。
   - `crates/rurge-http/src/{pool.rs,headers.rs}`：`Meter`、`Metered`、`Pool`、`Reusable`；`HOP_BY_HOP`、`Keep`、`strip`、`host_port`。
   - `crates/rurge-inbound/src/responses.rs`：`bad_request` 与它的写法。
   - `crates/rurge-engine/src/serve.rs`：`serve_mitm`、`serve_requests`（`Conn` 的字面量）；`crates/rurge-engine/src/engine.rs` 里监听器那一处 `rurge_http::Conn {`。
   - 用例夹具：`crates/rurge-net/src/testing.rs` 的 `TestServer::{spawn, spawn_tls_with, set, set_delay, requests, connections}`、`RecordedRequest`；`crates/rurge-proto/src/testing/tls.rs` 的 `TlsFixture::{new, roots, acceptor, acceptor_with_alpn, seen}` 与 `SeenHandshake.alpn`；`crates/rurge-engine/tests/mitm.rs` 的 `mitm_section`、`mitm_ca`、`client_config`、`handshake`、`harness_mitm`、`switching_echo_over_tls`；`common` 的 `FakeSocks5`、`connect_via_socks5`、`the_records`。
   - 外部库（注册表源码），行号写进记录：
     - hyper 1.11.1：`server/conn/http2.rs` 的 `Builder::{new, max_concurrent_streams, keep_alive_interval, timer, serve_connection}` 与 `Connection::graceful_shutdown`；`proto/h2/server.rs` 的缺省（`Config::default`）、`graceful_shutdown`（握手中只记 `close_pending`）、`H2Stream::poll2`（客户端重置时丢掉服务的 future、服务出错时 `send_reset(err.h2_reason())`）、`CONNECT` 带非零长度时直接重置；`client/conn/http2.rs` 的 `SendRequest::{is_ready, is_closed, try_send_request}`（只有派发通道关了才把请求交回）；`proto/h2/client.rs` 的 `Config::default`、`handshake` 里交给执行器的连接任务、`ClientTask::poll`（`poll_ready` 以 `NO_ERROR` 出错 → `Dispatched::Shutdown`）；`proto/h2/mod.rs` 的 `strip_connection_headers` 与 `PipeToSendStream`；`error.rs` 的 `h2_reason` 与 `source`；`proto/h1/role.rs` 对 `HTTP_2` 版本的处理。
     - h2 0.4.19：`proto/connection.rs` 的 `go_away_gracefully` 与收到 PING 回应时发最后的 GOAWAY；`proto/streams/streams.rs` 的 `recv_go_away`（`Error::remote_go_away`）；`error.rs` 的 `reason`、`is_go_away`、`is_remote`；`client.rs` 的 `Connection::max_concurrent_send_streams`、`SendRequest::is_extended_connect_protocol_enabled`。
     - rustls 0.23.43：服务端按自己列表的次序选 ALPN（`server/hs.rs`）；`ClientConnection::alpn_protocol`、`ServerConnection::alpn_protocol`。
4. **数**：`Cargo.lock` 里 `[[package]]` 的个数（520）；`LISTED` 的项数（23）；带本机账户名的文件数（派工文件给出要数的那个名字；数出来是 26）。
5. **现有用例**（P17 的表）：逐条找到、读它断言什么，把"预计要改 / 预计照过"逐条写进修正记录，一条用例一句。另扫一遍：`git grep -n 'Conn {'`、`git grep -n 'OriginTls {'`、`git grep -n 'Accepted {'`、`git grep -n 'Guard::new'`、`git grep -n 'serve_requests('`。表里没有的照实补一行。
6. **门禁**：跑一遍，记下测试二进制数、通过数、失败数与忽略数（M2a 收尾时是 91 / 4962 / 0 / 13，一台机器上的一次运行，不是 CI）。

**判据**：修正记录里有任务 0 的行；门禁的数字记下。

---

## Task 1：对客户端的 HTTP/2

**改哪里**（设计第 10.1、10.3 节；M2-D14、D16；P2 ～ P8）

1. `crates/rurge-http/Cargo.toml`：`[dev-dependencies]` 加 `h2.workspace = true`（P16）。
2. `crates/rurge-http/src/exec.rs`（新）：`Exec`（P2）。`lib.rs` 加 `mod exec;`。
3. `crates/rurge-http/src/version.rs`（新）：`Form` 与 `fit`（P8）。`lib.rs` 加 `mod version;`。
4. `crates/rurge-inbound/src/responses.rs`：`pub fn not_implemented(msg: &str) -> Response<ResponseBody>`，照 `bad_request` 写：状态 501、`Content-Type: text/plain; charset=utf-8`、正文是 `msg`。
5. `engine.rs`：
   - `Conn` 加 `pub h2: bool`。文档注释：客户端在这条连接上讲 HTTP/2（解密的连接、客户端用 ALPN 选了 `h2`），`serve_connection` 用 hyper 的 HTTP/2 服务端服务它；`head_wait` 对它不用（`phase 4 M2 design 10.1`）。全部构造处给 `false`：引擎 `serve.rs` 的 `serve_requests`（任务 4 换成协商的结果）、`engine.rs` 里监听器那一处、测试的 `rig_head_wait` 与 `rig_tls`。
   - `Activity`（P4）放进 `Shared`；`Guard::new(handle, stop, activity, left)`（P4、P7）。
   - `handle` 开头：`self.shared.conn.h2` 且方法是 `CONNECT` → P6 的那一支。
   - `handle` 第 5 步：`headers::strip` 之前 `let form = Form::of(&req, fallback)`（P8，`fallback` 是 `origin_authority(&dst_host, dst_port, default_port(scheme))`）；发送之前 `fit(&mut req, Version::HTTP_11, &form)`，重发（`retry` 与 `with_empty_body(head)` 的那一次）之前也 `fit` 一次。任务 1 里上游一律是 HTTP/1.1；任务 2 把这里换成按 `Sender::version`。
   - 第 7 步：应答的版本是 `HTTP_2` 而客户端讲 HTTP/1.1 时，改成 `HTTP_11`（P8 末尾）。
   - `serve_connection`：`conn.h2` 为真时走新的 `serve_h2`（P3、P5），否则照今天。
   - 常量 `MAX_CONCURRENT_STREAMS: u32 = 100`、`GOAWAY_GRACE: Duration = Duration::from_secs(1)`，各带一句文档注释写来由。
   - `Requests::routed` 的文档注释加一句：HTTP/2 上答 501 的 `CONNECT` 也算（它有自己的记录，P6）。

**用例**

- `crates/rurge-inbound/src/responses.rs` 的测试模块（没有就在文件末尾建）：`not_implemented_is_a_501_with_the_message`——状态 501、`Content-Type` 如上、正文逐字节是给的消息。
- `version.rs` 末尾的测试模块（纯函数，`#[test]`）：
  - `a_form_takes_the_uri_then_host_then_the_target`：
    - URI `https://u:p@a.test:8443/p` 而 `Host: b.test` → `authority` 是 `a.test:8443`（没有 userinfo）；
    - 源站形式 `/p` 与 `Host: b.test` → `b.test`；
    - 都没有 → 给的 `fallback`；
    - `TE: trailers` → `trailers` 真；`TE: deflate, Trailers` → 真；`TE: deflate` → 假；没有 `TE` → 假。
  - `an_h2_request_is_put_into_http1_form`：版本 `HTTP_2`、URI `https://a.test:8443/p?q=1`、两个 `cookie` 字段 `a=1` 与 `b=2`、一个 `te: trailers` → `fit(.., HTTP_11, ..)` 之后 URI 是 `/p?q=1`、`Host` 是 `a.test:8443`、版本 `HTTP_11`、`cookie` 只有一个字段且是 `a=1; b=2`、没有 `te`。
  - `an_http1_request_is_put_into_h2_form`：版本 `HTTP_11`、URI `/p`、`Host: a.test`、`transfer-encoding: chunked`，`Form { authority: "a.test", trailers: true }` → `fit(.., HTTP_2, ..)` 之后 URI 是 `https://a.test/p`、没有 `Host`、版本 `HTTP_2`、`te` 是 `trailers`、没有 `transfer-encoding`。
  - `an_http1_request_to_an_http1_upstream_is_left_alone`：版本 `HTTP_11`、绝对形式 `http://a.test/p`、两个 `cookie` → `fit(.., HTTP_11, ..)` 之后逐项不变（M2a 的行为，P8）。
  - `fitting_into_h2_then_http1_then_h2_is_fitting_into_h2_once`：同一个 HTTP/2 请求依次 `fit` 成 2、1.1、2，与只 `fit` 成 2 一次的 URI、头、版本相同（重发换了版本的情形）。
- `engine.rs` 末尾的测试模块：
  - 夹具：
    - `rig_h2(target, idle, drop_hold)`：照 `rig_head_wait`，`h2: true`、`origin: None`、`entry: Sniffed`；
    - `h2_client(stream: DuplexStream) -> (http2::SendRequest<Full<Bytes>>, JoinHandle<hyper::Result<()>>)`：`hyper::client::conn::http2::Builder::new(TokioExecutor::new())` 握手，连接任务 `tokio::spawn`；
    - 源站用 `TestServer::spawn()`（明文、HTTP/1.1）。URL 因此以 `http://` 开头（`origin: None`）。
  - `an_h2_client_is_served_over_an_http1_upstream`：
    - `GET http://target.test/hello` → 200、正文 `hi there`；
    - 源站收到的 `version` 是 `HTTP/1.1`、`path` 是 `/hello`、`Host` 是 `target.test`；
    - 记录：`url` 是 `http://target.test/hello`、`Completed`、状态 200、`up` 与 `down` 都大于 0。
  - `the_authority_of_an_h2_request_names_it_and_becomes_host`：`GET http://target.test:8443/a?q=1`（不带 `Host`）→ `Fake::seen` 的会话 `url` 是 `http://target.test:8443/a?q=1`、`http_host` 是 `target.test`；源站的 `path` 是 `/a?q=1`、`Host` 是 `target.test:8443`。
  - `the_cookies_of_an_h2_request_are_joined_for_an_http1_upstream`：两个 `cookie` 字段 `a=1`、`b=2` 与 `te: trailers` → 源站收到的 `headers` 里 `cookie` 恰好一项 `a=1; b=2`，没有 `te`。
  - `the_h2_server_offers_100_streams_and_no_extended_connect`：用 h2 的客户端（`h2::client::handshake`）发一个请求、等到应答（这时 SETTINGS 已经到了），断言 `Connection::max_concurrent_send_streams()` 是 100、`SendRequest::is_extended_connect_protocol_enabled()` 是假。
  - `a_connect_over_h2_is_a_501_with_a_record`：`CONNECT target.test:443`（hyper 的客户端，空正文）→ 状态 501、正文 `CONNECT over HTTP/2 is not supported`；记录 `Failed("CONNECT over HTTP/2 is not supported")`、状态 501、方法 `CONNECT`；`Fake::seen()` 空（没有选路）、`dials()` 是 0；`Served.routed` 是 1。
  - `a_stream_the_client_resets_is_cancelled_and_the_connection_stays`：
    - 源站 `set_delay("/hang", 10 秒)`、`set("/ok", "fine")`；
    - 发 `/hang`，等它的句柄出现，再丢掉它的应答 future（hyper 发 `RST_STREAM`）→ 记录 `Failed("client cancelled the request")`；
    - 同一个连接上再发 `/ok` → 200；`served` 这时还没结束（`!JoinHandle::is_finished()`）。
  - `an_idle_h2_connection_is_sent_goaway`：
    - `rig_h2`，`idle` 300 毫秒；一个请求 → 200；紧接着 `SendRequest::is_closed()` 是假；
    - 之后 3 秒之内 `SendRequest::is_closed()` 变真、客户端的连接任务结束（收到了 GOAWAY；结束时是 `Ok` 还是"对端关了连接"不断言）；
    - `served` 在 3 秒之内返回：`routed` 1。
  - `a_silent_h2_client_is_dropped_after_idle_and_the_grace`：`idle` 300 毫秒；客户端一侧的 duplex 什么也不写（不发 HTTP/2 的前言）→ `served` 不早于 1.2 秒（空闲 300 毫秒 + 从 GOAWAY 起的 `GOAWAY_GRACE` 1 秒，留 100 毫秒的余量）、不晚于 3 秒返回，`routed` 0、`error` `None`。
  - `a_stop_ends_an_h2_connection_with_goaway`：`/hang` 在途时取消连接的令牌 → 它的应答 future 以错误结束（流被重置）；客户端的连接任务与 `served` 都在 5 秒之内结束；记录 `Completed`（停止，同 M1b 的守卫）。
- 现有用例照 P17 的表。

**判据**

- 用例全绿；带等待的循环二十次（`cargo test -j 8 --workspace --lib -- engine::tests`）；
- 门禁绿。

---

## Task 2：对上游的 HTTP/2

**改哪里**（设计第 10.2 ～ 10.4 节；M2-D15、D16；P9 ～ P13）

1. `crates/rurge-http/Cargo.toml`：`h2.workspace = true` 从 `[dev-dependencies]` 挪到 `[dependencies]`（P13、P16）。
2. `mitm/decrypt.rs`：`OriginTls` 加 `pub h1: Arc<ClientConfig>`，文档注释：只给 `http/1.1` 的那份配置，升级与隧道用它（`phase 4 M2 design 10.4`）。`Mitm::build` 这时给它与 `config` 同一个（都是 `[http/1.1]`）；任务 4 才让 `config` 随 `h2`。
3. `engine.rs`：
   - `Sender`（P9）：`enum Sender { H1(http1::SendRequest<ReqBody>), H2(http2::SendRequest<ReqBody>) }`，方法 `version(&self) -> http::Version`、`is_closed(&self) -> bool`、`async fn send(&mut self, req: Request<ReqBody>) -> Sent`（替换今天的自由函数 `send`；两支都用 `try_send_request`）。
   - `connect(&self, handle, route, h1_only: bool)`：P9。`start` 改名 `start_h1`，新加 `start_h2`。
   - `Shared` 加 `h2: Mutex<Vec<SharedH2<U::Key>>>`，`struct SharedH2<K> { key: K, sender: http2::SendRequest<ReqBody>, peer: Option<SocketAddr> }`（P10）。
   - `handle`：
     - `needs_h1 = upgrade || tunnel` 挪到第 4 步之前算（P12）；
     - 第 4 步照 P10 的次序借；
     - 发送前 `fit` 用 `sender.version()`（P8），重发同样；
     - `Sent::Failed` 的重发条件加 `refused(&e)`（P13）；
     - HTTP/2 的应答头到了时 `handle.mark_first_byte()`（P11）。
   - `Outing.key` 改成 `Option<K>`（HTTP/2 的是 `None`），`Outing.sender` 是 `Sender`；`complete` 只把 HTTP/1.1 的放回池（P10）。
   - `Idle` 与 `Tail` 各握一个按请求的 `Meter`（`body`），HTTP/2 的一次发送前设成这个请求的句柄、HTTP/1.1 的设成 `None`；数据帧的长度经它记成 `up` / `down`（P11）。`Meter` 加两个 `pub(crate)` 方法 `count_up(&self, n: u64)`、`count_down(&self, n: u64)`：有借用它的句柄时加到那个句柄上。
   - `fn refused(e: &hyper::Error) -> bool`（P13）。
4. `pool.rs`：`Meter` 的两个方法（同上）。

**用例**（`engine.rs` 末尾的测试模块）

- 夹具：
  - `origin_tls` 照旧（`config` 与 `h1` 同一个 `[http/1.1]`）；新加 `origin_tls_h2(roots, name)`：`config` 给 `[h2, http/1.1]`、`h1` 给 `[http/1.1]`、`wait` 5 秒、不跳过校验；
  - `rig_tls_h2(origin, target)`：照 `rig_tls`，`h2: true`（客户端讲 HTTP/2）；
  - 讲 `h2` 的源站：`TestServer::spawn_tls_with(fixture.acceptor(false))`（`acceptor` 给 `[h2, http/1.1]`，hyper-util 的自动服务端按协商的协议讲）；只讲 HTTP/1.1 的：`acceptor_with_alpn(false, &[b"http/1.1"])`。
  - 会发 GOAWAY 或拒绝流的源站（P18）：h2 的服务端（`h2::server::handshake`）跑在 `fixture.acceptor_with_alpn(false, &[b"h2"])` 接受的 TLS 流上，行为由用例给。
- `an_h2_origin_is_spoken_to_in_h2`：HTTP/1.1 的客户端（`rig_tls` 加 `origin_tls_h2`）`GET /a`、`Host: target.test` →
  - 200；
  - 源站收到的 `version` 是 `HTTP/2.0`、`authority` 是 `Some("target.test")`、`headers` 里没有 `host`；
  - 记录 `url` 是 `https://target.test/a`；`first_byte_time()` 与 `connect_time()` 都有。
- `the_bytes_of_requests_on_a_shared_h2_connection_are_their_bodies`：同一个客户端连接上两个请求，源站讲 `h2`：
  - 第一个 `GET /first`（应答正文 `first`）：`down` 至少 5（拨号的请求带着握手的字节，P11）；
  - 第二个 `POST /second`，请求正文 100 字节、应答正文 `second` → `up` 恰好 100、`down` 恰好 6、`first_byte_time()` 有；`Fake::dials()` 仍是 1。源站给 `/second` 100 毫秒的延迟：`TestServer` 不读请求正文，先答完的话 hyper 会以 `NO_ERROR` 重置请求一侧，`up` 就看时机了。
- `requests_of_one_key_share_one_h2_connection`：
  - HTTP/2 的客户端（`rig_tls_h2`）：先一个请求（拨上那条连接），再并发五个 `GET /slow`（源站 `set_delay` 200 毫秒）→ 都是 200；源站 `connections()` 是 1、`Fake::dials()` 是 1；
  - HTTP/1.1 的客户端（`rig_tls`）依次两个请求 → `dials()` 是 1、`connections()` 是 1。
- `another_key_gets_another_h2_connection`：`Step::Go(7)` 与 `Step::Go(8)` 两个路径 → `dials()` 是 2、`connections()` 是 2。
- `an_h2_connection_that_went_away_is_not_lent_again`：源站（h2 的服务端）答完第一个流就 `graceful_shutdown` 并在连接结束后关掉 TCP；用例等源站看到这条连接结束，再发第二个 `GET` → 200、`dials()` 是 2、源站接受过两条连接。
- `a_request_the_origin_refuses_is_sent_again_once`：源站（h2 的服务端）在第一条连接上答第一个流、对之后的流 `send_reset(Reason::REFUSED_STREAM)`；第二条连接上照常答。
  - 先一个 `GET` 拨上；第二个 `GET`（借来的共享连接上被拒）→ 200，`dials()` 是 2；
  - 另起一个这样的源站与一条用例连接：先一个 `GET` 拨上，再 `POST`（带正文）被拒 → 502，记录 `Failed`、原因以 `upstream request failed: ` 开头，`dials()` 是 1（没有再发）。
- `an_upgrade_dials_http1_where_the_origin_speaks_h2`：用例自己的 TLS 源站（`fixture.acceptor(false)`，`[h2, http/1.1]`），协商到 `http/1.1` 才答 101 并回显；客户端经 `rig_tls` 加 `origin_tls_h2` 发 WebSocket 的升级 →
  - 101、回显 1000 字节；
  - fixture 的 `seen()` 最后一次握手的 `alpn` 是 `Some("http/1.1")`。
- `an_h2_request_keeps_its_authority_and_te_trailers_to_an_h2_origin`：HTTP/2 的客户端（`rig_tls_h2` 加 `origin_tls_h2`）`GET https://target.test:8443/x`、带 `te: trailers` → 源站的 `authority` 是 `Some("target.test:8443")`、`te` 是 `trailers`、没有 `host`。
- 现有用例照 P17 的表。

**判据**

- 用例全绿；带等待的循环二十次；
- `crates/rurge/tests/sockets.rs` 照过；
- 门禁绿。

---

## Task 3：HTTP/2 上的终止、看门狗、拒绝与停止

**改哪里**（设计第 10.5 节；M2-D17；P14）

1. `engine.rs`：
   - `Tail` 把上游的应答正文与 `Outing` 放进与看门狗共享的状态（P14）；`poll_frame`、`complete`、`fail`、`stop` 照它改，行为在 HTTP/1.1 的客户端上不变。
   - `watch_the_client`：HTTP/1.1 的客户端照今天；HTTP/2 的照 P14。
   - 被放掉之后 `Tail` 再被 poll：给一次错误（hyper 因此重置客户端一侧的流），之后结束。

**用例**（`engine.rs` 末尾的测试模块；`rig_h2` 与 `h2_client` 同任务 1）

- `a_killed_h2_stream_is_reset_and_the_others_go_on`：源站 `/slow` 延迟 10 秒、`/ok` 立即答；`/slow` 在途时 `kill` 它的句柄 →
  - 它的应答 future 以错误结束；记录 `Failed("killed")`；
  - 之后同一条连接上 `/ok` → 200；`served` 没有结束。
  - （等应答头时的终止在任务 1 之后就该绿：不要求先红。）
- `a_killed_h2_stream_whose_body_flows_is_reset_and_its_upstream_let_go`：`streaming_origin`；客户端读到一部分正文之后 `kill` →
  - 这个流的正文以错误结束；记录 `Failed("killed")`；
  - 源站在 3 秒之内看到它那条连接关了（`closed` 大于 0）；
  - `served` 没有结束。
- `an_h2_client_that_stops_reading_one_stream_fails_only_that_request`：
  - `idle` 300 毫秒；`streaming_origin`；客户端不用 `h2_client`，在用例里直接建 hyper 的 `Builder`，`initial_stream_window_size(1 MiB)`、`initial_connection_window_size(16 MiB)`（只让这一个流停住：第二个流的 4 MiB 放得下）；
  - 第一个流拿到应答头之后不读它的正文 → 3 秒之内记录 `Failed("client stopped reading")`、状态 200；源站看到那条上游连接关了；
  - 第二个流读完它的 4 MiB → `Completed`；`served` 没有结束；
  - 之后去读第一个流的正文：读完已经缓冲的部分之后以错误结束，不是正常的结尾（hyper 下一次 poll 它时才重置，设计第 10.5 节）。
- `a_stop_while_an_h2_client_does_not_read_ends_the_connection`：一个流不读时取消连接的令牌 → `served` 在 5 秒之内返回；记录 `Completed`。
- `a_rejected_h2_request_without_a_page_resets_only_its_stream`：`Step::Refused { refusal: Reject(Reject), pages: { error: true, reject: false } }` →
  - 这个请求的应答 future 以错误结束；记录 `Rejected(Reject)`；
  - 同一条连接上下一个请求 → 200。
  - （任务 1 之后就该绿：不要求先红。）
- `a_dropped_h2_request_is_held_then_reset`：`REJECT-DROP`、`drop_hold` 300 毫秒 → 应答 future 以错误结束，从发出起不早于 300 毫秒、不晚于 3 秒；记录 `Rejected(Drop)`；同一条连接上下一个请求 → 200。（同上，不要求先红。）
- M1b 讲看门狗、终止与停止的现有用例照 P17 的表照过。

**判据**

- 用例全绿；带等待的循环二十次；
- 门禁绿。

---

## Task 4：`Mitm` 按 `h2` 生效、引擎交出协议

**改哪里**（设计第 10 节开头、第 10.2 节；M2-D8、D15；P15）

1. `mitm/decrypt.rs`：
   - `Mitm` 加 `h2: bool`（`cfg.h2`）；
   - `decides` 的 ALPN 一步照 P15；
   - `server_config(authority, h2)`；
   - `Mitm::build`：`origin` 随 `h2`，另建只给 `http/1.1` 的 `origin_h1`；
   - `Accepted` 加 `pub h2: bool`：协商到的是 `h2`（在 `Watched` 包住之前，从服务端的 TLS 流读）；
   - `handshake` 给 `OriginTls { config, h1, name, wait }`。
2. `crates/rurge-engine/src/serve.rs`：`serve_requests` 加参数 `h2: bool`，建 `Conn` 时给它；`serve_mitm` 交 `accepted.h2`，`serve_http` 交 `false`。

**用例**（`decrypt.rs` 末尾的测试模块）

- `with_h2_an_h2_only_client_is_decrypted`：`h2 = true` 的 `[MITM]` → 只给 `h2` 的客户端是 `Decrypt`；`h2` 与 `http/1.1` 都给 → `Decrypt`；只给 `spdy/3` → `AlpnMismatch`。`h2 = false` 时只给 `h2` 仍是 `AlpnMismatch`（现有用例已经断言，照过）。
- `with_h2_the_client_gets_h2_when_it_offers_it`：`h2 = true`：
  - 客户端给 `[h2, http/1.1]` → 客户端读到的 ALPN 是 `h2`、`accepted.h2` 真；
  - 给 `[http/1.1]` → `http/1.1`、假；
  - 不给 → `None`、假。
- `the_origin_side_offers_h2_only_when_h2_is_on`：`h2 = true` 时 `accepted.origin.config.alpn_protocols` 是 `[h2, http/1.1]`、`h1` 的是 `[http/1.1]`；`h2 = false` 时两个都是 `[http/1.1]`。
- `a_client_is_served_http1_or_nothing`（现有）：加断言 `!accepted.h2`。
- 经引擎的接入由任务 5 的用例钉住。

**判据**：用例全绿；门禁绿。

---

## Task 5：端到端

**改哪里**：`crates/rurge-engine/Cargo.toml` 的 `[dev-dependencies]`（P16）；`crates/rurge-engine/tests/mitm.rs` 末尾加下面的用例。照 M2a 的写法：`harness_mitm`、`FakeSocks5`、`[Host]` 与按地址的规则作第二道防线、`mitm_section` 之后接一行 `h2 = true`。HTTP/2 的客户端是 rustls 握手（`client_config` 给 ALPN）之后在 TLS 流上用 hyper 的 `client::conn::http2`。

- `four_combinations_of_http1_and_h2_are_decrypted`：客户端给 `[http/1.1]` 或 `[h2, http/1.1]`，源站给 `[http/1.1]` 或 `[h2, http/1.1]`，四种各一次 `GET` →
  - 200 与正文；
  - 源站收到的 `version` 与它协商到的一致（`HTTP/1.1` 或 `HTTP/2.0`）；
  - 记录：`protocol` 是 `Https`、`url` 是 `https://origin.test/<路径>`、状态 200、`Completed`。
- `an_h2_only_client_is_decrypted_when_h2_is_on`：客户端只给 `h2` → 握手成功、协商到 `h2`、客户端信的是 MITM 的 CA；`GET` → 200。（`h2 = false` 的对照是 M2a 的 `a_client_offering_only_h2_is_passed_through`，照过。）
- `each_h2_stream_follows_its_own_rule`：同一条 HTTP/2 连接上两个流，`URL-REGEX` 让 `/a` 走 SOCKS5 策略、`/b` 走 `REJECT` → `/a` 200、`/b` 403 的错误页（在它自己的流上）；两条记录各照自己的规则；`/b` 之后同一条连接上再一个 `/a` → 200。
- `concurrent_h2_streams_share_one_upstream_connection`：客户端与源站都讲 `h2`；先一个请求，再并发五个（源站 `set_delay` 200 毫秒）→ 都是 200；源站 `connections()` 是 1。
- `the_cookies_of_an_h2_client_reach_an_http1_origin_as_one_field`：客户端讲 `h2`、源站只讲 `http/1.1`；两个 `cookie` 字段 → 源站收到一个 `cookie: a=1; b=2` 与 `Host: origin.test`。
- `a_websocket_from_an_http1_client_goes_over_http1_where_the_origin_speaks_h2`：照 M2a 的 `a_websocket_echoes_through_a_decrypted_connection`，`h2 = true`、源站的 fixture 给 `[h2, http/1.1]` → 回显照旧；源站看到的那次握手的 `alpn` 是 `http/1.1`。
- `a_connect_over_h2_has_one_record_and_a_501`：HTTP/2 的客户端发 `CONNECT origin.test:443` → 501；这条客户端连接只有一条记录：`Failed("CONNECT over HTTP/2 is not supported")`、状态 501（承载连接不另记，P6）。

**判据**

- 用例全绿；带等待的循环二十次（`cargo test -j 8 --workspace --test mitm`）；
- `Cargo.lock` 的包数 520；
- 门禁绿。

---

## Task 6：文档收尾（M2b 的部分）

**改哪里**（设计第 16、18 节）。照 M2a 任务 9 的写法与规矩：引的提交、用例名与数字对着仓库核过；历史的段落不改写；本地提交、CI 没有见过。

- `docs/api/phase4.md` 加「M2b　MITM（HTTP/2）」一节：
  - 什么时候讲 HTTP/2（`h2 = true`、客户端的 ALPN；对上游各自协商）；
  - 请求记录：`url` 与规则看 `:authority`；流被客户端取消的原因 `client cancelled the request`；`CONNECT` 的 501 与它的记录；HTTP/2 上游的 `up` / `down` 只记正文（拨号的那个请求带着握手）；
  - 共享的上游连接、被拒的流再发一次、空闲之后 GOAWAY 与之后的一秒；
  - 终止、看门狗、拒绝在 HTTP/2 上重置这个流；
  - 「没有覆盖的」：h2c、扩展 `CONNECT`、服务端推送、对上游的 PING、跨客户端连接的共享，与 M2c 的事。
- `docs/testing.md`：「命令」加 M2b 的过滤词；「测试约定（全文）」加阶段 4 / M2b 一节：客户端一侧的 HTTP/2 在 duplex 上用先验知识讲（不经 TLS）、h2 的客户端只用来读 SETTINGS、h2 的服务端用来造会 GOAWAY 与拒绝流的源站、窗口怎样调来只停住一个流。
- `docs/surge-compatibility-matrix.md`：
  - 7.2 节 `h2` 一行与设计第 16 节第 10 条的登记（并发 100 个流；上游的 HTTP/2 连接共享、字节只算正文、拨号的请求带着握手；客户端一侧 HTTP/2 而上游 HTTP/1.1 时上游连接数不设上限；HTTP/2 上的 `CONNECT` 答 501；看门狗到时客户端一侧的流要等下一次 poll 才重置；GOAWAY 之后一秒）；
  - 状态标记照 M2a 的规矩（有登记的差异才改）；附录照规则整张重数；第 5 行的「最近一次」照 M2a 的先例改成 M2b。
- `docs/status.md`：阶段 4 下加「### M2b（MITM：HTTP/2）」一节，照 M2a 一节的四段。
- `CLAUDE.md`：只改「现在在哪里」阶段 4 那一条，写 M2b 做完（本地提交、CI 没有见过）、下一步 M2c 的计划。
- 设计第 23 节：在"写 M2b 计划时"那几条之后，加"执行 M2b 时"：修正记录里改了设计正文意思的行，一条一段；没有就写一句"M2b 执行期没有改设计正文的意思"。
- `docs/README.md`：本计划那一项末尾接一句做完。
- `README.md`、`README_en.md`、`docs/requirements.md` 不改（M2c 收尾时改，设计第 18 节）。

**判据**

- 文档里引的提交、用例名与数字对着仓库核过；
- 门禁绿。

---

## 计划期决定

写计划时对着 `a9e759b` 核对过下面引的名字与行号。行号会随前面任务的改动漂移，以名字为准；名字不同就照实改、记进修正记录，不改语义。标了"订正设计"的，已随本计划写进设计第 23 节。

| # | 事项 | 决定 |
| - | ---- | ---- |
| P1 | 任务的拆法 | 设计第 2 节的 M2b 大纲（核对；执行器与服务端；上游的 HTTP/2 与共享；版本转换；终止；升级；端到端；文档）拆成七个任务：0 核对；1 对客户端的 HTTP/2（执行器、服务端、在途与空闲、`CONNECT`、守卫的原因、转到 HTTP/1.1）；2 对上游的 HTTP/2（`Sender`、共享、转到 HTTP/2、字节、升级、被拒的流）；3 终止与看门狗；4 `Mitm` 与引擎；5 端到端；6 文档。版本转换是一个纯函数的小文件，随任务 1 整个写好、带单元用例，任务 2 只是多一种调用 |
| P2 | 执行器 | `crates/rurge-http/src/exec.rs`：`#[derive(Clone)] pub(crate) struct Exec(pub(crate) TaskTracker);`，`impl<F> hyper::rt::Executor<F> for Exec where F: Future + Send + 'static, F::Output: Send + 'static`。`execute` 照 M1b 的 `start`：`tracker.spawn` 一个外层任务，里面 `tokio::spawn(fut)`，`JoinError::is_panic` 时 `tracing::error!("an HTTP/2 task panicked: {e}")`。服务端（客户端连接的每个流，hyper 1.11.1 `proto/h2/server.rs` 的 `exec.execute_h2stream`）与上游的 HTTP/2 连接（`proto/h2/client.rs:196-198` 交给执行器的连接任务，与每个请求的管道任务）都用它，跟踪器是 `Conn::tracker`：停止时等得到它们（设计第 10.1 节） |
| P3 | 服务端的参数 | `hyper::server::conn::http2::Builder::new(Exec(tracker))`（`server/conn/http2.rs:115`）`.timer(TokioTimer::new())`（:284）`.max_concurrent_streams(MAX_CONCURRENT_STREAMS)`（:215，100，M2-D14）。不调 `keep_alive_interval`（缺省 `None`，不发 PING）、不调 `enable_connect_protocol`（不宣告扩展 `CONNECT`，总设计 P4-D11）；别的照 hyper 的缺省（`proto/h2/server.rs:61-80`：两个窗口 1 MiB、帧 16 KiB、头 16 KiB、`date_header` 真）。hyper 自己的日志在 rurge 里是空操作（没开 `tracing` 特性）；h2 的 WARN 只有四处（`ping_pong.rs:128`、`recv.rs:915`、`streams.rs:1737`、`:1785`），都不带请求的内容 |
| P4 | 在途与空闲的计数 | `Shared` 里一个 `Arc<Activity>`：`in_flight: AtomicUsize` 与它最近一次减到 0 的时刻（建它时算一次）。`Guard::new` 加一，`Guard` 丢掉时减一（不论哪一支结束：答完、出错、被 hyper 丢掉、升级的转发结束）。`quiet(&Activity, d: Duration, since: tokio::time::Instant)`：在途为 0、且"最近一次减到 0 的时刻"与 `since` 两者中较晚的那个已过去 `d` 时返回；用 `tokio::time::sleep` 等到下一个可能的时刻再看，不忙等。`since` 让 GOAWAY 之后的时限从发 GOAWAY 的那一刻算起（P5），不被之前的空闲吃掉。HTTP/1.1 的连接也建它、不读它 |
| P5 | HTTP/2 连接的结束（订正设计） | `serve_h2` 的 `select!`（`biased`）四支：<br>① `abort` 取消 → 丢掉连接（同 M1b，`error` 为 `None`）；<br>② 连接自己结束 → hyper 的错误；<br>③ 连接的令牌（停止）→ `routed() == 0` 时丢掉连接（同 M1b）；否则 `graceful_shutdown()`，再收尾；<br>④ `quiet(activity, conn.idle, 开始服务的时刻)`（没有请求在途满 `idle_timeout`）→ `graceful_shutdown()`，再收尾。<br>**收尾**：`select!` `abort` → 丢掉；连接结束 → 它的结果；`quiet(activity, GOAWAY_GRACE, 调 graceful_shutdown 的时刻)` → 丢掉（`error` 为 `None`），写一行 DEBUG：`an HTTP/2 client did not close its connection within a second of GOAWAY; dropping it`。`GOAWAY_GRACE` 是 1 秒。<br>来由：h2 0.4.19 的平静关（`proto/connection.rs:629-651` 的 `go_away_gracefully`）先发 last stream id 为 2^31−1 的 GOAWAY 与一个 PING，收到 PING 的回应才发最后的 GOAWAY（`:572-583`），不回 PING 的客户端让连接一直开着；hyper 在握手中调 `graceful_shutdown` 只记下 `close_pending`（`proto/h2/server.rs:185-190`），一直不发前言的客户端同样；而可执行文件收尾时等跟踪器不设时限（`crates/rurge/src/cli/run.rs:2093-2100`，只有第二个信号能强退）。时限的长短参照 Go 的 `net/http` HTTP/2 服务端发 GOAWAY 之后的 `goAwayTimeout`（1 秒）。只在没有请求在途时计时：GOAWAY 之前已经开了的流照常答完 |
| P6 | HTTP/2 上的 `CONNECT` | `handle` 的第一步：`conn.h2` 且方法是 `CONNECT` → `prepare`（`url` 为 `None`、`http_host` 取请求行的 authority，同 M1b）→ `open` 句柄 → `Guard` → `set_method` → `routed` 加一 → `set_status(501)` → `finish(Failed(MSG))` → 答 `responses::not_implemented(MSG)`，`MSG` 是 `CONNECT over HTTP/2 is not supported`。不选路、不拨号、不经 `pages`（501 不是错误页）。`routed` 算它，是因为它有了自己的记录：不算的话承载连接会另记一条"no request"（M1b P10）。带非零 `content-length` 的 `CONNECT` 被 hyper 在服务之前重置（`proto/h2/server.rs:289`），没有记录（文档写明）。设计第 10.1 节"答 501、记录写原因"的细化 |
| P7 | 守卫的原因 | `Guard` 多一个 `left: &'static str`：客户端走掉时记的原因。HTTP/1.1 是今天的 `client closed the connection`；HTTP/2 是 `client cancelled the request`——客户端重置一个流时 hyper 丢掉服务的 future（`proto/h2/server.rs:445-452`）或正文，连接通常还在；整条连接关了时它的流也都算被取消。`killed` 与停止（`Completed`）照 M1b P18 |
| P8 | 版本转换 | `crates/rurge-http/src/version.rs`：<br>`pub(crate) struct Form { pub(crate) authority: String, pub(crate) trailers: bool }`；<br>`Form::of<B>(req: &Request<B>, fallback: String) -> Form`：`authority` 依次取 URI 的 authority、`Host`、`fallback`，都经 `headers::host_port`（去掉 userinfo）；`trailers` 是 `TE` 的逗号分隔的项里有 `trailers`（ASCII 不分大小写）。在 `headers::strip` 之前算（它去掉 `TE`）。`fallback` 是 `origin_authority(连接的目标, 端口, default_port(scheme))`。<br>`pub(crate) fn fit<B>(req: &mut Request<B>, to: http::Version, form: &Form) -> Result<(), http::Error>`：<br>**到 `HTTP_11`**：只在请求的版本是 `HTTP_2` 时动（来自 HTTP/2 的客户端，或上一次发送转成了 HTTP/2）：URI 改成源站形式（路径与查询，没有就 `/`）；`Host` 设成 `form.authority`（RFC 9113 第 8.3.1 节：中间人生成 `Host` 要用 `:authority` 的值）；版本 `HTTP_11`；多个 `cookie` 字段用 `; ` 连成一个（第 8.2.3 节）；去掉 `te`。<br>**到 `HTTP_2`**：URI 改成 `https://{form.authority}{路径与查询}`（hyper 的 HTTP/2 客户端拿它写 `:authority` 与 `:scheme`，h2 `client.rs:1636-1696`；只有 TLS 的上游会讲 HTTP/2）；去掉 `Host`；版本 `HTTP_2`；去掉 `transfer-encoding`（第 5 步给没有长度的正文补的）；`form.trailers` 时 `te: trailers`，否则没有 `te`。连接专用的其余几个头已经被 `headers::strip` 去掉（升级的请求不会到 HTTP/2，P12）。<br>**在哪里调**：每一次发送之前按那一次的 `Sender::version` 调，重发也调；两侧都是 HTTP/1.1 时什么也不变（M2a 的行为不变，解密的连接里的绝对形式请求照旧原样发）。<br>**应答**：版本是 `HTTP_2`、客户端讲 HTTP/1.1 时改成 `HTTP_11`（hyper 的 HTTP/1.1 服务端会自己改并写一行 DEBUG，`proto/h1/role.rs:432-436`，这里不靠它）；给 HTTP/2 客户端的应答不动（hyper 的 HTTP/2 服务端不看版本、自己去掉连接专用的头，`proto/h2/mod.rs:50-90`；rurge 的 `headers::strip` 先已去掉）。设计第 10.3 节的细化 |
| P9 | 上游的 HTTP/2 | `Sender` 见任务 2。`connect(handle, route, h1_only)`：拨号、计量、令牌的检查照今天；`origin` 是 `Some(tls)` 时用 `if h1_only { tls.h1 } else { tls.config }` 做 TLS；握手之后 `stream.get_ref().1.alpn_protocol() == Some(b"h2")` → `start_h2`，否则 `start_h1`（今天的 `start`）；明文的一律 `start_h1`。`start_h2`：`hyper::client::conn::http2::Builder::new(Exec(tracker)).handshake(TokioIo::new(io))`，连接的 future 照 `start_h1` 放上跟踪器（内层任务、panic 写 ERROR）；握手失败是 `DialError::Handshake`。客户端的参数照 hyper 的缺省（`proto/h2/client.rs:84-103`：流窗口 2 MiB、连接窗口 5 MiB、起始可发 100 个流、不发 PING） |
| P10 | 共享的 HTTP/2 上游（M2-D15） | `Shared.h2: Mutex<Vec<SharedH2<U::Key>>>`，与 M1b 的池一样属于一条客户端连接。<br>**借**（第 4 步）：不是 `needs_h1` 时，先找键相等且 `!sender.is_closed()` 的那一项，克隆它的 `SendRequest`（不独占）；路上遇到已关的项去掉。没有就照 M1b 从池里取（池里只有 HTTP/1.1 的）。还没有就拨。借来的同借池里的：`mark_connected`、`trail().set_peer`、留着 `route` 给一次重发。<br>**放**：拨来的是 HTTP/2 时，这个键还没有活着的项就放进去（键移进去，`Outing.key` 是 `None`）；已经有了就不放，这个请求独自用它拨的这条，请求结束、最后一个 `SendRequest` 丢掉时 hyper 平静地关掉它。HTTP/2 的从不进池；`complete` 只放回 HTTP/1.1 的。<br>**走掉**：源站的 GOAWAY（`NO_ERROR`）让 hyper 的派发任务结束（`proto/h2/client.rs:678-689`），之后 `is_closed()` 为真，不再借出。在途的请求握着自己的克隆，hyper 让连接留到它们的流结束（`:170-173` 的 `conn_drop_ref`）。<br>客户端讲 HTTP/2、上游讲 HTTP/1.1 时，并发的请求各占一条上游连接，数目不另设上限（登记，设计第 10.2 节） |
| P11 | 字节（订正设计） | HTTP/1.1 的上游照 M1b（换连接的计量，记线上的字节）。HTTP/2 的上游：`start_h2` 握手之后把这条连接的 `Meter` 设成 `None`，从此不再设——连接一层什么也不记；**拨这条连接的请求带着握手之前已经记下的字节**（TCP 之上的 TLS 握手与 HTTP/2 的前言，同 M2a 记 TLS 握手的做法），之后的每个请求只记自己的正文：`Idle` 把每个数据帧的长度经按请求的 `body` 计量记成 `up`，`Tail` 记成 `down`；HTTP/2 的应答头到了时 `mark_first_byte`。trailers 不计。`body` 计量在每一次发送前按那一次的 `Sender` 设（HTTP/2 设成句柄、HTTP/1.1 设成 `None`），所以重发换了版本也只记一边。订正设计第 10.2 节"只算正文"：拨号的那一个请求另带握手 |
| P12 | 升级与隧道只走 HTTP/1.1（订正设计） | `needs_h1 = upgrade \|\| tunnel`（`tunnel` 是解密的 HTTP/1.1 连接里的 `CONNECT`；HTTP/2 的客户端发不出这两种：`wants_upgrade` 要求 HTTP/1.1，`CONNECT` 在 P6 已答）。`needs_h1` 时跳过共享的 HTTP/2 项，照 M1b 从池里取（池里只有 HTTP/1.1 的，借来正好），拨号时 `connect(.., h1_only = true)` 用 `OriginTls::h1`。**池键不另分**：设计第 10.4 节写"池键里与别的请求分开"，用不着——池里本来只有 HTTP/1.1 的连接；一个用只给 `http/1.1` 的配置拨来、升级没成而放回池里的连接，下一个普通请求借去也只是讲 HTTP/1.1 |
| P13 | 被拒的流再发一次（订正设计） | `fn refused(e: &hyper::Error) -> bool`：顺着 `std::error::Error::source` 找 `h2::Error`；它的 `reason()` 是 `Some(h2::Reason::REFUSED_STREAM)`，或 `is_go_away() && is_remote()`（源站的 GOAWAY 说这个流没有被处理：h2 0.4.19 `proto/streams/streams.rs:790-804` 给 last stream id 之后的流 `Error::remote_go_away`）→ 真。`handle` 里 `Sent::Failed(e)` 的重发条件从 `e.is_incomplete_message()` 改成 `e.is_incomplete_message() \|\| refused(&e)`，别的条件照 M1b（借来的连接、留着头——幂等且没有正文）：RFC 9113 第 8.7 节，这样的请求一定没有被处理过，可以再发。别的请求照 M1b 答 502。hyper 只在派发通道已关时把请求整个交回（`client/conn/http2.rs:193-218`，即 `NotWritten`，照 M1b 任何方法都再发），已经交给派发任务的请求出错时请求不再交回（`proto/h2/client.rs` 的 `message: None`），所以需要这一条。为此 `rurge-http` 正式依赖 `h2`（锁文件里已有，包数不变）：订正设计第 3 节与 M2-D2 的依赖列表。再发时写一行 DEBUG：`an upstream refused the request; sending it again on a new connection` |
| P14 | HTTP/2 上的终止与看门狗（订正设计） | `Tail` 的上游（应答正文 `Incoming` 与 `Outing`）放进 `Arc<Mutex<Option<…>>>`，与看门狗共享；`complete` / `fail` / `stop` 从里面取走；`poll_frame` 每次锁着它 poll。<br>**看门狗，客户端讲 HTTP/1.1**：照今天（到时 `finish` 加 `abort.cancel()`；请求的令牌 → `abort.cancel()`）。<br>**客户端讲 HTTP/2**：到时 → `finish(Failed("client stopped reading"))`，取走上游并丢掉（HTTP/1.1 的上游连接因此关掉；HTTP/2 的上游流因此被 hyper 以 `CANCEL` 重置）；请求的令牌 → 连接的令牌也取消了（停止）时 `abort.cancel()`（整条连接本来就要走，不取消的话一个不读的流会让停止一直等下去）；否则（终止这一个请求）取走上游并丢掉，按守卫的规矩结束句柄（`killed`）。别的情形不取消 `abort`。<br>**之后**：`Tail` 再被 poll 时上游已不在 → 给一次 `Err`（hyper 以 `INTERNAL_ERROR` 重置客户端一侧的流），之后 `done`。客户端一侧的流要等 hyper 下一次 poll 它、或连接关了才重置（登记，设计第 10.5 节）；在这之前它的 `Tail` 与守卫还在，在途数（P4）不减，连接因此不会空闲关掉。<br>订正设计第 10.5 节"不取消 `Conn::abort`"：停止时仍取消 |
| P15 | `Mitm` 与 `h2` | `Mitm` 加 `h2: bool`（`cfg.h2`）。<br>`decides` 的 ALPN 一步：给了 ALPN、而里面既没有 `http/1.1`、也没有（`h2` 开着时的）`h2` → `AlpnMismatch`。那一行 DEBUG 不变。<br>`server_config(authority, h2)`：`alpn_protocols` 是 `h2` 开着时 `[h2, http/1.1]`，否则 `[http/1.1]`（rustls 0.23.43 的服务端按自己列表的次序选，`server/hs.rs:99-119`：两种都给的客户端拿到 `h2`）。<br>`Mitm::build`：`origin` 用 `origin_client_config(roots, skip, alpn)`，`alpn` 是 `h2` 开着时 `[h2, http/1.1]`，否则 `[http/1.1]`；`origin_h1` 是 `[http/1.1]`（`h2` 关着时与 `origin` 同一个 `Arc`）。<br>`Accepted.h2`：服务端的 TLS 流 `get_ref().1.alpn_protocol() == Some(b"h2")`，在 `Watched` 包住之前读 |
| P16 | 依赖 | `rurge-http`：任务 1 在 `[dev-dependencies]` 加 `h2`，任务 2 挪到 `[dependencies]`（P13）。`rurge-engine`：任务 5 在 `[dev-dependencies]` 加 `hyper`、`hyper-util`、`http-body-util`（工作区已有，`hyper` 开着 `client`、`server`、`http1`、`http2`）。`Cargo.lock` 的包数 520 不变，只有这两个 crate 的依赖列表变 |
| P17 | 现有的用例 | 见下面的表。表里"预计照过"的若红，停下报告，不改期望 |
| P18 | 测试的客户端与源站 | 客户端一侧：hyper 的 `client::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())`，在 duplex 上用先验知识讲（`serve_connection` 拿到的已经是解密之后的流）；要读服务端的 SETTINGS 时用 h2 的客户端（`h2::client::handshake`）。会 GOAWAY 或拒绝流的源站：h2 的服务端（`h2::server::handshake`）跑在 `TlsFixture::acceptor_with_alpn(false, &[b"h2"])` 接受的 TLS 流上（`rurge-proto` 的 `testing/h2proxy.rs` 是现成的写法）。别的源站：`TestServer`（hyper-util 的自动服务端，经 TLS 时按 ALPN 讲） |

### P17 的表：要改的、要留意的现有用例

| 用例 | 为什么 | 任务 | 预计 |
| ---- | ------ | ---- | ---- |
| `crates/rurge-http/src/engine.rs` 测试模块的 `rig_head_wait`、`rig_tls`（`Conn` 字面量） | `Conn` 多 `h2` | 1 | 改：`h2: false` |
| `crates/rurge-engine/src/serve.rs` 的 `serve_requests`、`crates/rurge-engine/src/engine.rs` 监听器那一处（`Conn` 字面量） | 同上 | 1 | 改：`h2: false`（任务 4 把前者换成参数） |
| `engine.rs` 测试模块的 `origin_tls` | `OriginTls` 多 `h1` | 2 | 改：`h1` 与 `config` 同一个 |
| `crates/rurge-http/src/mitm/decrypt.rs` 的 `a_client_is_served_http1_or_nothing` | `Accepted` 多 `h2` | 4 | 照过；加断言 `!accepted.h2` |
| 同文件 `decides_in_the_design_order` | ALPN 一步看 `h2` | 4 | 照过（`cfg()` 的 `h2` 是假） |
| `crates/rurge-engine/tests/mitm.rs` 的 `a_client_offering_only_h2_is_passed_through` | 同上 | 4 | 照过（Profile 没写 `h2`） |
| M1b 的 `a_client_that_stops_reading_fails_after_idle`、`a_killed_response_whose_client_does_not_read_is_closed`、`stopping_a_connection_ends_its_requests`、`a_request_hyper_drops_is_finished_by_its_guard`、`a_killed_request_closes_its_connection` | `Tail`、看门狗与守卫改写 | 1、3 | 照过（HTTP/1.1 的客户端照 M1b） |
| M1b 的 `a_request_not_yet_written_is_sent_again_on_a_new_connection`、`an_idempotent_request_cut_off_is_sent_again_once`、`a_post_cut_off_after_it_was_written_is_a_502`、`a_request_on_a_new_connection_is_not_sent_again` | `send` 换成 `Sender::send`；重发的条件多 `refused` | 2 | 照过 |
| M2a 的 `a_decrypted_request_goes_to_the_origin_over_tls`、`skip_server_cert_verify_reaches_an_untrusted_origin`、`a_slow_origin_handshake_is_a_502_after_its_wait`、`a_websocket_upgrade_goes_through_tls_and_is_counted_once` | `connect` 多一个参数、`start` 拆两个 | 2 | 照过 |
| `crates/rurge-http/src/headers.rs` 的两条 | `strip` 不改 | — | 照过 |
| `crates/rurge/tests/sockets.rs` | 生产代码多了 HTTP/2 的握手 | 1、2 | 照过：没有新建套接字 |
| `crates/rurge-engine/tests/{sniff.rs,http.rs,mitm.rs}` 其余 | 解密的连接在 `h2` 关着时照 M2a | 4 | 照过 |

---

## 执行期修正记录

下面写"执行者的裁定"的，是 Pi 在设计与计划的原则下定的；写"协调者的裁定"的，是协调的会话代项目所有者定的。项目所有者都可以改。计划的正文不改：正文里说得不对的地方，以这张表的行为准。

| # | 任务 | 内容 | 提交 |
| - | ---- | ---- | ---- |
| 1 | 0 | 开工前核对（第 1、2 项）：`git status --short` 空；分支 `phase4-m2`；`HEAD` 是 `7109a82`；`git log --oneline a9e759b..HEAD` 只有 `7109a82 docs(plan): 阶段 4 / M2b（MITM：HTTP/2）实施计划`，与派工写的一致。读了设计第 23 节（先读）、第 10、16.10、17.5、18、21、22 节，本计划的 P1 ～ P18 与 P17 的表，`docs/status.md` 阶段 4 / M2a 一节，M1b 计划的 P1、P6、P7、P11 与 P18（守卫的结局） | 本提交 |
| 2 | 0 | P2 的执行器成立。hyper 1.11.1 的 HTTP/2 服务端要 `E: Http2ServerConnExec<S::Future, B>`，总括实现要 `E: Clone + Executor<H2Stream<F, B, E>>` 且 `H2Stream<F, B, E>: Future`（`hyper-1.11.1/src/rt/bounds.rs:96`、`:117`）；客户端要 `E: Http2ClientConnExec<B, T> + Unpin`（`rt/bounds.rs:37`、`:62`，`client/conn/http2.rs:566-580`），总括实现同样要 `E: Clone + Executor<那个 future>`。`Executor` 自己没有总括实现（`hyper-1.11.1/src/rt/mod.rs:45-48`），所以 P2 的 `#[derive(Clone)] struct Exec(TaskTracker)` 加一条 `impl<F> Executor<F> for Exec where F: Future + Send + 'static, F::Output: Send + 'static` 两边都满足（`Exec` 要 `Send + Sync`，`TaskTracker` 是）。客户端 `handshake` 对 IO 要 `T: Read + Write + Unpin`，对正文要 `B: Body + 'static`、`B::Data: Send`、`B::Error: Into<Box<dyn Error + Send + Sync>>`，**不要求 `B: Unpin`**；`ReqBody`（`BoxBody<Bytes, BoxError>`，`crates/rurge-http/src/engine.rs:183`）满足，它本身 `Send + Sync`（`http-body-util-0.1.5/src/combinators/box_body.rs:12-13`），连接 future 因此可 `Send` | 本提交 |
| 3 | 0 | 计时器：`hyper_util::rt::TokioTimer` 在 hyper-util 的 `tokio` 特性下（`hyper-util-0.1.20/src/rt/mod.rs:8-12`、`rt/tokio.rs:91` 的 `pub fn new`），工作区开了它（根 `Cargo.toml:30` 的 `features` 里有 `tokio`）。服务端不设 `Builder::timer` 时 hyper 不 panic：`keep_alive_interval` 缺省 `None`，`ping_config.is_enabled()` 为假就不建 PING 通道、也就用不到计时器（`hyper-1.11.1/src/proto/h2/server.rs:209-220`）；会 panic 的是“显式设了超时却没有计时器”（`common/time.rs:70-84`，第 80 行的 `panic!`），HTTP/2 服务端唯一的那个超时就是 `keep_alive_interval`。P3 照样装 `TokioTimer` | 本提交 |
| 4 | 0 | `try_send_request`：HTTP/2 的是 `pub fn try_send_request(&mut self, req: Request<B>) -> impl Future<Output = Result<Response<IncomingBody>, TrySendError<Request<B>>>>`（`hyper-1.11.1/src/client/conn/http2.rs:193-196`）；HTTP/1.1 的是同一个签名、**同一个类型**（`client/conn/http1.rs:262`，类型在 `client/dispatch.rs:26`，从 `hyper::client::conn::TrySendError` 导出）。取回请求是 `take_message()`、错误是 `into_error()`（`dispatch.rs:308`、`:322`）。今天的 `send`（`crates/rurge-http/src/engine.rs:1041`）就是这么用的，所以任务 2 的 `Sender::send` 两支合成一个 `Sent`（`engine.rs:1028`）不需要新类型 | 本提交 |
| 5 | 0 | P13 的错误链成立：`hyper::Error` 的 `cause` 是 `Box<dyn StdError + Send + Sync>`（`hyper-1.11.1/src/error.rs:8`、`:37`），`source()` 直接交出它（`:619-625`），h2 的错误经 `Error::new_h2` 装上（`:492-497`）之后**就在第一层**；hyper 自己的 `h2_reason()`（`:353-359`）也是顺着 `source()` 找 `h2::Error`，只是它还继续往下钻（`:338-349`）。`is_incomplete_message` 在 HTTP/2 上永远为假：它只匹配 `Kind::IncompleteMessage`（`:271-278`），那个 kind 只有 HTTP/1.1 的 `new_incomplete`（`:363`）造得出。`Cargo.lock` 里 `h2` 只有一项（0.4.19），`downcast_ref::<h2::Error>()` 不会被两个版本骗过 | 本提交 |
| 6 | 0 | h2 0.4.19 的名字：`Error::{reason() -> Option<Reason>, is_go_away() -> bool, is_remote() -> bool}`（`h2-0.4.19/src/error.rs:52`、`:89`、`:101`，`is_remote` 只对远端来的 GOAWAY 与 RST_STREAM 为真）；`Reason::REFUSED_STREAM = Reason(7)`（`frame/reason.rs:44`）；`client::Connection::max_concurrent_send_streams(&self) -> usize`（`client.rs:1444`，取的是对端已确认的 SETTINGS）与 `client::SendRequest::is_extended_connect_protocol_enabled(&self) -> bool`（`client.rs:552`，同样是已确认的值，要等 SETTINGS 到了才真）；`server::SendResponse::send_reset(&mut self, reason: Reason)`（`server.rs:1307`，`SendStream` 上同名的一个在 `:1379`）与 `server::Connection::graceful_shutdown(&mut self)`（`server.rs:548`，`impl<T, B> Connection<T, B>` 从 `:379` 起）。任务 2 的源站照 `crates/rurge-proto/src/testing/h2proxy.rs:326-346` 的写法 | 本提交 |
| 7 | 0 | 窗口与流：hyper 客户端缺省流窗口 2 MiB、连接窗口 5 MiB、起始可发 100 个流、`adaptive_window` 假（`hyper-1.11.1/src/proto/h2/client.rs:48-51`、`:61`、`:83-88`）；服务端两个窗口都 1 MiB、帧 16 KiB、头 16 KiB（`proto/h2/server.rs:36-40`、`:63-80`）。客户端不读一个流时服务端最多先发约 1 MiB（服务端的流窗口先到，比客户端声明的 2 MiB 小），所以任务 3 把客户端的流窗口调成 1 MiB、连接窗口 16 MiB 正好只停住那一个流，与计划写的一致 | 本提交 |
| 8 | 0 | ALPN：`tokio_rustls::server::TlsStream::get_ref()` / `client::TlsStream::get_ref()` 都给 `(&IO, &ServerConnection)` / `(&IO, &ClientConnection)`（`tokio-rustls-0.26.4/src/server.rs:314`、`client.rs:217`；整对的 enum 在 `lib.rs:81`），`alpn_protocol()` 是 `CommonState` 的方法（`rustls-0.23.43/src/common_state.rs:148`），两种连接都 Deref 到它；服务端按**自己的**列表次序选（`server/hs.rs:99-119` 对 `alpn_protocols` 做 `find`），所以 `[h2, http/1.1]` 时两种都给的客户端拿到 `h2`，与 P15 一致 | 本提交 |
| 9 | 0 | `TestServer` 讲 HTTP/2：它用 hyper-util 的 `auto::Builder`（`crates/rurge-net/src/testing.rs:110`），经 TLS 时按协商到的 ALPN 讲，明文时看先验知识的前言；`RecordedRequest.version` 是 `format!("{:?}", req.version())`（`:369`），HTTP/2 的请求记成 `"HTTP/2.0"`；`authority` 是 `req.uri().authority()`（`:370`），HTTP/1.1 的源站形式请求没有 authority（`None`）、只有绝对形式的有；它不读请求的正文（`handle` 只取方法、URI 与头，`:359-381`），P11 的用例靠这一点只数请求自己的正文 | 本提交 |
| 10 | 0 | `Conn::idle` 今天不用于连接的空闲：两条请求之间的空闲是 hyper 的 `header_read_timeout(head_wait)` 加 `keep_alive(true)`（`crates/rurge-http/src/engine.rs:1756-1763`，嗅探来的连接 `head_wait` 就是 `idle_timeout`）；`Conn::idle` 用在四处——等应答头（`head_deadline`，`:1359`）、请求正文帧之间（`Idle`，`:1379`）、应答正文停住（`Tail`，`:1484`）、升级时写给客户端的时限（`promote`，`:910`）。P4 把 `idle` 当“没有请求在途多久”用是 M2b 的新用法，不改 HTTP/1.1 的那几处。`serve_connection` 的停止那一支：`routed() == 0` 直接丢掉连接（`:1773`），否则 `graceful_shutdown()`（`:1789`）再收尾（`:1768` 的 `select!`），`serve_h2` 照它写 | 本提交 |
| 11 | 0 | 计量今天怎样换：`Meter` 是 `Arc<Mutex<Option<Arc<SessionHandle>>>>`（`crates/rurge-http/src/pool.rs:103`），公开的只有 `new()`（`:106`）与 `set(...)`（`:111`），`current()` 是 `pool` 模块私有的（`:117`）。`connect` 拨号之后立刻 `meter.set(Some(handle.clone()))`（`engine.rs:722`），池借出时 `pooled.meter.set(Some(handle.clone()))`（`:325`），重发时再设一次（`:846`）；放回池与升级转发时清空（`:1580`、`:939`）。`Metered` 包在 TLS **之前**的流上（`:729` 一带），所以记的是线上的字节。任务 2 的 `count_up` / `count_down` 加在 `pool.rs` 的 `Meter` 上（执行者的裁定：加在 `pool.rs`，因为 `current()` 不对外，`engine.rs` 调不到） | 本提交 |
| 12 | 0 | 守卫的结局今天在 `crates/rurge-http/src/engine.rs:1711-1718`：`killed` → `Failed("killed")`（`:1712-1713`）、连接令牌已取消 → `Completed`（`:1714-1715`）、否则 → `Failed("client closed the connection")`（`:1716`）；`retire`（`:1722`）与 `Drop`（`:1727-1733`）都走 `outcome()`。hyper 的 HTTP/1.1 服务端在客户端断开时丢掉服务的 future，`a_request_hyper_drops_is_finished_by_its_guard`（`:3148-3171`）钉的就是这件事；HTTP/2 上同一件事经 hyper 丢 future 或正文（`hyper-1.11.1/src/proto/h2/server.rs:452-460`），P7 因此给 `Guard` 加 `left` | 本提交 |
| 13 | 0 | h2 0.4.19 的 `warn!` 正好四处，与 P3 数的一样：`proto/ping_pong.rs:128`、`proto/streams/recv.rs:915`、`proto/streams/streams.rs:1737`、`:1785`，四处都只说计数与原因，不带请求的内容。rurge 的日志过滤对非 `rurge` 的 target 缺省 `WARN`（`crates/rurge/src/cli/run.rs:161-171` 的 `with_default(LevelFilter::WARN)`），所以这四行到得了日志、不会把 INFO 弄吵 | 本提交 |
| 14 | 0 | 数（`7109a82`）：`Cargo.lock` 的 `[[package]]` 520 个；`h2` 一项、0.4.19；`crates/rurge-platform/tests/unsafe_functions.rs:18` 的 `LISTED` 声明是 `[(&str, &str); 23]`、数出来的项也是 23；带本机账户名的文件 26 个。四项都与派工给的基线相同 | 本提交 |
| 15 | 0 | P17 的表，任务 1 那两行核对：`rig_head_wait`（`engine.rs:2167`）与 `rig_tls`（`:2194`）的 `Conn` 字面量都不带 `..`（`:2175`、`:2196`），各加 `h2: false` 是必需的；`crates/rurge-engine/src/serve.rs` 的字面量在 `:526`（`serve_requests` 的定义在 `:506`、调用在 `:377` 与 `:478`），监听器那一处在 `crates/rurge-engine/src/engine.rs:2007`，两处也都不带 `..`。M1b 那五条（`engine.rs:3548`、`:3581`、`:3198`、`:3148`、`:3172`）今天断言的是 HTTP/1.1 客户端的行为（`client stopped reading`、`killed`、`client closed the connection`、连接关掉），P7 与 P14 都不改 HTTP/1.1 这一支，照过站得住 | 本提交 |
| 16 | 0 | P17 的表，任务 2 那三行核对：`origin_tls`（`engine.rs:2237`，字面量在 `:2243`）不带 `..`，加 `h1` 是必需的，且 P17 写的“`h1` 与 `config` 同一个”可行（`Mitm::build` 里 `h2` 关着时两个是同一个 `Arc`）；重发那四条（`:3755`、`:3782`、`:3800`、`:3824`）都经 `send` 与 `Sent::{NotWritten, Failed}`，`Sender::send` 换上去、重发条件多 `refused` 之后它们断言的两件事（该再发的再发一次、不该发的答 502 或不再发）仍成立；M2a 那四条（`:3909`、`:3992`、`:4056`、`:4109`）都不直接调 `connect`（`connect` 只被 `redial` 调，`:794`），签名多一个 `h1_only` 时它们照过，要改的只有 `origin_tls` 那一个构造点 | 本提交 |
| 17 | 0 | P17 的表里“`Tail`、看门狗与守卫改写”那一行（任务 1、3）落在五条用例上：`engine.rs:3548`（不读时看门狗到时记 `client stopped reading` 并丢掉连接）、`:3581`（被终止且不读时关掉）、`:3198`（停止时在途的请求都结束）、`:3148`（hyper 丢掉 future 时守卫收尾）、`:3172`（被终止时连接关上）。P14 把 `Tail` 的上游挪进共享状态之后，HTTP/1.1 这一支的结局与今天相同，五条照过站得住；任务 3 另加自己的 HTTP/2 用例 | 本提交 |
| 18 | 0 | P17 的表，任务 4 那三行核对：`a_client_is_served_http1_or_nothing`（`crates/rurge-http/src/mitm/decrypt.rs:655`）今天断言“没有 ALPN → `None`；给 `[h2, http/1.1]` → `http/1.1`”，`h2 = false` 的这代配置下仍成立，加 `!accepted.h2` 站得住；`decides_in_the_design_order`（`:482`）用的 `cfg()`（`:420`）不写 `h2`，`MitmConfig::default()` 的 `h2` 是假，照过；`crates/rurge-engine/tests/mitm.rs:273` 的 `a_client_offering_only_h2_is_passed_through` 的 Profile 只经 `mitm_section`（`:19`，不写 `h2`），照过；`sniff.rs`、`http.rs`、`mitm.rs` 其余用例都在明文或 `h2 = false` 的路上，照过 | 本提交 |
| 19 | 0 | P17 的表有两处与代码不符：`crates/rurge-http/src/headers.rs` 不是“两条”而是四条用例（`:87`、`:105`、`:130`、`:157`），`strip` 与 `host_port` 都不改，四条都照过；`crates/rurge/tests/sockets.rs:186` 的 `every_outbound_socket_is_made_in_one_place` 扫的是建套接字的地方，M2b 只在拨好的流上做 TLS 与 HTTP/2、不新建套接字，照过（任务 1、2 各跑一次） | 本提交 |
| 20 | 0 | 补扫（`git grep -n` 过 `Conn {`、`OriginTls {`、`Accepted {`、`Guard::new`、`serve_requests(`、`connect(`、`client closed the connection`）：构造 `Conn` 字面量而不带 `..` 的正好四处（`crates/rurge-engine/src/engine.rs:2007`、`crates/rurge-engine/src/serve.rs:526`、`crates/rurge-http/src/engine.rs:2175`、`:2196`）——任务 1 加 `h2`，任务 4 把 `serve_requests` 那一处换成参数；`OriginTls` 字面量两处（`crates/rurge-http/src/mitm/decrypt.rs:232` 在 `handshake` 里、`crates/rurge-http/src/engine.rs:2243` 在测试的 `origin_tls` 里）——任务 2 加 `h1`；`Accepted` 字面量一处（`decrypt.rs:229`）——任务 4 加 `h2`；`Guard::new` 只有一处（`engine.rs:277`）；`serve_requests(` 两个调用（`crates/rurge-engine/src/serve.rs:377`、`:478`）——任务 4 各交一个布尔；`connect(` 只有 `redial` 一处（`engine.rs:794`）——任务 2 加 `h1_only`；断言 `client closed the connection` 的用例两条：`engine.rs:3165`（HTTP/1.1，照旧）与 `crates/rurge-engine/tests/http.rs:2185`（监听器的明文隧道 `a_client_that_leaves_mid_response_leaves_no_open_record`，`:2143`，与版本无关，照旧）。表里没有的只有这两类，都记在这里 | 本提交 |
| 21 | 0 | 名字对不上的三处，按代码做：一、P6 写的 `responses::not_implemented(MSG)` 今天没有——`crates/rurge-inbound/src/responses.rs` 只有 `full`、`empty`、`connect_established`、`tiny_gif`、`proxy_auth_required`、`bad_request`、`error_page`（`:24` ～ `:91`），任务 1 照 `bad_request`（`:59-65`）的写法新加一个 `not_implemented`（501，文案 `CONNECT over HTTP/2 is not supported`）。二、任务 2 要用的 `Meter` 只有 `new` 与 `set` 两个公开方法，`count_up` / `count_down` 因此加在 `pool.rs` 的 `Meter` 上。三、hyper 的 `client::conn::http2::Connection` 读流上限的那个方法叫 `current_max_send_streams()`（`hyper-1.11.1/src/client/conn/http2.rs:254`），h2 的才叫 `max_concurrent_send_streams()`；P9 引的是 h2 的，没有要改的 | 本提交 |
| 22 | 0 | 门禁（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI；日志 `gate-task-0.log`）：`cargo fmt --all --check` 退出 0；`cargo clippy --all-targets -- -D warnings` 退出 0；`cargo test -j 8 --workspace --no-fail-fast` 跑了一次，其中 `rurge-dns` 的单元测试二进制以 `0xc0000374`（`STATUS_HEAP_CORRUPTION`）退出，那一次的结果是 90 个结果行、4675 通过、0 失败、13 忽略；单独重跑那一个二进制（`gate-task-0-rerun.log`）287 条全过，合起来 91 个结果行、4962 通过、0 失败、13 忽略，与基线（M2a 任务 8 修正第 1 轮的 91 / 4962 / 0 / 13）相同 | 本提交 |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
