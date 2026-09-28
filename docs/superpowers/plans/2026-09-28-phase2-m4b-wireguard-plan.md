# 阶段 2 / M4b「WireGuard 出站」Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现 `wireguard` 出站（TCP）：`[WireGuard <name>]` 节的类型化与校验；用户态 WireGuard 隧道（boringtun 的 sans-IO `Tunn` 加 smoltcp 协议栈，不建虚拟网卡）；多 peer 按 `allowed-ips` 最长前缀选路、`client-id`（WARP 保留字节）、握手包 DSCP；目标域名经隧道内 DNS 或在本机解析；按需启动、endpoint 跟随、网络变化入口、同一私钥与 peer 只留一条隧道；原生握手测速；经引擎的端到端、DNS 会话防环、`underlying-proxy` 的 REJECT；对 sing-box WireGuard 端点的互操作；能力表翻转 `wireguard`。

**Architecture:** `rurge-config` 新增 `wireguard`（`WireGuardSection` 与 `parse_section`，`E0023`）与 `spec::wireguard`（`WireGuardSpec`：spec 带着节的全部内容）；`rurge-net` 新增 UDP 载体（`Datagram`、`Connector::connect_udp`、`DirectConnector::connect_udp`）。新 crate `rurge-proto-wireguard`（`→ rurge-proto → rurge-net → rurge-config`，只被 `rurge-engine` 依赖）：`routes`（两张最长前缀表）、`wire`（报文类型与 `client-id`）、`stack`（锁里的全部状态：smoltcp 的接口与套接字、收发两个包队列、每个 peer 一个 `Tunn`；不做 I/O）、`device`（每条隧道一个任务：独占各 peer 的载体，锁外收发，按 250 ms 与 smoltcp 的截止时间推进；隧道表保证同一私钥与 peer 只留一条）、`stream`（每条 TCP 连接一个 smoltcp 套接字，读写直接在锁里操作缓冲）、`dns`（隧道内 DNS 与缓存）、`outbound`（`WireGuardOutbound`）与 `testing`（`PeerCore` / `FakeWgPeer`：boringtun 响应端加它自己的 smoltcp 主机，只在回环）。`rurge-proto` 的 `Outbound` 多一个可选的 `native_test`，`rurge-policy` 的测速多一种原生模式（`TestMode`）；`rurge-engine` 的工厂多一个分支；bin 关掉 boringtun 自己的日志。

**Tech Stack:** Rust 1.89 / edition 2024；新依赖 **boringtun 0.7.1**（`default-features = false`，只用 `noise::Tunn`）与 **smoltcp 0.12.0**（`default-features = false`，特性见 P2）；`Cargo.lock` 从 474 个包变为 505 个；其余用工作区已有的 `tokio`、`tokio-util`、`tracing`、`prefix-trie`、`ipnet`、`getrandom`、`hickory-proto`、`socket2`、`base64`。

**Spec:** `docs/superpowers/specs/2026-09-27-phase2-m4-wireguard-ssh-external-design.md`（第 2 节 M4-D1、D3 ～ D5、D7 ～ D9；第 4.1、4.3、4.6 ～ 4.8 节中 `wireguard` 的部分；第 6 节；第 8 ～ 12 节中 WireGuard 的部分；第 15 节 V3 ～ V8、V12；第 16 节 M4b 草图）；总设计 `docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md` 第 16 节 Q4、风险 G；M4a 计划 `docs/superpowers/plans/2026-09-27-phase2-m4a-ssh-plan.md` 末尾「延后事项」#1。与本计划「计划期决定」表不一致处，以该表为准，并由 Task 10 写回设计文档新增的第 19 节。

## Global Constraints

- MSRV **1.89**，edition 2024。`unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 是 `deny` + 唯一一个 `#[allow(unsafe_code)]` 函数。**本计划不新增任何 unsafe**（新 crate 用 `[lints] workspace = true` 继承 `forbid`）。
- 依赖方向：`rurge-proto-wireguard → rurge-proto → rurge-net → rurge-config`；`rurge-engine` 依赖 `rurge-proto-wireguard`；`rurge-policy` 不依赖任何协议实现；平台代码只在 `rurge-platform`（本计划不碰它）。**新第三方依赖只有 boringtun 0.7.1 与 smoltcp 0.12.0**：boringtun `default-features = false`（默认的 `device` 是一整套 TUN 驱动）；smoltcp `default-features = false` 且只开 P2 列的特性（不开 `phy-*`、`log`、`socket-tcp-cubic`）；smoltcp 0.13 起要求 Rust 1.91，不升级（M4-D3）。
- **测试绝不碰公网**：只用回环 + 端口 0 + 有界等待；不用固定 sleep 当同步手段（轮询 + 截止时间；只有"断言这段时间里什么也没发生"时才等一段固定时间）。WireGuard 的对端只用回环上的 `FakeWgPeer`，或 `tests/interop` 在 CI 上拉起的 sing-box（P8）。**任何带 `url-test` / `fallback` / `load-balance` / `smart` 组的测试配置，`proxy-test-url` 与 `internet-test-url` 都必须指向回环**——引擎用例的 `Profile::text` 已默认指向 `http://127.0.0.1:9/`，不要删掉。
- **测试与任何命令都不得修改本机的系统代理、注册表、网络设置，不得注册真实服务 / 计划任务，不得创建网卡或改路由**：不在 CLI 测试夹具之外运行 `rurge run --system-proxy`；不运行不带 `--dry-run` 的 `rurge service install | uninstall`；sing-box 的 WireGuard 端点只以 `system: false`（用户态）渲染。
- **不在本机下载或安装任何东西**（不装 sing-box、xray、`sshd`、WireGuard 工具，不 `rustup target add`、不 `cargo install`）。唯一的例外：首次构建时 cargo 从 crates.io 下载 boringtun、smoltcp 及其依赖（项目所有者已同意，M4-D3 / D4）。
- **密钥与凭据永不外泄**：`private-key`、`preshared-key` 不进日志、错误文本、API 输出与 `Debug`（`WireGuardSection` 的两个字段是 `Secret`）；节内的错误只点名键，不引用取值；日志只带策略名与 peer 序号（设计第 9 节）；订阅行设的 `test-url` 不进日志。
- rurge 专有的运行时选项只经命令行与环境变量提供，不扩展 Surge 配置格式（FR-CFG-17）。与 Surge 的每一处行为差异都登记进 `docs/surge-compatibility-matrix.md`（Task 10）。
- 日志、错误文本、CLI 输出、代码注释用英文；文档与提交标题用中文。注释密度、命名、惯用法与相邻代码保持一致；注释里不写评审轮次的标签。
- 提交信息结尾两行：`Co-Authored-By: <执行者自己的署名>` 与 `Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th`。**不 push、不 merge**，不 amend。
- 每个任务结束的门禁（全部通过才算完成）：

  ```bash
  RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check \
    && cargo clippy --all-targets -- -D warnings \
    && timeout 1500 cargo test --workspace --no-fail-fast
  ```

  `timeout` 不能省：`rurge-dns` 的一个用例曾让测试进程以 100% CPU 空转数小时（M3a「延后事项」#20）；写本计划时又遇到一次（`resolver::tests` 的 `aaaa_suppression_after_five_timeouts_and_flush_resumes` 与 `cache_fresh_stale_negative_and_coalescing`，与本计划的改动无关）。测试二进制异常退出而没有失败用例时（`STATUS_ACCESS_VIOLATION`、`STATUS_HEAP_CORRUPTION` / `0xc0000374`、段错误——本机已知的既有问题，M3b 计划 P21；写本计划时新 crate `rurge-proto-wireguard` 的测试二进制也遇到过一次），或整轮被 `timeout` 杀掉时，重跑一次并保留两次的日志，**不要在任务里去修它**。已知偶发失败的计时类用例（`rurge-dns` 的 `a_partial_result_completes_aaaa_in_the_background` 与 `bootstrap::tests::stale_entries_are_served_and_refreshed_once`、`rurge` 的 `run::watch_reloads_rules_on_change`）同样重跑。
- 第三方 crate 的 API 以本机源码为准：`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`（`boringtun-0.7.1`、`smoltcp-0.12.0`、`tokio-1.53.1`）。
- 本机的 bash 处理不了超过约 8 KB 或含反斜杠的 heredoc（`\\` 会被改写）：新文件一律用写文件的工具落盘，不用 heredoc。
- 构建目录会很快变大（每个任务的全量测试都要重编大半个工作区）：磁盘吃紧时删掉 `target/debug/incremental`，它只影响下次增量编译的速度。

## Review Focus

设计没有逐条写到、而最可能伤到使用者的五类输入或失败方式；每一条都在负责它的任务里配了用例。

1. **大流量**（下载、上传大文件）：不能卡住，也不能慢到每秒几十 KB——smoltcp 0.12 一次 `poll` 每条连接只发一个报文段，它的 Cubic 又把窗口压在一两个报文段；丢包之后是整窗重发，所以收端不能因为缓冲太小而丢包。用例：Task 3 `a_window_of_data_leaves_at_once`（一轮发完一整窗、拥塞控制是 Reno）；Task 4 `a_large_transfer_goes_through_whole`（2 MiB 双向、流控与收尾）；Task 2 `a_udp_flow_has_room_for_a_burst`；Task 7 的吞吐基准（忽略的用例，数字记进本计划）。
2. **改了 `[WireGuard]` 节之后重载，或两条策略指向同一个节**：同一私钥连着同一 peer 的两条隧道会互相抢 peer 记住的地址，新连接会莫名其妙卡 5 秒以上。用例：Task 6 `a_later_configuration_of_a_tunnel_takes_over`、`policies_that_name_one_section_share_its_tunnel`、`one_key_at_two_peers_is_two_tunnels`；Task 7 `a_reload_keeps_the_tunnel_unless_its_section_changes`。
3. **目标不在任何 peer 的 `allowed-ips` 里、隧道没有该地址族的地址、目标端口关着、peer 不回应**：立即得到说得清的错误（或在拨号时限处超时），绝不改走直连，也不挂住。用例：Task 4 `names_and_addresses_the_tunnel_cannot_use_fail_at_once`、`a_closed_port_refuses_the_connection`、`a_peer_that_never_answers_runs_the_dial_out_of_time`；Task 7 `a_tunnel_over_underlying_proxy_rejects_with_a_note`。
4. **endpoint 写成域名，又开了 `encrypted-dns-follow-outbound-mode`**：DNS 查询经 `wireguard` 出去，而启动隧道本身要解析 endpoint——不能互相等死。用例：Task 7 `a_dns_session_bypasses_a_wireguard_policy_whose_endpoint_is_a_host_name`。
5. **WARP（`client-id`）**：发出的每个报文都带上它，收到的报文先清零再解——否则握手成功不了。用例：Task 3 `the_client_id_goes_out_in_every_message_and_is_cleared_coming_in`、`without_the_client_id_such_a_peer_never_answers`；Task 4 `every_message_carries_the_client_id`；Task 9 对 sing-box（它给回应写保留字节）的互操作。

另有几条同样配了用例、但不那么常见的：endpoint 的解析结果变了（Task 6 `an_endpoint_written_as_a_name_is_followed`）；启动时有的 peer 连不上（Task 6 `a_peer_unreachable_at_the_start_is_dialled_again`）；同时进来的一批拨号只启动一条隧道、握手一次（Task 4 `dials_that_come_together_share_one_tunnel`）；订阅行想用主配置里的节——整行跳过（Task 1 `a_subscription_wireguard_line_may_not_use_the_profiles_sections`）。

## 计划期决定

写计划时对照设计、Surge 手册（`policies/wireguard.html`，2026-09-28 读取）、boringtun 0.7.1 / smoltcp 0.12.0 / hickory-proto / tokio 源码与本仓库源码核对后定下的事；与设计文档文字不同的，由 Task 10 写回设计文档第 19 节。

**本计划里的代码不是凭空写的。** 全部 10 个任务的改动在仓库的一份副本上按任务顺序真实做了一遍（副本用自己的构建目录，不与本仓库的 `target/` 混用），每个任务之后跑一次全工作区门禁：最后一次是 **49 个测试二进制，1121 通过 / 0 失败 / 2 忽略**（本计划开工前的 main 是 45 个测试二进制，1040 通过 / 1 忽略）。计划里新文件的全文取自副本上该任务的提交，修改处的"把 … 换成 …"由脚本从相邻两个任务提交的差异生成，并在拼好之后按计划的顺序套到开工前的源码上逐字核对过——计划文本与验证过的代码一字不差。每个任务 Step 2 的"预期失败"是只把该任务的用例块（及写明的前置改动）套到上一个任务的状态上、真实跑出来的。做的过程中发现的问题（P2、P3、P9、P10）已经改在了它们所属的任务里，数字与现象都记在对应的决定里。

| # | 事项 | 决定与依据 |
| - | ---- | ---------- |
| P1 | V3：boringtun 0.7.1 | 只用 `noise::Tunn`：`Tunn::new(私钥, peer 公钥, preshared, keepalive, index, None)`，index 取一个随机基数加 peer 序号（24 位），同一进程的几条隧道互不相撞，限速器用 boringtun 自己的。收发缓冲 65536 + 32 字节（`encapsulate` 要求不小于输入加 32、不小于 148）。`decapsulate` 返回 `WriteToNetwork` 之后以空输入再调，直到 `Done`——握手期间排队的包这时才出去。`update_timers` 每 250 ms 一次；它返回的 `Err(ConnectionExpired)` 表示握手 90 秒无回应（`REKEY_ATTEMPT_TIME`）或会话闲置到期，**此后每次都返回**，直到下一个包开始新的握手。"握手完成"= 类型 2（握手回应）的报文解出 `WriteToNetwork`（boringtun 随即回一个 keepalive）。报文头 4 字节被当作类型读：`client-id` 写在第 1 ～ 3 字节，收到的报文必须先清零（P15）。boringtun 自己用 `tracing` 记日志（`HANDSHAKE(REKEY_TIMEOUT)` 为 warn、`CONNECTION_EXPIRED` 为 error），不带策略名（P13） |
| P2 | V4：smoltcp 0.12.0 | 特性：`std` `medium-ip` `proto-ipv4` `proto-ipv6` `proto-ipv4-fragmentation` `proto-ipv6-fragmentation` `socket-tcp` `socket-tcp-reno` `async` `reassembly-buffer-size-16384` `reassembly-buffer-count-4`，Task 5 加 `socket-udp`。设备是内存里的收、发两个包队列（`Medium::Ip`，MTU 取节的 `mtu`）；接口地址是 `self-ip` / `self-ip-v6` 的 /32、/128，默认路由指向自己的地址（一切都从设备出去，出去之后按目的地址查 `Routes` 选 peer）。每条 TCP 连接两个 256 KiB 缓冲、关 Nagle、显式设 Reno；本端端口在 49152 ～ 65535 里从随机起点轮转、跳过 TCP 与 UDP 已占用的。**`Interface::poll` 每次对每个套接字只调一次 `dispatch`，一条连接最多发一个报文段**：`Stack::advance` 反复 `poll` 到返回 `PollResult::None`——否则每条连接每轮只发一个报文段，剩下的要等下一轮（`sleep_until` 的精度是 1 ms），上传被卡在约每毫秒一个报文段。`poll_delay` 给出下一个截止时间；读写者经 `register_recv_waker` / `register_send_waker` 挂起，状态变化（含 `abort`）会唤醒它们。发往本端地址的 ICMP echo 由 smoltcp 回应；分片重组最多同时 4 个、每个 16 KiB；超过 MTU 的 IPv4 外发包被分片后发出（TCP 报文段按 MSS 切，本来不会超出）。流被丢弃后：对端已经结束、也没有没读的数据就关闭，否则重置；进入 TimeWait / Closed 后遗忘，30 秒还没结束就重置 |
| P3 | smoltcp 0.12 的 TCP 限制（源码核对） | ① **Cubic 用不了**：RFC 8312 的窗口以报文段计，0.12 按字节算（`w_max` 初值 2048 字节，K ≈ 11.5 秒），又在第一次发送时就把"恢复起点"设为当时，此后每 100 ms 用公式覆盖一次窗口——窗口一直是一两个报文段，真实网络上上传只剩每秒几十 KB。改用 Reno：只开 `socket-tcp-reno`，并在每条连接上显式设置（别的 crate 经特性合并打开 `socket-tcp-cubic` 时默认会变回 Cubic）。② 重传是回退 N：快速重传与超时重传都从第一个未确认字节起重发整个窗口，不按 SACK 补发——丢包很贵，所以收端不能因缓冲太小而丢包（P5、P9）。③ 发送方没有零窗口探测（persist 定时器）：窗口更新丢了，发送方会停到转发的空闲超时。④ 在 CLOSE-WAIT / LAST-ACK / CLOSING 里，一个 ACK 会把还有数据在途的重传定时器置空：对端关闭发送方向之后丢了的报文段不再重传（`a_large_transfer_goes_through_whole` 因此最后才发 FIN）。⑤ keep-alive 探测发 seq − 1 加一个 0x00 字节，定时器状态不对时会把这个字节当数据交出去：不开 keep-alive。②～④ 登记为已知限制；smoltcp 0.13 起要求 Rust 1.91（M4-D3），是否修了这些要到升级时再核对（延后事项） |
| P4 | V5：hickory-proto | 查询：`Message::new(id, Query, Query)`，要求递归，一条 `Query::query(name, A 或 AAAA)`，id 随机；名字先转小写、补末尾的点，不是 ASCII 的名字不查（IDN 在 M8）。回答：id、类型、问题（名字不分大小写、记录类型）都对得上，且响应码是 `NoError` 或 `NXDomain` 才算作答——后者与"有名字没地址"都是空的回答，同样结束查找；`ServFail` 之类不算作答，问下一个服务器 |
| P5 | V6：`Datagram` 与 `connect_udp` | `Datagram` 是 `poll_send` / `poll_recv`（设计写的是收发的 async 方法：一个任务要同时等几个载体，只能轮询）；`set_tos(u8)` 取 TOS 字节（0x88 即 DSCP AF41；0 回到策略自己的 `tos`；做不到的载体忽略）代替 `set_dscp`；`peer_addr` 给出载体去的地址。`Connector::connect_udp` 默认返回 `Unsupported`（`this connection cannot carry UDP`：M5 之前的链路）。`DirectConnector::connect_udp`：与 TCP 同一个地址计划（按 `ip-version`、`[General] ipv6`），取第一个地址（UDP 没有"连上"可比）；`interface` / `tos` 经 `SocketHook` 作用在 UDP 套接字上；收发缓冲尽量设为 7 MiB（wireguard-go 的取值；系统可能封顶——Linux 默认封在约 416 KiB）：Windows 默认的 64 KiB 收缓冲装不下一个 TCP 窗口的突发（回环实测丢掉约一万个报文，整窗重发把吞吐压到 0.1 MiB/s） |
| P6 | V7：DNS 会话防环 | `engine.rs` 的 `named_server(spec)`：原来只看策略的 `server` 是不是域名；`wireguard` 策略没有 `server`，改看它的 peer 里有没有写成域名的 endpoint，有就与"以域名配置的代理"同样绕开（同一句说明） |
| P7 | V8：`TestSpec` / `TestCase` | `rurge-policy` 新增 `TestMode { Url(Url), Native(Target) }`：`TestCase.url` 换成 `mode`，`TestSpec.url` 换成 `mode: Option<TestMode>`（`None` 仍是测试 URL 解析不了）；`test_slot` 以"有模式"判断能不能测，`round_timeout`、`sample_of`、`tested_at` 都经它，不用改。`TestSpec` 的 key 由定义、`wireguard` 的节、模式（URL 原文或 `native`）与超时算出：改了节的结果不再算数。`TestObserver::begin` 改收 `&Target`（`TestMode::target`：URL 的主机与端口，或第一个 peer 的 endpoint）。`Outbound` 新增 `native_test(&self) -> Option<BoxFuture<'_, Result<Duration, OutboundError>>>`，默认 `None`；`TestBook` 以 `test-timeout` 包住它，超时是 `timed out`。`POST /v1/policies/test` 给了 `url` 时一律按 URL 测（一次性、不保存） |
| P8 | V12：sing-box 的 WireGuard 端点 | sing-box 1.14.1（CI 固定的版本）的 `endpoints`：`type: wireguard`、`system: false`（用户态，不建网卡）、`address`、`private_key`（Base64）、`listen_port`、`peers[].public_key` / `allowed_ips` / `reserved`；隧道里出来的连接交给 `direct`。端点没有监听地址这一项，它的 UDP 端口开在所有地址上（只在 CI 上跑）；UDP 端口无从探测，就绪看同一份配置里一个只听 `127.0.0.1` 的 `mixed` 入站。本机没有 sing-box：这个用例只在 CI 上真正跑 |
| P9 | 设备任务 | 每条隧道一个任务，独占各 peer 的载体；协议栈在 `std::sync::Mutex` 里，锁内只做内存操作（M4-D5）。收：各载体轮流先被问（`recv_any`），收到一个之后用空唤醒器把已经到了的全部收下（最多 256 个）再推进协议栈——每轮只收一个再推进时，回环上一次突发有约一万个报文因收缓冲满而丢掉；批量收之后两个方向都不丢，循环次数从约两万降到约一千。发在锁外，握手发起包前后切换 TOS（`set_tos` 失败一次就不再标记）。定时：每 250 ms 推进 boringtun 的定时器，smoltcp 按 `poll_delay`；流的读写与新连接经 `Notify` 立即唤醒任务 |
| P10 | 同一私钥与 peer 只留一条隧道 | 同一私钥连着同一 peer 的两条隧道会互相抢 peer 记住的地址（peer 回应最后写来的地址）：写本计划时，重载改了节之后，旧隧道的一个延迟 ACK 与新隧道的握手发起一同到了假对端，握手回应被发给旧隧道，新连接等了 5 秒（负载下 48 次里 7 次）；真实服务器按"最后一个通过认证的报文"更新对端地址，同样会这样。做法：进程内一张隧道表（节、设备的弱引用）与一把全局异步锁（隧道逐个启动）；节相同的策略共用隧道；每个 `WireGuardOutbound` 构造时从全局计数器领一个代次（重载总是先构造新出站，再释放旧的）；"冲突"= 私钥相同且有共同的 peer 公钥；启动时结束冲突且代次更早的隧道（中止任务、重置它的全部连接，不再发任何报文），冲突而代次更晚的隧道已在时拒绝启动（`wireguard: a newer configuration of this tunnel is in use`），不会来回争抢。私钥相同而 peer 不同的两个节互不影响 |
| P11 | 隧道的寿命 | 设计 6.5 写的是"出站对象被释放 → 设备任务结束，进行中的流读写返回错误"。改为：流持有设备，隧道活到出站与经它的连接都释放为止（重载不打断无关的连接，M3a）；只有被更晚的配置接替时（P10）才立即结束、连接随之失败 |
| P12 | 重拨与网络变化 | 每 5 分钟（`REDIAL`）为写成域名的 endpoint 与启动时没连上的 peer 各新拨一条载体：去的地址（`peer_addr`）不同或原来没有载体时换上，**并立即向那个 peer 发起握手**（否则要等 boringtun 5 秒一次的重试）；地址相同就丢掉新拨的。设计说"地址变了重连、地址族变了重建"，两种在这里是一回事。`WireGuardOutbound::network_changed()`：隧道在运行时中止未完成的重拨、为每个 peer 新拨载体并换上（同样立即握手）；阶段 2 没有调用它的探测器 |
| P13 | 日志 | 只带策略名与 peer 序号（从 1 起）。握手完成：该 peer 第一次、或"没有回应"之后恢复时记 `info` `wireguard: handshake completed`（流量不断时每两分钟换一次密钥，每次都记会刷屏）；握手没有回应：boringtun 放弃重试时（约 90 秒）记一次 `warn` `wireguard: the peer did not answer the handshake`；启动时 peer 连不上 `warn` `wireguard: the peer cannot be reached`（附连接器的错误），重拨失败 `debug`；endpoint 换了地址 `info` `wireguard: the peer's endpoint moved`。bin 的日志订阅加一个 `Targets` 过滤，`boringtun` 为 `OFF` |
| P14 | 隧道内 DNS（设计 6.3 的细节） | `dns-server` 按列表顺序问，第一个作答的服务器为准（空的回答也算，P4）；该地址族没有本端地址或没有 peer 覆盖的服务器直接跳过、不等；每个服务器最多等 2 秒（`DNS_WAIT`）；`system` 表示在那个位置改用本机解析器（含 `[Host]`）。A 与 AAAA 按本端有的地址族同时问。缓存每个出站一份：只存有地址的回答、按最小 TTL、最长 1 小时、最多 256 个名字（满了先丢最早到期的）。没有 `dns-server` 时用本机解析器（M4-D9）。挑地址：只取本端有那个地址族的，`prefer-ipv6` 时 IPv6 在前，同族保持回答的顺序 |
| P15 | 配置层的细节（设计 4.1） | 密钥：Base64（带不带填充）或 64 位十六进制，32 字节；`client-id`：`83/12/235`、6 位十六进制（可带 `0x`）或 4 字符 Base64；`allowed-ips` 写成纯地址时按单个主机，前缀的主机位被截掉；`endpoint` 是 `host:port`，IPv6 写 `[addr]:port`；`dns-server` 不带端口时是 53，组播地址与 URL 不收。键写错了值时只报"值不对"，不再同时报"缺少必填键"。重名的节：`W0020`（`duplicate [WireGuard home] ignored; the first definition is used`），与重名策略一致。peer 的 endpoint 主机名进 `Config::proxy_hostnames`：`[Host]` 不作用于它，与代理服务器的主机名相同。`[Tailscale]` 仍然 `deferred` |
| P16 | 脱敏（设计 4.7） | 设计说 `preshared-key` 已在名单里——并不在（`redact.rs` 只有 `pre-shared-key`）。M4b 加进去：`peer = (…, preshared-key = …)` 里的值在 `profiles/current`（`sensitive=0`）中为 `***` |
| P17 | `underlying-proxy`（M4-D7） | 加载：`W0029`，说法固定为 `` `underlying-proxy` does not work with `wireguard` policies in this version; the policy rejects every connection ``（通用的"解析了但没有效果"不对：策略会 REJECT）；写的是 `DIRECT` 时不报（`DIRECT` 就是没有链）。拨号：链路的 `connect_udp` 返回 `Unsupported` → 出站返回 `OutboundError::Unsupported("wireguard over underlying-proxy")` → 引擎 REJECT 并在请求记录写 `policy protocol not implemented: wireguard over underlying-proxy`——此前引擎只在解析期写这类说明，拨号期的 `Unsupported` 不写 |
| P18 | 任务的切分 | 设计第 16 节草图的 10 个任务，但 7 与 8 对调：先接进引擎（Task 7），再做测速（Task 8）——原生测速的端到端用例要经引擎。吞吐基准放在 Task 7（引擎装配之后，按草图第 8 项）；bin 的日志过滤也在 Task 7（从那时起 bin 就能跑 WireGuard 了） |
| P19 | 吞吐基准 | `outbound.rs` 里一个忽略的用例 `throughput`：经回环 `FakeWgPeer` 双向回显 64 MiB（两个工作线程）。写本计划时在 Windows 11 上：修正前 98 秒（0.7 MiB/s 每方向）；P2、P3、P5、P9 之后约 9.8 秒（**约 6.5 MiB/s 每方向**，三次一致）。两端都是 smoltcp，最小 RTO 只有 10 ms，调度抖动会引起假重传（整窗重发）；每个报文约 46 µs 的开销里 Windows 的 UDP 系统调用占大头——所以这是偏保守的下限，真实服务端是内核 TCP。不作门禁 |
| P20 | `FakeWgPeer` | `PeerCore` 是不做 I/O 的对端：boringtun 响应端加它自己的 smoltcp 主机，`any_ip`（隧道里发给它的任何地址都答）：7 号端口回显、80 号端口对任何请求回 `204 No Content`（连接保持）、`10.0.0.53:53` 的名字服务器；同时保持 8 个监听套接字（一批同时到的 SYN 都接得住）；可要求并写回 `client-id`；记录握手次数、保留字节、接受的连接及其目的地址、重置、DNS 问题与 HTTP 请求数。`FakeWgPeer` 把它放在回环 UDP 端口上：回应最后写来的地址（漫游）、记下客户端用过的地址、可以"沉默"；按 smoltcp 的截止时间唤醒、一次收完已到的报文、收发缓冲 8 MiB——否则回环基准测的是假对端而不是 rurge |
| P21 | 错误文本 | 全部是固定说法，不带服务器原文与密钥：`wireguard: no peer's allowed-ips covers <地址>`、`wireguard: the tunnel has no IPv4 address` / `… IPv6 address`、`wireguard: <地址> cannot be connected to`（如 `0.0.0.0`）、`wireguard: every local port of the tunnel is in use`、`dns: wireguard: dns lookup of <名字> failed`（`OutboundError::Dns`）、`wireguard: the destination refused the connection`（`ConnectionRefused`）、`wireguard: a newer configuration of this tunnel is in use`；握手没有回应归为拨号超时；隧道起不来时是连接器的错误（例如 endpoint 解析不了） |

## 承接事项

之前计划「延后事项」表里标给 M4b 的条目，及仍然有效的既有现象。

| # | 来源 | 事项 | 处理 | 任务 |
| - | ---- | ---- | ---- | ---- |
| C1 | M4a #1（M3b #10） | WireGuard 的 `section-name=` 是按名字用到主配置的材料，要进订阅安全门 | `reaches_into_profile` 的循环加上 `section-name`；订阅内容里只取 `[Proxy]`，所以订阅的 `wireguard` 行只能引用主配置的节——正是要挡的 | 1 |
| C2 | M3b #7（P21） | 测试二进制偶发崩溃 | 照旧：门禁遇到就重跑 | — |
| C3 | M4a #11 | russh 的 `log` 记录经 `LogTracer` 进 rurge 的日志 | 不在本计划；本计划只关 boringtun 的（P13），russh 的由项目所有者决定 | — |

## File Structure

新建：

| 文件 | 职责 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/wireguard.rs` | `WireGuardSection`、`TunnelDns`、`WireGuardPeer`、`PeerEndpoint`、`parse_section`，与用例 | 1 |
| `crates/rurge-config/src/spec/wireguard.rs` | `WireGuardSpec`、`read_wireguard`、`NOT_APPLICABLE`，与用例 | 1 |
| `crates/rurge-proto-wireguard/Cargo.toml`、`src/lib.rs` | 新 crate 的清单与入口 | 3（4、5 加模块） |
| `crates/rurge-proto-wireguard/src/routes.rs` | `Routes`：两张最长前缀表，与用例 | 3 |
| `crates/rurge-proto-wireguard/src/wire.rs` | 报文类型、`client-id` 的写与清、握手包的 TOS，与用例 | 3 |
| `crates/rurge-proto-wireguard/src/stack.rs` | `Stack`、`Outgoing`、`Refusal`、内存里的包队列，与用例 | 3、5、6、8 |
| `crates/rurge-proto-wireguard/src/testing/mod.rs` | `PeerCore`、`PeerOpts`、测试密钥与节的辅助函数 | 3、4、5、7、8 |
| `crates/rurge-proto-wireguard/src/device.rs` | `Device`：设备任务、载体、隧道表 | 4、5、6、8 |
| `crates/rurge-proto-wireguard/src/stream.rs` | `TunnelStream` | 4 |
| `crates/rurge-proto-wireguard/src/outbound.rs` | `WireGuardOutbound`，与用例 | 4 ～ 8 |
| `crates/rurge-proto-wireguard/src/testing/peer.rs` | `FakeWgPeer` | 4、6 |
| `crates/rurge-proto-wireguard/src/dns.rs` | 隧道内 DNS 的问与答、缓存，与用例 | 5 |
| `crates/rurge-engine/tests/outbounds_wireguard.rs` | 经引擎的端到端用例 | 7、8 |
| `tests/interop/tests/sing_box_wireguard.rs` | 对 sing-box WireGuard 端点的互操作用例 | 9 |

修改：

| 文件 | 改动 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/{lib.rs, config.rs, diagnostic.rs, deferred.rs, redact.rs, spec/mod.rs}` | 节的类型化与 `E0023`、重名节、`proxy_hostnames`、脱敏（1）；`SpecEnv.wireguard`、`ProtoSpec::WireGuard`、`to_spec` 分支（7） | 1、7 |
| `crates/rurge-policy/src/assemble.rs` | 订阅安全门认 `section-name`（1）；`SpecEnv.wireguard`（7） | 1、7 |
| `tests/corpus/valid/kitchen-sink.conf`、快照 | 能通过校验的密钥；`[WireGuard home]` 不再 `W0016`（1）；两条 `W0029`（7） | 1、7 |
| `crates/rurge-net/src/connector.rs` | `Datagram`、`connect_udp` | 2 |
| `Cargo.toml` | 工作区依赖：`rurge-proto-wireguard`、`boringtun`、`smoltcp`（3）；`socket-udp`（5） | 3、5 |
| `crates/rurge-proto/src/{outbound.rs, http.rs, socks5.rs}` | `Outbound::native_test`（8）；用例里的 `SpecEnv`（7） | 7、8 |
| `crates/rurge-policy/src/{registry.rs, testbook.rs, auto.rs, testing.rs}` | `TestMode` 与两种测速 | 8 |
| `crates/rurge-engine/{Cargo.toml, src/outbounds.rs, src/engine.rs, src/auto.rs, tests/common/mod.rs, tests/pipeline.rs}` | 工厂分支、`Unsupported` 的说明、DNS 会话防环、`Profile.sections`（7）；测试会话的目标（8） | 7、8 |
| `crates/rurge/src/cli/run.rs` | 关掉 boringtun 的日志 | 7 |
| `tests/interop/{Cargo.toml, src/lib.rs, tests/common/mod.rs, README.md}` | WireGuard 端点的夹具 | 9 |
| `crates/rurge/src/capabilities.rs`、`crates/rurge/tests/cli.rs` | 能力表翻转与用例 | 10 |
| 文档（兼容性清单、两份 README、`CLAUDE.md`、手工验收、两份 API 文档、M4 设计第 19 节、总设计 Q4 与风险 G） | 见 Task 10 | 10 |

## 任务一览

| 任务 | 交付物 | 依赖 |
| ---- | ------ | ---- |
| 1 | 配置层：`[WireGuard <name>]` 与 `WireGuardSpec`；订阅安全门认 `section-name`（承接 C1） | — |
| 2 | `rurge-net`：UDP 载体（`Datagram`、`connect_udp`） | — |
| 3 | `rurge-proto-wireguard` 骨架：路由表、`client-id`、协议栈与内存里的对端 | 1 |
| 4 | 隧道：设备任务、流、`WireGuardOutbound`、`FakeWgPeer`；在本机解析目标域名 | 2、3 |
| 5 | 隧道内 DNS | 4 |
| 6 | 生命周期：重拨与 endpoint 跟随、网络变化入口、握手日志、同一私钥与 peer 只留一条隧道 | 5 |
| 7 | 接入：`ProtoSpec::WireGuard`、引擎工厂、`underlying-proxy` 的 REJECT、DNS 会话防环、端到端、吞吐基准、bin 的日志过滤 | 6 |
| 8 | 测速：`TestMode`、原生握手测速、经隧道的 URL 测速 | 7 |
| 9 | 互操作：sing-box 的 WireGuard 端点 | 8 |
| 10 | 能力表翻转 `wireguard` 与文档 | 9 |

---

### Task 1: 配置层——`[WireGuard <name>]` 与 `WireGuardSpec`；订阅安全门认 `section-name`（承接 C1）

`[WireGuard <name>]` 从 `deferred` 转为类型化的节（设计 4.1）：加载时逐节校验，没被策略引用的节也校验；节内的错误与缺必填键是新的 `E0023`，不认识的键与 `peer` 字段是 `W0001`，重名的节只用第一个（`W0020`，P15）。`wireguard` 策略行的参数读成 `WireGuardSpec`（设计 4.3）：spec 带着所引用节的全部内容，节改了 spec 就不同，重载按指纹复用出站自然失效。本任务只提供 `read_wireguard`；`to_spec` 的 `wireguard` 分支在 Task 7 接上（那时才有出站可以构建，能力表在 Task 10 翻转，之前 `wireguard` 行照旧 `W0007`）。另外三件小事：peer 的 endpoint 主机名进 `Config::proxy_hostnames`（`[Host]` 不作用于它，P15）；脱敏名单补上 `preshared-key`（P16）；订阅行自己写的 `section-name=` 指向的是主配置的节（含私钥）——像 `client-cert=`、`private-key=` 一样，只认 `external-policy-modifier` 设上的值，否则整行跳过（C1，设计 4.8）。

**Files:**
- Create: `crates/rurge-config/src/wireguard.rs`（`DEFAULT_MTU`、`MTU_RANGE`、`WireGuardSection`、`TunnelDns`、`WireGuardPeer`、`PeerEndpoint`、`parse_section`，与用例）
- Create: `crates/rurge-config/src/spec/wireguard.rs`（`NOT_APPLICABLE`、`WireGuardSpec`、`read_wireguard`，与用例）
- Modify: `crates/rurge-config/src/lib.rs`（`pub mod wireguard;`）、`src/spec/mod.rs`（`pub mod wireguard;` 与导出）、`src/diagnostic.rs`（`E_WIREGUARD_SECTION`）、`src/config.rs`（`Config.wireguard`、逐节校验与重名、`proxy_hostnames`，与用例）、`src/deferred.rs`（`WireGuard` 不再 deferred，与用例）、`src/redact.rs`（`preshared-key`，与用例）
- Modify: `crates/rurge-policy/src/assemble.rs`（`reaches_into_profile` 认 `section-name`，与用例）
- Modify: `tests/corpus/valid/kitchen-sink.conf`（能通过校验的密钥）与 `crates/rurge-config/tests/snapshots/corpus__corpus__kitchen-sink.snap`

**Interfaces:**
- Consumes: 既有的 `rurge_config::spec::{Secret, ParamReader, CommonOpts}`、`spec::tls::refuse_tls`、`text::Section`、`value::{split_definition, split_list, parse_bool, parse_key_value}`、`HostName`。
- Produces:
  - `rurge_config::wireguard::{DEFAULT_MTU: u16 (1280), MTU_RANGE: RangeInclusive<u16> (576..=1420)}`
  - `pub struct WireGuardSection { pub name: String, pub private_key: Secret<[u8; 32]>, pub self_ip: Option<Ipv4Addr>, pub self_ip_v6: Option<Ipv6Addr>, pub dns_servers: Vec<TunnelDns>, pub prefer_ipv6: bool, pub mtu: u16, pub peers: Vec<WireGuardPeer> }`（`Clone + Debug + Default + PartialEq + Eq + Hash`；`Debug` 不显示两个密钥）
  - `pub enum TunnelDns { Server(SocketAddr), System }`
  - `pub struct WireGuardPeer { pub public_key: [u8; 32], pub allowed_ips: Vec<IpNet>, pub endpoint: PeerEndpoint, pub preshared_key: Option<Secret<[u8; 32]>>, pub keepalive: Option<u16>, pub client_id: Option<[u8; 3]> }`
  - `pub struct PeerEndpoint { pub host: HostName, pub port: u16 }`（`Display`：`host:port`，IPv6 写作 `[addr]:port`）
  - `pub fn parse_section(section: &Section, diags: &mut Diagnostics) -> Option<WireGuardSection>`（有 `E0023` 时 `None`）
  - `Config.wireguard: Vec<WireGuardSection>`（能用的节，按出现顺序，重名只留第一个）
  - `rurge_config::spec::wireguard::{NOT_APPLICABLE: [&str; 4], WireGuardSpec { pub section: WireGuardSection }, read_wireguard(r: &mut ParamReader<'_>, common: &mut CommonOpts, sections: &[WireGuardSection]) -> WireGuardSpec}`（报错后返回值无意义，调用方看 `r.has_errors()`）与导出 `rurge_config::spec::WireGuardSpec`
  - `rurge_config::diagnostic::codes::E_WIREGUARD_SECTION`（`"E0023"`）

- [ ] **Step 1: 先写用例**

`crates/rurge-policy/src/assemble.rs`——把

```rust
            )]
        );
    }
}

```

换成

```rust
            )]
        );
    }

    /// A section of the profile, after its groups.
    const HOME: &str = "[WireGuard home]
private-key = yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=
self-ip = 10.20.0.2
peer = (public-key = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=, allowed-ips = 0.0.0.0/0, endpoint = vpn.test:51820)";

    /// A `wireguard` line's `section-name` names a section of the profile,
    /// keys and all: like `private-key`, only the user's modifier may set it
    /// (phase 2 M4 design 4.8).
    #[test]
    fn a_subscription_wireguard_line_may_not_use_the_profiles_sections() {
        let cfg = profile(
            "Corp = http, corp.test, 80",
            &format!(
                "G = select, policy-path=https://sub.test/g
H = select, policy-path=https://sub.test/h, external-policy-modifier=\"section-name=home\"
{HOME}"
            ),
        );
        let a = assemble(
            &cfg,
            &snapshots(
                &cfg,
                &[
                    (
                        "G",
                        "Own = wireguard, section-name=home
Plain = http, p.test, 80",
                    ),
                    ("H", "Mod = wireguard, section-name=home"),
                ],
            ),
        );
        assert_eq!(members(&a, "G"), ["Plain"]);
        assert_eq!(members(&a, "H"), ["Mod"]);
        let skipped: Vec<_> = warnings(&a)
            .into_iter()
            .filter(|(code, _)| *code == codes::W_SET_LINES_SKIPPED)
            .collect();
        assert_eq!(
            skipped,
            [(
                codes::W_SET_LINES_SKIPPED,
                "policy group `G`: `policy-path` line 1: a subscription line's own `section-name` is not honoured (only `external-policy-modifier` may set it); skipped".to_string()
            )]
        );
    }
}

```

`crates/rurge-config/src/config.rs`——把

```rust
        assert!(loaded.config.spec("Old1").is_none() && loaded.config.spec("Old2").is_none());
    }

    #[test]
    fn keystore_item_unknown_field_warns() {
```

换成

```rust
        assert!(loaded.config.spec("Old1").is_none() && loaded.config.spec("Old2").is_none());
    }

    const WG_PRIVATE: &str = "yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=";
    const WG_PUBLIC: &str = "xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=";

    fn wireguard_section(name: &str, endpoint: &str) -> String {
        format!(
            "[WireGuard {name}]
private-key = {WG_PRIVATE}
self-ip = 10.0.0.2
peer = (public-key = {WG_PUBLIC}, allowed-ips = 10.0.0.0/24, endpoint = {endpoint})
"
        )
    }

    /// A `[WireGuard <name>]` section is typed, no longer a deferred one
    /// (phase 2 M4 design 4.1).
    #[test]
    fn wireguard_sections_are_typed() {
        let l = load_text(&format!(
            "{}{}[Rule]
FINAL,DIRECT
",
            wireguard_section("home", "vpn.example.com:51820"),
            wireguard_section("office", "192.0.2.1:51820")
        ));
        assert!(l.diagnostics.is_empty(), "{:?}", l.diagnostics.into_vec());
        let names: Vec<&str> = l.config.wireguard.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, ["home", "office"]);
        assert!(l.config.deferred.sections.is_empty());
    }

    /// Checked whether a policy uses it or not; the first of two sections
    /// of one name is used.
    #[test]
    fn every_wireguard_section_is_checked() {
        let l = load_text(&format!(
            "[WireGuard spare]
self-ip = 10.0.0.2
{}{}[Rule]
FINAL,DIRECT
",
            wireguard_section("home", "a.test:51820"),
            wireguard_section("home", "b.test:51820")
        ));
        let found: Vec<(&str, &str)> = l
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                (
                    codes::E_WIREGUARD_SECTION,
                    "[WireGuard spare]: `private-key` is required"
                ),
                (
                    codes::E_WIREGUARD_SECTION,
                    "[WireGuard spare]: at least one `peer` is required"
                ),
                (
                    codes::W_DUPLICATE_RULESET,
                    "duplicate [WireGuard home] ignored; the first definition is used"
                ),
            ]
        );
        assert_eq!(l.config.wireguard.len(), 1);
        assert_eq!(
            l.config.wireguard[0].peers[0].endpoint.to_string(),
            "a.test:51820"
        );
    }

    /// A peer's endpoint is a proxy server like any other: `[Host]` does not
    /// apply to its name (matrix 6.3).
    #[test]
    fn wireguard_endpoints_are_proxy_hostnames() {
        let l = load_text(&format!(
            "[Proxy]
A = http, proxy.example.com, 8080
{}{}[Rule]
FINAL,DIRECT
",
            wireguard_section("home", "VPN.example.com:51820"),
            wireguard_section("office", "192.0.2.1:51820")
        ));
        let mut names: Vec<String> = l.config.proxy_hostnames().into_iter().collect();
        names.sort();
        assert_eq!(names, ["proxy.example.com", "vpn.example.com"]);
    }

    #[test]
    fn keystore_item_unknown_field_warns() {
```

`crates/rurge-config/src/deferred.rs`——把

```rust
        assert!(is_deferred("WireGuard home"));
        assert!(is_deferred("WireGuard "));
        assert!(!is_deferred("WireGuard"));
```

换成

```rust
        assert!(is_deferred("Tailscale home"));
        assert!(is_deferred("Tailscale "));
        assert!(!is_deferred("Tailscale"));
        // typed since phase 2 M4
        assert!(!is_deferred("WireGuard home"));
```

`crates/rurge-config/src/deferred.rs`——把

```rust
        assert!(is_deferred("WireGuard 家里"));
```

换成

```rust
        assert!(is_deferred("Tailscale 家里"));
```

`crates/rurge-config/src/redact.rs`——把

```rust
            "peer = (pre-shared-key = ***, endpoint = 1.2.3.4:51820)"
        );
    }

    /// A subscription URL usually carries a token, and a modifier can set any
```

换成

```rust
            "peer = (pre-shared-key = ***, endpoint = 1.2.3.4:51820)"
        );
    }

    /// The manual spells a peer's key `preshared-key`.
    #[test]
    fn a_wireguard_peers_preshared_key_is_redacted() {
        assert_eq!(
            redact_profile(
                "peer = (public-key = PUB, preshared-key = PSK1, endpoint = 1.2.3.4:51820)"
            ),
            "peer = (public-key = PUB, preshared-key = ***, endpoint = 1.2.3.4:51820)"
        );
    }

    /// A subscription URL usually carries a token, and a modifier can set any
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-policy a_subscription_wireguard_line_may_not_use_the_profiles_sections`
Expected: FAIL——订阅行自己写的 `section-name=` 还没被挡住（`rurge-config` 自己的用例此时编译不过，Step 3 之后才能跑）：

```text
test assemble::tests::a_subscription_wireguard_line_may_not_use_the_profiles_sections ... FAILED
thread 'assemble::tests::a_subscription_wireguard_line_may_not_use_the_profiles_sections' panicked at crates\rurge-policy\src\assemble.rs:1636:9:
assertion `left == right` failed
  left: ["Own", "Plain"]
 right: ["Plain"]
```

- [ ] **Step 3: 实现（新模块自带用例）**

新建 `crates/rurge-config/src/wireguard.rs`：

```rust
//! `[WireGuard <name>]` sections (manual: Policies › WireGuard): the key,
//! the tunnel addresses and the peers of the tunnel a `wireguard` policy's
//! `section-name` names. Every section is checked at load, whether a policy
//! uses it or not.

use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::span::Span;
use crate::spec::Secret;
use crate::text::Section;
use crate::types::HostName;
use crate::value::{parse_bool, parse_key_value, split_definition, split_list};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use ipnet::IpNet;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::ops::RangeInclusive;

/// `mtu` when the section has none.
pub const DEFAULT_MTU: u16 = 1280;
/// The `mtu` values the manual accepts.
pub const MTU_RANGE: RangeInclusive<u16> = 576..=1420;
/// The port of a `dns-server` written without one.
const DNS_PORT: u16 = 53;

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct WireGuardSection {
    /// What follows `WireGuard ` in the header: the name `section-name` gives.
    pub name: String,
    pub private_key: Secret<[u8; 32]>,
    pub self_ip: Option<Ipv4Addr>,
    pub self_ip_v6: Option<Ipv6Addr>,
    /// `dns-server`, in the order written; empty when there is none.
    pub dns_servers: Vec<TunnelDns>,
    pub prefer_ipv6: bool,
    pub mtu: u16,
    pub peers: Vec<WireGuardPeer>,
}

/// One entry of `dns-server`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TunnelDns {
    /// A resolver reached through the tunnel.
    Server(SocketAddr),
    /// `system`: rurge's own resolver, on this machine.
    System,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WireGuardPeer {
    pub public_key: [u8; 32],
    /// The destinations routed to this peer, and the addresses it may send from.
    pub allowed_ips: Vec<IpNet>,
    pub endpoint: PeerEndpoint,
    pub preshared_key: Option<Secret<[u8; 32]>>,
    /// Persistent keepalive, in seconds; `None` when off (`0`).
    pub keepalive: Option<u16>,
    /// `client-id`: bytes 1–3 of every WireGuard message (WARP).
    pub client_id: Option<[u8; 3]>,
}

/// A peer's UDP address as written: a host name is resolved when the tunnel
/// starts.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PeerEndpoint {
    pub host: HostName,
    pub port: u16,
}

impl fmt::Display for PeerEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.host {
            HostName::Ip(IpAddr::V6(v6)) => write!(f, "[{v6}]:{}", self.port),
            host => write!(f, "{host}:{}", self.port),
        }
    }
}

/// Where problems of one section go: `E0023` for what makes it unusable,
/// `W0001` for what is ignored. Values that are secrets are never quoted.
struct Report<'a> {
    name: &'a str,
    diags: &'a mut Diagnostics,
    failed: bool,
}

impl Report<'_> {
    fn error(&mut self, span: &Span, message: impl fmt::Display) {
        self.failed = true;
        self.diags.push(
            Diagnostic::error(
                codes::E_WIREGUARD_SECTION,
                format!("[WireGuard {}]: {message}", self.name),
            )
            .at(span.clone()),
        );
    }

    fn warn(&mut self, span: &Span, message: impl fmt::Display) {
        self.diags.push(
            Diagnostic::warning(
                codes::W_UNKNOWN_KEY,
                format!("[WireGuard {}]: {message}", self.name),
            )
            .at(span.clone()),
        );
    }
}

/// A 32-byte key, the way WireGuard writes it (Base64) or as 64 hex digits.
fn key32(value: &str) -> Option<[u8; 32]> {
    let value = value.trim();
    let bytes = if value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
        (0..32)
            .map(|i| u8::from_str_radix(&value[2 * i..2 * i + 2], 16).ok())
            .collect::<Option<Vec<u8>>>()?
    } else {
        STANDARD
            .decode(value)
            .or_else(|_| STANDARD_NO_PAD.decode(value))
            .ok()?
    };
    bytes.try_into().ok()
}

fn tunnel_dns(entry: &str) -> Result<TunnelDns, String> {
    let entry = entry.trim();
    if entry.eq_ignore_ascii_case("system") {
        return Ok(TunnelDns::System);
    }
    if entry.contains("://") {
        return Err(format!(
            "`dns-server` `{entry}`: an encrypted-DNS URL is not accepted here"
        ));
    }
    let addr = match entry.parse::<SocketAddr>() {
        Ok(addr) => addr,
        Err(_) => match entry.parse::<IpAddr>() {
            Ok(ip) => SocketAddr::new(ip, DNS_PORT),
            Err(_) => {
                return Err(format!(
                    "invalid `dns-server` `{entry}` (expected an IP address, an address with a port, or `system`)"
                ));
            }
        },
    };
    if matches!(addr.ip(), IpAddr::V4(v4) if v4.is_multicast()) {
        return Err(format!(
            "`dns-server` `{entry}`: a multicast address is not accepted"
        ));
    }
    Ok(TunnelDns::Server(addr))
}

fn endpoint(value: &str) -> Option<PeerEndpoint> {
    let value = value.trim();
    if let Ok(addr) = value.parse::<SocketAddr>() {
        return (addr.port() != 0).then(|| PeerEndpoint {
            host: HostName::Ip(addr.ip()),
            port: addr.port(),
        });
    }
    let (host, port) = value.rsplit_once(':')?;
    // an IPv6 address goes in brackets; anything else with a colon is no host
    if host.is_empty() || host.contains([':', '[', ']']) || host.contains(char::is_whitespace) {
        return None;
    }
    let port = port.parse::<u16>().ok().filter(|p| *p != 0)?;
    Some(PeerEndpoint {
        host: HostName::parse(host),
        port,
    })
}

/// `83/12/235`, three bytes in hex (`530ceb`) or four Base64 characters
/// (`Uwzr`).
fn client_id(value: &str) -> Option<[u8; 3]> {
    let value = value.trim();
    let parts: Vec<&str> = value.split('/').collect();
    if parts.len() == 3 {
        let decimals: Option<Vec<u8>> = parts.iter().map(|p| p.parse::<u8>().ok()).collect();
        if let Some(bytes) = decimals {
            return bytes.try_into().ok();
        }
    }
    let hex = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        let bytes: Option<Vec<u8>> = (0..3)
            .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok())
            .collect();
        return bytes?.try_into().ok();
    }
    if value.len() == 4 {
        return STANDARD.decode(value).ok()?.try_into().ok();
    }
    None
}

fn allowed_ips(value: &str) -> Result<Vec<IpNet>, String> {
    let mut out = Vec::new();
    for entry in value.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let net = match entry.parse::<IpNet>() {
            Ok(net) => net.trunc(),
            // a bare address is a route to that address alone
            Err(_) => match entry.parse::<IpAddr>() {
                Ok(ip) => IpNet::from(ip),
                Err(_) => return Err(format!("invalid `allowed-ips` entry `{entry}`")),
            },
        };
        out.push(net);
    }
    if out.is_empty() {
        return Err("`allowed-ips` is empty".to_string());
    }
    Ok(out)
}

/// One `( … )` of a `peer` line; `n` counts the section's peers from 1.
fn peer(report: &mut Report<'_>, span: &Span, item: &str, n: usize) -> Option<WireGuardPeer> {
    let Some(inner) = item
        .trim()
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
    else {
        report.error(
            span,
            format!("peer {n}: expected `(public-key = …, allowed-ips = …, endpoint = …)`"),
        );
        return None;
    };
    let failed_before = report.failed;
    let (mut public_key, mut ips, mut end, mut psk) = (None, Vec::new(), None, None);
    let (mut keepalive, mut id) = (None, None);
    // what was written, valid or not: only a field left out is "required"
    let mut given: Vec<String> = Vec::new();
    for field in split_list(inner) {
        let Some((key, value)) = parse_key_value(&field) else {
            // never quoted: it may be a key that lost its name
            report.warn(span, format!("peer {n}: a field without `=` ignored"));
            continue;
        };
        given.push(key.to_ascii_lowercase());
        match key.to_ascii_lowercase().as_str() {
            "public-key" => match key32(value) {
                Some(k) => public_key = Some(k),
                None => report.error(
                    span,
                    format!("peer {n}: `public-key` is not a 32-byte key in Base64 or hex"),
                ),
            },
            "allowed-ips" => match allowed_ips(value) {
                Ok(list) => ips = list,
                Err(why) => report.error(span, format!("peer {n}: {why}")),
            },
            "endpoint" => match endpoint(value) {
                Some(e) => end = Some(e),
                None => report.error(
                    span,
                    format!("peer {n}: invalid `endpoint` `{value}` (expected host:port)"),
                ),
            },
            "preshared-key" => match key32(value) {
                Some(k) => psk = Some(Secret::new(k)),
                None => report.error(
                    span,
                    format!("peer {n}: `preshared-key` is not a 32-byte key in Base64 or hex"),
                ),
            },
            "keepalive" => match value.trim().parse::<u16>() {
                Ok(secs) => keepalive = (secs > 0).then_some(secs),
                Err(_) => report.error(
                    span,
                    format!("peer {n}: invalid `keepalive` `{value}` (expected 0-65535 seconds)"),
                ),
            },
            "client-id" => match client_id(value) {
                Some(bytes) => id = Some(bytes),
                None => report.error(
                    span,
                    format!(
                        "peer {n}: invalid `client-id` `{value}` (expected `a/b/c`, three bytes in hex or four Base64 characters)"
                    ),
                ),
            },
            other => report.warn(span, format!("peer {n}: unknown field `{other}` ignored")),
        }
    }
    for field in ["public-key", "allowed-ips", "endpoint"] {
        if !given.iter().any(|g| g == field) {
            report.error(span, format!("peer {n}: `{field}` is required"));
        }
    }
    if report.failed != failed_before {
        return None;
    }
    Some(WireGuardPeer {
        public_key: public_key?,
        allowed_ips: ips,
        endpoint: end?,
        preshared_key: psk,
        keepalive,
        client_id: id,
    })
}

/// The section, or `None` after its errors went to `diags`.
pub fn parse_section(section: &Section, diags: &mut Diagnostics) -> Option<WireGuardSection> {
    let name = section.name["WireGuard ".len()..].trim();
    let mut report = Report {
        name,
        diags,
        failed: false,
    };
    if name.is_empty() {
        report.error(&section.span, "the section needs a name");
        return None;
    }
    let mut private_key = None;
    let (mut self_ip, mut self_ip_v6) = (None, None);
    let mut dns_servers = Vec::new();
    let mut prefer_ipv6 = false;
    let mut mtu = DEFAULT_MTU;
    let mut peers = Vec::new();
    // peers are numbered as written, broken ones included
    let mut written = 0;
    // what was written, valid or not: only a key left out is "required"
    let mut given: Vec<String> = Vec::new();
    for entry in section.active_entries() {
        let span = &entry.span;
        let Some((key, value)) = split_definition(&entry.raw) else {
            report.error(span, "expected `key = value`");
            continue;
        };
        given.push(key.to_ascii_lowercase());
        match key.to_ascii_lowercase().as_str() {
            "private-key" => match key32(value) {
                Some(k) => private_key = Some(Secret::new(k)),
                None => report.error(span, "`private-key` is not a 32-byte key in Base64 or hex"),
            },
            "self-ip" => match value.parse::<Ipv4Addr>() {
                Ok(ip) => self_ip = Some(ip),
                Err(_) => report.error(
                    span,
                    format!("invalid `self-ip` `{value}` (expected an IPv4 address, not a prefix)"),
                ),
            },
            "self-ip-v6" => match value.parse::<Ipv6Addr>() {
                Ok(ip) => self_ip_v6 = Some(ip),
                Err(_) => report.error(
                    span,
                    format!(
                        "invalid `self-ip-v6` `{value}` (expected an IPv6 address, not a prefix)"
                    ),
                ),
            },
            "dns-server" => {
                for item in split_list(value) {
                    match tunnel_dns(&item) {
                        Ok(dns) => dns_servers.push(dns),
                        Err(why) => report.error(span, why),
                    }
                }
            }
            "prefer-ipv6" => match parse_bool(value) {
                Some(b) => prefer_ipv6 = b,
                None => report.error(
                    span,
                    format!("invalid `prefer-ipv6` `{value}` (expected true or false)"),
                ),
            },
            "mtu" => match value.parse::<u16>() {
                Ok(n) if MTU_RANGE.contains(&n) => mtu = n,
                _ => report.error(
                    span,
                    format!(
                        "invalid `mtu` `{value}` (expected {}-{})",
                        MTU_RANGE.start(),
                        MTU_RANGE.end()
                    ),
                ),
            },
            "peer" => {
                for item in split_list(value) {
                    written += 1;
                    peers.extend(peer(&mut report, span, &item, written));
                }
            }
            other => report.warn(span, format!("unknown key `{other}` ignored")),
        }
    }
    let given = |key: &str| given.iter().any(|g| g == key);
    if !given("private-key") {
        report.error(&section.span, "`private-key` is required");
    }
    if !given("self-ip") && !given("self-ip-v6") {
        report.error(
            &section.span,
            "at least one of `self-ip` and `self-ip-v6` is required",
        );
    }
    if written == 0 {
        report.error(&section.span, "at least one `peer` is required");
    }
    if report.failed {
        return None;
    }
    Some(WireGuardSection {
        name: name.to_string(),
        private_key: private_key?,
        self_ip,
        self_ip_v6,
        dns_servers,
        prefer_ipv6,
        mtu,
        peers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Severity;
    use crate::text::{Origin, parse_str};
    use std::path::Path;
    use std::sync::Arc;

    /// Keys from the WireGuard documentation; they guard nothing.
    const PRIVATE: &str = "yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=";
    const PUBLIC: &str = "xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=";
    const PRIVATE_HEX: &str = "c809f3e5317e9575c9b5ed78b638b7ce530dabe85ddab614220241801ddf0669";

    fn parse(text: &str) -> (Option<WireGuardSection>, Vec<Diagnostic>) {
        let (profile, d) = parse_str(text, Arc::from(Path::new("w.conf")), Origin::Main);
        assert!(d.is_empty(), "{:?}", d.into_vec());
        let mut diags = Diagnostics::default();
        let section = parse_section(&profile.sections[0], &mut diags);
        (section, diags.into_vec())
    }

    fn ok(text: &str) -> WireGuardSection {
        let (section, diags) = parse(text);
        assert!(diags.is_empty(), "{diags:?}");
        section.expect("a section")
    }

    fn errors(text: &str) -> Vec<String> {
        let (section, diags) = parse(text);
        assert!(section.is_none(), "{text}");
        diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .inspect(|d| assert_eq!(d.code, codes::E_WIREGUARD_SECTION))
            .map(|d| d.message.clone())
            .collect()
    }

    #[test]
    fn the_manuals_warp_example() {
        let s = ok(&format!(
            "[WireGuard warp]\nprivate-key = {PRIVATE}\nself-ip = 172.16.0.2\nself-ip-v6 = 2606:4700:110:0000::2\n\
dns-server = 1.1.1.1, 2606:4700:4700::1111\n\
peer = (public-key = {PUBLIC}, allowed-ips = \"0.0.0.0/0, ::/0\", endpoint = engage.cloudflareclient.com:2408, client-id = 83/12/235)\n"
        ));
        assert_eq!(s.name, "warp");
        assert_eq!(s.private_key.expose(), &key32(PRIVATE_HEX).unwrap());
        assert_eq!(s.self_ip, Some(Ipv4Addr::new(172, 16, 0, 2)));
        assert_eq!(s.self_ip_v6, Some("2606:4700:110::2".parse().unwrap()));
        assert_eq!(
            s.dns_servers,
            [
                TunnelDns::Server("1.1.1.1:53".parse().unwrap()),
                TunnelDns::Server("[2606:4700:4700::1111]:53".parse().unwrap()),
            ]
        );
        assert!(!s.prefer_ipv6);
        assert_eq!(s.mtu, DEFAULT_MTU);
        let p = &s.peers[0];
        assert_eq!(p.public_key, key32(PUBLIC).unwrap());
        assert_eq!(
            p.allowed_ips,
            [
                "0.0.0.0/0".parse::<IpNet>().unwrap(),
                "::/0".parse().unwrap()
            ]
        );
        assert_eq!(p.endpoint.to_string(), "engage.cloudflareclient.com:2408");
        assert_eq!(p.client_id, Some([83, 12, 235]));
        assert_eq!((p.keepalive, &p.preshared_key), (None, &None));
    }

    #[test]
    fn keys_are_base64_or_hex() {
        let hex = ok(&format!(
            "[WireGuard h]\nprivate-key = {PRIVATE_HEX}\nself-ip = 10.0.0.2\n\
peer = (public-key = {PUBLIC}, allowed-ips = 10.0.0.0/24, endpoint = 192.0.2.1:51820, preshared-key = {PRIVATE_HEX})\n"
        ));
        let b64 = ok(&format!(
            "[WireGuard h]\nprivate-key = {PRIVATE}\nself-ip = 10.0.0.2\n\
peer = (public-key = {PUBLIC}, allowed-ips = 10.0.0.0/24, endpoint = 192.0.2.1:51820, preshared-key = {PRIVATE})\n"
        ));
        assert_eq!(hex, b64);
        assert_eq!(key32(&PRIVATE[..43]), key32(PRIVATE), "padding is optional");
        for bad in ["", "AAAA", &PRIVATE_HEX[..62], &format!("{PRIVATE_HEX}00")] {
            assert_eq!(key32(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_client_id_is_written_three_ways() {
        for form in ["83/12/235", "530ceb", "0x530CEB", "Uwzr"] {
            assert_eq!(client_id(form), Some([83, 12, 235]), "{form}");
        }
        for bad in ["83/12", "83/12/256", "530ce", "Uwz", "Uwzr=", "1/2/3/4"] {
            assert_eq!(client_id(bad), None, "{bad}");
        }
    }

    #[test]
    fn dns_server_entries() {
        let s = ok(&format!(
            "[WireGuard d]\nprivate-key = {PRIVATE}\nself-ip = 10.0.0.2\n\
dns-server = 10.20.0.1, fd00:20::1, 10.20.0.2:5353, [fd00:20::2]:5353, system\n\
peer = (public-key = {PUBLIC}, allowed-ips = 10.20.0.0/16, endpoint = 192.0.2.1:51820)\n"
        ));
        assert_eq!(
            s.dns_servers,
            [
                TunnelDns::Server("10.20.0.1:53".parse().unwrap()),
                TunnelDns::Server("[fd00:20::1]:53".parse().unwrap()),
                TunnelDns::Server("10.20.0.2:5353".parse().unwrap()),
                TunnelDns::Server("[fd00:20::2]:5353".parse().unwrap()),
                TunnelDns::System,
            ]
        );
        let found = errors(&format!(
            "[WireGuard d]\nprivate-key = {PRIVATE}\nself-ip = 10.0.0.2\n\
dns-server = 224.0.0.251, https://dns.test/dns-query, nope\n\
peer = (public-key = {PUBLIC}, allowed-ips = 10.20.0.0/16, endpoint = 192.0.2.1:51820)\n"
        ));
        assert_eq!(
            found,
            [
                "[WireGuard d]: `dns-server` `224.0.0.251`: a multicast address is not accepted",
                "[WireGuard d]: `dns-server` `https://dns.test/dns-query`: an encrypted-DNS URL is not accepted here",
                "[WireGuard d]: invalid `dns-server` `nope` (expected an IP address, an address with a port, or `system`)",
            ]
        );
    }

    #[test]
    fn endpoints_name_a_host_and_a_udp_port() {
        for (text, shown) in [
            ("vpn.example.com:51820", "vpn.example.com:51820"),
            ("192.0.2.1:51820", "192.0.2.1:51820"),
            ("[2001:db8::10]:51820", "[2001:db8::10]:51820"),
            ("VPN.Example.COM:1", "vpn.example.com:1"),
        ] {
            assert_eq!(endpoint(text).unwrap().to_string(), shown, "{text}");
        }
        for bad in [
            "vpn.example.com",
            "vpn.example.com:0",
            "vpn.example.com:65536",
            "2001:db8::10:51820",
            ":51820",
            "a b:1",
        ] {
            assert_eq!(endpoint(bad), None, "{bad}");
        }
    }

    #[test]
    fn peers_accumulate_over_lines_and_within_one() {
        let s = ok(&format!(
            "[WireGuard m]\nprivate-key = {PRIVATE}\nself-ip = 10.0.0.2\nmtu = 1420\nprefer-ipv6 = true\n\
peer = (public-key = {PUBLIC}, allowed-ips = 10.10.0.0/16, endpoint = a.test:51820), (public-key = {PRIVATE}, allowed-ips = \"10.20.0.0/16, 10.30.0.1\", endpoint = b.test:51820, keepalive = 25)\n\
peer = (public-key = {PUBLIC}, allowed-ips = 10.40.1.9/16, endpoint = c.test:51820, keepalive = 0)\n"
        ));
        assert_eq!((s.mtu, s.prefer_ipv6), (1420, true));
        let hosts: Vec<String> = s.peers.iter().map(|p| p.endpoint.to_string()).collect();
        assert_eq!(hosts, ["a.test:51820", "b.test:51820", "c.test:51820"]);
        assert_eq!(
            s.peers[1].allowed_ips,
            [
                "10.20.0.0/16".parse::<IpNet>().unwrap(),
                "10.30.0.1/32".parse().unwrap()
            ]
        );
        assert_eq!(s.peers[1].keepalive, Some(25));
        assert_eq!(s.peers[2].keepalive, None, "0 is off");
        // a prefix written with host bits set is the prefix itself
        assert_eq!(
            s.peers[2].allowed_ips,
            ["10.40.0.0/16".parse::<IpNet>().unwrap()]
        );
    }

    #[test]
    fn what_a_section_must_have() {
        assert_eq!(
            errors("[WireGuard e]\nmtu = 1280\n"),
            [
                "[WireGuard e]: `private-key` is required",
                "[WireGuard e]: at least one of `self-ip` and `self-ip-v6` is required",
                "[WireGuard e]: at least one `peer` is required",
            ]
        );
        assert_eq!(
            errors(&format!(
                "[WireGuard e]\nprivate-key = {PRIVATE}\nself-ip = 10.0.0.2\npeer = (keepalive = 5), (public-key = {PUBLIC})\n"
            )),
            [
                "[WireGuard e]: peer 1: `public-key` is required",
                "[WireGuard e]: peer 1: `allowed-ips` is required",
                "[WireGuard e]: peer 1: `endpoint` is required",
                "[WireGuard e]: peer 2: `allowed-ips` is required",
                "[WireGuard e]: peer 2: `endpoint` is required",
            ]
        );
    }

    #[test]
    fn values_out_of_range_are_errors() {
        let found = errors(&format!(
            "[WireGuard v]\nprivate-key = {PRIVATE}\nself-ip = 10.0.0.2/32\nself-ip-v6 = 10.0.0.2\nmtu = 1500\nprefer-ipv6 = maybe\n\
peer = (public-key = {PUBLIC}, allowed-ips = 10.0.0.0/33, endpoint = a.test:51820, keepalive = 70000, client-id = 1/2)\n\
peer = public-key = {PUBLIC}\n"
        ));
        assert_eq!(
            found,
            [
                "[WireGuard v]: invalid `self-ip` `10.0.0.2/32` (expected an IPv4 address, not a prefix)",
                "[WireGuard v]: invalid `self-ip-v6` `10.0.0.2` (expected an IPv6 address, not a prefix)",
                "[WireGuard v]: invalid `mtu` `1500` (expected 576-1420)",
                "[WireGuard v]: invalid `prefer-ipv6` `maybe` (expected true or false)",
                "[WireGuard v]: peer 1: invalid `allowed-ips` entry `10.0.0.0/33`",
                "[WireGuard v]: peer 1: invalid `keepalive` `70000` (expected 0-65535 seconds)",
                "[WireGuard v]: peer 1: invalid `client-id` `1/2` (expected `a/b/c`, three bytes in hex or four Base64 characters)",
                "[WireGuard v]: peer 2: expected `(public-key = …, allowed-ips = …, endpoint = …)`",
            ]
        );
    }

    /// A key that does not decode is named, never quoted; nor does a
    /// section's `Debug` show one.
    #[test]
    fn keys_are_never_quoted() {
        let (section, diags) = parse(
            "[WireGuard k]\nprivate-key = s3cretPrivate\nself-ip = 10.0.0.2\n\
peer = (public-key = n0tAKey, allowed-ips = 10.0.0.0/8, endpoint = a.test:1, preshared-key = s3cretShared, s3cretLoose)\n",
        );
        assert!(section.is_none());
        let shown: Vec<String> = diags.iter().map(|d| d.to_string()).collect();
        assert_eq!(shown.len(), 4, "{shown:?}");
        for message in &shown {
            for secret in ["s3cretPrivate", "n0tAKey", "s3cretShared", "s3cretLoose"] {
                assert!(!message.contains(secret), "{message}");
            }
        }
        assert!(
            shown[3].ends_with("peer 1: a field without `=` ignored"),
            "{shown:?}"
        );
        let s = ok(&format!(
            "[WireGuard k]\nprivate-key = {PRIVATE}\nself-ip = 10.0.0.2\n\
peer = (public-key = {PUBLIC}, allowed-ips = 10.0.0.0/8, endpoint = a.test:1, preshared-key = {PRIVATE})\n"
        ));
        let debug = format!("{s:?}");
        assert!(debug.contains("Secret(***)"), "{debug}");
        let private = format!("{:?}", s.private_key.expose());
        assert!(!debug.contains(&private[1..20]), "{debug}");
    }

    #[test]
    fn unknown_keys_and_fields_are_warnings() {
        let (section, diags) = parse(&format!(
            "[WireGuard u]\nprivate-key = {PRIVATE}\nself-ip = 10.0.0.2\nlisten-port = 51820\n\
peer = (public-key = {PUBLIC}, allowed-ips = 10.0.0.0/8, endpoint = a.test:1, persistent = 1)\n"
        ));
        assert!(section.is_some());
        let found: Vec<(&str, &str)> = diags.iter().map(|d| (d.code, d.message.as_str())).collect();
        assert_eq!(
            found,
            [
                (
                    codes::W_UNKNOWN_KEY,
                    "[WireGuard u]: unknown key `listen-port` ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "[WireGuard u]: peer 1: unknown field `persistent` ignored"
                ),
            ]
        );
        assert_eq!(diags[0].span.as_ref().map(|s| s.line), Some(4));
    }
}
```

新建 `crates/rurge-config/src/spec/wireguard.rs`：

```rust
//! `wireguard` policy parameters (manual: Policies › WireGuard): the line
//! names a `[WireGuard <name>]` section and the spec carries the section's
//! contents along.

use super::common::CommonOpts;
use super::reader::ParamReader;
use super::tls::refuse_tls;
use crate::diagnostic::codes;
use crate::wireguard::WireGuardSection;

/// Common parameters that mean nothing for a WireGuard policy: the manual
/// has no interface binding for it, and `tfo` / `tos` concern a TCP
/// connection to the server. Warned about (`W0028`) and cleared.
pub const NOT_APPLICABLE: [&str; 4] = ["interface", "allow-other-interface", "tfo", "tos"];

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct WireGuardSpec {
    /// The section `section-name` names, as it is now: an edited section
    /// makes another spec, so a reload builds the policy anew.
    pub section: WireGuardSection,
}

/// Everything `wireguard`-specific on the line, and the common parameters
/// it has no use for taken out of `common`. After an error was reported the
/// returned value is meaningless: the caller checks `r.has_errors()`.
pub fn read_wireguard(
    r: &mut ParamReader<'_>,
    common: &mut CommonOpts,
    sections: &[WireGuardSection],
) -> WireGuardSpec {
    refuse_tls(r);
    for key in NOT_APPLICABLE {
        if r.has(key) {
            r.warn(
                codes::W_PARAM_NOT_APPLICABLE,
                format!("`{key}` does not apply to `wireguard` policies; ignored"),
            );
        }
    }
    common.interface = None;
    common.allow_other_interface = false;
    common.tfo = false;
    common.tos = 0;
    if common.test_url.as_deref().is_some_and(|url| {
        !url.get(..7)
            .is_some_and(|s| s.eq_ignore_ascii_case("http://"))
    }) {
        // never echoed: a subscription line may have set it (M3-D7)
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "the `test-url` of a `wireguard` policy must be a plain http:// URL".to_string(),
        );
    }
    let Some(name) = r
        .str("section-name")
        .map(str::trim)
        .filter(|n| !n.is_empty())
    else {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`section-name` is required".to_string(),
        );
        return WireGuardSpec::default();
    };
    match sections.iter().find(|s| s.name == name) {
        Some(section) => WireGuardSpec {
            section: section.clone(),
        },
        None => {
            r.error(
                codes::E_WIREGUARD_SECTION,
                format!(
                    "`section-name` names `[WireGuard {name}]`, which does not exist or has errors"
                ),
            );
            WireGuardSpec::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Diagnostic;
    use crate::policy::parse_policy;
    use crate::span::Span;
    use crate::spec::IpVersion;
    use crate::spec::common::{Applies, Notes, read_common};
    use std::path::Path;
    use std::sync::Arc;

    fn section(name: &str) -> WireGuardSection {
        WireGuardSection {
            name: name.to_string(),
            mtu: 1280,
            ..WireGuardSection::default()
        }
    }

    fn read(def: &str) -> (WireGuardSpec, CommonOpts, bool, Vec<Diagnostic>) {
        let p = parse_policy("W", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let mut common = read_common(&mut r, Applies::Proxy, &mut Notes::default());
        let spec = read_wireguard(&mut r, &mut common, &[section("home"), section("Office")]);
        let failed = r.has_errors();
        (spec, common, failed, r.finish())
    }

    fn errors(def: &str) -> Vec<(&'static str, String)> {
        let (_, _, failed, diags) = read(def);
        assert!(failed, "{def}");
        diags.into_iter().map(|d| (d.code, d.message)).collect()
    }

    #[test]
    fn the_line_brings_its_section_along() {
        let (spec, _, failed, diags) = read("wireguard, section-name=home");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.section, section("home"));
        // the name matches exactly
        let (spec, _, failed, _) = read("wireguard, section-name = Office");
        assert!(!failed);
        assert_eq!(spec.section.name, "Office");
    }

    #[test]
    fn the_section_name_is_required_and_must_name_a_section() {
        assert_eq!(
            errors("wireguard, test-timeout=5"),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `W`: `section-name` is required".to_string()
            )]
        );
        assert_eq!(
            errors("wireguard, section-name=office"),
            [(
                codes::E_WIREGUARD_SECTION,
                "policy `W`: `section-name` names `[WireGuard office]`, which does not exist or has errors".to_string()
            )]
        );
    }

    /// Interface binding is not supported for WireGuard (manual); `tfo` and
    /// `tos` concern a TCP connection. `ip-version` still picks the family
    /// of the endpoints.
    #[test]
    fn socket_parameters_do_not_apply() {
        let (_, common, failed, diags) = read(
            "wireguard, section-name=home, interface=en0, allow-other-interface=true, tfo=true, tos=0x10, ip-version=v4-only",
        );
        assert!(!failed);
        let found: Vec<(&str, &str)> = diags.iter().map(|d| (d.code, d.message.as_str())).collect();
        assert_eq!(
            found,
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `W`: `interface` does not apply to `wireguard` policies; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `W`: `allow-other-interface` does not apply to `wireguard` policies; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `W`: `tfo` does not apply to `wireguard` policies; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `W`: `tos` does not apply to `wireguard` policies; ignored"
                ),
            ]
        );
        assert_eq!(
            (
                common.interface,
                common.allow_other_interface,
                common.tfo,
                common.tos
            ),
            (None, false, false, 0)
        );
        assert_eq!(common.ip_version, IpVersion::V4Only);
    }

    /// Only plain HTTP (manual); the URL is never quoted.
    #[test]
    fn the_test_url_is_plain_http() {
        let (_, common, failed, _) =
            read("wireguard, section-name=home, test-url=HTTP://10.0.0.1/");
        assert!(!failed);
        assert_eq!(common.test_url.as_deref(), Some("HTTP://10.0.0.1/"));
        assert_eq!(
            errors("wireguard, section-name=home, test-url=https://t.test/?token=t0k3n"),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `W`: the `test-url` of a `wireguard` policy must be a plain http:// URL"
                    .to_string()
            )]
        );
    }

    #[test]
    fn tls_parameters_do_not_apply() {
        let (_, _, failed, diags) = read("wireguard, section-name=home, sni=x.test");
        assert!(!failed);
        assert_eq!(
            (diags[0].code, diags[0].message.as_str()),
            (
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `W`: `sni` does not apply to `wireguard` policies; ignored"
            )
        );
    }
}
```

`crates/rurge-config/src/lib.rs`——把

```rust
pub mod value;
```

换成

```rust
pub mod value;
pub mod wireguard;
```

`crates/rurge-config/src/lib.rs`——把

```rust
pub use value::ParamMap;
```

换成

```rust
pub use value::ParamMap;
pub use wireguard::{PeerEndpoint, TunnelDns, WireGuardPeer, WireGuardSection};
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub mod vmess;
```

换成

```rust
pub mod vmess;
pub mod wireguard;
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub use vmess::{VmessCipher, VmessSpec};
```

换成

```rust
pub use vmess::{VmessCipher, VmessSpec};
pub use wireguard::WireGuardSpec;
```

`crates/rurge-config/src/diagnostic.rs`——把

```rust
    pub const E_POLICY_BUILD: &str = "E0022";
```

换成

```rust
    pub const E_POLICY_BUILD: &str = "E0022";
    /// A `[WireGuard <name>]` section that cannot be used, or a
    /// `section-name` that names no usable section.
    pub const E_WIREGUARD_SECTION: &str = "E0023";
```

`crates/rurge-config/src/diagnostic.rs`——把

```rust
    pub const W_RULES_AFTER_FINAL: &str = "W0019";
```

换成

```rust
    pub const W_RULES_AFTER_FINAL: &str = "W0019";
    /// A named section defined twice (`[Ruleset X]`, `[WireGuard X]`): the
    /// first one is used.
```

`crates/rurge-config/src/config.rs`——把

```rust
use crate::value::{split_definition, split_list};
```

换成

```rust
use crate::value::{split_definition, split_list};
use crate::wireguard::{WireGuardSection, parse_section};
```

`crates/rurge-config/src/config.rs`——把

```rust
    pub keystore: Vec<KeystoreItem>,
```

换成

```rust
    pub keystore: Vec<KeystoreItem>,
    /// Every `[WireGuard <name>]` section without errors, in profile order.
    pub wireguard: Vec<WireGuardSection>,
```

`crates/rurge-config/src/config.rs`——把

```rust
    /// Lowercase hostnames of every proxy server; `[Host]` never applies to them.
    pub fn proxy_hostnames(&self) -> HashSet<String> {
        self.policies
            .iter()
            .filter_map(|p| p.server.as_ref()?.as_domain().map(str::to_string))
```

换成

```rust
    /// Lowercase hostnames of every proxy server — WireGuard peers' endpoints
    /// among them; `[Host]` never applies to them.
    pub fn proxy_hostnames(&self) -> HashSet<String> {
        let endpoints = self
            .wireguard
            .iter()
            .flat_map(|w| &w.peers)
            .map(|p| &p.endpoint.host);
        self.policies
            .iter()
            .filter_map(|p| p.server.as_ref())
            .chain(endpoints)
            .filter_map(|host| host.as_domain().map(str::to_string))
```

`crates/rurge-config/src/config.rs`——把

```rust
            Err(err) => diags.push(Diagnostic::from_parse(err, span.clone())),
        }
    }

    // Deferred and unknown sections.
```

换成

```rust
            Err(err) => diags.push(Diagnostic::from_parse(err, span.clone())),
        }
    }

    // [WireGuard <name>] sections: every one is checked, used or not.
    let mut wireguard = Vec::new();
    let mut wireguard_names: HashSet<String> = HashSet::new();
    for sec in profile.sections_with_prefix("WireGuard ") {
        let name = sec.name["WireGuard ".len()..].trim();
        if !wireguard_names.insert(name.to_string()) {
            diags.push(
                Diagnostic::warning(
                    codes::W_DUPLICATE_RULESET,
                    format!("duplicate [WireGuard {name}] ignored; the first definition is used"),
                )
                .at(sec.span.clone()),
            );
            continue;
        }
        wireguard.extend(parse_section(sec, &mut diags));
    }

    // Deferred and unknown sections.
```

`crates/rurge-config/src/config.rs`——把

```rust
        keystore,
```

换成

```rust
        keystore,
        wireguard,
```

`crates/rurge-config/src/deferred.rs`——把

```rust
    DEFERRED.iter().any(|d| d.eq_ignore_ascii_case(name))
        || starts_with_ci(name, "WireGuard ")
        || starts_with_ci(name, "Tailscale ")
```

换成

```rust
    DEFERRED.iter().any(|d| d.eq_ignore_ascii_case(name)) || starts_with_ci(name, "Tailscale ")
```

`crates/rurge-config/src/redact.rs`——把

```rust
/// token. Over-redacting is the safe side for an endpoint whose purpose is
/// safe output.
const SECRET_PARAMS: [&str; 15] = [
```

换成

```rust
/// token. A `[WireGuard]` peer's `preshared-key` is the manual's spelling;
/// `pre-shared-key` stays for profiles written the other way. Over-redacting
/// is the safe side for an endpoint whose purpose is safe output.
const SECRET_PARAMS: [&str; 16] = [
```

`crates/rurge-config/src/redact.rs`——把

```rust
    "pre-shared-key",
    "base64",
```

换成

```rust
    "pre-shared-key",
    "preshared-key",
    "base64",
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
    // Keystore items: a client certificate, an SSH private key.
    for key in ["client-cert", "private-key"] {
```

换成

```rust
    // Keystore items (a client certificate, an SSH private key) and a
    // `[WireGuard]` section.
    for key in ["client-cert", "private-key", "section-name"] {
```

kitchen-sink 语料里的密钥原来是占位的文字，现在要能通过校验；快照里 `[WireGuard home]` 不再出现在 `W0016` 的列表中：

`tests/corpus/valid/kitchen-sink.conf`——把

```text
private-key = cHJpdmF0ZS1rZXktZXhhbXBsZS1iYXNlNjQtc3RyaW5nMTIzNA==
```

换成

```text
private-key = yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=
```

`tests/corpus/valid/kitchen-sink.conf`——把

```text
peer = (public-key = cHVibGljLWtleS1leGFtcGxlLWJhc2U2NC1zdHJpbmctMTIzNA==, allowed-ips = "0.0.0.0/0, ::/0", endpoint = vpn.example.com:51820, keepalive = 25, client-id = 83/12/235)
```

换成

```text
peer = (public-key = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=, allowed-ips = "0.0.0.0/0, ::/0", endpoint = vpn.example.com:51820, keepalive = 25, client-id = 83/12/235)
```

`crates/rurge-config/tests/snapshots/corpus__corpus__kitchen-sink.snap`——把

```text
    - Port Forwarding
    - WireGuard home
```

换成

```text
    - Port Forwarding
```

`crates/rurge-config/tests/snapshots/corpus__corpus__kitchen-sink.snap`——把

```text
- - "warning[W0016]: sections parsed but inactive in this version: [MITM], [URL Rewrite], [Header Rewrite], [Body Rewrite], [Map Local], [Script], [Panel], [SSID Setting], [Port Forwarding], [WireGuard home], [Tailscale tailnet], [DHCP], [Snell Server], [MTProto], [Testing]"
```

换成

```text
- - "warning[W0016]: sections parsed but inactive in this version: [MITM], [URL Rewrite], [Header Rewrite], [Body Rewrite], [Map Local], [Script], [Panel], [SSID Setting], [Port Forwarding], [Tailscale tailnet], [DHCP], [Snell Server], [MTProto], [Testing]"
```

要点：
- 键与字段的写法见 P15；一个 `peer` 行里可以有几个括号，多行累加；peer 按书写顺序编号（有错的也占号），错误文本里是 `peer <n>`。
- 所有错误只点名键，不引用取值（私钥、预共享密钥写错时也不会出现在输出里）；`keys_are_never_quoted` 断言这一点，并断言 `Debug` 里没有密钥。
- 值写错了只报"值不对"，不再同时报"缺少必填键"（按写过的键记账）。
- `read_wireguard`：TLS 参数不适用（`refuse_tls`）；`interface` / `allow-other-interface` / `tfo` / `tos` 报 `W0028` 并从 `common` 里清掉；`test-url` 只收 `http://`（`E0018`，不引用取值——订阅行也可能写它）；`section-name` 缺了是 `E0018`，指向的节不在 `sections` 里是 `E0023`。
- `reaches_into_profile` 的循环从 `client-cert`、`private-key` 扩到 `section-name`，文本不变（只换键名）。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-config` → 193 passed（新增 `wireguard::tests` 10 条、`spec::wireguard::tests` 5 条、`config::tests` 3 条、`redact` 1 条）；`--test corpus` 2 passed。
Run: `cargo test -p rurge-policy assemble` → 25 passed（新增 `a_subscription_wireguard_line_may_not_use_the_profiles_sections`）。

- [ ] **Step 5: 门禁与提交**

跑门禁（45 个测试二进制，1060 通过 / 1 忽略）。

```bash
git add crates/rurge-config crates/rurge-policy/src/assemble.rs tests/corpus/valid/kitchen-sink.conf
git commit -m "feat(config): [WireGuard <name>] 类型化（E0023）与 WireGuardSpec；订阅行自己的 section-name 进订阅安全门"
```

### Task 2: `rurge-net`——UDP 载体（`Datagram`、`connect_udp`）

WireGuard 的每个 peer 要一条已连接的 UDP 流作载体（设计 6.6）。`rurge-net` 新增 `Datagram`（轮询式的收与发、`peer_addr`、`set_tos`）与 `BoxedDatagram`；`Connector` 多一个 `connect_udp`，默认返回 `Unsupported`（M5 之前的链路载不了 UDP）；`DirectConnector` 实现它：与 TCP 同一个地址计划、取第一个地址、`interface` / `tos` 经 `SocketHook` 作用在 UDP 套接字上、收发缓冲尽量放大（P5）。与设计文字的出入（轮询式、`set_tos` 代替 `set_dscp`）见 P5。

**Files:**
- Modify: `crates/rurge-net/src/connector.rs`（`Datagram`、`BoxedDatagram`、`UDP_BUFFER`、`Connector::connect_udp`、`DirectConnector::connect_udp`、`DirectDatagram`，与用例）

**Interfaces:**
- Consumes: 既有的 `DirectConnector` 的地址计划（`plan`）、`open_socket`、`SocketHook::{set_tos, bind_interface}`、`within`（时限）。
- Produces:
  - `pub trait Datagram: Send + Sync { fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>>; fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>>; fn peer_addr(&self) -> Option<SocketAddr> { None } fn set_tos(&self, tos: u8) -> io::Result<()> { Ok(()) } }`
  - `pub type BoxedDatagram = Box<dyn Datagram>;`
  - `Connector::connect_udp<'a>(&'a self, target: &'a Target, opts: &'a ConnectOpts) -> BoxFuture<'a, io::Result<BoxedDatagram>>`（默认 `Err(Unsupported, "this connection cannot carry UDP")`）；`DirectConnector` 覆盖它

- [ ] **Step 1: 先写用例**

`crates/rurge-net/src/connector.rs`——把

```rust
        assert_eq!(err.to_string(), "connect to slow.test:80 timed out");
```

换成

```rust
        assert_eq!(err.to_string(), "connect to slow.test:80 timed out");
        let err = connector
            .connect_udp(
                &Target::new(HostName::parse("slow.test"), 80),
                &ConnectOpts {
                    timeout: Duration::from_millis(50),
                },
            )
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.to_string(), "connect to slow.test:80 timed out");
```

`crates/rurge-net/src/connector.rs`——把

```rust
            "no usable address for v4.test: every answer was filtered out by ip-version"
        );
    }

    #[test]
    fn targets_are_displayed_the_way_they_are_dialled() {
```

换成

```rust
            "no usable address for v4.test: every answer was filtered out by ip-version"
        );
    }

    async fn send(datagram: &BoxedDatagram, bytes: &[u8]) {
        std::future::poll_fn(|cx| datagram.poll_send(cx, bytes))
            .await
            .unwrap();
    }

    async fn recv(datagram: &BoxedDatagram) -> Vec<u8> {
        let mut buf = [0u8; 64];
        let n = std::future::poll_fn(|cx| {
            let mut read = ReadBuf::new(&mut buf);
            datagram
                .poll_recv(cx, &mut read)
                .map_ok(|()| read.filled().len())
        })
        .await
        .unwrap();
        buf[..n].to_vec()
    }

    /// Answers every datagram with itself.
    async fn udp_echo() -> SocketAddr {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 1500];
            while let Ok((n, from)) = socket.recv_from(&mut buf).await {
                let _ = socket.send_to(&buf[..n], from).await;
            }
        });
        addr
    }

    #[tokio::test]
    async fn a_udp_flow_reaches_its_peer_by_address_and_by_name() {
        let echo = udp_echo().await;
        let c = DirectConnector::new(Arc::new(Fixed(vec![ip("127.0.0.1")])));
        for host in ["127.0.0.1", "echo.test"] {
            let datagram = c
                .connect_udp(
                    &Target::new(HostName::parse(host), echo.port()),
                    &ConnectOpts::default(),
                )
                .await
                .unwrap();
            assert_eq!(datagram.peer_addr(), Some(echo));
            send(&datagram, b"ping").await;
            assert_eq!(recv(&datagram).await, b"ping");
        }
    }

    /// The policy's socket options go on a UDP socket too; `set_tos(0)`
    /// goes back to the policy's own `tos`.
    #[tokio::test]
    async fn the_hook_sees_the_tos_and_the_interface_of_a_udp_flow() {
        let echo = udp_echo().await;
        let hook = Arc::new(RecordingHook::default());
        let connector = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts {
                interface: Some("test0".into()),
                tos: 0x10,
                ..SocketOpts::default()
            },
            hook.clone(),
        );
        let datagram = connector
            .connect_udp(
                &Target::new(HostName::Ip(echo.ip()), echo.port()),
                &ConnectOpts::default(),
            )
            .await
            .unwrap();
        datagram.set_tos(0x88).unwrap();
        datagram.set_tos(0).unwrap();
        assert_eq!(
            *hook.calls.lock().unwrap(),
            ["tos 0x10 V4", "bind test0 V4", "tos 0x88 V4", "tos 0x10 V4"]
        );
    }

    /// Whatever the system allows of `UDP_BUFFER`: more than it gives a
    /// socket of its own accord.
    #[test]
    fn a_udp_flow_has_room_for_a_burst() {
        let socket =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None).unwrap();
        let before = socket.recv_buffer_size().unwrap();
        make_room(&socket);
        assert!(
            socket.recv_buffer_size().unwrap() > before,
            "{before} bytes before"
        );
    }

    #[tokio::test]
    async fn ip_version_picks_the_address_of_a_udp_flow() {
        let with = |answer: Vec<IpAddr>| {
            DirectConnector::with_opts(
                Arc::new(Fixed(answer)),
                SocketOpts {
                    ip_version: IpVersion::V4Only,
                    ..SocketOpts::default()
                },
                Arc::new(NoopSocketHook),
            )
        };
        let name = Target::new(HostName::parse("both.test"), 9);
        let datagram = with(vec![ip("::1"), ip("127.0.0.1")])
            .connect_udp(&name, &ConnectOpts::default())
            .await
            .unwrap();
        assert_eq!(datagram.peer_addr(), Some("127.0.0.1:9".parse().unwrap()));
        let err = with(vec![ip("::1")])
            .connect_udp(&name, &ConnectOpts::default())
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "no usable address for both.test: every answer was filtered out by ip-version"
        );
    }

    #[tokio::test]
    async fn a_connector_without_udp_says_so() {
        struct TcpOnly;
        impl Connector for TcpOnly {
            fn connect<'a>(
                &'a self,
                _target: &'a Target,
                _opts: &'a ConnectOpts,
            ) -> BoxFuture<'a, io::Result<BoxedStream>> {
                Box::pin(std::future::ready(Err(io::Error::other("unused"))))
            }
        }
        let err = TcpOnly
            .connect_udp(
                &Target::new(HostName::parse("a.test"), 1),
                &ConnectOpts::default(),
            )
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        assert_eq!(err.to_string(), "this connection cannot carry UDP");
    }

    #[test]
    fn targets_are_displayed_the_way_they_are_dialled() {
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-net udp`
Expected: FAIL，编译错误（节选）——

```text
error[E0412]: cannot find type `BoxedDatagram` in this scope
   --> crates\rurge-net\src\connector.rs:600:30
error[E0433]: failed to resolve: use of undeclared type `UdpSocket`
   --> crates\rurge-net\src\connector.rs:621:22
error[E0599]: no method named `connect_udp` found for struct `connector::DirectConnector` in the current scope
   --> crates\rurge-net\src\connector.rs:551:14
error[E0425]: cannot find function `make_room` in this scope
   --> crates\rurge-net\src\connector.rs:687:9
error[E0599]: no method named `connect_udp` found for struct `TcpOnly` in the current scope
   --> crates\rurge-net\src\connector.rs:736:14
error: could not compile `rurge-net` (lib test) due to 11 previous errors
```

- [ ] **Step 3: 实现**

`crates/rurge-net/src/connector.rs`——把

```rust
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
```

换成

```rust
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpStream, UdpSocket};

/// What a UDP flow's socket asks for as its buffers each way: what
/// wireguard-go asks for its tunnels.
const UDP_BUFFER: usize = 7 << 20;
```

`crates/rurge-net/src/connector.rs`——把

```rust
pub type BoxedStream = Box<dyn AsyncStream>;
```

换成

```rust
pub type BoxedStream = Box<dyn AsyncStream>;

/// One UDP flow to a fixed peer (phase 2 M4 design 6.6): a WireGuard
/// tunnel's carrier to one of its peers. Polled rather than awaited, so that
/// one task can wait on several carriers at once.
pub trait Datagram: Send + Sync {
    /// Sends `buf` as one datagram.
    fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>>;
    /// Receives one datagram into `buf`.
    fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>>;
    /// Where the datagrams go, when the carrier knows.
    fn peer_addr(&self) -> Option<SocketAddr> {
        None
    }
    /// The IP TOS (IPv6 traffic class) byte of the datagrams sent from now
    /// on; `0` goes back to what the carrier started with. A carrier that
    /// cannot mark its datagrams ignores it.
    fn set_tos(&self, tos: u8) -> io::Result<()> {
        let _ = tos;
        Ok(())
    }
}

pub type BoxedDatagram = Box<dyn Datagram>;
```

`crates/rurge-net/src/connector.rs`——把

```rust
    ) -> BoxFuture<'a, io::Result<BoxedStream>>;
```

换成

```rust
    ) -> BoxFuture<'a, io::Result<BoxedStream>>;

    /// A UDP flow to `target` (phase 2 M4 design 6.6); `Unsupported` from a
    /// connector that carries none.
    fn connect_udp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
        let _ = (target, opts);
        Box::pin(std::future::ready(Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this connection cannot carry UDP",
        ))))
    }
```

`crates/rurge-net/src/connector.rs`——把

```rust
/// policy's `ip-version` allows (`crate::socket::race`).
```

换成

```rust
/// policy's `ip-version` allows (`crate::socket::race`). Plain UDP too: the
/// first of those addresses, nothing to race.
```

`crates/rurge-net/src/connector.rs`——把

```rust
async fn connect_one(
    addr: SocketAddr,
```

换成

```rust
/// A non-blocking socket of `addr`'s family with the policy's socket
/// options on it.
fn open_socket(
    addr: SocketAddr,
    kind: socket2::Type,
```

`crates/rurge-net/src/connector.rs`——把

```rust
) -> io::Result<TcpStream> {
```

换成

```rust
) -> io::Result<socket2::Socket> {
```

`crates/rurge-net/src/connector.rs`——把

```rust
    let socket = socket2::Socket::new(domain, socket2::Type::STREAM, Some(socket2::Protocol::TCP))?;
```

换成

```rust
    let protocol = if kind == socket2::Type::DGRAM {
        socket2::Protocol::UDP
    } else {
        socket2::Protocol::TCP
    };
    let socket = socket2::Socket::new(domain, kind, Some(protocol))?;
```

`crates/rurge-net/src/connector.rs`——把

```rust
            tracing::warn!(interface = %interface, error = %e, "interface unavailable; using the default one (allow-other-interface)");
        }
    }
    let std_stream: std::net::TcpStream = socket.into();
```

换成

```rust
            tracing::warn!(interface = %interface, error = %e, "interface unavailable; using the default one (allow-other-interface)");
        }
    }
    Ok(socket)
}

async fn connect_one(
    addr: SocketAddr,
    opts: &SocketOpts,
    hook: &dyn SocketHook,
    fallback_logged: &AtomicBool,
) -> io::Result<TcpStream> {
    let socket = open_socket(addr, socket2::Type::STREAM, opts, hook, fallback_logged)?;
    let std_stream: std::net::TcpStream = socket.into();
```

`crates/rurge-net/src/connector.rs`——把

```rust
        host => format!("{host}:{}", target.port),
    }
}

impl Connector for DirectConnector {
```

换成

```rust
        host => format!("{host}:{}", target.port),
    }
}

impl DirectConnector {
    /// The addresses to try first and the ones that join later
    /// (`plan_addresses`); the first list is never empty.
    async fn plan(&self, target: &Target) -> io::Result<(Vec<IpAddr>, Vec<IpAddr>)> {
        match &target.host {
            // `ip-version` only means something for a host name (manual)
            HostName::Ip(ip) => Ok((vec![*ip], Vec::new())),
            HostName::Domain(d) => {
                let addrs = self.resolver.resolve(d).await?;
                // Guards a `Resolve` implementation that answers with
                // an empty list instead of an error; the wording
                // matches what the two production resolvers
                // (`SystemResolve`, `rurge_dns::Resolver`) already
                // return as an `Err` themselves in that case.
                if addrs.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("no addresses for {d}"),
                    ));
                }
                let planned = plan_addresses(addrs, self.opts.ip_version, self.opts.v6_first);
                if planned.0.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "no usable address for {d}: every answer was filtered out by ip-version"
                        ),
                    ));
                }
                Ok(planned)
            }
        }
    }
}

/// `attempt`, with `timeout` covering name resolution and everything after.
async fn within<T>(
    target: &Target,
    timeout: Duration,
    attempt: impl std::future::Future<Output = io::Result<T>>,
) -> io::Result<T> {
    match tokio::time::timeout(timeout, attempt).await {
        Ok(done) => done,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("connect to {} timed out", display_target(target)),
        )),
    }
}

impl Connector for DirectConnector {
```

`crates/rurge-net/src/connector.rs`——把

```rust
        Box::pin(async move {
            let attempt = async {
                let (primary, secondary) = match &target.host {
                    // `ip-version` only means something for a host name (manual)
                    HostName::Ip(ip) => (vec![*ip], Vec::new()),
                    HostName::Domain(d) => {
                        let addrs = self.resolver.resolve(d).await?;
                        // Guards a `Resolve` implementation that answers with
                        // an empty list instead of an error; the wording
                        // matches what the two production resolvers
                        // (`SystemResolve`, `rurge_dns::Resolver`) already
                        // return as an `Err` themselves in that case.
                        if addrs.is_empty() {
                            return Err(io::Error::new(
                                io::ErrorKind::NotFound,
                                format!("no addresses for {d}"),
                            ));
                        }
                        let planned =
                            plan_addresses(addrs, self.opts.ip_version, self.opts.v6_first);
                        if planned.0.is_empty() {
                            return Err(io::Error::new(
                                io::ErrorKind::NotFound,
                                format!(
                                    "no usable address for {d}: every answer was filtered out by ip-version"
                                ),
                            ));
                        }
                        planned
                    }
                };
                let port = target.port;
                let with_port = |ips: Vec<IpAddr>| -> Vec<SocketAddr> {
                    ips.into_iter()
                        .map(|ip| SocketAddr::new(ip, port))
                        .collect()
                };
                let (socket_opts, hook, logged) = (
                    self.opts.clone(),
                    self.hook.clone(),
                    self.fallback_logged.clone(),
                );
                race(with_port(primary), with_port(secondary), move |addr| {
                    let (socket_opts, hook, logged) =
                        (socket_opts.clone(), hook.clone(), logged.clone());
                    async move { connect_one(addr, &socket_opts, hook.as_ref(), &logged).await }
                })
                .await
            };
            match tokio::time::timeout(opts.timeout, attempt).await {
                Ok(Ok(stream)) => Ok(Box::new(stream) as BoxedStream),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("connect to {} timed out", display_target(target)),
                )),
            }
        })
```

换成

```rust
        Box::pin(within(target, opts.timeout, async move {
            let (primary, secondary) = self.plan(target).await?;
            let port = target.port;
            let with_port = |ips: Vec<IpAddr>| -> Vec<SocketAddr> {
                ips.into_iter()
                    .map(|ip| SocketAddr::new(ip, port))
                    .collect()
            };
            let (socket_opts, hook, logged) = (
                self.opts.clone(),
                self.hook.clone(),
                self.fallback_logged.clone(),
            );
            let stream = race(with_port(primary), with_port(secondary), move |addr| {
                let (socket_opts, hook, logged) =
                    (socket_opts.clone(), hook.clone(), logged.clone());
                async move { connect_one(addr, &socket_opts, hook.as_ref(), &logged).await }
            })
            .await?;
            Ok(Box::new(stream) as BoxedStream)
        }))
    }

    fn connect_udp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
        Box::pin(within(target, opts.timeout, async move {
            // nothing answers a UDP "connection": the first address it is
            let (primary, _) = self.plan(target).await?;
            let addr = SocketAddr::new(primary[0], target.port);
            let socket = open_socket(
                addr,
                socket2::Type::DGRAM,
                &self.opts,
                self.hook.as_ref(),
                &self.fallback_logged,
            )?;
            make_room(&socket);
            socket.connect(&addr.into())?;
            let socket = UdpSocket::from_std(socket.into())?;
            Ok(Box::new(DirectDatagram {
                socket,
                family: Family::of(&addr.ip()),
                hook: self.hook.clone(),
                tos: self.opts.tos,
            }) as BoxedDatagram)
        }))
    }
}

/// A UDP flow may carry a tunnel at full speed: its socket's buffers take a
/// burst. Best effort: the system may cap them.
fn make_room(socket: &socket2::Socket) {
    let _ = socket.set_recv_buffer_size(UDP_BUFFER);
    let _ = socket.set_send_buffer_size(UDP_BUFFER);
}

/// A connected UDP socket carrying the policy's socket options.
struct DirectDatagram {
    socket: UdpSocket,
    family: Family,
    hook: Arc<dyn SocketHook>,
    /// The policy's own `tos`: what `set_tos(0)` goes back to.
    tos: u8,
}

impl Datagram for DirectDatagram {
    fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.socket.poll_send(cx, buf)
    }

    fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        self.socket.poll_recv(cx, buf)
    }

    fn peer_addr(&self) -> Option<SocketAddr> {
        self.socket.peer_addr().ok()
    }

    fn set_tos(&self, tos: u8) -> io::Result<()> {
        let tos = if tos == 0 { self.tos } else { tos };
        self.hook
            .set_tos(&socket2::SockRef::from(&self.socket), self.family, tos)
```

要点：
- `Datagram` 是轮询式的：设备任务要同时等几个 peer 的载体（P5）。`DirectDatagram` 包着一个已连接的 tokio `UdpSocket`；`set_tos(0)` 回到策略自己的 `tos`，其它值经 `SocketHook::set_tos` 设到套接字上（用例里的 `RecordingHook` 记下每一次调用）。
- `connect_udp` 用与 TCP 相同的地址计划（`ip-version`、`[General] ipv6`），取第一个地址——UDP 没有"连上"可以比较，不竞速；整个过程受 `ConnectOpts.timeout` 约束（与 TCP 共用 `within`）。
- `make_room`：收发缓冲尽量设为 7 MiB（`UDP_BUFFER`，wireguard-go 的取值），失败不算错——系统可能封顶。回环上 Windows 默认的 64 KiB 收缓冲装不下一个 TCP 窗口的突发（P5）。用例只断言"比系统默认给的大"：Linux 默认封在约 416 KiB，也满足。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-net` → 47 passed（新增 5 条：经地址与经名字的 UDP 往返、`SocketHook` 看到 UDP 流的 TOS 与网卡、`ip-version` 挑 UDP 流的地址、UDP 流的缓冲、不载 UDP 的连接器；另有 `the_timeout_covers_name_resolution` 一条扩展到 UDP）。

- [ ] **Step 5: 门禁与提交**

跑门禁（45 个测试二进制，1065 通过 / 1 忽略）。

```bash
git add crates/rurge-net/src/connector.rs
git commit -m "feat(net): UDP 载体——Datagram、Connector::connect_udp 与 DirectConnector 的实现（按 ip-version、set_tos、7 MiB 缓冲）"
```

### Task 3: `rurge-proto-wireguard` 骨架——路由表、`client-id`、协议栈与内存里的对端

新 crate `rurge-proto-wireguard`（设计第 3 节、M4-D3 ～ D5）。本任务是不做 I/O 的三块：`routes`（按全部 peer 的 `allowed-ips` 建的 IPv4、IPv6 两张最长前缀表，同一前缀两个 peer 都列了时后写的生效）、`wire`（报文类型、`client-id` 的写与清、握手包的 TOS）与 `stack`（锁里的全部状态：smoltcp 的接口与套接字、内存里的收发两个包队列、每个 peer 一个 boringtun `Tunn`；调用方把载体收到的交进来、把要发的拿出去，P1、P2），外加 `testing` 里的 `PeerCore`：boringtun 响应端加它自己的 smoltcp 主机（P20），本任务的用例在内存里把两边的报文直接递来递去。设备任务、流与出站在 Task 4。

**首次构建要联网**：cargo 从 crates.io 下载 boringtun 0.7.1、smoltcp 0.12.0 及其依赖（`Cargo.lock` 从 474 个包变为 505 个）；项目所有者已同意（M4-D3 / D4）。不要为此改用别的源或 `--offline`。

**Files:**
- Modify: `Cargo.toml`（工作区依赖：`rurge-proto-wireguard`、`boringtun`、`smoltcp`）
- Create: `crates/rurge-proto-wireguard/Cargo.toml`、`src/lib.rs`
- Create: `crates/rurge-proto-wireguard/src/routes.rs`（`Routes`，与用例）
- Create: `crates/rurge-proto-wireguard/src/wire.rs`（报文类型与 `client-id`，与用例）
- Create: `crates/rurge-proto-wireguard/src/stack.rs`（`Stack`、`Outgoing`、`Refusal`、`Queues`，与用例）
- Create: `crates/rurge-proto-wireguard/src/testing/mod.rs`（`PeerCore`、`PeerOpts`、`keypair`、`section`、`ECHO_PORT`）
- `Cargo.lock` 由 cargo 自己更新

**Interfaces:**
- Consumes: Task 1 的 `rurge_config::wireguard::{WireGuardSection, WireGuardPeer, PeerEndpoint, DEFAULT_MTU}`、`rurge_config::spec::Secret`。
- Produces:
  - `rurge_proto_wireguard::Routes`：`Routes::new<'a>(allowed: impl IntoIterator<Item = &'a [IpNet]>) -> Routes`（第 i 项是 peer i 的 `allowed-ips`）、`lookup(&self, ip: IpAddr) -> Option<usize>`
  - `rurge_proto_wireguard::wire::{HANDSHAKE_INITIATION: u8 (1), HANDSHAKE_RESPONSE: u8 (2), HANDSHAKE_TOS: u8 (0x88), message_type(&[u8]) -> Option<u8>, mark(&mut [u8], Option<[u8; 3]>), unmark(&mut [u8])}`
  - `rurge_proto_wireguard::{Stack, Outgoing, Refusal}`：`pub struct Outgoing { pub peer: usize, pub datagram: Vec<u8> }`；`pub enum Refusal { NoAddress(IpAddr), NoRoute(IpAddr), Unaddressable(IpAddr), NoPort }`（`Display` 即 P21 的错误文本）；`Stack::new(&WireGuardSection) -> Stack`、`initiate(&mut self, out: &mut Vec<Outgoing>)`、`receive(&mut self, peer: usize, datagram: &mut [u8], now: Instant, out: &mut Vec<Outgoing>)`、`advance(&mut self, now: Instant, out: &mut Vec<Outgoing>) -> Option<Duration>`、`tick(&mut self, out: &mut Vec<Outgoing>)`、`connect(&mut self, to: SocketAddr) -> Result<SocketHandle, Refusal>`、`tcp(&mut self, SocketHandle) -> &mut tcp::Socket<'static>`、`release(&mut self, SocketHandle, now: Instant)`（Task 5、6、8 再加方法、改返回值）；`pub(crate) struct Queues`
  - 特性 `testing`（下游的 dev 依赖开启）：`ECHO_PORT: u16 (7)`、`keypair() -> ([u8; 32], [u8; 32])`（私钥、公钥）、`section(private: [u8; 32], self_ip: Ipv4Addr, peers: &[([u8; 32], &[&str])]) -> WireGuardSection`（endpoint 是占位的回环端口）、`PeerOpts { address, address_v6, client_id, preshared_key, mtu }`（`Default`）、`PeerCore::new(client_public: [u8; 32], opts: &PeerOpts)`，方法 `public_key`、`open`、`receive(&mut [u8], &mut Vec<Vec<u8>>)`、`advance(&mut Vec<Vec<u8>>) -> Option<Duration>`、`tick`、`inject`、`ping`，字段 `handshakes`、`reserved`、`pongs`、`accepted`、`resets`

- [ ] **Step 1: 依赖、crate 骨架与内存里的对端**

`Cargo.toml`——把

```toml
rand = "0.10"
```

换成

```toml
rand = "0.10"
rurge-proto-wireguard = { path = "crates/rurge-proto-wireguard" }
# only the sans-IO `noise::Tunn`: no default features (`device` is a whole TUN driver)
boringtun = { version = "0.7.1", default-features = false }
# the tunnel's own IP stack: TCP with Reno congestion control, fragments of up to
# 16 KiB reassembled; no phy, no log
smoltcp = { version = "0.12.0", default-features = false, features = ["std", "medium-ip", "proto-ipv4", "proto-ipv6", "proto-ipv4-fragmentation", "proto-ipv6-fragmentation", "socket-tcp", "socket-tcp-reno", "async", "reassembly-buffer-size-16384", "reassembly-buffer-count-4"] }
```

新建 `crates/rurge-proto-wireguard/Cargo.toml`：

```toml
[package]
name = "rurge-proto-wireguard"
description = "The wireguard outbound of rurge: a userspace WireGuard tunnel with a TCP/IP stack of its own"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
rurge-config.workspace = true
boringtun.workspace = true
smoltcp.workspace = true
prefix-trie.workspace = true
ipnet.workspace = true
getrandom.workspace = true
tracing.workspace = true

[features]
# Loopback WireGuard peer (`rurge_proto_wireguard::testing`); enabled by dependants' dev-dependencies.
testing = []

[lints]
workspace = true
```

新建 `crates/rurge-proto-wireguard/src/lib.rs`：

```rust
//! The `wireguard` outbound (phase 2 M4 design §6): a userspace WireGuard
//! tunnel — boringtun's sans-IO `Tunn` for each peer and a smoltcp TCP/IP
//! stack of its own — that TCP connections are dialled through.

pub mod routes;
pub mod stack;
pub mod wire;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use routes::Routes;
pub use stack::{Outgoing, Refusal, Stack};
```

新建 `crates/rurge-proto-wireguard/src/testing/mod.rs`：

```rust
//! A WireGuard peer for the tests of this crate and of its dependants
//! (feature `testing`). `PeerCore` is the peer without I/O: boringtun
//! answering the client's handshakes, and a smoltcp host of its own that
//! answers on every address routed to it — a TCP echo service on port 7.

use crate::stack::Queues;
use crate::wire;
use boringtun::noise::{Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};
use rurge_config::HostName;
use rurge_config::spec::Secret;
use rurge_config::wireguard::{DEFAULT_MTU, PeerEndpoint, WireGuardPeer, WireGuardSection};
use smoltcp::iface::{Config, Interface, PollResult, SocketHandle, SocketSet};
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::socket::tcp;
use smoltcp::wire::{
    HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, IpProtocol, Ipv4Packet,
};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::{Duration, Instant};

/// The TCP echo service of a peer.
pub const ECHO_PORT: u16 = 7;
/// Connections the echo service takes at once: SYNs that arrive together.
const BACKLOG: usize = 8;

/// A fresh key pair: private, public.
pub fn keypair() -> ([u8; 32], [u8; 32]) {
    let mut private = [0u8; 32];
    getrandom::fill(&mut private).expect("randomness");
    let public = PublicKey::from(&StaticSecret::from(private)).to_bytes();
    (private, public)
}

/// A client's section with `peers`, each a public key and its
/// `allowed-ips`; the endpoints are placeholders, for a test to replace
/// where they matter (as any other field).
pub fn section(
    private: [u8; 32],
    self_ip: Ipv4Addr,
    peers: &[([u8; 32], &[&str])],
) -> WireGuardSection {
    WireGuardSection {
        name: "test".to_string(),
        private_key: Secret::new(private),
        self_ip: Some(self_ip),
        self_ip_v6: None,
        dns_servers: Vec::new(),
        prefer_ipv6: false,
        mtu: DEFAULT_MTU,
        peers: peers
            .iter()
            .enumerate()
            .map(|(i, (public_key, allowed))| WireGuardPeer {
                public_key: *public_key,
                allowed_ips: allowed.iter().map(|a| a.parse().unwrap()).collect(),
                endpoint: PeerEndpoint {
                    host: HostName::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                    port: 9 + i as u16,
                },
                preshared_key: None,
                keepalive: None,
                client_id: None,
            })
            .collect(),
    }
}

/// How a test peer behaves.
#[derive(Clone, Debug)]
pub struct PeerOpts {
    /// Its own addresses in the tunnel; it answers on every address routed
    /// to it all the same.
    pub address: Ipv4Addr,
    pub address_v6: Ipv6Addr,
    /// Written into every message it sends; a message without it is
    /// ignored (WARP routes by it).
    pub client_id: Option<[u8; 3]>,
    pub preshared_key: Option<[u8; 32]>,
    /// The MTU of its own stack.
    pub mtu: usize,
}

impl Default for PeerOpts {
    fn default() -> PeerOpts {
        PeerOpts {
            address: Ipv4Addr::new(10, 0, 0, 1),
            address_v6: "fd00::1".parse().expect("an address"),
            client_id: None,
            preshared_key: None,
            mtu: 1420,
        }
    }
}

struct Conn {
    handle: SocketHandle,
    /// The client finished its side.
    fin: bool,
}

pub struct PeerCore {
    tunnel: Tunn,
    public_key: [u8; 32],
    iface: Interface,
    sockets: SocketSet<'static>,
    queues: Queues,
    listeners: Vec<SocketHandle>,
    conns: Vec<Conn>,
    scratch: Vec<u8>,
    epoch: Instant,
    client_id: Option<[u8; 3]>,
    /// Handshakes the client started.
    pub handshakes: usize,
    /// The reserved bytes of every message received.
    pub reserved: Vec<[u8; 3]>,
    /// Echo replies received: identifier and data length.
    pub pongs: Vec<(u16, usize)>,
    /// TCP connections accepted.
    pub accepted: usize,
    /// TCP connections the client reset.
    pub resets: usize,
}

/// `message` with `client_id` in it.
fn marked(message: &mut [u8], client_id: Option<[u8; 3]>) -> Vec<u8> {
    wire::mark(message, client_id);
    message.to_vec()
}

impl PeerCore {
    /// A peer the client of `client_public` may reach.
    pub fn new(client_public: [u8; 32], opts: &PeerOpts) -> PeerCore {
        let (private, public_key) = keypair();
        let tunnel = Tunn::new(
            StaticSecret::from(private),
            PublicKey::from(client_public),
            opts.preshared_key,
            None,
            1,
            None,
        );
        let mut queues = Queues::new(opts.mtu);
        let mut iface = Interface::new(
            Config::new(HardwareAddress::Ip),
            &mut queues,
            smoltcp::time::Instant::ZERO,
        );
        iface.update_ip_addrs(|addrs| {
            let _ = addrs.push(IpCidr::new(IpAddress::Ipv4(opts.address), 32));
            let _ = addrs.push(IpCidr::new(IpAddress::Ipv6(opts.address_v6), 128));
        });
        let _ = iface.routes_mut().add_default_ipv4_route(opts.address);
        let _ = iface.routes_mut().add_default_ipv6_route(opts.address_v6);
        // whatever the client sends through it is for it
        iface.set_any_ip(true);
        let mut sockets = SocketSet::new(Vec::new());
        let listeners = (0..BACKLOG)
            .map(|_| listen(&mut sockets, ECHO_PORT))
            .collect();
        PeerCore {
            tunnel,
            public_key,
            iface,
            sockets,
            queues,
            listeners,
            conns: Vec::new(),
            scratch: vec![0; 65536 + 32],
            epoch: Instant::now(),
            client_id: opts.client_id,
            handshakes: 0,
            reserved: Vec::new(),
            pongs: Vec::new(),
            accepted: 0,
            resets: 0,
        }
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.public_key
    }

    /// Connections open now.
    pub fn open(&self) -> usize {
        self.conns.len()
    }

    /// A message from the client; the answers go to `out`.
    pub fn receive(&mut self, message: &mut [u8], out: &mut Vec<Vec<u8>>) {
        let Some(reserved) = message.get(1..4) else {
            return;
        };
        let reserved: [u8; 3] = reserved.try_into().expect("three bytes");
        self.reserved.push(reserved);
        if self.client_id.is_some_and(|id| id != reserved) {
            return;
        }
        wire::unmark(message);
        let initiation = wire::message_type(message) == Some(wire::HANDSHAKE_INITIATION);
        let packet = match self.tunnel.decapsulate(None, message, &mut self.scratch) {
            TunnResult::WriteToNetwork(answer) => {
                if initiation {
                    self.handshakes += 1;
                }
                out.push(marked(answer, self.client_id));
                while let TunnResult::WriteToNetwork(more) =
                    self.tunnel.decapsulate(None, &[], &mut self.scratch)
                {
                    out.push(marked(more, self.client_id));
                }
                return;
            }
            TunnResult::WriteToTunnelV4(packet, _) | TunnResult::WriteToTunnelV6(packet, _) => {
                packet.to_vec()
            }
            _ => return,
        };
        if !self.pong(&packet) {
            self.queues.rx.push_back(packet);
        }
    }

    /// An echo reply: noted, not handed to the stack (it would drop it).
    fn pong(&mut self, packet: &[u8]) -> bool {
        let Ok(ip) = Ipv4Packet::new_checked(packet) else {
            return false;
        };
        if ip.next_header() != IpProtocol::Icmp {
            return false;
        }
        let Ok(icmp) = Icmpv4Packet::new_checked(ip.payload()) else {
            return false;
        };
        match Icmpv4Repr::parse(&icmp, &ChecksumCapabilities::default()) {
            Ok(Icmpv4Repr::EchoReply { ident, data, .. }) => {
                self.pongs.push((ident, data.len()));
                true
            }
            _ => false,
        }
    }

    /// Runs its stack and its services; what it sends goes to `out`. How
    /// long until the stack wants to run again, when it has a deadline.
    pub fn advance(&mut self, out: &mut Vec<Vec<u8>>) -> Option<Duration> {
        let at = smoltcp::time::Instant::from_micros(self.epoch.elapsed().as_micros() as i64);
        self.iface.poll(at, &mut self.queues, &mut self.sockets);
        self.serve();
        while self.iface.poll(at, &mut self.queues, &mut self.sockets)
            == PollResult::SocketStateChanged
        {
            self.serve();
        }
        while let Some(packet) = self.queues.tx.pop_front() {
            self.inject(&packet, out);
        }
        self.iface.poll_delay(at, &self.sockets).map(Duration::from)
    }

    /// WireGuard's timers.
    pub fn tick(&mut self, out: &mut Vec<Vec<u8>>) {
        if let TunnResult::WriteToNetwork(message) = self.tunnel.update_timers(&mut self.scratch) {
            out.push(marked(message, self.client_id));
        }
    }

    /// Sends the IP packet `packet` into the tunnel as it is.
    pub fn inject(&mut self, packet: &[u8], out: &mut Vec<Vec<u8>>) {
        if let TunnResult::WriteToNetwork(message) =
            self.tunnel.encapsulate(packet, &mut self.scratch)
        {
            out.push(marked(message, self.client_id));
        }
    }

    /// An echo request with `size` bytes of data from `from` to `to`, in
    /// fragments of `fragment` bytes (a multiple of 8) when given.
    pub fn ping(
        &mut self,
        from: Ipv4Addr,
        to: Ipv4Addr,
        ident: u16,
        size: usize,
        fragment: Option<usize>,
        out: &mut Vec<Vec<u8>>,
    ) {
        let data = vec![0x5a; size];
        let request = Icmpv4Repr::EchoRequest {
            ident,
            seq_no: 1,
            data: &data,
        };
        let mut payload = vec![0; request.buffer_len()];
        request.emit(
            &mut Icmpv4Packet::new_unchecked(&mut payload),
            &ChecksumCapabilities::default(),
        );
        let step = fragment.unwrap_or(payload.len());
        let pieces: Vec<&[u8]> = payload.chunks(step).collect();
        for (k, piece) in pieces.iter().enumerate() {
            let mut packet = vec![0u8; 20 + piece.len()];
            let mut ip = Ipv4Packet::new_unchecked(&mut packet);
            ip.set_version(4);
            ip.set_header_len(20);
            ip.set_total_len((20 + piece.len()) as u16);
            ip.set_ident(ident);
            ip.clear_flags();
            ip.set_more_frags(k + 1 < pieces.len());
            ip.set_frag_offset((k * step) as u16);
            ip.set_hop_limit(64);
            ip.set_next_header(IpProtocol::Icmp);
            ip.set_src_addr(from);
            ip.set_dst_addr(to);
            ip.payload_mut().copy_from_slice(piece);
            ip.fill_checksum();
            self.inject(&packet, out);
        }
    }

    /// Echoes what each connection receives; a listener that took a
    /// connection is replaced, a closed connection forgotten.
    fn serve(&mut self) {
        for k in 0..self.listeners.len() {
            let handle = self.listeners[k];
            if self.sockets.get::<tcp::Socket>(handle).state() == tcp::State::Listen {
                continue;
            }
            self.accepted += 1;
            self.conns.push(Conn { handle, fin: false });
            self.listeners[k] = listen(&mut self.sockets, ECHO_PORT);
        }
        let sockets = &mut self.sockets;
        let mut resets = 0;
        self.conns.retain_mut(|conn| {
            let socket = sockets.get_mut::<tcp::Socket>(conn.handle);
            let room = socket.send_capacity() - socket.send_queue();
            let mut buf = vec![0u8; room.min(socket.recv_queue())];
            if let Ok(n) = socket.recv_slice(&mut buf) {
                let _ = socket.send_slice(&buf[..n]);
            }
            conn.fin |= matches!(
                socket.state(),
                tcp::State::CloseWait
                    | tcp::State::LastAck
                    | tcp::State::Closing
                    | tcp::State::TimeWait
            );
            // the client is done and everything went back: done too
            if conn.fin && !socket.may_recv() && socket.send_queue() == 0 {
                socket.close();
            }
            match socket.state() {
                tcp::State::Closed | tcp::State::TimeWait => {
                    if !conn.fin {
                        resets += 1;
                    }
                    sockets.remove(conn.handle);
                    false
                }
                _ => true,
            }
        });
        self.resets += resets;
    }
}

fn listen(sockets: &mut SocketSet<'static>, port: u16) -> SocketHandle {
    let mut socket = tcp::Socket::new(
        tcp::SocketBuffer::new(vec![0; 256 * 1024]),
        tcp::SocketBuffer::new(vec![0; 256 * 1024]),
    );
    socket.listen(port).expect("a port to listen on");
    sockets.add(socket)
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto-wireguard`
Expected: FAIL，编译错误（首次运行会先下载并编译 boringtun、smoltcp 一族）——

```text
error[E0583]: file not found for module `routes`
 --> crates\rurge-proto-wireguard\src\lib.rs:5:1
error[E0583]: file not found for module `stack`
 --> crates\rurge-proto-wireguard\src\lib.rs:6:1
error[E0583]: file not found for module `wire`
 --> crates\rurge-proto-wireguard\src\lib.rs:7:1
error: could not compile `rurge-proto-wireguard` (lib) due to 3 previous errors
```

- [ ] **Step 3: 实现（新模块自带用例）**

新建 `crates/rurge-proto-wireguard/src/routes.rs`：

```rust
//! Cryptokey routing (manual: `allowed-ips`): the peer a destination goes
//! to is the one with the longest prefix covering it, IPv4 and IPv6 in
//! tables of their own; and a peer may only send from what routes to it.

use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use prefix_trie::PrefixMap;
use std::net::IpAddr;

#[derive(Clone, Debug, Default)]
pub struct Routes {
    v4: PrefixMap<Ipv4Net, usize>,
    v6: PrefixMap<Ipv6Net, usize>,
}

impl Routes {
    /// `allowed` is each peer's `allowed-ips`, in peer order. A prefix two
    /// peers both list goes to the later one, as with `wg`.
    pub fn new<'a>(allowed: impl IntoIterator<Item = &'a [IpNet]>) -> Routes {
        let mut routes = Routes::default();
        for (peer, nets) in allowed.into_iter().enumerate() {
            for net in nets {
                match net.trunc() {
                    IpNet::V4(net) => routes.v4.insert(net, peer),
                    IpNet::V6(net) => routes.v6.insert(net, peer),
                };
            }
        }
        routes
    }

    /// The peer `ip` is routed to.
    pub fn lookup(&self, ip: IpAddr) -> Option<usize> {
        match ip {
            IpAddr::V4(v4) => self.v4.get_lpm(&Ipv4Net::from(v4)).map(|(_, p)| *p),
            IpAddr::V6(v6) => self.v6.get_lpm(&Ipv6Net::from(v6)).map(|(_, p)| *p),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routes(peers: &[&[&str]]) -> Routes {
        let nets: Vec<Vec<IpNet>> = peers
            .iter()
            .map(|p| p.iter().map(|n| n.parse().unwrap()).collect())
            .collect();
        Routes::new(nets.iter().map(Vec::as_slice))
    }

    fn at(r: &Routes, ip: &str) -> Option<usize> {
        r.lookup(ip.parse().unwrap())
    }

    #[test]
    fn the_longest_prefix_picks_the_peer() {
        let r = routes(&[
            &["0.0.0.0/0", "::/0"],
            &["10.0.0.0/8"],
            &["10.2.0.0/16", "fd00::/64"],
        ]);
        assert_eq!(at(&r, "192.0.2.1"), Some(0));
        assert_eq!(at(&r, "10.1.0.1"), Some(1));
        assert_eq!(at(&r, "10.2.9.9"), Some(2));
        assert_eq!(at(&r, "2001:db8::1"), Some(0));
        assert_eq!(at(&r, "fd00::7"), Some(2));
    }

    #[test]
    fn the_families_have_tables_of_their_own() {
        let r = routes(&[&["10.0.0.0/8"]]);
        assert_eq!(at(&r, "10.0.0.1"), Some(0));
        assert_eq!(
            at(&r, "::ffff:10.0.0.1"),
            None,
            "an IPv6 address, whatever it maps"
        );
        assert_eq!(at(&r, "192.0.2.1"), None);
        let r = routes(&[&["::/0"]]);
        assert_eq!(at(&r, "192.0.2.1"), None);
    }

    #[test]
    fn a_prefix_two_peers_list_goes_to_the_later_one() {
        let r = routes(&[&["10.0.0.0/24"], &["10.0.0.9/24"]]);
        assert_eq!(at(&r, "10.0.0.1"), Some(1));
    }
}
```

新建 `crates/rurge-proto-wireguard/src/wire.rs`：

```rust
//! What rurge reads or writes in a WireGuard message itself: the message
//! type (byte 0) and the three reserved bytes after it, which `client-id`
//! fills (manual: WARP routes by them).

/// A handshake initiation (the WireGuard paper, §5.4.2).
pub const HANDSHAKE_INITIATION: u8 = 1;
/// A handshake response (§5.4.3).
pub const HANDSHAKE_RESPONSE: u8 = 2;
/// The TOS byte a handshake initiation is sent with: DSCP AF41, as the
/// manual and WireGuard itself mark it.
pub const HANDSHAKE_TOS: u8 = 0x88;

/// The type of `message`.
pub fn message_type(message: &[u8]) -> Option<u8> {
    message.first().copied()
}

/// Writes `client_id` into the reserved bytes of an outgoing message.
pub fn mark(message: &mut [u8], client_id: Option<[u8; 3]>) {
    if let (Some(id), Some(reserved)) = (client_id, message.get_mut(1..4)) {
        reserved.copy_from_slice(&id);
    }
}

/// Clears the reserved bytes of an incoming message before boringtun reads
/// it (it takes them for part of the type): a server that routes by
/// `client-id` sends them back filled in. Standard peers send zeros.
pub fn unmark(message: &mut [u8]) {
    if let Some(reserved) = message.get_mut(1..4) {
        reserved.fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reserved_bytes_carry_the_client_id_out_and_are_cleared_in() {
        let mut message = [HANDSHAKE_INITIATION, 0, 0, 0, 0xaa, 0xbb];
        mark(&mut message, Some([83, 12, 235]));
        assert_eq!(message, [1, 83, 12, 235, 0xaa, 0xbb]);
        unmark(&mut message);
        assert_eq!(message, [1, 0, 0, 0, 0xaa, 0xbb]);
        assert_eq!(message_type(&message), Some(HANDSHAKE_INITIATION));
        // without an id nothing is written; what is too short is left alone
        mark(&mut message, None);
        assert_eq!(message, [1, 0, 0, 0, 0xaa, 0xbb]);
        let mut short = [4u8, 1];
        mark(&mut short, Some([9, 9, 9]));
        unmark(&mut short);
        assert_eq!(short, [4, 1]);
        assert_eq!(message_type(&[]), None);
    }
}
```

新建 `crates/rurge-proto-wireguard/src/stack.rs`：

```rust
//! The tunnel's state, all of it behind one lock (phase 2 M4 design 6.1):
//! a smoltcp interface and its sockets, the packet queues between it and
//! the peers, and a boringtun `Tunn` for each peer. Nothing here does I/O or
//! waits: the caller hands in what the carriers received and sends what
//! comes out.

use crate::routes::Routes;
use crate::wire;
use boringtun::noise::errors::WireGuardError;
use boringtun::noise::{Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};
use rurge_config::wireguard::WireGuardSection;
use smoltcp::iface::{Config, Interface, PollResult, SocketHandle, SocketSet};
use smoltcp::phy::{self, DeviceCapabilities, Medium};
use smoltcp::socket::{AnySocket, tcp};
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr};
use std::collections::VecDeque;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::ops::RangeInclusive;
use std::time::{Duration, Instant};

/// The buffers of every TCP connection, each way.
const TCP_BUFFER: usize = 256 * 1024;
/// How long a connection whose stream is gone may take to close before it
/// is reset.
const LINGER: Duration = Duration::from_secs(30);
/// The local ports of the tunnel's connections: the dynamic range.
const PORTS: RangeInclusive<u16> = 49152..=65535;
/// Room for the largest UDP datagram and what boringtun adds to a message.
const SCRATCH: usize = 65536 + 32;

/// A message for the carrier of `peer`.
#[derive(Debug)]
pub struct Outgoing {
    pub peer: usize,
    pub datagram: Vec<u8>,
}

/// Why a connection is not opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The tunnel has no address of the destination's family.
    NoAddress(IpAddr),
    /// No peer's `allowed-ips` covers the destination.
    NoRoute(IpAddr),
    /// No connection can go there (`0.0.0.0`).
    Unaddressable(IpAddr),
    /// Every local port is in use.
    NoPort,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::NoAddress(IpAddr::V4(_)) => {
                f.write_str("wireguard: the tunnel has no IPv4 address")
            }
            Refusal::NoAddress(IpAddr::V6(_)) => {
                f.write_str("wireguard: the tunnel has no IPv6 address")
            }
            Refusal::NoRoute(ip) => write!(f, "wireguard: no peer's allowed-ips covers {ip}"),
            Refusal::Unaddressable(ip) => write!(f, "wireguard: {ip} cannot be connected to"),
            Refusal::NoPort => f.write_str("wireguard: every local port of the tunnel is in use"),
        }
    }
}

struct Peer {
    tunnel: Tunn,
    client_id: Option<[u8; 3]>,
    /// When a handshake rurge started with it last completed.
    handshake: Option<Instant>,
}

pub struct Stack {
    iface: Interface,
    sockets: SocketSet<'static>,
    queues: Queues,
    peers: Vec<Peer>,
    routes: Routes,
    v4: Option<Ipv4Addr>,
    v6: Option<Ipv6Addr>,
    scratch: Vec<u8>,
    next_port: u16,
    /// Connections whose stream is gone, and since when.
    released: Vec<(SocketHandle, Instant)>,
    /// What smoltcp's clock counts from.
    epoch: Instant,
}

/// A random number; a fixed one if the system has none to give (it only
/// spreads the handshake indices and the TCP sequence numbers).
fn random_u64() -> u64 {
    getrandom::u64().unwrap_or(0x5eed)
}

/// `message` with `client_id` in it, for the carrier of `peer`.
fn outgoing(peer: usize, message: &mut [u8], client_id: Option<[u8; 3]>) -> Outgoing {
    wire::mark(message, client_id);
    Outgoing {
        peer,
        datagram: message.to_vec(),
    }
}

impl Stack {
    pub fn new(section: &WireGuardSection) -> Stack {
        let mut queues = Queues::new(usize::from(section.mtu));
        let mut config = Config::new(HardwareAddress::Ip);
        config.random_seed = random_u64();
        let mut iface = Interface::new(config, &mut queues, smoltcp::time::Instant::ZERO);
        iface.update_ip_addrs(|addrs| {
            if let Some(v4) = section.self_ip {
                let _ = addrs.push(IpCidr::new(IpAddress::Ipv4(v4), 32));
            }
            if let Some(v6) = section.self_ip_v6 {
                let _ = addrs.push(IpCidr::new(IpAddress::Ipv6(v6), 128));
            }
        });
        // everything leaves through some peer: `advance` picks which one
        if let Some(v4) = section.self_ip {
            let _ = iface.routes_mut().add_default_ipv4_route(v4);
        }
        if let Some(v6) = section.self_ip_v6 {
            let _ = iface.routes_mut().add_default_ipv6_route(v6);
        }
        let private = StaticSecret::from(*section.private_key.expose());
        // boringtun uses 24 bits of it; apart per peer
        let base = random_u64() as u32;
        let peers = section
            .peers
            .iter()
            .enumerate()
            .map(|(i, p)| Peer {
                tunnel: Tunn::new(
                    private.clone(),
                    PublicKey::from(p.public_key),
                    p.preshared_key.as_ref().map(|k| *k.expose()),
                    p.keepalive,
                    base.wrapping_add(i as u32) & 0x00ff_ffff,
                    None,
                ),
                client_id: p.client_id,
                handshake: None,
            })
            .collect();
        let span = PORTS.end() - PORTS.start();
        Stack {
            iface,
            sockets: SocketSet::new(Vec::new()),
            queues,
            peers,
            routes: Routes::new(section.peers.iter().map(|p| p.allowed_ips.as_slice())),
            v4: section.self_ip,
            v6: section.self_ip_v6,
            scratch: vec![0; SCRATCH],
            next_port: PORTS.start() + (random_u64() % u64::from(span)) as u16,
            released: Vec::new(),
            epoch: Instant::now(),
        }
    }

    fn at(&self, now: Instant) -> smoltcp::time::Instant {
        smoltcp::time::Instant::from_micros(
            now.saturating_duration_since(self.epoch).as_micros() as i64
        )
    }

    /// A handshake with every peer, whatever the state of its session: the
    /// tunnel starts, a test asks, the network changed.
    pub fn initiate(&mut self, out: &mut Vec<Outgoing>) {
        for (i, peer) in self.peers.iter_mut().enumerate() {
            if let TunnResult::WriteToNetwork(message) = peer
                .tunnel
                .format_handshake_initiation(&mut self.scratch, true)
            {
                out.push(outgoing(i, message, peer.client_id));
            }
        }
    }

    /// What the carrier of `peer` received; the answers it needs go to `out`.
    pub fn receive(
        &mut self,
        peer: usize,
        datagram: &mut [u8],
        now: Instant,
        out: &mut Vec<Outgoing>,
    ) {
        let Some(p) = self.peers.get_mut(peer) else {
            return;
        };
        wire::unmark(datagram);
        let response = wire::message_type(datagram) == Some(wire::HANDSHAKE_RESPONSE);
        let (src, packet) = match p.tunnel.decapsulate(None, datagram, &mut self.scratch) {
            TunnResult::WriteToNetwork(message) => {
                if response {
                    p.handshake = Some(now);
                }
                out.push(outgoing(peer, message, p.client_id));
                // what waited for the handshake
                while let TunnResult::WriteToNetwork(message) =
                    p.tunnel.decapsulate(None, &[], &mut self.scratch)
                {
                    out.push(outgoing(peer, message, p.client_id));
                }
                return;
            }
            TunnResult::WriteToTunnelV4(packet, src) => (IpAddr::V4(src), packet.to_vec()),
            TunnResult::WriteToTunnelV6(packet, src) => (IpAddr::V6(src), packet.to_vec()),
            TunnResult::Done => return,
            TunnResult::Err(e) => {
                tracing::trace!(peer, error = ?e, "wireguard: a message was dropped");
                return;
            }
        };
        // a peer may send only from what routes to it (cryptokey routing)
        if self.routes.lookup(src) == Some(peer) {
            self.queues.rx.push_back(packet);
        } else {
            tracing::trace!(peer, %src, "wireguard: a packet from outside the peer's allowed-ips was dropped");
        }
    }

    /// Runs the IP stack at `now`: what the peers sent reaches the sockets,
    /// what the sockets have to send goes out. How long until it wants to
    /// run again, when it has a deadline.
    pub fn advance(&mut self, now: Instant, out: &mut Vec<Outgoing>) -> Option<Duration> {
        let at = self.at(now);
        // a poll sends at most one segment of each connection: again, until
        // none has anything more to send now
        while self.iface.poll(at, &mut self.queues, &mut self.sockets)
            == PollResult::SocketStateChanged
        {}
        while let Some(packet) = self.queues.tx.pop_front() {
            self.send(&packet, out);
        }
        self.reap(now);
        self.iface.poll_delay(at, &self.sockets).map(Duration::from)
    }

    /// One packet of the stack to the peer its destination routes to.
    fn send(&mut self, packet: &[u8], out: &mut Vec<Outgoing>) {
        // `connect` refuses what no peer covers: what is left is a reply to
        // a packet that came from elsewhere
        let Some(peer) = Tunn::dst_address(packet).and_then(|dst| self.routes.lookup(dst)) else {
            return;
        };
        let p = &mut self.peers[peer];
        match p.tunnel.encapsulate(packet, &mut self.scratch) {
            TunnResult::WriteToNetwork(message) => out.push(outgoing(peer, message, p.client_id)),
            TunnResult::Err(e) => {
                tracing::trace!(peer, error = ?e, "wireguard: a packet was dropped");
            }
            // held until the handshake is done
            _ => {}
        }
    }

    /// WireGuard's timers (retries, keepalives, expiry); every quarter of a
    /// second or so.
    pub fn tick(&mut self, out: &mut Vec<Outgoing>) {
        for (i, p) in self.peers.iter_mut().enumerate() {
            match p.tunnel.update_timers(&mut self.scratch) {
                TunnResult::WriteToNetwork(message) => out.push(outgoing(i, message, p.client_id)),
                // an idle session ran out: the next packet starts another
                TunnResult::Err(WireGuardError::ConnectionExpired) => {}
                TunnResult::Err(e) => {
                    tracing::trace!(peer = i, error = ?e, "wireguard: a timer failed");
                }
                _ => {}
            }
        }
    }

    /// A TCP connection to `to`, from the tunnel's address of its family.
    pub fn connect(&mut self, to: SocketAddr) -> Result<SocketHandle, Refusal> {
        let local = match to.ip() {
            IpAddr::V4(_) => self.v4.map(IpAddr::V4),
            IpAddr::V6(_) => self.v6.map(IpAddr::V6),
        }
        .ok_or(Refusal::NoAddress(to.ip()))?;
        if self.routes.lookup(to.ip()).is_none() {
            return Err(Refusal::NoRoute(to.ip()));
        }
        let port = self.free_port().ok_or(Refusal::NoPort)?;
        let mut socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0; TCP_BUFFER]),
            tcp::SocketBuffer::new(vec![0; TCP_BUFFER]),
        );
        // what the relay writes goes out as it is written, as on a real socket
        socket.set_nagle_enabled(false);
        // smoltcp 0.12's Cubic counts its window in bytes where RFC 8312
        // counts segments: it keeps the window at a segment or two
        socket.set_congestion_control(tcp::CongestionControl::Reno);
        socket
            .connect(self.iface.context(), to, SocketAddr::new(local, port))
            .map_err(|_| Refusal::Unaddressable(to.ip()))?;
        Ok(self.sockets.add(socket))
    }

    fn free_port(&mut self) -> Option<u16> {
        for _ in PORTS {
            let port = self.next_port;
            self.next_port = if port == *PORTS.end() {
                *PORTS.start()
            } else {
                port + 1
            };
            let taken = self.sockets.iter().any(|(_, s)| {
                tcp::Socket::downcast(s)
                    .and_then(tcp::Socket::local_endpoint)
                    .is_some_and(|e| e.port == port)
            });
            if !taken {
                return Some(port);
            }
        }
        None
    }

    /// The TCP connection of `handle`.
    pub fn tcp(&mut self, handle: SocketHandle) -> &mut tcp::Socket<'static> {
        self.sockets.get_mut(handle)
    }

    /// The stream of `handle` is gone. A connection the other side is done
    /// with is closed, anything else reset; `advance` forgets it once it is
    /// over.
    pub fn release(&mut self, handle: SocketHandle, now: Instant) {
        let socket = self.sockets.get_mut::<tcp::Socket>(handle);
        if socket.may_recv() || socket.recv_queue() > 0 {
            socket.abort();
        } else {
            socket.close();
        }
        self.released.push((handle, now));
    }

    fn reap(&mut self, now: Instant) {
        let sockets = &mut self.sockets;
        self.released.retain(|&(handle, since)| {
            let socket = sockets.get_mut::<tcp::Socket>(handle);
            let over = match socket.state() {
                tcp::State::TimeWait => true,
                // with no endpoint left, the reset went out
                tcp::State::Closed => socket.local_endpoint().is_none(),
                _ => false,
            };
            if over {
                sockets.remove(handle);
                return false;
            }
            if now.saturating_duration_since(since) >= LINGER {
                socket.abort();
            }
            true
        });
    }
}

/// The stack's device: packets from the peers on their way in, packets of
/// the stack on their way out.
pub(crate) struct Queues {
    pub(crate) rx: VecDeque<Vec<u8>>,
    pub(crate) tx: VecDeque<Vec<u8>>,
    mtu: usize,
}

impl Queues {
    pub(crate) fn new(mtu: usize) -> Queues {
        Queues {
            rx: VecDeque::new(),
            tx: VecDeque::new(),
            mtu,
        }
    }
}

pub(crate) struct RxToken(Vec<u8>);

pub(crate) struct TxToken<'a>(&'a mut VecDeque<Vec<u8>>);

impl phy::Device for Queues {
    type RxToken<'a>
        = RxToken
    where
        Self: 'a;
    type TxToken<'a>
        = TxToken<'a>
    where
        Self: 'a;

    fn receive(
        &mut self,
        _timestamp: smoltcp::time::Instant,
    ) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let packet = self.rx.pop_front()?;
        Some((RxToken(packet), TxToken(&mut self.tx)))
    }

    fn transmit(&mut self, _timestamp: smoltcp::time::Instant) -> Option<Self::TxToken<'_>> {
        Some(TxToken(&mut self.tx))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = self.mtu;
        caps
    }
}

impl phy::RxToken for RxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.0)
    }
}

impl phy::TxToken for TxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut packet = vec![0; len];
        let done = f(&mut packet);
        self.0.push_back(packet);
        done
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{PeerCore, PeerOpts, keypair, section};
    use std::net::Ipv4Addr;

    /// A client stack and its peers, every message delivered at once.
    struct Net {
        client: Stack,
        peers: Vec<PeerCore>,
        /// How many times the last `run` went round.
        rounds: usize,
    }

    impl Net {
        /// `allowed` is each peer's `allowed-ips`.
        fn new(peers: &[(PeerOpts, &[&str])], client_ids: &[Option<[u8; 3]>]) -> Net {
            let (private, public) = keypair();
            let cores: Vec<PeerCore> = peers
                .iter()
                .map(|(opts, _)| PeerCore::new(public, opts))
                .collect();
            let keys: Vec<([u8; 32], &[&str])> = cores
                .iter()
                .zip(peers)
                .map(|(core, (_, allowed))| (core.public_key(), *allowed))
                .collect();
            let mut section = section(private, Ipv4Addr::new(10, 9, 0, 2), &keys);
            for (peer, id) in section.peers.iter_mut().zip(client_ids) {
                peer.client_id = *id;
            }
            let mut net = Net {
                client: Stack::new(&section),
                peers: cores,
                rounds: 0,
            };
            let mut out = Vec::new();
            net.client.initiate(&mut out);
            net.run_with(out);
            net
        }

        fn run(&mut self) {
            self.run_with(Vec::new());
        }

        /// Until neither side has anything more to send.
        fn run_with(&mut self, mut to_peers: Vec<Outgoing>) {
            for round in 1..=500 {
                self.rounds = round;
                let now = Instant::now();
                self.client.advance(now, &mut to_peers);
                let mut to_client: Vec<(usize, Vec<u8>)> = Vec::new();
                for Outgoing { peer, mut datagram } in to_peers.drain(..) {
                    let mut back = Vec::new();
                    self.peers[peer].receive(&mut datagram, &mut back);
                    to_client.extend(back.into_iter().map(|d| (peer, d)));
                }
                for (i, core) in self.peers.iter_mut().enumerate() {
                    let mut back = Vec::new();
                    core.advance(&mut back);
                    to_client.extend(back.into_iter().map(|d| (i, d)));
                }
                if to_client.is_empty() {
                    return;
                }
                for (peer, mut datagram) in to_client {
                    self.client.receive(peer, &mut datagram, now, &mut to_peers);
                }
            }
            panic!("the tunnel never went quiet");
        }

        fn connect(&mut self, to: &str) -> SocketHandle {
            let handle = self.client.connect(to.parse().unwrap()).unwrap();
            self.run();
            assert_eq!(self.client.tcp(handle).state(), tcp::State::Established);
            handle
        }

        fn echo(&mut self, handle: SocketHandle, data: &[u8]) -> Vec<u8> {
            self.client.tcp(handle).send_slice(data).unwrap();
            self.run();
            let mut buf = vec![0u8; data.len() + 16];
            let n = self.client.tcp(handle).recv_slice(&mut buf).unwrap();
            buf.truncate(n);
            buf
        }
    }

    fn one_peer(opts: PeerOpts) -> Net {
        Net::new(&[(opts, &["10.0.0.0/8"])], &[None])
    }

    #[test]
    fn a_connection_through_the_tunnel_echoes() {
        let mut net = one_peer(PeerOpts::default());
        assert_eq!(net.peers[0].handshakes, 1);
        let handle = net.connect("10.0.0.1:7");
        assert_eq!(net.echo(handle, b"ping"), b"ping");
        assert!(net.client.peers[0].handshake.is_some());
    }

    /// A poll sends one segment of each connection: an advance sends all a
    /// connection may, and the window grows past a segment or two (Reno:
    /// smoltcp 0.12's Cubic keeps it there).
    #[test]
    fn a_window_of_data_leaves_at_once() {
        let mut net = one_peer(PeerOpts::default());
        let handle = net.connect("10.0.0.1:7");
        assert_eq!(
            net.client.tcp(handle).congestion_control(),
            tcp::CongestionControl::Reno
        );
        let data: Vec<u8> = (0..200 * 1024).map(|i| (i % 251) as u8).collect();
        assert!(net.echo(handle, &data) == data, "all of it came back");
        assert!(net.rounds < 20, "{} rounds", net.rounds);
    }

    /// Every message out carries the id; the peer's own, which it writes
    /// into its answers, is cleared before boringtun reads them (manual).
    #[test]
    fn the_client_id_goes_out_in_every_message_and_is_cleared_coming_in() {
        let id = [83, 12, 235];
        let mut net = Net::new(
            &[(
                PeerOpts {
                    client_id: Some(id),
                    ..PeerOpts::default()
                },
                &["10.0.0.0/8"],
            )],
            &[Some(id)],
        );
        let handle = net.connect("10.0.0.1:7");
        assert_eq!(net.echo(handle, b"ping"), b"ping");
        let reserved = &net.peers[0].reserved;
        assert!(reserved.len() >= 3, "{reserved:?}");
        assert!(reserved.iter().all(|r| *r == id), "{reserved:?}");
    }

    /// A peer that routes by the id ignores what comes without it.
    #[test]
    fn without_the_client_id_such_a_peer_never_answers() {
        let mut net = Net::new(
            &[(
                PeerOpts {
                    client_id: Some([83, 12, 235]),
                    ..PeerOpts::default()
                },
                &["10.0.0.0/8"],
            )],
            &[None],
        );
        assert_eq!(net.peers[0].handshakes, 0);
        let handle = net.client.connect("10.0.0.1:7".parse().unwrap()).unwrap();
        net.run();
        assert_eq!(net.client.tcp(handle).state(), tcp::State::SynSent);
    }

    #[test]
    fn the_longest_prefix_picks_the_peer() {
        let mut net = Net::new(
            &[
                (PeerOpts::default(), &["10.0.0.0/8"]),
                (PeerOpts::default(), &["10.2.0.0/16"]),
            ],
            &[None, None],
        );
        let wide = net.connect("10.1.0.1:7");
        let narrow = net.connect("10.2.0.1:7");
        assert_eq!(net.echo(wide, b"a"), b"a");
        assert_eq!(net.echo(narrow, b"b"), b"b");
        assert_eq!((net.peers[0].accepted, net.peers[1].accepted), (1, 1));
    }

    #[test]
    fn what_the_tunnel_cannot_reach_is_refused() {
        let mut net = one_peer(PeerOpts::default());
        let refused = |net: &mut Net, to: &str| {
            net.client
                .connect(to.parse().unwrap())
                .unwrap_err()
                .to_string()
        };
        assert_eq!(
            refused(&mut net, "192.0.2.1:80"),
            "wireguard: no peer's allowed-ips covers 192.0.2.1"
        );
        assert_eq!(
            refused(&mut net, "[2001:db8::1]:80"),
            "wireguard: the tunnel has no IPv6 address"
        );
        let mut all = Net::new(&[(PeerOpts::default(), &["0.0.0.0/0"])], &[None]);
        assert_eq!(
            refused(&mut all, "0.0.0.0:80"),
            "wireguard: 0.0.0.0 cannot be connected to"
        );
    }

    /// Cryptokey routing: what a peer sends from outside its own
    /// `allowed-ips` is dropped.
    #[test]
    fn a_peer_may_only_send_from_its_allowed_ips() {
        let mut net = Net::new(
            &[
                (PeerOpts::default(), &["10.1.0.0/16"]),
                (PeerOpts::default(), &["10.2.0.0/16"]),
            ],
            &[None, None],
        );
        let client = Ipv4Addr::new(10, 9, 0, 2);
        let mut out = Vec::new();
        net.peers[1].ping(Ipv4Addr::new(10, 1, 0, 5), client, 1, 32, None, &mut out);
        net.deliver(1, out);
        // had it been taken, the answer would have gone to the other peer
        assert!(net.peers[0].pongs.is_empty(), "{:?}", net.peers[0].pongs);
        assert!(net.peers[1].pongs.is_empty(), "{:?}", net.peers[1].pongs);
        let mut out = Vec::new();
        net.peers[1].ping(Ipv4Addr::new(10, 2, 0, 5), client, 2, 32, None, &mut out);
        net.deliver(1, out);
        assert_eq!(net.peers[1].pongs, [(2, 32)]);
    }

    /// The stack answers a ping to its own address, and nothing else.
    #[test]
    fn only_echo_requests_to_the_tunnel_address_are_answered() {
        let mut net = one_peer(PeerOpts::default());
        let mut out = Vec::new();
        let from = Ipv4Addr::new(10, 0, 0, 1);
        net.peers[0].ping(from, Ipv4Addr::new(10, 9, 0, 3), 1, 8, None, &mut out);
        net.peers[0].ping(from, Ipv4Addr::new(10, 9, 0, 2), 2, 8, None, &mut out);
        net.deliver(0, out);
        assert_eq!(net.peers[0].pongs, [(2, 8)]);
    }

    #[test]
    fn a_fragmented_packet_is_reassembled() {
        let mut net = one_peer(PeerOpts::default());
        let mut out = Vec::new();
        let (from, to) = (Ipv4Addr::new(10, 0, 0, 1), Ipv4Addr::new(10, 9, 0, 2));
        net.peers[0].ping(from, to, 7, 1000, Some(496), &mut out);
        assert_eq!(out.len(), 3, "the request went in three fragments");
        net.deliver(0, out);
        assert_eq!(net.peers[0].pongs, [(7, 1000)]);
    }

    /// The other side done too: closed and forgotten. Not done: reset.
    #[test]
    fn a_released_connection_is_closed_or_reset_and_forgotten() {
        let mut net = one_peer(PeerOpts::default());
        let done = net.connect("10.0.0.1:7");
        net.client.tcp(done).close();
        net.run();
        // the echo server closes after us; our side has read everything
        let mut buf = [0u8; 8];
        assert_eq!(
            net.client.tcp(done).recv_slice(&mut buf),
            Err(tcp::RecvError::Finished)
        );
        net.client.release(done, Instant::now());
        net.run();
        assert_eq!(net.client.sockets.iter().count(), 0);
        assert_eq!(net.peers[0].open(), 0);

        let busy = net.connect("10.0.0.1:7");
        assert_eq!(net.peers[0].open(), 1);
        net.client.release(busy, Instant::now());
        net.run();
        assert_eq!(net.client.sockets.iter().count(), 0);
        assert_eq!(net.peers[0].open(), 0, "the reset closed the peer's side");
        assert_eq!(net.peers[0].resets, 1);
    }

    impl Net {
        /// Messages `peer` sent on its own.
        fn deliver(&mut self, peer: usize, messages: Vec<Vec<u8>>) {
            let mut out = Vec::new();
            for mut message in messages {
                self.client
                    .receive(peer, &mut message, Instant::now(), &mut out);
            }
            self.run_with(out);
        }
    }
}
```

要点：
- `Stack` 里的一切都是内存操作，没有 I/O、不等待（M4-D5）：`receive` 清掉 `client-id` 字节 → `decapsulate`（要回写网络的照办、再以空输入调到 `Done`）→ 解出的 IP 包先核对内层源地址落在这个 peer 的 `allowed-ips` 里（密钥路由），不在就丢 → 进收队列；`advance` 推进 smoltcp，把它吐出的包按目的地址查 `Routes` 选 peer、`encapsulate`、写上 `client-id` 交给调用方。
- **`advance` 反复 `poll` 到 `PollResult::None`**：smoltcp 0.12 的一次 `poll` 每条连接最多发一个报文段（P2）。`a_window_of_data_leaves_at_once` 在内存里回显 200 KiB：一次 `poll` 时要 167 轮，现在 4 轮；它同时断言连接用的是 Reno（P3）。
- `connect`：目标族没有本端地址 → `NoAddress`；没有 peer 覆盖 → `NoRoute`（绝不直连兜底）；`0.0.0.0` 之类 → `Unaddressable`；本端端口用尽 → `NoPort`。连接关 Nagle（转发写多少就发多少，与真实套接字一样）、显式设 Reno。
- `release`：流被丢弃时，对端已经结束且没有没读的数据就关闭，否则重置；`advance` 在连接结束（TimeWait，或 Closed 且重置已发出）后遗忘它，30 秒还没结束就重置。
- `PeerCore` 是用例的对端（P20）：它自己的 smoltcp 主机开了 `any_ip`，隧道里发给它的任何地址都答；7 号端口回显，同时保持 8 个监听套接字。`without_the_client_id_such_a_peer_never_answers`：对端要求 `client-id` 时，客户端不写就握手不成。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto-wireguard` → 14 passed（`routes` 3 条、`wire` 1 条、`stack` 10 条：回显、一轮发完一整窗、`client-id` 的写与清、不写 `client-id` 握手不成、最长前缀选 peer、够不着的目标被拒、peer 只能从自己的 `allowed-ips` 发来、只回应发给本端的 ping、分片重组、流被丢弃之后的关闭与遗忘）。
Run: `cargo tree -p rurge-proto-wireguard -e features -i smoltcp` → smoltcp 只开了 P2 列的特性（没有 `socket-tcp-cubic`、`log`、`phy-*`）。

- [ ] **Step 5: 门禁与提交**

跑门禁（47 个测试二进制，1079 通过 / 1 忽略）。

```bash
git add Cargo.toml Cargo.lock crates/rurge-proto-wireguard
git commit -m "feat(proto-wireguard): 新 crate rurge-proto-wireguard——路由表、client-id 与协议栈（boringtun 0.7.1 + smoltcp 0.12.0，Reno，每次推进 poll 到发完）"
```

### Task 4: 隧道——设备任务、流、`WireGuardOutbound`、`FakeWgPeer`；在本机解析目标域名

把 Task 3 的协议栈跑起来（设计 6.1、6.2、6.4）。`Device`：第一次拨号时启动，给每个 peer 经连接器建一条 UDP 载体（全部连不上才算失败，连不上的 peer 留空）→ 协议栈进锁 → 设备任务独占各载体：收到的批量交给协议栈（P9）、发在锁外、握手发起包标 TOS 0x88、每 250 ms 推进 boringtun 的定时器、按 `poll_delay` 推进 smoltcp、被流的读写唤醒。`TunnelStream`：每条 TCP 连接一个 smoltcp 套接字，读写直接在锁里操作它的缓冲，数据或空间不够就挂在套接字的唤醒器上。`WireGuardOutbound`：隧道按需启动、同时进来的拨号共用这一次启动；目标是 IP 就直接用，是域名就用 rurge 的解析器在本机解析（含 `[Host]`，M4-D9；`dns-server` 在 Task 5）；挑地址见 P14。重拨、网络变化、握手日志与"一个私钥一条隧道"在 Task 6。

`FakeWgPeer` 把 `PeerCore` 放在回环 UDP 端口上（P20）。

**Files:**
- Modify: `crates/rurge-proto-wireguard/Cargo.toml`（`rurge-net`、`rurge-proto`、`tokio`、`tokio-util`；`testing` 特性带上 `socket2`）、`src/lib.rs`（`device`、`outbound`、`stream` 三个模块与导出）
- Create: `crates/rurge-proto-wireguard/src/device.rs`（`Device`、载体、设备任务）
- Create: `crates/rurge-proto-wireguard/src/stream.rs`（`TunnelStream`）
- Create: `crates/rurge-proto-wireguard/src/outbound.rs`（`WireGuardOutbound`，与用例）
- Modify: `crates/rurge-proto-wireguard/src/testing/mod.rs`（`endpoint`、`PeerCore.connected_to`、`mod peer`）
- Create: `crates/rurge-proto-wireguard/src/testing/peer.rs`（`FakeWgPeer`）

**Interfaces:**
- Consumes: Task 1 的 `WireGuardSpec`、`WireGuardSection`；Task 2 的 `Datagram`、`BoxedDatagram`、`Connector::connect_udp`；Task 3 的 `Stack`、`Outgoing`、`Refusal`、`wire`、`testing::{PeerCore, PeerOpts, keypair, section}`；`rurge_net::connector::{Connector, ConnectOpts, Target, Resolve, BoxedStream}`、`rurge_proto::{Outbound, OutboundError}`。
- Produces:
  - `rurge_proto_wireguard::WireGuardOutbound`：`WireGuardOutbound::new(name: &str, spec: &WireGuardSpec, resolver: Arc<dyn Resolve>, connector: Arc<dyn Connector>) -> WireGuardOutbound`（构建时不开套接字、不解析名字）；`impl Outbound for WireGuardOutbound`（`connect_tcp` 整个受 `ConnectOpts.timeout` 约束，超时是 `OutboundError::Timeout`）
  - crate 内：`Device::start(policy: &str, section: &WireGuardSection, connector: &Arc<dyn Connector>, opts: &ConnectOpts) -> Result<Arc<Device>, OutboundError>`、`Device::connect(self: &Arc<Device>, to: SocketAddr) -> Result<TunnelStream, OutboundError>`、`Shared { stack: Mutex<Stack>, kick }`、`TunnelStream::new(Arc<Device>, SocketHandle)`（Task 5、6、8 再扩展）
  - `testing::endpoint(addr: SocketAddr) -> PeerEndpoint`；`testing::FakeWgPeer`：`FakeWgPeer::start(client_public: [u8; 32], opts: PeerOpts).await`、`addr() -> SocketAddr`、`public_key() -> [u8; 32]`、`core() -> MutexGuard<'_, PeerCore>`、`go_silent(bool)`；`PeerCore.connected_to: Vec<SocketAddr>`

- [ ] **Step 1: 先写对端，并声明新模块**

`crates/rurge-proto-wireguard/Cargo.toml`——把

```toml
rurge-config.workspace = true
```

换成

```toml
rurge-config.workspace = true
rurge-net.workspace = true
rurge-proto.workspace = true
```

`crates/rurge-proto-wireguard/Cargo.toml`——把

```toml
tracing.workspace = true
```

换成

```toml
tokio.workspace = true
tokio-util.workspace = true
tracing.workspace = true
socket2 = { workspace = true, optional = true }

[dev-dependencies]
socket2.workspace = true
```

`crates/rurge-proto-wireguard/Cargo.toml`——把

```toml
testing = []
```

换成

```toml
testing = ["dep:socket2"]
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
//! answers on every address routed to it — a TCP echo service on port 7.
```

换成

```rust
//! answers on every address routed to it — a TCP echo service on port 7.
//! `FakeWgPeer` puts one on a loopback UDP port.

mod peer;

pub use peer::FakeWgPeer;
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
```

换成

```rust
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
            .collect(),
```

换成

```rust
            .collect(),
    }
}

/// `addr` as a peer's `endpoint`.
pub fn endpoint(addr: SocketAddr) -> PeerEndpoint {
    PeerEndpoint {
        host: HostName::Ip(addr.ip()),
        port: addr.port(),
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
    pub accepted: usize,
```

换成

```rust
    pub accepted: usize,
    /// Where each accepted connection went: the address and port the
    /// client connected to.
    pub connected_to: Vec<SocketAddr>,
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
            accepted: 0,
```

换成

```rust
            accepted: 0,
            connected_to: Vec::new(),
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
            if self.sockets.get::<tcp::Socket>(handle).state() == tcp::State::Listen {
                continue;
```

换成

```rust
            let listener = self.sockets.get::<tcp::Socket>(handle);
            if listener.state() == tcp::State::Listen {
                continue;
            }
            if let Some(local) = listener.local_endpoint() {
                self.connected_to
                    .push(SocketAddr::new(local.addr.into(), local.port));
```

新建 `crates/rurge-proto-wireguard/src/testing/peer.rs`：

```rust
//! `FakeWgPeer`: a `PeerCore` on a loopback UDP port.

use super::{PeerCore, PeerOpts};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

/// How often the peer runs WireGuard's timers.
const TICK: Duration = Duration::from_millis(50);
/// The socket's buffers each way: a burst of a whole TCP window arrives at
/// once on loopback.
const SOCKET_BUFFER: usize = 8 << 20;

pub struct FakeWgPeer {
    addr: SocketAddr,
    public_key: [u8; 32],
    core: Arc<Mutex<PeerCore>>,
    silent: Arc<AtomicBool>,
    task: JoinHandle<()>,
}

impl FakeWgPeer {
    /// A peer for the client of `client_public`, on 127.0.0.1.
    pub async fn start(client_public: [u8; 32], opts: PeerOpts) -> FakeWgPeer {
        let socket = UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let buffers = socket2::SockRef::from(&socket);
        let _ = buffers.set_recv_buffer_size(SOCKET_BUFFER);
        let _ = buffers.set_send_buffer_size(SOCKET_BUFFER);
        let addr = socket.local_addr().expect("its address");
        let core = PeerCore::new(client_public, &opts);
        let public_key = core.public_key();
        let core = Arc::new(Mutex::new(core));
        let silent = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn(serve(socket, core.clone(), silent.clone()));
        FakeWgPeer {
            addr,
            public_key,
            core,
            silent,
            task,
        }
    }

    /// Its UDP address: a client's `endpoint`.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.public_key
    }

    /// What it has seen so far.
    pub fn core(&self) -> MutexGuard<'_, PeerCore> {
        self.core.lock().expect("the peer")
    }

    /// From now on it drops whatever arrives and sends nothing, as a peer
    /// that is down.
    pub fn go_silent(&self, silent: bool) {
        self.silent.store(silent, Ordering::SeqCst);
    }
}

impl Drop for FakeWgPeer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(socket: UdpSocket, core: Arc<Mutex<PeerCore>>, silent: Arc<AtomicBool>) {
    let mut buf = vec![0u8; 65536];
    let mut client = None;
    let mut timer = tokio::time::interval(TICK);
    // when its stack wants to run again (a retransmission, a delayed ACK)
    let mut due: Option<tokio::time::Instant> = None;
    loop {
        let mut out = Vec::new();
        let stack_due = async {
            match due {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        let ran = tokio::select! {
            received = socket.recv_from(&mut buf) => {
                // an ICMP error the system reports on the socket
                let Ok((n, from)) = received else { continue };
                if silent.load(Ordering::SeqCst) {
                    continue;
                }
                let mut core = core.lock().expect("the peer");
                let mut arrived = Some((n, from));
                while let Some((n, from)) = arrived {
                    client = Some(from);
                    core.receive(&mut buf[..n], &mut out);
                    // what else has arrived goes in before the stack runs
                    arrived = socket.try_recv_from(&mut buf).ok();
                }
                core.advance(&mut out)
            }
            _ = timer.tick() => {
                if silent.load(Ordering::SeqCst) {
                    continue;
                }
                let mut core = core.lock().expect("the peer");
                core.tick(&mut out);
                core.advance(&mut out)
            }
            _ = stack_due => {
                if silent.load(Ordering::SeqCst) {
                    continue;
                }
                core.lock().expect("the peer").advance(&mut out)
            }
        };
        due = ran.map(|wait| tokio::time::Instant::now() + wait);
        if let Some(to) = client {
            for message in out {
                let _ = socket.send_to(&message, to).await;
            }
        }
    }
}
```

`crates/rurge-proto-wireguard/src/lib.rs`——把

```rust
pub mod routes;
pub mod stack;
```

换成

```rust
mod device;
pub mod outbound;
pub mod routes;
pub mod stack;
mod stream;
```

`crates/rurge-proto-wireguard/src/lib.rs`——把

```rust
pub mod testing;

pub use routes::Routes;
```

换成

```rust
pub mod testing;

pub use outbound::WireGuardOutbound;
pub use routes::Routes;
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto-wireguard`
Expected: FAIL，编译错误——

```text
error[E0583]: file not found for module `device`
 --> crates\rurge-proto-wireguard\src\lib.rs:5:1
error[E0583]: file not found for module `outbound`
 --> crates\rurge-proto-wireguard\src\lib.rs:6:1
error[E0583]: file not found for module `stream`
 --> crates\rurge-proto-wireguard\src\lib.rs:9:1
error: could not compile `rurge-proto-wireguard` (lib) due to 3 previous errors
```

- [ ] **Step 3: 实现（新模块自带用例）**

新建 `crates/rurge-proto-wireguard/src/device.rs`：

```rust
//! A running tunnel (phase 2 M4 design 6.1, 6.5): the stack behind one lock
//! and the task that drives it. Only the task touches the peers' carriers,
//! and nothing sends, receives or waits with the lock held.

use crate::stack::{Outgoing, Stack};
use crate::stream::TunnelStream;
use crate::wire;
use rurge_config::wireguard::WireGuardSection;
use rurge_net::connector::{BoxedDatagram, ConnectOpts, Connector, Target};
use rurge_proto::OutboundError;
use std::future::{Future, poll_fn};
use std::io;
use std::net::SocketAddr;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};
use tokio::io::ReadBuf;
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tokio_util::task::AbortOnDropHandle;

/// How often WireGuard's timers run (M4 design 6.1).
const TICK: Duration = Duration::from_millis(250);
/// How many datagrams the carriers may hand in before the stack runs.
const BATCH: usize = 256;

pub(crate) struct Shared {
    pub(crate) stack: Mutex<Stack>,
    /// Wakes the task: a stream read, wrote or closed, a connection opened.
    kick: Notify,
}

impl Shared {
    pub(crate) fn kick(&self) {
        self.kick.notify_one();
    }
}

/// The tunnel while it runs: as long as the outbound or a connection
/// through it holds it.
pub struct Device {
    pub(crate) shared: Arc<Shared>,
    _task: AbortOnDropHandle<()>,
}

struct Carrier {
    datagram: BoxedDatagram,
    /// Handshake initiations go out marked (`set_tos` has not failed yet).
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
    /// A carrier to every peer, then the task and a handshake with each
    /// peer. Fails only when no peer can be reached at all: one that cannot
    /// is left without a carrier.
    pub(crate) async fn start(
        policy: &str,
        section: &WireGuardSection,
        connector: &Arc<dyn Connector>,
        opts: &ConnectOpts,
    ) -> Result<Arc<Device>, OutboundError> {
        let mut dials = JoinSet::new();
        for (i, peer) in section.peers.iter().enumerate() {
            let endpoint = Target::new(peer.endpoint.host.clone(), peer.endpoint.port);
            let (connector, opts) = (connector.clone(), opts.clone());
            dials.spawn(async move { (i, connector.connect_udp(&endpoint, &opts).await) });
        }
        let mut carriers: Vec<Option<Carrier>> = section.peers.iter().map(|_| None).collect();
        let mut failure = None;
        while let Some(joined) = dials.join_next().await {
            let Ok((i, dialled)) = joined else {
                continue;
            };
            match dialled {
                Ok(datagram) => {
                    carriers[i] = Some(Carrier {
                        datagram,
                        marks: true,
                    })
                }
                Err(e) => {
                    tracing::warn!(policy, peer = i + 1, error = %e, "wireguard: the peer cannot be reached");
                    failure = Some(e);
                }
            }
        }
        if carriers.iter().all(Option::is_none) {
            return Err(unreachable(failure.unwrap_or_else(|| {
                io::Error::other("wireguard: the tunnel has no peer")
            })));
        }
        let shared = Arc::new(Shared {
            stack: Mutex::new(Stack::new(section)),
            kick: Notify::new(),
        });
        let mut out = Vec::new();
        shared.stack.lock().expect("the tunnel").initiate(&mut out);
        let task = tokio::spawn(run(shared.clone(), carriers, out));
        Ok(Arc::new(Device {
            shared,
            _task: AbortOnDropHandle::new(task),
        }))
    }

    /// A TCP connection to `to` through the tunnel, once it is established.
    pub(crate) async fn connect(
        self: &Arc<Device>,
        to: SocketAddr,
    ) -> Result<TunnelStream, OutboundError> {
        let handle = self
            .shared
            .stack
            .lock()
            .expect("the tunnel")
            .connect(to)
            .map_err(|refusal| OutboundError::Proxy(refusal.to_string()))?;
        self.shared.kick();
        // the stream owns the connection from here: dropped, it goes away
        let stream = TunnelStream::new(self.clone(), handle);
        poll_fn(|cx| stream.poll_established(cx)).await?;
        Ok(stream)
    }
}

/// Every carrier's next datagram, whichever comes first; the carriers take
/// turns at being asked first.
fn recv_any<'a>(
    carriers: &'a [Option<Carrier>],
    buf: &'a mut [u8],
    first: &'a mut usize,
) -> impl Future<Output = (usize, io::Result<usize>)> + 'a {
    poll_fn(move |cx| {
        let n = carriers.len();
        for k in 0..n {
            let i = (*first + k) % n;
            let Some(carrier) = &carriers[i] else {
                continue;
            };
            let mut read = ReadBuf::new(&mut *buf);
            if let Poll::Ready(received) = carrier.datagram.poll_recv(cx, &mut read) {
                *first = (i + 1) % n;
                return Poll::Ready((i, received.map(|()| read.filled().len())));
            }
        }
        Poll::Pending
    })
}

/// A datagram some carrier holds already, without waiting for one.
fn ready(
    carriers: &[Option<Carrier>],
    buf: &mut [u8],
    first: &mut usize,
) -> Option<(usize, io::Result<usize>)> {
    let mut cx = Context::from_waker(Waker::noop());
    match pin!(recv_any(carriers, buf, first)).poll(&mut cx) {
        Poll::Ready(next) => Some(next),
        Poll::Pending => None,
    }
}

async fn send(carriers: &mut [Option<Carrier>], message: Outgoing) {
    let Some(Some(carrier)) = carriers.get_mut(message.peer) else {
        // a peer without a carrier: nothing reaches it
        return;
    };
    let marked =
        carrier.marks && wire::message_type(&message.datagram) == Some(wire::HANDSHAKE_INITIATION);
    if marked && carrier.datagram.set_tos(wire::HANDSHAKE_TOS).is_err() {
        carrier.marks = false;
    }
    let datagram = &carrier.datagram;
    if let Err(e) = poll_fn(|cx| datagram.poll_send(cx, &message.datagram)).await {
        tracing::trace!(peer = message.peer, error = %e, "wireguard: a message was not sent");
    }
    if marked && carrier.marks {
        let _ = carrier.datagram.set_tos(0);
    }
}

async fn run(shared: Arc<Shared>, mut carriers: Vec<Option<Carrier>>, mut out: Vec<Outgoing>) {
    let mut buf = vec![0u8; 65536];
    let mut next_tick = Instant::now() + TICK;
    let mut first = 0;
    loop {
        let deadline = {
            let mut stack = shared.stack.lock().expect("the tunnel");
            let now = Instant::now();
            if now >= next_tick {
                stack.tick(&mut out);
                next_tick = now + TICK;
            }
            let wait = stack.advance(now, &mut out);
            wait.map_or(next_tick, |wait| (now + wait).min(next_tick))
        };
        for message in out.drain(..) {
            send(&mut carriers, message).await;
        }
        tokio::select! {
            _ = shared.kick.notified() => {}
            (peer, received) = recv_any(&carriers, &mut buf, &mut first) => {
                let mut stack = shared.stack.lock().expect("the tunnel");
                let mut arrived = Some((peer, received));
                let mut taken = 0;
                while let Some((peer, received)) = arrived {
                    match received {
                        Ok(n) => stack.receive(peer, &mut buf[..n], Instant::now(), &mut out),
                        // an ICMP error the system reports on a connected socket
                        Err(e) => tracing::trace!(peer, error = %e, "wireguard: a carrier failed to receive"),
                    }
                    taken += 1;
                    // what else has arrived goes in before the stack runs
                    arrived = if taken < BATCH {
                        ready(&carriers, &mut buf, &mut first)
                    } else {
                        None
                    };
                }
            }
            _ = tokio::time::sleep_until(deadline.into()) => {}
        }
    }
}
```

新建 `crates/rurge-proto-wireguard/src/stream.rs`：

```rust
//! One TCP connection through the tunnel (phase 2 M4 design 6.1): reads and
//! writes go straight to its smoltcp socket under the lock, park on the
//! socket's wakers when there is nothing to read or no room to write, and
//! wake the device task so the stack runs.

use crate::device::Device;
use rurge_proto::OutboundError;
use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub struct TunnelStream {
    device: Arc<Device>,
    handle: SocketHandle,
}

impl TunnelStream {
    pub(crate) fn new(device: Arc<Device>, handle: SocketHandle) -> TunnelStream {
        TunnelStream { device, handle }
    }

    /// Ready once the connection is established; an error when the far end
    /// refused it.
    pub(crate) fn poll_established(&self, cx: &mut Context<'_>) -> Poll<Result<(), OutboundError>> {
        let mut stack = self.device.shared.stack.lock().expect("the tunnel");
        let socket = stack.tcp(self.handle);
        match socket.state() {
            tcp::State::SynSent | tcp::State::SynReceived => {
                socket.register_send_waker(cx.waker());
                Poll::Pending
            }
            // a reset in answer to the SYN
            tcp::State::Closed => Poll::Ready(Err(OutboundError::Io(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                "wireguard: the destination refused the connection",
            )))),
            _ => Poll::Ready(Ok(())),
        }
    }
}

impl AsyncRead for TunnelStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let mut stack = self.device.shared.stack.lock().expect("the tunnel");
        let socket = stack.tcp(self.handle);
        match socket.recv_slice(buf.initialize_unfilled()) {
            Ok(0) => {
                socket.register_recv_waker(cx.waker());
                Poll::Pending
            }
            Ok(n) => {
                buf.advance(n);
                drop(stack);
                // the window opened: the stack may have an update to send
                self.device.shared.kick();
                Poll::Ready(Ok(()))
            }
            // the far end finished: the end of the stream
            Err(tcp::RecvError::Finished) => Poll::Ready(Ok(())),
            Err(tcp::RecvError::InvalidState) => {
                Poll::Ready(Err(io::ErrorKind::ConnectionReset.into()))
            }
        }
    }
}

impl AsyncWrite for TunnelStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut stack = self.device.shared.stack.lock().expect("the tunnel");
        let socket = stack.tcp(self.handle);
        if !socket.may_send() {
            let kind = if socket.state() == tcp::State::Closed {
                io::ErrorKind::ConnectionReset
            } else {
                io::ErrorKind::BrokenPipe
            };
            return Poll::Ready(Err(kind.into()));
        }
        match socket.send_slice(data) {
            Ok(0) if !data.is_empty() => {
                socket.register_send_waker(cx.waker());
                Poll::Pending
            }
            Ok(n) => {
                drop(stack);
                self.device.shared.kick();
                Poll::Ready(Ok(n))
            }
            Err(tcp::SendError::InvalidState) => Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
        }
    }

    /// What was written is the stack's to send already.
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    /// A FIN, after what was written.
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.device
            .shared
            .stack
            .lock()
            .expect("the tunnel")
            .tcp(self.handle)
            .close();
        self.device.shared.kick();
        Poll::Ready(Ok(()))
    }
}

impl Drop for TunnelStream {
    fn drop(&mut self) {
        if let Ok(mut stack) = self.device.shared.stack.lock() {
            stack.release(self.handle, Instant::now());
        }
        self.device.shared.kick();
    }
}
```

新建 `crates/rurge-proto-wireguard/src/outbound.rs`：

```rust
//! `WireGuardOutbound` (phase 2 M4 design §6): the tunnel starts with the
//! first dial and runs while the outbound, or a connection through it,
//! lives.

use crate::device::Device;
use crate::stack::Refusal;
use rurge_config::HostName;
use rurge_config::spec::WireGuardSpec;
use rurge_config::wireguard::WireGuardSection;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Resolve, Target};
use rurge_proto::{Outbound, OutboundError};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct WireGuardOutbound {
    name: String,
    section: WireGuardSection,
    /// Destination names, without a `dns-server` (M4-D9).
    resolver: Arc<dyn Resolve>,
    /// What the carriers to the peers come from.
    connector: Arc<dyn Connector>,
    /// The running tunnel; the first dial starts it, the others wait.
    device: Mutex<Option<Arc<Device>>>,
}

impl WireGuardOutbound {
    /// Nothing is opened or resolved before the first dial (M4 design 6.5).
    pub fn new(
        name: &str,
        spec: &WireGuardSpec,
        resolver: Arc<dyn Resolve>,
        connector: Arc<dyn Connector>,
    ) -> WireGuardOutbound {
        WireGuardOutbound {
            name: name.to_string(),
            section: spec.section.clone(),
            resolver,
            connector,
            device: Mutex::new(None),
        }
    }

    pub(crate) async fn device(&self, opts: &ConnectOpts) -> Result<Arc<Device>, OutboundError> {
        let mut slot = self.device.lock().await;
        if let Some(device) = slot.as_ref() {
            return Ok(device.clone());
        }
        let device = Device::start(&self.name, &self.section, &self.connector, opts).await?;
        *slot = Some(device.clone());
        Ok(device)
    }

    /// Where a connection to `target` goes: its address, or its name
    /// resolved on this machine (M4-D9).
    async fn address(&self, target: &Target) -> Result<IpAddr, OutboundError> {
        let name = match &target.host {
            HostName::Ip(ip) => return Ok(*ip),
            HostName::Domain(name) => name,
        };
        let addrs =
            self.resolver.resolve(name).await.map_err(|_| {
                OutboundError::Dns(format!("wireguard: dns lookup of {name} failed"))
            })?;
        pick(&addrs, &self.section).map_err(|refusal| OutboundError::Proxy(refusal.to_string()))
    }

    async fn dial(
        &self,
        target: &Target,
        opts: &ConnectOpts,
    ) -> Result<BoxedStream, OutboundError> {
        let device = self.device(opts).await?;
        let ip = self.address(target).await?;
        let stream = device.connect(SocketAddr::new(ip, target.port)).await?;
        Ok(Box::new(stream))
    }
}

/// The address to connect to among `addrs`: of a family the tunnel has an
/// address of, IPv6 first with `prefer-ipv6`.
fn pick(addrs: &[IpAddr], section: &WireGuardSection) -> Result<IpAddr, Refusal> {
    let usable = |ip: &IpAddr| match ip {
        IpAddr::V4(_) => section.self_ip.is_some(),
        IpAddr::V6(_) => section.self_ip_v6.is_some(),
    };
    let mut ordered: Vec<IpAddr> = addrs.iter().copied().filter(usable).collect();
    // stable: the preferred family first, each in the order answered
    ordered.sort_by_key(|ip| ip.is_ipv6() != section.prefer_ipv6);
    match (ordered.first(), addrs.first()) {
        (Some(ip), _) => Ok(*ip),
        (None, Some(ip)) => Err(Refusal::NoAddress(*ip)),
        (None, None) => Err(Refusal::NoAddress(IpAddr::V4(
            std::net::Ipv4Addr::UNSPECIFIED,
        ))),
    }
}

impl Outbound for WireGuardOutbound {
    fn name(&self) -> &str {
        &self.name
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            match tokio::time::timeout(opts.timeout, self.dial(target, opts)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{ECHO_PORT, FakeWgPeer, PeerOpts, endpoint, keypair, section};
    use rurge_config::wireguard::PeerEndpoint;
    use rurge_net::connector::{BoxedDatagram, Datagram, DirectConnector, SystemResolve};
    use std::io;
    use std::net::Ipv4Addr;
    use std::sync::Mutex as StdMutex;
    use std::task::{Context, Poll};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadBuf};

    /// Names to addresses, for the destinations resolved on this machine.
    struct Names(Vec<(&'static str, Vec<IpAddr>)>);

    impl Resolve for Names {
        fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            let found = self.0.iter().find(|(name, _)| *name == host);
            Box::pin(std::future::ready(match found {
                Some((_, addrs)) => Ok(addrs.clone()),
                None => Err(io::Error::new(io::ErrorKind::NotFound, "no such name")),
            }))
        }
    }

    fn no_names() -> Arc<dyn Resolve> {
        Arc::new(Names(Vec::new()))
    }

    fn direct() -> Arc<dyn Connector> {
        Arc::new(DirectConnector::new(Arc::new(SystemResolve)))
    }

    /// A tunnel to one peer that takes 10.0.0.0/8, and the peer.
    async fn tunnel(
        opts: PeerOpts,
        edit: impl FnOnce(&mut WireGuardSection),
    ) -> (FakeWgPeer, WireGuardOutbound) {
        tunnel_with(opts, edit, no_names(), direct()).await
    }

    async fn tunnel_with(
        opts: PeerOpts,
        edit: impl FnOnce(&mut WireGuardSection),
        resolver: Arc<dyn Resolve>,
        connector: Arc<dyn Connector>,
    ) -> (FakeWgPeer, WireGuardOutbound) {
        let (private, public) = keypair();
        let peer = FakeWgPeer::start(public, opts).await;
        let mut section = section(
            private,
            Ipv4Addr::new(10, 9, 0, 2),
            &[(peer.public_key(), &["10.0.0.0/8"])],
        );
        section.peers[0].endpoint = endpoint(peer.addr());
        edit(&mut section);
        let outbound =
            WireGuardOutbound::new("WG", &WireGuardSpec { section }, resolver, connector);
        (peer, outbound)
    }

    fn at(host: &str, port: u16) -> Target {
        Target::new(HostName::parse(host), port)
    }

    fn within(secs: u64) -> ConnectOpts {
        ConnectOpts {
            timeout: Duration::from_secs(secs),
        }
    }

    async fn echo(stream: &mut BoxedStream, data: &[u8]) -> Vec<u8> {
        tokio::time::timeout(Duration::from_secs(5), async {
            stream.write_all(data).await.unwrap();
            let mut back = vec![0u8; data.len()];
            stream.read_exact(&mut back).await.unwrap();
            back
        })
        .await
        .expect("the echo came back")
    }

    #[tokio::test]
    async fn a_connection_through_the_tunnel_echoes() {
        let (peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let mut stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut stream, b"ping").await, b"ping");
        assert_eq!(peer.core().handshakes, 1);
        assert_eq!(
            peer.core().connected_to,
            ["10.0.0.1:7".parse::<SocketAddr>().unwrap()]
        );
    }

    /// Flow control both ways: more than the buffers hold, written and read
    /// at once; then the end of the stream both ways. The FIN goes last:
    /// the peer is smoltcp 0.12 too, which in CLOSE-WAIT stops
    /// retransmitting what it still has in flight.
    #[tokio::test]
    async fn a_large_transfer_goes_through_whole() {
        let (_peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        let data: Vec<u8> = (0..2 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
        let (mut read, mut write) = tokio::io::split(stream);
        let sent = data.clone();
        let writer = tokio::spawn(async move {
            write.write_all(&sent).await.unwrap();
            write
        });
        let mut back = vec![0u8; data.len()];
        tokio::time::timeout(Duration::from_secs(30), read.read_exact(&mut back))
            .await
            .expect("everything came back")
            .unwrap();
        assert!(back == data, "the bytes came back in order");
        let mut write = writer.await.unwrap();
        write.shutdown().await.unwrap();
        let mut rest = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), read.read_to_end(&mut rest))
            .await
            .expect("the far end finished too")
            .unwrap();
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn every_message_carries_the_client_id() {
        let id = [83, 12, 235];
        let (peer, wg) = tunnel(
            PeerOpts {
                client_id: Some(id),
                ..PeerOpts::default()
            },
            |s| s.peers[0].client_id = Some(id),
        )
        .await;
        let mut stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut stream, b"ping").await, b"ping");
        let core = peer.core();
        assert!(
            core.reserved.iter().all(|r| *r == id),
            "{:?}",
            core.reserved
        );
    }

    /// Without a `dns-server`, a name is resolved on this machine, with
    /// `[Host]` (M4-D9); the family follows the tunnel's addresses and
    /// `prefer-ipv6`.
    #[tokio::test]
    async fn a_name_is_resolved_on_this_machine() {
        let names = Arc::new(Names(vec![(
            "echo.test",
            vec!["fd00::1".parse().unwrap(), "10.0.0.1".parse().unwrap()],
        )]));
        for (prefer_ipv6, expected) in [(false, "10.0.0.1:7"), (true, "[fd00::1]:7")] {
            let (peer, wg) = tunnel_with(
                PeerOpts::default(),
                |s| {
                    s.self_ip_v6 = Some("fd00::2".parse().unwrap());
                    s.peers[0].allowed_ips.push("fd00::/64".parse().unwrap());
                    s.prefer_ipv6 = prefer_ipv6;
                },
                names.clone(),
                direct(),
            )
            .await;
            let mut stream = wg
                .connect_tcp(&at("echo.test", ECHO_PORT), &within(5))
                .await
                .expect("a connection");
            assert_eq!(echo(&mut stream, b"ping").await, b"ping");
            assert_eq!(
                peer.core().connected_to,
                [expected.parse::<SocketAddr>().unwrap()]
            );
        }
        // an IPv4-only tunnel takes the IPv4 answer whatever `prefer-ipv6` says
        let (peer, wg) = tunnel_with(
            PeerOpts::default(),
            |s| s.prefer_ipv6 = true,
            names.clone(),
            direct(),
        )
        .await;
        wg.connect_tcp(&at("echo.test", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(
            peer.core().connected_to,
            ["10.0.0.1:7".parse::<SocketAddr>().unwrap()]
        );
    }

    #[tokio::test]
    async fn names_and_addresses_the_tunnel_cannot_use_fail_at_once() {
        let names = Arc::new(Names(vec![("v6.test", vec!["fd00::1".parse().unwrap()])]));
        let (_peer, wg) = tunnel_with(PeerOpts::default(), |_| {}, names, direct()).await;
        let refused = |target: Target| {
            let wg = &wg;
            async move {
                let started = std::time::Instant::now();
                let e = wg
                    .connect_tcp(&target, &within(5))
                    .await
                    .map(|_| ())
                    .unwrap_err();
                assert!(started.elapsed() < Duration::from_secs(2), "{e}");
                e.to_string()
            }
        };
        assert_eq!(
            refused(at("192.0.2.1", 80)).await,
            "wireguard: no peer's allowed-ips covers 192.0.2.1"
        );
        assert_eq!(
            refused(at("v6.test", 80)).await,
            "wireguard: the tunnel has no IPv6 address"
        );
        assert_eq!(
            refused(at("nx.test", 80)).await,
            "dns: wireguard: dns lookup of nx.test failed"
        );
    }

    #[tokio::test]
    async fn a_closed_port_refuses_the_connection() {
        let (_peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let e = wg
            .connect_tcp(&at("10.0.0.1", 1), &within(5))
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            "wireguard: the destination refused the connection"
        );
    }

    #[tokio::test]
    async fn a_peer_that_never_answers_runs_the_dial_out_of_time() {
        let (peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        peer.go_silent(true);
        let e = wg
            .connect_tcp(
                &at("10.0.0.1", ECHO_PORT),
                &ConnectOpts {
                    timeout: Duration::from_millis(500),
                },
            )
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(matches!(e, OutboundError::Timeout), "{e}");
    }

    /// One tunnel and one handshake for every dial that comes in together.
    #[tokio::test]
    async fn dials_that_come_together_share_one_tunnel() {
        let (peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let (target, opts) = (at("10.0.0.1", ECHO_PORT), within(5));
        let dial = || wg.connect_tcp(&target, &opts);
        let (a, b, c, d, e) = tokio::join!(dial(), dial(), dial(), dial(), dial());
        for dialled in [a, b, c, d, e] {
            let mut stream = dialled.expect("a connection");
            assert_eq!(echo(&mut stream, b"x").await, b"x");
        }
        assert_eq!(peer.core().handshakes, 1);
        assert_eq!(peer.core().accepted, 5);
    }

    /// The outbound gone and no connection left: the tunnel stops. A
    /// connection keeps it running until it ends.
    #[tokio::test]
    async fn the_tunnel_lives_as_long_as_the_outbound_or_a_connection() {
        let (_peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let mut stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        let device = Arc::downgrade(&wg.device(&within(5)).await.unwrap());
        drop(wg);
        assert_eq!(echo(&mut stream, b"still").await, b"still");
        assert!(device.upgrade().is_some());
        drop(stream);
        assert!(
            device.upgrade().is_none(),
            "the last holder was the connection"
        );
    }

    /// Handshake initiations go out marked AF41 (manual), the rest not.
    #[tokio::test]
    async fn handshake_initiations_go_out_marked() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let recording: Arc<dyn Connector> = Arc::new(Recording {
            inner: direct(),
            log: log.clone(),
        });
        let (_peer, wg) = tunnel_with(PeerOpts::default(), |_| {}, no_names(), recording).await;
        let mut stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut stream, b"ping").await, b"ping");
        let log = log.lock().unwrap().clone();
        assert_eq!(log[..3], ["tos 0x88", "send 1", "tos 0x00"], "{log:?}");
        assert!(
            log[3..].iter().all(|entry| entry == "send 4"),
            "only data after the handshake: {log:?}"
        );
    }

    /// Connects through `inner` and notes every `set_tos` and the type of
    /// every message sent.
    struct Recording {
        inner: Arc<dyn Connector>,
        log: Arc<StdMutex<Vec<String>>>,
    }

    struct RecordingDatagram {
        inner: BoxedDatagram,
        log: Arc<StdMutex<Vec<String>>>,
    }

    impl Connector for Recording {
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
            Box::pin(async move {
                let inner = self.inner.connect_udp(target, opts).await?;
                Ok(Box::new(RecordingDatagram {
                    inner,
                    log: self.log.clone(),
                }) as BoxedDatagram)
            })
        }
    }

    impl Datagram for RecordingDatagram {
        fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
            let sent = self.inner.poll_send(cx, buf);
            if sent.is_ready() {
                self.log.lock().unwrap().push(format!("send {}", buf[0]));
            }
            sent
        }

        fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            self.inner.poll_recv(cx, buf)
        }

        fn set_tos(&self, tos: u8) -> io::Result<()> {
            self.log.lock().unwrap().push(format!("tos {tos:#04x}"));
            Ok(())
        }
    }

    /// The endpoint of the only peer cannot be resolved: the dial fails, and
    /// the next one tries again.
    #[tokio::test]
    async fn a_tunnel_whose_peer_cannot_be_reached_does_not_start() {
        let (private, _) = keypair();
        let mut section = section(
            private,
            Ipv4Addr::new(10, 9, 0, 2),
            &[(keypair().1, &["10.0.0.0/8"])],
        );
        section.peers[0].endpoint = PeerEndpoint {
            host: HostName::parse("nx.invalid"),
            port: 51820,
        };
        let failing: Arc<dyn Connector> = Arc::new(DirectConnector::new(no_names()));
        let wg = WireGuardOutbound::new("WG", &WireGuardSpec { section }, no_names(), failing);
        for _ in 0..2 {
            let e = wg
                .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
                .await
                .map(|_| ())
                .unwrap_err();
            assert_eq!(e.to_string(), "no such name");
        }
        assert!(wg.device.lock().await.is_none());
    }
}
```

要点：
- 锁（`std::sync::Mutex<Stack>`）里只做内存操作；收发 UDP、等待都在锁外（M4-D5）。设备任务一轮：推定时器与协议栈（锁内）→ 把要发的逐个发出（锁外）→ 等"被唤醒 / 收到报文 / 截止时间"其中之一。收到一个报文后，用空唤醒器把各载体上已经到了的全部收下（最多 `BATCH` = 256 个）再推进协议栈（P9）。
- 载体一律经连接器建（`underlying-proxy` 时是链路的连接器，Task 7）：连接器不载 UDP 时（`io::ErrorKind::Unsupported`）拨号得到 `OutboundError::Unsupported("wireguard over underlying-proxy")`（P17）；全部 peer 都连不上时得到最后一个错误，隧道不启动，下一次拨号重来。
- 握手发起包前后切换载体的 TOS（0x88，然后回到 0）：`handshake_initiations_go_out_marked` 用一个记录调用的连接器断言；`set_tos` 失败一次就不再标记。
- 流被丢弃时交还协议栈（`Stack::release`）并唤醒任务；读到 `Finished` 是 EOF，`InvalidState` 是连接被重置；写端关闭发 FIN。**流持有设备**：出站被释放后，隧道活到最后一个经它的连接结束（P11，`the_tunnel_lives_as_long_as_the_outbound_or_a_connection`）。
- 建连：先查路由（`no peer's allowed-ips covers …` 等立即失败），再等连接建立；对端以 RST 回应 SYN 时是 `wireguard: the destination refused the connection`（`io::ErrorKind::ConnectionRefused`）。
- `a_large_transfer_goes_through_whole` 双向同时传 2 MiB（超过两边的缓冲），最后才关写端：对端也是 smoltcp 0.12，它在 CLOSE-WAIT 里不重传在途的数据（P3 ④）。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto-wireguard` → 25 passed（新增 `outbound::tests` 11 条：回显、大块双向传输、`client-id`、本机解析与 `prefer-ipv6`、用不了的名字与地址立即失败、关着的端口、不回应的 peer 在时限处超时、同时进来的拨号共用一条隧道与一次握手、隧道的寿命、握手包的 TOS、peer 一个都连不上时不启动）。

- [ ] **Step 5: 门禁与提交**

跑门禁（47 个测试二进制，1090 通过 / 1 忽略）。

```bash
git add crates/rurge-proto-wireguard
git commit -m "feat(proto-wireguard): WireGuardOutbound——设备任务、载体与批量收、TunnelStream、握手包 TOS、本机解析目标域名；FakeWgPeer"
```

### Task 5: 隧道内 DNS

节里配了 `dns-server` 时，目标域名经隧道查询（设计 6.3，细节见 P4、P14）：smoltcp 的 UDP 套接字、源地址是对应族的本端地址；A 与 AAAA 按本端有的地址族同时问；按列表顺序换服务器，每个最多等 2 秒；`system` 在那个位置改用本机解析；第一个作答的服务器为准；成功的回答按 TTL 缓存。报文的编解码用工作区已有的 `hickory-proto`。

**Files:**
- Modify: `Cargo.toml`（smoltcp 加 `socket-udp`）、`crates/rurge-proto-wireguard/Cargo.toml`（`hickory-proto`）、`src/lib.rs`（`mod dns;`）
- Create: `crates/rurge-proto-wireguard/src/dns.rs`（问、答与缓存，与用例）
- Modify: `crates/rurge-proto-wireguard/src/stack.rs`（UDP 套接字：`udp_open` / `udp` / `udp_close`；本端端口的分配顾及 UDP）
- Modify: `crates/rurge-proto-wireguard/src/device.rs`（`Device::query`：经隧道的一问一答）
- Modify: `crates/rurge-proto-wireguard/src/outbound.rs`（`lookup`、`ask`、缓存与 `DNS_WAIT`，与用例）
- Modify: `crates/rurge-proto-wireguard/src/testing/mod.rs`（`DNS_ADDRESS` 上的名字服务器、`PeerOpts.dns`、`PeerCore.dns_questions`、`dns_reply`）

**Interfaces:**
- Consumes: Task 1 的 `TunnelDns`；Task 3 的 `Stack`、`Refusal`；Task 4 的 `Device`、`WireGuardOutbound`、`testing::{FakeWgPeer, PeerOpts}`；`hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode}`、`hickory_proto::rr::{Name, RData, RecordType}`。
- Produces:
  - crate 内：`dns::{Family (V4 / V6), question(id: u16, name: &str, family: Family) -> Option<Vec<u8>>, answer(response: &[u8], id: u16, name: &str, family: Family) -> Option<Vec<(IpAddr, u32)>>, Cache { get(&self, name, now) -> Option<Vec<IpAddr>>, put(&mut self, name, addrs, ttl, now) }}`；`Stack::{udp_open(&mut self, to: IpAddr) -> Result<SocketHandle, Refusal>, udp(&mut self, SocketHandle) -> &mut udp::Socket<'static>, udp_close(&mut self, SocketHandle)}`；`Device::query<T>(self: &Arc<Device>, server: SocketAddr, message: &[u8], wait: Duration, accept: impl Fn(&[u8]) -> Option<T>) -> Option<T>`
  - `testing::DNS_ADDRESS: Ipv4Addr (10.0.0.53)`；`PeerOpts.dns: Vec<(String, Vec<IpAddr>)>`（名字服务器认识的名字，其余一律 NXDOMAIN）；`PeerCore.dns_questions: Vec<String>`（`<名字> A` / `<名字> AAAA`）；`pub(crate) testing::dns_reply(query: &[u8], lookup: impl FnMut(&str, Family) -> Option<Vec<IpAddr>>) -> Option<Vec<u8>>`

- [ ] **Step 1: 先写问答模块、对端的名字服务器与出站用例**

问答与缓存是纯函数，先写（自带用例）；对端的名字服务器要用它们：

`Cargo.toml`——把

```toml
# the tunnel's own IP stack: TCP with Reno congestion control, fragments of up to
# 16 KiB reassembled; no phy, no log
smoltcp = { version = "0.12.0", default-features = false, features = ["std", "medium-ip", "proto-ipv4", "proto-ipv6", "proto-ipv4-fragmentation", "proto-ipv6-fragmentation", "socket-tcp", "socket-tcp-reno", "async", "reassembly-buffer-size-16384", "reassembly-buffer-count-4"] }
```

换成

```toml
# the tunnel's own IP stack: TCP with Reno congestion control, UDP for the tunnel's
# DNS, fragments of up to 16 KiB reassembled; no phy, no log
smoltcp = { version = "0.12.0", default-features = false, features = ["std", "medium-ip", "proto-ipv4", "proto-ipv6", "proto-ipv4-fragmentation", "proto-ipv6-fragmentation", "socket-tcp", "socket-tcp-reno", "socket-udp", "async", "reassembly-buffer-size-16384", "reassembly-buffer-count-4"] }
```

`crates/rurge-proto-wireguard/Cargo.toml`——把

```toml
getrandom.workspace = true
```

换成

```toml
getrandom.workspace = true
hickory-proto.workspace = true
```

`crates/rurge-proto-wireguard/src/lib.rs`——把

```rust
mod device;
```

换成

```rust
mod device;
mod dns;
```

新建 `crates/rurge-proto-wireguard/src/dns.rs`：

```rust
//! Destination names inside the tunnel (phase 2 M4 design 6.3): the A and
//! AAAA questions to a section's `dns-server`, their answers, and a small
//! cache of the answers by their TTL.

use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::{Name, RData, RecordType};
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// How many names the cache holds at most.
const CACHE_SIZE: usize = 256;
/// How long an answer is kept at most, whatever its TTL.
const LONGEST_TTL: Duration = Duration::from_secs(3600);

/// Which addresses a question asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Family {
    V4,
    V6,
}

impl Family {
    fn record_type(self) -> RecordType {
        match self {
            Family::V4 => RecordType::A,
            Family::V6 => RecordType::AAAA,
        }
    }
}

/// The question for `name`'s addresses of `family`, with `id`; `None` for
/// a name no question can hold.
pub(crate) fn question(id: u16, name: &str, family: Family) -> Option<Vec<u8>> {
    let mut fqdn = name.trim_end_matches('.').to_ascii_lowercase();
    fqdn.push('.');
    let name = Name::from_ascii(&fqdn).ok()?;
    let mut message = Message::new(id, MessageType::Query, OpCode::Query);
    message.metadata.recursion_desired = true;
    message.add_query(Query::query(name, family.record_type()));
    message.to_vec().ok()
}

/// The addresses in `response`, each with its TTL, when it answers the
/// question `id` asked for `name`: empty for a name that has none. `None`
/// for anything else — the answer to another question, a server failure.
pub(crate) fn answer(
    response: &[u8],
    id: u16,
    name: &str,
    family: Family,
) -> Option<Vec<(IpAddr, u32)>> {
    let message = Message::from_vec(response).ok()?;
    if message.id != id || message.message_type != MessageType::Response {
        return None;
    }
    if !matches!(
        message.response_code,
        ResponseCode::NoError | ResponseCode::NXDomain
    ) {
        return None;
    }
    let asked = message.queries.first()?;
    let asked_name = asked.name().to_ascii();
    if asked.query_type() != family.record_type()
        || !asked_name
            .trim_end_matches('.')
            .eq_ignore_ascii_case(name.trim_end_matches('.'))
    {
        return None;
    }
    Some(
        message
            .answers
            .iter()
            .filter_map(|record| match (&record.data, family) {
                (RData::A(a), Family::V4) => Some((IpAddr::V4(a.0), record.ttl)),
                (RData::AAAA(a), Family::V6) => Some((IpAddr::V6(a.0), record.ttl)),
                _ => None,
            })
            .collect(),
    )
}

/// Answers by name until their TTL runs out; only answers with addresses.
#[derive(Default)]
pub(crate) struct Cache {
    entries: HashMap<String, (Vec<IpAddr>, Instant)>,
}

impl Cache {
    pub(crate) fn get(&self, name: &str, now: Instant) -> Option<Vec<IpAddr>> {
        self.entries
            .get(name)
            .filter(|(_, until)| now < *until)
            .map(|(addrs, _)| addrs.clone())
    }

    pub(crate) fn put(&mut self, name: &str, addrs: Vec<IpAddr>, ttl: Duration, now: Instant) {
        if addrs.is_empty() || ttl.is_zero() {
            return;
        }
        if self.entries.len() >= CACHE_SIZE && !self.entries.contains_key(name) {
            self.entries.retain(|_, (_, until)| now < *until);
            let soonest = self
                .entries
                .iter()
                .min_by_key(|(_, (_, until))| *until)
                .map(|(name, _)| name.clone());
            if self.entries.len() >= CACHE_SIZE
                && let Some(soonest) = soonest
            {
                self.entries.remove(&soonest);
            }
        }
        self.entries
            .insert(name.to_string(), (addrs, now + ttl.min(LONGEST_TTL)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::dns_reply;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn an_answer_is_read_for_the_question_asked() {
        let asked = question(7, "Echo.Test.", Family::V4).unwrap();
        let reply = dns_reply(&asked, |name, family| {
            assert_eq!((name, family), ("echo.test", Family::V4));
            Some(vec![ip("10.0.0.1"), ip("10.0.0.2")])
        })
        .unwrap();
        assert_eq!(
            answer(&reply, 7, "echo.test", Family::V4),
            Some(vec![(ip("10.0.0.1"), 60), (ip("10.0.0.2"), 60)])
        );
        // another question's answer is no answer
        assert_eq!(answer(&reply, 8, "echo.test", Family::V4), None);
        assert_eq!(answer(&reply, 7, "other.test", Family::V4), None);
        assert_eq!(answer(&reply, 7, "echo.test", Family::V6), None);
        assert_eq!(answer(b"garbage", 7, "echo.test", Family::V4), None);
        assert_eq!(
            answer(&asked, 7, "echo.test", Family::V4),
            None,
            "a question"
        );
    }

    /// A name the server does not know has no addresses: an answer all the
    /// same, which ends the search.
    #[test]
    fn a_name_without_addresses_is_an_empty_answer() {
        let asked = question(9, "nx.test", Family::V6).unwrap();
        let reply = dns_reply(&asked, |_, _| None).unwrap();
        assert_eq!(answer(&reply, 9, "nx.test", Family::V6), Some(Vec::new()));
        assert_eq!(
            question(1, "bücher.test", Family::V4),
            None,
            "no IDN before M8"
        );
    }

    #[test]
    fn the_cache_keeps_an_answer_for_its_ttl() {
        let mut cache = Cache::default();
        let now = Instant::now();
        cache.put("a.test", vec![ip("10.0.0.1")], Duration::from_secs(30), now);
        cache.put("none.test", Vec::new(), Duration::from_secs(30), now);
        cache.put("zero.test", vec![ip("10.0.0.2")], Duration::ZERO, now);
        assert_eq!(
            cache.get("a.test", now + Duration::from_secs(29)),
            Some(vec![ip("10.0.0.1")])
        );
        assert_eq!(cache.get("a.test", now + Duration::from_secs(30)), None);
        assert_eq!(cache.get("none.test", now), None);
        assert_eq!(cache.get("zero.test", now), None);
        // a TTL of a day is kept an hour
        cache.put(
            "long.test",
            vec![ip("10.0.0.3")],
            Duration::from_secs(86400),
            now,
        );
        assert_eq!(cache.get("long.test", now + LONGEST_TTL), None);
    }

    #[test]
    fn a_full_cache_makes_room_by_what_runs_out_first() {
        let mut cache = Cache::default();
        let now = Instant::now();
        for i in 0..CACHE_SIZE {
            let ttl = Duration::from_secs(100 + i as u64);
            cache.put(&format!("n{i}.test"), vec![ip("10.0.0.1")], ttl, now);
        }
        cache.put(
            "new.test",
            vec![ip("10.0.0.9")],
            Duration::from_secs(50),
            now,
        );
        assert_eq!(cache.entries.len(), CACHE_SIZE);
        assert_eq!(
            cache.get("n0.test", now),
            None,
            "the one that ran out soonest"
        );
        assert!(cache.get("n1.test", now).is_some());
        assert!(cache.get("new.test", now).is_some());
    }
}
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
//! answers on every address routed to it — a TCP echo service on port 7.
//! `FakeWgPeer` puts one on a loopback UDP port.
```

换成

```rust
//! answers on every address routed to it — a TCP echo service on port 7 and
//! a name server at `DNS_ADDRESS`. `FakeWgPeer` puts one on a loopback UDP
//! port.
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
pub use peer::FakeWgPeer;

use crate::stack::Queues;
```

换成

```rust
pub use peer::FakeWgPeer;

use crate::dns::Family;
use crate::stack::Queues;
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
use boringtun::x25519::{PublicKey, StaticSecret};
```

换成

```rust
use boringtun::x25519::{PublicKey, StaticSecret};
use hickory_proto::op::{Message, MessageType, OpCode, ResponseCode};
use hickory_proto::rr::rdata::{A, AAAA};
use hickory_proto::rr::{RData, Record, RecordType};
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
use smoltcp::socket::tcp;
```

换成

```rust
use smoltcp::socket::{tcp, udp};
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
const BACKLOG: usize = 8;
```

换成

```rust
const BACKLOG: usize = 8;
/// Where a peer's name server listens, on port 53.
pub const DNS_ADDRESS: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 53);

/// A name server's answer to `query`: what `lookup` gives for its name and
/// family, with a TTL of 60 seconds, or no such name.
pub(crate) fn dns_reply(
    query: &[u8],
    mut lookup: impl FnMut(&str, Family) -> Option<Vec<IpAddr>>,
) -> Option<Vec<u8>> {
    let asked = Message::from_vec(query).ok()?;
    let question = asked.queries.first()?.clone();
    let family = match question.query_type() {
        RecordType::A => Family::V4,
        RecordType::AAAA => Family::V6,
        _ => return None,
    };
    let name = question.name().to_ascii();
    let mut reply = Message::new(asked.id, MessageType::Response, OpCode::Query);
    reply.metadata.recursion_desired = true;
    reply.metadata.recursion_available = true;
    match lookup(name.trim_end_matches('.'), family) {
        Some(addrs) => {
            for addr in addrs {
                let data = match addr {
                    IpAddr::V4(v4) => RData::A(A(v4)),
                    IpAddr::V6(v6) => RData::AAAA(AAAA(v6)),
                };
                reply
                    .answers
                    .push(Record::from_rdata(question.name().clone(), 60, data));
            }
        }
        None => reply.metadata.response_code = ResponseCode::NXDomain,
    }
    reply.add_query(question);
    reply.to_vec().ok()
}
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
    pub mtu: usize,
```

换成

```rust
    pub mtu: usize,
    /// What its name server answers: names and their addresses; any other
    /// name does not exist.
    pub dns: Vec<(String, Vec<IpAddr>)>,
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
            mtu: 1420,
```

换成

```rust
            mtu: 1420,
            dns: Vec::new(),
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
    conns: Vec<Conn>,
```

换成

```rust
    conns: Vec<Conn>,
    name_server: SocketHandle,
    names: Vec<(String, Vec<IpAddr>)>,
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
    pub resets: usize,
```

换成

```rust
    pub resets: usize,
    /// The questions its name server was asked: `<name> A` or `<name> AAAA`.
    pub dns_questions: Vec<String>,
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
            .collect();
```

换成

```rust
            .collect();
        let buffer = || udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 8], vec![0; 8192]);
        let mut name_server = udp::Socket::new(buffer(), buffer());
        name_server
            .bind((IpAddress::Ipv4(DNS_ADDRESS), 53))
            .expect("the name server's port");
        let name_server = sockets.add(name_server);
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
            conns: Vec::new(),
```

换成

```rust
            conns: Vec::new(),
            name_server,
            names: opts.dns.clone(),
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
            resets: 0,
```

换成

```rust
            resets: 0,
            dns_questions: Vec::new(),
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
        self.resets += resets;
```

换成

```rust
        self.resets += resets;
        self.answer_questions();
    }

    fn answer_questions(&mut self) {
        let socket = self.sockets.get_mut::<udp::Socket>(self.name_server);
        let mut queries = Vec::new();
        while let Ok((query, meta)) = socket.recv() {
            queries.push((query.to_vec(), meta.endpoint));
        }
        for (query, from) in queries {
            let (names, asked) = (&self.names, &mut self.dns_questions);
            let reply = dns_reply(&query, |name, family| {
                let kind = if family == Family::V4 { "A" } else { "AAAA" };
                asked.push(format!("{name} {kind}"));
                let addrs = &names.iter().find(|(n, _)| n == name)?.1;
                Some(
                    addrs
                        .iter()
                        .filter(|a| a.is_ipv4() == (family == Family::V4))
                        .copied()
                        .collect(),
                )
            });
            if let Some(reply) = reply {
                let socket = self.sockets.get_mut::<udp::Socket>(self.name_server);
                let _ = socket.send_slice(&reply, from);
            }
        }
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
    use crate::testing::{ECHO_PORT, FakeWgPeer, PeerOpts, endpoint, keypair, section};
```

换成

```rust
    use crate::testing::{
        DNS_ADDRESS, ECHO_PORT, FakeWgPeer, PeerOpts, endpoint, keypair, section,
    };
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
            Ok(())
        }
    }

    /// The endpoint of the only peer cannot be resolved: the dial fails, and
```

换成

```rust
            Ok(())
        }
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// A peer that answers `echo.test` with `addrs`, and whose name server
    /// comes after `before` in the section's `dns-server`.
    async fn with_tunnel_dns(
        addrs: &[&str],
        before: &[TunnelDns],
        edit: impl FnOnce(&mut WireGuardSection),
    ) -> (FakeWgPeer, WireGuardOutbound) {
        let names = Arc::new(Names(vec![("local.test", vec![ip("10.0.0.1")])]));
        let dns = vec![(
            "echo.test".to_string(),
            addrs.iter().map(|a| ip(a)).collect(),
        )];
        let mut servers = before.to_vec();
        servers.push(TunnelDns::Server(SocketAddr::new(DNS_ADDRESS.into(), 53)));
        tunnel_with(
            PeerOpts {
                dns,
                ..PeerOpts::default()
            },
            |s| {
                s.dns_servers = servers;
                edit(s);
            },
            names,
            direct(),
        )
        .await
    }

    /// With a `dns-server`, a name is asked through the tunnel, and the
    /// answer kept for its TTL.
    #[tokio::test]
    async fn a_name_is_resolved_through_the_tunnel() {
        let (peer, wg) = with_tunnel_dns(&["10.0.0.1"], &[], |_| {}).await;
        for _ in 0..2 {
            let mut stream = wg
                .connect_tcp(&at("echo.test", ECHO_PORT), &within(5))
                .await
                .expect("a connection");
            assert_eq!(echo(&mut stream, b"ping").await, b"ping");
        }
        assert_eq!(peer.core().dns_questions, ["echo.test A"], "asked once");
    }

    /// Both families asked at once when the tunnel has both addresses;
    /// `prefer-ipv6` picks among the answers.
    #[tokio::test]
    async fn prefer_ipv6_picks_among_what_the_tunnel_dns_answers() {
        for (prefer_ipv6, expected) in [(false, "10.0.0.1:7"), (true, "[fd00::1]:7")] {
            let (peer, wg) = with_tunnel_dns(&["10.0.0.1", "fd00::1"], &[], |s| {
                s.self_ip_v6 = Some("fd00::2".parse().unwrap());
                s.peers[0].allowed_ips.push("fd00::/64".parse().unwrap());
                s.prefer_ipv6 = prefer_ipv6;
            })
            .await;
            wg.connect_tcp(&at("echo.test", ECHO_PORT), &within(5))
                .await
                .expect("a connection");
            let mut asked = peer.core().dns_questions.clone();
            asked.sort();
            assert_eq!(asked, ["echo.test A", "echo.test AAAA"]);
            assert_eq!(
                peer.core().connected_to,
                [expected.parse::<SocketAddr>().unwrap()]
            );
        }
    }

    /// A server that says nothing gives way to the next after `dns_wait`;
    /// one the tunnel cannot reach is passed over at once.
    #[tokio::test]
    async fn the_next_dns_server_is_asked_when_one_cannot_answer() {
        let silent = TunnelDns::Server("10.0.0.54:53".parse().unwrap());
        let (_peer, mut wg) = with_tunnel_dns(&["10.0.0.1"], &[silent], |_| {}).await;
        wg.dns_wait = Duration::from_millis(300);
        let started = std::time::Instant::now();
        wg.connect_tcp(&at("echo.test", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert!(started.elapsed() >= Duration::from_millis(300));

        let unreachable = [
            // no IPv6 address in the tunnel
            TunnelDns::Server("[fd00::53]:53".parse().unwrap()),
            // no peer takes it
            TunnelDns::Server("192.0.2.53:53".parse().unwrap()),
        ];
        let (_peer, mut wg) = with_tunnel_dns(&["10.0.0.1"], &unreachable, |_| {}).await;
        wg.dns_wait = Duration::from_secs(5);
        let started = std::time::Instant::now();
        wg.connect_tcp(&at("echo.test", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert!(started.elapsed() < Duration::from_secs(2), "no waiting");
    }

    /// `system` asks this machine where it stands in the list; an answer
    /// without addresses ends the search all the same.
    #[tokio::test]
    async fn system_asks_this_machine_and_an_empty_answer_is_final() {
        let (peer, wg) = with_tunnel_dns(&[], &[TunnelDns::System], |_| {}).await;
        wg.connect_tcp(&at("local.test", ECHO_PORT), &within(5))
            .await
            .expect("resolved on this machine");
        assert!(peer.core().dns_questions.is_empty());

        let (peer, wg) = with_tunnel_dns(&[], &[], |s| s.dns_servers.push(TunnelDns::System)).await;
        let e = wg
            .connect_tcp(&at("local.test", ECHO_PORT), &within(5))
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            "dns: wireguard: dns lookup of local.test failed"
        );
        assert_eq!(peer.core().dns_questions, ["local.test A"]);
    }

    /// The endpoint of the only peer cannot be resolved: the dial fails, and
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto-wireguard`
Expected: FAIL，编译错误（节选）——出站还不认识 `dns-server`：

```text
error[E0412]: cannot find type `TunnelDns` in this scope
   --> crates\rurge-proto-wireguard\src\outbound.rs:507:19
error[E0433]: failed to resolve: use of undeclared type `TunnelDns`
   --> crates\rurge-proto-wireguard\src\outbound.rs:516:22
error[E0609]: no field `dns_wait` on type `outbound::WireGuardOutbound`
   --> crates\rurge-proto-wireguard\src\outbound.rs:577:12
error[E0609]: no field `dns_wait` on type `outbound::WireGuardOutbound`
   --> crates\rurge-proto-wireguard\src\outbound.rs:591:12
error: could not compile `rurge-proto-wireguard` (lib test) due to 9 previous errors
```

- [ ] **Step 3: 实现**

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
use smoltcp::socket::{AnySocket, tcp};
```

换成

```rust
use smoltcp::socket::{AnySocket, tcp, udp};
```

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
    /// A TCP connection to `to`, from the tunnel's address of its family.
    pub fn connect(&mut self, to: SocketAddr) -> Result<SocketHandle, Refusal> {
        let local = match to.ip() {
```

换成

```rust
    /// The tunnel's address of `to`'s family, when some peer takes `to`.
    fn source_for(&self, to: IpAddr) -> Result<IpAddr, Refusal> {
        let local = match to {
```

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
        .ok_or(Refusal::NoAddress(to.ip()))?;
        if self.routes.lookup(to.ip()).is_none() {
            return Err(Refusal::NoRoute(to.ip()));
        }
```

换成

```rust
        .ok_or(Refusal::NoAddress(to))?;
        if self.routes.lookup(to).is_none() {
            return Err(Refusal::NoRoute(to));
        }
        Ok(local)
    }

    /// A TCP connection to `to`, from the tunnel's address of its family.
    pub fn connect(&mut self, to: SocketAddr) -> Result<SocketHandle, Refusal> {
        let local = self.source_for(to.ip())?;
```

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
                tcp::Socket::downcast(s)
                    .and_then(tcp::Socket::local_endpoint)
                    .is_some_and(|e| e.port == port)
```

换成

```rust
                let tcp = tcp::Socket::downcast(s)
                    .and_then(tcp::Socket::local_endpoint)
                    .map(|e| e.port);
                let udp = udp::Socket::downcast(s).map(|u| u.endpoint().port);
                tcp.or(udp) == Some(port)
```

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
        None
    }
```

换成

```rust
        None
    }

    /// A UDP socket on the tunnel's address of `to`'s family, for an
    /// exchange with `to`.
    pub fn udp_open(&mut self, to: IpAddr) -> Result<SocketHandle, Refusal> {
        let local = self.source_for(to)?;
        let port = self.free_port().ok_or(Refusal::NoPort)?;
        let buffer = || udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 4], vec![0; 4096]);
        let mut socket = udp::Socket::new(buffer(), buffer());
        socket
            .bind(SocketAddr::new(local, port))
            .map_err(|_| Refusal::Unaddressable(to))?;
        Ok(self.sockets.add(socket))
    }

    /// The UDP socket of `handle`.
    pub fn udp(&mut self, handle: SocketHandle) -> &mut udp::Socket<'static> {
        self.sockets.get_mut(handle)
    }

    /// Forgets the UDP socket of `handle`.
    pub fn udp_close(&mut self, handle: SocketHandle) {
        self.sockets.remove(handle);
    }
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
use rurge_proto::OutboundError;
```

换成

```rust
use rurge_proto::OutboundError;
use smoltcp::iface::SocketHandle;
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
        Ok(stream)
    }
}
```

换成

```rust
        Ok(stream)
    }

    /// One exchange with `server` over UDP through the tunnel: `message`
    /// out, then the first datagram from `server` that `accept` takes,
    /// within `wait`. `None` when none came, or no peer takes `server`.
    pub(crate) async fn query<T>(
        self: &Arc<Device>,
        server: SocketAddr,
        message: &[u8],
        wait: Duration,
        accept: impl Fn(&[u8]) -> Option<T>,
    ) -> Option<T> {
        let socket = {
            let mut stack = self.shared.stack.lock().expect("the tunnel");
            let handle = stack.udp_open(server.ip()).ok()?;
            let socket = UdpExchange {
                device: self.clone(),
                handle,
            };
            stack.udp(handle).send_slice(message, server).ok()?;
            socket
        };
        self.shared.kick();
        let answer = poll_fn(|cx| {
            let mut stack = self.shared.stack.lock().expect("the tunnel");
            let udp = stack.udp(socket.handle);
            while let Ok((datagram, meta)) = udp.recv() {
                let from = SocketAddr::new(meta.endpoint.addr.into(), meta.endpoint.port);
                if from == server
                    && let Some(answer) = accept(datagram)
                {
                    return Poll::Ready(answer);
                }
            }
            udp.register_recv_waker(cx.waker());
            Poll::Pending
        });
        tokio::time::timeout(wait, answer).await.ok()
    }
}

/// The UDP socket of one exchange: gone with the exchange.
struct UdpExchange {
    device: Arc<Device>,
    handle: SocketHandle,
}

impl Drop for UdpExchange {
    fn drop(&mut self) {
        if let Ok(mut stack) = self.device.shared.stack.lock() {
            stack.udp_close(self.handle);
        }
    }
}
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
use crate::device::Device;
```

换成

```rust
use crate::device::Device;
use crate::dns::{self, Cache, Family};
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
use rurge_config::wireguard::WireGuardSection;
```

换成

```rust
use rurge_config::wireguard::{TunnelDns, WireGuardSection};
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
use std::sync::Arc;
use tokio::sync::Mutex;
```

换成

```rust
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// How long one `dns-server` has to answer before the next is asked.
const DNS_WAIT: Duration = Duration::from_secs(2);
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
    device: Mutex<Option<Arc<Device>>>,
```

换成

```rust
    device: Mutex<Option<Arc<Device>>>,
    /// What the tunnel's `dns-server`s answered, for their TTL.
    cache: StdMutex<Cache>,
    /// `DNS_WAIT` (shorter in the tests).
    dns_wait: Duration,
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
            device: Mutex::new(None),
```

换成

```rust
            device: Mutex::new(None),
            cache: StdMutex::new(Cache::default()),
            dns_wait: DNS_WAIT,
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
    /// Where a connection to `target` goes: its address, or its name
    /// resolved on this machine (M4-D9).
    async fn address(&self, target: &Target) -> Result<IpAddr, OutboundError> {
```

换成

```rust
    /// Where a connection to `target` goes: its address, or its name's.
    async fn address(
        &self,
        target: &Target,
        device: &Arc<Device>,
    ) -> Result<IpAddr, OutboundError> {
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
        let addrs =
            self.resolver.resolve(name).await.map_err(|_| {
                OutboundError::Dns(format!("wireguard: dns lookup of {name} failed"))
            })?;
        pick(&addrs, &self.section).map_err(|refusal| OutboundError::Proxy(refusal.to_string()))
```

换成

```rust
        let addrs = self
            .lookup(name, device)
            .await
            .ok_or_else(|| OutboundError::Dns(format!("wireguard: dns lookup of {name} failed")))?;
        pick(&addrs, &self.section).map_err(|refusal| OutboundError::Proxy(refusal.to_string()))
    }

    /// `name`'s addresses: from the section's `dns-server`s in order through
    /// the tunnel — the first that answers ends the search, `system` asks
    /// this machine — or, without any, from this machine (M4-D9).
    async fn lookup(&self, name: &str, device: &Arc<Device>) -> Option<Vec<IpAddr>> {
        if self.section.dns_servers.is_empty() {
            return self.resolver.resolve(name).await.ok();
        }
        if let Some(addrs) = self
            .cache
            .lock()
            .expect("the cache")
            .get(name, Instant::now())
        {
            return Some(addrs);
        }
        for server in &self.section.dns_servers {
            let server = match server {
                TunnelDns::System => match self.resolver.resolve(name).await {
                    Ok(addrs) => return Some(addrs),
                    Err(_) => continue,
                },
                TunnelDns::Server(server) => *server,
            };
            let Some(answer) = self.ask(device, server, name).await else {
                continue;
            };
            let ttl = answer.iter().map(|(_, ttl)| *ttl).min().unwrap_or(0);
            let addrs: Vec<IpAddr> = answer.into_iter().map(|(ip, _)| ip).collect();
            self.cache.lock().expect("the cache").put(
                name,
                addrs.clone(),
                Duration::from_secs(ttl.into()),
                Instant::now(),
            );
            return (!addrs.is_empty()).then_some(addrs);
        }
        None
    }

    /// `server`'s answer for `name`: A and AAAA at once, for the families
    /// the tunnel has an address of. `None` when it gave none within
    /// `dns_wait`, or no peer takes it.
    async fn ask(
        &self,
        device: &Arc<Device>,
        server: SocketAddr,
        name: &str,
    ) -> Option<Vec<(IpAddr, u32)>> {
        let ask = |family: Family, wanted: bool| async move {
            if !wanted {
                return None;
            }
            let id = getrandom::u32().unwrap_or(0) as u16;
            let question = dns::question(id, name, family)?;
            device
                .query(server, &question, self.dns_wait, |reply| {
                    dns::answer(reply, id, name, family)
                })
                .await
        };
        let (v4, v6) = tokio::join!(
            ask(Family::V4, self.section.self_ip.is_some()),
            ask(Family::V6, self.section.self_ip_v6.is_some())
        );
        if v4.is_none() && v6.is_none() {
            return None;
        }
        Some(v4.into_iter().chain(v6).flatten().collect())
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
        let ip = self.address(target).await?;
```

换成

```rust
        let ip = self.address(target, &device).await?;
```

要点：
- `Stack::udp_open` 与 `connect` 共用 `source_for`：服务器的地址族没有本端地址、或没有 peer 覆盖它时，这个服务器直接跳过、不等（`the_next_dns_server_is_asked_when_one_cannot_answer` 的后半）。本端端口的分配改为顾及 UDP 套接字。
- `Device::query` 发一个查询、等第一个来自那个服务器且 `accept` 认可的回答，最多 `wait`；套接字随这次查询关闭（`UdpExchange`）。
- `lookup`：没有 `dns-server` → 本机解析（M4-D9）；有 → 先查缓存，再按列表顺序：`system` 用本机解析器，服务器用 `ask`（A 与 AAAA 同时）；第一个作答的为准，空回答同样结束查找（`dns: wireguard: dns lookup of <名字> failed`）；只缓存有地址的回答（P14）。
- 对端的名字服务器在 `10.0.0.53:53`，按 `PeerOpts.dns` 作答、记下每个问题：用例据此断言"问了什么、问了几次"（缓存命中时不再问）。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto-wireguard` → 33 passed（新增 `dns::tests` 4 条与 `outbound::tests` 4 条：经隧道解析并缓存、两族同时问与 `prefer-ipv6`、不作答的服务器让给下一个而够不着的直接跳过、`system` 与空回答结束查找）。

- [ ] **Step 5: 门禁与提交**

跑门禁（47 个测试二进制，1098 通过 / 1 忽略）。

```bash
git add Cargo.toml crates/rurge-proto-wireguard
git commit -m "feat(proto-wireguard): 隧道内 DNS——按 dns-server 顺序经隧道问 A / AAAA、system 走本机、按 TTL 缓存"
```

### Task 6: 生命周期——重拨与 endpoint 跟随、网络变化入口、握手日志、同一私钥与 peer 只留一条隧道

设计 6.5 的其余部分与第 9 节的握手日志：写成域名的 endpoint 与启动时没连上的 peer 每 5 分钟重拨，新载体去了别的地址就换上并立即握手（P12）；"网络已变化"的入口（阶段 2 没有探测器，P12）；握手完成与没有回应的日志（P13）。另外是写本计划时在重载用例里发现的问题（P10）：同一私钥连着同一 peer 的两条隧道会互相抢 peer 记住的地址——节相同的策略共用一条隧道；新配置的隧道启动时结束与它冲突的旧隧道，旧配置也抢不回来；私钥相同而 peer 不同的两个节互不影响。

**Files:**
- Modify: `crates/rurge-proto-wireguard/src/stack.rs`（`initiate_peer`；`receive` 返回是否完成了握手；`tick` 返回已到期的 peer；`abort_all`；内存用例的握手计数）
- Modify: `crates/rurge-proto-wireguard/src/device.rs`（`REDIAL`；隧道表 `TUNNELS` 与启动锁 `STARTING`；`Driver`：重拨、换载体、网络变化、握手日志；`Device::{network_changed, close, is_closed}`）
- Modify: `crates/rurge-proto-wireguard/src/outbound.rs`（代次、`redial`、`network_changed()`、设备已被结束时重新启动，与用例）
- Modify: `crates/rurge-proto-wireguard/src/testing/peer.rs`（`FakeWgPeer::clients`）

**Interfaces:**
- Consumes: Task 4 的 `Device`、`WireGuardOutbound`、`FakeWgPeer`；Task 2 的 `Datagram::peer_addr`。
- Produces:
  - `WireGuardOutbound::network_changed(&self)`：隧道在运行时为每个 peer 新拨载体、换上并重新握手；正在启动时什么也不做（新隧道本来就用新网络）
  - crate 内：`device::REDIAL: Duration`（300 秒）；`Device::start(policy: &str, section: &WireGuardSection, generation: u64, connector: &Arc<dyn Connector>, opts: &ConnectOpts, redial: Duration) -> Result<Arc<Device>, OutboundError>`；`Device::{network_changed(&self), is_closed(&self) -> bool}`；`WireGuardOutbound` 的字段 `generation: u64`、`redial: Duration`（用例里改短）
  - `Stack::{initiate_peer(&mut self, peer: usize, out: &mut Vec<Outgoing>), receive(...) -> bool, tick(&mut self, out) -> Vec<usize>, abort_all(&mut self)}`
  - `testing::FakeWgPeer::clients(&self) -> Vec<SocketAddr>`（客户端的消息来过的每个地址，按先后）

- [ ] **Step 1: 先写用例**

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
    use std::sync::Mutex as StdMutex;
```

换成

```rust
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
        assert!(wg.device.lock().await.is_none());
    }
}
```

换成

```rust
        assert!(wg.device.lock().await.is_none());
    }

    /// Two policies that name one section: one tunnel, one handshake.
    #[tokio::test]
    async fn policies_that_name_one_section_share_its_tunnel() {
        let (peer, a) = tunnel(PeerOpts::default(), |_| {}).await;
        let spec = WireGuardSpec {
            section: a.section.clone(),
        };
        let b = WireGuardOutbound::new("Other", &spec, no_names(), direct());
        for wg in [&a, &b] {
            let mut stream = wg
                .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
                .await
                .expect("a connection");
            assert_eq!(echo(&mut stream, b"ping").await, b"ping");
        }
        assert_eq!(peer.core().handshakes, 1);
        assert_eq!(peer.clients().len(), 1);
    }

    /// A reload that edits the section: the new tunnel ends the old one and
    /// its connections, and the old configuration does not take it back.
    #[tokio::test]
    async fn a_later_configuration_of_a_tunnel_takes_over() {
        let (peer, old) = tunnel(PeerOpts::default(), |_| {}).await;
        let mut stream = old
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut stream, b"old").await, b"old");
        let mut section = old.section.clone();
        section.mtu = 1400;
        let new = WireGuardOutbound::new("WG", &WireGuardSpec { section }, no_names(), direct());
        let mut fresh = new
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection through the new tunnel");
        assert_eq!(echo(&mut fresh, b"new").await, b"new");
        assert_eq!(peer.core().handshakes, 2);
        let mut buf = [0u8; 8];
        let read = tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buf))
            .await
            .expect("the old connection ends at once");
        assert!(read.is_err(), "{read:?}");
        let e = old
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            "wireguard: a newer configuration of this tunnel is in use"
        );
        assert_eq!(echo(&mut fresh, b"still").await, b"still");
    }

    /// One key at two peers is two tunnels, side by side.
    #[tokio::test]
    async fn one_key_at_two_peers_is_two_tunnels() {
        let (private, public) = keypair();
        let at_peer = |peer: &FakeWgPeer| {
            let mut s = section(
                private,
                Ipv4Addr::new(10, 9, 0, 2),
                &[(peer.public_key(), &["10.0.0.0/8"])],
            );
            s.peers[0].endpoint = endpoint(peer.addr());
            WireGuardOutbound::new("WG", &WireGuardSpec { section: s }, no_names(), direct())
        };
        let first = FakeWgPeer::start(public, PeerOpts::default()).await;
        let second = FakeWgPeer::start(public, PeerOpts::default()).await;
        let (a, b) = (at_peer(&first), at_peer(&second));
        let mut x = a
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        let mut y = b
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut y, b"y").await, b"y");
        assert_eq!(echo(&mut x, b"x").await, b"x");
    }

    /// Waits until `done` holds, 5 seconds at most.
    async fn until(done: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !done() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("in time");
    }

    /// Names to addresses that a test changes as it goes.
    #[derive(Default)]
    struct Table(StdMutex<Vec<(String, IpAddr)>>);

    impl Table {
        fn set(&self, name: &str, ip: IpAddr) {
            self.0.lock().unwrap().push((name.to_string(), ip));
        }
    }

    impl Resolve for Table {
        fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            let table = self.0.lock().unwrap();
            let found: Vec<IpAddr> = table
                .iter()
                .filter(|(name, _)| name == host)
                .map(|(_, ip)| *ip)
                .collect();
            Box::pin(std::future::ready(if found.is_empty() {
                Err(io::Error::new(io::ErrorKind::NotFound, "no such name"))
            } else {
                Ok(found)
            }))
        }
    }

    /// Dials through `inner` and counts the dials; once `moved` is set, the
    /// carriers say they go there, as if the endpoint's name had come to
    /// point elsewhere.
    struct Moving {
        inner: Arc<dyn Connector>,
        moved: Arc<StdMutex<Option<SocketAddr>>>,
        dials: Arc<AtomicUsize>,
    }

    struct MovingDatagram {
        inner: BoxedDatagram,
        moved: Option<SocketAddr>,
    }

    impl Connector for Moving {
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
            Box::pin(async move {
                self.dials.fetch_add(1, Ordering::SeqCst);
                let inner = self.inner.connect_udp(target, opts).await?;
                let moved = *self.moved.lock().unwrap();
                Ok(Box::new(MovingDatagram { inner, moved }) as BoxedDatagram)
            })
        }
    }

    impl Datagram for MovingDatagram {
        fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
            self.inner.poll_send(cx, buf)
        }

        fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            self.inner.poll_recv(cx, buf)
        }

        fn peer_addr(&self) -> Option<SocketAddr> {
            self.moved.or_else(|| self.inner.peer_addr())
        }
    }

    /// An endpoint written as a name is dialled again every `redial`: the
    /// same address keeps its carrier; another gets a new one, on which the
    /// peer is greeted at once.
    #[tokio::test]
    async fn an_endpoint_written_as_a_name_is_followed() {
        let names = Arc::new(Table::default());
        names.set("wg.test", ip("127.0.0.1"));
        let (moved, dials) = (Arc::new(StdMutex::new(None)), Arc::new(AtomicUsize::new(0)));
        let moving: Arc<dyn Connector> = Arc::new(Moving {
            inner: Arc::new(DirectConnector::new(names)),
            moved: moved.clone(),
            dials: dials.clone(),
        });
        let (peer, mut wg) = tunnel_with(
            PeerOpts::default(),
            |s| s.peers[0].endpoint.host = HostName::parse("wg.test"),
            no_names(),
            moving,
        )
        .await;
        wg.redial = Duration::from_millis(100);
        let mut stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        until(|| dials.load(Ordering::SeqCst) >= 3).await;
        assert_eq!(echo(&mut stream, b"same").await, b"same");
        assert_eq!(
            peer.clients().len(),
            1,
            "the same address keeps its carrier"
        );

        *moved.lock().unwrap() = Some("127.0.0.1:1".parse().unwrap());
        until(|| peer.clients().len() == 2).await;
        assert_eq!(echo(&mut stream, b"moved").await, b"moved");
        assert_eq!(peer.core().handshakes, 2, "greeted on the new carrier");
    }

    /// A network change: every carrier is dialled anew and each peer
    /// greeted again on its new one; the connections carry on.
    #[tokio::test]
    async fn a_network_change_gives_every_peer_a_new_carrier() {
        let (peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let mut stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut stream, b"before").await, b"before");
        wg.network_changed();
        until(|| peer.clients().len() == 2).await;
        assert_eq!(echo(&mut stream, b"after").await, b"after");
        assert_eq!(peer.core().handshakes, 2);
    }

    /// A peer that cannot be reached when the tunnel starts is dialled
    /// again every `redial`; the others carry on meanwhile.
    #[tokio::test]
    async fn a_peer_unreachable_at_the_start_is_dialled_again() {
        let (private, public) = keypair();
        let a = FakeWgPeer::start(public, PeerOpts::default()).await;
        let b = FakeWgPeer::start(public, PeerOpts::default()).await;
        let mut section = section(
            private,
            Ipv4Addr::new(10, 9, 0, 2),
            &[
                (a.public_key(), &["10.1.0.0/16"]),
                (b.public_key(), &["10.2.0.0/16"]),
            ],
        );
        section.peers[0].endpoint = endpoint(a.addr());
        section.peers[1].endpoint = PeerEndpoint {
            host: HostName::parse("b.test"),
            port: b.addr().port(),
        };
        let names = Arc::new(Table::default());
        let connector: Arc<dyn Connector> = Arc::new(DirectConnector::new(names.clone()));
        let mut wg =
            WireGuardOutbound::new("WG", &WireGuardSpec { section }, no_names(), connector);
        wg.redial = Duration::from_millis(100);
        let mut stream = wg
            .connect_tcp(&at("10.1.0.1", ECHO_PORT), &within(5))
            .await
            .expect("through the peer that can be reached");
        assert_eq!(echo(&mut stream, b"a").await, b"a");

        names.set("b.test", ip("127.0.0.1"));
        let mut stream = wg
            .connect_tcp(&at("10.2.0.1", ECHO_PORT), &within(5))
            .await
            .expect("through the other, once it can be reached");
        assert_eq!(echo(&mut stream, b"b").await, b"b");
        assert_eq!(b.core().accepted, 1);
    }
}
```

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
        rounds: usize,
```

换成

```rust
        rounds: usize,
        /// Handshakes the client saw complete.
        completed: usize,
```

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
                rounds: 0,
```

换成

```rust
                rounds: 0,
                completed: 0,
```

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
                    self.client.receive(peer, &mut datagram, now, &mut to_peers);
```

换成

```rust
                    if self.client.receive(peer, &mut datagram, now, &mut to_peers) {
                        self.completed += 1;
                    }
```

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
        assert_eq!(net.peers[0].handshakes, 1);
```

换成

```rust
        assert_eq!(net.peers[0].handshakes, 1);
        assert_eq!(net.completed, 1);
```

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
        assert!(net.client.peers[0].handshake.is_some());
```

换成

```rust
        assert!(net.client.peers[0].handshake.is_some());
        assert_eq!(net.completed, 1, "data completes no handshake");
```

`crates/rurge-proto-wireguard/src/testing/peer.rs`——把

```rust
    silent: Arc<AtomicBool>,
```

换成

```rust
    silent: Arc<AtomicBool>,
    clients: Arc<Mutex<Vec<SocketAddr>>>,
```

`crates/rurge-proto-wireguard/src/testing/peer.rs`——把

```rust
        let task = tokio::spawn(serve(socket, core.clone(), silent.clone()));
```

换成

```rust
        let clients = Arc::new(Mutex::new(Vec::new()));
        let task = tokio::spawn(serve(socket, core.clone(), silent.clone(), clients.clone()));
```

`crates/rurge-proto-wireguard/src/testing/peer.rs`——把

```rust
            silent,
```

换成

```rust
            silent,
            clients,
```

`crates/rurge-proto-wireguard/src/testing/peer.rs`——把

```rust
        self.core.lock().expect("the peer")
    }

    /// From now on it drops whatever arrives and sends nothing, as a peer
```

换成

```rust
        self.core.lock().expect("the peer")
    }

    /// Every address the client's messages came from, in order.
    pub fn clients(&self) -> Vec<SocketAddr> {
        self.clients.lock().expect("the clients").clone()
    }

    /// From now on it drops whatever arrives and sends nothing, as a peer
```

`crates/rurge-proto-wireguard/src/testing/peer.rs`——把

```rust
async fn serve(socket: UdpSocket, core: Arc<Mutex<PeerCore>>, silent: Arc<AtomicBool>) {
```

换成

```rust
async fn serve(
    socket: UdpSocket,
    core: Arc<Mutex<PeerCore>>,
    silent: Arc<AtomicBool>,
    clients: Arc<Mutex<Vec<SocketAddr>>>,
) {
```

`crates/rurge-proto-wireguard/src/testing/peer.rs`——把

```rust
                    client = Some(from);
```

换成

```rust
                    // it answers where the client last wrote from (roaming)
                    if client != Some(from) {
                        client = Some(from);
                        let mut clients = clients.lock().expect("the clients");
                        if !clients.contains(&from) {
                            clients.push(from);
                        }
                    }
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto-wireguard`
Expected: FAIL，编译错误——

```text
error[E0609]: no field `redial` on type `outbound::WireGuardOutbound`
   --> crates\rurge-proto-wireguard\src\outbound.rs:925:12
error[E0599]: no method named `network_changed` found for struct `outbound::WireGuardOutbound` in the current scope
   --> crates\rurge-proto-wireguard\src\outbound.rs:954:12
error[E0609]: no field `redial` on type `outbound::WireGuardOutbound`
   --> crates\rurge-proto-wireguard\src\outbound.rs:984:12
error[E0308]: mismatched types
   --> crates\rurge-proto-wireguard\src\stack.rs:537:24
error: could not compile `rurge-proto-wireguard` (lib test) due to 4 previous errors
```

- [ ] **Step 3: 实现**

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
    /// tunnel starts, a test asks, the network changed.
    pub fn initiate(&mut self, out: &mut Vec<Outgoing>) {
        for (i, peer) in self.peers.iter_mut().enumerate() {
            if let TunnResult::WriteToNetwork(message) = peer
                .tunnel
                .format_handshake_initiation(&mut self.scratch, true)
            {
                out.push(outgoing(i, message, peer.client_id));
            }
```

换成

```rust
    /// tunnel starts, a test asks.
    pub fn initiate(&mut self, out: &mut Vec<Outgoing>) {
        for peer in 0..self.peers.len() {
            self.initiate_peer(peer, out);
        }
    }

    /// A handshake with `peer`, whatever the state of its session: it has a
    /// new carrier.
    pub fn initiate_peer(&mut self, peer: usize, out: &mut Vec<Outgoing>) {
        let Some(p) = self.peers.get_mut(peer) else {
            return;
        };
        if let TunnResult::WriteToNetwork(message) = p
            .tunnel
            .format_handshake_initiation(&mut self.scratch, true)
        {
            out.push(outgoing(peer, message, p.client_id));
```

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
    /// What the carrier of `peer` received; the answers it needs go to `out`.
```

换成

```rust
    /// What the carrier of `peer` received; the answers it needs go to `out`.
    /// Whether it completed a handshake rurge started.
```

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
    ) {
        let Some(p) = self.peers.get_mut(peer) else {
            return;
```

换成

```rust
    ) -> bool {
        let Some(p) = self.peers.get_mut(peer) else {
            return false;
```

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
                    out.push(outgoing(peer, message, p.client_id));
                }
                return;
```

换成

```rust
                    out.push(outgoing(peer, message, p.client_id));
                }
                return response;
```

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
            TunnResult::Done => return,
            TunnResult::Err(e) => {
                tracing::trace!(peer, error = ?e, "wireguard: a message was dropped");
                return;
```

换成

```rust
            TunnResult::Done => return false,
            TunnResult::Err(e) => {
                tracing::trace!(peer, error = ?e, "wireguard: a message was dropped");
                return false;
```

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
            tracing::trace!(peer, %src, "wireguard: a packet from outside the peer's allowed-ips was dropped");
        }
    }
```

换成

```rust
            tracing::trace!(peer, %src, "wireguard: a packet from outside the peer's allowed-ips was dropped");
        }
        false
    }
```

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
    /// second or so.
    pub fn tick(&mut self, out: &mut Vec<Outgoing>) {
```

换成

```rust
    /// second or so. The peers whose session has run out, or whose
    /// handshake went unanswered for 90 seconds: the next packet to one
    /// starts another.
    pub fn tick(&mut self, out: &mut Vec<Outgoing>) -> Vec<usize> {
        let mut expired = Vec::new();
```

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
                // an idle session ran out: the next packet starts another
                TunnResult::Err(WireGuardError::ConnectionExpired) => {}
```

换成

```rust
                TunnResult::Err(WireGuardError::ConnectionExpired) => expired.push(i),
```

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
                _ => {}
            }
        }
    }

    /// The tunnel's address of `to`'s family, when some peer takes `to`.
```

换成

```rust
                _ => {}
            }
        }
        expired
    }

    /// The tunnel's address of `to`'s family, when some peer takes `to`.
```

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
        self.released.push((handle, now));
```

换成

```rust
        self.released.push((handle, now));
    }

    /// Resets every TCP connection at once, without a word to the far end:
    /// the tunnel is ending. Whoever waits on one is woken.
    pub fn abort_all(&mut self) {
        for (_, socket) in self.sockets.iter_mut() {
            if let Some(tcp) = tcp::Socket::downcast_mut(socket) {
                tcp.abort();
            }
        }
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
use std::sync::{Arc, Mutex};
```

换成

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
const BATCH: usize = 256;
```

换成

```rust
const BATCH: usize = 256;
/// How often the endpoints written as names, and the peers that could not
/// be reached, are dialled again (M4 design 6.5).
pub(crate) const REDIAL: Duration = Duration::from_secs(300);

/// The tunnels of this process and their sections. Two tunnels with one
/// private key at one peer would take each other's packets — a peer answers
/// wherever the key last wrote from — so the policies that name a section
/// share its tunnel, and a tunnel that starts ends the one of an earlier
/// configuration: its key with a peer in common.
static TUNNELS: Mutex<Vec<(WireGuardSection, Weak<Device>)>> = Mutex::new(Vec::new());
/// Tunnels start one at a time.
static STARTING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Whether a tunnel of `a` and one of `b` would take each other's packets.
fn conflict(a: &WireGuardSection, b: &WireGuardSection) -> bool {
    a.private_key == b.private_key
        && a.peers
            .iter()
            .any(|p| b.peers.iter().any(|q| q.public_key == p.public_key))
}
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
    kick: Notify,
```

换成

```rust
    kick: Notify,
    /// Set by `Device::network_changed`, taken by the task.
    network_changed: AtomicBool,
    /// A tunnel of a later configuration took over.
    closed: AtomicBool,
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
/// The tunnel while it runs: as long as the outbound or a connection
```

换成

```rust
/// The tunnel while it runs: as long as an outbound or a connection
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
    _task: AbortOnDropHandle<()>,
```

换成

```rust
    task: AbortOnDropHandle<()>,
    /// The `WireGuardOutbound::generation` that started it.
    generation: u64,
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
    /// A carrier to every peer, then the task and a handshake with each
    /// peer. Fails only when no peer can be reached at all: one that cannot
    /// is left without a carrier.
```

换成

```rust
    /// The tunnel of `section`: the one running, when a policy naming the
    /// section started it; else a carrier to every peer, then the task and a
    /// handshake with each peer. Fails when no peer can be reached at all —
    /// one that cannot is dialled again every `redial` — and when a tunnel
    /// of a later configuration (`generation`) has taken over.
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
        connector: &Arc<dyn Connector>,
        opts: &ConnectOpts,
    ) -> Result<Arc<Device>, OutboundError> {
        let mut dials = JoinSet::new();
        for (i, peer) in section.peers.iter().enumerate() {
            let endpoint = Target::new(peer.endpoint.host.clone(), peer.endpoint.port);
            let (connector, opts) = (connector.clone(), opts.clone());
```

换成

```rust
        generation: u64,
        connector: &Arc<dyn Connector>,
        opts: &ConnectOpts,
        redial: Duration,
    ) -> Result<Arc<Device>, OutboundError> {
        let _one_at_a_time = STARTING.lock().await;
        {
            let mut tunnels = TUNNELS.lock().expect("the tunnels");
            tunnels.retain(|(_, device)| device.upgrade().is_some_and(|d| !d.is_closed()));
            for (other, device) in tunnels.iter() {
                let Some(device) = device.upgrade() else {
                    continue;
                };
                if other == section {
                    return Ok(device);
                }
                if conflict(other, section) && device.generation > generation {
                    return Err(OutboundError::Proxy(
                        "wireguard: a newer configuration of this tunnel is in use".to_string(),
                    ));
                }
            }
        }
        let endpoints: Vec<Target> = section
            .peers
            .iter()
            .map(|p| Target::new(p.endpoint.host.clone(), p.endpoint.port))
            .collect();
        let mut dials = JoinSet::new();
        for (i, endpoint) in endpoints.iter().enumerate() {
            let (connector, endpoint, opts) = (connector.clone(), endpoint.clone(), opts.clone());
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
            })));
        }
        let shared = Arc::new(Shared {
```

换成

```rust
            })));
        }
        // the tunnel it replaces goes quiet before a peer hears of this one
        TUNNELS
            .lock()
            .expect("the tunnels")
            .retain(|(other, device)| {
                let replaced = conflict(other, section);
                if replaced && let Some(device) = device.upgrade() {
                    device.close();
                }
                !replaced
            });
        let shared = Arc::new(Shared {
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
            kick: Notify::new(),
```

换成

```rust
            kick: Notify::new(),
            network_changed: AtomicBool::new(false),
            closed: AtomicBool::new(false),
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
        let task = tokio::spawn(run(shared.clone(), carriers, out));
        Ok(Arc::new(Device {
            shared,
            _task: AbortOnDropHandle::new(task),
        }))
```

换成

```rust
        let peers = carriers.len();
        let driver = Driver {
            shared: shared.clone(),
            policy: policy.to_string(),
            carriers,
            endpoints,
            connector: connector.clone(),
            redial,
            waiting: vec![false; peers],
            up: vec![false; peers],
        };
        let task = tokio::spawn(driver.run(out));
        let device = Arc::new(Device {
            shared,
            task: AbortOnDropHandle::new(task),
            generation,
        });
        TUNNELS
            .lock()
            .expect("the tunnels")
            .push((section.clone(), Arc::downgrade(&device)));
        Ok(device)
    }

    /// A tunnel of a later configuration took over: nothing more goes out,
    /// and every connection through this one fails.
    fn close(&self) {
        self.shared.closed.store(true, Ordering::SeqCst);
        self.task.abort();
        if let Ok(mut stack) = self.shared.stack.lock() {
            stack.abort_all();
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::SeqCst)
    }

    /// The network changed: every carrier is dialled anew and each peer
    /// greeted again on its new one (M4 design 6.5).
    pub(crate) fn network_changed(&self) {
        self.shared.network_changed.store(true, Ordering::SeqCst);
        self.shared.kick();
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
async fn send(carriers: &mut [Option<Carrier>], message: Outgoing) {
    let Some(Some(carrier)) = carriers.get_mut(message.peer) else {
        // a peer without a carrier: nothing reaches it
        return;
    };
```

换成

```rust
async fn send(carrier: &mut Carrier, message: &Outgoing) {
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
async fn run(shared: Arc<Shared>, mut carriers: Vec<Option<Carrier>>, mut out: Vec<Outgoing>) {
    let mut buf = vec![0u8; 65536];
    let mut next_tick = Instant::now() + TICK;
    let mut first = 0;
    loop {
        let deadline = {
            let mut stack = shared.stack.lock().expect("the tunnel");
            let now = Instant::now();
            if now >= next_tick {
                stack.tick(&mut out);
                next_tick = now + TICK;
            }
            let wait = stack.advance(now, &mut out);
            wait.map_or(next_tick, |wait| (now + wait).min(next_tick))
        };
        for message in out.drain(..) {
            send(&mut carriers, message).await;
        }
        tokio::select! {
            _ = shared.kick.notified() => {}
            (peer, received) = recv_any(&carriers, &mut buf, &mut first) => {
                let mut stack = shared.stack.lock().expect("the tunnel");
                let mut arrived = Some((peer, received));
                let mut taken = 0;
                while let Some((peer, received)) = arrived {
                    match received {
                        Ok(n) => stack.receive(peer, &mut buf[..n], Instant::now(), &mut out),
                        // an ICMP error the system reports on a connected socket
                        Err(e) => tracing::trace!(peer, error = %e, "wireguard: a carrier failed to receive"),
                    }
                    taken += 1;
                    // what else has arrived goes in before the stack runs
                    arrived = if taken < BATCH {
                        ready(&carriers, &mut buf, &mut first)
                    } else {
                        None
                    };
                }
            }
            _ = tokio::time::sleep_until(deadline.into()) => {}
```

换成

```rust
/// A dial of `peer`'s carrier: whether it replaces the one there in any
/// case, and what came of it.
type Dialled = (usize, bool, io::Result<BoxedDatagram>);

/// What the task holds besides the stack.
struct Driver {
    shared: Arc<Shared>,
    policy: String,
    carriers: Vec<Option<Carrier>>,
    endpoints: Vec<Target>,
    connector: Arc<dyn Connector>,
    redial: Duration,
    /// A handshake initiation went to the peer and nothing answered yet.
    waiting: Vec<bool>,
    /// A handshake with the peer completed, and it has not failed to answer
    /// one since.
    up: Vec<bool>,
}

impl Driver {
    /// Dials the carriers of `peers` side by side; `anew`: the new carriers
    /// replace the old whatever they go to.
    fn dial(&self, dials: &mut JoinSet<Dialled>, peers: Vec<usize>, anew: bool) {
        for peer in peers {
            let (connector, endpoint) = (self.connector.clone(), self.endpoints[peer].clone());
            dials.spawn(async move {
                let opts = ConnectOpts::default();
                (peer, anew, connector.connect_udp(&endpoint, &opts).await)
            });
        }
    }

    /// A dialled carrier of `peer` takes the place of the one there when
    /// there is none, when it goes to another address, or when `anew`
    /// says so; the peer is greeted on it at once.
    fn land(
        &mut self,
        peer: usize,
        anew: bool,
        dialled: io::Result<BoxedDatagram>,
        out: &mut Vec<Outgoing>,
    ) {
        let datagram = match dialled {
            Ok(datagram) => datagram,
            Err(e) => {
                tracing::debug!(policy = %self.policy, peer = peer + 1, error = %e, "wireguard: the peer cannot be reached");
                return;
            }
        };
        if let Some(carrier) = &self.carriers[peer]
            && !anew
        {
            if carrier.datagram.peer_addr() == datagram.peer_addr() {
                return;
            }
            tracing::info!(policy = %self.policy, peer = peer + 1, "wireguard: the peer's endpoint moved");
        }
        self.carriers[peer] = Some(Carrier {
            datagram,
            marks: true,
        });
        self.shared
            .stack
            .lock()
            .expect("the tunnel")
            .initiate_peer(peer, out);
    }

    /// What goes out now, to the peers that have a carrier.
    async fn send_all(&mut self, out: &mut Vec<Outgoing>) {
        for message in out.drain(..) {
            let Some(Some(carrier)) = self.carriers.get_mut(message.peer) else {
                continue;
            };
            if wire::message_type(&message.datagram) == Some(wire::HANDSHAKE_INITIATION) {
                self.waiting[message.peer] = true;
            }
            send(carrier, &message).await;
        }
    }

    async fn run(mut self, mut out: Vec<Outgoing>) {
        let mut buf = vec![0u8; 65536];
        let mut next_tick = Instant::now() + TICK;
        let mut next_redial = Instant::now() + self.redial;
        let mut first = 0;
        let mut dials = JoinSet::new();
        loop {
            let now = Instant::now();
            if self.shared.network_changed.swap(false, Ordering::SeqCst) {
                dials.abort_all();
                self.dial(&mut dials, (0..self.endpoints.len()).collect(), true);
                next_redial = now + self.redial;
            } else if now >= next_redial {
                let due = (0..self.endpoints.len())
                    .filter(|&p| {
                        self.carriers[p].is_none() || self.endpoints[p].host.as_domain().is_some()
                    })
                    .collect();
                self.dial(&mut dials, due, false);
                next_redial = now + self.redial;
            }
            let deadline = {
                let mut stack = self.shared.stack.lock().expect("the tunnel");
                if now >= next_tick {
                    for peer in stack.tick(&mut out) {
                        // once for each handshake that went unanswered
                        if std::mem::take(&mut self.waiting[peer]) {
                            self.up[peer] = false;
                            tracing::warn!(policy = %self.policy, peer = peer + 1, "wireguard: the peer did not answer the handshake");
                        }
                    }
                    next_tick = now + TICK;
                }
                let wait = stack.advance(now, &mut out);
                let next = next_tick.min(next_redial);
                wait.map_or(next, |wait| (now + wait).min(next))
            };
            self.send_all(&mut out).await;
            tokio::select! {
                _ = self.shared.kick.notified() => {}
                (peer, received) = recv_any(&self.carriers, &mut buf, &mut first) => {
                    let mut stack = self.shared.stack.lock().expect("the tunnel");
                    let mut completed = Vec::new();
                    let mut arrived = Some((peer, received));
                    let mut taken = 0;
                    while let Some((peer, received)) = arrived {
                        match received {
                            Ok(n) => {
                                if stack.receive(peer, &mut buf[..n], Instant::now(), &mut out) {
                                    completed.push(peer);
                                }
                            }
                            // an ICMP error the system reports on a connected socket
                            Err(e) => tracing::trace!(peer, error = %e, "wireguard: a carrier failed to receive"),
                        }
                        taken += 1;
                        // what else has arrived goes in before the stack runs
                        arrived = if taken < BATCH {
                            ready(&self.carriers, &mut buf, &mut first)
                        } else {
                            None
                        };
                    }
                    drop(stack);
                    for peer in completed {
                        self.waiting[peer] = false;
                        if !std::mem::replace(&mut self.up[peer], true) {
                            tracing::info!(policy = %self.policy, peer = peer + 1, "wireguard: handshake completed");
                        }
                    }
                }
                Some(joined) = dials.join_next(), if !dials.is_empty() => {
                    if let Ok((peer, anew, dialled)) = joined {
                        self.land(peer, anew, dialled, &mut out);
                    }
                }
                _ = tokio::time::sleep_until(deadline.into()) => {}
            }
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
use crate::device::Device;
```

换成

```rust
use crate::device::{Device, REDIAL};
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
use std::net::{IpAddr, SocketAddr};
```

换成

```rust
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
const DNS_WAIT: Duration = Duration::from_secs(2);

pub struct WireGuardOutbound {
```

换成

```rust
const DNS_WAIT: Duration = Duration::from_secs(2);

/// The generation of the next outbound: a reload builds its outbounds after
/// the ones they replace.
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub struct WireGuardOutbound {
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
    section: WireGuardSection,
```

换成

```rust
    section: WireGuardSection,
    /// Later configurations have higher ones: a tunnel they start ends
    /// this one's, never the other way round.
    generation: u64,
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
    dns_wait: Duration,
```

换成

```rust
    dns_wait: Duration,
    /// `REDIAL` (shorter in the tests).
    redial: Duration,
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
            section: spec.section.clone(),
```

换成

```rust
            section: spec.section.clone(),
            generation: GENERATION.fetch_add(1, Ordering::Relaxed),
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
            dns_wait: DNS_WAIT,
```

换成

```rust
            dns_wait: DNS_WAIT,
            redial: REDIAL,
        }
    }

    /// The network changed: a running tunnel dials every carrier anew and
    /// greets the peers again (M4 design 6.5). Nothing detects it before
    /// phase 3; one that is starting uses the new network anyway.
    pub fn network_changed(&self) {
        if let Ok(slot) = self.device.try_lock()
            && let Some(device) = slot.as_ref()
        {
            device.network_changed();
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
        if let Some(device) = slot.as_ref() {
            return Ok(device.clone());
        }
        let device = Device::start(&self.name, &self.section, &self.connector, opts).await?;
```

换成

```rust
        if let Some(device) = slot.as_ref()
            && !device.is_closed()
        {
            return Ok(device.clone());
        }
        let device = Device::start(
            &self.name,
            &self.section,
            self.generation,
            &self.connector,
            opts,
            self.redial,
        )
        .await?;
```

要点：
- 设备任务改由 `Driver` 承载：除了载体，它还记着各 peer 的 endpoint、连接器、`redial`，以及每个 peer "发了握手还没回应"（`waiting`）与"握手成功过、之后没失败"（`up`）两个标志。
- 重拨（P12）：到了 `redial` 就为写成域名的 endpoint 与没有载体的 peer 各拨一条新载体（放进 `JoinSet`，不挡住主循环）；`land`：原来没有载体、`peer_addr` 不同或"网络已变化"时换上，并立即 `initiate_peer`——否则要等 boringtun 5 秒一次的重试（`a_peer_unreachable_at_the_start_is_dialled_again` 在不握手时就超时）；地址相同就丢掉新拨的。发往没有载体的 peer 的报文直接丢弃，也不算"发了握手"。
- 日志（P13）：`receive` 返回 `true` 时清 `waiting`，`up` 从假变真时记 `wireguard: handshake completed`；`tick` 返回已到期的 peer，其中 `waiting` 为真的记一次 `wireguard: the peer did not answer the handshake` 并把 `up` 置假（boringtun 到期后每次都返回到期，`waiting` 保证只记一次）。
- 隧道表（P10）：`STARTING` 让隧道逐个启动；`TUNNELS` 记着节与设备的弱引用。启动时：节相同且在运行的隧道直接共用；与它冲突（私钥相同且有共同的 peer 公钥）而代次更晚的隧道已在时拒绝启动；否则建好载体之后、发出第一个握手之前，结束冲突的旧隧道（`close`：中止任务、`abort_all` 重置它的全部连接——不发 RST，任务已停——并唤醒挂着的读写者），再登记自己。代次在 `WireGuardOutbound::new` 时从全局计数器领取：重载总是先构造新出站，再释放旧的；干构建也领代次，但从不启动隧道。
- 出站的设备槽里是已被结束的设备时，下一次拨号重新启动（代次更早的会得到"已有更新的配置"）。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto-wireguard` → 39 passed（新增 `outbound::tests` 6 条：节相同的策略共用隧道、更晚的配置接替（旧连接立即失败、旧配置抢不回来）、同一私钥连两个不同的 peer 是两条隧道、写成域名的 endpoint 跟随地址变化、网络变化给每个 peer 换载体、启动时连不上的 peer 被重拨；`stack::tests` 的回显用例多断言一次握手完成）。

- [ ] **Step 5: 门禁与提交**

跑门禁（47 个测试二进制，1104 通过 / 1 忽略）。

```bash
git add crates/rurge-proto-wireguard
git commit -m "feat(proto-wireguard): 生命周期——5 分钟重拨与 endpoint 跟随、网络变化入口、握手日志；同一私钥与 peer 只留一条隧道"
```

### Task 7: 接入——`ProtoSpec::WireGuard`、引擎工厂、`underlying-proxy` 的 REJECT、DNS 会话防环、端到端、吞吐基准、日志过滤

把 `wireguard` 策略接进配置与引擎（设计 8.2）：`SpecEnv` 带上主配置的节，`to_spec` 多一个 `wireguard` 分支；带 `underlying-proxy`（不是 `DIRECT`）时加载报 `W0029`（P17）；引擎工厂以 `WireGuardOutbound` 构建（本机解析用引擎的解析器，含 `[Host]`；载体经策略自己的连接器——`underlying-proxy` 时是链路的连接器，于是拨号得到 `Unsupported`）；拨号期的 `Unsupported` 在请求记录里写说明（P17）；DNS 会话防环认 endpoint 写成域名的 `wireguard`（P6）。重载按指纹复用出站：spec 带着节，节不变就沿用出站与隧道，改了就重建（旧隧道由 Task 6 的接替结束）。另外两件：吞吐基准（P19，忽略的用例，不作门禁）；bin 不输出 boringtun 自己的日志（P13）——从本任务起 bin 就能跑 `wireguard` 了（能力表在 Task 10 才翻转，之前加载时照旧有 `W0007`）。

**Files:**
- Modify: `crates/rurge-config/src/spec/mod.rs`（`SpecEnv.wireguard`、`ProtoSpec::WireGuard`、`to_spec` 分支与 `W0029`，与用例）、`src/config.rs`（`SpecEnv` 带上 `cfg.wireguard`）、`tests/snapshots/corpus__corpus__kitchen-sink.snap`（两条 `W0029`）
- Modify: `crates/rurge-policy/src/assemble.rs`（订阅行的 `SpecEnv` 同样带上主配置的节）
- Modify: `crates/rurge-proto/src/http.rs`、`src/socks5.rs`（用例里的 `SpecEnv`）
- Modify: `crates/rurge-engine/Cargo.toml`（`rurge-proto-wireguard`，dev 依赖开 `testing`）、`src/outbounds.rs`（工厂分支）、`src/engine.rs`（`named_server`、拨号期 `Unsupported` 的说明）
- Modify: `crates/rurge-engine/tests/common/mod.rs`（`Profile.sections`、导出 WireGuard 的测试辅助）、`tests/pipeline.rs`（DNS 会话防环用例）
- Create: `crates/rurge-engine/tests/outbounds_wireguard.rs`（端到端用例）
- Modify: `crates/rurge-proto-wireguard/src/testing/mod.rs`（`hex`、`section_text`）、`src/outbound.rs`（吞吐基准）
- Modify: `crates/rurge/src/cli/run.rs`（`quiet_dependencies`，与用例）

**Interfaces:**
- Consumes: Task 1 的 `read_wireguard`、`WireGuardSpec`、`Config.wireguard`；Task 4 ～ 6 的 `WireGuardOutbound`、`testing::{FakeWgPeer, PeerOpts, keypair, ECHO_PORT}`；既有的 `EngineFactory`、`socket_opener`、`bypass_to_direct`、`rurge_policy::Note`。
- Produces:
  - `rurge_config::spec::SpecEnv.wireguard: &'a [WireGuardSection]`；`ProtoSpec::WireGuard(WireGuardSpec)`（`tls()` 为 `None`）
  - `EngineFactory::build` 的 `ProtoSpec::WireGuard` 分支：`WireGuardOutbound::new(&spec.name, wireguard, self.resolver.clone(), connector)`
  - `testing::{hex(key: &[u8; 32]) -> String, section_text(name: &str, private: &[u8; 32], peer: &FakeWgPeer) -> String}`（`[WireGuard <name>]` 的文本：隧道地址 10.9.0.2，`peer` 覆盖 10.0.0.0/8）
  - 引擎用例的 `Profile.sections: &str`（整节文本，放在 `[Rule]` 之后）；`common` 导出 `ECHO_PORT`、`FakeWgPeer`、`PeerOpts`、`keypair`、`section_text`

- [ ] **Step 1: 先写用例**

端到端用例要引擎能依赖 `rurge-proto-wireguard`（dev 依赖开 `testing`），以及拼节文本的两个辅助函数：

`crates/rurge-engine/Cargo.toml`——把

```toml
rurge-proto-ssh.workspace = true
```

换成

```toml
rurge-proto-ssh.workspace = true
rurge-proto-wireguard.workspace = true
```

`crates/rurge-engine/Cargo.toml`——把

```toml
rurge-proto-ssh = { workspace = true, features = ["testing"] }
```

换成

```toml
rurge-proto-ssh = { workspace = true, features = ["testing"] }
rurge-proto-wireguard = { workspace = true, features = ["testing"] }
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
        port: addr.port(),
    }
}
```

换成

```rust
        port: addr.port(),
    }
}

/// `key` as the 64 hexadecimal digits a profile takes for a key.
pub fn hex(key: &[u8; 32]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

/// The profile text of section `[WireGuard <name>]` for the client key
/// `private`: tunnel address 10.9.0.2, and `peer` taking 10.0.0.0/8.
pub fn section_text(name: &str, private: &[u8; 32], peer: &FakeWgPeer) -> String {
    format!(
        "[WireGuard {name}]\nprivate-key = {}\nself-ip = 10.9.0.2\n\
peer = (public-key = {}, allowed-ips = 10.0.0.0/8, endpoint = {})\n",
        hex(private),
        hex(&peer.public_key()),
        peer.addr()
    )
}
```

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
};
pub use rurge_rules::{GeoUrls, OutboundMode};
```

换成

```rust
};
pub use rurge_proto_wireguard::testing::{ECHO_PORT, FakeWgPeer, PeerOpts, keypair, section_text};
pub use rurge_rules::{GeoUrls, OutboundMode};
```

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
    pub keystore: &'a str,
```

换成

```rust
    pub keystore: &'a str,
    /// Whole sections after `[Rule]`, such as `[WireGuard <name>]`.
    pub sections: &'a str,
```

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
[Proxy]\n{}\n[Proxy Group]\n{}\n[Host]\n{}\n[Keystore]\n{}\n[Rule]\n{}\nFINAL,DIRECT\n",
            self.general, self.proxies, self.groups, self.hosts, self.keystore, self.rules
```

换成

```rust
[Proxy]\n{}\n[Proxy Group]\n{}\n[Host]\n{}\n[Keystore]\n{}\n[Rule]\n{}\nFINAL,DIRECT\n{}",
            self.general,
            self.proxies,
            self.groups,
            self.hosts,
            self.keystore,
            self.rules,
            self.sections
```

新建 `crates/rurge-engine/tests/outbounds_wireguard.rs`：

```rust
//! Sessions that leave through the `wireguard` outbound: profile text →
//! Runtime → Engine → loopback listeners → `FakeWgPeer`, whose own stack
//! echoes on port 7 of every address in the tunnel (phase 2 M4 design §6).

mod common;

use common::*;
use rurge_config::HostName;
use rurge_config::session::SessionInfo;
use rurge_inbound::{DialError, Dialer};

/// A peer, and the section of policy `WG = wireguard, section-name=w` that
/// reaches it.
async fn peer() -> (FakeWgPeer, String) {
    let (private, public) = keypair();
    let peer = FakeWgPeer::start(public, PeerOpts::default()).await;
    let section = section_text("w", &private, &peer);
    (peer, section)
}

const WG: &str = "WG = wireguard, section-name=w";
const TO_WG: &str = "IP-CIDR,10.0.0.0/8,WG";

fn echo_addr() -> SocketAddr {
    SocketAddr::from(([10, 0, 0, 1], ECHO_PORT))
}

#[tokio::test]
async fn a_connect_leaves_through_the_tunnel() {
    let (peer, section) = peer().await;
    let h = harness(Profile {
        proxies: WG,
        rules: TO_WG,
        sections: &section,
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "10.0.0.1:7").await;
    echo_through(&mut tunnel, b"through the tunnel").await;
    assert_eq!(peer.core().connected_to, [echo_addr()]);
    drop(tunnel);
    let log = h.engine.request_log();
    wait_until("the session to finish", || !log.recent(10).is_empty()).await;
    assert_eq!(log.recent(10)[0].policy, ["WG"]);
}

/// Without a `dns-server`, a destination name is resolved on this machine,
/// `[Host]` included (M4-D9); the tunnel carries the address.
#[tokio::test]
async fn a_name_is_resolved_on_this_machine_with_host_items() {
    let (peer, section) = peer().await;
    let h = harness(Profile {
        proxies: WG,
        hosts: "echo.test = 10.0.0.1",
        rules: "DOMAIN,echo.test,WG",
        sections: &section,
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "echo.test:7").await;
    echo_through(&mut tunnel, b"by name").await;
    assert_eq!(peer.core().connected_to, [echo_addr()]);
    assert!(h.dns.queries().is_empty(), "[Host] answered");
}

/// No UDP through a chain before M5: the policy rejects and says why, and
/// never goes around the chain (M4-D7).
#[tokio::test]
async fn a_tunnel_over_underlying_proxy_rejects_with_a_note() {
    let (peer, section) = peer().await;
    let h = harness(Profile {
        proxies: "WG = wireguard, section-name=w, underlying-proxy=Up\nUp = socks5, 127.0.0.1, 9",
        rules: TO_WG,
        sections: &section,
        ..Profile::default()
    })
    .await;
    let session = SessionInfo::tcp(HostName::parse("10.0.0.1"), ECHO_PORT);
    match h.engine.dial(session).await {
        Err(DialError::Reject { kind, handle, .. }) => {
            assert_eq!(kind, rurge_proto::RejectKind::Reject);
            assert_eq!(
                handle.error().as_deref(),
                Some("policy protocol not implemented: wireguard over underlying-proxy")
            );
        }
        Err(DialError::Failed { message, .. }) => panic!("expected a reject, failed: {message}"),
        Ok(_) => panic!("expected a reject, got a stream"),
    }
    assert!(peer.clients().is_empty(), "nothing went to the peer");
}

/// A reload that leaves the line and its section alone keeps the tunnel,
/// handshake and all; an edited section builds the policy anew.
#[tokio::test]
async fn a_reload_keeps_the_tunnel_unless_its_section_changes() {
    let (peer, section) = peer().await;
    let h = harness(Profile {
        proxies: WG,
        rules: TO_WG,
        sections: &section,
        ..Profile::default()
    })
    .await;
    let mut first = connect_via_http(h.http(), "10.0.0.1:7").await;
    echo_through(&mut first, b"one").await;
    let before = outbound_now(&h, "WG");

    let proxies = format!("{WG}\nOther = http, other.example, 8080");
    let unrelated = Profile {
        proxies: &proxies,
        rules: TO_WG,
        sections: &section,
        ..Profile::default()
    }
    .text(h.dns.addr());
    h.engine
        .swap_runtime(runtime(h.dir.path(), &unrelated, h.engine.shared()).await);
    assert!(
        Arc::ptr_eq(&before, &outbound_now(&h, "WG")),
        "WG was rebuilt"
    );
    let mut second = connect_via_http(h.http(), "10.0.0.1:7").await;
    echo_through(&mut second, b"two").await;
    assert_eq!(peer.core().handshakes, 1, "the same tunnel");

    let edited = format!("{section}mtu = 1400\n");
    let next = Profile {
        proxies: &proxies,
        rules: TO_WG,
        sections: &edited,
        ..Profile::default()
    }
    .text(h.dns.addr());
    h.engine
        .swap_runtime(runtime(h.dir.path(), &next, h.engine.shared()).await);
    assert!(
        !Arc::ptr_eq(&before, &outbound_now(&h, "WG")),
        "WG was kept"
    );
    let mut third = connect_via_http(h.http(), "10.0.0.1:7").await;
    echo_through(&mut third, b"three").await;
    assert_eq!(peer.core().handshakes, 2, "a tunnel of its own");
}
```

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
        "the mock DNS server was actually queried"
    );
}

/// A DNS session does not wait for an `evaluate-before-use` group's first
```

换成

```rust
        "the mock DNS server was actually queried"
    );
}

/// The same for a `wireguard` policy whose peer's endpoint is a host name:
/// starting the tunnel would need the very lookup the session carries.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dns_session_bypasses_a_wireguard_policy_whose_endpoint_is_a_host_name() {
    use rurge_proto_wireguard::testing::{hex, keypair};
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    dns.set("wg.test", &["127.0.0.1"], &[], 60);
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\n\
encrypted-dns-follow-outbound-mode = true\nencrypted-dns-server = tcp://127.0.0.1:{}\nipv6 = false\n\
[Proxy]\nWG = wireguard, section-name=w\n[Proxy Group]\n[Rule]\nPROTOCOL,DNS,WG\nFINAL,DIRECT\n\
[WireGuard w]\nprivate-key = {}\nself-ip = 10.9.0.2\n\
peer = (public-key = {}, allowed-ips = 0.0.0.0/0, endpoint = wg.test:51820)\n",
        dns.addr().port(),
        hex(&keypair().0),
        hex(&keypair().1)
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = tokio::time::timeout(
        Duration::from_secs(5),
        engine
            .runtime()
            .stack
            .resolver
            .lookup("target.test", rurge_dns::resolver::LookupOpts::default()),
    )
    .await
    .expect("the lookup must not wait for the endpoint's own name to be resolved");
    assert!(res.is_ok(), "resolution through the pipeline: {res:?}");
    let internal = internal_sessions(&engine);
    assert!(
        internal.iter().any(|r| {
            r.error.as_deref()
                == Some(
                    "dns-follow: proxy configured by host name bypassed to avoid a resolution loop",
                )
                && r.policy.first().map(String::as_str) == Some("WG")
        }),
        "a bypassed internal DNS session: {internal:?}"
    );
}

/// A DNS session does not wait for an `evaluate-before-use` group's first
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    use crate::types::HostName;
```

换成

```rust
    use crate::types::HostName;
    use crate::wireguard::WireGuardSection;
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        };
        to_spec(
```

换成

```rust
        };
        let wireguard = [WireGuardSection {
            name: "home".into(),
            mtu: 1280,
            ..WireGuardSection::default()
        }];
        to_spec(
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
                keystore: &keystore,
```

换成

```rust
                keystore: &keystore,
                wireguard: &wireguard,
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        assert_eq!(ssh.idle_timeout, std::time::Duration::from_secs(60));
```

换成

```rust
        assert_eq!(ssh.idle_timeout, std::time::Duration::from_secs(60));
    }

    /// The spec carries the section the line names: an edited section is
    /// another spec, and a reload builds the policy anew (M4 design 4.3).
    #[test]
    fn a_wireguard_line_carries_its_section() {
        let o = outcome("W", "wireguard, section-name=home, ecn=on");
        let spec = o.spec.expect("a wireguard spec");
        let ProtoSpec::WireGuard(wg) = &spec.proto else {
            panic!("{:?}", spec.proto);
        };
        assert_eq!(wg.section.name, "home");
        assert_eq!((spec.server, spec.port), (None, None));
        assert_eq!(o.inert, ["ecn"]);
        let o = outcome("W", "wireguard, section-name=home, shadow-tls-password=pw");
        assert!(o.spec.is_none());
        assert_eq!(
            o.diagnostics[0].message,
            "policy `W`: Shadow TLS cannot be combined with a `wireguard` policy"
        );
    }

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

`crates/rurge/src/cli/run.rs`——把

```rust
    #[test]
    fn log_levels_map_like_surge() {
```

换成

```rust
    #[test]
    fn boringtun_says_nothing() {
        let filter = quiet_dependencies();
        let error = &tracing::Level::ERROR;
        assert!(!filter.would_enable("boringtun::noise::timers", error));
        assert!(filter.would_enable("rurge_proto_wireguard::device", &tracing::Level::TRACE));
    }

    #[test]
    fn log_levels_map_like_surge() {
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-engine --test outbounds_wireguard`
Expected: FAIL——`wireguard` 行还没有 spec，策略被当成未实现的协议 REJECT，CONNECT 直接被关掉（`rurge-config` 与 bin 的新用例此时编译不过，Step 3 之后才能跑）：

```text
test a_tunnel_over_underlying_proxy_rejects_with_a_note ... FAILED
test a_reload_keeps_the_tunnel_unless_its_section_changes ... FAILED
test a_name_is_resolved_on_this_machine_with_host_items ... FAILED
test a_connect_leaves_through_the_tunnel ... FAILED
thread 'a_tunnel_over_underlying_proxy_rejects_with_a_note' panicked at crates\rurge-engine\tests\outbounds_wireguard.rs:82:13:
assertion `left == right` failed
  left: Some("policy protocol not implemented: wireguard")
 right: Some("policy protocol not implemented: wireguard over underlying-proxy")
thread 'a_connect_leaves_through_the_tunnel' panicked at crates\rurge-engine\tests\common\mod.rs:175:9:
closed before the CONNECT response: ""
test result: FAILED. 0 passed; 4 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
```

- [ ] **Step 3: 实现**

`crates/rurge-config/src/spec/mod.rs`——把

```rust
use crate::types::HostName;
use common::{Notes, read_common};
```

换成

```rust
use crate::types::HostName;
use crate::wireguard::WireGuardSection;
use common::{Notes, read_common};
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    pub keystore: &'a [KeystoreItem],
```

换成

```rust
    pub keystore: &'a [KeystoreItem],
    /// The `[WireGuard <name>]` sections `section-name` may name.
    pub wireguard: &'a [WireGuardSection],
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    Ssh(SshSpec),
```

换成

```rust
    Ssh(SshSpec),
    WireGuard(WireGuardSpec),
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            ProtoSpec::Direct | ProtoSpec::Reject(_) | ProtoSpec::Ssh(_) => None,
```

换成

```rust
            ProtoSpec::Direct
            | ProtoSpec::Reject(_)
            | ProtoSpec::Ssh(_)
            | ProtoSpec::WireGuard(_) => None,
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            (common, ProtoSpec::Ssh(ssh))
        }
        _ => return SpecOutcome::default(),
```

换成

```rust
            (common, ProtoSpec::Ssh(ssh))
        }
        PolicyKind::WireGuard => {
            let mut common = read_common(&mut r, Applies::Proxy, &mut notes);
            let wireguard = wireguard::read_wireguard(&mut r, &mut common, env.wireguard);
            (common, ProtoSpec::WireGuard(wireguard))
        }
        _ => return SpecOutcome::default(),
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        check_underlying(&mut r, &mut common, env);
```

换成

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

`crates/rurge-config/src/config.rs`——把

```rust
            keystore: &cfg.keystore,
```

换成

```rust
            keystore: &cfg.keystore,
            wireguard: &cfg.wireguard,
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
            keystore: &cfg.keystore,
```

换成

```rust
            keystore: &cfg.keystore,
            wireguard: &cfg.wireguard,
```

`crates/rurge-proto/src/http.rs`——把

```rust
                keystore: &[],
```

换成

```rust
                keystore: &[],
                wireguard: &[],
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
                keystore: &[],
```

换成

```rust
                keystore: &[],
                wireguard: &[],
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
use rurge_proto_ssh::SshOutbound;
```

换成

```rust
use rurge_proto_ssh::SshOutbound;
use rurge_proto_wireguard::WireGuardOutbound;
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            )?),
        };
```

换成

```rust
            )?),
            // destination names without a `dns-server` are resolved here,
            // with `[Host]` (M4-D9)
            ProtoSpec::WireGuard(wireguard) => Arc::new(WireGuardOutbound::new(
                &spec.name,
                wireguard,
                self.resolver.clone(),
                connector,
            )),
        };
```

`crates/rurge-engine/src/engine.rs`——把

```rust
use rurge_config::spec::PolicySpec;
```

换成

```rust
use rurge_config::spec::{PolicySpec, ProtoSpec};
```

`crates/rurge-engine/src/engine.rs`——把

```rust
use rurge_policy::{PolicyRegistry, TerminalKind};
```

换成

```rust
use rurge_policy::{Note, PolicyRegistry, TerminalKind};
```

`crates/rurge-engine/src/engine.rs`——把

```rust
    None
}

/// The bypass every anti-loop arm of `dial_internal` takes (a REJECT, a
```

换成

```rust
    None
}

/// Whether reaching `spec`'s server takes a name lookup: the server, or the
/// endpoint of one of a `wireguard` policy's peers, is a host name.
fn named_server(spec: &PolicySpec) -> bool {
    match &spec.proto {
        ProtoSpec::WireGuard(wireguard) => wireguard
            .section
            .peers
            .iter()
            .any(|peer| peer.endpoint.host.as_domain().is_some()),
        _ => matches!(spec.server, Some(HostName::Domain(_))),
    }
}

/// The bypass every anti-loop arm of `dial_internal` takes (a REJECT, a
```

`crates/rurge-engine/src/engine.rs`——把

```rust
                Some(spec) if matches!(spec.server, Some(HostName::Domain(_))) => {
```

换成

```rust
                Some(spec) if named_server(spec) => {
```

`crates/rurge-engine/src/engine.rs`——把

```rust
                OutboundError::Unsupported(_) => reject(handle, rurge_proto::RejectKind::Reject),
```

换成

```rust
                OutboundError::Unsupported(what) => {
                    // what the outbound cannot do here (M4-D7): say so
                    handle.set_error(Note::Unsupported(what).to_string());
                    reject(handle, rurge_proto::RejectKind::Reject)
                }
```

`crates/rurge/src/cli/run.rs`——把

```rust
use tracing_subscriber::filter::LevelFilter;
```

换成

```rust
use tracing_subscriber::filter::{LevelFilter, Targets};
```

`crates/rurge/src/cli/run.rs`——把

```rust
type LevelHandle = reload::Handle<LevelFilter, Registry>;

fn init_logging(
```

换成

```rust
type LevelHandle = reload::Handle<LevelFilter, Registry>;

/// What rurge says better itself: boringtun's handshake and timer messages
/// carry no policy name, and the `wireguard` outbound logs its handshakes.
fn quiet_dependencies() -> Targets {
    Targets::new()
        .with_default(LevelFilter::TRACE)
        .with_target("boringtun", LevelFilter::OFF)
}

fn init_logging(
```

`crates/rurge/src/cli/run.rs`——把

```rust
        .with(level_layer)
```

换成

```rust
        .with(level_layer)
        .with(quiet_dependencies())
```

kitchen-sink 语料的 `WG` 行（`underlying-proxy=SS, test-url=http://example.com/, ecn=false`）现在有了 spec，多出两条 `W0029`：

`crates/rurge-config/tests/snapshots/corpus__corpus__kitchen-sink.snap`——把

```text
  - "warning[W0029] valid/kitchen-sink.conf:55: policy parameter `udp-relay` is parsed but has no effect in this version"
```

换成

```text
  - "warning[W0029] valid/kitchen-sink.conf:55: policy parameter `udp-relay` is parsed but has no effect in this version"
  - "warning[W0029] valid/kitchen-sink.conf:69: policy `WG`: `underlying-proxy` does not work with `wireguard` policies in this version; the policy rejects every connection"
  - "warning[W0029] valid/kitchen-sink.conf:69: policy parameter `ecn` is parsed but has no effect in this version"
```

要点：
- `W0029` 要在 `check_underlying` 之后判断：`underlying-proxy=DIRECT` 被它清掉，就不报（`DIRECT` 就是没有链）。
- 订阅行的 `SpecEnv` 同样给主配置的节：订阅行自己写的 `section-name=` 已被 Task 1 的安全门挡掉，只有 `external-policy-modifier` 设上的才会走到这里。
- 工厂分支用 `self.resolver`（引擎的解析器单元，含 `[Host]`；被复用的出站跟随新一代解析器，M2b）；干构建不开套接字，`WireGuardOutbound::new` 什么也不做，`rurge check` 照常。
- `named_server`：`wireguard` 看 peer 的 endpoint，别的协议照旧看 `server`；绕开时的说明与"以域名配置的代理"相同。
- 拨号期的 `OutboundError::Unsupported(what)` 以 `Note::Unsupported(what)` 的文本写进请求记录再 REJECT（此前只在解析期写）。目前只有 `wireguard` 会在拨号期返回它。
- `quiet_dependencies()` 是加在日志订阅上的一个 `Targets` 全局过滤：默认 `TRACE`（级别仍由可重载的 `LevelFilter` 决定），`boringtun` 为 `OFF`。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-engine --test outbounds_wireguard` → 4 passed（经 HTTP 入站 CONNECT 进隧道回显、`[Host]` 解析的目标、`underlying-proxy` 时 REJECT 附说明且什么也没发给 peer、重载时节不变沿用隧道（握手仍是 1 次）而改了节就换新的（握手 2 次））。
Run: `cargo test -p rurge-engine --test pipeline a_dns_session` → 8 passed（新增 endpoint 写成域名的 `wireguard` 被绕开）。
Run: `cargo test -p rurge-config` → 195 passed（新增 `spec::tests` 2 条）；`--test corpus` 2 passed。
Run: `cargo test -p rurge --bin rurge boringtun_says_nothing` → 1 passed。

- [ ] **Step 5: 吞吐基准（不作门禁）**

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
        assert!(wg.device.lock().await.is_none());
    }

    /// Two policies that name one section: one tunnel, one handshake.
```

换成

```rust
        assert!(wg.device.lock().await.is_none());
    }

    /// Not a gate (M4 design §10): how fast a bulk transfer goes through the
    /// tunnel to a loopback peer, whose own smoltcp echoes it back. In
    /// release: `cargo test -p rurge-proto-wireguard --release throughput --
    /// --ignored --nocapture`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn throughput() {
        const TOTAL: usize = 64 * 1024 * 1024;
        let (_peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        let (mut read, mut write) = tokio::io::split(stream);
        let started = std::time::Instant::now();
        let writer = tokio::spawn(async move {
            let chunk = vec![0x5a; 64 * 1024];
            for _ in 0..TOTAL / chunk.len() {
                write.write_all(&chunk).await.unwrap();
            }
            write
        });
        let mut buf = vec![0u8; 64 * 1024];
        let mut received = 0;
        while received < TOTAL {
            let n = read.read(&mut buf).await.unwrap();
            assert!(n > 0, "the echo ended early");
            received += n;
        }
        let elapsed = started.elapsed();
        let _write = writer.await.unwrap();
        println!(
            "{} MiB through the tunnel and back in {elapsed:.2?}: {:.1} MiB/s each way",
            TOTAL >> 20,
            (TOTAL >> 20) as f64 / elapsed.as_secs_f64()
        );
    }

    /// Two policies that name one section: one tunnel, one handshake.
```

Run: `cargo test -p rurge-proto-wireguard --release throughput -- --ignored --nocapture`
写本计划时（Windows 11，P19）：`64 MiB through the tunnel and back in 9.79s: 6.5 MiB/s each way`，三次（9.71 / 9.79 / 9.89 秒）一致。把实测数字写进本计划末尾的「执行期修正记录」；比这个慢一个数量级（例如 1 MiB/s 以下）说明 P2 / P3 / P5 / P9 里有一项没做到，先查它们。

- [ ] **Step 6: 门禁与提交**

跑门禁（48 个测试二进制，1112 通过 / 2 忽略——新多出的忽略就是吞吐基准）。

```bash
git add crates/rurge-config crates/rurge-policy/src/assemble.rs crates/rurge-proto crates/rurge-engine crates/rurge-proto-wireguard crates/rurge/src/cli/run.rs
git commit -m "feat(engine): 接入 wireguard——ProtoSpec::WireGuard、工厂分支、underlying-proxy 的 REJECT 与说明、DNS 会话防环；吞吐基准；bin 关掉 boringtun 的日志"
```

### Task 8: 测速——`TestMode`、原生握手测速、经隧道的 URL 测速

设计 6.7 与 8.1：`wireguard` 策略的节里没有 `dns-server`、策略也没写 `test-url` 时，测试是一次握手（原生模式）：向每个 peer 强制握手，从发起到第一个握手完成用了多久就是结果；否则是照常经出站的两次 `HEAD`。两种都在超时之外另加 10 秒（手册：第一次测试可能要先启动隧道）。实现上（P7）：`rurge-proto` 的 `Outbound` 多一个可选的 `native_test`；`rurge-policy` 的测试用例从"一个 URL"变成 `TestMode`（URL 或原生），注册表构建时静态决定每个策略用哪种；测试会话的目标在原生模式下是第一个 peer 的 endpoint。`FakeWgPeer` 加一个极小的 HTTP 服务（P20），用来证明经隧道的 URL 测试。

**Files:**
- Modify: `crates/rurge-proto/src/outbound.rs`（`Outbound::native_test`）
- Modify: `crates/rurge-policy/src/testbook.rs`（`TestMode`、`TestCase.mode`、`TestObserver::begin` 收 `&Target`、原生模式的测试，与用例）、`src/registry.rs`（`TestSpec` 的模式与 key、`WIREGUARD_START`，与用例）、`src/auto.rs`（用例里的 `TestCase`）、`src/testing.rs`（`FakeFactory` 能构建 `wireguard`）
- Modify: `crates/rurge-engine/src/auto.rs`（测试会话的目标、一次性 URL 测试）
- Modify: `crates/rurge-proto-wireguard/src/stack.rs`（`last_handshake`）、`src/device.rs`（`Device::handshake`：强制握手并等第一个完成）、`src/outbound.rs`（`native_test`，与用例）、`src/testing/mod.rs`（80 号端口的 HTTP 服务、`http_requests`）
- Modify: `crates/rurge-engine/tests/outbounds_wireguard.rs`（两条测速用例）

**Interfaces:**
- Consumes: Task 3 的 `Peer.handshake`（每个 peer 上一次由 rurge 发起的握手完成的时刻）；Task 6 的 `Driver`、`Shared`；Task 7 的工厂与 `Profile.sections`；既有的 `TestBook`、`probe`、`TestSpec`、`test_slot`、`TestSessions`。
- Produces:
  - `rurge_proto::Outbound::native_test(&self) -> Option<BoxFuture<'_, Result<Duration, OutboundError>>>`（默认 `None`）；`WireGuardOutbound` 实现它（隧道没启动就先启动）
  - `rurge_policy::testbook::TestMode { Url(Url), Native(Target) }`（`Clone + Debug + PartialEq + Eq`）与 `TestMode::target(&self) -> Target`；`TestCase.mode: TestMode`（取代 `url`）；`TestObserver::begin(&self, policy: &str, target: &Target) -> Box<dyn TestRecord>`
  - crate 内：`Stack::last_handshake(&self) -> Option<Instant>`；`Device::handshake(&self) -> Duration`
  - `testing::HTTP_PORT: u16 (80)`；`PeerCore.http_requests: usize`

- [ ] **Step 1: 先写用例**

`crates/rurge-policy/src/testbook.rs`——把

```rust
    use rurge_net::connector::{BoxedStream, ConnectOpts, Target};
```

换成

```rust
    use rurge_net::connector::{BoxedStream, ConnectOpts};
```

`crates/rurge-policy/src/testbook.rs`——把

```rust
            url: Url::parse("http://127.0.0.1:9/").unwrap(),
```

换成

```rust
            mode: TestMode::Url(Url::parse("http://127.0.0.1:9/").unwrap()),
```

`crates/rurge-policy/src/testbook.rs`——把

```rust
        fn begin(&self, policy: &str, url: &Url) -> Box<dyn TestRecord> {
            self.0.lock().unwrap().push(format!("begin {policy} {url}"));
```

换成

```rust
        fn begin(&self, policy: &str, target: &Target) -> Box<dyn TestRecord> {
            self.0
                .lock()
                .unwrap()
                .push(format!("begin {policy} {}:{}", target.host, target.port));
```

`crates/rurge-policy/src/testbook.rs`——把

```rust
            [
                "begin P http://127.0.0.1:9/",
                "end P connect: closed by the gate"
            ]
        );
```

换成

```rust
            ["begin P 127.0.0.1:9", "end P connect: closed by the gate"]
        );
    }

    /// Tests itself its own way, in the time given, or not at all.
    struct Own(Option<Duration>);

    impl Outbound for Own {
        fn name(&self) -> &str {
            "Own"
        }
        fn connect_tcp<'a>(
            &'a self,
            _target: &'a Target,
            _opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
            Box::pin(std::future::ready(Err(OutboundError::Proxy(
                "never dialled".to_string(),
            ))))
        }
        fn native_test(&self) -> Option<BoxFuture<'_, Result<Duration, OutboundError>>> {
            let took = self.0?;
            Some(Box::pin(async move {
                tokio::time::sleep(took).await;
                Ok(took)
            }))
        }
    }

    /// A policy tested without a URL: the outbound's own test is the score
    /// (phase 2 M4 design 6.7), seen going to its target.
    #[tokio::test]
    async fn a_native_test_is_the_outbounds_own() {
        let native = |outbound: OutboundRef| TestCase {
            mode: TestMode::Native(Target::new(HostName::parse("wg.test"), 51820)),
            timeout: Duration::from_millis(300),
            ..case("W", outbound, 1)
        };
        let seen = Arc::new(Seen::default());
        let book = book();
        book.observe(Arc::new(seen.clone()));
        let passed = book
            .test(native(Arc::new(Own(Some(Duration::from_millis(7))))))
            .await;
        assert_eq!(passed.outcome, Ok(Duration::from_millis(7)));
        assert_eq!(seen.0.lock().unwrap()[0], "begin W wg.test:51820");
        let without = book.test_once(&native(Arc::new(Own(None)))).await;
        assert_eq!(
            without.outcome,
            Err("the policy has no test of its own".to_string())
        );
        let slow = book
            .test_once(&native(Arc::new(Own(Some(Duration::from_secs(60))))))
            .await;
        assert_eq!(slow.outcome, Err("timed out".to_string()));
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            Some("DIRECT")
        );
    }

    /// A round's tests run eight at a time, each within its own timeout.
```

换成

```rust
            Some("DIRECT")
        );
    }

    /// A section with the test keys, its peer at `wg.test:51820`.
    fn wireguard_section(name: &str, mtu: u16, extra: &str) -> String {
        format!(
            "[WireGuard {name}]\nprivate-key = yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=\n\
self-ip = 10.9.0.2\nmtu = {mtu}\n{extra}\
peer = (public-key = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=, allowed-ips = 0.0.0.0/0, endpoint = wg.test:51820)\n"
        )
    }

    /// A `wireguard` policy without a `dns-server` or a `test-url` of its
    /// own is tested by a handshake, which goes to its first peer; either
    /// makes it a test at a URL. Both may take 10 seconds more, and an
    /// edited section is another test (phase 2 M4 design 6.7).
    #[test]
    fn a_wireguard_policy_is_tested_by_a_handshake_unless_it_can_fetch_a_url() {
        let text = |mtu: u16| {
            format!(
                "[General]\nproxy-test-url = http://127.0.0.1:9/\ntest-timeout = 3\n\
[Proxy]\nN = wireguard, section-name=plain\nU = wireguard, section-name=plain, test-url=http://t.test/\n\
D = wireguard, section-name=dns\n[Rule]\nFINAL,DIRECT\n{}{}",
                wireguard_section("plain", mtu, ""),
                wireguard_section("dns", 1280, "dns-server = 10.0.0.53\n")
            )
        };
        let reg = generation(&text(1280), &FakeFactory::new(), None);
        let case = |reg: &PolicyRegistry, name: &str| reg.test_case(name).expect("tested");
        assert_eq!(
            case(&reg, "N").mode,
            TestMode::Native(Target::new(HostName::parse("wg.test"), 51820))
        );
        assert_eq!(
            case(&reg, "U").mode,
            TestMode::Url(Url::parse("http://t.test/").unwrap())
        );
        assert_eq!(
            case(&reg, "D").mode,
            TestMode::Url(Url::parse("http://127.0.0.1:9/").unwrap())
        );
        for name in ["N", "U", "D"] {
            assert_eq!(case(&reg, name).timeout, Duration::from_secs(13), "{name}");
        }
        let edited = generation(&text(1400), &FakeFactory::new(), None);
        assert_ne!(case(&edited, "N").key, case(&reg, "N").key);
        assert_eq!(
            case(&edited, "D").key,
            case(&reg, "D").key,
            "its own section is the same"
        );
    }

    /// A round's tests run eight at a time, each within its own timeout.
```

`crates/rurge-policy/src/auto.rs`——把

```rust
            url: url::Url::parse("http://127.0.0.1:9/").unwrap(),
```

换成

```rust
            mode: crate::testbook::TestMode::Url(url::Url::parse("http://127.0.0.1:9/").unwrap()),
```

`crates/rurge-policy/src/testing.rs`——把

```rust
use rurge_config::spec::{CommonOpts, PolicySpec};
```

换成

```rust
use rurge_config::spec::{CommonOpts, PolicySpec, ProtoSpec};
```

`crates/rurge-policy/src/testing.rs`——把

```rust
        if matches!(spec.proto, rurge_config::spec::ProtoSpec::Direct) {
            return Ok(Arc::new(rurge_proto::Direct::new(connector)));
        }
        let server = Target::new(
            spec.server.clone().expect("a proxy policy has a server"),
            spec.port.expect("and a port"),
        );
```

换成

```rust
        let server = match &spec.proto {
            ProtoSpec::Direct => return Ok(Arc::new(rurge_proto::Direct::new(connector))),
            // a tunnel's first stop is its first peer
            ProtoSpec::WireGuard(wireguard) => {
                let first = &wireguard.section.peers[0].endpoint;
                Target::new(first.host.clone(), first.port)
            }
            _ => Target::new(
                spec.server.clone().expect("a proxy policy has a server"),
                spec.port.expect("and a port"),
            ),
        };
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
//! answers on every address routed to it — a TCP echo service on port 7 and
//! a name server at `DNS_ADDRESS`. `FakeWgPeer` puts one on a loopback UDP
//! port.
```

换成

```rust
//! answers on every address routed to it — a TCP echo service on port 7, an
//! HTTP service on port 80 and a name server at `DNS_ADDRESS`. `FakeWgPeer`
//! puts one on a loopback UDP port.
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
/// Connections the echo service takes at once: SYNs that arrive together.
```

换成

```rust
/// The HTTP service of a peer: `204 No Content` to every request, on a
/// connection that stays open.
pub const HTTP_PORT: u16 = 80;
/// Connections a service takes at once: SYNs that arrive together.
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
    conns: Vec<Conn>,
```

换成

```rust
    conns: Vec<Conn>,
    web: Vec<SocketHandle>,
    /// The HTTP connections and what arrived of the request being read.
    requests: Vec<(SocketHandle, Vec<u8>)>,
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
    pub dns_questions: Vec<String>,
```

换成

```rust
    pub dns_questions: Vec<String>,
    /// HTTP requests answered.
    pub http_requests: usize,
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
            .collect();
```

换成

```rust
            .collect();
        let web = (0..BACKLOG)
            .map(|_| listen(&mut sockets, HTTP_PORT))
            .collect();
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
            conns: Vec::new(),
```

换成

```rust
            conns: Vec::new(),
            web,
            requests: Vec::new(),
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
            dns_questions: Vec::new(),
```

换成

```rust
            dns_questions: Vec::new(),
            http_requests: 0,
```

`crates/rurge-proto-wireguard/src/testing/mod.rs`——把

```rust
        self.answer_questions();
```

换成

```rust
        self.answer_requests();
        self.answer_questions();
    }

    fn answer_requests(&mut self) {
        for k in 0..self.web.len() {
            let handle = self.web[k];
            if self.sockets.get::<tcp::Socket>(handle).state() == tcp::State::Listen {
                continue;
            }
            self.requests.push((handle, Vec::new()));
            self.web[k] = listen(&mut self.sockets, HTTP_PORT);
        }
        let (sockets, answered) = (&mut self.sockets, &mut self.http_requests);
        self.requests.retain_mut(|(handle, request)| {
            let socket = sockets.get_mut::<tcp::Socket>(*handle);
            let mut buf = [0u8; 2048];
            while let Ok(n @ 1..) = socket.recv_slice(&mut buf) {
                request.extend_from_slice(&buf[..n]);
            }
            while let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                request.drain(..end + 4);
                *answered += 1;
                let _ = socket.send_slice(b"HTTP/1.1 204 No Content\r\n\r\n");
            }
            // the client is done: done too
            if !socket.may_recv() && socket.may_send() {
                socket.close();
            }
            let over = matches!(socket.state(), tcp::State::Closed | tcp::State::TimeWait);
            if over {
                sockets.remove(*handle);
            }
            !over
        });
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
            (TOTAL >> 20) as f64 / elapsed.as_secs_f64()
        );
    }

    /// Two policies that name one section: one tunnel, one handshake.
```

换成

```rust
            (TOTAL >> 20) as f64 / elapsed.as_secs_f64()
        );
    }

    /// The native test: a handshake with the peers, however fresh the
    /// session; its time is the result. It starts the tunnel when need be.
    #[tokio::test]
    async fn the_native_test_is_a_handshake_with_the_peers() {
        let (peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let test = || wg.native_test().expect("wireguard has one");
        let rtt = test().await.expect("a handshake");
        assert!(rtt < Duration::from_secs(2), "{rtt:?}");
        let before = peer.core().handshakes;
        test().await.expect("a handshake");
        assert!(peer.core().handshakes > before, "the session was fresh");
        peer.go_silent(true);
        let silent = tokio::time::timeout(Duration::from_millis(500), test()).await;
        assert!(silent.is_err(), "no answer, no result");
    }

    /// Two policies that name one section: one tunnel, one handshake.
```

`crates/rurge-engine/tests/outbounds_wireguard.rs`——把

```rust
    assert!(peer.clients().is_empty(), "nothing went to the peer");
}

/// A reload that leaves the line and its section alone keeps the tunnel,
```

换成

```rust
    assert!(peer.clients().is_empty(), "nothing went to the peer");
}

/// Without a `dns-server` or a `test-url`, a test of the policy is a
/// handshake with its peers, which the session log shows going to the
/// first peer (phase 2 M4 design 6.7).
#[tokio::test]
async fn a_policy_test_is_a_handshake_with_the_peers() {
    let (peer, section) = peer().await;
    let h = harness(Profile {
        proxies: WG,
        sections: &section,
        ..Profile::default()
    })
    .await;
    let results = h
        .engine
        .test_policies(&["WG".to_string()], None)
        .await
        .unwrap();
    let result = results[0].1.as_ref().expect("a wireguard policy is tested");
    assert!(result.outcome.is_ok(), "{:?}", result.outcome);
    assert!(peer.core().handshakes >= 1);
    let log = h.engine.request_log();
    let test = || {
        log.recent(10)
            .into_iter()
            .find(|r| r.rule.as_deref() == Some("policy test"))
    };
    wait_until("the test session", || test().is_some()).await;
    assert_eq!(test().unwrap().dst, peer.addr().to_string());
}

/// With a `test-url`, the test fetches it through the tunnel.
#[tokio::test]
async fn a_test_url_is_fetched_through_the_tunnel() {
    let (peer, section) = peer().await;
    let h = harness(Profile {
        proxies: "WG = wireguard, section-name=w, test-url=http://10.0.0.1/",
        sections: &section,
        ..Profile::default()
    })
    .await;
    let results = h
        .engine
        .test_policies(&["WG".to_string()], None)
        .await
        .unwrap();
    let result = results[0].1.as_ref().expect("a wireguard policy is tested");
    assert!(result.outcome.is_ok(), "{:?}", result.outcome);
    assert_eq!(peer.core().http_requests, 2, "two HEADs, one connection");
}

/// A reload that leaves the line and its section alone keeps the tunnel,
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-policy wireguard`
Expected: FAIL，编译错误（节选）——

```text
error[E0433]: failed to resolve: could not find `TestMode` in `testbook`
   --> crates\rurge-policy\src\auto.rs:521:36
error[E0407]: method `native_test` is not a member of trait `Outbound`
   --> crates\rurge-policy\src\testbook.rs:418:9
error[E0412]: cannot find type `Target` in this scope
   --> crates\rurge-policy\src\testbook.rs:281:26
error[E0560]: struct `testbook::TestCase` has no field named `mode`
   --> crates\rurge-policy\src\auto.rs:521:13
error[E0609]: no field `mode` on type `testbook::TestCase`
    --> crates\rurge-policy\src\registry.rs:2204:29
error: could not compile `rurge-policy` (lib test) due to 19 previous errors
```

- [ ] **Step 3: 实现**

`crates/rurge-proto/src/outbound.rs`——把

```rust
use std::sync::Arc;
```

换成

```rust
use std::sync::Arc;
use std::time::Duration;
```

`crates/rurge-proto/src/outbound.rs`——把

```rust
        None
    }
}
```

换成

```rust
        None
    }
    /// A test of its own, for a policy tested without a URL (phase 2 M4
    /// design 6.7): how long it took. `None`: the outbound has none.
    fn native_test(&self) -> Option<BoxFuture<'_, Result<Duration, OutboundError>>> {
        None
    }
}
```

`crates/rurge-policy/src/testbook.rs`——把

```rust
use crate::probe::{Probed, probe};
```

换成

```rust
use crate::probe::{Probed, probe};
use rurge_config::HostName;
use rurge_net::connector::Target;
```

`crates/rurge-policy/src/testbook.rs`——把

```rust
    pub when: SystemTime,
}

/// What to test, as the registry in use has it.
```

换成

```rust
    pub when: SystemTime,
}

/// What a test measures (phase 2 M4 design 6.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestMode {
    /// Two HEAD requests to the URL through the outbound (M3 design 6.1).
    Url(Url),
    /// The outbound's own test (`Outbound::native_test`): a `wireguard`
    /// policy's handshake, which the session log shows going to the
    /// target — its first peer.
    Native(Target),
}

impl TestMode {
    /// Where the test goes, for the session log: of a URL its host and
    /// port, never the rest, which a subscription line may have set (M3-D7).
    pub fn target(&self) -> Target {
        match self {
            TestMode::Url(url) => Target::new(
                HostName::parse(url.host_str().unwrap_or_default()),
                url.port_or_known_default().unwrap_or(0),
            ),
            TestMode::Native(target) => target.clone(),
        }
    }
}

/// What to test, as the registry in use has it.
```

`crates/rurge-policy/src/testbook.rs`——把

```rust
    pub url: Url,
```

换成

```rust
    pub mode: TestMode,
```

`crates/rurge-policy/src/testbook.rs`——把

```rust
    fn begin(&self, policy: &str, url: &Url) -> Box<dyn TestRecord>;
```

换成

```rust
    fn begin(&self, policy: &str, target: &Target) -> Box<dyn TestRecord>;
```

`crates/rurge-policy/src/testbook.rs`——把

```rust
            .map(|observer| observer.begin(&case.policy, &case.url));
        let outcome = match probe(&case.outbound, &case.url, case.timeout, case.roots.clone()).await
        {
            Probed::Passed { score, reused } => {
                if !reused
                    && self
                        .warned
                        .lock()
                        .expect("warned")
                        .insert(case.url.to_string())
                {
                    // the URL stays out of the log: a subscription line may
                    // have set it (M3-D7)
                    tracing::warn!(
                        policy = %case.policy,
                        "the test server does not keep the connection: the score includes the dial"
                    );
                }
                Ok(score)
            }
            Probed::Failed(why) => Err(why),
```

换成

```rust
            .map(|observer| observer.begin(&case.policy, &case.mode.target()));
        let outcome = match &case.mode {
            TestMode::Url(url) => {
                match probe(&case.outbound, url, case.timeout, case.roots.clone()).await {
                    Probed::Passed { score, reused } => {
                        if !reused && self.warned.lock().expect("warned").insert(url.to_string()) {
                            // the URL stays out of the log: a subscription
                            // line may have set it (M3-D7)
                            tracing::warn!(
                                policy = %case.policy,
                                "the test server does not keep the connection: the score includes the dial"
                            );
                        }
                        Ok(score)
                    }
                    Probed::Failed(why) => Err(why),
                }
            }
            TestMode::Native(_) => native(&case.outbound, case.timeout).await,
```

`crates/rurge-policy/src/testbook.rs`——把

```rust
        let _ = tx.send(Some(result));
    }
}

#[cfg(test)]
```

换成

```rust
        let _ = tx.send(Some(result));
    }
}

/// The outbound's own test, within `timeout`.
async fn native(outbound: &OutboundRef, timeout: Duration) -> Result<Duration, String> {
    let test = outbound
        .native_test()
        .ok_or_else(|| "the policy has no test of its own".to_string())?;
    match tokio::time::timeout(timeout, test).await {
        Ok(result) => result.map_err(|e| e.to_string()),
        Err(_) => Err("timed out".to_string()),
    }
}

#[cfg(test)]
```

`crates/rurge-policy/src/registry.rs`——把

```rust
use crate::testbook::{MAX_CONCURRENT_TESTS, TestCase, TestResult};
use rurge_config::rule::PolicyRef;
use rurge_config::spec::{CommonOpts, GroupSpec, IpVersion, PolicySpec};
use rurge_config::{Builtin, Config, GroupKind, KeystoreType, PolicyKind, Span};
use rurge_net::connector::Connector;
```

换成

```rust
use crate::testbook::{MAX_CONCURRENT_TESTS, TestCase, TestMode, TestResult};
use rurge_config::rule::PolicyRef;
use rurge_config::spec::{CommonOpts, GroupSpec, IpVersion, PolicySpec, ProtoSpec};
use rurge_config::wireguard::WireGuardSection;
use rurge_config::{Builtin, Config, GroupKind, KeystoreType, PolicyKind, Span};
use rurge_net::connector::{Connector, Target};
```

`crates/rurge-policy/src/registry.rs`——把

```rust
/// How a policy is tested (M3 design 6.1), worked out as the registry is
/// built.
struct TestSpec {
    /// `None`: the test URL does not parse, and the policy never passes.
    url: Option<Url>,
```

换成

```rust
/// What a `wireguard` policy's test may take on top of its timeout: the
/// test may be what starts the tunnel (manual).
const WIREGUARD_START: Duration = Duration::from_secs(10);

/// How a policy is tested (M3 design 6.1; phase 2 M4 design 6.7), worked
/// out as the registry is built.
struct TestSpec {
    /// `None`: the test URL does not parse, and the policy never passes.
    mode: Option<TestMode>,
```

`crates/rurge-policy/src/registry.rs`——把

```rust
    fn new(definition: &str, url: &str, timeout: Duration) -> TestSpec {
        let mut h = DefaultHasher::new();
        (definition, url, timeout).hash(&mut h);
        TestSpec {
            url: Url::parse(url).ok(),
```

换成

```rust
    /// The key is what the policy is — its definition, and the section of
    /// a `wireguard` policy — and how it is tested (`how`: the URL, or
    /// `native`).
    fn new(
        definition: &str,
        section: Option<&WireGuardSection>,
        mode: Option<TestMode>,
        how: &str,
        timeout: Duration,
    ) -> TestSpec {
        let mut h = DefaultHasher::new();
        (definition, section, how, timeout).hash(&mut h);
        TestSpec {
            mode,
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            key: h.finish(),
        }
    }
```

换成

```rust
            key: h.finish(),
        }
    }

    /// A test at `url`.
    fn at(
        definition: &str,
        section: Option<&WireGuardSection>,
        url: &str,
        timeout: Duration,
    ) -> TestSpec {
        let mode = Url::parse(url).ok().map(TestMode::Url);
        TestSpec::new(definition, section, mode, url, timeout)
    }
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            let (url, timeout) = cfg.general.test_target(
                common.and_then(|c| c.test_url.as_deref()),
                common.and_then(|c| c.test_timeout),
                direct,
            );
            Some(TestSpec::new(definition, url, timeout))
```

换成

```rust
            let own_url = common.and_then(|c| c.test_url.as_deref());
            let (url, timeout) =
                cfg.general
                    .test_target(own_url, common.and_then(|c| c.test_timeout), direct);
            let Some(ProtoSpec::WireGuard(wireguard)) = spec.map(|s| &s.proto) else {
                return Some(TestSpec::at(definition, None, url, timeout));
            };
            let section = &wireguard.section;
            let timeout = timeout + WIREGUARD_START;
            // without a `dns-server` or a `test-url` of its own: a handshake
            // with the peers (phase 2 M4 design 6.7)
            Some(if section.dns_servers.is_empty() && own_url.is_none() {
                let first = &section.peers[0].endpoint;
                let mode = TestMode::Native(Target::new(first.host.clone(), first.port));
                TestSpec::new(definition, Some(section), Some(mode), "native", timeout)
            } else {
                TestSpec::at(definition, Some(section), url, timeout)
            })
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        table
            .tests
            .insert("DIRECT".to_string(), TestSpec::new("DIRECT", url, timeout));
```

换成

```rust
        table.tests.insert(
            "DIRECT".to_string(),
            TestSpec::at("DIRECT", None, url, timeout),
        );
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        test.url.as_ref()?;
```

换成

```rust
        test.mode.as_ref()?;
```

`crates/rurge-policy/src/registry.rs`——把

```rust
    /// test URL. `None` for what never passes — a REJECT, a protocol not
    /// implemented, a test URL that does not parse — and for a group.
```

换成

```rust
    /// test URL or its own way (phase 2 M4 design 6.7). `None` for what never
    /// passes — a REJECT, a protocol not implemented, a test URL that does
    /// not parse — and for a group.
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            url: test.url.clone()?,
```

换成

```rust
            mode: test.mode.clone()?,
```

`crates/rurge-engine/src/auto.rs`——把

```rust
use rurge_config::rule::PolicyRef;
use rurge_config::session::{ListenerKind, SessionInfo};
use rurge_config::{GroupKind, HostName};
use rurge_inbound::{SessionHandle, SessionOutcome};
use rurge_policy::auto::SelectCtx;
use rurge_policy::testbook::{TestObserver, TestRecord, TestResult};
```

换成

```rust
use rurge_config::GroupKind;
use rurge_config::rule::PolicyRef;
use rurge_config::session::{ListenerKind, SessionInfo};
use rurge_inbound::{SessionHandle, SessionOutcome};
use rurge_net::connector::Target;
use rurge_policy::auto::SelectCtx;
use rurge_policy::testbook::{TestMode, TestObserver, TestRecord, TestResult};
```

`crates/rurge-engine/src/auto.rs`——把

```rust
/// chain, and for its target the test URL's host and port — never the rest
/// of the URL, which a subscription line may have set (M3-D7).
```

换成

```rust
/// chain, and for its target `TestMode::target` — the test URL's host and
/// port, or the first peer of a `wireguard` policy tested by a handshake.
```

`crates/rurge-engine/src/auto.rs`——把

```rust
    fn begin(&self, policy: &str, url: &Url) -> Box<dyn TestRecord> {
```

换成

```rust
    fn begin(&self, policy: &str, target: &Target) -> Box<dyn TestRecord> {
```

`crates/rurge-engine/src/auto.rs`——把

```rust
        let host = HostName::parse(url.host_str().unwrap_or_default());
        let mut session = SessionInfo::tcp(host, url.port_or_known_default().unwrap_or(0));
```

换成

```rust
        let mut session = SessionInfo::tcp(target.host.clone(), target.port);
```

`crates/rurge-engine/src/auto.rs`——把

```rust
                            case.url = url;
```

换成

```rust
                            case.mode = TestMode::Url(url);
```

`crates/rurge-proto-wireguard/src/stack.rs`——把

```rust
            now.saturating_duration_since(self.epoch).as_micros() as i64
        )
    }
```

换成

```rust
            now.saturating_duration_since(self.epoch).as_micros() as i64
        )
    }

    /// When a handshake rurge started last completed, with whichever peer.
    pub fn last_handshake(&self) -> Option<Instant> {
        self.peers.iter().filter_map(|p| p.handshake).max()
    }
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
    network_changed: AtomicBool,
```

换成

```rust
    network_changed: AtomicBool,
    /// Set by `Device::handshake`, taken by the task: a handshake with
    /// every peer now.
    greet: AtomicBool,
    /// Told of every handshake that completes.
    handshaken: Notify,
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
            network_changed: AtomicBool::new(false),
```

换成

```rust
            network_changed: AtomicBool::new(false),
            greet: AtomicBool::new(false),
            handshaken: Notify::new(),
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
        self.shared.kick();
    }
```

换成

```rust
        self.shared.kick();
    }

    /// A handshake with every peer now, however fresh their sessions: how
    /// long until the first completed (phase 2 M4 design 6.7).
    pub(crate) async fn handshake(&self) -> Duration {
        let asked = Instant::now();
        self.shared.greet.store(true, Ordering::SeqCst);
        self.shared.kick();
        loop {
            let mut completed = pin!(self.shared.handshaken.notified());
            completed.as_mut().enable();
            let last = self
                .shared
                .stack
                .lock()
                .expect("the tunnel")
                .last_handshake();
            if let Some(at) = last.filter(|at| *at > asked) {
                return at - asked;
            }
            completed.await;
        }
    }
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
                next_redial = now + self.redial;
            }
            let deadline = {
```

换成

```rust
                next_redial = now + self.redial;
            }
            if self.shared.greet.swap(false, Ordering::SeqCst) {
                self.shared
                    .stack
                    .lock()
                    .expect("the tunnel")
                    .initiate(&mut out);
            }
            let deadline = {
```

`crates/rurge-proto-wireguard/src/device.rs`——把

```rust
                    drop(stack);
```

换成

```rust
                    drop(stack);
                    if !completed.is_empty() {
                        self.shared.handshaken.notify_waiters();
                    }
```

`crates/rurge-proto-wireguard/src/outbound.rs`——把

```rust
        })
    }
```

换成

```rust
        })
    }

    /// A handshake with the peers (phase 2 M4 design 6.7); the tunnel starts
    /// first when it has not.
    fn native_test(&self) -> Option<BoxFuture<'_, Result<Duration, OutboundError>>> {
        Some(Box::pin(async move {
            let device = self.device(&ConnectOpts::default()).await?;
            Ok(device.handshake().await)
        }))
    }
```

要点：
- 模式在注册表构建时决定（`test_spec`）：`wireguard` 的节里没有 `dns-server` 且策略没写自己的 `test-url`（`[General]` 的 `proxy-test-url` 不算）→ `TestMode::Native(第一个 peer 的 endpoint)`；否则照旧按 URL。`wireguard` 两种都加 `WIREGUARD_START`（10 秒）。key 由定义、节、模式（URL 原文或 `native`）与超时算出。
- `TestBook` 的原生测试：没有 `native_test` 的出站是 `the policy has no test of its own`（注册表不会给别的协议选原生模式，这只是兜底）；超时是 `timed out`，与 URL 测试相同；出站的错误原样作为 `error`。"测试服务器不保持连接"的一次性告警只属于 URL 模式。
- `Device::handshake`：记下"现在"，让设备任务向每个 peer `initiate`（会话再新也重新握手），然后等 `Stack::last_handshake` 晚于那一刻——设备任务每完成一次握手就 `notify_waiters` 一次；先 `enable` 通知再查，不会漏掉。peer 不回应时一直等，时限由 `TestBook` 给。
- 引擎的测试会话：目标取 `TestMode::target`；`POST /v1/policies/test` 给了 `url` 时把模式换成那个 URL（一次性、不保存）。
- `FakeWgPeer` 的 HTTP 服务：80 号端口同时保持 8 个监听套接字，读到一个完整的请求头就回 `HTTP/1.1 204 No Content`，连接保持——探针的第二次 `HEAD` 走同一条连接；`http_requests` 记下回了几次。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-policy` → 128 passed（新增 `registry` 与 `testbook` 各 1 条：`wireguard` 的模式、超时与 key；原生测试的分数、没有自己测试的出站、超时）。
Run: `cargo test -p rurge-proto-wireguard` → 40 passed / 1 ignored（新增 `the_native_test_is_a_handshake_with_the_peers`：结果合理、会话再新也重新握手、peer 不回应时不出结果）。
Run: `cargo test -p rurge-engine --test outbounds_wireguard` → 6 passed（新增：握手测速，测试会话的目标是 peer 的 endpoint；写了 `test-url` 时经隧道两次 `HEAD`，同一条连接）。
Run: `cargo test -p rurge-engine --test auto_groups` 与 `--test smart` → 照旧全部通过（测试会话的目标仍是 URL 的主机与端口）。

- [ ] **Step 5: 门禁与提交**

跑门禁（48 个测试二进制，1117 通过 / 2 忽略）。

```bash
git add crates/rurge-proto/src/outbound.rs crates/rurge-policy crates/rurge-engine crates/rurge-proto-wireguard
git commit -m "feat(policy): 测速的两种模式（TestMode）——wireguard 没有 dns-server 与 test-url 时以握手测速，另加 10 秒；FakeWgPeer 的 HTTP 服务"
```

### Task 9: 互操作——sing-box 的 WireGuard 端点

对 sing-box 1.14.1 的 WireGuard 端点（总设计 Q4、设计第 10 节第 3 层，P8）：sing-box 以 rurge 为唯一的 peer、给发往它的报文写保留字节；rurge 的 `wireguard` 出站带同样的 `client-id` 与它握手，经隧道连回环上的 echo（sing-box 把隧道里出来的连接交给 `direct`），覆盖单块与跨多块的往返与原生测速。rurge 收到的报文里 sing-box 写的保留字节必须先清零——否则 boringtun 认不出报文类型，握手不成。夹具新增 WireGuard 端点的渲染与启动：端点没有监听地址这一项，它的 UDP 端口开在所有地址上；UDP 端口无从探测，就绪看同一份配置里一个只听回环的 `mixed` 入站。

本机没有 sing-box：用例打印一行 `skipping …` 后返回，只在 CI 上真正运行（CI 已安装 sing-box 1.14.1 并设置 `RURGE_TEST_SING_BOX` 与 `RURGE_INTEROP_REQUIRED=1`，不用改 CI）。夹具自己的两条用例（渲染的写法、"配置绝不碰本机"的安全守卫）在本机照常运行。

**Files:**
- Modify: `tests/interop/Cargo.toml`（`base64`；dev 依赖 `rurge-proto-wireguard` 开 `testing`）
- Modify: `tests/interop/src/lib.rs`（`free_udp_port`、`WireGuardEndpoint`、`render_wireguard`、`SingBox::spawn_wireguard` 与共用的 `launch`，与用例）
- Modify: `tests/interop/tests/common/mod.rs`（导出 `WireGuardEndpoint`、`keypair`）
- Create: `tests/interop/tests/sing_box_wireguard.rs`
- Modify: `tests/interop/README.md`

**Interfaces:**
- Consumes: Task 7 的工厂（`common::outbound` 由配置文本经 `EngineFactory` 建出站）；Task 8 的 `native_test`；`rurge_proto_wireguard::testing::keypair`；既有的夹具 `Reference`、`SingBox`、`free_port`、`echo_server`、`roundtrip`、`roundtrip_big`、`hex`。
- Produces:
  - `rurge_interop::{free_udp_port() -> u16, WireGuardEndpoint { private_key: [u8; 32], address: String, peer_public_key: [u8; 32], peer_allowed_ips: Vec<String>, reserved: Option<[u8; 3]> }, render_wireguard(endpoint: &WireGuardEndpoint, port: u16, ready: u16) -> Value}`
  - `SingBox::spawn_wireguard(binary: &Path, dir: &Path, endpoint: &WireGuardEndpoint) -> (SingBox, u16)`（第二项是端点的 UDP 端口）

- [ ] **Step 1: 先写夹具的用例**

`tests/interop/src/lib.rs`——把

```rust
        assert_eq!(v2["detour"], "in-3");
    }

    #[test]
    fn free_ports_are_usable() {
```

换成

```rust
        assert_eq!(v2["detour"], "in-3");
    }

    fn endpoint() -> WireGuardEndpoint {
        WireGuardEndpoint {
            private_key: [1; 32],
            address: "10.9.0.1/32".into(),
            peer_public_key: [2; 32],
            peer_allowed_ips: vec!["10.9.0.2/32".into()],
            reserved: Some([1, 2, 3]),
        }
    }

    /// In user space, nothing of the machine's; only the endpoint's UDP port
    /// is on every address.
    #[test]
    fn the_wireguard_configuration_never_touches_the_machine() {
        let config = render_wireguard(&endpoint(), 51820, 1001);
        let text = config.to_string();
        for forbidden in ["set_system_proxy", "tun", "auto_route", "0.0.0.0", "::"] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
        }
        let top: Vec<&String> = config.as_object().unwrap().keys().collect();
        assert_eq!(top, ["endpoints", "inbounds", "log", "outbounds"]);
        assert_eq!(config["endpoints"][0]["system"], false);
        assert_eq!(config["inbounds"][0]["listen"], "127.0.0.1");
        assert_eq!(
            config["outbounds"],
            json!([{ "type": "direct", "tag": "direct" }])
        );
    }

    #[test]
    fn the_wireguard_endpoint_is_rendered_as_sing_box_spells_it() {
        let config = render_wireguard(&endpoint(), 51820, 1001);
        let wg = &config["endpoints"][0];
        assert_eq!(
            (&wg["type"], &wg["listen_port"]),
            (&json!("wireguard"), &json!(51820))
        );
        assert_eq!(wg["address"], json!(["10.9.0.1/32"]));
        assert_eq!(
            wg["private_key"],
            "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE="
        );
        assert_eq!(
            wg["peers"],
            json!([{
                "public_key": "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=",
                "allowed_ips": ["10.9.0.2/32"],
                "reserved": [1, 2, 3],
            }])
        );
        assert_eq!(config["inbounds"][0]["listen_port"], 1001);
    }

    #[test]
    fn free_ports_are_usable() {
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-interop --lib`
Expected: FAIL，编译错误——

```text
error[E0412]: cannot find type `WireGuardEndpoint` in this scope
   --> tests\interop\src\lib.rs:479:22
error[E0422]: cannot find struct, variant or union type `WireGuardEndpoint` in this scope
   --> tests\interop\src\lib.rs:480:9
error[E0425]: cannot find function `render_wireguard` in this scope
   --> tests\interop\src\lib.rs:493:22
error[E0425]: cannot find function `render_wireguard` in this scope
   --> tests\interop\src\lib.rs:510:22
error: could not compile `rurge-interop` (lib test) due to 4 previous errors
```

- [ ] **Step 3: 实现夹具与互操作用例**

`tests/interop/Cargo.toml`——把

```toml
[dependencies]
```

换成

```toml
[dependencies]
base64.workspace = true
```

`tests/interop/Cargo.toml`——把

```toml
rurge-proto-ssh = { workspace = true, features = ["testing"] }
```

换成

```toml
rurge-proto-ssh = { workspace = true, features = ["testing"] }
rurge-proto-wireguard = { workspace = true, features = ["testing"] }
```

`tests/interop/src/lib.rs`——把

```rust
//! 127.0.0.1 only, its single outbound is `direct`, and it never holds a key
//! that touches the machine (`set_system_proxy`, `tun`, `auto_route`).
```

换成

```rust
//! 127.0.0.1 only — but for the UDP port of a WireGuard endpoint, which
//! sing-box opens on every address — its single outbound is `direct`, and it
//! never holds a key that touches the machine (`set_system_proxy`, `tun`,
//! `auto_route`; a WireGuard endpoint runs in user space).
```

`tests/interop/src/lib.rs`——把

```rust
use serde_json::{Value, json};
use std::net::{SocketAddr, TcpListener, TcpStream};
```

换成

```rust
use base64::Engine;
use serde_json::{Value, json};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
```

`tests/interop/src/lib.rs`——把

```rust
        .and_then(|l| l.local_addr())
```

换成

```rust
        .and_then(|l| l.local_addr())
        .expect("a free loopback port")
        .port()
}

/// A UDP port that was free a moment ago.
pub fn free_udp_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .and_then(|s| s.local_addr())
```

`tests/interop/src/lib.rs`——把

```rust
    })
}

/// A reference implementation running as a child on the loopback; killed and
```

换成

```rust
    })
}

/// sing-box's WireGuard endpoint (sing-box 1.11 and later) with one peer:
/// rurge, whose tunnel address must be in `peer_allowed_ips`, and to which
/// every message carries `reserved` (its `client-id`).
pub struct WireGuardEndpoint {
    pub private_key: [u8; 32],
    /// The endpoint's own tunnel address, as a prefix.
    pub address: String,
    pub peer_public_key: [u8; 32],
    pub peer_allowed_ips: Vec<String>,
    pub reserved: Option<[u8; 3]>,
}

/// The whole configuration for `endpoint` on UDP port `port`, in user space
/// (`system: false`: no interface, no route), and a `mixed` inbound on
/// `ready` that says when sing-box is up — a UDP port cannot be probed.
/// What comes out of the tunnel goes `direct`.
pub fn render_wireguard(endpoint: &WireGuardEndpoint, port: u16, ready: u16) -> Value {
    let key = |k: &[u8; 32]| base64::engine::general_purpose::STANDARD.encode(k);
    let mut peer = json!({
        "public_key": key(&endpoint.peer_public_key),
        "allowed_ips": endpoint.peer_allowed_ips,
    });
    if let Some(reserved) = endpoint.reserved {
        peer["reserved"] = json!(reserved);
    }
    json!({
        "log": { "level": "warn", "timestamp": false },
        "endpoints": [{
            "type": "wireguard",
            "tag": "wg",
            "system": false,
            "address": [endpoint.address],
            "private_key": key(&endpoint.private_key),
            "listen_port": port,
            "peers": [peer],
        }],
        "inbounds": [{ "type": "mixed", "tag": "ready", "listen": "127.0.0.1", "listen_port": ready }],
        "outbounds": [{ "type": "direct", "tag": "direct" }],
    })
}

/// A reference implementation running as a child on the loopback; killed and
```

`tests/interop/src/lib.rs`——把

```rust
        let config = dir.join("sing-box.json");
        std::fs::write(&config, render(&with_ports).to_string()).expect("write the config");
        let mut command = Command::new(binary);
        command.arg("run").arg("-c").arg(&config).arg("-D").arg(dir);
```

换成

```rust
        SingBox::launch(binary, dir, &render(&with_ports), ports)
    }

    /// Starts `binary` with `endpoint` on a UDP port of its own; that port,
    /// once sing-box is up.
    pub fn spawn_wireguard(
        binary: &Path,
        dir: &Path,
        endpoint: &WireGuardEndpoint,
    ) -> (SingBox, u16) {
        let (port, ready) = (free_udp_port(), free_port());
        let config = render_wireguard(endpoint, port, ready);
        (SingBox::launch(binary, dir, &config, vec![ready]), port)
    }

    fn launch(binary: &Path, dir: &Path, config: &Value, ports: Vec<u16>) -> SingBox {
        let path = dir.join("sing-box.json");
        std::fs::write(&path, config.to_string()).expect("write the config");
        let mut command = Command::new(binary);
        command.arg("run").arg("-c").arg(&path).arg("-D").arg(dir);
```

`tests/interop/tests/common/mod.rs`——把

```rust
pub use rurge_interop::{Inbound, InboundKind, SingBox, TlsFiles, sing_box_or_skip};
```

换成

```rust
pub use rurge_interop::{
    Inbound, InboundKind, SingBox, TlsFiles, WireGuardEndpoint, sing_box_or_skip,
};
```

`tests/interop/tests/common/mod.rs`——把

```rust
pub use rurge_proto::{OutboundError, OutboundRef};
```

换成

```rust
pub use rurge_proto::{OutboundError, OutboundRef};
pub use rurge_proto_wireguard::testing::keypair;
```

新建 `tests/interop/tests/sing_box_wireguard.rs`：

```rust
//! The `wireguard` outbound against sing-box's WireGuard endpoint (phase 2
//! M4 design §10; total design Q4): the handshake, TCP through the tunnel,
//! the reserved bytes sing-box writes into its messages to a peer with a
//! `client-id`, and the handshake test.

mod common;

use common::*;
use std::time::Duration;

#[tokio::test]
async fn a_connection_goes_through_a_sing_box_wireguard_endpoint() {
    let Some(bin) = sing_box_or_skip("a_connection_goes_through_a_sing_box_wireguard_endpoint")
    else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (ours, our_public) = keypair();
    let (theirs, their_public) = keypair();
    let (_sb, port) = SingBox::spawn_wireguard(
        &bin,
        dir.path(),
        &WireGuardEndpoint {
            private_key: theirs,
            address: "10.9.0.1/32".to_string(),
            peer_public_key: our_public,
            peer_allowed_ips: vec!["10.9.0.2/32".to_string()],
            reserved: Some([1, 2, 3]),
        },
    );
    let echo = echo_server().await;
    let profile = format!(
        "[Proxy]\nWG = wireguard, section-name=sb\n[Rule]\nFINAL,DIRECT\n\
[WireGuard sb]\nprivate-key = {}\nself-ip = 10.9.0.2\n\
peer = (public-key = {}, allowed-ips = 127.0.0.1/32, endpoint = 127.0.0.1:{port}, client-id = 1/2/3)\n",
        hex(&ours),
        hex(&their_public)
    );
    let out = outbound(&profile, "WG", None);
    roundtrip(&out, echo).await;
    roundtrip_big(&out, echo).await;
    let test = out.native_test().expect("a wireguard policy tests itself");
    tokio::time::timeout(Duration::from_secs(10), test)
        .await
        .expect("sing-box answers the handshake in time")
        .expect("a handshake");
}
```

`tests/interop/README.md`——把

```markdown
`rurge-interop` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它把 [sing-box](https://sing-box.sagernet.org/) 与 [xray](https://github.com/XTLS/Xray-core) 作为参照实现，以回环子进程的方式拉起来，驱动 rurge 的 `http` / `https` / `socks5` / `trojan` / `vmess` / `anytls` 出站，以及包在 Shadow TLS 里的 `trojan`，去连它们，验证 rurge 与真实的第三方实现互通。xray 只用来跑 `vmess`：VMess 协议由 xray 所在的这一脉实现定义，sing-box 的实现是重写，手写的编解码需要两个独立参照互相印证（M2 设计 M2-D5）。
```

换成

```markdown
`rurge-interop` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它把 [sing-box](https://sing-box.sagernet.org/) 与 [xray](https://github.com/XTLS/Xray-core) 作为参照实现，以回环子进程的方式拉起来，驱动 rurge 的 `http` / `https` / `socks5` / `trojan` / `vmess` / `anytls` / `wireguard` 出站，以及包在 Shadow TLS 里的 `trojan`，去连它们，验证 rurge 与真实的第三方实现互通。xray 只用来跑 `vmess`：VMess 协议由 xray 所在的这一脉实现定义，sing-box 的实现是重写，手写的编解码需要两个独立参照互相印证（M2 设计 M2-D5）。
```

`tests/interop/README.md`——把

```markdown
本地默认不安装 sing-box 与 xray：`cargo test -p rurge-interop` 会正常通过，sing-box 的十一个互操作用例与 xray 的一个互操作用例各打印一行 `skipping …` 后直接返回（两个夹具自身的单元测试——配置渲染、"配置绝不碰本机"的安全守卫、取空闲端口——照常运行并断言）。要在本机真正跑 sing-box 的用例，二选一：
```

换成

```markdown
本地默认不安装 sing-box 与 xray：`cargo test -p rurge-interop` 会正常通过，sing-box 的十二个互操作用例与 xray 的一个互操作用例各打印一行 `skipping …` 后直接返回（两个夹具自身的单元测试——配置渲染、"配置绝不碰本机"的安全守卫、取空闲端口——照常运行并断言）。要在本机真正跑 sing-box 的用例，二选一：
```

`tests/interop/README.md`——把

```markdown
- Shadow TLS（`tests/sing_box_shadow_tls.rs`）：sing-box 的 `shadowtls` 入站（v2 与 v3，v3 开 `strict_mode`）把握手转发给夹具自己在回环上起的 TLS 服务端（伪装站点，证书由夹具的 CA 签发），解出来的流量经 `detour` 交给同一个 sing-box 里的 `trojan` 入站；覆盖小负载与跨多帧的往返，以及口令错误（v3 的会话文本、伪装站点确实收到了那个 HTTP 请求）。v2 的用例让伪装站点不发 session ticket（sing-box 对"首帧之前又转发了字节"只多容忍一次写），v3 的发两张（覆盖数据阶段开头的残留记录）。
```

换成

```markdown
- Shadow TLS（`tests/sing_box_shadow_tls.rs`）：sing-box 的 `shadowtls` 入站（v2 与 v3，v3 开 `strict_mode`）把握手转发给夹具自己在回环上起的 TLS 服务端（伪装站点，证书由夹具的 CA 签发），解出来的流量经 `detour` 交给同一个 sing-box 里的 `trojan` 入站；覆盖小负载与跨多帧的往返，以及口令错误（v3 的会话文本、伪装站点确实收到了那个 HTTP 请求）。v2 的用例让伪装站点不发 session ticket（sing-box 对"首帧之前又转发了字节"只多容忍一次写），v3 的发两张（覆盖数据阶段开头的残留记录）。

- WireGuard（`tests/sing_box_wireguard.rs`）：sing-box 的 WireGuard 端点（`endpoints`，sing-box 1.11 起；`system: false`，在用户态运行，不建网卡、不改路由）以 rurge 为唯一的 peer，给发往它的每个报文写上保留字节 `1/2/3`；rurge 的 `wireguard` 出站带 `client-id = 1/2/3` 与它握手，经隧道连回环上的 echo（sing-box 把隧道里出来的连接交给 `direct`），覆盖单块与跨多块的往返，以及原生测速（强制握手）。rurge 收到的报文里 sing-box 写的保留字节必须先清零，否则 boringtun 认不出报文类型、握手不成。端点没有监听地址这一项，**它的 UDP 端口开在所有地址上**；就绪与否看同一份配置里一个只听 `127.0.0.1` 的 `mixed` 入站（UDP 端口无从探测）。
```

`tests/interop/README.md`——把

```markdown
- 夹具渲染出的 sing-box 配置只有 `log` / `inbounds` / `outbounds` 三个顶层键；每个入站只监听 `127.0.0.1`；唯一的出站是 `direct`。xray 配置同样只有这三个顶层键，唯一的出站是 `freedom`。任何地方都不出现 `set_system_proxy`、`tun`、`auto_route` 这些键（`rurge_interop::render` 与 `rurge_interop::xray::render` 的单元测试 `the_configuration_never_touches_the_machine` 各自断言这一点）；`shadowtls` 入站的 `handshake.server` 恒为 `127.0.0.1`（夹具的单元用例断言）。
```

换成

```markdown
- 夹具渲染出的 sing-box 配置只有 `log` / `inbounds` / `outbounds` 三个顶层键（WireGuard 的配置另有 `endpoints`）；每个入站只监听 `127.0.0.1`；唯一的出站是 `direct`。WireGuard 端点在用户态运行（`system: false`），它的 UDP 端口开在所有地址上（端点没有监听地址这一项；`rurge_interop::render_wireguard` 的单元测试 `the_wireguard_configuration_never_touches_the_machine` 断言其余各项）。xray 配置同样只有这三个顶层键，唯一的出站是 `freedom`。任何地方都不出现 `set_system_proxy`、`tun`、`auto_route` 这些键（`rurge_interop::render` 与 `rurge_interop::xray::render` 的单元测试 `the_configuration_never_touches_the_machine` 各自断言这一点）；`shadowtls` 入站的 `handshake.server` 恒为 `127.0.0.1`（夹具的单元用例断言）。
```

要点：
- 渲染：`system: false`（用户态，不建网卡、不改路由）必须显式写出——`true` 会在本机建 TUN 设备；安全守卫用例断言它，并断言配置里没有 `tun`、`auto_route`、`set_system_proxy`、`0.0.0.0`、`::`，入站只听 `127.0.0.1`，唯一的出站是 `direct`。密钥按 sing-box 的写法用标准 Base64；`reserved` 是三个数。
- `SingBox::spawn` 与 `spawn_wireguard` 共用 `launch`（写配置、启动、等端口就绪）。
- 互操作用例：rurge 的节 `allowed-ips = 127.0.0.1/32`、`client-id = 1/2/3`，sing-box 的 peer `allowed_ips` 是 rurge 的隧道地址 `10.9.0.2/32`、`reserved = [1, 2, 3]`；每一步都有界（`roundtrip` 10 秒、`roundtrip_big` 20 秒、原生测速 10 秒），在 CI 上回归会得到失败而不是挂住。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-interop` → 夹具 8 passed（新增 2 条）；`sing_box_wireguard` 1 passed（本机打印 `skipping a_connection_goes_through_a_sing_box_wireguard_endpoint: no sing-box …` 后返回）。

- [ ] **Step 5: 门禁与提交**

跑门禁（49 个测试二进制，1120 通过 / 2 忽略）。

```bash
git add tests/interop
git commit -m "test(interop): 对 sing-box WireGuard 端点的互操作用例（保留字节、多块往返、握手测速）"
```

### Task 10: 能力表翻转 `wireguard` 与文档

`wireguard` 的全部行为已经就位（Task 1 ～ 9），翻转 bin 的能力表：此后 `W0007` 不再因 `wireguard` 出现（设计 8.4、验收第 4 条）。翻转前核对设计承诺的行为都已存在（M2 设计第 8 节的教训）：节的类型化与参数（Task 1）、UDP 载体（Task 2）、路由 / `client-id` / 协议栈（Task 3）、隧道与本机解析（Task 4）、隧道内 DNS（Task 5）、生命周期与一个私钥一条隧道（Task 6）、工厂 / `underlying-proxy` / DNS 会话防环 / 端到端（Task 7）、测速（Task 8）、互操作（Task 9）。然后是文档：兼容性清单（设计第 12 节的 `wireguard` 部分与本计划的差异）、两份 README、`CLAUDE.md`、手工验收清单的 M4b 一节、两份 API 文档（阶段 1 的脱敏名单、阶段 2 的测速）、M4 设计新增的第 19 节（本计划与设计文字不同的地方）、总设计的 Q4、风险 G 与技术选型表的协议栈一行。本计划末尾的「执行期修正记录」与「延后事项」两张表，由控制者在派发本任务时给出要补的行（执行中的偏差、各任务门禁的实际数字、Task 7 的吞吐基准数字、执行中新发现的延后事项），一并写入。

两次提交：能力表一次，文档一次。

**Files:**
- Modify: `crates/rurge/src/capabilities.rs`
- Test: `crates/rurge/tests/cli.rs`（`check_knows_wireguard`）
- Modify: `docs/surge-compatibility-matrix.md`、`README.md`、`README_en.md`、`CLAUDE.md`、`docs/acceptance/phase2-manual.md`、`docs/api/phase1.md`、`docs/api/phase2.md`、`docs/superpowers/specs/2026-09-27-phase2-m4-wireguard-ssh-external-design.md`、`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`
- Modify: 本计划文件末尾两张表（按控制者给的行）

**Interfaces:**
- Consumes: Task 1 ～ 9。
- Produces: 无新接口；`capabilities::current()` 的 `policy_kinds` 多了 `PolicyKind::WireGuard`。

- [ ] **Step 1: 先写用例**

`crates/rurge/tests/cli.rs`——把

```rust
            "keystore item `key1` is not an OpenSSH private key",
```

换成

```rust
            "keystore item `key1` is not an OpenSSH private key",
        ))
        .stdout(predicate::str::contains("c2VjcmV0").not());
}

const WIREGUARD: &str = "[General]\n[Proxy]\nW = wireguard, section-name=home\n\
Old = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\n[Rule]\nFINAL,DIRECT\n\
[WireGuard home]\nprivate-key = yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=\nself-ip = 10.9.0.2\n\
peer = (public-key = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=, allowed-ips = 0.0.0.0/0, endpoint = vpn.test:51820)\n";
const WIREGUARD_BAD_KEY: &str = "[General]\n[Proxy]\nW = wireguard, section-name=home\n[Rule]\nFINAL,DIRECT\n\
[WireGuard home]\nprivate-key = c2VjcmV0IGtleSBtYXRlcmlhbA==\nself-ip = 10.9.0.2\n\
peer = (public-key = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=, allowed-ips = 0.0.0.0/0, endpoint = vpn.test:51820)\n";

/// `rurge check` knows `wireguard` policies and their sections: a key that
/// is no key is an error at its line, named and not quoted (M4 design 4.1).
#[test]
fn check_knows_wireguard() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "wg.conf", WIREGUARD))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // `ss` is still a later milestone; `wireguard` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`ss`") && !out.contains("`wireguard`"),
        "{out}"
    );
    assert!(!out.contains("yAnz5TF"), "{out}");

    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "bad.conf", WIREGUARD_BAD_KEY))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("E0023"))
        .stdout(predicate::str::contains(
            "bad.conf:7: [WireGuard home]: `private-key` is not a 32-byte key in Base64 or hex",
        ))
        // and the policy that names it
        .stdout(predicate::str::contains(
            "bad.conf:3: policy `W`: `section-name` names `[WireGuard home]`, which does not exist or has errors",
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge --test cli check_knows_wireguard`
Expected: FAIL——`wireguard` 仍报 `W0007`（临时目录名每次不同）：

```text
test check_knows_wireguard ... FAILED
thread 'check_knows_wireguard' panicked at crates\rurge\tests\cli.rs:331:5:
assertion `left == right` failed: warning[W0007] …\wg.conf:3: policy type `wireguard` is not implemented in this version; such policies behave as REJECT
warning[W0007] …\wg.conf:4: policy type `ss` is not implemented in this version; such policies behave as REJECT
…\wg.conf: 0 error(s), 2 warning(s), 0 note(s)
  left: 2
 right: 1
```

- [ ] **Step 3: 实现**

`crates/rurge/src/capabilities.rs`——把

```rust
            PolicyKind::Ssh,
```

换成

```rust
            PolicyKind::Ssh,
            PolicyKind::WireGuard,
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge --test cli check_knows` → 6 passed（`W0007` 只报 `ss`，输出里没有私钥；私钥不是密钥的节让 `rurge check` 退出 2，`E0023` 落在 `private-key` 那一行，策略行另有一条"节不存在或有错"，文本都不引用私钥）。

- [ ] **Step 5: 门禁与提交（能力表）**

跑门禁（49 个测试二进制，1121 通过 / 2 忽略）。

```bash
git add crates/rurge/src/capabilities.rs crates/rurge/tests/cli.rs
git commit -m "feat(rurge): 能力表翻转 wireguard（W0007 不再因 wireguard 出现）"
```

- [ ] **Step 6: 兼容性清单**

`docs/surge-compatibility-matrix.md`——把

```markdown
| `[WireGuard <name>]` | WireGuard 策略配置 | ✅ | 2 | |
```

换成

```markdown
| `[WireGuard <name>]` | WireGuard 策略配置 | ✅ | 2 | M4b 已实现：加载时逐节校验（没被策略引用的节也校验），节内的错误与 `section-name` 指向不存在或有错的节都是 `E0023`，不认识的键与 `peer` 字段 `W0001`，同名的节只用第一个（`W0020`）；键与字段见 4.6 节 `[WireGuard <name>]` 与 `peer` 两行 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `wireguard` | WireGuard L3 隧道作为策略 | 全部 | ✅ | 2 | |
| `tailscale` | Tailscale 节点作为策略 | iOS 5.20 / Mac 6.7+ | ❓ | 远期 | 需嵌入 Tailscale 客户端（控制面协议、DERP、MagicDNS）；单独评估 |
| `external` | 外部代理程序（本地 SOCKS5） | Mac only（iOS 视为 REJECT） | ✅ | 2 | rurge 在 Win/Lin/mac 均支持 |
| 策略指向 rurge 尚未实现的协议类型 | Surge 原生支持全部协议 | 全部 | 🟡 | 1 / 2 | 加载时告警 `W0007`；运行时该策略按 `REJECT` 处理，会话日志 `error = policy protocol not implemented: <type>`；随阶段 2 各里程碑逐协议移除：M1 已移除 `http` `https` `socks5` `socks5-tls`；M2a 已移除 `trojan`；M2b 已移除 `vmess`（写了 `vmess-aead=true` 的行）与 `anytls`；M4a 已移除 `ssh`。没写 `vmess-aead=true` 的 `vmess` 行是唯一例外：仍按 `W0007` 处理，但走专门的诊断文本 `` `vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet ``（每次加载一条，不是每行一条）与专门的会话日志文本 `policy protocol not implemented: vmess (legacy handshake)`，不是这里的通用 `<type>` 模板 |
```

换成

```markdown
| `wireguard` | WireGuard L3 隧道作为策略 | 全部 | ✅ | 2 | M4b（阶段 2）已实现（TCP）：用户态 WireGuard（boringtun 的 `Tunn` 加 smoltcp 协议栈，不建虚拟网卡、不改路由）；第一次用到（拨号或测速）时启动：给每个 peer 建一条 UDP 载体、各握手一次，启动时连不上的 peer 每 5 分钟再试；指向同一个节的策略共用一条隧道；同一私钥连着同一 peer 的两条隧道会互相抢 peer 记住的地址（peer 回应最后写来的地址），所以一条隧道启动时结束同一私钥、有共同 peer、来自更早配置的那条——重载改了节时，旧隧道上的连接随之断开——更早配置的策略也不能再把它抢回去（拨号报 `wireguard: a newer configuration of this tunnel is in use`）；私钥相同而 peer 不同的两个节互不影响。目标是域名时：节里配了 `dns-server` 就经隧道查询，没配（或写 `system`）时在本机解析、含 `[Host]`（手册只说一般不能经这个策略解析）；目标地址不在任何 peer 的 `allowed-ips` 里时立即失败（`wireguard: no peer's allowed-ips covers <地址>`，绝不直连兜底），隧道没有该地址族的本端地址时同样立即失败（`wireguard: the tunnel has no IPv4 address` / `… IPv6 address`）；目标端口拒绝连接是 `wireguard: the destination refused the connection`；peer 不回应握手时拨号在时限处超时。带 `underlying-proxy` 时隧道起不来（M5 之前链路不载 UDP）：会话 REJECT，请求记录的说明是 `policy protocol not implemented: wireguard over underlying-proxy`，不静默改走直连。endpoint 写成域名时每 5 分钟重新解析，地址变了就换新的载体并立即握手（日志 `wireguard: the peer's endpoint moved`）；"网络已变化"的入口（重建全部载体、重新握手）已有，阶段 2 没有触发它的探测器。日志只带策略名与 peer 序号：握手成功（第一次与失败之后恢复时）、握手没有回应（boringtun 放弃重试时，约 90 秒后）、启动时 peer 连不上；boringtun 自己的日志不输出。出站被释放（重载删掉或改了这条策略）后，隧道在经它的最后一个连接结束时停止（被接替的除外）。已知的 smoltcp 0.12 限制：TCP 丢包后回退 N 重传（整窗重发）；发送方没有零窗口探测；对端关闭发送方向之后仍在途的数据丢失时不再重传——极少数上传可能卡到空闲超时；不开 TCP keep-alive；拥塞控制用 Reno（0.12 的 Cubic 把窗口的单位算错）。吞吐参考：Windows 11 回环上两端都是 smoltcp 的双向回显约 6.5 MiB/s 每方向。不支持 UDP（M5）。参数与差异见 4.6 节 `wireguard` 行 |
| `tailscale` | Tailscale 节点作为策略 | iOS 5.20 / Mac 6.7+ | ❓ | 远期 | 需嵌入 Tailscale 客户端（控制面协议、DERP、MagicDNS）；单独评估 |
| `external` | 外部代理程序（本地 SOCKS5） | Mac only（iOS 视为 REJECT） | ✅ | 2 | rurge 在 Win/Lin/mac 均支持 |
| 策略指向 rurge 尚未实现的协议类型 | Surge 原生支持全部协议 | 全部 | 🟡 | 1 / 2 | 加载时告警 `W0007`；运行时该策略按 `REJECT` 处理，会话日志 `error = policy protocol not implemented: <type>`；随阶段 2 各里程碑逐协议移除：M1 已移除 `http` `https` `socks5` `socks5-tls`；M2a 已移除 `trojan`；M2b 已移除 `vmess`（写了 `vmess-aead=true` 的行）与 `anytls`；M4a 已移除 `ssh`；M4b 已移除 `wireguard`。没写 `vmess-aead=true` 的 `vmess` 行是唯一例外：仍按 `W0007` 处理，但走专门的诊断文本 `` `vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet ``（每次加载一条，不是每行一条）与专门的会话日志文本 `policy protocol not implemented: vmess (legacy handshake)`，不是这里的通用 `<type>` 模板 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| 约束：Shadow TLS 不能与 TUIC / WireGuard / Tailscale / 其他 QUIC 类协议组合 | 配置错误 | ✅ | 2 | 判断函数已落地（`tuic` `tuic-v5` `hysteria2` `masque` `wireguard` `tailscale`：`` E0018 Shadow TLS cannot be combined with a `<type>` policy ``）；这些协议的 spec 出现之前该错误不会真的报出——它们的整行目前都还没有被读取 |
```

换成

```markdown
| 约束：Shadow TLS 不能与 TUIC / WireGuard / Tailscale / 其他 QUIC 类协议组合 | 配置错误 | ✅ | 2 | 判断函数已落地（`tuic` `tuic-v5` `hysteria2` `masque` `wireguard` `tailscale`：`` E0018 Shadow TLS cannot be combined with a `<type>` policy ``）；`wireguard` 自 M4b 起真的报出，其余协议的 spec 出现之前该错误不会真的报出——它们的整行目前都还没有被读取 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `wireguard` 策略行 | `section-name`（必填）`underlying-proxy`（默认 DIRECT）`test-url`（仅 http）`test-timeout`（另加 10 秒 L3 初始化）`ecn` | ✅ | 2 | |
| `[WireGuard <name>]` | `private-key`（Base64 或 64 位十六进制）`self-ip` / `self-ip-v6`（至少一个）`dns-server` `prefer-ipv6` `mtu`（576–1420，默认 1280）`peer`（可多个，多行累加） | ✅ | 2 | |
| `peer` 字段 | `public-key` `allowed-ips`（最长前缀匹配，v4/v6 分表）`endpoint` `preshared-key` `keepalive`（0–65535）`client-id`（`83/12/235` / 3 字节十六进制 / 4 字符 Base64，WARP 保留字节） | ✅ | 2 | |
| WireGuard 生命周期 | 加载时准备、按需握手；网络变化或底层策略变化时重建；分片重组；仅回应发往本地隧道地址的 ICMP echo；握手包 DSCP 0x88 | ✅ | 2 | |
| WireGuard 测试 | 无 `dns-server` 且无 `test-url` → 原生 RTT 探测；否则标准 URL 测试 | ✅ | 2 | |
```

换成

```markdown
| `wireguard` 策略行 | `section-name`（必填）`underlying-proxy`（默认 DIRECT）`test-url`（仅 http）`test-timeout`（另加 10 秒 L3 初始化）`ecn` | ✅ | 2 | M4b 已实现。`section-name` 必填（缺了是 `E0018`），指向不存在或有错的节是 `E0023`；spec 带着节的全部内容：改了节，重载就重建出站（隧道随之替换，见 4.2 节 `wireguard` 行）；`test-url` 只接受 `http://`（否则 `E0018`，不引用取值）；`test-timeout` 照常，另加 10 秒留给隧道启动（照手册）；`interface` / `allow-other-interface` / `tfo` / `tos` 不适用（`W0028`，忽略）；`ip-version` 决定解析 endpoint 时优先的地址族；`ecn`（`W0029`）与 `underlying-proxy` 在 M5 生效——后者此前让策略 REJECT，加载时的 `W0029` 说明这一点；不能叠 Shadow TLS（`E0018`）；订阅行自己写的 `section-name=` 不生效（见 5.2 节 `policy-path` 行） |
| `[WireGuard <name>]` | `private-key`（Base64 或 64 位十六进制）`self-ip` / `self-ip-v6`（至少一个）`dns-server` `prefer-ipv6` `mtu`（576–1420，默认 1280）`peer`（可多个，多行累加） | ✅ | 2 | M4b 已实现。密钥接受 Base64（带不带填充都行）或 64 位十六进制，错误只点名键，不引用取值；`self-ip` / `self-ip-v6` 是纯地址（写成前缀是 `E0023`）；`dns-server` 逗号分隔：IPv4 / IPv6 地址、带端口的 `1.1.1.1:53` / `[2606:4700::1111]:53` 或 `system`，组播地址与加密 DNS 的 URL 不接受（`E0023`）——按列表顺序问，第一个作答的服务器为准（"没有这个名字"也算作答），每个最多等 2 秒，该地址族没有本端地址或没有 peer 覆盖的服务器直接跳过，`system` 表示在那个位置改用本机解析；A 与 AAAA 按本端有的地址族同时问，成功的结果按 TTL 缓存（最多 256 个名字、最长 1 小时）；`prefer-ipv6` 在两族本端地址都有时决定先用哪一族（隧道内 DNS 与本机解析都是）；`private-key` 与 `peer` 的 `preshared-key` 在 `profiles/current`（`sensitive=0`）里为 `***`（`preshared-key` 自 M4b 起进了脱敏名单） |
| `peer` 字段 | `public-key` `allowed-ips`（最长前缀匹配，v4/v6 分表）`endpoint` `preshared-key` `keepalive`（0–65535）`client-id`（`83/12/235` / 3 字节十六进制 / 4 字符 Base64，WARP 保留字节） | ✅ | 2 | M4b 已实现。`public-key`、`allowed-ips`、`endpoint` 必填；一个节至少一个 `peer`；`allowed-ips` 含逗号时整值加引号，写成纯地址时按单个主机，两个 peer 列了同一前缀时后写的生效；内层源地址不在该 peer `allowed-ips` 里的包丢弃；`endpoint` 写作 `host:port`（IPv6 写作 `[addr]:port`），主机名不受 `[Host]` 影响（与代理服务器的主机名相同）；`keepalive` 为 0 表示关闭；`client-id` 写进每个发出的报文的保留字节，收到的报文先把这 3 个字节清零再解（照手册） |
| WireGuard 生命周期 | 加载时准备、按需握手；网络变化或底层策略变化时重建；分片重组；仅回应发往本地隧道地址的 ICMP echo；握手包 DSCP 0x88 | 🟡 | 2 | M4b：构建（含 `rurge check` 的干构建）只解析配置，不开 socket、不解析域名；第一次用到时启动；隧道内 IP 分片重组（最多同时 4 个、每个 16 KiB）；超过 MTU 的 IPv4 外发包由协议栈分片后发出（手册：丢弃；TCP 报文段按 MSS 切分，本来不会超出）；只回应发往本端隧道地址的 ICMP echo；握手发起包的 TOS 字节标 0x88（DSCP AF41），其它包不标——Windows 通常忽略应用设置的 DSCP；载体的 UDP 收发缓冲尽量设为 7 MiB（系统可能封顶）；"网络已变化"的入口已有，阶段 2 没有触发它的探测器（阶段 3）；底层策略（`underlying-proxy`）在 M5 生效 |
| WireGuard 测试 | 无 `dns-server` 且无 `test-url` → 原生 RTT 探测；否则标准 URL 测试 | ✅ | 2 | M4b 已实现。原生测试向每个 peer 强制握手（会话再新也握），从发起到第一个握手完成用了多久就是结果（多个 peer 时最快者）；它只证明 peer 可达、握手成功，不证明路由与出口（照手册）；测试会话的目标记为第一个 peer 的 endpoint。URL 测试经隧道两次 `HEAD`（目标域名按 4.2 节 `wireguard` 行的规则解析）。两种都在 `test-timeout` 之外另加 10 秒；`POST /v1/policies/test` 给了 `url` 时一律按那个 URL 测 |
```

- [ ] **Step 7: 两份 README 与 `CLAUDE.md`**

`README.md`——把

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

换成

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

`README.md`——把

```markdown
| 出站协议       | DIRECT / REJECT 系列 / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / 外部程序（DIRECT 与 REJECT 系四种已实现，M3a；HTTP / SOCKS5（含 `socks5-tls`）TCP 上游与 `underlying-proxy` 链已实现，阶段 2 / M1；Trojan（TCP；WebSocket 传输）已实现，阶段 2 / M2a；VMess（AEAD 握手，TCP；可叠加 TLS / WebSocket）与 AnyTLS（TCP，会话复用）已实现，阶段 2 / M2b；SSH（TCP，会话复用）已实现，阶段 2 / M4a） | 2     |
```

换成

```markdown
| 出站协议       | DIRECT / REJECT 系列 / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / 外部程序（DIRECT 与 REJECT 系四种已实现，M3a；HTTP / SOCKS5（含 `socks5-tls`）TCP 上游与 `underlying-proxy` 链已实现，阶段 2 / M1；Trojan（TCP；WebSocket 传输）已实现，阶段 2 / M2a；VMess（AEAD 握手，TCP；可叠加 TLS / WebSocket）与 AnyTLS（TCP，会话复用）已实现，阶段 2 / M2b；SSH（TCP，会话复用）已实现，阶段 2 / M4a；WireGuard（TCP，用户态隧道）已实现，阶段 2 / M4b） | 2     |
```

`README.md`——把

```markdown
3. **阶段 2** 出站协议全集、策略组、策略订阅（进行中：M1 出站地基与 HTTP / SOCKS5 上游已完成；M2a（Trojan，TCP + WebSocket）已完成；M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成；M3a（成员装配与订阅）已完成；M3b（测速与自动组）已完成；M3c（`smart`）已完成；M4a（SSH）已完成）
```

换成

```markdown
3. **阶段 2** 出站协议全集、策略组、策略订阅（进行中：M1 出站地基与 HTTP / SOCKS5 上游已完成；M2a（Trojan，TCP + WebSocket）已完成；M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成；M3a（成员装配与订阅）已完成；M3b（测速与自动组）已完成；M3c（`smart`）已完成；M4a（SSH）已完成；M4b（WireGuard）已完成）
```

`README_en.md`——把

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

换成

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; M4b (WireGuard) is done — the `wireguard` policy (TCP; a user-space tunnel, no virtual interface; several peers chosen by `allowed-ips`; `client-id` (WARP's reserved bytes); destination names looked up through the tunnel when the section has a `dns-server`, on this machine otherwise; tested by a handshake when there is neither a `dns-server` nor a `test-url`) is usable; the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

`README_en.md`——把

```markdown
| Outbound            | DIRECT / REJECT family / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / external program (DIRECT and the four REJECT flavours implemented, M3a; HTTP / SOCKS5 (including `socks5-tls`) TCP upstreams and `underlying-proxy` chains implemented, phase 2 / M1; Trojan (TCP; WebSocket transport) implemented, phase 2 / M2a; VMess (AEAD handshake, TCP; optionally TLS / WebSocket) and AnyTLS (TCP, session reuse) implemented, phase 2 / M2b; SSH (TCP, session reuse) implemented, phase 2 / M4a) | 2     |
```

换成

```markdown
| Outbound            | DIRECT / REJECT family / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / external program (DIRECT and the four REJECT flavours implemented, M3a; HTTP / SOCKS5 (including `socks5-tls`) TCP upstreams and `underlying-proxy` chains implemented, phase 2 / M1; Trojan (TCP; WebSocket transport) implemented, phase 2 / M2a; VMess (AEAD handshake, TCP; optionally TLS / WebSocket) and AnyTLS (TCP, session reuse) implemented, phase 2 / M2b; SSH (TCP, session reuse) implemented, phase 2 / M4a; WireGuard (TCP, user-space tunnel) implemented, phase 2 / M4b) | 2     |
```

`README_en.md`——把

```markdown
3. **Phase 2** All outbound protocols, policy groups, subscriptions (in progress: M1, the outbound foundation and HTTP / SOCKS5 upstreams, is done; M2a, Trojan (TCP + WebSocket), is done; M2b, VMess / AnyTLS, is done; M2c, Shadow TLS, is done; M3a, member assembly and subscriptions, is done; M3b, connectivity tests and automatic groups, is done; M3c, `smart` groups, is done; M4a, SSH, is done)
```

换成

```markdown
3. **Phase 2** All outbound protocols, policy groups, subscriptions (in progress: M1, the outbound foundation and HTTP / SOCKS5 upstreams, is done; M2a, Trojan (TCP + WebSocket), is done; M2b, VMess / AnyTLS, is done; M2c, Shadow TLS, is done; M3a, member assembly and subscriptions, is done; M3b, connectivity tests and automatic groups, is done; M3c, `smart` groups, is done; M4a, SSH, is done; M4b, WireGuard, is done)
```

`CLAUDE.md`——把

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）、M4c（external）尚未开始。
```

换成

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、网络变化入口、同一私钥与 peer 只留一条隧道）、`stream`、`dns`（隧道内 DNS 与缓存）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。M4c（external）尚未开始。
```

`CLAUDE.md`——把

```markdown
- `docs/superpowers/specs/2026-09-27-phase2-m4-wireguard-ssh-external-design.md`：阶段 2 / M4 细化设计（WireGuard / SSH / external 出站），细化总设计的 M4 里程碑、不一致处以它为准。三份计划的拆分（M4a SSH → M4b WireGuard → M4c external）；已决事项 M4-D1 ～ D13（russh 0.63.3 + `ring` 后端、smoltcp 0.12.0 以保住 MSRV 1.89、boringtun 0.7.1 只用 sans-IO 的 `Tunn`、协议栈"共享锁 + waker"、Windows 用 Job Object 清理外部进程树（`rurge-platform` 第二个 unsafe 例外）、只做 TCP、`wireguard` 经 `underlying-proxy` 到 M5、订阅安全门等）；`[WireGuard <name>]` 类型化与新错误码 `E0023`；三种出站的语义、WireGuard 测速的原生模式、需登记的差异；第 15 节 V1–V14 是写各份计划时必须核对的事项，第 16 节是三份计划的任务草图，第 17 节是 M4a 计划期的订正，第 18 节是 M4a 实施期的订正。
- `docs/superpowers/plans/2026-09-27-phase2-m4a-ssh-plan.md`：阶段 2 / M4a（SSH）实施计划（7 个任务）。开头「计划期决定」表（P1–P19）记录核对 russh 源码与手册得出的结论和与设计文字不同的决定（russh 默认没有 `aes128-gcm` 要补上、去掉 `ssh-rsa` 主机密钥签名、RSA 私钥只用 SHA-2 签名、DSA 私钥能解码所以解码后按算法拒绝、重载的指纹要含 SSH 私钥、没配指纹的告警按出站对象计一次、空闲断开按通道计数、`sshd` 互操作只在 Unix 与 CI 上跑等）与「承接事项」；末尾「执行期修正记录」与「延后事项」两张表。
```

换成

```markdown
- `docs/superpowers/specs/2026-09-27-phase2-m4-wireguard-ssh-external-design.md`：阶段 2 / M4 细化设计（WireGuard / SSH / external 出站），细化总设计的 M4 里程碑、不一致处以它为准。三份计划的拆分（M4a SSH → M4b WireGuard → M4c external）；已决事项 M4-D1 ～ D13（russh 0.63.3 + `ring` 后端、smoltcp 0.12.0 以保住 MSRV 1.89、boringtun 0.7.1 只用 sans-IO 的 `Tunn`、协议栈"共享锁 + waker"、Windows 用 Job Object 清理外部进程树（`rurge-platform` 第二个 unsafe 例外）、只做 TCP、`wireguard` 经 `underlying-proxy` 到 M5、订阅安全门等）；`[WireGuard <name>]` 类型化与新错误码 `E0023`；三种出站的语义、WireGuard 测速的原生模式、需登记的差异；第 15 节 V1–V14 是写各份计划时必须核对的事项，第 16 节是三份计划的任务草图，第 17 节是 M4a 计划期的订正，第 18 节是 M4a 实施期的订正，第 19 节是 M4b 计划期的订正。
- `docs/superpowers/plans/2026-09-27-phase2-m4a-ssh-plan.md`：阶段 2 / M4a（SSH）实施计划（7 个任务）。开头「计划期决定」表（P1–P19）记录核对 russh 源码与手册得出的结论和与设计文字不同的决定（russh 默认没有 `aes128-gcm` 要补上、去掉 `ssh-rsa` 主机密钥签名、RSA 私钥只用 SHA-2 签名、DSA 私钥能解码所以解码后按算法拒绝、重载的指纹要含 SSH 私钥、没配指纹的告警按出站对象计一次、空闲断开按通道计数、`sshd` 互操作只在 Unix 与 CI 上跑等）与「承接事项」；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/superpowers/plans/2026-09-28-phase2-m4b-wireguard-plan.md`：阶段 2 / M4b（WireGuard）实施计划（10 个任务）。开头「计划期决定」表（P1–P21）记录核对 boringtun / smoltcp / hickory-proto 源码与手册得出的结论和与设计文字不同的决定（smoltcp 0.12 每次 `poll` 每条连接只发一个报文段、它的 Cubic 把窗口单位算错而改用 Reno、设备批量收与载体 7 MiB 缓冲、同一私钥与 peer 只留一条隧道、连接持有隧道、握手日志只在状态变化时记、引擎装配排在测速之前等）与「承接事项」；末尾「执行期修正记录」与「延后事项」两张表。
```

`CLAUDE.md`——把

```markdown
cargo test -p rurge-engine --test outbounds_ssh   # 经 ssh 出站的端到端用例：目标名交给服务器解析、Keystore 私钥与指纹、经 Shadow TLS、经 SSH 的测速
```

换成

```markdown
cargo test -p rurge-engine --test outbounds_ssh   # 经 ssh 出站的端到端用例：目标名交给服务器解析、Keystore 私钥与指纹、经 Shadow TLS、经 SSH 的测速
cargo test -p rurge-proto-wireguard             # wireguard：路由表、client-id、协议栈（内存里的对端）、隧道与流、隧道内 DNS、生命周期、原生测速，出站对回环假对端（FakeWgPeer）
cargo test -p rurge-engine --test outbounds_wireguard   # 经 wireguard 出站的端到端用例：回显、[Host] 解析、underlying-proxy 的 REJECT、重载沿用与替换、握手测速与经隧道的 URL 测速
cargo test -p rurge-proto-wireguard --release throughput -- --ignored --nocapture   # WireGuard 吞吐基准（经回环假对端双向回显 64 MiB；不作门禁）
```

- [ ] **Step 8: 手工验收、两份 API 文档与两份设计文档**

`docs/acceptance/phase2-manual.md`——把

```markdown
- [ ] 日志（含 `--log-level verbose`）里搜不到口令与私钥内容。

```

换成

```markdown
- [ ] 日志（含 `--log-level verbose`）里搜不到口令与私钥内容。

## M4b　WireGuard

需要一个自己的 WireGuard 服务端（`wg-quick`、路由器或云主机均可），WARP 一项另需一份 Cloudflare WARP 的配置（带 `client-id`）。自动化测试只用回环与假对端，对 sing-box 的互操作只在 CI 上跑，覆盖不了真实网络。

- [ ] 基本连通：照服务端写 `[WireGuard home]`（`private-key`、`self-ip`、`peer = (public-key = …, allowed-ips = 0.0.0.0/0, endpoint = <服务器>:<端口>)`）与 `WG = wireguard, section-name=home`，经 `WG` 浏览几个网站正常；日志里有一条 `wireguard: handshake completed`（只带 `policy=WG peer=1`）；服务端 `wg show` 里 rurge 这个 peer 有 latest handshake 与收发字节。
- [ ] 目标域名：不写 `dns-server` 时目标域名在本机解析（`[Host]` 里写的映射生效）；写上 `dns-server = <隧道那头的 DNS>` 后经隧道查询（服务端抓包或 DNS 日志能看到查询），查不到的名字请求失败，错误是 `dns: wireguard: dns lookup of <名字> failed`。
- [ ] 路由：`allowed-ips` 只写服务端内网网段（如 `10.8.0.0/24`），规则把一个公网域名指到 `WG`：请求立即失败，错误是 `wireguard: no peer's allowed-ips covers <地址>`，没有改走直连。
- [ ] 吞吐：经 `WG` 下载、上传一个几百 MiB 的文件，速度与官方客户端在同一量级、全程不卡住；记下实测数字（自动化基准只测回环，两端都是 smoltcp）。
- [ ] WARP：用 WARP 的配置（`client-id = <三个数字>`，endpoint `engage.cloudflareclient.com:2408`），经 `WG` 访问 `https://www.cloudflare.com/cdn-cgi/trace`，输出里有 `warp=on`。
- [ ] 测速：把 `WG` 放进 `url-test` 组。不写 `dns-server` 与 `test-url` 时测速结果是握手往返时间（`GET /v1/policy_groups/test_results`），请求记录里测试会话的目标是服务端的 endpoint；写上 `test-url=http://…` 后改为经隧道的 URL 测试。
- [ ] 重载：只改无关的内容后 `rurge reload`，经 `WG` 的下载不中断、服务端没有新的握手；改了 `[WireGuard home]`（如 `mtu`）后 `rurge reload`，旧连接断开，新连接正常，服务端 `wg show` 里这个 peer 的 endpoint 稳定在一个地址上。
- [ ] endpoint 写成域名：让它的解析结果换成同一服务端的另一个地址（或另一台同配置的服务端），5 分钟内日志出现 `wireguard: the peer's endpoint moved`，之后的新连接走新地址。
- [ ] 服务端停掉：经 `WG` 的请求在拨号时限处失败；约 90 秒后日志有一条 `wireguard: the peer did not answer the handshake`；服务端恢复后下一个请求正常，日志再有一条 `wireguard: handshake completed`。
- [ ] 日志（含 `--log-level verbose`）里搜不到私钥与 `preshared-key` 的内容；`GET /v1/profiles/current?sensitive=0` 里二者都是 `***`。

```

`docs/api/phase1.md`——把

```markdown
| GET | `/v1/profiles/current?sensitive=0\|1` | | `text/plain`；默认（`sensitive=0`）把下列内容替换为 `***`，其余内容与行数、行尾 CRLF 原样保留：① 独立成行的密钥 `key = value`（键名大小写不敏感）`password`、`ca-passphrase`、`ca-p12`、`private-key`、`psk`、`pre-shared-key`、`token`；② 值里任意位置的内联参数 `name = value`（值到**第一个顶层逗号**为止，顶层按解析器自己的规则判定：`"` 或 `'` 在值里**任何位置**都会开启一段引号，`"` 内 `\` 转义下一个字符、`'` 内不转义，`(` / `)` 分组，引号内与括号内的逗号都属于值——`password="p,w"`、`password=ab"c,d"`、`password=a(b,c)d` 都是一整个值；引号或括号未闭合时抹到行尾。参数名前面必须是行首、逗号、空白或 `(`）`password`、`psk`、`private-key`、`pre-shared-key`、`base64`、`token`、`uuid`、`username`、`headers`、`ws-headers`、`ws-path`、`shadow-tls-password`、`policy-path`、`external-policy-modifier`、`test-url`（后三个是阶段 2 加的：订阅链接与订阅行设的测试 URL 常带 token，修饰列表能设任何参数；`username` 会连带脱敏无害的 SSH 用户名；`headers=` 与 `ws-headers=` 的值整体被抹掉，连 header 名也不保留，`ws-path=` 的值同样整体被抹掉——这是有意的过度脱敏，自定义 header 的值与 WebSocket 路径都属于凭据）；③ `http-api` / `external-controller-access` / `http-listen` / `socks5-listen` 的 `key@` 前缀与 `wifi-access-http-auth` 的口令；④ 所有写作 `type, server, port` 的代理类型（`http` `https` `h2-connect` `socks5` `socks5-tls` `ss` `snell` `vmess` `trojan` `tuic` `tuic-v5` `hysteria2` `masque` `anytls` `trust-tunnel` `ssh`）策略行第 4 个起的 token——**凡不是 `name=value` 具名参数的 token 一律抹掉**，最常见的就是根本不含 `=` 的裸 token（位置凭据）；含 `=` 时还要首个 `=` 之后非空且不全是 `=` 才算具名参数并保留（`tfo=true` 保留，`aHVudGVyMg==` 脱敏，`sni=` 属于可接受的过度脱敏；以引号开头的 token 整个算一个位置值，里面的 `=` 与逗号都不作数）。切分只在顶层逗号处进行（引号内与括号内的逗号不切，与 ② 同一套扫描）。前四种是 Surge 文档化的位置凭据写法；其余类型这个位置本就是多余参数（`W0001`），按偏安全一侧一并抹掉。列出的以外一律不脱敏 |
```

换成

```markdown
| GET | `/v1/profiles/current?sensitive=0\|1` | | `text/plain`；默认（`sensitive=0`）把下列内容替换为 `***`，其余内容与行数、行尾 CRLF 原样保留：① 独立成行的密钥 `key = value`（键名大小写不敏感）`password`、`ca-passphrase`、`ca-p12`、`private-key`、`psk`、`pre-shared-key`、`token`；② 值里任意位置的内联参数 `name = value`（值到**第一个顶层逗号**为止，顶层按解析器自己的规则判定：`"` 或 `'` 在值里**任何位置**都会开启一段引号，`"` 内 `\` 转义下一个字符、`'` 内不转义，`(` / `)` 分组，引号内与括号内的逗号都属于值——`password="p,w"`、`password=ab"c,d"`、`password=a(b,c)d` 都是一整个值；引号或括号未闭合时抹到行尾。参数名前面必须是行首、逗号、空白或 `(`）`password`、`psk`、`private-key`、`pre-shared-key`、`preshared-key`、`base64`、`token`、`uuid`、`username`、`headers`、`ws-headers`、`ws-path`、`shadow-tls-password`、`policy-path`、`external-policy-modifier`、`test-url`（后三个是阶段 2 加的：订阅链接与订阅行设的测试 URL 常带 token，修饰列表能设任何参数；`preshared-key` 是 M4b 加的，即 `[WireGuard]` 节 `peer` 里的预共享密钥；`username` 会连带脱敏无害的 SSH 用户名；`headers=` 与 `ws-headers=` 的值整体被抹掉，连 header 名也不保留，`ws-path=` 的值同样整体被抹掉——这是有意的过度脱敏，自定义 header 的值与 WebSocket 路径都属于凭据）；③ `http-api` / `external-controller-access` / `http-listen` / `socks5-listen` 的 `key@` 前缀与 `wifi-access-http-auth` 的口令；④ 所有写作 `type, server, port` 的代理类型（`http` `https` `h2-connect` `socks5` `socks5-tls` `ss` `snell` `vmess` `trojan` `tuic` `tuic-v5` `hysteria2` `masque` `anytls` `trust-tunnel` `ssh`）策略行第 4 个起的 token——**凡不是 `name=value` 具名参数的 token 一律抹掉**，最常见的就是根本不含 `=` 的裸 token（位置凭据）；含 `=` 时还要首个 `=` 之后非空且不全是 `=` 才算具名参数并保留（`tfo=true` 保留，`aHVudGVyMg==` 脱敏，`sni=` 属于可接受的过度脱敏；以引号开头的 token 整个算一个位置值，里面的 `=` 与逗号都不作数）。切分只在顶层逗号处进行（引号内与括号内的逗号不切，与 ② 同一套扫描）。前四种是 Surge 文档化的位置凭据写法；其余类型这个位置本就是多余参数（`W0001`），按偏安全一侧一并抹掉。列出的以外一律不脱敏 |
```

`docs/api/phase2.md`——把

```markdown
- 省略 `url`：每个策略按它自己的测试 URL 与超时测（策略的 `test-url` / `test-timeout`，否则 `[General]` 的 `proxy-test-url`——直连类用 `internet-test-url`——与 `test-timeout`）；结果保存，自动组随即按它选择。
```

换成

```markdown
- 省略 `url`：每个策略按它自己的测试 URL 与超时测（策略的 `test-url` / `test-timeout`，否则 `[General]` 的 `proxy-test-url`——直连类用 `internet-test-url`——与 `test-timeout`）；`wireguard` 策略没有 `dns-server` 也没写 `test-url` 时改为握手测速（见下面「`wireguard` 的测速」）；结果保存，自动组随即按它选择。
```

`docs/api/phase2.md`——把

```markdown
### 测试会话

每次测试是请求记录（`GET /v1/requests/recent`）里的一条内部会话：`listener` 为 `internal`、`rule` 为 `policy test`、`policy` 是被测的策略、目标是测试 URL 的主机与端口（不含路径与参数）；失败时 `error` 是上面 Result 里的原因。它们与 DNS 会话一样不能经 `POST /v1/requests/kill` 终止（409）。
```

换成

```markdown
### `wireguard` 的测速（M4b）

- 节里没有 `dns-server`、策略也没写 `test-url` 时，测试是一次握手：向每个 peer 强制握手，`delay` 是从发起到第一个握手完成的毫秒数（多个 peer 时最快者）；它只证明 peer 可达，不证明路由与出口（照手册）。peer 不回应时 `error` 是 `timed out`；隧道起不来时是出站的错误（如 `policy protocol not implemented: wireguard over underlying-proxy`）。
- 否则经隧道两次 `HEAD`，与其它策略相同。
- 两种都在超时之外另加 10 秒（第一次测试可能要先启动隧道）；`POST /v1/policies/test` 给了 `url` 时一律按那个 URL 测。

### 测试会话

每次测试是请求记录（`GET /v1/requests/recent`）里的一条内部会话：`listener` 为 `internal`、`rule` 为 `policy test`、`policy` 是被测的策略、目标是测试 URL 的主机与端口（不含路径与参数；`wireguard` 的握手测速是第一个 peer 的 endpoint）；失败时 `error` 是上面 Result 里的原因。它们与 DNS 会话一样不能经 `POST /v1/requests/kill` 终止（409）。
```

`docs/superpowers/specs/2026-09-27-phase2-m4-wireguard-ssh-external-design.md`——把

```markdown
| 5.3 "口令认证用 `password`" | 口令只经 SSH 的 `password` 方法发送，不走 `keyboard-interactive`：只经 `keyboard-interactive` 收口令的服务器（FreeBSD 的默认配置、部分 PAM 配置）登录失败（`ssh: authentication failed`）；已登记为差异 | 终审 |

```

换成

```markdown
| 5.3 "口令认证用 `password`" | 口令只经 SSH 的 `password` 方法发送，不走 `keyboard-interactive`：只经 `keyboard-interactive` 收口令的服务器（FreeBSD 的默认配置、部分 PAM 配置）登录失败（`ssh: authentication failed`）；已登记为差异 | 终审 |

## 19. M4b 计划期的订正

写 M4b 计划（`docs/superpowers/plans/2026-09-28-phase2-m4b-wireguard-plan.md`）时核对 boringtun 0.7.1、smoltcp 0.12.0、hickory-proto 与本仓库源码，并把全部任务在仓库副本上真实做过一遍之后，与上文不同的地方；P 编号是该计划「计划期决定」表的编号。

| 本文原文 | 计划 | 依据 |
| -------- | ---- | ---- |
| 6.6 `Datagram` 有收、发与 `set_dscp` | 收发是 `poll_send` / `poll_recv`（一个任务同时等几个载体）；`set_tos` 取 TOS 字节（0x88 即 DSCP AF41），0 回到策略自己的 `tos`；另有 `peer_addr`；`DirectConnector::connect_udp` 取解析结果的第一个地址（UDP 没有"连上"可比），收发缓冲尽量设为 7 MiB（P5） | 回环基准：Windows 默认的 64 KiB 收缓冲装不下一个 TCP 窗口的突发，丢包后 smoltcp 整窗重发 |
| 6.1 "设备任务：每个 WireGuard 出站一个" | 每个节一条隧道，指向同一个节的策略共用；一条隧道启动时结束同一私钥、有共同 peer、来自更早配置的那条，更早配置的策略不能再把它抢回去（P10） | 同一私钥连着同一 peer 的两条隧道会互相抢 peer 记住的地址（peer 回应最后写来的地址）：重载改了节、两条策略指向同一个节时都会出现 |
| 6.5 "出站对象被释放 → 设备任务结束，载体关闭，进行中的流读写返回错误" | 隧道活到出站与经它的连接都释放为止；被更新的配置接替时才立即结束、连接随之失败（P11） | 重载不打断无关的连接（M3a）；隧道之间的冲突已由 P10 处理 |
| 6.5 "地址变了就重连该载体，地址族变了就重建" | 两种一样：每 5 分钟为写成域名的 endpoint 新拨一条载体，它去的地址不同就换上并立即握手；启动时连不上的 peer 也每 5 分钟再拨（P12） | 新载体总是新 socket，两种情形没有区别 |
| 6.1 "被流的读写唤醒时立即推进一次协议栈" | 推进时反复 `poll` 到没有东西可发；收到报文时先把载体上已到的全部收下（最多 256 个）再推进（P2、P9） | smoltcp 0.12 的一次 `poll` 每条连接最多发一个报文段 |
| —（没写拥塞控制） | Reno（只开 `socket-tcp-reno`，每条连接显式设置）（P3） | smoltcp 0.12 的 Cubic 把 RFC 8312 以报文段计的窗口按字节算，窗口始终只有一两个报文段 |
| 6.4 "超过 MTU 的包不发（手册：丢弃）" | TCP 报文段按 MSS 切分，本来不会超出；更大的 IPv4 外发包由 smoltcp 分片后发出（P2） | 开了分片重组的特性，分片随之开启 |
| 6.3 的细节 | 第一个作答的服务器为准（"没有这个名字"也算）；该族没有本端地址或没有 peer 覆盖的服务器直接跳过；每个服务器最多等 2 秒；缓存最多 256 个名字、最长 1 小时、只存有地址的结果（P14） | 设计只写了原则 |
| 第 9 节"WireGuard 的握手成功 / 失败" | 成功只在第一次与失败之后恢复时记（`info`），失败在 boringtun 放弃重试时记一次（约 90 秒后，`warn`）；boringtun 自己的日志在 bin 里关掉（P13） | 流量不断时每两分钟换一次密钥，每次都记会刷屏；boringtun 的日志不带策略名 |
| 4.3 "`underlying-proxy` → `W0029`" | `W0029` 用专门的说法：`` `underlying-proxy` does not work with `wireguard` policies in this version; the policy rejects every connection ``；拨号时的 `Unsupported` 由引擎写进请求记录（此前引擎只在解析期写这类说明）（P17） | 通用说法"没有效果"不对：策略会 REJECT |
| 4.7 "`preshared-key` 已在名单里" | 不在，M4b 加上（P16） | `redact.rs` 的名单 |
| 4.1 没写重名的节 | 同名的节只用第一个，`W0020`（P15）；peer 的 endpoint 主机名进"代理服务器主机名"集合，`[Host]` 不作用于它 | 与重名策略、代理服务器主机名一致 |
| 第 16 节草图：7 测速、8 引擎 | 7 引擎、8 测速（P18） | 原生测速的端到端用例要经引擎 |

```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| 用户态协议栈 | `smoltcp`，作为 `rurge-proto-wireguard` 的内部模块（见 D9） | M4 | TCP 吞吐基准 |
```

换成

```markdown
| 用户态协议栈 | `smoltcp`，作为 `rurge-proto-wireguard` 的内部模块（见 D9）；M4b 用 0.12.0，拥塞控制 Reno | M4 | TCP 吞吐基准 |
```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| G | smoltcp 的 TCP 吞吐 | WireGuard 出站性能 | M4 做基准；接口收窄以便替换；阶段 3 复核 |
```

换成

```markdown
| G | smoltcp 的 TCP 吞吐 | WireGuard 出站性能 | M4 做基准；接口收窄以便替换；阶段 3 复核。M4b 实测（Windows 11 回环、两端都是 smoltcp 的双向回显）约 6.5 MiB/s 每方向；0.12 另有回退 N 重传、没有零窗口探测等限制（M4 设计第 19 节、M4b 计划 P3） |
```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| Q4 | sing-box 的用户态 WireGuard 端点能否充当带保留字节的对端 | M4 细化设计时验证；不行则用 boringtun 写回环对端 |
```

换成

```markdown
| Q4 | sing-box 的用户态 WireGuard 端点能否充当带保留字节的对端 | M4b 已决：sing-box 1.14.1 的 `endpoints`（`type: wireguard`，`system: false`）以 `peers[].reserved` 给发往 rurge 的报文写保留字节，可以充当；它的 UDP 端口开在所有地址上（端点没有监听地址这一项）。互操作用例只在 CI 上跑；回环对端另用 boringtun 写（`FakeWgPeer`） |
```

- [ ] **Step 9: 本计划末尾的两张表**

把控制者给出的行写进本计划末尾的「执行期修正记录」与「延后事项」两张表（没有要补的行时保持原样）。

- [ ] **Step 10: 门禁与提交（文档）**

跑门禁（只改了文档：49 个测试二进制，1121 通过 / 2 忽略）。

```bash
git add docs README.md README_en.md CLAUDE.md
git commit -m "docs: M4b WireGuard——兼容性清单、README、CLAUDE.md、手工验收、API 文档、M4 设计第 19 节与总设计 Q4"
```

---

## 验收对照（设计第 11 节，WireGuard 部分）

| # | 验收项 | 由谁保证 |
| - | ------ | -------- |
| 1 | SSH 经 `FakeSsh` 动态转发 | 不在本计划（M4a） |
| 2 | WireGuard 与带 `client-id` 的端点握手并转发 TCP：回环 `FakeWgPeer` 必过；对 sing-box 的互操作在 CI 上通过 | Task 3：`the_client_id_goes_out_in_every_message_and_is_cleared_coming_in`、`without_the_client_id_such_a_peer_never_answers`；Task 4：`a_connection_through_the_tunnel_echoes`、`every_message_carries_the_client_id`；Task 7：`outbounds_wireguard` 四条；Task 9：`a_connection_goes_through_a_sing_box_wireguard_endpoint`（CI） |
| 3 | `external` 的再拉起与进程树清理 | 不在本计划（M4c） |
| 4 | `W0007` 不再因 `wireguard` 出现；订阅安全门有用例 | Task 10：`check_knows_wireguard`；Task 1：`a_subscription_wireguard_line_may_not_use_the_profiles_sections`（`external` 在 M4c） |
| 5 | 门禁全绿（fmt / clippy 零警告 / `cargo test --workspace`） | 各任务的门禁 |
| 6 | 需要真实环境的项目进手工验收清单 | Task 10：`docs/acceptance/phase2-manual.md` 的 M4b 一节（自建 WireGuard、WARP 的 `client-id`、吞吐、重载、endpoint 跟随），由项目所有者验收 |
| — | 第 10 节第 2 层的 WireGuard 各项 | 握手与回显、两个 peer 的选路、没有路由的错误、`client-id`、内层源地址、分片重组、ICMP：Task 3；本机解析、关着的端口、不回应的 peer、同时的拨号、隧道的寿命、握手包的 TOS：Task 4；隧道内 DNS：Task 5；重拨、网络变化、共用与接替：Task 6；`underlying-proxy` 的 REJECT、重载沿用、经引擎的端到端：Task 7；原生测速与经隧道的 URL 测试：Task 8 |
| — | 第 10 节的吞吐基准 | Task 7 Step 5（不作门禁，数字记进下表） |

## 执行期修正记录

| 任务 | 计划原文 | 实际做法 | 原因 | 提交 |
| ---- | -------- | -------- | ---- | ---- |
| 3 | `self-ip` / `self-ip-v6` 只检查能否解析成地址 | 只收单播地址，组播、广播、未指定地址是 `E0023`；新增用例 `self_ips_are_unicast_addresses` | smoltcp 对非单播的接口地址 panic：能通过 `rurge check` 的配置第一次拨号就 panic（评审发现） | d461af7 |
| 4 | 批量收时持协议栈的锁读载体（P9 的代码） | 锁只包住 `Stack::receive`，载体在锁外读；Task 6、8 的 `device.rs` 块随之改写 | 收报文的系统调用进了锁，违反 M4-D5（评审发现） | f4818dc |
| 5 | `Device::query` 先建 `UdpExchange` 再发送；`dns-server` 只挡 IPv4 组播 | 发送成功后才建；发送失败在持锁时关掉套接字、换下一个服务器；`dns-server` 不收两族组播、未指定地址与端口 0；新增用例 `a_dns_server_the_stack_cannot_send_to_is_passed_over`、`dns_servers_are_addresses_a_question_can_go_to` | 发送失败时 `UdpExchange` 的 `Drop` 在持锁时再次加锁，线程卡死并逐步拖住整个进程（评审发现）；P15 写的是组播一律不收 | 7373344 |
| 6 | 按节共用隧道；全局启动锁 `STARTING`；`close()` 只中止任务 | 按节与载体键共用（`WireGuardOutbound::with_carrier`）；去掉全局启动锁：先在锁外拨载体，再在隧道表的一次临界区里决定共用、拒绝或接替；被接替的设备任务不再发送；新增三条用例 | 只按节共用会让带 `underlying-proxy` 的策略共用直连隧道、让只改策略行的重载不生效；全局锁让启动排队、两条隧道可能互等到超时；中止不抢占正在被轮询的任务（评审发现） | c1addb4 |
| 7 | 工厂分支 `WireGuardOutbound::new(…)` | 另设载体键 `.with_carrier(…)`：`[General] ipv6`、`ip-version`、`underlying-proxy`；新增端到端用例 `a_policy_over_underlying_proxy_never_shares_the_tunnel`、`a_reload_that_changes_the_carriers_takes_the_tunnel_over`；Task 7、8 的 brief 按重放后的副本重新生成 | Task 6 修正轮的裁决 | 3744f9c |
| 7 | 吞吐基准：写计划时约 9.8 秒（6.5 MiB/s 每方向） | 实测 64 MiB 双向 10.99 秒（5.8 MiB/s 每方向） | 同一量级 | 3744f9c |
| 8 | 原生测速用例的两次等待没有时限 | 两处都设 5 秒时限 | 违反"有界等待"（评审发现） | 45d54d3 |
| — | P3 ① 的理由："0.12 的 Cubic 把窗口压在一两个报文段" | 理由改正：0.12 从不拿拥塞窗口与在途字节数比较（tcp.rs:2159，报文段大小也不看它，tcp.rs:2418-2420），两种算法都不限制发送；仍显式设 Reno | 评审对照 smoltcp 0.12.0 源码 | — |
| 各任务 | 门禁通过数：1060 / 1065 / 1079 / 1090 / 1098 / 1104 / 1112 / 1117 / 1120 / 1121 | 实际：1060 / 1065 / 1080 / 1091 / 1101 / 1110 / 1120 / 1125 / 1128 / 1129（2 个忽略自 Task 7 起） | 修正轮与补充的用例 | — |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | smoltcp 0.12 的 TCP 限制（P3 ② ～ ⑤）：丢包后整窗重发、发送方没有零窗口探测、对端关闭之后在途数据丢失不再重传、不开 keep-alive、拥塞窗口实际不限制发送（0.12 从不与在途字节数比较）。极少数上传可能卡到空闲超时 | MSRV 升到 1.91 时改用 smoltcp 0.13 或更新版本，届时逐条核对（项目所有者决定）；已登记为差异 |
| 2 | 经 WireGuard 的 UDP、`underlying-proxy`、`ecn` | M5 |
| 3 | "网络已变化"的探测器（入口已有，P12） | 阶段 3 |
| 4 | 对 sing-box WireGuard 端点的互操作只在 CI 上真正运行，本机验证不了（P8） | 首次推送后看 CI |
| 5 | 每个报文的开销以 Windows 的 UDP 系统调用为主（P19）；没做批量收发（`recvmmsg` / GSO / GRO） | 有用户报告吞吐问题、或阶段 3 做 TUN 时再看 |
| 6 | （已取消）隧道逐个启动（`STARTING`）——Task 6 修正轮去掉了全局启动锁，见执行期修正记录 | — |
| 7 | 改了节或载体设置的重载进行时，还在用旧配置的拨号或测试得到 `wireguard: a newer configuration of this tunnel is in use`（P10，有意为之：不来回争抢） | 不处理 |
| 8 | 隧道内 DNS 不查非 ASCII 的名字（P4） | M8（IDN） |
