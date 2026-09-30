# 阶段 2 / M5c「WireGuard 的 UDP 与其余」Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `wireguard` 出站承载 UDP（全锥），`wireguard` 能经 `underlying-proxy` 建隧道；`test-udp` / `proxy-test-udp`、`smart` 计入 UDP、`dns-follow-interface` 三项生效；M4b 延后事项 #15 收尾。M5（UDP 路径）至此完成。

**Architecture:** `rurge-proto-wireguard` 新增 `udp` 模块：`TunnelUdp` 在隧道的 smoltcp 协议栈里每个地址族开一个 UDP socket，按包收发，目标名与 TCP 一样解析（为此把名字解析从出站里抽成共享的 `Names`）。`rurge-net` 新增 `packet_datagram`，把按包收发的载体变成一条到固定目标的 `Datagram`，`ChainConnector::connect_udp` 用它把 WireGuard 的载体接到底层策略的 UDP 上。`rurge-policy` 新增 `udp_probe`（一次 A 查询），引擎的 `Engine::test_udp` 与 API 的 `udp` 键把它接到 `POST /v1/policies/test`。引擎的 UDP 流在 `smart` 组下向 `SmartBook` 回报。`dns-follow-interface` 经 `rurge_net::connector::Via` / `ResolveVia` 与 `Resolver::lookup_via`，让策略自己的解析经它的网卡问普通 DNS 服务器、答案另存。

**Tech Stack:** Rust 1.89 / edition 2024；不新增任何依赖（`smoltcp` 0.12 已在 `rurge-proto-wireguard` 的依赖里；UDP 测试的 DNS 报文手写，不引入 DNS 库）。

**Spec:** `docs/superpowers/specs/2026-09-29-phase2-m5-udp-design.md`（M5-D1 ～ D11；第 7 节 `wireguard` 一行；第 8 节；第 9 ～ 12 节中 M5c 的部分；第 15 节 V7 ～ V10；第 16 节 M5c 草图；第 17 ～ 20 节的订正）与总设计 `docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`。与本计划「计划期决定」表不一致处，以该表为准；写计划时一并写进设计文档新增的第 21 节。

## Global Constraints

- MSRV **1.89**，edition 2024。`unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 的两个函数例外（本计划不碰）。**本计划不新增任何 unsafe**。
- 依赖方向不变：`rurge-dns → rurge-rules → rurge-net → rurge-config`；`rurge-engine → { rurge-inbound → rurge-proto, rurge-policy → rurge-proto, rurge-dns }`；`rurge-api → rurge-engine`。**不新增依赖**。
- **测试绝不碰公网**：只用回环 + 端口 0 + 有界等待；不用固定 sleep 当同步手段（轮询 + 截止时间；只有"断言这段时间里什么也没发生"时才等一段固定时间）。**任何带 `url-test` / `fallback` / `load-balance` / `smart` 组的测试配置，`proxy-test-url` 与 `internet-test-url` 都必须指向回环**——引擎用例的 `Profile::text` 已默认指向 `http://127.0.0.1:9/`，不要删掉。UDP 测试的目标只用假对端隧道里的名字服务器（`10.0.0.53`，经回环上的假 WireGuard 对端）或回环。
- **测试与任何命令都不得修改本机的系统代理、注册表、网络设置，不得建网卡或路由，不得注册真实服务 / 计划任务**：不在 CLI 测试夹具之外运行 `rurge run --system-proxy`；不运行不带 `--dry-run` 的 `rurge service install | uninstall`。`dns-follow-interface` 的用例不绑真实网卡（引擎用例用 `NoopSocketHook`，`rurge-dns` 的用例用记录去向的连接器）。
- **不在本机下载或安装任何东西**（不装 sing-box、xray、WireGuard 工具，不 `rustup target add`、不 `cargo install`）。互操作用例在本机没有二进制时按既有约定跳过。
- **载荷与凭据永不外泄**：UDP 载荷与 DNS 查询名不进日志与错误文本；WireGuard 私钥、预共享密钥不进日志、错误文本与 `Debug`。
- rurge 专有的运行时选项只经命令行与环境变量提供，不扩展 Surge 配置格式（FR-CFG-17）。与 Surge 的每一处行为差异都登记进 `docs/surge-compatibility-matrix.md`（Task 6）。
- 日志、错误文本、CLI 输出、代码注释用英文；文档与提交标题用中文。注释密度、命名、惯用法与相邻代码保持一致；注释里不写评审轮次的标签。
- 提交信息结尾两行：`Co-Authored-By: <执行者自己的署名>` 与 `Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th`。**不 push、不 merge**，不 amend。
- 每个任务结束的门禁（全部通过才算完成）：

  ```bash
  RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check \
    && cargo clippy --all-targets -- -D warnings \
    && timeout 1500 cargo test --workspace --no-fail-fast
  ```

  `timeout` 不能省：`rurge-dns` 的一个用例曾让测试进程以 100% CPU 空转数小时（M3a「延后事项」#20）。测试二进制异常退出而没有失败用例时（`STATUS_ACCESS_VIOLATION`、`STATUS_HEAP_CORRUPTION` / `0xc0000374`、段错误——本机已知的既有问题，M3b 计划 P21），或整轮被 `timeout` 杀掉时，重跑一次并保留两次的日志，**不要在任务里去修它**。已知偶发失败的用例（`rurge-dns` 的 `a_partial_result_completes_aaaa_in_the_background` 与 `bootstrap::tests::stale_entries_are_served_and_refreshed_once`、`rurge` 的 `run::watch_reloads_rules_on_change` 与 `run::run_system_proxy_is_applied_switched_and_restored`、`rurge-engine` 的 `udp::a_closed_port_does_not_break_the_carrier`——见 P13）同样重跑。**编译器（`rustc` / 链接器）自己崩溃、报 PDB 损坏时多半是磁盘满了**：先看 `df -h /d`，删 `target/debug/incremental` 再重跑。
- 第三方 crate 的 API 以本机源码为准：`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`（`smoltcp-0.12.0`、`tokio-1.53.1`）。
- 本机的 bash 处理不了超过约 8 KB 或含反斜杠的 heredoc（`\\` 会被改写）：新文件与含反斜杠的改动一律用写文件的工具落盘，不用 heredoc。

## Review Focus

设计没有逐条写到、而最可能伤到使用者的五类输入或失败方式；每一条都在负责它的任务里配了用例。

1. **经 WireGuard 的 P2P 联机与语音**（对方从隧道里的另一个地址发来）：隧道那一端任何来源的回包都要原样送回客户端（全锥），而不是只认发过去的那个地址。用例：Task 1 `anyone_in_the_tunnel_may_answer`。
2. **发往域名的 UDP**：名字按节里的 `dns-server` 经隧道解析（与 TCP 一致），回包的来源是解析出的地址。用例：Task 1 `a_datagram_to_a_name_goes_where_the_name_says`。
3. **隧道去不了的目标**（不在任何 peer 的 `allowed-ips` 里、隧道没有该地址族的本端地址）：那条流以清楚的错误失败，绝不绕过隧道直连。用例：Task 1 `udp_where_the_tunnel_cannot_go_is_refused`。
4. **底层策略不载 UDP**（`underlying-proxy` 指向一条 `http` 策略）：拨号以说明失败（`via <底层策略>: the underlying policy cannot carry UDP`），不静默改走直连，也不让别的策略的隧道受影响。用例：Task 2 `a_tunnel_over_an_underlying_proxy_without_udp_fails_saying_so`、`a_policy_over_underlying_proxy_never_shares_the_tunnel`。
5. **只发不收的游戏与语音经 `smart` 组**：别的端口上 3 秒没有回包不能算成员失败（否则好节点被打低分）；DNS（53）与 QUIC（443）上的静默才算。用例：Task 4 `udp_silence_counts_only_where_answers_always_come`、`silence_that_tells_nothing_is_no_failure`。

## 计划期决定

写计划时对照设计、smoltcp 0.12 与本仓库源码、Surge 手册（`manual.nssurge.com` 的 `policies/parameters.html` 与 `policies/udp.html`，2026-09-30 查阅）核对后定下的事；与设计文档文字不同的，写进设计文档第 21 节。

**本计划里的代码不是凭空写的。** 全部 6 个任务的改动在仓库的一份副本上按任务顺序真实做了一遍（副本用自己的构建目录，不与本仓库的 `target/` 混用），最后一次全工作区门禁见 Task 6 的 Step 5。计划里新文件的全文取自副本上该任务的提交，修改处的"把 … 换成 …"由脚本从相邻两个任务提交的差异生成，并在拼好之后按计划的顺序套到开工前的源码上逐字核对过——计划文本与验证过的代码一字不差。每个任务 Step 2 的"预期失败"是只把该任务的用例块（及写明的前置改动）套到上一个任务的状态上、真实跑出来的。

| # | 事项 | 决定与依据 |
| - | ---- | ---------- |
| P1 | V7：隧道里的 UDP socket | smoltcp 0.12 的 `udp::Socket` 每个方向一个 `PacketBuffer`（64 个包、256 KiB，设计第 7 节），`bind` 要非零端口：用 TCP 已在用的 `Stack::free_port`（49152–65535）。每个载体按地址族各开一个 socket，第一次发往该族时绑定隧道那一族的本端地址（`Stack::udp_bind`）；与隧道内 DNS 自己的 socket 同在一个 `SocketSet` 里互不相干。发往任何地址（全锥，M5-D2），每个包按目的地址由既有的路由表选 peer；`Stack::check` 先问路由（没有 peer、没有该族本端地址时拒绝，错误文本同 TCP）。发送缓冲满（`BufferFull`）时丢掉这个包、不报错（同一个满了的 socket）；`Unaddressable` 是 `wireguard: <ip> cannot be sent to`。收包在两个 socket 上轮询、登记 waker；新开 socket 时唤醒等着收包的那一方。载体释放时关掉 socket |
| P2 | 名字解析共享 | 目标名的解析原本是 `WireGuardOutbound` 的私有方法，UDP 载体也要用：抽成 `pub(crate) struct Names`（节、解析器、缓存、隧道 DNS 的等待时长），出站与载体各持一个 `Arc<Names>`。用例原本直接改 `dns_wait` 字段，改用 `#[cfg(test)] fn set_dns_wait` |
| P3 | 链上的 UDP（M5a 延后事项 #3） | `ChainConnector::connect_udp` = `open_udp`（M5a 的链式载体）+ 解析目标 + 新的 `rurge_net::packet_datagram::packet_datagram(socket, to)`：发送、接收各一个任务与容量 256 的队列（满了丢，同 socket），目标是 IP 时只收来源是它的包（名字目标的回包来源是地址，不过滤），载体结束时收包以 `BrokenPipe`（`the carrier through the chain has closed`）结束。`wireguard` 的 peer 载体本来就走 `Connector::connect_udp`，于是自然经 `underlying-proxy` |
| P4 | 底层策略不载 UDP（订正设计 8.1） | 设计写"照旧 REJECT 并附说明"。实际链式载体打不开时，出站的错误是 `Io`（`via <底层策略>: the underlying policy cannot carry UDP`，与 M5a 的 `socks5` 经链一致），拨号按连接失败处理（`DialError::Failed`，请求记录写这句）；不做成 REJECT：REJECT 有 30 秒 50 次的自动升级，而这是配置问题。`device.rs` 里把载体错误改写成 `Unsupported` 的 `unreachable()` 去掉。`wireguard` 专用的 `W0029` 退役 |
| P5 | M4b 延后事项 #15 | "the peer cannot be reached" 按（策略、peer 序号）限频：5 分钟内第二次起降为 `debug!`（进程级的一张表，`may_say_unreachable`）。只影响日志，不影响重试 |
| P6 | M4b 延后事项 #24 | 已无对象：TCP 拨号里产生 `OutboundError::Unsupported` 的只剩 P4 去掉的那一处；`socks5` / `external` / 默认实现的 `Unsupported` 都只在 `open_udp`，UDP 流不换成员（P9）。不改 |
| P7 | V10：UDP 测试的形状 | `POST /v1/policies/test` 的每个结果多一个可选的 `udp` 键（`{"delay": ms}` 或 `{"error": …}`），有 UDP 测试的策略才有——新增键，不破坏现有客户端。另开 `Engine::test_udp(names)`，与 URL 测试并行（`tokio::join!`）；不经 `TestBook`：结果不保存、不影响组的选择（设计 8.4，手册"延迟测试只测 TCP"）、不进请求记录。策略的 `test-udp` 优先，否则 `[General] proxy-test-udp`；策略不载 UDP、不能测时没有这个键。时限是该策略的测试超时（`TestCase.timeout`，含 `wireguard` 另加的 10 秒） |
| P8 | UDP 测试的报文 | 新模块 `rurge_policy::udp_probe`：经 `open_udp` 的载体向 `ipv4:53` 发一个手写的 A 查询（RD 置位；ID 由时钟的纳秒与进程内计数混合，前后两次不同），等到来源是那个服务器、ID 相同、是应答的报文即成功（不看应答码：手册只说"发一个 DNS 查询"，能回就说明 UDP 通）；超时是 `udp test timed out`，不载 UDP 是 `the policy carries no UDP`，名字编不成报文是 `` `<名字>` cannot be asked for ``。只有 `probe_udp_at` 能指定服务器端口，供用例使用 |
| P9 | V9：`smart` 计入 UDP（M3c 延后事项 #6） | 复用 M3c 的 `watch`：多一个 `silence: bool` 参数，UDP 流只在目标端口是 53 或 443 时为 `true`（TCP 一律 `true`）。载体打不开时 `report_failure`（与 TCP 连接失败同样计分）；载体就绪后 `used` + `watch`，第一个回包即首字节（M5a 的 `SessionHandle` 在 UDP 流上已有首字节时刻）。UDP 不换成员；经 `udp-policy-not-supported-behaviour` 改走 DIRECT 的流不回报（那不是成员的表现） |
| P10 | V8：`dns-follow-interface` 的范围（订正设计 8.5） | 设计写"解析这条策略的服务器名"。手册：「DNS requests that match the policy will use this interface for queries」。取两者之间：策略自己的直连连接器做的全部解析——`direct` 策略的目标、代理策略的服务器名——都经它的网卡问；规则匹配时的解析发生在选定策略之前，仍走全局（与设计一致）。做法：`rurge-net` 的 `Resolve` 多一个带默认实现的 `resolve_via(host, &Via)`（`Via` = 键 + 连接器）与包装 `ResolveVia`；引擎的 `direct_connector` 在 `interface` 与 `dns-follow-interface` 都写了时，给策略的 `DirectConnector` 一个 `ResolveVia`，其连接器是绑这个网卡的 `DirectConnector`；`ResolverCell` 转发 `resolve_via` |
| P11 | `dns-follow-interface` 在 `rurge-dns` 里 | `Resolver::lookup_via`：`[Host]`、hosts 文件、`.local` 与经系统接口的解析照常（它们不发 DNS 报文）；到了问上游那一步，**只有没配加密 DNS 时**才改问经 `Via` 的一组上游——`dns-server` 里的普通服务器与 `system` 展开的系统服务器（`traditional_specs`），每个是 `UdpUpstream::via`（UDP 经连接器的 `connect_udp`，截断后的 TCP 重试经它的 `connect`）；这组上游与自己的缓存按 `Via.key`（网卡名）存一份，`flush` 清掉（网络变化后按新的系统服务器重建）。配了 `encrypted-dns-server` 时照常问加密 DNS（它的连接走全局，跟随网卡要另建一套加密上游，不在本计划），第一次时记一条 info |
| P12 | `dns-follow-interface` 没写 `interface` | `W0028`（`` `dns-follow-interface` has no effect without `interface`; ignored ``），取值当作 `false`；从 `W0029` 的"解析但不生效"名单里去掉 |
| P13 | 既有的偶发失败 | 副本上跑 `cargo test -p rurge-net -p rurge-dns -p rurge-config -p rurge-engine` 时，M5a 的 `udp::a_closed_port_does_not_break_the_carrier` 失败过一次（300 ms 的"静默窗口"里收到了东西：它先释放一个端口当"关着的端口"，并行的别的用例可能恰好绑上它），单独连跑 5 次都通过。与本计划无关，列入门禁的重跑名单与延后事项 |
| P14 | 互操作 | sing-box 的 WireGuard 端点对 UDP 同样把发往端点自身隧道地址的包改写到回环（与 TCP 同一个入口）；在既有用例里经隧道往返一个回环 UDP 回显（发往 `10.9.0.1:<回显端口>`）。本机没有 sing-box，由 CI 首跑证明 |
| P15 | 任务的切分 | 与设计第 16 节草图相同的 6 个任务：1 WireGuard 的 UDP；2 链上的 UDP、经 `underlying-proxy`、M4b #15 / #24；3 UDP 测试；4 `smart` 计入 UDP；5 `dns-follow-interface`；6 互操作与文档 |

## 承接事项

之前计划「延后事项」表里标给 M5c（或 M5）的条目，及仍然有效的既有现象。

| # | 来源 | 事项 | 处理 | 任务 |
| - | ---- | ---- | ---- | ---- |
| C1 | M5 设计第 16 节 | WireGuard 的 UDP、经 `underlying-proxy`、UDP 测试、`smart` 计入 UDP、`dns-follow-interface`、互操作与文档 | 本计划 | 1–6 |
| C2 | M4b #2 | 经 WireGuard 的 UDP、`underlying-proxy`、`ecn` | UDP 与 `underlying-proxy` 本计划（1、2）；`ecn` 仍解析并忽略（`W0029`，设计 M5-D6） | 1、2 |
| C3 | M4b #15 | "the peer cannot be reached" 不限频 | 本计划（P5） | 2 |
| C4 | M4b #24 | `Unsupported` 覆盖 `smart` 的说明 | 已无对象（P6） | — |
| C5 | M3c #6 | UDP 不计入 `smart` 的打分 | 本计划（P9） | 4 |
| C6 | M5a #3 | `ChainConnector::connect_udp` | 本计划（P3） | 2 |
| C7 | M5a #12 | `ChainConnector` 的 UDP / TCP 前导重复、几个分支缺单元测试 | 不做：`connect_udp` 复用 `open_udp`，没有第三份前导；照旧延后 | — |
| C8 | M3b #7（P21） | 测试二进制偶发崩溃 | 照旧：门禁遇到就重跑 | — |

## File Structure

新建：

| 文件 | 职责 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-proto-wireguard/src/udp.rs` | 隧道里的 UDP 载体 `TunnelUdp`（每个地址族一个 socket、全锥） | 1 |
| `crates/rurge-net/src/packet_datagram.rs` | `packet_datagram`：按包收发的载体 → 到固定目标的 `Datagram`（与用例） | 2 |
| `crates/rurge-policy/src/udp_probe.rs` | UDP 测试：经策略的 UDP 问一次 A 记录（与用例） | 3 |

修改：

| 文件 | 改动 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-proto-wireguard/src/{stack.rs, outbound.rs, lib.rs}` | `udp_bind` / `check`；`Names`、`udp()` / `open_udp()`，与用例 | 1 |
| `crates/rurge-proto-wireguard/src/testing/{mod.rs, peer.rs}` | 假对端的 UDP 回显与 `udp_from` / `send_udp` | 1 |
| `crates/rurge-proto-wireguard/src/device.rs` | 载体错误不再改写成 `Unsupported`；"the peer cannot be reached" 限频 | 2 |
| `crates/rurge-net/src/lib.rs`、`crates/rurge-policy/src/cell.rs` | `mod packet_datagram`；`ChainConnector::connect_udp` | 2 |
| `crates/rurge-config/src/spec/{mod.rs, common.rs}`、`tests/policy_spec.rs`、快照 | `wireguard` 的 `W0029` 退役（2）；`test-udp` 生效（3）；`dns-follow-interface`（5） | 2、3、5 |
| `crates/rurge-policy/src/lib.rs`、`crates/rurge-engine/src/auto.rs`、`crates/rurge-api/src/routes/policies.rs` | `mod udp_probe`；`Engine::test_udp`；`udp` 键 | 3 |
| `crates/rurge-engine/src/{smart.rs, engine.rs, udp.rs}` | `watch` 的 `silence`；UDP 流的回报 | 4 |
| `crates/rurge-net/src/connector.rs`、`crates/rurge-dns/src/{resolver.rs, upstream/udp.rs}`、`crates/rurge-engine/src/{outbounds.rs, shared.rs}` | `Via` / `ResolveVia`；`lookup_via` 与 `UdpUpstream::via`；`direct_connector` 与 `ResolverCell` | 5 |
| `crates/rurge-engine/tests/{outbounds_wireguard.rs, smart.rs}` | 经引擎的用例 | 1–4 |
| `tests/interop/{tests/sing_box_wireguard.rs, README.md}` | 经 sing-box WireGuard 端点的 UDP | 6 |
| 文档（兼容性清单、`docs/api/phase2.md`、两份 README、`CLAUDE.md`、手工验收） | 见 Task 6 | 6 |

## 任务一览

| 任务 | 交付物 | 依赖 |
| ---- | ------ | ---- |
| 1 | `TunnelUdp`、`wireguard` 的 `udp()` / `open_udp()`、假对端的 UDP、经引擎的用例 | — |
| 2 | `packet_datagram`、`ChainConnector::connect_udp`、经 `underlying-proxy` 的隧道、M4b #15 | 1（用例） |
| 3 | `udp_probe`、`Engine::test_udp`、API 的 `udp` 键、`test-udp` 生效 | 1（隧道里的 UDP） |
| 4 | `smart` 计入 UDP | — |
| 5 | `dns-follow-interface` | — |
| 6 | 互操作与文档 | 1–5 |

---

### Task 1: WireGuard 的 UDP

隧道的 smoltcp 协议栈里开 UDP socket（P1），`WireGuardOutbound` 的 `udp()` 变成 `Native`，`open_udp` 启动隧道后交出 `TunnelUdp`。目标名的解析从出站里抽成共享的 `Names`（P2），载体与 TCP 拨号用同一份。假对端 `FakeWgPeer` 学会 UDP：隧道里任何地址的 `ECHO_PORT`（7）上都有 UDP 回显，另能从隧道里任意地址主动发包（全锥用例的"陌生人"）。

**Files:**
- Create: `crates/rurge-proto-wireguard/src/udp.rs`
- Modify: `crates/rurge-proto-wireguard/src/stack.rs`（`udp_bind` / `check`）、`src/outbound.rs`（`Names`、`udp()` / `open_udp()`，与用例）、`src/lib.rs`、`src/testing/mod.rs`、`src/testing/peer.rs`、`crates/rurge-engine/tests/outbounds_wireguard.rs`

**Interfaces:**
- Consumes: M5a 的 `rurge_net::connector::{PacketSocket, BoxedPacketSocket}`、`Outbound::udp` / `open_udp` 与 `UdpSupport`；既有的 `Stack::{free_port, source_for, udp}`、`Refusal`、`Device::{shared, kick}`、`dns::answered`。
- Produces:
  - `Stack::udp_bind(&mut self, family: IpAddr) -> Result<SocketHandle, Refusal>`、`Stack::check(&self, to: IpAddr) -> Result<(), Refusal>`
  - `pub(crate) struct outbound::Names`：`address(&self, target: &Target, device: &Arc<Device>) -> Result<IpAddr, OutboundError>`（地址原样返回，名字经隧道 DNS 或本机解析）；出站的字段 `names: Arc<Names>`；`#[cfg(test)] fn set_dns_wait(&mut self, wait: Duration)`
  - `pub(crate) struct udp::TunnelUdp`，`TunnelUdp::new(device: Arc<Device>, names: Arc<Names>)`，实现 `PacketSocket`
  - `WireGuardOutbound`：`udp()` 为 `Native`；`open_udp` 在 `opts.timeout` 内启动隧道（超时是 `OutboundError::Timeout`）
  - 测试设施：`FakeWgPeer` 核心的 `pub udp_echoed: Vec<(SocketAddr, SocketAddr)>`（回显过的包：来源、目标）与 `pub fn udp_from(&mut self, from: SocketAddrV4, to: SocketAddrV4, payload: &[u8], out: &mut Vec<Vec<u8>>)`；`FakeWgPeer::send_udp(&self, from: SocketAddrV4, to: SocketAddrV4, payload: &[u8])`

- [ ] **Step 1: 先写用例（连同假对端的 UDP）**

假对端的 UDP 回显与主动发包：

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
//! answers on every address routed to it — a TCP echo service on port 7, an
//! HTTP service on port 80 and a name server at `DNS_ADDRESS`. `FakeWgPeer`
//! puts one on a loopback UDP port.
```

换成

```rust
//! answers on every address routed to it — a TCP and a UDP echo service on
//! port 7, an HTTP service on port 80 and a name server at `DNS_ADDRESS`.
//! `FakeWgPeer` puts one on a loopback UDP port.
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
    HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, IpProtocol, Ipv4Packet,
};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};

/// The TCP echo service of a peer.
```

换成

```rust
    HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, IpProtocol, Ipv4Packet, Ipv4Repr,
    UdpPacket, UdpRepr,
};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4};
use std::time::{Duration, Instant};

