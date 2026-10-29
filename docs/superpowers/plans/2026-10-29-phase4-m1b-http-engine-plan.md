# 阶段 4 / M1b「明文 HTTP 引擎」实施计划

> **执行方式**：交给 Pi，由协调的会话逐个任务派工、逐个任务评审（设计第 2 节、M1-D1；做法同 M1a）。
>
> - 协调的会话每个任务开一个 Pi 会话，交给它这个任务的全文与要用的接口（派工文件）。Pi 做完把报告写进派工文件指定的路径，在本地提交。
> - 协调者读改动、重跑门禁、评审。有问题回到同一个 Pi 会话修正，再评审；过了才派下一个任务。
> - 每个任务：先写失败的用例、看到红；再实现到绿；跑门禁；对照本任务的「判据」与「全局约束」逐条自查；最后提交（**只在本地，不推送**）。
> - 与计划的出入记进末尾的「执行期修正记录」，**不改计划的正文**。
>
> 没有等 CI 的停点：M1 没有平台代码（设计开头）。M1b 做完由项目所有者推送一次，协调者读回那次 CI（M1-D1）。

**目标**：判为 HTTP 的连接进一个 hyper 写的明文 HTTP/1.1 引擎，每个请求各自求规则、选策略、借或拨上游连接，各有一条记录；HTTP 监听器的绝对形式请求交给同一个引擎，今天的 `forward()` 退役。具体是：

- `rurge-http` 的第二部分：每条客户端连接的上游连接池；HTTP/1.1 的服务（每个请求的会话信息、逐跳的头、拒绝与失败的答复、时限、守卫）；升级（WebSocket）与隧道里的 `CONNECT`；借来的连接上的重发。
- 引擎：`connect_session` 拆成选路与拨号两步；每条客户端连接一个实现 `Upstreams` 的对象；嗅探判为 HTTP 的连接（与 `force-http-engine-hosts` 强制进来的非 TLS 连接）交给引擎；承载请求的连接只在一个请求也没有走到选路时补一条记录。
- 记录与 API：每条记录加 `method`、`url`、`statusCode`；借来的连接的 `remoteAddr`；每个请求一行会话日志，加方法与状态码。
- HTTP 监听器：绝对形式请求经 `Dialer::plain_requests` 交给引擎；主机是 fake 地址时反查回名字（R27）。
- 计数 `requests`；文档收尾。

**架构**：没有平台代码，全部在开发机上编译、运行。

- `rurge-http` 在 M1b 加四条依赖边：`rurge-config`、`rurge-net`、`rurge-inbound`、`rurge-proto`（只为 `RejectKind`）。新模块：`pool`（`Pool`、`Reusable`、`Meter` 与 `Metered`）、`headers`（逐跳的头，从 `rurge-inbound` 搬来）、`engine`（`Upstreams` 与相关类型、每条连接的 hyper 服务、每个请求的流程 `Requests::handle`）。它仍然不认识引擎：选路、拨号与转发都经 `Upstreams` 这个 trait 问引擎。
- `rurge-inbound`：`SessionHandle` 加两个侧槽（方法、状态码）；`ResponseBody` 的错误类型放宽；`Dialer` 加第三个带缺省实现的方法 `plain_requests`，与 `PlainRequests`、`ClientConn`、`Close`；HTTP 监听器的绝对形式请求交给 `plain_requests`，`forward()`、`origin_form`、`absolute_form` 退役。
- `rurge-net`：`Trail::set_peer`。
- 引擎：`connect_session` 拆成 `route_session` 与 `dial_route`，拒绝与失败先"不结束句柄"地交回、由答复的一方结束（P4）；新模块 `http.rs`：每条客户端连接的 `ConnUpstreams`（实现 `Upstreams`）、HTTP 监听器的 `PlainRequests` 实现、承载连接的记录；`serve_sniffed` 把判为 HTTP 的连接交给它。`rurge-api` 照旧只依赖 `rurge-engine` 与 `rurge-config`。

**技术栈**：hyper 1.11.1（`server::conn::http1`、`client::conn::http1`、`upgrade`）、hyper-util 0.1.20（`TokioIo`、`TokioTimer`）、http 1、http-body-util 0.1.5（`BoxBody`、`BodyExt`）、bytes、tokio、tokio-util（`CancellationToken`、`TaskTracker`）。都已在工作区与 `Cargo.lock` 里。不加外部包；`http-body` 不进工作区依赖，`Body` 与 `Frame` 用 hyper 的再导出（`hyper::body::{Body, Frame, Incoming}`，P2）。

**依据**：

- M1 细化设计（下面写"设计"指它）：`docs/superpowers/specs/2026-10-29-phase4-m1-session-entry-http-engine-design.md`。M1b 用到第 3、6.4、8 ～ 12、13 节第 4 ～ 6 条、第 14 节第 4、6 条、第 15、16 节与已决事项 M1-D12 ～ D25。**先读它第 20、21 节**：第 20 节是两份计划改了设计正文的地方（M1b 的订正写计划时补在那里），第 21 节是 M1a 执行期的订正。本计划改了设计正文的地方列在「计划期决定」里，以计划为准。
- 阶段 4 总设计：`docs/superpowers/specs/2026-10-29-phase4-http-engine-design.md` 第 6 节（HTTP 引擎）与第 9.1 节（请求记录）。
- M1a 的计划与它的「执行期修正记录」：`docs/superpowers/plans/2026-10-29-phase4-m1a-sniffing-plan.md`。M1a 做了什么、留下什么，见 `docs/status.md` 阶段 4 的 M1a 一节。
- hyper 1.11.1 的源码（本机 cargo 注册表，设计开头「调研」一行引的几处）：
  - `server/conn/http1.rs`：`Builder::{preserve_header_case, timer, header_read_timeout, keep_alive, half_close, serve_connection}`、`Connection::with_upgrades`、`UpgradeableConnection::graceful_shutdown`；
  - `proto/h1/conn.rs` 第 210 ～ 233 行（`header_read_timeout` 从读一个头开始计时，空闲的 keep-alive 连接也算）与第 487 ～ 504 行（`mid_message_detect_eof`）；
  - `client/conn/http1.rs`：`handshake`、`SendRequest::{ready, is_ready, is_closed, send_request, try_send_request}`、`Connection::with_upgrades`；`client/dispatch.rs` 的 `TrySendError::{take_message, message, into_error, error}`（请求还没写出时把它还回来）；
  - `error.rs`：`Error::{is_incomplete_message, is_canceled, is_user, is_closed, is_timeout}`；
  - `upgrade.rs`：`hyper::upgrade::on` 对请求与应答两侧。
- 手册：`profile/general.html`（`show-error-page`、`show-error-page-for-reject`、`force-http-engine-hosts`）、`policies/reject.html`（REJECT 一族）。原话在设计里。
- 写计划时对着 M1a 合并之后的 `main`（`376cb56`）核对过的事实写在「计划期决定」里，包括设计第 16 节 V11 ～ V16 的结论（P21）。

## 全局约束

**依赖**

- `rurge-http` 的 `[dependencies]` 在任务 1 加：`rurge-config`、`rurge-net`、`rurge-inbound`、`rurge-proto`、`hyper`、`hyper-util`、`http`、`http-body-util`、`bytes`、`tokio-util`、`tracing`，都是工作区依赖（`*.workspace = true`）；`tokio` 照旧。`[dev-dependencies]` 加 `rurge-net = { workspace = true, features = ["testing"] }`（照 `rurge-inbound` 的写法）与 `tokio` 的测试用法照现有的。
- 新的依赖边只有 `rurge-http → { rurge-config, rurge-net, rurge-inbound, rurge-proto }`（设计第 3 节、M1-D26，M1a 的 P2 留给 M1b 的那四条）。`rurge-engine → rurge-http` 在 M1a 已有。`rurge-tun`、`rurge-inbound`、`rurge-api` 不依赖 `rurge-http`。
- 不加外部包。`Cargo.lock` 的包数在 M1a 之后是 520 个，M1b 的每个任务之后照旧是 520 个。
- `unsafe_code`：不加放开的函数，`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 仍是 23 项。

**缺省的 `cargo test` 不碰系统**

- M1 没有平台代码、没有 L2 用例。不建网卡，不改系统的任何设置。
- 不执行 `netsh`、`route`、`ipconfig`、`ip`、`resolvectl`、`networksetup`，不读真实的网络环境。
- 虚拟网卡的用例只用 `rurge-tun` 的内存设备与引擎用例的 `Rig`（`crates/rurge-engine/tests/common/tun.rs`）。

**测试的约定**

`CLAUDE.md`「测试约定」整节适用。写经引擎的 TUN 用例之前，读 `docs/testing.md`「测试约定（全文）」里阶段 3 / M3b、M3c 两节与「阶段 4 / M1a 起：嗅探」一节。与本计划直接相关的：

- 只用回环与端口 0，失败时也不离开回环。用例绑不了 80，所以"目标是 80"的会话照 M1a 的 P14 走到回环上的服务器：规则把它送到一个 SOCKS5 策略（`rurge_proto::testing::FakeSocks5`，`Socks5Script.connect_to` 指向用例的回环服务器），或用 `force-http-engine-hosts` 让端口 0 的服务器的端口进引擎。经 HTTP 监听器的绝对形式请求直接写回环服务器的端口（`http://target.test:<端口>/`，`target.test` 经 MockDns 落在 `127.0.0.1`）。
- 写了主机名的 Profile 另给第二道防线：经引擎的 TUN 用例用 `guarded`；监听器与 SOCKS5 的用例里，`target.test` 与 `alt.test` 经 MockDns 落在 `127.0.0.1`，别的名字加一行 `[Host]` 钉到 `127.0.0.1`。
- 嗅探的等待照 M1a 调短：`EngineShared { sniff_wait: Duration::from_millis(300), ..EngineShared::default() }`。HTTP 的请求在客户端连上之后马上发，嗅探在第一个请求的开头就判出 `Http`，用例不必等 300 毫秒。
- 空闲时限（`RuntimeOptions.idle_timeout`，引擎里的请求头、等应答头与正文的时限都取它）在要等它到的用例里调短，用任务 4 在 `crates/rurge-engine/tests/common/mod.rs` 加的夹具（P15）。
- 数连接：用任务 1 在 `rurge-net::testing` 加的数接受次数的回环服务器（P16），不靠日志或时间。
- 等待都有时限。固定等待只许"断言什么都没发生"，注释写它等的是什么、余量多大。
- 新写的、带等待的用例提交前单独循环跑二十次以上：任务 1 的池、任务 2 与 3 的引擎单元用例、任务 4 ～ 7 的引擎用例。
- 抓日志用 `LogCapture`，跑在当前线程的运行时上（普通的 `#[tokio::test]`）。
- 测试里手写的 HTTP 客户端把每个请求——请求头与正文——在一次写里发出（`CLAUDE.md`）。
- **内联的测试模块放在文件的末尾**：`crates/rurge/tests/sockets.rs` 读生产代码，读到第一个 `#[cfg(test)] mod … {` 为止；那之后再出现顶格的条目会让它变红。

**门禁与这台开发机**（每个任务末尾一次；说"通过"之前必跑）

- 门禁：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`。
- 本机 msvc 工具链没有 rustfmt。跑 `cargo fmt` 之前让环境变量 `RUSTFMT` 指向 gnu 工具链里同版本的 rustfmt：用户目录下的 `.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe`（同版本，输出与 CI 一致）。不要为此安装或升级工具链。
- 聚焦地跑用例也经整个工作区：单元测试 `cargo test -j 8 --workspace --lib -- <过滤词>`；集成测试 `cargo test -j 8 --workspace --test <名字> -- <过滤词>`。
- **不用 `-p`**：它另编一份依赖，`target/` 涨得很快，写满过 D: 盘。
- 跑全量之前 `df -h /d`：剩余不足约 15 GiB 就停下报告。
- 测试二进制以 `STATUS_HEAP_CORRUPTION`（本机管控软件注入的钩子 DLL）或 `STATUS_ACCESS_VIOLATION` 退出：单独重跑那一个二进制，报告里写明。`cli` 的用例偶发 `give_up` 20 秒超时（M5a 延后第 37 行那种卡住，M1a 的修正记录第 23 行）：单独重跑 `cli`，把标准错误原样抄进报告。
- rustc 在增量编译里报内部编译器错误：删掉出错的那个目标在 `target/debug/incremental/` 下的目录（可再生的缓存）再编。
- 回环的 TCP / UDP 7、9、13、17、19 号端口在这台机器上开着，CI 上关着：用例不假定任何固定端口的状态。"一个没有应答的地方"是用例自己持有的套接字。
- 结果照实记：本机的全量运行是"一台机器上的一次运行，不是 CI"；需要参照服务端的互操作用例在本机立刻返回、算在通过的数里，要说出来。

**仓库的规矩**（`CLAUDE.md`「必须遵守的规矩」整节适用；与本计划直接相关的）

- 日志（设计第 12 节）：
  - 每个请求一行会话日志，级别照旧：完成的在 DEBUG，被拒与失败的在 INFO；加 `method` 与 `status`；
  - 路径只在 DEBUG 的行里，写成 `scheme://主机[:端口]/路径`，**不写查询串与 userinfo**；请求头与正文在任何级别都不进日志；
  - 虚拟网卡上的名字与地址照旧只在 DEBUG；进程与网络的信息照旧只在 DEBUG。
