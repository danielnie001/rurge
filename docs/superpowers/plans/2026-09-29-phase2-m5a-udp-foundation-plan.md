# 阶段 2 / M5a「UDP 地基」Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** rurge 的 UDP 真正可用的第一步：SOCKS5 监听支持 UDP ASSOCIATE；引擎按"关联 + 目标"为每条 UDP 流匹配规则、写请求记录、空闲回收，按"关联 × 出站"共用载体（全锥 NAT）；DIRECT、REJECT、`socks5` / `socks5-tls` / `external`（`udp-relay`）与 `underlying-proxy` 链承载 UDP；`block-quic` 与 `udp-policy-not-supported-behaviour` 生效。`trojan` / `vmess` / `anytls` 的 UDP 在 M5b，`wireguard` 与其余在 M5c。

**Architecture:** `rurge-net` 新增按包收发、带地址的 UDP 载体 `PacketSocket` 与 `Connector::open_udp`（`DirectConnector`：每个地址族一个未连接的 socket）。`rurge-proto` 的 `Outbound` 多两个方法 `udp()` / `open_udp()`；DIRECT 与 SOCKS5（`udp-relay`，含 `external`）实现它们；`rurge-policy` 的 `ChainConnector::open_udp` 让 UDP 经底层策略。`rurge-inbound` 的 SOCKS5 监听处理 UDP ASSOCIATE，经 `Dialer::associate` 把关联（`UdpClient`）交给引擎；`rurge-engine` 的 `udp` 模块为每个目标开一条流（自己的会话句柄与请求记录），首包匹配规则与策略，再按出站取或开载体，载体上任何来源的回包都送回客户端。

**Tech Stack:** Rust 1.89 / edition 2024；不新增任何依赖（`tokio`、`tokio-util`、`socket2` 已在各 crate 的依赖里）。

**Spec:** `docs/superpowers/specs/2026-09-29-phase2-m5-udp-design.md`（M5-D1 ～ D11；第 4、5、6 节；第 7 节 DIRECT / `socks5` / `socks5-tls` / `external` 行；第 9 ～ 12 节中 M5a 的部分；第 15 节 V1 ～ V5；第 16 节 M5a 草图）与总设计 `docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`。与本计划「计划期决定」表不一致处，以该表为准；写计划时一并写进设计文档新增的第 17 节。

## Global Constraints

- MSRV **1.89**，edition 2024。`unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 的两个函数例外（本计划不碰）。**本计划不新增任何 unsafe**。
- 依赖方向不变：`rurge-proto → rurge-net → rurge-config`；`rurge-inbound` 经 `Dialer` 与引擎相连，不认识出站；`rurge-policy` 不依赖协议实现。**不新增依赖**。
- **测试绝不碰公网**：只用回环 + 端口 0（或刚刚空闲的端口）+ 有界等待；不用固定 sleep 当同步手段（轮询 + 截止时间；只有"断言这段时间里什么也没发生"时才等一段固定时间）。UDP 回显、假 SOCKS5 服务端与测试辅助程序都只在 127.0.0.1 上。**任何带 `url-test` / `fallback` / `load-balance` / `smart` 组的测试配置，`proxy-test-url` 与 `internet-test-url` 都必须指向回环**——引擎用例的 `Profile::text` 已默认指向 `http://127.0.0.1:9/`，不要删掉。
- **测试与任何命令都不得修改本机的系统代理、注册表、网络设置，不得注册真实服务 / 计划任务**：不在 CLI 测试夹具之外运行 `rurge run --system-proxy`；不运行不带 `--dry-run` 的 `rurge service install | uninstall`。
- **不在本机下载或安装任何东西**（不装 sing-box，不 `rustup target add`、不 `cargo install`）。
- **载荷与凭据永不外泄**：UDP 载荷不进日志、错误文本与请求记录；SOCKS5 的用户名 / 口令、订阅内容、测试 URL 沿用此前的规则。
- rurge 专有的运行时选项只经命令行与环境变量提供，不扩展 Surge 配置格式（FR-CFG-17）。与 Surge 的每一处行为差异都登记进 `docs/surge-compatibility-matrix.md`（Task 7）。
- 日志、错误文本、CLI 输出、代码注释用英文；文档与提交标题用中文。注释密度、命名、惯用法与相邻代码保持一致；注释里不写评审轮次的标签。
- 提交信息结尾两行：`Co-Authored-By: <执行者自己的署名>` 与 `Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th`。**不 push、不 merge**，不 amend。
- 每个任务结束的门禁（全部通过才算完成）：

  ```bash
  RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check \
    && cargo clippy --all-targets -- -D warnings \
    && timeout 1500 cargo test --workspace --no-fail-fast
  ```

  `timeout` 不能省：`rurge-dns` 的一个用例曾让测试进程以 100% CPU 空转数小时（M3a「延后事项」#20）。测试二进制异常退出而没有失败用例时（`STATUS_ACCESS_VIOLATION`、`STATUS_HEAP_CORRUPTION` / `0xc0000374`、段错误——本机已知的既有问题，M3b 计划 P21），或整轮被 `timeout` 杀掉时，重跑一次并保留两次的日志，**不要在任务里去修它**。已知偶发失败的计时类用例（`rurge-dns` 的 `a_partial_result_completes_aaaa_in_the_background` 与 `bootstrap::tests::stale_entries_are_served_and_refreshed_once`、`rurge` 的 `run::watch_reloads_rules_on_change`）同样重跑。**编译器（`rustc` / 链接器）自己崩溃、报 PDB 损坏时多半是磁盘满了**（写本计划时遇到过：只剩 11 GB）：先看 `df -h /d`，删 `target/debug/incremental` 再重跑。
- 第三方 crate 的 API 以本机源码为准：`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`（`tokio-1.53.1`、`socket2-*`）。
- 本机的 bash 处理不了超过约 8 KB 或含反斜杠的 heredoc（`\\` 会被改写）：新文件与含反斜杠的改动一律用写文件的工具落盘，不用 heredoc。

## Review Focus

设计没有逐条写到、而最可能伤到使用者的五类输入或失败方式；每一条都在负责它的任务里配了用例。

1. **发往没人监听的端口的 UDP**（游戏探测服务器、DNS 发给关掉的服务器）：Windows 会在同一个 socket 的下一次接收上报 `ConnectionReset`（ICMP 端口不可达），不能让它把整个载体——连同同一关联里别的流——弄死。用例：Task 1 `a_closed_port_does_not_stop_a_direct_carrier`；Task 4 `a_closed_port_does_not_break_the_carrier`（写本计划时就是它发现了这个问题，P4）。
2. **客户端退出或断线**：控制连接一断，关联里的每条流都要结束、记录收尾，载体随之关闭，不留任务与 socket。用例：Task 3 `closing_the_control_connection_ends_the_association`；Task 4 `closing_the_control_connection_ends_every_flow`；Task 2 `a_closed_control_connection_ends_the_association`（上游的关联断了）。
3. **UDP 洪泛**（扫描器、失控的 P2P 客户端向成千上万个地址发包）：每个关联的流数有上限，超出的丢弃，不能把内存与请求记录撑爆。用例：Task 4 `an_association_has_at_most_1024_flows`。
4. **P2P 联机与语音**（对方从另一个地址发来）：全锥——载体上任何来源的回包都送回客户端。用例：Task 1 `a_direct_carrier_talks_to_any_address`；Task 4 `anyone_may_answer_the_carrier`；Task 2 `udp_goes_through_the_association`（经 SOCKS5 上游）。
5. **浏览器的 QUIC 与短查询**：QUIC 按 `block-quic` 阻断后浏览器回落 TCP，别的 UDP 443 不受影响；DNS 流收到回答后很快回收。用例：Task 5 `block_quic_drops_quic_and_nothing_else`、`per_policy_block_quic_follows_the_terminal_policy`、`which_quic_flows_are_blocked`、`a_quic_initial_is_recognised`；Task 4 `when_a_flow_is_reclaimed`。

另有几条同样配了用例、但不那么常见的：不支持 UDP 的策略（Task 4 `a_policy_without_udp_rejects`，Task 5 `a_policy_without_udp_may_fall_back_to_direct`）；链路的底层不支持 UDP（Task 6 `a_chain_without_udp_fails_the_flow`）；陌生人冒用关联的端口、分片包（Task 3 `strangers_and_fragments_are_ignored`）。

## 计划期决定

写计划时对照设计、Surge 手册（`profile/general.html`、`policies/udp.html`、`policies/socks5.html`、`policies/external.html`、`rules/protocol-and-network.html`）、RFC 1928 / 9000 / 9369、tokio 1.53.1 源码与本仓库源码核对后定下的事；与设计文档文字不同的，写进设计文档第 17 节。

**本计划里的代码不是凭空写的。** 全部 7 个任务的改动在仓库的一份副本上按任务顺序真实做了一遍（副本用自己的构建目录，不与本仓库的 `target/` 混用），最后一次全工作区门禁见 Task 7 的 Step 5。计划里新文件的全文取自副本上该任务的提交，修改处的"把 … 换成 …"由脚本从相邻两个任务提交的差异生成，并在拼好之后按计划的顺序套到开工前的源码上逐字核对过——计划文本与验证过的代码一字不差。每个任务 Step 2 的"预期失败"是只把该任务的用例块（及写明的前置改动）套到上一个任务的状态上、真实跑出来的。做的过程中发现的问题（P4 的 `ConnectionReset`、P10 的用例时序）已经改在了它们所属的任务里。

| # | 事项 | 决定与依据 |
| - | ---- | ---------- |
| P1 | V1：SOCKS5 UDP ASSOCIATE（RFC 1928 §4、§7） | 应答里的 BND 是客户端连到的本机地址（控制连接的 `local_addr`，监听在 `0.0.0.0` 时也不会回 `0.0.0.0`）加一个新开的 UDP 端口。只收控制连接那个客户端 IP 的包（`::ffff:a.b.c.d` 视同 IPv4）；端口取请求里声明的，声明为 0 时取第一个包的源端口。`FRAG ≠ 0` 的包、未知地址类型、不是主机名的名字一律丢弃。控制连接上之后收到的数据丢弃，读到 EOF 或出错即结束关联。访问限制（`restrict`，只收局域网）在 TCP 接入时已判定，UDP 端口只认那个客户端，不另判。`Dialer::admit_udp()` 先问：`NotSupported`（不带 UDP 的 dialer，如各单元测试的假 dialer）回 0x07，`Busy`（关联数到上限）回 0x01 |
| P2 | V2：入站与引擎的接口 | `rurge-inbound` 新增 `UdpClient`（`recv` / `send`，两个可并行）与 `Dialer::admit_udp` / `Dialer::associate(client, session, closed)`，都带默认实现，已有的 dialer 不用改。引擎侧：`Engine` 用 `Arc::new_cyclic` 记住自己的 `Weak`，每条流是一个任务（`TaskTracker` 上），关联结束（`closed`，或 `cancel_sessions`）时流随之结束。每条流有自己的 `SessionHandle`：`up` / `down` 是载荷字节，"出站就绪"是载体可用，"首字节"是第一个回包。请求记录与 API 的 JSON 新增 `transport`（`tcp` / `udp`） |
| P3 | `PacketSocket` 的形状 | 设计 4.1 写的是轮询式（`poll_send_to` / `poll_recv_from`）。改为异步方法（`send_to` / `recv_from` 返回 `BoxFuture`）加一个 `resolve`：每个载体有自己的收包任务，不需要一个任务同时轮询几个载体；而发送可能要做异步的事（DIRECT 解析名字、M5b 的协议往 TLS 流里写）。`resolve(to)` 给出发往 `to` 的包实际去的地址：DIRECT 在此解析名字，别的协议原样交回（由服务器解析） |
| P4 | V3：DIRECT 的载体 | `ip-version` 允许的每个地址族各一个未连接的 socket，绑定到未指定地址的 0 端口（IPv6 的设 `only_v6`），网卡与 TOS 经 `SocketHook`；本机开不出某一族的 socket 时略去它，两族都开不出才失败。名字按 `ip-version` 的计划解析，取有 socket 的那一族的第一个地址。引擎在建流时调用一次 `resolve`，此后这条流固定发往那个地址，回包按真实地址归流。**写本计划时发现**：Windows 上发往没人监听的端口后，下一次 `recv_from` 报 `ConnectionReset`（ICMP 端口不可达），原来的收包循环因此结束、整个载体失效——`recv_from` 现在跳过这个错误（同样的处理在入站的关联端口与假服务端里） |
| P5 | V4：SOCKS5 的 UDP 客户端 | 对服务器发 `UDP ASSOCIATE 0.0.0.0:0`（让中继认第一个包的来源），应答的 BND 就是中继地址；是未指定地址时改用策略的服务器地址（常见实现的读法）。控制连接由一个任务持有，读到 EOF 即令载体失效（发送报 `BrokenPipe`，接收报 `socks5: the proxy closed the UDP association`）；载体被丢弃时任务随之中止、控制连接关闭。数据报经连接器的 `open_udp` 发出：DIRECT，或 `underlying-proxy` 的底层策略。`socks5-tls` 的 UDP 是明文 UDP（只有控制连接是 TLS，RFC 1928 本来如此）。sing-box `socks` 入站的 UDP 只在 CI 上验证 |
| P6 | V5：QUIC 的识别 | 目标 UDP 443，首包首字节高两位为 `11`（长首部与固定位）、版本号不为 0、包类型是 Initial（v1 与 draft 为 0，v2 `0x6b3343cf` 为 1），且至少 1200 字节（客户端的 Initial 按 RFC 9000 §14.1 补到 1200）。识别放在引擎的 `sniff` 模块（设计草图写在 `rurge-net`：只有引擎用它）。首包识别出 QUIC 时会话的 `protocol` 为 `QUIC`，否则为 `UDP`；STUN 与 DNS 的嗅探没做（延后事项） |
| P7 | `PROTOCOL` 规则按传输层 | 手册："`PROTOCOL,UDP` 也匹配 QUIC 与 STUN"。匹配器改为：`PROTOCOL,UDP` 匹配每条 UDP 流，`PROTOCOL,TCP` 匹配每个 TCP 会话（此前 TCP 会话从不带 `TCP` 这个协议值，这条规则从不命中），其余协议照旧按嗅探出的值 |
| P8 | 全局 `block-quic` 的四个值 | 设计 6.2 说配置层"只认前两个"——不对，`BlockQuicGlobal` 在阶段 1 就有全部四个值，这里不改配置层（设计第 17 节订正） |
| P9 | 全锥的归流（设计 5.3） | 回包的来源等于某条流发往的地址时计入那条流，否则计入这个载体上最早的、还在的一条流。设计说要在该记录上"标注收到其它来源的包"——不标：请求记录能写说明的只有 `error`，写在那里会被读成失败（延后事项） |
| P10 | 流的并发与失败 | 每条流自己做规则匹配、策略解析与开载体，互不阻塞；开载体期间到达的包在这条流的队列里等（最多 64 个，满了丢弃）。同一关联开同一出站的载体只开一次（按出站对象加锁），开失败不缓存，下一条流再试。失败或被拒的流留在表里丢弃发往它的包，直到 60 秒无包再回收（免得每个包都重新匹配一次）。关联结束时还在路由的流记为完成。**写本计划时发现**：用例若发完包立即关掉关联，还在路由的流随之记为完成、判定没来得及做——端到端用例先等流"有了策略"（`routed`）再关 |
| P11 | 上限（M5-D11） | 每个关联最多 1024 条流、全进程最多 4096 个关联：超出的新流丢弃；关联数到上限时新的 UDP ASSOCIATE 回 0x01；两种情况都每分钟至多一条 `warn`。失败的流也占名额，直到它回收 |
| P12 | `udp-policy-not-supported-behaviour` | `REJECT`：记录为 REJECT，说明 `policy does not support UDP`；`DIRECT`：改用 DIRECT 的载体，说明写 `policy does not support UDP; sent through DIRECT`（记录不是失败）。判断看终端出站的 `udp()`：`socks5` / `external` 没写 `udp-relay=true` 时同样算不支持（手册：必须显式打开） |
| P13 | `block-quic` 的判定 | 策略的设置取终端策略（策略链最后一个名字）的 `block-quic`，内置策略（DIRECT）视为 `auto`；`auto` 对代理阻断、对 DIRECT 放行。被阻断的流记为 REJECT，说明 `QUIC blocked`；UDP 流不计入 REJECT 的自动升级（阶段 1 的"30 秒 50 次"只对 TCP）。策略级 `block-quic` 的 `W0029` 退役 |
| P14 | `external` 的 `udp-relay` | `ExternalSpec` 多一个 `udp_relay`（`W0029` 退役）；开载体时照 M4c 的规则拉起程序、连本机端口（500 ms 一次、最多 6 次），发 UDP ASSOCIATE，数据报经直连的 UDP 发往程序给出的中继（未指定地址时是 127.0.0.1）。测试辅助程序 `socks-helper` 加上 UDP ASSOCIATE |
| P15 | 链式 UDP（设计 4.3） | `ChainConnector::open_udp` 解析底层策略（与 TCP 相同），底层出站 `udp()` 不支持时报 `via <底层>: the underlying policy cannot carry UDP`（`Unsupported`），否则交给它的 `open_udp`。设计 4.3 写的"把包载体包成到固定服务器的 `Datagram`（`ChainConnector::connect_udp`）"留给需要它的 M5c（WireGuard 的 peer 载体）；M5a 里 SOCKS5 的中继段直接用包载体 |
| P16 | 任务的切分 | 设计第 16 节草图的 7 个任务，两处挪动：SOCKS5 的 UDP 客户端与 `socks5` 出站的 `udp-relay` 都在 Task 2（客户端就在 `socks5.rs` 里）；Task 6 是 `external` 的 `udp-relay`、链式 UDP 与测试辅助程序 |

## 承接事项

之前计划「延后事项」表里标给 M5 的条目，及仍然有效的既有现象。

| # | 来源 | 事项 | 处理 | 任务 |
| - | ---- | ---- | ---- | ---- |
| C1 | M1a / M1b 延后 | `udp-relay`、UDP 载体（链式） | `socks5` / `socks5-tls` 的 `udp-relay`（Task 2）、`external` 的与链式 UDP（Task 6） | 2、6 |
| C2 | M4c #5 | `external` 的 `udp-relay` | P14 | 6 |
| C3 | M3c #6、M4b #2 / #15 / #24、`dns-follow-interface`、`test-udp` | `smart` 计入 UDP、WireGuard 的 UDP 与 `underlying-proxy`、告警限频、说明文本、按网卡的 DNS、UDP 测速 | 不在本计划：M5c（设计 1.4） | — |
| C4 | M3b #7（P21） | 测试二进制偶发崩溃 | 照旧：门禁遇到就重跑 | — |

## File Structure

新建：

| 文件 | 职责 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-inbound/src/udp.rs` | `Socks5UdpClient`（关联的 UDP 端口）、UDP 头的解析与封装，与用例 | 3 |
| `crates/rurge-engine/src/udp.rs` | UDP 流水线：关联、流、载体、全锥归流、回收、上限、`block-quic` 判定，与用例 | 4、5 |
| `crates/rurge-engine/tests/udp.rs` | 经引擎的端到端用例 | 4 ～ 6 |

修改：

| 文件 | 改动 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-net/src/connector.rs` | `PacketSocket`、`Connector::open_udp`、DIRECT 的载体（1）；`Target` 可哈希（4） | 1、4 |
| `crates/rurge-config/src/{session.rs, spec/mod.rs, spec/common.rs, spec/external.rs}`、`tests/policy_spec.rs`、快照 | `SessionInfo::udp`（1）；`udp-relay` 与 `block-quic` 的 `W0029` 退役（2、5、6）；`ExternalSpec.udp_relay`（6） | 1、2、5、6 |
| `crates/rurge-proto/src/{outbound.rs, lib.rs, direct.rs, addr.rs, socks5.rs, external.rs, testing/socks5.rs}` | `UdpSupport` 与两个方法、DIRECT 与 SOCKS5 的 UDP、`FakeSocks5` 的 UDP（2）；`external` 的 UDP（6） | 2、6 |
| `crates/rurge-inbound/src/{lib.rs, session.rs, socks5.rs}` | `UdpClient`、`UdpAdmission`、`Dialer` 的两个方法、UDP ASSOCIATE | 3 |
| `crates/rurge-engine/src/{engine.rs, lib.rs, observe.rs, sniff.rs}`、`tests/common/mod.rs` | 关联的接入与上限、`transport`（4）；QUIC 识别（5） | 4、5 |
| `crates/rurge-api/src/routes/requests.rs` | JSON 的 `transport` | 4 |
| `crates/rurge-rules/src/matcher.rs` | `PROTOCOL` 按传输层 | 5 |
| `crates/rurge-policy/src/cell.rs` | `ChainConnector::open_udp` | 6 |
| `tests/external/{src/bin/socks-helper.rs, tests/common/mod.rs, tests/outbound.rs}` | 辅助程序的 UDP ASSOCIATE 与用例 | 6 |
| `tests/interop/{tests/sing_box.rs, README.md}` | 对 sing-box `socks` 入站的 UDP 用例 | 7 |
| 文档（兼容性清单、两份 README、`CLAUDE.md`、`docs/api/phase1.md`、手工验收） | 见 Task 7 | 7 |

## 任务一览

| 任务 | 交付物 | 依赖 |
| ---- | ------ | ---- |
| 1 | `rurge-net`：`PacketSocket`、`Connector::open_udp`、DIRECT 的载体；`SessionInfo::udp` | — |
| 2 | `rurge-proto`：`Outbound::udp` / `open_udp`、DIRECT 与 `socks5` / `socks5-tls` 的 UDP（`udp-relay`）、`FakeSocks5` 的 UDP ASSOCIATE（承接 C1） | 1 |
| 3 | `rurge-inbound`：SOCKS5 UDP ASSOCIATE 与 `Dialer` 的 UDP 接口 | 1 |
| 4 | `rurge-engine`：UDP 流水线（流、载体、全锥、回收、上限、请求记录的 `transport`）与端到端用例 | 2、3 |
| 5 | `block-quic`、`udp-policy-not-supported-behaviour`、QUIC 识别、`PROTOCOL` 按传输层 | 4 |
| 6 | `external` 的 `udp-relay`、链式 UDP、测试辅助程序的 UDP（承接 C1、C2） | 5 |
| 7 | 对 sing-box 的 UDP 互操作与文档 | 6 |

---

### Task 1: `rurge-net`——`PacketSocket`、`Connector::open_udp`、DIRECT 的载体

UDP 载体的抽象（设计 4.1，P3）：`PacketSocket` 按包收发，每个包带目的地址或来源地址，发与收可以在两个任务里同时进行；`resolve` 给出发往某个目标的包实际去的地址。`Connector` 多一个 `open_udp`（默认"不支持"）。`DirectConnector::open_udp` 开出 DIRECT 的载体（P4）：`ip-version` 允许的每个地址族一个未连接的 socket，网卡与 TOS 照策略；名字在 `resolve` / `send_to` 里解析；Windows 在 ICMP 端口不可达之后下一次接收报的 `ConnectionReset` 被跳过，不让一个关着的端口弄死整个载体。`SessionInfo::udp` 是 UDP 流的会话构造。

**Files:**
- Modify: `crates/rurge-net/src/connector.rs`（`PacketSocket`、`BoxedPacketSocket`、`Connector::open_udp`、`DirectConnector::open_packet` 与 `DirectPacket`，与用例）
- Modify: `crates/rurge-config/src/session.rs`（`SessionInfo::udp`）

**Interfaces:**
- Consumes: 既有的 `rurge_net::connector::{Target, ConnectOpts, Connector, DirectConnector, Resolve}`、`rurge_net::socket::{SocketOpts, SocketHook, Family, plan_addresses}`、`rurge_config::spec::IpVersion`。
- Produces:
  - `pub trait PacketSocket: Send + Sync { fn resolve<'a>(&'a self, to: &'a Target) -> BoxFuture<'a, io::Result<Target>>` (默认原样交回) `; fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>>; fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>>; }`
  - `pub type BoxedPacketSocket = Box<dyn PacketSocket>;`
  - `Connector::open_udp<'a>(&'a self, opts: &'a ConnectOpts) -> BoxFuture<'a, io::Result<BoxedPacketSocket>>`（默认 `Unsupported`，文本 `this connection cannot carry UDP`）；`DirectConnector` 实现它
  - `rurge_config::session::SessionInfo::udp(dst_host: HostName, dst_port: u16) -> SessionInfo`（`transport = Udp`，其余同 `SessionInfo::tcp`）

- [ ] **Step 1: 先写用例**

`crates/rurge-net/src/connector.rs`——把

```rust
        assert_eq!(err.to_string(), "this connection cannot carry UDP");
```

换成

```rust
        assert_eq!(err.to_string(), "this connection cannot carry UDP");
        let err = TcpOnly
            .open_udp(&ConnectOpts::default())
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    }

    /// DIRECT's carrier sends to any address, by address and by name, and
    /// hears from any address — not only from the one it sent to (full cone).
    #[tokio::test]
    async fn a_direct_carrier_talks_to_any_address() {
        let echo = udp_echo().await;
        let c = DirectConnector::new(Arc::new(Fixed(vec![ip("127.0.0.1")])));
        let carrier = c.open_udp(&ConnectOpts::default()).await.unwrap();
        let by_name = Target::new(HostName::parse("echo.test"), echo.port());
        let resolved = carrier.resolve(&by_name).await.unwrap();
        assert_eq!(resolved, Target::new(HostName::Ip(echo.ip()), echo.port()));
        let mut buf = [0u8; 64];
        for to in [&by_name, &resolved] {
            carrier.send_to(b"ping", to).await.unwrap();
            let (n, from) = carrier.recv_from(&mut buf).await.unwrap();
            assert_eq!((&buf[..n], &from), (&b"ping"[..], &resolved));
        }
        // a stranger writes to the carrier's port, never having been written to
        let seen = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let seen_addr = seen.local_addr().unwrap();
        carrier
            .send_to(
                b"who",
                &Target::new(HostName::Ip(seen_addr.ip()), seen_addr.port()),
            )
            .await
            .unwrap();
        let (n, carrier_addr) = seen.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"who");
        let stranger = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let stranger_addr = stranger.local_addr().unwrap();
        stranger.send_to(b"hello", carrier_addr).await.unwrap();
        let (n, from) = carrier.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"hello");
        assert_eq!(
            from,
            Target::new(HostName::Ip(stranger_addr.ip()), stranger_addr.port())
        );
    }

    /// A datagram to a port nobody listens on does not stop the carrier:
    /// Windows reports the ICMP "port unreachable" on the next receive.
    #[tokio::test]
    async fn a_closed_port_does_not_stop_a_direct_carrier() {
        let echo = udp_echo().await;
        let closed = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let closed = closed.local_addr().unwrap();
        let c = DirectConnector::new(Arc::new(SystemResolve));
        let carrier = c.open_udp(&ConnectOpts::default()).await.unwrap();
        let to = |addr: SocketAddr| Target::new(HostName::Ip(addr.ip()), addr.port());
        carrier.send_to(b"lost", &to(closed)).await.unwrap();
        // a window for the unreachable answer to arrive
        tokio::time::sleep(Duration::from_millis(200)).await;
        carrier.send_to(b"ping", &to(echo)).await.unwrap();
        let mut buf = [0u8; 64];
        let (n, from) = tokio::time::timeout(Duration::from_secs(5), carrier.recv_from(&mut buf))
            .await
            .expect("the echo comes back")
            .unwrap();
        assert_eq!((&buf[..n], from), (&b"ping"[..], to(echo)));
    }

    /// `ip-version` decides which families the carrier sends to.
    #[tokio::test]
    async fn ip_version_limits_a_direct_carrier() {
        let c = DirectConnector::with_opts(
            Arc::new(Fixed(vec![ip("::1"), ip("127.0.0.1")])),
            SocketOpts {
                ip_version: IpVersion::V4Only,
                ..SocketOpts::default()
            },
            Arc::new(NoopSocketHook),
        );
        let carrier = c.open_udp(&ConnectOpts::default()).await.unwrap();
        assert_eq!(
            carrier
                .resolve(&Target::new(HostName::parse("both.test"), 53))
                .await
                .unwrap(),
            Target::new(HostName::parse("127.0.0.1"), 53)
        );
        let err = carrier
            .send_to(b"x", &Target::new(HostName::parse("::1"), 53))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AddrNotAvailable);
        assert_eq!(err.to_string(), "this policy sends no UDP to [::1]:53");
    }

    /// The policy's socket options go on each socket of the carrier.
    #[tokio::test]
    async fn the_hook_sees_every_socket_of_a_direct_carrier() {
        let hook = Arc::new(RecordingHook::default());
        let c = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts {
                interface: Some("test0".into()),
                tos: 0x10,
                ip_version: IpVersion::V4Only,
                ..SocketOpts::default()
            },
            hook.clone(),
        );
        c.open_udp(&ConnectOpts::default()).await.unwrap();
        assert_eq!(
            *hook.calls.lock().unwrap(),
            ["tos 0x10 V4", "bind test0 V4"]
        );
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-net connector`
Expected: FAIL——`open_udp` 还不存在：

