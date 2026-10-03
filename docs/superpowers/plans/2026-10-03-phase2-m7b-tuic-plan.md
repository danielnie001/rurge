# 阶段 2 / M7b「TUIC v5 与 v4」Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `tuic-v5` 与 `tuic`（v4）两种出站：TCP、UDP（QUIC 数据报或单向流，自动选择）、心跳、`port-hopping`、`ecn`，建在 M7a 的 QUIC 地基与会话之上。

**Architecture:** `rurge-config::spec` 新增 `TuicSpec`（`TuicVersion::{V4 { token }, V5 { uuid, password }}`，一个 spec 管两个关键字）。`rurge-proto-quic` 新增 `tuic` 模块：`auth`（v5 用 TLS 导出密钥算 token，v4 用 BLAKE3；各自的关闭码与"口令错"的判断）、`addr`、`tcp`（v5 发完 Connect 即转发，v4 等 `04 ff` 应答）、`udp`（每条连接的关联、v5 的分片与重组、数据报或单向流两种模式、单向流由连接自己的写入任务逐条写完、`Dissociate`）、`heartbeat`（有转发时 v5 每 3 秒一个数据报、v4 每 10 秒一条单向流），两个版本共用一套实现、在这几处按版本区分；`FakeTuic` 同时扮演两个版本的服务端。引擎工厂装上它，能力表翻转；互操作对 sing-box 1.14.2 的 `tuic` 入站（v5）与官方 tuic 0.8.5 服务端（v4，只在 Linux CI）。

**Tech Stack:** Rust 1.89 / edition 2024；**不新增外部 crate**：quinn（M7a）、`blake3`（工作区依赖）、`getrandom` 已在依赖树里；UUID 解析复用 `rurge-config` 里 vmess 的 `parse_uuid`。

**Spec:** `docs/superpowers/specs/2026-10-02-phase2-m7-quic-design.md`（M7-D1 ～ D12；第 3 节共用部分与第 14、15 节的订正；第 6 节 TUIC；第 8、9 节中 M7b 的部分；第 11 节 V1、V4、V5、V7、V10；第 12 节 M7b 草图）。与本计划「计划期决定」表不一致处，以该表为准；执行开始时一并写进设计文档新增的第 16 节。

**基线：** main 8548d51（M7a 已合并）。开工时 main 若有了别的提交，按新的 main 重新生成本计划。

## Global Constraints

- MSRV **1.89**（CI 与本机都钉在 1.89.0），edition 2024。`unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 的两个函数例外（本计划不碰）。**本计划不新增任何 unsafe**。
- 依赖方向不变：`rurge-engine → rurge-proto-quic → { rurge-proto, rurge-net, rurge-config }`。**不新增外部 crate**；不给已有 crate 引入第二个大版本。
- **测试绝不碰公网**：只用回环 + 端口 0 + 有界等待；不用固定 sleep 当同步手段（只有"断言这段时间里什么也没发生"本身是被测条件时才等一段固定时间）。**任何带 `url-test` / `fallback` / `load-balance` / `smart` 组的测试配置，`proxy-test-url` 与 `internet-test-url` 都必须指向回环**。回环 UDP 的单个载荷不超过 **9216 字节**（macOS 的缺省上限）。
- **测试与任何命令都不得修改本机的系统代理、注册表、网络设置，不得注册真实服务 / 计划任务**：不在 CLI 测试夹具之外运行 `rurge run --system-proxy`；不运行不带 `--dry-run` 的 `rurge service install | uninstall`。
- **不在本机下载或安装任何东西**——哪怕只是为了算一个校验和也不行（不装 sing-box、tuic-server，不 `rustup target add`、不 `cargo install`）。互操作用例在本机没有二进制时按既有约定跳过。TUIC 的参考实现是 GPL：只取协议事实，不抄代码；tuic 0.8.5 的服务端二进制只作为外部进程运行。
- **凭据与载荷永不外泄**：`uuid`、`password`、`token` 与 TCP / UDP 的载荷不进日志、错误文本与 `Debug`（配置里用 `Secret<T>`）。
- 锁里只做内存操作，不跨 `.await`；后台任务（心跳、单向流的读写、数据报分发、`Dissociate`）随所属的连接或对象结束，不留孤儿任务。
- M7a 的 quinn 规矩照旧：各层发送失败（`WouldBlock` 以外）一律丢包、不报给 quinn；QUIC 的 TLS 只用 1.3；只对仍开着的连接调用 quinn 的 `close`。M7a 评审的并发教训照旧：同一个 UDP 载体不让两个包的分片交错（每个载体一把发送锁）；UDP 载体至多每秒比较一次底层策略。
- rurge 专有的运行时选项只经命令行与环境变量提供，不扩展 Surge 配置格式（FR-CFG-17）。与 Surge 的每一处行为差异都登记进 `docs/surge-compatibility-matrix.md`（Task 6）。
- 日志、错误文本、CLI 输出、代码注释用英文；文档与提交标题用中文。注释密度、命名、惯用法与相邻代码保持一致；注释里不写评审轮次的标签。
- 提交信息结尾两行：`Co-Authored-By: <执行者自己的署名>` 与 `Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th`。**不 push、不 merge**，不 amend。每个提交都带上它改动的 `Cargo.lock`（若有）。
- 每个任务结束的门禁（全部通过才算完成）：

  ```bash
  RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check \
    && cargo clippy --all-targets -- -D warnings \
    && timeout 1500 cargo test --workspace --no-fail-fast
  ```

  `timeout` 不能省（M3a「延后事项」#20）。测试二进制异常退出而没有失败用例时（`STATUS_ACCESS_VIOLATION`、`STATUS_HEAP_CORRUPTION` / `0xc0000374`、段错误——本机已知的既有问题），或整轮被 `timeout` 杀掉时，重跑一次并保留两次的日志，**不要在任务里去修它**。已知偶发失败的用例（`rurge-dns` 的 `a_partial_result_completes_aaaa_in_the_background` 与 `bootstrap::tests::stale_entries_are_served_and_refreshed_once`、`rurge` 的 `run::watch_reloads_rules_on_change` 与 `run::run_system_proxy_is_applied_switched_and_restored`、`rurge-engine` 的 `udp::a_closed_port_does_not_break_the_carrier`）同样重跑。**链接器报 LNK1180 / LNK1318 / LNK1285 或 "no space on device" 时是磁盘满了**：先看 `df -h /d`，删 `target/debug/incremental` 与 `target/debug/deps` 里旧的 `*.exe` / `*.pdb` 再重跑。
- 第三方 crate 的 API 以本机源码为准：`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/` 下的 `quinn-0.11.12`、`quinn-proto-0.11.19`、`rustls-0.23.43`、`blake3-1.8.7`。
- 本机的 bash 处理不了超过约 8 KB 或含反斜杠的 heredoc（`\\` 会被改写）：新文件与含反斜杠的改动一律用写文件的工具落盘，不用 heredoc。

## Review Focus

设计没有逐条写到、而最可能伤到使用者的五类输入或失败方式；每一条都在负责它的任务里配了用例。

1. **口令或 token 写错**：v5 的服务端（sing-box、tuic-server）对一切失败都以码 0 关连接，单看关闭认不出；用户应当看到 `tuic: authentication failed`，而不是一条莫名断掉的隧道或静默丢掉的 UDP。用例：Task 2 `a_refused_authentication_ends_the_tunnel_and_the_next_dials_again`；Task 3 `a_refused_authentication_fails_a_send_once_and_the_next_binds_anew`；Task 4 `v4_a_wrong_token_fails_the_request_and_the_next_dials_again`、`v4_the_servers_close_codes_say_what_went_wrong`；Task 5 `a_refused_password_fails_the_session`、`a_refused_token_fails_the_session`。
2. **服务端不收 QUIC 数据报（UDP 走单向流）而引擎取消了一次发送**：被截短的单向流会让 sing-box 与 0.8.5 关掉整条连接（同一连接上的 TCP 一起断）。用例：Task 3 `a_send_given_up_on_never_cuts_its_packet_short`；Task 4 `v4_a_send_given_up_on_never_cuts_its_packet_short`；Task 5 `udp_goes_on_streams_where_the_server_takes_no_datagrams`。
3. **服务端的空闲超时比 QUIC 保活短**（tuic-server 缺省 10 秒）：有转发时要有心跳，没有转发时不发。用例：Task 3 `heartbeats_go_while_a_relay_is_under_way_and_stop_when_none_is`；Task 4 `v4_heartbeats_go_on_streams_while_a_relay_is_under_way`。
4. **大的 UDP 包**：v5 放不下就分片、丢一片只丢这个包；v4 不分片，放不下的包单独丢弃、流照常。用例：Task 3 `long_packets_go_in_fragments_both_ways`、`a_lost_fragment_loses_its_packet_alone`；Task 4 `v4_a_packet_too_long_for_a_datagram_is_refused_and_nothing_goes_out`；Task 5 `v4_udp_goes_whole_in_datagrams_or_not_at_all`。
5. **v4 服务端以 IPv4 映射的 IPv6 地址报回包来源**（0.8.5 的双栈 socket）：要按 IPv4 读，否则核对来源的客户端（例如 DNS 解析器）会丢掉应答。用例：Task 4 `an_ipv4_address_in_ipv6_form_reads_as_the_ipv4_address`。

## 计划期决定

写计划时对照设计、TUIC v5 的 `SPEC.md` 与 v4（0.8.5）的协议说明、sing-box 的 `sing-quic/tuic` 与 tuic-server 1.0.0 / 0.8.5 的源码（都是 GPL，只读、只取协议事实）、quinn 0.11.12 / rustls 0.23.43 的本机源码、Surge 手册（`policies/tuic.html`，2026-10-03 查阅）与本仓库源码后定下的事；与设计文档文字不同的，写进设计文档第 16 节。

**本计划里的代码不是凭空写的。** 全部 6 个任务的改动在仓库的一份副本上按任务顺序真实做了一遍（副本建在 8548d51 上，用自己的构建目录），最后一次全工作区门禁见 Task 6 的 Step 5。计划里新文件的全文取自副本上该任务的提交，修改处的"把 … 换成 …"由脚本从相邻两个任务提交的差异生成，并在拼好之后按计划的顺序套到开工前的源码上逐字核对过。有单独失败步骤的任务（1、3、4、5），Step 2 的"预期失败"是只把该任务 Step 1 的内容套到上一个任务的状态上、真实跑出来的；Task 2 是自带用例的新模块，Task 6 是互操作与文档。执行时按 M7a 的做法用转写脚本把每一步的代码块原样写入，不手工重打。

| # | 事项 | 决定与依据 |
| - | ---- | ---------- |
| P1 | 一个 spec 管两个关键字（细化设计 6.1） | `TuicSpec { tls, quic, version }`，`TuicVersion::V4 { token }` / `V5 { uuid, password }`，凭据都是 `Secret`；UUID 原样存 16 字节，复用 vmess 的 `parse_uuid`（接受带连字符的写法，也接受 32 位裸十六进制，同 vmess）。缺了或格式不对是 `E0018`，不引用取值；位置值 `W0001` |
| P2 | 诊断（细化 6.1 与第 14 节第 2 条） | 写了另一个版本的凭据（`tuic-v5` 行上的 `token`、`tuic` 行上的 `uuid` / `password`）是 `W0028` "only applies to … policies; ignored"；`udp-relay` 与 `tfo` 是 `W0028`；`ecn` 生效、不再是 `W0029`；`port-hopping` × `underlying-proxy` 与 Shadow TLS × TUIC 的两条 `E0018` 真正报出（后者 M2c 的判断函数本来就排除两种 TUIC）。写错的 `tuic` / `tuic-v5` 行从此是加载错误（原先只有 `W0007`）。`required` 小函数从 `spec/h2.rs` 移到 `spec/mod.rs`，`trust-tunnel` 与 TUIC 共用 |
| P3 | 分步落地的门 | 同 M6 / M7a：Task 1 里 TUIC 行读完并检查但暂不产出 spec，引擎工厂先放 `BuildError` 分支；Task 5 去掉 |
| P4 | v5 的认证（细化 6.2，SPEC 核对） | token = TLS 导出密钥（label = UUID 原始字节，context = 口令，32 字节）；`05 00 ‖ UUID ‖ TOKEN` 共 50 字节写在自己的单向流上、写完即关，不等服务端，也不另起任务，所以总在连接的第一个请求之前发出 |
| P5 | v5 的"口令错"（V4，订正 6.2 的"能认出就报"） | sing-box 与 tuic-server 1.0.0 对一切失败（口令错、3 秒认证超时、坏命令、自己关机）都以码 0、无理由关连接，单看关闭认不出。rurge 只在服务端还没转发过任何字节就关闭时报 `tuic: authentication failed`，否则报 `tuic: the server closed the connection (code 0x…)`。对 sing-box，口令错还可能先表现为隧道读到空就结束、或写时 `the server stopped the stream (code 0x0)`（登记）；经引擎时 CONNECT 先回 200、隧道随后断开（v5 的 Connect 没有应答） |
| P6 | v5 的 TCP | Connect 一写进流就把隧道交给调用方（没有应答）；"失效连接重试一次"只在 Connect 交给 QUIC 之前就发现连接已失效时适用。`NAME = "tuic"`、`NO_ERROR = 0`（tuic-server / client 1.0.0 与 sing-box 都用 0） |
| P7 | v5 的 UDP（细化 6.2） | 服务端收 QUIC 数据报就用 `native`，否则每个包一条单向流（`quic` 模式），两种回包都收。关联号按连接从 0 计、跳过正在用的；载体换到新连接时在那里另取一个号（不同于 Hysteria 2 按出站终生不变的会话号）。`Dissociate` 由连接自己的任务在关联结束、连接仍开着时发出（含载体离开底层策略变了的连接时），丢掉载体从不等待。每个关联至多同时重组 8 个包；服务端发来的坏消息丢弃、连接照旧。口令被拒时一次 UDP 发送报 `tuic: authentication failed`，UDP 不会静默黑洞 |
| P8 | 发送锁与路由比较（承接 M7a） | 每个载体一把发送锁，分片不交错——sing-box、tuic-server 与现在的 mihomo 都按包号重组，交错对它们无害，但早期的 Clash.Meta TUIC v5 一次只重组一个包（去掉锁时并发用例 20 次失败 18 次）。UDP 载体至多每秒比较一次底层策略（M7a 的做法） |
| P9 | 单向流由连接的写入任务写（写计划时发现） | `quic` 模式下，每个包在 `send_to` 里就地写一条单向流时，引擎取消发送会把流截短，sing-box（v5）与 0.8.5（v4，截在命令头里）随即关掉整条连接。改为：每条连接一个写入任务，按顺序一次写完一条流，由 `OUTBOX` = 64 的队列喂给它；队列满或已关即丢掉这个包、记 debug 日志、`send_to` 照常返回成功（与 `native` 模式丢包一样）；写失败只丢这一个包，从不关连接。代价（登记）：`quic` 模式下引擎的发送时限不再起作用，服务端若不再读流，UDP 静默丢包，同一连接上的 TCP 不受影响 |
| P10 | 心跳（细化 6.2 / 6.3） | v5：有 TCP 隧道或 UDP 关联时每 3 秒一个 `05 04` 数据报，都没有时停；服务端不收数据报时不发（sing-box 收到走流的心跳会关连接）。v4：有转发时每 10 秒一条 `04 04` 单向流，不论服务端收不收数据报（0.8.5 收到不是 Packet 的数据报会关连接）。sing-box 在认证前就发心跳，所以心跳不能当作登录成功的证据；它把认证前到达的数据报排在 quic-go 的队列里（至多 128 个），所以 UDP 不等认证流写完 |
| P11 | v4（细化 6.3） | 与 v5 共用一套实现，只在认证、TCP 应答、UDP 包格式、关联号、`Dissociate`、心跳与关闭码这几处按版本区分。token = 原文 UTF-8 的 BLAKE3（已知答案用照规范自写、先对上官方向量的 BLAKE3 算出）。TCP 收到 `04 ff 00` 才交出隧道，`ff` 报 `tuic: the server could not connect to the target`、连接照用；失效连接规则同 Hysteria 2（等应答期间连接丢了算 `Failure::Lost`）。关闭码 `0xfffffff1` / `0xfffffff2` → 认证失败 / 认证超时（错误种类 `PermissionDenied`，免得被引擎改写成 "connect timed out"），`0xfffffff0` / `0xfffffff3` → `tuic: protocol error`，其它同 v5 的通用文本；v4 不用 P5 的推断。UDP 包放不进一个数据报时 `InvalidInput`、什么也不发（引擎只丢这一个包、在流的记录上注明），不改走单向流（0.8.5 会因此关连接）；关联号是随机 `u32`、跳过正在用的 |
| P12 | IPv4 映射的来源（写计划时发现） | 0.8.5 的双栈 socket 把 IPv4 来源报成 `::ffff:…`：回包来源按 IPv4 读（`to_canonical`），对 v5 同样适用 |
| P13 | 引擎（Task 5） | 一个工厂分支为两个版本构建 `TuicOutbound`；端口跳跃遇上链式代理由 M7a 的 `Dialer::new` 统一拒绝、组级派生的 `M (via R)` 只略去这一条、加载期 `W0023` 照样报出（由 `check_knows_tuic` 钉住）。没有用例拿 TUIC 当"未实现协议"的例子（都已是 `tailscale`）。`outbounds_hysteria2.rs` 里四个与协议无关的辅助函数移进 `tests/common` |
| P14 | 互操作（M7-D6） | sing-box 1.14.2 的 `tuic` 入站（只有 v5；只监听回环、夹具的 CA、`alpn: ["h3"]`、一个用户）；官方 tuic 0.8.5 服务端（v4）只经 `RURGE_TEST_TUIC_SERVER` 找（不查 PATH——PATH 上多半是只说 v5 的 1.x），日志出现 `Server started` 即就绪，配置只写 0.8.5 认识的 7 个键（`alpn` 必须写 `["h3"]`，它的缺省为空）。两个参考服务端都总收数据报、没有开关，`quic` 模式只能靠假服务端与手工验收 |
| P15 | 0.8.5 的 SHA-256（V5，待项目所有者定） | GitHub 发布资产的 `digest` 都是 null；CI 步骤里 `sha=` 是明确标出的占位值，填上真实值之前 Linux 的 CI 任务会在这一步失败。发布里有对应的 `.sha256sum` 文本资产——见文末「延后事项」#1 |

## 承接事项

| # | 来源 | 事项 | 处理 | 任务 |
| - | ---- | ---- | ---- | ---- |
| C1 | M7 设计第 12 节 | M7b 的全部内容 | 本计划 | 1–6 |
| C2 | M7-D3 | TUIC v4 也实现；自动化以假服务端为主，Linux CI 对 0.8.5 | 本计划（P11、P14） | 4、6 |
| C3 | M7a 的评审（P8 / 第 15 节） | 分片不交错、UDP 至多每秒比较一次底层策略 | 照做（P8） | 3、4 |
| C4 | M3b #7（P21） | 测试二进制偶发崩溃 | 照旧：门禁遇到就重跑 | — |

## File Structure

新建：

| 文件 | 职责 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/spec/tuic.rs` | `TuicSpec`、`TuicVersion`、`read_tuic`（与用例） | 1 |
| `crates/rurge-proto-quic/src/tuic/{mod.rs, auth.rs, addr.rs, tcp.rs}` | `TuicOutbound`、认证、地址编码、TCP（与用例） | 2、3、4 |
| `crates/rurge-proto-quic/src/tuic/{udp.rs, heartbeat.rs}` | UDP（两种模式、分片与重组、关联、写入任务、`Dissociate`）、心跳（与用例） | 3、4 |
| `crates/rurge-proto-quic/src/test_log.rs` | 测试用的日志捕获（从 hysteria2 的用例里移出、共用） | 2 |
| `crates/rurge-proto-quic/src/testing/tuic.rs` | `FakeTuic`（v5 与 v4） | 2、3、4 |
| `crates/rurge-engine/tests/outbounds_tuic.rs` | 经引擎的端到端用例 | 5 |
| `tests/interop/src/tuic_server.rs`、`tests/interop/tests/tuic.rs` | tuic 0.8.5 的夹具、对 sing-box 与 0.8.5 的互操作用例 | 6 |

修改：`crates/rurge-config/src/spec/{mod.rs, h2.rs, vmess.rs}`、`src/redact.rs` 与配置的用例（1、5）；`crates/rurge-engine/src/outbounds.rs`（1、5）；`crates/rurge-proto-quic/{Cargo.toml, src/lib.rs, src/hysteria2/mod.rs, src/testing/mod.rs}`（2、3、4）；`crates/rurge-engine/tests/{common/mod.rs, outbounds_hysteria2.rs}`、`crates/rurge/{src/capabilities.rs, tests/cli.rs}`（5）；`tests/interop/{src/lib.rs, README.md, tests/common/mod.rs, tests/sing_box_hysteria2.rs}`、`.github/workflows/ci.yml` 与文档（6）。

## 任务一览

| 任务 | 交付物 | 依赖 |
| ---- | ------ | ---- |
| 1 | `TuicSpec`、两条 `E0018`、诊断 | — |
| 2 | v5 的认证与 TCP、`FakeTuic` | 1 |
| 3 | v5 的 UDP 与心跳 | 2 |
| 4 | v4 | 3 |
| 5 | 引擎装配、能力表翻转、经引擎的用例 | 1–4 |
| 6 | 互操作（sing-box、tuic 0.8.5）与文档 | 1–5 |

---

### Task 1: TUIC 的配置

`TuicSpec` 与 `TuicVersion`（P1）；诊断与两条 `E0018`（P2）；TUIC 行读完并检查但暂不产出 spec，引擎工厂先放占位分支（P3）。

**Files:**
- Create: `crates/rurge-config/src/spec/tuic.rs`（自带用例）
- Modify: `crates/rurge-config/src/spec/mod.rs`（与用例）、`src/spec/h2.rs`（`required` 移走）、`src/spec/vmess.rs`（`parse_uuid` 改为 `pub(crate)`）、`src/redact.rs`（用例）、`tests/policy_spec.rs`、`crates/rurge-engine/src/outbounds.rs`

**Interfaces:**
- Consumes: M7a 的 `read_quic_tls` / `read_quic`、`QuicOpts`、`ProtoSpec::quic()`；vmess 的 `parse_uuid`；既有的诊断码。
- Produces: `rurge_config::spec::tuic`：`pub struct TuicSpec { pub tls: TlsOpts, pub quic: QuicOpts, pub version: TuicVersion }`、`pub enum TuicVersion { V4 { token: Secret<String> }, V5 { uuid: Secret<[u8; 16]>, password: Secret<String> } }`、`read_tuic`；`ProtoSpec::Tuic`（`tls()` / `quic()` 各有分支）；`spec` 模块里的私有函数 `required`（原在 `h2.rs`，子模块共用）

- [ ] **Step 1: 先写用例**

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        assert!(tcp.proto.quic().is_none() && ProtoSpec::Direct.quic().is_none());
    }
}
```

换成

```rust
        assert!(tcp.proto.quic().is_none() && ProtoSpec::Direct.quic().is_none());
    }

    /// `tuic` and `tuic-v5` lines are read and checked in full; they have no
    /// spec until the engine builds TUIC (M7b task 5). `ecn` takes effect on
    /// them, `tfo` and `udp-relay` do not apply, and neither version takes
    /// the other's credentials (phase 2 M7 design 3.1, 6.1).
    #[test]
    fn a_tuic_line_is_checked_but_has_no_spec_yet() {
        let id = "0233d11c-15a4-47d3-ade3-48ffca0ce119";
        for def in [
            "tuic, 192.168.20.6, 443, token=pwd, alpn=h3".to_string(),
            format!("tuic-v5, 192.168.20.6, 443, uuid={id}, password=pwd, alpn=h3"),
            format!(
                "tuic-v5, h.test, 443, uuid={id}, password=pw, port-hopping=1234;5000-6000, port-hopping-interval=10, ecn=off, sni=edge.test, client-cert=cert1"
            ),
            "tuic, h.test, 443, token=t, ecn=on, underlying-proxy=Pick".to_string(),
            "tuic, h.test, 443, token=t, port-hopping=5000-6000, underlying-proxy=DIRECT"
                .to_string(),
        ] {
            let o = outcome("T", &def);
            assert!(o.diagnostics.is_empty(), "{def}: {:?}", o.diagnostics);
            assert!(o.inert.is_empty(), "{def}: {:?}", o.inert);
            assert!(o.spec.is_none() && o.not_implemented.is_none(), "{def}");
        }
        let o = outcome(
            "T",
            "tuic-v5, h.test, 443, s3cret, tfo=true, udp-relay=true, token=t, mystery=1",
        );
        let found: Vec<(&str, &str)> = o
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `T`: `tfo` does not apply to `tuic-v5` policies; ignored"
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `T`: `uuid` is required"
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `T`: `password` is required"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `T`: `token` only applies to `tuic` policies; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `T`: `udp-relay` does not apply to `tuic-v5` policies (UDP is always relayed); ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `T`: unknown parameter `mystery` ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `T`: unexpected positional value #1 ignored"
                ),
            ]
        );
        assert!(o.inert.is_empty(), "{:?}", o.inert);
    }

    /// What a QUIC-based line cannot combine (phase 2 M7 design 3.1), on
    /// either TUIC version: port hopping with a chain, and Shadow TLS (the
    /// manual: a configuration error).
    #[test]
    fn port_hopping_over_a_chain_and_shadow_tls_are_errors_on_tuic() {
        let v5 = "uuid=0233d11c-15a4-47d3-ade3-48ffca0ce119, password=pw";
        for (kind, credentials) in [("tuic", "token=t"), ("tuic-v5", v5)] {
            for relay in ["Entry", "Pick"] {
                let o = outcome(
                    "T",
                    &format!(
                        "{kind}, h.test, 443, {credentials}, port-hopping=5000-6000, underlying-proxy={relay}"
                    ),
                );
                let found: Vec<(&str, &str)> = o
                    .diagnostics
                    .iter()
                    .map(|d| (d.code, d.message.as_str()))
                    .collect();
                assert_eq!(
                    found,
                    [(
                        codes::E_INVALID_POLICY_PARAM,
                        "policy `T`: `port-hopping` cannot be combined with `underlying-proxy`"
                    )],
                    "{kind} {relay}"
                );
            }
            let o = outcome(
                "T",
                &format!(
                    "{kind}, h.test, 443, {credentials}, shadow-tls-password=st, shadow-tls-version=3, shadow-tls-sni=site.test"
                ),
            );
            let found: Vec<(&str, String)> = o
                .diagnostics
                .iter()
                .map(|d| (d.code, d.message.clone()))
                .collect();
            assert_eq!(
                found,
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    format!("policy `T`: Shadow TLS cannot be combined with a `{kind}` policy")
                )]
            );
        }
    }

    /// TUIC runs TLS over QUIC, whichever the version: the TLS options, and
    /// with them the client certificate, and the QUIC options are its own.
    #[test]
    fn a_tuic_spec_has_tls_and_quic_options() {
        for version in [
            TuicVersion::V4 { token: "t".into() },
            TuicVersion::V5 {
                uuid: Secret::new([7; 16]),
                password: "pw".into(),
            },
        ] {
            let tuic = ProtoSpec::Tuic(TuicSpec {
                tls: TlsOpts {
                    alpn: vec!["h3".into()],
                    client_cert: Some("cert1".into()),
                    ..TlsOpts::default()
                },
                quic: QuicOpts {
                    port_hopping: Some(PortHopping {
                        ports: vec![5000..=6000],
                        interval: quic::DEFAULT_HOP_INTERVAL,
                    }),
                },
                version,
            });
            assert_eq!(tuic.keystore_item(), Some("cert1"));
            assert_eq!(
                tuic.tls().map(|tls| tls.alpn.clone()),
                Some(vec!["h3".to_string()])
            );
            assert!(tuic.quic().is_some_and(|quic| quic.port_hopping.is_some()));
        }
    }
}
```

`crates/rurge-config/src/redact.rs`——把

```rust
            "H = hysteria2, h.test, 443, password=***, gecko-password=***"
        );
    }
}

```

换成

```rust
            "H = hysteria2, h.test, 443, password=***, gecko-password=***"
        );
    }

    /// TUIC (phase 2 M7 design 8): v4's token, v5's UUID and password, and a
    /// stray positional value; the ALPN and the hopping ports stay.
    #[test]
    fn a_tuic_line_loses_its_credentials() {
        assert_eq!(
            redact_definition(
                "tuic, 192.168.20.6, 443, token=s3cret, alpn=h3, port-hopping=1234;5000-6000"
            ),
            "tuic, 192.168.20.6, 443, token=***, alpn=h3, port-hopping=1234;5000-6000"
        );
        assert_eq!(
            redact_profile(
                "T = tuic-v5, h.test, 443, hunter2, uuid=0233d11c-15a4-47d3-ade3-48ffca0ce119, password=s3cret, alpn=h3"
            ),
            "T = tuic-v5, h.test, 443, ***, uuid=***, password=***, alpn=h3"
        );
    }
}

```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
    assert_eq!(d.common.ecn, Tristate::Off);
}

/// A group's `underlying-proxy` chains the proxies it takes in (`M (via
```

换成

```rust
    assert_eq!(d.common.ecn, Tristate::Off);
}

/// `tuic` and `tuic-v5` lines are read and checked at load (phase 2 M7
/// design 3.1, 6.1): the combinations they cannot have and a credential
/// that is not one are errors, a hop interval under 5 seconds is raised to
/// 5, and `ecn` has an effect on them. They have no spec until the engine
/// builds them (M7b task 5).
#[test]
fn tuic_lines_are_checked_at_load() {
    let loaded = load(
        "A = tuic, a.example, 443, token=t, port-hopping=5000-6000, underlying-proxy=E
B = tuic-v5, b.example, 443, uuid=0233d11c-15a4-47d3-ade3-48ffca0ce119, password=p, shadow-tls-password=st
C = tuic-v5, c.example, 443, uuid=0233d11c-15a4, password=p
D = tuic-v5, d.example, 443, uuid=0233d11c-15a4-47d3-ade3-48ffca0ce119, password=p, ecn=off, port-hopping=5000-6000, port-hopping-interval=2
E = socks5, e.example, 1080",
        "",
    );
    let found: Vec<(&str, String, u32)> = loaded
        .diagnostics
        .iter()
        .map(|d| (d.code, d.message.clone(), d.span.as_ref().unwrap().line))
        .collect();
    assert_eq!(
        found,
        [
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `A`: `port-hopping` cannot be combined with `underlying-proxy`".to_string(),
                2
            ),
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `B`: Shadow TLS cannot be combined with a `tuic-v5` policy".to_string(),
                3
            ),
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `C`: `uuid` is not a valid UUID".to_string(),
                4
            ),
            (
                codes::W_INVALID_VALUE,
                "policy `D`: `port-hopping-interval` is below the minimum of 5 seconds; using 5"
                    .to_string(),
                5
            ),
        ]
    );
    for name in ["A", "B", "C", "D"] {
        assert!(loaded.config.spec(name).is_none(), "{name}");
    }
}

/// A group's `underlying-proxy` chains the proxies it takes in (`M (via
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-config --lib`
Expected: FAIL——`TuicSpec`、`TuicVersion` 与 `ProtoSpec::Tuic` 由 Step 3 引入，编译不过：

```text
error[E0422]: cannot find struct, variant or union type `TuicSpec` in this scope
    --> crates\rurge-config\src\spec\mod.rs:1434:40
error[E0599]: no variant or associated item named `Tuic` found for enum `spec::ProtoSpec` in the current scope
    --> crates\rurge-config\src\spec\mod.rs:1434:35
    --> crates\rurge-config\src\spec\mod.rs:114:5
error[E0433]: failed to resolve: use of undeclared type `TuicVersion`
    --> crates\rurge-config\src\spec\mod.rs:1428:13
error[E0433]: failed to resolve: use of undeclared type `TuicVersion`
    --> crates\rurge-config\src\spec\mod.rs:1429:13
Some errors have detailed explanations: E0422, E0433, E0599.
For more information about an error, try `rustc --explain E0422`.
error: could not compile `rurge-config` (lib test) due to 4 previous errors
```

（`cargo test -p rurge-config --test policy_spec` 能编译，`tuic_lines_are_checked_at_load` 以 `left: []` 失败：今天对这些行什么也不报。）

- [ ] **Step 3: 实现**

新建 `crates/rurge-config/src/spec/tuic.rs`：

```rust
//! `tuic` and `tuic-v5` policy parameters (manual: Policies › TUIC; phase 2
//! M7 design 6.1): TLS over QUIC. The keyword is the version, and the two
//! authenticate differently — `tuic` (TUIC v4) with a token, `tuic-v5` with
//! a UUID and a password; they are not interchangeable (manual).

use super::common::CommonOpts;
use super::quic::{QuicOpts, read_quic, read_quic_tls};
use super::reader::ParamReader;
use super::required;
use super::secret::Secret;
use super::tls::TlsOpts;
use super::vmess::parse_uuid;
use crate::diagnostic::codes;
use crate::keystore::KeystoreItem;
use crate::policy::PolicyKind;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TuicSpec {
    /// TUIC always runs TLS over QUIC; `alpn` is `h3` unless written.
    pub tls: TlsOpts,
    pub quic: QuicOpts,
    /// The version the line's keyword names, with its credentials.
    pub version: TuicVersion,
}

/// A TUIC version and what it authenticates with (phase 2 M7 design 6.2,
/// 6.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TuicVersion {
    /// `tuic`: TUIC v4 and its `token`.
    V4 { token: Secret<String> },
    /// `tuic-v5`: the user's `uuid` and its `password`.
    V5 {
        uuid: Secret<[u8; 16]>,
        password: Secret<String>,
    },
}

/// Everything `tuic`- or `tuic-v5`-specific on the line, and `tfo` taken
/// out of `common` (`read_quic`). After an error was reported the returned
/// value is meaningless: the caller checks `r.has_errors()`.
///
/// The credentials are named-only, as the manual writes them, and never
/// quoted in a diagnostic: a positional value stays unread and is reported
/// as an extra positional value. The other version's credentials do not
/// apply (`W0028`).
pub fn read_tuic(
    r: &mut ParamReader<'_>,
    common: &mut CommonOpts,
    keystore: &[KeystoreItem],
) -> TuicSpec {
    let tls = read_quic_tls(r, keystore);
    let quic = read_quic(r, common);
    let kind = r.policy().kind;
    let (version, other, others) = if kind == PolicyKind::Tuic {
        let token = required(r, "token");
        let others = ["uuid", "password"].as_slice();
        (TuicVersion::V4 { token }, PolicyKind::TuicV5, others)
    } else {
        let uuid = read_uuid(r);
        let password = required(r, "password");
        let others = ["token"].as_slice();
        (TuicVersion::V5 { uuid, password }, PolicyKind::Tuic, others)
    };
    for key in others {
        if r.has(key) {
            r.touch(key);
            r.warn(
                codes::W_PARAM_NOT_APPLICABLE,
                format!(
                    "`{key}` only applies to `{}` policies; ignored",
                    other.keyword()
                ),
            );
        }
    }
    if r.has("udp-relay") {
        r.touch("udp-relay");
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            format!(
                "`udp-relay` does not apply to `{}` policies (UDP is always relayed); ignored",
                kind.keyword()
            ),
        );
    }
    TuicSpec { tls, quic, version }
}

/// `uuid`: the hyphenated form (manual), or the same 32 digits without
/// hyphens, as `vmess` takes its id.
fn read_uuid(r: &mut ParamReader<'_>) -> Secret<[u8; 16]> {
    let uuid = match r.str("uuid").map(parse_uuid) {
        Some(Some(uuid)) => uuid,
        found => {
            let text = match found {
                None => "`uuid` is required",
                Some(_) => "`uuid` is not a valid UUID",
            };
            r.error(codes::E_INVALID_POLICY_PARAM, text.to_string());
            [0; 16]
        }
    };
    Secret::new(uuid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use crate::spec::Sni;
    use crate::spec::common::{Applies, Notes, read_common};
    use crate::spec::quic::{DEFAULT_HOP_INTERVAL, PortHopping};
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    const ID: &str = "0233d11c-15a4-47d3-ade3-48ffca0ce119";
    const BYTES: [u8; 16] = [
        0x02, 0x33, 0xd1, 0x1c, 0x15, 0xa4, 0x47, 0xd3, 0xad, 0xe3, 0x48, 0xff, 0xca, 0x0c, 0xe1,
        0x19,
    ];

    fn read(def: &str) -> (TuicSpec, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let mut common = read_common(&mut r, Applies::Proxy, &mut Notes::default());
        let spec = read_tuic(&mut r, &mut common, &[]);
        let failed = r.has_errors();
        (spec, failed, r.finish())
    }

    fn messages(diags: &[Diagnostic]) -> Vec<(&str, &str)> {
        diags.iter().map(|d| (d.code, d.message.as_str())).collect()
    }

    fn v5(uuid: [u8; 16], password: &str) -> TuicVersion {
        TuicVersion::V5 {
            uuid: Secret::new(uuid),
            password: password.into(),
        }
    }

    #[test]
    fn the_manuals_examples() {
        let (spec, failed, diags) = read("tuic, 192.168.20.6, 443, token=pwd, alpn=h3");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(
            spec.version,
            TuicVersion::V4 {
                token: "pwd".into()
            }
        );
        assert_eq!(spec.quic, QuicOpts::default());
        assert_eq!(
            spec.tls,
            TlsOpts {
                alpn: vec!["h3".into()],
                ..TlsOpts::default()
            }
        );
        let (spec, failed, diags) = read(&format!(
            "tuic-v5, 192.168.20.6, 443, uuid={ID}, password=pwd, alpn=h3"
        ));
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.version, v5(BYTES, "pwd"));
        assert_eq!(spec.quic, QuicOpts::default());
        // `h3` without `alpn` too (manual)
        let (spec, failed, _) = read("tuic, h.test, 443, token=pwd");
        assert!(!failed);
        assert_eq!(spec.tls.alpn, ["h3"]);
    }

    #[test]
    fn every_parameter() {
        let (spec, failed, diags) = read(&format!(
            "tuic-v5, h.test, 443, uuid={}, password=pw, port-hopping=443;20000-30000, port-hopping-interval=60, alpn=h3-29, sni=edge.test, skip-cert-verify=true",
            ID.replace('-', "").to_uppercase()
        ));
        assert!(!failed && diags.is_empty(), "{diags:?}");
        // hyphens and case do not matter, as on `vmess`
        assert_eq!(spec.version, v5(BYTES, "pw"));
        assert_eq!(
            spec.quic.port_hopping,
            Some(PortHopping {
                ports: vec![443..=443, 20000..=30000],
                interval: Duration::from_secs(60),
            })
        );
        assert_eq!(spec.tls.alpn, ["h3-29"]);
        assert_eq!(spec.tls.sni, Sni::Name("edge.test".into()));
        assert!(spec.tls.skip_cert_verify);
        let (spec, failed, diags) = read("tuic, h.test, 443, token=t, port-hopping=443");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(
            spec.quic.port_hopping.map(|hopping| hopping.interval),
            Some(DEFAULT_HOP_INTERVAL)
        );
    }

    #[test]
    fn the_credentials_are_required_named_and_never_quoted() {
        let (_, failed, diags) = read("tuic, h.test, 443");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: `token` is required"
            )]
        );
        let (_, failed, diags) = read("tuic, h.test, 443, hunter2, token=");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `token` is required"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `P`: unexpected positional value #1 ignored"
                ),
            ]
        );
        let (_, failed, diags) = read("tuic-v5, h.test, 443");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `uuid` is required"
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `password` is required"
                ),
            ]
        );
        // positional values are not read as the credentials
        let (_, failed, diags) = read(&format!("tuic-v5, h.test, 443, {ID}, hunter2, password="));
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `uuid` is required"
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `password` is required"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `P`: unexpected positional value #1 ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `P`: unexpected positional value #2 ignored"
                ),
            ]
        );
    }

    #[test]
    fn a_uuid_that_is_not_one_is_an_error_that_never_quotes_it() {
        for bad in [
            "not-a-uuid",
            "0233d11c-15a4-47d3-ade3-48ffca0ce11", // one digit short
            "0233d11c15a4-47d3-ade3-48ffca0ce119-", // hyphens in the wrong places
            "0233d11c-15a4-47d3-ade3-48ffca0ce11g", // not hex
            "{0233d11c-15a4-47d3-ade3-48ffca0ce119}", // braces
            "",
        ] {
            let (_, failed, diags) = read(&format!(
                "tuic-v5, h.test, 443, uuid=\"{bad}\", password=pw"
            ));
            assert!(failed, "{bad}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `uuid` is not a valid UUID"
                )],
                "{bad}"
            );
        }
    }

    /// The two versions are not interchangeable (manual): what one
    /// authenticates with, the other does not use.
    #[test]
    fn the_other_versions_credentials_do_not_apply() {
        let (spec, failed, diags) = read(&format!(
            "tuic, h.test, 443, token=tok, uuid={ID}, password=pw"
        ));
        assert!(!failed);
        assert_eq!(
            spec.version,
            TuicVersion::V4 {
                token: "tok".into()
            }
        );
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `uuid` only applies to `tuic-v5` policies; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `password` only applies to `tuic-v5` policies; ignored"
                ),
            ]
        );
        let (spec, failed, diags) = read(&format!(
            "tuic-v5, h.test, 443, uuid={ID}, password=pw, token=tok"
        ));
        assert!(!failed);
        assert_eq!(spec.version, v5(BYTES, "pw"));
        assert_eq!(
            messages(&diags),
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `P`: `token` only applies to `tuic` policies; ignored"
            )]
        );
        // a v5 line under the v4 keyword lacks the token
        let (_, failed, diags) = read(&format!("tuic, h.test, 443, uuid={ID}, password=pw"));
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `token` is required"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `uuid` only applies to `tuic-v5` policies; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `password` only applies to `tuic-v5` policies; ignored"
                ),
            ]
        );
    }

    /// UDP is relayed whatever the line says (manual).
    #[test]
    fn udp_relay_does_not_apply() {
        for (def, kind) in [
            (
                "tuic, h.test, 443, token=t, udp-relay=true".to_string(),
                "tuic",
            ),
            (
                format!("tuic-v5, h.test, 443, uuid={ID}, password=pw, udp-relay=false"),
                "tuic-v5",
            ),
        ] {
            let (_, failed, diags) = read(&def);
            assert!(!failed, "{def}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::W_PARAM_NOT_APPLICABLE,
                    format!(
                        "policy `P`: `udp-relay` does not apply to `{kind}` policies (UDP is always relayed); ignored"
                    )
                    .as_str()
                )]
            );
        }
    }

    #[test]
    fn the_spec_does_not_print_its_credentials() {
        let (v4, _, _) = read("tuic, h.test, 443, token=s3cretTok");
        let (v5, _, _) = read(&format!(
            "tuic-v5, h.test, 443, uuid={ID}, password=hunter2"
        ));
        for printed in [format!("{v4:?}"), format!("{v5:?}")] {
            assert!(printed.contains("Secret(***)"), "{printed}");
            assert!(
                !printed.contains("s3cretTok") && !printed.contains("hunter2"),
                "{printed}"
            );
            // 0xd1, the third byte of the UUID, as `Debug` would print it
            assert!(!printed.contains("209"), "{printed}");
        }
    }
}
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub mod trojan;
```

换成

```rust
pub mod trojan;
pub mod tuic;
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub use trojan::TrojanSpec;
```

换成

```rust
pub use trojan::TrojanSpec;
pub use tuic::{TuicSpec, TuicVersion};
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    Hysteria2(Hysteria2Spec),
```

换成

```rust
    Hysteria2(Hysteria2Spec),
    Tuic(TuicSpec),
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            ProtoSpec::Hysteria2(hysteria2) => Some(&hysteria2.tls),
```

换成

```rust
            ProtoSpec::Hysteria2(hysteria2) => Some(&hysteria2.tls),
            ProtoSpec::Tuic(tuic) => Some(&tuic.tls),
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            ProtoSpec::Hysteria2(hysteria2) => Some(&hysteria2.quic),
```

换成

```rust
            ProtoSpec::Hysteria2(hysteria2) => Some(&hysteria2.quic),
            ProtoSpec::Tuic(tuic) => Some(&tuic.quic),
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            Vec::new()
        }
    }
}

fn check_underlying(r: &mut ParamReader<'_>, common: &mut CommonOpts, env: &SpecEnv<'_>) {
```

换成

```rust
            Vec::new()
        }
    }
}

/// A named credential that must be there; its value is never quoted.
fn required(r: &mut ParamReader<'_>, key: &str) -> Secret<String> {
    let value = r.str(key).unwrap_or_default();
    if value.is_empty() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            format!("`{key}` is required"),
        );
    }
    Secret::from(value)
}

fn check_underlying(r: &mut ParamReader<'_>, common: &mut CommonOpts, env: &SpecEnv<'_>) {
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            (common, ProtoSpec::Hysteria2(hysteria2))
        }
        PolicyKind::Ssh => {
```

换成

```rust
            (common, ProtoSpec::Hysteria2(hysteria2))
        }
        PolicyKind::Tuic | PolicyKind::TuicV5 => {
            let mut common = read_common(&mut r, Applies::Proxy, &mut notes);
            let tuic = tuic::read_tuic(&mut r, &mut common, env.keystore);
            // as on `hysteria2`: `ecn` takes effect, `tfo` does not apply
            notes.inert.retain(|name| !matches!(*name, "ecn" | "tfo"));
            (common, ProtoSpec::Tuic(tuic))
        }
        PolicyKind::Ssh => {
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    let spec = (!failed && not_implemented.is_none()).then(|| PolicySpec {
```

换成

```rust
    // the engine builds `tuic` and `tuic-v5` from M7b task 5 on: until then
    // a valid line is checked in full but has no spec
    let built = !matches!(policy.kind, PolicyKind::Tuic | PolicyKind::TuicV5);
    let spec = (!failed && not_implemented.is_none() && built).then(|| PolicySpec {
```

`required` 从 `h2.rs` 移到 `spec/mod.rs`（行为不变）：

`crates/rurge-config/src/spec/h2.rs`——把

```rust
use super::{read_credentials, read_headers};
```

换成

```rust
use super::{read_credentials, read_headers, required};
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
    headers
}

/// A named credential that must be there; its value is never quoted.
fn required(r: &mut ParamReader<'_>, key: &str) -> Secret<String> {
    let value = r.str(key).unwrap_or_default();
    if value.is_empty() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            format!("`{key}` is required"),
        );
    }
    Secret::from(value)
```

换成

```rust
    headers
```

`crates/rurge-config/src/spec/vmess.rs`——把

```rust
fn parse_uuid(text: &str) -> Option<[u8; 16]> {
```

换成

```rust
pub(crate) fn parse_uuid(text: &str) -> Option<[u8; 16]> {
```

占位分支（Task 5 去掉）：

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            )?),
            // nothing starts here: the program starts on the first dial
```

换成

```rust
            )?),
            // the loader makes no spec of these lines before M7b task 5
            ProtoSpec::Tuic(_) => {
                return Err(BuildError::new(format!(
                    "policy `{}`: `{}` is not implemented yet",
                    spec.name,
                    spec.kind.keyword()
                )));
            }
            // nothing starts here: the program starts on the first dial
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-config` → 通过（新增 12 条：`spec::tuic` 7 条、`spec::tests` 3 条、脱敏 1 条、`policy_spec` 1 条）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-config crates/rurge-engine/src/outbounds.rs
git commit -m "feat(config): tuic 与 tuic-v5 的配置；两条 E0018 真正报出"
```

### Task 2: TUIC v5 的认证与 TCP、`FakeTuic`

`TuicOutbound` 的 TCP（P4–P6）：每条新连接先在单向流上发认证，每个连接一条双向流 `05 01 ‖ ADDR`、写完即转发。`FakeTuic` 独立实现（quinn 服务端；它自己的命令与地址编解码、自己的服务端导出密钥核对）。`udp()` 暂用缺省（不支持，Task 3 翻转）；v4 的 spec 暂时建出站即报错（Task 4 去掉）。hysteria2 用例里的日志捕获移到共用的 `test_log.rs`。新模块的用例都在新文件里，与实现一同写出，没有单独的失败步骤。

**Files:**
- Create: `crates/rurge-proto-quic/src/tuic/{mod.rs, auth.rs, addr.rs, tcp.rs}`（自带用例）、`src/test_log.rs`、`src/testing/tuic.rs`
- Modify: `crates/rurge-proto-quic/src/{lib.rs, hysteria2/mod.rs（用例）, testing/mod.rs}`

**Interfaces:**
- Consumes: Task 1 的 `TuicSpec`；M7a 的 `Dialer` / `Session` / `Protocol` / `Failure` / `ServerPort` / `MultiPort`；quinn 的 `Connection::export_keying_material`。
- Produces: `rurge_proto_quic::tuic::TuicOutbound::new(name: &str, server: Target, spec: &TuicSpec, ecn: Tristate, keystore: &[KeystoreItem], roots: Arc<RootCertStore>, connector: Arc<dyn Connector>) -> Result<TuicOutbound, BuildError>`（实现 `Outbound` 的 TCP）；测试设施 `FakeTuic`、`TuicScript`、`RecordedTuicAuth`、`RecordedConnect`；`crate::test_log::Log`

- [ ] **Step 1: 新模块与假服务端**

`crates/rurge-proto-quic/src/lib.rs`——把

```rust
//! (`hysteria2`).
```

换成

```rust
//! (`hysteria2`) and TUIC's (`tuic`).
```

`crates/rurge-proto-quic/src/lib.rs`——把

```rust
#[cfg(any(test, feature = "testing"))]
pub mod testing;
```

换成

```rust
#[cfg(test)]
mod test_log;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod tuic;
```

`crates/rurge-proto-quic/src/lib.rs`——把

```rust
pub use session::{Dialer, Failure, Layer, Lease, Protocol, Session};
```

换成

```rust
pub use session::{Dialer, Failure, Layer, Lease, Protocol, Session};
pub use tuic::TuicOutbound;
```

新建 `crates/rurge-proto-quic/src/test_log.rs`：

```rust
//! What the tests of this crate log, captured: credentials and payloads
//! must show in no line of it.

use std::io;
use std::sync::{Arc, Mutex};

/// Captures what is logged, formatted.
#[derive(Clone, Default)]
pub(crate) struct Log(Arc<Mutex<Vec<u8>>>);

impl Log {
    /// What this thread logs, at every level, while the guard and the
    /// second dispatcher live. While one dispatcher alone is registered,
    /// tracing-core asks only the thread that first reaches a call site
    /// whether it is wanted: a site another test reaches first would stay
    /// shut on this thread too. With a second one it asks every dispatcher.
    pub(crate) fn capture() -> (Log, tracing::subscriber::DefaultGuard, tracing::Dispatch) {
        let log = Log::default();
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        let logging = tracing::subscriber::set_default(subscriber);
        let second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
        (log, logging, second)
    }

    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl io::Write for Log {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
```

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

```rust
    use super::*;
```

换成

```rust
    use super::*;
    use crate::test_log::Log;
```

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

```rust
            started.elapsed()
        );
    }

    /// Captures what is logged, formatted.
    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<u8>>>);

    impl Log {
        /// What this thread logs, at every level, while the guard and the
        /// second dispatcher live. While one dispatcher alone is
        /// registered, tracing-core asks only the thread that first reaches
        /// a call site whether it is wanted: a site another test reaches
        /// first would stay shut on this thread too. With a second one it
        /// asks every dispatcher.
        fn capture() -> (Log, tracing::subscriber::DefaultGuard, tracing::Dispatch) {
            let log = Log::default();
            let writer = log.clone();
            let subscriber = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::TRACE)
                .with_ansi(false)
                .with_writer(move || writer.clone())
                .finish();
            let logging = tracing::subscriber::set_default(subscriber);
            let second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
            (log, logging, second)
        }

        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    impl io::Write for Log {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// What datagrams carry shows nowhere — in no error and in no log
```

换成

```rust
            started.elapsed()
        );
    }

    /// What datagrams carry shows nowhere — in no error and in no log
```

新建 `crates/rurge-proto-quic/src/tuic/mod.rs`：

```rust
//! `tuic-v5` outbound (manual: Policies › TUIC; phase 2 M7 design 6): the
//! policy's QUIC connection, which its requests share (`session`),
//! authenticated as it comes up (`auth`).
//!
//! - Every command is `u8 version ‖ u8 type ‖ …`, big endian (TUIC v5
//!   SPEC); the server answers none of them.
//! - A TCP connection is a bidirectional stream of its own, the tunnel as
//!   soon as the Connect naming its target (`addr`) is written (`tcp`).

mod addr;
mod auth;
mod tcp;

use crate::session::{Dialer, Session};
use auth::Auth;
use rurge_config::KeystoreItem;
use rurge_config::spec::{Tristate, TuicSpec, TuicVersion};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_proto::{BuildError, Outbound, OutboundError};
use rustls::RootCertStore;
use std::sync::Arc;

/// TUIC v5's version, first in every command.
const VERSION: u8 = 0x05;

/// No `Debug`: it holds the credentials.
pub struct TuicOutbound {
    name: String,
    session: Session<Auth>,
}

impl TuicOutbound {
    /// The connection goes as `connector` dials (design 3.3), with the
    /// policy's `ecn`; `client-cert` comes from `keystore`.
    pub fn new(
        name: &str,
        server: Target,
        spec: &TuicSpec,
        ecn: Tristate,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<TuicOutbound, BuildError> {
        // error texts carry no policy name: the registry's `build_one` and
        // the dry build both prefix it
        let TuicVersion::V5 { uuid, password } = &spec.version else {
            return Err(BuildError::new("TUIC v4 (`tuic`) is not supported yet"));
        };
        // the configuration checked it, but a spec made by hand may not be
        if password.expose().is_empty() {
            return Err(BuildError::new("`password` is empty"));
        }
        let dialer = Dialer::new(
            connector, server, &spec.tls, &spec.quic, ecn, keystore, roots,
        )?;
        let auth = Auth::new(*uuid.expose(), password.expose());
        Ok(TuicOutbound {
            name: name.to_string(),
            session: Session::new(name, dialer, auth),
        })
    }
}

impl Outbound for TuicOutbound {
    fn name(&self) -> &str {
        &self.name
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            // never dial for a target whose name cannot be sent
            let request = tcp::request(target)?;
            // one budget for the connection (when one is dialed), QUIC's
            // handshake and the Connect, which nothing answers
            let tunnel = self
                .session
                .request(opts, |lease| Box::pin(tcp::open(lease, &request)));
            match tokio::time::timeout(opts.timeout, tunnel).await {
                Ok(tunnel) => Ok(Box::new(tunnel?) as BoxedStream),
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_log::Log;
    use crate::testing::{FakeTuic, RecordedConnect, TuicScript};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::tuic::read_tuic;
    use rurge_config::spec::{CommonOpts, ParamReader, PortHopping, Secret};
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_net::quic::testing::MultiPort;
    use rurge_proto::testing::echo_server;
    use std::io;
    use std::net::SocketAddr;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio::time::{Instant, timeout};

    const BOUND: Duration = Duration::from_secs(10);
    /// The manual's example user, and its 16 bytes.
    const ID: &str = "0233d11c-15a4-47d3-ade3-48ffca0ce119";
    const UUID: [u8; 16] = [
        0x02, 0x33, 0xd1, 0x1c, 0x15, 0xa4, 0x47, 0xd3, 0xad, 0xe3, 0x48, 0xff, 0xca, 0x0c, 0xe1,
        0x19,
    ];
    const PASSWORD: &str = "pw";

    /// A fake that lets `ID` in with `PASSWORD`.
    fn script() -> TuicScript {
        TuicScript {
            uuid: UUID,
            password: PASSWORD.to_string(),
            ..TuicScript::default()
        }
    }

    /// `uuid=<uuid>, password=<password>`.
    fn credentials(uuid: &str, password: &str) -> String {
        format!("uuid={uuid}, password={password}")
    }

    /// The spec of `tuic-v5, 127.0.0.1, <port>, <params>`, as the
    /// configuration reads it.
    fn spec(port: u16, params: &str) -> TuicSpec {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let line = format!("tuic-v5, 127.0.0.1, {port}, {params}");
        let policy = parse_policy("T", &line, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let spec = read_tuic(&mut r, &mut CommonOpts::default(), &[]);
        assert!(!r.has_errors(), "{:?}", r.finish());
        spec
    }

    /// `spec`'s outbound to `127.0.0.1:<port>`, trusting `fake`.
    fn outbound_with(spec: &TuicSpec, port: u16, fake: &FakeTuic) -> TuicOutbound {
        TuicOutbound::new(
            "T",
            Target::new(HostName::parse("127.0.0.1"), port),
            spec,
            Tristate::Auto,
            &[],
            fake.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    /// `tuic-v5, 127.0.0.1, <the fake's port>, <params>`.
    fn outbound(fake: &FakeTuic, params: &str) -> TuicOutbound {
        let port = fake.addr().port();
        outbound_with(&spec(port, params), port, fake)
    }

    /// The outbound of the fake's user.
    fn user(fake: &FakeTuic) -> TuicOutbound {
        outbound(fake, &credentials(ID, PASSWORD))
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    async fn tunnel(out: &TuicOutbound, to: &Target) -> BoxedStream {
        timeout(BOUND, out.connect_tcp(to, &ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .expect("a tunnel")
    }

    async fn failure(out: &TuicOutbound, to: &Target) -> OutboundError {
        timeout(BOUND, out.connect_tcp(to, &ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .err()
            .expect("no tunnel")
    }

    async fn round_trip(stream: &mut (impl AsyncRead + AsyncWrite + Unpin), payload: &[u8]) {
        stream.write_all(payload).await.unwrap();
        let mut back = vec![0u8; payload.len()];
        timeout(BOUND, stream.read_exact(&mut back))
            .await
            .expect("the echo arrives")
            .unwrap();
        assert_eq!(back, payload);
    }

    /// How a tunnel ends, read out: with an error.
    async fn end_of(stream: &mut BoxedStream) -> io::Error {
        let mut rest = Vec::new();
        timeout(BOUND, stream.read_to_end(&mut rest))
            .await
            .expect("the end of the tunnel")
            .expect_err("an error")
    }

    /// Design 6.2: once per connection, a unidirectional stream carries
    /// the user's UUID and the token its TLS session exports — the fake's
    /// own side of the session exports the same. The Connect goes along
    /// without waiting for a verdict, and is relayed once the server let
    /// the client in. The token is the session's: another connection's
    /// differs.
    #[tokio::test]
    async fn the_authentication_carries_the_uuid_and_a_token_of_the_tls_session() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        assert_eq!(out.name(), "T");
        assert!(out.http_forward().is_none());
        let mut stream = tunnel(&out, &target(echo)).await;
        round_trip(&mut stream, b"through tuic").await;
        let auths = fake.auths();
        assert_eq!(auths.len(), 1);
        assert_eq!((auths[0].connection, auths[0].uuid), (0, UUID));
        assert_eq!(auths[0].token, auths[0].expected);
        assert!(auths[0].accepted);
        assert_eq!(
            fake.connects(),
            [RecordedConnect {
                connection: 0,
                address: echo.to_string(),
            }]
        );
        let other = user(&fake);
        let mut stream = tunnel(&other, &target(echo)).await;
        round_trip(&mut stream, b"on another connection").await;
        let auths = fake.auths();
        assert_eq!((auths.len(), auths[1].connection), (2, 1));
        assert!(auths[1].accepted);
        assert_ne!(auths[1].token, auths[0].token);
    }

    /// Design 6.2: the server answers no authentication. One it refuses —
    /// a wrong password, a user it does not know — it closes the
    /// connection with code 0 and no reason, as tuic-server and sing-box
    /// close it, before it relayed anything: a tunnel on it ends with
    /// `tuic: authentication failed`. The next tunnel dials and
    /// authenticates again. A close with another code is no such refusal.
    #[tokio::test]
    async fn a_refused_authentication_ends_the_tunnel_and_the_next_dials_again() {
        const STRANGER: &str = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
        let echo = echo_server().await;
        for (params, code, expected) in [
            (credentials(ID, "wrong"), 0, "tuic: authentication failed"),
            (
                credentials(STRANGER, PASSWORD),
                0,
                "tuic: authentication failed",
            ),
            (
                credentials(ID, "wrong"),
                0x2a,
                "tuic: the server closed the connection (code 0x2a)",
            ),
        ] {
            let fake = FakeTuic::spawn(TuicScript {
                auth_failure_code: code,
                ..script()
            });
            let out = outbound(&fake, &params);
            for _ in 0..2 {
                // nothing answers the Connect: the tunnel comes, and ends
                let mut stream = tunnel(&out, &target(echo)).await;
                assert_eq!(end_of(&mut stream).await.to_string(), expected, "{params}");
            }
            assert_eq!(fake.connections(), 2);
            let auths = fake.auths();
            assert_eq!(auths.len(), 2);
            assert!(auths.iter().all(|auth| !auth.accepted));
        }
    }

    /// Past both sides' stream windows (8 MiB ours, quinn's 1.25 MB the
    /// fake's); the shutdown reaches the target as a FIN, and the target's
    /// close comes back as the end of the stream.
    #[tokio::test]
    async fn a_tunnel_carries_a_large_payload_and_a_half_close() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let stream = tunnel(&out, &target(echo)).await;
        let data: Vec<u8> = (0..(9 << 20) + 777).map(|i| (i % 253) as u8).collect();
        let (mut r, mut w) = tokio::io::split(stream);
        let sent = data.clone();
        let writer = tokio::spawn(async move {
            w.write_all(&sent).await.unwrap();
            w.shutdown().await.unwrap();
        });
        let mut back = Vec::new();
        timeout(Duration::from_secs(60), r.read_to_end(&mut back))
            .await
            .expect("stalled")
            .unwrap();
        writer.await.unwrap();
        assert!(back == data, "the echo differs");
    }

    /// Design 6.2: an address goes to the server as its bytes, a name as a
    /// name — an IDN as its A-labels — for the server to resolve. A target
    /// that cannot be named is refused before anything is dialed.
    #[tokio::test]
    async fn targets_go_out_as_the_server_reads_them_and_an_unsendable_name_never_dials() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(TuicScript {
            connect_to: Some(echo),
            ..script()
        });
        let out = user(&fake);
        for (host, address) in [
            (HostName::parse("2001:db8::1"), "[2001:db8::1]:443"),
            (HostName::parse("192.0.2.1"), "192.0.2.1:443"),
            (
                HostName::Domain("bücher.example".into()),
                "xn--bcher-kva.example:443",
            ),
            (HostName::parse("remote.example"), "remote.example:443"),
        ] {
            let mut stream = tunnel(&out, &Target::new(host, 443)).await;
            round_trip(&mut stream, b"x").await;
            assert_eq!(fake.connects().last().unwrap().address, address);
        }
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let unsendable = "tuic: the host name cannot be sent to the server";
        for (name, text) in [
            ("x@blocked.test".to_string(), unsendable),
            ("a b.test".to_string(), unsendable),
            ("a.test\r\nx: 1".to_string(), unsendable),
            (String::new(), unsendable),
            (
                "a".repeat(256),
                "tuic: the host name is longer than 255 bytes",
            ),
        ] {
            let err = failure(&out, &Target::new(HostName::Domain(name), 80)).await;
            assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
            assert_eq!(err.to_string(), text);
        }
        assert_eq!(fake.connections(), 0);
    }

    /// One dial and one authentication for the tunnels that come together,
    /// and the connection carries those after them too.
    #[tokio::test]
    async fn tunnels_that_come_together_share_one_connection() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let to = target(echo);
        let (a, b, c) = tokio::join!(tunnel(&out, &to), tunnel(&out, &to), tunnel(&out, &to));
        for (mut stream, payload) in [(a, b"one"), (b, b"two"), (c, b"six")] {
            round_trip(&mut stream, payload).await;
        }
        let mut later = tunnel(&out, &to).await;
        round_trip(&mut later, b"later").await;
        assert_eq!(fake.connections(), 1);
        assert_eq!(fake.auths().len(), 1);
        assert_eq!(fake.connects().len(), 4);
    }

    /// A tunnel under way ends with its connection — closed by a server
    /// that relayed on it, which is no refused authentication —; the next
    /// tunnel dials and authenticates again.
    #[tokio::test]
    async fn after_the_server_closes_the_connection_the_next_tunnel_dials_again() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let mut first = tunnel(&out, &target(echo)).await;
        round_trip(&mut first, b"first").await;
        fake.close_connections();
        assert_eq!(
            end_of(&mut first).await.to_string(),
            "tuic: the server closed the connection (code 0x0)"
        );
        let mut second = tunnel(&out, &target(echo)).await;
        round_trip(&mut second, b"second").await;
        assert_eq!(fake.connections(), 2);
        assert_eq!(fake.auths().len(), 2);
    }

    /// Design 3.5 and 6.2: the server restarts, losing its connections.
    /// Nothing answers a Connect, so nothing tells the client before the
    /// tunnel's own packets go out: a tunnel opened on the lost connection
    /// after the restart, and the one under way, end with the stateless
    /// reset that answers the first of them. The next tunnel dials anew.
    #[tokio::test]
    async fn after_a_restart_the_tunnels_of_the_lost_connection_end_and_the_next_dials_again() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let mut before = tunnel(&out, &target(echo)).await;
        round_trip(&mut before, b"before").await;
        fake.restart();
        let mut stale = tunnel(&out, &target(echo)).await;
        let reset = "quic: the server no longer knows the connection (stateless reset)";
        assert_eq!(end_of(&mut stale).await.to_string(), reset);
        assert_eq!(end_of(&mut before).await.to_string(), reset);
        let mut after = tunnel(&out, &target(echo)).await;
        round_trip(&mut after, b"after").await;
        let carried: Vec<usize> = fake.connects().iter().map(|c| c.connection).collect();
        assert_eq!(carried, [0, 1]);
        assert_eq!(fake.auths().len(), 2);
    }

    /// Design 3.5: a request that finds its connection gone before its
    /// Connect went out — the server closed it as the request took it —
    /// goes again, once, on a new connection.
    #[tokio::test]
    async fn a_connection_found_gone_before_the_connect_went_out_is_dialed_again_once() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let mut first = tunnel(&out, &target(echo)).await;
        round_trip(&mut first, b"first").await;
        let request = tcp::request(&target(echo)).unwrap();
        let attempts = AtomicUsize::new(0);
        let opts = ConnectOpts::default();
        let reopened = out.session.request(&opts, |lease| {
            let gone = attempts.fetch_add(1, Ordering::SeqCst) == 0;
            let (fake, request) = (&fake, &request);
            Box::pin(async move {
                if gone {
                    fake.close_connections();
                    timeout(BOUND, lease.connection().closed())
                        .await
                        .expect("closed");
                }
                tcp::open(lease, request).await
            })
        });
        let mut second = timeout(BOUND, reopened)
            .await
            .expect("an outcome within the bound")
            .expect("a tunnel");
        round_trip(&mut second, b"second").await;
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(fake.connections(), 2);
        assert_eq!(fake.auths().len(), 2);
    }

    /// Design 3.3: the port changes under the connection — a new local
    /// socket each time — and its tunnels, old and new, go on. The interval
    /// is set past the configuration's minimum of 5 seconds.
    #[tokio::test]
    async fn tunnels_keep_working_while_the_port_hops() {
        let echo = echo_server().await;
        let ports = MultiPort::bind(3);
        let fake = FakeTuic::spawn_on(script(), ports.clone());
        let mut spec = spec(9, &credentials(ID, PASSWORD));
        spec.quic.port_hopping = Some(PortHopping {
            ports: ports.ports().iter().map(|&p| p..=p).collect(),
            interval: Duration::from_millis(100),
        });
        // the line's own port is not used
        let out = outbound_with(&spec, 9, &fake);
        let mut early = tunnel(&out, &target(echo)).await;
        let deadline = Instant::now() + BOUND;
        while ports.heard().iter().filter(|&&n| n > 0).count() < 2 {
            assert!(
                Instant::now() < deadline,
                "heard only on {:?}",
                ports.heard()
            );
            round_trip(&mut early, b"hop").await;
        }
        let mut late = tunnel(&out, &target(echo)).await;
        round_trip(&mut late, b"after a hop").await;
        round_trip(&mut early, b"still there").await;
        assert!(ports.clients() >= 2, "a new local socket per hop");
        assert_eq!(fake.connections(), 1);
    }

    /// `opts.timeout` bounds the request: the dial and QUIC's handshake
    /// with a server that never answers it.
    #[tokio::test]
    async fn a_server_that_never_answers_the_handshake_times_out() {
        let echo = echo_server().await;
        // nothing speaks QUIC on this port
        let quiet = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = quiet.local_addr().unwrap().port();
        let fake = FakeTuic::spawn(script());
        let out = outbound_with(&spec(port, &credentials(ID, PASSWORD)), port, &fake);
        let opts = ConnectOpts {
            timeout: Duration::from_millis(500),
        };
        let started = Instant::now();
        let err = timeout(BOUND, out.connect_tcp(&target(echo), &opts))
            .await
            .expect("an outcome within the bound")
            .err()
            .unwrap();
        assert!(matches!(err, OutboundError::Timeout), "{err}");
        // well before QUIC's own handshake timeout, about 3 seconds
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    /// Design 6.2: nothing answers a Connect either. A target the server
    /// cannot reach shows as the stream ending, as sing-box ends it:
    /// nothing to read, and what the caller writes is stopped. The
    /// connection goes on.
    #[tokio::test]
    async fn a_target_the_server_cannot_reach_ends_the_tunnel_and_the_connection_goes_on() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        // the fake resolves no name
        let unknown = Target::new(HostName::parse("unknown.test"), 80);
        let mut stream = tunnel(&out, &unknown).await;
        let mut rest = Vec::new();
        let read = timeout(BOUND, stream.read_to_end(&mut rest))
            .await
            .expect("the end of the stream");
        assert_eq!(read.unwrap(), 0);
        let deadline = Instant::now() + BOUND;
        let stopped = loop {
            assert!(Instant::now() < deadline, "writes still taken");
            if let Err(e) = stream.write_all(b"x").await {
                break e;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert_eq!(
            stopped.to_string(),
            "tuic: the server stopped the stream (code 0x0)"
        );
        let mut next = tunnel(&out, &target(echo)).await;
        round_trip(&mut next, b"still up").await;
        assert_eq!(fake.connections(), 1);
        assert_eq!(fake.connects().len(), 2);
    }

    /// The credentials go to the server and nowhere else: not into the
    /// error of a refused authentication, nor into a build error, nor into
    /// a log line at any level.
    #[tokio::test]
    async fn the_credentials_never_show() {
        const SECRET: &str = "s3cret-tuic";
        let (log, _logging, _second) = Log::capture();
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = outbound(&fake, &credentials(ID, SECRET));
        let mut stream = tunnel(&out, &target(echo)).await;
        let err = end_of(&mut stream).await;
        assert_eq!(err.to_string(), "tuic: authentication failed");
        let hex: String = UUID.iter().map(|b| format!("{b:02x}")).collect();
        let shown = format!("{err} {err:?}");
        for secret in [SECRET, ID, hex.as_str()] {
            assert!(!shown.contains(secret), "{shown}");
        }
        let mut spec = spec(443, &credentials(ID, SECRET));
        spec.version = TuicVersion::V5 {
            uuid: Secret::new(UUID),
            password: "".into(),
        };
        let err = TuicOutbound::new(
            "T",
            Target::new(HostName::parse("127.0.0.1"), 443),
            &spec,
            Tristate::Auto,
            &[],
            fake.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .map(|_| ())
        .unwrap_err();
        assert_eq!(err.message, "`password` is empty");
        let log = log.text();
        assert!(!log.is_empty());
        for secret in [SECRET, ID, hex.as_str()] {
            assert!(!log.contains(secret));
        }
    }
}
```

新建 `crates/rurge-proto-quic/src/tuic/auth.rs`：

```rust
//! The authentication of every new connection (TUIC v5 SPEC,
//! "Authenticate"; phase 2 M7 design 6.2): a unidirectional stream of its
//! own carrying `05 00 ‖ UUID ‖ TOKEN` — the user's UUID, its 16 bytes as
//! they are, and the 32 bytes the connection's TLS session exports with the
//! UUID's bytes as the label and the password as the context (RFC 5705, RFC
//! 8446 7.5), which only the two ends of that session can work out.
//!
//! - The server answers nothing, and holds the commands that come before it
//!   checked the token (SPEC): the command goes out as the connection comes
//!   up, and the connection's requests follow without waiting.
//! - A server that refuses it closes the connection — tuic-server and
//!   sing-box with code 0 and no reason —, which the connection's tunnels
//!   then tell (`Authenticated::ended`).

use super::VERSION;
use crate::session::Protocol;
use quinn::{Connection, ConnectionError, ReadError, VarInt, WriteError};
use rurge_net::BoxFuture;
use rurge_net::quic::connection_error;
use rurge_proto::OutboundError;
use rurge_proto::outbound::untrusted_text;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

/// The command's type.
const AUTHENTICATE: u8 = 0x00;

/// The code TUIC's implementations close a connection with, whatever the
/// cause — tuic-server 1.0.0, tuic-client 1.0.0 and sing-box alike: the v5
/// SPEC defines none.
const CLOSE: VarInt = VarInt::from_u32(0);

/// How much of the server's reason for a close an error text quotes.
const REASON_MAX: usize = 200;

/// What authenticates the policy's connections. No `Debug`: it holds the
/// credentials.
pub(crate) struct Auth {
    uuid: [u8; 16],
    password: String,
}

/// What a connection keeps once its authentication went out.
#[derive(Default)]
pub(crate) struct Authenticated {
    /// The server relayed something on the connection: it took the
    /// authentication.
    relayed: AtomicBool,
}

impl Auth {
    pub(crate) fn new(uuid: [u8; 16], password: &str) -> Auth {
        Auth {
            uuid,
            password: password.to_string(),
        }
    }

    /// The token of `connection`'s TLS session.
    fn token(&self, connection: &Connection) -> [u8; 32] {
        let mut token = [0u8; 32];
        connection
            .export_keying_material(&mut token, &self.uuid, self.password.as_bytes())
            .expect("a connection is up once its TLS handshake is done, which exports 32 bytes");
        token
    }
}

/// `05 00 ‖ UUID ‖ TOKEN`.
fn command(uuid: &[u8; 16], token: &[u8; 32]) -> [u8; 50] {
    let mut command = [0u8; 50];
    command[..2].copy_from_slice(&[VERSION, AUTHENTICATE]);
    command[2..18].copy_from_slice(uuid);
    command[18..].copy_from_slice(token);
    command
}

impl Protocol for Auth {
    type State = Authenticated;

    const NAME: &'static str = "tuic";

    const NO_ERROR: VarInt = CLOSE;

    /// The command on a stream of its own, finished: handed to QUIC, it
    /// takes no round trip, and nothing comes back to wait for. Not left to
    /// a task of its own (as tuic-client 1.0.0 and sing-box leave it): it
    /// goes before the connection's first request, and a connection that
    /// cannot take it fails the dial.
    fn handshake<'a>(
        &'a self,
        connection: &'a Connection,
    ) -> BoxFuture<'a, Result<Authenticated, OutboundError>> {
        Box::pin(async move {
            let command = command(&self.uuid, &self.token(connection));
            let state = Authenticated::default();
            let mut stream = connection.open_uni().await.map_err(|e| state.ended(&e))?;
            stream
                .write_all(&command)
                .await
                .map_err(|e| state.write_error(e))?;
            // fails only for a stream finished or reset already
            let _ = stream.finish();
            Ok(state)
        })
    }
}

impl Authenticated {
    /// Something the server relayed came on the connection: a byte from a
    /// tunnel's target.
    pub(super) fn mark_relayed(&self) {
        if !self.relayed.load(Ordering::Relaxed) {
            self.relayed.store(true, Ordering::Relaxed);
        }
    }

    /// How the connection ended, as its requests and tunnels tell it
    /// (design 3.6, 6.2). The server's own close is TUIC's, and v5 defines
    /// no codes: `tuic: the server closed the connection (code …)`, its
    /// reason cleaned and cut short. Code 0 and no reason before the server
    /// relayed anything on the connection is how tuic-server and sing-box
    /// refuse the authentication: `tuic: authentication failed` (sing-box
    /// closes so whatever failed, its session's end included; once the
    /// server relayed, the authentication went through). Any other end as
    /// QUIC tells it.
    pub(super) fn ended(&self, e: &ConnectionError) -> io::Error {
        let ConnectionError::ApplicationClosed(close) = e else {
            return connection_error(e);
        };
        if close.error_code == CLOSE
            && close.reason.is_empty()
            && !self.relayed.load(Ordering::Relaxed)
        {
            return io::Error::new(
                io::ErrorKind::PermissionDenied,
                "tuic: authentication failed",
            );
        }
        let code = close.error_code.into_inner();
        let reason = untrusted_text(&String::from_utf8_lossy(&close.reason), REASON_MAX);
        let text = if reason.is_empty() {
            format!("tuic: the server closed the connection (code {code:#x})")
        } else {
            format!("tuic: the server closed the connection (code {code:#x}: {reason})")
        };
        io::Error::new(io::ErrorKind::ConnectionAborted, text)
    }

    /// A failed read on one of the connection's streams: the connection's
    /// end as `ended` tells it, the stream's as TUIC's.
    pub(super) fn read_error(&self, e: ReadError) -> io::Error {
        match e {
            ReadError::ConnectionLost(e) => self.ended(&e),
            ReadError::Reset(code) => io::Error::new(
                io::ErrorKind::ConnectionReset,
                format!(
                    "tuic: the server reset the stream (code {:#x})",
                    code.into_inner()
                ),
            ),
            ReadError::ClosedStream
            | ReadError::IllegalOrderedRead
            | ReadError::ZeroRttRejected => closed_stream(),
        }
    }

    /// A failed write on one of the connection's streams, as `read_error`.
    pub(super) fn write_error(&self, e: WriteError) -> io::Error {
        match e {
            WriteError::ConnectionLost(e) => self.ended(&e),
            WriteError::Stopped(code) => io::Error::new(
                io::ErrorKind::ConnectionReset,
                format!(
                    "tuic: the server stopped the stream (code {:#x})",
                    code.into_inner()
                ),
            ),
            WriteError::ClosedStream | WriteError::ZeroRttRejected => closed_stream(),
        }
    }
}

fn closed_stream() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "tuic: the stream is closed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use quinn::ApplicationClose;

    #[test]
    fn the_command_is_its_header_the_uuid_and_the_token() {
        let uuid: [u8; 16] = std::array::from_fn(|i| i as u8);
        let command = command(&uuid, &[0xa5; 32]);
        assert_eq!(command[..2], [0x05, 0x00]);
        assert_eq!(command[2..18], uuid);
        assert_eq!(command[18..], [0xa5; 32]);
    }

    fn closed(code: u32, reason: &'static [u8]) -> ConnectionError {
        ConnectionError::ApplicationClosed(ApplicationClose {
            error_code: VarInt::from_u32(code),
            reason: Bytes::from_static(reason),
        })
    }

    /// Design 6.2: code 0 and no reason is the refused authentication only
    /// before the server relayed anything; any other close says its code,
    /// and the transport's ends are QUIC's.
    #[test]
    fn the_end_of_a_connection_reads_as_tuic_tells_it() {
        use io::ErrorKind::*;
        let fresh = Authenticated::default();
        let relayed = Authenticated::default();
        relayed.mark_relayed();
        for (state, error, kind, text) in [
            (
                &fresh,
                closed(0, b""),
                PermissionDenied,
                "tuic: authentication failed",
            ),
            (
                &relayed,
                closed(0, b""),
                ConnectionAborted,
                "tuic: the server closed the connection (code 0x0)",
            ),
            (
                &fresh,
                closed(0, b"bye"),
                ConnectionAborted,
                "tuic: the server closed the connection (code 0x0: bye)",
            ),
            (
                &fresh,
                closed(0x2a, b"go\r\naway"),
                ConnectionAborted,
                "tuic: the server closed the connection (code 0x2a: goaway)",
            ),
            (
                &fresh,
                ConnectionError::Reset,
                ConnectionReset,
                "quic: the server no longer knows the connection (stateless reset)",
            ),
            (
                &fresh,
                ConnectionError::TimedOut,
                TimedOut,
                "quic: the server stopped answering (idle timeout)",
            ),
        ] {
            let ended = state.ended(&error);
            assert_eq!((ended.kind(), ended.to_string().as_str()), (kind, text));
        }
        // as the streams on the connection tell it
        assert_eq!(
            fresh
                .read_error(ReadError::ConnectionLost(closed(0, b"")))
                .to_string(),
            "tuic: authentication failed"
        );
        assert_eq!(
            relayed
                .write_error(WriteError::ConnectionLost(closed(0, b"")))
                .to_string(),
            "tuic: the server closed the connection (code 0x0)"
        );
        assert_eq!(
            fresh
                .read_error(ReadError::Reset(VarInt::from_u32(3)))
                .to_string(),
            "tuic: the server reset the stream (code 0x3)"
        );
        assert_eq!(
            fresh
                .write_error(WriteError::Stopped(VarInt::from_u32(0)))
                .to_string(),
            "tuic: the server stopped the stream (code 0x0)"
        );
        // a reason as long as a datagram is cut short
        let long = ConnectionError::ApplicationClosed(ApplicationClose {
            error_code: VarInt::from_u32(1),
            reason: Bytes::from(vec![b'x'; 1000]),
        });
        assert_eq!(
            fresh.ended(&long).to_string(),
            format!(
                "tuic: the server closed the connection (code 0x1: {})",
                "x".repeat(REASON_MAX)
            )
        );
    }
}
```

新建 `crates/rurge-proto-quic/src/tuic/addr.rs`：

```rust
//! The address of TUIC's commands (TUIC v5 SPEC, "Address"; phase 2 M7
//! design 6.2): `u8 type ‖ address ‖ u16 port`, big endian — `0x00` a
//! domain name, its length in a byte first; `0x01` an IPv4 address; `0x02`
//! an IPv6 address. (`0xff`, no address and no port, only follows the first
//! fragment of a UDP packet.)

use rurge_config::HostName;
use rurge_net::connector::Target;
use rurge_proto::http::wire_host;
use std::fmt;
use std::net::IpAddr;

const DOMAIN: u8 = 0x00;
const IPV4: u8 = 0x01;
const IPV6: u8 = 0x02;

/// Why a target cannot be named to the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AddrError {
    /// The name is not one a request may carry (`http::wire_host`).
    Unsendable,
    /// Longer than its one-byte length allows.
    TooLong,
}

impl fmt::Display for AddrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AddrError::Unsendable => "tuic: the host name cannot be sent to the server",
            AddrError::TooLong => "tuic: the host name is longer than 255 bytes",
        })
    }
}

/// `target`'s address at the end of `out`: a name goes out as a name —
/// the server resolves it —, an IDN as its A-labels. Nothing is written
/// for a target that cannot be named.
pub(super) fn put(out: &mut Vec<u8>, target: &Target) -> Result<(), AddrError> {
    match &target.host {
        HostName::Ip(IpAddr::V4(v4)) => {
            out.push(IPV4);
            out.extend_from_slice(&v4.octets());
        }
        HostName::Ip(IpAddr::V6(v6)) => {
            out.push(IPV6);
            out.extend_from_slice(&v6.octets());
        }
        HostName::Domain(_) => {
            let name = wire_host(target).ok_or(AddrError::Unsendable)?;
            let len = u8::try_from(name.len()).map_err(|_| AddrError::TooLong)?;
            out.push(DOMAIN);
            out.push(len);
            out.extend_from_slice(name.as_bytes());
        }
    }
    out.extend_from_slice(&target.port.to_be_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(target: &Target) -> Result<Vec<u8>, AddrError> {
        let mut out = Vec::new();
        put(&mut out, target).map(|()| out)
    }

    fn at(host: &str) -> Target {
        Target::new(HostName::parse(host), 0x1f90)
    }

    #[test]
    fn the_three_address_types() {
        assert_eq!(
            address(&at("10.1.2.3")),
            Ok(vec![0x01, 10, 1, 2, 3, 0x1f, 0x90])
        );
        let mut v6 = vec![0x02, 0x20, 0x01, 0x0d, 0xb8];
        v6.extend_from_slice(&[0; 11]);
        v6.extend_from_slice(&[1, 0x1f, 0x90]);
        assert_eq!(address(&at("2001:db8::1")), Ok(v6));
        // a name as its A-labels, its length first
        let mut name = vec![0x00, 21];
        name.extend_from_slice(b"xn--bcher-kva.example");
        name.extend_from_slice(&[0x1f, 0x90]);
        assert_eq!(address(&at("bücher.example")), Ok(name));
    }

    #[test]
    fn a_name_must_be_one_the_server_can_read() {
        // the longest a length byte gives
        let longest = address(&at(&"a".repeat(255))).unwrap();
        assert_eq!((longest[1], longest.len()), (255, 1 + 1 + 255 + 2));
        let mut out = Vec::new();
        for (host, error) in [
            ("a".repeat(256), AddrError::TooLong),
            ("x@blocked.test".to_string(), AddrError::Unsendable),
            ("a b.test".to_string(), AddrError::Unsendable),
            (String::new(), AddrError::Unsendable),
        ] {
            let target = Target::new(HostName::Domain(host), 80);
            assert_eq!(put(&mut out, &target), Err(error));
        }
        assert!(out.is_empty(), "nothing written for a target not named");
        assert_eq!(
            AddrError::Unsendable.to_string(),
            "tuic: the host name cannot be sent to the server"
        );
        assert_eq!(
            AddrError::TooLong.to_string(),
            "tuic: the host name is longer than 255 bytes"
        );
    }
}
```

新建 `crates/rurge-proto-quic/src/tuic/tcp.rs`：

```rust
//! TCP over TUIC v5 (SPEC, "Connect"; phase 2 M7 design 6.2): every
//! connection a bidirectional stream of its own on the policy's QUIC
//! connection.
//!
//! - The stream begins `05 01 ‖ ADDR`, written as it opens, and is the
//!   tunnel at once: the server answers nothing. A target it cannot reach
//!   shows as the stream ending.
//! - With no answer to wait for, a connection gone unknown to us — its
//!   server restarted — shows only once packets of the tunnel meet the
//!   server. A request goes again on a new connection (`Failure::Lost`)
//!   only when it finds its connection gone as the stream opens or as the
//!   Connect is handed to QUIC; after that the tunnel is the caller's, and
//!   ends with the connection.

use super::VERSION;
use super::addr;
use super::auth::Authenticated;
use crate::session::{Failure, Lease};
use quinn::{RecvStream, SendStream, WriteError};
use rurge_net::connector::Target;
use rurge_proto::OutboundError;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The command's type.
const CONNECT: u8 = 0x01;

/// The Connect for `target`: refused, before anything is dialed, for a
/// target that cannot be named to the server.
pub(super) fn request(target: &Target) -> Result<Vec<u8>, OutboundError> {
    let mut request = vec![VERSION, CONNECT];
    addr::put(&mut request, target).map_err(|e| OutboundError::Proxy(e.to_string()))?;
    Ok(request)
}

/// The tunnel `request` asks for, on the connection of `lease`: the
/// Connect on a stream of its own, handed to QUIC, and the stream is the
/// caller's. A connection found gone by then is `Lost`: nothing of the
/// caller's went out yet.
pub(super) async fn open(lease: Lease<Authenticated>, request: &[u8]) -> Result<Tunnel, Failure> {
    let state = lease.state();
    let (mut send, recv) = lease
        .connection()
        .open_bi()
        .await
        .map_err(|e| Failure::Lost(state.ended(&e).into()))?;
    send.write_all(request).await.map_err(|e| match e {
        WriteError::ConnectionLost(e) => Failure::Lost(state.ended(&e).into()),
        e => Failure::Failed(state.write_error(e).into()),
    })?;
    Ok(Tunnel { send, recv, lease })
}

/// A TCP connection through the server: the Connect's stream, on a
/// connection its lease keeps. Shutdown sends the FIN the target then
/// sees; reading goes on. Dropped, the stream ends both ways (a FIN, and
/// STOP_SENDING for what is left unread). No `Debug`: nothing in it is
/// worth printing.
pub(super) struct Tunnel {
    send: SendStream,
    recv: RecvStream,
    /// Last: the stream ends before its connection may be closed.
    lease: Lease<Authenticated>,
}

impl AsyncRead for Tunnel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let filled = buf.filled().len();
        let state = this.lease.state();
        match ready!(this.recv.poll_read_buf(cx, buf)) {
            Ok(()) => {
                if buf.filled().len() > filled {
                    state.mark_relayed();
                }
                Poll::Ready(Ok(()))
            }
            Err(e) => Poll::Ready(Err(state.read_error(e))),
        }
    }
}

impl AsyncWrite for Tunnel {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        Pin::new(&mut this.send)
            .poll_write(cx, buf)
            .map_err(|e| this.lease.state().write_error(e))
    }

    /// QUIC sends what the stream took.
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // fails only for a stream finished or reset already
        let _ = self.get_mut().send.finish();
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;

    #[test]
    fn a_connect_is_its_header_and_the_address() {
        let target = Target::new(HostName::parse("192.0.2.1"), 443);
        assert_eq!(
            request(&target).unwrap(),
            [0x05, 0x01, 0x01, 192, 0, 2, 1, 0x01, 0xbb]
        );
        let target = Target::new(HostName::parse("example.test"), 80);
        let mut expected = vec![0x05, 0x01, 0x00, 12];
        expected.extend_from_slice(b"example.test");
        expected.extend_from_slice(&[0, 80]);
        assert_eq!(request(&target).unwrap(), expected);
        let unsendable = Target::new(HostName::Domain("a@b.test".into()), 80);
        let err = request(&unsendable).unwrap_err();
        assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
        assert_eq!(
            err.to_string(),
            "tuic: the host name cannot be sent to the server"
        );
    }
}
```

新建 `crates/rurge-proto-quic/src/testing/tuic.rs`：

```rust
//! A scriptable TUIC v5 server (phase 2 M7 design 6.4): QUIC with ALPN
//! `h3` on a loopback port of its own (`ServerPort`: it can restart) or on
//! a socket it is given (`MultiPort` for port hopping). It checks the
//! authentication against what its own side of the TLS session exports,
//! holds the Connects that come before it (as the SPEC asks), and relays
//! each to its target. What the client sends is read by code that is not
//! the client's: the commands and their addresses by the code here. It
//! never resolves a name.

use super::ServerPort;
use quinn::{AsyncUdpSocket, Connection, Endpoint, RecvStream, SendStream, VarInt};
use rurge_net::quic::testing::{TestCert, endpoint};
use rustls::RootCertStore;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::io::{AsyncReadExt, copy_bidirectional_with_sizes};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_util::task::AbortOnDropHandle;

// the wire as the fake knows it
const VERSION: u8 = 0x05;
const AUTHENTICATE: u8 = 0x00;
const CONNECT: u8 = 0x01;
/// How the fake closes a connection, and stops a stream: with 0, as sing-box
/// does.
const CLOSE: VarInt = VarInt::from_u32(0);
/// How much of a TCP tunnel the fake relays at a time, each way.
const RELAY: usize = 64 << 10;

#[derive(Clone, Debug, Default)]
pub struct TuicScript {
    /// The one user it lets in, by its UUID's 16 bytes, and its password.
    pub uuid: [u8; 16],
    pub password: String,
    /// The code it closes a connection with when the authentication
    /// failed, never with a reason: 0 — as tuic-server 1.0.0 and sing-box
    /// close it — unless set.
    pub auth_failure_code: u32,
    /// Relay here whatever the client asked for (needed for a name).
    pub connect_to: Option<SocketAddr>,
}

/// An Authenticate command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedTuicAuth {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    pub uuid: [u8; 16],
    pub token: [u8; 32],
    /// What the fake's own side of the TLS session exports for its user:
    /// the token it lets in.
    pub expected: [u8; 32],
    pub accepted: bool,
}

/// A Connect command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedConnect {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// The target as the command named it: `name:port`, or an address and
    /// its port (IPv6 in brackets).
    pub address: String,
}

pub struct FakeTuic {
    cert: TestCert,
    addr: SocketAddr,
    /// `None` on a socket it was given: it cannot restart.
    port: Option<ServerPort>,
    shared: Arc<Shared>,
    /// Every endpoint so far, with its accept loop: closed with the fake.
    endpoints: Mutex<Vec<(Endpoint, AbortOnDropHandle<()>)>>,
}

/// What the connections of every endpoint share.
struct Shared {
    script: TuicScript,
    /// The server's side of every connection accepted, in order.
    connections: Mutex<Vec<Connection>>,
    auths: Mutex<Vec<RecordedTuicAuth>>,
    connects: Mutex<Vec<RecordedConnect>>,
}

impl Shared {
    fn connections(&self) -> MutexGuard<'_, Vec<Connection>> {
        self.connections.lock().expect("connections")
    }
}

impl FakeTuic {
    /// On a loopback port of its own, which it can take over again
    /// (`restart`).
    pub fn spawn(script: TuicScript) -> FakeTuic {
        let port = ServerPort::bind();
        let fake = FakeTuic::new(script, port.addr(), Some(port));
        fake.take_over();
        fake
    }

    /// On `socket` — a `MultiPort` for port hopping —, which it cannot
    /// take over again.
    pub fn spawn_on(script: TuicScript, socket: Arc<dyn AsyncUdpSocket>) -> FakeTuic {
        let addr = socket.local_addr().expect("the socket's address");
        let fake = FakeTuic::new(script, addr, None);
        fake.listen(endpoint(fake.cert.server_config(&["h3"]), socket));
        fake
    }

    fn new(script: TuicScript, addr: SocketAddr, port: Option<ServerPort>) -> FakeTuic {
        FakeTuic {
            cert: TestCert::new(),
            addr,
            port,
            shared: Arc::new(Shared {
                script,
                connections: Mutex::default(),
                auths: Mutex::default(),
                connects: Mutex::default(),
            }),
            endpoints: Mutex::default(),
        }
    }

    /// Its port: the first of them, on a socket of several.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Roots that trust its certificate (for `localhost` and `127.0.0.1`).
    pub fn roots(&self) -> Arc<RootCertStore> {
        self.cert.roots()
    }

    /// Every Authenticate it read, in arrival order.
    pub fn auths(&self) -> Vec<RecordedTuicAuth> {
        self.shared.auths.lock().expect("auths").clone()
    }

    /// Every Connect it read, in arrival order: before the authentication
    /// went through, as the commands that wait for it.
    pub fn connects(&self) -> Vec<RecordedConnect> {
        self.shared.connects.lock().expect("connects").clone()
    }

    /// The connections accepted so far: QUIC's handshake done.
    pub fn connections(&self) -> usize {
        self.shared.connections().len()
    }

    /// Closes every connection accepted so far — code 0, no reason —, as
    /// sing-box ends its sessions.
    pub fn close_connections(&self) {
        for connection in self.shared.connections().iter() {
            connection.close(CLOSE, b"");
        }
    }

    /// A new endpoint takes the port over, as after a restart: the
    /// connections of the one before are gone, and the client learns it
    /// from a stateless reset of its next packet.
    pub fn restart(&self) {
        self.take_over();
    }

    /// A new endpoint takes the port over; the one before falls silent.
    fn take_over(&self) {
        let port = self.port.as_ref().expect("a fake on a port of its own");
        self.listen(port.endpoint(self.cert.server_config(&["h3"])));
    }

    /// Serves on `endpoint` until the fake is dropped.
    fn listen(&self, endpoint: Endpoint) {
        let accepting = tokio::spawn(accept(endpoint.clone(), self.shared.clone()));
        self.endpoints
            .lock()
            .expect("endpoints")
            .push((endpoint, AbortOnDropHandle::new(accepting)));
    }
}

impl Drop for FakeTuic {
    /// Every connection ends with the fake, and with them the tasks that
    /// serve them.
    fn drop(&mut self) {
        for (endpoint, _) in self.endpoints.get_mut().expect("endpoints").iter() {
            endpoint.close(CLOSE, b"");
        }
    }
}

/// The connections of `endpoint`, each served in a task of its own until
/// it ends.
async fn accept(endpoint: Endpoint, shared: Arc<Shared>) {
    while let Some(incoming) = endpoint.accept().await {
        let shared = shared.clone();
        tokio::spawn(async move {
            if let Ok(connection) = incoming.await {
                serve(connection, shared).await;
            }
        });
    }
}

/// One connection: its streams, each in a task of its own that ends with
/// the connection. The Connects wait for the authentication.
async fn serve(connection: Connection, shared: Arc<Shared>) {
    let number = {
        let mut connections = shared.connections();
        connections.push(connection.clone());
        connections.len() - 1
    };
    let authenticated = watch::Sender::new(false);
    let mut streams = JoinSet::new();
    loop {
        tokio::select! {
            stream = connection.accept_uni() => match stream {
                Ok(stream) => {
                    streams.spawn(authenticate(
                        stream,
                        connection.clone(),
                        number,
                        authenticated.clone(),
                        shared.clone(),
                    ));
                }
                Err(_) => return,
            },
            stream = connection.accept_bi() => match stream {
                Ok((send, recv)) => {
                    streams.spawn(relay(
                        send,
                        recv,
                        number,
                        authenticated.subscribe(),
                        shared.clone(),
                    ));
                }
                Err(_) => return,
            },
            // a stream that ended; nothing to collect
            Some(_) = streams.join_next() => {}
        }
    }
}

/// A unidirectional stream: the authentication, read whole — the client
/// finishes the stream after it — and checked against what the fake's own
/// side of the TLS session exports for its user, with the UUID's bytes as
/// the label and the password as the context. Let in, the connection's
/// Connects go on; refused, the connection is closed as the reference
/// servers close it, with no reason.
async fn authenticate(
    mut stream: RecvStream,
    connection: Connection,
    number: usize,
    authenticated: watch::Sender<bool>,
    shared: Arc<Shared>,
) {
    let Ok(command) = stream.read_to_end(64).await else {
        return;
    };
    let [VERSION, AUTHENTICATE, rest @ ..] = command.as_slice() else {
        return;
    };
    let Some((uuid, token)) = rest.split_first_chunk::<16>() else {
        return;
    };
    let Ok(token) = <[u8; 32]>::try_from(token) else {
        return;
    };
    let script = &shared.script;
    let mut expected = [0u8; 32];
    connection
        .export_keying_material(&mut expected, &script.uuid, script.password.as_bytes())
        .expect("the keying material of a session whose handshake is done");
    let accepted = *uuid == script.uuid && token == expected;
    shared.auths.lock().expect("auths").push(RecordedTuicAuth {
        connection: number,
        uuid: *uuid,
        token,
        expected,
        accepted,
    });
    if accepted {
        authenticated.send_replace(true);
    } else {
        connection.close(VarInt::from_u32(script.auth_failure_code), b"");
    }
}

/// A Connect: read and recorded, held until the authentication went
/// through, then relayed to its target, the half-closes too. One it cannot
/// carry out — no Connect, a name it has no `connect_to` for, a target
/// that refuses — it ends as sing-box ends one whose target it cannot
/// reach: STOP_SENDING (code 0) for what the client sends, FIN for what it
/// reads.
async fn relay(
    send: SendStream,
    mut recv: RecvStream,
    number: usize,
    mut authenticated: watch::Receiver<bool>,
    shared: Arc<Shared>,
) {
    let Some(address) = read_connect(&mut recv).await else {
        return refuse(send, recv);
    };
    let connect = RecordedConnect {
        connection: number,
        address: address.to_string(),
    };
    shared.connects.lock().expect("connects").push(connect);
    // ends with the connection when the authentication fails
    if authenticated.wait_for(|&let_in| let_in).await.is_err() {
        return;
    }
    // never resolves: a name without `connect_to` is a dead end
    let to = shared.script.connect_to.or(match address {
        Address::Ip(to) => Some(to),
        Address::Name(..) => None,
    });
    let upstream = match to {
        Some(to) => TcpStream::connect(to).await.ok(),
        None => None,
    };
    let Some(mut upstream) = upstream else {
        return refuse(send, recv);
    };
    let mut client = tokio::io::join(recv, send);
    // in large pieces, so that a bulk transfer measures the client rather
    // than this relay
    let _ = copy_bidirectional_with_sizes(&mut client, &mut upstream, RELAY, RELAY).await;
}

/// Ends a stream the fake does not relay.
fn refuse(mut send: SendStream, mut recv: RecvStream) {
    let _ = recv.stop(CLOSE);
    let _ = send.finish();
}

/// A target as a command names it.
enum Address {
    Name(String, u16),
    Ip(SocketAddr),
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Address::Name(name, port) => write!(f, "{name}:{port}"),
            Address::Ip(addr) => addr.fmt(f),
        }
    }
}

/// The target of a Connect: `05 01 ‖ ADDR`; `None` for anything else.
async fn read_connect(recv: &mut RecvStream) -> Option<Address> {
    let mut header = [0u8; 2];
    recv.read_exact(&mut header).await.ok()?;
    if header != [VERSION, CONNECT] {
        return None;
    }
    read_address(recv).await
}

/// ADDR, a field at a time (SPEC "Address"): `0x00` a name, its length in
/// a byte first; `0x01` an IPv4 address; `0x02` an IPv6 address; then the
/// port. `None` for none (`0xff`), which names no target, and for a type
/// it does not know.
async fn read_address(recv: &mut RecvStream) -> Option<Address> {
    let ip = match recv.read_u8().await.ok()? {
        0x00 => {
            let mut name = vec![0u8; usize::from(recv.read_u8().await.ok()?)];
            recv.read_exact(&mut name).await.ok()?;
            let name = String::from_utf8(name).ok()?;
            let port = recv.read_u16().await.ok()?;
            return Some(Address::Name(name, port));
        }
        0x01 => {
            let mut octets = [0u8; 4];
            recv.read_exact(&mut octets).await.ok()?;
            IpAddr::from(octets)
        }
        0x02 => {
            let mut octets = [0u8; 16];
            recv.read_exact(&mut octets).await.ok()?;
            IpAddr::from(octets)
        }
        _ => return None,
    };
    let port = recv.read_u16().await.ok()?;
    Some(Address::Ip(SocketAddr::new(ip, port)))
}
```

`crates/rurge-proto-quic/src/testing/mod.rs`——把

```rust
//! by a server that restarts, and a scriptable Hysteria 2 server. Never
//! used by production code.
```

换成

```rust
//! by a server that restarts, and scriptable Hysteria 2 and TUIC servers.
//! Never used by production code.
```

`crates/rurge-proto-quic/src/testing/mod.rs`——把

```rust
mod obfs;
```

换成

```rust
mod obfs;
mod tuic;
```

`crates/rurge-proto-quic/src/testing/mod.rs`——把

```rust
pub use obfs::RecordedFragment;
```

换成

```rust
pub use obfs::RecordedFragment;
pub use tuic::{FakeTuic, RecordedConnect, RecordedTuicAuth, TuicScript};
```

要点：
- 认证：`05 00 ‖ UUID（16 字节）‖ TOKEN（32 字节）`，TOKEN = TLS 导出密钥（label = UUID 原始字节、context = 口令、长度 32）；写在自己的单向流上、写完即关、不等服务端。
- 地址：`00` 域名（长度字节 + 名字，IDN 转 A-label，发不出去的名字拨号前就拒绝）、`01` IPv4、`02` IPv6，后接 2 字节端口。
- 口令错的判断见 P5；关闭码 `NO_ERROR = 0`。

- [ ] **Step 2: 运行**

Run: `cargo test -p rurge-proto-quic tuic`
Expected: 通过（新增 17 条：认证带着 UUID 与 TLS 会话的 token、口令被拒时隧道结束且下一次重拨、TCP 回显（含大数据与半关闭）、三种地址与发不出去的名字、并发隧道共用一条连接、服务端关闭后重连、服务端重启后丢失连接上的隧道结束而下一个请求重拨、端口跳跃中隧道不断、握手不应答时受时限约束、凭据不进错误文本 / 日志 / `Debug`、地址与认证命令的布局、连接结束的文本）。

- [ ] **Step 3: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-proto-quic
git commit -m "feat(proto-quic): TUIC v5 的认证与 TCP——TLS 导出密钥的 token、Connect 即转发；FakeTuic"
```

### Task 3: TUIC v5 的 UDP 与心跳

`udp()` 翻转为原生支持，`open_udp` 给出全锥的 `PacketSocket`（P7–P10）：`native` 模式 `05 02 ‖ ASSOC_ID ‖ PKT_ID ‖ FRAG_TOTAL ‖ FRAG_ID ‖ SIZE ‖ ADDR ‖ 负载`（放不下才分片、只有第一片带地址）；服务端不收数据报时每个包一条单向流，由连接自己的写入任务逐条写完；`Dissociate`；心跳。`FakeTuic` 补上 UDP 一侧（独立的编解码、每个关联一个服务端 UDP socket、自己把大的回包分片、可丢指定的片、"不收数据报"的脚本开关、记下心跳与 `Dissociate`、读不了的单向流照参考服务端关连接）。

**Files:**
- Create: `crates/rurge-proto-quic/src/tuic/{udp.rs, heartbeat.rs}`（自带用例）
- Modify: `crates/rurge-proto-quic/src/tuic/{mod.rs（与用例）, auth.rs（与用例）, addr.rs（与用例）, tcp.rs}`、`src/testing/{mod.rs, tuic.rs}`

**Interfaces:**
- Consumes: Task 2 的认证状态与地址编码；M7a 的 `Session::lease` / `serves` / `serves_without_route`、Hysteria 2 载体的做法（发送锁、每秒一次的路由比较、换连接）。
- Produces: `TuicOutbound::udp()` → `UdpSupport::Native`、`open_udp`；`tuic::udp`（关联、分片与重组、写入任务 `OUTBOX` = 64、`Dissociate`）；`tuic::heartbeat`；`FakeTuic` 的 UDP 一侧（`TuicScript.no_datagrams`、`drop_fragment`、`stream_window`、`hold_streams` / `release_streams`，以及收到的包、心跳与 `Dissociate` 的记录）

- [ ] **Step 1: 先写用例（连同假服务端的 UDP 一侧与新模块）**

假服务端：

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
//! holds the Connects that come before it (as the SPEC asks), and relays
//! each to its target. What the client sends is read by code that is not
//! the client's: the commands and their addresses by the code here. It
//! never resolves a name.

use super::ServerPort;
use quinn::{AsyncUdpSocket, Connection, Endpoint, RecvStream, SendStream, VarInt};
use rurge_net::quic::testing::{TestCert, endpoint};
use rustls::RootCertStore;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::io::{AsyncReadExt, copy_bidirectional_with_sizes};
use tokio::net::TcpStream;
```

换成

```rust
//! holds the commands that come before it (as the SPEC asks; sing-box
//! leaves early datagrams in QUIC's queue so), relays each Connect to its
//! target, and gives each UDP association a loopback socket of its own
//! (full cone), which answers the way the association's first packet came:
//! in datagrams — split into fragments of its own when one cannot hold an
//! answer — or on streams. A unidirectional stream it cannot read as one
//! whole command — one cut short — closes the connection, as sing-box
//! closes it. It can hold the client's streams unread for a while, each
//! taking no more than a window of the script's meanwhile. What the client
//! sends is read by code that is not the client's: the commands and their
//! addresses by the code here, which also writes the answers. It counts
//! heartbeats and records Dissociates. It never resolves a name.

use super::ServerPort;
use quinn::{
    AsyncUdpSocket, Connection, Endpoint, ReadError, ReadToEndError, RecvStream, SendDatagramError,
    SendStream, ServerConfig, TransportConfig, VarInt,
};
use rurge_net::quic::testing::{TestCert, endpoint};
use rustls::RootCertStore;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::io::{AsyncReadExt, copy_bidirectional_with_sizes};
use tokio::net::{TcpStream, UdpSocket};
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
const CONNECT: u8 = 0x01;
```

换成

```rust
const CONNECT: u8 = 0x01;
const PACKET: u8 = 0x02;
const DISSOCIATE: u8 = 0x03;
const HEARTBEAT: u8 = 0x04;
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
const RELAY: usize = 64 << 10;
```

换成

```rust
const RELAY: usize = 64 << 10;
/// The longest unidirectional stream it reads: past any one command.
const MAX_STREAM: usize = 128 << 10;
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    /// Relay here whatever the client asked for (needed for a name).
    pub connect_to: Option<SocketAddr>,
```

换成

```rust
    /// Relay here whatever the client asked for, TCP and UDP alike (needed
    /// for a name).
    pub connect_to: Option<SocketAddr>,
    /// Take no QUIC datagrams at all: the transport parameter that allows
    /// them is not sent, and the client relays UDP on streams (`quic` mode).
    pub no_datagrams: bool,
    /// The window QUIC gives each of the client's streams, in bytes: what
    /// the client sends on one goes no further ahead of what the fake read
    /// of it. quinn's (1.25 MB) unless set.
    pub stream_window: Option<u32>,
    /// The first UDP answer it splits into fragments goes without this
    /// one.
    pub drop_fragment: Option<u8>,
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    pub address: String,
}

pub struct FakeTuic {
```

换成

```rust
    pub address: String,
}

/// A Packet command from the client: a whole UDP packet, or one fragment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedTuicPacket {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    pub association: u16,
    pub packet: u16,
    pub fragments: u8,
    pub fragment: u8,
    /// The address as the command named it, as a Connect's is recorded;
    /// `None` for none.
    pub address: Option<String>,
    /// How long its payload was: a fragment's own part, for a fragment.
    pub len: usize,
    /// It came on a unidirectional stream of its own (`quic` mode), not in
    /// a datagram.
    pub on_stream: bool,
}

/// A Dissociate command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedDissociate {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    pub association: u16,
}

/// A Packet command as the fake reads and writes it, for tests that send
/// their own: `05 02 ‖ ASSOC_ID ‖ PKT_ID ‖ FRAG_TOTAL ‖ FRAG_ID ‖ SIZE ‖
/// ADDR ‖ payload`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TuicPacket {
    pub association: u16,
    pub packet: u16,
    pub fragments: u8,
    pub fragment: u8,
    /// `host:port`, an address (IPv6 in brackets) or a name; `None` for
    /// none.
    pub address: Option<String>,
    pub payload: Vec<u8>,
}

impl TuicPacket {
    /// A UDP packet that goes whole: fragment 0 of 1.
    pub fn whole(association: u16, packet: u16, address: &str, payload: &[u8]) -> TuicPacket {
        TuicPacket {
            association,
            packet,
            fragments: 1,
            fragment: 0,
            address: Some(address.to_string()),
            payload: payload.to_vec(),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![VERSION, PACKET];
        out.extend_from_slice(&self.association.to_be_bytes());
        out.extend_from_slice(&self.packet.to_be_bytes());
        out.extend_from_slice(&[self.fragments, self.fragment]);
        let size = u16::try_from(self.payload.len()).expect("a payload of at most 65535 bytes");
        out.extend_from_slice(&size.to_be_bytes());
        match self.address.as_deref() {
            None => out.push(0xff),
            Some(address) => put_address(&mut out, address),
        }
        out.extend_from_slice(&self.payload);
        out
    }

    /// `None` for anything but a Packet whose SIZE is just what follows its
    /// address, as sing-box reads one.
    fn decode(data: &[u8]) -> Option<TuicPacket> {
        let (header, rest) = data.split_at_checked(10)?;
        if header[..2] != [VERSION, PACKET] {
            return None;
        }
        let (address, rest) = match rest.split_first()? {
            (&0xff, rest) => (None, rest),
            _ => {
                let (address, rest) = take_address(rest)?;
                (Some(address.to_string()), rest)
            }
        };
        if rest.len() != usize::from(u16::from_be_bytes([header[8], header[9]])) {
            return None;
        }
        Some(TuicPacket {
            association: u16::from_be_bytes([header[2], header[3]]),
            packet: u16::from_be_bytes([header[4], header[5]]),
            fragments: header[6],
            fragment: header[7],
            address,
            payload: rest.to_vec(),
        })
    }
}

/// `host:port` as ADDR: an address as its type and bytes, a name with its
/// length first, then the port.
fn put_address(out: &mut Vec<u8>, address: &str) {
    let port = match address.parse::<SocketAddr>() {
        Ok(addr) => {
            match addr.ip() {
                IpAddr::V4(v4) => {
                    out.push(0x01);
                    out.extend_from_slice(&v4.octets());
                }
                IpAddr::V6(v6) => {
                    out.push(0x02);
                    out.extend_from_slice(&v6.octets());
                }
            }
            addr.port()
        }
        Err(_) => {
            let (name, port) = address.rsplit_once(':').expect("host:port");
            out.push(0x00);
            out.push(u8::try_from(name.len()).expect("a name of at most 255 bytes"));
            out.extend_from_slice(name.as_bytes());
            port.parse().expect("a port")
        }
    };
    out.extend_from_slice(&port.to_be_bytes());
}

/// ADDR from its type on (SPEC "Address"), and what follows it: `None` for
/// a type it does not know, or an address cut short.
fn take_address(data: &[u8]) -> Option<(Address, &[u8])> {
    let (kind, rest) = data.split_first()?;
    let (ip, rest) = match kind {
        0x00 => {
            let (&len, rest) = rest.split_first()?;
            let (name, rest) = rest.split_at_checked(usize::from(len))?;
            let (port, rest) = rest.split_at_checked(2)?;
            let name = String::from_utf8(name.to_vec()).ok()?;
            return Some((
                Address::Name(name, u16::from_be_bytes([port[0], port[1]])),
                rest,
            ));
        }
        0x01 => {
            let (octets, rest) = rest.split_first_chunk::<4>()?;
            (IpAddr::from(*octets), rest)
        }
        0x02 => {
            let (octets, rest) = rest.split_first_chunk::<16>()?;
            (IpAddr::from(*octets), rest)
        }
        _ => return None,
    };
    let (port, rest) = rest.split_first_chunk::<2>()?;
    Some((
        Address::Ip(SocketAddr::new(ip, u16::from_be_bytes(*port))),
        rest,
    ))
}

pub struct FakeTuic {
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    connects: Mutex<Vec<RecordedConnect>>,
```

换成

```rust
    connects: Mutex<Vec<RecordedConnect>>,
    packets: Mutex<Vec<RecordedTuicPacket>>,
    dissociates: Mutex<Vec<RecordedDissociate>>,
    heartbeats: AtomicUsize,
    /// Each association's own socket, in the order they opened.
    outside: Mutex<Vec<SocketAddr>>,
    /// The fragment the next UDP answer in fragments goes without.
    drop_fragment: Mutex<Option<u8>>,
    /// Unidirectional streams that come are held unread
    /// (`FakeTuic::hold_streams`).
    holding: watch::Sender<bool>,
    /// The streams held so far.
    held: AtomicUsize,
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
        self.connections.lock().expect("connections")
```

换成

```rust
        self.connections.lock().expect("connections")
    }

    /// Lets a new unidirectional stream through: at once, unless the fake
    /// holds them — then once it lets them go, counted meanwhile.
    async fn let_through(&self) {
        let mut holding = self.holding.subscribe();
        if *holding.borrow() {
            self.held.fetch_add(1, Ordering::SeqCst);
            // `self` keeps the sender
            let _ = holding.wait_for(|&holding| !holding).await;
        }
    }

    /// `answer` in datagrams of at most `max` bytes: whole when it fits,
    /// else in fragments of one size, only the first naming the source. The
    /// first answer split goes without the fragment the script drops.
    fn split(&self, answer: TuicPacket, max: usize) -> Vec<Vec<u8>> {
        let whole = answer.encode();
        if whole.len() <= max {
            return vec![whole];
        }
        let room = max - (whole.len() - answer.payload.len());
        let fragments = u8::try_from(answer.payload.len().div_ceil(room)).expect("255 fragments");
        let dropped = self.drop_fragment.lock().expect("drop").take();
        answer
            .payload
            .chunks(room)
            .zip(0..fragments)
            .filter(|&(_, fragment)| Some(fragment) != dropped)
            .map(|(chunk, fragment)| {
                TuicPacket {
                    association: answer.association,
                    packet: answer.packet,
                    fragments,
                    fragment,
                    address: if fragment == 0 {
                        answer.address.clone()
                    } else {
                        None
                    },
                    payload: chunk.to_vec(),
                }
                .encode()
            })
            .collect()
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
        fake.listen(endpoint(fake.cert.server_config(&["h3"]), socket));
```

换成

```rust
        fake.listen(endpoint(fake.server_config(), socket));
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
            shared: Arc::new(Shared {
```

换成

```rust
            shared: Arc::new(Shared {
                drop_fragment: Mutex::new(script.drop_fragment),
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
                connects: Mutex::default(),
```

换成

```rust
                connects: Mutex::default(),
                packets: Mutex::default(),
                dissociates: Mutex::default(),
                heartbeats: AtomicUsize::new(0),
                outside: Mutex::default(),
                holding: watch::Sender::new(false),
                held: AtomicUsize::new(0),
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
        self.shared.connects.lock().expect("connects").clone()
    }

    /// The connections accepted so far: QUIC's handshake done.
```

换成

```rust
        self.shared.connects.lock().expect("connects").clone()
    }

    /// Every Packet it read, in the order it read them: each fragment
    /// apart.
    pub fn udp_packets(&self) -> Vec<RecordedTuicPacket> {
        self.shared.packets.lock().expect("packets").clone()
    }

    /// Each association's own socket, in the order they opened: whoever
    /// sends to one is heard by the client.
    pub fn udp_outside(&self) -> Vec<SocketAddr> {
        self.shared.outside.lock().expect("outside").clone()
    }

    /// Every Dissociate it read, in arrival order.
    pub fn dissociates(&self) -> Vec<RecordedDissociate> {
        self.shared.dissociates.lock().expect("dissociates").clone()
    }

    /// The heartbeats it heard so far, on every connection.
    pub fn heartbeats(&self) -> usize {
        self.shared.heartbeats.load(Ordering::SeqCst)
    }

    /// Sends `packet`, as it is, to the client in a datagram of connection
    /// `n`.
    pub fn send_udp(&self, n: usize, packet: &TuicPacket) {
        self.connection(n)
            .send_datagram(packet.encode().into())
            .expect("a datagram the client takes");
    }

    /// The connections accepted so far: QUIC's handshake done.
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
        self.shared.connections().len()
```

换成

```rust
        self.shared.connections().len()
    }

    /// The server's side of connection `n`, 0 for the first accepted.
    pub fn connection(&self, n: usize) -> Connection {
        self.shared.connections()[n].clone()
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
            connection.close(CLOSE, b"");
        }
    }

    /// A new endpoint takes the port over, as after a restart: the
```

换成

```rust
            connection.close(CLOSE, b"");
        }
    }

    /// Holds the unidirectional streams that come from now on, unread,
    /// until `release_streams`: what the client sends on one goes no
    /// further than the stream's window (`TuicScript::stream_window`)
    /// meanwhile.
    pub fn hold_streams(&self) {
        self.shared.holding.send_replace(true);
    }

    /// Reads the streams it holds, and those that come from now on.
    pub fn release_streams(&self) {
        self.shared.holding.send_replace(false);
    }

    /// The streams it held so far.
    pub fn held_streams(&self) -> usize {
        self.shared.held.load(Ordering::SeqCst)
    }

    /// A new endpoint takes the port over, as after a restart: the
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
        self.take_over();
    }

    /// A new endpoint takes the port over; the one before falls silent.
```

换成

```rust
        self.take_over();
    }

    /// QUIC with ALPN `h3`: datagrams included unless the script says
    /// none, quinn's stream window unless it sets one.
    fn server_config(&self) -> ServerConfig {
        let script = &self.shared.script;
        let mut transport = TransportConfig::default();
        if script.no_datagrams {
            transport.datagram_receive_buffer_size(None);
        }
        if let Some(window) = script.stream_window {
            transport.stream_receive_window(VarInt::from_u32(window));
        }
        let mut config = self.cert.server_config(&["h3"]);
        config.transport_config(Arc::new(transport));
        config
    }

    /// A new endpoint takes the port over; the one before falls silent.
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
        self.listen(port.endpoint(self.cert.server_config(&["h3"])));
```

换成

```rust
        self.listen(port.endpoint(self.server_config()));
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
/// One connection: its streams, each in a task of its own that ends with
/// the connection. The Connects wait for the authentication.
```

换成

```rust
/// One connection: its streams and its datagrams, each in a task of its
/// own that ends with the connection, as do its associations. The
/// commands wait for the authentication.
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    let mut streams = JoinSet::new();
```

换成

```rust
    let link = Arc::new(Link {
        connection: connection.clone(),
        number,
        associations: Mutex::default(),
        shared: shared.clone(),
    });
    let mut streams = JoinSet::new();
    streams.spawn(read_datagrams(link.clone(), authenticated.subscribe()));
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
                    streams.spawn(authenticate(
                        stream,
                        connection.clone(),
                        number,
                        authenticated.clone(),
                        shared.clone(),
                    ));
```

换成

```rust
                    streams.spawn(serve_uni(stream, link.clone(), authenticated.clone()));
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
/// A unidirectional stream: the authentication, read whole — the client
/// finishes the stream after it — and checked against what the fake's own
/// side of the TLS session exports for its user, with the UUID's bytes as
/// the label and the password as the context. Let in, the connection's
/// Connects go on; refused, the connection is closed as the reference
/// servers close it, with no reason.
async fn authenticate(
    mut stream: RecvStream,
    connection: Connection,
    number: usize,
    authenticated: watch::Sender<bool>,
    shared: Arc<Shared>,
) {
    let Ok(command) = stream.read_to_end(64).await else {
        return;
    };
    let [VERSION, AUTHENTICATE, rest @ ..] = command.as_slice() else {
        return;
    };
    let Some((uuid, token)) = rest.split_first_chunk::<16>() else {
```

换成

```rust
/// A connection and its UDP associations, as its tasks share them.
struct Link {
    connection: Connection,
    /// The connection's number, 0 for the first accepted.
    number: usize,
    associations: Mutex<HashMap<u16, Association>>,
    shared: Arc<Shared>,
}

/// A unidirectional stream, read whole — the client finishes each after
/// its one command —: the Authenticate, or — once that went through — a
/// Packet (`quic` mode) or a Dissociate. A stream that holds no whole
/// command — one cut short — closes the connection, as sing-box closes it
/// on a stream it cannot read: every tunnel and association on it ends.
/// While the fake holds streams, it waits unread.
async fn serve_uni(mut stream: RecvStream, link: Arc<Link>, authenticated: watch::Sender<bool>) {
    link.shared.let_through().await;
    let command = match stream.read_to_end(MAX_STREAM).await {
        Ok(command) => command,
        // nothing left to close
        Err(ReadToEndError::Read(ReadError::ConnectionLost(_))) => return,
        Err(_) => return link.unreadable(),
    };
    match command.get(..2) {
        Some([VERSION, AUTHENTICATE]) => authenticate(&command, &link, &authenticated),
        Some([VERSION, PACKET | DISSOCIATE]) => {
            if authenticated
                .subscribe()
                .wait_for(|&let_in| let_in)
                .await
                .is_err()
            {
                return;
            }
            if command[1] == PACKET {
                match TuicPacket::decode(&command) {
                    Some(packet) => link.relay_packet(packet, true).await,
                    None => link.unreadable(),
                }
            } else if let [_, _, high, low] = command[..] {
                link.dissociate(u16::from_be_bytes([high, low]));
            } else {
                link.unreadable();
            }
        }
        _ => link.unreadable(),
    }
}

/// The authentication, checked against what the fake's own side of the TLS
/// session exports for its user, with the UUID's bytes as the label and the
/// password as the context. Let in, the connection's commands go on;
/// refused, the connection is closed as the reference servers close it,
/// with no reason.
fn authenticate(command: &[u8], link: &Link, authenticated: &watch::Sender<bool>) {
    let Some((uuid, token)) = command[2..].split_first_chunk::<16>() else {
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    let script = &shared.script;
    let mut expected = [0u8; 32];
    connection
```

换成

```rust
    let script = &link.shared.script;
    let mut expected = [0u8; 32];
    link.connection
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    shared.auths.lock().expect("auths").push(RecordedTuicAuth {
        connection: number,
        uuid: *uuid,
        token,
        expected,
        accepted,
    });
```

换成

```rust
    link.shared
        .auths
        .lock()
        .expect("auths")
        .push(RecordedTuicAuth {
            connection: link.number,
            uuid: *uuid,
            token,
            expected,
            accepted,
        });
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
        connection.close(VarInt::from_u32(script.auth_failure_code), b"");
```

换成

```rust
        link.connection
            .close(VarInt::from_u32(script.auth_failure_code), b"");
    }
}

/// The client's datagrams, once the authentication went through — they
/// wait in QUIC's queue until then, as sing-box leaves them —: heartbeats
/// counted, packets relayed.
async fn read_datagrams(link: Arc<Link>, mut authenticated: watch::Receiver<bool>) {
    if authenticated.wait_for(|&let_in| let_in).await.is_err() {
        return;
    }
    while let Ok(datagram) = link.connection.read_datagram().await {
        match datagram.get(..2) {
            Some([VERSION, HEARTBEAT]) => {
                link.shared.heartbeats.fetch_add(1, Ordering::SeqCst);
            }
            Some([VERSION, PACKET]) => {
                if let Some(packet) = TuicPacket::decode(&datagram) {
                    link.relay_packet(packet, false).await;
                }
            }
            _ => {}
        }
    }
}

impl Link {
    /// A packet of the client's: recorded, and sent on by its association —
    /// opened with its first packet, answering the way that came — once it
    /// is whole.
    async fn relay_packet(&self, packet: TuicPacket, on_stream: bool) {
        self.shared
            .packets
            .lock()
            .expect("packets")
            .push(RecordedTuicPacket {
                connection: self.number,
                association: packet.association,
                packet: packet.packet,
                fragments: packet.fragments,
                fragment: packet.fragment,
                address: packet.address.clone(),
                len: packet.payload.len(),
                on_stream,
            });
        let (socket, whole) = {
            let mut associations = self.associations.lock().expect("associations");
            let association = match associations.entry(packet.association) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    match Association::open(self, packet.association, on_stream) {
                        Ok(association) => entry.insert(association),
                        Err(_) => return,
                    }
                }
            };
            (association.socket.clone(), association.put_together(packet))
        };
        let Some((payload, address)) = whole else {
            return;
        };
        // never resolves: a name without `connect_to` is a dead end
        let to = self
            .shared
            .script
            .connect_to
            .or_else(|| address.parse().ok());
        if let Some(to) = to {
            let _ = socket.send_to(&payload, to).await;
        }
    }

    /// A Dissociate: recorded, and the association, if any, goes — its
    /// socket with it.
    fn dissociate(&self, association: u16) {
        self.shared
            .dissociates
            .lock()
            .expect("dissociates")
            .push(RecordedDissociate {
                connection: self.number,
                association,
            });
        self.associations
            .lock()
            .expect("associations")
            .remove(&association);
    }

    /// Closes the connection for a stream it cannot read, as sing-box
    /// closes it: code 0, no reason.
    fn unreadable(&self) {
        self.connection.close(CLOSE, b"");
    }
}

/// One of the client's associations on a connection: a loopback socket of
/// its own, which every packet of the association leaves by, and the
/// packets it is putting together, by id — side by side, as sing-box and
/// tuic-server put them together.
struct Association {
    socket: Arc<UdpSocket>,
    partial: HashMap<u16, Partial>,
    _answers: AbortOnDropHandle<()>,
}

/// A packet being put together: its fragments so far, and the first's
/// address.
struct Partial {
    parts: Vec<Option<Vec<u8>>>,
    address: Option<String>,
}

impl Association {
    /// Its socket, and the task that sends back whatever reaches it, the
    /// way the association's first packet came.
    fn open(link: &Link, association: u16, on_stream: bool) -> io::Result<Association> {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0")?;
        socket.set_nonblocking(true)?;
        let socket = Arc::new(UdpSocket::from_std(socket)?);
        link.shared
            .outside
            .lock()
            .expect("outside")
            .push(socket.local_addr()?);
        let answers = tokio::spawn(relay_answers(
            socket.clone(),
            link.connection.clone(),
            association,
            on_stream,
            link.shared.clone(),
        ));
        Ok(Association {
            socket,
            partial: HashMap::new(),
            _answers: AbortOnDropHandle::new(answers),
        })
    }

    /// The UDP packet `packet` makes whole, and its target: itself when it
    /// is no fragment. A fragment is dropped, as tuic-server drops it, when
    /// its count says it is none, or when it names an address and is not
    /// the first, or names none and is.
    fn put_together(&mut self, packet: TuicPacket) -> Option<(Vec<u8>, String)> {
        if packet.fragments <= 1 {
            return Some((packet.payload, packet.address?));
        }
        let (index, count) = (usize::from(packet.fragment), usize::from(packet.fragments));
        if index >= count || (index == 0) != packet.address.is_some() {
            return None;
        }
        let partial = self.partial.entry(packet.packet).or_insert(Partial {
            parts: Vec::new(),
            address: None,
        });
        if partial.parts.len() != count {
            partial.parts = vec![None; count];
        }
        if index == 0 {
            partial.address = packet.address;
        }
        partial.parts[index].get_or_insert(packet.payload);
        if partial.parts.iter().any(Option::is_none) {
            return None;
        }
        let whole = self.partial.remove(&packet.packet)?;
        Some((
            whole.parts.into_iter().flatten().flatten().collect(),
            whole.address?,
        ))
    }
}

/// Whatever reaches an association's socket goes back to the client under
/// its source — full cone —, packets counted from 0: on a stream of its own
/// each when the association's first packet came so, else in datagrams, in
/// fragments when one cannot hold it.
async fn relay_answers(
    socket: Arc<UdpSocket>,
    connection: Connection,
    association: u16,
    on_stream: bool,
    shared: Arc<Shared>,
) {
    let mut buf = vec![0u8; 65536];
    let mut packets = 0u16;
    loop {
        let (n, from) = match socket.recv_from(&mut buf).await {
            Ok(got) => got,
            // an ICMP "unreachable" for an earlier packet (Windows)
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
            Err(_) => return,
        };
        let answer = TuicPacket::whole(association, packets, &from.to_string(), &buf[..n]);
        packets = packets.wrapping_add(1);
        if on_stream {
            let Ok(mut stream) = connection.open_uni().await else {
                return;
            };
            if stream.write_all(&answer.encode()).await.is_ok() {
                let _ = stream.finish();
            }
            continue;
        }
        let Some(max) = connection.max_datagram_size() else {
            return;
        };
        for datagram in shared.split(answer, max) {
            if let Err(SendDatagramError::ConnectionLost(_)) =
                connection.send_datagram(datagram.into())
            {
                return;
            }
        }
```

`crates/rurge-proto-quic/src/testing/mod.rs`——把

```rust
pub use tuic::{FakeTuic, RecordedConnect, RecordedTuicAuth, TuicScript};
```

换成

```rust
pub use tuic::{
    FakeTuic, RecordedConnect, RecordedDissociate, RecordedTuicAuth, RecordedTuicPacket,
    TuicPacket, TuicScript,
};
```

出站的 UDP 与心跳用例：

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
    use crate::test_log::Log;
    use crate::testing::{FakeTuic, RecordedConnect, TuicScript};
```

换成

```rust
    use crate::session::Lease;
    use crate::test_log::Log;
    use crate::testing::{FakeTuic, RecordedConnect, RecordedDissociate, TuicPacket, TuicScript};
    use auth::Authenticated;
    use quinn::{ConnectionError, VarInt};
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_net::quic::testing::MultiPort;
    use rurge_proto::testing::echo_server;
```

换成

```rust
    use rurge_net::connector::{
        BoxedDatagram, DirectConnector, PacketSocket, Route, SystemResolve,
    };
    use rurge_net::quic::testing::MultiPort;
    use rurge_proto::testing::{echo_server, udp_echo_server};
    use std::collections::{BTreeSet, HashMap};
    use std::hash::{DefaultHasher, Hash, Hasher};
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
    use std::path::Path;
```

换成

```rust
    use std::path::Path;
    use std::sync::Mutex;
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
```

换成

```rust
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio::sync::Barrier;
    use tokio::task::JoinSet;
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
            assert!(!log.contains(secret));
        }
    }
}

```

换成

```rust
            assert!(!log.contains(secret));
        }
    }

    fn direct() -> Arc<dyn Connector> {
        Arc::new(DirectConnector::new(Arc::new(SystemResolve)))
    }

    /// The outbound of the fake's user, its connections dialled through
    /// `connector` and its heartbeat every `every`.
    fn built(fake: &FakeTuic, connector: Arc<dyn Connector>, every: Duration) -> TuicOutbound {
        let port = fake.addr().port();
        let spec = spec(port, &credentials(ID, PASSWORD));
        let dialer = Dialer::new(
            connector,
            Target::new(HostName::parse("127.0.0.1"), port),
            &spec.tls,
            &spec.quic,
            Tristate::Auto,
            &[],
            fake.roots(),
        )
        .unwrap();
        let auth = Auth::new(UUID, PASSWORD, every);
        TuicOutbound {
            name: "T".to_string(),
            session: Arc::new(Session::new("T", dialer, auth)),
        }
    }

    /// A lease on the outbound's connection, dialed when there is none.
    async fn held(out: &TuicOutbound) -> Lease<Authenticated> {
        timeout(BOUND, out.session.lease(&ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .expect("a connection")
    }

    async fn udp_carrier(out: &TuicOutbound) -> BoxedPacketSocket {
        timeout(BOUND, out.open_udp(&ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .expect("a carrier")
    }

    async fn udp_answer(carrier: &dyn PacketSocket) -> (Vec<u8>, Target) {
        let mut buf = vec![0u8; 65536];
        let (n, from) = timeout(BOUND, carrier.recv_from(&mut buf))
            .await
            .expect("an answer within the bound")
            .unwrap();
        (buf[..n].to_vec(), from)
    }

    async fn udp_round_trip(carrier: &dyn PacketSocket, to: SocketAddr, payload: &[u8]) {
        carrier.send_to(payload, &target(to)).await.unwrap();
        assert_eq!(udp_answer(carrier).await, (payload.to_vec(), target(to)));
    }

    /// Nothing comes for a while: a fixed wait, only to see nothing happen.
    async fn no_udp_answer(carrier: &dyn PacketSocket) {
        let mut buf = vec![0u8; 65536];
        let quiet = timeout(Duration::from_millis(300), carrier.recv_from(&mut buf)).await;
        assert!(quiet.is_err(), "{quiet:?}");
    }

    /// Sends `payload` to the echo at `to` until it comes back: what goes
    /// out on a connection as it ends is lost.
    async fn until_answered(carrier: &dyn PacketSocket, to: SocketAddr, payload: &[u8]) {
        let deadline = Instant::now() + BOUND;
        let mut buf = vec![0u8; 65536];
        loop {
            assert!(Instant::now() < deadline, "no answer within the bound");
            carrier.send_to(payload, &target(to)).await.unwrap();
            let wait = Instant::now() + Duration::from_millis(500);
            while let Ok(got) = tokio::time::timeout_at(wait, carrier.recv_from(&mut buf)).await {
                let (n, _) = got.unwrap();
                if buf[..n] == *payload {
                    return;
                }
            }
        }
    }

    /// Waits for `what` to hold, polling within the bound.
    async fn eventually(what: &str, mut holds: impl FnMut() -> bool) {
        let deadline = Instant::now() + BOUND;
        while !holds() {
            assert!(Instant::now() < deadline, "never: {what}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// `(connection, association)` of every packet the fake read.
    fn carried(fake: &FakeTuic) -> Vec<(usize, u16)> {
        fake.udp_packets()
            .iter()
            .map(|p| (p.connection, p.association))
            .collect()
    }

    /// Design 6.2: where the server takes QUIC datagrams, a packet goes
    /// whole in one — association 0, the first of the connection; packets
    /// counted from 0; fragment 0 of 1 —, its target named in it. The
    /// carrier reaches any target through its association's one socket on
    /// the server, and hears anyone who sends to that socket (full cone);
    /// TCP goes on beside it.
    #[tokio::test]
    async fn udp_goes_in_datagrams_of_the_connection_to_any_target() {
        let (one, two) = (udp_echo_server().await, udp_echo_server().await);
        let tcp_echo = echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        assert_eq!(out.udp(), UdpSupport::Native);
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), one, b"to one").await;
        udp_round_trip(carrier.as_ref(), two, b"to two").await;
        let packets = fake.udp_packets();
        assert_eq!(packets.len(), 2);
        for (packet, (id, to)) in packets.iter().zip([(0, one), (1, two)]) {
            assert_eq!(
                (packet.connection, packet.association, packet.packet),
                (0, 0, id)
            );
            assert_eq!((packet.fragments, packet.fragment), (1, 0));
            assert_eq!(packet.address, Some(to.to_string()));
            assert_eq!(packet.len, 6);
            assert!(!packet.on_stream);
        }
        let outside = fake.udp_outside();
        assert_eq!(outside.len(), 1, "one socket for the association");
        let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        stranger.send_to(b"unasked", outside[0]).await.unwrap();
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"unasked".to_vec(), target(stranger.local_addr().unwrap()))
        );
        let mut stream = tunnel(&out, &target(tcp_echo)).await;
        round_trip(&mut stream, b"beside").await;
        assert_eq!(fake.connections(), 1);
        assert_eq!(fake.auths().len(), 1);
    }

    /// Design 6.2: where the server takes no QUIC datagrams, each packet
    /// goes whole on a unidirectional stream of its own — the long ones too
    /// —, and the server answers on streams, full cone as well. No
    /// heartbeat goes: the SPEC sends it in a datagram only.
    #[tokio::test]
    async fn without_datagrams_each_packet_goes_on_a_stream_of_its_own() {
        let echo = udp_echo_server().await;
        let tcp_echo = echo_server().await;
        let fake = FakeTuic::spawn(TuicScript {
            no_datagrams: true,
            ..script()
        });
        let out = user(&fake);
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"on a stream").await;
        let long: Vec<u8> = (0..9000).map(|i| (i % 251) as u8).collect();
        udp_round_trip(carrier.as_ref(), echo, &long).await;
        let packets = fake.udp_packets();
        assert_eq!(packets.len(), 2);
        for (packet, len) in packets.iter().zip([11, 9000]) {
            assert!(packet.on_stream);
            assert_eq!((packet.fragments, packet.fragment), (1, 0));
            assert_eq!(
                (packet.address.clone(), packet.len),
                (Some(echo.to_string()), len)
            );
        }
        let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        stranger
            .send_to(b"unasked", fake.udp_outside()[0])
            .await
            .unwrap();
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"unasked".to_vec(), target(stranger.local_addr().unwrap()))
        );
        let mut stream = tunnel(&out, &target(tcp_echo)).await;
        round_trip(&mut stream, b"beside").await;
        let lease = held(&out).await;
        assert!(lease.connection().max_datagram_size().is_none());
        assert!(!lease.state().beats());
        assert_eq!(fake.connections(), 1);
    }

    /// A name goes to the server as a name — an IDN as its A-labels —, an
    /// address as its bytes; an answer names its source as the server
    /// heard it, a name too (as sing-box names one). A target that cannot
    /// be named is refused, and nothing goes out.
    #[tokio::test]
    async fn udp_targets_go_out_as_the_server_reads_them() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(TuicScript {
            connect_to: Some(echo),
            ..script()
        });
        let out = user(&fake);
        let carrier = udp_carrier(&out).await;
        for (host, address) in [
            (
                HostName::Domain("bücher.example".into()),
                "xn--bcher-kva.example:53",
            ),
            (HostName::parse("2001:db8::1"), "[2001:db8::1]:53"),
        ] {
            carrier.send_to(b"q", &Target::new(host, 53)).await.unwrap();
            assert_eq!(
                udp_answer(carrier.as_ref()).await,
                (b"q".to_vec(), target(echo))
            );
            let last = fake.udp_packets().pop().unwrap();
            assert_eq!(last.address.as_deref(), Some(address));
        }
        let unsendable = "tuic: the host name cannot be sent to the server";
        for (name, text) in [
            ("x@blocked.test".to_string(), unsendable),
            ("a b.test".to_string(), unsendable),
            (String::new(), unsendable),
            (
                "a".repeat(256),
                "tuic: the host name is longer than 255 bytes",
            ),
        ] {
            let err = carrier
                .send_to(b"q", &Target::new(HostName::Domain(name), 53))
                .await
                .unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(err.to_string(), text);
        }
        assert_eq!(fake.udp_packets().len(), 2);
        fake.send_udp(0, &TuicPacket::whole(0, 7, "dns.example:53", b"by name"));
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (
                b"by name".to_vec(),
                Target::new(HostName::parse("dns.example"), 53)
            )
        );
    }

    /// Design 6.2: a packet too long for one QUIC datagram goes in
    /// fragments under its packet id, only the first naming the target;
    /// the echo, as long, comes back in fragments of the server's that the
    /// carrier puts together. (At most 9216 bytes: macOS sends no longer
    /// UDP datagram by default.)
    #[tokio::test]
    async fn long_packets_go_in_fragments_both_ways() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let carrier = udp_carrier(&out).await;
        for (id, len) in [(0, 3000), (1, 9000)] {
            let payload: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            udp_round_trip(carrier.as_ref(), echo, &payload).await;
            let packets = fake.udp_packets();
            let count = packets.last().unwrap().fragments;
            assert!(count > 1, "{count}");
            let fragments = &packets[packets.len() - usize::from(count)..];
            for (i, fragment) in fragments.iter().enumerate() {
                assert_eq!(
                    (fragment.association, fragment.packet, fragment.fragment),
                    (0, id, i as u8)
                );
                let address = (i == 0).then(|| echo.to_string());
                assert_eq!(fragment.address, address, "fragment {i}");
            }
            assert_eq!(fragments.iter().map(|f| f.len).sum::<usize>(), len);
        }
    }

    /// Design 6.2: a fragment lost loses its packet and nothing more.
    #[tokio::test]
    async fn a_lost_fragment_loses_its_packet_alone() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(TuicScript {
            drop_fragment: Some(1),
            ..script()
        });
        let out = user(&fake);
        let carrier = udp_carrier(&out).await;
        let (lost, kept) = (vec![1u8; 3000], vec![2u8; 3000]);
        for payload in [&lost[..], &kept, b"short"] {
            carrier.send_to(payload, &target(echo)).await.unwrap();
        }
        // in the order they came: the first never came whole
        assert_eq!(udp_answer(carrier.as_ref()).await.0, kept);
        assert_eq!(udp_answer(carrier.as_ref()).await.0, b"short");
        no_udp_answer(carrier.as_ref()).await;
    }

    /// Design 6.2: an association puts at most 8 packets together at a
    /// time; a ninth drops the one whose first fragment came first, which
    /// then never comes whole.
    #[tokio::test]
    async fn an_association_puts_at_most_eight_packets_together_at_a_time() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"hello").await;
        let from = "192.0.2.7:7";
        let fragment = |packet: u16, fragment: u8, payload: &str| TuicPacket {
            association: 0,
            packet,
            fragments: 2,
            fragment,
            address: (fragment == 0).then(|| from.to_string()),
            payload: payload.into(),
        };
        for packet in 1..=9 {
            fake.send_udp(0, &fragment(packet, 0, &format!("{packet}-")));
        }
        for packet in (2..=9).chain([1]) {
            fake.send_udp(0, &fragment(packet, 1, "x"));
        }
        let source = Target::new(HostName::parse("192.0.2.7"), 7);
        for packet in 2..=9 {
            assert_eq!(
                udp_answer(carrier.as_ref()).await,
                (format!("{packet}-x").into_bytes(), source.clone())
            );
        }
        no_udp_answer(carrier.as_ref()).await;
    }

    /// What the digest target answers for `payload`.
    fn digest(payload: &[u8]) -> Vec<u8> {
        let mut hasher = DefaultHasher::new();
        payload.hash(&mut hasher);
        hasher.finish().to_be_bytes().to_vec()
    }

    /// A loopback UDP target that answers each packet with a digest of it:
    /// what comes back is small and whole, so it is lost to nothing but
    /// what went out broken.
    async fn digest_target() -> SocketAddr {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 65536];
            loop {
                let (n, from) = match socket.recv_from(&mut buf).await {
                    Ok(got) => got,
                    // an ICMP "unreachable" for an earlier packet (Windows)
                    Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                    Err(_) => return,
                };
                let _ = socket.send_to(&digest(&buf[..n]), from).await;
            }
        });
        addr
    }

    /// Tasks that send through one carrier at once, on threads of their
    /// own, keep each packet's fragments together on the wire — as a server
    /// that puts one packet of an association together at a time needs
    /// (Clash.Meta's first TUIC v5 server did) — and every packet reaches
    /// the target whole.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_sends_keep_each_packets_fragments_together() {
        const TASKS: u8 = 8;
        const ROUNDS: u8 = 5;
        let to = target(digest_target().await);
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let carrier: Arc<dyn PacketSocket> = Arc::from(udp_carrier(&out).await);
        let mut buf = vec![0u8; 65536];
        for round in 0..ROUNDS {
            // 3000 bytes, three fragments or more, unlike any other's
            let payloads: Vec<Vec<u8>> = (0..TASKS)
                .map(|task| {
                    (0..3000u32)
                        .map(|i| (i as u8) ^ (round << 4 | task))
                        .collect()
                })
                .collect();
            // all of them at once
            let start = Arc::new(Barrier::new(payloads.len()));
            let mut sending = JoinSet::new();
            for payload in payloads.clone() {
                let (carrier, start, to) = (carrier.clone(), start.clone(), to.clone());
                sending.spawn(async move {
                    start.wait().await;
                    carrier.send_to(&payload, &to).await.unwrap();
                });
            }
            while let Some(sent) = sending.join_next().await {
                sent.unwrap();
            }
            let mut expected: Vec<Vec<u8>> = payloads.iter().map(|p| digest(p)).collect();
            let mut got = Vec::new();
            let deadline = Instant::now() + BOUND;
            while got.len() < expected.len() {
                let Ok(answer) =
                    tokio::time::timeout_at(deadline, carrier.recv_from(&mut buf)).await
                else {
                    break;
                };
                let (n, _) = answer.unwrap();
                got.push(buf[..n].to_vec());
            }
            expected.sort();
            got.sort();
            assert_eq!(got, expected, "round {round}");
        }
        // in the order the server read them, each packet's fragments in a
        // row
        let mut positions: HashMap<u16, Vec<usize>> = HashMap::new();
        for (at, packet) in fake.udp_packets().iter().enumerate() {
            positions.entry(packet.packet).or_default().push(at);
        }
        assert_eq!(positions.len(), usize::from(TASKS * ROUNDS));
        for (packet, at) in positions {
            assert!(at.len() > 1, "packet {packet} went whole");
            assert_eq!(
                at[at.len() - 1] - at[0] + 1,
                at.len(),
                "packet {packet}: {at:?}"
            );
        }
    }

    /// Design 6.2: a carrier gone dissociates its association on the
    /// connection it was on, which forgets it: what the server sends for it
    /// afterwards is dropped. The next carrier is an association of its
    /// own, with a socket of its own.
    #[tokio::test]
    async fn a_carrier_gone_dissociates_its_association() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let lease = held(&out).await;
        let udp = &lease.state().udp;
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"here").await;
        assert_eq!(udp.associations(), [0]);
        assert_eq!(lease.state().relays(), 1);
        drop(carrier);
        assert!(udp.associations().is_empty());
        assert_eq!(lease.state().relays(), 0);
        eventually("the Dissociate", || {
            fake.dissociates()
                == [RecordedDissociate {
                    connection: 0,
                    association: 0,
                }]
        })
        .await;
        fake.send_udp(0, &TuicPacket::whole(0, 9, "192.0.2.1:53", b"too late"));
        let next = udp_carrier(&out).await;
        udp_round_trip(next.as_ref(), echo, b"next").await;
        assert_eq!(udp.associations(), [1]);
        assert_eq!(fake.udp_outside().len(), 2);
        assert_eq!(fake.connections(), 1);
    }

    /// No heartbeat comes for a while, past one that may be on its way: a
    /// fixed wait, only to see nothing happen.
    async fn no_heartbeat(fake: &FakeTuic, every: Duration) {
        tokio::time::sleep(every * 3).await;
        let heard = fake.heartbeats();
        tokio::time::sleep(every * 5).await;
        assert_eq!(fake.heartbeats(), heard, "a heartbeat came");
    }

    /// Design 6.2: the heartbeat goes while a relay is under way on the
    /// connection — a TCP tunnel, a UDP carrier —, and not while none is.
    /// Its interval is cut short here (3 seconds otherwise).
    #[tokio::test]
    async fn heartbeats_go_while_a_relay_is_under_way_and_stop_when_none_is() {
        const EVERY: Duration = Duration::from_millis(100);
        let (tcp_echo, udp_echo) = (echo_server().await, udp_echo_server().await);
        let fake = FakeTuic::spawn(script());
        let out = built(&fake, direct(), EVERY);
        // up, and nothing relayed
        let lease = held(&out).await;
        assert!(lease.state().beats());
        no_heartbeat(&fake, EVERY).await;
        assert_eq!(fake.heartbeats(), 0);
        let mut tcp = tunnel(&out, &target(tcp_echo)).await;
        round_trip(&mut tcp, b"tcp").await;
        eventually("heartbeats with a tunnel", || fake.heartbeats() >= 2).await;
        drop(tcp);
        no_heartbeat(&fake, EVERY).await;
        let before = fake.heartbeats();
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), udp_echo, b"udp").await;
        eventually("heartbeats with a carrier", || {
            fake.heartbeats() >= before + 2
        })
        .await;
        drop(carrier);
        no_heartbeat(&fake, EVERY).await;
        assert_eq!(lease.state().relays(), 0);
        assert_eq!(fake.connections(), 1);
    }

    /// The connection's tasks — the one that reads the server's packets,
    /// the writer, the heartbeat — end with it.
    #[tokio::test]
    async fn the_tasks_of_a_connection_end_with_it() {
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let lease = held(&out).await;
        let state = lease.state();
        assert!(state.udp.is_reading() && state.udp.is_writing() && state.beats());
        // nothing answers the authentication: the server's side may come
        // after the client's
        eventually("the server's side", || fake.connections() == 1).await;
        fake.close_connections();
        eventually("the tasks' end", || {
            !state.udp.is_reading() && !state.udp.is_writing() && !state.beats()
        })
        .await;
    }

    /// Design 6.2: a carrier whose connection ended goes on, on the
    /// connection that replaces it — whether the server closed the
    /// connection, or lost it in a restart (a stateless reset ends it at
    /// the next packet) —, under the id that connection gives it: each
    /// counts its own from 0. A carrier opened later gets an id of its own.
    #[tokio::test]
    async fn a_carrier_moves_to_a_new_connection_after_a_close_or_a_restart() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"first").await;
        fake.close_connections();
        until_answered(carrier.as_ref(), echo, b"after a close").await;
        fake.restart();
        until_answered(carrier.as_ref(), echo, b"after a restart").await;
        let carried = carried(&fake);
        assert!(carried.iter().all(|&(_, association)| association == 0));
        let connections: BTreeSet<usize> =
            carried.iter().map(|&(connection, _)| connection).collect();
        assert_eq!(connections, BTreeSet::from([0, 1, 2]));
        let later = udp_carrier(&out).await;
        udp_round_trip(later.as_ref(), echo, b"later").await;
        assert_eq!(fake.udp_packets().last().unwrap().association, 1);
        // the server lost the old connections: nothing to dissociate there
        assert!(fake.dissociates().is_empty());
    }

    /// A UDP packet from the server tells the authentication went through,
    /// as a byte from a tunnel's target does: a later close with code 0 and
    /// no reason is the server's, not a refusal.
    #[tokio::test]
    async fn an_answer_tells_the_authentication_went_through() {
        let (tcp_echo, udp_echo) = (echo_server().await, udp_echo_server().await);
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        // nothing read from it
        let mut tcp = tunnel(&out, &target(tcp_echo)).await;
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), udp_echo, b"answered").await;
        fake.close_connections();
        assert_eq!(
            end_of(&mut tcp).await.to_string(),
            "tuic: the server closed the connection (code 0x0)"
        );
    }

    /// Design 6.2: the server answers no authentication, so a carrier comes
    /// up all the same; its packets go until the refusal closes the
    /// connection. The send that finds it so fails, once, as Hysteria 2's
    /// carrier fails where its authentication is refused; the next one binds
    /// anew, on a new connection.
    #[tokio::test]
    async fn a_refused_authentication_fails_a_send_once_and_the_next_binds_anew() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = outbound(&fake, &credentials(ID, "wrong"));
        let carrier = udp_carrier(&out).await;
        let deadline = Instant::now() + BOUND;
        let err = loop {
            assert!(Instant::now() < deadline, "no send failed");
            match carrier.send_to(b"x", &target(echo)).await {
                Ok(()) => tokio::time::sleep(Duration::from_millis(5)).await,
                Err(e) => break e,
            }
        };
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(err.to_string(), "tuic: authentication failed");
        carrier.send_to(b"y", &target(echo)).await.unwrap();
        eventually("the next authentication", || fake.auths().len() == 2).await;
        assert!(fake.auths().iter().all(|auth| !auth.accepted));
        assert!(fake.udp_packets().is_empty());
    }

    /// Dials directly, leads where it is told to, and counts the times it
    /// is asked where it leads, as a group that is the `underlying-proxy`
    /// leads to its member.
    struct Rerouted {
        inner: DirectConnector,
        route: Mutex<Arc<u8>>,
        asked: AtomicUsize,
    }

    impl Rerouted {
        fn new() -> Rerouted {
            Rerouted {
                inner: DirectConnector::new(Arc::new(SystemResolve)),
                route: Mutex::new(Arc::new(1)),
                asked: AtomicUsize::new(0),
            }
        }

        fn asked(&self) -> usize {
            self.asked.load(Ordering::Relaxed)
        }
    }

    impl Connector for Rerouted {
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
            self.inner.connect_udp(target, opts)
        }

        fn route(&self) -> Option<Route> {
            self.asked.fetch_add(1, Ordering::Relaxed);
            Some(Route::of(&self.route.lock().unwrap()))
        }
    }

    /// Let go of by the client: closed as its session closes a connection,
    /// with code 0.
    async fn closed_by_the_client(connection: &quinn::Connection) {
        let closed = timeout(BOUND, connection.closed()).await.expect("closed");
        assert!(
            matches!(&closed, ConnectionError::ApplicationClosed(close)
                if close.error_code == VarInt::from_u32(0)),
            "{closed:?}"
        );
    }

    /// Phase 2 M7 design 3.5 and 6.2: the underlying policy leads elsewhere
    /// now (a group's member changed). Each carrier's next packet goes on a
    /// new connection, dialled the new way, under the id it gives; the
    /// associations left behind are dissociated on the connection before,
    /// which the tunnel under way keeps — closed once nothing holds it. Said
    /// once in the log, which names the policy and nothing secret.
    #[tokio::test]
    async fn carriers_move_off_a_connection_whose_underlying_policy_changed() {
        let (log, _logging, _second) = Log::capture();
        let (udp_echo, tcp_echo) = (udp_echo_server().await, echo_server().await);
        let fake = FakeTuic::spawn(script());
        let rerouted = Arc::new(Rerouted::new());
        let out = built(&fake, rerouted.clone(), heartbeat::EVERY);
        let (one, two) = (udp_carrier(&out).await, udp_carrier(&out).await);
        udp_round_trip(one.as_ref(), udp_echo, b"one").await;
        udp_round_trip(two.as_ref(), udp_echo, b"two").await;
        let mut under_way = tunnel(&out, &target(tcp_echo)).await;
        let lease = held(&out).await;
        *rerouted.route.lock().unwrap() = Arc::new(2);
        // A carrier asks where the policy leads at most once a second (its
        // own look: `a_carrier_finds_a_changed_route_by_itself`). Here
        // someone else looks first, as a request would, and the carriers
        // see the connection off the way at once.
        assert!(!out.session.serves(&lease));
        drop(lease);
        udp_round_trip(one.as_ref(), udp_echo, b"one, moved").await;
        udp_round_trip(two.as_ref(), udp_echo, b"two, moved").await;
        round_trip(&mut under_way, b"still there").await;
        assert_eq!(carried(&fake), [(0, 0), (0, 1), (1, 0), (1, 1)]);
        assert_eq!(fake.connections(), 2);
        eventually("the Dissociates", || fake.dissociates().len() == 2).await;
        let mut dissociated: Vec<(usize, u16)> = fake
            .dissociates()
            .iter()
            .map(|d| (d.connection, d.association))
            .collect();
        dissociated.sort_unstable();
        assert_eq!(dissociated, [(0, 0), (0, 1)]);
        drop(under_way);
        closed_by_the_client(&fake.connection(0)).await;
        let log = log.text();
        let changed = "tuic: the underlying policy changed; a new connection is made";
        assert_eq!(log.matches(changed).count(), 1, "{log}");
        assert!(log.contains("policy=T"), "{log}");
        for secret in [PASSWORD, ID] {
            assert!(!log.contains(secret));
        }
    }

    /// Where the policy leads can cost a group's resolution: a carrier asks
    /// at most once a second, however many packets it sends meanwhile.
    #[tokio::test]
    async fn the_route_is_compared_at_most_once_a_second_on_udp() {
        let to = target(digest_target().await);
        let fake = FakeTuic::spawn(script());
        let rerouted = Arc::new(Rerouted::new());
        let out = built(&fake, rerouted.clone(), heartbeat::EVERY);
        let carrier = udp_carrier(&out).await;
        let before = rerouted.asked();
        for i in 0..50u8 {
            carrier.send_to(&[i], &to).await.unwrap();
        }
        let asked = rerouted.asked() - before;
        assert!(asked <= 3, "asked {asked} times for 50 packets");
    }

    /// A carrier finds out for itself, at its next comparison of the
    /// route, that the policy leads elsewhere now: its packets then go on a
    /// new connection. Until then — a second at most — they go on the one
    /// before.
    #[tokio::test]
    async fn a_carrier_finds_a_changed_route_by_itself() {
        let to = target(digest_target().await);
        let fake = FakeTuic::spawn(script());
        let rerouted = Arc::new(Rerouted::new());
        let out = built(&fake, rerouted.clone(), heartbeat::EVERY);
        let carrier = udp_carrier(&out).await;
        carrier.send_to(b"first", &to).await.unwrap();
        *rerouted.route.lock().unwrap() = Arc::new(2);
        let deadline = Instant::now() + BOUND;
        while !fake.udp_packets().iter().any(|p| p.connection == 1) {
            assert!(
                Instant::now() < deadline,
                "the carrier never left its connection"
            );
            carrier.send_to(b"probe", &to).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(fake.connections(), 2);
    }

    /// A carrier that does not read loses what its inbox cannot hold, and
    /// holds up no other carrier of the connection.
    #[tokio::test]
    async fn a_full_inbox_drops_rather_than_holds_up_the_other_carriers() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let (slow, other) = (udp_carrier(&out).await, udp_carrier(&out).await);
        udp_round_trip(slow.as_ref(), echo, b"slow").await;
        udp_round_trip(other.as_ref(), echo, b"other").await;
        assert_eq!(carried(&fake), [(0, 0), (0, 1)]);
        let from = "192.0.2.9:9";
        for i in 0..udp::INBOX as u16 + 10 {
            fake.send_udp(0, &TuicPacket::whole(0, i, from, &i.to_be_bytes()));
        }
        fake.send_udp(0, &TuicPacket::whole(1, 0, from, b"not held up"));
        assert_eq!(udp_answer(other.as_ref()).await.0, b"not held up");
        for i in 0..udp::INBOX as u16 {
            assert_eq!(udp_answer(slow.as_ref()).await.0, i.to_be_bytes());
        }
        no_udp_answer(slow.as_ref()).await;
    }

    /// Design 6.2: where the server takes no QUIC datagrams, a send hands
    /// its packet to the connection's writer and waits for nothing more, so
    /// that one its caller gives up on — the engine bounds every send, and
    /// cuts one short when its association ends — still goes whole. Cut
    /// short on the wire, it would be a stream sing-box cannot read, for
    /// which it closes the connection, the tunnel beside it too, as the fake
    /// does. Here the server holds the packet's stream unread, its window
    /// shorter than the packet, until the send was given up on.
    #[tokio::test]
    async fn a_send_given_up_on_never_cuts_its_packet_short() {
        let (tcp_echo, udp_echo) = (echo_server().await, udp_echo_server().await);
        let fake = FakeTuic::spawn(TuicScript {
            no_datagrams: true,
            stream_window: Some(8),
            ..script()
        });
        let out = user(&fake);
        let mut tcp = tunnel(&out, &target(tcp_echo)).await;
        round_trip(&mut tcp, b"up").await;
        let carrier = udp_carrier(&out).await;
        let to = target(udp_echo);
        fake.hold_streams();
        let held = || fake.held_streams() == 1;
        // given up on once the server holds its stream, if not done by then
        tokio::select! {
            sent = carrier.send_to(b"whole", &to) => sent.unwrap(),
            () = eventually("the packet's stream held", held) => {}
        }
        eventually("the packet's stream held", held).await;
        fake.release_streams();
        assert_eq!(udp_answer(carrier.as_ref()).await, (b"whole".to_vec(), to));
        round_trip(&mut tcp, b"still up").await;
        assert_eq!(fake.connections(), 1);
    }

    /// Design 6.2: a send never waits for the connection's writer. Behind a
    /// stream the server does not read, the writer holds one packet half
    /// sent and `OUTBOX` more wait for it; the packets past them are
    /// dropped, the send `Ok` all the same — as a datagram the connection
    /// cannot take is lost —, which the log tells without their payload.
    /// The ones that waited go once the server reads on.
    #[tokio::test]
    async fn a_full_outbox_drops_rather_than_holds_up_a_send() {
        const MARK: &[u8] = b"payload-3f81c2";
        let (log, _logging, _second) = Log::capture();
        // takes the packets and answers none
        let quiet = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let to = target(quiet.local_addr().unwrap());
        let fake = FakeTuic::spawn(TuicScript {
            no_datagrams: true,
            stream_window: Some(8),
            ..script()
        });
        let out = user(&fake);
        let carrier = udp_carrier(&out).await;
        // each its own length, which tells them apart on the server
        let payload = |len: usize| -> Vec<u8> { MARK.iter().copied().cycle().take(len).collect() };
        // the authentication's own stream is not held
        eventually("the authentication", || fake.auths().len() == 1).await;
        fake.hold_streams();
        let first = payload(100);
        timeout(BOUND, carrier.send_to(&first, &to))
            .await
            .expect("the first packet handed over")
            .unwrap();
        eventually("its stream held", || fake.held_streams() == 1).await;
        for len in 1..=udp::OUTBOX + 10 {
            let packet = payload(len);
            let sent = timeout(Duration::from_millis(100), carrier.send_to(&packet, &to)).await;
            sent.expect("a send that does not wait").unwrap();
        }
        fake.release_streams();
        let mut expected: Vec<usize> = (1..=udp::OUTBOX).chain([100]).collect();
        eventually("the packets that waited", || {
            fake.udp_packets().len() == expected.len()
        })
        .await;
        // none of the others after them: a fixed wait, only to see nothing
        // happen
        tokio::time::sleep(Duration::from_millis(300)).await;
        let mut lens: Vec<usize> = fake.udp_packets().iter().map(|p| p.len).collect();
        lens.sort_unstable();
        expected.sort_unstable();
        assert_eq!(lens, expected);
        let log = log.text();
        let dropped = "tuic: a packet for the server is dropped: too many wait for a stream";
        assert_eq!(log.matches(dropped).count(), 10, "{log}");
        assert!(!log.contains("payload-3f81c2"));
        assert_eq!(fake.connections(), 1);
    }

    /// Carriers that open together share one connection and one
    /// authentication, each an association of its own, with a socket of its
    /// own on the server.
    #[tokio::test]
    async fn carriers_that_come_together_share_one_connection_each_its_own_association() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = Arc::new(user(&fake));
        let mut opening = JoinSet::new();
        for i in 0..16u8 {
            let out = out.clone();
            opening.spawn(async move {
                let carrier = udp_carrier(&out).await;
                udp_round_trip(carrier.as_ref(), echo, &[i; 100]).await;
                carrier
            });
        }
        let mut carriers = Vec::new();
        while let Some(carrier) = opening.join_next().await {
            carriers.push(carrier.unwrap());
        }
        assert_eq!(fake.connections(), 1);
        assert_eq!(fake.auths().len(), 1);
        let associations: BTreeSet<u16> = carried(&fake).iter().map(|&(_, a)| a).collect();
        assert_eq!(associations, (0..16).collect());
        assert_eq!(fake.udp_outside().len(), 16);
    }

    /// A packet longer than UDP carries, or one whose target cannot be
    /// named, is refused — `InvalidInput`, which loses that packet alone —,
    /// and nothing goes out, in datagrams and on streams alike; the carrier
    /// goes on.
    #[tokio::test]
    async fn a_packet_too_long_is_refused_and_nothing_goes_out() {
        let echo = udp_echo_server().await;
        for no_datagrams in [false, true] {
            let fake = FakeTuic::spawn(TuicScript {
                no_datagrams,
                ..script()
            });
            let out = user(&fake);
            let carrier = udp_carrier(&out).await;
            let err = carrier
                .send_to(&vec![0u8; 65_536], &target(echo))
                .await
                .unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(err.to_string(), "tuic: a datagram longer than 65535 bytes");
            let long = Target::new(HostName::Domain("a".repeat(256)), 53);
            let err = carrier.send_to(b"q", &long).await.unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert!(fake.udp_packets().is_empty());
            udp_round_trip(carrier.as_ref(), echo, b"still").await;
        }
    }

    /// `opts.timeout` bounds opening a carrier: the dial and QUIC's
    /// handshake.
    #[tokio::test]
    async fn opening_a_carrier_is_bounded_by_its_time() {
        // nothing speaks QUIC on this port
        let quiet = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = quiet.local_addr().unwrap().port();
        let fake = FakeTuic::spawn(script());
        let out = outbound_with(&spec(port, &credentials(ID, PASSWORD)), port, &fake);
        let opts = ConnectOpts {
            timeout: Duration::from_millis(500),
        };
        let started = Instant::now();
        let err = out.open_udp(&opts).await.err().unwrap();
        assert!(matches!(err, OutboundError::Timeout), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    /// What packets carry shows nowhere — in no error and in no log line,
    /// not even at the finest level — whether they go through, are refused,
    /// are no packet, find no association, or go out again after a close.
    #[tokio::test]
    async fn payloads_never_show_in_errors_or_logs() {
        const MARK: &[u8] = b"payload-7c1e9a";
        let (log, _logging, _second) = Log::capture();
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, MARK).await;
        let err = carrier
            .send_to(&MARK.repeat(5000), &target(echo))
            .await
            .unwrap_err();
        // no packet (its SIZE says otherwise), then one for no association:
        // read before the echo that follows them
        let mut junk = TuicPacket::whole(0, 9, "192.0.2.1:53", MARK).encode();
        junk.push(0);
        fake.connection(0).send_datagram(junk.into()).unwrap();
        fake.send_udp(0, &TuicPacket::whole(9, 1, "192.0.2.1:53", MARK));
        udp_round_trip(carrier.as_ref(), echo, MARK).await;
        fake.close_connections();
        until_answered(carrier.as_ref(), echo, MARK).await;
        let text = format!("{err} {err:?}");
        assert!(!text.contains("payload-7c1e9a"), "{text}");
        let log = log.text();
        assert!(
            log.contains("tuic: what the server sent is no packet; dropped"),
            "{log}"
        );
        assert!(
            log.contains("tuic: UDP association 0 moves to a new connection, as association 0"),
            "{log}"
        );
        assert!(!log.contains("payload-7c1e9a"));
    }
}

```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
        let fresh = Authenticated::default();
        let relayed = Authenticated::default();
        relayed.mark_relayed();
        for (state, error, kind, text) in [
            (
                &fresh,
```

换成

```rust
        for (relayed, error, kind, text) in [
            (
                false,
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
                &relayed,
```

换成

```rust
                true,
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
            (
                &fresh,
                closed(0, b"bye"),
```

换成

```rust
            (
                false,
                closed(0, b"bye"),
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
            (
                &fresh,
                closed(0x2a, b"go\r\naway"),
```

换成

```rust
            (
                false,
                closed(0x2a, b"go\r\naway"),
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
            (
                &fresh,
                ConnectionError::Reset,
```

换成

```rust
            (
                false,
                ConnectionError::Reset,
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
            (
                &fresh,
                ConnectionError::TimedOut,
```

换成

```rust
            (
                false,
                ConnectionError::TimedOut,
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
            let ended = state.ended(&error);
            assert_eq!((ended.kind(), ended.to_string().as_str()), (kind, text));
```

换成

```rust
            let end = ended(&error, relayed);
            assert_eq!((end.kind(), end.to_string().as_str()), (kind, text));
            assert_eq!(refused(&error, relayed), kind == PermissionDenied);
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
            fresh
                .read_error(ReadError::ConnectionLost(closed(0, b"")))
                .to_string(),
```

换成

```rust
            read_error(ReadError::ConnectionLost(closed(0, b"")), false).to_string(),
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
            relayed
                .write_error(WriteError::ConnectionLost(closed(0, b"")))
                .to_string(),
```

换成

```rust
            write_error(WriteError::ConnectionLost(closed(0, b"")), true).to_string(),
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
            fresh
                .read_error(ReadError::Reset(VarInt::from_u32(3)))
                .to_string(),
```

换成

```rust
            read_error(ReadError::Reset(VarInt::from_u32(3)), false).to_string(),
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
            fresh
                .write_error(WriteError::Stopped(VarInt::from_u32(0)))
                .to_string(),
```

换成

```rust
            write_error(WriteError::Stopped(VarInt::from_u32(0)), false).to_string(),
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
            fresh.ended(&long).to_string(),
```

换成

```rust
            ended(&long, false).to_string(),
```

`crates/rurge-proto-quic/src/tuic/addr.rs`——把

```rust
            "tuic: the host name is longer than 255 bytes"
        );
    }
}

```

换成

```rust
            "tuic: the host name is longer than 255 bytes"
        );
    }

    /// What `put` writes reads back, and so does none; whatever follows is
    /// not the address's.
    #[test]
    fn an_address_reads_back_with_its_length() {
        for host in [
            "10.1.2.3",
            "2001:db8::1",
            "dns.example",
            "xn--bcher-kva.example",
        ] {
            let mut data = address(&at(host)).unwrap();
            let len = data.len();
            data.extend_from_slice(b"payload");
            assert_eq!(get(&data), Some((Some(at(host)), len)), "{host}");
        }
        assert_eq!(get(&[NONE, 0x1f, 0x90]), Some((None, 1)));
    }

    #[test]
    fn an_address_that_is_none_a_server_writes_reads_as_nothing() {
        for bad in [
            &[][..],
            // a type TUIC does not know
            &[0x03, 1, 2, 3, 4, 0, 53][..],
            // cut short: in the address, in the port
            &[0x01, 1, 2, 3][..],
            &[0x01, 1, 2, 3, 4, 0][..],
            &[0x02, 0x20, 0x01][..],
            &[0x00, 5, b'a', b'b'][..],
            &[0x00, 1, b'a', 0][..],
            // a name that is no host name: empty, a blank, not UTF-8
            &[0x00, 0, 0, 53][..],
            &[0x00, 3, b'a', b' ', b'b', 0, 53][..],
            &[0x00, 2, 0xff, 0xfe, 0, 53][..],
        ] {
            assert_eq!(get(bad), None, "{bad:?}");
        }
    }
}

```

UDP 与心跳两个模块，以及它们要的前置改动（认证状态里的读写任务与"服务端转发过"的标记、TCP 隧道计入"有转发"、地址的 `ff`）；`udp()` / `open_udp` 的接线在 Step 3：

新建 `crates/rurge-proto-quic/src/tuic/udp.rs`：

```rust
//! UDP over TUIC v5 (SPEC, "Packet", "Dissociate"; phase 2 M7 design 6.2):
//! each carrier — one client association on the outbound — is an
//! association of its own on the policy's connection, full cone: every
//! target through it, and answers from anyone, as the server keeps a UDP
//! socket per association.
//!
//! - A packet is `05 02 ‖ ASSOC_ID ‖ PKT_ID ‖ FRAG_TOTAL ‖ FRAG_ID ‖ SIZE ‖
//!   ADDR ‖ payload`, big endian: SIZE is the payload of this message, ADDR
//!   (`addr`) names the target going out and the source coming back.
//! - Where the server takes QUIC datagrams (`native`), a packet goes in
//!   datagrams of the connection: whole when it fits one, else in
//!   fragments, only the first naming the address — the others name none
//!   —, all under the packet's id. A carrier sends one packet at a time:
//!   Clash.Meta's first TUIC v5 server put one packet of an association
//!   together at a time, as Hysteria's does, and lost both of two whose
//!   fragments came in turn.
//! - Where it takes none (`quic`), each packet goes whole on a
//!   unidirectional stream of its own, which the connection's writer opens,
//!   writes and finishes, one after the other: a send hands its packet over
//!   and waits for nothing — dropped when `OUTBOX` wait already —, so that
//!   a send given up on never leaves a packet cut short on the wire, a
//!   stream sing-box answers by closing the connection, every tunnel and
//!   association on it with it. The server answers an association the way
//!   its first packet came (SPEC); answers are read either way.
//! - Association ids count per connection from 0, past the ones in use. A
//!   carrier moves to the session's connection on its next packet once its
//!   own takes nothing new — closed, or dialled another way than the policy
//!   leads now (`Session::serves`: where the policy leads is asked at most
//!   once a second, the rest on every packet) —, under an id that
//!   connection gives it: the server takes it for a new association
//!   whatever its id.
//! - A carrier that goes, or moves, dissociates its association on the
//!   connection it leaves while that is open: `05 03 ‖ ASSOC_ID` on a
//!   stream of its own, which the connection's task sends — the drop waits
//!   for nothing.
//! - That task reads the server's packets — its datagrams, and its streams
//!   — into their associations' inboxes, putting fragments together first:
//!   at most 8 packets of an association at a time, a ninth dropping the
//!   oldest. What finds no association, or a full inbox, is dropped.

use super::VERSION;
use super::addr::{self, NONE};
use super::auth::{self, Auth, Authenticated};
use super::heartbeat::{HEARTBEAT, Relaying};
use crate::session::{Lease, Session};
use bytes::Bytes;
use quinn::{Connection, RecvStream};
use rurge_net::BoxFuture;
use rurge_net::connector::{ConnectOpts, PacketSocket, Target};
use rurge_proto::OutboundError;
use std::collections::{HashMap, VecDeque, hash_map};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::task::JoinSet;
use tokio::time::Instant;
use tokio_util::task::AbortOnDropHandle;

/// Packets waiting for the engine, per carrier; more are dropped, as a
/// socket would.
pub(super) const INBOX: usize = 64;
/// Packets waiting for the connection's writer (`quic` mode); more are
/// dropped, as a datagram the connection cannot take is.
pub(super) const OUTBOX: usize = 64;
/// How often a carrier asks where the policy leads (`Session::serves`): it
/// can cost a group's resolution, and every packet would ask.
const ROUTE_EVERY: Duration = Duration::from_secs(1);
/// The most fragments a packet goes in: their count is one byte.
const MAX_FRAGMENTS: usize = 255;
/// The longest UDP datagram: what a packet's SIZE allows, and the longest
/// packet put together.
const MAX_DATAGRAM: usize = 65_535;
/// An association's packets put together at a time (design 6.2).
const UNDER_WAY: usize = 8;
/// `VER ‖ TYPE ‖ ASSOC_ID ‖ PKT_ID ‖ FRAG_TOTAL ‖ FRAG_ID ‖ SIZE`.
const HEADER: usize = 10;
/// The longest packet a stream carries: the header, the longest address
/// (a name of 255 bytes), the longest payload.
const MAX_MESSAGE: usize = HEADER + 1 + 1 + 255 + 2 + MAX_DATAGRAM;

// the commands' types
const PACKET: u8 = 0x02;
const DISSOCIATE: u8 = 0x03;

/// A packet for the engine, and where it came from.
type Answer = (Bytes, Target);

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("tuic: {what}"))
}

/// One Packet command: `05 02 ‖ ASSOC_ID ‖ PKT_ID ‖ FRAG_TOTAL ‖ FRAG_ID ‖
/// SIZE ‖ ADDR ‖ payload`, `address` the ADDR as `addr::put` writes it.
/// `payload` is at most 65535 bytes.
fn encode(
    association: u16,
    packet: u16,
    fragments: u8,
    fragment: u8,
    address: &[u8],
    payload: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + address.len() + payload.len());
    out.extend_from_slice(&[VERSION, PACKET]);
    out.extend_from_slice(&association.to_be_bytes());
    out.extend_from_slice(&packet.to_be_bytes());
    out.extend_from_slice(&[fragments, fragment]);
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    out.extend_from_slice(address);
    out.extend_from_slice(payload);
    out
}

/// The QUIC datagrams, none longer than `max`, that carry `payload` to
/// `address` as packet `packet` of association `association`: one when it
/// fits — fragment 0 of 1 —, else fragments, only the first naming the
/// address. Refused when 255 fragments cannot carry it, or when the first
/// leaves no room for any of it.
fn datagrams(
    association: u16,
    packet: u16,
    address: &[u8],
    payload: &[u8],
    max: usize,
) -> io::Result<Vec<Vec<u8>>> {
    if HEADER + address.len() + payload.len() <= max {
        return Ok(vec![encode(association, packet, 1, 0, address, payload)]);
    }
    let first = max.saturating_sub(HEADER + address.len());
    let rest = max.saturating_sub(HEADER + 1);
    if first == 0 || rest == 0 {
        return Err(invalid("the host name is too long for a datagram"));
    }
    let longest = MAX_DATAGRAM.min(first + (MAX_FRAGMENTS - 1) * rest);
    if payload.len() > longest {
        return Err(invalid(&format!("a datagram longer than {longest} bytes")));
    }
    // longer than `first`: it did not fit whole
    let (head, tail) = payload.split_at(first);
    let chunks: Vec<&[u8]> = std::iter::once(head).chain(tail.chunks(rest)).collect();
    let fragments = chunks.len() as u8;
    Ok(chunks
        .into_iter()
        .enumerate()
        .map(|(fragment, chunk)| {
            let address = if fragment == 0 { address } else { &[NONE][..] };
            encode(
                association,
                packet,
                fragments,
                fragment as u8,
                address,
                chunk,
            )
        })
        .collect())
}

/// A Packet as it came from the server, its SIZE exactly what follows the
/// address (as sing-box reads one).
struct Message {
    association: u16,
    packet: u16,
    fragments: u8,
    fragment: u8,
    /// The address: the source, on the first fragment.
    source: Option<Target>,
    payload: Bytes,
}

impl Message {
    /// `None` for anything but a Packet a server writes.
    fn parse(data: Bytes) -> Option<Message> {
        let header: [u8; HEADER] = data.get(..HEADER)?.try_into().ok()?;
        if header[..2] != [VERSION, PACKET] {
            return None;
        }
        let (source, len) = addr::get(&data[HEADER..])?;
        let start = HEADER + len;
        if data.len() - start != usize::from(u16::from_be_bytes([header[8], header[9]])) {
            return None;
        }
        Some(Message {
            association: u16::from_be_bytes([header[2], header[3]]),
            packet: u16::from_be_bytes([header[4], header[5]]),
            fragments: header[6],
            fragment: header[7],
            source,
            payload: data.slice(start..),
        })
    }
}

/// An association's packets being put together from their fragments
/// (design 6.2): at most `UNDER_WAY` at a time, a ninth dropping the one
/// whose first fragment came first; each whole once every fragment came,
/// never when one is lost. A fragment seen already is dropped; one whose
/// count differs from its packet's begins the packet anew (as sing-box).
#[derive(Default)]
struct Reassembly(VecDeque<Partial>);

struct Partial {
    packet: u16,
    /// By fragment: each its own bytes, not a slice of what QUIC received,
    /// which would stay with it.
    parts: Vec<Option<Vec<u8>>>,
    missing: usize,
    size: usize,
    /// The first fragment's address.
    source: Option<Target>,
}

impl Reassembly {
    /// The packet `message` makes whole, and its source: the message's own
    /// payload when it is no fragment; `None` while fragments are missing,
    /// and for a packet that names no source.
    fn feed(&mut self, message: Message) -> Option<Answer> {
        let count = usize::from(message.fragments);
        // a count of 0 too, as sing-box reads it
        if count <= 1 {
            return Some((message.payload, message.source?));
        }
        let fragment = usize::from(message.fragment);
        if fragment >= count {
            return None;
        }
        let at = match self.0.iter().position(|p| p.packet == message.packet) {
            Some(at) if self.0[at].parts.len() == count => at,
            known => {
                if let Some(at) = known {
                    self.0.remove(at);
                }
                if self.0.len() == UNDER_WAY {
                    self.0.pop_front();
                }
                self.0.push_back(Partial {
                    packet: message.packet,
                    parts: vec![None; count],
                    missing: count,
                    size: 0,
                    source: None,
                });
                self.0.len() - 1
            }
        };
        let partial = &mut self.0[at];
        if partial.parts[fragment].is_some() {
            return None;
        }
        partial.size += message.payload.len();
        if partial.size > MAX_DATAGRAM {
            self.0.remove(at);
            return None;
        }
        if fragment == 0 {
            partial.source = message.source;
        }
        partial.parts[fragment] = Some(message.payload.to_vec());
        partial.missing -= 1;
        if partial.missing > 0 {
            return None;
        }
        let partial = self.0.remove(at)?;
        let mut whole = Vec::with_capacity(partial.size);
        for part in partial.parts.into_iter().flatten() {
            whole.extend_from_slice(&part);
        }
        Some((whole.into(), partial.source?))
    }
}

/// A carrier's association on a connection.
struct Association {
    inbox: mpsc::Sender<Answer>,
    reassembly: Reassembly,
}

/// A connection's associations, by id, and where the search for a free id
/// goes on.
#[derive(Default)]
struct Associations {
    by_id: HashMap<u16, Association>,
    next: u16,
}

impl Associations {
    /// Takes `inbox` in under the next id free, and says which: `None` when
    /// all 65536 are in use.
    fn join(&mut self, inbox: &mpsc::Sender<Answer>) -> Option<u16> {
        for _ in 0..=u16::MAX {
            let id = self.next;
            self.next = id.wrapping_add(1);
            if let hash_map::Entry::Vacant(free) = self.by_id.entry(id) {
                free.insert(Association {
                    inbox: inbox.clone(),
                    reassembly: Reassembly::default(),
                });
                return Some(id);
            }
        }
        None
    }
}

/// A connection's UDP: its associations, the task that reads the server's
/// packets into them and sends the Dissociates asked for, and the writer
/// that sends the packets that go on streams (`quic` mode). The tasks end
/// with the connection, or when this is dropped.
pub(super) struct Udp {
    associations: Arc<Mutex<Associations>>,
    dissociate: mpsc::UnboundedSender<u16>,
    /// The writer's queue (`queue`).
    outbox: mpsc::Sender<Vec<u8>>,
    _reader: AbortOnDropHandle<()>,
    _writer: AbortOnDropHandle<()>,
}

impl Udp {
    /// The reader marks `relayed` for every packet the server relays.
    pub(super) fn start(connection: &Connection, relayed: Arc<AtomicBool>) -> Udp {
        let associations = Arc::<Mutex<Associations>>::default();
        let (dissociate, dissociations) = mpsc::unbounded_channel();
        let reader = tokio::spawn(read(
            connection.clone(),
            associations.clone(),
            relayed,
            dissociations,
        ));
        let (outbox, packets) = mpsc::channel(OUTBOX);
        let writer = tokio::spawn(write(connection.clone(), packets));
        Udp {
            associations,
            dissociate,
            outbox,
            _reader: AbortOnDropHandle::new(reader),
            _writer: AbortOnDropHandle::new(writer),
        }
    }

    /// Takes `inbox` in under the next id free on the connection
    /// (`Associations::join`).
    fn join(&self, inbox: &mpsc::Sender<Answer>) -> Option<u16> {
        self.associations.lock().expect("associations").join(inbox)
    }

    /// Forgets association `id`: what comes for it from now on is dropped.
    /// With `dissociate`, the server is told so, by the connection's task.
    fn leave(&self, id: u16, dissociate: bool) {
        self.associations
            .lock()
            .expect("associations")
            .by_id
            .remove(&id);
        if dissociate {
            // the task is gone with its connection: nothing to tell
            let _ = self.dissociate.send(id);
        }
    }

    /// Hands `message`, a whole packet, to the connection's writer
    /// (`write`), never waiting: dropped when `OUTBOX` wait already, as a
    /// full send buffer drops a datagram, or when the connection ended just
    /// now — the next send finds it closed and binds anew.
    fn queue(&self, message: Vec<u8>) {
        if let Err(e) = self.outbox.try_send(message) {
            let why = match e {
                TrySendError::Full(_) => "too many wait for a stream",
                TrySendError::Closed(_) => "the connection is closed",
            };
            tracing::debug!("tuic: a packet for the server is dropped: {why}");
        }
    }

    /// Test support: the ids of the associations, in order.
    #[cfg(test)]
    pub(super) fn associations(&self) -> Vec<u16> {
        let associations = self.associations.lock().expect("associations");
        let mut ids: Vec<u16> = associations.by_id.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// Test support: whether the task still reads.
    #[cfg(test)]
    pub(super) fn is_reading(&self) -> bool {
        !self._reader.is_finished()
    }

    /// Test support: whether the writer still writes.
    #[cfg(test)]
    pub(super) fn is_writing(&self) -> bool {
        !self._writer.is_finished()
    }
}

/// Reads the server's packets on `connection` — its datagrams, and its
/// unidirectional streams, each read by a task of its own — into their
/// associations' inboxes, and sends each Dissociate asked for on a stream
/// of its own, until the connection ends: what came before its end
/// included.
async fn read(
    connection: Connection,
    associations: Arc<Mutex<Associations>>,
    relayed: Arc<AtomicBool>,
    mut dissociations: mpsc::UnboundedReceiver<u16>,
) {
    let mut streams = JoinSet::new();
    let mut dissociating = JoinSet::new();
    loop {
        tokio::select! {
            datagram = connection.read_datagram() => match datagram {
                Ok(datagram) => deliver(&associations, &relayed, datagram),
                Err(_) => return,
            },
            stream = connection.accept_uni() => match stream {
                Ok(stream) => {
                    streams.spawn(read_stream(stream));
                }
                Err(_) => return,
            },
            Some(done) = streams.join_next() => {
                if let Ok(Some(message)) = done {
                    deliver(&associations, &relayed, message);
                }
            }
            Some(id) = dissociations.recv() => {
                dissociating.spawn(dissociate(connection.clone(), id));
            }
            // a Dissociate gone out; nothing to collect
            Some(_) = dissociating.join_next() => {}
        }
    }
}

/// A packet on a unidirectional stream of the server's (`quic` mode), read
/// to the stream's end, which follows it (sing-box and tuic-server finish
/// such a stream after one packet); `None` for a stream longer than the
/// longest packet, or cut short.
async fn read_stream(mut stream: RecvStream) -> Option<Bytes> {
    stream.read_to_end(MAX_MESSAGE).await.ok().map(Bytes::from)
}

/// `05 03 ‖ ASSOC_ID` on a unidirectional stream of its own, finished: the
/// server frees the association's socket.
async fn dissociate(connection: Connection, id: u16) {
    let [high, low] = id.to_be_bytes();
    let Ok(mut stream) = connection.open_uni().await else {
        return;
    };
    if stream
        .write_all(&[VERSION, DISSOCIATE, high, low])
        .await
        .is_ok()
    {
        // fails only for a stream finished or reset already
        let _ = stream.finish();
    }
}

/// Hands what the server sent — a datagram, or a stream's packet — to its
/// association's inbox, once the packet it carries is whole. A heartbeat
/// (sing-box sends one every 10 seconds from the connection's start) says
/// nothing. A packet tells the authentication went through: servers relay
/// none before (`relayed`). Dropped when it is no packet, for no
/// association of the connection (a carrier gone), or meets a full inbox:
/// the reader waits for no carrier, so that one that reads slowly holds up
/// no other.
fn deliver(associations: &Mutex<Associations>, relayed: &AtomicBool, data: Bytes) {
    if data.starts_with(&[VERSION, HEARTBEAT]) {
        return;
    }
    let Some(message) = Message::parse(data) else {
        tracing::trace!("tuic: what the server sent is no packet; dropped");
        return;
    };
    auth::mark(relayed);
    let mut associations = associations.lock().expect("associations");
    let Some(association) = associations.by_id.get_mut(&message.association) else {
        return;
    };
    if let Some(answer) = association.reassembly.feed(message) {
        let _ = association.inbox.try_send(answer);
    }
}

/// Where a carrier is: its association on one connection, which its lease
/// keeps, counted among the connection's relays. Dropped, the connection
/// forgets the association, and tells the server when it is still open.
struct Bound {
    id: u16,
    _relaying: Relaying,
    lease: Lease<Authenticated>,
}

impl Bound {
    /// Whether it is on `lease`'s connection.
    fn is_on(&self, lease: &Lease<Authenticated>) -> bool {
        // both leases keep their connections: no two have the same id
        self.lease.connection().stable_id() == lease.connection().stable_id()
    }
}

impl Drop for Bound {
    fn drop(&mut self) {
        let open = self.lease.connection().close_reason().is_none();
        self.lease.state().udp.leave(self.id, open);
    }
}

/// One client association's UDP through the outbound (design 6.2): an
/// association on the policy's connection, then on whichever connection
/// replaces it. No `Debug`: its session holds the credentials.
pub(super) struct TuicUdp {
    session: Arc<Session<Auth>>,
    /// The budget of binding to a new connection, its dial included.
    opts: ConnectOpts,
    bound: Mutex<Option<Arc<Bound>>>,
    /// The id of the carrier's next packet.
    packets: AtomicU16,
    /// Held while a packet's fragments go out: concurrent sends of the
    /// carrier take turns.
    sending: Mutex<()>,
    /// When the policy's route was last compared (`route_due`).
    route_checked: Mutex<Option<Instant>>,
    /// What the connections it binds to fill `inbox` through.
    answers: mpsc::Sender<Answer>,
    inbox: tokio::sync::Mutex<mpsc::Receiver<Answer>>,
}

impl TuicUdp {
    /// A carrier on the session's connection, dialed when there is none.
    /// Nothing bounds the time it takes here.
    pub(super) async fn open(
        session: Arc<Session<Auth>>,
        opts: &ConnectOpts,
    ) -> Result<TuicUdp, OutboundError> {
        let (answers, inbox) = mpsc::channel(INBOX);
        let udp = TuicUdp {
            session,
            opts: opts.clone(),
            bound: Mutex::default(),
            packets: AtomicU16::new(0),
            sending: Mutex::default(),
            route_checked: Mutex::default(),
            answers,
            inbox: tokio::sync::Mutex::new(inbox),
        };
        udp.bind().await?;
        Ok(udp)
    }

    /// Where the carrier is, while that connection takes what is new
    /// (`Session::serves`): that it is open is looked at on every call,
    /// where the policy leads at most once a second. A connection the
    /// server closed as it closes one whose authentication it refused fails
    /// the send that finds it so — the carrier then is bound nowhere, and
    /// the next send binds anew, dialing.
    fn current(&self) -> io::Result<Option<Arc<Bound>>> {
        let Some(bound) = self.bound.lock().expect("bound").clone() else {
            return Ok(None);
        };
        let serves = if self.route_due() {
            self.session.serves(&bound.lease)
        } else {
            self.session.serves_without_route(&bound.lease)
        };
        if serves {
            return Ok(Some(bound));
        }
        let state = bound.lease.state();
        match bound.lease.connection().close_reason() {
            Some(end) if state.refused(&end) => {
                self.unbind(&bound);
                Err(state.ended(&end))
            }
            _ => Ok(None),
        }
    }

    /// Whether the policy's route is to be compared now: it is when it was
    /// not within the last second, or at all, and this then counts as the
    /// comparison.
    fn route_due(&self) -> bool {
        let mut checked = self.route_checked.lock().expect("route checked");
        let now = Instant::now();
        if checked.is_some_and(|at| now.duration_since(at) < ROUTE_EVERY) {
            return false;
        }
        *checked = Some(now);
        true
    }

    /// Binds the carrier to the session's connection — a lease on it, the
    /// connection dialed when there is none — under an association id that
    /// connection gives it.
    async fn bind(&self) -> Result<Arc<Bound>, OutboundError> {
        let lease = self.session.lease(&self.opts).await?;
        let mut bound = self.bound.lock().expect("bound");
        // a send bound it meanwhile, to this very connection
        if let Some(current) = bound.as_ref().filter(|bound| bound.is_on(&lease)) {
            return Ok(current.clone());
        }
        let Some(id) = lease.state().udp.join(&self.answers) else {
            return Err(OutboundError::Proxy(
                "tuic: no association id is free on the connection".to_string(),
            ));
        };
        if let Some(old) = bound.as_ref() {
            tracing::debug!(
                "tuic: UDP association {} moves to a new connection, as association {id}",
                old.id
            );
        }
        let new = Arc::new(Bound {
            id,
            _relaying: lease.state().relaying(),
            lease,
        });
        let old = bound.replace(new.clone());
        drop(bound);
        // the connection before forgets the association, out of the lock
        drop(old);
        Ok(new)
    }

    /// `bind` for a send: within the carrier's budget, failing as a send
    /// fails.
    async fn rebind(&self) -> io::Result<Arc<Bound>> {
        match tokio::time::timeout(self.opts.timeout, self.bind()).await {
            Ok(Ok(bound)) => Ok(bound),
            Ok(Err(e)) => Err(io::Error::other(e.to_string())),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                OutboundError::Timeout.to_string(),
            )),
        }
    }

    /// Lets go of `bound`, unless the carrier is bound elsewhere already.
    /// The caller's hold on it keeps it past the lock.
    fn unbind(&self, bound: &Arc<Bound>) {
        let mut current = self.bound.lock().expect("bound");
        if current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, bound))
        {
            *current = None;
        }
    }
}

/// Hands `datagrams` to QUIC in order, never waiting: a full send buffer
/// makes room by dropping the oldest datagrams in it. A path that has
/// narrowed since their size was taken drops the packet they carry, as any
/// hop may, and so does a connection that closed just now — the next send
/// finds it closed and binds anew.
fn transmit(connection: &Connection, datagrams: Vec<Vec<u8>>) {
    for datagram in datagrams {
        if let Err(e) = connection.send_datagram(datagram.into()) {
            // the rest is no use without this one
            tracing::debug!("tuic: a datagram for the server is dropped: {e}");
            return;
        }
    }
}

/// The connection's writer (`quic` mode): each packet handed to it goes on
/// a stream of its own, one after the other, until the connection ends. No
/// send waits for it, so no send given up on cuts a packet short: a stream
/// that ends early is one sing-box cannot read, and it closes the
/// connection for that.
async fn write(connection: Connection, mut packets: mpsc::Receiver<Vec<u8>>) {
    loop {
        let message = tokio::select! {
            biased;
            _ = connection.closed() => return,
            message = packets.recv() => message,
        };
        // the connection's state, and the queue with it, is gone
        let Some(message) = message else {
            return;
        };
        send_on_stream(&connection, &message).await;
    }
}

/// `message` on a unidirectional stream of its own, finished: the writer's
/// (`write`). It waits for QUIC to open the stream — the server's limit on
/// open streams — and to take the message, however long that takes. A
/// stream that fails loses its packet alone, as any hop may lose one: the
/// server stopped it, or the connection ended — the next send then finds it
/// closed and binds anew.
async fn send_on_stream(connection: &Connection, message: &[u8]) {
    let mut stream = match connection.open_uni().await {
        Ok(stream) => stream,
        Err(e) => {
            tracing::debug!("tuic: a packet for the server is dropped: {e}");
            return;
        }
    };
    match stream.write_all(message).await {
        Ok(()) => {
            // fails only for a stream finished or reset already
            let _ = stream.finish();
        }
        Err(e) => tracing::debug!("tuic: a packet for the server is dropped: {e}"),
    }
}

impl PacketSocket for TuicUdp {
    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            // checked first: nothing is dialed for what cannot be sent
            let mut address = Vec::new();
            addr::put(&mut address, to)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
            if buf.len() > MAX_DATAGRAM {
                return Err(invalid(&format!(
                    "a datagram longer than {MAX_DATAGRAM} bytes"
                )));
            }
            let bound = match self.current()? {
                Some(bound) => bound,
                // its connection takes nothing new: the session's, under
                // the id that one gives
                None => self.rebind().await?,
            };
            let connection = bound.lease.connection();
            let packet = self.packets.fetch_add(1, Ordering::Relaxed);
            match connection.max_datagram_size() {
                Some(max) => {
                    let out = datagrams(bound.id, packet, &address, buf, max)?;
                    // all of its fragments in a row, with nothing awaited
                    // meanwhile
                    let sending = self.sending.lock().expect("sending");
                    transmit(connection, out);
                    drop(sending);
                }
                None => {
                    let message = encode(bound.id, packet, 1, 0, &address, buf);
                    bound.lease.state().udp.queue(message);
                }
            }
            Ok(())
        })
    }

    /// A packet longer than `buf` is dropped: give it 64 KiB.
    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            let mut inbox = self.inbox.lock().await;
            loop {
                // `self` holds a sender: the inbox never closes
                let Some((packet, from)) = inbox.recv().await else {
                    return Err(io::ErrorKind::BrokenPipe.into());
                };
                if let Some(space) = buf.get_mut(..packet.len()) {
                    space.copy_from_slice(&packet);
                    return Ok((packet.len(), from));
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;

    /// `10.0.0.1:53` as ADDR.
    const ADDR: [u8; 7] = [0x01, 10, 0, 0, 1, 0, 53];

    fn source() -> Target {
        Target::new(HostName::parse("10.0.0.1"), 53)
    }

    fn parse(data: &[u8]) -> Option<Message> {
        Message::parse(Bytes::copy_from_slice(data))
    }

    /// `(association, packet, fragments, fragment)` of `message`.
    fn ids(message: &Message) -> (u16, u16, u8, u8) {
        (
            message.association,
            message.packet,
            message.fragments,
            message.fragment,
        )
    }

    /// TUIC v5 SPEC "Packet": the header, its fields big endian, SIZE the
    /// payload's length, then the address and the payload.
    #[test]
    fn a_packet_that_fits_goes_whole() {
        let mut expected = vec![0x05, 0x02, 0x12, 0x34, 0x56, 0x78, 1, 0, 0, 5];
        expected.extend_from_slice(&ADDR);
        expected.extend_from_slice(b"query");
        assert_eq!(
            datagrams(0x1234, 0x5678, &ADDR, b"query", 1200).unwrap(),
            [expected]
        );
        // exactly as long as the limit
        let payload = vec![7u8; 1200 - HEADER - ADDR.len()];
        let whole = datagrams(1, 1, &ADDR, &payload, 1200).unwrap();
        assert_eq!((whole.len(), whole[0].len()), (1, 1200));
        // nothing to carry is a packet too
        assert_eq!(datagrams(1, 1, &ADDR, b"", 1200).unwrap()[0].len(), 17);
    }

    /// Design 6.2: what does not fit goes in fragments under one packet
    /// id, the first naming the address and the others none (`0xff`), each
    /// as long as the limit allows.
    #[test]
    fn what_does_not_fit_goes_in_fragments_only_the_first_naming_the_address() {
        let payload: Vec<u8> = (0..3000).map(|i| (i % 251) as u8).collect();
        let fragments = datagrams(9, 77, &ADDR, &payload, 1000).unwrap();
        // 983 bytes in the first, 989 in each of the others
        assert_eq!(fragments.len(), 4);
        let mut whole = Vec::new();
        for (i, datagram) in fragments.iter().enumerate() {
            let message = parse(datagram).unwrap();
            assert_eq!(ids(&message), (9, 77, 4, i as u8));
            if i == 0 {
                assert_eq!(message.source, Some(source()));
            } else {
                assert_eq!(message.source, None);
                assert_eq!(datagram[HEADER], 0xff);
            }
            assert_eq!(datagram.len(), if i < 3 { 1000 } else { 50 });
            whole.extend_from_slice(&message.payload);
        }
        assert_eq!(whole, payload);
        // a byte over the limit: two fragments
        let over = vec![1u8; 1000 - HEADER - ADDR.len() + 1];
        assert_eq!(datagrams(1, 1, &ADDR, &over, 1000).unwrap().len(), 2);
    }

    #[test]
    fn a_packet_255_fragments_cannot_carry_is_refused() {
        // 200 - 17 bytes in the first, 200 - 11 in each of the others
        let longest = 183 + 254 * 189;
        let mut payload = vec![0u8; longest];
        assert_eq!(datagrams(1, 1, &ADDR, &payload, 200).unwrap().len(), 255);
        payload.push(0);
        let err = datagrams(1, 1, &ADDR, &payload, 200).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            format!("tuic: a datagram longer than {longest} bytes")
        );
        // a first fragment with no room past its address
        let err = datagrams(1, 1, &ADDR, b"x", HEADER + ADDR.len()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            "tuic: the host name is too long for a datagram"
        );
    }

    /// A packet is read as sing-box reads one: its SIZE exactly what
    /// follows the address.
    #[test]
    fn a_packet_is_read_as_sing_box_reads_it() {
        let mut data = vec![0x05, 0x02, 0, 7, 0x12, 0x34, 3, 1, 0, 7];
        data.extend_from_slice(&ADDR);
        data.extend_from_slice(b"payload");
        let message = parse(&data).unwrap();
        assert_eq!(ids(&message), (7, 0x1234, 3, 1));
        assert_eq!(message.source, Some(source()));
        assert_eq!(&message.payload[..], b"payload");
        // none for an address, no payload at all
        let message = parse(&[0x05, 0x02, 0, 1, 0, 2, 2, 1, 0, 0, 0xff]).unwrap();
        assert_eq!((message.source, message.payload.len()), (None, 0));
        let mut short = data.clone();
        short.pop();
        let mut long = data.clone();
        long.push(0);
        let mut v4 = data.clone();
        v4[0] = 0x04;
        let mut dissociate = data.clone();
        dissociate[1] = 0x03;
        for bad in [
            &data[..9],
            // SIZE says otherwise
            &short[..],
            &long[..],
            // another version, another command
            &v4[..],
            &dissociate[..],
            // an address cut short
            &data[..12],
        ] {
            assert!(parse(bad).is_none(), "{bad:?}");
        }
    }

    fn fragment(packet: u16, fragment: u8, fragments: u8, payload: &[u8]) -> Message {
        Message {
            association: 0,
            packet,
            fragments,
            fragment,
            source: (fragment == 0).then(source),
            payload: Bytes::copy_from_slice(payload),
        }
    }

    fn whole(payload: &[u8]) -> Option<Answer> {
        Some((Bytes::copy_from_slice(payload), source()))
    }

    #[test]
    fn fragments_make_their_packet_whole_in_any_order() {
        let mut reassembly = Reassembly::default();
        assert_eq!(reassembly.feed(fragment(5, 2, 3, b"ef")), None);
        assert_eq!(reassembly.feed(fragment(5, 0, 3, b"ab")), None);
        assert_eq!(reassembly.feed(fragment(5, 1, 3, b"cd")), whole(b"abcdef"));
        assert!(reassembly.0.is_empty());
    }

    /// Unlike Hysteria 2's, two packets of an association come together at
    /// once (as sing-box and tuic-server put them together), and a whole
    /// packet passes them both.
    #[test]
    fn packets_are_put_together_side_by_side() {
        let mut reassembly = Reassembly::default();
        assert_eq!(reassembly.feed(fragment(1, 0, 2, b"one-")), None);
        assert_eq!(reassembly.feed(fragment(2, 0, 2, b"two-")), None);
        assert_eq!(
            reassembly.feed(fragment(9, 0, 1, b"whole")),
            whole(b"whole")
        );
        // a count of 0 is a whole packet too
        assert_eq!(reassembly.feed(fragment(9, 0, 0, b"zero")), whole(b"zero"));
        assert_eq!(reassembly.feed(fragment(2, 1, 2, b"b")), whole(b"two-b"));
        assert_eq!(reassembly.feed(fragment(1, 1, 2, b"a")), whole(b"one-a"));
    }

    /// Design 6.2: at most 8 packets under way; the one whose first
    /// fragment came first is dropped for a ninth, and its last fragment,
    /// come late, makes nothing whole.
    #[test]
    fn a_ninth_packet_under_way_drops_the_oldest() {
        let mut reassembly = Reassembly::default();
        for packet in 1..=9 {
            assert_eq!(reassembly.feed(fragment(packet, 0, 2, b"x")), None);
        }
        assert_eq!(reassembly.0.len(), UNDER_WAY);
        for packet in 2..=9 {
            assert_eq!(reassembly.feed(fragment(packet, 1, 2, b"y")), whole(b"xy"));
        }
        assert_eq!(reassembly.feed(fragment(1, 1, 2, b"y")), None);
    }

    #[test]
    fn duplicates_and_fragments_past_the_count_are_dropped() {
        let mut reassembly = Reassembly::default();
        assert_eq!(reassembly.feed(fragment(4, 2, 2, b"x")), None);
        assert_eq!(reassembly.feed(fragment(4, 0, 2, b"a")), None);
        // the first one stays
        assert_eq!(reassembly.feed(fragment(4, 0, 2, b"A")), None);
        assert_eq!(reassembly.feed(fragment(4, 1, 2, b"b")), whole(b"ab"));
        // a fragment counting otherwise begins its packet anew
        assert_eq!(reassembly.feed(fragment(6, 0, 3, b"a")), None);
        assert_eq!(reassembly.feed(fragment(6, 1, 2, b"b")), None);
        assert_eq!(reassembly.feed(fragment(6, 0, 2, b"c")), whole(b"cb"));
    }

    /// The source is the first fragment's: a packet without it, or a whole
    /// one that names none, goes nowhere.
    #[test]
    fn a_packet_that_names_no_source_is_dropped() {
        let mut reassembly = Reassembly::default();
        let mut first = fragment(3, 0, 2, b"a");
        first.source = None;
        assert_eq!(reassembly.feed(first), None);
        assert_eq!(reassembly.feed(fragment(3, 1, 2, b"b")), None);
        let mut lone = fragment(4, 0, 1, b"lone");
        lone.source = None;
        assert_eq!(reassembly.feed(lone), None);
    }

    #[test]
    fn a_packet_longer_than_udp_allows_is_dropped() {
        let mut reassembly = Reassembly::default();
        let half = vec![0u8; 32_768];
        assert_eq!(reassembly.feed(fragment(6, 0, 3, &half)), None);
        assert_eq!(reassembly.feed(fragment(6, 1, 3, &half)), None);
        assert!(reassembly.0.is_empty());
        // the longest there is goes through
        assert_eq!(reassembly.feed(fragment(7, 0, 2, &vec![0; 65_000])), None);
        let (packet, _) = reassembly.feed(fragment(7, 1, 2, &[0; 535])).unwrap();
        assert_eq!(packet.len(), MAX_DATAGRAM);
    }

    /// Design 6.2: ids count per connection from 0, past the ones in use;
    /// one let go of is given again once the count comes round, and none
    /// is left once all 65536 are in use.
    #[test]
    fn association_ids_count_from_0_past_the_ones_in_use() {
        let mut associations = Associations::default();
        let (inbox, _rx) = mpsc::channel(1);
        assert_eq!(
            (associations.join(&inbox), associations.join(&inbox)),
            (Some(0), Some(1))
        );
        associations.next = u16::MAX;
        assert_eq!(associations.join(&inbox), Some(u16::MAX));
        // round again: 0 and 1 are in use
        assert_eq!(associations.join(&inbox), Some(2));
        associations.by_id.remove(&0);
        associations.next = u16::MAX;
        assert_eq!(associations.join(&inbox), Some(0));
        while associations.by_id.len() < 1 << 16 {
            associations.join(&inbox).unwrap();
        }
        assert_eq!(associations.join(&inbox), None);
    }
}
```

新建 `crates/rurge-proto-quic/src/tuic/heartbeat.rs`：

```rust
//! The heartbeat (TUIC v5 SPEC, "Heartbeat"; phase 2 M7 design 6.2): while
//! anything is relayed on a connection — a TCP tunnel, a UDP association —
//! a `05 04` QUIC datagram every few seconds keeps the server from taking
//! the connection for idle, as tuic-client 1.0.0 keeps it: tuic-server's
//! idle timeout is 10 seconds, which QUIC's own keep-alive every 10 seconds
//! (design 3.2) does not beat.
//!
//! - Nothing goes while nothing is relayed: an idle connection is let go of
//!   — by the server, or by the session (`IDLE_TIMEOUT`).
//! - The SPEC sends it as a datagram only: a server that takes no QUIC
//!   datagrams gets none (sing-box ends a connection on a heartbeat in a
//!   stream: an unknown command there), and QUIC's keep-alive alone keeps
//!   the connection up — against a server whose idle timeout is longer
//!   than 10 seconds (sing-box's: quic-go's 30).

use super::VERSION;
use bytes::Bytes;
use quinn::{Connection, SendDatagramError};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio_util::task::AbortOnDropHandle;

/// How often, while something is relayed: tuic-client 1.0.0's.
pub(super) const EVERY: Duration = Duration::from_secs(3);

/// The command's type.
pub(super) const HEARTBEAT: u8 = 0x04;

/// The relays under way on a connection — its TCP tunnels and the UDP
/// associations on it —, which its heartbeat keeps it up for.
pub(super) struct Relays(watch::Sender<usize>);

impl Relays {
    pub(super) fn new() -> Arc<Relays> {
        Arc::new(Relays(watch::Sender::new(0)))
    }

    /// One more relay under way, until the guard goes. Only the first wakes
    /// the heartbeat.
    pub(super) fn begin(self: &Arc<Self>) -> Relaying {
        self.0.send_if_modified(|relays| {
            *relays += 1;
            *relays == 1
        });
        Relaying(self.clone())
    }

    /// Test support: how many are under way.
    #[cfg(test)]
    pub(super) fn count(&self) -> usize {
        *self.0.borrow()
    }
}

/// A relay under way on a connection, counted while it lives.
pub(super) struct Relaying(Arc<Relays>);

impl Drop for Relaying {
    /// The heartbeat finds out when it is next due: nothing wakes it.
    fn drop(&mut self) {
        self.0.0.send_if_modified(|relays| {
            *relays -= 1;
            false
        });
    }
}

/// Sends the heartbeat on `connection` every `every` while `relays` has
/// any under way, until the connection ends — or the handle, or `relays`,
/// goes.
pub(super) fn start(
    connection: &Connection,
    relays: &Relays,
    every: Duration,
) -> AbortOnDropHandle<()> {
    let connection = connection.clone();
    let relays = relays.0.subscribe();
    AbortOnDropHandle::new(tokio::spawn(async move {
        tokio::select! {
            _ = connection.closed() => {}
            () = beat(&connection, relays, every) => {}
        }
    }))
}

async fn beat(connection: &Connection, mut relays: watch::Receiver<usize>, every: Duration) {
    loop {
        // asleep until a relay begins
        if relays.wait_for(|&relays| relays > 0).await.is_err() {
            return;
        }
        tokio::time::sleep(every).await;
        if *relays.borrow() == 0 {
            continue;
        }
        let heartbeat = Bytes::from_static(&[VERSION, HEARTBEAT]);
        if let Err(SendDatagramError::ConnectionLost(_)) = connection.send_datagram(heartbeat) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relays_count_while_their_guards_live() {
        let relays = Relays::new();
        let (one, two) = (relays.begin(), relays.begin());
        assert_eq!(relays.count(), 2);
        drop(one);
        assert_eq!(relays.count(), 1);
        drop(two);
        assert_eq!(relays.count(), 0);
    }

    /// Only the first relay wakes the heartbeat; the end of one wakes
    /// nothing.
    #[test]
    fn only_the_first_relay_wakes_the_heartbeat() {
        let relays = Relays::new();
        let mut watching = relays.0.subscribe();
        let first = relays.begin();
        assert!(watching.has_changed().unwrap());
        watching.borrow_and_update();
        let second = relays.begin();
        drop(first);
        drop(second);
        assert!(!watching.has_changed().unwrap());
        let again = relays.begin();
        assert!(watching.has_changed().unwrap());
        drop(again);
    }
}
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
//!   up, and the connection's requests follow without waiting.
//! - A server that refuses it closes the connection — tuic-server and
//!   sing-box with code 0 and no reason —, which the connection's tunnels
//!   then tell (`Authenticated::ended`).

use super::VERSION;
```

换成

```rust
//!   up, and the connection's requests follow without waiting — UDP
//!   packets too, which sing-box leaves in QUIC's queue until then.
//! - A server that refuses it closes the connection — tuic-server and
//!   sing-box with code 0 and no reason —, which the connection's tunnels
//!   and carriers then tell (`Authenticated::ended`).

use super::VERSION;
use super::heartbeat::{self, Relaying, Relays};
use super::udp::Udp;
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
use std::sync::atomic::{AtomicBool, Ordering};
```

换成

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio_util::task::AbortOnDropHandle;
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
    password: String,
}

/// What a connection keeps once its authentication went out.
#[derive(Default)]
pub(crate) struct Authenticated {
    /// The server relayed something on the connection: it took the
    /// authentication.
    relayed: AtomicBool,
```

换成

```rust
    password: String,
    /// How often the heartbeat goes while something is relayed.
    heartbeat: Duration,
}

/// What a connection keeps once its authentication went out: what tells
/// how it ended, its relays and their heartbeat, its UDP associations.
pub(crate) struct Authenticated {
    /// The server relayed something on the connection — a byte from a
    /// tunnel's target, a UDP packet —: it took the authentication.
    relayed: Arc<AtomicBool>,
    relays: Arc<Relays>,
    pub(super) udp: Udp,
    /// Sends the heartbeat; none on a connection without QUIC datagrams.
    _heartbeat: Option<AbortOnDropHandle<()>>,
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
    pub(crate) fn new(uuid: [u8; 16], password: &str) -> Auth {
```

换成

```rust
    pub(crate) fn new(uuid: [u8; 16], password: &str, heartbeat: Duration) -> Auth {
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
            password: password.to_string(),
```

换成

```rust
            password: password.to_string(),
            heartbeat,
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
            let state = Authenticated::default();
            let mut stream = connection.open_uni().await.map_err(|e| state.ended(&e))?;
```

换成

```rust
            // nothing relayed on a connection that just came up
            let mut stream = connection.open_uni().await.map_err(|e| ended(&e, false))?;
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
                .map_err(|e| state.write_error(e))?;
            // fails only for a stream finished or reset already
            let _ = stream.finish();
            Ok(state)
```

换成

```rust
                .map_err(|e| write_error(e, false))?;
            // fails only for a stream finished or reset already
            let _ = stream.finish();
            Ok(Authenticated::start(connection, self.heartbeat))
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
    /// Something the server relayed came on the connection: a byte from a
    /// tunnel's target.
    pub(super) fn mark_relayed(&self) {
        if !self.relayed.load(Ordering::Relaxed) {
            self.relayed.store(true, Ordering::Relaxed);
```

换成

```rust
    /// The state of `connection`, its tasks started: the one that reads
    /// the server's packets, and the heartbeat where QUIC datagrams go.
    fn start(connection: &Connection, every: Duration) -> Authenticated {
        let relayed = Arc::new(AtomicBool::new(false));
        let relays = Relays::new();
        let beating = connection
            .max_datagram_size()
            .map(|_| heartbeat::start(connection, &relays, every));
        Authenticated {
            udp: Udp::start(connection, relayed.clone()),
            relayed,
            relays,
            _heartbeat: beating,
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
    /// How the connection ended, as its requests and tunnels tell it
    /// (design 3.6, 6.2). The server's own close is TUIC's, and v5 defines
    /// no codes: `tuic: the server closed the connection (code …)`, its
    /// reason cleaned and cut short. Code 0 and no reason before the server
    /// relayed anything on the connection is how tuic-server and sing-box
    /// refuse the authentication: `tuic: authentication failed` (sing-box
    /// closes so whatever failed, its session's end included; once the
    /// server relayed, the authentication went through). Any other end as
    /// QUIC tells it.
    pub(super) fn ended(&self, e: &ConnectionError) -> io::Error {
        let ConnectionError::ApplicationClosed(close) = e else {
            return connection_error(e);
        };
        if close.error_code == CLOSE
            && close.reason.is_empty()
            && !self.relayed.load(Ordering::Relaxed)
        {
            return io::Error::new(
                io::ErrorKind::PermissionDenied,
                "tuic: authentication failed",
            );
        }
        let code = close.error_code.into_inner();
        let reason = untrusted_text(&String::from_utf8_lossy(&close.reason), REASON_MAX);
        let text = if reason.is_empty() {
            format!("tuic: the server closed the connection (code {code:#x})")
        } else {
            format!("tuic: the server closed the connection (code {code:#x}: {reason})")
        };
        io::Error::new(io::ErrorKind::ConnectionAborted, text)
    }

    /// A failed read on one of the connection's streams: the connection's
    /// end as `ended` tells it, the stream's as TUIC's.
    pub(super) fn read_error(&self, e: ReadError) -> io::Error {
        match e {
            ReadError::ConnectionLost(e) => self.ended(&e),
            ReadError::Reset(code) => io::Error::new(
                io::ErrorKind::ConnectionReset,
                format!(
                    "tuic: the server reset the stream (code {:#x})",
                    code.into_inner()
                ),
            ),
            ReadError::ClosedStream
            | ReadError::IllegalOrderedRead
            | ReadError::ZeroRttRejected => closed_stream(),
        }
    }

    /// A failed write on one of the connection's streams, as `read_error`.
    pub(super) fn write_error(&self, e: WriteError) -> io::Error {
        match e {
            WriteError::ConnectionLost(e) => self.ended(&e),
            WriteError::Stopped(code) => io::Error::new(
                io::ErrorKind::ConnectionReset,
                format!(
                    "tuic: the server stopped the stream (code {:#x})",
                    code.into_inner()
                ),
            ),
            WriteError::ClosedStream | WriteError::ZeroRttRejected => closed_stream(),
        }
```

换成

```rust
    /// Something the server relayed came on the connection.
    pub(super) fn mark_relayed(&self) {
        mark(&self.relayed);
    }

    /// A relay under way on the connection, until the guard goes: the
    /// heartbeat goes meanwhile.
    pub(super) fn relaying(&self) -> Relaying {
        self.relays.begin()
    }

    /// How the connection ended, as its requests, tunnels and carriers
    /// tell it (`ended`).
    pub(super) fn ended(&self, e: &ConnectionError) -> io::Error {
        ended(e, self.relayed.load(Ordering::Relaxed))
    }

    /// Whether the connection ended as a server ends one whose
    /// authentication it refused (`refused`).
    pub(super) fn refused(&self, e: &ConnectionError) -> bool {
        refused(e, self.relayed.load(Ordering::Relaxed))
    }

    /// A failed read on one of the connection's streams (`read_error`).
    pub(super) fn read_error(&self, e: ReadError) -> io::Error {
        read_error(e, self.relayed.load(Ordering::Relaxed))
    }

    /// A failed write on one of the connection's streams (`write_error`).
    pub(super) fn write_error(&self, e: WriteError) -> io::Error {
        write_error(e, self.relayed.load(Ordering::Relaxed))
    }

    /// Test support: whether the heartbeat's task runs.
    #[cfg(test)]
    pub(super) fn beats(&self) -> bool {
        self._heartbeat
            .as_ref()
            .is_some_and(|task| !task.is_finished())
    }

    /// Test support: how many relays are under way.
    #[cfg(test)]
    pub(super) fn relays(&self) -> usize {
        self.relays.count()
    }
}

/// Marks `relayed`: the server took the authentication.
pub(super) fn mark(relayed: &AtomicBool) {
    if !relayed.load(Ordering::Relaxed) {
        relayed.store(true, Ordering::Relaxed);
    }
}

/// Whether a connection that ended so was refused its authentication: code
/// 0 and no reason before the server relayed anything (`relayed`) is how
/// tuic-server and sing-box refuse it — sing-box closes so whatever failed,
/// its session's end included; once the server relayed, the authentication
/// went through.
fn refused(e: &ConnectionError, relayed: bool) -> bool {
    matches!(e, ConnectionError::ApplicationClosed(close)
        if close.error_code == CLOSE && close.reason.is_empty() && !relayed)
}

/// How a connection ended (design 3.6, 6.2), `relayed` whether the server
/// relayed anything on it. The server's own close is TUIC's, and v5 defines
/// no codes: `tuic: the server closed the connection (code …)`, its reason
/// cleaned and cut short; a refused authentication (`refused`) is `tuic:
/// authentication failed`. Any other end as QUIC tells it.
fn ended(e: &ConnectionError, relayed: bool) -> io::Error {
    let ConnectionError::ApplicationClosed(close) = e else {
        return connection_error(e);
    };
    if refused(e, relayed) {
        return io::Error::new(
            io::ErrorKind::PermissionDenied,
            "tuic: authentication failed",
        );
    }
    let code = close.error_code.into_inner();
    let reason = untrusted_text(&String::from_utf8_lossy(&close.reason), REASON_MAX);
    let text = if reason.is_empty() {
        format!("tuic: the server closed the connection (code {code:#x})")
    } else {
        format!("tuic: the server closed the connection (code {code:#x}: {reason})")
    };
    io::Error::new(io::ErrorKind::ConnectionAborted, text)
}

/// A failed read on one of a connection's streams: the connection's end as
/// `ended` tells it, the stream's as TUIC's.
fn read_error(e: ReadError, relayed: bool) -> io::Error {
    match e {
        ReadError::ConnectionLost(e) => ended(&e, relayed),
        ReadError::Reset(code) => io::Error::new(
            io::ErrorKind::ConnectionReset,
            format!(
                "tuic: the server reset the stream (code {:#x})",
                code.into_inner()
            ),
        ),
        ReadError::ClosedStream | ReadError::IllegalOrderedRead | ReadError::ZeroRttRejected => {
            closed_stream()
        }
    }
}

/// A failed write on one of a connection's streams, as `read_error`.
fn write_error(e: WriteError, relayed: bool) -> io::Error {
    match e {
        WriteError::ConnectionLost(e) => ended(&e, relayed),
        WriteError::Stopped(code) => io::Error::new(
            io::ErrorKind::ConnectionReset,
            format!(
                "tuic: the server stopped the stream (code {:#x})",
                code.into_inner()
            ),
        ),
        WriteError::ClosedStream | WriteError::ZeroRttRejected => closed_stream(),
```

`crates/rurge-proto-quic/src/tuic/addr.rs`——把

```rust
//! an IPv6 address. (`0xff`, no address and no port, only follows the first
//! fragment of a UDP packet.)
```

换成

```rust
//! an IPv6 address; `0xff` none, with no port either: what every fragment of
//! a UDP packet but the first carries.
```

`crates/rurge-proto-quic/src/tuic/addr.rs`——把

```rust
const IPV6: u8 = 0x02;
```

换成

```rust
const IPV6: u8 = 0x02;
/// No address and no port.
pub(super) const NONE: u8 = 0xff;
```

`crates/rurge-proto-quic/src/tuic/addr.rs`——把

```rust
    Ok(())
```

换成

```rust
    Ok(())
}

/// The address at the start of `data`, as the server writes the source of
/// a UDP packet, and how many bytes it took: `None` inside for none
/// (`0xff`). `None` for anything else: a type TUIC does not know, an
/// address cut short, a name that is no host name (`HostName::from_wire`).
/// A server may name the source as a name: sing-box answers so for a
/// target that was one.
pub(super) fn get(data: &[u8]) -> Option<(Option<Target>, usize)> {
    let (&kind, rest) = data.split_first()?;
    let (host, len) = match kind {
        NONE => return Some((None, 1)),
        IPV4 => {
            let octets: [u8; 4] = rest.get(..4)?.try_into().ok()?;
            (HostName::Ip(IpAddr::from(octets)), 4)
        }
        IPV6 => {
            let octets: [u8; 16] = rest.get(..16)?.try_into().ok()?;
            (HostName::Ip(IpAddr::from(octets)), 16)
        }
        DOMAIN => {
            let (&n, name) = rest.split_first()?;
            let name = std::str::from_utf8(name.get(..usize::from(n))?).ok()?;
            (HostName::from_wire(name)?, 1 + usize::from(n))
        }
        _ => return None,
    };
    let port = rest.get(len..len + 2)?;
    let port = u16::from_be_bytes([port[0], port[1]]);
    Some((Some(Target::new(host, port)), 1 + len + 2))
```

`crates/rurge-proto-quic/src/tuic/tcp.rs`——把

```rust
//!   shows as the stream ending.
```

换成

```rust
//!   shows as the stream ending.
//! - While a tunnel lives, the connection's heartbeat goes (`heartbeat`).
```

`crates/rurge-proto-quic/src/tuic/tcp.rs`——把

```rust
use super::auth::Authenticated;
```

换成

```rust
use super::auth::Authenticated;
use super::heartbeat::Relaying;
```

`crates/rurge-proto-quic/src/tuic/tcp.rs`——把

```rust
    Ok(Tunnel { send, recv, lease })
```

换成

```rust
    Ok(Tunnel {
        send,
        recv,
        _relaying: state.relaying(),
        lease,
    })
```

`crates/rurge-proto-quic/src/tuic/tcp.rs`——把

```rust
/// connection its lease keeps. Shutdown sends the FIN the target then
/// sees; reading goes on. Dropped, the stream ends both ways (a FIN, and
/// STOP_SENDING for what is left unread). No `Debug`: nothing in it is
/// worth printing.
```

换成

```rust
/// connection its lease keeps, counted among the connection's relays.
/// Shutdown sends the FIN the target then sees; reading goes on. Dropped,
/// the stream ends both ways (a FIN, and STOP_SENDING for what is left
/// unread). No `Debug`: nothing in it is worth printing.
```

`crates/rurge-proto-quic/src/tuic/tcp.rs`——把

```rust
    recv: RecvStream,
```

换成

```rust
    recv: RecvStream,
    _relaying: Relaying,
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
//!   soon as the Connect naming its target (`addr`) is written (`tcp`).
```

换成

```rust
//!   soon as the Connect naming its target (`addr`) is written (`tcp`).
//! - UDP goes in QUIC datagrams of the same connection, or in streams of
//!   its own where the server takes no datagrams, each client association
//!   an association of its own there (`udp`).
//! - While anything is relayed, a heartbeat keeps the connection up
//!   (`heartbeat`).
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
mod tcp;
```

换成

```rust
mod heartbeat;
mod tcp;
mod udp;
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_proto::{BuildError, Outbound, OutboundError};
use rustls::RootCertStore;
use std::sync::Arc;
```

换成

```rust
use rurge_net::connector::{BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, Target};
use rurge_proto::{BuildError, Outbound, OutboundError, UdpSupport};
use rustls::RootCertStore;
use std::sync::Arc;
use udp::TuicUdp;
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
    session: Session<Auth>,
```

换成

```rust
    /// Shared with its UDP carriers, which may outlive it.
    session: Arc<Session<Auth>>,
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
        let auth = Auth::new(*uuid.expose(), password.expose());
        Ok(TuicOutbound {
            name: name.to_string(),
            session: Session::new(name, dialer, auth),
```

换成

```rust
        let auth = Auth::new(*uuid.expose(), password.expose(), heartbeat::EVERY);
        Ok(TuicOutbound {
            name: name.to_string(),
            session: Arc::new(Session::new(name, dialer, auth)),
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto-quic --lib`
Expected: FAIL——出站还报"不支持 UDP"，UDP 与心跳的用例都失败：

```text
test tuic::tests::a_carrier_finds_a_changed_route_by_itself ... FAILED
test tuic::tests::a_carrier_moves_to_a_new_connection_after_a_close_or_a_restart ... FAILED
test tuic::tests::a_full_inbox_drops_rather_than_holds_up_the_other_carriers ... FAILED
test tuic::tests::a_lost_fragment_loses_its_packet_alone ... FAILED
test tuic::tests::a_full_outbox_drops_rather_than_holds_up_a_send ... FAILED
test tuic::tests::a_packet_too_long_is_refused_and_nothing_goes_out ... FAILED
test tuic::tests::a_refused_authentication_fails_a_send_once_and_the_next_binds_anew ... FAILED
test tuic::tests::a_carrier_gone_dissociates_its_association ... FAILED
test tuic::tests::an_association_puts_at_most_eight_packets_together_at_a_time ... FAILED
test tuic::tests::carriers_move_off_a_connection_whose_underlying_policy_changed ... FAILED
test tuic::tests::carriers_that_come_together_share_one_connection_each_its_own_association ... FAILED
test tuic::tests::concurrent_sends_keep_each_packets_fragments_together ... FAILED
test tuic::tests::long_packets_go_in_fragments_both_ways ... FAILED
test tuic::tests::opening_a_carrier_is_bounded_by_its_time ... FAILED
test tuic::tests::an_answer_tells_the_authentication_went_through ... FAILED
test tuic::tests::payloads_never_show_in_errors_or_logs ... FAILED
test tuic::tests::the_route_is_compared_at_most_once_a_second_on_udp ... FAILED
test tuic::tests::a_send_given_up_on_never_cuts_its_packet_short ... FAILED
test tuic::tests::udp_goes_in_datagrams_of_the_connection_to_any_target ... FAILED
test tuic::tests::udp_targets_go_out_as_the_server_reads_them ... FAILED
test tuic::tests::without_datagrams_each_packet_goes_on_a_stream_of_its_own ... FAILED
test tuic::tests::heartbeats_go_while_a_relay_is_under_way_and_stop_when_none_is ... FAILED
assertion `left == right` failed
  left: Unsupported
 right: Native
test result: FAILED. 125 passed; 22 failed; 1 ignored; 0 measured; 0 filtered out; finished in 3.18s
```

- [ ] **Step 3: 接上 `udp()` 与 `open_udp`**

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
```

换成

```rust
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }

    /// Whatever the policy says: TUIC always relays UDP (manual).
    fn udp(&self) -> UdpSupport {
        UdpSupport::Native
    }

    /// An association of its own on the policy's connection, dialed when
    /// there is none. Nothing answers the authentication: the carrier is
    /// there as soon as the connection is up.
    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedPacketSocket, OutboundError>> {
        Box::pin(async move {
            // one budget for the connection (when one is dialed) and QUIC's
            // handshake
            let open = TuicUdp::open(self.session.clone(), opts);
            match tokio::time::timeout(opts.timeout, open).await {
                Ok(udp) => Ok(Box::new(udp?) as BoxedPacketSocket),
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
```

要点：
- 载体持有连接的租约；每次发送前检查连接开着、没在离开、没被标成换了路，底层策略至多每秒比较一次；换到新连接时在那里另取关联号。
- 一个载体一次只发一个包的全部分片（发送锁）。
- `quic` 模式的包交给连接的写入任务（P9）：`send_to` 从不等待，队列满即丢；写失败只丢这一个包。
- 心跳只在有 TCP 隧道或 UDP 关联时发，服务端不收数据报时不发。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto-quic` → 通过（新增 38 条：经回环 UDP 回显的往返（两种模式）、全锥、两个方向的分片、丢一片只丢这个包、每个关联至多同时重组 8 个包、并发发送时分片不交错（多线程）、载体离开后发 `Dissociate`、有转发时有心跳且没有时停、口令被拒时一次发送报认证失败而下一次重新绑定、连接关闭或重启后载体迁到新连接、底层策略变了时载体迁移且至多每秒比较一次、收件箱满时丢包、被放弃的发送不会截短它的包、写入队列满时丢包而不拖住发送、超长的包、名字与 IPv6 目标、载荷不进日志与错误文本，以及 `udp.rs` / `heartbeat.rs` / `addr.rs` 的编解码与计时）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-proto-quic
git commit -m "feat(proto-quic): TUIC v5 的 UDP 与心跳——数据报或单向流、分片与重组、Dissociate、连接的写入任务"
```

### Task 4: TUIC v4

v4 与 v5 共用一套实现（P11、P12）：认证 `04 00 ‖ BLAKE3(token)`；TCP 等 `04 ff ‖ REP`；UDP `04 02 ‖ ASSOC_ID u32 ‖ LEN ‖ ADDR ‖ 负载`、不分片；`Dissociate` 用 `u32`；心跳走单向流、每 10 秒；关闭码。`FakeTuic` 同时扮演 v4（脚本 `TuicScript::v4(token)` 与 `silent`、`FakeTuic::close_connections_with`）。回包来源里 IPv4 映射的 IPv6 地址按 IPv4 读。

**Files:**
- Modify: `crates/rurge-proto-quic/{Cargo.toml, src/tuic/mod.rs, src/tuic/auth.rs, src/tuic/addr.rs, src/tuic/tcp.rs, src/tuic/udp.rs, src/tuic/heartbeat.rs}`（均含用例）、`src/testing/tuic.rs`

**Interfaces:**
- Consumes: Task 2 / 3 的全部；`blake3`。
- Produces: `TuicOutbound::new` 对两个版本都能构建（去掉 Task 2 的临时报错）；`RecordedTuicAuth.uuid` 改为 `Option`，包与 `Dissociate` 的关联号改为 `u32`；`TuicScript::v4(token)` 与脚本项 `silent`、`FakeTuic::close_connections_with(code, reason)`

- [ ] **Step 1: 先写用例（连同假服务端的 v4）**

`crates/rurge-proto-quic/Cargo.toml`——把

```toml
blake2.workspace = true
```

换成

```toml
blake2.workspace = true
blake3.workspace = true
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
//! A scriptable TUIC v5 server (phase 2 M7 design 6.4): QUIC with ALPN
//! `h3` on a loopback port of its own (`ServerPort`: it can restart) or on
//! a socket it is given (`MultiPort` for port hopping). It checks the
//! authentication against what its own side of the TLS session exports,
//! holds the commands that come before it (as the SPEC asks; sing-box
//! leaves early datagrams in QUIC's queue so), relays each Connect to its
//! target, and gives each UDP association a loopback socket of its own
//! (full cone), which answers the way the association's first packet came:
//! in datagrams — split into fragments of its own when one cannot hold an
//! answer — or on streams. A unidirectional stream it cannot read as one
//! whole command — one cut short — closes the connection, as sing-box
//! closes it. It can hold the client's streams unread for a while, each
//! taking no more than a window of the script's meanwhile. What the client
//! sends is read by code that is not the client's: the commands and their
//! addresses by the code here, which also writes the answers. It counts
//! heartbeats and records Dissociates. It never resolves a name.
```

换成

```rust
//! A scriptable TUIC server (phase 2 M7 design 6.4) — v5, or v4 as tuic
//! 0.8.5's server speaks it when its script names a token: QUIC with ALPN
//! `h3` on a loopback port of its own (`ServerPort`: it can restart) or on
//! a socket it is given (`MultiPort` for port hopping). It checks the
//! authentication — v5's against what its own side of the TLS session
//! exports, v4's against the BLAKE3 hash of its token —, holds the commands
//! that come before it (as both protocols ask; sing-box leaves early
//! datagrams in QUIC's queue so), relays each Connect to its target —
//! answering a v4 one with whether it reached it —, and gives each UDP
//! association a loopback socket of its own (full cone), which answers the
//! way the association's first packet came: in datagrams — a v5 answer that
//! one cannot hold split into fragments of its own, a v4 one dropped — or on
//! streams. A unidirectional stream it cannot read as one whole command —
//! one cut short — closes the connection, as sing-box closes it. It can
//! hold the client's streams unread for a while, each taking no more than a
//! window of the script's meanwhile. A v4 fake does what tuic 0.8.5's
//! server does that a client must live with: it names an IPv4 source in
//! IPv6 form (its dual-stack socket), and closes the connection with the
//! protocol's codes on a stream whose command's header it cannot read
//! (0xfffffff0; a packet cut short past its header is only dropped), a
//! token it does not know (0xfffffff1), a datagram that is no packet and a
//! packet that does not come the way the connection's first came
//! (0xfffffff3). What the client sends is read by code that is not the
//! client's: the commands and their addresses by the code here, which also
//! writes the answers. It counts heartbeats and records Dissociates. It
//! never resolves a name.
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
// the wire as the fake knows it
const VERSION: u8 = 0x05;
```

换成

```rust
// the wire as the fake knows it: the versions, the commands' types
const V4: u8 = 0x04;
const V5: u8 = 0x05;
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
/// How the fake closes a connection, and stops a stream: with 0, as sing-box
/// does.
const CLOSE: VarInt = VarInt::from_u32(0);
```

换成

```rust
/// v4's answer to a Connect, `04 ff ‖ REP`, and its two replies.
const RESPONSE: u8 = 0xff;
const SUCCEEDED: u8 = 0x00;
const FAILED: u8 = 0xff;
/// How a v5 fake closes a connection, and how every fake stops a stream:
/// with 0, as sing-box does.
const CLOSE: VarInt = VarInt::from_u32(0);
/// How a v4 fake closes a connection, with tuic 0.8.5's codes and reasons:
/// for a stream it cannot read, the text of the I/O error that a command
/// cut short gives.
const PROTOCOL_ERROR: (u32, &[u8]) = (0xffff_fff0, b"early eof");
const AUTHENTICATION_FAILED: (u32, &[u8]) = (0xffff_fff1, b"authentication failed");
const BAD_COMMAND: (u32, &[u8]) = (0xffff_fff3, b"bad command");
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    /// The one user it lets in, by its UUID's 16 bytes, and its password.
    pub uuid: [u8; 16],
    pub password: String,
    /// The code it closes a connection with when the authentication
    /// failed, never with a reason: 0 — as tuic-server 1.0.0 and sing-box
    /// close it — unless set.
```

换成

```rust
    /// v5: the one user it lets in, by its UUID's 16 bytes, and its
    /// password.
    pub uuid: [u8; 16],
    pub password: String,
    /// Speak TUIC v4, as tuic 0.8.5's server, letting this token in.
    pub v4_token: Option<String>,
    /// v5: the code it closes a connection with when the authentication
    /// failed, never with a reason: 0 — as tuic-server 1.0.0 and sing-box
    /// close it — unless set. A v4 fake closes it as tuic 0.8.5 does.
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    pub connect_to: Option<SocketAddr>,
```

换成

```rust
    pub connect_to: Option<SocketAddr>,
    /// Never carry out a Connect, nor answer a v4 one: what the client sends
    /// on it is dropped until it gives up.
    pub silent: bool,
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    /// The first UDP answer it splits into fragments goes without this
    /// one.
    pub drop_fragment: Option<u8>,
```

换成

```rust
    /// v5: the first UDP answer it splits into fragments goes without this
    /// one.
    pub drop_fragment: Option<u8>,
}

impl TuicScript {
    /// A v4 server that lets `token` in.
    pub fn v4(token: &str) -> TuicScript {
        TuicScript {
            v4_token: Some(token.to_string()),
            ..TuicScript::default()
        }
    }
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    pub uuid: [u8; 16],
    pub token: [u8; 32],
    /// What the fake's own side of the TLS session exports for its user:
    /// the token it lets in.
```

换成

```rust
    /// v5's user; v4 names none.
    pub uuid: Option<[u8; 16]>,
    pub token: [u8; 32],
    /// The token it lets in: v5's as the fake's own side of the TLS session
    /// exports it for its user, v4's the hash of its token.
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    pub connection: usize,
    pub association: u16,
    pub packet: u16,
```

换成

```rust
    pub connection: usize,
    /// v5's are 16 bits.
    pub association: u32,
    /// v4's packets have neither ids nor fragments: 0, 1 and 0.
    pub packet: u16,
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    pub connection: usize,
    pub association: u16,
}
```

换成

```rust
    pub connection: usize,
    pub association: u32,
}
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
/// their own: `05 02 ‖ ASSOC_ID ‖ PKT_ID ‖ FRAG_TOTAL ‖ FRAG_ID ‖ SIZE ‖
/// ADDR ‖ payload`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TuicPacket {
    pub association: u16,
```

换成

```rust
/// their own: v5's `05 02 ‖ ASSOC_ID ‖ PKT_ID ‖ FRAG_TOTAL ‖ FRAG_ID ‖ SIZE
/// ‖ ADDR ‖ payload`, or v4's `04 02 ‖ ASSOC_ID ‖ LEN ‖ ADDR ‖ payload`, which
/// has neither a packet id nor fragments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TuicPacket {
    /// v5's are 16 bits.
    pub association: u32,
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    pub fn whole(association: u16, packet: u16, address: &str, payload: &[u8]) -> TuicPacket {
```

换成

```rust
    pub fn whole(association: u32, packet: u16, address: &str, payload: &[u8]) -> TuicPacket {
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![VERSION, PACKET];
        out.extend_from_slice(&self.association.to_be_bytes());
        out.extend_from_slice(&self.packet.to_be_bytes());
        out.extend_from_slice(&[self.fragments, self.fragment]);
```

换成

```rust
    /// As a v5 server writes it.
    pub fn encode(&self) -> Vec<u8> {
        let association = u16::try_from(self.association).expect("a v5 association id of 16 bits");
        let mut out = vec![V5, PACKET];
        out.extend_from_slice(&association.to_be_bytes());
        out.extend_from_slice(&self.packet.to_be_bytes());
        out.extend_from_slice(&[self.fragments, self.fragment]);
        self.put_rest(&mut out);
        out
    }

    /// As a v4 server writes it: the packet's id and fragments are not.
    pub fn encode_v4(&self) -> Vec<u8> {
        let mut out = vec![V4, PACKET];
        out.extend_from_slice(&self.association.to_be_bytes());
        self.put_rest(&mut out);
        out
    }

    /// `SIZE ‖ ADDR ‖ payload`, v4's LEN alike.
    fn put_rest(&self, out: &mut Vec<u8>) {
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
            Some(address) => put_address(&mut out, address),
        }
        out.extend_from_slice(&self.payload);
        out
    }

    /// `None` for anything but a Packet whose SIZE is just what follows its
    /// address, as sing-box reads one.
    fn decode(data: &[u8]) -> Option<TuicPacket> {
        let (header, rest) = data.split_at_checked(10)?;
        if header[..2] != [VERSION, PACKET] {
            return None;
        }
```

换成

```rust
            Some(address) => put_address(out, address),
        }
        out.extend_from_slice(&self.payload);
    }

    /// `None` for anything but a v5 Packet whose SIZE is just what follows
    /// its address, as sing-box reads one.
    fn decode(data: &[u8]) -> Option<TuicPacket> {
        let (header, rest) = data.split_at_checked(10)?;
        if header[..2] != [V5, PACKET] {
            return None;
        }
        let association = u16::from_be_bytes([header[2], header[3]]);
        let mut packet = TuicPacket::rest(header[8..].try_into().ok()?, rest)?;
        packet.association = u32::from(association);
        packet.packet = u16::from_be_bytes([header[4], header[5]]);
        (packet.fragments, packet.fragment) = (header[6], header[7]);
        Some(packet)
    }

    /// `None` for anything but a v4 Packet whose LEN is just what follows
    /// its address — whole, as v4 has no fragments.
    fn decode_v4(data: &[u8]) -> Option<TuicPacket> {
        let (header, rest) = data.split_at_checked(8)?;
        if header[..2] != [V4, PACKET] {
            return None;
        }
        let mut packet = TuicPacket::rest(header[6..].try_into().ok()?, rest)?;
        packet.association = u32::from_be_bytes(header[2..6].try_into().ok()?);
        Some(packet)
    }

    /// The address and the payload that follow a header ending with
    /// `size`, as a whole packet of association 0.
    fn rest(size: [u8; 2], rest: &[u8]) -> Option<TuicPacket> {
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
        if rest.len() != usize::from(u16::from_be_bytes([header[8], header[9]])) {
```

换成

```rust
        if rest.len() != usize::from(u16::from_be_bytes(size)) {
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
            association: u16::from_be_bytes([header[2], header[3]]),
            packet: u16::from_be_bytes([header[4], header[5]]),
            fragments: header[6],
            fragment: header[7],
```

换成

```rust
            association: 0,
            packet: 0,
            fragments: 1,
            fragment: 0,
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    script: TuicScript,
```

换成

```rust
    script: TuicScript,
    /// The version it speaks, as the script says.
    version: u8,
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    /// `answer` in datagrams of at most `max` bytes: whole when it fits,
    /// else in fragments of one size, only the first naming the source. The
    /// first answer split goes without the fragment the script drops.
    fn split(&self, answer: TuicPacket, max: usize) -> Vec<Vec<u8>> {
        let whole = answer.encode();
        if whole.len() <= max {
            return vec![whole];
```

换成

```rust
    /// `packet` as the fake's version writes it.
    fn encode(&self, packet: &TuicPacket) -> Vec<u8> {
        if self.version == V4 {
            packet.encode_v4()
        } else {
            packet.encode()
        }
    }

    /// A packet of the client's as the fake's version reads it.
    fn decode(&self, data: &[u8]) -> Option<TuicPacket> {
        if self.version == V4 {
            TuicPacket::decode_v4(data)
        } else {
            TuicPacket::decode(data)
        }
    }

    /// `answer` in datagrams of at most `max` bytes: whole when it fits;
    /// else a v4 answer is dropped (tuic 0.8.5's server fails to send it),
    /// and a v5 one goes in fragments of one size, only the first naming
    /// the source. The first answer split goes without the fragment the
    /// script drops.
    fn split(&self, answer: TuicPacket, max: usize) -> Vec<Vec<u8>> {
        let whole = self.encode(&answer);
        if whole.len() <= max {
            return vec![whole];
        }
        if self.version == V4 {
            return Vec::new();
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
            shared: Arc::new(Shared {
```

换成

```rust
            shared: Arc::new(Shared {
                version: if script.v4_token.is_some() { V4 } else { V5 },
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    /// The heartbeats it heard so far, on every connection.
```

换成

```rust
    /// The heartbeats it heard so far, on every connection: v5's in
    /// datagrams, v4's on streams.
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    /// Sends `packet`, as it is, to the client in a datagram of connection
    /// `n`.
    pub fn send_udp(&self, n: usize, packet: &TuicPacket) {
        self.connection(n)
            .send_datagram(packet.encode().into())
```

换成

```rust
    /// Sends `packet`, as the fake's version writes it, to the client in a
    /// datagram of connection `n`.
    pub fn send_udp(&self, n: usize, packet: &TuicPacket) {
        self.connection(n)
            .send_datagram(self.shared.encode(packet).into())
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
        for connection in self.shared.connections().iter() {
            connection.close(CLOSE, b"");
```

换成

```rust
        self.close_connections_with(0, b"");
    }

    /// Closes every connection accepted so far with `code` and `reason`.
    pub fn close_connections_with(&self, code: u32, reason: &[u8]) {
        for connection in self.shared.connections().iter() {
            connection.close(VarInt::from_u32(code), reason);
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
        associations: Mutex::default(),
```

换成

```rust
        associations: Mutex::default(),
        on_streams: Mutex::default(),
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    associations: Mutex<HashMap<u16, Association>>,
```

换成

```rust
    associations: Mutex<HashMap<u32, Association>>,
    /// v4: whether the connection's UDP comes on streams, as its first
    /// packet came — tuic 0.8.5's server holds the rest to it.
    on_streams: Mutex<Option<bool>>,
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
/// Packet (`quic` mode) or a Dissociate. A stream that holds no whole
/// command — one cut short — closes the connection, as sing-box closes it
/// on a stream it cannot read: every tunnel and association on it ends.
/// While the fake holds streams, it waits unread.
```

换成

```rust
/// Packet (`quic` mode), a Dissociate, or a v4 heartbeat. A stream that
/// holds no whole command — one cut short — closes the connection, as
/// sing-box closes it on a stream it cannot read, and tuic 0.8.5 on one
/// whose command's header it cannot: every tunnel and association on it
/// ends. tuic 0.8.5 reads a packet's payload apart, and loses no more than
/// the packet when that is cut short. While the fake holds streams, it
/// waits unread.
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    match command.get(..2) {
        Some([VERSION, AUTHENTICATE]) => authenticate(&command, &link, &authenticated),
        Some([VERSION, PACKET | DISSOCIATE]) => {
            if authenticated
                .subscribe()
                .wait_for(|&let_in| let_in)
                .await
                .is_err()
            {
                return;
            }
            if command[1] == PACKET {
                match TuicPacket::decode(&command) {
                    Some(packet) => link.relay_packet(packet, true).await,
                    None => link.unreadable(),
                }
            } else if let [_, _, high, low] = command[..] {
                link.dissociate(u16::from_be_bytes([high, low]));
            } else {
                link.unreadable();
            }
```

换成

```rust
    let version = link.shared.version;
    let kind = match command[..] {
        [v, kind, ..] if v == version => kind,
        _ => return link.unreadable(),
    };
    if kind == AUTHENTICATE {
        return authenticate(&command, &link, &authenticated);
    }
    if !matches!(kind, PACKET | DISSOCIATE | HEARTBEAT) {
        return link.unreadable();
    }
    if authenticated
        .subscribe()
        .wait_for(|&let_in| let_in)
        .await
        .is_err()
    {
        return;
    }
    match (version, kind, &command[2..]) {
        (_, PACKET, _) => match link.shared.decode(&command) {
            Some(packet) => link.relay_packet(packet, true).await,
            None if version == V4 && v4_header_whole(&command) => {}
            None => link.unreadable(),
        },
        (V5, DISSOCIATE, &[high, low]) => {
            link.dissociate(u32::from(u16::from_be_bytes([high, low])))
        }
        (V4, DISSOCIATE, &[a, b, c, d]) => link.dissociate(u32::from_be_bytes([a, b, c, d])),
        // v4's goes on a stream (v5's in a datagram)
        (V4, HEARTBEAT, []) => {
            link.shared.heartbeats.fetch_add(1, Ordering::SeqCst);
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
/// The authentication, checked against what the fake's own side of the TLS
/// session exports for its user, with the UUID's bytes as the label and the
/// password as the context. Let in, the connection's commands go on;
/// refused, the connection is closed as the reference servers close it,
/// with no reason.
fn authenticate(command: &[u8], link: &Link, authenticated: &watch::Sender<bool>) {
    let Some((uuid, token)) = command[2..].split_first_chunk::<16>() else {
        return;
    };
    let Ok(token) = <[u8; 32]>::try_from(token) else {
        return;
    };
    let script = &link.shared.script;
    let mut expected = [0u8; 32];
    link.connection
        .export_keying_material(&mut expected, &script.uuid, script.password.as_bytes())
        .expect("the keying material of a session whose handshake is done");
    let accepted = *uuid == script.uuid && token == expected;
```

换成

```rust
/// Whether `command` holds a v4 Packet's header whole — `04 02 ‖ ASSOC_ID ‖
/// LEN ‖ ADDR` —, whatever follows it.
fn v4_header_whole(command: &[u8]) -> bool {
    command.get(8..).and_then(take_address).is_some()
}

/// The authentication, checked against what the fake lets in: v5's token
/// against what its own side of the TLS session exports for its user, with
/// the UUID's bytes as the label and the password as the context; v4's
/// against the BLAKE3 hash of its token. Let in, the connection's commands
/// go on; refused, the connection is closed as the reference servers close
/// it.
fn authenticate(command: &[u8], link: &Link, authenticated: &watch::Sender<bool>) {
    let script = &link.shared.script;
    let (uuid, token, expected, (code, reason)) = match &script.v4_token {
        Some(own) => {
            let Ok(token) = <[u8; 32]>::try_from(&command[2..]) else {
                return;
            };
            let expected = *blake3::hash(own.as_bytes()).as_bytes();
            (None, token, expected, AUTHENTICATION_FAILED)
        }
        None => {
            let Some((uuid, token)) = command[2..].split_first_chunk::<16>() else {
                return;
            };
            let Ok(token) = <[u8; 32]>::try_from(token) else {
                return;
            };
            let mut expected = [0u8; 32];
            link.connection
                .export_keying_material(&mut expected, &script.uuid, script.password.as_bytes())
                .expect("the keying material of a session whose handshake is done");
            (
                Some(*uuid),
                token,
                expected,
                (script.auth_failure_code, &b""[..]),
            )
        }
    };
    let accepted = token == expected && uuid.is_none_or(|uuid| uuid == script.uuid);
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
            uuid: *uuid,
```

换成

```rust
            uuid,
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
        link.connection
            .close(VarInt::from_u32(script.auth_failure_code), b"");
```

换成

```rust
        link.connection.close(VarInt::from_u32(code), reason);
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
/// wait in QUIC's queue until then, as sing-box leaves them —: heartbeats
/// counted, packets relayed.
```

换成

```rust
/// wait in QUIC's queue until then, as sing-box leaves them —: packets
/// relayed, v5's heartbeats counted. A v4 fake closes the connection on
/// anything but a packet, as tuic 0.8.5's server does.
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    while let Ok(datagram) = link.connection.read_datagram().await {
        match datagram.get(..2) {
            Some([VERSION, HEARTBEAT]) => {
                link.shared.heartbeats.fetch_add(1, Ordering::SeqCst);
            }
            Some([VERSION, PACKET]) => {
                if let Some(packet) = TuicPacket::decode(&datagram) {
```

换成

```rust
    let version = link.shared.version;
    while let Ok(datagram) = link.connection.read_datagram().await {
        match (version, datagram.get(..2)) {
            (_, Some(&[v, PACKET])) if v == version => {
                if let Some(packet) = link.shared.decode(&datagram) {
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
                    link.relay_packet(packet, false).await;
                }
            }
            _ => {}
```

换成

```rust
                    link.relay_packet(packet, false).await;
                }
            }
            (V5, Some(&[V5, HEARTBEAT])) => {
                link.shared.heartbeats.fetch_add(1, Ordering::SeqCst);
            }
            (V4, _) => return link.close(BAD_COMMAND),
            _ => {}
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    /// is whole.
    async fn relay_packet(&self, packet: TuicPacket, on_stream: bool) {
```

换成

```rust
    /// is whole. A v4 fake closes the connection on a packet that does not
    /// come the way the connection's first came.
    async fn relay_packet(&self, packet: TuicPacket, on_stream: bool) {
        if self.shared.version == V4
            && *self
                .on_streams
                .lock()
                .expect("mode")
                .get_or_insert(on_stream)
                != on_stream
        {
            return self.close(BAD_COMMAND);
        }
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    fn dissociate(&self, association: u16) {
```

换成

```rust
    fn dissociate(&self, association: u32) {
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    /// Closes the connection for a stream it cannot read, as sing-box
    /// closes it: code 0, no reason.
    fn unreadable(&self) {
        self.connection.close(CLOSE, b"");
```

换成

```rust
    /// Closes the connection for a stream it cannot read: as sing-box
    /// closes it (v5), code 0 and no reason; as tuic 0.8.5 closes it (v4),
    /// with its protocol error.
    fn unreadable(&self) {
        if self.shared.version == V4 {
            self.close(PROTOCOL_ERROR);
        } else {
            self.connection.close(CLOSE, b"");
        }
    }

    /// Closes the connection with one of v4's codes and its reason.
    fn close(&self, (code, reason): (u32, &[u8])) {
        self.connection.close(VarInt::from_u32(code), reason);
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    fn open(link: &Link, association: u16, on_stream: bool) -> io::Result<Association> {
```

换成

```rust
    fn open(link: &Link, association: u32, on_stream: bool) -> io::Result<Association> {
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
/// its source — full cone —, packets counted from 0: on a stream of its own
/// each when the association's first packet came so, else in datagrams, in
/// fragments when one cannot hold it.
```

换成

```rust
/// its source — full cone; a v4 fake names an IPv4 source in IPv6 form, as
/// tuic 0.8.5's dual-stack socket hears it —, v5's packets counted from 0:
/// on a stream of its own each when the association's first packet came
/// so, else in datagrams, in fragments when one cannot hold a v5 one.
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    association: u16,
```

换成

```rust
    association: u32,
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
        };
        let answer = TuicPacket::whole(association, packets, &from.to_string(), &buf[..n]);
```

换成

```rust
        };
        let from = match from {
            SocketAddr::V4(v4) if shared.version == V4 => {
                SocketAddr::new(IpAddr::V6(v4.ip().to_ipv6_mapped()), v4.port())
            }
            from => from,
        };
        let answer = TuicPacket::whole(association, packets, &from.to_string(), &buf[..n]);
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
            if stream.write_all(&answer.encode()).await.is_ok() {
```

换成

```rust
            if stream.write_all(&shared.encode(&answer)).await.is_ok() {
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
/// through, then relayed to its target, the half-closes too. One it cannot
/// carry out — no Connect, a name it has no `connect_to` for, a target
/// that refuses — it ends as sing-box ends one whose target it cannot
/// reach: STOP_SENDING (code 0) for what the client sends, FIN for what it
/// reads.
async fn relay(
    send: SendStream,
```

换成

```rust
/// through, then relayed to its target, the half-closes too — a v4 fake
/// first answers that it reached it. One it cannot carry out — no Connect,
/// a name it has no `connect_to` for, a target that refuses — it ends as
/// sing-box ends one whose target it cannot reach: STOP_SENDING (code 0)
/// for what the client sends, FIN for what it reads; a v4 fake answers it
/// could not reach the target, as tuic 0.8.5's server does, and ends the
/// stream.
async fn relay(
    mut send: SendStream,
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    let Some(address) = read_connect(&mut recv).await else {
```

换成

```rust
    let Some(address) = read_connect(&mut recv, shared.version).await else {
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
        return;
    }
    // never resolves: a name without `connect_to` is a dead end
```

换成

```rust
        return;
    }
    if shared.script.silent {
        let _ = tokio::io::copy(&mut recv, &mut tokio::io::sink()).await;
        return;
    }
    // never resolves: a name without `connect_to` is a dead end
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
    let Some(mut upstream) = upstream else {
        return refuse(send, recv);
    };
```

换成

```rust
    let v4 = shared.version == V4;
    let Some(mut upstream) = upstream else {
        if v4 {
            let _ = send.write_all(&[V4, RESPONSE, FAILED]).await;
            let _ = send.finish();
            return;
        }
        return refuse(send, recv);
    };
    if v4 && send.write_all(&[V4, RESPONSE, SUCCEEDED]).await.is_err() {
        return;
    }
```

`crates/rurge-proto-quic/src/testing/tuic.rs`——把

```rust
/// The target of a Connect: `05 01 ‖ ADDR`; `None` for anything else.
async fn read_connect(recv: &mut RecvStream) -> Option<Address> {
    let mut header = [0u8; 2];
    recv.read_exact(&mut header).await.ok()?;
    if header != [VERSION, CONNECT] {
```

换成

```rust
/// The target of a Connect: `VER 01 ‖ ADDR`; `None` for anything else.
async fn read_connect(recv: &mut RecvStream, version: u8) -> Option<Address> {
    let mut header = [0u8; 2];
    recv.read_exact(&mut header).await.ok()?;
    if header != [version, CONNECT] {
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
    /// The spec of `tuic-v5, 127.0.0.1, <port>, <params>`, as the
    /// configuration reads it.
    fn spec(port: u16, params: &str) -> TuicSpec {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let line = format!("tuic-v5, 127.0.0.1, {port}, {params}");
```

换成

```rust
    /// The spec of `<keyword>, 127.0.0.1, <port>, <params>`, as the
    /// configuration reads it.
    fn spec_of(keyword: &str, port: u16, params: &str) -> TuicSpec {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let line = format!("{keyword}, 127.0.0.1, {port}, {params}");
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
        spec
    }
```

换成

```rust
        spec
    }

    /// A `tuic-v5` line's spec.
    fn spec(port: u16, params: &str) -> TuicSpec {
        spec_of("tuic-v5", port, params)
    }
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
        assert_eq!((auths[0].connection, auths[0].uuid), (0, UUID));
```

换成

```rust
        assert_eq!((auths[0].connection, auths[0].uuid), (0, Some(UUID)));
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
        let request = tcp::request(&target(echo)).unwrap();
```

换成

```rust
        let request = tcp::request(Version::V5, &target(echo)).unwrap();
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
    /// The outbound of the fake's user, its connections dialled through
    /// `connector` and its heartbeat every `every`.
    fn built(fake: &FakeTuic, connector: Arc<dyn Connector>, every: Duration) -> TuicOutbound {
        let port = fake.addr().port();
        let spec = spec(port, &credentials(ID, PASSWORD));
        let dialer = Dialer::new(
            connector,
            Target::new(HostName::parse("127.0.0.1"), port),
```

换成

```rust
    /// The outbound of the fake's v5 user, its connections dialled through
    /// `connector` and its heartbeat every `every`.
    fn built(fake: &FakeTuic, connector: Arc<dyn Connector>, every: Duration) -> TuicOutbound {
        let spec = spec(fake.addr().port(), &credentials(ID, PASSWORD));
        built_from(&spec, fake, connector, every)
    }

    /// `spec`'s outbound to the fake, its connections dialled through
    /// `connector` and its heartbeat every `every`.
    fn built_from(
        spec: &TuicSpec,
        fake: &FakeTuic,
        connector: Arc<dyn Connector>,
        every: Duration,
    ) -> TuicOutbound {
        let dialer = Dialer::new(
            connector,
            Target::new(HostName::parse("127.0.0.1"), fake.addr().port()),
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
        let auth = Auth::new(UUID, PASSWORD, every);
        TuicOutbound {
            name: "T".to_string(),
            session: Arc::new(Session::new("T", dialer, auth)),
```

换成

```rust
        TuicOutbound {
            name: "T".to_string(),
            version: Version::of(&spec.version),
            session: Arc::new(Session::new("T", dialer, Auth::new(&spec.version, every))),
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
    fn carried(fake: &FakeTuic) -> Vec<(usize, u16)> {
```

换成

```rust
    fn carried(fake: &FakeTuic) -> Vec<(usize, u32)> {
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
        let rerouted = Arc::new(Rerouted::new());
        let out = built(&fake, rerouted.clone(), heartbeat::EVERY);
        let (one, two) = (udp_carrier(&out).await, udp_carrier(&out).await);
```

换成

```rust
        let rerouted = Arc::new(Rerouted::new());
        let out = built(&fake, rerouted.clone(), heartbeat::every(Version::V5));
        let (one, two) = (udp_carrier(&out).await, udp_carrier(&out).await);
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
        let mut dissociated: Vec<(usize, u16)> = fake
```

换成

```rust
        let mut dissociated: Vec<(usize, u32)> = fake
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
        let fake = FakeTuic::spawn(script());
        let rerouted = Arc::new(Rerouted::new());
        let out = built(&fake, rerouted.clone(), heartbeat::EVERY);
        let carrier = udp_carrier(&out).await;
        let before = rerouted.asked();
```

换成

```rust
        let fake = FakeTuic::spawn(script());
        let rerouted = Arc::new(Rerouted::new());
        let out = built(&fake, rerouted.clone(), heartbeat::every(Version::V5));
        let carrier = udp_carrier(&out).await;
        let before = rerouted.asked();
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
        let fake = FakeTuic::spawn(script());
        let rerouted = Arc::new(Rerouted::new());
        let out = built(&fake, rerouted.clone(), heartbeat::EVERY);
        let carrier = udp_carrier(&out).await;
        carrier.send_to(b"first", &to).await.unwrap();
```

换成

```rust
        let fake = FakeTuic::spawn(script());
        let rerouted = Arc::new(Rerouted::new());
        let out = built(&fake, rerouted.clone(), heartbeat::every(Version::V5));
        let carrier = udp_carrier(&out).await;
        carrier.send_to(b"first", &to).await.unwrap();
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
        let associations: BTreeSet<u16> = carried(&fake).iter().map(|&(_, a)| a).collect();
```

换成

```rust
        let associations: BTreeSet<u32> = carried(&fake).iter().map(|&(_, a)| a).collect();
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
        assert!(!log.contains("payload-7c1e9a"));
    }
}
```

换成

```rust
        assert!(!log.contains("payload-7c1e9a"));
    }

    /// TUIC v4's (`tuic`): the manual's example token.
    const TOKEN: &str = "pwd";

    /// `tuic, 127.0.0.1, <the fake's port>, <params>`.
    fn v4_outbound(fake: &FakeTuic, params: &str) -> TuicOutbound {
        let port = fake.addr().port();
        outbound_with(&spec_of("tuic", port, params), port, fake)
    }

    /// The v4 outbound the fake lets in.
    fn v4_user(fake: &FakeTuic) -> TuicOutbound {
        v4_outbound(fake, &format!("token={TOKEN}"))
    }

    /// Design 6.3: v4 authenticates each connection with the BLAKE3 hash of
    /// the token, on a stream of its own — the same on every connection,
    /// unlike v5's token.
    #[tokio::test]
    async fn v4_the_authentication_carries_the_hash_of_the_token() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_user(&fake);
        let mut stream = tunnel(&out, &target(echo)).await;
        round_trip(&mut stream, b"through tuic v4").await;
        let other = v4_user(&fake);
        let mut stream = tunnel(&other, &target(echo)).await;
        round_trip(&mut stream, b"on another connection").await;
        let auths = fake.auths();
        assert_eq!(auths.len(), 2);
        for (n, auth) in auths.iter().enumerate() {
            assert_eq!((auth.connection, auth.uuid), (n, None));
            assert_eq!(auth.token, auth.expected);
            assert!(auth.accepted);
        }
        assert_eq!(auths[0].token, auths[1].token);
        let carried: Vec<usize> = fake.connects().iter().map(|c| c.connection).collect();
        assert_eq!(carried, [0, 1]);
    }

    /// Design 6.3: v4's Connect waits for the server's response, which
    /// comes once the server reached the target; the tunnel begins after it
    /// — with what a target that speaks first sends.
    #[tokio::test]
    async fn v4_a_tunnel_begins_after_the_response_with_what_the_target_sends() {
        let greeter = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = greeter.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = greeter.accept().await {
                let _ = socket.write_all(b"220 ready").await;
                let mut rest = Vec::new();
                let _ = socket.read_to_end(&mut rest).await;
            }
        });
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_user(&fake);
        let mut stream = tunnel(&out, &target(at)).await;
        let mut greeting = [0u8; 9];
        timeout(BOUND, stream.read_exact(&mut greeting))
            .await
            .expect("the greeting within the bound")
            .unwrap();
        assert_eq!(&greeting, b"220 ready");
        assert_eq!(fake.connects()[0].address, at.to_string());
    }

    /// Design 6.3: a target the server cannot reach is its `ff` response:
    /// the request fails, saying so, and the connection goes on.
    #[tokio::test]
    async fn v4_a_target_the_server_cannot_reach_fails_the_request_and_the_connection_goes_on() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_user(&fake);
        // the fake resolves no name
        let err = failure(&out, &Target::new(HostName::parse("unknown.test"), 80)).await;
        assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
        assert_eq!(
            err.to_string(),
            "tuic: the server could not connect to the target"
        );
        let mut next = tunnel(&out, &target(echo)).await;
        round_trip(&mut next, b"still up").await;
        assert_eq!(fake.connections(), 1);
        assert_eq!(fake.connects().len(), 2);
    }

    /// Design 6.3: a v4 server that does not know the token closes the
    /// connection with 0xfffffff1, before the Connect's response: the
    /// request fails with `tuic: authentication failed`. The next dials and
    /// authenticates again.
    #[tokio::test]
    async fn v4_a_wrong_token_fails_the_request_and_the_next_dials_again() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_outbound(&fake, "token=wrong");
        for _ in 0..2 {
            let err = failure(&out, &target(echo)).await;
            assert!(
                matches!(&err, OutboundError::Io(e) if e.kind() == io::ErrorKind::PermissionDenied),
                "{err:?}"
            );
            assert_eq!(err.to_string(), "tuic: authentication failed");
        }
        assert_eq!(fake.connections(), 2);
        let auths = fake.auths();
        assert_eq!(auths.len(), 2);
        assert!(auths.iter().all(|auth| !auth.accepted));
    }

    /// Design 6.3: the code a v4 server closes a connection with says what
    /// went wrong, the server's reason aside — its authentication refused,
    /// too late, or a protocol error —; any other code is told as v5's.
    #[tokio::test]
    async fn v4_the_servers_close_codes_say_what_went_wrong() {
        let echo = echo_server().await;
        for (code, reason, expected) in [
            (
                0xffff_fff1,
                "authentication failed",
                "tuic: authentication failed",
            ),
            (
                0xffff_fff2,
                "authentication timeout",
                "tuic: authentication timed out",
            ),
            (
                0xffff_fff0,
                "Unsupported TUIC version: 5",
                "tuic: protocol error",
            ),
            (0xffff_fff3, "bad command", "tuic: protocol error"),
            (
                0x2a,
                "",
                "tuic: the server closed the connection (code 0x2a)",
            ),
        ] {
            let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
            let out = v4_user(&fake);
            let mut stream = tunnel(&out, &target(echo)).await;
            round_trip(&mut stream, b"up").await;
            fake.close_connections_with(code, reason.as_bytes());
            let err = end_of(&mut stream).await.to_string();
            assert_eq!(err, expected, "{code:#x}");
        }
    }

    /// Design 3.5, 6.3: v4's Connect waits for a response, so a request
    /// that finds its connection gone before that came — the server
    /// restarted, and answers with a stateless reset — sent nothing of the
    /// caller's, and goes again, once, on a new connection. The tunnel
    /// under way on the lost connection ends.
    #[tokio::test]
    async fn v4_a_request_that_finds_the_server_restarted_goes_again_on_a_new_connection() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_user(&fake);
        let mut before = tunnel(&out, &target(echo)).await;
        round_trip(&mut before, b"before").await;
        fake.restart();
        let mut after = tunnel(&out, &target(echo)).await;
        round_trip(&mut after, b"after").await;
        assert_eq!(
            end_of(&mut before).await.to_string(),
            "quic: the server no longer knows the connection (stateless reset)"
        );
        let carried: Vec<usize> = fake.connects().iter().map(|c| c.connection).collect();
        assert_eq!(carried, [0, 1]);
        assert_eq!(fake.auths().len(), 2);
    }

    /// `opts.timeout` bounds a v4 request, its response included: a server
    /// that takes the Connect and never answers it.
    #[tokio::test]
    async fn v4_a_server_that_never_answers_the_connect_times_out() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(TuicScript {
            silent: true,
            ..TuicScript::v4(TOKEN)
        });
        let out = v4_user(&fake);
        // up already
        drop(held(&out).await);
        let opts = ConnectOpts {
            timeout: Duration::from_millis(500),
        };
        let started = Instant::now();
        let err = timeout(BOUND, out.connect_tcp(&target(echo), &opts))
            .await
            .expect("an outcome within the bound")
            .err()
            .unwrap();
        assert!(matches!(err, OutboundError::Timeout), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        eventually("the Connect", || fake.connects().len() == 1).await;
    }

    /// Design 6.3: v4's UDP goes whole in datagrams of the connection — no
    /// packet id, no fragments —, under one association id the carrier
    /// draws at random, and comes back so. A v4 server names an IPv4 source
    /// in IPv6 form (tuic 0.8.5's dual-stack socket): it reads as the IPv4
    /// address it is. Full cone.
    #[tokio::test]
    async fn v4_udp_goes_whole_in_datagrams_of_the_connection() {
        let (one, two) = (udp_echo_server().await, udp_echo_server().await);
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_user(&fake);
        assert_eq!(out.udp(), UdpSupport::Native);
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), one, b"to one").await;
        udp_round_trip(carrier.as_ref(), two, b"to two").await;
        let packets = fake.udp_packets();
        assert_eq!(packets.len(), 2);
        let association = packets[0].association;
        for (packet, to) in packets.iter().zip([one, two]) {
            assert_eq!((packet.connection, packet.association), (0, association));
            assert_eq!(
                (packet.packet, packet.fragments, packet.fragment),
                (0, 1, 0)
            );
            assert_eq!(packet.address, Some(to.to_string()));
            assert_eq!(packet.len, 6);
            assert!(!packet.on_stream);
        }
        let outside = fake.udp_outside();
        assert_eq!(outside.len(), 1, "one socket for the association");
        let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        stranger.send_to(b"unasked", outside[0]).await.unwrap();
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"unasked".to_vec(), target(stranger.local_addr().unwrap()))
        );
        assert_eq!(fake.connections(), 1);
    }

    /// Design 6.3: where the server takes no QUIC datagrams, each v4 packet
    /// goes whole on a unidirectional stream of its own — a long one too —,
    /// and the server answers on streams.
    #[tokio::test]
    async fn v4_without_datagrams_each_packet_goes_on_a_stream_of_its_own() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(TuicScript {
            no_datagrams: true,
            ..TuicScript::v4(TOKEN)
        });
        let out = v4_user(&fake);
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"on a stream").await;
        let long: Vec<u8> = (0..9000).map(|i| (i % 251) as u8).collect();
        udp_round_trip(carrier.as_ref(), echo, &long).await;
        let packets = fake.udp_packets();
        let seen: Vec<(bool, u8, usize)> = packets
            .iter()
            .map(|p| (p.on_stream, p.fragments, p.len))
            .collect();
        assert_eq!(seen, [(true, 1, 11), (true, 1, 9000)]);
        assert_eq!(packets[0].association, packets[1].association);
        let lease = held(&out).await;
        assert!(lease.connection().max_datagram_size().is_none());
        assert_eq!(fake.connections(), 1);
    }

    /// Design 6.3: as v5's, a v4 send hands its packet to the connection's
    /// writer and waits for nothing more, so that one its caller gives up
    /// on still goes whole: tuic 0.8.5 closes the connection, the tunnel
    /// beside it too, on a stream that ends within its command's header, as
    /// the fake does. Here the server holds the packet's stream unread, its
    /// window shorter than the header, until the send was given up on.
    #[tokio::test]
    async fn v4_a_send_given_up_on_never_cuts_its_packet_short() {
        let (tcp_echo, udp_echo) = (echo_server().await, udp_echo_server().await);
        let fake = FakeTuic::spawn(TuicScript {
            no_datagrams: true,
            stream_window: Some(8),
            ..TuicScript::v4(TOKEN)
        });
        let out = v4_user(&fake);
        let mut tcp = tunnel(&out, &target(tcp_echo)).await;
        round_trip(&mut tcp, b"up").await;
        let carrier = udp_carrier(&out).await;
        let to = target(udp_echo);
        fake.hold_streams();
        let held = || fake.held_streams() == 1;
        // given up on once the server holds its stream, if not done by then
        tokio::select! {
            sent = carrier.send_to(b"whole", &to) => sent.unwrap(),
            () = eventually("the packet's stream held", held) => {}
        }
        eventually("the packet's stream held", held).await;
        fake.release_streams();
        assert_eq!(udp_answer(carrier.as_ref()).await, (b"whole".to_vec(), to));
        round_trip(&mut tcp, b"still up").await;
        assert_eq!(fake.connections(), 1);
    }

    /// Design 6.3: v4 cannot split a packet. One that no datagram of the
    /// connection holds is refused — `InvalidInput`, which loses that packet
    /// alone: the engine drops it, as any its outbound cannot frame —, and
    /// nothing goes out; the carrier goes on. Without datagrams, the same
    /// packet goes on a stream.
    #[tokio::test]
    async fn v4_a_packet_too_long_for_a_datagram_is_refused_and_nothing_goes_out() {
        // past what any datagram of the connection holds: path MTU discovery
        // stops at 1452 bytes
        let long = vec![7u8; 2000];
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_user(&fake);
        let carrier = udp_carrier(&out).await;
        let err = carrier.send_to(&long, &target(echo)).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(
            err.to_string().starts_with("tuic: a datagram longer than "),
            "{err}"
        );
        assert!(fake.udp_packets().is_empty());
        udp_round_trip(carrier.as_ref(), echo, &[1u8; 1000]).await;
        let fake = FakeTuic::spawn(TuicScript {
            no_datagrams: true,
            ..TuicScript::v4(TOKEN)
        });
        let out = v4_user(&fake);
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, &long).await;
    }

    /// Design 6.3: a v4 carrier gone dissociates its association — `04 03
    /// ‖ ASSOC_ID`, its 32 bits — on the connection it was on; the next
    /// carrier is an association of its own, with a socket of its own.
    #[tokio::test]
    async fn v4_a_carrier_gone_dissociates_its_association() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_user(&fake);
        let lease = held(&out).await;
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"here").await;
        let association = fake.udp_packets()[0].association;
        assert_eq!(lease.state().udp.associations(), [association]);
        drop(carrier);
        assert!(lease.state().udp.associations().is_empty());
        eventually("the Dissociate", || {
            fake.dissociates()
                == [RecordedDissociate {
                    connection: 0,
                    association,
                }]
        })
        .await;
        let next = udp_carrier(&out).await;
        udp_round_trip(next.as_ref(), echo, b"next").await;
        assert_eq!(fake.udp_outside().len(), 2);
        assert_eq!(fake.connections(), 1);
    }

    /// Design 6.3: v4's heartbeat — `04 04` on a stream of its own; a v4
    /// server closes the connection on one in a datagram, as the fake does
    /// — goes while a relay is under way, and not while none is, whether the
    /// server takes QUIC datagrams or not. Its interval is cut short here
    /// (10 seconds otherwise).
    #[tokio::test]
    async fn v4_heartbeats_go_on_streams_while_a_relay_is_under_way() {
        const EVERY: Duration = Duration::from_millis(100);
        let (tcp_echo, udp_echo) = (echo_server().await, udp_echo_server().await);
        for no_datagrams in [false, true] {
            let fake = FakeTuic::spawn(TuicScript {
                no_datagrams,
                ..TuicScript::v4(TOKEN)
            });
            let spec = spec_of("tuic", fake.addr().port(), &format!("token={TOKEN}"));
            let out = built_from(&spec, &fake, direct(), EVERY);
            // up, and nothing relayed
            let lease = held(&out).await;
            assert!(lease.state().beats());
            no_heartbeat(&fake, EVERY).await;
            assert_eq!(fake.heartbeats(), 0);
            let mut tcp = tunnel(&out, &target(tcp_echo)).await;
            round_trip(&mut tcp, b"tcp").await;
            eventually("heartbeats with a tunnel", || fake.heartbeats() >= 2).await;
            drop(tcp);
            no_heartbeat(&fake, EVERY).await;
            let before = fake.heartbeats();
            let carrier = udp_carrier(&out).await;
            udp_round_trip(carrier.as_ref(), udp_echo, b"udp").await;
            eventually("heartbeats with a carrier", || {
                fake.heartbeats() >= before + 2
            })
            .await;
            drop(carrier);
            assert_eq!(lease.state().relays(), 0);
            // none came in a datagram: the server kept the connection
            assert!(lease.connection().close_reason().is_none());
            assert_eq!(fake.connections(), 1);
        }
    }

    /// Design 6.3: a v4 carrier whose connection ended goes on, on the
    /// connection that replaces it — whether the server closed it or lost
    /// it in a restart —, under an id drawn anew there.
    #[tokio::test]
    async fn v4_a_carrier_moves_to_a_new_connection_after_a_close_or_a_restart() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_user(&fake);
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"first").await;
        fake.close_connections_with(0xffff_fff0, b"protocol error");
        until_answered(carrier.as_ref(), echo, b"after a close").await;
        fake.restart();
        until_answered(carrier.as_ref(), echo, b"after a restart").await;
        let connections: BTreeSet<usize> =
            fake.udp_packets().iter().map(|p| p.connection).collect();
        assert_eq!(connections, BTreeSet::from([0, 1, 2]));
        assert_eq!(fake.auths().len(), 3);
        // the server lost the old connections: nothing to dissociate there
        assert!(fake.dissociates().is_empty());
    }

    /// Design 6.3: as v5's, a v4 carrier comes up before the server's
    /// verdict on the token; the send that finds the connection closed for
    /// a token it did not know fails, once, and the next binds anew, on a
    /// new connection.
    #[tokio::test]
    async fn v4_a_wrong_token_fails_a_send_once_and_the_next_binds_anew() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_outbound(&fake, "token=wrong");
        let carrier = udp_carrier(&out).await;
        let deadline = Instant::now() + BOUND;
        let err = loop {
            assert!(Instant::now() < deadline, "no send failed");
            match carrier.send_to(b"x", &target(echo)).await {
                Ok(()) => tokio::time::sleep(Duration::from_millis(5)).await,
                Err(e) => break e,
            }
        };
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(err.to_string(), "tuic: authentication failed");
        carrier.send_to(b"y", &target(echo)).await.unwrap();
        eventually("the next authentication", || fake.auths().len() == 2).await;
        assert!(fake.udp_packets().is_empty());
    }

    /// The token, and its hash — which lets a client in as the token does
    /// —, go to the server and nowhere else: not into the error of a refused
    /// authentication, nor into a build error, nor into a log line at any
    /// level.
    #[tokio::test]
    async fn v4_the_token_never_shows() {
        const SECRET: &str = "s3cret-tuic-v4";
        // computed apart, with the BLAKE3 from the specification that the
        // known answers of `a_v4_token_is_the_blake3_hash_of_the_token` come
        // from
        const HASH: &str = "557b8d354fae3c1f473c6f2d211cb1d827252b2f2473de4ad3425eb33b3cb1f9";
        let (log, _logging, _second) = Log::capture();
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_outbound(&fake, &format!("token={SECRET}"));
        let err = failure(&out, &target(echo)).await;
        assert_eq!(err.to_string(), "tuic: authentication failed");
        let sent: String = fake.auths()[0]
            .token
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(sent, HASH);
        let shown = format!("{err} {err:?}");
        for secret in [SECRET, HASH] {
            assert!(!shown.contains(secret), "{shown}");
        }
        let mut spec = spec_of("tuic", 443, &format!("token={SECRET}"));
        spec.version = TuicVersion::V4 { token: "".into() };
        let err = TuicOutbound::new(
            "T",
            Target::new(HostName::parse("127.0.0.1"), 443),
            &spec,
            Tristate::Auto,
            &[],
            fake.roots(),
            direct(),
        )
        .map(|_| ())
        .unwrap_err();
        assert_eq!(err.message, "`token` is empty");
        let log = log.text();
        assert!(!log.is_empty());
        for secret in [SECRET, HASH] {
            assert!(!log.contains(secret));
        }
    }
}
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
    use quinn::ApplicationClose;

    #[test]
```

换成

```rust
    use quinn::ApplicationClose;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
        let command = command(&uuid, &[0xa5; 32]);
```

换成

```rust
        let command = command_v5(&uuid, &[0xa5; 32]);
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
        assert_eq!(command[18..], [0xa5; 32]);
```

换成

```rust
        assert_eq!(command[18..], [0xa5; 32]);
    }

    /// Design 6.3: v4's token is the BLAKE3 hash of the token's bytes as
    /// written, against hashes computed with a BLAKE3 written in Python from
    /// its specification (checked against the official test vectors of
    /// inputs 0, 1, 3 and 1023 bytes long); the command is `04 00 ‖ TOKEN`.
    #[test]
    fn a_v4_token_is_the_blake3_hash_of_the_token() {
        for (token, hash) in [
            (
                "",
                "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
            ),
            (
                "abc",
                "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85",
            ),
            // the manual's example
            (
                "pwd",
                "4d013614663c2a826d5388d8766d0af9aec887a90f0289ffe333b0a6979f9d59",
            ),
        ] {
            assert_eq!(hex(&token_hash(token)), hash, "{token:?}");
        }
        let command = command_v4(&[0xa5; 32]);
        assert_eq!(command[..2], [0x04, 0x00]);
        assert_eq!(command[2..], [0xa5; 32]);
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
                "tuic: the server closed the connection (code 0x2a: goaway)",
            ),
            (
```

换成

```rust
                "tuic: the server closed the connection (code 0x2a: goaway)",
            ),
            // v4's codes mean nothing to v5
            (
                false,
                closed(0xffff_fff1, b"authentication failed"),
                ConnectionAborted,
                "tuic: the server closed the connection (code 0xfffffff1: authentication failed)",
            ),
            (
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
            let end = ended(&error, relayed);
            assert_eq!((end.kind(), end.to_string().as_str()), (kind, text));
            assert_eq!(refused(&error, relayed), kind == PermissionDenied);
```

换成

```rust
            let end = ended(Version::V5, &error, relayed);
            assert_eq!((end.kind(), end.to_string().as_str()), (kind, text));
            assert_eq!(
                refused(Version::V5, &error, relayed),
                kind == PermissionDenied
            );
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
            read_error(ReadError::ConnectionLost(closed(0, b"")), false).to_string(),
```

换成

```rust
            read_error(
                Version::V5,
                ReadError::ConnectionLost(closed(0, b"")),
                false
            )
            .to_string(),
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
            write_error(WriteError::ConnectionLost(closed(0, b"")), true).to_string(),
```

换成

```rust
            write_error(
                Version::V5,
                WriteError::ConnectionLost(closed(0, b"")),
                true
            )
            .to_string(),
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
            read_error(ReadError::Reset(VarInt::from_u32(3)), false).to_string(),
```

换成

```rust
            read_error(Version::V5, ReadError::Reset(VarInt::from_u32(3)), false).to_string(),
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
            write_error(WriteError::Stopped(VarInt::from_u32(0)), false).to_string(),
```

换成

```rust
            write_error(Version::V5, WriteError::Stopped(VarInt::from_u32(0)), false).to_string(),
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
            ended(&long, false).to_string(),
```

换成

```rust
            ended(Version::V5, &long, false).to_string(),
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
                "x".repeat(REASON_MAX)
            )
        );
    }
}

```

换成

```rust
                "x".repeat(REASON_MAX)
            )
        );
    }

    /// Design 6.3: v4's codes tell a refused authentication — the token,
    /// or its time — and a protocol error, whatever the server's reason and
    /// whether it relayed anything; any other close says its code, code 0
    /// with no reason too (no refusal in v4).
    #[test]
    fn the_end_of_a_v4_connection_reads_as_its_codes_tell_it() {
        use io::ErrorKind::*;
        for (error, kind, text) in [
            (
                closed(0xffff_fff1, b"authentication failed"),
                PermissionDenied,
                "tuic: authentication failed",
            ),
            (
                closed(0xffff_fff2, b"authentication timeout"),
                PermissionDenied,
                "tuic: authentication timed out",
            ),
            (
                closed(0xffff_fff0, b"Unsupported TUIC version: 5"),
                ConnectionAborted,
                "tuic: protocol error",
            ),
            (
                closed(0xffff_fff3, b"bad command"),
                ConnectionAborted,
                "tuic: protocol error",
            ),
            (
                closed(0, b""),
                ConnectionAborted,
                "tuic: the server closed the connection (code 0x0)",
            ),
            (
                closed(0x2a, b"bye"),
                ConnectionAborted,
                "tuic: the server closed the connection (code 0x2a: bye)",
            ),
            (
                ConnectionError::Reset,
                ConnectionReset,
                "quic: the server no longer knows the connection (stateless reset)",
            ),
        ] {
            for relayed in [false, true] {
                let end = ended(Version::V4, &error, relayed);
                assert_eq!((end.kind(), end.to_string().as_str()), (kind, text));
                assert_eq!(
                    refused(Version::V4, &error, relayed),
                    kind == PermissionDenied
                );
            }
        }
        // as the streams on the connection tell it
        let refusal = || closed(0xffff_fff1, b"authentication failed");
        assert_eq!(
            read_error(Version::V4, ReadError::ConnectionLost(refusal()), true).to_string(),
            "tuic: authentication failed"
        );
        assert_eq!(
            write_error(Version::V4, WriteError::ConnectionLost(refusal()), false).to_string(),
            "tuic: authentication failed"
        );
    }
}

```

`crates/rurge-proto-quic/src/tuic/addr.rs`——把

```rust
        assert_eq!(get(&[NONE, 0x1f, 0x90]), Some((None, 1)));
    }

    #[test]
    fn an_address_that_is_none_a_server_writes_reads_as_nothing() {
```

换成

```rust
        assert_eq!(get(&[NONE, 0x1f, 0x90]), Some((None, 1)));
    }

    /// An IPv4 address in IPv6 form names the IPv4 address — tuic 0.8.5's
    /// server names an IPv4 source so —; any other IPv6 address is itself.
    #[test]
    fn an_ipv4_address_in_ipv6_form_reads_as_the_ipv4_address() {
        for (v6, host) in [
            ("::ffff:192.0.2.1", "192.0.2.1"),
            // compatible, not mapped: no IPv4 address
            ("::192.0.2.1", "::192.0.2.1"),
            ("2001:db8::1", "2001:db8::1"),
        ] {
            let mut data = vec![IPV6];
            data.extend_from_slice(&v6.parse::<Ipv6Addr>().unwrap().octets());
            data.extend_from_slice(&[0, 53]);
            let source = Target::new(HostName::parse(host), 53);
            assert_eq!(get(&data), Some((Some(source), 19)), "{v6}");
        }
    }

    #[test]
    fn an_address_that_is_none_a_server_writes_reads_as_nothing() {
```

`crates/rurge-proto-quic/src/tuic/tcp.rs`——把

```rust
            request(&target).unwrap(),
            [0x05, 0x01, 0x01, 192, 0, 2, 1, 0x01, 0xbb]
```

换成

```rust
            request(Version::V5, &target).unwrap(),
            [0x05, 0x01, 0x01, 192, 0, 2, 1, 0x01, 0xbb]
        );
        // v4's: the same, but for its version
        assert_eq!(
            request(Version::V4, &target).unwrap(),
            [0x04, 0x01, 0x01, 192, 0, 2, 1, 0x01, 0xbb]
```

`crates/rurge-proto-quic/src/tuic/tcp.rs`——把

```rust
        assert_eq!(request(&target).unwrap(), expected);
        let unsendable = Target::new(HostName::Domain("a@b.test".into()), 80);
        let err = request(&unsendable).unwrap_err();
```

换成

```rust
        assert_eq!(request(Version::V5, &target).unwrap(), expected);
        let unsendable = Target::new(HostName::Domain("a@b.test".into()), 80);
        let err = request(Version::V4, &unsendable).unwrap_err();
```

`crates/rurge-proto-quic/src/tuic/tcp.rs`——把

```rust
            "tuic: the host name cannot be sent to the server"
        );
    }
}

```

换成

```rust
            "tuic: the host name cannot be sent to the server"
        );
    }

    /// Design 6.3: v4's response `04 ff 00` lets the stream through, `04 ff
    /// ff` is the server's failure to reach the target; anything else is
    /// no response.
    #[test]
    fn a_v4_response_lets_the_stream_through_or_says_why_not() {
        assert!(reply([0x04, 0xff, 0x00]).is_ok());
        let err = reply([0x04, 0xff, 0xff]).unwrap_err();
        assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
        assert_eq!(
            err.to_string(),
            "tuic: the server could not connect to the target"
        );
        for malformed in [
            [0x04, 0xff, 0x01],
            [0x05, 0xff, 0x00],
            [0x04, 0x01, 0x00],
            [0x00, 0x00, 0x00],
        ] {
            assert_eq!(
                reply(malformed).unwrap_err().to_string(),
                "tuic: the server's response is malformed",
                "{malformed:?}"
            );
        }
    }
}

```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
    use rurge_config::HostName;
```

换成

```rust
    use rurge_config::HostName;
    use std::collections::BTreeSet;
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
    fn parse(data: &[u8]) -> Option<Message> {
        Message::parse(Bytes::copy_from_slice(data))
```

换成

```rust
    /// `data` as a v5 server's packet.
    fn parse(data: &[u8]) -> Option<Message> {
        Message::parse(Version::V5, Bytes::copy_from_slice(data))
    }

    /// `data` as a v4 server's packet.
    fn parse_v4(data: &[u8]) -> Option<Message> {
        Message::parse(Version::V4, Bytes::copy_from_slice(data))
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
    fn ids(message: &Message) -> (u16, u16, u8, u8) {
```

换成

```rust
    fn ids(message: &Message) -> (u32, u16, u8, u8) {
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
        let mut associations = Associations::default();
```

换成

```rust
        let mut associations = Associations::new(Version::V5);
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
        assert_eq!(associations.join(&inbox), Some(u16::MAX));
```

换成

```rust
        assert_eq!(associations.join(&inbox), Some(u32::from(u16::MAX)));
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
        assert_eq!(associations.join(&inbox), None);
    }
}
```

换成

```rust
        assert_eq!(associations.join(&inbox), None);
    }

    /// Design 6.3: v4's ids are drawn at random from all 32 bits, as tuic
    /// 0.8.5's client draws them, never one in use.
    #[test]
    fn v4_association_ids_are_drawn_at_random_past_the_ones_in_use() {
        let mut associations = Associations::new(Version::V4);
        let (inbox, _rx) = mpsc::channel(1);
        let ids: BTreeSet<u32> = (0..1000)
            .map(|_| associations.join(&inbox).unwrap())
            .collect();
        assert_eq!(ids.len(), 1000);
        assert_eq!(associations.by_id.len(), 1000);
        // a thousand below 65536 would be counted, not drawn
        assert!(ids.iter().any(|&id| id > u32::from(u16::MAX)), "{ids:?}");
    }

    /// v4's Packet (protocol README): `04 02 ‖ ASSOC_ID ‖ LEN ‖ ADDR ‖
    /// payload`, its fields big endian, LEN the payload's length — in one
    /// datagram when it fits, else not at all: v4 has no fragments.
    #[test]
    fn a_v4_packet_goes_whole_or_not_at_all() {
        let mut expected = vec![0x04, 0x02, 0x12, 0x34, 0x56, 0x78, 0, 5];
        expected.extend_from_slice(&ADDR);
        expected.extend_from_slice(b"query");
        assert_eq!(
            datagram_v4(0x1234_5678, &ADDR, b"query", 1200).unwrap(),
            expected
        );
        assert_eq!(encode_v4(0x1234_5678, &ADDR, b"query"), expected);
        // exactly as long as the limit
        let room = 1200 - HEADER_V4 - ADDR.len();
        let whole = datagram_v4(1, &ADDR, &vec![7u8; room], 1200).unwrap();
        assert_eq!(whole.len(), 1200);
        let err = datagram_v4(1, &ADDR, &vec![7u8; room + 1], 1200).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            format!("tuic: a datagram longer than {room} bytes")
        );
        // an address that leaves no room
        let err = datagram_v4(1, &ADDR, b"x", HEADER_V4 + ADDR.len()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            "tuic: the host name is too long for a datagram"
        );
    }

    /// A v4 packet is read whole, its LEN exactly what follows the address:
    /// fragment 0 of 1, with no packet id.
    #[test]
    fn a_v4_packet_is_read_whole_with_its_length_exactly() {
        let mut data = vec![0x04, 0x02, 0xde, 0xad, 0xbe, 0xef, 0, 7];
        data.extend_from_slice(&ADDR);
        data.extend_from_slice(b"payload");
        let message = parse_v4(&data).unwrap();
        assert_eq!(ids(&message), (0xdead_beef, 0, 1, 0));
        assert_eq!(message.source, Some(source()));
        assert_eq!(&message.payload[..], b"payload");
        let mut short = data.clone();
        short.pop();
        let mut long = data.clone();
        long.push(0);
        let mut dissociate = data.clone();
        dissociate[1] = 0x03;
        let mut v5 = vec![0x05, 0x02, 0, 7, 0x12, 0x34, 1, 0, 0, 7];
        v5.extend_from_slice(&ADDR);
        v5.extend_from_slice(b"payload");
        for bad in [
            &data[..7],
            // LEN says otherwise
            &short[..],
            &long[..],
            // another command, another version
            &dissociate[..],
            &v5[..],
            // an address cut short
            &data[..10],
        ] {
            assert!(parse_v4(bad).is_none(), "{bad:?}");
        }
        // and the other way round
        assert!(parse(&data).is_none());
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto-quic --lib`
Expected: FAIL——v4 的命令、版本区分与新的假服务端接口由 Step 3 引入，编译不过：

```text
error[E0412]: cannot find type `Ipv6Addr` in this scope
error[E0433]: failed to resolve: use of undeclared type `Version`
error[E0425]: cannot find value `HEADER_V4` in this scope
error[E0425]: cannot find function `every` in module `heartbeat`
error[E0425]: cannot find function `command_v5` in this scope
error[E0425]: cannot find function `token_hash` in this scope
error[E0425]: cannot find function `command_v4` in this scope
error[E0061]: this function takes 2 arguments but 3 arguments were supplied
```

- [ ] **Step 3: 实现**

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
//! The authentication of every new connection (TUIC v5 SPEC,
//! "Authenticate"; phase 2 M7 design 6.2): a unidirectional stream of its
//! own carrying `05 00 ‖ UUID ‖ TOKEN` — the user's UUID, its 16 bytes as
//! they are, and the 32 bytes the connection's TLS session exports with the
//! UUID's bytes as the label and the password as the context (RFC 5705, RFC
//! 8446 7.5), which only the two ends of that session can work out.
//!
//! - The server answers nothing, and holds the commands that come before it
//!   checked the token (SPEC): the command goes out as the connection comes
//!   up, and the connection's requests follow without waiting — UDP
//!   packets too, which sing-box leaves in QUIC's queue until then.
//! - A server that refuses it closes the connection — tuic-server and
//!   sing-box with code 0 and no reason —, which the connection's tunnels
//!   and carriers then tell (`Authenticated::ended`).

use super::VERSION;
```

换成

```rust
//! The authentication of every new connection (phase 2 M7 design 6.2,
//! 6.3): a unidirectional stream of its own carrying one command —
//!
//! - v5 (SPEC, "Authenticate"): `05 00 ‖ UUID ‖ TOKEN` — the user's UUID,
//!   its 16 bytes as they are, and the 32 bytes the connection's TLS
//!   session exports with the UUID's bytes as the label and the password as
//!   the context (RFC 5705, RFC 8446 7.5), which only the two ends of that
//!   session can work out;
//! - v4 (tuic 0.8.5's protocol README, "Authenticate"): `04 00 ‖ TOKEN` —
//!   the BLAKE3 hash of the token as written, as the server hashes the
//!   tokens it lets in: the same on every connection.
//!
//! - The server answers nothing, and holds the commands that come before it
//!   checked the token (both protocols): the command goes out as the
//!   connection comes up, and the connection's requests follow without
//!   waiting — UDP packets too, which sing-box leaves in QUIC's queue until
//!   then.
//! - A server that refuses it closes the connection — v5's (tuic-server and
//!   sing-box) with code 0 and no reason, v4's with a code of the protocol's
//!   —, which the connection's tunnels and carriers then tell
//!   (`Authenticated::ended`).

use super::Version;
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
use quinn::{Connection, ConnectionError, ReadError, VarInt, WriteError};
```

换成

```rust
use quinn::{Connection, ConnectionError, ReadError, VarInt, WriteError};
use rurge_config::spec::TuicVersion;
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
/// The code TUIC's implementations close a connection with, whatever the
/// cause — tuic-server 1.0.0, tuic-client 1.0.0 and sing-box alike: the v5
/// SPEC defines none.
const CLOSE: VarInt = VarInt::from_u32(0);
```

换成

```rust
/// The code TUIC v5's implementations close a connection with, whatever the
/// cause — tuic-server 1.0.0, tuic-client 1.0.0 and sing-box alike: the v5
/// SPEC defines none. The session closes v4's connections with it too: v4
/// gives it no meaning.
const CLOSE: VarInt = VarInt::from_u32(0);

// What a v4 server closes a connection with (protocol README, "Error
// Codes"), its error's text as the reason: the protocol's version or a
// command it cannot read, a token it does not know, an authentication that
// came too late, a command on the wrong kind of stream.
const PROTOCOL_ERROR: u64 = 0xffff_fff0;
const AUTHENTICATION_FAILED: u64 = 0xffff_fff1;
const AUTHENTICATION_TIMEOUT: u64 = 0xffff_fff2;
const BAD_COMMAND: u64 = 0xffff_fff3;
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
    uuid: [u8; 16],
    password: String,
```

换成

```rust
    credentials: Credentials,
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
/// What a connection keeps once its authentication went out: what tells
/// how it ended, its relays and their heartbeat, its UDP associations.
pub(crate) struct Authenticated {
```

换成

```rust
/// What the command carries.
enum Credentials {
    /// The token's hash: what lets a client in, as the token itself does.
    V4 { token: [u8; 32] },
    /// The user, and the password its token is exported with.
    V5 { uuid: [u8; 16], password: String },
}

/// What a connection keeps once its authentication went out: its version,
/// what tells how it ended, its relays and their heartbeat, its UDP
/// associations.
pub(crate) struct Authenticated {
    version: Version,
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
    /// Sends the heartbeat; none on a connection without QUIC datagrams.
```

换成

```rust
    /// Sends the heartbeat; none for v5 on a connection without QUIC
    /// datagrams.
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
    pub(crate) fn new(uuid: [u8; 16], password: &str, heartbeat: Duration) -> Auth {
        Auth {
            uuid,
            password: password.to_string(),
```

换成

```rust
    /// The credentials of a `tuic` (v4) or `tuic-v5` line; the heartbeat
    /// every `heartbeat`.
    pub(crate) fn new(credentials: &TuicVersion, heartbeat: Duration) -> Auth {
        let credentials = match credentials {
            TuicVersion::V4 { token } => Credentials::V4 {
                token: token_hash(token.expose()),
            },
            TuicVersion::V5 { uuid, password } => Credentials::V5 {
                uuid: *uuid.expose(),
                password: password.expose().clone(),
            },
        };
        Auth {
            credentials,
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
    /// The token of `connection`'s TLS session.
    fn token(&self, connection: &Connection) -> [u8; 32] {
        let mut token = [0u8; 32];
        connection
            .export_keying_material(&mut token, &self.uuid, self.password.as_bytes())
            .expect("a connection is up once its TLS handshake is done, which exports 32 bytes");
        token
```

换成

```rust
    fn version(&self) -> Version {
        match self.credentials {
            Credentials::V4 { .. } => Version::V4,
            Credentials::V5 { .. } => Version::V5,
        }
    }

    /// The command for `connection`: v5's token is its TLS session's.
    fn command(&self, connection: &Connection) -> Vec<u8> {
        match &self.credentials {
            Credentials::V4 { token } => command_v4(token).to_vec(),
            Credentials::V5 { uuid, password } => {
                let mut token = [0u8; 32];
                connection
                    .export_keying_material(&mut token, uuid, password.as_bytes())
                    .expect(
                        "a connection is up once its TLS handshake is done, which exports 32 bytes",
                    );
                command_v5(uuid, &token).to_vec()
            }
        }
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
/// `05 00 ‖ UUID ‖ TOKEN`.
fn command(uuid: &[u8; 16], token: &[u8; 32]) -> [u8; 50] {
    let mut command = [0u8; 50];
    command[..2].copy_from_slice(&[VERSION, AUTHENTICATE]);
```

换成

```rust
/// v4's token on the wire: the BLAKE3 hash of `token`'s bytes, as tuic
/// 0.8.5's server hashes the tokens it lets in.
fn token_hash(token: &str) -> [u8; 32] {
    *blake3::hash(token.as_bytes()).as_bytes()
}

/// `04 00 ‖ TOKEN`.
fn command_v4(token: &[u8; 32]) -> [u8; 34] {
    let mut command = [0u8; 34];
    command[..2].copy_from_slice(&[Version::V4.byte(), AUTHENTICATE]);
    command[2..].copy_from_slice(token);
    command
}

/// `05 00 ‖ UUID ‖ TOKEN`.
fn command_v5(uuid: &[u8; 16], token: &[u8; 32]) -> [u8; 50] {
    let mut command = [0u8; 50];
    command[..2].copy_from_slice(&[Version::V5.byte(), AUTHENTICATE]);
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
            let command = command(&self.uuid, &self.token(connection));
            // nothing relayed on a connection that just came up
            let mut stream = connection.open_uni().await.map_err(|e| ended(&e, false))?;
```

换成

```rust
            let version = self.version();
            let command = self.command(connection);
            // nothing relayed on a connection that just came up
            let mut stream = connection
                .open_uni()
                .await
                .map_err(|e| ended(version, &e, false))?;
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
                .map_err(|e| write_error(e, false))?;
            // fails only for a stream finished or reset already
            let _ = stream.finish();
            Ok(Authenticated::start(connection, self.heartbeat))
```

换成

```rust
                .map_err(|e| write_error(version, e, false))?;
            // fails only for a stream finished or reset already
            let _ = stream.finish();
            Ok(Authenticated::start(connection, version, self.heartbeat))
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
    /// The state of `connection`, its tasks started: the one that reads
    /// the server's packets, and the heartbeat where QUIC datagrams go.
    fn start(connection: &Connection, every: Duration) -> Authenticated {
        let relayed = Arc::new(AtomicBool::new(false));
        let relays = Relays::new();
        let beating = connection
            .max_datagram_size()
            .map(|_| heartbeat::start(connection, &relays, every));
        Authenticated {
            udp: Udp::start(connection, relayed.clone()),
            relayed,
            relays,
            _heartbeat: beating,
        }
```

换成

```rust
    /// The state of `connection`, its tasks started: the one that reads the
    /// server's packets, and the heartbeat where it can go.
    fn start(connection: &Connection, version: Version, every: Duration) -> Authenticated {
        let relayed = Arc::new(AtomicBool::new(false));
        let relays = Relays::new();
        Authenticated {
            version,
            udp: Udp::start(connection, version, relayed.clone()),
            _heartbeat: heartbeat::start(connection, &relays, version, every),
            relayed,
            relays,
        }
    }

    /// The TUIC the connection speaks.
    pub(super) fn version(&self) -> Version {
        self.version
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
        ended(e, self.relayed.load(Ordering::Relaxed))
```

换成

```rust
        ended(self.version, e, self.relayed.load(Ordering::Relaxed))
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
        refused(e, self.relayed.load(Ordering::Relaxed))
```

换成

```rust
        refused(self.version, e, self.relayed.load(Ordering::Relaxed))
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
        read_error(e, self.relayed.load(Ordering::Relaxed))
```

换成

```rust
        read_error(self.version, e, self.relayed.load(Ordering::Relaxed))
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
        write_error(e, self.relayed.load(Ordering::Relaxed))
```

换成

```rust
        write_error(self.version, e, self.relayed.load(Ordering::Relaxed))
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
/// Whether a connection that ended so was refused its authentication: code
/// 0 and no reason before the server relayed anything (`relayed`) is how
```

换成

```rust
/// Whether a connection that ended so was refused its authentication. v4
/// says so in a code: the token, or its time. v5 defines none: code 0 and
/// no reason before the server relayed anything (`relayed`) is how
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
fn refused(e: &ConnectionError, relayed: bool) -> bool {
    matches!(e, ConnectionError::ApplicationClosed(close)
        if close.error_code == CLOSE && close.reason.is_empty() && !relayed)
}

/// How a connection ended (design 3.6, 6.2), `relayed` whether the server
/// relayed anything on it. The server's own close is TUIC's, and v5 defines
/// no codes: `tuic: the server closed the connection (code …)`, its reason
/// cleaned and cut short; a refused authentication (`refused`) is `tuic:
/// authentication failed`. Any other end as QUIC tells it.
fn ended(e: &ConnectionError, relayed: bool) -> io::Error {
```

换成

```rust
fn refused(version: Version, e: &ConnectionError, relayed: bool) -> bool {
    let ConnectionError::ApplicationClosed(close) = e else {
        return false;
    };
    match version {
        Version::V4 => matches!(
            close.error_code.into_inner(),
            AUTHENTICATION_FAILED | AUTHENTICATION_TIMEOUT
        ),
        Version::V5 => close.error_code == CLOSE && close.reason.is_empty() && !relayed,
    }
}

/// How a connection ended (design 3.6, 6.2, 6.3), `relayed` whether the
/// server relayed anything on it. The server's own close is TUIC's: a
/// refused authentication (`refused`) says so, and so do v4's protocol
/// errors, the server's reason aside; any other close is `tuic: the server
/// closed the connection (code …)`, its reason cleaned and cut short. Any
/// other end as QUIC tells it.
fn ended(version: Version, e: &ConnectionError, relayed: bool) -> io::Error {
    use io::ErrorKind::{ConnectionAborted, PermissionDenied};
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
    if refused(e, relayed) {
        return io::Error::new(
            io::ErrorKind::PermissionDenied,
            "tuic: authentication failed",
        );
    }
    let code = close.error_code.into_inner();
    let reason = untrusted_text(&String::from_utf8_lossy(&close.reason), REASON_MAX);
    let text = if reason.is_empty() {
        format!("tuic: the server closed the connection (code {code:#x})")
    } else {
        format!("tuic: the server closed the connection (code {code:#x}: {reason})")
    };
    io::Error::new(io::ErrorKind::ConnectionAborted, text)
```

换成

```rust
    let code = close.error_code.into_inner();
    let (kind, text) = match (version, code) {
        (Version::V4, AUTHENTICATION_TIMEOUT) => {
            (PermissionDenied, "tuic: authentication timed out")
        }
        (Version::V4, PROTOCOL_ERROR | BAD_COMMAND) => (ConnectionAborted, "tuic: protocol error"),
        _ if refused(version, e, relayed) => (PermissionDenied, "tuic: authentication failed"),
        _ => {
            let reason = untrusted_text(&String::from_utf8_lossy(&close.reason), REASON_MAX);
            let text = if reason.is_empty() {
                format!("tuic: the server closed the connection (code {code:#x})")
            } else {
                format!("tuic: the server closed the connection (code {code:#x}: {reason})")
            };
            return io::Error::new(ConnectionAborted, text);
        }
    };
    io::Error::new(kind, text)
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
fn read_error(e: ReadError, relayed: bool) -> io::Error {
    match e {
        ReadError::ConnectionLost(e) => ended(&e, relayed),
```

换成

```rust
fn read_error(version: Version, e: ReadError, relayed: bool) -> io::Error {
    match e {
        ReadError::ConnectionLost(e) => ended(version, &e, relayed),
```

`crates/rurge-proto-quic/src/tuic/auth.rs`——把

```rust
fn write_error(e: WriteError, relayed: bool) -> io::Error {
    match e {
        WriteError::ConnectionLost(e) => ended(&e, relayed),
```

换成

```rust
fn write_error(version: Version, e: WriteError, relayed: bool) -> io::Error {
    match e {
        WriteError::ConnectionLost(e) => ended(version, &e, relayed),
```

`crates/rurge-proto-quic/src/tuic/addr.rs`——把

```rust
//! The address of TUIC's commands (TUIC v5 SPEC, "Address"; phase 2 M7
//! design 6.2): `u8 type ‖ address ‖ u16 port`, big endian — `0x00` a
//! domain name, its length in a byte first; `0x01` an IPv4 address; `0x02`
//! an IPv6 address; `0xff` none, with no port either: what every fragment of
//! a UDP packet but the first carries.
```

换成

```rust
//! The address of TUIC's commands (TUIC v5 SPEC, "Address"; v4's protocol
//! README, the same; phase 2 M7 design 6.2, 6.3): `u8 type ‖ address ‖ u16
//! port`, big endian — `0x00` a domain name, its length in a byte first;
//! `0x01` an IPv4 address; `0x02` an IPv6 address; in v5 only, `0xff` none,
//! with no port either: what every fragment of a UDP packet but the first
//! carries.
```

`crates/rurge-proto-quic/src/tuic/addr.rs`——把

```rust
use std::net::IpAddr;
```

换成

```rust
use std::net::{IpAddr, Ipv6Addr};
```

`crates/rurge-proto-quic/src/tuic/addr.rs`——把

```rust
/// target that was one.
```

换成

```rust
/// target that was one. An IPv4 address in IPv6 form (RFC 4291 2.5.5.2) is
/// the IPv4 address it names: tuic 0.8.5's server names an IPv4 source so,
/// as its dual-stack socket heard it.
```

`crates/rurge-proto-quic/src/tuic/addr.rs`——把

```rust
            (HostName::Ip(IpAddr::from(octets)), 16)
```

换成

```rust
            (HostName::Ip(Ipv6Addr::from(octets).to_canonical()), 16)
```

`crates/rurge-proto-quic/src/tuic/tcp.rs`——把

```rust
//! TCP over TUIC v5 (SPEC, "Connect"; phase 2 M7 design 6.2): every
//! connection a bidirectional stream of its own on the policy's QUIC
//! connection.
//!
//! - The stream begins `05 01 ‖ ADDR`, written as it opens, and is the
//!   tunnel at once: the server answers nothing. A target it cannot reach
//!   shows as the stream ending.
//! - While a tunnel lives, the connection's heartbeat goes (`heartbeat`).
//! - With no answer to wait for, a connection gone unknown to us — its
//!   server restarted — shows only once packets of the tunnel meet the
//!   server. A request goes again on a new connection (`Failure::Lost`)
//!   only when it finds its connection gone as the stream opens or as the
//!   Connect is handed to QUIC; after that the tunnel is the caller's, and
//!   ends with the connection.

use super::VERSION;
use super::addr;
use super::auth::Authenticated;
use super::heartbeat::Relaying;
use crate::session::{Failure, Lease};
use quinn::{RecvStream, SendStream, WriteError};
```

换成

```rust
//! TCP over TUIC (v5 SPEC, "Connect"; v4's protocol README, "Connect" and
//! "Response"; phase 2 M7 design 6.2, 6.3): every connection a
//! bidirectional stream of its own on the policy's QUIC connection.
//!
//! - The stream begins `VER 01 ‖ ADDR`, written as it opens.
//! - v5's server answers nothing: the stream is the tunnel at once, and a
//!   target it cannot reach shows as the stream ending. v4's answers on the
//!   stream once it tried the target, `04 ff ‖ REP` — `00` it reached it,
//!   `ff` it could not —, and nothing of the caller's goes before.
//! - While a tunnel lives, the connection's heartbeat goes (`heartbeat`).
//! - A request goes again on a new connection (`Failure::Lost`) when it
//!   finds its connection gone — its server restarted, say — while nothing
//!   of the caller's went out yet: as the stream opens or as the Connect is
//!   handed to QUIC, and in v4 until the response came. A v5 tunnel, with no
//!   response to wait for, finds a connection gone unknown to us only once
//!   its packets meet the server: by then it is the caller's, and ends with
//!   the connection.

use super::auth::Authenticated;
use super::heartbeat::Relaying;
use super::{Version, addr};
use crate::session::{Failure, Lease};
use quinn::{ReadError, ReadExactError, RecvStream, SendStream, WriteError};
```

`crates/rurge-proto-quic/src/tuic/tcp.rs`——把

```rust
const CONNECT: u8 = 0x01;

/// The Connect for `target`: refused, before anything is dialed, for a
/// target that cannot be named to the server.
pub(super) fn request(target: &Target) -> Result<Vec<u8>, OutboundError> {
    let mut request = vec![VERSION, CONNECT];
```

换成

```rust
const CONNECT: u8 = 0x01;
/// v4's answer to a Connect, `04 ff ‖ REP`, and its two replies.
const V4: u8 = Version::V4.byte();
const RESPONSE: u8 = 0xff;
const SUCCEEDED: u8 = 0x00;
const FAILED: u8 = 0xff;

/// `version`'s Connect for `target`: refused, before anything is dialed,
/// for a target that cannot be named to the server.
pub(super) fn request(version: Version, target: &Target) -> Result<Vec<u8>, OutboundError> {
    let mut request = vec![version.byte(), CONNECT];
```

`crates/rurge-proto-quic/src/tuic/tcp.rs`——把

```rust
/// Connect on a stream of its own, handed to QUIC, and the stream is the
/// caller's. A connection found gone by then is `Lost`: nothing of the
/// caller's went out yet.
pub(super) async fn open(lease: Lease<Authenticated>, request: &[u8]) -> Result<Tunnel, Failure> {
    let state = lease.state();
    let (mut send, recv) = lease
```

换成

```rust
/// Connect on a stream of its own, handed to QUIC — and in v4 the server's
/// response —, and the stream is the caller's. A connection found gone by
/// then is `Lost`: nothing of the caller's went out yet.
pub(super) async fn open(lease: Lease<Authenticated>, request: &[u8]) -> Result<Tunnel, Failure> {
    let state = lease.state();
    let (mut send, mut recv) = lease
```

`crates/rurge-proto-quic/src/tuic/tcp.rs`——把

```rust
    })?;
```

换成

```rust
    })?;
    if state.version() == Version::V4 {
        response(state, &mut recv).await?;
    }
```

`crates/rurge-proto-quic/src/tuic/tcp.rs`——把

```rust
    })
}
```

换成

```rust
    })
}

/// v4: the server's response on the stream (`reply`).
async fn response(state: &Authenticated, recv: &mut RecvStream) -> Result<(), Failure> {
    let mut response = [0u8; 3];
    recv.read_exact(&mut response).await.map_err(|e| match e {
        ReadExactError::FinishedEarly(_) => {
            Failure::Failed(proxy("the server ended the stream without a response"))
        }
        ReadExactError::ReadError(ReadError::ConnectionLost(e)) => {
            Failure::Lost(state.ended(&e).into())
        }
        ReadExactError::ReadError(e) => Failure::Failed(state.read_error(e).into()),
    })?;
    reply(response).map_err(Failure::Failed)
}

/// What v4's response says: the server reached the target, or could not.
/// Anything but `04 ff 00` and `04 ff ff` is no response.
fn reply(response: [u8; 3]) -> Result<(), OutboundError> {
    match response {
        [V4, RESPONSE, SUCCEEDED] => Ok(()),
        [V4, RESPONSE, FAILED] => Err(proxy("the server could not connect to the target")),
        _ => Err(proxy("the server's response is malformed")),
    }
}

fn proxy(text: &str) -> OutboundError {
    OutboundError::Proxy(format!("tuic: {text}"))
}
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
//! UDP over TUIC v5 (SPEC, "Packet", "Dissociate"; phase 2 M7 design 6.2):
//! each carrier — one client association on the outbound — is an
//! association of its own on the policy's connection, full cone: every
//! target through it, and answers from anyone, as the server keeps a UDP
//! socket per association.
//!
//! - A packet is `05 02 ‖ ASSOC_ID ‖ PKT_ID ‖ FRAG_TOTAL ‖ FRAG_ID ‖ SIZE ‖
//!   ADDR ‖ payload`, big endian: SIZE is the payload of this message, ADDR
//!   (`addr`) names the target going out and the source coming back.
//! - Where the server takes QUIC datagrams (`native`), a packet goes in
//!   datagrams of the connection: whole when it fits one, else in
//!   fragments, only the first naming the address — the others name none
//!   —, all under the packet's id. A carrier sends one packet at a time:
//!   Clash.Meta's first TUIC v5 server put one packet of an association
//!   together at a time, as Hysteria's does, and lost both of two whose
//!   fragments came in turn.
```

换成

```rust
//! UDP over TUIC (v5 SPEC, "Packet", "Dissociate"; v4's protocol README,
//! the same; phase 2 M7 design 6.2, 6.3): each carrier — one client
//! association on the outbound — is an association of its own on the
//! policy's connection, full cone: every target through it, and answers
//! from anyone, as the server keeps a UDP socket per association.
//!
//! - A v5 packet is `05 02 ‖ ASSOC_ID ‖ PKT_ID ‖ FRAG_TOTAL ‖ FRAG_ID ‖ SIZE
//!   ‖ ADDR ‖ payload`, a v4 one `04 02 ‖ ASSOC_ID ‖ LEN ‖ ADDR ‖ payload`,
//!   big endian: SIZE and LEN are the payload of this message, ADDR (`addr`)
//!   names the target going out and the source coming back.
//! - Where the server takes QUIC datagrams (`native`), a packet goes in
//!   datagrams of the connection: whole when it fits one; else v5 splits it
//!   into fragments, only the first naming the address — the others name
//!   none —, all under the packet's id, and v4, which cannot, refuses it as
//!   too long, as the engine drops what an outbound's framing cannot carry.
//!   A carrier sends one packet at a time: Clash.Meta's first TUIC v5
//!   server put one packet of an association together at a time, as
//!   Hysteria's does, and lost both of two whose fragments came in turn.
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
//!   association on it with it. The server answers an association the way
//!   its first packet came (SPEC); answers are read either way.
//! - Association ids count per connection from 0, past the ones in use. A
//!   carrier moves to the session's connection on its next packet once its
//!   own takes nothing new — closed, or dialled another way than the policy
//!   leads now (`Session::serves`: where the policy leads is asked at most
//!   once a second, the rest on every packet) —, under an id that
//!   connection gives it: the server takes it for a new association
//!   whatever its id.
//! - A carrier that goes, or moves, dissociates its association on the
//!   connection it leaves while that is open: `05 03 ‖ ASSOC_ID` on a
```

换成

```rust
//!   association on it with it, as tuic 0.8.5 does when the cut is in the
//!   command's header. The server answers an association the way its first
//!   packet came (v4's holds the whole connection's UDP to that way, which
//!   one connection never leaves); answers are read either way.
//! - Association ids are per connection, past the ones in use: v5's count
//!   from 0 (16 bits), v4's are drawn at random (32 bits), as the reference
//!   clients' are. A carrier moves to the session's connection on its next
//!   packet once its own takes nothing new — closed, or dialled another way
//!   than the policy leads now (`Session::serves`: where the policy leads is
//!   asked at most once a second, the rest on every packet) —, under an id
//!   that connection gives it: the server takes it for a new association
//!   whatever its id.
//! - A carrier that goes, or moves, dissociates its association on the
//!   connection it leaves while that is open: `VER 03 ‖ ASSOC_ID` on a
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
//!   — into their associations' inboxes, putting fragments together first:
//!   at most 8 packets of an association at a time, a ninth dropping the
//!   oldest. What finds no association, or a full inbox, is dropped.

use super::VERSION;
```

换成

```rust
//!   — into their associations' inboxes, putting v5's fragments together
//!   first: at most 8 packets of an association at a time, a ninth dropping
//!   the oldest. What finds no association, or a full inbox, is dropped.

use super::Version;
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
/// `VER ‖ TYPE ‖ ASSOC_ID ‖ PKT_ID ‖ FRAG_TOTAL ‖ FRAG_ID ‖ SIZE`.
const HEADER: usize = 10;
/// The longest packet a stream carries: the header, the longest address
/// (a name of 255 bytes), the longest payload.
```

换成

```rust
/// v5's `VER ‖ TYPE ‖ ASSOC_ID ‖ PKT_ID ‖ FRAG_TOTAL ‖ FRAG_ID ‖ SIZE`.
const HEADER: usize = 10;
/// v4's `VER ‖ TYPE ‖ ASSOC_ID ‖ LEN`.
const HEADER_V4: usize = 8;
/// The longest packet a stream carries: v5's header, the longer, the
/// longest address (a name of 255 bytes), the longest payload.
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
/// One Packet command: `05 02 ‖ ASSOC_ID ‖ PKT_ID ‖ FRAG_TOTAL ‖ FRAG_ID ‖
/// SIZE ‖ ADDR ‖ payload`, `address` the ADDR as `addr::put` writes it.
```

换成

```rust
/// One v5 Packet command: `05 02 ‖ ASSOC_ID ‖ PKT_ID ‖ FRAG_TOTAL ‖ FRAG_ID
/// ‖ SIZE ‖ ADDR ‖ payload`, `address` the ADDR as `addr::put` writes it.
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
    out.extend_from_slice(&[VERSION, PACKET]);
```

换成

```rust
    out.extend_from_slice(&[Version::V5.byte(), PACKET]);
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
/// A Packet as it came from the server, its SIZE exactly what follows the
/// address (as sing-box reads one).
struct Message {
    association: u16,
```

换成

```rust
/// One v4 Packet command: `04 02 ‖ ASSOC_ID ‖ LEN ‖ ADDR ‖ payload`, as
/// `encode`.
fn encode_v4(association: u32, address: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_V4 + address.len() + payload.len());
    out.extend_from_slice(&[Version::V4.byte(), PACKET]);
    out.extend_from_slice(&association.to_be_bytes());
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    out.extend_from_slice(address);
    out.extend_from_slice(payload);
    out
}

/// The one QUIC datagram, no longer than `max`, that carries `payload` to
/// `address` for v4's association `association`: refused when it does not
/// fit — v4 has no fragments.
fn datagram_v4(
    association: u32,
    address: &[u8],
    payload: &[u8],
    max: usize,
) -> io::Result<Vec<u8>> {
    let room = max.saturating_sub(HEADER_V4 + address.len());
    if payload.len() > room {
        return Err(if room == 0 {
            invalid("the host name is too long for a datagram")
        } else {
            invalid(&format!("a datagram longer than {room} bytes"))
        });
    }
    Ok(encode_v4(association, address, payload))
}

/// A Packet as it came from the server, its SIZE (v5) or LEN (v4) exactly
/// what follows the address: as sing-box reads v5's, as tuic 0.8.5's server
/// writes v4's.
struct Message {
    association: u32,
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
    /// `None` for anything but a Packet a server writes.
    fn parse(data: Bytes) -> Option<Message> {
        let header: [u8; HEADER] = data.get(..HEADER)?.try_into().ok()?;
        if header[..2] != [VERSION, PACKET] {
            return None;
        }
        let (source, len) = addr::get(&data[HEADER..])?;
        let start = HEADER + len;
        if data.len() - start != usize::from(u16::from_be_bytes([header[8], header[9]])) {
```

换成

```rust
    /// `None` for anything but a Packet a server of `version` writes. v4's
    /// has neither a packet id nor fragments: it reads as fragment 0 of 1.
    fn parse(version: Version, data: Bytes) -> Option<Message> {
        let start = match version {
            Version::V4 => HEADER_V4,
            Version::V5 => HEADER,
        };
        let header = data.get(..start)?;
        if header[..2] != [version.byte(), PACKET] {
            return None;
        }
        let be16 = |at: usize| u16::from_be_bytes([header[at], header[at + 1]]);
        let (association, packet, fragments, fragment, size) = match version {
            Version::V4 => {
                let association = u32::from_be_bytes(header[2..6].try_into().ok()?);
                (association, 0, 1, 0, be16(6))
            }
            Version::V5 => (u32::from(be16(2)), be16(4), header[6], header[7], be16(8)),
        };
        let (source, len) = addr::get(&data[start..])?;
        let start = start + len;
        if data.len() - start != usize::from(size) {
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
            association: u16::from_be_bytes([header[2], header[3]]),
            packet: u16::from_be_bytes([header[4], header[5]]),
            fragments: header[6],
            fragment: header[7],
```

换成

```rust
            association,
            packet,
            fragments,
            fragment,
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
/// A connection's associations, by id, and where the search for a free id
/// goes on.
#[derive(Default)]
struct Associations {
    by_id: HashMap<u16, Association>,
```

换成

```rust
/// A connection's associations, by id, and where the count of ids goes on
/// (`join`).
struct Associations {
    version: Version,
    by_id: HashMap<u32, Association>,
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
    /// Takes `inbox` in under the next id free, and says which: `None` when
    /// all 65536 are in use.
    fn join(&mut self, inbox: &mpsc::Sender<Answer>) -> Option<u16> {
        for _ in 0..=u16::MAX {
            let id = self.next;
            self.next = id.wrapping_add(1);
```

换成

```rust
    fn new(version: Version) -> Associations {
        Associations {
            version,
            by_id: HashMap::new(),
            next: 0,
        }
    }

    /// Takes `inbox` in under an id free, and says which: v5's count from 0
    /// (as sing-box's client counts them), v4's are drawn at random (as tuic
    /// 0.8.5's client draws them) — counted as v5's without a random source.
    /// `None` when all v5's 65536 are in use.
    fn join(&mut self, inbox: &mpsc::Sender<Answer>) -> Option<u32> {
        for _ in 0..=u16::MAX {
            let counted = self.next;
            self.next = counted.wrapping_add(1);
            let id = match self.version {
                Version::V4 => getrandom::u32().unwrap_or(u32::from(counted)),
                Version::V5 => u32::from(counted),
            };
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
    dissociate: mpsc::UnboundedSender<u16>,
```

换成

```rust
    dissociate: mpsc::UnboundedSender<u32>,
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
    /// The reader marks `relayed` for every packet the server relays.
    pub(super) fn start(connection: &Connection, relayed: Arc<AtomicBool>) -> Udp {
        let associations = Arc::<Mutex<Associations>>::default();
```

换成

```rust
    /// The reader reads and writes `version`'s commands, and marks
    /// `relayed` for every packet the server relays.
    pub(super) fn start(
        connection: &Connection,
        version: Version,
        relayed: Arc<AtomicBool>,
    ) -> Udp {
        let associations = Arc::new(Mutex::new(Associations::new(version)));
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
            connection.clone(),
```

换成

```rust
            connection.clone(),
            version,
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
    /// Takes `inbox` in under the next id free on the connection
    /// (`Associations::join`).
    fn join(&self, inbox: &mpsc::Sender<Answer>) -> Option<u16> {
```

换成

```rust
    /// Takes `inbox` in under an id free on the connection
    /// (`Associations::join`).
    fn join(&self, inbox: &mpsc::Sender<Answer>) -> Option<u32> {
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
    fn leave(&self, id: u16, dissociate: bool) {
```

换成

```rust
    fn leave(&self, id: u32, dissociate: bool) {
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
    pub(super) fn associations(&self) -> Vec<u16> {
        let associations = self.associations.lock().expect("associations");
        let mut ids: Vec<u16> = associations.by_id.keys().copied().collect();
```

换成

```rust
    pub(super) fn associations(&self) -> Vec<u32> {
        let associations = self.associations.lock().expect("associations");
        let mut ids: Vec<u32> = associations.by_id.keys().copied().collect();
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
/// Reads the server's packets on `connection` — its datagrams, and its
/// unidirectional streams, each read by a task of its own — into their
/// associations' inboxes, and sends each Dissociate asked for on a stream
/// of its own, until the connection ends: what came before its end
```

换成

```rust
/// Reads the server's `version` packets on `connection` — its datagrams,
/// and its unidirectional streams, each read by a task of its own — into
/// their associations' inboxes, and sends each Dissociate asked for on a
/// stream of its own, until the connection ends: what came before its end
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
    associations: Arc<Mutex<Associations>>,
    relayed: Arc<AtomicBool>,
    mut dissociations: mpsc::UnboundedReceiver<u16>,
```

换成

```rust
    version: Version,
    associations: Arc<Mutex<Associations>>,
    relayed: Arc<AtomicBool>,
    mut dissociations: mpsc::UnboundedReceiver<u32>,
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
                Ok(datagram) => deliver(&associations, &relayed, datagram),
```

换成

```rust
                Ok(datagram) => deliver(version, &associations, &relayed, datagram),
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
                    deliver(&associations, &relayed, message);
```

换成

```rust
                    deliver(version, &associations, &relayed, message);
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
                dissociating.spawn(dissociate(connection.clone(), id));
```

换成

```rust
                dissociating.spawn(dissociate(connection.clone(), version, id));
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
/// `05 03 ‖ ASSOC_ID` on a unidirectional stream of its own, finished: the
/// server frees the association's socket.
async fn dissociate(connection: Connection, id: u16) {
    let [high, low] = id.to_be_bytes();
```

换成

```rust
/// `VER 03 ‖ ASSOC_ID` on a unidirectional stream of its own, finished —
/// exactly that: sing-box closes the connection on a longer one —: the
/// server frees the association's socket.
async fn dissociate(connection: Connection, version: Version, id: u32) {
    let mut command = vec![version.byte(), DISSOCIATE];
    match version {
        Version::V4 => command.extend_from_slice(&id.to_be_bytes()),
        // counted in 16 bits (`Associations::join`)
        Version::V5 => command.extend_from_slice(&(id as u16).to_be_bytes()),
    }
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
    if stream
        .write_all(&[VERSION, DISSOCIATE, high, low])
        .await
        .is_ok()
    {
```

换成

```rust
    if stream.write_all(&command).await.is_ok() {
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
/// association's inbox, once the packet it carries is whole. A heartbeat
```

换成

```rust
/// association's inbox, once the packet it carries is whole. A v5 heartbeat
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
fn deliver(associations: &Mutex<Associations>, relayed: &AtomicBool, data: Bytes) {
    if data.starts_with(&[VERSION, HEARTBEAT]) {
        return;
    }
    let Some(message) = Message::parse(data) else {
```

换成

```rust
fn deliver(
    version: Version,
    associations: &Mutex<Associations>,
    relayed: &AtomicBool,
    data: Bytes,
) {
    if data.starts_with(&[version.byte(), HEARTBEAT]) {
        return;
    }
    let Some(message) = Message::parse(version, data) else {
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
    id: u16,
```

换成

```rust
    id: u32,
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
/// One client association's UDP through the outbound (design 6.2): an
```

换成

```rust
/// One client association's UDP through the outbound (design 6.2, 6.3): an
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
    /// The id of the carrier's next packet.
```

换成

```rust
    /// The id of the carrier's next v5 packet (`packet`).
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
            _ => Ok(None),
        }
    }
```

换成

```rust
            _ => Ok(None),
        }
    }

    /// The id of a v5 packet: one for every packet, whole or not, counted
    /// from 0. v4's packets have none.
    fn packet(&self) -> u16 {
        self.packets.fetch_add(1, Ordering::Relaxed)
    }
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
/// that ends early is one sing-box cannot read, and it closes the
/// connection for that.
```

换成

```rust
/// that ends early is one sing-box cannot read — nor tuic 0.8.5, when its
/// header is cut —, and either closes the connection for that.
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
            let packet = self.packets.fetch_add(1, Ordering::Relaxed);
            match connection.max_datagram_size() {
                Some(max) => {
                    let out = datagrams(bound.id, packet, &address, buf, max)?;
```

换成

```rust
            let version = bound.lease.state().version();
            // v5's ids are counted in 16 bits (`Associations::join`)
            let (id, short_id) = (bound.id, bound.id as u16);
            match connection.max_datagram_size() {
                Some(max) => {
                    let out = match version {
                        Version::V4 => vec![datagram_v4(id, &address, buf, max)?],
                        Version::V5 => datagrams(short_id, self.packet(), &address, buf, max)?,
                    };
```

`crates/rurge-proto-quic/src/tuic/udp.rs`——把

```rust
                    let message = encode(bound.id, packet, 1, 0, &address, buf);
```

换成

```rust
                    let message = match version {
                        Version::V4 => encode_v4(id, &address, buf),
                        Version::V5 => encode(short_id, self.packet(), 1, 0, &address, buf),
                    };
```

`crates/rurge-proto-quic/src/tuic/heartbeat.rs`——把

```rust
//! The heartbeat (TUIC v5 SPEC, "Heartbeat"; phase 2 M7 design 6.2): while
//! anything is relayed on a connection — a TCP tunnel, a UDP association —
//! a `05 04` QUIC datagram every few seconds keeps the server from taking
//! the connection for idle, as tuic-client 1.0.0 keeps it: tuic-server's
//! idle timeout is 10 seconds, which QUIC's own keep-alive every 10 seconds
//! (design 3.2) does not beat.
//!
//! - Nothing goes while nothing is relayed: an idle connection is let go of
//!   — by the server, or by the session (`IDLE_TIMEOUT`).
//! - The SPEC sends it as a datagram only: a server that takes no QUIC
//!   datagrams gets none (sing-box ends a connection on a heartbeat in a
//!   stream: an unknown command there), and QUIC's keep-alive alone keeps
//!   the connection up — against a server whose idle timeout is longer
//!   than 10 seconds (sing-box's: quic-go's 30).

use super::VERSION;
use bytes::Bytes;
use quinn::{Connection, SendDatagramError};
```

换成

```rust
//! The heartbeat (v5 SPEC, "Heartbeat"; v4's protocol README; phase 2 M7
//! design 6.2, 6.3): while anything is relayed on a connection — a TCP
//! tunnel, a UDP association — a `VER 04` command every few seconds keeps
//! the server from taking the connection for idle.
//!
//! - v5: a QUIC datagram every 3 seconds, as tuic-client 1.0.0 sends it:
//!   tuic-server's idle timeout is 10 seconds, which QUIC's own keep-alive
//!   every 10 seconds (design 3.2) does not beat. The SPEC sends it as a
//!   datagram only: a server that takes no QUIC datagrams gets none
//!   (sing-box ends a connection on a heartbeat in a stream: an unknown
//!   command there), and QUIC's keep-alive alone keeps the connection up —
//!   against a server whose idle timeout is longer than 10 seconds
//!   (sing-box's: quic-go's 30).
//! - v4: a unidirectional stream of its own every 10 seconds, as tuic
//!   0.8.5's client sends it — its server takes a heartbeat on a stream
//!   only, and closes the connection on a datagram that is no packet —,
//!   whether the server takes QUIC datagrams or not.
//! - Nothing goes while nothing is relayed: an idle connection is let go of
//!   — by the server, or by the session (`IDLE_TIMEOUT`).

use super::Version;
use bytes::Bytes;
use quinn::{Connection, ConnectionError, SendDatagramError};
```

`crates/rurge-proto-quic/src/tuic/heartbeat.rs`——把

```rust
/// How often, while something is relayed: tuic-client 1.0.0's.
pub(super) const EVERY: Duration = Duration::from_secs(3);

/// The command's type.
pub(super) const HEARTBEAT: u8 = 0x04;
```

换成

```rust
/// The command's type.
pub(super) const HEARTBEAT: u8 = 0x04;

/// How often, while something is relayed: v5's as tuic-client 1.0.0's,
/// v4's as tuic 0.8.5's client's.
pub(super) fn every(version: Version) -> Duration {
    match version {
        Version::V4 => Duration::from_secs(10),
        Version::V5 => Duration::from_secs(3),
    }
}
```

`crates/rurge-proto-quic/src/tuic/heartbeat.rs`——把

```rust
/// Sends the heartbeat on `connection` every `every` while `relays` has
/// any under way, until the connection ends — or the handle, or `relays`,
/// goes.
```

换成

```rust
/// Sends `version`'s heartbeat on `connection` every `every` while `relays`
/// has any under way, until the connection ends — or the handle, or
/// `relays`, goes. None for v5 on a connection without QUIC datagrams.
```

`crates/rurge-proto-quic/src/tuic/heartbeat.rs`——把

```rust
    every: Duration,
) -> AbortOnDropHandle<()> {
    let connection = connection.clone();
    let relays = relays.0.subscribe();
    AbortOnDropHandle::new(tokio::spawn(async move {
        tokio::select! {
            _ = connection.closed() => {}
            () = beat(&connection, relays, every) => {}
        }
    }))
}

async fn beat(connection: &Connection, mut relays: watch::Receiver<usize>, every: Duration) {
```

换成

```rust
    version: Version,
    every: Duration,
) -> Option<AbortOnDropHandle<()>> {
    if version == Version::V5 && connection.max_datagram_size().is_none() {
        return None;
    }
    let connection = connection.clone();
    let relays = relays.0.subscribe();
    Some(AbortOnDropHandle::new(tokio::spawn(async move {
        tokio::select! {
            _ = connection.closed() => {}
            () = beat(&connection, relays, version, every) => {}
        }
    })))
}

async fn beat(
    connection: &Connection,
    mut relays: watch::Receiver<usize>,
    version: Version,
    every: Duration,
) {
```

`crates/rurge-proto-quic/src/tuic/heartbeat.rs`——把

```rust
        let heartbeat = Bytes::from_static(&[VERSION, HEARTBEAT]);
        if let Err(SendDatagramError::ConnectionLost(_)) = connection.send_datagram(heartbeat) {
            return;
```

换成

```rust
        if send(connection, version).await.is_err() {
            return;
        }
    }
}

/// One heartbeat, failing only once the connection is gone. A v4 stream
/// that breaks takes its heartbeat along; the next goes on another.
async fn send(connection: &Connection, version: Version) -> Result<(), ConnectionError> {
    let heartbeat = [version.byte(), HEARTBEAT];
    match version {
        Version::V5 => match connection.send_datagram(Bytes::copy_from_slice(&heartbeat)) {
            Err(SendDatagramError::ConnectionLost(e)) => Err(e),
            _ => Ok(()),
        },
        Version::V4 => {
            let mut stream = connection.open_uni().await?;
            if stream.write_all(&heartbeat).await.is_ok() {
                // fails only for a stream finished or reset already
                let _ = stream.finish();
            }
            Ok(())
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
//! `tuic-v5` outbound (manual: Policies › TUIC; phase 2 M7 design 6): the
//! policy's QUIC connection, which its requests share (`session`),
//! authenticated as it comes up (`auth`).
//!
//! - Every command is `u8 version ‖ u8 type ‖ …`, big endian (TUIC v5
//!   SPEC); the server answers none of them.
//! - A TCP connection is a bidirectional stream of its own, the tunnel as
//!   soon as the Connect naming its target (`addr`) is written (`tcp`).
//! - UDP goes in QUIC datagrams of the same connection, or in streams of
//!   its own where the server takes no datagrams, each client association
//!   an association of its own there (`udp`).
//! - While anything is relayed, a heartbeat keeps the connection up
//!   (`heartbeat`).
```

换成

```rust
//! `tuic` and `tuic-v5` outbounds (manual: Policies › TUIC; phase 2 M7
//! design 6): the policy's QUIC connection, which its requests share
//! (`session`), authenticated as it comes up (`auth`) — TUIC v5 for
//! `tuic-v5`, TUIC v4 as tuic 0.8.5 speaks it for `tuic`.
//!
//! - Every command is `u8 version ‖ u8 type ‖ …`, big endian (TUIC v5
//!   SPEC; v4's protocol README). The server answers none, but for v4's
//!   Connect.
//! - A TCP connection is a bidirectional stream of its own, beginning with
//!   the Connect that names its target (`addr`): the tunnel as soon as that
//!   is written in v5, once the server answered that it reached the target
//!   in v4 (`tcp`).
//! - UDP goes in QUIC datagrams of the same connection, or in streams of
//!   its own where the server takes no datagrams, each client association
//!   an association of its own there (`udp`): v5 splits a packet that does
//!   not fit a datagram, v4 cannot.
//! - While anything is relayed, a heartbeat keeps the connection up
//!   (`heartbeat`): a datagram in v5, a stream in v4.
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
/// TUIC v5's version, first in every command.
const VERSION: u8 = 0x05;
```

换成

```rust
/// The TUIC a policy speaks (design 6.2, 6.3), which every command names
/// first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Version {
    /// `tuic`: tuic 0.8.5's protocol.
    V4,
    /// `tuic-v5`: the SPEC's.
    V5,
}

impl Version {
    /// The version `credentials` are for.
    fn of(credentials: &TuicVersion) -> Version {
        match credentials {
            TuicVersion::V4 { .. } => Version::V4,
            TuicVersion::V5 { .. } => Version::V5,
        }
    }

    /// The first byte of every command.
    const fn byte(self) -> u8 {
        match self {
            Version::V4 => 0x04,
            Version::V5 => 0x05,
        }
    }
}
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
    name: String,
```

换成

```rust
    name: String,
    version: Version,
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
    /// policy's `ecn`; `client-cert` comes from `keystore`.
```

换成

```rust
    /// policy's `ecn`; `client-cert` comes from `keystore`. The line's
    /// keyword is the version (`spec.version`).
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
        // the dry build both prefix it
        let TuicVersion::V5 { uuid, password } = &spec.version else {
            return Err(BuildError::new("TUIC v4 (`tuic`) is not supported yet"));
        };
        // the configuration checked it, but a spec made by hand may not be
        if password.expose().is_empty() {
            return Err(BuildError::new("`password` is empty"));
```

换成

```rust
        // the dry build both prefix it; the configuration checked the
        // credentials, but a spec made by hand may not be
        let (key, credential) = match &spec.version {
            TuicVersion::V4 { token } => ("token", token.expose()),
            TuicVersion::V5 { password, .. } => ("password", password.expose()),
        };
        if credential.is_empty() {
            return Err(BuildError::new(format!("`{key}` is empty")));
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
        let auth = Auth::new(*uuid.expose(), password.expose(), heartbeat::EVERY);
        Ok(TuicOutbound {
            name: name.to_string(),
```

换成

```rust
        let version = Version::of(&spec.version);
        let auth = Auth::new(&spec.version, heartbeat::every(version));
        Ok(TuicOutbound {
            name: name.to_string(),
            version,
```

`crates/rurge-proto-quic/src/tuic/mod.rs`——把

```rust
            let request = tcp::request(target)?;
            // one budget for the connection (when one is dialed), QUIC's
            // handshake and the Connect, which nothing answers
```

换成

```rust
            let request = tcp::request(self.version, target)?;
            // one budget for the connection (when one is dialed), QUIC's
            // handshake and the Connect — and v4's response to it
```

要点：
- 关闭码 `0xfffffff1` / `0xfffffff2` 的错误种类是 `PermissionDenied`，免得被引擎改写成 "connect timed out"。
- v4 的 UDP 包放不进一个数据报时 `InvalidInput`、什么也不发，不改走单向流。
- 心跳走单向流，不论服务端收不收数据报。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto-quic` → 通过（新增 23 条：BLAKE3 token 的已知答案、v4 认证带着 token 的哈希、收到应答才交出隧道、目标连不上时请求失败而连接照用、token 错时请求失败且下一次重拨、各个关闭码的文本、服务端重启后的失效连接重试、不应答 Connect 时受时限约束、UDP 整包走数据报与走单向流、放不下的包被拒、`Dissociate`、单向流上的心跳、载体迁到新连接、被放弃的发送不截短它的包、IPv4 映射的来源按 IPv4 读，以及各模块 v4 的编解码）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add Cargo.lock crates/rurge-proto-quic
git commit -m "feat(proto-quic): TUIC v4——BLAKE3 token、Connect 应答、不分片的 UDP、单向流心跳、关闭码"
```

### Task 5: 引擎装配与能力表

去掉 Task 1 的门，引擎工厂用一个分支为两个版本构建 `TuicOutbound`，能力表加入 `tuic` 与 `tuic-v5`（P13）；`outbounds_hysteria2.rs` 里四个与协议无关的辅助函数移进 `tests/common`。

**Files:**
- Create: `crates/rurge-engine/tests/outbounds_tuic.rs`
- Modify: `crates/rurge-config/src/spec/mod.rs`（与用例）、`tests/policy_spec.rs`、`crates/rurge-engine/src/outbounds.rs`（与用例）、`tests/common/mod.rs`、`tests/outbounds_hysteria2.rs`、`crates/rurge/src/capabilities.rs`、`crates/rurge/tests/cli.rs`

**Interfaces:**
- Consumes: Task 1 ～ 4 的全部；M7a 的 `Dialer::new`（端口跳跃遇上链式代理即拒绝）与加载期 `W0023`。
- Produces: 工厂的 TUIC 分支；能力表的 `tuic` / `tuic-v5`；`tests/common` 再导出 `FakeTuic` / `TuicScript` 与共用的辅助函数。

- [ ] **Step 1: 先写用例**

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
pub use rurge_proto_quic::testing::{FakeHysteria2, Hysteria2Script};
```

换成

```rust
pub use rurge_proto_quic::testing::{FakeHysteria2, FakeTuic, Hysteria2Script, TuicScript};
```

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
    s
}

/// One `GET` over an established tunnel (or any stream to an origin).
```

换成

```rust
    s
}

/// `CONNECT host:port` through rurge's SOCKS5 listener, the host as a
/// name; returns the tunnel.
pub async fn connect_via_socks5(proxy: SocketAddr, host: &str, port: u16) -> TcpStream {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut method = [0u8; 2];
    s.read_exact(&mut method).await.unwrap();
    assert_eq!(method, [5, 0]);
    let mut request = vec![5, 1, 0, 3, host.len() as u8];
    request.extend_from_slice(host.as_bytes());
    request.extend_from_slice(&port.to_be_bytes());
    s.write_all(&request).await.unwrap();
    let mut reply = [0u8; 10];
    tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut reply))
        .await
        .expect("the proxy answers")
        .unwrap();
    assert_eq!(reply[1], 0, "{reply:?}");
    s
}

/// One `GET` over an established tunnel (or any stream to an origin).
```

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
```

换成

```rust
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The first `n` sessions' records, oldest first, once they have finished.
pub async fn the_records(h: &Harness, n: usize) -> Vec<rurge_engine::RequestRecord> {
    let log = h.engine.request_log();
    wait_until("the sessions to finish", || log.recent(10).len() >= n).await;
    let mut records = log.recent(10);
    records.reverse();
    records
}

/// The finished UDP flows' records, newest first.
pub fn udp_records(h: &Harness) -> Vec<rurge_engine::RequestRecord> {
    h.engine
        .request_log()
        .recent(4096)
        .into_iter()
        .filter(|r| r.transport == rurge_config::session::Transport::Udp)
        .collect()
}

/// One session to the echo through `h`: a round trip, then the client
/// goes and the session finishes.
pub async fn one_session(h: &Harness, n: usize, payload: &[u8]) -> rurge_engine::RequestRecord {
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, payload).await;
    drop(tunnel);
    the_records(h, n).await[n - 1].clone()
}
```

`crates/rurge-engine/tests/outbounds_hysteria2.rs`——把

```rust
use rurge_config::session::Transport;
use rurge_config::spec::{Hysteria2Obfs, ProtoSpec};
use rurge_engine::RequestRecord;
```

换成

```rust
use rurge_config::spec::{Hysteria2Obfs, ProtoSpec};
```

`crates/rurge-engine/tests/outbounds_hysteria2.rs`——把

```rust
    harness_with(profile(proxies), trusting(fake)).await
}

fn udp_records(h: &Harness) -> Vec<RequestRecord> {
    h.engine
        .request_log()
        .recent(4096)
        .into_iter()
        .filter(|r| r.transport == Transport::Udp)
        .collect()
}

/// The first `n` sessions' records, oldest first, once they have finished.
async fn the_records(h: &Harness, n: usize) -> Vec<RequestRecord> {
    let log = h.engine.request_log();
    wait_until("the sessions to finish", || log.recent(10).len() >= n).await;
    let mut records = log.recent(10);
    records.reverse();
    records
}

/// One session to the echo through `h`: a round trip, then the client
/// goes and the session finishes.
async fn one_session(h: &Harness, n: usize, payload: &[u8]) -> RequestRecord {
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, payload).await;
    drop(tunnel);
    the_records(h, n).await[n - 1].clone()
}

/// `CONNECT host:port` through rurge's SOCKS5 listener, the host as a
/// name; returns the tunnel.
async fn connect_via_socks5(proxy: SocketAddr, host: &str, port: u16) -> TcpStream {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut method = [0u8; 2];
    s.read_exact(&mut method).await.unwrap();
    assert_eq!(method, [5, 0]);
    let mut request = vec![5, 1, 0, 3, host.len() as u8];
    request.extend_from_slice(host.as_bytes());
    request.extend_from_slice(&port.to_be_bytes());
    s.write_all(&request).await.unwrap();
    let mut reply = [0u8; 10];
    tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut reply))
        .await
        .expect("the proxy answers")
        .unwrap();
    assert_eq!(reply[1], 0, "{reply:?}");
    s
```

换成

```rust
    harness_with(profile(proxies), trusting(fake)).await
```

新建 `crates/rurge-engine/tests/outbounds_tuic.rs`：

```rust
//! Sessions that leave through `tuic-v5` and `tuic` (phase 2 M7 design 6):
//! TCP and UDP through the engine to the loopback `FakeTuic` — the policy's
//! one QUIC connection in either version, UDP in QUIC datagrams (v5's in
//! fragments, v4's whole or not at all) and on streams where the server
//! takes no datagrams, a refused password and a refused token, port
//! hopping, `underlying-proxy` (a SOCKS5 proxy's UDP), policy tests and
//! reloads.

mod common;
use common::*;
use rurge_config::HostName;
use rurge_config::spec::ProtoSpec;
use rurge_net::connector::Target;
use rurge_net::quic::testing::MultiPort;
use rurge_proto::testing::{echo_server, udp_echo_server};

/// The manual's example user, and its 16 bytes.
const ID: &str = "0233d11c-15a4-47d3-ade3-48ffca0ce119";
const UUID: [u8; 16] = [
    0x02, 0x33, 0xd1, 0x1c, 0x15, 0xa4, 0x47, 0xd3, 0xad, 0xe3, 0x48, 0xff, 0xca, 0x0c, 0xe1, 0x19,
];
/// What the fakes let in: v5's password, v4's token.
const PASSWORD: &str = "s3same";
const TOKEN: &str = "t0kenV4";

/// The TUIC a test speaks.
#[derive(Clone, Copy, Debug)]
enum Version {
    /// `tuic-v5`.
    V5,
    /// `tuic`: tuic 0.8.5's.
    V4,
}

impl Version {
    /// The line's keyword.
    fn keyword(self) -> &'static str {
        match self {
            Version::V5 => "tuic-v5",
            Version::V4 => "tuic",
        }
    }

    /// The credentials its fake lets in, as a line writes them.
    fn credentials(self) -> String {
        match self {
            Version::V5 => format!("uuid={ID}, password={PASSWORD}"),
            Version::V4 => format!("token={TOKEN}"),
        }
    }

    /// A fake that speaks it and lets `credentials` in.
    fn script(self) -> TuicScript {
        match self {
            Version::V5 => TuicScript {
                uuid: UUID,
                password: PASSWORD.into(),
                ..TuicScript::default()
            },
            Version::V4 => TuicScript::v4(TOKEN),
        }
    }
}

/// `T = <version's keyword>, 127.0.0.1, <the fake's port>, <params>`.
fn line(version: Version, fake: &FakeTuic, params: &str) -> String {
    format!(
        "T = {}, 127.0.0.1, {}, {params}",
        version.keyword(),
        fake.addr().port()
    )
}

/// `line` with the credentials the fake lets in and nothing else.
fn plain_line(version: Version, fake: &FakeTuic) -> String {
    line(version, fake, &version.credentials())
}

/// A profile that sends the name `target.test` and the loopback through
/// `T`.
fn profile(proxies: &str) -> Profile<'_> {
    Profile {
        proxies,
        rules: "DOMAIN,target.test,T\nIP-CIDR,127.0.0.1/32,T,no-resolve",
        ..Profile::default()
    }
}

/// What makes an engine trust `fake`'s certificate.
fn trusting(fake: &FakeTuic) -> EngineShared {
    EngineShared {
        roots: Some(fake.roots()),
        ..EngineShared::default()
    }
}

/// An engine on `proxies` (`profile`) that trusts `fake`.
async fn through(fake: &FakeTuic, proxies: &str) -> Harness {
    harness_with(profile(proxies), trusting(fake)).await
}

/// A CONNECT through the HTTP listener and one through the SOCKS5 listener
/// each leave as a stream of the policy's one QUIC connection, after one
/// authentication, in either version; the server is asked for the name.
#[tokio::test]
async fn tcp_leaves_through_either_version_on_one_connection() {
    let echo = echo_server().await;
    for version in [Version::V5, Version::V4] {
        let fake = FakeTuic::spawn(TuicScript {
            connect_to: Some(echo),
            ..version.script()
        });
        let h = through(&fake, &plain_line(version, &fake)).await;
        let mut http = connect_via_http(h.http(), "target.test:7").await;
        echo_through(&mut http, b"through tuic").await;
        echo_through(&mut http, &vec![0x5a; 300_000]).await;
        let mut socks = connect_via_socks5(h.socks(), "target.test", 7).await;
        echo_through(&mut socks, b"and from SOCKS5").await;
        let connects: Vec<(usize, String)> = fake
            .connects()
            .into_iter()
            .map(|c| (c.connection, c.address))
            .collect();
        assert_eq!(
            connects,
            vec![(0, "target.test:7".to_string()); 2],
            "{version:?}"
        );
        assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
        let auths = fake.auths();
        assert_eq!((fake.connections(), auths.len()), (1, 1), "{version:?}");
        assert!(auths[0].accepted, "{version:?}");
        drop((http, socks));
        for record in the_records(&h, 2).await {
            assert_eq!(record.policy, ["T"], "{record:?}");
            assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
        }
    }
}

/// v5 UDP (design 6.2): the datagrams through one client association go in
/// one association of the policy's connection, in QUIC datagrams — to two
/// echoes; whoever writes to the server's socket for it reaches the client
/// (full cone); one longer than a QUIC datagram goes in fragments, both
/// ways, only the first naming the target. The association goes with the
/// client's: the server is told.
#[tokio::test]
async fn udp_leaves_through_tuic_v5_in_datagrams() {
    let fake = FakeTuic::spawn(Version::V5.script());
    let h = through(&fake, &plain_line(Version::V5, &fake)).await;
    let (one, two) = (udp_echo_server().await, udp_echo_server().await);
    let association = udp_associate(h.socks()).await;
    for (echo, payload) in [(one, &b"one"[..]), (two, b"two"), (one, b"again")] {
        association.send("127.0.0.1", echo.port(), payload).await;
        assert_eq!(association.recv().await, (echo, payload.to_vec()));
    }
    let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    stranger
        .send_to(b"unasked", fake.udp_outside()[0])
        .await
        .unwrap();
    assert_eq!(
        association.recv().await,
        (stranger.local_addr().unwrap(), b"unasked".to_vec())
    );
    // QUIC datagrams of a loopback path hold about 1,200 to 1,400 bytes
    let long: Vec<u8> = (0..3000u32).map(|i| i as u8).collect();
    association.send("127.0.0.1", two.port(), &long).await;
    assert_eq!(association.recv().await, (two, long.clone()));

    let packets = fake.udp_packets();
    assert!(
        packets
            .iter()
            .all(|p| (p.connection, p.association, p.on_stream) == (0, 0, false)),
        "{packets:?}"
    );
    let whole: Vec<Option<String>> = packets
        .iter()
        .filter(|p| p.fragments == 1)
        .map(|p| p.address.clone())
        .collect();
    assert_eq!(whole, [one, two, one].map(|echo| Some(echo.to_string())));
    let fragments: Vec<_> = packets.iter().filter(|p| p.fragments > 1).collect();
    assert!(fragments.len() >= 3, "{fragments:?}");
    assert!(
        fragments.iter().all(|p| p.packet == fragments[0].packet
            && usize::from(p.fragments) == fragments.len()
            && p.address == (p.fragment == 0).then(|| two.to_string())),
        "{fragments:?}"
    );
    assert_eq!(fragments.iter().map(|p| p.len).sum::<usize>(), long.len());
    assert_eq!(fake.connections(), 1);
    drop(association);
    wait_until("both flows to finish", || udp_records(&h).len() == 2).await;
    for r in udp_records(&h) {
        assert_eq!(r.policy, ["T"], "{r:?}");
        assert_eq!(r.status, RecordStatus::Completed, "{r:?}");
    }
    wait_until("the association to be dissociated", || {
        fake.dissociates()
            .iter()
            .map(|d| (d.connection, d.association))
            .eq([(0, 0)])
    })
    .await;
}

/// v4 UDP (design 6.3): whole packets in QUIC datagrams of one association,
/// whose answers come back under their IPv4 source (tuic 0.8.5 names it in
/// IPv6 form); a packet too long for a datagram is dropped alone — v4
/// cannot split one —: the flow goes on, and its record says why.
#[tokio::test]
async fn v4_udp_goes_whole_in_datagrams_or_not_at_all() {
    let fake = FakeTuic::spawn(Version::V4.script());
    let h = through(&fake, &plain_line(Version::V4, &fake)).await;
    let (one, two) = (udp_echo_server().await, udp_echo_server().await);
    let association = udp_associate(h.socks()).await;
    for (echo, payload) in [(one, &b"one"[..]), (two, b"two")] {
        association.send("127.0.0.1", echo.port(), payload).await;
        assert_eq!(association.recv().await, (echo, payload.to_vec()));
    }
    association
        .send("127.0.0.1", one.port(), &vec![0x4c; 3000])
        .await;
    association.send("127.0.0.1", one.port(), b"after").await;
    assert_eq!(association.recv().await, (one, b"after".to_vec()));

    // one association, its id drawn at random; no packet split
    let packets = fake.udp_packets();
    let id = packets[0].association;
    assert!(
        packets
            .iter()
            .all(|p| (p.connection, p.association, p.on_stream, p.fragments) == (0, id, false, 1)),
        "{packets:?}"
    );
    let seen: Vec<(Option<String>, usize)> =
        packets.iter().map(|p| (p.address.clone(), p.len)).collect();
    assert_eq!(
        seen,
        [
            (Some(one.to_string()), 3),
            (Some(two.to_string()), 3),
            (Some(one.to_string()), 5)
        ]
    );
    drop(association);
    wait_until("both flows to finish", || udp_records(&h).len() == 2).await;
    let records = udp_records(&h);
    let to_one = records
        .iter()
        .find(|r| r.dst == one.to_string())
        .expect("the flow to the first echo");
    assert_eq!(to_one.status, RecordStatus::Completed, "{to_one:?}");
    assert!(
        to_one
            .error
            .as_deref()
            .is_some_and(|e| e.starts_with("tuic: a datagram longer than ")),
        "{to_one:?}"
    );
}

/// Where the server takes no QUIC datagrams (design 6.2, 6.3: `quic`
/// mode), each packet goes whole on a stream of its own in either version
/// — one too long for a datagram too —, and the answers come back on
/// streams.
#[tokio::test]
async fn udp_goes_on_streams_where_the_server_takes_no_datagrams() {
    let udp_echo = udp_echo_server().await;
    let long: Vec<u8> = (0..3000u32).map(|i| i as u8).collect();
    for version in [Version::V5, Version::V4] {
        let fake = FakeTuic::spawn(TuicScript {
            no_datagrams: true,
            ..version.script()
        });
        let h = through(&fake, &plain_line(version, &fake)).await;
        let association = udp_associate(h.socks()).await;
        for payload in [&b"on a stream"[..], &long] {
            association
                .send("127.0.0.1", udp_echo.port(), payload)
                .await;
            assert_eq!(
                association.recv().await,
                (udp_echo, payload.to_vec()),
                "{version:?}"
            );
        }
        let seen: Vec<(bool, u8, usize)> = fake
            .udp_packets()
            .iter()
            .map(|p| (p.on_stream, p.fragments, p.len))
            .collect();
        assert_eq!(seen, [(true, 1, 11), (true, 1, long.len())], "{version:?}");
        assert_eq!(fake.connections(), 1, "{version:?}");
    }
}

/// A password the server refuses (design 6.2): nothing answers a v5
/// Connect, so the CONNECT is answered and its tunnel then ends; the
/// session fails with the text that says why — the server closed the
/// connection before it relayed anything —, and neither the password nor
/// the UUID is in the record.
#[tokio::test]
async fn a_refused_password_fails_the_session() {
    let fake = FakeTuic::spawn(Version::V5.script());
    let h = through(
        &fake,
        &line(Version::V5, &fake, &format!("uuid={ID}, password=wr0ngPw")),
    )
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    let mut rest = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), tunnel.read_to_end(&mut rest))
        .await
        .expect("the tunnel ends within the bound");
    assert!(rest.is_empty(), "{rest:?}");
    let record = the_records(&h, 1).await[0].clone();
    assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
    assert_eq!(record.error.as_deref(), Some("tuic: authentication failed"));
    let shown = format!("{record:?}");
    assert!(!shown.contains("wr0ngPw") && !shown.contains(ID), "{shown}");
    let auths = fake.auths();
    assert_eq!(auths.len(), 1);
    assert_eq!((auths[0].uuid, auths[0].accepted), (Some(UUID), false));
}

/// A token the server refuses (design 6.3): tuic 0.8.5 closes the
/// connection with its code for that, which the Connect meets while it
/// waits for the server's answer — the CONNECT is answered 502 —, and the
/// session fails with the text the code stands for; the token is nowhere
/// in the record.
#[tokio::test]
async fn a_refused_token_fails_the_session() {
    let fake = FakeTuic::spawn(Version::V4.script());
    let h = through(&fake, &line(Version::V4, &fake, "token=wr0ngTk")).await;
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(b"CONNECT target.test:7 HTTP/1.1\r\nHost: target.test:7\r\n\r\n")
        .await
        .unwrap();
    let mut answer = [0u8; 12];
    tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut answer))
        .await
        .expect("the CONNECT is answered within the bound")
        .unwrap();
    assert_eq!(&answer, b"HTTP/1.1 502");
    let record = the_records(&h, 1).await[0].clone();
    assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
    assert_eq!(record.error.as_deref(), Some("tuic: authentication failed"));
    assert!(!format!("{record:?}").contains("wr0ngTk"));
    let auths = fake.auths();
    assert_eq!(auths.len(), 1);
    assert_eq!((auths[0].uuid, auths[0].accepted), (None, false));
}

/// Every `tuic` and `tuic-v5` policy of `cfg` hops every `interval`: the
/// loader takes no `port-hopping-interval` under 5 seconds.
fn hop_every(cfg: &mut rurge_config::Config, interval: Duration) {
    for spec in &mut cfg.specs {
        if let ProtoSpec::Tuic(tuic) = &mut spec.proto
            && let Some(hopping) = &mut tuic.quic.port_hopping
        {
            hopping.interval = interval;
        }
    }
}

/// Port hopping (design 3.3) with a server on three ports: the connection
/// moves among them — a new local socket each time — and its tunnels, old
/// and new, go on.
#[tokio::test]
async fn tunnels_keep_working_while_the_port_hops() {
    let echo = echo_server().await;
    let ports = MultiPort::bind(3);
    let fake = FakeTuic::spawn_on(Version::V5.script(), ports.clone());
    let set: Vec<String> = ports.ports().iter().map(u16::to_string).collect();
    let proxies = line(
        Version::V5,
        &fake,
        &format!(
            "{}, port-hopping={}",
            Version::V5.credentials(),
            set.join(";")
        ),
    );
    let h = harness_edited(profile(&proxies), trusting(&fake), |cfg| {
        hop_every(cfg, Duration::from_millis(100))
    })
    .await;
    let to = echo.to_string();
    let mut early = connect_via_http(h.http(), &to).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while ports.heard().iter().filter(|&&n| n > 0).count() < 2 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "heard only on {:?}",
            ports.heard()
        );
        echo_through(&mut early, b"hop").await;
    }
    let mut late = connect_via_http(h.http(), &to).await;
    echo_through(&mut late, b"after a hop").await;
    echo_through(&mut early, b"still there").await;
    assert!(ports.clients() >= 2, "a new local socket per hop");
    assert_eq!(fake.connections(), 1);
}

/// `tuic-v5` over a SOCKS5 `underlying-proxy` (design 3.3): the QUIC
/// connection goes in datagrams of the entry's UDP association, to the
/// server as the line names it, and carries TCP and UDP alike.
#[tokio::test]
async fn tuic_goes_over_an_underlying_socks5_proxy() {
    let (echo, udp_echo) = (echo_server().await, udp_echo_server().await);
    let fake = FakeTuic::spawn(Version::V5.script());
    let entry = FakeSocks5::spawn(Socks5Script::default()).await;
    let proxies = format!(
        "Entry = socks5, 127.0.0.1, {}, udp-relay=true\n{}",
        entry.addr().port(),
        line(
            Version::V5,
            &fake,
            &format!("{}, underlying-proxy=Entry", Version::V5.credentials())
        )
    );
    let h = through(&fake, &proxies).await;
    let mut tunnel = connect_via_http(h.http(), &echo.to_string()).await;
    echo_through(&mut tunnel, b"socks5 then tuic").await;
    let association = udp_associate(h.socks()).await;
    association
        .send("127.0.0.1", udp_echo.port(), b"by UDP")
        .await;
    assert_eq!(association.recv().await, (udp_echo, b"by UDP".to_vec()));
    let commands: Vec<u8> = entry.requests().iter().map(|r| r.command).collect();
    assert_eq!(commands, [3], "one UDP association, no TCP");
    let server = Target::new(HostName::Ip(fake.addr().ip()), fake.addr().port());
    let datagrams = entry.datagrams();
    assert!(
        !datagrams.is_empty() && datagrams.iter().all(|to| *to == server),
        "{datagrams:?}"
    );
    assert_eq!(fake.connections(), 1);
}

/// A policy test fetches the policy's `test-url` through its connection,
/// in either version: one tunnel to the loopback origin, two HEADs on it.
#[tokio::test]
async fn a_policy_test_goes_through_the_connection() {
    for version in [Version::V5, Version::V4] {
        let origin = TestServer::spawn().await;
        let fake = FakeTuic::spawn(version.script());
        let params = format!("{}, test-url={}", version.credentials(), origin.url("/"));
        let h = through(&fake, &line(version, &fake, &params)).await;
        let results = h
            .engine
            .test_policies(&["T".to_string()], None)
            .await
            .unwrap();
        let result = results[0].1.as_ref().expect("a tuic policy is tested");
        assert!(result.outcome.is_ok(), "{version:?}: {:?}", result.outcome);
        let port = origin.url("/").port().unwrap();
        let tunnels: Vec<String> = fake.connects().into_iter().map(|c| c.address).collect();
        assert_eq!(tunnels, [format!("127.0.0.1:{port}")], "{version:?}");
        let methods: Vec<String> = origin.requests().into_iter().map(|r| r.method).collect();
        assert_eq!(methods, ["HEAD", "HEAD"], "{version:?}");
    }
}

/// The outbound — and with it its QUIC connection — is kept across a
/// reload that leaves the line alone, and rebuilt when its credentials,
/// `ecn`, port hopping or version change (design 3.5, 8).
#[tokio::test]
async fn a_reload_keeps_an_unchanged_policy_and_its_connection() {
    let echo = echo_server().await;
    let fake = FakeTuic::spawn(TuicScript {
        connect_to: Some(echo),
        ..Version::V5.script()
    });
    let base = plain_line(Version::V5, &fake);
    let h = through(&fake, &base).await;
    let reload = |proxies: String| {
        let next = profile(&proxies).text(h.dns.addr());
        let dir = h.dir.path().to_path_buf();
        let shared = h.engine.shared();
        async move { runtime(&dir, &next, shared).await }
    };
    one_session(&h, 1, b"before the reload").await;
    let before = outbound_now(&h, "T");
    h.engine
        .swap_runtime(reload(format!("{base}\nOther = http, other.example, 8080")).await);
    assert!(
        Arc::ptr_eq(&before, &outbound_now(&h, "T")),
        "an unrelated reload rebuilt T"
    );
    one_session(&h, 2, b"after the reload").await;
    let places: Vec<usize> = fake.connects().iter().map(|c| c.connection).collect();
    assert_eq!(places, [0, 0], "the QUIC connection was kept");
    assert_eq!(fake.connections(), 1);

    let other = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
    let mut previous = before;
    for changed in [
        line(Version::V5, &fake, &format!("uuid={ID}, password=0ther")),
        line(Version::V5, &fake, &format!("uuid={other}, password=0ther")),
        line(
            Version::V5,
            &fake,
            &format!("uuid={other}, password=0ther, ecn=off"),
        ),
        line(
            Version::V5,
            &fake,
            &format!("uuid={other}, password=0ther, ecn=off, port-hopping=5000-6000"),
        ),
        // the other version
        line(Version::V4, &fake, "token=0ther"),
        line(Version::V4, &fake, "token=an0ther"),
    ] {
        h.engine.swap_runtime(reload(changed.clone()).await);
        let now = outbound_now(&h, "T");
        assert!(!Arc::ptr_eq(&previous, &now), "kept after: {changed}");
        previous = now;
    }
}
```

TUIC 有了 spec 之后改变的用例：

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    /// `tuic` and `tuic-v5` lines are read and checked in full; they have no
    /// spec until the engine builds TUIC (M7b task 5). `ecn` takes effect on
    /// them, `tfo` and `udp-relay` do not apply, and neither version takes
    /// the other's credentials (phase 2 M7 design 3.1, 6.1).
    #[test]
    fn a_tuic_line_is_checked_but_has_no_spec_yet() {
        let id = "0233d11c-15a4-47d3-ade3-48ffca0ce119";
        for def in [
            "tuic, 192.168.20.6, 443, token=pwd, alpn=h3".to_string(),
            format!("tuic-v5, 192.168.20.6, 443, uuid={id}, password=pwd, alpn=h3"),
            format!(
                "tuic-v5, h.test, 443, uuid={id}, password=pw, port-hopping=1234;5000-6000, port-hopping-interval=10, ecn=off, sni=edge.test, client-cert=cert1"
            ),
            "tuic, h.test, 443, token=t, ecn=on, underlying-proxy=Pick".to_string(),
            "tuic, h.test, 443, token=t, port-hopping=5000-6000, underlying-proxy=DIRECT"
                .to_string(),
```

换成

```rust
    /// `tuic` and `tuic-v5` lines are read and checked in full, and a sound
    /// one has a spec, of the version its keyword names. `ecn` takes effect
    /// on them, `tfo` and `udp-relay` do not apply, and neither version
    /// takes the other's credentials (phase 2 M7 design 3.1, 6.1).
    #[test]
    fn a_tuic_line_is_checked_and_has_a_spec() {
        let id = "0233d11c-15a4-47d3-ade3-48ffca0ce119";
        for (def, kind, chained) in [
            (
                "tuic, 192.168.20.6, 443, token=pwd, alpn=h3".to_string(),
                PolicyKind::Tuic,
                false,
            ),
            (
                format!("tuic-v5, 192.168.20.6, 443, uuid={id}, password=pwd, alpn=h3"),
                PolicyKind::TuicV5,
                false,
            ),
            (
                format!(
                    "tuic-v5, h.test, 443, uuid={id}, password=pw, port-hopping=1234;5000-6000, port-hopping-interval=10, ecn=off, sni=edge.test, client-cert=cert1"
                ),
                PolicyKind::TuicV5,
                false,
            ),
            (
                "tuic, h.test, 443, token=t, ecn=on, underlying-proxy=Pick".to_string(),
                PolicyKind::Tuic,
                true,
            ),
            // DIRECT is no chain: the hops stay
            (
                "tuic, h.test, 443, token=t, port-hopping=5000-6000, underlying-proxy=DIRECT"
                    .to_string(),
                PolicyKind::Tuic,
                false,
            ),
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            assert!(o.spec.is_none() && o.not_implemented.is_none(), "{def}");
```

换成

```rust
            assert_eq!(o.not_implemented, None, "{def}");
            let spec = o.spec.unwrap_or_else(|| panic!("{def}: no spec"));
            let ProtoSpec::Tuic(tuic) = &spec.proto else {
                panic!("{def}: {:?}", spec.proto)
            };
            assert_eq!(spec.kind, kind, "{def}");
            assert_eq!(
                matches!(tuic.version, TuicVersion::V5 { .. }),
                kind == PolicyKind::TuicV5,
                "{def}"
            );
            assert_eq!(spec.common.underlying_proxy.is_some(), chained, "{def}");
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        assert!(o.inert.is_empty(), "{:?}", o.inert);
    }
```

换成

```rust
        assert!(o.inert.is_empty(), "{:?}", o.inert);
        assert!(o.spec.is_none());
    }
```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
/// that is not one are errors, a hop interval under 5 seconds is raised to
/// 5, and `ecn` has an effect on them. They have no spec until the engine
/// builds them (M7b task 5).
```

换成

```rust
/// that is not one are errors, which leave no spec; a hop interval under 5
/// seconds is raised to 5, and `ecn` has an effect on them.
```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
    for name in ["A", "B", "C", "D"] {
        assert!(loaded.config.spec(name).is_none(), "{name}");
    }
```

换成

```rust
    for name in ["A", "B", "C"] {
        assert!(loaded.config.spec(name).is_none(), "{name}");
    }
    let d = loaded.config.spec("D").expect("a warning leaves the spec");
    let ProtoSpec::Tuic(tuic) = &d.proto else {
        panic!("{:?}", d.proto)
    };
    assert_eq!(
        tuic.quic.port_hopping.as_ref().map(|h| h.interval),
        Some(Duration::from_secs(5))
    );
    assert_eq!(d.common.ecn, Tristate::Off);
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
HY = hysteria2, proxy.test, 443, password=pw, download-bandwidth=100, salamander-password=obfs, port-hopping=5000-6000, ecn=off\n\
```

换成

```rust
HY = hysteria2, proxy.test, 443, password=pw, download-bandwidth=100, salamander-password=obfs, port-hopping=5000-6000, ecn=off\n\
TU = tuic, proxy.test, 443, token=pw, port-hopping=5000-6000, ecn=off\n\
TU5 = tuic-v5, proxy.test, 443, uuid=0233d11c-15a4-47d3-ade3-48ffca0ce119, password=pw, sni=tu.test\n\
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            ("HY", "HY"),
```

换成

```rust
            ("HY", "HY"),
            ("TU", "TU"),
            ("TU5", "TU5"),
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
        assert!(!messages[0].contains("hunter2") && !messages[0].contains("s3same0pen"));
    }

    #[test]
    fn a_dry_build_of_an_anytls_policy_leaves_no_task_behind() {
```

换成

```rust
        assert!(!messages[0].contains("hunter2") && !messages[0].contains("s3same0pen"));
    }

    /// Sound `tuic` and `tuic-v5` lines pass the dry build, which opens no
    /// connection (no tokio runtime here); a client certificate that cannot
    /// be read is a load error, and neither its password nor the policy's
    /// credentials are quoted (phase 2 M7 design 3.2, 6.1).
    #[test]
    fn the_tuic_policies_pass_the_dry_build() {
        let cfg = config(
            "[Proxy]\nT4 = tuic, proxy.test, 443, token=t0ken, port-hopping=1234;5000-6000, port-hopping-interval=10, ecn=off\n\
T5 = tuic-v5, proxy.test, 443, uuid=0233d11c-15a4-47d3-ade3-48ffca0ce119, password=pw, alpn=h3, sni=tu.test\n\
[Rule]\nFINAL,DIRECT\n",
        );
        assert!(dry_build(&cfg).is_empty());

        let cfg = config(
            "[Proxy]\nT4 = tuic, proxy.test, 443, token=s3same0pen, client-cert=cert1\n\
T5 = tuic-v5, proxy.test, 443, uuid=0233d11c-15a4-47d3-ade3-48ffca0ce119, password=s3same0pen, client-cert=cert1\n\
[Keystore]\ncert1 = type=p12, base64=QUJD, password=hunter2\n[Rule]\nFINAL,DIRECT\n",
        );
        let diags = dry_build(&cfg).sorted();
        let messages: Vec<String> = diags.iter().map(|d| d.message.clone()).collect();
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(messages[0].starts_with("policy `T4` cannot be built: keystore item `cert1`"));
        assert!(messages[1].starts_with("policy `T5` cannot be built: keystore item `cert1`"));
        for m in &messages {
            assert!(
                !m.contains("hunter2") && !m.contains("s3same0pen") && !m.contains("0233d11c"),
                "{m}"
            );
        }
    }

    #[test]
    fn a_dry_build_of_an_anytls_policy_leaves_no_task_behind() {
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
HY = hysteria2, proxy.test, 443, password=pw, skip-cert-verify=true\n\
```

换成

```rust
HY = hysteria2, proxy.test, 443, password=pw, skip-cert-verify=true\n\
TU = tuic, proxy.test, 443, token=pw, skip-cert-verify=true\n\
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
        assert_eq!(flagged, ["A", "V", "H2", "TT", "HY"]);
```

换成

```rust
        assert_eq!(flagged, ["A", "V", "H2", "TT", "HY", "TU"]);
```

`crates/rurge/tests/cli.rs`——把

```rust
        .stdout(predicate::str::contains("s3cretPw").not());
```

换成

```rust
        .stdout(predicate::str::contains("s3cretPw").not());
}

const TUIC: &str = "[General]\n[Proxy]\n\
T5 = tuic-v5, proxy.test, 443, uuid=0233d11c-15a4-47d3-ade3-48ffca0ce119, password=s3cretPw, port-hopping=5000-6000, port-hopping-interval=2, ecn=off, udp-relay=true\n\
T4 = tuic, proxy.test, 443, token=t0kenValue, tfo=true\n\
Old = tailscale, section-name=ts\nE = socks5, proxy.test, 1080\n\
[Proxy Group]\nG = select, T5, T4, underlying-proxy=E\n[Rule]\nFINAL,DIRECT\n";
const TUIC_BROKEN: &str = "[General]\n[Proxy]\n\
A = tuic-v5, proxy.test, 443, uuid=0233d11c-15a4, password=s3cretPw\n\
B = tuic, proxy.test, 443, token=t0kenValue, port-hopping=5000-6000, underlying-proxy=E\n\
C = tuic, proxy.test, 443, token=t0kenValue, shadow-tls-password=st, shadow-tls-version=3, shadow-tls-sni=site.test\n\
E = socks5, proxy.test, 1080\n[Rule]\nFINAL,DIRECT\n";

/// `rurge check` knows `tuic` and `tuic-v5` (phase 2 M7 design 6.1):
/// neither is "not implemented"; `ecn` has an effect on them, `tfo` and
/// `udp-relay` do not apply; a hop interval under 5 seconds is raised to 5,
/// saying so; a group that relays a member with `port-hopping` says it
/// leaves that member out; a UUID that is not one, `port-hopping` over an
/// `underlying-proxy` and Shadow TLS are errors; the credentials are never
/// printed.
#[test]
fn check_knows_tuic() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "tuic.conf", TUIC))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // `tailscale` is still a later milestone; TUIC is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("policy type `tailscale`") && !out.contains("policy type `tuic"),
        "{out}"
    );
    assert!(!out.contains("W0029"), "{out}");
    for (code, said) in [
        (
            "W0012",
            "tuic.conf:3: policy `T5`: `port-hopping-interval` is below the minimum of 5 seconds; using 5",
        ),
        (
            "W0028",
            "tuic.conf:3: policy `T5`: `udp-relay` does not apply to `tuic-v5` policies (UDP is always relayed); ignored",
        ),
        (
            "W0028",
            "tuic.conf:4: policy `T4`: `tfo` does not apply to `tuic` policies; ignored",
        ),
        (
            "W0023",
            "tuic.conf:8: policy group `G` relays its members through `underlying-proxy`: `T5` uses `port-hopping`, which cannot go through it; `T5 (via E)` is left out",
        ),
    ] {
        let warning = format!("warning[{code}]");
        assert!(
            out.lines()
                .any(|line| line.starts_with(&warning) && line.ends_with(said)),
            "{code} {said}\n{out}"
        );
    }
    assert!(
        !out.contains("s3cretPw") && !out.contains("t0kenValue") && !out.contains("0233d11c"),
        "{out}"
    );

    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "bad.conf", TUIC_BROKEN))
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    for said in [
        "bad.conf:3: policy `A`: `uuid` is not a valid UUID",
        "bad.conf:4: policy `B`: `port-hopping` cannot be combined with `underlying-proxy`",
        "bad.conf:5: policy `C`: Shadow TLS cannot be combined with a `tuic` policy",
    ] {
        assert!(
            out.lines()
                .any(|line| line.starts_with("error[E0018]") && line.ends_with(said)),
            "{said}\n{out}"
        );
    }
    assert!(
        !out.contains("s3cretPw") && !out.contains("t0kenValue") && !out.contains("0233d11c"),
        "{out}"
    );
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-engine --test outbounds_tuic`
Expected: FAIL——TUIC 行还没有 spec、工厂还不会构建，经它们的会话都被拒绝：

```text
test a_policy_test_goes_through_the_connection ... FAILED
test tuic_goes_over_an_underlying_socks5_proxy ... FAILED
test a_refused_token_fails_the_session ... FAILED
test tunnels_keep_working_while_the_port_hops ... FAILED
test a_reload_keeps_an_unchanged_policy_and_its_connection ... FAILED
test tcp_leaves_through_either_version_on_one_connection ... FAILED
test a_refused_password_fails_the_session ... FAILED
test v4_udp_goes_whole_in_datagrams_or_not_at_all ... FAILED
test udp_goes_on_streams_where_the_server_takes_no_datagrams ... FAILED
test udp_leaves_through_tuic_v5_in_datagrams ... FAILED
test result: FAILED. 0 passed; 10 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.07s
error: test failed, to rerun pass `-p rurge-engine --test outbounds_tuic`
```

- [ ] **Step 3: 实现**

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    // the engine builds `tuic` and `tuic-v5` from M7b task 5 on: until then
    // a valid line is checked in full but has no spec
    let built = !matches!(policy.kind, PolicyKind::Tuic | PolicyKind::TuicV5);
    let spec = (!failed && not_implemented.is_none() && built).then(|| PolicySpec {
```

换成

```rust
    let spec = (!failed && not_implemented.is_none()).then(|| PolicySpec {
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
use rurge_proto_quic::Hysteria2Outbound;
```

换成

```rust
use rurge_proto_quic::{Hysteria2Outbound, TuicOutbound};
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            // the loader makes no spec of these lines before M7b task 5
            ProtoSpec::Tuic(_) => {
                return Err(BuildError::new(format!(
                    "policy `{}`: `{}` is not implemented yet",
                    spec.name,
                    spec.kind.keyword()
                )));
            }
```

换成

```rust
            // `tuic` and `tuic-v5` alike: the spec says which version
            ProtoSpec::Tuic(tuic) => Arc::new(TuicOutbound::new(
                &spec.name,
                server_of(spec)?,
                tuic,
                spec.common.ecn,
                &self.keystore,
                self.roots.clone(),
                connector,
            )?),
```

`crates/rurge/src/capabilities.rs`——把

```rust
//! M7a), `select` groups, `url-test` / `fallback` / `load-balance` groups
//! (phase 2 M3b), and `smart` groups (phase 2 M3c).
```

换成

```rust
//! M7a), `tuic` and `tuic-v5` (phase 2 M7b), `select` groups, `url-test` /
//! `fallback` / `load-balance` groups (phase 2 M3b), and `smart` groups
//! (phase 2 M3c).
```

`crates/rurge/src/capabilities.rs`——把

```rust
            PolicyKind::Hysteria2,
```

换成

```rust
            PolicyKind::Hysteria2,
            PolicyKind::Tuic,
            PolicyKind::TuicV5,
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-engine --test outbounds_tuic` → 通过（10 条：v5 与 v4 的 TCP 共用一条连接、v5 经数据报的 UDP（分片、关联结束时发 `Dissociate`）、v4 整包发送且放不下的包单独丢弃而流继续、两个版本服务端不收数据报时走单向流、v5 口令错时会话失败（CONNECT 先回 200、隧道随后断开）、v4 token 错时回 502、端口跳跃中隧道不断、经 `underlying-proxy`（socks5，带 UDP）、经该策略的 URL 测试、重载时沿用未变的策略与它的连接而重建变了的）。
Run: `cargo test -p rurge --test cli check_knows_tuic` → 通过。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates
git commit -m "feat(engine): 装配 tuic 与 tuic-v5、能力表翻转"
```

### Task 6: 互操作与文档

sing-box 1.14.2 `tuic` 入站（v5）与官方 tuic 0.8.5 服务端（v4）的夹具与用例（P14、P15）；CI；兼容性清单、手工验收、两份 README、`CLAUDE.md` 与总设计。

**Files:**
- Create: `tests/interop/src/tuic_server.rs`（自带用例）、`tests/interop/tests/tuic.rs`
- Modify: `tests/interop/{src/lib.rs, README.md, tests/common/mod.rs, tests/sing_box_hysteria2.rs}`、`.github/workflows/ci.yml`、`docs/surge-compatibility-matrix.md`、`docs/acceptance/phase2-manual.md`、`README.md`、`README_en.md`、`CLAUDE.md`、`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`

**Interfaces:**
- Consumes: 互操作夹具既有的 `SingBox`、`Reference`、`roundtrip` / `roundtrip_big`、测试 TLS 夹具。
- Produces: `rurge_interop` 的 `render_tuic` / `SingBox::spawn_tuic`；`rurge_interop::tuic_server`（`RURGE_TEST_TUIC_SERVER`；只监听 `127.0.0.1`）；`tests/common` 的 `transfer` 与 `udp_side_by_side`（从 hysteria2 的互操作用例移来）。

- [ ] **Step 1: 互操作夹具与用例**

`tests/interop/src/lib.rs`——把

```rust
//! 127.0.0.1 only (a `hysteria2` inbound's UDP port too) — but for the UDP
//! port of a WireGuard endpoint, which sing-box opens on every address — its
//! single outbound is `direct`, and it never holds a key that touches the
//! machine (`set_system_proxy`, `tun`, `auto_route`; a WireGuard endpoint
//! runs in user space).
```

换成

```rust
//! 127.0.0.1 only (the UDP port of a `hysteria2` or `tuic` inbound too) —
//! but for the UDP port of a WireGuard endpoint, which sing-box opens on
//! every address — its single outbound is `direct`, and it never holds a key
//! that touches the machine (`set_system_proxy`, `tun`, `auto_route`; a
//! WireGuard endpoint runs in user space).
```

`tests/interop/src/lib.rs`——把

```rust
pub mod trusttunnel;
```

换成

```rust
pub mod trusttunnel;
pub mod tuic_server;
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

/// sing-box's `tuic` inbound, which speaks TUIC v5 only: QUIC with TLS (ALPN
/// `h3`) and one user.
pub struct TuicInbound {
    /// The user's UUID, hyphenated.
    pub uuid: String,
    pub password: String,
    /// No client certificate is asked for: `client_ca` must be `None`.
    pub tls: TlsFiles,
}

/// The whole configuration for `inbound` on UDP port `port`, and a `mixed`
/// inbound on `ready` that says when sing-box is up, as for `hysteria2`.
/// sing-box's defaults stay — Cubic, a 3 s authentication timeout, a
/// heartbeat every 10 s, no 0-RTT —, and its TUIC server always takes QUIC
/// datagrams: nothing in its configuration turns them off.
pub fn render_tuic(inbound: &TuicInbound, port: u16, ready: u16) -> Value {
    assert!(
        inbound.tls.client_ca.is_none(),
        "the tuic inbound asks for no client certificate here"
    );
    json!({
        "log": { "level": "warn", "timestamp": false },
        "inbounds": [
            {
                "type": "tuic",
                "tag": "tuic",
                "listen": "127.0.0.1",
                "listen_port": port,
                "users": [{ "name": "u", "uuid": inbound.uuid, "password": inbound.password }],
                "tls": {
                    "enabled": true,
                    "alpn": ["h3"],
                    "certificate_path": inbound.tls.certificate,
                    "key_path": inbound.tls.key,
                },
            },
            { "type": "mixed", "tag": "ready", "listen": "127.0.0.1", "listen_port": ready },
        ],
        "outbounds": [{ "type": "direct", "tag": "direct" }],
    })
}

/// A reference implementation running as a child on the loopback; killed and
```

`tests/interop/src/lib.rs`——把

```rust
            }
        }
    }

    pub(crate) fn port(&self, index: usize) -> u16 {
```

换成

```rust
            }
        }
    }

    /// Waits until the child's output holds `needle`: the readiness of a
    /// reference with no TCP port to probe, which says when it listens.
    pub(crate) fn wait_for_output(&mut self, needle: &str) {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if self.log_text().contains(needle) {
                return;
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!(
                    "{} exited early ({status}):\n{}",
                    self.what,
                    self.log_text()
                );
            }
            assert!(
                Instant::now() < deadline,
                "{} never said `{needle}`:\n{}",
                self.what,
                self.log_text()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub(crate) fn port(&self, index: usize) -> u16 {
```

`tests/interop/src/lib.rs`——把

```rust
        let config = render_hysteria2(inbound, port, ready);
```

换成

```rust
        let config = render_hysteria2(inbound, port, ready);
        (SingBox::launch(binary, dir, &config, vec![ready]), port)
    }

    /// Starts `binary` with `inbound` on a UDP port of its own; that port,
    /// once sing-box is up.
    pub fn spawn_tuic(binary: &Path, dir: &Path, inbound: &TuicInbound) -> (SingBox, u16) {
        let (port, ready) = (free_udp_port(), free_port());
        let config = render_tuic(inbound, port, ready);
```

`tests/interop/src/lib.rs`——把

```rust
                json!({ "type": name, "password": "obfs-pw" })
            );
        }
    }

    #[test]
    fn free_ports_are_usable() {
```

换成

```rust
                json!({ "type": name, "password": "obfs-pw" })
            );
        }
    }

    fn tuic() -> TuicInbound {
        TuicInbound {
            uuid: "0233d11c-15a4-47d3-ade3-48ffca0ce119".into(),
            password: "tuic-pw".into(),
            tls: TlsFiles {
                certificate: "leaf.pem".into(),
                key: "leaf.key".into(),
                client_ca: None,
            },
        }
    }

    /// The QUIC inbound and the probe on 127.0.0.1, nothing of the machine's.
    #[test]
    fn the_tuic_configuration_never_touches_the_machine() {
        let config = render_tuic(&tuic(), 4443, 1001);
        let text = config.to_string();
        for forbidden in ["set_system_proxy", "tun", "auto_route", "0.0.0.0", "::"] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
        }
        let top: Vec<&String> = config.as_object().unwrap().keys().collect();
        assert_eq!(top, ["inbounds", "log", "outbounds"]);
        for inbound in config["inbounds"].as_array().unwrap() {
            assert_eq!(inbound["listen"], "127.0.0.1");
        }
        assert_eq!(
            config["outbounds"],
            json!([{ "type": "direct", "tag": "direct" }])
        );
    }

    #[test]
    fn the_tuic_inbound_is_rendered_as_sing_box_spells_it() {
        let config = render_tuic(&tuic(), 4443, 1001);
        let inbound = &config["inbounds"][0];
        assert_eq!(
            (&inbound["type"], &inbound["listen_port"]),
            (&json!("tuic"), &json!(4443))
        );
        // a TUIC user is a name, a UUID and a password
        assert_eq!(
            inbound["users"],
            json!([{ "name": "u", "uuid": "0233d11c-15a4-47d3-ade3-48ffca0ce119", "password": "tuic-pw" }])
        );
        // rurge offers `h3` unless told otherwise, and a QUIC server that
        // picks no protocol fails its handshake (phase 2 M7 design 3.2)
        assert_eq!(
            inbound["tls"],
            json!({
                "enabled": true,
                "alpn": ["h3"],
                "certificate_path": "leaf.pem",
                "key_path": "leaf.key",
            })
        );
        // sing-box's defaults
        for absent in [
            "congestion_control",
            "auth_timeout",
            "zero_rtt_handshake",
            "heartbeat",
        ] {
            assert!(inbound.get(absent).is_none(), "`{absent}` in {inbound}");
        }
        // the probe after it: sing-box starts its inbounds in order
        assert_eq!(
            config["inbounds"][1],
            json!({ "type": "mixed", "tag": "ready", "listen": "127.0.0.1", "listen_port": 1001 })
        );
    }

    #[test]
    fn free_ports_are_usable() {
```

新建 `tests/interop/src/tuic_server.rs`：

```rust
//! The official TUIC server, tuic 0.8.5, as a child process on the loopback:
//! the reference for `tuic` — TUIC v4, which sing-box does not speak (phase
//! 2 M7 design 6.4, M7-D3, M7-D6). It is GPL-licensed and only ever runs as
//! a process of its own; nothing of it is linked or copied. CI installs it
//! on Linux only: elsewhere a missing binary is always a skip, even with
//! `RURGE_INTEROP_REQUIRED=1`. Only `RURGE_TEST_TUIC_SERVER` names it, never
//! `PATH`: a `tuic-server` found there is likely a 1.x one, which speaks
//! TUIC v5 only and reads another configuration.
//!
//! The same rules as for sing-box apply: nothing is downloaded or installed
//! here, and the rendered configuration listens on 127.0.0.1 only — but for
//! the socket the server relays UDP through, which 0.8.5 always binds to a
//! free port on every address (`[::]:0`), with no setting for it. It has no
//! TCP port to probe: it is up once its log says so.

use crate::{REQUIRED_ENV, Reference, TlsFiles, free_udp_port};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BINARY_ENV: &str = "RURGE_TEST_TUIC_SERVER";

/// What the server logs (at `info`) once its socket is bound.
const STARTED: &str = "Server started";

/// `RURGE_TEST_TUIC_SERVER`, and nothing else.
pub fn locate() -> Option<PathBuf> {
    std::env::var_os(BINARY_ENV)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
}

/// The binary — or `None` after saying why `test` is skipped. With
/// `RURGE_INTEROP_REQUIRED=1` (CI) a missing binary is a failure on Linux,
/// the only platform CI installs it on.
pub fn tuic_server_or_skip(test: &str) -> Option<PathBuf> {
    if let Some(path) = locate() {
        return Some(path);
    }
    if cfg!(target_os = "linux") && std::env::var(REQUIRED_ENV).as_deref() == Ok("1") {
        panic!("{REQUIRED_ENV}=1 but {BINARY_ENV} names no tuic-server");
    }
    eprintln!(
        "skipping {test}: no tuic-server 0.8.5 ({BINARY_ENV}; installed on Linux CI only); see tests/interop/README.md"
    );
    None
}

/// The whole configuration: one server on `127.0.0.1:port` letting `token`
/// in, with the certificate and key of `tls` (PEM paths) and ALPN `h3` — by
/// default it selects none, and a QUIC client that offered one then fails
/// the handshake (phase 2 M7 design 3.2) —, logging at `info`, which says
/// when it is up. The server refuses a key it does not know, so nothing
/// else is written; its other defaults stay (Cubic, a 15 s idle timeout, a
/// 1 s authentication timeout, at most 1500 bytes taken from a target in a
/// UDP packet).
pub fn render(port: u16, token: &str, tls: &TlsFiles) -> Value {
    assert!(
        tls.client_ca.is_none(),
        "tuic-server asks for no client certificate"
    );
    json!({
        "port": port,
        "token": [token],
        "certificate": tls.certificate,
        "private_key": tls.key,
        "ip": "127.0.0.1",
        "alpn": ["h3"],
        "log_level": "info",
    })
}

/// A running tuic-server; killed and reaped on drop.
pub struct TuicServer {
    server: Reference,
    port: u16,
}

impl TuicServer {
    /// Writes the configuration into `dir`, starts `binary` there and waits
    /// until it says it listens.
    pub fn spawn(binary: &Path, dir: &Path, token: &str, tls: &TlsFiles) -> TuicServer {
        let port = free_udp_port();
        let config = dir.join("tuic-server.json");
        std::fs::write(&config, render(port, token, tls).to_string()).expect("write the config");
        let mut command = Command::new(binary);
        command.arg("-c").arg(&config).current_dir(dir);
        let mut server = Reference::start(
            "tuic-server",
            command,
            Vec::new(),
            dir.join("tuic-server.log"),
        );
        server.wait_for_output(STARTED);
        TuicServer { server, port }
    }

    /// The loopback UDP port.
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn log_text(&self) -> String {
        self.server.log_text()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> TlsFiles {
        TlsFiles {
            certificate: "leaf.pem".into(),
            key: "leaf.key".into(),
            client_ca: None,
        }
    }

    #[test]
    fn the_configuration_stays_on_the_loopback() {
        let config = render(4001, "t0ken", &files());
        let text = config.to_string();
        for forbidden in ["0.0.0.0", "::"] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
        }
        assert_eq!(config["ip"], "127.0.0.1");
    }

    /// Every key is one 0.8.5 reads: it refuses any other.
    #[test]
    fn the_configuration_is_written_as_tuic_server_reads_it() {
        assert_eq!(
            render(4001, "t0ken", &files()),
            json!({
                "port": 4001,
                "token": ["t0ken"],
                "certificate": "leaf.pem",
                "private_key": "leaf.key",
                "ip": "127.0.0.1",
                "alpn": ["h3"],
                "log_level": "info",
            })
        );
    }
}
```

`tests/interop/tests/common/mod.rs`——把

```rust
    assert_eq!(&buf, b"interop");
```

换成

```rust
    assert_eq!(&buf, b"interop");
}

/// `len` bytes each way at once through one tunnel. Bounded like
/// `roundtrip`, by a minute.
pub async fn transfer(out: &OutboundRef, echo: SocketAddr, len: usize) {
    let bound = std::time::Duration::from_secs(60);
    let payload: Vec<u8> = (0..len).map(|i| (i % 253) as u8).collect();
    let stream = tokio::time::timeout(
        bound,
        out.connect_tcp(&target(echo), &ConnectOpts::default()),
    )
    .await
    .expect("the tunnel is established within the bound")
    .expect("the tunnel is established");
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut back = vec![0u8; len];
    let both = async {
        tokio::join!(async { writer.write_all(&payload).await.unwrap() }, async {
            reader.read_exact(&mut back).await.unwrap()
        })
    };
    tokio::time::timeout(bound, both)
        .await
        .expect("the payload makes it there and back within the bound");
    assert!(back == payload, "the echo differs");
}

/// Two carriers of one policy at once, each a UDP session of its own on
/// the policy's connection, each with payloads of its own (an answer handed
/// to the other carrier would show): a few bytes, then 3000 — more than a
/// QUIC datagram holds, so a protocol that splits packets sends them in
/// fragments both ways (macOS carries no loopback datagram past 9216).
/// Bounded like `roundtrip`.
pub async fn udp_side_by_side(out: &OutboundRef, echo: SocketAddr) {
    let bound = std::time::Duration::from_secs(10);
    let mut carriers = Vec::new();
    for _ in 0..2 {
        let carrier = tokio::time::timeout(bound, out.open_udp(&ConnectOpts::default()))
            .await
            .expect("the carrier opens within the bound")
            .expect("the carrier opens");
        carriers.push(carrier);
    }
    let small = |n: u8| vec![n; 7];
    let large = |n: u8| -> Vec<u8> { (0..3000u32).map(|i| (i as u8) ^ n).collect() };
    for payloads in [[small(1), small(2)], [large(1), large(2)]] {
        for (carrier, payload) in carriers.iter().zip(&payloads) {
            carrier.send_to(payload, &target(echo)).await.unwrap();
            let mut buf = vec![0u8; 65536];
            let (n, from) = tokio::time::timeout(bound, carrier.recv_from(&mut buf))
                .await
                .expect("the echo comes back within the bound")
                .unwrap();
            assert_eq!(from, target(echo));
            assert!(buf[..n] == payload[..], "the echo differs ({n} bytes)");
        }
    }
```

`tests/interop/tests/sing_box_hysteria2.rs`——把

```rust
    format!("[Proxy]\n{}[Rule]\nFINAL,DIRECT\n", lines.concat())
}

/// `len` bytes each way at once through one tunnel. Bounded like
/// `roundtrip`, by a minute.
async fn transfer(out: &OutboundRef, echo: SocketAddr, len: usize) {
    let bound = Duration::from_secs(60);
    let payload: Vec<u8> = (0..len).map(|i| (i % 253) as u8).collect();
    let stream = tokio::time::timeout(
        bound,
        out.connect_tcp(&target(echo), &ConnectOpts::default()),
    )
    .await
    .expect("the tunnel is established within the bound")
    .expect("the tunnel is established");
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut back = vec![0u8; len];
    let both = async {
        tokio::join!(async { writer.write_all(&payload).await.unwrap() }, async {
            reader.read_exact(&mut back).await.unwrap()
        })
    };
    tokio::time::timeout(bound, both)
        .await
        .expect("the payload makes it there and back within the bound");
    assert!(back == payload, "the echo differs");
}

/// Two carriers of one policy at once, each a UDP session of its own on the
/// policy's connection, each with payloads of its own (an answer handed to
/// the other session would show): a few bytes, then 3000 — more than a QUIC
/// datagram holds, so they go in Hysteria fragments both ways (sing-box
/// cuts its answers at about 1200 bytes and sends no message past 4096;
/// macOS carries no loopback datagram past 9216). Bounded like `roundtrip`.
async fn udp(out: &OutboundRef, echo: SocketAddr) {
    let bound = Duration::from_secs(10);
    let mut carriers = Vec::new();
    for _ in 0..2 {
        let carrier = tokio::time::timeout(bound, out.open_udp(&ConnectOpts::default()))
            .await
            .expect("the carrier opens within the bound")
            .expect("the carrier opens");
        carriers.push(carrier);
    }
    let small = |n: u8| vec![n; 7];
    let large = |n: u8| -> Vec<u8> { (0..3000u32).map(|i| (i as u8) ^ n).collect() };
    for payloads in [[small(1), small(2)], [large(1), large(2)]] {
        for (carrier, payload) in carriers.iter().zip(&payloads) {
            carrier.send_to(payload, &target(echo)).await.unwrap();
            let mut buf = vec![0u8; 65536];
            let (n, from) = tokio::time::timeout(bound, carrier.recv_from(&mut buf))
                .await
                .expect("the echo comes back within the bound")
                .unwrap();
            assert_eq!(from, target(echo));
            assert!(buf[..n] == payload[..], "the echo differs ({n} bytes)");
        }
    }
}

/// TCP with one chunk and with many, then UDP, on the policy's one
```

换成

```rust
    format!("[Proxy]\n{}[Rule]\nFINAL,DIRECT\n", lines.concat())
}

/// TCP with one chunk and with many, then UDP, on the policy's one
```

`tests/interop/tests/sing_box_hysteria2.rs`——把

```rust
    udp(out, udp_echo).await;
```

换成

```rust
    udp_side_by_side(out, udp_echo).await;
```

`tests/interop/tests/sing_box_hysteria2.rs`——把

```rust
        transfer(&out, echo, (9 << 20) + 777).await;
    }
}

#[tokio::test]
```

换成

```rust
        transfer(&out, echo, (9 << 20) + 777).await;
    }
}

/// Two sessions side by side; their 3000 bytes go in Hysteria fragments
/// both ways (sing-box cuts its answers at about 1200 bytes and sends no
/// message past 4096).
#[tokio::test]
```

`tests/interop/tests/sing_box_hysteria2.rs`——把

```rust
    udp(&out, udp_echo_server().await).await;
```

换成

```rust
    udp_side_by_side(&out, udp_echo_server().await).await;
```

新建 `tests/interop/tests/tuic.rs`：

```rust
//! rurge's TUIC outbounds against the references (phase 2 M7 design 6.4,
//! M7-D3, M7-D6): `tuic-v5` against sing-box 1.14.2's `tuic` inbound, which
//! speaks v5 only — TCP with one chunk, with many and past a stream's
//! receive window, UDP whole and in TUIC fragments, a wrong password —, and
//! `tuic` (v4) against the official tuic 0.8.5 server, on Linux CI only —
//! TCP, UDP, a wrong token. Each server listens on a loopback UDP port with
//! a certificate for 127.0.0.1 signed by the fixture's CA, which rurge
//! trusts, and selects ALPN `h3`, rurge's default; every target is a
//! loopback IP literal. Both always take QUIC datagrams, so UDP goes in
//! `native` mode: `quic` mode (a stream per packet) is the loopback fake's
//! alone. Port hopping is not covered: the server would need a DNAT range.

mod common;

use common::*;
use rurge_interop::TuicInbound;
use rurge_interop::tuic_server::{TuicServer, tuic_server_or_skip};
use std::io;
use std::time::Duration;

/// The manual's example user.
const UUID: &str = "0233d11c-15a4-47d3-ade3-48ffca0ce119";
const PASSWORD: &str = "tuic-interop";
const TOKEN: &str = "tuic-v4-interop";

/// What both versions say once the server refused the authentication.
const REFUSED: &str = "tuic: authentication failed";

/// sing-box's `tuic` inbound for `UUID` with `PASSWORD`, its UDP port, and
/// the fixture whose CA signed its certificate for 127.0.0.1.
fn sing_box(bin: &Path, dir: &Path) -> (SingBox, u16, Arc<TlsFixture>) {
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let inbound = TuicInbound {
        uuid: UUID.to_string(),
        password: PASSWORD.to_string(),
        tls: leaf_files(&fixture, dir),
    };
    let (sb, port) = SingBox::spawn_tuic(bin, dir, &inbound);
    (sb, port, fixture)
}

/// tuic 0.8.5's server letting `TOKEN` in, and the fixture whose CA signed
/// its certificate for 127.0.0.1.
fn tuic_server(bin: &Path, dir: &Path) -> (TuicServer, Arc<TlsFixture>) {
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let server = TuicServer::spawn(bin, dir, TOKEN, &leaf_files(&fixture, dir));
    (server, fixture)
}

/// The outbound of `name = tuic-v5, 127.0.0.1, <port>, uuid=UUID,
/// password=<password>`.
fn v5(port: u16, password: &str, fixture: &Arc<TlsFixture>) -> OutboundRef {
    let profile = format!(
        "[Proxy]\nT = tuic-v5, 127.0.0.1, {port}, uuid={UUID}, password={password}\n[Rule]\nFINAL,DIRECT\n"
    );
    outbound(&profile, "T", Some(fixture))
}

/// The outbound of `name = tuic, 127.0.0.1, <port>, token=<token>`.
fn v4(port: u16, token: &str, fixture: &Arc<TlsFixture>) -> OutboundRef {
    let profile =
        format!("[Proxy]\nT = tuic, 127.0.0.1, {port}, token={token}\n[Rule]\nFINAL,DIRECT\n");
    outbound(&profile, "T", Some(fixture))
}

/// Datagrams to `echo` through a fresh carrier of `out`, one after another
/// until a send fails, and that failure. Nothing answers the
/// authentication, so the carrier comes up, and its packets go until the
/// server's refusal closes the connection. Bounded like `roundtrip`.
async fn udp_until_refused(out: &OutboundRef, echo: SocketAddr) -> io::Error {
    let bound = Duration::from_secs(10);
    let carrier = tokio::time::timeout(bound, out.open_udp(&ConnectOpts::default()))
        .await
        .expect("the carrier opens within the bound")
        .expect("the carrier opens: nothing answers the authentication");
    let deadline = tokio::time::Instant::now() + bound;
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no send failed within the bound"
        );
        match carrier.send_to(b"interop", &target(echo)).await {
            Ok(()) => tokio::time::sleep(Duration::from_millis(20)).await,
            Err(e) => return e,
        }
    }
}

/// One chunk, many, then 9 MiB each way at once: past rurge's receive window
/// of a stream (8 MiB, phase 2 M7 design 3.2), what comes back needs the
/// credit rurge grants as it reads.
#[tokio::test]
async fn v5_tcp_against_sing_box() {
    let Some(bin) = sing_box_or_skip("v5_tcp_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (_sb, port, fixture) = sing_box(&bin, dir.path());
    let (out, echo) = (v5(port, PASSWORD, &fixture), echo_server().await);
    roundtrip(&out, echo).await;
    roundtrip_big(&out, echo).await;
    transfer(&out, echo, (9 << 20) + 777).await;
}

/// Two associations side by side; their 3000 bytes go in TUIC fragments
/// both ways (sing-box splits its answers into datagrams of at most 1197
/// bytes).
#[tokio::test]
async fn v5_udp_against_sing_box() {
    let Some(bin) = sing_box_or_skip("v5_udp_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (_sb, port, fixture) = sing_box(&bin, dir.path());
    let out = v5(port, PASSWORD, &fixture);
    udp_side_by_side(&out, udp_echo_server().await).await;
}

/// sing-box answers no v5 authentication. It closes a connection whose token
/// does not match — code 0 and no reason, as it closes one for any failure
/// —, and often ends a Connect it holds first (STOP_SENDING 0 and a FIN).
/// So the tunnel comes up, as every v5 tunnel does at once, and then ends
/// with nothing relayed: on `tuic: authentication failed` where the close
/// comes first, on the end of the stream where the Connect's end does, and
/// a write may meet `the server stopped the stream`. UDP knows no such
/// race: the send that finds the connection closed so fails with `tuic:
/// authentication failed`.
#[tokio::test]
async fn v5_a_wrong_password_against_sing_box() {
    let Some(bin) = sing_box_or_skip("v5_a_wrong_password_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (_sb, port, fixture) = sing_box(&bin, dir.path());
    let out = v5(port, "nope", &fixture);
    let bound = Duration::from_secs(10);
    let tunnel = tokio::time::timeout(
        bound,
        out.connect_tcp(&target(echo_server().await), &ConnectOpts::default()),
    )
    .await
    .expect("the tunnel comes up or fails within the bound");
    match tunnel {
        // the refusal was in before the Connect went out
        Err(err) => assert_eq!(err.to_string(), REFUSED),
        Ok(mut tunnel) => {
            let write = tokio::time::timeout(bound, tunnel.write_all(b"interop"))
                .await
                .expect("the write ends within the bound");
            if let Err(e) = write {
                let text = e.to_string();
                assert!(
                    text == REFUSED || text == "tuic: the server stopped the stream (code 0x0)",
                    "{text}"
                );
            }
            let mut buf = [0u8; 7];
            let read = tokio::time::timeout(bound, tunnel.read(&mut buf))
                .await
                .expect("the tunnel ends within the bound");
            match read {
                Ok(n) => assert_eq!(n, 0, "a wrong password must relay nothing"),
                Err(e) => assert_eq!(e.to_string(), REFUSED),
            }
        }
    }
    let err = udp_until_refused(&out, udp_echo_server().await).await;
    assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(err.to_string(), REFUSED);
}

/// One chunk and many, each tunnel once the server answered its Connect
/// (`04 ff 00`).
#[tokio::test]
async fn v4_tcp_against_tuic_server() {
    let Some(bin) = tuic_server_or_skip("v4_tcp_against_tuic_server") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = tuic_server(&bin, dir.path());
    let (out, echo) = (v4(server.port(), TOKEN, &fixture), echo_server().await);
    roundtrip(&out, echo).await;
    roundtrip_big(&out, echo).await;
}

/// Whole packets in QUIC datagrams (v4 has no fragments). The server names
/// an IPv4 source in its IPv4-mapped IPv6 form, as its one dual-stack socket
/// reports it; rurge reads it as the IPv4 address `udp_roundtrip` expects.
#[tokio::test]
async fn v4_udp_against_tuic_server() {
    let Some(bin) = tuic_server_or_skip("v4_udp_against_tuic_server") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = tuic_server(&bin, dir.path());
    let out = v4(server.port(), TOKEN, &fixture);
    udp_roundtrip(&out, udp_echo_server().await).await;
}

/// tuic 0.8.5 closes a connection whose token it does not know with the
/// code for it (0xfffffff1): the Connect, which waits for its response,
/// fails with `tuic: authentication failed`, and so does the UDP send that
/// finds the connection closed so.
#[tokio::test]
async fn v4_a_wrong_token_against_tuic_server() {
    let Some(bin) = tuic_server_or_skip("v4_a_wrong_token_against_tuic_server") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = tuic_server(&bin, dir.path());
    let out = v4(server.port(), "nope", &fixture);
    let tcp = tokio::time::timeout(
        Duration::from_secs(10),
        out.connect_tcp(&target(echo_server().await), &ConnectOpts::default()),
    )
    .await
    .expect("the server answers within the bound");
    let Err(tcp) = tcp else {
        panic!("a wrong token must not open a tunnel");
    };
    assert!(
        matches!(&tcp, OutboundError::Io(e) if e.kind() == io::ErrorKind::PermissionDenied),
        "{tcp:?}"
    );
    assert_eq!(tcp.to_string(), REFUSED);
    let udp = udp_until_refused(&out, udp_echo_server().await).await;
    assert_eq!(udp.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(udp.to_string(), REFUSED);
}
```

- [ ] **Step 2: 运行**

Run: `cargo test -p rurge-interop`
Expected: 本机没有 sing-box 与 tuic-server 时，6 条 TUIC 互操作用例各打印一行 `skipping …` 后通过；夹具自己的单元用例照常断言并通过。互操作由 CI 证明（sing-box 三平台都装；tuic 0.8.5 只在 Linux——它的 SHA-256 见「延后事项」#1）。

- [ ] **Step 3: CI 与文档**

CI：只在 Linux 作业里按 SHA-256 安装 tuic 0.8.5 服务端（`tuic-server-0.8.5-x86_64-linux-musl`），设 `RURGE_TEST_TUIC_SERVER`：

`.github/workflows/ci.yml`——把

```yaml
          echo "RURGE_TEST_SNELL_SERVER=$bin" >> "$GITHUB_ENV"
```

换成

```yaml
          echo "RURGE_TEST_SNELL_SERVER=$bin" >> "$GITHUB_ENV"
      - name: Install the tuic 0.8.5 server for the TUIC v4 interoperability tests
        if: runner.os == 'Linux'
        shell: bash
        run: |
          set -euo pipefail
          version=0.8.5
          asset="tuic-server-$version-x86_64-linux-musl"
          # PLACEHOLDER: GitHub publishes no `digest` for this release's
          # assets, so the SHA-256 is not pinned yet. The step fails until the
          # owner writes the asset's SHA-256 here (tests/interop/README.md,
          # "tuic-server").
          sha=FILL-IN-THE-SHA-256-OF-tuic-server-0.8.5-x86_64-linux-musl
          if ! [[ "$sha" =~ ^[0-9a-f]{64}$ ]]; then
            echo "tuic-server: no SHA-256 is pinned for $asset; see tests/interop/README.md"
            exit 1
          fi
          cd "$RUNNER_TEMP"
          curl -fsSL --retry 3 --retry-all-errors -o "$asset" "https://github.com/tuic-protocol/tuic/releases/download/$version/$asset"
          actual=$(sha256sum "$asset" | cut -d' ' -f1)
          if [ "$actual" != "$sha" ]; then
            echo "tuic-server checksum mismatch: expected $sha, got $actual"
            exit 1
          fi
          mkdir -p tuic-server
          mv "$asset" tuic-server/tuic-server
          chmod +x tuic-server/tuic-server
          echo "RURGE_TEST_TUIC_SERVER=$PWD/tuic-server/tuic-server" >> "$GITHUB_ENV"
```

`tests/interop/README.md`——把

```markdown
`rurge-interop` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它把 [sing-box](https://sing-box.sagernet.org/)、[xray](https://github.com/XTLS/Xray-core)、[shadowsocks-rust](https://github.com/shadowsocks/shadowsocks-rust) 的 `ssserver`、Surge 官方的 `snell-server` 与 [TrustTunnel](https://github.com/TrustTunnel/TrustTunnel) 的 `trusttunnel_endpoint` 作为参照实现，以回环子进程的方式拉起来，驱动 rurge 的 `http` / `https` / `socks5` / `trojan` / `vmess` / `anytls` / `wireguard` / `ss` / `snell` / `h2-connect` / `trust-tunnel` / `hysteria2` 出站，以及包在 Shadow TLS 里的 `trojan`，去连它们，验证 rurge 与真实的第三方实现互通。xray 只用来跑 `vmess`：VMess 协议由 xray 所在的这一脉实现定义，sing-box 的实现是重写，手写的编解码需要两个独立参照互相印证（M2 设计 M2-D5）。
```

换成

```markdown
`rurge-interop` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它把 [sing-box](https://sing-box.sagernet.org/)、[xray](https://github.com/XTLS/Xray-core)、[shadowsocks-rust](https://github.com/shadowsocks/shadowsocks-rust) 的 `ssserver`、Surge 官方的 `snell-server`、[TrustTunnel](https://github.com/TrustTunnel/TrustTunnel) 的 `trusttunnel_endpoint` 与 TUIC 官方 [tuic](https://github.com/tuic-protocol/tuic) 0.8.5 的 `tuic-server` 作为参照实现，以回环子进程的方式拉起来，驱动 rurge 的 `http` / `https` / `socks5` / `trojan` / `vmess` / `anytls` / `wireguard` / `ss` / `snell` / `h2-connect` / `trust-tunnel` / `hysteria2` / `tuic-v5` / `tuic` 出站，以及包在 Shadow TLS 里的 `trojan`，去连它们，验证 rurge 与真实的第三方实现互通。xray 只用来跑 `vmess`：VMess 协议由 xray 所在的这一脉实现定义，sing-box 的实现是重写，手写的编解码需要两个独立参照互相印证（M2 设计 M2-D5）。tuic-server 只用来跑 `tuic`（TUIC v4）：sing-box 只实现 v5（M7 设计 M7-D3）。
```

`tests/interop/README.md`——把

```markdown
本地默认不安装 sing-box、xray、shadowsocks-rust、snell-server 与 TrustTunnel endpoint：`cargo test -p rurge-interop` 会正常通过，sing-box 的二十四个互操作用例、xray 的两个、shadowsocks-rust 的两个、snell-server 的四个与 TrustTunnel endpoint 的四个互操作用例各打印一行 `skipping …` 后直接返回（各夹具自身的单元测试——配置渲染、"配置绝不碰本机"的安全守卫、取空闲端口——照常运行并断言）。要在本机真正跑 sing-box 的用例，二选一：
```

换成

```markdown
本地默认不安装 sing-box、xray、shadowsocks-rust、snell-server、TrustTunnel endpoint 与 tuic-server：`cargo test -p rurge-interop` 会正常通过，sing-box 的二十七个互操作用例、xray 的两个、shadowsocks-rust 的两个、snell-server 的四个、TrustTunnel endpoint 的四个与 tuic-server 的三个互操作用例各打印一行 `skipping …` 后直接返回（各夹具自身的单元测试——配置渲染、"配置绝不碰本机"的安全守卫、取空闲端口——照常运行并断言）。要在本机真正跑 sing-box 的用例，二选一：
```

`tests/interop/README.md`——把

```markdown
xray、shadowsocks-rust、snell-server 与 TrustTunnel endpoint 的本机运行方式同理，见下面「xray」「shadowsocks-rust」「snell-server」「TrustTunnel」四节。
```

换成

```markdown
xray、shadowsocks-rust、snell-server、TrustTunnel endpoint 与 tuic-server 的本机运行方式同理（tuic-server 只认环境变量，不在 `PATH` 上找），见下面「xray」「shadowsocks-rust」「snell-server」「TrustTunnel」「tuic-server」五节。
```

`tests/interop/README.md`——把

```markdown
- `RURGE_INTEROP_REQUIRED=1`：找不到二进制时让用例直接失败，而不是打印 `skipping …` 后跳过；对 sing-box、xray、shadowsocks-rust 与 sshd 各夹具都生效，对 snell-server 只在 Linux 上生效（它只有 Linux 版），对 TrustTunnel endpoint 只在 Linux 与 macOS 上生效（它没有 Windows 版）。CI 会设置它，本机一般不需要。
```

换成

```markdown
- `RURGE_TEST_TUIC_SERVER`：tuic 0.8.5 的 `tuic-server` 可执行文件的路径；只认这个变量，不在 `PATH` 上找（`PATH` 上的 `tuic-server` 多半是 1.x 版，只说 TUIC v5，配置也不同）。
- `RURGE_INTEROP_REQUIRED=1`：找不到二进制时让用例直接失败，而不是打印 `skipping …` 后跳过；对 sing-box、xray、shadowsocks-rust 与 sshd 各夹具都生效，对 snell-server 只在 Linux 上生效（它只有 Linux 版），对 TrustTunnel endpoint 只在 Linux 与 macOS 上生效（它没有 Windows 版），对 tuic-server 只在 Linux 上生效（CI 只在 Linux 上装它）。CI 会设置它，本机一般不需要。
```

`tests/interop/README.md`——把

```markdown
**不覆盖端口跳跃**：`port-hopping` 要在服务端用防火墙把一段端口 DNAT 到它的监听端口（Hysteria 文档的部署方式），夹具不改本机的防火墙与网络设置，回环上做不到；端口跳跃由回环假服务端（`rurge_proto_quic::testing::FakeHysteria2` 同时监听几个端口，跳跃间隔经只用于测试的入口缩短）与手工验收（`docs/acceptance/phase2-manual.md` 的 M7a 一节）覆盖。
```

换成

```markdown
- TUIC v5（`tests/tuic.rs` 里 `v5_…_against_sing_box` 的三个用例，阶段 2 / M7b）：sing-box 的 `tuic` 入站（只实现 v5）只听回环上的一个 UDP 端口，证书同 Hysteria 2 一节；`tls.alpn` 写 `["h3"]`（rurge 没写 `alpn` 时发 `h3`，QUIC 服务端不选协议时握手失败）；一个用户（`users[]` 的 `uuid` 与 `password`）；拥塞控制、认证时限（3 秒）、心跳（10 秒）与 0-RTT（关）都用 sing-box 的缺省。覆盖 TCP 的单块、跨多块（100 000 字节）与每个方向 9 MiB 同时往返（超过 rurge 每条流 8 MiB 的接收窗口）；UDP：同一条连接上两个载体（各占一个关联号）各自往返几个字节与 3000 字节的数据报，后者两个方向都按 TUIC 的分片走（sing-box 把应答切成至多 1197 字节的数据报）；口令错误：sing-box 不回应 v5 的认证，令牌不对时以码 0、空原因关闭连接（它因任何错误关连接都是这样），而且常常先结束手上挂着的 Connect（STOP_SENDING 0 加 FIN），所以 TCP 隧道照样先建立（v5 的隧道一律立即交出），然后什么也没转发就结束——关闭先到时读到 `tuic: authentication failed`，Connect 的结束先到时读到流的结束，写可能遇到 `tuic: the server stopped the stream (code 0x0)`；UDP 没有这种竞争，发现连接被这样关闭的那次发送以 `tuic: authentication failed` 失败。**只覆盖 `native` 模式**：sing-box 的 TUIC 服务端总是接受 QUIC 数据报（配置里没有关掉它的项），`quic` 模式（服务端不接受数据报时每个包一条单向流）只由回环假服务端（`rurge_proto_quic::testing::FakeTuic` 的 `no_datagrams`）覆盖。

**不覆盖端口跳跃**：`port-hopping`（Hysteria 2 与 TUIC）要在服务端用防火墙把一段端口 DNAT 到它的监听端口（Hysteria 文档的部署方式），夹具不改本机的防火墙与网络设置，回环上做不到；端口跳跃由回环假服务端（`rurge_proto_quic::testing::FakeHysteria2` 与 `FakeTuic` 同时监听几个端口，跳跃间隔经只用于测试的入口缩短）与手工验收（`docs/acceptance/phase2-manual.md` 的 M7a 与 M7b 两节）覆盖。
```

`tests/interop/README.md`——把

```markdown
渲染出的配置（`rurge_interop::trusttunnel::render`）是三个 TOML 文件：`vpn.toml`（`listen_address = "127.0.0.1:<端口>"`、`ipv6_available = false`、只开 `[listen_protocols.http2]`、`[forward_protocol]` 为 `direct`，以及 `allow_private_network_connections = true`——默认值 `false` 会让 endpoint 拒绝连回环上的 echo）、`hosts.toml`（一个主机 `tt.test` 与证书、私钥的绝对路径）与 `credentials.toml`（一个用户）。夹具的单元用例 `the_configuration_stays_on_the_loopback` 断言只监听回环、只开 HTTP/2。启动命令是 `trusttunnel_endpoint vpn.toml hosts.toml`，就绪与否看 TCP 端口能否连上。

## sshd
```

换成

```markdown
渲染出的配置（`rurge_interop::trusttunnel::render`）是三个 TOML 文件：`vpn.toml`（`listen_address = "127.0.0.1:<端口>"`、`ipv6_available = false`、只开 `[listen_protocols.http2]`、`[forward_protocol]` 为 `direct`，以及 `allow_private_network_connections = true`——默认值 `false` 会让 endpoint 拒绝连回环上的 echo）、`hosts.toml`（一个主机 `tt.test` 与证书、私钥的绝对路径）与 `credentials.toml`（一个用户）。夹具的单元用例 `the_configuration_stays_on_the_loopback` 断言只监听回环、只开 HTTP/2。启动命令是 `trusttunnel_endpoint vpn.toml hosts.toml`，就绪与否看 TCP 端口能否连上。

## tuic-server

TUIC 官方 [tuic](https://github.com/tuic-protocol/tuic) 的服务端 `tuic-server` 是 `tuic`（TUIC v4）的参照：sing-box 只实现 v5（阶段 2 / M7 设计 M7-D3、M7-D6）。它是 GPL-3.0 程序，只作为外部进程运行，rurge 不链接、不复制它的任何代码。互操作测试固定 **0.8.5**（2022-07-30 发布），只在 Linux 上用这一个资产：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (x86_64，静态链接的 musl 版) | `tuic-server-0.8.5-x86_64-linux-musl`（可执行文件本身，不是压缩包） | **尚未钉住，待项目所有者填写**（见下） |

**SHA-256 尚未钉住**：GitHub 发布 API 给这个版本的每个资产的 `digest` 都是 `null`（2022 年上传的资产没有），发布页也不显示校验和，而本仓库的规矩是不在本机下载二进制来自己算。所以 `.github/workflows/ci.yml` 里这一步的 `sha=` 是一个占位值，**在项目所有者填上真实的值之前，Linux 的 CI 任务会在这一步失败**（Windows 与 macOS 的任务不受影响）。发布里每个二进制旁边都有一个 `sha256sum` 格式的同名校验文件（`tuic-server-0.8.5-x86_64-linux-musl.sha256sum`）：在可信的机器上读它，或下载该二进制自己算一次，把 64 位十六进制填进 `sha=`，再填进上表。

`tests/tuic.rs` 里 `v4_…_against_tuic_server` 的三个用例覆盖：TCP 的单块与跨多块（100 000 字节）往返（每条隧道都在服务端应答 `04 ff 00` 之后才交出）；UDP 往返一个回环 UDP 回显两次（v4 不分片，整包走 QUIC 数据报；服务端经它那一个双栈 socket 把 IPv4 来源写成 IPv4 映射的 IPv6 地址，rurge 按 IPv4 读，用例核对来源就是回显的地址）；token 错误时 TCP 请求以 `tuic: authentication failed` 失败（服务端以 0xfffffff1 关闭连接），UDP 发送同样。0.8.5 同样总是接受 QUIC 数据报，**`quic` 模式只由回环假服务端覆盖**。

CI 只在 Linux 上下载、校验并设置 `RURGE_TEST_TUIC_SERVER`；Windows 与 macOS 上这三个用例照常编译，运行时打印 `skipping …` 后返回（`RURGE_INTEROP_REQUIRED=1` 对 tuic-server 只在 Linux 上生效）。本机不安装它：只认 `RURGE_TEST_TUIC_SERVER`，不在 `PATH` 上找（`PATH` 上的 `tuic-server` 多半是 1.x 版，只说 v5，配置也不同），没设就跳过；这个 crate 不会下载或安装它。

渲染出的配置（`rurge_interop::tuic_server::render`）是一个 JSON 对象，只有 0.8.5 认识的七个键（它拒绝不认识的键）：`port`、`token`（一个 token）、`certificate` 与 `private_key`（夹具的证书与私钥，PEM 文件的路径）、`ip`（`127.0.0.1`）、`alpn`（`["h3"]`：它的缺省是不选 ALPN，rurge 的 QUIC 客户端于是握手失败）与 `log_level`（`info`）；其余用它的缺省（Cubic、空闲 15 秒、认证时限 1 秒）。启动命令是 `tuic-server -c <配置文件>`；它没有 TCP 端口可探，就绪与否看日志里出现 `Server started`（`info` 级，绑定端口之后才打印）。夹具的单元用例 `the_configuration_stays_on_the_loopback` 与 `the_configuration_is_written_as_tuic_server_reads_it` 断言只监听回环、只写这七个键。它转发 UDP 用的 socket 绑在所有地址上的一个空闲端口（`[::]:0`；0.8.5 总是这样，没有配置项）。

## sshd
```

`tests/interop/README.md`——把

```markdown
- 夹具渲染出的 sing-box 配置只有 `log` / `inbounds` / `outbounds` 三个顶层键（WireGuard 的配置另有 `endpoints`）；每个入站只监听 `127.0.0.1`（`hysteria2` 入站的 UDP 端口也是，同一份配置里另有一个只用来判断就绪的 `mixed` 入站；`rurge_interop::render_hysteria2` 的单元测试 `the_hysteria2_configuration_never_touches_the_machine` 断言这些）；唯一的出站是 `direct`。WireGuard 端点在用户态运行（`system: false`），它的 UDP 端口开在所有地址上（端点没有监听地址这一项；`rurge_interop::render_wireguard` 的单元测试 `the_wireguard_configuration_never_touches_the_machine` 断言其余各项）。xray 配置同样只有这三个顶层键，唯一的出站是 `freedom`。`ssserver` 的配置只有 `servers`，每个服务端只听 `127.0.0.1`（TCP 与同号的 UDP）。`snell-server` 的配置只有 `[snell-server]` 一节，只听 `127.0.0.1`。TrustTunnel endpoint 只听 `127.0.0.1`、只开 HTTP/2，转发只有 `direct`；`allow_private_network_connections = true` 只是为了让它连回环上的 echo。任何地方都不出现 `set_system_proxy`、`tun`、`auto_route` 这些键（`rurge_interop::render` 与 `rurge_interop::xray::render` 的单元测试 `the_configuration_never_touches_the_machine` 各自断言这一点）；`shadowtls` 入站的 `handshake.server` 恒为 `127.0.0.1`（夹具的单元用例断言）。
- 每个用例的连接目标都是回环 IP 字面量（`127.0.0.1` 上的 echo / 测试服务器；WireGuard 用例在隧道里连的是 sing-box 自己的隧道地址 `10.9.0.1`，由 sing-box 映射到它的 `127.0.0.1`），sing-box、xray、`ssserver`、`snell-server` 与 TrustTunnel endpoint 因此既不解析域名也不会访问公网。
- 这个 crate 本身不下载、不安装任何东西；本机是否装有 sing-box、xray、shadowsocks-rust、snell-server 或 TrustTunnel endpoint 由项目所有者决定，没装就跳过。
```

换成

```markdown
- 夹具渲染出的 sing-box 配置只有 `log` / `inbounds` / `outbounds` 三个顶层键（WireGuard 的配置另有 `endpoints`）；每个入站只监听 `127.0.0.1`（`hysteria2` 与 `tuic` 入站的 UDP 端口也是，同一份配置里另有一个只用来判断就绪的 `mixed` 入站；`rurge_interop::render_hysteria2` 与 `rurge_interop::render_tuic` 的单元测试 `the_hysteria2_configuration_never_touches_the_machine` 与 `the_tuic_configuration_never_touches_the_machine` 断言这些）；唯一的出站是 `direct`。WireGuard 端点在用户态运行（`system: false`），它的 UDP 端口开在所有地址上（端点没有监听地址这一项；`rurge_interop::render_wireguard` 的单元测试 `the_wireguard_configuration_never_touches_the_machine` 断言其余各项）。xray 配置同样只有这三个顶层键，唯一的出站是 `freedom`。`ssserver` 的配置只有 `servers`，每个服务端只听 `127.0.0.1`（TCP 与同号的 UDP）。`snell-server` 的配置只有 `[snell-server]` 一节，只听 `127.0.0.1`。TrustTunnel endpoint 只听 `127.0.0.1`、只开 HTTP/2，转发只有 `direct`；`allow_private_network_connections = true` 只是为了让它连回环上的 echo。tuic-server 的配置只有它认识的七个键，只听 `127.0.0.1`（它转发 UDP 的 socket 绑在所有地址上的一个空闲端口，0.8.5 没有配置项）。任何地方都不出现 `set_system_proxy`、`tun`、`auto_route` 这些键（`rurge_interop::render` 与 `rurge_interop::xray::render` 的单元测试 `the_configuration_never_touches_the_machine` 各自断言这一点）；`shadowtls` 入站的 `handshake.server` 恒为 `127.0.0.1`（夹具的单元用例断言）。
- 每个用例的连接目标都是回环 IP 字面量（`127.0.0.1` 上的 echo / 测试服务器；WireGuard 用例在隧道里连的是 sing-box 自己的隧道地址 `10.9.0.1`，由 sing-box 映射到它的 `127.0.0.1`），sing-box、xray、`ssserver`、`snell-server`、TrustTunnel endpoint 与 `tuic-server` 因此既不解析域名也不会访问公网。
- 这个 crate 本身不下载、不安装任何东西；本机是否装有 sing-box、xray、shadowsocks-rust、snell-server、TrustTunnel endpoint 或 tuic-server 由项目所有者决定，没装就跳过。
```

兼容性清单（4.2 的 `tuic` / `tuic-v5` 行与 `W0007` 一行、4.3 的 `tfo` 与 `underlying-proxy`、4.4 的 `alpn` 与 Shadow TLS 约束、4.5 的 UDP、4.6 的参数行）：

`docs/surge-compatibility-matrix.md`——把

```markdown
| `tuic` / `tuic-v5` | TUIC v4（token）/ v5（uuid + password） | 全部 | ✅ | 2 | |
```

换成

```markdown
| `tuic` / `tuic-v5` | TUIC v4（token）/ v5（uuid + password） | 全部 | ✅ | 2 | M7b（阶段 2）已实现两个版本的 TCP 与 UDP：`tuic` 是 tuic 0.8.5 所说的 TUIC v4，`tuic-v5` 是 v5（SPEC），两者不能互换（照手册）。**连接**与 `hysteria2` 相同（见下一行的「连接」）：每个策略一条 QUIC 连接、多路复用（每个 TCP 连接一条双向流，UDP 走同一条连接），第一次用到时才拨号，同时到来的请求共用一次拨号与认证、失败一并报给等着的请求（不退避），只用 TLS 1.3、不用 0-RTT，空闲超时 30 秒、保活 10 秒，没有流也没有 UDP 载体满 60 秒的连接主动关闭，`underlying-proxy` 是组时跟随组的当前成员（日志 `tuic: the underlying policy changed; a new connection is made`），服务端重启与休眠唤醒后的情形也一样；TUIC 没有 GOAWAY。**认证**：连接建好后在一条单向流上写出认证命令并结束这条流，不等任何回应（协议没有回应），连接上的请求随即跟上——v5 是 `05 00 ‖ UUID ‖ TOKEN`，TOKEN 是这条连接的 TLS 会话导出的 32 字节（RFC 5705 的导出器，label 是 UUID 的 16 个原始字节，context 是口令）；v4 是 `04 00 ‖ BLAKE3(token)`（同 0.8.5 的服务端）。**认证失败怎样表现**：v4 的服务端以 0xfffffff1 关闭连接，是 `tuic: authentication failed`（0xfffffff2 是 `tuic: authentication timed out`，0xfffffff0 与 0xfffffff3 是 `tuic: protocol error`，这四个码都不引用服务端给的原因；其它码是 `tuic: the server closed the connection (code 0x…: <原因>)`）；v5 的规范没有定义错误码，tuic-server 1.0.0 拒绝认证时、sing-box 因任何错误关闭连接时都用码 0、空原因，所以 rurge 只在这样的关闭来在服务端转发过任何东西（隧道读到目标的字节、UDP 回包）之前时报 `tuic: authentication failed`，否则报 `tuic: the server closed the connection (code 0x0)`——这是推断（登记的差异）：sing-box 的会话结束（服务端停机）或它 3 秒的认证时限在什么都还没转发的新连接上同样读作认证失败；而且 sing-box 拒绝认证时常常先结束手上挂着的 Connect（STOP_SENDING 0 加 FIN）再关连接，这时 TCP 隧道读到的是流的结束、写遇到的是 `tuic: the server stopped the stream (code 0x0)`，不是 `tuic: authentication failed`。经引擎时，`tuic-v5` 口令错误的 CONNECT 先回 200（v5 的 Connect 没有应答，隧道立即交出），之后隧道断开，请求记录写 `tuic: authentication failed`；`tuic` 的 token 错误时 CONNECT 回 502（v4 的 Connect 等应答，同 `hysteria2`）。UDP 载体发现连接这样被拒时，那一次发送以 `tuic: authentication failed` 失败（引擎的流随之失败），下一次发送重新拨号——口令错误不会让 UDP 静默黑洞。`tuic-v5` 的行连 v4 的服务端时是 `tuic: the server closed the connection (code 0xfffffff0: …)`（v4 的码对 v5 没有含义）。**TCP**：每个连接一条双向流，开流即写出 `VER 01 ‖ 目标`（目标：`00` 名字（长度字节 + A-label，字母表规则同 `hysteria2`；写不出的名字不拨号即失败 `tuic: the host name cannot be sent to the server`，超过 255 字节是 `tuic: the host name is longer than 255 bytes`）、`01` IPv4、`02` IPv6，端口在后）。v5 的服务端不回应：流立即作为隧道交出（Connect 不等首段负载，服务端先说话的协议照常可用），目标连不上表现为流的结束；v4 等同一条流上的应答 `04 ff ‖ REP`：`00` 才交出隧道，`ff` 是 `tuic: the server could not connect to the target`（连接照常），其它字节是 `tuic: the server's response is malformed`，应答之前流就结束是 `tuic: the server ended the stream without a response`（0.8.5 解析目标名失败时就是这样）。失效连接重试一次（见 `hysteria2` 一行）：v4 在应答到达之前都算（服务端拒绝认证恰好撞上在已建好的连接上等应答的请求时，多拨一次、结果相同）；v5 只到 Connect 交给 QUIC 为止——之后隧道已归调用方，经已被服务端忘掉的连接（服务端重启）的隧道随连接结束、不重试（登记的差异）。流上的错误：`tuic: the server reset the stream (code 0x…)`、`tuic: the server stopped the stream (code 0x…)`、`tuic: the stream is closed`。**UDP**（自动，不需要参数）：每个 UDP 载体（"关联 × 出站"）是策略连接上的一个关联，全锥。**模式自动选择**（手册没有 UDP 模式参数，登记的差异）：服务端接受 QUIC 数据报时走 `native`（数据报），否则走 `quic`（每个包整包一条单向流，写完即结束）；按连接定，回包两种都收。`native` 下，v5 放不进一个数据报的包分片发出（只有第一片带地址，其余写 `0xff`；至多 255 片；包号每个载体从 0 递增；一个载体同一时刻只发一个包的片、片不交错——Clash.Meta 早期的 TUIC v5 服务端一次只重组一个包），255 片也装不下的包与名字长得第一片连头都放不下的包不发送（`tuic: a datagram longer than <n> bytes` / `tuic: the host name is too long for a datagram`，只丢那一个，流照常）；**v4 不分片：放不进一个数据报的包不发送**（`tuic: a datagram longer than <n> bytes`，只丢那一个，流照常，记录上写明；一个 v4 数据报装得下的负载起初约 1150 字节、路径 MTU 探测之后至多约 1.4 KB，1200 字节以上的 QUIC Initial 经 v4 因此可能发不出去，是 v4 协议本身的限制）；超过 65535 字节的包两种模式都不发送。收到的分片每个关联同时至多重组 8 个包（第 9 个挤掉最旧的），任一片丢失整个包丢弃；服务端发来的畸形消息丢弃（sing-box 自己的客户端会关连接），服务端的心跳忽略。`quic` 模式的包交给每条连接一个的写任务，按序一条流写完再写下一条（被放弃的发送因此不会在线上留下半个包——sing-box 收到半个包会关掉整条连接，连同其上的全部隧道与关联）：发送从不等待，已有 64 个在排队时再来的包丢弃（debug 日志）；**代价：服务端不再读流时 `quic` 模式的 UDP 静默丢包，引擎 15 秒的发送时限（`SEND_TIMEOUT`）在这里不再起作用**（登记的差异），同一连接上的 TCP 不受影响。**关联号按连接分配**：v5 每条连接从 0 递增（16 位）、跳过在用的；v4 随机取 32 位（同 0.8.5 的客户端）、在用时重取；载体换到新连接（旧的关闭、服务端重启、底层策略换了）时在新连接上取新号（服务端视为新关联，全锥的映射随之改变；`hysteria2` 沿用会话号）；载体关闭或离开仍开着的连接时，在一条单向流上发 `Dissociate`（`VER 03 ‖ 关联号`）。认证之前即可发 UDP（sing-box 把认证完成前到达的数据报留在 quic-go 的队列里，至多 128 个）。回包来源是 IPv4 映射的 IPv6 地址时按 IPv4 读（0.8.5 的服务端经一个双栈 socket 这样写 IPv4 来源）。**心跳**（内定值，手册没有参数，登记的差异）：有 TCP 隧道或 UDP 关联在用时，v5 每 3 秒发一个心跳数据报（`05 04`，同 tuic-client 1.0.0：tuic-server 的空闲超时是 10 秒，QUIC 10 秒一次的保活来不及），服务端不接受 QUIC 数据报时不发（规范只定义数据报的心跳，sing-box 收到流上的心跳会关连接）——这时只靠 QUIC 的保活，空闲超时 10 秒的服务端（tuic-server 1.x）可能关掉安静的连接；v4 每 10 秒在一条单向流上发 `04 04`（同 0.8.5 的客户端；它的服务端只在流上收心跳，空闲超时 15 秒），不论服务端接不接受数据报。错误文本、日志与 `Debug` 不含 token、UUID、口令与载荷。测速经出站本身（URL 测速的两次 `HEAD` 在同一条隧道里）；重载时参数没变的策略沿用出站与它的 QUIC 连接（凭据、版本、`ecn`、`port-hopping` 变了就重建）。参数见 4.6；不能叠 Shadow TLS（4.4）。互操作参照：sing-box 1.14.2 的 `tuic` 入站（v5，只能覆盖 `native` 模式），tuic 0.8.5 的服务端（v4，只在 Linux CI） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| 策略指向 rurge 尚未实现的协议类型 | Surge 原生支持全部协议 | 全部 | 🟡 | 1 / 2 | 加载时告警 `W0007`；运行时该策略按 `REJECT` 处理，会话日志 `error = policy protocol not implemented: <type>`；随阶段 2 各里程碑逐协议移除：M1 已移除 `http` `https` `socks5` `socks5-tls`；M2a 已移除 `trojan`；M2b 已移除 `vmess`（写了 `vmess-aead=true` 的行）与 `anytls`；M4a 已移除 `ssh`；M4b 已移除 `wireguard`；M4c 已移除 `external`；M6a 已移除 `ss`（流式旧方法除外）；M6b 已移除 `snell`（v4 / v5）；M6c 已移除 `h2-connect` 与 `trust-tunnel`（HTTP/2；`h3=true` 是 `W0029`，不是 `W0007`）；M7a 已移除 `hysteria2`。`W0007` 与 `OutboundError::Unsupported` 在 M8 退役（M7 细化设计 M7-D12：M7 之后仍有 `tailscale`、ss 流式旧方法、vmess 旧握手与 Snell v1–v3 / v6）。例外有三个：没写 `vmess-aead=true` 的 `vmess` 行仍按 `W0007` 处理，但走专门的诊断文本 `` `vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet ``（每次加载一条，不是每行一条）与专门的会话日志文本 `policy protocol not implemented: vmess (legacy handshake)`；流式旧方法的 `ss` 行同样如此，诊断文本 `` `ss` stream cipher `<method>` is not implemented yet; such policies behave as REJECT ``（每种方法每次加载一条），会话日志 `policy protocol not implemented: ss (<method>)`；`version` 为 1–3 或 6（含没写 `version` 的，Surge 的缺省是 1）的 `snell` 行也是，诊断文本 `` `snell` version <n> is not implemented; rurge supports versions 4 and 5 (`version` must match the server); such policies behave as REJECT ``（每个版本每次加载一条），会话日志 `policy protocol not implemented: snell v<n>`；三者都不是这里的通用 `<type>` 模板 |
```

换成

```markdown
| 策略指向 rurge 尚未实现的协议类型 | Surge 原生支持全部协议 | 全部 | 🟡 | 1 / 2 | 加载时告警 `W0007`；运行时该策略按 `REJECT` 处理，会话日志 `error = policy protocol not implemented: <type>`；随阶段 2 各里程碑逐协议移除：M1 已移除 `http` `https` `socks5` `socks5-tls`；M2a 已移除 `trojan`；M2b 已移除 `vmess`（写了 `vmess-aead=true` 的行）与 `anytls`；M4a 已移除 `ssh`；M4b 已移除 `wireguard`；M4c 已移除 `external`；M6a 已移除 `ss`（流式旧方法除外）；M6b 已移除 `snell`（v4 / v5）；M6c 已移除 `h2-connect` 与 `trust-tunnel`（HTTP/2；`h3=true` 是 `W0029`，不是 `W0007`）；M7a 已移除 `hysteria2`；M7b 已移除 `tuic` 与 `tuic-v5`（此前格式不对的这两种行只报 `W0007`，现在是加载错误 `E0018`）。`W0007` 与 `OutboundError::Unsupported` 在 M8 退役（M7 细化设计 M7-D12：M7 之后仍有 `tailscale`、ss 流式旧方法、vmess 旧握手与 Snell v1–v3 / v6）。例外有三个：没写 `vmess-aead=true` 的 `vmess` 行仍按 `W0007` 处理，但走专门的诊断文本 `` `vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet ``（每次加载一条，不是每行一条）与专门的会话日志文本 `policy protocol not implemented: vmess (legacy handshake)`；流式旧方法的 `ss` 行同样如此，诊断文本 `` `ss` stream cipher `<method>` is not implemented yet; such policies behave as REJECT ``（每种方法每次加载一条），会话日志 `policy protocol not implemented: ss (<method>)`；`version` 为 1–3 或 6（含没写 `version` 的，Surge 的缺省是 1）的 `snell` 行也是，诊断文本 `` `snell` version <n> is not implemented; rurge supports versions 4 and 5 (`version` must match the server); such policies behave as REJECT ``（每个版本每次加载一条），会话日志 `policy protocol not implemented: snell v<n>`；三者都不是这里的通用 `<type>` 模板 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `tfo` | 布尔；默认 false | 🟡 | 2 | 解析并校验，`W0029`，三平台都不生效：`socket2` 0.6 没有 TFO 的安全封装，不为此引入 unsafe；有安全封装后再评估。QUIC 类（M7a 起 `hysteria2`）不开 TCP 连接，写了是 `W0028` 并忽略 |
| `tos` | 0–255 或 `0x` 十六进制；默认 0 | 🟡 | 2 | M1 已实现；Windows 上对 IPv6 不生效；有 underlying-proxy 时无效（socket 选项属于真正打开 socket 的那一跳）；M2a 起加载时报 W0028。QUIC 类（M7a）的直连 socket 同样带上它，代价是发出的包没有 ECN 标记、没有 GSO（见 `ecn` 一行） |
| `ecn` | `auto` `on` `off`；QUIC 类协议默认开启，WireGuard/Tailscale 默认关闭 | 🟡 | 2 | M1 解析并校验取值。M7a 起在 `hysteria2` 上生效（M7-D4，跟随 quinn）：`auto` 与 `on` 都取 quinn 的缺省——给发出的包标 ECT(0)，对端的 ACK 不带 ECN 计数或计数校验失败时自行停用；quinn 没有"强制开启"，**`on` 等同 `auto`**；`off` 在 UDP 层去掉发出与收到两侧的 ECN 标记，quinn 的 ECN 校验随之停用。**写了 `tos`（非 0）时**直连的 QUIC socket 发送不经 quinn-udp（它在 Linux / macOS 上给每个包写的 TOS 只有 ECN 两位，会把 DSCP 清零），所以**发出的包没有 ECN 标记、也没有 GSO**；接收照常经 quinn-udp（读对端的 ECN 标记，照常做路径 MTU 探测）；三个平台一样。经 `underlying-proxy` 的载体没有 ECN。TCP 类协议与 `wireguard` 上仍只解析（`W0029`；手册说它对纯 TCP 协议不起作用） |
```

换成

```markdown
| `tfo` | 布尔；默认 false | 🟡 | 2 | 解析并校验，`W0029`，三平台都不生效：`socket2` 0.6 没有 TFO 的安全封装，不为此引入 unsafe；有安全封装后再评估。QUIC 类（M7a 起 `hysteria2`，M7b 起 `tuic` / `tuic-v5`）不开 TCP 连接，写了是 `W0028` 并忽略 |
| `tos` | 0–255 或 `0x` 十六进制；默认 0 | 🟡 | 2 | M1 已实现；Windows 上对 IPv6 不生效；有 underlying-proxy 时无效（socket 选项属于真正打开 socket 的那一跳）；M2a 起加载时报 W0028。QUIC 类（M7a 起 `hysteria2`，M7b 起 `tuic` / `tuic-v5`）的直连 socket 同样带上它，代价是发出的包没有 ECN 标记、没有 GSO（见 `ecn` 一行） |
| `ecn` | `auto` `on` `off`；QUIC 类协议默认开启，WireGuard/Tailscale 默认关闭 | 🟡 | 2 | M1 解析并校验取值。M7a 起在 `hysteria2` 上、M7b 起在 `tuic` / `tuic-v5` 上生效（M7-D4，跟随 quinn）：`auto` 与 `on` 都取 quinn 的缺省——给发出的包标 ECT(0)，对端的 ACK 不带 ECN 计数或计数校验失败时自行停用；quinn 没有"强制开启"，**`on` 等同 `auto`**；`off` 在 UDP 层去掉发出与收到两侧的 ECN 标记，quinn 的 ECN 校验随之停用。**写了 `tos`（非 0）时**直连的 QUIC socket 发送不经 quinn-udp（它在 Linux / macOS 上给每个包写的 TOS 只有 ECN 两位，会把 DSCP 清零），所以**发出的包没有 ECN 标记、也没有 GSO**；接收照常经 quinn-udp（读对端的 ECN 标记，照常做路径 MTU 探测）；三个平台一样。经 `underlying-proxy` 的载体没有 ECN。TCP 类协议与 `wireguard` 上仍只解析（`W0029`；手册说它对纯 TCP 协议不起作用） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `underlying-proxy` | 另一策略或策略组名；仅代理策略；不能与 `port-hopping` 同用 | ✅ | 2 | 目标代理主机名在上游远程解析；M1 已实现（TCP）：底层可以是策略或组，按名字在拨号时解析，组的选择变了链的入口随之变；成环是 `E0019`；链上某一跳失败时错误文本带 `via <名字>:` 前缀；中继是没有成员的组（订阅还没下载到、过滤滤光了）时，经它的连接一律失败（`via <组名>: policy group has no members`），不回退 DIRECT，也不看 `--empty-group-reject`（阶段 2 / M3b：设中继就是不让流量直出；Surge 未说明）。QUIC 类（M7a 起 `hysteria2`）经底层策略的 UDP 载体连服务器（底层策略不载 UDP 时拨号失败，同 M5c；这种载体不做路径 MTU 探测，QUIC 包停在 1200 字节，也没有 ECN）；与 `port-hopping` 同写是 `E0018`（`` `port-hopping` cannot be combined with `underlying-proxy` ``；写 `underlying-proxy=DIRECT` 不算链，写组名算）；组级 `underlying-proxy` 派生的 `M (via R)` 里，成员 `M` 写了 `port-hopping` 时这个派生成员构建失败、从组里略去（日志 `policy cannot be built; it is left out`，同 M3a 里其它构建不了的派生成员），组照常用其余成员；配置本身给组的这类成员（行上写的、`include-other-group` 与 `include-all-proxies` 取来的）在加载时各报一条 `W0023`（`` policy group `G` relays its members through `underlying-proxy`: `M` uses `port-hopping`, which cannot go through it; `M (via R)` is left out ``，`rurge check` 可见），订阅带来的加载时不知道、只记日志；成员全被略去时组为空，与其它空组一样由 DIRECT 代替，`--empty-group-reject` 时 REJECT（见 5.1 节「嵌套与循环」） |
```

换成

```markdown
| `underlying-proxy` | 另一策略或策略组名；仅代理策略；不能与 `port-hopping` 同用 | ✅ | 2 | 目标代理主机名在上游远程解析；M1 已实现（TCP）：底层可以是策略或组，按名字在拨号时解析，组的选择变了链的入口随之变；成环是 `E0019`；链上某一跳失败时错误文本带 `via <名字>:` 前缀；中继是没有成员的组（订阅还没下载到、过滤滤光了）时，经它的连接一律失败（`via <组名>: policy group has no members`），不回退 DIRECT，也不看 `--empty-group-reject`（阶段 2 / M3b：设中继就是不让流量直出；Surge 未说明）。QUIC 类（M7a 起 `hysteria2`，M7b 起 `tuic` / `tuic-v5`）经底层策略的 UDP 载体连服务器（底层策略不载 UDP 时拨号失败，同 M5c；这种载体不做路径 MTU 探测，QUIC 包停在 1200 字节，也没有 ECN）；与 `port-hopping` 同写是 `E0018`（`` `port-hopping` cannot be combined with `underlying-proxy` ``；写 `underlying-proxy=DIRECT` 不算链，写组名算）；组级 `underlying-proxy` 派生的 `M (via R)` 里，成员 `M` 写了 `port-hopping` 时这个派生成员构建失败、从组里略去（日志 `policy cannot be built; it is left out`，同 M3a 里其它构建不了的派生成员），组照常用其余成员；配置本身给组的这类成员（行上写的、`include-other-group` 与 `include-all-proxies` 取来的）在加载时各报一条 `W0023`（`` policy group `G` relays its members through `underlying-proxy`: `M` uses `port-hopping`, which cannot go through it; `M (via R)` is left out ``，`rurge check` 可见），订阅带来的加载时不知道、只记日志；成员全被略去时组为空，与其它空组一样由 DIRECT 代替，`--empty-group-reject` 时 REJECT（见 5.1 节「嵌套与循环」） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `alpn` | 协议列表；TUIC / Hysteria 2 / MASQUE 默认 `h3` | ✅ | 2 | M1 已实现：`https` / `socks5-tls` 握手不带 ALPN，除非写了 `alpn`（未与真实 Surge 核对）；M6c：`h2-connect` / `trust-tunnel` 固定 `h2`，写了别的取值 `W0028` 并忽略；M7a：`hysteria2` 没写 `alpn` 时发 `h3`（照手册），写了就发所写的列表；QUIC 的握手只用 TLS 1.3；服务端不选 ALPN 时握手失败——rustls 一类的服务端没有相同的协议时直接拒绝（`tls: the server refused the handshake (NoApplicationProtocol)`），完成握手却不选协议的服务端同样被拒（登记的差异） |
```

换成

```markdown
| `alpn` | 协议列表；TUIC / Hysteria 2 / MASQUE 默认 `h3` | ✅ | 2 | M1 已实现：`https` / `socks5-tls` 握手不带 ALPN，除非写了 `alpn`（未与真实 Surge 核对）；M6c：`h2-connect` / `trust-tunnel` 固定 `h2`，写了别的取值 `W0028` 并忽略；M7a：`hysteria2` 没写 `alpn` 时发 `h3`（照手册），写了就发所写的列表；QUIC 的握手只用 TLS 1.3；服务端不选 ALPN 时握手失败——rustls 一类的服务端没有相同的协议时直接拒绝（`tls: the server refused the handshake (NoApplicationProtocol)`），完成握手却不选协议的服务端同样被拒（登记的差异）；M7b：`tuic` / `tuic-v5` 同样没写时发 `h3`；TUIC 官方的服务端（0.8.5 与 tuic-server 1.x）缺省不配 ALPN、也就不选协议，这样的服务端 rurge 连不上，要在服务端配上 `h3`（手册说 `alpn` 须与服务端的设置相符） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| 约束：Shadow TLS 不能与 TUIC / WireGuard / Tailscale / 其他 QUIC 类协议组合 | 配置错误 | ✅ | 2 | 判断函数已落地（`tuic` `tuic-v5` `hysteria2` `masque` `wireguard` `tailscale`：`` E0018 Shadow TLS cannot be combined with a `<type>` policy ``）；`wireguard` 自 M4b 起、`hysteria2` 自 M7a 起真的报出，`tuic` / `tuic-v5` / `masque` 的 spec 出现之前（M7b / M7c）与 `tailscale` 的 spec 出现之前（远期）该错误不会真的报出——它们的整行目前都还没有被读取；`h3=true` 的 `trust-tunnel` 眼下仍按 HTTP/2 连、照常可叠 Shadow TLS，M7c 让它走 QUIC 时须把它加进判断函数 |
```

换成

```markdown
| 约束：Shadow TLS 不能与 TUIC / WireGuard / Tailscale / 其他 QUIC 类协议组合 | 配置错误 | ✅ | 2 | 判断函数已落地（`tuic` `tuic-v5` `hysteria2` `masque` `wireguard` `tailscale`：`` E0018 Shadow TLS cannot be combined with a `<type>` policy ``）；`wireguard` 自 M4b 起、`hysteria2` 自 M7a 起、`tuic` / `tuic-v5` 自 M7b 起真的报出，`masque` 的 spec 出现之前（M7c）与 `tailscale` 的 spec 出现之前（远期）该错误不会真的报出——它们的整行目前都还没有被读取；`h3=true` 的 `trust-tunnel` 眼下仍按 HTTP/2 连、照常可叠 Shadow TLS，M7c 让它走 QUIC 时须把它加进判断函数 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `udp-relay`（布尔；默认 false） | 适用 SOCKS5 / SOCKS5-TLS / Shadowsocks / External / HTTP/2 CONNECT（RFC 9298） | ✅ M5a：`socks5` / `socks5-tls` / `external` 已生效；M6a：Shadowsocks 已生效；M6c：HTTP/2 CONNECT（`h2-connect` 的 CONNECT-UDP，每个目标一条流，对称型；服务端须通告 extended CONNECT，见 4.2 `h2-connect` 一行）已生效；写在 `trust-tunnel` 行上是 `W0028` 并忽略；写在 `hysteria2` 行上同样是 `W0028`（UDP 自动支持，M7a） | 2 |
| `udp-port`（端口；默认主端口） | 适用 Shadowsocks / Snell | ✅ M6a：Shadowsocks 已生效（1–65535，其它取值 `E0018`；没写 `udp-relay=true` 时不用）；M6b：Snell 只解析——写了是 `W0029`，UDP over TCP 一律走主端口（手册用它指服务端另开的 UDP 端口，即 v5 的 QUIC Proxy Mode，rurge 不实现；见 4.2 `snell` 一行） | 2 |
| 自动支持 UDP 的协议：Snell v3+、VMess、Trojan、TUIC、Hysteria 2、MASQUE、AnyTLS（UDP over TCP）、WireGuard、Tailscale | | ✅ M5b：VMess（对称型）、Trojan、AnyTLS 已生效；M5c：WireGuard 已生效；M6b：Snell v4 / v5 已生效（UDP over TCP，全锥；v1–v3 在 M8 之前不实现）；M7a：Hysteria 2 已生效（QUIC 数据报，全锥；服务端在认证时回 `Hysteria-UDP: false` 时打开载体即失败，见 4.2 `hysteria2` 一行）；其余随各自的协议 | 2 |
```

换成

```markdown
| `udp-relay`（布尔；默认 false） | 适用 SOCKS5 / SOCKS5-TLS / Shadowsocks / External / HTTP/2 CONNECT（RFC 9298） | ✅ M5a：`socks5` / `socks5-tls` / `external` 已生效；M6a：Shadowsocks 已生效；M6c：HTTP/2 CONNECT（`h2-connect` 的 CONNECT-UDP，每个目标一条流，对称型；服务端须通告 extended CONNECT，见 4.2 `h2-connect` 一行）已生效；写在 `trust-tunnel` 行上是 `W0028` 并忽略；写在 `hysteria2` 行上同样是 `W0028`（UDP 自动支持，M7a），写在 `tuic` / `tuic-v5` 行上也是（M7b） | 2 |
| `udp-port`（端口；默认主端口） | 适用 Shadowsocks / Snell | ✅ M6a：Shadowsocks 已生效（1–65535，其它取值 `E0018`；没写 `udp-relay=true` 时不用）；M6b：Snell 只解析——写了是 `W0029`，UDP over TCP 一律走主端口（手册用它指服务端另开的 UDP 端口，即 v5 的 QUIC Proxy Mode，rurge 不实现；见 4.2 `snell` 一行） | 2 |
| 自动支持 UDP 的协议：Snell v3+、VMess、Trojan、TUIC、Hysteria 2、MASQUE、AnyTLS（UDP over TCP）、WireGuard、Tailscale | | ✅ M5b：VMess（对称型）、Trojan、AnyTLS 已生效；M5c：WireGuard 已生效；M6b：Snell v4 / v5 已生效（UDP over TCP，全锥；v1–v3 在 M8 之前不实现）；M7a：Hysteria 2 已生效（QUIC 数据报，全锥；服务端在认证时回 `Hysteria-UDP: false` 时打开载体即失败，见 4.2 `hysteria2` 一行）；M7b：TUIC v5 / v4 已生效（全锥；服务端接受 QUIC 数据报时走数据报，否则每个包一条单向流，自动选择；v4 放不进一个数据报的包丢弃，见 4.2 `tuic` / `tuic-v5` 一行）；其余随各自的协议 | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `tuic` / `tuic-v5` | `token`（v4）/ `uuid` + `password`（v5）`alpn`（默认 h3）`port-hopping`（`1234;5000-6000`）`port-hopping-interval`（默认 30） | ✅ | 2 | 自动 UDP；默认 ECN |
```

换成

```markdown
| `tuic` / `tuic-v5` | `token`（v4）/ `uuid` + `password`（v5）`alpn`（默认 h3）`port-hopping`（`1234;5000-6000`）`port-hopping-interval`（默认 30） | ✅ | 2 | M7b 已实现。凭据只读命名写法、都必填：`tuic` 要 `token`，`tuic-v5` 要 `uuid` 与 `password`（缺了是 `E0018` `` `<参数>` is required ``，`token` 与 `password` 为空也算缺，都不引用取值；写在端口之后的位置值是 `W0001`，脱敏时同样抹掉）。`uuid` 接受手册写的带连字符的 8-4-4-4-12 形式，也接受不带连字符的同样 32 位十六进制（同 `vmess` 的 `username`：不区分大小写，两端空白去掉）；别的写法（花括号、`urn:uuid:`、长度或连字符位置不对、非十六进制、空值）是 `E0018` `` `uuid` is not a valid UUID ``，不引用取值。`token` 按写的原样（UTF-8 字节，不去空白）取 BLAKE3 发出（同 0.8.5 的服务端对它的 token 的处理）。另一个版本的凭据写了是 `W0028` 并忽略（`` `uuid` only applies to `tuic-v5` policies; ignored ``、`` `password` only applies to `tuic-v5` policies; ignored `` / `` `token` only applies to `tuic` policies; ignored ``）——v5 的行误写成 `tuic` 时，报缺 `token` 加两条指向 `tuic-v5` 的 `W0028`。`port-hopping` / `port-hopping-interval` 的写法、诊断码与行为同 `hysteria2`（小于 5 秒 `W0012` 并按 5，没写 `port-hopping` 时 `W0028`；与 `underlying-proxy` 同写是 `E0018`；组级 `underlying-proxy` 的组里写了它的成员被略去、加载时 `W0023`，见 4.3）。`udp-relay` 与 `tfo` 写了是 `W0028` 并忽略（`` `udp-relay` does not apply to `tuic` policies (UDP is always relayed); ignored ``：UDP 自动支持；QUIC 不开 TCP 连接）；`ecn` 生效（见 4.3）；TLS 参数适用（4.4，`alpn` 缺省 `h3`）；不能叠 Shadow TLS（`E0018`）。M7b 之前格式不对的这两种行只报 `W0007`，现在是加载错误。手册没有 UDP 模式、心跳、0-RTT 与拥塞控制参数：UDP 模式自动选择、心跳取内定值（见 4.2），不用 0-RTT，拥塞控制固定 Cubic；`token`、`uuid`、`password` 在 `profiles/current` 与 `policies/detail` 里脱敏 |
```

`docs/acceptance/phase2-manual.md`——把

```markdown
- [ ] 脱敏：`GET /v1/policies/detail?policy_name=H` 与 `GET /v1/profiles/current` 里 `password`、`salamander-password` 与 `gecko-password` 都是 `***`。

```

换成

```markdown
- [ ] 脱敏：`GET /v1/policies/detail?policy_name=H` 与 `GET /v1/profiles/current` 里 `password`、`salamander-password` 与 `gecko-password` 都是 `***`。

## M7b　TUIC

前置：同 M5a 一节的 SOCKS5 UDP 客户端；自己的 TUIC v5 节点（记下服务端实现与版本，如 tuic-server 1.x 或 sing-box 的 `tuic` 入站）与 TUIC v4 节点（官方 tuic 0.8.5 的服务端）——两种服务端都要配 `alpn` 为 `["h3"]`（它们缺省不配 ALPN，rurge 没写 `alpn` 时发 `h3`，服务端不选协议时握手失败）；Surge（对照一项用）；一个开了 UDP 的上游 SOCKS5 节点（`udp-relay=true`，`underlying-proxy` 一项用）。策略行照 Surge 的写法：`T5 = tuic-v5, <服务器>, <端口>, uuid=<UUID>, password=<口令>` 与 `T4 = tuic, <服务器>, <端口>, token=<token>`（证书不是公共 CA 签的时另加 `server-cert-fingerprint-sha256=` 或 `skip-cert-verify=true`）。每份配置 `[Rule]` 里 `FINAL,T5`（v4 的各项换成 `FINAL,T4`），`rurge check -c <配置>` 零错误（没有 `W0007`）。

- [ ] v5 的 TCP：`curl -x http://127.0.0.1:<http-listen 端口> https://example.com/ -I` 与 `curl --socks5-hostname 127.0.0.1:<socks5-listen 端口> https://example.com/ -I` 都返回 200，请求记录里策略链是 `T5`、`error` 为空；同时打开几个网站，服务端日志（或抓包）只看到一条 QUIC 连接、一次认证。下载一个几百 MiB 的文件，速度与官方客户端（tuic-client 1.x 或 sing-box 的 `tuic` 出站）在同一量级，记下数字。
- [ ] v5 的 UDP：不需要参数，经 SOCKS5 UDP 发 DNS 查询（如 Proxifier 代理 `nslookup example.com 8.8.8.8`）得到回答；查一个应答超过 1200 字节的记录（如 `dig +bufsize=4096 <TXT 记录很多的域名> TXT @8.8.8.8`）也得到完整的回答（放不进一个 QUIC 数据报的包分片走）；经它进行一次语音通话或联机游戏；用 NAT 类型检测工具（STUN）检测，结果是 Full Cone（节点的出口须是全锥）；抓包看到 UDP 走的是 QUIC 数据报（`native` 模式）。
- [ ] v4（官方 tuic 0.8.5 的服务端）：同上两项换成 `T4`，TCP 与 UDP 都能往返。v4 不分片：经 SOCKS5 UDP 向任一 UDP 目标发一个 1500 字节的包，rurge 不发送它，会话记录写 `tuic: a datagram longer than <n> bytes`，同一条流里之后的小包照常；服务端的大应答同样放不进一个数据报（应答超过约 1.2 KB 的 DNS 查询，如上一项的 `dig`，得不到回答）。
- [ ] 口令错误（v5）：把 `password` 改错一位，重载后 `curl -x http://… https://example.com/ -I`：CONNECT 先回 200（v5 的 Connect 没有应答），随后传输失败；会话记录的 `error` 是 `tuic: authentication failed`（服务端是 sing-box 时也可能是空的连接或 `tuic: the server stopped the stream (code 0x0)`——记下看到的是哪一个与服务端版本）；经 SOCKS5 UDP 的 DNS 查询失败、记录写 `tuic: authentication failed`。**错误文本与日志（含 `--log-level verbose`）里都没有 UUID 与口令**。
- [ ] token 错误（v4）：把 `token` 改错一位，重载后 `curl` 得到 502，会话记录写 `tuic: authentication failed`；UDP 同样失败、写同一个错误；日志里没有 token。
- [ ] 心跳：经 `T5` 开一个 SSH 会话后一分钟不输入（服务端是 tuic-server 1.x 时它的空闲超时只有 10 秒，v5 有转发时每 3 秒一个心跳）、经 `T4` 同样（0.8.5 的空闲超时 15 秒，v4 每 10 秒一个心跳）：会话都不断。
- [ ] 端口跳跃：服务端把一段 UDP 端口转发到它的监听端口（手工加 DNAT / REDIRECT 规则，如 `iptables -t nat -A PREROUTING -p udp --dport 20000:20100 -j REDIRECT --to-ports <监听端口>`），策略写 `port-hopping=20000-20100, port-hopping-interval=10`：经它的下载进行中，抓包看到发往服务端的目的端口约每 10 秒换一个、下载不中断；`port-hopping-interval=2` 时 `rurge check` 报一条 `W0012`（按 5 秒）。
- [ ] 不接受 QUIC 数据报的服务端（如果找得到；tuic 0.8.5、tuic-server 1.x 与 sing-box 都总是接受）：UDP 改走每个包一条单向流（`quic` 模式，抓包看不到 DATAGRAM 帧），DNS 查询照常有回答，同一连接上的 TCP 照常。
- [ ] `underlying-proxy`：策略写 `underlying-proxy=<SOCKS5 节点>`，TCP 与 UDP 都能往返，SOCKS5 节点那边看到的是一条 UDP 关联（QUIC 经它的 UDP 走），没有 TCP 的 CONNECT；同时写了 `port-hopping` 时 `rurge check` 报 `E0018`。
- [ ] 休眠唤醒与换网：经 `T5`（与 `T4`）的连接在用时，让电脑休眠再唤醒，或在 Wi-Fi 与有线 / 手机热点之间切换：约 30 秒内（QUIC 的空闲超时）之后的请求走新建的连接、恢复正常，不必重启 rurge；其间的请求可能超时失败。
- [ ] 与 Surge 对照：同样的两行策略原样放进 Surge 的配置、连同两个服务端，rurge 与 Surge 都连得上（`alpn` 的缺省、凭据的写法相同）；只有一边连得上时记下服务端实现与版本，在兼容性清单登记差异。
- [ ] 脱敏：`GET /v1/policies/detail?policy_name=T5`（与 `T4`）与 `GET /v1/profiles/current` 里 `uuid`、`password` 与 `token` 都是 `***`。

```

`README.md`——把

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；M4c（external）已完成——`external` 策略（TCP；第一次用到时拉起外部程序、经它在本机端口上的 SOCKS5 转发；程序退出后下次用到时再拉起；输出写进数据目录；rurge 退出时连同它启动的进程一起停掉——Unix 用进程组，Windows 用 Job Object；三平台都支持，Surge 只在 Mac 上有）已可用，订阅导入的 `external` 一律跳过；M5a（UDP 地基）已完成——SOCKS5 监听支持 UDP ASSOCIATE，UDP 按规则分流到 DIRECT、REJECT 或 `socks5` / `socks5-tls` / `external`（`udp-relay=true`，含 `underlying-proxy` 链），全锥 NAT，每条 UDP 流一条请求记录，`block-quic` 与 `udp-policy-not-supported-behaviour` 生效；M5b（TLS 族的 UDP）已完成——`trojan`（UDP ASSOCIATE）、`anytls`（UDP over TCP v2）全锥，`vmess`（命令 2，每个目标一条连接）对称型；M5c（WireGuard 的 UDP 与其余）已完成——`wireguard` 的 UDP（全锥）与经 `underlying-proxy` 的隧道、`test-udp` / `proxy-test-udp`、`smart` 组计入 UDP、`dns-follow-interface`；M6（Shadowsocks / Snell / HTTP/2 族）已完成：M6a（Shadowsocks）已完成——`ss` 的 AEAD、`none` 与 SS 2022（含多用户身份头）方法、simple-obfs（`http` / `tls`），TCP 与 UDP（`udp-relay=true`、`udp-port`，全锥），流式旧方法在 M8 之前按 REJECT 处理；M6b（Snell）已完成——`snell` v4 / v5 的 TCP 与 UDP（UDP over TCP，全锥）、`reuse`（连接回池给下一个请求用）、simple-obfs `http`，v1–v3 与 v6 按 REJECT 处理（没写 `version` 即 v1）；M6c（HTTP/2 族）已完成——`h2-connect`（HTTP/2 CONNECT，多条隧道共用连接、`max-streams`；`udp-relay=true` 时经 CONNECT-UDP（RFC 9298）转发 UDP，每个目标一条流、对称型）与 `trust-tunnel`（HTTP/2，TCP；`h3=true` 在 M7 之前仍按 HTTP/2 连）；M7（QUIC 族）进行中：M7a（QUIC 地基与 Hysteria 2）已完成——`hysteria2` 的 TCP 与 UDP（QUIC 数据报，全锥）、Salamander 与 Gecko 混淆、`port-hopping`、`download-bandwidth`（只上报给服务端）与 `ecn`，可经 `underlying-proxy` 连出去；M7b（TUIC）与 M7c（MASQUE、`trust-tunnel` 的 HTTP/3、DoQ / DoH3）在后；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

换成

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；M4c（external）已完成——`external` 策略（TCP；第一次用到时拉起外部程序、经它在本机端口上的 SOCKS5 转发；程序退出后下次用到时再拉起；输出写进数据目录；rurge 退出时连同它启动的进程一起停掉——Unix 用进程组，Windows 用 Job Object；三平台都支持，Surge 只在 Mac 上有）已可用，订阅导入的 `external` 一律跳过；M5a（UDP 地基）已完成——SOCKS5 监听支持 UDP ASSOCIATE，UDP 按规则分流到 DIRECT、REJECT 或 `socks5` / `socks5-tls` / `external`（`udp-relay=true`，含 `underlying-proxy` 链），全锥 NAT，每条 UDP 流一条请求记录，`block-quic` 与 `udp-policy-not-supported-behaviour` 生效；M5b（TLS 族的 UDP）已完成——`trojan`（UDP ASSOCIATE）、`anytls`（UDP over TCP v2）全锥，`vmess`（命令 2，每个目标一条连接）对称型；M5c（WireGuard 的 UDP 与其余）已完成——`wireguard` 的 UDP（全锥）与经 `underlying-proxy` 的隧道、`test-udp` / `proxy-test-udp`、`smart` 组计入 UDP、`dns-follow-interface`；M6（Shadowsocks / Snell / HTTP/2 族）已完成：M6a（Shadowsocks）已完成——`ss` 的 AEAD、`none` 与 SS 2022（含多用户身份头）方法、simple-obfs（`http` / `tls`），TCP 与 UDP（`udp-relay=true`、`udp-port`，全锥），流式旧方法在 M8 之前按 REJECT 处理；M6b（Snell）已完成——`snell` v4 / v5 的 TCP 与 UDP（UDP over TCP，全锥）、`reuse`（连接回池给下一个请求用）、simple-obfs `http`，v1–v3 与 v6 按 REJECT 处理（没写 `version` 即 v1）；M6c（HTTP/2 族）已完成——`h2-connect`（HTTP/2 CONNECT，多条隧道共用连接、`max-streams`；`udp-relay=true` 时经 CONNECT-UDP（RFC 9298）转发 UDP，每个目标一条流、对称型）与 `trust-tunnel`（HTTP/2，TCP；`h3=true` 在 M7 之前仍按 HTTP/2 连）；M7（QUIC 族）进行中：M7a（QUIC 地基与 Hysteria 2）已完成——`hysteria2` 的 TCP 与 UDP（QUIC 数据报，全锥）、Salamander 与 Gecko 混淆、`port-hopping`、`download-bandwidth`（只上报给服务端）与 `ecn`，可经 `underlying-proxy` 连出去；M7b（TUIC）已完成——`tuic-v5`（TUIC v5）与 `tuic`（TUIC v4，tuic 0.8.5 的协议）的 TCP 与 UDP（服务端接受 QUIC 数据报时走数据报，v5 放不下的包分片，否则每个包一条单向流；全锥）、`port-hopping` 与 `ecn`，可经 `underlying-proxy` 连出去；M7c（MASQUE、`trust-tunnel` 的 HTTP/3、DoQ / DoH3）在后；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

`README.md`——把

```markdown
| 出站协议       | DIRECT / REJECT 系列 / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / 外部程序（DIRECT 与 REJECT 系四种已实现，M3a；HTTP（TCP）与 SOCKS5（含 `socks5-tls`，TCP 与 UDP）上游及 `underlying-proxy` 链已实现，阶段 2 / M1、M5a；Trojan（TCP 与 UDP；WebSocket 传输）已实现，阶段 2 / M2a、M5b；VMess（AEAD 握手，TCP 与 UDP；可叠加 TLS / WebSocket）与 AnyTLS（TCP 与 UDP，会话复用）已实现，阶段 2 / M2b、M5b；SSH（TCP，会话复用）已实现，阶段 2 / M4a；WireGuard（TCP 与 UDP，用户态隧道）已实现，阶段 2 / M4b、M5c；外部程序（TCP 与 UDP，三平台）已实现，阶段 2 / M4c、M5a；Shadowsocks（AEAD / 2022、obfs，TCP 与 UDP）已实现，阶段 2 / M6a；Snell v4 / v5（TCP 与 UDP、`reuse`、obfs `http`）已实现，阶段 2 / M6b；HTTP/2 CONNECT（`h2-connect`，TCP 与 CONNECT-UDP）与 Trust Tunnel（HTTP/2，TCP）已实现，阶段 2 / M6c；Hysteria 2（TCP 与 UDP、Salamander / Gecko 混淆、端口跳跃）已实现，阶段 2 / M7a） | 2     |
```

换成

```markdown
| 出站协议       | DIRECT / REJECT 系列 / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / 外部程序（DIRECT 与 REJECT 系四种已实现，M3a；HTTP（TCP）与 SOCKS5（含 `socks5-tls`，TCP 与 UDP）上游及 `underlying-proxy` 链已实现，阶段 2 / M1、M5a；Trojan（TCP 与 UDP；WebSocket 传输）已实现，阶段 2 / M2a、M5b；VMess（AEAD 握手，TCP 与 UDP；可叠加 TLS / WebSocket）与 AnyTLS（TCP 与 UDP，会话复用）已实现，阶段 2 / M2b、M5b；SSH（TCP，会话复用）已实现，阶段 2 / M4a；WireGuard（TCP 与 UDP，用户态隧道）已实现，阶段 2 / M4b、M5c；外部程序（TCP 与 UDP，三平台）已实现，阶段 2 / M4c、M5a；Shadowsocks（AEAD / 2022、obfs，TCP 与 UDP）已实现，阶段 2 / M6a；Snell v4 / v5（TCP 与 UDP、`reuse`、obfs `http`）已实现，阶段 2 / M6b；HTTP/2 CONNECT（`h2-connect`，TCP 与 CONNECT-UDP）与 Trust Tunnel（HTTP/2，TCP）已实现，阶段 2 / M6c；Hysteria 2（TCP 与 UDP、Salamander / Gecko 混淆、端口跳跃）已实现，阶段 2 / M7a；TUIC v5 与 v4（TCP 与 UDP、端口跳跃）已实现，阶段 2 / M7b） | 2     |
```

`README.md`——把

```markdown
3. **阶段 2** 出站协议全集、策略组、策略订阅（进行中：M1 出站地基与 HTTP / SOCKS5 上游已完成；M2a（Trojan，TCP + WebSocket）已完成；M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成；M3a（成员装配与订阅）已完成；M3b（测速与自动组）已完成；M3c（`smart`）已完成；M4a（SSH）已完成；M4b（WireGuard）已完成；M4c（external）已完成；M5a（UDP 地基）已完成；M5b（TLS 族的 UDP）已完成；M5c（WireGuard 的 UDP 与其余）已完成；M6a（Shadowsocks）已完成；M6b（Snell）已完成；M6c（HTTP/2 族）已完成，M6 至此完成；M7a（QUIC 地基与 Hysteria 2）已完成，M7b（TUIC）与 M7c（HTTP/3 族与 DoQ / DoH3）在后）
```

换成

```markdown
3. **阶段 2** 出站协议全集、策略组、策略订阅（进行中：M1 出站地基与 HTTP / SOCKS5 上游已完成；M2a（Trojan，TCP + WebSocket）已完成；M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成；M3a（成员装配与订阅）已完成；M3b（测速与自动组）已完成；M3c（`smart`）已完成；M4a（SSH）已完成；M4b（WireGuard）已完成；M4c（external）已完成；M5a（UDP 地基）已完成；M5b（TLS 族的 UDP）已完成；M5c（WireGuard 的 UDP 与其余）已完成；M6a（Shadowsocks）已完成；M6b（Snell）已完成；M6c（HTTP/2 族）已完成，M6 至此完成；M7a（QUIC 地基与 Hysteria 2）已完成；M7b（TUIC）已完成，M7c（HTTP/3 族与 DoQ / DoH3）在后）
```

`README_en.md`——把

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; M4b (WireGuard) is done — the `wireguard` policy (TCP; a user-space tunnel, no virtual interface; several peers chosen by `allowed-ips`; `client-id` (WARP's reserved bytes); destination names looked up through the tunnel when the section has a `dns-server`, on this machine otherwise; tested by a handshake when there is neither a `dns-server` nor a `test-url`) is usable; M4c (external) is done — the `external` policy (TCP; the program is started on first use and reached as a SOCKS5 proxy on its local port; a program that exited is started again on the next use; its output goes to the data directory; when rurge exits, the program is stopped together with whatever it started — a process group on Unix, a Job Object on Windows; all three platforms, where Surge has it on the Mac only) is usable, and subscriptions never bring `external` policies in; M5a (the UDP foundation) is done — the SOCKS5 listener takes UDP ASSOCIATE, and UDP is routed by rule to DIRECT, REJECT or `socks5` / `socks5-tls` / `external` (`udp-relay=true`, `underlying-proxy` chains included), full-cone NAT, one request record per UDP flow, and `block-quic` and `udp-policy-not-supported-behaviour` take effect; M5b (UDP over the TLS family) is done — `trojan` (UDP ASSOCIATE) and `anytls` (UDP over TCP v2) with full-cone NAT, `vmess` (command 2, one connection per target) symmetric; M5c (UDP over WireGuard and the rest) is done — UDP over `wireguard` (full cone) and tunnels over an `underlying-proxy`, `test-udp` / `proxy-test-udp`, UDP in `smart` groups, and `dns-follow-interface`; M6 (Shadowsocks / Snell / the HTTP/2 family) is done: M6a (Shadowsocks) is done — `ss` with the AEAD, `none` and SS 2022 (multi-user identity headers included) methods, simple-obfs (`http` / `tls`), TCP and UDP (`udp-relay=true`, `udp-port`, full cone), with the legacy stream ciphers behaving as REJECT until M8; M6b (Snell) is done — `snell` v4 / v5 over TCP and UDP (UDP over TCP, full cone), `reuse` (a connection goes back to a pool for the next request) and simple-obfs `http`, with v1–v3 and v6 behaving as REJECT (a line without `version` is v1); M6c (the HTTP/2 family) is done — `h2-connect` (HTTP/2 CONNECT, tunnels sharing connections up to `max-streams`; with `udp-relay=true`, UDP over CONNECT-UDP (RFC 9298), one stream per target, symmetric) and `trust-tunnel` (HTTP/2, TCP; `h3=true` still connects over HTTP/2 until M7); M7 (the QUIC family) is in progress: M7a (the QUIC foundation and Hysteria 2) is done — `hysteria2` over TCP and UDP (QUIC datagrams, full cone), Salamander and Gecko obfuscation, `port-hopping`, `download-bandwidth` (only reported to the server) and `ecn`, optionally through an `underlying-proxy`; M7b (TUIC) and M7c (MASQUE, `trust-tunnel` over HTTP/3, DoQ / DoH3) come next; the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

换成

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; M4b (WireGuard) is done — the `wireguard` policy (TCP; a user-space tunnel, no virtual interface; several peers chosen by `allowed-ips`; `client-id` (WARP's reserved bytes); destination names looked up through the tunnel when the section has a `dns-server`, on this machine otherwise; tested by a handshake when there is neither a `dns-server` nor a `test-url`) is usable; M4c (external) is done — the `external` policy (TCP; the program is started on first use and reached as a SOCKS5 proxy on its local port; a program that exited is started again on the next use; its output goes to the data directory; when rurge exits, the program is stopped together with whatever it started — a process group on Unix, a Job Object on Windows; all three platforms, where Surge has it on the Mac only) is usable, and subscriptions never bring `external` policies in; M5a (the UDP foundation) is done — the SOCKS5 listener takes UDP ASSOCIATE, and UDP is routed by rule to DIRECT, REJECT or `socks5` / `socks5-tls` / `external` (`udp-relay=true`, `underlying-proxy` chains included), full-cone NAT, one request record per UDP flow, and `block-quic` and `udp-policy-not-supported-behaviour` take effect; M5b (UDP over the TLS family) is done — `trojan` (UDP ASSOCIATE) and `anytls` (UDP over TCP v2) with full-cone NAT, `vmess` (command 2, one connection per target) symmetric; M5c (UDP over WireGuard and the rest) is done — UDP over `wireguard` (full cone) and tunnels over an `underlying-proxy`, `test-udp` / `proxy-test-udp`, UDP in `smart` groups, and `dns-follow-interface`; M6 (Shadowsocks / Snell / the HTTP/2 family) is done: M6a (Shadowsocks) is done — `ss` with the AEAD, `none` and SS 2022 (multi-user identity headers included) methods, simple-obfs (`http` / `tls`), TCP and UDP (`udp-relay=true`, `udp-port`, full cone), with the legacy stream ciphers behaving as REJECT until M8; M6b (Snell) is done — `snell` v4 / v5 over TCP and UDP (UDP over TCP, full cone), `reuse` (a connection goes back to a pool for the next request) and simple-obfs `http`, with v1–v3 and v6 behaving as REJECT (a line without `version` is v1); M6c (the HTTP/2 family) is done — `h2-connect` (HTTP/2 CONNECT, tunnels sharing connections up to `max-streams`; with `udp-relay=true`, UDP over CONNECT-UDP (RFC 9298), one stream per target, symmetric) and `trust-tunnel` (HTTP/2, TCP; `h3=true` still connects over HTTP/2 until M7); M7 (the QUIC family) is in progress: M7a (the QUIC foundation and Hysteria 2) is done — `hysteria2` over TCP and UDP (QUIC datagrams, full cone), Salamander and Gecko obfuscation, `port-hopping`, `download-bandwidth` (only reported to the server) and `ecn`, optionally through an `underlying-proxy`; M7b (TUIC) is done — `tuic-v5` (TUIC v5) and `tuic` (TUIC v4, as tuic 0.8.5 speaks it) over TCP and UDP (in QUIC datagrams where the server takes them, v5 splitting a packet that does not fit, else a stream per packet; full cone), `port-hopping` and `ecn`, optionally through an `underlying-proxy`; M7c (MASQUE, `trust-tunnel` over HTTP/3, DoQ / DoH3) comes next; the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

`README_en.md`——把

```markdown
| Outbound            | DIRECT / REJECT family / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / external program (DIRECT and the four REJECT flavours implemented, M3a; HTTP (TCP) and SOCKS5 (including `socks5-tls`, TCP and UDP) upstreams and `underlying-proxy` chains implemented, phase 2 / M1, M5a; Trojan (TCP and UDP; WebSocket transport) implemented, phase 2 / M2a, M5b; VMess (AEAD handshake, TCP and UDP; optionally TLS / WebSocket) and AnyTLS (TCP and UDP, session reuse) implemented, phase 2 / M2b, M5b; SSH (TCP, session reuse) implemented, phase 2 / M4a; WireGuard (TCP and UDP, user-space tunnel) implemented, phase 2 / M4b, M5c; external program (TCP and UDP, all three platforms) implemented, phase 2 / M4c, M5a; Shadowsocks (AEAD / 2022, obfs, TCP and UDP) implemented, phase 2 / M6a; Snell v4 / v5 (TCP and UDP, `reuse`, obfs `http`) implemented, phase 2 / M6b; HTTP/2 CONNECT (`h2-connect`, TCP and CONNECT-UDP) and Trust Tunnel (HTTP/2, TCP) implemented, phase 2 / M6c; Hysteria 2 (TCP and UDP, Salamander / Gecko obfuscation, port hopping) implemented, phase 2 / M7a) | 2     |
```

换成

```markdown
| Outbound            | DIRECT / REJECT family / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / external program (DIRECT and the four REJECT flavours implemented, M3a; HTTP (TCP) and SOCKS5 (including `socks5-tls`, TCP and UDP) upstreams and `underlying-proxy` chains implemented, phase 2 / M1, M5a; Trojan (TCP and UDP; WebSocket transport) implemented, phase 2 / M2a, M5b; VMess (AEAD handshake, TCP and UDP; optionally TLS / WebSocket) and AnyTLS (TCP and UDP, session reuse) implemented, phase 2 / M2b, M5b; SSH (TCP, session reuse) implemented, phase 2 / M4a; WireGuard (TCP and UDP, user-space tunnel) implemented, phase 2 / M4b, M5c; external program (TCP and UDP, all three platforms) implemented, phase 2 / M4c, M5a; Shadowsocks (AEAD / 2022, obfs, TCP and UDP) implemented, phase 2 / M6a; Snell v4 / v5 (TCP and UDP, `reuse`, obfs `http`) implemented, phase 2 / M6b; HTTP/2 CONNECT (`h2-connect`, TCP and CONNECT-UDP) and Trust Tunnel (HTTP/2, TCP) implemented, phase 2 / M6c; Hysteria 2 (TCP and UDP, Salamander / Gecko obfuscation, port hopping) implemented, phase 2 / M7a; TUIC v5 and v4 (TCP and UDP, port hopping) implemented, phase 2 / M7b) | 2     |
```

`README_en.md`——把

```markdown
3. **Phase 2** All outbound protocols, policy groups, subscriptions (in progress: M1, the outbound foundation and HTTP / SOCKS5 upstreams, is done; M2a, Trojan (TCP + WebSocket), is done; M2b, VMess / AnyTLS, is done; M2c, Shadow TLS, is done; M3a, member assembly and subscriptions, is done; M3b, connectivity tests and automatic groups, is done; M3c, `smart` groups, is done; M4a, SSH, is done; M4b, WireGuard, is done; M4c, external, is done; M5a, the UDP foundation, is done; M5b, UDP over the TLS family, is done; M5c, UDP over WireGuard and the rest, is done; M6a, Shadowsocks, is done; M6b, Snell, is done; M6c, the HTTP/2 family, is done, which completes M6; M7a, the QUIC foundation and Hysteria 2, is done, with M7b, TUIC, and M7c, the HTTP/3 family and DoQ / DoH3, next)
```

换成

```markdown
3. **Phase 2** All outbound protocols, policy groups, subscriptions (in progress: M1, the outbound foundation and HTTP / SOCKS5 upstreams, is done; M2a, Trojan (TCP + WebSocket), is done; M2b, VMess / AnyTLS, is done; M2c, Shadow TLS, is done; M3a, member assembly and subscriptions, is done; M3b, connectivity tests and automatic groups, is done; M3c, `smart` groups, is done; M4a, SSH, is done; M4b, WireGuard, is done; M4c, external, is done; M5a, the UDP foundation, is done; M5b, UDP over the TLS family, is done; M5c, UDP over WireGuard and the rest, is done; M6a, Shadowsocks, is done; M6b, Snell, is done; M6c, the HTTP/2 family, is done, which completes M6; M7a, the QUIC foundation and Hysteria 2, is done; M7b, TUIC, is done, with M7c, the HTTP/3 family and DoQ / DoH3, next)
```

`CLAUDE.md`——把

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、发送时本机地址或路由失效或握手无回应时换新载体（网络变化后自愈）、网络变化入口、同一私钥与 peer 只留一条隧道，节与载体设置都相同的策略共用）、`stream`、`dns`（隧道内 DNS 与缓存；每个问题 2 秒内重发、先等隧道第一次握手）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。M4c（external）已完成——`rurge-config::spec::ExternalSpec`（`args` 是 `Secret`；两个 `external` 策略写了同一个 `local-port` 是 `E0018`；`args` 进内联参数的脱敏名单）、订阅导入的 `external` 行一律跳过（`W0023`）；`rurge-platform::process`（Unix 进程组与 `killpg`，Windows Job Object）；`rurge_proto::external`（`ExternalOutbound`：第一次用到时拉起、输出写 `<数据目录>/external/<策略名>.log` 且超过 1 MiB 时在拉起前轮转、子进程环境去掉代理变量并设 `NO_PROXY=*`、程序退出后下次用到时再拉起且两次拉起至少隔 2 秒、每 500 ms 连一次本机端口最多 6 次；`ProcessHook` / `ProcessGroup` 与 `NoProcessGroups`；程序自己退出时连同它启动的进程一起结束）；`rurge-engine` 的工厂分支、`EngineShared.processes` / `externals` 与 `Engine::stop_external_programs`；bin 的 `PlatformProcesses` 与退出流程最后停掉全部外部程序；只用于测试的工作区成员 `tests/external`（`rurge-external-tests`：极小的 SOCKS5 辅助程序 `socks-helper` 与真实拉起它的用例）；能力表翻转 `external`（`W0007` 不再因 `external` 出现）。M4 至此完成。M5（UDP 路径）按三份计划推进（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）：M5a 已完成——`rurge_net::connector::PacketSocket`（按包收发、带地址的 UDP 载体）与 `Connector::open_udp`（`DirectConnector`：每个地址族一个未连接的 socket，忽略 Windows 的 ICMP 不可达报错）；`Outbound::udp()` / `open_udp()` 与 `UdpSupport`；DIRECT 与 `socks5` / `socks5-tls` / `external` 的 UDP（`udp-relay`，`W0029` 退役）；`ChainConnector::open_udp`（链式 UDP 载体）；`rurge-inbound` 的 SOCKS5 UDP ASSOCIATE（`UdpClient`、`UdpAdmission`、`Dialer::admit_udp` / `associate`）；`rurge-engine` 的 UDP 流水线（`udp` 模块：一条流 = 关联 + 目标、按"关联 × 出站"共用载体的全锥、60 秒 / DNS 10 秒回收、1024 流 / 4096 关联的上限）、请求记录与 API 的 `transport`、`block-quic`（`W0029` 退役）与 `udp-policy-not-supported-behaviour`、QUIC Initial 识别、`PROTOCOL` 规则按传输层匹配 `TCP` / `UDP`；`FakeSocks5` 与 `tests/external` 辅助程序的 UDP ASSOCIATE；对 sing-box `socks` 入站的 UDP 互操作用例。M5b（TLS 族的 UDP）已完成——`rurge-proto` 的 `stream_udp`（一条字节流上按包收发：请求头随第一个包发出、写那个包的目标；`trojan` 的 UDP ASSOCIATE 与 `anytls` 的 UDP over TCP v2 两种封装）、`trojan` / `anytls` 的 `open_udp`（全锥）、`vmess` 的命令 2（`vmess::udp`：每个目标一条 VMess 连接、随它的第一个包建立、每个数据报一个分块，对称型）；`FakeTrojan` / `FakeAnyTls` / `FakeVmess` 的 UDP 与 `rurge_proto::testing::udp_echo_server`；经引擎的端到端用例（`tests/udp_tls_family.rs`）；对 sing-box（三种）与 xray（vmess）的 UDP 互操作用例。M5c（WireGuard 的 UDP 与其余）已完成——`rurge-proto-wireguard` 的 `TunnelUdp`（隧道里每个地址族一个 UDP socket、第一次发往该族时绑定、全锥，目标名经隧道 DNS 或本机解析）与 `Stack::udp_bind` / `check`；`rurge_net::packet_datagram`（把 `PacketSocket` 变成一条到固定目标的 `Datagram`）与 `ChainConnector::connect_udp`，`wireguard` 的载体经 `underlying-proxy`（底层策略不载 UDP 时拨号失败，`W0029` 退役）；启动时 peer 连不上的告警每个策略的每个 peer 5 分钟至多一次（M4b 延后事项 #15）；`rurge_policy::udp_probe`（经策略的 UDP 向 `hostname@ipv4` 问一次 A 记录）、`Engine::test_udp` 与 `POST /v1/policies/test` 结果里的 `udp` 键（`test-udp` / `proxy-test-udp`，不保存、不参与组的选择）；`smart` 计入 UDP（载体打不开算失败、第一个回包算首字节、3 秒无回包只在 53 / 443 端口算失败、UDP 不换成员）；`dns-follow-interface`（`rurge_net::connector::Via` / `ResolveVia`、`Resolver::lookup_via`：策略自己的解析经它的 `interface` 问普通 DNS 服务器、答案另存，配了加密 DNS 时不跟随；没有 `interface` 时 `W0028`）；对 sing-box WireGuard 端点的 UDP 互操作。M5 至此完成。M5 合并后的复查（2026-10-02，M5 设计第 23 节）改了：Windows 上写了 `interface` 的策略能开 UDP 载体；发给客户端失败只丢那个包；发往上游时出站不收或系统拒发的包只丢那一个（`lost_alone`），其余发送失败的流记为失败并歇 5 秒，一次发送至多 15 秒（`SEND_TIMEOUT`，超时作废载体）；TCP keepalive（`rurge_net::socket::keep_alive`，监听与 `DirectConnector` 都开）；名字目标的回包按端口归流，`smart` 计入 UDP 的发送失败、3 秒规则只看发往地址的流；重建注册表时沿用内置 DIRECT；`wireguard` 跟随底层策略的变化（`Connector::route` / `rurge_net::connector::Route`、`PolicyRegistry::relay_now`）；开 UDP 流时的解析限时 10 秒；每个关联排队至多 1 MiB（`Queued`）、一次打开失败等着它的流一起失败；双栈监听上 IPv4 客户端的回包发往映射地址。M6（Shadowsocks / Snell / HTTP/2 族）按三份计划推进（M6a Shadowsocks → M6b Snell → M6c HTTP/2 族）：M6a（Shadowsocks）已完成——`rurge-config::spec` 的 `SsSpec`（`SsMethod`：AEAD 五种、`none`、SS 2022 两种；2022 的 `password` 按冒号拆成逐层的 Base64 密钥、最后一段是用户密钥，不合法是 `E0018` 且不引用取值；`udp-relay`、`udp-port`）与 `ObfsOpts`（与 Snell 共用；`obfs-host` 缺省服务器主机名、`obfs-uri` 缺省 `/`）、`NotImplemented`（取代 `legacy_vmess` 标记：vmess 旧握手与 `ss` 流式旧方法都是 `W0007` + REJECT，流式方法每种每次加载一条，会话日志 `policy protocol not implemented: ss (<method>)`）、`obfs-host` 进内联参数的脱敏名单；`rurge_proto::transport::obfs`（simple-obfs 的 `http` / `tls`，自写模板，`Stack` 的一层：connect → shadow-tls → obfs → tls → ws）；`rurge_proto::shadowsocks`（`ShadowsocksOutbound`：AEAD 分块流与 `none`、SS 2022（BLAKE3 子密钥、请求头块与填充、应答的类型 / 时间戳 / 回显 salt 校验、SIP023 多用户身份头）、UDP（`udp-relay=true` 发往 `udp-port`，全锥；2022 的分离头、按服务端 session 的防重放窗口））；新依赖只有 `blake3`；`rurge_proto::testing` 的 `FakeShadowsocks`（AEAD、2022 含身份头、UDP、两种 obfs）与 `accept_obfs`；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_shadowsocks.rs`）；能力表翻转 `ss`（流式旧方法除外）；测试里"未实现的协议"的例子从 `ss` 换成 `hysteria2`；`tests/interop` 对 shadowsocks-rust `ssserver`（固定版本 v1.25.0，`RURGE_TEST_SSSERVER`）与 sing-box `shadowsocks` 入站的互操作用例。M6b（Snell）已完成——`rurge-config::spec` 的 `SnellSpec`（`SnellVersion` 只有 V4 / V5；`version` 1–6、缺省 1，1–3 与 6 是 `NotImplemented::SnellVersion`：`W0007` + REJECT，每个版本每次加载一条，会话日志 `policy protocol not implemented: snell v<n>`；`psk` 是 `Secret`；v4 / v5 的 `obfs` 只有 `http`；`mode` 只对 v6 校验）；`rurge_proto::snell`（`SnellOutbound`：`kdf`（Argon2id 取前 16 字节，在阻塞线程上算；工作区依赖 `argon2` 0.6，已在依赖树里）、`record`（`SnellStream`：每方向一个 salt、7 字节加密头、首帧 256–511 字节填充与负载密文交错、每帧至多 0x3FFF、空帧结束一个方向、复用时计数器延续）、`tunnel`（请求头随首段负载写出，`reuse=false` 发 Connect `0x01`、`reuse=true` 发 ConnectV2 `0x05`；应答 `00` / `02`；干净结束的连接在 `Drop` 时回池，没结束的在后台补完再回池；池里取出的失效连接换新连接重发一次）、`pool`（每个出站至多 8 条空闲连接、空闲 60 秒回收）、`udp`（命令 `0x06`：每个载体一条自己的连接、从不进池，每个数据报一帧，全锥；`udp-port` 只解析（`W0029`），UDP 一律走主端口））；`LazyHead` 改为泛型（`LazyHead<S = BoxedStream>`，加 `get_mut` / `into_inner`）；`rurge_proto::testing` 的 `FakeSnell`（`SnellScript`：TCP、ConnectV2 复用、UDP、obfs http、拒绝、每条连接的请求数上限）；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_snell.rs`）；能力表翻转 `snell`（v4 / v5）；`tests/interop` 对 sing-box `snell` 入站（`version: 5`，同时接受 v4 客户端；sing-box 固定版本从 1.14.1 升到 1.14.2）与 Surge 官方 snell-server v5.0.1（只有 Linux 版，`RURGE_TEST_SNELL_SERVER`，只在 Linux CI 上装）的互操作用例。M6c（HTTP/2 族）已完成——`rurge-config::spec::h2`（`H2ConnectSpec`：凭据写在端口之后或命名、命名的优先，`headers`、`max-streams` 缺省 3 且至少 1、`udp-relay`；`TrustTunnelSpec`：`username` / `password` 只读命名写法、必填（`E0018`）；两者 `alpn` 固定 `h2`（写了别的 `W0028`），`headers` 里 HTTP/2 禁止的连接专用字段与 `host` 各 `W0028` 并去掉；`trust-tunnel` 的 `h3=true` 是 `W0029`、M7 之前按 HTTP/2 连，`udp-relay` 是 `W0028`）；`rurge_proto::h2pool`（`H2Pool`：每个出站一个池，每条 TLS + HTTP/2 连接至多 `max-streams` 条流且不超过服务端的上限、流数自己计，新连接收到服务端 SETTINGS 之前只承载一条流（得知 SETTINGS 的 PING 5 秒内没有回应即不再分配新流），在途的每次拨号算作 `max-streams` 个位置、放不下的请求再拨（至多 8 条在途），连接窗口为 `max-streams` × 1 MiB（至少 4 MiB），没有流的连接空闲 60 秒关闭，收到 GOAWAY 的连接不再分配新流，请求没得到应答时用 PING 检查连接是否还活着（5 秒），死了就换新连接；`H2Stream`：按流控窗口分段写、读即释放容量、END_STREAM 半关闭；`StackDial`：ALPN 不是 `h2` 时 `<协议>: the server does not speak HTTP/2`）；`rurge_proto::h2connect`（`H2ConnectOutbound`：CONNECT + Basic，`headers` 替换同名的自生成字段、`<random-string>` 按请求生成；407 是 `h2-connect: proxy authentication required`；`capsule`（RFC 9297 的 capsule 与 varint）与 `udp`（`udp-relay=true` 时的 CONNECT-UDP：每个目标一条 extended CONNECT 流、对称型，masque 默认模板的路径，DATAGRAM capsule，其它 capsule 与 context id 丢弃，超过 65527 字节的数据报不发，服务端没通告 extended CONNECT 时发往每个目标的第一个包失败，空闲 60 秒关流））；`rurge_proto::trust_tunnel`（`TrustTunnelOutbound`：同一套 CONNECT，`user-agent: rurge`，407 是 `trust-tunnel: authentication failed`，不载 UDP）；`TlsClient::wrap_negotiated` / `Stack::open_negotiated`（取协商出的 ALPN）；`rurge_proto::testing` 的 `FakeH2Proxy`（`H2ProxyScript`：Basic 与 407、拒绝、服务端流数上限、GOAWAY、extended CONNECT 与 CONNECT-UDP、多余的 capsule、`user-agent` 必填、不参与 ALPN、双向 TLS）；`rurge-engine` 的两个工厂分支与经引擎的端到端用例（`tests/outbounds_http2.rs`）；能力表翻转 `h2-connect` 与 `trust-tunnel`；`tests/interop` 对 TrustTunnel endpoint v1.1.0（只有 Linux / macOS 版，`RURGE_TEST_TRUSTTUNNEL`；sing-box 1.14.2 的 `http` 入站只说 HTTP/1，所以 endpoint 同时充当 `h2-connect` 普通 CONNECT 的参照）的互操作用例；CONNECT-UDP 没有可用的参照服务端。M6 至此完成；合并后的复查（M6 设计第 18 节）又修了会话池的串行拨号与 SETTINGS 到达前静默断掉的连接、Snell 的 `udp-port`（只解析）与重拨时只唤醒最后一个轮询者、CONNECT-UDP 的 `:authority`（取 TLS 的名字）、`headers` 的值进 `Debug`、ss 首字节前被重置时的提示、`obfs-uri` 脱敏，并把 CI 钉在 1.89（`RUSTUP_TOOLCHAIN=1.89.0`；此前 CI 每次装最新 stable，三个平台都在 clippy 一步失败，测试与互操作从未执行过）。M7（QUIC 族）按三份计划推进（M7a QUIC 地基 + Hysteria 2 → M7b TUIC → M7c HTTP/3 族与 DoQ / DoH3）：M7a 已完成——新依赖只有 `quinn` 0.11.12（`default-features = false`，`runtime-tokio` + `rustls-ring`，随之引入 quinn-proto / quinn-udp；不用 crates.io 的 `h3`，M7-D5），`blake2` 0.10 沿用依赖树里的；`rurge-config::spec` 的 `QuicOpts`（只有 `port_hopping: Option<PortHopping>`；`alpn` 留在 `TlsOpts`、QUIC 类缺省 `h3`，`ecn` 留在 `CommonOpts`）、`Hysteria2Spec`（`password` 必填、`Secret`；`download-bandwidth` 整数 Mbps；`Hysteria2Obfs` 的 Salamander / Gecko 互斥，都是 `Secret`）与 `ProtoSpec::quic()`：`port-hopping` 写法错误是 `E0018`，`port-hopping-interval` 小于 5 是 `W0012` 并按 5、没有 `port-hopping` 时 `W0028`，`tfo` / `udp-relay` 在 `hysteria2` 上是 `W0028`，`ecn` 在它上面不再 `W0029`，`port-hopping` × `underlying-proxy` 与 Shadow TLS × `hysteria2` 两条 `E0018` 真正报出，组级 `underlying-proxy` 的组里带 `port-hopping` 的成员（配置本身给出的）加载时报 `W0023`，`salamander-password` / `gecko-password` 进内联参数的脱敏名单；`rurge-net` 的 `Connector::as_direct()`（只有 `DirectConnector` 覆盖：直连时 QUIC 自己建 socket，其余经 `connect_udp` 的载体）与 `quic` 模块（`Path::open`：直连 socket 复用 `DirectConnector::bind_udp`，`tos ≠ 0` 时发送不经 quinn-udp，`ecn=off` 去掉两侧的标记，链式载体（占位地址 `192.0.2.1:443`、不做 PMTU 探测、放不下就丢），端口跳跃层（每次换到另一个端口、上一个 socket 留到下一跳）；socket 层的发送错误一律丢包——非 `WouldBlock` 的错误会让 quinn 的连接挂死；`dial`：每条连接一个 quinn endpoint、`Transport::STANDARD`（空闲 30 秒、保活 10 秒、8 / 20 MiB 窗口、Cubic）、`connection_error` 的 `quic: …` / `tls: …` 文本；`testing` 的 `TestCert` / `EchoServer` / `MultiPort`）与 `quic::h3`（自写的最小 HTTP/3 客户端：只开控制流、SETTINGS 与等待、QPACK 静态表 + 字面量 + Huffman 解码（照 RFC 9204 附录 A 与 RFC 7541 附录 B 录入并独立核对）、HEADERS / DATA、可取消安全的写、GOAWAY；`testing` 的 `H3Server`）；`rurge-proto` 的 `QuicTls` / `build::quic_tls`（与 `TlsClient` 共用构造，只用 TLS 1.3）；新 crate `rurge-proto-quic`：`session`（`Dialer`（`with_layer`；`port-hopping` 遇上非直连的连接器在构建时失败，组中继派生的 `M (via R)` 因此被略去）、`Protocol`、`Session`：在请求自己的 future 里单飞拨号与握手、只关还开着的连接、按租约计的 60 秒空闲关闭、失效连接重试一次、跟随底层策略的去处（`Connector::route()`）；`testing::ServerPort`：能重启并回 stateless reset 的服务端端口）与 `hysteria2`（`Hysteria2Outbound`：HTTP/3 认证（失败以 0x101 关闭，`Hysteria-UDP` 按 Go 的 `ParseBool` 读）、TCP（`0x401`，任何非 0 状态都是拒绝）、UDP（会话号按出站递增、分片与重组、`Hysteria-UDP: false` 时打开载体即失败、换连接沿用会话号）、`obfs`（Salamander：BLAKE2b-256 用 RFC 7693 的自测核对；Gecko：长头包拆 2–8 片、512–1200 字节、至多 8 组在途、8 秒过期；混淆口令不对只表现为超时）；`testing::FakeHysteria2`（线上编解码与混淆都独立实现））；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_hysteria2.rs`；`common::harness_edited` 缩短跳跃间隔）与不作门禁的吞吐基准（Windows 回环、release：每方向明文约 70–80 MiB/s、Salamander 约 56–66 MiB/s）；bin 不输出 `quinn` / `quinn_proto` 的日志，`quinn_udp` 留 WARN；能力表翻转 `hysteria2`；测试里"未实现的协议"的例子从 `hysteria2` 换成 `tailscale`；`tests/interop` 对 sing-box 1.14.2 `hysteria2` 入站（无混淆、Salamander、Gecko）的互操作用例（CI 已在三平台装 sing-box，不用改）。
```

换成

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、发送时本机地址或路由失效或握手无回应时换新载体（网络变化后自愈）、网络变化入口、同一私钥与 peer 只留一条隧道，节与载体设置都相同的策略共用）、`stream`、`dns`（隧道内 DNS 与缓存；每个问题 2 秒内重发、先等隧道第一次握手）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。M4c（external）已完成——`rurge-config::spec::ExternalSpec`（`args` 是 `Secret`；两个 `external` 策略写了同一个 `local-port` 是 `E0018`；`args` 进内联参数的脱敏名单）、订阅导入的 `external` 行一律跳过（`W0023`）；`rurge-platform::process`（Unix 进程组与 `killpg`，Windows Job Object）；`rurge_proto::external`（`ExternalOutbound`：第一次用到时拉起、输出写 `<数据目录>/external/<策略名>.log` 且超过 1 MiB 时在拉起前轮转、子进程环境去掉代理变量并设 `NO_PROXY=*`、程序退出后下次用到时再拉起且两次拉起至少隔 2 秒、每 500 ms 连一次本机端口最多 6 次；`ProcessHook` / `ProcessGroup` 与 `NoProcessGroups`；程序自己退出时连同它启动的进程一起结束）；`rurge-engine` 的工厂分支、`EngineShared.processes` / `externals` 与 `Engine::stop_external_programs`；bin 的 `PlatformProcesses` 与退出流程最后停掉全部外部程序；只用于测试的工作区成员 `tests/external`（`rurge-external-tests`：极小的 SOCKS5 辅助程序 `socks-helper` 与真实拉起它的用例）；能力表翻转 `external`（`W0007` 不再因 `external` 出现）。M4 至此完成。M5（UDP 路径）按三份计划推进（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）：M5a 已完成——`rurge_net::connector::PacketSocket`（按包收发、带地址的 UDP 载体）与 `Connector::open_udp`（`DirectConnector`：每个地址族一个未连接的 socket，忽略 Windows 的 ICMP 不可达报错）；`Outbound::udp()` / `open_udp()` 与 `UdpSupport`；DIRECT 与 `socks5` / `socks5-tls` / `external` 的 UDP（`udp-relay`，`W0029` 退役）；`ChainConnector::open_udp`（链式 UDP 载体）；`rurge-inbound` 的 SOCKS5 UDP ASSOCIATE（`UdpClient`、`UdpAdmission`、`Dialer::admit_udp` / `associate`）；`rurge-engine` 的 UDP 流水线（`udp` 模块：一条流 = 关联 + 目标、按"关联 × 出站"共用载体的全锥、60 秒 / DNS 10 秒回收、1024 流 / 4096 关联的上限）、请求记录与 API 的 `transport`、`block-quic`（`W0029` 退役）与 `udp-policy-not-supported-behaviour`、QUIC Initial 识别、`PROTOCOL` 规则按传输层匹配 `TCP` / `UDP`；`FakeSocks5` 与 `tests/external` 辅助程序的 UDP ASSOCIATE；对 sing-box `socks` 入站的 UDP 互操作用例。M5b（TLS 族的 UDP）已完成——`rurge-proto` 的 `stream_udp`（一条字节流上按包收发：请求头随第一个包发出、写那个包的目标；`trojan` 的 UDP ASSOCIATE 与 `anytls` 的 UDP over TCP v2 两种封装）、`trojan` / `anytls` 的 `open_udp`（全锥）、`vmess` 的命令 2（`vmess::udp`：每个目标一条 VMess 连接、随它的第一个包建立、每个数据报一个分块，对称型）；`FakeTrojan` / `FakeAnyTls` / `FakeVmess` 的 UDP 与 `rurge_proto::testing::udp_echo_server`；经引擎的端到端用例（`tests/udp_tls_family.rs`）；对 sing-box（三种）与 xray（vmess）的 UDP 互操作用例。M5c（WireGuard 的 UDP 与其余）已完成——`rurge-proto-wireguard` 的 `TunnelUdp`（隧道里每个地址族一个 UDP socket、第一次发往该族时绑定、全锥，目标名经隧道 DNS 或本机解析）与 `Stack::udp_bind` / `check`；`rurge_net::packet_datagram`（把 `PacketSocket` 变成一条到固定目标的 `Datagram`）与 `ChainConnector::connect_udp`，`wireguard` 的载体经 `underlying-proxy`（底层策略不载 UDP 时拨号失败，`W0029` 退役）；启动时 peer 连不上的告警每个策略的每个 peer 5 分钟至多一次（M4b 延后事项 #15）；`rurge_policy::udp_probe`（经策略的 UDP 向 `hostname@ipv4` 问一次 A 记录）、`Engine::test_udp` 与 `POST /v1/policies/test` 结果里的 `udp` 键（`test-udp` / `proxy-test-udp`，不保存、不参与组的选择）；`smart` 计入 UDP（载体打不开算失败、第一个回包算首字节、3 秒无回包只在 53 / 443 端口算失败、UDP 不换成员）；`dns-follow-interface`（`rurge_net::connector::Via` / `ResolveVia`、`Resolver::lookup_via`：策略自己的解析经它的 `interface` 问普通 DNS 服务器、答案另存，配了加密 DNS 时不跟随；没有 `interface` 时 `W0028`）；对 sing-box WireGuard 端点的 UDP 互操作。M5 至此完成。M5 合并后的复查（2026-10-02，M5 设计第 23 节）改了：Windows 上写了 `interface` 的策略能开 UDP 载体；发给客户端失败只丢那个包；发往上游时出站不收或系统拒发的包只丢那一个（`lost_alone`），其余发送失败的流记为失败并歇 5 秒，一次发送至多 15 秒（`SEND_TIMEOUT`，超时作废载体）；TCP keepalive（`rurge_net::socket::keep_alive`，监听与 `DirectConnector` 都开）；名字目标的回包按端口归流，`smart` 计入 UDP 的发送失败、3 秒规则只看发往地址的流；重建注册表时沿用内置 DIRECT；`wireguard` 跟随底层策略的变化（`Connector::route` / `rurge_net::connector::Route`、`PolicyRegistry::relay_now`）；开 UDP 流时的解析限时 10 秒；每个关联排队至多 1 MiB（`Queued`）、一次打开失败等着它的流一起失败；双栈监听上 IPv4 客户端的回包发往映射地址。M6（Shadowsocks / Snell / HTTP/2 族）按三份计划推进（M6a Shadowsocks → M6b Snell → M6c HTTP/2 族）：M6a（Shadowsocks）已完成——`rurge-config::spec` 的 `SsSpec`（`SsMethod`：AEAD 五种、`none`、SS 2022 两种；2022 的 `password` 按冒号拆成逐层的 Base64 密钥、最后一段是用户密钥，不合法是 `E0018` 且不引用取值；`udp-relay`、`udp-port`）与 `ObfsOpts`（与 Snell 共用；`obfs-host` 缺省服务器主机名、`obfs-uri` 缺省 `/`）、`NotImplemented`（取代 `legacy_vmess` 标记：vmess 旧握手与 `ss` 流式旧方法都是 `W0007` + REJECT，流式方法每种每次加载一条，会话日志 `policy protocol not implemented: ss (<method>)`）、`obfs-host` 进内联参数的脱敏名单；`rurge_proto::transport::obfs`（simple-obfs 的 `http` / `tls`，自写模板，`Stack` 的一层：connect → shadow-tls → obfs → tls → ws）；`rurge_proto::shadowsocks`（`ShadowsocksOutbound`：AEAD 分块流与 `none`、SS 2022（BLAKE3 子密钥、请求头块与填充、应答的类型 / 时间戳 / 回显 salt 校验、SIP023 多用户身份头）、UDP（`udp-relay=true` 发往 `udp-port`，全锥；2022 的分离头、按服务端 session 的防重放窗口））；新依赖只有 `blake3`；`rurge_proto::testing` 的 `FakeShadowsocks`（AEAD、2022 含身份头、UDP、两种 obfs）与 `accept_obfs`；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_shadowsocks.rs`）；能力表翻转 `ss`（流式旧方法除外）；测试里"未实现的协议"的例子从 `ss` 换成 `hysteria2`；`tests/interop` 对 shadowsocks-rust `ssserver`（固定版本 v1.25.0，`RURGE_TEST_SSSERVER`）与 sing-box `shadowsocks` 入站的互操作用例。M6b（Snell）已完成——`rurge-config::spec` 的 `SnellSpec`（`SnellVersion` 只有 V4 / V5；`version` 1–6、缺省 1，1–3 与 6 是 `NotImplemented::SnellVersion`：`W0007` + REJECT，每个版本每次加载一条，会话日志 `policy protocol not implemented: snell v<n>`；`psk` 是 `Secret`；v4 / v5 的 `obfs` 只有 `http`；`mode` 只对 v6 校验）；`rurge_proto::snell`（`SnellOutbound`：`kdf`（Argon2id 取前 16 字节，在阻塞线程上算；工作区依赖 `argon2` 0.6，已在依赖树里）、`record`（`SnellStream`：每方向一个 salt、7 字节加密头、首帧 256–511 字节填充与负载密文交错、每帧至多 0x3FFF、空帧结束一个方向、复用时计数器延续）、`tunnel`（请求头随首段负载写出，`reuse=false` 发 Connect `0x01`、`reuse=true` 发 ConnectV2 `0x05`；应答 `00` / `02`；干净结束的连接在 `Drop` 时回池，没结束的在后台补完再回池；池里取出的失效连接换新连接重发一次）、`pool`（每个出站至多 8 条空闲连接、空闲 60 秒回收）、`udp`（命令 `0x06`：每个载体一条自己的连接、从不进池，每个数据报一帧，全锥；`udp-port` 只解析（`W0029`），UDP 一律走主端口））；`LazyHead` 改为泛型（`LazyHead<S = BoxedStream>`，加 `get_mut` / `into_inner`）；`rurge_proto::testing` 的 `FakeSnell`（`SnellScript`：TCP、ConnectV2 复用、UDP、obfs http、拒绝、每条连接的请求数上限）；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_snell.rs`）；能力表翻转 `snell`（v4 / v5）；`tests/interop` 对 sing-box `snell` 入站（`version: 5`，同时接受 v4 客户端；sing-box 固定版本从 1.14.1 升到 1.14.2）与 Surge 官方 snell-server v5.0.1（只有 Linux 版，`RURGE_TEST_SNELL_SERVER`，只在 Linux CI 上装）的互操作用例。M6c（HTTP/2 族）已完成——`rurge-config::spec::h2`（`H2ConnectSpec`：凭据写在端口之后或命名、命名的优先，`headers`、`max-streams` 缺省 3 且至少 1、`udp-relay`；`TrustTunnelSpec`：`username` / `password` 只读命名写法、必填（`E0018`）；两者 `alpn` 固定 `h2`（写了别的 `W0028`），`headers` 里 HTTP/2 禁止的连接专用字段与 `host` 各 `W0028` 并去掉；`trust-tunnel` 的 `h3=true` 是 `W0029`、M7 之前按 HTTP/2 连，`udp-relay` 是 `W0028`）；`rurge_proto::h2pool`（`H2Pool`：每个出站一个池，每条 TLS + HTTP/2 连接至多 `max-streams` 条流且不超过服务端的上限、流数自己计，新连接收到服务端 SETTINGS 之前只承载一条流（得知 SETTINGS 的 PING 5 秒内没有回应即不再分配新流），在途的每次拨号算作 `max-streams` 个位置、放不下的请求再拨（至多 8 条在途），连接窗口为 `max-streams` × 1 MiB（至少 4 MiB），没有流的连接空闲 60 秒关闭，收到 GOAWAY 的连接不再分配新流，请求没得到应答时用 PING 检查连接是否还活着（5 秒），死了就换新连接；`H2Stream`：按流控窗口分段写、读即释放容量、END_STREAM 半关闭；`StackDial`：ALPN 不是 `h2` 时 `<协议>: the server does not speak HTTP/2`）；`rurge_proto::h2connect`（`H2ConnectOutbound`：CONNECT + Basic，`headers` 替换同名的自生成字段、`<random-string>` 按请求生成；407 是 `h2-connect: proxy authentication required`；`capsule`（RFC 9297 的 capsule 与 varint）与 `udp`（`udp-relay=true` 时的 CONNECT-UDP：每个目标一条 extended CONNECT 流、对称型，masque 默认模板的路径，DATAGRAM capsule，其它 capsule 与 context id 丢弃，超过 65527 字节的数据报不发，服务端没通告 extended CONNECT 时发往每个目标的第一个包失败，空闲 60 秒关流））；`rurge_proto::trust_tunnel`（`TrustTunnelOutbound`：同一套 CONNECT，`user-agent: rurge`，407 是 `trust-tunnel: authentication failed`，不载 UDP）；`TlsClient::wrap_negotiated` / `Stack::open_negotiated`（取协商出的 ALPN）；`rurge_proto::testing` 的 `FakeH2Proxy`（`H2ProxyScript`：Basic 与 407、拒绝、服务端流数上限、GOAWAY、extended CONNECT 与 CONNECT-UDP、多余的 capsule、`user-agent` 必填、不参与 ALPN、双向 TLS）；`rurge-engine` 的两个工厂分支与经引擎的端到端用例（`tests/outbounds_http2.rs`）；能力表翻转 `h2-connect` 与 `trust-tunnel`；`tests/interop` 对 TrustTunnel endpoint v1.1.0（只有 Linux / macOS 版，`RURGE_TEST_TRUSTTUNNEL`；sing-box 1.14.2 的 `http` 入站只说 HTTP/1，所以 endpoint 同时充当 `h2-connect` 普通 CONNECT 的参照）的互操作用例；CONNECT-UDP 没有可用的参照服务端。M6 至此完成；合并后的复查（M6 设计第 18 节）又修了会话池的串行拨号与 SETTINGS 到达前静默断掉的连接、Snell 的 `udp-port`（只解析）与重拨时只唤醒最后一个轮询者、CONNECT-UDP 的 `:authority`（取 TLS 的名字）、`headers` 的值进 `Debug`、ss 首字节前被重置时的提示、`obfs-uri` 脱敏，并把 CI 钉在 1.89（`RUSTUP_TOOLCHAIN=1.89.0`；此前 CI 每次装最新 stable，三个平台都在 clippy 一步失败，测试与互操作从未执行过）。M7（QUIC 族）按三份计划推进（M7a QUIC 地基 + Hysteria 2 → M7b TUIC → M7c HTTP/3 族与 DoQ / DoH3）：M7a 已完成——新依赖只有 `quinn` 0.11.12（`default-features = false`，`runtime-tokio` + `rustls-ring`，随之引入 quinn-proto / quinn-udp；不用 crates.io 的 `h3`，M7-D5），`blake2` 0.10 沿用依赖树里的；`rurge-config::spec` 的 `QuicOpts`（只有 `port_hopping: Option<PortHopping>`；`alpn` 留在 `TlsOpts`、QUIC 类缺省 `h3`，`ecn` 留在 `CommonOpts`）、`Hysteria2Spec`（`password` 必填、`Secret`；`download-bandwidth` 整数 Mbps；`Hysteria2Obfs` 的 Salamander / Gecko 互斥，都是 `Secret`）与 `ProtoSpec::quic()`：`port-hopping` 写法错误是 `E0018`，`port-hopping-interval` 小于 5 是 `W0012` 并按 5、没有 `port-hopping` 时 `W0028`，`tfo` / `udp-relay` 在 `hysteria2` 上是 `W0028`，`ecn` 在它上面不再 `W0029`，`port-hopping` × `underlying-proxy` 与 Shadow TLS × `hysteria2` 两条 `E0018` 真正报出，组级 `underlying-proxy` 的组里带 `port-hopping` 的成员（配置本身给出的）加载时报 `W0023`，`salamander-password` / `gecko-password` 进内联参数的脱敏名单；`rurge-net` 的 `Connector::as_direct()`（只有 `DirectConnector` 覆盖：直连时 QUIC 自己建 socket，其余经 `connect_udp` 的载体）与 `quic` 模块（`Path::open`：直连 socket 复用 `DirectConnector::bind_udp`，`tos ≠ 0` 时发送不经 quinn-udp，`ecn=off` 去掉两侧的标记，链式载体（占位地址 `192.0.2.1:443`、不做 PMTU 探测、放不下就丢），端口跳跃层（每次换到另一个端口、上一个 socket 留到下一跳）；socket 层的发送错误一律丢包——非 `WouldBlock` 的错误会让 quinn 的连接挂死；`dial`：每条连接一个 quinn endpoint、`Transport::STANDARD`（空闲 30 秒、保活 10 秒、8 / 20 MiB 窗口、Cubic）、`connection_error` 的 `quic: …` / `tls: …` 文本；`testing` 的 `TestCert` / `EchoServer` / `MultiPort`）与 `quic::h3`（自写的最小 HTTP/3 客户端：只开控制流、SETTINGS 与等待、QPACK 静态表 + 字面量 + Huffman 解码（照 RFC 9204 附录 A 与 RFC 7541 附录 B 录入并独立核对）、HEADERS / DATA、可取消安全的写、GOAWAY；`testing` 的 `H3Server`）；`rurge-proto` 的 `QuicTls` / `build::quic_tls`（与 `TlsClient` 共用构造，只用 TLS 1.3）；新 crate `rurge-proto-quic`：`session`（`Dialer`（`with_layer`；`port-hopping` 遇上非直连的连接器在构建时失败，组中继派生的 `M (via R)` 因此被略去）、`Protocol`、`Session`：在请求自己的 future 里单飞拨号与握手、只关还开着的连接、按租约计的 60 秒空闲关闭、失效连接重试一次、跟随底层策略的去处（`Connector::route()`）；`testing::ServerPort`：能重启并回 stateless reset 的服务端端口）与 `hysteria2`（`Hysteria2Outbound`：HTTP/3 认证（失败以 0x101 关闭，`Hysteria-UDP` 按 Go 的 `ParseBool` 读）、TCP（`0x401`，任何非 0 状态都是拒绝）、UDP（会话号按出站递增、分片与重组、`Hysteria-UDP: false` 时打开载体即失败、换连接沿用会话号）、`obfs`（Salamander：BLAKE2b-256 用 RFC 7693 的自测核对；Gecko：长头包拆 2–8 片、512–1200 字节、至多 8 组在途、8 秒过期；混淆口令不对只表现为超时）；`testing::FakeHysteria2`（线上编解码与混淆都独立实现））；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_hysteria2.rs`；`common::harness_edited` 缩短跳跃间隔）与不作门禁的吞吐基准（Windows 回环、release：每方向明文约 70–80 MiB/s、Salamander 约 56–66 MiB/s）；bin 不输出 `quinn` / `quinn_proto` 的日志，`quinn_udp` 留 WARN；能力表翻转 `hysteria2`；测试里"未实现的协议"的例子从 `hysteria2` 换成 `tailscale`；`tests/interop` 对 sing-box 1.14.2 `hysteria2` 入站（无混淆、Salamander、Gecko）的互操作用例（CI 已在三平台装 sing-box，不用改）。M7b（TUIC）已完成——`rurge-config::spec::TuicSpec`（`tuic` 与 `tuic-v5` 共用一份：`TuicVersion::V4 { token }` / `V5 { uuid, password }`，都是 `Secret`；`uuid` 沿用 `vmess` 的 `parse_uuid`；缺凭据或 UUID 不对是 `E0018` 且不引用取值，另一个版本的凭据与 `udp-relay` / `tfo` 是 `W0028`，`ecn` 生效；`port-hopping` × `underlying-proxy` 与 Shadow TLS × `tuic` / `tuic-v5` 两条 `E0018` 真正报出；格式不对的这两种行从只报 `W0007` 变成加载错误；`required` 移到 `spec/mod.rs` 与 `trust-tunnel` 共用）；`rurge-proto-quic` 的 `tuic`（`TuicOutbound`：一份实现，按版本分支；认证在连接建好后写进一条单向流、不等回应——v5 的 token 是 TLS 导出的 32 字节（label 为 UUID 的原始字节、context 为口令），v4 是 BLAKE3(token)（只加 `blake3` 的依赖边）；TCP 开流即写 Connect，v5 立即交出隧道、v4 等 `04 ff REP`；v5 没有错误码，"码 0、空原因、服务端还没转发过任何东西"的关闭读作 `tuic: authentication failed`，v4 的 0xfffffff0 ～ 0xfffffff3 各有文本；UDP：服务端接受 QUIC 数据报时 `native`，否则每个包一条单向流、经每条连接一个的写任务（至多 64 个排队、满了丢包、从不截断一个包），v5 分片与重组（每个关联至多 8 个包在重组）、v4 不分片（放不下的包 `InvalidInput`），关联号按连接分配（v5 从 0 递增、v4 随机 32 位，换连接时取新号），载体走时发 `Dissociate`，认证被拒时一次发送失败，IPv4 映射的来源按 IPv4 读；心跳只在有转发时发（v5 每 3 秒一个数据报、服务端不收数据报时不发，v4 每 10 秒一条单向流）；`testing::FakeTuic`（v4 / v5、两种 UDP 模式、认证失败的关闭、流的扣留与放行，线上编解码独立实现）；测试用的日志捕获移到共用的 `test_log`）；`rurge-engine` 的工厂分支（两种关键字一个分支）与经引擎的端到端用例（`tests/outbounds_tuic.rs`；`tests/common` 收了四个与协议无关的辅助函数）；能力表翻转 `tuic` / `tuic-v5`；`tests/interop` 对 sing-box 1.14.2 `tuic` 入站（v5，只能覆盖 `native` 模式）与官方 tuic 0.8.5 服务端（v4；GPL，只作为外部进程运行；`RURGE_TEST_TUIC_SERVER`，只在 Linux CI 上装）的互操作用例——这个发布没有 GitHub 的 `digest`，CI 那一步的 SHA-256 是占位值，项目所有者填上之前 Linux 的 CI 任务在这一步失败。
```

`CLAUDE.md`——把

```markdown
- `docs/superpowers/specs/2026-10-02-phase2-m7-quic-design.md`：阶段 2 / M7 细化设计（QUIC 族：Hysteria 2、TUIC v5 / v4、MASQUE、`trust-tunnel` 的 h3、DoQ / DoH3），细化总设计的 M7 里程碑、不一致处以它为准，关闭总设计的开放问题 Q6 并处理风险 B。三份计划的拆分（M7a QUIC 地基 + Hysteria 2 → M7b TUIC → M7c HTTP/3 族与 DoQ / DoH3）；已决事项 M7-D1 ～ D12（全部都做、TUIC v4 也做、`ecn` 跟随 quinn、自写最小的 HTTP/3 客户端而不用 crates.io 的 `h3`、参考服务端、新依赖只有 `quinn` 0.11.12、`rurge-net::quic` 与新 crate `rurge-proto-quic` 的分工、每条连接一个 quinn endpoint、客户端一律 Cubic、Gecko 照官网规范而与 Surge 是否相同未证实、`W0007` 与 `Unsupported` 的退役改到 M8）；QUIC 共用部分（配置、拨号与 TLS、UDP 层、ECN、会话、错误文本）、HTTP/3 客户端、各协议的线上格式与三层测试；第 9 节是需登记的差异，第 11 节 V1 ～ V10 是写各份计划时必须核对的事项，第 12 节是任务草图，第 14 节是 M7a 计划期的订正，第 15 节是 M7a 实施期的订正。
- `docs/superpowers/plans/2026-10-02-phase2-m7a-quic-hysteria2-plan.md`：阶段 2 / M7a（QUIC 地基 + Hysteria 2）实施计划（9 个任务）。开头「计划期决定」表记录核对 quinn / quinn-proto / quinn-udp 源码、RFC 9000 / 9114 / 9204 / 7541 / 7693、Hysteria 官网规范与参考实现得出的结论和与设计文字不同的决定（`QuicOpts` 只放 `port-hopping`、`alpn` 与 `ecn` 留在原处、间隔小于 5 秒是 `W0012`、`tfo` / `udp-relay` 是 `W0028`；`tos ≠ 0` 只在发送一侧不经 quinn-udp、端口跳跃每次换到另一个端口、链式载体放不下就丢且不做 PMTU 探测、socket 层的发送错误一律丢包；HTTP/3 只开控制流、两张码表照原文录入并独立核对；单飞拨号在请求自己的 future 里、只关还开着的连接、失效连接只对请求到来时已建好的连接重试一次、跟随底层策略的去处；认证失败以 0x101 关闭、任何非 0 的 TCP 状态都是拒绝、会话号按出站递增、`Hysteria-UDP: false` 时打开载体即失败；混淆层在最外面、Salamander 丢弃不超过 8 字节的包、Gecko 的第 9 组挤掉最旧的；`port-hopping` 遇上非直连连接器在构建时失败；"未实现的协议"的例子换成 `tailscale` 等）；末尾「执行期修正记录」与「延后事项」两张表。
```

换成

```markdown
- `docs/superpowers/specs/2026-10-02-phase2-m7-quic-design.md`：阶段 2 / M7 细化设计（QUIC 族：Hysteria 2、TUIC v5 / v4、MASQUE、`trust-tunnel` 的 h3、DoQ / DoH3），细化总设计的 M7 里程碑、不一致处以它为准，关闭总设计的开放问题 Q6 并处理风险 B。三份计划的拆分（M7a QUIC 地基 + Hysteria 2 → M7b TUIC → M7c HTTP/3 族与 DoQ / DoH3）；已决事项 M7-D1 ～ D12（全部都做、TUIC v4 也做、`ecn` 跟随 quinn、自写最小的 HTTP/3 客户端而不用 crates.io 的 `h3`、参考服务端、新依赖只有 `quinn` 0.11.12、`rurge-net::quic` 与新 crate `rurge-proto-quic` 的分工、每条连接一个 quinn endpoint、客户端一律 Cubic、Gecko 照官网规范而与 Surge 是否相同未证实、`W0007` 与 `Unsupported` 的退役改到 M8）；QUIC 共用部分（配置、拨号与 TLS、UDP 层、ECN、会话、错误文本）、HTTP/3 客户端、各协议的线上格式与三层测试；第 9 节是需登记的差异，第 11 节 V1 ～ V10 是写各份计划时必须核对的事项，第 12 节是任务草图，第 14 节是 M7a 计划期的订正，第 15 节是 M7a 实施期的订正，第 16 节是 M7b 计划期的订正。
- `docs/superpowers/plans/2026-10-02-phase2-m7a-quic-hysteria2-plan.md`：阶段 2 / M7a（QUIC 地基 + Hysteria 2）实施计划（9 个任务）。开头「计划期决定」表记录核对 quinn / quinn-proto / quinn-udp 源码、RFC 9000 / 9114 / 9204 / 7541 / 7693、Hysteria 官网规范与参考实现得出的结论和与设计文字不同的决定（`QuicOpts` 只放 `port-hopping`、`alpn` 与 `ecn` 留在原处、间隔小于 5 秒是 `W0012`、`tfo` / `udp-relay` 是 `W0028`；`tos ≠ 0` 只在发送一侧不经 quinn-udp、端口跳跃每次换到另一个端口、链式载体放不下就丢且不做 PMTU 探测、socket 层的发送错误一律丢包；HTTP/3 只开控制流、两张码表照原文录入并独立核对；单飞拨号在请求自己的 future 里、只关还开着的连接、失效连接只对请求到来时已建好的连接重试一次、跟随底层策略的去处；认证失败以 0x101 关闭、任何非 0 的 TCP 状态都是拒绝、会话号按出站递增、`Hysteria-UDP: false` 时打开载体即失败；混淆层在最外面、Salamander 丢弃不超过 8 字节的包、Gecko 的第 9 组挤掉最旧的；`port-hopping` 遇上非直连连接器在构建时失败；"未实现的协议"的例子换成 `tailscale` 等）；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/superpowers/plans/2026-10-03-phase2-m7b-tuic-plan.md`：阶段 2 / M7b（TUIC v5 / v4）实施计划（6 个任务）。开头「计划期决定」表记录核对 TUIC v5 的 SPEC、v4（tuic 0.8.5）的协议说明、sing-box 与 tuic-server 的源码（GPL，只取协议事实）、quinn / rustls 的导出密钥与本仓库得出的结论和与设计文字不同的决定（两个版本共用一份 `TuicSpec`、另一个版本的凭据是 `W0028`、认证写进单向流后不等回应、v5 的认证失败按"码 0、空原因、尚未转发"推断、Connect 开流即写、UDP 模式按连接自动选择、`quic` 模式的包经每条连接一个的写任务、关联号按连接分配且换连接时取新号、心跳只在有转发时发且 v5 不在流上发、v4 放不下的包丢弃、IPv4 映射的来源按 IPv4 读、tuic 0.8.5 只在 Linux CI 上跑且它的 SHA-256 待项目所有者填写等）；末尾「执行期修正记录」与「延后事项」两张表。
```

`CLAUDE.md`——把

```markdown
cargo test -p rurge-external-tests              # external：真实拉起测试辅助程序（socks-helper）——按需拉起、参数顺序与代理变量、再拉起与 2 秒间隔、连接被拒的重试、日志、整棵进程树的停止、经引擎的端到端、检查不拉起、重载沿用与替换
cargo test -p rurge-interop                     # 对 sing-box 1.14.2（HTTP/2 族之外的全部协议，含 hysteria2 的无混淆 / Salamander / Gecko）、xray（只测 vmess）、shadowsocks-rust ssserver（只测 ss）、官方 snell-server v5.0.1（只测 snell，只在 Linux）、TrustTunnel endpoint v1.1.0（测 trust-tunnel 与 h2-connect 的 TCP，只在 Linux / macOS）与 OpenSSH sshd（只测 ssh，只在 Unix）的互操作测试；没装就跳过（RURGE_TEST_SING_BOX / RURGE_TEST_XRAY / RURGE_TEST_SSSERVER / RURGE_TEST_SNELL_SERVER / RURGE_TEST_TRUSTTUNNEL / RURGE_TEST_SSHD / RURGE_INTEROP_REQUIRED=1）
```

换成

```markdown
cargo test -p rurge-proto-quic tuic             # tuic：v5 / v4 的命令与地址、导出密钥的认证与 BLAKE3 token 的已知答案、TCP（v4 的应答）、UDP 的两种模式（v5 的分片与重组、写任务）与 Dissociate、心跳、关闭码与认证失败、失效连接的重试、端口跳跃，出站对回环假服务端（FakeTuic）
cargo test -p rurge-engine --test outbounds_tuic   # 经 tuic / tuic-v5 出站的端到端用例：TCP、UDP 的两种模式（v5 分片、v4 放不下就丢）、口令与 token 错误、端口跳跃、经 underlying-proxy（socks5）、测速、重载沿用与替换
cargo test -p rurge-external-tests              # external：真实拉起测试辅助程序（socks-helper）——按需拉起、参数顺序与代理变量、再拉起与 2 秒间隔、连接被拒的重试、日志、整棵进程树的停止、经引擎的端到端、检查不拉起、重载沿用与替换
cargo test -p rurge-interop                     # 对 sing-box 1.14.2（HTTP/2 族与 TUIC v4 之外的全部协议，含 hysteria2 的无混淆 / Salamander / Gecko 与 tuic-v5）、xray（只测 vmess）、shadowsocks-rust ssserver（只测 ss）、官方 snell-server v5.0.1（只测 snell，只在 Linux）、TrustTunnel endpoint v1.1.0（测 trust-tunnel 与 h2-connect 的 TCP，只在 Linux / macOS）、官方 tuic 0.8.5 的 tuic-server（只测 tuic，只在 Linux；只认环境变量）与 OpenSSH sshd（只测 ssh，只在 Unix）的互操作测试；没装就跳过（RURGE_TEST_SING_BOX / RURGE_TEST_XRAY / RURGE_TEST_SSSERVER / RURGE_TEST_SNELL_SERVER / RURGE_TEST_TRUSTTUNNEL / RURGE_TEST_TUIC_SERVER / RURGE_TEST_SSHD / RURGE_INTEROP_REQUIRED=1）
```

总设计（crate 布局的注释与第 13 节 M7 的参考实现）：

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
├── rurge-proto-quic/        # ★ M7：quinn；QUIC 会话；hysteria2（M7a）tuic tuic-v5 masque trust-tunnel(h3)
```

换成

```markdown
├── rurge-proto-quic/        # ★ M7：quinn；QUIC 会话；hysteria2（M7a）tuic tuic-v5（M7b）masque trust-tunnel(h3)
```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| M7 | 按 M7 细化设计 M7-D6：sing-box 1.14.2 的 `hysteria2` 入站（无混淆、Salamander、Gecko）与 `tuic` 入站（v5），三平台；官方 tuic 0.8.5（v4；GPL，只作为外部进程运行），只在 Linux CI；Vincent-bin/masque-server v0.13.0（MIT）：`masque`，只在 Linux / macOS CI；TrustTunnel endpoint v1.1.0 的 QUIC 监听：`trust-tunnel` 的 h3，只在 Linux / macOS CI。端口跳跃要在服务端做 DNAT，只有回环假服务端与手工验收。M7a 已接入 sing-box 的 `hysteria2` 入站（TCP、UDP、认证失败、两种混淆） |
```

换成

```markdown
| M7 | 按 M7 细化设计 M7-D6：sing-box 1.14.2 的 `hysteria2` 入站（无混淆、Salamander、Gecko）与 `tuic` 入站（v5），三平台；官方 tuic 0.8.5（v4；GPL，只作为外部进程运行），只在 Linux CI；Vincent-bin/masque-server v0.13.0（MIT）：`masque`，只在 Linux / macOS CI；TrustTunnel endpoint v1.1.0 的 QUIC 监听：`trust-tunnel` 的 h3，只在 Linux / macOS CI。端口跳跃要在服务端做 DNAT，只有回环假服务端与手工验收。M7a 已接入 sing-box 的 `hysteria2` 入站（TCP、UDP、认证失败、两种混淆）；M7b 已接入 sing-box 的 `tuic` 入站（v5：TCP、UDP、认证失败；它总是接受 QUIC 数据报，`quic` 模式只有回环假服务端）与 tuic 0.8.5 的服务端（v4：TCP、UDP、认证失败；它的发布没有 GitHub 的 `digest`，CI 里的 SHA-256 待项目所有者填写） |
```

- [ ] **Step 4: 核对**

- `README.md` 与 `README_en.md` 的状态一段、特性表与路线图内容一致；PRD 不需要改（它没有写 TUIC 的细节）。
- 兼容性清单的 `tuic` / `tuic-v5` 行与 P1、P2、P5、P7、P9 ～ P12 的文字一致；文档里引用的错误文本与代码一致。

- [ ] **Step 5: 门禁与提交**

跑门禁。副本上最后一次全工作区门禁：fmt、clippy 通过，`cargo test --workspace --no-fail-fast` 65 个测试二进制 1902 通过、0 失败、3 忽略。

```bash
git add tests/interop .github docs README.md README_en.md CLAUDE.md
git commit -m "docs: M7b TUIC——兼容性清单、手工验收、README 与 CLAUDE.md；sing-box 与 tuic 0.8.5 的互操作"
```

## 验收对照（设计第 1 节与第 6 节）

| # | 验收项 | 由谁保证 |
| - | ------ | -------- |
| 1 | `tuic-v5` 的 TCP 与 UDP 对参考实现转发通过 | Task 6 的 sing-box 用例（三平台 CI）；Task 2 / 3 的回环往返 |
| 2 | `tuic`（v4）的 TCP 与 UDP 对参考实现转发通过 | Task 6 的 tuic 0.8.5 用例（Linux CI）；Task 4 的回环往返 |
| 3 | 两种 UDP 模式 | Task 3 / 4 的假服务端（`no_datagrams`）；Task 5 经引擎；真实服务端都总收数据报，`quic` 模式靠手工验收 |
| 4 | 心跳 | Task 3 / 4 的心跳用例 |
| 5 | 能力表不再为两种 TUIC 出 `W0007` | Task 5 `check_knows_tuic` |
| 6 | 门禁全绿 | 各任务的门禁 |
| 7 | 需要真实节点的项目进手工验收清单 | Task 6：`docs/acceptance/phase2-manual.md` 的 M7b 一节 |

## 执行期修正记录

| # | 任务 | 与计划的出入 | 原因 |
| - | ---- | ------------ | ---- |
| 1 | Task 1 | （3c29014）`tuic-v5` 的 `uuid` 与 `vmess` 的 `username` 共用一个读取函数 `vmess::read_uuid(r, key)`（缺失与不合法两条 `E0018` 的文本按键名生成，与原来逐字相同），`parse_uuid` 改回私有；计划里 `tuic.rs` 自带一份 `read_uuid`、`parse_uuid` 改为 `pub(crate)` | Task 1 评审（Important，计划写定）：两份逐行相同的凭据读取块，"凭据不进诊断"要在两处各自维护 |
| 2 | Task 2 ～ 4 | （9460ec7）v5 认证命令的构造返回 `Result`：TLS 导出密钥失败时拨号以 `tls: the session exports no keying material` 失败；计划的代码在这里 `.expect`。没有新用例（回环上造不出导出失败） | Task 2 评审（Important，计划写定）：rustls 在握手之后的消息出错后导出即失败（服务端可以引起），`.expect` 会让请求所在的任务 panic，等着这次拨号的请求再一个个重拨、再 panic；Task 3 / 4 改写这段代码，所以放到 Task 4 之后修 |
| 3 | Task 2 ～ 4 | （9460ec7）`Dissociate` 改由连接的写入任务发出：与 `quic` 模式的包共用 `OUTBOX`（64），排在此前交出的包之后，队列满时不发（服务端的关联留到连接结束）；去掉计划里的无界通道、`JoinSet` 与异步的 `dissociate`；写入队列丢弃时的日志改为 `tuic: a message for the server is dropped: …` | Task 3 评审（Minor）：`quic` 模式下 `Dissociate` 可能赶到排队的包前面（服务端随后被迟到的包重建关联）；服务端不给单向流额度时，每丢一个载体就多一个挂起的任务 |
| 4 | Task 2 ～ 4 | （9460ec7）读任务在一个来源（数据报 / 单向流）结束后继续读另一个和已在读的流，两个都结束才返回；不用 `biased`，免得一侧饿死另一侧 | Task 3 评审（Minor）：计划的 `select!` 一侧报错即返回，另一侧缓冲里的东西随之丢掉，"服务端转发过"的标记因此可能漏标（P5 的推断随之误报） |
| 5 | Task 2 ～ 4 | （9460ec7）v4 的随机关联号在取 `associations` 锁之前抽取，锁里只在撞上在用的号（或没有随机源）时再取；单元用例 `v4_association_ids_are_drawn_at_random_past_the_ones_in_use` 改为真正走到撞号的分支 | Task 4 评审（Minor）：锁里只做内存操作；原用例只断言 1000 个号互不相同 |
| 6 | Task 2 ～ 4 | （9460ec7）第一片放下地址之后再无余地时，两个版本的文本都由 `tuic: the host name is too long for a datagram` 改为 `tuic: the address leaves no room in a datagram`（目标是 IP 时原文本不对） | Task 3 / 4 评审（Minor） |
| 7 | Task 2 ～ 4 | （9460ec7）补用例与时限：`the_credentials_never_show` 加查 token 的十六进制与 `Debug` 形式、UUID 的 `Debug` 形式；`TuicScript` 加 `no_response`（v4 对到不了的目标结束流而不应答）与用例（请求以 `tuic: the server ended the stream without a response` 失败，连接照用）；用例里可能挂住的等待加上 `timeout(BOUND, …)` | Task 2 / 3 / 4 评审（Minor） |
| 8 | 终审 | （9e2b94d）引擎 `tests/common` 的 SOCKS5 辅助函数里方法选择与 UDP ASSOCIATE 应答的读取加上 10 秒时限；TUIC 用例里其余的 `send_to` 加上时限；两个凭据错误的互操作用例在断言信息里附上服务端日志 | 终审（Task 5 / 6 评审的延后记录）：有界等待；互操作从未真正跑过，第一次 CI 失败时要靠服务端日志分辨问题在哪一边 |
| 9 | 终审 | 文档：兼容性清单的 `tuic` 行（第 3、6 条的行为与文本，v4 的 0xfffffff2 时 UDP 发送的文本，有效空闲超时取两端较小值、10 秒的保活赶不上空闲超时 10 秒的服务端）与 4.4 的适用范围加上 `tuic-v5`；`CLAUDE.md`（第 1 条、四个关闭码三条文本、`Dissociate` 经写入队列）；`tests/interop/README.md` 写明 tuic 0.8.5 的命令行、配置键与就绪日志还没有对真实二进制核对 | 各评审与终审 |
| 10 | 门禁 | 最终门禁（终审修正 9e2b94d 之后）：fmt、clippy 通过；`cargo test --workspace --no-fail-fast` 一轮通过：65 个测试二进制 1903 通过 / 0 失败 / 3 忽略 | 比计划的 1902 多出的一条是第 7 条补的用例（`no_response`） |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | tuic 0.8.5 服务端的 SHA-256：GitHub 发布资产的 `digest` 是 null，CI 步骤里是标出的占位值、填上之前 Linux 作业失败；发布里有 `.sha256sum` 文本资产 | 项目所有者决定怎样取得并填入（P15） |
| 2 | `quic` 模式（服务端不收 QUIC 数据报）没有参考服务端可测 | 手工验收 |
| 3 | v5 的"口令错"只能推断（P5）；对 sing-box 可能先表现为空隧道或流被停 | 接受，已登记 |
| 4 | `quic` 模式下引擎的发送时限不再起作用（P9） | 接受，已登记 |
| 5 | 认证或握手反复失败时没有退避（M7a 延后事项 #3，TUIC 同样） | 有用户报告再说 |
| 6 | tuic 0.8.5 的命令行（`-c`）、七个配置键与就绪日志 `Server started`：按 P14 从源码读出、没有对真实二进制跑过，又是夹具唯一的就绪判据 | 第一次在 Linux CI 上运行时核对（填上 #1 之后） |
| 7 | v5 的线上细节（分片里 `0xff` 地址之后没有端口、SIZE 是本片长度、导出密钥的 label 是 UUID 的原始字节）只对回环假服务端验证过 | 第一次 CI 的 sing-box 互操作（`v5_udp_against_sing_box`、`v5_tcp_against_sing_box`） |
| 8 | SOCKS5 客户端发往 IPv4 映射地址（`[::ffff:a.b.c.d]`）时，经 `tuic` 的回包来源按 IPv4 读（P12），引擎按来源归流时对不上，回退到这个载体最早的流（字节数与 `smart` 的静默判断记到别的流上） | M8：引擎归流时比较规范化后的地址，或入站把 SOCKS5 的目标规范化 |
| 9 | `tuic` 与 `hysteria2` 的载体辅助（每秒一次的路由比较、带时限的重新绑定、`transmit`、`recv_from`）与流的错误映射几乎逐字相同 | M7c：第三个 QUIC 协议落地时抽出共用部分 |
| 10 | `tuic/mod.rs` 约 2200 行，大半是 UDP 用例；v4 的几条用例与 v5 的几乎逐字相同 | 有空时整理（UDP 用例移进 `udp.rs`，按版本做表驱动） |
| 11 | `native` 模式的 `Dissociate` 也经 64 个的写入队列（执行期修正记录第 3 条）：一条连接上同时丢掉 64 个以上载体时，多出的不发，服务端的关联留到连接结束 | 接受 |
| 12 | 假服务端对长度不对的 v4 认证命令只是忽略、不关连接（0.8.5 怎样处理没有核对） | 有协议事实时再补 |
| 13 | 空闲超时不超过 10 秒的服务端上，M7a 的 10 秒保活赶不上：没有转发的连接约 10 秒后关闭（下一个请求重新拨号），`quic` 模式下安静的隧道随之断开 | 接受（兼容性清单已登记）；M8 再评估保活间隔 |
| 14 | v5 TCP 的 `write_all` 遇到 `ConnectionLost` 转成可重试的 `Lost` 的分支没有用例（`open_bi` 成功之后连接再断，回环上造不出确定的时序） | 接受 |
| 15 | TUIC 没有"组中继里带端口跳跃的成员被略去"与"底层组换成员后换新连接"的运行期用例，`ecn` 从工厂到 `Dialer` 的连线也没有用例（都与协议无关，`hysteria2` 的用例覆盖同一条路径；`hysteria2` 同样没有 `ecn` 连线的用例） | 可选 |
| 16 | M7a 的 `hysteria2` 用例里还有同样没加时限的 `carrier.send_to(...).await`（`hysteria2/mod.rs` 与 `hysteria2/udp.rs` 约十处；内部只在受 10 秒时限约束的重新绑定处等待） | 顺手时补上 |