/// The TCP and the UDP echo service of a peer.
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
    name_server: SocketHandle,
```

换成

```rust
    name_server: SocketHandle,
    udp_echo: SocketHandle,
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
    pub http_requests: usize,
```

换成

```rust
    pub http_requests: usize,
    /// Every datagram its UDP echo service answered: where it came from and
    /// where it went.
    pub udp_echoed: Vec<(SocketAddr, SocketAddr)>,
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
        let name_server = sockets.add(name_server);
```

换成

```rust
        let name_server = sockets.add(name_server);
        let echo_buffer =
            || udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 16], vec![0; 65536]);
        let mut udp_echo = udp::Socket::new(echo_buffer(), echo_buffer());
        // every address routed to it
        udp_echo.bind(ECHO_PORT).expect("the echo port");
        let udp_echo = sockets.add(udp_echo);
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
            name_server,
```

换成

```rust
            name_server,
            udp_echo,
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
            http_requests: 0,
```

换成

```rust
            http_requests: 0,
            udp_echoed: Vec::new(),
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
            out.push(marked(message, self.client_id));
        }
    }

    /// An echo request with `size` bytes of data from `from` to `to`, in
```

换成

```rust
            out.push(marked(message, self.client_id));
        }
    }

    /// A UDP datagram from `from` to `to` into the tunnel, as if a host
    /// behind the peer sent it.
    pub fn udp_from(
        &mut self,
        from: SocketAddrV4,
        to: SocketAddrV4,
        payload: &[u8],
        out: &mut Vec<Vec<u8>>,
    ) {
        let udp = UdpRepr {
            src_port: from.port(),
            dst_port: to.port(),
        };
        let ip = Ipv4Repr {
            src_addr: *from.ip(),
            dst_addr: *to.ip(),
            next_header: IpProtocol::Udp,
            payload_len: udp.header_len() + payload.len(),
            hop_limit: 64,
        };
        let caps = ChecksumCapabilities::default();
        let mut packet = vec![0u8; ip.buffer_len() + ip.payload_len];
        ip.emit(&mut Ipv4Packet::new_unchecked(&mut packet), &caps);
        udp.emit(
            &mut UdpPacket::new_unchecked(&mut packet[ip.buffer_len()..]),
            &IpAddress::Ipv4(*from.ip()),
            &IpAddress::Ipv4(*to.ip()),
            payload.len(),
            |room| room.copy_from_slice(payload),
            &caps,
        );
        self.inject(&packet, out);
    }

    /// An echo request with `size` bytes of data from `from` to `to`, in
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
        self.answer_questions();
```

换成

```rust
        self.answer_questions();
        self.echo_datagrams();
    }

    /// Sends every datagram to port 7 back where it came from, from where it
    /// went.
    fn echo_datagrams(&mut self) {
        let socket = self.sockets.get_mut::<udp::Socket>(self.udp_echo);
        let mut datagrams = Vec::new();
        while let Ok((data, meta)) = socket.recv() {
            datagrams.push((data.to_vec(), meta));
        }
        for (data, meta) in datagrams {
            let from = SocketAddr::new(meta.endpoint.addr.into(), meta.endpoint.port);
            let to = meta
                .local_address
                .map(|a| SocketAddr::new(a.into(), ECHO_PORT));
            if let Some(to) = to {
                self.udp_echoed.push((from, to));
            }
            let socket = self.sockets.get_mut::<udp::Socket>(self.udp_echo);
            let _ = socket.send_slice(&data, meta);
        }
```

`crates/rurge-proto-wireguard/src/testing/peer.rs`——把

```rust
use std::net::SocketAddr;
```

换成

```rust
use std::net::{SocketAddr, SocketAddrV4};
```

`crates/rurge-proto-wireguard/src/testing/peer.rs`——把

```rust
    clients: Arc<Mutex<Vec<SocketAddr>>>,
    task: JoinHandle<()>,
```

换成

```rust
    clients: Arc<Mutex<Vec<SocketAddr>>>,
    socket: Arc<UdpSocket>,
    task: JoinHandle<()>,
```

`crates/rurge-proto-wireguard/src/testing/peer.rs`——把

```rust
        let task = tokio::spawn(serve(socket, core.clone(), silent.clone(), clients.clone()));
```

换成

```rust
        let socket = Arc::new(socket);
        let task = tokio::spawn(serve(
            socket.clone(),
            core.clone(),
            silent.clone(),
            clients.clone(),
        ));
```

`crates/rurge-proto-wireguard/src/testing/peer.rs`——把

```rust
            clients,
```

换成

```rust
            clients,
            socket,
```

`crates/rurge-proto-wireguard/src/testing/peer.rs`——把

```rust
        self.clients.lock().expect("the clients").clone()
    }

    /// From now on it drops whatever arrives and sends nothing, as a peer
```

换成

```rust
        self.clients.lock().expect("the clients").clone()
    }

    /// A UDP datagram from `from` to `to` through the tunnel, as if a host
    /// behind it sent it; to where the client last wrote from.
    pub async fn send_udp(&self, from: SocketAddrV4, to: SocketAddrV4, payload: &[u8]) {
        let mut out = Vec::new();
        self.core().udp_from(from, to, payload, &mut out);
        let client = *self.clients().last().expect("a client wrote");
        for message in out {
            let _ = self.socket.send_to(&message, client).await;
        }
    }

    /// From now on it drops whatever arrives and sends nothing, as a peer
```

`crates/rurge-proto-wireguard/src/testing/peer.rs`——把

```rust
    socket: UdpSocket,
```

换成

```rust
    socket: Arc<UdpSocket>,
```

出站的用例（既有用例改用 `set_dns_wait` 与 `names.section`，新增四条）：

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
    use rurge_net::connector::{BoxedDatagram, Datagram, DirectConnector, SystemResolve};
    use std::io;
    use std::net::Ipv4Addr;
```

换成

```rust
    use rurge_net::connector::{
        BoxedDatagram, Datagram, DirectConnector, PacketSocket, SystemResolve,
    };
    use std::io;
    use std::net::{Ipv4Addr, SocketAddrV4};
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
        wg.dns_wait = Duration::from_millis(900);
```

换成

```rust
        wg.set_dns_wait(Duration::from_millis(900));
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
        wg.dns_wait = Duration::from_millis(300);
```

换成

```rust
        wg.set_dns_wait(Duration::from_millis(300));
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
        let (_peer, mut wg) = with_tunnel_dns(&["10.0.0.1"], &unreachable, |_| {}).await;
        wg.dns_wait = Duration::from_secs(5);
```

换成

```rust
        let (_peer, mut wg) = with_tunnel_dns(&["10.0.0.1"], &unreachable, |_| {}).await;
        wg.set_dns_wait(Duration::from_secs(5));
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
                wg.dns_wait = Duration::from_secs(5);
```

换成

```rust
                wg.set_dns_wait(Duration::from_secs(5));
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
        let (peer, a) = tunnel(PeerOpts::default(), |_| {}).await;
        let spec = WireGuardSpec {
            section: a.section.clone(),
        };
```

换成

```rust
        let (peer, a) = tunnel(PeerOpts::default(), |_| {}).await;
        let spec = WireGuardSpec {
            section: a.names.section.clone(),
        };
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
        let mut section = old.section.clone();
```

换成

```rust
        let mut section = old.names.section.clone();
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
        assert_eq!(echo(&mut stream, b"ping").await, b"ping");
        let spec = WireGuardSpec {
            section: a.section.clone(),
        };
```

换成

```rust
        assert_eq!(echo(&mut stream, b"ping").await, b"ping");
        let spec = WireGuardSpec {
            section: a.names.section.clone(),
        };
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
            section: old.section.clone(),
```

换成

```rust
            section: old.names.section.clone(),
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
            section: a1.section.clone(),
```

换成

```rust
            section: a1.names.section.clone(),
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
        stuck_dial.abort();
    }
}
```

换成

```rust
        stuck_dial.abort();
    }

    async fn udp_answer(carrier: &dyn PacketSocket) -> (Vec<u8>, Target) {
        let mut buf = vec![0u8; 65536];
        let (n, from) = tokio::time::timeout(Duration::from_secs(5), carrier.recv_from(&mut buf))
            .await
            .expect("an answer within the bound")
            .unwrap();
        (buf[..n].to_vec(), from)
    }

    /// Datagrams through the tunnel to the peer's UDP echo and back, from
    /// the tunnel's address: one socket for every destination.
    #[tokio::test]
    async fn udp_goes_through_the_tunnel() {
        let (peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        assert_eq!(wg.udp(), UdpSupport::Native);
        let carrier = wg.open_udp(&within(5)).await.expect("a carrier");
        for (host, payload) in [("10.0.0.1", &b"ping"[..]), ("10.0.0.2", b"pong")] {
            let to = at(host, ECHO_PORT);
            carrier.send_to(payload, &to).await.unwrap();
            assert_eq!(udp_answer(carrier.as_ref()).await, (payload.to_vec(), to));
        }
        let echoed = peer.core().udp_echoed.clone();
        assert_eq!(echoed.len(), 2);
        assert_eq!(echoed[0].0.ip(), IpAddr::V4(Ipv4Addr::new(10, 9, 0, 2)));
        assert_eq!(echoed[0].0, echoed[1].0, "one socket for both");
        assert_eq!(
            (echoed[0].1, echoed[1].1),
            ("10.0.0.1:7".parse().unwrap(), "10.0.0.2:7".parse().unwrap())
        );
    }

    /// Full cone: a host behind the peer that was never written to reaches
    /// the carrier, under its own address.
    #[tokio::test]
    async fn anyone_in_the_tunnel_may_answer() {
        let (peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let carrier = wg.open_udp(&within(5)).await.expect("a carrier");
        carrier
            .send_to(b"hello", &at("10.0.0.1", ECHO_PORT))
            .await
            .unwrap();
        udp_answer(carrier.as_ref()).await;
        let SocketAddr::V4(ours) = peer.core().udp_echoed[0].0 else {
            panic!("the tunnel is IPv4")
        };
        let stranger: SocketAddrV4 = "10.0.0.9:5000".parse().unwrap();
        peer.send_udp(stranger, ours, b"unasked").await;
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"unasked".to_vec(), at("10.0.0.9", 5000))
        );
    }

    /// A name is looked up as for a connection (here on this machine), and
    /// the datagram goes to what it gave.
    #[tokio::test]
    async fn a_datagram_to_a_name_goes_where_the_name_says() {
        let names = Arc::new(Names(vec![(
            "echo.test",
            vec!["10.0.0.1".parse().unwrap()],
        )]));
        let (_peer, wg) = tunnel_with(PeerOpts::default(), |_| {}, names, direct()).await;
        let carrier = wg.open_udp(&within(5)).await.expect("a carrier");
        let name = at("echo.test", ECHO_PORT);
        assert_eq!(
            carrier.resolve(&name).await.unwrap(),
            at("10.0.0.1", ECHO_PORT)
        );
        carrier.send_to(b"q", &name).await.unwrap();
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"q".to_vec(), at("10.0.0.1", ECHO_PORT))
        );
    }

    /// A destination no peer takes, or of a family the tunnel has no address
    /// of, is refused at once, saying why.
    #[tokio::test]
    async fn udp_where_the_tunnel_cannot_go_is_refused() {
        let (_peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let carrier = wg.open_udp(&within(5)).await.expect("a carrier");
        for (host, text) in [
            (
                "192.168.1.1",
                "wireguard: no peer's allowed-ips covers 192.168.1.1",
            ),
            ("fd00::1", "wireguard: the tunnel has no IPv6 address"),
        ] {
            let err = carrier.send_to(b"x", &at(host, 53)).await.unwrap_err();
            assert_eq!(err.to_string(), text);
        }
    }
}
```

经引擎的用例：

`crates/rurge-engine/tests/outbounds_wireguard.rs`——把

```rust
    assert_eq!(peer.core().handshakes, 2, "a tunnel of its own");
}

```

换成

```rust
    assert_eq!(peer.core().handshakes, 2, "a tunnel of its own");
}

/// UDP from a SOCKS5 association through the tunnel to the peer's echo, and
/// back; a host behind the peer that was never written to reaches the
/// client too (full cone, phase 2 M5 design §7).
#[tokio::test]
async fn udp_leaves_through_the_tunnel() {
    let (peer, section) = peer().await;
    let h = harness(Profile {
        proxies: WG,
        rules: TO_WG,
        sections: &section,
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    association.send("10.0.0.1", ECHO_PORT, b"through").await;
    assert_eq!(association.recv().await, (echo_addr(), b"through".to_vec()));
    let SocketAddr::V4(ours) = peer.core().udp_echoed[0].0 else {
        panic!("the tunnel is IPv4")
    };
    peer.send_udp("10.0.0.9:5000".parse().unwrap(), ours, b"unasked")
        .await;
    assert_eq!(
        association.recv().await,
        (SocketAddr::from(([10, 0, 0, 9], 5000)), b"unasked".to_vec())
    );
    drop(association);
    let log = h.engine.request_log();
    wait_until("the flow to finish", || !log.recent(10).is_empty()).await;
    assert_eq!(log.recent(10)[0].policy, ["WG"]);
}

```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto-wireguard --lib outbound`
Expected: FAIL——`Names`、`set_dns_wait` 与 `UdpSupport` 由 Step 3 引入，编译不过：

```text
error[E0433]: failed to resolve: use of undeclared type `UdpSupport`
    --> crates\rurge-proto-wireguard\src\outbound.rs:1517:30
error[E0599]: no method named `set_dns_wait` found for struct `outbound::WireGuardOutbound` in the current scope
   --> crates\rurge-proto-wireguard\src\outbound.rs:711:12
error[E0599]: no method named `set_dns_wait` found for struct `outbound::WireGuardOutbound` in the current scope
   --> crates\rurge-proto-wireguard\src\outbound.rs:779:12
error[E0599]: no method named `set_dns_wait` found for struct `outbound::WireGuardOutbound` in the current scope
   --> crates\rurge-proto-wireguard\src\outbound.rs:793:12
error[E0599]: no method named `set_dns_wait` found for struct `outbound::WireGuardOutbound` in the current scope
   --> crates\rurge-proto-wireguard\src\outbound.rs:822:20
error[E0609]: no field `names` on type `outbound::WireGuardOutbound`
   --> crates\rurge-proto-wireguard\src\outbound.rs:952:24
error[E0609]: no field `names` on type `outbound::WireGuardOutbound`
   --> crates\rurge-proto-wireguard\src\outbound.rs:976:31
error[E0609]: no field `names` on type `outbound::WireGuardOutbound`
    --> crates\rurge-proto-wireguard\src\outbound.rs:1383:24
error[E0609]: no field `names` on type `outbound::WireGuardOutbound`
    --> crates\rurge-proto-wireguard\src\outbound.rs:1414:26
error[E0609]: no field `names` on type `outbound::WireGuardOutbound`
    --> crates\rurge-proto-wireguard\src\outbound.rs:1490:25
Some errors have detailed explanations: E0433, E0599, E0609.
For more information about an error, try `rustc --explain E0433`.
error: could not compile `rurge-proto-wireguard` (lib test) due to 10 previous errors
exit 101
```

Run: `cargo test -p rurge-engine --test outbounds_wireguard udp`
Expected: FAIL——`wireguard` 还不载 UDP，引擎按 `udp-policy-not-supported-behaviour` 的默认拒绝这条流，客户端等不到回包（`common/mod.rs:277` 是 `UdpAssociation::recv` 的 `expect("a datagram comes back")`）：

```text
test udp_leaves_through_the_tunnel ... FAILED
thread 'udp_leaves_through_the_tunnel' panicked at crates\rurge-engine\tests\common\mod.rs:277:18:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 8 filtered out; finished in 5.03s
error: test failed, to rerun pass `-p rurge-engine --test outbounds_wireguard`
exit 101
```

- [ ] **Step 3: 实现**

协议栈的两个入口：

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
const SCRATCH: usize = 65536 + 32;
```

换成

```rust
const SCRATCH: usize = 65536 + 32;
/// A UDP carrier's socket, each way (phase 2 M5 design §7): this many
/// datagrams, in this much room, wait at most.
const UDP_PACKETS: usize = 64;
const UDP_BUFFER: usize = 256 * 1024;
```

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
            .map_err(|_| Refusal::Unaddressable(to))?;
        Ok(self.sockets.add(socket))
    }
```

换成

```rust
            .map_err(|_| Refusal::Unaddressable(to))?;
        Ok(self.sockets.add(socket))
    }

    /// A UDP carrier's socket (phase 2 M5 design §7): on the tunnel's
    /// address of `family`'s family and a free port, for datagrams to and
    /// from anywhere.
    pub fn udp_bind(&mut self, family: IpAddr) -> Result<SocketHandle, Refusal> {
        let local = match family {
            IpAddr::V4(_) => self.v4.map(IpAddr::V4),
            IpAddr::V6(_) => self.v6.map(IpAddr::V6),
        }
        .ok_or(Refusal::NoAddress(family))?;
        let port = self.free_port().ok_or(Refusal::NoPort)?;
        let buffer = || {
            udp::PacketBuffer::new(
                vec![udp::PacketMetadata::EMPTY; UDP_PACKETS],
                vec![0; UDP_BUFFER],
            )
        };
        let mut socket = udp::Socket::new(buffer(), buffer());
        socket
            .bind(SocketAddr::new(local, port))
            .map_err(|_| Refusal::Unaddressable(family))?;
        Ok(self.sockets.add(socket))
    }

    /// Why nothing can be sent to `to` through the tunnel, if something
    /// stands in the way: no address of its family, no peer that takes it.
    pub fn check(&self, to: IpAddr) -> Result<(), Refusal> {
        self.source_for(to).map(|_| ())
    }
```

新模块：

新建 `crates/rurge-proto-wireguard/src/udp.rs`：

```rust
//! UDP through the tunnel (phase 2 M5 design §7): one socket of the tunnel's
//! stack for each address family, on the tunnel's address and a free port,
//! for datagrams to and from anywhere the peers take — full cone. Each
//! datagram leaves through the peer its destination routes to.

use crate::device::Device;
use crate::outbound::Names;
use rurge_config::HostName;
use rurge_net::BoxFuture;
use rurge_net::connector::{PacketSocket, Target};
use smoltcp::iface::SocketHandle;
use smoltcp::socket::udp;
use std::future::poll_fn;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};

pub(crate) struct TunnelUdp {
    device: Arc<Device>,
    names: Arc<Names>,
    /// The IPv4 socket and the IPv6 one, once a datagram went to its family.
    sockets: Mutex<[Option<SocketHandle>; 2]>,
    /// Whoever waits to receive: woken when a socket opens.
    receiver: Mutex<Option<Waker>>,
}

fn slot(ip: IpAddr) -> usize {
    match ip {
        IpAddr::V4(_) => 0,
        IpAddr::V6(_) => 1,
    }
}

impl TunnelUdp {
    pub(crate) fn new(device: Arc<Device>, names: Arc<Names>) -> TunnelUdp {
        TunnelUdp {
            device,
            names,
            sockets: Mutex::new([None, None]),
            receiver: Mutex::new(None),
        }
    }

    /// `to`'s address: a name is looked up as for a connection.
    async fn address(&self, to: &Target) -> io::Result<IpAddr> {
        match &to.host {
            HostName::Ip(ip) => Ok(*ip),
            HostName::Domain(_) => self
                .names
                .address(to, &self.device)
                .await
                .map_err(|e| io::Error::other(e.to_string())),
        }
    }
}

impl PacketSocket for TunnelUdp {
    fn resolve<'a>(&'a self, to: &'a Target) -> BoxFuture<'a, io::Result<Target>> {
        Box::pin(async move {
            let ip = self.address(to).await?;
            Ok(Target::new(HostName::Ip(ip), to.port))
        })
    }

    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            let ip = self.address(to).await?;
            {
                let mut stack = self.device.shared.stack.lock().expect("the tunnel");
                stack.check(ip).map_err(|refusal| {
                    io::Error::new(io::ErrorKind::Unsupported, refusal.to_string())
                })?;
                let mut sockets = self.sockets.lock().expect("the sockets");
                let handle = match sockets[slot(ip)] {
                    Some(handle) => handle,
                    None => {
                        let handle = stack.udp_bind(ip).map_err(|refusal| {
                            io::Error::new(io::ErrorKind::AddrNotAvailable, refusal.to_string())
                        })?;
                        sockets[slot(ip)] = Some(handle);
                        // the receiver waits on the sockets it knew of
                        if let Some(waker) = self.receiver.lock().expect("the receiver").take() {
                            waker.wake();
                        }
                        handle
                    }
                };
                match stack
                    .udp(handle)
                    .send_slice(buf, SocketAddr::new(ip, to.port))
                {
                    Ok(()) => {}
                    // no room left: dropped, as a full socket drops it
                    Err(udp::SendError::BufferFull) => return Ok(()),
                    Err(udp::SendError::Unaddressable) => {
                        return Err(io::Error::new(
                            io::ErrorKind::AddrNotAvailable,
                            format!("wireguard: {ip} cannot be sent to"),
                        ));
                    }
                }
            }
            self.device.shared.kick();
            Ok(())
        })
    }

    /// A datagram longer than `buf` is cut to it: give it 64 KiB.
    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(poll_fn(move |cx| {
            // told of a socket that opens from here on
            *self.receiver.lock().expect("the receiver") = Some(cx.waker().clone());
            let handles = *self.sockets.lock().expect("the sockets");
            let mut stack = self.device.shared.stack.lock().expect("the tunnel");
            for handle in handles.into_iter().flatten() {
                let socket = stack.udp(handle);
                if let Ok((datagram, meta)) = socket.recv() {
                    let n = datagram.len().min(buf.len());
                    buf[..n].copy_from_slice(&datagram[..n]);
                    let from =
                        Target::new(HostName::Ip(meta.endpoint.addr.into()), meta.endpoint.port);
                    return Poll::Ready(Ok((n, from)));
                }
                socket.register_recv_waker(cx.waker());
            }
            Poll::Pending
        }))
    }
}

impl Drop for TunnelUdp {
    fn drop(&mut self) {
        let handles = *self.sockets.lock().expect("the sockets");
        if let Ok(mut stack) = self.device.shared.stack.lock() {
            for handle in handles.into_iter().flatten() {
                stack.udp_close(handle);
            }
        }
    }
}
```

`crates/rurge-proto-wireguard/src/lib.rs`——把

```rust
//! stack of its own — that TCP connections are dialled through.
```

换成

```rust
//! stack of its own — that TCP connections and UDP datagrams go through.
```

`crates/rurge-proto-wireguard/src/lib.rs`——把

```rust
mod stream;
```

换成

```rust
mod stream;
mod udp;
```

出站：`Names` 抽出、`udp()` / `open_udp()`：

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
//! lives.
```

换成

```rust
//! lives. UDP goes through a socket of the tunnel's own stack (`udp`).
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
use crate::stack::Refusal;
```

换成

```rust
use crate::stack::Refusal;
use crate::udp::TunnelUdp;
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Resolve, Target};
use rurge_proto::{Outbound, OutboundError};
```

换成

```rust
use rurge_net::connector::{
    BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, Resolve, Target,
};
use rurge_proto::{Outbound, OutboundError, UdpSupport};
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
    name: String,
    section: WireGuardSection,
```

换成

```rust
    name: String,
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
    carrier: String,
    /// Destination names, without a `dns-server` (M4-D9) and for its
    /// `system` entries.
    resolver: Arc<dyn Resolve>,
```

换成

```rust
    carrier: String,
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
    device: Mutex<Option<Arc<Device>>>,
```

换成

```rust
    device: Mutex<Option<Arc<Device>>>,
    /// The section, and how destination names are looked up: shared with
    /// the UDP carriers.
    names: Arc<Names>,
    /// `REDIAL` (shorter in the tests).
    redial: Duration,
}

/// Where destination names go (M4-D9): the section's `dns-server`s through
/// the tunnel, or this machine.
pub(crate) struct Names {
    section: WireGuardSection,
    /// Destination names, without a `dns-server` (M4-D9) and for its
    /// `system` entries.
    resolver: Arc<dyn Resolve>,
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
    dns_wait: Duration,
    /// `REDIAL` (shorter in the tests).
    redial: Duration,
```

换成

```rust
    dns_wait: Duration,
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
            section: spec.section.clone(),
            generation: GENERATION.fetch_add(1, Ordering::Relaxed),
            carrier: String::new(),
            resolver,
            connector,
            device: Mutex::new(None),
            cache: StdMutex::new(Cache::default()),
            dns_wait: DNS_WAIT,
```

换成

```rust
            generation: GENERATION.fetch_add(1, Ordering::Relaxed),
            carrier: String::new(),
            connector,
            device: Mutex::new(None),
            names: Arc::new(Names {
                section: spec.section.clone(),
                resolver,
                cache: StdMutex::new(Cache::default()),
                dns_wait: DNS_WAIT,
            }),
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
            &self.section,
```

换成