```text
   Compiling thiserror v2.0.20
   Compiling thiserror-impl v2.0.20
error[E0599]: no method named `open_udp` found for struct `TcpOnly` in the current scope
   --> crates\rurge-net\src\connector.rs:884:14
error[E0599]: no method named `open_udp` found for struct `connector::DirectConnector` in the current scope
   --> crates\rurge-net\src\connector.rs:897:25
error[E0599]: no method named `open_udp` found for struct `connector::DirectConnector` in the current scope
   --> crates\rurge-net\src\connector.rs:938:25
error[E0599]: no method named `open_udp` found for struct `connector::DirectConnector` in the current scope
   --> crates\rurge-net\src\connector.rs:963:25
error[E0599]: no method named `open_udp` found for struct `connector::DirectConnector` in the current scope
   --> crates\rurge-net\src\connector.rs:993:11
For more information about this error, try `rustc --explain E0599`.
error: could not compile `rurge-net` (lib test) due to 5 previous errors
exit 101
```

- [ ] **Step 3: 实现**

`crates/rurge-net/src/connector.rs`——把

```rust
use rurge_config::HostName;
```

换成

```rust
use rurge_config::HostName;
use rurge_config::spec::IpVersion;
```

`crates/rurge-net/src/connector.rs`——把

```rust
pub type BoxedDatagram = Box<dyn Datagram>;
```

换成

```rust
pub type BoxedDatagram = Box<dyn Datagram>;

/// One client association's UDP carrier on one outbound (phase 2 M5 design
/// 4.1): datagrams go to, and come back from, any address — the carrier of
/// a full-cone association. `send_to` and `recv_from` may run at the same
/// time, from two tasks.
pub trait PacketSocket: Send + Sync {
    /// Where the datagrams for `to` really go. A carrier that looks names up
    /// itself (DIRECT) answers with an address; the others hand `to` back:
    /// their server resolves it.
    fn resolve<'a>(&'a self, to: &'a Target) -> BoxFuture<'a, io::Result<Target>> {
        Box::pin(std::future::ready(Ok(to.clone())))
    }
    /// Sends `buf` as one datagram to `to`.
    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>>;
    /// Receives one datagram into `buf`: its length, and where it came from.
    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>>;
}

pub type BoxedPacketSocket = Box<dyn PacketSocket>;
```

`crates/rurge-net/src/connector.rs`——把

```rust
        Box::pin(std::future::ready(Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this connection cannot carry UDP",
        ))))
    }
```

换成

```rust
        Box::pin(std::future::ready(Err(no_udp())))
    }

    /// A UDP carrier that sends to any address (phase 2 M5 design 4.3);
    /// `Unsupported` from a connector that carries none.
    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedPacketSocket>> {
        let _ = opts;
        Box::pin(std::future::ready(Err(no_udp())))
    }
}

fn no_udp() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "this connection cannot carry UDP",
    )
```

`crates/rurge-net/src/connector.rs`——把

```rust
            }) as BoxedDatagram)
```

换成

```rust
            }) as BoxedDatagram)
        }))
    }

    fn open_udp<'a>(
        &'a self,
        _opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedPacketSocket>> {
        Box::pin(std::future::ready(
            self.open_packet().map(|p| Box::new(p) as BoxedPacketSocket),
        ))
    }
}

impl DirectConnector {
    /// One unconnected UDP socket per address family the policy's
    /// `ip-version` allows, each bound to the unspecified address and
    /// carrying the policy's socket options. A family this machine cannot
    /// open a socket of is left out, unless it is the only one.
    fn open_packet(&self) -> io::Result<DirectPacket> {
        let families: &[Family] = match self.opts.ip_version {
            IpVersion::V4Only => &[Family::V4],
            IpVersion::V6Only => &[Family::V6],
            _ => &[Family::V4, Family::V6],
        };
        let (mut v4, mut v6, mut failure) = (None, None, None);
        for family in families {
            let unspecified = match family {
                Family::V4 => IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
                Family::V6 => IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
            };
            let opened = open_socket(
                SocketAddr::new(unspecified, 0),
                socket2::Type::DGRAM,
                &self.opts,
                self.hook.as_ref(),
                &self.fallback_logged,
            )
            .and_then(|socket| {
                if *family == Family::V6 {
                    socket.set_only_v6(true)?;
                }
                make_room(&socket);
                socket.bind(&SocketAddr::new(unspecified, 0).into())?;
                UdpSocket::from_std(socket.into())
            });
            match (opened, family) {
                (Ok(socket), Family::V4) => v4 = Some(socket),
                (Ok(socket), Family::V6) => v6 = Some(socket),
                (Err(e), _) => failure = Some(e),
            }
        }
        if v4.is_none() && v6.is_none() {
            return Err(failure.unwrap_or_else(no_udp));
        }
        Ok(DirectPacket {
            v4,
            v6,
            resolver: self.resolver.clone(),
            opts: self.opts.clone(),
        })
    }
}

/// DIRECT's UDP carrier: unconnected sockets, one per address family.
struct DirectPacket {
    v4: Option<UdpSocket>,
    v6: Option<UdpSocket>,
    resolver: Arc<dyn Resolve>,
    opts: SocketOpts,
}

impl DirectPacket {
    fn socket_for(&self, ip: &IpAddr) -> Option<&UdpSocket> {
        match Family::of(ip) {
            Family::V4 => self.v4.as_ref(),
            Family::V6 => self.v6.as_ref(),
        }
    }

    /// The first address of `name` the policy's `ip-version` allows and
    /// this carrier has a socket for.
    async fn address_of(&self, name: &str) -> io::Result<IpAddr> {
        let addrs = self.resolver.resolve(name).await?;
        let (primary, secondary) = plan_addresses(addrs, self.opts.ip_version, self.opts.v6_first);
        primary
            .into_iter()
            .chain(secondary)
            .find(|ip| self.socket_for(ip).is_some())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no usable address for {name}"),
                )
            })
    }
}

impl PacketSocket for DirectPacket {
    fn resolve<'a>(&'a self, to: &'a Target) -> BoxFuture<'a, io::Result<Target>> {
        Box::pin(async move {
            match &to.host {
                HostName::Ip(_) => Ok(to.clone()),
                HostName::Domain(name) => Ok(Target::new(
                    HostName::Ip(self.address_of(name).await?),
                    to.port,
                )),
            }
        })
    }

    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            let ip = match &to.host {
                HostName::Ip(ip) => *ip,
                HostName::Domain(name) => self.address_of(name).await?,
            };
            let socket = self.socket_for(&ip).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    format!("this policy sends no UDP to {}", display_target(to)),
                )
            })?;
            socket.send_to(buf, SocketAddr::new(ip, to.port)).await?;
            Ok(())
        })
    }

    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(std::future::poll_fn(move |cx| {
            let mut read = ReadBuf::new(buf);
            for socket in [&self.v4, &self.v6].into_iter().flatten() {
                loop {
                    match socket.poll_recv_from(cx, &mut read) {
                        // an ICMP "unreachable" for an earlier datagram, which
                        // Windows reports on the next receive: nothing came
                        Poll::Ready(Err(e)) if e.kind() == io::ErrorKind::ConnectionReset => {}
                        Poll::Ready(from) => {
                            let from = from?;
                            return Poll::Ready(Ok((
                                read.filled().len(),
                                Target::new(HostName::Ip(from.ip()), from.port()),
                            )));
                        }
                        Poll::Pending => break,
                    }
                }
            }
            Poll::Pending
```

`crates/rurge-config/src/session.rs`——把

```rust
            device: None,
        }
    }

    /// The `HOSTNAME-TYPE` classification of the destination.
```

换成

```rust
            device: None,
        }
    }

    /// A UDP flow from the loopback with every optional field empty.
    pub fn udp(dst_host: HostName, dst_port: u16) -> SessionInfo {
        SessionInfo {
            transport: Transport::Udp,
            ..SessionInfo::tcp(dst_host, dst_port)
        }
    }

    /// The `HOSTNAME-TYPE` classification of the destination.
```

要点：
- 两个 socket 各自 `bind` 到未指定地址的 0 端口：没有绑定的 UDP socket 在 Windows 上一收就报错；IPv6 的设 `only_v6`，两族互不干扰。
- `recv_from` 在一个 `poll_fn` 里轮流问两个 socket；跳过 `ConnectionReset` 时接着问同一个 socket（它可能已经有下一个包）。
- 发往某一族而没有那一族的 socket（`ip-version` 不允许）时是 `AddrNotAvailable`：`this policy sends no UDP to <地址>`。
- `a_closed_port_does_not_stop_a_direct_carrier` 里的 200 ms 只是给 ICMP 回来留的窗口：之后的断言不依赖它是否已经到了。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-net connector` → 通过（新增 `a_direct_carrier_talks_to_any_address`、`a_closed_port_does_not_stop_a_direct_carrier`、`ip_version_limits_a_direct_carrier`、`the_hook_sees_every_socket_of_a_direct_carrier`；`a_connector_without_udp_says_so` 多了 `open_udp` 的断言）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-net/src/connector.rs crates/rurge-config/src/session.rs
git commit -m "feat(net): 按包收发的 UDP 载体 PacketSocket 与 Connector::open_udp；DIRECT 的载体"
```


### Task 2: `rurge-proto`——`Outbound` 的 UDP、DIRECT 与 `socks5` / `socks5-tls` 的 UDP（承接 C1）

`Outbound` 多两个方法（设计 4.2）：`udp()` 说这个策略按写法能不能承载 UDP，`open_udp()` 为一个客户端关联开一个载体；默认都是"不支持"。DIRECT 把连接器的载体交出去。`socks5` / `socks5-tls` 在 `udp-relay=true` 时经 UDP ASSOCIATE 承载 UDP（P5）：TCP 控制连接由一个任务持有并监视，数据报加上 RFC 1928 §7 的头发往服务器给出的中继（未指定地址时是服务器本身），经连接器的 `open_udp` 发出——所以有 `underlying-proxy` 时中继那一段也经底层策略（Task 6 接上链式）。SOCKS5 握手顺带交出应答里的地址（`negotiate_bound`）。`udp-relay` 对 `socks5` 的 `W0029` 退役。回环假服务端 `FakeSocks5` 学会 UDP ASSOCIATE。

**Files:**
- Modify: `crates/rurge-proto/src/outbound.rs`（`UdpSupport`、`Outbound::udp` / `open_udp`，与用例）、`src/lib.rs`（导出 `UdpSupport`）、`src/direct.rs`（DIRECT 的 UDP，与用例）、`src/addr.rs`（`parse_socks_addr`，与用例）、`src/socks5.rs`（`negotiate_bound`、`relay_of`、`Socks5Udp`、出站的 UDP，与用例）、`src/testing/socks5.rs`（UDP ASSOCIATE）
- Modify: `crates/rurge-config/src/spec/mod.rs`（`socks5` 的 `udp-relay` 不再 `W0029`，与用例）、`tests/policy_spec.rs`、`tests/snapshots/corpus__corpus__kitchen-sink.snap`

**Interfaces:**
- Consumes: Task 1 的 `PacketSocket`、`BoxedPacketSocket`、`Connector::open_udp`；既有的 `Socks5Spec.udp_relay`、`transport::Stack`。
- Produces:
  - `rurge_proto::UdpSupport { Native, Unsupported }`（`Clone + Copy + Debug + PartialEq + Eq`）
  - `Outbound::udp(&self) -> UdpSupport`（默认 `Unsupported`）、`Outbound::open_udp<'a>(&'a self, opts: &'a ConnectOpts) -> BoxFuture<'a, Result<BoxedPacketSocket, OutboundError>>`（默认 `OutboundError::Unsupported("UDP")`）
  - `Direct`：`Native`；`Socks5Outbound`：`udp_relay` 时 `Native`，否则 `Unsupported`（`open_udp` 报 `UDP without \`udp-relay=true\``）
  - `pub(crate)`：`addr::parse_socks_addr(&[u8]) -> Option<(Target, usize)>`、`socks5::{connect_request, negotiate, negotiate_bound, relay_of, Socks5Udp}`（`Socks5Udp::new(control: BoxedStream, relay: Target, socket: BoxedPacketSocket)`）、`socks5::UDP_ASSOCIATE`（Task 6 改为 `pub(crate)`）
  - `rurge_proto::testing::Socks5Script` 多 `udp_unspecified: bool`、`udp_close_after: Option<usize>`；`RecordedSocks5` 多 `command: u8`；`FakeSocks5::datagrams() -> Vec<Target>`

- [ ] **Step 1: 先写用例（连同假服务端的 UDP）**

假服务端是测试设施，先放进去：

`crates/rurge-proto/src/testing/socks5.rs`——把

```rust
//! A scriptable SOCKS5 server (RFC 1928 / 1929), CONNECT only.

use super::{AbortOnDrop, TlsFixture};
use rurge_net::connector::BoxedStream;
```

换成

```rust
//! A scriptable SOCKS5 server (RFC 1928 / 1929): CONNECT and UDP ASSOCIATE.

use super::{AbortOnDrop, TlsFixture};
use rurge_config::HostName;
use rurge_net::connector::{BoxedStream, Target};
```

`crates/rurge-proto/src/testing/socks5.rs`——把

```rust
use tokio::net::{TcpListener, TcpStream};
```

换成

```rust
use tokio::net::{TcpListener, TcpStream, UdpSocket};
```

`crates/rurge-proto/src/testing/socks5.rs`——把

```rust
    pub force_method: Option<u8>,
```

换成

```rust
    pub force_method: Option<u8>,
    /// Answer `UDP ASSOCIATE` with the unspecified address (and the relay's
    /// port): the client is to send where the control connection went.
    pub udp_unspecified: bool,
    /// Close the control connection after relaying this many answers.
    pub udp_close_after: Option<usize>,
```

`crates/rurge-proto/src/testing/socks5.rs`——把

```rust
    pub credentials: Option<(String, String)>,
```

换成

```rust
    pub credentials: Option<(String, String)>,
    /// 1 = CONNECT, 3 = UDP ASSOCIATE.
    pub command: u8,
```

`crates/rurge-proto/src/testing/socks5.rs`——把

```rust
    requests: Arc<Mutex<Vec<RecordedSocks5>>>,
    _task: AbortOnDrop,
```

换成

```rust
    requests: Arc<Mutex<Vec<RecordedSocks5>>>,
    datagrams: Arc<Mutex<Vec<Target>>>,
    _task: AbortOnDrop,
```

`crates/rurge-proto/src/testing/socks5.rs`——把

```rust
    stream.write_all(&out).await
}

async fn serve(
```

换成

```rust
    stream.write_all(&out).await
}

/// A UDP association: datagrams from the client go out by their address
/// (names are not resolved: they are dropped), answers from anywhere come
/// back with the sender's address, until the control connection closes.
async fn relay_udp(
    mut control: BoxedStream,
    script: &Socks5Script,
    datagrams: Arc<Mutex<Vec<Target>>>,
) -> io::Result<()> {
    let relay = UdpSocket::bind("127.0.0.1:0").await?;
    let outside = UdpSocket::bind("127.0.0.1:0").await?;
    let port = relay.local_addr()?.port();
    let mut bound = vec![1];
    bound.extend_from_slice(&if script.udp_unspecified {
        [0, 0, 0, 0]
    } else {
        [127, 0, 0, 1]
    });
    bound.extend_from_slice(&port.to_be_bytes());
    reply(&mut control, 0, &bound).await?;
    let (mut client, mut answered) = (None, 0);
    let (mut up, mut down, mut sink) = ([0u8; 2048], [0u8; 2048], [0u8; 64]);
    loop {
        tokio::select! {
            read = control.read(&mut sink) => {
                if !matches!(read, Ok(n) if n > 0) {
                    return Ok(());
                }
            }
            got = relay.recv_from(&mut up) => {
                // Windows reports an ICMP "unreachable" on the next receive
                let Ok((n, from)) = got else { continue };
                client = Some(from);
                let Some((to, start)) = parse(&up[..n]) else { continue };
                datagrams.lock().expect("datagrams").push(to.clone());
                if let HostName::Ip(ip) = to.host {
                    let _ = outside.send_to(&up[start..n], SocketAddr::new(ip, to.port)).await;
                }
            }
            got = outside.recv_from(&mut down) => {
                let Ok((n, from)) = got else { continue };
                let Some(client) = client else { continue };
                let mut datagram = vec![0, 0, 0];
                datagram.extend(address(&from));
                datagram.extend_from_slice(&down[..n]);
                relay.send_to(&datagram, client).await?;
                answered += 1;
                if script.udp_close_after == Some(answered) {
                    return control.shutdown().await;
                }
            }
        }
    }
}

/// RSV RSV FRAG ATYP ADDR PORT, then the payload; only unfragmented
/// datagrams are taken.
fn parse(datagram: &[u8]) -> Option<(Target, usize)> {
    if datagram.get(..3)? != [0, 0, 0] {
        return None;
    }
    let (host, rest) = match *datagram.get(3)? {
        1 => {
            let b: [u8; 4] = datagram.get(4..8)?.try_into().ok()?;
            (HostName::Ip(IpAddr::from(b)), 8)
        }
        4 => {
            let b: [u8; 16] = datagram.get(4..20)?.try_into().ok()?;
            (HostName::Ip(IpAddr::from(b)), 20)
        }
        3 => {
            let len = usize::from(*datagram.get(4)?);
            let name = String::from_utf8_lossy(datagram.get(5..5 + len)?).into_owned();
            (HostName::parse(&name), 5 + len)
        }
        _ => return None,
    };
    let port = u16::from_be_bytes(datagram.get(rest..rest + 2)?.try_into().ok()?);
    Some((Target::new(host, port), rest + 2))
}

fn address(addr: &SocketAddr) -> Vec<u8> {
    let mut out = match addr.ip() {
        IpAddr::V4(v4) => [&[1][..], &v4.octets()].concat(),
        IpAddr::V6(v6) => [&[4][..], &v6.octets()].concat(),
    };
    out.extend_from_slice(&addr.port().to_be_bytes());
    out
}

async fn serve(
```

`crates/rurge-proto/src/testing/socks5.rs`——把

```rust
    requests: Arc<Mutex<Vec<RecordedSocks5>>>,
) -> io::Result<()> {
```

换成

```rust
    requests: Arc<Mutex<Vec<RecordedSocks5>>>,
    datagrams: Arc<Mutex<Vec<Target>>>,
) -> io::Result<()> {
```

`crates/rurge-proto/src/testing/socks5.rs`——把

```rust
        credentials,
```

换成

```rust
        credentials,
        command: request[1],
```

`crates/rurge-proto/src/testing/socks5.rs`——把

```rust
    });
    tokio::time::sleep(script.delay).await;
```

换成

```rust
    });
    if request[1] == 3 {
        return relay_udp(stream, &script, datagrams).await;
    }
    tokio::time::sleep(script.delay).await;
```

`crates/rurge-proto/src/testing/socks5.rs`——把

```rust
        let log = requests.clone();
```

换成

```rust
        let datagrams: Arc<Mutex<Vec<Target>>> = Arc::default();
        let (log, udp_log) = (requests.clone(), datagrams.clone());
```

`crates/rurge-proto/src/testing/socks5.rs`——把

```rust
                let (script, log, tls) = (script.clone(), log.clone(), tls.clone());
```

换成

```rust
                let (script, log, udp_log, tls) =
                    (script.clone(), log.clone(), udp_log.clone(), tls.clone());
```

`crates/rurge-proto/src/testing/socks5.rs`——把

```rust
                    let _ = serve(stream, script, log).await;
```

换成

```rust
                    let _ = serve(stream, script, log, udp_log).await;
```

`crates/rurge-proto/src/testing/socks5.rs`——把

```rust
            requests,
```

换成

```rust
            requests,
            datagrams,
```

`crates/rurge-proto/src/testing/socks5.rs`——把

```rust
    /// Every CONNECT request seen so far, in arrival order.
```

换成

```rust
    /// Every request (CONNECT, UDP ASSOCIATE) seen so far, in arrival order.
```

`crates/rurge-proto/src/testing/socks5.rs`——把

```rust
        self.requests.lock().expect("requests").clone()
    }
}
```

换成

```rust
        self.requests.lock().expect("requests").clone()
    }

    /// Where every datagram relayed so far was addressed, in arrival order.
    pub fn datagrams(&self) -> Vec<Target> {
        self.datagrams.lock().expect("datagrams").clone()
    }
}
```

出站与地址的用例：

`crates/rurge-proto/src/outbound.rs`——把

```rust
            "policy unavailable: subscription item is broken"
        );
    }

    #[test]
    fn only_http_proxies_forward_plain_requests() {
```

换成

```rust
            "policy unavailable: subscription item is broken"
        );
    }

    #[tokio::test]
    async fn an_outbound_carries_no_udp_unless_it_says_so() {
        let reject = Reject::new(RejectKind::Reject);
        assert_eq!(reject.udp(), UdpSupport::Unsupported);
        let err = reject
            .open_udp(&ConnectOpts::default())
            .await
            .err()
            .unwrap();
        assert_eq!(err.to_string(), "policy protocol not implemented: UDP");
    }

    #[test]
    fn only_http_proxies_forward_plain_requests() {
```

`crates/rurge-proto/src/direct.rs`——把

```rust
            assert_eq!(&buf, b"ping");
        }
    }

    #[tokio::test]
    async fn resolution_and_connect_failures_are_io_errors() {
```

换成

```rust
            assert_eq!(&buf, b"ping");
        }
    }

    /// DIRECT carries UDP: to an address, and to a name looked up here.
    #[tokio::test]
    async fn direct_carries_udp() {
        let echo = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = echo.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut buf = [0u8; 64];
            while let Ok((n, from)) = echo.recv_from(&mut buf).await {
                let _ = echo.send_to(&buf[..n], from).await;
            }
        });
        let direct = Direct::with_resolver(Arc::new(Loopback));
        assert_eq!(direct.udp(), UdpSupport::Native);
        let carrier = direct.open_udp(&ConnectOpts::default()).await.unwrap();
        let by_name = Target::new(HostName::parse("echo.test"), port);
        let to = carrier.resolve(&by_name).await.unwrap();
        assert_eq!(to, Target::new(HostName::parse("127.0.0.1"), port));
        carrier.send_to(b"ping", &to).await.unwrap();
        let mut buf = [0u8; 64];
        let (n, from) = carrier.recv_from(&mut buf).await.unwrap();
        assert_eq!((&buf[..n], from), (&b"ping"[..], to));
    }

    #[tokio::test]
    async fn resolution_and_connect_failures_are_io_errors() {
```

`crates/rurge-proto/src/addr.rs`——把

```rust
    use super::*;
```

换成

```rust
    use super::*;

    #[test]
    fn a_socks_address_reads_back_as_it_was_written() {
        for host in ["10.1.2.3", "2001:db8::1", "example.com"] {
            let target = Target::new(HostName::parse(host), 853);
            let mut bytes = socks_addr(&target).unwrap();
            let written = bytes.len();
            bytes.extend_from_slice(b"payload");
            assert_eq!(parse_socks_addr(&bytes), Some((target, written)));
        }
        assert_eq!(parse_socks_addr(&[1, 10, 1, 2, 3, 0]), None, "cut short");
        assert_eq!(parse_socks_addr(&[9, 0, 0]), None, "unknown type");
        assert_eq!(
            parse_socks_addr(&[3, 3, b'a', b' ', b'b', 0, 53]),
            None,
            "no host name"
        );
    }
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
        assert_eq!(buf, payload);
```

换成

```rust
        assert_eq!(buf, payload);
    }

    /// Answers every datagram with itself.
    async fn udp_echo() -> SocketAddr {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 1500];
            while let Ok((n, from)) = socket.recv_from(&mut buf).await {
                let _ = socket.send_to(&buf[..n], from).await;
            }
        });
        addr
    }

    async fn udp_roundtrip(carrier: &dyn PacketSocket, to: SocketAddr, payload: &[u8]) {
        carrier.send_to(payload, &target(to)).await.unwrap();
        let mut buf = [0u8; 1500];
        let (n, from) = tokio::time::timeout(Duration::from_secs(5), carrier.recv_from(&mut buf))
            .await
            .expect("an answer")
            .unwrap();
        assert_eq!((&buf[..n], from), (payload, target(to)));
    }

    /// `udp-relay=true`: datagrams go through the relay the proxy names,
    /// each with its address; the proxy's answers come back with theirs.
    #[tokio::test]
    async fn udp_goes_through_the_association() {
        let (one, two) = (udp_echo().await, udp_echo().await);
        let proxy = FakeSocks5::spawn(Socks5Script {
            auth: Some(("u".into(), "p".into())),
            ..Socks5Script::default()
        })
        .await;
        let out = outbound(
            &format!(
                "socks5, 127.0.0.1, {}, u, p, udp-relay=true",
                proxy.addr().port()
            ),
            no_roots(),
        );
        assert_eq!(out.udp(), UdpSupport::Native);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), one, b"to one").await;
        udp_roundtrip(carrier.as_ref(), two, b"to two").await;
        let seen = proxy.requests();
        assert_eq!((seen.len(), seen[0].command), (1, 3), "one association");
        assert_eq!(proxy.datagrams(), [target(one), target(two)]);
    }

    /// A relay that answers with the unspecified address listens where the
    /// control connection went.
    #[tokio::test]
    async fn an_unspecified_relay_address_means_the_server() {
        let echo = udp_echo().await;
        let proxy = FakeSocks5::spawn(Socks5Script {
            udp_unspecified: true,
            ..Socks5Script::default()
        })
        .await;
        let out = outbound(
            &format!("socks5, 127.0.0.1, {}, udp-relay=true", proxy.addr().port()),
            no_roots(),
        );
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), echo, b"here").await;
        assert_eq!(
            relay_of(
                Target::new(HostName::parse("::"), 7),
                &HostName::parse("s.test")
            ),
            Target::new(HostName::parse("s.test"), 7)
        );
    }

    /// The association ends with its control connection.
    #[tokio::test]
    async fn a_closed_control_connection_ends_the_association() {
        let echo = udp_echo().await;
        let proxy = FakeSocks5::spawn(Socks5Script {
            udp_close_after: Some(1),
            ..Socks5Script::default()
        })
        .await;
        let out = outbound(
            &format!("socks5, 127.0.0.1, {}, udp-relay=true", proxy.addr().port()),
            no_roots(),
        );
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), echo, b"once").await;
        let mut buf = [0u8; 64];
        let err = tokio::time::timeout(Duration::from_secs(5), carrier.recv_from(&mut buf))
            .await
            .expect("bounded")
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "socks5: the proxy closed the UDP association"
        );
        let err = carrier.send_to(b"late", &target(echo)).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    }

    /// Without `udp-relay=true` the policy carries no UDP (the manual: it
    /// must be switched on), and nothing is asked of the server.
    #[tokio::test]
    async fn no_udp_without_udp_relay() {
        let proxy = FakeSocks5::spawn(Socks5Script::default()).await;
        let out = outbound(
            &format!("socks5, 127.0.0.1, {}", proxy.addr().port()),
            no_roots(),
        );
        assert_eq!(out.udp(), UdpSupport::Unsupported);
        let err = out.open_udp(&ConnectOpts::default()).await.err().unwrap();
        assert_eq!(
            err.to_string(),
            "policy protocol not implemented: UDP without `udp-relay=true`"
        );
        assert!(proxy.requests().is_empty());
    }

    #[test]
    fn fragments_and_garbage_are_not_datagrams() {
        let mut datagram = udp_header(&Target::new(HostName::parse("10.0.0.1"), 53)).unwrap();
        datagram.extend_from_slice(b"q");
        assert_eq!(
            parse_udp(&datagram),
            Some((
                Target::new(HostName::parse("10.0.0.1"), 53),
                datagram.len() - 1
            ))
        );
        datagram[2] = 1;
        assert_eq!(parse_udp(&datagram), None, "a fragment");
        assert_eq!(parse_udp(&[0, 0]), None);
```