- 凭据：没有新的带凭据的字段。`Proxy-Authorization`（客户端给 rurge 的、客户端给它自己的代理的、rurge 给上游 HTTP 代理的）与 URL 的 userinfo 不进日志、错误文本与 API；记录的 `url` 去掉 userinfo（查询串留着，设计第 19 节 Q1 的临时做法）。
- 诊断：不加新码。
- 出站套接字只有一处建法：上游连接一律经引擎的拨号（`dial_route`）建，`rurge-http` 只在拨好的流上跑 hyper 的客户端，不建套接字、不问系统解析库、不建连接器。若改动让 `crates/rurge/tests/sockets.rs` 变红，停下报告。池里的连接活得比一次请求长：键里有出口表，出口表变了不再借出（设计第 8.5 节；`CLAUDE.md` 那一条对活得长的套接字的要求）。
- `rurge-tun` 不改。停止取消正在转发的会话，不取消正在进行的拨号；结束一个会话靠取消它的令牌，不靠丢掉拨号或转发的 future——引擎里的请求同样：拨号不与停止竞速，被 hyper 丢掉的请求由守卫结束句柄（设计第 8.11 节）。
- 文档与报告用写文件 / 改文件的工具改。**永远不要用把一个文件的内容经管道写回同一个文件的命令**（`Get-Content x | Out-File x -Append` 一类；2026-10-27 这样写满过 D: 盘）。

**写法**

- 注释、日志与错误文本用英文，照所在文件的写法：文档注释写"为什么"与前提，句子完整。注释里提到设计或计划，写 `phase 4 M1 design 8.5`、`the phase 4 / M1b plan's execution record row N`。
- 提交说明照 `git log` 的样子写，标题用中文：`feat(http): …`、`feat(inbound): …`、`feat(net): …`、`feat(engine): …`、`feat(api): …`、`test(engine): …`、`refactor(engine): …`、`docs: …`。正文说做了什么、改了哪些现有用例的期望、为什么。Pi 的提交不带署名行。
- 只在本地提交：不 `push`、不 `pull`、不 `fetch` 远端、不 `amend`、不改写历史。

**不做的事**

- 只运行本仓库构建出来的二进制与 Pi 自己。不下载、不安装任何东西（含 `rustup target add`）。
- 不手动跑 `rurge run`、`rurge dns lookup`；`rurge service` 只用 `--dry-run`。
- 参考实现：GPL 的（sing-box、mihomo、Xray-core）只取事实，代码不抄。RFC 是公开的事实。
- 不做设计第 1 节「不做」一栏的事：HTTP/2、解密、重写、Map Local、抓包、`Via` 与 `X-Forwarded-For`、跨客户端连接共享上游连接。

**计划没有写到、要做决定的地方**

- 能按设计与总设计的原则定的：自己定，记进修正记录，写"执行者的裁定"。
- 去向写着"项目所有者"的（设计第 19 节），或会改变 M1-D1 ～ D27 的：停下来列出，交给协调者。
- 现有用例变红：本计划点了名的照计划改（P11），在提交说明里说；没点名的停下来，记进修正记录，不顺手改期望值。

## 评审要点

设计没写、而最可能咬到使用者的几种输入，与负责它们的任务和用例：

1. **记录一直开着。** hyper 在客户端走了时丢掉请求的 future；拨号不与停止竞速；升级之后的转发在别的任务里。
   - 期望：每个走到选路的请求，不论怎样结束，句柄都结束一次，记录不留在活动列表里。
   - 用例（任务 2、3、7）：`a_request_hyper_drops_is_finished_by_its_guard`、`a_client_that_leaves_mid_response_leaves_no_open_record`、`a_killed_request_closes_its_connection`、`stopping_a_connection_ends_its_requests`。
2. **一条上游连接借给了不该借的请求**（另一个策略、另一代配置、出口表换过之后）。
   - 期望：键不相等就不借；重载、`select` 换了成员、出口表变了之后旧连接不再借出。
   - 用例（任务 1、7）：`a_connection_is_lent_to_its_own_key_only`、`a_reload_or_a_new_egress_table_lends_no_old_connection`、`two_requests_of_one_connection_follow_their_own_rules`。
3. **字节记错**：借来的连接上的字节记到上一个请求、升级之后记两次。
   - 期望：池连接的计数层只记到"当前请求"；升级时清空，之后由 `relay` 记。
   - 用例（任务 1、3、7）：`a_metered_stream_counts_onto_the_current_request_only`、`an_upgraded_connection_is_counted_once`、`each_request_counts_its_own_bytes`。
4. **非幂等的请求被重发**（`POST` 发了两次）。
   - 期望：还没写出的任何方法重发一次；写出了的只有幂等且没有正文的重发一次；其余 502。
   - 用例（任务 3）：`a_request_not_yet_written_is_sent_again_on_a_new_connection`、`an_idempotent_request_cut_off_is_sent_again_once`、`a_post_cut_off_after_it_was_written_is_a_502`。
5. **头漏出去**：客户端给 rurge 的 `Proxy-Authorization` 到了上游；逐跳的头穿过；`Upgrade` 被去掉让 WebSocket 不通。
   - 期望：设计第 8.7 节的名单与两处不同。
   - 用例（任务 2、6）：`hop_by_hop_headers_are_dropped_both_ways`、`proxy_authorization_is_forwarded_only_on_a_sniffed_connection`、`the_listener_consumes_its_proxy_authorization`。
6. **凭据与查询串进了日志。**
   - 期望：路径只在 DEBUG，不带查询串与 userinfo；头与正文不进日志；记录的 `url` 去掉 userinfo。
   - 用例（任务 5）：`the_request_log_line_shows_no_query_and_no_userinfo`、`a_url_with_userinfo_is_recorded_without_it`。
7. **时限**：请求头、应答头、正文、池里的空闲连接。
   - 期望：设计第 8.10 节的表。
   - 用例（任务 1、2、7）：`a_connection_idle_for_ninety_seconds_is_not_lent`、`a_late_response_head_is_a_504`、`a_stalled_body_fails_the_request`。
8. **承载连接的记录**：一个请求也没有走到选路的连接（发半个头就关、读不懂、fake 地址反查不到）。
   - 期望：补一条失败的记录；走到过选路的连接不另记。
   - 用例（任务 4、7）：`a_connection_that_never_reaches_the_rules_leaves_one_failed_record`、`a_connection_whose_requests_were_routed_leaves_no_record_of_its_own`。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `crates/rurge-net/src/{trail.rs,testing.rs}` | 1 | `Trail::set_peer`；数接受次数的回环服务器（P16） |
| `crates/rurge-inbound/src/{session.rs,responses.rs,http.rs}` | 1 | 句柄的方法与状态码侧槽；`ResponseBody` 的错误类型 |
| `crates/rurge-http/{Cargo.toml,src/lib.rs,src/pool.rs（新）}` | 1 | 依赖；池、`Reusable`、`Meter` 与 `Metered` |
| `crates/rurge-inbound/src/{session.rs,lib.rs}`、`crates/rurge-http/src/{headers.rs（新）,engine.rs（新）,lib.rs}` | 2 | `Close`；逐跳的头；`Upstreams` 与 HTTP/1.1 的服务 |
| `crates/rurge-http/src/engine.rs` | 3 | 升级、隧道里的 `CONNECT`、重发 |
| `crates/rurge-engine/src/{engine.rs,http.rs（新）,serve.rs,outbounds.rs,lib.rs}`、`crates/rurge-engine/tests/common/mod.rs`、P11 点名的现有用例、`tests/http.rs`（新） | 4 | 选路与拨号两步；`ConnUpstreams`；嗅探到的 HTTP 进引擎；承载连接的记录 |
| `crates/rurge-engine/src/{observe.rs,engine.rs}`、`crates/rurge-api/src/routes/requests.rs`、`crates/rurge-api/tests/api.rs` | 5 | 记录的新字段、`RequestJson` 的新键、每个请求的日志行 |
| `crates/rurge-inbound/src/{session.rs,lib.rs,http.rs,testing.rs}`、`crates/rurge-engine/src/http.rs`、`crates/rurge-engine/tests/http.rs` | 6 | `plain_requests`；监听器的绝对形式请求交给引擎；R27；`forward()` 退役与它的用例搬家 |
| `crates/rurge-engine/tests/http.rs` | 7 | 端到端 |
| `crates/rurge-engine/src/{serve.rs,http.rs}`、`crates/rurge-api/src/routes/rurge_http.rs`、`tests/api.rs`、文档（任务 8 列全） | 8 | 计数 `requests`；收尾 |

模块的名字与拆法可以照实际调整，调了记进修正记录。

---

## Task 0：开工前核对

**做什么**：对着当时的分支核对下面每一项。

- 有出入的，记进「执行期修正记录」，一项一行，写"计划写 X，代码是 Y，按 Y 做"；按记录调整后面的任务。
- **不改计划的正文。**
- 没有出入也记一行。

1. **工作树**：`git status --short` 空；当前分支 `phase4-m1b`；`376cb56`（M1a 合并之后的 `main`）在历史里，之后只多了本计划的提交。
2. **读**：设计第 3、6.4、8 ～ 12、13 ～ 16、18 ～ 21 节；总设计第 6、9.1 节；本计划的「计划期决定」；`docs/status.md` 阶段 4 的 M1a 一节。
3. **名字与签名**：用 `git grep -n` 逐项找到，签名与「计划期决定」写的相同。
   - `rurge-net`：`trail.rs` 的 `Trail`（`new`、`follow`、`peer`、`clear_peer`、`dns`）与 `connected`；`connector.rs` 的 `Route`（`egress`、`PartialEq`）、`BoxedStream`、`Target`；`socket.rs` 的 `SocketHook::egress_route`；`testing.rs` 的 `TestServer`。
   - `rurge-inbound`：
     - `session.rs`：`SessionHandle` 的全部公开方法（尤其 `new`、`new_with_slot`、`token`、`kill`、`was_killed`、`trail`、`mark_connected`、`mark_first_byte`、`add_up`、`add_down`、`on_finish`、`finish`、`is_finished`）、`SessionOutcome`、`Counting`、`FailKind`、`Dialed`、`DialError`、`ServeOpts`、`trait Dialer`；
     - `responses.rs`：`ResponseBody`、`full`、`empty`、`tiny_gif`、`error_page`、`ErrorPage`、`bad_request`、`connect_established`；
     - `http.rs`：`serve_connection`、`handle`、`session_for`、`connect`、`HOP_BY_HOP`、`host_port`、`strip_hop_by_hop`、`origin_form`、`absolute_form`、`failure_response`、`forward`、`cancelled_outcome`、`HandlerError`，与 `mod tests` 的每一条用例（P11 的表）；
     - `listener.rs`：`ListenerOpts`（`show_error_page`、`show_error_page_for_reject`、`drop_hold`、`handshake_timeout`）；
     - `testing.rs`：`FakeDialer`；`lib.rs` 的导出。
   - `rurge-http`：`lib.rs` 的导出、`Prefixed`、`sniff`、`Sniffed`。
   - `rurge-engine`：
     - `engine.rs`：`connect_session`（逐段对着设计第 8.4 节）、`connect_through`、`fail`、`reject`、`log_session`、`new_handle`、`new_handle_with`、`look_up_owner`、`session_token`、`tracker`、`snapshot`、`choose_policy`、`Chosen`、`policy_known`、`impl Dialer for Engine`、`Engine.me`、`Engine.escalation`、`REFUSED_STRICT`、`EVALUATION_FAILED`、`CONNECT_TIMEOUT`；
     - `serve.rs`：`sniffable_session`、`serve_sniffed`、`SniffCounts`、`HttpCounts`、`SniffedCounts`；
     - `fake_address.rs`：`turn_back`、`Turned`；
     - `outbounds.rs`：`EngineFactory` 的 `hook` 字段；`runtime.rs`：`Runtime.factory`、`Runtime.idle_timeout`、`Runtime.config`；
     - `observe.rs`：`RequestRecord`、`record_of`、`RequestLog`（`mark_active`、`record_finished`、`kill`）；
     - `relay.rs`：`pump`；`smart.rs`：`attempts`、`unsendable`、`watch`、`retryable`；
     - `tests/common/mod.rs`：`Harness`、`Profile`、`harness`、`harness_with`、`harness_edited`、`harness_hooked`、`runtime`、`runtime_edited`、`connect_via_socks5`、`get`、`http_exchange`、`the_records`、`wait_until`、`origin_addr`；`tests/common/tun.rs`：`Rig`、`guarded`、`tun_records`、`WITHIN`。
   - `rurge-api`：`routes/requests.rs` 的 `RequestJson` 与 `From<&RequestRecord>`；`routes/rurge_http.rs`；`tests/api.rs` 里断言请求记录的键的用例。
   - `rurge-config`：`session.rs` 的 `SessionInfo`（`url` 的注释在第 74 ～ 79 行）、`General` 的 `show_error_page`、`show_error_page_for_reject`、`force_http_engine_hosts`。
   - `rurge-policy`：`Resolution`、`TerminalKind`、`SmartPick`；`rurge-proto`：`RejectKind`、`reject.rs`、`OutboundError`。
   - hyper 1.11.1：「依据」一节列的每一个名字在注册表的源码里找到。