```rust
            &self.names.section,
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
    /// Where a connection to `target` goes: its address, or its name's.
    async fn address(
```

换成

```rust
    async fn dial(
        &self,
        target: &Target,
        opts: &ConnectOpts,
    ) -> Result<BoxedStream, OutboundError> {
        let device = self.device(opts).await?;
        let ip = self.names.address(target, &device).await?;
        let stream = device.connect(SocketAddr::new(ip, target.port)).await?;
        Ok(Box::new(stream))
    }

    /// `DNS_WAIT`, before anything shares the names (a test's).
    #[cfg(test)]
    fn set_dns_wait(&mut self, wait: Duration) {
        Arc::get_mut(&mut self.names)
            .expect("nothing shares the names yet")
            .dns_wait = wait;
    }
}

impl Names {
    /// Where a connection or a datagram to `target` goes: its address, or
    /// its name's.
    pub(crate) async fn address(
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
        dns::answered(v4.into_iter().chain(v6).collect())
    }

    async fn dial(
        &self,
        target: &Target,
        opts: &ConnectOpts,
    ) -> Result<BoxedStream, OutboundError> {
        let device = self.device(opts).await?;
        let ip = self.address(target, &device).await?;
        let stream = device.connect(SocketAddr::new(ip, target.port)).await?;
        Ok(Box::new(stream))
    }
```

换成

```rust
        dns::answered(v4.into_iter().chain(v6).collect())
    }
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
        }))
    }
}
```

换成

```rust
        }))
    }

    fn udp(&self) -> UdpSupport {
        UdpSupport::Native
    }

    /// A socket of the tunnel's stack for every destination (full cone,
    /// phase 2 M5 design §7); the tunnel starts first when it has not.
    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedPacketSocket, OutboundError>> {
        Box::pin(async move {
            let device = match tokio::time::timeout(opts.timeout, self.device(opts)).await {
                Ok(device) => device?,
                Err(_) => return Err(OutboundError::Timeout),
            };
            Ok(Box::new(TunnelUdp::new(device, self.names.clone())) as BoxedPacketSocket)
        })
    }
}
```

要点：
- 协议栈的锁里只做内存操作：`send_to` 先在锁外解析目标，锁里检查路由、按需绑定 socket、写进 socket 的缓冲，出锁后 `kick` 设备去发。
- 新开 socket 时唤醒等着收包的一方：它此前只在已有的 socket 上登记了 waker，不唤醒就收不到新 socket 上的回包。
- `BufferFull` 当作已发出（丢包，同一个满了的 socket）；别的拒绝以错误返回，引擎据此让这条流失败。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto-wireguard --lib outbound` → 通过（新增 `udp_goes_through_the_tunnel`、`anyone_in_the_tunnel_may_answer`、`a_datagram_to_a_name_goes_where_the_name_says`、`udp_where_the_tunnel_cannot_go_is_refused`）。
Run: `cargo test -p rurge-engine --test outbounds_wireguard` → 通过（新增 `udp_leaves_through_the_tunnel`）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-proto-wireguard crates/rurge-engine/tests/outbounds_wireguard.rs
git commit -m "feat(wireguard): 隧道里的 UDP——每个地址族一个 socket、全锥；名字解析抽成共享的 Names；假对端的 UDP 回显"
```

### Task 2: 链上的 UDP 与经 `underlying-proxy` 的隧道

`ChainConnector::connect_udp`（M5a 延后事项 #3，P3）让 `wireguard` 的 peer 载体经底层策略的 UDP 出去；底层策略不载 UDP 时拨号以说明失败（P4），`wireguard` 专用的 `W0029` 退役。顺带 M4b #15："the peer cannot be reached" 限频（P5）。

**Files:**
- Create: `crates/rurge-net/src/packet_datagram.rs`（与用例）
- Modify: `crates/rurge-net/src/lib.rs`、`crates/rurge-policy/src/cell.rs`、`crates/rurge-proto-wireguard/src/device.rs`（与新的用例模块）、`src/outbound.rs`（用例）、`crates/rurge-config/src/spec/mod.rs`（与用例）、`crates/rurge-config/tests/snapshots/corpus__corpus__kitchen-sink.snap`、`crates/rurge-engine/tests/outbounds_wireguard.rs`

**Interfaces:**
- Consumes: M5a 的 `ChainConnector::open_udp`、`PacketSocket` / `Datagram`；既有的 `Resolve`、`Target`。
- Produces:
  - `pub fn rurge_net::packet_datagram::packet_datagram(socket: BoxedPacketSocket, to: Target) -> BoxedDatagram`
  - `ChainConnector` 的 `Connector::connect_udp`：`open_udp` + 解析 `target` + `packet_datagram`；底层不载 UDP 时的错误是 `via <底层策略>: the underlying policy cannot carry UDP`（`io::Error`，由 `open_udp` 给出）
  - `device.rs`：`const UNREACHABLE_EVERY: Duration`（300 秒）与 `fn may_say_unreachable(policy: &str, peer: usize, now: Instant) -> bool`；`unreachable()` 删除，载体错误经 `OutboundError::from(io::Error)` 原样上报

- [ ] **Step 1: 先写用例**

配置：`underlying-proxy` 写在 `wireguard` 上不再告警（快照里那一行随之消失）：

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    /// No UDP through a chain before M5: the policy loads with a warning and
    /// rejects (M4-D7). `DIRECT` is no chain.
    #[test]
    fn underlying_proxy_on_a_wireguard_policy_is_warned_about() {
        let o = outcome("W", "wireguard, section-name=home, underlying-proxy=Entry");
        assert!(o.spec.is_some());
        let found: Vec<(&str, &str)> = o
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str()))
            .collect();
        assert_eq!(
            found,
            [(
                codes::W_PARAM_NOT_EFFECTIVE,
                "policy `W`: `underlying-proxy` does not work with `wireguard` policies in this version; the policy rejects every connection"
            )]
        );
        let o = outcome("W", "wireguard, section-name=home, underlying-proxy=DIRECT");
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
```

换成

```rust
    /// A chain carries UDP since M5: a `wireguard` policy over one loads
    /// without a word, its peers reached through the chain (phase 2 M5
    /// design 8.1). `DIRECT` is no chain.
    #[test]
    fn underlying_proxy_on_a_wireguard_policy_is_accepted() {
        for line in [
            "wireguard, section-name=home, underlying-proxy=Entry",
            "wireguard, section-name=home, underlying-proxy=DIRECT",
        ] {
            let o = outcome("W", line);
            assert!(o.spec.is_some());
            assert!(o.diagnostics.is_empty(), "{line}: {:?}", o.diagnostics);
        }
```

`crates/rurge-config/tests/snapshots/corpus__corpus__kitchen-sink.snap`——把

```text
  - "warning[W0029] valid/kitchen-sink.conf:53: policy parameter `tfo` is parsed but has no effect in this version"
  - "warning[W0029] valid/kitchen-sink.conf:69: policy `WG`: `underlying-proxy` does not work with `wireguard` policies in this version; the policy rejects every connection"
```

换成

```text
  - "warning[W0029] valid/kitchen-sink.conf:53: policy parameter `tfo` is parsed but has no effect in this version"
```

出站库：经链的策略不共用直连的隧道，底层不载 UDP 时错误是 `Io` 而不再是 `Unsupported`：

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
    /// with the chain's usual refusal — the other policy's tunnel keeps
    /// running untouched.
```

换成

```rust
    /// with what the chain said — the other policy's tunnel keeps running
    /// untouched.
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
        assert!(matches!(e, OutboundError::Unsupported(_)), "{e}");
        assert_eq!(
            e.to_string(),
            "policy protocol not implemented: wireguard over underlying-proxy"
        );
```

换成

```rust
        assert!(matches!(e, OutboundError::Io(_)), "{e}");
        assert_eq!(e.to_string(), "this connection cannot carry UDP");
```

经引擎的用例（经 `socks5` 底层策略的隧道；底层是 `http` 时失败并说明）：

`crates/rurge-engine/tests/outbounds_wireguard.rs`——把

```rust
/// No UDP through a chain before M5: the policy rejects and says why, and
/// never goes around the chain (M4-D7).
#[tokio::test]
async fn a_tunnel_over_underlying_proxy_rejects_with_a_note() {
    let (peer, section) = peer().await;
    let h = harness(Profile {
        proxies: "WG = wireguard, section-name=w, underlying-proxy=Up\nUp = socks5, 127.0.0.1, 9",
```

换成

```rust
/// The peers are reached through the `underlying-proxy`: here a SOCKS5
/// proxy's UDP ASSOCIATE (phase 2 M5 design 8.1).
#[tokio::test]
async fn a_tunnel_goes_over_an_underlying_socks5_proxy() {
    let (peer, section) = peer().await;
    let up = FakeSocks5::spawn(Socks5Script::default()).await;
    let proxies = format!(
        "WG = wireguard, section-name=w, underlying-proxy=Up\nUp = socks5, 127.0.0.1, {}, udp-relay=true",
        up.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        rules: TO_WG,
        sections: &section,
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "10.0.0.1:7").await;
    echo_through(&mut tunnel, b"over the chain").await;
    assert_eq!(peer.core().connected_to, [echo_addr()]);
    let peer_at =
        rurge_net::connector::Target::new(HostName::parse("127.0.0.1"), peer.addr().port());
    assert!(
        up.datagrams().iter().all(|to| *to == peer_at),
        "every datagram went to the peer through the proxy"
    );
    assert!(!up.datagrams().is_empty());
}

/// An `underlying-proxy` that carries no UDP fails the dial, saying so, and
/// never goes around the chain (M4-D7).
#[tokio::test]
async fn a_tunnel_over_an_underlying_proxy_without_udp_fails_saying_so() {
    let (peer, section) = peer().await;
    let h = harness(Profile {
        proxies: "WG = wireguard, section-name=w, underlying-proxy=Up\nUp = http, 127.0.0.1, 9",
```

`crates/rurge-engine/tests/outbounds_wireguard.rs`——把

```rust
        Err(DialError::Reject { kind, handle, .. }) => {
            assert_eq!(kind, rurge_proto::RejectKind::Reject);
            assert_eq!(
                handle.error().as_deref(),
                Some("policy protocol not implemented: wireguard over underlying-proxy")
            );
        }
        Err(DialError::Failed { message, .. }) => panic!("expected a reject, failed: {message}"),
        Ok(_) => panic!("expected a reject, got a stream"),
```

换成

```rust
        Err(DialError::Failed { message, .. }) => {
            assert_eq!(message, "via Up: the underlying policy cannot carry UDP");
        }
        Err(DialError::Reject { .. }) => panic!("expected a failure, got a reject"),
        Ok(_) => panic!("expected a failure, got a stream"),
```

`crates/rurge-engine/tests/outbounds_wireguard.rs`——把

```rust
/// carriers are alike: the one over `underlying-proxy` still rejects, and
/// the tunnel the other one started goes on (M4-D7).
```

换成

```rust
/// carriers are alike: the one over an `underlying-proxy` without UDP
/// fails, and the tunnel the other one started goes on (M4-D7).
```

`crates/rurge-engine/tests/outbounds_wireguard.rs`——把

```rust
        Err(DialError::Reject { handle, .. }) => assert_eq!(
            handle.error().as_deref(),
            Some("policy protocol not implemented: wireguard over underlying-proxy")
        ),
        Err(DialError::Failed { message, .. }) => panic!("expected a reject, failed: {message}"),
        Ok(_) => panic!("expected a reject, got a stream"),
```

换成

```rust
        Err(DialError::Failed { message, .. }) => {
            assert_eq!(message, "via Up: the underlying policy cannot carry UDP")
        }
        Err(DialError::Reject { .. }) => panic!("expected a failure, got a reject"),
        Ok(_) => panic!("expected a failure, got a stream"),
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-engine --test outbounds_wireguard`
Expected: FAIL——经底层策略的隧道仍按 REJECT 处理（`outbounds_wireguard.rs:112` / `:140` 是 `panic!("expected a failure, got a reject")`；经 `socks5` 的那条连 CONNECT 的应答都等不到）：

```text
test a_tunnel_over_an_underlying_proxy_without_udp_fails_saying_so ... FAILED
test a_tunnel_goes_over_an_underlying_socks5_proxy ... FAILED
test a_policy_over_underlying_proxy_never_shares_the_tunnel ... FAILED
thread 'a_tunnel_over_an_underlying_proxy_without_udp_fails_saying_so' panicked at crates\rurge-engine\tests\outbounds_wireguard.rs:112:42:
thread 'a_tunnel_goes_over_an_underlying_socks5_proxy' panicked at crates\rurge-engine\tests\common\mod.rs:175:9:
thread 'a_policy_over_underlying_proxy_never_shares_the_tunnel' panicked at crates\rurge-engine\tests\outbounds_wireguard.rs:140:42:
test result: FAILED. 7 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.09s
error: test failed, to rerun pass `-p rurge-engine --test outbounds_wireguard`
exit 101
```

Run: `cargo test -p rurge-config --lib spec`
Expected: FAIL——`W0029` 还在：

```text
test spec::tests::underlying_proxy_on_a_wireguard_policy_is_accepted ... FAILED
thread 'spec::tests::underlying_proxy_on_a_wireguard_policy_is_accepted' panicked at crates\rurge-config\src\spec\mod.rs:541:13:
test result: FAILED. 85 passed; 1 failed; 0 ignored; 0 measured; 122 filtered out; finished in 0.01s
error: test failed, to rerun pass `-p rurge-config --lib`
exit 101
```

Run: `cargo test -p rurge-proto-wireguard --lib outbound`
Expected: FAIL——错误仍被改写成 `Unsupported`：

```text
test outbound::tests::a_policy_over_a_chain_never_shares_a_direct_tunnel ... FAILED
thread 'outbound::tests::a_policy_over_a_chain_never_shares_a_direct_tunnel' panicked at crates\rurge-proto-wireguard\src\outbound.rs:1435:9:
test result: FAILED. 33 passed; 1 failed; 1 ignored; 0 measured; 19 filtered out; finished in 5.32s
error: test failed, to rerun pass `-p rurge-proto-wireguard --lib`
exit 101
```

- [ ] **Step 3: 实现**

新模块（自带用例）：

新建 `crates/rurge-net/src/packet_datagram.rs`：

```rust
//! A packet carrier used as a datagram to one peer (phase 2 M5 design 4.3):
//! what a `wireguard` tunnel sends to a peer through an `underlying-proxy`
//! chain. Everything it sends goes to the peer; it takes only what comes from
//! the peer, when the peer is known by address — a carrier that hands names
//! to its server (SOCKS5) cannot tell, and then everything is taken.

use crate::connector::{BoxedDatagram, BoxedPacketSocket, Datagram, PacketSocket, Target};
use rurge_config::HostName;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tokio::io::ReadBuf;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Datagrams waiting either way; more are dropped, as a full socket drops them.
const QUEUE: usize = 256;

struct PacketDatagram {
    to: Target,
    outgoing: mpsc::Sender<Vec<u8>>,
    incoming: Mutex<mpsc::Receiver<Vec<u8>>>,
    tasks: [JoinHandle<()>; 2],
}

/// `socket` as a datagram to `to`, which it has resolved (`PacketSocket::resolve`).
pub fn packet_datagram(socket: BoxedPacketSocket, to: Target) -> BoxedDatagram {
    let socket: Arc<dyn PacketSocket> = Arc::from(socket);
    let (outgoing, mut queued) = mpsc::channel::<Vec<u8>>(QUEUE);
    let (arrived, incoming) = mpsc::channel::<Vec<u8>>(QUEUE);
    let (sender, peer) = (socket.clone(), to.clone());
    let send = tokio::spawn(async move {
        while let Some(datagram) = queued.recv().await {
            if let Err(e) = sender.send_to(&datagram, &peer).await {
                tracing::trace!(error = %e, "a datagram through the chain was not sent");
            }
        }
    });
    let from_peer = match &to.host {
        HostName::Ip(_) => Some(to.clone()),
        HostName::Domain(_) => None,
    };
    let receive = tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        // the carrier's end is the datagram's: `arrived` drops with it
        while let Ok((n, from)) = socket.recv_from(&mut buf).await {
            if from_peer.as_ref().is_none_or(|peer| *peer == from) {
                let _ = arrived.try_send(buf[..n].to_vec());
            }
        }
    });
    Box::new(PacketDatagram {
        to,
        outgoing,
        incoming: Mutex::new(incoming),
        tasks: [send, receive],
    })
}

impl Datagram for PacketDatagram {
    fn poll_send(&self, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(match self.outgoing.try_send(buf.to_vec()) {
            // a full queue drops it, as a full socket would
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => Ok(buf.len()),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(io::ErrorKind::BrokenPipe.into()),
        })
    }

    fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let mut incoming = self.incoming.lock().expect("the incoming datagrams");
        match incoming.poll_recv(cx) {
            Poll::Ready(Some(datagram)) => {
                let n = datagram.len().min(buf.remaining());
                buf.put_slice(&datagram[..n]);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(None) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the carrier through the chain has closed",
            ))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn peer_addr(&self) -> Option<SocketAddr> {
        match self.to.host {
            HostName::Ip(ip) => Some(SocketAddr::new(ip, self.to.port)),
            HostName::Domain(_) => None,
        }
    }
}

impl Drop for PacketDatagram {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BoxFuture;
    use std::future::poll_fn;
    use std::time::Duration;

    /// Datagrams with where they came from or went.
    type Log = Arc<Mutex<Vec<(Target, Vec<u8>)>>>;

    /// A carrier whose datagrams come from a script, and which notes what
    /// it sends.
    struct Scripted {
        arriving: tokio::sync::Mutex<mpsc::Receiver<(Target, Vec<u8>)>>,
        sent: Log,
    }

    impl PacketSocket for Scripted {
        fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
            self.sent.lock().unwrap().push((to.clone(), buf.to_vec()));
            Box::pin(std::future::ready(Ok(())))
        }

        fn recv_from<'a>(
            &'a self,
            buf: &'a mut [u8],
        ) -> BoxFuture<'a, io::Result<(usize, Target)>> {
            Box::pin(async move {
                match self.arriving.lock().await.recv().await {
                    Some((from, data)) => {
                        buf[..data.len()].copy_from_slice(&data);
                        Ok((data.len(), from))
                    }
                    None => Err(io::ErrorKind::BrokenPipe.into()),
                }
            })
        }
    }

    type Script = (mpsc::Sender<(Target, Vec<u8>)>, Log);

    fn scripted() -> (BoxedPacketSocket, Script) {
        let (tx, rx) = mpsc::channel(16);
        let sent = Arc::new(Mutex::new(Vec::new()));
        let socket = Scripted {
            arriving: tokio::sync::Mutex::new(rx),
            sent: sent.clone(),
        };
        (Box::new(socket), (tx, sent))
    }

    fn at(host: &str, port: u16) -> Target {
        Target::new(HostName::parse(host), port)
    }

    async fn recv(datagram: &BoxedDatagram) -> io::Result<Vec<u8>> {
        let mut storage = [0u8; 1500];
        let mut buf = ReadBuf::new(&mut storage);
        tokio::time::timeout(
            Duration::from_secs(5),
            poll_fn(|cx| datagram.poll_recv(cx, &mut buf)),
        )
        .await
        .expect("an outcome within the bound")?;
        Ok(buf.filled().to_vec())
    }

    /// What it sends goes to the peer; of what arrives, a peer known by
    /// address is heard alone.
    #[tokio::test]
    async fn a_peer_by_address_is_heard_alone() {
        let (socket, (arrive, sent)) = scripted();
        let peer = at("192.0.2.7", 51820);
        let datagram = packet_datagram(socket, peer.clone());
        assert_eq!(
            datagram.peer_addr(),
            Some("192.0.2.7:51820".parse().unwrap())
        );
        poll_fn(|cx| datagram.poll_send(cx, b"out")).await.unwrap();
        arrive
            .send((at("198.51.100.1", 53), b"stranger".to_vec()))
            .await
            .unwrap();
        arrive.send((peer.clone(), b"back".to_vec())).await.unwrap();
        assert_eq!(recv(&datagram).await.unwrap(), b"back");
        assert_eq!(*sent.lock().unwrap(), [(peer, b"out".to_vec())]);
    }

    /// A peer known by name (the chain's server resolves it) cannot be told
    /// from others: everything is heard. The carrier's end is the
    /// datagram's.
    #[tokio::test]
    async fn a_peer_by_name_hears_everything_until_the_carrier_ends() {
        let (socket, (arrive, _sent)) = scripted();
        let datagram = packet_datagram(socket, at("wg.example", 51820));
        assert_eq!(datagram.peer_addr(), None);
        arrive
            .send((at("192.0.2.7", 51820), b"any".to_vec()))
            .await
            .unwrap();
        assert_eq!(recv(&datagram).await.unwrap(), b"any");
        drop(arrive);
        assert_eq!(
            recv(&datagram).await.unwrap_err().to_string(),
            "the carrier through the chain has closed"
        );
    }
}
```

`crates/rurge-net/src/lib.rs`——把

```rust
pub mod http;
```

换成

```rust
pub mod http;
pub mod packet_datagram;
```

链式连接器：

`crates/rurge-policy/src/cell.rs`——把

```rust
use rurge_net::connector::{BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, Target};
```

换成

```rust
use rurge_net::connector::{
    BoxedDatagram, BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, Target,
};
use rurge_net::packet_datagram::packet_datagram;
```

`crates/rurge-policy/src/cell.rs`——把

```rust
                .open_udp(opts)
                .await
                .map_err(|e| self.via(e))
        })
    }
```

换成

```rust
                .open_udp(opts)
                .await
                .map_err(|e| self.via(e))
        })
    }

    /// The underlying policy's UDP carrier as a datagram to `target` (phase
    /// 2 M5 design 4.3, 8.1): a `wireguard` peer through this hop. `target`
    /// is resolved the carrier's way, once.
    fn connect_udp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
        Box::pin(async move {
            let socket = self.open_udp(opts).await?;
            let to = socket.resolve(target).await?;
            Ok(packet_datagram(socket, to))
        })
    }
```

设备：载体错误原样上报、告警限频（文件末尾新增的用例模块测的正是本步加的 `may_say_unreachable`，随实现一起写）：

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
use smoltcp::iface::SocketHandle;
```

换成

```rust
use smoltcp::iface::SocketHandle;
use std::collections::HashMap;
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
const SENDS: u32 = 3;
```

换成

```rust
const SENDS: u32 = 3;
/// How often at most a policy says that a peer cannot be reached; more
/// often it only tells the debug log (M4b deferred item #15).
const UNREACHABLE_EVERY: Duration = Duration::from_secs(300);

/// When each policy last said that each peer cannot be reached.
static UNREACHABLE_SAID: Mutex<Option<HashMap<(String, usize), Instant>>> = Mutex::new(None);

/// Whether a policy may say again that `peer` cannot be reached.
fn may_say_unreachable(policy: &str, peer: usize, now: Instant) -> bool {
    let mut said = UNREACHABLE_SAID.lock().expect("the unreachable warnings");
    let said = said.get_or_insert_with(HashMap::new);
    let key = (policy.to_string(), peer);
    if said
        .get(&key)
        .is_some_and(|at| now.duration_since(*at) < UNREACHABLE_EVERY)
    {
        return false;
    }
    said.insert(key, now);
    true
}
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
    marks: bool,
}

/// What a peer's carrier failing to come up means for the dial.
fn unreachable(e: io::Error) -> OutboundError {
    if e.kind() == io::ErrorKind::Unsupported {
        // a chain carries no UDP before M5 (M4-D7)
        OutboundError::Unsupported("wireguard over underlying-proxy".to_string())
    } else {
        OutboundError::from(e)
    }
}

impl Device {
```

换成

```rust
    marks: bool,
}

