# 阶段 4 / M4b「请求记录的完整」实施计划

> **执行方式**：交给 Pi，由协调的会话逐个任务派工、逐个任务评审（设计第 2 节、M4-D1；做法同 M1a ～ M4a）。
>
> - 协调的会话每个任务开一个 Pi 会话，交给它这个任务的全文与要用的接口（派工文件）。Pi 做完把报告写进派工文件指定的路径，在本地提交。
> - 协调者读改动、重跑门禁、评审。有问题回到同一个 Pi 会话修正，再评审；过了才派下一个任务。
> - 每个任务：先写失败的用例、看到红；再实现到绿；跑门禁；对照本任务的「判据」与「全局约束」逐条自查；最后提交（**只在本地，不推送**）。钉住现有行为的用例（任务里写明"不要求先红"的）可以一写就绿。
> - 与计划的出入记进末尾的「执行期修正记录」，**不改计划的正文**。
>
> **停点**：没有。M4b 没有平台代码、没有只在 CI 上编译的代码（设计第 2 节）。M4b 做完之后由项目所有者推送（M1 ～ M4 那时还没有推送的一起），推送之后读回那次 CI；再往后是 M5。

**目标**：让请求记录完整（FR-HTTP-10 的 M4b 部分，设计第 10 节）。

- 记录多三个键：`receiveMs`、`requestBodyBytes`、`responseBodyBytes`（第 10.1 节）。正文的字节数在 HTTP 引擎交出每一帧时、记录结束之前数。
- 可嗅探的会话没有进 HTTP 引擎、或判为 TLS 而 `[MITM]` 的名单命中却没有解密时，备注写九种原因之一（第 10.2 节）。
- `remoteAddr` 照新口径："这条会话离开 rurge 时所经的那条连接或载体的远端"：补全复用的 TCP 载体（snell 连同失效重拨、anytls、`h2pool`、ssh）、QUIC 一族、WireGuard、UDP 流，另有内部的 UDP 会话（第 10.3 节；内部会话是 P8 ④ 订正设计）。
- `dnsMs` 在解析途中也有数（第 10.4 节）。
- 量一次一条 HTTP 请求的记录的大小（第 10.5 节）；手工清单加 M4b 的条目；M4b 的收尾文档。

**架构**：

- 不加包、不加工作区边、不改任何 `Cargo.toml`（P2）。
- 句柄（`rurge-inbound`）多两个计数与一个读数；记录（`rurge-engine` 的 `observe.rs`）与 API（`rurge-api` 的 `requests.rs`）多三个键；两个正文在 `rurge-http` 的引擎里数（`Idle`、`Tail`、`LocalFile` 与 rurge 自己的答复），不在 M4a 包住应答正文的那一层（M4-D30）。
- 直通的原因在 `rurge-engine` 的 `serve.rs` 判、在建这条会话的句柄时写；`rurge-http` 的 `Mitm::decides` 交回五种结果，名单最先看（P6）。
- 远端：`rurge-net` 的 `trail` 多 `current()` 与在途查找的票据 `Lookup`；`Datagram::far_end` 与 `PacketSocket::far_end` 两个有缺省的新方法；复用、共用的载体记着自己拨号时的远端，借出时写进会话的 `Trail`（P7 ～ P11）。

**技术栈**：都已在 `Cargo.lock` 里，源码在本机 cargo 注册表里。**不下载、不加新包**（M4-D2）。

- hyper 1.11.1：HTTP/1.1 服务端不写 HEAD 应答的正文（`proto/h1/role.rs` 的 `can_have_body`，`proto/h1/dispatch.rs` 在 `!can_write_body` 时丢下正文）；HTTP/2 服务端不看 HEAD、照样把正文发出去（`proto/h2/server.rs` 第 515 行附近：`body.is_end_stream()` 为假就建 `PipeToSendStream`）。
- tokio：`tokio::time::Instant`（暂停的时钟之下按暂停的时钟走；不在运行时里时取系统时钟）、`task_local!` 的 `try_with`。
- quinn：`Connection::remote_address` **不用**（P10：跳端口与经底层代理的路径上它是替身或第一个端口）。

**依据**：

- M4 细化设计（下面写"设计"指它）：`docs/superpowers/specs/2026-11-01-phase4-m4-capture-records-design.md`。
  - M4b 用到第 10 节、第 11 节第 6 ～ 8、11 条、第 12 节第 6、7 条、第 13、14（V13、V14）、16（M4-D21 ～ D25、D30、D31）、17 节。
  - **先读它第 18 节**：本计划改了设计正文的地方随本计划写在那里（"写 M4b 计划时"之下）。
  - 设计与本计划的「计划期决定」不一致的，以计划为准。
- M4a 计划（`docs/superpowers/plans/2026-11-01-phase4-m4a-capture-plan.md`）的执行期修正记录；`docs/status.md` 阶段 4 / M4a 一节「以后的会话必须知道的事」整段适用，M3b、M2b、M1b 三节的同名段落也适用。
- `remoteAddr` 与 `dnsMs` 的旧说法：`docs/api/phase1.md` 第 59 ～ 70 行与阶段 1 / M5a 计划延后第 3、20、21、22 行。**历史段落不改**，`docs/api/phase4.md` 写订正（任务 9a）。
- 写计划时对着 `33efe98`（分支 `phase4-m4`，与本地 `main` 相同）核对过的事实写在「计划期决定」里。外部库的事实对着注册表里锁定版本的源码。

## 全局约束

**依赖**

- **不改任何 `Cargo.toml`**：要用 `rurge_net::trail` 的 crate（`rurge-proto`、`rurge-proto-quic`、`rurge-proto-ssh`、`rurge-proto-wireguard`、`rurge-dns`、`rurge-policy`、`rurge-http`、`rurge-engine`、`rurge-inbound`）都已正式依赖 `rurge-net`。
- `Cargo.lock` 的 `[[package]]` 今天 520 个，每个任务之后照旧 520 个，锁文件一个字不变。`cargo` 要下载任何东西就停下报告。
- 依赖方向不变：`rurge-http` 不依赖 `rurge-engine`；`rurge-inbound` 不依赖 `rurge-http`；`rurge-net` 不依赖任何协议 crate。
- `unsafe_code`：不加放开的函数，`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 仍是 23 项。
- `crates/rurge/tests/sockets.rs` 的 `LISTED` 仍是 26 行：M4b 不建套接字、不问解析库、不建不带钩子的连接器。名单之外的原因让那条扫描用例变红，就停下报告。

**缺省的 `cargo test` 不碰系统**

- 同 M4a：不建网卡，不改系统的任何设置；不执行 `netsh`、`route`、`ipconfig`、`ip`、`resolvectl`、`networksetup`。
- **测试从不把任何 CA 装进信任库，也不读系统的信任库**：建引擎的用例一律给 `EngineShared.roots`；解密的用例用 `TlsFixture` 的根。
- 量记录大小的两条用例带 `#[ignore]`，缺省的 `cargo test` 不跑；它们读峰值工作集时起的 PowerShell 是只读的系统工具（同 `crates/rurge-http/src/capture/memory.rs`）。

**测试的约定**

`CLAUDE.md`「测试约定」整节适用。写用例之前读 `docs/testing.md`「测试约定（全文）」的阶段 4 各节。与本计划直接相关的：

- 只用回环与端口 0。**断言的远端地址都是回环上夹具的地址**（假服务端、中继、源站的 `addr()`）；文档地址（`192.0.2.x`、`2001:db8::…`）只出现在不拨号的单元用例里（`Trail` 的用例、`set_peer` 的哨兵）。
- 示例主机名经一行 `[Host]` 钉到 `127.0.0.1`，第二道防线是共用夹具的 `dns-server`（回环上的 `MockDns`）。
- 等待都有时限。"记录结束了"、"`dnsMs` 涨到了"用有时限的轮询等（`wait_until`），不用固定的等待；固定等待只许"断言什么都没发生"，注释写它等的是什么、余量多大。新写的、带等待或并发的用例提交前单独循环跑二十次以上（任务 2、3、4、5、6、7）。
- 抓日志的用例用 `LogCapture`，跑在当前线程运行时。测试里手写的 HTTP/1.1 客户端把每个请求——请求头与正文——在一次写里发出。
- 要发大于 9216 字节数据报的测试 socket 先调高发送缓冲；从 `PacketSocket` 收包的缓冲给 64 KiB。
- **内联的测试模块放在文件的末尾**：`crates/rurge/tests/sockets.rs` 读生产代码，读到第一个 `#[cfg(test)] mod … {` 为止。

**门禁与这台开发机**（每个任务末尾一次；说"通过"之前必跑）

- 门禁：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`。
- 本机 msvc 工具链没有 rustfmt。跑 `cargo fmt` 之前让环境变量 `RUSTFMT` 指向 gnu 工具链里同版本的 rustfmt：用户目录下的 `.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe`。不要为此安装或升级工具链。
- **门禁的日志用重定向写、再追加退出码，不用 `Tee-Object`**；单独重跑的结果也写进日志再引，引之前先看它在、看它写的是什么。
- 聚焦地跑用例也经整个工作区：
  - 单元测试：`cargo test -j 8 --workspace --lib -- <过滤词>`；
  - 集成测试：`cargo test -j 8 --workspace --test <名字> -- <过滤词>`。
- **不用 `-p`**：它另编一份依赖，`target/` 涨得很快，写满过 D: 盘。**也不用 `--release`**：量记录大小的用例在调试构建里跑（P12）。
- 跑全量之前 `df -h /d`：剩余不足约 15 GiB 就停下报告。
- 测试二进制以 `STATUS_HEAP_CORRUPTION`（本机管控软件注入的钩子 DLL）或 `STATUS_ACCESS_VIOLATION` 退出：单独重跑那一个二进制，报告里写明。
- `cli` 的用例偶发 `give_up` 20 秒超时：单独重跑 `cli`，把标准错误原样抄进报告。
- rustc 在增量编译里报内部编译器错误：删掉出错的那个目标在 `target/debug/incremental/` 下的目录再编。
- 回环的 TCP / UDP 7、9、13、17、19 号端口在这台机器上开着，CI 上关着：用例不假定任何固定端口的状态。
- 结果照实记：本机的全量运行是"一台机器上的一次运行，不是 CI"；需要参照服务端的互操作用例在本机立刻返回、算在通过的数里，要说出来。

**仓库的规矩**（`CLAUDE.md`「必须遵守的规矩」整节适用；与本计划直接相关的）

- **凭据**：M4b 不加带凭据的字段；备注与远端地址里没有凭据。记录里的 URL 照 M1b（不带 userinfo）。
- **日志的分级**：M4b **不加日志行**。`decrypting` 里已有的两行 DEBUG（没有可用的 CA、ALPN 合不上）照旧，文字不变；新的三种原因（开关关着、来源不允许、名字签不了证书）只进备注。
- **诊断**：不设新码。
- **网络与进程**：会话的进程照旧读 `SessionHandle` 的进程槽，M4b 不动它。
- **出站套接字只有一处建法**：M4b 只读已建好的套接字与连接的地址，不建新的。
- 文档与报告用写文件 / 改文件的工具改。**永远不要用把一个文件的内容经管道写回同一个文件的命令**（2026-10-27 这样写满过 D: 盘）。
- 计划末尾两张表里已写下的行不改写、不删除。追加新行时**不要把已有的行当成锚点替换掉**（M2a 任务 9 这样删掉过一行）。

**写法**

- 注释、日志、错误文本与诊断用英文，照所在文件的写法：文档注释写"为什么"与前提，句子完整。注释里提到设计或计划，写 `phase 4 M4 design 10.3`、`the phase 4 / M4b plan's P8`。
- 提交说明照 `git log` 的样子写，标题用中文：`feat(inbound): …`、`feat(engine): …`、`feat(api): …`、`feat(http): …`、`feat(net): …`、`feat(dns): …`、`feat(proto): …`、`feat(policy): …`、`feat(proto-quic): …`、`feat(proto-ssh): …`、`feat(proto-wireguard): …`、`test(engine): …`、`docs: …`。正文说做了什么、改了哪些现有用例的期望、为什么。Pi 的提交不带署名行。
- 只在本地提交：不 `push`、不 `pull`、不 `fetch` 远端、不 `amend`、不改写历史。

**不做的事**

- 只运行本仓库构建出来的二进制与 Pi 自己。不下载、不安装任何东西。不手动跑 `rurge run`、`rurge dns lookup`；`rurge service` 只用 `--dry-run`。
- 参考实现：GPL 的（sing-box、mihomo、Xray-core）只取事实，代码不抄。
- 不做设计第 1 节「不做」一栏的事，也不做这些：
  - 来源的设备（阶段 7）、脚本的备注（阶段 5）；
  - 改 `--request-log-size` 的缺省（M4-D25，量到的数交给所有者）；
  - 给 `vmess`、`h2-connect`、MASQUE 的 UDP 补远端（按目标在另一个任务里懒开，照旧 `null`，设计第 10.3 节登记）；
  - 改 `docs/api/phase1.md`、阶段 1 的计划与别的历史段落（订正写在 `docs/api/phase4.md`）；
  - 改 M4a 的抓包（`capture` 模块、`CaptureSlot`、HAR）——抓包的 `remote` 读的是同一个 `Trail`，跟着新口径变，不另改；
  - 修 HTTP/2 上 HEAD 的应答带正文这件事（hyper 的行为，M4b 只照实数，P5 ③）。

**计划没有写到、要做决定的地方**

- 能按设计与总设计的原则定的：自己定，记进修正记录，写"执行者的裁定"。
- 要停下来列出、交给协调者的：去向写着"项目所有者"的（设计第 17 节）；会改变 M4-D1 ～ D31 的。
- 现有用例变红：P15 的表点了名的，照计划改，在提交说明里说；没点名的，停下来，记进修正记录，不顺手改期望值。

## 评审要点

设计没写透、而最可能咬到使用者的几种输入，与负责它们的任务和用例：

1. **经底层代理（`underlying-proxy`）的复用、QUIC 与 UDP 载体**：hysteria2 经 socks5 的 UDP 中继、snell 的池连接经 socks5、DoQ 的内部会话经 socks5 的关联。
   - 期望：记录是第一跳（中继或代理的地址），从不是替身地址 `192.0.2.1:443`，也从不是 UDP 关联所靠的那条 TCP 控制连接。
   - 用例（任务 5、6、7）：`a_carrier_over_a_socks5_association_records_the_relay`、`a_pooled_snell_connection_through_a_socks5_proxy_records_the_first_hop`、`a_carried_path_far_end_is_its_carriers_not_the_stand_in`、`hysteria2_through_a_socks5_relay_records_the_relay`。
2. **借来的连接与共用的连接**：同一条 snell / anytls / HTTP/2 / SSH / QUIC 连接上先后或同时的几条会话；池里的连接失效、转发时换新连接重发。
   - 期望：每一条会话都有那条连接的远端，不只是拨了它的那一条；重拨之后是新连接的远端，重拨花的解析时间算进这条会话的 `dnsMs`。
   - 用例（任务 6、7）：`a_pooled_snell_connection_gives_its_record_the_servers_address`、`a_redial_notes_the_new_connection_on_the_sessions_trail`、`a_pooled_anytls_session_gives_its_record_the_servers_address`、`every_stream_on_a_shared_h2_connection_records_its_address`、`every_channel_on_an_ssh_session_records_its_address`、`every_stream_on_a_quic_connection_records_the_servers_address`。
3. **卡住或很慢的解析**：上游 DNS 几秒才答、外层超时打断查找、同一个名字并发的查找。
   - 期望：进行中的会话的 `dnsMs` 随时间增长，结束之后是合计；被打断的记它已花的时间；不在任何 `follow` 里的查找不写进任何会话。
   - 用例（任务 4）：`a_lookup_under_way_counts_up_to_now`、`a_lookup_under_way_shows_on_the_trail_while_it_waits`、`an_active_session_shows_the_time_its_lookup_has_taken_so_far`，现有的 `a_lookup_cut_short_is_noted_for_the_time_it_took` 照过。
4. **不是一问一答的 HTTP**：HEAD、`101` 升级（WebSocket）、客户端在正文中途走掉、分块与流式的应答、几 GB 的下载。
   - 期望：两个正文的字节数是交出去的、到记录结束那一刻为止的数；升级之后的字节不算正文（算在 `up` / `down`）；HTTP/1.1 上 HEAD 的应答正文是 0；计数是 `u64`、不回绕。
   - 用例（任务 2）：`a_chunked_answer_counts_every_frame`、`a_head_answer_counts_no_body_on_http1`、`an_upgrade_counts_no_body`、`a_client_that_leaves_mid_body_counts_what_was_handed_over`。
5. **原因与结局不一致的会话**：命中 `always-raw-tcp-hosts` 而又被规则拒绝的、嗅探中途客户端关掉的、暂停的引擎、被预匹配拒绝的、fake 地址背后没有名字的。
   - 期望：备注照判断次序取最先成立的那一条、建句柄时写下，之后被规则拒绝的照样带着（登记，P6 ⑤）；客户端关掉的（`Sniffed::Closed`）不写（P6 ④）；暂停的、fake 地址背后没有名字的不写；被预匹配拒绝的只在名单没有命中时不写（名单排在预匹配之前，P6 ⑤）。
   - 用例（任务 3）：`a_listed_raw_host_that_the_rules_reject_keeps_its_note`、`a_client_that_closes_during_the_wait_has_no_note`、`a_paused_engine_writes_no_reason`、`pre_matching_writes_no_reason_unless_a_list_came_first`。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `crates/rurge-inbound/src/session.rs`、`crates/rurge-engine/src/observe.rs`、`crates/rurge-api/src/routes/requests.rs` | 1 | 句柄的两个计数与 `first_byte_at`；记录与 API 的三个键 |
| `crates/rurge-http/src/engine.rs` | 2 | `Idle`、`Tail`、`LocalFile` 与 rurge 自己的答复数正文 |
| `crates/rurge-http/src/mitm/decrypt.rs`、`crates/rurge-engine/src/{serve.rs,engine.rs}`、`crates/rurge-engine/tests/{sniff.rs,mitm.rs}` | 3 | 九种原因 |
| `crates/rurge-net/src/trail.rs`、`crates/rurge-dns/src/resolver.rs`、`crates/rurge-engine/tests/pipeline.rs` | 4 | 在途的查找、`current()` |
| `crates/rurge-net/src/{connector.rs,packet_datagram.rs}`、`crates/rurge-policy/src/cell.rs`、`crates/rurge-engine/src/{dns_pipeline.rs,udp.rs,observe.rs}`、`crates/rurge-proto/src/{shadowsocks/udp.rs,socks5.rs,stream_udp.rs,anytls/mod.rs,trojan.rs,snell/mod.rs,snell/udp.rs}` 与 UDP 的用例 | 5 | 两个 `far_end`；UDP 流与内部 UDP 会话的远端；记录不再分 TCP 与 UDP |
| `crates/rurge-proto/src/{snell/*,anytls/*,h2pool/mod.rs}`、`crates/rurge-proto-ssh/src/outbound.rs` 与 TCP 载体的用例 | 6 | 复用、共用的 TCP 载体记着远端 |
| `crates/rurge-net/src/quic/{mod.rs,hop.rs}`、`crates/rurge-proto-quic/src/{session.rs,hysteria2/udp.rs,tuic/udp.rs}`、`crates/rurge-proto-wireguard/src/{device.rs,outbound.rs,udp.rs}` 与它们的用例 | 7 | QUIC 一族与 WireGuard 的远端 |
| `crates/rurge-api/tests/api.rs`、`crates/rurge-engine/src/observe/memory.rs`（新）、`crates/rurge-engine/src/observe.rs` | 8 | 经 API 的端到端；量一条记录的大小 |
| `docs/acceptance/phase4-manual.md`、`docs/api/phase4.md`、`docs/testing.md` | 9a | 手工清单加 M4b 的条目；M4b 的 API 与测试文档 |
| 收尾文档（任务 9b 列全） | 9b | M4b 的收尾 |

模块的名字与拆法可以照实际调整，调了记进修正记录。

---

## Task 0：开工前核对

**做什么**：对着当时的分支核对下面每一项。有出入的记进「执行期修正记录」，一项一行，写"计划写 X，代码是 Y，按 Y 做"；按记录调整后面的任务。**不改计划的正文。** 没有出入也记一行。

1. **工作树**：`git status --short` 空；当前分支 `phase4-m4`；`33efe98` 在历史里，之后只多了本计划的提交（本计划、设计第 18 节、`docs/README.md`、`CLAUDE.md`）。
2. **读**：设计第 18 节（先读）、第 10 节、第 11 节第 6 ～ 8、11 条、第 12 节第 6、7 条、第 13、14、16、17 节；本计划的「计划期决定」、「表后的原文」与 P15 的表；`docs/status.md` 阶段 4 / M4a 一节（「以后的会话必须知道的事」整段）与 M3b、M2b、M1b 三节的同名段落；`docs/api/phase1.md` 第 59 ～ 70 行。
3. **名字与签名**（设计第 14 节 V13、V14）：用 `git grep -n` 逐项找到、记下行号；与「计划期决定」写的不同的照实记。
   - `rurge-inbound` 的 `session.rs`：`SessionHandle` 的字段（`up`、`down`、`connected`、`first_byte`、`method`）、三个构造函数里建全部字段的那一个（`new_with_slot`）、`add_up`、`add_down`、`bytes`、`mark_connected`、`mark_first_byte`、`connect_time`、`first_byte_time`、`set_method`、`method`、`add_note`、`notes`、`trail`、`elapsed`；它的测试模块。
   - `rurge-engine` 的 `observe.rs`：`RequestRecord` 的字段与文档注释（`remote_addr` 那一段）、`record_of`（`elapsed_ms`、`remote_addr` 的 `match s.transport`、`dns_ms`）、`RequestLog::{new, mark_active, record_finished, recent, active}`；测试模块里的 `a_record_tells_the_port_the_address_and_the_lookup_time_of_its_session`、`the_record_of_a_udp_session_has_no_remote_address`。
   - `rurge-api` 的 `routes/requests.rs`：`RequestJson` 的字段、`From<&RequestRecord>`；测试模块的 `record()`、`every_field_of_a_record_is_sent_under_its_own_name`、`the_timings_are_null_until_known`、`the_listen_port_the_remote_address_and_the_lookup_time`；`git grep -n 'RequestRecord {'` 列出的全部字面量。
   - `rurge-http` 的 `engine.rs`：
     - `Idle`（字段、`new`、唯一的调用处、`poll_frame` 里数据帧那一支）与建 `Idle` 的地方这个请求的句柄叫什么；
     - `Tail`（字段 `guard`、`new`、`poll_frame` 里 `PolledUpstream::Frame` 那一支与其中 `complete()` 的位置）；`LocalFile`（字段 `guard`、`poll_frame` 的 `LocalRead::Data` 那一支）；
     - rurge 自己作答的每一处与它结束记录的那一句（`finish`、`guard.finish` 一类）的行号：`answer` 的各支（`TinyGif`、`Reject` / `NoDrop`、`Drop`、`Fail`，哪一支在建应答之前就结束了记录）、`failed`、`timed_out`、`page`、URL Rewrite 当场答复的两支（`Redirect`、`Reject`）、`map_local`（内存里的数据、HEAD 的 `data = None`、文件经 `LocalFile`、结束记录的那一支）；
     - 引擎的应答正文的类型与它的 `size_hint`；`self.shared.conn.h2`；`responses.rs` 的 `full`、`empty`、`error_page`、`tiny_gif`；
     - 测试模块的 `Fake`（`Inner` 的字段、`open`、`handle`、`finished`）、`Rig`、`rig_h2`、`h2_request`、`fixed_origin`、`chunked_origin`、`recording_origin`、`upgrading_origin`、`streaming_origin` 与 `Holding`、测 Map Local 的现有用例与它的夹具、测 HEAD 的现有用例（有没有）。
   - `rurge-engine`（V14）：
     - `serve.rs` 的 `sniffable_session` 的七步（行号）、`named_at_a_proxy_port`、`forces_http_engine`、`named_target`、`widened_by_mitm`、`decrypting`（开关、`named_target`、SNI、没有 CA、`decides` 各在哪一步，两行 DEBUG 的原文）、`serve_sniffed`（`Sniffed` 的六支与其后的 `self.dial(session)`）；
     - `engine.rs` 的 `impl Dialer for Engine` 的 `dial`（`snapshot`、`turn_back`、`look_up_owner`、`new_handle_with`、`Turned::Unknown`、`quic_adaptive.on_tcp`、`trail.follow(self.connect_session(…))`）、`is_suspended`、`mitm_on` 与用例里关掉 `mitm` 开关的写法；
     - 入口调 `sniffable` 与 `dial` 的几处（`rurge-inbound` 的 `http.rs`、`socks5.rs`，`rurge-tun` 的 `session.rs`）。
   - `rurge-http` 的 `mitm/decrypt.rs`：`Decision`、`decides`（四步的次序）、`blocked`、`widens`、`effective`；`git grep -n 'Decision::'` 列出的全部；`decides_in_the_design_order`、`with_h2_an_h2_only_client_is_decrypted` 与别的断言 `Decision::Pass` 的用例。
   - `rurge-config`：`[MITM]` 一节的 `effective` 与它的 `matches`、`matches_sni`；`General` 的 `always_raw_tcp_hosts`、`always_raw_tcp_keywords`、`force_http_engine_hosts`。
   - `rurge-net` 的 `trail.rs`（V13）：`CURRENT`、`Trail`、`Left`、`follow`、`peer`、`clear_peer`、`set_peer`、`dns`、`connected`、`canonical`、`looked_up` 与测试模块；`rurge-dns` 的 `resolver.rs` 的 `Noted`、它唯一的建处（`lookup_inner`）、同一个名字在途时别的查找怎样等（合并的等待）、文件用的是哪一个 `Instant`；`git grep -n 'looked_up'` 列出的全部。
   - `rurge-net` 的 `connector.rs`：`Datagram`（`peer_addr` 的文档与用到它的地方：`rurge-proto-wireguard` 的 `land` 拿它比端点）、`PacketSocket`、`DirectDatagram`、`DirectPacket`；调 `trail::connected` 的两处（`DirectConnector::connect`、`rurge-proto` 的 `external.rs`）；`packet_datagram.rs` 的 `PacketDatagram`（字段与建法）；`rurge-policy` 的 `cell.rs` 的 `Relayed`；`rurge-engine` 的 `dns_pipeline.rs` 的 `FinishOnDrop`、`CountingDatagram`、`wrap_internal_udp` 与它的调用处。
   - 生产代码里 `impl … Datagram for` 与 `impl … PacketSocket for` 的全部（`git grep -n 'impl.*Datagram for\|impl.*PacketSocket for' -- 'crates/*/src'`，去掉测试模块里的），一项一行，与 P8 的表比对。
   - UDP 载体：`SsUdp`（`server`、`socket`）、`Socks5Udp`（`relay`、`socket`）、`StreamUdp::new` 与它的调用处、`SnellUdp::open` 与调用处、`Hysteria2Udp`（`bound`、`Bound.lease`、`open` 里先 `bind`）、`TuicUdp`（同样的槽，何时绑定）、`MasqueUdp`、`H2Udp`、`VmessUdp`（按目标懒开）、`TunnelUdp`（持有的设备、`resolve`）、`external` 的 UDP（`external.rs` 里的 `Socks5Udp::open`）。
   - `rurge-engine` 的 `udp.rs`：`run_flow`（`follow` 包住的是哪一段）、`open`（`Carriers::get`、`resolve` 之后的 `mark_connected`）、`Carrier`（`socket`、`routes`）、`Carriers::get`；测试模块里的 `HearsOnce`、`Stuck`。
   - TCP 载体（V13）：snell 的 `Dialer::fresh`、`SnellOutbound::open`、`open_udp`、`pool.rs` 的 `Pool`（`idle`、`put`、`take`、`reap`、`len`）、`tunnel.rs` 的 `Conn`、`Retry`、`SnellTunnel::{new, reused, fail, poll_open}`、`Drop` 与 `finish`，`snell/mod.rs` 测重拨的现有用例；anytls 的 `new_session`、`open`、`open_udp`、`pool.rs` 的 `Pool`、`session.rs` 的 `Session`（字段、`start`、`with_synack_timeout`、`open`）、`AnyTlsStream` 的 `Drop`；`h2pool/mod.rs` 的 `H2Pool::{open, lease, dial_one, connect}`、`Conn`（字段与 `lease`）、`Lease`；ssh 的 `Session`（字段）、`dial`、`session`、`establish`。
   - QUIC（V13）：`rurge-net` 的 `quic/mod.rs` 的 `Path`（字段、`open`、`open_resolved`、`open_over` 的三种、ECN 那一层）、`hop.rs` 的 `Shared`（`server`、`remote`、`state().port`）与 `Hopping`（`shared`、`remote`）、`carrier.rs` 的 `Carried` 与 `STAND_IN`；`rurge-proto-quic` 的 `session.rs` 的 `Dialer::{connect, path}`、`Session::{connect, dial, take, lease, request}`、`Conn`（字段与唯一的建处）、`Lease`；`rurge-dns` 的 `upstream/session.rs` 调 `quic::connect` 的那一处（不改）。
   - WireGuard（V13）：`device.rs` 的 `Shared`（字段与唯一的建处）、`Device`、`Carrier`、`start`（在 `JoinSet` 里拨载体）、持有 `carriers` 的那个结构与 `git grep -n 'carriers\['` 列出的每一处赋值、`land`；`routes.rs` 的 `Routes::{new, lookup}`；`stack.rs` 建 `routes` 的那一句；`outbound.rs` 的 `dial`、`open_udp`；`udp.rs` 的 `TunnelUdp`。
   - 用例的夹具：`tests/common/mod.rs` 的 `Harness`、`Profile`、`harness`、`connect_via_http`、`echo_through`、`udp_associate`、`udp_echo`、`wait_until`、`the_records`、`finished`；`FakeSocks5`（UDP 中继的地址怎样读）、`FakeSnell`（`SnellScript::tunnels_per_connection`）、`FakeAnyTls`、hysteria2 / tuic / masque 的假服务端、WireGuard 的假对端（`outbounds_wireguard.rs`）、`MockDns::set_delay`、`FakeDoq`；`tests/sniff.rs` 的 `sniffing()`、`handshake`、`socks5_connect`；`tests/mitm.rs` 里关掉开关、不给 CA、写 `client-source-address`、`h2` 的现有用例。
   - 外部库：hyper 1.11.1 的 HTTP/1.1 与 HTTP/2 服务端对 HEAD 的处理（「技术栈」两处），行号写进记录。