配置层的两处期望：`udp-relay` 不再列为"暂不生效"（`policy_spec.rs` 改用仍在 M5c 才生效的 `test-udp` 来测"每个名字报一次"）：

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        // Shadow TLS took effect in M2c: no longer on the list
        assert_eq!(o.inert, ["udp-relay"]);
```

换成

```rust
        // Shadow TLS took effect in M2c, `udp-relay` in M5a: nothing on the list
        assert!(o.inert.is_empty(), "{:?}", o.inert);
```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
        "A = socks5, a.example, 1080, udp-relay=true, tfo=true\nB = socks5, b.example, 1080, udp-relay=true, hybrid=on\nC = http, c.example, 80, hybrid=off",
```

换成

```rust
        "A = socks5, a.example, 1080, test-udp=apple.com@8.8.8.8, tfo=true\nB = socks5, b.example, 1080, test-udp=apple.com@8.8.8.8, hybrid=on\nC = http, c.example, 80, hybrid=off",
```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
                "policy parameter `udp-relay` is parsed but has no effect in this version"
                    .to_string(),
```

换成

```rust
                "policy parameter `tfo` is parsed but has no effect in this version".to_string(),
```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
            ),
            (
                codes::W_PARAM_NOT_EFFECTIVE,
                "policy parameter `tfo` is parsed but has no effect in this version".to_string(),
                2
            ),
```

换成

```rust
            ),
            (
                codes::W_PARAM_NOT_EFFECTIVE,
                "policy parameter `test-udp` is parsed but has no effect in this version"
                    .to_string(),
                2
            ),
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto --lib`
Expected: FAIL——`UdpSupport`、`parse_socks_addr` 等还不存在：

```text
error[E0405]: cannot find trait `PacketSocket` in this scope
   --> crates\rurge-proto\src\socks5.rs:304:42
error[E0433]: failed to resolve: use of undeclared type `UdpSupport`
   --> crates\rurge-proto\src\direct.rs:118:34
error[E0433]: failed to resolve: use of undeclared type `UdpSupport`
   --> crates\rurge-proto\src\outbound.rs:178:34
error[E0433]: failed to resolve: use of undeclared type `UdpSupport`
   --> crates\rurge-proto\src\socks5.rs:331:31
error[E0433]: failed to resolve: use of undeclared type `UdpSupport`
   --> crates\rurge-proto\src\socks5.rs:402:31
error[E0425]: cannot find function `parse_socks_addr` in this scope
  --> crates\rurge-proto\src\addr.rs:75:24
error[E0425]: cannot find function `parse_socks_addr` in this scope
  --> crates\rurge-proto\src\addr.rs:77:20
error[E0425]: cannot find function `parse_socks_addr` in this scope
  --> crates\rurge-proto\src\addr.rs:78:20
error[E0425]: cannot find function `parse_socks_addr` in this scope
  --> crates\rurge-proto\src\addr.rs:80:13
error[E0425]: cannot find function `relay_of` in this scope
   --> crates\rurge-proto\src\socks5.rs:357:13
error[E0425]: cannot find function `udp_header` in this scope
   --> crates\rurge-proto\src\socks5.rs:413:28
error[E0425]: cannot find function `parse_udp` in this scope
   --> crates\rurge-proto\src\socks5.rs:416:13
error[E0425]: cannot find function `parse_udp` in this scope
   --> crates\rurge-proto\src\socks5.rs:423:20
error[E0425]: cannot find function `parse_udp` in this scope
   --> crates\rurge-proto\src\socks5.rs:424:20
Some errors have detailed explanations: E0405, E0425, E0433.
For more information about an error, try `rustc --explain E0405`.
error: could not compile `rurge-proto` (lib test) due to 14 previous errors
exit 101
```

- [ ] **Step 3: 实现**

`crates/rurge-proto/src/outbound.rs`——把

```rust
use rurge_net::connector::{BoxedStream, ConnectOpts, Target};
```

换成

```rust
use rurge_net::connector::{BoxedPacketSocket, BoxedStream, ConnectOpts, Target};
```

`crates/rurge-proto/src/outbound.rs`——把

```rust
    fn request_headers(&self) -> Vec<(String, String)>;
}

/// A way to reach a destination. Phase 1 ships `Direct` and `Reject`; every
```

换成

```rust
    fn request_headers(&self) -> Vec<(String, String)>;
}

/// Whether an outbound carries UDP (phase 2 M5 design 4.2). UDP over TCP
/// (`anytls`) is the protocol's own business: `Native` all the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UdpSupport {
    Native,
    Unsupported,
}

/// A way to reach a destination. Phase 1 ships `Direct` and `Reject`; every
```

`crates/rurge-proto/src/outbound.rs`——把

```rust
    fn native_test(&self) -> Option<BoxFuture<'_, Result<Duration, OutboundError>>> {
        None
    }
```

换成

```rust
    fn native_test(&self) -> Option<BoxFuture<'_, Result<Duration, OutboundError>>> {
        None
    }
    /// Whether `open_udp` can work, as the policy is written.
    fn udp(&self) -> UdpSupport {
        UdpSupport::Unsupported
    }
    /// A UDP carrier for one client association (phase 2 M5 design 4.2):
    /// datagrams to and from any address. `opts.timeout` bounds opening it.
    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedPacketSocket, OutboundError>> {
        let _ = opts;
        Box::pin(std::future::ready(Err(OutboundError::Unsupported(
            "UDP".to_string(),
        ))))
    }
```

`crates/rurge-proto/src/lib.rs`——把

```rust
pub use outbound::{HttpForward, Outbound, OutboundError, OutboundRef, RejectKind};
```

换成

```rust
pub use outbound::{HttpForward, Outbound, OutboundError, OutboundRef, RejectKind, UdpSupport};
```

`crates/rurge-proto/src/direct.rs`——把

```rust
use crate::outbound::{Outbound, OutboundError};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, DirectConnector, Resolve, Target};
```

换成

```rust
use crate::outbound::{Outbound, OutboundError, UdpSupport};
use rurge_net::BoxFuture;
use rurge_net::connector::{
    BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, DirectConnector, Resolve, Target,
};
```

`crates/rurge-proto/src/direct.rs`——把

```rust
                Ok(Ok(stream)) => Ok(stream),
```

换成

```rust
                Ok(Ok(stream)) => Ok(stream),
                Ok(Err(e)) => Err(OutboundError::from(e)),
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }

    fn udp(&self) -> UdpSupport {
        UdpSupport::Native
    }

    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedPacketSocket, OutboundError>> {
        Box::pin(async move {
            match tokio::time::timeout(opts.timeout, self.connector.open_udp(opts)).await {
                Ok(Ok(socket)) => Ok(socket),
```

`crates/rurge-proto/src/addr.rs`——把

```rust
    Ok(out)
}

/// `PORT TYPE ADDR` as VMess writes it: the port first, and the types are
```

换成

```rust
    Ok(out)
}

/// `ATYP ADDR PORT` at the start of `bytes`, and how many bytes it took;
/// `None` when it is cut short, of an unknown type, or a name that is no
/// host name.
pub(crate) fn parse_socks_addr(bytes: &[u8]) -> Option<(Target, usize)> {
    let (host, rest) = match *bytes.first()? {
        1 => {
            let b: [u8; 4] = bytes.get(1..5)?.try_into().ok()?;
            (HostName::Ip(IpAddr::from(b)), 5)
        }
        4 => {
            let b: [u8; 16] = bytes.get(1..17)?.try_into().ok()?;
            (HostName::Ip(IpAddr::from(b)), 17)
        }
        3 => {
            let len = usize::from(*bytes.get(1)?);
            let name = std::str::from_utf8(bytes.get(2..2 + len)?).ok()?;
            (HostName::from_wire(name)?, 2 + len)
        }
        _ => return None,
    };
    let port = u16::from_be_bytes(bytes.get(rest..rest + 2)?.try_into().ok()?);
    Some((Target::new(host, port), rest + 2))
}

/// `PORT TYPE ADDR` as VMess writes it: the port first, and the types are
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
use crate::transport::Stack;
use crate::{BuildError, Outbound, OutboundError};
use rurge_config::KeystoreItem;
use rurge_config::spec::{PolicySpec, ProtoSpec};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
```

换成

```rust
use crate::task::AbortOnDrop;
use crate::transport::Stack;
use crate::{BuildError, Outbound, OutboundError, UdpSupport};
use rurge_config::spec::{PolicySpec, ProtoSpec};
use rurge_config::{HostName, KeystoreItem};
use rurge_net::BoxFuture;
use rurge_net::connector::{
    BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, PacketSocket, Target,
};
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const VERSION: u8 = 5;
```

换成

```rust
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

const VERSION: u8 = 5;
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
const MAX_CREDENTIAL: usize = 255;

pub struct Socks5Outbound {
```

换成

```rust
const MAX_CREDENTIAL: usize = 255;

/// `UDP ASSOCIATE` with no address of its own: the relay takes datagrams
/// from wherever the association's first one comes from (RFC 1928 §7).
const UDP_ASSOCIATE: [u8; 10] = [VERSION, 3, 0, 1, 0, 0, 0, 0, 0, 0];

pub struct Socks5Outbound {
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
    credentials: Option<(String, String)>,
```

换成

```rust
    credentials: Option<(String, String)>,
    /// `udp-relay`: the server takes `UDP ASSOCIATE` (the manual: it must
    /// be switched on, many servers do not).
    udp_relay: bool,
    /// The server as written, for a relay that answers with the
    /// unspecified address.
    server: Target,
    /// Where the relayed datagrams leave from: the same way the control
    /// connection goes (DIRECT, or `underlying-proxy`).
    connector: Arc<dyn Connector>,
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
                connector,
```

换成

```rust
                connector.clone(),
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
            credentials,
        })
```

换成

```rust
            credentials,
            udp_relay: socks.udp_relay,
            server: Target::new(host.clone(), port),
            connector,
        })
    }

    async fn associate(&self, opts: &ConnectOpts) -> Result<BoxedPacketSocket, OutboundError> {
        let control = self.stack.open(opts).await?;
        let (control, relay) =
            negotiate_bound(control, &UDP_ASSOCIATE, self.credentials.as_ref()).await?;
        let socket = self.connector.open_udp(opts).await?;
        Ok(Box::new(Socks5Udp::new(
            control,
            relay_of(relay, &self.server.host),
            socket,
        )))
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
        negotiate(stream, &request, self.credentials.as_ref()).await
    }
}

/// The SOCKS5 handshake on `stream`, a connection to the proxy: method
```

换成

```rust
        negotiate(stream, &request, self.credentials.as_ref()).await
    }
}

/// Where the relay said to send datagrams; a relay that answered with the
/// unspecified address listens where the control connection went, which
/// is `server` (the usual reading of RFC 1928 §6).
pub(crate) fn relay_of(bound: Target, server: &HostName) -> Target {
    match bound.host {
        HostName::Ip(ip) if ip.is_unspecified() => Target::new(server.clone(), bound.port),
        _ => bound,
    }
}

/// The header of a datagram to or from `target` (RFC 1928 §7): RSV RSV
/// FRAG, then the address.
fn udp_header(target: &Target) -> Result<Vec<u8>, OutboundError> {
    let mut out = vec![0, 0, 0];
    out.extend(crate::addr::socks_addr(target).map_err(|e| match e {
        crate::addr::AddrError::Unsendable => {
            proxy("the host name cannot be sent to a SOCKS5 proxy")
        }
        crate::addr::AddrError::TooLong => proxy("the host name is longer than 255 bytes"),
    })?);
    Ok(out)
}

/// A datagram's source and where its payload starts; `None` for one that
/// is fragmented (FRAG ≠ 0: never reassembled) or malformed.
fn parse_udp(datagram: &[u8]) -> Option<(Target, usize)> {
    if datagram.get(..3)? != [0, 0, 0] {
        return None;
    }
    let (from, len) = crate::addr::parse_socks_addr(&datagram[3..])?;
    Some((from, 3 + len))
}

/// A SOCKS5 UDP association (`socks5`, `socks5-tls`, `external`): the
/// control connection, held open and watched, and the relay's datagrams
/// through `socket`. The association ends when either side closes the
/// control connection (RFC 1928 §7): this carrier then fails.
pub(crate) struct Socks5Udp {
    relay: Target,
    socket: BoxedPacketSocket,
    closed: CancellationToken,
    _control: AbortOnDrop,
}

impl Socks5Udp {
    pub(crate) fn new(
        mut control: BoxedStream,
        relay: Target,
        socket: BoxedPacketSocket,
    ) -> Socks5Udp {
        let closed = CancellationToken::new();
        let watch = closed.clone();
        // nothing more comes on the control connection: its end is the
        // association's end
        let task = tokio::spawn(async move {
            let mut sink = [0u8; 64];
            while matches!(control.read(&mut sink).await, Ok(n) if n > 0) {}
            watch.cancel();
        });
        Socks5Udp {
            relay,
            socket,
            closed,
            _control: AbortOnDrop(task),
        }
    }
}

fn association_closed() -> io::Error {
    io::Error::new(
        io::ErrorKind::BrokenPipe,
        "socks5: the proxy closed the UDP association",
    )
}

impl PacketSocket for Socks5Udp {
    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            if self.closed.is_cancelled() {
                return Err(association_closed());
            }
            let mut datagram = udp_header(to).map_err(|e| io::Error::other(e.to_string()))?;
            datagram.extend_from_slice(buf);
            self.socket.send_to(&datagram, &self.relay).await
        })
    }

    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            loop {
                let (n, _) = tokio::select! {
                    _ = self.closed.cancelled() => return Err(association_closed()),
                    got = self.socket.recv_from(buf) => got?,
                };
                // what the relay cannot have meant (a fragment, garbage) is dropped
                if let Some((from, start)) = parse_udp(&buf[..n]) {
                    buf.copy_within(start..n, 0);
                    return Ok((n - start, from));
                }
            }
        })
    }
}

/// The SOCKS5 handshake on `stream`, a connection to the proxy: method
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
pub(crate) async fn negotiate(
```

换成

```rust
pub(crate) async fn negotiate(
    stream: BoxedStream,
    request: &[u8],
    credentials: Option<&(String, String)>,
) -> Result<BoxedStream, OutboundError> {
    negotiate_bound(stream, request, credentials)
        .await
        .map(|(stream, _)| stream)
}

/// `negotiate`, and the address the proxy's reply named (BND.ADDR,
/// BND.PORT): for `UDP ASSOCIATE`, where the datagrams go.
pub(crate) async fn negotiate_bound(
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
    credentials: Option<&(String, String)>,
) -> Result<BoxedStream, OutboundError> {
    let offered: &[u8] = if credentials.is_some() {
```

换成

```rust
    credentials: Option<&(String, String)>,
) -> Result<(BoxedStream, Target), OutboundError> {
    let offered: &[u8] = if credentials.is_some() {
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
    // skip the bound address
```

换成

```rust
    let mut bound = vec![reply[3]];
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
            stream.read_exact(&mut len).await.map_err(handshake_io)?;
```

换成

```rust
            stream.read_exact(&mut len).await.map_err(handshake_io)?;
            bound.push(len[0]);
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
    let mut bound = vec![0u8; remaining];
    stream.read_exact(&mut bound).await.map_err(handshake_io)?;
    Ok(stream)
```

换成

```rust
    let start = bound.len();
    bound.resize(start + remaining, 0);
    stream
        .read_exact(&mut bound[start..])
        .await
        .map_err(handshake_io)?;
    let (bound, _) = crate::addr::parse_socks_addr(&bound)
        .ok_or_else(|| proxy("the reply names an address that is no host name"))?;
    Ok((stream, bound))
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
            match tokio::time::timeout(opts.timeout, self.handshake(target, opts)).await {
```

换成

```rust
            match tokio::time::timeout(opts.timeout, self.handshake(target, opts)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }

    fn udp(&self) -> UdpSupport {
        if self.udp_relay {
            UdpSupport::Native
        } else {
            UdpSupport::Unsupported
        }
    }

    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedPacketSocket, OutboundError>> {
        Box::pin(async move {
            if !self.udp_relay {
                return Err(OutboundError::Unsupported(
                    "UDP without `udp-relay=true`".to_string(),
                ));
            }
            match tokio::time::timeout(opts.timeout, self.associate(opts)).await {
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            let udp_relay = r.bool("udp-relay").unwrap_or(false);
            if udp_relay {
                notes.inert.insert(0, "udp-relay");
            }
```

换成

```rust
            let udp_relay = r.bool("udp-relay").unwrap_or(false);
```

kitchen-sink 语料里 `socks5` 的 `udp-relay` 不再报 `W0029`；`external` 的那一条（Task 6 才退役）因此成了这一次加载里第一个报的：

`crates/rurge-config/tests/snapshots/corpus__corpus__kitchen-sink.snap`——把

```text
  - "warning[W0029] valid/kitchen-sink.conf:53: policy parameter `tfo` is parsed but has no effect in this version"
  - "warning[W0029] valid/kitchen-sink.conf:55: policy parameter `udp-relay` is parsed but has no effect in this version"
```

换成

```text
  - "warning[W0029] valid/kitchen-sink.conf:53: policy parameter `tfo` is parsed but has no effect in this version"
```

`crates/rurge-config/tests/snapshots/corpus__corpus__kitchen-sink.snap`——把

```text
  - "warning[W0029] valid/kitchen-sink.conf:71: policy parameter `addresses` is parsed but has no effect in this version"
```

换成

```text
  - "warning[W0029] valid/kitchen-sink.conf:71: policy parameter `addresses` is parsed but has no effect in this version"
  - "warning[W0029] valid/kitchen-sink.conf:71: policy parameter `udp-relay` is parsed but has no effect in this version"
```

要点：
- `negotiate` 的行为不变（CONNECT 的用例照旧通过）；它现在调用 `negotiate_bound`，丢掉应答里的地址。应答里的地址不是主机名时报 `socks5: the reply names an address that is no host name`。
- `Socks5Udp::recv_from` 直接收进调用方的缓冲，再把载荷挪到开头（不为每个包分配）；分片与认不出的数据报丢弃、接着收。
- 载体发出的每个数据报都带 3 字节保留位与地址；目标名发给服务器解析（与 TCP 相同，名字按 `to_ascii` 转成 A-label，发不出去的名字是 `socks5: the host name cannot be sent to a SOCKS5 proxy`）。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto` → 通过（新增 `udp_goes_through_the_association`、`an_unspecified_relay_address_means_the_server`、`a_closed_control_connection_ends_the_association`、`no_udp_without_udp_relay`、`fragments_and_garbage_are_not_datagrams`、`direct_carries_udp`、`a_socks_address_reads_back_as_it_was_written`、`an_outbound_carries_no_udp_unless_it_says_so`）。
Run: `cargo test -p rurge-config` → 通过（含 `--test corpus` 与 `--test policy_spec`）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-proto crates/rurge-config
git commit -m "feat(proto): Outbound 的 UDP（udp / open_udp）；DIRECT 与 socks5 / socks5-tls 的 UDP ASSOCIATE（udp-relay 生效）"
```


### Task 3: `rurge-inbound`——SOCKS5 UDP ASSOCIATE 与 `Dialer` 的 UDP 接口

SOCKS5 监听接受 UDP ASSOCIATE（设计 5.1，P1）：先问 dialer 能不能接（`admit_udp`），再在客户端连到的本机地址上开一个 UDP 端口并在应答里给出；关联的客户端一侧是 `Socks5UdpClient`（只收控制连接那个客户端的包、解析与封装 UDP 头、跳过 Windows 的 `ConnectionReset`），交给 dialer 的 `associate`，直到控制连接结束。引擎的实现在 Task 4；本任务的默认实现是"不支持"，已有的 dialer（各单元测试的假 dialer）因此回 0x07，与此前相同。

**Files:**
- Create: `crates/rurge-inbound/src/udp.rs`（`Socks5UdpClient`、`parse_header`、`header`，与用例）
- Modify: `crates/rurge-inbound/src/session.rs`（`UdpClient`、`UdpAdmission`、`Dialer::admit_udp` / `associate`）、`src/lib.rs`（模块与导出）、`src/socks5.rs`（`Request`、`reply_bound`、`associate`，与用例）

**Interfaces:**
- Consumes: Task 1 的 `SessionInfo::udp`；既有的 `rurge_net::connector::Target`、`rurge_config::HostName::from_wire`。
- Produces:
  - `pub trait UdpClient: Send + Sync { fn recv<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>>; fn send<'a>(&'a self, payload: &'a [u8], from: &'a Target) -> BoxFuture<'a, io::Result<()>>; }`（`recv` 给出长度与目的地；`send` 以 `from` 为来源发给客户端；客户端还没发过包时静默丢弃）
  - `pub enum UdpAdmission { Accepted, NotSupported, Busy }`
  - `Dialer::admit_udp(&self) -> UdpAdmission`（默认 `NotSupported`）、`Dialer::associate<'a>(&'a self, client: Arc<dyn UdpClient>, session: SessionInfo, closed: CancellationToken) -> BoxFuture<'a, ()>`（默认立即返回）；`session` 是 `SessionInfo::udp(<本机地址>, 0)`，`src` 是客户端、`in_port` 是监听端口、`listener` 是 `Socks5`
  - 导出：`rurge_inbound::{UdpAdmission, UdpClient}`

- [ ] **Step 1: 先写用例**

`crates/rurge-inbound/src/socks5.rs`——把

```rust
    use crate::testing::{FakeDialer, echo_server};
```

换成

```rust
    use crate::session::UdpClient;
    use crate::testing::{FakeDialer, echo_server};
    use rurge_net::BoxFuture;
    use rurge_net::connector::Target;
    use std::sync::atomic::{AtomicBool, Ordering};
```

`crates/rurge-inbound/src/socks5.rs`——把

```rust
        assert_eq!(read_reply(&mut s).await[1], REP_COMMAND_NOT_SUPPORTED);
    }

    /// An empty ATYP=0x03 name has nothing to dial: answer 0x01 and stop.
```

换成

```rust
        assert_eq!(read_reply(&mut s).await[1], REP_COMMAND_NOT_SUPPORTED);
    }

    /// Echoes every datagram back as coming from where it went; records
    /// the association's session and whether it saw `closed`.
    struct UdpEcho {
        admission: UdpAdmission,
        session: std::sync::Mutex<Option<SessionInfo>>,
        ended: AtomicBool,
    }

    impl UdpEcho {
        fn new(admission: UdpAdmission) -> Arc<UdpEcho> {
            Arc::new(UdpEcho {
                admission,
                session: std::sync::Mutex::new(None),
                ended: AtomicBool::new(false),
            })
        }
    }

    impl Dialer for UdpEcho {
        fn dial<'a>(
            &'a self,
            _session: SessionInfo,
        ) -> BoxFuture<'a, Result<crate::session::Dialed, DialError>> {
            unreachable!("UDP only")
        }

        fn relay<'a>(
            &'a self,
            _client: rurge_net::connector::BoxedStream,
            _upstream: rurge_net::connector::BoxedStream,
            _handle: Arc<crate::session::SessionHandle>,
        ) -> BoxFuture<'a, ()> {
            unreachable!("UDP only")
        }

        fn admit_udp(&self) -> UdpAdmission {
            self.admission
        }

        fn associate<'a>(
            &'a self,
            client: Arc<dyn UdpClient>,
            session: SessionInfo,
            closed: CancellationToken,
        ) -> BoxFuture<'a, ()> {
            *self.session.lock().unwrap() = Some(session);
            Box::pin(async move {
                let mut buf = [0u8; 2048];
                loop {
                    tokio::select! {
                        _ = closed.cancelled() => break,
                        got = client.recv(&mut buf) => {
                            let Ok((n, to)) = got else { break };
                            let _ = client.send(&buf[..n], &to).await;
                        }
                    }
                }
                self.ended.store(true, Ordering::SeqCst);
            })
        }
    }

    async fn udp_listener(dialer: Arc<UdpEcho>) -> Running {
        Socks5Listener::bind(
            "127.0.0.1:0".parse().unwrap(),
            dialer,
            ListenerOpts {
                kind: ListenerKind::Socks5,
                ..ListenerOpts::default()
            },
            CancellationToken::new(),
        )
        .await
        .unwrap()
    }

    /// UDP ASSOCIATE with the client's source address; the reply names the
    /// association's port.
    async fn associate_from(addr: SocketAddr, source: SocketAddr) -> (TcpStream, SocketAddr) {
        let mut s = negotiate(addr).await;
        let SocketAddr::V4(v4) = source else {
            panic!("loopback is v4")
        };
        let mut req = vec![VERSION, CMD_UDP_ASSOCIATE, 0, ATYP_V4];
        req.extend_from_slice(&v4.ip().octets());
        req.extend_from_slice(&v4.port().to_be_bytes());
        s.write_all(&req).await.unwrap();
        let r = read_reply(&mut s).await;
        assert_eq!((r[1], r[3]), (REP_SUCCESS, ATYP_V4));
        let bound = SocketAddr::from((
            Ipv4Addr::new(r[4], r[5], r[6], r[7]),
            u16::from_be_bytes([r[8], r[9]]),
        ));
        (s, bound)
    }

    fn datagram(to: &Target, payload: &[u8]) -> Vec<u8> {
        let mut out = crate::udp::header(to).unwrap();
        out.extend_from_slice(payload);
        out
    }

    #[tokio::test]
    async fn udp_associate_carries_datagrams_both_ways() {
        let dialer = UdpEcho::new(UdpAdmission::Accepted);
        let running = udp_listener(dialer.clone()).await;
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (_control, relay) =
            associate_from(running.local_addr, client.local_addr().unwrap()).await;
        assert_eq!(relay.ip(), running.local_addr.ip());
        let to = Target::new(HostName::parse("game.test"), 27015);
        client
            .send_to(&datagram(&to, b"ping"), relay)
            .await
            .unwrap();
        let mut buf = [0u8; 256];
        let (n, from) = tokio::time::timeout(Duration::from_secs(5), client.recv_from(&mut buf))
            .await
            .expect("an answer")
            .unwrap();
        assert_eq!(from, relay);
        assert_eq!(&buf[..n], &datagram(&to, b"ping")[..]);
        let session = dialer.session.lock().unwrap().clone().unwrap();
        assert_eq!(session.transport, Transport::Udp);
        assert_eq!(session.listener, ListenerKind::Socks5);
        assert_eq!(session.src.ip(), client.local_addr().unwrap().ip());
    }

    /// Only the client's own address may use the association; fragments
    /// are dropped.
    #[tokio::test]
    async fn strangers_and_fragments_are_ignored() {
        let dialer = UdpEcho::new(UdpAdmission::Accepted);
        let running = udp_listener(dialer).await;
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (_control, relay) =
            associate_from(running.local_addr, client.local_addr().unwrap()).await;
        let to = Target::new(HostName::parse("10.0.0.1"), 53);
        let stranger = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        stranger.send_to(&datagram(&to, b"x"), relay).await.unwrap();
        let mut fragment = datagram(&to, b"y");
        fragment[2] = 1;
        client.send_to(&fragment, relay).await.unwrap();
        let mut buf = [0u8; 64];
        // a window to observe that nothing comes back
        for socket in [&stranger, &client] {
            assert!(
                tokio::time::timeout(Duration::from_millis(300), socket.recv_from(&mut buf))
                    .await
                    .is_err()
            );
        }
        client.send_to(&datagram(&to, b"z"), relay).await.unwrap();
        let (n, _) = tokio::time::timeout(Duration::from_secs(5), client.recv_from(&mut buf))
            .await
            .expect("an answer")
            .unwrap();
        assert_eq!(&buf[..n], &datagram(&to, b"z")[..]);
    }

    /// The association ends with its control connection.
    #[tokio::test]
    async fn closing_the_control_connection_ends_the_association() {
        let dialer = UdpEcho::new(UdpAdmission::Accepted);
        let running = udp_listener(dialer.clone()).await;
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (control, _relay) =
            associate_from(running.local_addr, client.local_addr().unwrap()).await;
        drop(control);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !dialer.ended.load(Ordering::SeqCst) {
            assert!(
                std::time::Instant::now() < deadline,
                "the association lives on"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// A dialer without UDP answers "command not supported"; one at its
    /// limit "general failure".
    #[tokio::test]
    async fn udp_associate_can_be_refused() {
        for (admission, code) in [
            (UdpAdmission::NotSupported, REP_COMMAND_NOT_SUPPORTED),
            (UdpAdmission::Busy, REP_GENERAL_FAILURE),
        ] {
            let running = udp_listener(UdpEcho::new(admission)).await;
            let mut s = negotiate(running.local_addr).await;
            s.write_all(&[VERSION, CMD_UDP_ASSOCIATE, 0, ATYP_V4, 0, 0, 0, 0, 0, 0])
                .await
                .unwrap();
            assert_eq!(read_reply(&mut s).await[1], code, "{admission:?}");
        }
    }

    /// An empty ATYP=0x03 name has nothing to dial: answer 0x01 and stop.
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-inbound`
Expected: FAIL——`UdpClient`、`UdpAdmission`、`Dialer` 的两个方法与 `udp` 模块还不存在：