impl Device {
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
                    tracing::warn!(policy, peer = i + 1, error = %e, "wireguard: the peer cannot be reached");
```

换成

```rust
                    if may_say_unreachable(policy, i, Instant::now()) {
                        tracing::warn!(policy, peer = i + 1, error = %e, "wireguard: the peer cannot be reached");
                    } else {
                        tracing::debug!(policy, peer = i + 1, error = %e, "wireguard: the peer cannot be reached");
                    }
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
            return Err(unreachable(failure.unwrap_or_else(|| {
```

换成

```rust
            // an `underlying-proxy` that carries no UDP says so itself
            return Err(OutboundError::from(failure.unwrap_or_else(|| {
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
                _ = tokio::time::sleep_until(deadline.into()) => {}
            }
        }
    }
}

```

换成

```rust
                _ = tokio::time::sleep_until(deadline.into()) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A policy says a peer cannot be reached once every five minutes at
    /// most; each policy and peer on its own (M4b deferred item #15).
    #[test]
    fn a_peer_that_cannot_be_reached_is_said_so_once_in_a_while() {
        let now = Instant::now();
        assert!(may_say_unreachable("throttle-test", 0, now));
        assert!(!may_say_unreachable(
            "throttle-test",
            0,
            now + Duration::from_secs(299)
        ));
        assert!(may_say_unreachable("throttle-test", 1, now));
        assert!(may_say_unreachable("throttle-test-2", 0, now));
        assert!(may_say_unreachable(
            "throttle-test",
            0,
            now + UNREACHABLE_EVERY
        ));
    }
}

```

配置：去掉 `wireguard` 的 `underlying-proxy` 告警：

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        check_underlying(&mut r, &mut common, env);
        // a chain carries no UDP before M5: the tunnel cannot start (M4-D7)
        if matches!(proto, ProtoSpec::WireGuard(_)) && common.underlying_proxy.is_some() {
            r.warn(
                codes::W_PARAM_NOT_EFFECTIVE,
                "`underlying-proxy` does not work with `wireguard` policies in this version; the policy rejects every connection".to_string(),
            );
        }
```

换成

```rust
        check_underlying(&mut r, &mut common, env);
```

要点：
- `packet_datagram` 的收发两个任务在 `Datagram` 释放时中止（`Drop` 里 `abort`）；队列满时丢包、不阻塞发送方；发送失败只记 `trace!`（同一个 socket 发不出去时的做法）。
- 目标是名字时不按来源过滤回包：名字目标的回包来源是解析出的地址，比不上。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-net packet_datagram` → 通过（`a_peer_by_address_is_heard_alone`、`a_peer_by_name_hears_everything_until_the_carrier_ends`）。
Run: `cargo test -p rurge-proto-wireguard --lib` → 通过（含 `device::tests::a_peer_that_cannot_be_reached_is_said_so_once_in_a_while`）。
Run: `cargo test -p rurge-config` → 通过（含 `underlying_proxy_on_a_wireguard_policy_is_accepted` 与语料库快照）。
Run: `cargo test -p rurge-engine --test outbounds_wireguard` → 通过（新增 `a_tunnel_goes_over_an_underlying_socks5_proxy`、`a_tunnel_over_an_underlying_proxy_without_udp_fails_saying_so`）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-net crates/rurge-policy/src/cell.rs crates/rurge-proto-wireguard crates/rurge-config crates/rurge-engine/tests/outbounds_wireguard.rs
git commit -m "feat(policy): 链上的 UDP（packet_datagram）——wireguard 经 underlying-proxy 建隧道，底层不载 UDP 时失败并说明；peer 连不上的告警限频"
```

### Task 3: UDP 测试（`test-udp` / `proxy-test-udp`）

经策略的 UDP 向 `hostname@ipv4` 的 53 端口问一次 A 记录（P8）；`Engine::test_udp` 与 `POST /v1/policies/test` 结果里的 `udp` 键（P7）。`test-udp` 从 `W0029` 的"解析但不生效"名单里去掉。经引擎的用例让一条 `wireguard` 策略问假对端隧道里的名字服务器（`10.0.0.53:53`）——测试只能问 53 端口，回环上的假服务器没法用这个端口，隧道里的可以。

**Files:**
- Create: `crates/rurge-policy/src/udp_probe.rs`（与用例）
- Modify: `crates/rurge-policy/src/lib.rs`、`crates/rurge-engine/src/auto.rs`、`crates/rurge-api/src/routes/policies.rs`、`crates/rurge-config/src/spec/common.rs`（与用例）、`crates/rurge-config/tests/policy_spec.rs`、`crates/rurge-engine/tests/outbounds_wireguard.rs`

**Interfaces:**
- Consumes: 既有的 `rurge_config::general::UdpTest`（`hostname`、`server: Ipv4Addr`）、`CommonOpts::test_udp`、`General::proxy_test_udp`、`PolicyRegistry::{test_case, spec}` 与 `TestCase { outbound, timeout, .. }`、`Engine::snapshot`；Task 1 的 `wireguard` UDP。
- Produces:
  - `pub async fn rurge_policy::udp_probe::probe_udp(outbound: &OutboundRef, test: &UdpTest, timeout: Duration) -> Result<Duration, String>`；`pub(crate) async fn probe_udp_at(outbound, hostname: &str, server: SocketAddr, timeout)`；`pub(crate) fn question(id: u16, hostname: &str) -> Option<Vec<u8>>`、`pub(crate) fn answers(datagram: &[u8], id: u16) -> bool`
  - `pub async fn Engine::test_udp(&self, names: &[String]) -> Vec<(String, Option<Result<Duration, String>>)>`：与 `names` 同序；`None` = 这个策略没有 UDP 测试（没写、不载 UDP、不能测）
  - API：`POST /v1/policies/test` 每个结果可多一个 `"udp": {"delay": <ms>}` 或 `"udp": {"error": "<原因>"}`

- [ ] **Step 1: 先写用例**

配置：`test-udp` 不再是"解析但不生效"（`policy_spec` 里原来拿它当例子的那条改用仍不生效的 `ecn`）：

`crates/rurge-config/src/spec/common.rs`——把

```rust
        assert_eq!(
            notes.inert,
            ["dns-follow-interface", "tfo", "test-udp", "ecn"]
        );
```

换成

```rust
        assert_eq!(notes.inert, ["dns-follow-interface", "tfo", "ecn"]);
```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
        "A = socks5, a.example, 1080, test-udp=apple.com@8.8.8.8, tfo=true\nB = socks5, b.example, 1080, test-udp=apple.com@8.8.8.8, hybrid=on\nC = http, c.example, 80, hybrid=off",
```

换成

```rust
        "A = socks5, a.example, 1080, ecn=on, tfo=true\nB = socks5, b.example, 1080, ecn=on, hybrid=on\nC = http, c.example, 80, hybrid=off",
```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
                "policy parameter `test-udp` is parsed but has no effect in this version"
                    .to_string(),
```

换成

```rust
                "policy parameter `ecn` is parsed but has no effect in this version".to_string(),
```

经引擎的用例（经隧道问名字服务器；`http` 策略与 `DIRECT` 没有 UDP 测试）：

`crates/rurge-engine/tests/outbounds_wireguard.rs`——把

```rust
    wait_until("the flow to finish", || !log.recent(10).is_empty()).await;
    assert_eq!(log.recent(10)[0].policy, ["WG"]);
}

```

换成

```rust
    wait_until("the flow to finish", || !log.recent(10).is_empty()).await;
    assert_eq!(log.recent(10)[0].policy, ["WG"]);
}

/// The UDP test (`test-udp`) asks its question through the tunnel; a policy
/// without UDP, or without a UDP test, has none (phase 2 M5 design 8.4).
#[tokio::test]
async fn a_udp_test_asks_through_the_tunnel() {
    let (peer, section) = peer().await;
    let h = harness(Profile {
        proxies: "WG = wireguard, section-name=w, test-udp=echo.test@10.0.0.53
H = http, 127.0.0.1, 9",
        sections: &section,
        ..Profile::default()
    })
    .await;
    let names = ["WG", "H", "DIRECT"].map(String::from);
    let results = h.engine.test_udp(&names).await;
    let outcome = |name: &str| results.iter().find(|(n, _)| n == name).unwrap().1.clone();
    assert!(matches!(outcome("WG"), Some(Ok(_))), "{:?}", outcome("WG"));
    assert_eq!(outcome("H"), None, "an http proxy carries no UDP");
    assert_eq!(outcome("DIRECT"), None, "no proxy-test-udp");
    assert_eq!(peer.core().dns_questions, ["echo.test A"]);
}

```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-engine --test outbounds_wireguard udp_test`
Expected: FAIL——`Engine::test_udp` 由 Step 3 引入，编译不过：

```text
error[E0599]: no method named `test_udp` found for struct `std::sync::Arc<Engine>` in the current scope
   --> crates\rurge-engine\tests\outbounds_wireguard.rs:326:28
For more information about this error, try `rustc --explain E0599`.
error: could not compile `rurge-engine` (test "outbounds_wireguard") due to 1 previous error
exit 101
```

Run: `cargo test -p rurge-config --lib common`
Expected: FAIL——`test-udp` 仍在不生效名单里：

```text
test spec::common::tests::every_common_parameter_is_parsed ... FAILED
thread 'spec::common::tests::every_common_parameter_is_parsed' panicked at crates\rurge-config\src\spec\common.rs:262:9:
assertion `left == right` failed
  left: ["dns-follow-interface", "tfo", "test-udp", "ecn"]
 right: ["dns-follow-interface", "tfo", "ecn"]
test result: FAILED. 6 passed; 1 failed; 0 ignored; 0 measured; 201 filtered out; finished in 0.00s
error: test failed, to rerun pass `-p rurge-config --lib`
exit 101
```

- [ ] **Step 3: 实现**

新模块（自带用例）：

新建 `crates/rurge-policy/src/udp_probe.rs`：

```rust
//! The UDP test (phase 2 M5 design 8.4; the manual's `test-udp` /
//! `proxy-test-udp`): one DNS question for `hostname`'s A records, through
//! the policy's own UDP carrier, to the server's port 53. Any answer to it
//! passes — a name the server does not know too; the result is how long the
//! answer took. It is shown, never kept: the groups pick by the TCP tests
//! alone (the manual: the latency test measures TCP).

use rurge_config::HostName;
use rurge_config::general::UdpTest;
use rurge_net::connector::{ConnectOpts, Target};
use rurge_proto::{OutboundRef, UdpSupport};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Where a UDP test asks.
const DNS_PORT: u16 = 53;

/// A question's ID: different from the last one's.
fn next_id() -> u16 {
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos()) as u16;
    seed ^ NEXT.fetch_add(0x9e37, Ordering::Relaxed)
}

/// The question for `hostname`'s A records, with `id`; `None` for a name no
/// question can hold.
pub(crate) fn question(id: u16, hostname: &str) -> Option<Vec<u8>> {
    let name = hostname.trim_end_matches('.');
    if name.is_empty() || name.len() > 253 || !name.is_ascii() {
        return None;
    }
    // ID, recursion desired, one question
    let mut out = id.to_be_bytes().to_vec();
    out.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
    for label in name.split('.') {
        let len = u8::try_from(label.len())
            .ok()
            .filter(|l| (1..=63).contains(l))?;
        out.push(len);
        out.extend_from_slice(label.as_bytes());
    }
    // the root, then A, IN
    out.extend_from_slice(&[0, 0, 1, 0, 1]);
    Some(out)
}

/// Whether `datagram` answers the question `id`.
pub(crate) fn answers(datagram: &[u8], id: u16) -> bool {
    datagram.len() >= 12 && datagram[..2] == id.to_be_bytes() && datagram[2] & 0x80 != 0
}

/// The UDP test of `outbound` at `test`, within `timeout`.
pub async fn probe_udp(
    outbound: &OutboundRef,
    test: &UdpTest,
    timeout: Duration,
) -> Result<Duration, String> {
    let server = SocketAddr::new(IpAddr::V4(test.server), DNS_PORT);
    probe_udp_at(outbound, &test.hostname, server, timeout).await
}

/// `probe_udp` to a server on any port (the tests' loopback servers).
pub(crate) async fn probe_udp_at(
    outbound: &OutboundRef,
    hostname: &str,
    server: SocketAddr,
    timeout: Duration,
) -> Result<Duration, String> {
    if outbound.udp() == UdpSupport::Unsupported {
        return Err("the policy carries no UDP".to_string());
    }
    let id = next_id();
    let question =
        question(id, hostname).ok_or_else(|| format!("`{hostname}` cannot be asked for"))?;
    let to = Target::new(HostName::Ip(server.ip()), server.port());
    let exchange = async {
        let carrier = outbound
            .open_udp(&ConnectOpts { timeout })
            .await
            .map_err(|e| e.to_string())?;
        let sent = Instant::now();
        carrier
            .send_to(&question, &to)
            .await
            .map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 65536];
        loop {
            let (n, from) = carrier
                .recv_from(&mut buf)
                .await
                .map_err(|e| e.to_string())?;
            if from == to && answers(&buf[..n], id) {
                return Ok(sent.elapsed());
            }
        }
    };
    match tokio::time::timeout(timeout, exchange).await {
        Ok(outcome) => outcome,
        Err(_) => Err("udp test timed out".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_proto::{Direct, Reject, RejectKind};
    use std::sync::Arc;
    use tokio::net::UdpSocket;

    fn direct() -> OutboundRef {
        Arc::new(Direct::new(Arc::new(DirectConnector::new(Arc::new(
            SystemResolve,
        )))))
    }

    /// A name server on the loopback: answers every question after a wrong
    /// ID first, or never (`silent`).
    async fn server(silent: bool) -> SocketAddr {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 512];
            while let Ok((n, from)) = socket.recv_from(&mut buf).await {
                if silent {
                    continue;
                }
                let mut answer = buf[..n].to_vec();
                answer[2] |= 0x80;
                let mut other = answer.clone();
                other[0] ^= 0xff;
                let _ = socket.send_to(&other, from).await;
                let _ = socket.send_to(&answer, from).await;
            }
        });
        addr
    }

    #[test]
    fn the_question_asks_for_a_records() {
        assert_eq!(
            question(0x1234, "a.bc").unwrap(),
            [
                0x12, 0x34, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 1, b'a', 2, b'b', b'c', 0, 0, 1, 0, 1
            ]
        );
        assert_eq!(question(1, "apple.com.").unwrap().len(), 12 + 11 + 4);
        for bad in ["", "a..b", "bücher.example", &"a".repeat(64)] {
            assert_eq!(question(1, bad), None, "{bad}");
        }
        assert!(answers(
            &[0x12, 0x34, 0x81, 0x80, 0, 1, 0, 0, 0, 0, 0, 0],
            0x1234
        ));
        assert!(
            !answers(&[0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0], 0x1234),
            "a question"
        );
        assert!(!answers(
            &[0x12, 0x35, 0x81, 0x80, 0, 1, 0, 0, 0, 0, 0, 0],
            0x1234
        ));
    }

    /// The answer to our question counts, whatever came before it.
    #[tokio::test]
    async fn an_answer_is_timed() {
        let at = server(false).await;
        let took = probe_udp_at(&direct(), "apple.com", at, Duration::from_secs(5))
            .await
            .expect("an answer");
        assert!(took < Duration::from_secs(5));
    }

    /// No answer within the time is a failure, saying so; a policy without
    /// UDP is not asked at all.
    #[tokio::test]
    async fn silence_and_no_udp_fail() {
        let at = server(true).await;
        let started = Instant::now();
        let err = probe_udp_at(&direct(), "apple.com", at, Duration::from_millis(300))
            .await
            .unwrap_err();
        assert_eq!(err, "udp test timed out");
        assert!(started.elapsed() < Duration::from_secs(5));
        let reject: OutboundRef = Arc::new(Reject::new(RejectKind::Reject));
        assert_eq!(
            probe_udp_at(&reject, "apple.com", at, Duration::from_secs(1))
                .await
                .unwrap_err(),
            "the policy carries no UDP"
        );
    }
}
```

`crates/rurge-policy/src/lib.rs`——把

```rust
pub(crate) mod testing;
```

换成

```rust
pub(crate) mod testing;
pub mod udp_probe;
```

引擎：

`crates/rurge-engine/src/auto.rs`——把

```rust
use rurge_policy::{PolicyRegistry, Resolution};
```

换成

```rust
use rurge_policy::udp_probe::probe_udp;
use rurge_policy::{PolicyRegistry, Resolution};
use rurge_proto::UdpSupport;
```

`crates/rurge-engine/src/auto.rs`——把

```rust
        Ok(out)
    }

    /// Tests every member of `group` now, whatever its `interval`, and the
```

换成

```rust
        Ok(out)
    }

    /// The UDP test of each of `names` now (phase 2 M5 design 8.4), side by
    /// side, each within its policy's test timeout: `None` for a policy
    /// without one — no `test-udp` and no `proxy-test-udp`, no UDP, or not
    /// testable at all (a group, a REJECT). Nothing is kept, and no group
    /// picks by it.
    pub async fn test_udp(
        &self,
        names: &[String],
    ) -> Vec<(String, Option<Result<Duration, String>>)> {
        let (runtime, registry) = self.snapshot();
        let fallback = runtime.config.general.proxy_test_udp.clone();
        let tests: Vec<_> = names
            .iter()
            .map(|name| {
                let case = registry.test_case(name);
                let test = registry
                    .spec(name)
                    .and_then(|spec| spec.common.test_udp.clone())
                    .or_else(|| fallback.clone());
                tokio::spawn(async move {
                    let (case, test) = (case?, test?);
                    if case.outbound.udp() == UdpSupport::Unsupported {
                        return None;
                    }
                    Some(probe_udp(&case.outbound, &test, case.timeout).await)
                })
            })
            .collect();
        let mut out = Vec::with_capacity(names.len());
        for (name, test) in names.iter().zip(tests) {
            let outcome = test
                .await
                .unwrap_or_else(|_| Some(Err("the test did not finish".to_string())));
            out.push((name.clone(), outcome));
        }
        out
    }

    /// Tests every member of `group` now, whatever its `interval`, and the
```

API（与 URL 测试并行，结果并进同一个对象）：

`crates/rurge-api/src/routes/policies.rs`——把

```rust
/// `POST /v1/policies/test` (phase 2 M3 design 6.6): `{"<name>": Result…}`.
/// The manual gives no response sample: the shape is provisional.
```

换成

```rust
/// `POST /v1/policies/test` (phase 2 M3 design 6.6): `{"<name>": Result…}`,
/// with `udp` in a result when the policy has a UDP test (phase 2 M5 design
/// 8.4), run alongside. The manual gives no response sample: the shape is
/// provisional.
```

`crates/rurge-api/src/routes/policies.rs`——把

```rust
    let results = app
        .engine
        .test_policies(&body.policy_names, url)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let mut out = Map::new();
    for (name, result) in results {
        let result = match result {
```

换成

```rust
    let (results, udp) = tokio::join!(
        app.engine.test_policies(&body.policy_names, url),
        app.engine.test_udp(&body.policy_names)
    );
    let results = results.map_err(|e| ApiError::bad_request(e.to_string()))?;
    let mut out = Map::new();
    for ((name, result), (_, udp)) in results.into_iter().zip(udp) {
        let mut result = match result {
```

`crates/rurge-api/src/routes/policies.rs`——把

```rust
        };
```

换成

```rust
        };
        if let Some(udp) = udp {
            result["udp"] = match udp {
                Ok(delay) => json!({ "delay": delay.as_millis() as u64 }),
                Err(error) => json!({ "error": error }),
            };
        }
```

配置：

`crates/rurge-config/src/spec/common.rs`——把

```rust
            ("tfo", tfo),
            ("test-udp", test_udp.is_some()),
```

换成

```rust
            ("tfo", tfo),
```

要点：
- 每个策略的 UDP 测试各在一个任务里跑，彼此并行；任务 panic 时这一项是 `the test did not finish`，不拖垮整个请求。
- 只认来源是那个服务器、ID 相同、QR 位置位的报文；隧道里别的来源的包（全锥）照收不误、不算作答。
- 结果不写进 `TestBook`、不发布、不进请求记录（P7）。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-policy udp_probe` → 通过（`the_question_asks_for_a_records`、`an_answer_is_timed`、`silence_and_no_udp_fail`）。
Run: `cargo test -p rurge-engine --test outbounds_wireguard` → 通过（新增 `a_udp_test_asks_through_the_tunnel`）。
Run: `cargo test -p rurge-config` 与 `cargo test -p rurge-api` → 通过。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-policy crates/rurge-engine crates/rurge-api crates/rurge-config
git commit -m "feat(engine): UDP 测试——经策略的 UDP 问一次 A 记录，POST /v1/policies/test 的结果多一个 udp 键；test-udp 生效"
```

### Task 4: `smart` 计入 UDP

UDP 流经 `smart` 组时向 `SmartBook` 回报（P9，M3c 延后事项 #6）：载体打不开算该成员失败；载体就绪后计一次使用并挂上 M3c 的 `watch`——第一个回包是首字节，3 秒没有回包只在目标端口 53 / 443 时算失败。

**Files:**
- Modify: `crates/rurge-engine/src/smart.rs`（与用例）、`src/engine.rs`、`src/udp.rs`、`crates/rurge-engine/tests/smart.rs`

**Interfaces:**
- Consumes: M3c 的 `SmartBook::{report_failure, used}`、`Resolution.smart`（`SmartPick { group, member, .. }`）、`smart::watch`；M5a 的 UDP 流水线（`udp::open` 与流的 `SessionHandle`）。
- Produces: `pub(crate) fn smart::watch(handle: &Arc<SessionHandle>, book: Arc<SmartBook>, policy: &str, outbound: &OutboundRef, host: &str, silence: bool)`——`silence` 为 `false` 时不挂 3 秒计时；TCP 的调用处一律传 `true`

- [ ] **Step 1: 先写用例**

`watch` 的单元用例（既有调用补上 `true`，新增一条）：

`crates/rurge-engine/src/smart.rs`——把

```rust
        let (book, a, h) = (Arc::new(SmartBook::new()), outbound(), session());
        watch(&h, book.clone(), "A", &a, "a.test");
        h.mark_first_byte();
```

换成

```rust
        let (book, a, h) = (Arc::new(SmartBook::new()), outbound(), session());
        watch(&h, book.clone(), "A", &a, "a.test", true);
        h.mark_first_byte();
```

`crates/rurge-engine/src/smart.rs`——把

```rust
        let (book, a, h) = (Arc::new(SmartBook::new()), outbound(), session());
        watch(&h, book.clone(), "A", &a, "a.test");
        tokio::time::sleep(NO_RESPONSE + Duration::from_millis(10)).await;
```

换成

```rust
        let (book, a, h) = (Arc::new(SmartBook::new()), outbound(), session());
        watch(&h, book.clone(), "A", &a, "a.test", true);
        tokio::time::sleep(NO_RESPONSE + Duration::from_millis(10)).await;
```

`crates/rurge-engine/src/smart.rs`——把

```rust
        assert!(!h.is_finished());
    }

    /// A client that left before the three seconds were up takes the
```

换成

```rust
        assert!(!h.is_finished());
    }

    /// Where silence tells nothing (a UDP flow to a port that need not
    /// answer, phase 2 M5 design 8.3), three silent seconds are no failure;
    /// an answer still counts.
    #[tokio::test(start_paused = true)]
    async fn silence_that_tells_nothing_is_no_failure() {
        let (book, a, h) = (Arc::new(SmartBook::new()), outbound(), session());
        watch(&h, book.clone(), "A", &a, "a.test", false);
        tokio::time::sleep(NO_RESPONSE + Duration::from_millis(10)).await;
        assert_eq!(
            book.health("A", &a, Instant::now()),
            Health::Unknown { failures: 0 }
        );
        h.mark_first_byte();
        assert!(matches!(
            book.health("A", &a, Instant::now()),
            Health::Healthy(_)
        ));
    }

    /// A client that left before the three seconds were up takes the
```

`crates/rurge-engine/src/smart.rs`——把

```rust
        let (book, a, h) = (Arc::new(SmartBook::new()), outbound(), session());
        watch(&h, book.clone(), "A", &a, "a.test");
        tokio::time::sleep(Duration::from_secs(1)).await;
```

换成

```rust
        let (book, a, h) = (Arc::new(SmartBook::new()), outbound(), session());
        watch(&h, book.clone(), "A", &a, "a.test", true);
        tokio::time::sleep(Duration::from_secs(1)).await;
```

`crates/rurge-engine/src/smart.rs`——把

```rust
        watch(&failed, book.clone(), "A", &a, "a.test");
```

换成

```rust
        watch(&failed, book.clone(), "A", &a, "a.test", true);
```

`crates/rurge-engine/src/smart.rs`——把

```rust
        watch(&killed, book.clone(), "B", &b, "a.test");
```

换成

```rust
        watch(&killed, book.clone(), "B", &b, "a.test", true);
```

`crates/rurge-engine/src/smart.rs`——把

```rust
        watch(&ended, book.clone(), "C", &c, "a.test");
```

换成

```rust
        watch(&ended, book.clone(), "C", &c, "a.test", true);
```

经引擎的用例（`smart` 组的成员是开了 `udp-relay` 的 `socks5`）：

`crates/rurge-engine/tests/smart.rs`——把

```rust
        rurge_policy::smart::Health::Unknown { failures: 0 }
    );
}

```

换成

```rust
        rurge_policy::smart::Health::Unknown { failures: 0 }
    );
}

/// A `smart` group over UDP members (`socks5` with `udp-relay`), for the
/// flows to the loopback.
async fn udp_smart(members: &str, proxies: &str) -> Harness {
    harness(Profile {
        proxies,
        groups: &format!("S = smart, {members}"),
        rules: "IP-CIDR,127.0.0.1/32,S,no-resolve",
        ..Profile::default()
    })
    .await
}