4. **数**：
   - `Cargo.lock` 里 `[[package]]` 的个数（520）；
   - `unsafe_functions.rs` 的 `LISTED` 的项数（23）；
   - `sockets.rs` 的 `LISTED` 的行数（26）；
   - 带本机账户名的文件数：派工文件给出要数的那个名字，数出来是 26。
5. **现有用例**（P15 的表）：逐条找到、读它断言什么，把"预计要改 / 预计照过"逐条写进修正记录，一条用例一句。表里没有的照实补一行。另扫一遍：`git grep -n 'remote_addr' -- 'crates/*/tests' 'crates/*/src'`、`git grep -n '"remoteAddr"'`、`git grep -n 'Decision::Pass'`、`git grep -n 'Noted\|looked_up'`、`git grep -n 'StreamUdp::new\|SnellUdp::open\|SnellTunnel::new\|SnellTunnel::reused\|Session::start'`。
6. **门禁**：跑一遍，记下测试二进制数、通过数、失败数与忽略数（M4a 任务 8 修正轮时是 93 / 5309 / 0 / 15，一台机器上的一次运行，不是 CI）。

**判据**：修正记录里有任务 0 的行；门禁的数字记下。

---

## Task 1：句柄的计数与记录、API 的三个键

**改哪里**：

- `crates/rurge-inbound/src/session.rs`：`SessionHandle` 多两个字段 `request_body: AtomicU64`、`response_body: AtomicU64`（紧跟 `up`、`down`，建全部字段的构造函数里初始化为 0）；四个方法；测试模块里的用例。
- `crates/rurge-engine/src/observe.rs`：`RequestRecord` 多三个字段、`record_of` 填它们；测试模块里的用例。
- `crates/rurge-api/src/routes/requests.rs`：`RequestJson` 多三个字段、`From` 填它们；测试模块的两处 `RequestRecord` 字面量补上三个字段，用例照 P15 改。

**接口**

- 消费：现有的 `elapsed`、`method`、`first_byte`。
- 产出：

```rust
// rurge-inbound
impl SessionHandle {
    /// 请求正文发往上游的字节（P3）
    pub fn add_request_body(&self, n: u64);
    /// 应答正文交给客户端的字节（P3）
    pub fn add_response_body(&self, n: u64);
    /// (请求正文发往上游的, 应答正文交给客户端的) 到此刻为止
    pub fn body_bytes(&self) -> (u64, u64);
    /// 从会话开始算起、第一个上游字节回来的那一刻；没有回来过是 None
    pub fn first_byte_at(&self) -> Option<Duration>;
}

// rurge-engine 的 RequestRecord（字段的位置与文档注释见 P4）
pub receive_ms: Option<u64>,
pub request_body_bytes: Option<u64>,
pub response_body_bytes: Option<u64>,

// rurge-api 的 RequestJson（serde 的 camelCase：receiveMs、requestBodyBytes、responseBodyBytes）
pub receive_ms: Option<u64>,
pub request_body_bytes: Option<u64>,
pub response_body_bytes: Option<u64>,
```

**用例（先红）**

- `session.rs`：
  - `the_body_counters_add_up_apart_from_the_bytes`：新句柄 `body_bytes() == (0, 0)`；`add_request_body(3)`、`add_request_body(4)`、`add_response_body(10)` 之后是 `(7, 10)`；`bytes()` 仍是 `(0, 0)`（两个计数不碰 `up` / `down`）。
  - `first_byte_at_is_counted_from_the_start`：`mark_first_byte` 之前是 `None`；`mark_connected`、`mark_first_byte` 之后 `first_byte_at()` 是 `Some(f)`，`f <= elapsed()`、`f >= connect_time().unwrap()`，而且 `first_byte_time() == Some(f - connect_time().unwrap())`。不睡、不等。
- `observe.rs`：
  - `a_finished_record_has_its_receive_time_and_its_body_bytes`：一个 `set_method("GET")` 的句柄，`mark_active`、`mark_connected`、`mark_first_byte`、`add_request_body(5)`、`add_response_body(11)`。进行中读 `active()`：`receive_ms` 是 `None`，两个字节数是 `Some(5)`、`Some(11)`。`finish(Completed)` 与 `record_finished` 之后读 `recent(1)`：`receive_ms` 是 `Some(r)` 而且 `r <= elapsed_ms`，两个字节数照旧。
  - `a_session_that_is_no_http_request_has_no_body_bytes`：不设 `method` 的句柄加了正文字节也是两个 `None`；结束之前收到过上游字节的有 `receive_ms`，没有收到过的 `receive_ms` 是 `None`。
  - `the_receive_time_is_none_without_a_first_byte`：设了 `method`、没有 `mark_first_byte`，结束之后 `receive_ms` 是 `None`，两个字节数是 `Some(0)`。
- `requests.rs`：
  - `every_field_of_a_record_is_sent_under_its_own_name`（P15，预计要改）：字面量补 `receive_ms: Some(30)`、`request_body_bytes: Some(5)`、`response_body_bytes: Some(1234)`，期望的 JSON 多 `"receiveMs": 30`、`"requestBodyBytes": 5`、`"responseBodyBytes": 1234`。
  - `the_timings_are_null_until_known`（P15，预计要改）：`receiveMs`、`requestBodyBytes`、`responseBodyBytes` 都是 `null`。
  - `the_new_keys_sit_where_the_design_puts_them`：把一条记录序列化成字符串，`"dnsMs"` 的位置小于 `"receiveMs"`、`"receiveMs"` 小于 `"status"`；`"statusCode"` 小于 `"requestBodyBytes"`、它小于 `"responseBodyBytes"`、它小于 `"up"`（设计第 10.1 节：紧跟 `dnsMs`、紧跟 `statusCode`）。

**实现**：照 P3、P4。

**判据**

- 上面的用例都在、都绿；P15 点名的两条照改，提交说明里说。
- `RequestRecord` 的字面量全都补齐（`git grep -n 'RequestRecord {'`）；没有别的用例变红。
- 门禁绿。

---

## Task 2：引擎里数两个正文

**改哪里**：`crates/rurge-http/src/engine.rs`：`Idle` 多一个字段与 `new` 的一个参数；`Tail`、`LocalFile` 的 `poll_frame` 各多一句；rurge 自己作答的各处在结束记录之前记下正文的长度；测试模块的 `Fake` 多一个口子与新用例。

**接口**

- 消费：任务 1 的 `SessionHandle::{add_request_body, add_response_body, body_bytes}`。
- 产出：engine 里私有的 `fn own_body(handle: &SessionHandle, response: &Response<ResponseBody>, h2: bool)`（P5 ④）；测试模块的 `async fn body_at_finish(&self, n: usize) -> (u64, u64)`（P5 ⑤）。

**用例（先红）**（`engine.rs` 的测试模块；`n` 是 `Fake` 的句柄的下标；读的都是**记录结束那一刻**的数，`Fake::body_at_finish`）

- `the_request_body_sent_upstream_is_counted_before_the_record_ends`：经引擎 `POST` 一个 3000 字节的正文到 `recording_origin`（请求头与正文一次写出）：`body_at_finish(n).0 == 3000`；同一条连接上随后一个 `GET`：`.0 == 0`。
- `a_one_frame_answer_is_counted_before_the_record_ends`：`fixed_origin` 用 `content-length: 5` 一帧答 `hello`：`.1 == 5`（`Tail` 先 `complete()`、再交出最后一帧的那种次序；数在 `complete()` 之前）。
- `a_chunked_answer_counts_every_frame`：`chunked_origin` 分几块答，合起来 N 字节：`.1 == N`，N 等于客户端读到的正文长度。
- `rurges_own_answers_count_their_length`：一个请求一种，`.1` 都等于客户端读到的正文的长度（不在用例里写死数字）：拨号失败的 502 页、被拒绝的 403 页（`Refused`，`Pages { reject: true, .. }`）、`REJECT-TINYGIF` 的 GIF、URL Rewrite 的 `reject` 给的 403 页、HTTP/2 上 `CONNECT` 得到的 501（`rig_h2`）；`REJECT-DROP` 与 URL Rewrite 的 `302` 是 0。三处 `bad_request`：任务 0 找到能触发它的现有用例就加一句同样的断言，找不到就照实记进修正记录、不新写夹具。
- `map_local_answers_count_their_length`：Map Local 的 `text` 数据与一个文件（用例的临时目录里的小文件）：`.1` 等于数据与文件的长度。
- `a_head_answer_counts_no_body_on_http1`：HTTP/1.1 上对 `HEAD` 的、rurge 自己的 403 页：`.1 == 0`，客户端也没有读到正文（P5 ③）。
- `an_upgrade_counts_no_body`：`upgrading_origin` 的 `101` 之后两边各转几个字节：`body_at_finish(n) == (0, 0)`，升级之后的字节在 `bytes()` 里。
- `a_client_that_leaves_mid_body_counts_what_was_handed_over`：`streaming_origin` 先答一块、客户端读到它之后走掉：`.1` 等于交出的那一块的长度，不多于源站写出的。
- `an_h2_client_counts_its_bodies_too`：`rig_h2` 上一个带 100 字节正文的请求、源站答 7 字节：`body_at_finish(n) == (100, 7)`。

**实现**：照 P5。

**判据**

- 上面的用例都在、都绿；带等待的单独循环跑过二十次，次数与结果写进报告。
- P5 ④ 列的每一处，`own_body` 都在它结束记录的那一句**之前**（报告里逐处写行号）；`Tail`、`LocalFile` 数的那一句都在 `complete()` 之前。
- M4a 的抓包用例照过（P15）：M4a 包住应答的那一层一行不改。
- 门禁绿。

---

## Task 3：九种直通的原因

**改哪里**：

- `crates/rurge-http/src/mitm/decrypt.rs`：`Decision` 改成五种、`decides` 先看名单（P6 ①）；测试模块的用例照 P15 改。
- `crates/rurge-engine/src/serve.rs`：九句备注的常量；判两张名单的函数（P6 ②）；`decrypting` 改成交回原因（P6 ③）；`serve_sniffed` 把原因交给拨号（P6 ④）。
- `crates/rurge-engine/src/engine.rs`：`impl Dialer for Engine` 的 `dial` 的函数体搬进 `dial_noted`，在建好句柄、认过 fake 地址之后写原因（P6 ⑤）。
- `crates/rurge-engine/tests/sniff.rs`、`crates/rurge-engine/tests/mitm.rs`：新用例。

**接口**

- 消费：现有的 `SessionHandle::add_note`。
- 产出：

```rust
// rurge-http::mitm
pub enum Decision { Decrypt, NotListed, SourceBlocked, NoCertificate, AlpnMismatch }

// rurge-engine（crate 内）
pub(crate) enum PassedBy {
    /// 没有嗅探的会话：拨号时照 `sniffable_session` 的次序判两张名单
    Lists,
    /// 嗅探过的会话：原因（或没有）已经定了
    Sniff(Option<&'static str>),
}
impl Engine {
    pub(crate) async fn dial_noted(&self, session: SessionInfo, passed: PassedBy) -> Result<Dialed, DialError>;
}
pub(crate) fn raw_listed(general: &General, host: &HostName, port: u16) -> Option<&'static str>;   // serve.rs
```

**用例（先红）**

- `decrypt.rs`：
  - `decides_in_the_design_order`（P15，预计要改）：改名 `decides_lists_first_then_source_name_and_alpn`；来源不允许、名单命中 → `SourceBlocked`；来源不允许、名单没有命中 → `NotListed`；`-bad.test` → `NotListed`；没有 SNI 而端口不是 443 → `NotListed`；64 个字母的标签 → `NoCertificate`；其余照旧。
  - `with_h2_an_h2_only_client_is_decrypted` 与别的断言 `Decision::Pass` 的用例：照任务 0 读到的、每一处换成它此时成立的那一种，提交说明里逐处写。
- `tests/sniff.rs`（每条断言记录的 `notes` **恰好**含那一句，不含别的 `Passed through:` / `Not decrypted:`）：
  - `always_raw_tcp_hosts_and_keywords_are_noted`：`always-raw-tcp-hosts = raw.test`、`always-raw-tcp-keywords = rawkey`；到 `raw.test:443` 的会话 → `Passed through: always-raw-tcp-hosts`；到 `a-rawkey.test:443` → `…keywords`；两张都命中的 `rawkey.raw.test`（若 `raw.test` 的写法不覆盖子域就照任务 0 读到的换一个两张都命中的名字）→ `…hosts`（次序在前）。经 HTTP 监听器的 `CONNECT` 与 SOCKS5 各一次。
  - `a_listed_raw_host_on_a_port_that_is_never_sniffed_is_noted_still`：到 `raw.test:7` 的会话 → `…hosts`；到 `other.test:7` 的 → 没有原因（最先成立的是"端口不可嗅探"）。
  - `a_listed_raw_host_that_the_rules_reject_keeps_its_note`：`DOMAIN,raw.test,REJECT`：记录是被拒绝的，`notes` 里有 `…hosts`（P6 ⑤，登记）。
  - `neither_http_nor_tls_is_noted`：到 443 的会话先发 `SSH-2.0-test\r\n` → `Passed through: neither HTTP nor TLS`；同样的会话、`force-http-engine-hosts` 命中它 → 没有这一句（留在引擎里）。
  - `a_sniff_that_waits_out_its_time_is_noted`：照现有的 `a_server_first_protocol_passes_after_the_wait`，客户端什么也不发 → `Passed through: sniffing timed out`。
  - `a_client_that_closes_during_the_wait_has_no_note`：照现有的 `a_client_that_closes_during_the_wait_is_still_dialed` → 没有原因（P6 ④）。
  - `a_paused_engine_writes_no_reason`：在现有的 `a_paused_engine_sniffs_nothing` 的做法上，`always-raw-tcp-hosts` 也命中这个名字：记录没有任何 `Passed through:`（暂停排在名单之前）。
  - `pre_matching_writes_no_reason_unless_a_list_came_first`：在现有的 `a_session_that_pre_matching_refuses_is_not_sniffed` 的做法上两条会话：名单没有命中的那一条没有任何 `Passed through:`；`always-raw-tcp-hosts` 也命中的那一条有 `Passed through: always-raw-tcp-hosts`、记录是被拒绝的（名单排在预匹配之前，M4-D31，P6 ⑤）。
  - `an_unlisted_tls_session_has_no_note`：没有 `[MITM]`，到 443 的 TLS 会话 → 没有原因。
- `tests/mitm.rs`（同样"恰好那一句"）：
  - `a_listed_session_with_mitm_off_is_noted`：`[MITM]` 有 CA、名单有 `listed.test`，开关关着：到 `listed.test:443` 的 TLS → `Not decrypted: MITM is off`；到 `other.test:443` → 没有原因。
  - `a_listed_session_without_a_usable_ca_is_noted`：名单有它、开关开着、没有可用的 CA（不写 `ca-p12`）→ `Not decrypted: no usable CA`。
  - `a_listed_session_from_a_source_not_allowed_is_noted`：`client-source-address = 10.0.0.0/8`（回环的客户端不在里面）：到 `listed.test` → `Not decrypted: the client is not in client-source-address`；到 `other.test`（名单没有命中）→ 没有原因。
  - `a_listed_session_whose_alpn_rurge_does_not_decrypt_is_noted`：客户端只给 `spdy/3`（或 `h2` 而 `[MITM] h2` 不开）→ `Not decrypted: the client offers no protocol rurge decrypts`。
  - `a_listed_name_no_certificate_can_be_made_for_is_noted`：名单写 `*.test`，用例自己写一个 ClientHello（照 `crates/rurge-http/src/tls.rs` 的 `hello_records` 的样子），SNI 是 64 个 `a` 的标签加 `.test` → `Not decrypted: no certificate can be made for this name`。
  - `a_decrypted_session_has_no_reason`：照现有的一条解密用例，记录没有任何 `Not decrypted:`（不要求先红）。

**实现**：照 P6。九句的原文见「表后的原文」P6。

**判据**

- 上面的用例都在、都绿；带等待的单独循环跑过二十次。
- `rurge-http` 里 `Decision::Pass` 不再有（`git grep -n 'Decision::Pass'` 空）。
- `decrypting` 的两行 DEBUG 文字不变；没有新的日志行。
- 门禁绿。

---

## Task 4：在途的查找与 `trail::current()`

**改哪里**：

- `crates/rurge-net/src/trail.rs`：`Left` 记在途的查找；新的票据 `Lookup`；`dns()` 加上在途的；新的 `current()`；`connected`、`peer` 的文档注释照新口径改（P7 ③）；测试模块里的用例。
- `crates/rurge-dns/src/resolver.rs`：`lookup_inner` 用 `Lookup::begin()` 换掉 `Noted`；删掉 `Noted`；测试模块里一条用例。
- `crates/rurge-engine/src/observe.rs`：`dns_ms` 的文档注释换成「表后的原文」P4 的那一段（P7 ③）。
- `crates/rurge-engine/tests/pipeline.rs`：一条端到端的用例。

**接口**

- 消费：无。
- 产出：

```rust
// rurge-net::trail
/// 一次进行中的查找（P7）：开始时在跟随着的那条 trail 上登记，丢下时把花的时间加上并注销
pub struct Lookup { /* … */ }
impl Lookup {
    pub fn begin() -> Lookup;
}
/// 正在跟随的那条 trail（克隆）；不在任何 `follow` 里是 None
pub fn current() -> Option<Trail>;
```

**用例（先红）**

- `trail.rs`（时间用 `#[tokio::test(start_paused = true)]` 与 `tokio::time::advance`）：
  - `a_lookup_under_way_counts_up_to_now`：在 `follow` 里 `Lookup::begin()` 拿到票据、带出 `follow`；`advance(30 ms)` → `trail.dns() == Some(30 ms)`；再 `advance(20 ms)` → `Some(50 ms)`；丢下票据 → `Some(50 ms)`；再 `advance(100 ms)` → 仍是 `Some(50 ms)`。
  - `lookups_done_and_under_way_add_up`：`follow` 里 `looked_up(5 ms)`，再一张票据在途 10 ms → `Some(15 ms)`；两张票据同时在途 10 ms → 各算各的，`Some(20 ms)`。
  - `a_lookup_begun_outside_a_follow_notes_nothing`：不在 `follow` 里的票据丢下之后，任何 trail 都是 `None`。
  - `a_lookup_ends_on_the_trail_it_began_on`：在 trail A 的 `follow` 里开始、在 trail B 的 `follow` 里丢下 → 时间进 A，不进 B。
  - `current_is_the_trail_followed_now`：`follow` 之外是 `None`；`follow` 里 `current()` 拿到的经 `set_peer` 写的地址出现在那条 trail 上；里层另一条 trail 的 `follow` 里 `current()` 是里层的。
- `resolver.rs`：`a_lookup_under_way_shows_on_the_trail_while_it_waits`（`start_paused`）：`MockDns::set_delay(30 s)`，在另一个任务里 `trail.follow(r.lookup("slow.test", …))`；用例 `sleep(1 s)` 之后 `trail.dns()` 是 `Some(d)`、`1 s <= d < 30 s`；然后 `abort` 那个任务。现有的四条 trail 用例（`lookups_are_noted_on_the_trail_of_whoever_is_dialling` 等）预计照过。
- `pipeline.rs`：`an_active_session_shows_the_time_its_lookup_has_taken_so_far`：`MockDns` 对 `slow.test` 延迟 3 s；`IP-CIDR` 规则（不带 `no-resolve`）让会话先解析；经 HTTP 监听器 `CONNECT slow.test:<回环上的端口>`：`wait_until` 等到 `active()` 里这条会话的 `dns_ms` 至少 300（限时 10 s）；再 `wait_until` 等到它比上一次读到的大；结束之后的记录 `dns_ms >= 3000`。提交前单独循环跑二十次。

**实现**：照 P7。

**判据**

- 上面的用例都在、都绿；`rurge-dns` 里没有 `Noted` 了；`looked_up` 仍是公开的（测试与它的文档用到）。
- 时序的两条单独循环跑过二十次。
- 门禁绿。

---

## Task 5：UDP 的远端

**改哪里**：

- `crates/rurge-net/src/connector.rs`：`Datagram::far_end`、`PacketSocket::far_end` 两个有缺省的方法；`DirectDatagram`、`DirectPacket` 实现它们；测试模块里的用例。
- `crates/rurge-net/src/packet_datagram.rs`：`PacketDatagram` 建时记下载体的远端；测试模块的 `Scripted` 能答一个远端。
- `crates/rurge-policy/src/cell.rs`：`Relayed` 转交 `far_end`。
- `crates/rurge-engine/src/dns_pipeline.rs`：`FinishOnDrop`、`CountingDatagram` 转交；`wrap_internal_udp` 写内部 UDP 会话的远端（P8 ④）。
- `crates/rurge-proto/src/{shadowsocks/udp.rs,socks5.rs,stream_udp.rs,snell/udp.rs,snell/mod.rs,anytls/mod.rs,trojan.rs}`：各 UDP 载体的远端（P8 ②）。
- `crates/rurge-proto/src/testing/socks5.rs`：`FakeSocks5::relays()`（P8 ⑥）。
- `crates/rurge-engine/src/udp.rs`：流在 `resolve` 之后写远端（P8 ③）。
- `crates/rurge-engine/src/observe.rs`：记录读 `Trail` 的对端时不再分 TCP 与 UDP；`remote_addr` 的文档注释换成「表后的原文」P4 的那一段。
- 用例：`crates/rurge-engine/tests/{udp.rs,pipeline.rs,outbounds_shadowsocks.rs,outbounds_snell.rs,udp_tls_family.rs}` 与上面各文件的测试模块。

**接口**

- 消费：任务 4 的 `trail::current()`。
- 产出：

```rust
// rurge-net::connector
pub trait Datagram: Send + Sync {
    /// 这个载体的数据报离开 rurge 时所经的那一跳的远端（P8 ①）；不知道是 None
    fn far_end(&self) -> Option<SocketAddr> { None }
}
pub trait PacketSocket: Send + Sync {
    /// 发往 `to` 的数据报离开 rurge 时所经的那一跳的远端（P8 ①）；不知道是 None
    fn far_end(&self, to: &Target) -> Option<SocketAddr> { let _ = to; None }
}

// rurge-proto
impl StreamUdp { pub(crate) fn new(stream: BoxedStream, framing: Framing, head: Vec<u8>, far_end: Option<SocketAddr>) -> StreamUdp; }
impl SnellUdp { pub(super) async fn open(stream: SnellStream, far_end: Option<SocketAddr>) -> io::Result<SnellUdp>; }

// rurge-proto::testing
impl FakeSocks5 { pub fn relays(&self) -> Vec<SocketAddr>; }

// rurge-engine（crate 内）
fn note_far_end(handle: &SessionHandle, socket: &dyn PacketSocket, to: &Target);
```

**用例（先红）**

- `connector.rs`：`a_direct_datagram_far_end_is_its_peer`（`connect_udp` 到回环上的回声，`far_end() == Some(回声的地址) == peer_addr()`）；`a_direct_packet_far_end_is_the_address_it_sends_to`（`DirectPacket::far_end` 对地址的目标是它，对名字是 `None`）。
- `packet_datagram.rs`：`a_packet_datagram_far_end_is_its_carriers`：`Scripted` 答 `Some(x)` → `far_end() == Some(x)`，`peer_addr()` 照旧是目标；答 `None` → `None`。
- `cell.rs`：`a_relays_carrier_tells_the_far_end_of_the_carrier_under_it`：照 `a_relays_carrier_fails_under_the_relays_name_both_ways` 里 `Failing` 答 `peer_addr` 的做法，测试数据报答一个 `far_end`，`Relayed` 的 `far_end()` 交回同一个。
- `udp.rs`（引擎的测试模块）：`the_far_end_replaces_what_the_carriers_dial_noted`：句柄的 trail 先 `set_peer(192.0.2.7:1080)`；一个答 `None` 的测试载体 → `note_far_end` 之后 `peer()` 是 `None`；答 `Some(x)` 的 → `Some(x)`。
- `observe.rs`：`the_record_of_a_udp_session_has_no_remote_address`（P15，预计要改）改名 `a_udp_record_reads_the_trail_as_a_tcp_record_does`：进行中与结束之后都是 `Some(peer)`。
- `tests/udp.rs`：
  - `a_flow_through_a_socks5_proxy_has_no_remote_address`（P15，预计要改）改名 `a_flow_through_a_socks5_proxy_records_the_relay`：`remote_addr == Some(up.relays()[0])`，不是 `up.addr()`。
  - `a_direct_flow_records_where_its_datagrams_go`：经 DIRECT 发往回声的流 → `Some(回声的地址)`；按名字 `target.test`（`[Host]` 钉到 `127.0.0.1`）发的流 → `Some(127.0.0.1:回声的端口)`。
  - `the_lookup_a_rule_makes_counts_for_a_flow_too` 的两个被拒绝的流仍是 `None`（P15，预计照过）。
- `tests/pipeline.rs`：
  - `a_carrier_over_a_socks5_association_has_no_remote_address`（P15，预计要改）改名 `a_carrier_over_a_socks5_association_records_the_relay`：策略是 `Socks` 的载体会话的 `remote_addr` 是 `proxy.relays()` 里的一个，不是 `proxy.addr()`。
  - `a_direct_doq_carrier_records_its_server`：DoQ 直连时内部 UDP 会话的 `remote_addr == Some(doq.addr())`。
- `tests/outbounds_shadowsocks.rs`：一个 UDP 流的记录 `remote_addr == Some(fake.udp_addr())`。
- `tests/outbounds_snell.rs`：`udp_goes_through_snell` 之后的记录 `remote_addr == Some(fake.addr())`（加一句断言，提交说明里说）。
- `tests/udp_tls_family.rs`：`trojan` 与 `anytls` 各一个 UDP 流的记录是服务端的地址（加断言或新用例，照那个文件的写法）。
- `external` 的 UDP：任务 0 找到现有的 `external` UDP 用例就加一句断言（远端是 `127.0.0.1:<中继的端口>`）；没有就照实记进修正记录，不新写夹具。

**实现**：照 P8。

**判据**

- 上面的用例都在、都绿；P15 点名的三条照改，提交说明里逐条说为什么。
- 生产代码里 `impl … Datagram for` 的五处（P8 ①）都转交或答了 `far_end`；`PacketSocket` 的各实现照 P8 ② 的表；表里写"缺省"的不动。
- 门禁绿。

---

## Task 6：复用、共用的 TCP 载体

**改哪里**：

- `crates/rurge-proto/src/snell/{mod.rs,pool.rs,tunnel.rs}`：`fresh` 交回远端；池里的连接带着远端；借出时写进会话的 `Trail`；失效重拨在会话的 `Trail` 下（P9 ②）。
- `crates/rurge-proto/src/anytls/{mod.rs,session.rs}`：`Session` 带着远端（P9 ③）。
- `crates/rurge-proto/src/h2pool/mod.rs`：`Conn` 带着远端，每次借出写进会话（P9 ④）。
- `crates/rurge-proto-ssh/src/outbound.rs`：`Session` 带着远端（P9 ⑤）。
- 用例：`snell/mod.rs` 的测试模块；`crates/rurge-engine/tests/{outbounds_snell.rs,outbounds_tls_family.rs,outbounds_http2.rs,outbounds_ssh.rs}`。

**接口**

- 消费：任务 4 的 `trail::current()`；任务 5 的 `SnellUdp::open(stream, far_end)`。
- 产出（都在 crate 内）：

```rust
// snell
impl Dialer { pub(crate) async fn fresh(&self, opts: &ConnectOpts) -> Result<(SnellStream, Option<SocketAddr>), OutboundError>; }
impl Pool {
    pub(crate) fn put(&self, stream: SnellStream, peer: Option<SocketAddr>);
    pub(crate) fn take(&self) -> Option<(SnellStream, Option<SocketAddr>)>;
}
impl SnellTunnel {
    pub(crate) fn new(stream: SnellStream, peer: Option<SocketAddr>, head: Vec<u8>, pool: Option<Weak<Pool>>) -> SnellTunnel;
    pub(crate) fn reused(stream: SnellStream, peer: Option<SocketAddr>, head: Vec<u8>, pool: Weak<Pool>, dialer: Arc<Dialer>, opts: ConnectOpts) -> SnellTunnel;
}
// anytls
impl Session {
    pub(crate) fn with_peer(self, peer: Option<SocketAddr>) -> Session;
    pub(crate) fn peer(&self) -> Option<SocketAddr>;
}
```

**用例（先红）**

- `snell/mod.rs`：`a_redial_notes_the_new_connection_on_the_sessions_trail`：照这个模块测失效重拨的现有用例（`tunnels_per_connection: Some(1)`）。第二个隧道在 `trail.follow(…)` 里打开；打开之后、在任何 `follow` 之外 `trail.set_peer(192.0.2.9:9)` 当哨兵；再收发一次让它重拨；回声回来之后 `trail.peer() == Some(假服务端的地址)`（哨兵被重拨的连接换掉了）。
- `tests/outbounds_snell.rs`：
  - `a_pooled_snell_connection_gives_its_record_the_servers_address`：`reuse=true`，先后两条会话（照 `reuse_carries_sessions_one_after_another_on_one_connection`，等连接回到池里）：`fake.connections() == 1`，两条记录的 `remote_addr` 都是 `Some(fake.addr())`。
  - `a_pooled_snell_connection_through_a_socks5_proxy_records_the_first_hop`：`underlying-proxy` 是一个 `FakeSocks5`、`reuse=true`：两条记录都是 `Some(socks5.addr())`。
  - `a_redialled_snell_session_records_the_new_connection`：`reuse=true`、`tunnels_per_connection: Some(1)`，服务端在策略行里写成 `snell.test`、`[Host]` 把它钉到 `127.0.0.1`：第二条记录 `remote_addr == Some(fake.addr())`、`dns_ms` 是 `Some`（重拨的那次查找，P9 ②），`fake.connections() == 2`。
- `tests/outbounds_tls_family.rs`：`a_pooled_anytls_session_gives_its_record_the_servers_address`：先后两条会话走同一个 anytls 会话（服务端只见过一条连接）：两条记录都是服务端的地址。
- `tests/outbounds_http2.rs`：`every_stream_on_a_shared_h2_connection_records_its_address`：同时两条会话共用一条 `h2-connect` 连接（服务端只见过一条连接）：两条记录都是服务端的地址。
- `tests/outbounds_ssh.rs`：`every_channel_on_an_ssh_session_records_its_address`：两条会话走同一个 SSH 会话：两条记录都是服务端的地址。

**实现**：照 P9。

**判据**