4. **数**：`Cargo.lock` 里 `[[package]]` 的个数（520）；`LISTED` 的项数（23）。
5. **现有用例**（P11）：P11 列的每一条都找到，读它断言什么。把"预计要改 / 预计照过"逐条写进修正记录，一条用例一句；与 P11 的预计不同的，写理由。
6. **门禁**：跑一遍，记下测试二进制数、通过数、失败数与忽略数。

**判据**：修正记录里有任务 0 的行；门禁的数字记下。

---

## Task 1：`Trail::set_peer`、句柄的两个侧槽、`ResponseBody`、上游连接池

**改哪里**（设计第 8.4、8.5、8.7、10.1 节；M1-D14、D15、D17、D23；P2、P5 ～ P7、P16）

1. `rurge-net` 的 `trail.rs`：`Trail::set_peer(&self, addr: SocketAddr)`，把 `addr` 记成这条 trail 的对端，不经任务局部的作用域；IPv4 映射的地址照 `connected` 记成 IPv4（把那一段换算提成一个私有函数，两处共用）。文档注释：引擎里借到池里连接的请求，记下那条连接拨号时的远端地址（`phase 4 M1 design 8.4`）。
2. `rurge-inbound`：
   - `responses.rs`：`pub type BoxError = Box<dyn std::error::Error + Send + Sync>;`，`ResponseBody` 改成 `BoxBody<Bytes, BoxError>`；`full` 与 `empty` 的 `map_err` 随之改。
   - `http.rs`：`forward()` 交回上游应答时把 `hyper::Error` 换成 `BoxError`（`body.map_err(…).boxed()`）。别的不动（`forward()` 在任务 6 退役）。
   - `session.rs` 的 `SessionHandle` 加两个侧槽（P5）：
     - 方法：`set_method(&self, method: &str)`，只有第一次算；`method(&self) -> Option<String>`；
     - 状态码：`set_status(&self, code: u16)`，只有第一次算；`status(&self) -> Option<u16>`。
     - 文档注释：HTTP 引擎里的一个请求的方法与答给客户端的状态码（上游的，或引擎自己答的拒绝与错误页的），进记录与会话日志（`phase 4 M1 design 8.3, 10.1`）；不是 HTTP 请求的会话两个都是 `None`。
   - `lib.rs`：导出 `responses::BoxError`（`pub use responses::{BoxError, ResponseBody};` 或等价的写法，照现有的导出风格）。
3. `rurge-http`：
   - `Cargo.toml` 加「全局约束」里列的依赖。
   - `pool.rs`（新）：
     - `pub trait Reusable { fn is_closed(&self) -> bool; fn is_ready(&self) -> bool; }`：池里放的东西要能答这两问。本任务不为任何生产类型实现它；任务 2 为引擎里池的那一项（握着 hyper 的 `SendRequest`）实现，转调 `SendRequest` 同名的两个方法。
     - `pub struct Pool<K, C>`：一条客户端连接的空闲上游连接，按放回的先后存 `(键, 连接, 放回的时刻)`。`pub const MAX_IDLE: usize = 4;`、`pub const IDLE_FOR: Duration = Duration::from_secs(90);`。
     - `pub fn new() -> Pool<K, C>`；`pub fn take(&mut self, key: &K, now: Instant) -> Option<C>`：先丢掉放回已满 `IDLE_FOR` 的与 `is_closed()` 的，再取键相等、`is_ready()` 的第一条（取走）；`pub fn put(&mut self, key: K, conn: C, now: Instant)`：先同样清一遍；`conn.is_closed()` 的直接丢掉；放进去之后多于 `MAX_IDLE` 条就丢掉最早放回的；`pub fn len(&self) -> usize`。丢掉就是 drop（`SendRequest` 被丢掉时 hyper 关它的连接）。
     - 时刻由调用方给（`now`），池不读时钟：用例用可控的时刻（P6）。
     - `pub struct Meter`（`Clone`）：池里一条连接的"当前请求"，`Arc<Mutex<Option<Arc<SessionHandle>>>>`；`pub fn new() -> Meter`、`pub fn set(&self, handle: Option<Arc<SessionHandle>>)`。
     - `pub struct Metered<S>`，`pub fn new(inner: S, meter: Meter) -> Metered<S>`：包一条上游的流与一个 `Meter`，实现 `AsyncRead` / `AsyncWrite`（照 `rurge_inbound::Counting` 的写法）：读到 n 个字节时（n > 0）记到当前请求的 `add_down(n)` 并 `mark_first_byte()`；写出 n 个字节时记 `add_up(n)`；没有当前请求时什么也不记。文档注释：HTTP/1.1 上一条连接同一时刻只给一个请求用，借出时换成借它的请求、还回与升级时清空（`phase 4 M1 design 8.5`）。
   - `lib.rs`：`pub mod pool;`，导出 `Pool`、`Reusable`、`Meter`、`Metered`；文件头的 `//!` 改写成 M1b 之后的样子（它不再"只依赖 tokio"，仍然不认识引擎：选路与拨号经 `Upstreams`，任务 2 加）。
4. `rurge-net` 的 `testing.rs`：`TestServer` 的 `State` 加一个接受次数，接受循环每接受一条连接加一；`pub fn connections(&self) -> usize` 读它（P16）。文档注释：数的是 TCP 连接，不是请求（`hits` 数请求）。

**用例**

- `trail.rs`：`set_peer_notes_the_address_on_this_trail`：不在任何 `follow` 里调也记在这条 trail 上；IPv4 映射的记成 IPv4；之后 `clear_peer` 清掉。
- `session.rs`：`the_method_and_the_status_are_kept_once`：第二次 `set_method` / `set_status` 不改；新句柄两个都是 `None`。
- `responses.rs`：现有的两条照过（改了类型）。
- `pool.rs`（文件末尾的测试模块；连接用一个假的 `Reusable`，字段 `closed` 与 `ready` 可以改，带一个编号以便断言取到的是哪一条）：
  - `a_connection_is_lent_to_its_own_key_only`：放进键 A、B 各一条，取 A 得 A 的那条，再取 A 得 `None`；
  - `a_closed_connection_is_dropped_not_lent`：`closed` 的取不到，`len` 随之变小；放回一条 `closed` 的不留；
  - `a_connection_that_is_not_ready_is_skipped_and_kept`：`ready` 为假的不借、留着，变真之后借得到；
  - `at_most_four_idle_connections_the_oldest_goes`：放五条，最早的那条不在了；
  - `a_connection_idle_for_ninety_seconds_is_not_lent`：放回时刻是 `t`，`take` 在 `t + 89 秒` 借得到、在 `t + 90 秒` 借不到且被丢掉；`put` 同样清掉过期的。
- `Metered`：`a_metered_stream_counts_onto_the_current_request_only`（`tokio::io::duplex`）：当前请求是 A 时读写记到 A，换成 B 之后记到 B，清空之后谁也不记；第一个读到的字节标 A 的 `first_byte_seen`。
- `TestServer::connections()`：`it_counts_every_accepted_connection`（`rurge-net` 的 `testing.rs` 末尾的测试模块，或那个 crate 已有的放 `TestServer` 用例的地方；两条连接、其中一条上两个请求 → 数是 2，`hits` 是 3）。

**判据**

- 用例全绿；
- `Cargo.lock` 520；
- 门禁绿。

---

## Task 2：`rurge-http` 的 HTTP/1.1 服务：逐跳的头、`Upstreams`、每个请求的流程

**改哪里**（设计第 8.1 ～ 8.5、8.7、8.8、8.10、8.11 节；M1-D12 ～ D22；P1、P3、P4、P6、P7、P12、P13、P17、P18）

1. `rurge-inbound`：
   - `session.rs` 加 `pub struct Close;`（`Debug`，实现 `Display` 与 `std::error::Error`，文本 `connection closed by policy`，同今天 `HandlerError` 的文本）：服务不答、让 hyper 关掉连接（P13）。`lib.rs` 导出它。
   - 别的不动：监听器的 `HandlerError` 在任务 6 换成它。
2. `rurge-http/src/headers.rs`（新）：
   - 把 `rurge-inbound/src/http.rs` 的 `HOP_BY_HOP`、`host_port`、`strip_hop_by_hop` **抄**过来（`rurge-inbound` 里的那几份在任务 6 随 `forward()` 删掉；本任务两处并存）。
   - `pub fn strip(headers: &mut HeaderMap, keep: Keep)`，`pub struct Keep { pub upgrade: bool, pub proxy_authorization: bool }`（设计第 8.7 节）：
     - 先去掉 `Connection` 列出的名字，再去掉名单上的；
     - `keep.upgrade`：`Upgrade` 不去，`Connection` 去掉之后再放回恰好一个 `Connection: upgrade`；
     - `keep.proxy_authorization`：`Proxy-Authorization` 不去。
   - `pub fn host_port(authority: &Authority) -> String`（同今天，去掉 userinfo）。
   - 用例（文件末尾的测试模块）：`hop_by_hop_headers_and_the_ones_connection_names_go`；`an_upgrade_keeps_upgrade_and_connection_upgrade`；`proxy_authorization_stays_only_when_asked`；`host_port_has_no_userinfo`。
3. `rurge-http/src/engine.rs`（新）：下面的类型与函数；名字照写，私有的帮手函数自定。
   - **入口与连接**：
     ```rust
     /// Where the connection came in (design 8.1, 9).
     pub enum Entry {
         /// Sniffed as HTTP (or forced in, design 6.4): a request's target is the connection's.
         Sniffed,
         /// The HTTP listener's absolute-form requests: a request's target is its URL's.
         Listener,
     }

     /// One client connection the engine serves (design 8.1).
     pub struct Conn {
         /// src, in_port, local, listener, transport and, for `Sniffed`, the target and the SNI.
         pub session: SessionInfo,
         pub entry: Entry,
         /// `idle_timeout`: waiting for a response head, and between two frames of a body (design 8.10).
         pub idle: Duration,
         /// Reading a request head, the keep-alive idle included: `idle` for `Sniffed`, the listener's `handshake_timeout` for `Listener` (design 8.10, 9).
         pub head_wait: Duration,
         /// REJECT-DROP (design 8.8).
         pub drop_hold: Duration,
         /// The connection's token; each request's token is its child (design 8.11).
         pub token: CancellationToken,
         /// The engine's tracker: the upstream connections' hyper tasks and the relays of upgrades run on it.
         pub tracker: TaskTracker,
     }
     ```
   - **`Upstreams`**（引擎实现，每条客户端连接一个对象；P4、P17）：
     ```rust
     pub trait Upstreams: Send + Sync + 'static {
         /// The pool's key (design 8.5): equal keys may share an upstream connection.
         type Key: PartialEq + Send + 'static;
         /// What `route` worked out and `dial` needs; opaque here.
         type Route: Send + 'static;
         /// Held while a request whose target was a fake address lasts (design 9, R27).
         type Lease: Send + 'static;

         /// The name behind a fake address a request names (`Entry::Listener` only).
         fn named(&self, host: &HostName) -> Named<Self::Lease>;
         /// The request's handle, made before anything is awaited, so that the request's guard holds it from the start (design 8.11): numbered, registered, its token a child of `Conn::token`, the connection's process slot.
         fn open(&self, session: SessionInfo) -> Arc<SessionHandle>;
         /// The rules and the policy (design 8.4 "选路"). Refusals come back unfinished: the HTTP engine answers, sets the status and finishes the handle (P4).
         fn route<'a>(&'a self, handle: &'a Arc<SessionHandle>, forwardable: bool) -> BoxFuture<'a, Routed<Self::Route, Self::Key>>;
         /// Dials what `route` chose (design 8.4 "拨号"), in the request's own task.
         fn dial<'a>(&'a self, handle: &'a Arc<SessionHandle>, route: Self::Route) -> BoxFuture<'a, Result<Upstream, Refusal>>;
         /// Relays an upgraded exchange to its end and finishes `handle` (design 8.9).
         fn relay<'a>(&'a self, client: BoxedStream, upstream: BoxedStream, handle: Arc<SessionHandle>) -> BoxFuture<'a, ()>;
     }

     pub enum Named<L> { No, Name(String, L), Unknown }
     pub enum Routed<R, K> {
         Go { route: R, key: K, pages: Pages },
         Refused { refusal: Refusal, pages: Pages },
     }
     /// What answers a refusal: the generation the request was routed by says (design 8.8).
     #[derive(Clone, Copy, Debug)]
     pub struct Pages { pub error: bool, pub reject: bool }
     pub enum Refusal { Reject(RejectKind), Fail { kind: FailKind, message: String } }
     pub struct Upstream {
         pub stream: BoxedStream,
         /// `Some` when `stream` leads to an HTTP proxy that takes the request in absolute form: the headers to put on it (`Dialed::forward`).
         pub forward: Option<Vec<(String, String)>>,
         /// Where the dial connected to (the handle's `Trail`), kept with the pooled connection for the requests that borrow it (design 8.4, 10.1).
         pub peer: Option<SocketAddr>,
     }
     ```
   - **`Requests<U>`**：一条客户端连接上每个请求的流程。`pub fn new(conn: Conn, upstreams: U) -> Requests<U>`；`pub async fn handle(&self, req: Request<Incoming>) -> Result<Response<ResponseBody>, Close>`（返回的 future 是 `Send`）；`pub fn routed(&self) -> u64`（走到 `route` 的请求数，P10）。它是 `Clone`（里面一个 `Arc`），交给 hyper 的 `service_fn` 时每个请求克隆一份。HTTP 监听器在任务 6 直接调 `handle`。
   - **`pub async fn serve_connection<U: Upstreams>(client: BoxedStream, requests: Requests<U>) -> Served`**：设计第 8.1 节的 hyper 服务（`preserve_header_case(true)`、`timer(TokioTimer::new())`、`header_read_timeout(head_wait)`、`keep_alive(true)`、`with_upgrades()`）；连接令牌取消时 `graceful_shutdown()`、再等连接结束。`pub struct Served { pub routed: u64, pub error: Option<String> }`：`error` 是 hyper 交回的连接错误的文本（读不出请求头、请求头超时、客户端发了半个头就关），承载连接的记录用它（任务 4）。