/// A UDP flow's first answer is the member's sample, and the member worked
/// at the site (phase 2 M5 design 8.3).
#[tokio::test]
async fn a_udp_answer_is_reported() {
    let good = FakeSocks5::spawn(Socks5Script::default()).await;
    let proxies = format!(
        "Good = socks5, 127.0.0.1, {}, udp-relay=true",
        good.addr().port()
    );
    let h = udp_smart("Good", &proxies).await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"hi").await;
    assert_eq!(association.recv().await, (echo, b"hi".to_vec()));
    let registry = h.engine.registry();
    let smart = &registry.auto().smart;
    wait_until("the report of the answer", || {
        smart.site("127.0.0.1", Instant::now()).worked == ["Good"]
    })
    .await;
    let good = outbound_now(&h, "Good");
    assert!(matches!(
        smart.health("Good", &good, Instant::now()),
        rurge_policy::smart::Health::Healthy(_)
    ));
}

/// A member whose UDP carrier does not open counts against it; UDP tries
/// no other member (phase 2 M5 design 8.3).
#[tokio::test]
async fn a_member_whose_udp_carrier_does_not_open_counts_against_it() {
    let proxies = format!(
        "Dead = socks5, 127.0.0.1, {}, udp-relay=true",
        closed_port().await
    );
    let h = udp_smart("Dead", &proxies).await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"hi").await;
    let registry = h.engine.registry();
    let smart = &registry.auto().smart;
    wait_until("the failure of the member", || {
        smart.site("127.0.0.1", Instant::now()).failed == ["Dead"]
    })
    .await;
}

/// Three seconds without an answer count against the member only where an
/// answer always comes — here port 443 — never on another port, where a
/// game may only send (phase 2 M5 design 8.3).
#[tokio::test]
async fn udp_silence_counts_only_where_answers_always_come() {
    let quiet = FakeSocks5::spawn(Socks5Script::default()).await;
    let proxies = format!(
        "Quiet = socks5, 127.0.0.1, {}, udp-relay=true",
        quiet.addr().port()
    );
    let h = udp_smart("Quiet", &proxies).await;
    // a socket that takes datagrams and never answers
    let silent = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let association = udp_associate(h.socks()).await;
    association
        .send(
            "127.0.0.1",
            silent.local_addr().unwrap().port(),
            b"only sends",
        )
        .await;
    association.send("127.0.0.1", 443, b"not quic").await;
    let registry = h.engine.registry();
    let smart = &registry.auto().smart;
    wait_until("the silence on port 443 counted", || {
        smart.site("127.0.0.1", Instant::now()).failed == ["Quiet"]
    })
    .await;
    // the other flow's three seconds are up too by now: it did not count
    tokio::time::sleep(Duration::from_millis(500)).await;
    let quiet = outbound_now(&h, "Quiet");
    assert_eq!(
        smart.health("Quiet", &quiet, Instant::now()),
        rurge_policy::smart::Health::Unknown { failures: 1 }
    );
}

```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-engine --test smart udp`
Expected: FAIL——UDP 流还不回报，三条用例都等不到（`common/mod.rs:203` 是 `wait_until` 的超时）：

```text
test a_member_whose_udp_carrier_does_not_open_counts_against_it ... FAILED
test udp_silence_counts_only_where_answers_always_come ... FAILED
test a_udp_answer_is_reported ... FAILED
thread 'a_member_whose_udp_carrier_does_not_open_counts_against_it' panicked at crates\rurge-engine\tests\common\mod.rs:203:9:
timed out waiting for the failure of the member
thread 'udp_silence_counts_only_where_answers_always_come' panicked at crates\rurge-engine\tests\common\mod.rs:203:9:
timed out waiting for the silence on port 443 counted
thread 'a_udp_answer_is_reported' panicked at crates\rurge-engine\tests\common\mod.rs:203:9:
timed out waiting for the report of the answer
test result: FAILED. 0 passed; 3 failed; 0 ignored; 0 measured; 9 filtered out; finished in 5.07s
error: test failed, to rerun pass `-p rurge-engine --test smart`
exit 101
```

Run: `cargo test -p rurge-engine --lib smart`
Expected: FAIL——`watch` 还没有第六个参数，编译不过（节选）：

```text
error[E0061]: this function takes 5 arguments but 6 arguments were supplied
   --> crates\rurge-engine\src\smart.rs:169:9
   --> crates\rurge-engine\src\smart.rs:106:15
error[E0061]: this function takes 5 arguments but 6 arguments were supplied
   --> crates\rurge-engine\src\smart.rs:183:9
   --> crates\rurge-engine\src\smart.rs:106:15
error[E0061]: this function takes 5 arguments but 6 arguments were supplied
   --> crates\rurge-engine\src\smart.rs:197:9
   --> crates\rurge-engine\src\smart.rs:106:15
error[E0061]: this function takes 5 arguments but 6 arguments were supplied
   --> crates\rurge-engine\src\smart.rs:215:9
   --> crates\rurge-engine\src\smart.rs:106:15
```

- [ ] **Step 3: 实现**

`crates/rurge-engine/src/smart.rs`——把

```rust
/// there, or upstream ending before it sent anything while the client was
/// still there, is a failure — whichever comes first, once. A `kill` and a
/// shutdown say nothing of the member.
```

换成

```rust
/// there — when `silence` says such silence tells anything — or upstream
/// ending before it sent anything while the client was still there, is a
/// failure — whichever comes first, once. A `kill` and a shutdown say
/// nothing of the member.
```

`crates/rurge-engine/src/smart.rs`——把

```rust
    host: &str,
```

换成

```rust
    host: &str,
    silence: bool,
```

`crates/rurge-engine/src/smart.rs`——把

```rust
    });
    let session = Arc::downgrade(handle);
```

换成

```rust
    });
    if !silence {
        return;
    }
    let session = Arc::downgrade(handle);
```

TCP 的调用处传 `true`：

`crates/rurge-engine/src/engine.rs`——把

```rust
                    crate::smart::watch(&handle, book.clone(), &pick.member, outbound, &host);
```

换成

```rust
                    crate::smart::watch(&handle, book.clone(), &pick.member, outbound, &host, true);
```

`crates/rurge-engine/src/engine.rs`——把

```rust
                                &host,
```

换成

```rust
                                &host,
                                true,
```

UDP 流：

`crates/rurge-engine/src/udp.rs`——把

```rust
    let mut outbound = resolution.outbound.clone();
```

换成

```rust
    let mut outbound = resolution.outbound.clone();
    // a `smart` group's member tells the book how it did (phase 2 M5
    // design 8.3); not when the flow goes through DIRECT in its place
    let mut smart = resolution.smart.clone();
```

`crates/rurge-engine/src/udp.rs`——把

```rust
                handle.set_error("policy does not support UDP; sent through DIRECT");
```

换成

```rust
                handle.set_error("policy does not support UDP; sent through DIRECT");
                smart = None;
```

`crates/rurge-engine/src/udp.rs`——把

```rust
    let carrier = association
```

换成

```rust
    let host = to.host.to_string();
    let book = &registry.auto().smart;
    let carrier = match association
```

`crates/rurge-engine/src/udp.rs`——把

```rust
        .await
        .map_err(|e| failed(handle, e.to_string()))?;
    let send_to = carrier
```

换成

```rust
        .await
    {
        Ok(carrier) => carrier,
        Err(e) => {
            if let Some(pick) = &smart {
                book.report_failure(&pick.member, &outbound, Some(&host), Instant::now());
            }
            return Err(failed(handle, e.to_string()));
        }
    };
    let send_to = carrier
```

`crates/rurge-engine/src/udp.rs`——把

```rust
    handle.mark_connected();
```

换成

```rust
    handle.mark_connected();
    if let Some(pick) = &smart {
        book.used(&pick.group, &pick.member, Instant::now());
        // three silent seconds tell only where an answer always comes: DNS,
        // and the port QUIC answers on; a game may only send
        let silence = matches!(to.port, 53 | 443);
        crate::smart::watch(
            handle,
            book.clone(),
            &pick.member,
            &outbound,
            &host,
            silence,
        );
    }
```

要点：
- 经 `udp-policy-not-supported-behaviour` 改走 DIRECT 的流把 `smart` 置空：那不是成员的表现，不回报。
- UDP 不换成员（设计 8.3）：载体打不开时这条流照旧失败，下一条流按新的排序选。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-engine --lib smart` → 通过（新增 `silence_that_tells_nothing_is_no_failure`）。
Run: `cargo test -p rurge-engine --test smart` → 通过（新增 `a_udp_answer_is_reported`、`a_member_whose_udp_carrier_does_not_open_counts_against_it`、`udp_silence_counts_only_where_answers_always_come`）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-engine
git commit -m "feat(engine): smart 计入 UDP——载体打不开算失败、第一个回包是首字节、3 秒无回包只在 53 / 443 端口算失败"
```

### Task 5: `dns-follow-interface`

策略写了 `interface` 且 `dns-follow-interface=true` 时，它自己的解析——`direct` 的目标、代理的服务器名——经这个网卡问普通 DNS 服务器，答案与全局缓存分开存（P10、P11）；没写 `interface` 时 `W0028` 并忽略（P12）。

**Files:**
- Modify: `crates/rurge-net/src/connector.rs`、`crates/rurge-dns/src/resolver.rs`（与用例）、`crates/rurge-dns/src/upstream/udp.rs`、`crates/rurge-engine/src/outbounds.rs`（与用例）、`crates/rurge-engine/src/shared.rs`、`crates/rurge-config/src/spec/common.rs`（与用例）

**Interfaces:**
- Consumes: 既有的 `Resolve`、`DirectConnector::with_opts`、`SocketOpts`、`SystemResolve`、`EngineFactory::direct_connector`、`ResolverCell`；`rurge-dns` 的 `traditional_specs`、`UpstreamSpec`、`DnsCache`、`query_coalesced`、`encrypted_specs`；M4b 起就有的 `Connector::connect_udp`。
- Produces:
  - `rurge_net::connector`：`Resolve::resolve_via<'a>(&'a self, host: &'a str, via: &'a Via) -> BoxFuture<'a, io::Result<Vec<IpAddr>>>`（默认实现 = `resolve`）；`#[derive(Clone)] pub struct Via { pub key: String, pub connector: Arc<dyn Connector> }`；`pub struct ResolveVia`，`ResolveVia::new(inner: Arc<dyn Resolve>, via: Via)`，它的 `resolve` 是 `inner.resolve_via(host, &via)`
  - `pub async fn Resolver::lookup_via(&self, host: &str, opts: LookupOpts, via: &Via) -> Result<DnsResult, DnsError>`；`Resolver` 的 `Resolve` 实现 `resolve_via`
  - `pub fn UdpUpstream::via(addr: SocketAddr, key: &str, connector: Arc<dyn Connector>) -> UdpUpstream`（名字 `udp://<addr> via <key>`）
  - `CommonOpts::dns_follow_interface` 只在写了 `interface` 时为 `true`

- [ ] **Step 1: 先写接口与用例**

接口先放进来（默认实现不改变任何行为）：

`crates/rurge-net/src/connector.rs`——把

```rust
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>>;
```

换成

```rust
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>>;

    /// `host`'s addresses, the questions leaving the way `via` says (a
    /// policy's `interface` with `dns-follow-interface`, phase 2 M5 design
    /// 8.5). A resolver that sends no questions of its own resolves as
    /// usual.
    fn resolve_via<'a>(
        &'a self,
        host: &'a str,
        via: &'a Via,
    ) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        let _ = via;
        self.resolve(host)
    }
}

/// How a lookup's DNS questions leave: through `connector`, their answers
/// kept apart from the others' under `key` (an interface's name).
#[derive(Clone)]
pub struct Via {
    pub key: String,
    pub connector: Arc<dyn Connector>,
}

/// `inner`, every lookup of it through `via`.
pub struct ResolveVia {
    inner: Arc<dyn Resolve>,
    via: Via,
}

impl ResolveVia {
    pub fn new(inner: Arc<dyn Resolve>, via: Via) -> ResolveVia {
        ResolveVia { inner, via }
    }
}

impl Resolve for ResolveVia {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        self.inner.resolve_via(host, &self.via)
    }
```

两处导入（用例经 `use super::*` 用到 `Via` 与 `SystemResolve`）：

`crates/rurge-engine/src/outbounds.rs`——把

```rust
use rurge_config::spec::{CommonOpts, PolicySpec, ProtoSpec};
use rurge_config::{Config, Diagnostic, Diagnostics, KeystoreItem};
use rurge_net::BoxFuture;
use rurge_net::connector::{Connector, DirectConnector, Resolve};
```

换成

```rust
use rurge_config::spec::{CommonOpts, IpVersion, PolicySpec, ProtoSpec};
use rurge_config::{Config, Diagnostic, Diagnostics, KeystoreItem};
use rurge_net::BoxFuture;
use rurge_net::connector::{Connector, DirectConnector, Resolve, ResolveVia, SystemResolve, Via};
```

`crates/rurge-dns/src/resolver.rs`——把

```rust
use rurge_net::connector::{Connector, Resolve};
```

换成

```rust
use rurge_net::connector::{Connector, Resolve, Via};
```

用例：

`crates/rurge-config/src/spec/common.rs`——把

```rust
        assert_eq!(notes.inert, ["dns-follow-interface", "tfo", "ecn"]);
        assert_eq!(notes.ios_only, ["hybrid"]);
```

换成

```rust
        assert_eq!(notes.inert, ["tfo", "ecn"]);
        assert_eq!(notes.ios_only, ["hybrid"]);
    }

    /// `dns-follow-interface` follows the policy's `interface`: written
    /// without one, it says so and does nothing.
    #[test]
    fn dns_follow_interface_without_an_interface_is_ignored() {
        let (c, notes, diags) = read("http, h, 1, dns-follow-interface=true", Applies::Proxy);
        assert!(!c.dns_follow_interface);
        assert!(notes.inert.is_empty());
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].code, codes::W_PARAM_NOT_APPLICABLE);
        assert_eq!(
            diags[0].message,
            "policy `P`: `dns-follow-interface` has no effect without `interface`; ignored"
        );
```

`crates/rurge-dns/src/resolver.rs`——把

```rust
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_net::connector::{DirectConnector, SystemResolve};
```

换成

```rust
    use rurge_config::HostName;
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_net::connector::{
        BoxedDatagram, BoxedStream, ConnectOpts, DirectConnector, SystemResolve, Target,
    };
```

`crates/rurge-dns/src/resolver.rs`——把

```rust
        assert!(delays[0].result.is_ok());
    }
}
```

换成

```rust
        assert!(delays[0].result.is_ok());
    }

    /// A direct connector that notes where its UDP flows go: the interface
    /// a `Via` stands for.
    struct Noting {
        inner: DirectConnector,
        udp: Mutex<Vec<Target>>,
    }

    impl Noting {
        fn via(key: &str) -> (Via, Arc<Noting>) {
            let noting = Arc::new(Noting {
                inner: DirectConnector::new(Arc::new(SystemResolve)),
                udp: Mutex::new(Vec::new()),
            });
            let via = Via {
                key: key.to_string(),
                connector: noting.clone(),
            };
            (via, noting)
        }

        fn udp(&self) -> Vec<Target> {
            self.udp.lock().expect("udp").clone()
        }
    }

    impl Connector for Noting {
        fn connect<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedStream>> {
            self.inner.connect(target, opts)
        }

        fn connect_udp<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
            self.udp.lock().expect("udp").push(target.clone());
            self.inner.connect_udp(target, opts)
        }
    }

    /// Lookups through a `Via` ask the plain servers through its connector
    /// and keep their answers apart; `[Host]` answers them as usual (phase 2
    /// M5 design 8.5).
    #[tokio::test]
    async fn lookups_via_an_interface_go_through_it_with_answers_of_their_own() {
        let mock = MockDns::spawn().await;
        mock.set("a.test", &["10.0.0.1"], &[], 60);
        let e = env(&profile(
            &format!("dns-server = {}", mock.addr()),
            "b.test = 10.0.0.9",
        ));
        let (r, _) = resolver(&e, StaticSystemDns::default());
        let (via, noting) = Noting::via("en1");

        let a = r
            .lookup_via("a.test", LookupOpts::default(), &via)
            .await
            .unwrap();
        assert_eq!(a.v4, vec![v4("10.0.0.1")]);
        let server = Target::new(HostName::Ip(mock.addr().ip()), mock.addr().port());
        assert_eq!(noting.udp(), vec![server]);
        assert_eq!(mock.query_count("a.test", Qtype::A), 1);

        // the lookup through the interface filled no answer of the global one
        r.lookup("a.test", LookupOpts::default()).await.unwrap();
        assert_eq!(mock.query_count("a.test", Qtype::A), 2);
        // and its own answer is kept
        r.lookup_via("a.test", LookupOpts::default(), &via)
            .await
            .unwrap();
        assert_eq!(mock.query_count("a.test", Qtype::A), 2);

        let b = r
            .lookup_via("b.test", LookupOpts::default(), &via)
            .await
            .unwrap();
        assert_eq!(b.v4, vec![v4("10.0.0.9")]);
        assert_eq!(mock.query_count("b.test", Qtype::A), 0);
        assert_eq!(noting.udp().len(), 1, "one flow carries every question");
    }

    /// With encrypted DNS configured, a lookup through a `Via` asks the
    /// encrypted servers as usual, not through its connector (phase 2 M5
    /// design 8.5).
    #[tokio::test]
    async fn with_encrypted_dns_lookups_via_an_interface_go_as_usual() {
        let udp = MockDns::spawn().await;
        let tcp = MockDns::spawn().await;
        udp.set("dns.example", &["127.0.0.1"], &[], 60);
        tcp.set("a.test", &["10.0.0.42"], &[], 60);
        let e = env(&profile(
            &format!(
                "dns-server = {}\nencrypted-dns-server = tcp://dns.example:{}",
                udp.addr(),
                tcp.addr().port()
            ),
            "",
        ));
        let (r, _) = resolver(&e, StaticSystemDns::default());
        let (via, noting) = Noting::via("en1");
        let a = r
            .lookup_via("a.test", LookupOpts::default(), &via)
            .await
            .unwrap();
        assert_eq!(a.v4, vec![v4("10.0.0.42")]);
        assert!(noting.udp().is_empty());
        assert_eq!(tcp.query_count("a.test", Qtype::A), 1);
    }
}
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
    use rurge_net::connector::{ConnectOpts, SystemResolve, Target};
```

换成

```rust
    use rurge_net::connector::{ConnectOpts, Target};
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
        assert!(dry_build(&cfg).is_empty());
    }
}
```

换成

```rust
        assert!(dry_build(&cfg).is_empty());
    }

    /// A resolver that answers the loopback and notes how it was asked.
    #[derive(Default)]
    struct Noting(std::sync::Mutex<Vec<String>>);

    impl Resolve for Noting {
        fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            self.0.lock().unwrap().push(host.to_string());
            Box::pin(std::future::ready(Ok(vec![IpAddr::from([127, 0, 0, 1])])))
        }

        fn resolve_via<'a>(
            &'a self,
            host: &'a str,
            via: &'a Via,
        ) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            self.0
                .lock()
                .unwrap()
                .push(format!("{host} via {}", via.key));
            Box::pin(std::future::ready(Ok(vec![IpAddr::from([127, 0, 0, 1])])))
        }
    }

    /// With `dns-follow-interface`, what a policy looks up is asked through
    /// its interface; without it, as usual (phase 2 M5 design 8.5).
    #[tokio::test]
    async fn a_policy_that_follows_its_interface_looks_up_through_it() {
        let echo = echo_server().await;
        let cfg = config(
            "[Proxy]\nFollow = direct, interface=eth9, dns-follow-interface=true\n\
Plain = direct, interface=eth9\n[Rule]\nFINAL,DIRECT\n",
        );
        let noting = Arc::new(Noting::default());
        let f = EngineFactory::new(&cfg, noting.clone(), Arc::new(NoopSocketHook));
        let target = Target::new(rurge_config::HostName::parse("echo.test"), echo.port());
        for name in ["Follow", "Plain"] {
            let spec = cfg.spec(name).unwrap();
            let out = f.build(spec, f.direct_connector(&spec.common)).unwrap();
            out.connect_tcp(&target, &ConnectOpts::default())
                .await
                .unwrap();
        }
        assert_eq!(
            *noting.0.lock().unwrap(),
            ["echo.test via eth9", "echo.test"]
        );
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-engine --lib outbounds`
Expected: FAIL——`direct_connector` 还不给策略 `ResolveVia`，两次都按普通解析：

```text
test outbounds::tests::a_policy_that_follows_its_interface_looks_up_through_it ... FAILED
thread 'outbounds::tests::a_policy_that_follows_its_interface_looks_up_through_it' panicked at crates\rurge-engine\src\outbounds.rs:614:9:
assertion `left == right` failed
  left: ["echo.test", "echo.test"]
 right: ["echo.test via eth9", "echo.test"]
test result: FAILED. 13 passed; 1 failed; 0 ignored; 0 measured; 52 filtered out; finished in 0.03s
error: test failed, to rerun pass `-p rurge-engine --lib`
exit 101
```

Run: `cargo test -p rurge-dns --lib resolver`
Expected: FAIL——`lookup_via` 由 Step 3 引入，编译不过：

```text
  --> crates\rurge-dns\src\resolver.rs:20:48
error[E0599]: no method named `lookup_via` found for struct `std::sync::Arc<resolver::Resolver>` in the current scope
    --> crates\rurge-dns\src\resolver.rs:1708:14
    --> crates\rurge-dns\src\resolver.rs:477:5
error[E0599]: no method named `lookup_via` found for struct `std::sync::Arc<resolver::Resolver>` in the current scope
    --> crates\rurge-dns\src\resolver.rs:1720:11
    --> crates\rurge-dns\src\resolver.rs:477:5
error[E0599]: no method named `lookup_via` found for struct `std::sync::Arc<resolver::Resolver>` in the current scope
    --> crates\rurge-dns\src\resolver.rs:1726:14
    --> crates\rurge-dns\src\resolver.rs:477:5
error[E0599]: no method named `lookup_via` found for struct `std::sync::Arc<resolver::Resolver>` in the current scope
    --> crates\rurge-dns\src\resolver.rs:1754:14
    --> crates\rurge-dns\src\resolver.rs:477:5
For more information about this error, try `rustc --explain E0599`.
error: could not compile `rurge-dns` (lib test) due to 4 previous errors
exit 101
```

Run: `cargo test -p rurge-config --lib common`
Expected: FAIL：

```text
test spec::common::tests::every_common_parameter_is_parsed ... FAILED
test spec::common::tests::dns_follow_interface_without_an_interface_is_ignored ... FAILED
thread 'spec::common::tests::every_common_parameter_is_parsed' panicked at crates\rurge-config\src\spec\common.rs:261:9:
assertion `left == right` failed
  left: ["dns-follow-interface", "tfo", "ecn"]
 right: ["tfo", "ecn"]
thread 'spec::common::tests::dns_follow_interface_without_an_interface_is_ignored' panicked at crates\rurge-config\src\spec\common.rs:270:9:
assertion failed: !c.dns_follow_interface
test result: FAILED. 6 passed; 2 failed; 0 ignored; 0 measured; 201 filtered out; finished in 0.00s
error: test failed, to rerun pass `-p rurge-config --lib`
exit 101
```

- [ ] **Step 3: 实现**

DNS 上游经连接器：

`crates/rurge-dns/src/upstream/udp.rs`——把

```rust
//! be in flight at once), and one retry over TCP when the answer is truncated.
```

换成

```rust
//! be in flight at once), and one retry over TCP when the answer is truncated.
//! An upstream `via` a connector sends its questions, and the retry, through
//! it (`dns-follow-interface`, phase 2 M5 design 8.5).
```

`crates/rurge-dns/src/upstream/udp.rs`——把

```rust
use rurge_net::BoxFuture;
use std::collections::HashMap;
```

换成

```rust
use rurge_config::HostName;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedDatagram, BoxedStream, ConnectOpts, Connector, Target};
use std::collections::HashMap;
use std::future::poll_fn;
```

`crates/rurge-dns/src/upstream/udp.rs`——把

```rust
use std::time::Duration;
```

换成

```rust
use std::time::Duration;
use tokio::io::ReadBuf;
```

`crates/rurge-dns/src/upstream/udp.rs`——把

```rust
struct Shared {
    socket: UdpSocket,
```

换成

```rust
/// What the questions go out on: a socket of its own, or a datagram from
/// the connector the upstream goes `via`.
enum Socket {
    Own(UdpSocket),
    Via(BoxedDatagram),
}

impl Socket {
    async fn send(&self, wire: &[u8]) -> std::io::Result<()> {
        match self {
            Socket::Own(socket) => socket.send(wire).await.map(|_| ()),
            Socket::Via(datagram) => poll_fn(|cx| datagram.poll_send(cx, wire)).await.map(|_| ()),
        }
    }

    async fn recv(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Socket::Own(socket) => socket.recv(buf).await,
            Socket::Via(datagram) => {
                let mut read = ReadBuf::new(buf);
                poll_fn(|cx| datagram.poll_recv(cx, &mut read)).await?;
                Ok(read.filled().len())
            }
        }
    }
}

struct Shared {
    socket: Socket,
```

`crates/rurge-dns/src/upstream/udp.rs`——把

```rust
    addr: SocketAddr,
```

换成