- 上面的用例都在、都绿；带等待的单独循环跑过二十次。
- snell、anytls 现有的池与重拨的用例照过（P15）；它们的单元用例里建 `SnellTunnel`、`Pool::put` 的地方补 `None`。
- 门禁绿。

---

## Task 7：QUIC 一族与 WireGuard 的远端

**改哪里**：

- `crates/rurge-net/src/quic/{mod.rs,hop.rs}`：`FarEnd`、`Path::far_end`；跳端口的套接字交出此刻的端口（P10 ①）。
- `crates/rurge-proto-quic/src/session.rs`：`Conn` 带着 `FarEnd`；`Lease::far_end`；`take` 交回之前写进会话（P10 ②）。
- `crates/rurge-proto-quic/src/{hysteria2/udp.rs,tuic/udp.rs}`：UDP 载体的远端（P10 ③）。
- `crates/rurge-proto-wireguard/src/{device.rs,outbound.rs,udp.rs}`：对端的端点（P11）。
- 用例：上面各文件的测试模块；`crates/rurge-engine/tests/{outbounds_hysteria2.rs,outbounds_tuic.rs,outbounds_masque.rs,outbounds_wireguard.rs}`。

**接口**

- 消费：任务 5 的 `Datagram::far_end`、`PacketSocket::far_end`。
- 产出：

```rust
// rurge-net::quic
#[derive(Clone, Debug)]
pub struct FarEnd(/* … */);
impl FarEnd {
    /// 此刻的远端：跳端口的是此刻在用的那个端口（P10 ①）
    pub fn now(&self) -> Option<SocketAddr>;
}
impl Path { pub fn far_end(&self) -> FarEnd; }

// rurge-net::quic::hop（crate 内）
pub(crate) struct Current(/* Arc<Shared> */);
impl Hopping { pub(crate) fn current(&self) -> Current; }
impl Current { pub(crate) fn now(&self) -> SocketAddr; }

// rurge-proto-quic
impl<S> Lease<S> { pub fn far_end(&self) -> Option<SocketAddr>; }

// rurge-proto-wireguard（crate 内）
impl Device { pub(crate) fn far_end_for(&self, ip: IpAddr) -> Option<SocketAddr>; }
```

**用例（先红）**

- `quic/mod.rs`：`a_direct_path_far_end_is_its_remote`；`a_carried_path_far_end_is_its_carriers_not_the_stand_in`：载体的数据报答 `far_end() == Some(x)`（测试模块里已有的测试数据报加一个能答远端的）→ `Path::far_end().now() == Some(x)`，而 `path.remote == STAND_IN`。
- `quic/hop.rs`：`the_current_port_follows_the_hops`：照 `a_hop_moves_to_another_port_of_the_set` 的做法，`current().now()` 是 `server` 与此刻状态里的端口；跳一次之后跟着变。
- `tests/outbounds_hysteria2.rs`：
  - `every_stream_on_a_quic_connection_records_the_servers_address`：先后两条 TCP 会话、`fake.connections() == 1`：两条记录都是 `Some(fake.addr())`。
  - `a_hysteria2_udp_flow_records_the_server`：UDP 流的记录是 `Some(fake.addr())`。
  - `hysteria2_through_a_socks5_relay_records_the_relay`：`underlying-proxy` 是 `udp-relay=true` 的 `FakeSocks5`：记录是 `socks5.relays()` 里的那一个，不是 `192.0.2.1:443`，也不是 `socks5.addr()`。
- `tests/outbounds_tuic.rs`：TCP 两条会话、UDP 一个流，记录都是服务端的地址。
- `tests/outbounds_masque.rs`：TCP 会话的记录是服务端的地址；UDP 流的记录照旧 `None`（钉住登记的那一种，不要求先红）。
- `tests/outbounds_wireguard.rs`：`a_session_through_wireguard_records_its_peers_endpoint`（TCP，`Some(peer.addr())`）；`a_udp_flow_through_wireguard_records_its_peers_endpoint`（`Some(peer.addr())`）；夹具能起两个对端、各管一段 `allowed-ips` 的，加一条：两个目标各是自己那个对端的端点。

**实现**：照 P10、P11。

**判据**

- 上面的用例都在、都绿；带等待的单独循环跑过二十次。
- `rurge-net` 的 `Path` 之外的 QUIC 用户（`rurge-dns` 的 `upstream/session.rs`）一行不改。
- WireGuard 给 `carriers` 赋值的每一处都同时更新了远端（报告里逐处写行号）。
- 门禁绿。

---

## Task 8：经 API 的端到端与量一条记录的大小

**改哪里**：`crates/rurge-api/tests/api.rs`（一条用例）；`crates/rurge-engine/src/observe/memory.rs`（新）与 `crates/rurge-engine/src/observe.rs` 里的 `#[cfg(test)] mod memory;`（放在文件末尾的 `#[cfg(test)] mod tests {` 之前）。

**接口**：消费任务 1 ～ 7 的全部；不产出新接口。

**用例**

- `api.rs`：`the_request_records_carry_the_three_new_keys`（照这个文件起真引擎、问 `/v1/requests/recent` 的现有用例）：经 HTTP 监听器向回环上的 `TestServer` `POST` 9 字节的正文 → 记录的 `requestBodyBytes == 9`、`responseBodyBytes` 等于源站答的正文长度、`receiveMs` 是数；经 HTTP 监听器 `CONNECT` 到回环上的回声（不可嗅探的端口）收发一次 → `requestBodyBytes`、`responseBodyBytes` 是 `null`，`receiveMs` 是数，`remoteAddr` 是回声的地址。
- `observe/memory.rs`：照 `crates/rurge-http/src/capture/memory.rs` 的做法（P12）：
  - `a_full_request_log_of_http_requests`（`#[ignore]`）：`RequestLog::new(100_000)` 装满 100 000 条 HTTP 请求样子的记录、再多 10 000 条（挤掉最早的），打印记录数与峰值工作集；
  - `an_empty_request_log_is_the_baseline`（`#[ignore]`）：同一个测试二进制、空的记录表，打印峰值工作集；
  - `the_peak_is_read_from_what_the_system_prints`：照抄 M4a 的那一条（不忽略）。
  - 各自单独运行（`cargo test -j 8 --workspace --lib -- observe::memory::<名字> --ignored --exact --nocapture`），调试构建；两次的差除以 100 000 是一条记录的字节数，写进报告（一台机器上的一次运行，不是 CI）。

**判据**

- `api.rs` 的那一条绿；量的两条单独跑过、数字写进报告；缺省的 `cargo test` 不跑它们。
- `--request-log-size` 的缺省不改（M4-D25）。
- 门禁绿。

---

## Task 9a：手工清单、API 与测试的文档

**改哪里**：

- `docs/api/phase4.md`：新的一节「M4b　请求记录的完整」，放在「M4a　抓包」一节之后，内容照 P13 ①。
- `docs/acceptance/phase4-manual.md`：M4 一节加 M4b 的条目（P13 ②）；开头的项数随之改。
- `docs/testing.md`：量记录的做法与 M4a 量抓包的相同，在 M4a 那一条后补一句"请求记录的大小也这样量（`crates/rurge-engine/src/observe/memory.rs`）"。

**判据**：三份文档照 P13 改完；`docs/api/phase4.md` 里每一个数都出自任务 8 的报告或用例；中文、直引号、LF（`git ls-files --eol` 看过）。

---

## Task 9b：M4b 的收尾文档

**改哪里**：

- `docs/status.md`：阶段 4 下新的一节「M4b（请求记录的完整）」，照 `CLAUDE.md`「工作流约定」的「进度」那一条写：做了什么、没做的与不要当成已经有的、以后的会话必须知道的事（P13 ③）、去向写着项目所有者的事（设计第 17 节的两行 M4b 的、推送、手工验收）。
- `docs/surge-compatibility-matrix.md`：L884 一行（记录的三个新字段、九种备注、`remoteAddr` 的新口径与照旧 `null` 的三种）；开头"最近一次"的括号。
- `README.md` 与 `README_en.md`：特性表与状态，两份一致。
- `CLAUDE.md`：只改「现在在哪里」：M4b 做完、M4 至此做完；M1a ～ M4b 的提交都在本地、CI 都没有见过；下一步是项目所有者推送（M1 ～ M4 一起），推送之后读回那次 CI，再往后是 M5。
- 设计第 18 节："执行 M4b 时"之下，执行期改了设计正文意思的地方一条一段（没有就写"没有"），每条指到修正记录的行。

**判据**：各文档照上面改完；行尾照旧（`CLAUDE.md`、两份 README 在工作区是 CRLF，别的是 LF）；带本机账户名的文件数照旧 26；`git diff --check` 干净。

---

## 计划期决定

写计划时对着 `33efe98` 核对过下面引的名字与行号。行号会随前面任务的改动漂移，以名字为准；名字不同就照实改、记进修正记录，不改语义。标了"订正设计"的，已随本计划写进设计第 18 节。

| # | 事项 | 决定 |
| - | ---- | ---- |
| P1 | 任务的拆法（订正设计） | 设计第 2 节的 M4b 大纲拆成任务 0 ～ 8、9a、9b：<br>• 0 核对；1 句柄的两个计数与 `first_byte_at`、记录与 API 的三个键；2 引擎里数两个正文；3 九种原因；4 在途的查找与 `trail::current()`；5 UDP 的远端（两个 `far_end`、UDP 流、内部的 UDP 会话，记录读 `Trail` 时不再分 TCP 与 UDP）；6 复用、共用的 TCP 载体；7 QUIC 一族与 WireGuard；8 经 API 的端到端与量一条记录的大小；9a 手工清单、`docs/api/phase4.md`、`docs/testing.md`；9b 其余的收尾文档。<br>• 与大纲的不同：三个新字段分成两个任务（1 只碰句柄、记录与 API，2 在引擎里数）；UDP 排在 TCP 载体之前——任务 7 的 QUIC 与 WireGuard 要用任务 5 的 `Datagram::far_end`，`record_of` 不再分 TCP 与 UDP 也随 UDP 一起改；文档收尾分成 9a、9b（同 M4a 的 P1）。<br>• 任务 4 在 5 ～ 7 之前：它们用 `trail::current()` |
| P2 | 依赖 | 不改任何 `Cargo.toml`：用到 `rurge_net::trail` 的 crate 都已正式依赖 `rurge-net`（全局约束）；工作区的 `tokio` 开着 `time`。不加包，锁文件一个字不变（520 个包）；`unsafe_functions.rs` 的 `LISTED`（23 项）、`sockets.rs` 的 `LISTED`（26 行）、带本机账户名的文件数（26）都照旧 |
| P3 | `SessionHandle`（任务 1） | 两个字段 `request_body: AtomicU64`、`response_body: AtomicU64` 紧跟 `up`、`down`，在 `new_with_slot` 里初始化为 0（另两个构造函数调它）。`add_request_body(n)`、`add_response_body(n)` 是 `fetch_add(n, Relaxed)`，`body_bytes()` 是两个 `load(Relaxed)`（同 `add_up`、`bytes`）。`first_byte_at()` 是 `self.first_byte.get().copied()`：`first_byte` 存的就是从开始算起的那一刻（`mark_first_byte` 里 `started.elapsed()`）。文档注释写：两个计数是 HTTP 引擎数的正文字节，与 `up` / `down`（上游一侧的全部字节，含头）无关；记录在 `finish` 那一刻拍下，之后加的进不了记录（`phase 4 M4 design 10.1`）|
| P4 | 记录与 API（任务 1） | **`record_of`**：`let elapsed = h.elapsed();` 只取一次，`elapsed_ms` 由它来；`receive_ms: outcome.and(h.first_byte_at()).map(\|f\| elapsed.saturating_sub(f).as_millis() as u64)`——进行中的会话（`outcome` 是 `None`）与没有收到过上游字节的是 `None`；`let (request_body, response_body) = h.body_bytes();`，`request_body_bytes: h.method().map(\|_\| request_body)`，`response_body_bytes` 同样——不是 HTTP 请求的会话（`method` 是 `None`）两个都是 `None`。<br>**位置**：`RequestRecord` 与 `RequestJson` 里 `request_body_bytes`、`response_body_bytes` 紧跟 `status_code`，`receive_ms` 紧跟 `dns_ms`（设计第 10.1 节；`RequestJson` 的字段次序就是 JSON 里键的次序）；`From<&RequestRecord>` 照抄三个值。<br>**文档注释**：三个新字段的照抄原文 P4。`remote_addr` 的值与文档注释在任务 5 改（P8 ⑤），`dns_ms` 的文档注释在任务 4 改（P7 ③）：任务 1 只加三个字段 |
| P5 | 引擎里数两个正文（任务 2） | ① **请求正文**：`Idle` 多字段 `handle: Arc<SessionHandle>` 与 `new` 的同名参数（排在 `body` 之后），唯一的调用处给 `handle.clone()`；`poll_frame` 里数据帧那一支、`this.body.count_up(..)` 旁边加 `this.handle.add_request_body(data.len() as u64)`。两种版本都数：`Meter` 只在 HTTP/2 上把请求自己的字节记到句柄（M2b 计划 P11），这里另数，与 `up` 无关。<br>② **应答正文**：`Tail` 的 `poll_frame` 的 `PolledUpstream::Frame` 那一支，在 `if ended { this.complete(); }` **之前**：数据帧 → `this.guard.handle().add_response_body(len)`，不看 `counted`（读在前面的、改写过的正文也是交给客户端的）。`LocalFile` 的 `LocalRead::Data` 那一支，在 `if this.left == 0 { this.complete(); }` 之前同样一句。<br>③ **HEAD**：hyper 1.11.1 的 HTTP/1.1 服务端不写 HEAD 应答的正文，HTTP/2 服务端照样发出去（「技术栈」）。上游对 HEAD 的应答本来没有正文，`Tail` 数到 0；Map Local 对 HEAD 不读数据（`data`、`file` 都是 `None`），也是 0；rurge 自己的页由 ④ 的 `own_body` 照版本定：HTTP/1.1 上 0，HTTP/2 上是页的长度（照实数，不改 hyper 的行为；没有用例，登记）。<br>④ **rurge 自己的答复**：私有的 `fn own_body(handle: &SessionHandle, response: &Response<ResponseBody>, h2: bool)`：`!h2 && handle.method().as_deref() == Some("HEAD")` 时什么也不做，否则 `handle.add_response_body(response.body().size_hint().exact().unwrap_or(0))`（这些正文都是 `Full` 或空的，长度是确切的）。每一处 rurge 自己作答、在交出应答之前就结束记录的，**先建好应答，调 `own_body(handle, &response, self.shared.conn.h2)`，再 `finish`**：<br>• `answer` 的 `TinyGif`（今天先 `finish` 再建 GIF，改成先建）与 `Reject` / `NoDrop` 的页；<br>• `failed`、`timed_out` 的页（`map_local_failed` 经 `failed`）；<br>• URL Rewrite 的 `Reject` 的页；<br>• HTTP/2 上 `CONNECT` 的 501（`not_implemented`）；<br>• 三处 `bad_request`：第 5 步 `absolute_form` / `origin_form` 失败、`fit` 失败、重发时 `fit` 失败（今天都是先 `finish` 再建，改成先建）；<br>• `map_local` 的 `(None, data)` 那一支：先建 `responses::full` / `responses::empty()`，再 `own_body`、`finish`、`drop(guard)`。<br>正文空的（URL Rewrite 的 `302` / `307`、`responses::empty()`）调不调都是 0，可以不调。`REJECT-DROP`、没有页的拒绝与失败（`Err(Close)`）什么也不交，不调。监听器的两个 400（建句柄之前）没有记录，不调。<br>⑤ **测试的口子**：测试模块的 `Inner` 多 `bodies: Arc<Mutex<HashMap<u64, (u64, u64)>>>`；`Fake::open` 建好句柄之后 `handle.on_finish(..)`，钩子握着 `bodies` 的克隆，把 `h.body_bytes()` 记在 `h.id()` 下（钩子在 `finish` 里跑，与记录拍下是同一刻）；`async fn body_at_finish(&self, n: usize) -> (u64, u64)` 先 `finished(n)`，再取第 `n` 个句柄的 id 下记的数，没有就 panic（"the handle finished without its hook"） |
| P6 | 九种原因（任务 3；订正设计两处） | ① **`Decision`**：`Decrypt`、`NotListed`、`SourceBlocked`、`NoCertificate`、`AlpnMismatch`（派生照旧）。`decides` 的次序改成名单 → 来源 → 名字 → ALPN：`listed != Some(true)` → `NotListed`；`self.blocked(source)` → `SourceBlocked`；rustls 不收的名字 → `NoCertificate`；客户端给了 ALPN 而没有一个是 rurge 解密的 → `AlpnMismatch`；否则 `Decrypt`。只有标签变了：四个条件都成立才解密，与今天相同。文档注释照新的次序改，并写为什么名单先看（来源不允许而名单没有命中的不该说"来源不允许"，设计第 10.2 节）。<br>② **`pub(crate) fn raw_listed(general: &General, host: &HostName, port: u16) -> Option<&'static str>`**（`serve.rs`）：`general.always_raw_tcp_hosts.matches(host, port) == Some(true)` → `RAW_HOSTS`；`host` 是 `Domain(name)` 而有一个关键字非空、`name.contains(&k.to_ascii_lowercase())` → `RAW_KEYWORDS`；否则 `None`。`sniffable_session` 的那两段换成 `if raw_listed(general, host, port).is_some() { return false; }`，两段的注释留在调用处。九句是 `serve.rs` 里的私有常量（名字与原文见「表后的原文」P6）。<br>③ **`decrypting`** 交回 `Result<(Arc<Mitm>, HostName), Option<&'static str>>`（`Err(None)` 是不写原因的直通）。次序：`named_target`（`None` → `Err(None)`）；SNI 与 `name` 照旧；`let runtime = self.runtime();`，用 `runtime.config.mitm.effective` 判名单（有 SNI 用 `matches_sni(sni, port)`，没有用 `matches(&target, port)`，同今天没有 CA 的那一支），不命中 → `Err(None)`；开关关着 → `Err(Some(MITM_OFF))`；`runtime.mitm` 是 `None` → 今天那行 DEBUG、`Err(Some(NO_CA))`；`mitm.decides(..)`：`Decrypt` → `Ok`，`NotListed` → `Err(None)`，`SourceBlocked` → `SOURCE`，`NoCertificate` → `NO_CERTIFICATE`，`AlpnMismatch` → 今天那行 DEBUG、`NO_PROTOCOL`。名单移到开关之前只多一次读 `runtime()`，解不解密与今天相同（V14 核定的次序：名单 → 开关 → CA → 来源 → 名字 → ALPN）。函数的文档注释照新的次序改。<br>④ **`serve_sniffed`**：`let mut passed = None;`；`Tls` → `match self.decrypting(..) { Ok(..) => { serve_mitm; return } Err(note) => passed = note }`；`Http` 照旧；`Other` → `into_engine = forced`，不 `forced` 时 `passed = Some(NEITHER)`；`Closed`（订正设计：客户端在嗅探时关掉的没有表示任何协议，不写）→ `into_engine = forced`，不写；`Timeout` → `passed = Some(TIMED_OUT)`（`forced` 也照旧拨号，照写）；`Failed` 照旧 `return`。之后 `self.dial_noted(session, PassedBy::Sniff(passed))`。<br>⑤ **`dial_noted`**（`engine.rs` 的 `impl Engine`）：`Dialer::dial` 的函数体整个搬进来，`Dialer::dial` 改成 `Box::pin(self.dial_noted(session, PassedBy::Lists))`。原因在 `match turned { .. }` 之后（`Turned::Unknown` 在那里失败，不写）、`quic_adaptive.on_tcp` 之前写：`Sniff(note)` 就是 `note`；`Lists` 只判这样的会话——`Transport::Tcp`、监听器是 `Http` / `Socks5` / `Tun`、引擎没有暂停（同 `sniffable_session` 的前两步），这时 `raw_listed(&rt.config.general, &s.dst_host, s.dst_port)`（`rt` 是同一次 `snapshot` 的；`s` 是 `turn_back` 之后的会话，与 `sniffable_session` 看的是同一个名字，TUN 的会话本来就带着名字）。有就 `handle.add_note(note)`。<br>登记（订正设计第 10.2 节"被预匹配拒绝的……也不写"）：`sniffable_session` 先看名单、最后才问预匹配，所以命中名单而又被预匹配的规则拒绝的会话照 M4-D31 写那一句；之后被任何规则拒绝的都照样带着。另外两件：监听器问过 `sniffable` 之后、拨号之前重载的，按拨号时的这一代判；不可嗅探的端口上命中名单的照写（名单在端口之前）。<br>不加日志行 |
| P7 | 在途的查找与 `current()`（任务 4） | ① **`Left`** 多 `under_way: Vec<(u64, tokio::time::Instant)>` 与 `next: u64`（票据的号）。时钟用 `tokio::time::Instant`，同 `rurge-dns` 的 `resolver.rs`：暂停的时钟下用例才推得动。**`pub struct Lookup { trail: Option<Trail>, id: u64, started: tokio::time::Instant }`**（不派生 `Clone`，不实现 `Debug`）：`begin()` 取 `Instant::now()` 与 `current()`，有 trail 时锁住、取号（`next += 1`）、推进 `under_way`。`Drop`：有 trail 时锁住、从 `under_way` 去掉这个号、`dns = Some(dns.unwrap_or_default().saturating_add(started.elapsed()))`（同 `looked_up`）。没有 trail 的票据什么也不做。<br>② **`dns()`**：锁住；`dns` 是 `None` 而 `under_way` 是空的 → `None`；否则 `Some(dns.unwrap_or_default() + Σ now.saturating_duration_since(start))`，`now` 只取一次。`looked_up` 不变，仍是 `pub`。**`pub fn current() -> Option<Trail>`**：`CURRENT.try_with(Trail::clone).ok()`。<br>③ **文档注释**：模块注释的第一句与 `peer`、`connected`、`dns`、`Lookup`、`current` 的照抄原文 P7；`observe.rs` 的 `dns_ms` 一段照抄原文 P4。<br>④ **`rurge-dns`**：`lookup_inner` 里的 `let _noted = Noted(started);` 换成 `let _lookup = rurge_net::trail::Lookup::begin();`（同一处：地址字面量在它之前早退，不算查找）；删掉 `struct Noted`、它的 `Drop` 与文档注释。同一个名字在途时别的调用方怎样等照旧：每次 `lookup_inner` 各一张票据，记在各自的 trail 上 |
| P8 | UDP 的远端（任务 5；④ 订正设计） | ① **`Datagram::far_end(&self) -> Option<SocketAddr>`**（缺省 `None`）：这个载体的数据报离开 rurge 时所经的那一跳的远端——自己发出去的是它发往的地址，架在别的套接字上的是那个套接字的远端。与 `peer_addr` 不是一回事：`PacketDatagram` 的 `peer_addr` 是它所代表的目标（WireGuard 的 `land` 拿它比端点，不动）。文档注释写这两句。生产代码里的五个实现：`DirectDatagram` 交回 `self.peer_addr()`（连上的地址）；`PacketDatagram` 多字段 `far_end: Option<SocketAddr>`，建它时（`packet_datagram(socket, to)`）取 `socket.far_end(&to)`；`Relayed`（`rurge-policy` 的 `cell.rs`）、`FinishOnDrop`、`CountingDatagram`（`rurge-engine` 的 `dns_pipeline.rs`）转交里面的。测试里的实现用缺省。<br>② **`PacketSocket::far_end(&self, to: &Target) -> Option<SocketAddr>`**（缺省 `None`）：<br>• `DirectPacket`：`to` 是地址时就是它，是名字时 `None`（流在 `resolve` 之后才问，那时已是地址）；<br>• `SsUdp`：`self.socket.far_end(&self.server)`（`server` 是开载体时查过的）；<br>• `Socks5Udp`：`self.socket.far_end(&self.relay)`（`external` 的 UDP 也是它，远端是本机的中继 `127.0.0.1:<端口>`）；<br>• `StreamUdp`：`new` 多参数 `far_end`，存成字段、交回它；`trojan` 与 `anytls` 的 `open_udp` 在开好流之后传 `trail::current().and_then(\|t\| t.peer())`（拨号在流的 `follow` 里，这时 `Trail` 上是那条 TCP 连接的第一跳）；<br>• `SnellUdp`：`open(stream, far_end)`，`snell` 的 `open_udp` 在 `fresh` 之后同样传 `current()` 读到的（任务 6 改成 `fresh` 交回的，P9 ②）；<br>• `Hysteria2Udp`、`TuicUdp`：任务 7（P10 ③）；`TunnelUdp`：任务 7（P11）；<br>• `MasqueUdp`、`H2Udp`、`VmessUdp`：缺省（按目标在另一个任务里懒开，登记）；测试里的实现：缺省。<br>③ **UDP 流**：`udp.rs` 的 `open` 里 `handle.mark_connected();` 之后 `note_far_end(handle, carrier.socket.as_ref(), &send_to)`。`fn note_far_end(handle: &SessionHandle, socket: &dyn PacketSocket, to: &Target)`：`socket.far_end(to)` 是 `Some(a)` → `handle.trail().set_peer(a)`，`None` → `handle.trail().clear_peer()`——载体在流的 `follow` 里开时可能记下了它所靠的 TCP 连接（socks5 的控制连接、vmess 的一条），那不是这个流的远端。被拒绝的、在 `resolve` 之前失败的流到不了这里，照旧 `None`。<br>④ **内部的 UDP 会话**（订正设计：第 10.3 节只说了客户端的 UDP 流）：`wrap_internal_udp` 在包之前照 ③ 的做法用 `datagram.far_end()` 写或清 `handle.trail()`。直连的 DoQ / DoH3 是服务端的地址，经 socks5 的是中继。<br>⑤ **记录**：`record_of` 的 `remote_addr: h.trail().peer()`，不再 `match s.transport`；文档注释照抄原文 P4。<br>⑥ **夹具**：`rurge-proto` 的 `testing/socks5.rs` 的 `FakeSocks5` 多 `relays: Arc<Mutex<Vec<SocketAddr>>>`，`relay_udp` 多一个参数，绑好中继之后推 `relay.local_addr()?`（客户端发往的就是它）；`pub fn relays(&self) -> Vec<SocketAddr>` 照 `datagrams()` 的写法，文档注释 "Where the relay of every UDP association was bound, in arrival order." |
| P9 | 复用、共用的 TCP 载体（任务 6） | ① **做法**：新拨的连接在拨号（`stack.open` 或它的等价物）之后读 `trail::current().and_then(\|t\| t.peer())`，记在连接上——拨号在会话的 `follow` 里，这时 `Trail` 上就是 `DirectConnector::connect` 刚写的第一跳；不在任何 `follow` 里时是 `None`。借出或共用时，在会话的 `follow` 里、交给会话之前 `if let Some(p) = 记着的 { trail::connected(p) }`。<br>② **snell**：`Dialer::fresh` 交回 `(SnellStream, Option<SocketAddr>)`（读在 `stack.open` 之后、`SnellStream::open` 之前）。`Pool.idle` 是 `Mutex<Vec<(SnellStream, Option<SocketAddr>, Instant)>>`，`put(stream, peer)`、`take() -> Option<(SnellStream, Option<SocketAddr>)>`，`reap`、`len` 不变。`SnellTunnel` 多字段 `peer`，`new(stream, peer, head, pool)`、`reused(stream, peer, head, pool, dialer, opts)`，`Drop` 与 `finish` 把它随连接放回池里（`finish(stream, peer, pool)`）。`SnellOutbound::open` 从池里取出的先 `connected`、再 `reused`；`open_udp` 用 `fresh` 交回的远端（替掉任务 5 的 `current()`）。**失效重拨**：`Retry` 多 `trail: Option<Trail>`，`reused` 里取 `trail::current()`（这时在会话的 `follow` 里）。`fail()` 建的重拨 future：有 trail 时先 `clear_peer()`，再在 `trail.follow(..)` 里跑原来的 `timeout(opts.timeout, dialer.fresh(&opts))`（`None` 时照旧不跟随），交回 `(stream, peer)`；`poll_open` 拿到之后把 `self.peer` 换成新的。这样重拨的连接写进会话的 `Trail`，重拨里的查找也算进这条会话的 `dnsMs`（登记：那是这条会话花的时间）。<br>③ **anytls**：`Session` 多字段 `peer: Option<SocketAddr>`（`start` 里是 `None`），`with_peer(self, peer) -> Session`（同 `with_synack_timeout` 的写法）、`peer(&self) -> Option<SocketAddr>`；`new_session` 在 `stack.open` 之后读远端、`.with_peer(peer)`；`open` 从池里取出的会话，`open` 之前记下 `session.peer()`、`open` 成功之后 `connected`。会话回到池里时字段跟着它。<br>④ **`h2pool`**：`Conn` 多字段 `peer: Option<SocketAddr>`，`connect` 在 `self.dial.dial(opts)` 之后读远端；`open` 在 `lease` 之后（新拨的与借来的都）`connected`（新拨的那一次 `Trail` 上已经是它，再写一次无妨）。<br>⑤ **ssh**：`Session` 多字段 `peer`，`establish` 在建好底下的连接之后读远端；`dial` 在每次 `session()` 之后 `connected`。<br>池的大小、复用的规则与超时都不改 |
| P10 | QUIC 一族（任务 7；订正设计：不用 quinn 的 `remote_address()`） | ① **`rurge-net::quic`**：`pub struct FarEnd(Far)`、私有的 `enum Far { Fixed(Option<SocketAddr>), Hopping(hop::Current) }`，都派生 `Clone, Debug`；`FarEnd::now(&self) -> Option<SocketAddr>`：`Fixed(a)` 是 `a`，`Hopping(c)` 是 `Some(c.now())`。`hop.rs`：`pub(crate) struct Current(Arc<Shared>)`（`Clone, Debug`），`Current::now(&self) -> SocketAddr` 是 `SocketAddr::new(self.0.server, self.0.state().port)`，`Hopping::current(&self) -> Current`。`Path` 多私有字段 `far_end: FarEnd`，`open_over` 的三种各自给：直连是 `Fixed(Some(remote))`；跳端口的是 `Hopping(hopping.current())`（在 `Arc::new(hopping)` 之前取）；经承载的是 `Fixed(carrier.far_end())`（在 `Carried::new(carrier)` 之前取，任务 5 的方法），**不是** `STAND_IN`；ECN 那一层的 `..path` 带着它。`pub fn far_end(&self) -> FarEnd` 交回克隆。不用 quinn 的 `remote_address()`：跳端口时它是第一个端口（`Shared.remote`），经承载时是替身 `192.0.2.1:443`。<br>② **`rurge-proto-quic` 的 `session.rs`**：`Dialer::connect` 交回 `(Connection, FarEnd)`（`path()` 之后、把 `path` 交给 `quic::connect` 之前取 `path.far_end()`；协议那一层换掉的是 `path.socket`，碰不到它）；`Session::connect` 把它带出来，唯一建 `Conn` 的地方存进新字段 `far_end: FarEnd`；`Lease::far_end(&self) -> Option<SocketAddr>` 是 `self.conn.far_end.now()`；`take` 在交回 `Ok` 的每一处之前 `if let Some(far) = lease.far_end() { trail::connected(far) }`——新拨的与借来的连接都这样（`take` 在会话的 `follow` 里；UDP 载体在发送时重新绑定不在任何 `follow` 里，什么也不写）。`rurge-dns` 的 `upstream/session.rs` 调 `quic::connect` 的那一处不改。<br>③ **UDP 载体**：`Hysteria2Udp`、`TuicUdp` 的 `far_end(&self, _to)` 是此刻绑定的那个 lease 的 `far_end()`（两者都在 `open` 里先 `bind`），没有绑定是 `None`。MASQUE 的 TCP 经 `take` 自然有了；它的 UDP 照旧 `None`（P8 ②） |
| P11 | WireGuard（任务 7） | `device.rs` 的 `Shared` 多两个字段：`routes: Routes`（`start` 里照 `stack.rs` 的那一句 `Routes::new(section.peers.iter().map(\|p\| p.allowed_ips.as_slice()))` 建，与协议栈选对端的是同一张表）与 `far_ends: Mutex<Vec<Option<SocketAddr>>>`（每个对端的载体的远端，`start` 里由拨好的 `carriers` 各自的 `datagram.far_end()` 初始化）。任务（`Driver`）里换掉载体的每一处同时更新它：`land` 里 `self.carriers[peer] = Some(..)` 那一句（先取 `datagram.far_end()` 再搬走 `datagram`）、收包时载体结束的 `self.carriers[peer] = None` 那一句；锁照这个文件里别的锁的写法（中毒的照样取）。`Device::far_end_for(&self, ip: IpAddr) -> Option<SocketAddr>`：`routes.lookup(ip)` 那个对端的 `far_ends`。`outbound.rs` 的 `dial` 在 `device.connect(..)` 成功之后 `if let Some(far) = device.far_end_for(ip) { trail::connected(far) }`；`udp.rs` 的 `TunnelUdp::far_end(to)`：`to` 是地址时 `far_end_for`，是名字时 `None`。直连的对端是它的端点；经底层代理的是承载的远端（任务 5 的 `PacketDatagram::far_end`） |
| P12 | 量一条记录的大小（任务 8；同 M4a 的 P15，调试构建） | **文件**：`crates/rurge-engine/src/observe/memory.rs`；`observe.rs` 里 `#[cfg(test)] mod memory;` 放在文件末尾的 `#[cfg(test)] mod tests {` 之前。开头的模块注释照 `crates/rurge-http/src/capture/memory.rs` 写：这是量、不是门，与跑的命令。`peak_resident_bytes`、`parse_count`、`parse_vm_hwm` 与它们的用例从那个文件照抄，注释写来处。<br>**`a_full_request_log_of_http_requests`**（`#[ignore]`）：`RequestLog::new(100_000)`；造 110 000 个句柄，每个像一条真实的 HTTP 请求：`dst` 是约 30 字节的名字加端口、`url` 约 80 字节、`sni` 约 20 字节、规则文字约 40 字节、两项的策略链、一条约 40 字节的备注，`set_method("GET")`、`set_status(200)`、`mark_connected`、`mark_first_byte`、`add_up` / `add_down` 与两个正文的计数；`finish(Completed)` 之后 `record_finished`（同引擎里的路，经 `record_of`）。打印 `recent` 能读到的条数（应是 100 000）与 `peak_resident_bytes()`。<br>**`an_empty_request_log_is_the_baseline`**（`#[ignore]`）：只建一份空的 `RequestLog::new(100_000)`，打印峰值。<br>**跑法**：`cargo test -j 8 --workspace --lib -- observe::memory::<名字> --ignored --exact --nocapture`，两条各单独跑一次。<br>**判断**：一条的大小 ≈（装满的峰值 − 基线的峰值）/ 100 000，取整到 8 字节；连同 1000 条（缺省）、10 000 条、100 000 条时的总量写进修正记录，任务 9a 写进 `docs/api/phase4.md`。缺省不改（M4-D25），量到的数交给所有者 |
| P13 | 收尾文档的清单（任务 9a、9b） | ① **`docs/api/phase4.md` 的「M4b　请求记录的完整」一节**：<br>• 三个新键：意思、何时 `null`、在哪里数；HEAD（P5 ③）、升级之后的字节不算正文、源站提前作答时请求正文只到记录结束（设计第 11 节第 11 条）；<br>• 九种备注的表（原文 P6）与判断次序：名单 → 开关 → CA → 来源 → 名字 → ALPN；原因在建句柄时写，之后被拒绝也带着；客户端在嗅探时关掉的不写；<br>• `remoteAddr` 的新口径与订正的那一句（写成：`docs/api/phase1.md` 那一段写的"会话自己开的那条 TCP 连接，复用、隧道与 UDP 一律 `null`"是 M4b 之前的口径；M4b 起是这条会话离开 rurge 时所经的那条连接或载体的远端）；各载体的表（照设计第 10.3 节的表、按实际写）；照旧 `null` 的三种；<br>• `dnsMs` 在解析途中随时间增长；<br>• 量到的一条记录的大小（任务 8 的数、怎样量的，一台机器上的一次运行，不是 CI）。<br>② **`docs/acceptance/phase4-manual.md`** 的 M4 一节加 M4b 的条目，每项写怎样做、看什么：真实浏览器的页面请求，三个新键与开发者工具的数对得上量级；`always-raw-tcp-hosts` 命中与 `[MITM]` 开关关着时备注的那一句；经真实的 snell（`reuse=true`）/ anytls / hysteria2 / tuic / WireGuard 服务端的会话，`remoteAddr` 是服务端的地址，跳端口的 hysteria2 跳过之后是新的端口；经 socks5 底层代理的 hysteria2 是代理的 UDP 中继；DIRECT 的 QUIC（浏览器的 HTTP/3）记录有 `remoteAddr`；`dns-server` 指到一个慢的上游时，进行中的会话 `dnsMs` 在增长。开头的项数随之改。<br>③ **`docs/status.md` 的 M4b 一节「以后的会话必须知道的事」**至少写：<br>• 直通的原因在建句柄时写（`dial_noted` 与 `PassedBy`）：新的入口要么经 `Dialer::dial`，要么自己给 `PassedBy`；<br>• `Decision` 的五种，名单先看；<br>• 新加的 rurge 自己的答复要在 `finish` 之前调 `own_body`；新的应答正文类型要在交出每一帧时、`complete()` 之前数；<br>• 新加的复用或共用的载体要记着自己的远端、借出时 `connected`；新的 UDP 载体要答 `far_end`；<br>• QUIC 的远端是 `Path::far_end`，不是 quinn 的；WireGuard 换载体的地方要同时更新 `far_ends`；<br>• `Trail::dns()` 含在途的查找，新的查找用 `Lookup::begin()`；`trail::current()` 只在 `follow` 里有。<br>兼容性清单 L884 登记设计第 11 节第 6 ～ 8、11 条与本计划的四处：HTTP/2 上 HEAD 的、rurge 自己的页数页的长度（P5 ③）；被拒绝的也带着备注、`Sniffed::Closed` 不写（P6）；重拨里的查找算进 `dnsMs`（P9 ②）；照旧 `null` 的三种（P8 ②） |
| P14 | 端到端的做法 | • 引擎的用例照所在文件的写法（`mod common;` 的 `Harness`、`Profile`、`harness`）；读记录照这个文件里读记录的现有写法，进行中的读 `active()`。<br>• "服务端的地址"是假服务端的 `addr()`（snell、anytls、HTTP/2、SSH、hysteria2、tuic、MASQUE 的假服务端与 `FakeWgPeer`）；shadowsocks 的 UDP 是 `FakeShadowsocks::udp_addr()`；socks5 的 UDP 中继是 `FakeSocks5::relays()`（P8 ⑥）。断言写 `assert_eq!(record.remote_addr, Some(..), "{record:?}")`。<br>• 要查找的名字（`snell.test` 一类）经一行 `[Host]` 钉到 `127.0.0.1`，第二道防线是共用夹具的 `dns-server`。<br>• "连接回到了池里"照这个文件里测复用的现有用例的等法（有时限）；"服务端只见过一条连接"用假服务端的连接数（有的话）。<br>• "记录结束了"、"`dnsMs` 涨到了"用 `wait_until`（有时限）；"什么也没有发生"的固定等待写注释（等的是什么、余量多大）。<br>• 同时的两条会话用 `tokio::join!` 或两个任务，各自有时限。<br>• 新写的带等待或并发的用例提交前单独循环跑二十次以上，次数与结果写进报告 |
| P15 | 现有的用例 | 见下面的表。表里"预计照过"的若红，停下报告，不改期望 |