4. **每个请求的流程**（`Requests::handle`，设计第 8.2 节；本任务不做升级、隧道里的 `CONNECT` 与重发——任务 3）：
   1. **会话信息**（设计第 8.3 节）：从 `Conn::session` 复制，`protocol = Some(Http)`。
      - 目标：`Sniffed` 是连接的目标；`Listener` 是 URL 的主机与端口（缺省 80），主机照今天 `forward()` 的写法 `HostName::parse(authority.host()).canonical()`（IPv4 映射的 IPv6 地址成 IPv4）。`Listener` 的主机先问 `named`：`Name` 时请求行的主机、与它相同的 `Host` 头都换成名字，再算 `url` 与 `http_host`（R27）；`Unknown` 时照原样建会话，第 3 步之后答失败 `fake address not known`（`Refusal::Fail { kind: Other, .. }` 的答法）。
      - `url`：绝对形式取请求行；源站形式 `http://` + `Host` 头 + 路径与查询串，没有 `Host` 时用连接的目标；都去掉 userinfo（`host_port`）。
      - `http_host`（P22）：绝对形式取请求行的主机，源站形式取 `Host` 头（照 `Authority` 解析）的主机；都转小写、不带端口；源站形式没有 `Host` 时 `None`。`user_agent`：`User-Agent` 照原样。
      - `forwardable`（`may_forward` 的一半，设计第 8.4 节）：`Listener` 的 `http://` 请求为真；`Sniffed` 上为假（源站形式的请求交给连接的目标；绝对形式的请求是客户端对它自己的代理说的，照原样交给连接的目标，设计第 8.9 节）。
   2. `open` → 句柄；立刻建这个请求的**守卫**（P18）：
      - 守卫被丢掉而句柄还没结束时结束它：被终止的（`was_killed`）→ `Failed("killed")`；连接令牌已取消（停止、守护进程退出）→ `Completed`；否则 → `Failed("client closed the connection")`。
      - 守卫随应答的正文交给 hyper（第 7 步），正文读完或出错时由正文结束句柄。
      - `handle.set_method(方法)`；`Named::Name` 的租约挂到 `handle.on_finish` 上丢掉。
   3. `route`（**不与令牌竞速**：它握着句柄，同第 7.1 节"拨号不竞速"；P1）。返回之后看令牌：已取消 → 照守卫的规则结束、`Err(Close)`。
      - `Refused` → 答复（第 6 步）。
   4. 池里有键相等、可用的连接（`Pool::take`）→ 借：`handle.mark_connected()`、`handle.trail().set_peer(那条连接的 peer)`（有的话），把那条连接的 `Meter` 换成这个请求的句柄。没有 → `dial`（同样不竞速，返回之后看令牌）；`Err(refusal)` → 答复（第 6 步）；`Ok(upstream)` → `Metered::new(upstream.stream, meter)`、在它上面 `hyper::client::conn::http1::Builder::new().preserve_header_case(true).handshake(…)`，连接的任务（`conn.with_upgrades()`，任务 3 的升级要它）在 `Conn::tracker` 上起，`SendRequest` 被丢掉或对端关闭时结束；握手失败 → 句柄 `Failed(format!("upstream handshake failed: {e}"))`、502（第 6 步的失败）。不用 hyper-util 的 `legacy::Client`：它的 `build_http(` 在 `crates/rurge/tests/sockets.rs` 的模式里，而且它自己建连接。
   5. **改写并发出**（设计第 8.7 节）：
      - 逐跳的头：`headers::strip(…, Keep { upgrade: false, proxy_authorization: entry == Sniffed })`。
      - `Listener`：`upstream.forward` 是 `Some` → 绝对形式（照今天的 `absolute_form`：URI 去掉 userinfo、`Host` 跟着目标、`forward` 的头替换同名的头）；`None` → 源站形式（照今天的 `origin_form`：URI 只剩路径与查询串、`Host` 换成目标的主机与端口）。改写失败照今天：句柄 `Failed(format!("bad request uri: {e}"))`、400 `malformed request URI`。
      - `Sniffed`：请求行与 `Host` 照客户端写的。
      - 请求的正文包一层空闲时限（两帧之间 `Conn::idle`，到了以错误结束，hyper 关上游连接），交给 `SendRequest`；等应答头与令牌竞速、并有 `Conn::idle` 的时限：
        - 令牌先到 → 照守卫的规则结束、`Err(Close)`；
        - 时限先到 → `Failed("upstream response timed out")`，504（第 6 步）；
        - 出错 → 不是 `is_user()` 的先 `handle.mark_upstream_failed()`，再 `Failed(format!("upstream request failed: {e}"))`、502（同今天 `forward()`）。
      - `Listener` 以绝对形式交给 HTTP 代理而它答 407：照今天 `forward()` 那一段（`Failed("http proxy answered 407 Proxy Authentication Required")`，502，信息只来自状态码）。
   6. **答复**（设计第 8.8 节；读 `Pages`）。先 `handle.set_status(码)`（关连接的不设），再结束句柄，再返回：
      - `Reject(Reject | NoDrop)`：`pages.reject` → 403 错误页（照今天 `forward()` 的 `ErrorPage`：标题 `Request rejected`、信息 `The request was rejected by the {kind} policy.`），否则 `Err(Close)`；结局 `Rejected(kind)`。
      - `Reject(TinyGif)`：200 与 `responses::tiny_gif()`；结局 `Rejected(TinyGif)`。
      - `Reject(Drop)`：先结束句柄（`Rejected(Drop)`），再停到 `drop_hold`、与请求的令牌竞速，然后 `Err(Close)`。
      - `Fail { kind, message }`：`pages.error` → 502 错误页（照今天的 `failure_response`：标题 `Connection failed`，按 `FailKind` 的三句前缀），否则 `Err(Close)`；结局 `Failed(message)`。
      - 等应答头超时：`pages.error` → 504（标题 `Gateway timeout`），否则 `Err(Close)`。
      - 错误页里的规则与策略链取句柄上的（`handle.rule()`、`handle.policy_chain()`）；页上的信息文字照今天 `forward()` 各处的写法：失败是 `{三句前缀之一}: {message}`，握手失败 `Upstream handshake failed: {e}`，发出失败 `Upstream request failed: {e}`，407 是 `Upstream proxy rejected the request: 407 Proxy Authentication Required`；结局的文字照今天小写开头（`upstream handshake failed: {e}` 等）。现有用例断言这些文字，改了它们会红（P11）。
   7. **应答**：`handle.set_status(上游的码)`；逐跳的头去掉（同第 5 步，`proxy_authorization` 不保留）；正文包一层（`Tail`）：
      - 两帧之间 `Conn::idle` 的时限，到了以错误结束（hyper 关客户端连接），句柄 `Failed("upstream body stalled")`；
      - 请求的令牌取消时以错误结束，句柄照守卫的规则；
      - 读完（`poll_frame` 给 `None`，或 `is_end_stream()`）→ 句柄 `Completed`；`Meter` 清空；`SendRequest` 没有关闭（`!is_closed()`）就 `Pool::put(键, 连接, 现在)`；
      - 出错 → 句柄 `Failed(format!("upstream body failed: {e}"))`，连接不回池；
      - 被丢掉而没读完（客户端走了）→ 守卫的规则，连接不回池。
5. **池**：`Requests` 里一个 `Arc<Mutex<Pool<U::Key, Pooled>>>`；`Pooled { sender: SendRequest<ReqBody>, meter: Meter, peer: Option<SocketAddr>, forward: Option<Vec<(String, String)>> }`，为它实现 `Reusable`（转调 `SendRequest::is_closed` 与 `is_ready`）。`ReqBody` 是 `BoxBody<Bytes, BoxError>`。
6. `lib.rs`：`pub mod engine; pub mod headers;`，导出 `Conn`、`Entry`、`Upstreams`、`Named`、`Routed`、`Pages`、`Refusal`、`Upstream`、`Requests`、`Served`、`serve_connection`；`//!` 改写（第二部分：HTTP/1.1 引擎；选路、拨号与转发经 `Upstreams` 问引擎）。

**用例**（`rurge-http/src/engine.rs` 末尾的测试模块，普通的 `#[tokio::test]`）

- 夹具：
  - `FakeUpstreams`：`Key = u32`、`Route = u32`、`Lease = ()`；脚本给每个请求的 `Routed`（按请求的 URL 路径挑）；`dial` 连到用例的回环源站（`TcpStream::connect`，测试代码不受 `sockets.rs` 管），数被调了几次；`open` 用 `SessionHandle::new_with_token(编号, 会话, 连接令牌的孩子)`，把句柄记下来供断言；`relay` 用 `tokio::io::copy_bidirectional` 并结束句柄。
  - 源站：任务 1 的数接受次数的服务器（P16），或用例自己的回环监听器（要停住、半途关的那几条）。
  - 客户端：`tokio::io::duplex` 的一端交给 `serve_connection`，另一端由用例写请求（每个请求一次写完）、读应答（有界）。
- `a_request_goes_through_and_its_handle_finishes`：200、正文到客户端；句柄 `Completed`、`method` 是 `GET`、`status` 是 200、`up` 与 `down` 不为零、`connect_time` 与 `first_byte_time` 有值。
- `two_requests_on_one_connection_share_an_upstream_connection`：同一个键的两个请求，`dial` 一次、源站接受一次；第二个句柄 `connect_time` 有值、`trail().peer()` 是源站的地址。
- `a_connection_is_lent_to_its_own_key_only`：两个键 → 两次 `dial`。
- `the_session_of_a_request_says_what_the_rules_see`：`Sniffed` 的源站形式请求（`url` 由 `Host` 重建，查询串在，`http_host` 小写无端口，目标是连接的）、`Sniffed` 的绝对形式请求（`url` 取请求行、`forwardable` 为假）、`Listener` 的绝对形式请求（目标是 URL 的、缺省端口 80、userinfo 去掉、`forwardable` 为真）。从 `FakeUpstreams` 记下的会话与 `forwardable` 断言。
- `a_fake_address_is_turned_back_into_its_name`（`Listener`）：`named` 给 `Name("site.test", ())` → 会话的目标、`url`、`http_host` 与发出的 `Host` 都是名字；`Unknown` → 502、句柄 `Failed("fake address not known")`、不 `dial`。
- `hop_by_hop_headers_are_dropped_both_ways`：源站收到的请求里没有 `Connection` 与它列出的名字、`Keep-Alive`、`Proxy-Connection`、`TE`；客户端收到的应答里同样没有。
- `proxy_authorization_is_forwarded_only_on_a_sniffed_connection`：`Sniffed` 上源站收到它；`Listener` 上收不到。
- `refusals_are_answered_as_the_pages_say`：`Reject` + `pages.reject` → 403 页；`Reject` 不带 → 连接关闭、没有应答；`TinyGif` → 200 与 43 字节；`Fail` + `pages.error` → 502 页、不带 → 关闭；每种的句柄结局与 `status`（关闭的没有）。
- `a_dropped_request_is_held_then_closed`：`Reject(Drop)`、`drop_hold` 300 毫秒 → 不早于 300 毫秒、五秒之内连接关闭；句柄一开始就 `Rejected(Drop)`。
- `a_late_response_head_is_a_504`：源站读完请求不答，`Conn::idle` 300 毫秒 → 504；句柄 `Failed`、`status` 504。
- `a_stalled_body_fails_the_request`：源站答头与半个正文就停住 → 客户端读到连接关闭（有界）；句柄 `Failed("upstream body stalled")`；源站那条连接不回池（下一个请求另拨）。
- `a_request_hyper_drops_is_finished_by_its_guard`：源站读完请求不答，客户端关掉 → 句柄 `Failed("client closed the connection")`（有界地等）。
- `a_killed_request_closes_its_connection`：等应答时 `handle.kill()` → 客户端连接关闭；句柄 `Failed("killed")`。
- `stopping_a_connection_ends_its_requests`：等应答时取消连接令牌 → `serve_connection` 返回（有界）；句柄 `Completed`；空闲的 keep-alive 连接在令牌取消时也立刻返回。
- `a_connection_that_reaches_no_route_says_why`：客户端发一行不是 HTTP 的字节 → hyper 答 400、`Served { routed: 0, error: Some(_) }`；正常的两个请求之后 `routed` 是 2。

**判据**

- 用例全绿；新写的、带等待的用例单独循环二十次（`cargo test -j 8 --workspace --lib -- engine::`）；
- `Cargo.lock` 520；
- 门禁绿。