```rust
    addr: SocketAddr,
    /// What the questions go through, instead of a socket of its own.
    via: Option<Arc<dyn Connector>>,
```

`crates/rurge-dns/src/upstream/udp.rs`——把

```rust
            state: OnceCell::new(),
        }
```

换成

```rust
            via: None,
            state: OnceCell::new(),
        }
    }

    /// The server at `addr`, asked through `connector`; `key` (an
    /// interface's name) sets its name apart from the upstream asked
    /// directly.
    pub fn via(addr: SocketAddr, key: &str, connector: Arc<dyn Connector>) -> UdpUpstream {
        UdpUpstream {
            name: format!("udp://{addr} via {key}"),
            addr,
            via: Some(connector),
            state: OnceCell::new(),
        }
    }

    fn target(&self) -> Target {
        Target::new(HostName::Ip(self.addr.ip()), self.addr.port())
```

`crates/rurge-dns/src/upstream/udp.rs`——把

```rust
                let socket = UdpSocket::bind(bind).await.map_err(io_err)?;
                socket.connect(self.addr).await.map_err(io_err)?;
```

换成

```rust
                let socket = match &self.via {
                    None => {
                        let socket = UdpSocket::bind(bind).await.map_err(io_err)?;
                        socket.connect(self.addr).await.map_err(io_err)?;
                        Socket::Own(socket)
                    }
                    Some(connector) => Socket::Via(
                        connector
                            .connect_udp(&self.target(), &ConnectOpts::default())
                            .await
                            .map_err(io_err)?,
                    ),
                };
```

`crates/rurge-dns/src/upstream/udp.rs`——把

```rust
            let mut tcp =
                match tokio::time::timeout_at(deadline, TcpStream::connect(self.addr)).await {
                    Ok(Ok(s)) => s,
                    Ok(Err(e)) => return Err(io_err(e)),
                    Err(_) => return Err(UpstreamError::Timeout),
                };
```

换成

```rust
            let connecting = async {
                match &self.via {
                    None => TcpStream::connect(self.addr)
                        .await
                        .map(|s| Box::new(s) as BoxedStream),
                    Some(connector) => {
                        let timeout = deadline.saturating_duration_since(Instant::now());
                        connector
                            .connect(&self.target(), &ConnectOpts { timeout })
                            .await
                    }
                }
            };
            let mut tcp = match tokio::time::timeout_at(deadline, connecting).await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => return Err(io_err(e)),
                Err(_) => return Err(UpstreamError::Timeout),
            };
```

解析器：

`crates/rurge-dns/src/resolver.rs`——把

```rust
        }
    }
}

pub struct Resolver {
```

换成

```rust
        }
    }
}

/// What the lookups through one `Via` ask (`dns-follow-interface`, phase 2
/// M5 design 8.5): the plain servers, through its connector, with answers of
/// their own.
struct ViaSet {
    upstreams: Arc<Vec<UpstreamRef>>,
    cache: DnsCache,
}

pub struct Resolver {
```

`crates/rurge-dns/src/resolver.rs`——把

```rust
    self_weak: Mutex<Weak<Resolver>>,
```

换成

```rust
    self_weak: Mutex<Weak<Resolver>>,
    /// The upstream sets of the lookups through a `Via`, by its key.
    vias: Mutex<HashMap<String, Arc<ViaSet>>>,
    /// Said once: lookups through a `Via` do not follow it with encrypted
    /// DNS configured.
    via_unfollowed: AtomicBool,
```

`crates/rurge-dns/src/resolver.rs`——把

```rust
            self_weak: Mutex::new(Weak::new()),
```

换成

```rust
            self_weak: Mutex::new(Weak::new()),
            vias: Mutex::new(HashMap::new()),
            via_unfollowed: AtomicBool::new(false),
```

`crates/rurge-dns/src/resolver.rs`——把

```rust
        self.cache.flush();
```

换成

```rust
        self.cache.flush();
        // rebuilt from the servers of the moment by the next lookup
        self.vias.lock().expect("vias").clear();
```

`crates/rurge-dns/src/resolver.rs`——把

```rust
    pub async fn lookup(&self, host: &str, opts: LookupOpts) -> Result<DnsResult, DnsError> {
```

换成

```rust
    pub async fn lookup(&self, host: &str, opts: LookupOpts) -> Result<DnsResult, DnsError> {
        self.lookup_inner(host, opts, None).await
    }

    /// `lookup`, the questions to the plain servers leaving through `via`
    /// with answers kept apart (`dns-follow-interface`, phase 2 M5 design
    /// 8.5). `[Host]`, the hosts file and the system's own lookups go as
    /// usual; with encrypted DNS configured, nothing follows `via`.
    pub async fn lookup_via(
        &self,
        host: &str,
        opts: LookupOpts,
        via: &Via,
    ) -> Result<DnsResult, DnsError> {
        self.lookup_inner(host, opts, Some(via)).await
    }

    /// The upstream set of `via`, built from the plain servers of the moment.
    fn via_set(&self, via: &Via) -> Arc<ViaSet> {
        let mut vias = self.vias.lock().expect("vias");
        if let Some(set) = vias.get(&via.key) {
            return set.clone();
        }
        let system: Vec<UpstreamSpec> = self
            .system
            .servers()
            .into_iter()
            .map(UpstreamSpec::Udp)
            .collect();
        let upstreams = traditional_specs(&self.configured_udp, self.wants_system, &system)
            .iter()
            .filter_map(|spec| match spec {
                UpstreamSpec::Udp(addr) => Some(Arc::new(UdpUpstream::via(
                    *addr,
                    &via.key,
                    via.connector.clone(),
                )) as UpstreamRef),
                _ => None,
            })
            .collect();
        let set = Arc::new(ViaSet {
            upstreams: Arc::new(upstreams),
            cache: DnsCache::new(self.cfg.cache_capacity),
        });
        vias.insert(via.key.clone(), set.clone());
        set
    }

    /// The upstream step of a lookup through `via`: its own cache, then its
    /// own servers.
    async fn lookup_on(
        &self,
        name: &str,
        want_v6: bool,
        opts: &LookupOpts,
        via: &Via,
        started: Instant,
    ) -> Result<DnsResult, DnsError> {
        let set = self.via_set(via);
        if !opts.bypass_cache {
            match set.cache.get(name) {
                Some(CacheHit::Fresh(a)) if a.v6_queried || !want_v6 => {
                    return Ok(from_cached(&a, Source::Cache { stale: false }, started));
                }
                Some(CacheHit::Negative) => return Err(DnsError::EmptyAnswer),
                _ => {}
            }
        }
        let result = self
            .query_coalesced(&set.upstreams, name, want_v6, opts)
            .await;
        match &result {
            Ok(a) => set.cache.put(name, cached_from(a, want_v6)),
            Err(DnsError::EmptyAnswer) => set.cache.put_negative(name),
            Err(_) => {}
        }
        let answers = result?;
        Ok(from_answers(
            &answers,
            Source::Upstream(answers.upstream.clone()),
            started,
        ))
    }

    async fn lookup_inner(
        &self,
        host: &str,
        opts: LookupOpts,
        via: Option<&Via>,
    ) -> Result<DnsResult, DnsError> {
```

`crates/rurge-dns/src/resolver.rs`——把

```rust
                .system_lookup(&candidate, want_v6, Source::System, started, &opts)
                .await;
        }
```

换成

```rust
                .system_lookup(&candidate, want_v6, Source::System, started, &opts)
                .await;
        }

        if let Some(via) = via {
            if self.encrypted_specs.is_empty() {
                return self.lookup_on(&current, want_v6, &opts, via, started).await;
            }
            if !self.via_unfollowed.swap(true, Ordering::Relaxed) {
                tracing::info!(
                    "dns-follow-interface: the encrypted DNS servers are asked as usual, not through the policy's interface"
                );
            }
        }
```

`crates/rurge-dns/src/resolver.rs`——把

```rust
impl Resolve for Resolver {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        Box::pin(async move {
            let r = self
                .lookup(host, LookupOpts::default())
                .await
                .map_err(io::Error::other)?;
            let addrs = r.addrs();
            if addrs.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no addresses for {host}"),
                ));
            }
            Ok(addrs)
```

换成

```rust
/// The addresses of a lookup of `host`, or the error a connector reports.
fn addresses(host: &str, found: Result<DnsResult, DnsError>) -> io::Result<Vec<IpAddr>> {
    let addrs = found.map_err(io::Error::other)?.addrs();
    if addrs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no addresses for {host}"),
        ));
    }
    Ok(addrs)
}

impl Resolve for Resolver {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        Box::pin(async move { addresses(host, self.lookup(host, LookupOpts::default()).await) })
    }

    fn resolve_via<'a>(
        &'a self,
        host: &'a str,
        via: &'a Via,
    ) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        Box::pin(async move {
            addresses(
                host,
                self.lookup_via(host, LookupOpts::default(), via).await,
            )
```

引擎：`ResolverCell` 转发 `resolve_via`，`direct_connector` 给跟随网卡的策略一个 `ResolveVia`：

`crates/rurge-engine/src/shared.rs`——把

```rust
use rurge_net::BoxFuture;
use rurge_net::connector::Resolve;
```

换成

```rust
use rurge_net::BoxFuture;
use rurge_net::connector::{Resolve, Via};
```

`crates/rurge-engine/src/shared.rs`——把

```rust
            current.resolve(host).await
```

换成

```rust
            current.resolve(host).await
        })
    }

    fn resolve_via<'a>(
        &'a self,
        host: &'a str,
        via: &'a Via,
    ) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        Box::pin(async move {
            let Some(current) = self.0.load_full() else {
                return Err(io::Error::other("no resolver is active"));
            };
            current.resolve_via(host, via).await
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
        Arc::new(DirectConnector::with_opts(
            self.resolver.clone(),
```

换成

```rust
        // with `dns-follow-interface`, what the policy looks up — DIRECT's
        // destinations, a proxy's server — is asked through its interface
        // (phase 2 M5 design 8.5)
        let resolver = match (&common.interface, common.dns_follow_interface) {
            (Some(interface), true) => {
                let questions = DirectConnector::with_opts(
                    // the DNS servers are addresses: nothing to look up
                    Arc::new(SystemResolve),
                    SocketOpts {
                        interface: Some(interface.clone()),
                        allow_other_interface: common.allow_other_interface,
                        ip_version: IpVersion::Dual,
                        v6_first: self.v6_first,
                        tos: 0,
                    },
                    self.hook.clone(),
                );
                let via = Via {
                    key: interface.clone(),
                    connector: Arc::new(questions),
                };
                Arc::new(ResolveVia::new(self.resolver.clone(), via)) as Arc<dyn Resolve>
            }
            _ => self.resolver.clone(),
        };
        Arc::new(DirectConnector::with_opts(
            resolver,
```

配置：

`crates/rurge-config/src/spec/common.rs`——把

```rust
    let dns_follow_interface = r.bool("dns-follow-interface").unwrap_or(false);
```

换成

```rust
    let mut dns_follow_interface = r.bool("dns-follow-interface").unwrap_or(false);
    // it moves the policy's lookups onto its interface: without one there
    // is nothing to move them to (phase 2 M5 design 8.5)
    if dns_follow_interface && interface.is_none() {
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            "`dns-follow-interface` has no effect without `interface`; ignored".to_string(),
        );
        dns_follow_interface = false;
    }
```

`crates/rurge-config/src/spec/common.rs`——把

```rust
        let inert = [
            ("dns-follow-interface", dns_follow_interface),
```

换成

```rust
        let inert = [
```

要点：
- 问 DNS 的连接器绑的是策略的网卡、`ip-version` 取 `dual`、TOS 为 0：DNS 服务器是地址，不需要解析；`allow-other-interface` 沿用策略的。
- `[Host]`、hosts 文件、`.local` 与经系统接口的解析不发 DNS 报文，照常走；只有问上游那一步换成经 `Via` 的一组上游（P11）。
- 这组上游与缓存按网卡名存一份，`flush`（网络变化）时清掉，下一次按新的系统服务器重建。
- 配了加密 DNS 时不跟随，第一次记一条 info（不带查询名）。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-dns --lib resolver` → 通过（新增 `lookups_via_an_interface_go_through_it_with_answers_of_their_own`、`with_encrypted_dns_lookups_via_an_interface_go_as_usual`）。
Run: `cargo test -p rurge-engine --lib outbounds` → 通过（新增 `a_policy_that_follows_its_interface_looks_up_through_it`）。
Run: `cargo test -p rurge-config --lib common` → 通过（新增 `dns_follow_interface_without_an_interface_is_ignored`）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-net/src/connector.rs crates/rurge-dns crates/rurge-engine/src crates/rurge-config
git commit -m "feat(dns): dns-follow-interface——策略自己的解析经它的网卡问普通 DNS 服务器、答案另存；没有 interface 时 W0028"
```

### Task 6: 互操作与文档

经 sing-box WireGuard 端点往返一个 UDP 回显（P14）；兼容性清单、API 文档、两份 README、`CLAUDE.md` 与手工验收跟上 M5c。

**Files:**
- Modify: `tests/interop/tests/sing_box_wireguard.rs`、`tests/interop/README.md`、`docs/surge-compatibility-matrix.md`、`docs/api/phase2.md`、`README.md`、`README_en.md`、`CLAUDE.md`、`docs/acceptance/phase2-manual.md`

**Interfaces:**
- Consumes: 互操作夹具既有的 `udp_roundtrip(out, echo)` 与 `udp_echo_server()`（M5b）；Task 1 的 `wireguard` UDP。
- Produces: 无代码接口。

- [ ] **Step 1: 互操作用例**

在既有的 WireGuard 用例里加一次 UDP 往返（发往 sing-box 自己的隧道地址 `10.9.0.1` 上回显的端口，它被改写到回环，与 TCP 相同）：

`tests/interop/tests/sing_box_wireguard.rs`——把

```rust
//! `client-id`, and the handshake test.
```

换成

```rust
//! `client-id`, the handshake test, and UDP through the tunnel (phase 2
//! M5c).
```

`tests/interop/tests/sing_box_wireguard.rs`——把

```rust
    let echo = echo_server().await;
```

换成

```rust
    let echo = echo_server().await;
    let udp_echo = udp_echo_server().await;
```

`tests/interop/tests/sing_box_wireguard.rs`——把

```rust
    roundtrip_big(&out, through).await;
```

换成

```rust
    roundtrip_big(&out, through).await;
    udp_roundtrip(&out, SocketAddr::from(([10, 9, 0, 1], udp_echo.port()))).await;
```

`tests/interop/README.md`——把

```markdown
- WireGuard（`tests/sing_box_wireguard.rs`）：sing-box 的 WireGuard 端点（`endpoints`，sing-box 1.11 起；`system: false`，在用户态运行，不建网卡、不改路由）以 rurge 为唯一的 peer，给发往它的每个报文写上保留字节 `1/2/3`；rurge 的 `wireguard` 出站带 `client-id = 1/2/3` 与它握手，经隧道连 sing-box 自己的隧道地址 `10.9.0.1` 上的 echo 端口（节里 `allowed-ips = 10.9.0.1/32`）：sing-box 把发往端点自身地址的连接改写到它的回环 `127.0.0.1` / `::1`，echo 就听在那里（sing-box 1.14.1 `protocol/wireguard/endpoint.go` 的 `NewConnectionEx`）——直接发往 `127.0.0.1` 的目标经隧道进来，可能被它的用户态协议栈丢弃；隧道里出来的连接交给 `direct`。覆盖单块与跨多块的往返，以及原生测速（强制握手）。rurge 收到的报文里 sing-box 写的保留字节必须先清零，否则 boringtun 认不出报文类型、握手不成。端点没有监听地址这一项，**它的 UDP 端口开在所有地址上**；就绪与否看同一份配置里一个只听 `127.0.0.1` 的 `mixed` 入站（UDP 端口无从探测）。
```

换成

```markdown
- WireGuard（`tests/sing_box_wireguard.rs`）：sing-box 的 WireGuard 端点（`endpoints`，sing-box 1.11 起；`system: false`，在用户态运行，不建网卡、不改路由）以 rurge 为唯一的 peer，给发往它的每个报文写上保留字节 `1/2/3`；rurge 的 `wireguard` 出站带 `client-id = 1/2/3` 与它握手，经隧道连 sing-box 自己的隧道地址 `10.9.0.1` 上的 echo 端口（节里 `allowed-ips = 10.9.0.1/32`）：sing-box 把发往端点自身地址的连接改写到它的回环 `127.0.0.1` / `::1`，echo 就听在那里（sing-box 1.14.1 `protocol/wireguard/endpoint.go` 的 `NewConnectionEx`）——直接发往 `127.0.0.1` 的目标经隧道进来，可能被它的用户态协议栈丢弃；隧道里出来的连接交给 `direct`。覆盖单块与跨多块的往返、原生测速（强制握手），以及经隧道往返一个 UDP 回显（阶段 2 / M5c；发往隧道地址 `10.9.0.1` 上回显的端口，同样被改写到回环）。rurge 收到的报文里 sing-box 写的保留字节必须先清零，否则 boringtun 认不出报文类型、握手不成。端点没有监听地址这一项，**它的 UDP 端口开在所有地址上**；就绪与否看同一份配置里一个只听 `127.0.0.1` 的 `mixed` 入站（UDP 端口无从探测）。
```

- [ ] **Step 2: 运行**

Run: `cargo test -p rurge-interop --test sing_box_wireguard`
Expected: 本机没有 sing-box 时打印一行 `skipping …` 后通过（`RURGE_TEST_SING_BOX` 或 `PATH` 上有 sing-box 1.14.1 时真正运行）。互操作由首次推送后的 CI 证明（CI 设置 `RURGE_INTEROP_REQUIRED=1`）。

- [ ] **Step 3: 文档**

兼容性清单（`proxy-test-udp`、`wireguard` 的 4.2 与 4.6 两行、`dns-follow-interface`、`ecn`（仍只解析，M5-D6）、`test-udp`、自动支持 UDP 的协议、UDP 测试、`wireguard` 策略行、WireGuard 生命周期、`smart`）：

`docs/surge-compatibility-matrix.md`——把

```markdown
| `proxy-test-udp` | `hostname@ipv4` | 全部 | ✅ | 2 | |
```

换成

```markdown
| `proxy-test-udp` | `hostname@ipv4` | 全部 | ✅ | 2 | M5c 生效：策略没写 `test-udp` 时的缺省，见 `test-udp` 行 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `wireguard` | WireGuard L3 隧道作为策略 | 全部 | ✅ | 2 | M4b（阶段 2）已实现（TCP）：用户态 WireGuard（boringtun 的 `Tunn` 加 smoltcp 协议栈，不建虚拟网卡、不改路由）；第一次用到（拨号或测速）时启动：给每个 peer 建一条 UDP 载体、各握手一次，启动时连不上的 peer 每 5 分钟再试；指向同一个节、载体设置（`ip-version`、`underlying-proxy`、`[General] ipv6`）也相同的策略共用一条隧道，载体设置不同的按冲突处理（见下）；同一私钥连着同一 peer 的两条隧道会互相抢 peer 记住的地址（peer 回应最后写来的地址），所以一条隧道启动时结束同一私钥、有共同 peer、来自更早配置的那条——重载改了节或载体设置时，旧隧道上的连接随之断开——更早配置的策略也不能再把它抢回去（拨号报 `wireguard: a newer configuration of this tunnel is in use`；同一份配置里两条策略指向同一个节而载体设置不同时，同样只有较晚构建的那条能用；同一份配置里两个节私钥相同、又有共同的 peer 时同样冲突：较晚构建的策略能用，另一条拨号报 `wireguard: a newer configuration of this tunnel is in use`，加载时没有诊断）；隧道之间的启动不排队；私钥相同而 peer 不同的两个节互不影响。目标是域名时：节里配了 `dns-server` 就经隧道查询，没配（或写 `system`）时在本机解析、含 `[Host]`（手册只说一般不能经这个策略解析）；经隧道查询时先等隧道第一次握手完成（受拨号时限约束，刚启动时丢了的第一个握手发起包不会让查询失败），每个问题在它的 2 秒等待里每过三分之一重发一次（同一个问题，最多发三次）；A 与 AAAA 都问时，一族没有回应而另一族没有地址不算作答：接着问下一个服务器，也不缓存；一族有地址而另一族没有回应时算作答，只缓存有地址的那一族到 TTL 为止（例如 AAAA 没回应时这段时间里只用 IPv4，`prefer-ipv6` 用不上）；目标地址不在任何 peer 的 `allowed-ips` 里时立即失败（`wireguard: no peer's allowed-ips covers <地址>`，绝不直连兜底），隧道没有该地址族的本端地址时同样立即失败（`wireguard: the tunnel has no IPv4 address` / `… IPv6 address`）；目标端口拒绝连接是 `wireguard: the destination refused the connection`；peer 不回应握手时拨号在时限处超时。带 `underlying-proxy` 时隧道起不来（M5 之前链路不载 UDP）：会话 REJECT，请求记录的说明是 `policy protocol not implemented: wireguard over underlying-proxy`，不静默改走直连。endpoint 写成域名时每 5 分钟重新解析，地址变了就换新的载体并立即握手（日志 `wireguard: the peer's endpoint moved`）——名字有多条记录、每次解析的顺序轮换时，旧地址仍然可用也可能在这 5 分钟一次的重拨时换载体并记这条日志；某个 peer 的载体发送时本机地址或路由已经失效（地址不可用、网络或主机不可达、网络已断；每个 peer 至多每 10 秒一次；别的发送错误，如发送缓冲区满，只丢掉那一个报文）或 peer 不再回应握手（boringtun 约 90 秒后放弃重试）时，给它新拨一条载体（去的地址相同也换）并立即握手，所以网络变化（换了网络、本机地址变了）之后隧道不必重启 rurge 就能恢复（最坏约两分钟）；peer 一直不回应时，每约 90 秒这样重试一次，每次记一条 `wireguard: the peer did not answer the handshake`；"网络已变化"的入口（重建全部载体、重新握手）已有，阶段 2 没有触发它的探测器。开了 `encrypted-dns-follow-outbound-mode` 时，DNS 会话路由到的 `wireguard` 策略只要有一个 peer 的 endpoint 写成域名，就与以域名配置的代理一样告警并改走直连（防环：启动隧道先要解析那个域名；把 endpoint 写成地址即可避免）。日志只带策略名与 peer 序号：握手成功（第一次与失败之后恢复时）、握手没有回应（boringtun 放弃重试时，约 90 秒后）、启动时 peer 连不上；boringtun 自己的日志不输出。出站被释放（重载删掉或改了这条策略）后，隧道在经它的最后一个连接结束时停止（被接替的除外）。已知的 smoltcp 0.12 限制：TCP 丢包后回退 N 重传（整窗重发）；发送方没有零窗口探测；对端关闭发送方向之后仍在途的数据丢失时不再重传——极少数上传可能卡到空闲超时；不开 TCP keep-alive；拥塞窗口实际不限制发送（0.12 只拿它与对端窗口的余量比较，从不与在途字节数比较），每条连接仍显式设为 Reno。每条连接在隧道里有 256 KiB 的发送与接收缓冲，单条连接每个往返因此至多传约 256 KiB（如往返 100 毫秒时单连接约 2.5 MiB/s）。吞吐参考：Windows 11 回环上两端都是 smoltcp 的双向回显约 6 MiB/s 每方向。不支持 UDP（M5）。参数与差异见 4.6 节 `wireguard` 行 |
```

换成