```text
error[E0432]: unresolved import `crate::session::UdpClient`
   --> crates\rurge-inbound\src\socks5.rs:211:9
error[E0407]: method `admit_udp` is not a member of trait `Dialer`
   --> crates\rurge-inbound\src\socks5.rs:383:9
error[E0407]: method `associate` is not a member of trait `Dialer`
   --> crates\rurge-inbound\src\socks5.rs:387:9
error[E0433]: failed to resolve: could not find `udp` in the crate root
   --> crates\rurge-inbound\src\socks5.rs:445:30
error[E0412]: cannot find type `UdpAdmission` in this scope
   --> crates\rurge-inbound\src\socks5.rs:351:20
error[E0412]: cannot find type `UdpAdmission` in this scope
   --> crates\rurge-inbound\src\socks5.rs:357:27
error[E0412]: cannot find type `UdpAdmission` in this scope
   --> crates\rurge-inbound\src\socks5.rs:383:32
error[E0425]: cannot find value `CMD_UDP_ASSOCIATE` in this scope
   --> crates\rurge-inbound\src\socks5.rs:431:37
error[E0433]: failed to resolve: use of undeclared type `UdpSocket`
   --> crates\rurge-inbound\src\socks5.rs:454:22
error[E0433]: failed to resolve: use of undeclared type `UdpSocket`
   --> crates\rurge-inbound\src\socks5.rs:482:22
error[E0433]: failed to resolve: use of undeclared type `UdpSocket`
   --> crates\rurge-inbound\src\socks5.rs:486:24
error[E0433]: failed to resolve: use of undeclared type `UdpSocket`
   --> crates\rurge-inbound\src\socks5.rs:513:22
error[E0425]: cannot find value `CMD_UDP_ASSOCIATE` in this scope
   --> crates\rurge-inbound\src\socks5.rs:537:36
error[E0433]: failed to resolve: use of undeclared type `UdpAdmission`
   --> crates\rurge-inbound\src\socks5.rs:452:35
error[E0433]: failed to resolve: use of undeclared type `UdpAdmission`
   --> crates\rurge-inbound\src\socks5.rs:480:35
error[E0433]: failed to resolve: use of undeclared type `UdpAdmission`
   --> crates\rurge-inbound\src\socks5.rs:511:35
error[E0433]: failed to resolve: use of undeclared type `UdpAdmission`
   --> crates\rurge-inbound\src\socks5.rs:532:14
error[E0433]: failed to resolve: use of undeclared type `UdpAdmission`
   --> crates\rurge-inbound\src\socks5.rs:533:14
Some errors have detailed explanations: E0407, E0412, E0425, E0432, E0433.
For more information about an error, try `rustc --explain E0407`.
error: could not compile `rurge-inbound` (lib test) due to 18 previous errors
exit 101
```

- [ ] **Step 3: 实现（新模块自带用例）**

新建 `crates/rurge-inbound/src/udp.rs`：

```rust
//! The client's side of a SOCKS5 UDP association (RFC 1928 §7, phase 2 M5
//! design 5.1): one UDP port per association, datagrams taken only from the
//! client that asked for it.

use crate::session::UdpClient;
use rurge_config::HostName;
use rurge_net::BoxFuture;
use rurge_net::connector::Target;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::OnceLock;
use tokio::net::UdpSocket;

/// The association's port. Datagrams count only when they come from the
/// control connection's client address and, once known, its port: the one
/// the request declared, or else the first datagram's.
pub(crate) struct Socks5UdpClient {
    socket: UdpSocket,
    client_ip: IpAddr,
    client_port: OnceLock<u16>,
}

impl Socks5UdpClient {
    pub(crate) fn new(socket: UdpSocket, client_ip: IpAddr, declared_port: u16) -> Socks5UdpClient {
        let client_port = OnceLock::new();
        if declared_port != 0 {
            let _ = client_port.set(declared_port);
        }
        Socks5UdpClient {
            socket,
            client_ip: canonical(client_ip),
            client_port,
        }
    }

    fn is_client(&self, from: SocketAddr) -> bool {
        if canonical(from.ip()) != self.client_ip {
            return false;
        }
        *self.client_port.get_or_init(|| from.port()) == from.port()
    }
}

/// An IPv4 client may show up as `::ffff:a.b.c.d` on a dual-stack socket.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// RSV RSV FRAG ATYP ADDR PORT: where the datagram goes and where its
/// payload starts; `None` for a fragment (FRAG ≠ 0, never reassembled), an
/// unknown address type, or a name that is no host name.
pub(crate) fn parse_header(datagram: &[u8]) -> Option<(Target, usize)> {
    if datagram.get(..3)? != [0, 0, 0] {
        return None;
    }
    let (host, end) = match *datagram.get(3)? {
        1 => {
            let b: [u8; 4] = datagram.get(4..8)?.try_into().ok()?;
            (HostName::Ip(IpAddr::from(b)), 8)
        }
        4 => {
            let b: [u8; 16] = datagram.get(4..20)?.try_into().ok()?;
            (HostName::Ip(IpAddr::from(b)), 20)
        }
        3 => {
            let len = usize::from(*datagram.get(4)?);
            let name = std::str::from_utf8(datagram.get(5..5 + len)?).ok()?;
            (HostName::from_wire(name)?, 5 + len)
        }
        _ => return None,
    };
    let port = u16::from_be_bytes(datagram.get(end..end + 2)?.try_into().ok()?);
    Some((Target::new(host, port), end + 2))
}

/// The header of a datagram to the client, as coming from `from`; `None`
/// for a name too long to write.
pub(crate) fn header(from: &Target) -> Option<Vec<u8>> {
    let mut out = vec![0, 0, 0];
    match &from.host {
        HostName::Ip(IpAddr::V4(v4)) => {
            out.push(1);
            out.extend_from_slice(&v4.octets());
        }
        HostName::Ip(IpAddr::V6(v6)) => {
            out.push(4);
            out.extend_from_slice(&v6.octets());
        }
        HostName::Domain(name) => {
            out.push(3);
            out.push(u8::try_from(name.len()).ok()?);
            out.extend_from_slice(name.as_bytes());
        }
    }
    out.extend_from_slice(&from.port.to_be_bytes());
    Some(out)
}

impl UdpClient for Socks5UdpClient {
    fn recv<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            loop {
                let (n, from) = match self.socket.recv_from(buf).await {
                    Ok(got) => got,
                    // an ICMP "unreachable" for an earlier answer to the
                    // client, which Windows reports on the next receive
                    Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                    Err(e) => return Err(e),
                };
                if !self.is_client(from) {
                    continue;
                }
                if let Some((to, start)) = parse_header(&buf[..n]) {
                    buf.copy_within(start..n, 0);
                    return Ok((n - start, to));
                }
            }
        })
    }

    fn send<'a>(&'a self, payload: &'a [u8], from: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            // nothing has come from the client yet: nowhere to send to
            let (Some(port), Some(mut datagram)) = (self.client_port.get(), header(from)) else {
                return Ok(());
            };
            datagram.extend_from_slice(payload);
            self.socket
                .send_to(&datagram, SocketAddr::new(self.client_ip, *port))
                .await?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_reads_back_as_it_was_written() {
        for host in ["10.0.0.1", "2001:db8::1", "example.com"] {
            let target = Target::new(HostName::parse(host), 443);
            let mut datagram = header(&target).unwrap();
            let start = datagram.len();
            datagram.extend_from_slice(b"quic");
            assert_eq!(parse_header(&datagram), Some((target, start)));
        }
    }

    #[test]
    fn fragments_and_bad_names_are_no_datagrams() {
        let mut datagram = header(&Target::new(HostName::parse("10.0.0.1"), 53)).unwrap();
        datagram[2] = 1;
        assert_eq!(parse_header(&datagram), None, "a fragment");
        let mut bad = vec![0, 0, 0, 3, 3];
        bad.extend_from_slice(b"a b");
        bad.extend_from_slice(&53u16.to_be_bytes());
        assert_eq!(parse_header(&bad), None, "no host name");
        assert_eq!(parse_header(&[0, 0, 0, 1, 10]), None, "cut short");
        assert_eq!(parse_header(&[0, 0, 0, 9, 0, 0]), None, "unknown type");
    }

    #[test]
    fn an_ipv4_mapped_client_is_the_ipv4_client() {
        assert_eq!(
            canonical("::ffff:127.0.0.1".parse().unwrap()),
            "127.0.0.1".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            canonical("::1".parse().unwrap()),
            "::1".parse::<IpAddr>().unwrap()
        );
    }
}
```

`crates/rurge-inbound/src/session.rs`——把

```rust
use rurge_net::connector::BoxedStream;
use rurge_proto::RejectKind;
```

换成

```rust
use rurge_net::connector::{BoxedStream, Target};
use rurge_proto::RejectKind;
use std::io;
```

`crates/rurge-inbound/src/session.rs`——把

```rust
        handle: Arc<SessionHandle>,
    },
}

/// Implemented by the engine: rules → policy → outbound (`dial`), then the
```

换成

```rust
        handle: Arc<SessionHandle>,
    },
}

/// The client's side of one UDP association (phase 2 M5 design 5.1), as the
/// engine sees it. `recv` and `send` may run at the same time.
pub trait UdpClient: Send + Sync {
    /// The next datagram from the client: its length in `buf`, and where it goes.
    fn recv<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>>;
    /// Sends `payload` to the client as a datagram from `from`.
    fn send<'a>(&'a self, payload: &'a [u8], from: &'a Target) -> BoxFuture<'a, io::Result<()>>;
}

/// Whether a UDP association may start (`Dialer::admit_udp`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UdpAdmission {
    Accepted,
    /// The dialer carries no UDP: "command not supported".
    NotSupported,
    /// Too many associations right now: "general failure".
    Busy,
}

/// Implemented by the engine: rules → policy → outbound (`dial`), then the
```

`crates/rurge-inbound/src/session.rs`——把

```rust
    ) -> BoxFuture<'a, ()>;
```

换成

```rust
    ) -> BoxFuture<'a, ()>;
    /// Asked before a UDP association is set up.
    fn admit_udp(&self) -> UdpAdmission {
        UdpAdmission::NotSupported
    }
    /// Serves one UDP association until `closed` fires (the control
    /// connection ended). `session` describes the client — its address, the
    /// listener — and each flow of the association starts from a copy of it
    /// with its own destination. Returning early ends the association: the
    /// listener closes the control connection.
    fn associate<'a>(
        &'a self,
        client: Arc<dyn UdpClient>,
        session: SessionInfo,
        closed: CancellationToken,
    ) -> BoxFuture<'a, ()> {
        let _ = (client, session, closed);
        Box::pin(std::future::ready(()))
    }
```

`crates/rurge-inbound/src/lib.rs`——把

```rust
pub(crate) mod testing;
```

换成

```rust
pub(crate) mod testing;
mod udp;
```

`crates/rurge-inbound/src/lib.rs`——把

```rust
pub use session::{Counting, DialError, Dialed, Dialer, FailKind, SessionHandle, SessionOutcome};
```

换成

```rust
pub use session::{
    Counting, DialError, Dialed, Dialer, FailKind, SessionHandle, SessionOutcome, UdpAdmission,
    UdpClient,
};
```

`crates/rurge-inbound/src/socks5.rs`——把

```rust
//! SOCKS5 (RFC 1928) listener: no authentication, CONNECT only (M3 design §6.3).

use crate::listener::{ListenerOpts, Running, bind, serve};
use crate::session::{DialError, Dialer, FailKind, SessionOutcome};
```

换成

```rust
//! SOCKS5 (RFC 1928) listener: no authentication, CONNECT (M3 design §6.3)
//! and UDP ASSOCIATE (phase 2 M5 design 5.1).

use crate::listener::{ListenerOpts, Running, bind, serve};
use crate::session::{DialError, Dialer, FailKind, SessionOutcome, UdpAdmission};
use crate::udp::Socks5UdpClient;
```

`crates/rurge-inbound/src/socks5.rs`——把

```rust
use tokio::net::TcpStream;
```

换成

```rust
use tokio::net::{TcpStream, UdpSocket};
```

`crates/rurge-inbound/src/socks5.rs`——把

```rust
const CMD_CONNECT: u8 = 0x01;
```

换成

```rust
const CMD_CONNECT: u8 = 0x01;
const CMD_UDP_ASSOCIATE: u8 = 0x03;
```

`crates/rurge-inbound/src/socks5.rs`——把

```rust
async fn read_request(stream: &mut TcpStream) -> io::Result<Result<(HostName, u16), u8>> {
```

换成

```rust
/// A success reply naming `bound`.
fn reply_bound(bound: SocketAddr) -> Vec<u8> {
    let mut out = vec![VERSION, REP_SUCCESS, 0x00];
    match bound.ip() {
        IpAddr::V4(v4) => {
            out.push(ATYP_V4);
            out.extend_from_slice(&v4.octets());
        }
        IpAddr::V6(v6) => {
            out.push(ATYP_V6);
            out.extend_from_slice(&v6.octets());
        }
    }
    out.extend_from_slice(&bound.port().to_be_bytes());
    out
}

/// A request the listener serves: the command and its address.
struct Request {
    command: u8,
    host: HostName,
    port: u16,
}

async fn read_request(stream: &mut TcpStream) -> io::Result<Result<Request, u8>> {
```

`crates/rurge-inbound/src/socks5.rs`——把

```rust
    if head[1] != CMD_CONNECT {
        return Ok(Err(REP_COMMAND_NOT_SUPPORTED));
    }
    Ok(Ok((host, port)))
}

/// Method negotiation plus the CONNECT request. `Ok(None)` means the client
/// was already answered (unacceptable method) and the session is over.
async fn handshake(stream: &mut TcpStream) -> io::Result<Option<Result<(HostName, u16), u8>>> {
```

换成

```rust
    if head[1] != CMD_CONNECT && head[1] != CMD_UDP_ASSOCIATE {
        return Ok(Err(REP_COMMAND_NOT_SUPPORTED));
    }
    Ok(Ok(Request {
        command: head[1],
        host,
        port,
    }))
}

/// Method negotiation plus the request. `Ok(None)` means the client was
/// already answered (unacceptable method) and the session is over.
async fn handshake(stream: &mut TcpStream) -> io::Result<Option<Result<Request, u8>>> {
```

`crates/rurge-inbound/src/socks5.rs`——把

```rust
    let (host, port) = match negotiated {
        Some(Ok(target)) => target,
```

换成

```rust
    let Request {
        command,
        host,
        port,
    } = match negotiated {
        Some(Ok(request)) => request,
```

`crates/rurge-inbound/src/socks5.rs`——把

```rust
    };
    let mut session = SessionInfo::tcp(host, port);
```

换成

```rust
    };
    if command == CMD_UDP_ASSOCIATE {
        return associate(stream, peer, dialer, port).await;
    }
    let mut session = SessionInfo::tcp(host, port);
```

`crates/rurge-inbound/src/socks5.rs`——把

```rust
        }
    }
}

#[cfg(test)]
```

换成

```rust
        }
    }
}

/// UDP ASSOCIATE: a UDP port on the address the client reached this
/// listener at, served by the dialer until the control connection ends
/// (RFC 1928 §7). `declared_port` is the client's source port when it said
/// (0: learnt from its first datagram).
async fn associate(
    mut control: TcpStream,
    peer: SocketAddr,
    dialer: Arc<dyn Dialer>,
    declared_port: u16,
) -> io::Result<()> {
    match dialer.admit_udp() {
        UdpAdmission::Accepted => {}
        UdpAdmission::NotSupported => {
            return control.write_all(&reply(REP_COMMAND_NOT_SUPPORTED)).await;
        }
        UdpAdmission::Busy => return control.write_all(&reply(REP_GENERAL_FAILURE)).await,
    }
    let here = control.local_addr()?;
    let socket = match UdpSocket::bind(SocketAddr::new(here.ip(), 0)).await {
        Ok(socket) => socket,
        Err(e) => {
            control.write_all(&reply(REP_GENERAL_FAILURE)).await?;
            return Err(e);
        }
    };
    control
        .write_all(&reply_bound(socket.local_addr()?))
        .await?;
    let client = Arc::new(Socks5UdpClient::new(socket, peer.ip(), declared_port));
    let mut session = SessionInfo::udp(HostName::Ip(here.ip()), 0);
    session.src = peer;
    session.in_port = here.port();
    session.listener = ListenerKind::Socks5;
    let closed = CancellationToken::new();
    let serving = dialer.associate(client, session, closed.clone());
    tokio::pin!(serving);
    // whatever else the client writes on the control connection means nothing
    let mut sink = [0u8; 256];
    loop {
        tokio::select! {
            _ = &mut serving => return Ok(()),
            read = control.read(&mut sink) => {
                if !matches!(read, Ok(n) if n > 0) {
                    closed.cancel();
                    serving.await;
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(test)]
```

要点：
- 命令字先读完整个请求（地址与端口）再判断，BIND（0x02）等照旧回 0x07。
- `associate` 同时等 dialer 的 `associate` 与控制连接：控制连接读到 EOF 或出错时先 `closed.cancel()`，再等 dialer 收尾；dialer 先返回（到了上限等）时直接结束，控制连接随之关闭。
- `strangers_and_fragments_are_ignored` 里的 300 ms 只用来观察"什么也没回来"。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-inbound` → 通过（新增 `udp_associate_carries_datagrams_both_ways`、`strangers_and_fragments_are_ignored`、`closing_the_control_connection_ends_the_association`、`udp_associate_can_be_refused`，`udp` 模块 3 条）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-inbound
git commit -m "feat(inbound): SOCKS5 UDP ASSOCIATE 与 Dialer 的 UDP 接口（UdpClient、admit_udp、associate）"
```


### Task 4: `rurge-engine`——UDP 关联、流、全锥归属与回收；请求记录的 `transport`

引擎实现 Task 3 的两个方法（设计第 6 节，P7、P11、P12）。一个客户端关联（`Association`）从客户端收包，按目的地址找流；没有就新建一条流（`Flow`：`SessionInfo::udp` 走一遍与 TCP 相同的规则匹配与策略解析，队列 64 个包，满了丢新包）。流按"策略的出站对象"共用载体（`Carrier`，每个关联内每个出站一个），一个载体上收到的包按全锥归属：先按来源地址找发往它的流，找不到就算在该载体上最早的那条流——来源地址原样回给客户端（D1）。流 60 秒没有包回收，DNS（端口 53）的流在第一个回包后 10 秒回收；关联的流数上限 1024、全局关联上限 4096，超了拒绝并各告警一次（D11）。拨号失败的流留到它的回收时限，其间的包丢弃。REJECT 系列直接丢包（D8）；不支持 UDP 的策略本任务先一律拒绝（Task 5 接上 `udp-policy-not-supported-behaviour`）。请求记录多一列 `transport`（`tcp` / `udp`），API 照出。

**Files:**
- Create: `crates/rurge-engine/src/udp.rs`（关联、流、载体、回收，与用例）、`crates/rurge-engine/tests/udp.rs`
- Modify: `crates/rurge-engine/src/lib.rs`、`src/engine.rs`（`Arc::new_cyclic` 取得自身的 `Weak`、关联计数与告警、`snapshot` / `choose_policy` / `Chosen` 改 `pub(crate)`、`Dialer` 的两个方法）、`src/observe.rs`（`transport`）、`tests/common/mod.rs`（`UdpAssociation`、`udp_associate`、`udp_echo`）
- Modify: `crates/rurge-api/src/routes/requests.rs`（`transport`，与用例）
- Modify: `crates/rurge-net/src/connector.rs`（`Target` 加 `Hash`）

**Interfaces:**
- Consumes: Task 1 的 `PacketSocket` / `SessionInfo::udp`；Task 2 的 `Outbound::udp` / `open_udp`、`UdpSupport`；Task 3 的 `UdpClient`、`UdpAdmission`、`Dialer::admit_udp` / `associate`。
- Produces:
  - `rurge_engine::observe::RequestRecord.transport: Transport`；API 的请求 JSON 多 `"transport": "tcp" | "udp"`
  - `pub(crate)`：`udp::serve`、`udp::deadline(last: Instant, answered: Option<Instant>, port: u16) -> Instant`；`pub` 常量 `udp::{FLOW_IDLE, DNS_LINGER, FLOWS_PER_ASSOCIATION, ASSOCIATIONS}`（60 s、10 s、1024、4096），私有的 `QUEUE`（64）；`Engine::{snapshot, choose_policy, session_token, warn_udp_limit}`、`engine::Chosen`
  - 测试辅助：`common::UdpAssociation`（`async fn send(&self, host: &str, port: u16, payload: &[u8])`、`async fn recv(&self) -> (SocketAddr, Vec<u8>)`）、`common::udp_associate(socks: SocketAddr) -> UdpAssociation`、`common::udp_echo() -> (SocketAddr, Arc<Mutex<Vec<SocketAddr>>>)`（第二项是回显服务见过的来源地址）

- [ ] **Step 1: 先写用例**

API 的一列：

`crates/rurge-api/src/routes/requests.rs`——把

```rust
            listener,
```

换成

```rust
            listener,
            transport: Transport::Tcp,
```

`crates/rurge-api/src/routes/requests.rs`——把

```rust
        assert_eq!(j.listener, "socks5");
```

换成

```rust
        assert_eq!(j.listener, "socks5");
        assert_eq!(j.transport, "tcp");
        r.transport = Transport::Udp;
        assert_eq!(RequestJson::from(&r).transport, "udp");
```

引擎端到端的辅助与用例：

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// One request/response over a fresh connection to rurge's HTTP listener.
```

换成

```rust
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// A SOCKS5 UDP association through rurge's listener (phase 2 M5): the
/// control connection, the client's UDP socket, and the association's port.
pub struct UdpAssociation {
    pub control: TcpStream,
    pub socket: tokio::net::UdpSocket,
    pub relay: SocketAddr,
}

pub async fn udp_associate(socks: SocketAddr) -> UdpAssociation {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut control = TcpStream::connect(socks).await.unwrap();
    control.write_all(&[5, 1, 0]).await.unwrap();
    let mut method = [0u8; 2];
    control.read_exact(&mut method).await.unwrap();
    assert_eq!(method, [5, 0]);
    let SocketAddr::V4(me) = socket.local_addr().unwrap() else {
        panic!("loopback is v4")
    };
    let mut request = vec![5, 3, 0, 1];
    request.extend_from_slice(&me.ip().octets());
    request.extend_from_slice(&me.port().to_be_bytes());
    control.write_all(&request).await.unwrap();
    let mut reply = [0u8; 10];
    control.read_exact(&mut reply).await.unwrap();
    assert_eq!((reply[1], reply[3]), (0, 1), "{reply:?}");
    let relay = SocketAddr::from((
        [reply[4], reply[5], reply[6], reply[7]],
        u16::from_be_bytes([reply[8], reply[9]]),
    ));
    UdpAssociation {
        control,
        socket,
        relay,
    }
}

impl UdpAssociation {
    /// Sends `payload` to `host:port` (an IP literal or a name).
    pub async fn send(&self, host: &str, port: u16, payload: &[u8]) {
        let mut datagram = vec![0, 0, 0];
        match host.parse::<std::net::IpAddr>() {
            Ok(std::net::IpAddr::V4(v4)) => {
                datagram.push(1);
                datagram.extend_from_slice(&v4.octets());
            }
            Ok(std::net::IpAddr::V6(v6)) => {
                datagram.push(4);
                datagram.extend_from_slice(&v6.octets());
            }
            Err(_) => {
                datagram.push(3);
                datagram.push(host.len() as u8);
                datagram.extend_from_slice(host.as_bytes());
            }
        }
        datagram.extend_from_slice(&port.to_be_bytes());
        datagram.extend_from_slice(payload);
        self.socket.send_to(&datagram, self.relay).await.unwrap();
    }

    /// The next datagram: where it says it came from (`ip:port`) and its payload.
    pub async fn recv(&self) -> (SocketAddr, Vec<u8>) {
        let mut buf = [0u8; 2048];
        let (n, from) =
            tokio::time::timeout(Duration::from_secs(5), self.socket.recv_from(&mut buf))
                .await
                .expect("a datagram comes back")
                .unwrap();
        assert_eq!(from, self.relay);
        assert_eq!(&buf[..3], [0, 0, 0]);
        let (ip, rest): (std::net::IpAddr, usize) = match buf[3] {
            1 => (<[u8; 4]>::try_from(&buf[4..8]).unwrap().into(), 8),
            4 => (<[u8; 16]>::try_from(&buf[4..20]).unwrap().into(), 20),
            other => panic!("address type {other}"),
        };
        let port = u16::from_be_bytes([buf[rest], buf[rest + 1]]);
        (SocketAddr::new(ip, port), buf[rest + 2..n].to_vec())
    }