---

## Task 3：升级、隧道里的 `CONNECT`、借来的连接上的重发

**改哪里**（设计第 8.6、8.9 节；M1-D18、D19；P1）

1. **升级**（`Upgrade` 头的请求，WebSocket）：
   - 第 5 步的逐跳的头改为 `Keep { upgrade: true, .. }`；
   - 拆开请求之前取客户端一侧的 `hyper::upgrade::on(&mut req)`；上游答 101 时，拆开应答之前取上游一侧的；
   - 在 `Conn::tracker` 上起一个任务：等两侧升级完成 → `upstreams.relay(客户端, 上游, handle)`（它结束句柄）；任一侧升级失败 → 句柄 `Failed(format!("upgrade failed: {e}"))`；
   - 101 交给客户端；`Meter` 清空；这条上游连接不回池；守卫交给那个任务（句柄由 `relay` 结束）。
   - 上游没答 101：照普通请求走完。
2. **隧道里的 `CONNECT`**（只在 `Sniffed`；设计第 8.9 节）：
   - `CONNECT` 照原样转给连接的目标（会话信息照第 4 步第 1 小步，`url` 是 `None`、`http_host` 取请求行的主机）；
   - 上游答 2xx：同上，两侧升级之后 `relay`；
   - 不是 2xx：应答照原样交给客户端，句柄照普通请求结束。
   - `Listener` 上的 `CONNECT` 不会到这里（监听器自己处理）：到了就答 400、句柄 `Failed`，写一行注释说明。
3. **重发**（设计第 8.6 节）：
   - 发请求用 `SendRequest::try_send_request`，把结果分成三种（私有的 `enum Sent { Answered(Response<Incoming>), NotWritten(Request<ReqBody>, hyper::Error), Failed { error: hyper::Error, written: bool } }`）；
   - **借来的**连接：
     - `NotWritten` → 换一条新拨的连接（`dial`）重发一次，任何方法；
     - `Failed { written: true }` 而 `error.is_incomplete_message()`、方法幂等（`GET`、`HEAD`、`OPTIONS`、`TRACE`、`PUT`、`DELETE`）、请求没有正文（`Incoming::is_end_stream()` 或 `size_hint().exact() == Some(0)`）→ 用发出之前留的那份请求头（方法、URI、版本、头）配空正文，换一条新拨的连接重发一次；
   - 其余（新拨的连接上失败、非幂等、有正文、第二次又失败）→ 502。
   - 留请求头的那一份只在借来的连接、幂等、没有正文时留（省得每个请求都复制头）。
   - 重发换了连接：`handle.trail().set_peer` 换成新连接的，`Meter` 换到新连接上。

**用例**（同一个测试模块）

- `a_websocket_upgrade_is_relayed_and_counted_once`：源站答 101 之后回显；客户端在升级之后写、读回；句柄 `status` 101、`Completed`（客户端关掉之后）、字节只记一次（`up` 等于请求头与升级之后写的字节之和这一类精确的断言照实算，或断言没有翻倍）；`Upgrade` 与 `Connection: upgrade` 到了源站。
- `a_connect_on_a_sniffed_connection_goes_to_its_target`：`CONNECT x.test:443` → 源站（扮演客户端自己的代理）答 200 → 隧道两头互通；句柄的 `status` 200。
- `a_connect_refused_by_the_target_is_answered_as_it_is`：源站答 403 → 客户端收到 403 与正文。
- `a_request_not_yet_written_is_sent_again_on_a_new_connection`：直接测 `send` 的那一层：一条连接的 `Connection` 任务已经结束（丢掉）的 `SendRequest` → `NotWritten`，请求原样还回来。再加一条经 `serve_connection` 的：借出前源站已关、`is_closed()` 还没变真的竞速不好造，不写；在用例的注释里说明。
- `an_idempotent_request_cut_off_is_sent_again_once`：源站第一条连接答完第一个请求、读完第二个请求就关 → `GET` 在新拨的连接上重发、客户端拿到 200；`dial` 两次。
- `a_post_cut_off_after_it_was_written_is_a_502`：同样的源站，第二个请求是带正文的 `POST` → 502；`dial` 一次。
- `a_request_on_a_new_connection_is_not_sent_again`：新拨的连接上同样被切断 → 502，`dial` 一次。

**判据**

- 用例全绿；带等待的单独循环二十次；
- 门禁绿。

---

## Task 4：引擎：选路与拨号两步、`ConnUpstreams`、判为 HTTP 的连接进引擎、承载连接的记录

**改哪里**（设计第 8.1、8.4、8.5、10.2 节；M1-D12 ～ D16；P3、P4、P8 ～ P10、P17、P19）

1. `engine.rs`：`connect_session` 拆成两步（P4）：
   - `pub(crate) enum Refused { Reject(RejectKind), Fail { kind: FailKind, message: String } }`。
   - `async fn route_session(&self, rt: &Arc<Runtime>, registry: &Arc<PolicyRegistry>, handle: &Arc<SessionHandle>, forwardable: bool) -> Result<Way, Refused>`：今天 `connect_session` 从开头到 `let attempts = …` 为止（暂停、`choose_policy`、`resolve_ready`、策略链与说明、目标与 `use-local-host-item-for-proxy` 钉住的地址、`may_forward`、`unsendable`、`attempts`）。`may_forward = forwardable && !pinned`（P3）。`Way { target, may_forward, attempts, book }`：`book` 是今天在循环之前取的 `registry.auto().smart.clone()`（P21 V11）；`host` 在 `dial_route` 里从句柄的会话重算。
   - `async fn dial_route(&self, handle: &Arc<SessionHandle>, way: Way) -> Result<(BoxedStream, Option<Vec<(String, String)>>), Refused>`：今天的尝试循环与失败的映射（`escalation` 照旧在 `OutboundError::Reject` 上；`Unsupported` 照旧 `set_error` 之后 `Reject(Reject)`）。
   - `fn settle(handle: &SessionHandle, refused: &Refused)`：`Fail` 时做今天 `fail()` 开头那一段（句柄上已有说明时把 `error` 设成 `"{note}; {message}"`），`Reject` 时什么也不做（今天的 `reject()` 不合说明）；只设 `error`、**不结束**。`fail()` 改成调它（那一段从 `fail()` 挪进 `settle`，不复制）。
   - `connect_session` 变成：`route_session` → `dial_route` → `Ok` 包成 `Dialed`；`Err(Fail { kind, message })` → `fail(handle, kind, message)`、`Err(Reject(kind))` → `reject(handle, kind)`，照今天结束（`fail` 里已经调了 `settle`，`connect_session` 不再另调，否则说明合两次）。今天的会话的行为、记录与日志一点不变——现有用例照过（P11）。
   - `forwardable` 在 `connect_session` 里照今天推断：`listener == Http && url.is_some()`（任务 6 之后不再有这样的会话，那时改成恒为假并删掉推断，记进修正记录）。
   - `new_handle_with` 加一个给定父令牌的变体 `new_handle_under(&self, session, process, parent: &CancellationToken)`（`new_handle_with` 转调它，给 `sessions_root`）。
2. `outbounds.rs`：`EngineFactory` 加 `pub(crate) fn egress_route(&self) -> Option<Route> { self.hook.egress_route() }`（P8）。
3. `http.rs`（新）：
   - `pub(crate) struct PoolKey { generation: Route, registry: Route, outbound: Route, chain: Vec<String>, target: Target, may_forward: bool, egress: Option<Route> }`，`#[derive(PartialEq)]`；`generation`、`registry`、`outbound` 用 `Route::of`（按指针比，并让旧的一代活到连接关掉，设计第 8.5 节）；`outbound` 与 `chain` 取第一个尝试的；`egress` 取这一代的 `factory.egress_route()`。
   - `pub(crate) struct ConnUpstreams { engine: Arc<Engine>, token: CancellationToken, process: ProcessSlot }`，实现 `rurge_http::Upstreams`：
     - `named`：恒为 `Named::No`（嗅探到的连接在开始时整条换过名字，第 4 项）；
     - `open`：`engine.new_handle_under(session, process.clone(), &token)`；
     - `route`：`snapshot()`；`requests` 计数加一（P19，任务 8 才有那个计数——本任务先留一行 `// phase 4 M1 design 11: counted here (task 8)`，不写空函数）；在 `handle.trail().follow(…)` 里 `route_session`；`Ok(way)` → `Routed::Go { key, pages, route: way }`；`Err(refused)` → `settle`、`Routed::Refused { refusal, pages }`。`pages` 读这一代的 `general.show_error_page` 与 `show_error_page_for_reject`。
     - `dial`：在 `handle.trail().follow(…)` 里 `dial_route`；`Ok` → `Upstream { stream, forward, peer: handle.trail().peer() }`；`Err` → `settle`、`Refusal`。
     - `relay`：`crate::relay::pump(client, upstream, handle, rt.idle_timeout)`。
   - `Refused` 与 `rurge_http::Refusal` 一一对应，写一个 `From`。
4. `serve.rs` 的 `serve_sniffed`（P9、P10）：嗅探之后、拨号之前，判为 HTTP 的连接（与 `force_http_engine_hosts.matches(主机, 端口) == Some(true)` 命中的 `Other`、`Closed`）交给新的 `serve_http`，别的照旧：
   - `fn serve_http(&self, session, client: Prefixed<…>, opts) -> …`：
     - 连接的会话先 `turn_back`（租约握到连接结束）；`Turned::Unknown` → 承载连接记一条失败（`FAKE_ADDRESS_NOT_KNOWN`）、关连接、返回；
     - `look_up_owner` 一次；`quic_adaptive.on_tcp` 一次（照 `dial` 里的那两行）；
     - 连接令牌 = `sessions_root.child_token()`；
     - `Conn { session, entry: Sniffed, idle: rt.idle_timeout, head_wait: rt.idle_timeout, drop_hold: opts.drop_hold, token, tracker }`；
     - `rurge_http::serve_connection(client, Requests::new(conn, upstreams))` 与 `stopped(opts.stop)` 竞速：停止先到 → 取消连接令牌、再等它结束（不丢掉）；
     - 结束后：`served.routed == 0` → 补承载连接的记录（第 5 项）。
   - `Tls` 与 `Timeout` 照旧照原样转发（P9）。
5. **承载连接的记录**（设计第 10.2 节；P10）：承载连接的句柄**不登记**：
   - 在 `serve_http` 开头用 `SessionHandle::new_with_slot(编号, 会话, 令牌, 进程槽)` 建（编号照 `next_session` 取），不调 `new_handle_with`（它登记、挂日志与记录的钩子）；
   - 连接结束时 `routed == 0`：挂上与 `new_handle_with` 相同的日志与记录的钩子、`mark_active`，再以 `Failed(原因)` 结束（原因：`served.error` 的文本，没有时 `"no request"`）——把挂钩子的那几行从 `new_handle_with` 提成一个私有函数，两处共用；
   - `routed > 0`：什么也不记。
6. 现有用例（P11）：M1a 之后，80 端口上经 `CONNECT` / SOCKS5 发明文 HTTP 的现有用例要改的，照 P11 的表改，提交说明里逐条说。
7. `crates/rurge-engine/tests/common/mod.rs`：调短空闲时限的夹具（P15）——把 `runtime_edited` 里的 `Duration::from_secs(600)` 提成参数：`pub async fn runtime_timed(dir, profile, shared, idle: Duration, edit)`，`runtime_edited` 转调它给 600 秒；`pub async fn harness_timed(p: Profile<'_>, shared: EngineShared, idle: Duration) -> Harness` 照 `harness_edited` 写、转调 `runtime_timed`。现有用例不变（这个文件开头的 `allow(dead_code)` 让没有用到它的测试二进制不报警告）。任务 7 用它。

**用例**（`crates/rurge-engine/tests/http.rs`，新；规则把会话送到回环上的 `FakeSocks5`，照 M1a 的 P14；嗅探等待 300 毫秒）

- `a_sniffed_http_connection_has_a_record_per_request`：经 SOCKS5 到 `target.test:80`，一条连接上两个请求 → 两条记录（`protocol` 是 `Http`、`rule`、`policy`），`FakeSocks5` 只被问一次（池）。
- `two_requests_of_one_connection_follow_their_own_rules`：`URL-REGEX,^http://target\.test/a,A` 与 `URL-REGEX,^http://target\.test/b,B`：`/a` 走 `A`、`/b` 走 `B`，各问一次。
- `force_http_engine_hosts_bring_another_port_into_the_engine`：`target.test:<回环源站的端口>` 命中 → 一条连接上两个请求两条记录。另一条连接在同一个端口上发 `hello\r\n\r\n`（判为 `Other`，被强制进引擎，设计第 6.4 节）→ hyper 答 400 并关连接，一条 `Failed` 的承载连接记录；没有设置时同样的字节照原样转发（不进引擎）。
- `a_tls_connection_is_still_relayed_as_it_is`：443 上的 TLS → 一条记录、`protocol` 是 `Https`。
- `a_connection_that_never_reaches_the_rules_leaves_one_failed_record`：发一行不是 HTTP 请求、却以方法开头的字节（例如 `GET \x01\r\n\r\n`；先确认 M1a 的嗅探判它为 `Http`、hyper 答 400）→ 一条记录、`Failed`、`protocol` 是 `Http`。
- `a_connection_whose_requests_were_routed_leaves_no_record_of_its_own`：两个请求之后关连接 → 恰好两条记录（再等 300 毫秒确认没有第三条：仅有允许的固定等待，注释写它等的是什么）。
- `stopping_the_adapter_ends_its_http_connections`：经虚拟网卡（`Rig`、`guarded`）的 80 上，一个请求在等应答（源站不答）时 `rig.tun.stop()` → 应用读到关闭（`WITHIN`）、记录 `Completed`。