### 表后的原文（照抄）

P4、P6、P7 要照抄的文字含反引号，放在这里。每个代码块里的文字就是要的那一串，没有为 Markdown 加的转义；文档注释照所在文件的宽度折行，`{…}` 是要填的值。

**P4：`observe.rs` 的 `RequestRecord` 的文档注释**

```text
receive_ms（任务 1）
From the first byte that came back from upstream until the session ended
(`phase 4 M4 design 10.1`); `None` while the session goes on, and when no
byte ever came back. With `connect_ms` and `first_byte_ms` it makes the three
spans of a request: the connection, the wait for the answer, its body.

request_body_bytes（任务 1）
The bytes of an HTTP request's body that went upstream, counted as the engine
handed each frame on, up to the moment the record was made (`phase 4 M4
design 10.1`): a body still going up after the answer ended counts what went
before. `None` for a session that is no HTTP request; `up` counts every byte
that went upstream, the head included.

response_body_bytes（任务 1）
The bytes of the answer's body handed to the client, counted the same way:
the upstream's, or the body of an answer of rurge's own — an error page, a
refusal, a `[Map Local]` answer. `None` for a session that is no HTTP
request. A `HEAD` answered on HTTP/1.1 counts none: hyper sends no body
there.

dns_ms（任务 4，换掉整段）
The time the dial spent on name lookups, the rules' and the outbound's
together, a lookup still under way counted up to the moment the record is
read (`phase 4 M4 design 10.4`); `None` when it made none — the target is an
address, or the proxy resolves the name itself.

remote_addr（任务 5，换掉整段）
The far end of the connection or carrier the session left rurge by (`phase 4
M4 design 10.3`): one its own dial made, one it borrowed from a pool or
shares with other sessions, or a UDP flow's carrier — the first hop when the
policy chains. `None` when it left by none (rejected, failed before it was
dialled) or when its carrier cannot tell: the UDP carriers of `vmess`,
`h2-connect` and MASQUE open their connections in another task, target by
target.
```

**P6：九句备注（`serve.rs` 的私有常量：名字与文字）**

```text
RAW_HOSTS       Passed through: always-raw-tcp-hosts
RAW_KEYWORDS    Passed through: always-raw-tcp-keywords
NEITHER         Passed through: neither HTTP nor TLS
TIMED_OUT       Passed through: sniffing timed out
MITM_OFF        Not decrypted: MITM is off
NO_CA           Not decrypted: no usable CA
NO_PROTOCOL     Not decrypted: the client offers no protocol rurge decrypts
SOURCE          Not decrypted: the client is not in client-source-address
NO_CERTIFICATE  Not decrypted: no certificate can be made for this name
```

**P7：`trail.rs` 的文档注释**

```text
模块注释的第一句（换掉 "What a dial leaves of itself … waited for name lookups."）
What a dial leaves of itself for the session that made it: the far end its
traffic leaves rurge by, and how long it waited for name lookups.

peer（换掉整段）
The far end the session's traffic leaves rurge by, as the dials and leases
under this trail last noted it: the TCP connection a dial made, or the
connection or carrier a pool or a shared connection lent (`phase 4 M4 design
10.3`).

connected（换掉整段）
The session's traffic leaves rurge by a connection or carrier whose far end
is `addr`: a TCP connection just made, or a pooled or shared one just lent to
the session (`phase 4 M4 design 10.3`). An IPv4-mapped address
(`::ffff:a.b.c.d`) is noted as the IPv4 address it is, as the project reads
it at every door. Does nothing outside a `follow`.

dns（换掉整段）
The time spent on name lookups: those that ended, and those under way up to
now (`phase 4 M4 design 10.4`); `None` when none was begun.

Lookup
A name lookup under way (`phase 4 M4 design 10.4`): `begin` registers it on
the trail being followed, if any; dropping it — when the lookup ends, however
it ends, or when a dial gives up on it half way — adds the time it took and
lets it go. It ends on the trail it began on, whichever task drops it.

current
The trail being followed now, a clone that shares its record; `None` outside
a `follow`. For what notes a far end later and elsewhere: a redial in the
relay, a carrier's far end read after its dial.
```

### P15 的表：要改的、要留意的现有用例

| 用例 | 为什么 | 任务 | 预计 |
| ---- | ------ | ---- | ---- |
| `crates/rurge-inbound/src/session.rs` 的现有用例 | 多两个计数与四个方法 | 1 | 照过 |
| `crates/rurge-engine/src/observe.rs` 的 `a_record_tells_the_port_the_address_and_the_lookup_time_of_its_session` | 多三个字段；TCP 的远端照旧 | 1、5 | 照过 |
| `crates/rurge-engine/src/observe.rs` 的 `the_record_of_a_udp_session_has_no_remote_address` | UDP 的记录也读 `Trail`（P8 ⑤） | 5 | 改：改名 `a_udp_record_reads_the_trail_as_a_tcp_record_does`，进行中与结束之后都是 `Some(peer)` |
| `crates/rurge-api/src/routes/requests.rs` 的 `every_field_of_a_record_is_sent_under_its_own_name` | 多三个键 | 1 | 改：字面量与期望的 JSON 各补三个（任务 1 的用例） |
| 同一文件的 `the_timings_are_null_until_known` | 三个新键是 `null` | 1 | 改：补三个 `null` 的断言 |
| 同一文件的其余用例与 `git grep -n 'RequestRecord {'` 列出的字面量 | 字面量要补字段 | 1 | 改：只补字段（`None`），期望不变 |
| `crates/rurge-api/tests/api.rs` 的 `requests_carry_the_listen_port_the_remote_address_and_the_lookup_time` | TCP；目标是地址，没有查找 | 1、4、5 | 照过 |
| `crates/rurge-http/src/engine.rs` 测试模块的全部 | `Idle::new` 多一个参数、`Fake::open` 多一个钩子；数正文不改行为；先建应答再 `finish` 不改交出去的东西 | 2 | 照过 |
| M4a 的抓包用例（`crates/rurge-http/src/capture/`、`crates/rurge-engine/tests/capture.rs`、`crates/rurge-api` 的抓包与 HAR 用例） | 包住应答的那一层一行不改；条目的 `remote` 读同一个 `Trail`，经引擎的请求都是 TCP | 2、5 | 照过 |
| `crates/rurge-http/src/mitm/decrypt.rs` 的 `decides_in_the_design_order` | `Pass` 拆成三种、名单先看 | 3 | 改：改名 `decides_lists_first_then_source_name_and_alpn`（任务 3 的用例） |
| 同一文件断言 `Decision::Pass` 的其余（`with_h2_an_h2_only_client_is_decrypted` 等） | 同上 | 3 | 改：换成此时成立的那一种 |
| `crates/rurge-engine/tests/sniff.rs` 与 `tests/mitm.rs` 的现有用例 | 直通的会话多一句备注 | 3 | 照过；断言 `notes` 整个相等或为空的，任务 0 逐条列出，写明那条会话会不会多一句 |
| `crates/rurge-net/src/trail.rs` 的现有用例 | `dns()` 多算在途的；没有在途的票据时值不变 | 4 | 照过 |
| `crates/rurge-dns/src/resolver.rs` 的 trail 用例（`lookups_are_noted_on_the_trail_of_whoever_is_dialling`、`a_lookup_cut_short_is_noted_for_the_time_it_took` 等四条） | `Noted` 换成 `Lookup` | 4 | 照过 |
| `crates/rurge-engine/tests/auto_groups.rs` 断言测试的会话是 `(0, None, None)` 的那一条 | 测试不在会话的 `follow` 里，什么也不记 | 4、5 | 照过 |
| `crates/rurge-engine/tests/udp.rs` 的 `a_flow_through_a_socks5_proxy_has_no_remote_address` | UDP 流记载体的远端 | 5 | 改：改名 `a_flow_through_a_socks5_proxy_records_the_relay`，期望 `Some(up.relays()[0])` |
| 同一文件的 `the_lookup_a_rule_makes_counts_for_a_flow_too`（两个被拒绝的流是 `None`） | 被拒绝的流不开载体、不写远端 | 5 | 照过 |
| `crates/rurge-engine/tests/pipeline.rs` 的 `a_carrier_over_a_socks5_association_has_no_remote_address` | 内部的 UDP 会话记载体的远端（P8 ④） | 5 | 改：改名 `a_carrier_over_a_socks5_association_records_the_relay`，期望是 `proxy.relays()` 里的一个 |
| 同一文件的 `an_internal_dns_session_records_the_server_it_connected_to`、`a_smart_group_forgets_the_address_of_a_member_that_failed`、`a_rejected_session_has_no_remote_address_but_the_lookups_of_the_rules` 与第 3008 行附近断言 `None` 的那一条 | TCP 的远端照旧；`tcp://` 的上游没有 UDP 的内部会话 | 5 | 照过 |
| `crates/rurge-engine/tests/http.rs` 断言 `remote_addr` 的四处 | 经引擎的请求是 TCP；借来的连接照 M1b 的 `set_peer` | 5 ～ 7 | 照过 |
| `crates/rurge-proto` 里建 `SnellTunnel`、调 `Pool::put` / `take`、`SnellUdp::open`、`StreamUdp::new`、anytls 的 `Session::start` 的单元用例 | 多一个参数或一项 | 5、6 | 改：只补参数（`None`） |
| `crates/rurge-net/src/quic/` 与 `crates/rurge-proto-quic/src/` 里写 `Path { .. }` 字面量、建 `Conn` 的单元用例 | 多一个字段 | 7 | 改：只补字段 |
| `crates/rurge-engine/tests/outbounds_*.rs` 的现有用例 | 只多了远端 | 5 ～ 7 | 照过 |
| `crates/rurge/tests/sockets.rs` 的扫描用例 | 不建套接字 | 全部 | 照过 |
| `crates/rurge-platform/tests/unsafe_functions.rs` | 不加放开的函数 | 全部 | 照过 |
| `crates/rurge-config/tests/secrets.rs` | 不加带凭据的写法 | 全部 | 照过 |

---

## 执行期修正记录

下面写"执行者的裁定"的，是 Pi 在设计与计划的原则下定的；写"协调者的裁定"的，是协调的会话代项目所有者定的。项目所有者都可以改。计划的正文不改：正文里说得不对的地方，以这张表的行为准。