    /// Whether nothing comes back within `window` (only to observe that
    /// nothing happens).
    pub async fn quiet_for(&self, window: Duration) -> bool {
        let mut buf = [0u8; 2048];
        tokio::time::timeout(window, self.socket.recv_from(&mut buf))
            .await
            .is_err()
    }
}

/// A loopback UDP server answering every datagram with itself; it notes
/// who wrote to it.
pub async fn udp_echo() -> (SocketAddr, Arc<std::sync::Mutex<Vec<SocketAddr>>>) {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = socket.local_addr().unwrap();
    let seen: Arc<std::sync::Mutex<Vec<SocketAddr>>> = Arc::default();
    let log = seen.clone();
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        loop {
            let Ok((n, from)) = socket.recv_from(&mut buf).await else {
                continue;
            };
            log.lock().unwrap().push(from);
            let _ = socket.send_to(&buf[..n], from).await;
        }
    });
    (addr, seen)
}

/// One request/response over a fresh connection to rurge's HTTP listener.
```

新建 `crates/rurge-engine/tests/udp.rs`：

```rust
//! The UDP pipeline through the engine (phase 2 M5 design §5): SOCKS5 UDP
//! ASSOCIATE in, rules and policies per destination, one carrier per
//! outbound, full cone.

mod common;
use common::*;
use rurge_config::session::Transport;
use rurge_engine::RequestRecord;

fn udp_records(h: &Harness) -> Vec<RequestRecord> {
    h.engine
        .request_log()
        .recent(4096)
        .into_iter()
        .filter(|r| r.transport == Transport::Udp)
        .collect()
}

/// Waits until `count` UDP records are finished, and returns them.
async fn finished(h: &Harness, count: usize) -> Vec<RequestRecord> {
    wait_until("the UDP flows to finish", || udp_records(h).len() >= count).await;
    udp_records(h)
}

#[tokio::test]
async fn a_datagram_goes_direct_and_comes_back() {
    let h = harness(Profile::default()).await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"ping").await;
    assert_eq!(association.recv().await, (echo, b"ping".to_vec()));
    // a second datagram rides the same flow
    association.send("127.0.0.1", echo.port(), b"pong").await;
    assert_eq!(association.recv().await, (echo, b"pong".to_vec()));
    drop(association);
    let records = finished(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    let r = &records[0];
    assert_eq!(r.dst, format!("127.0.0.1:{}", echo.port()));
    assert_eq!(r.policy, ["DIRECT"]);
    assert_eq!(r.status, RecordStatus::Completed);
    assert_eq!((r.up, r.down), (8, 8));
    assert!(r.connect_ms.is_some() && r.first_byte_ms.is_some(), "{r:?}");
}

/// DIRECT looks a name up here (with the profile's DNS) and sends to the
/// address; the record keeps the name.
#[tokio::test]
async fn a_name_is_looked_up_for_direct() {
    let h = harness(Profile::default()).await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association
        .send("target.test", echo.port(), b"by name")
        .await;
    assert_eq!(association.recv().await, (echo, b"by name".to_vec()));
    drop(association);
    let records = finished(&h, 1).await;
    assert_eq!(records[0].dst, format!("target.test:{}", echo.port()));
    assert_eq!((records[0].up, records[0].down), (7, 7));
}

/// Full cone (M5-D2): once the client has written out, anyone may write
/// back to the carrier's address and reach the client.
#[tokio::test]
async fn anyone_may_answer_the_carrier() {
    let h = harness(Profile::default()).await;
    let (echo, seen) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"hello").await;
    association.recv().await;
    let carrier = seen.lock().unwrap()[0];
    let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    stranger.send_to(b"from elsewhere", carrier).await.unwrap();
    assert_eq!(
        association.recv().await,
        (stranger.local_addr().unwrap(), b"from elsewhere".to_vec())
    );
    // it is counted on the association's flow
    drop(association);
    let records = finished(&h, 1).await;
    assert_eq!((records[0].up, records[0].down), (5, 5 + 14));
}

/// A datagram to a port nobody listens on (an ICMP "port unreachable"
/// comes back, which Windows reports on the next receive) does not break
/// the carrier the other flows share.
#[tokio::test]
async fn a_closed_port_does_not_break_the_carrier() {
    let h = harness(Profile::default()).await;
    let (echo, _) = udp_echo().await;
    let closed = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let closed_port = closed.local_addr().unwrap().port();
    drop(closed);
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"one").await;
    association.recv().await;
    association.send("127.0.0.1", closed_port, b"lost").await;
    // a window for the unreachable answer to arrive
    assert!(association.quiet_for(Duration::from_millis(300)).await);
    association.send("127.0.0.1", echo.port(), b"two").await;
    assert_eq!(association.recv().await, (echo, b"two".to_vec()));
}

