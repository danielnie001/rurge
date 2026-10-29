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

下面写"执行者的裁定"的，是 Pi 在设计与计划的原则下定的；写"协调者的裁定"的，是协调的会话代项目所有者定的。项目所有者都可以改。计划的正文不改：正文里说得不对的地方，以这张表的行为准。

| # | 任务 | 内容 | 提交 |
| - | ---- | ---- | ---- |
| 1 | 0 | 第 1 项（工作树）与第 2 项（读）合并：`git status --short` 空；分支 `phase4-m1b`，`HEAD` = `6c3ad20`；`git log --oneline 376cb56..HEAD` 两行——`a92221b`（本计划、设计第 20 节第 12 ～ 18 条与 `docs/README.md` 的一项）与 `6c3ad20`（修正记录表头之前的一段说明），`376cb56` 在历史里，其后没有别的提交；本地 `main` 也在 `376cb56`（本分支未推送）。读过：设计第 3、6.4、8 ～ 12、13 ～ 16、18 ～ 21 节（第 20、21 节先读）、总设计第 6 与 9.1 节、本计划「计划期决定」P1 ～ P23 与 P11 的两张表、`docs/status.md` 阶段 4 / M1a 一节。无出入 | 本提交 |
| 2 | 0 | 第 3 项（`rurge-inbound`）：名字都在，签名与 P4、P13、P21 写的相同。`session.rs`：`pub trait Dialer: Send + Sync`（`:465`）今天六个方法——`dial`（`:466`）、`relay`（`:467`）、`sniffable`（`:477`，缺省 false）、`serve`（`:486`，缺省 dial → relay、REJECT-DROP 时 `hold_dropped`）、`admit_udp`（`:528`，缺省 `NotSupported`）、`associate`（`:536`，缺省什么都不做）；任务 1 的 `plain_requests` 是第四个带缺省实现的方法。`SessionHandle`（`:31`）：`new(id, session)`（`:65`，没有令牌参数）、`new_with_token(id, session, token)`（`:71`，P4 没点名、今天已有）、`new_with_slot(id, session, token, process: ProcessSlot)`（`:82`，收令牌与进程槽、返回 `Arc<SessionHandle>`）；方法侧槽与状态码侧槽今天都没有，任务 1 加。`ServeOpts`（`:451`）三个字段 `stop`、`drop_hold`、`linger` 与 P4 相同。`Counting`（`:327`）、`FailKind`（`:381`：Dns / Connect / Timeout / Other）、`Dialed`（`:388`：stream / handle / forward）、`DialError`（`:398`：Reject{kind, rule, handle} 与 Failed{kind, message, rule, handle}）。`responses.rs`：`pub type ResponseBody = BoxBody<Bytes, hyper::Error>`（`:9`，任务 1 放宽错误类型）、`full`（`:18`）、`empty`（`:22`）、`connect_established`（`:28`）、`tiny_gif`（`:35`）、`proxy_auth_required`（`:44`）、`bad_request`（`:53`）、`ErrorPage`（`:61`）、`error_page`（`:85`）。`http.rs`：`serve_connection`（`:90`）、`handle`（`:141`）、`session_for`（`:161`）、`connect`（`:171`）、`HOP_BY_HOP`（`:276`，9 项）、`host_port`（`:290`）、`strip_hop_by_hop`（`:299`）、`origin_form`（`:317`）、`absolute_form`（`:340`）、`failure_response`（`:375`）、`forward`（`:397`）、`cancelled_outcome`（`:581`）、`HandlerError`（`:47`，只有 `Close` 一支、`pub(crate)`）；`mod tests` 起于 `:590`。`listener.rs` 的 `ListenerOpts`（`:40`，七个字段，`drop_hold` 与 `handshake_timeout` 的缺省都是 30 秒在 `:53` 起）。`testing.rs` 的 `FakeDialer`（`:20`）、`new`（`:30`）、`sessions`（`:40`）、`echo_server`（`:154`）与 `lib.rs` 的导出（`:18` ～ `:26`）今天都没有 `PlainRequests`、`ClientConn`、`Close`，任务 2、6 加。与派工清单不同的一处是 `http_exchange` 的所在（第 7 行） | 本提交 |
| 3 | 0 | 第 3 项（`rurge-net`）：名字都在。`trail.rs` 的 `Trail`（`:23`）：`new`（`:34`）、`follow`（`:40`）、`peer`（`:45`）、`clear_peer`（`:50`）、`dns`（`:55`），自由函数 `connected(addr)`（`:67`）与 `looked_up(spent)`（`:77`）；`set_peer` 今天没有，任务 1 加（P4、设计第 8.4 节）。`connector.rs`：`pub type BoxedStream = Box<dyn AsyncStream>`（`:23`）、`Target { host, port }`（`:89`）、`Route`（`:204`）的 `of`（`:212`）、`egress`（`:223`）、`is_egress`（`:233`）与 `impl PartialEq`（`:238`，按对象指针比）。`socket.rs` 的 `pub trait SocketHook`（`:69`）的 `egress`（`:81`）与 `egress_route(&self) -> Option<Route>`（`:92`，缺省 None）。`testing.rs` 的 `TestServer`（`:64`）：`spawn`（`:72`）、`spawn_tls`（`:76`）、`spawn_tls_with`（`:82`）、`url`（`:127`）、`set`（`:132`）、`set_status`（`:144`）、`set_delay`（`:150`）、`set_header`（`:156`）、`set_validators`（`:165`）、`requests`（`:171`）、`hits`（`:175`）、`handle`（`:345`）；`State`（`:59`）今天只有 `routes` 与 `requests` 两个字段，接受循环在 `start`（`:86`）里（每接受一条连接起一个 `auto::Builder` 的服务）——`connections()` 今天没有，任务 1 在 `State` 里加一个计数、在接受循环里加一（P16），用例侧照 P16 的样子读。`TestServer` 由 `#[cfg(any(test, feature = "testing"))]` 带进来（`lib.rs:15`） | 本提交 |
| 4 | 0 | 第 3 项（`rurge-http`、`rurge-config`、`rurge-policy` 与 `rurge-proto`）：名字都在。`rurge-http`：`lib.rs` 导出 `Prefixed`、`ClientHello`、`judge`、`sniff`、`Sniffed`、`Judged`、`MAX_PREFIX`、`SNIFF_WAIT`（`:11` ～ `:13`，比「计划期决定」列的多几个，语义不变）；`sniff.rs` 的 `Judged`（`:22`：`NeedMore(usize)` / Tls / Http / Other）与 `Sniffed`（`:75`：Tls / Http / Other / Timeout / Closed / Failed，第 20 节第 3 条的第六种）、`sniff`（`:91`）；依赖只有 `tokio`（`Cargo.toml:11`），任务 1 加派工给的四条。`rurge-config`：`SessionInfo` 的 `url` 注释在 `session.rs:74` ～ `:79`（与设计第 8.3 节说的一样），`General` 的 `force_http_engine_hosts`（`general.rs:202`）、`show_error_page`（`:208`）、`show_error_page_for_reject`（`:209`），两个错误页开关的缺省是 true 与 false（`:446`、`:447`）。`rurge-policy`：`TerminalKind`（`registry.rs:40`：Direct / Reject / Proxy）、`Note`（`:48`）、`Resolution`（`:124`：chain、outbound、terminal、note、pending、smart）、`SmartPick`（`:142`：group、member、retry）。`rurge-proto`：`RejectKind`（`outbound.rs:13`：Reject / Drop / NoDrop / TinyGif，`name()` 在 `:29`、`escalates()` 在 `:41`）、`OutboundError`（`:47`）；P3 引的 `reject.rs:32` 正是 `connect_tcp` 返回 `OutboundError::Reject(self.kind)` 的那一行 | 本提交 |
| 5 | 0 | 第 3 项（`rurge-engine`）：名字都在。`connect_session`（`engine.rs:1495` ～ `:1675`）逐段：文档与签名 `:1493` ～ `:1500`；暂停的检查 `:1501` ～ `:1508`（`fail(handle, FailKind::Other, REFUSED_STRICT)`）；`choose_policy` 与它的 `Chosen::DnsFailed` `:1509` ～ `:1512`；`SelectCtx` 与 `resolve_ready`（求值失败时记策略链再 `fail(EVALUATION_FAILED)`）、策略链与 `note` `:1513` ～ `:1531`；目标与 `use-local-host-item-for-proxy` 钉住的地址 `:1532` ～ `:1558`；`plain_http` 与 `may_forward`（今天由 `listener == Http && url.is_some()` 推出）`:1559` ～ `:1568`；`let book = registry.auto().smart.clone()` `:1569`；`unsendable` 与 `smart::attempts` `:1570` ～ `:1579`；`begun`、`tried`、`failure` `:1580` ～ `:1582`；逐个 `attempts` 拨号的循环 `:1583` ～ `:1646`（`clear_peer`、`k>0` 时换策略链、每次尝试分到的时限、`connect_through`、`mark_connected`、`book.used`、`smart::watch`、`report_failure` 与 `tried`、`retryable` 决定要不要再来一次）；“试了多个”的说明 `:1647` ～ `:1655`；最后一段 `:1656` ～ `:1674`——`OutboundError::Reject(kind)` 经 `self.escalation.refusal(...)`（`:1658` ～ `:1660`）升级后 `reject()`，`Unsupported` 记说明后 `reject(Reject)`，`Dns` / `Io` / `Timeout` / `Proxy` / `Tls` / `Unavailable` 各调 `fail(...)`。其余：`fail`（`:1428`）、`reject`（`:1476`）、`connect_through`（`:1455`）、`log_session`（`:1405`）、`new_handle`（`:476`）、`new_handle_with`（`:518`，令牌取 `self.sessions_root.child_token()`、没有给定父令牌的参数，任务 4 加一个变体）、`look_up_owner`（`:492`，`is_local(session.src.ip())` 在 `:499`）、`session_token`（`:459`，`pub(crate)`）、`tracker`（`:1361`，`pub`）、`snapshot`（`:322`）、`choose_policy`（`:813`）与 `choose_policy_with`（`:824`）、`Chosen`（`:889`）、`policy_known`（`:907`）、`Engine.me: std::sync::Weak<Engine>`（`:211`）、`pub(crate) escalation: Escalation`（`:185`）、`REFUSED_STRICT`（`:65`）、`EVALUATION_FAILED`（在 `auto.rs:28`，与 `resolve_ready` 同一文件）、`CONNECT_TIMEOUT`（`:46`）。`serve.rs`：`sniffable_session`（`:86`）、`serve_sniffed`（`:168`）、`SniffCounts`（`:25`）、`SniffedCounts`（`:62`）、`HttpCounts`（`:74`，今天只有 `sniffed` 一项）。`fake_address.rs`：`turn_back`（`:33`）与 `Turned`（`:13`：Unchanged / Name(Lease) / Unknown）。`observe.rs`：`RequestRecord`（`:28` ～ `:73`）、`record_of`（`:91`）、`RequestLog`（`:141`）的 `mark_active`（`:162`）、`record_finished`（`:180`）、`kill`（`:230`）。`relay.rs` 的 `pump`（`:41`）；`smart.rs` 的 `attempts`（`:28`）、`retryable`（`:63`）、`unsendable`（`:76`）、`watch`（`:128`）。`outbounds.rs` 的 `EngineFactory.hook: Arc<dyn SocketHook>`（`:36`，私有字段）；`runtime.rs` 的 `Runtime.config`（`:28`）、`idle_timeout`（`:32`）、`factory`（`:43`，`pub(crate)`）。用例夹具：`tests/common/mod.rs` 的 `Harness`（`:44`）、`Profile`（`:187`）、`harness`（`:218`）、`harness_with`（`:231`）、`harness_edited`（`:237`）、`harness_hooked`（`:279`）、`runtime`（`:82`）、`runtime_edited`（`:89`，`idle_timeout` 写死 600 秒）、`connect_via_socks5`（`:365`）、`get`（`:388`）、`plain_get`（`:652`）、`the_records`（`:418`）、`wait_until`（`:400`）、`origin_addr`（`:667`）；`tests/common/tun.rs` 的 `Rig`（`:44`）、`guarded`（`:31`）、`tun_records`（`:142`）、`WITHIN`（`:18`） | 本提交 |
| 6 | 0 | 第 3 项（`rurge-api`）：名字都在。`routes/requests.rs` 的 `RequestJson`（`:45`）二十四个字段（`id` 到 `notes`），今天没有 `method`、`url`、`status_code`（任务 5 加），`From<&RequestRecord>`（`:75`），用例 `every_field_of_a_record_is_sent_under_its_own_name`（`:232`）与记录字面量的辅助 `record`（`:201`）；`routes/rurge_http.rs` 的 `HttpJson`（`:12`）只有 `sniffed`，`SniffedJson`（`:20`）五个键，`http`（`:28`）——任务 8 加 `requests`。`tests/api.rs` 的 `rurge_http_counts_start_at_zero`（`:1113`）断言的两处 JSON 今天没有 `requests` | 本提交 |
| 7 | 0 | 第 3 项的一处出入：派工把 `http_exchange` 列在 `tests/common/mod.rs` 里，代码是 `crates/rurge-engine/tests/pipeline.rs:143` 的一个文件内私有辅助（写一次请求、按 `Content-Length` 读一条应答，3 秒的读时限），`tests/common/mod.rs` 里没有它；`common` 里近的是 `get`（`:388`）、`plain_get`（`:652`，带 `Connection: close` 与 `Proxy-Connection: keep-alive`，读到 EOF）与 `connect_via_http`、`connect_via_socks5`。按代码做：任务 4 ～ 7 写 `crates/rurge-engine/tests/http.rs` 时用 `common::get` / `common::plain_get`，要按正文长度读完整就照 `pipeline.rs:143` 的样子在 `http.rs` 里自备一个（每个测试文件本来就各有一份）。语义不改 | 本提交 |
| 8 | 0 | 第 3 项的另一处出入（计划没写到，任务 4 照这一行做）：设计第 8.4 节与 P21 只把 `dial_route` 写在句柄的 `Trail::follow` 之下，今天 `Dialer::dial` 包的是整个 `connect_session`（`engine.rs:1683` ～ `:1706`：`turn_back`、`look_up_owner`、`quic_adaptive.on_tcp` 与 `trail.follow(self.connect_session(...))`）——规则在选路时的解析也记在 `dns_ms` 上（`rurge-dns` 的 `Noted` 在 `resolver.rs:1297` ～ `:1305` 往任务局部的 `Trail` 记）。拆成两步之后，`route_session` 也必须在句柄的 `trail.follow` 里跑（`dial_route` 照计划另包一次；`looked_up` 是累加的，两次 `follow` 不冲突），否则 IP 规则在 `choose_policy` 里的那次解析写不进 `dns_ms`，`pipeline.rs` 的 `a_rejected_session_has_no_remote_address_but_the_lookups_of_the_rules`（`:3678`）在 `target.dns_ms.is_some()` 上变红——P11 说它照过。不改语义，只改这两步各包在哪一层 | 本提交 |
| 9 | 0 | 第 3 项第 1 件（`forward()` 的错误页与结局的文字，任务 2 第 6 步逐字照抄；核实与协调者读到的一致）：拨号失败一张映射两处用——`http.rs:265` ～ `:267`（`connect()`）与 `:466` ～ `:468`（`forward()`）都是 `FailKind::Dns` → `DNS lookup failed`、`Timeout` → `Connection timed out`、`Connect` 与 `Other` → `Connection failed`；页面由 `failure_response`（`:375` ～ `:394`）给，502、标题 `Connection failed`（`:387`）、正文 `format!("{what}: {message}")`，`show_error_page` 关着时不答就关（`:379` ～ `:381`）。上游握手失败：结局 `SessionOutcome::Failed(format!("upstream handshake failed: {e}"))`（`:483` ～ `:485`），页面 `format!("Upstream handshake failed: {e}")`（`:486`）。发出请求失败：`if !e.is_user() { handle.mark_upstream_failed() }`（`:568` ～ `:570`），结局 `Failed(format!("upstream request failed: {e}"))`（`:571` ～ `:573`），页面 `format!("Upstream request failed: {e}")`（`:574`）。改写失败：结局 `Failed(format!("bad request uri: {e}"))`（`:529`），答 `responses::bad_request("malformed request URI")`（`:530`，400、`text/plain; charset=utf-8`）。407 那一种今天这样答（只在向上游 HTTP 代理以绝对形式发出、`upstream_headers.is_some()` 时）：结局 `Failed("http proxy answered 407 Proxy Authentication Required")`（`:552` ～ `:554`），答 `failure_response(..., "Upstream proxy rejected the request: 407 Proxy Authentication Required")`（`:555` ～ `:559`）——502 错误页（标题仍是 `Connection failed`）或关连接，上游那条 407 应答本身不回给客户端；`outbounds.rs:212` 的用例钉着这两句文字。终止：`cancelled_outcome` 见第 12 行，答 `Err(HandlerError::Close)`（`:538` ～ `:541`）。上游连接驱动：`Completed`（`:504`）或 `Failed(format!("upstream connection error: {e}"))`（`:505`），先等 `handled` 被取消再结束句柄。拒绝：TINYGIF 200 与 43 字节的 GIF（`:434`）、DROP 停 `drop_hold` 再关（`:438` ～ `:441`）、其余看 `show_error_page_for_reject`——开着时 403 页（标题 `Request rejected`，`:450`，正文 `format!("The request was rejected by the {} policy.", kind.name())`，页面里还带着记录的 `rule`、策略链与 `dst`），关着时不答就关（`:443` ～ `:445`）。结局的文字都是小写开头，页面的文字不是 | 本提交 |
| 10 | 0 | 第 3 项第 2 件（`fail()` 的合并，P4 的 `settle` 接过它）：与协调者读到的一致——`fail`（`engine.rs:1428` ～ `:1453`）在句柄上已有说明（`handle.error()` 是 `Some`，来自 `set_error`：组循环、不支持的做法、空的组）时把它写成 `format!("{note}; {message}")`（`:1441` ～ `:1443`），`DialError::Failed.message` 仍是那句没有合并过的失败文字；`reject`（`:1476` ～ `:1481`）只 `finish(Rejected(kind))`，不合并、不动 `error`。任务 4 的 `settle` 照这两条做 | 本提交 |
| 11 | 0 | 第 3 项第 3 件（`query_for` 读会话的哪几项）：与协调者读到的一致——`owner.rs:62` ～ `:84` 的 `query_for(session, since)` 按 `(session.listener, session.transport)` 分四支，读 `in_port`、`local`、`src`：HTTP 与 SOCKS5 一律 `Transport::Tcp`、`local` 取 `session.local`（没有时 `0.0.0.0:<in_port>`）、`local` 字段填 `src`；虚拟网卡的 TCP 用 `session.local` 当对端、UDP 不带对端；`Forward` 与 `Internal` 返回 `None`。`is_local` 在 `look_up_owner` 里读 `session.src.ip()`（`engine.rs:499`），不在 `query_for` 里。没有别的读法；任务 6 用 `SessionInfo::tcp(0.0.0.0, 0)` 加 `session_for` 的那几项（`src`、`in_port`、`local`、`listener`、`transport`）建会话就够（`http.rs:161` ～ `:170`） | 本提交 |
| 12 | 0 | 第 3 项第 4、5 件：`cancelled_outcome`（`http.rs:581` ～ `:587`）与协调者读到的一致——`was_killed()` 为真记 `SessionOutcome::Failed("killed")`，否则记 `Completed`（停止与守护进程退出那一种）。`DROP_HOLD`（`engine.rs:47`）与 `HANDSHAKE_TIMEOUT`（`:49`）都是 `pub const`（30 秒），由 `listener_opts`（`:385` ～ `:395`，`fn`，不带 `pub`）给监听器；任务 6 的 `crates/rurge-engine/src/http.rs` 与引擎在同一个 crate 里，用 `crate::engine::DROP_HOLD` 就够，不必改可见性 | 本提交 |
| 13 | 0 | 第 3 项第 6 件（hyper 1.11.1，「依据」一节逐项找到，路径都相对 `hyper-1.11.1/src/`）：「依据」列的名字全在——`server/conn/http1.rs` 的 `Builder::preserve_header_case`（`:320`）、`timer`（`:411`）、`header_read_timeout`（`:352`）、`keep_alive`（`:273`）、`half_close`（`:265`）、`serve_connection`（`:452`）、`Connection::with_upgrades`（`:199`）、`UpgradeableConnection::graceful_shutdown`（`:526`）；`proto/h1/conn.rs` 的 `header_read_timeout` 在 `poll_read_head` 开头起表（`:218` ～ `:235`，每个头一次）与 `mid_message_detect_eof`（`:487` ～ `:504`，读到 EOF 时 `close_read()` 再返 `new_incomplete()`）；`client/conn/http1.rs` 的 `handshake`（`:148`）、`SendRequest::poll_ready`（`:164`）、`ready`（`:173`）、`is_ready`（`:184`）、`is_closed`（`:189`）、`send_request`（`:228`）、`try_send_request`（`:259`）、`Connection::with_upgrades`（`:302`）；`TrySendError` 是 `client/dispatch.rs:26` 的 `pub struct`，由 `client/conn/mod.rs:22` 导出成 `hyper::client::conn::TrySendError`（不在 `http1` 底下），`take_message`（`:308`）、`message`（`:317`）、`into_error`（`:322`）、`error`（`:327`）；`error.rs` 的 `is_user`（`:222`）、`is_canceled`（`:231`）、`is_closed`（`:240`）、`is_incomplete_message`（`:271`）、`is_timeout`（`:315`）；`upgrade.rs` 的 `pub fn on`（`:106`，请求与应答两侧都吃）。P23 引的三处读过：`proto/h1/conn.rs` 的 `Reading::Continue` 一支（`:405` ～ `:416`）在 `Writing::Init` 时把 `b"HTTP/1.1 100 Continue\r\n\r\n"` 直接拼进 `headers_buf`（`:409` ～ `:410`）；`poll_drain_or_close_read`（`:859` ～ `:875`）把 `Reading::Continue` 推成 `Body` 时注释写着“skip sending the 100-continue”（`:861`），排不空的就读侧关掉（`:873`）；客户端一侧 `proto/h1/role.rs` 的 `parse` 在一个 `loop` 里跳过 informational 应答（`:1031` 起，`head.subject.is_informational()` 在 `:1184`，缓冲区空了就返回 `Ok(None)` 再读）。工作区的 `hyper` 是 `"1"`、`hyper-util` 是 `"0.1"`（`Cargo.toml:29`、`:30`），锁到的是 1.11.1 与 0.1.20；`http-body` 不是工作区依赖（P2 对） | 本提交 |
| 14 | 0 | 第 4 项（数）：`Cargo.lock` 里 `[[package]]` 520 个（与计划写的相同）；`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 是 `const LISTED: [(&str, &str); 23]`（`:18` ～ `:42`），数下来也是 23 项。都与计划写的一样 | 本提交 |
| 15 | 0 | 第 5 项（P11 第 1 张表，`crates/rurge-inbound/src/http.rs` 的 `mod tests`，行号与表里的逐条相同，`mod tests` 起于 `:590`）：`connect_tunnels_to_the_dialed_stream`（`:693`）断言 CONNECT 拿到 200、隧道回显、一条已结束的记录——照过；`an_ipv4_mapped_target_is_dialed_as_ipv4`（`:722`）断言 CONNECT 与绝对形式两处都把 `[::ffff:127.0.0.1]` 变成 IPv4、CONNECT 答 200——留 CONNECT 那一半，绝对形式那一半随 `forward()` 挪进引擎（表里叫 `an_ipv4_mapped_url_host_is_dialed_as_ipv4`）；`a_session_names_the_client_and_the_listener_port`（`:752`）断言隧道与明文请求两条会话的 `src`、`in_port`、`local`——同样一分为二；`rejected_connect_closes_without_a_response`（`:787`）断言 CONNECT 这一路上 REJECT、NO-DROP 与 TINYGIF 三个端口都不答就关（TINYGIF 只对明文请求有意义）、DROP 停满 `drop_hold` 再关、拨号失败答 502——照过；`basic_auth_compares_the_password_only`（`:819`）与 `constant_time_compare_matches_plain_equality`（`:883`）断言鉴权的比较——照过；`hop_by_hop_headers_and_the_tokens_connection_names_are_dropped`（`:891`）只剩 `strip_hop_by_hop` 一件事——删，任务 2 在 `rurge-http` 的 `headers.rs` 有同样的与升级的那一条；`non_proxy_requests_get_400`（`:909`）断言源站形式、`https`、`ftp` 与没有端口的 CONNECT 四种 400——照过（scheme 的检查挪进 `handle`，文字不变）；`stalled_http_handshakes_time_out`（`:928`）断言发半个头（连一个字节也不发）的客户端被 `handshake_timeout` 放开——照过（监听器一侧的读时限不动，设计第 9 节）；`plain_requests_are_forwarded_per_request_with_rewritten_headers`（`:951`）断言逐请求转发、`Host` 换成目标、userinfo 去掉、逐跳的头两个方向都去掉、两个请求两条记录——删，引擎的 `the_listener_rewrites_a_plain_request_for_the_origin`；`rejects_and_failures_render_pages_or_close`（`:1037`）断言 REJECT 与 DROP 关、失败 502 页含 `DNS lookup failed` 与 `dns.test:80`、`show_error_page_for_reject` 开着时 403 页含 `REJECT-NO-DROP` 与 `FAKE,rule`、`show_error_page` 关着时失败不答——删，引擎的 `the_listener_answers_refusals_and_failures_as_the_profile_says`；`plain_forward_session_is_killable`（`:1105`）断言终止一条挂住的明文请求后客户端读到 EOF、记录是 `Failed("killed")`——删，引擎的 `a_plain_request_is_killable`；`a_client_leaving_mid_upload_is_no_upstream_failure`（`:1163`）断言客户端在正文里走掉不算上游失败（`upstream_failed()` 为假）——删，引擎的同名用例（`mark_upstream_failed` 只在 `!e.is_user()` 时调，第 9 行）；`an_http_authority_cannot_carry_control_characters_or_spaces`（`:1220`）断言 `http` 库不接受这几种 authority——照过；`absolute_form_keeps_the_uri_and_applies_the_upstream_headers`（`:1233`）与 `the_upstream_headers_replace_same_name_headers_including_host`（`:1267`）断言绝对形式保留 URI、去掉 userinfo、上游的头替换同名的（`Host` 也算）、坏头名丢掉——删，任务 2 在 `rurge-http` 有同样的两条；`a_sniffable_connect_is_answered_before_it_is_dialed`（`:1295`）与 `a_sniffable_connect_is_relayed_after_the_answer`（`:1315`）断言可嗅探的 CONNECT 先答 200、拨号失败之后不再答、成功照旧回显——照过。与表的预计不同的：无（表把 `plain_requests_are_forwarded_...` 里第二个请求的 TinyGif 也算进要搬的那一条，一并搬） | 本提交 |
| 16 | 0 | 第 5 项（P11 第 2 张表，任务 4 与任务 8 的那一处；`crates/rurge-engine/tests/sniff.rs`）：`the_prefix_reaches_the_upstream_whole`（`:762`）在 443 上先写 `GE`、100 毫秒后写 `T / HTTP/1.1…` 与 10 KiB，断言逐字节回显——M1b 起判为 `Http` 就进引擎，回显的服务器答不出 HTTP 应答，任务 4 照表把第二次写换成判为 `Other` 的 `GEt …`（小写 `t`），两次写与 100 毫秒不动；`the_sniffs_are_counted`（`:874`）五段各一种结局（TLS、`GET / HTTP/1.1`、`hello`、不发、`GE` 后关写端），断言 `the_records(&h, 5)` 与 `HttpCounts { sniffed: SniffedCounts { tls: 1, http: 1, other: 1, timeout: 1, closed: 1 } }`——第二段起进引擎，客户端读到的 18 字节改成以 `HTTP/1.1 502` 开头，记录仍是 5 条（那个请求走到了选路，承载连接不另记）；任务 8 在那个字面量里加 `requests: 1`；`a_plain_http_session_is_marked_http`（`:453`）两段断言 `CONNECT :80` 里一个请求的协议是 `http`、`dst` 是 `target.test:80`、上游被问一次，以及 `PROTOCOL,HTTP` 在 `DOMAIN` 之前命中——照过（请求进引擎，拨一次、一条记录；`dst` 仍是连接的目标，P22 只改 `http_host`）；`force_http_engine_hosts_sniff_another_port`（`:574`）三段：强制端口上先写 `hello`、断言 `reply[1] == 0x00` 且之后读到空（拨号失败结束隧道），再是不被嗅探的两段 `0x05`——第一段照表改写一个带 `Connection: close` 的完整请求（`GET / HTTP/1.1`、`Host: target.test:8080`），`reply[1]` 仍是 `0x00`（先答后读），读到的是 `HTTP/1.1 502`（`A` 是 reply 5 的假 SOCKS5，拨号失败，`show-error-page` 缺省真），后两段不动。与表的预计不同的：无 | 本提交 |
| 17 | 0 | 第 5 项（P11 第 2 张表，任务 5）：`crates/rurge-engine/tests/pipeline.rs` 的 `SESSION_FIELDS`（`:661` ～ `:672`，十个名字）与 `a_field_of_a_log_line_is_read_whole`（`:690`，`#[test]`，两段手写的样例行）——`field()`（`:677`）靠这张表找“下一个字段名”来切值，断言 `policy` 与 `rule` 里的空格与逗号读得全、最后一段的值读到行尾——任务 5 加 `method`、`status`、`url` 三项（不加就把值切在中间），两段样例行照新的行写（P20 把三项放在 `elapsed_ms` 之后）；`crates/rurge-api/src/routes/requests.rs` 的 `every_field_of_a_record_is_sent_under_its_own_name`（`:232`）建一条每个字段都不同的记录，断言 `RequestJson` 的 JSON 与字面量逐个相同、没有多余的键——任务 5 在 `RequestJson`（`:45`）的 `protocol` 之后加 `method`、`url`、`statusCode`，字面量与 `record` 辅助（`:202`）一起加。与表的预计不同的：无 | 本提交 |
| 18 | 0 | 第 5 项（P11 第 2 张表，任务 6 的全部；逐条找过读过，要点写在这里）：`tests/outbounds.rs` 的 `a_plain_request_goes_to_an_http_upstream_in_absolute_form`（`:73`）断言上游收到一条以绝对形式写出的请求行（userinfo 去掉）、`Host`、`Proxy-Authorization` 与配置的头，没有 `CONNECT`、没有 `Proxy-Connection`；`always_use_connect_and_other_protocols_tunnel_plain_requests`（`:114`）断言 `always-use-connect=true` 与 SOCKS5 两种策略下明文请求被隧道化（`CONNECT` 的请求行与 SOCKS5 的 ATYP 钉着）；`a_forward_mode_407_is_a_failed_session_and_a_502_page`（`:212`）断言上游 407 时客户端拿到 502、记录 `Failed` 且 `error` 恰是 `http proxy answered 407 Proxy Authentication Required`、上游只收到一条请求（不重发）；`a_pinned_host_turns_forwarding_into_a_tunnel`（`:345`）断言 `[Host]` 钉住地址时改走隧道（`CONNECT 10.1.2.3:80`）、请求里的 `Host` 仍是名字；`rurge_talks_to_rurge`（`:467`）断言两个引擎互为上游时 B 记三条会话（两条 `CONNECT`、一条明文请求，`ListenerKind` 是 2 个 Http 与 1 个 Socks5）且只有 B 问了 DNS；`tests/smart.rs` 的 `three_seconds_without_an_answer_count_against_the_member`（`:380`）断言 `CONNECT :80` 里一个请求、源站不答时 3 秒后记一次成员失败而记录仍在活动（`book.used` 与 `smart::watch` 要留在 `dial_route` 里）；`the_first_byte_back_is_reported`（`:315`）断言应答到达时记一次成员成功（首字节由池的计数层标，P7）；`a_plain_request_whose_upstream_hangs_up_counts_against_the_member`（`:427`）断言上游挂着不回算成员失败（第 9 行那句 `mark_upstream_failed` 要留）；`tests/outbounds.rs` 的 `a_local_host_item_reaches_the_proxy_as_an_ip`（`:263`，两轮 `true`/`false` 各两次会话）、`a_local_host_item_with_several_addresses_gives_the_proxy_one_at_random`（`:298`，40 条隧道 40 次）、`a_selection_applies_to_the_next_connection_and_survives_a_restart`（`:522`）、`a_subnet_group_follows_the_network_the_engine_is_on`（`:579`）、`tests/network.rs` 的 `a_network_change_moves_the_subnet_group`（`:165`，`visit` 在 `:80`）、`tests/outbounds_ssh.rs` 的 `a_keystore_key_logs_in_to_a_pinned_server`（`:55`）与 `an_ssh_session_runs_inside_shadow_tls`（`:89`）都是 `CONNECT :80` 之后一个请求、数上游（或数登录）——池属于一条客户端连接，一次请求拨一次，照过；`tests/outbounds_shadow_tls.rs` 的 `an_http_upstream_behind_shadow_tls_v2_needs_no_sni`（`:62`）与 `tests/outbounds_tls_family.rs` 的 `a_plain_request_is_tunnelled_through_trojan_over_websocket`（`:290`）断言明文请求经 8080 上的隧道，照过；`tests/pipeline.rs` 的十八条（`:218`、`:249`、`:414`、`:472`、`:495`、`:530`、`:563`、`:595`、`:717`、`:765`、`:787`、`:1070`、`:1091`、`:1325`、`:3158`、`:3230`、`:3448`、`:3678`）共同的样子是 `get_via_proxy`（`:179`：一条连接、一个绝对形式的 `GET`、按 `Content-Length` 读完整）或 `http_exchange`（`:143`）之后断言状态行、正文、记录、日志行与规则命中数；要点：`:249` 要 `connect_ms` 与 `first_byte_ms` 都有、`:787` 只要求日志里的 `error` 与记录里的相同（文字变了也过）、`:3448` 要暂停时 502 页含 `REFUSED_STRICT` 且记录 `Failed` 带同一个文字、`:3678` 要 REJECT 的记录 `remote_addr` 为 None 而 IP 规则那条的 `dns_ms` 有值（第 8 行）、`:717`/`:765`/`:787` 三条日志用例在任务 5 改了 `SESSION_FIELDS` 之后仍要过——照过；`crates/rurge/tests/cli.rs` 的十六个测试（`:2925`、`:2978`、`:3081`、`:3411`、`:3493`、`:3685`、`:4103`、`:5090`、`:6458`、`:6505`、`:6555`、`:6625`、`:6666`、`:7094`、`:8132`、`:8194`，正好是表里那 16 行）都经 `http_get`（`:2738` 与 `:2743`：一次写出发一条绝对形式的 `GET` 加 `Connection: close`、读到 EOF），断言状态行、正文、记录条数（`:3493` 三条被拒的请求留三条记录）与 `PROCESS-NAME` 规则（`:8132`、`:8194`），六条严格刷新的用例断言 502 与 `REFUSED_STRICT`；`crates/rurge/tests/run/enhanced_mode.rs` 的 `the_daemon_s_own_dns_and_downloads_keep_working_while_the_enhanced_mode_is_on`（`:1104`）断言守护进程自己的 DNS 答了会话、目标被问一次——照过；`crates/rurge-api/tests/api.rs` 的十条（`:748`、`:800`、`:899`、`:934`、`:992`、`:1207`、`:1330`、`:1490`、`:1530`、`:2354`）经 `get_via_proxy`（`:377`：一条连接、`Connection: close`、读到 EOF），断言记录的键、`notes`、`listenPort`、`remoteAddr`、`dnsMs`、`/v1/traffic` 的分组与策略选择；`:800` 还断言明文请求的记录在交换期间是 `active` 且 `POST /v1/requests/kill` 能结束它（引擎里请求的守卫照 P18）；`tests/interop/tests/sing_box.rs` 的 `a_plain_request_in_absolute_form_is_served_by_sing_box`（`:45`）只用 `rurge-proto` 的 `HttpForward`（`:64`），不经监听器，照过。与表的预计不同的：无 | 本提交 |
| 19 | 0 | 第 5 项的补扫（另扫了 `crates/*/tests` 全部、`crates/*/src` 里的内联测试与 `tests/` 下的成员，找的是两张表上没有的两种用例）：(1) 目标端口是 80（或被 `force-http-engine-hosts` 强制的端口）、之后写了 `HTTP/1.` 请求字节的——`connect_via_http(…:80)` 十四处全在表里（`network.rs:81`、`outbounds.rs:280`、`:289`、`:319`、`:542`、`:552`、`:602`、`:612`、`outbounds_ssh.rs:81`、`:121`、`smart.rs:327`、`:403`、`sniff.rs:467`、`:487`），写 443 的十八处全在 `sniff.rs`（`:169`、`:385`、`:418`、`:521`、`:542`、`:567`、`:631`、`:658`、`:686`、`:739`、`:779`、`:820`、`:861`、`:893`、`:900`、`:910`、`:920`、`:928`），其中 TLS 字节的（`:169`、`:385`、`:418`、`:861`）与不嗅探、只写非 HTTP 字节的（`:521`、`:542`、`:567`、`:631`、`:658`、`:686`）与不发字节的（`:739`）与 `GE` 后关写端的（`:820`）都不受影响，剩下的 `:779`、`:893`、`:900`、`:910`、`:920`、`:928` 在表里的两条用例中；强制端口上的只有 `force_http_engine_hosts_sniff_another_port`（`:574`，表里）与只调 `sniffable` 的 `which_sessions_are_sniffable`（`:26`，没有流量）；写非 HTTP 字节的还有 `tun.rs:110` 的 `hello far away`，判为 `Other`，不被强制就照原样转发。没有表外的。(2) 向 HTTP 监听器发绝对形式请求的——表里三组（`pipeline.rs` 18 条、`cli.rs` 16 条、`api.rs` 10 条）之外没有：`pipeline.rs` 里用 `http_exchange` 而不用 `get_via_proxy` 的几条都在 `CONNECT` 上（`:305`、`:354`、`:1383`、`:3401`、`:3499`、`:3546`），`process.rs:601`（`CONNECT ads.test:8443`）、`tests/external/tests/engine.rs:110`（`CONNECT` 到回环的临时端口）、`tun_dns.rs:126` 的 `connect_answer` 与它那两条 502 的用例（`:631`、`:712`，都是临时端口，不嗅探）、`api.rs:1405`（只把 `target_url()` 当策略测试的 URL，不发代理请求）都不发绝对形式；`outbounds.rs:500` 在 `rurge_talks_to_rurge` 里。派工说“看过”的三处也读了：`tun.rs:110` 是 `tcp_to_a_documentation_address_follows_the_rules`，应用写的是 `hello far away`（`Other`，不强制就不进引擎）；`tun_dns.rs` 里两条 `:80`——`:571` 在 SYN 就被预匹配拒（到不了嗅探），`:674` 是背后没有名字的 fake 地址（M1a 的 P5 已让它不嗅探，照今天的路拿 `0x05`），其余 fake 地址的用例都用临时端口；`network.rs` 的 `visit`（`:80` ～ `:87`）是 `CONNECT :80` 加一个 `GET`，已在表里。找到的表外的：无 | 本提交 |
| 20 | 0 | 第 6 项（门禁，一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 退出码 0；`cargo clippy --all-targets -- -D warnings` 0 个警告（退出码 0）；`cargo test -j 8 --workspace --no-fail-fast` 89 个结果行、4792 通过、1 失败、13 忽略，失败的是 `crates/rurge-engine/tests/pipeline.rs` 的 `a_doq_carrier_the_proxy_will_not_open_fails_naming_the_policy`（`:3006` 的 `assert_eq!(doq.connections(), 0)` 拿到 1）——单独重跑它六次全过（`gate-task-0-rerun.log`，每次 5.02 ～ 5.03 秒，正好吃满 `wait_for_record` 的 5 秒界，全量时这台机器上负载更高），与本分支的改动无关（本分支只有文档提交），两次合起来按 89 / 4793 / 0 / 13 记，与基线（89 / 4793 / 0 / 13）相同；没有二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出，`cli` 的 `give_up` 也没有再出现。这一条偶发照实记下来，后面每个任务的全量若再见到它，单独重跑这一个用例，不算本任务的失败。日志 `gate-task-0.log` 在工作区（git 忽略）。需要参照服务端的互操作用例在本机立刻返回、算在通过的数里 | 本提交 |
| 21 | 0 | 修正第 1 轮（评审）：三件事。一、覆盖第 2 行末的半句“任务 1 的 `plain_requests` 是第四个带缺省实现的方法”：`plain_requests` 是**任务 6** 加的（本计划「文件结构」任务 6 那一行与任务 6 的需求），今天 `Dialer` 带缺省实现的已经有四个（`sniffable`、`serve`、`admit_udp`、`associate`），加上它是第五个；设计第 3 节写的“三个带缺省实现的方法”指的是 M1 新加的 `sniffable`、`serve`、`plain_requests` 这三个，不是说它今天是第几个。二、覆盖第 8 行的前提：设计第 8.4 节（设计 `:282`）只把 `dial_route` 写在句柄的 `Trail` 之下、没有说 `route`，P21 没有提 `Trail`，而**本计划任务 4 第 3 项已经这样写**——`route` 在 `handle.trail().follow(…)` 里跑 `route_session`（`:463`）、`dial` 在 `handle.trail().follow(…)` 里跑 `dial_route`（`:464`）；派工只给了任务 0 的需求，写第 8 行时没有读到那一项。第 8 行给的理由照旧成立、也有用：规则在选路时的那次解析记在 `dns_ms` 上（`rurge-dns` 往任务局部的 `Trail` 记），`pipeline.rs:3678` 断言它，所以任务 4 不能把 `route_session` 挪出 `follow`；`Dialer::dial` 那一路照今天整个包在一次 `follow` 里。三、第 20 行记下的 DoQ 偶发另立在「延后事项」第 1 行。本轮的门禁（协调者的决定：只改计划末尾的两张表，不跑全量）：`git diff --stat` 只有本计划文件的两行；拿本机账户名作字面量 `git grep -l` 一遍（名字不写在这一句里——写进来本文件就进那个名单了），列出的文件仍是 26 个 | 本提交 |
| 22 | 1 | 第 1 ～ 4 项的做法（行号是本提交的）：`rurge-net` 的 `Trail::set_peer`（`trail.rs:60`）把 IPv4 映射的换算提成私有 `canonical`（`:85`），`connected`（`:77`）与它共用，`clear_peer` 照旧清得掉；`rurge-inbound` 的两个侧槽是 `OnceLock`（`session.rs:51` 的方法、`:52` 的状态码），`set_method`（`:219`）、`method`（`:225`）、`set_status`（`:233`）、`status`（`:239`）；`responses.rs:9` 加 `BoxError`、`:15` 的 `ResponseBody` 放宽成 `BoxBody<Bytes, BoxError>`，`full`/`empty` 的 `map_err(|never| match never {})` 照旧没改（`!` 能推出 `BoxError`，编译通过），`forward()` 交回应答的那一处（`http.rs:562`）改成 `map_err(BoxError::from).boxed()`，`lib.rs:22` 导出 `BoxError` 与 `ResponseBody`；`rurge-http` 的依赖照「全局约束」一次加齐（`Cargo.toml:11` ～ `:22` 的十一条新依赖，`tokio` 在 `:15` 照旧，`dev-dependencies` 的 `rurge-net` 带 `testing` 在 `:25`），`Cargo.lock` 仍是 520 个包、只有 `rurge-http` 那一项的依赖列表变长；新文件 `pool.rs`（`Pool` 在 `:37`、`Reusable` 在 `:29`、`MAX_IDLE` 在 `:21`、`IDLE_FOR` 在 `:25`、`Meter` 在 `:103`、`Metered` 在 `:126`），`lib.rs:12` 声明、`:17` 导出，文件头的 `//!` 改写成 M1b 之后的样子（`Upstreams` 与 HTTP 服务留给任务 2、3）；`testing.rs` 的 `State.connections`（`:64`）、接受循环在交给任务之前加一（`:105`，TLS 也数）、`connections()`（`:195`）。与 brief 不同的两处，都是 clippy 的 lint（执行者的裁定，语义不变）：`Pool` 与 `Meter` 各加了 `Default`（否则 `new_without_default` 变红），`Pool` 另加了 `is_empty()`（否则 `len_without_is_empty` 变红） | 本提交 |
| 23 | 1 | 门禁（一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 退出码 0；`cargo clippy --all-targets -- -D warnings` 0 个警告（退出码 0）；`cargo test -j 8 --workspace --no-fail-fast` 89 个结果行、4801 通过、1 失败、13 忽略，失败的是任务 0 就记下的那一条偶发——`crates/rurge-engine/tests/pipeline.rs` 的 `a_doq_carrier_the_proxy_will_not_open_fails_naming_the_policy`（延后事项第 1 行；与本分支的改动无关，本任务没改引擎与那一条的路径），单独重跑六次全过（`gate-task-1-rerun.log`，5.02 ～ 5.03 秒，与任务 0 的六次相同，又吃满 `wait_for_record` 的 5 秒界），两次合起来按 89 / 4802 / 0 / 13 记，比基线（89 / 4793 / 0 / 13）多出的 9 条正是本任务的 9 条新用例（`trail.rs` 1、`session.rs` 1、`pool.rs` 5、`Metered` 1、`TestServer` 1），结果行数不变；没有二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出，`cli` 的 `give_up` 也没有出现；池的 6 条与 `it_counts_every_accepted_connection` 各自单独循环跑 20 次、全过。需要参照服务端的互操作用例在本机立刻返回、算在通过的数里。日志 `gate-task-1.log`、`gate-task-1-rerun.log`、`loop-task-1-pool.log`、`loop-task-1-connections.log` 在工作区（git 忽略） | 本提交 |
| 24 | 1 | 修正第 1 轮（评审；派工把这一行叫第 23 行——第 23 行已经是任务 1 的门禁行，编号照规矩接着往下走）。一、判据（一台机器上的一次运行，不是 CI）：门禁三步的结果，全量 89 / 4801 / 1 / 13，失败的是 `crates/rurge-engine/tests/pipeline.rs` 的 `a_doq_carrier_the_proxy_will_not_open_fails_naming_the_policy`（同样是 `:3006` 的 `doq.connections()` 拿到 1），这是第二次（第一次在任务 0，修正记录第 20 行），单独重跑六次全过，合起来 89 / 4802 / 0 / 13，没有 `STATUS_` 退出的二进制；不查（延后事项第 1 行）。二、`a_metered_stream_counts_onto_the_current_request_only` 的末段原来是空测：读到 EOF 之前 `meter.set(None)` 已经清空，没有请求借着这条连接时 `Metered` 本来就什么也不记，`b.first_byte_seen()` 为假与 EOF 怎样处理无关（把 `poll_read` 里的 `n > 0` 去掉，那一段照样绿）。改在 `client.shutdown()` 之前 `meter.set(Some(b.clone()))`，EOF 在 `b` 借着这条连接时读到，断言 `b.bytes()` 仍是 `(1, 0)`、`!b.first_byte_seen()`；生产代码不动。临时去掉 `n > 0` 只跑这一条：`crates\rurge-http\src\pool.rs:358:9: the end is no first byte` 变红，用编辑工具改回去、`git diff` 确认 `pool.rs` 的生产代码与 `469e586` 相同。修正之后：`cargo fmt --all --check` 退出码 0、`cargo clippy --all-targets -- -D warnings` 0 个警告、`cargo test -j 8 --workspace --lib -- pool::tests` 循环二十次全过（`loop-task-1-fix-1.log`） | 本提交 |
| 25 | 2 | 做了什么（行号是本提交的）：`rurge-inbound` 加 `Close`（`session.rs:31`，`Display` 文本 `connection closed by policy`，`lib.rs` 导出）。`rurge-http/src/headers.rs`（新）：`Keep`（`:23`，`NONE` 常量）、`strip`（`:49`）、`host_port`（`:75`），从 `rurge-inbound/src/http.rs` 抄来（那几份在任务 6 随 `forward()` 删）；`strip` 先去掉 `Connection` 列出的名字（`keep` 留的除外）再去掉名单上的，`keep.upgrade` 留下 `Upgrade` 并放回恰好一个 `Connection: upgrade`。`rurge-http/src/engine.rs`（新）：`Entry`（`:45`）、`Conn`（`:55`）、`Named`/`Routed`/`Pages`/`Refusal`/`Upstream`（`:78` ～ `:121`）、`Upstreams`（`:126`）、`Requests`（`:206`）与 `handle`（`:240`）的七步、`prepare`（`:439`）、`answer`（`:488`）、`failed`（`:538`）、`timed_out`（`:557`）、`page`（`:575`）、`form_of`（`:625`）、`rename`（`:659`）、`Outing`（`:685`）、`origin_form`/`absolute_form`（`:695`、`:713`）、`Stalled`（`:744`）、`Idle`（`:759`，请求正文）、`Tail`（`:822`，应答正文，`complete` 在 `:866`、`fail` 在 `:899`、`stop` 在 `:909`）、`Guard`（`:979`）、`Served`（`:1022`）、`serve_connection`（`:1033`）；`lib.rs` 声明两个模块并导出。守卫在 `open` 之后、第一个 `.await` 之前建（`:251` ～ `:254`）；流程自己答复的（400、拒绝、失败、504）先 `set_status`、再 `finish`，守卫随后丢掉时看到已结束、什么也不做；交给 hyper 的应答把守卫带进 `Tail`，读完由 `Tail::complete`、出错由 `Tail::fail`、令牌取消由 `Tail::stop`（走守卫的规则）结束句柄，`Tail` 被丢掉（客户端走了）由守卫结束。上游连接的任务照 `forward()` 两层 spawn（`:341` ～ `:356`），不结束任何句柄，出错只在 DEBUG 记一行、不带地址。执行者的裁定：一、`Upstreams::Key` 的界加了 `Sync`（brief 写的是 `PartialEq + Send`）：键被 `Tail` 持有而 `Tail` 要 `.boxed()`（`BoxBody` 要求 `Sync`），任务 4 的 `PoolKey` 各字段都 `Sync`。二、`Outing` 装进 `Box`：`Tail` 的 `Body::poll_frame` 用 `get_mut`，要 `Unpin`，而 `U::Key` 不保证 `Unpin`。三、504 页的信息文字 brief 没写，取 `The upstream server did not answer in time.`（标题 `Gateway timeout` 照设计）。四、源站形式的 `url` 用 `Host` 头原样重建（`Host: Target.Test:8080` 得 `http://Target.Test:8080/…`），`http_host` 按 P14、P22 小写；绝对形式取请求行。五、`Listener` 的请求没有 authority、或 scheme 不是 `http://` 时 `handle` 先答 400（两句文字照今天的 `forward()`），不建句柄、不记记录，同今天——这也是 P11 表里 `non_proxy_requests_get_400` 说的「scheme 的检查挪进 `handle`」的位置。用例在文件末尾的测试模块（`:1063` 起）：夹具 `Fake`（`:1105`，`Key = Route = u32`、`Lease = ()`，按 URL 路径挑脚本、`dial` 连回环源站并数次数、`open` 用 `new_with_token` 把句柄记下来）、`Client`（读有界的 duplex 客户端）、`connected`/`rig_timed`（`:1383` 起）、`recording_origin`（`:1865`）、`holding_origin`（`:1926`）；17 条用例（`:1432` ～ `:2195`） | 本提交 |
| 26 | 2 | 两条提示要求的用例与它们的红证据。`a_response_without_a_body_completes_its_request`（`:2150`）：源站对 `HEAD` 答 200 带 `Content-Length: 21`（不送正文）、对一个 `GET` 答 204，两个句柄都是 `Completed`、`status` 是 200 与 204，第三个请求借到同一条上游连接（`dial` 一次、源站接受一次）。临时去掉 `Tail::new` 里「建时内层已经是末尾就 `complete()`」那一段只跑这一条，红在 `Some(Failed("client closed the connection"))` 对 `Some(Completed)`——正是派工预言的那种结局；用编辑工具改回去。`a_request_body_keeps_its_framing`（`:2195`）：源站收到的 `GET` 没有 `Transfer-Encoding` 也没有 `Content-Length`，`Content-Length: 5` 的 `POST` 到了源站仍是 `Content-Length: 5` 与那 5 个字节。把 `Idle` 的 `is_end_stream` 改成 `false`、`size_hint` 改成 `SizeHint::default()` 只跑这一条：红在 `a POST with no body was framed: … transfer-encoding: chunked`。读 hyper 1.11.1 发现的、与派工说法不同的一处：未知长度时 GET、HEAD、CONNECT 被当作零长度（`proto/h1/role.rs:1397` ～ `:1409`），其余方法才写 `Transfer-Encoding: chunked`，所以「一个没有正文的 `GET` 会带着 chunked 发给上游」在这个版本上不成立、GET 那一段钉不住包装；用例因此加了一段没有正文的 `POST`（`:2220` 起），红的证据取自它。另一处照实记：`a_stalled_body_fails_the_request` 的判据里「下一个请求另拨」不能在同一条客户端连接上验——正文出错时 hyper 把客户端连接关掉，没有下一个请求可发；改为断言源站那条连接被关（`Holding::closed`，`:2080` 起） | 本提交 |
| 27 | 2 | 门禁（一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 退出码 0；`cargo clippy --all-targets -- -D warnings` 0 个警告；`cargo test -j 8 --workspace --no-fail-fast` 89 个结果行、4823 通过、0 失败、13 忽略，比基线（89 / 4802 / 0 / 13）多出的 21 条正是本任务的用例（`headers.rs` 4、`engine.rs` 17），结果行数不变；没有二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出；任务 0 起的 DoQ 偶发（延后事项第 1 行）这一次没有出现，没有重跑。`Cargo.lock` 仍是 520 个包；`crates/rurge/tests/sockets.rs` 的扫描 6 条通过（新代码不建套接字，`Fake` 的 `TcpStream::connect` 只在测试模块里）。带等待的引擎用例单独循环：`cargo test -j 8 --workspace --lib -- engine::tests::`（会连带 `rurge-rules` 里同名的 `engine::tests` 9 条，无妨）跑了 36 次，其中 35 次 17 条全过，第 7 次没有输出任何 `test result` 行（第一次循环里发生，没有留下完整输出；之后 29 次都全过，没有可复现的失败）。日志 `gate-task-2.log`、`loop-task-2-engine.log` 在工作区（git 忽略）。需要参照服务端的互操作用例在本机立刻返回、算在通过的数里 | 本提交 |
| 28 | 2 | 修正第 1 轮（评审；第 25 ～ 27 行里提到的行号是 `b2f24fe` 的，本行的行号是本提交的）。一、一条断言被注释吞掉：`engine.rs:1714` ～ `:1719` 里那两行注释后面原本是字面的 `\n` 而不是换行，整条 `assert_eq!` 成了注释的一部分、从来没有执行；拆成真的三行（两行注释与 `assert_eq!`），再用工作区的 `find_literal_newlines.py` 扫一遍五个改过的文件，无输出。二、等应答头的时限从请求正文的进展起算：`Progress`（`:797`，`started`/`last`/`waiting_on_source`，`moved`/`waiting`/`remaining` 在 `:813` ～ `:836`）由 `Idle`（`:862`，字段在 `:867`）在交出一帧或读到末尾时 `moved`、内层 `Pending` 时 `waiting`；第 5 步的 `select!` 换成令牌 → 应答 → `head_deadline(progress, idle)`（`:842`，循环睡、醒来重算；`waiting_on_source` 为真时只睡一个 `idle` 再看）的次序（`:403`）。三、长度未知的请求正文保住框架：`strip` 之后、请求正文 `!is_end_stream()` 且 `size_hint().exact()` 为 `None` 时插入 `Transfer-Encoding: chunked`（`:386`）。四、没有 `Host` 时 `url` 的写法：新的 `origin_authority`（`:680`）——端口 80 不写、IPv6 地址加方括号；`form_of`（`:643`）那一支改用它。五、客户端不读应答：`Conn` 加 `abort`（`:73`，文档照派工的话，不是 `token` 的孩子），`Tail` 带自己的 `Progress`（`:933`）与一个看门狗（`watch_the_client`，`:992`；`Tail` 的 `complete`/`fail`/`stop` 与 `Drop`（`:1097`）取消它的退出令牌），看门狗等三样（`biased`）：退出令牌、请求的令牌（取消时 `abort.cancel()`）、`waiting_on_source` 为假且距 `last` 满 `idle`（句柄 `Failed("client stopped reading")`、再 `abort.cancel()`）；`serve_connection`（`:1223`）两支都与 `abort.cancelled()` 竞速（`:1246`、`:1252`），`abort` 先到时 `Served.error` 是 `None`。夹具：`recording_origin` 会读分块正文（`read_chunked`，`:2084`），新的 `streaming_origin`（`:2190`，四个 MiB、写不动就停在那里、数对端关闭）与 `wait_until`。新的用例：`a_slow_upload_is_not_a_late_response`（`:2638`，正文分块、每块隔 100 毫秒，`idle` 400 毫秒）、`a_request_without_host_is_named_by_its_target`（`:2670`，四种目标）、`a_client_that_stops_reading_fails_after_idle`（`:2689`，`idle` 300 毫秒）、`a_killed_response_whose_client_does_not_read_is_closed`（`:2722`），以及 `stopping_a_connection_ends_its_requests`（`:2420`）的第三段（不读的客户端 + 取消连接令牌）。红证据（都是临时改一处、只跑那一条、用编辑工具改回去）：一、把期望的 URL 改错一个字符 → `left: Some("http://Target.Test:8080/a/b?q=1")` 对 `right: …?q=2`；二、把 `Idle` 的两处改回不转调（`is_end_stream` 恒假）→ `a POST with no body was framed: … transfer-encoding: chunked`；三、在正确的写法上（时限从发出起算）新用例在约 400 毫秒处拿到 `HTTP/1.1 504 Gateway Timeout`；四、去掉插入 `chunked` 的那一句 → `the body lost its framing: GET /chunked HTTP/1.1` 与 `Host: target.test` 两行、正文没到；五、不叫看门狗 → `the handle finished within 5 s: Elapsed(())`；不竞速 `abort` → 同样是 5 秒后 `the handle finished within 5 s`；`graceful_shutdown()` 之后不竞速 → `the connection ended within 5 s`。留给后面的三件：一、`Conn` 多了 `abort`，任务 4 的 `serve_http` 与任务 6 的监听器要照着给、照着竞速（协调者改派工）；二、设计第 8.10、8.11 节要在任务 8 收尾时写进第 21 节的订正：等应答头的时限从请求正文的进展起算，客户端不读时由看门狗以 `client stopped reading` 结束请求、关连接，停止也不再等不读的客户端；三、长度未知的请求正文补 `Transfer-Encoding: chunked` | 本提交 |
| 29 | 3 | 派工提示第 1 条改了 brief 第 3 项对 `Sent` 的写法：hyper 1.11.1 里 `written: false` 的 `Failed { error, written }` 没有来源——`try_send_request` 只在请求还没写出时经 `TrySendError::take_message` 把它还回来（`hyper-1.11.1/src/client/dispatch.rs:308`），写出了之后只有不带消息的错误。`Sent` 写成三支：`Answered(Response<Incoming>)`、`NotWritten(Request<ReqBody>, hyper::Error)`、`Failed(hyper::Error)`（`engine.rs:972`），私有的 `send`（`:985`）用 `try_send_request` 分这三支。订正第 28 行的那半句：那一行第二件里写的「在正确的写法上（时限从发出起算）」应是「在原来的写法上（时限从发出起算）」，报告里写对了 | 本提交 |
| 30 | 3 | 派工提示第 2 条改了 brief 第 1 项「第 5 步的逐跳的头改为 `Keep { upgrade: true, .. }`」：照字面会给每个请求都插一个 `Connection: upgrade`（`headers.rs` 的 `strip` 末尾），改成只有要升级的请求用 `Keep { upgrade: true, proxy_authorization: sniffed }`（`engine.rs:339` ～ `:350`），其余请求仍是 `upgrade: false`；「要升级」按 Go 的 `upgradeType` 读：HTTP/1.1、有 `Upgrade` 头、`Connection` 列着 `upgrade` 记号（不分大小写，`wants_upgrade` 在 `:1012`） | 本提交 |
| 31 | 3 | 派工提示第 3 条改了 brief 第 2 项的末半句「`Listener` 上的 `CONNECT` 到了就答 400、句柄 `Failed`」：`handle` 的第 1 步今天就在开句柄之前答 400（authority 形式的请求行没有 scheme），没有句柄可结束；保留这个答法，只在那一支加一行注释说明监听器自己处理 `CONNECT`（`engine.rs:252` ～ `:255`），不加用例 | 本提交 |
| 32 | 3 | 做了什么（行号是本提交的）：`Progress` 的时钟改成固定的 `epoch` 加原子量，加 `restart`（`engine.rs:1255`，重发之前把这一次的等待重新起表），`Idle` 的时限改为第一次 `poll_frame` 才起（`:1309`、`:1350`），新拨号的时间不再算成上传停住或应答迟到。开连接提成 `connect`（`:686`），`redial`（`:738`）把拨号与握手的失败答成拒绝页或 502，`retry`（`:769`）在同一个 route 上重拨：`mark_connected`、`trail().set_peer` 换成新连接的、`Meter` 指向这个句柄、`progress.restart()`、再发一次，与请求的令牌和 `head_deadline(progress, idle)` 竞速；`head_of`（`:1026`）与 `with_empty_body`（`:1038`）与 `is_idempotent`（`:997`）。第 4 步借来的那一支把没用掉的 `route` 留成 `Some`（`:317` 起）；第 5 步只在借来的连接、幂等、没有正文（`is_end_stream()` 或 `size_hint().exact() == Some(0)`）时留头（`:379`）；第 6 步两支：`NotWritten` 任何方法原样重发一次（`:400` 起），`Failed` 且 `is_incomplete_message()` 时用留的头配空正文重发一次（`:427` 起），其余与第二次失败都经 `upstream_failed`（`:812`，`!e.is_user()` 时 `mark_upstream_failed`）答 502。升级：`wants_upgrade`（`:1012`），客户端一侧的 `OnUpgrade` 在拆开请求之前取（`:383`，只要升级的请求与嗅探连接上的 `CONNECT`）；第 7 步先判没要升级而答 101 → 502（`:478` 起，记录 `upstream answered 101 Switching Protocols unasked`、页面 `Upstream answered 101 Switching Protocols to a request that did not ask to upgrade`），再判（要升级且 101）或（隧道里的 `CONNECT` 且 2xx）→ `promote`（`:833`）：设状态、101 用 `Keep { upgrade: true, proxy_authorization: false }`、`CONNECT` 的 2xx 用 `Keep::NONE` 并去掉 `Content-Length`、正文 `responses::empty()`、不建 `Tail`、`Outing` 不进池；转发的任务在 `Conn::tracker` 上（`:865`）——令牌与两侧的 `OnUpgrade` 竞速（`biased`，令牌先到 `abort.cancel()` 再返回），任一侧失败 `Failed(format!("upgrade failed: {e}"))`（`:883`），先清 `Meter`（`:891`），把 `Upgraded` `downcast` 回 `TokioIo<Metered<BoxedStream>>` 取 `read_buf` 先写给客户端（与令牌、`Conn::idle` 竞速），再 `relay`（`:937`），守卫随那个任务走（`:866` 的 `let _guard = guard`）。`prepare` 里嗅探到的 `CONNECT` 没有 `url`、`http_host` 取请求行的主机（`:546` 起）。`Fake::relay` 记字节（`:1923` 的 `copy_bidirectional` 返回值）。执行者的裁定：一、借来的连接改投新连接之前把旧 `Meter` 清空（`:406`、`:433`），免得旧连接关闭前读到的字节算到这个请求上；二、`DialError` 分 `Refused`、`Stopped`、`Handshake` 三支，`redial` 把前两支同一处置；三、升到 `promote` 的守卫必须被那个任务接住（第一版没有，句柄在 `promote` 返回时以 `client closed the connection` 结束，两个升级用例当场变红） | 本提交 |
| 33 | 3 | 用例与红证据（都是一台机器上的临时一改、只跑那一条、再用编辑工具改回去）：一、把 `downcast` 的目标换成 `TokioIo<BoxedStream>`（必然失败、`Upgraded` 照原样进 `relay`）→ `a_websocket_upgrade_is_relayed_and_counted_once` 红在 `(84, 111)` 对 `(84, 96)`，`down` 多出的 15 正是与 101 同一次写出的问候；二、把 `(tunnel && resp.status().is_success())` 改成恒假 → `a_connect_on_a_sniffed_connection_goes_to_its_target` 红在 `the connection closed before a head`（`Content-Length` 留在 `CONNECT` 的 2xx 上，hyper 的服务端作废了整个应答）；三、把留头的条件加一个恒假 → `an_idempotent_request_cut_off_is_sent_again_once` 的第二个应答是 `HTTP/1.1 502 Bad Gateway`。新用例 8 条（`engine.rs:3362` ～ `:3565`）：`a_websocket_upgrade_is_relayed_and_counted_once`（句柄 `status` 101、`Completed`，`bytes()` 精确等于源站读到的与写出的）、`a_connect_on_a_sniffed_connection_goes_to_its_target`（`dst` 仍是连接的、`url` 为 `None`、`http_host` 是请求行的、`status` 200）、`a_connect_refused_by_the_target_is_answered_as_it_is`（403 与正文照原样）、`a_101_the_request_did_not_ask_for_is_a_502`、`a_request_not_yet_written_is_sent_again_on_a_new_connection`（直接测 `send`：不 poll 的 `Connection` 丢掉 → `NotWritten`，请求原样还回来）、`an_idempotent_request_cut_off_is_sent_again_once`（`dial` 2 次、第二个 200）、`a_post_cut_off_after_it_was_written_is_a_502`（`dial` 1 次）、`a_request_on_a_new_connection_is_not_sent_again`（`dial` 1 次）。`Idle` 的时限与 `Progress::restart` 这一改没有能红的用例（要造出竞速），照派工的话说一句 | 本提交 |
| 34 | 3 | 门禁（一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 退出码 0；`cargo clippy --all-targets -- -D warnings` 0 个警告；`cargo test -j 8 --workspace --no-fail-fast` 88 个结果行、4654 通过、0 失败、13 忽略，`-p rurge-policy --lib` 的二进制以 `STATUS_HEAP_CORRUPTION` 退出（本机管控软件注入的钩子 DLL，CLAUDE.md）——单独跑那个 `.exe` 181 通过，两次合起来 89 / 4835 / 0 / 13，与基线（89 / 4827 / 0 / 13）多出的 8 条正是本任务的用例；任务 0 起的 DoQ 偶发（延后事项第 1 行）这一次没有出现。带等待与新用例一起循环：`cargo test -j 8 --workspace --lib -- engine::tests::` 21 次全过（`loop-task-3-engine.log`，会连带 `rurge-rules` 里同名的 9 条，无妨）。日志 `gate-task-3.log`、`gate-task-3-rerun.log` 在工作区（git 忽略）。需要参照服务端的互操作用例在本机立刻返回、算在通过的数里 | 本提交 |
| 35 | 3 | 修正第 1 轮（评审；本轮不改生产逻辑）。一、两条升级用例补上了源站一侧的断言：`upgrading_origin` 现在把它读到的每个请求头（到空行为止）记进 `Arc<Mutex<Vec<String>>>` 一并返回（`engine.rs:2787` 起），`a_websocket_upgrade_is_relayed_and_counted_once` 断言源站收到的头里有 `upgrade: websocket`、`Connection` 的值恰好是 `upgrade`（不分大小写，`:3395` 起），`a_connect_on_a_sniffed_connection_goes_to_its_target` 断言源站收到的第一行是 `CONNECT x.test:443 HTTP/1.1`（`:3451` 起）；`058e3ea` 的报告第三节那一格当时没有断言撑着。红证据：临时把第 5 步的 `Keep` 改成 `upgrade: false`、只跑 WebSocket 那一条，红在新的断言上，收到的头只有 `get /ws http/1.1` 与 `host: target.test`；用编辑工具改回去。二、`wants_upgrade` 的文档注释与第 7 步 101 那一支的注释里 `Go's reversing proxy` 改成 `Go's reverse proxy`；`promote` 里重复的一段注释删掉 `tracker.spawn` 上面那一段，留 `let _guard = guard;` 上面那一段。三、订正第 33 行的行号范围：新用例在 `058e3ea` 的 `engine.rs:3362` ～ `:3556`，那一行与报告写成 `:3565`（文件只有 3557 行）。门禁（一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 退出码 0；`cargo clippy --all-targets -- -D warnings` 0 个警告；`cargo test -j 8 --workspace --no-fail-fast` 89 个结果行、4835 通过、0 失败、13 忽略，这一次 `rurge-policy` 没有以 `STATUS_HEAP_CORRUPTION` 退出、没有重跑。两条用例单独循环 21 次全过（`loop-task-3-fix-1.log`） | 本提交 |
| 36 | 4 | 第 1 项的订正（协调者的裁定）：brief 写的 `Conn { … }` 少了 `abort`。`serve_http`（`crates/rurge-engine/src/serve.rs`）给 `abort: CancellationToken::new()`——它不是连接令牌的孩子，`rurge_http::serve_connection` 自己与它竞速——，`tracker: self.tracker().clone()`，`token: self.session_token()`（连接令牌）。 | 本提交 |
| 37 | 4 | 第 4 项的订正（协调者的裁定）：fake 地址的租约不放在 `serve_http` 的局部变量里，而进 `ConnUpstreams` 的 `_lease: Option<Lease>`（`crates/rurge-engine/src/http.rs`）：升级之后 `serve_http` 已返回，转发的任务还握着 `Shared`，租约因此活到连接真正结束。`type Lease = rurge_dns::fakeip::Lease`，任务 6 的 `named` 会交出它。 | 本提交 |
| 38 | 4 | 第 5 项的订正（计划漏的，协调者的裁定）：`sniffable_session` 对代理端口上的 fake 地址拿它背后的名字比 `force-http-engine-hosts`；`serve_sniffed` 判 `Other` / `Closed` 要不要进引擎时若拿 `session.dst_host`（还是 fake 地址）去比，同一条连接两处判得不一样。那段提成 `Engine::named_at_a_proxy_port`（`crates/rurge-engine/src/serve.rs`），`sniffable_session` 与新的 `forces_http_engine` 共用；名字没有（返回 `None`）就不进引擎。没有加用例：那要一个跑着的 TUN 再从代理端口去 fake 地址。 | 本提交 |
| 39 | 4 | 第 7 项的补（计划漏的，协调者的裁定）：升级之后 `serve_http` 已经返回，虚拟网卡的停止没有人听。`ConnUpstreams` 带 `stop: Option<CancellationToken>`（`opts.stop` 的克隆），`relay` 照 `serve_sniffed` 的竞速写：`pump` pin 住，`stopped` 先到就取消句柄的令牌、再等 pump 结束，否则等 pump 结束；不另起活到连接令牌被取消的看门任务。新增用例 `stopping_the_adapter_ends_an_upgraded_relay`（`crates/rurge-engine/tests/http.rs`）；红证据是把竞速换成直接等 pump 时它红在记录上（记成 `Failed`，`the connection was reset`）。 | 本提交 |
| 40 | 4 | `stopping_the_adapter_ends_an_upgraded_relay` 的 brief 写“记录 `Completed`、`status` 101”；记录的 HTTP 状态码是任务 5 才加的字段（今天的 `RequestRecord` 没有它），所以本任务的用例断言应用侧读到 `HTTP/1.1 101` 与记录 `Completed`，任务 5 再补上 `statusCode` 的断言（执行者的裁定）。 | 本提交 |
| 41 | 4 | 结构上的三处（执行者的裁定，语义不变）：`Way` 的字段做成 `pub(crate)`，`settle` / `route_session` / `dial_route` 做成 `pub(crate)`，让新的 `src/http.rs` 用；`Engine` 加 `next_id()` 与 `self_arc()` 两个 `pub(crate)` 方法（`me` / `next_session` 仍私有）；挂钩子的那几行从 `new_handle_with` 提成 `pub(crate) fn attach_observers`（`crates/rurge-engine/src/engine.rs`），`new_handle_with`、`new_handle_under` 与 `serve_http` 的承载连接共用。 | 本提交 |
| 42 | 4 | P11 第 2 张表的三条照修正记录第 16 行改了（`crates/rurge-engine/tests/sniff.rs`）：`the_prefix_reaches_the_upstream_whole` 的第二次写换成以 `t / not http…` 开头，与先写的 `GE` 合起来判 `Other`；`the_sniffs_are_counted` 的第二段改成读 12 字节、断言 `HTTP/1.1 502`（记录仍是 5 条）；`force_http_engine_hosts_sniff_another_port` 的第一段换成带 `Connection: close` 的完整请求、断言读到的开头是 `HTTP/1.1 502`，后两段不动。三条改后全过。 | 本提交 |
| 43 | 4 | 门禁（一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 退出码 0；`cargo clippy --all-targets -- -D warnings` 0 个警告；`cargo test -j 8 --workspace --no-fail-fast` 90 个结果行、4843 通过、0 失败、13 忽略（基线 89 / 4835 / 0 / 13：多出的一个结果行是新的 `tests/http.rs`，多出的 8 条通过是 brief 的 7 条用例与第 7 项的一条）；`Cargo.lock` 的 `[[package]]` 仍是 520。带等待的 `cargo test -j 8 --workspace --test http` 循环 20 次全过。一次聚焦运行里 `crates/rurge-engine/tests/pipeline.rs` 的 `a_doq_server_routed_to_a_proxy_without_udp_fails_while_another_answers` 失败过（不在 P11 的两张表里），单独重跑与全量都通过，原因没有查；它与延后事项第 1 行的 DoQ 偶发同一个文件。日志 `gate-task-4.log`、`loop-task-4-http.log` 在工作区（git 忽略）。 | 本提交 |
| 44 | 4 | 修正第 1 轮（评审的第一件，动一处生产逻辑）：`crates/rurge-http/src/engine.rs` 的 `serve_connection` 在连接令牌先到时，若 `counter.routed() == 0` 就当场结束（`error` 为 `None`、不 `graceful_shutdown`），否则照旧优雅关闭。hyper 1.11.1 的事实（路径相对 `hyper-1.11.1/src/`）：新连接从 `KA::Busy` 开始（`proto/h1/conn.rs:57`），整个请求头解析完才再调 `busy()`（`:293`）；`graceful_shutdown` 走 `proto/h1/dispatch.rs:91` 起的 `disable_keep_alive`，只在写的一侧已关或 `has_initial_read_write_state()`（`conn.rs:195`：读写都是 `Init` 且读缓冲为空）时当场关，否则 `Conn::disable_keep_alive`（`conn.rs:886`）只关 `KA::Idle` 的连接，所以第一个请求头读了一半的连接会读到 `header_read_timeout`（嗅探到的连接是 `idle_timeout`，缺省 600 秒）。新用例 `a_stop_before_the_first_head_is_whole_ends_the_connection_at_once`（`engine.rs`；夹具加 `rig_head_wait`，`rig_timed` 转调它给 5 秒）：客户端一次写半个头、取消令牌，2 秒内返回，`routed == 0`、`error == None`；`select!` 是 `biased`（abort、连接、令牌），所以 hyper 一定先读到这些字节。红证据：去掉改法只跑这一条，红在 2 秒的界上（`Elapsed(())`）。 | 本提交 |
| 45 | 4 | 修正第 1 轮（评审的第二、三件，只改注释）：`crates/rurge-engine/src/engine.rs` 的 `route_session` 与 `crates/rurge-http/src/engine.rs` 的 `Upstreams::route` / `dial` 三处文档注释里的 `(design 8.4 "选路")` / `"拨号"` 改成英文（`design 8.4, routing` / `design 8.4, the dial`）；后两处是任务 2 写的，评审时漏了（协调者的裁定）。`crates/rurge-engine/src/http.rs` 的 `_lease` 注释从 `(design 9, R27)` 改成引 `P9`（那是嗅探到的连接在进引擎之前 `turn_back` 一次、租约握到连接结束）；`crates/rurge-engine/tests/sniff.rs` 的 `the_prefix_reaches_the_upstream_whole` 加一句注释说小写 `t` 让 `GEt` 不是方法、判 `Other`、原样转发，而 HTTP 会进引擎（P11）；`crates/rurge-engine/src/serve.rs` 的模块注释与 `serve_sniffed` 的文档注释补上判为 HTTP（与强制进引擎）的交给 `serve_http`。 | 本提交 |
| 46 | 5 | 做了什么（行号是本提交的）：`RequestRecord` 在 `protocol` 之后多了 `method`、`url`、`status_code`（`crates/rurge-engine/src/observe.rs:54`、`:59`、`:64`），`record_of` 取 `h.method()`、`s.url.clone()`、`h.status()`（`:143`）；`RequestJson` 照 `protocol` 之后加三项、序列化成 `statusCode`（`crates/rurge-api/src/routes/requests.rs:64`、`:67`、`:70`），`From` 照搬（`:114` ～ `:116`）；`log_session` 的三行都在 `elapsed_ms` 之后加 `method = %method` 与 `status = %status`（`crates/rurge-engine/src/engine.rs:1446`），空值照 `rule` 的写法（`unwrap_or_default()`；`status` 是 `Option<u16>`，`map(|code| code.to_string()).unwrap_or_default()` 才是空串而不是 `0`），完成的那一行再加 `url = %url`，值由新的私有函数 `logged_url`（`:1439`）取 `?` 与 `#` 之前的部分。派工提示第 2 条：监听器的绝对形式请求（过渡期仍走 `forward()`）不设方法与状态两个侧槽，它们的记录两项在任务 6 之前是 `null`，不为它补。 | 本提交 |
| 47 | 5 | 用例与红证据（一台开发机上的一次运行）：新用例四条在 `crates/rurge-engine/tests/http.rs`——`a_request_record_has_its_method_url_and_status`（`:593`；200、403、`REJECT-TINYGIF` 的 200 在一条连接上，`show-error-page-for-reject` 关着时另起一个 harness 与另一条连接，连接被关、`status_code` 是 `None`）、`a_url_with_userinfo_is_recorded_without_it`（`:685`；嗅探到的连接上绝对形式的 `GET http://u:p@target.test/x` → `url` 是 `http://target.test/x`，派工提示第 3 条说的 `http` 1.5.0 的 `Authority` 与任务 2 的 `form_of` 都对）、`a_borrowed_connection_records_where_it_went`（`:726`）、`the_request_log_line_shows_no_query_and_no_userinfo`（`:772`；`LogCapture`、DEBUG，一条连接上先绝对形式带 userinfo 与 `?x=1` 的请求、再一个被拒的 `/r`）。先写的红是编译错误（`cargo test -j 8 --workspace --test http --no-run`：11 个 `E0609`，`no field method/url/status_code on type RequestRecord`，`red-task-5.log`）。另两条临时一改的红：一、`logged_url` 改成原样返回 → 完成行是 `… status=200 url=http://target.test/a?x=1`，红在 `!line.contains("x=1")`（`http.rs:817`）；二、`record_of` 的 `status_code` 改成恒 `None` → `left: None, right: Some(200)`。两处都用编辑工具改回去了。`stopping_the_adapter_ends_an_upgraded_relay` 补上 `status_code == Some(101)`、删掉说任务 5 才加这个字段的注释（第 40 行留的）。`pipeline.rs` 的 `SESSION_FIELDS`（`:661`）加 `method`、`status`、`url`，`a_field_of_a_log_line_is_read_whole`（`:693`）两行样例照新的行写，断言三个新字段读得出、`policy` 不吞掉后面的、失败行的空 `method=`/`status=` 读成空串；`pipeline.rs` 的四条日志用例（第 595、717、765、787 行起）照过。 | 本提交 |
| 48 | 5 | 门禁（一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 退出码 0（先用 `cargo fmt --all` 改了一处断言块的排版）；`cargo clippy --all-targets -- -D warnings` 0 个警告；`cargo test -j 8 --workspace --no-fail-fast` 90 个结果行、4848 通过、0 失败、13 忽略（基线 90 / 4844 / 0 / 13：多出的 4 条正是本任务的用例）；`Cargo.lock` 不变（`git diff` 无输出）。带等待的 `cargo test -j 8 --workspace --test http` 循环 22 次全过（12 条用例，`loop-task-5-http.log`）。日志 `gate-task-5.log`、`red-task-5.log` 在工作区（git 忽略）。这一次没有二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出，任务 0 起的 DoQ 偶发（延后事项第 1 行）也没有出现。需要参照服务端的互操作用例在本机立刻返回、算在通过的数里 | 本提交 |
| 49 | 6 | 做了什么（行号是本提交的）：`rurge-inbound` 的 `ClientConn`（`session.rs:50`）与 `PlainRequests`（`:68`），`Dialer::plain_requests`（`:612`，第五个带缺省实现的方法），`lib.rs` 导出三者并再导出 `::http::{Request, Response}` 与 `hyper::body::Incoming`（执行者的裁定：引擎实现这个 trait 不必新加 `http`/`hyper` 依赖边，依赖边照计划只有那两条）；`http.rs`：`HandlerError` 删掉，`handle`（`:147`）与 `connect`/`failure_response`（`:323`）用 `Close`，`Ctx` 加 `abort`（`:44`）与 `plain`（`:48`，`OnceLock`），`bind` 里建 `abort`，`serve_connection`（`:84`）与 `abort` 竞速（`biased`，abort 在前；abort 先到就不记那行 DEBUG），`handle` 的次序是鉴权（验过之后去掉 `Proxy-Authorization`）→ `CONNECT` → 绝对形式（`plain`，`:171`：先查 scheme 的 400，文字照今天的；再问 `PlainRequests`，`None` 时 502 页）→ 别的 400；`forward()`、`origin_form`、`absolute_form`、`HOP_BY_HOP`、`host_port`、`strip_hop_by_hop`、`cancelled_outcome` 与只给它们用的 `use`、夹具 `listener_with_target`、`read_response` 删掉。引擎：`ConnUpstreams` 加 `listener`（`http.rs:57`，`new` 六参），`named`（`:85`）为真时问 `Engine::named_fake_address`（`fake_address.rs:35`，与 `turn_back`（`:56`）共用），`open`（`:98`）为真时每个请求 `quic_adaptive.on_tcp` 一次，新的 `PlainRequests`（`:222`）；`Engine::plain_requests`（`engine.rs:1923`）：占位的目标 `0.0.0.0:0`、进程查一次、`HANDSHAKE_TIMEOUT`/`DROP_HOLD`/`listener: true`/`lease: None`/`stop: None`；`connect_session` 的 `forwardable` 删掉、`route_session` 收 `false` | 本提交 |
| 50 | 6 | 派工提示第 1 条改了 brief 第 1 项对 `ClientConn` 的写法：多了第四个字段 `pub abort: CancellationToken`（`session.rs:61`）。它由监听器在 `bind` 的闭包里建、放进 `Ctx`，问 `plain_requests` 时随 `ClientConn` 交出，引擎放进 `Conn::abort`；监听器的 `serve_connection` 与它竞速（`biased`，abort 在前），abort 先到就丢掉连接、不记错误——`rurge-http` 在请求被终止或停止、客户端不再读时要这条连接当场走，而连接的主人是监听器（`Conn::abort` 的文档注释、设计 8.10、8.11） | 本提交 |
| 51 | 6 | 派工提示第 2 条加了一条用例 `a_killed_plain_request_whose_client_does_not_read_is_closed`（`crates/rurge-engine/tests/http.rs:1273`，夹具 `streaming_origin` 在 `:1228`）：客户端经监听器发一个绝对形式的 `GET` 后什么也不读，等到记录的下行字节数大于 0 且两次取样（间隔 200 毫秒）相同（“塞满了”，注释写了这一段等的是什么）再 `engine.kill`；判据是记录在 `WITHIN` 内变成 `Failed("killed")`，之后客户端有界地读到 EOF。红证据：把监听器的竞速改成直接 `conn.as_mut().await`，只跑这一条，红在 `tests/common/mod.rs:437` 的 `timed out waiting for the sessions to finish`（5 秒；`Tail` 握着守卫、卡住的写握着 `Tail`）；改回去之后绿 | 本提交 |
| 52 | 6 | 派工提示第 3 条：`ConnUpstreams::new` 的第六个参数是 `listener: bool`（`crates/rurge-engine/src/http.rs:67`）。`serve_http`（`serve.rs`）给 `false`、别的照旧；`plain_requests` 给 `true`、`lease: None`、`stop: None`——监听器没有自己的停止，升级之后的转发由 `relay::pump` 看句柄的令牌 | 本提交 |
| 53 | 6 | 派工提示第 4 条：`connect_session` 的 `forwardable` 删掉、`route_session` 收 `false`，注释写“经 `Dialer::dial` 来的会话都是隧道（设计 8.4）”；`SessionInfo.url` 的注释（`crates/rurge-config/src/session.rs`）照设计 8.3 改写：每个引擎里的请求都有 URL，以绝对形式交给 HTTP 代理与否由引擎显式给出（`may_forward`），不从 `listener` 与 `url` 推断 | 本提交 |
| 54 | 6 | 派工提示第 7 条定了任务 0 留给本任务的事：`a_client_leaving_mid_upload_is_no_upstream_failure`（`tests/http.rs:1030`）用 smart 组的书断言（照 `tests/smart.rs` 的 `a_plain_request_whose_upstream_hangs_up_counts_against_the_member`）：`Rude = http, …` 读走请求不再答，客户端只发 3 个字节就走，有界地等记录以 `Failed` 结束之后断言 `registry.auto().smart.site("target.test", …).failed` 是空的（`smart::watch` 只在 `upstream_failed() && !first_byte_seen() && !was_killed()` 时记失败，三秒的那次检查跳过已结束的会话）。红证据：临时让 `rurge-http` 的 `upstream_failed` 不看 `is_user()`、对客户端的错误也调 `mark_upstream_failed()`，只跑这一条，红在 `the client's own error counted against the member: ["Rude"]`；改回去之后绿 | 本提交 |
| 55 | 6 | 派工提示第 9 条：没有引擎时的 502 页用 `responses::error_page(StatusCode::BAD_GATEWAY, …)`，`title: "Connection failed"`、`session_id: 0`、`dst` 是 URL 的主机:端口（不带 userinfo，端口缺省 80）、`rule: None`、`chain` 空、`message: "This listener has no HTTP engine behind it."`；不看 `show_error_page`（只有测试的假拨号器会走到这里）。用例 `an_absolute_form_request_without_an_engine_is_a_502`（`crates/rurge-inbound/src/http.rs:671`）断言 502、页里有 `no HTTP engine` 与 `echo.test:80`、`dialer.sessions()` 为空 | 本提交 |
| 56 | 6 | 派工提示第 10 条：反查不到名字的 fake 地址在两路上次序不同，本任务不改。`rurge-http` 的 `Requests::handle` 先 `route` 再看 `prepared.unknown`，所以绝对形式的请求先求过规则（规则的命中数加一、记录带着规则）再答 `fake address not known`；`CONNECT` 那一路（`Dialer::dial` 的 `Turned::Unknown`）在求规则之前就失败。`an_unknown_fake_address_in_a_plain_request_is_a_502`（`tests/http.rs:1194`）断言 502、`Failed`、`error` 是 `fake address not known`、`remote_addr` 为 `None`（没有拨号），不断言记录没有规则；任务 8 写进延后事项 | 本提交 |
| 57 | 6 | 派工提示第 14 条：`log_session` 里完成那一支的注释（`crates/rurge-engine/src/engine.rs`）原来写“the path is DEBUG-only, as the processes and the addresses of the sessions are”，说错了——被拒与失败的两行在 INFO，也带着 `src` 与 `dst`；改成只说事实：只有这一行（DEBUG）带路径，被拒与失败的 INFO 行不带（设计 12）。只改注释 | 本提交 |
| 58 | 6 | 用例：P11 第 1 张表逐条照办——删 `hop_by_hop_headers_and_the_tokens_connection_names_are_dropped`、`plain_requests_are_forwarded_per_request_with_rewritten_headers`、`rejects_and_failures_render_pages_or_close`、`plain_forward_session_is_killable`、`a_client_leaving_mid_upload_is_no_upstream_failure`、`absolute_form_keeps_the_uri_and_applies_the_upstream_headers`、`the_upstream_headers_replace_same_name_headers_including_host`（七条）；`an_ipv4_mapped_target_is_dialed_as_ipv4` 与 `a_session_names_the_client_and_the_listener_port` 留下 `CONNECT` 那一半（转发那一半在引擎的 `an_ipv4_mapped_url_host_is_dialed_as_ipv4`、`a_plain_request_record_names_the_client_and_the_listener_port`）；其余照过；加 `an_absolute_form_request_without_an_engine_is_a_502`。引擎「监听器」一节九条（`tests/http.rs:843`、`:941`、`:988`、`:1030`、`:1089`、`:1117`、`:1151`、`:1194`、`:1273`）。红：在起点 `7a88d1c` 上跑，`rurge-inbound` 那一条红在 502 页写的是 `Upstream request failed: invalid HTTP version parsed`（旧 `forward()` 把请求发给了 echo 服务器）而 `dialer.sessions()` 非空；引擎九条全红（`--test http`：12 passed / 9 failed），失败的形态见报告 | 本提交 |
| 59 | 6 | 门禁（一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 退出码 0；`cargo clippy --all-targets -- -D warnings` 0 个警告（退出码 0）；`cargo test -j 8 --workspace --no-fail-fast` 90 个结果行、4851 通过、0 失败、13 忽略（基线 90 / 4848 / 0 / 13：−7 +1 +9）；`Cargo.lock` 不变（520 个包）；`crates/rurge/tests/sockets.rs` 6 条通过；`cargo test -j 8 --workspace --test http`（21 条，含九条新用例）循环 21 次全过（`loop-task-6-http.log`）；`cargo test -j 8 --workspace -- http::tests` 连 `rurge-inbound` 的 29 条全过；`--test http --test sniff --test tun_dns` 与 P11 第 2 张表任务 6 的六套（`outbounds`、`outbounds_shadow_tls`、`outbounds_tls_family`、`pipeline`、`smart`、`api`）全过。这一次没有二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出，任务 0 起的 DoQ 偶发（延后事项第 1 行）也没有出现。日志 `gate-task-6.log`、`loop-task-6-http.log` 在工作区（git 忽略）。需要参照服务端的互操作用例在本机立刻返回、算在通过的数里 | 本提交 |
| 60 | 6 | 修正第 1 轮（评审第 1 件）：两条 fake 地址的用例没有回环守卫——规则之后紧接着夹具的 `FINAL,DIRECT`，反查或“反查不到就拒”一回落，请求会带着 `198.18.x.y:<端口>` 走 `FINAL,DIRECT` 拨到回环之外。`tun_dns.rs:116` 的 `listener_rules` 挪进 `tests/common/tun.rs:45`（照搬文档注释，`pub`），`tun_dns.rs` 删自己的一份、照旧调用；两条新用例改用 `listener_rules(port)`（`tests/http.rs:1167`、`:1213`；前者不包 `guarded`——请求来自监听器的回环，不是 app 的地址，注释里说了）。第二条的结局不变（502、`remote_addr` 为空），记录带的规则改成断言 `IP-CIDR,198.18.0.0/15,REJECT,no-resolve`：REJECT 在拨号时才生效（`route_session` 不拒 REJECT），而“反查不到”在求规则之后、拨号之前（`rurge-http` 的 `engine.rs:294` 的 `prepared.unknown`），修正记录第 56 行说的“先求过规则”就由这条断言钉住。跑出来正是这条规则 | 本提交 |
| 61 | 6 | 修正第 1 轮（评审第 2 件）：`a_client_leaving_mid_upload_is_no_upstream_failure`（`tests/http.rs:1043`）从 `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]` 改回 `#[tokio::test]`——`SessionHandle::finish` 照挂上的先后跑钩子（记录的钩子先于 `smart::watch` 的），两个工作线程时用例可能恰好在两个钩子之间读书，产品代码退回去了也可能是绿的；单线程时 `finish` 一口气跑完。注释照这个意思重写。重做了看红：临时去掉 `rurge-http` 的 `upstream_failed` 里 `!e.is_user()` 的判断，只跑这一条，红在 `the client's own error counted against the member: ["Rude"]`；改回去之后绿 | 本提交 |
| 62 | 6 | 修正第 1 轮（评审第 3 件）：`a_killed_plain_request_whose_client_does_not_read_is_closed`（`tests/http.rs:1298`）的应答总量从 4 MiB 改成 256 MiB（注释写了为什么：Linux 回环的 `tcp_rmem`/`tcp_wmem` 上限缺省 6 / 4 MiB、调过的机器 16 ～ 32 MiB，macOS 约 4 MiB；4 MiB 可能整个塞进两端的缓冲，请求就做完了，`active()` 空了，两次取样等不到）；源站只在对端肯收时才写，不占内存。`streaming_origin`（`:1253`）的文档注释照实写：它写满 `total` 字节（对端不读就停在写上），然后一直读到对端走 | 本提交 |
| 63 | 6 | 修正第 1 轮（评审第 4 ～ 7 件）：一、`rurge-inbound/src/http.rs:40` 的 `Ctx.abort` 注释说错来处——它是监听器在 `bind` 里给每条连接新建的、经 `ClientConn` 交给引擎当 `Conn::abort`，照实写。二、`a_plain_request_is_killable`（`tests/http.rs:992`）等的是 `origin.requests().len() == 1`（`TestServer` 先记下请求再按 `set_delay` 等），不是“有一条活动的记录”（记录在拨号之前就有，kill 可能落在拨号或发请求时），注释写清。三、`the_listener_rewrites_a_plain_request_for_the_origin`（`:843`）的第二个应答再加一条 `content-length: 43`（转小写后比）。四、同一条用例的规则去掉多余的 `FINAL,DIRECT`（`Profile.rules` 插在夹具那一行之前，夹具会自己加） | 本提交 |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | `crates/rurge-engine/tests/pipeline.rs` 的 `a_doq_carrier_the_proxy_will_not_open_fails_naming_the_policy` 在任务 0 的全量里失败过一次（`:3006`，`doq.connections()` 是 1，而它要 0）：`FakeDoq` 数的是**完成了 QUIC 握手**的连接，而这条用例里去 DoQ 服务器的唯一的路是那个会拒绝登录的 SOCKS5。单独重跑六次全过；那时本分支只有文档提交。原因没有查，两种可能都没有证据：同一个测试二进制里别的用例的 QUIC 客户端（Profile 写了 `encrypted-dns-skip-cert-verification = true`、不验证书）还在重传，撞上了端口被复用的这个 `FakeDoq`——这台机器的临时端口区间不是缺省的（`CLAUDE.md`「这台开发机」）；或者载体在某个竞速里绕过了策略、直连了 DoQ 服务器，若是这样，就是一次本该经代理的 DNS 查询漏到了直连 | 以后的会话：再见到时先分清是哪一种（例如在 `FakeDoq` 里记下每条连接的对端端口，看它是不是这个引擎的）；M1b 的收尾（任务 8）把它写进 `docs/status.md` 的 M1b 一节（修正记录第 20、21 行） |