| # | 任务 | 内容 | 提交 |
| - | ---- | ---- | ---- |
| 1 | 0 | 开工前核对（第 1、2、4、6 项）：`git status --short` 空；分支 `phase4-m4`；`HEAD` 是 `47f4552`；`git log --oneline 33efe98..HEAD` 恰好一个提交（`47f4552 docs: 阶段 4 / M4b 实施计划（请求记录的完整）`），与派工一致。读了设计第 18 节（先读）、第 10 ～ 18 节、本计划的 P1 ～ P15 与 P15 的表（工作区的 `common-brief.md` 整份）、`docs/status.md` 阶段 4 的 M4a（第 1013 ～ 1042 行）、M3b（第 949 ～ 961 行）、M2b（第 772 ～ 787 行）、M1b（第 684 ～ 698 行）四处与 `docs/api/phase1.md` 第 59 ～ 70 行。数：`Cargo.lock` 的 `[[package]]` 520 个、`unsafe_functions.rs` 的 `LISTED` 23 项（声明 23、实际 23）、`sockets.rs` 的 `LISTED` 26 项（声明 26、实际 26）、带本机账户名的文件 26 个，四项都照旧。门禁（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过（exit=0，0 个警告），`cargo test -j 8 --workspace --no-fail-fast` 93 个结果行、5309 通过、0 失败、15 忽略（exit=0），与基线（93 / 5309 / 0 / 15）相同，没有重跑；日志在工作区的 `gate-task-0-fmt.log`、`gate-task-0-clippy.log`、`gate-task-0.log` | 本提交 |
| 2 | 0 | 名字与签名（第 3 项）：与 P3 ～ P11 写的**一处也不冲突**，一项也没有要改的。逐项核对过的：`SessionHandle` 的字段（`session.rs:77`～`109`，`up` / `down` 在 154 / 155、`connected` 157、`first_byte` 159、`method` 163、`status` 164）与 `new`（120）/ `new_with_token`（128）/ `new_with_slot`（137，建全部字段的那一个）、`add_up`（305）、`add_down`（309）、`bytes`（314）、`mark_connected`（322）、`mark_first_byte`（329）、`connect_time`（382）、`first_byte_time`（387）、`set_method`（270）、`method`（276）、`add_note`（236）、`notes`（240）、`trail`（247）、`elapsed`（203）（`first_byte` 存的正是 `started.elapsed()`，P3 的 `first_byte_at()` 成立）；`RequestRecord`（`observe.rs:28`）与 `record_of`（104）、`RequestLog::{new 172, mark_active 181, record_finished 199, recent 219, active 231}`；`RequestJson`（`requests.rs:45`）与 `From<&RequestRecord>`（84）；`mitm/decrypt.rs` 的 `Decision`（42）、`decides`（161，今天次序是来源 → 名单 → 名字 → ALPN，与设计第 10.2 节的描述相同）、`blocked`（232）、`widens`（207）；`serve.rs` 的 `sniffable_session`（129，七步在 131 / 141 / 148 / 156 / 161 / 171 / 182）、`named_at_a_proxy_port`（198）、`forces_http_engine`（221）、`named_target`（238）、`widened_by_mitm`（254）、`decrypting`（280，开关在 281 最前、名单只在没有 CA 那一支里看，与 P6 ③ 要改的现状一致）、`serve_sniffed`（695）、`trail.rs` 的 `CURRENT`（21）/ `Left`（27）/ `follow`（42）/ `connected`（74）/ `looked_up`（90）/ `set_peer`（60）/ `dns`（66）、`resolver.rs` 的 `Noted`（1299）与 `use tokio::time::Instant`（40）、`connector.rs` 的 `Datagram`（28）/ `PacketSocket`（61）/ `DirectDatagram`（964）/ `DirectPacket`（770）、`packet_datagram.rs` 的 `PacketDatagram`（34）、`cell.rs` 的 `Relayed`（195）、`dns_pipeline.rs` 的 `FinishOnDrop`（223）/ `CountingDatagram`（312）/ `wrap_internal_udp`（359）、`udp.rs` 的 `Carrier`（248）/ `run_flow`（503）/ `open`（598）、`snell/pool.rs` 的 `Pool`（25）、`snell/tunnel.rs` 的 `Conn`（106）/ `Retry`（160）/ `SnellTunnel`（170）/ `retry` 的重拨（221）、`h2pool` 的 `Conn`（407）/ `Lease`（493）、`quic/mod.rs` 的 `Path`（38）/ `open`（55）/ `open_resolved`（71）/ `open_over`（82）、`hop.rs` 的 `Shared`（115）/ `Hopping`（144）、`carrier.rs` 的 `STAND_IN`（27）/ `Carried::new`（37）、`proto-quic/src/session.rs` 的 `Dialer::{connect 164, path 172}` 与 `Session::{lease 387, take 452, dial 512, connect 551}` 与 `Conn`（706，唯一的建处 522）、`wireguard/src/device.rs` 的 `Shared`（155）/ `Device`（245）/ `start`（268）/ `Driver`（659）/ `land`（759）/ `carriers` 的赋值（299、781、909）与建 `Shared` 的唯一处（325）、`routes.rs` 的 `new`（18）/ `lookup`（32）、`stack.rs:158`、`outbound.rs` 的 `dial`（142）/ `open_udp`（331）、`udp.rs` 的 `TunnelUdp`（20）；`rurge-config` 的 `MitmConfig::effective`（`mitm.rs:31`）与 `HostList::{matches 70, matches_sni 82}`、`General::{force_http_engine_hosts 202, always_raw_tcp_hosts 203, always_raw_tcp_keywords 204}` | 本提交 |
| 3 | 0 | 现有用例（第 5 项，P15 的表逐条对过，一条也不与预计不同）：任务 1 的 `session.rs` 21 条与 `observe.rs` 的 `a_record_tells_the_port_the_address_and_the_lookup_time_of_its_session`（`:541`）照过，`requests.rs` 的 `every_field_of_a_record_is_sent_under_its_own_name`（`247`）与 `the_timings_are_null_until_known`（`413`）照表改；`git grep -n 'RequestRecord {'` 只有三处字面量（`requests.rs:214`、`requests.rs:248`、`observe.rs:120`），**都不带 `..`**，任务 1 要补字段。任务 2 的 `engine.rs` 测试模块 121 条里没有一条读 `body_bytes()`，`Idle::new(` 的唯一调用处是 `engine.rs:1043`（句柄叫 `handle`，`engine.rs:481`）。任务 3：`crates/rurge-engine/tests/sniff.rs`（21 条）与 `tests/mitm.rs`（37 条）里**没有一条断言 `notes`**，也没有一处整个比一条 `RequestRecord`，所以 P15 那一行要求逐条列出的答案是"一条也没有"，多一句备注不会让它们红；`decrypt.rs` 的 `decides_in_the_design_order`（`534`）照表改名并改成新次序，`with_h2_an_h2_only_client_is_decrypted`（`749`）里只断言 `Decrypt` / `AlpnMismatch`。任务 4：`trail.rs` 的 12 条与 `resolver.rs` 的四条 trail 用例都先等查找结束再读，照过（包括 `a_lookup_cut_short_is_noted_for_the_time_it_took`，`1770`）；全工作区断言 `dns_ms` 为 `None` 的用例（`pipeline.rs:1818`、`3597`、`3643`、`3677`、`3704`、`3740`、`3816`，`auto_groups.rs:275`，`udp.rs:644`、`676`，`api.rs:1358`）都是"没有查找"，仍为 `None`；唯一读进行中记录的 `dns_ms` 的是 `observe.rs:579` 的 `live`，而它正是 P15 要改的那一条。任务 5 ～ 7：`observe.rs` 的 `the_record_of_a_udp_session_has_no_remote_address`（`578`）、`tests/udp.rs` 的 `a_flow_through_a_socks5_proxy_has_no_remote_address`（`587`）、`tests/pipeline.rs` 的 `a_carrier_over_a_socks5_association_has_no_remote_address`（`3822`）照表改；断言 `remoteAddr` 为 `null` 的其余用例（`api.rs:1358`、`pipeline.rs:3683`、`pipeline.rs:3701`、`auto_groups.rs:275`）都是被拒或没有拨号，仍为 `null`。补扫（任务 1 加字段会编不过、任务 5 ～ 7 加字段要补的地方）：`SessionHandle {`、`RequestJson {`、`SnellTunnel {`（`tunnel.rs:188`）、`Retry {`（`tunnel.rs:208`，另有解构 `230`）、`PacketDatagram {`（`packet_datagram.rs:127`）、`StreamUdp {`（`stream_udp.rs:125`）、`Conn {`（`proto-quic/src/session.rs:522`、`h2pool/mod.rs:327`）、`Shared {`（`wireguard/src/device.rs:325`、`quic/hop.rs:168`）、`Session {`（`anytls/session.rs:325`、`anytls/session.rs:340`、`rurge-proto-ssh/src/outbound.rs:718`）各只有一处或两处，都是生产代码自己的建法；`Path {` 另有 `crates/rurge-net/src/quic/dial.rs:629` 与 `quic/h3/mod.rs:266` 两个**测试模块里的字面量**，任务 7 加私有字段时要一并补；对 `Decision` 的穷尽 `match` 只有 `serve.rs` 的 `decrypting` 一处；断言 `/v1/requests` 一条记录键的集合或个数的用例只有 `requests.rs:247`（P15 已点名）；没有一条现有用例断言 `remoteAddr` 或 `dnsMs` 的键数 | 本提交 |
| 4 | 0 | 外部库的事实（写计划时要核对的第 3 项「外部库」一条，读的是锁文件锁定版本的源码）：hyper 1.11.1 的 HTTP/1.1 服务端不写 HEAD 应答的正文（`hyper-1.11.1/src/proto/h1/role.rs:515` 的 `can_have_body` 交给 `can_chunked`，后者在 `:519`～`:527` 对 `HEAD`、`CONNECT` 的 2xx 与 1xx 答 `false`；`hyper-1.11.1/src/proto/h1/dispatch.rs:392` 的 `if !self.conn.can_write_body()` 把正文 `clear_body` 丢下），HTTP/2 服务端不看 HEAD、照样发（`hyper-1.11.1/src/proto/h2/server.rs:515` 的 `if !body.is_end_stream()` 就建 `PipeToSendStream`）——P5 ③ 成立。tokio：锁文件里的版本是 1.53.1（`Cargo.lock` 的 `name = "tokio"`），工作区给 `tokio` 开着 `time`；`test-util` 在七个 crate 的开发依赖里（`crates/rurge-dns/Cargo.toml:37`、`rurge-engine:48`、`rurge-net:44`、`rurge-proto-ssh:28`、`rurge-proto:52`、`rurge-tun:24`、`crates/rurge/Cargo.toml:54`）；`tokio::time::Instant::now()` 是 `variant::now()`（`tokio-1.53.1/src/time/instant.rs:48`），开了 `test-util` 时走 `clock::now()`（`tokio-1.53.1/src/time/clock.rs:284`）：从来没暂停过时取系统时钟（`:286`），暂停过而当前执行上下文里有时钟时取那个时钟（`:290`），没有时（时钟暂停之后、运行时之外）取系统时钟（`:293`）；`task_local!` 的 `LocalKey::try_with<F, R>(&'static self, f: F) -> Result<R, AccessError>` 写在 `tokio-1.53.1/src/task/task_local.rs:245`，错误的类型是 `AccessError`，正是 `trail::current()` 要的 `CURRENT.try_with(Trail::clone).ok()`（今天 `trail.rs:75` 与 `:91` 用的就是 `try_with`）。quinn（0.11.12）：不用它的 `remote_address()`（P10），今天 `rurge-net` 与 `rurge-proto-quic` 里没有一处调它（`git grep -n 'remote_address' -- crates/rurge-net crates/rurge-proto-quic crates/rurge-dns` 为空），所以任务 7 不会与现有代码冲突 | 本提交 |
| 5 | 0 | 内部的几件（写计划时要核对的第 3 项末段）：① 记录在**句柄的 `on_finish` 钩子里**拍下：`Engine::attach_observers`（`crates/rurge-engine/src/engine.rs:583`～`597`）给每个注册的句柄挂一个 `on_finish`，里面调 `o.log.record_finished(h, outcome, &o.traffic)`（`:591`），`dns_pipeline.rs:770` 的内部分话也是同一个写法；所以 P5 ⑤ 说的"钩子在 `finish` 里跑，与记录拍下是同一刻"**站得住**（钩子在 `SessionHandle::finish` 里、按挂上的先后跑），若测试的钩子挂在 `attach_observers` 之后，它看到的 `body_bytes()` 与记录里的一样；`finish` 之后再加的计数进不了记录（`record_of` 在那之前就读完了）。② `trail::follow` 是 `CURRENT.scope(self.clone(), f)`（`trail.rs:42`），只对这一支 future 生效、`tokio::spawn` 出去的看不到；`DirectConnector::connect` 里的 `trail::connected`（`connector.rs:658`）写的就是 `CURRENT`（`trail.rs:74`）；会话的拨号在 `follow` 里（`engine.rs:2052`～`2055` 的 `trail.follow(self.connect_session(..))`，`dial_route` 在 `connect_session` 里面），转发（`Dialer::relay`，`engine.rs:2058`～`2067`）**不在**，所以 P9 ② 的重拨要自己 `follow`。③ `turn_back` 之后 `s.dst_host` 确实就是 `sniffable_session` 看的那个名字：`sniffable_session` 用 `named_at_a_proxy_port` 把 fake 地址换成名字（`serve.rs:148`～`151` 与 `:198`～`218`），`turn_back` 把 `session.dst_host` 写成 `HostName::Domain(name)`（`fake_address.rs:56`～`69`）；HTTP 引擎里每个请求的拨号走 `ConnUpstreams::route` / `dial`（`crates/rurge-engine/src/http.rs`），不经 `Dialer::dial`，所以直通备注只在 `serve_sniffed` 与 `Dialer::dial` 两条路上写。④ `Hopping` 握着的是 `shared: Arc<Shared>`（`hop.rs:144`～`148`），`Shared.remote` 是第一个端口的地址（`hop.rs:115`～`121` 的文档写着），P10 ① 的 `Current(Arc<Shared>)` 直接拿 `self.shared.clone()` 即可；`Shared` 与 `state()` 都是私有的，`Current` 放在 `hop.rs` 里才拿得到。⑤ WireGuard 的 `Driver`（持有 `carriers`，`device.rs:659`～`675`）跑在 `tokio::spawn(driver.run(out))` 里（`device.rs:375`），它自己持着 `shared: Arc<Shared>`，所以 P11 的 `far_ends` 放在 `Shared` 里、由 `Driver::land`（`759`）、`carriers[peer] = None`（`909`）与 `Device::start` 的初值（`299`）更新是可行的 | 本提交 |
| 6 | 0 | 协调者核过任务 0（`9205dfe`，第 1 ～ 5 行）：门禁的数（93 / 5309 / 0 / 15，一台机器上的一次运行，不是 CI；没有崩溃与重跑；日志里 `cannot bind http-api … (os error 10048)` 那一行是现有用例的预期输出）、`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0、计划的两张表只多不少（修正记录多第 1 ～ 5 行，延后事项没有动）、弯引号 0、带本机账户名的文件 26，都对得上。第 3 行的补扫抽查过：`git grep -n 'RequestRecord {'` 在 `crates/` 下只有那三处字面量；`Decision::` 在 `rurge-http` 之外只有 `crates/rurge-engine/src/serve.rs:324`、`:325`、`:337`；`Path {` 的两个测试字面量在；`tests/sniff.rs` 与 `tests/mitm.rs` 里没有 `notes`；断言 `remote_addr` / `remoteAddr` 的用例除第 3 行列的之外还有 `crates/rurge-engine/tests/http.rs:1434`（什么也没拨，照旧 `None`）、`crates/rurge-engine/tests/pipeline.rs:3008`、`:3797`（P15 已列，照过）与 `crates/rurge-api/tests/api.rs:3831`（M4a 抓包条目的 `remoteAddr`，构造的值，不受影响）。第 2、3、5 行引的本仓行号抽查了九十来处，十处差一到五行：`crates/rurge-inbound/src/session.rs` 的 `new_with_token` 在 `:126`（写成 128），`new_with_slot` 里 `connected`、`first_byte`、`method`、`status` 的初始化在 `:156`、`:157`、`:158`、`:159`（写成 157、159、163、164）；`crates/rurge-net/src/trail.rs` 的 `static CURRENT` 在 `:18`、`CURRENT.scope` 在 `:41`、`connected` 在 `:77`、`looked_up` 在 `:93`（写成 21、42、74、90）；`crates/rurge-engine/src/udp.rs` 的 `handle.mark_connected();` 在 `:716`（写成 719）。都以名字为准，后面的任务照名字找。第 4、5 行的事实接受：P5 ③（hyper 1.11.1 对 HEAD）、P5 ⑤（记录在 `on_finish` 的钩子里拍下，与测试的钩子同一刻）、P7（`tokio::time::Instant`、`try_with`）、P9 ②（转发不在 `follow` 里，重拨要自己 `follow`）、P10 ①（`Hopping.shared` 是 `Arc<Shared>`，`Current` 放在 `hop.rs`）、P11（`Driver` 握着 `shared`）都成立，计划正文不改 | 本提交 |
| 7 | 1 | 任务 1 做完了（`session.rs`、`observe.rs`、`requests.rs`）：`SessionHandle` 多 `request_body` / `response_body` 两个 `AtomicU64`（`session.rs:95`、`:96`，`new_with_slot` 里在 `:165`、`:166` 初始化为 0，另两个构造函数照旧调它）、`add_request_body`（`:334`）、`add_response_body`（`:339`）、`body_bytes`（`:344`）与 `first_byte_at`（`:420`，紧挨 `first_byte_time`）；`RequestRecord` 多三个字段（`observe.rs:70`、`:76`、`:96`，两段文档注释照 P4 抄，`dns_ms` 与 `remote_addr` 这一次不动），`record_of` 的 `elapsed` 只取一次（`:125`）、`remote_addr` 的 `match s.transport` 不动；`RequestJson` 多三个键（`requests.rs:73`、`:76`、`:86`）与 `From` 的三个值（`:126` 起），字段位置与 `RequestRecord` 相同。新加 6 条用例：`session.rs:907`（`the_body_counters_add_up_apart_from_the_bytes`）、`:921`（`first_byte_at_is_counted_from_the_start`），`observe.rs:633`（`a_finished_record_has_its_receive_time_and_its_body_bytes`）、`:669`（`a_session_that_is_no_http_request_has_no_body_bytes`）、`:712`（`the_receive_time_is_none_without_a_first_byte`），`requests.rs:460`（`the_new_keys_sit_where_the_design_puts_them`）。P15 表点名的两条照改：`every_field_of_a_record_is_sent_under_its_own_name`（`requests.rs:262`）的字面量与期望 JSON 各补 `receive_ms: Some(30)`、`request_body_bytes: Some(5)`、`response_body_bytes: Some(1234)` 与 `"receiveMs": 30`、`"requestBodyBytes": 5`、`"responseBodyBytes": 1234`，`the_timings_are_null_until_known`（`:434`）补三个 `null` 的断言、之后各设一个数看得到；`git grep -n 'RequestRecord {'` 的三处字面量都补齐了。门禁（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0，`cargo test -j 8 --workspace --no-fail-fast` 93 个结果行、5315 通过、0 失败、15 忽略（基线 5309 加本任务 6），没有崩溃与重跑；日志在工作区的 `gate-task-1-fmt.log`、`gate-task-1-clippy.log`、`gate-task-1.log` | 本提交 |
| 8 | 1 | 执行者的裁定（写法）：`record_of` 里 `method` 只读一次（`let method = h.method();` 与 `let is_http = method.is_some();`），两个字节数写 `is_http.then_some(..)`；P4 写的是 `h.method().map(\|_\| …)`，而 `h.method()` 每调一次都克隆那个 `String`，读一次意思不变。`receive_ms` 紧跟 `dns_ms`、两个字节数紧跟 `status_code` 的位置照 P4 | 本提交 |
| 9 | 1 | 与 P15 表的一处出入：`the_timings_are_null_until_known` 的文档注释原来写 "The two timing fields (M3c design 4.1)"，这条用例现在断言五个字段（`connectMs`、`firstByteMs` 与新加的 `receiveMs`、`requestBodyBytes`、`responseBodyBytes`），注释改成 M4b 的一句（`phase 4 M4 design 10.1`）；用例名与 P15 表要的断言改法不变 | 本提交 |
| 10 | 1 | 修正轮 1（协调者的评审）：只改了一处注释——`the_timings_are_null_until_known` 的文档注释（`requests.rs:432`、`:433`）原来写 "The three keys of M4b (`phase 4 M4 design 10.1`): `null` until known, numbers after, and `null` for a session that is no HTTP request."，与用例不符：① 用例断言的五个键里，前两个 `connectMs`、`firstByteMs` 是 M3c 的；② "`null` for a session that is no HTTP request" 这条用例没有测（`record()` 的 `method` 是 `None`，用例随后在同一份记录上设了 `request_body_bytes = Some(78)`，API 照样交出 `78`），那是 `record_of` 定的，API 这一层只照抄。换成 "The two timing fields (M3c design 4.1) and the three keys of M4b (`phase 4 M4 design 10.1`): `null` until known, numbers after."，用例名、断言与别的文件都没动。跑过（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0，`cargo test -j 8 --workspace --lib -- routes::requests::tests` 9 条全过（exit=0），日志在工作区的 `gate-task-1fix1-fmt.log`、`gate-task-1fix1-clippy.log`、`gate-task-1fix1-test.log`；只改了一处注释，没有跑全量门禁（任务 2 的全量门禁会把它包含进去） | 本提交 |
| 11 | 1 | 协调者核过任务 1（`a535147`，第 7 ～ 9 行；修正轮 1 `8f7623f`，第 10 行）：门禁的数（93 / 5315 / 0 / 15，一台机器上的一次运行，不是 CI；没有崩溃与重跑；日志里 `cannot bind http-api … (os error 10048)` 那一行是现有用例的预期输出）、`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0、计划的两张表只多不少（修正记录多第 7 ～ 10 行，延后事项没有动）、弯引号 0、带本机账户名的文件 26，都对得上；`a535147` 只改了三个源文件与计划，`Cargo.toml` 与 `Cargo.lock` 没有动。`RequestRecord` 三个新字段的文档注释与「表后的原文」P4 的三段逐字比对过（去掉 `///` 与折行之后相同）；`record_of` 里 `elapsed` 只取一次、`receive_ms` 是 `outcome.and(h.first_byte_at())`、两个字节数以 `method` 是不是 `Some` 定，与 P4 同义（第 8 行的写法接受）；`RequestJson` 三个键的位置与 `the_new_keys_sit_where_the_design_puts_them` 的五个比较对得上设计第 10.1 节；六条新用例与 P15 表点名的两条照 brief。第 9 行换上的那一句注释与用例不符（说成"三个键"，又说"不是 HTTP 请求的是 `null`"，这条用例两样都不测），修正轮 1 换成照用例的一句（第 10 行；只改注释，跑了 fmt、clippy 与那个模块的用例，没有跑全量门禁，任务 2 的门禁会包含它）。第 7 行引的行号抽查了二十四处，一处差两行：`session.rs` 的 `response_body: AtomicU64` 在 `:98`（写成 96）；以名字为准。**任务 1 过了** | 本提交 |
| 12 | 2 | 任务 2 的生产代码（`crates/rurge-http/src/engine.rs`，行号是 `b4ae895` 上的）：`Idle` 多字段 `handle: Arc<SessionHandle>`（`:2755`，`new` 的同名参数在 `:2765`、排在 `tap` 之后，唯一调用处（`:1053` 附近）给 `handle.clone()`），数据帧那一支的 `this.handle.add_request_body(data.len() as u64)` 在 `:2811`，与 `Meter` 各记各的、两种版本都数；`Tail` 的 `PolledUpstream::Frame` 那一支在 `:3434` 数应答正文（不看 `counted`），`if ended { this.complete(); }` 在 `:3438`（`Tail::complete` 在 `:3032`）；`LocalFile` 的 `LocalRead::Data` 那一支在 `:3294` 数，`if this.left == 0 { this.complete(); }` 在 `:3295`（`LocalFile::complete` 在 `:3202`）；私有的 `own_body`（`:1762`）照 P5 ④，`!h2` 的 `HEAD` 直接返回、其余按 `size_hint().exact().unwrap_or(0)` 数。`own_body` 在结束记录的那一句之前的十处（`own_body` 行 / `finish` 行）：HTTP/2 上 `CONNECT` 的 501 `:442` / `:443`、URL Rewrite 的 `reject` 有页 `:669` / `:670`、`bad_request` ①（`absolute_form` / `origin_form` 失败）`:978` / `:979`、②（第一次发的 `fit` 失败）`:1003` / `:1004`、③（`retry` 里重发的 `fit` 失败）`:2027` / `:2028`、`map_local` 的 `(None, data)` `:1533` / `:1534`、`answer` 的 `TinyGif` `:1653` / `:1654`、`answer` 的 `Reject` / `NoDrop` 有页 `:1669` / `:1670`、`failed` 有页 `:1709` / `:1710`、`timed_out` 有页 `:1729` / `:1730`。换次序的是 `TinyGif`、501、三处 `bad_request` 与 `map_local` 的 `(None, data)`（今日先 `finish` 再建应答）；URL Rewrite 的 `302` / `307`、`REJECT-DROP`、没有页的拒绝与失败、监听器的两个 400 照 P5 ④ 不调 | `b4ae895` |
| 13 | 2 | 任务 2 的用例与门禁：新加 10 条（`the_request_body_sent_upstream_is_counted_before_the_record_ends`、`a_one_frame_answer_is_counted_before_the_record_ends`、`a_chunked_answer_counts_every_frame`、`rurges_own_answers_count_their_length`、`map_local_answers_count_their_length`、`a_head_answer_counts_no_body_on_http1`、`an_h2_connect_501_counts_its_page`、`an_upgrade_counts_no_body`、`a_client_that_leaves_mid_body_counts_what_was_handed_over`、`an_h2_client_counts_its_bodies_too`），给 `a_late_response_head_is_a_504`、`a_missing_file_answers_502`、`a_file_that_is_a_directory_answers_502` 各加一句 `body_at_finish(..).1` 等于客户端读到的正文长度的断言（用例名与别的断言不改）。门禁（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0；`cargo test -j 8 --workspace --no-fail-fast` 的日志里 92 个结果行、5294 通过、0 失败、15 忽略，少的那一个结果行是 `rurge-proto-ssh` 的 lib 测试二进制以 `STATUS_HEAP_CORRUPTION` 退出（这台开发机的已知偶发，见 `CLAUDE.md`），单独重跑它 31 条全过（`task-2-ssh-rerun.log`），合起来 93 个结果行、5325 通过、0 失败、15 忽略（基线 5315 加本任务 10）；日志在工作区的 `task-2-fmt.log`、`task-2-clippy.log`、`task-2-test.log`。新加与加断言的那 13 条各自单独跑过 20 次（`task-2-loop.log`），全过 | `b4ae895` |
| 14 | 2 | 协调者的裁定（派工第 4 条，对 P5 ⑤ 的改动）：`body_at_finish` 先 `finished(n)`、再有时限地等这个句柄的 id 出现在 `bodies` 里（5 秒、每 5 毫秒看一次，同 `finished` 的写法），到时限 panic 的信息是 "the handle finished without its hook"。理由是 `finish` 先置 `finished`、再跑钩子，多线程的运行时里 `finished(n)` 可能在钩子跑之前就返回，当场取会偶发 panic | `b4ae895` |
| 15 | 2 | 两处出入，执行者的裁定：① brief 列了九个用例名，派工第 7 条要的是 10 条，P5 ④ 的 HTTP/2 `CONNECT` 501 单写成 `an_h2_connect_501_counts_its_page`（它要 `rig_h2`，与 `rurges_own_answers_count_their_length` 的 HTTP/1.1 那一组不同一条连接），这是第 10 条；② `a_chunked_answer_counts_every_frame` 的正文给 40 000 字节：`chunked_origin` 只答一个块，正文短于一帧时"每一帧都数"测不到，40 000 字节让 hyper 交出多于一帧，不新写夹具 | `b4ae895` |
| 16 | 2 | 执行者的裁定：`a_head_answer_counts_no_body_on_http1` 不写死页长，只断言这条 `HEAD` 应答的 `content-length` 是一个非 0 的值（实跑是 603）、客户端没有读到正文、下一条应答的头紧接着来。写死的 `Some("0")` 是错的：hyper 1.11.1 的 HTTP/1.1 服务端给 `HEAD` 的头写上正文的长度、只把正文丢下（`hyper-1.11.1/src/proto/h1/role.rs`） | `b4ae895` |
| 17 | 2 | 执行者的裁定（P5 ④ 的 `map_local` 写法）：`match (file, data)` 交回 `(ResponseBody, Option<Guard>)`——文件那一支把 `guard` 交给 `LocalFile::new`、交回 `None`，`(None, data)` 那一支建 `responses::full` / `responses::empty()`、交回 `Some(guard)`；建好 `response`（状态与头）之后 `if let Some(guard) = ending { own_body(guard.handle(), &response, self.shared.conn.h2); guard.handle().finish(SessionOutcome::Completed); drop(guard); }`，再 `Ok(response)` | `b4ae895` |
| 18 | 2 | 三处 `bad_request` 没有能触发它们的用例：唯一见到 400 的 `a_connection_that_reaches_no_route_says_why` 是 hyper 自己的解析错误，那时还没有句柄；照 brief 记在这里，不新写夹具，三处照样换了次序（用例里没有覆盖，登记） | `b4ae895` |
| 19 | 2 | 自查的变红验证（改一处代码看用例变红，都在回环与内存里，改完立刻改回）：① 把 `own_body` 的体换成空操作，`rurges_own_answers_count_their_length`、`map_local_answers_count_their_length`、`an_h2_connect_501_counts_its_page`、`a_late_response_head_is_a_504`、`a_missing_file_answers_502` 红（`an_h2_client_counts_its_bodies_too` 照过，它走 `Tail`）；② 把 `Tail` 与 `LocalFile` 的两句 `add_response_body` 停掉，`a_one_frame_answer_is_counted_before_the_record_ends`、`a_chunked_answer_counts_every_frame`、`map_local_answers_count_their_length` 红。改回之后 10 条新用例全绿 | `b4ae895` |
| 20 | 2 | 修正轮 1（协调者的评审）：只改了一处用例的断言——`rurges_own_answers_count_their_length` 第一段循环（`/fail`、`/reject`、`/gif`）原来只断言 `head.starts_with("HTTP/1.1 ")`，三条路径各走到了哪一支用例不说（`/gif` 若因别的原因得到一个 403 页，`TinyGif` 那一处 `own_body` 就没有被测到，用例照样绿）。改成每个路径带它应得的状态（`/fail` → `502`、`/reject` → `403`、`/gif` → `200`）并断言正文不是空的（空的话 `0 == 0` 也会过）；`body_at_finish` 的两句断言不变，别的用例、别的断言与生产代码都没动。跑过（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0，`cargo test -j 8 --workspace --lib -- engine::tests::rurges_own_answers_count_their_length` 通过，直接跑 `rurge_http` 的测试二进制、`--exact engine::tests::rurges_own_answers_count_their_length` 循环 20 次全过（日志在工作区的 `gate-task-2fix1-fmt.log`、`gate-task-2fix1-clippy.log`、`task-2fix1-loop.log`）；只改了一条用例的断言，没有跑全量门禁（任务 3 的全量门禁会包含它） | 本提交 |
| 21 | 2 | 协调者核过任务 2（`b4ae895` 与 `f22c72e`，第 12 ～ 19 行；修正轮 1 `3470e06`，第 20 行）：门禁的数（日志里 92 个结果行、5294 通过、0 失败、15 忽略，少的那一个结果行是 `rurge-proto-ssh` 的 lib 测试二进制以 `STATUS_HEAP_CORRUPTION` 退出，单独重跑 31 条全过，合起来 93 / 5325 / 0 / 15；一台机器上的一次运行，不是 CI；日志里 `cannot bind http-api … (os error 10048)` 那一行是现有用例的预期输出）、`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0、计划的两张表只多不少（修正记录多第 12 ～ 20 行，延后事项没有动）、弯引号 0、带本机账户名的文件 26，都对得上；`b4ae895` 只改了 `crates/rurge-http/src/engine.rs`。`own_body` 的十处逐行核过，每一处都在结束记录的 `finish` 的前一行（第 12 行的十对行号都对）；`Tail` 数在 `:3434`、`if ended {` 在 `:3438`，`LocalFile` 数在 `:3294`、`if this.left == 0` 在 `:3295`。生产代码里别的 `finish` 也逐处看过：URL Rewrite 的 `302` / `307`（`:656`，正文空）、没有页的拒绝与失败、请求正文的几种失败（`Err(Close)`）、升级（隧道的字节在 `up` / `down`；升级时 hyper 预读、交给客户端的字节也不是正文）、`Tail` 与 `LocalFile` 自己的结束，都不该数，没有漏的。`own_body` 的 `HEAD` 一支只对 rurge 自己的页起作用：Map Local 对 `HEAD`、`204`、`304` 本来就不带数据（`map_local` 里的 `head` 与 `bodyless`），数到 0。与计划的两处出入接受：`Idle::new` 的 `handle` 参数排在 `tap` 之后、不是 P5 ① 写的"`body` 之后"（写计划时漏看了 M4a 加在 `body` 之后的 `tap`，排在最后意思不变）；派工单第 7 条的"十条"是协调者把 brief 的九个用例名数成了十个，第 15 行 ① 把 HTTP/2 上 `CONNECT` 的 501 单写成第十条。`rurges_own_answers_count_their_length` 的第一段循环原来只断言 `HTTP/1.1 `，三条路径各走到哪一支用例不说，修正轮 1 改成每条路径断言它的状态（502、403、200）与正文不空（第 20 行；只改断言，跑了 fmt、clippy 与那一条的二十次循环，没有跑全量门禁，任务 3 的门禁会包含它）。三处 `bad_request` 没有用例（第 18 行）、HTTP/2 上 `HEAD` 数页长没有用例（P5 ③），照登记。**任务 2 过了** | 本提交 |
| 22 | 3 | 任务 3 的生产代码：`crates/rurge-http/src/mitm/decrypt.rs` 的 `Decision` 变成五种（`Pass` 删掉，`git grep -n 'Decision::Pass' -- crates/` 为空），`decides` 的次序改成名单 → 来源 → 名字 → ALPN；`crates/rurge-engine/src/serve.rs` 的九句备注是私有常量（`RAW_HOSTS` `:38`、`RAW_KEYWORDS` `:40`、`NEITHER` `:43`、`TIMED_OUT` `:45`、`MITM_OFF` `:47`、`NO_CA` `:49`、`NO_PROTOCOL` `:51`、`SOURCE` `:53`、`NO_CERTIFICATE` `:56`），`raw_listed` 在 `:63`，`sniffable_session` 的两段换成调它、两段的注释留在调用处，`decrypting` 在 `:328` 交回 `Result<(Arc<Mitm>, HostName), Option<&'static str>>`（没有名字 `:333` 与名单不命中 `:355` → `Err(None)`，开关 `:358` → `MITM_OFF`，没有 CA `:364` → `NO_CA`，`Decrypt` `:373`、`NotListed` `:376`、`SourceBlocked` `:377`、`NoCertificate` `:378`、ALPN `:389` → `NO_PROTOCOL`），两行 DEBUG 的文字一字不改（只是没有 CA 那一行的缩进少了一层）；`crates/rurge-engine/src/engine.rs` 的 `dial_noted` 在 `:2017`、写备注的是 `:2052` 的 `handle.add_note(note)`（在 `match turned` 之后、`quic_adaptive.on_tcp` 之前），`judged_at_the_entry` 在 `:2071`（`Transport::Tcp`、监听器是 `Http` / `Socks5` / `Tun`、引擎没有暂停），`PassedBy` 在 `:2084`，`Dialer::dial` 只剩 `Box::pin(self.dial_noted(session, PassedBy::Lists))`。没有新的日志行 | `51e5886` |
| 23 | 3 | 任务 3 的用例：`tests/sniff.rs` 九条（`always_raw_tcp_hosts_and_keywords_are_noted`、`a_listed_raw_host_on_a_port_that_is_never_sniffed_is_noted_still`、`a_listed_raw_host_that_the_rules_reject_keeps_its_note`、`neither_http_nor_tls_is_noted`、`a_sniff_that_waits_out_its_time_is_noted`、`a_client_that_closes_during_the_wait_has_no_note`、`a_paused_engine_writes_no_reason`、`pre_matching_writes_no_reason_unless_a_list_came_first`、`an_unlisted_tls_session_has_no_note`）、`tests/mitm.rs` 六条（`a_listed_session_with_mitm_off_is_noted`、`a_listed_session_without_a_usable_ca_is_noted`、`a_listed_session_from_a_source_not_allowed_is_noted`、`a_listed_session_whose_alpn_rurge_does_not_decrypt_is_noted`、`a_listed_name_no_certificate_can_be_made_for_is_noted`、`a_decrypted_session_has_no_reason`），外加两个文件各一个 `reasons` 与 `assert_reason`（断言备注里恰好是那一句，失败信息带整条记录）。门禁（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0；`cargo test -j 8 --workspace --no-fail-fast` 的日志里 93 个结果行、5340 通过、0 失败、15 忽略（任务 2 之后的 93 / 5325 / 0 / 15 加本任务 15 条），没有重跑；日志在工作区的 `task-3-fmt.log`、`task-3-clippy.log`、`task-3-test.log`。`decrypt.rs` 的 `decides_lists_first_then_source_name_and_alpn` 先红（换成新断言、`decides` 还是旧次序时失败在 `left: Pass, right: SourceBlocked`，`task-3-red-decrypt.log`）后绿；15 条新的集成用例各单独跑过 20 次（`task-3-loop.log`），全过 | `30eeed7` |
| 24 | 3 | 协调者的裁定（派工第 3 条，对 brief 的改动）：`a_listed_raw_host_on_a_port_that_is_never_sniffed_is_noted_still` 的端口 7 换成 8443（`which_sessions_are_sniffable` 里不嗅探的那一个端口），两个名字都经 `FakeSocks5`：本机回环的 7 号端口开着（Windows 的 Simple TCP/IP Services），CI 上关着，写死的端口不能既当"开着"又当"关着" | `30eeed7` |
| 25 | 3 | 协调者的裁定（派工第 3 条，对 brief 的改动）：brief 的 `rawkey.raw.test` 不能两张都命中——`always-raw-tcp-hosts` 的 `raw.test` 经 `Glob::matches` 只认它自己，不覆盖子域。用例写 `always-raw-tcp-hosts = raw.test, both-rawkey.test`、`always-raw-tcp-keywords = rawkey`：`raw.test` 只命中名单、`a-rawkey.test` 只命中关键字、`both-rawkey.test` 两张都命中并写名单那一句（次序在前）。另：这张名单解析时没有缺省端口（`general.rs` 的 `parse_host_list(.., None)`），不写端口的 `raw.test` 在每个端口上都命中 | `30eeed7` |
| 26 | 3 | 五处出入，执行者的裁定：① `neither_http_nor_tls_is_noted` 的"强制进引擎"那一半要写 `force-http-engine-hosts = forced.test:443`：不写端口是 80（`force_http_engine_hosts_sniff_another_port` 钉住的），443 上的会话根本不受它影响，`forced` 就不成立；② 手写的 ClientHello 字节要说两遍 `\r\n`（`SSH-2.0-test\r\n\r\n`）：只给一个 CRLF 时 hyper 还等着请求头的空白行，连接不会在时限内结束；③ `tests/mitm.rs` 里已有一个 `fn client_hello(config, name)`，本任务另写的那一个改名 `hand_written_hello`；④ `a_listed_session_whose_alpn_rurge_does_not_decrypt_is_noted` 只看"握手失败"、不看失败的原因：`spdy/3` 的客户端拿到的是 `NoApplicationProtocol`（源站也不支持 ALPN 里的这个协议），不是 `UnknownIssuer`；⑤ `a_listed_raw_host_that_the_rules_reject_keeps_its_note` 的 SOCKS5 回码是 `0x02`（not allowed by ruleset），不是 `0x05`：名单命中的会话拨号在应答之前，拒绝就是拒绝 | `30eeed7` |
| 27 | 3 | 自查的变红验证（改一处代码看用例变红，都在回环与内存里，改完立刻改回）：把 `dial_noted` 里 `handle.add_note(note)` 那一条注释掉，`tests/sniff.rs` 的 6 条（期望非空备注的）与 `tests/mitm.rs` 的 5 条都红在 `notes: []`（期望"没有原因"的四条照过，这正是应当的），`task-3-red-notes.log`；改回之后两个文件 30 / 43 条全绿（`task-3-green-notes.log`）。`tests/*.rs` 拿到不九个常量（它们是 `serve.rs` 的私有常量），用例里照"表后的原文"P6 写字面量 | `30eeed7` |
| 28 | 3 | 修正轮 1（协调者的评审）：两处。① `crates/rurge-engine/tests/sniff.rs` 的 `a_sniff_that_waits_out_its_time_is_noted` 原来用 `tokio::time::sleep(Duration::from_millis(400))` 等嗅探的 300 ms 超时与随后的拨号，这是 `CLAUDE.md`「测试约定」的「等待」只允许的固定等待之外的一种（那一处等的是要发生的事，不是"什么都没发生"），而且机器忙时计时器可能晚于 400 ms 才被处理，客户端先关掉就变成 `Sniffed::Closed`、不写备注，用例偶发地红；改成有时限的 `wait_until`（5 秒，`tests/common/mod.rs`），等的是 `request_log().active()` 里出现 `dst == "quiet.test:443"` 的那一条（句柄与备注在 `dial_noted` 里建，嗅探结束之前没有句柄），注释照实写它等的是什么，用例其余部分与断言不变；② `crates/rurge-http/src/mitm/decrypt.rs` 里 `decides` 与 `decides_lists_first_then_source_name_and_alpn` 两段文档注释把"名单先看"的次序引成了 M2 设计第 7.1 节，而那一节的次序是开关 → CA → 来源 → 名单 → ALPN（`docs/superpowers/specs/2026-10-30-phase4-m2-mitm-design.md:236`～`244`）：次序改成引 `phase 4 M4 design 10.2` 与 the phase 4 / M4b plan's P6，条件仍归 M2 设计第 7.1 节，两段注释其余的话不动。跑过（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0，`cargo test -j 8 --workspace --lib -- decrypt::tests` 通过（`rurge-http` 的 lib 14 条），直接跑 `sniff` 的测试二进制、`--exact a_sniff_that_waits_out_its_time_is_noted` 循环 20 次全过；只改了一条用例的等法与两段注释，没有跑全量门禁（任务 4 的全量门禁会包含它）。日志在工作区的 `task-3fix1-fmt.log`、`task-3fix1-clippy.log`、`task-3fix1-decrypt.log`、`task-3fix1-loop.log` | 本提交 |
| 29 | 3 | 协调者核过任务 3（`51e5886`、`30eeed7` 与 `40f31be`，第 22 ～ 27 行；修正轮 1 `0af3451`，第 28 行）：门禁的数（93 / 5340 / 0 / 15，一台机器上的一次运行，不是 CI；没有崩溃与重跑；日志里 `cannot bind http-api … (os error 10048)` 那一行是现有用例的预期输出）、`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0、计划的两张表只多不少（修正记录多第 22 ～ 28 行，延后事项没有动）、弯引号 0、带本机账户名的文件 26，都对得上；`51e5886` 只改了 `decrypt.rs`、`serve.rs` 与 `engine.rs`，`30eeed7` 只改了 `tests/sniff.rs` 与 `tests/mitm.rs`。逐项核过：九句备注与 P6「表后的原文」逐字相同；`git grep -n 'Decision::Pass' -- crates/` 为空；`decides` 的次序是名单 → 来源 → 名字 → ALPN，`decrypting` 的是名单 → 开关 → CA → 来源 → 名字 → ALPN，两行 DEBUG 的文字不变、写出的条件也与改之前相同（没有 CA 的那一行仍只在名单命中而开关开着时写，ALPN 那一行仍要前面四个条件都成立）；`serve_sniffed` 里 `Other` 不被强制时写、`Closed` 不写、`Timeout` 写；`dial_noted` 在 `turn_back` 的结局之后、拨号之前写一句，`judged_at_the_entry` 与 `sniffable_session` 在两张名单之前的几步对得上（TCP 且是三个客户端入口之一、引擎没有暂停；fake 地址背后没有名字的由 `Turned::Unknown` 的早退管住），所以被规则或预匹配拒绝的照样带着、暂停的不写。`decrypting` 里 `NotListed` 那一支到不了、留着让 `match` 穷尽（报告的顾虑 1，接受）。十五条用例逐条读过：会话都经回环上的 `FakeSocks5`，每个名字一行 `[Host]`；变红验证（注释掉 `add_note`，11 条红在 `notes: []`）证明它们测到了写备注的那一句；ALPN 那一条只断言握手失败（顾虑 2，接受：要测的是备注）。修正轮 1 改了两处：`a_sniff_that_waits_out_its_time_is_noted` 原来用固定的 400 ms 等嗅探超时与拨号——`CLAUDE.md`「测试约定」只允许"断言什么都没发生"的固定等待，机器忙时客户端可能先关、嗅探判成 `Closed`、用例偶发地红——改成有时限的 `wait_until`，等进行中的记录里出现这条会话；`decides` 与它的用例的文档注释把"名单先看"的次序引成 M2 设计第 7.1 节，那一节的次序是开关 → CA → 来源 → 名单 → ALPN，改成次序引 M4 设计第 10.2 节与 P6（第 28 行；只改了一条用例的等法与两段注释，跑了 fmt、clippy、`decrypt::tests` 与那一条的二十次循环，没有跑全量门禁，任务 4 的门禁会包含它）。三处文字的出入照实记在这里，不改原行：`30eeed7` 的标题"十五种直通原因都有一条用例"说的是十五条用例、九种原因；第 26 行 ② 的"手写的 ClientHello 字节"指 `neither_http_nor_tls_is_noted` 后一半发的 SSH 开场白 `SSH-2.0-test\r\n\r\n`，不是 ClientHello；第 27 行的"拿到不九个常量"是"拿不到"。**任务 3 过了** | 本提交 |
| 30 | 4 | `crates/rurge-dns/src/resolver.rs` 的新用例不能用 `resolver_in_memory` 的夹具（执行者的裁定）：那条路的 `fast()` 扇出是 100 ms × 3，300 ms 就放弃，`mock.set_delay(30 s)` 的查找在 1 秒时已经结束，`trail.dns()` 是 `Some(300ms)`（第一次跑就是这样红的）。用例改成 `resolver_with_fanout(&e, Arc::new(StaticSystemDns::default()), FanoutOpts::default())`（1 s × 5 = 5 s），再 `r.primary.store(Arc::new(vec![mock.upstream()]))` 换成件的上游，其余照 `a_lookup_cut_short_is_noted_for_the_time_it_took`（`MockDns::spawn`、`mock.set`、`env(&profile(..))`）；1 秒时 `1 s <= d < 30 s` 的断言不变 | `3c6ccdb` |
| 31 | 4 | `crates/rurge-engine/tests/pipeline.rs` 的 `an_active_session_shows_the_time_its_lookup_has_taken_so_far` 不给 `slow.test` 写 `[Host]`（协调者的裁定，派工单第 5 节）：钉住了就不问 `MockDns`，用例测不到查找。第二道防线是 `IP-CIDR,127.0.0.0/8,DIRECT` 那条按地址的规则与回环上的 `MockDns`（它把 `slow.test` 答成 `127.0.0.1`），同 `sessions_dialling_together_record_their_own_address_and_lookup_time` 的做法 | `448a4e2` |
| 32 | 4 | 自查的变红验证（改一处代码看用例变红，在回环与内存里，改完立刻改回）：在 `Trail::dns()` 里临时加一句 `return left.dns;`（只算已经结束的查找），`pipeline` 的 `an_active_session_shows_the_time_its_lookup_has_taken_so_far` 变红——`timed out waiting for the lookup to count up`（`tests/common/mod.rs:437`，13.07 s），因为第一次等到的是规则那次查找结束之后的 3000 ms、之后再不动；改回之后绿。这一改之后没有单独再跑 `trail::tests` 与 `resolver` 的用例 | `793f371` |
| 33 | 4 | `Lookup::begin()` 的号从 0 起、`next` 用 `wrapping_add(1)`（P7 只写 `next += 1`）：号不进任何记录、也不进 `Debug`，只在 `under_way` 里找到自己那一项，与时间的相加都用 `saturating_*` | `793f371` |
| 34 | 4 | 门禁（一台机器上的一次运行，不是 CI）：第一次全量 **92 个结果行、5268 通过、0 失败、15 忽略，退出码 101**——`-p rurge-engine --test pipeline` 的二进制中途死掉、没有结果行，那 79 条一条也没有计入；单独重跑那个二进制 79 通过、0 失败，之后直接跑那个二进制两遍，各 79 通过、0 失败。合计 **93 个结果行、5347 通过、0 失败、15 忽略**，正好是任务 3 的 93 / 5340 / 0 / 15 加本任务新加的 7 条；`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0。日志：`gate-task-4-fmt.log`、`gate-task-4-clippy.log`、`gate-task-4-test.log`、`task-4-pipeline-rerun.log` | 本提交 |
| 35 | 4 | 协调者核过任务 4（`793f371`、`3c6ccdb`、`448a4e2` 与 `9457f8a`，第 30 ～ 34 行）：门禁的数（93 / 5347 / 0 / 15，一台机器上的一次运行，不是 CI）、`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0、计划的两张表只多不少（修正记录多第 30 ～ 34 行，延后事项没有动）、弯引号 0、带本机账户名的文件 26，都对得上；四个提交只改了 `trail.rs`、`resolver.rs`、`observe.rs`（只有 `dns_ms` 的文档注释）、`tests/pipeline.rs` 与计划，`Cargo.toml` 与 `Cargo.lock` 没有动。逐项核过：`Lookup` 照 P7 ①（不派生 `Clone`、不实现 `Debug`；`begin` 取 `Instant::now()` 与 `current()`，有 trail 才取号、登记；`Drop` 按号去掉那一项、`saturating_add` 加上花的时间；没有 trail 的什么也不做），`dns()` 照 P7 ②（`now` 只取一次，没有结束的也没有在途的才是 `None`），`current()` 是 `CURRENT.try_with(Trail::clone).ok()`，锁都不跨 `await`；`resolver.rs` 在原处（地址字面量的早退之后）换成 `let _lookup = rurge_net::trail::Lookup::begin();`，`git grep -n 'Noted' -- crates/rurge-dns` 为空；P7 的六段文档注释（模块注释只换了第一句，其余几段的文字不变）与 P4 的 `dns_ms` 一段，去掉 `///`、`//!` 与折行之后与「表后的原文」逐字相同，`set_peer`、`clear_peer`、`follow`、`looked_up` 的注释没有动。七条新用例逐条读过：`trail.rs` 的五条与 `resolver.rs` 的一条跑在暂停的时钟上，`current_is_the_trail_followed_now` 不碰时钟、用普通的 `#[tokio::test]`（派工单写的是五条都 `start_paused`，接受）；`pipeline.rs` 那一条的两次等待都有 10 秒的时限、没有固定等待，第二次"比上一次读到的大"正是分得出两种口径的那一句（第 32 行的变红验证；照读代码，`trail.rs` 的新用例在 `return left.dns;` 下也会红，因为它们断言在途时就是 `Some`）。第 30 行（执行者的裁定）接受，理由核过：`resolver()` 经 `resolver_with` 用 `fast()`（100 ms × 3），缺省的 `FanoutOpts` 是 1 s × 5（`fanout.rs:29`），内存里的应答照样等 `set_delay`（`testing.rs:497` ～ `504`）；第 33 行接受。门禁的经过与第 34 行、报告的顾虑 1 有两处出入，照实记在这里，不改原行：其一，Pi 跑过两次全量：第一次（04:13 ～ 04:26）是 93 / 5347 / 0 / 15、退出码 0，协调者在它被第二次覆盖之前读过那一份日志，那时的树与最后的树只差 `trail.rs` 模块注释第一句的折行（只是注释）；第 34 行说的"第一次全量"是第二次，跑的是最后的树。其二，第二次里 `pipeline` 的二进制死掉的退出码在日志里写着：`gate-task-4-test.log` 第 2473 行 `exit code: 0xc0000374, STATUS_HEAP_CORRUPTION`，就是 `CLAUDE.md`「这台开发机」说的那一种偶发，顾虑 1 说的"没有拿到它的退出码"不对；重跑与直接再跑两遍都是 79 / 0。另两处文字：第 30 行的"换成件的上游"是"换成内存里的上游（`MockDns::upstream`）"；`9457f8a` 的标题"M4b 任务的 dnsMs 注释"少了任务号（任务 4）。计划在工作区的副本是 CRLF 与 LF 混着（原有的行是 CRLF、新加的五行是 LF），提交进去的是纯 LF（`core.autocrlf` 在加入时换掉了），内容没有出入；协调者把工作区的副本换回提交里的那一份（LF）。**任务 4 过了** | 本提交 |
| 36 | 5 | 两处 `clear_peer`（执行者的裁定）：一条开的载体没开成、因此谁也没走的 UDP 流（`crates/rurge-engine/src/udp.rs` 的 `run_flow` 的 `Err(outcome)` 那一支）与一个载体没开成的内部 UDP 会话（`crates/rurge-engine/src/engine.rs` 里 `open_udp` 失败之后、`finish` 之前），都要把 trail 上留着的那句清掉——载体的拨号所经的第一跳（socks5 的控制连接）不是这条会话的远端。来由：P15 没有点名的 `crates/rurge-engine/tests/pipeline.rs` 的 `a_doq_carrier_the_proxy_will_not_open_fails_naming_the_policy` 变红了（它断言 `remote_addr` 是 `None`，而当时读到的是代理的地址），照设计第 10.3 节，没走成的是 `None`；P8 ③ 写的"被拒绝的、在 `resolve` 之前失败的流到不了这里，照旧 `None`"这句话原来不成立 | 本提交 |
| 37 | 5 | 执行者的裁定：`crates/rurge-engine/tests/udp_tls_family.rs` 的共用夹具 `two_echoes_through` 改成交回 `Harness`（它原来在里面 `drop` 掉），两个用例才读得到那些记录去断言远端；用例名与别的断言没有改 | 本提交 |
| 38 | 5 | 执行者的裁定：`crates/rurge-net/src/packet_datagram.rs` 的 `a_packet_datagram_far_end_is_its_carriers` 用 `#[tokio::test]`（`packet_datagram` 起两个任务，`#[test]` 下报"there is no reactor running"）；派工单没有写这一层 | 本提交 |
| 39 | 5 | external 的 UDP 只断言到"回环上、不是目标"（协调者的裁定，派工单的提示 3）：`tests/external/tests/outbound.rs` 的 `udp_goes_through_the_program` 在 `open_udp` 之后断言 `far_end(&target(echo_addr))` 是 `Some`、`ip()` 是回环、不等于回声的地址——中继的端口是那个程序自己绑的，用例不知道 | 本提交 |
| 40 | 5 | 门禁（一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0；全量 **93 个结果行、5354 通过、0 失败、15 忽略**，退出码 0（任务 4 的 5347 加本任务的 7 条新用例）；循环：本任务改了断言或新写的 12 条用例各自单独跑 20 次，240 次全过。构建期见过两次与改动无关的机器故障：第一次 `cargo build --workspace --all-targets` 里 rustc 以 `0xc0000409`（`STATUS_STACK_BUFFER_OVERRUN`）退出、`link.exe` 报 `LNK1102: out of memory`，换 `-j 4` 之后通过；另一次 `LNK1104`（测试二进制的文件被占着），等 6 秒重跑即过 | 本提交 |
| 41 | 5 | 协调者核过任务 5（`95a4abc`、`e85d60d`、`4323335`、`73e3552`、`1e6cc8a` 与 `d129367`，第 36 ～ 40 行）。所有者让先合并到 `main`：本地 `main` 在这次评审之前就快进到了 `d129367`，所以 `main` 里的任务 5 是评审之前合进去的。门禁：执行者的两份全量日志协调者数过，都是 93 / 5354 / 0 / 15、退出码 0、没有死掉的测试二进制（一台机器上的一次运行，不是 CI），`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0；计划的两张表只多不少（修正记录多第 36 ～ 40 行，延后事项没有动），弯引号 0，计划的行尾 `i/lf w/lf`，`git diff --check` 干净，带本机账户名的文件 26，`Cargo.toml` 与 `Cargo.lock` 没有动。逐项核过 P8：① 两个 trait 的缺省方法，生产代码里 `impl … Datagram for` 的五处都转交或答了；② 的表逐项（`DirectPacket`、`SsUdp`、`Socks5Udp`、`StreamUdp`、`SnellUdp`），任务 7 的与写着缺省的没有动，`git grep -n 'StreamUdp::new\|SnellUdp::open' -- crates` 的七处都是四个 / 两个参数；③ `note_far_end` 紧跟在 `open` 的 `mark_connected()` 之后；④ `wrap_internal_udp` 在包两层之前写或清；⑤ `record_of` 读 `h.trail().peer()`；⑥ `relays()` 的文档注释与 P8 ⑥ 逐字相同；`remote_addr` 一段去掉 `///` 与折行之后与「表后的原文」P4 逐字相同（草稿区的脚本）。第 36 行（执行者的裁定）接受，理由核过：`run_flow` 的 `Err` 那一支只在假地址查不到、打开途中被取消与 `open` 失败（规则拒绝、查不到、载体没开成、`resolve` 失败）时走到，开成的流走 `Ok` 那一支，`open` 在 `note_far_end` 之后没有会出错返回的地方，所以这一句抹不掉开成的流的远端；`engine.rs` 那一处只在内部会话的 `open_udp` 失败时走到。这与 P8 ③ 的原则（socks5 的控制连接不是这条流的远端）一致。与 TCP 的不同照实记：`dial_route` 在每次尝试之前清、最后一次失败之后不清，所以经代理、在第一跳连上之后失败的 TCP 会话留着第一跳的地址（`remote_addr` 的注释说的"离开 rurge 时所经的"那条连接），没开成载体的 UDP 流与内部 UDP 会话是 `None`（数据没有走过任何一跳）；任务 9a 写 `remoteAddr` 的文档时要说这一句。第 37、38 行接受，第 39 行是协调者的裁定。顾虑 2 接受：`udp_through` 的四个用例都经同一个服务端的 UDP 端口，约束更强，各循环二十次全过。两处没有记进修正记录的出入，照实记在这里，都接受：其一，`a_direct_flow_records_where_its_datagrams_go` 按名字的那条流没有照 brief 写 `[Host]`，用的是共用夹具的 `MockDns`（`target.test` 答 `127.0.0.1`，同一个文件里现成那条（`tests/udp.rs:208`）也是这样写），Profile 里没有写主机名，两种写法都只在回环上；其二，brief 写"用例（先红）"，而派工单第 4 节要先做完生产代码，用例写出来就是绿的，执行者改代码看用例变红补了验证（`task-5-red-record.log` 五条红、`task-5-red-carriers.log` 两条红、`cell` 那条单独跑红），协调者读过这两份日志。第 40 行只写了一次全量的数，执行者其实跑了两次：第一次的树与 `1e6cc8a` 相同（第 36 ～ 40 行是之后加的），第二次的树是 `d129367`，两次都是 93 / 5354 / 0 / 15、退出码 0（报告里两次都写了）。一处与本任务无关的改动：`crates/rurge-engine/src/udp.rs` 的测试模块里 `struct Stuck;` 与 `impl PacketSocket for Stuck` 之间的空行被删了（`73e3552`），报告没有提（报告自查第 5 条说的是另一处、没有进提交的）；不影响编译与行为，交给任务 6 的派工单，单独一个提交补回。协调者自己跑的：本任务新写与加了断言的十二条端到端用例，加上第 36 行让它转绿的 `a_doq_carrier_the_proxy_will_not_open_fails_naming_the_policy`，各直接跑测试二进制、`--exact`、二十次（每次限时 120 秒），260 次全过；新写的六条单元用例跑一次全过；新增的用例代码里没有固定等待。**任务 5 过了** | 本提交 |
| 42 | 6 | 任务 5 误删了 `crates/rurge-engine/src/udp.rs` 的测试模块里 `struct Stuck;` 与 `impl PacketSocket for Stuck` 之间的空行（修正记录第 39 行提到）：任务 6 开头单独一个提交补回这一行，别的一个字不动，`cargo fmt --all --check` 照过。 | `43fcb45` |
| 43 | 6 | 执行者的裁定：`a_redialled_snell_session_records_the_new_connection` 照 brief 用一行 `[Host]` 把 `snell.test` 钉到 `127.0.0.1` 不通——`[Host]` 从不作用于代理服务器自己的名字（`crates/rurge-config/src/config.rs` 的 `proxy_hostnames`），`lookup_inner` 在它之前就 `break`，会话因此解析失败、入口答 502。改用共用夹具的 `MockDns`（`h.dns.set("snell.test", …)`）答这个名字：`lookup_inner` 照样开一张 `Lookup` 票据，`dns_ms` 仍是 `Some`，brief 要求的断言没有改。 | `8a11377` |
| 44 | 6 | snell：`Dialer::fresh` 交回 `(SnellStream, Option<SocketAddr>)`（读在 `stack.open` 之后、`SnellStream::open` 之前，`mod.rs:68`）；`Pool.idle` 是 `Mutex<Vec<(SnellStream, Option<SocketAddr>, Instant)>>`，`put(stream, peer)` 与 `take() -> Option<(SnellStream, Option<SocketAddr>)>`；`SnellTunnel` 多 `peer` 字段，`new(stream, peer, head, pool)` 与 `reused(stream, peer, head, pool, dialer, opts)`，`Drop` 与 `finish(stream, peer, pool)` 把它随连接放回池；`SnellOutbound::open` 从池里取出时先 `connected`、再 `reused`（`mod.rs:163`）；`open_udp` 用 `fresh` 交回的远端，删掉任务 5 的 `current()` 那一句。失效重拨：`Retry` 多 `trail: Option<Trail>`（`reused` 里取 `current()`），`fail()` 的重拨 future 有 trail 时先 `clear_peer()`、再在 `trail.follow(..)` 里跑原来的 `timeout(opts.timeout, dialer.fresh(&opts))`，交回 `(stream, peer)`，`poll_open` 把 `self.peer` 换成新的（`tunnel.rs:247` ～ `:279`、`:296`）。单元用例 `a_redial_notes_the_new_connection_on_the_sessions_trail`；`crates/rurge-proto` 里建 `SnellTunnel`、调 `Pool::put` 的用例只补 `None`。 | `b7f9a53` |
| 45 | 6 | 经引擎的 snell 用例三条：`a_pooled_snell_connection_gives_its_record_the_servers_address`、`a_pooled_snell_connection_through_a_socks5_proxy_records_the_first_hop`、`a_redialled_snell_session_records_the_new_connection`（第二条记录 `remote_addr` 是服务端、`dns_ms` 是 `Some`、`fake.connections()` 是 2）。 | `8a11377` |
| 46 | 6 | anytls：`Session` 多 `peer: Option<SocketAddr>`（`start` 里是 `None`）、`with_peer(self, peer)` 与 `peer(&self)`；`new_session` 在 `stack.open` 之后读远端、`.with_peer(peer)`（`mod.rs:107`）；`open` 从池里取出的会话在 `open` 成功之后 `connected`（`mod.rs:139`）。用例 `a_pooled_anytls_session_gives_its_record_the_servers_address`；RED 是第二条记录的 `remote_addr` 为 `None`。 | `ca87b0b` |
| 47 | 6 | `h2pool`：`Conn` 多 `peer`，`connect` 在 `self.dial.dial(opts)` 之后读远端（`mod.rs:306`），`open` 在 `lease` 之后 `connected`（`mod.rs:188`）。用例 `every_stream_on_a_shared_h2_connection_records_its_address`；RED 是第二条流的记录为 `None`。 | `e783ccd` |
| 48 | 6 | ssh：`Session` 多 `peer`，`establish` 在建好底下的连接之后读远端（`outbound.rs:706`），`dial` 在每次 `session()` 之后 `connected`（`outbound.rs:641`、`:649`）。用例 `every_channel_on_an_ssh_session_records_its_address`；RED 是第二条通道的记录为 `None`。 | `0ec7cd9` |
| 49 | 6 | 门禁三次（一台 Windows 11 开发机上的一次运行，不是 CI）：第一次在 `0ec7cd9` 的树上；第二次在同一棵树加完这几行之前；第三次就是加完本提交的树之后的再一次全量（细节与第三次的数记在 `task-6-report.md` 里）。前两次里 `cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0；`cargo test -j 8 --workspace --no-fail-fast` 三次都是 93 个结果行、5361 通过、0 失败、15 忽略（第二次的两次运行分别以 `STATUS_HEAP_CORRUPTION` 退出：`rurge_inbound` 与 `smart`，各按约定单独重跑那一个二进制、都 80 条与 22 条全过，补齐之后就是 5361；第三次退出码 0）。没有留下 `rurge.exe`。新加的 6 条带等待的用例各自单独跑二十次（`--exact`），全过。计划文件的尾仍是 `i/lf w/lf`，弯引号 0 个。 | 本提交 |
| 50 | 5 | 订正第 36、41 行（协调者，评审任务 6 时补记；任务 6 进行中执行者在往这张表加行，那时加会撞号）：P15 的表里写着 `an_internal_dns_session_records_the_server_it_connected_to`、`a_smart_group_forgets_the_address_of_a_member_that_failed`、`a_rejected_session_has_no_remote_address_but_the_lookups_of_the_rules` 的那一行，其实按行号指到了第 36 行说的那条用例——"第 3008 行附近断言 `None` 的那一条"（`47f4552` 上 `crates/rurge-engine/tests/pipeline.rs` 第 3008 行正是 `a_doq_carrier_the_proxy_will_not_open_fails_naming_the_policy` 的 `assert_eq!(failed.remote_addr, None, ..)`，今天在 `:3029`），期望"照过"；那一行写的理由（"`tcp://` 的上游没有 UDP 的内部会话"）不合这一条：它是经 socks5 的 DoQ 载体，一个内部的 UDP 会话。所以第 36 行说的"P15 没有点名的"不准确（P15 只按行号指到了它、理由写错了），第 41 行也没有看出来；第 36 行的两处 `clear_peer` 正好让 P15 的期望成立，第 41 行的结论（接受）不变 | 本提交 |
| 51 | 6 | 协调者核过任务 6（`43fcb45`、`b7f9a53`、`8a11377`、`ca87b0b`、`e783ccd`、`0ec7cd9`、`8360a13` 与 `3845ad0`，第 42 ～ 49 行）。门禁：五次全量的日志协调者数过，日志的时间与提交的时间对得上：第一次在 `0ec7cd9` 的树上，93 / 5361 / 0 / 15、退出码 0；第二批三次都在 `8360a13`（加完第 42 ～ 49 行）的树上——2a 是 92 个结果行、5281 通过、退出码 101（`rurge_inbound` 的测试二进制以 `0xc0000374 STATUS_HEAP_CORRUPTION` 死掉，单独重跑 80 / 0），2b 是 92 个结果行、5339 通过、退出码 101（`smart` 的，单独重跑 22 / 0），2c 是 93 / 5361 / 0 / 15、退出码 0；第三次在 `3845ad0` 的树上，93 / 5361 / 0 / 15、退出码 0；`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 跑了三次（第一次、第二批之前、第三次），退出码都是 0（一台机器上的运行，不是 CI；报告正文说的"四次""三次"与它自己的表对不上，以表为准）。计划的两张表从 `7d06251` 起只多不少（修正记录多第 42 ～ 49 行，延后事项没有动），弯引号 0，计划的行尾 `i/lf w/lf`，`git diff --check` 干净，带本机账户名的文件 26，`Cargo.toml`、`Cargo.lock`、`unsafe_functions.rs` 与 `sockets.rs` 都没有动。逐项核过 P9：② snell 的 `fresh` 在 `stack.open` 之后、`SnellStream::open` 之前读远端，池里的连接带着它，从池里取出时先 `connected` 再 `reused`，`open_udp` 换成 `fresh` 交回的（任务 5 的 `current()` 那一句与它上面的三行注释删了）；失效重拨在 `reused` 时记下的会话 trail 下先 `clear_peer` 再 `follow`，`poll_open` 换上新的 `peer`，`Drop` 与 `finish` 放回池里时带着它，没有 trail 的照旧不跟随；③ anytls 的 `Session` 带着远端，从池里取出的在 `open` 成功之后才 `connected`；④ `h2pool` 的拨号在调用方的任务里一路 `await`（`lease` → `dial_one` → `connect`，不另起任务），所以 `connect` 读得到会话的 trail，`open` 在 `lease` 之后写；⑤ ssh 的 `establish` 读，`dial` 里两处 `session()` 之后都写（`outbound.rs` 里调 `session()` 的只有这两处）。池的大小、复用的规则与超时没有改，没有加日志行。第 43 行（执行者的裁定）接受，错在派工单的前提：`[Host]` 不作用于代理服务器自己的名字，是阶段 1 的"代理主机名防环"（FR-DNS-05，`docs/acceptance/phase1-acceptance.md` 那一条：`lookup_inner` 对 `proxy_hostnames` 不查 `[Host]`），协调者写派工单时没有查到；改用夹具的 `MockDns`（回环上、Profile 里唯一的 DNS 服务器）答 `snell.test`，断言没有改。顾虑 2 接受：三条 snell 的引擎用例写在实现之后（派工单「分小步做」的次序），照读代码，没有 P9 ② 它们都会红：借来的连接没有拨号，第二条会话的 trail 上没有远端；重拨不在会话的 `follow` 里，`dns_ms` 也是 `None`。两处文字：第 42 行说空行的事"修正记录第 39 行提到"，是第 41 行；第 49 行在 `8360a13` 写下之后被 `3845ad0` 改写了——已写下的行不改写（`CLAUDE.md`「工作流约定」），订正应当另加一行写明它覆盖哪一行。改写前的那一版说"两次都没有死掉的测试二进制"，不对；改写后的那一版说第二次在"加完这几行之前"的树上，也不对（是 `8360a13`，加完之后），并且没有提 2c 那一次；以本行写的五次为准，第 49 行不再改回。协调者自己跑的：七条新用例各直接跑测试二进制、`--exact`、二十次（每次限时 120 秒），140 次全过；新增的用例代码里唯一的定时是 `read_exact` 外面 5 秒的时限（等要发生的事），不是固定等待；`192.0.2.9:9` 只当哨兵写进 trail、不拨。**任务 6 过了** | 本提交 |
| 52 | 7 | `rurge-net::quic`（P10 ①）：`FarEnd(Far)` 与私有的 `Far { Fixed(Option<SocketAddr>), Hopping(hop::Current) }`、`FarEnd::now`；`hop::Current` 与 `Hopping::current()`（`hop.rs:212` 一带）；`Path` 多私有字段 `far_end`，`open_over` 的三支各自给（直连 `mod.rs:123`、跳端口 `:134` 在 `Arc::new(hopping)` 之前、经承载 `:149` 在 `Carried::new` 之前，不是 `STAND_IN`），ECN 那一层的 `..path` 带着它；`Path::far_end`（`:186`）。两个测试字面量补字段：`quic/dial.rs` 那个经承载的补 `FarEnd(Far::Fixed(None))`（不把替身写进远端），`quic/h3/mod.rs` 那个直连的补 `Fixed(Some(remote))`。用例三条先红：`the_current_port_follows_the_hops`（红在缺少方法 `current`）、`a_direct_path_far_end_is_its_remote` 与 `a_carried_path_far_end_is_its_carriers_not_the_stand_in`（红在缺少方法 `far_end`）；测试数据报 `CutDatagram` 加 `far_end` 转交 `self.0.far_end()`。`rurge-dns` 的 `upstream/session.rs` 一行不改；`git grep -n 'remote_address' -- crates` 改前改后都是同样的 7 行。 | `e9b99d4` |
| 53 | 7 | `rurge-proto-quic`（P10 ②）：`Dialer::connect` 交回 `(Connection, FarEnd)`（`path()` 之后、交给 `quic::connect` 之前取）；`Session::connect` 交回的元组多一项；`Conn` 多 `far_end: FarEnd`（唯一建 `Conn` 的地方存进去）；`Lease::far_end` 是 `self.conn.far_end.now()`（`session.rs:264`）；`take` 在交回 `Ok` 的每一处之前写进会话的 trail——借来的在 `session.rs:488`（`slot` 的锁里；`connected` 不 `.await`）、新拨的在 `:511`（先 `?` 取出 lease、写、再 `Ok`）。 | `9235156` |
| 54 | 7 | 两个 UDP 载体（P10 ③）：`Hysteria2Udp::far_end` 与 `TuicUdp::far_end` 都是此刻绑定的那个 lease 的 `far_end()`，没有绑定是 `None`；直接读 `bound` 而不经 `current()`——tuic 的 `current()` 可能 `unbind`，有副作用。MASQUE 的 UDP 照旧是缺省的 `None`（P8 ②）。 | `9235156` |
| 55 | 7 | WireGuard（P11）：`device.rs` 的 `Shared` 多 `routes: Routes`（与协议栈选对端的是同一张表）与 `far_ends: Mutex<Vec<Option<SocketAddr>>>`；`Shared` 的建处（`device.rs:340` 一带）在 `carriers` 搬进 `Driver` 之前，由拨好的 `carriers` 各自的 `datagram.far_end()` 初始化（`:336`），`start` 里给 `carriers[i]` 的赋值（`:308`）被它盖住、不另写。`Driver::note_far_end`（`:793`）让给 `self.carriers[peer]` 赋值的两处同时更新：`land` 的 `Some(..)`（`:829` 先取、`:830` 再搬走 `datagram`）与收包时载体结束的 `None`（`:958`、`:959`）；锁中毒的照样取。`Device::far_end_for`（`:421`）；`outbound.rs:152` 在 `connect` 成功之后 `connected`；`udp.rs:127` 在 `to` 是地址时问它、是名字时 `None`。 | `4cd3e86` |
| 56 | 7 | 用例：`outbounds_hysteria2.rs` 三条、`outbounds_tuic.rs` 两条、`outbounds_masque.rs` 两条、`outbounds_wireguard.rs` 三条，连 `quic/mod.rs` 的两条与 `hop.rs` 的一条共 13 条。循环二十次（直接跑测试二进制、`--exact`、每次套 180 秒时限）：11 条新用例各 0/20 失败；MASQUE 的 UDP 那条原叫 `a_masque_udp_flow_has_no_remote_address`，为了让派工要求的 `remote_address` grep 改前改后相同，改名为 `a_masque_udp_flow_records_no_far_end`（执行者的裁定），改名之后又单独跑了二十次、0/20 失败。RED 的证据：把 `Lease::far_end`（`session.rs:264`）与 `Device::far_end_for`（`device.rs:421`）临时改成 `None`，前后各让这两族的用例变红（记录是 `None`，期望是夹具在回环上的地址），随后改回。 | `7fe4998` |
| 57 | 7 | 门禁（一台 Windows 11 开发机上的一次运行，不是 CI）：在 `7fe4998` 的树上，`cargo fmt --all --check` 退出码 0；`cargo clippy -j 4 --all-targets -- -D warnings` 退出码 0（`-j 4` 是因为缺省的并行度在链接 `rurge-interop` 与两条 `rurge-engine` 用例时撞上 `LNK1102: out of memory`，重跑一次就好了，见报告）；`cargo test -j 8 --workspace --no-fail-fast` 退出码 0，93 个结果行、5374 通过、0 失败、15 忽略（基线 5361 加本任务的 13 条）。此后只加了本节这几行的文字，另跑了一次全量，那一次的数在 `task-7-report.md` 里。 | 本提交 |
| 58 | 7 | 第二次门禁（同一台开发机上的再一次运行，不是 CI）：在 `f0aa553` 的树上（就是加完第 52 ～ 57 行的那一棵），`cargo fmt --all --check` 退出码 0、`cargo clippy -j 4 --all-targets -- -D warnings` 退出码 0、`cargo test -j 8 --workspace --no-fail-fast` 退出码 0，93 个结果行、5374 通过、0 失败、15 忽略，与第一次相同。加完本行之后再跑的一次在 `task-7-report.md` 里。 | 本提交 |
| 59 | 7 | 修正轮 1（协调者的评审，三处）：① `outbounds_wireguard.rs` 的 `a_session_records_the_peer_its_destination_routes_to` 原来按 `the_records` 的次序断言两个远端，而那个次序是会话**结束**的先后（`RequestLog::recent` 读的是 `record_finished` 推进环里的次序）；两条会话经两个载体结束，谁先结束取决于 `recv_any` 从哪个载体问起（`device.rs:627` 每收一个就换 `first`），本机二十次没红不等于别的平台不红；改成按 `dst` 找各自的记录（`10.1.0.1:7` 与 `10.2.0.1:7`，先例是 `outbounds_tuic.rs:250` ～ `:253`），找不到时的信息带上全部记录，其余断言不变，两条会话照旧同时开、同时关。② 同一条用例的 `let ((private, public), _) = (keypair(), ());` 改成 `let (private, public) = keypair();`（这个文件自己的写法，这里只要一对钥匙）。③ `hysteria2/udp.rs` 的 `far_end` 注释说 `current` "moves the carrier"，而 hysteria2 的 `current`（同文件 `:428`）只比路由、不挪载体（换连接的是 `bind`），tuic 的才可能 `unbind`；改成照实说：路由离开那条连接之后它答 `None`，而在下一次发送绑上新连接之前，数据报照旧走绑着的那一条。代码不动。验证（一台开发机上的一次运行，不是 CI）：`cargo fmt --all --check` 退出码 0、`cargo clippy -j 4 --all-targets -- -D warnings` 退出码 0；`outbounds_wireguard` 的测试二进制不过滤跑一遍，19 通过、0 失败、退出码 0；`--exact a_session_records_the_peer_its_destination_routes_to` 单独循环二十次，0/20 失败。只改了一条用例的找法、一行绑定与一句注释，**没有跑全量门禁**（任务 8 的全量门禁会包含它），所以这一行没有第三个门禁数。 | 本提交 |
| 60 | 7 | 协调者核过任务 7（`e9b99d4`、`9235156`、`4cd3e86`、`7fe4998`、`f0aa553` 与 `e960894`，第 52 ～ 58 行）与它的修正轮 1（`c5689c1`，第 59 行）。门禁：三次全量的日志协调者数过，日志的时间与提交的时间对得上。第一次是先跑、后提交的：门禁在 11:49 ～ 11:55，四个代码提交在 11:56 ～ 11:57，源码与用例的最后修改都在门禁开始之前，所以跑的就是 `7fe4998` 的内容。第二次在 `f0aa553` 的树上，第三次在 `e960894` 的树上。三次都是 93 / 5374 / 0 / 15、退出码 0，`cargo fmt --all --check` 与 `cargo clippy -j 4 --all-targets -- -D warnings` 的退出码都是 0，没有崩溃；13 条新用例在第一次与第三次的日志里都是 `ok`（一台机器上的运行，不是 CI）。第三次的数只写在报告里（第 58 行把它指到报告），记在这里。计划的两张表从 `339e141` 起只多第 52 ～ 59 行，延后事项没有动；弯引号 0，计划的行尾 `i/lf w/lf`，`git diff --check` 干净；带本机账户名的文件 26，`Cargo.lock` 520 个包，`unsafe_functions.rs` 与 `sockets.rs` 的 `LISTED` 没有动，`rurge-proto-wireguard` 的 `testing/` 与 `rurge-dns` 没有动；执行者的提交不带署名行。逐项核过 P10 与 P11。① `FarEnd` / `Far` / `Current` 照派工单定义。`open_over` 的三支各给自己的远端：直连 `Fixed(Some(remote))`（`mod.rs:123`）；跳端口 `Hopping(hopping.current())`（`:134`，在 `:137` 的 `Arc::new(hopping)` 之前取）；经承载 `Fixed(carrier.far_end())`（`:149`，在 `:150` 的 `Carried::new` 之前取，不是 `STAND_IN`）。ECN 那一层的 `..path` 带着它。两个测试字面量：`dial.rs:633` 补 `Fixed(None)`，`h3/mod.rs:270` 补 `Fixed(Some(remote))`。`git grep -n 'remote_address' -- crates` 按提交读，`339e141` 与 `c5689c1` 上都是同样的 7 行（工作区里的 CRLF 会让两份输出直接比对时只在行尾上不同）；`rurge-dns` 的 `upstream/session.rs` 没有动。② `Dialer::connect` 在把 `path` 交给 `quic::connect` 之前取 `path.far_end()`；`Conn` 唯一的建处存进 `far_end`；`take` 交回 `Ok` 的两处之前都写：`session.rs:488` 在 `slot` 的锁里，`connected` 不 `.await`；`:511` 在 `?` 之后。③ hysteria2 与 tuic 的 `far_end` 读 `bound`、不经 `current()`；MASQUE 的 UDP 照旧缺省。P11：`Shared.routes` 与 `stack.rs:158` 同一句建；`far_ends` 在 `carriers` 搬进 `Driver` 之前由拨好的载体初始化；`note_far_end` 在两处赋值旁边——`land` 的 `:829` / `:830` 在"端点没有变就不换"的早退之后，没有换的载体不改写；载体结束的在 `:958` / `:959`。`carriers[` 的其余命中都是读，`send_all` 的 `get_mut` 不换数据报。`dial` 在 `connect` 成功之后写，`TunnelUdp::far_end` 对地址问 `far_end_for`、对名字答 `None`。两条顾虑接受。其一：跳端口的 hysteria2 记的是 `take` 那一刻的端口——这是 P10 的口径，单元那一层由 `the_current_port_follows_the_hops` 钉住，引擎那一层没有跳端口的用例。其二：WireGuard 的 `far_end_for` 在 `connect` 之后读、不与换载体在同一把锁里，碰上换载体的会话可能记下前一个载体的远端；这仍是"记下时的那一跳"（M4-D23），不改。第 56 行的改名（`a_masque_udp_flow_records_no_far_end`，好让 grep 前后相同）接受：那一条查的是对 quinn 的 `remote_address()` 的调用，改名无害。修正轮 1 的三处协调者读过改后的代码：按 `dst` 找记录（理由在第 59 行）；`let (private, public) = keypair();`；hysteria2 那句注释照实说 `current` 只比路由。修正轮按修正单没有跑全量，任务 8 的全量门禁是第一次包含它的。协调者自己跑的：在 `c5689c1` 上 `cargo test -j 4 --workspace --no-run`，退出码 0，76 个测试二进制；13 条新用例各直接跑测试二进制、`--exact`、二十次（每次限时 120 秒）。`rurge-net` 的三条在它的测试二进制上跑（`c5689c1` 的编译交回的还是 11:10 链接的那一个，`rurge-net` 此后没有改），60 次全过；引擎的十条在 `c5689c1` 编译的二进制上跑，200 次全过。新增的用例代码里没有固定等待：`the_records` 与 `wait_until` 都限 5 秒，`hop.rs` 的那一条不等。`192.0.2.1:443` 在用例里只出现在 `assert_ne!` 里。**任务 7 过了** | 本提交 |
| 61 | 8 | P12 里的一处出入（执行者的裁定）：`a_full_request_log_of_http_requests` 照 P12 本来要打印"`recent` 能读到的条数"，而 `recent(100_000)` 会把十万条记录各复制一份、在例句行里活到打印之后，读峰值之前调它就把峰值抬高，量到的就不是记录表的开销；改成先读峰值、先打印峰值，再打印 `log.len()`（不复制），并断言它是 100_000。`observe/memory.rs` 的注释里写明了这一点。 | `91ec9f1` |
| 62 | 8 | 量的结果（一台 Windows 11 开发机上的一次运行，不是 CI；两条各单独跑一次、调试构建，输出在 `memory-task-8.log`）：`an_empty_request_log_is_the_baseline` 的峰值是 17 702 912 字节，`a_full_request_log_of_http_requests` 是 126 230 528 字节（容量 100 000 条，装满之后再进 10 000 条、挤掉最早的，打印 `records held: 100000`）。差 108 527 616 字节，除以 100 000 是 1085.3 字节一条，取整到 8 字节是 **1088 字节**一条；1 000 条（`--request-log-size` 的缺省）约 1.04 MiB、10 000 条约 10.4 MiB、100 000 条约 103.8 MiB。缺省不改（M4-D25），这个数交给项目所有者，任务 9a 写进 `docs/api/phase4.md`。 | `91ec9f1` |
| 63 | 8 | 门禁（同一台开发机上的一次运行，不是 CI）：在 `97b5fe4` 的树上（两个提交 `91ec9f1` 与 `97b5fe4`），`cargo fmt --all --check` 退出码 0、`cargo clippy --all-targets -- -D warnings` 退出码 0、`cargo test -j 8 --workspace --no-fail-fast` 退出码 0，93 个结果行、5376 通过、0 失败、17 忽略（基线 5374 / 0 / 15 加本任务两条通过的与两条忽略的），没有崩溃、没有重跑。`api.rs` 的新用例 `the_request_records_carry_the_three_new_keys` 直接跑测试二进制、`--exact`、二十次，0/20 失败；它测的是任务 1 ～ 7 已经做好的行为，第一次跑就是绿的，没有 RED。此后只加了本节第 61 ～ 63 行的文字，按规矩又跑了一次全量，那一次的数写在 `task-8-report.md` 里。 | 本提交 |
| 64 | 8 | 订正第 61 行的一处手误（第 61 行不改）：那里写的"在例句行里活到打印之后"，说的是 `recent` 交回的那一份 `Vec` 活到 `println!` 那一句打印完，第 61 行取错了一个词。第 61 行说的其余不变。 | 本提交 |
| 65 | 8 | 修正轮 1（协调者的评审）：`observe/memory.rs` 的两处注释把量到的容量说成了缺省——模块注释的第一句（"A request log costs in memory at its default capacity"）与 `a_full_request_log_of_http_requests` 的文档注释第一句（"A log at the default capacity"）。缺省是 1 000 条（`crates/rurge/src/cli/run.rs` 的 `unwrap_or(1000)`），这里量的是 `RequestLog::new(100_000)`；两句都改成照实说十万条，模块注释那一句带上"缺省的一千条是它的千分之一"。只改这两句注释，代码与别的注释不动。验证（一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 退出码 0、`cargo clippy -j 4 --all-targets -- -D warnings` 退出码 0、`cargo test -j 8 --workspace --lib -- observe::memory` 1 通过、2 忽略、退出码 0。提交之后又跑了一次全量（M4b 最后一棵代码树的那一次），数写在 `task-8-report.md` 里。第 61 ～ 64 行不改写。 | 本提交 |
| 66 | 8 | 修正轮 2（协调者的评审）：订正第 65 行（第 65 行不改写）。第 65 行错在两处：① 它写的"缺省的一千条是它的千分之一"，一千条是十万条的**百分之一**，不是千分之一（同一个文件里那句 "a hundred times the default 1 000" 才是对的）；② 它引原来的模块注释写成"A request log costs in memory at its default capacity"，原文是 "What a request log costs in memory at its default capacity."，少了开头的 "What" 与句末的句点。代码里那一个词也改了：`crates/rurge-engine/src/observe/memory.rs:2` 的 "(a request log of the default 1 000 costs a thousandth of it)" 改成 "a hundredth of it"，只改这一个词。验证（一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 退出码 0、`cargo clippy -j 4 --all-targets -- -D warnings` 退出码 0、`cargo test -j 8 --workspace --lib -- observe::memory` 1 通过、2 忽略、退出码 0。提交之后又跑了一次全量（仍是 M4b 最后一棵代码树），数写在 `task-8-report.md` 里。 | 本提交 |
| 67 | 8 | 协调者核过任务 8（`91ec9f1`、`97b5fe4`、`005daa1` 与 `2e6abfe`，第 61 ～ 64 行）与它的两个修正轮（`af84a16`，第 65 行；`f5723fd`，第 66 行）。门禁：日志协调者数过，日志的时间与提交的时间对得上（一台机器上的运行，不是 CI）。第一次在 `97b5fe4` 的树上，93 / 5376 / 0 / 17、退出码 0。第二次在 `005daa1` 的树上：92 个结果行、5363 通过、0 失败、17 忽略、退出码 101——`outbounds_tls_family` 的测试二进制以 `0xc0000374 STATUS_HEAP_CORRUPTION` 死掉（日志 `Caused by:` 下那一行），单独重跑 13 / 0，合计 93 / 5376 / 0 / 17。第三次在 `2e6abfe` 的树上，93 / 5376 / 0 / 17、退出码 0。三次的 `cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 退出码都是 0。第二次的数只写在报告里（第 63 行把它指到报告），第三次没有哪一行提到，都记在这里。修正轮 1 提交之后在 `af84a16` 的树上跑的那一次：93 / 5376 / 0 / 17、退出码 0（fmt、clippy 在提交之前跑过，退出码 0）。修正轮 2 提交之后的那一次：在 `f5723fd` 的树上 93 / 5376 / 0 / 17、退出码 0（fmt、clippy 在改完那一个词之后、提交之前跑过，退出码 0；全量日志的时间 13:10:25 在提交的 13:06:39 之后）。计划的两张表从 `97f02a8` 起只多第 61 ～ 66 行，延后事项没有动；第 64 行另起一行订正第 61 行、第 66 行另起一行订正第 65 行，都没有改写，合规矩。弯引号 0，计划与新文件 `observe/memory.rs` 的行尾都是 `i/lf w/lf`，`git diff --check` 干净；带本机账户名的文件 26，`Cargo.lock` 520 个包，两张 `LISTED` 没有动，`docs/api` 与 `docs/acceptance` 没有动；执行者的提交不带署名行。逐项核过 P12：`#[cfg(test)] mod memory;` 在 `observe.rs` 末尾的 `#[cfg(test)] mod tests {` 之前；`peak_resident_bytes`、`parse_count`、`parse_vm_hwm` 与 `the_peak_is_read_from_what_the_system_prints` 的函数体与 `crates/rurge-http/src/capture/memory.rs` 的逐字相同；`a_full_request_log_of_http_requests` 造 110 000 个句柄，照引擎的路 `mark_active` → `finish` → `record_finished`，每个记完就丢；先读峰值、再 `log.len()`（第 61 行的裁定，接受：`recent(100_000)` 会把十万条复制一份）；两条量的各单独跑了一次，输出里只有数。量到的数与算法对（第 62 行）：差 108 527 616 字节，除以 100 000 是 1085.3，向上取整到 8 的倍数是 1088 字节一条；三个总量 1.04 / 10.4 / 103.8 MiB 是由 1088 算的，由量到的差直接算是约 1.03 / 10.3 / 103.5 MiB。报告顾虑 1 的一句理由不对，修正轮 1 在报告里订正了：空表的 `VecDeque::with_capacity` 只分配、不写，没写过的页不进工作集——空表的峰值（17 702 912 字节）比十万格还小，一格 `RequestRecord` 本身就有三百多字节；所以 1088 字节里含着环里那一格本身，加上它在堆上的字符串与向量，正是一条记录在表里的开销，数不变。`api.rs` 的 `the_request_records_carry_the_three_new_keys`：`POST` 的请求头与正文一次写出；两处等法都按内容找记录（`method == "POST"`、`dst`），都有时限（`wait_until` 5 秒、四处 `tokio::time::timeout` 各 5 秒），没有固定等待；`CONNECT` 的应答读一次，同这个文件的先例（`:1258`、`:1369`）。派工之前协调者照代码核过的前提都成立：端口不是 80 / 443 的 `CONNECT` 不嗅探（`sniffable_session`）；`method` 只由 `rurge-http` 的引擎写，所以隧道的两个正文计数是 `null`；`relay.rs:89` 记下第一个回来的字节，所以 `receiveMs` 是数。没有 RED（第 63 行，接受：它测的是任务 1 ～ 7 已经做好的行为）。修正轮 1：`observe/memory.rs` 的两句注释原来说 "default capacity"，而缺省是 1 000 条（`crates/rurge/src/cli/run.rs:1769`），量的是 100 000 条，改成照实说；但模块注释新写的 "a thousandth" 错了（1 000 是 100 000 的百分之一，同一个文件另一句写的 "a hundred times" 是对的），第 65 行与它的提交说明也写成"千分之一"，第 65 行引的原文还少了开头的 "What"。修正轮 2 把那个词改成 "a hundredth"，第 66 行订正第 65 行的两处。协调者自己跑的：在 `f5723fd` 上编出测试二进制（`--no-run`，76 个），`tests/api.rs` 的 `the_request_records_carry_the_three_new_keys` 与库内的 `observe::memory::the_peak_is_read_from_what_the_system_prints` 各单独跑二十次（`--exact`），都是 20 / 20；量的两条也各单独跑了一次（`--ignored --exact --nocapture`，调试构建）：空表的峰值 17 649 664 字节，装满的 125 829 120 字节（`records held: 100000`），差 108 179 456 字节，除以 100 000 是 1081.8 字节一条（总量约 1.03 / 10.3 / 103.2 MiB），比任务 8 的 1085.3 少约 3.5 字节（约 0.3%）；都是一台机器上的运行，不是 CI。1081.8 取最近的 8 的倍数是 1080、向上取整才是 1088，所以 1088 是一次运行向上取整的数，不是精确的；任务 9a 在 `docs/api/phase4.md` 里照 M4a 那张表，把两次量到的数各写一行（协调者的决定）。**任务 8 过了** | 本提交 |
| 68 | 9a | 三份文档。`docs/api/phase4.md`：在「M4a　抓包」之后加一节「M4b　请求记录的完整」（自第 940 行起），六小节：三个新键（意思、何时 `null`、在哪里数、`HEAD`、升级、源站提前作答）、九句备注的表与判断次序（名单 → 开关 → CA → 来源 → 名字 → ALPN）、`remoteAddr` 的新口径与订正 `docs/api/phase1.md` 的那一句（历史段落不改）与各载体的表（照任务 5 ～ 7 实际做的写，与设计第 10.3 节不同的地方指到修正记录的行）、`dnsMs` 在解析途中、量到的一条记录、没有覆盖的。`docs/acceptance/phase4-manual.md`：M4 一节加 W11、W12、M11、M12、L11、L12 与 P14 ～ P17 共 10 项（结果、日期、系统版本三栏留空）；第 5 行末尾与 M4 一节开头那两句改成 M4b 的项也在了（2026-11-02 加，编号从 W11、M11、L11 与 P14 起）；第 76 行起的五条验收标准那张表不动（M4b 的项不证明那五条）。`docs/testing.md`：第 217、218 行加两条量记录的用例命令（各 1 条、`#[ignore]`），第 549 行（M4a 那段）末尾补一句。数：两次量记录的大小按 M4a「量到的内存」那张表的列各写一行——任务 8 的 17,702,912 / 126,230,528 / 108,527,616 / 1085.3（`memory-task-8.log`）与协调者评审那一次的 17,649,664 / 125,829,120 / 108,179,456 / 1081.8（没有日志文件，数在第 67 行）；1 000 / 10 000 / 100 000 条的约 1.03 / 10.3 / 103 MiB 由多出的直接算；1088 只写成本任务 8 那一次向上取整到 8 的数、不写成精确的（第 67 行）。用例名、条数与登记没有做的（三处 `bad_request` 没有用例——第 18 行；跳端口的 hysteria2 没有引擎用例与 WireGuard 的 `far_end_for`——第 60 行；`external` 的 UDP 只断言到回环——第 39 行）出自任务 1 ～ 8 的报告与本计划的修正记录。这一份手工清单一项也没有做，文档照实写 | 本提交 |
| 69 | 9a | 修正轮 1（协调者的评审，十一处，都是照代码或出处写错的）：`docs/api/phase4.md` ① 第 958 行起「三种不普通的情形」改成「四种」；第三种（源站提前作答、请求正文还在上传）不再挂 `a_client_that_leaves_mid_body_counts_what_was_handed_over`、改写成没有用例（登记），另加第四种「客户端在应答正文中途走掉」（`responseBodyBytes` 是到那时交给它的字节数，不多于源站写的）并挂那一条用例（`crates/rurge-http/src/engine.rs:6344` 测的是这一种：`Content-Length: 100`、源站只写 10 字节、客户端读完就走，记录是 `(0, 10)`）；② 第 964 行的「加法不回绕」改成 `u64` 的两个 `AtomicU64::fetch_add` 累加、要 2^64 字节才会溢出回绕（`crates/rurge-inbound/src/session.rs` 的 `add_request_body` / `add_response_body`）；③ 第 1018 行「它是 `connectMs` 里的一段」改成照实：出站就绪之前的查找在 `connectMs` 那一段里，出站就绪之前 `connectMs` 是 `null` 而 `dnsMs` 可以已经有数，出站就绪之后的查找也算进 `dnsMs`（snell 的重拨，设计第 18 节第 16 条、计划 P9 ②），所以 `dnsMs` 不一定不大于 `connectMs`；④ 第 1003 行 quinn 那句的出处从修正记录改成设计第 18 节第 14 条与计划 P10 ①（做法仍指修正记录第 52、53 行）；⑤ 第 984 行预匹配那句的出处从修正记录第 22 行改成设计第 18 节第 11 条与计划 P6 ⑤ 的登记；⑥ 第 1014 行的用例单补 `tests/outbounds_tls_family.rs` 的 `a_pooled_anytls_session_gives_its_record_the_servers_address` 与 `tests/pipeline.rs` 的 `a_carrier_over_a_socks5_association_records_the_relay`；⑦ 第 1031 行的句柄描述照 `crates/rurge-engine/src/observe/memory.rs` 的 `handle()` 改成 `url` 87 ～ 92 字节、备注 53 字节（`dst` 32、`sni` 23、规则文字 42 不变）；⑧ 第 1040 行 1088 那一句改成：任务 8 的 1085.3 取整是 1088（第 62 行），协调者那一次的 1081.8 向上取整也是 1088、取最近是 1080（第 67 行）；⑨ 第 1006 行 UDP 流那一格把 QUIC 一族与 `wireguard` 从「只有一个远端」里拿掉、改成「见上面两格」。`docs/acceptance/phase4-manual.md`：⑩ 第 339 行 P17 的期望改正（进行中的那一条 `connectMs` 是 `null`；结束之后 `connectMs` 不小于 `dnsMs`；`[Host]` 钉住的名字是一次接近 0 的查找，不是 `null`——`crates/rurge-dns/src/resolver.rs:748` 在走 `[Host]` 之前就 `Lookup::begin()`，`:1700` 的用例断言钉住的名字 `is_some()`，`docs/api/phase1.md:69`）；⑪ 第 337、338 行 P15 (c) 与 P16 的中继改成 socks5 服务端在 `UDP ASSOCIATE` 的应答里给的 `BND.ADDR:BND.PORT`（地址是 `0.0.0.0` 一类时用代理的地址配那个端口，`crates/rurge-proto/src/socks5.rs:183` 的 `relay_of`）。同时订正第 68 行（第 68 行不改写）：它写的「1088 只写成本任务 8 那一次向上取整到 8 的数」多了个「本」字、读成「成本」；它说的「源站提前作答」挂着的用例与 1088 的取整，以本行为准。只改文档，没有跑门禁 | 本提交 |
| 70 | 9a | 协调者核过任务 9a（`223a83c`，第 68 行）与它的修正轮 1（`11fbd9d`，第 69 行）。只改文档、没有跑门禁，照派工单；M4b 最后一棵代码树仍是 `f5723fd`（第 67 行）。结构：`docs/api/phase4.md` 只在末尾加「M4b　请求记录的完整」一节（第 940 行起，旧文一字未动）；手工清单只改了第 5 行与 M4 一节开头那两句（派工单让改的），四张表各接着加 W11、W12、M11、M12、L11、L12 与 P14 ～ P17；`docs/testing.md` 第 217、218 行加两条命令，第 549 行的旧文是新行的前缀；计划的两张表从 `fcb6aa1` 起只多第 68、69 行，延后事项没有动，第 69 行另起一行订正第 68 行、没有改写。弯引号的数照旧（`phase4.md` 32、手工清单 0、`testing.md` 116，都是原有的；计划 0），四份的行尾 `i/lf w/lf`，`git diff --check` 干净，带本机账户名的文件 26；执行者的提交不带署名行。照代码逐项核过：新一节与手工清单点名的用例都在它说的文件里（逐个 `git grep` 了 `fn <名>`；`the_record_of_a_udp_session_has_no_remote_address` 是 `47f4552` 上的旧名）；`RequestJson` 的键序（`statusCode` → `requestBodyBytes` → `responseBodyBytes`，`firstByteMs` → `dnsMs` → `receiveMs`）；九句备注与 `serve.rs` 的九个常量逐字相同；判断次序（名单 → 开关 → CA → `decides` 的来源 → 名字 → ALPN）；`dial_noted` / `PassedBy`、`Turned::Unknown` 的早退与 `judged_at_the_entry`；正文在 `Idle`、`Tail`、`LocalFile` 交出每一帧时与 `own_body` 里数；`HEAD` 与 Map Local（`map_local` 对 `HEAD` 不带数据）；`trust-tunnel` 的 HTTP/2 版用 `H2Pool`、QUIC 版用 `session::Session`；`H2Udp` 是 `h2connect` 的；没解密的 TLS 会话 `protocol` 是 `https`；W11 (b) 的前提（报告顾虑 1，接受）：引擎替每个请求开句柄用 `new_handle_under`、不经 `dial_noted`，两张名单只在 `sniffable_session` 与 `dial_noted` 里问，所以 HTTP 监听器上绝对形式的请求不受名单影响，要经 SOCKS5 才看得到那句备注。报告顾虑 2（`responseBodyBytes` 与浏览器解压之后的长度不同，期望栏写明照实记）与 3（只列用例名、不写条数）也接受。修正轮 1 改的十一处都是协调者照代码与出处核出来的：① 第三种情形挂错了用例（那一条测的是客户端在应答正文中途走掉；源站提前作答、请求正文还在上传那一种没有用例，加进「没有覆盖的」）；② "加法不回绕"（代码是 `fetch_add`）；③ `dnsMs`"是 `connectMs` 里的一段"（snell 转发时重拨的查找在出站就绪之后，进行中的会话 `connectMs` 是 `null`）；④⑤ quinn 与预匹配两句的出处（设计第 18 节第 14、11 条）；⑥ 用例单少的两条；⑦ 句柄的 `url` 与备注照抄了 P12 的估计（代码里是 87 ～ 92 与 53 字节）；⑧ 1088 的取整把两次说混了（1085.3 取最近也是 1088）；⑨ UDP 流那一格；⑩ 手工 P17 的两句期望（`[Host]` 的命中也是一次查找，`resolver.rs:748`、`:1700`）；⑪ P15 (c)、P16 的中继地址（`relay_of` 用 `BND.PORT`）。修正轮之后协调者逐行比过 `223a83c` 与 `11fbd9d` 存着的文件：只有这十一处与第 69 行变了，第 69 行逐条写了依据。**任务 9a 过了** | 本提交 |
| 71 | 9b | M4b 的收尾文档（只改文档，没有跑门禁；M4b 最后一棵代码树仍是 `f5723fd`，第 67 行）。`docs/status.md`：阶段 4 下加「M4b（请求记录的完整）」一节（第 1048 ～ 1109 行，在 M4a 那一节之后、「## 附：依赖方向一条的原文」之前），照 M4a 那一节的结构写（做了什么、各任务与门禁、没做的与不要当成已经有的、以后的会话必须知道的事、去向写着项目所有者的、依赖附录）。`docs/surge-compatibility-matrix.md`：第 884 行（`GET /v1/requests/recent` 那一行）的说明格末尾补一段「阶段 4 / M4b 起」（三个新键、九种备注、`remoteAddr` 的新口径与照旧 `null` 的三种、`dnsMs` 在途，与设计第 11 节第 6 ～ 8、11 条和本计划 P5 ③、P6、P9 ②、P8 ② 的四处），第 5 行「最近一次」的括号换成 2026-11-02、阶段 4 / M4b 的收尾并把原来那一段改成「上一次」。`README.md` 第 36、52、66 行与 `README_en.md` 第 36、51、65 行（M4 至此做完、M4b 的一句、手工清单 62 项、下一步）；`CLAUDE.md` 的「现在在哪里」（阶段 4 那一条末尾接 M4b 做完与 M4 至此做完，CI 那一条的括号加 M4b、粗体那句改成 M1 ～ M4）；设计第 18 节末尾加「执行 M4b 时」（没有改过正文的意思，另记正文没写的两处限定——第 36、41 行）。 | 本提交 |
| 72 | 9b | 修正轮 1（协调者的评审，十一处；只改文档，没有跑门禁；第 71 行不改写）。`docs/status.md`：① 第 1054 行的 `receive_ms` 照代码重写（`observe.rs` 的 `record_of`：`h.elapsed()` 减去 `first_byte_at()`，即从上游的第一个字节到会话结束）；② 第 1070 行任务 6 的用例增量 +6 改成 +7（5354 到 5361）；③ 第 1076 行的门禁三处（不只一次的任务：4、5 各两次、6 五次、7 三次、8 五次，第 35、41、51、60、67 行；`STATUS_HEAP_CORRUPTION` 的出处补第 35 行；循环的出处补第 40、41 行，任务 4 只记在报告里）；④ 第 1081 行：两份量内存的 `memory.rs` 里 Linux 与其余平台的两支只在 CI 编译（`capture/memory.rs` 也如此），并订正第 1008 行（M4a 那一句）；⑤ 第 1093 行 `dial_noted` 与 `PassedBy` 改成在 `engine.rs`（`serve.rs` 是九句常量与 `PassedBy::Sniff(..)`）；⑥ 第 1103 行补两份 `memory.rs` 的两支；⑦ 第 1104 行补一句 `docs/api/phase1.md` 的历史段落不改、订正在 `docs/api/phase4.md` 的「M4b」一节；⑧ 第 1108 行的依赖附录改成照代码说（用 `rurge_net::trail` 的七个 crate，`rurge-policy` 实现 `Datagram::far_end`、`rurge-http` 经 `handle.trail()`）。`docs/surge-compatibility-matrix.md` 第 884 行：⑨ 共用连接那一组补上 HTTP/2 的 `trust-tunnel`（与 `h2-connect` 用同一个 `H2Pool`）。设计第 18 节：⑩ 第 617 行的"第 7、8 条"改成"第 7 条"，并指到下一段。`CLAUDE.md` 第 17 行：⑪ 第一次在 CI 上编译的加上两份 `memory.rs` 的两支。 | 本提交 |
| 73 | 9b | 修正轮 2（协调者的评审，两处措辞；只改文档，没有跑门禁；第 72 行不改写）。① `CLAUDE.md` 第 17 行：那两支在 CI 上只编译、不运行——调用 `peak_resident_bytes` 的只有带 `#[ignore]` 的用例（`crates/rurge-http/src/capture/memory.rs` 的 `:209` 与 `:232`，`#[ignore]` 在 `:180` 与 `:224`；`crates/rurge-engine/src/observe/memory.rs` 的 `:125` 与 `:138`，`#[ignore]` 在 `:113` 与 `:135`），而 CI 跑的是 `cargo test --workspace --no-fail-fast`（`.github/workflows/ci.yml:365`）；`#[cfg(unix)]` 那两处照旧是编译与运行，两句分开写。第 72 行 ⑩ 改成的句子把两支也说成"编译与运行"，那是协调者的修正单带的例句，以本行为准。② `docs/status.md` 第 1081 行两处：谁抄谁——M4b 的 `observe/memory.rs` 照抄 M4a 的 `capture/memory.rs`（`:56`、`:82`、`:143` 的注释写着 `Copied from crates/rurge-http/src/capture/memory.rs`），不是反过来；并补一句同 ①，那两支在 CI 上只编译、不运行。第 1103 行写的"也是第一次在那里编译"本来就对，没动。 | 本提交 |
| 74 | 9b | 协调者核过任务 9b（`b4e72a6`，第 71 行）与它的修正轮 1（`3f76784`，第 72 行）、修正轮 2（`c835f25`，第 73 行）。只改文档、没有跑门禁，照派工单；M4b 最后一棵代码树仍是 `f5723fd`（第 67 行）。结构：`docs/status.md` 只在 `### M4a（抓包）` 那一节之后、`## 附：依赖方向一条的原文` 之前加 `### M4b（请求记录的完整）` 一节，别的行一字未动；兼容清单只改第 5、884 行——第 884 行的旧格是新格的前缀、竖线数照旧 7，第 5 行在"上一次："之前插进 M4b 那一段、之后与原来的"最近一次"那一段逐字相同；`README.md` 第 36、52、66 行与 `README_en.md` 第 36、51、65 行，两份意思一致；`CLAUDE.md` 只改第 16、17 行（第 16 行的旧文是新行的前缀）；设计第 18 节只在末尾加；计划的两张表每一步都只多一行（第 71、72、73 行），延后事项照旧是空表，第 72 行没有改写第 71 行、第 73 行另起一行订正第 72 行 ⑪。行尾照旧（`CLAUDE.md` 与两份 README `i/lf w/crlf`，其余 `i/lf w/lf`），`git diff --check` 干净，带本机账户名的文件 26，手工清单 62 项（W、M、L 各 14，P 20），新一节引的提交号都在 `git log 33efe98..HEAD` 里；执行者的提交不带署名行。弯引号：`docs/status.md` 新加的两对与 M4a 一节第 973 行同一种写法，接受（报告顾虑 1）——派工单第 7 条的"弯引号 0"是协调者写宽了，只对计划与设计成立；报告顾虑 2（多一条文档的规矩）接受。照代码逐句核过新的一节：句柄的两个计数与四个方法、`RequestJson` 的键序、十处 `own_body`、`Decision` 的五种与 `decides` 的次序、`decrypting` 的次序与两行 DEBUG、`judged_at_the_entry` 的三个条件、两个 `far_end` 的缺省、`note_far_end` / `wrap_internal_udp` 与两处 `clear_peer`、四处 `trail::connected`、`FarEnd` / `hop::Current` / `STAND_IN`、`far_ends` / `far_end_for` / `Driver::note_far_end`、`Lookup` 与 `current()`、`resolver.rs:748`、两条 `#[ignore]`、520 个包与两张 `LISTED`；设计第 11 节第 6 ～ 8、11 条都登记到了兼容清单。修正轮 1 改的十一处都是协调者照代码与修正记录核出来的：① `receive_ms` 的算法（`h.elapsed()` 减 `first_byte_at()`）；② 任务 6 是 +7 条（5354 → 5361）；③ 门禁三处（有几个任务不止一次、`STATUS_HEAP_CORRUPTION` 的出处补第 35 行、循环的出处补第 40、41 行与任务 4 只在报告里）；④ 两份量内存的 `memory.rs` 里 Linux 与其余平台的两支只有 CI 编译，M4a 一节第 1008 行"也没有只在 CI 上编译的代码"同样不对、由新的一节订正；⑤ `dial_noted` 与 `PassedBy` 在 `engine.rs`；⑥ 推送那一条补那两支；⑦ `docs/api/phase1.md` 自 `33efe98` 起没有动，订正在 `docs/api/phase4.md`；⑧ 依赖附录照 `git grep` 列用 `rurge_net::trail` 的 crate；⑨ 兼容清单少了 HTTP/2 的 `trust-tunnel`（与 `h2-connect` 共用 `H2Pool`）；⑩ 设计第 18 节把 M4a 的第 8 条算成"改了做法或数"；⑪ `CLAUDE.md` 第 17 行补那两支。修正轮 2 改的两处：`CLAUDE.md` 第 17 行把那两支也说成在 CI 上"编译与运行"——调用 `peak_resident_bytes` 的只有带 `#[ignore]` 的用例、CI 的 `cargo test --workspace --no-fail-fast`（`ci.yml:365`）不跑它们，只编译（这一处是协调者修正轮 1 的例句带出来的）；`docs/status.md` 第 1081 行谁抄谁（M4b 那一份的注释写明抄自 `crates/rurge-http/src/capture/memory.rs`）。第 73 行写的"第 72 行 ⑩"应为 ⑪（⑩ 是设计第 18 节那一处），以本行为准。每一轮之后协调者逐行比过提交里存着的文件：只有修正单列的那几处与新加的那一行变了。**任务 9b 过了，M4b 做完** | 本提交 |
| 75 | 10 | 协调者的裁定：项目所有者推送了 `20efc16`（M1 ～ M4 的 295 个提交）。CI 运行 41402787066（第 16 次）里，`bench` 与 Linux、Windows 的 `check` 都通过；macOS 的 `check` 的 fmt 与 clippy 都过，测试一步只失败一条：`-p rurge-tun --lib :: tests::dispatch::queued_bytes_are_given_back_when_the_shard_takes_the_packets`，在 `crates/rurge-tun/src/tests/support.rs:314:9` panic：`timed out waiting for the shards' queue to take up its bytes`，是用例里第一个 `eventually`（10 s）。这条用例是阶段 3 / M4a 写的（那份计划修正记录第 17 行），第 13、15 次运行全绿，第 14 次的失败摘要只列 tuic 那一条，所以它在 macOS 上通过过三次。`41c35d4` 之后改过 `rurge-tun` 的只有 `b9d1cd7`（握手之后可嗅探的会话交给 `Dialer::serve`）与 `3667af5`（注释），都不碰读设备的任务、片的一轮、写者与预算。原因照源码读出，没有在 CI 上观察过，在用例的时序假设，不在产品代码。① `syn_v4` 的包 40 字节，`Vec` 的容量照 std 的扩容推算是 64（`ipv4_packet` 从 `vec![0x45, 0]` 起逐段 `extend`：2 → 8 → 16 → 32 → 64），预算按容量记（`dispatch.rs:147`），8 KiB 只装得下 128 个 SYN。② 读设备的任务一次至多取 256 个（`RECV_BATCH`），从不等队列，装不下的当场丢、计 `queue_full`。③ 片一轮至多取 256 个（`INGRESS_PER_WAKE`，`tcp/mod.rs:42`），一轮的段作为一批交给写者（`shard.rs:1185` 的 `writer.send(turn.out).await`）。④ 写者一轮从每个队列至多取 `writer_queue` 批（这里是 1，`writer.rs` 的 `limit`），`GatedDevice` 把它的 `send` 挡在闸门上。所以片最晚在第三批的 `send` 上被挡住：一批在写者手里，一批在长度为 1 的队列里，而且要先有三轮有段可发；在那之前它至多取 3 × 256 = 768 个。片落在读设备的任务后面时（忙的机器上，读设备的任务可以在片的第一轮之前就把 1024 个都过完），预算装下 128 个、其余的丢掉；片一轮取完、只发一批、不被挡住，队列里什么也不剩，`queue_bytes_queued() > 0` 一直不成立。片能不能在 SYN 用完之前轮到三轮，看各任务跑的先后：用例的前提（注释里的 "far more SYNs than the shard takes before the writer's queue is full"）不总成立。修法是插入任务 10，派给 Pi，只改用例。预算装得下比片被挡住之前能取的更多的 SYN：`3 * INGRESS_PER_WAKE + 128` 个，按 `syn(..).capacity()` 算（照同一个文件第一条用例的写法）；1024 个 SYN 照旧。这样在任何先后之下，片被挡住时队列里至少还剩 128 个，`<= BUDGET` 那一句仍有意义（片落后时，最后的 128 个照样可能被丢）。另加一条同样的用例，跑在当前线程的运行时上（`#[tokio::test]`）：预计它在旧的预算上必红，由任务 10 的 RED 核实。不改生产代码、不改 CI、不加 `#[ignore]`、不重试、不放宽时限 | 本提交 |
| 76 | 10 | RED：只改 `crates/rurge-tun/src/tests/dispatch.rs`。先把用例的身体挪进私有的 `async fn queued_bytes_come_back()`（执行者的裁定：名字照派工单的例子），旧的那一条 `queued_bytes_are_given_back_when_the_shard_takes_the_packets` 只调它，预算照旧 8 KiB；`cargo test -j 8 --workspace --lib -- tests::dispatch::queued_bytes_are_given_back_when_the_shard_takes_the_packets` 照旧绿（1 passed，0.05 s）。再加 `#[tokio::test]` 的 `queued_bytes_are_given_back_when_the_reader_runs_ahead_of_the_shard`（当前线程的运行时），预算仍是 8 KiB；直接跑 `rurge_tun-db57ae684811af7c.exe --exact tests::dispatch::queued_bytes_are_given_back_when_the_reader_runs_ahead_of_the_shard` 三次，三次都以退出码 101 失败，都在 `crates\rurge-tun\src\tests\support.rs:314:9` panic `timed out waiting for the shards' queue to take up its bytes`（第一个 `eventually`），各 10.04 s、10.03 s、10.02 s；日志是工作区的 `task-10-red-1.log` ～ `task-10-red-3.log`。为什么应当失败：当前线程的运行时上，用例的身体在第一次让出之前就把 1024 个 SYN 都交给了应用一端，读设备的任务在片的第一轮之前把它们过完；8 KiB 只装得下 128 个，其余当场丢掉、计 `queue_full`，片一轮取完 128 个、只发一批、不被写者挡住，`queue_bytes_queued() > 0` 一直不成立（第 75 行） | 本提交 |
| 77 | 10 | GREEN：预算换成 `(3 * INGRESS_PER_WAKE + 128) * syn(1_000).capacity()`（`INGRESS_PER_WAKE` 从 `crate::tcp` 引入），`BUDGET` 常量去掉、`<= ` 那一句与它的失败信息改用 `budget`；1024 个 SYN 与 `TunOptions` 的其余几项照旧。执行者的裁定：`budget` 在 `app` 与 `syn` 之后算，所以 `GatedDevice::pair()`、`app4`、`syn`、`rig_with` 的次序变了（原先是 `BUDGET` 常量在前）。注释：旧的那一条的文档注释照旧以 "The bytes a packet holds while it waits are given back when a shard takes it" 起头；预算为什么是这个数、旧的 8 KiB 为什么不够与第 75 行，写在私有 `queued_bytes_come_back` 的文档注释里；新的一条自己一行文档注释，说它让读设备的任务跑在片前面。循环里那句改成 "more SYNs than the shard takes in the three turns it may have before the writer's queue is full"。`cargo test -j 8 --workspace --lib -- tests::dispatch::queued_bytes`：两条都 ok（2 passed，0.25 s）。直接跑 `rurge_tun-db57ae684811af7c.exe --exact <用例的全名>`，每次套 60 s 时限（`Start-Process` 与 `WaitForExit(60000)`）：`queued_bytes_are_given_back_when_the_shard_takes_the_packets` 25 次全过，`queued_bytes_are_given_back_when_the_reader_runs_ahead_of_the_shard` 25 次全过，没有一次超时。`syn(1_000).capacity()` 是多少字节：没有观察（用例里没有加打印），照第 75 行按 std 的扩容推算是 64，预算是 896 × 64 = 57344 字节 | 本提交 |
| 78 | 10 | 门禁（`f0d1d35` 的代码树上只有 `crates/rurge-tun/src/tests/dispatch.rs` 与本文件的改动；日志在工作区）：`cargo fmt --all --check` 退出码 0、`cargo clippy --all-targets -- -D warnings` 退出码 0、`cargo test -j 8 --workspace --no-fail-fast` 退出码 0，93 个结果行、5377 通过、0 失败、17 忽略（基线 93 / 5376 / 0 / 17，多的是新加的用例）。一台机器上的一次运行，不是 CI。`Cargo.lock` 一个字不变，带本机账户名的文件 26 个（`git grep -l $env:USERNAME`）。生产代码一个字没动（`git diff --stat` 只有那个测试文件与本文件）。没有改 CI、没有加 `#[ignore]`、没有重试、`WITHIN` 照旧 10 s | 本提交 |
| 79 | 10 | 修正轮 1（协调者的评审，一处注释；第 77、78 行不改写）：`crates/rurge-tun/src/tests/dispatch.rs` 的 `queued_bytes_come_back` 文档注释最后一句有两处不准。① "the writer and the device take without holding it up"：`GatedDevice` 什么也没收——它的注释（`crates/rurge-tun/src/tests/support.rs:170` ～ `:172`）说 "takes nothing to write until the test opens it: what the TUN writes waits in the writer's hands, then in its queue"，它的 `send`（`:201` ～ `:210`）先等闸门开再写；那一批是写者从队列里取走、拿在手里、停在闸门上，片没被挡住是因为长度为 1 的队列又空了。② "take all of it"：`it` 的先行词成了"what does not fit"（被丢掉的），照字面成了片取走被丢的包；要说的是装下的那 128 个。改成 "so a shard that lags behind the reader may take the 128 that fit in one turn, send one batch, which waits in the writer's hands without holding the shard up, and leave nothing waiting"，每行不超过 80 列；前面几句与别的注释没动。只改注释，没有重跑全量门禁，只跑了 `cargo fmt --all --check`（退出码 0）、`cargo clippy --all-targets -- -D warnings`（退出码 0）与 `cargo test -j 8 --workspace --lib -- tests::dispatch::queued_bytes`（2 passed，0.49 s，退出码 0）；第 78 行的门禁数是同一份代码的数，照旧成立 | 本提交 |
| 80 | 10 | 协调者核过任务 10（`76f7017`，第 76 ～ 78 行）与它的修正轮 1（`1261c71`，第 79 行）。两个提交只改 `crates/rurge-tun/src/tests/dispatch.rs` 与本文件：生产代码、`tests/support.rs`、CI 都没动，没有加 `#[ignore]`，`WITHIN` 照旧 10 s。RED：工作区的 `task-10-red-1.log` ～ `task-10-red-3.log` 三份都在 `support.rs:314:9` panic、信息与 CI 上的相同、各约 10 s，是第 75 行预计的那一种失败；这只说明在当前线程的运行时上旧的预算必红，CI 那一次的 macOS 上各任务实际是什么先后没有观察过（任务日志要登录才能读）。循环：会话里那条循环命令（`Start-Process` 与 `WaitForExit(60000)`）的输出是两条各 `ok=25 bad=`，没有失败、没有超时。门禁：协调者从 `gate-task-10-test.log` 重新数过，93 个结果行、5377 / 0 / 17、`exit=0`；日志里的 `error: cannot bind http-api on 127.0.0.1:…` 是一条用例故意占着端口时的输出，任务 8 的门禁日志里同样有（端口不同）；fmt 与 clippy 的日志都是 `exit=0`。照代码核过新预算的推理：读设备的任务只在队列装满时丢（`dispatch.rs:147` 按容量预留），SYN 的容量都一样，所以丢的那一刻队列里正好有 896 个；片被挡住之前一共至多取 3 × 256 = 768 个（`tcp/mod.rs:42` 的 `INGRESS_PER_WAKE`，`shard.rs:1185` 一轮交一批，写者一轮从每个队列至多取 `writer_queue` 批，`GatedDevice` 挡住 `send`），被挡住之后不再取，所以至少还剩 128 个；从没丢过的话 1024 个都进了队列，至少剩 256 个。这在任何先后下都成立，`<= budget` 那一句照旧有意义。修正轮 1 是协调者评审出的一句注释（第 79 行）：只改注释，跑了 fmt、clippy 与这两条用例。计划弯引号 0、LF，`git diff --check` 干净，带本机账户名的文件 26，第 76 ～ 79 行只加不改。修正在 macOS 的 CI 上管不管用，要看这些提交推送之后的那一次运行。**任务 10 过了** | 本提交 |
| 81 | 10 | 协调者读回运行 41402787066 之后改的文档（本提交，只改文档，没有跑门禁）。`docs/status.md` 在「M4b（请求记录的完整）」一节之后、附录之前加「M1 ～ M4 推送之后的 CI（2026-11-02）」一节：第一次被 CI 编译、运行的，失败的那一条、原因与修正（第 75 ～ 80 行），没有变的，注解，各任务，文档怎样订正的，下一步；它订正阶段 4 各节与 `docs/testing.md` 阶段 4 各节开头、`docs/README.md` 各项里"CI 没有见过"一类的说法（历史的段落不逐处改）。`CLAUDE.md`「现在在哪里」第 15 ～ 17 行。`README.md` 第 36、52、60、66 行与 `README_en.md` 第 36、51、59、65 行，两份意思一致。`docs/requirements.md` 第 531 行（第 7 节阶段 4 的状态）按日期接一句：M4b 做完（M4b 收尾时没有写这一行）与这次 CI。兼容清单第 5 行：新的「最近一次」与阶段 4 的状态（"M4 还剩 M4b"那一句改成 M4 做完、推送与这次 CI）；表里的行一行也没有改。`docs/api/phase4.md` 末尾加同名的一节，订正各节开头与「没有覆盖的」里"CI 没有见过"的说法。`docs/testing.md` 第 286 行（「阶段 3 / M3a 起：`rurge-tun`」）末尾加一句：要片或写者被挡住才成立的用例，前提在各任务的任何先后下都要成立。新写的句子里引的原文（M2c 一节那一句、`docs/api/phase4.md` 第 5 行与各节开头、八处「没有覆盖的」）都在原处核过。行尾照旧（`CLAUDE.md`、两份 README 与需求文档 `w/crlf`，其余 `w/lf`），`git diff --check` 干净，带本机账户名的文件 26，本文件弯引号 0 | 本提交 |
| 82 | 11 | 协调者的裁定：项目所有者推送了 `f0d1d35` ～ `a262d4c`（`origin/main` 在 `a262d4c`）。CI 运行 41411445901（第 17 次，2026-11-02 08:13:15Z ～ 08:31:21Z）没有全绿：`bench` 与 macOS 的 `check` 都通过，第 75 行那一条 `rurge-tun` 用例这一次在 macOS 上通过（测试一步 689 s）；Linux 与 Windows 的 `check` 的 fmt 与 clippy 都过，测试一步各有失败。注解（公开可读；任务日志要登录才能读）列出三条。① Windows 两条，都在 `crates/rurge-engine/tests/egress.rs`：`every_kind_of_dns_upstream_asks_the_hook_before_it_dials` 在 `:166:58`、`an_egress_error_stops_each_kind_before_anything_is_sent` 在 `:439:91`，都是 ``called `Result::unwrap()` on an `Err` value: Os { code: 10013, kind: PermissionDenied, message: "An attempt was made to access a socket in a way forbidden by its access permissions." }``。两处都先在 TCP 上 bind `127.0.0.1:0`，再在同一个号上 bind UDP（`truncated()`，与 `Reach::Udp | Reach::Truncated` 那一支）：系统给 TCP 的临时端口，同一个号在 UDP 上不保证能用。10013 是 `WSAEACCES`：Microsoft 的 "Using SO_REUSEADDR and SO_EXCLUSIVEADDRUSE" 一页说，另一个用户账户的套接字绑在通配地址的这个端口上时，第二次 bind 到具体地址得到它；号落在系统保留的号段里也会得到它（这一条只见于第三方的报告）。这一次是哪一种没有证据。这两条用例是阶段 3 / M4b 任务 9 写的（`423a62e`），第 13 ～ 16 次运行都包含它们，Windows 的 `check` 在第 13 ～ 16 次都通过，所以是偶发。`MockDns::start`（`crates/rurge-dns/src/testing.rs`）为同一件事改过（`94cfd9e`，2026-10-18）：至多 64 轮，两种次序轮流，都不成时 panic 的文本说出两种各自最后的错误（`no_port`）。同样的写法还在 `crates/rurge-dns/src/upstream/udp.rs` 的两条单元测试里（`truncated_answer_is_retried_over_tcp` 与 `a_truncated_answer_to_a_query_sent_under_another_id_is_retried_as_it_came`），CI 还没有在它们上面失败过。② Linux 一条：`-p rurge-interop --test snell :: obfs_http_against_sing_box` 在 `tests/interop/tests/common/mod.rs:101:6` panic：`the tunnel is established: Io(Os { code: 111, kind: ConnectionRefused, message: "Connection refused" })`，是 `roundtrip` 的第一步（`tcp(&profile, &["Obfs"])`）。这是客户端自己连 sing-box 时的系统错误：sing-box 拒绝请求时，错误是 `crates/rurge-proto/src/snell/tunnel.rs` 的 `refused` 拼的 `snell: the server refused: …`，不带系统的错误码。所以那一刻 `sb.port(0)` 上没有人在听，而 `Reference::wait_ready`（`tests/interop/src/lib.rs`）之前连上过这个号（每次连之前先问 `try_wait`）。是 sing-box 在就绪之后退出了，还是就绪的探测连上的是别人，照注解分不出：用例失败时没有打印别的，sing-box 的日志没有进注解。这条用例从阶段 2 / M6b（`15e2d33`）就有，Linux 的 `check` 在第 13 ～ 16 次都通过。修法是插入任务 11，派给 Pi，只改测试的代码。(a) 把 `MockDns::start` 里找号的循环原样抽成 `rurge_dns::testing` 里的一个公开函数；`MockDns::start`、上面两条 `rurge-dns` 单元测试与 `egress.rs` 的两处都用它。(b) `tests/interop/src/lib.rs` 的 `Reference` 在用例失败时（`std::thread::panicking()`）由 `Drop` 先问子进程是不是自己退出了，杀掉之后用 `eprintln!` 说出这一点与日志的最后几行。文本有上限，放得进注解的 60 行、3000 字。原因留给下一次失败的注解。两处都没有能在本机变红的用例：(a) 的失败要碰上系统给的号，(b) 只是多说话。不改生产代码、不改 CI、不加 `#[ignore]`、不重试用例、不放宽时限 | 本提交 |
| 83 | 11 | 甲（本提交）：把 `MockDns::start` 里找号的循环原样抽成 `crates/rurge-dns/src/testing.rs:253` 的 `pub async fn tcp_and_udp_on_one_port() -> (TcpListener, UdpSocket)`：至多 64 轮、单数轮先 TCP 后 UDP、双数轮先 UDP 后 TCP、0 号端口自己 bind 失败也记下、都不成时 `panic!` 的文本由 `no_port` 拼出，一个字没有改；`MockDns::start` 的那段注释挪成它的文档注释，另加一句证据：CI 运行 41411445901 的 Windows 上，在 TCP 监听器的号上 bind UDP 得到了 `WSAEACCES`（第 82 行①）。`MockDns::start`（`crates/rurge-dns/src/testing.rs:306`）改成调它，再自己取 `local_addr()`、把 UDP 套接字包进 `Arc`，别的部分不动；`no_port` 与它的用例 `no_port_tells_the_last_error_of_each_kind` 一字未改。四处用它：`start`；`crates/rurge-dns/src/upstream/udp.rs:444` 的 `truncated_answer_is_retried_over_tcp` 与 `:569` 的 `a_truncated_answer_to_a_query_sent_under_another_id_is_retried_as_it_came`（监听器与 UDP 套接字都从它拿，`port` 照旧取监听器的 `local_addr()`，回声、带 TC 的答复、断言一字未动，第二条的套接字照旧包进 `Arc`，那句 `TCP echo bound first` 的注释改成与新写法相符的一句）；`crates/rurge-engine/tests/egress.rs` 的 `truncated()`（`:171`，监听器交给新抽出的 `serve_silently`（`:133`，同步函数，里面 `tokio::spawn`），UDP 一端回 TC 的答复不动），`silent_tcp()`（`:147`）先 bind 再交给 `serve_silently`、签名与行为不变，`an_egress_error_stops_each_kind_before_anything_is_sent`（`:437`，`Reach::Tcp` 照旧只 bind 一个 TCP 监听器，`Reach::Udp | Reach::Truncated` 的监听器与 UDP 套接字从 `tcp_and_udp_on_one_port()` 拿，`addr` 照旧从监听器取，之后的断言不动）；引入写在 `egress.rs:13` 的 `use rurge_dns::testing::tcp_and_udp_on_one_port;`，`udp.rs` 的测试模块把它加进已有的 `crate::testing::{…}`，两处不再用到的 `TcpListener`（`udp.rs`）从 `use` 里删掉。生产代码的行为一字不变（`MockDns::start` 的轮数、次序、记下的错误与 panic 的文本照旧：新函数与原循环逐行对照过，只有 "找到之后" 存的是 `(listener, udp)` 而不是 `(listener, addr, Arc::new(udp))`）；`tcp_and_udp_on_one_port` 的可见性是 `pub`、`testing.rs` 本来就在编译进去的模块里。不要求先红（失败要碰上系统给的、UDP 上不能用的号，本机碰不上）：`cargo test -j 8 --workspace --lib -- testing::tests`、`… --lib -- upstream::udp::tests`、`cargo test -j 8 --workspace --test egress` 都过，`exit=0`（`task-11-jia-testing.log`、`task-11-jia-udp.log`、`task-11-jia-egress.log`）。循环：四条用例直接跑测试二进制、`--exact`、每次 60 s 时限，各 25 次，`ok=25 bad=0 timeout=0`（`task-11-jia-loop.log`）；这只说明改写没有弄坏它们，CI 上的那种失败本机碰不上 | 本提交 |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