/// A REJECT rule drops the datagrams; the record says REJECT.
#[tokio::test]
async fn a_reject_rule_drops_the_datagrams() {
    let h = harness(Profile {
        rules: "DOMAIN,blocked.test,REJECT",
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    association.send("blocked.test", 443, b"x").await;
    association.send("blocked.test", 443, b"y").await;
    assert!(association.quiet_for(Duration::from_millis(300)).await);
    let records = finished(&h, 1).await;
    assert_eq!(records.len(), 1, "one flow however many datagrams");
    assert_eq!(records[0].status, RecordStatus::Rejected("REJECT".into()));
    assert_eq!(records[0].policy, ["REJECT"]);
}

/// A policy that carries no UDP (here `http`) rejects the flow and says
/// why (`udp-policy-not-supported-behaviour` defaults to REJECT).
#[tokio::test]
async fn a_policy_without_udp_rejects() {
    let h = harness(Profile {
        proxies: "Web = http, 127.0.0.1, 9",
        rules: "DOMAIN,web.test,Web",
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    association.send("web.test", 53, b"q").await;
    assert!(association.quiet_for(Duration::from_millis(300)).await);
    let records = finished(&h, 1).await;
    assert_eq!(records[0].status, RecordStatus::Rejected("REJECT".into()));
    assert_eq!(
        records[0].error.as_deref(),
        Some("policy does not support UDP")
    );
}

/// The association — every flow of it — ends with the control connection.
#[tokio::test]
async fn closing_the_control_connection_ends_every_flow() {
    let h = harness(Profile::default()).await;
    let ((one, _), (two, _)) = (udp_echo().await, udp_echo().await);
    let association = udp_associate(h.socks()).await;
    for echo in [one, two] {
        association.send("127.0.0.1", echo.port(), b"x").await;
        association.recv().await;
    }
    assert_eq!(
        h.engine
            .request_log()
            .active()
            .iter()
            .filter(|r| r.transport == Transport::Udp)
            .count(),
        2
    );
    drop(association.control);
    let records = finished(&h, 2).await;
    assert!(
        records.iter().all(|r| r.status == RecordStatus::Completed),
        "{records:?}"
    );
}

/// At most `FLOWS_PER_ASSOCIATION` flows (M5-D11); the rest are dropped.
/// Datagrams to closed ports also check that a port-unreachable answer
/// does not break the carrier.
#[tokio::test]
async fn an_association_has_at_most_1024_flows() {
    let h = harness(Profile::default()).await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    let mut port = 20000u16;
    let mut sent = 0;
    while sent < 1025 {
        port += 1;
        if port == echo.port() {
            continue;
        }
        association.send("127.0.0.1", port, b"x").await;
        sent += 1;
    }
    wait_until("1024 flows", || {
        h.engine
            .request_log()
            .active()
            .iter()
            .filter(|r| r.transport == Transport::Udp)
            .count()
            == 1024
    })
    .await;
    // a new destination is not taken
    association.send("127.0.0.1", echo.port(), b"late").await;
    assert!(association.quiet_for(Duration::from_millis(300)).await);
    drop(association);
    wait_until("every flow to finish", || {
        !h.engine
            .request_log()
            .active()
            .iter()
            .any(|r| r.transport == Transport::Udp)
    })
    .await;
    // the log keeps the newest 1000 records: the late destination would be among them
    let late = format!("127.0.0.1:{}", echo.port());
    assert!(!udp_records(&h).iter().any(|r| r.dst == late));
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-engine --test udp`
Expected: FAIL——请求记录还没有 `transport`（引擎也还不接 UDP ASSOCIATE）：

```text
error[E0609]: no field `transport` on type `&RequestRecord`
  --> crates\rurge-engine\tests\udp.rs:15:23
error[E0609]: no field `transport` on type `&&RequestRecord`
   --> crates\rurge-engine\tests\udp.rs:159:27
error[E0609]: no field `transport` on type `&&RequestRecord`
   --> crates\rurge-engine\tests\udp.rs:194:27
error[E0609]: no field `transport` on type `&RequestRecord`
   --> crates\rurge-engine\tests\udp.rs:208:24
For more information about this error, try `rustc --explain E0609`.
error: could not compile `rurge-engine` (test "udp") due to 4 previous errors
exit 101
```

- [ ] **Step 3: 实现**

`crates/rurge-net/src/connector.rs`——把

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
```

换成

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
```

`crates/rurge-engine/src/observe.rs`——把

```rust
use rurge_config::session::ListenerKind;
```

换成

```rust
use rurge_config::session::{ListenerKind, Transport};
```

`crates/rurge-engine/src/observe.rs`——把

```rust
    pub listener: ListenerKind,
```

换成

```rust
    pub listener: ListenerKind,
    /// TCP, or a UDP flow (phase 2 M5 design 5.2).
    pub transport: Transport,
```

`crates/rurge-engine/src/observe.rs`——把

```rust
        listener: s.listener,
```

换成

```rust
        listener: s.listener,
        transport: s.transport,
```

`crates/rurge-api/src/routes/requests.rs`——把

```rust
use rurge_config::session::ListenerKind;
```

换成

```rust
use rurge_config::session::{ListenerKind, Transport};
```

`crates/rurge-api/src/routes/requests.rs`——把

```rust
    pub listener: &'static str,
```

换成

```rust
    pub listener: &'static str,
    pub transport: &'static str,
```

`crates/rurge-api/src/routes/requests.rs`——把

```rust
            listener: listener_name(r.listener),
```

换成

```rust
            listener: listener_name(r.listener),
            transport: match r.transport {
                Transport::Tcp => "tcp",
                Transport::Udp => "udp",
            },
```

新建 `crates/rurge-engine/src/udp.rs`：

```rust
//! The UDP pipeline (phase 2 M5 design §5): the flows of one SOCKS5 UDP
//! association — one per destination, each with its own request record —
//! and the carriers they share, one per outbound. A carrier hands back
//! every datagram it receives, whoever sent it (full cone, M5-D2).

use crate::auto::{EVALUATION_FAILED, resolve_ready};
use crate::engine::{CONNECT_TIMEOUT, Chosen, Engine};
use rurge_config::policy::Builtin;
use rurge_config::session::SessionInfo;
use rurge_inbound::{SessionHandle, SessionOutcome, UdpClient};
use rurge_net::connector::{ConnectOpts, PacketSocket, Target};
use rurge_policy::TerminalKind;
use rurge_policy::auto::SelectCtx;
use rurge_proto::{OutboundError, OutboundRef, RejectKind, UdpSupport};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;

/// A flow nothing has gone through for this long is reclaimed (M5-D7).
pub const FLOW_IDLE: Duration = Duration::from_secs(60);
/// A DNS flow (destination port 53) is reclaimed this long after its first
/// answer (M5-D7).
pub const DNS_LINGER: Duration = Duration::from_secs(10);
/// At most this many flows per association (M5-D11).
pub const FLOWS_PER_ASSOCIATION: usize = 1024;
/// At most this many associations at once (M5-D11).
pub const ASSOCIATIONS: usize = 4096;
/// Datagrams waiting for their flow to get going; more are dropped.
const QUEUE: usize = 64;
/// Room for the largest datagram.
const DATAGRAM: usize = 65536;

/// When a flow is reclaimed: `FLOW_IDLE` after its last datagram either
/// way, and a DNS flow `DNS_LINGER` after its first answer.
pub(crate) fn deadline(last: Instant, answered: Option<Instant>, port: u16) -> Instant {
    let idle = last + FLOW_IDLE;
    match answered {
        Some(at) if port == 53 => idle.min(at + DNS_LINGER),
        _ => idle,
    }
}

struct Times {
    last: Instant,
    answered: Option<Instant>,
}

/// One destination of an association, as its carrier sees it.
struct Flow {
    handle: Arc<SessionHandle>,
    port: u16,
    times: Mutex<Times>,
}

impl Flow {
    fn new(handle: Arc<SessionHandle>, port: u16) -> Flow {
        Flow {
            handle,
            port,
            times: Mutex::new(Times {
                last: Instant::now(),
                answered: None,
            }),
        }
    }

    fn touch(&self) {
        self.times.lock().expect("flow times").last = Instant::now();
    }

    fn answered(&self) {
        let now = Instant::now();
        let mut times = self.times.lock().expect("flow times");
        times.last = now;
        times.answered.get_or_insert(now);
    }

    fn deadline(&self) -> Instant {
        let times = self.times.lock().expect("flow times");
        deadline(times.last, times.answered, self.port)
    }
}

/// Which flow a datagram coming back counts for: the one that sent to its
/// source, else the oldest flow still on the carrier (full cone).
#[derive(Default)]
struct Routes(Mutex<Vec<(Target, Weak<Flow>)>>);

impl Routes {
    fn add(&self, to: Target, flow: &Arc<Flow>) {
        let mut routes = self.0.lock().expect("routes");
        routes.retain(|(_, f)| f.strong_count() > 0);
        routes.push((to, Arc::downgrade(flow)));
    }

    fn remove(&self, flow: &Arc<Flow>) {
        let mut routes = self.0.lock().expect("routes");
        routes
            .retain(|(_, f)| f.strong_count() > 0 && !std::ptr::eq(f.as_ptr(), Arc::as_ptr(flow)));
    }

    fn flow_for(&self, from: &Target) -> Option<Arc<Flow>> {
        let routes = self.0.lock().expect("routes");
        routes
            .iter()
            .find(|(to, _)| to == from)
            .and_then(|(_, f)| f.upgrade())
            .or_else(|| routes.iter().find_map(|(_, f)| f.upgrade()))
    }
}

/// An outbound's carrier within one association, and the task handing its
/// datagrams back to the client.
struct Carrier {
    socket: Arc<dyn PacketSocket>,
    routes: Arc<Routes>,
    _receive: AbortOnDropHandle<()>,
}

async fn receive(socket: Arc<dyn PacketSocket>, routes: Arc<Routes>, client: Arc<dyn UdpClient>) {
    let mut buf = vec![0u8; DATAGRAM];
    while let Ok((n, from)) = socket.recv_from(&mut buf).await {
        if let Some(flow) = routes.flow_for(&from) {
            flow.handle.add_down(n as u64);
            flow.handle.mark_first_byte();
            flow.answered();
        }
        if client.send(&buf[..n], &from).await.is_err() {
            return;
        }
    }
}

/// The association's carriers, one per outbound object, opened once each.
#[derive(Default)]
struct Carriers(Mutex<HashMap<usize, Arc<tokio::sync::Mutex<Weak<Carrier>>>>>);

impl Carriers {
    async fn get(
        &self,
        outbound: &OutboundRef,
        client: &Arc<dyn UdpClient>,
    ) -> Result<Arc<Carrier>, OutboundError> {
        let key = Arc::as_ptr(outbound) as *const () as usize;
        let slot = self
            .0
            .lock()
            .expect("carriers")
            .entry(key)
            .or_default()
            .clone();
        let mut held = slot.lock().await;
        if let Some(carrier) = held.upgrade() {
            return Ok(carrier);
        }
        let opts = ConnectOpts {
            timeout: CONNECT_TIMEOUT,
        };
        let socket: Arc<dyn PacketSocket> = Arc::from(outbound.open_udp(&opts).await?);
        let routes = Arc::new(Routes::default());
        let task = tokio::spawn(receive(socket.clone(), routes.clone(), client.clone()));
        let carrier = Arc::new(Carrier {
            socket,
            routes,
            _receive: AbortOnDropHandle::new(task),
        });
        *held = Arc::downgrade(&carrier);
        Ok(carrier)
    }
}

/// What every flow of an association shares.
struct Association {
    client: Arc<dyn UdpClient>,
    template: SessionInfo,
    carriers: Carriers,
    /// Fires when the association ends: the flows finish.
    ended: CancellationToken,
    done: mpsc::UnboundedSender<(Target, u64)>,
}

/// Serves one association until the client's control connection ends
/// (`closed`) or the engine cancels every session.
pub(crate) async fn serve(
    engine: Arc<Engine>,
    client: Arc<dyn UdpClient>,
    template: SessionInfo,
    closed: CancellationToken,
) {
    let (done, mut finished) = mpsc::unbounded_channel();
    let association = Arc::new(Association {
        client: client.clone(),
        template,
        carriers: Carriers::default(),
        ended: engine.session_token(),
        done,
    });
    let mut flows: HashMap<Target, (u64, mpsc::Sender<Vec<u8>>)> = HashMap::new();
    let mut next = 0u64;
    let mut buf = vec![0u8; DATAGRAM];
    loop {
        tokio::select! {
            _ = closed.cancelled() => break,
            _ = association.ended.cancelled() => break,
            Some((to, id)) = finished.recv() => {
                if flows.get(&to).is_some_and(|(current, _)| *current == id) {
                    flows.remove(&to);
                }
            }
            got = client.recv(&mut buf) => {
                let Ok((n, to)) = got else { break };
                let mut datagram = buf[..n].to_vec();
                if let Some((_, queue)) = flows.get(&to) {
                    match queue.try_send(datagram) {
                        Ok(()) | Err(TrySendError::Full(_)) => continue,
                        // the flow is ending: this datagram starts a new one
                        Err(TrySendError::Closed(back)) => {
                            flows.remove(&to);
                            datagram = back;
                        }
                    }
                }
                if flows.len() >= FLOWS_PER_ASSOCIATION {
                    engine.warn_udp_limit("udp: too many flows on this association");
                    continue;
                }
                next += 1;
                let (queue, waiting) = mpsc::channel(QUEUE);
                flows.insert(to.clone(), (next, queue));
                engine.tracker().spawn(run_flow(
                    engine.clone(),
                    association.clone(),
                    to,
                    datagram,
                    waiting,
                    next,
                ));
            }
        }
    }
    association.ended.cancel();
}

async fn run_flow(
    engine: Arc<Engine>,
    association: Arc<Association>,
    to: Target,
    first: Vec<u8>,
    mut waiting: mpsc::Receiver<Vec<u8>>,
    id: u64,
) {
    let mut session = association.template.clone();
    session.dst_host = to.host.clone();
    session.dst_port = to.port;
    let handle = engine.new_handle(session);
    let opened = tokio::select! {
        _ = association.ended.cancelled() => Err(SessionOutcome::Completed),
        opened = open(&engine, &association, &handle, &to) => opened,
    };
    match opened {
        Ok((flow, carrier, send_to)) => {
            let outcome =
                forward(&association, &flow, &carrier, &send_to, first, &mut waiting).await;
            carrier.routes.remove(&flow);
            handle.finish(outcome);
        }
        Err(outcome) => {
            handle.finish(outcome);
            // the flow stays, dropping what comes for it, until it is idle
            drain(&association, &mut waiting).await;
        }
    }
    let _ = association.done.send((to, id));
}

/// A failure, with whatever note is already on the record in front of it.
fn failed(handle: &SessionHandle, message: impl Into<String>) -> SessionOutcome {
    let message = message.into();
    if let Some(note) = handle.error() {
        handle.set_error(format!("{note}; {message}"));
    }
    SessionOutcome::Failed(message)
}

/// Rules → policy → the outbound's carrier (M5 design 5.2).
async fn open(
    engine: &Engine,
    association: &Association,
    handle: &Arc<SessionHandle>,
    to: &Target,
) -> Result<(Arc<Flow>, Arc<Carrier>, Target), SessionOutcome> {
    let (rt, registry) = engine.snapshot();
    let policy = match engine.choose_policy(&rt, &registry, handle).await {
        Chosen::Policy(p) => p,
        Chosen::DnsFailed => return Err(failed(handle, "dns lookup failed")),
    };
    let ctx = SelectCtx {
        host: Some(to.host.to_string()),
    };
    let resolution = match resolve_ready(&registry, &policy, &ctx).await {
        Ok(resolution) => resolution,
        Err(chain) => {
            handle.set_policy_chain(chain);
            return Err(failed(handle, EVALUATION_FAILED));
        }
    };
    handle.set_policy_chain(resolution.chain.clone());
    if let Some(note) = &resolution.note {
        handle.set_error(note.to_string());
    }
    if resolution.terminal == TerminalKind::Reject {
        return Err(SessionOutcome::Rejected(reject_kind(&resolution.chain)));
    }
    let outbound = resolution.outbound.clone();
    if outbound.udp() == UdpSupport::Unsupported {
        handle.set_error("policy does not support UDP");
        return Err(SessionOutcome::Rejected(RejectKind::Reject));
    }
    let carrier = association
        .carriers
        .get(&outbound, &association.client)
        .await
        .map_err(|e| failed(handle, e.to_string()))?;
    let send_to = carrier
        .socket
        .resolve(to)
        .await
        .map_err(|e| failed(handle, e.to_string()))?;
    handle.mark_connected();
    let flow = Arc::new(Flow::new(handle.clone(), to.port));
    carrier.routes.add(send_to.clone(), &flow);
    Ok((flow, carrier, send_to))
}

/// The REJECT flavour at the end of `chain`.
fn reject_kind(chain: &[String]) -> RejectKind {
    chain
        .last()
        .and_then(|name| Builtin::parse(name))
        .and_then(RejectKind::from_builtin)
        .unwrap_or(RejectKind::Reject)
}

/// Sends the flow's datagrams until it is idle or the association ends.
async fn forward(
    association: &Association,
    flow: &Flow,
    carrier: &Carrier,
    send_to: &Target,
    first: Vec<u8>,
    waiting: &mut mpsc::Receiver<Vec<u8>>,
) -> SessionOutcome {
    let mut next = Some(first);
    loop {
        if let Some(datagram) = next.take() {
            if let Err(e) = carrier.socket.send_to(&datagram, send_to).await {
                return failed(&flow.handle, e.to_string());
            }
            flow.handle.add_up(datagram.len() as u64);
            flow.touch();
        }
        let wake = flow.deadline();
        tokio::select! {
            _ = association.ended.cancelled() => return SessionOutcome::Completed,
            got = waiting.recv() => match got {
                Some(datagram) => next = Some(datagram),
                None => return SessionOutcome::Completed,
            },
            _ = tokio::time::sleep_until(wake.into()) => {
                if Instant::now() >= flow.deadline() {
                    return SessionOutcome::Completed;
                }
            }
        }
    }
}

/// Drops what comes for a flow that could not start, until it is idle.
async fn drain(association: &Association, waiting: &mut mpsc::Receiver<Vec<u8>>) {
    let mut last = Instant::now();
    loop {
        tokio::select! {
            _ = association.ended.cancelled() => return,
            got = waiting.recv() => match got {
                Some(_) => last = Instant::now(),
                None => return,
            },
            _ = tokio::time::sleep_until((last + FLOW_IDLE).into()) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flow lives `FLOW_IDLE` past its last datagram; a DNS flow no longer
    /// than `DNS_LINGER` past its first answer.
    #[test]
    fn when_a_flow_is_reclaimed() {
        let t0 = Instant::now();
        let later = t0 + Duration::from_secs(5);
        assert_eq!(deadline(later, None, 443), later + FLOW_IDLE);
        assert_eq!(deadline(later, Some(t0), 443), later + FLOW_IDLE);
        assert_eq!(deadline(later, None, 53), later + FLOW_IDLE);
        assert_eq!(deadline(later, Some(t0), 53), t0 + DNS_LINGER);
        // a long-quiet DNS flow goes at its idle time, if that comes first
        let answered = t0 + Duration::from_secs(100);
        assert_eq!(deadline(t0, Some(answered), 53), t0 + FLOW_IDLE);
    }

    /// Answers count for the flow that wrote to their source, or else the
    /// oldest flow still there.
    #[test]
    fn answers_find_their_flow() {
        let routes = Routes::default();
        let handle = |port| {
            SessionHandle::new(
                u64::from(port),
                SessionInfo::udp(rurge_config::HostName::parse("10.0.0.1"), port),
            )
        };
        let target = |port| Target::new(rurge_config::HostName::parse("10.0.0.1"), port);
        let a = Arc::new(Flow::new(handle(1), 1));
        let b = Arc::new(Flow::new(handle(2), 2));
        routes.add(target(1), &a);
        routes.add(target(2), &b);
        assert!(Arc::ptr_eq(&routes.flow_for(&target(2)).unwrap(), &b));
        assert!(Arc::ptr_eq(&routes.flow_for(&target(9)).unwrap(), &a));
        routes.remove(&a);
        assert!(Arc::ptr_eq(&routes.flow_for(&target(9)).unwrap(), &b));
        drop(b);
        assert!(routes.flow_for(&target(2)).is_none());
    }
}
```

`crates/rurge-engine/src/lib.rs`——把

```rust
mod subscriptions;
```

换成

```rust
mod subscriptions;
mod udp;
```

`crates/rurge-engine/src/engine.rs`——把

```rust
    SessionHandle, SessionOutcome, Socks5Listener,
```

换成

```rust
    SessionHandle, SessionOutcome, Socks5Listener, UdpAdmission, UdpClient,
```

`crates/rurge-engine/src/engine.rs`——把

```rust
    generation: std::sync::Mutex<()>,
```

换成

```rust
    generation: std::sync::Mutex<()>,
    /// The engine itself, for the tasks a UDP association spawns.
    me: std::sync::Weak<Engine>,
    /// UDP associations being served (at most `udp::ASSOCIATIONS`).
    udp_associations: std::sync::atomic::AtomicUsize,
    /// When a UDP limit was last warned about.
    udp_warned: std::sync::Mutex<Option<Instant>>,
```

`crates/rurge-engine/src/engine.rs`——把

```rust
        let engine = Arc::new(Engine {
```

换成

```rust
        let engine = Arc::new_cyclic(|me| Engine {
```

`crates/rurge-engine/src/engine.rs`——把

```rust
            generation: std::sync::Mutex::new(()),
```

换成

```rust
            generation: std::sync::Mutex::new(()),
            me: me.clone(),
            udp_associations: std::sync::atomic::AtomicUsize::new(0),
            udp_warned: std::sync::Mutex::new(None),
```

`crates/rurge-engine/src/engine.rs`——把

```rust
    fn snapshot(&self) -> (Arc<Runtime>, Arc<PolicyRegistry>) {
```

换成

```rust
    pub(crate) fn snapshot(&self) -> (Arc<Runtime>, Arc<PolicyRegistry>) {
```

`crates/rurge-engine/src/engine.rs`——把

```rust
        self.bind_listeners().await
```

换成

```rust
        self.bind_listeners().await
    }

    /// A token every session's cancellation reaches (`cancel_sessions`).
    pub(crate) fn session_token(&self) -> CancellationToken {
        self.sessions_root.child_token()
    }

    /// Says, at most once a minute, that a UDP limit dropped something
    /// (M5-D11).
    pub(crate) fn warn_udp_limit(&self, what: &'static str) {
        let now = Instant::now();
        let mut last = self.udp_warned.lock().expect("udp warning");
        if last.is_none_or(|at| now.duration_since(at) >= Duration::from_secs(60)) {
            *last = Some(now);
            tracing::warn!("{what}; new flows are dropped");
        }
```

`crates/rurge-engine/src/engine.rs`——把

```rust
    /// Mode / rule → policy, shared by `dial` and `dial_internal` (M4 §4.1).
    async fn choose_policy(
```

换成

```rust
    /// Mode / rule → policy, shared by `dial`, `dial_internal` and the UDP
    /// flows (M4 §4.1).
    pub(crate) async fn choose_policy(
```

`crates/rurge-engine/src/engine.rs`——把

```rust
enum Chosen {
```

换成

```rust
pub(crate) enum Chosen {
```

`crates/rurge-engine/src/engine.rs`——把

```rust
        Box::pin(crate::relay::pump(client, upstream, handle, idle))
    }
}
```

换成

```rust
        Box::pin(crate::relay::pump(client, upstream, handle, idle))
    }

    fn admit_udp(&self) -> UdpAdmission {
        if self.udp_associations.load(Ordering::Relaxed) < crate::udp::ASSOCIATIONS {
            UdpAdmission::Accepted
        } else {
            self.warn_udp_limit("udp: too many associations");
            UdpAdmission::Busy
        }
    }

    fn associate<'a>(
        &'a self,
        client: Arc<dyn UdpClient>,
        session: SessionInfo,
        closed: CancellationToken,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let Some(engine) = self.me.upgrade() else {
                return;
            };
            /// One association counted while it is served.
            struct Counted<'e>(&'e std::sync::atomic::AtomicUsize);
            impl Drop for Counted<'_> {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, Ordering::Relaxed);
                }
            }
            let before = self.udp_associations.fetch_add(1, Ordering::Relaxed);
            let _counted = Counted(&self.udp_associations);
            if before >= crate::udp::ASSOCIATIONS {
                self.warn_udp_limit("udp: too many associations");
                return;
            }
            crate::udp::serve(engine, client, session, closed).await;
        })
    }
}
```

要点：
- 路由表（`Routes`）只在锁里做内存操作；拨号、发送都在锁外。流的任务结束时从表里摘掉自己，并把自己的会话按正常结束或失败记进请求记录。
- 关联的 `ended` 令牌取自引擎的会话令牌：控制连接断了或引擎退出时取消，全部流与载体随之结束。关联数的上限在 `admit_udp` 里判断（回 `Busy`），流数的上限在收到新目的地时判断（丢包并告警一次）。
- `a_closed_port_does_not_break_the_carrier` 与 `a_reject_rule_drops_the_datagrams` 里的短等待只用来观察"什么也没回来"。
- `an_association_has_at_most_1024_flows`：请求记录只保留 1000 条，所以用例等活动会话归零后断言"第 1025 个目的地没有出现"，而不是去数记录。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-engine --test udp` → 通过（`a_datagram_goes_direct_and_comes_back`、`a_name_is_looked_up_for_direct`、`anyone_may_answer_the_carrier`、`a_closed_port_does_not_break_the_carrier`、`a_reject_rule_drops_the_datagrams`、`a_policy_without_udp_rejects`、`closing_the_control_connection_ends_every_flow`、`an_association_has_at_most_1024_flows`）。
Run: `cargo test -p rurge-engine --lib udp` → 通过（`when_a_flow_is_reclaimed`、`answers_find_their_flow`）。
Run: `cargo test -p rurge-api` → 通过。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-engine crates/rurge-api crates/rurge-net/src/connector.rs
git commit -m "feat(engine): SOCKS5 的 UDP 关联——按目的地址的流、共用载体的全锥归属、60 / 10 秒回收与上限；请求记录的 transport"
```


### Task 5: QUIC 识别、`block-quic`、`PROTOCOL` 规则与 `udp-policy-not-supported-behaviour`

流的第一个包决定它的协议（P6、P7）：发往 UDP 443、至少 1200 字节、长头且类型是 Initial（QUIC v1 的类型 0、v2 的类型 1）的是 QUIC，其余是 UDP；`session.protocol` 在规则匹配之前设好，`PROTOCOL,QUIC` / `PROTOCOL,UDP` 因此能命中，`PROTOCOL,TCP` / `PROTOCOL,UDP` 按传输层判断（P8）。`block-quic`（D7、P9、P13）：全局 `[General] block-quic` 的四个取值——`always-allow` 放行、`all` 全部阻断、`all-proxy` 只阻断代理、`per-policy` 看链末那个策略自己的 `block-quic`（`on` 阻断、`off` 放行、`auto` 对代理阻断而对 DIRECT 放行）；被阻断的 QUIC 流按 REJECT 丢包，记录的说明是 `QUIC blocked`，浏览器因此退回 TCP。`block-quic` 的 `W0029` 退役。不支持 UDP 的策略按 `[General] udp-policy-not-supported-behaviour`（P10）：`REJECT`（默认）记 `policy does not support UDP`，`DIRECT` 改经 DIRECT 并记 `policy does not support UDP; sent through DIRECT`。

**Files:**
- Modify: `crates/rurge-engine/src/sniff.rs`（`is_quic_initial`，与用例）、`src/udp.rs`（协议、`quic_blocked`、`UdpFallback`，与用例）、`tests/udp.rs`
- Modify: `crates/rurge-rules/src/matcher.rs`（`PROTOCOL,TCP` / `UDP` 按传输层，与用例）
- Modify: `crates/rurge-config/src/spec/common.rs`（`block-quic` 不再 `W0029`，与用例）

**Interfaces:**
- Consumes: Task 4 的 `udp::{run_flow, open}`、`Engine::choose_policy`；既有的 `General::{block_quic, udp_policy_not_supported_behaviour}`（`BlockQuicGlobal`、`UdpFallback`）、`CommonOpts.block_quic`、`SessionInfo.protocol: Option<rurge_config::rule::ProtocolKind>`（`Quic` / `Udp`）、`Transport`。
- Produces:
  - `pub fn sniff::is_quic_initial(datagram: &[u8]) -> bool`（端口 443 由调用方判断）
  - `pub(crate) fn udp::quic_blocked(global: BlockQuicGlobal, policy: Tristate, direct: bool) -> bool`

- [ ] **Step 1: 先写用例**

`crates/rurge-config/src/spec/common.rs`——把

```rust
            [
                "dns-follow-interface",
                "tfo",
                "test-udp",
                "block-quic",
                "ecn"
            ]
```

换成

```rust
            ["dns-follow-interface", "tfo", "test-udp", "ecn"]
```

`crates/rurge-engine/src/udp.rs`——把

```rust
    use super::*;

    /// A flow lives `FLOW_IDLE` past its last datagram; a DNS flow no longer
```

换成

```rust
    use super::*;

    #[test]
    fn which_quic_flows_are_blocked() {
        use BlockQuicGlobal::*;
        use Tristate::*;
        // (global, policy's block-quic, terminal is DIRECT) → blocked
        for (global, policy, direct, blocked) in [
            (PerPolicy, Auto, false, true),
            (PerPolicy, Auto, true, false),
            (PerPolicy, On, true, true),
            (PerPolicy, Off, false, false),
            (AllProxy, Off, false, true),
            (AllProxy, On, true, false),
            (All, Off, true, true),
            (AlwaysAllow, On, false, false),
        ] {
            assert_eq!(
                quic_blocked(global, policy, direct),
                blocked,
                "{global:?} {policy:?} direct={direct}"
            );
        }
    }

    /// A flow lives `FLOW_IDLE` past its last datagram; a DNS flow no longer
```

`crates/rurge-rules/src/matcher.rs`——把

```rust
        assert_eq!(eval("PROTOCOL,HTTP", &s), Verdict::NoMatch);
```

换成

```rust
        assert_eq!(eval("PROTOCOL,HTTP", &s), Verdict::NoMatch);
        assert_eq!(eval("PROTOCOL,TCP", &s), Verdict::Match);
        assert_eq!(eval("PROTOCOL,UDP", &s), Verdict::NoMatch);
        let mut quic = SessionInfo::udp(HostName::parse("example.com"), 443);
        quic.protocol = Some(ProtocolKind::Quic);
        assert_eq!(eval("PROTOCOL,QUIC", &quic), Verdict::Match);
        assert_eq!(eval("PROTOCOL,UDP", &quic), Verdict::Match);
        assert_eq!(eval("PROTOCOL,TCP", &quic), Verdict::NoMatch);
```

`crates/rurge-engine/tests/udp.rs`——把

```rust
        .collect()
```

换成

```rust
        .collect()
}

/// Waits until `count` UDP flows have a policy: routed, or already
/// finished. Ending the association earlier would end the flows still
/// being routed.
async fn routed(h: &Harness, count: usize) {
    wait_until("the UDP flows to be routed", || {
        let log = h.engine.request_log();
        log.active()
            .into_iter()
            .chain(log.recent(4096))
            .filter(|r| r.transport == Transport::Udp && !r.policy.is_empty())
            .count()
            >= count
    })
    .await;
```

`crates/rurge-engine/tests/udp.rs`——把

```rust
        Some("policy does not support UDP")
    );
}

/// The association — every flow of it — ends with the control connection.
```

换成

```rust
        Some("policy does not support UDP")
    );
}

/// A QUIC Initial-looking datagram (padded to 1200 bytes).
fn quic_initial() -> Vec<u8> {
    let mut out = vec![0xc3, 0, 0, 0, 1];
    out.resize(1200, 0);
    out
}

/// `block-quic` (M5 design 6.2): QUIC to UDP 443 is dropped with a note —
/// the browser falls back to TCP — while other UDP to 443 goes through.
#[tokio::test]
async fn block_quic_drops_quic_and_nothing_else() {
    let h = harness(Profile {
        general: "block-quic = all",
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    association.send("quic.test", 443, &quic_initial()).await;
    association.send("alt.test", 443, b"not quic").await;
    routed(&h, 2).await;
    drop(association);
    let records = finished(&h, 2).await;
    let by_dst = |dst: &str| records.iter().find(|r| r.dst == dst).unwrap();
    let quic = by_dst("quic.test:443");
    assert_eq!(quic.status, RecordStatus::Rejected("REJECT".into()));
    assert_eq!(quic.error.as_deref(), Some("QUIC blocked"));
    assert_eq!(quic.protocol, Some(rurge_config::rule::ProtocolKind::Quic));
    let plain = by_dst("alt.test:443");
    assert_eq!(plain.status, RecordStatus::Completed, "{plain:?}");
    assert_eq!(plain.protocol, Some(rurge_config::rule::ProtocolKind::Udp));
}

/// `per-policy`: a proxy's `auto` blocks, its `off` lets QUIC through;
/// DIRECT's `auto` lets it through.
#[tokio::test]
async fn per_policy_block_quic_follows_the_terminal_policy() {
    let up = FakeSocks5::spawn(Socks5Script::default()).await;
    let proxies = format!(
        "Auto = socks5, 127.0.0.1, {0}, udp-relay=true
Off = socks5, 127.0.0.1, {0}, udp-relay=true, block-quic=off",
        up.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        rules: "DOMAIN,auto.test,Auto
DOMAIN,off.test,Off",
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    for host in ["auto.test", "off.test", "direct.test"] {
        association.send(host, 443, &quic_initial()).await;
    }
    routed(&h, 3).await;
    drop(association);
    let records = finished(&h, 3).await;
    let blocked = |dst: &str| {
        records
            .iter()
            .find(|r| r.dst == dst)
            .unwrap()
            .error
            .as_deref()
            == Some("QUIC blocked")
    };
    assert!(blocked("auto.test:443"));
    assert!(!blocked("off.test:443"));
    assert!(!blocked("direct.test:443"));
}

/// `PROTOCOL,UDP` matches every UDP flow; `PROTOCOL,QUIC` the QUIC ones.
#[tokio::test]
async fn protocol_rules_see_udp_and_quic() {
    let h = harness(Profile {
        rules: "PROTOCOL,QUIC,REJECT-DROP
PROTOCOL,UDP,REJECT-NO-DROP",
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    association.send("q.test", 443, &quic_initial()).await;
    association.send("u.test", 53, b"dns?").await;
    routed(&h, 2).await;
    drop(association);
    let records = finished(&h, 2).await;
    let status = |dst: &str| {
        records
            .iter()
            .find(|r| r.dst == dst)
            .unwrap()
            .status
            .clone()
    };
    assert_eq!(
        status("q.test:443"),
        RecordStatus::Rejected("REJECT-DROP".into())
    );
    assert_eq!(
        status("u.test:53"),
        RecordStatus::Rejected("REJECT-NO-DROP".into())
    );
}

/// `udp-policy-not-supported-behaviour = DIRECT`: a policy without UDP
/// sends through DIRECT instead, and the record says so.
#[tokio::test]
async fn a_policy_without_udp_may_fall_back_to_direct() {
    let h = harness(Profile {
        general: "udp-policy-not-supported-behaviour = DIRECT",
        proxies: "Web = http, 127.0.0.1, 9",
        rules: "DOMAIN,web.test,Web",
        ..Profile::default()
    })
    .await;
    let (echo, _) = udp_echo().await;
    h.dns.set("web.test", &["127.0.0.1"], &[], 60);
    let association = udp_associate(h.socks()).await;
    association
        .send("web.test", echo.port(), b"via direct")
        .await;
    assert_eq!(association.recv().await, (echo, b"via direct".to_vec()));
    drop(association);
    let records = finished(&h, 1).await;
    assert_eq!(records[0].policy, ["Web"]);
    assert_eq!(
        records[0].error.as_deref(),
        Some("policy does not support UDP; sent through DIRECT")
    );
}

/// The association — every flow of it — ends with the control connection.
```

- [ ] **Step 2: 运行，确认失败**

`udp.rs` 的单元用例用到的 `quic_blocked` 还不存在，`--lib` 编不过；端到端用例能编译，跑出四个失败：

Run: `cargo test -p rurge-engine --test udp`
Expected: FAIL——

```text
test block_quic_drops_quic_and_nothing_else ... FAILED
test per_policy_block_quic_follows_the_terminal_policy ... FAILED
test protocol_rules_see_udp_and_quic ... FAILED
test a_policy_without_udp_may_fall_back_to_direct ... FAILED
thread 'block_quic_drops_quic_and_nothing_else' panicked at crates\rurge-engine\tests\udp.rs:184:5:
assertion `left == right` failed
  left: Completed
 right: Rejected("REJECT")
thread 'per_policy_block_quic_follows_the_terminal_policy' panicked at crates\rurge-engine\tests\udp.rs:225:5:
assertion failed: blocked("auto.test:443")
thread 'protocol_rules_see_udp_and_quic' panicked at crates\rurge-engine\tests\udp.rs:253:5:
assertion `left == right` failed
  left: Completed
 right: Rejected("REJECT-DROP")
thread 'a_policy_without_udp_may_fall_back_to_direct' panicked at crates\rurge-engine\tests\common\mod.rs:277:18:
test result: FAILED. 8 passed; 4 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.06s
error: test failed, to rerun pass `-p rurge-engine --test udp`
exit 101
```

- [ ] **Step 3: 实现**

`crates/rurge-engine/src/sniff.rs`——把

```rust
//! malformed, truncated, or non-TLS input returns `None`.
```

换成

```rust
//! malformed, truncated, or non-TLS input returns `None`. And QUIC Initial
//! recognition for UDP flows (phase 2 M5 design 6.1).

/// QUIC v2's version number (RFC 9369), whose Initial packets carry type 1.
const QUIC_V2: u32 = 0x6b33_43cf;

/// Whether `datagram` is a QUIC Initial packet: a long header with the
/// fixed bit, a version other than 0 (version negotiation), the Initial
/// type — 0 in v1 and the drafts, 1 in v2 — and the 1200 bytes a client's
/// Initial is padded to (RFC 9000 §14.1, §17.2.2; RFC 9369 §3.2).
pub fn is_quic_initial(datagram: &[u8]) -> bool {
    let Some(&first) = datagram.first() else {
        return false;
    };
    let Some(version) = datagram.get(1..5) else {
        return false;
    };
    let version = u32::from_be_bytes([version[0], version[1], version[2], version[3]]);
    let kind = (first >> 4) & 0b11;
    datagram.len() >= 1200
        && first & 0xc0 == 0xc0
        && match version {
            0 => false,
            QUIC_V2 => kind == 0b01,
            _ => kind == 0b00,
        }
}
```

`crates/rurge-engine/src/sniff.rs`——把

```rust
    None
```

换成

```rust
    None
}

#[cfg(test)]
mod quic_tests {
    use super::*;

    fn initial(first: u8, version: u32, len: usize) -> Vec<u8> {
        let mut out = vec![first];
        out.extend_from_slice(&version.to_be_bytes());
        out.resize(len, 0);
        out
    }

    #[test]
    fn a_quic_initial_is_recognised() {
        assert!(is_quic_initial(&initial(0xc3, 1, 1200)), "v1");
        assert!(is_quic_initial(&initial(0xd3, QUIC_V2, 1250)), "v2");
        assert!(
            is_quic_initial(&initial(0xc0, 0xff00_001d, 1200)),
            "draft 29"
        );
        assert!(!is_quic_initial(&initial(0xc3, 1, 1199)), "unpadded");
        assert!(
            !is_quic_initial(&initial(0xe3, 1, 1200)),
            "a handshake packet"
        );
        assert!(
            !is_quic_initial(&initial(0xc3, QUIC_V2, 1200)),
            "v2 type 0 is 0-RTT"
        );
        assert!(
            !is_quic_initial(&initial(0xc3, 0, 1200)),
            "version negotiation"
        );
        assert!(!is_quic_initial(&initial(0x43, 1, 1200)), "a short header");
        assert!(!is_quic_initial(&[0xc3, 0, 0]));
        assert!(!is_quic_initial(&[]));
    }
```

`crates/rurge-engine/src/udp.rs`——把

```rust
use rurge_config::policy::Builtin;
use rurge_config::session::SessionInfo;
```

换成

```rust
use rurge_config::general::{BlockQuicGlobal, UdpFallback};
use rurge_config::policy::Builtin;
use rurge_config::rule::{PolicyRef, ProtocolKind};
use rurge_config::session::SessionInfo;
use rurge_config::spec::Tristate;
```

`crates/rurge-engine/src/udp.rs`——把

```rust
const DATAGRAM: usize = 65536;
```

换成

```rust
const DATAGRAM: usize = 65536;

/// Whether a QUIC flow is blocked (M5 design 6.2): the global setting
/// overrides; `per-policy` asks the terminal policy's own `block-quic`,
/// whose `auto` blocks proxies and lets DIRECT through.
pub(crate) fn quic_blocked(global: BlockQuicGlobal, policy: Tristate, direct: bool) -> bool {
    match global {
        BlockQuicGlobal::AlwaysAllow => false,
        BlockQuicGlobal::All => true,
        BlockQuicGlobal::AllProxy => !direct,
        BlockQuicGlobal::PerPolicy => match policy {
            Tristate::On => true,
            Tristate::Off => false,
            Tristate::Auto => !direct,
        },
    }
}
```

`crates/rurge-engine/src/udp.rs`——把

```rust
    session.dst_port = to.port;
```

换成

```rust
    session.dst_port = to.port;
    // the rules see what the first datagram is (`PROTOCOL,QUIC`)
    session.protocol = Some(if to.port == 443 && crate::sniff::is_quic_initial(&first) {
        ProtocolKind::Quic
    } else {
        ProtocolKind::Udp
    });
```

`crates/rurge-engine/src/udp.rs`——把

```rust
    let outbound = resolution.outbound.clone();
    if outbound.udp() == UdpSupport::Unsupported {
        handle.set_error("policy does not support UDP");
        return Err(SessionOutcome::Rejected(RejectKind::Reject));
```

换成

```rust
    let direct = resolution.terminal == TerminalKind::Direct;
    let setting = resolution
        .chain
        .last()
        .and_then(|name| registry.spec(name))
        .map(|spec| spec.common.block_quic)
        .unwrap_or_default();
    if handle.session().protocol == Some(ProtocolKind::Quic)
        && quic_blocked(rt.config.general.block_quic, setting, direct)
    {
        // the browser falls back to TCP
        handle.set_error("QUIC blocked");
        return Err(SessionOutcome::Rejected(RejectKind::Reject));
    }
    let mut outbound = resolution.outbound.clone();
    if outbound.udp() == UdpSupport::Unsupported {
        match rt.config.general.udp_policy_not_supported_behaviour {
            UdpFallback::Reject => {
                handle.set_error("policy does not support UDP");
                return Err(SessionOutcome::Rejected(RejectKind::Reject));
            }
            UdpFallback::Direct => {
                handle.set_error("policy does not support UDP; sent through DIRECT");
                outbound = registry
                    .resolve_with(&PolicyRef::Builtin(Builtin::Direct), &ctx)
                    .outbound;
            }
        }
```

`crates/rurge-rules/src/matcher.rs`——把

```rust
use rurge_config::session::{ProcessInfo, SessionInfo};
```

换成

```rust
use rurge_config::session::{ProcessInfo, SessionInfo, Transport};
```

`crates/rurge-rules/src/matcher.rs`——把

```rust
            Matcher::Protocol(k) => (s.protocol == Some(*k)).into(),
```

换成

```rust
            // `PROTOCOL,UDP` also matches QUIC and STUN (manual), and
            // `PROTOCOL,TCP` every TCP session: the transport decides
            Matcher::Protocol(k) => (s.protocol == Some(*k)
                || matches!(
                    (k, s.transport),
                    (ProtocolKind::Tcp, Transport::Tcp) | (ProtocolKind::Udp, Transport::Udp)
                ))
            .into(),
```

`crates/rurge-config/src/spec/common.rs`——把

```rust
    let mut ecn = r.choice("ecn", &TRISTATES).unwrap_or_default();
    let block_quic_present = r.has("block-quic");
```

换成

```rust
    let mut ecn = r.choice("ecn", &TRISTATES).unwrap_or_default();
```

`crates/rurge-config/src/spec/common.rs`——把

```rust
            ("test-udp", test_udp.is_some()),
            ("block-quic", block_quic_present),
```

换成

```rust
            ("test-udp", test_udp.is_some()),
```

要点：
- 识别只看第一个包：之后的包不重新判断，流的协议与规则、策略都不变。
- "是不是 DIRECT"看解析结果的终端（`TerminalKind::Direct`），所以"策略组选中 DIRECT"与"规则直接 DIRECT"一样放行；`per-policy` 读的是解析链上最后一个名字的 `block-quic`。
- 顺序：REJECT 先于 QUIC 阻断，QUIC 阻断先于"策略不支持 UDP"——被阻断的 QUIC 不会因为兜底而改走 DIRECT。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-engine --test udp` → 通过（新增 `block_quic_drops_quic_and_nothing_else`、`per_policy_block_quic_follows_the_terminal_policy`、`protocol_rules_see_udp_and_quic`、`a_policy_without_udp_may_fall_back_to_direct`）。
Run: `cargo test -p rurge-engine --lib` → 通过（新增 `a_quic_initial_is_recognised`、`which_quic_flows_are_blocked`）。
Run: `cargo test -p rurge-rules matcher` 与 `cargo test -p rurge-config` → 通过。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-engine crates/rurge-rules crates/rurge-config
git commit -m "feat(engine): QUIC Initial 识别、block-quic（全局与策略）、PROTOCOL 按传输层、udp-policy-not-supported-behaviour"
```


### Task 6: 经 `socks5` 上游、`underlying-proxy` 链与 `external` 的 UDP

把 Task 2 的出站接到链式与 `external` 上（P14、P16）。`ChainConnector::open_udp`：底层策略自己能开 UDP 载体时交给它，不能时报 `via <名字>: the underlying policy cannot carry UDP`（而不是连接器默认的那句）。`external` 的 `udp-relay=true` 时经本机程序的 SOCKS5 做 UDP ASSOCIATE（与 `socks5` 出站共用 `negotiate_bound` / `Socks5Udp`），`udp-relay` 对 `external` 的 `W0029` 退役；`tests/external` 的 `socks-helper` 学会 UDP ASSOCIATE。

**Files:**
- Modify: `crates/rurge-policy/src/cell.rs`（`ChainConnector::open_udp`）
- Modify: `crates/rurge-proto/src/external.rs`（`udp_relay`、`connect_local`、`associate`、`udp` / `open_udp`，与用例）、`src/socks5.rs`（`UDP_ASSOCIATE` 改 `pub(crate)`）
- Modify: `crates/rurge-config/src/spec/external.rs`（`udp_relay`）、`src/spec/mod.rs`（与用例）、`tests/snapshots/corpus__corpus__kitchen-sink.snap`
- Modify: `crates/rurge-engine/tests/udp.rs`
- Modify: `tests/external/src/bin/socks-helper.rs`、`tests/external/tests/common/mod.rs`、`tests/external/tests/outbound.rs`

**Interfaces:**
- Consumes: Task 1 的 `Connector::open_udp`；Task 2 的 `socks5::{negotiate_bound, relay_of, Socks5Udp, UDP_ASSOCIATE}`、`FakeSocks5` 的 UDP；Task 4 的 `common::{udp_associate, udp_echo}`。
- Produces:
  - `rurge_config::spec::ExternalSpec.udp_relay: bool`
  - `ExternalOutbound`：`udp_relay` 时 `UdpSupport::Native`
  - `ChainConnector` 实现 `Connector::open_udp`

- [ ] **Step 1: 先写用例**

`socks-helper` 是测试程序，与用例一起放进去：

`tests/external/src/bin/socks-helper.rs`——把

```rust
//! A SOCKS5 server as small as a test needs (no authentication, CONNECT
//! only) that rurge starts as an `external` policy's program. It listens on
```

换成

```rust
//! A SOCKS5 server as small as a test needs (no authentication, CONNECT and
//! UDP ASSOCIATE) that rurge starts as an `external` policy's program. It listens on
```

`tests/external/src/bin/socks-helper.rs`——把

```rust
use std::net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::process::{Command, Stdio};
```

换成

```rust
use std::net::{
    Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket,
};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
```

`tests/external/src/bin/socks-helper.rs`——把

```rust
    };
    let Ok(upstream) = TcpStream::connect(&host[..]) else {
```

换成

```rust
    };
    if request[1] == 3 {
        relay_udp(client)?;
        return Ok(true);
    }
    let Ok(upstream) = TcpStream::connect(&host[..]) else {
```

`tests/external/src/bin/socks-helper.rs`——把

```rust
    Ok(true)
}

fn port(stream: &mut TcpStream) -> std::io::Result<u16> {
```

换成

```rust
    Ok(true)
}

/// UDP ASSOCIATE: a relay port on 127.0.0.1; datagrams to IPv4 addresses
/// go out, answers from anywhere come back to the client, until the control
/// connection closes.
fn relay_udp(mut control: TcpStream) -> std::io::Result<()> {
    let relay = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    let outside = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    let mut reply = vec![5, 0, 0, 1, 127, 0, 0, 1];
    reply.extend_from_slice(&relay.local_addr()?.port().to_be_bytes());
    control.write_all(&reply)?;
    for socket in [&relay, &outside] {
        socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    }
    let client: Arc<Mutex<Option<SocketAddr>>> = Arc::default();
    let done = Arc::new(AtomicBool::new(false));
    let up = {
        let (relay, outside) = (relay.try_clone()?, outside.try_clone()?);
        let (client, done) = (client.clone(), done.clone());
        std::thread::spawn(move || {
            let mut buf = [0u8; 2048];
            while !done.load(Ordering::SeqCst) {
                // errors include Windows' ICMP "unreachable": keep going
                let Ok((n, from)) = relay.recv_from(&mut buf) else {
                    continue;
                };
                *client.lock().unwrap() = Some(from);
                if n >= 10 && buf[..4] == [0, 0, 0, 1] {
                    let to = SocketAddr::from((
                        Ipv4Addr::new(buf[4], buf[5], buf[6], buf[7]),
                        u16::from_be_bytes([buf[8], buf[9]]),
                    ));
                    let _ = outside.send_to(&buf[10..n], to);
                }
            }
        })
    };
    let down = {
        let done = done.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 2048];
            while !done.load(Ordering::SeqCst) {
                let Ok((n, from)) = outside.recv_from(&mut buf[10..]) else {
                    continue;
                };
                let (Some(to), SocketAddr::V4(from)) = (*client.lock().unwrap(), from) else {
                    continue;
                };
                buf[..4].copy_from_slice(&[0, 0, 0, 1]);
                buf[4..8].copy_from_slice(&from.ip().octets());
                buf[8..10].copy_from_slice(&from.port().to_be_bytes());
                let _ = relay.send_to(&buf[..10 + n], to);
            }
        })
    };
    let mut sink = [0u8; 64];
    while matches!(control.read(&mut sink), Ok(n) if n > 0) {}
    done.store(true, Ordering::SeqCst);
    let _ = up.join();
    let _ = down.join();
    Ok(())
}

fn port(stream: &mut TcpStream) -> std::io::Result<u16> {
```

`tests/external/tests/common/mod.rs`——把

```rust
        addresses: Vec::new(),
```

换成

```rust
        addresses: Vec::new(),
        udp_relay: true,
```

`tests/external/tests/outbound.rs`——把

```rust
    wait_closed(elsewhere).await;
}

/// Stopping ends the program and what it started (M4-D6): the helper's
```

换成

```rust
    wait_closed(elsewhere).await;
}

/// `udp-relay=true` (phase 2 M5): UDP goes through the program's own
/// SOCKS5 server, started for it when it does not run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn udp_goes_through_the_program() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let o = outbound("Ext", &args(port, &[]), port, dir.path());
    let echo = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 256];
        while let Ok((n, from)) = echo.recv_from(&mut buf).await {
            let _ = echo.send_to(&buf[..n], from).await;
        }
    });
    let carrier = o.open_udp(&ConnectOpts::default()).await.unwrap();
    carrier.send_to(b"ping", &target(echo_addr)).await.unwrap();
    let mut buf = [0u8; 256];
    let (n, from) = tokio::time::timeout(Duration::from_secs(5), carrier.recv_from(&mut buf))
        .await
        .expect("the echo comes back")
        .unwrap();
    assert_eq!((&buf[..n], from), (&b"ping"[..], target(echo_addr)));
    drop(carrier);
    o.stop().await;
    wait_closed(port).await;
}

/// Stopping ends the program and what it started (M4-D6): the helper's
```

`crates/rurge-proto/src/external.rs`——把

```rust
            addresses: Vec::new(),
```

换成

```rust
            addresses: Vec::new(),
            udp_relay: false,
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        assert_eq!(o.inert, ["ecn", "addresses", "udp-relay"]);
```

换成

```rust
        assert_eq!(o.inert, ["ecn", "addresses"]);
        assert!(external.udp_relay);
```

`crates/rurge-config/tests/snapshots/corpus__corpus__kitchen-sink.snap`——把

```text
  - "warning[W0029] valid/kitchen-sink.conf:71: policy parameter `addresses` is parsed but has no effect in this version"
  - "warning[W0029] valid/kitchen-sink.conf:71: policy parameter `udp-relay` is parsed but has no effect in this version"
```

换成

```text
  - "warning[W0029] valid/kitchen-sink.conf:71: policy parameter `addresses` is parsed but has no effect in this version"
```

`crates/rurge-engine/tests/udp.rs`——把

```rust
use rurge_config::session::Transport;
use rurge_engine::RequestRecord;
```

换成

```rust
use rurge_config::HostName;
use rurge_config::session::Transport;
use rurge_engine::RequestRecord;
use rurge_net::connector::Target;
```

`crates/rurge-engine/tests/udp.rs`——把

```rust
        Some("policy does not support UDP; sent through DIRECT")
    );
}