**判据**

- 用例全绿；带等待的单独循环二十次（`cargo test -j 8 --workspace --test http`）；
- P11 表里"预计照过"的照过；
- `Cargo.lock` 520；
- 门禁绿。

---

## Task 5：记录的三个新键、借来的连接的 `remoteAddr`、每个请求的日志行

**改哪里**（设计第 10.1、12 节；M1-D20、D25；P5、P20）

1. `rurge-engine/src/observe.rs`：
   - `RequestRecord` 在 `protocol` 之后加 `pub method: Option<String>`、`pub url: Option<String>`、`pub status_code: Option<u16>`，文档注释照设计第 10.1 节（不是 HTTP 请求的会话三项都是 `None`；`url` 去掉 userinfo、带查询串；`status_code` 是答给客户端的码：上游的，或引擎自己答的拒绝与错误页的，关连接的没有）。
   - `record_of`：`method: h.method()`、`url: s.url.clone()`、`status_code: h.status()`。`url` 由 `rurge-http` 组、已经没有 userinfo（任务 2），这里不再处理；文档注释说出处。
2. `rurge-api/src/routes/requests.rs`：`RequestJson` 在 `protocol` 之后加 `method`、`url`、`status_code`（序列化成 `statusCode`，照这个结构体已有的 `camelCase`）；`From<&RequestRecord>` 照搬。`every_field_of_a_record_is_sent_under_its_own_name`（第 232 行起）的记录字面量与整段 JSON 随之加三项（P11）。
3. `rurge-engine/src/engine.rs` 的 `log_session`（P20）：
   - 三行（完成、被拒、失败）都加 `method = %…` 与 `status = %…`，放在 `elapsed_ms` 之后；不是 HTTP 请求的会话两项的值是空的（照 `rule = %rule` 的写法：`unwrap_or_default()`；状态码没有时写空）。
   - 只有完成的那一行（DEBUG）再加 `url = %…`：`SessionInfo.url` 去掉 `?` 与 `#` 之后的部分（一个私有函数，文档注释说为什么：查询串里常有令牌，设计第 12 节、总设计 P4-D25）；没有 `url` 时写空。被拒与失败的行在 INFO，不带它。
4. `crates/rurge-engine/tests/pipeline.rs` 的 `SESSION_FIELDS`（第 661 行）加 `"method"`、`"status"`、`"url"`（P11）；`a_field_of_a_log_line_is_read_whole` 的两行样例照新的行写，各加一句断言新字段读得出、`policy` 不吞掉它们。

**用例**（`crates/rurge-engine/tests/http.rs`；规则把会话送到回环上的 `FakeSocks5`）

- `a_request_record_has_its_method_url_and_status`：经 SOCKS5 到 `target.test:80`，一条连接上：`GET /a?x=1` → `method` 是 `GET`、`url` 是 `http://target.test/a?x=1`、`status_code` 是 200；`URL-REGEX,^http://target\.test/r,REJECT` 命中的 `GET /r`（`show-error-page-for-reject = true`）→ 403；`REJECT-TINYGIF` 命中的 → 200；`show-error-page-for-reject` 关着时 → `None`（连接被关）。
- `a_url_with_userinfo_is_recorded_without_it`：嗅探到的连接上一个绝对形式请求 `GET http://u:p@target.test/x HTTP/1.1` → `url` 是 `http://target.test/x`。
- `a_borrowed_connection_records_where_it_went`：同一个键的两个请求 → 两条记录的 `remote_addr` 都是 `FakeSocks5` 的地址（第二条是借来的连接的那一个，设计第 10.1 节）；第二条的 `connect_ms` 有值。
- `the_request_log_line_shows_no_query_and_no_userinfo`（`LogCapture`，DEBUG）：完成的那一行有 `method=GET`、`status=200`、`url=http://target.test/a`，整行没有 `x=1`、没有 `u:p`；被拒的那一行（INFO）有 `method` 与 `status`、没有 `url=`。
- `requests.rs` 的那条单元用例照第 2 项改过、绿。

**判据**

- 用例全绿；带等待的单独循环二十次；
- `pipeline.rs` 的四条日志用例（第 595、717、765、787 行起）照过；
- 门禁绿。

---

## Task 6：HTTP 监听器的绝对形式请求交给引擎；`forward()` 退役

**改哪里**（设计第 9 节；M1-D21、D22；P12、P13；R27）

1. `rurge-inbound/src/session.rs`：
   - `pub struct ClientConn { pub src: SocketAddr, pub in_port: u16, pub local: Option<SocketAddr> }`；
   - `pub trait PlainRequests: Send + Sync { fn handle(&self, req: Request<Incoming>) -> BoxFuture<'_, Result<Response<ResponseBody>, Close>>; }`；
   - `Dialer` 加第三个带缺省实现的方法 `fn plain_requests(&self, conn: ClientConn) -> Option<Arc<dyn PlainRequests>> { None }`，文档注释照设计第 9 节；`lib.rs` 导出两者。
2. `rurge-inbound/src/http.rs`：
   - `HandlerError` 换成 `Close`（P13）；
   - `handle`：鉴权照旧（验过之后去掉 `Proxy-Authorization`，不往下传）；`CONNECT` 照旧；绝对形式：先检查 scheme（不是 `http://` 的 400，照今天的文字，从 `forward()` 挪到这里），再问这条连接的 `PlainRequests`；
   - `Ctx` 加一个 `OnceLock<Option<Arc<dyn PlainRequests>>>`：这条连接上第一个绝对形式请求到来时才问 `dialer.plain_requests(ClientConn { src: peer, in_port: local.port(), local: here })`（P12）；`None` → 502（`responses::error_page`，标题 `Connection failed`，信息说这个监听器后面没有 HTTP 引擎；只有测试的假拨号器会这样）；
   - 删掉 `forward`、`origin_form`、`absolute_form`、`HOP_BY_HOP`、`host_port`、`strip_hop_by_hop`、`cancelled_outcome` 与因此不再用的 `use`；`failure_response` 留着（`connect()` 用它）。
3. 引擎（`http.rs`）：
   - 不另起类型：任务 4 的 `ConnUpstreams` 加一个字段 `listener: bool`（嗅探到的连接为假），只有两处看它：
     - `named`：为假时照旧 `Named::No`；为真时照 `turn_back` 的写法对这个主机问 fake 池（提一个两处共用的私有函数；`turn_back` 的行为不变）：池里有名字 → `Name(名字, 租约)`；是 fake 地址而没有名字 → `Unknown`；别的 → `No`；
     - `open`：为真时每个请求 `quic_adaptive.on_tcp` 一次（照 `dial` 里的那两行：监听器上一个请求就是今天的一个会话）；为假时不调（连接开始时调过一次，任务 4）。
     - `route`、`dial`、`relay` 两种一样。
   - `impl Dialer for Engine` 加 `plain_requests`：
     - 连接的会话：`SessionInfo::tcp(HostName::Ip(Ipv4Addr::UNSPECIFIED.into()), 0)`，再照监听器的 `session_for` 设 `src`、`in_port`、`local`、`listener: Http`、`transport: Tcp`。目标是占位：每个请求换成 URL 的（任务 2 第 4 步第 1 小步），文档注释说明。
     - 进程查一次：`look_up_owner(&那个会话)`。它不看目标：`crates/rurge-engine/src/owner.rs` 的 `query_for` 只读 `listener`、`transport`、`src`、`local`、`in_port`，`is_local` 只问 `src`（写计划时读过）。
     - 令牌 = `session_token()`；
     - `Conn { session: 那个会话, entry: Listener, idle: rt.idle_timeout, head_wait: HANDSHAKE_TIMEOUT, drop_hold: DROP_HOLD, token, tracker }`（监听器自己的 hyper 服务用它自己的 `handshake_timeout`；`head_wait` 在这条路上不用，写一行注释）；
     - 返回一个包着 `rurge_http::Requests<ConnUpstreams>`（`listener: true`）的 `PlainRequests`。
   - `connect_session` 里 `forwardable` 的推断（任务 4 留下的 `listener == Http && url.is_some()`）改成恒为假并删掉：经 `Dialer::dial` 来的会话都是隧道（设计第 8.4 节）。`rurge-config/src/session.rs` 第 74 ～ 79 行 `SessionInfo.url` 的注释照设计第 8.3 节改写。
4. 用例搬家（设计第 9 节末尾；P11 的表）：`rurge-inbound` 的 `http.rs` 里只讲转发的用例删掉，照它们断言的行为在引擎一层（`crates/rurge-engine/tests/http.rs` 的「监听器」一节）或 `rurge-http` 的单元用例里重写；只有一半讲转发的，那一半挪走、另一半留下。逐条见 P11 第 1 张表。

**用例**

- `rurge-inbound`：`an_absolute_form_request_without_an_engine_is_a_502`（`FakeDialer` 的缺省 `plain_requests`）；`non_proxy_requests_get_400` 照过（scheme 的检查挪了地方，文字不变）；留下的一半照过。
- 引擎（`tests/http.rs`「监听器」一节）：
  - `the_listener_rewrites_a_plain_request_for_the_origin`：源站收到 `Host: target.test:<端口>`、没有逐跳的头与 `Proxy-Authorization`、有 `User-Agent`；客户端收到的应答没有逐跳的头；同一条连接上的第二个请求 `REJECT-TINYGIF` → 43 字节的 GIF；两条记录（`protocol` `Http`、`url` 去掉 userinfo、`method`、`status_code`）。（原 `plain_requests_are_forwarded_per_request_with_rewritten_headers`）
  - `the_listener_answers_refusals_and_failures_as_the_profile_says`：REJECT 关连接、`show-error-page-for-reject` 时 403 页（含规则）；`REJECT-NO-DROP` 同 REJECT；DNS 失败的 502 页（含 `DNS lookup failed` 与目标）；`show-error-page = false` 时失败关连接。（原 `rejects_and_failures_render_pages_or_close`；REJECT-DROP 的 30 秒不在这里等）
  - `a_plain_request_is_killable`：源站读完不答，`engine.kill(编号)` → 客户端连接关闭、没有 502；记录 `Failed`、`error` 是 `killed`。（原 `plain_forward_session_is_killable`）
  - `a_client_leaving_mid_upload_is_no_upstream_failure`：`POST` 声明 10 字节、只发 3 字节就走 → 记录 `Failed`，不算上游的失败（用 smart 组的书断言，照 `crates/rurge-engine/tests/smart.rs` 的写法；或断言句柄的 `upstream_failed()` 为假，看哪个在引擎一层看得到——任务 0 定，记进修正记录）。（原同名）
  - `a_plain_request_record_names_the_client_and_the_listener_port`：`src`、`listen_port`。（原 `a_session_names_the_client_and_the_listener_port` 的那一半）
  - `an_ipv4_mapped_url_host_is_dialed_as_ipv4`（原 `an_ipv4_mapped_target_is_dialed_as_ipv4` 的那一半；`[::ffff:127.0.0.1]` 的回环源站）。
  - R27：`a_fake_address_in_a_plain_request_is_turned_back_into_its_name`（`Rig` 起着、`rig.fakes("target.test")` 的地址写进 URL 与 `Host`）→ 源站收到的 `Host` 是名字、记录的 `dst` 与 `url` 是名字；`an_unknown_fake_address_in_a_plain_request_is_a_502`（池里没有名字的地址）→ 502、记录 `Failed`、`error` 是 `fake address not known`、没有拨号。
- `rurge-http`：`absolute_form` 的两条单元用例（原 `absolute_form_keeps_the_uri_and_applies_the_upstream_headers`、`the_upstream_headers_replace_same_name_headers_including_host`）在任务 2 已经照写在 `rurge-http` 里（第 5 步的绝对形式），本任务只删 `rurge-inbound` 的那两条。

**判据**

- 用例全绿；P11 第 2 张表里经 HTTP 监听器发绝对形式请求的现有用例照过（不照过的停下报告，不改期望）；
- `crates/rurge/tests/sockets.rs` 照过；
- 门禁绿。

---

## Task 7：端到端

**改哪里**：只改 `crates/rurge-engine/tests/` 与 `crates/rurge-api/tests/api.rs`（设计第 14 节第 4 条）。产品代码不改：照计划写了之后红、而原因在产品代码的，停下写进报告。

**用例**（`crates/rurge-engine/tests/http.rs`；规则把会话送到回环上的 `FakeSocks5`；嗅探等待 300 毫秒；要等时限的用 `harness_timed`）