```markdown
| `wireguard` | WireGuard L3 隧道作为策略 | 全部 | ✅ | 2 | M4b（阶段 2）已实现（TCP）：用户态 WireGuard（boringtun 的 `Tunn` 加 smoltcp 协议栈，不建虚拟网卡、不改路由）；第一次用到（拨号或测速）时启动：给每个 peer 建一条 UDP 载体、各握手一次，启动时连不上的 peer 每 5 分钟再试；指向同一个节、载体设置（`ip-version`、`underlying-proxy`、`[General] ipv6`）也相同的策略共用一条隧道，载体设置不同的按冲突处理（见下）；同一私钥连着同一 peer 的两条隧道会互相抢 peer 记住的地址（peer 回应最后写来的地址），所以一条隧道启动时结束同一私钥、有共同 peer、来自更早配置的那条——重载改了节或载体设置时，旧隧道上的连接随之断开——更早配置的策略也不能再把它抢回去（拨号报 `wireguard: a newer configuration of this tunnel is in use`；同一份配置里两条策略指向同一个节而载体设置不同时，同样只有较晚构建的那条能用；同一份配置里两个节私钥相同、又有共同的 peer 时同样冲突：较晚构建的策略能用，另一条拨号报 `wireguard: a newer configuration of this tunnel is in use`，加载时没有诊断）；隧道之间的启动不排队；私钥相同而 peer 不同的两个节互不影响。目标是域名时：节里配了 `dns-server` 就经隧道查询，没配（或写 `system`）时在本机解析、含 `[Host]`（手册只说一般不能经这个策略解析）；经隧道查询时先等隧道第一次握手完成（受拨号时限约束，刚启动时丢了的第一个握手发起包不会让查询失败），每个问题在它的 2 秒等待里每过三分之一重发一次（同一个问题，最多发三次）；A 与 AAAA 都问时，一族没有回应而另一族没有地址不算作答：接着问下一个服务器，也不缓存；一族有地址而另一族没有回应时算作答，只缓存有地址的那一族到 TTL 为止（例如 AAAA 没回应时这段时间里只用 IPv4，`prefer-ipv6` 用不上）；目标地址不在任何 peer 的 `allowed-ips` 里时立即失败（`wireguard: no peer's allowed-ips covers <地址>`，绝不直连兜底），隧道没有该地址族的本端地址时同样立即失败（`wireguard: the tunnel has no IPv4 address` / `… IPv6 address`）；目标端口拒绝连接是 `wireguard: the destination refused the connection`；peer 不回应握手时拨号在时限处超时。带 `underlying-proxy` 时（M5c 起）载体经底层策略的 UDP：底层策略不载 UDP（如 `http`）时隧道起不来，拨号失败（`via <底层策略>: the underlying policy cannot carry UDP`），不静默改走直连。endpoint 写成域名时每 5 分钟重新解析，地址变了就换新的载体并立即握手（日志 `wireguard: the peer's endpoint moved`）——名字有多条记录、每次解析的顺序轮换时，旧地址仍然可用也可能在这 5 分钟一次的重拨时换载体并记这条日志；某个 peer 的载体发送时本机地址或路由已经失效（地址不可用、网络或主机不可达、网络已断；每个 peer 至多每 10 秒一次；别的发送错误，如发送缓冲区满，只丢掉那一个报文）或 peer 不再回应握手（boringtun 约 90 秒后放弃重试）时，给它新拨一条载体（去的地址相同也换）并立即握手，所以网络变化（换了网络、本机地址变了）之后隧道不必重启 rurge 就能恢复（最坏约两分钟）；peer 一直不回应时，每约 90 秒这样重试一次，每次记一条 `wireguard: the peer did not answer the handshake`；"网络已变化"的入口（重建全部载体、重新握手）已有，阶段 2 没有触发它的探测器。开了 `encrypted-dns-follow-outbound-mode` 时，DNS 会话路由到的 `wireguard` 策略只要有一个 peer 的 endpoint 写成域名，就与以域名配置的代理一样告警并改走直连（防环：启动隧道先要解析那个域名；把 endpoint 写成地址即可避免）。日志只带策略名与 peer 序号：握手成功（第一次与失败之后恢复时）、握手没有回应（boringtun 放弃重试时，约 90 秒后）、启动时 peer 连不上；boringtun 自己的日志不输出。出站被释放（重载删掉或改了这条策略）后，隧道在经它的最后一个连接结束时停止（被接替的除外）。已知的 smoltcp 0.12 限制：TCP 丢包后回退 N 重传（整窗重发）；发送方没有零窗口探测；对端关闭发送方向之后仍在途的数据丢失时不再重传——极少数上传可能卡到空闲超时；不开 TCP keep-alive；拥塞窗口实际不限制发送（0.12 只拿它与对端窗口的余量比较，从不与在途字节数比较），每条连接仍显式设为 Reno。每条连接在隧道里有 256 KiB 的发送与接收缓冲，单条连接每个往返因此至多传约 256 KiB（如往返 100 毫秒时单连接约 2.5 MiB/s）。吞吐参考：Windows 11 回环上两端都是 smoltcp 的双向回显约 6 MiB/s 每方向。UDP（M5c）：隧道里每个地址族一个 UDP socket，第一次发往该族时绑定隧道的本端地址与一个空闲端口；全锥（隧道那一端任何来源的回包都送回客户端，来源原样）；目标是域名时与 TCP 一样解析（节里有 `dns-server` 时经隧道，否则在本机）；目标不在任何 peer 的 `allowed-ips` 里或隧道没有该地址族的本端地址时，那条 UDP 流失败（错误同 TCP）；隧道里的发送缓冲满时丢掉那个数据报。启动时 peer 连不上的告警（`wireguard: the peer cannot be reached`）每个策略的每个 peer 5 分钟至多一次，其余只记 debug。参数与差异见 4.6 节 `wireguard` 行 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `dns-follow-interface` | 布尔；默认 false | 🟡 | 2（M5） | 解析，W0029；M5 生效 |
```

换成

```markdown
| `dns-follow-interface` | 布尔；默认 false | 🟡 | 2 | M5c 已实现：策略写了 `interface` 时，它自己的解析（`direct` 的目标、代理的服务器名）经这个网卡问 `dns-server` 里的普通服务器（含 `system` 展开的系统服务器），答案与全局缓存分开存；`[Host]`、hosts 文件与经系统接口的解析照常；配了 `encrypted-dns-server` 时不跟随（照常问加密 DNS，日志说明一次）；没有 `interface` 时 `W0028` 并忽略。规则匹配时的解析发生在选定策略之前，仍走全局（手册只说"匹配该策略的 DNS 请求使用这个网卡查询"） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `ecn` | `auto` `on` `off`；QUIC 类协议默认开启，WireGuard/Tailscale 默认关闭 | 🟡 | 2 | 取决于所选 QUIC 库对 ECN 的支持；M1 解析并校验取值，`W0029`；M5 生效 |
```

换成

```markdown
| `ecn` | `auto` `on` `off`；QUIC 类协议默认开启，WireGuard/Tailscale 默认关闭 | 🟡 | 2 | 取决于所选 QUIC 库对 ECN 的支持；M1 解析并校验取值，`W0029`；M5 仍只解析（M5-D6），随 QUIC 族再议 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `test-udp` | `hostname@ipv4` | ✅ | 2 | M1 解析并校验取值，`W0029`；M5 生效（M3 细化设计订正了原来的"M3 生效"） |
```

换成

```markdown
| `test-udp` | `hostname@ipv4` | ✅ | 2 | M1 解析并校验取值；M5c 生效：经策略的 UDP 向 `ipv4` 的 53 端口问一次 `hostname` 的 A 记录，时限同该策略的测试超时；结果只在 `POST /v1/policies/test` 的 `udp` 键里给出，不保存、不参与组的选择、不进请求记录；不载 UDP 的策略不测 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| 自动支持 UDP 的协议：Snell v3+、VMess、Trojan、TUIC、Hysteria 2、MASQUE、AnyTLS（UDP over TCP）、WireGuard、Tailscale | | ✅ M5b：VMess（对称型）、Trojan、AnyTLS 已生效；WireGuard 随 M5c，其余随各自的协议 | 2 |
| 不支持 UDP 的协议：HTTP / HTTPS、Trust Tunnel、SSH | 受 `udp-policy-not-supported-behaviour` 控制 | ✅ M5a 已实现；`underlying-proxy` 的底层策略不支持 UDP 时，经它的 UDP 流失败并写 `via <底层策略>: the underlying policy cannot carry UDP` | 2 |
| DIRECT / REJECT 系始终处理 UDP | | ✅ | 1 |
| UDP 测试：通过中继向 `hostname@ipv4` 做 DNS 查询 | `proxy-test-udp` / `test-udp` | ✅ | 2 |
```

换成

```markdown
| 自动支持 UDP 的协议：Snell v3+、VMess、Trojan、TUIC、Hysteria 2、MASQUE、AnyTLS（UDP over TCP）、WireGuard、Tailscale | | ✅ M5b：VMess（对称型）、Trojan、AnyTLS 已生效；M5c：WireGuard 已生效；其余随各自的协议 | 2 |
| 不支持 UDP 的协议：HTTP / HTTPS、Trust Tunnel、SSH | 受 `udp-policy-not-supported-behaviour` 控制 | ✅ M5a 已实现；`underlying-proxy` 的底层策略不支持 UDP 时，经它的 UDP 流失败并写 `via <底层策略>: the underlying policy cannot carry UDP` | 2 |
| DIRECT / REJECT 系始终处理 UDP | | ✅ | 1 |
| UDP 测试：通过中继向 `hostname@ipv4` 做 DNS 查询 | `proxy-test-udp` / `test-udp` | ✅ M5c：结果只在 `POST /v1/policies/test` 的 `udp` 键里，不参与组的选择 | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `wireguard` 策略行 | `section-name`（必填）`underlying-proxy`（默认 DIRECT）`test-url`（仅 http）`test-timeout`（另加 10 秒 L3 初始化）`ecn` | ✅ | 2 | M4b 已实现。`section-name` 必填（缺了是 `E0018`），指向不存在或有错的节是 `E0023`；spec 带着节的全部内容：改了节，重载就重建出站（隧道随之替换，见 4.2 节 `wireguard` 行）；`test-url` 只接受 `http://`（否则 `E0018`，不引用取值）；`test-timeout` 照常，另加 10 秒留给隧道启动（照手册）；`interface` / `allow-other-interface` / `tfo` / `tos` 不适用（`W0028`，忽略）；`ip-version` 决定解析 endpoint 时优先的地址族；`ecn`（`W0029`）与 `underlying-proxy` 在 M5 生效——后者此前让策略 REJECT，加载时的 `W0029` 说明这一点；不能叠 Shadow TLS（`E0018`）；订阅行自己写的 `section-name=` 不生效（见 5.2 节 `policy-path` 行） |
| `[WireGuard <name>]` | `private-key`（Base64 或 64 位十六进制）`self-ip` / `self-ip-v6`（至少一个）`dns-server` `prefer-ipv6` `mtu`（576–1420，默认 1280）`peer`（可多个，多行累加） | ✅ | 2 | M4b 已实现。密钥接受 Base64（带不带填充都行）或 64 位十六进制，错误只点名键，不引用取值；`self-ip` / `self-ip-v6` 是单播的纯地址（写成前缀，或写成组播、广播、未指定地址，都是 `E0023`）；`dns-server` 逗号分隔：IPv4 / IPv6 地址、带端口的 `1.1.1.1:53` / `[2606:4700::1111]:53` 或 `system`，组播地址（IPv4 与 IPv6）、未指定地址、端口 0 与加密 DNS 的 URL 不接受（`E0023`）——按列表顺序问，第一个作答的服务器为准（"没有这个名字"也算作答），每个最多等 2 秒，该地址族没有本端地址或没有 peer 覆盖的服务器直接跳过，`system` 表示在那个位置改用本机解析；A 与 AAAA 按本端有的地址族同时问，成功的结果按 TTL 缓存（最多 256 个名字、最长 1 小时）；`prefer-ipv6` 在两族本端地址都有时决定先用哪一族（隧道内 DNS 与本机解析都是）；`private-key` 与 `peer` 的 `preshared-key` 在 `profiles/current`（`sensitive=0`）里为 `***`（`preshared-key` 自 M4b 起进了脱敏名单） |
| `peer` 字段 | `public-key` `allowed-ips`（最长前缀匹配，v4/v6 分表）`endpoint` `preshared-key` `keepalive`（0–65535）`client-id`（`83/12/235` / 3 字节十六进制 / 4 字符 Base64，WARP 保留字节） | ✅ | 2 | M4b 已实现。`public-key`、`allowed-ips`、`endpoint` 必填；一个节至少一个 `peer`；`allowed-ips` 含逗号时整值加引号，写成纯地址时按单个主机，两个 peer 列了同一前缀时后写的生效；内层源地址不在该 peer `allowed-ips` 里的包丢弃；`endpoint` 写作 `host:port`（IPv6 写作 `[addr]:port`），主机名不受 `[Host]` 影响（与代理服务器的主机名相同）；`keepalive` 为 0 表示关闭；`client-id` 写进每个发出的报文的保留字节，收到的报文先把这 3 个字节清零再解（照手册） |
| WireGuard 生命周期 | 加载时准备、按需握手；网络变化或底层策略变化时重建；分片重组；仅回应发往本地隧道地址的 ICMP echo；握手包 DSCP 0x88 | 🟡 | 2 | M4b：构建（含 `rurge check` 的干构建）只解析配置，不开 socket、不解析域名；第一次用到时启动；隧道内 IP 分片重组（最多同时 4 个、每个 16 KiB）；超过 MTU 的 IPv4 外发包由协议栈分片后发出，但只到 smoltcp 的分片缓冲（1500 字节）为止，更大的丢弃（手册：丢弃；TCP 报文段按 MSS 切分，本来不会超出，只有 ICMP 回显应答可能更大）；只回应发往本端隧道地址的 ICMP echo；握手发起包的 TOS 字节标 0x88（DSCP AF41），其它包不标——Windows 通常忽略应用设置的 DSCP；载体的 UDP 收发缓冲尽量设为 7 MiB（系统可能封顶）；"网络已变化"的入口已有，阶段 2 没有触发它的探测器（阶段 3），在那之前靠载体发送时本机地址或路由失效、或 peer 不再回应握手时换新的载体恢复（见 4.2 节 `wireguard` 行）；底层策略（`underlying-proxy`）在 M5 生效 |
```

换成

```markdown
| `wireguard` 策略行 | `section-name`（必填）`underlying-proxy`（默认 DIRECT）`test-url`（仅 http）`test-timeout`（另加 10 秒 L3 初始化）`ecn` | ✅ | 2 | M4b 已实现。`section-name` 必填（缺了是 `E0018`），指向不存在或有错的节是 `E0023`；spec 带着节的全部内容：改了节，重载就重建出站（隧道随之替换，见 4.2 节 `wireguard` 行）；`test-url` 只接受 `http://`（否则 `E0018`，不引用取值）；`test-timeout` 照常，另加 10 秒留给隧道启动（照手册）；`interface` / `allow-other-interface` / `tfo` / `tos` 不适用（`W0028`，忽略）；`ip-version` 决定解析 endpoint 时优先的地址族；`underlying-proxy` 自 M5c 生效（见 4.2 节 `wireguard` 行）；`ecn` 解析并忽略（`W0029`）；不能叠 Shadow TLS（`E0018`）；订阅行自己写的 `section-name=` 不生效（见 5.2 节 `policy-path` 行） |
| `[WireGuard <name>]` | `private-key`（Base64 或 64 位十六进制）`self-ip` / `self-ip-v6`（至少一个）`dns-server` `prefer-ipv6` `mtu`（576–1420，默认 1280）`peer`（可多个，多行累加） | ✅ | 2 | M4b 已实现。密钥接受 Base64（带不带填充都行）或 64 位十六进制，错误只点名键，不引用取值；`self-ip` / `self-ip-v6` 是单播的纯地址（写成前缀，或写成组播、广播、未指定地址，都是 `E0023`）；`dns-server` 逗号分隔：IPv4 / IPv6 地址、带端口的 `1.1.1.1:53` / `[2606:4700::1111]:53` 或 `system`，组播地址（IPv4 与 IPv6）、未指定地址、端口 0 与加密 DNS 的 URL 不接受（`E0023`）——按列表顺序问，第一个作答的服务器为准（"没有这个名字"也算作答），每个最多等 2 秒，该地址族没有本端地址或没有 peer 覆盖的服务器直接跳过，`system` 表示在那个位置改用本机解析；A 与 AAAA 按本端有的地址族同时问，成功的结果按 TTL 缓存（最多 256 个名字、最长 1 小时）；`prefer-ipv6` 在两族本端地址都有时决定先用哪一族（隧道内 DNS 与本机解析都是）；`private-key` 与 `peer` 的 `preshared-key` 在 `profiles/current`（`sensitive=0`）里为 `***`（`preshared-key` 自 M4b 起进了脱敏名单） |
| `peer` 字段 | `public-key` `allowed-ips`（最长前缀匹配，v4/v6 分表）`endpoint` `preshared-key` `keepalive`（0–65535）`client-id`（`83/12/235` / 3 字节十六进制 / 4 字符 Base64，WARP 保留字节） | ✅ | 2 | M4b 已实现。`public-key`、`allowed-ips`、`endpoint` 必填；一个节至少一个 `peer`；`allowed-ips` 含逗号时整值加引号，写成纯地址时按单个主机，两个 peer 列了同一前缀时后写的生效；内层源地址不在该 peer `allowed-ips` 里的包丢弃；`endpoint` 写作 `host:port`（IPv6 写作 `[addr]:port`），主机名不受 `[Host]` 影响（与代理服务器的主机名相同）；`keepalive` 为 0 表示关闭；`client-id` 写进每个发出的报文的保留字节，收到的报文先把这 3 个字节清零再解（照手册） |
| WireGuard 生命周期 | 加载时准备、按需握手；网络变化或底层策略变化时重建；分片重组；仅回应发往本地隧道地址的 ICMP echo；握手包 DSCP 0x88 | 🟡 | 2 | M4b：构建（含 `rurge check` 的干构建）只解析配置，不开 socket、不解析域名；第一次用到时启动；隧道内 IP 分片重组（最多同时 4 个、每个 16 KiB）；超过 MTU 的 IPv4 外发包由协议栈分片后发出，但只到 smoltcp 的分片缓冲（1500 字节）为止，更大的丢弃（手册：丢弃；TCP 报文段按 MSS 切分，本来不会超出，只有 ICMP 回显应答可能更大）；只回应发往本端隧道地址的 ICMP echo；握手发起包的 TOS 字节标 0x88（DSCP AF41），其它包不标——Windows 通常忽略应用设置的 DSCP；载体的 UDP 收发缓冲尽量设为 7 MiB（系统可能封顶）；"网络已变化"的入口已有，阶段 2 没有触发它的探测器（阶段 3），在那之前靠载体发送时本机地址或路由失效、或 peer 不再回应握手时换新的载体恢复（见 4.2 节 `wireguard` 行）；底层策略（`underlying-proxy`）自 M5c 生效：peer 的载体经底层策略的 UDP |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `smart` | 按真实连接质量动态选择：首响应延迟时间加权均值 + 重传惩罚（约每 1% 丢包 50 ms）× `policy-priority`；接近最优者构成优选集，其余为重试列表；按站点记忆约 1 小时；固定 5 分钟重测，`interval` 无效；>12 成员只测子集；忽略嵌套组与内置策略 | 🟡 | 2 | M3c 已实现，算法细节手册未公开，rurge 按手册描述近似实现（常数见 M3c 设计第 9 节）。差异：用失败罚分（每次 800 ms，5 分钟减半）近似重传率；只在拨号阶段换成员——选中的成员连不上时依次再试排在后面的两个，每次最多用"剩余时间 ÷ 剩余尝试次数"，总计 10 秒；"3 秒无响应"只计入打分，不在已建立的连接上换成员；首字节耗时与 3 秒都从出站就绪起算；分数与站点记忆按策略记，几个 `smart` 组共享；站点按目标主机名原样区分；被忽略的成员（嵌套组、内置策略、`direct` / `reject` 别名）在加载时记一行 INFO（Surge 不提示）；DNS 会话与链的中间跳不换成员；改了策略的 `test-url` / `test-timeout` 时该策略的打分从头积累；除了 5 分钟一轮，拨号时只要有能测的成员还没有结论（启动、成员新增或定义变了）也会触发一轮；"超过 12 个只测子集"按能测的成员算，也只在它们里面抽（尚未实现协议、测试 URL 解析不了的成员不占名额）；没有 UDP 信号（M5） |
```

换成

```markdown
| `smart` | 按真实连接质量动态选择：首响应延迟时间加权均值 + 重传惩罚（约每 1% 丢包 50 ms）× `policy-priority`；接近最优者构成优选集，其余为重试列表；按站点记忆约 1 小时；固定 5 分钟重测，`interval` 无效；>12 成员只测子集；忽略嵌套组与内置策略 | 🟡 | 2 | M3c 已实现，算法细节手册未公开，rurge 按手册描述近似实现（常数见 M3c 设计第 9 节）。差异：用失败罚分（每次 800 ms，5 分钟减半）近似重传率；只在拨号阶段换成员——选中的成员连不上时依次再试排在后面的两个，每次最多用"剩余时间 ÷ 剩余尝试次数"，总计 10 秒；"3 秒无响应"只计入打分，不在已建立的连接上换成员；首字节耗时与 3 秒都从出站就绪起算；分数与站点记忆按策略记，几个 `smart` 组共享；站点按目标主机名原样区分；被忽略的成员（嵌套组、内置策略、`direct` / `reject` 别名）在加载时记一行 INFO（Surge 不提示）；DNS 会话与链的中间跳不换成员；改了策略的 `test-url` / `test-timeout` 时该策略的打分从头积累；除了 5 分钟一轮，拨号时只要有能测的成员还没有结论（启动、成员新增或定义变了）也会触发一轮；"超过 12 个只测子集"按能测的成员算，也只在它们里面抽（尚未实现协议、测试 URL 解析不了的成员不占名额）；UDP（M5c）：UDP 流的载体打不开算该成员失败，第一个回包算首字节，3 秒没有回包只在目标端口是 53 或 443 时算失败（别的端口上的游戏、语音可能只发不收），UDP 不换成员；经 `udp-policy-not-supported-behaviour` 改走 DIRECT 的流不回报 |
```

API 文档（`udp` 键；`wireguard` 测速的错误例子）：

`docs/api/phase2.md`——把

````markdown
响应 `{"<名字>": Result 或 {"error": "not testable"}}`；不能测的是策略组、`REJECT` 族、尚未实现的协议，以及自己的测试 URL 解析不了的策略（即使请求里给了 `url`）。

```json
````

换成

````markdown
响应 `{"<名字>": Result 或 {"error": "not testable"}}`；不能测的是策略组、`REJECT` 族、尚未实现的协议，以及自己的测试 URL 解析不了的策略（即使请求里给了 `url`）。

UDP 测试（M5c）：策略有 UDP 测试时——策略的 `test-udp`，否则 `[General]` 的 `proxy-test-udp`——它的 Result 多一个 `udp` 键，与 URL 测试同时进行：经策略的 UDP 向 `hostname@ipv4` 的 53 端口问一次 A 记录，`{"delay": <毫秒>}` 或 `{"error": "<原因>"}`（如 `udp test timed out`），时限同该策略的测试超时；没有 UDP 测试、策略不载 UDP、不能测时没有这个键。UDP 测试的结果不保存、不影响任何组，也不进请求记录；新增的键不改变已有的形状。

```json
{"HK": {"delay": 128, "time": 1758790000.25, "udp": {"delay": 41}}}
```