/// The association — every flow of it — ends with the control connection.
```

换成

```rust
        Some("policy does not support UDP; sent through DIRECT")
    );
}

/// `socks5` with `udp-relay=true` carries UDP through the proxy's own
/// association (M5 design §7).
#[tokio::test]
async fn udp_goes_through_a_socks5_proxy() {
    let up = FakeSocks5::spawn(Socks5Script::default()).await;
    let proxies = format!(
        "Up = socks5, 127.0.0.1, {}, udp-relay=true",
        up.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        rules: "IP-CIDR,127.0.0.1/32,Up",
        ..Profile::default()
    })
    .await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association
        .send("127.0.0.1", echo.port(), b"via socks5")
        .await;
    assert_eq!(association.recv().await, (echo, b"via socks5".to_vec()));
    assert_eq!(
        up.datagrams(),
        [Target::new(HostName::Ip(echo.ip()), echo.port())]
    );
}

/// `underlying-proxy` carries UDP (M5 design 4.3): the front proxy's
/// association is set up through the back one, and its datagrams go
/// through the back one's association.
#[tokio::test]
async fn udp_goes_through_a_chain() {
    let (back, front) = (
        FakeSocks5::spawn(Socks5Script::default()).await,
        FakeSocks5::spawn(Socks5Script::default()).await,
    );
    let proxies = format!(
        "Back = socks5, 127.0.0.1, {}, udp-relay=true\nFront = socks5, 127.0.0.1, {}, udp-relay=true, underlying-proxy=Back",
        back.addr().port(),
        front.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        rules: "IP-CIDR,127.0.0.1/32,Front",
        ..Profile::default()
    })
    .await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association
        .send("127.0.0.1", echo.port(), b"two hops")
        .await;
    assert_eq!(association.recv().await, (echo, b"two hops".to_vec()));
    let commands: Vec<u8> = back.requests().iter().map(|r| r.command).collect();
    assert_eq!(
        commands,
        [1, 3],
        "the front's control connection, then the back's association"
    );
    assert_eq!(
        front.datagrams(),
        [Target::new(HostName::Ip(echo.ip()), echo.port())]
    );
    assert_eq!(
        back.datagrams().len(),
        1,
        "the front's datagram, to the front's relay"
    );
}

/// A chain whose underlying policy carries no UDP fails the flow and says
/// why.
#[tokio::test]
async fn a_chain_without_udp_fails_the_flow() {
    let front = FakeSocks5::spawn(Socks5Script::default()).await;
    let back = FakeHttpProxy::spawn(HttpProxyScript {
        connect_to: Some(front.addr()),
        ..HttpProxyScript::default()
    })
    .await;
    let proxies = format!(
        "Back = http, 127.0.0.1, {}\nFront = socks5, 127.0.0.1, {}, udp-relay=true, underlying-proxy=Back",
        back.addr().port(),
        front.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        rules: "IP-CIDR,127.0.0.1/32,Front",
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", 9, b"x").await;
    let records = finished(&h, 1).await;
    assert_eq!(records[0].status, RecordStatus::Failed);
    assert_eq!(
        records[0].error.as_deref(),
        Some("via Back: the underlying policy cannot carry UDP")
    );
}

/// The association — every flow of it — ends with the control connection.
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-engine --test udp chain`
Expected: FAIL——链式的中继一段还开不出 UDP 载体，错误文本是连接器的默认那句：

```text
test a_chain_without_udp_fails_the_flow ... FAILED
test udp_goes_through_a_chain ... FAILED
thread 'a_chain_without_udp_fails_the_flow' panicked at crates\rurge-engine\tests\udp.rs:387:5:
assertion `left == right` failed
  left: Some("this connection cannot carry UDP")
 right: Some("via Back: the underlying policy cannot carry UDP")
thread 'udp_goes_through_a_chain' panicked at crates\rurge-engine\tests\common\mod.rs:277:18:
test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 13 filtered out; finished in 5.03s
error: test failed, to rerun pass `-p rurge-engine --test udp`
exit 101
```

（`udp_goes_through_a_socks5_proxy` 这时已能通过：没有 `underlying-proxy` 的 `socks5` 出站在 Task 2 就能承载 UDP。`tests/external` 与 `rurge-proto` 的 `external` 用例这时编不过：`ExternalSpec` 还没有 `udp_relay`。）

- [ ] **Step 3: 实现**

`crates/rurge-policy/src/cell.rs`——把

```rust
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_proto::OutboundError;
```

换成

```rust
use rurge_net::connector::{BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, Target};
use rurge_proto::{OutboundError, UdpSupport};
```

`crates/rurge-policy/src/cell.rs`——把

```rust
                .connect_tcp(target, opts)
```

换成

```rust
                .connect_tcp(target, opts)
                .await
                .map_err(|e| self.via(e))
        })
    }

    /// The underlying policy's own UDP carrier (phase 2 M5 design 4.3): what
    /// this hop sends to its server goes through it.
    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedPacketSocket>> {
        Box::pin(async move {
            let Some(registry) = self.cell.load() else {
                return Err(io::Error::other(format!(
                    "via {}: no policy registry is active",
                    self.name
                )));
            };
            if !registry.contains(&self.name) {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("via {}: the policy no longer exists", self.name),
                ));
            }
            let resolution = registry.resolve_relay(&self.name);
            if let (TerminalKind::Reject, Some(note)) = (resolution.terminal, &resolution.note) {
                return Err(io::Error::other(format!("via {}: {note}", self.name)));
            }
            if resolution.outbound.udp() == UdpSupport::Unsupported {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("via {}: the underlying policy cannot carry UDP", self.name),
                ));
            }
            resolution
                .outbound
                .open_udp(opts)
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
const UDP_ASSOCIATE: [u8; 10] = [VERSION, 3, 0, 1, 0, 0, 0, 0, 0, 0];
```

换成

```rust
pub(crate) const UDP_ASSOCIATE: [u8; 10] = [VERSION, 3, 0, 1, 0, 0, 0, 0, 0, 0];
```

`crates/rurge-config/src/spec/external.rs`——把

```rust
    pub addresses: Vec<IpAddr>,
```

换成

```rust
    pub addresses: Vec<IpAddr>,
    /// `udp-relay`: the program's SOCKS5 server takes `UDP ASSOCIATE`
    /// (the manual: it must, for this to be switched on).
    pub udp_relay: bool,
```

`crates/rurge-config/src/spec/external.rs`——把

```rust
        addresses,
```

换成

```rust
        addresses,
        udp_relay: r.bool("udp-relay").unwrap_or(false),
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
                notes.inert.push("addresses");
            }
            if r.bool("udp-relay").unwrap_or(false) {
                notes.inert.push("udp-relay");
            }
```

换成

```rust
                notes.inert.push("addresses");
            }
```

`crates/rurge-proto/src/external.rs`——把

```rust
use crate::socks5::{connect_request, negotiate};
use crate::{Outbound, OutboundError};
use rurge_config::spec::ExternalSpec;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Target};
```

换成

```rust
use crate::socks5::{
    Socks5Udp, UDP_ASSOCIATE, connect_request, negotiate, negotiate_bound, relay_of,
};
use crate::{Outbound, OutboundError, UdpSupport};
use rurge_config::HostName;
use rurge_config::spec::ExternalSpec;
use rurge_net::BoxFuture;
use rurge_net::connector::{
    BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, DirectConnector, SystemResolve, Target,
};
```

`crates/rurge-proto/src/external.rs`——把

```rust
    port: u16,
    log: PathBuf,
```

换成

```rust
    port: u16,
    udp_relay: bool,
    log: PathBuf,
```

`crates/rurge-proto/src/external.rs`——把

```rust
            port: spec.local_port,
            log: log_dir.join(log_file_name(name)),
```

换成

```rust
            port: spec.local_port,
            udp_relay: spec.udp_relay,
            log: log_dir.join(log_file_name(name)),
```

`crates/rurge-proto/src/external.rs`——把

```rust
        let request = connect_request(target)?;