1. `every_entry_brings_its_requests_into_the_engine`：SOCKS5 到 80、`CONNECT` 到 80、虚拟网卡到 80（`Rig`、`guarded`）、HTTP 监听器的绝对形式：每种入口一条连接上两个请求 → 各两条记录，`listener` 各是入口的，`method`、`url`、`status_code` 都在。
2. `two_requests_of_one_connection_follow_their_own_rules`：照任务 4 的那条，加 HTTP 监听器与虚拟网卡两种入口。
3. `the_upstream_connection_is_lent_and_counted`：同一个键的三个请求 → 源站 `connections()` 是 1（P16）、`FakeSocks5::requests()` 是 1。
4. `a_reload_or_a_new_egress_table_lends_no_old_connection`：
   - 两个请求之间 `swap_runtime`（同样的 Profile 文本）→ 第二个请求另拨（`FakeSocks5::requests()` 是 2）；
   - `harness_hooked` 给一个出口表可换的钩子（用例里的 `SocketHook`，`egress_route()` 返回它当时握着的 `Route::egress(&Arc<…>)`，用例可以换一个新的 `Arc`）→ 换了之后另拨。
5. `a_select_switch_lends_no_old_connection`：`select` 组换了成员之后的请求另拨，走新成员。
6. `each_request_counts_its_own_bytes`：同一条上游连接上两个请求，源站答的正文各 100 与 5000 字节 → 第一条记录的 `down` 小于 5000、第二条不小于 5000；两条的 `up` 都大于零。
7. `refusals_are_answered_on_every_entry`：嗅探到的连接与监听器上，REJECT（关 / 403）、`REJECT-TINYGIF`、DNS 失败（502 / 关），两个 `show-error-page` 设置各一遍。
8. `a_late_response_head_is_a_504`：`harness_timed` 300 毫秒，源站读完不答 → 504、记录 `Failed`、`status_code` 504；`show-error-page = false` 时关连接。
9. `a_client_that_leaves_mid_response_leaves_no_open_record`：源站慢慢写一个大正文，客户端读一点就关 → 记录 `Failed`、`error` 是 `client closed the connection`，`active()` 为空（有界地等）。
10. `a_request_killed_through_the_engine_closes_its_connection`：`engine.kill(编号)`（经嗅探到的连接）→ 客户端读到关闭；记录 `Failed`、`killed`。
11. `a_websocket_goes_through_every_entry`：用例自己的回环源站答 101、之后回显；经 SOCKS5 到 80 与 HTTP 监听器各一遍：升级之后写、读回；记录 `status_code` 101，`up` 与 `down` 不翻倍。
12. `chunked_bodies_go_both_ways`：`Transfer-Encoding: chunked` 的请求正文与应答正文各一遍，源站收到的与客户端收到的内容逐字节相同。
13. `expect_100_continue_is_answered`（P23）：经 SOCKS5 到 80，客户端先只写 `POST` 的请求头（带 `Expect: 100-continue` 与 `Content-Length`）→ 有界地读到 `HTTP/1.1 100 Continue`；再写正文 → 拿到 200，源站收到的正文逐字节相同、请求头里有 `Expect: 100-continue`。这一条讲的就是"先发头、等 100 再发正文"，是 `docs/testing.md`「一次写」那条规矩的例外（同 `a_late_body_does_not_cost_…` 一组），在用例的注释里说。另加一段：同样的请求被 `REJECT` 命中（`show-error-page-for-reject = true`）→ 客户端直接读到 403，之前没有 `100 Continue`。
14. `an_idempotent_request_cut_off_is_sent_again_through_the_engine`：源站第一条连接答完第一个请求、读完第二个就关 → 第二个 `GET` 在另一条连接上拿到 200；`connections()` 是 2。
15. `stopping_the_adapter_ends_its_http_connections`：照任务 4 的那条，加"空闲的 keep-alive 连接也立刻结束"。
16. `the_carrier_connection_is_recorded_only_when_nothing_was_routed`：发半个请求头就关 → 一条 `Failed` 的记录；两个请求之后关 → 恰好两条（再等 300 毫秒确认，注释照规矩写）。

`crates/rurge-api/tests/api.rs`：`requests_carry_the_method_the_url_and_the_status_code`：经 HTTP 监听器一个绝对形式请求与一个被拒的请求 → `GET /v1/requests/recent` 里两条的 `method`、`url`、`statusCode`（被拒而关连接的那条 `statusCode` 是 `null`）；键紧跟 `protocol`（照这个文件里断言键序的写法，或取原文看 `"protocol":…,"method":…`）。

**判据**

- 用例全绿；带等待的循环二十次（`cargo test -j 8 --workspace --test http`，与 `--test api -- requests_carry`）；
- 先看到红：本任务不改产品代码，红靠临时改期望看（照 M1a 任务 7 的做法），报告里写三四处；
- 门禁绿。

---

## Task 8：计数 `requests` 与文档收尾

**改哪里**（设计第 11、15 节；M1-D24；P19）

1. 引擎：`requests` 计数（`AtomicU64`，`Ordering::Relaxed`），在 `ConnUpstreams::route` 里加一（任务 4 留的那一行注释换成它；嗅探到的连接与监听器两种都算；走到选路的请求，被拒的也算，P19）；`HttpCounts` 加 `pub requests: u64`；`http_counts()` 填它。`tests/sniff.rs` 的 `the_sniffs_are_counted` 的字面量加 `requests: 1`（P11）。
2. `rurge-api`：`HttpJson` 在 `sniffed` 之后加 `requests`；`rurge_http_counts_start_at_zero` 的两处期望加 `"requests":0`（原文字节那一句一起改，P11）。
3. 用例：引擎 `the_requests_are_counted`（嗅探到的连接上两个请求、监听器上一个被拒的请求 → `requests` 是 3）。
4. 文档（照 M1a 任务 9 的写法与规矩：引的提交、用例名与数字对着仓库核过；历史的段落不改写；本地提交、CI 没有见过）：
   - `docs/surge-compatibility-matrix.md`：7.1 节"明文 HTTP 代理转发"一行（今天的 L570）改写：引擎、每条客户端连接的池、WebSocket、504、错误页的两个设置；L564（`force-http-engine-hosts`）与 L562 的"**目前（阶段 4 / M1a）**"改成 M1b 之后的样子；L265 的 **HTTP/1.1 `Host`** 一段：引擎里的请求带 `http_host`，`extended-matching` 对它匹配；`show-error-page` 与 `show-error-page-for-reject` 两行（`grep -n` 找）写 rurge 在引擎里的做法；设计第 13 节第 4 ～ 6 条登记在对应行的备注里；状态标记照 M1a 任务 9 的规矩（有登记的差异才改）；附录的统计用脚本整张重数；文件开头"最近一次"那一句。
   - `docs/api/phase4.md`：M1b 一节——`requests`；记录的 `method`、`url`、`statusCode`（键序、各自的意思、不是 HTTP 的会话是 `null`）；借来的连接的 `remoteAddr`、`connectMs`、`firstByteMs`；`up` / `down` 在 HTTP 请求上的含义（设计第 10.1 节）；承载连接的记录；每个请求的日志行（哪一级、哪几个字段、`url` 不带查询串与 userinfo）。
   - `docs/api/phase1.md`：「`remoteAddr`」一条里"复用已有连接的会话没有"接一句：经引擎的 HTTP 请求借来的连接有（指到 `phase4.md`）。
   - `docs/testing.md`：「命令」加 `rurge-http` 引擎的单元用例与 `--test http` 的过滤词；「测试约定（全文）」阶段 4 那一节加：`harness_timed`、`TestServer::connections()`、"一条 `CONNECT :80` 隧道一条记录"的用例在 M1b 改成每个请求一条（P11）。
   - `docs/status.md`：阶段 4 下加「### M1b（明文 HTTP 引擎）」一节（四段，照 M1a 一节的结构）；依赖的附录接一句：阶段 4 / M1b 加了 `rurge-http → { rurge-config, rurge-net, rurge-inbound, rurge-proto }` 四条边。
   - `CLAUDE.md`：「现在在哪里」阶段 4 那一条：M1 做完（本地提交、CI 没有见过）、下一步 M2；「架构」里 `rurge-http` 的依赖那一句照第 7 段的事实改（P20 那样的事实订正）。
   - `README.md`、`README_en.md`（两份一致）：状态、特性表、路线图里阶段 4 的一句（设计第 15 节）；`docs/requirements.md` 第 7 节阶段 4 的状态一句（FR 的字面不改，阶段栏归项目所有者）。
   - 设计第 21 节：在 M1a 的各条之后，加任务 0 ～ 8 的修正记录里改了设计正文意思的行，一条一段（写清改了正文哪一处、以哪一行为准）；没有就写一句"M1b 执行期没有改设计正文的意思"。第 20 节不动：本计划改了设计正文的地方（「计划期决定」里标了"订正设计"的各条）在本计划提交时已写在那里。
   - `docs/README.md`：M1b 计划那一项末尾接一句做完。

**判据**

- 用例全绿；文档里引的提交、用例名与数字对着仓库核过；
- 门禁绿。

---

## 计划期决定

写计划时对着 `376cb56` 核对过下面引的名字与行号。行号会随前面任务的改动漂移，以名字为准；名字不同就照实改、记进修正记录，不改语义。标了"订正设计"的，已随本计划写进设计第 20 节。

