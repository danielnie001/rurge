# 阶段 2 / M7a「QUIC 地基与 Hysteria 2」Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** QUIC 族的共用地基（`rurge-net::quic` 的 UDP 层与拨号、自写的最小 HTTP/3 客户端 `rurge-net::quic::h3`、新 crate `rurge-proto-quic` 的会话）与第一种 QUIC 协议 `hysteria2`（TCP、UDP、Salamander、Gecko、`port-hopping`、`download-bandwidth`、`ecn`）。

**Architecture:** `rurge-config::spec` 新增 `QuicOpts` / `PortHopping`（`quic.rs`）与 `Hysteria2Spec`。`rurge-net::quic` 用 quinn 0.11.12：每条 QUIC 连接一个 `Endpoint`，配我们自己的 `AsyncUdpSocket` 层（直连沿用 `DirectConnector` 的 socket 选项；`tos ≠ 0` 时发送绕开 quinn-udp；`ecn=off`；经 `underlying-proxy` 的 `Datagram` 载体；端口跳跃）；`rurge-net::quic::h3` 是只用静态表的最小 HTTP/3 客户端（外加给假服务端用的服务端一侧）。新 crate `rurge-proto-quic` 的 `session` 管每个策略一条多路复用的 QUIC 连接（单飞拨号与认证、重连、空闲关闭、失效连接重试一次、跟随底层策略的变化）；`hysteria2` 模块在其上实现认证（HTTP/3 POST，233）、TCP（0x401 帧）、UDP（QUIC 数据报，分片与重组）与两种混淆。引擎工厂装上它，能力表翻转；互操作对 sing-box 1.14.2 的 `hysteria2` 入站。

**Tech Stack:** Rust 1.89 / edition 2024；新增依赖只有 `quinn` 0.11.12（`default-features = false`，features `runtime-tokio`、`rustls-ring`；随之引入 `quinn-proto` 0.11.19 与 `quinn-udp` 0.5.16）；`blake2` 0.10 已在锁文件里（只加依赖边）。

**Spec:** `docs/superpowers/specs/2026-10-02-phase2-m7-quic-design.md`（M7-D1 ～ D12；第 3、4、5 节；第 8、9 节中 M7a 的部分；第 11 节 V1–V3、V7–V10；第 12 节 M7a 草图）与总设计。与本计划「计划期决定」表不一致处，以该表为准；执行开始时一并写进设计文档新增的第 14 节。

**基线：** 本计划以 main 已经合并 `phase2-m5-review-fixes`（272f624，M5 合并后的复查：`Connector::route()`、Windows 上 `interface` 的 UDP 绑定修正、引擎 UDP 的发送失败处理等）为前提——副本就是建在它上面的。开工时 main 若还没有它，先合并它；若 main 又有了别的提交，按新的 main 重新生成本计划。

## Global Constraints

- MSRV **1.89**（CI 与本机都钉在 1.89.0），edition 2024。`unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 的两个函数例外（本计划不碰）。**本计划不新增任何 unsafe**。
- 依赖方向：`rurge-engine → rurge-proto-quic → { rurge-proto, rurge-net, rurge-config }`；`rurge-proto → rurge-net → rurge-config`；`rurge-dns → rurge-rules → rurge-net`。**新增的外部 crate 只有 `quinn` 0.11.12**（在 `[workspace.dependencies]` 里写一次：`default-features = false, features = ["runtime-tokio", "rustls-ring"]`；不碰 aws-lc-rs）；不用 `h3` / `h3-quinn` / `h3-datagram` / `hickory-net`。`blake2`、`bytes`、`tracing-subscriber`（dev）已在锁文件里，只加依赖边。不给已有 crate 引入第二个大版本。构建时 cargo 从 crates.io 拉这些 crate 的源码属于正常构建。
- **测试绝不碰公网**：只用回环 + 端口 0 + 有界等待；不用固定 sleep 当同步手段（轮询 + 截止时间；只有"断言这段时间里什么也没发生"本身是被测条件时才等一段固定时间）。**任何带 `url-test` / `fallback` / `load-balance` / `smart` 组的测试配置，`proxy-test-url` 与 `internet-test-url` 都必须指向回环**。回环 UDP 的单个载荷不超过 **9216 字节**（macOS 的缺省上限）。
- **测试与任何命令都不得修改本机的系统代理、注册表、网络设置，不得注册真实服务 / 计划任务**：不在 CLI 测试夹具之外运行 `rurge run --system-proxy`；不运行不带 `--dry-run` 的 `rurge service install | uninstall`。
- **不在本机下载或安装任何东西**——哪怕只是为了算一个校验和也不行（不装 sing-box、hysteria，不 `rustup target add`、不 `cargo install`）。互操作用例在本机没有二进制时按既有约定跳过。
- **凭据与载荷永不外泄**：`password`、`salamander-password`、`gecko-password` 与 TCP / UDP 的载荷不进日志、错误文本与 `Debug`（配置里用 `Secret<T>`；HTTP/3 里口令是 never-indexed 的敏感字段）。
- 锁里只做内存操作，不跨 `.await`；后台任务（连接驱动、端口跳跃的计时、数据报分发、空闲回收）随所属对象结束，不留孤儿任务。
- quinn 的 socket 层规矩（Task 2 核对 quinn 0.11.12 源码得出）：`AsyncUdpSocket::try_send` 除 `WouldBlock` 以外的错误会让 quinn 的驱动结束而连接挂住——各层发送失败一律丢包、不报给 quinn；交给 quinn 的 TLS 配置必须允许 TLS 1.3（否则握手时 panic）；quinn 的 `Connection::close` 会把"对端已重置"这类结局覆盖成 `LocallyClosed`——只对仍开着的连接调用它。
- rurge 专有的运行时选项只经命令行与环境变量提供，不扩展 Surge 配置格式（FR-CFG-17）。与 Surge 的每一处行为差异都登记进 `docs/surge-compatibility-matrix.md`（Task 9）。
- 日志、错误文本、CLI 输出、代码注释用英文；文档与提交标题用中文。注释密度、命名、惯用法与相邻代码保持一致；注释里不写评审轮次的标签。
- 提交信息结尾两行：`Co-Authored-By: <执行者自己的署名>` 与 `Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th`。**不 push、不 merge**，不 amend。
- 每个任务结束的门禁（全部通过才算完成）：

  ```bash
  RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check \
    && cargo clippy --all-targets -- -D warnings \
    && timeout 1500 cargo test --workspace --no-fail-fast
  ```

  `timeout` 不能省：`rurge-dns` 的一个用例曾让测试进程以 100% CPU 空转数小时（M3a「延后事项」#20）。测试二进制异常退出而没有失败用例时（`STATUS_ACCESS_VIOLATION`、`STATUS_HEAP_CORRUPTION` / `0xc0000374`、段错误——本机已知的既有问题，M3b 计划 P21），或整轮被 `timeout` 杀掉时，重跑一次并保留两次的日志，**不要在任务里去修它**。已知偶发失败的用例（`rurge-dns` 的 `a_partial_result_completes_aaaa_in_the_background` 与 `bootstrap::tests::stale_entries_are_served_and_refreshed_once`、`rurge` 的 `run::watch_reloads_rules_on_change` 与 `run::run_system_proxy_is_applied_switched_and_restored`、`rurge-engine` 的 `udp::a_closed_port_does_not_break_the_carrier`）同样重跑。**链接器报 LNK1180 / LNK1318 / LNK1285 或 "no space on device" 时是磁盘满了**：先看 `df -h /d`，删 `target/debug/incremental` 与 `target/debug/deps` 里旧的 `*.exe` / `*.pdb` 再重跑。
- 第三方 crate 的 API 以本机源码为准：`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/` 下的 `quinn-0.11.12`、`quinn-proto-0.11.19`、`quinn-udp-0.5.16`（第一次构建后出现）、`rustls-0.23.43`、`blake2-0.10.6`。
- 本机的 bash 处理不了超过约 8 KB 或含反斜杠的 heredoc（`\\` 会被改写）：新文件与含反斜杠的改动一律用写文件的工具落盘，不用 heredoc。

## Review Focus

设计没有逐条写到、而最可能伤到使用者的五类输入或失败方式；每一条都在负责它的任务里配了用例。

1. **休眠唤醒、换网或服务端重启之后**：池里的 QUIC 连接已经失效，新请求要换新连接而不是一个个超时。用例：Task 2 `keep_alive_holds_an_idle_connection`；Task 4 `a_request_that_finds_its_connection_gone_goes_again_once_on_a_new_one`、`a_connection_that_lost_its_path_is_dialed_again`；Task 5 `a_request_that_finds_the_server_restarted_goes_again_on_a_new_connection`。
2. **口令写错与混淆口令写错**：前者要以 `hysteria2: authentication failed` 清楚地失败，后者（协议里没有完整性校验）只能表现为请求超时，但不能挂住。用例：Task 5 `a_wrong_password_fails_the_authentication_and_closes_the_connection`；Task 7 `obfuscation_that_does_not_match_fails_by_the_time_of_the_request`；Task 8 `a_refused_password_fails_the_session`。
3. **大的 UDP 包、丢片与服务端不转发 UDP**：超过 QUIC 数据报上限的包要分片，丢一片只丢这一个包；服务端回 `Hysteria-UDP: false` 时 UDP 流要清楚地失败。用例：Task 6 `a_lost_fragment_loses_its_datagram_alone`、`a_server_that_relays_no_udp_refuses_the_carrier`；Task 8 `a_server_that_relays_no_udp_fails_the_flow`。
4. **底层策略组换了成员**（`underlying-proxy` 是组，测速或手选改变了当前成员）：新请求要走新成员上的新连接，UDP 载体随之迁移。用例：Task 4 `a_connection_whose_underlying_policy_changed_takes_no_new_requests`；Task 6 `carriers_move_off_a_connection_whose_underlying_policy_changed`；Task 8 `a_new_member_of_the_underlying_group_gets_a_new_connection`。
5. **`port-hopping` 遇上链式代理**：单独一行写了是 `E0018`；经组级 `underlying-proxy` 派生出的 `M (via R)` 不能悄悄地不跳或建出坏连接，而要只略去这一条。用例：Task 1 `port_hopping_over_a_chain_and_shadow_tls_are_errors_on_hysteria2`；Task 8 `a_hopping_member_of_a_relayed_group_is_left_out`。

## 计划期决定

写计划时对照设计、RFC 9000 / 9001 / 9114 / 9204 / 7541、quinn 0.11.12 / quinn-proto 0.11.19 / quinn-udp 0.5.16 / rustls 0.23.43 的本机源码、Hysteria 2 官网协议规范与 MIT 许可的参考实现（apernet/hysteria，只读）、sing-box 1.14 的文档与源码（只读）、Surge 手册（`policies/hysteria2.html`、`parameters.html`、`tls.html`、`udp.html`，2026-10-02 查阅）与本仓库源码后定下的事；与设计文档文字不同的，写进设计文档第 14 节。

**本计划里的代码不是凭空写的。** 全部 9 个任务的改动在仓库的一份副本上按任务顺序真实做了一遍（副本建在 272f624 上，用自己的构建目录），最后一次全工作区门禁见 Task 9 的 Step 5。计划里新文件的全文取自副本上该任务的提交，修改处的"把 … 换成 …"由脚本从相邻两个任务提交的差异生成，并在拼好之后按计划的顺序套到开工前的源码上逐字核对过。有单独失败步骤的任务（1、4、6、7、8），Step 2 的"预期失败"是只把该任务 Step 1 的内容套到上一个任务的状态上、真实跑出来的；其余任务是自带用例的新模块，没有单独的失败步骤。

| # | 事项 | 决定与依据 |
| - | ---- | ---------- |
| P1 | 配置的落点（细化设计 3.1） | `QuicOpts` 只有 `port_hopping: Option<PortHopping>`（间隔在 `PortHopping` 里，"有间隔无端口"不可表达）；`alpn` 仍是 TLS 参数（`TlsOpts.alpn`，QUIC 类没写时由 `read_quic_tls` 填 `h3`，同 M6c 固定 `h2` 的做法）；`ecn` 仍在 `CommonOpts`。不重复存放，重载的指纹照旧覆盖三者。`ProtoSpec::quic()` 给出 QUIC 选项（M7b / M7c 要补上各自的变体） |
| P2 | `port-hopping` 与间隔 | `;` 分隔的单个端口或 `a-b`（允许空白），端口 1–65535、`a <= b`；空值、空项、端口 0、超界、颠倒、`a-b-c`、逗号（Hysteria 自己的写法）都是 `E0018` 并引用取值。`port-hopping-interval` 是整秒（`u32`），缺省 30；**小于 5 是 `W0012`（取值无效，按 5）**，不是设计写的 `W0028`（那是"不适用、忽略"）；没写 `port-hopping` 时它是 `W0028` 且不检查取值。参考实现对小于 5 秒直接报错，rurge 按设计改用 5 |
| P3 | `hysteria2` 的参数（设计 5.1，手册核对） | `password` 只认命名写法、必填（空即缺，`E0018`，不引用；位置值 `W0001`）；`download-bandwidth` 是 `u32` Mbps（负数、小数、单位、空、超界都是 `E0018`），没写为 0；两种混淆口令都不能为空、不能同时写（`E0018`，不引用），不设最短长度。`tfo` 与 `udp-relay` 在 `hysteria2` 上是 `W0028`（QUIC 不开 TCP 连接；UDP 自动转发）；`ecn` 在 `hysteria2` 上生效，不再是 `W0029`（`wireguard` 与纯 TCP 协议照旧） |
| P4 | 两条 `E0018` 真正报出（设计 3.1） | `port-hopping` × `underlying-proxy`：在 `check_underlying` 之后判断，`underlying-proxy=DIRECT`（没有链）允许，组名算链。Shadow TLS × `hysteria2`：M2c 的判断函数本来就排除 `hysteria2`，行进了 `to_spec` 就报出。`h3=true` 的 `trust-tunnel` 不在其内（今天仍走 HTTP/2），留给 M7c |
| P5 | 分步落地的门 | 同 M6：Task 1 里 `hysteria2` 行读完并检查但暂不产出 spec，引擎工厂先放 `BuildError` 分支；Task 8 去掉 |
| P6 | 直连还是载体（细化设计 3.3 / M7-D9） | 由出站本来就拿到的连接器决定：`Connector::as_direct()`（缺省 `None`，只有 `DirectConnector` 返回自己）。直连时 QUIC 用该连接器的解析器、`SocketOpts` 与 `SocketHook` 自己开 socket——与 DIRECT 的 UDP 载体同一个入口 `DirectConnector::bind_udp`（含 99846f4 的 Windows `interface` 修正：IPv6 的 `only_v6` 在网卡绑定之前，已绑定的不再绑第二次），所以 `[Host]`、`dns-follow-interface`、`ip-version`、`interface`、`tos` 与其它协议一致；其它连接器（实际是 `ChainConnector`）走 `connect_udp` 的载体。引擎工厂照常把 `connector` 交给 `hysteria2` |
| P7 | `tos ≠ 0`（细化设计 3.3 / 3.4） | 发送绕开 quinn-udp（保住 DSCP；发出的包没有 ECN 标记、没有 GSO），**接收仍经 quinn-udp**（照样读服务端的 ECN 标记、保留 PMTU 探测）。各平台一个行为 |
| P8 | 链式载体 | quinn 拿到占位地址 `192.0.2.1:443`；不做 PMTU 探测；载体从不报 `WouldBlock`——放不下的包丢弃（`Datagram` 没有"可写"信号，报 `WouldBlock` 会让 quinn 空转）；载体结束，连接随即结束 |
| P9 | 端口跳跃（细化设计 3.3） | 端口集合合并去重（重叠的区间不加权）；第一个端口均匀随机；**每次跳到另一个端口**（参考实现可能重复）；开不了 socket 的那次跳跃跳过；地址每条连接只解析一次。socket 层不设下限（加载时已保证 ≥ 5 秒），测试直接构造 `PortHopping` 缩短间隔，引擎层的用例经只用于测试的 `common::harness_edited` 改已载入的配置 |
| P10 | 拨号（M7-D9，设计 3.2） | 每条连接一个 `Endpoint`；quinn 的 `ClientConfig` 每个出站建一次。`Transport::STANDARD`：空闲超时 30 秒、保活 10 秒、每条流窗口 8 MiB、每条连接 20 MiB、服务端不能开双向流、Cubic。错误文本 `quic: …`（传输层）与 `tls: …`（TLS 告警，不论哪一端发现） |
| P11 | 自写的 HTTP/3（M7-D5，设计 4） | 只开我们的控制流（不开 QPACK 流——动态表容量为 0 时 RFC 9204 允许；不发 GREASE）；`Client::new` 是同步的，`settings()` 是显式的"等 SETTINGS"。**`Client` 必须与连接同生共死**：丢掉它会结束我们的控制流，严格的服务端以 H3_CLOSED_CRITICAL_STREAM 关连接。错误分级：响应头超过 64 KiB 只结束该流（H3_EXCESSIVE_LOAD），畸形应答只结束该流（H3_MESSAGE_ERROR），QPACK 与帧序违规关整条连接；丢掉没读完的请求以 H3_REQUEST_CANCELLED 重置 / 停止（quinn 自己的 drop 会发 FIN、截断写了一半的帧）；隧道写入一次一个 DATA 帧、写了一半的帧下次先写完 |
| P12 | 两张表的核对（V9） | Huffman 码表与本机 h2 0.4.19 的表逐符号一致，RFC 7541 C.4 / C.6 的例子全部吻合；QPACK 静态表的 99 项另经独立核对，与 RFC 9204 附录 A（HTML 与纯文本）及 quic-go/qpack、nghttp3、quiche、ls-qpack 逐项一致 |
| P13 | QUIC 的 TLS（设计 3.2） | rurge-proto 新增 `QuicTls` 与 `build::quic_tls`，与 `TlsClient` 共用一个构建函数（同一套校验器、`sni`、`server-cert-verify-name`、指纹、`skip-cert-verify`、ALPN、`client-cert`），只用 TLS 1.3，不用 0-RTT；交给 quinn 的服务器名是文本，所以没写 `sni=` 的 IP 服务器不发 SNI |
| P14 | 会话（细化设计 3.5） | 单飞拨号在请求自己的 future 里（不另起任务，同 HTTP/2 会话池），拨号的失败只交给等它的那些请求；`Protocol` trait 给出每条连接的状态、关闭码、握手钩子与"正在离开"的判断。连接只在仍开着时才由我们关闭。空闲关闭：没有租约满 60 秒，在下一次取用或每 30 秒一次的回收时放掉（60–90 秒）。**失效连接重试一次**：请求到来时就已就绪的连接，在请求自己的数据发出之前被发现已失效（`Failure::Lost`），换新连接再试一次。会话内部不设时限 |
| P15 | 跟随底层策略（承接 2b0d166） | 拨号前读一次 `connector.route()` 记在连接上，取用时比较（每个请求、UDP 载体的每次发送），不设计时器；变了的连接当作"正在离开"——不再接新的工作（即使后来又变回同一个对象），已有的流走完，最后一个租约放手时关闭；每条连接记一次 INFO 日志 `hysteria2: the underlying policy changed; a new connection is made`。直连时 `route()` 两边都是 `None`，没有开销 |
| P16 | Hysteria 2 的认证（设计 5.2，参考实现核对） | 认证失败时以 H3_GENERAL_PROTOCOL_ERROR（0x101）关闭连接（参考实现 `core/client/client.go`）；`Hysteria-UDP` 按 Go 的 `ParseBool` 读（缺少或不认识即 false）；口令是 never-indexed 的敏感字段；含控制字符（TAB 除外）的口令发不出去，建出站时报错、加载时经干构建报在该行 |
| P17 | Hysteria 2 的 TCP（细化设计 5.3） | 构造函数 `Hysteria2Outbound::new(name, server, spec, ecn, keystore, roots, connector)`（没有 `shadow_tls`：加载时已是 `E0018`）。**应答状态非 0 即拒绝**（设计写的是 1），消息经 `rurge_proto::outbound::untrusted_text` 清洗、截到 200 字符；地址 ≤ 2048 字节（否则拨号前就拒绝）、消息 ≤ 2048、填充 ≤ 4096（否则 `the server's answer is malformed`）；填充一律是字母与数字（认证 256–2047、TCP 64–511）；失效连接的重试覆盖"收到应答之前"的任何连接丢失（不做 fast open，此时调用方的数据一个字节都没发出） |
| P18 | Hysteria 2 的 UDP（细化设计 5.3） | `Hysteria-UDP: false`（或服务端不支持 QUIC 数据报）时 `open_udp` 本身失败：`hysteria2: the server does not allow UDP`（同 snell / socks5 报服务端拒绝 UDP 的做法）。**会话号按出站从 1 计、载体终生不变**（设计写的是每条连接内计数——那样换连接后号码可能被别的载体占去）；载体跨连接存活：所在连接关闭、正在离开或底层策略变了，下一次发送就迁到会话当前的连接上。发送时连接刚好结束的那个包丢失、不重发；超长或发不出的包是 `InvalidInput` 且什么也不发；重组照参考实现的语义，每个未完成的包至多 64 KiB |
| P19 | 混淆（细化设计 5.4） | 混淆层经 `Dialer::with_layer` 套在最外（直连、端口跳跃、载体、`ecn=off` 之外），每条连接一个。Salamander 丢掉 8 字节及以下的包（参考实现）；没有完整性校验，口令错只表现为请求超时。Gecko：每个长头包都分成 2–8 片，解混淆后最高位为 1 即按 Gecko 帧解析（低 7 位忽略，同参考实现）；待重组的包满 8 个时**挤掉最旧的**（参考实现拒绝新的），8 秒过期、到时才检查；某一片 `WouldBlock` 时整包丢弃。口令只以带密钥的 BLAKE2b 状态保存 |
| P20 | 引擎（Task 8） | `port-hopping` 遇上非直连的连接器时在 `Dialer::new` 里拒绝（`BuildError`，`E0018` 的原文）——经组级 `underlying-proxy` 派生出的 `M (via R)` 因此只略去这一条（M3a 的行为），M7b / M7c 也随之继承。测试里"未实现的协议"的例子从 `hysteria2` 换成 `tailscale`（订阅里那一行不带 `section-name`：订阅安全门会拒绝它）；原先借它的 `password=x` 检查脱敏的用例改用一行真正带凭据的 `trojan`。bin 关掉 `quinn` / `quinn_proto` 的日志，`quinn_udp` 留 WARN（只有它记系统拒发的包） |
| P21 | 吞吐基准（不作门禁） | release 下双向各 64 MiB：不混淆 69–80 MiB/s，Salamander 56–66 MiB/s（开销约 18%）。基准自带快速回显、假服务端每次转发 64 KiB——否则测到的是测试夹具（约 12 MiB/s） |
| P22 | 互操作（M7-D6） | sing-box 1.14.2 的 `hysteria2` 入站（只监听回环、夹具的 CA、`alpn: ["h3"]`、一个用户、可选 Salamander / Gecko；没有 masquerade，所以口令错时答 404）与一个列在最后、只用来判断就绪的 `mixed` 入站。CI 已装 sing-box 1.14.2，不用改。端口跳跃要服务端配 DNAT，只能靠假服务端与手工验收 |

## 承接事项

| # | 来源 | 事项 | 处理 | 任务 |
| - | ---- | ---- | ---- | ---- |
| C1 | M7 设计第 12 节 | M7a 的全部内容 | 本计划 | 1–9 |
| C2 | M5 设计 M5-D6 | `ecn` 延后到 QUIC 族 | 本计划（M7-D4，P7） | 1、2、8、9 |
| C3 | 总设计风险 B、开放问题 Q6 | `h3` 的成熟度；MASQUE 与 `trust-tunnel` h3 的参考服务端 | M7-D5（自写 HTTP/3）与 M7-D6；总设计在 Task 9 更新 | 3、9 |
| C4 | M5 合并后的复查（272f624） | `Connector::route()` 与 Windows `interface` 的 UDP 绑定 | P6、P15 | 2、4、6 |
| C5 | M3b #7（P21） | 测试二进制偶发崩溃 | 照旧：门禁遇到就重跑 | — |

## File Structure

新建：

| 文件 | 职责 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/spec/quic.rs` | `QuicOpts`、`PortHopping`、`read_quic` / `read_quic_tls`（与用例） | 1 |
| `crates/rurge-config/src/spec/hysteria2.rs` | `Hysteria2Spec`、`Hysteria2Obfs`、`read_hysteria2`（与用例） | 1 |
| `crates/rurge-net/src/quic/{mod.rs, socket.rs, hop.rs, carrier.rs, dial.rs, testing.rs}` | UDP 层（直连、`tos`、`ecn=off`、载体、端口跳跃）、拨号与传输参数、测试用的 quinn 服务端（与用例） | 2 |
| `crates/rurge-net/src/quic/h3/{mod.rs, frame.rs, qpack.rs, huffman.rs, stream.rs, client.rs, testing.rs}` | 最小 HTTP/3 客户端与给假服务端用的服务端一侧（与用例） | 3 |
| `crates/rurge-proto-quic/{Cargo.toml, src/lib.rs, src/session.rs}` | 新 crate 与会话（与用例） | 4 |
| `crates/rurge-proto-quic/src/testing.rs` → `src/testing/mod.rs` | `ServerPort` 等测试设施；Task 5 改成目录 | 4、5 |
| `crates/rurge-proto-quic/src/varint.rs` | QUIC 变长整数（Hysteria 的字段用） | 5 |
| `crates/rurge-proto-quic/src/hysteria2/{mod.rs, auth.rs, tcp.rs}` | `Hysteria2Outbound`、认证、TCP（与用例） | 5 |
| `crates/rurge-proto-quic/src/hysteria2/udp.rs` | UDP 载体、分片与重组、分发（与用例） | 6 |
| `crates/rurge-proto-quic/src/hysteria2/obfs/{mod.rs, salamander.rs, gecko.rs}` | 混淆层与两种算法（与用例） | 7 |
| `crates/rurge-proto-quic/src/testing/{hysteria2.rs, obfs.rs}` | `FakeHysteria2`（认证、TCP、UDP、端口跳跃、重启）与它自己的混淆实现 | 5、6、7 |
| `crates/rurge-engine/tests/outbounds_hysteria2.rs` | 经引擎的端到端用例 | 8 |
| `tests/interop/tests/sing_box_hysteria2.rs` | 对 sing-box 的互操作用例 | 9 |

修改：`crates/rurge-config/src/{spec/mod.rs, redact.rs}` 与配置的用例（1、8）；`Cargo.toml`（2、4、7）；`crates/rurge-net/{Cargo.toml, src/lib.rs, src/connector.rs}`（2）；`crates/rurge-proto/src/{transport/tls.rs, build.rs, outbound.rs}`（4、5）；`crates/rurge-engine/{Cargo.toml, src/outbounds.rs}` 与引擎、策略、API、CLI 的用例（1、8）；`crates/rurge/src/{capabilities.rs, cli/run.rs}`（8）；`tests/interop/{src/lib.rs, README.md}` 与文档（9）。

## 任务一览

| 任务 | 交付物 | 依赖 |
| ---- | ------ | ---- |
| 1 | `QuicOpts` / `Hysteria2Spec`、两条 `E0018`、脱敏 | — |
| 2 | `rurge-net::quic`：quinn、UDP 层、拨号 | — |
| 3 | `rurge-net::quic::h3`：最小 HTTP/3 客户端 | 2 |
| 4 | `QuicTls`、新 crate `rurge-proto-quic` 与 `session` | 2 |
| 5 | Hysteria 2 的认证与 TCP、`FakeHysteria2` | 1、3、4 |
| 6 | Hysteria 2 的 UDP | 5 |
| 7 | Salamander 与 Gecko | 5、6 |
| 8 | 引擎装配、能力表翻转、经引擎的用例、吞吐基准 | 1–7 |
| 9 | 互操作（sing-box）与文档 | 1–8 |

---

### Task 1: QUIC 类的共用配置与 `hysteria2` 的配置

`QuicOpts` / `PortHopping`（P1、P2）与 `Hysteria2Spec`（P3）；两条 `E0018`（P4）；脱敏名单加 `salamander-password` 与 `gecko-password`；`hysteria2` 行读完并检查但暂不产出 spec，引擎工厂先放占位分支（P5）。

**Files:**
- Create: `crates/rurge-config/src/spec/quic.rs`、`crates/rurge-config/src/spec/hysteria2.rs`（均自带用例）
- Modify: `crates/rurge-config/src/spec/mod.rs`（与用例）、`src/redact.rs`（与用例）、`tests/policy_spec.rs`、`crates/rurge-engine/src/outbounds.rs`

**Interfaces:**
- Consumes: 既有的 `ParamReader`、`read_tls`、`read_common`、`check_underlying`、`shadow_tls::allowed_on`、`Secret<T>`、诊断码（`E0018`、`W0001`、`W0012`、`W0028`、`W0029`）。
- Produces: `rurge_config::spec::quic`：`DEFAULT_HOP_INTERVAL`（30 秒）、`MIN_HOP_INTERVAL`（5 秒）、`pub struct QuicOpts { pub port_hopping: Option<PortHopping> }`、`pub struct PortHopping { pub ports: Vec<RangeInclusive<u16>>, pub interval: Duration }`、`read_quic`、`read_quic_tls`（`pub(crate)`）；`rurge_config::spec::hysteria2`：`pub struct Hysteria2Spec { tls, quic, password: Secret<String>, download_bandwidth: u32, obfs: Option<Hysteria2Obfs> }`、`pub enum Hysteria2Obfs { Salamander(Secret<String>), Gecko(Secret<String>) }`、`read_hysteria2`；`ProtoSpec::Hysteria2`、`ProtoSpec::quic()`

- [ ] **Step 1: 先写用例**

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        let o = outcome("H", "hysteria2, h, 443, password=x, mystery=1");
```

换成

```rust
        let o = outcome("T", "tailscale, section-name=ts, mystery=1");
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        assert!(tt.tls().is_some());
    }
}
```

换成

```rust
        assert!(tt.tls().is_some());
    }

    /// A `hysteria2` line is read and checked in full; it has no spec until
    /// the engine builds `hysteria2` (M7a task 8). `ecn` takes effect on it,
    /// `tfo` and `udp-relay` do not apply (phase 2 M7 design 3.1, 5.1).
    #[test]
    fn a_hysteria2_line_is_checked_but_has_no_spec_yet() {
        for def in [
            "hysteria2, 192.168.20.6, 443, password=pwd, download-bandwidth=100",
            "hysteria2, h.test, 443, password=pw, salamander-password=obfs, port-hopping=1234;5000-6000, port-hopping-interval=10, ecn=off, sni=edge.test, client-cert=cert1",
            "hysteria2, h.test, 443, password=pw, gecko-password=obfs, alpn=h3, underlying-proxy=Pick",
            "hysteria2, h.test, 443, password=pw, port-hopping=5000-6000, underlying-proxy=DIRECT",
        ] {
            let o = outcome("H", def);
            assert!(o.diagnostics.is_empty(), "{def}: {:?}", o.diagnostics);
            assert!(o.inert.is_empty(), "{def}: {:?}", o.inert);
            assert!(o.spec.is_none() && o.not_implemented.is_none(), "{def}");
        }
        let o = outcome(
            "H",
            "hysteria2, h.test, 443, s3cret, tfo=true, udp-relay=true, mystery=1",
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
                    "policy `H`: `tfo` does not apply to `hysteria2` policies; ignored"
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `H`: `password` is required"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `H`: `udp-relay` does not apply to `hysteria2` policies (UDP is always relayed); ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `H`: unknown parameter `mystery` ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `H`: unexpected positional value #1 ignored"
                ),
            ]
        );
        assert!(o.inert.is_empty(), "{:?}", o.inert);
    }

    /// What a QUIC-based line cannot combine (phase 2 M7 design 3.1): port
    /// hopping with a chain (manual), and Shadow TLS, which wraps a TCP
    /// connection. `underlying-proxy=DIRECT` is no chain.
    #[test]
    fn port_hopping_over_a_chain_and_shadow_tls_are_errors_on_hysteria2() {
        for relay in ["Entry", "Pick"] {
            let o = outcome(
                "H",
                &format!(
                    "hysteria2, h.test, 443, password=pw, port-hopping=5000-6000, underlying-proxy={relay}"
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
                    "policy `H`: `port-hopping` cannot be combined with `underlying-proxy`"
                )],
                "{relay}"
            );
        }
        let o = outcome(
            "H",
            "hysteria2, h.test, 443, password=pw, shadow-tls-password=st, shadow-tls-version=3, shadow-tls-sni=site.test",
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
                "policy `H`: Shadow TLS cannot be combined with a `hysteria2` policy"
            )]
        );
    }

    /// Hysteria 2 runs TLS over QUIC: the TLS options, and with them the
    /// client certificate, and the QUIC options are its own.
    #[test]
    fn a_hysteria2_spec_has_tls_and_quic_options() {
        let hysteria2 = ProtoSpec::Hysteria2(Hysteria2Spec {
            tls: TlsOpts {
                alpn: vec!["h3".into()],
                client_cert: Some("cert1".into()),
                ..TlsOpts::default()
            },
            quic: QuicOpts {
                port_hopping: Some(PortHopping {
                    ports: vec![1234..=1234, 5000..=6000],
                    interval: quic::DEFAULT_HOP_INTERVAL,
                }),
            },
            password: "pw".into(),
            download_bandwidth: 0,
            obfs: Some(Hysteria2Obfs::Salamander("obfs".into())),
        });
        assert_eq!(hysteria2.keystore_item(), Some("cert1"));
        assert_eq!(
            hysteria2.tls().map(|tls| tls.alpn.clone()),
            Some(vec!["h3".to_string()])
        );
        assert!(
            hysteria2
                .quic()
                .is_some_and(|quic| quic.port_hopping.is_some())
        );
        let tcp = outcome("A", "anytls, h.test, 443, password=pw")
            .spec
            .unwrap();
        assert!(tcp.proto.quic().is_none() && ProtoSpec::Direct.quic().is_none());
    }
}
```

`crates/rurge-config/src/redact.rs`——把

```rust
            "trust-tunnel, 192.168.20.62, 443, username=***, password=***, headers=***, h3=true"
        );
    }
}

```

换成

```rust
            "trust-tunnel, 192.168.20.62, 443, username=***, password=***, headers=***, h3=true"
        );
    }

    /// Hysteria 2 (phase 2 M7 design 8): the password and the obfuscation
    /// password, Salamander or Gecko; `download-bandwidth` and the hopping
    /// ports stay.
    #[test]
    fn a_hysteria2_line_loses_its_passwords() {
        assert_eq!(
            redact_definition(
                "hysteria2, h.test, 443, password=s3cret, download-bandwidth=100, salamander-password=0bfs, port-hopping=1234;5000-6000"
            ),
            "hysteria2, h.test, 443, password=***, download-bandwidth=100, salamander-password=***, port-hopping=1234;5000-6000"
        );
        assert_eq!(
            redact_profile("H = hysteria2, h.test, 443, password=s3cret, gecko-password=0bfs"),
            "H = hysteria2, h.test, 443, password=***, gecko-password=***"
        );
    }
}

```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
        [codes::E_UNKNOWN_POLICY_REF]
    );
}

```

换成

```rust
        [codes::E_UNKNOWN_POLICY_REF]
    );
}

/// `hysteria2` lines are read and checked at load (phase 2 M7 design 3.1,
/// 5.1): the combinations they cannot have are errors, and `ecn` has an
/// effect on them. They have no spec until the engine builds them (M7a
/// task 8).
#[test]
fn hysteria2_lines_are_checked_at_load() {
    let loaded = load(
        "A = hysteria2, a.example, 443, password=p, port-hopping=5000-6000, underlying-proxy=E
B = hysteria2, b.example, 443, password=p, salamander-password=s, gecko-password=g
C = hysteria2, c.example, 443, password=p, shadow-tls-password=st
D = hysteria2, d.example, 443, password=p, ecn=off, port-hopping=5000-6000, port-hopping-interval=2
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
                "policy `B`: `salamander-password` cannot be combined with `gecko-password`"
                    .to_string(),
                3
            ),
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `C`: Shadow TLS cannot be combined with a `hysteria2` policy".to_string(),
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

```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-config --lib`
Expected: FAIL——两个 spec、`ProtoSpec` 的变体与 `quic()` 由 Step 3 引入，编译不过：

```text
error[E0422]: cannot find struct, variant or union type `Hysteria2Spec` in this scope
    --> crates\rurge-config\src\spec\mod.rs:1213:46
error[E0422]: cannot find struct, variant or union type `QuicOpts` in this scope
    --> crates\rurge-config\src\spec\mod.rs:1219:19
error[E0422]: cannot find struct, variant or union type `PortHopping` in this scope
    --> crates\rurge-config\src\spec\mod.rs:1220:36
error[E0599]: no variant or associated item named `Hysteria2` found for enum `spec::ProtoSpec` in the current scope
    --> crates\rurge-config\src\spec\mod.rs:1213:36
error[E0433]: failed to resolve: use of unresolved module or unlinked crate `quic`
    --> crates\rurge-config\src\spec\mod.rs:1222:31
error[E0433]: failed to resolve: use of undeclared type `Hysteria2Obfs`
    --> crates\rurge-config\src\spec\mod.rs:1227:24
error[E0599]: no method named `quic` found for enum `spec::ProtoSpec` in the current scope
    --> crates\rurge-config\src\spec\mod.rs:1242:27
error[E0599]: no method named `quic` found for enum `spec::ProtoSpec` in the current scope
    --> crates\rurge-config\src\spec\mod.rs:1242:65
Some errors have detailed explanations: E0422, E0433, E0599.
For more information about an error, try `rustc --explain E0422`.
error: could not compile `rurge-config` (lib test) due to 8 previous errors
```

（`cargo test -p rurge-config --test policy_spec` 能编译，`hysteria2_lines_are_checked_at_load` 以 `left: []` 失败：今天对这些行什么也不报。）

- [ ] **Step 3: 实现**

新建 `crates/rurge-config/src/spec/quic.rs`：

```rust
//! What the QUIC-based protocols share on their lines (phase 2 M7 design
//! 3.1): `port-hopping` and `port-hopping-interval`. Of the design's other
//! two, `alpn` is a TLS parameter (`TlsOpts`, `h3` when the line has none)
//! and `ecn` a common one (`CommonOpts`).

use super::common::CommonOpts;
use super::reader::ParamReader;
use super::tls::{TlsOpts, read_tls};
use crate::diagnostic::codes;
use crate::keystore::KeystoreItem;
use std::ops::RangeInclusive;
use std::time::Duration;

/// `port-hopping-interval` when the line has none (manual).
pub const DEFAULT_HOP_INTERVAL: Duration = Duration::from_secs(30);

/// The shortest `port-hopping-interval`: the reference implementation
/// refuses anything shorter; rurge raises it to this.
pub const MIN_HOP_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuicOpts {
    /// `port-hopping`: the server's ports to rotate among; the line's own
    /// port is then not used (manual).
    pub port_hopping: Option<PortHopping>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortHopping {
    /// The ports and inclusive ranges in the order written; a single port is
    /// a range of one.
    pub ports: Vec<RangeInclusive<u16>>,
    /// `port-hopping-interval`: how long one port is used, at least
    /// `MIN_HOP_INTERVAL`.
    pub interval: Duration,
}

/// The TLS options of a QUIC-based protocol: `alpn` is `h3` when the line
/// has none (manual: Policies › TLS).
pub(crate) fn read_quic_tls(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> TlsOpts {
    let mut tls = read_tls(r, keystore);
    if tls.alpn.is_empty() {
        tls.alpn = vec!["h3".to_string()];
    }
    tls
}

/// The QUIC options on the line, and `tfo` taken out of `common`: a QUIC
/// protocol opens no TCP connection, so it does not apply (`W0028`). After
/// an error was reported the returned value is meaningless: the caller
/// checks `r.has_errors()`.
pub fn read_quic(r: &mut ParamReader<'_>, common: &mut CommonOpts) -> QuicOpts {
    if r.has("tfo") {
        let kind = r.policy().kind.keyword();
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            format!("`tfo` does not apply to `{kind}` policies; ignored"),
        );
    }
    common.tfo = false;
    let Some(v) = r.str("port-hopping") else {
        if r.has("port-hopping-interval") {
            r.touch("port-hopping-interval");
            r.warn(
                codes::W_PARAM_NOT_APPLICABLE,
                "`port-hopping-interval` has no effect without `port-hopping`; ignored".to_string(),
            );
        }
        return QuicOpts::default();
    };
    let ports = match parse_ports(v) {
        Some(ports) => ports,
        None => {
            r.invalid(
                "port-hopping",
                v,
                "ports from 1 to 65535 and ranges such as `5000-6000`, separated by `;`",
            );
            Vec::new()
        }
    };
    let interval = read_interval(r);
    QuicOpts {
        port_hopping: Some(PortHopping { ports, interval }),
    }
}

/// `1234;5000-6000`: ports and inclusive ranges, separated by `;`. `None`
/// when an item is empty, is not a port from 1 to 65535, or is a range that
/// runs backwards.
fn parse_ports(value: &str) -> Option<Vec<RangeInclusive<u16>>> {
    let port = |s: &str| s.trim().parse::<u16>().ok().filter(|p| *p != 0);
    value
        .split(';')
        .map(|item| match item.split_once('-') {
            Some((first, last)) => {
                let (first, last) = (port(first)?, port(last)?);
                (first <= last).then_some(first..=last)
            }
            None => port(item).map(|p| p..=p),
        })
        .collect()
}

/// `port-hopping-interval` in whole seconds; a shorter one than
/// `MIN_HOP_INTERVAL` is raised to it with a warning (`W0012`).
fn read_interval(r: &mut ParamReader<'_>) -> Duration {
    let Some(v) = r.str("port-hopping-interval") else {
        return DEFAULT_HOP_INTERVAL;
    };
    let Ok(secs) = v.trim().parse::<u32>() else {
        r.invalid("port-hopping-interval", v, "a whole number of seconds");
        return DEFAULT_HOP_INTERVAL;
    };
    let interval = Duration::from_secs(secs.into());
    if interval < MIN_HOP_INTERVAL {
        let min = MIN_HOP_INTERVAL.as_secs();
        r.warn(
            codes::W_INVALID_VALUE,
            format!("`port-hopping-interval` is below the minimum of {min} seconds; using {min}"),
        );
        return MIN_HOP_INTERVAL;
    }
    interval
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use crate::spec::common::{Applies, Notes, read_common};
    use std::path::Path;
    use std::sync::Arc;

    fn read(def: &str) -> (QuicOpts, CommonOpts, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let mut common = read_common(&mut r, Applies::Proxy, &mut Notes::default());
        // what the protocol reads for itself
        r.touch("password");
        let quic = read_quic(&mut r, &mut common);
        let failed = r.has_errors();
        (quic, common, failed, r.finish())
    }

    fn messages(diags: &[Diagnostic]) -> Vec<(&str, &str)> {
        diags.iter().map(|d| (d.code, d.message.as_str())).collect()
    }

    fn hopping(ports: Vec<RangeInclusive<u16>>, secs: u64) -> Option<PortHopping> {
        Some(PortHopping {
            ports,
            interval: Duration::from_secs(secs),
        })
    }

    #[test]
    fn the_manuals_port_list_and_the_default_interval() {
        let (quic, _, failed, diags) =
            read("hysteria2, h.test, 443, password=p, port-hopping=1234;5000-6000");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(
            quic.port_hopping,
            hopping(vec![1234..=1234, 5000..=6000], 30)
        );
        // blanks around the items, a range of one, the order as written
        let (quic, _, failed, diags) = read(
            "hysteria2, h.test, 443, password=p, port-hopping=\" 20000 - 50000 ; 8443-8443;443 \"",
        );
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(
            quic.port_hopping,
            hopping(vec![20000..=50000, 8443..=8443, 443..=443], 30)
        );
        // nothing written, no hopping
        let (quic, _, failed, diags) = read("hysteria2, h.test, 443, password=p");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(quic, QuicOpts::default());
    }

    #[test]
    fn a_bad_port_list_is_an_error_that_quotes_it() {
        for bad in [
            "",
            "0",
            "65536",
            "6000-5000",
            "5000-",
            "-6000",
            "1-2-3",
            "1234;",
            "1234;;5000",
            "443,8443",
            "https",
        ] {
            let (_, _, failed, diags) = read(&format!(
                "hysteria2, h.test, 443, password=p, port-hopping=\"{bad}\""
            ));
            assert!(failed, "{bad}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    format!(
                        "policy `P`: invalid value `{bad}` for `port-hopping` (expected ports from 1 to 65535 and ranges such as `5000-6000`, separated by `;`)"
                    )
                    .as_str()
                )]
            );
        }
    }

    /// 30 seconds unless written (manual); shorter than 5 is raised to 5,
    /// the floor of the reference implementation (phase 2 M7 design 3.1).
    #[test]
    fn the_interval_is_whole_seconds_and_at_least_5() {
        for (written, secs) in [("60", 60), ("5", 5), (" 10 ", 10)] {
            let (quic, _, failed, diags) = read(&format!(
                "hysteria2, h.test, 443, password=p, port-hopping=5000-6000, port-hopping-interval=\"{written}\""
            ));
            assert!(!failed && diags.is_empty(), "{written}: {diags:?}");
            assert_eq!(quic.port_hopping, hopping(vec![5000..=6000], secs));
        }
        for short in ["4", "0"] {
            let (quic, _, failed, diags) = read(&format!(
                "hysteria2, h.test, 443, password=p, port-hopping=5000-6000, port-hopping-interval={short}"
            ));
            assert!(!failed, "{short}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::W_INVALID_VALUE,
                    "policy `P`: `port-hopping-interval` is below the minimum of 5 seconds; using 5"
                )]
            );
            assert_eq!(quic.port_hopping, hopping(vec![5000..=6000], 5));
        }
        for bad in ["-1", "2.5", "30s", "", "4294967296"] {
            let (_, _, failed, diags) = read(&format!(
                "hysteria2, h.test, 443, password=p, port-hopping=5000-6000, port-hopping-interval=\"{bad}\""
            ));
            assert!(failed, "{bad}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    format!(
                        "policy `P`: invalid value `{bad}` for `port-hopping-interval` (expected a whole number of seconds)"
                    )
                    .as_str()
                )]
            );
        }
    }

    #[test]
    fn the_interval_has_no_effect_without_ports() {
        for interval in ["10", "soon"] {
            let (quic, _, failed, diags) = read(&format!(
                "hysteria2, h.test, 443, password=p, port-hopping-interval={interval}"
            ));
            assert!(!failed, "{interval}");
            assert_eq!(quic, QuicOpts::default());
            assert_eq!(
                messages(&diags),
                [(
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `port-hopping-interval` has no effect without `port-hopping`; ignored"
                )]
            );
        }
    }

    /// QUIC runs over UDP: TCP Fast Open has nothing to act on.
    #[test]
    fn tfo_does_not_apply() {
        let (_, common, failed, diags) = read("hysteria2, h.test, 443, password=p, tfo=true");
        assert!(!failed);
        assert!(!common.tfo);
        assert_eq!(
            messages(&diags),
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `P`: `tfo` does not apply to `hysteria2` policies; ignored"
            )]
        );
    }

    #[test]
    fn alpn_is_h3_unless_written() {
        for (def, alpn) in [
            ("hysteria2, h.test, 443", vec!["h3"]),
            ("hysteria2, h.test, 443, alpn=", vec!["h3"]),
            ("hysteria2, h.test, 443, alpn=h3", vec!["h3"]),
            (
                "hysteria2, h.test, 443, alpn=\"h3-29, h3\"",
                vec!["h3-29", "h3"],
            ),
        ] {
            let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
            let mut r = ParamReader::new(&p);
            let tls = read_quic_tls(&mut r, &[]);
            assert!(r.finish().is_empty(), "{def}");
            assert_eq!(tls.alpn, alpn, "{def}");
        }
    }
}
```

新建 `crates/rurge-config/src/spec/hysteria2.rs`：

```rust
//! `hysteria2` policy parameters (manual: Policies › Hysteria 2; phase 2 M7
//! design 5.1): TLS over QUIC, each UDP packet of which may be obfuscated
//! as well (Salamander or Gecko).

use super::common::CommonOpts;
use super::quic::{QuicOpts, read_quic, read_quic_tls};
use super::reader::ParamReader;
use super::secret::Secret;
use super::tls::TlsOpts;
use crate::diagnostic::codes;
use crate::keystore::KeystoreItem;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hysteria2Spec {
    /// Hysteria 2 always runs TLS over QUIC; `alpn` is `h3` unless written.
    pub tls: TlsOpts,
    pub quic: QuicOpts,
    pub password: Secret<String>,
    /// `download-bandwidth` in Mbps, `0` when not written: a hint for the
    /// server's congestion control; the client's own sending does not
    /// follow it (M7-D10).
    pub download_bandwidth: u32,
    /// `salamander-password` or `gecko-password`.
    pub obfs: Option<Hysteria2Obfs>,
}

/// The obfuscation every UDP packet of the connection goes through (phase 2
/// M7 design 5.4), with its password.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hysteria2Obfs {
    Salamander(Secret<String>),
    Gecko(Secret<String>),
}

/// Everything `hysteria2`-specific on the line, and `tfo` taken out of
/// `common` (`read_quic`). After an error was reported the returned value
/// is meaningless: the caller checks `r.has_errors()`.
///
/// The passwords are named-only, as the manual writes them, and never
/// quoted in a diagnostic: a positional value stays unread and is reported
/// as an extra positional value.
pub fn read_hysteria2(
    r: &mut ParamReader<'_>,
    common: &mut CommonOpts,
    keystore: &[KeystoreItem],
) -> Hysteria2Spec {
    let tls = read_quic_tls(r, keystore);
    let quic = read_quic(r, common);
    let password = r.str("password").unwrap_or_default();
    if password.is_empty() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`password` is required".to_string(),
        );
    }
    let download_bandwidth = r
        .number::<u32>("download-bandwidth", "a whole number of Mbps")
        .unwrap_or(0);
    let obfs = read_obfuscation(r);
    if r.has("udp-relay") {
        r.touch("udp-relay");
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            "`udp-relay` does not apply to `hysteria2` policies (UDP is always relayed); ignored"
                .to_string(),
        );
    }
    Hysteria2Spec {
        tls,
        quic,
        password: password.into(),
        download_bandwidth,
        obfs,
    }
}

/// `salamander-password` or `gecko-password`: not both (manual), and not
/// empty.
fn read_obfuscation(r: &mut ParamReader<'_>) -> Option<Hysteria2Obfs> {
    let salamander = r.str("salamander-password");
    let gecko = r.str("gecko-password");
    for (key, password) in [
        ("salamander-password", salamander),
        ("gecko-password", gecko),
    ] {
        if password.is_some_and(str::is_empty) {
            r.error(codes::E_INVALID_POLICY_PARAM, format!("`{key}` is empty"));
        }
    }
    match (salamander, gecko) {
        (Some(_), Some(_)) => {
            r.error(
                codes::E_INVALID_POLICY_PARAM,
                "`salamander-password` cannot be combined with `gecko-password`".to_string(),
            );
            None
        }
        (Some(password), None) => Some(Hysteria2Obfs::Salamander(password.into())),
        (None, Some(password)) => Some(Hysteria2Obfs::Gecko(password.into())),
        (None, None) => None,
    }
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

    fn read(def: &str) -> (Hysteria2Spec, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let mut common = read_common(&mut r, Applies::Proxy, &mut Notes::default());
        let spec = read_hysteria2(&mut r, &mut common, &[]);
        let failed = r.has_errors();
        (spec, failed, r.finish())
    }

    fn messages(diags: &[Diagnostic]) -> Vec<(&str, &str)> {
        diags.iter().map(|d| (d.code, d.message.as_str())).collect()
    }

    #[test]
    fn the_manuals_example() {
        let (spec, failed, diags) =
            read("hysteria2, 192.168.20.6, 443, password=pwd, download-bandwidth=100");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.password.expose(), "pwd");
        assert_eq!(spec.download_bandwidth, 100);
        assert_eq!(spec.obfs, None);
        assert_eq!(spec.quic, QuicOpts::default());
        assert_eq!(
            spec.tls,
            TlsOpts {
                alpn: vec!["h3".into()],
                ..TlsOpts::default()
            }
        );
        // without `download-bandwidth` the server is told 0: unknown
        let (spec, failed, _) = read("hysteria2, h.test, 443, password=pw");
        assert!(!failed);
        assert_eq!(spec.download_bandwidth, 0);
    }

    #[test]
    fn every_parameter() {
        let (spec, failed, diags) = read(
            "hysteria2, h.test, 443, password=pw, download-bandwidth=0, gecko-password=0bfs, port-hopping=443;20000-30000, port-hopping-interval=60, alpn=h3-29, sni=edge.test, skip-cert-verify=true",
        );
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.download_bandwidth, 0);
        assert_eq!(spec.obfs, Some(Hysteria2Obfs::Gecko("0bfs".into())));
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
        let (spec, _, _) =
            read("hysteria2, h.test, 443, password=pw, salamander-password=0bfs, port-hopping=443");
        assert_eq!(spec.obfs, Some(Hysteria2Obfs::Salamander("0bfs".into())));
        assert_eq!(
            spec.quic.port_hopping.map(|hopping| hopping.interval),
            Some(DEFAULT_HOP_INTERVAL)
        );
    }

    #[test]
    fn the_password_is_required_named_and_never_quoted() {
        let (_, failed, diags) = read("hysteria2, h.test, 443");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: `password` is required"
            )]
        );
        let (_, failed, diags) = read("hysteria2, h.test, 443, hunter2, password=");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `password` is required"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `P`: unexpected positional value #1 ignored"
                ),
            ]
        );
        assert!(diags.iter().all(|d| !d.message.contains("hunter2")));
    }

    #[test]
    fn download_bandwidth_is_a_whole_number_of_mbps() {
        for bad in ["-1", "1.5", "100Mbps", "", "4294967296"] {
            let (_, failed, diags) = read(&format!(
                "hysteria2, h.test, 443, password=pw, download-bandwidth=\"{bad}\""
            ));
            assert!(failed, "{bad}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    format!(
                        "policy `P`: invalid value `{bad}` for `download-bandwidth` (expected a whole number of Mbps)"
                    )
                    .as_str()
                )]
            );
        }
    }

    /// The two obfuscations are mutually exclusive (manual) and neither
    /// password is ever quoted.
    #[test]
    fn salamander_or_gecko_not_both_and_not_empty() {
        let (_, failed, diags) = read(
            "hysteria2, h.test, 443, password=pw, salamander-password=s3cretA, gecko-password=s3cretB",
        );
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: `salamander-password` cannot be combined with `gecko-password`"
            )]
        );
        assert!(
            diags
                .iter()
                .all(|d| !d.message.contains("s3cretA") && !d.message.contains("s3cretB"))
        );
        for key in ["salamander-password", "gecko-password"] {
            let (_, failed, diags) = read(&format!("hysteria2, h.test, 443, password=pw, {key}="));
            assert!(failed, "{key}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    format!("policy `P`: `{key}` is empty").as_str()
                )]
            );
        }
    }

    /// UDP rides on QUIC datagrams whatever the line says (manual).
    #[test]
    fn udp_relay_does_not_apply() {
        let (_, failed, diags) = read("hysteria2, h.test, 443, password=pw, udp-relay=true");
        assert!(!failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `P`: `udp-relay` does not apply to `hysteria2` policies (UDP is always relayed); ignored"
            )]
        );
    }

    #[test]
    fn the_spec_does_not_print_its_passwords() {
        for obfs in ["salamander-password", "gecko-password"] {
            let (spec, _, _) = read(&format!(
                "hysteria2, h.test, 443, password=hunter2, {obfs}=s3cret0bfs"
            ));
            let printed = format!("{spec:?}");
            assert!(printed.contains("Secret(***)"), "{printed}");
            assert!(
                !printed.contains("hunter2") && !printed.contains("s3cret0bfs"),
                "{printed}"
            );
        }
    }
}
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub mod obfs;
```

换成

```rust
pub mod hysteria2;
pub mod obfs;
pub mod quic;
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub use obfs::{ObfsMode, ObfsOpts};
```

换成

```rust
pub use hysteria2::{Hysteria2Obfs, Hysteria2Spec};
pub use obfs::{ObfsMode, ObfsOpts};
pub use quic::{PortHopping, QuicOpts};
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    TrustTunnel(TrustTunnelSpec),
```

换成

```rust
    TrustTunnel(TrustTunnelSpec),
    Hysteria2(Hysteria2Spec),
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            ProtoSpec::TrustTunnel(tt) => Some(&tt.tls),
```

换成

```rust
            ProtoSpec::TrustTunnel(tt) => Some(&tt.tls),
            ProtoSpec::Hysteria2(hysteria2) => Some(&hysteria2.tls),
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            | ProtoSpec::Snell(_) => None,
```

换成

```rust
            | ProtoSpec::Snell(_) => None,
        }
    }

    /// The QUIC options of the protocol, when it runs over QUIC.
    pub fn quic(&self) -> Option<&QuicOpts> {
        match self {
            ProtoSpec::Hysteria2(hysteria2) => Some(&hysteria2.quic),
            ProtoSpec::Direct
            | ProtoSpec::Reject(_)
            | ProtoSpec::Http(_)
            | ProtoSpec::Socks5(_)
            | ProtoSpec::Trojan(_)
            | ProtoSpec::Vmess(_)
            | ProtoSpec::AnyTls(_)
            | ProtoSpec::Ssh(_)
            | ProtoSpec::WireGuard(_)
            | ProtoSpec::External(_)
            | ProtoSpec::Ss(_)
            | ProtoSpec::Snell(_)
            | ProtoSpec::H2Connect(_)
            | ProtoSpec::TrustTunnel(_) => None,
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            (common, ProtoSpec::TrustTunnel(read.spec))
        }
        PolicyKind::Ssh => {
```

换成

```rust
            (common, ProtoSpec::TrustTunnel(read.spec))
        }
        PolicyKind::Hysteria2 => {
            let mut common = read_common(&mut r, Applies::Proxy, &mut notes);
            let hysteria2 = hysteria2::read_hysteria2(&mut r, &mut common, env.keystore);
            // `ecn` takes effect on QUIC (phase 2 M7 design 3.4); `tfo` does
            // not apply at all (`W0028`): no second word on either
            notes.inert.retain(|name| !matches!(*name, "ecn" | "tfo"));
            (common, ProtoSpec::Hysteria2(hysteria2))
        }
        PolicyKind::Ssh => {
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        check_underlying(&mut r, &mut common, env);
```

换成

```rust
        check_underlying(&mut r, &mut common, env);
        // the manual: `underlying-proxy` cannot be combined with the
        // `port-hopping` of the QUIC-based protocols
        if common.underlying_proxy.is_some()
            && proto.quic().is_some_and(|quic| quic.port_hopping.is_some())
        {
            r.error(
                codes::E_INVALID_POLICY_PARAM,
                "`port-hopping` cannot be combined with `underlying-proxy`".to_string(),
            );
        }
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    let spec = (!failed && not_implemented.is_none()).then(|| PolicySpec {
```

换成

```rust
    // the engine builds `hysteria2` from M7a task 8 on: until then a valid
    // line is checked in full but has no spec
    let built = policy.kind != PolicyKind::Hysteria2;
    let spec = (!failed && not_implemented.is_none() && built).then(|| PolicySpec {
```

`crates/rurge-config/src/redact.rs`——把

```rust
/// needs its own entry: `password` only matches at a token boundary. A
/// group's `policy-path` usually carries a subscription token, and its
```

换成

```rust
/// and Hysteria 2's `salamander-password` / `gecko-password` need entries of
/// their own: `password` only matches at a token boundary. A group's
/// `policy-path` usually carries a subscription token, and its
```

`crates/rurge-config/src/redact.rs`——把

```rust
const SECRET_PARAMS: [&str; 19] = [
```

换成

```rust
const SECRET_PARAMS: [&str; 21] = [
```

`crates/rurge-config/src/redact.rs`——把

```rust
    "shadow-tls-password",
```

换成

```rust
    "shadow-tls-password",
    "salamander-password",
    "gecko-password",
```

占位分支（Task 8 去掉）：

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            )?),
            // nothing starts here: the program starts on the first dial
```

换成

```rust
            )?),
            // the loader makes no spec of these lines before M7a task 8
            ProtoSpec::Hysteria2(_) => {
                return Err(BuildError::new(format!(
                    "policy `{}`: `{}` is not implemented yet",
                    spec.name,
                    spec.kind.keyword()
                )));
            }
            // nothing starts here: the program starts on the first dial
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-config` → 通过（新增 18 条：`spec::quic` 6 条、`spec::hysteria2` 7 条、`spec::tests` 3 条、脱敏 1 条、`policy_spec` 1 条）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-config crates/rurge-engine/src/outbounds.rs
git commit -m "feat(config): QUIC 类的 port-hopping 与 hysteria2 的配置；两条 E0018 真正报出"
```

### Task 2: `rurge-net::quic`——quinn、UDP 层与拨号

quinn 依赖；`AsyncUdpSocket` 的几层（直连、`tos ≠ 0`、`ecn=off`、链式载体、端口跳跃，P6–P9）与拨号（P10）；`Connector::as_direct()` 与 `DirectConnector::bind_udp`（DIRECT 的 UDP 载体与 QUIC 直连共用一个开 socket 的入口）。新模块的用例都在新文件里，与实现一同写出，没有单独的失败步骤。

**Files:**
- Create: `crates/rurge-net/src/quic/{mod.rs, socket.rs, hop.rs, carrier.rs, dial.rs, testing.rs}`（自带用例；`testing.rs` 在 `testing` feature 下）
- Modify: `Cargo.toml`（工作区依赖 `quinn`）、`crates/rurge-net/Cargo.toml`、`src/lib.rs`、`src/connector.rs`（与用例）

**Interfaces:**
- Consumes: 既有的 `Connector`、`DirectConnector`、`SocketOpts` / `SocketHook`、`Datagram`、`ConnectOpts`、`Target`；272f624 的 `Connector::route()`、`open_socket` / `is_bound`。
- Produces: `Connector::as_direct(&self) -> Option<&DirectConnector>`；`DirectConnector::bind_udp`（`pub(crate)`）；`rurge_net::quic::Path { socket, remote }` 与 `Path::open(connector: &dyn Connector, server: &Target, hopping: Option<&PortHopping>, ecn: Tristate, opts: &ConnectOpts) -> io::Result<Path>`；`Transport`（`Transport::STANDARD`）、`client_config(tls: Arc<rustls::ClientConfig>, transport: &Transport) -> io::Result<quinn::ClientConfig>`、`connect(path: Path, config: &ClientConfig, server_name: &str) -> io::Result<quinn::Connection>`、`connection_error(&ConnectionError) -> io::Error`；测试设施 `rurge_net::quic::testing::{TestCert, endpoint, loopback_socket, EchoServer, MultiPort}`

- [ ] **Step 1: 依赖**

`Cargo.toml`——把

```toml
smoltcp = { version = "0.12.0", default-features = false, features = ["std", "medium-ip", "proto-ipv4", "proto-ipv6", "proto-ipv4-fragmentation", "proto-ipv6-fragmentation", "socket-tcp", "socket-tcp-reno", "socket-udp", "async", "reassembly-buffer-size-16384", "reassembly-buffer-count-4"] }
```

换成

```toml
smoltcp = { version = "0.12.0", default-features = false, features = ["std", "medium-ip", "proto-ipv4", "proto-ipv6", "proto-ipv4-fragmentation", "proto-ipv6-fragmentation", "socket-tcp", "socket-tcp-reno", "socket-udp", "async", "reassembly-buffer-size-16384", "reassembly-buffer-count-4"] }
# QUIC (phase 2 M7): tokio and rustls on ring only; no aws-lc-rs, no platform
# verifier, no bloom filter (a server's), no `log` bridge
quinn = { version = "0.11.12", default-features = false, features = ["runtime-tokio", "rustls-ring"] }
```

`crates/rurge-net/Cargo.toml`——把

```toml
socket2.workspace = true
```

换成

```toml
socket2.workspace = true
quinn.workspace = true
getrandom.workspace = true
```

`crates/rurge-net/src/lib.rs`——把

```rust
pub mod packet_datagram;
```

换成

```rust
pub mod packet_datagram;
pub mod quic;
```

- [ ] **Step 2: 连接器**

`as_direct` 与 `bind_udp`（DIRECT 的 UDP 载体改走它，行为不变）：

`crates/rurge-net/src/connector.rs`——把

```rust
        None
    }
}
```

换成

```rust
        None
    }

    /// This connector, when it dials directly: a QUIC connection then sends
    /// from UDP sockets of its own, made the way this connector makes them
    /// (phase 2 M7 design 3.3); through any other connector it goes over
    /// `connect_udp`'s carrier.
    fn as_direct(&self) -> Option<&DirectConnector> {
        None
    }
}
```

`crates/rurge-net/src/connector.rs`——把

```rust
/// first of those addresses, nothing to race.
```

换成

```rust
/// first of those addresses, nothing to race.
#[derive(Clone)]
```

`crates/rurge-net/src/connector.rs`——把

```rust
                Ok(planned)
            }
        }
    }
}

/// `attempt`, with `timeout` covering name resolution and everything after.
```

换成

```rust
                Ok(planned)
            }
        }
    }

    /// Where a UDP flow to `target` goes: nothing answers a UDP
    /// "connection", so the first address `plan` gives.
    pub(crate) async fn udp_peer(&self, target: &Target) -> io::Result<SocketAddr> {
        let (primary, _) = self.plan(target).await?;
        Ok(SocketAddr::new(primary[0], target.port))
    }

    /// An unconnected UDP socket of `family` with the policy's socket
    /// options and room for a burst, bound to the unspecified address —
    /// unless binding to the policy's interface bound it already (Windows
    /// binds to an interface by binding to its address). DIRECT's UDP
    /// carrier and a direct QUIC connection send from these.
    pub(crate) fn bind_udp(&self, family: Family) -> io::Result<std::net::UdpSocket> {
        let unspecified = match family {
            Family::V4 => IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            Family::V6 => IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
        };
        let socket = open_socket(
            SocketAddr::new(unspecified, 0),
            socket2::Type::DGRAM,
            &self.opts,
            self.hook.as_ref(),
            &self.fallback_logged,
        )?;
        make_room(&socket);
        // `interface` has bound it already on Windows (to the interface's
        // address): a second bind fails
        if !is_bound(&socket) {
            socket.bind(&SocketAddr::new(unspecified, 0).into())?;
        }
        Ok(socket.into())
    }

    /// The policy's `tos`; `0` leaves the kernel default.
    pub(crate) fn tos(&self) -> u8 {
        self.opts.tos
    }
}

/// `attempt`, with `timeout` covering name resolution and everything after.
```

`crates/rurge-net/src/connector.rs`——把

```rust
            // nothing answers a UDP "connection": the first address it is
            let (primary, _) = self.plan(target).await?;
            let addr = SocketAddr::new(primary[0], target.port);
```

换成

```rust
            let addr = self.udp_peer(target).await?;
```

`crates/rurge-net/src/connector.rs`——把

```rust
        ))
    }
}
```

换成

```rust
        ))
    }

    fn as_direct(&self) -> Option<&DirectConnector> {
        Some(self)
    }
}
```

`crates/rurge-net/src/connector.rs`——把

```rust
    /// `ip-version` allows, each bound to the unspecified address and
    /// carrying the policy's socket options. A family this machine cannot
    /// open a socket of is left out, unless it is the only one.
```

换成

```rust
    /// `ip-version` allows (`bind_udp`). A family this machine cannot open
    /// a socket of is left out, unless it is the only one.
```

`crates/rurge-net/src/connector.rs`——把

```rust
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
                make_room(&socket);
                // `interface` has bound it already on Windows (to the
                // interface's address): a second bind fails
                if !is_bound(&socket) {
                    socket.bind(&SocketAddr::new(unspecified, 0).into())?;
                }
                UdpSocket::from_std(socket.into())
            });
```

换成

```rust
            let opened = self.bind_udp(*family).and_then(UdpSocket::from_std);
```

`crates/rurge-net/src/connector.rs`——把

```rust
            ["tos 0x10 V4", "bind test0 V4", "tos 0x88 V4", "tos 0x10 V4"]
        );
    }

    /// Whatever the system allows of `UDP_BUFFER`: more than it gives a
```

换成

```rust
            ["tos 0x10 V4", "bind test0 V4", "tos 0x88 V4", "tos 0x10 V4"]
        );
    }

    /// The UDP sockets of DIRECT's carrier and of a direct QUIC connection
    /// have the policy's options and are bound to the unspecified address —
    /// when binding to the interface did not bind them already, as it does
    /// on Windows (`a_carrier_keeps_the_binding_of_its_interface`).
    #[test]
    fn a_udp_socket_has_the_policys_options() {
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
        let addr = connector
            .bind_udp(Family::V4)
            .unwrap()
            .local_addr()
            .unwrap();
        assert!(addr.ip().is_unspecified() && addr.port() != 0, "{addr}");
        assert_eq!(
            *hook.calls.lock().unwrap(),
            ["tos 0x10 V4", "bind test0 V4"]
        );
    }

    /// Whatever the system allows of `UDP_BUFFER`: more than it gives a
```

- [ ] **Step 3: UDP 层与拨号**

新建 `crates/rurge-net/src/quic/mod.rs`：

```rust
//! QUIC for what runs over it (phase 2 M7 design 3): the UDP layers under
//! a connection, all quinn `AsyncUdpSocket`s, and the dial.
//!
//! - Direct: a socket of the policy's own (its interface, `ip-version`,
//!   buffers) that quinn-udp drives — segmentation offload, per-packet ECN.
//!   With `tos`, one that sends plainly instead, which keeps the DSCP.
//! - `underlying-proxy`: the chain's UDP flow to the server, named as the
//!   line names it for the underlying proxy to resolve.
//! - `port-hopping`: a port of the set at a time, a random other one and a
//!   new local socket every interval.
//! - `ecn=off`: no ECN marks either way.
//!
//! Every connection has an endpoint of its own on such a socket (M7-D9):
//! its hops, and the obfuscation a protocol may layer on it, belong to one
//! connection.

mod carrier;
mod dial;
mod hop;
mod socket;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use dial::{Transport, client_config, connect, connection_error};

use crate::connector::{ConnectOpts, Connector, Target};
use crate::socket::Family;
use quinn::AsyncUdpSocket;
use rurge_config::spec::{PortHopping, Tristate};
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

/// What a QUIC connection's packets go through to its server, and the
/// address quinn knows the server by — its own, or one that stands for it.
#[derive(Debug)]
pub struct Path {
    pub socket: Arc<dyn AsyncUdpSocket>,
    pub remote: SocketAddr,
}

impl Path {
    /// The path to `server` through `connector`. A direct connector
    /// resolves the server the policy's way (`[Host]`, `dns-follow-
    /// interface`, `ip-version`), once — `port-hopping` keeps the address
    /// and replaces the line's port with its own. Any other connector
    /// carries the packets over its UDP flow to `server` as named, which
    /// cannot hop. `ecn=off` strips ECN marks; `auto` and `on` are quinn's
    /// own (design 3.4). `opts` bounds the carrier's setup only: the caller
    /// bounds the whole dial.
    pub async fn open(
        connector: &dyn Connector,
        server: &Target,
        hopping: Option<&PortHopping>,
        ecn: Tristate,
        opts: &ConnectOpts,
    ) -> io::Result<Path> {
        let path = match (connector.as_direct(), hopping) {
            (Some(direct), None) => {
                let remote = direct.udp_peer(server).await?;
                Path {
                    socket: socket::direct_socket(direct, Family::of(&remote.ip()))?,
                    remote,
                }
            }
            (Some(direct), Some(hopping)) => {
                let server = direct.udp_peer(server).await?.ip();
                let direct = direct.clone();
                let open = move || socket::direct_socket(&direct, Family::of(&server));
                let hopping =
                    hop::Hopping::start(server, &hopping.ports, hopping.interval, Box::new(open))?;
                Path {
                    remote: hopping.remote(),
                    socket: Arc::new(hopping),
                }
            }
            (None, None) => Path {
                socket: Arc::new(carrier::Carried(connector.connect_udp(server, opts).await?)),
                remote: carrier::STAND_IN,
            },
            (None, Some(_)) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "quic: `port-hopping` cannot be combined with `underlying-proxy`",
                ));
            }
        };
        Ok(match ecn {
            Tristate::Off => Path {
                socket: Arc::new(socket::NoEcn(path.socket)),
                remote: path.remote,
            },
            Tristate::Auto | Tristate::On => path,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{EchoServer, MultiPort, TestCert, endpoint, loopback_socket};
    use super::*;
    use crate::BoxFuture;
    use crate::connector::{BoxedDatagram, BoxedStream, Datagram, DirectConnector, Resolve};
    use crate::packet_datagram::packet_datagram;
    use crate::socket::{SocketHook, SocketOpts};
    use bytes::Bytes;
    use quinn::udp::{RecvMeta, Transmit};
    use quinn::{Connection, ConnectionError, Runtime, TokioRuntime, UdpPoller};
    use rurge_config::HostName;
    use std::io::IoSliceMut;
    use std::net::IpAddr;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::{Context, Poll, Waker, ready};
    use std::time::Duration;
    use tokio::io::ReadBuf;
    use tokio::time::{Instant, timeout};

    const ALPN: &[&str] = &["h3"];
    const BOUND: Duration = Duration::from_secs(10);

    /// Answers every name with the loopback.
    struct Loopback;

    impl Resolve for Loopback {
        fn resolve<'a>(&'a self, _host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            Box::pin(std::future::ready(Ok(vec![IpAddr::from([127, 0, 0, 1])])))
        }
    }

    fn direct() -> DirectConnector {
        DirectConnector::new(Arc::new(Loopback))
    }

    fn at(host: &str, port: u16) -> Target {
        Target::new(HostName::parse(host), port)
    }

    async fn open(
        connector: &dyn Connector,
        server: &Target,
        hopping: Option<&PortHopping>,
        ecn: Tristate,
    ) -> Path {
        Path::open(connector, server, hopping, ecn, &ConnectOpts::default())
            .await
            .expect("a path")
    }

    async fn dial(path: Path, cert: &TestCert, transport: &Transport) -> io::Result<Connection> {
        let config = client_config(cert.client_tls(ALPN), transport)?;
        timeout(BOUND, connect(path, &config, "localhost"))
            .await
            .expect("an outcome within the bound")
    }

    /// A stream and a datagram there and back.
    async fn echoes(connection: &Connection) {
        timeout(BOUND, async {
            let (mut send, mut recv) = connection.open_bi().await.unwrap();
            send.write_all(b"ping").await.unwrap();
            send.finish().unwrap();
            assert_eq!(recv.read_to_end(64).await.unwrap(), b"ping");
            // a datagram may be lost: sent again until one comes back
            loop {
                connection
                    .send_datagram(Bytes::from_static(b"datagram"))
                    .unwrap();
                let wait = timeout(Duration::from_millis(200), connection.read_datagram());
                if let Ok(back) = wait.await {
                    assert_eq!(&back.unwrap()[..], b"datagram");
                    return;
                }
            }
        })
        .await
        .expect("the echoes within the bound");
    }

    #[tokio::test]
    async fn a_direct_path_carries_streams_and_datagrams() {
        let cert = TestCert::new();
        let server = EchoServer::spawn(cert.server_config(ALPN));
        let port = server.addr().port();
        // by address, and by a name the policy's own resolver answers
        for host in ["127.0.0.1", "quic.test"] {
            let path = open(&direct(), &at(host, port), None, Tristate::Auto).await;
            assert_eq!(path.remote, server.addr());
            let connection = dial(path, &cert, &Transport::STANDARD).await.unwrap();
            echoes(&connection).await;
        }
    }

    /// Notes the `tos` the sockets get.
    #[derive(Default)]
    struct TosHook(Mutex<Vec<u8>>);

    impl SocketHook for TosHook {
        fn bind_interface(&self, _: &socket2::Socket, _: &str, _: Family) -> io::Result<()> {
            Ok(())
        }

        fn set_tos(&self, _: &socket2::Socket, _: Family, tos: u8) -> io::Result<()> {
            self.0.lock().unwrap().push(tos);
            Ok(())
        }
    }

    /// With `tos` the socket keeps it and sends one datagram at a time, and
    /// still finds the path MTU as quinn's own sockets do.
    #[tokio::test]
    async fn a_path_with_tos_carries_everything() {
        let cert = TestCert::new();
        let server = EchoServer::spawn(cert.server_config(ALPN));
        let hook = Arc::new(TosHook::default());
        let connector = DirectConnector::with_opts(
            Arc::new(Loopback),
            SocketOpts {
                tos: 0x88,
                ..SocketOpts::default()
            },
            hook.clone(),
        );
        let path = open(
            &connector,
            &at("127.0.0.1", server.addr().port()),
            None,
            Tristate::Auto,
        )
        .await;
        assert_eq!(path.socket.max_transmit_segments(), 1);
        assert_eq!(path.socket.may_fragment(), loopback_socket().may_fragment());
        let connection = dial(path, &cert, &Transport::STANDARD).await.unwrap();
        echoes(&connection).await;
        assert_eq!(*hook.0.lock().unwrap(), [0x88]);
    }

    /// Binds the socket to the interface's address, as the Windows hook
    /// does (`rurge-platform::socket::bind_interface`).
    struct BindingHook;

    impl SocketHook for BindingHook {
        fn bind_interface(
            &self,
            socket: &socket2::Socket,
            _: &str,
            family: Family,
        ) -> io::Result<()> {
            let source = match family {
                Family::V4 => IpAddr::from([127, 0, 0, 1]),
                Family::V6 => IpAddr::from(std::net::Ipv6Addr::LOCALHOST),
            };
            socket.bind(&SocketAddr::new(source, 0).into())
        }

        fn set_tos(&self, _: &socket2::Socket, _: Family, _: u8) -> io::Result<()> {
            Ok(())
        }
    }

    /// `interface` on Windows binds the socket to the interface's address:
    /// the connection sends from that binding, which is not made a second
    /// time — and an IPv6 socket was made IPv6-only before it, which a
    /// bound socket takes no more. IPv6 where this machine has a loopback
    /// of it.
    #[tokio::test]
    async fn a_direct_path_keeps_the_binding_of_its_interface() {
        let cert = TestCert::new();
        let connector = DirectConnector::with_opts(
            Arc::new(Loopback),
            SocketOpts {
                interface: Some("lo".into()),
                ..SocketOpts::default()
            },
            Arc::new(BindingHook),
        );
        let mut servers = vec![std::net::UdpSocket::bind("127.0.0.1:0").unwrap()];
        servers.extend(std::net::UdpSocket::bind("[::1]:0").ok());
        for socket in servers {
            let addr = socket.local_addr().unwrap();
            let socket = TokioRuntime.wrap_udp_socket(socket).unwrap();
            let _server = EchoServer::spawn_on(cert.server_config(ALPN), socket);
            let server = Target::new(HostName::Ip(addr.ip()), addr.port());
            let path = open(&connector, &server, None, Tristate::Auto).await;
            let local = path.socket.local_addr().unwrap();
            assert!(local.ip() == addr.ip() && local.port() != 0, "{local}");
            let connection = dial(path, &cert, &Transport::STANDARD).await.unwrap();
            echoes(&connection).await;
        }
    }

    #[tokio::test]
    async fn a_path_without_ecn_carries_everything() {
        let cert = TestCert::new();
        let server = EchoServer::spawn(cert.server_config(ALPN));
        let path = open(
            &direct(),
            &at("127.0.0.1", server.addr().port()),
            None,
            Tristate::Off,
        )
        .await;
        let connection = dial(path, &cert, &Transport::STANDARD).await.unwrap();
        echoes(&connection).await;
    }

    /// Carries a direct connector's UDP the way `ChainConnector` carries
    /// the underlying policy's: the carrier is asked for the server as
    /// named and resolves it itself. A carrier can be cut.
    #[derive(Default)]
    struct Chained {
        asked: Mutex<Vec<Target>>,
        cut: Arc<Cut>,
    }

    #[derive(Default)]
    struct Cut {
        done: AtomicBool,
        reader: Mutex<Option<Waker>>,
    }

    impl Cut {
        fn now(&self) {
            self.done.store(true, Ordering::SeqCst);
            if let Some(reader) = self.reader.lock().unwrap().take() {
                reader.wake();
            }
        }
    }

    struct CutDatagram(BoxedDatagram, Arc<Cut>);

    impl Datagram for CutDatagram {
        fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
            self.0.poll_send(cx, buf)
        }

        fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            *self.1.reader.lock().unwrap() = Some(cx.waker().clone());
            if self.1.done.load(Ordering::SeqCst) {
                return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "cut")));
            }
            self.0.poll_recv(cx, buf)
        }
    }

    impl Connector for Chained {
        fn connect<'a>(
            &'a self,
            _target: &'a Target,
            _opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedStream>> {
            Box::pin(std::future::ready(Err(io::Error::other("UDP only"))))
        }

        fn connect_udp<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
            self.asked.lock().unwrap().push(target.clone());
            Box::pin(async move {
                let socket = direct().open_udp(opts).await?;
                let to = socket.resolve(target).await?;
                let datagram = packet_datagram(socket, to);
                Ok(Box::new(CutDatagram(datagram, self.cut.clone())) as BoxedDatagram)
            })
        }
    }

    /// Through `underlying-proxy`'s carrier: the server goes to it as
    /// named, quinn knows a stand-in, and no path MTU is probed.
    #[tokio::test]
    async fn a_carried_path_reaches_the_server_by_name() {
        let cert = TestCert::new();
        let server = EchoServer::spawn(cert.server_config(ALPN));
        let chain = Chained::default();
        let named = at("quic.test", server.addr().port());
        let path = open(&chain, &named, None, Tristate::Auto).await;
        assert_eq!(*chain.asked.lock().unwrap(), [named]);
        assert_eq!(path.remote, carrier::STAND_IN);
        assert!(path.socket.may_fragment());
        let connection = dial(path, &cert, &Transport::STANDARD).await.unwrap();
        echoes(&connection).await;
        assert_eq!(connection.stats().path.sent_plpmtud_probes, 0);
    }

    /// A carrier that ends takes the connection with it at once, not after
    /// the idle timeout. quinn reports the endpoint it lost, and then — as
    /// the connection's driver goes — that it was closed.
    #[tokio::test]
    async fn a_carrier_that_ends_ends_the_connection() {
        let cert = TestCert::new();
        let server = EchoServer::spawn(cert.server_config(ALPN));
        let chain = Chained::default();
        let path = open(
            &chain,
            &at("quic.test", server.addr().port()),
            None,
            Tristate::Auto,
        )
        .await;
        let connection = dial(path, &cert, &Transport::STANDARD).await.unwrap();
        chain.cut.now();
        let closed = timeout(Duration::from_secs(5), connection.closed())
            .await
            .expect("closed at once");
        assert!(
            matches!(
                closed,
                ConnectionError::LocallyClosed | ConnectionError::TransportError(_)
            ),
            "{closed:?}"
        );
    }

    #[tokio::test]
    async fn hopping_cannot_go_through_a_carrier() {
        let hopping = PortHopping {
            ports: vec![5000..=6000],
            interval: Duration::from_secs(30),
        };
        let err = Path::open(
            &Chained::default(),
            &at("quic.test", 443),
            Some(&hopping),
            Tristate::Auto,
            &ConnectOpts::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            "quic: `port-hopping` cannot be combined with `underlying-proxy`"
        );
    }

    /// The server listens on three ports; the client hops among them every
    /// 200 ms (the loader allows no less than 5 s), from a new local
    /// socket each time, and the connection carries on.
    #[tokio::test]
    async fn a_hopping_path_moves_among_the_ports() {
        let cert = TestCert::new();
        let ports = MultiPort::bind(3);
        let _server = EchoServer::spawn_on(cert.server_config(ALPN), ports.clone());
        let hopping = PortHopping {
            ports: ports.ports().iter().map(|&p| p..=p).collect(),
            interval: Duration::from_millis(200),
        };
        // the line's own port is not used
        let path = open(
            &direct(),
            &at("127.0.0.1", 9),
            Some(&hopping),
            Tristate::Auto,
        )
        .await;
        assert!(ports.ports().contains(&path.remote.port()));
        let connection = dial(path, &cert, &Transport::STANDARD).await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while ports.heard().iter().filter(|&&n| n > 0).count() < 2 {
            assert!(
                Instant::now() < deadline,
                "heard only on {:?}",
                ports.heard()
            );
            echoes(&connection).await;
        }
        assert!(ports.clients() >= 2, "a new local socket per hop");
        echoes(&connection).await;
    }

    /// Each hop's socket is made as the first one: with the policy's
    /// `tos`, under `ecn=off`.
    #[tokio::test]
    async fn every_hop_has_the_policys_socket() {
        let cert = TestCert::new();
        let ports = MultiPort::bind(2);
        let _server = EchoServer::spawn_on(cert.server_config(ALPN), ports.clone());
        let hook = Arc::new(TosHook::default());
        let connector = DirectConnector::with_opts(
            Arc::new(Loopback),
            SocketOpts {
                tos: 0x88,
                ..SocketOpts::default()
            },
            hook.clone(),
        );
        let hopping = PortHopping {
            ports: ports.ports().iter().map(|&p| p..=p).collect(),
            interval: Duration::from_millis(100),
        };
        let path = open(
            &connector,
            &at("127.0.0.1", 9),
            Some(&hopping),
            Tristate::Off,
        )
        .await;
        let connection = dial(path, &cert, &Transport::STANDARD).await.unwrap();
        // the first socket and two hops'
        let deadline = Instant::now() + BOUND;
        while hook.0.lock().unwrap().len() < 3 {
            assert!(Instant::now() < deadline, "no hops");
            echoes(&connection).await;
        }
        assert!(hook.0.lock().unwrap().iter().all(|&tos| tos == 0x88));
        echoes(&connection).await;
    }

    /// A server socket that counts the datagrams it heard with an ECN mark.
    #[derive(Debug)]
    struct Marks {
        inner: Arc<dyn AsyncUdpSocket>,
        marked: AtomicUsize,
    }

    impl AsyncUdpSocket for Marks {
        fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
            self.inner.clone().create_io_poller()
        }

        fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
            self.inner.try_send(transmit)
        }

        fn poll_recv(
            &self,
            cx: &mut Context<'_>,
            bufs: &mut [IoSliceMut<'_>],
            meta: &mut [RecvMeta],
        ) -> Poll<io::Result<usize>> {
            let n = ready!(self.inner.poll_recv(cx, bufs, meta))?;
            let marked = meta[..n].iter().filter(|meta| meta.ecn.is_some()).count();
            self.marked.fetch_add(marked, Ordering::SeqCst);
            Poll::Ready(Ok(n))
        }

        fn local_addr(&self) -> io::Result<SocketAddr> {
            self.inner.local_addr()
        }

        fn max_transmit_segments(&self) -> usize {
            self.inner.max_transmit_segments()
        }

        fn max_receive_segments(&self) -> usize {
            self.inner.max_receive_segments()
        }

        fn may_fragment(&self) -> bool {
            self.inner.may_fragment()
        }
    }

    /// How many ECN-marked datagrams the server hears of a short exchange
    /// over a path with `tos` and `ecn`.
    async fn marks_heard(tos: u8, ecn: Tristate) -> usize {
        let cert = TestCert::new();
        let marks = Arc::new(Marks {
            inner: loopback_socket(),
            marked: AtomicUsize::new(0),
        });
        let server = EchoServer::spawn_on(cert.server_config(ALPN), marks.clone());
        let connector = DirectConnector::with_opts(
            Arc::new(Loopback),
            SocketOpts {
                tos,
                ..SocketOpts::default()
            },
            Arc::new(TosHook::default()),
        );
        let path = open(
            &connector,
            &at("127.0.0.1", server.addr().port()),
            None,
            ecn,
        )
        .await;
        let connection = dial(path, &cert, &Transport::STANDARD).await.unwrap();
        echoes(&connection).await;
        marks.marked.load(Ordering::SeqCst)
    }

    /// quinn marks what it sends; `ecn=off` takes the marks off, and with
    /// `tos` packets go out plainly, unmarked. A system that shows no marks
    /// on the loopback at all leaves nothing to tell apart.
    #[tokio::test]
    async fn ecn_marks_go_out_unless_off_or_with_tos() {
        if marks_heard(0, Tristate::Auto).await == 0 {
            return;
        }
        assert_eq!(marks_heard(0, Tristate::Off).await, 0);
        assert_eq!(marks_heard(0x88, Tristate::Auto).await, 0);
    }

    #[test]
    fn the_standard_transport_is_the_designs() {
        assert_eq!(
            Transport::STANDARD,
            Transport {
                idle_timeout: Duration::from_secs(30),
                keep_alive: Duration::from_secs(10),
                stream_window: 8 * 1024 * 1024,
                connection_window: 20 * 1024 * 1024,
            }
        );
    }

    /// A raw server endpoint and the client's connection to it, with the
    /// server's side of it.
    async fn pair(transport: &Transport) -> (quinn::Endpoint, Connection, Connection) {
        let cert = TestCert::new();
        let server = endpoint(cert.server_config(ALPN), loopback_socket());
        let addr = server.local_addr().unwrap();
        let path = open(
            &direct(),
            &at("127.0.0.1", addr.port()),
            None,
            Tristate::Auto,
        )
        .await;
        let (client, accepted) = tokio::join!(dial(path, &cert, transport), async {
            let incoming = server.accept().await.expect("a connection");
            incoming.await.expect("accepted")
        });
        (server, client.unwrap(), accepted)
    }

    /// The server may open unidirectional streams (HTTP/3 needs three), not
    /// bidirectional ones.
    #[tokio::test]
    async fn the_server_opens_no_bidirectional_stream() {
        let (_server, client, server_side) = pair(&Transport::STANDARD).await;
        let mut uni = server_side.open_uni().await.unwrap();
        uni.write_all(b"settings").await.unwrap();
        uni.finish().unwrap();
        let mut accepted = timeout(BOUND, client.accept_uni()).await.unwrap().unwrap();
        assert_eq!(accepted.read_to_end(64).await.unwrap(), b"settings");
        assert!(
            timeout(Duration::from_millis(300), server_side.open_bi())
                .await
                .is_err(),
            "a bidirectional stream from the server"
        );
    }

    #[tokio::test]
    async fn keep_alive_holds_an_idle_connection() {
        let transport = Transport {
            idle_timeout: Duration::from_secs(1),
            keep_alive: Duration::from_millis(250),
            ..Transport::STANDARD
        };
        let (_server, client, _server_side) = pair(&transport).await;
        // more than two idle timeouts with nothing sent but keep-alives
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert!(client.close_reason().is_none());
    }

    #[tokio::test]
    async fn an_idle_connection_times_out() {
        let transport = Transport {
            idle_timeout: Duration::from_millis(300),
            // past the idle timeout: no keep-alive in time
            keep_alive: Duration::from_secs(10),
            ..Transport::STANDARD
        };
        let (_server, client, _server_side) = pair(&transport).await;
        let closed = timeout(BOUND, client.closed()).await.unwrap();
        let err = connection_error(&closed);
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(
            err.to_string(),
            "quic: the server stopped answering (idle timeout)"
        );
    }

    #[tokio::test]
    async fn a_server_that_never_answers_times_the_handshake_out() {
        // a socket that takes the packets and never answers
        let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = silent.local_addr().unwrap().port();
        let transport = Transport {
            idle_timeout: Duration::from_millis(300),
            ..Transport::STANDARD
        };
        let path = open(&direct(), &at("127.0.0.1", port), None, Tristate::Auto).await;
        let err = dial(path, &TestCert::new(), &transport).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(err.to_string(), "quic: the handshake timed out");
    }

    #[tokio::test]
    async fn an_untrusted_certificate_is_a_tls_error() {
        let server = EchoServer::spawn(TestCert::new().server_config(ALPN));
        let path = open(
            &direct(),
            &at("127.0.0.1", server.addr().port()),
            None,
            Tristate::Auto,
        )
        .await;
        // trusts another certificate, of the same subject
        let err = dial(path, &TestCert::new(), &Transport::STANDARD)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string()
                .starts_with("tls: invalid peer certificate: "),
            "{err}"
        );
    }

    /// QUIC requires ALPN: a server that has none of the client's, or none
    /// at all, refuses the handshake.
    #[tokio::test]
    async fn the_handshake_needs_a_common_alpn() {
        for server_alpn in [&["x"][..], &[]] {
            let cert = TestCert::new();
            let server = EchoServer::spawn(cert.server_config(server_alpn));
            let path = open(
                &direct(),
                &at("127.0.0.1", server.addr().port()),
                None,
                Tristate::Auto,
            )
            .await;
            let err = dial(path, &cert, &Transport::STANDARD).await.unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData);
            assert_eq!(
                err.to_string(),
                "tls: the server refused the handshake (NoApplicationProtocol)"
            );
        }
    }

    /// The server's close and its code; what it says is cleaned.
    #[tokio::test]
    async fn a_close_by_the_server_carries_its_code() {
        let (_server, client, server_side) = pair(&Transport::STANDARD).await;
        server_side.close(0x101u32.into(), b"going\naway");
        let closed = timeout(BOUND, client.closed()).await.unwrap();
        let err = connection_error(&closed);
        assert_eq!(err.kind(), io::ErrorKind::ConnectionAborted);
        assert_eq!(
            err.to_string(),
            "quic: the server closed the connection (code 0x101: goingaway)"
        );
    }

    #[tokio::test]
    async fn a_name_tls_cannot_use_is_refused_before_any_packet() {
        let path = open(&direct(), &at("127.0.0.1", 443), None, Tristate::Auto).await;
        let config = client_config(TestCert::new().client_tls(ALPN), &Transport::STANDARD).unwrap();
        let err = connect(path, &config, "no such name!").await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(err.to_string(), "quic: invalid server name: no such name!");
    }
}
```

新建 `crates/rurge-net/src/quic/socket.rs`：

```rust
//! The sockets under a direct QUIC connection and the `ecn=off` layer
//! (phase 2 M7 design 3.3, 3.4).

use crate::connector::DirectConnector;
use crate::socket::Family;
use quinn::udp::{RecvMeta, Transmit, UdpSocketState};
use quinn::{AsyncUdpSocket, Runtime, TokioRuntime, UdpPoller};
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use tokio::io::Interest;

/// A socket of `family` made as DIRECT's UDP carrier makes its own
/// (`DirectConnector::bind_udp`: the policy's socket options, the binding
/// to its interface kept), ready for quinn: quinn-udp's own — segmentation
/// offload, per-packet ECN — unless the policy sets `tos`.
pub(crate) fn direct_socket(
    connector: &DirectConnector,
    family: Family,
) -> io::Result<Arc<dyn AsyncUdpSocket>> {
    let socket = connector.bind_udp(family)?;
    if connector.tos() == 0 {
        TokioRuntime.wrap_udp_socket(socket)
    } else {
        Ok(Arc::new(TosSocket::new(socket)?))
    }
}

/// A socket that keeps the policy's `tos`. quinn-udp writes the TOS byte of
/// every packet it sends (on Linux and macOS; only the ECN bits), which
/// clears the DSCP set on the socket: what goes out is sent plainly, without
/// ECN marks and one datagram at a time. What comes in still goes through
/// quinn-udp, which also makes the socket's packets unfragmentable: the
/// server's ECN marks are seen and the path MTU is discovered.
#[derive(Debug)]
struct TosSocket {
    io: tokio::net::UdpSocket,
    state: UdpSocketState,
}

impl TosSocket {
    fn new(socket: std::net::UdpSocket) -> io::Result<TosSocket> {
        let state = UdpSocketState::new((&socket).into())?;
        Ok(TosSocket {
            io: tokio::net::UdpSocket::from_std(socket)?,
            state,
        })
    }
}

impl AsyncUdpSocket for TosSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(Writable(self))
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        match self.io.try_send_to(transmit.contents, transmit.destination) {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Err(e),
            Err(e) => {
                // as quinn-udp's own sockets: a datagram that cannot go is
                // lost, which QUIC recovers from, not a failed connection
                tracing::trace!(error = %e, "a QUIC datagram was not sent");
                Ok(())
            }
            Ok(_) => Ok(()),
        }
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        // as quinn's own sockets: a failed receive (an ICMP error Windows
        // reports, say) leaves the socket readable and is tried again
        loop {
            ready!(self.io.poll_recv_ready(cx))?;
            if let Ok(n) = self.io.try_io(Interest::READABLE, || {
                self.state.recv((&self.io).into(), bufs, meta)
            }) {
                return Poll::Ready(Ok(n));
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.io.local_addr()
    }

    fn max_receive_segments(&self) -> usize {
        self.state.gro_segments()
    }

    fn may_fragment(&self) -> bool {
        self.state.may_fragment()
    }
}

#[derive(Debug)]
struct Writable(Arc<TosSocket>);

impl UdpPoller for Writable {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.0.io.poll_send_ready(cx)
    }
}

/// Never waits: for sockets that drop what they cannot send now, as a
/// full socket buffer would.
#[derive(Debug)]
pub(crate) struct NeverBlocks;

impl UdpPoller for NeverBlocks {
    fn poll_writable(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// `ecn=off` (phase 2 M7 design 3.4): no ECN mark on what goes out, none
/// seen on what comes in. quinn's ECN validation then fails at the first
/// acknowledgement on either side, and both stop marking.
#[derive(Debug)]
pub(crate) struct NoEcn(pub(crate) Arc<dyn AsyncUdpSocket>);

impl AsyncUdpSocket for NoEcn {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        self.0.clone().create_io_poller()
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        self.0.try_send(&Transmit {
            ecn: None,
            ..transmit.clone()
        })
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let n = ready!(self.0.poll_recv(cx, bufs, meta))?;
        for meta in &mut meta[..n] {
            meta.ecn = None;
        }
        Poll::Ready(Ok(n))
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.0.local_addr()
    }

    fn max_transmit_segments(&self) -> usize {
        self.0.max_transmit_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.0.max_receive_segments()
    }

    fn may_fragment(&self) -> bool {
        self.0.may_fragment()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quinn::udp::EcnCodepoint;
    use std::sync::Mutex;

    /// Notes what it is asked to send; hands out one datagram marked CE.
    #[derive(Debug, Default)]
    struct Scripted {
        sent: Mutex<Vec<Option<EcnCodepoint>>>,
    }

    impl AsyncUdpSocket for Scripted {
        fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
            Box::pin(NeverBlocks)
        }

        fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
            self.sent.lock().unwrap().push(transmit.ecn);
            Ok(())
        }

        fn poll_recv(
            &self,
            _cx: &mut Context<'_>,
            bufs: &mut [IoSliceMut<'_>],
            meta: &mut [RecvMeta],
        ) -> Poll<io::Result<usize>> {
            bufs[0][..2].copy_from_slice(b"hi");
            meta[0] = RecvMeta {
                addr: "192.0.2.1:443".parse().unwrap(),
                len: 2,
                stride: 2,
                ecn: Some(EcnCodepoint::Ce),
                dst_ip: None,
            };
            Poll::Ready(Ok(1))
        }

        fn local_addr(&self) -> io::Result<SocketAddr> {
            Ok("0.0.0.0:0".parse().unwrap())
        }
    }

    /// `ecn=off` takes the marks off both ways and leaves everything else.
    #[test]
    fn ecn_off_strips_the_marks_both_ways() {
        let inner = Arc::new(Scripted::default());
        let socket = NoEcn(inner.clone());
        socket
            .try_send(&Transmit {
                destination: "192.0.2.1:443".parse().unwrap(),
                ecn: Some(EcnCodepoint::Ect0),
                contents: b"out",
                segment_size: None,
                src_ip: None,
            })
            .unwrap();
        assert_eq!(*inner.sent.lock().unwrap(), [None]);
        let mut storage = [0u8; 16];
        let mut bufs = [IoSliceMut::new(&mut storage)];
        let mut meta = [RecvMeta::default()];
        let mut cx = Context::from_waker(std::task::Waker::noop());
        let Poll::Ready(Ok(n)) = socket.poll_recv(&mut cx, &mut bufs, &mut meta) else {
            panic!("the scripted datagram is there");
        };
        assert_eq!(n, 1);
        assert_eq!(meta[0].ecn, None);
        assert_eq!((meta[0].len, &storage[..2]), (2, &b"hi"[..]));
    }
}
```

新建 `crates/rurge-net/src/quic/hop.rs`：

```rust
//! Port hopping (phase 2 M7 design 3.3): the server is reached on one port
//! of its set at a time, at first a random one; every interval a random
//! other port, from a new local socket. The previous socket stays until
//! the next hop: what the server sent before it saw the change still
//! arrives. quinn never learns: what it sends goes to the current port
//! whatever the destination, and everything received comes from the
//! address quinn knows the server by (a client drops packets from any
//! other).

use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use std::io::{self, IoSliceMut};
use std::net::{IpAddr, SocketAddr};
use std::ops::RangeInclusive;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::task::JoinHandle;

/// Makes the socket of a hop: the same kind each time.
pub(crate) type Open = Box<dyn Fn() -> io::Result<Arc<dyn AsyncUdpSocket>> + Send + Sync>;

/// The ports a connection hops among: each once, in ascending runs.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PortSet(Vec<RangeInclusive<u16>>);

impl PortSet {
    /// `ranges` as written, overlapping or not.
    fn new(ranges: &[RangeInclusive<u16>]) -> PortSet {
        let mut sorted = ranges.to_vec();
        sorted.sort_by_key(|range| *range.start());
        let mut runs: Vec<RangeInclusive<u16>> = Vec::new();
        for range in sorted {
            match runs.last_mut() {
                Some(last) if u32::from(*range.start()) <= u32::from(*last.end()) + 1 => {
                    if range.end() > last.end() {
                        *last = *last.start()..=*range.end();
                    }
                }
                _ => runs.push(range),
            }
        }
        PortSet(runs)
    }

    fn len(&self) -> u32 {
        self.0
            .iter()
            .map(|run| u32::from(*run.end()) - u32::from(*run.start()) + 1)
            .sum()
    }

    /// The port at `index` in ascending order; `index < len()`.
    fn nth(&self, mut index: u32) -> u16 {
        for run in &self.0 {
            let len = u32::from(*run.end()) - u32::from(*run.start()) + 1;
            if index < len {
                return run.start() + index as u16;
            }
            index -= len;
        }
        unreachable!("an index past the set")
    }

    fn index_of(&self, port: u16) -> Option<u32> {
        let mut before = 0;
        for run in &self.0 {
            if run.contains(&port) {
                return Some(before + u32::from(port - run.start()));
            }
            before += u32::from(*run.end()) - u32::from(*run.start()) + 1;
        }
        None
    }

    /// Any port of the set, all alike likely.
    fn random(&self) -> u16 {
        self.nth(below(self.len()))
    }

    /// Any port of the set but `port`, all alike likely; `port` itself
    /// when it is the only one.
    fn other_than(&self, port: u16) -> u16 {
        match self.index_of(port) {
            Some(at) if self.len() > 1 => {
                let pick = below(self.len() - 1);
                self.nth(if pick >= at { pick + 1 } else { pick })
            }
            _ => self.random(),
        }
    }
}

/// A random number below `n` (`n > 0`).
fn below(n: u32) -> u32 {
    getrandom::u32().unwrap_or(0) % n
}

#[derive(Debug)]
struct State {
    /// The server port in use, and the socket that sends to it.
    port: u16,
    current: Arc<dyn AsyncUdpSocket>,
    /// The socket before the last hop: still heard.
    previous: Option<Arc<dyn AsyncUdpSocket>>,
    /// Counts the hops: a poller sees that its socket was replaced.
    hops: u64,
    /// The endpoint's receive loop, woken by a hop to listen on the new
    /// socket.
    reader: Option<Waker>,
}

#[derive(Debug)]
struct Shared {
    server: IpAddr,
    /// What quinn knows the server by: the first port's address.
    remote: SocketAddr,
    ports: PortSet,
    state: Mutex<State>,
}

impl Shared {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("the hopping state")
    }

    /// Moves to a random other port, sending from `socket` from now on.
    fn hop(&self, socket: Arc<dyn AsyncUdpSocket>) {
        let (_retired, reader) = {
            let mut state = self.state();
            state.port = self.ports.other_than(state.port);
            let previous = std::mem::replace(&mut state.current, socket);
            state.hops += 1;
            (state.previous.replace(previous), state.reader.take())
        };
        if let Some(reader) = reader {
            reader.wake();
        }
    }
}

#[derive(Debug)]
pub(crate) struct Hopping {
    shared: Arc<Shared>,
    /// Hops every interval; ends with the socket.
    timer: JoinHandle<()>,
}

impl Hopping {
    /// Starts on a random port of `ports` at `server`, from a socket `open`
    /// makes, and hops every `interval`. A hop whose socket cannot be made
    /// is skipped, as the reference implementation does.
    pub(crate) fn start(
        server: IpAddr,
        ports: &[RangeInclusive<u16>],
        interval: Duration,
        open: Open,
    ) -> io::Result<Hopping> {
        let ports = PortSet::new(ports);
        if ports.len() == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "quic: no port to hop among",
            ));
        }
        let port = ports.random();
        let shared = Arc::new(Shared {
            server,
            remote: SocketAddr::new(server, port),
            ports,
            state: Mutex::new(State {
                port,
                current: open()?,
                previous: None,
                hops: 0,
                reader: None,
            }),
        });
        let timer = tokio::spawn({
            let shared = shared.clone();
            async move {
                loop {
                    tokio::time::sleep(interval).await;
                    match open() {
                        Ok(socket) => shared.hop(socket),
                        Err(e) => {
                            tracing::debug!(error = %e, "no socket for the next hop; staying on this port");
                        }
                    }
                }
            }
        });
        Ok(Hopping { shared, timer })
    }

    /// What quinn knows the server by.
    pub(crate) fn remote(&self) -> SocketAddr {
        self.shared.remote
    }
}

impl Drop for Hopping {
    fn drop(&mut self) {
        self.timer.abort();
    }
}

impl AsyncUdpSocket for Hopping {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(HopPoller {
            shared: self.shared.clone(),
            inner: None,
        })
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        let (socket, port) = {
            let state = self.shared.state();
            (state.current.clone(), state.port)
        };
        socket.try_send(&Transmit {
            destination: SocketAddr::new(self.shared.server, port),
            ..transmit.clone()
        })
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let (current, previous) = {
            let mut state = self.shared.state();
            if !state
                .reader
                .as_ref()
                .is_some_and(|reader| reader.will_wake(cx.waker()))
            {
                state.reader = Some(cx.waker().clone());
            }
            (state.current.clone(), state.previous.clone())
        };
        for socket in [Some(current), previous].into_iter().flatten() {
            if let Poll::Ready(received) = socket.poll_recv(cx, bufs, meta) {
                let n = received?;
                for meta in &mut meta[..n] {
                    meta.addr = self.shared.remote;
                }
                return Poll::Ready(Ok(n));
            }
        }
        Poll::Pending
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.shared.state().current.local_addr()
    }

    fn max_transmit_segments(&self) -> usize {
        self.shared.state().current.max_transmit_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.shared.state().current.max_receive_segments()
    }

    fn may_fragment(&self) -> bool {
        self.shared.state().current.may_fragment()
    }
}

/// Waits until the current socket can send, following it across hops.
#[derive(Debug)]
struct HopPoller {
    shared: Arc<Shared>,
    /// The current socket's own poller, and the hop it belongs to.
    inner: Option<(u64, Pin<Box<dyn UdpPoller>>)>,
}

impl UdpPoller for HopPoller {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let (hops, current) = {
            let state = this.shared.state();
            (state.hops, state.current.clone())
        };
        if this.inner.as_ref().is_none_or(|(of, _)| *of != hops) {
            this.inner = Some((hops, current.create_io_poller()));
        }
        let (_, inner) = this.inner.as_mut().expect("the current socket's poller");
        inner.as_mut().poll_writable(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quinn::{Runtime, TokioRuntime};
    use std::sync::Weak;
    use tokio::net::UdpSocket;
    use tokio::time::Instant;

    fn set(ranges: &[RangeInclusive<u16>]) -> PortSet {
        PortSet::new(ranges)
    }

    #[test]
    fn the_port_set_merges_what_overlaps_or_touches() {
        assert_eq!(
            set(&[5000..=6000, 1234..=1234, 5500..=6100, 6101..=6101, 80..=80]),
            PortSet(vec![80..=80, 1234..=1234, 5000..=6101])
        );
        let ports = set(&[443..=443, 5000..=5002, 443..=443]);
        assert_eq!(ports.len(), 4);
        assert_eq!(
            (0..4).map(|i| ports.nth(i)).collect::<Vec<_>>(),
            [443, 5000, 5001, 5002]
        );
        assert_eq!(ports.index_of(5001), Some(2));
        assert_eq!(ports.index_of(444), None);
        assert_eq!(set(&[1..=65535]).len(), 65535);
    }

    /// A hop always moves to another port of the set; a set of one stays.
    #[test]
    fn a_hop_moves_to_another_port_of_the_set() {
        let ports = set(&[7000..=7002, 9000..=9000]);
        let mut seen = std::collections::BTreeSet::new();
        let mut port = ports.random();
        for _ in 0..400 {
            let next = ports.other_than(port);
            assert_ne!(next, port);
            assert!(ports.index_of(next).is_some(), "{next}");
            seen.insert(next);
            port = next;
        }
        assert_eq!(
            seen.into_iter().collect::<Vec<_>>(),
            [7000, 7001, 7002, 9000]
        );
        assert_eq!(set(&[443..=443]).other_than(443), 443);
    }

    fn loopback() -> io::Result<Arc<dyn AsyncUdpSocket>> {
        TokioRuntime.wrap_udp_socket(std::net::UdpSocket::bind("127.0.0.1:0")?)
    }

    /// Plain sockets as the server: one per port.
    async fn server(count: usize) -> Vec<UdpSocket> {
        let mut sockets = Vec::new();
        for _ in 0..count {
            sockets.push(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        }
        sockets
    }

    fn ports_of(sockets: &[UdpSocket]) -> Vec<RangeInclusive<u16>> {
        sockets
            .iter()
            .map(|s| {
                let port = s.local_addr().unwrap().port();
                port..=port
            })
            .collect()
    }

    /// Sends as quinn does: waits until the socket can, then tries.
    async fn send(socket: &Arc<Hopping>, bytes: &[u8]) {
        let mut poller = socket.clone().create_io_poller();
        let transmit = Transmit {
            destination: socket.remote(),
            ecn: None,
            contents: bytes,
            segment_size: None,
            src_ip: None,
        };
        tokio::time::timeout(
            Duration::from_secs(5),
            std::future::poll_fn(|cx| {
                loop {
                    if let Err(e) = std::task::ready!(poller.as_mut().poll_writable(cx)) {
                        return Poll::Ready(Err(e));
                    }
                    match socket.try_send(&transmit) {
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                        sent => return Poll::Ready(sent),
                    }
                }
            }),
        )
        .await
        .expect("sent within the bound")
        .unwrap();
    }

    async fn recv(socket: &Hopping) -> (Vec<u8>, SocketAddr) {
        let mut storage = [0u8; 1500];
        let mut meta = [RecvMeta::default()];
        let n = tokio::time::timeout(
            Duration::from_secs(5),
            std::future::poll_fn(|cx| {
                let mut bufs = [IoSliceMut::new(&mut storage)];
                socket.poll_recv(cx, &mut bufs, &mut meta)
            }),
        )
        .await
        .expect("a datagram within the bound")
        .unwrap();
        assert_eq!(n, 1);
        (storage[..meta[0].len].to_vec(), meta[0].addr)
    }

    /// Which of `sockets` heard `bytes`, and from where.
    async fn heard(sockets: &[UdpSocket], bytes: &[u8]) -> (usize, SocketAddr) {
        let mut buf = [0u8; 1500];
        let (at, n, from) = tokio::time::timeout(
            Duration::from_secs(5),
            std::future::poll_fn(|cx| {
                for (at, socket) in sockets.iter().enumerate() {
                    let mut read = tokio::io::ReadBuf::new(&mut buf);
                    if let Poll::Ready(from) = socket.poll_recv_from(cx, &mut read) {
                        return Poll::Ready((at, read.filled().len(), from.unwrap()));
                    }
                }
                Poll::Pending
            }),
        )
        .await
        .expect("a datagram within the bound");
        assert_eq!(&buf[..n], bytes);
        (at, from)
    }

    /// What goes out goes to the current port from the current socket,
    /// whatever quinn names; what comes back — on the previous socket too —
    /// comes from the address quinn knows.
    #[tokio::test]
    async fn late_packets_on_the_previous_socket_are_still_heard() {
        let server = server(2).await;
        let socket = Arc::new(
            Hopping::start(
                "127.0.0.1".parse().unwrap(),
                &ports_of(&server),
                Duration::from_secs(3600),
                Box::new(loopback),
            )
            .unwrap(),
        );
        send(&socket, b"first").await;
        let (at, before) = heard(&server, b"first").await;
        assert_eq!(
            server[at].local_addr().unwrap(),
            socket.remote(),
            "starts on the port quinn knows"
        );
        socket.shared.hop(loopback().unwrap());
        send(&socket, b"second").await;
        let (now, after) = heard(&server, b"second").await;
        assert_ne!(now, at, "another port");
        assert_ne!(after, before, "another local socket");
        // the server answers the old socket late, and the new one
        server[at].send_to(b"late", before).await.unwrap();
        assert_eq!(recv(&socket).await, (b"late".to_vec(), socket.remote()));
        server[now].send_to(b"new", after).await.unwrap();
        assert_eq!(recv(&socket).await, (b"new".to_vec(), socket.remote()));
        // one more hop retires the first socket
        socket.shared.hop(loopback().unwrap());
        server[at].send_to(b"gone", before).await.unwrap();
        server[now].send_to(b"kept", after).await.unwrap();
        assert_eq!(recv(&socket).await.0, b"kept");
    }

    /// Hops come by themselves, every interval.
    #[tokio::test]
    async fn the_timer_hops_every_interval() {
        let server = server(3).await;
        let socket = Hopping::start(
            "127.0.0.1".parse().unwrap(),
            &ports_of(&server),
            Duration::from_millis(50),
            Box::new(loopback),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while socket.shared.state().hops < 3 {
            assert!(Instant::now() < deadline, "no hops");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// The timer is the socket's: it ends when the socket goes.
    #[tokio::test]
    async fn dropping_the_socket_ends_its_timer() {
        let server = server(2).await;
        let socket = Hopping::start(
            "127.0.0.1".parse().unwrap(),
            &ports_of(&server),
            Duration::from_millis(20),
            Box::new(loopback),
        )
        .unwrap();
        let shared: Weak<Shared> = Arc::downgrade(&socket.shared);
        drop(socket);
        let deadline = Instant::now() + Duration::from_secs(5);
        while shared.upgrade().is_some() {
            assert!(Instant::now() < deadline, "the timer outlived its socket");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[test]
    fn no_port_no_hopping() {
        let err = Hopping::start(
            "127.0.0.1".parse().unwrap(),
            &[],
            Duration::from_secs(30),
            Box::new(loopback),
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "quic: no port to hop among");
    }
}
```

新建 `crates/rurge-net/src/quic/carrier.rs`：

```rust
//! A QUIC connection's packets through a carrier (phase 2 M7 design 3.3):
//! the UDP flow `underlying-proxy` gives to the server, which knows where
//! its datagrams go. quinn knows the server by a stand-in address: what it
//! sends goes to the carrier's peer whatever the destination, and all the
//! carrier receives comes from the stand-in.

use super::socket::NeverBlocks;
use crate::connector::BoxedDatagram;
use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::{Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker, ready};
use tokio::io::ReadBuf;

/// What quinn knows a carried connection's server by: an address of the
/// documentation range (RFC 5737), never dialled.
pub(crate) const STAND_IN: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), 443);

pub(crate) struct Carried(pub(crate) BoxedDatagram);

impl fmt::Debug for Carried {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Carried").finish_non_exhaustive()
    }
}

impl AsyncUdpSocket for Carried {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(NeverBlocks)
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        // a carrier queues what it sends and drops it when full, as a full
        // socket would: one that cannot take it now drops it too
        let mut cx = Context::from_waker(Waker::noop());
        if let Poll::Ready(Err(e)) = self.0.poll_send(&mut cx, transmit.contents) {
            tracing::trace!(error = %e, "a QUIC datagram through the chain was not sent");
        }
        Ok(())
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let mut buf = ReadBuf::new(&mut bufs[0]);
        // a carrier that has closed fails the endpoint, and with it the
        // connection, at once
        ready!(self.0.poll_recv(cx, &mut buf))?;
        let len = buf.filled().len();
        meta[0] = RecvMeta {
            addr: STAND_IN,
            len,
            stride: len,
            ecn: None,
            dst_ip: None,
        };
        Poll::Ready(Ok(1))
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0))
    }

    /// Datagrams through a proxy may be fragmented on the way: no path MTU
    /// discovery, quinn stays at the 1200 bytes every path carries.
    fn may_fragment(&self) -> bool {
        true
    }
}
```

新建 `crates/rurge-net/src/quic/dial.rs`：

```rust
//! The dial (phase 2 M7 design 3.2): a quinn endpoint of its own per
//! connection, the transport parameters every QUIC policy uses, and quinn's
//! errors as I/O errors (design 3.6).

use super::Path;
use quinn::congestion::CubicConfig;
use quinn::crypto::rustls::QuicClientConfig;
use quinn::{
    ClientConfig, Connection, ConnectionError, Endpoint, EndpointConfig, IdleTimeout, TokioRuntime,
    TransportConfig, TransportErrorCode, VarInt,
};
use std::io;
use std::sync::Arc;
use std::time::Duration;

/// The transport parameters of a client connection. `STANDARD` is what
/// every QUIC policy uses; tests shorten its times.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transport {
    /// Closes the connection when nothing came from the server for this
    /// long: a path that died (sleep, a network change) is noticed.
    pub idle_timeout: Duration,
    /// How often an otherwise idle connection proves it is alive.
    pub keep_alive: Duration,
    /// What a stream may receive before its reader catches up.
    pub stream_window: u32,
    /// What the connection's streams together may receive.
    pub connection_window: u32,
}

impl Transport {
    /// Idle timeout 30 seconds, keep-alive every 10; receive windows of
    /// 8 MiB per stream and 20 MiB per connection, as the Hysteria
    /// reference client's.
    pub const STANDARD: Transport = Transport {
        idle_timeout: Duration::from_secs(30),
        keep_alive: Duration::from_secs(10),
        stream_window: 8 << 20,
        connection_window: 20 << 20,
    };

    fn config(&self) -> TransportConfig {
        let mut config = TransportConfig::default();
        config
            // a timeout too long to say is none
            .max_idle_timeout(IdleTimeout::try_from(self.idle_timeout).ok())
            .keep_alive_interval(Some(self.keep_alive))
            .stream_receive_window(VarInt::from_u32(self.stream_window))
            .receive_window(VarInt::from_u32(self.connection_window))
            // no protocol over QUIC here has the server open a
            // bidirectional stream; unidirectional ones stay as quinn has
            // them (HTTP/3's control and QPACK streams, TUIC's packets)
            .max_concurrent_bidi_streams(VarInt::from_u32(0))
            // quinn's own default, and its only congestion control not
            // marked experimental (M7-D10)
            .congestion_controller_factory(Arc::new(CubicConfig::default()));
        config
    }
}

/// The client configuration of a QUIC policy: `tls` — which must allow TLS
/// 1.3 (quinn panics on a handshake without it) and offers the protocol's
/// ALPN — with `transport`. Built once per outbound; never 0-RTT.
pub fn client_config(
    tls: Arc<rustls::ClientConfig>,
    transport: &Transport,
) -> io::Result<ClientConfig> {
    let crypto = QuicClientConfig::try_from(tls)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("tls: {e}")))?;
    let mut config = ClientConfig::new(Arc::new(crypto));
    config.transport_config(Arc::new(transport.config()));
    Ok(config)
}

/// A connection to the server behind `path`, on an endpoint of its own
/// that ends with the connection. `server_name` is what TLS verifies and
/// sends as SNI (none for an IP address). Nothing bounds the handshake
/// here but the idle timeout: the caller's request timeout does (design
/// 3.5).
pub async fn connect(
    path: Path,
    config: &ClientConfig,
    server_name: &str,
) -> io::Result<Connection> {
    let endpoint = Endpoint::new_with_abstract_socket(
        EndpointConfig::default(),
        None,
        path.socket,
        Arc::new(TokioRuntime),
    )?;
    let connecting = endpoint
        .connect_with(config.clone(), path.remote, server_name)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("quic: {e}")))?;
    connecting.await.map_err(|e| describe(&e, true))
}

/// A connection's end as an I/O error: `quic: …`, or `tls: …` for a failed
/// handshake. Neither quotes anything sent; what the server sent along is
/// cleaned and cut short.
pub fn connection_error(e: &ConnectionError) -> io::Error {
    describe(e, false)
}

/// How long a reason from the server may be in an error text.
const REASON_MAX: usize = 200;

/// quinn's reason when the endpoint's receive loop is gone: the socket
/// under it failed (a carrier that closed) and every connection with it.
const DRIVER_LOST: &str = "endpoint driver future was dropped";

fn describe(e: &ConnectionError, handshake: bool) -> io::Error {
    use io::ErrorKind::{
        BrokenPipe, ConnectionAborted, ConnectionReset, InvalidData, Other, TimedOut,
    };
    let (kind, text) = match e {
        ConnectionError::TimedOut if handshake => {
            (TimedOut, "quic: the handshake timed out".to_string())
        }
        ConnectionError::TimedOut => (
            TimedOut,
            "quic: the server stopped answering (idle timeout)".to_string(),
        ),
        ConnectionError::Reset => (
            ConnectionReset,
            "quic: the server no longer knows the connection (stateless reset)".to_string(),
        ),
        // closed here — or, once its driver goes, a connection whose
        // endpoint was lost (`DRIVER_LOST` before)
        ConnectionError::LocallyClosed => (
            ConnectionAborted,
            "quic: the connection was closed".to_string(),
        ),
        ConnectionError::VersionMismatch => (
            Other,
            "quic: the server does not speak QUIC version 1".to_string(),
        ),
        ConnectionError::CidsExhausted => (
            Other,
            "quic: no connection ID left for a new connection".to_string(),
        ),
        // what went wrong here; a TLS failure carries the alert it sent
        ConnectionError::TransportError(error) => {
            if alert(error.code.into()).is_some() {
                (
                    InvalidData,
                    format!("tls: {}", untrusted(error.reason.as_bytes())),
                )
            } else if error.code == TransportErrorCode::INTERNAL_ERROR
                && error.reason == DRIVER_LOST
            {
                (
                    BrokenPipe,
                    "quic: the UDP path to the server failed".to_string(),
                )
            } else if error.reason.is_empty() {
                (InvalidData, format!("quic: {}", error.code))
            } else {
                (
                    InvalidData,
                    format!("quic: {}", untrusted(error.reason.as_bytes())),
                )
            }
        }
        ConnectionError::ConnectionClosed(close) => match alert(close.error_code.into()) {
            Some(alert) => (
                InvalidData,
                format!("tls: the server refused the handshake ({alert:?})"),
            ),
            None => (
                ConnectionAborted,
                closed(
                    "transport error",
                    u64::from(close.error_code),
                    &close.reason,
                ),
            ),
        },
        ConnectionError::ApplicationClosed(close) => (
            ConnectionAborted,
            closed("code", close.error_code.into_inner(), &close.reason),
        ),
    };
    io::Error::new(kind, text)
}

/// The TLS alert of a QUIC transport error code: CRYPTO_ERROR is 0x100
/// plus the alert (RFC 9001 4.8).
fn alert(code: u64) -> Option<rustls::AlertDescription> {
    (0x100..0x200)
        .contains(&code)
        .then(|| rustls::AlertDescription::from(code as u8))
}

fn closed(what: &str, code: u64, reason: &[u8]) -> String {
    let reason = untrusted(reason);
    if reason.is_empty() {
        format!("quic: the server closed the connection ({what} {code:#x})")
    } else {
        format!("quic: the server closed the connection ({what} {code:#x}: {reason})")
    }
}

/// Text from the other end, made safe for a log line: control characters
/// dropped, at most `REASON_MAX` characters kept.
fn untrusted(text: &[u8]) -> String {
    String::from_utf8_lossy(text)
        .chars()
        .filter(|c| !c.is_control())
        .take(REASON_MAX)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use quinn::{ApplicationClose, ConnectionClose};

    /// quinn's settings as its `Debug` shows them: ours, and the defaults
    /// HTTP/3 and TUIC rely on (100 unidirectional streams from the server,
    /// datagram buffers, path MTU discovery from 1200 bytes).
    #[test]
    fn the_transport_parameters_reach_quinn() {
        let config = format!("{:?}", Transport::STANDARD.config());
        for expected in [
            "max_concurrent_bidi_streams: 0,",
            "max_concurrent_uni_streams: 100,",
            "max_idle_timeout: Some(30000),",
            "stream_receive_window: 8388608,",
            "receive_window: 20971520,",
            "keep_alive_interval: Some(10s),",
            "initial_mtu: 1200,",
            "upper_bound: 1452,",
            "datagram_receive_buffer_size: Some(1250000),",
            "datagram_send_buffer_size: 1048576,",
        ] {
            assert!(config.contains(expected), "{expected} in {config}");
        }
    }

    /// QUIC's first packets are protected with AES-128-GCM: TLS without it
    /// cannot be used.
    #[test]
    fn tls_without_the_initial_cipher_suite_is_refused() {
        let provider = rustls::crypto::CryptoProvider {
            cipher_suites: vec![rustls::crypto::ring::cipher_suite::TLS13_AES_256_GCM_SHA384],
            ..rustls::crypto::ring::default_provider()
        };
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(provider))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();
        let err = client_config(Arc::new(tls), &Transport::STANDARD).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(err.to_string(), "tls: no initial cipher suite found");
    }

    /// A transport error found here, with its reason.
    fn found(code: TransportErrorCode, reason: &str) -> ConnectionError {
        let mut error = ConnectionError::TransportError(code.into());
        if let ConnectionError::TransportError(e) = &mut error {
            e.reason = reason.to_string();
        }
        error
    }

    #[test]
    fn quinns_errors_read_as_quic_or_tls() {
        use io::ErrorKind::*;
        let cases = [
            (
                found(
                    TransportErrorCode::crypto(42),
                    "invalid peer certificate: BadSignature",
                ),
                InvalidData,
                "tls: invalid peer certificate: BadSignature",
            ),
            (
                found(TransportErrorCode::INTERNAL_ERROR, DRIVER_LOST),
                BrokenPipe,
                "quic: the UDP path to the server failed",
            ),
            (
                found(TransportErrorCode::PROTOCOL_VIOLATION, "bad\nframe"),
                InvalidData,
                "quic: badframe",
            ),
            (
                found(TransportErrorCode::FLOW_CONTROL_ERROR, ""),
                InvalidData,
                "quic: received more data than permitted in advertised data limits",
            ),
            (
                ConnectionError::ConnectionClosed(ConnectionClose {
                    error_code: TransportErrorCode::crypto(120),
                    frame_type: None,
                    reason: Bytes::new(),
                }),
                InvalidData,
                "tls: the server refused the handshake (NoApplicationProtocol)",
            ),
            (
                ConnectionError::ConnectionClosed(ConnectionClose {
                    error_code: TransportErrorCode::CONNECTION_REFUSED,
                    frame_type: None,
                    reason: Bytes::new(),
                }),
                ConnectionAborted,
                "quic: the server closed the connection (transport error 0x2)",
            ),
            (
                ConnectionError::ApplicationClosed(ApplicationClose {
                    error_code: VarInt::from_u32(0x100),
                    reason: Bytes::from_static(b"bye"),
                }),
                ConnectionAborted,
                "quic: the server closed the connection (code 0x100: bye)",
            ),
            (
                ConnectionError::Reset,
                ConnectionReset,
                "quic: the server no longer knows the connection (stateless reset)",
            ),
            (
                ConnectionError::TimedOut,
                TimedOut,
                "quic: the server stopped answering (idle timeout)",
            ),
            (
                ConnectionError::LocallyClosed,
                ConnectionAborted,
                "quic: the connection was closed",
            ),
            (
                ConnectionError::VersionMismatch,
                Other,
                "quic: the server does not speak QUIC version 1",
            ),
            (
                ConnectionError::CidsExhausted,
                Other,
                "quic: no connection ID left for a new connection",
            ),
        ];
        for (error, kind, text) in cases {
            let io = connection_error(&error);
            assert_eq!(
                (io.kind(), io.to_string().as_str()),
                (kind, text),
                "{error:?}"
            );
        }
        assert_eq!(
            describe(&ConnectionError::TimedOut, true).to_string(),
            "quic: the handshake timed out"
        );
    }

    /// What the server says is cut short.
    #[test]
    fn a_long_reason_is_cut_short() {
        let error = ConnectionError::ApplicationClosed(ApplicationClose {
            error_code: VarInt::from_u32(1),
            reason: Bytes::from(vec![b'x'; 1000]),
        });
        assert_eq!(
            connection_error(&error).to_string(),
            format!(
                "quic: the server closed the connection (code 0x1: {})",
                "x".repeat(REASON_MAX)
            )
        );
    }
}
```

新建 `crates/rurge-net/src/quic/testing.rs`：

```rust
//! Loopback QUIC servers for tests: a certificate and the configurations on
//! either side of it, an echo server, and a server socket on several ports
//! for port hopping. Never used by production code.

use super::socket::NeverBlocks;
use quinn::crypto::rustls::QuicServerConfig;
use quinn::udp::{RecvMeta, Transmit};
use quinn::{
    AsyncUdpSocket, Connection, Endpoint, EndpointConfig, Runtime, ServerConfig, TokioRuntime,
    UdpPoller,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::collections::HashMap;
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tokio::task::JoinHandle;

/// A self-signed certificate for `localhost` and `127.0.0.1`.
pub struct TestCert {
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
}

impl Default for TestCert {
    fn default() -> Self {
        TestCert::new()
    }
}

impl TestCert {
    pub fn new() -> TestCert {
        let rcgen::CertifiedKey { cert, signing_key } = rcgen::generate_simple_self_signed(vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
        ])
        .expect("a self-signed certificate");
        TestCert {
            cert: cert.der().clone(),
            key: signing_key.into(),
        }
    }

    /// Roots that trust the certificate.
    pub fn roots(&self) -> Arc<rustls::RootCertStore> {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(self.cert.clone()).expect("the certificate");
        Arc::new(roots)
    }

    /// A server presenting the certificate over TLS 1.3, choosing among
    /// `alpn`; datagrams on, the rest quinn's defaults.
    pub fn server_config(&self, alpn: &[&str]) -> ServerConfig {
        let mut tls = rustls::ServerConfig::builder_with_provider(provider())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("TLS 1.3")
            .with_no_client_auth()
            .with_single_cert(vec![self.cert.clone()], self.key.clone_key())
            .expect("the certificate and its key");
        tls.alpn_protocols = alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
        let crypto = QuicServerConfig::try_from(Arc::new(tls)).expect("a QUIC server config");
        ServerConfig::with_crypto(Arc::new(crypto))
    }

    /// A client's TLS 1.3 that trusts the certificate, offering `alpn`.
    pub fn client_tls(&self, alpn: &[&str]) -> Arc<rustls::ClientConfig> {
        let mut tls = rustls::ClientConfig::builder_with_provider(provider())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("TLS 1.3")
            .with_root_certificates(self.roots())
            .with_no_client_auth();
        tls.alpn_protocols = alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
        Arc::new(tls)
    }
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// A server endpoint presenting `config` on `socket`.
pub fn endpoint(config: ServerConfig, socket: Arc<dyn AsyncUdpSocket>) -> Endpoint {
    Endpoint::new_with_abstract_socket(
        EndpointConfig::default(),
        Some(config),
        socket,
        Arc::new(TokioRuntime),
    )
    .expect("a server endpoint")
}

/// A UDP socket on `127.0.0.1`, any port, that quinn-udp drives.
pub fn loopback_socket() -> Arc<dyn AsyncUdpSocket> {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("a loopback socket");
    TokioRuntime
        .wrap_udp_socket(socket)
        .expect("a quinn socket")
}

/// Answers on every connection: each bidirectional stream with what it
/// carries, each datagram with itself. Dropping it closes the endpoint.
pub struct EchoServer {
    endpoint: Endpoint,
    addr: SocketAddr,
    accept: JoinHandle<()>,
}

impl EchoServer {
    /// On `127.0.0.1`, any port.
    pub fn spawn(config: ServerConfig) -> EchoServer {
        EchoServer::spawn_on(config, loopback_socket())
    }

    /// On `socket` (`MultiPort` for port hopping).
    pub fn spawn_on(config: ServerConfig, socket: Arc<dyn AsyncUdpSocket>) -> EchoServer {
        let addr = socket.local_addr().expect("the server's address");
        let endpoint = endpoint(config, socket);
        let accept = tokio::spawn({
            let endpoint = endpoint.clone();
            async move {
                while let Some(incoming) = endpoint.accept().await {
                    tokio::spawn(async move {
                        if let Ok(connection) = incoming.await {
                            echo(connection).await;
                        }
                    });
                }
            }
        });
        EchoServer {
            endpoint,
            addr,
            accept,
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for EchoServer {
    fn drop(&mut self) {
        self.accept.abort();
        self.endpoint.close(0u32.into(), b"");
    }
}

async fn echo(connection: Connection) {
    loop {
        tokio::select! {
            stream = connection.accept_bi() => {
                let Ok((mut send, mut recv)) = stream else { return };
                tokio::spawn(async move {
                    if tokio::io::copy(&mut recv, &mut send).await.is_ok() {
                        let _ = send.finish();
                    }
                });
            }
            datagram = connection.read_datagram() => {
                let Ok(datagram) = datagram else { return };
                let _ = connection.send_datagram(datagram);
            }
        }
    }
}

/// A server socket on several loopback ports at once, for port hopping
/// (phase 2 M7 design 3.3). It hears on every port and answers each client
/// address from the port it last heard that address on, as a server behind
/// a port-range DNAT does; it counts what each port heard.
#[derive(Debug)]
pub struct MultiPort {
    sockets: Vec<Arc<dyn AsyncUdpSocket>>,
    ports: Vec<u16>,
    heard: Vec<AtomicUsize>,
    /// The socket each client address was last heard on.
    routes: Mutex<HashMap<SocketAddr, usize>>,
}

impl MultiPort {
    /// On `count` ports of `127.0.0.1`.
    pub fn bind(count: usize) -> Arc<MultiPort> {
        let sockets: Vec<_> = (0..count).map(|_| loopback_socket()).collect();
        let ports = sockets
            .iter()
            .map(|s| s.local_addr().expect("a port").port())
            .collect();
        Arc::new(MultiPort {
            heard: sockets.iter().map(|_| AtomicUsize::new(0)).collect(),
            sockets,
            ports,
            routes: Mutex::default(),
        })
    }

    pub fn ports(&self) -> &[u16] {
        &self.ports
    }

    /// How many datagrams each port heard, in the order of `ports`.
    pub fn heard(&self) -> Vec<usize> {
        self.heard
            .iter()
            .map(|n| n.load(Ordering::Relaxed))
            .collect()
    }

    /// How many client addresses it heard from.
    pub fn clients(&self) -> usize {
        self.routes.lock().expect("the routes").len()
    }
}

impl AsyncUdpSocket for MultiPort {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(NeverBlocks)
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        let at = self
            .routes
            .lock()
            .expect("the routes")
            .get(&transmit.destination)
            .copied()
            .unwrap_or(0);
        match self.sockets[at].try_send(transmit) {
            // dropped, as by a full socket: one port waiting stalls no other
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(()),
            sent => sent,
        }
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        for (at, socket) in self.sockets.iter().enumerate() {
            if let Poll::Ready(received) = socket.poll_recv(cx, bufs, meta) {
                let n = received?;
                let mut routes = self.routes.lock().expect("the routes");
                for meta in &meta[..n] {
                    routes.insert(meta.addr, at);
                    self.heard[at].fetch_add(1, Ordering::Relaxed);
                }
                return Poll::Ready(Ok(n));
            }
        }
        Poll::Pending
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.sockets[0].local_addr()
    }

    fn max_transmit_segments(&self) -> usize {
        self.sockets[0].max_transmit_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.sockets[0].max_receive_segments()
    }

    fn may_fragment(&self) -> bool {
        self.sockets[0].may_fragment()
    }
}
```

要点：
- 各层的 `try_send` 遇到 `WouldBlock` 以外的错误一律丢包、返回成功（报给 quinn 会让它的驱动结束而连接挂住）。
- 端口跳跃的计时任务随 socket 结束；上一个 socket 保留到下一次跳跃，期间照收迟到的包；交给 quinn 的来源地址总是固定的服务器地址。

- [ ] **Step 4: 运行**

Run: `cargo test -p rurge-net quic`
Expected: 通过（新增 30 条：每一层上的流与数据报回显、`tos` 的路径、`ecn=off`、经回环 `Datagram` 的载体、对同时监听几个端口的服务端跳跃且连接不断、迟到的包照收、socket 丢掉后计时任务结束、写了 `interface` 时直连路径保留网卡的绑定、保活与空闲超时、拨号错误的文本）。
Run: `cargo test -p rurge-net connector` → 通过（`a_udp_socket_has_the_policys_options`）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add Cargo.toml Cargo.lock crates/rurge-net
git commit -m "feat(net): QUIC 的 UDP 层与拨号——直连 / tos / ecn=off / 链式载体 / 端口跳跃，每条连接一个 Endpoint"
```

### Task 3: `rurge-net::quic::h3`——最小 HTTP/3 客户端

只用静态表的 QPACK、控制流与 SETTINGS、请求与应答、可作隧道的请求流、GOAWAY，以及给假服务端用的服务端一侧（P11、P12）。extended CONNECT、`:protocol`、`H3_DATAGRAM` 与 HTTP Datagram 属于 M7c：SETTINGS 可查询、请求经 `http::Request` 传入，留好扩展点，现在不实现。新模块的用例都在新文件里，没有单独的失败步骤。

**Files:**
- Create: `crates/rurge-net/src/quic/h3/{mod.rs, frame.rs, qpack.rs, huffman.rs, stream.rs, client.rs, testing.rs}`（自带用例；`testing.rs` 在 `testing` feature 下）
- Modify: `crates/rurge-net/src/quic/mod.rs`

**Interfaces:**
- Consumes: Task 2 的 `quinn::Connection`（经 `connect` 得到）。
- Produces: `rurge_net::quic::h3::Client`（`Client::new(connection: Connection) -> Client`、`settings()`（等服务端的 SETTINGS）、`is_going_away()`、`request(http::Request<()>) -> RequestStream`）；`RequestStream`（`response()`；作字节流时 `AsyncRead + AsyncWrite`，写入取消安全，`shutdown` 发 FIN）；`Settings::get`；测试设施 `h3::testing::{H3Server, ServerStream}`（`peek_varint`、`read_headers`、`read_body`、`send_headers` / `send_data`、原始读写）

- [ ] **Step 1: 新模块**

`crates/rurge-net/src/quic/mod.rs`——把

```rust
mod dial;
```

换成

```rust
mod dial;
pub mod h3;
```

新建 `crates/rurge-net/src/quic/h3/mod.rs`：

```rust
//! A minimal HTTP/3 client (phase 2 M7 design 4): what the QUIC-based
//! protocols need of HTTP/3 — Hysteria 2's authentication request now,
//! MASQUE's and `trust-tunnel`'s CONNECT and DNS over HTTP/3 in M7c.
//!
//! - Our control stream carries our SETTINGS: no dynamic QPACK table
//!   (capacity 0, no blocked streams) and field sections of at most 64 KiB.
//!   We open no QPACK streams: an encoder that never uses a dynamic table
//!   and a decoder that has none have nothing to say on them (RFC 9204
//!   4.2).
//! - Of the server's streams, its control stream must start with SETTINGS
//!   and may then bring GOAWAY; its QPACK streams are read and their bytes
//!   dropped; a push stream is a connection error (we never send
//!   MAX_PUSH_ID); a stream of any other type is stopped.
//! - Each request is a bidirectional stream of its own: HEADERS, then the
//!   content as DATA frames written through the stream; the response head
//!   comes back (interim responses passed over), then the response content
//!   as the stream's bytes. The trailers are ignored.
//! - QPACK uses the static table alone: an exact entry, else a literal
//!   value with the entry of the name, else a literal name and value.
//!   Nothing sent is Huffman-encoded; what comes back may be.
//! - An error of the connection closes it with its RFC 9114 or RFC 9204
//!   code; an error of a response ends its stream both ways.
//!
//! The client opens bidirectional streams for its requests and takes none
//! of the connection's others: a protocol may open streams of its own on
//! the same connection (Hysteria 2's TCP requests).
//!
//! Extended CONNECT (`:protocol`) and HTTP Datagrams come in M7c: the
//! server's SETTINGS are kept for them (`Settings::get`).

mod client;
mod frame;
mod huffman;
mod qpack;
mod stream;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use client::Client;
pub use frame::Settings;
pub use stream::RequestStream;

use quinn::VarInt;
use std::borrow::Cow;
use std::io;

/// The dynamic table capacity a peer's QPACK encoder may use (RFC 9204 5).
pub const SETTINGS_QPACK_MAX_TABLE_CAPACITY: u64 = 0x01;
/// The largest field section a peer accepts (RFC 9114 7.2.4.1).
pub const SETTINGS_MAX_FIELD_SECTION_SIZE: u64 = 0x06;
/// How many streams a peer's QPACK decoder lets wait for the dynamic table
/// (RFC 9204 5).
pub const SETTINGS_QPACK_BLOCKED_STREAMS: u64 = 0x07;
/// Extended CONNECT (RFC 9220 3).
pub const SETTINGS_ENABLE_CONNECT_PROTOCOL: u64 = 0x08;
/// HTTP Datagrams (RFC 9297 2.1.1).
pub const SETTINGS_H3_DATAGRAM: u64 = 0x33;

/// The largest field section this client takes — names and values plus 32
/// bytes per field (RFC 9114 4.2.2) — as our SETTINGS announce. It also
/// bounds the payload of a HEADERS frame: an encoder that compresses at
/// all never needs more bytes than the section's size.
pub(crate) const MAX_FIELD_SECTION_SIZE: u64 = 64 << 10;

// unidirectional stream types (RFC 9114 6.2, RFC 9204 4.2)
pub(crate) const CONTROL_STREAM: u64 = 0x00;
pub(crate) const PUSH_STREAM: u64 = 0x01;
pub(crate) const QPACK_ENCODER_STREAM: u64 = 0x02;
pub(crate) const QPACK_DECODER_STREAM: u64 = 0x03;

// error codes (RFC 9114 8.1, RFC 9204 6)
pub(crate) const H3_STREAM_CREATION_ERROR: VarInt = VarInt::from_u32(0x103);
pub(crate) const H3_CLOSED_CRITICAL_STREAM: VarInt = VarInt::from_u32(0x104);
pub(crate) const H3_FRAME_UNEXPECTED: VarInt = VarInt::from_u32(0x105);
pub(crate) const H3_FRAME_ERROR: VarInt = VarInt::from_u32(0x106);
pub(crate) const H3_EXCESSIVE_LOAD: VarInt = VarInt::from_u32(0x107);
pub(crate) const H3_ID_ERROR: VarInt = VarInt::from_u32(0x108);
pub(crate) const H3_SETTINGS_ERROR: VarInt = VarInt::from_u32(0x109);
pub(crate) const H3_MISSING_SETTINGS: VarInt = VarInt::from_u32(0x10a);
pub(crate) const H3_REQUEST_CANCELLED: VarInt = VarInt::from_u32(0x10c);
pub(crate) const H3_MESSAGE_ERROR: VarInt = VarInt::from_u32(0x10e);
pub(crate) const QPACK_DECOMPRESSION_FAILED: VarInt = VarInt::from_u32(0x200);

/// The name of an HTTP/3 or QPACK error code, as error texts say it; an
/// unknown code in hex.
pub(crate) fn code_name(code: VarInt) -> Cow<'static, str> {
    Cow::Borrowed(match u64::from(code) {
        0x100 => "H3_NO_ERROR",
        0x101 => "H3_GENERAL_PROTOCOL_ERROR",
        0x102 => "H3_INTERNAL_ERROR",
        0x103 => "H3_STREAM_CREATION_ERROR",
        0x104 => "H3_CLOSED_CRITICAL_STREAM",
        0x105 => "H3_FRAME_UNEXPECTED",
        0x106 => "H3_FRAME_ERROR",
        0x107 => "H3_EXCESSIVE_LOAD",
        0x108 => "H3_ID_ERROR",
        0x109 => "H3_SETTINGS_ERROR",
        0x10a => "H3_MISSING_SETTINGS",
        0x10b => "H3_REQUEST_REJECTED",
        0x10c => "H3_REQUEST_CANCELLED",
        0x10d => "H3_REQUEST_INCOMPLETE",
        0x10e => "H3_MESSAGE_ERROR",
        0x10f => "H3_CONNECT_ERROR",
        0x110 => "H3_VERSION_FALLBACK",
        0x200 => "QPACK_DECOMPRESSION_FAILED",
        0x201 => "QPACK_ENCODER_STREAM_ERROR",
        0x202 => "QPACK_DECODER_STREAM_ERROR",
        other => return Cow::Owned(format!("{other:#x}")),
    })
}

/// A violation of HTTP/3 or QPACK: the code that closes the connection or
/// ends the stream it happened on, and what happened.
#[derive(Clone, Debug)]
pub(crate) struct H3Error {
    pub(crate) code: VarInt,
    pub(crate) text: Cow<'static, str>,
}

impl H3Error {
    pub(crate) fn new(code: VarInt, text: impl Into<Cow<'static, str>>) -> H3Error {
        H3Error {
            code,
            text: text.into(),
        }
    }

    pub(crate) fn io(&self) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, format!("h3: {}", self.text))
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{H3Server, ServerStream, response_section};
    use super::*;
    use crate::quic::testing::{TestCert, endpoint, loopback_socket};
    use crate::quic::{Path, Transport, client_config, connect};
    use http::{HeaderValue, Request, Response};
    use quinn::{Connection, ConnectionError, Endpoint, SendStream, TransportConfig};
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Waker};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio::time::{Instant, timeout};

    const ALPN: &[&str] = &["h3"];
    const BOUND: Duration = Duration::from_secs(10);
    /// The fake's SETTINGS unless a test says otherwise.
    const SERVER_SETTINGS: &[(u64, u64)] = &[(SETTINGS_MAX_FIELD_SECTION_SIZE, 1 << 16)];
    /// `:status 200` from the static table.
    const OK: &[u8] = &[0x00, 0x00, 0xd9];

    /// A QUIC connection over the loopback with the client's `transport`
    /// and `tune` applied to the server's: the server's endpoint, the
    /// client's side and the server's side.
    async fn connected_with(
        transport: &Transport,
        tune: impl FnOnce(&mut TransportConfig),
    ) -> (Endpoint, Connection, Connection) {
        let cert = TestCert::new();
        let mut config = cert.server_config(ALPN);
        let mut server_transport = TransportConfig::default();
        tune(&mut server_transport);
        config.transport_config(Arc::new(server_transport));
        let server = endpoint(config, loopback_socket());
        let path = Path {
            socket: loopback_socket(),
            remote: server.local_addr().unwrap(),
        };
        let client = client_config(cert.client_tls(ALPN), transport).unwrap();
        let (connection, accepted) = tokio::join!(connect(path, &client, "localhost"), async {
            server
                .accept()
                .await
                .expect("a connection")
                .await
                .expect("accepted")
        });
        (server, connection.expect("connected"), accepted)
    }

    async fn connected() -> (Endpoint, Connection, Connection) {
        connected_with(&Transport::STANDARD, |_| {}).await
    }

    /// Our client and the fake on one connection, the fake's SETTINGS sent.
    struct Pair {
        client: Client,
        server: H3Server,
        /// The client's side of the connection, for streams of its own.
        quic: Connection,
        server_quic: Connection,
        _endpoint: Endpoint,
    }

    async fn pair() -> Pair {
        let (endpoint, quic, server_quic) = connected().await;
        Pair {
            client: Client::new(quic.clone()),
            server: H3Server::start(server_quic.clone(), SERVER_SETTINGS)
                .await
                .unwrap(),
            quic,
            server_quic,
            _endpoint: endpoint,
        }
    }

    fn get(uri: &str) -> Request<()> {
        Request::get(uri).body(()).unwrap()
    }

    fn tunnel(authority: &str) -> Request<()> {
        Request::connect(authority).body(()).unwrap()
    }

    fn owned(fields: &[(&str, &str)]) -> Vec<(String, String)> {
        fields
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    /// A unidirectional stream of the fake's carrying `bytes`, ended or not.
    async fn open_uni(quic: &Connection, bytes: &[u8], end: bool) -> io::Result<SendStream> {
        let mut send = quic.open_uni().await?;
        send.write_all(bytes).await?;
        if end {
            let _ = send.finish();
        }
        Ok(send)
    }

    /// The code the client closed the connection with, as the server saw it.
    async fn closed_with(server_quic: &Connection) -> u64 {
        match timeout(BOUND, server_quic.closed())
            .await
            .expect("closed within the bound")
        {
            ConnectionError::ApplicationClosed(close) => close.error_code.into_inner(),
            other => panic!("closed otherwise: {other:?}"),
        }
    }

    /// Waits until `check` holds, looking again and again until the bound.
    async fn eventually(mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + BOUND;
        while !check() {
            assert!(Instant::now() < deadline, "not within the bound");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// A request without content, answered 204 without content.
    async fn round_trip(client: &Client, server: &H3Server) -> io::Result<u16> {
        let request = async {
            let mut stream = client.request(get("https://example.test/")).await?;
            stream.shutdown().await?;
            Ok(stream.response().await?.status().as_u16())
        };
        let serve = async {
            let mut stream = server.accept().await.expect("a request");
            stream.read_headers().await.unwrap();
            stream.send_headers(204, &[]).await.unwrap();
            stream.finish();
        };
        let (status, ()) = timeout(BOUND, async { tokio::join!(request, serve) })
            .await
            .expect("within the bound");
        status
    }

    /// A request answered with a HEADERS frame of `section`, the server's
    /// side left open: the response, and the server's side of the stream.
    async fn answered_with(p: &Pair, section: &[u8]) -> (io::Result<Response<()>>, ServerStream) {
        let request = async {
            let mut stream = p.client.request(get("https://example.test/")).await?;
            stream.shutdown().await?;
            stream.response().await
        };
        let serve = async {
            let mut stream = p.server.accept().await.expect("a request");
            stream.read_headers().await.unwrap();
            stream.send_frame(0x01, section).await.unwrap();
            stream
        };
        timeout(BOUND, async { tokio::join!(request, serve) })
            .await
            .expect("within the bound")
    }

    /// Hysteria 2's authentication as it goes: the head with its secret, a
    /// content, a response of status 233 with fields the fake encodes
    /// with Huffman, and its content.
    #[tokio::test]
    async fn a_request_and_its_response_with_content() {
        let p = pair().await;
        let mut secret = HeaderValue::from_static("secret");
        secret.set_sensitive(true);
        let request = Request::post("https://hysteria/auth")
            .header("hysteria-auth", secret)
            .header("hysteria-cc-rx", "0")
            .body(())
            .unwrap();
        let client = async {
            let mut stream = p.client.request(request).await.unwrap();
            stream.write_all(b"ping").await.unwrap();
            stream.shutdown().await.unwrap();
            let response = stream.response().await.unwrap();
            let mut content = Vec::new();
            stream.read_to_end(&mut content).await.unwrap();
            (response, content)
        };
        let server = async {
            let mut stream = p.server.accept().await.unwrap();
            let head = stream.read_headers().await.unwrap();
            let content = stream.read_body().await.unwrap();
            stream
                .send_headers(233, &[("hysteria-udp", "true"), ("hysteria-cc-rx", "auto")])
                .await
                .unwrap();
            stream.send_data(b"pong").await.unwrap();
            stream.finish();
            (head, content)
        };
        let ((response, content), (head, request_content)) =
            timeout(BOUND, async { tokio::join!(client, server) })
                .await
                .unwrap();
        assert_eq!(
            head,
            owned(&[
                (":method", "POST"),
                (":scheme", "https"),
                (":authority", "hysteria"),
                (":path", "/auth"),
                ("hysteria-auth", "secret"),
                ("hysteria-cc-rx", "0"),
            ])
        );
        assert_eq!(request_content, b"ping");
        assert_eq!(response.status(), 233);
        assert_eq!(response.headers()["hysteria-udp"], "true");
        assert_eq!(response.headers()["hysteria-cc-rx"], "auto");
        assert_eq!(content, b"pong");
    }

    /// Contents larger than the windows, both ways; the response's in
    /// frames of one byte, a megabyte and the rest.
    #[tokio::test]
    async fn large_contents_cross_both_ways() {
        let p = pair().await;
        let up: Vec<u8> = (0..3usize << 20).map(|i| (i % 251) as u8).collect();
        let down: Vec<u8> = up.iter().rev().copied().collect();
        let client = async {
            let mut stream = p
                .client
                .request(Request::post("https://example.test/").body(()).unwrap())
                .await
                .unwrap();
            stream.write_all(&up).await.unwrap();
            stream.shutdown().await.unwrap();
            assert_eq!(stream.response().await.unwrap().status(), 200);
            let mut content = Vec::new();
            stream.read_to_end(&mut content).await.unwrap();
            content
        };
        let server = async {
            let mut stream = p.server.accept().await.unwrap();
            stream.read_headers().await.unwrap();
            let content = stream.read_body().await.unwrap();
            stream.send_headers(200, &[]).await.unwrap();
            stream.send_data(&down[..1]).await.unwrap();
            stream.send_data(&down[1..1 << 20]).await.unwrap();
            stream.send_data(&down[1 << 20..]).await.unwrap();
            stream.finish();
            content
        };
        let (content, received) = timeout(BOUND, async { tokio::join!(client, server) })
            .await
            .unwrap();
        assert!(received == up, "the request's content");
        assert!(content == down, "the response's content");
    }

    #[tokio::test]
    async fn the_client_sends_its_settings_and_no_qpack_streams() {
        let p = pair().await;
        let settings = timeout(BOUND, p.server.client_settings())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            settings,
            [
                (SETTINGS_QPACK_MAX_TABLE_CAPACITY, 0),
                (SETTINGS_MAX_FIELD_SECTION_SIZE, 65_536),
                (SETTINGS_QPACK_BLOCKED_STREAMS, 0),
            ]
        );
        assert_eq!(round_trip(&p.client, &p.server).await.unwrap(), 204);
        assert_eq!(p.server.client_streams(), [CONTROL_STREAM]);
    }

    /// Requests do not wait for the server's SETTINGS; the wait for them
    /// ends when they come, all of them kept.
    #[tokio::test]
    async fn settings_that_come_late_are_waited_for() {
        let (_endpoint, quic, server_quic) = connected().await;
        let client = Client::new(quic);
        let mut server = H3Server::new(server_quic);
        assert!(
            timeout(Duration::from_millis(200), client.settings())
                .await
                .is_err(),
            "SETTINGS that never came"
        );
        assert_eq!(round_trip(&client, &server).await.unwrap(), 204);
        let (settings, sent) = timeout(BOUND, async {
            tokio::join!(
                client.settings(),
                server.send_settings(&[(SETTINGS_ENABLE_CONNECT_PROTOCOL, 1), (0x21, 7)])
            )
        })
        .await
        .unwrap();
        sent.unwrap();
        let settings = settings.unwrap();
        assert_eq!(settings.get(SETTINGS_ENABLE_CONNECT_PROTOCOL), Some(1));
        assert_eq!(settings.get(0x21), Some(7));
        assert_eq!(settings.get(SETTINGS_H3_DATAGRAM), None);
    }

    #[tokio::test]
    async fn goaway_stops_new_requests_and_lets_those_under_way_finish() {
        let mut p = pair().await;
        timeout(BOUND, async {
            let mut first = p
                .client
                .request(get("https://example.test/a"))
                .await
                .unwrap();
            first.shutdown().await.unwrap();
            let mut served = p.server.accept().await.unwrap();
            served.read_headers().await.unwrap();
            // the first request is on stream 0: GOAWAY 8, then 4, leave it
            // served (a GOAWAY may come down)
            p.server.goaway(8).await.unwrap();
            p.server.goaway(4).await.unwrap();
            eventually(|| p.client.is_going_away()).await;
            let err = p
                .client
                .request(get("https://example.test/b"))
                .await
                .unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::ConnectionAborted);
            assert_eq!(err.to_string(), "h3: the server is going away");
            served.send_headers(200, &[]).await.unwrap();
            served.finish();
            assert_eq!(first.response().await.unwrap().status(), 200);
        })
        .await
        .unwrap();
    }

    /// A GOAWAY may only come down, and names a request stream.
    #[tokio::test]
    async fn a_goaway_that_goes_up_or_names_no_request_closes_the_connection() {
        for ids in [&[8, 12][..], &[2]] {
            let mut p = pair().await;
            for &id in ids {
                p.server.goaway(id).await.unwrap();
            }
            assert_eq!(
                closed_with(&p.server_quic).await,
                u64::from(H3_ID_ERROR),
                "GOAWAY {ids:?}"
            );
        }
    }

    /// What the fake does on a fresh connection, to see the client close it.
    enum Act {
        /// SETTINGS on the fake's control stream.
        Settings(&'static [(u64, u64)]),
        /// A frame on the control stream, after the SETTINGS.
        Control(u64, &'static [u8]),
        /// A unidirectional stream of these bytes, left open.
        Open(&'static [u8]),
        /// A unidirectional stream of these bytes, ended.
        Ended(&'static [u8]),
    }

    #[tokio::test]
    async fn the_servers_streams_are_checked() {
        use Act::*;
        // a control stream with an empty SETTINGS frame
        const CONTROL: &[u8] = &[0x00, 0x04, 0x00];
        let cases: [(&str, &[Act], VarInt); 15] = [
            (
                "a push stream",
                &[Settings(&[]), Open(&[0x01, 0x00])],
                H3_ID_ERROR,
            ),
            (
                "a second control stream",
                &[Settings(&[]), Open(CONTROL)],
                H3_STREAM_CREATION_ERROR,
            ),
            (
                "a second QPACK decoder stream",
                &[Settings(&[]), Open(&[0x03]), Open(&[0x03])],
                H3_STREAM_CREATION_ERROR,
            ),
            (
                "a control stream that starts with GOAWAY",
                &[Open(&[0x00, 0x07, 0x01, 0x00])],
                H3_MISSING_SETTINGS,
            ),
            (
                "a second SETTINGS",
                &[Settings(&[]), Control(0x04, &[])],
                H3_FRAME_UNEXPECTED,
            ),
            (
                "an HTTP/2 setting",
                &[Settings(&[(0x02, 1)])],
                H3_SETTINGS_ERROR,
            ),
            (
                "a setting twice",
                &[Settings(&[(0x06, 1), (0x06, 2)])],
                H3_SETTINGS_ERROR,
            ),
            (
                "DATA on the control stream",
                &[Settings(&[]), Control(0x00, b"x")],
                H3_FRAME_UNEXPECTED,
            ),
            (
                "HEADERS on the control stream",
                &[Settings(&[]), Control(0x01, OK)],
                H3_FRAME_UNEXPECTED,
            ),
            (
                "HTTP/2's PING",
                &[Settings(&[]), Control(0x06, &[0; 8])],
                H3_FRAME_UNEXPECTED,
            ),
            (
                "MAX_PUSH_ID, a client's frame",
                &[Settings(&[]), Control(0x0d, &[0x00])],
                H3_FRAME_UNEXPECTED,
            ),
            (
                "CANCEL_PUSH with no push allowed",
                &[Settings(&[]), Control(0x03, &[0x00])],
                H3_ID_ERROR,
            ),
            (
                "a GOAWAY with more than its ID",
                &[Settings(&[]), Control(0x07, &[0x00, 0x00])],
                H3_FRAME_ERROR,
            ),
            (
                "the control stream ended",
                &[Ended(CONTROL)],
                H3_CLOSED_CRITICAL_STREAM,
            ),
            (
                "a QPACK encoder stream ended",
                &[Settings(&[]), Ended(&[0x02])],
                H3_CLOSED_CRITICAL_STREAM,
            ),
        ];
        for (case, acts, code) in cases {
            let (_endpoint, quic, server_quic) = connected().await;
            let _client = Client::new(quic);
            let mut server = H3Server::new(server_quic.clone());
            let mut streams = Vec::new();
            for act in acts {
                // an act after the close finds the connection gone
                let _ = match act {
                    Settings(settings) => server.send_settings(settings).await,
                    Control(ty, payload) => server.control_frame(*ty, payload).await,
                    Open(bytes) => open_uni(&server_quic, bytes, false)
                        .await
                        .map(|s| streams.push(s)),
                    Ended(bytes) => open_uni(&server_quic, bytes, true)
                        .await
                        .map(|s| streams.push(s)),
                };
            }
            assert_eq!(closed_with(&server_quic).await, u64::from(code), "{case}");
        }
    }

    /// The QPACK streams' instructions are dropped, a stream of an unknown
    /// type is stopped, an unknown frame is passed over: the connection
    /// goes on.
    #[tokio::test]
    async fn qpack_streams_and_unknown_types_are_passed_over() {
        let mut p = pair().await;
        timeout(BOUND, async {
            // Set Dynamic Table Capacity 0; Stream Cancellation of stream 0
            let _encoder = open_uni(&p.server_quic, &[0x02, 0x20], false)
                .await
                .unwrap();
            let _decoder = open_uni(&p.server_quic, &[0x03, 0x40], false)
                .await
                .unwrap();
            let reserved = open_uni(&p.server_quic, &[0x21, b'?'], false)
                .await
                .unwrap();
            assert_eq!(
                reserved.stopped().await.unwrap(),
                Some(H3_STREAM_CREATION_ERROR)
            );
            p.server.control_frame(0x21, b"reserved").await.unwrap();
            assert_eq!(round_trip(&p.client, &p.server).await.unwrap(), 204);
        })
        .await
        .unwrap();
        assert!(p.quic.close_reason().is_none());
    }

    #[tokio::test]
    async fn interim_responses_unknown_frames_and_trailers_are_passed_over() {
        let p = pair().await;
        let client = async {
            let mut stream = p
                .client
                .request(get("https://example.test/"))
                .await
                .unwrap();
            stream.shutdown().await.unwrap();
            let response = stream.response().await.unwrap();
            let mut content = Vec::new();
            stream.read_to_end(&mut content).await.unwrap();
            (response, content)
        };
        let server = async {
            let mut stream = p.server.accept().await.unwrap();
            stream.read_headers().await.unwrap();
            stream.send_frame(0x21, b"before").await.unwrap();
            stream
                .send_headers(103, &[("link", "</style.css>; rel=preload")])
                .await
                .unwrap();
            stream.send_headers(100, &[]).await.unwrap();
            stream
                .send_headers(200, &[("content-type", "text/plain")])
                .await
                .unwrap();
            stream.send_data(b"a").await.unwrap();
            stream.send_frame(0x1f * 7 + 0x21, b"").await.unwrap();
            stream.send_data(b"").await.unwrap();
            stream.send_data(b"b").await.unwrap();
            // trailers, then a reserved frame
            stream.send_frame(0x01, &[0x00, 0x00]).await.unwrap();
            stream.send_frame(0x21, b"after").await.unwrap();
            stream.finish();
        };
        let ((response, content), ()) = timeout(BOUND, async { tokio::join!(client, server) })
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "text/plain");
        assert!(response.headers().get("link").is_none());
        assert_eq!(content, b"ab");
    }

    /// A stream window of one byte has the server's streams come a byte at
    /// a time: every frame start, field section and payload is read in
    /// pieces.
    #[tokio::test]
    async fn a_response_that_comes_a_byte_at_a_time() {
        let transport = Transport {
            stream_window: 1,
            ..Transport::STANDARD
        };
        let (_endpoint, quic, server_quic) = connected_with(&transport, |_| {}).await;
        let client = Client::new(quic);
        let server = H3Server::start(server_quic, SERVER_SETTINGS).await.unwrap();
        // a field section and a payload too long for a one-byte length
        let long = "v".repeat(100);
        let request = async {
            let mut stream = client.request(get("https://example.test/")).await.unwrap();
            stream.shutdown().await.unwrap();
            let response = stream.response().await.unwrap();
            let mut content = Vec::new();
            stream.read_to_end(&mut content).await.unwrap();
            (response, content)
        };
        let serve = async {
            let mut stream = server.accept().await.unwrap();
            stream.read_headers().await.unwrap();
            stream.send_frame(0x21, b"reserved").await.unwrap();
            stream.send_headers(103, &[]).await.unwrap();
            stream
                .send_headers(200, &[("x-long", &long)])
                .await
                .unwrap();
            stream.send_data(&[7; 300]).await.unwrap();
            stream.send_frame(0x01, &[0x00, 0x00]).await.unwrap();
            stream.finish();
        };
        let ((response, content), ()) = timeout(BOUND, async { tokio::join!(request, serve) })
            .await
            .unwrap();
        assert_eq!(response.headers()["x-long"], long.as_str());
        assert_eq!(content, [7; 300]);
        let settings = timeout(BOUND, client.settings()).await.unwrap().unwrap();
        assert_eq!(settings.get(SETTINGS_MAX_FIELD_SECTION_SIZE), Some(1 << 16));
    }

    /// RFC 9204 B.4's field section refers to a dynamic table this client
    /// never allowed.
    #[tokio::test]
    async fn a_dynamic_table_reference_closes_the_connection() {
        let p = pair().await;
        let (response, _served) = answered_with(&p, &[0x05, 0x00, 0x80, 0xc1, 0x81]).await;
        let err = response.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            err.to_string(),
            "h3: the server's field section cannot be decoded"
        );
        assert_eq!(
            closed_with(&p.server_quic).await,
            u64::from(QPACK_DECOMPRESSION_FAILED)
        );
        let err = p
            .client
            .request(get("https://example.test/"))
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "h3: the server's field section cannot be decoded"
        );
    }

    /// A malformed response ends its own stream (H3_MESSAGE_ERROR); the
    /// connection serves on.
    #[tokio::test]
    async fn a_malformed_response_ends_its_stream_alone() {
        let p = pair().await;
        for fields in [
            &[("content-type", "text/plain")][..],
            &[("x", "1"), (":status", "200")],
            &[(":status", "200"), ("X-Upper", "1")],
            &[(":status", "200"), ("connection", "close")],
        ] {
            let section = qpack::encode(
                fields
                    .iter()
                    .map(|(n, v)| (n.as_bytes(), v.as_bytes(), false)),
            );
            let (response, served) = answered_with(&p, &section).await;
            let err = response.unwrap_err();
            assert_eq!(
                err.to_string(),
                "h3: the server sent a malformed response",
                "{fields:?}"
            );
            assert_eq!(
                timeout(BOUND, served.client_stopped()).await.unwrap(),
                Some(u64::from(H3_MESSAGE_ERROR))
            );
        }
        assert_eq!(round_trip(&p.client, &p.server).await.unwrap(), 204);
    }

    /// A field section of 64 KiB is taken; one byte more, or a HEADERS
    /// frame longer than that, ends the stream (H3_EXCESSIVE_LOAD).
    #[tokio::test]
    async fn an_oversized_response_head_ends_its_stream_alone() {
        let p = pair().await;
        // :status 200 counts 42 bytes, x-big 37 and its value
        let fits = "a".repeat(65_536 - 42 - 37);
        let (response, _served) =
            answered_with(&p, &response_section(200, &[("x-big", &fits)])).await;
        assert_eq!(response.unwrap().headers()["x-big"].len(), fits.len());
        let over = format!("{fits}a");
        for section in [response_section(200, &[("x-big", &over)]), vec![0; 65_537]] {
            let (response, served) = answered_with(&p, &section).await;
            assert_eq!(
                response.unwrap_err().to_string(),
                "h3: the response head is larger than 65536 bytes"
            );
            assert_eq!(
                timeout(BOUND, served.client_stopped()).await.unwrap(),
                Some(u64::from(H3_EXCESSIVE_LOAD))
            );
        }
        assert_eq!(round_trip(&p.client, &p.server).await.unwrap(), 204);
    }

    /// Frames as the fake sends them: type and payload.
    type Frames = &'static [(u64, &'static [u8])];

    #[tokio::test]
    async fn frames_out_of_place_close_the_connection() {
        let cases: [(&str, Frames, VarInt); 6] = [
            ("DATA before the head", &[(0x00, b"x")], H3_FRAME_UNEXPECTED),
            (
                "SETTINGS on a request stream",
                &[(0x04, b"")],
                H3_FRAME_UNEXPECTED,
            ),
            (
                "a push promised",
                &[(0x05, &[0x00, 0x00, 0x00])],
                H3_ID_ERROR,
            ),
            (
                "HTTP/2's WINDOW_UPDATE",
                &[(0x08, &[0; 4])],
                H3_FRAME_UNEXPECTED,
            ),
            (
                "GOAWAY after the head",
                &[(0x01, OK), (0x07, &[0x00])],
                H3_FRAME_UNEXPECTED,
            ),
            (
                "DATA after the trailers",
                &[(0x01, OK), (0x01, &[0x00, 0x00]), (0x00, b"x")],
                H3_FRAME_UNEXPECTED,
            ),
        ];
        for (case, frames, code) in cases {
            let p = pair().await;
            let request = async {
                let mut stream = p.client.request(get("https://example.test/")).await?;
                stream.shutdown().await?;
                stream.response().await?;
                stream.read_to_end(&mut Vec::new()).await
            };
            let serve = async {
                let mut stream = p.server.accept().await.unwrap();
                stream.read_headers().await.unwrap();
                for (ty, payload) in frames {
                    // a frame after the close finds the stream gone
                    let _ = stream.send_frame(*ty, payload).await;
                }
                stream
            };
            let (result, _served) = timeout(BOUND, async { tokio::join!(request, serve) })
                .await
                .unwrap();
            assert!(result.is_err(), "{case}");
            assert_eq!(closed_with(&p.server_quic).await, u64::from(code), "{case}");
        }
    }

    #[tokio::test]
    async fn a_frame_cut_by_the_end_of_the_stream_closes_the_connection() {
        let p = pair().await;
        let request = async {
            let mut stream = p.client.request(get("https://example.test/")).await?;
            stream.shutdown().await?;
            stream.response().await?;
            let mut content = Vec::new();
            stream.read_to_end(&mut content).await.map(|_| content)
        };
        let serve = async {
            let mut stream = p.server.accept().await.unwrap();
            stream.read_headers().await.unwrap();
            stream.send_frame(0x01, OK).await.unwrap();
            // DATA of 10 bytes, 3 of which come before the end
            stream
                .write_all(&[0x00, 0x0a, b'a', b'b', b'c'])
                .await
                .unwrap();
            stream.finish();
            stream
        };
        let (result, _served) = timeout(BOUND, async { tokio::join!(request, serve) })
            .await
            .unwrap();
        assert_eq!(
            result.unwrap_err().to_string(),
            "h3: the server's stream ended within a frame"
        );
        assert_eq!(closed_with(&p.server_quic).await, u64::from(H3_FRAME_ERROR));
    }

    /// The server takes 2000 bytes of a stream before it reads: a frame
    /// larger than that waits half written, the next write is given up (as
    /// a write that times out is), and what follows comes after the whole
    /// frame — nothing of what was given up.
    #[tokio::test]
    async fn a_write_given_up_mid_frame_is_finished_first() {
        let (_endpoint, quic, server_quic) = connected_with(&Transport::STANDARD, |transport| {
            transport.stream_receive_window(VarInt::from_u32(2000));
        })
        .await;
        let client = Client::new(quic);
        let server = H3Server::start(server_quic, SERVER_SETTINGS).await.unwrap();
        timeout(BOUND, async {
            let mut stream = client.request(tunnel("example.test:443")).await.unwrap();
            let mut served = server.accept().await.unwrap();
            served.read_headers().await.unwrap();
            served.send_headers(200, &[]).await.unwrap();
            assert_eq!(stream.response().await.unwrap().status(), 200);
            assert_eq!(stream.write(&[1; 3000]).await.unwrap(), 3000);
            let mut cx = Context::from_waker(Waker::noop());
            assert!(
                Pin::new(&mut stream)
                    .poll_write(&mut cx, &[2; 100])
                    .is_pending()
            );
            let read = async {
                let mut frames = Vec::new();
                while let Some(frame) = served.read_frame().await.unwrap() {
                    frames.push(frame);
                }
                frames
            };
            let write = async {
                stream.write_all(&[3; 50]).await.unwrap();
                stream.shutdown().await.unwrap();
            };
            let (frames, ()) = tokio::join!(read, write);
            assert_eq!(frames, [(0x00, vec![1; 3000]), (0x00, vec![3; 50])]);
        })
        .await
        .unwrap();
    }

    /// A CONNECT has `:method` and `:authority` alone (RFC 9114 4.4); its
    /// shutdown sends the FIN and the tunnel reads on.
    #[tokio::test]
    async fn a_tunnel_reads_on_after_its_shutdown() {
        let p = pair().await;
        let client = async {
            let mut stream = p.client.request(tunnel("example.test:443")).await.unwrap();
            assert_eq!(stream.response().await.unwrap().status(), 200);
            stream.write_all(b"hello").await.unwrap();
            stream.shutdown().await.unwrap();
            let err = stream.write_all(b"more").await.unwrap_err();
            assert_eq!(err.to_string(), "h3: write after shutdown");
            let mut back = Vec::new();
            stream.read_to_end(&mut back).await.unwrap();
            back
        };
        let server = async {
            let mut stream = p.server.accept().await.unwrap();
            let head = stream.read_headers().await.unwrap();
            stream.send_headers(200, &[]).await.unwrap();
            let content = stream.read_body().await.unwrap();
            stream.send_data(b"world").await.unwrap();
            stream.finish();
            (head, content)
        };
        let (back, (head, content)) = timeout(BOUND, async { tokio::join!(client, server) })
            .await
            .unwrap();
        assert_eq!(
            head,
            owned(&[(":method", "CONNECT"), (":authority", "example.test:443")])
        );
        assert_eq!(content, b"hello");
        assert_eq!(back, b"world");
    }

    #[tokio::test]
    async fn a_request_dropped_is_cancelled_both_ways() {
        let p = pair().await;
        timeout(BOUND, async {
            let stream = p
                .client
                .request(get("https://example.test/"))
                .await
                .unwrap();
            let mut served = p.server.accept().await.unwrap();
            served.read_headers().await.unwrap();
            drop(stream);
            let cancelled = Some(u64::from(H3_REQUEST_CANCELLED));
            assert_eq!(served.client_reset().await, cancelled);
            assert_eq!(served.client_stopped().await, cancelled);
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_reset_by_the_server_is_reported_with_its_code() {
        let p = pair().await;
        let request = async {
            let mut stream = p.client.request(get("https://example.test/")).await?;
            stream.shutdown().await?;
            stream.response().await
        };
        let serve = async {
            let mut stream = p.server.accept().await.unwrap();
            stream.read_headers().await.unwrap();
            // H3_REQUEST_REJECTED: not served, may be sent again
            stream.reset(0x10b);
            stream
        };
        let (response, _served) = timeout(BOUND, async { tokio::join!(request, serve) })
            .await
            .unwrap();
        let err = response.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionReset);
        assert_eq!(
            err.to_string(),
            "h3: the server reset the stream (H3_REQUEST_REJECTED)"
        );
    }

    /// Hysteria 2 opens streams of its own on the client's connection; the
    /// fake tells them apart by their first integer.
    #[tokio::test]
    async fn other_protocols_open_streams_beside_the_client() {
        let p = pair().await;
        timeout(BOUND, async {
            let mut request = p
                .client
                .request(get("https://example.test/"))
                .await
                .unwrap();
            request.shutdown().await.unwrap();
            let (mut send, mut recv) = p.quic.open_bi().await.unwrap();
            send.write_all(&[0x44, 0x01, b'h', b'i']).await.unwrap();
            send.finish().unwrap();
            let mut http = p.server.accept().await.unwrap();
            let mut other = p.server.accept().await.unwrap();
            assert_eq!(http.peek_varint().await.unwrap(), 0x01);
            assert_eq!(other.peek_varint().await.unwrap(), 0x401);
            // a peek takes nothing: the stream is read from its start
            let mut start = Vec::new();
            other.read_to_end(&mut start).await.unwrap();
            assert_eq!(start, [0x44, 0x01, b'h', b'i']);
            other.write_all(b"raw").await.unwrap();
            AsyncWriteExt::shutdown(&mut other).await.unwrap();
            assert_eq!(recv.read_to_end(16).await.unwrap(), b"raw");
            http.read_headers().await.unwrap();
            http.send_headers(204, &[]).await.unwrap();
            http.finish();
            assert_eq!(request.response().await.unwrap().status(), 204);
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_lost_connection_fails_the_wait_and_new_requests() {
        let (_endpoint, quic, server_quic) = connected().await;
        let client = Client::new(quic);
        let _server = H3Server::new(server_quic.clone());
        server_quic.close(VarInt::from_u32(0x100), b"bye");
        let closed = "quic: the server closed the connection (code 0x100: bye)";
        let err = timeout(BOUND, client.settings())
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(err.to_string(), closed);
        let err = client
            .request(get("https://example.test/"))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), closed);
    }

    /// The client's task holds its control stream: the stream ends with
    /// the client.
    #[tokio::test]
    async fn the_clients_task_ends_with_it() {
        let p = pair().await;
        timeout(BOUND, p.server.client_settings())
            .await
            .unwrap()
            .unwrap();
        assert!(p.server.client_streams_ended().is_empty());
        drop(p.client);
        eventually(|| p.server.client_streams_ended() == [CONTROL_STREAM]).await;
    }

    /// The error codes as the texts name them.
    #[test]
    fn codes_have_their_names() {
        assert_eq!(code_name(H3_REQUEST_CANCELLED), "H3_REQUEST_CANCELLED");
        assert_eq!(
            code_name(QPACK_DECOMPRESSION_FAILED),
            "QPACK_DECOMPRESSION_FAILED"
        );
        assert_eq!(code_name(VarInt::from_u32(0x1234)), "0x1234");
    }
}
```

新建 `crates/rurge-net/src/quic/h3/frame.rs`：

```rust
//! HTTP/3 frames (RFC 9114 7), the variable-length integers they are made
//! of (RFC 9000 16), the server's SETTINGS, and the reading of a stream
//! frame by frame.

use super::{H3_FRAME_ERROR, H3_SETTINGS_ERROR, H3Error};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use quinn::{ReadError, RecvStream, VarInt};
use std::collections::HashSet;
use std::task::{Context, Poll, ready};
use tokio::io::ReadBuf;

// frame types (RFC 9114 7.2)
pub(crate) const DATA: u64 = 0x00;
pub(crate) const HEADERS: u64 = 0x01;
pub(crate) const CANCEL_PUSH: u64 = 0x03;
pub(crate) const SETTINGS: u64 = 0x04;
pub(crate) const PUSH_PROMISE: u64 = 0x05;
pub(crate) const GOAWAY: u64 = 0x07;
pub(crate) const MAX_PUSH_ID: u64 = 0x0d;

/// Frame types of HTTP/2 that HTTP/3 has none of: receiving one is
/// H3_FRAME_UNEXPECTED (RFC 9114 7.2.8).
pub(crate) fn is_http2_only(ty: u64) -> bool {
    matches!(ty, 0x02 | 0x06 | 0x08 | 0x09)
}

/// The largest variable-length integer.
pub(crate) const VARINT_MAX: u64 = (1 << 62) - 1;

/// The variable-length integer at the start of `buf` and how many bytes it
/// takes; `None` while `buf` holds only part of it. It may be longer than
/// its value needs.
pub(crate) fn get_varint(buf: &[u8]) -> Option<(u64, usize)> {
    let len = varint_len(buf);
    let bytes = buf.get(..len)?;
    let value = bytes[1..]
        .iter()
        .fold(u64::from(bytes[0] & 0x3f), |value, &byte| {
            (value << 8) | u64::from(byte)
        });
    Some((value, len))
}

/// How many bytes the variable-length integer at the start of `buf` takes,
/// as its first two bits say; 1 for an empty `buf`.
fn varint_len(buf: &[u8]) -> usize {
    buf.first().map_or(1, |&first| 1 << (first >> 6))
}

/// `value`, at most `VARINT_MAX`, in its shortest form.
pub(crate) fn put_varint(out: &mut impl BufMut, value: u64) {
    debug_assert!(value <= VARINT_MAX, "{value} is past QUIC's integers");
    match value {
        0..=0x3f => out.put_u8(value as u8),
        0x40..=0x3fff => out.put_u16(0x4000 | value as u16),
        0x4000..=0x3fff_ffff => out.put_u32(0x8000_0000 | value as u32),
        _ => out.put_u64(0xc000_0000_0000_0000 | value),
    }
}

/// A whole frame: its type, the length of `payload`, `payload`.
pub(crate) fn frame(ty: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 16);
    put_varint(&mut out, ty);
    put_varint(&mut out, payload.len() as u64);
    out.extend_from_slice(payload);
    out
}

/// The start of a DATA frame of `len` bytes, and how much of the array it
/// takes.
pub(crate) fn data_header(len: usize) -> ([u8; 9], usize) {
    let mut header = [0u8; 9];
    let mut rest = &mut header[..];
    put_varint(&mut rest, DATA);
    put_varint(&mut rest, len as u64);
    let unused = rest.len();
    (header, header.len() - unused)
}

/// A peer's SETTINGS (RFC 9114 7.2.4): each identifier it sent, with the
/// value, in the order sent — the unknown ones too, for the extensions that
/// look them up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings(Vec<(u64, u64)>);

impl Settings {
    /// The value sent for `id`; `None` when the peer did not send it, and
    /// it has its default.
    pub fn get(&self, id: u64) -> Option<u64> {
        self.0
            .iter()
            .find(|&&(sent, _)| sent == id)
            .map(|&(_, value)| value)
    }

    /// The payload of the server's SETTINGS frame: pairs of identifier and
    /// value. A pair cut short is H3_FRAME_ERROR; an identifier twice, or
    /// one of HTTP/2's that HTTP/3 reserved (0x02 to 0x05), is
    /// H3_SETTINGS_ERROR (RFC 9114 7.2.4, 7.2.4.1).
    pub(crate) fn decode(mut payload: &[u8]) -> Result<Settings, H3Error> {
        let cut = || H3Error::new(H3_FRAME_ERROR, "the server's SETTINGS frame is cut short");
        let mut pairs = Vec::new();
        let mut seen = HashSet::new();
        while !payload.is_empty() {
            let (id, n) = get_varint(payload).ok_or_else(cut)?;
            let (value, m) = get_varint(&payload[n..]).ok_or_else(cut)?;
            payload = &payload[n + m..];
            if (0x02..=0x05).contains(&id) {
                return Err(H3Error::new(
                    H3_SETTINGS_ERROR,
                    format!("the server sent the HTTP/2 setting {id:#x}"),
                ));
            }
            if !seen.insert(id) {
                return Err(H3Error::new(
                    H3_SETTINGS_ERROR,
                    format!("the server sent the setting {id:#x} twice"),
                ));
            }
            pairs.push((id, value));
        }
        Ok(Settings(pairs))
    }
}

/// A frame's start: its type and the length of its payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Header {
    pub(crate) ty: u64,
    pub(crate) len: u64,
}

/// Why a stream could not be read frame by frame.
#[derive(Debug)]
pub(crate) enum ReadFailure {
    /// The stream failed: reset, already stopped, or its connection lost.
    Stream(ReadError),
    /// It ended within a frame: H3_FRAME_ERROR (RFC 9114 7.1).
    Truncated,
}

/// The most one read takes from the stream when nothing says how much.
const CHUNK: usize = 16 << 10;

/// Reads a stream frame by frame. Frame starts and the payloads taken whole
/// are read into a buffer, exactly as much as they need; a DATA payload
/// goes from the stream straight into the caller's buffer.
pub(crate) struct FrameReader {
    recv: RecvStream,
    /// Read from the stream and not yet taken.
    buf: BytesMut,
    /// The stream ended after `buf`.
    end: bool,
}

impl FrameReader {
    pub(crate) fn new(recv: RecvStream) -> FrameReader {
        FrameReader {
            recv,
            buf: BytesMut::new(),
            end: false,
        }
    }

    /// A variable-length integer of its own (a unidirectional stream's
    /// type); `None` when the stream ended before it began.
    pub(crate) fn poll_varint(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<u64>, ReadFailure>> {
        loop {
            if let Some((value, n)) = get_varint(&self.buf) {
                self.buf.advance(n);
                return Poll::Ready(Ok(Some(value)));
            }
            if !ready!(self.poll_fill(cx, varint_len(&self.buf)))? {
                return Poll::Ready(self.ended());
            }
        }
    }

    /// The next frame's start; `None` when the stream ended between frames.
    pub(crate) fn poll_header(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<Header>, ReadFailure>> {
        loop {
            // both integers are taken together: a read that has to wait
            // leaves the type where it was
            let want = match get_varint(&self.buf) {
                None => varint_len(&self.buf),
                Some((ty, n)) => match get_varint(&self.buf[n..]) {
                    Some((len, m)) => {
                        self.buf.advance(n + m);
                        return Poll::Ready(Ok(Some(Header { ty, len })));
                    }
                    None => n + varint_len(&self.buf[n..]),
                },
            };
            if !ready!(self.poll_fill(cx, want))? {
                return Poll::Ready(self.ended());
            }
        }
    }

    /// The whole payload of `len` bytes.
    pub(crate) fn poll_payload(
        &mut self,
        cx: &mut Context<'_>,
        len: usize,
    ) -> Poll<Result<Bytes, ReadFailure>> {
        if !ready!(self.poll_fill(cx, len))? {
            return Poll::Ready(Err(ReadFailure::Truncated));
        }
        Poll::Ready(Ok(self.buf.split_to(len).freeze()))
    }

    /// Passes over the `left` bytes of a payload, counting them down.
    pub(crate) fn poll_skip(
        &mut self,
        cx: &mut Context<'_>,
        left: &mut u64,
    ) -> Poll<Result<(), ReadFailure>> {
        while *left > 0 {
            let want = usize::try_from(*left).unwrap_or(usize::MAX);
            if self.buf.is_empty() && ready!(self.poll_more(cx, want.min(CHUNK)))? == 0 {
                return Poll::Ready(Err(ReadFailure::Truncated));
            }
            let n = want.min(self.buf.len());
            self.buf.advance(n);
            *left -= n as u64;
        }
        Poll::Ready(Ok(()))
    }

    /// Some of the `left` bytes of a DATA payload into `out`, counting
    /// them down; nothing when `out` has no room.
    pub(crate) fn poll_data(
        &mut self,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
        left: &mut u64,
    ) -> Poll<Result<(), ReadFailure>> {
        let room = usize::try_from(*left)
            .unwrap_or(usize::MAX)
            .min(out.remaining());
        if room == 0 {
            return Poll::Ready(Ok(()));
        }
        let n = if !self.buf.is_empty() {
            let n = room.min(self.buf.len());
            out.put_slice(&self.buf[..n]);
            self.buf.advance(n);
            n
        } else {
            if self.end {
                return Poll::Ready(Err(ReadFailure::Truncated));
            }
            let read = ready!(self.recv.poll_read(cx, out.initialize_unfilled_to(room)));
            match read.map_err(ReadFailure::Stream)? {
                0 => {
                    self.end = true;
                    return Poll::Ready(Err(ReadFailure::Truncated));
                }
                n => {
                    out.advance(n);
                    n
                }
            }
        };
        *left -= n as u64;
        Poll::Ready(Ok(()))
    }

    /// Reads and drops whatever comes, until the stream ends.
    pub(crate) fn poll_drain(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), ReadFailure>> {
        loop {
            self.buf.clear();
            if ready!(self.poll_more(cx, CHUNK))? == 0 {
                return Poll::Ready(Ok(()));
            }
        }
    }

    /// Asks the peer to send no more (STOP_SENDING with `code`).
    pub(crate) fn stop(&mut self, code: VarInt) {
        // fails only for a stream already stopped or ended
        let _ = self.recv.stop(code);
    }

    /// The stream ended: cleanly when nothing is left half read.
    fn ended<T>(&self) -> Result<Option<T>, ReadFailure> {
        if self.buf.is_empty() {
            Ok(None)
        } else {
            Err(ReadFailure::Truncated)
        }
    }

    /// At least `want` bytes in the buffer; `false` when the stream ended
    /// first.
    fn poll_fill(&mut self, cx: &mut Context<'_>, want: usize) -> Poll<Result<bool, ReadFailure>> {
        while self.buf.len() < want {
            if ready!(self.poll_more(cx, want - self.buf.len()))? == 0 {
                return Poll::Ready(Ok(false));
            }
        }
        Poll::Ready(Ok(true))
    }

    /// Up to `max` (more than 0) more bytes into the buffer: how many, 0
    /// when the stream ended.
    fn poll_more(&mut self, cx: &mut Context<'_>, max: usize) -> Poll<Result<usize, ReadFailure>> {
        if self.end {
            return Poll::Ready(Ok(0));
        }
        let start = self.buf.len();
        self.buf.resize(start + max, 0);
        let read = self.recv.poll_read(cx, &mut self.buf[start..]);
        let n = match read {
            Poll::Ready(Ok(n)) => n,
            _ => 0,
        };
        self.buf.truncate(start + n);
        match read {
            Poll::Ready(Ok(0)) => {
                self.end = true;
                Poll::Ready(Ok(0))
            }
            Poll::Ready(Ok(n)) => Poll::Ready(Ok(n)),
            Poll::Ready(Err(e)) => Poll::Ready(Err(ReadFailure::Stream(e))),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quic::h3::{
        SETTINGS_ENABLE_CONNECT_PROTOCOL, SETTINGS_MAX_FIELD_SECTION_SIZE,
        SETTINGS_QPACK_MAX_TABLE_CAPACITY,
    };

    #[test]
    fn varints_go_out_shortest_and_come_in_any_length() {
        // the examples of RFC 9000 A.1, the last one longer than it needs
        for (bytes, value) in [
            (
                &[0xc2, 0x19, 0x7c, 0x5e, 0xff, 0x14, 0xe8, 0x8c][..],
                151_288_809_941_952_652,
            ),
            (&[0x9d, 0x7f, 0x3e, 0x7d], 494_878_333),
            (&[0x7b, 0xbd], 15_293),
            (&[0x25], 37),
            (&[0x40, 0x25], 37),
        ] {
            assert_eq!(get_varint(bytes), Some((value, bytes.len())), "{value}");
        }
        // the edges of each length
        for (value, len) in [
            (0, 1),
            (63, 1),
            (64, 2),
            (16_383, 2),
            (16_384, 4),
            ((1 << 30) - 1, 4),
            (1 << 30, 8),
            (VARINT_MAX, 8),
        ] {
            let mut out = Vec::new();
            put_varint(&mut out, value);
            assert_eq!(out.len(), len, "{value}");
            assert_eq!(get_varint(&out), Some((value, len)), "{value}");
        }
        // Hysteria 2's TCP request (0x401) as it goes on the wire; what
        // follows an integer is left alone
        let mut out = Vec::new();
        put_varint(&mut out, 0x401);
        assert_eq!(out, [0x44, 0x01]);
        assert_eq!(get_varint(&[0x25, 0xff]), Some((37, 1)));
    }

    #[test]
    fn a_varint_needs_all_its_bytes() {
        for bytes in [&[][..], &[0x40], &[0x80, 0, 0], &[0xc0, 0, 0, 0, 0, 0, 0]] {
            assert_eq!(get_varint(bytes), None, "{bytes:02x?}");
        }
    }

    #[test]
    fn frames_are_type_length_payload() {
        assert_eq!(frame(HEADERS, b"abc"), [0x01, 0x03, b'a', b'b', b'c']);
        // a reserved type (0x1f * N + 0x21), a length of two bytes
        let reserved = frame(0x1f * 3 + 0x21, &[0; 300]);
        assert_eq!(reserved[..4], [0x40, 0x7e, 0x41, 0x2c]);
        assert_eq!(reserved.len(), 304);
        let (header, n) = data_header(70_000);
        assert_eq!(header[..n], [0x00, 0x80, 0x01, 0x11, 0x70]);
        let (header, n) = data_header(5);
        assert_eq!(header[..n], [0x00, 0x05]);
        assert!(is_http2_only(0x06) && !is_http2_only(GOAWAY) && !is_http2_only(0x21));
    }

    #[test]
    fn settings_keep_every_pair_in_order() {
        let mut payload = Vec::new();
        for (id, value) in [
            (SETTINGS_QPACK_MAX_TABLE_CAPACITY, 0),
            (SETTINGS_MAX_FIELD_SECTION_SIZE, 65_536),
            // a reserved identifier and 0, reserved but not HTTP/2's
            (0x21, 7),
            (0x00, 1),
        ] {
            put_varint(&mut payload, id);
            put_varint(&mut payload, value);
        }
        let settings = Settings::decode(&payload).unwrap();
        assert_eq!(
            settings,
            Settings(vec![(0x01, 0), (0x06, 65_536), (0x21, 7), (0x00, 1)])
        );
        assert_eq!(settings.get(SETTINGS_MAX_FIELD_SECTION_SIZE), Some(65_536));
        assert_eq!(settings.get(SETTINGS_ENABLE_CONNECT_PROTOCOL), None);
        assert_eq!(Settings::decode(&[]).unwrap(), Settings::default());
    }

    #[test]
    fn bad_settings_are_errors_of_the_connection() {
        for (payload, code) in [
            // HTTP/2's ENABLE_PUSH and MAX_FRAME_SIZE
            (&[0x02, 0x00][..], H3_SETTINGS_ERROR),
            (&[0x05, 0x01], H3_SETTINGS_ERROR),
            (&[0x06, 0x01, 0x06, 0x02], H3_SETTINGS_ERROR),
            (&[0x06], H3_FRAME_ERROR),
            (&[0x06, 0x40], H3_FRAME_ERROR),
        ] {
            assert_eq!(
                Settings::decode(payload).unwrap_err().code,
                code,
                "{payload:02x?}"
            );
        }
    }
}
```

新建 `crates/rurge-net/src/quic/h3/huffman.rs`：

```rust
//! The Huffman code of HPACK (RFC 7541 Appendix B), which QPACK string
//! literals may use (RFC 9204 4.1.2). The client decodes it and never
//! encodes with it.

/// The code of each octet and then of EOS, as (code, length in bits),
/// transcribed from RFC 7541 Appendix B. The code is complete and
/// canonical: the codes of one length follow each other in octet order,
/// and those of the next length start right after them. `decode` relies on
/// it; a test checks it.
pub(crate) const CODES: [(u32, u8); 257] = [
    (0x1ff8, 13),     // (0)
    (0x7fffd8, 23),   // (1)
    (0xfffffe2, 28),  // (2)
    (0xfffffe3, 28),  // (3)
    (0xfffffe4, 28),  // (4)
    (0xfffffe5, 28),  // (5)
    (0xfffffe6, 28),  // (6)
    (0xfffffe7, 28),  // (7)
    (0xfffffe8, 28),  // (8)
    (0xffffea, 24),   // (9)
    (0x3ffffffc, 30), // (10)
    (0xfffffe9, 28),  // (11)
    (0xfffffea, 28),  // (12)
    (0x3ffffffd, 30), // (13)
    (0xfffffeb, 28),  // (14)
    (0xfffffec, 28),  // (15)
    (0xfffffed, 28),  // (16)
    (0xfffffee, 28),  // (17)
    (0xfffffef, 28),  // (18)
    (0xffffff0, 28),  // (19)
    (0xffffff1, 28),  // (20)
    (0xffffff2, 28),  // (21)
    (0x3ffffffe, 30), // (22)
    (0xffffff3, 28),  // (23)
    (0xffffff4, 28),  // (24)
    (0xffffff5, 28),  // (25)
    (0xffffff6, 28),  // (26)
    (0xffffff7, 28),  // (27)
    (0xffffff8, 28),  // (28)
    (0xffffff9, 28),  // (29)
    (0xffffffa, 28),  // (30)
    (0xffffffb, 28),  // (31)
    (0x14, 6),        // ' ' (32)
    (0x3f8, 10),      // '!' (33)
    (0x3f9, 10),      // '"' (34)
    (0xffa, 12),      // '#' (35)
    (0x1ff9, 13),     // '$' (36)
    (0x15, 6),        // '%' (37)
    (0xf8, 8),        // '&' (38)
    (0x7fa, 11),      // ''' (39)
    (0x3fa, 10),      // '(' (40)
    (0x3fb, 10),      // ')' (41)
    (0xf9, 8),        // '*' (42)
    (0x7fb, 11),      // '+' (43)
    (0xfa, 8),        // ',' (44)
    (0x16, 6),        // '-' (45)
    (0x17, 6),        // '.' (46)
    (0x18, 6),        // '/' (47)
    (0x0, 5),         // '0' (48)
    (0x1, 5),         // '1' (49)
    (0x2, 5),         // '2' (50)
    (0x19, 6),        // '3' (51)
    (0x1a, 6),        // '4' (52)
    (0x1b, 6),        // '5' (53)
    (0x1c, 6),        // '6' (54)
    (0x1d, 6),        // '7' (55)
    (0x1e, 6),        // '8' (56)
    (0x1f, 6),        // '9' (57)
    (0x5c, 7),        // ':' (58)
    (0xfb, 8),        // ';' (59)
    (0x7ffc, 15),     // '<' (60)
    (0x20, 6),        // '=' (61)
    (0xffb, 12),      // '>' (62)
    (0x3fc, 10),      // '?' (63)
    (0x1ffa, 13),     // '@' (64)
    (0x21, 6),        // 'A' (65)
    (0x5d, 7),        // 'B' (66)
    (0x5e, 7),        // 'C' (67)
    (0x5f, 7),        // 'D' (68)
    (0x60, 7),        // 'E' (69)
    (0x61, 7),        // 'F' (70)
    (0x62, 7),        // 'G' (71)
    (0x63, 7),        // 'H' (72)
    (0x64, 7),        // 'I' (73)
    (0x65, 7),        // 'J' (74)
    (0x66, 7),        // 'K' (75)
    (0x67, 7),        // 'L' (76)
    (0x68, 7),        // 'M' (77)
    (0x69, 7),        // 'N' (78)
    (0x6a, 7),        // 'O' (79)
    (0x6b, 7),        // 'P' (80)
    (0x6c, 7),        // 'Q' (81)
    (0x6d, 7),        // 'R' (82)
    (0x6e, 7),        // 'S' (83)
    (0x6f, 7),        // 'T' (84)
    (0x70, 7),        // 'U' (85)
    (0x71, 7),        // 'V' (86)
    (0x72, 7),        // 'W' (87)
    (0xfc, 8),        // 'X' (88)
    (0x73, 7),        // 'Y' (89)
    (0xfd, 8),        // 'Z' (90)
    (0x1ffb, 13),     // '[' (91)
    (0x7fff0, 19),    // '\' (92)
    (0x1ffc, 13),     // ']' (93)
    (0x3ffc, 14),     // '^' (94)
    (0x22, 6),        // '_' (95)
    (0x7ffd, 15),     // '`' (96)
    (0x3, 5),         // 'a' (97)
    (0x23, 6),        // 'b' (98)
    (0x4, 5),         // 'c' (99)
    (0x24, 6),        // 'd' (100)
    (0x5, 5),         // 'e' (101)
    (0x25, 6),        // 'f' (102)
    (0x26, 6),        // 'g' (103)
    (0x27, 6),        // 'h' (104)
    (0x6, 5),         // 'i' (105)
    (0x74, 7),        // 'j' (106)
    (0x75, 7),        // 'k' (107)
    (0x28, 6),        // 'l' (108)
    (0x29, 6),        // 'm' (109)
    (0x2a, 6),        // 'n' (110)
    (0x7, 5),         // 'o' (111)
    (0x2b, 6),        // 'p' (112)
    (0x76, 7),        // 'q' (113)
    (0x2c, 6),        // 'r' (114)
    (0x8, 5),         // 's' (115)
    (0x9, 5),         // 't' (116)
    (0x2d, 6),        // 'u' (117)
    (0x77, 7),        // 'v' (118)
    (0x78, 7),        // 'w' (119)
    (0x79, 7),        // 'x' (120)
    (0x7a, 7),        // 'y' (121)
    (0x7b, 7),        // 'z' (122)
    (0x7ffe, 15),     // '{' (123)
    (0x7fc, 11),      // '|' (124)
    (0x3ffd, 14),     // '}' (125)
    (0x1ffd, 13),     // '~' (126)
    (0xffffffc, 28),  // (127)
    (0xfffe6, 20),    // (128)
    (0x3fffd2, 22),   // (129)
    (0xfffe7, 20),    // (130)
    (0xfffe8, 20),    // (131)
    (0x3fffd3, 22),   // (132)
    (0x3fffd4, 22),   // (133)
    (0x3fffd5, 22),   // (134)
    (0x7fffd9, 23),   // (135)
    (0x3fffd6, 22),   // (136)
    (0x7fffda, 23),   // (137)
    (0x7fffdb, 23),   // (138)
    (0x7fffdc, 23),   // (139)
    (0x7fffdd, 23),   // (140)
    (0x7fffde, 23),   // (141)
    (0xffffeb, 24),   // (142)
    (0x7fffdf, 23),   // (143)
    (0xffffec, 24),   // (144)
    (0xffffed, 24),   // (145)
    (0x3fffd7, 22),   // (146)
    (0x7fffe0, 23),   // (147)
    (0xffffee, 24),   // (148)
    (0x7fffe1, 23),   // (149)
    (0x7fffe2, 23),   // (150)
    (0x7fffe3, 23),   // (151)
    (0x7fffe4, 23),   // (152)
    (0x1fffdc, 21),   // (153)
    (0x3fffd8, 22),   // (154)
    (0x7fffe5, 23),   // (155)
    (0x3fffd9, 22),   // (156)
    (0x7fffe6, 23),   // (157)
    (0x7fffe7, 23),   // (158)
    (0xffffef, 24),   // (159)
    (0x3fffda, 22),   // (160)
    (0x1fffdd, 21),   // (161)
    (0xfffe9, 20),    // (162)
    (0x3fffdb, 22),   // (163)
    (0x3fffdc, 22),   // (164)
    (0x7fffe8, 23),   // (165)
    (0x7fffe9, 23),   // (166)
    (0x1fffde, 21),   // (167)
    (0x7fffea, 23),   // (168)
    (0x3fffdd, 22),   // (169)
    (0x3fffde, 22),   // (170)
    (0xfffff0, 24),   // (171)
    (0x1fffdf, 21),   // (172)
    (0x3fffdf, 22),   // (173)
    (0x7fffeb, 23),   // (174)
    (0x7fffec, 23),   // (175)
    (0x1fffe0, 21),   // (176)
    (0x1fffe1, 21),   // (177)
    (0x3fffe0, 22),   // (178)
    (0x1fffe2, 21),   // (179)
    (0x7fffed, 23),   // (180)
    (0x3fffe1, 22),   // (181)
    (0x7fffee, 23),   // (182)
    (0x7fffef, 23),   // (183)
    (0xfffea, 20),    // (184)
    (0x3fffe2, 22),   // (185)
    (0x3fffe3, 22),   // (186)
    (0x3fffe4, 22),   // (187)
    (0x7ffff0, 23),   // (188)
    (0x3fffe5, 22),   // (189)
    (0x3fffe6, 22),   // (190)
    (0x7ffff1, 23),   // (191)
    (0x3ffffe0, 26),  // (192)
    (0x3ffffe1, 26),  // (193)
    (0xfffeb, 20),    // (194)
    (0x7fff1, 19),    // (195)
    (0x3fffe7, 22),   // (196)
    (0x7ffff2, 23),   // (197)
    (0x3fffe8, 22),   // (198)
    (0x1ffffec, 25),  // (199)
    (0x3ffffe2, 26),  // (200)
    (0x3ffffe3, 26),  // (201)
    (0x3ffffe4, 26),  // (202)
    (0x7ffffde, 27),  // (203)
    (0x7ffffdf, 27),  // (204)
    (0x3ffffe5, 26),  // (205)
    (0xfffff1, 24),   // (206)
    (0x1ffffed, 25),  // (207)
    (0x7fff2, 19),    // (208)
    (0x1fffe3, 21),   // (209)
    (0x3ffffe6, 26),  // (210)
    (0x7ffffe0, 27),  // (211)
    (0x7ffffe1, 27),  // (212)
    (0x3ffffe7, 26),  // (213)
    (0x7ffffe2, 27),  // (214)
    (0xfffff2, 24),   // (215)
    (0x1fffe4, 21),   // (216)
    (0x1fffe5, 21),   // (217)
    (0x3ffffe8, 26),  // (218)
    (0x3ffffe9, 26),  // (219)
    (0xffffffd, 28),  // (220)
    (0x7ffffe3, 27),  // (221)
    (0x7ffffe4, 27),  // (222)
    (0x7ffffe5, 27),  // (223)
    (0xfffec, 20),    // (224)
    (0xfffff3, 24),   // (225)
    (0xfffed, 20),    // (226)
    (0x1fffe6, 21),   // (227)
    (0x3fffe9, 22),   // (228)
    (0x1fffe7, 21),   // (229)
    (0x1fffe8, 21),   // (230)
    (0x7ffff3, 23),   // (231)
    (0x3fffea, 22),   // (232)
    (0x3fffeb, 22),   // (233)
    (0x1ffffee, 25),  // (234)
    (0x1ffffef, 25),  // (235)
    (0xfffff4, 24),   // (236)
    (0xfffff5, 24),   // (237)
    (0x3ffffea, 26),  // (238)
    (0x7ffff4, 23),   // (239)
    (0x3ffffeb, 26),  // (240)
    (0x7ffffe6, 27),  // (241)
    (0x3ffffec, 26),  // (242)
    (0x3ffffed, 26),  // (243)
    (0x7ffffe7, 27),  // (244)
    (0x7ffffe8, 27),  // (245)
    (0x7ffffe9, 27),  // (246)
    (0x7ffffea, 27),  // (247)
    (0x7ffffeb, 27),  // (248)
    (0xffffffe, 28),  // (249)
    (0x7ffffec, 27),  // (250)
    (0x7ffffed, 27),  // (251)
    (0x7ffffee, 27),  // (252)
    (0x7ffffef, 27),  // (253)
    (0x7fffff0, 27),  // (254)
    (0x3ffffee, 26),  // (255)
    (0x3fffffff, 30), // EOS (256)
];

/// The longest code: EOS's, and those of three control octets.
const LONGEST: usize = 30;

/// The canonical code as decoding walks it: for each length, the first
/// code of that length, how many codes have it, and where their symbols
/// start in `symbols` — every symbol, by length and then octet.
struct Canonical {
    first: [u32; LONGEST + 1],
    count: [u16; LONGEST + 1],
    start: [u16; LONGEST + 1],
    symbols: [u16; 257],
}

const CANONICAL: Canonical = canonical();

const fn canonical() -> Canonical {
    let mut count = [0u16; LONGEST + 1];
    let mut symbol = 0;
    while symbol < CODES.len() {
        count[CODES[symbol].1 as usize] += 1;
        symbol += 1;
    }
    let mut first = [0u32; LONGEST + 1];
    let mut start = [0u16; LONGEST + 1];
    let (mut code, mut at) = (0u32, 0u16);
    let mut len = 1;
    while len <= LONGEST {
        code = (code + count[len - 1] as u32) << 1;
        first[len] = code;
        start[len] = at;
        at += count[len];
        len += 1;
    }
    let mut symbols = [0u16; 257];
    let mut next = start;
    let mut symbol = 0;
    while symbol < CODES.len() {
        let len = CODES[symbol].1 as usize;
        symbols[next[len] as usize] = symbol as u16;
        next[len] += 1;
        symbol += 1;
    }
    Canonical {
        first,
        count,
        start,
        symbols,
    }
}

/// `src` decoded; `None` for what no encoder makes (RFC 7541 5.2): EOS
/// among the symbols, or padding longer than 7 bits or other than the
/// first bits of EOS (all ones).
pub(crate) fn decode(src: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(src.len() * 8 / 5);
    // the bits of a code not yet complete
    let (mut code, mut len) = (0u32, 0usize);
    for &byte in src {
        for bit in (0..8).rev() {
            code = (code << 1) | u32::from((byte >> bit) & 1);
            len += 1;
            let at = code.wrapping_sub(CANONICAL.first[len]);
            if at < u32::from(CANONICAL.count[len]) {
                let symbol = CANONICAL.symbols[usize::from(CANONICAL.start[len]) + at as usize];
                // EOS, the one symbol past the octets, is not data
                out.push(u8::try_from(symbol).ok()?);
                (code, len) = (0, 0);
            } else if len == LONGEST {
                return None;
            }
        }
    }
    (len <= 7 && code == (1 << len) - 1).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quic::h3::testing::huffman_encode;

    fn hex(text: &str) -> Vec<u8> {
        let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        digits
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    /// The codes fill the code space exactly (Kraft), and each is where
    /// the canonical order of (length, octet) puts it.
    #[test]
    fn the_code_is_complete_and_canonical() {
        let kraft: u64 = CODES
            .iter()
            .map(|&(_, len)| 1u64 << (LONGEST - usize::from(len)))
            .sum();
        assert_eq!(kraft, 1 << LONGEST);
        let mut order: Vec<usize> = (0..CODES.len()).collect();
        order.sort_by_key(|&symbol| (CODES[symbol].1, symbol));
        let (mut code, mut len) = (0u32, CODES[order[0]].1);
        for (n, &symbol) in order.iter().enumerate() {
            if n > 0 {
                code = (code + 1) << (CODES[symbol].1 - len);
                len = CODES[symbol].1;
            }
            assert_eq!(CODES[symbol], (code, len), "symbol {symbol}");
        }
        assert_eq!(CODES[256], (0x3fff_ffff, 30), "EOS");
    }

    /// The Huffman-encoded strings of the examples of RFC 7541 Appendix C.4
    /// (requests) and C.6 (responses).
    #[test]
    fn the_strings_of_rfc7541_c4_and_c6() {
        for (encoded, text) in [
            ("f1e3 c2e5 f23a 6ba0 ab90 f4ff", "www.example.com"),
            ("a8eb 1064 9cbf", "no-cache"),
            ("25a8 49e9 5ba9 7d7f", "custom-key"),
            ("25a8 49e9 5bb8 e8b4 bf", "custom-value"),
            ("6402", "302"),
            ("aec3 771a 4b", "private"),
            (
                "d07a be94 1054 d444 a820 0595 040b 8166 e082 a62d 1bff",
                "Mon, 21 Oct 2013 20:13:21 GMT",
            ),
            (
                "9d29 ad17 1863 c78f 0b97 c8e9 ae82 ae43 d3",
                "https://www.example.com",
            ),
            ("640e ff", "307"),
            (
                "d07a be94 1054 d444 a820 0595 040b 8166 e084 a62d 1bff",
                "Mon, 21 Oct 2013 20:13:22 GMT",
            ),
            ("9bd9 ab", "gzip"),
            (
                "94e7 821d d7f2 e6c7 b335 dfdf cd5b 3960 d5af 2708 7f36 72c1 \
                 ab27 0fb5 291f 9587 3160 65c0 03ed 4ee5 b106 3d50 07",
                "foo=ASDJKHQKBZXOQWEOPIUAXQWEOIU; max-age=3600; version=1",
            ),
        ] {
            assert_eq!(
                decode(&hex(encoded)).as_deref(),
                Some(text.as_bytes()),
                "{text}"
            );
        }
    }

    /// Every octet, the longest codes among them, and nothing at all.
    #[test]
    fn every_octet_decodes() {
        let octets: Vec<u8> = (0..=u8::MAX).collect();
        assert_eq!(decode(&huffman_encode(&octets)), Some(octets));
        assert_eq!(decode(&[]), Some(Vec::new()));
    }

    #[test]
    fn padding_longer_than_7_bits_is_refused() {
        // '0' (00000) and three bits of padding
        assert_eq!(decode(&[0x07]), Some(b"0".to_vec()));
        // '0' and eleven bits of padding
        assert_eq!(decode(&[0x07, 0xff]), None);
        // eight bits of padding and no symbol
        assert_eq!(decode(&[0xff]), None);
    }

    #[test]
    fn padding_is_the_start_of_eos() {
        // '0' (00000), then 000 and 110
        assert_eq!(decode(&[0x00]), None);
        assert_eq!(decode(&[0x06]), None);
    }

    #[test]
    fn an_encoded_eos_is_refused() {
        // EOS, then two bits of padding
        assert_eq!(decode(&[0xff; 4]), None);
        // 'a' (00011), then EOS and its padding
        assert_eq!(decode(&[0x1f, 0xff, 0xff, 0xff, 0xff]), None);
    }
}
```

新建 `crates/rurge-net/src/quic/h3/qpack.rs`：

```rust
//! QPACK with the static table alone (RFC 9204): field sections as this
//! client sends them and as it takes them. Our SETTINGS give the server's
//! encoder no dynamic table, so a field section that needs one — a
//! Required Insert Count, a reference to an entry — cannot be decoded.

use super::frame::VARINT_MAX;
use super::huffman;

/// The static table (RFC 9204 Appendix A).
pub(crate) const STATIC_TABLE: [(&str, &str); 99] = [
    (":authority", ""),
    (":path", "/"),
    ("age", "0"),
    ("content-disposition", ""),
    ("content-length", "0"),
    ("cookie", ""),
    ("date", ""),
    ("etag", ""),
    ("if-modified-since", ""),
    ("if-none-match", ""),
    ("last-modified", ""),
    ("link", ""),
    ("location", ""),
    ("referer", ""),
    ("set-cookie", ""),
    (":method", "CONNECT"),
    (":method", "DELETE"),
    (":method", "GET"),
    (":method", "HEAD"),
    (":method", "OPTIONS"),
    (":method", "POST"),
    (":method", "PUT"),
    (":scheme", "http"),
    (":scheme", "https"),
    (":status", "103"),
    (":status", "200"),
    (":status", "304"),
    (":status", "404"),
    (":status", "503"),
    ("accept", "*/*"),
    ("accept", "application/dns-message"),
    ("accept-encoding", "gzip, deflate, br"),
    ("accept-ranges", "bytes"),
    ("access-control-allow-headers", "cache-control"),
    ("access-control-allow-headers", "content-type"),
    ("access-control-allow-origin", "*"),
    ("cache-control", "max-age=0"),
    ("cache-control", "max-age=2592000"),
    ("cache-control", "max-age=604800"),
    ("cache-control", "no-cache"),
    ("cache-control", "no-store"),
    ("cache-control", "public, max-age=31536000"),
    ("content-encoding", "br"),
    ("content-encoding", "gzip"),
    ("content-type", "application/dns-message"),
    ("content-type", "application/javascript"),
    ("content-type", "application/json"),
    ("content-type", "application/x-www-form-urlencoded"),
    ("content-type", "image/gif"),
    ("content-type", "image/jpeg"),
    ("content-type", "image/png"),
    ("content-type", "text/css"),
    ("content-type", "text/html; charset=utf-8"),
    ("content-type", "text/plain"),
    ("content-type", "text/plain;charset=utf-8"),
    ("range", "bytes=0-"),
    ("strict-transport-security", "max-age=31536000"),
    (
        "strict-transport-security",
        "max-age=31536000; includesubdomains",
    ),
    (
        "strict-transport-security",
        "max-age=31536000; includesubdomains; preload",
    ),
    ("vary", "accept-encoding"),
    ("vary", "origin"),
    ("x-content-type-options", "nosniff"),
    ("x-xss-protection", "1; mode=block"),
    (":status", "100"),
    (":status", "204"),
    (":status", "206"),
    (":status", "302"),
    (":status", "400"),
    (":status", "403"),
    (":status", "421"),
    (":status", "425"),
    (":status", "500"),
    ("accept-language", ""),
    ("access-control-allow-credentials", "FALSE"),
    ("access-control-allow-credentials", "TRUE"),
    ("access-control-allow-headers", "*"),
    ("access-control-allow-methods", "get"),
    ("access-control-allow-methods", "get, post, options"),
    ("access-control-allow-methods", "options"),
    ("access-control-expose-headers", "content-length"),
    ("access-control-request-headers", "content-type"),
    ("access-control-request-method", "get"),
    ("access-control-request-method", "post"),
    ("alt-svc", "clear"),
    ("authorization", ""),
    (
        "content-security-policy",
        "script-src 'none'; object-src 'none'; base-uri 'none'",
    ),
    ("early-data", "1"),
    ("expect-ct", ""),
    ("forwarded", ""),
    ("if-range", ""),
    ("origin", ""),
    ("purpose", "prefetch"),
    ("server", ""),
    ("timing-allow-origin", "*"),
    ("upgrade-insecure-requests", "1"),
    ("user-agent", ""),
    ("x-forwarded-for", ""),
    ("x-frame-options", "deny"),
    ("x-frame-options", "sameorigin"),
];

/// A field line: its name and its value.
pub(crate) type Field = (Vec<u8>, Vec<u8>);

/// Why a field section was not taken.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// It needs a dynamic table, refers past the static table, or is cut
    /// short, too long or badly Huffman-encoded somewhere:
    /// QPACK_DECOMPRESSION_FAILED, an error of the connection (RFC 9204
    /// 4.5).
    Invalid,
    /// Its size is over the limit (RFC 9114 4.2.2).
    TooLarge,
}

/// The fields of `section`, whose size — the names and values plus 32
/// bytes per field — must not pass `limit`.
pub(crate) fn decode(mut section: &[u8], limit: u64) -> Result<Vec<Field>, Refused> {
    use Refused::Invalid;
    // without a dynamic table the Required Insert Count is 0 (RFC 9204
    // 4.5.1.1); a Sign bit of 1 would then make the Base negative
    // (4.5.1.2); any other Base serves static references alike
    let (_, required) = get_int(&mut section, 8)?;
    let (sign, _) = get_int(&mut section, 7)?;
    if required != 0 || sign != 0 {
        return Err(Invalid);
    }
    let mut fields = Vec::new();
    let mut size = 0u64;
    while let Some(&first) = section.first() {
        let field = if first & 0x80 != 0 {
            // indexed field line: 1 T index(6+), T = 1 for the static table
            let (flags, index) = get_int(&mut section, 6)?;
            if flags & 0x40 == 0 {
                return Err(Invalid);
            }
            let (name, value) = entry(index)?;
            (name.as_bytes().to_vec(), value.as_bytes().to_vec())
        } else if first & 0x40 != 0 {
            // literal field line with name reference: 01 N T index(4+)
            let (flags, index) = get_int(&mut section, 4)?;
            if flags & 0x10 == 0 {
                return Err(Invalid);
            }
            let (name, _) = entry(index)?;
            (name.as_bytes().to_vec(), get_string(&mut section, 7)?)
        } else if first & 0x20 != 0 {
            // literal field line with literal name: 001 N H length(3+)
            let name = get_string(&mut section, 3)?;
            (name, get_string(&mut section, 7)?)
        } else {
            // 0001: indexed field line with post-base index; 0000: literal
            // field line with post-base name reference — both entries of
            // the dynamic table
            return Err(Invalid);
        };
        size += (field.0.len() + field.1.len()) as u64 + 32;
        if size > limit {
            return Err(Refused::TooLarge);
        }
        fields.push(field);
    }
    Ok(fields)
}

/// The field section of `fields` — (name, value, sensitive), in order —
/// with the static table alone: Required Insert Count 0 and Base 0 (RFC
/// 9204 4.5.1), then each field as its exact entry, else as a literal
/// value with the entry of its name, else as a literal name and value. No
/// string is Huffman-encoded. A sensitive value is always a literal, marked
/// never to be indexed (RFC 9204 7.1.3).
pub(crate) fn encode<'a>(fields: impl IntoIterator<Item = (&'a [u8], &'a [u8], bool)>) -> Vec<u8> {
    let mut out = vec![0, 0];
    for (name, value, sensitive) in fields {
        let exact = STATIC_TABLE
            .iter()
            .position(|&(n, v)| n.as_bytes() == name && v.as_bytes() == value);
        let named = STATIC_TABLE.iter().position(|&(n, _)| n.as_bytes() == name);
        match (exact, named) {
            // 1 T=1 index(6+)
            (Some(index), _) if !sensitive => put_int(&mut out, 0xc0, 6, index as u64),
            // 01 N T=1 index(4+), then the value
            (_, Some(index)) => {
                let never = if sensitive { 0x20 } else { 0 };
                put_int(&mut out, 0x50 | never, 4, index as u64);
                put_string(&mut out, 0, 7, value);
            }
            // 001 N H=0 length(3+) and the name, then the value
            _ => {
                let never = if sensitive { 0x10 } else { 0 };
                put_string(&mut out, 0x20 | never, 3, name);
                put_string(&mut out, 0, 7, value);
            }
        }
    }
    out
}

/// The entry at `index` of the static table.
fn entry(index: u64) -> Result<(&'static str, &'static str), Refused> {
    usize::try_from(index)
        .ok()
        .and_then(|index| STATIC_TABLE.get(index))
        .copied()
        .ok_or(Refused::Invalid)
}

/// `value` as an integer with an `n`-bit prefix (RFC 7541 5.1), `flags`
/// in the bits of the first byte above it.
fn put_int(out: &mut Vec<u8>, flags: u8, n: u32, value: u64) {
    let max = (1u64 << n) - 1;
    if value < max {
        out.push(flags | value as u8);
        return;
    }
    out.push(flags | max as u8);
    let mut rest = value - max;
    while rest >= 0x80 {
        out.push(0x80 | (rest & 0x7f) as u8);
        rest >>= 7;
    }
    out.push(rest as u8);
}

/// `bytes` as a string literal (RFC 9204 4.1.2) whose length has an
/// `n`-bit prefix, `flags` above it — and H, the bit right above the
/// prefix, never set.
fn put_string(out: &mut Vec<u8>, flags: u8, n: u32, bytes: &[u8]) {
    put_int(out, flags, n, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

/// The integer with an `n`-bit prefix at the start of `src`, and the bits
/// of its first byte above the prefix. QPACK's integers have at most 62
/// bits (RFC 9204 4.1.1).
fn get_int(src: &mut &[u8], n: u32) -> Result<(u8, u64), Refused> {
    let (&first, rest) = src.split_first().ok_or(Refused::Invalid)?;
    *src = rest;
    let max = (1u64 << n) - 1;
    let mut value = u64::from(first) & max;
    if value == max {
        // seven bits per byte, the least significant first: nine bytes
        // reach past 62 bits, and their sum still fits 64
        let mut shift = 0;
        loop {
            let (&byte, rest) = src.split_first().ok_or(Refused::Invalid)?;
            *src = rest;
            value += u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift > 56 {
                return Err(Refused::Invalid);
            }
        }
        if value > VARINT_MAX {
            return Err(Refused::Invalid);
        }
    }
    Ok((first & !(max as u8), value))
}

/// The string literal at the start of `src`: its length has an `n`-bit
/// prefix, and the bit right above it says whether it is
/// Huffman-encoded.
fn get_string(src: &mut &[u8], n: u32) -> Result<Vec<u8>, Refused> {
    let (flags, len) = get_int(src, n)?;
    let len = usize::try_from(len).map_err(|_| Refused::Invalid)?;
    if len > src.len() {
        return Err(Refused::Invalid);
    }
    let (bytes, rest) = src.split_at(len);
    *src = rest;
    if flags & (1 << n) != 0 {
        huffman::decode(bytes).ok_or(Refused::Invalid)
    } else {
        Ok(bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(list: &[(&str, &str)]) -> Vec<Field> {
        list.iter()
            .map(|(n, v)| (n.as_bytes().to_vec(), v.as_bytes().to_vec()))
            .collect()
    }

    /// The examples of RFC 7541 C.1, which QPACK's integers are.
    #[test]
    fn prefixed_integers_of_rfc7541_c1() {
        for (n, value, bytes) in [
            // C.1.1: 10 with a 5-bit prefix
            (5, 10, &[0x0a][..]),
            // C.1.2: 1337 with a 5-bit prefix
            (5, 1337, &[0x1f, 0x9a, 0x0a]),
            // C.1.3: 42 starting at an octet boundary
            (8, 42, &[0x2a]),
        ] {
            let mut out = Vec::new();
            put_int(&mut out, 0, n, value);
            assert_eq!(out, bytes, "{value}");
            let mut src = bytes;
            assert_eq!(get_int(&mut src, n), Ok((0, value)));
            assert!(src.is_empty());
        }
        // the bits above the prefix are the caller's
        let mut out = Vec::new();
        put_int(&mut out, 0xe0, 5, 1337);
        assert_eq!(out, [0xff, 0x9a, 0x0a]);
        assert_eq!(get_int(&mut &out[..], 5), Ok((0xe0, 1337)));
        // the largest QPACK integer
        let mut out = Vec::new();
        put_int(&mut out, 0, 6, VARINT_MAX);
        assert_eq!(get_int(&mut &out[..], 6), Ok((0, VARINT_MAX)));
    }

    #[test]
    fn integers_cut_short_or_too_long_are_refused() {
        // nothing, a full prefix with nothing after, a byte that says more
        // comes
        for bytes in [&[][..], &[0x1f], &[0x1f, 0x80]] {
            assert_eq!(
                get_int(&mut &bytes[..], 5),
                Err(Refused::Invalid),
                "{bytes:?}"
            );
        }
        // 2^62 does not fit
        let mut out = Vec::new();
        put_int(&mut out, 0, 6, VARINT_MAX + 1);
        assert_eq!(get_int(&mut &out[..], 6), Err(Refused::Invalid));
        // ten more bytes reach past 64 bits
        let long = [
            0x3f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01,
        ];
        assert_eq!(get_int(&mut &long[..], 6), Err(Refused::Invalid));
    }

    /// The entries whose whitespace the RFC's text wraps, checked against
    /// its HTML table: a space after the semicolon in 52, 57 and 58, none
    /// in 54.
    #[test]
    fn the_static_table_is_rfc9204_appendix_a() {
        assert_eq!(STATIC_TABLE[0], (":authority", ""));
        assert_eq!(STATIC_TABLE[25], (":status", "200"));
        assert_eq!(STATIC_TABLE[31], ("accept-encoding", "gzip, deflate, br"));
        assert_eq!(
            STATIC_TABLE[52],
            ("content-type", "text/html; charset=utf-8")
        );
        assert_eq!(
            STATIC_TABLE[54],
            ("content-type", "text/plain;charset=utf-8")
        );
        assert_eq!(
            STATIC_TABLE[58],
            (
                "strict-transport-security",
                "max-age=31536000; includesubdomains; preload"
            )
        );
        assert_eq!(STATIC_TABLE[72], ("accept-language", ""));
        assert_eq!(STATIC_TABLE[98], ("x-frame-options", "sameorigin"));
        assert!(
            STATIC_TABLE
                .iter()
                .all(|(name, _)| !name.contains(|c: char| c.is_ascii_uppercase()))
        );
    }

    /// RFC 9204 B.1: a literal value with the name of a static entry.
    #[test]
    fn rfc9204_b1_name_reference() {
        let section = [
            0x00, 0x00, 0x51, 0x0b, 0x2f, 0x69, 0x6e, 0x64, 0x65, 0x78, 0x2e, 0x68, 0x74, 0x6d,
            0x6c,
        ];
        assert_eq!(
            decode(&section, 1 << 16),
            Ok(fields(&[(":path", "/index.html")]))
        );
        assert_eq!(
            encode([(&b":path"[..], &b"/index.html"[..], false)]),
            section
        );
    }

    /// Hysteria 2's authentication head: exact entries, names with literal
    /// values (an index past the prefix), literal names (a length past the
    /// prefix), and the never-indexed secret.
    #[test]
    fn requests_use_exact_entries_then_names_then_literals() {
        let section = encode([
            (&b":method"[..], &b"POST"[..], false),
            (b":scheme", b"https", false),
            (b":authority", b"hysteria", false),
            (b":path", b"/auth", false),
            (b"hysteria-auth", b"pw", true),
            (b"hysteria-cc-rx", b"0", false),
            (b"content-type", b"application/dns-message", false),
            (b"user-agent", b"rurge", false),
        ]);
        let mut expected = vec![0x00, 0x00, 0xd4, 0xd7, 0x50, 0x08];
        expected.extend_from_slice(b"hysteria");
        expected.extend_from_slice(&[0x51, 0x05]);
        expected.extend_from_slice(b"/auth");
        expected.extend_from_slice(&[0x37, 0x06]);
        expected.extend_from_slice(b"hysteria-auth");
        expected.extend_from_slice(&[0x02]);
        expected.extend_from_slice(b"pw");
        expected.extend_from_slice(&[0x27, 0x07]);
        expected.extend_from_slice(b"hysteria-cc-rx");
        expected.extend_from_slice(&[0x01, b'0', 0xec, 0x5f, 0x50, 0x05]);
        expected.extend_from_slice(b"rurge");
        assert_eq!(section, expected);
    }

    /// A sensitive value never takes an exact entry and is marked never to
    /// be indexed, with the name of an entry (the first of that name) or
    /// with its own.
    #[test]
    fn a_sensitive_value_is_a_never_indexed_literal() {
        let section = encode([
            (&b":method"[..], &b"GET"[..], true),
            (b"authorization", b"secret", true),
            (b"x-secret", b"s", true),
        ]);
        let mut expected = vec![0x00, 0x00, 0x7f, 0x00, 0x03];
        expected.extend_from_slice(b"GET");
        expected.extend_from_slice(&[0x7f, 0x45, 0x06]);
        expected.extend_from_slice(b"secret");
        expected.extend_from_slice(&[0x37, 0x01]);
        expected.extend_from_slice(b"x-secret");
        expected.extend_from_slice(&[0x01, b's']);
        assert_eq!(section, expected);
    }

    /// Every form that needs no dynamic table: exact entries, literals with
    /// a static name or their own, Huffman-encoded or not, never-indexed or
    /// not, and any Base.
    #[test]
    fn every_static_form_decodes() {
        let mut section = vec![0x00, 0x05, 0xd9];
        // custom-key: custom-value, both Huffman-encoded (RFC 7541 C.4.3)
        section.extend_from_slice(&[0x2f, 0x01, 0x25, 0xa8, 0x49, 0xe9, 0x5b, 0xa9, 0x7d, 0x7f]);
        section.extend_from_slice(&[0x89, 0x25, 0xa8, 0x49, 0xe9, 0x5b, 0xb8, 0xe8, 0xb4, 0xbf]);
        // cache-control (36) with "private" Huffman-encoded (RFC 7541 C.6.1)
        section.extend_from_slice(&[0x5f, 0x15, 0x85, 0xae, 0xc3, 0x77, 0x1a, 0x4b]);
        // a never-indexed literal name and value
        section.extend_from_slice(&[0x33, b'x', b'-', b'n', 0x01, b'1']);
        // a never-indexed value with a static name, x-frame-options (97)
        section.extend_from_slice(&[0x7f, 0x52, 0x02, b'n', b'o']);
        assert_eq!(
            decode(&section, 1 << 16),
            Ok(fields(&[
                (":status", "200"),
                ("custom-key", "custom-value"),
                ("cache-control", "private"),
                ("x-n", "1"),
                ("x-frame-options", "no"),
            ]))
        );
    }

    /// RFC 9204 B.2 and B.4 refer to their dynamic table, and so does each
    /// form with T = 0 or a post-base index.
    #[test]
    fn dynamic_references_are_refused() {
        for section in [
            // B.2, stream 4: Required Insert Count 2, post-base indexes
            &[0x03, 0x81, 0x10, 0x11][..],
            // B.4, stream 8: Required Insert Count 4, dynamic entries
            &[0x05, 0x00, 0x80, 0xc1, 0x81],
            // indexed, T = 0
            &[0x00, 0x00, 0x80],
            // indexed with post-base index
            &[0x00, 0x00, 0x10],
            // literal with a dynamic name reference
            &[0x00, 0x00, 0x40, 0x00],
            // literal with post-base name reference
            &[0x00, 0x00, 0x00, 0x00],
        ] {
            assert_eq!(
                decode(section, 1 << 16),
                Err(Refused::Invalid),
                "{section:02x?}"
            );
        }
    }

    #[test]
    fn a_base_below_zero_is_refused() {
        // Required Insert Count 0, Sign 1, Delta Base 0
        assert_eq!(decode(&[0x00, 0x80, 0xd9], 1 << 16), Err(Refused::Invalid));
    }

    #[test]
    fn an_index_past_the_static_table_is_refused() {
        // indexed 98, then 99
        assert_eq!(
            decode(&[0x00, 0x00, 0xff, 0x23], 1 << 16),
            Ok(fields(&[("x-frame-options", "sameorigin")]))
        );
        assert_eq!(
            decode(&[0x00, 0x00, 0xff, 0x24], 1 << 16),
            Err(Refused::Invalid)
        );
        // a name reference to 99
        assert_eq!(
            decode(&[0x00, 0x00, 0x5f, 0x54, 0x00], 1 << 16),
            Err(Refused::Invalid)
        );
    }

    #[test]
    fn a_bad_huffman_string_is_refused() {
        // a value of one byte: eight bits of padding
        assert_eq!(
            decode(&[0x00, 0x00, 0x5f, 0x1d, 0x81, 0xff], 1 << 16),
            Err(Refused::Invalid)
        );
    }

    /// :status 200 counts 7 + 3 + 32 and x: y 1 + 1 + 32.
    #[test]
    fn the_size_counts_names_values_and_32_per_field() {
        let section = [0x00, 0x00, 0xd9, 0x21, b'x', 0x01, b'y'];
        assert!(decode(&section, 76).is_ok());
        assert_eq!(decode(&section, 75), Err(Refused::TooLarge));
    }

    /// A section cut anywhere is refused, or — cut between field lines —
    /// gives the fields before the cut.
    #[test]
    fn a_section_cut_anywhere_is_refused_or_shorter() {
        let whole = encode([
            (&b":status"[..], &b"200"[..], false),
            (b"user-agent", b"rurge", false),
            (b"a-name-longer-than-the-prefix", b"a value", true),
        ]);
        let all = decode(&whole, 1 << 16).unwrap();
        for cut in 0..whole.len() {
            match decode(&whole[..cut], 1 << 16) {
                Err(e) => assert_eq!(e, Refused::Invalid, "cut at {cut}"),
                Ok(some) => assert!(
                    some.len() < all.len() && all.starts_with(&some),
                    "cut at {cut}"
                ),
            }
        }
    }
}
```

新建 `crates/rurge-net/src/quic/h3/stream.rs`：

```rust
//! A request's stream (phase 2 M7 design 4): the request's content goes
//! out as DATA frames, the response head comes back, then the response's
//! content as the stream's bytes. A CONNECT's tunnel is the same stream.
//!
//! - A write takes at most 64 KiB, as one DATA frame, and takes nothing
//!   until the stream takes the frame's first byte. The part of the frame
//!   the stream could not take at once is finished — by the next write,
//!   flush or shutdown — before anything new is taken: a write future
//!   dropped (a timeout) never leaves a frame cut short, and never fails
//!   the connection.
//! - Shutdown finishes what was begun and sends the FIN; reading goes on.
//! - Reads hand over the DATA payloads as they come; QUIC's flow control
//!   holds the server back. Interim responses, frames of unknown types and
//!   the trailers are passed over.
//! - A stream dropped before both directions ended cancels its request:
//!   the sending part is reset and the receiving part stopped, with
//!   H3_REQUEST_CANCELLED (RFC 9114 4.1.1).

use super::client::Inner;
use super::frame::{self, FrameReader, ReadFailure};
use super::qpack::{self, Field, Refused};
use super::{
    H3_EXCESSIVE_LOAD, H3_FRAME_ERROR, H3_FRAME_UNEXPECTED, H3_ID_ERROR, H3_MESSAGE_ERROR,
    H3_REQUEST_CANCELLED, H3Error, MAX_FIELD_SECTION_SIZE, QPACK_DECOMPRESSION_FAILED,
};
use bytes::{Buf, Bytes, BytesMut};
use http::header::{HeaderName, HeaderValue};
use http::{Response, StatusCode};
use quinn::{RecvStream, SendStream, VarInt};
use std::fmt;
use std::future::poll_fn;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The most one write takes: one DATA frame.
const MAX_DATA_FRAME: usize = 64 << 10;

/// The fields of HTTP/1.1's connections, which HTTP/3 has no place for
/// (RFC 9114 4.2).
const CONNECTION_FIELDS: [&[u8]; 5] = [
    b"connection",
    b"keep-alive",
    b"proxy-connection",
    b"transfer-encoding",
    b"upgrade",
];

/// One request on its own bidirectional stream: write the content, shut
/// down, read the response head (`response`), then the response's content.
pub struct RequestStream {
    inner: Arc<Inner>,
    send: SendStream,
    /// The rest of a frame begun: out before anything new is taken.
    pending: Bytes,
    /// The FIN went out.
    finished: bool,
    reader: FrameReader,
    recv: Recv,
}

/// Where reading the response is.
enum Recv {
    /// Before the final response head, between frames.
    Head,
    /// In the payload of a HEADERS frame before the final head.
    HeadFields(usize),
    /// After the head, between frames.
    Body,
    /// In a DATA frame: what is left of it.
    Data(u64),
    /// After the trailers, between frames.
    Trailers,
    /// Passing over the rest of a frame, then back between frames.
    Skip { left: u64, then: Between },
    /// The server's side ended after its content.
    End,
    /// Failed: reads fail again, the same way.
    Failed(io::ErrorKind, Arc<str>),
}

/// The places between frames that passing over a frame goes back to.
#[derive(Clone, Copy)]
enum Between {
    Head,
    Body,
    Trailers,
}

impl From<Between> for Recv {
    fn from(between: Between) -> Recv {
        match between {
            Between::Head => Recv::Head,
            Between::Body => Recv::Body,
            Between::Trailers => Recv::Trailers,
        }
    }
}

impl RequestStream {
    /// A request whose head — a whole HEADERS frame — goes out first.
    pub(super) fn new(
        inner: Arc<Inner>,
        send: SendStream,
        recv: RecvStream,
        head: Bytes,
    ) -> RequestStream {
        RequestStream {
            inner,
            send,
            pending: head,
            finished: false,
            reader: FrameReader::new(recv),
            recv: Recv::Head,
        }
    }

    /// The response head; interim (1xx) responses and frames of unknown
    /// types are passed over. A head that is malformed or larger than
    /// 64 KiB ends the stream (H3_MESSAGE_ERROR, H3_EXCESSIVE_LOAD); a
    /// field section that cannot be decoded, or a frame where none may be,
    /// closes the connection.
    pub async fn response(&mut self) -> io::Result<Response<()>> {
        poll_fn(|cx| self.poll_response(cx)).await
    }

    fn poll_response(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<Response<()>>> {
        loop {
            match self.recv {
                Recv::Head => {
                    let header = match ready!(self.reader.poll_header(cx)) {
                        Ok(Some(header)) => header,
                        Ok(None) => {
                            return Poll::Ready(Err(self.fail(
                                io::ErrorKind::UnexpectedEof,
                                "h3: the server ended the stream without a response".to_string(),
                            )));
                        }
                        Err(failure) => return Poll::Ready(Err(self.read_failure(failure))),
                    };
                    match header.ty {
                        frame::HEADERS if header.len > MAX_FIELD_SECTION_SIZE => {
                            return Poll::Ready(Err(self.too_large()));
                        }
                        frame::HEADERS => self.recv = Recv::HeadFields(header.len as usize),
                        frame::DATA => {
                            return Poll::Ready(Err(self.violation(H3Error::new(
                                H3_FRAME_UNEXPECTED,
                                "the server sent DATA before the response head",
                            ))));
                        }
                        ty => self.pass_over(ty, header.len, Between::Head)?,
                    }
                }
                Recv::HeadFields(len) => {
                    let payload = match ready!(self.reader.poll_payload(cx, len)) {
                        Ok(payload) => payload,
                        Err(failure) => return Poll::Ready(Err(self.read_failure(failure))),
                    };
                    let head = match qpack::decode(&payload, MAX_FIELD_SECTION_SIZE) {
                        Ok(fields) => response_head(fields),
                        Err(Refused::Invalid) => {
                            return Poll::Ready(Err(self.violation(H3Error::new(
                                QPACK_DECOMPRESSION_FAILED,
                                "the server's field section cannot be decoded",
                            ))));
                        }
                        Err(Refused::TooLarge) => return Poll::Ready(Err(self.too_large())),
                    };
                    match head {
                        None => {
                            return Poll::Ready(Err(self.stream_error(
                                H3_MESSAGE_ERROR,
                                "h3: the server sent a malformed response".to_string(),
                            )));
                        }
                        Some(head) if head.status().is_informational() => self.recv = Recv::Head,
                        Some(head) => {
                            self.recv = Recv::Body;
                            return Poll::Ready(Ok(head));
                        }
                    }
                }
                Recv::Skip { .. } => ready!(self.poll_skip(cx))?,
                Recv::Failed(kind, ref text) => {
                    return Poll::Ready(Err(io::Error::new(kind, text.to_string())));
                }
                Recv::Body | Recv::Data(_) | Recv::Trailers | Recv::End => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "h3: the response head was read already",
                    )));
                }
            }
        }
    }

    /// A frame neither DATA nor HEADERS: a frame of another stream is an
    /// error of the connection, one of an unknown type is passed over (RFC
    /// 9114 7.2, 9).
    fn pass_over(&mut self, ty: u64, len: u64, then: Between) -> io::Result<()> {
        match ty {
            // we sent no MAX_PUSH_ID: no push ID is allowed (RFC 9114 7.2.5)
            frame::PUSH_PROMISE => {
                Err(self.violation(H3Error::new(H3_ID_ERROR, "the server promised a push")))
            }
            frame::CANCEL_PUSH | frame::SETTINGS | frame::GOAWAY | frame::MAX_PUSH_ID => {
                Err(self.unexpected(ty))
            }
            ty if frame::is_http2_only(ty) => Err(self.unexpected(ty)),
            _ => {
                self.recv = Recv::Skip { left: len, then };
                Ok(())
            }
        }
    }

    /// Passes over the rest of a frame, then goes back between frames.
    fn poll_skip(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let Recv::Skip { left, then } = &mut self.recv else {
            return Poll::Ready(Ok(()));
        };
        let then = *then;
        match ready!(self.reader.poll_skip(cx, left)) {
            Ok(()) => {
                self.recv = then.into();
                Poll::Ready(Ok(()))
            }
            Err(failure) => Poll::Ready(Err(self.read_failure(failure))),
        }
    }

    fn unexpected(&mut self, ty: u64) -> io::Error {
        self.violation(H3Error::new(
            H3_FRAME_UNEXPECTED,
            format!("the server sent a frame of type {ty:#x} on a request stream"),
        ))
    }

    fn too_large(&mut self) -> io::Error {
        self.stream_error(
            H3_EXCESSIVE_LOAD,
            format!("h3: the response head is larger than {MAX_FIELD_SECTION_SIZE} bytes"),
        )
    }

    /// An error of the connection (RFC 9114 8): it closes with `error`,
    /// and the stream fails with what it says.
    fn violation(&mut self, error: H3Error) -> io::Error {
        let e = error.io();
        self.inner.fail(error);
        self.fail(e.kind(), e.to_string())
    }

    /// An error of this stream alone: both directions end with `code`.
    fn stream_error(&mut self, code: VarInt, text: String) -> io::Error {
        // fails only for a part that ended already
        let _ = self.send.reset(code);
        self.reader.stop(code);
        self.fail(io::ErrorKind::InvalidData, text)
    }

    fn read_failure(&mut self, failure: ReadFailure) -> io::Error {
        match failure {
            ReadFailure::Truncated => self.violation(H3Error::new(
                H3_FRAME_ERROR,
                "the server's stream ended within a frame",
            )),
            ReadFailure::Stream(e) => {
                let e = self.inner.read_error(e);
                self.fail(e.kind(), e.to_string())
            }
        }
    }

    /// Reading failed with `text`: it fails so from now on.
    fn fail(&mut self, kind: io::ErrorKind, text: String) -> io::Error {
        let error = io::Error::new(kind, text.clone());
        self.recv = Recv::Failed(kind, text.into());
        error
    }

    /// What is left of a frame begun goes out.
    fn poll_pending(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while !self.pending.is_empty() {
            match ready!(Pin::new(&mut self.send).poll_write(cx, &self.pending)) {
                Ok(n) => self.pending.advance(n),
                Err(e) => return Poll::Ready(Err(self.inner.write_error(e))),
            }
        }
        Poll::Ready(Ok(()))
    }
}

/// The response `fields` make; `None` for a malformed one (RFC 9114 4.1.2):
/// `:status` must come first and alone of the pseudo-header fields, a
/// status from 100 to 599; names must be tokens in lowercase and values
/// field content (RFC 9110 5.1, 5.5); none of HTTP/1.1's connection fields
/// may come (RFC 9114 4.2).
fn response_head(fields: Vec<Field>) -> Option<Response<()>> {
    let mut fields = fields.into_iter();
    let (name, status) = fields.next()?;
    if name != b":status" {
        return None;
    }
    let status = StatusCode::from_bytes(&status)
        .ok()
        .filter(|status| status.as_u16() < 600)?;
    let mut response = Response::new(());
    *response.status_mut() = status;
    for (name, value) in fields {
        // a pseudo-header field among them is no token
        if !is_field_name(&name)
            || !is_field_value(&value)
            || CONNECTION_FIELDS.contains(&&name[..])
        {
            return None;
        }
        response.headers_mut().append(
            HeaderName::from_bytes(&name).ok()?,
            HeaderValue::from_bytes(&value).ok()?,
        );
    }
    Some(response)
}

/// A token in lowercase (RFC 9110 5.1, 5.6.2).
fn is_field_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name.iter().all(|&b| {
            matches!(b, b'a'..=b'z' | b'0'..=b'9'
                | b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.'
                | b'^' | b'_' | b'`' | b'|' | b'~')
        })
}

/// Field content (RFC 9110 5.5): visible characters and those past ASCII,
/// with spaces and tabs between them but not around them.
fn is_field_value(value: &[u8]) -> bool {
    let blank = |b: Option<&u8>| matches!(b, Some(b' ' | b'\t'));
    value
        .iter()
        .all(|&b| b == b' ' || b == b'\t' || (b >= 0x21 && b != 0x7f))
        && !blank(value.first())
        && !blank(value.last())
}

impl AsyncRead for RequestStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            match this.recv {
                Recv::Body | Recv::Trailers => {
                    let after_trailers = matches!(this.recv, Recv::Trailers);
                    let header = match ready!(this.reader.poll_header(cx)) {
                        Ok(Some(header)) => header,
                        Ok(None) => {
                            this.recv = Recv::End;
                            return Poll::Ready(Ok(()));
                        }
                        Err(failure) => return Poll::Ready(Err(this.read_failure(failure))),
                    };
                    match header.ty {
                        frame::DATA | frame::HEADERS if after_trailers => {
                            return Poll::Ready(Err(this.violation(H3Error::new(
                                H3_FRAME_UNEXPECTED,
                                "the server sent a frame after the trailers",
                            ))));
                        }
                        frame::DATA if header.len > 0 => this.recv = Recv::Data(header.len),
                        frame::DATA => {}
                        // the trailers: ignored
                        frame::HEADERS => {
                            this.recv = Recv::Skip {
                                left: header.len,
                                then: Between::Trailers,
                            };
                        }
                        ty => {
                            let then = if after_trailers {
                                Between::Trailers
                            } else {
                                Between::Body
                            };
                            this.pass_over(ty, header.len, then)?;
                        }
                    }
                }
                Recv::Data(mut left) => {
                    let read = ready!(this.reader.poll_data(cx, buf, &mut left));
                    this.recv = if left == 0 {
                        Recv::Body
                    } else {
                        Recv::Data(left)
                    };
                    return Poll::Ready(read.map_err(|failure| this.read_failure(failure)));
                }
                Recv::Skip { .. } => ready!(this.poll_skip(cx))?,
                Recv::End => return Poll::Ready(Ok(())),
                Recv::Failed(kind, ref text) => {
                    return Poll::Ready(Err(io::Error::new(kind, text.to_string())));
                }
                Recv::Head | Recv::HeadFields(_) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "h3: the response head has not been read",
                    )));
                }
            }
        }
    }
}

impl AsyncWrite for RequestStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.finished {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "h3: write after shutdown",
            )));
        }
        ready!(this.poll_pending(cx))?;
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let payload = &buf[..buf.len().min(MAX_DATA_FRAME)];
        let (header, len) = frame::data_header(payload.len());
        let header = &header[..len];
        // nothing is taken until the stream takes the frame's first byte
        let sent = match ready!(Pin::new(&mut this.send).poll_write(cx, header)) {
            Ok(sent) => sent,
            Err(e) => return Poll::Ready(Err(this.inner.write_error(e))),
        };
        if sent < header.len() {
            let mut rest = BytesMut::with_capacity(header.len() - sent + payload.len());
            rest.extend_from_slice(&header[sent..]);
            rest.extend_from_slice(payload);
            this.pending = rest.freeze();
            return Poll::Ready(Ok(payload.len()));
        }
        // the frame has begun: its payload follows, now or from `pending`
        let sent = match Pin::new(&mut this.send).poll_write(cx, payload) {
            Poll::Ready(Ok(sent)) => sent,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(this.inner.write_error(e))),
            Poll::Pending => 0,
        };
        if sent < payload.len() {
            this.pending = Bytes::copy_from_slice(&payload[sent..]);
        }
        Poll::Ready(Ok(payload.len()))
    }

    /// What was taken goes to the stream; QUIC sends it from there.
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut().poll_pending(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.finished {
            ready!(this.poll_pending(cx))?;
            // fails only for a stream reset already, which reads report
            let _ = this.send.finish();
            this.finished = true;
        }
        Poll::Ready(Ok(()))
    }
}

impl Drop for RequestStream {
    fn drop(&mut self) {
        // a request given up on ends abruptly both ways (RFC 9114 4.1.1);
        // a frame begun is never cut short by a FIN
        if !self.finished {
            let _ = self.send.reset(H3_REQUEST_CANCELLED);
        }
        if !matches!(self.recv, Recv::End) {
            self.reader.stop(H3_REQUEST_CANCELLED);
        }
    }
}

impl fmt::Debug for RequestStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestStream")
            .field("id", &self.send.id())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(fields: &[(&str, &str)]) -> Option<Response<()>> {
        response_head(
            fields
                .iter()
                .map(|(n, v)| (n.as_bytes().to_vec(), v.as_bytes().to_vec()))
                .collect(),
        )
    }

    #[test]
    fn a_response_head_is_its_status_and_fields() {
        let response = head(&[
            (":status", "233"),
            ("hysteria-udp", "true"),
            ("x", "inner  spaces\tand\u{e9}"),
            ("x", ""),
        ])
        .unwrap();
        assert_eq!(response.status(), 233);
        assert_eq!(response.headers()["hysteria-udp"], "true");
        assert_eq!(response.headers().get_all("x").iter().count(), 2);
    }

    #[test]
    fn malformed_response_heads_are_refused() {
        for fields in [
            &[][..],
            &[("content-type", "text/plain")],
            &[(":status", "200"), (":status", "200")],
            &[("x", "1"), (":status", "200")],
            &[(":status", "200"), (":path", "/")],
            &[(":status", "2000")],
            &[(":status", "099")],
            &[(":status", "600")],
            &[(":status", "20a")],
            &[(":status", "200"), ("X-Upper", "1")],
            &[(":status", "200"), ("a name", "1")],
            &[(":status", "200"), ("", "1")],
            &[(":status", "200"), ("connection", "close")],
            &[(":status", "200"), ("transfer-encoding", "chunked")],
            &[(":status", "200"), ("x", " leading")],
            &[(":status", "200"), ("x", "trailing\t")],
            &[(":status", "200"), ("x", "a\r\nb")],
            &[(":status", "200"), ("x", "a\0b")],
        ] {
            assert!(head(fields).is_none(), "{fields:?}");
        }
    }
}
```

新建 `crates/rurge-net/src/quic/h3/client.rs`：

```rust
//! The client's side of an HTTP/3 connection (phase 2 M7 design 4): our
//! control stream, the server's unidirectional streams, and requests.

use super::frame::{self, FrameReader, Header, ReadFailure, Settings};
use super::stream::RequestStream;
use super::{
    CONTROL_STREAM, H3_CLOSED_CRITICAL_STREAM, H3_EXCESSIVE_LOAD, H3_FRAME_ERROR,
    H3_FRAME_UNEXPECTED, H3_ID_ERROR, H3_MISSING_SETTINGS, H3_STREAM_CREATION_ERROR, H3Error,
    MAX_FIELD_SECTION_SIZE, PUSH_STREAM, QPACK_DECODER_STREAM, QPACK_ENCODER_STREAM,
    SETTINGS_MAX_FIELD_SECTION_SIZE, SETTINGS_QPACK_BLOCKED_STREAMS,
    SETTINGS_QPACK_MAX_TABLE_CAPACITY, code_name, qpack,
};
use crate::quic::connection_error;
use bytes::Bytes;
use http::{Method, Request};
use quinn::{Connection, ConnectionError, ReadError, RecvStream, WriteError};
use std::convert::Infallible;
use std::future::poll_fn;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};

/// Our SETTINGS: no dynamic table for the server's QPACK encoder and no
/// stream blocked on one, field sections of at most
/// `MAX_FIELD_SECTION_SIZE`. HTTP Datagrams (M7c) add `H3_DATAGRAM`.
const OUR_SETTINGS: [(u64, u64); 3] = [
    (SETTINGS_QPACK_MAX_TABLE_CAPACITY, 0),
    (SETTINGS_MAX_FIELD_SECTION_SIZE, MAX_FIELD_SECTION_SIZE),
    (SETTINGS_QPACK_BLOCKED_STREAMS, 0),
];

/// The most a frame of the server's control stream that is read whole
/// (SETTINGS, GOAWAY) may take.
const MAX_CONTROL_FRAME: u64 = 16 << 10;

/// HTTP/3 on one QUIC connection, as its client. A background task reads
/// the server's unidirectional streams and holds our control stream open;
/// it ends with the client. No `Debug`: nothing in it is worth printing.
pub struct Client {
    inner: Arc<Inner>,
    driver: JoinHandle<()>,
}

/// What the client, its task and its requests share.
pub(super) struct Inner {
    quic: Connection,
    state: watch::Sender<State>,
    /// The critical streams the server opened, a bit each (control, QPACK
    /// encoder, QPACK decoder): there is one of each at most.
    critical: AtomicU8,
}

#[derive(Default)]
struct State {
    /// The server's SETTINGS, once they came.
    settings: Option<Settings>,
    /// The lowest stream ID the server's latest GOAWAY leaves unprocessed.
    goaway: Option<u64>,
    /// What this client closed the connection for.
    failure: Option<H3Error>,
    /// How the connection ended, once the task saw it.
    ended: Option<ConnectionError>,
}

impl Client {
    /// HTTP/3 on `connection`, whose handshake chose ALPN `h3`: our control
    /// stream goes out with our SETTINGS, and the server's streams are read
    /// until the connection ends or the client is dropped.
    ///
    /// The client must live as long as the connection serves anything —
    /// HTTP/3 or a protocol's own streams: dropping it ends our control
    /// stream, which the server may take for the end of the connection
    /// (H3_CLOSED_CRITICAL_STREAM, RFC 9114 6.2.1).
    pub fn new(connection: Connection) -> Client {
        let inner = Arc::new(Inner {
            quic: connection,
            state: watch::Sender::new(State::default()),
            critical: AtomicU8::new(0),
        });
        let driver = tokio::spawn(drive(inner.clone()));
        Client { inner, driver }
    }

    /// The server's SETTINGS, once they came: extended CONNECT and HTTP
    /// Datagrams need them first. The caller bounds the wait; an error when
    /// the connection ended before.
    pub async fn settings(&self) -> io::Result<Settings> {
        let mut state = self.inner.state.subscribe();
        let state = state
            .wait_for(|s| s.settings.is_some() || s.failure.is_some() || s.ended.is_some())
            .await
            .expect("the client holds the sender");
        match (&state.settings, &state.failure, &state.ended) {
            (Some(settings), _, _) => Ok(settings.clone()),
            (None, Some(failure), _) => Err(failure.io()),
            (None, None, Some(ended)) => Err(connection_error(ended)),
            (None, None, None) => unreachable!("waited for one of them"),
        }
    }

    /// Whether the server sent GOAWAY: no new request goes on this
    /// connection; those under way run to their end (RFC 9114 5.2).
    pub fn is_going_away(&self) -> bool {
        self.inner.state.borrow().goaway.is_some()
    }

    /// Sends `request`'s head on a new bidirectional stream, as one
    /// HEADERS frame: its pseudo-header fields — `:method` and `:authority`
    /// alone for a CONNECT (RFC 9114 4.4), else `:scheme` (`https` unless
    /// the URI says), `:authority` and `:path` — then its header fields, a
    /// sensitive value never to be indexed. The request's content, if any,
    /// is written to the stream and ended by its shutdown; the response
    /// comes back on it. Refused once the server sent GOAWAY.
    pub async fn request(&self, request: Request<()>) -> io::Result<RequestStream> {
        {
            let state = self.inner.state.borrow();
            if let Some(failure) = &state.failure {
                return Err(failure.io());
            }
            if state.goaway.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "h3: the server is going away",
                ));
            }
        }
        let head = frame::frame(frame::HEADERS, &head(&request)?);
        let (send, recv) = self
            .inner
            .quic
            .open_bi()
            .await
            .map_err(|e| self.inner.lost(&e))?;
        // from here on, a request given up on is cancelled by the stream
        let mut stream = RequestStream::new(self.inner.clone(), send, recv, Bytes::from(head));
        stream.flush().await?;
        Ok(stream)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.driver.abort();
    }
}

/// The field section of `request`'s head.
fn head(request: &Request<()>) -> io::Result<Vec<u8>> {
    let uri = request.uri();
    let Some(authority) = uri.authority() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "h3: a request needs an authority",
        ));
    };
    let authority = authority.as_str().as_bytes();
    let mut fields = vec![(&b":method"[..], request.method().as_str().as_bytes(), false)];
    // extended CONNECT (M7c) has `:protocol`, `:scheme` and `:path` too
    if request.method() == Method::CONNECT {
        fields.push((&b":authority"[..], authority, false));
    } else {
        let scheme = uri.scheme_str().unwrap_or("https");
        let path = uri.path_and_query().map_or("/", |path| path.as_str());
        fields.push((&b":scheme"[..], scheme.as_bytes(), false));
        fields.push((&b":authority"[..], authority, false));
        fields.push((&b":path"[..], path.as_bytes(), false));
    }
    fields.extend(request.headers().iter().map(|(name, value)| {
        (
            name.as_str().as_bytes(),
            value.as_bytes(),
            value.is_sensitive(),
        )
    }));
    Ok(qpack::encode(fields))
}

impl Inner {
    /// Closes the connection for `error` (RFC 9114 8): its requests fail
    /// with what it says. Once only, and not once the connection ended.
    pub(super) fn fail(&self, error: H3Error) {
        let first = self.state.send_if_modified(|state| {
            if state.failure.is_some() || state.ended.is_some() {
                return false;
            }
            state.failure = Some(error.clone());
            true
        });
        if first {
            tracing::debug!(
                code = %code_name(error.code),
                "h3: {}; closing the connection",
                error.text
            );
            self.quic.close(error.code, error.text.as_bytes());
        }
    }

    /// The connection's end as an I/O error: what this client closed it
    /// for, when it did.
    pub(super) fn lost(&self, e: &ConnectionError) -> io::Error {
        match (&self.state.borrow().failure, e) {
            (Some(failure), ConnectionError::LocallyClosed) => failure.io(),
            _ => connection_error(e),
        }
    }

    /// A failed read of a request stream as an I/O error.
    pub(super) fn read_error(&self, e: ReadError) -> io::Error {
        match e {
            ReadError::Reset(code) => io::Error::new(
                io::ErrorKind::ConnectionReset,
                format!("h3: the server reset the stream ({})", code_name(code)),
            ),
            ReadError::ConnectionLost(e) => self.lost(&e),
            ReadError::ClosedStream
            | ReadError::IllegalOrderedRead
            | ReadError::ZeroRttRejected => closed_stream(),
        }
    }

    /// A failed write of a request stream as an I/O error.
    pub(super) fn write_error(&self, e: WriteError) -> io::Error {
        match e {
            WriteError::Stopped(code) => io::Error::new(
                io::ErrorKind::ConnectionReset,
                format!("h3: the server stopped the stream ({})", code_name(code)),
            ),
            WriteError::ConnectionLost(e) => self.lost(&e),
            WriteError::ClosedStream | WriteError::ZeroRttRejected => closed_stream(),
        }
    }

    /// Notes that the server opened its critical stream of `kind` (`bit`);
    /// a second one is H3_STREAM_CREATION_ERROR (RFC 9114 6.2.1, RFC 9204
    /// 4.2).
    fn opened(&self, bit: u8, kind: &str) -> Result<(), H3Error> {
        if self.critical.fetch_or(bit, Ordering::Relaxed) & bit != 0 {
            return Err(H3Error::new(
                H3_STREAM_CREATION_ERROR,
                format!("the server opened a second {kind} stream"),
            ));
        }
        Ok(())
    }

    /// The server's GOAWAY for `id`: one of our request streams, and none
    /// later than a GOAWAY before (RFC 9114 5.2, 7.2.6).
    fn goaway(&self, id: u64) -> Result<(), H3Error> {
        if id % 4 != 0 {
            return Err(H3Error::new(
                H3_ID_ERROR,
                "the server's GOAWAY names no request stream",
            ));
        }
        let mut grew = false;
        self.state.send_if_modified(|state| match state.goaway {
            Some(before) if id > before => {
                grew = true;
                false
            }
            _ => {
                state.goaway = Some(id);
                true
            }
        });
        if grew {
            return Err(H3Error::new(
                H3_ID_ERROR,
                "the server's GOAWAY named a later stream than its last",
            ));
        }
        Ok(())
    }
}

fn closed_stream() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "h3: the stream is closed")
}

/// Holds our control stream and reads the server's unidirectional streams,
/// each in a task of its own, until the connection ends — or until the
/// client aborts it, and its tasks with it.
async fn drive(inner: Arc<Inner>) {
    let mut tasks = JoinSet::new();
    tasks.spawn(control(inner.clone()));
    loop {
        tokio::select! {
            accepted = inner.quic.accept_uni() => match accepted {
                Ok(recv) => {
                    tasks.spawn(server_stream(inner.clone(), recv));
                }
                Err(e) => {
                    inner.state.send_modify(|state| state.ended = Some(e));
                    return;
                }
            },
            // a stream that ended; nothing to collect
            Some(_) = tasks.join_next() => {}
        }
    }
}

/// Opens our control stream, sends our SETTINGS as its first frame, and
/// holds it open: closing it would end HTTP/3 on the connection (RFC 9114
/// 6.2.1). A failure is the connection's, which the driver reports.
async fn control(inner: Arc<Inner>) {
    let mut payload = Vec::new();
    for (id, value) in OUR_SETTINGS {
        frame::put_varint(&mut payload, id);
        frame::put_varint(&mut payload, value);
    }
    let mut bytes = Vec::new();
    frame::put_varint(&mut bytes, CONTROL_STREAM);
    bytes.extend_from_slice(&frame::frame(frame::SETTINGS, &payload));
    if let Ok(mut send) = inner.quic.open_uni().await
        && send.write_all(&bytes).await.is_ok()
    {
        std::future::pending::<()>().await;
    }
}

/// Why reading one of the server's critical streams stopped.
enum Stop {
    /// The server broke HTTP/3: the connection closes.
    Violation(H3Error),
    /// The connection ended, which the driver reports.
    Lost,
}

impl From<H3Error> for Stop {
    fn from(error: H3Error) -> Stop {
        Stop::Violation(error)
    }
}

/// A failed read of a critical stream: its end is the server's error
/// (H3_CLOSED_CRITICAL_STREAM, cut within a frame or not); the connection's
/// end is not.
fn critical_failure(failure: ReadFailure) -> Stop {
    match failure {
        ReadFailure::Truncated | ReadFailure::Stream(ReadError::Reset(_)) => closed_critical(),
        ReadFailure::Stream(_) => Stop::Lost,
    }
}

fn closed_critical() -> Stop {
    Stop::Violation(H3Error::new(
        H3_CLOSED_CRITICAL_STREAM,
        "the server closed a critical stream",
    ))
}

/// One of the server's unidirectional streams, as its type says (RFC 9114
/// 6.2, RFC 9204 4.2).
async fn server_stream(inner: Arc<Inner>, recv: RecvStream) {
    let mut reader = FrameReader::new(recv);
    // a stream that ends before its type says nothing
    let Ok(Some(ty)) = poll_fn(|cx| reader.poll_varint(cx)).await else {
        return;
    };
    let stop = match ty {
        CONTROL_STREAM => match inner.opened(0b001, "control") {
            Ok(()) => match control_stream(&inner, &mut reader).await {
                Err(stop) => stop,
            },
            Err(error) => error.into(),
        },
        PUSH_STREAM => H3Error::new(H3_ID_ERROR, "the server opened a push stream").into(),
        QPACK_ENCODER_STREAM | QPACK_DECODER_STREAM => {
            let (bit, kind) = if ty == QPACK_ENCODER_STREAM {
                (0b010, "QPACK encoder")
            } else {
                (0b100, "QPACK decoder")
            };
            match inner.opened(bit, kind) {
                Ok(()) => qpack_stream(&mut reader).await,
                Err(error) => error.into(),
            }
        }
        // a type this client does not know (RFC 9114 6.2.3)
        _ => {
            reader.stop(H3_STREAM_CREATION_ERROR);
            return;
        }
    };
    if let Stop::Violation(error) = stop {
        inner.fail(error);
    }
}

/// The server's control stream: SETTINGS first, then GOAWAY; frames of
/// unknown types are passed over (RFC 9114 6.2.1, 7.2). It never ends but
/// in an error.
async fn control_stream(inner: &Inner, reader: &mut FrameReader) -> Result<Infallible, Stop> {
    let mut first = true;
    loop {
        let header = match poll_fn(|cx| reader.poll_header(cx)).await {
            Ok(Some(header)) => header,
            Ok(None) => return Err(closed_critical()),
            Err(failure) => return Err(critical_failure(failure)),
        };
        if first && header.ty != frame::SETTINGS {
            return Err(H3Error::new(
                H3_MISSING_SETTINGS,
                "the server's control stream did not start with SETTINGS",
            )
            .into());
        }
        match header.ty {
            frame::SETTINGS if first => {
                let settings = Settings::decode(&whole(reader, header).await?)?;
                inner
                    .state
                    .send_modify(|state| state.settings = Some(settings));
            }
            frame::GOAWAY => {
                let payload = whole(reader, header).await?;
                match frame::get_varint(&payload) {
                    Some((id, n)) if n == payload.len() => inner.goaway(id)?,
                    _ => {
                        return Err(H3Error::new(
                            H3_FRAME_ERROR,
                            "the server's GOAWAY is malformed",
                        )
                        .into());
                    }
                }
            }
            frame::CANCEL_PUSH => {
                return Err(H3Error::new(H3_ID_ERROR, "the server cancelled a push").into());
            }
            ty if matches!(
                ty,
                frame::DATA
                    | frame::HEADERS
                    | frame::SETTINGS
                    | frame::PUSH_PROMISE
                    | frame::MAX_PUSH_ID
            ) || frame::is_http2_only(ty) =>
            {
                return Err(H3Error::new(
                    H3_FRAME_UNEXPECTED,
                    format!("the server sent a frame of type {ty:#x} on its control stream"),
                )
                .into());
            }
            _ => {
                let mut left = header.len;
                poll_fn(|cx| reader.poll_skip(cx, &mut left))
                    .await
                    .map_err(critical_failure)?;
            }
        }
        first = false;
    }
}

/// The payload of a frame of the control stream, read whole.
async fn whole(reader: &mut FrameReader, header: Header) -> Result<Bytes, Stop> {
    if header.len > MAX_CONTROL_FRAME {
        return Err(H3Error::new(
            H3_EXCESSIVE_LOAD,
            format!("the server sent a control frame of {} bytes", header.len),
        )
        .into());
    }
    poll_fn(|cx| reader.poll_payload(cx, header.len as usize))
        .await
        .map_err(critical_failure)
}

/// The server's QPACK encoder or decoder stream. With no dynamic table on
/// either side there is nothing on it to act on: its bytes are dropped. It
/// must stay open (RFC 9204 4.2).
async fn qpack_stream(reader: &mut FrameReader) -> Stop {
    match poll_fn(|cx| reader.poll_drain(cx)).await {
        Ok(()) => closed_critical(),
        Err(failure) => critical_failure(failure),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request without an authority cannot be sent: HTTPS wants one
    /// (RFC 9114 4.3.1), and so does a CONNECT.
    #[test]
    fn a_request_needs_an_authority() {
        for request in [
            Request::get("/index.html").body(()).unwrap(),
            Request::connect("/").body(()).unwrap(),
        ] {
            let err = head(&request).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(err.to_string(), "h3: a request needs an authority");
        }
    }
}
```

新建 `crates/rurge-net/src/quic/h3/testing.rs`：

```rust
//! The server's side of HTTP/3 for the loopback fakes (phase 2 M7 design
//! 4, 5.5): SETTINGS on a control stream of its own, the client's streams
//! read, requests taken apart and answered. Its frames and its QPACK are
//! code of its own — only the static table and the Huffman code are the
//! client's — so what the client sends is read by code that is not the
//! client's. It reads what this client sends and no more: field sections
//! without a dynamic table, and no Huffman-encoded string (the client
//! sends none). Never used by production code.

use super::huffman::CODES;
use super::qpack::STATIC_TABLE;
use quinn::{Connection, RecvStream, SendStream, VarInt};
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};

// the wire as the fake knows it
const DATA: u64 = 0x00;
const HEADERS: u64 = 0x01;
const SETTINGS: u64 = 0x04;
const GOAWAY: u64 = 0x07;
const CONTROL: u64 = 0x00;

/// An HTTP/3 server on an accepted connection. Dropping it stops reading
/// the client's streams; the connection stays the caller's.
pub struct H3Server {
    quic: Connection,
    /// Our control stream, once SETTINGS went out on it.
    control: Option<SendStream>,
    client: watch::Receiver<ClientStreams>,
    reader: JoinHandle<()>,
}

/// What the client's unidirectional streams brought.
#[derive(Default)]
struct ClientStreams {
    /// Their types, in the order they came.
    opened: Vec<u64>,
    /// The types of those the client ended.
    ended: Vec<u64>,
    /// The client's SETTINGS, once they came.
    settings: Option<Vec<(u64, u64)>>,
}

impl H3Server {
    /// HTTP/3 on `connection` without SETTINGS yet (`send_settings`): the
    /// client's unidirectional streams are read from now on.
    pub fn new(connection: Connection) -> H3Server {
        let (seen, client) = watch::channel(ClientStreams::default());
        let reader = tokio::spawn(read_client(connection.clone(), Arc::new(seen)));
        H3Server {
            quic: connection,
            control: None,
            client,
            reader,
        }
    }

    /// HTTP/3 on `connection`, `settings` sent at once.
    pub async fn start(connection: Connection, settings: &[(u64, u64)]) -> io::Result<H3Server> {
        let mut server = H3Server::new(connection);
        server.send_settings(settings).await?;
        Ok(server)
    }

    /// Opens our control stream with a SETTINGS frame of `settings`, in
    /// order.
    pub async fn send_settings(&mut self, settings: &[(u64, u64)]) -> io::Result<()> {
        let mut payload = Vec::new();
        for &(id, value) in settings {
            payload.extend(varint(id));
            payload.extend(varint(value));
        }
        let mut bytes = varint(CONTROL);
        bytes.extend(frame(SETTINGS, &payload));
        let mut control = self.quic.open_uni().await?;
        control.write_all(&bytes).await?;
        self.control = Some(control);
        Ok(())
    }

    /// A frame on our control stream, after the SETTINGS.
    pub async fn control_frame(&mut self, ty: u64, payload: &[u8]) -> io::Result<()> {
        let control = self.control.as_mut().expect("the SETTINGS went out first");
        control.write_all(&frame(ty, payload)).await?;
        Ok(())
    }

    /// GOAWAY: no request on stream `id` or later is served.
    pub async fn goaway(&mut self, id: u64) -> io::Result<()> {
        self.control_frame(GOAWAY, &varint(id)).await
    }

    /// The client's SETTINGS, once they came; an error when the connection
    /// ended first.
    pub async fn client_settings(&self) -> io::Result<Vec<(u64, u64)>> {
        let mut client = self.client.clone();
        let streams = client
            .wait_for(|streams| streams.settings.is_some())
            .await
            .map_err(|_| io::Error::other("the connection ended before the client's SETTINGS"))?;
        Ok(streams.settings.clone().unwrap_or_default())
    }

    /// The types of the unidirectional streams the client opened so far.
    pub fn client_streams(&self) -> Vec<u64> {
        self.client.borrow().opened.clone()
    }

    /// The types of the client's unidirectional streams that it ended.
    pub fn client_streams_ended(&self) -> Vec<u64> {
        self.client.borrow().ended.clone()
    }

    /// The client's next bidirectional stream; `None` once the connection
    /// is gone.
    pub async fn accept(&self) -> Option<ServerStream> {
        let (send, recv) = self.quic.accept_bi().await.ok()?;
        Some(ServerStream {
            send,
            incoming: Incoming {
                recv,
                buf: Vec::new(),
            },
        })
    }
}

impl Drop for H3Server {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// Reads the client's unidirectional streams, each in a task of its own.
async fn read_client(quic: Connection, seen: Arc<watch::Sender<ClientStreams>>) {
    let mut streams = JoinSet::new();
    while let Ok(recv) = quic.accept_uni().await {
        streams.spawn(read_client_stream(recv, seen.clone()));
    }
}

/// One of the client's unidirectional streams: its type, the SETTINGS of
/// a control stream, and then whatever comes, dropped, until it ends.
async fn read_client_stream(recv: RecvStream, seen: Arc<watch::Sender<ClientStreams>>) {
    let mut incoming = Incoming {
        recv,
        buf: Vec::new(),
    };
    let Ok(Some(ty)) = incoming.varint().await else {
        return;
    };
    seen.send_modify(|streams| streams.opened.push(ty));
    if ty == CONTROL
        && let Ok(Some((SETTINGS, payload))) = incoming.frame().await
        && let Some(pairs) = settings(&payload)
    {
        seen.send_modify(|streams| streams.settings = Some(pairs));
    }
    loop {
        match incoming.recv.read_chunk(usize::MAX, true).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                seen.send_modify(|streams| streams.ended.push(ty));
                return;
            }
            Err(_) => return,
        }
    }
}

/// The pairs of a SETTINGS payload.
fn settings(mut payload: &[u8]) -> Option<Vec<(u64, u64)>> {
    let mut pairs = Vec::new();
    while !payload.is_empty() {
        let (id, n) = parse_varint(payload)?;
        let (value, m) = parse_varint(&payload[n..])?;
        payload = &payload[n + m..];
        pairs.push((id, value));
    }
    Some(pairs)
}

/// One of the client's bidirectional streams, as the fake serves it: HTTP/3
/// (`read_headers`, `send_headers`, …) or — after `peek_varint` — another
/// protocol's bytes (`AsyncRead`, `AsyncWrite`).
pub struct ServerStream {
    send: SendStream,
    incoming: Incoming,
}

impl ServerStream {
    /// The stream's first variable-length integer, left unread: a frame
    /// type (HEADERS, 0x01) for HTTP/3, 0x401 for a Hysteria 2 TCP request.
    pub async fn peek_varint(&mut self) -> io::Result<u64> {
        if !self.incoming.fill(1).await? {
            return Err(cut_short());
        }
        let len = 1 << (self.incoming.buf[0] >> 6);
        if !self.incoming.fill(len).await? {
            return Err(cut_short());
        }
        Ok(parse_varint(&self.incoming.buf)
            .expect("the whole integer")
            .0)
    }

    /// The fields of the next HEADERS frame, in order; frames of other
    /// types before it are passed over.
    pub async fn read_headers(&mut self) -> io::Result<Vec<(String, String)>> {
        loop {
            match self.incoming.frame().await? {
                Some((HEADERS, payload)) => return fields(&payload),
                Some((DATA, _)) => return Err(invalid("DATA where HEADERS was due")),
                Some(_) => {}
                None => return Err(cut_short()),
            }
        }
    }

    /// The request's content: its DATA frames up to the end of the stream.
    pub async fn read_body(&mut self) -> io::Result<Vec<u8>> {
        let mut body = Vec::new();
        while let Some((ty, payload)) = self.incoming.frame().await? {
            match ty {
                DATA => body.extend(payload),
                HEADERS => return Err(invalid("trailers: the client sends none")),
                _ => {}
            }
        }
        Ok(body)
    }

    /// The next frame, whole — type and payload — as the client framed it;
    /// `None` at the end of the stream.
    pub async fn read_frame(&mut self) -> io::Result<Option<(u64, Vec<u8>)>> {
        self.incoming.frame().await
    }

    /// A response head: `:status`, then `headers` (`response_section`).
    pub async fn send_headers(&mut self, status: u16, headers: &[(&str, &str)]) -> io::Result<()> {
        self.send_frame(HEADERS, &response_section(status, headers))
            .await
    }

    /// A DATA frame of `data`.
    pub async fn send_data(&mut self, data: &[u8]) -> io::Result<()> {
        self.send_frame(DATA, data).await
    }

    /// A frame of any type.
    pub async fn send_frame(&mut self, ty: u64, payload: &[u8]) -> io::Result<()> {
        self.send.write_all(&frame(ty, payload)).await?;
        Ok(())
    }

    /// Ends the server's side (FIN).
    pub fn finish(&mut self) {
        let _ = self.send.finish();
    }

    /// Resets the server's side with `code`.
    pub fn reset(&mut self, code: u64) {
        let _ = self
            .send
            .reset(VarInt::from_u64(code).expect("a QUIC integer"));
    }

    /// The code of the client's STOP_SENDING; `None` when it read the
    /// server's side to its end.
    pub async fn client_stopped(&self) -> Option<u64> {
        self.send.stopped().await.ok().flatten().map(u64::from)
    }

    /// The code of the client's RESET_STREAM; `None` when it finished.
    pub async fn client_reset(&mut self) -> Option<u64> {
        self.incoming
            .recv
            .received_reset()
            .await
            .ok()
            .flatten()
            .map(u64::from)
    }
}

impl AsyncRead for ServerStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.incoming.buf.is_empty() {
            return AsyncRead::poll_read(Pin::new(&mut this.incoming.recv), cx, buf);
        }
        // what a peek read ahead comes first
        let n = this.incoming.buf.len().min(buf.remaining());
        buf.put_slice(&this.incoming.buf[..n]);
        this.incoming.buf.drain(..n);
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for ServerStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncWrite::poll_write(Pin::new(&mut self.get_mut().send), cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.get_mut().send), cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_shutdown(Pin::new(&mut self.get_mut().send), cx)
    }
}

/// A stream read as the fake reads it: as much as it needs at a time.
struct Incoming {
    recv: RecvStream,
    /// Read and not yet taken.
    buf: Vec<u8>,
}

impl Incoming {
    /// At least `n` bytes read ahead; `false` when the stream ended first.
    async fn fill(&mut self, n: usize) -> io::Result<bool> {
        while self.buf.len() < n {
            match self.recv.read_chunk(n - self.buf.len(), true).await? {
                Some(chunk) => self.buf.extend_from_slice(&chunk.bytes),
                None => return Ok(false),
            }
        }
        Ok(true)
    }

    /// The next variable-length integer; `None` at the end of the stream.
    async fn varint(&mut self) -> io::Result<Option<u64>> {
        if !self.fill(1).await? {
            return Ok(None);
        }
        let len = 1 << (self.buf[0] >> 6);
        if !self.fill(len).await? {
            return Err(cut_short());
        }
        let (value, _) = parse_varint(&self.buf).expect("the whole integer");
        self.buf.drain(..len);
        Ok(Some(value))
    }

    /// The next frame whole: its type and payload; `None` at the end of the
    /// stream.
    async fn frame(&mut self) -> io::Result<Option<(u64, Vec<u8>)>> {
        let Some(ty) = self.varint().await? else {
            return Ok(None);
        };
        let len = self.varint().await?.ok_or_else(cut_short)?;
        let len = usize::try_from(len).map_err(|_| invalid("a frame longer than memory"))?;
        if !self.fill(len).await? {
            return Err(cut_short());
        }
        Ok(Some((ty, self.buf.drain(..len).collect())))
    }
}

/// The variable-length integer at the start of `bytes` and its length;
/// `None` when `bytes` holds part of it.
fn parse_varint(bytes: &[u8]) -> Option<(u64, usize)> {
    let len = 1 << (bytes.first()? >> 6);
    let bytes = bytes.get(..len)?;
    let mut value = u64::from(bytes[0] & 0x3f);
    for &byte in &bytes[1..] {
        value = (value << 8) | u64::from(byte);
    }
    Some((value, len))
}

/// `value` as a variable-length integer, as short as it goes.
fn varint(value: u64) -> Vec<u8> {
    if value < 1 << 6 {
        vec![value as u8]
    } else if value < 1 << 14 {
        (0x4000 | value as u16).to_be_bytes().to_vec()
    } else if value < 1 << 30 {
        (0x8000_0000 | value as u32).to_be_bytes().to_vec()
    } else {
        (0xc000_0000_0000_0000 | value).to_be_bytes().to_vec()
    }
}

fn frame(ty: u64, payload: &[u8]) -> Vec<u8> {
    let mut bytes = varint(ty);
    bytes.extend(varint(payload.len() as u64));
    bytes.extend_from_slice(payload);
    bytes
}

/// The fields of a field section from the client, in order: static
/// entries and literals only, no Huffman-encoded string.
fn fields(section: &[u8]) -> io::Result<Vec<(String, String)>> {
    // Required Insert Count 0 and Base 0, as the client always writes them
    let Some(mut src) = section.strip_prefix(&[0, 0]) else {
        return Err(invalid("a field section that needs a dynamic table"));
    };
    let mut fields = Vec::new();
    while let Some(&first) = src.first() {
        fields.push(match first {
            // indexed, static
            0xc0..=0xff => {
                let (name, value) = static_entry(prefixed(&mut src, 6)?)?;
                (name.to_string(), value.to_string())
            }
            // a literal value with a static name, never indexed or not
            0x50..=0x5f | 0x70..=0x7f => {
                let (name, _) = static_entry(prefixed(&mut src, 4)?)?;
                (name.to_string(), literal(&mut src, 7)?)
            }
            // a literal name and value
            0x20..=0x3f => {
                let name = literal(&mut src, 3)?;
                (name, literal(&mut src, 7)?)
            }
            _ => {
                return Err(invalid(format!(
                    "field line {first:#04x}: the dynamic table"
                )));
            }
        });
    }
    Ok(fields)
}

/// An integer with a `bits`-bit prefix (RFC 7541 5.1).
fn prefixed(src: &mut &[u8], bits: u32) -> io::Result<usize> {
    let mask = (1usize << bits) - 1;
    let (&first, rest) = src.split_first().ok_or_else(cut_short)?;
    *src = rest;
    let mut value = usize::from(first) & mask;
    if value == mask {
        let mut shift = 0;
        loop {
            let (&byte, rest) = src.split_first().ok_or_else(cut_short)?;
            *src = rest;
            value += usize::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift > 28 {
                return Err(invalid("an integer past what a test sends"));
            }
        }
    }
    Ok(value)
}

/// A string literal whose length has a `bits`-bit prefix, the H bit right
/// above it.
fn literal(src: &mut &[u8], bits: u32) -> io::Result<String> {
    if src.first().is_some_and(|&b| b & (1 << bits) != 0) {
        return Err(invalid("a Huffman-encoded string: the client sends none"));
    }
    let len = prefixed(src, bits)?;
    if len > src.len() {
        return Err(cut_short());
    }
    let (bytes, rest) = src.split_at(len);
    *src = rest;
    String::from_utf8(bytes.to_vec()).map_err(|_| invalid("a string that is no UTF-8"))
}

fn static_entry(index: usize) -> io::Result<(&'static str, &'static str)> {
    STATIC_TABLE
        .get(index)
        .copied()
        .ok_or_else(|| invalid("an index past the static table"))
}

/// A response's field section as a server's encoder might make it:
/// `:status` and each field as an exact static entry when there is one,
/// else as a literal — with the static name when there is one — whose
/// strings are Huffman-encoded.
pub fn response_section(status: u16, headers: &[(&str, &str)]) -> Vec<u8> {
    let status = status.to_string();
    let mut out = vec![0, 0];
    for (name, value) in
        std::iter::once((":status", status.as_str())).chain(headers.iter().copied())
    {
        if let Some(index) = STATIC_TABLE
            .iter()
            .position(|&entry| entry == (name, value))
        {
            put_prefixed(&mut out, 0xc0, 6, index);
            continue;
        }
        match STATIC_TABLE.iter().position(|&(n, _)| n == name) {
            Some(index) => put_prefixed(&mut out, 0x50, 4, index),
            None => put_huffman(&mut out, 0x20, 3, name.as_bytes()),
        }
        put_huffman(&mut out, 0x00, 7, value.as_bytes());
    }
    out
}

fn put_prefixed(out: &mut Vec<u8>, flags: u8, bits: u32, value: usize) {
    let mask = (1usize << bits) - 1;
    if value < mask {
        out.push(flags | value as u8);
        return;
    }
    out.push(flags | mask as u8);
    let mut rest = value - mask;
    while rest > 0x7f {
        out.push(0x80 | (rest & 0x7f) as u8);
        rest >>= 7;
    }
    out.push(rest as u8);
}

/// `bytes` Huffman-encoded as a string literal: H, the bit above the
/// `bits`-bit prefix of the length, set.
fn put_huffman(out: &mut Vec<u8>, flags: u8, bits: u32, bytes: &[u8]) {
    let encoded = huffman_encode(bytes);
    put_prefixed(out, flags | (1 << bits), bits, encoded.len());
    out.extend_from_slice(&encoded);
}

/// `bytes` Huffman-encoded (RFC 7541 5.2): the codes of `CODES`, padded to
/// the byte with the first bits of EOS.
pub(crate) fn huffman_encode(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u64, 0u32);
    for &byte in bytes {
        let (code, len) = CODES[usize::from(byte)];
        acc = (acc << len) | u64::from(code);
        bits += u32::from(len);
        while bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(((acc << (8 - bits)) as u8) | (0xff >> bits));
    }
    out
}

fn cut_short() -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "h3 fake: the stream ended early",
    )
}

fn invalid(what: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("h3 fake: {what}"))
}
```

要点：
- 我们只开控制流，SETTINGS 是 `QPACK_MAX_TABLE_CAPACITY=0`、`QPACK_BLOCKED_STREAMS=0`、`MAX_FIELD_SECTION_SIZE=65536`；编码只用静态索引与字面量，从不做 Huffman；解码遇到动态表引用即 `QPACK_DECOMPRESSION_FAILED`。
- `Client` 要与连接同生共死（P11）：Hysteria 2 在同一条连接上另开原始的双向流，`Client` 从不占用双向流。
- 应答的 `Content-Length` 不与 DATA 的总长比对、SETTINGS 0x08 / 0x33 的取值校验留给 M7c（「延后事项」#1）。

- [ ] **Step 2: 运行**

Run: `cargo test -p rurge-net h3`
Expected: 通过（新增 50 条：RFC 7541 C.4 / C.6 的 Huffman 例子与填充、EOS 的拒绝；静态表与字面量的编解码、截断与越界、动态表引用；帧编解码与未知类型；回环上的请求与应答（含大的内容）；迟到的 SETTINGS；GOAWAY 之后不再接新请求；写到一半被取消后流仍然有效；变长整数的边界）。

- [ ] **Step 3: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-net
git commit -m "feat(net): 最小 HTTP/3 客户端——静态表 QPACK、控制流与 SETTINGS、取消安全的 DATA 写入、GOAWAY"
```

### Task 4: `QuicTls` 与新 crate `rurge-proto-quic` 的会话

rurge-proto 的 TLS 层给 QUIC 一个只用 TLS 1.3 的配置（P13）；新 crate `rurge-proto-quic` 与 `session`（P14、P15）。会话的用例在新文件里；rurge-proto 的两条用例先写。

**Files:**
- Create: `crates/rurge-proto-quic/{Cargo.toml, src/lib.rs, src/session.rs, src/testing.rs}`（`session.rs` 自带用例）
- Modify: `Cargo.toml`（工作区成员与依赖）、`crates/rurge-proto/src/transport/tls.rs`（与用例）、`crates/rurge-proto/src/build.rs`（与用例）

**Interfaces:**
- Consumes: Task 1 的 `TlsOpts`（ALPN 已是 `h3`）、`QuicOpts`；Task 2 的 `Path::open`、`client_config`、`connect`、`connection_error`、`Transport::STANDARD`、`Connector::route()`。
- Produces: `rurge_proto::transport::tls::QuicTls`（`config: Arc<rustls::ClientConfig>`、`server_name: String`）与 `rurge_proto::build::quic_tls(tls: &TlsOpts, server: &HostName, default_alpn: &[&str], keystore: &[KeystoreItem], roots: Arc<RootCertStore>) -> Result<QuicTls, BuildError>`；`rurge_proto_quic::session`：`Dialer::new(connector, server: Target, tls: &TlsOpts, quic: &QuicOpts, ecn: Tristate, keystore, roots) -> Result<Dialer, BuildError>`、`trait Protocol { type State; const NAME; const NO_ERROR; fn handshake(&self, &Connection) -> BoxFuture<Result<State, OutboundError>>; fn is_going_away(&self, &State) -> bool }`、`Session::new(policy: &str, dialer: Dialer, protocol: P)`、`Session::request(opts, attempt: impl FnMut(Lease<State>) -> BoxFuture<Result<T, Failure>>)`、`Session::lease(opts)`、`Session::serves(&Lease) -> bool`、`Lease::{connection, state}`、`enum Failure { Lost(..), Failed(..) }` 与 `Failure::lost(&ConnectionError)`；测试设施 `rurge_proto_quic::testing::ServerPort`（同一端口上重启、共用重置密钥，得到真正的 stateless reset）

- [ ] **Step 1: 先写 rurge-proto 的用例**

`crates/rurge-proto/src/transport/tls.rs`——把

```rust
        assert_eq!(last_seen(&fixture, 2).await.alpn.as_deref(), Some("h2"));
    }

    #[test]
    fn unusable_names_are_build_errors() {
```

换成

```rust
        assert_eq!(last_seen(&fixture, 2).await.alpn.as_deref(), Some("h2"));
    }

    /// What quinn is given: the server's name or address, or the name `sni`
    /// gives — rustls sends no SNI for an address —, and nothing to send
    /// with `sni=off`. ALPN is chosen as for `TlsClient`.
    #[test]
    fn quic_tls_names_the_server_as_quinn_takes_it() {
        let roots = TlsFixture::new(&["proxy.test"]).roots();
        let build = |opts: &TlsOpts, host: &str| {
            QuicTls::build(opts, &HostName::parse(host), &["h3"], None, roots.clone()).unwrap()
        };
        let sni = |sni: Sni| TlsOpts {
            sni,
            ..TlsOpts::default()
        };
        for (opts, host, name, enable_sni) in [
            (TlsOpts::default(), "proxy.test", "proxy.test", true),
            (TlsOpts::default(), "192.0.2.1", "192.0.2.1", true),
            (TlsOpts::default(), "[2001:db8::1]", "2001:db8::1", true),
            (
                sni(Sni::Name("front.test".into())),
                "192.0.2.1",
                "front.test",
                true,
            ),
            (sni(Sni::Off), "proxy.test", "proxy.test", false),
        ] {
            let quic = build(&opts, host);
            assert_eq!(quic.server_name, name, "{host}");
            assert_eq!(quic.config.enable_sni, enable_sni, "{host}");
        }
        assert_eq!(
            build(&TlsOpts::default(), "proxy.test")
                .config
                .alpn_protocols,
            [b"h3".to_vec()]
        );
        let own = TlsOpts {
            alpn: vec!["hq-29".into(), "h3".into()],
            ..TlsOpts::default()
        };
        assert_eq!(
            build(&own, "proxy.test").config.alpn_protocols,
            [b"hq-29".to_vec(), b"h3".to_vec()]
        );
    }

    /// QUIC's TLS is 1.3 alone: a server that speaks only 1.2 is refused.
    #[tokio::test]
    async fn quic_tls_speaks_tls13_alone() {
        let fixture = TlsFixture::new(&["localhost"]);
        let quic = QuicTls::build(
            &TlsOpts::default(),
            &HostName::parse("localhost"),
            &[],
            None,
            fixture.roots(),
        )
        .unwrap();
        for (version, works) in [
            (&rustls::version::TLS12, false),
            (&rustls::version::TLS13, true),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let acceptor = fixture.camouflage_acceptor(&[version], 0);
            let server = tokio::spawn(async move {
                let (tcp, _) = listener.accept().await.unwrap();
                acceptor.accept(tcp).await.is_ok()
            });
            let name = ServerName::try_from(quic.server_name.clone()).unwrap();
            let tcp = TcpStream::connect(addr).await.unwrap();
            let client = TlsConnector::from(quic.config.clone())
                .connect(name, tcp)
                .await;
            assert_eq!(client.is_ok(), works, "{:?}", version.version);
            assert_eq!(server.await.unwrap(), works, "{:?}", version.version);
        }
    }

    #[test]
    fn unusable_names_are_build_errors() {
```

`crates/rurge-proto/src/build.rs`——把

```rust
        assert_eq!(err.message, "keystore item `missing` does not exist");
    }

    #[test]
    fn a_broken_client_cert_surfaces_at_build_time() {
```

换成

```rust
        assert_eq!(err.message, "keystore item `missing` does not exist");
    }

    /// QUIC's TLS takes `client-cert` from the keystore as `tls_client`
    /// does: the server that asks for one gets it.
    #[tokio::test]
    async fn quic_tls_presents_the_keystores_client_certificate() {
        use crate::testing::TlsFixture;
        use rustls::pki_types::ServerName;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let fixture = TlsFixture::new(&["localhost"]);
        let addr = fixture.spawn_echo(true).await;
        let item = KeystoreItem {
            name: "mtls".into(),
            kind: KeystoreType::P12,
            base64: fixture.client_p12_base64("quic client"),
            password: Some("pw".into()),
            unknown: Vec::new(),
            span: Span::new(Arc::from(Path::new("p.conf")), 1),
        };
        let opts = TlsOpts {
            client_cert: Some("mtls".to_string()),
            ..TlsOpts::default()
        };
        let localhost = HostName::parse("localhost");
        let quic = quic_tls(&opts, &localhost, &[], &[item], fixture.roots()).unwrap();
        let name = ServerName::try_from(quic.server_name.clone()).unwrap();
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut stream = tokio_rustls::TlsConnector::from(quic.config)
            .connect(name, tcp)
            .await
            .unwrap();
        stream.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
        assert!(fixture.seen_at_least(1).await[0].client_cert);
        let err = quic_tls(&opts, &localhost, &[], &[], no_roots())
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.message, "keystore item `mtls` does not exist");
    }

    #[test]
    fn a_broken_client_cert_surfaces_at_build_time() {
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto --lib --features testing`
Expected: FAIL——`QuicTls` 与 `quic_tls` 由 Step 3 引入，编译不过：

```text
error[E0425]: cannot find function `quic_tls` in this scope
   --> crates\rurge-proto\src\build.rs:244:20
error[E0425]: cannot find function `quic_tls` in this scope
   --> crates\rurge-proto\src\build.rs:256:19
error[E0433]: failed to resolve: use of undeclared type `QuicTls`
   --> crates\rurge-proto\src\transport\tls.rs:402:13
error[E0433]: failed to resolve: use of undeclared type `QuicTls`
   --> crates\rurge-proto\src\transport\tls.rs:444:20
Some errors have detailed explanations: E0425, E0433.
For more information about an error, try `rustc --explain E0425`.
error: could not compile `rurge-proto` (lib test) due to 4 previous errors
```

- [ ] **Step 3: QUIC 的 TLS**

`crates/rurge-proto/src/transport/tls.rs`——把

```rust
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
```

换成

```rust
use rustls::{
    ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme, SupportedProtocolVersion,
};
```

`crates/rurge-proto/src/transport/tls.rs`——把

```rust
        .map_err(|_| BuildError::new(format!("`{name}` is not a valid TLS server name")))
}

/// Everything that can be prepared ahead of a connection: built once per
```

换成

```rust
        .map_err(|_| BuildError::new(format!("`{name}` is not a valid TLS server name")))
}

/// The client configuration of a TLS layer on `versions`, and the name it
/// knows the server by: what `TlsClient` and `QuicTls` share.
fn client_config(
    opts: &TlsOpts,
    server: &HostName,
    default_alpn: &[&str],
    identity: Option<ClientIdentity>,
    roots: Arc<RootCertStore>,
    versions: &[&'static SupportedProtocolVersion],
) -> Result<(ClientConfig, ServerName<'static>), BuildError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let name = match (&opts.sni, server) {
        (Sni::Name(custom), _) => dns_name(custom)?,
        (_, HostName::Domain(d)) => dns_name(d)?,
        // rustls sends no SNI for an IP address
        (_, HostName::Ip(ip)) => ServerName::IpAddress((*ip).into()),
    };
    // parsed whatever the mode: a name that is not one is a build error
    let verify_name = opts.verify_name.as_deref().map(dns_name).transpose()?;
    let mode = if let Some(fingerprint) = opts.fingerprint_sha256 {
        Mode::Pinned(fingerprint)
    } else if opts.skip_cert_verify {
        Mode::Insecure
    } else {
        let inner = WebPkiServerVerifier::builder_with_provider(roots, provider.clone())
            .build()
            .map_err(|e| BuildError::new(format!("cannot set up certificate verification: {e}")))?;
        Mode::Standard { inner, verify_name }
    };
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(versions)
        .map_err(|e| BuildError::new(format!("cannot set up TLS: {e}")))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Verifier { mode, provider }));
    let mut config = match identity {
        Some(identity) => builder
            .with_client_auth_cert(identity.chain, identity.key)
            .map_err(|e| BuildError::new(format!("the client certificate cannot be used: {e}")))?,
        None => builder.with_no_client_auth(),
    };
    config.enable_sni = opts.sni != Sni::Off;
    config.alpn_protocols = if opts.alpn.is_empty() {
        default_alpn.iter().map(|p| p.as_bytes().to_vec()).collect()
    } else {
        opts.alpn.iter().map(|p| p.as_bytes().to_vec()).collect()
    };
    Ok((config, name))
}

/// Everything that can be prepared ahead of a connection: built once per
```

`crates/rurge-proto/src/transport/tls.rs`——把

```rust
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let name = match (&opts.sni, server) {
            (Sni::Name(custom), _) => dns_name(custom)?,
            (_, HostName::Domain(d)) => dns_name(d)?,
            // rustls sends no SNI for an IP address
            (_, HostName::Ip(ip)) => ServerName::IpAddress((*ip).into()),
        };
        // parsed whatever the mode: a name that is not one is a build error
        let verify_name = opts.verify_name.as_deref().map(dns_name).transpose()?;
        let mode = if let Some(fingerprint) = opts.fingerprint_sha256 {
            Mode::Pinned(fingerprint)
        } else if opts.skip_cert_verify {
            Mode::Insecure
        } else {
            let inner = WebPkiServerVerifier::builder_with_provider(roots, provider.clone())
                .build()
                .map_err(|e| {
                    BuildError::new(format!("cannot set up certificate verification: {e}"))
                })?;
            Mode::Standard { inner, verify_name }
        };
        let builder = ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| BuildError::new(format!("cannot set up TLS: {e}")))?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Verifier { mode, provider }));
        let mut config = match identity {
            Some(identity) => builder
                .with_client_auth_cert(identity.chain, identity.key)
                .map_err(|e| {
                    BuildError::new(format!("the client certificate cannot be used: {e}"))
                })?,
            None => builder.with_no_client_auth(),
        };
        config.enable_sni = opts.sni != Sni::Off;
        config.alpn_protocols = if opts.alpn.is_empty() {
            default_alpn.iter().map(|p| p.as_bytes().to_vec()).collect()
        } else {
            opts.alpn.iter().map(|p| p.as_bytes().to_vec()).collect()
        };
```

换成

```rust
        let (config, name) = client_config(
            opts,
            server,
            default_alpn,
            identity,
            roots,
            rustls::DEFAULT_VERSIONS,
        )?;
```

`crates/rurge-proto/src/transport/tls.rs`——把

```rust
        Ok((Box::new(tls), alpn))
```

换成

```rust
        Ok((Box::new(tls), alpn))
    }
}

/// The TLS of a QUIC connection (phase 2 M7 design 3.2): what `TlsClient`
/// sets up — verification, `sni`, ALPN, a client certificate — on TLS 1.3
/// alone, as QUIC requires (quinn's handshake panics on a configuration
/// without it). Built once per outbound.
pub struct QuicTls {
    pub config: Arc<ClientConfig>,
    /// The name quinn is given for the server: TLS verifies it and sends it
    /// as SNI — none for an IP address, a server written as one without
    /// `sni=`.
    pub server_name: String,
}

impl QuicTls {
    /// As `TlsClient::build`.
    pub fn build(
        opts: &TlsOpts,
        server: &HostName,
        default_alpn: &[&str],
        identity: Option<ClientIdentity>,
        roots: Arc<RootCertStore>,
    ) -> Result<QuicTls, BuildError> {
        let (config, name) = client_config(
            opts,
            server,
            default_alpn,
            identity,
            roots,
            &[&rustls::version::TLS13],
        )?;
        Ok(QuicTls {
            config: Arc::new(config),
            server_name: name.to_str().into_owned(),
        })
```

`crates/rurge-proto/src/build.rs`——把

```rust
use crate::transport::tls::TlsClient;
```

换成

```rust
use crate::transport::tls::{ClientIdentity, QuicTls, TlsClient};
```

`crates/rurge-proto/src/build.rs`——把

```rust
    let identity = match &opts.client_cert {
        None => None,
        Some(name) => {
            let item = keystore
                .iter()
                .find(|k| &k.name == name)
                .ok_or_else(|| BuildError::new(format!("keystore item `{name}` does not exist")))?;
            Some(decode_p12(item)?)
        }
    };
    TlsClient::build(opts, server, default_alpn, identity, roots).map(Some)
```

换成

```rust
    let identity = client_identity(opts, keystore)?;
    TlsClient::build(opts, server, default_alpn, identity, roots).map(Some)
}

/// The TLS of a QUIC policy (phase 2 M7 design 3.2): as `tls_client`, on
/// TLS 1.3 alone.
pub fn quic_tls(
    opts: &TlsOpts,
    server: &HostName,
    default_alpn: &[&str],
    keystore: &[KeystoreItem],
    roots: Arc<RootCertStore>,
) -> Result<QuicTls, BuildError> {
    let identity = client_identity(opts, keystore)?;
    QuicTls::build(opts, server, default_alpn, identity, roots)
}

/// The `client-cert` of `opts`, looked up in `keystore` and decoded.
fn client_identity(
    opts: &TlsOpts,
    keystore: &[KeystoreItem],
) -> Result<Option<ClientIdentity>, BuildError> {
    let Some(name) = &opts.client_cert else {
        return Ok(None);
    };
    let item = keystore
        .iter()
        .find(|k| &k.name == name)
        .ok_or_else(|| BuildError::new(format!("keystore item `{name}` does not exist")))?;
    decode_p12(item).map(Some)
```

Run: `cargo test -p rurge-proto --lib --features testing` → 通过（新增 3 条：只用 TLS 1.3、ALPN 与 SNI、IP 服务器不发 SNI、Keystore 里找不到证书的报错）。

- [ ] **Step 4: 新 crate 与会话**

`Cargo.toml`——把

```toml
quinn = { version = "0.11.12", default-features = false, features = ["runtime-tokio", "rustls-ring"] }
```

换成

```toml
quinn = { version = "0.11.12", default-features = false, features = ["runtime-tokio", "rustls-ring"] }
rurge-proto-quic = { path = "crates/rurge-proto-quic" }
```

新建 `crates/rurge-proto-quic/Cargo.toml`：

```toml
[package]
name = "rurge-proto-quic"
description = "The QUIC outbounds of rurge: one QUIC connection per policy, shared by its requests"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
rurge-config.workspace = true
rurge-net.workspace = true
rurge-proto.workspace = true
quinn.workspace = true
rustls.workspace = true
tokio.workspace = true
tokio-util.workspace = true
tracing.workspace = true

[dev-dependencies]
rurge-net = { workspace = true, features = ["testing"] }

[features]
# Loopback QUIC servers (`rurge_proto_quic::testing`); enabled by dependants' dev-dependencies.
testing = ["rurge-net/testing"]

[lints]
workspace = true
```

新建 `crates/rurge-proto-quic/src/lib.rs`：

```rust
//! The outbounds over QUIC (phase 2 M7 design): a policy keeps one QUIC
//! connection, which its requests share (`session`).

pub mod session;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use session::{Dialer, Failure, Lease, Protocol, Session};
```

新建 `crates/rurge-proto-quic/src/session.rs`：

```rust
//! A policy's QUIC connection, shared by its requests (phase 2 M7 design
//! 3.5).
//!
//! - The connection is dialed when a request first needs one, and the
//!   requests that come meanwhile wait for that dial: the path to the
//!   server, QUIC's handshake, then the protocol's own
//!   (`Protocol::handshake`: Hysteria 2's authentication). Its failure is
//!   theirs too.
//! - A connection that has closed — an error, the idle timeout, the
//!   server's close, the loss of the UDP path under it (which quinn reports
//!   as a local close) — or that its protocol state says is going away (an
//!   HTTP/3 GOAWAY) takes no new requests: the next one dials. The requests
//!   under way on it go on; their leases keep it.
//! - So does a connection dialled through an underlying policy that leads
//!   elsewhere now (`Connector::route`: a group's member changed, a reload
//!   rebuilt the policy), for good: the next request dials the new way.
//!   Looked at whenever a connection is picked — by a request, or by a UDP
//!   session for its next datagram (`Session::serves`) —, never by a timer.
//! - A connection that no request or UDP session has held (`Lease`) for
//!   `IDLE_TIMEOUT` is closed: looked at on every request and by a reaper.
//! - A request on a connection that was up when it came, which finds the
//!   connection gone before any of the request's own data went out, goes
//!   again, once, on a new connection (`Session::request`): a server that
//!   dropped the connection while it idled answers the request's first
//!   packet with a stateless reset.
//! - Nothing here bounds how long a request takes: the caller's
//!   `opts.timeout` covers the dial, both handshakes and the request.

use quinn::{Connection, ConnectionError, VarInt};
use rurge_config::KeystoreItem;
use rurge_config::spec::{PortHopping, QuicOpts, TlsOpts, Tristate};
use rurge_net::BoxFuture;
use rurge_net::connector::{ConnectOpts, Connector, Route, Target};
use rurge_net::quic::{self, Path, Transport};
use rurge_proto::build::quic_tls;
use rurge_proto::{BuildError, OutboundError};
use rustls::RootCertStore;
use std::io;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::task::AbortOnDropHandle;

/// How long a connection may go without a lease before the session closes
/// it (as the HTTP/2 pool, phase 2 M6 design 5.2).
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// How a session's connections are made (design 3.2, 3.3): the UDP path to
/// the server, then QUIC's handshake with the policy's TLS. Built once per
/// outbound.
pub struct Dialer {
    connector: Arc<dyn Connector>,
    server: Target,
    hopping: Option<PortHopping>,
    ecn: Tristate,
    config: quinn::ClientConfig,
    /// What TLS verifies and sends as SNI (none for an IP address).
    server_name: String,
}

impl Dialer {
    /// The path goes as `connector` dials: from sockets of its own when it
    /// dials directly, else over its UDP flow (`underlying-proxy`). `tls` is
    /// the policy's — ALPN `h3` unless it names others — with `client-cert`
    /// from `keystore`.
    pub fn new(
        connector: Arc<dyn Connector>,
        server: Target,
        tls: &TlsOpts,
        quic: &QuicOpts,
        ecn: Tristate,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
    ) -> Result<Dialer, BuildError> {
        let tls = quic_tls(tls, &server.host, &["h3"], keystore, roots)?;
        let config = quic::client_config(tls.config, &Transport::STANDARD)
            .map_err(|e| BuildError::new(e.to_string()))?;
        Ok(Dialer {
            connector,
            server,
            hopping: quic.port_hopping.clone(),
            ecn,
            config,
            server_name: tls.server_name,
        })
    }

    /// A new connection to the server: nothing bounds it here but quinn's
    /// idle timer.
    async fn connect(&self, opts: &ConnectOpts) -> io::Result<Connection> {
        let path = Path::open(
            self.connector.as_ref(),
            &self.server,
            self.hopping.as_ref(),
            self.ecn,
            opts,
        )
        .await?;
        quic::connect(path, &self.config, &self.server_name).await
    }

    /// Where the connector leads now (`Connector::route`): a connection
    /// dialled while it led elsewhere goes the old way.
    fn route(&self) -> Option<Route> {
        self.connector.route()
    }
}

/// What a protocol does on each new connection before any request uses it,
/// and what the connection's requests then share.
pub trait Protocol: Send + Sync + 'static {
    /// One connection's: what its handshake set up or learned (Hysteria 2:
    /// its HTTP/3 client, which must live as long as the connection, and
    /// whether the server relays UDP). Dropped with the connection.
    type State: Send + Sync + 'static;

    /// The protocol's name, as its log lines begin (`hysteria2`).
    const NAME: &'static str;

    /// The application error code a connection is closed with when the
    /// session lets it go (Hysteria 2: HTTP/3's H3_NO_ERROR).
    const NO_ERROR: VarInt;

    /// Runs on every new connection once QUIC's handshake is done. Its
    /// failure is the dial's, shared by the requests that waited for it;
    /// the connection is then closed with `NO_ERROR`, unless the protocol
    /// closed it with a code of its own.
    fn handshake<'a>(
        &'a self,
        connection: &'a Connection,
    ) -> BoxFuture<'a, Result<Self::State, OutboundError>>;

    /// Whether `state` says the connection takes no new requests (an HTTP/3
    /// GOAWAY); those under way go on.
    fn is_going_away(&self, state: &Self::State) -> bool {
        let _ = state;
        false
    }
}

/// A hold on one of the session's connections, for one request or UDP
/// session: the connection lives at least as long, and is not idle while
/// it does.
pub struct Lease<S> {
    conn: Arc<Conn<S>>,
}

impl<S> Lease<S> {
    pub fn connection(&self) -> &Connection {
        &self.conn.quic
    }

    /// What the protocol's handshake left for the connection's requests.
    pub fn state(&self) -> &S {
        &self.conn.state
    }
}

impl<S> Drop for Lease<S> {
    fn drop(&mut self) {
        let mut usage = self.conn.usage.lock().expect("usage");
        usage.leases -= 1;
        if usage.leases == 0 {
            usage.idle_since = Instant::now();
        }
    }
}

/// How a request on one of the session's connections failed.
#[derive(Debug)]
pub enum Failure {
    /// The connection could not take the request — it turned out closed,
    /// or going away — and nothing of the request's own data went out: on
    /// another connection the request may yet go through.
    Lost(OutboundError),
    /// Final.
    Failed(OutboundError),
}

impl Failure {
    /// The connection's end, met by a request before any of its data went
    /// out (quinn's `ConnectionLost` on one of its streams).
    pub fn lost(e: &ConnectionError) -> Failure {
        Failure::Lost(quic::connection_error(e).into())
    }

    fn into_error(self) -> OutboundError {
        match self {
            Failure::Lost(e) | Failure::Failed(e) => e,
        }
    }
}

impl From<OutboundError> for Failure {
    fn from(e: OutboundError) -> Failure {
        Failure::Failed(e)
    }
}

/// A policy's QUIC connection, shared by its requests (design 3.5). No
/// `Debug`: the protocol may hold credentials.
pub struct Session<P: Protocol> {
    dialer: Dialer,
    shared: Arc<Shared<P>>,
    /// A dial ended, or its request gave up on it: the requests waiting for
    /// one look again.
    dialed: Notify,
    /// Started by the first request: building an outbound (a dry build
    /// included) leaves no task behind.
    reaper: OnceLock<AbortOnDropHandle<()>>,
}

/// What the session's requests and its reaper share.
struct Shared<P: Protocol> {
    /// The policy's name, for the log.
    policy: String,
    protocol: P,
    /// How long a connection may go without a lease.
    idle: Duration,
    slot: Mutex<Slot<P::State>>,
}

/// The session's connection and its dial.
struct Slot<S> {
    /// The connection new requests go to.
    current: Option<Arc<Conn<S>>>,
    /// A dial is in flight.
    dialing: bool,
    /// Dials that have ended, success or failure.
    dials: u64,
    /// The latest dial's failure and the `dials` it ended as; `None` after
    /// a success.
    failure: Option<(u64, OutboundError)>,
}

impl<P: Protocol> Session<P> {
    /// `policy`'s session; dials nothing yet: the first request does.
    pub fn new(policy: &str, dialer: Dialer, protocol: P) -> Session<P> {
        Session::with_idle(policy, dialer, protocol, IDLE_TIMEOUT)
    }

    fn with_idle(policy: &str, dialer: Dialer, protocol: P, idle: Duration) -> Session<P> {
        Session {
            dialer,
            shared: Arc::new(Shared {
                policy: policy.to_string(),
                protocol,
                idle,
                slot: Mutex::new(Slot {
                    current: None,
                    dialing: false,
                    dials: 0,
                    failure: None,
                }),
            }),
            dialed: Notify::new(),
            reaper: OnceLock::new(),
        }
    }

    /// A lease on the session's connection, dialing one when there is none
    /// (once for all the requests that come meanwhile).
    pub async fn lease(&self, opts: &ConnectOpts) -> Result<Lease<P::State>, OutboundError> {
        self.take(opts).await.map(|(lease, _)| lease)
    }

    /// Whether `lease`'s connection still takes what is new — a request, a
    /// UDP session's next datagram —: open, not going away, and dialled the
    /// way the connector leads now. A UDP session on one that does not
    /// moves to the session's connection (`lease` again); what is under
    /// way on it goes on.
    pub fn serves(&self, lease: &Lease<P::State>) -> bool {
        let conn = &lease.conn;
        conn.quic.close_reason().is_none()
            && !self.shared.protocol.is_going_away(&conn.state)
            && self.shared.on_route(conn, self.dialer.route().as_ref())
    }

    /// Runs `attempt` on a lease of the session's connection; what it
    /// returns may keep the lease (a stream). A request that finds a
    /// connection that was up when it came gone before any of its own data
    /// went out (`Failure::Lost`) is attempted again, once, on a new
    /// connection; on a connection dialed for it, it fails. Either way the
    /// connection found gone takes no new requests.
    pub async fn request<'a, T>(
        &'a self,
        opts: &'a ConnectOpts,
        mut attempt: impl FnMut(Lease<P::State>) -> BoxFuture<'a, Result<T, Failure>>,
    ) -> Result<T, OutboundError> {
        let (lease, taken) = self.take(opts).await?;
        let conn = lease.conn.clone();
        let e = match attempt(lease).await {
            Err(Failure::Lost(e)) => e,
            done => return done.map_err(Failure::into_error),
        };
        self.forget(&conn);
        drop(conn);
        if !taken {
            return Err(e);
        }
        // the error says its layer (`quic: …`, or the protocol's)
        tracing::debug!("{e}; the request goes again on a new connection");
        let (lease, _) = self.take(opts).await?;
        attempt(lease).await.map_err(Failure::into_error)
    }

    /// A lease on a connection that takes requests, dialing one when there
    /// is none; and whether the connection was up when the request came,
    /// rather than dialed for it or for requests it waited with.
    async fn take(&self, opts: &ConnectOpts) -> Result<(Lease<P::State>, bool), OutboundError> {
        self.reaper.get_or_init(|| spawn_reaper(&self.shared));
        // the dials that had ended when this request last began to wait
        let mut waited_since = None;
        loop {
            // registered before the look: a dial that ends after it wakes us
            let dialed = self.dialed.notified();
            let mut dialed = pin!(dialed);
            dialed.as_mut().enable();
            // where the connector leads now, asked outside the lock
            let route = self.dialer.route();
            {
                let mut slot = self.shared.slot.lock().expect("session");
                if let Some(lease) = self.shared.pick(&mut slot, route.as_ref()) {
                    return Ok((lease, waited_since.is_none()));
                }
                if let Some(since) = waited_since
                    && !slot.dialing
                    && let Some((at, failure)) = &slot.failure
                    && *at > since
                {
                    return Err(copy_error(failure));
                }
                if !slot.dialing {
                    slot.dialing = true;
                    break;
                }
                waited_since = Some(slot.dials);
            }
            dialed.await;
        }
        self.dial(opts).await.map(|lease| (lease, false))
    }

    /// A new connection and its first lease. The dial stays in flight until
    /// it ends or its request gives up on it.
    async fn dial(&self, opts: &ConnectOpts) -> Result<Lease<P::State>, OutboundError> {
        let dialing = Dialing(self);
        let result = self.connect(opts).await;
        let lease = {
            let mut slot = self.shared.slot.lock().expect("session");
            slot.dials += 1;
            match result {
                Ok(conn) => {
                    slot.failure = None;
                    let conn = Arc::new(conn);
                    let lease = conn.lease();
                    slot.current = Some(conn);
                    Ok(lease)
                }
                Err(e) => {
                    slot.failure = Some((slot.dials, copy_error(&e)));
                    Err(e)
                }
            }
        };
        drop(dialing);
        lease
    }

    /// QUIC's handshake over the path to the server, then the protocol's.
    async fn connect(&self, opts: &ConnectOpts) -> Result<Conn<P::State>, OutboundError> {
        // asked before the dial: should the connector lead elsewhere while
        // it goes on, the next request finds the connection off the way,
        // never the other way round
        let route = self.dialer.route();
        let quic = self.dialer.connect(opts).await?;
        match self.shared.protocol.handshake(&quic).await {
            Ok(state) => Ok(Conn {
                quic,
                state,
                route,
                rerouted: AtomicBool::new(false),
                usage: Mutex::new(Usage {
                    leases: 0,
                    idle_since: Instant::now(),
                }),
                no_error: P::NO_ERROR,
            }),
            Err(e) => {
                close(&quic, P::NO_ERROR);
                Err(e)
            }
        }
    }

    /// Lets go of `conn` if requests still go to it.
    fn forget(&self, conn: &Arc<Conn<P::State>>) {
        let mut slot = self.shared.slot.lock().expect("session");
        if slot
            .current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, conn))
        {
            slot.current = None;
        }
    }

    /// Test support: the reaper's handle, once a request started it.
    #[cfg(test)]
    fn reaper(&self) -> Option<tokio::task::AbortHandle> {
        self.reaper.get().map(AbortOnDropHandle::abort_handle)
    }
}

impl<P: Protocol> Shared<P> {
    /// A lease on the connection a request goes to, when it takes one; a
    /// connection that does not — `prune`'s, or one dialled another way
    /// than the connector leads now (`route`) — is let go of on the way.
    /// The one place a connection is chosen: a cap on the streams of a
    /// connection (M7c's `trust-tunnel` over HTTP/3, design 3.5) makes it
    /// choose among several, the oldest with room first, as the HTTP/2 pool
    /// does.
    fn pick(&self, slot: &mut Slot<P::State>, route: Option<&Route>) -> Option<Lease<P::State>> {
        self.prune(slot, Instant::now());
        if slot
            .current
            .as_ref()
            .is_some_and(|conn| !self.on_route(conn, route))
        {
            slot.current = None;
        }
        slot.current.as_ref().map(Conn::lease)
    }

    /// Lets go of the connection once it takes no new requests: closed,
    /// going away, or without a lease for `idle`. Its leases keep it until
    /// they go too; then it is closed. The reaper's look too, which leaves
    /// where the connector leads to the requests (`pick`).
    fn prune(&self, slot: &mut Slot<P::State>, now: Instant) {
        if slot.current.as_ref().is_some_and(|conn| {
            conn.quic.close_reason().is_some()
                || self.protocol.is_going_away(&conn.state)
                || conn.is_expired(now, self.idle)
        }) {
            slot.current = None;
        }
    }

    /// Whether `conn` goes the way the connector leads now (`route`). Once
    /// it does not — a group's member changed under it, a reload rebuilt
    /// the underlying policy —, it takes nothing new again, whichever way
    /// the connector leads later; the first to see it says so.
    fn on_route(&self, conn: &Conn<P::State>, route: Option<&Route>) -> bool {
        if conn.rerouted.load(Ordering::Relaxed) {
            return false;
        }
        if conn.route.as_ref() == route {
            return true;
        }
        if !conn.rerouted.swap(true, Ordering::Relaxed) {
            tracing::info!(
                policy = %self.policy,
                "{}: the underlying policy changed; a new connection is made",
                P::NAME
            );
        }
        false
    }
}

/// A dial in flight; when it ends, the requests waiting for it look again.
struct Dialing<'a, P: Protocol>(&'a Session<P>);

impl<P: Protocol> Drop for Dialing<'_, P> {
    fn drop(&mut self) {
        self.0.shared.slot.lock().expect("session").dialing = false;
        self.0.dialed.notify_waiters();
    }
}

/// One connection and its protocol state. The session holds it while it
/// takes requests, each lease while the lease lives; when the last of them
/// lets go, the connection is closed.
struct Conn<S> {
    quic: Connection,
    state: S,
    /// Where the connector led when the connection was dialled.
    route: Option<Route>,
    /// The connector has led elsewhere since: nothing new goes to it.
    rerouted: AtomicBool,
    usage: Mutex<Usage>,
    /// `Protocol::NO_ERROR`.
    no_error: VarInt,
}

struct Usage {
    leases: usize,
    /// When `leases` last fell to 0, or the connection came up.
    idle_since: Instant,
}

impl<S> Conn<S> {
    /// Taken under the session's lock: the reaper, which looks under the
    /// same lock, never lets go of a connection a lease was just taken on.
    fn lease(self: &Arc<Self>) -> Lease<S> {
        self.usage.lock().expect("usage").leases += 1;
        Lease { conn: self.clone() }
    }

    fn is_expired(&self, now: Instant, idle: Duration) -> bool {
        let usage = self.usage.lock().expect("usage");
        usage.leases == 0 && now.duration_since(usage.idle_since) >= idle
    }
}

impl<S> Drop for Conn<S> {
    fn drop(&mut self) {
        // before the state goes (Hysteria 2's HTTP/3 client ends its control
        // stream when dropped): the server sees the connection closed, not
        // a broken protocol
        close(&self.quic, self.no_error);
    }
}

/// Closes `connection` with `code`, unless it has ended already: quinn
/// would then record its own close in place of how the connection ended
/// (a stateless reset, the server's close) for every handle still on it.
fn close(connection: &Connection, code: VarInt) {
    if connection.close_reason().is_none() {
        connection.close(code, b"");
    }
}

/// Lets go of the connection once it takes no new requests, every half
/// `idle`, until the session is gone: it holds the session weakly.
fn spawn_reaper<P: Protocol>(shared: &Arc<Shared<P>>) -> AbortOnDropHandle<()> {
    let every = shared.idle / 2;
    let shared = Arc::downgrade(shared);
    AbortOnDropHandle::new(tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let Some(shared) = shared.upgrade() else {
                return;
            };
            shared.prune(&mut shared.slot.lock().expect("session"), Instant::now());
        }
    }))
}

/// The latest dial's failure for the requests that waited for it.
fn copy_error(e: &OutboundError) -> OutboundError {
    match e {
        OutboundError::Reject(kind) => OutboundError::Reject(*kind),
        OutboundError::Unsupported(m) => OutboundError::Unsupported(m.clone()),
        OutboundError::Dns(m) => OutboundError::Dns(m.clone()),
        OutboundError::Io(e) => OutboundError::Io(io::Error::new(e.kind(), e.to_string())),
        OutboundError::Timeout => OutboundError::Timeout,
        OutboundError::Proxy(m) => OutboundError::Proxy(m.clone()),
        OutboundError::Tls(m) => OutboundError::Tls(m.clone()),
        OutboundError::Unavailable(m) => OutboundError::Unavailable(m.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::ServerPort;
    use quinn::crypto::rustls::HandshakeData;
    use quinn::{ReadError, ReadToEndError, WriteError};
    use rurge_config::HostName;
    use rurge_config::spec::Sni;
    use rurge_net::connector::{BoxedDatagram, BoxedStream, Datagram, DirectConnector, Resolve};
    use rurge_net::packet_datagram::packet_datagram;
    use rurge_net::quic::testing::{TestCert, endpoint, loopback_socket};
    use std::net::IpAddr;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::{Context, Poll, Waker};
    use tokio::io::ReadBuf;
    use tokio::time::timeout;

    const BOUND: Duration = Duration::from_secs(10);
    /// HTTP/3's H3_NO_ERROR, as Hysteria 2's.
    const NO_ERROR: VarInt = VarInt::from_u32(0x100);

    /// Answers every name with the loopback.
    struct Loopback;

    impl Resolve for Loopback {
        fn resolve<'a>(&'a self, _host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            Box::pin(std::future::ready(Ok(vec![IpAddr::from([127, 0, 0, 1])])))
        }
    }

    /// The protocol of these tests: a handshake that is one exchange with
    /// the server (as Hysteria 2's authentication) and fails while `refuse`
    /// is set, and a state the tests set to say the connection is going
    /// away.
    #[derive(Default)]
    struct Plain {
        handshakes: AtomicUsize,
        refuse: AtomicBool,
    }

    impl Plain {
        fn handshakes(&self) -> usize {
            self.handshakes.load(Ordering::SeqCst)
        }
    }

    impl Protocol for Plain {
        /// Going away.
        type State = AtomicBool;

        const NAME: &'static str = "plain";

        const NO_ERROR: VarInt = NO_ERROR;

        fn handshake<'a>(
            &'a self,
            connection: &'a Connection,
        ) -> BoxFuture<'a, Result<AtomicBool, OutboundError>> {
            Box::pin(async move {
                self.handshakes.fetch_add(1, Ordering::SeqCst);
                let (mut send, mut recv) = connection
                    .open_bi()
                    .await
                    .map_err(|e| quic::connection_error(&e))?;
                send.write_all(b"hello").await.map_err(io::Error::from)?;
                let _ = send.finish();
                recv.read_to_end(64).await.map_err(io::Error::other)?;
                if self.refuse.load(Ordering::SeqCst) {
                    return Err(OutboundError::Proxy("plain: refused".into()));
                }
                Ok(AtomicBool::new(false))
            })
        }

        fn is_going_away(&self, state: &AtomicBool) -> bool {
            state.load(Ordering::SeqCst)
        }
    }

    /// The far end: endpoints on one `ServerPort` that echo each
    /// bidirectional stream once the client has finished its side, and keep
    /// the connections they accepted.
    struct Server {
        port: ServerPort,
        cert: TestCert,
        accepted: Arc<Mutex<Vec<Connection>>>,
        accepting: Mutex<Vec<AbortOnDropHandle<()>>>,
    }

    impl Server {
        fn spawn() -> Server {
            let server = Server {
                port: ServerPort::bind(),
                cert: TestCert::new(),
                accepted: Arc::default(),
                accepting: Mutex::default(),
            };
            server.restart();
            server
        }

        /// A new endpoint takes the port over, as after a restart: the
        /// connections of the one before are gone, and the client learns it
        /// from a stateless reset on its next packet.
        fn restart(&self) {
            let endpoint = self.port.endpoint(self.cert.server_config(&["h3"]));
            let accepted = self.accepted.clone();
            let task = tokio::spawn(async move {
                while let Some(incoming) = endpoint.accept().await {
                    let accepted = accepted.clone();
                    tokio::spawn(async move {
                        let Ok(connection) = incoming.await else {
                            return;
                        };
                        accepted.lock().unwrap().push(connection.clone());
                        while let Ok((mut send, mut recv)) = connection.accept_bi().await {
                            tokio::spawn(async move {
                                if let Ok(data) = recv.read_to_end(1 << 16).await
                                    && send.write_all(&data).await.is_ok()
                                {
                                    let _ = send.finish();
                                }
                            });
                        }
                    });
                }
            });
            self.accepting
                .lock()
                .unwrap()
                .push(AbortOnDropHandle::new(task));
        }

        /// The connections accepted so far, once there are `count`.
        async fn accepted(&self, count: usize) -> Vec<Connection> {
            let deadline = Instant::now() + BOUND;
            loop {
                let accepted = self.accepted.lock().unwrap().clone();
                if accepted.len() >= count {
                    return accepted;
                }
                assert!(Instant::now() < deadline, "{} accepted", accepted.len());
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
    }

    fn dialer(port: u16, host: &str, tls: &TlsOpts, roots: Arc<RootCertStore>) -> Dialer {
        let direct = Arc::new(DirectConnector::new(Arc::new(Loopback)));
        dialer_via(direct, port, host, tls, roots)
    }

    fn dialer_via(
        connector: Arc<dyn Connector>,
        port: u16,
        host: &str,
        tls: &TlsOpts,
        roots: Arc<RootCertStore>,
    ) -> Dialer {
        Dialer::new(
            connector,
            Target::new(HostName::parse(host), port),
            tls,
            &QuicOpts::default(),
            Tristate::Auto,
            &[],
            roots,
        )
        .expect("a dialer")
    }

    fn session(server: &Server, protocol: Plain) -> Session<Plain> {
        let dialer = dialer(
            server.port.addr().port(),
            "127.0.0.1",
            &TlsOpts::default(),
            server.cert.roots(),
        );
        Session::new("P", dialer, protocol)
    }

    /// The UDP flows of a chain to the server (`underlying-proxy`), one per
    /// connection, which the tests can cut.
    #[derive(Default)]
    struct Chain {
        flows: Mutex<Vec<Arc<Cut>>>,
    }

    #[derive(Default)]
    struct Cut {
        done: AtomicBool,
        reader: Mutex<Option<Waker>>,
    }

    impl Cut {
        fn now(&self) {
            self.done.store(true, Ordering::SeqCst);
            if let Some(reader) = self.reader.lock().unwrap().take() {
                reader.wake();
            }
        }
    }

    struct Flow(BoxedDatagram, Arc<Cut>);

    impl Datagram for Flow {
        fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
            self.0.poll_send(cx, buf)
        }

        fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            *self.1.reader.lock().unwrap() = Some(cx.waker().clone());
            if self.1.done.load(Ordering::SeqCst) {
                return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
            }
            self.0.poll_recv(cx, buf)
        }
    }

    impl Connector for Chain {
        fn connect<'a>(
            &'a self,
            _target: &'a Target,
            _opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedStream>> {
            Box::pin(std::future::ready(Err(io::Error::other("UDP only"))))
        }

        fn connect_udp<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
            Box::pin(async move {
                let socket = DirectConnector::new(Arc::new(Loopback))
                    .open_udp(opts)
                    .await?;
                let to = socket.resolve(target).await?;
                let cut = Arc::<Cut>::default();
                self.flows.lock().unwrap().push(cut.clone());
                Ok(Box::new(Flow(packet_datagram(socket, to), cut)) as BoxedDatagram)
            })
        }
    }

    /// A stream's failure: the connection's end is `Lost`, anything else
    /// final.
    fn write_failure(e: WriteError) -> Failure {
        match e {
            WriteError::ConnectionLost(e) => Failure::lost(&e),
            e => Failure::Failed(OutboundError::Io(io::Error::other(e))),
        }
    }

    fn read_failure(e: ReadToEndError) -> Failure {
        match e {
            ReadToEndError::Read(ReadError::ConnectionLost(e)) => Failure::lost(&e),
            e => Failure::Failed(OutboundError::Io(io::Error::other(e))),
        }
    }

    /// The requests of these tests: `header` on a stream of its own, echoed
    /// back — the request's own exchange, as the head a proxy protocol
    /// sends before any of the caller's data. Nothing of the caller's has
    /// gone out: a connection found gone meanwhile is `Lost`.
    async fn exchange(lease: Lease<AtomicBool>, header: &[u8]) -> Result<Vec<u8>, Failure> {
        let (mut send, mut recv) = lease
            .connection()
            .open_bi()
            .await
            .map_err(|e| Failure::lost(&e))?;
        send.write_all(header).await.map_err(write_failure)?;
        let _ = send.finish();
        recv.read_to_end(1 << 16).await.map_err(read_failure)
    }

    async fn echo(
        session: &Session<Plain>,
        header: &'static [u8],
    ) -> Result<Vec<u8>, OutboundError> {
        let opts = ConnectOpts::default();
        timeout(
            BOUND,
            session.request(&opts, |lease| Box::pin(exchange(lease, header))),
        )
        .await
        .expect("an echo within the bound")
    }

    /// The client's handle on the connection requests go to now.
    async fn current(session: &Session<Plain>) -> Connection {
        let lease = session.lease(&ConnectOpts::default()).await.unwrap();
        lease.connection().clone()
    }

    /// Closed by the session: with the protocol's code, nothing else.
    async fn closed_by_us(connection: &Connection) {
        let closed = timeout(BOUND, connection.closed()).await.expect("closed");
        assert!(
            matches!(&closed, ConnectionError::ApplicationClosed(close)
                if close.error_code == NO_ERROR && close.reason.is_empty()),
            "{closed:?}"
        );
    }

    #[tokio::test]
    async fn requests_that_come_together_share_one_dial() {
        let server = Server::spawn();
        let session = session(&server, Plain::default());
        let (a, b, c) = tokio::join!(
            echo(&session, b"a"),
            echo(&session, b"b"),
            echo(&session, b"c")
        );
        assert_eq!(
            (a.unwrap(), b.unwrap(), c.unwrap()),
            (b"a".to_vec(), b"b".to_vec(), b"c".to_vec())
        );
        assert_eq!(session.shared.protocol.handshakes(), 1);
        // and the requests after them take the same connection
        assert_eq!(echo(&session, b"d").await.unwrap(), b"d");
        assert_eq!(session.shared.protocol.handshakes(), 1);
        assert_eq!(server.accepted(1).await.len(), 1);
    }

    #[tokio::test]
    async fn a_failed_handshake_fails_every_request_that_waited_and_the_next_dials_again() {
        let server = Server::spawn();
        let protocol = Plain {
            refuse: AtomicBool::new(true),
            ..Plain::default()
        };
        let session = session(&server, protocol);
        let (a, b, c) = tokio::join!(
            echo(&session, b"a"),
            echo(&session, b"b"),
            echo(&session, b"c")
        );
        for result in [a, b, c] {
            assert_eq!(result.unwrap_err().to_string(), "plain: refused");
        }
        assert_eq!(session.shared.protocol.handshakes(), 1);
        // the connection that was refused is closed
        closed_by_us(&server.accepted(1).await[0]).await;
        session
            .shared
            .protocol
            .refuse
            .store(false, Ordering::SeqCst);
        assert_eq!(echo(&session, b"d").await.unwrap(), b"d");
        assert_eq!(session.shared.protocol.handshakes(), 2);
    }

    #[tokio::test]
    async fn a_connection_the_server_closed_is_dialed_again() {
        let server = Server::spawn();
        let session = session(&server, Plain::default());
        assert_eq!(echo(&session, b"a").await.unwrap(), b"a");
        let lease = session.lease(&ConnectOpts::default()).await.unwrap();
        server.accepted(1).await[0].close(VarInt::from_u32(7), b"bye");
        timeout(BOUND, lease.connection().closed())
            .await
            .expect("closed");
        assert!(!session.serves(&lease));
        drop(lease);
        assert_eq!(echo(&session, b"b").await.unwrap(), b"b");
        assert_eq!(session.shared.protocol.handshakes(), 2);
        assert_eq!(server.accepted(2).await.len(), 2);
    }

    /// Over a chain (`underlying-proxy`), a connection whose flow ends has
    /// lost its path, which quinn tells as a close of our own: the next
    /// request dials again, over a new flow.
    #[tokio::test]
    async fn a_connection_that_lost_its_path_is_dialed_again() {
        let server = Server::spawn();
        let chain = Arc::new(Chain::default());
        let dialer = dialer_via(
            chain.clone(),
            server.port.addr().port(),
            "127.0.0.1",
            &TlsOpts::default(),
            server.cert.roots(),
        );
        let session = Session::new("P", dialer, Plain::default());
        assert_eq!(echo(&session, b"a").await.unwrap(), b"a");
        let client = current(&session).await;
        chain.flows.lock().unwrap()[0].now();
        timeout(BOUND, client.closed()).await.expect("closed");
        assert_eq!(echo(&session, b"b").await.unwrap(), b"b");
        assert_eq!(session.shared.protocol.handshakes(), 2);
        assert_eq!(chain.flows.lock().unwrap().len(), 2);
    }

    /// A connection its state says is going away takes no new requests;
    /// the stream under way on it goes on, and when its lease goes the
    /// connection is closed.
    #[tokio::test]
    async fn a_connection_going_away_takes_no_new_requests_and_its_streams_go_on() {
        let server = Server::spawn();
        let session = session(&server, Plain::default());
        let opts = ConnectOpts::default();
        let lease = session.lease(&opts).await.unwrap();
        let (mut send, mut recv) = lease.connection().open_bi().await.unwrap();
        send.write_all(b"under ").await.unwrap();
        assert!(session.serves(&lease));
        lease.state().store(true, Ordering::SeqCst);
        assert!(!session.serves(&lease));
        assert_eq!(echo(&session, b"new").await.unwrap(), b"new");
        assert_eq!(session.shared.protocol.handshakes(), 2);
        send.write_all(b"way").await.unwrap();
        send.finish().unwrap();
        let echoed = timeout(BOUND, recv.read_to_end(64)).await.unwrap().unwrap();
        assert_eq!(echoed, b"under way");
        drop((send, recv, lease));
        closed_by_us(&server.accepted(1).await[0]).await;
    }

    /// Dials through `inner`; where it leads is what `route` holds now, as
    /// a group that is the `underlying-proxy` leads to its member.
    struct Rerouted {
        inner: Arc<dyn Connector>,
        route: Mutex<Arc<u8>>,
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
            Some(Route::of(&self.route.lock().unwrap()))
        }
    }

    /// The underlying policy leads elsewhere now (a group's member changed,
    /// a reload rebuilt it): the connection dialled the old way takes no new
    /// requests — not even once the policy leads back —, the stream under
    /// way on it goes on, and it is closed once its last lease goes; the
    /// next request dials the new way.
    #[tokio::test]
    async fn a_connection_whose_underlying_policy_changed_takes_no_new_requests() {
        let server = Server::spawn();
        let chain = Arc::new(Chain::default());
        let rerouted = Arc::new(Rerouted {
            inner: chain.clone(),
            route: Mutex::new(Arc::new(1)),
        });
        let dialer = dialer_via(
            rerouted.clone(),
            server.port.addr().port(),
            "127.0.0.1",
            &TlsOpts::default(),
            server.cert.roots(),
        );
        let session = Session::new("P", dialer, Plain::default());
        let lease = session.lease(&ConnectOpts::default()).await.unwrap();
        let (mut send, mut recv) = lease.connection().open_bi().await.unwrap();
        send.write_all(b"under ").await.unwrap();
        assert_eq!(echo(&session, b"same way").await.unwrap(), b"same way");
        assert!(session.serves(&lease));
        let first = rerouted.route.lock().unwrap().clone();
        *rerouted.route.lock().unwrap() = Arc::new(2);
        assert!(!session.serves(&lease));
        assert_eq!(echo(&session, b"new way").await.unwrap(), b"new way");
        assert_eq!(session.shared.protocol.handshakes(), 2);
        assert_eq!(chain.flows.lock().unwrap().len(), 2, "dialled anew");
        send.write_all(b"way").await.unwrap();
        send.finish().unwrap();
        let echoed = timeout(BOUND, recv.read_to_end(64)).await.unwrap().unwrap();
        assert_eq!(echoed, b"under way");
        // back the first way: a connection of its own again
        *rerouted.route.lock().unwrap() = first;
        assert!(!session.serves(&lease));
        assert_eq!(echo(&session, b"back").await.unwrap(), b"back");
        assert_eq!(session.shared.protocol.handshakes(), 3);
        drop((send, recv, lease));
        closed_by_us(&server.accepted(1).await[0]).await;
    }

    /// Held, a connection stays past the idle time; let go of, it is closed
    /// once the idle time has passed, and the next request dials again.
    #[tokio::test]
    async fn an_idle_connection_is_closed_once_no_lease_holds_it() {
        const IDLE: Duration = Duration::from_millis(400);
        let server = Server::spawn();
        let dialer = dialer(
            server.port.addr().port(),
            "127.0.0.1",
            &TlsOpts::default(),
            server.cert.roots(),
        );
        let session = Session::with_idle("P", dialer, Plain::default(), IDLE);
        let lease = session.lease(&ConnectOpts::default()).await.unwrap();
        tokio::time::sleep(IDLE * 3).await;
        let accepted = server.accepted(1).await;
        assert!(accepted[0].close_reason().is_none(), "closed while held");
        let client = lease.connection().clone();
        let since = Instant::now();
        drop(lease);
        closed_by_us(&accepted[0]).await;
        assert!(
            since.elapsed() >= IDLE,
            "closed after {:?}",
            since.elapsed()
        );
        assert!(matches!(
            client.close_reason(),
            Some(ConnectionError::LocallyClosed)
        ));
        assert_eq!(echo(&session, b"again").await.unwrap(), b"again");
        assert_eq!(session.shared.protocol.handshakes(), 2);
    }

    /// The server restarts — losing every connection — once the request
    /// has taken the connection: the request finds the connection gone (a
    /// stateless reset answers its first packet) before anything of the
    /// caller's went out, and goes again, once, on a new connection.
    #[tokio::test]
    async fn a_request_that_finds_its_connection_gone_goes_again_once_on_a_new_one() {
        let server = Server::spawn();
        let session = session(&server, Plain::default());
        assert_eq!(echo(&session, b"first").await.unwrap(), b"first");
        let stale = current(&session).await;
        let attempts = AtomicUsize::new(0);
        let opts = ConnectOpts::default();
        let request = session.request(&opts, |lease| {
            let first = attempts.fetch_add(1, Ordering::SeqCst) == 0;
            let server = &server;
            Box::pin(async move {
                if first {
                    server.restart();
                }
                exchange(lease, b"second").await
            })
        });
        let echoed = timeout(BOUND, request).await.unwrap().unwrap();
        assert_eq!(echoed, b"second");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(session.shared.protocol.handshakes(), 2);
        assert!(
            matches!(stale.close_reason(), Some(ConnectionError::Reset)),
            "{:?}",
            stale.close_reason()
        );
    }

    /// Once the caller's data has gone out, a connection found gone is the
    /// request's end: it does not go again.
    #[tokio::test]
    async fn a_request_whose_data_went_out_does_not_go_again() {
        let server = Server::spawn();
        let session = session(&server, Plain::default());
        assert_eq!(echo(&session, b"first").await.unwrap(), b"first");
        let attempts = AtomicUsize::new(0);
        let opts = ConnectOpts::default();
        let request = session.request(&opts, |lease| {
            attempts.fetch_add(1, Ordering::SeqCst);
            let server = &server;
            Box::pin(async move {
                let (mut send, mut recv) = lease
                    .connection()
                    .open_bi()
                    .await
                    .map_err(|e| Failure::lost(&e))?;
                send.write_all(b"the caller's")
                    .await
                    .map_err(write_failure)?;
                // from here on whatever happens is final
                server.restart();
                let _ = send.finish();
                recv.read_to_end(1 << 16)
                    .await
                    .map_err(|e| Failure::Failed(read_failure(e).into_error()))
            })
        });
        let err = timeout(BOUND, request).await.unwrap().unwrap_err();
        assert_eq!(
            err.to_string(),
            "quic: the server no longer knows the connection (stateless reset)"
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(session.shared.protocol.handshakes(), 1);
        // the connection is gone: the next request dials
        assert_eq!(echo(&session, b"next").await.unwrap(), b"next");
        assert_eq!(session.shared.protocol.handshakes(), 2);
    }

    /// A connection dialed for the request is not stale: found gone, it is
    /// the request's end.
    #[tokio::test]
    async fn a_request_on_a_new_connection_found_gone_does_not_go_again() {
        let server = Server::spawn();
        let session = session(&server, Plain::default());
        let attempts = AtomicUsize::new(0);
        let opts = ConnectOpts::default();
        let request = session.request(&opts, |lease| {
            attempts.fetch_add(1, Ordering::SeqCst);
            let server = &server;
            Box::pin(async move {
                server.restart();
                exchange(lease, b"lost").await
            })
        });
        let err = timeout(BOUND, request).await.unwrap().unwrap_err();
        assert_eq!(
            err.to_string(),
            "quic: the server no longer knows the connection (stateless reset)"
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(session.shared.protocol.handshakes(), 1);
        assert_eq!(echo(&session, b"next").await.unwrap(), b"next");
        assert_eq!(session.shared.protocol.handshakes(), 2);
    }

    /// Dropped, the session ends its reaper and closes its connection; a
    /// lease outlives it, and the connection with it.
    #[tokio::test]
    async fn a_dropped_session_ends_its_task_and_its_connection_after_the_last_lease() {
        let server = Server::spawn();
        let session = session(&server, Plain::default());
        let lease = session.lease(&ConnectOpts::default()).await.unwrap();
        let reaper = session.reaper().expect("started by the request");
        drop(session);
        let deadline = Instant::now() + BOUND;
        while !reaper.is_finished() {
            assert!(Instant::now() < deadline, "the reaper still runs");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // the lease still has its connection
        let (mut send, mut recv) = lease.connection().open_bi().await.unwrap();
        send.write_all(b"still here").await.unwrap();
        send.finish().unwrap();
        let echoed = timeout(BOUND, recv.read_to_end(64)).await.unwrap().unwrap();
        assert_eq!(echoed, b"still here");
        drop(lease);
        closed_by_us(&server.accepted(1).await[0]).await;
    }

    /// The SNI and the ALPN the server sees.
    async fn seen(dialer: &Dialer, server: &quinn::Endpoint) -> (Option<String>, Option<Vec<u8>>) {
        let opts = ConnectOpts::default();
        let (client, accepted) = timeout(BOUND, async {
            tokio::join!(dialer.connect(&opts), async {
                server.accept().await.expect("a connection").await
            })
        })
        .await
        .expect("a handshake within the bound");
        client.expect("connected");
        let data = accepted
            .expect("accepted")
            .handshake_data()
            .expect("handshake data")
            .downcast::<HandshakeData>()
            .expect("rustls's");
        (data.server_name, data.protocol)
    }

    /// QUIC's TLS names the server as TLS over TCP does: no SNI for an
    /// address, unless `sni` names one; none with `sni=off`. ALPN is `h3`
    /// unless the policy names others.
    #[tokio::test]
    async fn the_dial_names_the_server_and_its_protocol_as_the_policy_says() {
        let cert = TestCert::new();
        let server = endpoint(cert.server_config(&["h3"]), loopback_socket());
        let port = server.local_addr().unwrap().port();
        let sni = |sni: Sni| TlsOpts {
            sni,
            ..TlsOpts::default()
        };
        for (host, tls, expected) in [
            ("127.0.0.1", TlsOpts::default(), None),
            ("localhost", TlsOpts::default(), Some("localhost")),
            (
                "127.0.0.1",
                sni(Sni::Name("localhost".into())),
                Some("localhost"),
            ),
            ("localhost", sni(Sni::Off), None),
        ] {
            let dialer = dialer(port, host, &tls, cert.roots());
            let (name, alpn) = seen(&dialer, &server).await;
            assert_eq!(name.as_deref(), expected, "{host} {:?}", tls.sni);
            assert_eq!(alpn.as_deref(), Some(&b"h3"[..]));
        }
        let own = TlsOpts {
            alpn: vec!["hq-29".into(), "h3".into()],
            ..TlsOpts::default()
        };
        let (_, alpn) = seen(&dialer(port, "localhost", &own, cert.roots()), &server).await;
        assert_eq!(alpn.as_deref(), Some(&b"h3"[..]));
    }

    /// The server's certificate is checked as the policy says.
    #[tokio::test]
    async fn the_dial_verifies_the_server_as_the_policy_says() {
        let cert = TestCert::new();
        let server = endpoint(cert.server_config(&["h3"]), loopback_socket());
        let port = server.local_addr().unwrap().port();
        let _accepting = AbortOnDropHandle::new(tokio::spawn(async move {
            while let Some(incoming) = server.accept().await {
                tokio::spawn(async move {
                    let _ = incoming.await;
                });
            }
        }));
        let opts = ConnectOpts::default();
        // trusting another certificate of the same names
        let untrusted = dialer(
            port,
            "localhost",
            &TlsOpts::default(),
            TestCert::new().roots(),
        );
        let err = timeout(BOUND, untrusted.connect(&opts))
            .await
            .unwrap()
            .unwrap_err();
        assert!(
            err.to_string()
                .starts_with("tls: invalid peer certificate: "),
            "{err}"
        );
        let insecure = TlsOpts {
            skip_cert_verify: true,
            ..TlsOpts::default()
        };
        let roots = Arc::new(RootCertStore::empty());
        let dialer = dialer(port, "localhost", &insecure, roots);
        timeout(BOUND, dialer.connect(&opts))
            .await
            .unwrap()
            .unwrap();
    }
}
```

新建 `crates/rurge-proto-quic/src/testing.rs`：

```rust
//! Loopback QUIC servers for the tests of this crate and of its dependants
//! (feature `testing`): a server port whose endpoint can be replaced, as
//! by a server that restarts. Never used by production code.

use quinn::udp::{RecvMeta, Transmit};
use quinn::{
    AsyncUdpSocket, ConnectionId, ConnectionIdGenerator, Endpoint, EndpointConfig, ServerConfig,
    TokioRuntime, UdpPoller,
};
use rurge_net::quic::testing::loopback_socket;
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

/// A loopback UDP port for QUIC servers that restart (phase 2 M7 design
/// 3.5: the stale connection). Its endpoints share the key of their
/// stateless resets and the scheme of their connection IDs, as a server
/// restarted with the same configuration: a new endpoint knows none of the
/// connections of the one before and answers their packets with a
/// stateless reset (RFC 9000 10.3), which ends them at the client. The
/// endpoint before hears and sends nothing any more.
pub struct ServerPort {
    socket: Arc<dyn AsyncUdpSocket>,
    config: EndpointConfig,
    /// The way to the port of the endpoint that answers now.
    current: Mutex<Option<Arc<Gate>>>,
}

impl ServerPort {
    /// On `127.0.0.1`, any port.
    pub fn bind() -> ServerPort {
        let mut config = EndpointConfig::default();
        let ids = Arc::new(AtomicU64::new(1));
        config.cid_generator(move || Box::new(SharedIds(ids.clone())));
        ServerPort {
            socket: loopback_socket(),
            config,
            current: Mutex::new(None),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.socket.local_addr().expect("the port's address")
    }

    /// A server endpoint presenting `config` that answers on the port from
    /// now on; the one before it falls silent.
    pub fn endpoint(&self, config: ServerConfig) -> Endpoint {
        let gate = Arc::new(Gate {
            socket: self.socket.clone(),
            open: AtomicBool::new(true),
        });
        let before = self.current.lock().expect("the gate").replace(gate.clone());
        if let Some(before) = before {
            before.open.store(false, Ordering::SeqCst);
        }
        Endpoint::new_with_abstract_socket(
            self.config.clone(),
            Some(config),
            gate,
            Arc::new(TokioRuntime),
        )
        .expect("a server endpoint")
    }
}

/// Connection IDs from a counter that all of a port's endpoints share:
/// never the same twice, and each taken for one of theirs (the trait's own
/// check), so that a packet for a connection an endpoint never had gets a
/// stateless reset instead of being dropped as not its own.
struct SharedIds(Arc<AtomicU64>);

impl ConnectionIdGenerator for SharedIds {
    fn generate_cid(&mut self) -> ConnectionId {
        ConnectionId::new(&self.0.fetch_add(1, Ordering::Relaxed).to_be_bytes())
    }

    fn cid_len(&self) -> usize {
        8
    }

    fn cid_lifetime(&self) -> Option<Duration> {
        None
    }
}

/// One endpoint's way to the port: open, everything passes; closed — for
/// good —, the endpoint hears nothing and what it sends is dropped.
#[derive(Debug)]
struct Gate {
    socket: Arc<dyn AsyncUdpSocket>,
    open: AtomicBool,
}

impl AsyncUdpSocket for Gate {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        self.socket.clone().create_io_poller()
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        if self.open.load(Ordering::SeqCst) {
            self.socket.try_send(transmit)
        } else {
            Ok(())
        }
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        if !self.open.load(Ordering::SeqCst) {
            // never opened again: nothing to wake the endpoint for
            return Poll::Pending;
        }
        self.socket.poll_recv(cx, bufs, meta)
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    fn max_transmit_segments(&self) -> usize {
        self.socket.max_transmit_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.socket.max_receive_segments()
    }

    fn may_fragment(&self) -> bool {
        self.socket.may_fragment()
    }
}
```

要点：
- 连接只在最后一个持有者放手、且仍开着时才关闭（quinn 的 `close` 会把 stateless reset 覆盖成 `LocallyClosed`）；关闭在协议状态（例如 HTTP/3 的 `Client`）丢掉之前，服务端看到的是正常关闭。
- 空闲回收任务第一次取用时才起，弱引用会话、随会话中止；干构建什么也不启动。

- [ ] **Step 5: 运行**

Run: `cargo test -p rurge-proto-quic`
Expected: 通过（13 条：同时到来的第一次取用只拨一次、握手失败交给每个等待者且下一次重拨、服务端关闭后重拨、路径丢失后重拨、空闲关闭只在没有租约时、失效连接重试一次而新拨的连接不重试、数据已发出的请求不重试、正在离开的连接不接新请求而已有的流走完、底层策略变了的连接不再接新请求、会话丢掉后任务结束）。

- [ ] **Step 6: 门禁与提交**

跑门禁。

```bash
git add Cargo.toml Cargo.lock crates/rurge-proto crates/rurge-proto-quic
git commit -m "feat(proto-quic): 新 crate 与 QUIC 会话——单飞拨号与认证、重连、空闲关闭、失效连接重试一次、跟随底层策略；QuicTls"
```

### Task 5: Hysteria 2 的认证与 TCP、`FakeHysteria2`

`Hysteria2Outbound` 的 TCP（P16、P17）：每条连接先经 HTTP/3 `POST https://hysteria/auth`，应答 233 才可用；每个连接一条原始双向流，`0x401` 请求帧、等应答后转发。`FakeHysteria2` 独立实现（quinn 服务端 + Task 3 的 HTTP/3 服务端一侧）。`udp()` 暂用缺省（不支持，Task 6 翻转），`spec.obfs` 暂不读（Task 7）。新模块的用例都在新文件里，没有单独的失败步骤。

**Files:**
- Create: `crates/rurge-proto-quic/src/varint.rs`、`src/hysteria2/{mod.rs, auth.rs, tcp.rs}`（自带用例）、`src/testing/mod.rs`、`src/testing/hysteria2.rs`
- Delete: `crates/rurge-proto-quic/src/testing.rs`（内容移到 `testing/mod.rs`）
- Modify: `crates/rurge-proto/src/outbound.rs`（`untrusted_text` 改为 `pub`）、`crates/rurge-proto-quic/{Cargo.toml, src/lib.rs, src/session.rs}`

**Interfaces:**
- Consumes: Task 1 的 `Hysteria2Spec`；Task 3 的 `h3::Client` 与 `h3::testing`；Task 4 的 `Dialer` / `Session` / `Protocol` / `Failure` / `ServerPort`；既有的 `Outbound` trait 与 `rurge_proto::outbound::untrusted_text`。
- Produces: `rurge_proto_quic::hysteria2::Hysteria2Outbound::new(name: &str, server: Target, spec: &Hysteria2Spec, ecn: Tristate, keystore: &[KeystoreItem], roots: Arc<RootCertStore>, connector: Arc<dyn Connector>) -> Result<Hysteria2Outbound, BuildError>`（实现 `Outbound` 的 TCP）；`rurge_proto_quic::varint`；测试设施 `FakeHysteria2`（`spawn`、在 `MultiPort` 上的 `spawn_on`、`restart` / `restart_at_next_request`、`close_connections`、`auths()`、`tcp_requests()`、`connections()`）与 `Hysteria2Script`

- [ ] **Step 1: 前置改动**

`crates/rurge-proto/src/outbound.rs`——把

```rust
pub(crate) fn untrusted_text(text: &str, max: usize) -> String {
```

换成

```rust
pub fn untrusted_text(text: &str, max: usize) -> String {
```

`crates/rurge-proto-quic/Cargo.toml`——把

```toml
rurge-proto.workspace = true
```

换成

```toml
rurge-proto.workspace = true
getrandom.workspace = true
http.workspace = true
```

`crates/rurge-proto-quic/Cargo.toml`——把

```toml
rurge-net = { workspace = true, features = ["testing"] }
```

换成

```toml
rurge-net = { workspace = true, features = ["testing"] }
rurge-proto = { workspace = true, features = ["testing"] }
```

`crates/rurge-proto-quic/src/lib.rs`——把

```rust
//! connection, which its requests share (`session`).

pub mod session;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use session::{Dialer, Failure, Lease, Protocol, Session};
```

换成

```rust
//! connection, which its requests share (`session`) — Hysteria 2's
//! (`hysteria2`).

pub mod hysteria2;
pub mod session;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
mod varint;

pub use hysteria2::Hysteria2Outbound;
pub use session::{Dialer, Failure, Lease, Protocol, Session};
```

会话的 `close` 给协议用（认证失败时以自己的码关闭）：

`crates/rurge-proto-quic/src/session.rs`——把

```rust
fn close(connection: &Connection, code: VarInt) {
```

换成

```rust
pub(crate) fn close(connection: &Connection, code: VarInt) {
```

- [ ] **Step 2: 出站与假服务端**

新建 `crates/rurge-proto-quic/src/varint.rs`：

```rust
//! QUIC's variable-length integers (RFC 9000 16), as the protocols over
//! QUIC write their own fields with them (Hysteria 2's requests).

/// `value` as a variable-length integer, as short as it goes. At most
/// 2^62 - 1: every value written here is a length or a type.
pub(crate) fn put(out: &mut Vec<u8>, value: u64) {
    debug_assert!(value < 1 << 62, "{value} is no QUIC integer");
    match value {
        0..0x40 => out.push(value as u8),
        0x40..0x4000 => out.extend_from_slice(&(0x4000 | value as u16).to_be_bytes()),
        0x4000..0x4000_0000 => out.extend_from_slice(&(0x8000_0000 | value as u32).to_be_bytes()),
        _ => out.extend_from_slice(&(0xc000_0000_0000_0000 | value).to_be_bytes()),
    }
}

/// How many bytes the integer that starts with `first` takes.
pub(crate) fn len(first: u8) -> usize {
    1 << (first >> 6)
}

/// The integer at the start of `bytes`, in any of its lengths, and how
/// many bytes it took; `None` while `bytes` holds only part of it.
pub(crate) fn get(bytes: &[u8]) -> Option<(u64, usize)> {
    let len = len(*bytes.first()?);
    let bytes = bytes.get(..len)?;
    let value = bytes[1..]
        .iter()
        .fold(u64::from(bytes[0] & 0x3f), |value, &b| {
            value << 8 | u64::from(b)
        });
    Some((value, len))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_one(value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        put(&mut out, value);
        out
    }

    /// RFC 9000 A.1, and the edges of each length.
    #[test]
    fn integers_go_out_shortest_and_come_in_any_length() {
        for (bytes, value) in [
            (
                &[0xc2, 0x19, 0x7c, 0x5e, 0xff, 0x14, 0xe8, 0x8c][..],
                151_288_809_941_952_652,
            ),
            (&[0x9d, 0x7f, 0x3e, 0x7d], 494_878_333),
            (&[0x7b, 0xbd], 15_293),
            (&[0x25], 37),
        ] {
            assert_eq!(get(bytes), Some((value, bytes.len())));
            assert_eq!(put_one(value), bytes);
        }
        // the same 37 in two bytes: read, never written so
        assert_eq!(get(&[0x40, 0x25]), Some((37, 2)));
        for (value, len) in [
            (0, 1),
            (63, 1),
            (64, 2),
            (16_383, 2),
            (16_384, 4),
            ((1 << 30) - 1, 4),
            (1 << 30, 8),
            ((1 << 62) - 1, 8),
        ] {
            let bytes = put_one(value);
            assert_eq!(bytes.len(), len, "{value}");
            assert_eq!(get(&bytes), Some((value, len)));
        }
        // a Hysteria 2 TCP request starts so
        assert_eq!(put_one(0x401), [0x44, 0x01]);
    }

    #[test]
    fn an_integer_needs_all_its_bytes() {
        assert_eq!(get(&[]), None);
        assert_eq!(get(&[0x44]), None);
        assert_eq!(get(&[0x9d, 0x7f, 0x3e]), None);
        // what follows is not the integer's
        assert_eq!(get(&[0x44, 0x01, 0xff]), Some((0x401, 2)));
    }
}
```

新建 `crates/rurge-proto-quic/src/hysteria2/mod.rs`：

```rust
//! `hysteria2` outbound (manual: Policies › Hysteria 2; phase 2 M7 design
//! 5): the policy's QUIC connection, which its requests share (`session`),
//! authenticated over HTTP/3 as it comes up (`auth`).
//!
//! - A TCP connection is a bidirectional stream of its own: the request
//!   names the target as `host:port` (an IPv6 literal in brackets, an IDN
//!   as its A-labels), and the server's answer lets the stream through or
//!   refuses it with a message (`tcp`).
//! - Requests and the authentication are padded as the reference client
//!   pads them: letters and digits, of a random length.

mod auth;
mod tcp;

use crate::session::{Dialer, Session};
use auth::Auth;
use rurge_config::KeystoreItem;
use rurge_config::spec::{Hysteria2Spec, Tristate};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_proto::{BuildError, Outbound, OutboundError};
use rustls::RootCertStore;
use std::ops::RangeInclusive;
use std::sync::Arc;

/// The characters of padding: letters and digits, as the reference's
/// (`core/internal/protocol/padding.go`).
const PADDING: &[u8; 62] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// Padding of a random length in `lengths`, of random letters and digits.
/// Padding, not a secret: without a random source it is the shortest, all
/// `a`.
fn padding(lengths: RangeInclusive<usize>) -> Vec<u8> {
    let (min, max) = lengths.into_inner();
    let span = (max - min + 1) as u32;
    let len = min + getrandom::u32().map_or(0, |pick| (pick % span) as usize);
    let mut padding = vec![0u8; len];
    let _ = getrandom::fill(&mut padding);
    for byte in &mut padding {
        *byte = PADDING[usize::from(*byte) % PADDING.len()];
    }
    padding
}

/// No `Debug`: it holds the password.
pub struct Hysteria2Outbound {
    name: String,
    session: Session<Auth>,
}

impl Hysteria2Outbound {
    /// The connection goes as `connector` dials (design 3.3), with the
    /// policy's `ecn`; `client-cert` comes from `keystore`.
    pub fn new(
        name: &str,
        server: Target,
        spec: &Hysteria2Spec,
        ecn: Tristate,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<Hysteria2Outbound, BuildError> {
        // error texts carry no policy name: the registry's `build_one` and
        // the dry build both prefix it; the configuration checked the
        // password, but a spec made by hand may not be
        let password = spec.password.expose();
        if password.is_empty() {
            return Err(BuildError::new("`password` is empty"));
        }
        let Some(auth) = Auth::new(password, spec.download_bandwidth) else {
            return Err(BuildError::new(
                "`password` cannot be sent in an HTTP field",
            ));
        };
        let dialer = Dialer::new(
            connector, server, &spec.tls, &spec.quic, ecn, keystore, roots,
        )?;
        Ok(Hysteria2Outbound {
            name: name.to_string(),
            session: Session::new(name, dialer, auth),
        })
    }
}

impl Outbound for Hysteria2Outbound {
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
            // handshake, the authentication and the server's answer
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
    use crate::testing::{FakeHysteria2, Hysteria2Script};
    use quinn::{ConnectionError, VarInt};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::hysteria2::read_hysteria2;
    use rurge_config::spec::{CommonOpts, ParamReader, PortHopping};
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_net::quic::testing::MultiPort;
    use rurge_proto::testing::echo_server;
    use std::collections::HashSet;
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::{Instant, timeout};

    const BOUND: Duration = Duration::from_secs(10);

    fn script(password: &str) -> Hysteria2Script {
        Hysteria2Script {
            password: password.to_string(),
            ..Hysteria2Script::default()
        }
    }

    /// The spec of `hysteria2, 127.0.0.1, <port>, <params>`, as the
    /// configuration reads it.
    fn spec(port: u16, params: &str) -> Hysteria2Spec {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let line = format!("hysteria2, 127.0.0.1, {port}, {params}");
        let policy = parse_policy("H", &line, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let spec = read_hysteria2(&mut r, &mut CommonOpts::default(), &[]);
        assert!(!r.has_errors(), "{:?}", r.finish());
        spec
    }

    /// `spec`'s outbound to `127.0.0.1:<port>`, trusting `fake`.
    fn outbound_with(spec: &Hysteria2Spec, port: u16, fake: &FakeHysteria2) -> Hysteria2Outbound {
        Hysteria2Outbound::new(
            "H",
            Target::new(HostName::parse("127.0.0.1"), port),
            spec,
            Tristate::Auto,
            &[],
            fake.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    /// `hysteria2, 127.0.0.1, <the fake's port>, <params>`.
    fn outbound(fake: &FakeHysteria2, params: &str) -> Hysteria2Outbound {
        let port = fake.addr().port();
        outbound_with(&spec(port, params), port, fake)
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    async fn tunnel(out: &Hysteria2Outbound, to: &Target) -> BoxedStream {
        timeout(BOUND, out.connect_tcp(to, &ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .expect("a tunnel")
    }

    async fn failure(out: &Hysteria2Outbound, to: &Target) -> OutboundError {
        timeout(BOUND, out.connect_tcp(to, &ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .err()
            .expect("no tunnel")
    }

    async fn round_trip(stream: &mut BoxedStream, payload: &[u8]) {
        stream.write_all(payload).await.unwrap();
        let mut back = vec![0u8; payload.len()];
        timeout(BOUND, stream.read_exact(&mut back))
            .await
            .expect("the echo arrives")
            .unwrap();
        assert_eq!(back, payload);
    }

    #[test]
    fn padding_is_letters_and_digits_of_a_length_in_range() {
        let mut lengths = HashSet::new();
        for _ in 0..200 {
            let padding = padding(64..=511);
            assert!((64..=511).contains(&padding.len()), "{}", padding.len());
            assert!(padding.iter().all(u8::is_ascii_alphanumeric));
            lengths.insert(padding.len());
        }
        assert!(lengths.len() > 10, "{lengths:?}");
        assert_eq!(padding(7..=7).len(), 7);
    }

    /// Design 5.2: the fields the server reads, and what its answer says of
    /// UDP is kept with the connection.
    #[tokio::test]
    async fn the_authentication_carries_the_password_the_bandwidth_and_padding() {
        let echo = echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw, download-bandwidth=100");
        assert_eq!(out.name(), "H");
        assert!(out.http_forward().is_none());
        let mut stream = tunnel(&out, &target(echo)).await;
        round_trip(&mut stream, b"through hysteria").await;
        let auths = fake.auths();
        assert_eq!(auths.len(), 1);
        let auth = &auths[0];
        for (name, value) in [
            (":method", "POST"),
            (":scheme", "https"),
            (":authority", "hysteria"),
            (":path", "/auth"),
            ("hysteria-auth", "pw"),
            // 100 Mbps in bytes per second
            ("hysteria-cc-rx", "12500000"),
        ] {
            assert_eq!(auth.field(name), Some(value), "{name}");
        }
        let padding = auth.field("hysteria-padding").expect("padding");
        assert!((256..=2047).contains(&padding.len()), "{}", padding.len());
        assert!(padding.bytes().all(|b| b.is_ascii_alphanumeric()));
        assert_eq!(auth.fields.len(), 7, "nothing else");
        let requests = fake.tcp_requests();
        assert_eq!(requests[0].address, echo.to_string());
        assert!((64..=511).contains(&requests[0].padding));
        let lease = out.session.lease(&ConnectOpts::default()).await.unwrap();
        assert!(lease.state().udp);

        // without a bandwidth the rate is unknown; a server that relays no
        // UDP says so
        let fake = FakeHysteria2::spawn(Hysteria2Script {
            no_udp: true,
            ..script("pw")
        });
        let out = outbound(&fake, "password=pw");
        let lease = timeout(BOUND, out.session.lease(&ConnectOpts::default()))
            .await
            .unwrap()
            .unwrap();
        assert!(!lease.state().udp);
        assert_eq!(fake.auths()[0].field("hysteria-cc-rx"), Some("0"));
    }

    /// The server answers a wrong password as a web site would: the
    /// authentication failed, and the client closes the connection as the
    /// reference client does (H3_GENERAL_PROTOCOL_ERROR). The next request
    /// tries again.
    #[tokio::test]
    async fn a_wrong_password_fails_the_authentication_and_closes_the_connection() {
        let echo = echo_server().await;
        let fake = FakeHysteria2::spawn(script("right"));
        let out = outbound(&fake, "password=wrong");
        for attempt in 0..2 {
            let err = failure(&out, &target(echo)).await;
            assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
            assert_eq!(err.to_string(), "hysteria2: authentication failed");
            let closed = timeout(BOUND, fake.connection(attempt).closed())
                .await
                .expect("closed");
            assert!(
                matches!(&closed, ConnectionError::ApplicationClosed(close)
                    if close.error_code == VarInt::from_u32(0x101)),
                "{closed:?}"
            );
        }
        assert_eq!(fake.auths().len(), 2);
        assert!(fake.tcp_requests().is_empty());
    }

    /// Past both sides' stream windows (8 MiB ours, quinn's 1.25 MB the
    /// server's); the shutdown reaches the target as a FIN, and the
    /// target's close comes back as the end of the stream.
    #[tokio::test]
    async fn a_tunnel_carries_a_large_payload_and_a_half_close() {
        let echo = echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
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

    /// Design 5.3: the server's message, without control characters and at
    /// most 200 characters; a message longer than the reference reads makes
    /// the answer malformed. A refused stream costs its connection nothing.
    #[tokio::test]
    async fn a_refusal_quotes_the_servers_message_cleaned_and_cut_short() {
        let echo = echo_server().await;
        let refused = "hysteria2: the server refused the connection";
        for (message, expected) in [
            (
                format!("no route\r\n\x1b[31m{}", "é".repeat(300)),
                format!("{refused}: no route[31m{}", "é".repeat(188)),
            ),
            (String::new(), refused.to_string()),
            // the longest message the reference reads, and one byte more
            ("x".repeat(2048), format!("{refused}: {}", "x".repeat(200))),
            (
                "x".repeat(2049),
                "hysteria2: the server's answer is malformed".to_string(),
            ),
        ] {
            let fake = FakeHysteria2::spawn(Hysteria2Script {
                refuse: Some(message),
                ..script("pw")
            });
            let out = outbound(&fake, "password=pw");
            for _ in 0..2 {
                let err = failure(&out, &target(echo)).await;
                assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
                assert_eq!(err.to_string(), expected);
            }
            assert_eq!(fake.connections(), 1);
        }
    }

    #[tokio::test]
    async fn names_go_out_as_a_labels_ipv6_in_brackets_and_an_unsendable_name_never_dials() {
        let echo = echo_server().await;
        let fake = FakeHysteria2::spawn(Hysteria2Script {
            connect_to: Some(echo),
            ..script("pw")
        });
        let out = outbound(&fake, "password=pw");
        for (host, address) in [
            (HostName::parse("2001:db8::1"), "[2001:db8::1]:443"),
            (
                HostName::Domain("bücher.example".into()),
                "xn--bcher-kva.example:443",
            ),
            (HostName::parse("remote.example"), "remote.example:443"),
        ] {
            let mut stream = tunnel(&out, &Target::new(host, 443)).await;
            round_trip(&mut stream, b"x").await;
            assert_eq!(fake.tcp_requests().last().unwrap().address, address);
        }
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        // `name:80` one byte longer than the reference reads
        let long = "a".repeat(2046);
        for name in [
            "x@blocked.test",
            "a b.test",
            "a.test\r\nx: 1",
            "",
            long.as_str(),
        ] {
            let err = failure(&out, &Target::new(HostName::Domain(name.into()), 80)).await;
            assert_eq!(
                err.to_string(),
                "hysteria2: the host name cannot be sent to the server"
            );
        }
        assert_eq!(fake.connections(), 0);
    }

    /// One dial and one authentication for the tunnels that come together,
    /// and the connection carries those after them too.
    #[tokio::test]
    async fn tunnels_that_come_together_share_one_connection() {
        let echo = echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let to = target(echo);
        let (a, b, c) = tokio::join!(tunnel(&out, &to), tunnel(&out, &to), tunnel(&out, &to));
        for (mut stream, payload) in [(a, b"one"), (b, b"two"), (c, b"six")] {
            round_trip(&mut stream, payload).await;
        }
        let mut later = tunnel(&out, &to).await;
        round_trip(&mut later, b"later").await;
        assert_eq!(fake.connections(), 1);
        assert_eq!(fake.auths().len(), 1);
        assert_eq!(fake.tcp_requests().len(), 4);
    }

    /// A tunnel under way ends with its connection; the next one dials and
    /// authenticates again.
    #[tokio::test]
    async fn after_the_server_closes_the_connection_the_next_tunnel_dials_again() {
        let echo = echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let mut first = tunnel(&out, &target(echo)).await;
        round_trip(&mut first, b"first").await;
        fake.close_connections();
        let mut rest = Vec::new();
        let err = timeout(BOUND, first.read_to_end(&mut rest))
            .await
            .expect("the end of the tunnel")
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "quic: the server closed the connection (code 0x100)"
        );
        let mut second = tunnel(&out, &target(echo)).await;
        round_trip(&mut second, b"second").await;
        assert_eq!(fake.connections(), 2);
        assert_eq!(fake.auths().len(), 2);
    }

    /// Design 3.5: the server restarts — losing its connections — as a
    /// request comes. The request finds its connection gone (a stateless
    /// reset answers the client's next packet) before any of the caller's
    /// data went out, and goes again, once, on a new connection. A restart
    /// between two requests is no harder.
    #[tokio::test]
    async fn a_request_that_finds_the_server_restarted_goes_again_on_a_new_connection() {
        let echo = echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let mut before = tunnel(&out, &target(echo)).await;
        round_trip(&mut before, b"before").await;
        fake.restart_at_next_request();
        // room for a keep-alive (10 seconds) to bring the reset, should the
        // lost endpoint have acknowledged the request before it fell silent
        let opts = ConnectOpts {
            timeout: Duration::from_secs(25),
        };
        let mut after = timeout(
            Duration::from_secs(30),
            out.connect_tcp(&target(echo), &opts),
        )
        .await
        .expect("an outcome within the bound")
        .expect("a tunnel");
        round_trip(&mut after, b"after").await;
        let carried: Vec<usize> = fake.tcp_requests().iter().map(|r| r.connection).collect();
        assert_eq!(carried, [0, 0, 1]);
        assert_eq!(fake.auths().len(), 2);
        let mut rest = Vec::new();
        let err = timeout(BOUND, before.read_to_end(&mut rest))
            .await
            .expect("the end of the tunnel")
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "quic: the server no longer knows the connection (stateless reset)"
        );

        fake.restart();
        let mut later = timeout(
            Duration::from_secs(30),
            out.connect_tcp(&target(echo), &opts),
        )
        .await
        .expect("an outcome within the bound")
        .expect("a tunnel");
        round_trip(&mut later, b"later").await;
        assert_eq!(fake.connections(), 3);
        assert_eq!(fake.tcp_requests().last().unwrap().connection, 2);
    }

    /// Design 3.3: the port changes under the connection — a new local
    /// socket each time — and its tunnels, old and new, go on. The interval
    /// is set past the configuration's minimum of 5 seconds.
    #[tokio::test]
    async fn tunnels_keep_working_while_the_port_hops() {
        let echo = echo_server().await;
        let ports = MultiPort::bind(3);
        let fake = FakeHysteria2::spawn_on(script("pw"), ports.clone());
        let mut spec = spec(9, "password=pw");
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

    /// `opts.timeout` bounds the whole request: the dial, the handshakes
    /// and the answer.
    #[tokio::test]
    async fn a_server_that_never_answers_times_out() {
        let echo = echo_server().await;
        let opts = ConnectOpts {
            timeout: Duration::from_millis(500),
        };
        // nothing speaks QUIC on this port
        let quiet = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = quiet.local_addr().unwrap().port();
        let fake = FakeHysteria2::spawn(Hysteria2Script {
            silent: true,
            ..script("pw")
        });
        let out = outbound_with(&spec(port, "password=pw"), port, &fake);
        let started = Instant::now();
        let err = out.connect_tcp(&target(echo), &opts).await.err().unwrap();
        assert!(matches!(err, OutboundError::Timeout), "{err}");
        // well before QUIC's own handshake timeout, about 3 seconds
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        // a server that takes the request and never answers it, on a
        // connection that is up already
        let out = outbound(&fake, "password=pw");
        drop(
            timeout(BOUND, out.session.lease(&ConnectOpts::default()))
                .await
                .unwrap()
                .unwrap(),
        );
        let started = Instant::now();
        let err = out.connect_tcp(&target(echo), &opts).await.err().unwrap();
        assert!(matches!(err, OutboundError::Timeout), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(fake.tcp_requests().len(), 1, "the request went out");
    }

    /// The password goes to the server and nowhere else: not into the error
    /// of a failed authentication, nor into a build error.
    #[tokio::test]
    async fn the_password_never_shows() {
        const PASSWORD: &str = "s3cret-hy2";
        let echo = echo_server().await;
        let fake = FakeHysteria2::spawn(script("another"));
        let out = outbound(&fake, &format!("password={PASSWORD}"));
        let err = failure(&out, &target(echo)).await;
        assert_eq!(err.to_string(), "hysteria2: authentication failed");
        assert!(!format!("{err:?}").contains(PASSWORD));
        assert_eq!(fake.auths()[0].field("hysteria-auth"), Some(PASSWORD));
        for (password, message) in [
            ("", "`password` is empty"),
            ("s3cret\nhy2", "`password` cannot be sent in an HTTP field"),
        ] {
            let mut spec = spec(443, "password=x");
            spec.password = password.into();
            let err = Hysteria2Outbound::new(
                "H",
                Target::new(HostName::parse("127.0.0.1"), 443),
                &spec,
                Tristate::Auto,
                &[],
                fake.roots(),
                Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
            )
            .map(|_| ())
            .unwrap_err();
            assert_eq!(err.message, message);
        }
    }
}
```

新建 `crates/rurge-proto-quic/src/hysteria2/auth.rs`：

```rust
//! The authentication of every new connection (Hysteria 2 protocol,
//! "Authentication"; phase 2 M7 design 5.2): an HTTP/3 `POST
//! https://hysteria/auth` carrying the password (`Hysteria-Auth`), the rate
//! the client takes in (`Hysteria-CC-RX`, bytes per second, `0` for
//! unknown) and padding. Status 233 lets the client in and says whether
//! the server relays UDP (`Hysteria-UDP`); any other is the web site the
//! server shows strangers: the authentication failed, and the connection
//! is closed as the reference client closes it. The server's own
//! `Hysteria-CC-RX` changes nothing here (M7-D10).

use super::padding;
use crate::session::{Protocol, close};
use http::header::{HeaderName, HeaderValue};
use http::{HeaderMap, Method, Request, Uri};
use quinn::{Connection, VarInt};
use rurge_net::BoxFuture;
use rurge_net::quic::h3;
use rurge_proto::OutboundError;
use std::ops::RangeInclusive;
use tokio::io::AsyncWriteExt;

/// H3_NO_ERROR (RFC 9114 8.1): a connection let go of.
const H3_NO_ERROR: VarInt = VarInt::from_u32(0x100);
/// H3_GENERAL_PROTOCOL_ERROR (RFC 9114 8.1): what the reference client
/// closes a connection with when its authentication failed
/// (`core/client/client.go`).
const H3_GENERAL_PROTOCOL_ERROR: VarInt = VarInt::from_u32(0x101);

/// The status of an authentication that let the client in.
const AUTH_OK: u16 = 233;

const AUTH: HeaderName = HeaderName::from_static("hysteria-auth");
const CC_RX: HeaderName = HeaderName::from_static("hysteria-cc-rx");
const PADDING: HeaderName = HeaderName::from_static("hysteria-padding");
const UDP: HeaderName = HeaderName::from_static("hysteria-udp");

/// The reference's padding of an authentication request.
const AUTH_PADDING: RangeInclusive<usize> = 256..=2047;

/// What authenticates the policy's connections. No `Debug`: it holds the
/// password.
pub(crate) struct Auth {
    /// `Hysteria-Auth`, marked sensitive: QPACK never indexes it.
    password: HeaderValue,
    /// `Hysteria-CC-RX`.
    rx: u64,
}

/// What an authenticated connection keeps for its requests.
pub(crate) struct Authenticated {
    /// Lives as long as the connection: dropped, it would end our HTTP/3
    /// control stream, which a server may take for the end of HTTP/3 on
    /// the connection.
    h3: h3::Client,
    /// `Hysteria-UDP`: whether the server relays UDP.
    // only tests read it until the UDP carriers do (M7a task 6)
    #[allow(dead_code)]
    pub(crate) udp: bool,
}

impl Auth {
    /// `None` for a password no HTTP field can carry (a control
    /// character). `download_bandwidth` is in Mbps; the server learns it in
    /// bytes per second.
    pub(crate) fn new(password: &str, download_bandwidth: u32) -> Option<Auth> {
        let mut password = HeaderValue::from_str(password).ok()?;
        password.set_sensitive(true);
        Some(Auth {
            password,
            rx: u64::from(download_bandwidth) * 125_000,
        })
    }

    /// The authentication request, its padding drawn anew.
    fn request(&self) -> Request<()> {
        let mut request = Request::new(());
        *request.method_mut() = Method::POST;
        *request.uri_mut() = Uri::from_static("https://hysteria/auth");
        let padding = HeaderValue::from_bytes(&padding(AUTH_PADDING)).expect("letters and digits");
        let headers = request.headers_mut();
        headers.insert(AUTH, self.password.clone());
        headers.insert(CC_RX, HeaderValue::from(self.rx));
        headers.insert(PADDING, padding);
        request
    }

    /// Whether the server relays UDP, once it let the client in.
    async fn authenticate(&self, h3: &h3::Client) -> Result<bool, OutboundError> {
        let mut stream = h3.request(self.request()).await?;
        // no content
        stream.shutdown().await?;
        let response = stream.response().await?;
        if response.status().as_u16() != AUTH_OK {
            return Err(OutboundError::Proxy(
                "hysteria2: authentication failed".to_string(),
            ));
        }
        Ok(relays_udp(response.headers()))
    }
}

/// `Hysteria-UDP` as the reference reads it (Go's `strconv.ParseBool`):
/// anything but one of its forms of true — no field included — is false.
fn relays_udp(headers: &HeaderMap) -> bool {
    headers.get(UDP).is_some_and(|value| {
        matches!(
            value.as_bytes(),
            b"1" | b"t" | b"T" | b"true" | b"TRUE" | b"True"
        )
    })
}

impl Protocol for Auth {
    type State = Authenticated;

    const NAME: &'static str = "hysteria2";

    const NO_ERROR: VarInt = H3_NO_ERROR;

    fn handshake<'a>(
        &'a self,
        connection: &'a Connection,
    ) -> BoxFuture<'a, Result<Authenticated, OutboundError>> {
        Box::pin(async move {
            let h3 = h3::Client::new(connection.clone());
            match self.authenticate(&h3).await {
                Ok(udp) => Ok(Authenticated { h3, udp }),
                Err(e) => {
                    close(connection, H3_GENERAL_PROTOCOL_ERROR);
                    Err(e)
                }
            }
        })
    }

    /// The server sent GOAWAY: it is going down.
    fn is_going_away(&self, state: &Authenticated) -> bool {
        state.h3.is_going_away()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_is_a_post_to_hysteria_auth_that_hides_the_password() {
        let auth = Auth::new("s3cret", 8).expect("a password a field can carry");
        let request = auth.request();
        assert_eq!(request.method(), Method::POST);
        assert_eq!(request.uri(), "https://hysteria/auth");
        let headers = request.headers();
        assert_eq!(headers[AUTH], "s3cret");
        assert!(headers[AUTH].is_sensitive());
        // 8 Mbps in bytes per second
        assert_eq!(headers[CC_RX], "1000000");
        let padding = headers[PADDING].as_bytes();
        assert!(AUTH_PADDING.contains(&padding.len()), "{}", padding.len());
        assert!(padding.iter().all(u8::is_ascii_alphanumeric));
        assert_eq!(headers.len(), 3);
        assert!(!format!("{request:?}").contains("s3cret"));
        // drawn anew for every connection
        let lengths: std::collections::HashSet<usize> = (0..50)
            .map(|_| auth.request().headers()[PADDING].len())
            .collect();
        assert!(lengths.len() > 1, "{lengths:?}");
        // without a bandwidth the rate is unknown
        let unknown = Auth::new("s3cret", 0).unwrap();
        assert_eq!(unknown.request().headers()[CC_RX], "0");
    }

    #[test]
    fn a_password_with_a_control_character_cannot_be_sent() {
        for password in ["a\nb", "a\rb", "a\0b", "a\u{1}b", "a\u{7f}b"] {
            assert!(Auth::new(password, 0).is_none(), "{password:?}");
        }
        for password in ["a b", "tab\there", "ünïcödé", "p@ss:w;rd"] {
            assert!(Auth::new(password, 0).is_some(), "{password:?}");
        }
    }

    #[test]
    fn hysteria_udp_reads_as_the_reference_reads_it() {
        let answer = |value: Option<&'static str>| {
            let mut headers = HeaderMap::new();
            if let Some(value) = value {
                headers.insert(UDP, HeaderValue::from_static(value));
            }
            relays_udp(&headers)
        };
        for value in ["true", "1", "t", "T", "TRUE", "True"] {
            assert!(answer(Some(value)), "{value}");
        }
        for value in ["false", "0", "yes", "tRUE", "", " true"] {
            assert!(!answer(Some(value)), "{value:?}");
        }
        assert!(!answer(None));
    }
}
```

新建 `crates/rurge-proto-quic/src/hysteria2/tcp.rs`：

```rust
//! TCP over Hysteria 2 (Hysteria 2 protocol, "TCP"; phase 2 M7 design
//! 5.3): every connection a bidirectional stream of its own on the policy's
//! QUIC connection.
//!
//! - The request goes out at once: `varint 0x401 ‖ varint length ‖
//!   host:port ‖ varint length ‖ padding`.
//! - Nothing of the caller's goes before the server's answer (the
//!   reference client's default; no fast open): `u8 status ‖ varint length
//!   ‖ message ‖ varint length ‖ padding`. Status 0 lets the stream
//!   through; any other is the server's refusal, its message quoted. A
//!   length past the reference's bounds makes the answer malformed.

use super::auth::Authenticated;
use super::padding;
use crate::session::{Failure, Lease};
use crate::varint;
use quinn::{ReadError, ReadExactError, RecvStream, SendStream, WriteError};
use rurge_net::connector::Target;
use rurge_net::quic::connection_error;
use rurge_proto::OutboundError;
use rurge_proto::http::wire_host;
use rurge_proto::outbound::untrusted_text;
use std::io;
use std::ops::RangeInclusive;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The frame type that starts a TCP request.
const TCP_REQUEST: u64 = 0x401;
/// The reference's bounds (`core/internal/protocol/proxy.go`): the longest
/// address it reads, the longest message and padding of an answer.
const MAX_ADDRESS: usize = 2048;
const MAX_MESSAGE: u64 = 2048;
const MAX_PADDING: u64 = 4096;
/// The reference's padding of a TCP request.
const REQUEST_PADDING: RangeInclusive<usize> = 64..=511;
/// How much of the server's message an error text quotes.
const MESSAGE_MAX: usize = 200;

/// `host:port` as Hysteria 2 names a target: an IPv6 literal in brackets,
/// a name as its A-labels (`http::wire_host`); `None` for a name that
/// cannot be sent, or an address longer than the server reads.
fn address(target: &Target) -> Option<String> {
    let address = format!("{}:{}", wire_host(target)?, target.port);
    (address.len() <= MAX_ADDRESS).then_some(address)
}

/// The TCP request for `target`: refused, before anything is dialed, for a
/// name that cannot be sent.
pub(super) fn request(target: &Target) -> Result<Vec<u8>, OutboundError> {
    let Some(address) = address(target) else {
        return Err(proxy("the host name cannot be sent to the server"));
    };
    let padding = padding(REQUEST_PADDING);
    let mut request = Vec::with_capacity(2 + 2 + address.len() + 2 + padding.len());
    varint::put(&mut request, TCP_REQUEST);
    varint::put(&mut request, address.len() as u64);
    request.extend_from_slice(address.as_bytes());
    varint::put(&mut request, padding.len() as u64);
    request.extend_from_slice(&padding);
    Ok(request)
}

/// The tunnel `request` asks for, on the connection of `lease`: the request
/// on a stream of its own, then the server's answer. A connection found
/// gone before the answer came is `Lost` — nothing of the caller's went
/// out yet; the stream handed back is the caller's.
pub(super) async fn open(lease: Lease<Authenticated>, request: &[u8]) -> Result<Tunnel, Failure> {
    let (mut send, mut recv) = lease
        .connection()
        .open_bi()
        .await
        .map_err(|e| Failure::lost(&e))?;
    send.write_all(request).await.map_err(|e| match e {
        WriteError::ConnectionLost(e) => Failure::lost(&e),
        e => Failure::Failed(write_error(e).into()),
    })?;
    answer(&mut recv).await?;
    Ok(Tunnel {
        send,
        recv,
        _lease: lease,
    })
}

/// The server's answer: the stream goes through, or its refusal.
async fn answer(recv: &mut RecvStream) -> Result<(), Failure> {
    let mut status = [0u8];
    read(recv, &mut status).await?;
    let message = field(recv, MAX_MESSAGE).await?;
    // the padding, dropped
    field(recv, MAX_PADDING).await?;
    if status[0] == 0 {
        return Ok(());
    }
    // the server's text: cleaned and cut short
    let message = untrusted_text(&String::from_utf8_lossy(&message), MESSAGE_MAX);
    Err(Failure::Failed(if message.is_empty() {
        proxy("the server refused the connection")
    } else {
        proxy(&format!("the server refused the connection: {message}"))
    }))
}

/// A field of the answer, its length first: at most `max` bytes.
async fn field(recv: &mut RecvStream, max: u64) -> Result<Vec<u8>, Failure> {
    let mut integer = [0u8; 8];
    read(recv, &mut integer[..1]).await?;
    let n = varint::len(integer[0]);
    read(recv, &mut integer[1..n]).await?;
    let (len, _) = varint::get(&integer[..n]).expect("a whole integer");
    if len > max {
        return Err(Failure::Failed(proxy("the server's answer is malformed")));
    }
    let mut field = vec![0u8; len as usize];
    read(recv, &mut field).await?;
    Ok(field)
}

async fn read(recv: &mut RecvStream, buf: &mut [u8]) -> Result<(), Failure> {
    recv.read_exact(buf).await.map_err(|e| match e {
        ReadExactError::FinishedEarly(_) => {
            Failure::Failed(proxy("the server ended the stream without an answer"))
        }
        ReadExactError::ReadError(ReadError::ConnectionLost(e)) => Failure::lost(&e),
        ReadExactError::ReadError(e) => Failure::Failed(read_error(e).into()),
    })
}

fn proxy(text: &str) -> OutboundError {
    OutboundError::Proxy(format!("hysteria2: {text}"))
}

/// A TCP connection through the server: the request's stream, on a
/// connection its lease keeps. Shutdown sends the FIN the target then
/// sees; reading goes on. Dropped, the stream ends both ways (a FIN, and
/// STOP_SENDING for what is left unread), as the reference client closes
/// it. No `Debug`: nothing in it is worth printing.
pub(super) struct Tunnel {
    send: SendStream,
    recv: RecvStream,
    /// Last: the stream ends before its connection may be closed.
    _lease: Lease<Authenticated>,
}

impl AsyncRead for Tunnel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.get_mut()
            .recv
            .poll_read_buf(cx, buf)
            .map_err(read_error)
    }
}

impl AsyncWrite for Tunnel {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().send)
            .poll_write(cx, buf)
            .map_err(write_error)
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

/// A failed read as an I/O error: the connection's end as QUIC tells it
/// (`quic: …`), the stream's as the protocol's.
fn read_error(e: ReadError) -> io::Error {
    match e {
        ReadError::ConnectionLost(e) => connection_error(&e),
        ReadError::Reset(code) => io::Error::new(
            io::ErrorKind::ConnectionReset,
            format!(
                "hysteria2: the server reset the stream (code {:#x})",
                code.into_inner()
            ),
        ),
        ReadError::ClosedStream | ReadError::IllegalOrderedRead | ReadError::ZeroRttRejected => {
            closed_stream()
        }
    }
}

fn write_error(e: WriteError) -> io::Error {
    match e {
        WriteError::ConnectionLost(e) => connection_error(&e),
        WriteError::Stopped(code) => io::Error::new(
            io::ErrorKind::ConnectionReset,
            format!(
                "hysteria2: the server stopped the stream (code {:#x})",
                code.into_inner()
            ),
        ),
        WriteError::ClosedStream | WriteError::ZeroRttRejected => closed_stream(),
    }
}

fn closed_stream() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "hysteria2: the stream is closed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;

    /// The address and the padding of `request`.
    fn parts(request: &[u8]) -> (&[u8], &[u8]) {
        assert_eq!(&request[..2], [0x44, 0x01], "the type 0x401");
        let (len, n) = varint::get(&request[2..]).expect("the address length");
        let (address, rest) = request[2 + n..].split_at(len as usize);
        let (len, n) = varint::get(rest).expect("the padding length");
        assert_eq!(rest.len(), n + len as usize, "nothing after the padding");
        (address, &rest[n..])
    }

    #[test]
    fn a_request_is_its_type_the_address_and_padding() {
        for (host, address) in [
            ("192.0.2.1", "192.0.2.1:443"),
            ("2001:db8::1", "[2001:db8::1]:443"),
            ("bücher.example", "xn--bcher-kva.example:443"),
        ] {
            let target = Target::new(HostName::parse(host), 443);
            let request = request(&target).unwrap();
            let (sent, padding) = parts(&request);
            assert_eq!(sent, address.as_bytes());
            assert!(
                REQUEST_PADDING.contains(&padding.len()),
                "{}",
                padding.len()
            );
            assert!(padding.iter().all(u8::is_ascii_alphanumeric));
        }
        // a length of 64 or more takes two bytes
        let long = Target::new(HostName::Domain(format!("{}.test", "a".repeat(80))), 80);
        let request = request(&long).unwrap();
        assert_eq!(varint::get(&request[2..]), Some((88, 2)));
        assert_eq!(parts(&request).0.len(), 88);
    }

    #[test]
    fn an_address_must_be_one_the_server_reads() {
        // `name:80` of exactly the longest address the reference reads
        let longest = Target::new(HostName::Domain("a".repeat(2045)), 80);
        assert_eq!(parts(&request(&longest).unwrap()).0.len(), 2048);
        for target in [
            Target::new(HostName::Domain("a".repeat(2046)), 80),
            Target::new(HostName::Domain("x@blocked.test".into()), 80),
            Target::new(HostName::Domain(String::new()), 80),
        ] {
            let err = request(&target).unwrap_err();
            assert_eq!(
                err.to_string(),
                "hysteria2: the host name cannot be sent to the server"
            );
        }
    }
}
```

删除 `crates/rurge-proto-quic/src/testing.rs`（它的内容移到了新目录下的文件里）。

新建 `crates/rurge-proto-quic/src/testing/mod.rs`：

```rust
//! Loopback QUIC servers for the tests of this crate and of its dependants
//! (feature `testing`): a server port whose endpoint can be replaced, as
//! by a server that restarts, and a scriptable Hysteria 2 server. Never
//! used by production code.

mod hysteria2;

pub use hysteria2::{FakeHysteria2, Hysteria2Script, RecordedAuth, RecordedTcp};

use quinn::udp::{RecvMeta, Transmit};
use quinn::{
    AsyncUdpSocket, ConnectionId, ConnectionIdGenerator, Endpoint, EndpointConfig, ServerConfig,
    TokioRuntime, UdpPoller,
};
use rurge_net::quic::testing::loopback_socket;
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

/// A loopback UDP port for QUIC servers that restart (phase 2 M7 design
/// 3.5: the stale connection). Its endpoints share the key of their
/// stateless resets and the scheme of their connection IDs, as a server
/// restarted with the same configuration: a new endpoint knows none of the
/// connections of the one before and answers their packets with a
/// stateless reset (RFC 9000 10.3), which ends them at the client. The
/// endpoint before hears and sends nothing any more.
pub struct ServerPort {
    socket: Arc<dyn AsyncUdpSocket>,
    config: EndpointConfig,
    /// The way to the port of the endpoint that answers now.
    current: Mutex<Option<Arc<Gate>>>,
}

impl ServerPort {
    /// On `127.0.0.1`, any port.
    pub fn bind() -> ServerPort {
        let mut config = EndpointConfig::default();
        let ids = Arc::new(AtomicU64::new(1));
        config.cid_generator(move || Box::new(SharedIds(ids.clone())));
        ServerPort {
            socket: loopback_socket(),
            config,
            current: Mutex::new(None),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.socket.local_addr().expect("the port's address")
    }

    /// A server endpoint presenting `config` that answers on the port from
    /// now on; the one before it falls silent.
    pub fn endpoint(&self, config: ServerConfig) -> Endpoint {
        let gate = Arc::new(Gate {
            socket: self.socket.clone(),
            open: AtomicBool::new(true),
        });
        let before = self.current.lock().expect("the gate").replace(gate.clone());
        if let Some(before) = before {
            before.open.store(false, Ordering::SeqCst);
        }
        Endpoint::new_with_abstract_socket(
            self.config.clone(),
            Some(config),
            gate,
            Arc::new(TokioRuntime),
        )
        .expect("a server endpoint")
    }
}

/// Connection IDs from a counter that all of a port's endpoints share:
/// never the same twice, and each taken for one of theirs (the trait's own
/// check), so that a packet for a connection an endpoint never had gets a
/// stateless reset instead of being dropped as not its own.
struct SharedIds(Arc<AtomicU64>);

impl ConnectionIdGenerator for SharedIds {
    fn generate_cid(&mut self) -> ConnectionId {
        ConnectionId::new(&self.0.fetch_add(1, Ordering::Relaxed).to_be_bytes())
    }

    fn cid_len(&self) -> usize {
        8
    }

    fn cid_lifetime(&self) -> Option<Duration> {
        None
    }
}

/// One endpoint's way to the port: open, everything passes; closed — for
/// good —, the endpoint hears nothing and what it sends is dropped.
#[derive(Debug)]
struct Gate {
    socket: Arc<dyn AsyncUdpSocket>,
    open: AtomicBool,
}

impl AsyncUdpSocket for Gate {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        self.socket.clone().create_io_poller()
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        if self.open.load(Ordering::SeqCst) {
            self.socket.try_send(transmit)
        } else {
            Ok(())
        }
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        if !self.open.load(Ordering::SeqCst) {
            // never opened again: nothing to wake the endpoint for
            return Poll::Pending;
        }
        self.socket.poll_recv(cx, bufs, meta)
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    fn max_transmit_segments(&self) -> usize {
        self.socket.max_transmit_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.socket.max_receive_segments()
    }

    fn may_fragment(&self) -> bool {
        self.socket.may_fragment()
    }
}
```

新建 `crates/rurge-proto-quic/src/testing/hysteria2.rs`：

```rust
//! A scriptable Hysteria 2 server (phase 2 M7 design 5.5): QUIC with ALPN
//! `h3` on a loopback port of its own (`ServerPort`: it can restart) or on
//! a socket it is given (`MultiPort` for port hopping), HTTP/3 for the
//! authentication, then TCP requests relayed to their targets. What the
//! client sends is read by code that is not the client's: the
//! authentication's fields by the HTTP/3 server side of `rurge_net`, the
//! TCP requests by the code here, which also writes the answers — every
//! length in them in two bytes, as QUIC allows. It never resolves a name.

use super::ServerPort;
use quinn::{AsyncUdpSocket, Connection, Endpoint, VarInt};
use rurge_net::quic::h3::testing::{H3Server, ServerStream};
use rurge_net::quic::testing::{TestCert, endpoint};
use rustls::RootCertStore;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::task::JoinSet;
use tokio_util::task::AbortOnDropHandle;

// the wire as the fake knows it
const HEADERS: u64 = 0x01;
const TCP_REQUEST: u64 = 0x401;
/// H3_NO_ERROR: how the fake closes its connections.
const H3_NO_ERROR: VarInt = VarInt::from_u32(0x100);
/// H3_REQUEST_REJECTED: a stream the fake does not serve.
const H3_REQUEST_REJECTED: u64 = 0x10b;

#[derive(Clone, Debug, Default)]
pub struct Hysteria2Script {
    /// The password it lets in; any other authentication is answered as a
    /// web site would answer it (404).
    pub password: String,
    /// Answer `Hysteria-UDP: false`.
    pub no_udp: bool,
    /// Refuse every TCP request with this message.
    pub refuse: Option<String>,
    /// Take TCP requests and never answer them.
    pub silent: bool,
    /// Relay here whatever the client asked for (needed for a name).
    pub connect_to: Option<SocketAddr>,
}

/// An authentication request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedAuth {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// Every field, in order: the pseudo-header fields first.
    pub fields: Vec<(String, String)>,
}

impl RecordedAuth {
    /// The first field called `name`.
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// A TCP request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedTcp {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// `host:port`, as the request carried it.
    pub address: String,
    /// How long its padding was.
    pub padding: usize,
}

pub struct FakeHysteria2 {
    inner: Arc<Inner>,
}

struct Inner {
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
    script: Hysteria2Script,
    /// The server's side of every connection accepted, in order.
    connections: Mutex<Vec<Connection>>,
    auths: Mutex<Vec<RecordedAuth>>,
    requests: Mutex<Vec<RecordedTcp>>,
    /// Restart once the next TCP request came.
    restart_at_request: AtomicBool,
    /// The fake, to restart it.
    fake: Weak<Inner>,
}

impl Shared {
    fn connections(&self) -> MutexGuard<'_, Vec<Connection>> {
        self.connections.lock().expect("connections")
    }
}

impl FakeHysteria2 {
    /// On a loopback port of its own, which it can take over again
    /// (`restart`).
    pub fn spawn(script: Hysteria2Script) -> FakeHysteria2 {
        let port = ServerPort::bind();
        let fake = FakeHysteria2::new(script, port.addr(), Some(port));
        fake.inner.take_over();
        fake
    }

    /// On `socket` — a `MultiPort` for port hopping —, which it cannot
    /// take over again.
    pub fn spawn_on(script: Hysteria2Script, socket: Arc<dyn AsyncUdpSocket>) -> FakeHysteria2 {
        let addr = socket.local_addr().expect("the socket's address");
        let fake = FakeHysteria2::new(script, addr, None);
        let config = fake.inner.cert.server_config(&["h3"]);
        fake.inner.listen(endpoint(config, socket));
        fake
    }

    fn new(script: Hysteria2Script, addr: SocketAddr, port: Option<ServerPort>) -> FakeHysteria2 {
        let inner = Arc::new_cyclic(|fake| Inner {
            cert: TestCert::new(),
            addr,
            port,
            shared: Arc::new(Shared {
                script,
                connections: Mutex::default(),
                auths: Mutex::default(),
                requests: Mutex::default(),
                restart_at_request: AtomicBool::new(false),
                fake: fake.clone(),
            }),
            endpoints: Mutex::default(),
        });
        FakeHysteria2 { inner }
    }

    /// Its port: the first of them, on a socket of several.
    pub fn addr(&self) -> SocketAddr {
        self.inner.addr
    }

    /// Roots that trust its certificate (for `localhost` and `127.0.0.1`).
    pub fn roots(&self) -> Arc<RootCertStore> {
        self.inner.cert.roots()
    }

    /// Every authentication request, in arrival order.
    pub fn auths(&self) -> Vec<RecordedAuth> {
        self.inner.shared.auths.lock().expect("auths").clone()
    }

    /// Every TCP request it read, in arrival order.
    pub fn tcp_requests(&self) -> Vec<RecordedTcp> {
        self.inner.shared.requests.lock().expect("requests").clone()
    }

    /// The connections accepted so far: QUIC's handshake done.
    pub fn connections(&self) -> usize {
        self.inner.shared.connections().len()
    }

    /// The server's side of connection `n`, 0 for the first accepted.
    pub fn connection(&self, n: usize) -> Connection {
        self.inner.shared.connections()[n].clone()
    }

    /// Closes every connection accepted so far (H3_NO_ERROR), as a server
    /// that goes down.
    pub fn close_connections(&self) {
        for connection in self.inner.shared.connections().iter() {
            connection.close(H3_NO_ERROR, b"");
        }
    }

    /// A new endpoint takes the port over, as after a restart: the
    /// connections of the one before are gone, and the client learns it
    /// from a stateless reset of its next packet.
    pub fn restart(&self) {
        self.inner.take_over();
    }

    /// The next TCP request finds the server gone: it restarts as `restart`
    /// does once the request came, before answering it.
    pub fn restart_at_next_request(&self) {
        self.inner
            .shared
            .restart_at_request
            .store(true, Ordering::SeqCst);
    }
}

impl Inner {
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

impl Drop for Inner {
    /// Every connection ends with the fake, and with them the tasks that
    /// serve them.
    fn drop(&mut self) {
        for (endpoint, _) in self.endpoints.get_mut().expect("endpoints").iter() {
            endpoint.close(H3_NO_ERROR, b"");
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

/// One connection: HTTP/3, and its streams, each in a task of its own that
/// ends with the connection.
async fn serve(connection: Connection, shared: Arc<Shared>) {
    let number = {
        let mut connections = shared.connections();
        connections.push(connection.clone());
        connections.len() - 1
    };
    let Ok(h3) = H3Server::start(connection, &[]).await else {
        return;
    };
    let authenticated = Arc::new(AtomicBool::new(false));
    let mut streams = JoinSet::new();
    loop {
        tokio::select! {
            stream = h3.accept() => match stream {
                Some(stream) => {
                    streams.spawn(serve_stream(
                        stream,
                        number,
                        authenticated.clone(),
                        shared.clone(),
                    ));
                }
                None => return,
            },
            // a stream that ended; nothing to collect
            Some(_) = streams.join_next() => {}
        }
    }
}

/// A stream by its first integer: an HTTP/3 request (the authentication)
/// or, once the connection is authenticated, a TCP request.
async fn serve_stream(
    mut stream: ServerStream,
    connection: usize,
    authenticated: Arc<AtomicBool>,
    shared: Arc<Shared>,
) {
    let _ = match stream.peek_varint().await {
        Ok(HEADERS) => authenticate(stream, connection, &authenticated, &shared).await,
        Ok(TCP_REQUEST) if authenticated.load(Ordering::SeqCst) => {
            relay(stream, connection, &shared).await
        }
        // before the authentication, or a stream of another kind
        Ok(_) => {
            stream.reset(H3_REQUEST_REJECTED);
            Ok(())
        }
        Err(_) => Ok(()),
    };
}

/// The authentication: 233 for the password, a web site's 404 for anything
/// else.
async fn authenticate(
    mut stream: ServerStream,
    connection: usize,
    authenticated: &AtomicBool,
    shared: &Shared,
) -> io::Result<()> {
    let fields = stream.read_headers().await?;
    stream.read_body().await?;
    let auth = RecordedAuth { connection, fields };
    let let_in = auth.field(":method") == Some("POST")
        && auth.field(":authority") == Some("hysteria")
        && auth.field(":path") == Some("/auth")
        && auth.field("hysteria-auth") == Some(shared.script.password.as_str());
    shared.auths.lock().expect("auths").push(auth);
    if let_in {
        authenticated.store(true, Ordering::SeqCst);
        let udp = if shared.script.no_udp {
            "false"
        } else {
            "true"
        };
        let fields = [
            ("hysteria-udp", udp),
            ("hysteria-cc-rx", "0"),
            ("hysteria-padding", "server-padding"),
        ];
        stream.send_headers(233, &fields).await?;
    } else {
        // what Go's `http.NotFound` answers
        stream
            .send_headers(404, &[("content-type", "text/plain; charset=utf-8")])
            .await?;
        stream.send_data(b"404 page not found\n").await?;
    }
    stream.finish();
    Ok(())
}

/// A TCP request: read and recorded, then the server restarts, leaves it
/// unanswered, refuses it, or answers it and relays it to its target, the
/// half-closes too. A request outside the reference's bounds ends the
/// stream without an answer, a target it cannot reach is refused with the
/// error's text, as the reference's server does
/// (`core/server/server.go`).
async fn relay(mut stream: ServerStream, connection: usize, shared: &Shared) -> io::Result<()> {
    // the type, peeked already
    read_varint(&mut stream).await?;
    let len = read_varint(&mut stream).await?;
    if !(1..=2048).contains(&len) {
        return Err(invalid("an address length the reference refuses"));
    }
    let mut address = vec![0u8; len as usize];
    stream.read_exact(&mut address).await?;
    let padding = read_varint(&mut stream).await?;
    if padding > 4096 {
        return Err(invalid("a padding length the reference refuses"));
    }
    stream.read_exact(&mut vec![0u8; padding as usize]).await?;
    let address = String::from_utf8_lossy(&address).into_owned();
    shared.requests.lock().expect("requests").push(RecordedTcp {
        connection,
        address: address.clone(),
        padding: padding as usize,
    });
    if shared.restart_at_request.swap(false, Ordering::SeqCst) {
        // gone before it answered: a new endpoint has the port
        if let Some(fake) = shared.fake.upgrade() {
            fake.take_over();
        }
        return Ok(());
    }
    if shared.script.silent {
        // what the client sends is dropped until it gives up
        tokio::io::copy(&mut stream, &mut tokio::io::sink()).await?;
        return Ok(());
    }
    if let Some(message) = &shared.script.refuse {
        stream.write_all(&answer(1, message.as_bytes())).await?;
        return stream.shutdown().await;
    }
    // never resolves: a name without `connect_to` is a dead end
    let upstream = match shared.script.connect_to.or_else(|| address.parse().ok()) {
        Some(to) => TcpStream::connect(to).await,
        None => Err(io::Error::other(format!("no address for {address}"))),
    };
    let mut upstream = match upstream {
        Ok(upstream) => upstream,
        Err(e) => {
            stream
                .write_all(&answer(1, e.to_string().as_bytes()))
                .await?;
            return stream.shutdown().await;
        }
    };
    // with the reference's message, which the client passes over
    stream.write_all(&answer(0, b"Connected")).await?;
    tokio::io::copy_bidirectional(&mut stream, &mut upstream).await?;
    Ok(())
}

/// A QUIC integer, a byte at a time.
async fn read_varint(stream: &mut ServerStream) -> io::Result<u64> {
    let first = stream.read_u8().await?;
    let mut value = u64::from(first & 0x3f);
    for _ in 1..1 << (first >> 6) {
        value = value << 8 | u64::from(stream.read_u8().await?);
    }
    Ok(value)
}

/// `status ‖ length ‖ message ‖ length ‖ padding`, each length in two
/// bytes: the client must read every length QUIC allows, not only the
/// shortest.
fn answer(status: u8, message: &[u8]) -> Vec<u8> {
    let mut out = vec![status];
    for field in [message, &[b'.'; 128][..]] {
        let len = u16::try_from(field.len())
            .ok()
            .filter(|&len| len < 1 << 14)
            .expect("a length of two bytes");
        out.extend_from_slice(&(0x4000 | len).to_be_bytes());
        out.extend_from_slice(field);
    }
    out
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("hysteria2 fake: {what}"),
    )
}
```

要点：
- 认证请求：`Hysteria-Auth`（口令，never-indexed）、`Hysteria-CC-RX`（`download-bandwidth × 125000`，没写为 0）、`Hysteria-Padding`（256–2047 个字母与数字）；非 233 即 `hysteria2: authentication failed`，并以 0x101 关闭连接。
- TCP 请求：`varint 0x401 ‖ varint 地址长度 ‖ "host:port" ‖ varint 填充长度 ‖ 64–511 个字母与数字`；应答 `u8 状态 ‖ varint 消息长度 ‖ 消息 ‖ varint 填充长度 ‖ 填充`。整个请求受 `opts.timeout` 约束。
- 假服务端的应答用非最短的变长整数，成功时消息为 `Connected`（同参考实现）。

- [ ] **Step 3: 运行**

Run: `cargo test -p rurge-proto-quic`
Expected: 通过（新增 19 条：认证请求的各字段与填充范围、口令错的文本与关闭、TCP 回显（含大数据与半关闭）、拒绝消息的清洗与截断、IPv6 与 IDN 的地址、并发隧道共用一条连接、服务端关闭后重连、服务端重启后的失效连接重试、端口跳跃中隧道不断、不应答的服务端受时限约束、口令不进错误文本与 `Debug`、变长整数）。

- [ ] **Step 4: 门禁与提交**

跑门禁。

```bash
git add -A crates/rurge-proto crates/rurge-proto-quic
git commit -m "feat(proto-quic): Hysteria 2 的认证与 TCP——HTTP/3 POST /auth（233）、0x401 请求帧；FakeHysteria2"
```

### Task 6: Hysteria 2 的 UDP

`udp()` 翻转为原生支持，`open_udp` 给出全锥的 `PacketSocket`（P18）：QUIC 数据报 `u32 会话号 ‖ u16 包号 ‖ u8 片号 ‖ u8 片数 ‖ varint 地址长度 ‖ "host:port" ‖ 负载`，放不下才分片；每条连接一个读任务按会话号分发；载体跨连接存活。`FakeHysteria2` 补上 UDP 一侧（独立的编解码、每个会话一个服务端 UDP socket、自己把大的回包分片、可丢掉指定的片）。

**Files:**
- Create: `crates/rurge-proto-quic/src/hysteria2/udp.rs`（自带用例）
- Modify: `crates/rurge-proto-quic/{Cargo.toml, src/hysteria2/mod.rs（与用例）, src/hysteria2/auth.rs, src/hysteria2/tcp.rs, src/testing/mod.rs, src/testing/hysteria2.rs}`

**Interfaces:**
- Consumes: Task 4 的 `Session::lease` / `serves`；Task 5 的认证状态（`Authenticated`）与地址编码；既有的 `UdpSupport`、`PacketSocket` / `BoxedPacketSocket`。
- Produces: `Hysteria2Outbound::udp()` → `UdpSupport::Native`、`open_udp`；`hysteria2::udp`（载体、分片与重组、`Datagrams` 分发）；`FakeHysteria2` 的 UDP 一侧（`udp_messages()`、`udp_outside()`）与 `goaway(n)`；`Hysteria2Script` 的 `no_udp`、`no_datagrams`、`drop_fragment` 脚本项

- [ ] **Step 1: 先写用例（连同假服务端的 UDP 一侧与新模块）**

假服务端：

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
//! authentication, then TCP requests relayed to their targets. What the
//! client sends is read by code that is not the client's: the
//! authentication's fields by the HTTP/3 server side of `rurge_net`, the
//! TCP requests by the code here, which also writes the answers — every
//! length in them in two bytes, as QUIC allows. It never resolves a name.

use super::ServerPort;
use quinn::{AsyncUdpSocket, Connection, Endpoint, VarInt};
```

换成

```rust
//! authentication, then TCP requests relayed to their targets and UDP
//! sessions, each with a loopback socket of its own (full cone). What the
//! client sends is read by code that is not the client's: the
//! authentication's fields by the HTTP/3 server side of `rurge_net`, the
//! TCP requests and the UDP messages by the code here, which also writes
//! the answers — every length in them in two bytes, as QUIC allows — and
//! splits a UDP answer too long for one datagram into fragments of its
//! own. It never resolves a name.

use super::ServerPort;
use quinn::{
    AsyncUdpSocket, Connection, Endpoint, SendDatagramError, ServerConfig, TransportConfig, VarInt,
};
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
use rustls::RootCertStore;
```

换成

```rust
use rustls::RootCertStore;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
use tokio::net::TcpStream;
```

换成

```rust
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::Notify;
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
    /// Answer `Hysteria-UDP: false`.
    pub no_udp: bool,
```

换成

```rust
    /// Answer `Hysteria-UDP: false`, and take no UDP.
    pub no_udp: bool,
    /// Take no QUIC datagrams at all: the transport parameter that allows
    /// them is not sent.
    pub no_datagrams: bool,
    /// The first UDP answer it splits into fragments goes without this
    /// one.
    pub drop_fragment: Option<u8>,
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
    /// Relay here whatever the client asked for (needed for a name).
```

换成

```rust
    /// Relay here whatever the client asked for, TCP and UDP alike (needed
    /// for a name).
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
    pub padding: usize,
}

pub struct FakeHysteria2 {
```

换成

```rust
    pub padding: usize,
}

/// A UDP message from the client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedUdp {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    pub session: u32,
    pub packet: u16,
    pub fragment: u8,
    pub fragments: u8,
    /// `host:port`, as the message carried it.
    pub address: String,
    /// How long its payload was: a fragment's own part, for a fragment.
    pub len: usize,
}

/// A UDP message as the fake reads and writes it, for tests that send
/// their own: `u32 session ‖ u16 packet ‖ u8 fragment ‖ u8 fragments ‖
/// varint length ‖ host:port ‖ payload`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UdpMessage {
    pub session: u32,
    pub packet: u16,
    pub fragment: u8,
    pub fragments: u8,
    /// `host:port`.
    pub address: String,
    pub payload: Vec<u8>,
}

impl UdpMessage {
    /// A datagram that goes whole: packet 0, fragment 0 of 1.
    pub fn whole(session: u32, address: &str, payload: &[u8]) -> UdpMessage {
        UdpMessage {
            session,
            packet: 0,
            fragment: 0,
            fragments: 1,
            address: address.to_string(),
            payload: payload.to_vec(),
        }
    }

    /// The address's length in two bytes, whatever it is: the client must
    /// read every length QUIC allows, not only the shortest.
    pub fn encode(&self) -> Vec<u8> {
        let len = u16::try_from(self.address.len())
            .ok()
            .filter(|&len| len < 1 << 14)
            .expect("an address length of two bytes");
        let mut out = Vec::with_capacity(10 + self.address.len() + self.payload.len());
        out.extend_from_slice(&self.session.to_be_bytes());
        out.extend_from_slice(&self.packet.to_be_bytes());
        out.extend_from_slice(&[self.fragment, self.fragments]);
        out.extend_from_slice(&(0x4000 | len).to_be_bytes());
        out.extend_from_slice(self.address.as_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    /// `None` for what the reference server would not read
    /// (`ParseUDPMessage`): an address of none or more than 2048 bytes, a
    /// message without a payload.
    fn decode(datagram: &[u8]) -> Option<UdpMessage> {
        let (fixed, rest) = datagram.split_at_checked(8)?;
        let first = *rest.first()?;
        let (integer, rest) = rest.split_at_checked(1 << (first >> 6))?;
        let len = integer[1..]
            .iter()
            .fold(u64::from(first & 0x3f), |len, &b| len << 8 | u64::from(b));
        let len = usize::try_from(len)
            .ok()
            .filter(|len| (1..=2048).contains(len) && *len < rest.len())?;
        let (address, payload) = rest.split_at(len);
        Some(UdpMessage {
            session: u32::from_be_bytes([fixed[0], fixed[1], fixed[2], fixed[3]]),
            packet: u16::from_be_bytes([fixed[4], fixed[5]]),
            fragment: fixed[6],
            fragments: fixed[7],
            address: String::from_utf8_lossy(address).into_owned(),
            payload: payload.to_vec(),
        })
    }
}

pub struct FakeHysteria2 {
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
    auths: Mutex<Vec<RecordedAuth>>,
    requests: Mutex<Vec<RecordedTcp>>,
```

换成

```rust
    /// What tells each of them to send GOAWAY, in the same order.
    goaways: Mutex<Vec<Arc<Notify>>>,
    auths: Mutex<Vec<RecordedAuth>>,
    requests: Mutex<Vec<RecordedTcp>>,
    udp: Mutex<Vec<RecordedUdp>>,
    /// Each UDP session's own socket, in the order they opened.
    outside: Mutex<Vec<SocketAddr>>,
    /// The fragment the next UDP answer in fragments goes without.
    drop_fragment: Mutex<Option<u8>>,
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
        self.connections.lock().expect("connections")
```

换成

```rust
        self.connections.lock().expect("connections")
    }

    /// `message` in datagrams of at most `max` bytes: whole when it fits,
    /// else in fragments under a random packet id. The first datagram split
    /// goes without the fragment the script drops.
    fn split(&self, message: UdpMessage, max: usize) -> Vec<Vec<u8>> {
        let whole = message.encode();
        if whole.len() <= max {
            return vec![whole];
        }
        let room = max - (whole.len() - message.payload.len());
        let fragments = u8::try_from(message.payload.len().div_ceil(room)).expect("255 fragments");
        let packet = getrandom::u32().map_or(1, |random| (random % 0xffff) as u16 + 1);
        let dropped = self.drop_fragment.lock().expect("drop").take();
        message
            .payload
            .chunks(room)
            .zip(0..fragments)
            .filter(|&(_, fragment)| Some(fragment) != dropped)
            .map(|(chunk, fragment)| {
                UdpMessage {
                    session: message.session,
                    packet,
                    fragment,
                    fragments,
                    address: message.address.clone(),
                    payload: chunk.to_vec(),
                }
                .encode()
            })
            .collect()
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
        let config = fake.inner.cert.server_config(&["h3"]);
```

换成

```rust
        let config = fake.inner.server_config();
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
                script,
                connections: Mutex::default(),
                auths: Mutex::default(),
                requests: Mutex::default(),
```

换成

```rust
                drop_fragment: Mutex::new(script.drop_fragment),
                script,
                connections: Mutex::default(),
                goaways: Mutex::default(),
                auths: Mutex::default(),
                requests: Mutex::default(),
                udp: Mutex::default(),
                outside: Mutex::default(),
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
        self.inner.shared.requests.lock().expect("requests").clone()
    }

    /// The connections accepted so far: QUIC's handshake done.
```

换成

```rust
        self.inner.shared.requests.lock().expect("requests").clone()
    }

    /// Every UDP message it read, in arrival order: each fragment apart.
    pub fn udp_messages(&self) -> Vec<RecordedUdp> {
        self.inner.shared.udp.lock().expect("udp").clone()
    }

    /// Each UDP session's own socket, in the order they opened: whoever
    /// sends to one is heard by the client.
    pub fn udp_outside(&self) -> Vec<SocketAddr> {
        self.inner.shared.outside.lock().expect("outside").clone()
    }

    /// Sends `message`, as it is, to the client on connection `n`.
    pub fn send_udp(&self, n: usize, message: &UdpMessage) {
        self.connection(n)
            .send_datagram(message.encode().into())
            .expect("a datagram the client takes");
    }

    /// The connections accepted so far: QUIC's handshake done.
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
        self.inner.shared.connections()[n].clone()
```

换成

```rust
        self.inner.shared.connections()[n].clone()
    }

    /// Sends GOAWAY on connection `n`, as a server about to go down: the
    /// client takes nothing new there, and what is under way goes on.
    pub fn goaway(&self, n: usize) {
        self.inner.shared.goaways.lock().expect("goaways")[n].notify_one();
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
impl Inner {
```

换成

```rust
impl Inner {
    /// QUIC with ALPN `h3`, datagrams included unless the script says
    /// none.
    fn server_config(&self) -> ServerConfig {
        let mut config = self.cert.server_config(&["h3"]);
        if self.shared.script.no_datagrams {
            let mut transport = TransportConfig::default();
            transport.datagram_receive_buffer_size(None);
            config.transport_config(Arc::new(transport));
        }
        config
    }

```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
        self.listen(port.endpoint(self.cert.server_config(&["h3"])));
```

换成

```rust
        self.listen(port.endpoint(self.server_config()));
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
/// One connection: HTTP/3, and its streams, each in a task of its own that
/// ends with the connection.
async fn serve(connection: Connection, shared: Arc<Shared>) {
```

换成

```rust
/// One connection: HTTP/3, its streams and its datagrams, each in a task
/// of its own that ends with the connection.
async fn serve(connection: Connection, shared: Arc<Shared>) {
    let goaway = Arc::new(Notify::new());
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
        connections.len() - 1
    };
    let Ok(h3) = H3Server::start(connection, &[]).await else {
```

换成

```rust
        shared.goaways.lock().expect("goaways").push(goaway.clone());
        connections.len() - 1
    };
    let Ok(mut h3) = H3Server::start(connection.clone(), &[]).await else {
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
    let mut streams = JoinSet::new();
```

换成

```rust
    let mut streams = JoinSet::new();
    if !shared.script.no_udp {
        streams.spawn(relay_udp(
            connection,
            number,
            authenticated.clone(),
            shared.clone(),
        ));
    }
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
            Some(_) = streams.join_next() => {}
```

换成

```rust
            Some(_) = streams.join_next() => {}
            () = goaway.notified() => {
                // the authentication, on stream 0, is the one HTTP/3
                // request a connection carries
                let _ = h3.goaway(4).await;
            }
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
    Ok(())
}

/// A QUIC integer, a byte at a time.
```

换成

```rust
    Ok(())
}

/// One of the client's UDP sessions on a connection: a loopback socket of
/// its own, and the datagram it is putting together.
struct UdpSession {
    socket: Arc<UdpSocket>,
    /// The packet id and the fragments so far.
    partial: Option<(u16, Vec<Option<Vec<u8>>>)>,
    _answers: AbortOnDropHandle<()>,
}

impl UdpSession {
    /// Its socket, and the task that sends back whatever reaches it.
    fn open(connection: &Connection, session: u32, shared: &Arc<Shared>) -> io::Result<UdpSession> {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0")?;
        socket.set_nonblocking(true)?;
        let socket = Arc::new(UdpSocket::from_std(socket)?);
        shared
            .outside
            .lock()
            .expect("outside")
            .push(socket.local_addr()?);
        let answers = tokio::spawn(relay_answers(
            socket.clone(),
            connection.clone(),
            session,
            shared.clone(),
        ));
        Ok(UdpSession {
            socket,
            partial: None,
            _answers: AbortOnDropHandle::new(answers),
        })
    }

    /// The datagram `message` makes whole: itself when it is no fragment.
    fn put_together(&mut self, message: UdpMessage) -> Option<Vec<u8>> {
        if message.fragments <= 1 {
            return Some(message.payload);
        }
        let (index, count) = (
            usize::from(message.fragment),
            usize::from(message.fragments),
        );
        if index >= count {
            return None;
        }
        if !matches!(&self.partial, Some((packet, parts))
            if *packet == message.packet && parts.len() == count)
        {
            self.partial = Some((message.packet, vec![None; count]));
        }
        let (_, parts) = self.partial.as_mut()?;
        parts[index].get_or_insert(message.payload);
        if parts.iter().any(Option::is_none) {
            return None;
        }
        let (_, parts) = self.partial.take()?;
        Some(parts.into_iter().flatten().flatten().collect())
    }
}

/// The client's datagrams on `connection`, once it is authenticated (as
/// the reference serves UDP only then): each session gets a socket of its
/// own with its first message, which every datagram of the session leaves
/// by, put together from its fragments first.
async fn relay_udp(
    connection: Connection,
    number: usize,
    authenticated: Arc<AtomicBool>,
    shared: Arc<Shared>,
) {
    let mut sessions = HashMap::new();
    while let Ok(datagram) = connection.read_datagram().await {
        if !authenticated.load(Ordering::SeqCst) {
            continue;
        }
        let Some(message) = UdpMessage::decode(&datagram) else {
            continue;
        };
        shared.udp.lock().expect("udp").push(RecordedUdp {
            connection: number,
            session: message.session,
            packet: message.packet,
            fragment: message.fragment,
            fragments: message.fragments,
            address: message.address.clone(),
            len: message.payload.len(),
        });
        let session = match sessions.entry(message.session) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => match UdpSession::open(&connection, message.session, &shared) {
                Ok(session) => entry.insert(session),
                Err(_) => continue,
            },
        };
        // never resolves: a name without `connect_to` is a dead end
        let to = shared
            .script
            .connect_to
            .or_else(|| message.address.parse().ok());
        if let (Some(to), Some(payload)) = (to, session.put_together(message)) {
            let _ = session.socket.send_to(&payload, to).await;
        }
    }
}

/// Whatever reaches a session's socket goes back to the client under its
/// source — full cone —, in fragments when one datagram cannot hold it.
async fn relay_answers(
    socket: Arc<UdpSocket>,
    connection: Connection,
    session: u32,
    shared: Arc<Shared>,
) {
    let mut buf = vec![0u8; 65536];
    loop {
        let (n, from) = match socket.recv_from(&mut buf).await {
            Ok(got) => got,
            // an ICMP "unreachable" for an earlier datagram (Windows)
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
            Err(_) => return,
        };
        let Some(max) = connection.max_datagram_size() else {
            return;
        };
        let message = UdpMessage::whole(session, &from.to_string(), &buf[..n]);
        for datagram in shared.split(message, max) {
            if let Err(SendDatagramError::ConnectionLost(_)) =
                connection.send_datagram(datagram.into())
            {
                return;
            }
        }
    }
}

/// A QUIC integer, a byte at a time.
```

`crates/rurge-proto-quic/src/testing/mod.rs`——把

```rust
pub use hysteria2::{FakeHysteria2, Hysteria2Script, RecordedAuth, RecordedTcp};
```

换成

```rust
pub use hysteria2::{
    FakeHysteria2, Hysteria2Script, RecordedAuth, RecordedTcp, RecordedUdp, UdpMessage,
};
```

出站的 UDP 用例：

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

```rust
    use crate::testing::{FakeHysteria2, Hysteria2Script};
```

换成

```rust
    use crate::testing::{FakeHysteria2, Hysteria2Script, RecordedUdp, UdpMessage};
```

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

```rust
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_net::quic::testing::MultiPort;
    use rurge_proto::testing::echo_server;
    use std::collections::HashSet;
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
```

换成

```rust
    use rurge_net::connector::{
        BoxedDatagram, DirectConnector, PacketSocket, Route, SystemResolve,
    };
    use rurge_net::quic::testing::MultiPort;
    use rurge_proto::testing::{echo_server, udp_echo_server};
    use std::collections::{BTreeSet, HashSet};
    use std::io;
    use std::net::SocketAddr;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::task::JoinSet;
```

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

```rust
        assert!(lease.state().udp);
```

换成

```rust
        assert!(lease.state().udp.is_some());
```

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

```rust
        assert!(!lease.state().udp);
```

换成

```rust
        assert!(lease.state().udp.is_none());
```

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

```rust
            assert_eq!(err.message, message);
        }
    }
}

```

换成

```rust
            assert_eq!(err.message, message);
        }
    }

    async fn udp_carrier(out: &Hysteria2Outbound) -> BoxedPacketSocket {
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

    /// Design 5.3: a datagram goes whole in one QUIC datagram of the
    /// connection, in session 1. The carrier reaches any target through
    /// its session's one socket on the server, and hears anyone who sends
    /// to that socket (full cone); TCP goes on beside it.
    #[tokio::test]
    async fn udp_goes_in_datagrams_of_the_connection_to_any_target() {
        let (one, two) = (udp_echo_server().await, udp_echo_server().await);
        let tcp_echo = echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        assert_eq!(out.udp(), UdpSupport::Native);
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), one, b"to one").await;
        udp_round_trip(carrier.as_ref(), two, b"to two").await;
        let whole = |to: SocketAddr| RecordedUdp {
            connection: 0,
            session: 1,
            packet: 0,
            fragment: 0,
            fragments: 1,
            address: to.to_string(),
            len: 6,
        };
        assert_eq!(fake.udp_messages(), [whole(one), whole(two)]);
        let outside = fake.udp_outside();
        assert_eq!(outside.len(), 1, "one socket for the session");
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

    /// A name goes to the server as its A-labels, an IPv6 address in
    /// brackets; the answer names its source as the server heard it. A
    /// name that cannot be sent is refused, and nothing goes out.
    #[tokio::test]
    async fn udp_names_go_out_as_a_labels_and_ipv6_in_brackets() {
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(Hysteria2Script {
            connect_to: Some(echo),
            ..script("pw")
        });
        let out = outbound(&fake, "password=pw");
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
            assert_eq!(fake.udp_messages().last().unwrap().address, address);
        }
        // `name:80` one byte longer than the reference reads
        let long = "a".repeat(2046);
        for name in ["x@blocked.test", "a b.test", "", long.as_str()] {
            let err = carrier
                .send_to(b"q", &Target::new(HostName::Domain(name.into()), 80))
                .await
                .unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(
                err.to_string(),
                "hysteria2: the host name cannot be sent to the server"
            );
        }
        assert_eq!(fake.udp_messages().len(), 2);
    }

    /// Design 5.3: a datagram too long for one QUIC datagram goes in
    /// fragments, each with the whole header, under one packet id that is
    /// not 0; the echo, as long, comes back in fragments of the server's
    /// that the carrier puts together. (At most 9216 bytes: macOS sends
    /// no longer UDP datagram by default.)
    #[tokio::test]
    async fn long_datagrams_go_in_fragments_both_ways() {
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let carrier = udp_carrier(&out).await;
        for len in [3000, 9000] {
            let payload: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            udp_round_trip(carrier.as_ref(), echo, &payload).await;
            let messages = fake.udp_messages();
            let count = messages.last().unwrap().fragments;
            assert!(count > 1, "{count}");
            let fragments = &messages[messages.len() - usize::from(count)..];
            let packet = fragments[0].packet;
            assert_ne!(packet, 0);
            for (i, fragment) in fragments.iter().enumerate() {
                assert_eq!(
                    (fragment.session, fragment.packet, fragment.fragment),
                    (1, packet, i as u8)
                );
                assert_eq!(fragment.address, echo.to_string());
            }
            assert_eq!(fragments.iter().map(|f| f.len).sum::<usize>(), len);
        }
    }

    /// Design 5.3: a fragment lost loses its datagram and nothing more:
    /// the next one, its fragments under another packet id, comes whole.
    #[tokio::test]
    async fn a_lost_fragment_loses_its_datagram_alone() {
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(Hysteria2Script {
            drop_fragment: Some(1),
            ..script("pw")
        });
        let out = outbound(&fake, "password=pw");
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

    /// The fragments of a newer datagram drop the one under way: its last
    /// fragment, come late, makes nothing whole, and holds nothing up.
    #[tokio::test]
    async fn a_newer_datagram_drops_an_older_one_under_way() {
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"hello").await;
        let session = fake.udp_messages()[0].session;
        let from = "192.0.2.7:7";
        for (packet, fragment, payload) in [
            (7, 0, "old-"),
            (8, 0, "new-"),
            (8, 1, "one"),
            (7, 1, "late"),
        ] {
            fake.send_udp(
                0,
                &UdpMessage {
                    session,
                    packet,
                    fragment,
                    fragments: 2,
                    address: from.to_string(),
                    payload: payload.into(),
                },
            );
        }
        fake.send_udp(0, &UdpMessage::whole(session, from, b"after"));
        let source = Target::new(HostName::parse("192.0.2.7"), 7);
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"new-one".to_vec(), source.clone())
        );
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"after".to_vec(), source)
        );
    }

    /// Design 5.3: the server's word on UDP comes with the authentication.
    /// A server that relays none refuses the carrier, as one that takes no
    /// QUIC datagrams does; TCP goes on, on the same connection.
    #[tokio::test]
    async fn a_server_that_relays_no_udp_refuses_the_carrier() {
        let echo = echo_server().await;
        for (server, refusal) in [
            (
                Hysteria2Script {
                    no_udp: true,
                    ..script("pw")
                },
                "hysteria2: the server does not allow UDP",
            ),
            (
                Hysteria2Script {
                    no_datagrams: true,
                    ..script("pw")
                },
                "hysteria2: the server does not support QUIC datagrams",
            ),
        ] {
            let fake = FakeHysteria2::spawn(server);
            let out = outbound(&fake, "password=pw");
            assert_eq!(out.udp(), UdpSupport::Native);
            for _ in 0..2 {
                let err = timeout(BOUND, out.open_udp(&ConnectOpts::default()))
                    .await
                    .expect("an outcome within the bound")
                    .err()
                    .expect("refused");
                assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
                assert_eq!(err.to_string(), refusal);
            }
            let mut stream = tunnel(&out, &target(echo)).await;
            round_trip(&mut stream, b"tcp still").await;
            assert_eq!(fake.connections(), 1);
        }
    }

    /// A carrier that does not read loses what its inbox cannot hold, and
    /// holds up no other carrier of the connection.
    #[tokio::test]
    async fn a_full_inbox_drops_rather_than_holds_up_the_other_carriers() {
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let (slow, other) = (udp_carrier(&out).await, udp_carrier(&out).await);
        udp_round_trip(slow.as_ref(), echo, b"slow").await;
        udp_round_trip(other.as_ref(), echo, b"other").await;
        let sessions: Vec<u32> = fake.udp_messages().iter().map(|m| m.session).collect();
        assert_eq!(sessions, [1, 2]);
        let from = "192.0.2.9:9";
        for i in 0..udp::INBOX as u32 + 10 {
            fake.send_udp(0, &UdpMessage::whole(1, from, &i.to_be_bytes()));
        }
        fake.send_udp(0, &UdpMessage::whole(2, from, b"not held up"));
        assert_eq!(udp_answer(other.as_ref()).await.0, b"not held up");
        for i in 0..udp::INBOX as u32 {
            assert_eq!(udp_answer(slow.as_ref()).await.0, i.to_be_bytes());
        }
        no_udp_answer(slow.as_ref()).await;
    }

    /// Design 5.3: a carrier whose connection ended goes on, under the
    /// same session id, on the connection that replaces it — whether the
    /// server closed the connection, or lost it in a restart (a stateless
    /// reset ends it at the next datagram). A carrier opened later gets an
    /// id of its own.
    #[tokio::test]
    async fn a_carrier_keeps_its_session_id_on_the_next_connection() {
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"first").await;
        fake.close_connections();
        until_answered(carrier.as_ref(), echo, b"after a close").await;
        fake.restart();
        until_answered(carrier.as_ref(), echo, b"after a restart").await;
        let messages = fake.udp_messages();
        assert!(messages.iter().all(|m| m.session == 1), "{messages:?}");
        let connections: BTreeSet<usize> = messages.iter().map(|m| m.connection).collect();
        assert_eq!(connections, BTreeSet::from([0, 1, 2]));
        let later = udp_carrier(&out).await;
        udp_round_trip(later.as_ref(), echo, b"later").await;
        assert_eq!(fake.udp_messages().last().unwrap().session, 2);
    }

    /// Let go of by the client: closed as its session closes a connection,
    /// with H3_NO_ERROR.
    async fn closed_by_the_client(connection: &quinn::Connection) {
        let closed = timeout(BOUND, connection.closed()).await.expect("closed");
        assert!(
            matches!(&closed, ConnectionError::ApplicationClosed(close)
                if close.error_code == VarInt::from_u32(0x100)),
            "{closed:?}"
        );
    }

    /// Design 5.3: a server going away (an HTTP/3 GOAWAY) takes no new UDP
    /// either: the carrier's next datagram goes on a new connection, under
    /// the session id it had, and the connection before is closed once
    /// nothing holds it.
    #[tokio::test]
    async fn a_carrier_moves_off_a_connection_that_is_going_away() {
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"before").await;
        let lease = timeout(BOUND, out.session.lease(&ConnectOpts::default()))
            .await
            .unwrap()
            .unwrap();
        fake.goaway(0);
        let deadline = Instant::now() + BOUND;
        while out.session.serves(&lease) {
            assert!(Instant::now() < deadline, "no GOAWAY heard");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        drop(lease);
        udp_round_trip(carrier.as_ref(), echo, b"after").await;
        let carried: Vec<(usize, u32)> = fake
            .udp_messages()
            .iter()
            .map(|m| (m.connection, m.session))
            .collect();
        assert_eq!(carried, [(0, 1), (1, 1)]);
        closed_by_the_client(&fake.connection(0)).await;
    }

    /// Dials through `inner`, the connection over its UDP flow as over an
    /// `underlying-proxy`'s; where it leads is what `route` holds now, as a
    /// group that is the underlying proxy leads to its member.
    struct Rerouted {
        inner: DirectConnector,
        route: Mutex<Arc<u8>>,
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
            Some(Route::of(&self.route.lock().unwrap()))
        }
    }

    /// Phase 2 M7 design 3.5 and 5.3: the underlying policy leads elsewhere
    /// now (a group's member changed). Each carrier's next datagram goes on
    /// a new connection, dialled the new way, under the session id it had;
    /// the tunnel under way stays on the connection before, which is closed
    /// once nothing holds it. Said once in the log, which names the policy
    /// and nothing secret.
    #[tokio::test]
    async fn carriers_move_off_a_connection_whose_underlying_policy_changed() {
        const PASSWORD: &str = "s3cret-hy2";
        let (log, _logging, _second) = Log::capture();
        let (udp_echo, tcp_echo) = (udp_echo_server().await, echo_server().await);
        let fake = FakeHysteria2::spawn(script(PASSWORD));
        let rerouted = Arc::new(Rerouted {
            inner: DirectConnector::new(Arc::new(SystemResolve)),
            route: Mutex::new(Arc::new(1)),
        });
        let port = fake.addr().port();
        let out = Hysteria2Outbound::new(
            "H",
            Target::new(HostName::parse("127.0.0.1"), port),
            &spec(port, &format!("password={PASSWORD}")),
            Tristate::Auto,
            &[],
            fake.roots(),
            rerouted.clone(),
        )
        .unwrap();
        let (one, two) = (udp_carrier(&out).await, udp_carrier(&out).await);
        udp_round_trip(one.as_ref(), udp_echo, b"one").await;
        udp_round_trip(two.as_ref(), udp_echo, b"two").await;
        let mut under_way = tunnel(&out, &target(tcp_echo)).await;
        *rerouted.route.lock().unwrap() = Arc::new(2);
        udp_round_trip(one.as_ref(), udp_echo, b"one, moved").await;
        udp_round_trip(two.as_ref(), udp_echo, b"two, moved").await;
        round_trip(&mut under_way, b"still there").await;
        let carried: Vec<(usize, u32)> = fake
            .udp_messages()
            .iter()
            .map(|m| (m.connection, m.session))
            .collect();
        assert_eq!(carried, [(0, 1), (0, 2), (1, 1), (1, 2)]);
        assert_eq!(fake.connections(), 2);
        drop(under_way);
        closed_by_the_client(&fake.connection(0)).await;
        let log = log.text();
        let changed = "hysteria2: the underlying policy changed; a new connection is made";
        assert_eq!(log.matches(changed).count(), 1, "{log}");
        assert!(log.contains("policy=H"), "{log}");
        assert!(!log.contains(PASSWORD));
    }

    /// Carriers that open together share one connection and one
    /// authentication, each a session of its own, with a socket of its own
    /// on the server.
    #[tokio::test]
    async fn carriers_that_come_together_share_one_connection_each_its_own_session() {
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = Arc::new(outbound(&fake, "password=pw"));
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
        let sessions: BTreeSet<u32> = fake.udp_messages().iter().map(|m| m.session).collect();
        assert_eq!(sessions, (1..=16).collect());
        assert_eq!(fake.udp_outside().len(), 16);
    }

    /// A datagram that 255 fragments cannot carry, or whose name leaves no
    /// room in a datagram, is refused, and nothing goes out; the carrier
    /// goes on.
    #[tokio::test]
    async fn a_datagram_too_long_is_refused_and_nothing_goes_out() {
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let carrier = udp_carrier(&out).await;
        // past 255 of the longest QUIC datagrams a path carries here
        let err = carrier
            .send_to(&vec![0u8; 400_000], &target(echo))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(
            err.to_string()
                .starts_with("hysteria2: a datagram longer than "),
            "{err}"
        );
        let name = Target::new(HostName::Domain("a".repeat(1500)), 53);
        let err = carrier.send_to(b"q", &name).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            "hysteria2: the host name is too long for a datagram"
        );
        assert!(fake.udp_messages().is_empty());
        udp_round_trip(carrier.as_ref(), echo, b"still").await;
    }

    /// Design 5.3: a carrier gone is forgotten by its connection; what the
    /// server sends for its session afterwards is dropped.
    #[tokio::test]
    async fn a_carrier_gone_is_forgotten_by_its_connection() {
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let lease = timeout(BOUND, out.session.lease(&ConnectOpts::default()))
            .await
            .unwrap()
            .unwrap();
        let udp = lease.state().udp.as_ref().expect("the server relays UDP");
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"here").await;
        assert_eq!(udp.sessions(), [1]);
        drop(carrier);
        assert!(udp.sessions().is_empty());
        fake.send_udp(0, &UdpMessage::whole(1, "192.0.2.1:53", b"too late"));
        let next = udp_carrier(&out).await;
        udp_round_trip(next.as_ref(), echo, b"next").await;
        assert_eq!(udp.sessions(), [2]);
    }

    /// The task that reads a connection's datagrams ends with it.
    #[tokio::test]
    async fn the_reader_of_datagrams_ends_with_its_connection() {
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let lease = timeout(BOUND, out.session.lease(&ConnectOpts::default()))
            .await
            .unwrap()
            .unwrap();
        let udp = lease.state().udp.as_ref().expect("the server relays UDP");
        assert!(udp.is_reading());
        fake.close_connections();
        let deadline = Instant::now() + BOUND;
        while udp.is_reading() {
            assert!(Instant::now() < deadline, "still reading");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// `opts.timeout` bounds opening a carrier: the dial and the
    /// authentication.
    #[tokio::test]
    async fn opening_a_carrier_is_bounded_by_its_time() {
        // nothing speaks QUIC on this port
        let quiet = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = quiet.local_addr().unwrap().port();
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound_with(&spec(port, "password=pw"), port, &fake);
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
    /// line, not even at the finest level — whether they go through, are
    /// refused, are no message, find no session, or go out again after a
    /// close.
    #[tokio::test]
    async fn payloads_never_show_in_errors_or_logs() {
        const MARK: &[u8] = b"payload-7c1e9a";
        let (log, _logging, _second) = Log::capture();
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, MARK).await;
        let err = carrier
            .send_to(&MARK.repeat(30_000), &target(echo))
            .await
            .unwrap_err();
        // no message (an address of no length), then one for no session:
        // read before the echo that follows them
        let mut junk = vec![0, 0, 0, 1, 0, 0, 0, 1, 0];
        junk.extend_from_slice(MARK);
        fake.connection(0).send_datagram(junk.into()).unwrap();
        fake.send_udp(0, &UdpMessage::whole(9, "192.0.2.1:53", MARK));
        udp_round_trip(carrier.as_ref(), echo, MARK).await;
        fake.close_connections();
        until_answered(carrier.as_ref(), echo, MARK).await;
        let text = format!("{err} {err:?}");
        assert!(!text.contains("payload-7c1e9a"), "{text}");
        let log = log.text();
        assert!(
            log.contains("hysteria2: a datagram that is no UDP message is dropped"),
            "{log}"
        );
        assert!(
            log.contains("UDP session 1 moves to a new connection"),
            "{log}"
        );
        assert!(!log.contains("payload-7c1e9a"));
    }
}

```

UDP 模块与它要的前置改动（依赖、认证状态里的分发器、地址编码从 `tcp.rs` 移到 `mod.rs` 共用）；`udp()` / `open_udp` 的接线在 Step 3：

`crates/rurge-proto-quic/Cargo.toml`——把

```toml
rurge-proto.workspace = true
```

换成

```toml
rurge-proto.workspace = true
bytes.workspace = true
```

`crates/rurge-proto-quic/Cargo.toml`——把

```toml
rurge-proto = { workspace = true, features = ["testing"] }
```

换成

```toml
rurge-proto = { workspace = true, features = ["testing"] }
tracing-subscriber.workspace = true
```

新建 `crates/rurge-proto-quic/src/hysteria2/udp.rs`：

```rust
//! UDP over Hysteria 2 (Hysteria 2 protocol, "UDP"; phase 2 M7 design
//! 5.3): every datagram in QUIC datagrams of the policy's connection, each
//! way.
//!
//! - A message is `u32 session ‖ u16 packet ‖ u8 fragment ‖ u8 fragments ‖
//!   varint length ‖ host:port ‖ payload`, big endian; the address names
//!   the target going out and the source coming back.
//! - Each carrier — one client association on the outbound — is a session
//!   of its own: every target through it, and answers from anyone (full
//!   cone, as the server keeps a socket per session). A carrier moves to
//!   the session's connection on its next datagram once its own takes
//!   nothing new — ended, going away, or dialled another way than the
//!   policy leads now (`Session::serves`). Session ids count from 1 per
//!   outbound, so it keeps its id there, where no other carrier holds it:
//!   the server takes it for a new session.
//! - A datagram that does not fit one QUIC datagram goes in fragments — at
//!   most 255, under a random packet id, each with the whole header —; one
//!   that fits goes whole, packet 0, fragment 0 of 1.
//! - One task per connection reads its datagrams into their sessions'
//!   inboxes, putting fragments together first: one datagram at a time per
//!   session, a fragment of another dropping the one under way. What finds
//!   no session, or a full inbox, is dropped.
//! - The protocol has no message that ends a session: a carrier that goes
//!   only forgets its id, and the server reclaims the session once it
//!   idles.

use super::auth::{Auth, Authenticated};
use super::{MAX_ADDRESS, address};
use crate::session::{Lease, Session};
use crate::varint;
use bytes::Bytes;
use quinn::Connection;
use rurge_config::HostName;
use rurge_net::BoxFuture;
use rurge_net::connector::{ConnectOpts, PacketSocket, Target};
use rurge_proto::OutboundError;
use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::task::AbortOnDropHandle;

/// Datagrams waiting for the engine, per carrier; more are dropped, as a
/// socket would.
pub(super) const INBOX: usize = 64;
/// The most fragments a datagram goes in: their count is one byte.
const MAX_FRAGMENTS: usize = 255;
/// The longest datagram put together: no UDP datagram is longer.
const MAX_DATAGRAM: usize = 65_535;
/// `u32 session ‖ u16 packet ‖ u8 fragment ‖ u8 fragments`.
const FIXED: usize = 8;

/// A datagram for the engine, and where it came from.
type Packet = (Bytes, Target);

/// The session ids of an outbound's carriers: from 1 upward, never 0.
#[derive(Default)]
pub(super) struct SessionIds(AtomicU32);

impl SessionIds {
    fn next(&self) -> u32 {
        loop {
            match self.0.fetch_add(1, Ordering::Relaxed).wrapping_add(1) {
                0 => continue,
                id => return id,
            }
        }
    }
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("hysteria2: {what}"))
}

/// The QUIC datagrams, none longer than `max`, that carry `payload` to
/// `address` in session `id`: one when it fits — packet 0, fragment 0 of 1
/// —, else fragments under a random packet id, each with the whole header
/// (the reference's `FragUDPMessage`). Refused when 255 fragments cannot
/// carry it.
fn datagrams(id: u32, address: &str, payload: &[u8], max: usize) -> io::Result<Vec<Vec<u8>>> {
    let mut header = Vec::with_capacity(FIXED + 2 + address.len());
    header.extend_from_slice(&id.to_be_bytes());
    // packet 0, fragment 0 of 1; a fragment says otherwise
    header.extend_from_slice(&[0, 0, 0, 1]);
    varint::put(&mut header, address.len() as u64);
    header.extend_from_slice(address.as_bytes());
    if header.len() + payload.len() <= max {
        header.extend_from_slice(payload);
        return Ok(vec![header]);
    }
    let room = max.saturating_sub(header.len());
    if room == 0 {
        return Err(invalid("the host name is too long for a datagram"));
    }
    if payload.len() > MAX_FRAGMENTS * room {
        return Err(invalid(&format!(
            "a datagram longer than {} bytes",
            MAX_FRAGMENTS * room
        )));
    }
    let fragments = payload.len().div_ceil(room) as u8;
    let packet = packet_id().to_be_bytes();
    Ok(payload
        .chunks(room)
        .enumerate()
        .map(|(fragment, chunk)| {
            let mut datagram = Vec::with_capacity(header.len() + chunk.len());
            datagram.extend_from_slice(&header);
            datagram[4..6].copy_from_slice(&packet);
            datagram[6] = fragment as u8;
            datagram[7] = fragments;
            datagram.extend_from_slice(chunk);
            datagram
        })
        .collect())
}

/// A fragmented datagram's packet id: random, 1 to 65535, as the reference
/// client draws it (`core/client/udp.go`); 1 without a random source.
fn packet_id() -> u16 {
    getrandom::u32().map_or(1, |random| (random % 65_535) as u16 + 1)
}

/// A message as it came. The address is one the reference reads (1 to 2048
/// bytes, `ParseUDPMessage`); the payload is whatever follows, nothing
/// included.
struct Message {
    session: u32,
    packet: u16,
    fragment: u8,
    fragments: u8,
    address: Bytes,
    payload: Bytes,
}

impl Message {
    fn parse(datagram: Bytes) -> Option<Message> {
        let fixed: [u8; FIXED] = datagram.get(..FIXED)?.try_into().ok()?;
        let (len, n) = varint::get(&datagram[FIXED..])?;
        let len = usize::try_from(len)
            .ok()
            .filter(|len| (1..=MAX_ADDRESS).contains(len))?;
        let start = FIXED + n;
        let end = start + len;
        if end > datagram.len() {
            return None;
        }
        Some(Message {
            session: u32::from_be_bytes([fixed[0], fixed[1], fixed[2], fixed[3]]),
            packet: u16::from_be_bytes([fixed[4], fixed[5]]),
            fragment: fixed[6],
            fragments: fixed[7],
            address: datagram.slice(start..end),
            payload: datagram.slice(end..),
        })
    }
}

/// Where a datagram came from: `host:port`, the host an address (IPv6 in
/// brackets) or a name; `None` for anything else.
fn source(address: &[u8]) -> Option<Target> {
    let (host, port) = std::str::from_utf8(address).ok()?.rsplit_once(':')?;
    Some(Target::new(HostName::from_wire(host)?, port.parse().ok()?))
}

/// A session's datagram put together from its fragments (design 5.3, the
/// reference's `Defragger`): one at a time — a fragment of another drops
/// the one under way —, whole once every fragment came, never when one is
/// lost.
#[derive(Default)]
enum Reassembly {
    #[default]
    Idle,
    Collecting {
        packet: u16,
        parts: Vec<Option<Bytes>>,
        missing: usize,
        size: usize,
    },
    /// Handed on, or dropped: whatever more comes of it is dropped too.
    Over { packet: u16, fragments: u8 },
}

impl Reassembly {
    /// The datagram `message` makes whole: the message's own payload when
    /// it is no fragment — the datagram under way stays —; `None` while
    /// fragments are missing.
    fn feed(&mut self, message: &Message) -> Option<Bytes> {
        let count = message.fragments;
        // a count of 0 too, as the reference reads it
        if count <= 1 {
            return Some(message.payload.clone());
        }
        if message.fragment >= count {
            return None;
        }
        let known = match self {
            Reassembly::Idle => false,
            Reassembly::Collecting { packet, parts, .. } => {
                *packet == message.packet && parts.len() == usize::from(count)
            }
            Reassembly::Over { packet, fragments } => {
                *packet == message.packet && *fragments == count
            }
        };
        if !known {
            // another datagram: the one under way is lost
            *self = Reassembly::Collecting {
                packet: message.packet,
                parts: vec![None; usize::from(count)],
                missing: usize::from(count),
                size: 0,
            };
        }
        let Reassembly::Collecting {
            parts,
            missing,
            size,
            ..
        } = self
        else {
            // of one handed on or dropped
            return None;
        };
        let part = &mut parts[usize::from(message.fragment)];
        if part.is_some() {
            // seen already
            return None;
        }
        *size += message.payload.len();
        if *size > MAX_DATAGRAM {
            *self = Reassembly::Over {
                packet: message.packet,
                fragments: count,
            };
            return None;
        }
        *part = Some(message.payload.clone());
        *missing -= 1;
        if *missing > 0 {
            return None;
        }
        let mut whole = Vec::with_capacity(*size);
        for part in parts.iter().flatten() {
            whole.extend_from_slice(part);
        }
        *self = Reassembly::Over {
            packet: message.packet,
            fragments: count,
        };
        Some(whole.into())
    }
}

/// A carrier's session on a connection.
struct Entry {
    inbox: mpsc::Sender<Packet>,
    reassembly: Reassembly,
}

type Sessions = Mutex<HashMap<u32, Entry>>;

/// A connection's UDP sessions, when its server relays UDP: the inbox of
/// each, and the task that reads the connection's datagrams into them. The
/// task ends with the connection, or when this is dropped.
pub(super) struct Datagrams {
    sessions: Arc<Sessions>,
    _reader: AbortOnDropHandle<()>,
}

impl Datagrams {
    pub(super) fn start(connection: &Connection) -> Datagrams {
        let sessions = Arc::new(Sessions::default());
        let reader = tokio::spawn(read(connection.clone(), sessions.clone()));
        Datagrams {
            sessions,
            _reader: AbortOnDropHandle::new(reader),
        }
    }

    /// Takes `inbox` in as session `id` — or, should a carrier hold `id`
    /// here, which only an outbound that went through every id can cause,
    /// as the next id of `ids` free here —, and says which.
    fn join(&self, mut id: u32, inbox: &mpsc::Sender<Packet>, ids: &SessionIds) -> u32 {
        let mut sessions = self.sessions.lock().expect("sessions");
        while sessions.contains_key(&id) {
            id = ids.next();
        }
        sessions.insert(
            id,
            Entry {
                inbox: inbox.clone(),
                reassembly: Reassembly::default(),
            },
        );
        id
    }

    /// Forgets session `id`: what comes for it from now on is dropped.
    fn leave(&self, id: u32) {
        self.sessions.lock().expect("sessions").remove(&id);
    }

    /// Test support: the ids of the sessions, in order.
    #[cfg(test)]
    pub(super) fn sessions(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self
            .sessions
            .lock()
            .expect("sessions")
            .keys()
            .copied()
            .collect();
        ids.sort_unstable();
        ids
    }

    /// Test support: whether the task still reads.
    #[cfg(test)]
    pub(super) fn is_reading(&self) -> bool {
        !self._reader.is_finished()
    }
}

/// Reads `connection`'s datagrams into their sessions' inboxes until it
/// ends — those that came before its end included.
async fn read(connection: Connection, sessions: Arc<Sessions>) {
    while let Ok(datagram) = connection.read_datagram().await {
        deliver(&sessions, datagram);
    }
}

/// Hands `datagram` to its session's inbox, once the datagram it carries
/// is whole. Dropped when it is no message, for no session of the
/// connection (a carrier gone), or meets a full inbox: the reader waits for
/// no carrier, so that one that reads slowly holds up no other.
fn deliver(sessions: &Sessions, datagram: Bytes) {
    let Some(message) = Message::parse(datagram) else {
        tracing::trace!("hysteria2: a datagram that is no UDP message is dropped");
        return;
    };
    let mut sessions = sessions.lock().expect("sessions");
    let Some(entry) = sessions.get_mut(&message.session) else {
        return;
    };
    let Some(whole) = entry.reassembly.feed(&message) else {
        return;
    };
    if let Some(from) = source(&message.address) {
        let _ = entry.inbox.try_send((whole, from));
    }
}

/// Where a carrier is: its session id on one connection, which its lease
/// keeps. Dropped, the connection forgets the session.
struct Bound {
    id: u32,
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
        if let Some(udp) = &self.lease.state().udp {
            udp.leave(self.id);
        }
    }
}

fn refused(what: &str) -> OutboundError {
    OutboundError::Proxy(format!("hysteria2: {what}"))
}

/// One client association's UDP through the outbound (design 5.3): a
/// session on the policy's connection, then on whichever connection
/// replaces it. No `Debug`: its session holds the password.
pub(super) struct Hysteria2Udp {
    session: Arc<Session<Auth>>,
    ids: Arc<SessionIds>,
    /// The budget of binding to a new connection, its dial included.
    opts: ConnectOpts,
    bound: Mutex<Option<Arc<Bound>>>,
    /// What the connections it binds to fill `inbox` through.
    answers: mpsc::Sender<Packet>,
    inbox: tokio::sync::Mutex<mpsc::Receiver<Packet>>,
}

impl Hysteria2Udp {
    /// A carrier on the session's connection, dialed when there is none.
    /// Nothing bounds the time it takes here.
    pub(super) async fn open(
        session: Arc<Session<Auth>>,
        ids: Arc<SessionIds>,
        opts: &ConnectOpts,
    ) -> Result<Hysteria2Udp, OutboundError> {
        let (answers, inbox) = mpsc::channel(INBOX);
        let udp = Hysteria2Udp {
            session,
            ids,
            opts: opts.clone(),
            bound: Mutex::default(),
            answers,
            inbox: tokio::sync::Mutex::new(inbox),
        };
        udp.bind().await?;
        Ok(udp)
    }

    /// Where the carrier is, while that connection takes what is new
    /// (`Session::serves`).
    fn current(&self) -> Option<Arc<Bound>> {
        let bound = self.bound.lock().expect("bound").clone()?;
        self.session.serves(&bound.lease).then_some(bound)
    }

    /// Binds the carrier to the session's connection — a lease on it, the
    /// connection dialed when there is none — under the session id it had,
    /// a new one the first time. Refused when that connection's server
    /// relays no UDP.
    async fn bind(&self) -> Result<Arc<Bound>, OutboundError> {
        let lease = self.session.lease(&self.opts).await?;
        let Some(udp) = &lease.state().udp else {
            return Err(refused("the server does not allow UDP"));
        };
        if lease.connection().max_datagram_size().is_none() {
            return Err(refused("the server does not support QUIC datagrams"));
        }
        let mut bound = self.bound.lock().expect("bound");
        // a send bound it meanwhile, to this very connection
        if let Some(current) = bound.as_ref().filter(|bound| bound.is_on(&lease)) {
            return Ok(current.clone());
        }
        let id = bound.as_ref().map_or_else(|| self.ids.next(), |old| old.id);
        let id = udp.join(id, &self.answers, &self.ids);
        if bound.is_some() {
            tracing::debug!("hysteria2: UDP session {id} moves to a new connection");
        }
        let new = Arc::new(Bound { id, lease });
        let old = bound.replace(new.clone());
        drop(bound);
        // the connection before forgets the session, out of the lock
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
}

/// Hands `datagrams` to QUIC in order, never waiting: a full send buffer
/// makes room by dropping the oldest datagrams in it. A path that has
/// narrowed since their size was taken drops the datagram they carry, as
/// any hop may, and so does a connection that closed just now — the next
/// send finds it closed and binds anew.
fn transmit(connection: &Connection, datagrams: Vec<Vec<u8>>) {
    for datagram in datagrams {
        if let Err(e) = connection.send_datagram(datagram.into()) {
            // the rest is no use without this one
            tracing::debug!("hysteria2: a datagram for the server is dropped: {e}");
            return;
        }
    }
}

impl PacketSocket for Hysteria2Udp {
    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            // checked first: nothing is dialed for what cannot be sent
            let Some(address) = address(to) else {
                return Err(invalid("the host name cannot be sent to the server"));
            };
            let bound = match self.current() {
                Some(bound) => bound,
                // its connection takes nothing new: the session's, the
                // same id
                None => self.rebind().await?,
            };
            let connection = bound.lease.connection();
            let Some(max) = connection.max_datagram_size() else {
                return Err(io::Error::other(
                    "hysteria2: the server does not support QUIC datagrams",
                ));
            };
            transmit(connection, datagrams(bound.id, &address, buf, max)?);
            Ok(())
        })
    }

    /// A datagram longer than `buf` is dropped: give it 64 KiB.
    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            let mut inbox = self.inbox.lock().await;
            loop {
                // `self` holds a sender: the inbox never closes
                let Some((datagram, from)) = inbox.recv().await else {
                    return Err(io::ErrorKind::BrokenPipe.into());
                };
                if let Some(space) = buf.get_mut(..datagram.len()) {
                    space.copy_from_slice(&datagram);
                    return Ok((datagram.len(), from));
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const SOURCE: &str = "192.0.2.1:53";

    fn parse(datagram: &[u8]) -> Option<Message> {
        Message::parse(Bytes::copy_from_slice(datagram))
    }

    /// `(session, packet, fragment, fragments)` of `message`.
    fn ids(message: &Message) -> (u32, u16, u8, u8) {
        (
            message.session,
            message.packet,
            message.fragment,
            message.fragments,
        )
    }

    #[test]
    fn a_datagram_that_fits_goes_whole() {
        let mut expected = vec![1, 2, 3, 4, 0, 0, 0, 1, 12];
        expected.extend_from_slice(b"192.0.2.1:53query");
        assert_eq!(
            datagrams(0x0102_0304, SOURCE, b"query", 1200).unwrap(),
            [expected]
        );
        // exactly as long as the limit
        let payload = vec![7u8; 1200 - 8 - 1 - SOURCE.len()];
        let whole = datagrams(1, SOURCE, &payload, 1200).unwrap();
        assert_eq!(whole.len(), 1);
        assert_eq!(whole[0].len(), 1200);
        // nothing to carry is a datagram too
        assert_eq!(datagrams(1, SOURCE, b"", 1200).unwrap()[0].len(), 21);
        // an address of 64 bytes or more: its length in two bytes
        let name = format!("{}.test:53", "a".repeat(80));
        assert_eq!(
            &datagrams(1, &name, b"x", 1200).unwrap()[0][8..10],
            [0x40, 88]
        );
    }

    #[test]
    fn what_does_not_fit_goes_in_fragments_each_with_the_whole_header() {
        let payload: Vec<u8> = (0..3000).map(|i| (i % 251) as u8).collect();
        let address = "[2001:db8::1]:443";
        // 8 + 1 + 17 bytes of header: 974 of payload each
        let fragments = datagrams(9, address, &payload, 1000).unwrap();
        assert_eq!(fragments.len(), 4);
        let packet = parse(&fragments[0]).unwrap().packet;
        assert_ne!(packet, 0);
        let mut whole = Vec::new();
        for (i, datagram) in fragments.iter().enumerate() {
            assert!(datagram.len() <= 1000, "{}", datagram.len());
            let message = parse(datagram).unwrap();
            assert_eq!(ids(&message), (9, packet, i as u8, 4));
            assert_eq!(&message.address[..], address.as_bytes());
            whole.extend_from_slice(&message.payload);
        }
        assert_eq!(whole, payload);
        // a byte over the limit: two fragments
        let over = vec![1u8; 1000 - 8 - 1 - SOURCE.len() + 1];
        assert_eq!(datagrams(1, SOURCE, &over, 1000).unwrap().len(), 2);
        // the packet id is drawn anew for every datagram
        let drawn: HashSet<u16> = (0..50)
            .map(|_| {
                parse(&datagrams(1, SOURCE, &over, 1000).unwrap()[0])
                    .unwrap()
                    .packet
            })
            .collect();
        assert!(drawn.len() > 1, "{drawn:?}");
        assert!(!drawn.contains(&0));
    }

    #[test]
    fn a_datagram_255_fragments_cannot_carry_is_refused() {
        // 1000 - (8 + 1 + 12) = 979 bytes of payload in each fragment
        let mut payload = vec![0u8; 255 * 979];
        assert_eq!(datagrams(1, SOURCE, &payload, 1000).unwrap().len(), 255);
        payload.push(0);
        let err = datagrams(1, SOURCE, &payload, 1000).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            "hysteria2: a datagram longer than 249645 bytes"
        );
        // a header that leaves no room, nothing to carry included
        let long = format!("{}:53", "a".repeat(990));
        for payload in [&b"x"[..], b""] {
            let err = datagrams(1, &long, payload, 1000).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(
                err.to_string(),
                "hysteria2: the host name is too long for a datagram"
            );
        }
    }

    #[test]
    fn a_message_is_read_as_the_reference_reads_it() {
        // the address's length in two bytes, as QUIC allows
        let mut datagram = vec![0, 0, 0, 7, 0x12, 0x34, 1, 3, 0x40, 9];
        datagram.extend_from_slice(b"1.2.3.4:5payload");
        let message = parse(&datagram).unwrap();
        assert_eq!(ids(&message), (7, 0x1234, 1, 3));
        assert_eq!(&message.address[..], b"1.2.3.4:5");
        assert_eq!(&message.payload[..], b"payload");
        // no payload at all
        let message = parse(&datagram[..datagram.len() - 7]).unwrap();
        assert!(message.payload.is_empty());
        // the longest address the reference reads
        let mut longest = vec![0, 0, 0, 1, 0, 0, 0, 1, 0x48, 0x00];
        longest.extend_from_slice(&[b'a'; 2048]);
        assert_eq!(parse(&longest).unwrap().address.len(), 2048);
        for bad in [
            &[0u8; 7][..],
            // no address length, an address of none or of 2049 bytes
            &[0, 0, 0, 1, 0, 0, 0, 1][..],
            &[0, 0, 0, 1, 0, 0, 0, 1, 0][..],
            &[0, 0, 0, 1, 0, 0, 0, 1, 0x48, 0x01][..],
            // an address cut short
            &[0, 0, 0, 1, 0, 0, 0, 1, 5, b'a', b'b'][..],
            &[0, 0, 0, 1, 0, 0, 0, 1, 0x40][..],
        ] {
            assert!(parse(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn the_source_is_a_host_and_a_port() {
        for (address, host, port) in [
            ("192.0.2.1:53", "192.0.2.1", 53),
            ("[2001:db8::1]:443", "2001:db8::1", 443),
            ("dns.example:53", "dns.example", 53),
        ] {
            assert_eq!(
                source(address.as_bytes()),
                Some(Target::new(HostName::parse(host), port)),
                "{address}"
            );
        }
        for bad in [
            &b""[..],
            b"192.0.2.1",
            b":53",
            b"192.0.2.1:65536",
            b"192.0.2.1:",
            b"a b.example:53",
            b"a\r\nb:53",
            b"\xff\xfe:53",
        ] {
            assert_eq!(source(bad), None, "{bad:?}");
        }
    }

    fn fragment(packet: u16, fragment: u8, fragments: u8, payload: &[u8]) -> Message {
        Message {
            session: 1,
            packet,
            fragment,
            fragments,
            address: Bytes::from_static(SOURCE.as_bytes()),
            payload: Bytes::copy_from_slice(payload),
        }
    }

    #[test]
    fn fragments_make_their_datagram_whole_in_any_order() {
        let mut reassembly = Reassembly::default();
        assert_eq!(reassembly.feed(&fragment(5, 2, 3, b"ef")), None);
        assert_eq!(reassembly.feed(&fragment(5, 0, 3, b"ab")), None);
        assert_eq!(
            reassembly.feed(&fragment(5, 1, 3, b"cd")).as_deref(),
            Some(&b"abcdef"[..])
        );
        // seen again once whole: no datagram anew
        assert_eq!(reassembly.feed(&fragment(5, 1, 3, b"cd")), None);
        assert_eq!(reassembly.feed(&fragment(5, 0, 3, b"ab")), None);
    }

    #[test]
    fn a_fragment_of_another_datagram_drops_the_one_under_way() {
        let mut reassembly = Reassembly::default();
        assert_eq!(reassembly.feed(&fragment(7, 0, 2, b"old-")), None);
        assert_eq!(reassembly.feed(&fragment(8, 0, 2, b"new-")), None);
        assert_eq!(
            reassembly.feed(&fragment(8, 1, 2, b"one")).as_deref(),
            Some(&b"new-one"[..])
        );
        // the old one's rest, late, makes nothing whole
        assert_eq!(reassembly.feed(&fragment(7, 1, 2, b"late")), None);
        // nor does the same packet id with another count
        assert_eq!(reassembly.feed(&fragment(9, 0, 2, b"a")), None);
        assert_eq!(reassembly.feed(&fragment(9, 1, 3, b"b")), None);
        assert_eq!(reassembly.feed(&fragment(9, 1, 2, b"c")), None);
    }

    #[test]
    fn whole_datagrams_pass_while_one_is_put_together() {
        let mut reassembly = Reassembly::default();
        assert_eq!(reassembly.feed(&fragment(3, 0, 2, b"a")), None);
        // a count of 1 — or 0 — makes a whole datagram, whatever the ids
        assert_eq!(
            reassembly.feed(&fragment(9, 4, 1, b"whole")).as_deref(),
            Some(&b"whole"[..])
        );
        assert_eq!(
            reassembly.feed(&fragment(0, 0, 0, b"zero")).as_deref(),
            Some(&b"zero"[..])
        );
        assert_eq!(
            reassembly.feed(&fragment(3, 1, 2, b"b")).as_deref(),
            Some(&b"ab"[..])
        );
    }

    #[test]
    fn duplicates_and_fragments_past_the_count_are_dropped() {
        let mut reassembly = Reassembly::default();
        assert_eq!(reassembly.feed(&fragment(4, 2, 2, b"x")), None);
        assert_eq!(reassembly.feed(&fragment(4, 0, 2, b"a")), None);
        // the first one stays
        assert_eq!(reassembly.feed(&fragment(4, 0, 2, b"A")), None);
        assert_eq!(reassembly.feed(&fragment(4, 7, 2, b"x")), None);
        assert_eq!(
            reassembly.feed(&fragment(4, 1, 2, b"b")).as_deref(),
            Some(&b"ab"[..])
        );
    }

    #[test]
    fn a_datagram_longer_than_udp_allows_is_dropped() {
        let mut reassembly = Reassembly::default();
        let half = vec![0u8; 32_768];
        assert_eq!(reassembly.feed(&fragment(6, 0, 3, &half)), None);
        assert_eq!(reassembly.feed(&fragment(6, 1, 3, &half)), None);
        // the rest of it too
        assert_eq!(reassembly.feed(&fragment(6, 2, 3, b"x")), None);
        // the longest there is goes through
        let mut reassembly = Reassembly::default();
        assert_eq!(reassembly.feed(&fragment(6, 0, 2, &vec![0; 65_000])), None);
        let whole = reassembly.feed(&fragment(6, 1, 2, &[0; 535])).unwrap();
        assert_eq!(whole.len(), MAX_DATAGRAM);
    }

    #[test]
    fn session_ids_count_from_1_and_never_give_0() {
        let ids = SessionIds::default();
        assert_eq!((ids.next(), ids.next(), ids.next()), (1, 2, 3));
        let ids = SessionIds(AtomicU32::new(u32::MAX - 1));
        assert_eq!(ids.next(), u32::MAX);
        assert_eq!(ids.next(), 1, "0 passed over");
    }

    /// A connection's sessions with one, `id`, whose inbox takes
    /// `capacity`.
    fn one_session(id: u32, capacity: usize) -> (Sessions, mpsc::Receiver<Packet>) {
        let (inbox, rx) = mpsc::channel(capacity);
        let sessions = Sessions::default();
        let entry = Entry {
            inbox,
            reassembly: Reassembly::default(),
        };
        sessions.lock().unwrap().insert(id, entry);
        (sessions, rx)
    }

    fn whole(id: u32, address: &str, payload: &[u8]) -> Bytes {
        datagrams(id, address, payload, 1200)
            .unwrap()
            .remove(0)
            .into()
    }

    #[test]
    fn a_full_inbox_drops_and_never_waits() {
        let (sessions, mut inbox) = one_session(1, INBOX);
        for i in 0..INBOX as u32 + 10 {
            deliver(&sessions, whole(1, SOURCE, &i.to_be_bytes()));
        }
        let mut got = Vec::new();
        while let Ok((datagram, from)) = inbox.try_recv() {
            assert_eq!(from, Target::new(HostName::parse("192.0.2.1"), 53));
            got.push(u32::from_be_bytes(datagram[..].try_into().unwrap()));
        }
        // the first that came; the rest dropped
        assert_eq!(got, (0..INBOX as u32).collect::<Vec<_>>());
    }

    #[test]
    fn what_no_session_takes_is_dropped() {
        let (sessions, mut inbox) = one_session(1, INBOX);
        // for another session, no message at all, from no host and port
        deliver(&sessions, whole(2, SOURCE, b"elsewhere"));
        deliver(&sessions, Bytes::from_static(&[0, 0, 0, 1, 0, 0]));
        deliver(&sessions, whole(1, "nowhere", b"lost"));
        deliver(&sessions, whole(1, SOURCE, b"kept"));
        assert_eq!(&inbox.try_recv().unwrap().0[..], b"kept");
        assert!(inbox.try_recv().is_err());
    }
}
```

`crates/rurge-proto-quic/src/hysteria2/auth.rs`——把

```rust
use super::padding;
```

换成

```rust
use super::padding;
use super::udp::Datagrams;
```

`crates/rurge-proto-quic/src/hysteria2/auth.rs`——把

```rust
    /// `Hysteria-UDP`: whether the server relays UDP.
    // only tests read it until the UDP carriers do (M7a task 6)
    #[allow(dead_code)]
    pub(crate) udp: bool,
```

换成

```rust
    /// The connection's UDP sessions, when the server relays UDP
    /// (`Hysteria-UDP`).
    pub(super) udp: Option<Datagrams>,
```

`crates/rurge-proto-quic/src/hysteria2/auth.rs`——把

```rust
                Ok(udp) => Ok(Authenticated { h3, udp }),
```

换成

```rust
                Ok(udp) => Ok(Authenticated {
                    h3,
                    udp: udp.then(|| Datagrams::start(connection)),
                }),
```

`crates/rurge-proto-quic/src/hysteria2/tcp.rs`——把

```rust
use super::padding;
```

换成

```rust
use super::{address, padding};
```

`crates/rurge-proto-quic/src/hysteria2/tcp.rs`——把

```rust
use rurge_proto::OutboundError;
use rurge_proto::http::wire_host;
```

换成

```rust
use rurge_proto::OutboundError;
```

`crates/rurge-proto-quic/src/hysteria2/tcp.rs`——把

```rust
/// address it reads, the longest message and padding of an answer.
const MAX_ADDRESS: usize = 2048;
```

换成

```rust
/// message and padding of an answer.
```

`crates/rurge-proto-quic/src/hysteria2/tcp.rs`——把

```rust
const MESSAGE_MAX: usize = 200;

/// `host:port` as Hysteria 2 names a target: an IPv6 literal in brackets,
/// a name as its A-labels (`http::wire_host`); `None` for a name that
/// cannot be sent, or an address longer than the server reads.
fn address(target: &Target) -> Option<String> {
    let address = format!("{}:{}", wire_host(target)?, target.port);
    (address.len() <= MAX_ADDRESS).then_some(address)
}
```

换成

```rust
const MESSAGE_MAX: usize = 200;
```

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

```rust
//!   refuses it with a message (`tcp`).
```

换成

```rust
//!   refuses it with a message (`tcp`).
//! - UDP goes in QUIC datagrams of the same connection, each client
//!   association a session of its own there (`udp`).
```

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

```rust
mod tcp;
```

换成

```rust
mod tcp;
mod udp;
```

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

```rust
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_proto::{BuildError, Outbound, OutboundError};
```

换成

```rust
use rurge_net::connector::{BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, Target};
use rurge_proto::http::wire_host;
use rurge_proto::{BuildError, Outbound, OutboundError, UdpSupport};
```

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

```rust
use std::sync::Arc;
```

换成

```rust
use std::sync::Arc;
use udp::{Hysteria2Udp, SessionIds};
```

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

```rust
const PADDING: &[u8; 62] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
```

换成

```rust
const PADDING: &[u8; 62] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// The longest address the reference reads, in a TCP request and in a UDP
/// message alike (`core/internal/protocol/proxy.go`).
const MAX_ADDRESS: usize = 2048;

/// `host:port` as Hysteria 2 names a target: an IPv6 literal in brackets,
/// a name as its A-labels (`http::wire_host`); `None` for a name that
/// cannot be sent, or an address longer than the server reads.
fn address(target: &Target) -> Option<String> {
    let address = format!("{}:{}", wire_host(target)?, target.port);
    (address.len() <= MAX_ADDRESS).then_some(address)
}
```

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

```rust
    session: Session<Auth>,
```

换成

```rust
    /// Shared with its UDP carriers, which may outlive it.
    session: Arc<Session<Auth>>,
    /// The session ids of its UDP carriers.
    ids: Arc<SessionIds>,
```

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

```rust
            session: Session::new(name, dialer, auth),
```

换成

```rust
            session: Arc::new(Session::new(name, dialer, auth)),
            ids: Arc::default(),
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto-quic --lib`
Expected: FAIL——出站还报"不支持 UDP"，UDP 的用例都失败：

```text
test hysteria2::tests::opening_a_carrier_is_bounded_by_its_time ... FAILED
test hysteria2::tests::a_carrier_keeps_its_session_id_on_the_next_connection ... FAILED
test hysteria2::tests::carriers_move_off_a_connection_whose_underlying_policy_changed ... FAILED
test hysteria2::tests::a_newer_datagram_drops_an_older_one_under_way ... FAILED
test hysteria2::tests::long_datagrams_go_in_fragments_both_ways ... FAILED
test hysteria2::tests::a_full_inbox_drops_rather_than_holds_up_the_other_carriers ... FAILED
test hysteria2::tests::payloads_never_show_in_errors_or_logs ... FAILED
test hysteria2::tests::a_server_that_relays_no_udp_refuses_the_carrier ... FAILED
test hysteria2::tests::udp_names_go_out_as_a_labels_and_ipv6_in_brackets ... FAILED
test hysteria2::tests::a_lost_fragment_loses_its_datagram_alone ... FAILED
test hysteria2::tests::carriers_that_come_together_share_one_connection_each_its_own_session ... FAILED
test hysteria2::tests::a_datagram_too_long_is_refused_and_nothing_goes_out ... FAILED
test hysteria2::tests::udp_goes_in_datagrams_of_the_connection_to_any_target ... FAILED
test hysteria2::tests::a_carrier_moves_off_a_connection_that_is_going_away ... FAILED
test hysteria2::tests::a_carrier_gone_is_forgotten_by_its_connection ... FAILED
thread 'hysteria2::tests::opening_a_carrier_is_bounded_by_its_time' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:1167:9:
thread 'hysteria2::tests::a_carrier_keeps_its_session_id_on_the_next_connection' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:624:14:
thread 'hysteria2::tests::carriers_move_off_a_connection_whose_underlying_policy_changed' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:624:14:
thread 'hysteria2::tests::a_newer_datagram_drops_an_older_one_under_way' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:624:14:
thread 'hysteria2::tests::long_datagrams_go_in_fragments_both_ways' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:624:14:
thread 'hysteria2::tests::a_full_inbox_drops_rather_than_holds_up_the_other_carriers' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:624:14:
---- hysteria2::tests::payloads_never_show_in_errors_or_logs stdout ----
thread 'hysteria2::tests::payloads_never_show_in_errors_or_logs' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:624:14:
thread 'hysteria2::tests::a_server_that_relays_no_udp_refuses_the_carrier' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:863:13:
assertion `left == right` failed
  left: Unsupported
 right: Native
thread 'hysteria2::tests::udp_names_go_out_as_a_labels_and_ipv6_in_brackets' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:624:14:
thread 'hysteria2::tests::a_lost_fragment_loses_its_datagram_alone' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:624:14:
thread 'hysteria2::tests::carriers_that_come_together_share_one_connection_each_its_own_session' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:624:14:
thread 'hysteria2::tests::carriers_that_come_together_share_one_connection_each_its_own_session' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:1071:35:
thread 'hysteria2::tests::a_datagram_too_long_is_refused_and_nothing_goes_out' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:624:14:
thread 'hysteria2::tests::udp_goes_in_datagrams_of_the_connection_to_any_target' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:676:9:
thread 'hysteria2::tests::a_carrier_moves_off_a_connection_that_is_going_away' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:624:14:
thread 'hysteria2::tests::a_carrier_gone_is_forgotten_by_its_connection' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:624:14:
    hysteria2::tests::payloads_never_show_in_errors_or_logs
test result: FAILED. 46 passed; 15 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.66s
```

- [ ] **Step 3: 接上 `udp()` 与 `open_udp`**

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

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

    /// Whatever the policy says: Hysteria 2 always relays UDP (manual). A
    /// server that relays none refuses the carriers (`open_udp`).
    fn udp(&self) -> UdpSupport {
        UdpSupport::Native
    }

    /// A session of its own on the policy's connection, dialed when there
    /// is none. The server's word on UDP comes with the authentication: a
    /// server that relays none refuses the carrier here (design 5.3).
    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedPacketSocket, OutboundError>> {
        Box::pin(async move {
            // one budget for the connection (when one is dialed), QUIC's
            // handshake and the authentication
            let open = Hysteria2Udp::open(self.session.clone(), self.ids.clone(), opts);
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
- `open_udp` 先取用（必要时拨号并认证，受 `opts.timeout` 约束）；服务端回 `Hysteria-UDP: false` 或不支持 QUIC 数据报时报 `hysteria2: the server does not allow UDP`。
- 载体持有连接的租约（空闲关闭因此把 UDP 会话算在内）；每次发送前经 `Session::serves` 检查，连接已关闭、正在离开或底层策略变了，就换到会话当前的连接，会话号不变。
- `send_datagram` 的任何错误都只丢这个包（记 debug 日志），从不关闭连接；锁里只做内存操作。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto-quic` → 通过（新增 29 条：经回环 UDP 回显的往返、全锥、两个方向的分片、丢一片只丢这个包、更新的包号挤掉未完成的旧包、`Hysteria-UDP: false`、收件箱满时丢包而不拖住别的载体、换连接后会话号不变、正在离开或底层策略变了时载体迁移、多个载体并发、超长的包、IDN 与 IPv6 的目标、载荷不进日志与错误文本，以及 `udp.rs` 的编解码与重组）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-proto-quic Cargo.lock
git commit -m "feat(proto-quic): Hysteria 2 的 UDP——QUIC 数据报、分片与重组、按出站计的会话号、载体跨连接迁移"
```

### Task 7: Salamander 与 Gecko

混淆层（P19）：经 `Dialer::with_layer` 套在每条连接的 UDP 路径最外面，一次一个包；Salamander（`BLAKE2b-256(口令 ‖ salt)` 循环异或）与 Gecko（长头包分 2–8 片，每片 5 字节头加填充使包长落在 512–1200，各做一次 Salamander）。`FakeHysteria2` 的混淆独立实现（`testing/obfs.rs`），经新的 `ServerPort::endpoint_over` 套在它的服务端 socket 上（重启后照旧）。

**Files:**
- Create: `crates/rurge-proto-quic/src/hysteria2/obfs/{mod.rs, salamander.rs, gecko.rs}`（自带用例）、`crates/rurge-proto-quic/src/testing/obfs.rs`
- Modify: `Cargo.toml`（工作区依赖 `blake2`）、`crates/rurge-proto-quic/{Cargo.toml, src/lib.rs, src/session.rs, src/hysteria2/mod.rs（与用例）, src/testing/mod.rs, src/testing/hysteria2.rs}`

**Interfaces:**
- Consumes: Task 1 的 `Hysteria2Obfs`；Task 2 的 `Path`；Task 4 的 `Dialer`；`blake2` 0.10.6 的 `Blake2b<U32>`。
- Produces: `Dialer::with_layer(self, layer: Layer) -> Dialer` 与 `session::Layer`（`Box<dyn Fn(Arc<dyn AsyncUdpSocket>) -> Arc<dyn AsyncUdpSocket> + Send + Sync>`）；`hysteria2::obfs`（混淆层、`Salamander`、Gecko 的分片与重组）；`Hysteria2Script.obfs`；`ServerPort::endpoint_over`

- [ ] **Step 1: 先写用例（连同假服务端的混淆）**

`Cargo.toml`——把

```toml
rurge-proto-quic = { path = "crates/rurge-proto-quic" }
```

换成

```toml
rurge-proto-quic = { path = "crates/rurge-proto-quic" }
# Hysteria 2's Salamander: BLAKE2b with a digest length of 32 (already in the
# tree through boringtun)
blake2 = "0.10"
```

`crates/rurge-proto-quic/Cargo.toml`——把

```toml
rurge-proto.workspace = true
```

换成

```toml
rurge-proto.workspace = true
blake2.workspace = true
```

新建 `crates/rurge-proto-quic/src/testing/obfs.rs`：

```rust
//! The fake's obfuscation (phase 2 M7 design 5.4), on its server socket, as
//! the reference server has it (`extras/obfs`): Salamander on every packet,
//! and with Gecko the long-header packets in fragments, either way. Written
//! apart from the client's — nothing shared but the protocol —, so that
//! each checks the other: the key through BLAKE2b's other interface, the
//! fragments put together as the reference does (at most 8 messages under
//! way per source, more refused), what it opens queued for quinn.

use blake2::Blake2bVar;
use blake2::digest::{Update, VariableOutput};
use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, ready};
use std::time::{Duration, Instant};

/// A fragment of the client's, as the fake read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedFragment {
    pub message: u8,
    pub index: u8,
    pub count: u8,
    /// Its UDP datagram's length, the salt included.
    pub datagram: usize,
}

/// A Gecko message being put together.
struct Message {
    count: u8,
    chunks: HashMap<u8, Vec<u8>>,
    until: Instant,
}

/// The fake's server socket, obfuscated: Salamander keyed with `password`,
/// Gecko over it when `gecko`.
pub(super) struct ServerObfs {
    inner: Arc<dyn AsyncUdpSocket>,
    password: Vec<u8>,
    gecko: bool,
    next_message: AtomicU8,
    /// Every Gecko fragment of the client's it read, for the tests.
    heard: Arc<Mutex<Vec<RecordedFragment>>>,
    state: Mutex<State>,
}

struct State {
    read: Vec<u8>,
    /// Packets opened, with how they came, waiting for the endpoint.
    ready: VecDeque<(Vec<u8>, RecvMeta)>,
    /// By source and message.
    messages: HashMap<(SocketAddr, u8), Message>,
}

impl ServerObfs {
    pub(super) fn new(
        inner: Arc<dyn AsyncUdpSocket>,
        password: &str,
        gecko: bool,
        heard: Arc<Mutex<Vec<RecordedFragment>>>,
    ) -> ServerObfs {
        let read = vec![0; 2048 * inner.max_receive_segments()];
        ServerObfs {
            inner,
            password: password.as_bytes().to_vec(),
            gecko,
            next_message: AtomicU8::new(0),
            heard,
            state: Mutex::new(State {
                read,
                ready: VecDeque::new(),
                messages: HashMap::new(),
            }),
        }
    }

    /// `BLAKE2b-256(password ‖ salt)`.
    fn key(&self, salt: &[u8]) -> [u8; 32] {
        let mut hash = Blake2bVar::new(32).expect("a digest length of 32");
        hash.update(&self.password);
        hash.update(salt);
        let mut key = [0; 32];
        hash.finalize_variable(&mut key).expect("32 bytes");
        key
    }

    fn seal(&self, payload: &[u8]) -> Vec<u8> {
        let mut salt = [0; 8];
        getrandom::fill(&mut salt).expect("randomness");
        let key = self.key(&salt);
        let mut datagram = salt.to_vec();
        datagram.extend(payload.iter().enumerate().map(|(i, b)| b ^ key[i % 32]));
        datagram
    }

    fn unseal(&self, datagram: &[u8]) -> Option<Vec<u8>> {
        if datagram.len() <= 8 {
            return None;
        }
        let key = self.key(&datagram[..8]);
        Some(
            datagram[8..]
                .iter()
                .enumerate()
                .map(|(i, b)| b ^ key[i % 32])
                .collect(),
        )
    }

    /// A long-header packet in 2 to 8 fragments, each padded to a datagram
    /// of 512 to 1200 bytes where it can be.
    fn fragments(&self, packet: &[u8]) -> Vec<Vec<u8>> {
        let count = 2 + random(7) as u8;
        let message = self.next_message.fetch_add(1, Ordering::Relaxed);
        let size = packet.len() / usize::from(count);
        (0..count)
            .map(|index| {
                let start = usize::from(index) * size;
                let chunk = if index == count - 1 {
                    &packet[start..]
                } else {
                    &packet[start..start + size]
                };
                let bare = 8 + 5 + chunk.len();
                let least = bare.max(512);
                let padding = if least > 1200 {
                    0
                } else {
                    least - bare + random(1200 - least + 1)
                };
                let mut frame = vec![0x80, message, index << 4 | count];
                frame.extend_from_slice(&(padding as u16).to_be_bytes());
                let mut pad = vec![0; padding];
                getrandom::fill(&mut pad).expect("randomness");
                frame.extend_from_slice(&pad);
                frame.extend_from_slice(chunk);
                self.seal(&frame)
            })
            .collect()
    }

    /// Takes one datagram the client sent: what it opens to goes to quinn,
    /// once whole.
    fn take(&self, datagram: &[u8], meta: RecvMeta, state: &mut State) {
        let Some(plain) = self.unseal(datagram) else {
            return;
        };
        if !self.gecko || plain[0] & 0x80 == 0 {
            state.ready.push_back((plain, meta));
            return;
        }
        if plain.len() < 5 {
            return;
        }
        let (message, index, count) = (plain[1], plain[2] >> 4, plain[2] & 0x0f);
        let padding = usize::from(u16::from_be_bytes([plain[3], plain[4]]));
        if !(2..=8).contains(&count) || index >= count || plain.len() < 5 + padding {
            return;
        }
        self.heard.lock().expect("heard").push(RecordedFragment {
            message,
            index,
            count,
            datagram: datagram.len(),
        });
        let now = Instant::now();
        state.messages.retain(|_, message| message.until > now);
        let key = (meta.addr, message);
        if !state.messages.contains_key(&key) {
            let under_way = state
                .messages
                .keys()
                .filter(|(source, _)| *source == meta.addr)
                .count();
            if under_way >= 8 {
                return;
            }
            state.messages.insert(
                key,
                Message {
                    count,
                    chunks: HashMap::new(),
                    until: now + Duration::from_secs(8),
                },
            );
        }
        let message = state.messages.get_mut(&key).expect("the message");
        if message.count != count {
            return;
        }
        message
            .chunks
            .entry(index)
            .or_insert_with(|| plain[5 + padding..].to_vec());
        if message.chunks.len() == usize::from(count) {
            let message = state.messages.remove(&key).expect("the message");
            let packet = (0..count)
                .flat_map(|index| message.chunks[&index].clone())
                .collect();
            state.ready.push_back((packet, meta));
        }
    }
}

/// A random number below `n`.
fn random(n: usize) -> usize {
    getrandom::u32().expect("randomness") as usize % n
}

impl fmt::Debug for ServerObfs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerObfs")
            .field("gecko", &self.gecko)
            .finish_non_exhaustive()
    }
}

impl AsyncUdpSocket for ServerObfs {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        self.inner.clone().create_io_poller()
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        let datagrams = if self.gecko && transmit.contents.first().is_some_and(|b| b & 0x80 != 0) {
            self.fragments(transmit.contents)
        } else {
            vec![self.seal(transmit.contents)]
        };
        for (n, datagram) in datagrams.iter().enumerate() {
            let sent = self.inner.try_send(&Transmit {
                contents: datagram,
                segment_size: None,
                ..transmit.clone()
            });
            // the first may wait for room; a fragment after it may not
            if n == 0 {
                sent?;
            }
        }
        Ok(())
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let mut guard = self.state.lock().expect("obfs");
        let state = &mut *guard;
        loop {
            if let Some((packet, came)) = state.ready.pop_front() {
                if packet.len() > bufs[0].len() {
                    continue;
                }
                bufs[0][..packet.len()].copy_from_slice(&packet);
                meta[0] = RecvMeta {
                    len: packet.len(),
                    stride: packet.len(),
                    ..came
                };
                return Poll::Ready(Ok(1));
            }
            let mut read = [RecvMeta::default()];
            let n = ready!(self.inner.poll_recv(
                cx,
                &mut [IoSliceMut::new(&mut state.read)],
                &mut read
            ))?;
            if n == 0 {
                continue;
            }
            let read = read[0];
            let bytes = state.read[..read.len].to_vec();
            for datagram in bytes.chunks(read.stride.max(1)) {
                self.take(datagram, read, state);
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    fn may_fragment(&self) -> bool {
        self.inner.may_fragment()
    }
}
```

`crates/rurge-proto-quic/src/testing/mod.rs`——把

```rust
mod hysteria2;
```

换成

```rust
mod hysteria2;
mod obfs;
```

`crates/rurge-proto-quic/src/testing/mod.rs`——把

```rust
    FakeHysteria2, Hysteria2Script, RecordedAuth, RecordedTcp, RecordedUdp, UdpMessage,
};

use quinn::udp::{RecvMeta, Transmit};
```

换成

```rust
    FakeHysteria2, Hysteria2Script, RecordedAuth, RecordedTcp, RecordedUdp, UdpMessage,
};
pub use obfs::RecordedFragment;

use quinn::udp::{RecvMeta, Transmit};
```

`crates/rurge-proto-quic/src/testing/mod.rs`——把

```rust
    pub fn endpoint(&self, config: ServerConfig) -> Endpoint {
```

换成

```rust
    pub fn endpoint(&self, config: ServerConfig) -> Endpoint {
        self.endpoint_over(config, |socket| socket)
    }

    /// As `endpoint`, its packets through what `layer` lays over the port
    /// (a fake's obfuscation): a layer of its own for each endpoint.
    pub fn endpoint_over(
        &self,
        config: ServerConfig,
        layer: impl FnOnce(Arc<dyn AsyncUdpSocket>) -> Arc<dyn AsyncUdpSocket>,
    ) -> Endpoint {
```

`crates/rurge-proto-quic/src/testing/mod.rs`——把

```rust
            gate,
```

换成

```rust
            layer(gate),
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
//! own. It never resolves a name.

use super::ServerPort;
```

换成

```rust
//! own. It never resolves a name. Scripted so, its socket obfuscates as
//! well, Salamander or Gecko, with code of its own too (`obfs`).

use super::ServerPort;
use super::obfs::{RecordedFragment, ServerObfs};
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
};
use rurge_net::quic::h3::testing::{H3Server, ServerStream};
```

换成

```rust
};
use rurge_config::spec::Hysteria2Obfs;
use rurge_net::quic::h3::testing::{H3Server, ServerStream};
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
    pub connect_to: Option<SocketAddr>,
```

换成

```rust
    pub connect_to: Option<SocketAddr>,
    /// Obfuscate every packet so, with this password.
    pub obfs: Option<Hysteria2Obfs>,
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
    outside: Mutex<Vec<SocketAddr>>,
```

换成

```rust
    outside: Mutex<Vec<SocketAddr>>,
    /// The client's Gecko fragments, as the endpoints' sockets read them.
    fragments: Arc<Mutex<Vec<RecordedFragment>>>,
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
        let config = fake.inner.server_config();
```

换成

```rust
        let config = fake.inner.server_config();
        let socket = fake.inner.obfuscated(socket);
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
                outside: Mutex::default(),
```

换成

```rust
                outside: Mutex::default(),
                fragments: Arc::default(),
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
        self.inner.shared.outside.lock().expect("outside").clone()
```

换成

```rust
        self.inner.shared.outside.lock().expect("outside").clone()
    }

    /// Every Gecko fragment of the client's it read, in arrival order.
    pub fn gecko_fragments(&self) -> Vec<RecordedFragment> {
        self.inner
            .shared
            .fragments
            .lock()
            .expect("fragments")
            .clone()
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
        config
    }

    /// A new endpoint takes the port over; the one before falls silent.
```

换成

```rust
        config
    }

    /// `socket` under the script's obfuscation, if any.
    fn obfuscated(&self, socket: Arc<dyn AsyncUdpSocket>) -> Arc<dyn AsyncUdpSocket> {
        let (password, gecko) = match &self.shared.script.obfs {
            None => return socket,
            Some(Hysteria2Obfs::Salamander(password)) => (password, false),
            Some(Hysteria2Obfs::Gecko(password)) => (password, true),
        };
        let heard = self.shared.fragments.clone();
        Arc::new(ServerObfs::new(socket, password.expose(), gecko, heard))
    }

    /// A new endpoint takes the port over; the one before falls silent.
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
        self.listen(port.endpoint(self.server_config()));
```

换成

```rust
        let endpoint = port.endpoint_over(self.server_config(), |socket| self.obfuscated(socket));
        self.listen(endpoint);
```

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

```rust
    use rurge_config::spec::{CommonOpts, ParamReader, PortHopping};
```

换成

```rust
    use rurge_config::spec::{CommonOpts, Hysteria2Obfs, ParamReader, PortHopping};
```

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

```rust
        assert!(!log.contains("payload-7c1e9a"));
    }
}
```

换成

```rust
        assert!(!log.contains("payload-7c1e9a"));
    }

    fn obfuscated(obfs: Hysteria2Obfs) -> Hysteria2Script {
        Hysteria2Script {
            obfs: Some(obfs),
            ..script("pw")
        }
    }

    /// Design 5.4: TCP and UDP through Salamander, and through Gecko, whose
    /// handshake goes in 2 to 8 fragments of 512 to 1200 bytes. A server
    /// that restarts says so through it as well: its stateless reset ends
    /// the tunnel under way, and the next one comes up on a new connection.
    #[tokio::test]
    async fn tcp_and_udp_go_through_either_obfuscation() {
        let (tcp_echo, udp_echo) = (echo_server().await, udp_echo_server().await);
        for (param, obfs) in [
            (
                "salamander-password=obfs",
                Hysteria2Obfs::Salamander("obfs".into()),
            ),
            ("gecko-password=obfs", Hysteria2Obfs::Gecko("obfs".into())),
        ] {
            let gecko = matches!(obfs, Hysteria2Obfs::Gecko(_));
            let fake = FakeHysteria2::spawn(obfuscated(obfs));
            let out = outbound(&fake, &format!("password=pw, {param}"));
            let mut before = tunnel(&out, &target(tcp_echo)).await;
            let payload: Vec<u8> = (0..1 << 20).map(|i| (i % 251) as u8).collect();
            round_trip(&mut before, &payload).await;
            let carrier = udp_carrier(&out).await;
            udp_round_trip(carrier.as_ref(), udp_echo, b"short").await;
            // in Hysteria's own fragments too, both ways
            udp_round_trip(carrier.as_ref(), udp_echo, &[7; 3000]).await;
            let fragments = fake.gecko_fragments();
            if gecko {
                assert!(!fragments.is_empty());
                for fragment in &fragments {
                    assert!((2..=8).contains(&fragment.count), "{fragment:?}");
                    assert!((512..=1200).contains(&fragment.datagram), "{fragment:?}");
                }
            } else {
                assert!(fragments.is_empty(), "{fragments:?}");
            }

            fake.restart();
            // room for a keep-alive (10 seconds) to bring the reset, should
            // the lost endpoint have acknowledged the request
            let opts = ConnectOpts {
                timeout: Duration::from_secs(25),
            };
            let mut after = timeout(
                Duration::from_secs(30),
                out.connect_tcp(&target(tcp_echo), &opts),
            )
            .await
            .expect("an outcome within the bound")
            .expect("a tunnel");
            round_trip(&mut after, b"after a restart").await;
            let mut rest = Vec::new();
            let err = timeout(BOUND, before.read_to_end(&mut rest))
                .await
                .expect("the end of the tunnel")
                .unwrap_err();
            assert_eq!(
                err.to_string(),
                "quic: the server no longer knows the connection (stateless reset)"
            );
            assert_eq!(fake.connections(), 2);
        }
    }

    /// A client and a server that do not obfuscate alike never bring a
    /// connection up: neither reads what the other sends. The request fails
    /// by its time, not later.
    #[tokio::test]
    async fn obfuscation_that_does_not_match_fails_by_the_time_of_the_request() {
        let echo = echo_server().await;
        let opts = ConnectOpts {
            timeout: Duration::from_millis(500),
        };
        let salamander = |password: &str| Some(Hysteria2Obfs::Salamander(password.into()));
        let gecko = |password: &str| Some(Hysteria2Obfs::Gecko(password.into()));
        let mut cases = JoinSet::new();
        for (server, client) in [
            (salamander("right"), ", salamander-password=wrong"),
            (gecko("right"), ", gecko-password=wrong"),
            (salamander("right"), ", gecko-password=right"),
            (gecko("right"), ""),
            (None, ", salamander-password=right"),
        ] {
            let opts = opts.clone();
            cases.spawn(async move {
                let fake = FakeHysteria2::spawn(Hysteria2Script {
                    obfs: server,
                    ..script("pw")
                });
                let out = outbound(&fake, &format!("password=pw{client}"));
                let started = Instant::now();
                let err = out.connect_tcp(&target(echo), &opts).await.err().unwrap();
                (client, err, started.elapsed(), fake.connections())
            });
        }
        while let Some(case) = cases.join_next().await {
            let (client, err, took, connections) = case.unwrap();
            assert!(matches!(err, OutboundError::Timeout), "{client}: {err}");
            assert!(took < Duration::from_secs(2), "{client}: {took:?}");
            assert_eq!(connections, 0, "{client}");
        }
    }

    /// Design 3.3 and 5.4: the obfuscation is the outermost layer; over
    /// port hopping, its datagrams go from each hop's socket in turn.
    #[tokio::test]
    async fn obfuscated_tunnels_keep_working_while_the_port_hops() {
        let echo = echo_server().await;
        let ports = MultiPort::bind(3);
        let fake = FakeHysteria2::spawn_on(
            obfuscated(Hysteria2Obfs::Gecko("obfs".into())),
            ports.clone(),
        );
        let mut spec = spec(9, "password=pw, gecko-password=obfs");
        spec.quic.port_hopping = Some(PortHopping {
            ports: ports.ports().iter().map(|&p| p..=p).collect(),
            interval: Duration::from_millis(100),
        });
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
        assert!(!fake.gecko_fragments().is_empty());
    }

    /// Design 5.4: over an underlying proxy's carrier as well.
    #[tokio::test]
    async fn obfuscation_goes_over_an_underlying_proxys_carrier() {
        let (tcp_echo, udp_echo) = (echo_server().await, udp_echo_server().await);
        let fake = FakeHysteria2::spawn(obfuscated(Hysteria2Obfs::Salamander("obfs".into())));
        let carried = Arc::new(Rerouted {
            inner: DirectConnector::new(Arc::new(SystemResolve)),
            route: Mutex::new(Arc::new(1)),
        });
        let port = fake.addr().port();
        let out = Hysteria2Outbound::new(
            "H",
            Target::new(HostName::parse("127.0.0.1"), port),
            &spec(port, "password=pw, salamander-password=obfs"),
            Tristate::Auto,
            &[],
            fake.roots(),
            carried,
        )
        .unwrap();
        let mut stream = tunnel(&out, &target(tcp_echo)).await;
        round_trip(&mut stream, b"carried, obfuscated").await;
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), udp_echo, b"udp too").await;
    }

    /// The obfuscation password keys the packets and goes nowhere else:
    /// not into an error, nor into a log line at any level.
    #[tokio::test]
    async fn the_obfuscation_password_never_shows() {
        const OBFS: &str = "s3cret-obfs";
        let (log, _logging, _second) = Log::capture();
        let echo = echo_server().await;
        let fake = FakeHysteria2::spawn(obfuscated(Hysteria2Obfs::Gecko(OBFS.into())));
        let out = outbound(&fake, &format!("password=pw, gecko-password={OBFS}"));
        let mut stream = tunnel(&out, &target(echo)).await;
        round_trip(&mut stream, b"obfuscated").await;
        let other = outbound(&fake, &format!("password=pw, gecko-password={OBFS}-not"));
        let opts = ConnectOpts {
            timeout: Duration::from_millis(300),
        };
        let err = other.connect_tcp(&target(echo), &opts).await.err().unwrap();
        let shown = format!("{err} {err:?}");
        assert!(!shown.contains(OBFS), "{shown}");
        let log = log.text();
        assert!(!log.is_empty());
        assert!(!log.contains(OBFS));
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto-quic --lib`
Expected: FAIL——客户端还没有混淆层，连不上混淆过的假服务端：

```text
test hysteria2::tests::obfuscation_that_does_not_match_fails_by_the_time_of_the_request ... FAILED
test hysteria2::tests::payloads_never_show_in_errors_or_logs ... ok
test hysteria2::tests::tcp_and_udp_go_through_either_obfuscation ... FAILED
test hysteria2::tests::obfuscation_goes_over_an_underlying_proxys_carrier ... FAILED
test hysteria2::tests::obfuscated_tunnels_keep_working_while_the_port_hops ... FAILED
test hysteria2::tests::the_obfuscation_password_never_shows ... FAILED
thread 'hysteria2::tests::obfuscation_that_does_not_match_fails_by_the_time_of_the_request' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:1375:77:
thread 'hysteria2::tests::obfuscation_that_does_not_match_fails_by_the_time_of_the_request' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:1380:57:
thread 'hysteria2::tests::tcp_and_udp_go_through_either_obfuscation' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:229:14:
thread 'hysteria2::tests::obfuscation_goes_over_an_underlying_proxys_carrier' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:229:14:
thread 'hysteria2::tests::obfuscated_tunnels_keep_working_while_the_port_hops' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:229:14:
thread 'hysteria2::tests::the_obfuscation_password_never_shows' panicked at crates\rurge-proto-quic\src\hysteria2\mod.rs:229:14:
test result: FAILED. 61 passed; 5 failed; 0 ignored; 0 measured; 0 filtered out; finished in 10.05s
```

- [ ] **Step 3: 实现**

新建 `crates/rurge-proto-quic/src/hysteria2/obfs/mod.rs`：

```rust
//! Hysteria 2's obfuscation (phase 2 M7 design 5.4): with
//! `salamander-password` every UDP packet of the policy's connections goes
//! through Salamander (`salamander`); with `gecko-password` as well, the
//! handshake's packets in fragments first (`gecko`). It is the outermost
//! layer of a connection's socket (`Obfuscation::over`): over port hopping,
//! and over an underlying proxy's carrier.
//!
//! - One packet at a time: quinn hands the layer no batch to send, and gets
//!   one packet per buffer.
//! - What does not open — no more than a salt, a frame that breaks a rule,
//!   more than quinn's buffer holds — is dropped, as are the fragments of a
//!   packet that never comes whole: quinn never hears of them.
//! - A send that fails other than by blocking loses its datagram, as a full
//!   socket would: an error would end the connection's driver. A Gecko
//!   packet whose first fragment went out is lost, not sent again, when a
//!   later one would block.

mod gecko;
mod salamander;

use gecko::Reassembly;
use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use rurge_config::spec::Hysteria2Obfs;
use rurge_proto::BuildError;
use salamander::{SALT, Salamander};
use std::borrow::Cow;
use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, ready};
use std::time::Instant;

/// The longest datagram read whole, as the reference reads them
/// (`udpBufferSize`): no QUIC packet comes near it. A longer one is cut,
/// and opens to nothing QUIC takes.
const READ: usize = 2048;

/// A policy's obfuscation: what each of its connections' sockets is laid
/// in. No `Debug`: it holds the password.
#[derive(Clone)]
pub(super) struct Obfuscation {
    salamander: Salamander,
    gecko: bool,
}

impl Obfuscation {
    /// Refused when the password is empty: the configuration checks it, a
    /// spec made by hand may not be.
    pub(super) fn new(obfs: &Hysteria2Obfs) -> Result<Obfuscation, BuildError> {
        let (key, password, gecko) = match obfs {
            Hysteria2Obfs::Salamander(password) => ("salamander-password", password, false),
            Hysteria2Obfs::Gecko(password) => ("gecko-password", password, true),
        };
        if password.expose().is_empty() {
            return Err(BuildError::new(format!("`{key}` is empty")));
        }
        Ok(Obfuscation {
            salamander: Salamander::new(password.expose().as_bytes()),
            gecko,
        })
    }

    /// `socket`, one connection's, obfuscated.
    pub(super) fn over(&self, socket: Arc<dyn AsyncUdpSocket>) -> Arc<dyn AsyncUdpSocket> {
        let read = READ * socket.max_receive_segments();
        Arc::new(Obfuscated {
            salamander: self.salamander.clone(),
            messages: self.gecko.then(AtomicU8::default),
            received: Mutex::new(Received {
                buf: vec![0; read].into_boxed_slice(),
                read: RecvMeta::default(),
                at: 0,
                reassembly: Reassembly::default(),
            }),
            inner: socket,
        })
    }
}

/// One connection's socket, obfuscated.
struct Obfuscated {
    inner: Arc<dyn AsyncUdpSocket>,
    salamander: Salamander,
    /// Gecko's: the message the next long-header packet goes in. `None`
    /// for Salamander alone.
    messages: Option<AtomicU8>,
    /// Taken by the endpoint's receive loop alone.
    received: Mutex<Received>,
}

/// What a read of the inner socket brought and quinn has not had yet: a
/// datagram, or several the system put together (`stride` apart), handed on
/// one by one.
struct Received {
    buf: Box<[u8]>,
    read: RecvMeta,
    /// Where the read's next datagram begins.
    at: usize,
    /// Gecko's fragments waiting for the rest of their packet.
    reassembly: Reassembly,
}

impl Obfuscated {
    /// `datagram` in place of what `transmit` carries. A failure other than
    /// blocking loses the datagram.
    fn send(&self, transmit: &Transmit<'_>, datagram: &[u8]) -> io::Result<()> {
        let sent = self.inner.try_send(&Transmit {
            contents: datagram,
            segment_size: None,
            ..transmit.clone()
        });
        match sent {
            Err(e) if e.kind() != io::ErrorKind::WouldBlock => {
                tracing::trace!(error = %e, "an obfuscated QUIC datagram was not sent");
                Ok(())
            }
            sent => sent,
        }
    }

    /// The QUIC packet `datagram` brings, opened in place — or the one it
    /// makes whole, a Gecko fragment —; `None` when it brings none.
    fn open<'a>(
        &self,
        datagram: &'a mut [u8],
        source: SocketAddr,
        reassembly: &mut Reassembly,
    ) -> Option<Cow<'a, [u8]>> {
        let packet = self.salamander.open(datagram)?;
        if self.messages.is_none() || !gecko::is_long_header(packet) {
            return Some(Cow::Borrowed(packet));
        }
        let frame = gecko::parse(packet)?;
        reassembly
            .feed(Instant::now(), source, &frame)
            .map(Cow::Owned)
    }
}

impl fmt::Debug for Obfuscated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Obfuscated")
            .field("gecko", &self.messages.is_some())
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl AsyncUdpSocket for Obfuscated {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        self.inner.clone().create_io_poller()
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        let packet = transmit.contents;
        let Some(messages) = self
            .messages
            .as_ref()
            .filter(|_| gecko::is_long_header(packet))
        else {
            let mut datagram = Vec::with_capacity(SALT + packet.len());
            datagram.resize(SALT, 0);
            datagram.extend_from_slice(packet);
            self.salamander.seal(&mut datagram);
            return self.send(transmit, &datagram);
        };
        let message = messages.fetch_add(1, Ordering::Relaxed);
        let fragments =
            gecko::fragments(&self.salamander, packet, gecko::fragment_count(), message);
        for (n, datagram) in fragments.iter().enumerate() {
            if let Err(e) = self.send(transmit, datagram) {
                // before anything went out, quinn tries again once the
                // socket can take it; after, the rest would make nothing
                // whole: the packet is lost
                if n == 0 {
                    return Err(e);
                }
                break;
            }
        }
        Ok(())
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let mut received = self.received.lock().expect("received");
        let Received {
            buf,
            read,
            at,
            reassembly,
        } = &mut *received;
        loop {
            let mut count = 0;
            while count < bufs.len() && *at < read.len {
                let end = read.len.min(*at + read.stride);
                let datagram = &mut buf[*at..end];
                *at = end;
                let Some(packet) = self.open(datagram, read.addr, reassembly) else {
                    continue;
                };
                // longer than quinn takes: no QUIC packet it would read
                let Some(room) = bufs[count].get_mut(..packet.len()) else {
                    continue;
                };
                room.copy_from_slice(&packet);
                meta[count] = RecvMeta {
                    len: packet.len(),
                    stride: packet.len(),
                    ..*read
                };
                count += 1;
            }
            if count > 0 {
                return Poll::Ready(Ok(count));
            }
            // the last read is all handed on, or dropped: the next one
            *read = RecvMeta::default();
            *at = 0;
            let mut into = [IoSliceMut::new(buf)];
            ready!(
                self.inner
                    .poll_recv(cx, &mut into, std::slice::from_mut(read))
            )?;
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    /// One datagram per transmit: each has a salt of its own.
    fn max_transmit_segments(&self) -> usize {
        1
    }

    /// One packet per buffer: the layer splits what the inner socket put
    /// together.
    fn max_receive_segments(&self) -> usize {
        1
    }

    /// The inner socket's: quinn's path MTU discovery, where it runs,
    /// measures the obfuscated datagrams.
    fn may_fragment(&self) -> bool {
        self.inner.may_fragment()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quinn::udp::EcnCodepoint;
    use std::collections::VecDeque;
    use std::task::Waker;

    const PASSWORD: &str = "s3cret-obfs";

    fn server() -> SocketAddr {
        "192.0.2.1:443".parse().unwrap()
    }

    fn obfuscation(gecko: bool) -> Obfuscation {
        let password = PASSWORD.into();
        Obfuscation::new(&if gecko {
            Hysteria2Obfs::Gecko(password)
        } else {
            Hysteria2Obfs::Salamander(password)
        })
        .unwrap()
    }

    /// The inner socket: notes what it sends — unless told to fail —, and
    /// hands out its reads, one per poll.
    #[derive(Debug, Default)]
    struct Scripted {
        sent: Mutex<Vec<Vec<u8>>>,
        /// How the next sends go: `None` goes, `Some` fails so; when none
        /// is left, they go.
        outcomes: Mutex<VecDeque<Option<io::ErrorKind>>>,
        /// Each read's bytes, and how far apart its datagrams are.
        reads: Mutex<VecDeque<(Vec<u8>, usize)>>,
        segments: usize,
    }

    impl Scripted {
        fn sent(&self) -> Vec<Vec<u8>> {
            std::mem::take(&mut *self.sent.lock().unwrap())
        }

        fn read(&self, bytes: Vec<u8>, stride: usize) {
            self.reads.lock().unwrap().push_back((bytes, stride));
        }
    }

    #[derive(Debug)]
    struct Writable;

    impl UdpPoller for Writable {
        fn poll_writable(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncUdpSocket for Scripted {
        fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
            Box::pin(Writable)
        }

        fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
            if let Some(Some(kind)) = self.outcomes.lock().unwrap().pop_front() {
                return Err(kind.into());
            }
            assert_eq!(transmit.segment_size, None);
            self.sent.lock().unwrap().push(transmit.contents.to_vec());
            Ok(())
        }

        fn poll_recv(
            &self,
            _cx: &mut Context<'_>,
            bufs: &mut [IoSliceMut<'_>],
            meta: &mut [RecvMeta],
        ) -> Poll<io::Result<usize>> {
            let Some((bytes, stride)) = self.reads.lock().unwrap().pop_front() else {
                return Poll::Pending;
            };
            bufs[0][..bytes.len()].copy_from_slice(&bytes);
            meta[0] = RecvMeta {
                addr: server(),
                len: bytes.len(),
                stride,
                ecn: Some(EcnCodepoint::Ce),
                dst_ip: None,
            };
            Poll::Ready(Ok(1))
        }

        fn local_addr(&self) -> io::Result<SocketAddr> {
            Ok("192.0.2.9:4433".parse().unwrap())
        }

        fn max_receive_segments(&self) -> usize {
            self.segments.max(1)
        }

        fn may_fragment(&self) -> bool {
            false
        }
    }

    fn transmit(contents: &[u8]) -> Transmit<'_> {
        Transmit {
            destination: server(),
            ecn: Some(EcnCodepoint::Ect0),
            contents,
            segment_size: None,
            src_ip: None,
        }
    }

    fn short_packet(len: usize) -> Vec<u8> {
        [&[0x40][..], &vec![1; len - 1]].concat()
    }

    fn long_packet(len: usize) -> Vec<u8> {
        [&[0xc0][..], &vec![2; len - 1]].concat()
    }

    /// What quinn gets from one poll offering `room` buffers of 1472 bytes,
    /// as its endpoint's are.
    fn poll(socket: &dyn AsyncUdpSocket, room: usize) -> Vec<(Vec<u8>, RecvMeta)> {
        let mut storage = vec![vec![0u8; 1472]; room];
        let mut bufs: Vec<IoSliceMut<'_>> =
            storage.iter_mut().map(|b| IoSliceMut::new(b)).collect();
        let mut meta = vec![RecvMeta::default(); room];
        let mut cx = Context::from_waker(Waker::noop());
        match socket.poll_recv(&mut cx, &mut bufs, &mut meta) {
            Poll::Pending => Vec::new(),
            Poll::Ready(received) => {
                let n = received.expect("never an error");
                drop(bufs);
                (0..n)
                    .map(|i| (storage[i][..meta[i].len].to_vec(), meta[i]))
                    .collect()
            }
        }
    }

    /// A short-header packet goes as Salamander alone. With Gecko a
    /// long-header one goes in 2 to 8 fragments of one message, the next
    /// one in the next message; with Salamander alone, whole.
    #[test]
    fn what_goes_out_is_sealed_and_the_handshake_with_gecko_in_fragments() {
        let opener = Salamander::new(PASSWORD.as_bytes());
        let open = |mut datagram: Vec<u8>| opener.open(&mut datagram).unwrap().to_vec();
        let (short, long) = (short_packet(100), long_packet(1200));
        let inner = Arc::new(Scripted::default());
        let socket = obfuscation(true).over(inner.clone());
        socket.try_send(&transmit(&short)).unwrap();
        let sent = inner.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].len(), SALT + short.len());
        assert_eq!(open(sent[0].clone()), short);
        for message in 0..2 {
            socket.try_send(&transmit(&long)).unwrap();
            let frames: Vec<Vec<u8>> = inner.sent().into_iter().map(&open).collect();
            assert!((2..=8).contains(&frames.len()), "{}", frames.len());
            let mut whole = Vec::new();
            for (index, frame) in frames.iter().enumerate() {
                let frame = gecko::parse(frame).expect("a frame");
                assert_eq!(
                    (
                        frame.message,
                        usize::from(frame.index),
                        usize::from(frame.count)
                    ),
                    (message, index, frames.len())
                );
                whole.extend_from_slice(frame.chunk);
            }
            assert_eq!(whole, long);
        }

        let socket = obfuscation(false).over(inner.clone());
        socket.try_send(&transmit(&long)).unwrap();
        let sent = inner.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(open(sent[0].clone()), long);
    }

    /// What one side's layer sends, the other's hands quinn as it was: the
    /// handshake's fragments put together again.
    #[test]
    fn what_comes_in_is_opened_and_put_together() {
        let (short, long) = (short_packet(300), long_packet(1252));
        let sender = Arc::new(Scripted::default());
        let out = obfuscation(true).over(sender.clone());
        for packet in [&short, &long, &short] {
            out.try_send(&transmit(packet)).unwrap();
        }
        let inner = Arc::new(Scripted::default());
        let socket = obfuscation(true).over(inner.clone());
        for datagram in sender.sent() {
            let len = datagram.len();
            inner.read(datagram, len);
        }
        let mut packets = Vec::new();
        loop {
            let received = poll(socket.as_ref(), 4);
            if received.is_empty() {
                break;
            }
            for (packet, meta) in received {
                assert_eq!(
                    (meta.addr, meta.len, meta.stride, meta.ecn),
                    (server(), packet.len(), packet.len(), Some(EcnCodepoint::Ce))
                );
                packets.push(packet);
            }
        }
        assert_eq!(packets, [short.clone(), long, short]);
    }

    /// What does not open — no more than a salt, a frame that breaks a
    /// rule, more than quinn's buffer holds — never reaches quinn, and is
    /// no error: the next packet does.
    #[test]
    fn what_does_not_open_never_reaches_quinn() {
        let sealer = Salamander::new(PASSWORD.as_bytes());
        let seal = |payload: &[u8]| {
            let mut datagram = [&[0; SALT][..], payload].concat();
            sealer.seal(&mut datagram);
            datagram
        };
        let inner = Arc::new(Scripted::default());
        let socket = obfuscation(true).over(inner.clone());
        let packet = short_packet(50);
        for datagram in [
            vec![9; SALT],
            // a count of 9
            seal(&[0x80, 1, 0x09, 0, 0, 7, 7]),
            // padding past the end
            seal(&[0x80, 1, 0x02, 0, 9, 7, 7]),
            seal(&short_packet(1473)),
            seal(&packet),
        ] {
            let len = datagram.len();
            inner.read(datagram, len);
        }
        let received = poll(socket.as_ref(), 4);
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].0, packet);
        assert!(poll(socket.as_ref(), 4).is_empty());
    }

    /// A read of several datagrams the system put together goes to quinn
    /// one per buffer, over as many polls as it takes.
    #[test]
    fn datagrams_read_together_go_to_quinn_one_per_buffer() {
        let sealer = Salamander::new(PASSWORD.as_bytes());
        let packets = [short_packet(92), short_packet(92), short_packet(52)];
        let mut read = Vec::new();
        for packet in &packets {
            let mut datagram = [&[0; SALT][..], packet].concat();
            sealer.seal(&mut datagram);
            read.extend_from_slice(&datagram);
        }
        let inner = Arc::new(Scripted {
            segments: 64,
            ..Scripted::default()
        });
        let socket = obfuscation(false).over(inner.clone());
        inner.read(read, 100);
        let first: Vec<Vec<u8>> = poll(socket.as_ref(), 2).into_iter().map(|r| r.0).collect();
        assert_eq!(first, packets[..2]);
        let rest: Vec<Vec<u8>> = poll(socket.as_ref(), 2).into_iter().map(|r| r.0).collect();
        assert_eq!(rest, packets[2..]);
        assert!(poll(socket.as_ref(), 2).is_empty());
    }

    /// A failed send loses its datagram and is no error for quinn; one
    /// that would block is quinn's to wait for — unless a fragment of the
    /// packet went out already, and the packet is lost.
    #[test]
    fn a_failed_send_loses_its_datagram_and_one_that_would_block_waits() {
        let next_sends = |inner: &Scripted, outcomes: &[Option<io::ErrorKind>]| {
            inner
                .outcomes
                .lock()
                .unwrap()
                .extend(outcomes.iter().copied());
        };
        let short = short_packet(100);
        let inner = Arc::new(Scripted::default());
        let socket = obfuscation(false).over(inner.clone());
        next_sends(
            &inner,
            &[Some(io::ErrorKind::Other), Some(io::ErrorKind::WouldBlock)],
        );
        socket.try_send(&transmit(&short)).unwrap();
        let blocked = socket.try_send(&transmit(&short)).unwrap_err();
        assert_eq!(blocked.kind(), io::ErrorKind::WouldBlock);
        socket.try_send(&transmit(&short)).unwrap();
        assert_eq!(inner.sent().len(), 1);

        let long = long_packet(1200);
        let inner = Arc::new(Scripted::default());
        let socket = obfuscation(true).over(inner.clone());
        next_sends(&inner, &[Some(io::ErrorKind::WouldBlock)]);
        let blocked = socket.try_send(&transmit(&long)).unwrap_err();
        assert_eq!(blocked.kind(), io::ErrorKind::WouldBlock);
        assert!(inner.sent().is_empty());
        next_sends(&inner, &[None, Some(io::ErrorKind::WouldBlock)]);
        socket.try_send(&transmit(&long)).unwrap();
        assert_eq!(inner.sent().len(), 1, "the first fragment, and no more");
    }

    /// One packet a time either way, the inner socket's address and
    /// fragmentation; and no password in what it shows of itself.
    #[test]
    fn the_layer_takes_one_packet_at_a_time_and_shows_no_password() {
        let inner = Arc::new(Scripted {
            segments: 64,
            ..Scripted::default()
        });
        for gecko in [false, true] {
            let socket = obfuscation(gecko).over(inner.clone());
            assert_eq!(socket.max_transmit_segments(), 1);
            assert_eq!(socket.max_receive_segments(), 1);
            assert!(!socket.may_fragment());
            assert_eq!(socket.local_addr().unwrap(), inner.local_addr().unwrap());
            let shown = format!("{socket:?}");
            assert!(shown.starts_with("Obfuscated"), "{shown}");
            assert!(!shown.contains(PASSWORD), "{shown}");
        }
        for (obfs, key) in [
            (Hysteria2Obfs::Salamander("".into()), "salamander-password"),
            (Hysteria2Obfs::Gecko("".into()), "gecko-password"),
        ] {
            let err = Obfuscation::new(&obfs).map(|_| ()).unwrap_err();
            assert_eq!(err.message, format!("`{key}` is empty"));
        }
    }
}
```

新建 `crates/rurge-proto-quic/src/hysteria2/obfs/salamander.rs`：

```rust
//! Salamander (Hysteria 2 protocol, "Salamander"; phase 2 M7 design 5.4):
//! every packet goes as `salt ‖ payload ^ key`, the salt 8 random bytes of
//! its own, the key `BLAKE2b-256(password ‖ salt)` repeated over the
//! payload. BLAKE2b-256 is BLAKE2b with a digest length of 32 — Go's
//! `blake2b.Sum256`, which the reference keys with
//! (`extras/obfs/salamander.go`) —, not BLAKE2b-512 cut short. Nothing in a
//! packet says it was changed on the way: only one too short to hold a salt
//! and a byte does not open; anything else opens, to garbage if it must,
//! which QUIC's own protection then drops.

use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};

/// The salt in front of every packet.
pub(super) const SALT: usize = 8;
/// What the key repeats over.
const KEY: usize = 32;

type Blake2b256 = Blake2b<U32>;

/// Salamander keyed with a password. No `Debug`: it holds the password.
#[derive(Clone)]
pub(super) struct Salamander {
    /// BLAKE2b-256 that has taken the password: each packet's key is a copy
    /// of it finished with the packet's salt.
    keyed: Blake2b256,
}

impl Salamander {
    pub(super) fn new(password: &[u8]) -> Salamander {
        Salamander {
            keyed: Blake2b256::new_with_prefix(password),
        }
    }

    /// The key of the packets salted with `salt`.
    fn key(&self, salt: &[u8]) -> [u8; KEY] {
        self.keyed.clone().chain_update(salt).finalize().into()
    }

    /// `datagram` — room for the salt, then the payload — sealed in place:
    /// a new random salt, the payload masked with its key.
    pub(super) fn seal(&self, datagram: &mut [u8]) {
        let mut salt = [0; SALT];
        // obfuscation, not a secret: without a random source, a salt of
        // zeros
        let _ = getrandom::fill(&mut salt);
        self.seal_with(salt, datagram);
    }

    fn seal_with(&self, salt: [u8; SALT], datagram: &mut [u8]) {
        datagram[..SALT].copy_from_slice(&salt);
        let key = self.key(&salt);
        mask(&mut datagram[SALT..], &key);
    }

    /// The payload of a sealed `datagram`, unmasked in place; `None` for
    /// one that holds no more than a salt (as the reference reads it).
    pub(super) fn open<'a>(&self, datagram: &'a mut [u8]) -> Option<&'a mut [u8]> {
        if datagram.len() <= SALT {
            return None;
        }
        let (salt, payload) = datagram.split_at_mut(SALT);
        let key = self.key(salt);
        mask(payload, &key);
        Some(payload)
    }
}

/// XORs `key`, over and over, onto `bytes`.
fn mask(bytes: &mut [u8], key: &[u8; KEY]) {
    for chunk in bytes.chunks_mut(KEY) {
        for (byte, k) in chunk.iter_mut().zip(key) {
            *byte ^= k;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blake2::digest::consts::{U20, U48, U64};
    use blake2::digest::generic_array::ArrayLength;
    use blake2::digest::typenum::{IsLessOrEqual, LeEq, NonZero};
    use blake2::digest::{KeyInit, Mac, Update, VariableOutput};
    use blake2::{Blake2b512, Blake2bMac, Blake2bVar};

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
            .collect()
    }

    /// BLAKE2b-256 of `abc`, as CPython's `hashlib.blake2b(b"abc",
    /// digest_size=32)` gives it, is no prefix of BLAKE2b-512 of `abc` (RFC
    /// 7693 Appendix A): the digest length is a parameter of the hash, as
    /// in Go's `blake2b.Sum256`, not a cut. The key is the hash of the
    /// password and the salt one after the other.
    #[test]
    fn blake2b_256_is_blake2b_with_a_digest_length_of_32() {
        let abc = hex("bddd813c634239723171ef3fee98579b94964e3bb1cb3e427262c8c068d52319");
        assert_eq!(Blake2b256::digest(b"abc")[..], abc);
        assert_eq!(
            Blake2b512::digest(b"abc")[..],
            hex(concat!(
                "ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d1",
                "7d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923"
            ))
        );
        assert_eq!(Salamander::new(b"ab").key(b"c")[..], abc);
        assert_eq!(Salamander::new(b"").key(b"abc")[..], abc);
    }

    /// RFC 7693 Appendix E: BLAKE2b of the digest lengths 20, 32, 48 and
    /// 64, keyed and not, over inputs of 0 to 1024 bytes, all hashed
    /// together by the BLAKE2b-256 Salamander keys with, gives the RFC's
    /// result.
    #[test]
    fn blake2b_256_passes_the_self_test_of_rfc_7693() {
        /// `selftest_seq`: the top byte of each term of a Fibonacci sequence.
        fn sequence(len: usize, seed: u32) -> Vec<u8> {
            let (mut a, mut b) = (0xDEAD_4BAD_u32.wrapping_mul(seed), 1u32);
            (0..len)
                .map(|_| {
                    let t = a.wrapping_add(b);
                    (a, b) = (b, t);
                    (t >> 24) as u8
                })
                .collect()
        }
        fn keyed<N>(key: &[u8], data: &[u8]) -> Vec<u8>
        where
            N: ArrayLength<u8> + IsLessOrEqual<U64>,
            LeEq<N, U64>: NonZero,
        {
            let mut mac = <Blake2bMac<N> as KeyInit>::new_from_slice(key).expect("a key");
            Mac::update(&mut mac, data);
            mac.finalize().into_bytes().to_vec()
        }
        let mut grand = Blake2b256::new();
        for len in [20, 32, 48, 64] {
            for input in [0, 3, 128, 129, 255, 1024] {
                let data = sequence(input, input as u32);
                let mut plain = Blake2bVar::new(len).expect("a digest length");
                Update::update(&mut plain, &data);
                let mut digest = vec![0; len];
                plain.finalize_variable(&mut digest).expect("the length");
                Digest::update(&mut grand, &digest);
                let key = sequence(len, len as u32);
                let digest = match len {
                    20 => keyed::<U20>(&key, &data),
                    32 => keyed::<U32>(&key, &data),
                    48 => keyed::<U48>(&key, &data),
                    _ => keyed::<U64>(&key, &data),
                };
                Digest::update(&mut grand, &digest);
            }
        }
        assert_eq!(
            grand.finalize()[..],
            hex("c23a7800d98123bd10f506c61e29da5603d763b8bbad2e737f5e765a7bccd475")
        );
    }

    /// A packet sealed with a known salt, byte for byte: computed apart, by
    /// CPython's `hashlib` (`salt + bytes(b ^ key[i % 32] …)`, `key =
    /// blake2b(password + salt, digest_size=32)`). The 70-byte payload goes
    /// past the key twice.
    #[test]
    fn a_packet_sealed_with_a_known_salt() {
        let salamander = Salamander::new(b"rurge salamander");
        let salt = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
        let payload: Vec<u8> = (0..70).collect();
        assert_eq!(
            salamander.key(&salt)[..],
            hex("7b21007e054064d04bbf06c42e4c538375be27ad59ab58f2a974dc1544f58c32")
        );
        let mut datagram = [&[0; SALT][..], &payload].concat();
        salamander.seal_with(salt, &mut datagram);
        assert_eq!(
            datagram,
            hex(concat!(
                "0123456789abcdef7b20027d014562d743b60ccf22415d8c65af35be4dbe4ee5",
                "b16dc60e58e8922d5b00225d216542f763962cef02617dac458f159e6d9e6ec5",
                "914de62e78c8b20d3b60423d4105"
            ))
        );
        assert_eq!(salamander.open(&mut datagram).unwrap(), &payload[..]);
    }

    /// Each packet has a salt of its own, so the same payload never goes
    /// out the same twice; each opens back to its payload.
    #[test]
    fn every_packet_has_a_salt_of_its_own() {
        let salamander = Salamander::new(b"pw");
        let payload = b"the same QUIC packet, sent twice";
        let sealed: Vec<Vec<u8>> = (0..2)
            .map(|_| {
                let mut datagram = [&[0; SALT][..], payload].concat();
                salamander.seal(&mut datagram);
                datagram
            })
            .collect();
        assert_ne!(sealed[0][..SALT], sealed[1][..SALT]);
        assert_ne!(sealed[0][SALT..], sealed[1][SALT..]);
        for mut datagram in sealed {
            assert_eq!(salamander.open(&mut datagram).unwrap(), payload);
        }
    }

    /// A datagram of the salt alone, or shorter, holds nothing: dropped.
    #[test]
    fn what_holds_no_more_than_a_salt_does_not_open() {
        let salamander = Salamander::new(b"pw");
        for len in 0..=SALT {
            assert!(salamander.open(&mut vec![7; len]).is_none(), "{len}");
        }
        assert_eq!(
            salamander.open(&mut [7; SALT + 1]).map(|p| p.len()),
            Some(1)
        );
    }

    /// Nothing tells a changed packet, or another password's: either opens,
    /// to something else than was sealed.
    #[test]
    fn a_changed_packet_or_another_password_opens_to_something_else() {
        let salamander = Salamander::new(b"pw");
        let payload = b"a QUIC packet";
        let mut sealed = [&[0; SALT][..], payload].concat();
        salamander.seal(&mut sealed);
        let mut changed = sealed.clone();
        changed[0] ^= 1;
        assert_ne!(salamander.open(&mut changed).unwrap(), payload);
        let mut other = sealed.clone();
        assert_ne!(Salamander::new(b"pW").open(&mut other).unwrap(), payload);
        assert_eq!(salamander.open(&mut sealed).unwrap(), payload);
    }
}
```

新建 `crates/rurge-proto-quic/src/hysteria2/obfs/gecko.rs`：

```rust
//! Gecko (Hysteria 2 protocol, "Gecko"; phase 2 M7 design 5.4): Salamander
//! with the handshake reshaped (`extras/obfs/gecko.go`,
//! `extras/obfs/gecko_frame.go` in the reference).
//!
//! - A packet with a short header — its first byte's top bit clear — goes
//!   as Salamander alone.
//! - One with a long header — the handshake's, and only they — goes in 2 to
//!   8 fragments, each a frame sealed apart: `0x80 ‖ message ‖ index << 4 |
//!   count ‖ u16 padding ‖ padding ‖ chunk`. The padding is random, so that
//!   the fragment's datagram comes to 512 to 1200 bytes (the reference's
//!   defaults: Surge has no parameters for them).
//! - Whatever opens to a first byte with the top bit set is such a frame: a
//!   long-header packet never goes whole. A frame that breaks a rule is
//!   dropped; the others wait, by source and message, for the rest of their
//!   packet, which then goes on whole.

use super::salamander::{SALT, Salamander};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// The top bit of the first byte: a QUIC packet's long header, or a frame.
const TOP: u8 = 0x80;
/// `0x80 ‖ message ‖ index << 4 | count ‖ u16 padding`.
const HEADER: usize = 5;
const MIN_FRAGMENTS: u8 = 2;
const MAX_FRAGMENTS: u8 = 8;
/// What a fragment's datagram is padded to, its salt included: at least,
/// and at most.
const MIN_SIZE: usize = 512;
const MAX_SIZE: usize = 1200;
/// Packets put together at a time: one more drops the oldest.
const PENDING: usize = 8;
/// How long the fragments of a packet wait for the rest.
const TTL: Duration = Duration::from_secs(8);

/// Whether `packet` has a long header: one of QUIC's handshake.
pub(super) fn is_long_header(packet: &[u8]) -> bool {
    packet.first().is_some_and(|first| first & TOP != 0)
}

/// How many fragments a long-header packet goes in: 2 to 8, at random.
pub(super) fn fragment_count() -> u8 {
    MIN_FRAGMENTS + below(usize::from(MAX_FRAGMENTS - MIN_FRAGMENTS) + 1) as u8
}

/// A random number below `n` (`n > 0`); 0 without a random source.
fn below(n: usize) -> usize {
    getrandom::u32().map_or(0, |random| random as usize % n)
}

/// The datagrams of `packet`, a long-header one, in `count` fragments of
/// message `message`, each sealed apart. The parts are `len / count` bytes
/// long, the last taking the rest, as the reference splits them.
pub(super) fn fragments(
    salamander: &Salamander,
    packet: &[u8],
    count: u8,
    message: u8,
) -> Vec<Vec<u8>> {
    let size = packet.len() / usize::from(count);
    (0..count)
        .map(|index| {
            let start = usize::from(index) * size;
            let end = if index + 1 == count {
                packet.len()
            } else {
                start + size
            };
            let chunk = &packet[start..end];
            let padding = padding(chunk.len());
            let mut datagram = Vec::with_capacity(SALT + HEADER + padding + chunk.len());
            datagram.resize(SALT, 0);
            datagram.extend_from_slice(&[TOP, message, index << 4 | count]);
            datagram.extend_from_slice(&(padding as u16).to_be_bytes());
            let at = datagram.len();
            datagram.resize(at + padding, 0);
            // seen through the mask, a padding of zeros would show the key
            let _ = getrandom::fill(&mut datagram[at..]);
            datagram.extend_from_slice(chunk);
            salamander.seal(&mut datagram);
            datagram
        })
        .collect()
}

/// The padding of a chunk of `len` bytes: its datagram comes to a random
/// size of 512 to 1200 bytes, never less than salt, header and chunk; none
/// when they alone pass 1200.
fn padding(len: usize) -> usize {
    let bare = SALT + HEADER + len;
    let least = bare.max(MIN_SIZE);
    if least > MAX_SIZE {
        return 0;
    }
    least - bare + below(MAX_SIZE - least + 1)
}

/// A frame as it came.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Frame<'a> {
    pub(super) message: u8,
    pub(super) index: u8,
    pub(super) count: u8,
    pub(super) chunk: &'a [u8],
}

/// The frame `bytes` hold — a datagram opened to a first byte with the top
/// bit set —, read as the reference reads one: 2 to 8 fragments, the index
/// below the count, the padding within the frame; `None` for one that
/// breaks a rule. The other bits of the first byte are not looked at.
pub(super) fn parse(bytes: &[u8]) -> Option<Frame<'_>> {
    let header: &[u8; HEADER] = bytes.first_chunk()?;
    let (index, count) = (header[2] >> 4, header[2] & 0x0f);
    if header[0] & TOP == 0 || !(MIN_FRAGMENTS..=MAX_FRAGMENTS).contains(&count) || index >= count {
        return None;
    }
    let padding = usize::from(u16::from_be_bytes([header[3], header[4]]));
    Some(Frame {
        message: header[1],
        index,
        count,
        chunk: bytes.get(HEADER + padding..)?,
    })
}

/// A packet being put together.
struct Pending {
    source: SocketAddr,
    message: u8,
    /// The chunks so far, by index: as many places as fragments.
    chunks: Vec<Option<Vec<u8>>>,
    missing: usize,
    since: Instant,
}

/// The fragments that wait for the rest of their packet (design 5.4): by
/// source and message, at most 8 packets at a time — one more drops the
/// oldest, the likeliest to have lost a fragment —, each for at most 8
/// seconds. A fragment seen already, or one that gives its message another
/// count than the first did, is dropped.
#[derive(Default)]
pub(super) struct Reassembly {
    /// In the order they began: the oldest first.
    pending: Vec<Pending>,
}

impl Reassembly {
    /// The packet `frame`, come from `source` at `now`, makes whole; `None`
    /// while one of its fragments is missing.
    pub(super) fn feed(
        &mut self,
        now: Instant,
        source: SocketAddr,
        frame: &Frame<'_>,
    ) -> Option<Vec<u8>> {
        self.pending
            .retain(|pending| now.duration_since(pending.since) < TTL);
        let count = usize::from(frame.count);
        let at = match self
            .pending
            .iter()
            .position(|pending| pending.source == source && pending.message == frame.message)
        {
            Some(at) => at,
            None => {
                if self.pending.len() == PENDING {
                    self.pending.remove(0);
                }
                self.pending.push(Pending {
                    source,
                    message: frame.message,
                    chunks: vec![None; count],
                    missing: count,
                    since: now,
                });
                self.pending.len() - 1
            }
        };
        let pending = &mut self.pending[at];
        if pending.chunks.len() != count {
            return None;
        }
        let chunk = &mut pending.chunks[usize::from(frame.index)];
        if chunk.is_some() {
            return None;
        }
        *chunk = Some(frame.chunk.to_vec());
        pending.missing -= 1;
        if pending.missing > 0 {
            return None;
        }
        let chunks: Vec<Vec<u8>> = self
            .pending
            .remove(at)
            .chunks
            .into_iter()
            .flatten()
            .collect();
        Some(chunks.concat())
    }

    /// Test support: how many packets are being put together.
    #[cfg(test)]
    pub(super) fn pending(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn salamander() -> Salamander {
        Salamander::new(b"gecko")
    }

    fn source() -> SocketAddr {
        "192.0.2.1:443".parse().unwrap()
    }

    /// A long-header packet of `len` bytes.
    fn long_packet(len: usize) -> Vec<u8> {
        let mut packet: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        packet[0] = 0xc3;
        packet
    }

    /// What each datagram opens to.
    fn opened(datagrams: &[Vec<u8>]) -> Vec<Vec<u8>> {
        datagrams
            .iter()
            .map(|datagram| {
                let mut datagram = datagram.clone();
                salamander().open(&mut datagram).expect("a frame").to_vec()
            })
            .collect()
    }

    /// Byte for byte, each fragment is `0x80`, the message, its index and
    /// the count, the padding's length, the padding, then its part of the
    /// packet: `len / count` bytes, the last fragment the rest.
    #[test]
    fn a_fragment_is_its_header_its_padding_and_a_part_of_the_packet() {
        let packet = long_packet(1250);
        for count in MIN_FRAGMENTS..=MAX_FRAGMENTS {
            let datagrams = fragments(&salamander(), &packet, count, 0x5a);
            assert_eq!(datagrams.len(), usize::from(count));
            let size = packet.len() / usize::from(count);
            let mut whole = Vec::new();
            for (index, frame) in opened(&datagrams).iter().enumerate() {
                assert_eq!(frame[..3], [0x80, 0x5a, (index as u8) << 4 | count]);
                let padding = usize::from(u16::from_be_bytes([frame[3], frame[4]]));
                let chunk = &frame[5 + padding..];
                let expected = if index + 1 == usize::from(count) {
                    packet.len() - index * size
                } else {
                    size
                };
                assert_eq!(chunk.len(), expected, "{count} fragments, #{index}");
                whole.extend_from_slice(chunk);
            }
            assert_eq!(whole, packet);
        }
    }

    /// Padded to 512 to 1200 bytes, at random; a chunk that leaves no room
    /// goes without padding.
    #[test]
    fn a_fragments_datagram_has_512_to_1200_bytes_unless_its_chunk_is_longer() {
        let mut sizes = BTreeSet::new();
        for _ in 0..50 {
            for datagram in fragments(&salamander(), &long_packet(100), 8, 0) {
                assert!(
                    (MIN_SIZE..=MAX_SIZE).contains(&datagram.len()),
                    "{}",
                    datagram.len()
                );
                sizes.insert(datagram.len());
            }
        }
        assert!(sizes.len() > 10, "{sizes:?}");
        // salt, header and chunk: exactly 1200, then past it
        for (len, size) in [(2 * 1187, 1200), (2 * 1188, 1201), (3000, 1513)] {
            let datagrams = fragments(&salamander(), &long_packet(len), 2, 0);
            assert!(datagrams.iter().all(|d| d.len() == size), "{len}");
            for frame in opened(&datagrams) {
                assert_eq!(frame[3..5], [0, 0]);
            }
        }
    }

    #[test]
    fn a_long_header_packet_goes_in_2_to_8_fragments_at_random() {
        let counts: BTreeSet<u8> = (0..500).map(|_| fragment_count()).collect();
        assert_eq!(counts, (2..=8).collect());
        assert!(is_long_header(&[0xc0]) && is_long_header(&[0x80]));
        assert!(!is_long_header(&[0x40]) && !is_long_header(&[]));
    }

    /// The reference's rules (`decodeFrame`): a count of 2 to 8, the index
    /// below it, the padding within the frame. The other bits of the first
    /// byte are not looked at.
    #[test]
    fn frames_that_break_a_rule_are_dropped() {
        let frame = |head: [u8; 3], padding: u16, rest: &[u8]| {
            [&head[..], &padding.to_be_bytes(), rest].concat()
        };
        assert_eq!(
            parse(&frame([0x80, 7, 0x12], 2, b"..chunk")),
            Some(Frame {
                message: 7,
                index: 1,
                count: 2,
                chunk: b"chunk"
            })
        );
        assert_eq!(
            parse(&frame([0xff, 7, 0x78], 0, b"x")).map(|f| f.index),
            Some(7)
        );
        // the padding to the end: an empty chunk
        assert_eq!(
            parse(&frame([0x80, 7, 0x02], 3, b"...")).map(|f| f.chunk.len()),
            Some(0)
        );
        for bad in [
            frame([0x7f, 7, 0x02], 0, b"x"),
            frame([0x80, 7, 0x00], 0, b"x"),
            frame([0x80, 7, 0x01], 0, b"x"),
            frame([0x80, 7, 0x09], 0, b"x"),
            frame([0x80, 7, 0x22], 0, b"x"),
            frame([0x80, 7, 0x82], 0, b"x"),
            frame([0x80, 7, 0x02], 4, b"..."),
            vec![0x80, 7, 0x02, 0],
            Vec::new(),
        ] {
            assert_eq!(parse(&bad), None, "{bad:?}");
        }
    }

    /// Feeds each datagram of `datagrams` from `source` at `now`, and says
    /// what came whole.
    fn feed(
        reassembly: &mut Reassembly,
        now: Instant,
        source: SocketAddr,
        datagrams: &[Vec<u8>],
    ) -> Vec<Vec<u8>> {
        opened(datagrams)
            .iter()
            .filter_map(|frame| reassembly.feed(now, source, &parse(frame).expect("a frame")))
            .collect()
    }

    #[test]
    fn fragments_make_their_packet_whole_in_any_order() {
        let (one, two) = (long_packet(1200), long_packet(300));
        let first = fragments(&salamander(), &one, 5, 1);
        let second = fragments(&salamander(), &two, 3, 2);
        // backwards, the two packets' fragments taking turns
        let arrivals: Vec<Vec<u8>> = [
            &first[4], &second[2], &first[3], &second[0], &first[2], &second[1], &first[1],
            &first[0],
        ]
        .into_iter()
        .cloned()
        .collect();
        let mut reassembly = Reassembly::default();
        let now = Instant::now();
        assert_eq!(feed(&mut reassembly, now, source(), &arrivals), [two, one]);
        assert_eq!(reassembly.pending(), 0);
    }

    /// The first copy of a fragment counts; a fragment that gives its
    /// message another count is dropped. Once the packet went on, a copy
    /// come late begins another, which never comes whole.
    #[test]
    fn copies_and_fragments_of_another_count_are_dropped() {
        let packet = long_packet(1000);
        let datagrams = fragments(&salamander(), &packet, 2, 9);
        let frames = opened(&datagrams);
        let mut reassembly = Reassembly::default();
        let now = Instant::now();
        let first = parse(&frames[0]).unwrap();
        assert_eq!(reassembly.feed(now, source(), &first), None);
        let mut changed = frames[0].clone();
        *changed.last_mut().unwrap() ^= 0xff;
        assert_eq!(
            reassembly.feed(now, source(), &parse(&changed).unwrap()),
            None
        );
        let mut other_count = frames[1].clone();
        other_count[2] = 0x13;
        assert_eq!(
            reassembly.feed(now, source(), &parse(&other_count).unwrap()),
            None
        );
        let second = parse(&frames[1]).unwrap();
        assert_eq!(reassembly.feed(now, source(), &second), Some(packet));
        assert_eq!(reassembly.feed(now, source(), &second), None);
        assert_eq!(reassembly.pending(), 1);
    }

    /// A packet that misses a fragment waits 8 seconds, then is dropped:
    /// its fragment come later begins another packet.
    #[test]
    fn a_packet_missing_a_fragment_is_dropped_after_8_seconds() {
        let packet = long_packet(800);
        let datagrams = fragments(&salamander(), &packet, 2, 3);
        let start = Instant::now();
        let mut reassembly = Reassembly::default();
        feed(&mut reassembly, start, source(), &datagrams[..1]);
        let late = start + TTL - Duration::from_millis(1);
        assert_eq!(
            feed(&mut reassembly, late, source(), &datagrams[1..]),
            [packet]
        );

        feed(&mut reassembly, start, source(), &datagrams[..1]);
        let too_late = start + TTL;
        assert!(feed(&mut reassembly, too_late, source(), &datagrams[1..]).is_empty());
        assert_eq!(reassembly.pending(), 1, "the late fragment's own");
    }

    /// Eight packets at a time: a ninth drops the oldest, whose last
    /// fragment then makes nothing whole.
    #[test]
    fn a_ninth_packet_under_way_drops_the_oldest() {
        let start = Instant::now();
        let packets: Vec<Vec<u8>> = (0..9).map(|n| long_packet(600 + n)).collect();
        let datagrams: Vec<Vec<Vec<u8>>> = packets
            .iter()
            .enumerate()
            .map(|(message, packet)| fragments(&salamander(), packet, 2, message as u8))
            .collect();
        let mut reassembly = Reassembly::default();
        for (n, datagrams) in datagrams.iter().enumerate() {
            let now = start + Duration::from_millis(n as u64);
            feed(&mut reassembly, now, source(), &datagrams[..1]);
            assert_eq!(reassembly.pending(), (n + 1).min(PENDING));
        }
        let later = start + Duration::from_millis(10);
        for n in [8, 1] {
            assert_eq!(
                feed(&mut reassembly, later, source(), &datagrams[n][1..]),
                [packets[n].clone()]
            );
        }
        assert!(feed(&mut reassembly, later, source(), &datagrams[0][1..]).is_empty());
    }

    /// Two sources' fragments of the same message never mix.
    #[test]
    fn the_fragments_of_two_sources_are_kept_apart() {
        let other: SocketAddr = "192.0.2.2:443".parse().unwrap();
        let (one, two) = (long_packet(700), long_packet(900));
        let (a, b) = (
            fragments(&salamander(), &one, 2, 4),
            fragments(&salamander(), &two, 2, 4),
        );
        let mut reassembly = Reassembly::default();
        let now = Instant::now();
        feed(&mut reassembly, now, source(), &a[..1]);
        feed(&mut reassembly, now, other, &b[..1]);
        assert_eq!(feed(&mut reassembly, now, other, &b[1..]), [two]);
        assert_eq!(feed(&mut reassembly, now, source(), &a[1..]), [one]);
    }
}
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
use quinn::{Connection, ConnectionError, VarInt};
```

换成

```rust
use quinn::{AsyncUdpSocket, Connection, ConnectionError, VarInt};
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
/// How a session's connections are made (design 3.2, 3.3): the UDP path to
/// the server, then QUIC's handshake with the policy's TLS. Built once per
/// outbound.
```

换成

```rust
/// What a protocol lays over each connection's socket, outermost: over the
/// hops and the carrier too (Hysteria 2's obfuscation, design 5.4).
pub type Layer = Box<dyn Fn(Arc<dyn AsyncUdpSocket>) -> Arc<dyn AsyncUdpSocket> + Send + Sync>;

/// How a session's connections are made (design 3.2, 3.3): the UDP path to
/// the server — under what the protocol lays over it (`with_layer`) —, then
/// QUIC's handshake with the policy's TLS. Built once per outbound.
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
    server_name: String,
```

换成

```rust
    server_name: String,
    layer: Option<Layer>,
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
            server_name: tls.server_name,
        })
```

换成

```rust
            server_name: tls.server_name,
            layer: None,
        })
    }

    /// Lays `layer` over every connection's socket.
    pub fn with_layer(self, layer: Layer) -> Dialer {
        Dialer {
            layer: Some(layer),
            ..self
        }
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
        let path = Path::open(
```

换成

```rust
        let mut path = Path::open(
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
        .await?;
        quic::connect(path, &self.config, &self.server_name).await
```

换成

```rust
        .await?;
        if let Some(layer) = &self.layer {
            path.socket = layer(path.socket);
        }
        quic::connect(path, &self.config, &self.server_name).await
```

`crates/rurge-proto-quic/src/lib.rs`——把

```rust
pub use session::{Dialer, Failure, Lease, Protocol, Session};
```

换成

```rust
pub use session::{Dialer, Failure, Layer, Lease, Protocol, Session};
```

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

```rust
//!   pads them: letters and digits, of a random length.

mod auth;
```

换成

```rust
//!   pads them: letters and digits, of a random length.
//! - With `salamander-password` or `gecko-password`, every UDP packet of the
//!   connection is obfuscated (`obfs`).

mod auth;
mod obfs;
```

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

```rust
use auth::Auth;
```

换成

```rust
use auth::Auth;
use obfs::Obfuscation;
```

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

```rust
    /// policy's `ecn`; `client-cert` comes from `keystore`.
```

换成

```rust
    /// policy's `ecn` and obfuscation; `client-cert` comes from `keystore`.
```

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

```rust
        let dialer = Dialer::new(
            connector, server, &spec.tls, &spec.quic, ecn, keystore, roots,
        )?;
```

换成

```rust
        let mut dialer = Dialer::new(
            connector, server, &spec.tls, &spec.quic, ecn, keystore, roots,
        )?;
        if let Some(obfs) = &spec.obfs {
            let obfuscation = Obfuscation::new(obfs)?;
            dialer = dialer.with_layer(Box::new(move |socket| obfuscation.over(socket)));
        }
```

要点：
- BLAKE2b-256 是输出长度参数为 32 的 BLAKE2b（与 Go 的 `blake2b.Sum256` 相同），不是截断的 BLAKE2b-512；已知答案取自 RFC 7693 附录 E 的自测与 "abc" 的公开值。
- 每个包的 salt 取自 `getrandom`；口令只以带密钥的 BLAKE2b 状态保存（`Salamander` 没有 `Debug`）。
- 层的 `max_transmit_segments` / `max_receive_segments` 为 1；读内层时按内层可能合并的段数开缓冲再逐包拆开。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto-quic` → 通过（新增 26 条：BLAKE2b-256 与固定 salt 的 Salamander 已知答案、往返与篡改、8 字节及以下的包丢弃；Gecko 的片数范围、包长、头部布局、乱序与重复、缺片 8 秒过期、第 9 组挤掉最旧的；两种混淆下 TCP 与 UDP 的端到端、混淆口令不符时请求按时限失败、混淆与端口跳跃同用、混淆经 `underlying-proxy` 的载体、口令不进日志与 `Debug`）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add Cargo.toml Cargo.lock crates/rurge-proto-quic
git commit -m "feat(proto-quic): Hysteria 2 的 Salamander 与 Gecko 混淆，套在 QUIC 路径的最外层"
```

### Task 8: 引擎装配、能力表与经引擎的用例

去掉 Task 1 的门，引擎工厂装上 `Hysteria2Outbound`（同其它协议的分支：`server_of`、keystore、根证书、`ecn`、直连或 `underlying-proxy` 的连接器），能力表加入 `hysteria2`；`port-hopping` 遇上非直连的连接器在 `Dialer::new` 里拒绝，组级派生的 `M (via R)` 因此只略去这一条（P20）；测试里"未实现的协议"的例子换成 `tailscale`；bin 关掉 quinn 的日志；一个不作门禁的吞吐基准（P21）。

**Files:**
- Create: `crates/rurge-engine/tests/outbounds_hysteria2.rs`
- Modify: `crates/rurge-config/src/spec/mod.rs`（与用例）、`tests/policy_spec.rs`、`crates/rurge-engine/{Cargo.toml, src/outbounds.rs（与用例）, src/observe.rs, src/smart.rs, tests/common/mod.rs, tests/pipeline.rs}`、`crates/rurge-policy/src/{assemble.rs, registry.rs}`（用例）、`crates/rurge-proto-quic/src/{session.rs（与用例）, hysteria2/mod.rs, testing/hysteria2.rs}`、`crates/rurge-api/tests/api.rs`、`crates/rurge/{src/capabilities.rs, src/cli/run.rs（与用例）, tests/cli.rs}`

**Interfaces:**
- Consumes: Task 1 ～ 7 的全部；272f624 的 `Connector::route()` 与引擎 UDP 的失败处理。
- Produces: 工厂的 `hysteria2` 分支；能力表的 `hysteria2`；`tests/common` 的 `harness_edited`（`Runtime::build` 之前改已载入的配置，只用于测试）与再导出的 `FakeHysteria2` / `Hysteria2Script`；`cargo test -p rurge-proto-quic --release throughput -- --ignored --nocapture`

- [ ] **Step 1: 先写用例**

经引擎的用例与它们要的依赖：

`crates/rurge-engine/Cargo.toml`——把

```toml
rurge-proto.workspace = true
```

换成

```toml
rurge-proto.workspace = true
rurge-proto-quic.workspace = true
```

`crates/rurge-engine/Cargo.toml`——把

```toml
rurge-proto = { workspace = true, features = ["testing"] }
```

换成

```toml
rurge-proto = { workspace = true, features = ["testing"] }
rurge-proto-quic = { workspace = true, features = ["testing"] }
```

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
};
pub use rurge_proto_ssh::testing::{
```

换成

```rust
};
pub use rurge_proto_quic::testing::{FakeHysteria2, Hysteria2Script};
pub use rurge_proto_ssh::testing::{
```

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
    std::fs::write(dir.join("t.conf"), profile).unwrap();
    let loaded = from_text(profile, &dir.join("t.conf"), &LoadOptions::for_tests());
```

换成

```rust
    runtime_edited(dir, profile, shared, |_| {}).await
}

/// As `runtime`, with `edit` applied to the loaded configuration first: for
/// what the text of a profile cannot say (a hop interval under the loader's
/// minimum).
pub async fn runtime_edited(
    dir: &std::path::Path,
    profile: &str,
    shared: EngineShared,
    edit: impl FnOnce(&mut rurge_config::Config),
) -> Runtime {
    std::fs::write(dir.join("t.conf"), profile).unwrap();
    let mut loaded = from_text(profile, &dir.join("t.conf"), &LoadOptions::for_tests());
```

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
            .collect::<Vec<_>>()
    );
    Runtime::build(
```

换成

```rust
            .collect::<Vec<_>>()
    );
    edit(&mut loaded.config);
    Runtime::build(
```

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
pub async fn harness_with(p: Profile<'_>, shared: EngineShared) -> Harness {
```

换成

```rust
pub async fn harness_with(p: Profile<'_>, shared: EngineShared) -> Harness {
    harness_edited(p, shared, |_| {}).await
}

/// As `harness_with`, with `edit` applied to the loaded configuration
/// first (`runtime_edited`).
pub async fn harness_edited(
    p: Profile<'_>,
    shared: EngineShared,
    edit: impl FnOnce(&mut rurge_config::Config),
) -> Harness {
```

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
    let engine = Engine::new(runtime(dir.path(), &text, shared).await);
```

换成

```rust
    let engine = Engine::new(runtime_edited(dir.path(), &text, shared, edit).await);
```

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
    pub async fn recv(&self) -> (SocketAddr, Vec<u8>) {
        let mut buf = [0u8; 2048];
```

换成

```rust
    pub async fn recv(&self) -> (SocketAddr, Vec<u8>) {
        let mut buf = vec![0u8; 65536];
```

新建 `crates/rurge-engine/tests/outbounds_hysteria2.rs`：

```rust
//! Sessions that leave through `hysteria2` (phase 2 M7 design 5): TCP and
//! UDP through the engine to the loopback `FakeHysteria2` — the policy's
//! one QUIC connection, a refused password, `Hysteria-UDP: false`, both
//! obfuscations, port hopping, `underlying-proxy` (a SOCKS5 proxy's UDP, a
//! group whose member changes), the relay of a group, policy tests and
//! reloads.

mod common;
use common::*;
use rurge_config::HostName;
use rurge_config::session::Transport;
use rurge_config::spec::{Hysteria2Obfs, ProtoSpec};
use rurge_engine::RequestRecord;
use rurge_net::connector::Target;
use rurge_net::quic::testing::MultiPort;
use rurge_proto::testing::{echo_server, udp_echo_server};

/// The password the fake lets in.
const PASSWORD: &str = "s3same";

fn script() -> Hysteria2Script {
    Hysteria2Script {
        password: PASSWORD.into(),
        ..Hysteria2Script::default()
    }
}

/// `H = hysteria2, 127.0.0.1, <the fake's port>, <params>`.
fn line(fake: &FakeHysteria2, params: &str) -> String {
    format!("H = hysteria2, 127.0.0.1, {}, {params}", fake.addr().port())
}

/// `line` with the password and nothing else.
fn plain_line(fake: &FakeHysteria2) -> String {
    line(fake, &format!("password={PASSWORD}"))
}

/// A profile that sends the name `target.test` and the loopback through
/// `H`.
fn profile(proxies: &str) -> Profile<'_> {
    Profile {
        proxies,
        rules: "DOMAIN,target.test,H\nIP-CIDR,127.0.0.1/32,H,no-resolve",
        ..Profile::default()
    }
}

/// What makes an engine trust `fake`'s certificate.
fn trusting(fake: &FakeHysteria2) -> EngineShared {
    EngineShared {
        roots: Some(fake.roots()),
        ..EngineShared::default()
    }
}

/// An engine on `proxies` (`profile`) that trusts `fake`.
async fn through(fake: &FakeHysteria2, proxies: &str) -> Harness {
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
}

/// A CONNECT through the HTTP listener and one through the SOCKS5 listener
/// each leave as a stream of the policy's one QUIC connection, after one
/// authentication; the server is asked for the name.
#[tokio::test]
async fn tcp_leaves_through_hysteria2_on_one_connection() {
    let echo = echo_server().await;
    let fake = FakeHysteria2::spawn(Hysteria2Script {
        connect_to: Some(echo),
        ..script()
    });
    let h = through(&fake, &plain_line(&fake)).await;
    let mut http = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut http, b"through hysteria2").await;
    echo_through(&mut http, &vec![0x5a; 300_000]).await;
    let mut socks = connect_via_socks5(h.socks(), "target.test", 7).await;
    echo_through(&mut socks, b"and from SOCKS5").await;
    let requests: Vec<(usize, String)> = fake
        .tcp_requests()
        .into_iter()
        .map(|r| (r.connection, r.address))
        .collect();
    assert_eq!(requests, vec![(0, "target.test:7".to_string()); 2]);
    assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
    assert_eq!((fake.connections(), fake.auths().len()), (1, 1));
    drop((http, socks));
    for record in the_records(&h, 2).await {
        assert_eq!(record.policy, ["H"], "{record:?}");
        assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
    }
}

/// A password the server refuses fails the session with the text that
/// says so — the CONNECT is answered 502 — and the password is nowhere in
/// the record.
#[tokio::test]
async fn a_refused_password_fails_the_session() {
    let fake = FakeHysteria2::spawn(script());
    let h = through(&fake, &line(&fake, "password=wr0ngPw")).await;
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
    assert_eq!(
        record.error.as_deref(),
        Some("hysteria2: authentication failed")
    );
    assert!(!format!("{record:?}").contains("wr0ngPw"));
    assert_eq!(fake.auths()[0].field("hysteria-auth"), Some("wr0ngPw"));
    assert!(fake.tcp_requests().is_empty());
}

/// Datagrams through one association to two echoes go in one session of
/// the policy's connection (session 1); whoever writes to the server's
/// socket for that session reaches the client (full cone); a datagram
/// longer than a QUIC datagram goes in fragments, both ways.
#[tokio::test]
async fn udp_leaves_through_hysteria2_in_datagrams() {
    let fake = FakeHysteria2::spawn(script());
    let h = through(&fake, &plain_line(&fake)).await;
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

    let messages = fake.udp_messages();
    assert!(
        messages.iter().all(|m| (m.connection, m.session) == (0, 1)),
        "{messages:?}"
    );
    let whole: Vec<String> = messages
        .iter()
        .filter(|m| m.fragments == 1)
        .map(|m| m.address.clone())
        .collect();
    assert_eq!(whole, [one, two, one].map(|echo| echo.to_string()));
    let fragments: Vec<_> = messages.iter().filter(|m| m.fragments > 1).collect();
    assert!(fragments.len() >= 3, "{fragments:?}");
    assert!(
        fragments.iter().all(|m| m.packet != 0
            && m.packet == fragments[0].packet
            && usize::from(m.fragments) == fragments.len()
            && m.address == two.to_string()),
        "{fragments:?}"
    );
    assert_eq!(fragments.iter().map(|m| m.len).sum::<usize>(), long.len());
    assert_eq!(fake.connections(), 1);
    drop(association);
    wait_until("both flows to finish", || udp_records(&h).len() == 2).await;
    for r in udp_records(&h) {
        assert_eq!(r.policy, ["H"], "{r:?}");
        assert_eq!(r.status, RecordStatus::Completed, "{r:?}");
    }
}

/// A server that relays no UDP (`Hysteria-UDP: false`) fails the flow with
/// the text that says so; nothing goes out as a datagram, and TCP goes on
/// over the same connection.
#[tokio::test]
async fn a_server_that_relays_no_udp_fails_the_flow() {
    let (echo, udp_echo) = (echo_server().await, udp_echo_server().await);
    let fake = FakeHysteria2::spawn(Hysteria2Script {
        no_udp: true,
        ..script()
    });
    let h = through(&fake, &plain_line(&fake)).await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", udp_echo.port(), b"q").await;
    wait_until("the flow to finish", || !udp_records(&h).is_empty()).await;
    let records = udp_records(&h);
    assert_eq!(records[0].status, RecordStatus::Failed, "{:?}", records[0]);
    assert_eq!(
        records[0].error.as_deref(),
        Some("hysteria2: the server does not allow UDP")
    );
    assert!(fake.udp_messages().is_empty());
    let mut tunnel = connect_via_http(h.http(), &echo.to_string()).await;
    echo_through(&mut tunnel, b"TCP still").await;
    assert_eq!((fake.connections(), fake.auths().len()), (1, 1));
}

/// With `salamander-password` or `gecko-password`, TCP and UDP go through a
/// server that obfuscates the same way; with Gecko, the handshake's packets
/// came in fragments.
#[tokio::test]
async fn either_obfuscation_carries_tcp_and_udp() {
    let (echo, udp_echo) = (echo_server().await, udp_echo_server().await);
    for (obfs, param) in [
        (
            Hysteria2Obfs::Salamander("0bfsPw".into()),
            "salamander-password=0bfsPw",
        ),
        (
            Hysteria2Obfs::Gecko("0bfsPw".into()),
            "gecko-password=0bfsPw",
        ),
    ] {
        let gecko = matches!(obfs, Hysteria2Obfs::Gecko(_));
        let fake = FakeHysteria2::spawn(Hysteria2Script {
            obfs: Some(obfs),
            ..script()
        });
        let h = through(
            &fake,
            &line(&fake, &format!("password={PASSWORD}, {param}")),
        )
        .await;
        let mut tunnel = connect_via_http(h.http(), &echo.to_string()).await;
        echo_through(&mut tunnel, b"obfuscated").await;
        let association = udp_associate(h.socks()).await;
        association
            .send("127.0.0.1", udp_echo.port(), b"obfuscated too")
            .await;
        assert_eq!(
            association.recv().await,
            (udp_echo, b"obfuscated too".to_vec())
        );
        assert_eq!(fake.connections(), 1, "{param}");
        assert_eq!(!fake.gecko_fragments().is_empty(), gecko, "{param}");
    }
}

/// Every `hysteria2` policy of `cfg` hops every `interval`: the loader
/// takes no `port-hopping-interval` under 5 seconds.
fn hop_every(cfg: &mut rurge_config::Config, interval: Duration) {
    for spec in &mut cfg.specs {
        if let ProtoSpec::Hysteria2(hysteria2) = &mut spec.proto
            && let Some(hopping) = &mut hysteria2.quic.port_hopping
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
    let fake = FakeHysteria2::spawn_on(script(), ports.clone());
    let set: Vec<String> = ports.ports().iter().map(u16::to_string).collect();
    let proxies = line(
        &fake,
        &format!("password={PASSWORD}, port-hopping={}", set.join(";")),
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

/// `hysteria2` over a SOCKS5 `underlying-proxy` (design 3.3): the QUIC
/// connection goes in datagrams of the entry's UDP association, to the
/// server as the line names it, and carries TCP and UDP alike.
#[tokio::test]
async fn hysteria2_goes_over_an_underlying_socks5_proxy() {
    let (echo, udp_echo) = (echo_server().await, udp_echo_server().await);
    let fake = FakeHysteria2::spawn(script());
    let entry = FakeSocks5::spawn(Socks5Script::default()).await;
    let proxies = format!(
        "Entry = socks5, 127.0.0.1, {}, udp-relay=true\n{}",
        entry.addr().port(),
        line(
            &fake,
            &format!("password={PASSWORD}, underlying-proxy=Entry")
        )
    );
    let h = through(&fake, &proxies).await;
    let mut tunnel = connect_via_http(h.http(), &echo.to_string()).await;
    echo_through(&mut tunnel, b"socks5 then hysteria2").await;
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

/// The `underlying-proxy` is a group: once another member is selected, the
/// next session goes on a new QUIC connection through that one, and the
/// tunnel under way goes on, on the connection it has (design 3.5).
#[tokio::test]
async fn a_new_member_of_the_underlying_group_gets_a_new_connection() {
    let echo = echo_server().await;
    let fake = FakeHysteria2::spawn(Hysteria2Script {
        connect_to: Some(echo),
        ..script()
    });
    let (one, two) = (
        FakeSocks5::spawn(Socks5Script::default()).await,
        FakeSocks5::spawn(Socks5Script::default()).await,
    );
    let proxies = format!(
        "{}\nOne = socks5, 127.0.0.1, {}, udp-relay=true\nTwo = socks5, 127.0.0.1, {}, udp-relay=true",
        line(&fake, &format!("password={PASSWORD}, underlying-proxy=Up")),
        one.addr().port(),
        two.addr().port()
    );
    let h = harness_with(
        Profile {
            proxies: &proxies,
            groups: "Up = select, One, Two",
            rules: "DOMAIN,target.test,H",
            ..Profile::default()
        },
        trusting(&fake),
    )
    .await;
    let mut first = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut first, b"through one").await;
    assert!(two.datagrams().is_empty());
    h.engine.select_group("Up", "Two").await.unwrap();
    let mut second = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut second, b"through two").await;
    assert!(!two.datagrams().is_empty());
    echo_through(&mut first, b"still through one").await;
    let places: Vec<usize> = fake.tcp_requests().iter().map(|r| r.connection).collect();
    assert_eq!(places, [0, 1]);
    assert_eq!(fake.connections(), 2);
}

/// A group's `underlying-proxy` chains its members (`M (via R)`, phase 2
/// M3 design 5.4), but port hopping cannot go through a chain: the member
/// that hops is left out, and the group goes on through the others.
#[tokio::test]
async fn a_hopping_member_of_a_relayed_group_is_left_out() {
    let echo = echo_server().await;
    let fake = FakeHysteria2::spawn(Hysteria2Script {
        connect_to: Some(echo),
        ..script()
    });
    let entry = FakeSocks5::spawn(Socks5Script::default()).await;
    let port = fake.addr().port();
    let proxies = format!(
        "Entry = socks5, 127.0.0.1, {}, udp-relay=true\n\
         Hop = hysteria2, 127.0.0.1, {port}, password={PASSWORD}, port-hopping={port}\n\
         Plain = hysteria2, 127.0.0.1, {port}, password={PASSWORD}",
        entry.addr().port()
    );
    let h = harness_with(
        Profile {
            proxies: &proxies,
            groups: "G = select, Hop, Plain, underlying-proxy=Entry",
            rules: "DOMAIN,target.test,G",
            ..Profile::default()
        },
        trusting(&fake),
    )
    .await;
    let registry = h.engine.registry();
    assert!(registry.contains("Hop") && !registry.contains("Hop (via Entry)"));
    assert_eq!(
        registry.members("G"),
        Some(&["Plain (via Entry)".to_string()][..])
    );
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"over the group's relay").await;
    assert!(!entry.datagrams().is_empty());
    drop(tunnel);
    let record = the_records(&h, 1).await[0].clone();
    assert_eq!(record.policy, ["G", "Plain (via Entry)"]);
}

/// A policy test fetches the policy's `test-url` through its connection:
/// one tunnel to the loopback origin, two HEADs on it.
#[tokio::test]
async fn a_policy_test_goes_through_the_connection() {
    let origin = TestServer::spawn().await;
    let fake = FakeHysteria2::spawn(script());
    let h = through(
        &fake,
        &line(
            &fake,
            &format!("password={PASSWORD}, test-url={}", origin.url("/")),
        ),
    )
    .await;
    let results = h
        .engine
        .test_policies(&["H".to_string()], None)
        .await
        .unwrap();
    let result = results[0].1.as_ref().expect("a hysteria2 policy is tested");
    assert!(result.outcome.is_ok(), "{:?}", result.outcome);
    let port = origin.url("/").port().unwrap();
    let tunnels: Vec<String> = fake.tcp_requests().into_iter().map(|r| r.address).collect();
    assert_eq!(tunnels, [format!("127.0.0.1:{port}")]);
    let methods: Vec<String> = origin.requests().into_iter().map(|r| r.method).collect();
    assert_eq!(methods, ["HEAD", "HEAD"]);
}

/// The outbound — and with it its QUIC connection — is kept across a
/// reload that leaves the line alone, and rebuilt when its password,
/// `download-bandwidth`, obfuscation, `ecn` or port hopping changes
/// (design 3.5, 8).
#[tokio::test]
async fn a_reload_keeps_an_unchanged_policy_and_its_connection() {
    let echo = echo_server().await;
    let fake = FakeHysteria2::spawn(Hysteria2Script {
        connect_to: Some(echo),
        ..script()
    });
    let proxies = |params: &str, extra: &str| format!("{}\n{extra}", line(&fake, params));
    let base = format!("password={PASSWORD}");
    let h = through(&fake, &proxies(&base, "")).await;
    let reload = |params: &str, extra: &str| {
        let next = profile(&proxies(params, extra)).text(h.dns.addr());
        let dir = h.dir.path().to_path_buf();
        let shared = h.engine.shared();
        async move { runtime(&dir, &next, shared).await }
    };
    one_session(&h, 1, b"before the reload").await;
    let before = outbound_now(&h, "H");
    h.engine
        .swap_runtime(reload(&base, "Other = http, other.example, 8080").await);
    assert!(
        Arc::ptr_eq(&before, &outbound_now(&h, "H")),
        "an unrelated reload rebuilt H"
    );
    one_session(&h, 2, b"after the reload").await;
    let places: Vec<usize> = fake.tcp_requests().iter().map(|r| r.connection).collect();
    assert_eq!(places, [0, 0], "the QUIC connection was kept");
    assert_eq!(fake.connections(), 1);

    let mut previous = before;
    for changed in [
        "password=0ther",
        "password=0ther, download-bandwidth=50",
        "password=0ther, download-bandwidth=50, salamander-password=0bfs",
        "password=0ther, download-bandwidth=50, gecko-password=0bfs",
        "password=0ther, download-bandwidth=50, gecko-password=0bfs, ecn=off",
        "password=0ther, download-bandwidth=50, gecko-password=0bfs, ecn=off, port-hopping=5000-6000",
    ] {
        h.engine.swap_runtime(reload(changed, "").await);
        let now = outbound_now(&h, "H");
        assert!(!Arc::ptr_eq(&previous, &now), "kept after: {changed}");
        previous = now;
    }
}
```

`hysteria2` 有了 spec 之后改变的用例，与改用 `tailscale` 的"未实现协议"例子：

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    /// A `hysteria2` line is read and checked in full; it has no spec until
    /// the engine builds `hysteria2` (M7a task 8). `ecn` takes effect on it,
    /// `tfo` and `udp-relay` do not apply (phase 2 M7 design 3.1, 5.1).
    #[test]
    fn a_hysteria2_line_is_checked_but_has_no_spec_yet() {
        for def in [
            "hysteria2, 192.168.20.6, 443, password=pwd, download-bandwidth=100",
            "hysteria2, h.test, 443, password=pw, salamander-password=obfs, port-hopping=1234;5000-6000, port-hopping-interval=10, ecn=off, sni=edge.test, client-cert=cert1",
            "hysteria2, h.test, 443, password=pw, gecko-password=obfs, alpn=h3, underlying-proxy=Pick",
            "hysteria2, h.test, 443, password=pw, port-hopping=5000-6000, underlying-proxy=DIRECT",
```

换成

```rust
    /// A `hysteria2` line is read and checked in full, and a sound one has a
    /// spec. `ecn` takes effect on it, `tfo` and `udp-relay` do not apply
    /// (phase 2 M7 design 3.1, 5.1).
    #[test]
    fn a_hysteria2_line_is_checked_and_has_a_spec() {
        for (def, chained) in [
            (
                "hysteria2, 192.168.20.6, 443, password=pwd, download-bandwidth=100",
                false,
            ),
            (
                "hysteria2, h.test, 443, password=pw, salamander-password=obfs, port-hopping=1234;5000-6000, port-hopping-interval=10, ecn=off, sni=edge.test, client-cert=cert1",
                false,
            ),
            (
                "hysteria2, h.test, 443, password=pw, gecko-password=obfs, alpn=h3, underlying-proxy=Pick",
                true,
            ),
            // DIRECT is no chain: the hops stay
            (
                "hysteria2, h.test, 443, password=pw, port-hopping=5000-6000, underlying-proxy=DIRECT",
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
            assert!(matches!(spec.proto, ProtoSpec::Hysteria2(_)), "{def}");
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
use rurge_config::spec::{IpVersion, ProtoSpec};
use rurge_config::{Severity, codes};
use std::path::Path;
```

换成

```rust
use rurge_config::spec::{IpVersion, ProtoSpec, Tristate};
use rurge_config::{Severity, codes};
use std::path::Path;
use std::time::Duration;
```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
        "A = http, a.example, 80, ip-version=v6-only\nHy = hysteria2, h.example, 443, password=x\nB = socks5, b.example, 1080\nC = direct, interface=eth0",
```

换成

```rust
        "A = http, a.example, 80, ip-version=v6-only\nTs = tailscale, section-name=ts\nB = socks5, b.example, 1080\nC = direct, interface=eth0",
```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
    assert_eq!(names, ["A", "B", "C"], "hysteria2 has no spec yet");
```

换成

```rust
    assert_eq!(names, ["A", "B", "C"], "tailscale has no spec");
```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
    assert!(loaded.config.spec("Hy").is_none() && loaded.config.spec("nope").is_none());
```

换成

```rust
    assert!(loaded.config.spec("Ts").is_none() && loaded.config.spec("nope").is_none());
```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
/// 5.1): the combinations they cannot have are errors, and `ecn` has an
/// effect on them. They have no spec until the engine builds them (M7a
/// task 8).
```

换成

```rust
/// 5.1): the combinations they cannot have are errors, which leave no
/// spec; a hop interval under 5 seconds is raised to 5, and `ecn` has an
/// effect on them.
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
    let ProtoSpec::Hysteria2(hysteria2) = &d.proto else {
        panic!("{:?}", d.proto)
    };
    assert_eq!(
        hysteria2.quic.port_hopping.as_ref().map(|h| h.interval),
        Some(Duration::from_secs(5))
    );
    assert_eq!(d.common.ecn, Tristate::Off);
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
TT = trust-tunnel, proxy.test, 443, username=u, password=pw, shadow-tls-password=st\n\
```

换成

```rust
TT = trust-tunnel, proxy.test, 443, username=u, password=pw, shadow-tls-password=st\n\
HY = hysteria2, proxy.test, 443, password=pw, download-bandwidth=100, salamander-password=obfs, port-hopping=5000-6000, ecn=off\n\
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            ("TT", "TT"),
```

换成

```rust
            ("TT", "TT"),
            ("HY", "HY"),
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            assert!(!m.contains("hunter2") && !m.contains("s3same0pen"), "{m}");
        }
    }

    #[test]
    fn a_dry_build_of_an_anytls_policy_leaves_no_task_behind() {
```

换成

```rust
            assert!(!m.contains("hunter2") && !m.contains("s3same0pen"), "{m}");
        }
    }

    /// Sound `hysteria2` lines pass the dry build, which opens no connection
    /// (no tokio runtime here); a client certificate that cannot be read is
    /// a load error, never quoted (phase 2 M7 design 3.2).
    #[test]
    fn a_hysteria2_policy_passes_the_dry_build() {
        let cfg = config(
            "[Proxy]\nHY = hysteria2, proxy.test, 443, password=pw, download-bandwidth=100, gecko-password=obfs, port-hopping=1234;5000-6000, port-hopping-interval=10, ecn=off, sni=hy.test\n\
[Rule]\nFINAL,DIRECT\n",
        );
        assert!(dry_build(&cfg).is_empty());

        let cfg = config(
            "[Proxy]\nHY = hysteria2, proxy.test, 443, password=s3same0pen, client-cert=cert1\n\
[Keystore]\ncert1 = type=p12, base64=QUJD, password=hunter2\n[Rule]\nFINAL,DIRECT\n",
        );
        let messages: Vec<String> = dry_build(&cfg).iter().map(|d| d.message.clone()).collect();
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(
            messages[0].starts_with("policy `HY` cannot be built: keystore item `cert1`"),
            "{}",
            messages[0]
        );
        assert!(!messages[0].contains("hunter2") && !messages[0].contains("s3same0pen"));
    }

    #[test]
    fn a_dry_build_of_an_anytls_policy_leaves_no_task_behind() {
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
TP = trust-tunnel, proxy.test, 443, username=u, password=pw, skip-cert-verify=true, server-cert-fingerprint-sha256=0000000000000000000000000000000000000000000000000000000000000000\n\
```

换成

```rust
TP = trust-tunnel, proxy.test, 443, username=u, password=pw, skip-cert-verify=true, server-cert-fingerprint-sha256=0000000000000000000000000000000000000000000000000000000000000000\n\
HY = hysteria2, proxy.test, 443, password=pw, skip-cert-verify=true\n\
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
        assert_eq!(flagged, ["A", "V", "H2", "TT"]);
```

换成

```rust
        assert_eq!(flagged, ["A", "V", "H2", "TT", "HY"]);
```

`crates/rurge-engine/src/observe.rs`——把

```rust
            "!unsupported:hysteria2".into(),
```

换成

```rust
            "!unsupported:tailscale".into(),
```

`crates/rurge-engine/src/smart.rs`——把

```rust
        assert!(!retryable(&OutboundError::Unsupported("hysteria2".into())));
```

换成

```rust
        assert!(!retryable(&OutboundError::Unsupported("tailscale".into())));
```

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
/// does not declare Hysteria 2 (`crates/rurge/src/capabilities.rs`), so
```

换成

```rust
/// does not declare Tailscale (`crates/rurge/src/capabilities.rs`), so
```

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
        .remove(&rurge_config::PolicyKind::Hysteria2);
```

换成

```rust
        .remove(&rurge_config::PolicyKind::Tailscale);
```

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
[Proxy]\nHK = hysteria2, 1.2.3.4, 443, password=x\nBlock = reject-tinygif\n\
```

换成

```rust
[Proxy]\nHK = tailscale, section-name=ts\nBlock = reject-tinygif\n\
Locked = trojan, 1.2.3.4, 443, password=x\n\
```

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
        "unsupported hysteria2 policy warned at load: {:?}",
```

换成

```rust
        "unsupported tailscale policy warned at load: {:?}",
```

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
    assert!(String::from_utf8_lossy(&body).contains("!unsupported:hysteria2"));
```

换成

```rust
    assert!(String::from_utf8_lossy(&body).contains("!unsupported:tailscale"));
```

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
                Some("policy protocol not implemented: hysteria2")
```

换成

```rust
                Some("policy protocol not implemented: tailscale")
```

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
[Proxy]\nHK = hysteria2, 1.2.3.4, 443, password=x\n[Proxy Group]\nPick = select, HK, DIRECT\n\
```

换成

```rust
[Proxy]\nHK = tailscale, section-name=ts\n[Proxy Group]\nPick = select, HK, DIRECT\n\
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
            "G = select, policy-path=https://sub.test/g",
        );
        let a = assemble(
```

换成

```rust
            "G = select, policy-path=https://sub.test/g",
        );
        // `Ts` has no `section-name`: a subscription line's own is refused
        let a = assemble(
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
Hy = hysteria2, h.test, 443, password=pw\n\
```

换成

```rust
Ts = tailscale\n\
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
        assert_eq!(members(&a, "G"), ["Hy", "Rc", "Sn", "Old", "Good"]);
```

换成

```rust
        assert_eq!(members(&a, "G"), ["Ts", "Rc", "Sn", "Old", "Good"]);
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
                    "policy group `G`: imported policies of type `hysteria2` are not implemented in this version; they behave as REJECT".to_string()
```

换成

```rust
                    "policy group `G`: imported policies of type `tailscale` are not implemented in this version; they behave as REJECT".to_string()
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
Corp = direct, interface=eth9\nBlock = reject\nHy = hysteria2, h.test, 443, password=pw",
            "Inner = select, A\n\
G = select, A, Corp, Block, DIRECT, Inner, Hy, policy-path=https://sub.test/g, underlying-proxy=Relay",
```

换成

```rust
Corp = direct, interface=eth9\nBlock = reject\nTs = tailscale, section-name=ts",
            "Inner = select, A\n\
G = select, A, Corp, Block, DIRECT, Inner, Ts, policy-path=https://sub.test/g, underlying-proxy=Relay",
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
                "Hy",
```

换成

```rust
                "Ts",
```

`crates/rurge-policy/src/registry.rs`——把

```rust
HK = hysteria2, 1.2.3.4, 443, password=x\n\
```

换成

```rust
HK = tailscale, section-name=ts\n\
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        assert_eq!(chain(&hk), vec!["HK", "!unsupported:hysteria2", "REJECT"]);
```

换成

```rust
        assert_eq!(chain(&hk), vec!["HK", "!unsupported:tailscale", "REJECT"]);
```

`crates/rurge-policy/src/registry.rs`——把

```rust
                Some(Note::Unsupported("hysteria2".into()))
```

换成

```rust
                Some(Note::Unsupported("tailscale".into()))
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            vec!["Pick", "HK", "!unsupported:hysteria2", "REJECT"]
```

换成

```rust
            vec!["Pick", "HK", "!unsupported:tailscale", "REJECT"]
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            vec!["Auto", "HK", "!unsupported:hysteria2", "REJECT"]
```

换成

```rust
            vec!["Auto", "HK", "!unsupported:tailscale", "REJECT"]
```

`crates/rurge-policy/src/registry.rs`——把

```rust
[Proxy]\nU = hysteria2, 1.2.3.4, 443, password=x\nA = http, a.example, 80\nB = http, b.example, 80\n\
```

换成

```rust
[Proxy]\nU = tailscale, section-name=ts\nA = http, a.example, 80\nB = http, b.example, 80\n\
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            ["OnlyU", "U", "!unsupported:hysteria2", "REJECT"]
```

换成

```rust
            ["OnlyU", "U", "!unsupported:tailscale", "REJECT"]
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            profile += &format!("{name} = hysteria2, {name}.example, 443, password=x\n");
```

换成

```rust
            profile += &format!("{name} = tailscale, section-name={name}\n");
```

`crates/rurge-api/tests/api.rs`——把

```rust
/// The binary does not declare Hysteria 2 (yet), so dropping it from the
```

换成

```rust
/// The binary does not declare Tailscale (yet), so dropping it from the
```

`crates/rurge-api/tests/api.rs`——把

```rust
        .remove(&rurge_config::PolicyKind::Hysteria2);
```

换成

```rust
        .remove(&rurge_config::PolicyKind::Tailscale);
```

`crates/rurge-api/tests/api.rs`——把

```rust
[Proxy]\nHK = hysteria2, 1.2.3.4, 443, password=x\nBlock = reject-tinygif\n\
```

换成

```rust
[Proxy]\nHK = tailscale, section-name=ts\nBlock = reject-tinygif\n\
Locked = trojan, 1.2.3.4, 443, password=x\n\
```

`crates/rurge-api/tests/api.rs`——把

```rust
        "W0007 for the unsupported hysteria2 policy: {body}"
```

换成

```rust
        "W0007 for the unsupported tailscale policy: {body}"
```

`crates/rurge-api/tests/api.rs`——把

```rust
    assert_eq!(members[0]["typeDescription"], "hysteria2");
```

换成

```rust
    assert_eq!(members[0]["typeDescription"], "tailscale");
```

`crates/rurge-api/tests/api.rs`——把

```rust
    assert_eq!(status, 200);
    let detail = body["HK"].as_str().expect("the definition");
    assert!(detail.starts_with("hysteria2, 1.2.3.4, 443"), "{detail}");
```

换成

```rust
    assert_eq!(
        (status, body),
        (200, json!({ "HK": "tailscale, section-name=ts" }))
    );
    let (status, body) = get(&api, "/v1/policies/detail?policy_name=Locked").await;
    assert_eq!(status, 200);
    let detail = body["Locked"].as_str().expect("the definition");
    assert!(detail.starts_with("trojan, 1.2.3.4, 443"), "{detail}");
```

`crates/rurge/tests/cli.rs`——把

```rust
const PROXIES: &str = "[General]\n[Proxy]\nH = http, proxy.test, 8080\nS = socks5-tls, proxy.test, 443\nOld = hysteria2, 1.2.3.4, 443, password=x\n[Rule]\nFINAL,DIRECT\n";
```

换成

```rust
const PROXIES: &str = "[General]\n[Proxy]\nH = http, proxy.test, 8080\nS = socks5-tls, proxy.test, 443\nOld = tailscale, section-name=ts\n[Rule]\nFINAL,DIRECT\n";
```

`crates/rurge/tests/cli.rs`——把

```rust
    assert!(out.contains("`hysteria2`"), "{out}");
```

换成

```rust
    assert!(out.contains("`tailscale`"), "{out}");
```

`crates/rurge/tests/cli.rs`——把

```rust
const TROJAN: &str = "[General]\n[Proxy]\nT = trojan, proxy.test, 443, password=s3same, ws=true, ws-path=/w\nOld = hysteria2, 1.2.3.4, 443, password=x\n[Rule]\nFINAL,DIRECT\n";
```

换成

```rust
const TROJAN: &str = "[General]\n[Proxy]\nT = trojan, proxy.test, 443, password=s3same, ws=true, ws-path=/w\nOld = tailscale, section-name=ts\n[Rule]\nFINAL,DIRECT\n";
```

`crates/rurge/tests/cli.rs`——把

```rust
    // `hysteria2` is still a later milestone; `trojan` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`hysteria2`") && !out.contains("`trojan`"),
```

换成

```rust
    // `tailscale` is still a later milestone; `trojan` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`tailscale`") && !out.contains("`trojan`"),
```

`crates/rurge/tests/cli.rs`——把

```rust
S = ssh, proxy.test, 22, username=u, password=s3same, idle-timeout=60\n\
Old = hysteria2, 1.2.3.4, 443, password=x\n[Rule]\nFINAL,DIRECT\n";
```

换成

```rust
S = ssh, proxy.test, 22, username=u, password=s3same, idle-timeout=60\n\
Old = tailscale, section-name=ts\n[Rule]\nFINAL,DIRECT\n";
```

`crates/rurge/tests/cli.rs`——把

```rust
    // `hysteria2` is still a later milestone; `ssh` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`hysteria2`") && !out.contains("`ssh`"),
```

换成

```rust
    // `tailscale` is still a later milestone; `ssh` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`tailscale`") && !out.contains("`ssh`"),
```

`crates/rurge/tests/cli.rs`——把

```rust
Old = hysteria2, 1.2.3.4, 443, password=x\n[Rule]\nFINAL,DIRECT\n\
```

换成

```rust
Old = tailscale, section-name=ts\n[Rule]\nFINAL,DIRECT\n\
```

`crates/rurge/tests/cli.rs`——把

```rust
    // `hysteria2` is still a later milestone; `wireguard` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`hysteria2`") && !out.contains("`wireguard`"),
```

换成

```rust
    // `tailscale` is still a later milestone; `wireguard` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`tailscale`") && !out.contains("`wireguard`"),
```

`crates/rurge/tests/cli.rs`——把

```rust
X = external, exec = \"/usr/bin/sshpass\", args = -p, args = hunter2, args = ssh, local-port = 1080\n\
Old = hysteria2, 1.2.3.4, 443, password=x\n[Rule]\nFINAL,DIRECT\n";
```

换成

```rust
X = external, exec = \"/usr/bin/sshpass\", args = -p, args = hunter2, args = ssh, local-port = 1080\n\
Old = tailscale, section-name=ts\n[Rule]\nFINAL,DIRECT\n";
```

`crates/rurge/tests/cli.rs`——把

```rust
    // `hysteria2` is still a later milestone; `external` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`hysteria2`") && !out.contains("`external`"),
```

换成

```rust
    // `tailscale` is still a later milestone; `external` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`tailscale`") && !out.contains("`external`"),
```

`crates/rurge/tests/cli.rs`——把

```rust
T2 = trust-tunnel, proxy.test, 443, username=bob, password=s3cretPw, h3=true\n\
Old = hysteria2, 1.2.3.4, 443, password=x\n[Rule]\nFINAL,DIRECT\n";
```

换成

```rust
T2 = trust-tunnel, proxy.test, 443, username=bob, password=s3cretPw, h3=true\n\
Old = tailscale, section-name=ts\n[Rule]\nFINAL,DIRECT\n";
```

`crates/rurge/tests/cli.rs`——把

```rust
    // `hysteria2` is still a later milestone; the HTTP/2 family is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`hysteria2`")
```

换成

```rust
    // `tailscale` is still a later milestone; the HTTP/2 family is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`tailscale`")
```

`crates/rurge/tests/cli.rs`——把

```rust
        .stdout(predicate::str::contains("bob").not());
```

换成

```rust
        .stdout(predicate::str::contains("bob").not());
}

const HYSTERIA2: &str = "[General]\n[Proxy]\n\
H = hysteria2, proxy.test, 443, password=s3cretPw, download-bandwidth=100, salamander-password=0bfsPw, port-hopping=5000-6000, port-hopping-interval=2, ecn=off\n\
Old = tailscale, section-name=ts\n[Rule]\nFINAL,DIRECT\n";
const HYSTERIA2_CHAINED: &str = "[General]\n[Proxy]\n\
H = hysteria2, proxy.test, 443, password=s3cretPw, port-hopping=5000-6000, underlying-proxy=E\n\
E = socks5, proxy.test, 1080\n[Rule]\nFINAL,DIRECT\n";

/// `rurge check` knows `hysteria2` (phase 2 M7 design 5.1): it is not "not
/// implemented"; `ecn` has an effect on it; a hop interval under 5 seconds
/// is raised to 5, saying so; `port-hopping` over an `underlying-proxy` is
/// an error; the passwords are never printed.
#[test]
fn check_knows_hysteria2() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "hy2.conf", HYSTERIA2))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // `tailscale` is still a later milestone; `hysteria2` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`tailscale`") && !out.contains("`hysteria2`"),
        "{out}"
    );
    assert!(!out.contains("W0029"), "{out}");
    assert!(
        out.contains("W0012")
            && out.contains(
                "hy2.conf:3: policy `H`: `port-hopping-interval` is below the minimum of 5 seconds; using 5"
            ),
        "{out}"
    );
    assert!(
        !out.contains("s3cretPw") && !out.contains("0bfsPw"),
        "{out}"
    );

    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "bad.conf", HYSTERIA2_CHAINED))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("E0018"))
        .stdout(predicate::str::contains(
            "bad.conf:3: policy `H`: `port-hopping` cannot be combined with `underlying-proxy`",
        ))
        .stdout(predicate::str::contains("s3cretPw").not());
```

`crates/rurge/src/cli/run.rs`——把

```rust
    #[test]
    fn log_levels_map_like_surge() {
```

换成

```rust
    #[test]
    fn quinn_says_only_what_rurge_cannot() {
        let filter = quiet_dependencies();
        let error = &tracing::Level::ERROR;
        // its drivers and the packets of the protocol: the QUIC outbounds
        // report what comes of them
        assert!(!filter.would_enable("quinn", error));
        assert!(!filter.would_enable("quinn_proto::connection", error));
        // what the system refused to send, which nothing else sees
        assert!(filter.would_enable("quinn_udp", &tracing::Level::WARN));
        assert!(!filter.would_enable("quinn_udp::imp", &tracing::Level::DEBUG));
        assert!(filter.would_enable("rurge_proto_quic::session", &tracing::Level::TRACE));
    }

    #[test]
    fn log_levels_map_like_surge() {
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
            .unwrap();
    }
}
```

换成

```rust
            .unwrap();
    }

    /// Port hopping cannot go over an `underlying-proxy` (the manual): a
    /// connector that does not dial directly gets no dialer that hops — as
    /// a group's relay would make one (phase 2 M3 design 5.4).
    #[test]
    fn hopping_over_a_connector_that_does_not_dial_directly_is_refused() {
        let quic = QuicOpts {
            port_hopping: Some(PortHopping {
                ports: vec![5000..=6000],
                interval: Duration::from_secs(30),
            }),
        };
        let build = |connector: Arc<dyn Connector>| {
            Dialer::new(
                connector,
                Target::new(HostName::parse("127.0.0.1"), 443),
                &TlsOpts::default(),
                &quic,
                Tristate::Auto,
                &[],
                TestCert::new().roots(),
            )
            .map(|_| ())
        };
        let err = build(Arc::new(Chain::default())).unwrap_err();
        assert_eq!(
            err.message,
            "`port-hopping` cannot be combined with `underlying-proxy`"
        );
        build(Arc::new(DirectConnector::new(Arc::new(Loopback)))).expect("a direct one hops");
    }
}
```

吞吐基准（`#[ignore]`）与假服务端为它加快的转发：

`crates/rurge-proto-quic/src/hysteria2/mod.rs`——把

```rust
        assert!(!log.contains(OBFS));
    }
}
```

换成

```rust
        assert!(!log.contains(OBFS));
    }

    /// A TCP echo that keeps up with a bulk transfer: `echo_server` echoes a
    /// kilobyte at a time, and a throughput measured through it would be
    /// its own.
    async fn bulk_echo() -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let (read, mut write) = stream.into_split();
                    let mut read = tokio::io::BufReader::with_capacity(256 << 10, read);
                    let _ = tokio::io::copy_buf(&mut read, &mut write).await;
                });
            }
        });
        addr
    }

    /// Not a gate (design 5.5): how fast a bulk transfer goes through a
    /// tunnel to a loopback echo behind `FakeHysteria2`, both ways at once,
    /// plain and with Salamander — the cost of the UDP layers under quinn
    /// (design 10). In release: `cargo test -p rurge-proto-quic --release
    /// throughput -- --ignored --nocapture`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn throughput() {
        const TOTAL: usize = 64 * 1024 * 1024;
        let echo = bulk_echo().await;
        for (what, obfs, params) in [
            ("plain", None, "password=pw"),
            (
                "salamander",
                Some(Hysteria2Obfs::Salamander("obfs".into())),
                "password=pw, salamander-password=obfs",
            ),
        ] {
            let fake = FakeHysteria2::spawn(Hysteria2Script {
                obfs,
                ..script("pw")
            });
            let out = outbound(&fake, params);
            let (mut read, mut write) = tokio::io::split(tunnel(&out, &target(echo)).await);
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
                "{what}: {} MiB through hysteria2 and back in {elapsed:.2?}: {:.1} MiB/s each way",
                TOTAL >> 20,
                (TOTAL >> 20) as f64 / elapsed.as_secs_f64()
            );
        }
    }
}
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
const H3_REQUEST_REJECTED: u64 = 0x10b;
```

换成

```rust
const H3_REQUEST_REJECTED: u64 = 0x10b;
/// How much of a TCP tunnel the fake relays at a time, each way.
const RELAY: usize = 64 << 10;
```

`crates/rurge-proto-quic/src/testing/hysteria2.rs`——把

```rust
    tokio::io::copy_bidirectional(&mut stream, &mut upstream).await?;
```

换成

```rust
    // in large pieces, so that a bulk transfer measures the client rather
    // than this relay (the throughput benchmark)
    tokio::io::copy_bidirectional_with_sizes(&mut stream, &mut upstream, RELAY, RELAY).await?;
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-engine --test outbounds_hysteria2`
Expected: FAIL——`hysteria2` 行还没有 spec、工厂还不会构建，经它的会话都被拒绝：

```text
test a_policy_test_goes_through_the_connection ... FAILED
test tunnels_keep_working_while_the_port_hops ... FAILED
test hysteria2_goes_over_an_underlying_socks5_proxy ... FAILED
test tcp_leaves_through_hysteria2_on_one_connection ... FAILED
test a_reload_keeps_an_unchanged_policy_and_its_connection ... FAILED
test a_hopping_member_of_a_relayed_group_is_left_out ... FAILED
test a_refused_password_fails_the_session ... FAILED
test either_obfuscation_carries_tcp_and_udp ... FAILED
test a_new_member_of_the_underlying_group_gets_a_new_connection ... FAILED
test a_server_that_relays_no_udp_fails_the_flow ... FAILED
test udp_leaves_through_hysteria2_in_datagrams ... FAILED
thread 'a_policy_test_goes_through_the_connection' panicked at crates\rurge-engine\tests\outbounds_hysteria2.rs:479:40:
thread 'tunnels_keep_working_while_the_port_hops' panicked at crates\rurge-engine\tests\common\mod.rs:200:9:
thread 'hysteria2_goes_over_an_underlying_socks5_proxy' panicked at crates\rurge-engine\tests\common\mod.rs:200:9:
thread 'tcp_leaves_through_hysteria2_on_one_connection' panicked at crates\rurge-engine\tests\common\mod.rs:200:9:
thread 'a_reload_keeps_an_unchanged_policy_and_its_connection' panicked at crates\rurge-engine\tests\common\mod.rs:200:9:
thread 'a_hopping_member_of_a_relayed_group_is_left_out' panicked at crates\rurge-engine\tests\outbounds_hysteria2.rs:448:5:
assertion `left == right` failed
  left: Some(["Hop", "Plain"])
 right: Some(["Plain (via Entry)"])
thread 'a_refused_password_fails_the_session' panicked at crates\rurge-engine\tests\outbounds_hysteria2.rs:155:10:
called `Result::unwrap()` on an `Err` value: Custom { kind: UnexpectedEof, error: "early eof" }
thread 'either_obfuscation_carries_tcp_and_udp' panicked at crates\rurge-engine\tests\common\mod.rs:200:9:
thread 'a_new_member_of_the_underlying_group_gets_a_new_connection' panicked at crates\rurge-engine\tests\common\mod.rs:200:9:
thread 'a_server_that_relays_no_udp_fails_the_flow' panicked at crates\rurge-engine\tests\outbounds_hysteria2.rs:241:5:
assertion `left == right` failed: RequestRecord { id: 1, listener: Socks5, transport: Udp, src: 127.0.0.1:6575, dst: "127.0.0.1:58505", rule: Some("IP-CIDR,127.0.0.1/32,H,no-resolve"), policy: ["H", "!unsupported:hysteria2", "REJECT"], sni: None, protocol: Some(Udp), up: 0, down: 0, started_ms: 1790938873694, elapsed_ms: 0, connect_ms: None, first_byte_ms: None, status: Rejected("REJECT"), error: Some("policy protocol not implemented: hysteria2") }
  left: Rejected("REJECT")
 right: Failed
thread 'udp_leaves_through_hysteria2_in_datagrams' panicked at crates\rurge-engine\tests\common\mod.rs:302:18:
test result: FAILED. 0 passed; 11 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.04s
```

- [ ] **Step 3: 实现**

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    // the engine builds `hysteria2` from M7a task 8 on: until then a valid
    // line is checked in full but has no spec
    let built = policy.kind != PolicyKind::Hysteria2;
    let spec = (!failed && not_implemented.is_none() && built).then(|| PolicySpec {
```

换成

```rust
    let spec = (!failed && not_implemented.is_none()).then(|| PolicySpec {
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
use rurge_proto::{Direct, OutboundRef};
```

换成

```rust
use rurge_proto::{Direct, OutboundRef};
use rurge_proto_quic::Hysteria2Outbound;
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            // the loader makes no spec of these lines before M7a task 8
            ProtoSpec::Hysteria2(_) => {
                return Err(BuildError::new(format!(
                    "policy `{}`: `{}` is not implemented yet",
                    spec.name,
                    spec.kind.keyword()
                )));
            }
```

换成

```rust
            ProtoSpec::Hysteria2(hysteria2) => Arc::new(Hysteria2Outbound::new(
                &spec.name,
                server_of(spec)?,
                hysteria2,
                spec.common.ecn,
                &self.keystore,
                self.roots.clone(),
                connector,
            )?),
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
    /// dials directly, else over its UDP flow (`underlying-proxy`). `tls` is
    /// the policy's — ALPN `h3` unless it names others — with `client-cert`
    /// from `keystore`.
```

换成

```rust
    /// dials directly, else over its UDP flow (`underlying-proxy`), which
    /// cannot hop. `tls` is the policy's — ALPN `h3` unless it names others
    /// — with `client-cert` from `keystore`.
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
    ) -> Result<Dialer, BuildError> {
```

换成

```rust
    ) -> Result<Dialer, BuildError> {
        // the configuration refuses a line that has both (E0018), but a
        // group's `underlying-proxy` chains its members without it (`M (via
        // R)`, phase 2 M3 design 5.4): such a member is not built, and the
        // registry leaves it out
        if quic.port_hopping.is_some() && connector.as_direct().is_none() {
            return Err(BuildError::new(
                "`port-hopping` cannot be combined with `underlying-proxy`",
            ));
        }
```

`crates/rurge/src/capabilities.rs`——把

```rust
//! `trust-tunnel` over HTTP/2 only (phase 2 M6c), `select` groups,
//! `url-test` / `fallback` / `load-balance` groups (phase 2 M3b), and
//! `smart` groups (phase 2 M3c).
```

换成

```rust
//! `trust-tunnel` over HTTP/2 only (phase 2 M6c), `hysteria2` (phase 2
//! M7a), `select` groups, `url-test` / `fallback` / `load-balance` groups
//! (phase 2 M3b), and `smart` groups (phase 2 M3c).
```

`crates/rurge/src/capabilities.rs`——把

```rust
            PolicyKind::TrustTunnel,
```

换成

```rust
            PolicyKind::TrustTunnel,
            PolicyKind::Hysteria2,
```

`crates/rurge/src/cli/run.rs`——把

```rust
/// carry no policy name, and the `wireguard` outbound logs its handshakes.
```

换成

```rust
/// carry no policy name, and the `wireguard` outbound logs its handshakes;
/// quinn's are about its drivers and every packet, and the QUIC outbounds
/// report what comes of them. quinn-udp's warnings stay: what the system
/// refused to send, which it drops without telling anyone else.
```

`crates/rurge/src/cli/run.rs`——把

```rust
        .with_target("boringtun", LevelFilter::OFF)
```

换成

```rust
        .with_target("boringtun", LevelFilter::OFF)
        // `quinn` covers `quinn_proto` too; the longer target wins
        .with_target("quinn", LevelFilter::OFF)
        .with_target("quinn_udp", LevelFilter::WARN)
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-engine --test outbounds_hysteria2` → 通过（11 条：经 HTTP CONNECT / SOCKS5 的 TCP 共用一条连接、经 SOCKS5 UDP ASSOCIATE 的 UDP（全锥、分片）、`Hysteria-UDP: false` 时 UDP 流失败、口令错时会话失败且请求记录里没有口令、两种混淆、端口跳跃中隧道不断、经 `underlying-proxy`（socks5，带 UDP）、底层组换成员后走新连接、重载沿用未变的策略与它的连接、经该策略的 URL 测试、带端口跳跃的组成员派生出的 `M (via R)` 被略去）。
Run: `cargo test -p rurge --test cli check_knows_hysteria2` → 通过。
Run（不作门禁，记下数字）: `cargo test -p rurge-proto-quic --release throughput -- --ignored --nocapture`

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add Cargo.lock crates
git commit -m "feat(engine): 装配 hysteria2、能力表翻转；端口跳跃不能经链；未实现协议的例子换成 tailscale"
```

### Task 9: 互操作与文档

sing-box 1.14.2 `hysteria2` 入站的夹具与用例（P22）；兼容性清单、手工验收、两份 README、`CLAUDE.md`、PRD 与总设计。

**Files:**
- Create: `tests/interop/tests/sing_box_hysteria2.rs`
- Modify: `tests/interop/{src/lib.rs, README.md}`、`docs/surge-compatibility-matrix.md`、`docs/acceptance/phase2-manual.md`、`README.md`、`README_en.md`、`CLAUDE.md`、`docs/requirements.md`、`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`

**Interfaces:**
- Consumes: 互操作夹具既有的 `SingBox`、`Reference`、`roundtrip` / `roundtrip_big`、测试 TLS 夹具。
- Produces: `rurge_interop` 的 `render_hysteria2` / `SingBox::spawn_hysteria2`（只监听 `127.0.0.1`）。

- [ ] **Step 1: 互操作夹具与用例**

`tests/interop/src/lib.rs`——把

```rust
//! 127.0.0.1 only — but for the UDP port of a WireGuard endpoint, which
//! sing-box opens on every address — its single outbound is `direct`, and it
//! never holds a key that touches the machine (`set_system_proxy`, `tun`,
//! `auto_route`; a WireGuard endpoint runs in user space).
```

换成

```rust
//! 127.0.0.1 only (a `hysteria2` inbound's UDP port too) — but for the UDP
//! port of a WireGuard endpoint, which sing-box opens on every address — its
//! single outbound is `direct`, and it never holds a key that touches the
//! machine (`set_system_proxy`, `tun`, `auto_route`; a WireGuard endpoint
//! runs in user space).
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

/// Hysteria 2's QUIC obfuscation, as sing-box's `obfs.type` names it
/// (`gecko` since sing-box 1.14.0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hysteria2Obfs {
    Salamander,
    Gecko,
}

/// sing-box's `hysteria2` inbound: QUIC with TLS (ALPN `h3`) and one user,
/// optionally behind Salamander or Gecko.
pub struct Hysteria2Inbound {
    pub password: String,
    /// The obfuscation and its password.
    pub obfs: Option<(Hysteria2Obfs, String)>,
    /// No client certificate is asked for: `client_ca` must be `None`.
    pub tls: TlsFiles,
}

/// The whole configuration for `inbound` on UDP port `port`, and a `mixed`
/// inbound on `ready` that says when sing-box is up — a UDP port cannot be
/// probed; sing-box starts its inbounds one after another in the order
/// written, so the probe comes last. Gecko keeps sing-box's default packet
/// sizes (512–1200 bytes, rurge's too); without a `masquerade` a wrong
/// password is answered 404.
pub fn render_hysteria2(inbound: &Hysteria2Inbound, port: u16, ready: u16) -> Value {
    assert!(
        inbound.tls.client_ca.is_none(),
        "the hysteria2 inbound asks for no client certificate here"
    );
    let mut hysteria2 = json!({
        "type": "hysteria2",
        "tag": "hysteria2",
        "listen": "127.0.0.1",
        "listen_port": port,
        "users": [{ "name": "u", "password": inbound.password }],
        "tls": {
            "enabled": true,
            "alpn": ["h3"],
            "certificate_path": inbound.tls.certificate,
            "key_path": inbound.tls.key,
        },
    });
    if let Some((kind, password)) = &inbound.obfs {
        let kind = match kind {
            Hysteria2Obfs::Salamander => "salamander",
            Hysteria2Obfs::Gecko => "gecko",
        };
        hysteria2["obfs"] = json!({ "type": kind, "password": password });
    }
    json!({
        "log": { "level": "warn", "timestamp": false },
        "inbounds": [
            hysteria2,
            { "type": "mixed", "tag": "ready", "listen": "127.0.0.1", "listen_port": ready },
        ],
        "outbounds": [{ "type": "direct", "tag": "direct" }],
    })
}

/// A reference implementation running as a child on the loopback; killed and
```

`tests/interop/src/lib.rs`——把

```rust
        let config = render_wireguard(endpoint, port, ready);
```

换成

```rust
        let config = render_wireguard(endpoint, port, ready);
        (SingBox::launch(binary, dir, &config, vec![ready]), port)
    }

    /// Starts `binary` with `inbound` on a UDP port of its own; that port,
    /// once sing-box is up.
    pub fn spawn_hysteria2(
        binary: &Path,
        dir: &Path,
        inbound: &Hysteria2Inbound,
    ) -> (SingBox, u16) {
        let (port, ready) = (free_udp_port(), free_port());
        let config = render_hysteria2(inbound, port, ready);
```

`tests/interop/src/lib.rs`——把

```rust
        assert_eq!(config["inbounds"][0]["listen_port"], 1001);
    }

    #[test]
    fn free_ports_are_usable() {
```

换成

```rust
        assert_eq!(config["inbounds"][0]["listen_port"], 1001);
    }

    fn hysteria2(obfs: Option<Hysteria2Obfs>) -> Hysteria2Inbound {
        Hysteria2Inbound {
            password: "hy-pw".into(),
            obfs: obfs.map(|kind| (kind, "obfs-pw".to_string())),
            tls: TlsFiles {
                certificate: "leaf.pem".into(),
                key: "leaf.key".into(),
                client_ca: None,
            },
        }
    }

    /// The QUIC inbound and the probe on 127.0.0.1, nothing of the machine's.
    #[test]
    fn the_hysteria2_configuration_never_touches_the_machine() {
        for obfs in [
            None,
            Some(Hysteria2Obfs::Salamander),
            Some(Hysteria2Obfs::Gecko),
        ] {
            let config = render_hysteria2(&hysteria2(obfs), 4443, 1001);
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
    }

    #[test]
    fn the_hysteria2_inbound_is_rendered_as_sing_box_spells_it() {
        let config = render_hysteria2(&hysteria2(None), 4443, 1001);
        let inbound = &config["inbounds"][0];
        assert_eq!(
            (&inbound["type"], &inbound["listen_port"]),
            (&json!("hysteria2"), &json!(4443))
        );
        assert_eq!(
            inbound["users"],
            json!([{ "name": "u", "password": "hy-pw" }])
        );
        assert_eq!(
            inbound["tls"],
            json!({
                "enabled": true,
                "alpn": ["h3"],
                "certificate_path": "leaf.pem",
                "key_path": "leaf.key",
            })
        );
        // sing-box's defaults: no bandwidth limit, a 404 for a stranger
        for absent in ["obfs", "up_mbps", "down_mbps", "masquerade"] {
            assert!(inbound.get(absent).is_none(), "`{absent}` in {inbound}");
        }
        // the probe after it: sing-box starts its inbounds in order
        assert_eq!(
            config["inbounds"][1],
            json!({ "type": "mixed", "tag": "ready", "listen": "127.0.0.1", "listen_port": 1001 })
        );
        for (kind, name) in [
            (Hysteria2Obfs::Salamander, "salamander"),
            (Hysteria2Obfs::Gecko, "gecko"),
        ] {
            let config = render_hysteria2(&hysteria2(Some(kind)), 4443, 1001);
            // Gecko's packet sizes stay sing-box's defaults
            assert_eq!(
                config["inbounds"][0]["obfs"],
                json!({ "type": name, "password": "obfs-pw" })
            );
        }
    }

    #[test]
    fn free_ports_are_usable() {
```

新建 `tests/interop/tests/sing_box_hysteria2.rs`：

```rust
//! rurge's `hysteria2` outbound against sing-box 1.14.2's `hysteria2`
//! inbound (phase 2 M7 design 5.5, M7-D6): TCP with one chunk, with many
//! and past a stream's receive window, UDP whole and in Hysteria fragments,
//! a wrong password, and either obfuscation (Salamander, Gecko). sing-box
//! listens on a loopback UDP port with a certificate for 127.0.0.1 signed by
//! the fixture's CA, which rurge trusts; every target is a loopback IP
//! literal. Port hopping is not covered: the server would need a DNAT range.

mod common;

use common::*;
use rurge_interop::{Hysteria2Inbound, Hysteria2Obfs};
use std::time::Duration;

const PASSWORD: &str = "hy2-interop";
const OBFS_PASSWORD: &str = "obfs-interop";

/// sing-box's `hysteria2` inbound with `obfs`, its UDP port, and the fixture
/// whose CA signed its certificate for 127.0.0.1.
fn server(bin: &Path, dir: &Path, obfs: Option<Hysteria2Obfs>) -> (SingBox, u16, Arc<TlsFixture>) {
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let inbound = Hysteria2Inbound {
        password: PASSWORD.to_string(),
        obfs: obfs.map(|kind| (kind, OBFS_PASSWORD.to_string())),
        tls: leaf_files(&fixture, dir),
    };
    let (sb, port) = SingBox::spawn_hysteria2(bin, dir, &inbound);
    (sb, port, fixture)
}

/// `name = hysteria2, 127.0.0.1, <port>, <params>`.
fn line(name: &str, port: u16, params: &str) -> String {
    format!("{name} = hysteria2, 127.0.0.1, {port}, {params}\n")
}

fn profile(lines: &[String]) -> String {
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
/// connection.
async fn tcp_and_udp(out: &OutboundRef) {
    let (echo, udp_echo) = (echo_server().await, udp_echo_server().await);
    roundtrip(out, echo).await;
    roundtrip_big(out, echo).await;
    udp(out, udp_echo).await;
}

/// Past rurge's receive window of a stream (8 MiB, phase 2 M7 design 3.2):
/// what comes back needs the credit rurge grants as it reads. With
/// `download-bandwidth` sing-box sends to rurge at that rate (Brutal)
/// instead of finding one (BBR).
#[tokio::test]
async fn tcp_against_sing_box() {
    let Some(bin) = sing_box_or_skip("tcp_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (_sb, port, fixture) = server(&bin, dir.path(), None);
    let profile = profile(&[
        line("Plain", port, &format!("password={PASSWORD}")),
        line(
            "Rx",
            port,
            &format!("password={PASSWORD}, download-bandwidth=100"),
        ),
    ]);
    let echo = echo_server().await;
    for name in ["Plain", "Rx"] {
        let out = outbound(&profile, name, Some(&fixture));
        roundtrip(&out, echo).await;
        roundtrip_big(&out, echo).await;
        transfer(&out, echo, (9 << 20) + 777).await;
    }
}

#[tokio::test]
async fn udp_against_sing_box() {
    let Some(bin) = sing_box_or_skip("udp_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (_sb, port, fixture) = server(&bin, dir.path(), None);
    let profile = profile(&[line("H", port, &format!("password={PASSWORD}"))]);
    let out = outbound(&profile, "H", Some(&fixture));
    udp(&out, udp_echo_server().await).await;
}

/// sing-box answers a wrong password as its masquerade would — 404 without
/// one —: the authentication failed, for TCP and UDP alike.
#[tokio::test]
async fn a_wrong_password_fails_the_authentication_against_sing_box() {
    let Some(bin) = sing_box_or_skip("a_wrong_password_fails_the_authentication_against_sing_box")
    else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (_sb, port, fixture) = server(&bin, dir.path(), None);
    let profile = profile(&[line("Wrong", port, "password=nope")]);
    let out = outbound(&profile, "Wrong", Some(&fixture));
    let bound = Duration::from_secs(10);
    let tcp = tokio::time::timeout(
        bound,
        out.connect_tcp(&target(echo_server().await), &ConnectOpts::default()),
    )
    .await
    .expect("sing-box answers within the bound");
    let Err(tcp) = tcp else {
        panic!("a wrong password must not open a tunnel");
    };
    let udp = tokio::time::timeout(bound, out.open_udp(&ConnectOpts::default()))
        .await
        .expect("sing-box answers within the bound");
    let Err(udp) = udp else {
        panic!("a wrong password must not open a UDP carrier");
    };
    for err in [tcp, udp] {
        assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
        assert_eq!(err.to_string(), "hysteria2: authentication failed");
    }
}

/// Salamander over every packet (`obfs.type: salamander`).
#[tokio::test]
async fn salamander_against_sing_box() {
    let Some(bin) = sing_box_or_skip("salamander_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (_sb, port, fixture) = server(&bin, dir.path(), Some(Hysteria2Obfs::Salamander));
    let profile = profile(&[line(
        "H",
        port,
        &format!("password={PASSWORD}, salamander-password={OBFS_PASSWORD}"),
    )]);
    tcp_and_udp(&outbound(&profile, "H", Some(&fixture))).await;
}

/// Gecko (`obfs.type: gecko`, sing-box 1.14.0 and later): the handshake's
/// packets in fragments both ways, every packet salted; sing-box's default
/// packet sizes (512–1200 bytes) are rurge's.
#[tokio::test]
async fn gecko_against_sing_box() {
    let Some(bin) = sing_box_or_skip("gecko_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (_sb, port, fixture) = server(&bin, dir.path(), Some(Hysteria2Obfs::Gecko));
    let profile = profile(&[line(
        "H",
        port,
        &format!("password={PASSWORD}, gecko-password={OBFS_PASSWORD}"),
    )]);
    tcp_and_udp(&outbound(&profile, "H", Some(&fixture))).await;
}
```

`tests/interop/README.md`——把

```markdown
`rurge-interop` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它把 [sing-box](https://sing-box.sagernet.org/)、[xray](https://github.com/XTLS/Xray-core)、[shadowsocks-rust](https://github.com/shadowsocks/shadowsocks-rust) 的 `ssserver`、Surge 官方的 `snell-server` 与 [TrustTunnel](https://github.com/TrustTunnel/TrustTunnel) 的 `trusttunnel_endpoint` 作为参照实现，以回环子进程的方式拉起来，驱动 rurge 的 `http` / `https` / `socks5` / `trojan` / `vmess` / `anytls` / `wireguard` / `ss` / `snell` / `h2-connect` / `trust-tunnel` 出站，以及包在 Shadow TLS 里的 `trojan`，去连它们，验证 rurge 与真实的第三方实现互通。xray 只用来跑 `vmess`：VMess 协议由 xray 所在的这一脉实现定义，sing-box 的实现是重写，手写的编解码需要两个独立参照互相印证（M2 设计 M2-D5）。
```

换成

```markdown
`rurge-interop` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它把 [sing-box](https://sing-box.sagernet.org/)、[xray](https://github.com/XTLS/Xray-core)、[shadowsocks-rust](https://github.com/shadowsocks/shadowsocks-rust) 的 `ssserver`、Surge 官方的 `snell-server` 与 [TrustTunnel](https://github.com/TrustTunnel/TrustTunnel) 的 `trusttunnel_endpoint` 作为参照实现，以回环子进程的方式拉起来，驱动 rurge 的 `http` / `https` / `socks5` / `trojan` / `vmess` / `anytls` / `wireguard` / `ss` / `snell` / `h2-connect` / `trust-tunnel` / `hysteria2` 出站，以及包在 Shadow TLS 里的 `trojan`，去连它们，验证 rurge 与真实的第三方实现互通。xray 只用来跑 `vmess`：VMess 协议由 xray 所在的这一脉实现定义，sing-box 的实现是重写，手写的编解码需要两个独立参照互相印证（M2 设计 M2-D5）。
```

`tests/interop/README.md`——把

```markdown
互操作测试固定 sing-box **1.14.2**（2026-09-24 发布的稳定版；阶段 2 / M6b 从 1.14.1 升上来：`snell` 入站 1.14.0 起才有，1.14.2 是 1.14.x 的最新版）。CI 下载并校验以下三个发布包（SHA-256 取自 GitHub 发布 API 每个资产的 `digest`）：
```

换成

```markdown
互操作测试固定 sing-box **1.14.2**（2026-09-24 发布的稳定版；阶段 2 / M6b 从 1.14.1 升上来：`snell` 入站 1.14.0 起才有，1.14.2 是 1.14.x 的最新版；`hysteria2` 入站的 Gecko 混淆同样 1.14.0 起才有，QUIC 支持（构建标签 `with_quic`）是发布包的缺省）。CI 下载并校验以下三个发布包（SHA-256 取自 GitHub 发布 API 每个资产的 `digest`）：
```

`tests/interop/README.md`——把

```markdown
本地默认不安装 sing-box、xray、shadowsocks-rust、snell-server 与 TrustTunnel endpoint：`cargo test -p rurge-interop` 会正常通过，sing-box 的十九个互操作用例、xray 的两个、shadowsocks-rust 的两个、snell-server 的四个与 TrustTunnel endpoint 的四个互操作用例各打印一行 `skipping …` 后直接返回（各夹具自身的单元测试——配置渲染、"配置绝不碰本机"的安全守卫、取空闲端口——照常运行并断言）。要在本机真正跑 sing-box 的用例，二选一：
```

换成

```markdown
本地默认不安装 sing-box、xray、shadowsocks-rust、snell-server 与 TrustTunnel endpoint：`cargo test -p rurge-interop` 会正常通过，sing-box 的二十四个互操作用例、xray 的两个、shadowsocks-rust 的两个、snell-server 的四个与 TrustTunnel endpoint 的四个互操作用例各打印一行 `skipping …` 后直接返回（各夹具自身的单元测试——配置渲染、"配置绝不碰本机"的安全守卫、取空闲端口——照常运行并断言）。要在本机真正跑 sing-box 的用例，二选一：
```

`tests/interop/README.md`——把

```markdown
- Snell（`tests/snell.rs` 里 `…_against_sing_box` 的四个用例，阶段 2 / M6b）：sing-box 的 `snell` 入站（1.14.0 起），`version: 5`、一个 `psk`、不配 `users`（服务端不看 client id）。sing-box 没有"version 4"的入站；`version: 5` 走 v4 / v5 共用的线上格式，所以同时接受 v4 客户端。覆盖 `version=5` 与 `version=4` 两种客户端的单块与跨多块（100 000 字节）TCP 往返；`reuse=true` 时一连四个请求（每个都两个方向干净结束，连接回池给下一个请求用）再加一个大负载；`obfs=http`（入站的 `obfs_mode: http`；含 `reuse=true` 与 UDP）；以及 UDP over TCP（命令 `0x06`）经同一个载体往返一个回环 UDP 回显两次。
```

换成

```markdown
- Snell（`tests/snell.rs` 里 `…_against_sing_box` 的四个用例，阶段 2 / M6b）：sing-box 的 `snell` 入站（1.14.0 起），`version: 5`、一个 `psk`、不配 `users`（服务端不看 client id）。sing-box 没有"version 4"的入站；`version: 5` 走 v4 / v5 共用的线上格式，所以同时接受 v4 客户端。覆盖 `version=5` 与 `version=4` 两种客户端的单块与跨多块（100 000 字节）TCP 往返；`reuse=true` 时一连四个请求（每个都两个方向干净结束，连接回池给下一个请求用）再加一个大负载；`obfs=http`（入站的 `obfs_mode: http`；含 `reuse=true` 与 UDP）；以及 UDP over TCP（命令 `0x06`）经同一个载体往返一个回环 UDP 回显两次。

- Hysteria 2（`tests/sing_box_hysteria2.rs` 的五个用例，阶段 2 / M7a）：sing-box 的 `hysteria2` 入站只听回环上的一个 UDP 端口；证书签给 `127.0.0.1`、由夹具的 CA 签发，rurge 信任这个 CA（服务器写成 IP，不发 SNI）；`tls.alpn` 写 `["h3"]`；一个用户（`users[].password`）；不配 `masquerade`（口令不对时 sing-box 回 404）、不限带宽。覆盖 TCP 的单块、跨多块（100 000 字节）与每个方向 9 MiB 同时往返（超过 rurge 每条流 8 MiB 的接收窗口：回来的数据要靠 rurge 边读边放出的流控额度），这三项再用写了 `download-bandwidth=100` 的策略做一遍（sing-box 于是按这个速率用 Brutal 发，不用 BBR）；UDP：同一条连接上两个载体（各占一个会话号）各自往返几个字节与 3000 字节的数据报，后者两个方向都按 Hysteria 的分片走（sing-box 把应答切成约 1200 字节一片，不回超过 4096 字节的消息；macOS 的回环不送超过 9216 字节的数据报）；口令错误时 TCP 与 UDP 都以 `hysteria2: authentication failed` 失败；`obfs` 的 `salamander` 与 `gecko`（包长用 sing-box 的缺省 512–1200 字节，与 rurge 相同）两种混淆下的 TCP（单块与跨多块）与 UDP。

**不覆盖端口跳跃**：`port-hopping` 要在服务端用防火墙把一段端口 DNAT 到它的监听端口（Hysteria 文档的部署方式），夹具不改本机的防火墙与网络设置，回环上做不到；端口跳跃由回环假服务端（`rurge_proto_quic::testing::FakeHysteria2` 同时监听几个端口，跳跃间隔经只用于测试的入口缩短）与手工验收（`docs/acceptance/phase2-manual.md` 的 M7a 一节）覆盖。
```

`tests/interop/README.md`——把

```markdown
- 夹具渲染出的 sing-box 配置只有 `log` / `inbounds` / `outbounds` 三个顶层键（WireGuard 的配置另有 `endpoints`）；每个入站只监听 `127.0.0.1`；唯一的出站是 `direct`。WireGuard 端点在用户态运行（`system: false`），它的 UDP 端口开在所有地址上（端点没有监听地址这一项；`rurge_interop::render_wireguard` 的单元测试 `the_wireguard_configuration_never_touches_the_machine` 断言其余各项）。xray 配置同样只有这三个顶层键，唯一的出站是 `freedom`。`ssserver` 的配置只有 `servers`，每个服务端只听 `127.0.0.1`（TCP 与同号的 UDP）。`snell-server` 的配置只有 `[snell-server]` 一节，只听 `127.0.0.1`。TrustTunnel endpoint 只听 `127.0.0.1`、只开 HTTP/2，转发只有 `direct`；`allow_private_network_connections = true` 只是为了让它连回环上的 echo。任何地方都不出现 `set_system_proxy`、`tun`、`auto_route` 这些键（`rurge_interop::render` 与 `rurge_interop::xray::render` 的单元测试 `the_configuration_never_touches_the_machine` 各自断言这一点）；`shadowtls` 入站的 `handshake.server` 恒为 `127.0.0.1`（夹具的单元用例断言）。
```

换成

```markdown
- 夹具渲染出的 sing-box 配置只有 `log` / `inbounds` / `outbounds` 三个顶层键（WireGuard 的配置另有 `endpoints`）；每个入站只监听 `127.0.0.1`（`hysteria2` 入站的 UDP 端口也是，同一份配置里另有一个只用来判断就绪的 `mixed` 入站；`rurge_interop::render_hysteria2` 的单元测试 `the_hysteria2_configuration_never_touches_the_machine` 断言这些）；唯一的出站是 `direct`。WireGuard 端点在用户态运行（`system: false`），它的 UDP 端口开在所有地址上（端点没有监听地址这一项；`rurge_interop::render_wireguard` 的单元测试 `the_wireguard_configuration_never_touches_the_machine` 断言其余各项）。xray 配置同样只有这三个顶层键，唯一的出站是 `freedom`。`ssserver` 的配置只有 `servers`，每个服务端只听 `127.0.0.1`（TCP 与同号的 UDP）。`snell-server` 的配置只有 `[snell-server]` 一节，只听 `127.0.0.1`。TrustTunnel endpoint 只听 `127.0.0.1`、只开 HTTP/2，转发只有 `direct`；`allow_private_network_connections = true` 只是为了让它连回环上的 echo。任何地方都不出现 `set_system_proxy`、`tun`、`auto_route` 这些键（`rurge_interop::render` 与 `rurge_interop::xray::render` 的单元测试 `the_configuration_never_touches_the_machine` 各自断言这一点）；`shadowtls` 入站的 `handshake.server` 恒为 `127.0.0.1`（夹具的单元用例断言）。
```

- [ ] **Step 2: 运行**

Run: `cargo test -p rurge-interop`
Expected: 本机没有 sing-box 时，5 条互操作用例各打印一行 `skipping …` 后通过；夹具的 2 条单元用例（配置的渲染、只监听回环）照常断言并通过。互操作由 CI（三平台都装了 sing-box 1.14.2、`RURGE_INTEROP_REQUIRED=1`）证明，工作流不用改。

- [ ] **Step 3: 文档**

兼容性清单（`hysteria2` 行、4.3 的 `tfo` 与 `ecn`、4.4 的 `alpn` 与 Shadow TLS 约束、4.5 的 UDP、4.6 的参数行、`underlying-proxy`、脱敏名单、统计附录中本行的变化）：

`docs/surge-compatibility-matrix.md`——把

```markdown
| `hysteria2` | Hysteria 2 | iOS 5.8 / Mac 5.4+ | ✅ | 2 | Salamander 混淆 ✅；Gecko 混淆 ❓ |
```

换成

```markdown
| `hysteria2` | Hysteria 2 | iOS 5.8 / Mac 5.4+ | ✅ | 2 | M7a（阶段 2）已实现 TCP 与 UDP。**连接**：每个策略一条 QUIC 连接（quinn；每条连接一个自己的 quinn endpoint），多路复用——每个 TCP 连接一条双向流，UDP 走同一条连接的 QUIC 数据报；第一次用到时才拨号，同时到来的请求共用一次拨号与认证，失败一并报给等着的请求（不退避：口令错误时每个新请求都重新拨号认证一次）；整个建立过程（拨号、QUIC 与 TLS 握手、认证、服务端对 TCP 请求的应答）受连接超时约束。直连时 QUIC 的 UDP socket 与 DIRECT 的 UDP 载体同样建（`interface` `allow-other-interface` `tos`，收发缓冲尽量设为 7 MiB），服务器名经策略自己的解析（含 `[Host]` 与 `dns-follow-interface`）、按 `ip-version` 取一个地址。只用 TLS 1.3，不用 0-RTT；服务器写成 IP 且没写 `sni` 时不发 SNI。空闲超时 30 秒、保活 10 秒：休眠唤醒、换网后静默断掉的连接约 30 秒内被认出，之后的请求新建连接（quinn 0.11 没有应用层的 PING，其间的请求各自超时）；接收窗口每条流 8 MiB、每条连接 20 MiB（同参考客户端）；拥塞控制固定 Cubic（`download-bandwidth` 只上报给服务端，见 4.6）。连接出错、空闲超时或被服务端关闭之后，下一个请求新建连接；收到 HTTP/3 GOAWAY 的连接不再接新请求，其上已有的流走完；没有流也没有 UDP 载体满 60 秒的连接主动关闭（每 30 秒检查一次）；刚取用的连接已被服务端丢弃（例如服务端重启后回 stateless reset）、请求在服务端应答之前就失败时，在新连接上重试一次（只对请求到来时已经建好的连接）。`underlying-proxy` 是组时跟随组的当前成员：成员变了，新请求与 UDP 载体改走经新成员的新连接（日志 `hysteria2: the underlying policy changed; a new connection is made`，每条连接一次），旧连接上的流走完。**认证**：HTTP/3 的 `POST https://hysteria/auth`（rurge 自写的最小 HTTP/3 客户端，只用 QPACK 静态表，M7-D5），带 `Hysteria-Auth`（QPACK 不入索引的字面量）、`Hysteria-CC-RX`（`download-bandwidth` × 125000，没写为 0）与 `Hysteria-Padding`（256–2047 个字母与数字）；应答 233 为成功，`Hysteria-UDP` 按 Go 的 `strconv.ParseBool` 读（缺了算 false）；其它状态都是 `hysteria2: authentication failed`，并以 H3_GENERAL_PROTOCOL_ERROR（0x101）关闭连接（同参考客户端）。**TCP**：请求 `0x401 ‖ 目标 ‖ 64–511 个字母与数字的填充`，目标写成 `host:port`（IPv6 带方括号，名字转成 A-label、交给服务端解析；写不出或超过 2048 字节的名字不拨号即失败 `hysteria2: the host name cannot be sent to the server`）；等服务端应答（不用 fast-open，参考客户端的缺省）：状态 0 开始转发，其它任何状态都是 `hysteria2: the server refused the connection: <消息>`（去掉控制字符、至多 200 字符）；应答里的长度超出上限是 `hysteria2: the server's answer is malformed`，没应答就结束了流是 `hysteria2: the server ended the stream without an answer`。**UDP**（自动，不需要参数）：每个 UDP 载体占一个会话号（每个出站从 1 递增），全锥；认证时服务端回 `Hysteria-UDP: false` 或它不支持 QUIC 数据报时，打开载体即失败（`hysteria2: the server does not allow UDP` / `hysteria2: the server does not support QUIC datagrams`），同一连接上的 TCP 照常；放不进一个 QUIC 数据报的包按 Hysteria 的分片发出（每片带完整的头，包号 1–65535 随机，至多 255 片；参考实现与 sing-box 的服务端不回超过约 4 KiB 的 UDP 包），255 片也装不下的包与名字长得一个数据报连头都放不下的包不发送（`hysteria2: a datagram longer than <n> bytes` / `hysteria2: the host name is too long for a datagram`，只丢那一个，流照常），收到的分片每个会话同一时刻只重组一个包、新包号到来就丢掉旧的、任一片丢失整个包丢弃；回包的来源可以是名字（服务端按客户端写的名字回；据 sing-box 文档 Surge 不接受这种回包，rurge 接受）；连接重建或收到 GOAWAY 后，载体在新连接上沿用同一会话号（服务端视为新会话，映射随之改变），连接正在结束时发出的数据报丢失、不重发。错误文本、日志与 `Debug` 不含口令、混淆口令与载荷；QUIC 层的错误形如 `quic: …`（如 `quic: the handshake timed out`、`quic: the server closed the connection (code 0x…)`），TLS 的形如 `tls: …`；quinn 自己的日志不输出，quinn-udp 关于系统拒发的包的告警保留。测速经出站本身（URL 测速的两次 `HEAD` 在同一条隧道里）；重载时参数没变的策略沿用出站与它的 QUIC 连接。混淆、端口跳跃与其余参数见 4.6；不能叠 Shadow TLS（4.4）。互操作参照：sing-box 1.14.2 的 `hysteria2` 入站（无混淆、Salamander、Gecko） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| 策略指向 rurge 尚未实现的协议类型 | Surge 原生支持全部协议 | 全部 | 🟡 | 1 / 2 | 加载时告警 `W0007`；运行时该策略按 `REJECT` 处理，会话日志 `error = policy protocol not implemented: <type>`；随阶段 2 各里程碑逐协议移除：M1 已移除 `http` `https` `socks5` `socks5-tls`；M2a 已移除 `trojan`；M2b 已移除 `vmess`（写了 `vmess-aead=true` 的行）与 `anytls`；M4a 已移除 `ssh`；M4b 已移除 `wireguard`；M4c 已移除 `external`；M6a 已移除 `ss`（流式旧方法除外）；M6b 已移除 `snell`（v4 / v5）；M6c 已移除 `h2-connect` 与 `trust-tunnel`（HTTP/2；`h3=true` 是 `W0029`，不是 `W0007`）。例外有三个：没写 `vmess-aead=true` 的 `vmess` 行仍按 `W0007` 处理，但走专门的诊断文本 `` `vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet ``（每次加载一条，不是每行一条）与专门的会话日志文本 `policy protocol not implemented: vmess (legacy handshake)`；流式旧方法的 `ss` 行同样如此，诊断文本 `` `ss` stream cipher `<method>` is not implemented yet; such policies behave as REJECT ``（每种方法每次加载一条），会话日志 `policy protocol not implemented: ss (<method>)`；`version` 为 1–3 或 6（含没写 `version` 的，Surge 的缺省是 1）的 `snell` 行也是，诊断文本 `` `snell` version <n> is not implemented; rurge supports versions 4 and 5 (`version` must match the server); such policies behave as REJECT ``（每个版本每次加载一条），会话日志 `policy protocol not implemented: snell v<n>`；三者都不是这里的通用 `<type>` 模板 |
```

换成

```markdown
| 策略指向 rurge 尚未实现的协议类型 | Surge 原生支持全部协议 | 全部 | 🟡 | 1 / 2 | 加载时告警 `W0007`；运行时该策略按 `REJECT` 处理，会话日志 `error = policy protocol not implemented: <type>`；随阶段 2 各里程碑逐协议移除：M1 已移除 `http` `https` `socks5` `socks5-tls`；M2a 已移除 `trojan`；M2b 已移除 `vmess`（写了 `vmess-aead=true` 的行）与 `anytls`；M4a 已移除 `ssh`；M4b 已移除 `wireguard`；M4c 已移除 `external`；M6a 已移除 `ss`（流式旧方法除外）；M6b 已移除 `snell`（v4 / v5）；M6c 已移除 `h2-connect` 与 `trust-tunnel`（HTTP/2；`h3=true` 是 `W0029`，不是 `W0007`）；M7a 已移除 `hysteria2`。`W0007` 与 `OutboundError::Unsupported` 在 M8 退役（M7 细化设计 M7-D12：M7 之后仍有 `tailscale`、ss 流式旧方法、vmess 旧握手与 Snell v1–v3 / v6）。例外有三个：没写 `vmess-aead=true` 的 `vmess` 行仍按 `W0007` 处理，但走专门的诊断文本 `` `vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet ``（每次加载一条，不是每行一条）与专门的会话日志文本 `policy protocol not implemented: vmess (legacy handshake)`；流式旧方法的 `ss` 行同样如此，诊断文本 `` `ss` stream cipher `<method>` is not implemented yet; such policies behave as REJECT ``（每种方法每次加载一条），会话日志 `policy protocol not implemented: ss (<method>)`；`version` 为 1–3 或 6（含没写 `version` 的，Surge 的缺省是 1）的 `snell` 行也是，诊断文本 `` `snell` version <n> is not implemented; rurge supports versions 4 and 5 (`version` must match the server); such policies behave as REJECT ``（每个版本每次加载一条），会话日志 `policy protocol not implemented: snell v<n>`；三者都不是这里的通用 `<type>` 模板 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `tfo` | 布尔；默认 false | 🟡 | 2 | 解析并校验，`W0029`，三平台都不生效：`socket2` 0.6 没有 TFO 的安全封装，不为此引入 unsafe；有安全封装后再评估 |
| `tos` | 0–255 或 `0x` 十六进制；默认 0 | 🟡 | 2 | M1 已实现；Windows 上对 IPv6 不生效；有 underlying-proxy 时无效（socket 选项属于真正打开 socket 的那一跳）；M2a 起加载时报 W0028 |
| `ecn` | `auto` `on` `off`；QUIC 类协议默认开启，WireGuard/Tailscale 默认关闭 | 🟡 | 2 | 取决于所选 QUIC 库对 ECN 的支持；M1 解析并校验取值，`W0029`；M5 仍只解析（M5-D6），随 QUIC 族再议 |
```

换成

```markdown
| `tfo` | 布尔；默认 false | 🟡 | 2 | 解析并校验，`W0029`，三平台都不生效：`socket2` 0.6 没有 TFO 的安全封装，不为此引入 unsafe；有安全封装后再评估。QUIC 类（M7a 起 `hysteria2`）不开 TCP 连接，写了是 `W0028` 并忽略 |
| `tos` | 0–255 或 `0x` 十六进制；默认 0 | 🟡 | 2 | M1 已实现；Windows 上对 IPv6 不生效；有 underlying-proxy 时无效（socket 选项属于真正打开 socket 的那一跳）；M2a 起加载时报 W0028。QUIC 类（M7a）的直连 socket 同样带上它，代价是发出的包没有 ECN 标记、没有 GSO（见 `ecn` 一行） |
| `ecn` | `auto` `on` `off`；QUIC 类协议默认开启，WireGuard/Tailscale 默认关闭 | 🟡 | 2 | M1 解析并校验取值。M7a 起在 `hysteria2` 上生效（M7-D4，跟随 quinn）：`auto` 与 `on` 都取 quinn 的缺省——给发出的包标 ECT(0)，对端的 ACK 不带 ECN 计数或计数校验失败时自行停用；quinn 没有"强制开启"，**`on` 等同 `auto`**；`off` 在 UDP 层去掉发出与收到两侧的 ECN 标记，quinn 的 ECN 校验随之停用。**写了 `tos`（非 0）时**直连的 QUIC socket 发送不经 quinn-udp（它在 Linux / macOS 上给每个包写的 TOS 只有 ECN 两位，会把 DSCP 清零），所以**发出的包没有 ECN 标记、也没有 GSO**；接收照常经 quinn-udp（读对端的 ECN 标记，照常做路径 MTU 探测）；三个平台一样。经 `underlying-proxy` 的载体没有 ECN。TCP 类协议与 `wireguard` 上仍只解析（`W0029`；手册说它对纯 TCP 协议不起作用） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `underlying-proxy` | 另一策略或策略组名；仅代理策略；不能与 `port-hopping` 同用 | ✅ | 2 | 目标代理主机名在上游远程解析；M1 已实现（TCP）：底层可以是策略或组，按名字在拨号时解析，组的选择变了链的入口随之变；成环是 `E0019`；链上某一跳失败时错误文本带 `via <名字>:` 前缀；中继是没有成员的组（订阅还没下载到、过滤滤光了）时，经它的连接一律失败（`via <组名>: policy group has no members`），不回退 DIRECT，也不看 `--empty-group-reject`（阶段 2 / M3b：设中继就是不让流量直出；Surge 未说明） |
```

换成

```markdown
| `underlying-proxy` | 另一策略或策略组名；仅代理策略；不能与 `port-hopping` 同用 | ✅ | 2 | 目标代理主机名在上游远程解析；M1 已实现（TCP）：底层可以是策略或组，按名字在拨号时解析，组的选择变了链的入口随之变；成环是 `E0019`；链上某一跳失败时错误文本带 `via <名字>:` 前缀；中继是没有成员的组（订阅还没下载到、过滤滤光了）时，经它的连接一律失败（`via <组名>: policy group has no members`），不回退 DIRECT，也不看 `--empty-group-reject`（阶段 2 / M3b：设中继就是不让流量直出；Surge 未说明）。QUIC 类（M7a 起 `hysteria2`）经底层策略的 UDP 载体连服务器（底层策略不载 UDP 时拨号失败，同 M5c；这种载体不做路径 MTU 探测，QUIC 包停在 1200 字节，也没有 ECN）；与 `port-hopping` 同写是 `E0018`（`` `port-hopping` cannot be combined with `underlying-proxy` ``；写 `underlying-proxy=DIRECT` 不算链，写组名算）；组级 `underlying-proxy` 派生的 `M (via R)` 里，成员 `M` 写了 `port-hopping` 时这个派生成员构建失败、从组里略去（日志 `policy cannot be built; it is left out`，同 M3a 里其它构建不了的派生成员），组照常用其余成员 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `alpn` | 协议列表；TUIC / Hysteria 2 / MASQUE 默认 `h3` | ✅ | 2 | M1 已实现：`https` / `socks5-tls` 握手不带 ALPN，除非写了 `alpn`（未与真实 Surge 核对）；M6c：`h2-connect` / `trust-tunnel` 固定 `h2`，写了别的取值 `W0028` 并忽略 |
```

换成

```markdown
| `alpn` | 协议列表；TUIC / Hysteria 2 / MASQUE 默认 `h3` | ✅ | 2 | M1 已实现：`https` / `socks5-tls` 握手不带 ALPN，除非写了 `alpn`（未与真实 Surge 核对）；M6c：`h2-connect` / `trust-tunnel` 固定 `h2`，写了别的取值 `W0028` 并忽略；M7a：`hysteria2` 没写 `alpn` 时发 `h3`（照手册），写了就发所写的列表；QUIC 的握手只用 TLS 1.3；服务端不选 ALPN 时握手失败——rustls 一类的服务端没有相同的协议时直接拒绝（`tls: the server refused the handshake (NoApplicationProtocol)`），完成握手却不选协议的服务端同样被拒（登记的差异） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| 约束：Shadow TLS 不能与 TUIC / WireGuard / Tailscale / 其他 QUIC 类协议组合 | 配置错误 | ✅ | 2 | 判断函数已落地（`tuic` `tuic-v5` `hysteria2` `masque` `wireguard` `tailscale`：`` E0018 Shadow TLS cannot be combined with a `<type>` policy ``）；`wireguard` 自 M4b 起真的报出，其余协议的 spec 出现之前该错误不会真的报出——它们的整行目前都还没有被读取 |
```

换成

```markdown
| 约束：Shadow TLS 不能与 TUIC / WireGuard / Tailscale / 其他 QUIC 类协议组合 | 配置错误 | ✅ | 2 | 判断函数已落地（`tuic` `tuic-v5` `hysteria2` `masque` `wireguard` `tailscale`：`` E0018 Shadow TLS cannot be combined with a `<type>` policy ``）；`wireguard` 自 M4b 起、`hysteria2` 自 M7a 起真的报出，`tuic` / `tuic-v5` / `masque` 的 spec 出现之前（M7b / M7c）该错误不会真的报出——它们的整行目前都还没有被读取；`h3=true` 的 `trust-tunnel` 眼下仍按 HTTP/2 连、照常可叠 Shadow TLS，M7c 让它走 QUIC 时须把它加进判断函数 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `udp-relay`（布尔；默认 false） | 适用 SOCKS5 / SOCKS5-TLS / Shadowsocks / External / HTTP/2 CONNECT（RFC 9298） | ✅ M5a：`socks5` / `socks5-tls` / `external` 已生效；M6a：Shadowsocks 已生效；M6c：HTTP/2 CONNECT（`h2-connect` 的 CONNECT-UDP，每个目标一条流，对称型；服务端须通告 extended CONNECT，见 4.2 `h2-connect` 一行）已生效；写在 `trust-tunnel` 行上是 `W0028` 并忽略 | 2 |
| `udp-port`（端口；默认主端口） | 适用 Shadowsocks / Snell | ✅ M6a：Shadowsocks 已生效（1–65535，其它取值 `E0018`；没写 `udp-relay=true` 时不用）；M6b：Snell 只解析——写了是 `W0029`，UDP over TCP 一律走主端口（手册用它指服务端另开的 UDP 端口，即 v5 的 QUIC Proxy Mode，rurge 不实现；见 4.2 `snell` 一行） | 2 |
| 自动支持 UDP 的协议：Snell v3+、VMess、Trojan、TUIC、Hysteria 2、MASQUE、AnyTLS（UDP over TCP）、WireGuard、Tailscale | | ✅ M5b：VMess（对称型）、Trojan、AnyTLS 已生效；M5c：WireGuard 已生效；M6b：Snell v4 / v5 已生效（UDP over TCP，全锥；v1–v3 在 M8 之前不实现）；其余随各自的协议 | 2 |
```

换成

```markdown
| `udp-relay`（布尔；默认 false） | 适用 SOCKS5 / SOCKS5-TLS / Shadowsocks / External / HTTP/2 CONNECT（RFC 9298） | ✅ M5a：`socks5` / `socks5-tls` / `external` 已生效；M6a：Shadowsocks 已生效；M6c：HTTP/2 CONNECT（`h2-connect` 的 CONNECT-UDP，每个目标一条流，对称型；服务端须通告 extended CONNECT，见 4.2 `h2-connect` 一行）已生效；写在 `trust-tunnel` 行上是 `W0028` 并忽略；写在 `hysteria2` 行上同样是 `W0028`（UDP 自动支持，M7a） | 2 |
| `udp-port`（端口；默认主端口） | 适用 Shadowsocks / Snell | ✅ M6a：Shadowsocks 已生效（1–65535，其它取值 `E0018`；没写 `udp-relay=true` 时不用）；M6b：Snell 只解析——写了是 `W0029`，UDP over TCP 一律走主端口（手册用它指服务端另开的 UDP 端口，即 v5 的 QUIC Proxy Mode，rurge 不实现；见 4.2 `snell` 一行） | 2 |
| 自动支持 UDP 的协议：Snell v3+、VMess、Trojan、TUIC、Hysteria 2、MASQUE、AnyTLS（UDP over TCP）、WireGuard、Tailscale | | ✅ M5b：VMess（对称型）、Trojan、AnyTLS 已生效；M5c：WireGuard 已生效；M6b：Snell v4 / v5 已生效（UDP over TCP，全锥；v1–v3 在 M8 之前不实现）；M7a：Hysteria 2 已生效（QUIC 数据报，全锥；服务端在认证时回 `Hysteria-UDP: false` 时打开载体即失败，见 4.2 `hysteria2` 一行）；其余随各自的协议 | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `hysteria2` | `password` `download-bandwidth`（Mbps）`port-hopping` `port-hopping-interval` `salamander-password` | ✅ | 2 | |
| `hysteria2` | `gecko-password` | ❓ | 2 | 混淆算法细节待确认 |
```

换成

```markdown
| `hysteria2` | `password` `download-bandwidth`（Mbps）`port-hopping`（`1234;5000-6000`）`port-hopping-interval`（默认 30） | ✅ | 2 | M7a 已实现。`password` 只读命名写法、必填（`E0018` `` `password` is required ``，空值算缺，不引用取值；写在端口之后的位置值是 `W0001`）；含 TAB 以外控制字符的口令放不进 HTTP 字段，构建失败 `` `password` cannot be sent in an HTTP field ``（同样不引用取值）。`download-bandwidth` 是非负整数 Mbps（小数、负数、带单位都是 `E0018`），只经 `Hysteria-CC-RX` 上报给服务端（× 125000 字节/秒；没写为 0，即"未知"），服务端据此定它发往 rurge 的速率；客户端自己的拥塞控制仍是 Cubic，不实现 Brutal（M7-D10）。`port-hopping`：分号分隔的端口与闭区间（端口 1–65535，区间两端可以相等，各项前后与 `-` 两边可有空格），写错（空值、空项、端口 0 或超过 65535、区间颠倒等）是 `E0018` 并引用取值；写了它，声明里的主端口不用（照手册）：每条连接的第一个端口在端口集合里均匀随机取，之后每个间隔换到集合里**另一个**随机端口（参考实现可能连着取到同一个；重叠或相邻的区间合并，每个端口只算一次），同时新开一个本机 socket，上一个保留到下一次跳跃、其间照收迟到的包；新 socket 开不出时跳过这一次；服务器名每条连接只解析一次。`port-hopping-interval` 是整秒数，缺省 30，**小于 5 时 `W0012` 并按 5**（参考实现直接报错，手册没写下限），不是整数是 `E0018`，没写 `port-hopping` 时 `W0028` 并忽略。`port-hopping` 与 `underlying-proxy` 同写是 `E0018`（照手册；见 4.3）。`udp-relay` 与 `tfo` 写了是 `W0028` 并忽略（UDP 自动支持；QUIC 不开 TCP 连接）；`ecn` 生效（见 4.3）；TLS 参数适用（4.4，`alpn` 缺省 `h3`）；不能叠 Shadow TLS（`E0018`） |
| `hysteria2` | `salamander-password` `gecko-password`（二者互斥） | ✅ | 2 | M7a 已实现。两个口令都不能为空（`E0018` `` `<参数>` is empty ``），同时写是 `E0018`（照手册）；都在 `profiles/current` 与 `policies/detail` 里脱敏，错误文本、日志与 `Debug` 里没有它们；不设最短长度（参考实现要求至少 4 字节，这样的服务端不存在，互通不受影响）。混淆层套在 UDP socket 的最外面（端口跳跃与 `underlying-proxy` 的载体之上），每条连接各有一份状态。**Salamander**：每个 UDP 包前加 8 字节随机 salt，`key = BLAKE2b-256(口令 ‖ salt)`（输出长度参数为 32 的 BLAKE2b，与 Go 的 `blake2b.Sum256` 相同，不是截短的 BLAKE2b-512；用 RFC 7693 的自测与已知答案核对过），负载与 key 循环异或；不超过 8 字节的包丢弃。**Gecko**：按 Hysteria 官网的公开规范实现，**与 Surge 的 `gecko-password` 是否同一算法未证实**（M7-D11；名称与 Surge 的更新说明强烈暗示是，手工验收里核对）。短头的包（首字节最高位为 0）只做 Salamander（口令取 `gecko-password`）；长头的握手包一律随机拆成 2–8 片，每片 5 字节头（`0x80 ‖ 消息号 ‖ 片号 << 4 \| 片数 ‖ u16 填充长度`）加随机填充，使整个 UDP 包落在 512–1200 字节（参考实现与 sing-box 的缺省；Surge 的取值未公开），分片本身超过上限时不填充，每片各做一次 Salamander；收到时按消息号重组，同时至多 8 个包在途、第 9 个挤掉最旧的（参考实现对同一来源拒收新的），8 秒过期，重复的片与片数不一致的片丢弃。协议没有完整性校验：混淆口令不对（或只有一端开了混淆）时服务端收到的只是乱码，表现为请求在连接超时处失败（`connect timed out`），没有专门的错误。互操作参照：sing-box 1.14.2 的 `hysteria2` 入站（`obfs.type` 为 `salamander` / `gecko`） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `GET /v1/profiles/current?sensitive=0` | 当前配置文本（可脱敏） | 全部 | ✅ | 1 | M4a 已实现；`sensitive=0`（默认）脱敏：独立密钥行 `password` / `ca-passphrase` / `ca-p12` / `private-key` / `psk` / `pre-shared-key` / `token`；内联参数 `password` / `psk` / `private-key` / `pre-shared-key` / `base64` / `token` / `uuid` / `username` / `headers` / `ws-headers` / `ws-path` / `shadow-tls-password` / `policy-path` / `external-policy-modifier`（后两个自 M3a 起：订阅链接常带 token，修饰列表能设任何参数）/ `test-url`（自 M3b 起：订阅行设的测试 URL 可能带 token）/ `args`（自 M4c 起：`external` 的参数里常带口令）/ `obfs-host`（自 M6a 起：伪装域名可能是用户的指纹）/ `obfs-uri`（自 M6 合并后的复查起：前面按路径分流时路径就是共享密钥，同 `ws-path`）；`http-api` / `external-controller-access` / `http-listen` / `socks5-listen` 的 `key@` 前缀与 `wifi-access-http-auth` 口令；所有写作 `type, server, port` 的代理类型（`http` `https` `h2-connect` `socks5` `socks5-tls` `ss` `snell` `vmess` `trojan` `tuic` `tuic-v5` `hysteria2` `masque` `anytls` `trust-tunnel` `ssh`）策略行第 4 个起、凡不是 `name=value` 具名参数的 token（最常见的是根本不含 `=` 的裸 token，即位置传递的凭据；含 `=` 但值为空或全是 `=` 的也算，例如带填充的 base64。除 `http` `https` `socks5` `socks5-tls` 与 `h2-connect` 外这个位置本就是多余参数 `W0001`，按偏安全一侧抹掉）。取值的结尾按解析器自己的规则找**第一个顶层逗号**：`"` 或 `'` 在值里任何位置都会开启一段引号（`"` 内 `\` 转义下一个字符），`(` / `)` 分组，引号内与括号内的逗号都属于值（`password="p,w"`、`password=ab"c,d"`、`password=a(b,c)d` 各是一整个值）；引号或括号未闭合时抹到行尾。名单以外不脱敏；其余内容与行数、行尾 CRLF 原样保留 |
```

换成

```markdown
| `GET /v1/profiles/current?sensitive=0` | 当前配置文本（可脱敏） | 全部 | ✅ | 1 | M4a 已实现；`sensitive=0`（默认）脱敏：独立密钥行 `password` / `ca-passphrase` / `ca-p12` / `private-key` / `psk` / `pre-shared-key` / `token`；内联参数 `password` / `psk` / `private-key` / `pre-shared-key` / `base64` / `token` / `uuid` / `username` / `headers` / `ws-headers` / `ws-path` / `shadow-tls-password` / `salamander-password` / `gecko-password`（自 M7a 起：Hysteria 2 的混淆口令；`password` 只按整个参数名匹配，所以它们单列）/ `policy-path` / `external-policy-modifier`（后两个自 M3a 起：订阅链接常带 token，修饰列表能设任何参数）/ `test-url`（自 M3b 起：订阅行设的测试 URL 可能带 token）/ `args`（自 M4c 起：`external` 的参数里常带口令）/ `obfs-host`（自 M6a 起：伪装域名可能是用户的指纹）/ `obfs-uri`（自 M6 合并后的复查起：前面按路径分流时路径就是共享密钥，同 `ws-path`）；`http-api` / `external-controller-access` / `http-listen` / `socks5-listen` 的 `key@` 前缀与 `wifi-access-http-auth` 口令；所有写作 `type, server, port` 的代理类型（`http` `https` `h2-connect` `socks5` `socks5-tls` `ss` `snell` `vmess` `trojan` `tuic` `tuic-v5` `hysteria2` `masque` `anytls` `trust-tunnel` `ssh`）策略行第 4 个起、凡不是 `name=value` 具名参数的 token（最常见的是根本不含 `=` 的裸 token，即位置传递的凭据；含 `=` 但值为空或全是 `=` 的也算，例如带填充的 base64。除 `http` `https` `socks5` `socks5-tls` 与 `h2-connect` 外这个位置本就是多余参数 `W0001`，按偏安全一侧抹掉）。取值的结尾按解析器自己的规则找**第一个顶层逗号**：`"` 或 `'` 在值里任何位置都会开启一段引号（`"` 内 `\` 转义下一个字符），`(` / `)` 分组，引号内与括号内的逗号都属于值（`password="p,w"`、`password=ab"c,d"`、`password=a(b,c)d` 各是一整个值）；引号或括号未闭合时抹到行尾。名单以外不脱敏；其余内容与行数、行尾 CRLF 原样保留 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| 4. 出站策略 | 85 | 75 | 6 | 1 | 0 | 3 |
```

换成

```markdown
| 4. 出站策略 | 85 | 76 | 6 | 1 | 0 | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| **合计** | **529** | **423** | **59** | **29** | **10** | **8** |
```

换成

```markdown
| **合计** | **529** | **424** | **59** | **29** | **10** | **7** |
```

`docs/acceptance/phase2-manual.md`——把

```markdown
- [ ] 脱敏：`GET /v1/policies/detail?policy_name=<策略名>` 与 `GET /v1/profiles/current` 里两种策略的用户名、口令（含 `h2-connect` 写在端口之后的位置值）与 `headers` 都是 `***`。

```

换成

```markdown
- [ ] 脱敏：`GET /v1/policies/detail?policy_name=<策略名>` 与 `GET /v1/profiles/current` 里两种策略的用户名、口令（含 `h2-connect` 写在端口之后的位置值）与 `headers` 都是 `***`。

## M7a　Hysteria 2

前置：同 M5a 一节的 SOCKS5 UDP 客户端；自己的 Hysteria 2 节点（记下服务端实现与版本，如官方 hysteria 或 sing-box 的 `hysteria2` 入站），以及同一服务端配 Salamander 与配 Gecko 的两份变体（Gecko 要服务端支持：官方 hysteria 2.9.2 起，标为实验性；sing-box 1.14.0 起）；Surge（Mac 6.7 / iOS 5.20 以上，Gecko 一项用）；一个开了 UDP 的上游 SOCKS5 节点（`udp-relay=true`，`underlying-proxy` 一项用）。策略行照 Surge 的写法：`H = hysteria2, <服务器>, <端口>, password=<口令>`（证书不是公共 CA 签的时另加 `server-cert-fingerprint-sha256=` 或 `skip-cert-verify=true`）。每份配置 `[Rule]` 里 `FINAL,H`，`rurge check -c <配置>` 零错误（没有 `W0007`）。

- [ ] TCP：`curl -x http://127.0.0.1:<http-listen 端口> https://example.com/ -I` 与 `curl --socks5-hostname 127.0.0.1:<socks5-listen 端口> https://example.com/ -I` 都返回 200，请求记录里策略链是 `H`、`error` 为空；同时打开几个网站，服务端日志（或抓包）只看到一条 QUIC 连接、一次认证。下载一个几百 MiB 的文件，速度与官方客户端在同一量级，记下数字；加上 `download-bandwidth=<本机实际的下行 Mbps>` 后再下一次，记下数字（服务端据此按这个速率发；rurge 自己的拥塞控制仍是 Cubic）。
- [ ] UDP：不需要参数，经 SOCKS5 UDP 发 DNS 查询（如 Proxifier 代理 `nslookup example.com 8.8.8.8`）得到回答；经它进行一次语音通话或联机游戏；用 NAT 类型检测工具（STUN）检测，结果是 Full Cone（节点的出口须是全锥）。
- [ ] Salamander：`salamander-password=<混淆口令>` 连配了 Salamander 的服务端，TCP 与 UDP 都能往返；抓包（Wireshark）认不出 QUIC。把混淆口令改错一位后重载：请求在连接超时处失败（`connect timed out`），没有更具体的错误（协议没有完整性校验）。
- [ ] Gecko：`gecko-password=<混淆口令>` 连配了 Gecko 的服务端，TCP 与 UDP 都能往返；抓包看到连接刚建立时发出的几个 UDP 包都在 512–1200 字节之间（握手包被拆成 2–8 片并填充）。**同一行策略原样放进 Surge 的配置、连同一个服务端**：rurge 与 Surge 都连得上，才说明 rurge 的 Gecko（照 Hysteria 官网的公开规范）就是 Surge 的 `gecko-password`——这一点尚未证实（M7-D11）。记下结果；只有一边连得上时记下服务端实现与版本，在兼容性清单登记差异。
- [ ] 端口跳跃：服务端把一段 UDP 端口转发到它的监听端口（官方 hysteria 在 Linux 上把 `listen` 写成端口区间即可，它自己设 nftables / iptables 规则；其它服务端手工加 DNAT / REDIRECT 规则），策略写 `port-hopping=<端口区间>, port-hopping-interval=10`：经 `H` 的下载进行中，抓包看到发往服务端的目的端口约每 10 秒换一个（每次换到另一个端口）、下载不中断；`port-hopping-interval=2` 时 `rurge check` 报一条 `W0012`（按 5 秒）。
- [ ] `underlying-proxy`：策略写 `underlying-proxy=<SOCKS5 节点>`，TCP 与 UDP 都能往返，SOCKS5 节点那边看到的是一条 UDP 关联（QUIC 经它的 UDP 走），没有 TCP 的 CONNECT；同时写了 `port-hopping` 时 `rurge check` 报 `E0018`（`` `port-hopping` cannot be combined with `underlying-proxy` ``）；把底层策略换成一条 `http` 策略后，拨号失败，请求记录写 `via <底层策略>: the underlying policy cannot carry UDP`。
- [ ] 休眠唤醒与换网：经 `H` 的连接在用时（保持一个下载或反复访问网站），让电脑休眠再唤醒，或在 Wi-Fi 与有线 / 手机热点之间切换：约 30 秒内（QUIC 的空闲超时）之后的请求走新建的连接、恢复正常，不必重启 rurge；其间的请求可能超时失败。
- [ ] 口令错误：把 `password` 改错一位，重载后访问 `https://example.com/`：请求失败，会话记录的 `error` 是 `hysteria2: authentication failed`；经 SOCKS5 UDP 的 DNS 查询同样失败、记录写同一个错误；**错误文本与日志（含 `--log-level verbose`）里都没有口令与混淆口令**。
- [ ] 服务端不转发 UDP（官方 hysteria 的 `disableUDP: true`，认证应答里 `Hysteria-UDP: false`）：TCP 照常；经 SOCKS5 UDP 的查询失败，记录写 `hysteria2: the server does not allow UDP`。
- [ ] 脱敏：`GET /v1/policies/detail?policy_name=H` 与 `GET /v1/profiles/current` 里 `password`、`salamander-password` 与 `gecko-password` 都是 `***`。

```

`README.md`——把

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；M4c（external）已完成——`external` 策略（TCP；第一次用到时拉起外部程序、经它在本机端口上的 SOCKS5 转发；程序退出后下次用到时再拉起；输出写进数据目录；rurge 退出时连同它启动的进程一起停掉——Unix 用进程组，Windows 用 Job Object；三平台都支持，Surge 只在 Mac 上有）已可用，订阅导入的 `external` 一律跳过；M5a（UDP 地基）已完成——SOCKS5 监听支持 UDP ASSOCIATE，UDP 按规则分流到 DIRECT、REJECT 或 `socks5` / `socks5-tls` / `external`（`udp-relay=true`，含 `underlying-proxy` 链），全锥 NAT，每条 UDP 流一条请求记录，`block-quic` 与 `udp-policy-not-supported-behaviour` 生效；M5b（TLS 族的 UDP）已完成——`trojan`（UDP ASSOCIATE）、`anytls`（UDP over TCP v2）全锥，`vmess`（命令 2，每个目标一条连接）对称型；M5c（WireGuard 的 UDP 与其余）已完成——`wireguard` 的 UDP（全锥）与经 `underlying-proxy` 的隧道、`test-udp` / `proxy-test-udp`、`smart` 组计入 UDP、`dns-follow-interface`；M6（Shadowsocks / Snell / HTTP/2 族）已完成：M6a（Shadowsocks）已完成——`ss` 的 AEAD、`none` 与 SS 2022（含多用户身份头）方法、simple-obfs（`http` / `tls`），TCP 与 UDP（`udp-relay=true`、`udp-port`，全锥），流式旧方法在 M8 之前按 REJECT 处理；M6b（Snell）已完成——`snell` v4 / v5 的 TCP 与 UDP（UDP over TCP，全锥）、`reuse`（连接回池给下一个请求用）、simple-obfs `http`，v1–v3 与 v6 按 REJECT 处理（没写 `version` 即 v1）；M6c（HTTP/2 族）已完成——`h2-connect`（HTTP/2 CONNECT，多条隧道共用连接、`max-streams`；`udp-relay=true` 时经 CONNECT-UDP（RFC 9298）转发 UDP，每个目标一条流、对称型）与 `trust-tunnel`（HTTP/2，TCP；`h3=true` 在 M7 之前仍按 HTTP/2 连）；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

换成

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；M4c（external）已完成——`external` 策略（TCP；第一次用到时拉起外部程序、经它在本机端口上的 SOCKS5 转发；程序退出后下次用到时再拉起；输出写进数据目录；rurge 退出时连同它启动的进程一起停掉——Unix 用进程组，Windows 用 Job Object；三平台都支持，Surge 只在 Mac 上有）已可用，订阅导入的 `external` 一律跳过；M5a（UDP 地基）已完成——SOCKS5 监听支持 UDP ASSOCIATE，UDP 按规则分流到 DIRECT、REJECT 或 `socks5` / `socks5-tls` / `external`（`udp-relay=true`，含 `underlying-proxy` 链），全锥 NAT，每条 UDP 流一条请求记录，`block-quic` 与 `udp-policy-not-supported-behaviour` 生效；M5b（TLS 族的 UDP）已完成——`trojan`（UDP ASSOCIATE）、`anytls`（UDP over TCP v2）全锥，`vmess`（命令 2，每个目标一条连接）对称型；M5c（WireGuard 的 UDP 与其余）已完成——`wireguard` 的 UDP（全锥）与经 `underlying-proxy` 的隧道、`test-udp` / `proxy-test-udp`、`smart` 组计入 UDP、`dns-follow-interface`；M6（Shadowsocks / Snell / HTTP/2 族）已完成：M6a（Shadowsocks）已完成——`ss` 的 AEAD、`none` 与 SS 2022（含多用户身份头）方法、simple-obfs（`http` / `tls`），TCP 与 UDP（`udp-relay=true`、`udp-port`，全锥），流式旧方法在 M8 之前按 REJECT 处理；M6b（Snell）已完成——`snell` v4 / v5 的 TCP 与 UDP（UDP over TCP，全锥）、`reuse`（连接回池给下一个请求用）、simple-obfs `http`，v1–v3 与 v6 按 REJECT 处理（没写 `version` 即 v1）；M6c（HTTP/2 族）已完成——`h2-connect`（HTTP/2 CONNECT，多条隧道共用连接、`max-streams`；`udp-relay=true` 时经 CONNECT-UDP（RFC 9298）转发 UDP，每个目标一条流、对称型）与 `trust-tunnel`（HTTP/2，TCP；`h3=true` 在 M7 之前仍按 HTTP/2 连）；M7（QUIC 族）进行中：M7a（QUIC 地基与 Hysteria 2）已完成——`hysteria2` 的 TCP 与 UDP（QUIC 数据报，全锥）、Salamander 与 Gecko 混淆、`port-hopping`、`download-bandwidth`（只上报给服务端）与 `ecn`，可经 `underlying-proxy` 连出去；M7b（TUIC）与 M7c（MASQUE、`trust-tunnel` 的 HTTP/3、DoQ / DoH3）在后；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

`README.md`——把

```markdown
| 出站协议       | DIRECT / REJECT 系列 / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / 外部程序（DIRECT 与 REJECT 系四种已实现，M3a；HTTP（TCP）与 SOCKS5（含 `socks5-tls`，TCP 与 UDP）上游及 `underlying-proxy` 链已实现，阶段 2 / M1、M5a；Trojan（TCP 与 UDP；WebSocket 传输）已实现，阶段 2 / M2a、M5b；VMess（AEAD 握手，TCP 与 UDP；可叠加 TLS / WebSocket）与 AnyTLS（TCP 与 UDP，会话复用）已实现，阶段 2 / M2b、M5b；SSH（TCP，会话复用）已实现，阶段 2 / M4a；WireGuard（TCP 与 UDP，用户态隧道）已实现，阶段 2 / M4b、M5c；外部程序（TCP 与 UDP，三平台）已实现，阶段 2 / M4c、M5a；Shadowsocks（AEAD / 2022、obfs，TCP 与 UDP）已实现，阶段 2 / M6a；Snell v4 / v5（TCP 与 UDP、`reuse`、obfs `http`）已实现，阶段 2 / M6b；HTTP/2 CONNECT（`h2-connect`，TCP 与 CONNECT-UDP）与 Trust Tunnel（HTTP/2，TCP）已实现，阶段 2 / M6c） | 2     |
```

换成

```markdown
| 出站协议       | DIRECT / REJECT 系列 / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / 外部程序（DIRECT 与 REJECT 系四种已实现，M3a；HTTP（TCP）与 SOCKS5（含 `socks5-tls`，TCP 与 UDP）上游及 `underlying-proxy` 链已实现，阶段 2 / M1、M5a；Trojan（TCP 与 UDP；WebSocket 传输）已实现，阶段 2 / M2a、M5b；VMess（AEAD 握手，TCP 与 UDP；可叠加 TLS / WebSocket）与 AnyTLS（TCP 与 UDP，会话复用）已实现，阶段 2 / M2b、M5b；SSH（TCP，会话复用）已实现，阶段 2 / M4a；WireGuard（TCP 与 UDP，用户态隧道）已实现，阶段 2 / M4b、M5c；外部程序（TCP 与 UDP，三平台）已实现，阶段 2 / M4c、M5a；Shadowsocks（AEAD / 2022、obfs，TCP 与 UDP）已实现，阶段 2 / M6a；Snell v4 / v5（TCP 与 UDP、`reuse`、obfs `http`）已实现，阶段 2 / M6b；HTTP/2 CONNECT（`h2-connect`，TCP 与 CONNECT-UDP）与 Trust Tunnel（HTTP/2，TCP）已实现，阶段 2 / M6c；Hysteria 2（TCP 与 UDP、Salamander / Gecko 混淆、端口跳跃）已实现，阶段 2 / M7a） | 2     |
```

`README.md`——把

```markdown
3. **阶段 2** 出站协议全集、策略组、策略订阅（进行中：M1 出站地基与 HTTP / SOCKS5 上游已完成；M2a（Trojan，TCP + WebSocket）已完成；M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成；M3a（成员装配与订阅）已完成；M3b（测速与自动组）已完成；M3c（`smart`）已完成；M4a（SSH）已完成；M4b（WireGuard）已完成；M4c（external）已完成；M5a（UDP 地基）已完成；M5b（TLS 族的 UDP）已完成；M5c（WireGuard 的 UDP 与其余）已完成；M6a（Shadowsocks）已完成；M6b（Snell）已完成；M6c（HTTP/2 族）已完成，M6 至此完成）
```

换成

```markdown
3. **阶段 2** 出站协议全集、策略组、策略订阅（进行中：M1 出站地基与 HTTP / SOCKS5 上游已完成；M2a（Trojan，TCP + WebSocket）已完成；M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成；M3a（成员装配与订阅）已完成；M3b（测速与自动组）已完成；M3c（`smart`）已完成；M4a（SSH）已完成；M4b（WireGuard）已完成；M4c（external）已完成；M5a（UDP 地基）已完成；M5b（TLS 族的 UDP）已完成；M5c（WireGuard 的 UDP 与其余）已完成；M6a（Shadowsocks）已完成；M6b（Snell）已完成；M6c（HTTP/2 族）已完成，M6 至此完成；M7a（QUIC 地基与 Hysteria 2）已完成，M7b（TUIC）与 M7c（HTTP/3 族与 DoQ / DoH3）在后）
```

`README_en.md`——把

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; M4b (WireGuard) is done — the `wireguard` policy (TCP; a user-space tunnel, no virtual interface; several peers chosen by `allowed-ips`; `client-id` (WARP's reserved bytes); destination names looked up through the tunnel when the section has a `dns-server`, on this machine otherwise; tested by a handshake when there is neither a `dns-server` nor a `test-url`) is usable; M4c (external) is done — the `external` policy (TCP; the program is started on first use and reached as a SOCKS5 proxy on its local port; a program that exited is started again on the next use; its output goes to the data directory; when rurge exits, the program is stopped together with whatever it started — a process group on Unix, a Job Object on Windows; all three platforms, where Surge has it on the Mac only) is usable, and subscriptions never bring `external` policies in; M5a (the UDP foundation) is done — the SOCKS5 listener takes UDP ASSOCIATE, and UDP is routed by rule to DIRECT, REJECT or `socks5` / `socks5-tls` / `external` (`udp-relay=true`, `underlying-proxy` chains included), full-cone NAT, one request record per UDP flow, and `block-quic` and `udp-policy-not-supported-behaviour` take effect; M5b (UDP over the TLS family) is done — `trojan` (UDP ASSOCIATE) and `anytls` (UDP over TCP v2) with full-cone NAT, `vmess` (command 2, one connection per target) symmetric; M5c (UDP over WireGuard and the rest) is done — UDP over `wireguard` (full cone) and tunnels over an `underlying-proxy`, `test-udp` / `proxy-test-udp`, UDP in `smart` groups, and `dns-follow-interface`; M6 (Shadowsocks / Snell / the HTTP/2 family) is done: M6a (Shadowsocks) is done — `ss` with the AEAD, `none` and SS 2022 (multi-user identity headers included) methods, simple-obfs (`http` / `tls`), TCP and UDP (`udp-relay=true`, `udp-port`, full cone), with the legacy stream ciphers behaving as REJECT until M8; M6b (Snell) is done — `snell` v4 / v5 over TCP and UDP (UDP over TCP, full cone), `reuse` (a connection goes back to a pool for the next request) and simple-obfs `http`, with v1–v3 and v6 behaving as REJECT (a line without `version` is v1); M6c (the HTTP/2 family) is done — `h2-connect` (HTTP/2 CONNECT, tunnels sharing connections up to `max-streams`; with `udp-relay=true`, UDP over CONNECT-UDP (RFC 9298), one stream per target, symmetric) and `trust-tunnel` (HTTP/2, TCP; `h3=true` still connects over HTTP/2 until M7); the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

换成

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; M4b (WireGuard) is done — the `wireguard` policy (TCP; a user-space tunnel, no virtual interface; several peers chosen by `allowed-ips`; `client-id` (WARP's reserved bytes); destination names looked up through the tunnel when the section has a `dns-server`, on this machine otherwise; tested by a handshake when there is neither a `dns-server` nor a `test-url`) is usable; M4c (external) is done — the `external` policy (TCP; the program is started on first use and reached as a SOCKS5 proxy on its local port; a program that exited is started again on the next use; its output goes to the data directory; when rurge exits, the program is stopped together with whatever it started — a process group on Unix, a Job Object on Windows; all three platforms, where Surge has it on the Mac only) is usable, and subscriptions never bring `external` policies in; M5a (the UDP foundation) is done — the SOCKS5 listener takes UDP ASSOCIATE, and UDP is routed by rule to DIRECT, REJECT or `socks5` / `socks5-tls` / `external` (`udp-relay=true`, `underlying-proxy` chains included), full-cone NAT, one request record per UDP flow, and `block-quic` and `udp-policy-not-supported-behaviour` take effect; M5b (UDP over the TLS family) is done — `trojan` (UDP ASSOCIATE) and `anytls` (UDP over TCP v2) with full-cone NAT, `vmess` (command 2, one connection per target) symmetric; M5c (UDP over WireGuard and the rest) is done — UDP over `wireguard` (full cone) and tunnels over an `underlying-proxy`, `test-udp` / `proxy-test-udp`, UDP in `smart` groups, and `dns-follow-interface`; M6 (Shadowsocks / Snell / the HTTP/2 family) is done: M6a (Shadowsocks) is done — `ss` with the AEAD, `none` and SS 2022 (multi-user identity headers included) methods, simple-obfs (`http` / `tls`), TCP and UDP (`udp-relay=true`, `udp-port`, full cone), with the legacy stream ciphers behaving as REJECT until M8; M6b (Snell) is done — `snell` v4 / v5 over TCP and UDP (UDP over TCP, full cone), `reuse` (a connection goes back to a pool for the next request) and simple-obfs `http`, with v1–v3 and v6 behaving as REJECT (a line without `version` is v1); M6c (the HTTP/2 family) is done — `h2-connect` (HTTP/2 CONNECT, tunnels sharing connections up to `max-streams`; with `udp-relay=true`, UDP over CONNECT-UDP (RFC 9298), one stream per target, symmetric) and `trust-tunnel` (HTTP/2, TCP; `h3=true` still connects over HTTP/2 until M7); M7 (the QUIC family) is in progress: M7a (the QUIC foundation and Hysteria 2) is done — `hysteria2` over TCP and UDP (QUIC datagrams, full cone), Salamander and Gecko obfuscation, `port-hopping`, `download-bandwidth` (only reported to the server) and `ecn`, optionally through an `underlying-proxy`; M7b (TUIC) and M7c (MASQUE, `trust-tunnel` over HTTP/3, DoQ / DoH3) come next; the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

`README_en.md`——把

```markdown
| Outbound            | DIRECT / REJECT family / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / external program (DIRECT and the four REJECT flavours implemented, M3a; HTTP (TCP) and SOCKS5 (including `socks5-tls`, TCP and UDP) upstreams and `underlying-proxy` chains implemented, phase 2 / M1, M5a; Trojan (TCP and UDP; WebSocket transport) implemented, phase 2 / M2a, M5b; VMess (AEAD handshake, TCP and UDP; optionally TLS / WebSocket) and AnyTLS (TCP and UDP, session reuse) implemented, phase 2 / M2b, M5b; SSH (TCP, session reuse) implemented, phase 2 / M4a; WireGuard (TCP and UDP, user-space tunnel) implemented, phase 2 / M4b, M5c; external program (TCP and UDP, all three platforms) implemented, phase 2 / M4c, M5a; Shadowsocks (AEAD / 2022, obfs, TCP and UDP) implemented, phase 2 / M6a; Snell v4 / v5 (TCP and UDP, `reuse`, obfs `http`) implemented, phase 2 / M6b; HTTP/2 CONNECT (`h2-connect`, TCP and CONNECT-UDP) and Trust Tunnel (HTTP/2, TCP) implemented, phase 2 / M6c) | 2     |
```

换成

```markdown
| Outbound            | DIRECT / REJECT family / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / external program (DIRECT and the four REJECT flavours implemented, M3a; HTTP (TCP) and SOCKS5 (including `socks5-tls`, TCP and UDP) upstreams and `underlying-proxy` chains implemented, phase 2 / M1, M5a; Trojan (TCP and UDP; WebSocket transport) implemented, phase 2 / M2a, M5b; VMess (AEAD handshake, TCP and UDP; optionally TLS / WebSocket) and AnyTLS (TCP and UDP, session reuse) implemented, phase 2 / M2b, M5b; SSH (TCP, session reuse) implemented, phase 2 / M4a; WireGuard (TCP and UDP, user-space tunnel) implemented, phase 2 / M4b, M5c; external program (TCP and UDP, all three platforms) implemented, phase 2 / M4c, M5a; Shadowsocks (AEAD / 2022, obfs, TCP and UDP) implemented, phase 2 / M6a; Snell v4 / v5 (TCP and UDP, `reuse`, obfs `http`) implemented, phase 2 / M6b; HTTP/2 CONNECT (`h2-connect`, TCP and CONNECT-UDP) and Trust Tunnel (HTTP/2, TCP) implemented, phase 2 / M6c; Hysteria 2 (TCP and UDP, Salamander / Gecko obfuscation, port hopping) implemented, phase 2 / M7a) | 2     |
```

`README_en.md`——把

```markdown
3. **Phase 2** All outbound protocols, policy groups, subscriptions (in progress: M1, the outbound foundation and HTTP / SOCKS5 upstreams, is done; M2a, Trojan (TCP + WebSocket), is done; M2b, VMess / AnyTLS, is done; M2c, Shadow TLS, is done; M3a, member assembly and subscriptions, is done; M3b, connectivity tests and automatic groups, is done; M3c, `smart` groups, is done; M4a, SSH, is done; M4b, WireGuard, is done; M4c, external, is done; M5a, the UDP foundation, is done; M5b, UDP over the TLS family, is done; M5c, UDP over WireGuard and the rest, is done; M6a, Shadowsocks, is done; M6b, Snell, is done; M6c, the HTTP/2 family, is done, which completes M6)
```

换成

```markdown
3. **Phase 2** All outbound protocols, policy groups, subscriptions (in progress: M1, the outbound foundation and HTTP / SOCKS5 upstreams, is done; M2a, Trojan (TCP + WebSocket), is done; M2b, VMess / AnyTLS, is done; M2c, Shadow TLS, is done; M3a, member assembly and subscriptions, is done; M3b, connectivity tests and automatic groups, is done; M3c, `smart` groups, is done; M4a, SSH, is done; M4b, WireGuard, is done; M4c, external, is done; M5a, the UDP foundation, is done; M5b, UDP over the TLS family, is done; M5c, UDP over WireGuard and the rest, is done; M6a, Shadowsocks, is done; M6b, Snell, is done; M6c, the HTTP/2 family, is done, which completes M6; M7a, the QUIC foundation and Hysteria 2, is done, with M7b, TUIC, and M7c, the HTTP/3 family and DoQ / DoH3, next)
```

`CLAUDE.md`——把

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、发送时本机地址或路由失效或握手无回应时换新载体（网络变化后自愈）、网络变化入口、同一私钥与 peer 只留一条隧道，节与载体设置都相同的策略共用）、`stream`、`dns`（隧道内 DNS 与缓存；每个问题 2 秒内重发、先等隧道第一次握手）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。M4c（external）已完成——`rurge-config::spec::ExternalSpec`（`args` 是 `Secret`；两个 `external` 策略写了同一个 `local-port` 是 `E0018`；`args` 进内联参数的脱敏名单）、订阅导入的 `external` 行一律跳过（`W0023`）；`rurge-platform::process`（Unix 进程组与 `killpg`，Windows Job Object）；`rurge_proto::external`（`ExternalOutbound`：第一次用到时拉起、输出写 `<数据目录>/external/<策略名>.log` 且超过 1 MiB 时在拉起前轮转、子进程环境去掉代理变量并设 `NO_PROXY=*`、程序退出后下次用到时再拉起且两次拉起至少隔 2 秒、每 500 ms 连一次本机端口最多 6 次；`ProcessHook` / `ProcessGroup` 与 `NoProcessGroups`；程序自己退出时连同它启动的进程一起结束）；`rurge-engine` 的工厂分支、`EngineShared.processes` / `externals` 与 `Engine::stop_external_programs`；bin 的 `PlatformProcesses` 与退出流程最后停掉全部外部程序；只用于测试的工作区成员 `tests/external`（`rurge-external-tests`：极小的 SOCKS5 辅助程序 `socks-helper` 与真实拉起它的用例）；能力表翻转 `external`（`W0007` 不再因 `external` 出现）。M4 至此完成。M5（UDP 路径）按三份计划推进（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）：M5a 已完成——`rurge_net::connector::PacketSocket`（按包收发、带地址的 UDP 载体）与 `Connector::open_udp`（`DirectConnector`：每个地址族一个未连接的 socket，忽略 Windows 的 ICMP 不可达报错）；`Outbound::udp()` / `open_udp()` 与 `UdpSupport`；DIRECT 与 `socks5` / `socks5-tls` / `external` 的 UDP（`udp-relay`，`W0029` 退役）；`ChainConnector::open_udp`（链式 UDP 载体）；`rurge-inbound` 的 SOCKS5 UDP ASSOCIATE（`UdpClient`、`UdpAdmission`、`Dialer::admit_udp` / `associate`）；`rurge-engine` 的 UDP 流水线（`udp` 模块：一条流 = 关联 + 目标、按"关联 × 出站"共用载体的全锥、60 秒 / DNS 10 秒回收、1024 流 / 4096 关联的上限）、请求记录与 API 的 `transport`、`block-quic`（`W0029` 退役）与 `udp-policy-not-supported-behaviour`、QUIC Initial 识别、`PROTOCOL` 规则按传输层匹配 `TCP` / `UDP`；`FakeSocks5` 与 `tests/external` 辅助程序的 UDP ASSOCIATE；对 sing-box `socks` 入站的 UDP 互操作用例。M5b（TLS 族的 UDP）已完成——`rurge-proto` 的 `stream_udp`（一条字节流上按包收发：请求头随第一个包发出、写那个包的目标；`trojan` 的 UDP ASSOCIATE 与 `anytls` 的 UDP over TCP v2 两种封装）、`trojan` / `anytls` 的 `open_udp`（全锥）、`vmess` 的命令 2（`vmess::udp`：每个目标一条 VMess 连接、随它的第一个包建立、每个数据报一个分块，对称型）；`FakeTrojan` / `FakeAnyTls` / `FakeVmess` 的 UDP 与 `rurge_proto::testing::udp_echo_server`；经引擎的端到端用例（`tests/udp_tls_family.rs`）；对 sing-box（三种）与 xray（vmess）的 UDP 互操作用例。M5c（WireGuard 的 UDP 与其余）已完成——`rurge-proto-wireguard` 的 `TunnelUdp`（隧道里每个地址族一个 UDP socket、第一次发往该族时绑定、全锥，目标名经隧道 DNS 或本机解析）与 `Stack::udp_bind` / `check`；`rurge_net::packet_datagram`（把 `PacketSocket` 变成一条到固定目标的 `Datagram`）与 `ChainConnector::connect_udp`，`wireguard` 的载体经 `underlying-proxy`（底层策略不载 UDP 时拨号失败，`W0029` 退役）；启动时 peer 连不上的告警每个策略的每个 peer 5 分钟至多一次（M4b 延后事项 #15）；`rurge_policy::udp_probe`（经策略的 UDP 向 `hostname@ipv4` 问一次 A 记录）、`Engine::test_udp` 与 `POST /v1/policies/test` 结果里的 `udp` 键（`test-udp` / `proxy-test-udp`，不保存、不参与组的选择）；`smart` 计入 UDP（载体打不开算失败、第一个回包算首字节、3 秒无回包只在 53 / 443 端口算失败、UDP 不换成员）；`dns-follow-interface`（`rurge_net::connector::Via` / `ResolveVia`、`Resolver::lookup_via`：策略自己的解析经它的 `interface` 问普通 DNS 服务器、答案另存，配了加密 DNS 时不跟随；没有 `interface` 时 `W0028`）；对 sing-box WireGuard 端点的 UDP 互操作。M5 至此完成。M5 合并后的复查（2026-10-02，M5 设计第 23 节）改了：Windows 上写了 `interface` 的策略能开 UDP 载体；发给客户端失败只丢那个包；发往上游时出站不收或系统拒发的包只丢那一个（`lost_alone`），其余发送失败的流记为失败并歇 5 秒，一次发送至多 15 秒（`SEND_TIMEOUT`，超时作废载体）；TCP keepalive（`rurge_net::socket::keep_alive`，监听与 `DirectConnector` 都开）；名字目标的回包按端口归流，`smart` 计入 UDP 的发送失败、3 秒规则只看发往地址的流；重建注册表时沿用内置 DIRECT；`wireguard` 跟随底层策略的变化（`Connector::route` / `rurge_net::connector::Route`、`PolicyRegistry::relay_now`）；开 UDP 流时的解析限时 10 秒；每个关联排队至多 1 MiB（`Queued`）、一次打开失败等着它的流一起失败；双栈监听上 IPv4 客户端的回包发往映射地址。M6（Shadowsocks / Snell / HTTP/2 族）按三份计划推进（M6a Shadowsocks → M6b Snell → M6c HTTP/2 族）：M6a（Shadowsocks）已完成——`rurge-config::spec` 的 `SsSpec`（`SsMethod`：AEAD 五种、`none`、SS 2022 两种；2022 的 `password` 按冒号拆成逐层的 Base64 密钥、最后一段是用户密钥，不合法是 `E0018` 且不引用取值；`udp-relay`、`udp-port`）与 `ObfsOpts`（与 Snell 共用；`obfs-host` 缺省服务器主机名、`obfs-uri` 缺省 `/`）、`NotImplemented`（取代 `legacy_vmess` 标记：vmess 旧握手与 `ss` 流式旧方法都是 `W0007` + REJECT，流式方法每种每次加载一条，会话日志 `policy protocol not implemented: ss (<method>)`）、`obfs-host` 进内联参数的脱敏名单；`rurge_proto::transport::obfs`（simple-obfs 的 `http` / `tls`，自写模板，`Stack` 的一层：connect → shadow-tls → obfs → tls → ws）；`rurge_proto::shadowsocks`（`ShadowsocksOutbound`：AEAD 分块流与 `none`、SS 2022（BLAKE3 子密钥、请求头块与填充、应答的类型 / 时间戳 / 回显 salt 校验、SIP023 多用户身份头）、UDP（`udp-relay=true` 发往 `udp-port`，全锥；2022 的分离头、按服务端 session 的防重放窗口））；新依赖只有 `blake3`；`rurge_proto::testing` 的 `FakeShadowsocks`（AEAD、2022 含身份头、UDP、两种 obfs）与 `accept_obfs`；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_shadowsocks.rs`）；能力表翻转 `ss`（流式旧方法除外）；测试里"未实现的协议"的例子从 `ss` 换成 `hysteria2`；`tests/interop` 对 shadowsocks-rust `ssserver`（固定版本 v1.25.0，`RURGE_TEST_SSSERVER`）与 sing-box `shadowsocks` 入站的互操作用例。M6b（Snell）已完成——`rurge-config::spec` 的 `SnellSpec`（`SnellVersion` 只有 V4 / V5；`version` 1–6、缺省 1，1–3 与 6 是 `NotImplemented::SnellVersion`：`W0007` + REJECT，每个版本每次加载一条，会话日志 `policy protocol not implemented: snell v<n>`；`psk` 是 `Secret`；v4 / v5 的 `obfs` 只有 `http`；`mode` 只对 v6 校验）；`rurge_proto::snell`（`SnellOutbound`：`kdf`（Argon2id 取前 16 字节，在阻塞线程上算；工作区依赖 `argon2` 0.6，已在依赖树里）、`record`（`SnellStream`：每方向一个 salt、7 字节加密头、首帧 256–511 字节填充与负载密文交错、每帧至多 0x3FFF、空帧结束一个方向、复用时计数器延续）、`tunnel`（请求头随首段负载写出，`reuse=false` 发 Connect `0x01`、`reuse=true` 发 ConnectV2 `0x05`；应答 `00` / `02`；干净结束的连接在 `Drop` 时回池，没结束的在后台补完再回池；池里取出的失效连接换新连接重发一次）、`pool`（每个出站至多 8 条空闲连接、空闲 60 秒回收）、`udp`（命令 `0x06`：每个载体一条自己的连接、从不进池，每个数据报一帧，全锥；`udp-port` 只解析（`W0029`），UDP 一律走主端口））；`LazyHead` 改为泛型（`LazyHead<S = BoxedStream>`，加 `get_mut` / `into_inner`）；`rurge_proto::testing` 的 `FakeSnell`（`SnellScript`：TCP、ConnectV2 复用、UDP、obfs http、拒绝、每条连接的请求数上限）；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_snell.rs`）；能力表翻转 `snell`（v4 / v5）；`tests/interop` 对 sing-box `snell` 入站（`version: 5`，同时接受 v4 客户端；sing-box 固定版本从 1.14.1 升到 1.14.2）与 Surge 官方 snell-server v5.0.1（只有 Linux 版，`RURGE_TEST_SNELL_SERVER`，只在 Linux CI 上装）的互操作用例。M6c（HTTP/2 族）已完成——`rurge-config::spec::h2`（`H2ConnectSpec`：凭据写在端口之后或命名、命名的优先，`headers`、`max-streams` 缺省 3 且至少 1、`udp-relay`；`TrustTunnelSpec`：`username` / `password` 只读命名写法、必填（`E0018`）；两者 `alpn` 固定 `h2`（写了别的 `W0028`），`headers` 里 HTTP/2 禁止的连接专用字段与 `host` 各 `W0028` 并去掉；`trust-tunnel` 的 `h3=true` 是 `W0029`、M7 之前按 HTTP/2 连，`udp-relay` 是 `W0028`）；`rurge_proto::h2pool`（`H2Pool`：每个出站一个池，每条 TLS + HTTP/2 连接至多 `max-streams` 条流且不超过服务端的上限、流数自己计，新连接收到服务端 SETTINGS 之前只承载一条流（得知 SETTINGS 的 PING 5 秒内没有回应即不再分配新流），在途的每次拨号算作 `max-streams` 个位置、放不下的请求再拨（至多 8 条在途），连接窗口为 `max-streams` × 1 MiB（至少 4 MiB），没有流的连接空闲 60 秒关闭，收到 GOAWAY 的连接不再分配新流，请求没得到应答时用 PING 检查连接是否还活着（5 秒），死了就换新连接；`H2Stream`：按流控窗口分段写、读即释放容量、END_STREAM 半关闭；`StackDial`：ALPN 不是 `h2` 时 `<协议>: the server does not speak HTTP/2`）；`rurge_proto::h2connect`（`H2ConnectOutbound`：CONNECT + Basic，`headers` 替换同名的自生成字段、`<random-string>` 按请求生成；407 是 `h2-connect: proxy authentication required`；`capsule`（RFC 9297 的 capsule 与 varint）与 `udp`（`udp-relay=true` 时的 CONNECT-UDP：每个目标一条 extended CONNECT 流、对称型，masque 默认模板的路径，DATAGRAM capsule，其它 capsule 与 context id 丢弃，超过 65527 字节的数据报不发，服务端没通告 extended CONNECT 时发往每个目标的第一个包失败，空闲 60 秒关流））；`rurge_proto::trust_tunnel`（`TrustTunnelOutbound`：同一套 CONNECT，`user-agent: rurge`，407 是 `trust-tunnel: authentication failed`，不载 UDP）；`TlsClient::wrap_negotiated` / `Stack::open_negotiated`（取协商出的 ALPN）；`rurge_proto::testing` 的 `FakeH2Proxy`（`H2ProxyScript`：Basic 与 407、拒绝、服务端流数上限、GOAWAY、extended CONNECT 与 CONNECT-UDP、多余的 capsule、`user-agent` 必填、不参与 ALPN、双向 TLS）；`rurge-engine` 的两个工厂分支与经引擎的端到端用例（`tests/outbounds_http2.rs`）；能力表翻转 `h2-connect` 与 `trust-tunnel`；`tests/interop` 对 TrustTunnel endpoint v1.1.0（只有 Linux / macOS 版，`RURGE_TEST_TRUSTTUNNEL`；sing-box 1.14.2 的 `http` 入站只说 HTTP/1，所以 endpoint 同时充当 `h2-connect` 普通 CONNECT 的参照）的互操作用例；CONNECT-UDP 没有可用的参照服务端。M6 至此完成；合并后的复查（M6 设计第 18 节）又修了会话池的串行拨号与 SETTINGS 到达前静默断掉的连接、Snell 的 `udp-port`（只解析）与重拨时只唤醒最后一个轮询者、CONNECT-UDP 的 `:authority`（取 TLS 的名字）、`headers` 的值进 `Debug`、ss 首字节前被重置时的提示、`obfs-uri` 脱敏，并把 CI 钉在 1.89（`RUSTUP_TOOLCHAIN=1.89.0`；此前 CI 每次装最新 stable，三个平台都在 clippy 一步失败，测试与互操作从未执行过）。
```

换成

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、发送时本机地址或路由失效或握手无回应时换新载体（网络变化后自愈）、网络变化入口、同一私钥与 peer 只留一条隧道，节与载体设置都相同的策略共用）、`stream`、`dns`（隧道内 DNS 与缓存；每个问题 2 秒内重发、先等隧道第一次握手）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。M4c（external）已完成——`rurge-config::spec::ExternalSpec`（`args` 是 `Secret`；两个 `external` 策略写了同一个 `local-port` 是 `E0018`；`args` 进内联参数的脱敏名单）、订阅导入的 `external` 行一律跳过（`W0023`）；`rurge-platform::process`（Unix 进程组与 `killpg`，Windows Job Object）；`rurge_proto::external`（`ExternalOutbound`：第一次用到时拉起、输出写 `<数据目录>/external/<策略名>.log` 且超过 1 MiB 时在拉起前轮转、子进程环境去掉代理变量并设 `NO_PROXY=*`、程序退出后下次用到时再拉起且两次拉起至少隔 2 秒、每 500 ms 连一次本机端口最多 6 次；`ProcessHook` / `ProcessGroup` 与 `NoProcessGroups`；程序自己退出时连同它启动的进程一起结束）；`rurge-engine` 的工厂分支、`EngineShared.processes` / `externals` 与 `Engine::stop_external_programs`；bin 的 `PlatformProcesses` 与退出流程最后停掉全部外部程序；只用于测试的工作区成员 `tests/external`（`rurge-external-tests`：极小的 SOCKS5 辅助程序 `socks-helper` 与真实拉起它的用例）；能力表翻转 `external`（`W0007` 不再因 `external` 出现）。M4 至此完成。M5（UDP 路径）按三份计划推进（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）：M5a 已完成——`rurge_net::connector::PacketSocket`（按包收发、带地址的 UDP 载体）与 `Connector::open_udp`（`DirectConnector`：每个地址族一个未连接的 socket，忽略 Windows 的 ICMP 不可达报错）；`Outbound::udp()` / `open_udp()` 与 `UdpSupport`；DIRECT 与 `socks5` / `socks5-tls` / `external` 的 UDP（`udp-relay`，`W0029` 退役）；`ChainConnector::open_udp`（链式 UDP 载体）；`rurge-inbound` 的 SOCKS5 UDP ASSOCIATE（`UdpClient`、`UdpAdmission`、`Dialer::admit_udp` / `associate`）；`rurge-engine` 的 UDP 流水线（`udp` 模块：一条流 = 关联 + 目标、按"关联 × 出站"共用载体的全锥、60 秒 / DNS 10 秒回收、1024 流 / 4096 关联的上限）、请求记录与 API 的 `transport`、`block-quic`（`W0029` 退役）与 `udp-policy-not-supported-behaviour`、QUIC Initial 识别、`PROTOCOL` 规则按传输层匹配 `TCP` / `UDP`；`FakeSocks5` 与 `tests/external` 辅助程序的 UDP ASSOCIATE；对 sing-box `socks` 入站的 UDP 互操作用例。M5b（TLS 族的 UDP）已完成——`rurge-proto` 的 `stream_udp`（一条字节流上按包收发：请求头随第一个包发出、写那个包的目标；`trojan` 的 UDP ASSOCIATE 与 `anytls` 的 UDP over TCP v2 两种封装）、`trojan` / `anytls` 的 `open_udp`（全锥）、`vmess` 的命令 2（`vmess::udp`：每个目标一条 VMess 连接、随它的第一个包建立、每个数据报一个分块，对称型）；`FakeTrojan` / `FakeAnyTls` / `FakeVmess` 的 UDP 与 `rurge_proto::testing::udp_echo_server`；经引擎的端到端用例（`tests/udp_tls_family.rs`）；对 sing-box（三种）与 xray（vmess）的 UDP 互操作用例。M5c（WireGuard 的 UDP 与其余）已完成——`rurge-proto-wireguard` 的 `TunnelUdp`（隧道里每个地址族一个 UDP socket、第一次发往该族时绑定、全锥，目标名经隧道 DNS 或本机解析）与 `Stack::udp_bind` / `check`；`rurge_net::packet_datagram`（把 `PacketSocket` 变成一条到固定目标的 `Datagram`）与 `ChainConnector::connect_udp`，`wireguard` 的载体经 `underlying-proxy`（底层策略不载 UDP 时拨号失败，`W0029` 退役）；启动时 peer 连不上的告警每个策略的每个 peer 5 分钟至多一次（M4b 延后事项 #15）；`rurge_policy::udp_probe`（经策略的 UDP 向 `hostname@ipv4` 问一次 A 记录）、`Engine::test_udp` 与 `POST /v1/policies/test` 结果里的 `udp` 键（`test-udp` / `proxy-test-udp`，不保存、不参与组的选择）；`smart` 计入 UDP（载体打不开算失败、第一个回包算首字节、3 秒无回包只在 53 / 443 端口算失败、UDP 不换成员）；`dns-follow-interface`（`rurge_net::connector::Via` / `ResolveVia`、`Resolver::lookup_via`：策略自己的解析经它的 `interface` 问普通 DNS 服务器、答案另存，配了加密 DNS 时不跟随；没有 `interface` 时 `W0028`）；对 sing-box WireGuard 端点的 UDP 互操作。M5 至此完成。M5 合并后的复查（2026-10-02，M5 设计第 23 节）改了：Windows 上写了 `interface` 的策略能开 UDP 载体；发给客户端失败只丢那个包；发往上游时出站不收或系统拒发的包只丢那一个（`lost_alone`），其余发送失败的流记为失败并歇 5 秒，一次发送至多 15 秒（`SEND_TIMEOUT`，超时作废载体）；TCP keepalive（`rurge_net::socket::keep_alive`，监听与 `DirectConnector` 都开）；名字目标的回包按端口归流，`smart` 计入 UDP 的发送失败、3 秒规则只看发往地址的流；重建注册表时沿用内置 DIRECT；`wireguard` 跟随底层策略的变化（`Connector::route` / `rurge_net::connector::Route`、`PolicyRegistry::relay_now`）；开 UDP 流时的解析限时 10 秒；每个关联排队至多 1 MiB（`Queued`）、一次打开失败等着它的流一起失败；双栈监听上 IPv4 客户端的回包发往映射地址。M6（Shadowsocks / Snell / HTTP/2 族）按三份计划推进（M6a Shadowsocks → M6b Snell → M6c HTTP/2 族）：M6a（Shadowsocks）已完成——`rurge-config::spec` 的 `SsSpec`（`SsMethod`：AEAD 五种、`none`、SS 2022 两种；2022 的 `password` 按冒号拆成逐层的 Base64 密钥、最后一段是用户密钥，不合法是 `E0018` 且不引用取值；`udp-relay`、`udp-port`）与 `ObfsOpts`（与 Snell 共用；`obfs-host` 缺省服务器主机名、`obfs-uri` 缺省 `/`）、`NotImplemented`（取代 `legacy_vmess` 标记：vmess 旧握手与 `ss` 流式旧方法都是 `W0007` + REJECT，流式方法每种每次加载一条，会话日志 `policy protocol not implemented: ss (<method>)`）、`obfs-host` 进内联参数的脱敏名单；`rurge_proto::transport::obfs`（simple-obfs 的 `http` / `tls`，自写模板，`Stack` 的一层：connect → shadow-tls → obfs → tls → ws）；`rurge_proto::shadowsocks`（`ShadowsocksOutbound`：AEAD 分块流与 `none`、SS 2022（BLAKE3 子密钥、请求头块与填充、应答的类型 / 时间戳 / 回显 salt 校验、SIP023 多用户身份头）、UDP（`udp-relay=true` 发往 `udp-port`，全锥；2022 的分离头、按服务端 session 的防重放窗口））；新依赖只有 `blake3`；`rurge_proto::testing` 的 `FakeShadowsocks`（AEAD、2022 含身份头、UDP、两种 obfs）与 `accept_obfs`；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_shadowsocks.rs`）；能力表翻转 `ss`（流式旧方法除外）；测试里"未实现的协议"的例子从 `ss` 换成 `hysteria2`；`tests/interop` 对 shadowsocks-rust `ssserver`（固定版本 v1.25.0，`RURGE_TEST_SSSERVER`）与 sing-box `shadowsocks` 入站的互操作用例。M6b（Snell）已完成——`rurge-config::spec` 的 `SnellSpec`（`SnellVersion` 只有 V4 / V5；`version` 1–6、缺省 1，1–3 与 6 是 `NotImplemented::SnellVersion`：`W0007` + REJECT，每个版本每次加载一条，会话日志 `policy protocol not implemented: snell v<n>`；`psk` 是 `Secret`；v4 / v5 的 `obfs` 只有 `http`；`mode` 只对 v6 校验）；`rurge_proto::snell`（`SnellOutbound`：`kdf`（Argon2id 取前 16 字节，在阻塞线程上算；工作区依赖 `argon2` 0.6，已在依赖树里）、`record`（`SnellStream`：每方向一个 salt、7 字节加密头、首帧 256–511 字节填充与负载密文交错、每帧至多 0x3FFF、空帧结束一个方向、复用时计数器延续）、`tunnel`（请求头随首段负载写出，`reuse=false` 发 Connect `0x01`、`reuse=true` 发 ConnectV2 `0x05`；应答 `00` / `02`；干净结束的连接在 `Drop` 时回池，没结束的在后台补完再回池；池里取出的失效连接换新连接重发一次）、`pool`（每个出站至多 8 条空闲连接、空闲 60 秒回收）、`udp`（命令 `0x06`：每个载体一条自己的连接、从不进池，每个数据报一帧，全锥；`udp-port` 只解析（`W0029`），UDP 一律走主端口））；`LazyHead` 改为泛型（`LazyHead<S = BoxedStream>`，加 `get_mut` / `into_inner`）；`rurge_proto::testing` 的 `FakeSnell`（`SnellScript`：TCP、ConnectV2 复用、UDP、obfs http、拒绝、每条连接的请求数上限）；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_snell.rs`）；能力表翻转 `snell`（v4 / v5）；`tests/interop` 对 sing-box `snell` 入站（`version: 5`，同时接受 v4 客户端；sing-box 固定版本从 1.14.1 升到 1.14.2）与 Surge 官方 snell-server v5.0.1（只有 Linux 版，`RURGE_TEST_SNELL_SERVER`，只在 Linux CI 上装）的互操作用例。M6c（HTTP/2 族）已完成——`rurge-config::spec::h2`（`H2ConnectSpec`：凭据写在端口之后或命名、命名的优先，`headers`、`max-streams` 缺省 3 且至少 1、`udp-relay`；`TrustTunnelSpec`：`username` / `password` 只读命名写法、必填（`E0018`）；两者 `alpn` 固定 `h2`（写了别的 `W0028`），`headers` 里 HTTP/2 禁止的连接专用字段与 `host` 各 `W0028` 并去掉；`trust-tunnel` 的 `h3=true` 是 `W0029`、M7 之前按 HTTP/2 连，`udp-relay` 是 `W0028`）；`rurge_proto::h2pool`（`H2Pool`：每个出站一个池，每条 TLS + HTTP/2 连接至多 `max-streams` 条流且不超过服务端的上限、流数自己计，新连接收到服务端 SETTINGS 之前只承载一条流（得知 SETTINGS 的 PING 5 秒内没有回应即不再分配新流），在途的每次拨号算作 `max-streams` 个位置、放不下的请求再拨（至多 8 条在途），连接窗口为 `max-streams` × 1 MiB（至少 4 MiB），没有流的连接空闲 60 秒关闭，收到 GOAWAY 的连接不再分配新流，请求没得到应答时用 PING 检查连接是否还活着（5 秒），死了就换新连接；`H2Stream`：按流控窗口分段写、读即释放容量、END_STREAM 半关闭；`StackDial`：ALPN 不是 `h2` 时 `<协议>: the server does not speak HTTP/2`）；`rurge_proto::h2connect`（`H2ConnectOutbound`：CONNECT + Basic，`headers` 替换同名的自生成字段、`<random-string>` 按请求生成；407 是 `h2-connect: proxy authentication required`；`capsule`（RFC 9297 的 capsule 与 varint）与 `udp`（`udp-relay=true` 时的 CONNECT-UDP：每个目标一条 extended CONNECT 流、对称型，masque 默认模板的路径，DATAGRAM capsule，其它 capsule 与 context id 丢弃，超过 65527 字节的数据报不发，服务端没通告 extended CONNECT 时发往每个目标的第一个包失败，空闲 60 秒关流））；`rurge_proto::trust_tunnel`（`TrustTunnelOutbound`：同一套 CONNECT，`user-agent: rurge`，407 是 `trust-tunnel: authentication failed`，不载 UDP）；`TlsClient::wrap_negotiated` / `Stack::open_negotiated`（取协商出的 ALPN）；`rurge_proto::testing` 的 `FakeH2Proxy`（`H2ProxyScript`：Basic 与 407、拒绝、服务端流数上限、GOAWAY、extended CONNECT 与 CONNECT-UDP、多余的 capsule、`user-agent` 必填、不参与 ALPN、双向 TLS）；`rurge-engine` 的两个工厂分支与经引擎的端到端用例（`tests/outbounds_http2.rs`）；能力表翻转 `h2-connect` 与 `trust-tunnel`；`tests/interop` 对 TrustTunnel endpoint v1.1.0（只有 Linux / macOS 版，`RURGE_TEST_TRUSTTUNNEL`；sing-box 1.14.2 的 `http` 入站只说 HTTP/1，所以 endpoint 同时充当 `h2-connect` 普通 CONNECT 的参照）的互操作用例；CONNECT-UDP 没有可用的参照服务端。M6 至此完成；合并后的复查（M6 设计第 18 节）又修了会话池的串行拨号与 SETTINGS 到达前静默断掉的连接、Snell 的 `udp-port`（只解析）与重拨时只唤醒最后一个轮询者、CONNECT-UDP 的 `:authority`（取 TLS 的名字）、`headers` 的值进 `Debug`、ss 首字节前被重置时的提示、`obfs-uri` 脱敏，并把 CI 钉在 1.89（`RUSTUP_TOOLCHAIN=1.89.0`；此前 CI 每次装最新 stable，三个平台都在 clippy 一步失败，测试与互操作从未执行过）。M7（QUIC 族）按三份计划推进（M7a QUIC 地基 + Hysteria 2 → M7b TUIC → M7c HTTP/3 族与 DoQ / DoH3）：M7a 已完成——新依赖只有 `quinn` 0.11.12（`default-features = false`，`runtime-tokio` + `rustls-ring`，随之引入 quinn-proto / quinn-udp；不用 crates.io 的 `h3`，M7-D5），`blake2` 0.10 沿用依赖树里的；`rurge-config::spec` 的 `QuicOpts`（只有 `port_hopping: Option<PortHopping>`；`alpn` 留在 `TlsOpts`、QUIC 类缺省 `h3`，`ecn` 留在 `CommonOpts`）、`Hysteria2Spec`（`password` 必填、`Secret`；`download-bandwidth` 整数 Mbps；`Hysteria2Obfs` 的 Salamander / Gecko 互斥，都是 `Secret`）与 `ProtoSpec::quic()`：`port-hopping` 写法错误是 `E0018`，`port-hopping-interval` 小于 5 是 `W0012` 并按 5、没有 `port-hopping` 时 `W0028`，`tfo` / `udp-relay` 在 `hysteria2` 上是 `W0028`，`ecn` 在它上面不再 `W0029`，`port-hopping` × `underlying-proxy` 与 Shadow TLS × `hysteria2` 两条 `E0018` 真正报出，`salamander-password` / `gecko-password` 进内联参数的脱敏名单；`rurge-net` 的 `Connector::as_direct()`（只有 `DirectConnector` 覆盖：直连时 QUIC 自己建 socket，其余经 `connect_udp` 的载体）与 `quic` 模块（`Path::open`：直连 socket 复用 `DirectConnector::bind_udp`，`tos ≠ 0` 时发送不经 quinn-udp，`ecn=off` 去掉两侧的标记，链式载体（占位地址 `192.0.2.1:443`、不做 PMTU 探测、放不下就丢），端口跳跃层（每次换到另一个端口、上一个 socket 留到下一跳）；socket 层的发送错误一律丢包——非 `WouldBlock` 的错误会让 quinn 的连接挂死；`dial`：每条连接一个 quinn endpoint、`Transport::STANDARD`（空闲 30 秒、保活 10 秒、8 / 20 MiB 窗口、Cubic）、`connection_error` 的 `quic: …` / `tls: …` 文本；`testing` 的 `TestCert` / `EchoServer` / `MultiPort`）与 `quic::h3`（自写的最小 HTTP/3 客户端：只开控制流、SETTINGS 与等待、QPACK 静态表 + 字面量 + Huffman 解码（照 RFC 9204 附录 A 与 RFC 7541 附录 B 录入并独立核对）、HEADERS / DATA、可取消安全的写、GOAWAY；`testing` 的 `H3Server`）；`rurge-proto` 的 `QuicTls` / `build::quic_tls`（与 `TlsClient` 共用构造，只用 TLS 1.3）；新 crate `rurge-proto-quic`：`session`（`Dialer`（`with_layer`；`port-hopping` 遇上非直连的连接器在构建时失败，组中继派生的 `M (via R)` 因此被略去）、`Protocol`、`Session`：在请求自己的 future 里单飞拨号与握手、只关还开着的连接、按租约计的 60 秒空闲关闭、失效连接重试一次、跟随底层策略的去处（`Connector::route()`）；`testing::ServerPort`：能重启并回 stateless reset 的服务端端口）与 `hysteria2`（`Hysteria2Outbound`：HTTP/3 认证（失败以 0x101 关闭，`Hysteria-UDP` 按 Go 的 `ParseBool` 读）、TCP（`0x401`，任何非 0 状态都是拒绝）、UDP（会话号按出站递增、分片与重组、`Hysteria-UDP: false` 时打开载体即失败、换连接沿用会话号）、`obfs`（Salamander：BLAKE2b-256 用 RFC 7693 的自测核对；Gecko：长头包拆 2–8 片、512–1200 字节、至多 8 组在途、8 秒过期；混淆口令不对只表现为超时）；`testing::FakeHysteria2`（线上编解码与混淆都独立实现））；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_hysteria2.rs`；`common::harness_edited` 缩短跳跃间隔）与不作门禁的吞吐基准（Windows 回环、release：每方向明文约 70–80 MiB/s、Salamander 约 56–66 MiB/s）；bin 不输出 `quinn` / `quinn_proto` 的日志，`quinn_udp` 留 WARN；能力表翻转 `hysteria2`；测试里"未实现的协议"的例子从 `hysteria2` 换成 `tailscale`；`tests/interop` 对 sing-box 1.14.2 `hysteria2` 入站（无混淆、Salamander、Gecko）的互操作用例（CI 已在三平台装 sing-box，不用改）。
```

`CLAUDE.md`——把

```markdown
- `docs/superpowers/plans/2026-10-01-phase2-m6c-http2-plan.md`：阶段 2 / M6c（HTTP/2 族：h2-connect、trust-tunnel）实施计划（7 个任务）。开头「计划期决定」表记录核对 RFC 9113 / 9298 / 9297、`h2` 0.4 源码、TrustTunnel 协议文档与 endpoint 源码得出的结论和与设计文字不同的决定（`h2-connect` 的凭据两种写法都收、`alpn` 固定 `h2`、连接专用字段加载时去掉、`headers` 替换自生成的同名字段、流数自己计且新连接在服务端 SETTINGS 之前只承载一条流、接收窗口取 1 MiB、`trust-tunnel` 的 `user-agent` 固定 `rurge`、没有 extended CONNECT 时错误出现在第一个数据报上、TrustTunnel endpoint 同时充当 `h2-connect` 的互操作参照等）；末尾「执行期修正记录」与「延后事项」两张表。
```

换成

```markdown
- `docs/superpowers/plans/2026-10-01-phase2-m6c-http2-plan.md`：阶段 2 / M6c（HTTP/2 族：h2-connect、trust-tunnel）实施计划（7 个任务）。开头「计划期决定」表记录核对 RFC 9113 / 9298 / 9297、`h2` 0.4 源码、TrustTunnel 协议文档与 endpoint 源码得出的结论和与设计文字不同的决定（`h2-connect` 的凭据两种写法都收、`alpn` 固定 `h2`、连接专用字段加载时去掉、`headers` 替换自生成的同名字段、流数自己计且新连接在服务端 SETTINGS 之前只承载一条流、接收窗口取 1 MiB、`trust-tunnel` 的 `user-agent` 固定 `rurge`、没有 extended CONNECT 时错误出现在第一个数据报上、TrustTunnel endpoint 同时充当 `h2-connect` 的互操作参照等）；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/superpowers/specs/2026-10-02-phase2-m7-quic-design.md`：阶段 2 / M7 细化设计（QUIC 族：Hysteria 2、TUIC v5 / v4、MASQUE、`trust-tunnel` 的 h3、DoQ / DoH3），细化总设计的 M7 里程碑、不一致处以它为准，关闭总设计的开放问题 Q6 并处理风险 B。三份计划的拆分（M7a QUIC 地基 + Hysteria 2 → M7b TUIC → M7c HTTP/3 族与 DoQ / DoH3）；已决事项 M7-D1 ～ D12（全部都做、TUIC v4 也做、`ecn` 跟随 quinn、自写最小的 HTTP/3 客户端而不用 crates.io 的 `h3`、参考服务端、新依赖只有 `quinn` 0.11.12、`rurge-net::quic` 与新 crate `rurge-proto-quic` 的分工、每条连接一个 quinn endpoint、客户端一律 Cubic、Gecko 照官网规范而与 Surge 是否相同未证实、`W0007` 与 `Unsupported` 的退役改到 M8）；QUIC 共用部分（配置、拨号与 TLS、UDP 层、ECN、会话、错误文本）、HTTP/3 客户端、各协议的线上格式与三层测试；第 9 节是需登记的差异，第 11 节 V1 ～ V10 是写各份计划时必须核对的事项，第 12 节是任务草图，第 14 节是 M7a 计划期的订正。
- `docs/superpowers/plans/2026-10-02-phase2-m7a-quic-hysteria2-plan.md`：阶段 2 / M7a（QUIC 地基 + Hysteria 2）实施计划（9 个任务）。开头「计划期决定」表记录核对 quinn / quinn-proto / quinn-udp 源码、RFC 9000 / 9114 / 9204 / 7541 / 7693、Hysteria 官网规范与参考实现得出的结论和与设计文字不同的决定（`QuicOpts` 只放 `port-hopping`、`alpn` 与 `ecn` 留在原处、间隔小于 5 秒是 `W0012`、`tfo` / `udp-relay` 是 `W0028`；`tos ≠ 0` 只在发送一侧不经 quinn-udp、端口跳跃每次换到另一个端口、链式载体放不下就丢且不做 PMTU 探测、socket 层的发送错误一律丢包；HTTP/3 只开控制流、两张码表照原文录入并独立核对；单飞拨号在请求自己的 future 里、只关还开着的连接、失效连接只对请求到来时已建好的连接重试一次、跟随底层策略的去处；认证失败以 0x101 关闭、任何非 0 的 TCP 状态都是拒绝、会话号按出站递增、`Hysteria-UDP: false` 时打开载体即失败；混淆层在最外面、Salamander 丢弃不超过 8 字节的包、Gecko 的第 9 组挤掉最旧的；`port-hopping` 遇上非直连连接器在构建时失败；"未实现的协议"的例子换成 `tailscale` 等）；末尾「执行期修正记录」与「延后事项」两张表。
```

`CLAUDE.md`——把

```markdown
- 依赖方向（M2 设计文档确认）：`rurge-dns → rurge-rules → rurge-net → rurge-config`；`[Host]` 集合键与 `LazyResolver` 都由 `rurge-dns` 依赖 `rurge-rules` 提供，而非并列关系；`rurge (bin) → rurge-engine → { rurge-inbound → rurge-proto, rurge-policy → rurge-proto, rurge-dns }`。M4 设计文档确认：`rurge (bin) → rurge-api → rurge-engine`；`rurge-api` 依赖 `rurge-engine` / `rurge-config`（`rurge-dns` 的类型经 `rurge-engine` 间接可达，不需要直接依赖），不依赖 `rurge-platform`，也不认识 bin。`tests/interop`（`rurge-interop`，`publish = false`）与 `tests/external`（`rurge-external-tests`，`publish = false`，M4c）是仅测试用的工作区成员。
```

换成

```markdown
- 依赖方向（M2 设计文档确认）：`rurge-dns → rurge-rules → rurge-net → rurge-config`；`[Host]` 集合键与 `LazyResolver` 都由 `rurge-dns` 依赖 `rurge-rules` 提供，而非并列关系；`rurge (bin) → rurge-engine → { rurge-inbound → rurge-proto, rurge-policy → rurge-proto, rurge-dns }`。M4 设计文档确认：`rurge (bin) → rurge-api → rurge-engine`；`rurge-api` 依赖 `rurge-engine` / `rurge-config`（`rurge-dns` 的类型经 `rurge-engine` 间接可达，不需要直接依赖），不依赖 `rurge-platform`，也不认识 bin。`tests/interop`（`rurge-interop`，`publish = false`）与 `tests/external`（`rurge-external-tests`，`publish = false`，M4c）是仅测试用的工作区成员。M7 设计文档确认（M7-D8）：`rurge-engine → rurge-proto-quic → { rurge-proto, rurge-net, rurge-config }`；QUIC 的共用部分（UDP 层、拨号与 TLS、自写的 HTTP/3 客户端）在 `rurge-net::quic`，M7c 的 DoQ / DoH3 经它（`rurge-dns → rurge-rules → rurge-net`），`rurge-dns` 不依赖协议 crate。
```

`CLAUDE.md`——把

```markdown
cargo test -p rurge-external-tests              # external：真实拉起测试辅助程序（socks-helper）——按需拉起、参数顺序与代理变量、再拉起与 2 秒间隔、连接被拒的重试、日志、整棵进程树的停止、经引擎的端到端、检查不拉起、重载沿用与替换
cargo test -p rurge-interop                     # 对 sing-box 1.14.2（HTTP/2 族之外的全部协议）、xray（只测 vmess）、shadowsocks-rust ssserver（只测 ss）、官方 snell-server v5.0.1（只测 snell，只在 Linux）、TrustTunnel endpoint v1.1.0（测 trust-tunnel 与 h2-connect 的 TCP，只在 Linux / macOS）与 OpenSSH sshd（只测 ssh，只在 Unix）的互操作测试；没装就跳过（RURGE_TEST_SING_BOX / RURGE_TEST_XRAY / RURGE_TEST_SSSERVER / RURGE_TEST_SNELL_SERVER / RURGE_TEST_TRUSTTUNNEL / RURGE_TEST_SSHD / RURGE_INTEROP_REQUIRED=1）
```

换成

```markdown
cargo test -p rurge-net quic                    # QUIC 共用部分：UDP 层（直连、tos、ecn=off、链式载体、端口跳跃）、拨号与传输参数、错误文本；自写的 HTTP/3 客户端（QPACK 静态表与 Huffman、帧、SETTINGS / GOAWAY、可取消安全的写）对回环 H3Server
cargo test -p rurge-proto-quic                  # QUIC 会话（单飞、空闲关闭、失效重试、跟随底层策略）与 hysteria2（认证、TCP、UDP 分片与重组、Salamander / Gecko 与 BLAKE2b-256 的已知答案），出站对回环假服务端（FakeHysteria2）
cargo test -p rurge-engine --test outbounds_hysteria2   # 经 hysteria2 出站的端到端用例：TCP 与 UDP、口令错误、Hysteria-UDP: false、两种混淆、端口跳跃、经 underlying-proxy（socks5）与底层组换成员、组中继里跳端口的成员被略去、测速、重载沿用与替换
cargo test -p rurge-proto-quic --release throughput -- --ignored --nocapture   # Hysteria 2 吞吐基准（经回环假服务端双向 64 MiB，明文与 Salamander；不作门禁）
cargo test -p rurge-external-tests              # external：真实拉起测试辅助程序（socks-helper）——按需拉起、参数顺序与代理变量、再拉起与 2 秒间隔、连接被拒的重试、日志、整棵进程树的停止、经引擎的端到端、检查不拉起、重载沿用与替换
cargo test -p rurge-interop                     # 对 sing-box 1.14.2（HTTP/2 族之外的全部协议，含 hysteria2 的无混淆 / Salamander / Gecko）、xray（只测 vmess）、shadowsocks-rust ssserver（只测 ss）、官方 snell-server v5.0.1（只测 snell，只在 Linux）、TrustTunnel endpoint v1.1.0（测 trust-tunnel 与 h2-connect 的 TCP，只在 Linux / macOS）与 OpenSSH sshd（只测 ssh，只在 Unix）的互操作测试；没装就跳过（RURGE_TEST_SING_BOX / RURGE_TEST_XRAY / RURGE_TEST_SSSERVER / RURGE_TEST_SNELL_SERVER / RURGE_TEST_TRUSTTUNNEL / RURGE_TEST_SSHD / RURGE_INTEROP_REQUIRED=1）
```

PRD（Gecko 不再是不做的项目）：

`docs/requirements.md`——把

```markdown
| Snell v6 与 v5 的 QUIC Proxy Mode、Hysteria 2 的 Gecko 混淆 | 协议细节未公开 | 待评估；Snell v4 / v5（阶段 2 M6b 已实现 TCP 与 UDP over TCP，v1–v3 只解析，阶段 2 M6-D2）与 Salamander 在范围内 |
```

换成

```markdown
| Snell v6 与 v5 的 QUIC Proxy Mode | 协议细节未公开 | 待评估；Snell v4 / v5（阶段 2 M6b 已实现 TCP 与 UDP over TCP，v1–v3 只解析，阶段 2 M6-D2）在范围内；Hysteria 2 的 Salamander 与 Gecko 混淆也在范围内（阶段 2 M7a 已实现；Gecko 照 Hysteria 官网的公开规范，与 Surge 的 `gecko-password` 是否同一算法未证实，在手工验收里核对，阶段 2 M7-D11） |
```

`docs/requirements.md`——把

```markdown
| FR-OUT-05 | 协议实现：`http` `https` `h2-connect` `socks5` `socks5-tls` `ss`（AEAD、2022、obfs）`snell`（v4 / v5，阶段 2 M6-D2）`vmess` `trojan` `tuic` `tuic-v5` `hysteria2`（Salamander）`masque` `anytls` `trust-tunnel`（h2 / h3）`ssh` `wireguard` `external`；每种协议的专属参数见清单 4.6 | P0 | 2 |
```

换成

```markdown
| FR-OUT-05 | 协议实现：`http` `https` `h2-connect` `socks5` `socks5-tls` `ss`（AEAD、2022、obfs）`snell`（v4 / v5，阶段 2 M6-D2）`vmess` `trojan` `tuic` `tuic-v5` `hysteria2`（Salamander、Gecko，阶段 2 M7a；Gecko 与 Surge 的 `gecko-password` 是否同一算法未证实，阶段 2 M7-D11）`masque` `anytls` `trust-tunnel`（h2 / h3）`ssh` `wireguard` `external`；每种协议的专属参数见清单 4.6 | P0 | 2 |
```

`docs/requirements.md`——把

```markdown
| FR-OUT-15 | Snell v6 与 v5 的 QUIC Proxy Mode、Gecko 混淆、Tailscale 的可行性评估报告 | P2 | 2 / 远期 |
```

换成

```markdown
| FR-OUT-15 | Snell v6 与 v5 的 QUIC Proxy Mode、Tailscale 的可行性评估报告（Hysteria 2 的 Gecko 混淆已在阶段 2 M7a 照 Hysteria 官网的公开规范实现，不再是评估项） | P2 | 2 / 远期 |
```

`docs/requirements.md`——把

```markdown
Tailscale 策略、Ponte 替代方案、Metered Network Mode、Snell v6 与 v5 的 QUIC Proxy Mode、Hysteria 2 Gecko 混淆、Windows 网关模式。每项以单独的评估文档决定是否进入路线图。
```

换成

```markdown
Tailscale 策略、Ponte 替代方案、Metered Network Mode、Snell v6 与 v5 的 QUIC Proxy Mode、Windows 网关模式。每项以单独的评估文档决定是否进入路线图。
```

`docs/requirements.md`——把

```markdown
| R1 | 非公开的协议与算法（Snell v6 与 v5 的 QUIC Proxy Mode、Gecko、smart 组评分） | 无法完全一致 | 近似实现并在清单标注差异；Snell v4 / v5 依公开描述实现、以对参考服务端的互操作为准（阶段 2 M6b），v6 列为待评估 |
```

换成

```markdown
| R1 | 非公开的协议与算法（Snell v6 与 v5 的 QUIC Proxy Mode、Surge 的 Gecko、smart 组评分） | 无法完全一致 | 近似实现并在清单标注差异；Snell v4 / v5 依公开描述实现、以对参考服务端的互操作为准（阶段 2 M6b），v6 列为待评估；Gecko 照 Hysteria 官网的公开规范实现、以对 sing-box 的互操作为准（阶段 2 M7a），与 Surge 是否同一算法在手工验收里核对 |
```

总设计（第 1.4 节 M7 行、技术选型、crate 布局、`W0007` / `Unsupported` 的退役、第 6 节的 QUIC 类协议行、第 13 节 M7 的参考实现、风险 B、开放问题 Q6）：

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| **M7 QUIC 族** | `rurge-net::quic` 公共件、`rurge-proto-quic`（`tuic` `tuic-v5` `hysteria2` `masque`、`trust-tunnel` 的 h3 模式）、`port-hopping`、`ecn`、DoH3 / DoQ 上游 | FR-OUT-03（`ecn`）/ 05、FR-DNS-04 | QUIC 族 TCP 与 UDP 转发通过；`h3://` `quic://` 上游解析正常 |
| **M8 收尾与验收** | P2 项（Shadowsocks 流式旧方法、VMess 旧握手）、Snell v1 ～ v3（需要时）、Snell v6 与 v5 QUIC Proxy Mode · Gecko · Tailscale 可行性报告、阶段验收清单、文档同步 | FR-OUT-06 / 15 | 第 14 节验收标准全部通过 |
```

换成

```markdown
| **M7 QUIC 族** | `rurge-net::quic` 公共件、`rurge-proto-quic`（`tuic` `tuic-v5` `hysteria2` `masque`、`trust-tunnel` 的 h3 模式）、`port-hopping`、`ecn`、DoH3 / DoQ 上游 | FR-OUT-03（`ecn`）/ 05、FR-DNS-04 | QUIC 族 TCP 与 UDP 转发通过；`h3://` `quic://` 上游解析正常。M7 细化设计 `2026-10-02-phase2-m7-quic-design.md` 把它拆成三份计划：M7a（QUIC 地基与 Hysteria 2）→ M7b（TUIC v5 / v4）→ M7c（HTTP/3 族：MASQUE、`trust-tunnel` 的 h3；DoQ / DoH3） |
| **M8 收尾与验收** | P2 项（Shadowsocks 流式旧方法、VMess 旧握手）、Snell v1 ～ v3（需要时）、Snell v6 与 v5 QUIC Proxy Mode · Tailscale 可行性报告（Gecko 已在 M7a 照 Hysteria 官网的公开规范实现，M7-D11）、`W0007` 与 `OutboundError::Unsupported` 退役（M7-D12）、阶段验收清单、文档同步 | FR-OUT-06 / 15 | 第 14 节验收标准全部通过 |
```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| QUIC / HTTP/3 | `quinn`（自定义 UDP 载体供链式拨号、自定义拥塞控制供 Hysteria 2、datagram 供 TUIC / MASQUE）+ `h3` | M7 | 风险 B：`h3` 的 extended CONNECT 与 HTTP Datagram 的成熟度；`ecn` 的支持程度 |
```

换成

```markdown
| QUIC / HTTP/3 | `quinn` 0.11（`runtime-tokio` + `rustls-ring`，M7-D7；自己的 `AsyncUdpSocket` 层供链式载体、端口跳跃与 Hysteria 2 的混淆，datagram 供 Hysteria 2 / TUIC / MASQUE；客户端一律 Cubic，M7-D10）+ 自写的最小 HTTP/3 客户端（`rurge-net::quic::h3`；不用 crates.io 的 `h3`，M7-D5） | M7 | 已验证（M7a）：quinn 0.11.12 的 `AsyncUdpSocket` 可以层层叠加（直连、链式载体、端口跳跃、混淆）；`ecn` 跟随 quinn（`on` 等同 `auto`，M7-D4）；风险 B 按 M7-D5 处理 |
```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
**crate 布局（已决，D4）**：按传输族拆分，把 quinn / h3、russh、boringtun / smoltcp 三棵重依赖树隔开；后续里程碑各自主要只改自己的 crate。
```

换成

```markdown
**crate 布局（已决，D4）**：按传输族拆分，把 quinn、russh、boringtun / smoltcp 三棵重依赖树隔开；后续里程碑各自主要只改自己的 crate。
```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
├── rurge-proto-quic/        # ★ M7：quinn + h3；tuic tuic-v5 hysteria2 masque trust-tunnel(h3)
├── rurge-proto-ssh/         # ★ M4：russh；ssh
├── rurge-proto-wireguard/   # ★ M4：boringtun + smoltcp；wireguard
├── rurge-net/               # + quic 公共件（M7；rurge-dns 的 DoH3 / DoQ 与 rurge-proto-quic 共用）
```

换成

```markdown
├── rurge-proto-quic/        # ★ M7：quinn；QUIC 会话；hysteria2（M7a）tuic tuic-v5 masque trust-tunnel(h3)
├── rurge-proto-ssh/         # ★ M4：russh；ssh
├── rurge-proto-wireguard/   # ★ M4：boringtun + smoltcp；wireguard
├── rurge-net/               # + quic 公共件（M7：UDP 层、拨号与 TLS、自写的 HTTP/3 客户端；rurge-dns 的 DoH3 / DoQ 与 rurge-proto-quic 共用）
```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
`ProxyPolicy` 保持不变（API 的策略详情、`redact_profile`、快照仍基于它）；`PolicySpec` 由它派生。各协议的 spec 随各自里程碑加入：M1 落 `CommonOpts`、`TlsOpts` 与 http / socks5；没轮到的协议继续产生 `W0007`，运行期按 REJECT 处理（阶段 1 的约定），`W0007` 在 M7 结束时消失。`capabilities::current()` 随里程碑增长，`#!REQUIREMENT` 的求值随之正确（PRD Q3）。
```

换成

```markdown
`ProxyPolicy` 保持不变（API 的策略详情、`redact_profile`、快照仍基于它）；`PolicySpec` 由它派生。各协议的 spec 随各自里程碑加入：M1 落 `CommonOpts`、`TlsOpts` 与 http / socks5；没轮到的协议继续产生 `W0007`，运行期按 REJECT 处理（阶段 1 的约定），`W0007` 在 M8 退役（原定 M7 结束时；M7 细化设计 M7-D12：M7 之后仍有 `tailscale`、ss 流式旧方法、vmess 旧握手与 Snell v1 ～ v3 / v6 留给 M8）。`capabilities::current()` 随里程碑增长，`#!REQUIREMENT` 的求值随之正确（PRD Q3）。
```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
`OutboundError` 增加 `Proxy(String)`（代理侧握手 / 鉴权失败，文本形如 `socks5: authentication failed`）、`Tls(String)`、`Unavailable(String)`；`Unsupported` 随 `W0007` 一起在 M7 结束时退役。凭据不出现在任何错误文本与日志里（FR-OBS-01）。
```

换成

```markdown
`OutboundError` 增加 `Proxy(String)`（代理侧握手 / 鉴权失败，文本形如 `socks5: authentication failed`）、`Tls(String)`、`Unavailable(String)`；`Unsupported` 随 `W0007` 一起在 M8 退役（原定 M7 结束时，M7-D12）。凭据不出现在任何错误文本与日志里（FR-OBS-01）。
```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| `trust-tunnel` | `rurge-proto`（h2）/ `rurge-proto-quic`（h3） | M6 / M7 | HTTP/2 或 HTTP/3 | 不支持 |
| `tuic` / `tuic-v5` | `rurge-proto-quic` | M7 | QUIC | 自动 |
| `hysteria2` | `rurge-proto-quic` | M7 | QUIC（+ Salamander） | 自动 |
| `masque` | `rurge-proto-quic` | M7 | HTTP/3 CONNECT + CONNECT-UDP | 自动 |
```

换成

```markdown
| `trust-tunnel` | `rurge-proto`（h2）/ `rurge-proto-quic`（h3） | M6 / M7（h3：M7c） | HTTP/2，或 QUIC 上的 HTTP/3（自写，`rurge-net::quic::h3`） | 不支持 |
| `tuic` / `tuic-v5` | `rurge-proto-quic` | M7（M7b） | QUIC（+ 端口跳跃） | 自动（`native` 数据报，服务端不支持数据报时每个包一条单向流） |
| `hysteria2` | `rurge-proto-quic` | M7（M7a） | QUIC（+ 端口跳跃）（+ Salamander / Gecko），认证经自写的 HTTP/3 | 自动（QUIC 数据报，按需分片） |
| `masque` | `rurge-proto-quic` | M7（M7c） | QUIC 上的 HTTP/3 CONNECT + CONNECT-UDP（自写，`rurge-net::quic::h3`）（+ 端口跳跃） | 自动（每个目标一条 `connect-udp` 流） |
```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| M7 | sing-box（tuic / hysteria2）；MASQUE 与 trust-tunnel 的参考服务端在 M7 细化设计里选定 |
```

换成

```markdown
| M7 | 按 M7 细化设计 M7-D6：sing-box 1.14.2 的 `hysteria2` 入站（无混淆、Salamander、Gecko）与 `tuic` 入站（v5），三平台；官方 tuic 0.8.5（v4；GPL，只作为外部进程运行），只在 Linux CI；Vincent-bin/masque-server v0.13.0（MIT）：`masque`，只在 Linux / macOS CI；TrustTunnel endpoint v1.1.0 的 QUIC 监听：`trust-tunnel` 的 h3，只在 Linux / macOS CI。端口跳跃要在服务端做 DNAT，只有回环假服务端与手工验收。M7a 已接入 sing-box 的 `hysteria2` 入站（TCP、UDP、认证失败、两种混淆） |
```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| B | `h3` 的 extended CONNECT 与 HTTP Datagram 仍属实验特性 | MASQUE 与 trust-tunnel 的 h3 模式 | M7 细化设计先验证；不成熟则该两项延后并登记，不阻塞 TUIC / Hysteria 2 |
```

换成

```markdown
| B | `h3` 的 extended CONNECT 与 HTTP Datagram 仍属实验特性 | MASQUE 与 trust-tunnel 的 h3 模式 | **已处理（2026-10-01，M7-D5）**：验证的结果是不用 crates.io 的 `h3`——h3 0.0.8 / h3-datagram 0.0.2（2025-05 之后没有新版本）普通 CONNECT 也带 `:scheme` / `:path`、HTTP Datagram 的流号恒为 0、写操作被取消时关掉整条连接、读服务端 SETTINGS 要开声明会有破坏性变更的 feature 且没有等待 SETTINGS 的接口；改为在 `rurge-net::quic::h3` 自写最小的 HTTP/3 客户端（M7 细化设计第 4 节），M7a 落了基本请求（Hysteria 2 的认证要用），extended CONNECT 与 HTTP Datagram 在 M7c。残余风险是自写层只对 quic-go 与 quiche 的服务端做过互操作（M7 细化设计第 10 节） |
```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| Q6 | MASQUE 与 trust-tunnel 的参考服务端 | 部分已决（2026-10-01，M6c）：`trust-tunnel` 的 h2 用 TrustTunnel endpoint v1.1.0（Linux / macOS），它也是 `h2-connect` 普通 CONNECT 的参照；MASQUE 与 `trust-tunnel` 的 h3 在 M7 细化设计时选定 |
```

换成

```markdown
| Q6 | MASQUE 与 trust-tunnel 的参考服务端 | 已决（2026-10-01）：`trust-tunnel` 的 h2 用 TrustTunnel endpoint v1.1.0（Linux / macOS；M6c），它也是 `h2-connect` 普通 CONNECT 的参照；M7 细化设计 M7-D6 定下另外两项——MASQUE 用 Vincent-bin/masque-server v0.13.0（MIT，Linux / macOS），`trust-tunnel` 的 h3 用 TrustTunnel endpoint v1.1.0 的 QUIC 监听（Linux / macOS；能否这样启用在写 M7c 的计划时核对，不行就退到假服务端与手工验收） |
```

- [ ] **Step 4: 核对**

- `README.md` 与 `README_en.md` 的状态一段、特性表与路线图内容一致；PRD 与 README 一致。
- 兼容性清单的 `hysteria2` 行与参数行和 P2、P3、P7、P16 ～ P19 的文字一致；文档里引用的错误文本与代码一致。

- [ ] **Step 5: 门禁与提交**

跑门禁。副本上最后一次全工作区门禁：fmt、clippy 通过，`cargo test --workspace --no-fail-fast` 63 个测试二进制 1784 通过、0 失败、3 忽略（吞吐基准与两条既有的忽略）。

```bash
git add tests/interop docs README.md README_en.md CLAUDE.md
git commit -m "docs: M7a QUIC 地基与 Hysteria 2——兼容性清单、手工验收、README、PRD 与 CLAUDE.md；sing-box 的互操作"
```

## 验收对照（设计第 1 节与第 5 节，M7a 部分）

| # | 验收项 | 由谁保证 |
| - | ------ | -------- |
| 1 | `hysteria2` 的 TCP 与 UDP 对参考实现转发通过 | Task 9 的 sing-box 用例（三平台 CI）；Task 5 / 6 的回环往返 |
| 2 | Salamander 与 Gecko | Task 7 的已知答案与端到端；Task 9 的 sing-box 用例 |
| 3 | `port-hopping` | Task 2 / 5 / 7 / 8 的跳跃用例（假服务端同时监听几个端口）；真实服务端靠手工验收 |
| 4 | `ecn`、`tos`、`interface` 与 `underlying-proxy` | Task 2 的各层用例；Task 8 经 socks5 的链 |
| 5 | 能力表不再为 `hysteria2` 出 `W0007` | Task 8 `check_knows_hysteria2` |
| 6 | 门禁全绿 | 各任务的门禁 |
| 7 | 需要真实节点的项目进手工验收清单 | Task 9：`docs/acceptance/phase2-manual.md` 的 M7a 一节 |

## 执行期修正记录

| # | 任务 | 与计划的出入 | 原因 |
| - | ---- | ------------ | ---- |
| 1 | Task 4 | （7c81942）补一条计划没有的用例 `a_dial_its_caller_gave_up_on_is_taken_over_by_a_waiter`：发起单飞拨号的请求放弃（它的 future 被丢掉，如超时）时，等着这次拨号的请求接过拨号，会话随后用它建的连接。只改测试：测试协议 `Plain` 加 `stall_first`（第一次握手不结束）与 `stalled`（卡住时通知） | Task 4 评审（计划规定的做法）：这条分支没有用例，一旦回归，策略会挂到重载为止 |
| 2 | Task 5 | （071b3f3）计划的 `git add` 没有列 `Cargo.lock`（`rurge-proto-quic` 新增 `getrandom` 与 `http` 两条依赖边），按项目要求一并提交 | 计划的疏漏：`Cargo.lock` 要提交 |
| 3 | Task 6 | （a0b5088）每个 UDP 载体一把锁（std `Mutex`，只包住同步的 `transmit`，不跨 `.await`），一个包的全部分片连着交给 QUIC；新用例 `concurrent_sends_keep_each_datagrams_fragments_together`（8 个任务经同一载体并发发 3000 字节的包；去掉锁时 30 次失败 29 次；假服务端本来就照参考实现，每个会话同一时刻只重组一个包号） | Task 6 评审（计划规定的做法）：并发发送的两个分片包的片会交错，参考实现的服务端每个会话只重组一个包号，两个包都会丢——多线程运行时下 1200 字节以上的 QUIC Initial 经 Hysteria 的 UDP 时很常见 |
| 4 | Task 6 | （a0b5088）UDP 载体至多每秒比较一次底层策略的去处：其余发送只看连接是否开着、是否正在离开、是否已被标成换了路（新增 `Session::serves_without_route`）；请求照旧每次比较。新用例 `the_route_is_compared_at_most_once_a_second_on_udp`、`a_carrier_finds_a_changed_route_by_itself`；会话模块文档里"每个数据报都比较"的说法随之改掉，P15 的"UDP 载体的每次发送"以本条为准 | Task 6 评审（计划规定的做法）：经 `underlying-proxy` 的每个数据报都做一次完整的 `ChainConnector::route()`（注册表读取、组解析、分配，`smart` 组还要取全局锁）。代价：没有请求先发现时，UDP 至多晚约 1 秒跟上底层组换的成员 |
| 5 | Task 6 | （328d817）服务端不支持 QUIC 数据报时 `open_udp` 的文本是 `hysteria2: the server does not support QUIC datagrams`，与 `Hysteria-UDP: false` 的 `hysteria2: the server does not allow UDP` 分开；P18 的文字说两种情况同一条文本，计划的代码与用例本来就是两条 | 计划的文字与代码不一致，以代码为准；设计第 15 节第 3 条、兼容性清单与手工验收列出两条文本 |
| 6 | 终审 | （63544a8）加载时对组级 `underlying-proxy` 的组里用了 `port-hopping` 的成员报 `W0023`（沿用"组不得不略去一个成员"的码，不新增诊断码），在组的那一行：`` policy group `G` relays its members through `underlying-proxy`: `M` uses `port-hopping`, which cannot go through it; `M (via R)` is left out ``。成员按注册表装配的顺序取配置本身给的（行上写的、`include-other-group` 与 `include-all-proxies` 取来的，各按组的过滤器）；订阅带来的加载时不知道，仍只记日志。新用例 `a_relayed_group_says_which_member_with_port_hopping_it_leaves_out`；`a_hopping_member_of_a_relayed_group_is_left_out` 加断言加载期报了这条，`check_knows_hysteria2` 断言 `rurge check` 的输出 | 终审（I2）：派生成员 `M (via R)` 构建失败只记日志（M3-D7），成员全被略去时组为空、悄悄按 DIRECT 走（M3-D3），`rurge check` 什么也不说（干构建只建配置自己的策略） |
| 7 | 终审 | （5b5ea5a）`h3::client::Inner::fail` 除了任务记下的 `state.ended`，还看 `quic.close_reason()`：连接已结束而任务还没看到时不再 `close`、也不记 failure。新用例 `a_failure_after_the_end_leaves_how_the_connection_ended`（去掉新条件时失败）；h3 测试模块的 `connected` 与 `BOUND` 改为 `pub(super)` 供它用 | 终审（m1，Task 3 评审的延后记录）：quinn 的 `close()` 会把已记下的结束原因（服务端的关闭、stateless reset）改成 `LocallyClosed` |
| 8 | 终审 | （5b5ea5a）测试的等待加上 `timeout(BOUND, …)`：h3 测试 `connected_with` 的握手 join 与 `pair` 里的 `H3Server::start`、`quic` 测试 `pair` 的握手 join、hysteria2 超时用例 `a_server_that_never_answers_times_out` 的两次 `connect_tcp` 以及同类的 `obfuscation_that_does_not_match_fails_by_the_time_of_the_request` 里的一次 | 终审（m4，Task 3 / 4 / 5 评审的延后记录）：回归时这些用例会挂住而不是失败 |
| 9 | 终审 | （5b5ea5a）`tcp_and_udp_go_through_either_obfuscation` 去掉 Salamander 分支的 `assert!(fragments.is_empty())`，注释指向层自身的用例 `what_goes_out_is_sealed_and_the_handshake_with_gecko_in_fragments`（它看到只用 Salamander 时握手整包发出） | 终审（m5，Task 7 评审的延后记录）：假服务端只在 Gecko 模式下记录分片，这条断言永远不会失败 |
| 10 | 终审 | 兼容性清单：`hysteria2` 行写明 UDP 约 1 秒内跟上底层组换的成员、stateless reset 只在服务端没重启时认得出（quic-go 缺省不发；官方 Hysteria 与 sing-box 每次启动随机生成密钥）、参考实现的服务端丢弃空载荷的 UDP 包；4.3 `underlying-proxy` 行与 4.6 `hysteria2` 参数行登记带跳跃的成员被略去、空组回退与新告警；Shadow TLS 约束行的"还不会真的报出"补上 `tailscale`。设计新增第 15 节「M7a 实施期的订正」；本表与「延后事项」补齐 | 终审（I1、m3）：执行期的偏差没有进计划与设计，清单的几处说法与实现或参考服务端不符 |
| 11 | 门禁 | 最终门禁（终审修正 5b5ea5a 之后）：fmt、clippy 通过；`cargo test --workspace --no-fail-fast` 一轮通过：63 个测试二进制 1790 通过 / 0 失败 / 3 忽略 | 终审修正加了 #6、#7 的两个用例 |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | HTTP/3 应答的 `Content-Length` 与 DATA 总长的比对（RFC 9114 4.1.2）、SETTINGS 0x08 / 0x33 的取值校验（P11） | M7c（DoH3 与 extended CONNECT 是第一批在意它的调用方） |
| 2 | quinn 0.11.12 没有应用层 PING：静默断掉的路径靠保活 10 秒 + 空闲超时 30 秒发现 | 接受；M8 再评估 |
| 3 | 认证或握手反复失败时没有退避（同时到来的请求共用一次拨号） | 有用户报告再说 |
| 4 | Hysteria 2 UDP 的随机包号：同一会话相邻两个分片包的号相同时会被误拼（1/65535，同参考实现） | 接受 |
| 5 | `h3=true` 的 `trust-tunnel` 与 Shadow TLS 组合的 `E0018`（P4） | M7c |
| 6 | Surge 的 `gecko-password` 是否就是公开规范里的 Gecko | 手工验收 |
| 7 | 端口跳跃对真实服务端的互操作（要服务端配 DNAT） | 手工验收 |
| 8 | `ecn` 在纯 TCP 协议上照旧是 `W0029`，而手册说它对纯 TCP 协议无效（`W0028` 更合适） | M8 |
| 9 | CONNECT 的 2xx 之后再来的 HEADERS 眼下当作 trailers 跳过，RFC 9114 4.4 要求报 H3_FRAME_UNEXPECTED（M7c 的隧道要知道自己是 CONNECT） | M7c |
| 10 | `:authority` 保留了 URI 里的 userinfo，RFC 9114 4.3.1 不允许：从配置来的 DoH3 URL 用到它之前去掉或拒绝 | M7c |
| 11 | 手写的 HTTP/3 隧道要先 `flush` 再等对端：写了一半的帧只在写、`flush` 或 `shutdown` 时推进（`copy_bidirectional` 没问题） | M7c |
| 12 | 被淘汰的连接（底层组换了成员、重载、GOAWAY）在最后一个租约放手时立刻 `close`，可能截掉最后一条流还没发出的尾部 | 有用户报告再说（或 M8：有界地等 `stopped()` 再关） |
| 13 | 混淆口令短于 4 字节时参考实现不接受（这样配置的服务端起不来）；rurge 不设下限，写了这样的口令只表现为连接超时（兼容性清单已登记） | M8 再议加载期告警 |