```

换成

```rust
        let request = connect_request(target)?;
        let stream = self.connect_local().await?;
        negotiate(stream, &request, None).await
    }

    /// A UDP association with the program's SOCKS5 server; its datagrams
    /// go to the relay it names on this machine.
    async fn associate(&self) -> Result<BoxedPacketSocket, OutboundError> {
        let control = self.connect_local().await?;
        let (control, relay) = negotiate_bound(control, &UDP_ASSOCIATE, None).await?;
        let relay = relay_of(relay, &HostName::Ip(Ipv4Addr::LOCALHOST.into()));
        let socket = DirectConnector::new(Arc::new(SystemResolve))
            .open_udp(&ConnectOpts::default())
            .await?;
        Ok(Box::new(Socks5Udp::new(control, relay, socket)))
    }

    /// A connection to the program's SOCKS5 port, starting the program
    /// when it does not run.
    async fn connect_local(&self) -> Result<BoxedStream, OutboundError> {
```

`crates/rurge-proto/src/external.rs`——把

```rust
                    return negotiate(Box::new(stream), &request, None).await;
```

换成

```rust
                    return Ok(Box::new(stream));
```

`crates/rurge-proto/src/external.rs`——把

```rust
            match tokio::time::timeout(opts.timeout, self.dial(target)).await {
```

换成

```rust
            match tokio::time::timeout(opts.timeout, self.dial(target)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }

    fn udp(&self) -> UdpSupport {
        if self.udp_relay {
            UdpSupport::Native
        } else {
            UdpSupport::Unsupported
        }
    }

    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedPacketSocket, OutboundError>> {
        Box::pin(async move {
            if !self.udp_relay {
                return Err(OutboundError::Unsupported(
                    "UDP without `udp-relay=true`".to_string(),
                ));
            }
            match tokio::time::timeout(opts.timeout, self.associate()).await {
```

要点：
- `external` 的 UDP 与 TCP 一样先确保程序已经拉起（`connect_local` 负责拉起与本机端口的重试，是从原来的 `dial` 里拆出来的）；控制连接断了，载体随之失效。
- 本机程序的中继地址是未指定地址时用 `127.0.0.1`（与 `socks5` 出站的"未指定就是服务器本身"同一条规则，`relay_of`）；发往中继的 socket 是本机的 DIRECT 载体（`external` 没有 `underlying-proxy`）。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-engine --test udp` → 通过（新增 `udp_goes_through_a_socks5_proxy`、`udp_goes_through_a_chain`、`a_chain_without_udp_fails_the_flow`）。
Run: `cargo test -p rurge-external-tests` → 通过（新增 `udp_goes_through_the_program`）。
Run: `cargo test -p rurge-proto external` 与 `cargo test -p rurge-config` → 通过。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-policy crates/rurge-proto crates/rurge-config crates/rurge-engine/tests/udp.rs tests/external
git commit -m "feat(policy): 经 underlying-proxy 链的 UDP；external 的 udp-relay（经本机程序的 UDP ASSOCIATE）"
```


### Task 7: 对 sing-box 的 UDP 互操作；文档

`socks5` 出站的 `udp-relay` 对 sing-box `socks` 入站往返一个 UDP 回显（P5）。本机没有 sing-box：用例按既有的约定跳过，只在装了 sing-box 的 CI 上真正跑（`RURGE_TEST_SING_BOX`，`RURGE_INTEROP_REQUIRED=1` 时没装即失败）。文档：兼容性清单登记 M5a 生效的各项与设计第 12 节的差异、`docs/api/phase1.md` 的 `transport` 一列、手工验收的 M5a 一节、两份 README 与 `CLAUDE.md`。

**Files:**
- Modify: `tests/interop/tests/sing_box.rs`、`tests/interop/README.md`
- Modify: `docs/surge-compatibility-matrix.md`、`docs/api/phase1.md`、`docs/acceptance/phase2-manual.md`、`README.md`、`README_en.md`、`CLAUDE.md`

**Interfaces:**
- Consumes: Task 2 的 `Outbound::open_udp`、`PacketSocket::{send_to, recv_from}`；`tests/interop` 既有的 `sing_box_or_skip`、`SingBox::spawn`、`plain(InboundKind::Socks, ..)`、`outbound`、`target`。
- Produces: 无（用例与文档）。

- [ ] **Step 1: 写互操作用例**

`tests/interop/tests/sing_box.rs`——把

```rust
    assert!(matches!(&refused, OutboundError::Proxy(_)), "{refused}");
}

/// What "skipped" means must itself be tested: without a binary the helper
```

换成

```rust
    assert!(matches!(&refused, OutboundError::Proxy(_)), "{refused}");
}

/// UDP through sing-box's SOCKS5 inbound (phase 2 M5): rurge's
/// `udp-relay=true` association, to a loopback UDP echo.
#[tokio::test]
async fn socks5_udp_goes_through_sing_box() {
    let Some(bin) = sing_box_or_skip("socks5_udp_goes_through_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sb = SingBox::spawn(&bin, dir.path(), vec![plain(InboundKind::Socks, &[])]);
    let echo = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 1500];
        while let Ok((n, from)) = echo.recv_from(&mut buf).await {
            let _ = echo.send_to(&buf[..n], from).await;
        }
    });
    let profile = format!(
        "[Proxy]\nUdp = socks5, 127.0.0.1, {}, udp-relay=true\n[Rule]\nFINAL,DIRECT\n",
        sb.port(0)
    );
    let out = outbound(&profile, "Udp", None);
    let bound = std::time::Duration::from_secs(10);
    let carrier = tokio::time::timeout(bound, out.open_udp(&ConnectOpts::default()))
        .await
        .expect("the association is set up within the bound")
        .expect("the association is set up");
    carrier
        .send_to(b"interop", &target(echo_addr))
        .await
        .unwrap();
    let mut buf = [0u8; 64];
    let (n, from) = tokio::time::timeout(bound, carrier.recv_from(&mut buf))
        .await
        .expect("the echo comes back within the bound")
        .unwrap();
    assert_eq!((&buf[..n], from), (&b"interop"[..], target(echo_addr)));
}

/// What "skipped" means must itself be tested: without a binary the helper
```

`tests/interop/README.md`——把

```markdown
`tests/sing_box.rs` 驱动的四个用例（另一个是夹具自检）覆盖：
```

换成

```markdown
`tests/sing_box.rs` 驱动的五个用例（另一个是夹具自检）覆盖：
```

`tests/interop/README.md`——把

```markdown
- `https`：私有 CA 校验、`sni=` 覆盖、`server-cert-fingerprint-sha256` 指纹钉定（含钉错的情形）、`client-cert=`（p12 客户端证书）双向 TLS，以及服务端要求客户端证书但客户端未提供的情形。
```

换成

```markdown
- `https`：私有 CA 校验、`sni=` 覆盖、`server-cert-fingerprint-sha256` 指纹钉定（含钉错的情形）、`client-cert=`（p12 客户端证书）双向 TLS，以及服务端要求客户端证书但客户端未提供的情形。
- `socks5` 的 UDP（阶段 2 / M5a）：`udp-relay=true` 时经 sing-box `socks` 入站的 UDP ASSOCIATE 往返一个回环 UDP 回显。
```

- [ ] **Step 2: 运行**

Run: `cargo test -p rurge-interop socks5_udp`
Expected: 本机没有 sing-box 时通过并在输出里说明跳过（`skipping socks5_udp_goes_through_sing_box: ...`）——本任务没有能在本机跑出来的失败。装了 sing-box 的环境（`RURGE_TEST_SING_BOX=<路径>`）上它真正经 sing-box 往返；在 Task 2 之前的代码上它编不过（`open_udp` 不存在）。

- [ ] **Step 3: 提交用例**

```bash
git add tests/interop
git commit -m "test(interop): socks5 的 UDP ASSOCIATE 对 sing-box socks 入站"
```

- [ ] **Step 4: 文档**

兼容性清单（M5a 生效的各项；差异照设计第 12 节中 M5a 的部分登记）：

`docs/surge-compatibility-matrix.md`——把

```markdown
| `udp-policy-not-supported-behaviour` | `REJECT` `DIRECT`；默认 `REJECT`（Mac 6.0 起） | 全部 | ✅ | 2 | |
| `udp-priority` | 布尔；默认 true | 全部 | 🟡 | 3 | 高负载下优先处理 UDP，尽力而为 |
| `block-quic` | `per-policy` `all-proxy` `all` `always-allow`；默认 `per-policy` | 全部 | ✅ | 2 | |
```

换成

```markdown
| `udp-policy-not-supported-behaviour` | `REJECT` `DIRECT`；默认 `REJECT`（Mac 6.0 起） | 全部 | ✅ | 2 | M5a 已实现：终端出站不支持 UDP 时，`REJECT` 丢包并在请求记录写 `policy does not support UDP`，`DIRECT` 改经 DIRECT 发出、请求记录写 `policy does not support UDP; sent through DIRECT` |
| `udp-priority` | 布尔；默认 true | 全部 | 🟡 | 3 | 高负载下优先处理 UDP，尽力而为 |
| `block-quic` | `per-policy` `all-proxy` `all` `always-allow`；默认 `per-policy` | 全部 | 🟡 | 2 | M5a 已实现，对经 SOCKS5 UDP 进来的流生效（TUN 在阶段 3）。四个值的含义按名字理解、未与 Surge 核对：`per-policy` 用终端策略自己的 `block-quic`，`all-proxy` 凡经代理一律阻断，`all` 连 DIRECT 也阻断，`always-allow` 一律不阻断。只在目标 UDP 443 上按 QUIC Initial 包（长首部、版本非 0、Initial 类型、至少 1200 字节）识别；被阻断的流丢包，请求记录为 REJECT 与 `QUIC blocked`，不计入 REJECT 的自动升级 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `socks5-listen` | `address[:port]` 列表；默认端口 6153；不支持密码 | 🟡 | 1 | 全平台可用；REJECT 时回 `0x02`（手册未说明） |
```

换成

```markdown
| `socks5-listen` | `address[:port]` 列表；默认端口 6153；不支持密码 | 🟡 | 1 | 全平台可用；REJECT 时回 `0x02`（手册未说明）。UDP ASSOCIATE（阶段 2 / M5a）：在客户端连到的本机地址上开一个 UDP 端口；只收控制连接那个客户端 IP 的包（端口取请求里声明的，没声明时取第一个包的）；分片包（FRAG ≠ 0）丢弃；控制连接一断，关联的流全部结束；全锥（同一关联在同一出站上共用一个载体，任何来源的回包都送回客户端，`vmess` 除外）；一条流 = 关联 + 目标，各有一条请求记录（`transport` 为 `udp`），60 秒无收发回收，目标端口 53 的流收到回答后 10 秒回收（rurge 自定）；每个关联最多 1024 条流、全进程最多 4096 个关联，超出时新流丢弃、新关联回 0x01，每分钟至多告警一条；REJECT 系列对 UDP 一律丢包（SOCKS5 回不了 ICMP） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `PROTOCOL` | `HTTP` `HTTPS` `TCP` `UDP` `QUIC` `STUN` `MTProto` `DOH` `DOH3` `DOQ` `DOT` `DNS`；区分大小写；`TCP` 覆盖 HTTP/HTTPS/MTProto，`UDP` 覆盖 QUIC/STUN | 全部 | ✅ | 1 / 3 | `DOH*` `DOQ` `DOT` `DNS` 只匹配 rurge 自身发出的 DNS 请求且需 `encrypted-dns-follow-outbound-mode=true`；`MTProto` 依赖阶段 7；M3b：DoT/DoH/DNS 标签按上游端口启发（853/443/其余）；基于 SNI 的路由与 `PROTOCOL,HTTPS` 的 dial 前匹配随阶段 4 |
```

换成

```markdown
| `PROTOCOL` | `HTTP` `HTTPS` `TCP` `UDP` `QUIC` `STUN` `MTProto` `DOH` `DOH3` `DOQ` `DOT` `DNS`；区分大小写；`TCP` 覆盖 HTTP/HTTPS/MTProto，`UDP` 覆盖 QUIC/STUN | 全部 | ✅ | 1 / 3 | M5a：`UDP` 匹配每条 UDP 流、`TCP` 匹配每个 TCP 会话（按传输层判断）；UDP 流只识别 QUIC（目标 UDP 443 上的 Initial 包），STUN 与 DNS 的嗅探未做；`DOH*` `DOQ` `DOT` `DNS` 只匹配 rurge 自身发出的 DNS 请求且需 `encrypted-dns-follow-outbound-mode=true`；`MTProto` 依赖阶段 7；M3b：DoT/DoH/DNS 标签按上游端口启发（853/443/其余）；基于 SNI 的路由与 `PROTOCOL,HTTPS` 的 dial 前匹配随阶段 4 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| UDP 行为：`REJECT` / `REJECT-NO-DROP` 回 ICMP Administratively Prohibited（限频后丢包），`REJECT-DROP` 直接丢包 | UDP 无预匹配阶段 | 全部 | ✅ | 3 | |
```

换成

```markdown
| UDP 行为：`REJECT` / `REJECT-NO-DROP` 回 ICMP Administratively Prohibited（限频后丢包），`REJECT-DROP` 直接丢包 | UDP 无预匹配阶段 | 全部 | 🟡 | 2 / 3 | M5a：经 SOCKS5 进来的 UDP 流按规则匹配（没有预匹配）；四种 REJECT 一律丢包并记录（SOCKS5 回不了 ICMP）；TUN 上的 ICMP 在阶段 3 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `socks5` / `socks5-tls` | SOCKS5 / SOCKS5 over TLS | 全部 | ✅ | 2 | M1（阶段 2）已实现（TCP）；udp-relay 解析但未生效（M5）；目标主机名在写线之前转成 A-label，且只允许 ASCII 字母、数字、`-`、`.`、`_`（其它字符的名字被拒绝：防止宽松的上游把 `a@b.test` 读成 userinfo + 主机而绕过域名规则）；Surge 大概原样发送（未核对）；只有密码没有用户名时密码被忽略 |
```

换成

```markdown
| `socks5` / `socks5-tls` | SOCKS5 / SOCKS5 over TLS | 全部 | ✅ | 2 | M1（阶段 2）已实现（TCP）；M5a 起 `udp-relay=true` 时经 UDP ASSOCIATE 转发 UDP（每个客户端关联一条关联：控制连接加服务器给出的中继地址，中继地址是未指定地址时改用服务器地址；`socks5-tls` 的 UDP 是明文 UDP，只有控制连接是 TLS；控制连接断开即关联失效；有 `underlying-proxy` 时中继那一段经底层策略的 UDP 载体）；目标主机名在写线之前转成 A-label，且只允许 ASCII 字母、数字、`-`、`.`、`_`（其它字符的名字被拒绝：防止宽松的上游把 `a@b.test` 读成 userinfo + 主机而绕过域名规则）；Surge 大概原样发送（未核对）；只有密码没有用户名时密码被忽略 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `block-quic` | `auto` `on` `off`；默认 `auto`（代理策略默认阻断，DIRECT 不阻断） | ✅ | 2 | 与 `[General] block-quic` 全局覆盖联动；M1 解析并校验取值，`W0029`；M7 生效 |
```

换成

```markdown
| `block-quic` | `auto` `on` `off`；默认 `auto`（代理策略默认阻断，DIRECT 不阻断） | ✅ | 2 | 与 `[General] block-quic` 全局覆盖联动；M5a 生效（经 SOCKS5 进来的 UDP；组按解析出的终端策略判断） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `udp-relay`（布尔；默认 false） | 适用 SOCKS5 / SOCKS5-TLS / Shadowsocks / External / HTTP/2 CONNECT（RFC 9298） | ✅ | 2 |
| `udp-port`（端口；默认主端口） | 适用 Shadowsocks / Snell | ✅ | 2 |
| 自动支持 UDP 的协议：Snell v3+、VMess、Trojan、TUIC、Hysteria 2、MASQUE、AnyTLS（UDP over TCP）、WireGuard、Tailscale | | ✅ | 2 |
| 不支持 UDP 的协议：HTTP / HTTPS、Trust Tunnel、SSH | 受 `udp-policy-not-supported-behaviour` 控制 | ✅ | 2 |
```

换成

```markdown
| `udp-relay`（布尔；默认 false） | 适用 SOCKS5 / SOCKS5-TLS / Shadowsocks / External / HTTP/2 CONNECT（RFC 9298） | ✅ | 2 | M5a：`socks5` / `socks5-tls` / `external` 已生效；Shadowsocks 与 HTTP/2 CONNECT 随 M6 |
| `udp-port`（端口；默认主端口） | 适用 Shadowsocks / Snell | ✅ | 2 |
| 自动支持 UDP 的协议：Snell v3+、VMess、Trojan、TUIC、Hysteria 2、MASQUE、AnyTLS（UDP over TCP）、WireGuard、Tailscale | | ✅ | 2 |
| 不支持 UDP 的协议：HTTP / HTTPS、Trust Tunnel、SSH | 受 `udp-policy-not-supported-behaviour` 控制 | ✅ | 2 | M5a 已实现；`underlying-proxy` 的底层策略不支持 UDP 时，经它的 UDP 流失败并写 `via <底层策略>: the underlying policy cannot carry UDP` |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `external` | `exec` `local-port` `args`（可重复）`addresses`（可重复）`udp-relay` | 🟡 | 2 | M4c 已实现（TCP）。第一次用到时拉起（`exec` 加按原顺序的 `args`，标准输入为空），经 SOCKS5 连 `127.0.0.1:<local-port>`；连不上时每 500 ms 一次、一个请求最多 6 次（每次连接限时 500 ms：Windows 上要约 2 秒才拒绝连接），仍不行是 `external: the local SOCKS5 port refused the connection`；程序退出后下次用到时再拉起（照手册）。差异：三平台都支持（Surge 仅 Mac）；输出追加写入 `<数据目录>/external/<策略名>.log`（名字里文件名不能用的字符换成 `_` 并加一段哈希；在不区分大小写的文件系统上（Windows、macOS）只差大小写的两个策略名共用一个日志文件），每次拉起先写一行分隔，超过 1 MiB 时在拉起前轮转、只留一个旧文件；同一策略两次拉起至少间隔 2 秒，拉起失败时间隔内的请求直接得到同一个 `external: could not start <策略名> (<错误种类>)`；子进程环境去掉 `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY`（大小写两种）并设 `NO_PROXY=*`；程序自己退出时它启动的进程一并结束；rurge 正常退出时最后停掉全部外部程序连同它们启动的进程（Unix：整个进程组先 SIGTERM、2 秒后 SIGKILL；Windows：Job Object 关闭即结束全部，rurge 崩溃时同样不留下进程；Unix 上 rurge 崩溃后留下的进程不处理）；程序在自己的进程组里（Unix 进程组、Windows `CREATE_NEW_PROCESS_GROUP`），在 rurge 的终端按 Ctrl-C 不会直接送到它，由 rurge 按上面的顺序停掉；程序不能交互式提问：Unix 上它与 rurge 同一会话、不同进程组，读终端（如 `ssh` 的主机密钥确认、口令提示）会被 SIGTTIN 挂起，rurge 仍当它在运行——用 `ssh` 时写上 `-o BatchMode=yes -o ExitOnForwardFailure=yes`（后者让端口绑定失败时 `ssh` 直接退出，而不是不监听地一直运行）；重载时 `exec` / `args` / `local-port` 都没变的策略沿用原程序，变了的，旧程序在新配置发布、旧出站随之释放时停掉（经它的连接随之断开）；旧出站仍被占用时（进行中的拨号或测速、`smart` 组会话的回报），最晚在新程序第一次在同一 `local-port` 上拉起之前停掉（连同它启动的进程，等它结束，至多约 2 秒），此后经旧出站的拨号立即得到 `external: a newer configuration of this policy is in use`，旧出站不再拉起程序；`args` 在 `profiles/current`、`policies/detail` 与 `lineHash` 里整体脱敏，日志只写策略名、pid 与退出码；两个策略写同一个 `local-port` 是错误（`E0018`），`local-port` 与 rurge 自己的 `http-listen` / `socks5-listen`（地址为回环或全零）同端口也是错误（`E0018`：`` policy `X`: `local-port` <p> is the port of rurge's own `socks5-listen` ``，否则 rurge 会连回自己）；`interface` `allow-other-interface` `tfo` `tos` `ip-version` `underlying-proxy` 不适用（`W0028`），不能叠 Shadow TLS（`E0018`）；`addresses`（阶段 3）与 `udp-relay`（M5）解析但暂不生效（`W0029`），`addresses` 只收 IP 地址；"外部进程的流量走 DIRECT"到阶段 3（有 TUN 才有意义）；订阅导入的 `external` 一律跳过（`W0023`）；干构建、`rurge check` 与 `POST /v1/profiles/check` 从不拉起程序 |
```

换成

```markdown
| `external` | `exec` `local-port` `args`（可重复）`addresses`（可重复）`udp-relay` | 🟡 | 2 | M4c 已实现（TCP）。第一次用到时拉起（`exec` 加按原顺序的 `args`，标准输入为空），经 SOCKS5 连 `127.0.0.1:<local-port>`；连不上时每 500 ms 一次、一个请求最多 6 次（每次连接限时 500 ms：Windows 上要约 2 秒才拒绝连接），仍不行是 `external: the local SOCKS5 port refused the connection`；程序退出后下次用到时再拉起（照手册）。差异：三平台都支持（Surge 仅 Mac）；输出追加写入 `<数据目录>/external/<策略名>.log`（名字里文件名不能用的字符换成 `_` 并加一段哈希；在不区分大小写的文件系统上（Windows、macOS）只差大小写的两个策略名共用一个日志文件），每次拉起先写一行分隔，超过 1 MiB 时在拉起前轮转、只留一个旧文件；同一策略两次拉起至少间隔 2 秒，拉起失败时间隔内的请求直接得到同一个 `external: could not start <策略名> (<错误种类>)`；子进程环境去掉 `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY`（大小写两种）并设 `NO_PROXY=*`；程序自己退出时它启动的进程一并结束；rurge 正常退出时最后停掉全部外部程序连同它们启动的进程（Unix：整个进程组先 SIGTERM、2 秒后 SIGKILL；Windows：Job Object 关闭即结束全部，rurge 崩溃时同样不留下进程；Unix 上 rurge 崩溃后留下的进程不处理）；程序在自己的进程组里（Unix 进程组、Windows `CREATE_NEW_PROCESS_GROUP`），在 rurge 的终端按 Ctrl-C 不会直接送到它，由 rurge 按上面的顺序停掉；程序不能交互式提问：Unix 上它与 rurge 同一会话、不同进程组，读终端（如 `ssh` 的主机密钥确认、口令提示）会被 SIGTTIN 挂起，rurge 仍当它在运行——用 `ssh` 时写上 `-o BatchMode=yes -o ExitOnForwardFailure=yes`（后者让端口绑定失败时 `ssh` 直接退出，而不是不监听地一直运行）；重载时 `exec` / `args` / `local-port` 都没变的策略沿用原程序，变了的，旧程序在新配置发布、旧出站随之释放时停掉（经它的连接随之断开）；旧出站仍被占用时（进行中的拨号或测速、`smart` 组会话的回报），最晚在新程序第一次在同一 `local-port` 上拉起之前停掉（连同它启动的进程，等它结束，至多约 2 秒），此后经旧出站的拨号立即得到 `external: a newer configuration of this policy is in use`，旧出站不再拉起程序；`args` 在 `profiles/current`、`policies/detail` 与 `lineHash` 里整体脱敏，日志只写策略名、pid 与退出码；两个策略写同一个 `local-port` 是错误（`E0018`），`local-port` 与 rurge 自己的 `http-listen` / `socks5-listen`（地址为回环或全零）同端口也是错误（`E0018`：`` policy `X`: `local-port` <p> is the port of rurge's own `socks5-listen` ``，否则 rurge 会连回自己）；`interface` `allow-other-interface` `tfo` `tos` `ip-version` `underlying-proxy` 不适用（`W0028`），不能叠 Shadow TLS（`E0018`）；`addresses`（阶段 3）解析但暂不生效（`W0029`），只收 IP 地址；`udp-relay=true`（M5a）时 UDP 经程序自己的 SOCKS5 UDP ASSOCIATE 转发，程序没在运行时同样先拉起；"外部进程的流量走 DIRECT"到阶段 3（有 TUN 才有意义）；订阅导入的 `external` 一律跳过（`W0023`）；干构建、`rurge check` 与 `POST /v1/profiles/check` 从不拉起程序 |
```

API 文档的 `transport`：

`docs/api/phase1.md`——把

```markdown
{"id":12,"listener":"http","src":"127.0.0.1:51234","dst":"example.com:443","rule":"DOMAIN-SUFFIX,example.com,Proxy","policy":["Proxy","HK"],"sni":"example.com","protocol":"https","up":1234,"down":56789,"startedMs":1757200000000,"elapsedMs":812,"connectMs":35,"firstByteMs":120,"status":"completed","rejectKind":null,"error":null}
```

换成

```markdown
{"id":12,"listener":"http","transport":"tcp","src":"127.0.0.1:51234","dst":"example.com:443","rule":"DOMAIN-SUFFIX,example.com,Proxy","policy":["Proxy","HK"],"sni":"example.com","protocol":"https","up":1234,"down":56789,"startedMs":1757200000000,"elapsedMs":812,"connectMs":35,"firstByteMs":120,"status":"completed","rejectKind":null,"error":null}
```

`docs/api/phase1.md`——把

```markdown
`connectMs` 是会话开始到出站就绪的毫秒数（规则匹配、DNS、`evaluate-before-use` 的等待与 `smart` 组换成员的重试都在内），`firstByteMs` 是出站就绪到收到第一个上游字节的毫秒数；还没有对应时刻（被拒绝、拨号失败、还没收到数据）时为 `null`，测试会话（`rule` 为 `policy test`）两者恒为 `null`（阶段 2 / M3c 起）。
```

换成

```markdown
`transport` 是 `tcp` 或 `udp`（阶段 2 / M5a 起：经 SOCKS5 UDP ASSOCIATE 进来的每条 UDP 流各有一条记录，`dst` 是它的目标，`up` / `down` 是载荷字节数）。`connectMs` 是会话开始到出站就绪的毫秒数（规则匹配、DNS、`evaluate-before-use` 的等待与 `smart` 组换成员的重试都在内），`firstByteMs` 是出站就绪到收到第一个上游字节的毫秒数；还没有对应时刻（被拒绝、拨号失败、还没收到数据）时为 `null`，测试会话（`rule` 为 `policy test`）两者恒为 `null`（阶段 2 / M3c 起）。
```

手工验收的 M5a 一节（真实节点，项目所有者验收）：

`docs/acceptance/phase2-manual.md`——把

```markdown
- [ ] 订阅：把一行 `external` 放进自己的订阅文件，重载后该行被跳过，`rurge check` 报 `` `external` policies are not imported from subscriptions ``。

```

换成

```markdown
- [ ] UDP（M5a）：配置里写 `udp-relay=true`，让它的 `ssh -D` 换成一个支持 SOCKS5 UDP 的外部程序（`ssh -D` 不支持 UDP），经 rurge 的 SOCKS5 UDP 往返一次（见下面 M5a 一节的客户端）。
- [ ] 订阅：把一行 `external` 放进自己的订阅文件，重载后该行被跳过，`rurge check` 报 `` `external` policies are not imported from subscriptions ``。

## M5a　UDP 地基

前置：一个支持 SOCKS5 UDP 的客户端（如 Proxifier、SocksCap64，或设置了 SOCKS5 代理的游戏 / 语音软件、Telegram 桌面版的语音通话），指向 rurge 的 `socks5-listen`；一个开了 UDP 的上游 SOCKS5 节点（`udp-relay=true`）。

- [ ] DNS：客户端经 SOCKS5 UDP 发 DNS 查询（如 Proxifier 代理 `nslookup example.com 8.8.8.8`），得到回答；`GET /v1/requests/recent` 里有一条 `transport` 为 `udp`、`dst` 为 `8.8.8.8:53` 的记录，回答后约 10 秒结束。
- [ ] 游戏或语音：经 rurge 的 SOCKS5 进行一次语音通话或联机游戏，DIRECT 与经上游 SOCKS5 节点各一次，都能通话 / 联机；通话结束后约 60 秒，对应的记录都结束。
- [ ] 全锥：用 NAT 类型检测工具（STUN）经 rurge 的 SOCKS5 检测，结果是 Full Cone（经 DIRECT 与经 SOCKS5 节点；节点本身须是全锥）。
- [ ] `block-quic`：配置 `block-quic = all`，客户端发往 UDP 443 的 QUIC 被丢弃，请求记录为 REJECT 与 `QUIC blocked`，应用回落到 TCP 后照常可用；改为 `always-allow` 后 QUIC 照常经过。
- [ ] 不支持 UDP 的策略：规则把 UDP 分到一条 `http` 策略，默认 REJECT（记录写 `policy does not support UDP`）；配置 `udp-policy-not-supported-behaviour = DIRECT` 后改经 DIRECT。
- [ ] 关联结束：客户端断开（关闭软件），它的全部 UDP 记录随即结束。

```

两份 README 与 `CLAUDE.md`：

`README.md`——把

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；M4c（external）已完成——`external` 策略（TCP；第一次用到时拉起外部程序、经它在本机端口上的 SOCKS5 转发；程序退出后下次用到时再拉起；输出写进数据目录；rurge 退出时连同它启动的进程一起停掉——Unix 用进程组，Windows 用 Job Object；三平台都支持，Surge 只在 Mac 上有）已可用，订阅导入的 `external` 一律跳过；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

换成

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；M4c（external）已完成——`external` 策略（TCP；第一次用到时拉起外部程序、经它在本机端口上的 SOCKS5 转发；程序退出后下次用到时再拉起；输出写进数据目录；rurge 退出时连同它启动的进程一起停掉——Unix 用进程组，Windows 用 Job Object；三平台都支持，Surge 只在 Mac 上有）已可用，订阅导入的 `external` 一律跳过；M5a（UDP 地基）已完成——SOCKS5 监听支持 UDP ASSOCIATE，UDP 按规则分流到 DIRECT、REJECT 或 `socks5` / `socks5-tls` / `external`（`udp-relay=true`，含 `underlying-proxy` 链），全锥 NAT，每条 UDP 流一条请求记录，`block-quic` 与 `udp-policy-not-supported-behaviour` 生效（`trojan` / `vmess` / `anytls` / `wireguard` 的 UDP 在 M5b / M5c）；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

`README_en.md`——把

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; M4b (WireGuard) is done — the `wireguard` policy (TCP; a user-space tunnel, no virtual interface; several peers chosen by `allowed-ips`; `client-id` (WARP's reserved bytes); destination names looked up through the tunnel when the section has a `dns-server`, on this machine otherwise; tested by a handshake when there is neither a `dns-server` nor a `test-url`) is usable; M4c (external) is done — the `external` policy (TCP; the program is started on first use and reached as a SOCKS5 proxy on its local port; a program that exited is started again on the next use; its output goes to the data directory; when rurge exits, the program is stopped together with whatever it started — a process group on Unix, a Job Object on Windows; all three platforms, where Surge has it on the Mac only) is usable, and subscriptions never bring `external` policies in; the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

换成

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; M4b (WireGuard) is done — the `wireguard` policy (TCP; a user-space tunnel, no virtual interface; several peers chosen by `allowed-ips`; `client-id` (WARP's reserved bytes); destination names looked up through the tunnel when the section has a `dns-server`, on this machine otherwise; tested by a handshake when there is neither a `dns-server` nor a `test-url`) is usable; M4c (external) is done — the `external` policy (TCP; the program is started on first use and reached as a SOCKS5 proxy on its local port; a program that exited is started again on the next use; its output goes to the data directory; when rurge exits, the program is stopped together with whatever it started — a process group on Unix, a Job Object on Windows; all three platforms, where Surge has it on the Mac only) is usable, and subscriptions never bring `external` policies in; M5a (the UDP foundation) is done — the SOCKS5 listener takes UDP ASSOCIATE, and UDP is routed by rule to DIRECT, REJECT or `socks5` / `socks5-tls` / `external` (`udp-relay=true`, `underlying-proxy` chains included), full-cone NAT, one request record per UDP flow, and `block-quic` and `udp-policy-not-supported-behaviour` take effect (UDP over `trojan` / `vmess` / `anytls` / `wireguard` comes in M5b / M5c); the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

`CLAUDE.md`——把

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、发送时本机地址或路由失效或握手无回应时换新载体（网络变化后自愈）、网络变化入口、同一私钥与 peer 只留一条隧道，节与载体设置都相同的策略共用）、`stream`、`dns`（隧道内 DNS 与缓存；每个问题 2 秒内重发、先等隧道第一次握手）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。M4c（external）已完成——`rurge-config::spec::ExternalSpec`（`args` 是 `Secret`；两个 `external` 策略写了同一个 `local-port` 是 `E0018`；`args` 进内联参数的脱敏名单）、订阅导入的 `external` 行一律跳过（`W0023`）；`rurge-platform::process`（Unix 进程组与 `killpg`，Windows Job Object）；`rurge_proto::external`（`ExternalOutbound`：第一次用到时拉起、输出写 `<数据目录>/external/<策略名>.log` 且超过 1 MiB 时在拉起前轮转、子进程环境去掉代理变量并设 `NO_PROXY=*`、程序退出后下次用到时再拉起且两次拉起至少隔 2 秒、每 500 ms 连一次本机端口最多 6 次；`ProcessHook` / `ProcessGroup` 与 `NoProcessGroups`；程序自己退出时连同它启动的进程一起结束）；`rurge-engine` 的工厂分支、`EngineShared.processes` / `externals` 与 `Engine::stop_external_programs`；bin 的 `PlatformProcesses` 与退出流程最后停掉全部外部程序；只用于测试的工作区成员 `tests/external`（`rurge-external-tests`：极小的 SOCKS5 辅助程序 `socks-helper` 与真实拉起它的用例）；能力表翻转 `external`（`W0007` 不再因 `external` 出现）。M4 至此完成。
```

换成

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、发送时本机地址或路由失效或握手无回应时换新载体（网络变化后自愈）、网络变化入口、同一私钥与 peer 只留一条隧道，节与载体设置都相同的策略共用）、`stream`、`dns`（隧道内 DNS 与缓存；每个问题 2 秒内重发、先等隧道第一次握手）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。M4c（external）已完成——`rurge-config::spec::ExternalSpec`（`args` 是 `Secret`；两个 `external` 策略写了同一个 `local-port` 是 `E0018`；`args` 进内联参数的脱敏名单）、订阅导入的 `external` 行一律跳过（`W0023`）；`rurge-platform::process`（Unix 进程组与 `killpg`，Windows Job Object）；`rurge_proto::external`（`ExternalOutbound`：第一次用到时拉起、输出写 `<数据目录>/external/<策略名>.log` 且超过 1 MiB 时在拉起前轮转、子进程环境去掉代理变量并设 `NO_PROXY=*`、程序退出后下次用到时再拉起且两次拉起至少隔 2 秒、每 500 ms 连一次本机端口最多 6 次；`ProcessHook` / `ProcessGroup` 与 `NoProcessGroups`；程序自己退出时连同它启动的进程一起结束）；`rurge-engine` 的工厂分支、`EngineShared.processes` / `externals` 与 `Engine::stop_external_programs`；bin 的 `PlatformProcesses` 与退出流程最后停掉全部外部程序；只用于测试的工作区成员 `tests/external`（`rurge-external-tests`：极小的 SOCKS5 辅助程序 `socks-helper` 与真实拉起它的用例）；能力表翻转 `external`（`W0007` 不再因 `external` 出现）。M4 至此完成。M5（UDP 路径）按三份计划推进（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）：M5a 已完成——`rurge_net::connector::PacketSocket`（按包收发、带地址的 UDP 载体）与 `Connector::open_udp`（`DirectConnector`：每个地址族一个未连接的 socket，忽略 Windows 的 ICMP 不可达报错）；`Outbound::udp()` / `open_udp()` 与 `UdpSupport`；DIRECT 与 `socks5` / `socks5-tls` / `external` 的 UDP（`udp-relay`，`W0029` 退役）；`ChainConnector::open_udp`（链式 UDP 载体）；`rurge-inbound` 的 SOCKS5 UDP ASSOCIATE（`UdpClient`、`UdpAdmission`、`Dialer::admit_udp` / `associate`）；`rurge-engine` 的 UDP 流水线（`udp` 模块：一条流 = 关联 + 目标、按"关联 × 出站"共用载体的全锥、60 秒 / DNS 10 秒回收、1024 流 / 4096 关联的上限）、请求记录与 API 的 `transport`、`block-quic`（`W0029` 退役）与 `udp-policy-not-supported-behaviour`、QUIC Initial 识别、`PROTOCOL` 规则按传输层匹配 `TCP` / `UDP`；`FakeSocks5` 与 `tests/external` 辅助程序的 UDP ASSOCIATE；对 sing-box `socks` 入站的 UDP 互操作用例。
```

`CLAUDE.md`——把

```markdown
- `docs/superpowers/plans/2026-09-29-phase2-m4c-external-plan.md`：阶段 2 / M4c（external）实施计划（5 个任务）。开头「计划期决定」表记录核对 windows-sys / nix / tokio 源码与本仓库得出的结论和与设计文字不同的决定（每次连接本机端口限时 500 ms——Windows 要约 2 秒才拒绝连接、`ProtoSpec::External` 与重复端口检查随引擎任务落地、Shadow TLS 不能与 `external` 组合、程序自己退出时连同它启动的进程一起结束、测试辅助程序放在只用于测试的工作区成员 `tests/external` 里等）与「承接事项」；末尾「执行期修正记录」与「延后事项」两张表。
```

换成

```markdown
- `docs/superpowers/plans/2026-09-29-phase2-m4c-external-plan.md`：阶段 2 / M4c（external）实施计划（5 个任务）。开头「计划期决定」表记录核对 windows-sys / nix / tokio 源码与本仓库得出的结论和与设计文字不同的决定（每次连接本机端口限时 500 ms——Windows 要约 2 秒才拒绝连接、`ProtoSpec::External` 与重复端口检查随引擎任务落地、Shadow TLS 不能与 `external` 组合、程序自己退出时连同它启动的进程一起结束、测试辅助程序放在只用于测试的工作区成员 `tests/external` 里等）与「承接事项」；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/superpowers/specs/2026-09-29-phase2-m5-udp-design.md`：阶段 2 / M5 细化设计（UDP 路径），细化总设计的 M5 里程碑、不一致处以它为准。三份计划的拆分（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）；已决事项 M5-D1 ～ D11（三类用途都要、全锥 NAT、按包收发的 `PacketSocket`、VMess 先做对称型、`ecn` 延后、60 秒 / DNS 10 秒回收、QUIC 只在 UDP 443 上识别、全局 `block-quic` 按取值名理解、REJECT 对 UDP 一律丢包、1024 流 / 4096 关联的上限）；第 15 节 V1 ～ V10 是写各份计划时必须核对的事项，第 16 节是任务草图，第 17 节是 M5a 计划期的订正。
- `docs/superpowers/plans/2026-09-29-phase2-m5a-udp-foundation-plan.md`：阶段 2 / M5a（UDP 地基）实施计划（7 个任务）。开头「计划期决定」表记录核对源码与手册得出的结论和与设计文字不同的决定；末尾「执行期修正记录」与「延后事项」两张表。
```

`CLAUDE.md`——把

```markdown
cargo test -p rurge-proto-wireguard --release throughput -- --ignored --nocapture   # WireGuard 吞吐基准（经回环假对端双向回显 64 MiB；不作门禁）
```

换成

```markdown
cargo test -p rurge-proto-wireguard --release throughput -- --ignored --nocapture   # WireGuard 吞吐基准（经回环假对端双向回显 64 MiB；不作门禁）
cargo test -p rurge-engine --test udp          # UDP 流水线：SOCKS5 UDP ASSOCIATE → DIRECT / REJECT / socks5（含链）、全锥、QUIC 阻断、不支持 UDP 的策略、关联结束与流数上限
```

- [ ] **Step 5: 门禁与提交**

跑门禁。写本计划时副本上最后一次全工作区门禁：fmt / clippy 通过，`cargo test --workspace` 52 个测试二进制、1209 通过、0 失败、2 忽略。

```bash
git add docs README.md README_en.md CLAUDE.md
git commit -m "docs: M5a UDP 地基——兼容性清单、API 文档的 transport、手工验收、README 与 CLAUDE.md"
```

---

## 验收对照（设计第 11 节，M5a 部分）

| # | 验收项 | 由谁保证 |
| - | ------ | -------- |
| 1 | 经 rurge 的 SOCKS5 UDP，DIRECT 与 `socks5` / `external` 对回环假服务端往返；全锥用例通过；对 sing-box 的互操作在 CI 上通过 | Task 4：`a_datagram_goes_direct_and_comes_back`、`a_name_is_looked_up_for_direct`、`anyone_may_answer_the_carrier`；Task 6：`udp_goes_through_a_socks5_proxy`、`udp_goes_through_a_chain`、`udp_goes_through_the_program`；Task 2：`udp_goes_through_the_association`；Task 7：`socks5_udp_goes_through_sing_box`（CI）。`trojan` / `vmess` / `anytls` 在 M5b，`wireguard` 在 M5c |
| 2 | `block-quic` 按第 6 节阻断 QUIC；Chrome 回落到 TCP | Task 5：`block_quic_drops_quic_and_nothing_else`、`per_policy_block_quic_follows_the_terminal_policy`、`which_quic_flows_are_blocked`、`a_quic_initial_is_recognised`；Chrome 一项进手工验收（Task 7） |
| 3 | 不支持 UDP 的策略按 `udp-policy-not-supported-behaviour` 处理 | Task 4：`a_policy_without_udp_rejects`；Task 5：`a_policy_without_udp_may_fall_back_to_direct` |
| 4 | `W0029` 不再因 `udp-relay`、`block-quic` 出现（`test-udp`、`dns-follow-interface` 在 M5c） | Task 2（`socks5`）、Task 5（`block-quic`）、Task 6（`external`）的配置层用例与 kitchen-sink 快照 |
| 5 | 门禁全绿 | 各任务的门禁 |
| 6 | 需要真实环境的项目进手工验收清单 | Task 7：`docs/acceptance/phase2-manual.md` 的 M5a 一节 |
| — | 第 10 节第 1 层（QUIC 识别、归流、回收时限、`block-quic` 判定、SOCKS5 地址与 UDP 头） | Task 5：`a_quic_initial_is_recognised`、`which_quic_flows_are_blocked`；Task 4：`answers_find_their_flow`、`when_a_flow_is_reclaimed`；Task 2：`a_socks_address_reads_back_as_it_was_written`、`fragments_and_garbage_are_not_datagrams`；Task 3 `udp` 模块的用例 |
| — | 第 10 节第 2 层（端到端：入站关联、规则、REJECT 丢包、上限、控制连接结束） | Task 3：`udp_associate_carries_datagrams_both_ways`、`closing_the_control_connection_ends_the_association`、`udp_associate_can_be_refused`；Task 4：`a_reject_rule_drops_the_datagrams`、`closing_the_control_connection_ends_every_flow`、`an_association_has_at_most_1024_flows`；Task 5：`protocol_rules_see_udp_and_quic` |

## 执行期修正记录

| # | 任务 | 与计划的出入 | 原因 |
| - | ---- | ------------ | ---- |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | STUN 与 DNS 的嗅探（`PROTOCOL,STUN` 对 UDP 流不命中；DNS 流只按端口 53 定回收时限）（P6） | 需要时另议 |
| 2 | 全锥时"收到其它来源的包"的标注：请求记录只有 `error` 能写说明（P9） | 请求记录有说明字段时 |
| 3 | `ChainConnector::connect_udp`（把包载体包成到固定服务器的 `Datagram`）（P15） | M5c（`wireguard` 经 `underlying-proxy`） |
| 4 | UDP 流不计入 REJECT 的自动升级（P13） | 需要时另议 |