| # | 事项 | 决定 |
| - | ---- | ---- |
| P1 | 选路的三步与竞速 | 设计第 8.4 节把"选路"写成一步（`route`：建句柄、求规则、选策略）。这里分成 `open`（同步，建句柄）与 `route`（异步）：hyper 在客户端走了时丢掉请求的 future，丢在第一个 `await` 上时，引擎已经建好、登记好的句柄没有人结束，记录会一直开着（今天 `forward()` 里 `dial` 被丢掉就是这样）。`open` 先把句柄交给 `rurge-http`，守卫从那时起握着它（P18）。`route` 与 `dial` 都**不与令牌竞速**（同设计第 7.1 节"拨号不竞速"：它们握着句柄，在自己的时限里结束），返回之后看令牌。订正设计 |
| P2 | `http-body` | 不进工作区依赖。`Body`、`Frame`、`SizeHint` 用 hyper 的再导出（`hyper::body::…`），`BoxBody`、`BodyExt`、`Full`、`Empty` 用 `http-body-util`（已在工作区） |
| P3 | REJECT 在哪里定 | REJECT 一族是拨号时定的：`REJECT` 的出站从 `connect_tcp` 返回 `OutboundError::Reject(kind)`（`crates/rurge-proto/src/reject.rs:32`），`dial_route` 再经 `escalation` 升级（今天 `connect_session` 的最后一段）。所以 `route` 只拒失败（暂停、DNS、求值失败），`Routed::Refused` 里的 `Reject` 只有 `ConnUpstreams` 映射时用不到的一支；池里不会有"REJECT 的连接"（它没有流），键里有出站，别的出站的连接也借不到 REJECT 的请求上。`may_forward = forwardable && !pinned`（`forwardable` 由 `rurge-http` 给，设计第 8.4 节）。设计第 8.2 节的流程图与第 8.8 节的表把 REJECT 一族写成"`route` 的结果"：照这里，它们是选路或拨号的结果，答复一样。订正设计 |
| P4 | 拒绝与失败交回而不结束 | `route_session` 与 `dial_route` 把 `Refused` 交回、不结束句柄。今天的会话由 `connect_session` 照旧结束（`fail` / `reject`，行为不变）；引擎里的请求由 `rurge-http` 先设状态码、再结束，所以记录与会话日志带着码。`settle` 照今天 `fail()` 把句柄上已有的说明与失败合在一起。类型见任务 2、4。设计第 3 节的接口草图写"拒绝、失败（句柄已结束）"、`route` 收 `RequestFacts`：照这里与任务 2 的 `Upstreams`。订正设计 |
| P5 | 方法与状态码 | 放在 `SessionHandle` 的两个侧槽里（只有第一次算），不进 `SessionInfo`（规则不看方法，设计第 8.3 节） |
| P6 | 池的时刻 | `take` 与 `put` 的时刻由调用方给，池不读时钟，用例用可控的时刻。`MAX_IDLE` 4、`IDLE_FOR` 90 秒（设计第 8.5 节） |
| P7 | 上游的字节 | 池里的连接包一层 `Metered`，"当前请求"可换（`Meter`）：借出时换成借它的请求、还回与升级时清空。不用 `rurge_inbound::Counting`：它绑死一个句柄 |
| P8 | 出口表 | `EngineFactory` 加 `egress_route()`，转调它私有的 `hook`（`SocketHook::egress_route`）。经 `Runtime.factory` 拿到 |
| P9 | 进引擎的连接 | 判为 `Http` 的连接进引擎；`force-http-engine-hosts` 命中（`matches(主机, 端口) == Some(true)`）的再加 `Other` 与 `Closed`（设计第 6.4 节）。`Tls` 与 `Timeout` 照原样转发。连接的会话在进引擎之前 `turn_back` 一次（租约握到连接结束），`look_up_owner` 与 `quic_adaptive.on_tcp` 各一次 |
| P10 | 承载连接的记录 | 承载连接的句柄用 `SessionHandle::new_with_slot` 建、不登记；`serve_connection` 返回 `routed == 0` 时才挂上日志与记录的钩子、`mark_active`、以失败结束（设计第 10.2 节）。挂钩子的那几行从 `new_handle_with` 提出来两处共用 |
| P11 | 现有的用例 | 见下面两张表。表里"预计照过"的若红，停下报告，不改期望 |
| P12 | 监听器的 `PlainRequests` | 每条客户端连接在第一个绝对形式请求到来时才问 `plain_requests`（只发 `CONNECT` 的连接不必查进程、不必建池）；放在 `Ctx` 的 `OnceLock` 里。设计第 9 节写"每条客户端连接开始时"。订正设计 |
| P13 | `Close` | 在 `rurge-inbound`（`rurge-http` 依赖它，监听器也用它）。监听器的 `HandlerError` 换成它 |
| P14 | 规则看见什么 | 不改规则的代码：`URL-REGEX` 读 `url`、`USER-AGENT` 读 `user_agent`、带 `extended-matching` 的域名规则与 `URL-REGEX` 读 `http_host`（`crates/rurge-rules/src/matcher.rs` 第 369 ～ 393、531 ～ 538 行）。M1b 起引擎里的请求有这几项；`http_host` 要小写（那里的约定） |
| P15 | 空闲时限的夹具 | `runtime_timed` 与 `harness_timed`（任务 4），`runtime_edited` 转调它给 600 秒，现有用例不变 |
| P16 | 数连接 | `rurge_net::testing::TestServer` 加 `pub fn connections(&self) -> usize`：接受循环里每接受一条连接加一（设计第 14 节第 4 条、V15）。`TestServer` 不支持升级、流式的正文与停住：要这些的用例（WebSocket、半途停住、慢正文、读完不答）用用例自己的回环监听器 |
| P17 | 错误页的设置与停留 | `Pages` 读选路时那一代的 `general.show_error_page` 与 `show_error_page_for_reject`（设计第 8.8 节）；`drop_hold` 跟着入口：嗅探到的连接是 `ServeOpts.drop_hold`，监听器是 `DROP_HOLD`（`Engine::listener_opts` 给监听器的同一个值） |
| P18 | 守卫的结局 | 被终止的 → `Failed("killed")`；连接令牌已取消（停止、守护进程退出）→ `Completed`（同今天 `cancelled_outcome`）；否则 → `Failed("client closed the connection")`。设计第 8.11 节只写了"被终止的算终止，否则算失败"，停止与退出那一种照今天的 `cancelled_outcome` 补上。订正设计 |
| P19 | `requests` 计数 | 在 `ConnUpstreams::route` 里加一（嗅探到的连接与监听器共用这一个实现）：走到选路的请求，被拒的也算；读不出请求头的连接不算（它们在承载连接的记录里） |
| P20 | 日志行 | `method` 与 `status` 在三行里都有（不是 HTTP 的会话值是空的），放在 `elapsed_ms` 之后；`url`（去掉查询串与片段，userinfo 本来就没有）只在完成的那一行（DEBUG）。`pipeline.rs` 的 `SESSION_FIELDS` 随之加三项（P11） |
| P21 | 设计第 16 节 V11 ～ V16 | V11：两步之间传 `Way { target, may_forward, attempts: Vec<Resolution>, book: Arc<SmartBook> }`（书取自 `registry.auto().smart`，今天在循环之前取）；`new_handle_with` 用 `sessions_root.child_token()`，`SessionHandle::new_with_slot` 收令牌。V12：出口表见 P8；`TCP_NODELAY` 已开（`crates/rurge-net/src/connector.rs:532`），不用改。V13：hyper 1.11.1 的接口见「依据」。V14：`ResponseBody` 用在 `responses.rs` 与 `http.rs`；`failure_response` 的三句是 `DNS lookup failed`、`Connection timed out`、`Connection failed`。V15：见 P16。V16：`Engine::tracker()` 是 `pub`（`engine.rs:1361`）；`sessions_root` 是私有字段，`session_token()`（`pub(crate)`）给它的孩子 |
| P22 | 规则看见的 `http_host` | 设计第 8.3 节写"`Host` 头的主机部分"。绝对形式的请求改取请求行的主机：RFC 9112 第 3.2.2 节——代理收到绝对形式的请求时忽略收到的 `Host`、换成请求目标的主机；今天 `forward()` 也取 URL 的（`authority.host()`）。源站形式照设计取 `Host` 头。两种都转小写、不带端口。订正设计 |
| P23 | `Expect: 100-continue` | 读 hyper 1.11.1 的源码：服务端在服务第一次读请求正文、而应答还没开始写时，自动写 `HTTP/1.1 100 Continue`（`proto/h1/conn.rs` 第 405 ～ 415 行）；应答先写了（拒绝、错误页）就不写，正文没读完时关连接（同文件第 859 行起的 `poll_drain_or_close_read`）；客户端一侧解析应答时跳过 1xx（`proto/h1/role.rs` 第 1031 行起）。所以引擎不另做什么：把请求的正文交给上游一侧的 `SendRequest` 时客户端拿到 rurge 写的 100，上游自己写的 100 被 hyper 的客户端跳过；`Expect` 不是逐跳的头，照原样转给上游。任务 7 第 13 条照此断言 |

### P11 第 1 张表：`rurge-inbound` 的 `http.rs` 里的用例（任务 6）

行号是 `376cb56` 上 `mod tests`（第 590 行起）里的。

| 用例 | 讲什么 | 任务 6 怎样处理 |
| ---- | ------ | ---------------- |
| `connect_tunnels_to_the_dialed_stream`（693） | `CONNECT` | 照过 |
| `an_ipv4_mapped_target_is_dialed_as_ipv4`（722） | `CONNECT` 一半 + 转发一半 | 留 `CONNECT` 那一半；转发那一半挪到引擎（`an_ipv4_mapped_url_host_is_dialed_as_ipv4`） |
| `a_session_names_the_client_and_the_listener_port`（752） | `CONNECT` 一半 + 转发一半 | 留 `CONNECT` 那一半；转发那一半挪到引擎（`a_plain_request_record_names_the_client_and_the_listener_port`） |
| `rejected_connect_closes_without_a_response`（787） | `CONNECT` 的拒绝 | 照过 |
| `basic_auth_compares_the_password_only`（819）、`constant_time_compare_matches_plain_equality`（883） | 鉴权 | 照过 |
| `hop_by_hop_headers_and_the_tokens_connection_names_are_dropped`（891） | `strip_hop_by_hop` | 删；任务 2 在 `rurge-http` 的 `headers.rs` 有同样的与升级的一条 |
| `non_proxy_requests_get_400`（909） | 四种 400 | 照过（scheme 的检查挪进 `handle`，文字不变） |
| `stalled_http_handshakes_time_out`（928） | 监听器的请求头时限 | 照过 |
| `plain_requests_are_forwarded_per_request_with_rewritten_headers`（951） | 转发 | 删；引擎的 `the_listener_rewrites_a_plain_request_for_the_origin` |
| `rejects_and_failures_render_pages_or_close`（1037） | 转发的拒绝与失败 | 删；引擎的 `the_listener_answers_refusals_and_failures_as_the_profile_says` |
| `plain_forward_session_is_killable`（1105） | 终止 | 删；引擎的 `a_plain_request_is_killable` |
| `a_client_leaving_mid_upload_is_no_upstream_failure`（1163） | `is_user` 的错误 | 删；引擎的同名用例 |
| `an_http_authority_cannot_carry_control_characters_or_spaces`（1220） | `http` 库的保证 | 照过 |
| `absolute_form_keeps_the_uri_and_applies_the_upstream_headers`（1233）、`the_upstream_headers_replace_same_name_headers_including_host`（1267） | `absolute_form` | 删；任务 2 在 `rurge-http` 有同样的两条 |
| `a_sniffable_connect_is_answered_before_it_is_dialed`（1295）、`a_sniffable_connect_is_relayed_after_the_answer`（1315） | 先答后交 | 照过 |

### P11 第 2 张表：别处要改的、要留意的现有用例

| 用例 | 为什么 | 任务 | 预计 |
| ---- | ------ | ---- | ---- |
| `crates/rurge-engine/tests/sniff.rs` 的 `the_prefix_reaches_the_upstream_whole` | 443 上以 `GE` 开头、再写 `T / HTTP/1.1…` 的字节判为 `Http`，M1b 起进引擎，回显的上游答不出 HTTP | 4 | 改：第二次写的字节换成判为 `Other` 的（例如 `GEt / not http…`：`t` 是小写，`judge` 判 `Other`），两次写与 100 毫秒照旧，讲的仍是"前缀一个字节不丢" |
| 同文件 `the_sniffs_are_counted` | 第二个会话 `GET / HTTP/1.1` 进引擎；回显的上游答不出 HTTP，客户端拿到 502 页，不再是回显的 18 字节 | 4、8 | 任务 4：那一段的期望改成读到的开头是 `HTTP/1.1 502`；记录仍是 5 条（那个请求走到了选路，承载连接不另记）。任务 8：`HttpCounts` 的字面量加 `requests: 1` |
| 同文件 `a_plain_http_session_is_marked_http` | `CONNECT :80` 里一个请求 | 4 | 照过（一个请求一条记录、拨一次） |
| 同文件 `force_http_engine_hosts_sniff_another_port` | 第一段在强制的 8080 上写 `hello`：判为 `Other`，M1b 起被强制的 `Other` 进引擎（P9），hyper 等一个完整的请求头，要到 600 秒的空闲时限才断，用例 5 秒的界先到 | 4 | 改：第一段改写一个带 `Connection: close` 的请求（`GET / HTTP/1.1\r\nHost: target.test:8080\r\nConnection: close\r\n\r\n`），`reply[1]` 仍是 0x00（先答后读，讲的仍是"设置里的端口被嗅探"），读到的开头是 `HTTP/1.1 502`（`A` 拒绝，拨号失败，引擎答错误页）；后两段不变 |
| `crates/rurge-engine/tests/pipeline.rs` 的 `SESSION_FIELDS`（661）与 `a_field_of_a_log_line_is_read_whole` | 日志行多了三个字段 | 5 | 改：加三项；样例照新的行写 |
| `crates/rurge-api/src/routes/requests.rs` 的 `every_field_of_a_record_is_sent_under_its_own_name`（232） | 记录多了三项 | 5 | 改：字面量与 JSON 加三项 |
| `crates/rurge-api/tests/api.rs` 的 `rurge_http_counts_start_at_zero` | 多了 `requests` | 8 | 改：两处期望加 `"requests":0` |
| `crates/rurge-engine/tests/smart.rs` 的 `three_seconds_without_an_answer_count_against_the_member`（380） | `CONNECT :80` 里一个请求、源站不答，3 秒后记一次失败而记录仍在活动 | 4 | 照过：真拨号挂 `smart::watch`，等应答头的时限是 600 秒 |
| 同文件 `the_first_byte_back_is_reported`（315） | 要应答到达时标首字节 | 4 | 照过（`Metered` 标） |
| 同文件 `a_plain_request_whose_upstream_hangs_up_counts_against_the_member`（427） | 今天由 `forward()` 的 `mark_upstream_failed` 来 | 6 | 照过（任务 2 第 5 步保留它；新拨的连接不重发） |
| `crates/rurge-engine/tests/outbounds.rs` 的 `a_local_host_item_with_several_addresses_gives_the_proxy_one_at_random`（298） | 40 条隧道各一个请求，上游 40 次 | 4 | 照过（池属于一条客户端连接） |
| 同文件 `a_local_host_item_reaches_the_proxy_as_an_ip`（263）、`a_selection_applies_to_the_next_connection_and_survives_a_restart`（522）、`a_subnet_group_follows_the_network_the_engine_is_on`（579）；`network.rs` 的 `a_network_change_moves_the_subnet_group`（165）；`outbounds_ssh.rs` 的两条（55、89） | `CONNECT :80` 里一个请求，数上游 | 4 | 照过 |
| `outbounds.rs` 的 `a_plain_request_goes_to_an_http_upstream_in_absolute_form`（73）、`always_use_connect_and_other_protocols_tunnel_plain_requests`（114）、`a_forward_mode_407_is_a_failed_session_and_a_502_page`（212，不重发）、`a_pinned_host_turns_forwarding_into_a_tunnel`（345）、`rurge_talks_to_rurge`（467） | 监听器的绝对形式经 HTTP 代理与隧道 | 6 | 照过 |
| `outbounds_shadow_tls.rs:62`、`outbounds_tls_family.rs:290` | 监听器的绝对形式经 8080 | 6 | 照过 |
| `pipeline.rs` 的 218、249、414、472、495、530、563、595、717、765、787、1070、1091、1325、3158、3230、3448、3678 各条 | 监听器的绝对形式：记录、日志、拒绝与错误页、规则 | 5、6 | 照过（日志的四条在任务 5 改了 `SESSION_FIELDS` 之后） |
| `crates/rurge/tests/cli.rs` 的 2925、2978、3081、3411、3493、3685、4103、5090、6458、6505、6555、6625、6666、7094、8132、8194 各条；`run/enhanced_mode.rs:1104` | 守护进程经监听器的绝对形式 | 6 | 照过（`http_get` 带 `Connection: close`） |
| `crates/rurge-api/tests/api.rs` 的 748、800、899、934、992、1207、1330、1490、1530、2354 各条 | 经监听器的绝对形式：记录的键、流量 | 5、6 | 照过 |
| `tests/interop/tests/sing_box.rs:45` | 直接用 `rurge-proto` 的 `HttpForward` | — | 照过（`HttpForward` 不动） |

---

## 执行期修正记录

| # | 任务 | 内容 | 提交 |
| - | ---- | ---- | ---- |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