```json
````

`docs/api/phase2.md`——把

```markdown
- 节里没有 `dns-server`、策略也没写 `test-url` 时，测试是一次握手：向每个 peer 强制握手，`delay` 是从强制发起到任一 peer 第一个握手完成的毫秒数——通常是一个往返，之前已有一次发起在途时（隧道刚启动、换密钥、另一条策略的测试）可能更短；它只证明 peer 可达，不证明路由与出口（照手册）。peer 不回应时 `error` 是 `timed out`；隧道起不来时是出站的错误（如 `policy protocol not implemented: wireguard over underlying-proxy`）。
```

换成

```markdown
- 节里没有 `dns-server`、策略也没写 `test-url` 时，测试是一次握手：向每个 peer 强制握手，`delay` 是从强制发起到任一 peer 第一个握手完成的毫秒数——通常是一个往返，之前已有一次发起在途时（隧道刚启动、换密钥、另一条策略的测试）可能更短；它只证明 peer 可达，不证明路由与出口（照手册）。peer 不回应时 `error` 是 `timed out`；隧道起不来时是出站的错误（如 `via Up: the underlying policy cannot carry UDP`）。
```

README：

`README.md`——把

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；M4c（external）已完成——`external` 策略（TCP；第一次用到时拉起外部程序、经它在本机端口上的 SOCKS5 转发；程序退出后下次用到时再拉起；输出写进数据目录；rurge 退出时连同它启动的进程一起停掉——Unix 用进程组，Windows 用 Job Object；三平台都支持，Surge 只在 Mac 上有）已可用，订阅导入的 `external` 一律跳过；M5a（UDP 地基）已完成——SOCKS5 监听支持 UDP ASSOCIATE，UDP 按规则分流到 DIRECT、REJECT 或 `socks5` / `socks5-tls` / `external`（`udp-relay=true`，含 `underlying-proxy` 链），全锥 NAT，每条 UDP 流一条请求记录，`block-quic` 与 `udp-policy-not-supported-behaviour` 生效；M5b（TLS 族的 UDP）已完成——`trojan`（UDP ASSOCIATE）、`anytls`（UDP over TCP v2）全锥，`vmess`（命令 2，每个目标一条连接）对称型（`wireguard` 的 UDP 在 M5c）；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

换成

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；M4c（external）已完成——`external` 策略（TCP；第一次用到时拉起外部程序、经它在本机端口上的 SOCKS5 转发；程序退出后下次用到时再拉起；输出写进数据目录；rurge 退出时连同它启动的进程一起停掉——Unix 用进程组，Windows 用 Job Object；三平台都支持，Surge 只在 Mac 上有）已可用，订阅导入的 `external` 一律跳过；M5a（UDP 地基）已完成——SOCKS5 监听支持 UDP ASSOCIATE，UDP 按规则分流到 DIRECT、REJECT 或 `socks5` / `socks5-tls` / `external`（`udp-relay=true`，含 `underlying-proxy` 链），全锥 NAT，每条 UDP 流一条请求记录，`block-quic` 与 `udp-policy-not-supported-behaviour` 生效；M5b（TLS 族的 UDP）已完成——`trojan`（UDP ASSOCIATE）、`anytls`（UDP over TCP v2）全锥，`vmess`（命令 2，每个目标一条连接）对称型；M5c（WireGuard 的 UDP 与其余）已完成——`wireguard` 的 UDP（全锥）与经 `underlying-proxy` 的隧道、`test-udp` / `proxy-test-udp`、`smart` 组计入 UDP、`dns-follow-interface`；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

`README_en.md`——把

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; M4b (WireGuard) is done — the `wireguard` policy (TCP; a user-space tunnel, no virtual interface; several peers chosen by `allowed-ips`; `client-id` (WARP's reserved bytes); destination names looked up through the tunnel when the section has a `dns-server`, on this machine otherwise; tested by a handshake when there is neither a `dns-server` nor a `test-url`) is usable; M4c (external) is done — the `external` policy (TCP; the program is started on first use and reached as a SOCKS5 proxy on its local port; a program that exited is started again on the next use; its output goes to the data directory; when rurge exits, the program is stopped together with whatever it started — a process group on Unix, a Job Object on Windows; all three platforms, where Surge has it on the Mac only) is usable, and subscriptions never bring `external` policies in; M5a (the UDP foundation) is done — the SOCKS5 listener takes UDP ASSOCIATE, and UDP is routed by rule to DIRECT, REJECT or `socks5` / `socks5-tls` / `external` (`udp-relay=true`, `underlying-proxy` chains included), full-cone NAT, one request record per UDP flow, and `block-quic` and `udp-policy-not-supported-behaviour` take effect; M5b (UDP over the TLS family) is done — `trojan` (UDP ASSOCIATE) and `anytls` (UDP over TCP v2) with full-cone NAT, `vmess` (command 2, one connection per target) symmetric (UDP over `wireguard` comes in M5c); the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

换成

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; M4b (WireGuard) is done — the `wireguard` policy (TCP; a user-space tunnel, no virtual interface; several peers chosen by `allowed-ips`; `client-id` (WARP's reserved bytes); destination names looked up through the tunnel when the section has a `dns-server`, on this machine otherwise; tested by a handshake when there is neither a `dns-server` nor a `test-url`) is usable; M4c (external) is done — the `external` policy (TCP; the program is started on first use and reached as a SOCKS5 proxy on its local port; a program that exited is started again on the next use; its output goes to the data directory; when rurge exits, the program is stopped together with whatever it started — a process group on Unix, a Job Object on Windows; all three platforms, where Surge has it on the Mac only) is usable, and subscriptions never bring `external` policies in; M5a (the UDP foundation) is done — the SOCKS5 listener takes UDP ASSOCIATE, and UDP is routed by rule to DIRECT, REJECT or `socks5` / `socks5-tls` / `external` (`udp-relay=true`, `underlying-proxy` chains included), full-cone NAT, one request record per UDP flow, and `block-quic` and `udp-policy-not-supported-behaviour` take effect; M5b (UDP over the TLS family) is done — `trojan` (UDP ASSOCIATE) and `anytls` (UDP over TCP v2) with full-cone NAT, `vmess` (command 2, one connection per target) symmetric; M5c (UDP over WireGuard and the rest) is done — UDP over `wireguard` (full cone) and tunnels over an `underlying-proxy`, `test-udp` / `proxy-test-udp`, UDP in `smart` groups, and `dns-follow-interface`; the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

`CLAUDE.md`（当前状态、文档清单、常用命令）：

`CLAUDE.md`——把

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、发送时本机地址或路由失效或握手无回应时换新载体（网络变化后自愈）、网络变化入口、同一私钥与 peer 只留一条隧道，节与载体设置都相同的策略共用）、`stream`、`dns`（隧道内 DNS 与缓存；每个问题 2 秒内重发、先等隧道第一次握手）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。M4c（external）已完成——`rurge-config::spec::ExternalSpec`（`args` 是 `Secret`；两个 `external` 策略写了同一个 `local-port` 是 `E0018`；`args` 进内联参数的脱敏名单）、订阅导入的 `external` 行一律跳过（`W0023`）；`rurge-platform::process`（Unix 进程组与 `killpg`，Windows Job Object）；`rurge_proto::external`（`ExternalOutbound`：第一次用到时拉起、输出写 `<数据目录>/external/<策略名>.log` 且超过 1 MiB 时在拉起前轮转、子进程环境去掉代理变量并设 `NO_PROXY=*`、程序退出后下次用到时再拉起且两次拉起至少隔 2 秒、每 500 ms 连一次本机端口最多 6 次；`ProcessHook` / `ProcessGroup` 与 `NoProcessGroups`；程序自己退出时连同它启动的进程一起结束）；`rurge-engine` 的工厂分支、`EngineShared.processes` / `externals` 与 `Engine::stop_external_programs`；bin 的 `PlatformProcesses` 与退出流程最后停掉全部外部程序；只用于测试的工作区成员 `tests/external`（`rurge-external-tests`：极小的 SOCKS5 辅助程序 `socks-helper` 与真实拉起它的用例）；能力表翻转 `external`（`W0007` 不再因 `external` 出现）。M4 至此完成。M5（UDP 路径）按三份计划推进（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）：M5a 已完成——`rurge_net::connector::PacketSocket`（按包收发、带地址的 UDP 载体）与 `Connector::open_udp`（`DirectConnector`：每个地址族一个未连接的 socket，忽略 Windows 的 ICMP 不可达报错）；`Outbound::udp()` / `open_udp()` 与 `UdpSupport`；DIRECT 与 `socks5` / `socks5-tls` / `external` 的 UDP（`udp-relay`，`W0029` 退役）；`ChainConnector::open_udp`（链式 UDP 载体）；`rurge-inbound` 的 SOCKS5 UDP ASSOCIATE（`UdpClient`、`UdpAdmission`、`Dialer::admit_udp` / `associate`）；`rurge-engine` 的 UDP 流水线（`udp` 模块：一条流 = 关联 + 目标、按"关联 × 出站"共用载体的全锥、60 秒 / DNS 10 秒回收、1024 流 / 4096 关联的上限）、请求记录与 API 的 `transport`、`block-quic`（`W0029` 退役）与 `udp-policy-not-supported-behaviour`、QUIC Initial 识别、`PROTOCOL` 规则按传输层匹配 `TCP` / `UDP`；`FakeSocks5` 与 `tests/external` 辅助程序的 UDP ASSOCIATE；对 sing-box `socks` 入站的 UDP 互操作用例。M5b（TLS 族的 UDP）已完成——`rurge-proto` 的 `stream_udp`（一条字节流上按包收发：请求头随第一个包发出、写那个包的目标；`trojan` 的 UDP ASSOCIATE 与 `anytls` 的 UDP over TCP v2 两种封装）、`trojan` / `anytls` 的 `open_udp`（全锥）、`vmess` 的命令 2（`vmess::udp`：每个目标一条 VMess 连接、随它的第一个包建立、每个数据报一个分块，对称型）；`FakeTrojan` / `FakeAnyTls` / `FakeVmess` 的 UDP 与 `rurge_proto::testing::udp_echo_server`；经引擎的端到端用例（`tests/udp_tls_family.rs`）；对 sing-box（三种）与 xray（vmess）的 UDP 互操作用例。
```

换成

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、发送时本机地址或路由失效或握手无回应时换新载体（网络变化后自愈）、网络变化入口、同一私钥与 peer 只留一条隧道，节与载体设置都相同的策略共用）、`stream`、`dns`（隧道内 DNS 与缓存；每个问题 2 秒内重发、先等隧道第一次握手）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。M4c（external）已完成——`rurge-config::spec::ExternalSpec`（`args` 是 `Secret`；两个 `external` 策略写了同一个 `local-port` 是 `E0018`；`args` 进内联参数的脱敏名单）、订阅导入的 `external` 行一律跳过（`W0023`）；`rurge-platform::process`（Unix 进程组与 `killpg`，Windows Job Object）；`rurge_proto::external`（`ExternalOutbound`：第一次用到时拉起、输出写 `<数据目录>/external/<策略名>.log` 且超过 1 MiB 时在拉起前轮转、子进程环境去掉代理变量并设 `NO_PROXY=*`、程序退出后下次用到时再拉起且两次拉起至少隔 2 秒、每 500 ms 连一次本机端口最多 6 次；`ProcessHook` / `ProcessGroup` 与 `NoProcessGroups`；程序自己退出时连同它启动的进程一起结束）；`rurge-engine` 的工厂分支、`EngineShared.processes` / `externals` 与 `Engine::stop_external_programs`；bin 的 `PlatformProcesses` 与退出流程最后停掉全部外部程序；只用于测试的工作区成员 `tests/external`（`rurge-external-tests`：极小的 SOCKS5 辅助程序 `socks-helper` 与真实拉起它的用例）；能力表翻转 `external`（`W0007` 不再因 `external` 出现）。M4 至此完成。M5（UDP 路径）按三份计划推进（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）：M5a 已完成——`rurge_net::connector::PacketSocket`（按包收发、带地址的 UDP 载体）与 `Connector::open_udp`（`DirectConnector`：每个地址族一个未连接的 socket，忽略 Windows 的 ICMP 不可达报错）；`Outbound::udp()` / `open_udp()` 与 `UdpSupport`；DIRECT 与 `socks5` / `socks5-tls` / `external` 的 UDP（`udp-relay`，`W0029` 退役）；`ChainConnector::open_udp`（链式 UDP 载体）；`rurge-inbound` 的 SOCKS5 UDP ASSOCIATE（`UdpClient`、`UdpAdmission`、`Dialer::admit_udp` / `associate`）；`rurge-engine` 的 UDP 流水线（`udp` 模块：一条流 = 关联 + 目标、按"关联 × 出站"共用载体的全锥、60 秒 / DNS 10 秒回收、1024 流 / 4096 关联的上限）、请求记录与 API 的 `transport`、`block-quic`（`W0029` 退役）与 `udp-policy-not-supported-behaviour`、QUIC Initial 识别、`PROTOCOL` 规则按传输层匹配 `TCP` / `UDP`；`FakeSocks5` 与 `tests/external` 辅助程序的 UDP ASSOCIATE；对 sing-box `socks` 入站的 UDP 互操作用例。M5b（TLS 族的 UDP）已完成——`rurge-proto` 的 `stream_udp`（一条字节流上按包收发：请求头随第一个包发出、写那个包的目标；`trojan` 的 UDP ASSOCIATE 与 `anytls` 的 UDP over TCP v2 两种封装）、`trojan` / `anytls` 的 `open_udp`（全锥）、`vmess` 的命令 2（`vmess::udp`：每个目标一条 VMess 连接、随它的第一个包建立、每个数据报一个分块，对称型）；`FakeTrojan` / `FakeAnyTls` / `FakeVmess` 的 UDP 与 `rurge_proto::testing::udp_echo_server`；经引擎的端到端用例（`tests/udp_tls_family.rs`）；对 sing-box（三种）与 xray（vmess）的 UDP 互操作用例。M5c（WireGuard 的 UDP 与其余）已完成——`rurge-proto-wireguard` 的 `TunnelUdp`（隧道里每个地址族一个 UDP socket、第一次发往该族时绑定、全锥，目标名经隧道 DNS 或本机解析）与 `Stack::udp_bind` / `check`；`rurge_net::packet_datagram`（把 `PacketSocket` 变成一条到固定目标的 `Datagram`）与 `ChainConnector::connect_udp`，`wireguard` 的载体经 `underlying-proxy`（底层策略不载 UDP 时拨号失败，`W0029` 退役）；启动时 peer 连不上的告警每个策略的每个 peer 5 分钟至多一次（M4b 延后事项 #15）；`rurge_policy::udp_probe`（经策略的 UDP 向 `hostname@ipv4` 问一次 A 记录）、`Engine::test_udp` 与 `POST /v1/policies/test` 结果里的 `udp` 键（`test-udp` / `proxy-test-udp`，不保存、不参与组的选择）；`smart` 计入 UDP（载体打不开算失败、第一个回包算首字节、3 秒无回包只在 53 / 443 端口算失败、UDP 不换成员）；`dns-follow-interface`（`rurge_net::connector::Via` / `ResolveVia`、`Resolver::lookup_via`：策略自己的解析经它的 `interface` 问普通 DNS 服务器、答案另存，配了加密 DNS 时不跟随；没有 `interface` 时 `W0028`）；对 sing-box WireGuard 端点的 UDP 互操作。M5 至此完成。
```

`CLAUDE.md`——把

```markdown
- `docs/superpowers/specs/2026-09-29-phase2-m5-udp-design.md`：阶段 2 / M5 细化设计（UDP 路径），细化总设计的 M5 里程碑、不一致处以它为准。三份计划的拆分（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）；已决事项 M5-D1 ～ D11（三类用途都要、全锥 NAT、按包收发的 `PacketSocket`、VMess 先做对称型、`ecn` 延后、60 秒 / DNS 10 秒回收、QUIC 只在 UDP 443 上识别、全局 `block-quic` 按取值名理解、REJECT 对 UDP 一律丢包、1024 流 / 4096 关联的上限）；第 15 节 V1 ～ V10 是写各份计划时必须核对的事项，第 16 节是任务草图，第 17 节是 M5a 计划期的订正，第 18 节是 M5a 实施期的订正，第 19 节是 M5b 计划期的订正，第 20 节是 M5b 实施期的订正。
- `docs/superpowers/plans/2026-09-29-phase2-m5a-udp-foundation-plan.md`：阶段 2 / M5a（UDP 地基）实施计划（7 个任务）。开头「计划期决定」表记录核对源码与手册得出的结论和与设计文字不同的决定；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/superpowers/plans/2026-09-29-phase2-m5b-udp-tls-family-plan.md`：阶段 2 / M5b（TLS 族的 UDP）实施计划（4 个任务）。开头「计划期决定」表记录核对参考实现源码（sing 的 `uot`、sing-vmess、trojan-gfw 协议文档）得出的逐字节细节与和设计文字不同的决定；末尾「执行期修正记录」与「延后事项」两张表。
```

换成

```markdown
- `docs/superpowers/specs/2026-09-29-phase2-m5-udp-design.md`：阶段 2 / M5 细化设计（UDP 路径），细化总设计的 M5 里程碑、不一致处以它为准。三份计划的拆分（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）；已决事项 M5-D1 ～ D11（三类用途都要、全锥 NAT、按包收发的 `PacketSocket`、VMess 先做对称型、`ecn` 延后、60 秒 / DNS 10 秒回收、QUIC 只在 UDP 443 上识别、全局 `block-quic` 按取值名理解、REJECT 对 UDP 一律丢包、1024 流 / 4096 关联的上限）；第 15 节 V1 ～ V10 是写各份计划时必须核对的事项，第 16 节是任务草图，第 17 节是 M5a 计划期的订正，第 18 节是 M5a 实施期的订正，第 19 节是 M5b 计划期的订正，第 20 节是 M5b 实施期的订正，第 21 节是 M5c 计划期的订正。
- `docs/superpowers/plans/2026-09-29-phase2-m5a-udp-foundation-plan.md`：阶段 2 / M5a（UDP 地基）实施计划（7 个任务）。开头「计划期决定」表记录核对源码与手册得出的结论和与设计文字不同的决定；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/superpowers/plans/2026-09-29-phase2-m5b-udp-tls-family-plan.md`：阶段 2 / M5b（TLS 族的 UDP）实施计划（4 个任务）。开头「计划期决定」表记录核对参考实现源码（sing 的 `uot`、sing-vmess、trojan-gfw 协议文档）得出的逐字节细节与和设计文字不同的决定；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/superpowers/plans/2026-09-30-phase2-m5c-udp-wireguard-rest-plan.md`：阶段 2 / M5c（WireGuard 的 UDP 与其余）实施计划（6 个任务）。开头「计划期决定」表记录核对 smoltcp / 本仓库与手册得出的结论和与设计文字不同的决定（隧道里每个地址族一个 UDP socket、链上的 UDP 经 `packet_datagram`、底层策略不载 UDP 时拨号失败而不是 REJECT、M4b #24 已无对象、UDP 测试另走 `Engine::test_udp` 且只加 `udp` 键、`smart` 的 3 秒只在 53 / 443、`dns-follow-interface` 覆盖策略自己的全部解析但不跟随加密 DNS 等）与「承接事项」；末尾「执行期修正记录」与「延后事项」两张表。
```

`CLAUDE.md`——把

```markdown
cargo test -p rurge-engine --test outbounds_wireguard   # 经 wireguard 出站的端到端用例：回显、[Host] 解析、underlying-proxy 的 REJECT、重载沿用与替换、握手测速与经隧道的 URL 测速
```

换成

```markdown
cargo test -p rurge-engine --test outbounds_wireguard   # 经 wireguard 出站的端到端用例：回显、[Host] 解析、经 underlying-proxy（socks5）的隧道与底层不载 UDP 时的失败、重载沿用与替换、握手测速与经隧道的 URL / UDP 测速、经隧道的 UDP
```

`CLAUDE.md`——把

```markdown
cargo test -p rurge-engine --test udp_tls_family   # 经 trojan / anytls（全锥）与 vmess（每个目标一条连接、对称）的 UDP 端到端用例（回环假服务端）
```

换成

```markdown
cargo test -p rurge-engine --test udp_tls_family   # 经 trojan / anytls（全锥）与 vmess（每个目标一条连接、对称）的 UDP 端到端用例（回环假服务端）
cargo test -p rurge-policy udp_probe           # UDP 测试：A 记录的问题、计时、静默与不载 UDP
```

手工验收：

`docs/acceptance/phase2-manual.md`——把

```markdown
- [ ] WebSocket 与 TLS：`trojan` 或 `vmess` 节点开 `ws=true` 时 UDP 照常往返。

```

换成

```markdown
- [ ] WebSocket 与 TLS：`trojan` 或 `vmess` 节点开 `ws=true` 时 UDP 照常往返。

## M5c　WireGuard 的 UDP 与其余

前置：同 M5a 一节的 SOCKS5 UDP 客户端；自己的 WireGuard 服务端（记下是哪个实现与版本）；一个开了 UDP 的上游 SOCKS5 节点；一台有两个网卡的机器（做 `dns-follow-interface`）。

- [ ] WireGuard 的 UDP：规则把 UDP 分到 `wireguard` 策略，经 SOCKS5 UDP 发 DNS 查询（如 Proxifier 代理 `nslookup example.com 1.1.1.1`）得到回答；经它进行一次语音通话或联机游戏；用 NAT 类型检测工具（STUN）检测，结果是 Full Cone（服务端的出口须是全锥）。
- [ ] 经底层策略的隧道：`wireguard` 策略写 `underlying-proxy=<SOCKS5 节点>`（节点 `udp-relay=true`），TCP 与 UDP 都能经隧道往返；把底层策略换成一条 `http` 策略，拨号失败，请求记录写 `via <底层策略>: the underlying policy cannot carry UDP`。
- [ ] UDP 测试：给一个节点写 `test-udp=apple.com@8.8.8.8`，`POST /v1/policies/test` 的结果里有 `udp` 键与延迟；不写 `test-udp`、写 `[General] proxy-test-udp` 时同样有；`http` 策略的结果里没有 `udp` 键。
- [ ] `smart` 与 UDP：`smart` 组里放一个 UDP 通的节点与一个 UDP 不通的节点（服务端关掉 UDP），经这个组发一阵 DNS 查询后，UDP 不通的节点的站点记忆里记为失败（日志的健康切换），通的节点记为成功。
- [ ] `dns-follow-interface`：`direct` 策略写 `interface=<第二块网卡>, dns-follow-interface=true`，`dns-server` 写一个只经第二块网卡可达的 DNS 服务器（或用抓包确认），规则把某个域名分到这条策略，访问它：抓包看到 DNS 查询从第二块网卡发出；配了 `encrypted-dns-server` 时查询照常走加密 DNS，日志有一条 `dns-follow-interface: the encrypted DNS servers are asked as usual` 的说明。

```

- [ ] **Step 4: 核对**

- 兼容性清单里不再有对 M5 的前瞻说法：`grep -n "（M5）\|M5 生效\|属 M5\|M5 之前" docs/surge-compatibility-matrix.md` 没有输出。
- `README.md` 与 `README_en.md` 的状态一段内容一致。

- [ ] **Step 5: 门禁与提交**

跑门禁。副本上最后一次全工作区门禁：fmt、clippy 通过，`cargo test --workspace --no-fail-fast` 53 个测试二进制、1261 通过、0 失败、2 忽略。

```bash
git add tests/interop docs README.md README_en.md CLAUDE.md
git commit -m "docs: M5c WireGuard 的 UDP 与其余——兼容性清单、API、手工验收、README 与 CLAUDE.md；sing-box WireGuard 端点的 UDP 互操作"
```

## 验收对照（设计第 11 节，M5c 部分）

| # | 验收项 | 由谁保证 |
| - | ------ | -------- |
| 1 | 经 rurge 的 SOCKS5 UDP，`wireguard` 对回环假对端往返；全锥用例通过；对 sing-box 的互操作在 CI 上通过 | Task 1：`udp_leaves_through_the_tunnel`（引擎）、`udp_goes_through_the_tunnel`、`anyone_in_the_tunnel_may_answer`、`a_datagram_to_a_name_goes_where_the_name_says`；Task 2：`a_tunnel_goes_over_an_underlying_socks5_proxy`；Task 6：`a_connection_goes_through_a_sing_box_wireguard_endpoint` 的 UDP 往返，CI |
| 2 | `block-quic` | 不在本计划（M5a）；`wireguard` 的 QUIC 流照 M5a 的规则判定 |
| 3 | 不支持 UDP 的策略按 `udp-policy-not-supported-behaviour` 处理 | 不在本计划（M5a）；`wireguard` 从此支持 UDP；底层策略不载 UDP 时隧道拨号失败（Task 2 `a_tunnel_over_an_underlying_proxy_without_udp_fails_saying_so`） |
| 4 | `W0029` 不再因 `udp-relay`、`block-quic`、`test-udp`、`dns-follow-interface` 出现（`ecn` 除外） | Task 3：`every_common_parameter_is_parsed`（不生效名单只剩 `tfo` 与 `ecn`，Task 5 之后）；Task 5：`dns_follow_interface_without_an_interface_is_ignored`（没有 `interface` 时是 `W0028`）；Task 2：`underlying_proxy_on_a_wireguard_policy_is_accepted` |
| 5 | 门禁全绿 | 各任务的门禁 |
| 6 | 需要真实环境的项目进手工验收清单 | Task 6：`docs/acceptance/phase2-manual.md` 的 M5c 一节 |
| — | 设计 8.2（M4b #15 / #24） | Task 2：`a_peer_that_cannot_be_reached_is_said_so_once_in_a_while`；#24 已无对象（P6） |
| — | 设计 8.3（`smart` 计入 UDP） | Task 4：`a_udp_answer_is_reported`、`a_member_whose_udp_carrier_does_not_open_counts_against_it`、`udp_silence_counts_only_where_answers_always_come`、`silence_that_tells_nothing_is_no_failure` |
| — | 设计 8.4（UDP 测试） | Task 3：`a_udp_test_asks_through_the_tunnel`、`the_question_asks_for_a_records`、`an_answer_is_timed`、`silence_and_no_udp_fail` |
| — | 设计 8.5（`dns-follow-interface`） | Task 5：`lookups_via_an_interface_go_through_it_with_answers_of_their_own`、`with_encrypted_dns_lookups_via_an_interface_go_as_usual`、`a_policy_that_follows_its_interface_looks_up_through_it` |

## 执行期修正记录

| # | 任务 | 与计划的出入 | 原因 |
| - | ---- | ------------ | ---- |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | `dns-follow-interface` 不跟随加密 DNS：配了 `encrypted-dns-server` 时策略自己的解析照常问加密 DNS（P11） | 有用户要求时另建一套经网卡的加密上游 |
| 2 | 规则匹配时的解析不跟随任何策略的网卡（选定策略之前发生；与手册一致） | 接受 |
| 3 | UDP 测试的结果不保存、不在 `GET /v1/policy_groups/test_results` 里出现、不进请求记录（P7） | 有展示需求时另议 |
| 4 | 隧道里 UDP socket 的缓冲固定为 64 个包 / 256 KiB（设计第 7 节）；发送缓冲满时静默丢包，没有计数 | 有吞吐问题时 |
| 5 | `ecn` 仍只解析（M5-D6） | 随 QUIC 族 |
| 6 | `udp::a_closed_port_does_not_break_the_carrier`（M5a）在并行负载下偶发失败：它释放的"关着的端口"可能被并行的用例绑上（P13） | 以后顺手改（例如用一个绑着却从不读的 socket 代替释放的端口——Windows 上它不会触发 ICMP 不可达，要另想办法） |
| 7 | `packet_datagram` 发送失败只记 `trace!`，名字目标的回包不按来源过滤（P3） | 接受 |
