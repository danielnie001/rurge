# 阶段 2 / M7c「HTTP/3 族与 DNS」Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `masque` 出站（TCP 走 HTTP/3 经典 CONNECT，UDP 走 CONNECT-UDP 与 HTTP/3 数据报）、`trust-tunnel` 的 `h3=true`（HTTP/3 上的 CONNECT，`max-streams` 按每条 QUIC 连接计）、DNS 上游 `quic://`（DoQ）与 `h3://`（DoH3），以及 `encrypted-dns-follow-outbound-mode` 下这两种上游的 QUIC 载体按规则走；建在 M7a 的 QUIC 地基、自写的 HTTP/3 客户端与会话之上。M7 至此完成。

**Architecture:** `rurge-config::spec` 新增 `MasqueSpec`，`TrustTunnelSpec` 带上 `h3`。`rurge-net::quic::h3` 补上扩展 CONNECT（`:protocol`）、HTTP/3 数据报（按 Quarter Stream ID 分发）、SETTINGS 0x08 / 0x33 的取值校验、`content-length` 的核对、隧道上不收第二个 HEADERS、`:authority` 不带 userinfo。`rurge-proto-quic` 新增 `masque`（经典 CONNECT 的 TCP、每个目标一条 CONNECT-UDP 流的 UDP）与 `trust_tunnel`（HTTP/3 版），两者共用一个私有的经典 CONNECT 隧道模块；会话（`Session`）可选地给每条 QUIC 连接设流数上限（照 HTTP/2 会话池的规则），`trust-tunnel` 的 `max-streams` 用它；`FakeH3Proxy` 是两者的回环假服务端。`rurge-dns` 新增 DoQ（RFC 9250，EDNS(0) 填充到 128 字节的整数倍）与 DoH3（RFC 8484 的 POST）两种上游，经 `rurge-net::quic` 自己拨号（主机名经引导解析）。引擎的 `PipelineConnector` 补上 UDP：跟随出站模式时 QUIC 载体作为内部 UDP 会话走规则与策略。引擎工厂装上两种出站，能力表翻转；互操作对 masque-server v0.13.0、TrustTunnel endpoint v1.1.0 的 QUIC 监听、AdGuard dnsproxy v0.86.0 与 CoreDNS v1.14.7。

**Tech Stack:** Rust 1.89 / edition 2024；**不新增外部 crate**：quinn 0.11.12（M7a）、`http`、`bytes`、`base64`、`hickory-proto` 0.26 都已在依赖树里；新增的只有依赖边 `rurge-dns → quinn`（工作区依赖）。

**Spec:** `docs/superpowers/specs/2026-10-02-phase2-m7-quic-design.md`（M7-D1 ～ D12；第 3、4 节共用部分与第 14 ～ 17 节的订正；第 7 节 M7c；第 8、9 节中 M7c 的部分；第 11 节 V5、V6、V7；第 12 节 M7c 草图）。与本计划「计划期决定」表不一致处，以该表为准；执行开始时一并写进设计文档新增的第 18 节。

**基线：** main 11093a9（M7b 已合并）。开工时 main 若有了别的提交，按新的 main 重新生成本计划。

## Global Constraints

- MSRV **1.89**（CI 与本机都钉在 1.89.0），edition 2024。`unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 的两个函数例外（本计划不碰）。**本计划不新增任何 unsafe**。
- 依赖方向不变：`rurge-engine → rurge-proto-quic → { rurge-proto, rurge-net, rurge-config }`；`rurge-dns → rurge-rules → rurge-net`，`rurge-dns` 不依赖 `rurge-proto` / `rurge-proto-quic`。**不新增外部 crate**；不给已有 crate 引入第二个大版本；新增的依赖边只有 `rurge-dns → quinn`。
- **测试绝不碰公网**：只用回环 + 端口 0 + 有界等待；不用固定 sleep 当同步手段（只有"断言这段时间里什么也没发生"本身是被测条件时才等一段固定时间）。**任何带 `url-test` / `fallback` / `load-balance` / `smart` 组的测试配置，`proxy-test-url` 与 `internet-test-url` 都必须指向回环**。回环 UDP 的单个载荷不超过 **9216 字节**（macOS 的缺省上限）。
- **测试与任何命令都不得修改本机的系统代理、注册表、网络设置，不得注册真实服务 / 计划任务**：不在 CLI 测试夹具之外运行 `rurge run --system-proxy`；不运行不带 `--dry-run` 的 `rurge service install | uninstall`。
- **不在本机下载或安装任何东西**——哪怕只是为了算一个校验和也不行（不装 masque-server、dnsproxy、CoreDNS、sing-box、TrustTunnel，不 `rustup target add`、不 `cargo install`）。互操作用例在本机没有二进制时按既有约定跳过。
- **凭据与载荷永不外泄**：`username`、`password`、`proxy-authorization` 的值、`headers` 的值与 TCP / UDP / DNS 的载荷不进日志、错误文本与 `Debug`（配置里用 `Secret<T>`；请求头标为敏感，QPACK 从不索引）。
- 锁里只做内存操作，不跨 `.await`；后台任务（HTTP/3 的连接驱动与数据报分发、UDP 载体的读任务、DNS 连接）随所属的连接或对象结束，不留孤儿任务；网络喂给的队列都有上限、满了即丢。
- M7a 的 quinn 规矩照旧：各层发送失败（`WouldBlock` 以外）一律丢包、不报给 quinn；QUIC 的 TLS 只用 1.3；只对仍开着的连接调用 quinn 的 `close`。M7a 评审的并发教训照旧：UDP 载体至多每秒比较一次底层策略。
- rurge 专有的运行时选项只经命令行与环境变量提供，不扩展 Surge 配置格式（FR-CFG-17）。与 Surge 的每一处行为差异都登记进 `docs/surge-compatibility-matrix.md`（Task 9）。
- 日志、错误文本、CLI 输出、代码注释用英文；文档与提交标题用中文。注释密度、命名、惯用法与相邻代码保持一致；注释里不写评审轮次的标签。
- 提交信息结尾两行：`Co-Authored-By: <执行者自己的署名>` 与 `Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th`。**不 push、不 merge**，不 amend。每个提交都带上它改动的 `Cargo.lock`（若有）。
- 每个任务结束的门禁（全部通过才算完成）：

  ```bash
  RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check \
    && cargo clippy --all-targets -- -D warnings \
    && timeout 1500 cargo test --workspace --no-fail-fast
  ```

  `timeout` 不能省（M3a「延后事项」#20）。测试二进制异常退出而没有失败用例时（`STATUS_ACCESS_VIOLATION`、`STATUS_HEAP_CORRUPTION` / `0xc0000374`、段错误——本机已知的既有问题），或整轮被 `timeout` 杀掉时，重跑一次并保留两次的日志，**不要在任务里去修它**。已知偶发失败的用例（`rurge-dns` 的 `a_partial_result_completes_aaaa_in_the_background`、`bootstrap::tests::stale_entries_are_served_and_refreshed_once` 与 `resolver::tests::a_v4_only_cache_entry_does_not_answer_a_caller_that_wants_aaaa`、`rurge` 的 `run::watch_reloads_rules_on_change` 与 `run::run_system_proxy_is_applied_switched_and_restored`、`rurge-engine` 的 `udp::a_closed_port_does_not_break_the_carrier`、`rurge-external-tests` 的 `engine::the_first_dial_starts_the_program`）同样重跑。**链接器报 LNK1180 / LNK1318 / LNK1285 或 "no space on device" 时是磁盘满了**：先看 `df -h /d`，删 `target/debug/incremental` 与 `target/debug/deps` 里旧的 `*.exe` / `*.pdb` 再重跑。**编译或链接报 "os error 1455"、LNK1102 或 LLVM "out of memory" 时是本机内存被别的程序占满了**：整轮门禁加 `-j 4` 重跑一次（`cargo clippy -j 4 …`、`cargo test -j 4 …`）。
- 第三方 crate 的 API 以本机源码为准：`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/` 下的 `quinn-0.11.12`、`quinn-proto-0.11.19`、`rustls-0.23.43`、`hickory-proto-0.26.2`。
- 本机的 bash 处理不了超过约 8 KB 或含反斜杠的 heredoc（`\\` 会被改写）：新文件与含反斜杠的改动一律用写文件的工具落盘，不用 heredoc。

## Review Focus

设计没有逐条写到、而最可能伤到使用者的五类输入或失败方式；每一条都在负责它的任务里配了用例。

1. **MASQUE 服务端不支持 CONNECT-UDP**（只开了 TCP 的服务端、普通的 HTTP/3 代理、关了 QUIC 数据报的部署）：TCP 应照常，UDP 以说明原因的文本失败，而不是整条连接断掉或包静默消失。用例：Task 4 `a_server_without_connect_udp_fails_the_datagrams_and_carries_tcp`；Task 8 `a_server_without_connect_udp_fails_the_flow_and_carries_tcp`。
2. **凭据写错**：用户应当看到 `masque: proxy authentication required` / `trust-tunnel: authentication failed`，经引擎时 CONNECT 回 502、请求记录写明原因。用例：Task 3 `every_connect_carries_the_credentials_and_none_goes_without_them`、`a_refusal_is_quoted_by_its_status`；Task 5 `a_refused_login_says_authentication_failed`；Task 8 `refused_credentials_fail_the_session`、`a_refused_trust_tunnel_login_over_http3_fails_the_session`。
3. **经 MASQUE 的大 UDP 包**（客户端自己的 QUIC Initial 就有 1200 字节以上，外层 HTTP/3 数据报只放得下约 1.2–1.4 KB）：放不下的那一个包被拒、什么也不发，流照常。用例：Task 4 `a_datagram_too_long_is_refused_and_nothing_goes_out`；Task 8 `a_packet_too_long_for_a_datagram_is_dropped_alone`。
4. **DoQ / DoH3 服务端关掉空闲连接或重启**（CoreDNS 空闲 10 秒、dnsproxy 30 秒即关）：下一个查询要照常得到应答，而不是先失败一次。用例：Task 6 `an_idle_connection_is_replaced`、DoQ 与 DoH3 各自的 `a_restarted_server_gets_the_query_again_on_a_new_connection`。
5. **跟随出站模式把 DoQ / DoH3 送进不载 UDP 的代理或 REJECT**：DNS 不能整个断掉——REJECT 照 TCP 的做法直连，不载 UDP 的代理只让这个上游失败、其它上游照常应答。用例：Task 7 `a_doq_server_routed_to_a_proxy_without_udp_fails_while_another_answers`、`a_doq_carrier_routed_to_reject_goes_directly`。

## 计划期决定

写计划时对照设计、RFC 9114 / 9220 / 9297 / 9298 / 9250 / 8484 与 draft-ietf-httpbis-connect-tcp-14 的原文、Surge 手册（`policies/masque.html`、`policies/trust-tunnel.html`、`dns/encrypted-dns.html` 等，2026-10-06 查阅）与发布说明、masque-server v0.13.0、TrustTunnel endpoint v1.1.0、quic-go/masque-go、sing-box、AdGuard dnsproxy、hickory-dns、CoreDNS 的源码与文档（只读）、quinn 0.11.12 / rustls 0.23.43 / hickory-proto 0.26.2 的本机源码与本仓库源码后定下的事；与设计文档文字不同的，写进设计文档第 18 节。项目所有者交代（2026-10-06）：一切决定自行做出、不再询问，多查 RFC、Surge 手册与 GitHub 上的开源实现，对标行业做法。

**本计划里的代码不是凭空写的。** 全部 9 个任务的改动在仓库的一份副本上按任务顺序真实做了一遍（副本建在 11093a9 上，用自己的构建目录），最后一次全工作区门禁见 Task 9 的 Step 5。计划里新文件的全文取自副本上该任务的提交，修改处的"把 … 换成 …"由脚本从相邻两个任务提交的差异生成，并在拼好之后按计划的顺序套到开工前的源码上逐字核对过。有单独失败步骤的任务，Step 2 的"预期失败"是只把该任务 Step 1 的内容套到上一个任务的状态上、真实跑出来的；自带用例的新模块与文档没有单独的失败步骤。执行时按 M7a / M7b 的做法用转写脚本把每一步的代码块原样写入，不手工重打。

| # | 事项 | 决定与依据 |
| - | ---- | ---------- |
| P1 | `masque` 的配置（细化设计 7.1） | `MasqueSpec { tls, quic, credentials: Option<MasqueCredentials { username, password }> }`（都是 `Secret`）；TLS 经 `read_quic_tls`（ALPN 缺省 `h3`）、QUIC 经 `read_quic`；凭据只认命名写法（手册的语法行没有位置写法），两个都不写为 `None`；只写一个是 `E0018` "`password` is required with `username`"（反之同理），写了但为空是 "`x` is empty"，都不引用取值；`udp-relay`、`tfo` 是 `W0028`，`ecn` 生效；手册没列的 `headers`、`max-streams` 是 `W0001`；`port-hopping` × `underlying-proxy` 与 Shadow TLS × `masque` 的两条 `E0018` 照常；组中继里带 `port-hopping` 的 `masque` 成员加载时报 `W0023` |
| P2 | `trust-tunnel` 的 `h3=true`（细化设计 7.2，M7a 延后事项 #5） | `TrustTunnelSpec.h3`；`read_trust_tunnel(r, &mut common, keystore) -> TrustTunnelSpec`（去掉 `TrustTunnelRead`）；先读 `h3`，ALPN 按传输钉住：没有 `h3` 是 `h2`、有 `h3` 是 `h3`，写别的是 `W0028`（"… without `h3=true`" / "… with `h3=true`"）；任何 `trust-tunnel` 行上的 `port-hopping` / `port-hopping-interval` 是 `W0028`（手册没给它列）；`h3=true` 时 `ecn` 生效、`tfo` 是 `W0028`、`headers` 的告警说 HTTP/3、Shadow TLS 是 `E0018`（`shadow_tls::refuse`，只报一条）；`ProtoSpec::quic()` 对 `trust-tunnel` 恒为 `None` |
| P3 | 分步落地的门 | 同 M7a / M7b：Task 1 里 `masque` 行读完并检查但暂不产出 spec、引擎先放 `BuildError` 分支；`h3=true` 仍是 `W0029`、引擎照旧建 HTTP/2 出站，并临时把 ALPN 换回 `h2`（spec 已钉成 `h3`，不换就是 `NoApplicationProtocol`）；Task 8 全部去掉 |
| P4 | HTTP/3 客户端的两种构造（细化设计第 4 节） | `Client::new` 照旧不宣告 `H3_DATAGRAM`（Hysteria 2 用它——Surge 6.9.1 修过"HTTP/3 Datagram 的协商干扰了 Hysteria 的 UDP 转发"）；新的 `Client::with_datagrams` 宣告 `H3_DATAGRAM = 1`，并按 Quarter Stream ID 分发连接的 QUIC 数据报 |
| P5 | 扩展 CONNECT（RFC 9220 / RFC 8441） | 请求扩展 `h3::Protocol`（`CONNECT_UDP`）；带它时 HEADERS 是 `:method :protocol :scheme :authority :path`；`request()` 先等服务端的 SETTINGS（调用方限时），没有 `SETTINGS_ENABLE_CONNECT_PROTOCOL = 1` 就以 `Unsupported` "h3: the server does not support extended CONNECT" 拒绝、什么也不发 |
| P6 | HTTP/3 数据报（RFC 9297） | `RequestStream::datagrams()`（异步、等 SETTINGS、每条流只取一次）给出 `h3::Datagrams { max_payload, send, recv }`；要服务端 `H3_DATAGRAM = 1` 且协商出 QUIC 数据报，否则 `Unsupported` "h3: the server does not support HTTP datagrams"（服务端宣告了而 QUIC 层没有时只是不可用，不关连接——宽松）；放不下的负载 `InvalidInput`、什么也不发；收到的按流号分发，每条流至多排队 64 个、满了即丢；太短或流号越界是连接错误 `H3_DATAGRAM_ERROR`；没有登记的流的数据报静默丢弃（宽松：RFC 9297 §2 要求中止该流）；流的任一方向结束时它的数据报随之结束 |
| P7 | 更严的应答（M7a 延后事项 #1、#9、#10、#11） | SETTINGS 0x08 / 0x33 的取值只能是 0 或 1，否则 `H3_SETTINGS_ERROR`；非 CONNECT 请求的应答按 `content-length` 核对 DATA 总长（多了立即、少了在 FIN 时以 `H3_MESSAGE_ERROR` 报畸形；HEAD、204、304 不计；相同的重复值照收，同 quic-go）；CONNECT 的 2xx 忽略 `content-length`，之后再来 HEADERS 是连接错误 `H3_FRAME_UNEXPECTED`；非 2xx 的 CONNECT 应答按普通应答处理（RFC 9110 §9.3.6）；`:authority` 去掉 userinfo；`RequestStream::id()`；隧道的写"半帧只在写、flush、shutdown 时推进"写进文档，DoH3 与 MASQUE 照做 |
| P8 | MASQUE 的 TCP（细化设计 7.1） | 经典 HTTP/3 CONNECT（手册："Surge relays TCP connections as HTTP/3 CONNECT tunnels multiplexed over a single QUIC connection"；`connect-tcp` 仍是草案 -14，masque-server 对它回 404，业界客户端都不用）：`:method CONNECT`、`:authority host:port`（IPv6 加方括号、A-label）；有凭据时每个 CONNECT 都带 `proxy-authorization: Basic …`（敏感；手册没写字段名，`Proxy-Authorization` 是代理语义，masque-server 与 volto 都认它——rurge 的选择）；2xx 即隧道，407 → `masque: proxy authentication required`，其它 → `masque: the proxy answered <code>`；应答到达之前连接已失效（关闭、going away，包括 GOAWAY 之后被 `H3_REQUEST_REJECTED` 拒绝的请求）算 `Failure::Lost`，照会话的规则在新连接上重试一次；服务端的流额满时等流释放（受时限约束），不另拨连接 |
| P9 | MASQUE 的 UDP（细化设计 7.1；订正"放不下改用 capsule"） | 每个目标一条 CONNECT-UDP 流（对称型，同 `h2-connect`）：`:protocol connect-udp`、`:scheme https`、`:authority` 为 TLS 名字加策略端口、`:path` 为 RFC 9298 的缺省模板（IPv6 不带方括号、`:` 写成 `%3A`）、`capsule-protocol: ?1`、凭据同 TCP；载体惰性打开，每个目标的流随它的第一个包建立；等 2xx 才发包（不乐观发送，同 masque-go）；2xx 却带 `content-length` / `content-type` / `transfer-encoding` 或是 204 / 205 / 206 → `masque: the proxy's response to CONNECT-UDP is malformed`；服务端没有扩展 CONNECT、`H3_DATAGRAM = 1` 或 QUIC 数据报时 UDP 以 `masque: the server does not support CONNECT-UDP` 失败而 TCP 照常（登记的差异：Surge 在建立连接时检查）；包是 `varint(0) ‖ 负载`；超过 65527 或此刻放不下的负载以 `InvalidInput` "masque: a datagram longer than <n> bytes" 拒绝、什么也不发，**不改用 DATAGRAM capsule**（RFC 9297 §3.5、RFC 9298 §6.1 为保住端到端的不可靠性而不转 capsule；masque-server 在 HTTP/3 上也不读请求流里的 capsule）；收包两种都收（context 0 的 HTTP 数据报与流上的 DATAGRAM capsule），其它 context 与未知 capsule 丢弃；每个目标空闲 60 秒以 FIN 关流；被服务端立刻结束的流每个包至多重开一次；连接关闭、going away 或底层策略变了（至多每秒比较一次）时在会话当前的连接上重开 |
| P10 | 共用的代码 | `rurge_proto::http::basic_authorization`（pub；h2-connect、trust-tunnel、MASQUE 共用）；`rurge_proto::connect_udp`（capsule 编解码与 `masque_path`，h2-connect 与 MASQUE 共用，错误文本的前缀由调用方给出；原 `h2connect/capsule.rs` 删去）；`rurge_proto::build::tls_name` 改为 pub；`rurge_proto::trust_tunnel::{connect_request, refused}`（pub，HTTP/2 与 HTTP/3 共用请求与拒绝的措辞；请求仍标 HTTP/2——自写的 h3 客户端不读版本）；`rurge_proto_quic::connect`（私有）：经典 CONNECT 的隧道与按连接状态判 `Lost`，MASQUE 与 `trust-tunnel` 共用 |
| P11 | 会话的流数上限（细化设计 3.5） | `Session::with_stream_cap(self, cap)`（`cap ≥ 1`）；一个租约即一条流；有上限时选最早的、开着、没在离开、在路由上且租约不足 `cap` 的连接；都满时，在途拨号还有余位（每次在途拨号算 `cap` 个位置）就等，否则再拨，同时至多 8 条在途；拨号的失败照 HTTP/2 会话池的整体规则交给等它的请求；空闲回收、GOAWAY、路由变化与失效重试按每条连接照旧；不设上限时行为与原来完全相同 |
| P12 | `trust-tunnel` 的 HTTP/3（细化设计 7.2，M6c 延后事项 #1） | `TrustTunnelH3Outbound`：经典 CONNECT（只有 `:method` 与 `:authority`——endpoint 对带 `:scheme` 或 `:path` 的回 400）、`proxy-authorization` Basic、`user-agent: rurge`、`headers=` 按名替换；407 → `trust-tunnel: authentication failed`，其它 → `trust-tunnel: the server answered <code>`；ALPN `h3`、`Client::new`（endpoint 没开 QUIC 数据报）、`ecn` 取策略的；`max-streams` 是每条 QUIC 连接的流数上限（P11）；不看服务端的 QUIC 流上限（quinn 不公开，`open_bi` 在时限内等额度；HTTP/2 版取两者较小者——登记）；不载 UDP；不回落 HTTP/2（手册："instead of HTTP/2 over TLS"） |
| P13 | DNS 上游的写法（细化设计 7.3） | `UpstreamSpec::Quic { host, port }`（缺省 853，只有主机与端口，结尾单独一个 `/` 照收）与 `UpstreamSpec::H3(Url)`（存成 https 形式、缺省 443；路径缺省或为 `/` 时用 `/dns-query`——sing-box、hickory、CoreDNS 都是这样，Surge 没写明；`https://` 不变）；`h3://` 带 userinfo 在解析时拒绝（错误文本不含口令）；`[Host]` 的 `server:` 同样接受；这两种 scheme 不再报 `W0026` |
| P14 | DNS 的 QUIC 拨号与连接 | TLS 1.3、ALPN 只发 `doq` / `h3`（RFC 9250；草案期的 `doq-i0x` 线上格式不同）、根证书与 `encrypted-dns-skip-cert-verification` 同其它加密上游（`rurge_net` 提供的 TLS 1.3 配置）；主机名经引导解析、IPv4 在前逐个地址试、以 IP 目标经 `Path::open`（`BootstrapConnector` 补上 `as_direct` 与 `connect_udp`）；每个上游一条连接、单飞拨号、已关闭的先换；连接在查询开始时已就绪而查询以连接级错误失败时，在新连接上重试一次（DNS 查询可以重问，不管数据是否已发出——同 dnsproxy / hickory）；DNS 连接不保活（`Transport::DNS`；`Transport::keep_alive` 改为 `Option`），空闲到期自然关闭、下次再拨；不用 0-RTT；`ServerPort` 移到 `rurge_net::quic::testing` 供 rurge-dns 的假服务端重启用 |
| P15 | DoQ（RFC 9250） | 每个查询一条新的双向流，`u16 长度 ‖ 报文`，报文 ID 置 0 并加 EDNS(0) 填充（选项 12）到 128 字节的整数倍（RFC 9250 §5.4 的 MUST + RFC 8467；读过的业界客户端都不填，rurge 照 RFC），已有的填充与 keepalive 选项换掉，写完即 FIN；应答读到流尾（至多 2 + 65535）、恢复原 ID；应答 ID 不为 0、在中途结束或之后还有字节是协议错误：查询失败并以 `DOQ_PROTOCOL_ERROR`（0x2）关连接；被放弃的查询以 `DOQ_REQUEST_CANCELLED`（0x3）停流；服务端重置流只让该查询失败；宽松：服务端的 STOP_SENDING、应答里的 keepalive、服务端开的单向流不追究 |
| P16 | DoH3（RFC 8484 over HTTP/3） | `POST` 到 URL 的路径，`:authority` 在 443 时不带端口，`content-type` / `accept: application/dns-message` 与 `content-length`，正文是 ID 置 0 的查询，写完即 FIN；成功是 200 且内容类型以 `application/dns-message` 开头、正文至多 65535 字节（`content-length` 由 h3 层核对）；GOAWAY 之后下一个查询用新连接；失效的判定按失败之后连接的状态（同 MASQUE）；不填充（同 DoH） |
| P17 | 跟随出站模式下的 QUIC 载体（细化设计 7.3，V6） | `PipelineConnector::connect_udp`：名字目标或引擎尚未接上时走 fallback（直连）；否则建内部 UDP 会话（`Internal`、`Transport::Udp`，端口 443 标 DOH3、其余标 DOQ——按端口推断，DoH3 不在 443 时被标成 DOQ，登记），经新的 `Engine::dial_internal_udp` 按规则选策略：DIRECT 用 DIRECT 出站自己的 UDP（与 TCP 的 DIRECT 一样带 interface / tos / hook 的设置）；链底是 REJECT（含未实现的协议与组环）或代理服务器写的是名字时，照 TCP 的做法告警后直连；载 UDP 的代理用 `open_udp` + `packet_datagram`；不载 UDP 的代理让这个上游这次失败（`via {策略}: policy does not support UDP`），不改走直连；交给解析器的错误一律带 `via {策略}: `；内部 UDP 会话进活动索引与请求记录、计两向字节、随载体结束；`dial_internal` 的前半段与 smart 回报抽成 `route_internal` / `report_internal`，TCP 的行为不变 |
| P18 | 引擎与能力表（Task 8） | 工厂两个分支：`ProtoSpec::Masque` → `MasqueOutbound`；`ProtoSpec::TrustTunnel(tt) if tt.h3` → `TrustTunnelH3Outbound`，否则 HTTP/2 原样；能力表加 `masque`，`CORE_VERSION` 不变（此前各次协议翻转都没动它）；`h3=true` 的 `W0029` 退役；干构建不起任务 |
| P19 | 测试的假服务端 | `FakeH3Proxy`（quinn + `H3Server`；仿 masque-server：quiche 式的 GREASE、407 带 `proxy-authenticate`、`H3_DATAGRAM` 与 QUIC 数据报的开关、CONNECT-UDP；仿 TrustTunnel endpoint：不宣告扩展 CONNECT、不收数据报）；rurge-dns 的 `FakeDoq` / `FakeDoh3`（共用 `MockDns` 的应答表）；假服务端的编解码都独立实现，不用客户端的代码 |
| P20 | 互操作（M7-D6、V5） | masque-server v0.13.0（只有 Linux 版，`RURGE_TEST_MASQUE_SERVER`）；TrustTunnel endpoint v1.1.0 加 `[listen_protocols.quic]`（Linux / macOS，CI 已装）；DNS 另加 AdGuard dnsproxy v0.86.0 与 CoreDNS v1.14.7（三平台，`RURGE_TEST_DNSPROXY` / `RURGE_TEST_COREDNS`；设计只把公共解析服务列进手工验收——这里另加两个回环上的参照服务端，CoreDNS 对非 0 ID 与 keepalive 最严格）；sing-box 1.15 alpha 的 HTTP/3 CONNECT / CONNECT-UDP 入站暂不用（alpha） |

## 承接事项

| # | 来源 | 事项 | 处理 | 任务 |
| - | ---- | ---- | ---- | ---- |
| C1 | M7 设计第 12 节 | M7c 的全部内容 | 本计划 | 1–9 |
| C2 | M7a 延后事项 #1 | 应答 `content-length` 与 DATA 总长的核对、SETTINGS 0x08 / 0x33 的取值校验 | 本计划（P7） | 2 |
| C3 | M7a 延后事项 #5 | `h3=true` 的 `trust-tunnel` 与 Shadow TLS 的 `E0018` | 本计划（P2） | 1 |
| C4 | M7a 延后事项 #9 | CONNECT 的 2xx 之后再来 HEADERS 应报 `H3_FRAME_UNEXPECTED` | 本计划（P7） | 2 |
| C5 | M7a 延后事项 #10 | `:authority` 不得带 userinfo | 本计划（P7、P13） | 2、6 |
| C6 | M7a 延后事项 #11 | 手写的 HTTP/3 隧道要先 `flush` 再等对端 | 文档写明，DoH3 与 MASQUE 照做（P7） | 2、4、6 |
| C7 | M7b 延后事项 #9 | `tuic` 与 `hysteria2` 的载体辅助与错误映射几乎逐字相同，"第三个 QUIC 协议落地时抽出" | 不抽：MASQUE 的 UDP 是每个目标一条 HTTP/3 流，与那两者"一个载体一个会话"的结构不同，三者没有共同的部分可抽；转入本计划「延后事项」 | — |
| C8 | M6c 延后事项 #1 | `trust-tunnel` 的 `h3=true` | 本计划（P2、P12） | 1、5、8 |
| C9 | M3b #7 | 测试二进制偶发崩溃 | 照旧：门禁遇到就重跑 | — |

## File Structure

新建：

| 文件 | 职责 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/spec/masque.rs` | `MasqueSpec`、`MasqueCredentials`、`read_masque`（与用例） | 1 |
| `crates/rurge-net/src/quic/h3/datagram.rs` | HTTP/3 数据报的分发与句柄（与用例） | 2 |
| `crates/rurge-proto-quic/src/masque/{mod.rs, tcp.rs}` | `MasqueOutbound`、经典 CONNECT 的请求（与用例） | 3、4、5 |
| `crates/rurge-proto-quic/src/masque/udp.rs` | CONNECT-UDP 的载体（与用例） | 4、5 |
| `crates/rurge-proto-quic/src/testing/h3proxy.rs` | `FakeH3Proxy`（MASQUE 与 `trust-tunnel` 的假服务端） | 3、4、5 |
| `crates/rurge-proto/src/connect_udp.rs` | CONNECT-UDP 的 capsule 编解码与 `masque_path`（自 `h2connect` 移来，与用例） | 4 |
| `crates/rurge-proto-quic/src/connect.rs` | 经典 CONNECT 的隧道（MASQUE 与 `trust-tunnel` 共用） | 5 |
| `crates/rurge-proto-quic/src/trust_tunnel.rs` | `TrustTunnelH3Outbound`（与用例） | 5 |
| `crates/rurge-dns/src/upstream/{doq.rs, doh3.rs, session.rs}` | DoQ、DoH3 上游与它们的 QUIC 连接（与用例） | 6 |
| `crates/rurge-dns/src/testing/quic.rs` | `FakeDoq`、`FakeDoh3` | 6 |
| `crates/rurge-engine/tests/outbounds_masque.rs` | 经引擎的 MASQUE 用例 | 8 |
| `tests/interop/src/{masque_server.rs, dnsproxy.rs, coredns.rs}`、`tests/interop/tests/{masque.rs, dns.rs}` | 参照服务端的夹具与互操作用例 | 9 |

删除：`crates/rurge-proto/src/h2connect/capsule.rs`（内容移到 `connect_udp.rs`，Task 4）。

修改：`crates/rurge-config/src/{spec/mod.rs, spec/h2.rs, spec/shadow_tls.rs, redact.rs, diagnostic.rs}` 与配置的用例（1、6、8）；`crates/rurge-net/src/quic/h3/{mod.rs, client.rs, frame.rs, stream.rs, testing.rs}`（2）、`src/{http.rs, quic/dial.rs, quic/mod.rs, quic/testing.rs}`（6）；`crates/rurge-proto/src/{http.rs, h2connect/*, trust_tunnel.rs, build.rs, lib.rs}`（1、3、4、5）；`crates/rurge-proto-quic/src/{lib.rs, session.rs, testing/mod.rs}`（3、4、5、6）；`crates/rurge-dns/{Cargo.toml, src/*, tests/resolver.rs}`（6）；`crates/rurge-engine/src/{outbounds.rs, dns_pipeline.rs, engine.rs, runtime.rs, stack.rs}` 与 `tests/{pipeline.rs, common/mod.rs, outbounds_http2.rs}`（1、6、7、8）；`crates/rurge/{src/capabilities.rs, src/cli/dns.rs, tests/cli.rs}`（6、8）；`tests/interop/{Cargo.toml, README.md, src/lib.rs, src/trusttunnel.rs, tests/http2.rs}`、`.github/workflows/ci.yml` 与文档（9）。

## 任务一览

| 任务 | 交付物 | 依赖 |
| ---- | ------ | ---- |
| 1 | `MasqueSpec`、`trust-tunnel` 的 `h3`、两条新的 `E0018` / `W0028` | — |
| 2 | HTTP/3：扩展 CONNECT、HTTP 数据报、更严的应答 | — |
| 3 | MASQUE 的 TCP、`FakeH3Proxy` | 1、2 |
| 4 | MASQUE 的 UDP（CONNECT-UDP） | 3 |
| 5 | 会话的流数上限、`trust-tunnel` 的 HTTP/3 | 1–4 |
| 6 | DoQ 与 DoH3 上游 | 2 |
| 7 | 跟随出站模式下 QUIC 载体按规则走 | 6 |
| 8 | 引擎装配、能力表翻转、经引擎的用例 | 1–7 |
| 9 | 互操作（masque-server、TrustTunnel h3、dnsproxy、CoreDNS）、CI 与文档 | 1–8 |

---

### Task 1: `masque` 与 `trust-tunnel` 的 `h3=true` 的配置

`MasqueSpec`（P1）；`TrustTunnelSpec.h3` 与按传输钉住的 ALPN、`port-hopping` 的 `W0028`、`h3=true` × Shadow TLS 的 `E0018`（P2）；`masque` 行读完并检查但暂不产出 spec，`h3=true` 的行照旧经 HTTP/2 连（P3）。

**Files:**
- Create: `crates/rurge-config/src/spec/masque.rs`（自带用例）
- Modify: `crates/rurge-config/src/spec/{mod.rs, h2.rs, shadow_tls.rs}`（与用例）、`src/redact.rs`（用例）、`tests/policy_spec.rs`、`crates/rurge-proto/src/trust_tunnel.rs`（用例的调用点）、`crates/rurge-engine/src/outbounds.rs`（与用例）

**Interfaces:**
- Consumes: M7a 的 `read_quic_tls` / `read_quic`、`QuicOpts`；`required` 与 `ParamReader`；`shadow_tls::allowed_on`。
- Produces: `rurge_config::spec::masque`：`pub struct MasqueSpec { pub tls: TlsOpts, pub quic: QuicOpts, pub credentials: Option<MasqueCredentials> }`、`pub struct MasqueCredentials { pub username: Secret<String>, pub password: Secret<String> }`、`pub fn read_masque(r: &mut ParamReader<'_>, common: &mut CommonOpts, keystore: &[KeystoreItem]) -> MasqueSpec`；`ProtoSpec::Masque(MasqueSpec)`（`tls()` / `quic()` 各有分支）；`TrustTunnelSpec { …, pub h3: bool }` 与 `pub fn read_trust_tunnel(r: &mut ParamReader<'_>, common: &mut CommonOpts, keystore: &[KeystoreItem]) -> TrustTunnelSpec`（`TrustTunnelRead` 去掉）；`shadow_tls::refuse`

- [ ] **Step 1: 先写用例**

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            max_streams: h2::DEFAULT_MAX_STREAMS,
        });
```

换成

```rust
            max_streams: h2::DEFAULT_MAX_STREAMS,
            h3: false,
        });
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            assert!(tuic.quic().is_some_and(|quic| quic.port_hopping.is_some()));
        }
    }
}

```

换成

```rust
            assert!(tuic.quic().is_some_and(|quic| quic.port_hopping.is_some()));
        }
    }

    /// `masque` lines are read and checked in full; they have no spec until
    /// the engine builds MASQUE (M7c task 8). `ecn` takes effect on them,
    /// `tfo` and `udp-relay` do not apply, the credentials go together, and
    /// what the manual does not list is unknown (phase 2 M7 design 3.1,
    /// 7.1).
    #[test]
    fn a_masque_line_is_checked_but_has_no_spec_yet() {
        for def in [
            "masque, example.com, 443, username=user, password=pass",
            "masque, h.test, 443",
            "masque, h.test, 443, username=u, password=pw, port-hopping=1234;5000-6000, port-hopping-interval=10, ecn=off, alpn=h3, sni=edge.test, client-cert=cert1",
            "masque, h.test, 443, ecn=on, underlying-proxy=Pick",
            // DIRECT is no chain: the hops stay
            "masque, h.test, 443, port-hopping=5000-6000, underlying-proxy=DIRECT",
        ] {
            let o = outcome("M", def);
            assert!(o.diagnostics.is_empty(), "{def}: {:?}", o.diagnostics);
            assert!(o.inert.is_empty(), "{def}: {:?}", o.inert);
            assert!(o.spec.is_none() && o.not_implemented.is_none(), "{def}");
        }
        let o = outcome(
            "M",
            "masque, h.test, 443, s3cret, tfo=true, udp-relay=true, username=s3cretUser, max-streams=5, headers=X-A:1",
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
                    "policy `M`: `tfo` does not apply to `masque` policies; ignored"
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `M`: `password` is required with `username`"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `M`: `udp-relay` does not apply to `masque` policies (UDP is always relayed); ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `M`: unknown parameter `max-streams` ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `M`: unknown parameter `headers` ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `M`: unexpected positional value #1 ignored"
                ),
            ]
        );
        assert!(o.inert.is_empty(), "{:?}", o.inert);
        assert!(o.spec.is_none());
    }

    /// What a QUIC-based line cannot combine (phase 2 M7 design 3.1), on
    /// `masque` as on the others: port hopping with a chain, and Shadow TLS
    /// (manual: it "cannot be combined with QUIC-based protocols").
    #[test]
    fn port_hopping_over_a_chain_and_shadow_tls_are_errors_on_masque() {
        for relay in ["Entry", "Pick"] {
            let o = outcome(
                "M",
                &format!("masque, h.test, 443, port-hopping=5000-6000, underlying-proxy={relay}"),
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
                    "policy `M`: `port-hopping` cannot be combined with `underlying-proxy`"
                )],
                "{relay}"
            );
        }
        let o = outcome(
            "M",
            "masque, h.test, 443, username=u, password=pw, shadow-tls-password=st, shadow-tls-version=3, shadow-tls-sni=site.test",
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
                "policy `M`: Shadow TLS cannot be combined with a `masque` policy"
            )]
        );
    }

    /// MASQUE runs TLS over QUIC: the TLS options, and with them the client
    /// certificate, and the QUIC options are its own.
    #[test]
    fn a_masque_spec_has_tls_and_quic_options() {
        let masque = ProtoSpec::Masque(MasqueSpec {
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
            credentials: Some(MasqueCredentials {
                username: "u".into(),
                password: "pw".into(),
            }),
        });
        assert_eq!(masque.keystore_item(), Some("cert1"));
        assert_eq!(
            masque.tls().map(|tls| tls.alpn.clone()),
            Some(vec!["h3".to_string()])
        );
        assert!(
            masque
                .quic()
                .is_some_and(|quic| quic.port_hopping.is_some())
        );
    }

    /// `h3=true` runs a `trust-tunnel` over QUIC (phase 2 M7 design 7.2):
    /// the spec says so and pins the ALPN to `h3`; `ecn` takes effect, `tfo`
    /// does not apply, and port hopping is no parameter of it. Until the
    /// engine builds it (M7c task 8) the load says `h3` has no effect yet,
    /// and the policy connects over HTTP/2.
    #[test]
    fn a_trust_tunnel_line_with_h3_runs_over_quic() {
        let o = outcome(
            "T",
            "trust-tunnel, h.test, 443, username=u, password=p, h3=true, alpn=h2, ecn=off, tfo=true, port-hopping=5000-6000",
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
                    "policy `T`: `alpn` is always `h3` on `trust-tunnel` policies with `h3=true`; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `T`: `tfo` does not apply to `trust-tunnel` policies with `h3=true`; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `T`: `port-hopping` does not apply to `trust-tunnel` policies; ignored"
                ),
            ]
        );
        assert_eq!(o.inert, ["h3"]);
        let spec = o.spec.expect("a spec");
        let ProtoSpec::TrustTunnel(tt) = &spec.proto else {
            panic!("{:?}", spec.proto)
        };
        assert!(tt.h3);
        assert_eq!(tt.tls.alpn, ["h3"]);
        assert_eq!((spec.common.ecn, spec.common.tfo), (Tristate::Off, false));
        // no QUIC options of its own: port hopping does not apply
        assert!(spec.proto.quic().is_none());

        // over HTTP/2 `ecn` and `tfo` are parsed but have no effect yet
        let o = outcome(
            "T",
            "trust-tunnel, h.test, 443, username=u, password=p, ecn=off, tfo=true",
        );
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        assert_eq!(o.inert, ["tfo", "ecn"]);
        let spec = o.spec.expect("a spec");
        let ProtoSpec::TrustTunnel(tt) = &spec.proto else {
            panic!("{:?}", spec.proto)
        };
        assert!(!tt.h3);
        assert_eq!(tt.tls.alpn, ["h2"]);
    }

    /// Shadow TLS wraps a TCP connection: a `trust-tunnel` has it over
    /// HTTP/2, not over HTTP/3 (phase 2 M7 design 3.1). One error, whatever
    /// the Shadow TLS parameters say.
    #[test]
    fn shadow_tls_is_an_error_on_a_trust_tunnel_with_h3() {
        for layer in [
            "shadow-tls-password=st, shadow-tls-version=3, shadow-tls-sni=site.test",
            // errors of their own over HTTP/2
            "shadow-tls-password=, shadow-tls-version=3",
        ] {
            let o = outcome(
                "T",
                &format!("trust-tunnel, h.test, 443, username=u, password=p, h3=true, {layer}"),
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
                    "policy `T`: Shadow TLS cannot be combined with `h3=true` on a `trust-tunnel` policy"
                )],
                "{layer}"
            );
            assert!(o.spec.is_none(), "{layer}");
        }
        // over HTTP/2 the layer is the policy's
        let o = outcome(
            "T",
            "trust-tunnel, h.test, 443, username=u, password=p, h3=false, shadow-tls-password=st",
        );
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        assert!(o.spec.and_then(|spec| spec.shadow_tls).is_some());
    }
}

```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
    use crate::span::Span;
```

换成

```rust
    use crate::span::Span;
    use crate::spec::common::{Applies, Notes, read_common};
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
    fn tt(def: &str) -> (TrustTunnelRead, bool, Vec<Diagnostic>) {
        reader_for(def, |r| read_trust_tunnel(r, &[]))
```

换成

```rust
    fn tt(def: &str) -> (TrustTunnelSpec, bool, Vec<Diagnostic>) {
        let ((spec, _), failed, diags) = tt_and_common(def);
        (spec, failed, diags)
    }

    /// `tt`, and the common parameters as the reader leaves them.
    fn tt_and_common(def: &str) -> ((TrustTunnelSpec, CommonOpts), bool, Vec<Diagnostic>) {
        reader_for(def, |r| {
            let mut common = read_common(r, Applies::Proxy, &mut Notes::default());
            let spec = read_trust_tunnel(r, &mut common, &[]);
            (spec, common)
        })
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
            let (got, failed, _) = tt(&format!(
```

换成

```rust
            let (spec, failed, _) = tt(&format!(
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
            assert_eq!(got.spec.max_streams, DEFAULT_MAX_STREAMS);
```

换成

```rust
            assert_eq!(spec.max_streams, DEFAULT_MAX_STREAMS);
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
        let (got, failed, diags) =
            tt("trust-tunnel, 192.168.20.62, 443, username=test, password=test");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert!(!got.h3);
        let spec = got.spec;
```

换成

```rust
        let (spec, failed, diags) =
            tt("trust-tunnel, 192.168.20.62, 443, username=test, password=test");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert!(!spec.h3);
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
        assert_eq!(spec.tls.alpn, ["h2"]);
        let (got, failed, diags) = tt(
```

换成

```rust
        assert_eq!(spec.tls.alpn, ["h2"]);
        let (spec, failed, diags) = tt(
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
        assert_eq!(got.spec.max_streams, 5);
        assert_eq!(got.spec.headers.len(), 1);
```

换成

```rust
        assert_eq!(spec.max_streams, 5);
        assert_eq!(spec.headers.len(), 1);
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
        let (got, _, diags) = tt("trust-tunnel, h.test, 443, username=u, password=p, alpn=h3");
        assert_eq!(got.spec.tls.alpn, ["h2"]);
```

换成

```rust
        let (spec, _, diags) = tt("trust-tunnel, h.test, 443, username=u, password=p, alpn=h3");
        assert_eq!(spec.tls.alpn, ["h2"]);
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
                "policy `P`: `alpn` is always `h2` on `trust-tunnel` policies; ignored"
```

换成

```rust
                "policy `P`: `alpn` is always `h2` on `trust-tunnel` policies without `h3=true`; ignored"
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
        for (headers, diags) in [(&spec.headers, &diags), (&read.spec.headers, &tt_diags)] {
```

换成

```rust
        for (headers, diags) in [(&spec.headers, &diags), (&read.headers, &tt_diags)] {
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
        let (got, _, _) = tt("trust-tunnel, h.test, 443, username=s3cretUser, password=hunter2");
        for printed in [format!("{spec:?}"), format!("{:?}", got.spec)] {
```

换成

```rust
        let (got, _, _) = tt("trust-tunnel, h.test, 443, username=s3cretUser, password=hunter2");
        for printed in [format!("{spec:?}"), format!("{got:?}")] {
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
            "trust-tunnel, h.test, 443, username=u, password=p, headers=Authorization:Bearer t0ken",
        );
        for printed in [format!("{spec:?}"), format!("{:?}", got.spec)] {
```

换成

```rust
            "trust-tunnel, h.test, 443, username=u, password=p, headers=Authorization:Bearer t0ken",
        );
        for printed in [format!("{spec:?}"), format!("{got:?}")] {
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
            assert!(!printed.contains("t0ken"), "{printed}");
        }
    }
}

```

换成

```rust
            assert!(!printed.contains("t0ken"), "{printed}");
        }
    }

    /// Over HTTP/3 the ALPN is `h3` whatever the line says (TrustTunnel
    /// protocol 3.2): `alpn=h3` is welcome, anything else is said and
    /// ignored.
    #[test]
    fn with_h3_alpn_is_always_h3() {
        for def in [
            "trust-tunnel, h.test, 443, username=u, password=p, h3=true",
            "trust-tunnel, h.test, 443, username=u, password=p, h3=true, alpn=h3",
        ] {
            let (spec, failed, diags) = tt(def);
            assert!(!failed && diags.is_empty(), "{def}: {diags:?}");
            assert!(spec.h3, "{def}");
            assert_eq!(spec.tls.alpn, ["h3"], "{def}");
        }
        let (spec, failed, diags) =
            tt("trust-tunnel, h.test, 443, username=u, password=p, h3=true, alpn=h2");
        assert!(!failed);
        assert_eq!(spec.tls.alpn, ["h3"]);
        assert_eq!(
            messages(&diags),
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `P`: `alpn` is always `h3` on `trust-tunnel` policies with `h3=true`; ignored"
            )]
        );
    }

    /// HTTP/3 runs over QUIC: TCP Fast Open has nothing to act on there.
    /// Over HTTP/2 the reader leaves `tfo` alone (the loader says it has no
    /// effect yet).
    #[test]
    fn with_h3_tfo_does_not_apply() {
        let ((_, common), failed, diags) =
            tt_and_common("trust-tunnel, h.test, 443, username=u, password=p, h3=true, tfo=true");
        assert!(!failed);
        assert!(!common.tfo);
        assert_eq!(
            messages(&diags),
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `P`: `tfo` does not apply to `trust-tunnel` policies with `h3=true`; ignored"
            )]
        );
        let ((_, common), failed, diags) =
            tt_and_common("trust-tunnel, h.test, 443, username=u, password=p, tfo=true");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert!(common.tfo);
    }

    /// The manual gives `trust-tunnel` no port hopping, over HTTP/3 either.
    #[test]
    fn port_hopping_does_not_apply() {
        for h3 in ["false", "true"] {
            let (_, failed, diags) = tt(&format!(
                "trust-tunnel, h.test, 443, username=u, password=p, h3={h3}, port-hopping=5000-6000, port-hopping-interval=10"
            ));
            assert!(!failed, "h3={h3}");
            assert_eq!(
                messages(&diags),
                [
                    (
                        codes::W_PARAM_NOT_APPLICABLE,
                        "policy `P`: `port-hopping` does not apply to `trust-tunnel` policies; ignored"
                    ),
                    (
                        codes::W_PARAM_NOT_APPLICABLE,
                        "policy `P`: `port-hopping-interval` does not apply to `trust-tunnel` policies; ignored"
                    ),
                ],
                "h3={h3}"
            );
        }
    }

    /// HTTP/3 forbids what HTTP/2 does (RFC 9114 4.2, 4.3.1); the warning
    /// names the version the tunnel runs over.
    #[test]
    fn with_h3_the_headers_http3_forbids_are_dropped() {
        let (spec, failed, diags) = tt(
            "trust-tunnel, h.test, 443, username=u, password=p, h3=true, headers=Connection:close;Host:cdn.test;X-A:1",
        );
        assert!(!failed);
        let names: Vec<&str> = spec.headers.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, ["X-A"]);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: header `connection` in `headers` is not allowed in HTTP/3; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: header `host` in `headers` is not allowed in HTTP/3 (`:authority` names the target); ignored"
                ),
            ]
        );
    }
}

```

`crates/rurge-config/src/redact.rs`——把

```rust
            "T = tuic-v5, h.test, 443, ***, uuid=***, password=***, alpn=h3"
        );
    }
}

```

换成

```rust
            "T = tuic-v5, h.test, 443, ***, uuid=***, password=***, alpn=h3"
        );
    }

    /// MASQUE (phase 2 M7 design 7.1): the named credentials and a stray
    /// positional value; the ALPN and the hopping ports stay.
    #[test]
    fn a_masque_line_loses_its_credentials() {
        assert_eq!(
            redact_definition(
                "masque, example.com, 443, username=user, password=s3cret, alpn=h3, port-hopping=1234;5000-6000"
            ),
            "masque, example.com, 443, username=***, password=***, alpn=h3, port-hopping=1234;5000-6000"
        );
        assert_eq!(
            redact_profile("M = masque, example.com, 443, hunter2, username=user, password=s3cret"),
            "M = masque, example.com, 443, ***, username=***, password=***"
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

/// `masque` lines are read and checked at load (phase 2 M7 design 3.1,
/// 7.1): the combinations they cannot have and one credential without the
/// other are errors; a hop interval under 5 seconds is raised to 5, and
/// `ecn` has an effect on them. They have no spec until the engine builds
/// them (M7c task 8).
#[test]
fn masque_lines_are_checked_at_load() {
    let loaded = load(
        "A = masque, a.example, 443, username=u, password=p, port-hopping=5000-6000, underlying-proxy=E
B = masque, b.example, 443, shadow-tls-password=st
C = masque, c.example, 443, username=u
D = masque, d.example, 443, ecn=off, port-hopping=5000-6000, port-hopping-interval=2
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
                "policy `B`: Shadow TLS cannot be combined with a `masque` policy".to_string(),
                3
            ),
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `C`: `password` is required with `username`".to_string(),
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

/// `trust-tunnel` lines with `h3=true` are read and checked at load (phase
/// 2 M7 design 7.2): another ALPN, `tfo` and port hopping do not apply,
/// Shadow TLS is an error, which leaves no spec, and `ecn` has an effect on
/// them. Until the engine builds them over HTTP/3 (M7c task 8), `h3` is
/// said once to have no effect yet.
#[test]
fn trust_tunnel_lines_with_h3_are_checked_at_load() {
    let loaded = load(
        "A = trust-tunnel, a.example, 443, username=u, password=p, h3=true, alpn=h2, ecn=off
B = trust-tunnel, b.example, 443, username=u, password=p, h3=true, tfo=true, port-hopping=5000-6000
C = trust-tunnel, c.example, 443, username=u, password=p, h3=true, shadow-tls-password=st",
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
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `A`: `alpn` is always `h3` on `trust-tunnel` policies with `h3=true`; ignored"
                    .to_string(),
                2
            ),
            (
                codes::W_PARAM_NOT_EFFECTIVE,
                "policy parameter `h3` is parsed but has no effect in this version".to_string(),
                2
            ),
            (
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `B`: `tfo` does not apply to `trust-tunnel` policies with `h3=true`; ignored"
                    .to_string(),
                3
            ),
            (
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `B`: `port-hopping` does not apply to `trust-tunnel` policies; ignored"
                    .to_string(),
                3
            ),
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `C`: Shadow TLS cannot be combined with `h3=true` on a `trust-tunnel` policy"
                    .to_string(),
                4
            ),
        ]
    );
    for (name, ecn) in [("A", Tristate::Off), ("B", Tristate::Auto)] {
        let spec = loaded.config.spec(name).expect("warnings leave the spec");
        let ProtoSpec::TrustTunnel(tt) = &spec.proto else {
            panic!("{:?}", spec.proto)
        };
        assert!(tt.h3, "{name}");
        assert_eq!(tt.tls.alpn, ["h3"], "{name}");
        assert_eq!((spec.common.ecn, spec.common.tfo), (ecn, false), "{name}");
    }
    assert!(loaded.config.spec("C").is_none());
}

/// A group's `underlying-proxy` chains the proxies it takes in (`M (via
```

`rurge-proto` 里 `trust-tunnel` 用例的调用点（`read_trust_tunnel` 多了 `common` 参数、直接返回 spec）：

`crates/rurge-proto/src/trust_tunnel.rs`——把

```rust
    use rurge_config::spec::ParamReader;
    use rurge_config::spec::h2::read_trust_tunnel;
    use rurge_config::spec::shadow_tls::read_shadow_tls;
```

换成

```rust
    use rurge_config::spec::h2::read_trust_tunnel;
    use rurge_config::spec::shadow_tls::read_shadow_tls;
    use rurge_config::spec::{CommonOpts, ParamReader};
```

`crates/rurge-proto/src/trust_tunnel.rs`——把

```rust
        let spec = read_trust_tunnel(&mut r, &[]).spec;
```

换成

```rust
        let spec = read_trust_tunnel(&mut r, &mut CommonOpts::default(), &[]);
```

引擎的用例（过渡期：`h3=true` 的行经工厂照旧走 HTTP/2，假服务端看到的 ALPN 是 `h2`）：

`crates/rurge-engine/src/outbounds.rs`——把

```rust
    use rurge_proto::testing::{FakeSocks5, Socks5Script, echo_server};
    use rurge_proto_ssh::testing::ED25519_WITH_PASSPHRASE;
    use std::path::Path;
```

换成

```rust
    use rurge_proto::testing::{
        FakeH2Proxy, FakeSocks5, H2ProxyScript, Socks5Script, TlsFixture, echo_server,
    };
    use rurge_proto_ssh::testing::ED25519_WITH_PASSPHRASE;
    use std::path::Path;
    use std::time::Duration;
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            ["echo.test via eth9", "echo.test"]
        );
    }
}

```

换成

```rust
            ["echo.test via eth9", "echo.test"]
        );
    }

    /// Until the engine builds `h3=true` over HTTP/3 (M7c task 8), such a
    /// `trust-tunnel` line connects over HTTP/2 as before: the `h3` its spec
    /// pins as the ALPN never reaches the TLS handshake over TCP.
    #[tokio::test]
    async fn a_trust_tunnel_line_with_h3_still_connects_over_http2() {
        let echo = echo_server().await;
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let fake = FakeH2Proxy::spawn(
            H2ProxyScript {
                users: vec![("u".into(), "p".into())],
                ..H2ProxyScript::default()
            },
            fixture.clone(),
        )
        .await;
        let cfg = config(&format!(
            "[Proxy]\nTT = trust-tunnel, 127.0.0.1, {}, username=u, password=p, h3=true\n[Rule]\nFINAL,DIRECT\n",
            fake.addr().port()
        ));
        let f = EngineFactory::with_roots(
            &cfg,
            Arc::new(SystemResolve),
            Arc::new(NoopSocketHook),
            fixture.roots(),
        );
        let spec = cfg.spec("TT").unwrap();
        let out = f.build(spec, f.direct_connector(&spec.common)).unwrap();
        let target = Target::new(rurge_config::HostName::Ip(echo.ip()), echo.port());
        let mut stream = tokio::time::timeout(
            Duration::from_secs(5),
            out.connect_tcp(&target, &ConnectOpts::default()),
        )
        .await
        .expect("the tunnel opens in time")
        .unwrap();
        stream.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut buf))
            .await
            .expect("the echo arrives")
            .unwrap();
        assert_eq!(&buf, b"ping");
        assert_eq!(
            fixture.seen_at_least(1).await[0].alpn.as_deref(),
            Some("h2")
        );
    }
}

```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-config --lib`
Expected: FAIL——`MasqueSpec`、`MasqueCredentials`、`ProtoSpec::Masque`、`TrustTunnelSpec.h3` 与 `read_trust_tunnel` 新增的 `&mut CommonOpts` 参数由 Step 3 引入，编译不过：

```text
error[E0412]: cannot find type `CommonOpts` in this scope
   --> crates\rurge-config\src\spec\h2.rs:197:55
error[E0422]: cannot find struct, variant or union type `MasqueSpec` in this scope
    --> crates\rurge-config\src\spec\mod.rs:1623:40
error[E0422]: cannot find struct, variant or union type `MasqueCredentials` in this scope
    --> crates\rurge-config\src\spec\mod.rs:1635:31
error[E0061]: this function takes 2 arguments but 3 arguments were supplied
   --> crates\rurge-config\src\spec\h2.rs:200:24
   --> crates\rurge-config\src\spec\h2.rs:139:8
error[E0609]: no field `h3` on type `spec::h2::TrustTunnelSpec`
   --> crates\rurge-config\src\spec\h2.rs:317:23
error[E0609]: no field `h3` on type `spec::h2::TrustTunnelSpec`
   --> crates\rurge-config\src\spec\h2.rs:390:21
error[E0609]: no field `h3` on type `spec::h2::TrustTunnelSpec`
   --> crates\rurge-config\src\spec\h2.rs:394:22
error[E0609]: no field `h3` on type `spec::h2::TrustTunnelSpec`
   --> crates\rurge-config\src\spec\h2.rs:540:26
error[E0560]: struct `spec::h2::TrustTunnelSpec` has no field named `h3`
    --> crates\rurge-config\src\spec\mod.rs:1176:13
error[E0599]: no variant or associated item named `Masque` found for enum `spec::ProtoSpec` in the current scope
    --> crates\rurge-config\src\spec\mod.rs:1623:33
error[E0609]: no field `h3` on type `&spec::h2::TrustTunnelSpec`
    --> crates\rurge-config\src\spec\mod.rs:1690:20
error[E0609]: no field `h3` on type `&spec::h2::TrustTunnelSpec`
    --> crates\rurge-config\src\spec\mod.rs:1707:21
error: could not compile `rurge-config` (lib test) due to 12 previous errors
```

（`cargo test -p rurge-config --test policy_spec` 同样编译不过；`cargo test -p rurge-engine --lib a_trust_tunnel_line_with_h3` 在此时能通过——它钉住"行为不变"，防的是 Step 3 改了 spec 而引擎没跟上。）

- [ ] **Step 3: 实现**

新建 `crates/rurge-config/src/spec/masque.rs`：

```rust
//! `masque` policy parameters (manual: Policies › MASQUE; phase 2 M7 design
//! 7.1): HTTP/3 over QUIC, every TCP connection a CONNECT stream and UDP
//! relayed with CONNECT-UDP (RFC 9298) whatever the line says.

use super::common::CommonOpts;
use super::quic::{QuicOpts, read_quic, read_quic_tls};
use super::reader::ParamReader;
use super::secret::Secret;
use super::tls::TlsOpts;
use crate::diagnostic::codes;
use crate::keystore::KeystoreItem;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MasqueSpec {
    /// MASQUE always runs TLS over QUIC; `alpn` is `h3` unless written.
    pub tls: TlsOpts,
    pub quic: QuicOpts,
    /// `username` and `password`, sent as Basic authorization with every
    /// CONNECT request; without them no authorization is sent (manual).
    pub credentials: Option<MasqueCredentials>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MasqueCredentials {
    pub username: Secret<String>,
    pub password: Secret<String>,
}

/// Everything `masque`-specific on the line, and `tfo` taken out of
/// `common` (`read_quic`). After an error was reported the returned value
/// is meaningless: the caller checks `r.has_errors()`.
///
/// The credentials are optional but go together. They are named-only, as
/// the manual writes them, and never quoted in a diagnostic: a positional
/// value stays unread and is reported as an extra positional value. The
/// manual gives MASQUE no `headers` and no `max-streams`: here they are
/// unknown parameters, as anything else not read.
pub fn read_masque(
    r: &mut ParamReader<'_>,
    common: &mut CommonOpts,
    keystore: &[KeystoreItem],
) -> MasqueSpec {
    let tls = read_quic_tls(r, keystore);
    let quic = read_quic(r, common);
    let credentials = read_credentials(r);
    if r.has("udp-relay") {
        r.touch("udp-relay");
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            "`udp-relay` does not apply to `masque` policies (UDP is always relayed); ignored"
                .to_string(),
        );
    }
    MasqueSpec {
        tls,
        quic,
        credentials,
    }
}

/// `username` and `password`: both or neither, and neither empty.
fn read_credentials(r: &mut ParamReader<'_>) -> Option<MasqueCredentials> {
    let username = r.str("username");
    let password = r.str("password");
    for (key, value) in [("username", username), ("password", password)] {
        if value.is_some_and(str::is_empty) {
            r.error(codes::E_INVALID_POLICY_PARAM, format!("`{key}` is empty"));
        }
    }
    let (missing, written) = match (username, password) {
        (Some(username), Some(password)) => {
            return Some(MasqueCredentials {
                username: username.into(),
                password: password.into(),
            });
        }
        (None, None) => return None,
        (Some(_), None) => ("password", "username"),
        (None, Some(_)) => ("username", "password"),
    };
    r.error(
        codes::E_INVALID_POLICY_PARAM,
        format!("`{missing}` is required with `{written}`"),
    );
    None
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

    fn read(def: &str) -> (MasqueSpec, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let mut common = read_common(&mut r, Applies::Proxy, &mut Notes::default());
        let spec = read_masque(&mut r, &mut common, &[]);
        let failed = r.has_errors();
        (spec, failed, r.finish())
    }

    fn messages(diags: &[Diagnostic]) -> Vec<(&str, &str)> {
        diags.iter().map(|d| (d.code, d.message.as_str())).collect()
    }

    fn credentials(username: &str, password: &str) -> Option<MasqueCredentials> {
        Some(MasqueCredentials {
            username: username.into(),
            password: password.into(),
        })
    }

    #[test]
    fn the_manuals_example() {
        let (spec, failed, diags) = read("masque, example.com, 443, username=user, password=pass");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.credentials, credentials("user", "pass"));
        assert_eq!(spec.quic, QuicOpts::default());
        // `h3` unless written (manual)
        assert_eq!(
            spec.tls,
            TlsOpts {
                alpn: vec!["h3".into()],
                ..TlsOpts::default()
            }
        );
    }

    #[test]
    fn every_parameter() {
        let (spec, failed, diags) = read(
            "masque, h.test, 443, username=u, password=pw, port-hopping=443;20000-30000, port-hopping-interval=60, alpn=h3-29, sni=edge.test, skip-cert-verify=true",
        );
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.credentials, credentials("u", "pw"));
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
        let (spec, failed, diags) = read("masque, h.test, 443, port-hopping=443");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(
            spec.quic.port_hopping.map(|hopping| hopping.interval),
            Some(DEFAULT_HOP_INTERVAL)
        );
    }

    /// Without credentials no authorization is sent (manual): nothing to
    /// say about the line.
    #[test]
    fn the_credentials_are_optional() {
        let (spec, failed, diags) = read("masque, example.com, 443");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.credentials, None);
    }

    /// One without the other, or one written empty, is an error that names
    /// the key and never quotes a value.
    #[test]
    fn the_credentials_go_together_and_are_never_quoted() {
        for (def, expected) in [
            (
                "masque, h.test, 443, username=s3cretUser",
                &["`password` is required with `username`"][..],
            ),
            (
                "masque, h.test, 443, password=hunter2",
                &["`username` is required with `password`"],
            ),
            (
                "masque, h.test, 443, username=s3cretUser, password=",
                &["`password` is empty"],
            ),
            (
                "masque, h.test, 443, username=, password=hunter2",
                &["`username` is empty"],
            ),
            (
                "masque, h.test, 443, username=, password=",
                &["`username` is empty", "`password` is empty"],
            ),
            (
                "masque, h.test, 443, username=",
                &[
                    "`username` is empty",
                    "`password` is required with `username`",
                ],
            ),
        ] {
            let (_, failed, diags) = read(def);
            assert!(failed, "{def}");
            let expected: Vec<(&str, String)> = expected
                .iter()
                .map(|text| (codes::E_INVALID_POLICY_PARAM, format!("policy `P`: {text}")))
                .collect();
            let found: Vec<(&str, String)> =
                diags.iter().map(|d| (d.code, d.message.clone())).collect();
            assert_eq!(found, expected, "{def}");
        }
    }

    /// Only the named form is read, as the manual writes it: positional
    /// values are no credentials, and are said to be ignored, unquoted.
    #[test]
    fn positional_values_are_not_the_credentials() {
        let (spec, failed, diags) = read("masque, h.test, 443, s3cretUser, hunter2");
        assert!(!failed);
        assert_eq!(spec.credentials, None);
        assert_eq!(
            messages(&diags),
            [
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

    /// UDP is relayed whatever the line says (manual), and QUIC opens no TCP
    /// connection to open fast.
    #[test]
    fn udp_relay_and_tfo_do_not_apply() {
        let (_, failed, diags) = read("masque, h.test, 443, udp-relay=true, tfo=true");
        assert!(!failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `tfo` does not apply to `masque` policies; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `udp-relay` does not apply to `masque` policies (UDP is always relayed); ignored"
                ),
            ]
        );
    }

    /// The manual lists neither for MASQUE: no parameters of it.
    #[test]
    fn headers_and_max_streams_are_unknown_parameters() {
        let (spec, failed, diags) =
            read("masque, h.test, 443, username=u, password=pw, headers=X-A:1, max-streams=5");
        assert!(!failed);
        assert_eq!(spec.credentials, credentials("u", "pw"));
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `P`: unknown parameter `headers` ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `P`: unknown parameter `max-streams` ignored"
                ),
            ]
        );
    }

    #[test]
    fn the_spec_does_not_print_its_credentials() {
        let (spec, _, _) = read("masque, h.test, 443, username=s3cretUser, password=hunter2");
        let printed = format!("{spec:?}");
        assert!(printed.contains("Secret(***)"), "{printed}");
        assert!(
            !printed.contains("s3cretUser") && !printed.contains("hunter2"),
            "{printed}"
        );
    }
}
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub mod hysteria2;
```

换成

```rust
pub mod hysteria2;
pub mod masque;
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub use hysteria2::{Hysteria2Obfs, Hysteria2Spec};
```

换成

```rust
pub use hysteria2::{Hysteria2Obfs, Hysteria2Spec};
pub use masque::{MasqueCredentials, MasqueSpec};
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    Tuic(TuicSpec),
```

换成

```rust
    Tuic(TuicSpec),
    Masque(MasqueSpec),
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            ProtoSpec::Tuic(tuic) => Some(&tuic.tls),
```

换成

```rust
            ProtoSpec::Tuic(tuic) => Some(&tuic.tls),
            ProtoSpec::Masque(masque) => Some(&masque.tls),
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    /// The QUIC options of the protocol, when it runs over QUIC.
```

换成

```rust
    /// The QUIC options of the protocol, when it runs over QUIC and takes
    /// them: a `trust-tunnel` with `h3=true` takes none (no port hopping).
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            ProtoSpec::Tuic(tuic) => Some(&tuic.quic),
```

换成

```rust
            ProtoSpec::Tuic(tuic) => Some(&tuic.quic),
            ProtoSpec::Masque(masque) => Some(&masque.quic),
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            let read = h2::read_trust_tunnel(&mut r, env.keystore);
            // connects over HTTP/2 until the QUIC family (phase 2 M6 design 5.1)
            if read.h3 {
                notes.inert.push("h3");
            }
            (common, ProtoSpec::TrustTunnel(read.spec))
```

换成

```rust
            let mut common = read_common(&mut r, Applies::Proxy, &mut notes);
            let tt = h2::read_trust_tunnel(&mut r, &mut common, env.keystore);
            if tt.h3 {
                // over QUIC, as on `hysteria2`: `ecn` takes effect, `tfo`
                // does not apply (phase 2 M7 design 7.2)
                notes.inert.retain(|name| !matches!(*name, "ecn" | "tfo"));
                // the engine connects over HTTP/2 until M7c task 8
                notes.inert.push("h3");
            }
            (common, ProtoSpec::TrustTunnel(tt))
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            (common, ProtoSpec::Tuic(tuic))
```

换成

```rust
            (common, ProtoSpec::Tuic(tuic))
        }
        PolicyKind::Masque => {
            let mut common = read_common(&mut r, Applies::Proxy, &mut notes);
            let masque = masque::read_masque(&mut r, &mut common, env.keystore);
            // as on `hysteria2`: `ecn` takes effect, `tfo` does not apply
            notes.inert.retain(|name| !matches!(*name, "ecn" | "tfo"));
            (common, ProtoSpec::Masque(masque))
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        shadow_tls = shadow_tls::read_shadow_tls(&mut r);
```

换成

```rust
        shadow_tls = match &proto {
            // Shadow TLS wraps a TCP connection; HTTP/3 runs over QUIC
            // (phase 2 M7 design 3.1)
            ProtoSpec::TrustTunnel(tt) if tt.h3 => {
                shadow_tls::refuse(
                    &mut r,
                    "Shadow TLS cannot be combined with `h3=true` on a `trust-tunnel` policy"
                        .to_string(),
                );
                None
            }
            _ => shadow_tls::read_shadow_tls(&mut r),
        };
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    let spec = (!failed && not_implemented.is_none()).then(|| PolicySpec {
```

换成

```rust
    // the engine builds `masque` from M7c task 8 on: until then a valid line
    // is checked in full but has no spec
    let built = policy.kind != PolicyKind::Masque;
    let spec = (!failed && not_implemented.is_none() && built).then(|| PolicySpec {
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
//! carry each connection as a CONNECT stream over a TLS + HTTP/2 connection.

use super::http::HeaderTemplate;
```

换成

```rust
//! carry each connection as a CONNECT stream over a TLS + HTTP/2
//! connection; `trust-tunnel` with `h3=true` over HTTP/3 (QUIC) instead
//! (phase 2 M7 design 7.2).

use super::common::CommonOpts;
use super::http::HeaderTemplate;
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
    /// Trust Tunnel always runs over TLS; `alpn` is always `h2`.
```

换成

```rust
    /// Trust Tunnel always runs over TLS; `alpn` is always the transport's:
    /// `h2`, or `h3` with `h3=true`.
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
    /// `max-streams`: CONNECT streams one HTTP/2 connection carries at once.
    pub max_streams: u32,
}

/// What a `trust-tunnel` line says: the spec, and whether it asks for
/// `h3=true`, which is parsed and reported (`W0029`) but connects over
/// HTTP/2 until the QUIC family lands (phase 2 M6 design 5.1).
pub struct TrustTunnelRead {
    pub spec: TrustTunnelSpec,
```

换成

```rust
    /// `max-streams`: CONNECT streams one connection carries at once.
    pub max_streams: u32,
    /// `h3=true`: the tunnel runs over HTTP/3 (QUIC) instead of HTTP/2 over
    /// TLS (manual).
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
/// Connection-specific header fields HTTP/2 forbids (RFC 9113 8.2.2); the
/// `h2` crate refuses a request that carries any of them.
```

换成

```rust
/// Connection-specific header fields HTTP/2 and HTTP/3 forbid (RFC 9113
/// 8.2.2, RFC 9114 4.2); the `h2` crate refuses a request that carries any
/// of them.
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
/// The TLS options with ALPN pinned to `h2`: an `alpn` that says anything
/// else is `W0028`.
fn read_h2_tls(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> TlsOpts {
    let mut tls = read_tls(r, keystore);
    if !tls.alpn.is_empty() && tls.alpn != ["h2"] {
```

换成

```rust
/// The TLS options with ALPN pinned to `alpn`, the one protocol the
/// transport speaks: an `alpn` that says anything else is `W0028`, which
/// names the policies — those of the line's type, narrowed by `which` when
/// the type has more than one transport.
fn read_pinned_tls(
    r: &mut ParamReader<'_>,
    keystore: &[KeystoreItem],
    alpn: &str,
    which: &str,
) -> TlsOpts {
    let mut tls = read_tls(r, keystore);
    if !tls.alpn.is_empty() && tls.alpn != [alpn] {
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
            format!("`alpn` is always `h2` on `{kind}` policies; ignored"),
        );
    }
    tls.alpn = vec!["h2".to_string()];
```

换成

```rust
            format!("`alpn` is always `{alpn}` on `{kind}` policies{which}; ignored"),
        );
    }
    tls.alpn = vec![alpn.to_string()];
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
/// `headers`, without the fields HTTP/2 forbids (`W0028` each). `host` is
/// one of them here: `:authority` names the CONNECT target, and a `Host`
/// that differs from it makes the request malformed (RFC 9113 8.3.1).
fn read_h2_headers(r: &mut ParamReader<'_>) -> Vec<HeaderTemplate> {
```

换成

```rust
/// `headers`, without the fields `http` (HTTP/2 or HTTP/3) forbids
/// (`W0028` each). `host` is one of them here: `:authority` names the
/// CONNECT target, and a `Host` that differs from it makes the request
/// malformed (RFC 9113 8.3.1, RFC 9114 4.3.1).
fn read_connect_headers(r: &mut ParamReader<'_>, http: &str) -> Vec<HeaderTemplate> {
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
            format!("header `{name}` in `headers` is not allowed in HTTP/2{why}; ignored"),
```

换成

```rust
            format!("header `{name}` in `headers` is not allowed in {http}{why}; ignored"),
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
    let tls = read_h2_tls(r, keystore);
    let (username, password) = read_credentials(r);
    let headers = read_h2_headers(r);
```

换成

```rust
    let tls = read_pinned_tls(r, keystore, "h2", "");
    let (username, password) = read_credentials(r);
    let headers = read_connect_headers(r, "HTTP/2");
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
/// Everything `trust-tunnel`-specific on the line. After an error was
/// reported the returned value is meaningless: the caller checks
/// `r.has_errors()`.
```

换成

```rust
/// Everything `trust-tunnel`-specific on the line, and with `h3=true` `tfo`
/// taken out of `common`. After an error was reported the returned value is
/// meaningless: the caller checks `r.has_errors()`.
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
/// reported as an extra positional value.
pub fn read_trust_tunnel(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> TrustTunnelRead {
    let tls = read_h2_tls(r, keystore);
    let username = required(r, "username");
    let password = required(r, "password");
    let headers = read_h2_headers(r);
    let max_streams = read_max_streams(r);
    let h3 = r.bool("h3").unwrap_or(false);
```

换成

```rust
/// reported as an extra positional value. `h3` picks the transport, and
/// with it the ALPN (TrustTunnel protocol 3.1, 3.2: `h2` over HTTP/2, `h3`
/// over HTTP/3). The manual gives `trust-tunnel` no port hopping, over
/// HTTP/3 either.
pub fn read_trust_tunnel(
    r: &mut ParamReader<'_>,
    common: &mut CommonOpts,
    keystore: &[KeystoreItem],
) -> TrustTunnelSpec {
    let h3 = r.bool("h3").unwrap_or(false);
    let (alpn, which, http) = if h3 {
        ("h3", " with `h3=true`", "HTTP/3")
    } else {
        ("h2", " without `h3=true`", "HTTP/2")
    };
    let tls = read_pinned_tls(r, keystore, alpn, which);
    if h3 {
        // HTTP/3 runs over QUIC: there is no TCP connection to open fast
        if r.has("tfo") {
            r.warn(
                codes::W_PARAM_NOT_APPLICABLE,
                "`tfo` does not apply to `trust-tunnel` policies with `h3=true`; ignored"
                    .to_string(),
            );
        }
        common.tfo = false;
    }
    let username = required(r, "username");
    let password = required(r, "password");
    let headers = read_connect_headers(r, http);
    let max_streams = read_max_streams(r);
    for key in ["port-hopping", "port-hopping-interval"] {
        if r.has(key) {
            r.touch(key);
            r.warn(
                codes::W_PARAM_NOT_APPLICABLE,
                format!("`{key}` does not apply to `trust-tunnel` policies; ignored"),
            );
        }
    }
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
    TrustTunnelRead {
        spec: TrustTunnelSpec {
            tls,
            username,
            password,
            headers,
            max_streams,
        },
```

换成

```rust
    TrustTunnelSpec {
        tls,
        username,
        password,
        headers,
        max_streams,
```

`crates/rurge-config/src/spec/shadow_tls.rs`——把

```rust
/// goes to a program on this machine.
```

换成

```rust
/// goes to a program on this machine. A `trust-tunnel` policy has it over
/// HTTP/2 only: with `h3=true` it runs over QUIC, and `to_spec` refuses
/// Shadow TLS there (`refuse`).
```

`crates/rurge-config/src/spec/shadow_tls.rs`——把

```rust
        for key in KEYS {
            r.touch(key);
        }
        r.error(
            codes::E_INVALID_POLICY_PARAM,
```

换成

```rust
        refuse(
            r,
```

`crates/rurge-config/src/spec/shadow_tls.rs`——把

```rust
    })
```

换成

```rust
    })
}

/// For a line that cannot have Shadow TLS: when it asks for it anyway, one
/// `E0018` saying `why`, and its parameters count as read — the error is
/// all there is to say about them.
pub(crate) fn refuse(r: &mut ParamReader<'_>, why: String) {
    if !KEYS.iter().any(|key| r.has(key)) {
        return;
    }
    for key in KEYS {
        r.touch(key);
    }
    r.error(codes::E_INVALID_POLICY_PARAM, why);
```

过渡期的引擎（Task 8 去掉）：`masque` 的占位分支；`trust-tunnel` 的 spec 现在把 `h3=true` 行的 `alpn` 钉成 `h3`，在 Task 8 之前仍按 HTTP/2 建出站，所以先把 ALPN 换回 `h2`：

`crates/rurge-engine/src/outbounds.rs`——把

```rust
use rurge_config::spec::{CommonOpts, IpVersion, PolicySpec, ProtoSpec};
```

换成

```rust
use rurge_config::spec::{CommonOpts, IpVersion, PolicySpec, ProtoSpec, TlsOpts, TrustTunnelSpec};
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            ProtoSpec::TrustTunnel(tt) => Arc::new(TrustTunnelOutbound::new(
                &spec.name,
                server_of(spec)?,
                tt,
                spec.shadow_tls.as_ref(),
                &self.keystore,
                self.roots.clone(),
                connector,
            )?),
```

换成

```rust
            ProtoSpec::TrustTunnel(tt) => {
                // over HTTP/2 until M7c task 8 builds `h3=true` over HTTP/3:
                // such a line's spec pins `alpn` to `h3`, which an HTTP/2
                // connection must not offer
                let h2 = TrustTunnelSpec {
                    tls: TlsOpts {
                        alpn: vec!["h2".to_string()],
                        ..tt.tls.clone()
                    },
                    ..tt.clone()
                };
                Arc::new(TrustTunnelOutbound::new(
                    &spec.name,
                    server_of(spec)?,
                    &h2,
                    spec.shadow_tls.as_ref(),
                    &self.keystore,
                    self.roots.clone(),
                    connector,
                )?)
            }
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            )?),
            // nothing starts here: the program starts on the first dial
```

换成

```rust
            )?),
            // the loader makes no spec of these lines before M7c task 8
            ProtoSpec::Masque(_) => {
                return Err(BuildError::new(format!(
                    "`{}` is not implemented yet",
                    spec.kind.keyword()
                )));
            }
            // nothing starts here: the program starts on the first dial
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-config` → 通过（新增 20 条：`spec::masque` 8 条、`spec::h2` 4 条、`spec::tests` 5 条、脱敏 1 条、`policy_spec` 2 条）。
Run: `cargo test -p rurge-proto trust_tunnel` 与 `cargo test -p rurge-engine --lib a_trust_tunnel_line_with_h3_still_connects_over_http2` → 通过。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-config crates/rurge-proto/src/trust_tunnel.rs crates/rurge-engine/src/outbounds.rs
git commit -m "feat(config): masque 与 trust-tunnel 的 h3=true 的配置；h3 与 Shadow TLS 组合的 E0018"
```

### Task 2: HTTP/3：扩展 CONNECT、HTTP 数据报、更严的应答

自写的 HTTP/3 客户端补上 MASQUE、`trust-tunnel` 的 HTTP/3 与 DoH3 要用的部分，并清掉 M7a 留给 HTTP/3 层的延后事项（P4–P7）；测试服务端 `H3Server` / `ServerStream` 补上数据报与流号（它的编解码照旧独立实现）。Hysteria 2 的行为不变。

**Files:**
- Create: `crates/rurge-net/src/quic/h3/datagram.rs`（自带用例）
- Modify: `crates/rurge-net/src/quic/h3/{mod.rs, client.rs, frame.rs, stream.rs}`（与用例）、`h3/testing.rs`

**Interfaces:**
- Consumes: M7a 的 `h3::{Client, RequestStream, Settings}`、`frame` / `qpack`、`testing::H3Server`。
- Produces: `h3::Protocol`（请求扩展，`pub const CONNECT_UDP: Protocol`）；`Client::with_datagrams(connection: Connection) -> Client`；`RequestStream::id(&self) -> u64`、`async fn RequestStream::datagrams(&mut self) -> io::Result<Datagrams>`（等服务端 SETTINGS，每条流一次）；`h3::Datagrams`：`fn max_payload(&self) -> usize`、`fn send(&self, payload: &[u8]) -> io::Result<()>`、`async fn recv(&self) -> Option<Bytes>`；`pub(crate) const H3_DATAGRAM_ERROR`；测试侧 `ServerStream::id(&self) -> u64`、`ServerStream::datagrams(&mut self) -> ServerDatagrams`（`fn send(&self, payload: &[u8]) -> io::Result<()>`、`async fn recv(&mut self) -> Option<Vec<u8>>`）

- [ ] **Step 1: 先写用例（连同测试服务端的数据报）**

`crates/rurge-net/src/quic/h3/testing.rs`——把

```rust
//! read, requests taken apart and answered. Its frames and its QPACK are
//! code of its own — only the static table and the Huffman code are the
//! client's — so what the client sends is read by code that is not the
//! client's. It reads what this client sends and no more: field sections
//! without a dynamic table, and no Huffman-encoded string (the client
//! sends none). Never used by production code.
```

换成

```rust
//! read, requests taken apart and answered, HTTP/3 datagrams by stream.
//! Its frames, its QPACK and its datagrams are code of its own — only the
//! static table and the Huffman code are the client's — so what the client
//! sends is read by code that is not the client's. It reads what this
//! client sends and no more: field sections without a dynamic table, and
//! no Huffman-encoded string (the client sends none). Never used by
//! production code.
```

`crates/rurge-net/src/quic/h3/testing.rs`——把

```rust
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::watch;
```

换成

```rust
use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, watch};
```

`crates/rurge-net/src/quic/h3/testing.rs`——把

```rust
const CONTROL: u64 = 0x00;

/// An HTTP/3 server on an accepted connection. Dropping it stops reading
/// the client's streams; the connection stays the caller's.
```

换成

```rust
const CONTROL: u64 = 0x00;
const H3_DATAGRAM: u64 = 0x33;

/// The client's datagrams that wait for one of the fake's streams; more
/// are dropped.
const DATAGRAM_QUEUE: usize = 1024;

/// The queues of the streams that take the client's datagrams, by quarter
/// stream ID; `None` while the fake does not read them.
type Demux = Mutex<Option<HashMap<u64, mpsc::Sender<Vec<u8>>>>>;

/// An HTTP/3 server on an accepted connection. Dropping it stops reading
/// the client's streams and datagrams; the connection stays the caller's.
```

`crates/rurge-net/src/quic/h3/testing.rs`——把

```rust
    reader: JoinHandle<()>,
```

换成

```rust
    reader: JoinHandle<()>,
    demux: Arc<Demux>,
    /// Reads the client's datagrams, once our SETTINGS said
    /// `H3_DATAGRAM = 1`.
    datagrams: Option<JoinHandle<()>>,
```

`crates/rurge-net/src/quic/h3/testing.rs`——把

```rust
            reader,
```

换成

```rust
            reader,
            demux: Arc::default(),
            datagrams: None,
```

`crates/rurge-net/src/quic/h3/testing.rs`——把

```rust
    /// order.
```

换成

```rust
    /// order. With `H3_DATAGRAM = 1` among them, the fake reads the
    /// connection's QUIC datagrams from then on as HTTP/3 datagrams, each
    /// for the stream it names (`ServerStream::datagrams`); without, it
    /// leaves them to the caller (Hysteria 2's UDP).
```

`crates/rurge-net/src/quic/h3/testing.rs`——把

```rust
        self.control = Some(control);
```

换成

```rust
        self.control = Some(control);
        if settings.contains(&(H3_DATAGRAM, 1)) && self.datagrams.is_none() {
            *self.demux.lock().expect("demux") = Some(HashMap::new());
            self.datagrams = Some(tokio::spawn(read_datagrams(
                self.quic.clone(),
                self.demux.clone(),
            )));
        }
```

`crates/rurge-net/src/quic/h3/testing.rs`——把

```rust
                buf: Vec::new(),
            },
        })
```

换成

```rust
                buf: Vec::new(),
            },
            quic: self.quic.clone(),
            demux: self.demux.clone(),
            datagrams_taken: false,
        })
```

`crates/rurge-net/src/quic/h3/testing.rs`——把

```rust
        self.reader.abort();
```

换成

```rust
        self.reader.abort();
        if let Some(datagrams) = &self.datagrams {
            datagrams.abort();
        }
        // the streams' datagram queues end with it
        *self.demux.lock().expect("demux") = None;
```

`crates/rurge-net/src/quic/h3/testing.rs`——把

```rust
        streams.spawn(read_client_stream(recv, seen.clone()));
    }
}
```

换成

```rust
        streams.spawn(read_client_stream(recv, seen.clone()));
    }
}

/// Reads the client's datagrams into the queues of the streams they name
/// until the connection ends, and the queues with it: a datagram for no
/// stream that takes them, one that meets a full queue, and one too short
/// for a quarter stream ID are dropped.
async fn read_datagrams(quic: Connection, demux: Arc<Demux>) {
    while let Ok(datagram) = quic.read_datagram().await {
        let Some((quarter, n)) = parse_varint(&datagram) else {
            continue;
        };
        if let Some(queue) = demux
            .lock()
            .expect("demux")
            .as_ref()
            .and_then(|queues| queues.get(&quarter))
        {
            let _ = queue.try_send(datagram[n..].to_vec());
        }
    }
    *demux.lock().expect("demux") = None;
}
```

`crates/rurge-net/src/quic/h3/testing.rs`——把

```rust
    incoming: Incoming,
```

换成

```rust
    incoming: Incoming,
    /// The connection, for the stream's datagrams.
    quic: Connection,
    demux: Arc<Demux>,
    /// `datagrams` was called.
    datagrams_taken: bool,
```

`crates/rurge-net/src/quic/h3/testing.rs`——把

```rust
impl ServerStream {
```

换成

```rust
impl ServerStream {
    /// The stream's QUIC stream ID.
    pub fn id(&self) -> u64 {
        u64::from(self.send.id())
    }

    /// The stream's HTTP/3 datagrams (RFC 9297 2.1), once per stream: those
    /// the client sends for it from now on come in, while the fake reads
    /// datagrams (`H3Server::send_settings`); those sent go out with the
    /// stream's quarter stream ID either way.
    pub fn datagrams(&mut self) -> ServerDatagrams {
        assert!(
            !self.datagrams_taken,
            "the stream's datagrams were taken already"
        );
        self.datagrams_taken = true;
        let quarter = self.id() / 4;
        let (queue, incoming) = mpsc::channel(DATAGRAM_QUEUE);
        // while the fake does not read datagrams, the queue ends at once
        if let Some(queues) = self.demux.lock().expect("demux").as_mut() {
            queues.insert(quarter, queue);
        }
        ServerDatagrams {
            quic: self.quic.clone(),
            quarter,
            demux: self.demux.clone(),
            incoming,
        }
    }

```

`crates/rurge-net/src/quic/h3/testing.rs`——把

```rust
        AsyncWrite::poll_shutdown(Pin::new(&mut self.get_mut().send), cx)
```

换成

```rust
        AsyncWrite::poll_shutdown(Pin::new(&mut self.get_mut().send), cx)
    }
}

/// The HTTP/3 datagrams of one of the client's streams, as the fake sends
/// and takes them (`ServerStream::datagrams`).
pub struct ServerDatagrams {
    quic: Connection,
    quarter: u64,
    demux: Arc<Demux>,
    incoming: mpsc::Receiver<Vec<u8>>,
}

impl ServerDatagrams {
    /// A datagram of `payload` for the stream.
    pub fn send(&self, payload: &[u8]) -> io::Result<()> {
        let mut datagram = varint(self.quarter);
        datagram.extend_from_slice(payload);
        self.quic
            .send_datagram(datagram.into())
            .map_err(io::Error::other)
    }

    /// The payload of the client's next datagram for the stream; `None`
    /// while the fake does not read datagrams, and once the connection
    /// ended.
    pub async fn recv(&mut self) -> Option<Vec<u8>> {
        self.incoming.recv().await
    }
}

impl Drop for ServerDatagrams {
    fn drop(&mut self) {
        if let Some(queues) = self.demux.lock().expect("demux").as_mut() {
            queues.remove(&self.quarter);
        }
```

`crates/rurge-net/src/quic/h3/client.rs`——把

```rust
            critical: AtomicU8::new(0),
        };
```

换成

```rust
            critical: AtomicU8::new(0),
            flows: None,
        };
```

`crates/rurge-net/src/quic/h3/client.rs`——把

```rust
            assert_eq!(err.to_string(), "h3: a request needs an authority");
        }
    }
}

```

换成

```rust
            assert_eq!(err.to_string(), "h3: a request needs an authority");
        }
    }

    /// Only a CONNECT carries a protocol (RFC 8441 4).
    #[test]
    fn a_request_with_a_protocol_must_be_a_connect() {
        let mut request = Request::post("https://example.test/").body(()).unwrap();
        request.extensions_mut().insert(Protocol::CONNECT_UDP);
        let err = head(&request).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            "h3: a request with a protocol must be a CONNECT"
        );
    }
}

```

`crates/rurge-net/src/quic/h3/frame.rs`——把

```rust
        SETTINGS_ENABLE_CONNECT_PROTOCOL, SETTINGS_MAX_FIELD_SECTION_SIZE,
```

换成

```rust
        SETTINGS_ENABLE_CONNECT_PROTOCOL, SETTINGS_H3_DATAGRAM, SETTINGS_MAX_FIELD_SECTION_SIZE,
```

`crates/rurge-net/src/quic/h3/frame.rs`——把

```rust
                "{payload:02x?}"
            );
        }
    }
}

```

换成

```rust
                "{payload:02x?}"
            );
        }
    }

    /// Extended CONNECT and HTTP Datagrams are on (1) or off (0); any other
    /// value is H3_SETTINGS_ERROR (RFC 8441 3, RFC 9297 2.1.1).
    #[test]
    fn extension_settings_are_0_or_1() {
        for id in [SETTINGS_ENABLE_CONNECT_PROTOCOL, SETTINGS_H3_DATAGRAM] {
            for value in [0, 1] {
                let mut payload = Vec::new();
                put_varint(&mut payload, id);
                put_varint(&mut payload, value);
                let settings = Settings::decode(&payload).unwrap();
                assert_eq!(settings.get(id), Some(value), "{id:#x}");
            }
            for value in [2, 1 << 20] {
                let mut payload = Vec::new();
                put_varint(&mut payload, id);
                put_varint(&mut payload, value);
                let error = Settings::decode(&payload).unwrap_err();
                assert_eq!(error.code, H3_SETTINGS_ERROR, "{id:#x} = {value}");
                assert_eq!(
                    error.text,
                    format!("the server sent the setting {id:#x} as {value}, neither 0 nor 1")
                );
            }
        }
    }
}

```

`crates/rurge-net/src/quic/h3/stream.rs`——把

```rust
            assert!(head(fields).is_none(), "{fields:?}");
        }
    }
}

```

换成

```rust
            assert!(head(fields).is_none(), "{fields:?}");
        }
    }

    /// A `content-length` is one number of digits alone, however many
    /// times it is given (RFC 9110 8.6).
    #[test]
    fn a_content_length_is_one_number() {
        let length = |values: &[&str]| {
            let mut headers = HeaderMap::new();
            for value in values {
                headers.append(CONTENT_LENGTH, HeaderValue::from_str(value).unwrap());
            }
            content_length(&headers)
        };
        assert_eq!(length(&[]), Ok(None));
        assert_eq!(length(&["0"]), Ok(Some(0)));
        assert_eq!(length(&["1234"]), Ok(Some(1234)));
        assert_eq!(length(&["007"]), Ok(Some(7)));
        assert_eq!(length(&["7", "7"]), Ok(Some(7)));
        assert_eq!(length(&["18446744073709551615"]), Ok(Some(u64::MAX)));
        for values in [
            &["7", "8"][..],
            &[""],
            &["+7"],
            &["-7"],
            &["7 "],
            &["0x7"],
            &["7, 7"],
            &["18446744073709551616"],
        ] {
            assert_eq!(length(values), Err(()), "{values:?}");
        }
    }
}

```

`crates/rurge-net/src/quic/h3/mod.rs`——把

```rust
    use http::{HeaderValue, Request, Response};
```

换成

```rust
    use bytes::Bytes;
    use http::{HeaderValue, Method, Request, Response};
```

`crates/rurge-net/src/quic/h3/mod.rs`——把

```rust
    const SERVER_SETTINGS: &[(u64, u64)] = &[(SETTINGS_MAX_FIELD_SECTION_SIZE, 1 << 16)];
```

换成

```rust
    const SERVER_SETTINGS: &[(u64, u64)] = &[(SETTINGS_MAX_FIELD_SECTION_SIZE, 1 << 16)];
    /// The fake's SETTINGS for MASQUE: extended CONNECT and HTTP Datagrams.
    const MASQUE_SETTINGS: &[(u64, u64)] = &[
        (SETTINGS_MAX_FIELD_SECTION_SIZE, 1 << 16),
        (SETTINGS_ENABLE_CONNECT_PROTOCOL, 1),
        (SETTINGS_H3_DATAGRAM, 1),
    ];
```

`crates/rurge-net/src/quic/h3/mod.rs`——把

```rust
        let (endpoint, quic, server_quic) = connected().await;
        Pair {
            client: Client::new(quic.clone()),
            server: timeout(BOUND, H3Server::start(server_quic.clone(), SERVER_SETTINGS))
```

换成

```rust
        pair_with(Client::new, SERVER_SETTINGS, |_| {}).await
    }

    /// A client with datagrams, and a fake that allows CONNECT-UDP.
    async fn masque_pair() -> Pair {
        pair_with(Client::with_datagrams, MASQUE_SETTINGS, |_| {}).await
    }

    /// A client `made` on a connection whose server's transport is
    /// `tune`d, and the fake with `settings` sent.
    async fn pair_with(
        made: fn(Connection) -> Client,
        settings: &[(u64, u64)],
        tune: impl FnOnce(&mut TransportConfig),
    ) -> Pair {
        let (endpoint, quic, server_quic) = connected_with(&Transport::STANDARD, tune).await;
        Pair {
            client: made(quic.clone()),
            server: timeout(BOUND, H3Server::start(server_quic.clone(), settings))
```

`crates/rurge-net/src/quic/h3/mod.rs`——把

```rust
        Request::connect(authority).body(()).unwrap()
```

换成

```rust
        Request::connect(authority).body(()).unwrap()
    }

    /// An extended CONNECT for CONNECT-UDP (RFC 9298) through the proxy
    /// `proxy.test:4433`, to `path`.
    fn connect_udp(path: &str) -> Request<()> {
        let mut request = Request::connect(format!("https://proxy.test:4433{path}"))
            .header("capsule-protocol", "?1")
            .body(())
            .unwrap();
        request.extensions_mut().insert(Protocol::CONNECT_UDP);
        request
    }

    /// A CONNECT-UDP request answered 200: the client's stream and the
    /// fake's.
    async fn udp_tunnel(p: &Pair, path: &str) -> (RequestStream, ServerStream) {
        let request = async {
            let mut stream = p.client.request(connect_udp(path)).await.unwrap();
            assert_eq!(stream.response().await.unwrap().status(), 200);
            stream
        };
        let serve = async {
            let mut stream = p.server.accept().await.expect("a request");
            stream.read_headers().await.unwrap();
            stream
                .send_headers(200, &[("capsule-protocol", "?1")])
                .await
                .unwrap();
            stream
        };
        timeout(BOUND, async { tokio::join!(request, serve) })
            .await
            .expect("within the bound")
```

`crates/rurge-net/src/quic/h3/mod.rs`——把

```rust
        let cases: [(&str, &[Act], VarInt); 15] = [
```

换成

```rust
        let cases: [(&str, &[Act], VarInt); 17] = [
```

`crates/rurge-net/src/quic/h3/mod.rs`——把

```rust
                &[Settings(&[(0x06, 1), (0x06, 2)])],
```

换成

```rust
                &[Settings(&[(0x06, 1), (0x06, 2)])],
                H3_SETTINGS_ERROR,
            ),
            (
                "extended CONNECT neither on nor off",
                &[Settings(&[(0x08, 2)])],
                H3_SETTINGS_ERROR,
            ),
            (
                "HTTP Datagrams neither on nor off",
                &[Settings(&[(0x33, 2)])],
```

`crates/rurge-net/src/quic/h3/mod.rs`——把

```rust
        assert_eq!(code_name(VarInt::from_u32(0x1234)), "0x1234");
```

换成

```rust
        assert_eq!(code_name(H3_DATAGRAM_ERROR), "H3_DATAGRAM_ERROR");
        assert_eq!(code_name(VarInt::from_u32(0x1234)), "0x1234");
    }

    /// An extended CONNECT says `:protocol`, `:scheme` and `:path` too (RFC
    /// 8441 4): the scheme and the path of its URI — `https` and `/` when
    /// it has none.
    #[tokio::test]
    async fn an_extended_connect_says_its_protocol_scheme_and_path() {
        let p = masque_pair().await;
        for (uri, scheme, path) in [
            (
                "https://proxy.test:4433/.well-known/masque/udp/192.0.2.6/443/",
                "https",
                "/.well-known/masque/udp/192.0.2.6/443/",
            ),
            ("proxy.test:4433", "https", "/"),
        ] {
            let mut request = Request::connect(uri)
                .header("capsule-protocol", "?1")
                .body(())
                .unwrap();
            request.extensions_mut().insert(Protocol::CONNECT_UDP);
            let client = async {
                let mut stream = p.client.request(request).await.unwrap();
                stream.response().await.unwrap().status()
            };
            let server = async {
                let mut stream = p.server.accept().await.unwrap();
                let head = stream.read_headers().await.unwrap();
                stream.send_headers(200, &[]).await.unwrap();
                head
            };
            let (status, head) = timeout(BOUND, async { tokio::join!(client, server) })
                .await
                .unwrap();
            assert_eq!(status, 200);
            assert_eq!(
                head,
                owned(&[
                    (":method", "CONNECT"),
                    (":protocol", "connect-udp"),
                    (":scheme", scheme),
                    (":authority", "proxy.test:4433"),
                    (":path", path),
                    ("capsule-protocol", "?1"),
                ])
            );
        }
    }

    /// An extended CONNECT goes only once the server's SETTINGS enabled it
    /// (RFC 8441 3): else it is refused, and nothing goes out.
    #[tokio::test]
    async fn an_extended_connect_needs_the_servers_settings_to_allow_it() {
        for settings in [&[][..], &[(SETTINGS_ENABLE_CONNECT_PROTOCOL, 0)]] {
            let p = pair_with(Client::with_datagrams, settings, |_| {}).await;
            let err = timeout(BOUND, p.client.request(connect_udp("/")))
                .await
                .unwrap()
                .unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::Unsupported, "{settings:?}");
            assert_eq!(
                err.to_string(),
                "h3: the server does not support extended CONNECT"
            );
            // the next request is the connection's first stream
            let (sent, served) = timeout(BOUND, async {
                tokio::join!(
                    p.client.request(get("https://example.test/")),
                    p.server.accept()
                )
            })
            .await
            .unwrap();
            assert_eq!(sent.unwrap().id(), 0);
            assert_eq!(served.unwrap().id(), 0);
        }
    }

    /// An extended CONNECT waits for the server's SETTINGS, and goes once
    /// they come.
    #[tokio::test]
    async fn an_extended_connect_waits_for_the_servers_settings() {
        let (_endpoint, quic, server_quic) = connected().await;
        let client = Client::new(quic);
        let mut server = H3Server::new(server_quic);
        let request = client.request(connect_udp("/"));
        tokio::pin!(request);
        assert!(
            timeout(Duration::from_millis(200), &mut request)
                .await
                .is_err(),
            "sent before the server's SETTINGS"
        );
        timeout(BOUND, async {
            server
                .send_settings(&[(SETTINGS_ENABLE_CONNECT_PROTOCOL, 1)])
                .await
                .unwrap();
            let (sent, served) = tokio::join!(request, server.accept());
            assert_eq!(sent.unwrap().id(), 0);
            let head = served.unwrap().read_headers().await.unwrap();
            assert_eq!(
                head[..2],
                owned(&[(":method", "CONNECT"), (":protocol", "connect-udp")])
            );
        })
        .await
        .unwrap();
    }

    /// Only a client made with HTTP Datagrams announces them (RFC 9297
    /// 2.1.1): a server would take Hysteria 2's own datagrams for HTTP/3's.
    #[tokio::test]
    async fn only_a_client_with_datagrams_announces_them() {
        for datagrams in [false, true] {
            let made: fn(Connection) -> Client = if datagrams {
                Client::with_datagrams
            } else {
                Client::new
            };
            let p = pair_with(made, SERVER_SETTINGS, |_| {}).await;
            let settings = timeout(BOUND, p.server.client_settings())
                .await
                .unwrap()
                .unwrap();
            let mut expected = vec![
                (SETTINGS_QPACK_MAX_TABLE_CAPACITY, 0),
                (SETTINGS_MAX_FIELD_SECTION_SIZE, 65_536),
                (SETTINGS_QPACK_BLOCKED_STREAMS, 0),
            ];
            if datagrams {
                expected.push((SETTINGS_H3_DATAGRAM, 1));
            }
            assert_eq!(settings, expected);
        }
    }

    /// A client made without datagrams leaves the connection's QUIC
    /// datagrams to the protocol: Hysteria 2 reads its UDP there.
    #[tokio::test]
    async fn a_client_without_datagrams_leaves_them_to_the_protocol() {
        let p = pair().await;
        // no HTTP/3 datagram: half a quarter stream ID
        p.server_quic
            .send_datagram(Bytes::from_static(&[0x40]))
            .unwrap();
        let datagram = timeout(BOUND, p.quic.read_datagram())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(datagram, &[0x40][..]);
        assert_eq!(round_trip(&p.client, &p.server).await.unwrap(), 204);
    }

    /// Each stream's datagrams go out with its quarter stream ID and come
    /// back to it by theirs (RFC 9297 2.1).
    #[tokio::test]
    async fn datagrams_go_both_ways_by_stream() {
        let p = masque_pair().await;
        timeout(BOUND, async {
            let (mut a, mut served_a) = udp_tunnel(&p, "/a").await;
            let (mut b, mut served_b) = udp_tunnel(&p, "/b").await;
            assert_eq!((a.id(), served_a.id(), b.id(), served_b.id()), (0, 0, 4, 4));
            let (mut sa, mut sb) = (served_a.datagrams(), served_b.datagrams());
            let da = a.datagrams().await.unwrap();
            let db = b.datagrams().await.unwrap();
            da.send(b"to a").unwrap();
            db.send(b"to b").unwrap();
            assert_eq!(sa.recv().await.unwrap(), b"to a");
            assert_eq!(sb.recv().await.unwrap(), b"to b");
            sb.send(b"from b").unwrap();
            sa.send(b"from a").unwrap();
            assert_eq!(da.recv().await.unwrap(), &b"from a"[..]);
            assert_eq!(db.recv().await.unwrap(), &b"from b"[..]);
            // an empty payload, each way
            da.send(b"").unwrap();
            assert_eq!(sa.recv().await.unwrap(), b"");
            sa.send(b"").unwrap();
            assert_eq!(da.recv().await.unwrap(), &b""[..]);
        })
        .await
        .unwrap();
    }

    /// A datagram for a stream whose datagrams do not flow — one never
    /// opened, the last QUIC can have, one whose datagrams were not taken,
    /// one whose handle is gone — is dropped, and the connection serves
    /// on.
    #[tokio::test]
    async fn a_datagram_for_a_stream_without_a_flow_is_dropped() {
        let p = masque_pair().await;
        timeout(BOUND, async {
            let (_a, mut served_a) = udp_tunnel(&p, "/a").await;
            let (mut b, mut served_b) = udp_tunnel(&p, "/b").await;
            let (sa, sb) = (served_a.datagrams(), served_b.datagrams());
            let db = b.datagrams().await.unwrap();
            // quarter stream IDs 100 (stream 400) and 2^60 - 1
            for datagram in [
                &[0x40, 0x64, b'?'][..],
                &[0xcf, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
            ] {
                p.server_quic
                    .send_datagram(Bytes::copy_from_slice(datagram))
                    .unwrap();
            }
            sa.send(b"not taken").unwrap();
            sb.send(b"taken").unwrap();
            assert_eq!(db.recv().await.unwrap(), &b"taken"[..]);
            drop(db);
            sb.send(b"gone").unwrap();
            let (mut c, mut served_c) = udp_tunnel(&p, "/c").await;
            let sc = served_c.datagrams();
            let dc = c.datagrams().await.unwrap();
            sc.send(b"c").unwrap();
            assert_eq!(dc.recv().await.unwrap(), &b"c"[..]);
        })
        .await
        .unwrap();
        assert!(p.quic.close_reason().is_none());
    }

    /// A datagram too short for its quarter stream ID, or naming a stream
    /// QUIC cannot have, closes the connection (H3_DATAGRAM_ERROR, RFC 9297
    /// 2.1).
    #[tokio::test]
    async fn a_datagram_the_client_cannot_place_closes_the_connection() {
        let short = "h3: the server sent a datagram too short for a quarter stream ID";
        for (datagram, text) in [
            (&[][..], short),
            (&[0x40], short),
            // 2^60
            (
                &[0xd0, 0, 0, 0, 0, 0, 0, 0],
                "h3: the server sent a datagram for a stream QUIC cannot have",
            ),
        ] {
            let p = masque_pair().await;
            p.server_quic
                .send_datagram(Bytes::copy_from_slice(datagram))
                .unwrap();
            assert_eq!(
                closed_with(&p.server_quic).await,
                u64::from(H3_DATAGRAM_ERROR),
                "{datagram:02x?}"
            );
            let err = p
                .client
                .request(get("https://example.test/"))
                .await
                .unwrap_err();
            assert_eq!(err.to_string(), text);
        }
    }

    /// A payload longer than a datagram takes now is refused, and nothing
    /// goes out.
    #[tokio::test]
    async fn a_datagram_too_long_is_refused_and_nothing_goes_out() {
        let p = masque_pair().await;
        timeout(BOUND, async {
            let (mut a, mut served) = udp_tunnel(&p, "/").await;
            let mut sa = served.datagrams();
            let da = a.datagrams().await.unwrap();
            // no task runs between the lines below, the connection's
            // neither: the path's MTU, and what fits, stay as they are
            let max = da.max_payload();
            // stream 0's quarter stream ID takes a byte
            assert_eq!(max, p.quic.max_datagram_size().unwrap() - 1);
            let err = da.send(&vec![1; max + 1]).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(
                err.to_string(),
                format!("h3: a datagram longer than {max} bytes")
            );
            da.send(&vec![2; max]).unwrap();
            assert_eq!(sa.recv().await.unwrap(), vec![2; max]);
        })
        .await
        .unwrap();
    }

    /// A stream's datagrams need a client made with them, a server whose
    /// SETTINGS say `H3_DATAGRAM = 1` and QUIC datagrams on the
    /// connection; the connection serves on without them.
    #[tokio::test]
    async fn datagrams_need_the_servers_support() {
        let without: &[(u64, u64)] = &[(SETTINGS_ENABLE_CONNECT_PROTOCOL, 1)];
        let off: &[(u64, u64)] = &[
            (SETTINGS_ENABLE_CONNECT_PROTOCOL, 1),
            (SETTINGS_H3_DATAGRAM, 0),
        ];
        for (case, settings, quic_datagrams) in [
            ("no H3_DATAGRAM", without, true),
            ("H3_DATAGRAM = 0", off, true),
            ("no QUIC datagrams", MASQUE_SETTINGS, false),
        ] {
            let p = pair_with(Client::with_datagrams, settings, |transport| {
                if !quic_datagrams {
                    transport.datagram_receive_buffer_size(None);
                }
            })
            .await;
            timeout(BOUND, async {
                let (mut a, _served) = udp_tunnel(&p, "/").await;
                let err = a.datagrams().await.unwrap_err();
                assert_eq!(err.kind(), io::ErrorKind::Unsupported, "{case}");
                assert_eq!(
                    err.to_string(),
                    "h3: the server does not support HTTP datagrams",
                    "{case}"
                );
            })
            .await
            .unwrap();
            assert!(p.quic.close_reason().is_none(), "{case}");
        }
        let p = pair_with(Client::new, MASQUE_SETTINGS, |_| {}).await;
        timeout(BOUND, async {
            let (mut a, _served) = udp_tunnel(&p, "/").await;
            let err = a.datagrams().await.unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::Unsupported);
            assert_eq!(
                err.to_string(),
                "h3: the client was made without HTTP datagrams"
            );
        })
        .await
        .unwrap();
    }

    /// A stream that does not read its datagrams holds up no other: those
    /// past what its queue holds are dropped.
    #[tokio::test]
    async fn a_full_queue_drops_rather_than_blocks() {
        let p = masque_pair().await;
        timeout(BOUND, async {
            let (mut a, mut served_a) = udp_tunnel(&p, "/a").await;
            let (mut b, mut served_b) = udp_tunnel(&p, "/b").await;
            let (sa, sb) = (served_a.datagrams(), served_b.datagrams());
            let da = a.datagrams().await.unwrap();
            let db = b.datagrams().await.unwrap();
            for i in 0..100u8 {
                sa.send(&[i]).unwrap();
            }
            sb.send(b"b").unwrap();
            assert_eq!(db.recv().await.unwrap(), &b"b"[..]);
            // a's datagrams flow no more: what its queue holds, then the end
            drop(a);
            let mut queued = 0;
            while da.recv().await.is_some() {
                queued += 1;
            }
            assert_eq!(queued, datagram::QUEUE);
        })
        .await
        .unwrap();
    }

    /// A stream's datagrams end with it — once the server ended its side,
    /// once the stream is dropped, once the connection ended: `recv` ends
    /// and `send` fails. They are taken once, and not once the stream
    /// ended.
    #[tokio::test]
    async fn datagrams_end_with_their_stream() {
        let p = masque_pair().await;
        timeout(BOUND, async {
            let (mut a, mut served_a) = udp_tunnel(&p, "/a").await;
            let da = a.datagrams().await.unwrap();
            served_a.finish();
            a.read_to_end(&mut Vec::new()).await.unwrap();
            assert!(da.recv().await.is_none());
            let err = da.send(b"late").unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
            assert_eq!(err.to_string(), "h3: the stream is closed");
            let err = a.datagrams().await.unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(
                err.to_string(),
                "h3: the stream's datagrams were taken already"
            );
            let (mut b, mut served_b) = udp_tunnel(&p, "/b").await;
            served_b.finish();
            b.read_to_end(&mut Vec::new()).await.unwrap();
            let err = b.datagrams().await.unwrap_err();
            assert_eq!(err.to_string(), "h3: the stream is closed");
            // the client drops its stream
            let (mut c, _served_c) = udp_tunnel(&p, "/c").await;
            let dc = c.datagrams().await.unwrap();
            drop(c);
            assert!(dc.recv().await.is_none());
            assert_eq!(
                dc.send(b"late").unwrap_err().to_string(),
                "h3: the stream is closed"
            );
            // the connection ends
            let (mut d, _served_d) = udp_tunnel(&p, "/d").await;
            let dd = d.datagrams().await.unwrap();
            p.server_quic.close(VarInt::from_u32(0x100), b"bye");
            assert!(dd.recv().await.is_none());
            assert_eq!(
                dd.send(b"late").unwrap_err().to_string(),
                "quic: the server closed the connection (code 0x100: bye)"
            );
        })
        .await
        .unwrap();
    }

    /// Header fields as the fake sends them.
    type Fields = &'static [(&'static str, &'static str)];
    /// The payloads of DATA frames as the fake sends them.
    type Payloads = &'static [&'static [u8]];

    /// How the fake ends a response's content.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum End {
        Fin,
        /// Trailers, the stream left open.
        Trailers,
        /// Nothing more, the stream left open.
        Open,
    }

    /// A response's content must be as long as its `content-length` says,
    /// which must be a number (RFC 9114 4.1.2): else the response is
    /// malformed, its stream ends (H3_MESSAGE_ERROR), the connection
    /// serves on.
    #[tokio::test]
    async fn content_that_its_content_length_does_not_match_is_malformed() {
        const SHORTER: &str =
            "h3: the server sent a malformed response (shorter than its content-length)";
        const LONGER: &str =
            "h3: the server sent a malformed response (longer than its content-length)";
        let p = pair().await;
        let cases: [(&str, Payloads, End, &str); 5] = [
            ("5", &[b"abc"], End::Fin, SHORTER),
            ("5", &[b"abc"], End::Trailers, SHORTER),
            ("3", &[b"abcde"], End::Open, LONGER),
            ("3", &[b"ab", b"cd"], End::Open, LONGER),
            ("0", &[b"x"], End::Open, LONGER),
        ];
        for (length, frames, end, text) in cases {
            let request = async {
                let mut stream = p.client.request(get("https://example.test/")).await?;
                stream.shutdown().await?;
                assert_eq!(stream.response().await?.status(), 200);
                stream.read_to_end(&mut Vec::new()).await
            };
            let serve = async {
                let mut stream = p.server.accept().await.unwrap();
                stream.read_headers().await.unwrap();
                stream
                    .send_headers(200, &[("content-length", length)])
                    .await
                    .unwrap();
                // a frame after the client stopped the stream finds it gone
                for frame in frames {
                    let _ = stream.send_data(frame).await;
                }
                match end {
                    End::Fin => stream.finish(),
                    End::Trailers => {
                        let _ = stream.send_frame(0x01, &[0x00, 0x00]).await;
                    }
                    End::Open => {}
                }
                stream
            };
            let (result, served) = timeout(BOUND, async { tokio::join!(request, serve) })
                .await
                .unwrap();
            let err = result.unwrap_err();
            let case = format!("{length} {frames:?} {end:?}");
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{case}");
            assert_eq!(err.to_string(), text, "{case}");
            // a stream the server ended takes no STOP_SENDING
            if end != End::Fin {
                assert_eq!(
                    timeout(BOUND, served.client_stopped()).await.unwrap(),
                    Some(u64::from(H3_MESSAGE_ERROR)),
                    "{case}"
                );
            }
        }
        for fields in [
            &[("content-length", "5x")][..],
            &[("content-length", "+5")],
            &[("content-length", "")],
            &[("content-length", "18446744073709551616")],
            &[("content-length", "3"), ("content-length", "4")],
        ] {
            let (response, served) = answered_with(&p, &response_section(200, fields)).await;
            assert_eq!(
                response.unwrap_err().to_string(),
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

    /// Content as long as the response's `content-length` says is read
    /// whole — in frames, the length given twice alike, trailers after it;
    /// that of a response without content — to HEAD, 204, 304 — is not
    /// counted.
    #[tokio::test]
    async fn content_as_long_as_its_content_length_is_read_whole() {
        let p = pair().await;
        let cases: [(Method, u16, Fields, Payloads); 6] = [
            (
                Method::GET,
                200,
                &[("content-length", "6")],
                &[b"abc", b"def"],
            ),
            (
                Method::GET,
                200,
                &[("content-length", "3"), ("content-length", "3")],
                &[b"abc"],
            ),
            (Method::GET, 200, &[("content-length", "0")], &[]),
            (Method::HEAD, 200, &[("content-length", "1234")], &[]),
            (Method::GET, 204, &[("content-length", "5")], &[]),
            (Method::GET, 304, &[("content-length", "5")], &[]),
        ];
        for (method, status, fields, frames) in cases {
            let request = Request::builder()
                .method(method.clone())
                .uri("https://example.test/")
                .body(())
                .unwrap();
            let client = async {
                let mut stream = p.client.request(request).await.unwrap();
                stream.shutdown().await.unwrap();
                assert_eq!(stream.response().await.unwrap().status(), status);
                let mut content = Vec::new();
                stream.read_to_end(&mut content).await.unwrap();
                content
            };
            let server = async {
                let mut stream = p.server.accept().await.unwrap();
                stream.read_headers().await.unwrap();
                stream.send_headers(status, fields).await.unwrap();
                for frame in frames {
                    stream.send_data(frame).await.unwrap();
                }
                stream.send_frame(0x01, &[0x00, 0x00]).await.unwrap();
                stream.finish();
            };
            let (content, ()) = timeout(BOUND, async { tokio::join!(client, server) })
                .await
                .unwrap();
            assert_eq!(content, frames.concat(), "{method} {status}");
        }
    }

    /// A 2xx to a CONNECT makes a tunnel, whose `content-length` is ignored
    /// (RFC 9110 9.3.6): the bytes go both ways.
    #[tokio::test]
    async fn a_tunnel_ignores_its_content_length() {
        let p = pair().await;
        let client = async {
            let mut stream = p.client.request(tunnel("example.test:443")).await.unwrap();
            assert_eq!(stream.response().await.unwrap().status(), 200);
            stream.write_all(b"hello").await.unwrap();
            stream.shutdown().await.unwrap();
            let mut back = Vec::new();
            stream.read_to_end(&mut back).await.unwrap();
            back
        };
        let server = async {
            let mut stream = p.server.accept().await.unwrap();
            stream.read_headers().await.unwrap();
            stream
                .send_headers(200, &[("content-length", "0")])
                .await
                .unwrap();
            let content = stream.read_body().await.unwrap();
            stream.send_data(b"world").await.unwrap();
            stream.finish();
            content
        };
        let (back, content) = timeout(BOUND, async { tokio::join!(client, server) })
            .await
            .unwrap();
        assert_eq!(content, b"hello");
        assert_eq!(back, b"world");
    }

    /// Nothing but DATA follows a tunnel's head (RFC 9114 4.4): HEADERS
    /// close the connection (H3_FRAME_UNEXPECTED), on a classic CONNECT or
    /// an extended one; a frame of an unknown type is passed over.
    #[tokio::test]
    async fn headers_on_a_tunnel_close_the_connection() {
        for connect in [tunnel("example.test:443"), connect_udp("/")] {
            let p = masque_pair().await;
            let request = async {
                let mut stream = p.client.request(connect).await?;
                assert_eq!(stream.response().await?.status(), 200);
                let mut content = Vec::new();
                let read = stream.read_to_end(&mut content).await;
                assert_eq!(content, b"x");
                read
            };
            let serve = async {
                let mut stream = p.server.accept().await.unwrap();
                stream.read_headers().await.unwrap();
                stream.send_headers(200, &[]).await.unwrap();
                stream.send_frame(0x21, b"reserved").await.unwrap();
                stream.send_data(b"x").await.unwrap();
                // trailers; after the close the stream is gone
                let _ = stream.send_frame(0x01, &[0x00, 0x00]).await;
                stream
            };
            let (result, _served) = timeout(BOUND, async { tokio::join!(request, serve) })
                .await
                .unwrap();
            assert_eq!(
                result.unwrap_err().to_string(),
                "h3: the server sent HEADERS on a CONNECT tunnel"
            );
            assert_eq!(
                closed_with(&p.server_quic).await,
                u64::from(H3_FRAME_UNEXPECTED)
            );
        }
    }

    /// A CONNECT's response other than a 2xx makes no tunnel (RFC 9110
    /// 9.3.6): its content is counted and its trailers passed over, as any
    /// response's.
    #[tokio::test]
    async fn a_refused_connect_is_an_ordinary_response() {
        let p = pair().await;
        let client = async {
            let mut stream = p.client.request(tunnel("example.test:443")).await.unwrap();
            let status = stream.response().await.unwrap().status();
            let mut content = Vec::new();
            stream.read_to_end(&mut content).await.unwrap();
            (status, content)
        };
        let server = async {
            let mut stream = p.server.accept().await.unwrap();
            stream.read_headers().await.unwrap();
            let fields = [
                ("proxy-authenticate", "Basic realm=\"masque\""),
                ("content-length", "6"),
            ];
            stream.send_headers(407, &fields).await.unwrap();
            stream.send_data(b"denied").await.unwrap();
            stream.send_frame(0x01, &[0x00, 0x00]).await.unwrap();
            stream.finish();
        };
        let ((status, content), ()) = timeout(BOUND, async { tokio::join!(client, server) })
            .await
            .unwrap();
        assert_eq!(status, 407);
        assert_eq!(content, b"denied");
        assert!(p.quic.close_reason().is_none());
    }

    /// The authority goes without userinfo (RFC 9114 4.3.1), a CONNECT's
    /// too.
    #[tokio::test]
    async fn the_authority_carries_no_userinfo() {
        let p = pair().await;
        for (request, expected) in [
            (
                get("https://user:secret@example.test:8443/x"),
                owned(&[
                    (":method", "GET"),
                    (":scheme", "https"),
                    (":authority", "example.test:8443"),
                    (":path", "/x"),
                ]),
            ),
            (
                tunnel("user:secret@example.test:443"),
                owned(&[(":method", "CONNECT"), (":authority", "example.test:443")]),
            ),
        ] {
            let (sent, head) = timeout(BOUND, async {
                tokio::join!(p.client.request(request), async {
                    let mut stream = p.server.accept().await.unwrap();
                    stream.read_headers().await.unwrap()
                })
            })
            .await
            .unwrap();
            sent.unwrap();
            assert_eq!(head, expected);
        }
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-net --lib quic::h3`
Expected: FAIL——`Protocol`、`with_datagrams`、`datagrams`、`id` 与 `content-length` 的核对由 Step 3 引入，编译不过：

```text
error[E0433]: failed to resolve: use of undeclared type `Protocol`
error[E0433]: failed to resolve: use of undeclared type `HeaderMap`
error[E0425]: cannot find value `CONTENT_LENGTH` in this scope
error[E0425]: cannot find value `H3_DATAGRAM_ERROR` in this scope
error[E0599]: no function or associated item named `with_datagrams` found for struct `h3::client::Client` in the current scope
error[E0560]: struct `h3::client::Inner` has no field named `flows`
error[E0425]: cannot find function `content_length` in this scope
error[E0599]: no method named `id` found for struct `h3::stream::RequestStream` in the current scope
error[E0599]: no method named `datagrams` found for struct `h3::stream::RequestStream` in the current scope
error[E0433]: failed to resolve: use of unresolved module or unlinked crate `datagram`
error: could not compile `rurge-net` (lib test) due to 32 previous errors
```

- [ ] **Step 3: 实现**

新建 `crates/rurge-net/src/quic/h3/datagram.rs`：

```rust
//! HTTP/3 datagrams (RFC 9297 2.1; phase 2 M7 design 4) on a client made
//! `with_datagrams`: each QUIC datagram of the connection carries
//! `varint(stream ID ÷ 4) ‖ payload`, either way.
//!
//! - A request stream's datagrams flow once taken
//!   (`RequestStream::datagrams`) until either direction of the stream
//!   ends — as reading the stream to its end or failure, its shutdown or a
//!   failed write finds —, the stream or the handle is dropped, or the
//!   connection ends; then `recv` hands over what is queued and ends, and
//!   `send` fails.
//! - One task reads the connection's datagrams into the queue of the
//!   stream each names. A datagram too short for its quarter stream ID, or
//!   naming a stream QUIC cannot have, closes the connection
//!   (H3_DATAGRAM_ERROR). One for a stream whose datagrams do not flow —
//!   never opened, ended, or one whose request has no use for datagrams,
//!   which RFC 9297 2 would abort with H3_DATAGRAM_ERROR — is dropped, and
//!   so is one that meets a full queue: the reader waits for no stream.
//! - Sending never waits: a payload a QUIC datagram on the path cannot
//!   carry now is refused, and nothing goes out.

use super::client::Inner;
use super::frame;
use super::{H3_DATAGRAM_ERROR, H3Error};
use bytes::{Buf, Bytes, BytesMut};
use quinn::SendDatagramError;
use std::collections::HashMap;
use std::fmt;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::mpsc;

/// The datagrams that wait for a stream's reader; more are dropped, as a
/// socket would.
pub(super) const QUEUE: usize = 64;

/// The largest quarter stream ID: that of QUIC's largest stream ID,
/// 2^62 − 1 (RFC 9297 2.1).
const MAX_QUARTER_STREAM_ID: u64 = (1 << 60) - 1;

/// The streams whose datagrams flow, by quarter stream ID, and the queue
/// of each.
#[derive(Default)]
pub(super) struct Flows(Mutex<Table>);

#[derive(Default)]
struct Table {
    queues: HashMap<u64, mpsc::Sender<Bytes>>,
    /// The reading ended: no stream's datagrams flow any more.
    ended: bool,
}

impl Flows {
    fn table(&self) -> MutexGuard<'_, Table> {
        self.0.lock().expect("flows")
    }

    /// The queue of stream `quarter`'s datagrams, which flow from now on;
    /// `None` once the reading ended.
    pub(super) fn open(&self, quarter: u64) -> Option<mpsc::Receiver<Bytes>> {
        let (queue, incoming) = mpsc::channel(QUEUE);
        let mut table = self.table();
        if table.ended {
            return None;
        }
        table.queues.insert(quarter, queue);
        Some(incoming)
    }

    /// Stream `quarter`'s datagrams flow no more: its queue ends after
    /// what it holds.
    pub(super) fn close(&self, quarter: u64) {
        self.table().queues.remove(&quarter);
    }

    fn is_open(&self, quarter: u64) -> bool {
        self.table().queues.contains_key(&quarter)
    }

    /// Hands `payload` to the queue of stream `quarter`. Dropped when the
    /// stream's datagrams do not flow, or its queue is full.
    fn deliver(&self, quarter: u64, payload: Bytes) {
        if let Some(queue) = self.table().queues.get(&quarter) {
            let _ = queue.try_send(payload);
        }
    }

    /// The reading ended: every queue ends after what it holds.
    fn end(&self) {
        let mut table = self.table();
        table.queues.clear();
        table.ended = true;
    }
}

/// Reads the connection's datagrams into the queues of the streams they
/// name until the connection ends — or the client goes, and its tasks with
/// it. Every flow ends with the reading, however it ends.
pub(super) async fn read(inner: Arc<Inner>) {
    let Some(flows) = &inner.flows else {
        return;
    };
    let _end = EndFlows(flows);
    while let Ok(datagram) = inner.quic.read_datagram().await {
        match parse(datagram) {
            Ok((quarter, payload)) => flows.deliver(quarter, payload),
            Err(error) => {
                inner.fail(error);
                return;
            }
        }
    }
}

/// Ends every flow when dropped: when the reading ends.
struct EndFlows<'a>(&'a Flows);

impl Drop for EndFlows<'_> {
    fn drop(&mut self) {
        self.0.end();
    }
}

/// The quarter stream ID at the start of `datagram`, and the payload after
/// it (RFC 9297 2.1).
fn parse(mut datagram: Bytes) -> Result<(u64, Bytes), H3Error> {
    let Some((quarter, n)) = frame::get_varint(&datagram) else {
        return Err(H3Error::new(
            H3_DATAGRAM_ERROR,
            "the server sent a datagram too short for a quarter stream ID",
        ));
    };
    if quarter > MAX_QUARTER_STREAM_ID {
        return Err(H3Error::new(
            H3_DATAGRAM_ERROR,
            "the server sent a datagram for a stream QUIC cannot have",
        ));
    }
    datagram.advance(n);
    Ok((quarter, datagram))
}

/// The HTTP/3 datagrams of one request stream (`RequestStream::datagrams`).
/// Dropped, the stream's datagrams flow no more.
pub struct Datagrams {
    inner: Arc<Inner>,
    quarter: u64,
    /// `varint(quarter)`, which every datagram sent starts with.
    prefix: Bytes,
    incoming: tokio::sync::Mutex<mpsc::Receiver<Bytes>>,
}

impl Datagrams {
    pub(super) fn new(
        inner: Arc<Inner>,
        quarter: u64,
        incoming: mpsc::Receiver<Bytes>,
    ) -> Datagrams {
        let mut prefix = BytesMut::new();
        frame::put_varint(&mut prefix, quarter);
        Datagrams {
            inner,
            quarter,
            prefix: prefix.freeze(),
            incoming: tokio::sync::Mutex::new(incoming),
        }
    }

    /// The longest payload a datagram carries now: what a QUIC datagram on
    /// the connection's path takes, less the quarter stream ID. It changes
    /// with the path.
    pub fn max_payload(&self) -> usize {
        self.inner
            .quic
            .max_datagram_size()
            .map_or(0, |max| max.saturating_sub(self.prefix.len()))
    }

    /// Sends `payload` in one datagram without waiting: QUIC sends it when
    /// it can — or drops it, as it drops the oldest of those not sent yet
    /// to make room for a new one. Refused, nothing sent, for a payload
    /// longer than `max_payload`, and once the stream's datagrams ended.
    pub fn send(&self, payload: &[u8]) -> io::Result<()> {
        let open = self
            .inner
            .flows
            .as_ref()
            .is_some_and(|flows| flows.is_open(self.quarter));
        if !open {
            return Err(self.inner.flow_ended());
        }
        let max = self.max_payload();
        if payload.len() > max {
            return Err(too_long(max));
        }
        let mut datagram = BytesMut::with_capacity(self.prefix.len() + payload.len());
        datagram.extend_from_slice(&self.prefix);
        datagram.extend_from_slice(payload);
        self.inner
            .quic
            .send_datagram(datagram.freeze())
            .map_err(|e| match e {
                SendDatagramError::ConnectionLost(e) => self.inner.lost(&e),
                // the path narrowed since `max` was taken
                SendDatagramError::TooLarge => too_long(self.max_payload()),
                SendDatagramError::UnsupportedByPeer | SendDatagramError::Disabled => {
                    io::Error::new(io::ErrorKind::Unsupported, format!("h3: {e}"))
                }
            })
    }

    /// The payload of the next datagram the server sent for the stream;
    /// `None` once its datagrams ended and those queued were taken.
    pub async fn recv(&self) -> Option<Bytes> {
        self.incoming.lock().await.recv().await
    }
}

fn too_long(max: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("h3: a datagram longer than {max} bytes"),
    )
}

impl Drop for Datagrams {
    fn drop(&mut self) {
        if let Some(flows) = &self.inner.flows {
            flows.close(self.quarter);
        }
    }
}

impl fmt::Debug for Datagrams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Datagrams")
            .field("stream", &(self.quarter * 4))
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A datagram's quarter stream ID in any length; QUIC's last stream is
    /// the last one (RFC 9297 2.1).
    #[test]
    fn a_datagram_starts_with_its_quarter_stream_id() {
        for (bytes, quarter, payload) in [
            (&[0x00][..], 0, &b""[..]),
            (&[0x01, b'u', b'p'], 1, b"up"),
            // 256 longer than it needs
            (&[0x80, 0x00, 0x01, 0x00, b'x'], 256, b"x"),
            (
                &[0xcf, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
                (1 << 60) - 1,
                b"",
            ),
        ] {
            let (got, rest) = parse(Bytes::copy_from_slice(bytes)).unwrap();
            assert_eq!((got, &rest[..]), (quarter, payload), "{bytes:02x?}");
        }
    }

    #[test]
    fn a_datagram_without_a_quarter_stream_id_of_quic_is_an_error() {
        let short = "the server sent a datagram too short for a quarter stream ID";
        for (bytes, text) in [
            (&[][..], short),
            (&[0x40], short),
            (&[0xc0, 0, 0, 0], short),
            // 2^60
            (
                &[0xd0, 0, 0, 0, 0, 0, 0, 0],
                "the server sent a datagram for a stream QUIC cannot have",
            ),
        ] {
            let error = parse(Bytes::copy_from_slice(bytes)).unwrap_err();
            assert_eq!(error.code, H3_DATAGRAM_ERROR, "{bytes:02x?}");
            assert_eq!(error.text, text, "{bytes:02x?}");
        }
    }

    /// Each stream's datagrams go to its own queue; one for a stream whose
    /// datagrams do not flow is dropped, and so is one that meets a full
    /// queue — the others' are delivered all the same.
    #[test]
    fn datagrams_go_to_their_streams_queues() {
        let flows = Flows::default();
        let mut a = flows.open(0).unwrap();
        let mut b = flows.open(1).unwrap();
        flows.deliver(7, Bytes::from_static(b"nobody's"));
        for i in 0..QUEUE + 10 {
            flows.deliver(0, Bytes::from(vec![i as u8]));
        }
        flows.deliver(1, Bytes::from_static(b"b"));
        assert_eq!(b.try_recv().unwrap(), &b"b"[..]);
        for i in 0..QUEUE {
            assert_eq!(a.try_recv().unwrap(), vec![i as u8], "{i}");
        }
        assert!(a.try_recv().is_err(), "the full queue's overflow dropped");
        assert!(flows.is_open(0) && !flows.is_open(7));
    }

    /// A flow closed, or every flow once the reading ended: the queue ends
    /// after what it holds, and no flow opens any more.
    #[test]
    fn flows_end() {
        let flows = Flows::default();
        let mut a = flows.open(0).unwrap();
        let mut b = flows.open(1).unwrap();
        flows.deliver(0, Bytes::from_static(b"queued"));
        flows.close(0);
        flows.deliver(0, Bytes::from_static(b"late"));
        assert_eq!(a.try_recv().unwrap(), &b"queued"[..]);
        assert_eq!(a.try_recv(), Err(mpsc::error::TryRecvError::Disconnected));
        assert!(!flows.is_open(0));
        flows.deliver(1, Bytes::from_static(b"queued"));
        flows.end();
        assert_eq!(b.try_recv().unwrap(), &b"queued"[..]);
        assert_eq!(b.try_recv(), Err(mpsc::error::TryRecvError::Disconnected));
        assert!(flows.open(2).is_none());
    }
}
```

`crates/rurge-net/src/quic/h3/mod.rs`——把

```rust
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
```

换成

```rust
//! protocols need of HTTP/3 — Hysteria 2's authentication request,
//! MASQUE's and `trust-tunnel`'s CONNECT, and DNS over HTTP/3.
//!
//! - Our control stream carries our SETTINGS: no dynamic QPACK table
//!   (capacity 0, no blocked streams) and field sections of at most 64 KiB;
//!   a client made `with_datagrams` adds `H3_DATAGRAM = 1`. We open no
//!   QPACK streams: an encoder that never uses a dynamic table and a
//!   decoder that has none have nothing to say on them (RFC 9204 4.2).
//! - Of the server's streams, its control stream must start with SETTINGS
//!   — those of extended CONNECT and of HTTP Datagrams 0 or 1 — and may
//!   then bring GOAWAY; its QPACK streams are read and their bytes dropped;
//!   a push stream is a connection error (we never send MAX_PUSH_ID); a
//!   stream of any other type is stopped.
```

`crates/rurge-net/src/quic/h3/mod.rs`——把

```rust
//!   as the stream's bytes. The trailers are ignored.
```

换成

```rust
//!   as the stream's bytes, as long as its `content-length` says. The
//!   trailers are ignored; a tunnel — a CONNECT's 2xx — has none.
//! - A CONNECT is classic — `:method` and `:authority` alone (RFC 9114
//!   4.4) — or, carrying a `Protocol`, extended (RFC 9220), which goes only
//!   once the server's SETTINGS allowed it.
//! - HTTP Datagrams (RFC 9297) ride the connection's QUIC datagrams on a
//!   client made `with_datagrams`, each for the request stream it names
//!   (`RequestStream::datagrams`).
```

`crates/rurge-net/src/quic/h3/mod.rs`——把

```rust
//! - An error of the connection closes it with its RFC 9114 or RFC 9204
//!   code; an error of a response ends its stream both ways.
```

换成

```rust
//! - An error of the connection closes it with its RFC 9114, RFC 9204 or
//!   RFC 9297 code; an error of a response ends its stream both ways.
```

`crates/rurge-net/src/quic/h3/mod.rs`——把

```rust
//! the same connection (Hysteria 2's TCP requests).
//!
//! Extended CONNECT (`:protocol`) and HTTP Datagrams come in M7c: the
//! server's SETTINGS are kept for them (`Settings::get`).

mod client;
```

换成

```rust
//! the same connection (Hysteria 2's TCP requests) and, on a client made
//! without datagrams, read the connection's QUIC datagrams itself
//! (Hysteria 2's UDP).

mod client;
mod datagram;
```

`crates/rurge-net/src/quic/h3/mod.rs`——把

```rust
pub use client::Client;
```

换成

```rust
pub use client::{Client, Protocol};
pub use datagram::Datagrams;
```

`crates/rurge-net/src/quic/h3/mod.rs`——把

```rust
// error codes (RFC 9114 8.1, RFC 9204 6)
```

换成

```rust
// error codes (RFC 9114 8.1, RFC 9204 6, RFC 9297 5.2)
pub(crate) const H3_DATAGRAM_ERROR: VarInt = VarInt::from_u32(0x33);
```

`crates/rurge-net/src/quic/h3/mod.rs`——把

```rust
/// The name of an HTTP/3 or QPACK error code, as error texts say it; an
/// unknown code in hex.
pub(crate) fn code_name(code: VarInt) -> Cow<'static, str> {
    Cow::Borrowed(match u64::from(code) {
```

换成

```rust
/// The name of an HTTP/3, QPACK or HTTP Datagrams error code, as error
/// texts say it; an unknown code in hex.
pub(crate) fn code_name(code: VarInt) -> Cow<'static, str> {
    Cow::Borrowed(match u64::from(code) {
        0x33 => "H3_DATAGRAM_ERROR",
```

`crates/rurge-net/src/quic/h3/client.rs`——把

```rust
use super::frame::{self, FrameReader, Header, ReadFailure, Settings};
use super::stream::RequestStream;
```

换成

```rust
use super::datagram::{self, Flows};
use super::frame::{self, FrameReader, Header, ReadFailure, Settings};
use super::stream::{Kind, RequestStream};
```

`crates/rurge-net/src/quic/h3/client.rs`——把

```rust
    SETTINGS_MAX_FIELD_SECTION_SIZE, SETTINGS_QPACK_BLOCKED_STREAMS,
    SETTINGS_QPACK_MAX_TABLE_CAPACITY, code_name, qpack,
```

换成

```rust
    SETTINGS_ENABLE_CONNECT_PROTOCOL, SETTINGS_H3_DATAGRAM, SETTINGS_MAX_FIELD_SECTION_SIZE,
    SETTINGS_QPACK_BLOCKED_STREAMS, SETTINGS_QPACK_MAX_TABLE_CAPACITY, code_name, qpack,
```

`crates/rurge-net/src/quic/h3/client.rs`——把

```rust
/// `MAX_FIELD_SECTION_SIZE`. HTTP Datagrams (M7c) add `H3_DATAGRAM`.
```

换成

```rust
/// `MAX_FIELD_SECTION_SIZE`. A client made `with_datagrams` adds
/// `H3_DATAGRAM = 1`.
```

`crates/rurge-net/src/quic/h3/client.rs`——把

```rust
/// HTTP/3 on one QUIC connection, as its client. A background task reads
/// the server's unidirectional streams and holds our control stream open;
/// it ends with the client. No `Debug`: nothing in it is worth printing.
```

换成

```rust
/// The protocol of an extended CONNECT (RFC 9220, RFC 8441 4), carried as
/// a request extension: a CONNECT with one says `:protocol`, `:scheme` and
/// `:path` too, and goes only once the server's SETTINGS allowed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Protocol(&'static str);

impl Protocol {
    /// Proxying UDP in HTTP (RFC 9298).
    pub const CONNECT_UDP: Protocol = Protocol("connect-udp");
}

/// HTTP/3 on one QUIC connection, as its client. A background task reads
/// the server's unidirectional streams — and, with datagrams, the
/// connection's QUIC datagrams — and holds our control stream open; it
/// ends with the client. No `Debug`: nothing in it is worth printing.
```

`crates/rurge-net/src/quic/h3/client.rs`——把

```rust
    quic: Connection,
```

换成

```rust
    pub(super) quic: Connection,
```

`crates/rurge-net/src/quic/h3/client.rs`——把

```rust
    critical: AtomicU8,
```

换成

```rust
    critical: AtomicU8,
    /// The request streams' HTTP/3 datagrams, on a client made
    /// `with_datagrams`.
    pub(super) flows: Option<Flows>,
```

`crates/rurge-net/src/quic/h3/client.rs`——把

```rust
    pub fn new(connection: Connection) -> Client {
```

换成

```rust
    ///
    /// The client neither announces nor reads HTTP Datagrams: the
    /// connection's QUIC datagrams are left to the protocol (Hysteria 2's
    /// UDP, which a server taking them for HTTP/3's would break).
    pub fn new(connection: Connection) -> Client {
        Client::start(connection, None)
    }

    /// HTTP/3 on `connection` with HTTP Datagrams (RFC 9297), as `new`
    /// otherwise: our SETTINGS also say `H3_DATAGRAM = 1`, and the client
    /// reads the connection's QUIC datagrams — all of them: nothing else
    /// may — each for the request stream it names
    /// (`RequestStream::datagrams`).
    pub fn with_datagrams(connection: Connection) -> Client {
        Client::start(connection, Some(Flows::default()))
    }

    fn start(connection: Connection, flows: Option<Flows>) -> Client {
```

`crates/rurge-net/src/quic/h3/client.rs`——把

```rust
            critical: AtomicU8::new(0),
        });
```

换成

```rust
            critical: AtomicU8::new(0),
            flows,
        });
```

`crates/rurge-net/src/quic/h3/client.rs`——把

```rust
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
```

换成

```rust
        self.inner.settings().await
```

`crates/rurge-net/src/quic/h3/client.rs`——把

```rust
    /// alone for a CONNECT (RFC 9114 4.4), else `:scheme` (`https` unless
    /// the URI says), `:authority` and `:path` — then its header fields, a
    /// sensitive value never to be indexed. The request's content, if any,
    /// is written to the stream and ended by its shutdown; the response
    /// comes back on it. Refused once the server sent GOAWAY.
    pub async fn request(&self, request: Request<()>) -> io::Result<RequestStream> {
```

换成

```rust
    /// alone for a CONNECT (RFC 9114 4.4); `:protocol`, `:scheme` and
    /// `:path` too for an extended CONNECT, one carrying a `Protocol` (RFC
    /// 8441 4); else `:scheme`, `:authority` and `:path` — then its header
    /// fields, a sensitive value never to be indexed. `:scheme` is `https`
    /// and `:path` `/` unless the URI says; the authority goes without
    /// userinfo (RFC 9114 4.3.1). The request's content, if any, is written
    /// to the stream and ended by its shutdown; the response comes back on
    /// it.
    ///
    /// An extended CONNECT first waits for the server's SETTINGS (the
    /// caller bounds the wait) and is refused, nothing sent, unless they
    /// enable it (RFC 8441 3, RFC 9220 3). Refused once the server sent
    /// GOAWAY.
    pub async fn request(&self, request: Request<()>) -> io::Result<RequestStream> {
        let head = frame::frame(frame::HEADERS, &head(&request)?);
        if request.extensions().get::<Protocol>().is_some() {
            let settings = self.inner.settings().await?;
            if settings.get(SETTINGS_ENABLE_CONNECT_PROTOCOL) != Some(1) {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "h3: the server does not support extended CONNECT",
                ));
            }
        }
```

`crates/rurge-net/src/quic/h3/client.rs`——把

```rust
                ));
            }
        }
        let head = frame::frame(frame::HEADERS, &head(&request)?);
```

换成

```rust
                ));
            }
        }
```

`crates/rurge-net/src/quic/h3/client.rs`——把

```rust
        let mut stream = RequestStream::new(self.inner.clone(), send, recv, Bytes::from(head));
```

换成

```rust
        let kind = Kind::of(request.method());
        let mut stream =
            RequestStream::new(self.inner.clone(), send, recv, Bytes::from(head), kind);
```

`crates/rurge-net/src/quic/h3/client.rs`——把

```rust
/// The field section of `request`'s head.
```

换成

```rust
/// The field section of `request`'s head, as `Client::request` says.
```

`crates/rurge-net/src/quic/h3/client.rs`——把

```rust
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
```

换成

```rust
    // what follows the last `@`, as `Authority::host` finds the host
    let authority = match authority.as_str().rsplit_once('@') {
        Some((_userinfo, host)) => host,
        None => authority.as_str(),
    }
    .as_bytes();
    let scheme = uri.scheme_str().unwrap_or("https").as_bytes();
    let path = uri
        .path_and_query()
        .map_or("/", |path| path.as_str())
        .as_bytes();
    let protocol = request.extensions().get::<Protocol>();
    let mut fields = vec![(&b":method"[..], request.method().as_str().as_bytes(), false)];
    match (request.method() == Method::CONNECT, protocol) {
        (true, None) => fields.push((&b":authority"[..], authority, false)),
        (true, Some(protocol)) => fields.extend([
            (&b":protocol"[..], protocol.0.as_bytes(), false),
            (&b":scheme"[..], scheme, false),
            (&b":authority"[..], authority, false),
            (&b":path"[..], path, false),
        ]),
        (false, None) => fields.extend([
            (&b":scheme"[..], scheme, false),
            (&b":authority"[..], authority, false),
            (&b":path"[..], path, false),
        ]),
        (false, Some(_)) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "h3: a request with a protocol must be a CONNECT",
            ));
        }
```

`crates/rurge-net/src/quic/h3/client.rs`——把

```rust
impl Inner {
```

换成

```rust
impl Inner {
    /// The server's SETTINGS, once they came; an error when the connection
    /// ended before. The caller bounds the wait.
    pub(super) async fn settings(&self) -> io::Result<Settings> {
        let mut state = self.state.subscribe();
        let state = state
            .wait_for(|s| s.settings.is_some() || s.failure.is_some() || s.ended.is_some())
            .await
            .expect("`self` holds the sender");
        match (&state.settings, &state.failure, &state.ended) {
            (Some(settings), _, _) => Ok(settings.clone()),
            (None, Some(failure), _) => Err(failure.io()),
            (None, None, Some(ended)) => Err(connection_error(ended)),
            (None, None, None) => unreachable!("waited for one of them"),
        }
    }

```

`crates/rurge-net/src/quic/h3/client.rs`——把

```rust
            WriteError::ClosedStream | WriteError::ZeroRttRejected => closed_stream(),
        }
    }

    /// Notes that the server opened its critical stream of `kind` (`bit`);
```

换成

```rust
            WriteError::ClosedStream | WriteError::ZeroRttRejected => closed_stream(),
        }
    }

    /// Why a stream's datagrams no longer flow, as an I/O error: the
    /// connection's end, when it ended; else the stream's.
    pub(super) fn flow_ended(&self) -> io::Error {
        match self.quic.close_reason() {
            Some(e) => self.lost(&e),
            None => closed_stream(),
        }
    }

    /// Notes that the server opened its critical stream of `kind` (`bit`);
```

`crates/rurge-net/src/quic/h3/client.rs`——把

```rust
/// Holds our control stream and reads the server's unidirectional streams,
/// each in a task of its own, until the connection ends — or until the
/// client aborts it, and its tasks with it.
```

换成

```rust
/// Holds our control stream and reads the server's unidirectional streams
/// — and, with datagrams, the connection's QUIC datagrams — each in a task
/// of its own, until the connection ends — or until the client aborts it,
/// and its tasks with it.
```

`crates/rurge-net/src/quic/h3/client.rs`——把

```rust
    tasks.spawn(control(inner.clone()));
```

换成

```rust
    tasks.spawn(control(inner.clone()));
    if inner.flows.is_some() {
        tasks.spawn(datagram::read(inner.clone()));
    }
```

`crates/rurge-net/src/quic/h3/client.rs`——把

```rust
    let mut payload = Vec::new();
    for (id, value) in OUR_SETTINGS {
```

换成

```rust
    let datagrams = inner.flows.is_some().then_some((SETTINGS_H3_DATAGRAM, 1));
    let mut payload = Vec::new();
    for (id, value) in OUR_SETTINGS.into_iter().chain(datagrams) {
```

`crates/rurge-net/src/quic/h3/frame.rs`——把

```rust
use super::{H3_FRAME_ERROR, H3_SETTINGS_ERROR, H3Error};
```

换成

```rust
use super::{
    H3_FRAME_ERROR, H3_SETTINGS_ERROR, H3Error, SETTINGS_ENABLE_CONNECT_PROTOCOL,
    SETTINGS_H3_DATAGRAM,
};
```

`crates/rurge-net/src/quic/h3/frame.rs`——把

```rust
    /// value. A pair cut short is H3_FRAME_ERROR; an identifier twice, or
    /// one of HTTP/2's that HTTP/3 reserved (0x02 to 0x05), is
    /// H3_SETTINGS_ERROR (RFC 9114 7.2.4, 7.2.4.1).
```

换成

```rust
    /// value. A pair cut short is H3_FRAME_ERROR; an identifier twice, one
    /// of HTTP/2's that HTTP/3 reserved (0x02 to 0x05), or extended
    /// CONNECT's or HTTP Datagrams' with a value but 0 or 1, is
    /// H3_SETTINGS_ERROR (RFC 9114 7.2.4, 7.2.4.1; RFC 8441 3, RFC 9297
    /// 2.1.1).
```

`crates/rurge-net/src/quic/h3/frame.rs`——把

```rust
                    format!("the server sent the HTTP/2 setting {id:#x}"),
```

换成

```rust
                    format!("the server sent the HTTP/2 setting {id:#x}"),
                ));
            }
            if matches!(id, SETTINGS_ENABLE_CONNECT_PROTOCOL | SETTINGS_H3_DATAGRAM) && value > 1 {
                return Err(H3Error::new(
                    H3_SETTINGS_ERROR,
                    format!("the server sent the setting {id:#x} as {value}, neither 0 nor 1"),
```

`crates/rurge-net/src/quic/h3/stream.rs`——把

```rust
//!   the trailers are passed over.
```

换成

```rust
//!   the trailers are passed over. The content must be as long as the
//!   response's `content-length` says (RFC 9114 4.1.2), unless the response
//!   has none (to HEAD, 204, 304) or makes a tunnel: a 2xx to a CONNECT,
//!   after which only DATA may come (RFC 9114 4.4).
```

`crates/rurge-net/src/quic/h3/stream.rs`——把

```rust
//!   H3_REQUEST_CANCELLED (RFC 9114 4.1.1).

use super::client::Inner;
```

换成

```rust
//!   H3_REQUEST_CANCELLED (RFC 9114 4.1.1).
//! - Its HTTP/3 datagrams (`datagrams`) flow until either direction ends.

use super::client::Inner;
use super::datagram::Datagrams;
```

`crates/rurge-net/src/quic/h3/stream.rs`——把

```rust
};
use bytes::{Buf, Bytes, BytesMut};
use http::header::{HeaderName, HeaderValue};
use http::{Response, StatusCode};
use quinn::{RecvStream, SendStream, VarInt};
```

换成

```rust
    SETTINGS_H3_DATAGRAM,
};
use bytes::{Buf, Bytes, BytesMut};
use http::header::{CONTENT_LENGTH, HeaderName, HeaderValue};
use http::{HeaderMap, Method, Response, StatusCode};
use quinn::{RecvStream, SendStream, VarInt, WriteError};
```

`crates/rurge-net/src/quic/h3/stream.rs`——把

```rust
    recv: Recv,
```

换成

```rust
    recv: Recv,
    /// What the request was, which reading the response depends on.
    kind: Kind,
    /// The response made the stream a tunnel: a 2xx to a CONNECT.
    tunnel: bool,
    /// The content still to come, as the response's `content-length` said;
    /// `None` when nothing is counted.
    content_left: Option<u64>,
    /// Its datagrams were taken (`datagrams`).
    datagrams_taken: bool,
}

/// What a request is, as far as reading its response goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// A CONNECT, classic or extended: a 2xx makes the stream a tunnel.
    Connect,
    /// A HEAD: its response has no content (RFC 9110 9.3.2).
    Head,
    /// Any other request.
    Other,
}

impl Kind {
    pub(super) fn of(method: &Method) -> Kind {
        if *method == Method::CONNECT {
            Kind::Connect
        } else if *method == Method::HEAD {
            Kind::Head
        } else {
            Kind::Other
        }
    }
```

`crates/rurge-net/src/quic/h3/stream.rs`——把

```rust
    /// A request whose head — a whole HEADERS frame — goes out first.
```

换成

```rust
    /// A request of `kind` whose head — a whole HEADERS frame — goes out
    /// first.
```

`crates/rurge-net/src/quic/h3/stream.rs`——把

```rust
        head: Bytes,
```

换成

```rust
        head: Bytes,
        kind: Kind,
```

`crates/rurge-net/src/quic/h3/stream.rs`——把

```rust
            recv: Recv::Head,
```

换成

```rust
            recv: Recv::Head,
            kind,
            tunnel: false,
            content_left: None,
            datagrams_taken: false,
        }
    }

    /// The stream's QUIC stream ID: 0, 4, 8 … for the client's requests
    /// (RFC 9000 2.1).
    pub fn id(&self) -> u64 {
        u64::from(self.send.id())
    }

    /// The stream's HTTP/3 datagrams (RFC 9297 2.1), taken once per stream
    /// of a client made `with_datagrams`. They need the server's SETTINGS
    /// to say `H3_DATAGRAM = 1` — they are waited for; the caller bounds
    /// the wait — and the connection to have negotiated QUIC datagrams.
    /// They flow until either direction of the stream ends, the stream or
    /// the handle is dropped, or the connection ends.
    pub async fn datagrams(&mut self) -> io::Result<Datagrams> {
        if self.inner.flows.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "h3: the client was made without HTTP datagrams",
            ));
        }
        if self.datagrams_taken {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "h3: the stream's datagrams were taken already",
            ));
        }
        if self.finished || matches!(self.recv, Recv::End | Recv::Failed(..)) {
            return Err(self.inner.flow_ended());
        }
        let settings = self.inner.settings().await?;
        if settings.get(SETTINGS_H3_DATAGRAM) != Some(1)
            || self.inner.quic.max_datagram_size().is_none()
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "h3: the server does not support HTTP datagrams",
            ));
        }
        let quarter = self.id() / 4;
        let Some(incoming) = self
            .inner
            .flows
            .as_ref()
            .and_then(|flows| flows.open(quarter))
        else {
            return Err(self.inner.flow_ended());
        };
        self.datagrams_taken = true;
        Ok(Datagrams::new(self.inner.clone(), quarter, incoming))
    }

    /// Either direction of the stream ended: its datagrams flow no more
    /// (RFC 9297 2.1).
    fn end_datagrams(&mut self) {
        if self.datagrams_taken
            && let Some(flows) = &self.inner.flows
        {
            flows.close(self.id() / 4);
```

`crates/rurge-net/src/quic/h3/stream.rs`——把

```rust
    /// types are passed over. A head that is malformed or larger than
    /// 64 KiB ends the stream (H3_MESSAGE_ERROR, H3_EXCESSIVE_LOAD); a
    /// field section that cannot be decoded, or a frame where none may be,
    /// closes the connection.
```

换成

```rust
    /// types are passed over. A head that is malformed — a
    /// `content-length` that is no number counted — or larger than 64 KiB
    /// ends the stream (H3_MESSAGE_ERROR, H3_EXCESSIVE_LOAD); a field
    /// section that cannot be decoded, or a frame where none may be, closes
    /// the connection.
```

`crates/rurge-net/src/quic/h3/stream.rs`——把

```rust
                        None => {
                            return Poll::Ready(Err(self.stream_error(
                                H3_MESSAGE_ERROR,
                                "h3: the server sent a malformed response".to_string(),
                            )));
                        }
                        Some(head) if head.status().is_informational() => self.recv = Recv::Head,
                        Some(head) => {
```

换成

```rust
                        None => return Poll::Ready(Err(self.malformed(None))),
                        Some(head) if head.status().is_informational() => self.recv = Recv::Head,
                        Some(head) => {
                            self.tunnel = self.kind == Kind::Connect && head.status().is_success();
                            if self.counts_content(head.status()) {
                                match content_length(head.headers()) {
                                    Ok(length) => self.content_left = length,
                                    Err(()) => return Poll::Ready(Err(self.malformed(None))),
                                }
                            }
```

`crates/rurge-net/src/quic/h3/stream.rs`——把

```rust
            format!("h3: the response head is larger than {MAX_FIELD_SECTION_SIZE} bytes"),
        )
    }

    /// An error of the connection (RFC 9114 8): it closes with `error`,
```

换成

```rust
            format!("h3: the response head is larger than {MAX_FIELD_SECTION_SIZE} bytes"),
        )
    }

    /// A malformed response ends the stream (H3_MESSAGE_ERROR, RFC 9114
    /// 4.1.2); `detail` says how, when the head does not.
    fn malformed(&mut self, detail: Option<&str>) -> io::Error {
        let text = match detail {
            Some(detail) => format!("h3: the server sent a malformed response ({detail})"),
            None => "h3: the server sent a malformed response".to_string(),
        };
        self.stream_error(H3_MESSAGE_ERROR, text)
    }

    /// Whether the response's content is counted against its
    /// `content-length` (RFC 9114 4.1.2): not a tunnel's, whose
    /// `content-length` is ignored (RFC 9110 9.3.6), nor that of a response
    /// without content — to HEAD, 204, 304 (RFC 9110 6.4.1).
    fn counts_content(&self, status: StatusCode) -> bool {
        !self.tunnel
            && self.kind != Kind::Head
            && status != StatusCode::NO_CONTENT
            && status != StatusCode::NOT_MODIFIED
    }

    /// An error of the connection (RFC 9114 8): it closes with `error`,
```

`crates/rurge-net/src/quic/h3/stream.rs`——把

```rust
        error
```

换成

```rust
        self.end_datagrams();
        error
    }

    /// The server's side ended after its content.
    fn end(&mut self) {
        self.recv = Recv::End;
        self.end_datagrams();
    }

    /// Writing failed: the sending part is gone.
    fn write_failed(&mut self, e: WriteError) -> io::Error {
        self.end_datagrams();
        self.inner.write_error(e)
```

`crates/rurge-net/src/quic/h3/stream.rs`——把

```rust
                Err(e) => return Poll::Ready(Err(self.inner.write_error(e))),
```

换成

```rust
                Err(e) => return Poll::Ready(Err(self.write_failed(e))),
```

`crates/rurge-net/src/quic/h3/stream.rs`——把

```rust
            }
        }
        Poll::Ready(Ok(()))
    }
}

/// The response `fields` make; `None` for a malformed one (RFC 9114 4.1.2):
```

换成

```rust
            }
        }
        Poll::Ready(Ok(()))
    }
}

/// The response's `content-length` (RFC 9110 8.6): `None` without one; an
/// error for a value that is no number, or values that differ.
fn content_length(headers: &HeaderMap) -> Result<Option<u64>, ()> {
    let mut length = None;
    for value in headers.get_all(CONTENT_LENGTH) {
        let digits = value.as_bytes();
        // `u64::from_str` would take a sign too
        if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
            return Err(());
        }
        let value = std::str::from_utf8(digits)
            .ok()
            .and_then(|digits| digits.parse().ok())
            .ok_or(())?;
        if length.is_some_and(|length| length != value) {
            return Err(());
        }
        length = Some(value);
    }
    Ok(length)
}

/// The response `fields` make; `None` for a malformed one (RFC 9114 4.1.2):
```

`crates/rurge-net/src/quic/h3/stream.rs`——把

```rust
                            this.recv = Recv::End;
```

换成

```rust
                            if this.content_left.is_some_and(|left| left > 0) {
                                return Poll::Ready(Err(
                                    this.malformed(Some("shorter than its content-length"))
                                ));
                            }
                            this.end();
```

`crates/rurge-net/src/quic/h3/stream.rs`——把

```rust
                        frame::DATA if header.len > 0 => this.recv = Recv::Data(header.len),
                        frame::DATA => {}
                        // the trailers: ignored
                        frame::HEADERS => {
```

换成

```rust
                        // nothing but DATA follows a tunnel's head (RFC 9114 4.4)
                        frame::HEADERS if this.tunnel => {
                            return Poll::Ready(Err(this.violation(H3Error::new(
                                H3_FRAME_UNEXPECTED,
                                "the server sent HEADERS on a CONNECT tunnel",
                            ))));
                        }
                        frame::DATA => {
                            match this.content_left {
                                Some(left) if header.len > left => {
                                    return Poll::Ready(Err(
                                        this.malformed(Some("longer than its content-length"))
                                    ));
                                }
                                Some(left) => this.content_left = Some(left - header.len),
                                None => {}
                            }
                            if header.len > 0 {
                                this.recv = Recv::Data(header.len);
                            }
                        }
                        // the trailers, which end the content: ignored
                        frame::HEADERS => {
                            if this.content_left.is_some_and(|left| left > 0) {
                                return Poll::Ready(Err(
                                    this.malformed(Some("shorter than its content-length"))
                                ));
                            }
```

`crates/rurge-net/src/quic/h3/stream.rs`——把

```rust
        }
    }
}

impl AsyncWrite for RequestStream {
```

换成

```rust
        }
    }
}

/// The request's content, or a tunnel's bytes, as DATA frames. A write
/// returns once the stream took the start of its frame: the rest of that
/// frame moves on only when the stream is written, flushed or shut down
/// again — a caller that writes and then waits for the server must flush
/// first.
impl AsyncWrite for RequestStream {
```

`crates/rurge-net/src/quic/h3/stream.rs`——把

```rust
            Err(e) => return Poll::Ready(Err(this.inner.write_error(e))),
```

换成

```rust
            Err(e) => return Poll::Ready(Err(this.write_failed(e))),
```

`crates/rurge-net/src/quic/h3/stream.rs`——把

```rust
            Poll::Ready(Err(e)) => return Poll::Ready(Err(this.inner.write_error(e))),
```

换成

```rust
            Poll::Ready(Err(e)) => return Poll::Ready(Err(this.write_failed(e))),
```

`crates/rurge-net/src/quic/h3/stream.rs`——把

```rust
            this.finished = true;
```

换成

```rust
            this.finished = true;
            this.end_datagrams();
```

`crates/rurge-net/src/quic/h3/stream.rs`——把

```rust
            self.reader.stop(H3_REQUEST_CANCELLED);
        }
    }
```

换成

```rust
            self.reader.stop(H3_REQUEST_CANCELLED);
        }
        self.end_datagrams();
    }
```

要点：
- `Client::new` 不宣告 `H3_DATAGRAM`、不读数据报（Hysteria 2）；`with_datagrams` 宣告并分发。
- 扩展 CONNECT 在开流之前等 SETTINGS，服务端不允许时什么也不发。
- 一条流的数据报在它任一方向结束时随之结束：用它的人要一直读这条流、在转发期间不 `shutdown`。
- 隧道（CONNECT 的 2xx）忽略 `content-length`，不收第二个 HEADERS；其余应答按 `content-length` 核对。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-net quic::h3` → 通过（新增 25 条：扩展 CONNECT 的伪首部、SETTINGS 不允许时拒绝与等 SETTINGS、只有 `with_datagrams` 宣告数据报、按流分发的往返、没登记的流的数据报被丢而连接照旧、解析不出流号即关连接、超长被拒且什么也不发、服务端不支持时不可用、队列满即丢、随流结束、0x08 / 0x33 的取值、`content-length` 的多与少与恰好、隧道忽略它、隧道上的第二个 HEADERS 关连接、被拒的 CONNECT 是普通应答、`:authority` 不带 userinfo，以及 `datagram.rs` / `frame.rs` / `stream.rs` / `client.rs` 的单元用例）。
Run: `cargo test -p rurge-proto-quic hysteria2` → 通过（Hysteria 2 不受影响）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-net
git commit -m "feat(net): HTTP/3 的扩展 CONNECT 与 HTTP 数据报；SETTINGS 取值校验、content-length 核对、隧道上不收第二个 HEADERS"
```

### Task 3: MASQUE 的 TCP 与 `FakeH3Proxy`

`MasqueOutbound` 的 TCP：每个连接一个 HTTP/3 经典 CONNECT，有凭据时每个请求都带 Basic（P8）；会话协议在握手时建带数据报的 HTTP/3 客户端（Task 4 要用）。`FakeH3Proxy` 是 MASQUE 与 `trust-tunnel` 的回环假服务端，照 masque-server 的做法（quiche 式的 GREASE、407 带 `proxy-authenticate`、SETTINGS 的开关），自己实现服务端一侧。`basic_authorization` 移到 `rurge_proto::http` 供三方共用（P10）。新模块的用例都在新文件里，与实现一同写出；移动的那一条用例有单独的失败步骤。

**Files:**
- Create: `crates/rurge-proto-quic/src/masque/{mod.rs, tcp.rs}`（自带用例）、`src/testing/h3proxy.rs`
- Modify: `crates/rurge-proto-quic/src/{lib.rs, testing/mod.rs}`、`crates/rurge-proto/src/{http.rs, h2connect/mod.rs, trust_tunnel.rs}`（`http.rs` 与 `h2connect/mod.rs` 含用例）

**Interfaces:**
- Consumes: Task 1 的 `MasqueSpec`；Task 2 的 `Client::with_datagrams`、`RequestStream`；M7a 的 `Dialer` / `Session` / `Protocol` / `Failure` / `ServerPort` / `MultiPort`；`rurge_proto::http::wire_host`。
- Produces: `rurge_proto::http::basic_authorization(user: &str, password: &str) -> String`；`rurge_proto_quic::masque::MasqueOutbound::new(name: &str, server: Target, spec: &MasqueSpec, ecn: Tristate, keystore: &[KeystoreItem], roots: Arc<RootCertStore>, connector: Arc<dyn Connector>) -> Result<MasqueOutbound, BuildError>`（实现 `Outbound` 的 TCP）；测试设施 `FakeH3Proxy`（`spawn` / `spawn_on` / `addr` / `roots` / `requests` / `connections` / `connection` / `close_connections` / `restart` 等）、`H3ProxyScript`、`RecordedH3Request`

- [ ] **Step 1: 先写移动的用例**

`crates/rurge-proto/src/http.rs`——把

```rust
        assert_eq!(m, "http proxy answered 403 Forbidden");
    }
}
```

换成

```rust
        assert_eq!(m, "http proxy answered 403 Forbidden");
    }

    #[test]
    fn basic_authorization_is_base64_of_user_colon_password() {
        // RFC 7617 2
        assert_eq!(
            basic_authorization("Aladdin", "open sesame"),
            "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
    }
}
```

`crates/rurge-proto/src/h2connect/mod.rs`——把

```rust
        assert!(!headers["user-agent"].is_sensitive());
    }

    #[test]
    fn basic_authorization_is_base64_of_user_colon_password() {
        // RFC 7617 2
        assert_eq!(
            basic_authorization("Aladdin", "open sesame"),
            "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
    }

    /// A fake that serves CONNECT-UDP.
```

换成

```rust
        assert!(!headers["user-agent"].is_sensitive());
    }

    /// A fake that serves CONNECT-UDP.
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto --lib http::tests`
Expected: FAIL——`basic_authorization` 还在 `h2connect` 里：

```text
error[E0425]: cannot find function `basic_authorization` in this scope
   --> crates\rurge-proto\src\http.rs:850:13
error: could not compile `rurge-proto` (lib test) due to 1 previous error
```

- [ ] **Step 3: 实现**

`basic_authorization` 移到 `rurge_proto::http`：

`crates/rurge-proto/src/http.rs`——把

```rust
        base.push((name, value));
    }
}
```

换成

```rust
        base.push((name, value));
    }
}

/// `Basic base64(user:password)` (RFC 9110 11.7.2, RFC 7617): the
/// `proxy-authorization` value of the CONNECT proxies over HTTP/2 and
/// HTTP/3 (`h2-connect`, `trust-tunnel`, `masque`), standard Base64 with
/// its padding.
pub fn basic_authorization(user: &str, password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{password}")))
}
```

`crates/rurge-proto/src/h2connect/mod.rs`——把

```rust
use crate::http::{merge, render, wire_host};
use crate::transport::Stack;
use crate::{BuildError, Outbound, OutboundError, UdpSupport};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
```

换成

```rust
use crate::http::{basic_authorization, merge, render, wire_host};
use crate::transport::Stack;
use crate::{BuildError, Outbound, OutboundError, UdpSupport};
```

`crates/rurge-proto/src/h2connect/mod.rs`——把

```rust
    udp_authority: Option<String>,
}

/// `Basic base64(user:password)` (RFC 9110 11.7.2, RFC 7617).
pub(crate) fn basic_authorization(user: &str, password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{password}")))
```

换成

```rust
    udp_authority: Option<String>,
```

`crates/rurge-proto/src/trust_tunnel.rs`——把

```rust
use crate::h2connect::{basic_authorization, connect_request};
use crate::h2pool::{H2Pool, StackDial};
```

换成

```rust
use crate::h2connect::connect_request;
use crate::h2pool::{H2Pool, StackDial};
use crate::http::basic_authorization;
```

MASQUE 的出站与假服务端：

`crates/rurge-proto-quic/src/lib.rs`——把

```rust
//! (`hysteria2`) and TUIC's (`tuic`).

pub mod hysteria2;
```

换成

```rust
//! (`hysteria2`), TUIC's (`tuic`) and MASQUE's (`masque`).

pub mod hysteria2;
pub mod masque;
```

`crates/rurge-proto-quic/src/lib.rs`——把

```rust
pub use hysteria2::Hysteria2Outbound;
```

换成

```rust
pub use hysteria2::Hysteria2Outbound;
pub use masque::MasqueOutbound;
```

新建 `crates/rurge-proto-quic/src/masque/mod.rs`：

```rust
//! `masque` outbound (manual: Policies › MASQUE; phase 2 M7 design 7.1):
//! the policy's QUIC connection, which its requests share (`session`), and
//! HTTP/3 on it, HTTP Datagrams announced: RFC 9297 recommends announcing
//! them always (2.1.1), and CONNECT-UDP rides them.
//!
//! - A TCP connection is a classic HTTP/3 CONNECT of its own (`tcp`), as
//!   Surge relays TCP ("HTTP/3 CONNECT tunnels multiplexed over a single
//!   QUIC connection"): the template-driven `connect-tcp` is still a draft,
//!   and masque-server answers it 404.
//! - With `username` and `password`, every request carries them as
//!   `proxy-authorization: Basic …`, which QPACK never indexes. The manual
//!   names no field: this is the one a proxy reads them from (RFC 9110
//!   11.7.2), and masque-server's. Without them no authorization goes.
//!   Nothing is authenticated per connection.

mod tcp;

use crate::session::{Dialer, Protocol, Session};
use http::header::HeaderValue;
use quinn::{Connection, VarInt};
use rurge_config::KeystoreItem;
use rurge_config::spec::{MasqueSpec, Tristate};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_net::quic::h3;
use rurge_proto::http::basic_authorization;
use rurge_proto::{BuildError, Outbound, OutboundError};
use rustls::RootCertStore;
use std::sync::Arc;

/// H3_NO_ERROR (RFC 9114 8.1): a connection let go of.
const H3_NO_ERROR: VarInt = VarInt::from_u32(0x100);

/// No `Debug`: it holds the credentials.
pub struct MasqueOutbound {
    name: String,
    session: Session<Http3>,
    /// `proxy-authorization`, marked sensitive: QPACK never indexes it.
    authorization: Option<HeaderValue>,
}

impl MasqueOutbound {
    /// The connection goes as `connector` dials (design 3.3), with the
    /// policy's `ecn`; `client-cert` comes from `keystore`.
    pub fn new(
        name: &str,
        server: Target,
        spec: &MasqueSpec,
        ecn: Tristate,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<MasqueOutbound, BuildError> {
        // error texts carry no policy name: the registry's `build_one` and
        // the dry build both prefix it
        let dialer = Dialer::new(
            connector, server, &spec.tls, &spec.quic, ecn, keystore, roots,
        )?;
        let authorization = spec.credentials.as_ref().map(|credentials| {
            let basic =
                basic_authorization(credentials.username.expose(), credentials.password.expose());
            let mut value = HeaderValue::try_from(basic).expect("Base64 makes a field value");
            value.set_sensitive(true);
            value
        });
        Ok(MasqueOutbound {
            name: name.to_string(),
            session: Session::new(name, dialer, Http3),
            authorization,
        })
    }
}

/// What every new connection runs before its requests: HTTP/3, nothing
/// more — each request carries its own credentials.
struct Http3;

/// What a connection keeps for its requests.
struct Connected {
    /// Lives as long as the connection: dropped, it would end our HTTP/3
    /// control stream, which a server may take for the end of HTTP/3 on the
    /// connection.
    h3: h3::Client,
}

impl Protocol for Http3 {
    type State = Connected;

    const NAME: &'static str = "masque";

    const NO_ERROR: VarInt = H3_NO_ERROR;

    /// Our SETTINGS go out, and requests may follow at once: a classic
    /// CONNECT waits for nothing of the server's.
    fn handshake<'a>(
        &'a self,
        connection: &'a Connection,
    ) -> BoxFuture<'a, Result<Connected, OutboundError>> {
        Box::pin(async move {
            Ok(Connected {
                h3: h3::Client::with_datagrams(connection.clone()),
            })
        })
    }

    /// The server sent GOAWAY: it is going down.
    fn is_going_away(&self, state: &Connected) -> bool {
        state.h3.is_going_away()
    }
}

impl Outbound for MasqueOutbound {
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
            let request = tcp::request(target, self.authorization.as_ref())?;
            // one budget for the connection (when one is dialed), QUIC's
            // handshake and the proxy's response
            let tunnel = self
                .session
                .request(opts, |lease| Box::pin(tcp::open(lease, request.clone())));
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
    use crate::testing::{FakeH3Proxy, H3ProxyScript, RecordedH3Request};
    use quinn::ConnectionError;
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::masque::read_masque;
    use rurge_config::spec::{CommonOpts, ParamReader, PortHopping};
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_net::quic::testing::MultiPort;
    use rurge_proto::testing::echo_server;
    use std::io;
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::time::{Instant, timeout};

    const BOUND: Duration = Duration::from_secs(10);

    /// The user `u` with the password `p`.
    fn users() -> Vec<(String, String)> {
        vec![("u".to_string(), "p".to_string())]
    }

    /// The spec of `masque, 127.0.0.1, <port>[, <params>]`, as the
    /// configuration reads it.
    fn spec(port: u16, params: &str) -> MasqueSpec {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let mut line = format!("masque, 127.0.0.1, {port}");
        if !params.is_empty() {
            line = format!("{line}, {params}");
        }
        let policy = parse_policy("M", &line, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let spec = read_masque(&mut r, &mut CommonOpts::default(), &[]);
        assert!(!r.has_errors(), "{:?}", r.finish());
        spec
    }

    /// `spec`'s outbound to `127.0.0.1:<port>`, trusting `fake`.
    fn outbound_with(spec: &MasqueSpec, port: u16, fake: &FakeH3Proxy) -> MasqueOutbound {
        MasqueOutbound::new(
            "M",
            Target::new(HostName::parse("127.0.0.1"), port),
            spec,
            Tristate::Auto,
            &[],
            fake.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    /// `masque, 127.0.0.1, <the fake's port>[, <params>]`.
    fn outbound(fake: &FakeH3Proxy, params: &str) -> MasqueOutbound {
        let port = fake.addr().port();
        outbound_with(&spec(port, params), port, fake)
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    /// A classic CONNECT to `authority`, as the fake records it.
    fn connect(connection: usize, authority: &str) -> RecordedH3Request {
        RecordedH3Request {
            connection,
            method: "CONNECT".to_string(),
            protocol: None,
            scheme: None,
            authority: Some(authority.to_string()),
            path: None,
            headers: Vec::new(),
        }
    }

    async fn tunnel(out: &MasqueOutbound, to: &Target) -> BoxedStream {
        timeout(BOUND, out.connect_tcp(to, &ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .expect("a tunnel")
    }

    async fn failure(out: &MasqueOutbound, to: &Target) -> OutboundError {
        timeout(BOUND, out.connect_tcp(to, &ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .err()
            .expect("no tunnel")
    }

    /// Flushed: a write may leave the rest of its DATA frame for the next.
    async fn round_trip(stream: &mut BoxedStream, payload: &[u8]) {
        stream.write_all(payload).await.unwrap();
        stream.flush().await.unwrap();
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

    /// Let go of by the client: closed as its session closes a connection,
    /// with H3_NO_ERROR.
    async fn closed_by_the_client(connection: &Connection) {
        let closed = timeout(BOUND, connection.closed()).await.expect("closed");
        assert!(
            matches!(&closed, ConnectionError::ApplicationClosed(close)
                if close.error_code == H3_NO_ERROR),
            "{closed:?}"
        );
    }

    /// Design 7.1: a TCP connection is a classic CONNECT — `:method` and
    /// `:authority` alone — whose 2xx makes the tunnel.
    #[tokio::test]
    async fn a_tunnel_is_a_classic_connect() {
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        assert_eq!(out.name(), "M");
        assert!(out.http_forward().is_none());
        let mut stream = tunnel(&out, &target(echo)).await;
        round_trip(&mut stream, b"through masque").await;
        assert_eq!(fake.requests(), [connect(0, &echo.to_string())]);
    }

    /// Past both sides' stream windows (8 MiB ours, quinn's 1.25 MB the
    /// fake's); the shutdown reaches the target as a FIN, and the target's
    /// close comes back as the end of the stream.
    #[tokio::test]
    async fn a_tunnel_carries_a_large_payload_and_a_half_close() {
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
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

    /// Each side's end reaches the other on its own: a target that ends
    /// its side first is read to its end while the tunnel still sends, and
    /// the tunnel's shutdown then reaches the target.
    #[tokio::test]
    async fn the_target_may_end_its_side_first() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let target_side = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (mut read, mut write) = stream.into_split();
            write.write_all(b"hello, and goodbye").await.unwrap();
            write.shutdown().await.unwrap();
            let mut heard = Vec::new();
            read.read_to_end(&mut heard).await.unwrap();
            heard
        });
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        let mut stream = tunnel(&out, &target(addr)).await;
        let mut greeting = Vec::new();
        timeout(BOUND, stream.read_to_end(&mut greeting))
            .await
            .expect("the target's end")
            .unwrap();
        assert_eq!(greeting, b"hello, and goodbye");
        stream.write_all(b"still here").await.unwrap();
        stream.shutdown().await.unwrap();
        let heard = timeout(BOUND, target_side).await.expect("our end").unwrap();
        assert_eq!(heard, b"still here");
    }

    /// Design 7.1: the authority names the target as `host:port` — an IPv6
    /// literal in brackets, a name as its A-labels. A target that cannot
    /// be named is refused before anything is dialed.
    #[tokio::test]
    async fn targets_go_out_as_host_and_port_and_an_unsendable_name_never_dials() {
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            connect_to: Some(echo),
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "");
        for (host, authority) in [
            (HostName::parse("192.0.2.1"), "192.0.2.1:443"),
            (HostName::parse("2001:db8::1"), "[2001:db8::1]:443"),
            (
                HostName::Domain("bücher.example".into()),
                "xn--bcher-kva.example:443",
            ),
            (HostName::parse("remote.example"), "remote.example:443"),
        ] {
            let mut stream = tunnel(&out, &Target::new(host, 443)).await;
            round_trip(&mut stream, b"x").await;
            assert_eq!(fake.requests().pop(), Some(connect(0, authority)));
        }
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        // longer than an authority may be
        let long = format!("{}.test", "a".repeat(70_000));
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
                "masque: the host name cannot be sent to the server"
            );
        }
        assert_eq!(fake.connections(), 0);
    }

    /// With `username` and `password`, every CONNECT carries them as Basic
    /// `proxy-authorization` — not only after a 407 —; a policy without
    /// them sends none, which a server that wants them refuses.
    #[tokio::test]
    async fn every_connect_carries_the_credentials_and_none_goes_without_them() {
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            users: users(),
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "username=u, password=p");
        for payload in [b"one", b"two"] {
            let mut stream = tunnel(&out, &target(echo)).await;
            round_trip(&mut stream, payload).await;
        }
        let requests = fake.requests();
        assert_eq!(requests.len(), 2);
        for request in &requests {
            // base64("u:p"), and nothing else
            assert_eq!(
                request.headers,
                [("proxy-authorization".to_string(), "Basic dTpw".to_string())]
            );
        }
        let without = outbound(&fake, "");
        let err = failure(&without, &target(echo)).await;
        assert_eq!(err.to_string(), "masque: proxy authentication required");
        assert!(fake.requests()[2].headers.is_empty());

        // a server that wants none lets it through
        let open = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&open, "");
        let mut stream = tunnel(&out, &target(echo)).await;
        round_trip(&mut stream, b"open").await;
        assert!(open.requests()[0].headers.is_empty());
    }

    /// Design 7.1: 407 is a refused login; any other final status is the
    /// proxy's refusal, quoted by its number — masque-server answers 502
    /// for a target it cannot reach. A refusal costs its connection
    /// nothing.
    #[tokio::test]
    async fn a_refusal_is_quoted_by_its_status() {
        // a port nothing listens on
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            users: users(),
            ..H3ProxyScript::default()
        });
        let wrong = outbound(&fake, "username=u, password=wrong");
        let right = outbound(&fake, "username=u, password=p");
        for (out, refusal) in [
            (&wrong, "masque: proxy authentication required"),
            (&right, "masque: the proxy answered 502"),
        ] {
            for _ in 0..2 {
                let err = failure(out, &target(closed)).await;
                assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
                assert_eq!(err.to_string(), refusal);
            }
        }
        assert_eq!(fake.connections(), 2, "one for each outbound");
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            refuse: Some(403),
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "");
        let err = failure(&out, &target(closed)).await;
        assert_eq!(err.to_string(), "masque: the proxy answered 403");
    }

    /// Design 3.5: one dial for the tunnels that come together, and the
    /// connection carries those after them too.
    #[tokio::test]
    async fn tunnels_that_come_together_share_one_connection() {
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        let to = target(echo);
        let (a, b, c) = tokio::join!(tunnel(&out, &to), tunnel(&out, &to), tunnel(&out, &to));
        for (mut stream, payload) in [(a, b"one"), (b, b"two"), (c, b"six")] {
            round_trip(&mut stream, payload).await;
        }
        let mut later = tunnel(&out, &to).await;
        round_trip(&mut later, b"later").await;
        assert_eq!(fake.connections(), 1);
        assert_eq!(fake.requests(), vec![connect(0, &echo.to_string()); 4]);
    }

    /// Design 3.5: a server going away (an HTTP/3 GOAWAY) takes no new
    /// tunnel: the next goes on a new connection, while one under way on
    /// the old runs to its end; the client closes the old connection once
    /// nothing holds it.
    #[tokio::test]
    async fn after_a_goaway_new_tunnels_go_on_a_new_connection_and_old_ones_finish() {
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            goaway_after: Some(1),
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "");
        // held from before the first request, to see its GOAWAY come
        let lease = timeout(BOUND, out.session.lease(&ConnectOpts::default()))
            .await
            .unwrap()
            .unwrap();
        let mut old = tunnel(&out, &target(echo)).await;
        round_trip(&mut old, b"before").await;
        let deadline = Instant::now() + BOUND;
        while out.session.serves(&lease) {
            assert!(Instant::now() < deadline, "no GOAWAY heard");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        drop(lease);
        let mut new = tunnel(&out, &target(echo)).await;
        round_trip(&mut new, b"after").await;
        round_trip(&mut old, b"still there").await;
        let carried: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
        assert_eq!(carried, [0, 1]);
        old.shutdown().await.unwrap();
        let mut rest = Vec::new();
        timeout(BOUND, old.read_to_end(&mut rest))
            .await
            .expect("the end of the tunnel")
            .unwrap();
        assert!(rest.is_empty());
        drop(old);
        closed_by_the_client(&fake.connection(0)).await;
    }

    /// A tunnel under way ends with its connection; the next one dials
    /// again.
    #[tokio::test]
    async fn after_the_server_closes_the_connection_the_next_tunnel_dials_again() {
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        let mut first = tunnel(&out, &target(echo)).await;
        round_trip(&mut first, b"first").await;
        fake.close_connections();
        assert_eq!(
            end_of(&mut first).await.to_string(),
            "quic: the server closed the connection (code 0x100)"
        );
        let mut second = tunnel(&out, &target(echo)).await;
        round_trip(&mut second, b"second").await;
        assert_eq!(fake.connections(), 2);
    }

    /// Design 3.5: the server restarts — losing its connections — as a
    /// request comes. The request finds its connection gone (a stateless
    /// reset answers the client's next packet) before the response came,
    /// and goes again, once, on a new connection. A restart between two
    /// requests is no harder.
    #[tokio::test]
    async fn a_request_that_finds_the_server_restarted_goes_again_on_a_new_connection() {
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
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
        let carried: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
        assert_eq!(carried, [0, 0, 1]);
        assert_eq!(
            end_of(&mut before).await.to_string(),
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
        assert_eq!(fake.requests().last().unwrap().connection, 2);
    }

    /// `opts.timeout` bounds the whole request: the dial, QUIC's handshake
    /// and the proxy's response.
    #[tokio::test]
    async fn a_server_that_never_answers_times_out() {
        let echo = echo_server().await;
        let opts = ConnectOpts {
            timeout: Duration::from_millis(500),
        };
        // nothing speaks QUIC on this port
        let quiet = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = quiet.local_addr().unwrap().port();
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            silent: true,
            ..H3ProxyScript::default()
        });
        let out = outbound_with(&spec(port, ""), port, &fake);
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
        // a server that takes the request and never answers it, on a
        // connection that is up already
        let out = outbound(&fake, "");
        drop(
            timeout(BOUND, out.session.lease(&ConnectOpts::default()))
                .await
                .unwrap()
                .unwrap(),
        );
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
        assert_eq!(fake.requests().len(), 1, "the request went out");
    }

    /// One QUIC connection carries every tunnel: past the server's limit on
    /// the streams open at once, a tunnel waits for one to end — within its
    /// time — rather than dialing another connection.
    #[tokio::test]
    async fn a_tunnel_past_the_servers_stream_limit_waits_for_one_to_end() {
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            max_streams: Some(1),
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "");
        let mut first = tunnel(&out, &target(echo)).await;
        round_trip(&mut first, b"first").await;
        let opts = ConnectOpts {
            timeout: Duration::from_millis(300),
        };
        let err = timeout(BOUND, out.connect_tcp(&target(echo), &opts))
            .await
            .expect("an outcome within the bound")
            .err()
            .unwrap();
        assert!(matches!(err, OutboundError::Timeout), "{err}");
        // the first ends both ways: its stream is the server's to reuse
        first.shutdown().await.unwrap();
        let mut rest = Vec::new();
        timeout(BOUND, first.read_to_end(&mut rest))
            .await
            .expect("the end of the tunnel")
            .unwrap();
        drop(first);
        let mut second = tunnel(&out, &target(echo)).await;
        round_trip(&mut second, b"second").await;
        assert_eq!(fake.connections(), 1);
        // the one that waited never went out
        assert_eq!(fake.requests().len(), 2);
    }

    /// quiche greases (masque-server, the TrustTunnel endpoint): a setting
    /// and a unidirectional stream of reserved kinds, and two frames of
    /// reserved types before every response head, a refusal's too. None of
    /// it gets in the way.
    #[tokio::test]
    async fn the_servers_grease_is_passed_over() {
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            users: users(),
            grease: true,
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "username=u, password=p");
        let mut stream = tunnel(&out, &target(echo)).await;
        round_trip(&mut stream, b"through the grease").await;
        let wrong = outbound(&fake, "username=u, password=wrong");
        let err = failure(&wrong, &target(echo)).await;
        assert_eq!(err.to_string(), "masque: proxy authentication required");
        // the connection goes on
        let mut again = tunnel(&out, &target(echo)).await;
        round_trip(&mut again, b"again").await;
        round_trip(&mut stream, b"and on").await;
        assert_eq!(fake.connections(), 2, "one for each outbound");
    }

    /// Design 3.3: the port changes under the connection — a new local
    /// socket each time — and its tunnels, old and new, go on. The interval
    /// is set past the configuration's minimum of 5 seconds.
    #[tokio::test]
    async fn tunnels_keep_working_while_the_port_hops() {
        let echo = echo_server().await;
        let ports = MultiPort::bind(3);
        let fake = FakeH3Proxy::spawn_on(H3ProxyScript::default(), ports.clone());
        let mut spec = spec(9, "");
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

    /// The credentials go to the server and nowhere else: not into the
    /// error of a refused login, nor into a log line at any level.
    #[tokio::test]
    async fn the_credentials_never_show() {
        const PASSWORD: &str = "s3cret-masque";
        let (log, _logging, _second) = Log::capture();
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            users: vec![("u".to_string(), PASSWORD.to_string())],
            ..H3ProxyScript::default()
        });
        let wrong = outbound(&fake, &format!("username=u, password={PASSWORD}-not"));
        let err = failure(&wrong, &target(echo)).await;
        assert_eq!(err.to_string(), "masque: proxy authentication required");
        let out = outbound(&fake, &format!("username=u, password={PASSWORD}"));
        let mut stream = tunnel(&out, &target(echo)).await;
        round_trip(&mut stream, b"let in").await;
        let basic = basic_authorization("u", PASSWORD);
        assert_eq!(
            fake.requests()[1].header("proxy-authorization"),
            Some(basic.as_str())
        );
        let shown = format!("{err} {err:?}");
        let log = log.text();
        assert!(!log.is_empty());
        let wrong_basic = basic_authorization("u", &format!("{PASSWORD}-not"));
        for secret in [
            PASSWORD,
            &basic["Basic ".len()..],
            &wrong_basic["Basic ".len()..],
        ] {
            assert!(!shown.contains(secret), "{shown}");
            assert!(!log.contains(secret));
        }
    }
}
```

新建 `crates/rurge-proto-quic/src/masque/tcp.rs`：

```rust
//! TCP over MASQUE (manual: Policies › MASQUE; phase 2 M7 design 7.1):
//! every connection a classic HTTP/3 CONNECT of its own on the policy's
//! QUIC connection (RFC 9114 4.4).
//!
//! - The request is `:method CONNECT` and `:authority host:port` — an IPv6
//!   literal in brackets, a name as its A-labels —, with
//!   `proxy-authorization` when the policy has credentials. It goes out at
//!   once: a classic CONNECT needs nothing of the server's SETTINGS.
//! - Nothing of the caller's goes before the response: a 2xx makes the
//!   stream the tunnel (RFC 9110 9.3.6); 407 is a refused login, any other
//!   final status the proxy's refusal, quoted by its number.
//! - Until the response came, a request that finds its connection gone —
//!   closed, lost by a server that restarted, or going away — is `Lost`:
//!   nothing of the caller's went out, and on another connection it may yet
//!   go through (`Session::request`).

use super::Connected;
use crate::session::{Failure, Lease};
use http::header::{HeaderValue, PROXY_AUTHORIZATION};
use http::{Request, StatusCode};
use rurge_net::connector::Target;
use rurge_net::quic::h3::RequestStream;
use rurge_proto::OutboundError;
use rurge_proto::http::wire_host;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The CONNECT for `target`, carrying the policy's `authorization`: refused,
/// before anything is dialed, for a name that cannot be sent.
pub(super) fn request(
    target: &Target,
    authorization: Option<&HeaderValue>,
) -> Result<Request<()>, OutboundError> {
    let unsendable = || proxy("the host name cannot be sent to the server");
    let host = wire_host(target).ok_or_else(unsendable)?;
    let mut request = Request::connect(format!("{host}:{}", target.port))
        .body(())
        // a name longer than an authority may be
        .map_err(|_| unsendable())?;
    if let Some(authorization) = authorization {
        request
            .headers_mut()
            .insert(PROXY_AUTHORIZATION, authorization.clone());
    }
    Ok(request)
}

/// The tunnel `request` asks for, on the connection of `lease`: the request
/// on a stream of its own, then the proxy's response. The stream handed
/// back is the caller's.
pub(super) async fn open(lease: Lease<Connected>, request: Request<()>) -> Result<Tunnel, Failure> {
    let mut stream = lease
        .state()
        .h3
        .request(request)
        .await
        .map_err(|e| failure(&lease, e))?;
    let response = stream.response().await.map_err(|e| failure(&lease, e))?;
    match response.status() {
        status if status.is_success() => Ok(Tunnel {
            stream,
            _lease: lease,
        }),
        StatusCode::PROXY_AUTHENTICATION_REQUIRED => {
            Err(Failure::Failed(proxy("proxy authentication required")))
        }
        status => Err(Failure::Failed(proxy(&format!(
            "the proxy answered {}",
            status.as_u16()
        )))),
    }
}

/// How a request failed before its response came: `Lost` when its
/// connection turned out gone or going away, else final. The error says
/// its layer (`h3: …`, `quic: …`).
fn failure(lease: &Lease<Connected>, e: io::Error) -> Failure {
    if lease.connection().close_reason().is_some() || lease.state().h3.is_going_away() {
        Failure::Lost(e.into())
    } else {
        Failure::Failed(e.into())
    }
}

fn proxy(text: &str) -> OutboundError {
    OutboundError::Proxy(format!("masque: {text}"))
}

/// A TCP connection through the proxy: the CONNECT's stream — the bytes go
/// as DATA frames —, on a connection its lease keeps. A write that returns
/// may leave the rest of its frame for the next write, flush or shutdown.
/// Shutdown sends the FIN the target then sees; reading goes on to the
/// proxy's. Dropped before both directions ended, the stream is cancelled
/// both ways (RFC 9114 4.1.1). No `Debug`: nothing in it is worth printing.
pub(super) struct Tunnel {
    stream: RequestStream,
    /// Last: the stream ends before its connection may be closed.
    _lease: Lease<Connected>,
}

impl AsyncRead for Tunnel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for Tunnel {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Method;
    use rurge_config::HostName;

    #[test]
    fn a_request_names_its_target_as_host_and_port_and_nothing_else() {
        for (host, authority) in [
            (HostName::parse("192.0.2.1"), "192.0.2.1:443"),
            (HostName::parse("2001:db8::1"), "[2001:db8::1]:443"),
            (
                HostName::Domain("bücher.example".into()),
                "xn--bcher-kva.example:443",
            ),
        ] {
            let request = request(&Target::new(host, 443), None).unwrap();
            assert_eq!(request.method(), Method::CONNECT);
            let uri = request.uri();
            assert_eq!(uri.authority().map(|a| a.as_str()), Some(authority));
            // a classic CONNECT: no scheme, no path (RFC 9114 4.4)
            assert!(uri.scheme().is_none(), "{uri}");
            assert!(uri.path_and_query().is_none(), "{uri}");
            assert!(request.headers().is_empty());
        }
        // longer than an authority may be
        let long = format!("{}.test", "a".repeat(70_000));
        for name in ["x@blocked.test", "a b.test", "", long.as_str()] {
            let target = Target::new(HostName::Domain(name.into()), 443);
            let err = request(&target, None).unwrap_err();
            assert_eq!(
                err.to_string(),
                "masque: the host name cannot be sent to the server"
            );
        }
    }

    #[test]
    fn the_credentials_go_marked_sensitive() {
        let mut authorization = HeaderValue::from_static("Basic dTpw");
        authorization.set_sensitive(true);
        let target = Target::new(HostName::parse("192.0.2.1"), 443);
        let request = request(&target, Some(&authorization)).unwrap();
        let headers = request.headers();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[PROXY_AUTHORIZATION], "Basic dTpw");
        // QPACK never indexes it (RFC 9204 7.1.3)
        assert!(headers[PROXY_AUTHORIZATION].is_sensitive());
        assert!(!format!("{request:?}").contains("dTpw"));
    }
}
```

新建 `crates/rurge-proto-quic/src/testing/h3proxy.rs`：

```rust
//! A scriptable MASQUE proxy (phase 2 M7 design 7.4): QUIC with ALPN `h3`
//! on a loopback port of its own (`ServerPort`: it can restart) or on a
//! socket it is given (`MultiPort` for port hopping), HTTP/3 by the server
//! side of `rurge_net`, and every classic CONNECT relayed to its target
//! over TCP, the half-closes too.
//!
//! - Its SETTINGS are masque-server's — field sections of at most 8 KiB,
//!   extended CONNECT, HTTP Datagrams — unless the script says otherwise.
//!   Scripted so, it greases as quiche does by default (masque-server and
//!   the TrustTunnel endpoint are quiche's).
//! - As masque-server, it wants Basic credentials in `proxy-authorization`
//!   on every request when its script names users: a request without them,
//!   or with others, is answered 407 with `proxy-authenticate`. Every
//!   refusal is a final status with `content-length: 0`, then the end of
//!   the stream.
//! - A request whose head is not well formed — its pseudo-header fields out
//!   of place, unknown or twice, a classic CONNECT with `:scheme` or
//!   `:path`, an authority without a port — is answered 400; one with
//!   another method or with a protocol it does not serve, 501; one whose
//!   target it cannot reach, 502.
//!
//! What the client sends is read by code that is not the client's: the
//! field sections by the HTTP/3 server side of `rurge_net`, the tunnels'
//! frames and the credentials by the code here. It records every request,
//! and never resolves a name.

use super::ServerPort;
use quinn::{AsyncUdpSocket, Connection, Endpoint, ServerConfig, TransportConfig, VarInt};
use rurge_net::quic::h3::testing::{H3Server, ServerStream};
use rurge_net::quic::testing::{TestCert, endpoint};
use rustls::RootCertStore;
use std::io;
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::task::JoinSet;
use tokio_util::task::AbortOnDropHandle;

// the wire as the fake knows it
const DATA: u64 = 0x00;
const HEADERS: u64 = 0x01;
const SETTINGS_MAX_FIELD_SECTION_SIZE: u64 = 0x06;
const SETTINGS_ENABLE_CONNECT_PROTOCOL: u64 = 0x08;
const SETTINGS_H3_DATAGRAM: u64 = 0x33;
/// H3_NO_ERROR: how the fake closes its connections.
const H3_NO_ERROR: VarInt = VarInt::from_u32(0x100);
/// H3_REQUEST_REJECTED: a request past the fake's GOAWAY, never served.
const H3_REQUEST_REJECTED: u64 = 0x10b;
/// The largest field section masque-server takes, as its SETTINGS say.
const MAX_FIELD_SECTION_SIZE: u64 = 8192;
/// The pseudo-header fields of a request (RFC 9114 4.3.1, RFC 8441 4).
const PSEUDO: [&str; 5] = [":method", ":protocol", ":scheme", ":authority", ":path"];
/// What quiche writes in its reserved frame and on its reserved stream.
const GREASE: &[u8] = b"GREASE is the word";
/// How much of a TCP tunnel the fake relays at a time, each way.
const RELAY: usize = 64 << 10;
/// The longest frame of the client's the fake reads: past any it sends.
const MAX_FRAME: u64 = 1 << 20;

#[derive(Clone, Debug, Default)]
pub struct H3ProxyScript {
    /// Want the Basic credentials of one of these (user, password) on every
    /// request; with none, anyone gets in.
    pub users: Vec<(String, String)>,
    /// Answer every request with this status instead of serving it.
    pub refuse: Option<u16>,
    /// Take requests and never answer them.
    pub silent: bool,
    /// Tunnel here whatever the client asked for (needed for a name).
    pub connect_to: Option<SocketAddr>,
    /// Send GOAWAY once a connection has taken this many requests: those run
    /// to their end; a later one on that connection is rejected
    /// (H3_REQUEST_REJECTED).
    pub goaway_after: Option<usize>,
    /// The most bidirectional streams the client may have open at once on a
    /// connection (QUIC's `initial_max_streams_bidi`): quinn's 100 unless
    /// set.
    pub max_streams: Option<u32>,
    /// Leave extended CONNECT out of its SETTINGS.
    pub no_extended_connect: bool,
    /// Say `H3_DATAGRAM = 0` in its SETTINGS.
    pub no_h3_datagram: bool,
    /// Take no QUIC datagrams at all: the transport parameter that allows
    /// them is not sent.
    pub no_datagrams: bool,
    /// Grease as quiche does: a setting of a reserved identifier, a
    /// unidirectional stream of a reserved type, and two frames of reserved
    /// types before every response head (RFC 9114 7.2.4.1, 6.2.3, 7.2.8).
    pub grease: bool,
}

/// A request, as the fake read its head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedH3Request {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// `:method`; empty without one.
    pub method: String,
    /// `:protocol` (extended CONNECT).
    pub protocol: Option<String>,
    pub scheme: Option<String>,
    pub authority: Option<String>,
    pub path: Option<String>,
    /// The other fields, in order.
    pub headers: Vec<(String, String)>,
}

impl RecordedH3Request {
    /// The pseudo-header fields of `fields` — the first of each name — and
    /// the others.
    fn new(connection: usize, fields: &[(String, String)]) -> RecordedH3Request {
        let pseudo = |name: &str| {
            fields
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
        };
        RecordedH3Request {
            connection,
            method: pseudo(":method").unwrap_or_default(),
            protocol: pseudo(":protocol"),
            scheme: pseudo(":scheme"),
            authority: pseudo(":authority"),
            path: pseudo(":path"),
            headers: fields
                .iter()
                .filter(|(name, _)| !name.starts_with(':'))
                .cloned()
                .collect(),
        }
    }

    /// The first header field called `name`.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

pub struct FakeH3Proxy {
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
    script: H3ProxyScript,
    /// The server's side of every connection accepted, in order.
    connections: Mutex<Vec<Connection>>,
    requests: Mutex<Vec<RecordedH3Request>>,
    /// Restart once the next request came.
    restart_at_request: AtomicBool,
    /// The fake, to restart it.
    fake: Weak<Inner>,
}

impl Shared {
    fn connections(&self) -> MutexGuard<'_, Vec<Connection>> {
        self.connections.lock().expect("connections")
    }

    /// Its SETTINGS: masque-server's as the script leaves them, and a
    /// reserved one when it greases.
    fn settings(&self) -> Vec<(u64, u64)> {
        let script = &self.script;
        let mut settings = vec![(SETTINGS_MAX_FIELD_SECTION_SIZE, MAX_FIELD_SECTION_SIZE)];
        if !script.no_extended_connect {
            settings.push((SETTINGS_ENABLE_CONNECT_PROTOCOL, 1));
        }
        settings.push((SETTINGS_H3_DATAGRAM, u64::from(!script.no_h3_datagram)));
        if script.grease {
            settings.push((grease_value(), grease_value()));
        }
        settings
    }

    /// Whether `request` carries one of the script's users in
    /// `proxy-authorization`, once and exactly so; anyone gets in when the
    /// script names none.
    fn authorized(&self, request: &RecordedH3Request) -> bool {
        let users = &self.script.users;
        let mut given = request
            .headers
            .iter()
            .filter(|(name, _)| name == "proxy-authorization")
            .map(|(_, value)| value);
        match (given.next(), given.next()) {
            _ if users.is_empty() => true,
            (Some(value), None) => users
                .iter()
                .any(|(user, password)| *value == basic(user, password)),
            _ => false,
        }
    }
}

impl FakeH3Proxy {
    /// On a loopback port of its own, which it can take over again
    /// (`restart`).
    pub fn spawn(script: H3ProxyScript) -> FakeH3Proxy {
        let port = ServerPort::bind();
        let fake = FakeH3Proxy::new(script, port.addr(), Some(port));
        fake.inner.take_over();
        fake
    }

    /// On `socket` — a `MultiPort` for port hopping —, which it cannot
    /// take over again.
    pub fn spawn_on(script: H3ProxyScript, socket: Arc<dyn AsyncUdpSocket>) -> FakeH3Proxy {
        let addr = socket.local_addr().expect("the socket's address");
        let fake = FakeH3Proxy::new(script, addr, None);
        fake.inner
            .listen(endpoint(fake.inner.server_config(), socket));
        fake
    }

    fn new(script: H3ProxyScript, addr: SocketAddr, port: Option<ServerPort>) -> FakeH3Proxy {
        let inner = Arc::new_cyclic(|fake| Inner {
            cert: TestCert::new(),
            addr,
            port,
            shared: Arc::new(Shared {
                script,
                connections: Mutex::default(),
                requests: Mutex::default(),
                restart_at_request: AtomicBool::new(false),
                fake: fake.clone(),
            }),
            endpoints: Mutex::default(),
        });
        FakeH3Proxy { inner }
    }

    /// Its port: the first of them, on a socket of several.
    pub fn addr(&self) -> SocketAddr {
        self.inner.addr
    }

    /// Roots that trust its certificate (for `localhost` and `127.0.0.1`).
    pub fn roots(&self) -> Arc<RootCertStore> {
        self.inner.cert.roots()
    }

    /// Every request it read, in arrival order.
    pub fn requests(&self) -> Vec<RecordedH3Request> {
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

    /// The next request finds the server gone: it restarts as `restart`
    /// does once the request came, before answering it.
    pub fn restart_at_next_request(&self) {
        self.inner
            .shared
            .restart_at_request
            .store(true, Ordering::SeqCst);
    }
}

impl Inner {
    /// QUIC with ALPN `h3`: datagrams included unless the script says
    /// none, quinn's limit on the client's streams unless it sets one.
    fn server_config(&self) -> ServerConfig {
        let script = &self.shared.script;
        let mut transport = TransportConfig::default();
        if script.no_datagrams {
            transport.datagram_receive_buffer_size(None);
        }
        if let Some(max) = script.max_streams {
            transport.max_concurrent_bidi_streams(VarInt::from_u32(max));
        }
        let mut config = self.cert.server_config(&["h3"]);
        config.transport_config(Arc::new(transport));
        config
    }

    /// A new endpoint takes the port over; the one before falls silent.
    fn take_over(&self) {
        let port = self.port.as_ref().expect("a fake on a port of its own");
        self.listen(port.endpoint(self.server_config()));
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

/// One connection: HTTP/3, and each request in a task of its own, all
/// ending with the connection. Once it took `goaway_after` requests it
/// sends GOAWAY, and rejects any request that comes after.
async fn serve(connection: Connection, shared: Arc<Shared>) {
    let number = {
        let mut connections = shared.connections();
        connections.push(connection.clone());
        connections.len() - 1
    };
    let Ok(mut h3) = H3Server::start(connection.clone(), &shared.settings()).await else {
        return;
    };
    let mut tasks = JoinSet::new();
    if shared.script.grease {
        tasks.spawn(grease_stream(connection));
    }
    let mut taken = 0;
    // the first stream its GOAWAY leaves unserved, once it sent one
    let mut unserved = None;
    loop {
        tokio::select! {
            stream = h3.accept() => match stream {
                Some(mut stream) => {
                    if unserved.is_some_and(|id| stream.id() >= id) {
                        stream.reset(H3_REQUEST_REJECTED);
                        continue;
                    }
                    taken += 1;
                    if shared.script.goaway_after == Some(taken) {
                        let id = stream.id() + 4;
                        let _ = h3.goaway(id).await;
                        unserved = Some(id);
                    }
                    let shared = shared.clone();
                    tasks.spawn(async move {
                        let _ = answer(stream, number, shared).await;
                    });
                }
                None => return,
            },
            // a task that ended; nothing to collect
            Some(_) = tasks.join_next() => {}
        }
    }
}

/// A request: read and recorded; then the fake restarts, leaves it
/// unanswered, refuses it, or makes the tunnel to its target.
async fn answer(
    mut stream: ServerStream,
    connection: usize,
    shared: Arc<Shared>,
) -> io::Result<()> {
    let fields = stream.read_headers().await?;
    let request = RecordedH3Request::new(connection, &fields);
    shared
        .requests
        .lock()
        .expect("requests")
        .push(request.clone());
    if shared.restart_at_request.swap(false, Ordering::SeqCst) {
        // gone before it answered: a new endpoint has the port
        if let Some(fake) = shared.fake.upgrade() {
            fake.take_over();
        }
        return Ok(());
    }
    let script = &shared.script;
    if script.silent {
        // what the client sends is dropped until it gives up
        tokio::io::copy(&mut stream, &mut tokio::io::sink()).await?;
        return Ok(());
    }
    let refusal = if !well_formed(&fields) {
        Some(400)
    } else if !shared.authorized(&request) {
        Some(407)
    } else if script.refuse.is_some() {
        script.refuse
    } else if request.method != "CONNECT" || request.protocol.is_some() {
        Some(501)
    } else {
        None
    };
    if let Some(status) = refusal {
        return refuse(&mut stream, status, script.grease).await;
    }
    // never resolves: a name without `connect_to` is a dead end
    let to = script
        .connect_to
        .or_else(|| request.authority.as_deref()?.parse().ok());
    let upstream = match to {
        Some(to) => TcpStream::connect(to).await.ok(),
        None => None,
    };
    // a target it cannot reach, as masque-server answers it
    let Some(upstream) = upstream else {
        return refuse(&mut stream, 502, script.grease).await;
    };
    respond(&mut stream, 200, &[], script.grease).await?;
    tunnel(stream, upstream).await
}

/// Whether the head of a request is well formed as the fake reads it: its
/// pseudo-header fields — a request's, each once — before the others,
/// whose names are lowercase (RFC 9114 4.2, 4.3); a classic CONNECT with
/// an authority `host:port` and neither `:scheme` nor `:path` (RFC 9114
/// 4.4), an extended one with all three (RFC 8441 4), any other request
/// with `:scheme` and `:path`.
fn well_formed(fields: &[(String, String)]) -> bool {
    let first_regular = fields
        .iter()
        .position(|(name, _)| !name.starts_with(':'))
        .unwrap_or(fields.len());
    let (pseudo, regular) = fields.split_at(first_regular);
    let count = |name: &str| pseudo.iter().filter(|(n, _)| n == name).count();
    if pseudo
        .iter()
        .any(|(name, _)| !PSEUDO.contains(&name.as_str()) || count(name) > 1)
        || regular
            .iter()
            .any(|(name, _)| name.starts_with(':') || name.bytes().any(|b| b.is_ascii_uppercase()))
    {
        return false;
    }
    let value = |name: &str| {
        pseudo
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    };
    let (authority, scheme, path) = (value(":authority"), value(":scheme"), value(":path"));
    match (value(":method"), value(":protocol")) {
        (Some("CONNECT"), None) => {
            authority.is_some_and(authority_form) && scheme.is_none() && path.is_none()
        }
        (Some("CONNECT"), Some(_)) => {
            authority.is_some_and(|a| !a.is_empty()) && scheme.is_some() && path.is_some()
        }
        (Some(_), None) => scheme.is_some() && path.is_some(),
        _ => false,
    }
}

/// `host:port`, as a CONNECT names its target (RFC 9112 3.2.3): a port from
/// 1 to 65535 after a host, an IPv6 literal in brackets.
fn authority_form(authority: &str) -> bool {
    let Some((host, port)) = authority.rsplit_once(':') else {
        return false;
    };
    let host_ok = match host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        Some(literal) => literal.parse::<Ipv6Addr>().is_ok(),
        None => !host.is_empty() && !host.contains([':', '[', ']', '@', '/']),
    };
    host_ok && port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok_and(|p| p > 0)
}

/// A final status and no tunnel, as masque-server refuses: `content-length:
/// 0` — after the challenge, for a 407 — and the end of the stream.
async fn refuse(stream: &mut ServerStream, status: u16, grease: bool) -> io::Result<()> {
    let mut fields = Vec::new();
    if status == 407 {
        fields.push(("proxy-authenticate", "Basic realm=\"masque\""));
    }
    fields.push(("content-length", "0"));
    respond(stream, status, &fields, grease).await?;
    stream.finish();
    Ok(())
}

/// The response head: when the fake greases, after a frame of a reserved
/// type without payload and one with, as quiche writes them before its
/// first HEADERS on a stream.
async fn respond(
    stream: &mut ServerStream,
    status: u16,
    fields: &[(&str, &str)],
    grease: bool,
) -> io::Result<()> {
    if grease {
        stream.send_frame(grease_value(), b"").await?;
        stream.send_frame(grease_value(), GREASE).await?;
    }
    stream.send_headers(status, fields).await
}

/// The client's DATA frames to the target, and what the target sends back
/// in DATA frames of the fake's; each side's end is the other's
/// half-close. Either side failing ends both.
async fn tunnel(stream: ServerStream, upstream: TcpStream) -> io::Result<()> {
    let (mut from_client, mut to_client) = tokio::io::split(stream);
    let (mut from_target, mut to_target) = upstream.into_split();
    let up = async {
        while let Some(data) = read_data(&mut from_client).await? {
            to_target.write_all(&data).await?;
        }
        to_target.shutdown().await
    };
    let down = async {
        let mut buf = vec![0u8; RELAY];
        loop {
            match from_target.read(&mut buf).await? {
                0 => return to_client.shutdown().await,
                n => to_client.write_all(&frame(DATA, &buf[..n])).await?,
            }
        }
    };
    tokio::try_join!(up, down).map(|_| ())
}

/// The payload of the client's next DATA frame on a tunnel, frames of
/// unknown types passed over; `None` at the end of the stream. HEADERS —
/// trailers — have no place there (RFC 9114 4.4).
async fn read_data(from: &mut (impl AsyncRead + Unpin)) -> io::Result<Option<Vec<u8>>> {
    loop {
        let Some(ty) = read_varint(from).await? else {
            return Ok(None);
        };
        let len = read_varint(from).await?.ok_or_else(cut_short)?;
        if len > MAX_FRAME {
            return Err(invalid("a frame longer than the fake reads"));
        }
        let mut payload = vec![0u8; len as usize];
        from.read_exact(&mut payload).await?;
        match ty {
            DATA => return Ok(Some(payload)),
            HEADERS => return Err(invalid("HEADERS on a tunnel")),
            _ => {}
        }
    }
}

/// A QUIC integer, a byte at a time; `None` at the end of the stream
/// before its first byte.
async fn read_varint(from: &mut (impl AsyncRead + Unpin)) -> io::Result<Option<u64>> {
    let mut first = [0u8];
    if from.read(&mut first).await? == 0 {
        return Ok(None);
    }
    let mut value = u64::from(first[0] & 0x3f);
    for _ in 1..1 << (first[0] >> 6) {
        value = value << 8 | u64::from(from.read_u8().await?);
    }
    Ok(Some(value))
}

/// `value` as a QUIC integer, as short as it goes.
fn put_varint(out: &mut Vec<u8>, value: u64) {
    match value {
        0..0x40 => out.push(value as u8),
        0x40..0x4000 => out.extend_from_slice(&(0x4000 | value as u16).to_be_bytes()),
        0x4000..0x4000_0000 => out.extend_from_slice(&(0x8000_0000 | value as u32).to_be_bytes()),
        _ => out.extend_from_slice(&(0xc000_0000_0000_0000 | value).to_be_bytes()),
    }
}

fn frame(ty: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + payload.len());
    put_varint(&mut out, ty);
    put_varint(&mut out, payload.len() as u64);
    out.extend_from_slice(payload);
    out
}

/// A unidirectional stream of a reserved type, as quiche opens one: the
/// client stops it, and HTTP/3 goes on (RFC 9114 6.2.3).
async fn grease_stream(connection: Connection) {
    let Ok(mut send) = connection.open_uni().await else {
        return;
    };
    let mut bytes = Vec::new();
    put_varint(&mut bytes, grease_value());
    bytes.extend_from_slice(GREASE);
    if send.write_all(&bytes).await.is_ok() {
        let _ = send.finish();
    }
}

/// A reserved value as quiche draws one: 0x1f × N + 0x21 (RFC 9114 7.2.8),
/// N as large as a QUIC integer allows.
fn grease_value() -> u64 {
    const N: u64 = 148_764_065_110_560_899;
    0x1f * getrandom::u64().map_or(0, |n| n % N) + 0x21
}

/// `Basic base64(user:password)`, standard Base64 with its padding (RFC
/// 7617 2, RFC 4648 4): the fake's own encoding, not the client's.
fn basic(user: &str, password: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::from("Basic ");
    for chunk in format!("{user}:{password}").as_bytes().chunks(3) {
        let bits = chunk
            .iter()
            .enumerate()
            .fold(0u32, |bits, (i, &b)| bits | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                char::from(ALPHABET[(bits >> (18 - 6 * i)) as usize & 0x3f])
            } else {
                '='
            });
        }
    }
    out
}

fn cut_short() -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "h3 proxy fake: the stream ended within a frame",
    )
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("h3 proxy fake: {what}"))
}
```

`crates/rurge-proto-quic/src/testing/mod.rs`——把

```rust
//! by a server that restarts, and scriptable Hysteria 2 and TUIC servers.
//! Never used by production code.

mod hysteria2;
```

换成

```rust
//! by a server that restarts, and scriptable Hysteria 2, TUIC and MASQUE
//! servers. Never used by production code.

mod h3proxy;
mod hysteria2;
```

`crates/rurge-proto-quic/src/testing/mod.rs`——把

```rust
mod tuic;

pub use hysteria2::{
```

换成

```rust
mod tuic;

pub use h3proxy::{FakeH3Proxy, H3ProxyScript, RecordedH3Request};
pub use hysteria2::{
```

要点：
- 请求只有 `:method CONNECT` 与 `:authority host:port`；凭据在每个 CONNECT 上，标为敏感。
- 应答到达之前，失败时看连接的状态（已关闭或正在离开）判 `Failure::Lost`，会话在新连接上重试一次。
- 隧道原样委托 `flush`：HTTP/3 的写可能把半帧留到下一次写、`flush` 或 `shutdown`。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto` → 通过。
Run: `cargo test -p rurge-proto-quic masque` → 通过（新增 17 条：经典 CONNECT 的形状、大数据与半关闭、目标先结束、`host:port` 的三种写法与发不出去的名字、凭据随每个请求而没有凭据时不带、拒绝按状态码说、并发隧道共用一条连接、GOAWAY 后新隧道上新连接而旧的走完、服务端关连接后重拨、服务端重启后重试一次、握手不应答时受时限约束、超过服务端流数时等流释放、服务端的 GREASE 被跳过、端口跳跃中隧道不断、凭据不进错误文本 / 日志 / `Debug`，以及 `tcp.rs` 的两条）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-proto crates/rurge-proto-quic
git commit -m "feat(proto-quic): MASQUE 的 TCP——HTTP/3 经典 CONNECT、每个请求的 Basic 认证；FakeH3Proxy"
```

### Task 4: MASQUE 的 UDP（CONNECT-UDP）

`MasqueOutbound::udp()` 翻转为原生支持：每个目标一条 CONNECT-UDP 流，包走 HTTP/3 数据报（P9）。capsule 编解码与 `masque_path` 从 `h2connect` 移到共用的 `rurge_proto::connect_udp`（P10）。`FakeH3Proxy` 补上 CONNECT-UDP 一侧。新模块的用例在新文件里，没有单独的失败步骤；移来的用例照常通过，钉住移动不改行为。

**Files:**
- Create: `crates/rurge-proto-quic/src/masque/udp.rs`（自带用例）、`crates/rurge-proto/src/connect_udp.rs`（自带用例，移来的）
- Delete: `crates/rurge-proto/src/h2connect/capsule.rs`
- Modify: `crates/rurge-proto/src/{lib.rs, build.rs, h2connect/mod.rs, h2connect/udp.rs}`、`crates/rurge-proto-quic/src/masque/{mod.rs, tcp.rs}`（`mod.rs` 含用例）、`src/testing/{h3proxy.rs, mod.rs}`

**Interfaces:**
- Consumes: Task 2 的 `h3::Protocol::CONNECT_UDP`、`RequestStream::datagrams`、`h3::Datagrams`；Task 3 的 `MasqueOutbound`、`FakeH3Proxy`；M7a 的会话与 UDP 载体的做法（至多每秒比较一次路由）。
- Produces: `rurge_proto::connect_udp::{MAX_PAYLOAD, masque_path(target: &Target) -> Option<String>, datagram(payload: &[u8]) -> Vec<u8>, read_datagram(r, label) }`；`rurge_proto::build::tls_name` 改为 `pub`；`MasqueOutbound::udp()` → `UdpSupport::Native`、`open_udp`；`FakeH3Proxy` 的 CONNECT-UDP（`H3ProxyScript` 的 `no_extended_connect` / `no_h3_datagram` / `no_datagrams` / `udp_response` / `udp_end_at_once` / `udp_capsules` / `udp_bad_capsule`，记录 `datagrams()` / `udp_ends()`，`send_datagram`）

- [ ] **Step 1: 共用的 CONNECT-UDP 代码移到 `rurge_proto::connect_udp`**

`crates/rurge-proto/src/lib.rs`——把

```rust
pub mod build;
```

换成

```rust
pub mod build;
pub mod connect_udp;
```

新建 `crates/rurge-proto/src/connect_udp.rs`：

```rust
//! CONNECT-UDP (RFC 9298) as `h2-connect` asks for it over HTTP/2 and
//! `masque` over HTTP/3: the request's path, by the default URI template,
//! and the capsules of its stream (RFC 9297 3.2).
//!
//! - Capsules go `type (varint), length (varint), value`, back to back
//!   across the stream's DATA frames. A DATAGRAM capsule (type 0) holds a
//!   context id (a varint, 0 for a UDP payload) and the datagram. Integers
//!   are QUIC varints (RFC 9000 16), which need not be minimal.
//! - Errors name the caller (`h2-connect: …`, `masque: …`).

use rurge_config::HostName;
use rurge_net::connector::Target;
use std::io;
use std::net::IpAddr;
use tokio::io::{AsyncRead, AsyncReadExt};

/// The DATAGRAM capsule type (RFC 9297 3.5).
const DATAGRAM: u64 = 0;

/// The longest UDP payload with context id 0 (RFC 9298 5): no longer one
/// is sent, and a longer one received ends the stream.
pub const MAX_PAYLOAD: usize = 65527;

/// The longest DATAGRAM capsule value read: an 8-byte context id and the
/// longest payload. Nothing longer is buffered.
const MAX_VALUE: u64 = 8 + MAX_PAYLOAD as u64;

/// The default URI template's path for `target` (RFC 9298 2, 3): an IPv6
/// literal without brackets and with its colons percent-encoded, an IDN
/// as its A-labels. Names and IPv4 literals hold only unreserved
/// characters, so nothing else needs encoding. `None` for a name that
/// cannot be sent (the alphabet of `http::wire_host`).
pub fn masque_path(target: &Target) -> Option<String> {
    let host = match &target.host {
        HostName::Ip(IpAddr::V6(v6)) => v6.to_string().replace(':', "%3A"),
        HostName::Ip(ip) => ip.to_string(),
        HostName::Domain(name) => crate::hostname::to_ascii(name)?,
    };
    Some(format!("/.well-known/masque/udp/{host}/{}/", target.port))
}

/// `value` as a minimal varint. Values from 2^62 on cannot be written; the
/// callers write lengths of at most 64 KiB.
fn put_varint(out: &mut Vec<u8>, value: u64) {
    match value {
        0..=0x3f => out.push(value as u8),
        0x40..=0x3fff => out.extend_from_slice(&(value as u16 | 0x4000).to_be_bytes()),
        0x4000..=0x3fff_ffff => out.extend_from_slice(&(value as u32 | 0x8000_0000).to_be_bytes()),
        _ => {
            debug_assert!(value < 1 << 62, "a varint holds 62 bits");
            out.extend_from_slice(&(value | 0xc000_0000_0000_0000).to_be_bytes());
        }
    }
}

/// The varint at the start of `bytes` and its length; `None` when `bytes`
/// ends inside it.
fn parse_varint(bytes: &[u8]) -> Option<(u64, usize)> {
    let len = 1usize << (bytes.first()? >> 6);
    let bytes = bytes.get(..len)?;
    let value = bytes[1..]
        .iter()
        .fold(u64::from(bytes[0] & 0x3f), |v, &b| v << 8 | u64::from(b));
    Some((value, len))
}

fn malformed(label: &str, what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{label}: a malformed capsule ({what})"),
    )
}

/// A varint from `r`; `None` at the end of the stream before its first
/// byte (`at_start`: the end is clean there).
async fn read_varint<R: AsyncRead + Unpin>(
    r: &mut R,
    at_start: bool,
    label: &str,
) -> io::Result<Option<u64>> {
    let mut bytes = [0u8; 8];
    if r.read(&mut bytes[..1]).await? == 0 {
        return if at_start {
            Ok(None)
        } else {
            Err(malformed(label, "truncated"))
        };
    }
    let len = 1usize << (bytes[0] >> 6);
    r.read_exact(&mut bytes[1..len])
        .await
        .map_err(|_| malformed(label, "truncated"))?;
    Ok(parse_varint(&bytes[..len]).map(|(value, _)| value))
}

/// A DATAGRAM capsule with context id 0 carrying `payload` (at most
/// `MAX_PAYLOAD` bytes).
pub fn datagram(payload: &[u8]) -> Vec<u8> {
    debug_assert!(payload.len() <= MAX_PAYLOAD);
    let mut out = Vec::with_capacity(payload.len() + 5);
    put_varint(&mut out, DATAGRAM);
    // the context id is one byte
    put_varint(&mut out, payload.len() as u64 + 1);
    put_varint(&mut out, 0);
    out.extend_from_slice(payload);
    out
}

/// The next UDP payload on the stream: capsules of other types and
/// datagrams with another context id are skipped (RFC 9297 3.2, RFC 9298
/// 4). `None` at a clean end, between two capsules. A capsule cut short,
/// a DATAGRAM capsule without a context id, or a payload longer than
/// `MAX_PAYLOAD` is an error (RFC 9297 3.3, RFC 9298 5), which says `label`:
/// the stream is done.
pub async fn read_datagram<R: AsyncRead + Unpin>(
    r: &mut R,
    label: &str,
) -> io::Result<Option<Vec<u8>>> {
    loop {
        let Some(kind) = read_varint(r, true, label).await? else {
            return Ok(None);
        };
        let Some(len) = read_varint(r, false, label).await? else {
            return Err(malformed(label, "truncated"));
        };
        if kind != DATAGRAM {
            // dropped without being buffered
            let skipped = tokio::io::copy(&mut (&mut *r).take(len), &mut tokio::io::sink()).await?;
            if skipped != len {
                return Err(malformed(label, "truncated"));
            }
            continue;
        }
        if len > MAX_VALUE {
            return Err(malformed(label, "a datagram too long"));
        }
        let mut value = vec![0u8; len as usize];
        r.read_exact(&mut value)
            .await
            .map_err(|_| malformed(label, "truncated"))?;
        let Some((context, at)) = parse_varint(&value) else {
            return Err(malformed(label, "no context id"));
        };
        if context != 0 {
            continue;
        }
        if value.len() - at > MAX_PAYLOAD {
            return Err(malformed(label, "a datagram too long"));
        }
        value.drain(..at);
        return Ok(Some(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn read_all(mut bytes: &[u8], label: &str) -> io::Result<Vec<Vec<u8>>> {
        let mut got = Vec::new();
        while let Some(payload) = read_datagram(&mut bytes, label).await? {
            got.push(payload);
        }
        Ok(got)
    }

    #[test]
    fn the_path_carries_the_target_by_the_default_template() {
        for (host, path) in [
            (
                HostName::parse("192.0.2.6"),
                "/.well-known/masque/udp/192.0.2.6/443/",
            ),
            // RFC 9298 3's example
            (
                HostName::parse("2001:db8::42"),
                "/.well-known/masque/udp/2001%3Adb8%3A%3A42/443/",
            ),
            (
                HostName::parse("::ffff:192.0.2.6"),
                "/.well-known/masque/udp/%3A%3Affff%3A192.0.2.6/443/",
            ),
            (
                HostName::Domain("bücher.example".into()),
                "/.well-known/masque/udp/xn--bcher-kva.example/443/",
            ),
            (
                HostName::Domain("_srv.example.test".into()),
                "/.well-known/masque/udp/_srv.example.test/443/",
            ),
        ] {
            assert_eq!(
                masque_path(&Target::new(host.clone(), 443)).as_deref(),
                Some(path),
                "{host:?}"
            );
        }
        for name in ["x@blocked.test", "a/b.test", "a b.test", ""] {
            let target = Target::new(HostName::Domain(name.into()), 443);
            assert_eq!(masque_path(&target), None, "{name:?}");
        }
    }

    #[test]
    fn varints_are_written_minimal_and_read_in_every_length() {
        // RFC 9000 A.1
        for (value, wire) in [
            (37u64, &[0x25u8][..]),
            (15293, &[0x7b, 0xbd]),
            (494_878_333, &[0x9d, 0x7f, 0x3e, 0x7d]),
            (
                151_288_809_941_952_652,
                &[0xc2, 0x19, 0x7c, 0x5e, 0xff, 0x14, 0xe8, 0x8c],
            ),
        ] {
            let mut out = Vec::new();
            put_varint(&mut out, value);
            assert_eq!(out, wire, "{value}");
            assert_eq!(parse_varint(wire), Some((value, wire.len())));
        }
        // 37 in two bytes, as RFC 9000 A.1 shows: not minimal, still 37
        assert_eq!(parse_varint(&[0x40, 0x25]), Some((37, 2)));
        assert_eq!(parse_varint(&[0x80, 0, 0]), None, "cut short");
        assert_eq!(parse_varint(&[]), None);
    }

    #[test]
    fn a_datagram_capsule_is_type_length_context_payload() {
        assert_eq!(datagram(b"abc"), [0x00, 0x04, 0x00, b'a', b'b', b'c']);
        assert_eq!(datagram(b""), [0x00, 0x01, 0x00]);
        let long = datagram(&[7u8; 100]);
        assert_eq!(&long[..4], [0x00, 0x40, 101, 0x00]);
        assert_eq!(long.len(), 104);
        let longest = datagram(&vec![7u8; MAX_PAYLOAD]);
        assert_eq!(&longest[..6], [0x00, 0x80, 0x00, 0xff, 0xf8, 0x00]);
    }

    #[tokio::test]
    async fn datagrams_come_back_whole_and_in_order() {
        let mut wire = datagram(b"one");
        wire.extend(datagram(b""));
        wire.extend(datagram(&vec![9u8; MAX_PAYLOAD]));
        let got = read_all(&wire, "h2-connect").await.unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(
            (got[0].as_slice(), got[1].as_slice()),
            (&b"one"[..], &b""[..])
        );
        assert!(got[2] == vec![9u8; MAX_PAYLOAD]);
    }

    #[tokio::test]
    async fn non_minimal_varints_are_read() {
        // type 0 in 8 bytes, length 4 in 4 bytes, context id 0 in 2 bytes
        let wire = [
            0xc0, 0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0, 4, 0x40, 0, b'h', b'i',
        ];
        assert_eq!(
            read_all(&wire, "h2-connect").await.unwrap(),
            [b"hi".to_vec()]
        );
    }

    #[tokio::test]
    async fn other_capsule_types_and_context_ids_are_skipped() {
        let mut wire = Vec::new();
        // an unknown type (0x2a2a, two bytes), five bytes of value
        wire.extend([0x6a, 0x2a, 0x05, 1, 2, 3, 4, 5]);
        // context id 2: a client-allocated context nobody registered
        wire.extend([0x00, 0x03, 0x02, b'n', b'o']);
        // an unknown type with an empty value
        wire.extend([0x17, 0x00]);
        wire.extend(datagram(b"yes"));
        assert_eq!(
            read_all(&wire, "h2-connect").await.unwrap(),
            [b"yes".to_vec()]
        );
    }

    /// The error names the caller: the codec is `h2-connect`'s and
    /// `masque`'s alike.
    #[tokio::test]
    async fn malformed_capsules_end_the_stream() {
        let too_long = {
            let mut wire = vec![0x00];
            put_varint(&mut wire, MAX_PAYLOAD as u64 + 2);
            wire.push(0x00);
            wire.extend(vec![0u8; MAX_PAYLOAD + 1]);
            wire
        };
        let over_the_bound = {
            let mut wire = vec![0x00];
            put_varint(&mut wire, MAX_VALUE + 1);
            wire
        };
        for (wire, what) in [
            (vec![0x00, 0x04, 0x00, b'a'], "truncated"),
            (vec![0x00], "truncated"),
            (vec![0x40], "truncated"),
            (vec![0x17, 0x09, 1, 2], "truncated"),
            (vec![0x00, 0x00], "no context id"),
            (vec![0x00, 0x01, 0x40], "no context id"),
            (too_long, "a datagram too long"),
            (over_the_bound, "a datagram too long"),
        ] {
            for label in ["h2-connect", "masque"] {
                let err = read_all(&wire, label).await.unwrap_err();
                assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{wire:?}");
                assert_eq!(
                    err.to_string(),
                    format!("{label}: a malformed capsule ({what})"),
                    "{wire:?}"
                );
            }
        }
    }
}
```

删除 `crates/rurge-proto/src/h2connect/capsule.rs`（它的内容移到了新目录下的文件里）。

`crates/rurge-proto/src/h2connect/mod.rs`——把

```rust
//!   extended CONNECT (RFC 8441), one stream per target (`udp`).

mod capsule;
```

换成

```rust
//!   extended CONNECT (RFC 8441), one stream per target (`udp`).

```

`crates/rurge-proto/src/h2connect/mod.rs`——把

```rust
use crate::build::{shadow_tls_client, tls_client, tls_name};
```

换成

```rust
use crate::build::{shadow_tls_client, tls_client, tls_name};
use crate::connect_udp::masque_path;
```

`crates/rurge-proto/src/h2connect/mod.rs`——把

```rust
        let Some(path) = udp::masque_path(target) else {
```

换成

```rust
        let Some(path) = masque_path(target) else {
```

`crates/rurge-proto/src/h2connect/udp.rs`——把

```rust
use super::Inner;
use super::capsule::{self, MAX_PAYLOAD};
use crate::OutboundError;
use crate::h2pool::H2Stream;
use crate::task::AbortOnDrop;
use rurge_config::HostName;
```

换成

```rust
use super::{Inner, LABEL};
use crate::OutboundError;
use crate::connect_udp::{self, MAX_PAYLOAD};
use crate::h2pool::H2Stream;
use crate::task::AbortOnDrop;
```

`crates/rurge-proto/src/h2connect/udp.rs`——把

```rust
use std::io;
use std::net::IpAddr;
```

换成

```rust
use std::io;
```

`crates/rurge-proto/src/h2connect/udp.rs`——把

```rust
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// The default URI template's path for `target` (RFC 9298 2, 3): an IPv6
/// literal without brackets and with its colons percent-encoded, an IDN
/// as its A-labels. Names and IPv4 literals hold only unreserved
/// characters, so nothing else needs encoding. `None` for a name that
/// cannot be sent (the alphabet of `http::wire_host`).
pub(super) fn masque_path(target: &Target) -> Option<String> {
    let host = match &target.host {
        HostName::Ip(IpAddr::V6(v6)) => v6.to_string().replace(':', "%3A"),
        HostName::Ip(ip) => ip.to_string(),
        HostName::Domain(name) => crate::hostname::to_ascii(name)?,
    };
    Some(format!("/.well-known/masque/udp/{host}/{}/", target.port))
}
```

换成

```rust
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
```

`crates/rurge-proto/src/h2connect/udp.rs`——把

```rust
                let mut next = std::pin::pin!(capsule::read_datagram(&mut reader));
```

换成

```rust
                let mut next = std::pin::pin!(connect_udp::read_datagram(&mut reader, LABEL));
```

`crates/rurge-proto/src/h2connect/udp.rs`——把

```rust
            let capsule = capsule::datagram(buf);
```

换成

```rust
            let capsule = connect_udp::datagram(buf);
```

`crates/rurge-proto/src/h2connect/udp.rs`——把

```rust
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_carries_the_target_by_the_default_template() {
        for (host, path) in [
            (
                HostName::parse("192.0.2.6"),
                "/.well-known/masque/udp/192.0.2.6/443/",
            ),
            // RFC 9298 3's example
            (
                HostName::parse("2001:db8::42"),
                "/.well-known/masque/udp/2001%3Adb8%3A%3A42/443/",
            ),
            (
                HostName::parse("::ffff:192.0.2.6"),
                "/.well-known/masque/udp/%3A%3Affff%3A192.0.2.6/443/",
            ),
            (
                HostName::Domain("bücher.example".into()),
                "/.well-known/masque/udp/xn--bcher-kva.example/443/",
            ),
            (
                HostName::Domain("_srv.example.test".into()),
                "/.well-known/masque/udp/_srv.example.test/443/",
            ),
        ] {
            assert_eq!(
                masque_path(&Target::new(host.clone(), 443)).as_deref(),
                Some(path),
                "{host:?}"
            );
        }
        for name in ["x@blocked.test", "a/b.test", "a b.test", ""] {
            let target = Target::new(HostName::Domain(name.into()), 443);
            assert_eq!(masque_path(&target), None, "{name:?}");
        }
    }
}

```

换成

```rust
        })
    }
}

```

`crates/rurge-proto/src/build.rs`——把

```rust
pub(crate) fn tls_name(tls: Option<&TlsOpts>, server: &HostName) -> HostName {
```

换成

```rust
pub fn tls_name(tls: Option<&TlsOpts>, server: &HostName) -> HostName {
```

- [ ] **Step 2: MASQUE 的 UDP 与假服务端的 CONNECT-UDP**

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
//! side of `rurge_net`, and every classic CONNECT relayed to its target
//! over TCP, the half-closes too.
```

换成

```rust
//! side of `rurge_net`, every classic CONNECT relayed to its target over
//! TCP, the half-closes too, and every CONNECT-UDP over UDP.
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
//!   target it cannot reach, 502.
```

换成

```rust
//!   target it cannot reach, 502.
//! - A CONNECT-UDP (RFC 9298) names its target in the default template's
//!   path. As masque-server, the fake answers `200` with `capsule-protocol:
//!   ?1` once its socket to the target is up, then relays the client's
//!   HTTP/3 datagrams with context id 0 to the target and the target's
//!   answers back in HTTP/3 datagrams — or, scripted so, in DATAGRAM
//!   capsules on the stream —; it reads no capsule of the client's. It
//!   records every datagram of the client's and how the client ended each
//!   such stream.
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
//! frames and the credentials by the code here. It records every request,
//! and never resolves a name.
```

换成

```rust
//! frames, the datagrams and the credentials by the code here. It records
//! every request, and never resolves a name.
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
use std::net::{Ipv6Addr, SocketAddr};
```

换成

```rust
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
use tokio::net::TcpStream;
```

换成

```rust
use tokio::net::{TcpStream, UdpSocket};
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
const SETTINGS_H3_DATAGRAM: u64 = 0x33;
```

换成

```rust
const SETTINGS_H3_DATAGRAM: u64 = 0x33;
/// The DATAGRAM capsule (RFC 9297 3.5).
const DATAGRAM_CAPSULE: u64 = 0x00;
/// A capsule type reserved for greasing (RFC 9297 5.4: 0x29 × N + 0x17).
const RESERVED_CAPSULE: u64 = 0x29 * 7 + 0x17;
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
    pub grease: bool,
```

换成

```rust
    pub grease: bool,
    /// Answer a CONNECT-UDP request with this status and these fields in
    /// place of `200` and `capsule-protocol: ?1`, and serve it as ever.
    pub udp_response: Option<(u16, Vec<(String, String)>)>,
    /// End a CONNECT-UDP stream as soon as its head went out.
    pub udp_end_at_once: bool,
    /// Send what a CONNECT-UDP target answers in DATAGRAM capsules on the
    /// stream rather than in HTTP/3 datagrams, each after a capsule of a
    /// reserved type and a DATAGRAM capsule with context id 2, all of it
    /// across two DATA frames.
    pub udp_capsules: bool,
    /// Follow the head of a CONNECT-UDP response with a DATAGRAM capsule
    /// that has no room for a context id: a malformed one.
    pub udp_bad_capsule: bool,
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
            .map(|(_, v)| v.as_str())
    }
}

pub struct FakeH3Proxy {
```

换成

```rust
            .map(|(_, v)| v.as_str())
    }
}

/// One of the client's HTTP/3 datagrams on a CONNECT-UDP stream, as the
/// fake read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedH3Datagram {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// The QUIC stream ID of its stream.
    pub stream: u64,
    pub context: u64,
    /// The length of what follows the context id: the UDP payload.
    pub len: usize,
}

/// How the client ended its side of a CONNECT-UDP stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedUdpEnd {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// Its QUIC stream ID.
    pub stream: u64,
    /// With a FIN, gracefully; else with a reset, or with the connection.
    pub fin: bool,
}

pub struct FakeH3Proxy {
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
    requests: Mutex<Vec<RecordedH3Request>>,
```

换成

```rust
    requests: Mutex<Vec<RecordedH3Request>>,
    datagrams: Mutex<Vec<RecordedH3Datagram>>,
    udp_ends: Mutex<Vec<RecordedUdpEnd>>,
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
                requests: Mutex::default(),
```

换成

```rust
                requests: Mutex::default(),
                datagrams: Mutex::default(),
                udp_ends: Mutex::default(),
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
        self.inner.shared.requests.lock().expect("requests").clone()
```

换成

```rust
        self.inner.shared.requests.lock().expect("requests").clone()
    }

    /// The client's datagrams on CONNECT-UDP streams, in arrival order.
    pub fn datagrams(&self) -> Vec<RecordedH3Datagram> {
        self.inner
            .shared
            .datagrams
            .lock()
            .expect("datagrams")
            .clone()
    }

    /// How the client ended its side of CONNECT-UDP streams, in order.
    pub fn udp_ends(&self) -> Vec<RecordedUdpEnd> {
        self.inner.shared.udp_ends.lock().expect("udp ends").clone()
    }

    /// An HTTP/3 datagram of `payload` — a context id first, or whatever a
    /// test wants — for stream `stream` of connection `n`.
    pub fn send_datagram(&self, n: usize, stream: u64, payload: &[u8]) {
        let mut datagram = Vec::with_capacity(8 + payload.len());
        put_varint(&mut datagram, stream / 4);
        datagram.extend_from_slice(payload);
        self.connection(n)
            .send_datagram(datagram.into())
            .expect("the client takes datagrams");
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
/// unanswered, refuses it, or makes the tunnel to its target.
```

换成

```rust
/// unanswered, refuses it, or makes the tunnel to its target — its UDP
/// relay for a CONNECT-UDP.
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
    } else if request.method != "CONNECT" || request.protocol.is_some() {
```

换成

```rust
    } else if request.method != "CONNECT"
        || request
            .protocol
            .as_deref()
            .is_some_and(|protocol| protocol != "connect-udp")
    {
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
        return refuse(&mut stream, status, script.grease).await;
```

换成

```rust
        return refuse(&mut stream, status, script.grease).await;
    }
    if request.protocol.is_some() {
        return connect_udp(stream, &request, &shared).await;
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
    tokio::try_join!(up, down).map(|_| ())
}

/// The payload of the client's next DATA frame on a tunnel, frames of
```

换成

```rust
    tokio::try_join!(up, down).map(|_| ())
}

/// A CONNECT-UDP request (RFC 9298), as masque-server serves one: the
/// target in its path — a name only with `connect_to` —, a UDP socket that
/// hears the target alone, the head once the socket is up, then the
/// client's datagrams with context id 0 to the target and the target's
/// answers back, until the client ends its side of the stream or the
/// connection ends.
async fn connect_udp(
    mut stream: ServerStream,
    request: &RecordedH3Request,
    shared: &Shared,
) -> io::Result<()> {
    let script = &shared.script;
    let Some((host, port)) = request.path.as_deref().and_then(masque_target) else {
        return refuse(&mut stream, 400, script.grease).await;
    };
    // never resolves: a name without `connect_to` is a dead end
    let to = script
        .connect_to
        .or_else(|| Some(SocketAddr::new(host.parse().ok()?, port)));
    let socket = match to {
        Some(to) => udp_socket(to).await.ok(),
        None => None,
    };
    // a target it cannot reach, as masque-server answers it
    let Some(socket) = socket else {
        return refuse(&mut stream, 502, script.grease).await;
    };
    let (connection, id) = (request.connection, stream.id());
    let quic = shared.connections()[connection].clone();
    // taken before the head: whatever the client sends after it is heard
    let mut datagrams = stream.datagrams();
    let (status, fields) = match &script.udp_response {
        Some((status, fields)) => (
            *status,
            fields
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect(),
        ),
        None => (200, vec![("capsule-protocol", "?1")]),
    };
    respond(&mut stream, status, &fields, script.grease).await?;
    if script.udp_bad_capsule {
        stream.send_data(&capsule(DATAGRAM_CAPSULE, b"")).await?;
    }
    if script.udp_end_at_once {
        stream.finish();
        return Ok(());
    }
    let (mut from_client, mut to_client) = tokio::io::split(stream);
    let up = async {
        while let Some(datagram) = datagrams.recv().await {
            // the client sends none without a context id
            let Some((context, n)) = parse_varint(&datagram) else {
                continue;
            };
            shared
                .datagrams
                .lock()
                .expect("datagrams")
                .push(RecordedH3Datagram {
                    connection,
                    stream: id,
                    context,
                    len: datagram.len() - n,
                });
            if context == 0 {
                let _ = socket.send(&datagram[n..]).await;
            }
        }
    };
    let down = async {
        let mut buf = vec![0u8; 65536];
        loop {
            let reply = match socket.recv(&mut buf).await {
                Ok(n) => &buf[..n],
                // an ICMP "unreachable" for an earlier datagram (Windows)
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                Err(_) => return,
            };
            if !script.udp_capsules {
                let mut datagram = Vec::with_capacity(16 + reply.len());
                put_varint(&mut datagram, id / 4);
                put_varint(&mut datagram, 0);
                datagram.extend_from_slice(reply);
                // what QUIC cannot take now is lost, as on any hop
                let _ = quic.send_datagram(datagram.into());
                continue;
            }
            let mut capsules = capsule(RESERVED_CAPSULE, GREASE);
            for context in [2, 0] {
                let mut value = vec![context];
                value.extend_from_slice(reply);
                capsules.extend(capsule(DATAGRAM_CAPSULE, &value));
            }
            // capsules run on from one DATA frame to the next (RFC 9297 3.1)
            let (first, second) = capsules.split_at(capsules.len() / 2);
            for part in [first, second] {
                if to_client.write_all(&frame(DATA, part)).await.is_err() {
                    return;
                }
            }
        }
    };
    let end = async {
        // nothing comes on the stream but its end
        let fin = loop {
            match read_data(&mut from_client).await {
                Ok(Some(_)) => {}
                Ok(None) => break true,
                Err(_) => break false,
            }
        };
        shared
            .udp_ends
            .lock()
            .expect("udp ends")
            .push(RecordedUdpEnd {
                connection,
                stream: id,
                fin,
            });
    };
    // the socket lives as long as the stream (RFC 9298 3.1)
    tokio::select! {
        () = up => {}
        () = down => {}
        () = end => {}
    }
    Ok(())
}

/// The target in `/.well-known/masque/udp/{host}/{port}/`, the default
/// template's path (RFC 9298 2): the host percent-decoded, and the port.
fn masque_target(path: &str) -> Option<(String, u16)> {
    let rest = path
        .strip_prefix("/.well-known/masque/udp/")?
        .strip_suffix('/')?;
    let (host, port) = rest.split_once('/')?;
    let mut decoded = Vec::new();
    let mut bytes = host.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            decoded.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            decoded.push(b);
        }
    }
    Some((String::from_utf8(decoded).ok()?, port.parse().ok()?))
}

/// A loopback UDP socket that sends to `to` and hears `to` alone.
async fn udp_socket(to: SocketAddr) -> io::Result<UdpSocket> {
    let local = if to.is_ipv4() {
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
    } else {
        SocketAddr::from((Ipv6Addr::LOCALHOST, 0))
    };
    let socket = UdpSocket::bind(local).await?;
    socket.connect(to).await?;
    Ok(socket)
}

/// The payload of the client's next DATA frame on a tunnel, frames of
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
    Ok(Some(value))
}

/// `value` as a QUIC integer, as short as it goes.
```

换成

```rust
    Ok(Some(value))
}

/// The QUIC integer at the start of `bytes`, and its length; `None` when
/// `bytes` ends within it.
fn parse_varint(bytes: &[u8]) -> Option<(u64, usize)> {
    let len = 1 << (bytes.first()? >> 6);
    let bytes = bytes.get(..len)?;
    let value = bytes[1..]
        .iter()
        .fold(u64::from(bytes[0] & 0x3f), |value, &b| {
            value << 8 | u64::from(b)
        });
    Some((value, len))
}

/// `value` as a QUIC integer, as short as it goes.
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
    out.extend_from_slice(payload);
    out
}
```

换成

```rust
    out.extend_from_slice(payload);
    out
}

/// A capsule (RFC 9297 3.2): its type, its length and its value, as a
/// frame goes.
fn capsule(ty: u64, value: &[u8]) -> Vec<u8> {
    frame(ty, value)
}
```

`crates/rurge-proto-quic/src/testing/mod.rs`——把

```rust
pub use h3proxy::{FakeH3Proxy, H3ProxyScript, RecordedH3Request};
```

换成

```rust
pub use h3proxy::{
    FakeH3Proxy, H3ProxyScript, RecordedH3Datagram, RecordedH3Request, RecordedUdpEnd,
};
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

```rust
//!   and masque-server answers it 404.
```

换成

```rust
//!   and masque-server answers it 404.
//! - UDP goes as CONNECT-UDP (RFC 9298), a stream of its own for each
//!   target, the packets in HTTP/3 datagrams (`udp`).
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

```rust
mod tcp;

use crate::session::{Dialer, Protocol, Session};
```

换成

```rust
mod tcp;
mod udp;

use crate::session::{Dialer, Failure, Lease, Protocol, Session};
use http::StatusCode;
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

```rust
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_net::quic::h3;
use rurge_proto::http::basic_authorization;
use rurge_proto::{BuildError, Outbound, OutboundError};
use rustls::RootCertStore;
use std::sync::Arc;
```

换成

```rust
use rurge_net::connector::{BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, Target};
use rurge_net::quic::h3;
use rurge_proto::build::tls_name;
use rurge_proto::http::{basic_authorization, wire_host};
use rurge_proto::{BuildError, Outbound, OutboundError, UdpSupport};
use rustls::RootCertStore;
use std::io;
use std::sync::Arc;
use udp::MasqueUdp;
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

```rust
    session: Session<Http3>,
    /// `proxy-authorization`, marked sensitive: QPACK never indexes it.
    authorization: Option<HeaderValue>,
```

换成

```rust
    /// Shared with its UDP carriers, which may outlive it.
    session: Arc<Session<Http3>>,
    /// `proxy-authorization`, marked sensitive: QPACK never indexes it.
    authorization: Option<HeaderValue>,
    /// CONNECT-UDP's `:authority`: the server as its TLS names it, and the
    /// policy's port.
    udp_authority: String,
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

```rust
    ) -> Result<MasqueOutbound, BuildError> {
```

换成

```rust
    ) -> Result<MasqueOutbound, BuildError> {
        // CONNECT-UDP names the server itself, as its TLS does (RFC 9298
        // 3.4, phase 2 M6 design 18)
        let named = Target::new(tls_name(Some(&spec.tls), &server.host), server.port);
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

```rust
        )?;
```

换成

```rust
        )?;
        // an address, or a name TLS took: letters, digits, `-`, `_` and dots
        let host = wire_host(&named).expect("a name TLS takes goes on the wire");
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

```rust
            session: Session::new(name, dialer, Http3),
            authorization,
        })
```

换成

```rust
            session: Arc::new(Session::new(name, dialer, Http3)),
            authorization,
            udp_authority: format!("{host}:{}", named.port),
        })
    }
}

/// How a request failed before its response came: `Lost` when its
/// connection turned out gone or going away, else final. The error says
/// its layer (`h3: …`, `quic: …`).
fn failure(lease: &Lease<Connected>, e: io::Error) -> Failure {
    if lease.connection().close_reason().is_some() || lease.state().h3.is_going_away() {
        Failure::Lost(e.into())
    } else {
        Failure::Failed(e.into())
    }
}

fn proxy(text: &str) -> OutboundError {
    OutboundError::Proxy(format!("masque: {text}"))
}

/// The proxy's refusal, a final status other than 2xx: 407 is a refused
/// login, any other is quoted by its number.
fn refused(status: StatusCode) -> OutboundError {
    match status {
        StatusCode::PROXY_AUTHENTICATION_REQUIRED => proxy("proxy authentication required"),
        status => proxy(&format!("the proxy answered {}", status.as_u16())),
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

```rust
    /// CONNECT waits for nothing of the server's.
```

换成

```rust
    /// CONNECT waits for nothing of the server's, a CONNECT-UDP for its
    /// SETTINGS alone (`udp`).
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

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

    /// Always (manual): UDP goes as CONNECT-UDP. A server that does not
    /// support it fails the datagrams, not the carrier (`udp`).
    fn udp(&self) -> UdpSupport {
        UdpSupport::Native
    }

    /// Nothing is opened yet: each target gets its stream with its first
    /// datagram.
    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedPacketSocket, OutboundError>> {
        let udp = MasqueUdp::new(
            self.session.clone(),
            self.udp_authority.clone(),
            self.authorization.clone(),
            opts.clone(),
            udp::IDLE,
        );
        Box::pin(std::future::ready(Ok(Box::new(udp) as BoxedPacketSocket)))
    }
}

#[cfg(test)]
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

```rust
    const BOUND: Duration = Duration::from_secs(10);

    /// The user `u` with the password `p`.
    fn users() -> Vec<(String, String)> {
```

换成

```rust
    pub(super) const BOUND: Duration = Duration::from_secs(10);

    /// The user `u` with the password `p`.
    pub(super) fn users() -> Vec<(String, String)> {
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

```rust
    fn spec(port: u16, params: &str) -> MasqueSpec {
```

换成

```rust
    pub(super) fn spec(port: u16, params: &str) -> MasqueSpec {
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

```rust
    fn outbound(fake: &FakeH3Proxy, params: &str) -> MasqueOutbound {
```

换成

```rust
    pub(super) fn outbound(fake: &FakeH3Proxy, params: &str) -> MasqueOutbound {
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

```rust
    fn target(addr: SocketAddr) -> Target {
```

换成

```rust
    pub(super) fn target(addr: SocketAddr) -> Target {
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

```rust
    async fn tunnel(out: &MasqueOutbound, to: &Target) -> BoxedStream {
```

换成

```rust
    pub(super) async fn tunnel(out: &MasqueOutbound, to: &Target) -> BoxedStream {
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

```rust
    async fn round_trip(stream: &mut BoxedStream, payload: &[u8]) {
```

换成

```rust
    pub(super) async fn round_trip(stream: &mut BoxedStream, payload: &[u8]) {
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

```rust
    async fn closed_by_the_client(connection: &Connection) {
```

换成

```rust
    pub(super) async fn closed_by_the_client(connection: &Connection) {
```

`crates/rurge-proto-quic/src/masque/tcp.rs`——把

```rust
use super::Connected;
use crate::session::{Failure, Lease};
use http::header::{HeaderValue, PROXY_AUTHORIZATION};
use http::{Request, StatusCode};
```

换成

```rust
use super::{Connected, failure, proxy, refused};
use crate::session::{Failure, Lease};
use http::Request;
use http::header::{HeaderValue, PROXY_AUTHORIZATION};
```

`crates/rurge-proto-quic/src/masque/tcp.rs`——把

```rust
        StatusCode::PROXY_AUTHENTICATION_REQUIRED => {
            Err(Failure::Failed(proxy("proxy authentication required")))
        }
        status => Err(Failure::Failed(proxy(&format!(
            "the proxy answered {}",
            status.as_u16()
        )))),
    }
}

/// How a request failed before its response came: `Lost` when its
/// connection turned out gone or going away, else final. The error says
/// its layer (`h3: …`, `quic: …`).
fn failure(lease: &Lease<Connected>, e: io::Error) -> Failure {
    if lease.connection().close_reason().is_some() || lease.state().h3.is_going_away() {
        Failure::Lost(e.into())
    } else {
        Failure::Failed(e.into())
    }
}

fn proxy(text: &str) -> OutboundError {
    OutboundError::Proxy(format!("masque: {text}"))
```

换成

```rust
        status => Err(Failure::Failed(refused(status))),
    }
```

新建 `crates/rurge-proto-quic/src/masque/udp.rs`：

```rust
//! UDP over MASQUE (manual: Policies › MASQUE; phase 2 M7 design 7.1):
//! CONNECT-UDP (RFC 9298) on the policy's QUIC connection, a request
//! stream of its own for each target, the packets in HTTP/3 datagrams (RFC
//! 9297 2.1).
//!
//! - The request is an extended CONNECT (RFC 9220): `:protocol
//!   connect-udp`, `:scheme https`, `:authority` the server as its TLS
//!   names it, `:path` by the default template
//!   (`/.well-known/masque/udp/{host}/{port}/`), then `proxy-authorization`
//!   when the policy has credentials, and `capsule-protocol: ?1` (RFC 9297
//!   3.4). It goes once the server's SETTINGS allow extended CONNECT and
//!   HTTP Datagrams and the connection carries QUIC datagrams (the manual:
//!   "The server must advertise support for extended CONNECT and HTTP
//!   Datagrams"); without them the datagram fails, and the connection goes
//!   on carrying TCP.
//! - Nothing goes before the proxy's 2xx, which must be one the Capsule
//!   Protocol allows (RFC 9297 3.2): without content, and none of 204, 205
//!   and 206.
//! - A stream is bound to its target, so every packet it brings back counts
//!   as the target's (symmetric, as `h2-connect`'s). Each packet goes in one
//!   HTTP/3 datagram, `varint(0) ‖ payload` (context id 0); one longer than
//!   a datagram of the connection carries now is refused — no DATAGRAM
//!   capsule takes it instead (RFC 9297 3.5, RFC 9298 6.1; masque-server
//!   reads none on HTTP/3).
//! - Coming back, the stream's HTTP/3 datagrams and the DATAGRAM capsules of
//!   its data alike are the target's packets when their context id is 0;
//!   other contexts and capsule types are passed over (RFC 9298 4, RFC 9297
//!   3.2), and a capsule that is malformed ends the stream.
//! - A target's stream is opened with its first datagram, once for those
//!   that come meanwhile, and closes gracefully (FIN) after `IDLE` without
//!   a datagram either way, or once the server ended its side. Once its
//!   connection takes nothing new — ended, going away, or dialled another
//!   way than the policy leads now (`Session::serves`: where the policy
//!   leads is asked at most once a second) —, the target's next datagram
//!   opens it anew on the session's connection. A datagram that meets its
//!   stream ended — the one opened for it, or for those it waited with —
//!   opens one anew, once, then fails: a proxy may end every stream at
//!   once.

use super::{Connected, Http3, failure, proxy, refused};
use crate::session::{Failure, Lease, Protocol, Session};
use crate::varint;
use bytes::Bytes;
use http::header::{
    CONTENT_LENGTH, CONTENT_TYPE, HeaderName, HeaderValue, PROXY_AUTHORIZATION, TRANSFER_ENCODING,
};
use http::{Request, Response};
use quinn::Connection;
use rurge_net::BoxFuture;
use rurge_net::connector::{ConnectOpts, PacketSocket, Target};
use rurge_net::quic::h3::{
    self, Datagrams, RequestStream, SETTINGS_ENABLE_CONNECT_PROTOCOL, SETTINGS_H3_DATAGRAM,
    Settings,
};
use rurge_proto::OutboundError;
use rurge_proto::connect_udp::{self, MAX_PAYLOAD, masque_path};
use std::collections::HashMap;
use std::io;
use std::pin::pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::sync::{OnceCell, mpsc};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;

/// Packets waiting for the engine, per carrier; more are dropped, as a
/// socket would.
pub(super) const INBOX: usize = 64;
/// A target's stream closes after this long without a datagram either way:
/// the engine's flow idle (M5-D7), as `h2-connect`'s.
pub(super) const IDLE: Duration = Duration::from_secs(60);
/// How often a carrier asks where the policy leads (`Session::serves`): it
/// can cost a group's resolution, and every datagram would ask.
const ROUTE_EVERY: Duration = Duration::from_secs(1);
/// `capsule-protocol` (RFC 9297 3.4).
const CAPSULE_PROTOCOL: HeaderName = HeaderName::from_static("capsule-protocol");

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A packet for the engine, and where it came from.
type Packet = (Bytes, Target);

/// A target's stream, opened once for the datagrams that come meanwhile.
type Slot = Arc<OnceCell<Arc<Stream>>>;

type Streams = Mutex<HashMap<Target, Slot>>;

/// The CONNECT-UDP request for `target` (RFC 9298 3.4), to `authority`:
/// refused, nothing opened, for a name that cannot be sent.
fn request(
    authority: &str,
    target: &Target,
    authorization: Option<&HeaderValue>,
) -> io::Result<Request<()>> {
    let unsendable = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "masque: the host name cannot be sent to the server",
        )
    };
    let path = masque_path(target).ok_or_else(unsendable)?;
    let mut request = Request::connect(format!("https://{authority}{path}"))
        .body(())
        // a name longer than a URI may be
        .map_err(|_| unsendable())?;
    request.extensions_mut().insert(h3::Protocol::CONNECT_UDP);
    let headers = request.headers_mut();
    if let Some(authorization) = authorization {
        headers.insert(PROXY_AUTHORIZATION, authorization.clone());
    }
    headers.insert(CAPSULE_PROTOCOL, HeaderValue::from_static("?1"));
    Ok(request)
}

/// Whether the server takes CONNECT-UDP with HTTP Datagrams: extended
/// CONNECT and `H3_DATAGRAM = 1` in its SETTINGS (RFC 9220 3, RFC 9297
/// 2.1.1), and QUIC datagrams on the connection (RFC 9221 3).
fn supports_connect_udp(settings: &Settings, connection: &Connection) -> bool {
    settings.get(SETTINGS_ENABLE_CONNECT_PROTOCOL) == Some(1)
        && settings.get(SETTINGS_H3_DATAGRAM) == Some(1)
        && connection.max_datagram_size().is_some()
}

/// Whether a 2xx breaks the Capsule Protocol's rules (RFC 9297 3.2): a
/// response with content — `content-length`, `content-type` or
/// `transfer-encoding` — or a 204, 205 or 206 is malformed.
fn breaks_capsule_protocol(response: &Response<()>) -> bool {
    matches!(response.status().as_u16(), 204..=206)
        || [CONTENT_LENGTH, CONTENT_TYPE, TRANSFER_ENCODING]
            .iter()
            .any(|name| response.headers().contains_key(name))
}

/// What a CONNECT-UDP request opened.
struct Opened {
    lease: Lease<Connected>,
    stream: RequestStream,
    datagrams: Datagrams,
}

/// The CONNECT-UDP `request` on the connection of `lease`, once the
/// server's SETTINGS allow it, and the proxy's response. Until the response
/// came, a request that finds its connection gone or going away is `Lost`,
/// as a TCP one (`tcp::open`).
async fn connect(lease: Lease<Connected>, request: Request<()>) -> Result<Opened, Failure> {
    let h3 = &lease.state().h3;
    let settings = h3.settings().await.map_err(|e| failure(&lease, e))?;
    if !supports_connect_udp(&settings, lease.connection()) {
        return Err(Failure::Failed(proxy(
            "the server does not support CONNECT-UDP",
        )));
    }
    let mut stream = h3.request(request).await.map_err(|e| failure(&lease, e))?;
    // taken before the response: what the proxy sends right after its 2xx
    // is not lost
    let datagrams = stream.datagrams().await.map_err(|e| failure(&lease, e))?;
    let response = stream.response().await.map_err(|e| failure(&lease, e))?;
    let status = response.status();
    if !status.is_success() {
        return Err(Failure::Failed(refused(status)));
    }
    if breaks_capsule_protocol(&response) {
        return Err(Failure::Failed(proxy(
            "the proxy's response to CONNECT-UDP is malformed",
        )));
    }
    Ok(Opened {
        lease,
        stream,
        datagrams,
    })
}

/// A target's stream, as the carrier's datagrams take it.
struct Stream {
    /// Tells it from a stream that took its place in the table.
    id: u64,
    /// The connection it is on (`Session::serves`), kept while it lives.
    lease: Lease<Connected>,
    datagrams: Arc<Datagrams>,
    /// The last datagram either way: the idle clock.
    last: Arc<Mutex<Instant>>,
    /// Its reading ended: so did the stream.
    ended: CancellationToken,
    /// Reads the stream (`read`); ends with it.
    _reader: AbortOnDropHandle<()>,
}

/// Why a datagram did not go.
enum Unsent {
    /// It is longer than a datagram of the stream carries now: this long.
    TooLong(usize),
    /// The stream, or its connection, ended.
    Ended,
}

impl Stream {
    /// The longest UDP payload a datagram of the stream carries now: what
    /// its HTTP/3 datagram takes past the context id, at most
    /// `MAX_PAYLOAD`.
    fn room(&self) -> usize {
        // context id 0 takes one byte
        self.datagrams
            .max_payload()
            .saturating_sub(1)
            .min(MAX_PAYLOAD)
    }

    /// `payload` in one HTTP/3 datagram with context id 0 (RFC 9298 5),
    /// never waiting: QUIC sends it when it can, or drops it as any hop
    /// may.
    fn send(&self, payload: &[u8]) -> Result<(), Unsent> {
        if self.ended.is_cancelled() {
            return Err(Unsent::Ended);
        }
        let room = self.room();
        if payload.len() > room {
            return Err(Unsent::TooLong(room));
        }
        let mut datagram = Vec::with_capacity(1 + payload.len());
        // context id 0
        datagram.push(0);
        datagram.extend_from_slice(payload);
        match self.datagrams.send(&datagram) {
            Ok(()) => {
                *self.last.lock().expect("clock") = Instant::now();
                Ok(())
            }
            // the path narrowed since `room` was taken
            Err(e) if e.kind() == io::ErrorKind::InvalidInput => Err(Unsent::TooLong(self.room())),
            // its datagrams flow no more: the stream, or its connection,
            // ended
            Err(_) => Err(Unsent::Ended),
        }
    }
}

fn too_long(max: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("masque: a datagram longer than {max} bytes"),
    )
}

/// One client association's UDP through the outbound (design 7.1): a
/// stream of its own for each target, on the policy's connection. No
/// `Debug`: it holds the credentials.
pub(super) struct MasqueUdp {
    session: Arc<Session<Http3>>,
    /// The server's own `:authority`.
    authority: String,
    /// `proxy-authorization`, marked sensitive.
    authorization: Option<HeaderValue>,
    /// The budget of opening a stream, a dial included.
    opts: ConnectOpts,
    idle: Duration,
    streams: Arc<Streams>,
    /// When the policy's route was last compared (`route_due`).
    route_checked: Mutex<Option<Instant>>,
    /// What the streams' readers fill `inbox` through.
    answers: mpsc::Sender<Packet>,
    inbox: tokio::sync::Mutex<mpsc::Receiver<Packet>>,
}

impl MasqueUdp {
    /// A carrier that opens nothing yet: each target's stream comes with
    /// its first datagram.
    pub(super) fn new(
        session: Arc<Session<Http3>>,
        authority: String,
        authorization: Option<HeaderValue>,
        opts: ConnectOpts,
        idle: Duration,
    ) -> MasqueUdp {
        let (answers, inbox) = mpsc::channel(INBOX);
        MasqueUdp {
            session,
            authority,
            authorization,
            opts,
            idle,
            streams: Arc::default(),
            route_checked: Mutex::default(),
            answers,
            inbox: tokio::sync::Mutex::new(inbox),
        }
    }

    /// `to`'s slot: the table's, else a new one put there.
    fn slot(&self, to: &Target) -> Slot {
        let mut streams = self.streams.lock().expect("streams");
        streams.entry(to.clone()).or_default().clone()
    }

    /// Forgets `slot` for `to`, unless another has taken its place.
    fn forget(&self, to: &Target, slot: &Slot) {
        let forgotten = {
            let mut streams = self.streams.lock().expect("streams");
            match streams.get(to) {
                Some(s) if Arc::ptr_eq(s, slot) => streams.remove(to),
                _ => None,
            }
        };
        // its stream goes once nothing else holds it, out of the lock
        drop(forgotten);
    }

    /// Whether `stream` takes the target's next datagram: its reading goes
    /// on, and its connection takes what is new (`Session::serves`) — where
    /// the policy leads is asked at most once a second, the rest every
    /// time.
    fn serves(&self, stream: &Stream) -> bool {
        if stream.ended.is_cancelled() {
            return false;
        }
        if self.route_due() {
            self.session.serves(&stream.lease)
        } else {
            self.session.serves_without_route(&stream.lease)
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

    /// `to`'s stream: the one it has while that takes the datagram, else
    /// one opened now, once for every datagram to `to` meanwhile. One
    /// opened for the datagram — or for those it waited with — and found
    /// ended already is opened anew once (`reopen`).
    async fn stream(&self, to: &Target, reopened: &mut bool) -> io::Result<Arc<Stream>> {
        loop {
            let slot = self.slot(to);
            if let Some(stream) = slot.get() {
                if self.serves(stream) {
                    return Ok(stream.clone());
                }
                if !stream.ended.is_cancelled() {
                    tracing::debug!("masque: a UDP stream moves to a new connection");
                }
                // the target's next stream goes on the session's connection
                self.forget(to, &slot);
                continue;
            }
            let stream = match slot.get_or_try_init(|| self.open(to)).await {
                Ok(stream) => stream.clone(),
                Err(e) => {
                    self.forget(to, &slot);
                    return Err(e);
                }
            };
            // A sender waiting on a cell whose opener failed opens again,
            // after that opener forgot the slot: the stream must be the
            // table's, or the next datagram to `to` opens another.
            let mut streams = self.streams.lock().expect("streams");
            // looked at under the lock its reader leaves the table with: an
            // ended stream is never put back
            if stream.ended.is_cancelled() {
                reopen(reopened)?;
                continue;
            }
            match streams.get(to) {
                None => {
                    streams.insert(to.clone(), slot.clone());
                    return Ok(stream);
                }
                Some(s) if Arc::ptr_eq(s, &slot) => return Ok(stream),
                // another slot took its place: use that one
                Some(_) => {}
            }
        }
    }

    /// A stream to `to` on the session's connection, and the task that
    /// reads it: one budget for the connection (when one is dialed), QUIC's
    /// handshake, the server's SETTINGS and the CONNECT-UDP exchange. A
    /// request that finds the connection gone goes again, once, on a new
    /// one (`Session::request`).
    async fn open(&self, to: &Target) -> io::Result<Arc<Stream>> {
        // checked first: nothing is dialed for a name that cannot be sent
        let request = request(&self.authority, to, self.authorization.as_ref())?;
        let opening = self.session.request(&self.opts, |lease| {
            Box::pin(connect(lease, request.clone()))
        });
        let Opened {
            lease,
            stream,
            datagrams,
        } = match tokio::time::timeout(self.opts.timeout, opening).await {
            Ok(Ok(opened)) => opened,
            Ok(Err(e)) => return Err(io::Error::other(e.to_string())),
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    OutboundError::Timeout.to_string(),
                ));
            }
        };
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let datagrams = Arc::new(datagrams);
        let last = Arc::new(Mutex::new(Instant::now()));
        let ended = CancellationToken::new();
        let reader = tokio::spawn({
            let (datagrams, last, ended) = (datagrams.clone(), last.clone(), ended.clone());
            let (answers, to, idle) = (self.answers.clone(), to.clone(), self.idle);
            let streams = Arc::downgrade(&self.streams);
            async move {
                let why = read(stream, &datagrams, &to, &answers, &last, idle).await;
                tracing::debug!("masque: a UDP stream ended: {why}");
                ended.cancel();
                // the table's no more, unless another took its place
                if let Some(streams) = streams.upgrade() {
                    leave(&streams, &to, id);
                }
            }
        });
        Ok(Arc::new(Stream {
            id,
            lease,
            datagrams,
            last,
            ended,
            _reader: AbortOnDropHandle::new(reader),
        }))
    }

    /// Test support: the targets with a stream, or one being opened.
    #[cfg(test)]
    fn tracked(&self) -> usize {
        self.streams.lock().expect("streams").len()
    }

    /// Test support: the longest payload `to`'s stream carries now.
    #[cfg(test)]
    fn room(&self, to: &Target) -> Option<usize> {
        let slot = self.streams.lock().expect("streams").get(to)?.clone();
        slot.get().map(|stream| stream.room())
    }
}

/// A datagram met its stream ended before it could go: it opens one anew,
/// once — a proxy may end every stream at once —, then fails.
fn reopen(reopened: &mut bool) -> io::Result<()> {
    if std::mem::replace(reopened, true) {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "masque: the server ended the UDP stream",
        ));
    }
    Ok(())
}

/// Takes `to`'s stream `id` out of `streams`, unless another has taken its
/// place.
fn leave(streams: &Streams, to: &Target, id: u64) {
    let left = {
        let mut streams = streams.lock().expect("streams");
        let ours = streams
            .get(to)
            .is_some_and(|slot| slot.get().is_some_and(|stream| stream.id == id));
        if ours { streams.remove(to) } else { None }
    };
    // the stream goes once nothing else holds it, out of the lock
    drop(left);
}

/// How reading a stream ended.
enum End {
    /// Without a datagram either way for the idle time, or with the server's
    /// end of its side: the stream closes gracefully.
    Closed(&'static str),
    /// A capsule that is malformed, or the stream's failure.
    Failed(String),
}

/// Reads `to`'s stream until it ends: its HTTP/3 datagrams and the
/// DATAGRAM capsules of its data with context id 0 are `to`'s packets, for
/// `answers` — which drops them when full: the reader waits for nobody.
/// Once the stream idled for `idle`, or the server ended its side, the FIN
/// goes and nothing more is read; a failure leaves the stream to its drop,
/// which cancels it. Says why it ended.
async fn read(
    stream: RequestStream,
    datagrams: &Datagrams,
    to: &Target,
    answers: &mpsc::Sender<Packet>,
    last: &Mutex<Instant>,
    idle: Duration,
) -> String {
    let deliver = |packet: Bytes| {
        *last.lock().expect("clock") = Instant::now();
        let _ = answers.try_send((packet, to.clone()));
    };
    let mut reader = BufReader::new(stream);
    // the datagrams end with the stream or its connection: reading the
    // stream says which
    let mut flowing = true;
    let end = 'stream: loop {
        // a capsule is read whole: neither a datagram nor the idle clock
        // may cut one in two, so the read lives on across them
        let mut capsule = pin!(connect_udp::read_datagram(&mut reader, Http3::NAME));
        loop {
            let deadline = *last.lock().expect("clock") + idle;
            tokio::select! {
                read = &mut capsule => match read {
                    Ok(Some(payload)) => {
                        deliver(payload.into());
                        break;
                    }
                    Ok(None) => break 'stream End::Closed("the server ended it"),
                    // the text names the fault, never the payload
                    Err(e) => break 'stream End::Failed(format!("read error: {e}")),
                },
                datagram = datagrams.recv(), if flowing => match datagram {
                    Some(datagram) => {
                        if let Some(packet) = udp_payload(datagram) {
                            deliver(packet);
                        }
                    }
                    None => flowing = false,
                },
                () = tokio::time::sleep_until(deadline) => {
                    // a datagram may have moved the deadline: look again
                    if *last.lock().expect("clock") + idle <= Instant::now() {
                        break 'stream End::Closed("idle");
                    }
                }
            }
        }
    };
    match end {
        End::Closed(why) => {
            // what the server sends from now on is not read: the stream's
            // drop stops it
            let _ = reader.shutdown().await;
            why.to_string()
        }
        End::Failed(why) => why,
    }
}

/// The UDP payload of an HTTP/3 datagram of the stream: what follows its
/// context id, when that is 0 (RFC 9298 5). `None` for another context —
/// none is registered (RFC 9298 4) — and for a datagram without one.
fn udp_payload(datagram: Bytes) -> Option<Bytes> {
    let Some((context, n)) = varint::get(&datagram) else {
        tracing::trace!("masque: a datagram without a context id is dropped");
        return None;
    };
    (context == 0).then(|| datagram.slice(n..))
}

impl PacketSocket for MasqueUdp {
    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            // checked first: nothing is opened for what never goes
            if buf.len() > MAX_PAYLOAD {
                return Err(too_long(MAX_PAYLOAD));
            }
            let mut reopened = false;
            loop {
                let stream = self.stream(to, &mut reopened).await?;
                match stream.send(buf) {
                    Ok(()) => return Ok(()),
                    Err(Unsent::TooLong(room)) => return Err(too_long(room)),
                    // it ended under the datagram
                    Err(Unsent::Ended) => {
                        leave(&self.streams, to, stream.id);
                        reopen(&mut reopened)?;
                    }
                }
            }
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
    use crate::masque::MasqueOutbound;
    use crate::masque::tests::{
        BOUND, closed_by_the_client, outbound, round_trip, spec, target, tunnel, users,
    };
    use crate::test_log::Log;
    use crate::testing::{
        FakeH3Proxy, H3ProxyScript, RecordedH3Datagram, RecordedH3Request, RecordedUdpEnd,
    };
    use http::Method;
    use rurge_config::HostName;
    use rurge_config::spec::Tristate;
    use rurge_net::connector::{
        BoxedDatagram, BoxedPacketSocket, BoxedStream, Connector, DirectConnector, Route,
        SystemResolve,
    };
    use rurge_proto::http::basic_authorization;
    use rurge_proto::testing::{echo_server, udp_echo_server};
    use rurge_proto::{Outbound, UdpSupport};
    use std::collections::BTreeSet;
    use std::net::SocketAddr;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::Barrier;
    use tokio::task::JoinSet;
    use tokio::time::timeout;

    /// `out`'s carrier, its targets' streams closing after `idle`.
    fn carrier(out: &MasqueOutbound, idle: Duration) -> MasqueUdp {
        MasqueUdp::new(
            out.session.clone(),
            out.udp_authority.clone(),
            out.authorization.clone(),
            ConnectOpts::default(),
            idle,
        )
    }

    async fn udp_carrier(out: &MasqueOutbound) -> BoxedPacketSocket {
        timeout(BOUND, out.open_udp(&ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .expect("a carrier")
    }

    /// The default template's path to `addr`.
    fn path(addr: SocketAddr) -> String {
        format!("/.well-known/masque/udp/{}/{}/", addr.ip(), addr.port())
    }

    /// A CONNECT-UDP to `path` without credentials, as the fake records it.
    fn udp_request(connection: usize, authority: &str, path: &str) -> RecordedH3Request {
        RecordedH3Request {
            connection,
            method: "CONNECT".to_string(),
            protocol: Some("connect-udp".to_string()),
            scheme: Some("https".to_string()),
            authority: Some(authority.to_string()),
            path: Some(path.to_string()),
            headers: vec![("capsule-protocol".to_string(), "?1".to_string())],
        }
    }

    async fn answer(carrier: &dyn PacketSocket) -> (Vec<u8>, Target) {
        let mut buf = vec![0u8; 65536];
        let (n, from) = timeout(BOUND, carrier.recv_from(&mut buf))
            .await
            .expect("an answer within the bound")
            .unwrap();
        (buf[..n].to_vec(), from)
    }

    async fn udp_round_trip(carrier: &dyn PacketSocket, to: &Target, payload: &[u8]) {
        timeout(BOUND, carrier.send_to(payload, to))
            .await
            .expect("sent within the bound")
            .unwrap();
        assert_eq!(answer(carrier).await, (payload.to_vec(), to.clone()));
    }

    /// Nothing comes for a while: a fixed wait, only to see nothing happen.
    async fn no_answer(carrier: &dyn PacketSocket) {
        let mut buf = vec![0u8; 65536];
        let quiet = timeout(Duration::from_millis(300), carrier.recv_from(&mut buf)).await;
        assert!(quiet.is_err(), "{quiet:?}");
    }

    /// How a datagram to `to` fails.
    async fn failure(carrier: &dyn PacketSocket, to: &Target) -> io::Error {
        timeout(BOUND, carrier.send_to(b"x", to))
            .await
            .expect("an outcome within the bound")
            .expect_err("refused")
    }

    /// Sends `payload` to the echo at `to` until it comes back: what goes
    /// out on a connection as it ends is lost.
    async fn until_answered(carrier: &dyn PacketSocket, to: &Target, payload: &[u8]) {
        let deadline = Instant::now() + BOUND;
        let mut buf = vec![0u8; 65536];
        loop {
            assert!(Instant::now() < deadline, "no answer within the bound");
            timeout(BOUND, carrier.send_to(payload, to))
                .await
                .expect("sent within the bound")
                .unwrap();
            let wait = Instant::now() + Duration::from_millis(500);
            while let Ok(got) = tokio::time::timeout_at(wait, carrier.recv_from(&mut buf)).await {
                let (n, _) = got.unwrap();
                if buf[..n] == *payload {
                    return;
                }
            }
        }
    }

    /// Polls `check` until it holds, within the bound.
    async fn eventually(mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + BOUND;
        while !check() {
            assert!(Instant::now() < deadline, "timed out");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// RFC 9298 3.4: an extended CONNECT to the server's own authority,
    /// the target in the default template's path — an IPv6 literal with
    /// `%3A`, a name as its A-labels —, the credentials, marked sensitive,
    /// then `capsule-protocol: ?1`. A name that cannot be sent is refused
    /// as a datagram that cannot go.
    #[test]
    fn a_connect_udp_request_names_its_target_in_the_path() {
        let mut authorization = HeaderValue::from_static("Basic dTpw");
        authorization.set_sensitive(true);
        for (host, path) in [
            (
                HostName::parse("192.0.2.6"),
                "/.well-known/masque/udp/192.0.2.6/53/",
            ),
            (
                HostName::parse("2001:db8::42"),
                "/.well-known/masque/udp/2001%3Adb8%3A%3A42/53/",
            ),
            (
                HostName::Domain("bücher.example".into()),
                "/.well-known/masque/udp/xn--bcher-kva.example/53/",
            ),
        ] {
            let asked = request(
                "proxy.test:443",
                &Target::new(host, 53),
                Some(&authorization),
            )
            .unwrap();
            assert_eq!(asked.method(), Method::CONNECT);
            assert_eq!(
                asked.extensions().get::<h3::Protocol>(),
                Some(&h3::Protocol::CONNECT_UDP)
            );
            assert_eq!(
                asked.uri().to_string(),
                format!("https://proxy.test:443{path}")
            );
            let fields: Vec<(&str, &str)> = asked
                .headers()
                .iter()
                .map(|(name, value)| (name.as_str(), value.to_str().unwrap()))
                .collect();
            assert_eq!(
                fields,
                [
                    ("proxy-authorization", "Basic dTpw"),
                    ("capsule-protocol", "?1")
                ]
            );
            // QPACK never indexes it (RFC 9204 7.1.3)
            assert!(asked.headers()[PROXY_AUTHORIZATION].is_sensitive());
            assert!(!format!("{asked:?}").contains("dTpw"));
        }
        let to = Target::new(HostName::parse("192.0.2.6"), 53);
        let asked = request("[2001:db8::1]:8443", &to, None).unwrap();
        assert_eq!(
            asked.uri().authority().map(|a| a.as_str()),
            Some("[2001:db8::1]:8443")
        );
        assert_eq!(asked.headers().len(), 1);
        // longer than a URI may be
        let long = format!("{}.test", "a".repeat(70_000));
        for name in ["x@blocked.test", "a b.test", "", long.as_str()] {
            let to = Target::new(HostName::Domain(name.into()), 53);
            let err = request("proxy.test:443", &to, None).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(
                err.to_string(),
                "masque: the host name cannot be sent to the server"
            );
        }
    }

    /// RFC 9298 4, 5: a packet is what follows context id 0, written in
    /// any length; another context, or none, carries no packet.
    #[test]
    fn a_packet_follows_context_id_0() {
        let payload = |bytes: &[u8]| udp_payload(Bytes::copy_from_slice(bytes));
        assert_eq!(payload(&[0, b'h', b'i']).as_deref(), Some(&b"hi"[..]));
        assert_eq!(payload(&[0]).as_deref(), Some(&b""[..]));
        // 0 in two bytes: not minimal, still 0
        assert_eq!(payload(&[0x40, 0, b'h', b'i']).as_deref(), Some(&b"hi"[..]));
        for other in [&[2, b'h', b'i'][..], &[0x40, 2, b'x'], &[], &[0x40]] {
            assert_eq!(payload(other), None, "{other:02x?}");
        }
    }

    /// RFC 9297 3.2: a 2xx that starts the Capsule Protocol has no content
    /// and is none of 204, 205 and 206.
    #[test]
    fn a_2xx_with_content_or_one_of_204_205_206_breaks_the_capsule_protocol() {
        let response = |status: u16, fields: &[(&str, &str)]| {
            let mut response = Response::builder().status(status);
            for &(name, value) in fields {
                response = response.header(name, value);
            }
            response.body(()).unwrap()
        };
        for fine in [
            response(200, &[]),
            response(200, &[("capsule-protocol", "?1")]),
            response(201, &[("proxy-status", "masque")]),
        ] {
            assert!(!breaks_capsule_protocol(&fine), "{fine:?}");
        }
        for broken in [
            response(200, &[("content-length", "0")]),
            response(200, &[("content-type", "application/octet-stream")]),
            response(200, &[("transfer-encoding", "chunked")]),
            response(204, &[]),
            response(205, &[]),
            response(206, &[("capsule-protocol", "?1")]),
        ] {
            assert!(breaks_capsule_protocol(&broken), "{broken:?}");
        }
    }

    /// Design 7.1: a target's first datagram opens its CONNECT-UDP stream
    /// — nothing before —, each target a stream of its own on the one
    /// connection, which TCP shares. Every packet goes in one HTTP/3
    /// datagram with context id 0, and each answer is its stream's
    /// target's.
    #[tokio::test]
    async fn udp_goes_in_http3_datagrams_a_stream_for_each_target() {
        let (one, two) = (udp_echo_server().await, udp_echo_server().await);
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        assert_eq!(out.udp(), UdpSupport::Native);
        let carrier = udp_carrier(&out).await;
        assert_eq!(fake.connections(), 0, "nothing is opened before a datagram");
        udp_round_trip(carrier.as_ref(), &target(one), b"to one").await;
        udp_round_trip(carrier.as_ref(), &target(two), b"to two").await;
        udp_round_trip(carrier.as_ref(), &target(one), b"one again").await;
        udp_round_trip(carrier.as_ref(), &target(one), b"").await;
        let authority = fake.addr().to_string();
        assert_eq!(
            fake.requests(),
            [
                udp_request(0, &authority, &path(one)),
                udp_request(0, &authority, &path(two))
            ]
        );
        let datagram = |stream, len| RecordedH3Datagram {
            connection: 0,
            stream,
            context: 0,
            len,
        };
        assert_eq!(
            fake.datagrams(),
            [
                datagram(0, 6),
                datagram(4, 6),
                datagram(0, 9),
                datagram(0, 0)
            ]
        );
        let mut stream = tunnel(&out, &target(echo_server().await)).await;
        round_trip(&mut stream, b"tcp beside").await;
        assert_eq!(fake.connections(), 1);
    }

    /// RFC 9298 2, 3: a name goes in the path as its A-labels, an IPv6
    /// literal with `%3A` for its colons; the answers count as the target's
    /// asked for (symmetric). `:authority` names the server as its TLS
    /// does: by the policy's `sni`, here. A name that cannot be sent is
    /// refused, and nothing goes out.
    #[tokio::test]
    async fn targets_go_in_the_path_and_the_server_by_its_tls_name() {
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            connect_to: Some(echo),
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "sni=localhost");
        let carrier = udp_carrier(&out).await;
        let authority = format!("localhost:{}", fake.addr().port());
        for (host, path) in [
            (
                HostName::Domain("bücher.example".into()),
                "/.well-known/masque/udp/xn--bcher-kva.example/53/",
            ),
            (
                HostName::parse("2001:db8::42"),
                "/.well-known/masque/udp/2001%3Adb8%3A%3A42/53/",
            ),
        ] {
            udp_round_trip(carrier.as_ref(), &Target::new(host, 53), b"q").await;
            assert_eq!(
                fake.requests().pop(),
                Some(udp_request(0, &authority, path))
            );
        }
        for name in ["x@blocked.test", "a b.test", ""] {
            let err = failure(
                carrier.as_ref(),
                &Target::new(HostName::Domain(name.into()), 53),
            )
            .await;
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(
                err.to_string(),
                "masque: the host name cannot be sent to the server"
            );
        }
        assert_eq!(fake.requests().len(), 2);
    }

    /// Every CONNECT-UDP carries the credentials, as every CONNECT does. A
    /// refusal fails the datagram — and its flow: no other datagram would
    /// fare better — with the texts of TCP: 407, then any other status by
    /// its number, masque-server's 502 for a target it cannot reach among
    /// them; the next datagram asks again.
    #[tokio::test]
    async fn every_connect_udp_carries_the_credentials_and_a_refusal_is_quoted() {
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            users: users(),
            ..H3ProxyScript::default()
        });
        let carrier = udp_carrier(&outbound(&fake, "username=u, password=p")).await;
        udp_round_trip(carrier.as_ref(), &target(echo), b"let in").await;
        // base64("u:p"), then the capsule protocol
        assert_eq!(
            fake.requests()[0].headers,
            [
                ("proxy-authorization".to_string(), "Basic dTpw".to_string()),
                ("capsule-protocol".to_string(), "?1".to_string())
            ]
        );
        let wrong = udp_carrier(&outbound(&fake, "username=u, password=wrong")).await;
        let none = udp_carrier(&outbound(&fake, "")).await;
        // a name, which the fake never resolves
        let name = Target::new(HostName::parse("remote.example"), 53);
        for (carrier, to, refusal) in [
            (
                &wrong,
                target(echo),
                "masque: proxy authentication required",
            ),
            (&none, target(echo), "masque: proxy authentication required"),
            (&carrier, name, "masque: the proxy answered 502"),
        ] {
            for _ in 0..2 {
                let err = failure(carrier.as_ref(), &to).await;
                assert_ne!(err.kind(), io::ErrorKind::InvalidInput, "{err}");
                assert_eq!(err.to_string(), refusal);
            }
        }
        assert_eq!(fake.requests().len(), 7, "each datagram asked again");
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            refuse: Some(403),
            ..H3ProxyScript::default()
        });
        let carrier = udp_carrier(&outbound(&fake, "")).await;
        let err = failure(carrier.as_ref(), &target(echo)).await;
        assert_eq!(err.to_string(), "masque: the proxy answered 403");
    }

    /// RFC 9297 3.2, RFC 9298 3.5: a 2xx with content, or a 204, 205 or
    /// 206, is no answer to CONNECT-UDP; the connection goes on.
    #[tokio::test]
    async fn a_2xx_that_breaks_the_capsule_protocol_is_refused() {
        let (udp, tcp) = (udp_echo_server().await, echo_server().await);
        for (status, fields) in [
            (200, &[("content-length", "0")][..]),
            (
                200,
                &[
                    ("capsule-protocol", "?1"),
                    ("content-type", "application/octet-stream"),
                ],
            ),
            (204, &[]),
            (205, &[]),
            (206, &[("capsule-protocol", "?1")]),
        ] {
            let fields = fields
                .iter()
                .map(|&(name, value)| (name.to_string(), value.to_string()))
                .collect();
            let fake = FakeH3Proxy::spawn(H3ProxyScript {
                udp_response: Some((status, fields)),
                ..H3ProxyScript::default()
            });
            let out = outbound(&fake, "");
            let carrier = udp_carrier(&out).await;
            let err = failure(carrier.as_ref(), &target(udp)).await;
            assert_eq!(
                err.to_string(),
                "masque: the proxy's response to CONNECT-UDP is malformed",
                "{status}"
            );
            let mut stream = tunnel(&out, &target(tcp)).await;
            round_trip(&mut stream, b"tcp still").await;
            assert_eq!(fake.connections(), 1);
        }
    }

    /// The manual: "The server must advertise support for extended CONNECT
    /// and HTTP Datagrams". Without extended CONNECT, with `H3_DATAGRAM =
    /// 0`, or without QUIC datagrams, a datagram fails, and no request goes
    /// out; the connection carries TCP all the same.
    #[tokio::test]
    async fn a_server_without_connect_udp_fails_the_datagrams_and_carries_tcp() {
        let (udp, tcp) = (udp_echo_server().await, echo_server().await);
        for script in [
            H3ProxyScript {
                no_extended_connect: true,
                ..H3ProxyScript::default()
            },
            H3ProxyScript {
                no_h3_datagram: true,
                ..H3ProxyScript::default()
            },
            H3ProxyScript {
                no_datagrams: true,
                ..H3ProxyScript::default()
            },
        ] {
            let fake = FakeH3Proxy::spawn(script.clone());
            let out = outbound(&fake, "");
            let carrier = udp_carrier(&out).await;
            for _ in 0..2 {
                let err = failure(carrier.as_ref(), &target(udp)).await;
                assert_ne!(err.kind(), io::ErrorKind::InvalidInput, "{script:?}");
                assert_eq!(
                    err.to_string(),
                    "masque: the server does not support CONNECT-UDP",
                    "{script:?}"
                );
            }
            assert!(fake.requests().is_empty(), "{script:?}");
            let mut stream = tunnel(&out, &target(tcp)).await;
            round_trip(&mut stream, b"tcp still").await;
            assert_eq!(fake.connections(), 1, "{script:?}");
        }
    }

    /// RFC 9298 5: at most 65527 bytes with context id 0, and no more than
    /// a datagram of the connection carries now: no DATAGRAM capsule takes
    /// what does not fit. Refused, nothing sent, the datagram alone: the
    /// carrier goes on.
    #[tokio::test]
    async fn a_datagram_too_long_is_refused_and_nothing_goes_out() {
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        let carrier = carrier(&out, IDLE);
        let to = target(echo);
        let err = carrier
            .send_to(&vec![0u8; MAX_PAYLOAD + 1], &to)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            "masque: a datagram longer than 65527 bytes"
        );
        assert_eq!(fake.connections(), 0, "nothing is opened for it");
        udp_round_trip(&carrier, &to, b"first").await;
        // taken and used with nothing awaited between: the path's size of
        // a datagram stays put
        let room = carrier.room(&to).expect("the target's stream");
        assert!((1000..MAX_PAYLOAD).contains(&room), "{room}");
        let err = carrier
            .send_to(&vec![0u8; room + 1], &to)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            format!("masque: a datagram longer than {room} bytes")
        );
        udp_round_trip(&carrier, &to, b"still").await;
        let sent: Vec<usize> = fake.datagrams().iter().map(|d| d.len).collect();
        assert_eq!(sent, [5, 5]);
    }

    /// RFC 9297 3.2, 3.5: the proxy may send a packet in a DATAGRAM capsule
    /// on the stream instead — here across two DATA frames, after a capsule
    /// of a reserved type and a DATAGRAM capsule with context id 2, both
    /// passed over. Each is the target's packet, once.
    #[tokio::test]
    async fn datagram_capsules_on_the_stream_are_packets_and_the_rest_is_passed_over() {
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            udp_capsules: true,
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "");
        let carrier = udp_carrier(&out).await;
        for payload in [&b"first"[..], b"", &[7u8; 1000]] {
            udp_round_trip(carrier.as_ref(), &target(echo), payload).await;
        }
        no_answer(carrier.as_ref()).await;
    }

    /// RFC 9298 4: an HTTP/3 datagram with another context id — none is
    /// registered — or with none at all carries no packet; context id 0
    /// written long is 0 all the same.
    #[tokio::test]
    async fn datagrams_of_other_contexts_carry_no_packet() {
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        let carrier = udp_carrier(&out).await;
        let to = target(echo);
        udp_round_trip(carrier.as_ref(), &to, b"open").await;
        let stream = fake.datagrams()[0].stream;
        for datagram in [
            &[2, b'n', b'o'][..],
            &[],
            &[0x40, 2, b'n', b'o'],
            &[0x40, 0, b'y', b'e', b's'],
            &[0, b'a', b'n', b'd'],
        ] {
            fake.send_datagram(0, stream, datagram);
        }
        assert_eq!(
            answer(carrier.as_ref()).await,
            (b"yes".to_vec(), to.clone())
        );
        assert_eq!(answer(carrier.as_ref()).await, (b"and".to_vec(), to));
        no_answer(carrier.as_ref()).await;
    }

    /// RFC 9297 3.3: a capsule that is malformed — a DATAGRAM capsule with
    /// no room for a context id — ends the stream as it stands, with no
    /// FIN; the target's next datagram opens another.
    #[tokio::test]
    async fn a_malformed_capsule_ends_the_stream() {
        let (log, _logging, _second) = Log::capture();
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            udp_bad_capsule: true,
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "");
        let carrier = carrier(&out, IDLE);
        let to = target(echo);
        timeout(BOUND, carrier.send_to(b"first", &to))
            .await
            .expect("sent within the bound")
            .unwrap();
        eventually(|| carrier.tracked() == 0).await;
        eventually(|| !fake.udp_ends().is_empty()).await;
        let end = RecordedUdpEnd {
            connection: 0,
            stream: 0,
            fin: false,
        };
        assert_eq!(fake.udp_ends(), [end]);
        assert!(
            log.text().contains(
                "masque: a UDP stream ended: read error: masque: a malformed capsule (no context id)"
            ),
            "{}",
            log.text()
        );
        timeout(BOUND, carrier.send_to(b"second", &to))
            .await
            .expect("sent within the bound")
            .unwrap();
        assert_eq!(fake.requests().len(), 2);
    }

    /// A proxy that answers 200 and ends every stream at once. A datagram
    /// that meets its stream ended — the one opened for it, or for those it
    /// waited with — opens one anew, once: met ended too, the datagram
    /// fails, and no third stream goes out. Senders that wait for another's
    /// stream run after its reader saw the end (one thread), and meet it
    /// ended.
    #[tokio::test]
    async fn a_stream_the_server_ends_at_once_is_opened_anew_once_per_datagram() {
        const SENDERS: usize = 8;
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            udp_end_at_once: true,
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "");
        let carrier = Arc::new(carrier(&out, IDLE));
        let to = target(echo);
        let mut failed = 0;
        for round in 0..5 {
            let before = fake.requests().len();
            let mut sending = JoinSet::new();
            for _ in 0..SENDERS {
                let (carrier, to) = (carrier.clone(), to.clone());
                sending.spawn(async move {
                    timeout(BOUND, carrier.send_to(b"x", &to))
                        .await
                        .expect("an outcome within the bound")
                });
            }
            while let Some(sent) = sending.join_next().await {
                if let Err(e) = sent.unwrap() {
                    assert_eq!(e.kind(), io::ErrorKind::ConnectionAborted);
                    assert_eq!(e.to_string(), "masque: the server ended the UDP stream");
                    failed += 1;
                }
            }
            // a stream, and one opened anew for the datagrams that met it
            // ended
            let asked = fake.requests().len() - before;
            assert!((1..=2).contains(&asked), "round {round}: {asked}");
            eventually(|| carrier.tracked() == 0).await;
        }
        assert!(failed > 0, "no datagram met its second stream ended");
    }

    /// Design 7.1: a stream without a datagram either way for the idle time
    /// closes gracefully — a FIN, and nothing more is read — and leaves the
    /// carrier; the target's next datagram opens another, on the same
    /// connection. Answers alone keep a stream open, as datagrams sent do.
    #[tokio::test]
    async fn an_idle_stream_closes_gracefully_and_the_next_datagram_opens_another() {
        const IDLE: Duration = Duration::from_secs(1);
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        let carrier = carrier(&out, IDLE);
        let to = target(echo);
        udp_round_trip(&carrier, &to, b"first").await;
        for _ in 0..8 {
            tokio::time::sleep(IDLE / 5).await;
            fake.send_datagram(0, 0, &[0, b'k']);
            assert_eq!(answer(&carrier).await, (b"k".to_vec(), to.clone()));
        }
        assert_eq!(carrier.tracked(), 1, "kept open by its answers");
        assert!(fake.udp_ends().is_empty());
        eventually(|| carrier.tracked() == 0).await;
        eventually(|| !fake.udp_ends().is_empty()).await;
        let end = RecordedUdpEnd {
            connection: 0,
            stream: 0,
            fin: true,
        };
        assert_eq!(fake.udp_ends(), [end]);
        udp_round_trip(&carrier, &to, b"second").await;
        let carried: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
        assert_eq!(carried, [0, 0]);
    }

    /// Design 3.5: a server going away (an HTTP/3 GOAWAY) takes no new
    /// stream: the target's next datagram opens its stream anew on a new
    /// connection; the one before is let go of, and its connection closed
    /// once nothing holds it.
    #[tokio::test]
    async fn after_a_goaway_the_next_datagram_goes_on_a_new_connection() {
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            goaway_after: Some(1),
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "");
        // held from before the first request, to see its GOAWAY come
        let lease = timeout(BOUND, out.session.lease(&ConnectOpts::default()))
            .await
            .unwrap()
            .unwrap();
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), &target(echo), b"before").await;
        let deadline = Instant::now() + BOUND;
        while out.session.serves(&lease) {
            assert!(Instant::now() < deadline, "no GOAWAY heard");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        drop(lease);
        udp_round_trip(carrier.as_ref(), &target(echo), b"after").await;
        let carried: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
        assert_eq!(carried, [0, 1]);
        closed_by_the_client(&fake.connection(0)).await;
    }

    /// A connection the server closed, or lost in a restart (a stateless
    /// reset answers the next datagram): the target's datagrams go on in a
    /// stream opened anew on a new connection.
    #[tokio::test]
    async fn after_the_connection_ends_the_next_datagrams_go_on_a_new_one() {
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        let carrier = udp_carrier(&out).await;
        let to = target(echo);
        udp_round_trip(carrier.as_ref(), &to, b"first").await;
        fake.close_connections();
        until_answered(carrier.as_ref(), &to, b"after a close").await;
        fake.restart();
        until_answered(carrier.as_ref(), &to, b"after a restart").await;
        let carried: BTreeSet<usize> = fake.requests().iter().map(|r| r.connection).collect();
        assert_eq!(carried, BTreeSet::from([0, 1, 2]));
        assert_eq!(fake.connections(), 3);
    }

    /// Dials directly, leads where it is told to, and counts the times it
    /// is asked where it leads.
    struct Counted {
        inner: DirectConnector,
        route: Mutex<Arc<u8>>,
        asked: AtomicUsize,
    }

    impl Counted {
        fn new() -> Counted {
            Counted {
                inner: DirectConnector::new(Arc::new(SystemResolve)),
                route: Mutex::new(Arc::new(1)),
                asked: AtomicUsize::new(0),
            }
        }

        fn asked(&self) -> usize {
            self.asked.load(Ordering::Relaxed)
        }
    }

    impl Connector for Counted {
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

    /// The outbound to `fake`, its connections dialled through `counted`.
    fn outbound_via(fake: &FakeH3Proxy, counted: Arc<Counted>) -> MasqueOutbound {
        let port = fake.addr().port();
        MasqueOutbound::new(
            "M",
            Target::new(HostName::parse("127.0.0.1"), port),
            &spec(port, ""),
            Tristate::Auto,
            &[],
            fake.roots(),
            counted,
        )
        .unwrap()
    }

    /// Where the policy leads can cost a group's resolution: a carrier asks
    /// at most once a second, however many datagrams it sends meanwhile.
    #[tokio::test]
    async fn the_route_is_compared_at_most_once_a_second() {
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let counted = Arc::new(Counted::new());
        let out = outbound_via(&fake, counted.clone());
        let carrier = udp_carrier(&out).await;
        let to = target(echo);
        udp_round_trip(carrier.as_ref(), &to, b"open").await;
        let before = counted.asked();
        for i in 0..50u8 {
            carrier.send_to(&[i], &to).await.unwrap();
        }
        let asked = counted.asked() - before;
        assert!(asked <= 3, "asked {asked} times for 50 datagrams");
    }

    /// A carrier finds out for itself, at its next comparison of the
    /// route, that the policy leads elsewhere now: the target's next
    /// datagram opens its stream anew on a new connection, dialled the new
    /// way. Until then — a second at most — datagrams go on the stream
    /// before.
    #[tokio::test]
    async fn a_carrier_finds_a_changed_route_by_itself() {
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let counted = Arc::new(Counted::new());
        let out = outbound_via(&fake, counted.clone());
        let carrier = udp_carrier(&out).await;
        let to = target(echo);
        udp_round_trip(carrier.as_ref(), &to, b"first").await;
        *counted.route.lock().unwrap() = Arc::new(2);
        let deadline = Instant::now() + BOUND;
        while !fake.requests().iter().any(|r| r.connection == 1) {
            assert!(
                Instant::now() < deadline,
                "the stream never left its connection"
            );
            carrier.send_to(b"probe", &to).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        until_answered(carrier.as_ref(), &to, b"moved").await;
        assert_eq!(fake.connections(), 2);
    }

    /// A carrier that does not read loses what its inbox cannot hold, and
    /// holds up no other carrier: a stream's reader waits for nobody.
    #[tokio::test]
    async fn a_full_inbox_drops_rather_than_holds_up_the_other_carriers() {
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        let (slow, other) = (udp_carrier(&out).await, udp_carrier(&out).await);
        udp_round_trip(slow.as_ref(), &target(echo), b"slow").await;
        udp_round_trip(other.as_ref(), &target(echo), b"other").await;
        let streams: Vec<u64> = fake.datagrams().iter().map(|d| d.stream).collect();
        assert_eq!(streams, [0, 4]);
        for i in 0..INBOX as u32 + 10 {
            let mut datagram = vec![0];
            datagram.extend_from_slice(&i.to_be_bytes());
            fake.send_datagram(0, 0, &datagram);
        }
        fake.send_datagram(0, 4, b"\0not held up");
        assert_eq!(answer(other.as_ref()).await.0, b"not held up");
        for i in 0..INBOX as u32 {
            assert_eq!(answer(slow.as_ref()).await.0, i.to_be_bytes());
        }
        no_answer(slow.as_ref()).await;
    }

    /// Targets at once, from tasks on threads of their own, two senders to
    /// each: one stream for each target, opened once for both, and each
    /// answer its target's.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn many_targets_at_once_get_a_stream_each() {
        const TARGETS: u8 = 8;
        let mut targets = Vec::new();
        for _ in 0..TARGETS {
            targets.push(target(udp_echo_server().await));
        }
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        let carrier: Arc<dyn PacketSocket> = Arc::from(udp_carrier(&out).await);
        for round in 0..3u8 {
            let start = Arc::new(Barrier::new(2 * usize::from(TARGETS)));
            let mut sending = JoinSet::new();
            for (i, to) in (0..TARGETS).zip(&targets) {
                for sender in 0..2u8 {
                    let (carrier, start, to) = (carrier.clone(), start.clone(), to.clone());
                    sending.spawn(async move {
                        start.wait().await;
                        timeout(BOUND, carrier.send_to(&[round, i, sender], &to))
                            .await
                            .expect("sent within the bound")
                            .unwrap();
                    });
                }
            }
            while let Some(sent) = sending.join_next().await {
                sent.unwrap();
            }
            let mut got = Vec::new();
            for _ in 0..2 * TARGETS {
                let (packet, from) = answer(carrier.as_ref()).await;
                assert_eq!(packet.len(), 3);
                assert_eq!(from, targets[usize::from(packet[1])], "round {round}");
                got.push(packet);
            }
            got.sort();
            let expected: Vec<Vec<u8>> = (0..TARGETS)
                .flat_map(|i| (0..2u8).map(move |sender| vec![round, i, sender]))
                .collect();
            assert_eq!(got, expected, "round {round}");
        }
        assert_eq!(fake.requests().len(), usize::from(TARGETS));
        assert_eq!(fake.connections(), 1);
    }

    /// `opts.timeout` bounds opening a stream: here a proxy that takes the
    /// CONNECT-UDP, on a connection that is up, and never answers it.
    #[tokio::test]
    async fn a_connect_udp_the_proxy_never_answers_times_out() {
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            silent: true,
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "");
        drop(
            timeout(BOUND, out.session.lease(&ConnectOpts::default()))
                .await
                .unwrap()
                .unwrap(),
        );
        let opts = ConnectOpts {
            timeout: Duration::from_millis(500),
        };
        let carrier = timeout(BOUND, out.open_udp(&opts)).await.unwrap().unwrap();
        let started = Instant::now();
        let err = failure(carrier.as_ref(), &target(echo)).await;
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(err.to_string(), "connect timed out");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(fake.requests().len(), 1, "the request went out");
    }

    /// A carrier gone takes its streams with it — cancelled, no FIN — and
    /// its tasks; the connection stays the session's, and carries TCP.
    #[tokio::test]
    async fn a_carrier_gone_ends_its_streams() {
        let (one, two) = (udp_echo_server().await, udp_echo_server().await);
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), &target(one), b"one").await;
        udp_round_trip(carrier.as_ref(), &target(two), b"two").await;
        drop(carrier);
        eventually(|| fake.udp_ends().len() == 2).await;
        let mut ends = fake.udp_ends();
        ends.sort_by_key(|end| end.stream);
        let end = |stream| RecordedUdpEnd {
            connection: 0,
            stream,
            fin: false,
        };
        assert_eq!(ends, [end(0), end(4)]);
        let mut stream = tunnel(&out, &target(echo_server().await)).await;
        round_trip(&mut stream, b"tcp still").await;
        assert_eq!(fake.connections(), 1);
    }

    /// The credentials go to the server, the payloads to their targets,
    /// and neither shows anywhere else — in no error and in no log line,
    /// not even at the finest level — whether a datagram goes, comes back
    /// in a capsule, or is refused.
    #[tokio::test]
    async fn payloads_and_credentials_never_show() {
        const PASSWORD: &str = "s3cret-masque-udp";
        const MARK: &[u8] = b"payload-4d2f81";
        let (log, _logging, _second) = Log::capture();
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            users: vec![("u".to_string(), PASSWORD.to_string())],
            udp_capsules: true,
            ..H3ProxyScript::default()
        });
        let wrong = outbound(&fake, &format!("username=u, password={PASSWORD}-not"));
        let refused = failure(udp_carrier(&wrong).await.as_ref(), &target(echo)).await;
        assert_eq!(refused.to_string(), "masque: proxy authentication required");
        let out = outbound(&fake, &format!("username=u, password={PASSWORD}"));
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), &target(echo), MARK).await;
        let too_long = carrier
            .send_to(&MARK.repeat(5000), &target(echo))
            .await
            .unwrap_err();
        let basic = basic_authorization("u", PASSWORD);
        assert_eq!(
            fake.requests()[1].header("proxy-authorization"),
            Some(basic.as_str())
        );
        let shown = format!("{refused} {refused:?} {too_long} {too_long:?}");
        let log = log.text();
        assert!(!log.is_empty());
        for secret in [PASSWORD, &basic["Basic ".len()..], "payload-4d2f81"] {
            assert!(!shown.contains(secret), "{shown}");
            assert!(!log.contains(secret));
        }
    }
}
```

要点：
- 载体惰性打开：每个目标的流随它的第一个包建立，能力检查（等 SETTINGS）在开流时做，所以"服务端不支持 CONNECT-UDP"出现在 `send_to` 上。
- 一直读每条流（流上可能有 DATAGRAM capsule；流的任一方向结束时它的数据报随之结束），转发期间不 `shutdown`；空闲关流时才发 FIN。
- 放不下的负载被拒、什么也不发，不改用 capsule。

- [ ] **Step 3: 运行**

Run: `cargo test -p rurge-proto` → 通过（移到 `connect_udp` 的 7 条与 `h2-connect` 的 UDP 用例照常通过）。
Run: `cargo test -p rurge-proto-quic masque` → 通过（新增 23 条：请求的路径与 `:authority`、context 0、2xx 破坏 capsule 协议、每个目标一条流的往返、凭据与拒绝、畸形 2xx 被拒、服务端不支持 CONNECT-UDP 时 UDP 失败而 TCP 照常、超长被拒、流上的 DATAGRAM capsule 与未知 capsule、其它 context 不是包、畸形 capsule 结束这条流、被立刻结束的流每个包至多重开一次、空闲时以 FIN 关流而下一个包再开、GOAWAY 与连接结束后迁到新连接、路由至多每秒比较一次与载体自己发现路由变了、收件箱满即丢、许多目标同时、不应答的 CONNECT-UDP 受时限约束、载体丢掉时它的流随之结束、载荷与凭据不进日志与错误文本）。

- [ ] **Step 4: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-proto crates/rurge-proto-quic
git commit -m "feat(proto-quic): MASQUE 的 UDP——CONNECT-UDP 与 HTTP/3 数据报、每个目标一条流；capsule 编解码移到 rurge_proto::connect_udp"
```

### Task 5: 会话的流数上限与 `trust-tunnel` 的 HTTP/3

会话可选地给每条 QUIC 连接设流数上限，有上限时同时用几条连接（P11）；`trust-tunnel` 的 HTTP/3 出站用它实现 `max-streams`（P12）。MASQUE 的经典 CONNECT 隧道与按连接状态判 `Lost` 移到私有的 `connect` 模块，两种出站共用；`trust-tunnel` 的请求与拒绝措辞在 `rurge-proto` 里公开、两种传输共用（P10）。引擎的装配在 Task 8。

**Files:**
- Create: `crates/rurge-proto-quic/src/{connect.rs, trust_tunnel.rs}`（`trust_tunnel.rs` 自带用例）
- Modify: `crates/rurge-proto-quic/src/{session.rs（与用例）, lib.rs, masque/mod.rs, masque/tcp.rs, masque/udp.rs, testing/h3proxy.rs, testing/mod.rs}`、`crates/rurge-proto/src/trust_tunnel.rs`（与用例）

**Interfaces:**
- Consumes: Task 3 / 4 的 MASQUE 与 `FakeH3Proxy`；Task 1 的 `TrustTunnelSpec.h3`；`rurge_proto::trust_tunnel::USER_AGENT`、`h2connect::connect_request`；M7a 的 `Session`。
- Produces: `Session::with_stream_cap(self, cap: usize) -> Session<P>`；`rurge_proto::trust_tunnel::connect_request(target: &Target, authorization: &str, headers: &[HeaderTemplate]) -> Result<Request<()>, OutboundError>` 与 `refused(status: StatusCode) -> OutboundError`；`rurge_proto_quic::trust_tunnel::TrustTunnelH3Outbound::new(name: &str, server: Target, spec: &TrustTunnelSpec, ecn: Tristate, keystore: &[KeystoreItem], roots: Arc<RootCertStore>, connector: Arc<dyn Connector>) -> Result<TrustTunnelH3Outbound, BuildError>`（实现 `Outbound` 的 TCP）

- [ ] **Step 1: 先写用例（连同假服务端仿 TrustTunnel endpoint 的部分）**

`crates/rurge-proto-quic/src/session.rs`——把

```rust
    use tokio::io::ReadBuf;
```

换成

```rust
    use tokio::io::ReadBuf;
    use tokio::sync::watch;
    use tokio::task::JoinSet;
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
    /// and it ends only with its dial, when the caller gives up on that.
```

换成

```rust
    /// and it ends only with its dial, when the caller gives up on that.
    /// While `held` says so, every handshake that begins waits, its dial in
    /// flight, until it is let go.
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
        stalled: Notify,
```

换成

```rust
        stalled: Notify,
        held: watch::Sender<bool>,
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
                    std::future::pending::<()>().await;
                }
                let (mut send, mut recv) = connection
```

换成

```rust
                    std::future::pending::<()>().await;
                }
                let mut held = self.held.subscribe();
                let _ = held.wait_for(|held| !*held).await;
                let (mut send, mut recv) = connection
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
            "{closed:?}"
        );
    }
```

换成

```rust
            "{closed:?}"
        );
    }

    /// `session` with each connection carrying `cap` leases at most.
    fn capped(server: &Server, protocol: Plain, cap: usize) -> Session<Plain> {
        session(server, protocol).with_stream_cap(cap)
    }

    /// The leases each connection that takes requests carries, the oldest
    /// first.
    fn carried(session: &Session<Plain>) -> Vec<usize> {
        let slot = session.shared.slot.lock().unwrap();
        slot.conns
            .iter()
            .map(|conn| conn.usage.lock().unwrap().leases)
            .collect()
    }

    /// Whether `lease` is on the connection at `n` among those that take
    /// requests, the oldest first.
    fn on(session: &Session<Plain>, lease: &Lease<AtomicBool>, n: usize) -> bool {
        Arc::ptr_eq(&lease.conn, &session.shared.slot.lock().unwrap().conns[n])
    }

    /// Polls `check` until it holds, within the bound.
    async fn until(mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + BOUND;
        while !check() {
            assert!(Instant::now() < deadline, "not within the bound");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
        build(Arc::new(DirectConnector::new(Arc::new(Loopback)))).expect("a direct one hops");
    }
}
```

换成

```rust
        build(Arc::new(DirectConnector::new(Arc::new(Loopback)))).expect("a direct one hops");
    }

    /// Under a cap of two, five requests at once: a dial is room for two of
    /// them, so three dials; the requests that waited fill the oldest
    /// connections first.
    #[tokio::test]
    async fn under_a_cap_requests_that_come_together_dial_the_connections_they_need() {
        let server = Server::spawn();
        let session = capped(&server, Plain::default(), 2);
        let opts = ConnectOpts::default();
        let (a, b, c, d, e) = timeout(BOUND, async {
            tokio::join!(
                session.lease(&opts),
                session.lease(&opts),
                session.lease(&opts),
                session.lease(&opts),
                session.lease(&opts)
            )
        })
        .await
        .expect("leases within the bound");
        let _leases = [a.unwrap(), b.unwrap(), c.unwrap(), d.unwrap(), e.unwrap()];
        assert_eq!(session.shared.protocol.handshakes(), 3);
        assert_eq!(server.accepted(3).await.len(), 3);
        assert_eq!(carried(&session), [2, 2, 1]);
    }

    /// Under a cap, a connection takes requests until it carries the cap,
    /// and the next one dials; a lease let go of frees its place, which the
    /// next request takes, the oldest connection with room first.
    #[tokio::test]
    async fn under_a_cap_a_place_let_go_of_is_taken_again_the_oldest_first() {
        let server = Server::spawn();
        let session = capped(&server, Plain::default(), 2);
        let opts = ConnectOpts::default();
        let mut leases = Vec::new();
        for _ in 0..5 {
            leases.push(session.lease(&opts).await.unwrap());
        }
        assert_eq!(carried(&session), [2, 2, 1]);
        assert_eq!(session.shared.protocol.handshakes(), 3);
        // a place freed on the newest and one on the oldest
        let newest = leases.iter().position(|l| on(&session, l, 2)).unwrap();
        drop(leases.remove(newest));
        let oldest = leases.iter().position(|l| on(&session, l, 0)).unwrap();
        drop(leases.remove(oldest));
        assert_eq!(carried(&session), [1, 2, 0]);
        let next = session.lease(&opts).await.unwrap();
        assert!(on(&session, &next, 0), "{:?}", carried(&session));
        leases.push(next);
        for _ in 0..2 {
            leases.push(session.lease(&opts).await.unwrap());
        }
        assert_eq!(carried(&session), [2, 2, 2]);
        assert_eq!(
            session.shared.protocol.handshakes(),
            3,
            "places taken again"
        );
        // every place taken: the next request dials
        leases.push(session.lease(&opts).await.unwrap());
        assert_eq!(carried(&session), [2, 2, 2, 1]);
        assert_eq!(session.shared.protocol.handshakes(), 4);
    }

    /// Under a cap, a dial in flight is room for `cap` requests — its own
    /// and `cap - 1` that wait for it —; a request that would not fit dials
    /// another, eight at most at once. Held at their handshakes, the dials
    /// stay in flight while the requests are counted.
    #[tokio::test]
    async fn under_a_cap_each_dial_is_room_for_cap_requests_and_eight_dial_at_most() {
        for (cap, requests, dials, connections) in [(3, 9, 3, 3), (3, 10, 4, 4), (1, 12, 8, 12)] {
            let server = Server::spawn();
            let protocol = Plain::default();
            protocol.held.send_replace(true);
            let session = Arc::new(capped(&server, protocol, cap));
            let mut leases = JoinSet::new();
            for _ in 0..requests {
                let session = session.clone();
                leases.spawn(async move { session.lease(&ConnectOpts::default()).await });
            }
            let waiting = requests - dials;
            until(|| session.shared.protocol.handshakes() == dials).await;
            until(|| session.shared.slot.lock().unwrap().waiting == waiting).await;
            // and no more while they are held
            tokio::time::sleep(Duration::from_millis(200)).await;
            let counted = {
                let slot = session.shared.slot.lock().unwrap();
                (slot.dialing, slot.waiting)
            };
            assert_eq!(counted, (dials, waiting), "cap {cap}, {requests} requests");
            assert_eq!(session.shared.protocol.handshakes(), dials);
            session.shared.protocol.held.send_replace(false);
            let leases = timeout(BOUND, leases.join_all())
                .await
                .expect("leases within the bound");
            assert!(leases.iter().all(Result::is_ok));
            let carried = carried(&session);
            assert_eq!(carried.len(), connections, "cap {cap}: {carried:?}");
            assert!(carried.iter().all(|&n| n <= cap), "cap {cap}: {carried:?}");
            assert_eq!(carried.iter().sum::<usize>(), requests);
        }
    }

    /// Under a cap, the requests that waited for a dial share its failure;
    /// the next request dials again.
    #[tokio::test]
    async fn under_a_cap_a_failed_dial_fails_the_requests_that_waited_for_it() {
        let server = Server::spawn();
        let protocol = Plain {
            refuse: AtomicBool::new(true),
            ..Plain::default()
        };
        let session = capped(&server, protocol, 3);
        let (a, b, c) = tokio::join!(
            echo(&session, b"a"),
            echo(&session, b"b"),
            echo(&session, b"c")
        );
        for result in [a, b, c] {
            assert_eq!(result.unwrap_err().to_string(), "plain: refused");
        }
        // one dial was room for the three
        assert_eq!(session.shared.protocol.handshakes(), 1);
        session
            .shared
            .protocol
            .refuse
            .store(false, Ordering::SeqCst);
        assert_eq!(echo(&session, b"d").await.unwrap(), b"d");
        assert_eq!(session.shared.protocol.handshakes(), 2);
    }

    /// Under a cap, a connection going away takes no new requests, room or
    /// not; the stream under way on it goes on, and when its lease goes the
    /// connection is closed.
    #[tokio::test]
    async fn under_a_cap_a_connection_going_away_takes_no_new_requests() {
        let server = Server::spawn();
        let session = capped(&server, Plain::default(), 2);
        let opts = ConnectOpts::default();
        let lease = session.lease(&opts).await.unwrap();
        let (mut send, mut recv) = lease.connection().open_bi().await.unwrap();
        send.write_all(b"under ").await.unwrap();
        lease.state().store(true, Ordering::SeqCst);
        let new = session.lease(&opts).await.unwrap();
        assert_ne!(new.connection().stable_id(), lease.connection().stable_id());
        assert_eq!(carried(&session), [1]);
        assert_eq!(session.shared.protocol.handshakes(), 2);
        send.write_all(b"way").await.unwrap();
        send.finish().unwrap();
        let echoed = timeout(BOUND, recv.read_to_end(64)).await.unwrap().unwrap();
        assert_eq!(echoed, b"under way");
        drop((send, recv, lease));
        closed_by_us(&server.accepted(1).await[0]).await;
        assert!(new.connection().close_reason().is_none());
    }

    /// Under a cap, every connection dialled while the connector led
    /// elsewhere is let go of, room or not; the next request dials the new
    /// way.
    #[tokio::test]
    async fn under_a_cap_connections_dialled_the_old_way_take_no_new_requests() {
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
        let session = Session::new("P", dialer, Plain::default()).with_stream_cap(2);
        let opts = ConnectOpts::default();
        let mut leases = Vec::new();
        for _ in 0..3 {
            leases.push(session.lease(&opts).await.unwrap());
        }
        assert_eq!(carried(&session), [2, 1]);
        *rerouted.route.lock().unwrap() = Arc::new(2);
        leases.push(session.lease(&opts).await.unwrap());
        assert_eq!(carried(&session), [1]);
        assert_eq!(chain.flows.lock().unwrap().len(), 3, "dialled anew");
        assert_eq!(session.shared.protocol.handshakes(), 3);
        assert!(leases[..3].iter().all(|lease| !session.serves(lease)));
    }

    /// Under a cap, each connection idles on its own: the one let go of is
    /// closed after the idle time while another, held, stays; let go of in
    /// turn, that one follows.
    #[tokio::test]
    async fn under_a_cap_idle_connections_are_closed_one_by_one() {
        const IDLE: Duration = Duration::from_millis(400);
        let server = Server::spawn();
        let dialer = dialer(
            server.port.addr().port(),
            "127.0.0.1",
            &TlsOpts::default(),
            server.cert.roots(),
        );
        let session = Session::with_idle("P", dialer, Plain::default(), IDLE).with_stream_cap(1);
        let opts = ConnectOpts::default();
        let first = session.lease(&opts).await.unwrap();
        let second = session.lease(&opts).await.unwrap();
        let accepted = server.accepted(2).await;
        for (lease, n) in [(first, 0), (second, 1)] {
            let since = Instant::now();
            drop(lease);
            closed_by_us(&accepted[n]).await;
            assert!(
                since.elapsed() >= IDLE,
                "closed after {:?}",
                since.elapsed()
            );
            // the one still held stays
            assert!(accepted[n + 1..].iter().all(|c| c.close_reason().is_none()));
        }
        assert!(carried(&session).is_empty());
        assert_eq!(echo(&session, b"again").await.unwrap(), b"again");
        assert_eq!(session.shared.protocol.handshakes(), 3);
    }
}
```

`crates/rurge-proto/src/trust_tunnel.rs`——把

```rust
    use rurge_config::spec::{CommonOpts, ParamReader};
```

换成

```rust
    use rurge_config::spec::{CommonOpts, HeaderPart, ParamReader};
```

`crates/rurge-proto/src/trust_tunnel.rs`——把

```rust
        assert_eq!(fake.connections(), 3);
    }
}
```

换成

```rust
        assert_eq!(fake.connections(), 3);
    }

    /// The request of both transports: a classic CONNECT — the target's
    /// authority, no scheme, no path — with the credentials and our
    /// `user-agent`, which a configured header replaces; the credentials
    /// and the configured values go never indexed.
    #[test]
    fn a_connect_request_carries_the_credentials_and_a_user_agent() {
        let target = Target::new(HostName::parse("2001:db8::1"), 443);
        let request = connect_request(&target, "Basic dTpw", &[]).unwrap();
        assert_eq!(request.method(), http::Method::CONNECT);
        let uri = request.uri();
        assert_eq!(
            uri.authority().map(|a| a.as_str()),
            Some("[2001:db8::1]:443")
        );
        assert!(uri.scheme().is_none(), "{uri}");
        assert!(uri.path_and_query().is_none(), "{uri}");
        let headers = request.headers();
        assert_eq!(headers.len(), 2);
        assert_eq!(headers["proxy-authorization"], "Basic dTpw");
        assert!(headers["proxy-authorization"].is_sensitive());
        assert_eq!(headers["user-agent"], USER_AGENT);
        assert!(!headers["user-agent"].is_sensitive());

        let configured = [HeaderTemplate {
            name: "User-Agent".into(),
            value: vec![HeaderPart::Literal("Linux client".into())],
        }];
        let request = connect_request(&target, "Basic dTpw", &configured).unwrap();
        let agents: Vec<_> = request.headers().get_all("user-agent").iter().collect();
        assert_eq!(agents, ["Linux client"]);
        assert!(agents[0].is_sensitive());

        let unsendable = Target::new(HostName::Domain("a b.test".into()), 443);
        let err = connect_request(&unsendable, "Basic dTpw", &[]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "trust-tunnel: the host name cannot be sent to the server"
        );
    }

    /// Both transports word a refusal alike: 407 is a refused login, any
    /// other final status is quoted by its number.
    #[test]
    fn a_refusal_is_worded_by_its_status() {
        for (status, text) in [
            (407, "trust-tunnel: authentication failed"),
            (502, "trust-tunnel: the server answered 502"),
            (403, "trust-tunnel: the server answered 403"),
        ] {
            let err = refused(StatusCode::from_u16(status).unwrap());
            assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
            assert_eq!(err.to_string(), text);
        }
    }
}
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
//! A scriptable MASQUE proxy (phase 2 M7 design 7.4): QUIC with ALPN `h3`
//! on a loopback port of its own (`ServerPort`: it can restart) or on a
//! socket it is given (`MultiPort` for port hopping), HTTP/3 by the server
//! side of `rurge_net`, every classic CONNECT relayed to its target over
//! TCP, the half-closes too, and every CONNECT-UDP over UDP.
```

换成

```rust
//! A scriptable CONNECT proxy over HTTP/3 — MASQUE's, or the TrustTunnel
//! endpoint's (phase 2 M7 design 7.4): QUIC with ALPN `h3` on a loopback
//! port of its own (`ServerPort`: it can restart) or on a socket it is
//! given (`MultiPort` for port hopping), HTTP/3 by the server side of
//! `rurge_net`, every classic CONNECT relayed to its target over TCP, the
//! half-closes too, and every CONNECT-UDP over UDP.
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
//! - As masque-server, it wants Basic credentials in `proxy-authorization`
//!   on every request when its script names users: a request without them,
//!   or with others, is answered 407 with `proxy-authenticate`. Every
//!   refusal is a final status with `content-length: 0`, then the end of
//!   the stream.
```

换成

```rust
//! - As masque-server and the TrustTunnel endpoint, it wants Basic
//!   credentials in `proxy-authorization` on every request when its script
//!   names users: a request without them, or with others, is answered 407
//!   with `proxy-authenticate` — masque-server's challenge, or the
//!   script's. Scripted so, a request without `user-agent` is answered 400.
//!   Every refusal is a final status with `content-length: 0`, then the end
//!   of the stream.
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
const MAX_FIELD_SECTION_SIZE: u64 = 8192;
```

换成

```rust
const MAX_FIELD_SECTION_SIZE: u64 = 8192;
/// masque-server's `proxy-authenticate` on a 407.
const MASQUE_CHALLENGE: &str = "Basic realm=\"masque\"";
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
    pub users: Vec<(String, String)>,
```

换成

```rust
    pub users: Vec<(String, String)>,
    /// The `proxy-authenticate` of a 407, masque-server's unless set (the
    /// TrustTunnel endpoint's is `Basic realm=Authorization Required`).
    pub challenge: Option<String>,
    /// Answer a request without a `user-agent` 400, as an endpoint that
    /// follows the TrustTunnel document to the letter would (v1.1.0 only
    /// records it).
    pub require_user_agent: bool,
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
        Some(407)
```

换成

```rust
        Some(407)
    } else if script.require_user_agent && request.header("user-agent").is_none() {
        Some(400)
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
        return refuse(&mut stream, status, script.grease).await;
```

换成

```rust
        return refuse(&mut stream, status, script).await;
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
    let Some(upstream) = upstream else {
        return refuse(&mut stream, 502, script.grease).await;
```

换成

```rust
    let Some(upstream) = upstream else {
        return refuse(&mut stream, 502, script).await;
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
/// 0` — after the challenge, for a 407 — and the end of the stream.
async fn refuse(stream: &mut ServerStream, status: u16, grease: bool) -> io::Result<()> {
    let mut fields = Vec::new();
    if status == 407 {
        fields.push(("proxy-authenticate", "Basic realm=\"masque\""));
    }
    fields.push(("content-length", "0"));
    respond(stream, status, &fields, grease).await?;
```

换成

```rust
/// 0` — after the script's challenge, or masque-server's, for a 407 — and
/// the end of the stream.
async fn refuse(stream: &mut ServerStream, status: u16, script: &H3ProxyScript) -> io::Result<()> {
    let mut fields = Vec::new();
    if status == 407 {
        let challenge = script.challenge.as_deref();
        fields.push(("proxy-authenticate", challenge.unwrap_or(MASQUE_CHALLENGE)));
    }
    fields.push(("content-length", "0"));
    respond(stream, status, &fields, script.grease).await?;
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
        return refuse(&mut stream, 400, script.grease).await;
```

换成

```rust
        return refuse(&mut stream, 400, script).await;
```

`crates/rurge-proto-quic/src/testing/h3proxy.rs`——把

```rust
    let Some(socket) = socket else {
        return refuse(&mut stream, 502, script.grease).await;
```

换成

```rust
    let Some(socket) = socket else {
        return refuse(&mut stream, 502, script).await;
```

`crates/rurge-proto-quic/src/testing/mod.rs`——把

```rust
//! by a server that restarts, and scriptable Hysteria 2, TUIC and MASQUE
//! servers. Never used by production code.
```

换成

```rust
//! by a server that restarts, and scriptable Hysteria 2, TUIC and HTTP/3
//! CONNECT (MASQUE, TrustTunnel) servers. Never used by production code.
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto-quic --lib session`
Expected: FAIL——`with_stream_cap` 与会话的多连接（`Slot` 的 `conns`、`waiting`）由 Step 3 引入，编译不过：

```text
error[E0599]: no method named `with_stream_cap` found for struct `session::Session` in the current scope
   --> crates\rurge-proto-quic\src\session.rs:931:35
error[E0609]: no field `conns` on type `std::sync::MutexGuard<'_, session::Slot<AtomicBool>>`
   --> crates\rurge-proto-quic\src\session.rs:938:14
error[E0609]: no field `conns` on type `std::sync::MutexGuard<'_, session::Slot<AtomicBool>>`
   --> crates\rurge-proto-quic\src\session.rs:947:71
error[E0609]: no field `waiting` on type `std::sync::MutexGuard<'_, session::Slot<AtomicBool>>`
    --> crates\rurge-proto-quic\src\session.rs:1552:58
error[E0609]: no field `waiting` on type `std::sync::MutexGuard<'_, session::Slot<AtomicBool>>`
    --> crates\rurge-proto-quic\src\session.rs:1557:37
error[E0599]: no method named `with_stream_cap` found for struct `session::Session` in the current scope
    --> crates\rurge-proto-quic\src\session.rs:1645:67
error[E0599]: no method named `with_stream_cap` found for struct `session::Session` in the current scope
    --> crates\rurge-proto-quic\src\session.rs:1673:79
error: could not compile `rurge-proto-quic` (lib test) due to 7 previous errors
```

（`cargo test -p rurge-proto --lib trust_tunnel` 同样编译不过：`connect_request` 与 `refused` 还没有。）

- [ ] **Step 3: 实现**

会话的流数上限：

`crates/rurge-proto-quic/src/session.rs`——把

```rust
//! 3.5).
```

换成

```rust
//! 3.5) — or its connections, under a cap on the requests each carries
//! (`Session::with_stream_cap`).
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
//!   theirs too.
```

换成

```rust
//!   theirs too.
//! - Under a cap (`trust-tunnel`'s `max-streams` over HTTP/3), a request
//!   takes the oldest connection that carries fewer requests than the cap,
//!   as the HTTP/2 pool does (phase 2 M6 design 16, 18). With none, it
//!   waits for the dials in flight while they have room for it — each is
//!   room for `cap` requests, its own and the ones that wait for it —,
//!   else it dials another, `MAX_DIALS` at most at once. Once no dial is in
//!   flight, a request that waited shares the latest dial's failure if that
//!   dial ended after it began to wait. Each connection then goes by the
//!   rules below on its own.
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
//! - A request on a connection that was up when it came, which finds the
//!   connection gone before any of the request's own data went out, goes
//!   again, once, on a new connection (`Session::request`): a server that
//!   dropped the connection while it idled answers the request's first
//!   packet with a stateless reset.
```

换成

```rust
//! - A request on a connection it took at once — one that was up when it
//!   came —, which finds the connection gone before any of the request's
//!   own data went out, goes again, once, on a new connection — under a
//!   cap, on another with room if there is one — (`Session::request`): a
//!   server that dropped the connection while it idled answers the
//!   request's first packet with a stateless reset.
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
```

换成

```rust
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// The most dials in flight at once under a cap, as in the HTTP/2 pool: a
/// burst after a network change does not open hundreds of connections
/// together.
const MAX_DIALS: usize = 8;
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
/// A policy's QUIC connection, shared by its requests (design 3.5). No
/// `Debug`: the protocol may hold credentials.
```

换成

```rust
/// A policy's QUIC connection — under a cap, its connections —, shared by
/// its requests (design 3.5). No `Debug`: the protocol may hold
/// credentials.
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
    shared: Arc<Shared<P>>,
```

换成

```rust
    shared: Arc<Shared<P>>,
    /// The most leases a connection carries at once (`with_stream_cap`);
    /// without a cap, the one connection takes them all.
    cap: Option<usize>,
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
/// The session's connection and its dial.
struct Slot<S> {
    /// The connection new requests go to.
    current: Option<Arc<Conn<S>>>,
    /// A dial is in flight.
    dialing: bool,
```

换成

```rust
/// The session's connections and their dials.
struct Slot<S> {
    /// The connections new requests go to, oldest first: one at most
    /// without a cap.
    conns: Vec<Arc<Conn<S>>>,
    /// Dials in flight: one at most without a cap.
    dialing: usize,
    /// Requests waiting for them.
    waiting: usize,
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
                    current: None,
                    dialing: false,
```

换成

```rust
                    conns: Vec::new(),
                    dialing: 0,
                    waiting: 0,
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
            }),
            dialed: Notify::new(),
```

换成

```rust
            }),
            cap: None,
            dialed: Notify::new(),
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
    /// A lease on the session's connection, dialing one when there is none
    /// (once for all the requests that come meanwhile).
```

换成

```rust
    /// The session with each connection carrying `cap` leases at most (1
    /// at least): it keeps as many connections as its requests need
    /// (`trust-tunnel`'s `max-streams` over HTTP/3, design 3.5).
    pub fn with_stream_cap(self, cap: usize) -> Session<P> {
        Session {
            cap: Some(cap.max(1)),
            ..self
        }
    }

    /// A lease on a connection of the session — under a cap, one with room
    /// —, dialing one when there is none: once for all the requests that
    /// come meanwhile, or, under a cap, as often as they need.
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
    /// at most once a second, `serves_without_route` in between.
```

换成

```rust
    /// at most once a second, `serves_without_route` in between. Under a
    /// cap, a new request needs room on it too.
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
    /// connection that was up when it came gone before any of its own data
    /// went out (`Failure::Lost`) is attempted again, once, on a new
    /// connection; on a connection dialed for it, it fails. Either way the
    /// connection found gone takes no new requests.
```

换成

```rust
    /// connection it took at once gone before any of its own data went out
    /// (`Failure::Lost`) is attempted again, once, on a new connection —
    /// under a cap, on another with room if there is one —; on a
    /// connection it waited for, it fails. Either way the connection found
    /// gone takes no new requests.
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
    /// is none; and whether the connection was up when the request came,
    /// rather than dialed for it or for requests it waited with.
```

换成

```rust
    /// is none; and whether the request took it at once — a connection that
    /// was up when it came —, rather than after a dial of its own or of
    /// others it waited for.
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
                if let Some(lease) = self.shared.pick(&mut slot, route.as_ref()) {
```

换成

```rust
                if let Some(lease) = self.shared.pick(&mut slot, route.as_ref(), self.cap) {
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
                    && !slot.dialing
```

换成

```rust
                    && slot.dialing == 0
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
                if !slot.dialing {
                    slot.dialing = true;
                    break;
                }
                waited_since = Some(slot.dials);
            }
```

换成

```rust
                if self.dials_another(&slot) {
                    slot.dialing += 1;
                    break;
                }
                slot.waiting += 1;
                waited_since = Some(slot.dials);
            }
            let _waiting = Waiting(&self.shared);
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
        self.dial(opts).await.map(|lease| (lease, false))
```

换成

```rust
        self.dial(opts).await.map(|lease| (lease, false))
    }

    /// Whether a request that found no room dials, rather than waits for
    /// the dials in flight. Without a cap, a dial is room for every request
    /// that comes meanwhile. Under one, a dial is room for its own request
    /// and `cap - 1` that wait for it, and a request that would not fit
    /// dials another, `MAX_DIALS` at most at once.
    fn dials_another(&self, slot: &Slot<P::State>) -> bool {
        match self.cap {
            None => slot.dialing == 0,
            Some(cap) => {
                slot.dialing < MAX_DIALS && slot.waiting >= slot.dialing.saturating_mul(cap - 1)
            }
        }
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
                    slot.current = Some(conn);
```

换成

```rust
                    slot.conns.push(conn);
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
        if slot
            .current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, conn))
        {
            slot.current = None;
        }
```

换成

```rust
        slot.conns.retain(|kept| !Arc::ptr_eq(kept, conn));
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
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
```

换成

```rust
    /// A lease on the connection a request goes to, when one takes it: the
    /// oldest with room under `cap`. A connection that takes no requests —
    /// `prune`'s, or one dialled another way than the connector leads now
    /// (`route`) — is let go of on the way. The one place a connection is
    /// chosen.
    fn pick(
        &self,
        slot: &mut Slot<P::State>,
        route: Option<&Route>,
        cap: Option<usize>,
    ) -> Option<Lease<P::State>> {
        self.prune(slot, Instant::now());
        slot.conns.retain(|conn| self.on_route(conn, route));
        slot.conns
            .iter()
            .find(|conn| conn.has_room(cap))
            .map(Conn::lease)
    }

    /// Lets go of each connection once it takes no new requests: closed,
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
        if slot.current.as_ref().is_some_and(|conn| {
            conn.quic.close_reason().is_some()
                || self.protocol.is_going_away(&conn.state)
                || conn.is_expired(now, self.idle)
        }) {
            slot.current = None;
        }
```

换成

```rust
        slot.conns.retain(|conn| {
            conn.quic.close_reason().is_none()
                && !self.protocol.is_going_away(&conn.state)
                && !conn.is_expired(now, self.idle)
        });
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
        self.0.shared.slot.lock().expect("session").dialing = false;
        self.0.dialed.notify_waiters();
```

换成

```rust
        self.0.shared.slot.lock().expect("session").dialing -= 1;
        self.0.dialed.notify_waiters();
    }
}

/// A request waiting for the dials in flight.
struct Waiting<'a, P: Protocol>(&'a Shared<P>);

impl<P: Protocol> Drop for Waiting<'_, P> {
    fn drop(&mut self) {
        self.0.slot.lock().expect("session").waiting -= 1;
```

`crates/rurge-proto-quic/src/session.rs`——把

```rust
        Lease { conn: self.clone() }
```

换成

```rust
        Lease { conn: self.clone() }
    }

    /// Whether the connection carries fewer leases than `cap`; any number
    /// without one. Looked at under the session's lock, as leases are
    /// taken: only a lease let go of can change it before `lease`.
    fn has_room(&self, cap: Option<usize>) -> bool {
        cap.is_none_or(|cap| self.usage.lock().expect("usage").leases < cap)
```

`trust-tunnel` 的请求与拒绝措辞，两种传输共用：

`crates/rurge-proto/src/trust_tunnel.rs`——把

```rust
//! CONNECT stream on a pooled TLS + HTTP/2 connection, as on `h2-connect`.
```

换成

```rust
//! CONNECT stream on a pooled TLS + HTTP/2 connection, as on `h2-connect`.
//! With `h3=true` the same requests go over HTTP/3 instead
//! (`rurge-proto-quic`), refused alike: `connect_request` and `refused` are
//! both transports'.
```

`crates/rurge-proto/src/trust_tunnel.rs`——把

```rust
use crate::h2connect::connect_request;
```

换成

```rust
use crate::h2connect;
```

`crates/rurge-proto/src/trust_tunnel.rs`——把

```rust
use http::StatusCode;
```

换成

```rust
use http::{Request, StatusCode};
```

`crates/rurge-proto/src/trust_tunnel.rs`——把

```rust
pub const USER_AGENT: &str = "rurge";
```

换成

```rust
pub const USER_AGENT: &str = "rurge";

/// The CONNECT for `target`, over HTTP/2 and HTTP/3 alike (TrustTunnel
/// protocol 5.1): `proxy-authorization` — `authorization`, the Basic value
/// — and `user-agent`, then `headers` rendered anew, a configured header
/// replacing one of ours with the same name; the credentials and the
/// configured values never indexed (HPACK, QPACK). Refused, before anything
/// is dialed, for a target whose name cannot be sent. The request says
/// HTTP/2, as `h2` wants; the HTTP/3 client reads its method, its authority
/// and its fields alone.
pub fn connect_request(
    target: &Target,
    authorization: &str,
    headers: &[HeaderTemplate],
) -> Result<Request<()>, OutboundError> {
    let ours = vec![
        ("proxy-authorization".to_string(), authorization.to_string()),
        ("user-agent".to_string(), USER_AGENT.to_string()),
    ];
    h2connect::connect_request(LABEL, target, ours, headers)
}

/// The endpoint's refusal of a CONNECT, a final status other than 2xx, over
/// HTTP/2 and HTTP/3 alike: 407 is a refused login, any other is quoted by
/// its number.
pub fn refused(status: StatusCode) -> OutboundError {
    match status {
        StatusCode::PROXY_AUTHENTICATION_REQUIRED => {
            OutboundError::Proxy(format!("{LABEL}: authentication failed"))
        }
        status => OutboundError::Proxy(format!("{LABEL}: the server answered {}", status.as_u16())),
    }
}
```

`crates/rurge-proto/src/trust_tunnel.rs`——把

```rust
        let ours = vec![
            (
                "proxy-authorization".to_string(),
                self.authorization.clone(),
            ),
            ("user-agent".to_string(), USER_AGENT.to_string()),
        ];
        // never dial for a target whose name cannot be sent
        let request = connect_request(LABEL, target, ours, &self.headers)?;
```

换成

```rust
        // never dial for a target whose name cannot be sent
        let request = connect_request(target, &self.authorization, &self.headers)?;
```

`crates/rurge-proto/src/trust_tunnel.rs`——把

```rust
            StatusCode::PROXY_AUTHENTICATION_REQUIRED => Err(OutboundError::Proxy(format!(
                "{LABEL}: authentication failed"
            ))),
            status => Err(OutboundError::Proxy(format!(
                "{LABEL}: the server answered {}",
                status.as_u16()
            ))),
```

换成

```rust
            status => Err(refused(status)),
```

经典 CONNECT 的隧道移到共用的 `connect`，MASQUE 改用它：

新建 `crates/rurge-proto-quic/src/connect.rs`：

```rust
//! What the CONNECT proxies over HTTP/3 share — `masque`, and
//! `trust-tunnel` with `h3=true` (phase 2 M7 design 7.1, 7.2): the HTTP/3
//! client a connection keeps (`Connected`), how a request on it fails
//! (`failure`), and the tunnel of a classic CONNECT (RFC 9114 4.4) on a
//! stream of its own (`open`).
//!
//! - The request goes out at once: a classic CONNECT needs nothing of the
//!   server's SETTINGS.
//! - Nothing of the caller's goes before the response: a 2xx makes the
//!   stream the tunnel (RFC 9110 9.3.6); any other final status is the
//!   proxy's refusal, which the protocol words.
//! - Until the response came, a request that finds its connection gone —
//!   closed, lost by a server that restarted, or going away — is `Lost`:
//!   nothing of the caller's went out, and on another connection it may yet
//!   go through (`Session::request`).

use crate::session::{Failure, Lease};
use http::{Request, StatusCode};
use quinn::VarInt;
use rurge_net::quic::h3::{self, RequestStream};
use rurge_proto::OutboundError;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// H3_NO_ERROR (RFC 9114 8.1): a connection let go of.
pub(crate) const H3_NO_ERROR: VarInt = VarInt::from_u32(0x100);

/// What a connection keeps for its requests.
pub(crate) struct Connected {
    /// Lives as long as the connection: dropped, it would end our HTTP/3
    /// control stream, which a server may take for the end of HTTP/3 on the
    /// connection.
    pub(crate) h3: h3::Client,
}

/// How a request failed before its response came: `Lost` when its
/// connection turned out gone or going away, else final. The error says
/// its layer (`h3: …`, `quic: …`).
pub(crate) fn failure(lease: &Lease<Connected>, e: io::Error) -> Failure {
    if lease.connection().close_reason().is_some() || lease.state().h3.is_going_away() {
        Failure::Lost(e.into())
    } else {
        Failure::Failed(e.into())
    }
}

/// The tunnel `request` — a classic CONNECT — asks for, on the connection
/// of `lease`: the request on a stream of its own, then the proxy's
/// response; a final status other than 2xx is the error `refused` words.
/// The stream handed back is the caller's.
pub(crate) async fn open(
    lease: Lease<Connected>,
    request: Request<()>,
    refused: fn(StatusCode) -> OutboundError,
) -> Result<Tunnel, Failure> {
    let mut stream = lease
        .state()
        .h3
        .request(request)
        .await
        .map_err(|e| failure(&lease, e))?;
    let response = stream.response().await.map_err(|e| failure(&lease, e))?;
    match response.status() {
        status if status.is_success() => Ok(Tunnel {
            stream,
            _lease: lease,
        }),
        status => Err(Failure::Failed(refused(status))),
    }
}

/// A TCP connection through the proxy: the CONNECT's stream — the bytes go
/// as DATA frames —, on a connection its lease keeps. A write that returns
/// may leave the rest of its frame for the next write, flush or shutdown.
/// Shutdown sends the FIN the target then sees; reading goes on to the
/// proxy's. Dropped before both directions ended, the stream is cancelled
/// both ways (RFC 9114 4.1.1). No `Debug`: nothing in it is worth printing.
pub(crate) struct Tunnel {
    stream: RequestStream,
    /// Last: the stream ends before its connection may be closed.
    _lease: Lease<Connected>,
}

impl AsyncRead for Tunnel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for Tunnel {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}
```

`crates/rurge-proto-quic/src/lib.rs`——把

```rust
//! (`hysteria2`), TUIC's (`tuic`) and MASQUE's (`masque`).

pub mod hysteria2;
```

换成

```rust
//! (`hysteria2`), TUIC's (`tuic`) and MASQUE's (`masque`) —, or, for
//! `trust-tunnel` over HTTP/3 (`trust_tunnel`), as many as its requests
//! need, each carrying `max-streams` of them at most.

mod connect;
pub mod hysteria2;
```

`crates/rurge-proto-quic/src/lib.rs`——把

```rust
pub mod testing;
```

换成

```rust
pub mod testing;
pub mod trust_tunnel;
```

`crates/rurge-proto-quic/src/lib.rs`——把

```rust
pub use session::{Dialer, Failure, Layer, Lease, Protocol, Session};
```

换成

```rust
pub use session::{Dialer, Failure, Layer, Lease, Protocol, Session};
pub use trust_tunnel::TrustTunnelH3Outbound;
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

```rust
//! - A TCP connection is a classic HTTP/3 CONNECT of its own (`tcp`), as
//!   Surge relays TCP ("HTTP/3 CONNECT tunnels multiplexed over a single
//!   QUIC connection"): the template-driven `connect-tcp` is still a draft,
//!   and masque-server answers it 404.
```

换成

```rust
//! - A TCP connection is a classic HTTP/3 CONNECT of its own (`tcp`,
//!   `connect`), as Surge relays TCP ("HTTP/3 CONNECT tunnels multiplexed
//!   over a single QUIC connection"): the template-driven `connect-tcp` is
//!   still a draft, and masque-server answers it 404.
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

```rust
use crate::session::{Dialer, Failure, Lease, Protocol, Session};
```

换成

```rust
use crate::connect::{self, Connected, H3_NO_ERROR};
use crate::session::{Dialer, Protocol, Session};
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

```rust
use std::io;
use std::sync::Arc;
use udp::MasqueUdp;

/// H3_NO_ERROR (RFC 9114 8.1): a connection let go of.
const H3_NO_ERROR: VarInt = VarInt::from_u32(0x100);
```

换成

```rust
use std::sync::Arc;
use udp::MasqueUdp;
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

```rust
        })
    }
}

/// How a request failed before its response came: `Lost` when its
/// connection turned out gone or going away, else final. The error says
/// its layer (`h3: …`, `quic: …`).
fn failure(lease: &Lease<Connected>, e: io::Error) -> Failure {
    if lease.connection().close_reason().is_some() || lease.state().h3.is_going_away() {
        Failure::Lost(e.into())
    } else {
        Failure::Failed(e.into())
    }
}

fn proxy(text: &str) -> OutboundError {
```

换成

```rust
        })
    }
}

fn proxy(text: &str) -> OutboundError {
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

```rust
struct Http3;

/// What a connection keeps for its requests.
struct Connected {
    /// Lives as long as the connection: dropped, it would end our HTTP/3
    /// control stream, which a server may take for the end of HTTP/3 on the
    /// connection.
    h3: h3::Client,
}
```

换成

```rust
struct Http3;
```

`crates/rurge-proto-quic/src/masque/mod.rs`——把

```rust
            let tunnel = self
                .session
                .request(opts, |lease| Box::pin(tcp::open(lease, request.clone())));
```

换成

```rust
            let tunnel = self.session.request(opts, |lease| {
                Box::pin(connect::open(lease, request.clone(), refused))
            });
```

`crates/rurge-proto-quic/src/masque/tcp.rs`——把

```rust
//! QUIC connection (RFC 9114 4.4).
```

换成

```rust
//! QUIC connection (RFC 9114 4.4), its tunnel as `connect` makes it.
```

`crates/rurge-proto-quic/src/masque/tcp.rs`——把

```rust
//!   `proxy-authorization` when the policy has credentials. It goes out at
//!   once: a classic CONNECT needs nothing of the server's SETTINGS.
//! - Nothing of the caller's goes before the response: a 2xx makes the
//!   stream the tunnel (RFC 9110 9.3.6); 407 is a refused login, any other
//!   final status the proxy's refusal, quoted by its number.
//! - Until the response came, a request that finds its connection gone —
//!   closed, lost by a server that restarted, or going away — is `Lost`:
//!   nothing of the caller's went out, and on another connection it may yet
//!   go through (`Session::request`).

use super::{Connected, failure, proxy, refused};
use crate::session::{Failure, Lease};
```

换成

```rust
//!   `proxy-authorization` when the policy has credentials.
//! - 407 is a refused login, any other final status but a 2xx the proxy's
//!   refusal, quoted by its number (`refused`).

use super::proxy;
```

`crates/rurge-proto-quic/src/masque/tcp.rs`——把

```rust
use rurge_net::quic::h3::RequestStream;
use rurge_proto::OutboundError;
use rurge_proto::http::wire_host;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
```

换成

```rust
use rurge_proto::OutboundError;
use rurge_proto::http::wire_host;
```

`crates/rurge-proto-quic/src/masque/tcp.rs`——把

```rust
    Ok(request)
}

/// The tunnel `request` asks for, on the connection of `lease`: the request
/// on a stream of its own, then the proxy's response. The stream handed
/// back is the caller's.
pub(super) async fn open(lease: Lease<Connected>, request: Request<()>) -> Result<Tunnel, Failure> {
    let mut stream = lease
        .state()
        .h3
        .request(request)
        .await
        .map_err(|e| failure(&lease, e))?;
    let response = stream.response().await.map_err(|e| failure(&lease, e))?;
    match response.status() {
        status if status.is_success() => Ok(Tunnel {
            stream,
            _lease: lease,
        }),
        status => Err(Failure::Failed(refused(status))),
    }
}

/// A TCP connection through the proxy: the CONNECT's stream — the bytes go
/// as DATA frames —, on a connection its lease keeps. A write that returns
/// may leave the rest of its frame for the next write, flush or shutdown.
/// Shutdown sends the FIN the target then sees; reading goes on to the
/// proxy's. Dropped before both directions ended, the stream is cancelled
/// both ways (RFC 9114 4.1.1). No `Debug`: nothing in it is worth printing.
pub(super) struct Tunnel {
    stream: RequestStream,
    /// Last: the stream ends before its connection may be closed.
    _lease: Lease<Connected>,
}

impl AsyncRead for Tunnel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for Tunnel {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
```

换成

```rust
    Ok(request)
```

`crates/rurge-proto-quic/src/masque/udp.rs`——把

```rust
use super::{Connected, Http3, failure, proxy, refused};
```

换成

```rust
use super::{Http3, proxy, refused};
use crate::connect::{Connected, failure};
```

`trust-tunnel` 的 HTTP/3 出站：

新建 `crates/rurge-proto-quic/src/trust_tunnel.rs`：

```rust
//! `trust-tunnel` outbound over HTTP/3 (manual: Policies › Trust Tunnel,
//! `h3`; TrustTunnel protocol 3.2, 5.1; phase 2 M7 design 7.2): every TCP
//! connection a classic HTTP/3 CONNECT of its own (`connect`) on one of the
//! policy's QUIC connections, each of which carries `max-streams` of them
//! at most (`Session::with_stream_cap`): more at once open another.
//!
//! - The request and its refusals are the HTTP/2 transport's
//!   (`rurge_proto::trust_tunnel`): `:method CONNECT` and `:authority
//!   host:port` alone — the endpoint answers 400 to a `:scheme` or a
//!   `:path` —, `proxy-authorization: Basic …` and `user-agent`, then the
//!   configured `headers`; 407 is a refused login, any other final status
//!   is quoted by its number.
//! - ALPN `h3`, and HTTP/3 without HTTP Datagrams: the endpoint takes no
//!   QUIC datagrams, and a classic CONNECT needs none. No port hopping.
//! - Past the server's own limit on the streams of a connection (QUIC's,
//!   which quinn keeps without telling it), a tunnel waits for a stream
//!   within its time, as on MASQUE: the session counts `max-streams`
//!   alone, where the HTTP/2 pool keeps the smaller of the two.
//! - TCP only, as over HTTP/2, and no fallback to HTTP/2: with `h3=true`
//!   the tunnel runs "over HTTP/3 (QUIC) instead of HTTP/2 over TLS"
//!   (manual).

use crate::connect::{self, Connected, H3_NO_ERROR};
use crate::session::{Dialer, Protocol, Session};
use quinn::{Connection, VarInt};
use rurge_config::KeystoreItem;
use rurge_config::spec::{HeaderTemplate, QuicOpts, Tristate, TrustTunnelSpec};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_net::quic::h3;
use rurge_proto::http::basic_authorization;
use rurge_proto::trust_tunnel::{connect_request, refused};
use rurge_proto::{BuildError, Outbound, OutboundError};
use rustls::RootCertStore;
use std::sync::Arc;

/// No `Debug`: it holds the credentials.
pub struct TrustTunnelH3Outbound {
    name: String,
    session: Session<Http3>,
    /// The `proxy-authorization` value, ready to send.
    authorization: String,
    headers: Vec<HeaderTemplate>,
}

impl TrustTunnelH3Outbound {
    /// The connections go as `connector` dials (design 3.3), with the
    /// policy's `ecn`; `client-cert` comes from `keystore`. `spec` is that
    /// of a line with `h3=true`, its `alpn` pinned to `h3`.
    pub fn new(
        name: &str,
        server: Target,
        spec: &TrustTunnelSpec,
        ecn: Tristate,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<TrustTunnelH3Outbound, BuildError> {
        // error texts carry no policy name: the registry's `build_one` and
        // the dry build both prefix it. A spec made by hand may carry
        // headers `rurge-config` never let through: never echo them.
        if let Some(n) = spec.headers.iter().position(|t| !t.is_valid()) {
            return Err(BuildError::new(format!(
                "custom header #{} is not valid",
                n + 1
            )));
        }
        // the SNI is the server's name or `sni=`: the endpoint picks its
        // host by the exact SNI
        let dialer = Dialer::new(
            connector,
            server,
            &spec.tls,
            &QuicOpts::default(),
            ecn,
            keystore,
            roots,
        )?;
        let max_streams = spec.max_streams as usize;
        Ok(TrustTunnelH3Outbound {
            name: name.to_string(),
            session: Session::new(name, dialer, Http3).with_stream_cap(max_streams),
            authorization: basic_authorization(spec.username.expose(), spec.password.expose()),
            headers: spec.headers.clone(),
        })
    }
}

/// What every new connection runs before its tunnels: HTTP/3, nothing
/// more — each request carries the credentials.
struct Http3;

impl Protocol for Http3 {
    type State = Connected;

    const NAME: &'static str = "trust-tunnel";

    const NO_ERROR: VarInt = H3_NO_ERROR;

    /// Our SETTINGS go out, and tunnels may follow at once: a classic
    /// CONNECT waits for nothing of the server's.
    fn handshake<'a>(
        &'a self,
        connection: &'a Connection,
    ) -> BoxFuture<'a, Result<Connected, OutboundError>> {
        Box::pin(async move {
            Ok(Connected {
                h3: h3::Client::new(connection.clone()),
            })
        })
    }

    /// The server sent GOAWAY: it is going down.
    fn is_going_away(&self, state: &Connected) -> bool {
        state.h3.is_going_away()
    }
}

impl Outbound for TrustTunnelH3Outbound {
    fn name(&self) -> &str {
        &self.name
    }

    // `udp()` stays `Unsupported`, as over HTTP/2: the engine applies
    // `udp-policy-not-supported-behaviour`

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            // never dial for a target whose name cannot be sent
            let request = connect_request(target, &self.authorization, &self.headers)?;
            // one budget for the connection (when one is dialed), QUIC's
            // handshake and the server's response
            let tunnel = self.session.request(opts, |lease| {
                Box::pin(connect::open(lease, request.clone(), refused))
            });
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
    use crate::testing::{FakeH3Proxy, H3ProxyScript};
    use quinn::ConnectionError;
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::h2::read_trust_tunnel;
    use rurge_config::spec::{CommonOpts, ParamReader};
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_proto::UdpSupport;
    use rurge_proto::testing::echo_server;
    use rurge_proto::trust_tunnel::USER_AGENT;
    use std::io;
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::{Instant, timeout};

    const BOUND: Duration = Duration::from_secs(10);
    /// The credentials the endpoint knows, unless a test says otherwise.
    const LOGIN: &str = "username=u, password=p";

    /// The fake as the TrustTunnel endpoint v1.1.0 answers over HTTP/3,
    /// `script` aside: quiche's grease, neither extended CONNECT nor HTTP
    /// Datagrams, its challenge on a 407, and the user `u` with the
    /// password `p` unless `script` names users.
    fn endpoint(mut script: H3ProxyScript) -> FakeH3Proxy {
        if script.users.is_empty() {
            script.users = vec![("u".to_string(), "p".to_string())];
        }
        FakeH3Proxy::spawn(H3ProxyScript {
            challenge: Some("Basic realm=Authorization Required".to_string()),
            grease: true,
            no_extended_connect: true,
            no_h3_datagram: true,
            no_datagrams: true,
            ..script
        })
    }

    /// The spec of `trust-tunnel, 127.0.0.1, <port>, h3=true, <params>`,
    /// as the configuration reads it.
    fn spec(port: u16, params: &str) -> TrustTunnelSpec {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let line = format!("trust-tunnel, 127.0.0.1, {port}, h3=true, {params}");
        let policy = parse_policy("T", &line, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let spec = read_trust_tunnel(&mut r, &mut CommonOpts::default(), &[]);
        assert!(!r.has_errors(), "{:?}", r.finish());
        spec
    }

    /// `trust-tunnel, 127.0.0.1, <the fake's port>, h3=true, <params>`.
    fn outbound(fake: &FakeH3Proxy, params: &str) -> TrustTunnelH3Outbound {
        let port = fake.addr().port();
        TrustTunnelH3Outbound::new(
            "T",
            Target::new(HostName::parse("127.0.0.1"), port),
            &spec(port, params),
            Tristate::Auto,
            &[],
            fake.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    async fn tunnel(out: &TrustTunnelH3Outbound, to: &Target) -> BoxedStream {
        timeout(BOUND, out.connect_tcp(to, &ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .expect("a tunnel")
    }

    async fn failure(out: &TrustTunnelH3Outbound, to: &Target) -> OutboundError {
        timeout(BOUND, out.connect_tcp(to, &ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .err()
            .expect("no tunnel")
    }

    /// Flushed: a write may leave the rest of its DATA frame for the next.
    async fn round_trip(stream: &mut BoxedStream, payload: &[u8]) {
        stream.write_all(payload).await.unwrap();
        stream.flush().await.unwrap();
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

    /// The tunnels each connection the fake accepted carried, fewest first.
    fn carried(fake: &FakeH3Proxy) -> Vec<usize> {
        let mut carried = vec![0; fake.connections()];
        for request in fake.requests() {
            carried[request.connection] += 1;
        }
        carried.sort_unstable();
        carried
    }

    /// The connection that carried the request for `authority`.
    fn connection_of(fake: &FakeH3Proxy, authority: &str) -> usize {
        let requests = fake.requests();
        let request = requests
            .iter()
            .find(|r| r.authority.as_deref() == Some(authority))
            .expect("a request for the authority");
        request.connection
    }

    /// Design 7.2: a classic HTTP/3 CONNECT — `:method` and `:authority`
    /// alone, as the endpoint wants — with the credentials and our
    /// `user-agent`, as over HTTP/2; its 2xx makes the tunnel. No UDP.
    #[tokio::test]
    async fn a_tunnel_is_a_classic_connect_with_the_credentials_and_a_user_agent() {
        let echo = echo_server().await;
        let fake = endpoint(H3ProxyScript {
            require_user_agent: true,
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, LOGIN);
        assert_eq!(out.name(), "T");
        assert!(out.http_forward().is_none());
        assert_eq!(out.udp(), UdpSupport::Unsupported);
        let mut stream = tunnel(&out, &target(echo)).await;
        round_trip(&mut stream, b"through the tunnel").await;
        let requests = fake.requests();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.method, "CONNECT");
        assert_eq!(request.authority, Some(echo.to_string()));
        assert_eq!(
            (&request.protocol, &request.scheme, &request.path),
            (&None, &None, &None)
        );
        let mut fields = request.headers.clone();
        fields.sort();
        // base64("u:p")
        assert_eq!(
            fields,
            [
                ("proxy-authorization".to_string(), "Basic dTpw".to_string()),
                ("user-agent".to_string(), USER_AGENT.to_string()),
            ]
        );
    }

    /// `headers` as over HTTP/2: a configured field replaces ours by name,
    /// and its placeholders are drawn anew for every tunnel.
    #[tokio::test]
    async fn headers_replace_the_user_agent_and_are_drawn_anew_for_every_tunnel() {
        let echo = echo_server().await;
        let fake = endpoint(H3ProxyScript::default());
        let out = outbound(
            &fake,
            &format!("{LOGIN}, headers=User-Agent:Linux client<random-string(12)>;X-Note:n"),
        );
        for _ in 0..2 {
            let mut stream = tunnel(&out, &target(echo)).await;
            round_trip(&mut stream, b"x").await;
        }
        let requests = fake.requests();
        assert_eq!(requests.len(), 2);
        let mut drawn = Vec::new();
        for request in &requests {
            let agents: Vec<&str> = request
                .headers
                .iter()
                .filter(|(name, _)| name == "user-agent")
                .map(|(_, value)| value.as_str())
                .collect();
            assert_eq!(agents.len(), 1, "one user-agent: {agents:?}");
            let random = agents[0].strip_prefix("Linux client").expect(agents[0]);
            assert_eq!(random.len(), 12, "{}", agents[0]);
            assert!(
                random
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
                "{random}"
            );
            drawn.push(random.to_string());
            assert_eq!(request.header("x-note"), Some("n"));
            assert_eq!(request.header("proxy-authorization"), Some("Basic dTpw"));
        }
        assert_ne!(drawn[0], drawn[1], "drawn anew for every tunnel");
    }

    /// 407 — with the endpoint's challenge — is a refused login, worded as
    /// over HTTP/2; it costs the connection nothing.
    #[tokio::test]
    async fn a_refused_login_says_authentication_failed() {
        let echo = echo_server().await;
        let fake = endpoint(H3ProxyScript::default());
        let wrong = outbound(&fake, "username=u, password=wrong");
        for _ in 0..2 {
            let err = failure(&wrong, &target(echo)).await;
            assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
            assert_eq!(err.to_string(), "trust-tunnel: authentication failed");
        }
        assert_eq!(fake.connections(), 1);
    }

    /// Any other final status is quoted by its number, as over HTTP/2: the
    /// endpoint answers 502 for a target it cannot reach.
    #[tokio::test]
    async fn an_unreachable_target_and_other_statuses_are_quoted_by_number() {
        // a port nothing listens on
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let fake = endpoint(H3ProxyScript::default());
        let out = outbound(&fake, LOGIN);
        let err = failure(&out, &target(closed)).await;
        assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
        assert_eq!(err.to_string(), "trust-tunnel: the server answered 502");
        let fake = endpoint(H3ProxyScript {
            refuse: Some(403),
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, LOGIN);
        let err = failure(&out, &target(closed)).await;
        assert_eq!(err.to_string(), "trust-tunnel: the server answered 403");
    }

    /// A target whose name cannot be sent is refused, as over HTTP/2,
    /// before anything is dialed.
    #[tokio::test]
    async fn a_name_that_cannot_be_sent_never_dials() {
        let fake = endpoint(H3ProxyScript::default());
        let out = outbound(&fake, LOGIN);
        for name in ["x@blocked.test", "a b.test", "a.test\r\nx: 1", ""] {
            let to = Target::new(HostName::Domain(name.into()), 443);
            assert_eq!(
                failure(&out, &to).await.to_string(),
                "trust-tunnel: the host name cannot be sent to the server"
            );
        }
        assert_eq!(fake.connections(), 0);
    }

    /// Design 7.2: `max-streams` tunnels at most on each QUIC connection;
    /// those that would not fit open another. A tunnel let go of frees its
    /// place, which the next takes before another connection is dialed.
    #[tokio::test]
    async fn max_streams_bounds_the_tunnels_of_each_quic_connection() {
        let echo = echo_server().await;
        // each tunnel's own authority tells its request apart
        let fake = endpoint(H3ProxyScript {
            connect_to: Some(echo),
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, &format!("{LOGIN}, max-streams=2"));
        // ports 1 to 8 of an address the fake never dials
        let to: Vec<Target> = (1..=8)
            .map(|port| Target::new(HostName::parse("192.0.2.1"), port))
            .collect();
        let (a, b, c, d, e) = tokio::join!(
            tunnel(&out, &to[0]),
            tunnel(&out, &to[1]),
            tunnel(&out, &to[2]),
            tunnel(&out, &to[3]),
            tunnel(&out, &to[4])
        );
        let mut tunnels = vec![a, b, c, d, e];
        for stream in &mut tunnels {
            round_trip(stream, b"five at once").await;
        }
        assert_eq!(carried(&fake), [1, 2, 2]);
        // the one place left
        tunnels.push(tunnel(&out, &to[5]).await);
        assert_eq!(carried(&fake), [2, 2, 2]);
        // the place of the tunnel to port 1, let go of
        let freed = connection_of(&fake, "192.0.2.1:1");
        drop(tunnels.remove(0));
        tunnels.push(tunnel(&out, &to[6]).await);
        assert_eq!(connection_of(&fake, "192.0.2.1:7"), freed);
        assert_eq!(fake.connections(), 3);
        // every place taken: a fourth connection
        tunnels.push(tunnel(&out, &to[7]).await);
        assert_eq!(fake.connections(), 4);
        for stream in &mut tunnels {
            round_trip(stream, b"all still here").await;
        }
    }

    /// Design 3.5: a server going away (an HTTP/3 GOAWAY) takes no new
    /// tunnel: the next goes on a new connection, while one under way on
    /// the old runs to its end; the client closes the old connection once
    /// nothing holds it.
    #[tokio::test]
    async fn after_a_goaway_new_tunnels_go_on_a_new_connection_and_old_ones_finish() {
        let echo = echo_server().await;
        let fake = endpoint(H3ProxyScript {
            goaway_after: Some(1),
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, LOGIN);
        // held from before the first tunnel, to see its GOAWAY come
        let lease = timeout(BOUND, out.session.lease(&ConnectOpts::default()))
            .await
            .unwrap()
            .unwrap();
        let mut old = tunnel(&out, &target(echo)).await;
        round_trip(&mut old, b"before").await;
        let deadline = Instant::now() + BOUND;
        while out.session.serves(&lease) {
            assert!(Instant::now() < deadline, "no GOAWAY heard");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        drop(lease);
        let mut new = tunnel(&out, &target(echo)).await;
        round_trip(&mut new, b"after").await;
        round_trip(&mut old, b"still there").await;
        let carried: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
        assert_eq!(carried, [0, 1]);
        old.shutdown().await.unwrap();
        let mut rest = Vec::new();
        timeout(BOUND, old.read_to_end(&mut rest))
            .await
            .expect("the end of the tunnel")
            .unwrap();
        assert!(rest.is_empty());
        drop(old);
        let closed = timeout(BOUND, fake.connection(0).closed())
            .await
            .expect("closed");
        assert!(
            matches!(&closed, ConnectionError::ApplicationClosed(close)
                if close.error_code == H3_NO_ERROR),
            "{closed:?}"
        );
    }

    /// Design 3.5: the server restarts — losing its connections — as a
    /// tunnel is asked for. The request finds its connection gone (a
    /// stateless reset answers the client's next packet) before the
    /// response came, and goes again, once, on a new connection.
    #[tokio::test]
    async fn a_request_that_finds_the_server_restarted_goes_again_on_a_new_connection() {
        let echo = echo_server().await;
        let fake = endpoint(H3ProxyScript::default());
        let out = outbound(&fake, LOGIN);
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
        let carried: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
        assert_eq!(carried, [0, 0, 1]);
        assert_eq!(
            end_of(&mut before).await.to_string(),
            "quic: the server no longer knows the connection (stateless reset)"
        );
    }

    /// `opts.timeout` bounds the whole request: here the server's response,
    /// on a connection that is up already.
    #[tokio::test]
    async fn a_server_that_never_answers_times_out() {
        let echo = echo_server().await;
        let fake = endpoint(H3ProxyScript {
            silent: true,
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, LOGIN);
        drop(
            timeout(BOUND, out.session.lease(&ConnectOpts::default()))
                .await
                .unwrap()
                .unwrap(),
        );
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
        assert_eq!(fake.requests().len(), 1, "the request went out");
    }

    /// The credentials and the configured values go to the server and
    /// nowhere else: not into the error of a refused login, nor into a log
    /// line at any level.
    #[tokio::test]
    async fn the_credentials_and_header_values_never_show() {
        const PASSWORD: &str = "s3cret-trust";
        const TOKEN: &str = "t0ken-of-the-header";
        let (log, _logging, _second) = Log::capture();
        let echo = echo_server().await;
        let fake = endpoint(H3ProxyScript {
            users: vec![("u".to_string(), PASSWORD.to_string())],
            ..H3ProxyScript::default()
        });
        let wrong = outbound(
            &fake,
            &format!("username=u, password={PASSWORD}-not, headers=X-Token:{TOKEN}"),
        );
        let err = failure(&wrong, &target(echo)).await;
        assert_eq!(err.to_string(), "trust-tunnel: authentication failed");
        let out = outbound(
            &fake,
            &format!("username=u, password={PASSWORD}, headers=X-Token:{TOKEN}"),
        );
        let mut stream = tunnel(&out, &target(echo)).await;
        round_trip(&mut stream, b"let in").await;
        let basic = basic_authorization("u", PASSWORD);
        let requests = fake.requests();
        assert_eq!(
            requests[1].header("proxy-authorization"),
            Some(basic.as_str())
        );
        assert_eq!(requests[1].header("x-token"), Some(TOKEN));
        let shown = format!("{err} {err:?}");
        let log = log.text();
        assert!(!log.is_empty());
        let wrong_basic = basic_authorization("u", &format!("{PASSWORD}-not"));
        for secret in [
            PASSWORD,
            &basic["Basic ".len()..],
            &wrong_basic["Basic ".len()..],
            TOKEN,
        ] {
            assert!(!shown.contains(secret), "{shown}");
            assert!(!log.contains(secret));
        }
    }
}
```

要点：
- 不设上限时会话的行为与原来完全相同（Hysteria 2、TUIC、MASQUE 的用例都不变）。
- 有上限时：最早的有空位的连接优先；在途拨号每次算 `cap` 个位置、同时至多 8 条；失败照 HTTP/2 会话池的整体规则交给等它的请求。
- `trust-tunnel` 的 HTTP/3 用不带数据报的 `Client::new`（endpoint 没开 QUIC 数据报）。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto-quic` → 通过（新增 17 条：会话有上限时 7 条——一起来的请求拨出所需的连接、放手的位置被最早的连接先占、每次拨号是 `cap` 个位置且至多 8 条在途、失败的拨号让等它的请求失败、离开中的连接不接新请求、旧路由的连接不接新请求、空闲的连接逐条关闭；`trust_tunnel` 10 条——带凭据与 `user-agent` 的经典 CONNECT、`headers` 替换 `user-agent` 且每条隧道重抽随机串、登录被拒说认证失败、连不上的目标与其它状态按数字说、发不出去的名字不拨号、`max-streams` 限住每条 QUIC 连接的隧道数、GOAWAY 后新隧道上新连接而旧的走完、服务端重启后重试一次、不应答时受时限约束、凭据与头的值不外泄）。
Run: `cargo test -p rurge-proto trust_tunnel` → 通过（新增 2 条）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-proto crates/rurge-proto-quic
git commit -m "feat(proto-quic): 会话的流数上限；trust-tunnel 的 HTTP/3——经典 CONNECT、max-streams 按每条 QUIC 连接计"
```

### Task 6: DoQ 与 DoH3 上游

`rurge-dns` 新增 `quic://`（RFC 9250）与 `h3://`（RFC 8484 over HTTP/3）两种上游（P13–P16）：经引导解析拨号、每个上游一条 QUIC 连接、失效重试一次、DoQ 的 EDNS(0) 填充；`W0026` 不再因它们出现。引擎的 `PipelineConnector` 先补一个直连的 `connect_udp`（Task 7 改为按规则走）。`ServerPort` 移到 `rurge_net::quic::testing` 供 rurge-dns 的假服务端重启用。

**Files:**
- Create: `crates/rurge-dns/src/upstream/{doq.rs, doh3.rs, session.rs}`（`doq.rs` / `doh3.rs` 自带用例）、`src/testing/quic.rs`
- Modify: `crates/rurge-dns/{Cargo.toml, src/bootstrap.rs, src/hosts.rs, src/message.rs, src/resolver.rs, src/testing.rs, src/upstream/mod.rs, tests/resolver.rs}`（均含用例）、`crates/rurge-net/src/{http.rs, quic/dial.rs, quic/mod.rs, quic/testing.rs}`（含用例）、`crates/rurge-proto-quic/src/testing/mod.rs`、`crates/rurge-config/src/diagnostic.rs`、`crates/rurge-engine/{src/dns_pipeline.rs, tests/pipeline.rs}`、`crates/rurge/src/cli/dns.rs`、`Cargo.lock`

**Interfaces:**
- Consumes: Task 2 的 HTTP/3 客户端（`Client::new`、`request`、`content-length` 的核对、`:authority` 不带 userinfo）；M7a 的 `rurge_net::quic::{Path::open, client_config, connect, connection_error}`；rurge-dns 的 `Upstream`、`Bootstrap`、`BootstrapConnector`、`MockDns`。
- Produces: `rurge_net::quic::Transport { keep_alive: Option<Duration>, … }` 与 `Transport::DNS`；`rurge_net::http::quic_tls_client_config(skip_verify: bool, alpn: &str) -> Result<Arc<rustls::ClientConfig>, HttpError>`；`rurge_net::quic::testing::ServerPort`（自 rurge-proto-quic 移来，那边重导出）；`UpstreamSpec::Quic { host, port }` 与 `UpstreamSpec::H3(Url)`；`DoqUpstream::new(host: &str, port: u16, connector: Arc<dyn Connector>, bootstrap: Arc<Bootstrap>, config: quinn::ClientConfig)`、`Doh3Upstream::new(url: Url, connector, bootstrap, config)`；`BootstrapConnector` 的 `as_direct` 与 `connect_udp`；测试设施 `rurge_dns::testing::{FakeDoq, DoqScript, RecordedDoqQuery, FakeDoh3, Doh3Script}`；`PipelineConnector::connect_udp`（暂时直连）

- [ ] **Step 1: 先写用例（连同假服务端）**

`rurge-net` 的测试设施与用例（`ServerPort` 移来、`keep_alive` 改为 `Option` 的字面量）：

`crates/rurge-net/src/quic/testing.rs`——把

```rust
//! either side of it, an echo server, and a server socket on several ports
//! for port hopping. Never used by production code.
```

换成

```rust
//! either side of it, an echo server, a server socket on several ports for
//! port hopping, and a port whose endpoints restart. Never used by
//! production code.
```

`crates/rurge-net/src/quic/testing.rs`——把

```rust
    AsyncUdpSocket, Connection, Endpoint, EndpointConfig, Runtime, ServerConfig, TokioRuntime,
    UdpPoller,
```

换成

```rust
    AsyncUdpSocket, Connection, ConnectionId, ConnectionIdGenerator, Endpoint, EndpointConfig,
    Runtime, ServerConfig, TokioRuntime, UdpPoller,
```

`crates/rurge-net/src/quic/testing.rs`——把

```rust
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
```

换成

```rust
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
```

`crates/rurge-net/src/quic/testing.rs`——把

```rust
        self.sockets[0].may_fragment()
    }
}

```

换成

```rust
        self.sockets[0].may_fragment()
    }
}

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
        self.endpoint_over(config, |socket| socket)
    }

    /// As `endpoint`, its packets through what `layer` lays over the port
    /// (a fake's obfuscation): a layer of its own for each endpoint.
    pub fn endpoint_over(
        &self,
        config: ServerConfig,
        layer: impl FnOnce(Arc<dyn AsyncUdpSocket>) -> Arc<dyn AsyncUdpSocket>,
    ) -> Endpoint {
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
            layer(gate),
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

`crates/rurge-net/src/quic/mod.rs`——把

```rust
                keep_alive: Duration::from_secs(10),
```

换成

```rust
                keep_alive: Some(Duration::from_secs(10)),
```

`crates/rurge-net/src/quic/mod.rs`——把

```rust
            keep_alive: Duration::from_millis(250),
```

换成

```rust
            keep_alive: Some(Duration::from_millis(250)),
```

`crates/rurge-net/src/quic/mod.rs`——把

```rust
            // past the idle timeout: no keep-alive in time
            keep_alive: Duration::from_secs(10),
```

换成

```rust
            // past the idle timeout: no keep-alive in time
            keep_alive: Some(Duration::from_secs(10)),
```

`crates/rurge-net/src/quic/dial.rs`——把

```rust
            assert!(config.contains(expected), "{expected} in {config}");
        }
    }

    /// QUIC's first packets are protected with AES-128-GCM: TLS without it
```

换成

```rust
            assert!(config.contains(expected), "{expected} in {config}");
        }
    }

    /// A DNS server's connection is the standard one, but nothing keeps it
    /// alive.
    #[test]
    fn a_dns_connection_is_not_kept_alive() {
        assert_eq!(
            Transport::DNS,
            Transport {
                keep_alive: None,
                ..Transport::STANDARD
            }
        );
        let config = format!("{:?}", Transport::DNS.config());
        for expected in [
            "keep_alive_interval: None,",
            "max_idle_timeout: Some(30000),",
            "max_concurrent_bidi_streams: 0,",
        ] {
            assert!(config.contains(expected), "{expected} in {config}");
        }
    }

    /// QUIC's first packets are protected with AES-128-GCM: TLS without it
```

`crates/rurge-net/src/http.rs`——把

```rust
        assert!(tls_client_config(true).is_ok());
    }
}
```

换成

```rust
        assert!(tls_client_config(true).is_ok());
    }

    /// A DNS server over QUIC gets TLS 1.3 alone, and the one ALPN of its
    /// protocol.
    #[test]
    fn quic_tls_client_config_is_tls_1_3_with_one_alpn() {
        for skip_verify in [false, true] {
            let cfg = quic_tls_client_config(skip_verify, "doq").unwrap();
            assert_eq!(cfg.alpn_protocols, vec![b"doq".to_vec()]);
            let debug = format!("{cfg:?}");
            assert!(debug.contains("versions: [TLSv1_3]"), "{debug}");
        }
    }
}
```

`crates/rurge-proto-quic/src/testing/mod.rs`——把

```rust
//! (feature `testing`): a server port whose endpoint can be replaced, as
//! by a server that restarts, and scriptable Hysteria 2, TUIC and HTTP/3
//! CONNECT (MASQUE, TrustTunnel) servers. Never used by production code.
```

换成

```rust
//! (feature `testing`): scriptable Hysteria 2, TUIC and HTTP/3 CONNECT
//! (MASQUE, TrustTunnel) servers, on ports that can restart (`ServerPort`,
//! `rurge_net`'s). Never used by production code.
```

`crates/rurge-proto-quic/src/testing/mod.rs`——把

```rust
pub use obfs::RecordedFragment;
```

换成

```rust
pub use obfs::RecordedFragment;
pub use rurge_net::quic::testing::ServerPort;
```

`crates/rurge-proto-quic/src/testing/mod.rs`——把

```rust
};

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
        self.endpoint_over(config, |socket| socket)
    }

    /// As `endpoint`, its packets through what `layer` lays over the port
    /// (a fake's obfuscation): a layer of its own for each endpoint.
    pub fn endpoint_over(
        &self,
        config: ServerConfig,
        layer: impl FnOnce(Arc<dyn AsyncUdpSocket>) -> Arc<dyn AsyncUdpSocket>,
    ) -> Endpoint {
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
            layer(gate),
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

换成

```rust
};

```

`rurge-dns` 的假服务端与用例（假服务端要用 `quinn`，依赖边先加上；`testing` 特性带上 `rurge-net/testing`）：

`crates/rurge-dns/Cargo.toml`——把

```toml
tracing.workspace = true
```

换成

```toml
tracing.workspace = true
quinn.workspace = true
```

`crates/rurge-dns/Cargo.toml`——把

```toml
# In-process mock DNS servers (`rurge_dns::testing`); enabled by dependants' dev-dependencies.
testing = ["dep:rcgen"]
```

换成

```toml
# In-process mock DNS servers (`rurge_dns::testing`), those over QUIC on
# `rurge_net`'s loopback QUIC servers; enabled by dependants' dev-dependencies.
testing = ["dep:rcgen", "rurge-net/testing"]
```

`crates/rurge-dns/src/testing.rs`——把

```rust
//! misbehaviour (delay, drops, truncation, error codes).
```

换成

```rust
//! misbehaviour (delay, drops, truncation, error codes) — and DNS over QUIC
//! and over HTTP/3 servers that answer from such a table (`quic`).

mod quic;

pub use quic::{
    Doh3Script, DoqScript, FakeDoh3, FakeDoq, RecordedDoh3Request, RecordedDoqQuery, RecordedStop,
};
```

新建 `crates/rurge-dns/src/testing/quic.rs`：

```rust
//! DNS servers over QUIC on the loopback: `FakeDoq` (RFC 9250) and
//! `FakeDoh3` (RFC 8484 over RFC 9114). Each answers from the table of a
//! `MockDns` — whose queries it records there too — on a port of its own
//! that can restart (`ServerPort`: the connections before end in a
//! stateless reset), and records what the client sent. Its certificate is
//! for `localhost` and `127.0.0.1`, which a resolver takes only with
//! `encrypted-dns-skip-cert-verification`.
//!
//! - `FakeDoq` reads a query's stream to its FIN before it answers, as
//!   dnsproxy does, and answers with the FIN after its reply.
//! - `FakeDoh3` reads a request — head, then content to its FIN — and
//!   answers `200` with `content-type: application/dns-message` and a
//!   `content-length`, unless its script says otherwise.
//! - A query the table drops (`MockDns::set_drop_all`) is left unanswered
//!   until the client gives up on it.

use super::{MockDns, State, respond};
use crate::upstream::doh::DNS_MESSAGE;
use quinn::{Connection, Endpoint, IdleTimeout, RecvStream, SendStream, TransportConfig, VarInt};
use rurge_net::quic::h3::testing::{H3Server, ServerStream};
use rurge_net::quic::testing::{ServerPort, TestCert};
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::task::{JoinHandle, JoinSet};

// the wire as the fakes know it
const DOQ_NO_ERROR: VarInt = VarInt::from_u32(0x0);
const DOQ_INTERNAL_ERROR: VarInt = VarInt::from_u32(0x1);
const H3_NO_ERROR: VarInt = VarInt::from_u32(0x100);
/// H3_REQUEST_REJECTED: a request past the fake's GOAWAY, never served.
const H3_REQUEST_REJECTED: u64 = 0x10b;
/// More than a client sends on a query's stream: a length and a message.
const MAX_QUERY: usize = 2 + 65_535 + 1;

#[derive(Clone, Debug, Default)]
pub struct DoqScript {
    /// Answer with this message ID instead of the query's.
    pub reply_id: Option<u16>,
    /// Send these bytes, then the FIN, on each query's stream instead of
    /// the answer and its length.
    pub reply_bytes: Option<Vec<u8>>,
    /// Reset each query's stream (DOQ_INTERNAL_ERROR) instead of answering.
    pub reset: bool,
    /// Close a connection that saw nothing for this long (QUIC's idle
    /// timeout): quinn's 30 seconds unless set.
    pub idle_timeout: Option<Duration>,
}

/// A query, as the fake read it: everything the client sent on its stream
/// before the FIN.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedDoqQuery {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// Its QUIC stream ID.
    pub stream: u64,
    pub bytes: Vec<u8>,
}

impl RecordedDoqQuery {
    /// What follows the 2-byte length: the message.
    pub fn message(&self) -> &[u8] {
        self.bytes.get(2..).unwrap_or_default()
    }
}

/// A query the client gave up on, and the code of its STOP_SENDING.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedStop {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// Its QUIC stream ID.
    pub stream: u64,
    pub code: u64,
}

/// A DNS over QUIC server (ALPN `doq`).
pub struct FakeDoq {
    port: Port,
    shared: Arc<DoqShared>,
}

struct DoqShared {
    table: Arc<Mutex<State>>,
    script: DoqScript,
    queries: Mutex<Vec<RecordedDoqQuery>>,
    stops: Mutex<Vec<RecordedStop>>,
}

impl FakeDoq {
    /// On a loopback port of its own, answering from `dns`'s table.
    pub fn spawn(dns: &MockDns, script: DoqScript) -> FakeDoq {
        let fake = FakeDoq {
            port: Port::bind("doq", script.idle_timeout, DOQ_NO_ERROR),
            shared: Arc::new(DoqShared {
                table: dns.state.clone(),
                script,
                queries: Mutex::default(),
                stops: Mutex::default(),
            }),
        };
        fake.listen();
        fake
    }

    fn listen(&self) {
        let shared = self.shared.clone();
        self.port
            .listen(move |connection, number| serve_doq(connection, number, shared.clone()));
    }

    pub fn addr(&self) -> SocketAddr {
        self.port.addr()
    }

    /// Every query it read, in arrival order.
    pub fn queries(&self) -> Vec<RecordedDoqQuery> {
        self.shared.queries.lock().expect("queries").clone()
    }

    /// The queries the client gave up on while they went unanswered.
    pub fn stops(&self) -> Vec<RecordedStop> {
        self.shared.stops.lock().expect("stops").clone()
    }

    /// The connections accepted so far: QUIC's handshake done.
    pub fn connections(&self) -> usize {
        self.port.connections().len()
    }

    /// The server's side of connection `n`, 0 for the first accepted.
    pub fn connection(&self, n: usize) -> Connection {
        self.port.connections()[n].clone()
    }

    /// A new endpoint takes the port over, as after a restart: the
    /// connections of the one before are gone, and the client learns it
    /// from a stateless reset of its next packet.
    pub fn restart(&self) {
        self.listen();
    }
}

/// One connection: each of the client's streams a query, in a task of its
/// own, until the connection ends.
async fn serve_doq(connection: Connection, number: usize, shared: Arc<DoqShared>) {
    let mut queries = JoinSet::new();
    loop {
        tokio::select! {
            stream = connection.accept_bi() => match stream {
                Ok((send, recv)) => {
                    queries.spawn(answer_doq(send, recv, number, shared.clone()));
                }
                Err(_) => return,
            },
            // a query that ended; nothing to collect
            Some(_) = queries.join_next() => {}
        }
    }
}

/// A query: read to its FIN and recorded; then reset, answered as the
/// script says or from the table, or left unanswered until the client gives
/// up on it.
async fn answer_doq(
    mut send: SendStream,
    mut recv: RecvStream,
    connection: usize,
    shared: Arc<DoqShared>,
) {
    let stream = u64::from(recv.id());
    let Ok(bytes) = recv.read_to_end(MAX_QUERY).await else {
        return;
    };
    let query = RecordedDoqQuery {
        connection,
        stream,
        bytes,
    };
    shared.queries.lock().expect("queries").push(query.clone());
    if shared.script.reset {
        let _ = send.reset(DOQ_INTERNAL_ERROR);
        return;
    }
    if let Some(bytes) = &shared.script.reply_bytes {
        if send.write_all(bytes).await.is_ok() {
            let _ = send.finish();
        }
        return;
    }
    let Some((delay, mut reply)) = respond(&shared.table, query.message(), "doq") else {
        if let Ok(Some(code)) = send.stopped().await {
            shared.stops.lock().expect("stops").push(RecordedStop {
                connection,
                stream,
                code: code.into_inner(),
            });
        }
        return;
    };
    tokio::time::sleep(delay).await;
    if let Some(id) = shared.script.reply_id {
        reply[..2].copy_from_slice(&id.to_be_bytes());
    }
    let mut framed = (reply.len() as u16).to_be_bytes().to_vec();
    framed.extend_from_slice(&reply);
    if send.write_all(&framed).await.is_ok() {
        let _ = send.finish();
    }
}

#[derive(Clone, Debug, Default)]
pub struct Doh3Script {
    /// Answer with this status instead of 200.
    pub status: Option<u16>,
    /// Answer with this `content-type` instead of `application/dns-message`.
    pub content_type: Option<String>,
    /// Say this `content-length` instead of the answer's.
    pub content_length: Option<u64>,
    /// Send GOAWAY once a connection has taken this many requests: those
    /// run to their end; a later one on that connection is rejected
    /// (H3_REQUEST_REJECTED).
    pub goaway_after: Option<usize>,
    /// Close a connection that saw nothing for this long (QUIC's idle
    /// timeout): quinn's 30 seconds unless set.
    pub idle_timeout: Option<Duration>,
}

/// A request, as the fake read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedDoh3Request {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// Its QUIC stream ID.
    pub stream: u64,
    /// Its fields in order, the pseudo-header fields among them.
    pub fields: Vec<(String, String)>,
    /// Its content, to the FIN.
    pub body: Vec<u8>,
}

impl RecordedDoh3Request {
    /// The first field called `name`.
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// A DNS over HTTP/3 server (ALPN `h3`), whatever the request's path.
pub struct FakeDoh3 {
    port: Port,
    shared: Arc<Doh3Shared>,
}

struct Doh3Shared {
    table: Arc<Mutex<State>>,
    script: Doh3Script,
    requests: Mutex<Vec<RecordedDoh3Request>>,
}

impl FakeDoh3 {
    /// On a loopback port of its own, answering from `dns`'s table.
    pub fn spawn(dns: &MockDns, script: Doh3Script) -> FakeDoh3 {
        let fake = FakeDoh3 {
            port: Port::bind("h3", script.idle_timeout, H3_NO_ERROR),
            shared: Arc::new(Doh3Shared {
                table: dns.state.clone(),
                script,
                requests: Mutex::default(),
            }),
        };
        fake.listen();
        fake
    }

    fn listen(&self) {
        let shared = self.shared.clone();
        self.port
            .listen(move |connection, number| serve_doh3(connection, number, shared.clone()));
    }

    pub fn addr(&self) -> SocketAddr {
        self.port.addr()
    }

    /// Every request it read, in arrival order.
    pub fn requests(&self) -> Vec<RecordedDoh3Request> {
        self.shared.requests.lock().expect("requests").clone()
    }

    /// The connections accepted so far: QUIC's handshake done.
    pub fn connections(&self) -> usize {
        self.port.connections().len()
    }

    /// The server's side of connection `n`, 0 for the first accepted.
    pub fn connection(&self, n: usize) -> Connection {
        self.port.connections()[n].clone()
    }

    /// A new endpoint takes the port over, as after a restart: the
    /// connections of the one before are gone, and the client learns it
    /// from a stateless reset of its next packet.
    pub fn restart(&self) {
        self.listen();
    }
}

/// One connection: HTTP/3, and each request in a task of its own, until
/// the connection ends. Once it took `goaway_after` requests it sends
/// GOAWAY, and rejects any request that comes after.
async fn serve_doh3(connection: Connection, number: usize, shared: Arc<Doh3Shared>) {
    let Ok(mut h3) = H3Server::start(connection, &[]).await else {
        return;
    };
    let mut requests = JoinSet::new();
    let mut taken = 0;
    // the first stream its GOAWAY leaves unserved, once it sent one
    let mut unserved = None;
    loop {
        tokio::select! {
            stream = h3.accept() => match stream {
                Some(mut stream) => {
                    if unserved.is_some_and(|id| stream.id() >= id) {
                        stream.reset(H3_REQUEST_REJECTED);
                        continue;
                    }
                    taken += 1;
                    if shared.script.goaway_after == Some(taken) {
                        let id = stream.id() + 4;
                        let _ = h3.goaway(id).await;
                        unserved = Some(id);
                    }
                    requests.spawn(answer_doh3(stream, number, shared.clone()));
                }
                None => return,
            },
            // a request that ended; nothing to collect
            Some(_) = requests.join_next() => {}
        }
    }
}

/// A request: its head and its content to the FIN, recorded; then answered
/// from the table, or left unanswered until the client gives up on it.
async fn answer_doh3(
    mut stream: ServerStream,
    connection: usize,
    shared: Arc<Doh3Shared>,
) -> io::Result<()> {
    let fields = stream.read_headers().await?;
    let body = stream.read_body().await?;
    shared
        .requests
        .lock()
        .expect("requests")
        .push(RecordedDoh3Request {
            connection,
            stream: stream.id(),
            fields,
            body: body.clone(),
        });
    let Some((delay, reply)) = respond(&shared.table, &body, "doh3") else {
        stream.client_stopped().await;
        return Ok(());
    };
    tokio::time::sleep(delay).await;
    let script = &shared.script;
    let length = script
        .content_length
        .unwrap_or(reply.len() as u64)
        .to_string();
    let content_type = script.content_type.as_deref().unwrap_or(DNS_MESSAGE);
    stream
        .send_headers(
            script.status.unwrap_or(200),
            &[("content-type", content_type), ("content-length", &length)],
        )
        .await?;
    stream.send_data(&reply).await?;
    stream.finish();
    Ok(())
}

/// A loopback port whose endpoints present the test certificate with one
/// ALPN, and the connections they accepted, in order.
struct Port {
    cert: TestCert,
    port: ServerPort,
    alpn: &'static str,
    idle_timeout: Option<Duration>,
    /// What an endpoint closes its connections with when the fake goes.
    no_error: VarInt,
    connections: Arc<Mutex<Vec<Connection>>>,
    /// Every endpoint so far, with its accept loop: closed with the fake.
    endpoints: Mutex<Vec<(Endpoint, JoinHandle<()>)>>,
}

impl Port {
    fn bind(alpn: &'static str, idle_timeout: Option<Duration>, no_error: VarInt) -> Port {
        Port {
            cert: TestCert::new(),
            port: ServerPort::bind(),
            alpn,
            idle_timeout,
            no_error,
            connections: Arc::default(),
            endpoints: Mutex::default(),
        }
    }

    fn addr(&self) -> SocketAddr {
        self.port.addr()
    }

    fn connections(&self) -> MutexGuard<'_, Vec<Connection>> {
        self.connections.lock().expect("connections")
    }

    /// A new endpoint takes the port over — the one before falls silent —
    /// and serves each connection it accepts, in a task of its own, with
    /// `serve`, given its number.
    fn listen<F, S>(&self, serve: S)
    where
        S: Fn(Connection, usize) -> F + Send + Sync + 'static,
        F: Future<Output = ()> + Send + 'static,
    {
        let mut transport = TransportConfig::default();
        if let Some(idle) = self.idle_timeout {
            transport.max_idle_timeout(Some(
                IdleTimeout::try_from(idle).expect("an idle timeout QUIC can say"),
            ));
        }
        let mut config = self.cert.server_config(&[self.alpn]);
        config.transport_config(Arc::new(transport));
        let endpoint = self.port.endpoint(config);
        let connections = self.connections.clone();
        let serve = Arc::new(serve);
        let accepting = tokio::spawn({
            let endpoint = endpoint.clone();
            async move {
                while let Some(incoming) = endpoint.accept().await {
                    let (connections, serve) = (connections.clone(), serve.clone());
                    tokio::spawn(async move {
                        let Ok(connection) = incoming.await else {
                            return;
                        };
                        let number = {
                            let mut connections = connections.lock().expect("connections");
                            connections.push(connection.clone());
                            connections.len() - 1
                        };
                        serve(connection, number).await;
                    });
                }
            }
        });
        self.endpoints
            .lock()
            .expect("endpoints")
            .push((endpoint, accepting));
    }
}

impl Drop for Port {
    /// Every connection ends with the fake, and with them the tasks that
    /// serve them.
    fn drop(&mut self) {
        for (endpoint, accepting) in self.endpoints.get_mut().expect("endpoints").iter() {
            accepting.abort();
            endpoint.close(self.no_error, b"");
        }
    }
}
```

`crates/rurge-dns/src/upstream/mod.rs`——把

```rust
        assert!(
            UpstreamSpec::parse("h3://dns.example.com/dns-query")
                .unwrap_err()
                .contains("phase 2")
        );
        assert!(
            UpstreamSpec::parse("quic://dns.example.com")
                .unwrap_err()
                .contains("phase 2")
        );
```

换成

```rust
        assert!(matches!(
            UpstreamSpec::parse("H3://dns.example.com/dns-query").unwrap(),
            UpstreamSpec::H3(_)
        ));
        assert!(matches!(
            UpstreamSpec::parse("Quic://dns.example.com").unwrap(),
            UpstreamSpec::Quic { .. }
        ));
```

`crates/rurge-dns/src/upstream/mod.rs`——把

```rust
        assert!(UpstreamSpec::from_encrypted(&h3).is_err());
```

换成

```rust
        assert_eq!(
            UpstreamSpec::from_encrypted(&h3).unwrap(),
            UpstreamSpec::parse("h3://dns.example.com/dns-query").unwrap()
        );
        let doq = EncryptedDns::parse("quic://dns.example.com").unwrap();
        assert_eq!(
            UpstreamSpec::from_encrypted(&doq).unwrap(),
            UpstreamSpec::Quic {
                host: "dns.example.com".into(),
                port: 853
            }
        );
```

`crates/rurge-dns/src/upstream/mod.rs`——把

```rust
            UpstreamSpec::Udp("1.1.1.1:53".parse().unwrap())
        );
    }
}

```

换成

```rust
            UpstreamSpec::Udp("1.1.1.1:53".parse().unwrap())
        );
    }

    /// `quic://` names a host and a port — 853 unless it says —, nothing
    /// else (RFC 9250 4.1.1).
    #[test]
    fn a_quic_url_is_a_host_and_a_port() {
        for (url, host, port) in [
            ("quic://dns.Example.com", "dns.example.com", 853),
            ("quic://dns.example.com:8853", "dns.example.com", 8853),
            ("quic://dns.example.com/", "dns.example.com", 853),
            ("quic://[::1]:853", "::1", 853),
            ("quic://127.0.0.1", "127.0.0.1", 853),
        ] {
            assert_eq!(
                UpstreamSpec::parse(url).unwrap(),
                UpstreamSpec::Quic {
                    host: host.into(),
                    port
                },
                "{url}"
            );
        }
        for url in [
            "quic://dns.example.com/dns-query",
            "quic://dns.example.com:853/x/",
            "quic://dns.example.com?x=1",
            "quic://",
            "quic://dns.example.com:99999",
        ] {
            assert!(UpstreamSpec::parse(url).is_err(), "{url}");
        }
        assert!(
            UpstreamSpec::parse("quic://dns.example.com/dns-query")
                .unwrap_err()
                .contains("names a host and a port, nothing else")
        );
    }

    /// `h3://` is the `https://` URL of its requests: port 443 unless it
    /// says, the path `/dns-query` unless it names one, never a user name
    /// or a password — which no error repeats.
    #[test]
    fn an_h3_url_defaults_to_port_443_and_the_dns_query_path() {
        let spec = |url: &str| match UpstreamSpec::parse(url).unwrap() {
            UpstreamSpec::H3(url) => url,
            other => panic!("{other:?}"),
        };
        for url in [
            "h3://dns.example.com",
            "h3://DNS.example.com/",
            "h3://dns.example.com:443",
        ] {
            let parsed = spec(url);
            assert_eq!(
                parsed.as_str(),
                "https://dns.example.com/dns-query",
                "{url}"
            );
            assert_eq!(parsed.port_or_known_default(), Some(443));
        }
        let parsed = spec("h3://dns.example.com:8443/q?x=1#part");
        assert_eq!(parsed.as_str(), "https://dns.example.com:8443/q?x=1");
        assert_eq!(spec("h3://[::1]/dns-query").host_str(), Some("[::1]"));
        let err = UpstreamSpec::parse("h3://user:secret@dns.example.com/dns-query").unwrap_err();
        assert_eq!(
            err,
            "`h3://dns.example.com/dns-query`: an h3:// URL cannot carry a user name or a password"
        );
        assert!(UpstreamSpec::parse("h3://user@dns.example.com").is_err());
        assert!(UpstreamSpec::parse("h3://").is_err());
    }

    #[test]
    fn quic_and_h3_names_hosts_and_bootstrap_need() {
        let doq = UpstreamSpec::parse("quic://dns.example.com").unwrap();
        assert_eq!(doq.name(), "quic://dns.example.com:853");
        assert_eq!(doq.host(), Some("dns.example.com"));
        assert!(doq.needs_bootstrap() && !doq.is_traditional());
        let doq_v6 = UpstreamSpec::parse("quic://[::1]:8853").unwrap();
        assert_eq!(doq_v6.name(), "quic://[::1]:8853");
        assert!(!doq_v6.needs_bootstrap());
        let doh3 = UpstreamSpec::parse("h3://dns.example.com").unwrap();
        assert_eq!(doh3.name(), "h3://dns.example.com/dns-query");
        assert_eq!(doh3.host(), Some("dns.example.com"));
        assert!(doh3.needs_bootstrap());
        let doh3_ip = UpstreamSpec::parse("h3://127.0.0.1:8443/q").unwrap();
        assert_eq!(doh3_ip.name(), "h3://127.0.0.1:8443/q");
        assert!(!doh3_ip.needs_bootstrap());
    }
}

```

`crates/rurge-dns/src/message.rs`——把

```rust
        assert!(ids.len() > 8, "{ids:?}");
    }
}
```

换成

```rust
        assert!(ids.len() > 8, "{ids:?}");
    }

    /// The EDNS(0) padding of `wire`: `None` without any.
    fn padding(wire: &[u8]) -> Option<Vec<u8>> {
        let edns = Message::from_vec(wire).unwrap().edns?;
        match edns.option(EdnsCode::Padding)? {
            EdnsOption::Unknown(12, padding) => Some(padding.clone()),
            other => panic!("padding read as {other:?}"),
        }
    }

    /// DoQ's query is the resolver's with ID 0, padded with zeros to the
    /// next multiple of 128 bytes — the option's own four bytes counted,
    /// no padding at all when they make the multiple.
    #[test]
    fn a_doq_query_has_id_0_and_is_padded_to_128_bytes() {
        // 97 bytes of name: 124 bytes of query, 128 with an empty option
        let fits = format!("{}.{}", "a".repeat(63), "b".repeat(31));
        let long = ["c".repeat(63), "d".repeat(63), "e".repeat(63)].join(".");
        for (name, len, padded) in [("a.test", 128, 89), (&fits, 128, 0), (&long, 256, 32)] {
            let question = q(name, Qtype::Aaaa);
            let wire = build_query(0x1234, &question).unwrap();
            let doq = doq_query(&wire).unwrap();
            assert_eq!(doq.len(), len, "{name}");
            assert_eq!(wire_id(&doq), Some(0));
            assert_eq!(parse_query(&doq).unwrap(), (0, question));
            assert_eq!(padding(&doq), Some(vec![0; padded]), "{name}");
        }
    }

    /// Padding the query had is replaced and its edns-tcp-keepalive left
    /// out (RFC 9250 5.5.2); a query without an OPT record gets one.
    #[test]
    fn a_doq_query_replaces_padding_and_keepalive_and_has_an_opt_record() {
        let mut m = Message::new(0x4242, MessageType::Query, OpCode::Query);
        m.add_query(Query::query(fqdn("a.test").unwrap(), RecordType::A));
        let mut edns = Edns::new();
        edns.options_mut().insert(EdnsOption::Unknown(
            EdnsCode::Keepalive.into(),
            vec![0, 100],
        ));
        edns.options_mut().insert(EdnsOption::Unknown(
            EdnsCode::Padding.into(),
            vec![0xff; 300],
        ));
        m.set_edns(edns);
        let doq = doq_query(&m.to_vec().unwrap()).unwrap();
        assert_eq!(doq.len(), 128);
        assert_eq!(padding(&doq).map(|p| p.iter().all(|&b| b == 0)), Some(true));
        let edns = Message::from_vec(&doq).unwrap().edns.unwrap();
        assert!(edns.option(EdnsCode::Keepalive).is_none());

        let mut bare = Message::new(7, MessageType::Query, OpCode::Query);
        bare.add_query(Query::query(fqdn("a.test").unwrap(), RecordType::A));
        let doq = doq_query(&bare.to_vec().unwrap()).unwrap();
        assert_eq!(doq.len(), 128);
        assert!(padding(&doq).is_some());
        assert!(doq_query(b"nope").is_err());
    }
}
```

`crates/rurge-dns/src/bootstrap.rs`——把

```rust
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
```

换成

```rust
    use std::future::poll_fn;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadBuf};
    use tokio::net::{TcpListener, UdpSocket};
```

`crates/rurge-dns/src/bootstrap.rs`——把

```rust
        assert!(!err.to_string().is_empty());
    }
}
```

换成

```rust
        assert!(!err.to_string().is_empty());
    }

    /// UDP goes as TCP does: a name to its address through the bootstrap,
    /// an address as it is. A QUIC path sees the inner connector's
    /// directness.
    #[tokio::test]
    async fn connector_carries_udp_to_the_resolved_address() {
        let echo = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = echo.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut buf = [0u8; 64];
            while let Ok((n, peer)) = echo.recv_from(&mut buf).await {
                let _ = echo.send_to(&buf[..n], peer).await;
            }
        });
        let dns = MockDns::spawn().await;
        dns.set("doq.example", &["127.0.0.1"], &[], 60);
        let bootstrap = Bootstrap::new(vec![Arc::new(UdpUpstream::new(dns.addr()))], false, fast());
        let connector = BootstrapConnector::new(
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
            bootstrap,
        );
        assert!(connector.as_direct().is_some());
        for host in ["doq.example", "127.0.0.1"] {
            let datagram = connector
                .connect_udp(
                    &Target::new(HostName::parse(host), port),
                    &ConnectOpts::default(),
                )
                .await
                .unwrap();
            poll_fn(|cx| datagram.poll_send(cx, b"ping")).await.unwrap();
            let mut buf = [0u8; 8];
            let mut read = ReadBuf::new(&mut buf);
            tokio::time::timeout(
                Duration::from_secs(5),
                poll_fn(|cx| datagram.poll_recv(cx, &mut read)),
            )
            .await
            .expect("the echo within the bound")
            .unwrap();
            assert_eq!(read.filled(), b"ping", "{host}");
        }
        assert_eq!(dns.query_count("doq.example", Qtype::A), 1);
        let err = connector
            .connect_udp(
                &Target::new(HostName::parse("nx.example"), port),
                &ConnectOpts::default(),
            )
            .await
            .err()
            .unwrap();
        assert!(err.to_string().contains("nx.example"), "{err}");
    }
}
```

`crates/rurge-dns/src/hosts.rs`——把

```rust
            "a.com = server:1.1.1.1, tls://dns.example.com\nb.com = server:system\nc.com = server:syslib\nd.com = script:my-script\ne.com = server:h3://dns.example.com/dns-query\nf.com = server:force-syslib\n",
```

换成

```rust
            "a.com = server:1.1.1.1, tls://dns.example.com\nb.com = server:system\nc.com = server:syslib\nd.com = script:my-script\ne.com = server:quic://dns.example.com/dns-query\nf.com = server:force-syslib\n",
```

`crates/rurge-dns/src/hosts.rs`——把

```rust
            HostAction::System(SystemMode::ForceSyslib)
```

换成

```rust
            HostAction::System(SystemMode::ForceSyslib)
        );
    }

    /// `quic://` and `h3://` servers are as good as the others: no W0026.
    #[tokio::test]
    async fn quic_and_h3_servers_need_no_warning() {
        let dir = tempfile::tempdir().unwrap();
        let (map, diags) = build(
            dir.path(),
            "a.com = server:quic://dns.example.com\nb.com = server:h3://dns.example.com, 1.1.1.1\n",
        );
        assert!(
            diags.is_empty(),
            "{:?}",
            diags.iter().map(|d| d.to_string()).collect::<Vec<_>>()
        );
        let servers = |name: &str| match map.lookup(name).unwrap().action {
            HostAction::Servers(specs) => specs.iter().map(UpstreamSpec::name).collect::<Vec<_>>(),
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(servers("a.com"), ["quic://dns.example.com:853"]);
        assert_eq!(
            servers("b.com"),
            ["h3://dns.example.com/dns-query", "udp://1.1.1.1:53"]
```

`crates/rurge-dns/src/resolver.rs`——把

```rust
                "dns-server = {}, [::1]:5353\nencrypted-dns-server = h3://dns.example/dns-query",
```

换成

```rust
                "dns-server = {}, [::1]:5353\nencrypted-dns-server = quic://dns.example/dns-query",
```

`crates/rurge-dns/src/resolver.rs`——把

```rust
            vec![format!("udp://{}", mock.addr())]
        );
    }
```

换成

```rust
            vec![format!("udp://{}", mock.addr())]
        );
    }

    /// `quic://` and `h3://` servers are upstreams like the others, in
    /// `encrypted-dns-server` and in `[Host]`: no W0026.
    #[tokio::test]
    async fn quic_and_h3_upstreams_are_built_without_a_warning() {
        let e = env(&profile(
            "encrypted-dns-server = quic://127.0.0.1, h3://dns.example:8443",
            "quic.test = server:quic://[::1]:8853",
        ));
        let (r, diags) = resolver(&e, StaticSystemDns::default());
        assert!(
            diags.is_empty(),
            "{:?}",
            diags.iter().map(|d| d.to_string()).collect::<Vec<_>>()
        );
        assert_eq!(
            r.primary_upstreams(),
            vec![
                "quic://127.0.0.1:853".to_string(),
                "h3://dns.example:8443/dns-query".to_string()
            ]
        );
        assert!(matches!(
            r.host_lookup("quic.test").map(|hit| hit.action),
            Some(HostAction::Servers(specs)) if specs == [UpstreamSpec::Quic {
                host: "::1".into(),
                port: 8853
            }]
        ));
    }
```

`crates/rurge-dns/tests/resolver.rs`——把

```rust
//! transport (UDP, tcp://, tls://, https://), the manual's retry and
//! partial-result timing with the default 1 s / 5 attempts, `[Host]`
//! `server:` URLs bootstrapped by traditional upstreams, and set keys.

use rurge_config::config::{LoadOptions, from_text};
use rurge_dns::message::{Qtype, Question, Rcode, build_response};
use rurge_dns::system::StaticSystemDns;
use rurge_dns::testing::MockDns;
```

换成

```rust
//! transport (UDP, tcp://, tls://, https://, quic://, h3://), the manual's
//! retry and partial-result timing with the default 1 s / 5 attempts,
//! `[Host]` `server:` URLs bootstrapped by traditional upstreams, and set
//! keys.

use rurge_config::config::{LoadOptions, from_text};
use rurge_dns::message::{Qtype, Question, Rcode, build_response, parse_query};
use rurge_dns::system::StaticSystemDns;
use rurge_dns::testing::{Doh3Script, DoqScript, FakeDoh3, FakeDoq, MockDns};
```

`crates/rurge-dns/tests/resolver.rs`——把

```rust
    assert_eq!(corp.queries()[0].1, "tcp");
}

#[tokio::test]
async fn resend_after_one_second_and_first_answer_wins() {
```

换成

```rust
    assert_eq!(corp.queries()[0].1, "tcp");
}

/// A profile whose encrypted DNS server is `url`, its certificate
/// unverified (the fakes' is self-signed), A and AAAA asked.
fn encrypted_profile(url: &str) -> String {
    profile(
        &format!(
            "encrypted-dns-server = {url}\nencrypted-dns-skip-cert-verification = true\nipv6 = true"
        ),
        "",
    )
}

fn with_ipv6() -> StaticSystemDns {
    StaticSystemDns {
        has_ipv6: true,
        ..StaticSystemDns::default()
    }
}

/// DoQ: A and AAAA on one connection, each on a stream of its own, with ID
/// 0 and padded; the caller's IDs come back (the answers match them).
#[tokio::test]
async fn doq_answers_a_and_aaaa() {
    let dns = MockDns::spawn().await;
    dns.set("a.test", &["10.0.0.1"], &["fd00::1"], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let url = format!("quic://127.0.0.1:{}", doq.addr().port());
    let (_d, r) = resolver(&encrypted_profile(&url), with_ipv6());
    assert_eq!(r.primary_upstreams(), vec![url.clone()]);
    let a = lookup(&r, "a.test").await;
    assert_eq!(a.addrs(), vec![ip("10.0.0.1"), ip("fd00::1")]);
    assert_eq!(a.source, Source::Upstream(url));
    assert_eq!(doq.connections(), 1);
    // a resend may have come too, should an answer have taken a second
    let asked: Vec<_> = doq
        .queries()
        .iter()
        .map(|q| {
            assert_eq!(q.message().len() % 128, 0, "padded");
            let (id, question) = parse_query(q.message()).unwrap();
            (q.connection, id, question.qtype)
        })
        .collect();
    for qtype in [Qtype::A, Qtype::Aaaa] {
        assert!(asked.contains(&(0, 0, qtype)), "{asked:?}");
    }
    assert!(
        asked
            .iter()
            .all(|&(connection, id, _)| (connection, id) == (0, 0))
    );
}

/// DoH3: A and AAAA on one connection, POSTed to the default path with ID
/// 0.
#[tokio::test]
async fn doh3_answers_a_and_aaaa() {
    let dns = MockDns::spawn().await;
    dns.set("a.test", &["10.0.0.2"], &["fd00::2"], 60);
    let doh3 = FakeDoh3::spawn(&dns, Doh3Script::default());
    let url = format!("h3://127.0.0.1:{}", doh3.addr().port());
    let (_d, r) = resolver(&encrypted_profile(&url), with_ipv6());
    let name = format!("{url}/dns-query");
    assert_eq!(r.primary_upstreams(), vec![name.clone()]);
    let a = lookup(&r, "a.test").await;
    assert_eq!(a.addrs(), vec![ip("10.0.0.2"), ip("fd00::2")]);
    assert_eq!(a.source, Source::Upstream(name));
    assert_eq!(doh3.connections(), 1);
    let requests = doh3.requests();
    assert!(requests.len() >= 2, "{requests:?}");
    for request in &requests {
        assert_eq!(request.connection, 0);
        assert_eq!(request.field(":method"), Some("POST"));
        assert_eq!(request.field(":path"), Some("/dns-query"));
        assert_eq!(parse_query(&request.body).unwrap().0, 0);
    }
}

/// A DoQ server named by host is resolved through the traditional server,
/// which is asked nothing else.
#[tokio::test]
async fn a_doq_server_named_by_host_is_bootstrapped() {
    let udp = MockDns::spawn().await;
    udp.set("doq.test", &["127.0.0.1"], &[], 60);
    udp.set("a.test", &["10.0.0.99"], &[], 60);
    let dns = MockDns::spawn().await;
    dns.set("a.test", &["10.0.0.42"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let (_d, r) = resolver(
        &profile(
            &format!(
                "dns-server = {}\nencrypted-dns-server = quic://doq.test:{}\nencrypted-dns-skip-cert-verification = true\nipv6 = false",
                udp.addr(),
                doq.addr().port()
            ),
            "",
        ),
        StaticSystemDns::default(),
    );
    assert_eq!(lookup(&r, "a.test").await.addrs(), vec![ip("10.0.0.42")]);
    assert_eq!(udp.query_count("doq.test", Qtype::A), 1);
    assert_eq!(udp.query_count("a.test", Qtype::A), 0);
    assert!(dns.query_count("a.test", Qtype::A) >= 1);
}

/// `[Host]` can send a name to a DoQ server.
#[tokio::test]
async fn a_host_entry_names_a_doq_server() {
    let udp = MockDns::spawn().await;
    udp.set("corp.test", &["10.0.0.99"], &[], 60);
    let dns = MockDns::spawn().await;
    dns.set("corp.test", &["10.10.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let (_d, r) = resolver(
        &profile(
            &format!(
                "dns-server = {}\nencrypted-dns-skip-cert-verification = true\nipv6 = false",
                udp.addr()
            ),
            &format!("corp.test = server:quic://127.0.0.1:{}", doq.addr().port()),
        ),
        StaticSystemDns::default(),
    );
    let c = lookup(&r, "corp.test").await;
    assert_eq!(c.addrs(), vec![ip("10.10.0.1")]);
    assert_eq!(c.source, Source::Host(HostKind::Server));
    assert_eq!(udp.query_count("corp.test", Qtype::A), 0);
    assert!(!doq.queries().is_empty());
}

#[tokio::test]
async fn resend_after_one_second_and_first_answer_wins() {
```

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
use rurge_dns::testing::MockDns;
```

换成

```rust
use rurge_dns::testing::{DoqScript, FakeDoq, MockDns};
```

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
        "nothing in this chain is resolved locally: {internal:?}"
    );
}

#[tokio::test]
async fn persisted_group_selection_is_honored() {
```

换成

```rust
        "nothing in this chain is resolved locally: {internal:?}"
    );
}

/// In follow mode a DoQ server is reached all the same: the pipeline hands
/// the QUIC carrier to its direct fallback.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_follow_mode_resolver_reaches_a_doq_server() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\nencrypted-dns-follow-outbound-mode = true\n\
encrypted-dns-server = quic://127.0.0.1:{}\nencrypted-dns-skip-cert-verification = true\n\
ipv6 = false\n[Proxy]\n[Proxy Group]\n[Rule]\nFINAL,DIRECT\n",
        doq.addr().port()
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = tokio::time::timeout(
        Duration::from_secs(10),
        engine
            .runtime()
            .stack
            .resolver
            .lookup("target.test", rurge_dns::resolver::LookupOpts::default()),
    )
    .await
    .expect("the lookup within the bound")
    .expect("an answer over DoQ");
    assert_eq!(res.v4, ["127.0.0.1".parse::<std::net::Ipv4Addr>().unwrap()]);
    assert_eq!(doq.connections(), 1);
    assert!(!doq.queries().is_empty());
}

#[tokio::test]
async fn persisted_group_selection_is_honored() {
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-dns --lib`
Expected: FAIL——`UpstreamSpec::{Quic, H3}`、DoQ 查询的写法 `doq_query` 与它用到的 EDNS 类型由 Step 3 引入，编译不过（假服务端与 `rurge-net` 的测试设施此时已能编译）：

```text
error[E0433]: failed to resolve: use of undeclared type `EdnsCode`
error[E0433]: failed to resolve: use of undeclared type `EdnsOption`
error[E0425]: cannot find function `doq_query` in this scope
error[E0599]: no variant named `Quic` found for enum `upstream::UpstreamSpec`
error[E0599]: no variant or associated item named `H3` found for enum `upstream::UpstreamSpec` in the current scope
error: could not compile `rurge-dns` (lib test) due to 17 previous errors
```

- [ ] **Step 3: 实现**

`rurge-net`：不保活的 DNS 传输参数、TLS 1.3 的 QUIC 客户端配置：

`crates/rurge-net/src/quic/dial.rs`——把

```rust
/// every QUIC policy uses; tests shorten its times.
```

换成

```rust
/// every QUIC policy uses, `DNS` what a DNS server's connection uses; tests
/// shorten their times.
```

`crates/rurge-net/src/quic/dial.rs`——把

```rust
    /// How often an otherwise idle connection proves it is alive.
    pub keep_alive: Duration,
```

换成

```rust
    /// How often an otherwise idle connection proves it is alive; `None`
    /// leaves it to idle out.
    pub keep_alive: Option<Duration>,
```

`crates/rurge-net/src/quic/dial.rs`——把

```rust
        keep_alive: Duration::from_secs(10),
        stream_window: 8 << 20,
        connection_window: 20 << 20,
```

换成

```rust
        keep_alive: Some(Duration::from_secs(10)),
        stream_window: 8 << 20,
        connection_window: 20 << 20,
    };

    /// A DNS server's connection (DoQ, DoH3): as `STANDARD`, but nothing
    /// keeps it alive — unused, it idles out at the shorter of both ends'
    /// idle timeouts, and the next query dials again.
    pub const DNS: Transport = Transport {
        keep_alive: None,
        ..Transport::STANDARD
```

`crates/rurge-net/src/quic/dial.rs`——把

```rust
            .keep_alive_interval(Some(self.keep_alive))
```

换成

```rust
            .keep_alive_interval(self.keep_alive)
```

`crates/rurge-net/src/http.rs`——把

```rust
pub(crate) fn build_tls_config(skip_verify: bool) -> Result<rustls::ClientConfig, HttpError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| HttpError::Tls(e.to_string()))?;
    let mut config = if skip_verify {
        tracing::warn!("TLS certificate verification is disabled for the internal HTTP client");
```

换成

```rust
/// A client configuration of `versions` that verifies the server with the
/// shared roots, or — `skip_verify` — not at all; no ALPN.
fn verifying_config(
    skip_verify: bool,
    versions: &[&'static rustls::SupportedProtocolVersion],
) -> Result<rustls::ClientConfig, HttpError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(versions)
        .map_err(|e| HttpError::Tls(e.to_string()))?;
    Ok(if skip_verify {
```

`crates/rurge-net/src/http.rs`——把

```rust
            .with_no_client_auth()
    };
```

换成

```rust
            .with_no_client_auth()
    })
}

pub(crate) fn build_tls_config(skip_verify: bool) -> Result<rustls::ClientConfig, HttpError> {
    if skip_verify {
        tracing::warn!("TLS certificate verification is disabled for the internal HTTP client");
    }
    let mut config = verifying_config(skip_verify, rustls::DEFAULT_VERSIONS)?;
```

`crates/rurge-net/src/http.rs`——把

```rust
    build_tls_config(skip_verify).map(Arc::new)
```

换成

```rust
    build_tls_config(skip_verify).map(Arc::new)
}

/// The TLS of a DNS server reached over QUIC (DoQ, DoH3): verified as
/// `tls_client_config` verifies, on TLS 1.3 alone — which QUIC requires —,
/// offering `alpn` alone.
pub fn quic_tls_client_config(
    skip_verify: bool,
    alpn: &str,
) -> Result<Arc<rustls::ClientConfig>, HttpError> {
    let mut config = verifying_config(skip_verify, &[&rustls::version::TLS13])?;
    config.alpn_protocols = vec![alpn.as_bytes().to_vec()];
    Ok(Arc::new(config))
```

`W0026` 的说明不再提这两种 scheme：

`crates/rurge-config/src/diagnostic.rs`——把

```rust
    /// A DNS upstream the current version cannot use (h3:// / quic://, or unparsable).
```

换成

```rust
    /// A DNS upstream that cannot be used: a URL that does not parse, or a
    /// transport whose client could not be built.
```

`rurge-dns`：两种上游、它们共用的 QUIC 连接、引导解析与解析器的接入：

`crates/rurge-dns/src/upstream/mod.rs`——把

```rust
pub mod doh;
```

换成

```rust
pub mod doh;
pub mod doh3;
pub mod doq;
mod session;
```

`crates/rurge-dns/src/upstream/mod.rs`——把

```rust
use rurge_config::general::{DnsServer, EncryptedDns, EncryptedDnsScheme};
```

换成

```rust
use rurge_config::general::{DnsServer, EncryptedDns};
```

`crates/rurge-dns/src/upstream/mod.rs`——把

```rust
use url::Url;
```

换成

```rust
use url::{Position, Url};
```

`crates/rurge-dns/src/upstream/mod.rs`——把

```rust
    Tcp { host: String, port: u16 },
    Tls { host: String, port: u16 },
    Https(Url),
}
```

换成

```rust
    Tcp {
        host: String,
        port: u16,
    },
    Tls {
        host: String,
        port: u16,
    },
    Https(Url),
    /// DNS over dedicated QUIC (RFC 9250): `quic://host[:port]`, port 853
    /// unless it says.
    Quic {
        host: String,
        port: u16,
    },
    /// DNS over HTTP/3: an `h3://` URL as the `https` one its requests go
    /// to — port 443 unless it says, the path `/dns-query` unless it names
    /// one.
    H3(Url),
}

/// The path of an `h3://` URL that names none, as sing-box, hickory and
/// CoreDNS take it (RFC 8484 itself sets none).
const DEFAULT_H3_PATH: &str = "/dns-query";
```

`crates/rurge-dns/src/upstream/mod.rs`——把

```rust
    Ok((host.trim_end_matches('.').to_ascii_lowercase(), port))
}

impl UpstreamSpec {
```

换成

```rust
    Ok((host.trim_end_matches('.').to_ascii_lowercase(), port))
}

/// An `h3://` URL — `rest` follows the scheme — as the `https` one its
/// requests go to: no user name or password, which HTTP/3 has no place to
/// send (RFC 9114 4.3.1), no fragment, `DEFAULT_H3_PATH` when it names no
/// path.
fn h3_url(s: &str, rest: &str) -> Result<Url, String> {
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    if let Some(at) = authority.rfind('@') {
        // quoted without what it carries
        return Err(format!(
            "`h3://{}`: an h3:// URL cannot carry a user name or a password",
            &rest[at + 1..]
        ));
    }
    let mut url = Url::parse(&format!("https://{rest}")).map_err(|e| format!("`{s}`: {e}"))?;
    if url.path() == "/" {
        url.set_path(DEFAULT_H3_PATH);
    }
    url.set_fragment(None);
    Ok(url)
}

/// What an `H3` upstream is called: its URL with the scheme it was written
/// with.
fn h3_name(url: &Url) -> String {
    format!("h3://{}", &url[Position::BeforeHost..])
}

/// `host:port`, an IPv6 address in brackets.
fn host_port_text(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

impl UpstreamSpec {
```

`crates/rurge-dns/src/upstream/mod.rs`——把

```rust
        if lower.starts_with("h3://") || lower.starts_with("quic://") {
            return Err(format!(
                "`{s}`: h3:// and quic:// upstreams are not supported until phase 2"
            ));
```

换成

```rust
        if let Some(rest) = strip_prefix_ci(s, "h3://") {
            return h3_url(s, rest).map(UpstreamSpec::H3);
        }
        if let Some(rest) = strip_prefix_ci(s, "quic://") {
            if rest.trim_end_matches('/').contains(['/', '?', '#']) {
                return Err(format!(
                    "`{s}`: a quic:// URL names a host and a port, nothing else"
                ));
            }
            let (host, port) = host_port(rest, 853).map_err(|e| format!("`{s}`: {e}"))?;
            return Ok(UpstreamSpec::Quic { host, port });
```

`crates/rurge-dns/src/upstream/mod.rs`——把

```rust
            "`{s}`: expected ip[:port], tcp://, tls:// or https://"
```

换成

```rust
            "`{s}`: expected ip[:port], tcp://, tls://, https://, h3:// or quic://"
```

`crates/rurge-dns/src/upstream/mod.rs`——把

```rust
        match e.scheme {
            EncryptedDnsScheme::H3 | EncryptedDnsScheme::Quic => Err(format!(
                "`{}`: h3:// and quic:// upstreams are not supported until phase 2",
                e.url
            )),
            _ => UpstreamSpec::parse(&e.url),
        }
```

换成

```rust
        UpstreamSpec::parse(&e.url)
```

`crates/rurge-dns/src/upstream/mod.rs`——把

```rust
            UpstreamSpec::Https(url) => url.as_str().to_string(),
```

换成

```rust
            UpstreamSpec::Https(url) => url.as_str().to_string(),
            UpstreamSpec::Quic { host, port } => format!("quic://{}", host_port_text(host, *port)),
            UpstreamSpec::H3(url) => h3_name(url),
```

`crates/rurge-dns/src/upstream/mod.rs`——把

```rust
            UpstreamSpec::Tcp { host, .. } | UpstreamSpec::Tls { host, .. } => Some(host),
            UpstreamSpec::Https(url) => url.host_str(),
```

换成

```rust
            UpstreamSpec::Tcp { host, .. }
            | UpstreamSpec::Tls { host, .. }
            | UpstreamSpec::Quic { host, .. } => Some(host),
            UpstreamSpec::Https(url) | UpstreamSpec::H3(url) => url.host_str(),
```

`crates/rurge-dns/src/message.rs`——把

```rust
//! messages beyond `wire_id` / `wire_truncated`.
```

换成

```rust
//! messages beyond `wire_id` / `wire_truncated`, and DoQ's `doq_query`.
```

`crates/rurge-dns/src/message.rs`——把

```rust
};
use hickory_proto::rr::rdata::{A, AAAA};
```

换成

```rust
};
use hickory_proto::rr::rdata::opt::{EdnsCode, EdnsOption};
use hickory_proto::rr::rdata::{A, AAAA};
```

`crates/rurge-dns/src/message.rs`——把

```rust
    bytes.len() >= 12 && bytes[2] & 0x02 != 0
}

#[cfg(test)]
```

换成

```rust
    bytes.len() >= 12 && bytes[2] & 0x02 != 0
}

/// What DNS over QUIC pads its queries to a multiple of: "Clients SHOULD
/// pad queries to the closest multiple of 128 octets" (RFC 8467 4.1, the
/// policy RFC 9250 5.4 points to).
pub const DOQ_PADDING_BLOCK: usize = 128;

/// The query `wire` as DNS over QUIC sends it (RFC 9250): message ID 0
/// (4.2.1), no edns-tcp-keepalive option (5.5.2), and an EDNS(0) Padding
/// option (RFC 7830) of zeros that makes it a multiple of
/// `DOQ_PADDING_BLOCK` bytes long (5.4) — in place of any it had, in the
/// OPT record it has or in one added.
pub fn doq_query(wire: &[u8]) -> Result<Vec<u8>, CodecError> {
    let mut m = Message::from_vec(wire).map_err(codec)?;
    m.metadata.id = 0;
    if let Some(edns) = &mut m.edns {
        edns.options_mut().remove(EdnsCode::Keepalive);
    }
    // an empty option first: its code and length are bytes of the message
    // too
    set_padding(&mut m, 0);
    let unpadded = m.to_vec().map_err(codec)?.len();
    set_padding(
        &mut m,
        (DOQ_PADDING_BLOCK - unpadded % DOQ_PADDING_BLOCK) % DOQ_PADDING_BLOCK,
    );
    m.to_vec().map_err(codec)
}

/// `len` zeros of EDNS(0) padding in `m`, in place of any it had.
fn set_padding(m: &mut Message, len: usize) {
    let options = m
        .edns
        .get_or_insert_with(|| {
            let mut edns = Edns::new();
            edns.set_max_payload(DEFAULT_MAX_PAYLOAD_LEN);
            edns
        })
        .options_mut();
    options.remove(EdnsCode::Padding);
    options.insert(EdnsOption::Unknown(EdnsCode::Padding.into(), vec![0; len]));
}

#[cfg(test)]
```

`crates/rurge-dns/src/bootstrap.rs`——把

```rust
//! (`tcp://` / `tls://` / `https://`) are resolved only through traditional
//! upstreams — the plain UDP servers, or the system's — with a small cache
//! (minimum TTL 60 s). `BootstrapConnector` wraps the injected `Connector` so
//! DoT / DoH connections dial the pre-resolved address while TLS still sees
//! the hostname.
```

换成

```rust
//! (`tcp://` / `tls://` / `https://` / `h3://` / `quic://`) are resolved only
//! through traditional upstreams — the plain UDP servers, or the system's —
//! with a small cache (minimum TTL 60 s). `BootstrapConnector` wraps the
//! injected `Connector` so DoT / DoH connections dial the pre-resolved
//! address while TLS still sees the hostname; the DoQ / DoH3 upstreams
//! resolve their server through the bootstrap themselves and open their
//! QUIC paths on it to an address.
```

`crates/rurge-dns/src/bootstrap.rs`——把

```rust
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target, interleave};
```

换成

```rust
use rurge_net::connector::{
    BoxedDatagram, BoxedStream, ConnectOpts, Connector, DirectConnector, Target, interleave,
};
use std::future::Future;
```

`crates/rurge-dns/src/bootstrap.rs`——把

```rust
        BootstrapConnector { inner, bootstrap }
    }
}
```

换成

```rust
        BootstrapConnector { inner, bootstrap }
    }

    /// `dial` to `target`: an address as it is, a name to each of its
    /// addresses through the bootstrap in turn, IPv4 first, until one
    /// answers.
    async fn each_address<T, F>(&self, target: &Target, dial: impl Fn(Target) -> F) -> io::Result<T>
    where
        F: Future<Output = io::Result<T>>,
    {
        let host = match &target.host {
            HostName::Ip(_) => return dial(target.clone()).await,
            HostName::Domain(d) => d,
        };
        let ips = self
            .bootstrap
            .resolve(host)
            .await
            .map_err(|e| io::Error::other(e.to_string()))?;
        let mut last = io::Error::new(io::ErrorKind::NotFound, format!("no addresses for {host}"));
        for ip in interleave(ips, false) {
            match dial(Target::new(HostName::Ip(ip), target.port)).await {
                Ok(done) => return Ok(done),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}
```

`crates/rurge-dns/src/bootstrap.rs`——把

```rust
        Box::pin(async move {
            let host = match &target.host {
                HostName::Ip(_) => return self.inner.connect(target, opts).await,
                HostName::Domain(d) => d.clone(),
            };
            let ips = self
                .bootstrap
                .resolve(&host)
                .await
                .map_err(|e| io::Error::other(e.to_string()))?;
            let mut last =
                io::Error::new(io::ErrorKind::NotFound, format!("no addresses for {host}"));
            for ip in interleave(ips, false) {
                let t = Target::new(HostName::Ip(ip), target.port);
                match self.inner.connect(&t, opts).await {
                    Ok(stream) => return Ok(stream),
                    Err(e) => last = e,
                }
            }
            Err(last)
        })
```

换成

```rust
        Box::pin(self.each_address(target, move |target| async move {
            self.inner.connect(&target, opts).await
        }))
    }

    fn connect_udp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
        Box::pin(self.each_address(target, move |target| async move {
            self.inner.connect_udp(&target, opts).await
        }))
    }

    /// The inner connector, when it dials directly: a QUIC path then sends
    /// from sockets of its own, to an address — which the DoQ / DoH3
    /// upstreams give it, so that no resolver but the bootstrap is asked.
    fn as_direct(&self) -> Option<&DirectConnector> {
        self.inner.as_direct()
```

新建 `crates/rurge-dns/src/upstream/session.rs`：

```rust
//! The QUIC connection of a DNS server over QUIC or HTTP/3 (DoQ, DoH3),
//! shared by its queries — the rules of a policy's QUIC session (phase 2
//! M7 design 3.5), as DNS needs them:
//!
//! - The connection is dialed when a query first needs one; the queries
//!   that come meanwhile wait for that dial and share its failure. A dial
//!   whose query gave up on it (its future dropped) is taken over by one of
//!   those that wait.
//! - A dial tries the server's addresses in turn — a name's from the
//!   bootstrap, IPv4 first — until one connects, each on a path opened on
//!   the resolver's connector to the address itself: a direct path asks no
//!   resolver.
//! - A connection that has closed — its idle timeout, the server's close, a
//!   protocol error — or that is going away (an HTTP/3 GOAWAY) takes no new
//!   query: the next one dials. Nothing keeps a connection alive
//!   (`Transport::DNS`): unused, it idles out.
//! - A query on a connection that was up when it came, which finds the
//!   connection gone (`Failure::Lost`) — a server that dropped it while it
//!   idled answers with a stateless reset —, goes again, once, on a new
//!   connection: a DNS query can be asked again.
//! - The caller's deadline bounds a query: the dial, the handshake and the
//!   exchange.

use super::UpstreamError;
use crate::bootstrap::Bootstrap;
use quinn::{Connection, ConnectionError, VarInt};
use rurge_config::HostName;
use rurge_config::spec::Tristate;
use rurge_net::BoxFuture;
use rurge_net::connector::{ConnectOpts, Connector, Target, interleave};
use rurge_net::http::quic_tls_client_config;
use rurge_net::quic::{self, Path, Transport};
use std::io;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
use tokio::time::Instant;

/// The client configuration of the DNS servers over QUIC that speak `alpn`
/// (`doq`, `h3`): their certificates verified with the shared roots, or not
/// at all (`encrypted-dns-skip-cert-verification`), on TLS 1.3, with
/// `Transport::DNS`.
pub(super) fn client_config(skip_verify: bool, alpn: &str) -> io::Result<quinn::ClientConfig> {
    let tls = quic_tls_client_config(skip_verify, alpn).map_err(io::Error::other)?;
    quic::client_config(tls, &Transport::DNS)
}

/// A failure of QUIC, or of what runs over it, as the upstream's error, by
/// the layer its text names (phase 2 M7 design 3.6): a failed handshake's
/// (`tls: …`) is TLS's, HTTP/3's (`h3: …`) HTTP's, any other — QUIC's
/// (`quic: …`), the path's — I/O.
pub(super) fn transport_error(e: io::Error) -> UpstreamError {
    let text = e.to_string();
    if let Some(tls) = text.strip_prefix("tls: ") {
        UpstreamError::Tls(tls.to_string())
    } else if text.starts_with("h3: ") {
        UpstreamError::Http(text)
    } else {
        UpstreamError::Io(text)
    }
}

/// How a DNS server's QUIC connections are made.
pub(super) struct Dialer {
    /// The upstream's name, for the log.
    name: String,
    /// The server as configured — a name, or an address without brackets
    /// —: what the bootstrap resolves, and what TLS verifies and sends as
    /// SNI (none for an address).
    host: String,
    port: u16,
    connector: Arc<dyn Connector>,
    bootstrap: Arc<Bootstrap>,
    config: quinn::ClientConfig,
}

impl Dialer {
    /// The paths to `host` are opened on `connector`, its name resolved
    /// through `bootstrap`.
    pub(super) fn new(
        name: String,
        host: &str,
        port: u16,
        connector: Arc<dyn Connector>,
        bootstrap: Arc<Bootstrap>,
        config: quinn::ClientConfig,
    ) -> Dialer {
        Dialer {
            name,
            host: host.to_string(),
            port,
            connector,
            bootstrap,
            config,
        }
    }

    /// A connection to the server: its addresses in turn, IPv4 first, until
    /// one connects. The query's deadline bounds it; here, `deadline` bounds
    /// only the opening of each path.
    async fn connect(&self, deadline: Instant) -> Result<Connection, UpstreamError> {
        let addresses = self.bootstrap.resolve(&self.host).await?;
        let mut last = UpstreamError::Bootstrap(format!("no addresses for {}", self.host));
        for address in interleave(addresses, false) {
            let server = Target::new(HostName::Ip(address), self.port);
            match self.connect_to(&server, deadline).await {
                Ok(connection) => return Ok(connection),
                Err(e) => {
                    tracing::debug!(upstream = %self.name, %address, error = %e, "cannot connect to the server at this address");
                    last = transport_error(e);
                }
            }
        }
        Err(last)
    }

    async fn connect_to(&self, server: &Target, deadline: Instant) -> io::Result<Connection> {
        let opts = ConnectOpts {
            timeout: deadline.saturating_duration_since(Instant::now()),
        };
        let path = Path::open(self.connector.as_ref(), server, None, Tristate::Auto, &opts).await?;
        quic::connect(path, &self.config, &self.host).await
    }
}

/// What a protocol over QUIC keeps for each connection, and how it lets one
/// go.
pub(super) trait Protocol: Send + Sync + 'static {
    /// One connection's: what its queries share (DoH3: its HTTP/3 client,
    /// which must live as long as the connection).
    type State: Send + Sync + 'static;

    /// The application error code a connection is closed with when it is
    /// let go of: DOQ_NO_ERROR, H3_NO_ERROR.
    const NO_ERROR: VarInt;

    /// What a new connection starts with, once QUIC's handshake is done.
    fn start(&self, connection: &Connection) -> Self::State;

    /// Whether `state` says the connection takes no new query (an HTTP/3
    /// GOAWAY); the queries under way on it go on.
    fn is_going_away(&self, state: &Self::State) -> bool {
        let _ = state;
        false
    }
}

/// One connection and what its protocol keeps for it. The session holds it
/// while it takes queries, each query while it runs; when the last lets go,
/// the connection is closed.
pub(super) struct Conn<S> {
    pub(super) quic: Connection,
    pub(super) state: S,
    /// `Protocol::NO_ERROR`.
    no_error: VarInt,
}

impl<S> Conn<S> {
    /// Closes the connection with `code` and `reason`, unless it has ended
    /// already: quinn would put a close of ours in place of how it ended (a
    /// stateless reset, the server's close).
    pub(super) fn close(&self, code: VarInt, reason: &str) {
        if self.quic.close_reason().is_none() {
            self.quic.close(code, reason.as_bytes());
        }
    }
}

impl<S> Drop for Conn<S> {
    fn drop(&mut self) {
        // before the state goes (DoH3's HTTP/3 client ends its control
        // stream when dropped): the server sees the connection closed, not
        // a broken protocol
        self.close(self.no_error, "");
    }
}

/// How a query on one of the session's connections failed.
#[derive(Debug)]
pub(super) enum Failure {
    /// The connection ended under the query, or is going away: on another
    /// connection the query may yet get its answer.
    Lost(UpstreamError),
    /// Final.
    Failed(UpstreamError),
}

impl Failure {
    /// The connection's end, met by a query.
    pub(super) fn lost(e: &ConnectionError) -> Failure {
        Failure::Lost(transport_error(quic::connection_error(e)))
    }

    fn into_error(self) -> UpstreamError {
        match self {
            Failure::Lost(e) | Failure::Failed(e) => e,
        }
    }
}

/// A DNS server's QUIC connection, shared by its queries.
pub(super) struct Session<P: Protocol> {
    protocol: P,
    dialer: Dialer,
    slot: Mutex<Slot<P::State>>,
    /// A dial ended, or its query gave up on it: the queries waiting for it
    /// look again.
    dialed: Notify,
}

/// The session's connection and its dial.
struct Slot<S> {
    /// The connection new queries go to.
    current: Option<Arc<Conn<S>>>,
    /// A query is dialing.
    dialing: bool,
    /// Dials that have ended, success or failure.
    dials: u64,
    /// The latest dial's failure and the `dials` it ended as; `None` after
    /// a success.
    failure: Option<(u64, UpstreamError)>,
}

impl<P: Protocol> Session<P> {
    /// Dials nothing yet: the first query does.
    pub(super) fn new(protocol: P, dialer: Dialer) -> Session<P> {
        Session {
            protocol,
            dialer,
            slot: Mutex::new(Slot {
                current: None,
                dialing: false,
                dials: 0,
                failure: None,
            }),
            dialed: Notify::new(),
        }
    }

    /// The upstream's name.
    pub(super) fn name(&self) -> &str {
        &self.dialer.name
    }

    /// Runs `attempt` on the session's connection, all of it by `deadline`.
    /// A query that finds a connection it took at once gone
    /// (`Failure::Lost`) is attempted again, once, on a new connection; on
    /// a connection it waited for, it fails.
    pub(super) async fn query<'a>(
        &'a self,
        deadline: Instant,
        attempt: impl Fn(Arc<Conn<P::State>>) -> BoxFuture<'a, Result<Vec<u8>, Failure>>,
    ) -> Result<Vec<u8>, UpstreamError> {
        let query = async {
            let (conn, taken) = self.take(deadline).await?;
            let e = match attempt(conn).await {
                Err(Failure::Lost(e)) if taken => e,
                done => return done.map_err(Failure::into_error),
            };
            // the error says its layer (`quic: …`, `h3: …`)
            tracing::debug!(upstream = %self.dialer.name, "{e}; the query goes again on a new connection");
            let (conn, _) = self.take(deadline).await?;
            attempt(conn).await.map_err(Failure::into_error)
        };
        tokio::time::timeout_at(deadline, query)
            .await
            .unwrap_or(Err(UpstreamError::Timeout))
    }

    /// The connection a query goes to, dialing one when there is none; and
    /// whether the query took it at once — one that was up when it came —,
    /// rather than after a dial of its own or of another it waited for.
    async fn take(&self, deadline: Instant) -> Result<(Arc<Conn<P::State>>, bool), UpstreamError> {
        // the dials that had ended when this query began to wait
        let mut waited_since = None;
        loop {
            // registered before the look: a dial that ends after it wakes us
            let dialed = self.dialed.notified();
            let mut dialed = pin!(dialed);
            dialed.as_mut().enable();
            {
                let mut slot = self.slot.lock().expect("session");
                if let Some(conn) = &slot.current
                    && conn.quic.close_reason().is_none()
                    && !self.protocol.is_going_away(&conn.state)
                {
                    return Ok((conn.clone(), waited_since.is_none()));
                }
                // closed or going away: the queries under way keep it
                slot.current = None;
                if let Some(since) = waited_since
                    && !slot.dialing
                    && let Some((at, failure)) = &slot.failure
                    && *at > since
                {
                    return Err(failure.clone());
                }
                if !slot.dialing {
                    slot.dialing = true;
                    break;
                }
                waited_since = Some(slot.dials);
            }
            dialed.await;
        }
        self.dial(deadline).await.map(|conn| (conn, false))
    }

    /// A new connection, which becomes the session's. The dial stays in
    /// flight until it ends or its query gives up on it.
    async fn dial(&self, deadline: Instant) -> Result<Arc<Conn<P::State>>, UpstreamError> {
        let dialing = Dialing(self);
        let dialed = self.dialer.connect(deadline).await.map(|quic| {
            Arc::new(Conn {
                state: self.protocol.start(&quic),
                quic,
                no_error: P::NO_ERROR,
            })
        });
        {
            let mut slot = self.slot.lock().expect("session");
            slot.dials += 1;
            match &dialed {
                Ok(conn) => {
                    slot.current = Some(conn.clone());
                    slot.failure = None;
                }
                Err(e) => slot.failure = Some((slot.dials, e.clone())),
            }
        }
        drop(dialing);
        dialed
    }
}

/// A dial in flight; when it ends — or its query gives up on it —, the
/// queries waiting for it look again.
struct Dialing<'a, P: Protocol>(&'a Session<P>);

impl<P: Protocol> Drop for Dialing<'_, P> {
    fn drop(&mut self) {
        self.0.slot.lock().expect("session").dialing = false;
        self.0.dialed.notify_waiters();
    }
}
```

新建 `crates/rurge-dns/src/upstream/doq.rs`：

```rust
//! DNS over dedicated QUIC (RFC 9250): one QUIC connection per server with
//! ALPN `doq`, shared by its queries (`super::session`), and each query on
//! a bidirectional stream of its own.
//!
//! - The query goes out as a 2-byte length and the message — message ID 0,
//!   EDNS(0) padding to a multiple of 128 bytes (4.2, 4.2.1, 5.4: a MUST of
//!   the RFC, which none of the clients rurge looked at follows) —, then
//!   the FIN. The reply is what the stream carries to its end: one length
//!   and one message, whose ID must be 0; the caller's ID goes back on it.
//! - A reply that breaks those rules is the server's protocol error: the
//!   query fails, and the connection is closed with DOQ_PROTOCOL_ERROR
//!   (4.3.3). A stream the server resets fails its query alone (4.3.2).
//! - A query given up on — its future dropped: the fan-out's deadline —
//!   stops its stream with DOQ_REQUEST_CANCELLED, and resets the sending
//!   side unless the FIN went out (4.3.1).

use super::session::{self, Conn, Dialer, Failure, Protocol, Session};
use super::tcp::MAX_MESSAGE;
use super::{Upstream, UpstreamError};
use crate::bootstrap::Bootstrap;
use crate::message::{doq_query, wire_id};
use quinn::{Connection, ReadError, ReadToEndError, RecvStream, SendStream, VarInt, WriteError};
use rurge_net::BoxFuture;
use rurge_net::connector::Connector;
use std::borrow::Cow;
use std::io;
use std::sync::Arc;
use tokio::time::Instant;

/// DoQ's ALPN (RFC 9250 4.1), alone: the drafts' (`doq-i0x`) frame their
/// messages otherwise.
pub const ALPN: &str = "doq";

// error codes (RFC 9250 4.3)
const DOQ_NO_ERROR: VarInt = VarInt::from_u32(0x0);
const DOQ_PROTOCOL_ERROR: VarInt = VarInt::from_u32(0x2);
const DOQ_REQUEST_CANCELLED: VarInt = VarInt::from_u32(0x3);

// the server's protocol errors with the reply's framing (RFC 9250 4.3.3)
const CUT_SHORT: &str = "the stream ended within the server's reply";
const MORE_THAN_A_REPLY: &str = "the server sent more than its reply on the stream";

/// A DNS server over QUIC.
pub struct DoqUpstream {
    session: Session<Doq>,
}

impl DoqUpstream {
    /// The server at `host` — a name, resolved through `bootstrap`, or an
    /// address — and `port`, its connections' paths opened on `connector`
    /// with `config` (`client_config`).
    pub fn new(
        host: &str,
        port: u16,
        connector: Arc<dyn Connector>,
        bootstrap: Arc<Bootstrap>,
        config: quinn::ClientConfig,
    ) -> DoqUpstream {
        let name = format!("quic://{}", super::host_port_text(host, port));
        DoqUpstream {
            session: Session::new(
                Doq,
                Dialer::new(name, host, port, connector, bootstrap, config),
            ),
        }
    }

    /// The client configuration of DoQ servers: ALPN `doq`, their
    /// certificates verified unless `skip_verify`.
    pub fn client_config(skip_verify: bool) -> io::Result<quinn::ClientConfig> {
        session::client_config(skip_verify, ALPN)
    }
}

/// DoQ keeps nothing for a connection.
struct Doq;

impl Protocol for Doq {
    type State = ();
    const NO_ERROR: VarInt = DOQ_NO_ERROR;

    fn start(&self, _connection: &Connection) {}
}

impl Upstream for DoqUpstream {
    fn name(&self) -> &str {
        self.session.name()
    }

    fn query<'a>(
        &'a self,
        wire: &'a [u8],
        deadline: Instant,
    ) -> BoxFuture<'a, Result<Vec<u8>, UpstreamError>> {
        Box::pin(async move {
            let id = wire_id(wire).ok_or_else(|| {
                UpstreamError::BadResponse("query shorter than a DNS header".to_string())
            })?;
            let message =
                doq_query(wire).map_err(|e| UpstreamError::BadResponse(format!("query: {e}")))?;
            let len = u16::try_from(message.len()).map_err(|_| {
                UpstreamError::BadResponse("query larger than 65535 bytes".to_string())
            })?;
            let mut framed = Vec::with_capacity(2 + message.len());
            framed.extend_from_slice(&len.to_be_bytes());
            framed.extend_from_slice(&message);
            let mut reply = self
                .session
                .query(deadline, |conn| Box::pin(exchange(conn, &framed)))
                .await?;
            reply[..2].copy_from_slice(&id.to_be_bytes());
            Ok(reply)
        })
    }
}

/// One query on a stream of its own: `framed` out and the FIN, then the
/// reply — its message.
async fn exchange(conn: Arc<Conn<()>>, framed: &[u8]) -> Result<Vec<u8>, Failure> {
    let (send, recv) = conn.quic.open_bi().await.map_err(|e| Failure::lost(&e))?;
    let mut stream = QueryStream {
        send,
        recv,
        finished: false,
        read: false,
    };
    stream.send.write_all(framed).await.map_err(write_failure)?;
    // fails only for a stream finished or reset before, which this is not
    let _ = stream.send.finish();
    stream.finished = true;
    let bytes = stream.recv.read_to_end(2 + MAX_MESSAGE).await;
    stream.read = true;
    match bytes {
        Ok(bytes) => reply(&conn, &bytes).map(<[u8]>::to_vec),
        Err(ReadToEndError::TooLong) => Err(violation(&conn, MORE_THAN_A_REPLY)),
        Err(ReadToEndError::Read(e)) => Err(read_failure(e)),
    }
}

/// The message of `bytes`, a stream read to its end: one length and one
/// message, whose ID is 0 (RFC 9250 4.2, 4.2.1, 4.3.3).
fn reply<'a>(conn: &Conn<()>, bytes: &'a [u8]) -> Result<&'a [u8], Failure> {
    let Some((len, message)) = bytes.split_first_chunk::<2>() else {
        return Err(violation(conn, CUT_SHORT));
    };
    let len = usize::from(u16::from_be_bytes(*len));
    if message.len() < len {
        return Err(violation(conn, CUT_SHORT));
    }
    if message.len() > len {
        return Err(violation(conn, MORE_THAN_A_REPLY));
    }
    let Some(id) = wire_id(message) else {
        return Err(Failure::Failed(UpstreamError::BadResponse(
            "response shorter than a DNS header".to_string(),
        )));
    };
    if id != 0 {
        return Err(violation(
            conn,
            &format!("the server's reply has message ID {id}, not 0"),
        ));
    }
    Ok(message)
}

/// The server broke DoQ: the query fails, and the connection is closed with
/// DOQ_PROTOCOL_ERROR (RFC 9250 4.3.3) — its other queries go again on a
/// new one.
fn violation(conn: &Conn<()>, what: &str) -> Failure {
    conn.close(DOQ_PROTOCOL_ERROR, what);
    Failure::Failed(UpstreamError::BadResponse(what.to_string()))
}

fn write_failure(e: WriteError) -> Failure {
    match e {
        WriteError::ConnectionLost(e) => Failure::lost(&e),
        WriteError::Stopped(code) => Failure::Failed(UpstreamError::Io(format!(
            "the server stopped the query's stream ({})",
            code_name(code)
        ))),
        e => Failure::Failed(UpstreamError::Io(e.to_string())),
    }
}

fn read_failure(e: ReadError) -> Failure {
    match e {
        ReadError::ConnectionLost(e) => Failure::lost(&e),
        ReadError::Reset(code) => Failure::Failed(UpstreamError::Io(format!(
            "the server reset the query's stream ({})",
            code_name(code)
        ))),
        e => Failure::Failed(UpstreamError::Io(e.to_string())),
    }
}

/// The name of a DoQ error code (RFC 9250 4.3), as error texts say it; an
/// unknown one in hex, which means DOQ_UNSPECIFIED_ERROR (4.3.4).
fn code_name(code: VarInt) -> Cow<'static, str> {
    Cow::Borrowed(match code.into_inner() {
        0x0 => "DOQ_NO_ERROR",
        0x1 => "DOQ_INTERNAL_ERROR",
        0x2 => "DOQ_PROTOCOL_ERROR",
        0x3 => "DOQ_REQUEST_CANCELLED",
        0x4 => "DOQ_EXCESSIVE_LOAD",
        0x5 => "DOQ_UNSPECIFIED_ERROR",
        other => return Cow::Owned(format!("{other:#x}")),
    })
}

/// A query's stream. Dropped before its reply was read — the query given
/// up on —, it is stopped with DOQ_REQUEST_CANCELLED, and its sending side
/// reset with it unless the FIN went out (RFC 9250 4.3.1).
struct QueryStream {
    send: SendStream,
    recv: RecvStream,
    /// The FIN went out.
    finished: bool,
    /// The reply was read, or reading it failed.
    read: bool,
}

impl Drop for QueryStream {
    fn drop(&mut self) {
        if self.read {
            return;
        }
        if !self.finished {
            let _ = self.send.reset(DOQ_REQUEST_CANCELLED);
        }
        let _ = self.recv.stop(DOQ_REQUEST_CANCELLED);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fanout::FanoutOpts;
    use crate::message::{Answer, Qtype, Question, build_query, parse_query, parse_response};
    use crate::testing::{DoqScript, FakeDoq, MockDns, RecordedStop};
    use crate::upstream::udp::UdpUpstream;
    use quinn::ConnectionError;
    use rurge_config::HostName;
    use rurge_net::connector::{
        BoxedDatagram, BoxedStream, ConnectOpts, DirectConnector, SystemResolve, Target,
    };
    use std::net::IpAddr;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::time::timeout;

    const BOUND: Duration = Duration::from_secs(10);

    fn direct() -> Arc<dyn Connector> {
        Arc::new(DirectConnector::new(Arc::new(SystemResolve)))
    }

    /// A bootstrap without a server: an address needs none.
    fn no_bootstrap() -> Arc<Bootstrap> {
        Bootstrap::new(Vec::new(), false, FanoutOpts::default())
    }

    /// The upstream of `fake`, by its address.
    fn upstream(fake: &FakeDoq, skip_verify: bool) -> DoqUpstream {
        DoqUpstream::new(
            "127.0.0.1",
            fake.addr().port(),
            direct(),
            no_bootstrap(),
            DoqUpstream::client_config(skip_verify).unwrap(),
        )
    }

    fn query(id: u16, name: &str, qtype: Qtype) -> Vec<u8> {
        build_query(
            id,
            &Question {
                name: name.to_string(),
                qtype,
            },
        )
        .unwrap()
    }

    /// `name`'s A records, asked with `id` and a deadline far enough off.
    async fn ask(up: &DoqUpstream, id: u16, name: &str) -> Result<Answer, UpstreamError> {
        let reply = up
            .query(&query(id, name, Qtype::A), Instant::now() + BOUND)
            .await?;
        Ok(parse_response(&reply).unwrap())
    }

    /// The connection and the stream of each query the fake read.
    fn streams(fake: &FakeDoq) -> Vec<(usize, u64)> {
        fake.queries()
            .iter()
            .map(|q| (q.connection, q.stream))
            .collect()
    }

    /// Waits until `check` holds, looking again and again until the bound.
    async fn eventually(mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + BOUND;
        while !check() {
            assert!(Instant::now() < deadline, "not within the bound");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Each query on a stream of its own of the one connection: a length,
    /// then the message — ID 0, padded to a multiple of 128 bytes —, then
    /// the FIN, which the fake waits for; the caller's ID comes back.
    #[tokio::test]
    async fn a_query_is_its_padded_message_with_id_0_on_a_stream_of_its_own() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &["fd00::1"], 60);
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let up = upstream(&fake, true);
        assert_eq!(
            up.name(),
            format!("quic://127.0.0.1:{}", fake.addr().port())
        );
        let a = ask(&up, 0x1234, "a.test").await.unwrap();
        assert_eq!(a.id, 0x1234);
        assert_eq!(a.v4, [("10.0.0.1".parse().unwrap(), 60)]);
        let wire = query(0x4321, "a.test", Qtype::Aaaa);
        let reply = up.query(&wire, Instant::now() + BOUND).await.unwrap();
        let aaaa = parse_response(&reply).unwrap();
        assert_eq!(aaaa.id, 0x4321);
        assert_eq!(aaaa.v6, [("fd00::1".parse().unwrap(), 60)]);
        assert_eq!(streams(&fake), [(0, 0), (0, 4)]);
        for (q, qtype) in fake.queries().iter().zip([Qtype::A, Qtype::Aaaa]) {
            let len = usize::from(u16::from_be_bytes([q.bytes[0], q.bytes[1]]));
            assert_eq!(q.bytes.len(), 2 + len, "one message, then the FIN");
            assert_eq!(len % 128, 0, "padded");
            let (id, question) = parse_query(q.message()).unwrap();
            assert_eq!(
                (id, question.name.as_str(), question.qtype),
                (0, "a.test", qtype)
            );
        }
        assert_eq!(fake.connections(), 1);
    }

    /// Queries that come together share one dial.
    #[tokio::test]
    async fn queries_that_come_together_share_one_dial() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        dns.set("b.test", &["10.0.0.2"], &[], 60);
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let up = upstream(&fake, true);
        let (a, b) = tokio::join!(ask(&up, 1, "a.test"), ask(&up, 2, "b.test"));
        assert_eq!(a.unwrap().v4[0].0.to_string(), "10.0.0.1");
        assert_eq!(b.unwrap().v4[0].0.to_string(), "10.0.0.2");
        assert_eq!(fake.connections(), 1);
        let mut used = streams(&fake);
        used.sort();
        assert_eq!(used, [(0, 0), (0, 4)]);
    }

    /// A reply with another ID than 0 breaks DoQ: the query fails and the
    /// connection is closed with DOQ_PROTOCOL_ERROR; the next query dials
    /// again.
    #[tokio::test]
    async fn a_reply_with_a_message_id_closes_the_connection() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoq::spawn(
            &dns,
            DoqScript {
                reply_id: Some(7),
                ..DoqScript::default()
            },
        );
        let up = upstream(&fake, true);
        let reason = "the server's reply has message ID 7, not 0";
        assert_eq!(
            ask(&up, 1, "a.test").await.unwrap_err(),
            UpstreamError::BadResponse(reason.to_string())
        );
        match timeout(BOUND, fake.connection(0).closed()).await.unwrap() {
            ConnectionError::ApplicationClosed(close) => {
                assert_eq!(close.error_code, DOQ_PROTOCOL_ERROR);
                assert_eq!(&close.reason[..], reason.as_bytes());
            }
            other => panic!("closed otherwise: {other:?}"),
        }
        assert!(ask(&up, 2, "a.test").await.is_err());
        assert_eq!(streams(&fake), [(0, 0), (1, 0)]);
    }

    /// A stream that does not carry one length and one message breaks DoQ
    /// as well (RFC 9250 4.3.3); a message too short for a DNS header is a
    /// bad response alone.
    #[tokio::test]
    async fn a_reply_framed_otherwise_closes_the_connection() {
        let dns = MockDns::spawn().await;
        let message = [0u8; 12];
        for (bytes, error, breaks) in [
            (Vec::new(), CUT_SHORT, true),
            (vec![0], CUT_SHORT, true),
            ([&[0, 13][..], &message].concat(), CUT_SHORT, true),
            ([&[0, 11][..], &message].concat(), MORE_THAN_A_REPLY, true),
            (
                vec![0, 2, 0, 0],
                "response shorter than a DNS header",
                false,
            ),
        ] {
            let fake = FakeDoq::spawn(
                &dns,
                DoqScript {
                    reply_bytes: Some(bytes.clone()),
                    ..DoqScript::default()
                },
            );
            let up = upstream(&fake, true);
            assert_eq!(
                ask(&up, 1, "a.test").await.unwrap_err(),
                UpstreamError::BadResponse(error.to_string()),
                "{bytes:?}"
            );
            if breaks {
                match timeout(BOUND, fake.connection(0).closed()).await.unwrap() {
                    ConnectionError::ApplicationClosed(close) => {
                        assert_eq!(close.error_code, DOQ_PROTOCOL_ERROR, "{bytes:?}");
                    }
                    other => panic!("closed otherwise: {other:?}"),
                }
            } else {
                assert!(fake.connection(0).close_reason().is_none());
            }
        }
    }

    /// A stream the server resets fails its query alone: the connection
    /// carries the next one.
    #[tokio::test]
    async fn a_stream_reset_by_the_server_fails_its_query_alone() {
        let dns = MockDns::spawn().await;
        let fake = FakeDoq::spawn(
            &dns,
            DoqScript {
                reset: true,
                ..DoqScript::default()
            },
        );
        let up = upstream(&fake, true);
        for id in [1, 2] {
            assert_eq!(
                ask(&up, id, "a.test").await.unwrap_err(),
                UpstreamError::Io(
                    "the server reset the query's stream (DOQ_INTERNAL_ERROR)".to_string()
                )
            );
        }
        assert_eq!(streams(&fake), [(0, 0), (0, 4)]);
        assert!(fake.connection(0).close_reason().is_none());
    }

    /// A query given up on — its deadline passed — stops its stream with
    /// DOQ_REQUEST_CANCELLED; the FIN went out before.
    #[tokio::test]
    async fn a_query_given_up_on_stops_its_stream() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let up = upstream(&fake, true);
        ask(&up, 1, "a.test").await.unwrap();
        dns.set_drop_all(true);
        let deadline = Instant::now() + Duration::from_millis(300);
        assert_eq!(
            up.query(&query(2, "a.test", Qtype::A), deadline).await,
            Err(UpstreamError::Timeout)
        );
        eventually(|| {
            fake.stops()
                == [RecordedStop {
                    connection: 0,
                    stream: 4,
                    code: 0x3,
                }]
        })
        .await;
    }

    /// A server that never answers the handshake costs the query its
    /// deadline, not QUIC's idle timeout.
    #[tokio::test]
    async fn a_server_that_never_answers_is_bounded_by_the_deadline() {
        let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let up = DoqUpstream::new(
            "127.0.0.1",
            silent.local_addr().unwrap().port(),
            direct(),
            no_bootstrap(),
            DoqUpstream::client_config(true).unwrap(),
        );
        let started = Instant::now();
        let deadline = started + Duration::from_millis(300);
        assert_eq!(
            up.query(&query(1, "a.test", Qtype::A), deadline).await,
            Err(UpstreamError::Timeout)
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// A connection the server let go of once it idled is replaced: the
    /// next query dials anew.
    #[tokio::test]
    async fn an_idle_connection_is_replaced() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoq::spawn(
            &dns,
            DoqScript {
                idle_timeout: Some(Duration::from_millis(300)),
                ..DoqScript::default()
            },
        );
        let up = upstream(&fake, true);
        ask(&up, 1, "a.test").await.unwrap();
        timeout(BOUND, fake.connection(0).closed())
            .await
            .expect("idled out within the bound");
        ask(&up, 2, "a.test").await.unwrap();
        assert_eq!(fake.connections(), 2);
        assert_eq!(streams(&fake), [(0, 0), (1, 0)]);
    }

    /// A server that restarted answers the query's packets with a stateless
    /// reset: the query goes again, once, on a new connection.
    #[tokio::test]
    async fn a_restarted_server_gets_the_query_again_on_a_new_connection() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let up = upstream(&fake, true);
        ask(&up, 1, "a.test").await.unwrap();
        fake.restart();
        assert_eq!(ask(&up, 2, "a.test").await.unwrap().id, 2);
        assert_eq!(fake.connections(), 2);
        assert_eq!(streams(&fake), [(0, 0), (1, 0)]);
    }

    /// The server's certificate is verified unless the profile says not to.
    #[tokio::test]
    async fn the_certificate_is_verified_unless_skipped() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let err = ask(&upstream(&fake, false), 1, "a.test").await.unwrap_err();
        assert!(
            matches!(&err, UpstreamError::Tls(text) if text.starts_with("invalid peer certificate")),
            "{err}"
        );
        assert_eq!(fake.connections(), 0);
        ask(&upstream(&fake, true), 2, "a.test").await.unwrap();
    }

    /// Carries UDP as a direct connector does, but refuses the flows to
    /// `refused`; a QUIC path goes over its flows.
    struct Refusing {
        inner: DirectConnector,
        refused: IpAddr,
        asked: Mutex<Vec<Target>>,
    }

    impl Connector for Refusing {
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
            self.asked.lock().unwrap().push(target.clone());
            if target.host == HostName::Ip(self.refused) {
                return Box::pin(std::future::ready(Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "refused",
                ))));
            }
            self.inner.connect_udp(target, opts)
        }
    }

    /// A server named by host goes through the bootstrap — the traditional
    /// server is asked for it, once —, and its addresses are tried in turn
    /// until one connects.
    #[tokio::test]
    async fn a_name_goes_through_the_bootstrap_and_its_addresses_in_turn() {
        let traditional = MockDns::spawn().await;
        traditional.set("doq.test", &["192.0.2.1", "127.0.0.1"], &[], 60);
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let port = fake.addr().port();
        let connector = Arc::new(Refusing {
            inner: DirectConnector::new(Arc::new(SystemResolve)),
            refused: "192.0.2.1".parse().unwrap(),
            asked: Mutex::new(Vec::new()),
        });
        let bootstrap = Bootstrap::new(
            vec![Arc::new(UdpUpstream::new(traditional.addr()))],
            false,
            FanoutOpts::default(),
        );
        let up = DoqUpstream::new(
            "doq.test",
            port,
            connector.clone(),
            bootstrap,
            DoqUpstream::client_config(true).unwrap(),
        );
        assert_eq!(up.name(), format!("quic://doq.test:{port}"));
        ask(&up, 1, "a.test").await.unwrap();
        ask(&up, 2, "a.test").await.unwrap();
        let at = |ip: &str| Target::new(HostName::parse(ip), port);
        assert_eq!(
            *connector.asked.lock().unwrap(),
            [at("192.0.2.1"), at("127.0.0.1")]
        );
        assert_eq!(traditional.query_count("doq.test", Qtype::A), 1);
        assert_eq!(dns.query_count("a.test", Qtype::A), 2);
    }

    #[test]
    fn doq_error_codes_have_their_names() {
        assert_eq!(code_name(VarInt::from_u32(0x1)), "DOQ_INTERNAL_ERROR");
        assert_eq!(code_name(VarInt::from_u32(0x5)), "DOQ_UNSPECIFIED_ERROR");
        assert_eq!(code_name(VarInt::from_u32(0x42)), "0x42");
    }
}
```

新建 `crates/rurge-dns/src/upstream/doh3.rs`：

```rust
//! DNS over HTTP/3: DoH's POSTs (RFC 8484) on one QUIC connection per
//! server with ALPN `h3`, shared by its queries (`super::session`), through
//! the HTTP/3 client of `rurge_net`.
//!
//! - A query is a POST to the URL's path — `:authority` the URL's host,
//!   with its port unless 443 —, `content-type` and `accept`
//!   `application/dns-message`, the message with ID 0 as its content, then
//!   the FIN. Its answer is a 200 of that content type, at most 65535 bytes
//!   — as long as its `content-length` says, which HTTP/3 checks —, and
//!   gets the caller's ID back. No EDNS(0) padding, as DoH has none.
//! - A GOAWAY lets the queries under way on its connection run to their
//!   end; the next one dials. A query the server rejected for it
//!   (H3_REQUEST_REJECTED), on a connection that was up when it came, goes
//!   again on the new one.

use super::doh::{DNS_MESSAGE, MAX_RESPONSE};
use super::session::{self, Conn, Dialer, Failure, Protocol, Session, transport_error};
use super::{Upstream, UpstreamError};
use crate::bootstrap::Bootstrap;
use crate::message::wire_id;
use http::header::{ACCEPT, CONTENT_LENGTH, CONTENT_TYPE};
use http::{Request, StatusCode};
use quinn::{Connection, VarInt};
use rurge_net::BoxFuture;
use rurge_net::connector::Connector;
use rurge_net::quic::h3;
use std::io;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;
use url::{Host, Url};

/// HTTP/3's ALPN (RFC 9114 3.1).
pub const ALPN: &str = "h3";

/// H3_NO_ERROR (RFC 9114 8.1): a connection let go of.
const H3_NO_ERROR: VarInt = VarInt::from_u32(0x100);

/// A DNS server over HTTP/3.
pub struct Doh3Upstream {
    /// Where the POSTs go: the `h3://` URL in its `https` form
    /// (`UpstreamSpec::H3`).
    url: Url,
    session: Session<Doh3>,
}

impl Doh3Upstream {
    /// The server of `url` (`UpstreamSpec::H3`'s): its host — a name,
    /// resolved through `bootstrap`, or an address — and port, its
    /// connections' paths opened on `connector` with `config`
    /// (`client_config`).
    pub fn new(
        url: Url,
        connector: Arc<dyn Connector>,
        bootstrap: Arc<Bootstrap>,
        config: quinn::ClientConfig,
    ) -> Doh3Upstream {
        // an IPv6 address without its brackets, as TLS takes it
        let host = match url.host() {
            Some(Host::Domain(name)) => name.to_string(),
            Some(Host::Ipv4(address)) => address.to_string(),
            Some(Host::Ipv6(address)) => address.to_string(),
            None => String::new(),
        };
        let port = url.port_or_known_default().unwrap_or(443);
        let dialer = Dialer::new(
            super::h3_name(&url),
            &host,
            port,
            connector,
            bootstrap,
            config,
        );
        Doh3Upstream {
            url,
            session: Session::new(Doh3, dialer),
        }
    }

    /// The client configuration of DoH3 servers: ALPN `h3`, their
    /// certificates verified unless `skip_verify`.
    pub fn client_config(skip_verify: bool) -> io::Result<quinn::ClientConfig> {
        session::client_config(skip_verify, ALPN)
    }
}

/// HTTP/3 keeps its client for each connection.
struct Doh3;

impl Protocol for Doh3 {
    type State = h3::Client;
    const NO_ERROR: VarInt = H3_NO_ERROR;

    fn start(&self, connection: &Connection) -> h3::Client {
        h3::Client::new(connection.clone())
    }

    fn is_going_away(&self, client: &h3::Client) -> bool {
        client.is_going_away()
    }
}

impl Upstream for Doh3Upstream {
    fn name(&self) -> &str {
        self.session.name()
    }

    fn query<'a>(
        &'a self,
        wire: &'a [u8],
        deadline: Instant,
    ) -> BoxFuture<'a, Result<Vec<u8>, UpstreamError>> {
        Box::pin(async move {
            let id = wire_id(wire).ok_or_else(|| {
                UpstreamError::BadResponse("query shorter than a DNS header".to_string())
            })?;
            let mut body = wire.to_vec();
            body[..2].fill(0);
            let mut reply = self
                .session
                .query(deadline, |conn| Box::pin(exchange(conn, &self.url, &body)))
                .await?;
            reply[..2].copy_from_slice(&id.to_be_bytes());
            Ok(reply)
        })
    }
}

/// The POST of a query of `len` bytes to `url` (RFC 8484 4.1): the URL's
/// authority goes without the port 443, which its `https` form leaves out.
fn request(url: &Url, len: usize) -> Result<Request<()>, UpstreamError> {
    Request::post(url.as_str())
        .header(CONTENT_TYPE, DNS_MESSAGE)
        .header(ACCEPT, DNS_MESSAGE)
        .header(CONTENT_LENGTH, len)
        .body(())
        .map_err(|e| UpstreamError::Http(e.to_string()))
}

/// One query: the POST of `body` to `url` on a stream of its own, then the
/// answer's content.
async fn exchange(conn: Arc<Conn<h3::Client>>, url: &Url, body: &[u8]) -> Result<Vec<u8>, Failure> {
    let request = request(url, body.len()).map_err(Failure::Failed)?;
    let failed = |e| failure(&conn, e);
    let mut stream = conn.state.request(request).await.map_err(failed)?;
    stream.write_all(body).await.map_err(failed)?;
    // the FIN, and the rest of a DATA frame begun before it
    stream.shutdown().await.map_err(failed)?;
    let response = stream.response().await.map_err(failed)?;
    let status = response.status();
    if status != StatusCode::OK {
        return Err(Failure::Failed(UpstreamError::Http(format!(
            "status {status}"
        ))));
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !content_type.starts_with(DNS_MESSAGE) {
        return Err(Failure::Failed(UpstreamError::BadResponse(format!(
            "content-type `{content_type}`"
        ))));
    }
    let mut reply = Vec::new();
    (&mut stream)
        .take(MAX_RESPONSE as u64 + 1)
        .read_to_end(&mut reply)
        .await
        .map_err(failed)?;
    if reply.len() > MAX_RESPONSE {
        return Err(Failure::Failed(UpstreamError::BadResponse(
            "response larger than 65535 bytes".to_string(),
        )));
    }
    if reply.len() < 12 {
        return Err(Failure::Failed(UpstreamError::BadResponse(
            "response shorter than a DNS header".to_string(),
        )));
    }
    Ok(reply)
}

/// How a query on `conn` failed: `Lost` when the connection ended or is
/// going away — a request the server rejected after its GOAWAY included
/// —, else final. The error says its layer (`h3: …`, `quic: …`).
fn failure(conn: &Conn<h3::Client>, e: io::Error) -> Failure {
    let error = transport_error(e);
    if conn.quic.close_reason().is_some() || conn.state.is_going_away() {
        Failure::Lost(error)
    } else {
        Failure::Failed(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fanout::FanoutOpts;
    use crate::message::{Answer, Qtype, Question, build_query, parse_response};
    use crate::testing::{Doh3Script, FakeDoh3, MockDns};
    use crate::upstream::UpstreamSpec;
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::time::Duration;

    const BOUND: Duration = Duration::from_secs(10);

    /// The `https` form of an `h3://` URL.
    fn h3(url: &str) -> Url {
        match UpstreamSpec::parse(url).unwrap() {
            UpstreamSpec::H3(url) => url,
            other => panic!("{other:?}"),
        }
    }

    /// The upstream of `fake` at `path` (none: the default one).
    fn upstream(fake: &FakeDoh3, path: &str) -> Doh3Upstream {
        Doh3Upstream::new(
            h3(&format!("h3://127.0.0.1:{}{path}", fake.addr().port())),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
            Bootstrap::new(Vec::new(), false, FanoutOpts::default()),
            Doh3Upstream::client_config(true).unwrap(),
        )
    }

    fn query(id: u16, name: &str, qtype: Qtype) -> Vec<u8> {
        build_query(
            id,
            &Question {
                name: name.to_string(),
                qtype,
            },
        )
        .unwrap()
    }

    /// `name`'s A records, asked with `id` and a deadline far enough off.
    async fn ask(up: &Doh3Upstream, id: u16, name: &str) -> Result<Answer, UpstreamError> {
        let reply = up
            .query(&query(id, name, Qtype::A), Instant::now() + BOUND)
            .await?;
        Ok(parse_response(&reply).unwrap())
    }

    /// The connection and the stream of each request the fake read.
    fn streams(fake: &FakeDoh3) -> Vec<(usize, u64)> {
        fake.requests()
            .iter()
            .map(|r| (r.connection, r.stream))
            .collect()
    }

    /// A query is a POST of the message with ID 0 — not padded — to the
    /// default path, of `application/dns-message` and taking it, then the
    /// FIN, which the fake waits for; the caller's ID comes back. The
    /// queries share one connection.
    #[tokio::test]
    async fn a_query_is_a_post_of_the_message_with_id_0() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &["fd00::1"], 60);
        let fake = FakeDoh3::spawn(&dns, Doh3Script::default());
        let port = fake.addr().port();
        let up = upstream(&fake, "");
        assert_eq!(up.name(), format!("h3://127.0.0.1:{port}/dns-query"));
        let a = ask(&up, 0x1234, "a.test").await.unwrap();
        assert_eq!(a.id, 0x1234);
        assert_eq!(a.v4, [("10.0.0.1".parse().unwrap(), 60)]);
        let wire = query(0x4321, "a.test", Qtype::Aaaa);
        let reply = up.query(&wire, Instant::now() + BOUND).await.unwrap();
        let aaaa = parse_response(&reply).unwrap();
        assert_eq!((aaaa.id, aaaa.v6.len()), (0x4321, 1));
        assert_eq!(streams(&fake), [(0, 0), (0, 4)]);
        let request = &fake.requests()[1];
        let authority = format!("127.0.0.1:{port}");
        let length = wire.len().to_string();
        for (name, value) in [
            (":method", "POST"),
            (":scheme", "https"),
            (":authority", authority.as_str()),
            (":path", "/dns-query"),
            ("content-type", "application/dns-message"),
            ("accept", "application/dns-message"),
            ("content-length", length.as_str()),
        ] {
            assert_eq!(request.field(name), Some(value), "{name}");
        }
        let mut sent = wire;
        sent[..2].fill(0);
        assert_eq!(request.body, sent);
    }

    /// The URL's path goes as it is; the authority leaves the port out
    /// when it is 443.
    #[tokio::test]
    async fn the_request_names_the_urls_path_and_authority() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoh3::spawn(&dns, Doh3Script::default());
        ask(&upstream(&fake, "/q?x=1"), 1, "a.test").await.unwrap();
        assert_eq!(fake.requests()[0].field(":path"), Some("/q?x=1"));
        for (url, authority) in [
            ("h3://dns.example", "dns.example"),
            ("h3://dns.example:443/dns-query", "dns.example"),
            ("h3://dns.example:8443", "dns.example:8443"),
            ("h3://[::1]", "[::1]"),
        ] {
            let request = request(&h3(url), 33).unwrap();
            assert_eq!(
                request.uri().authority().map(|a| a.as_str()),
                Some(authority),
                "{url}"
            );
            assert_eq!(request.uri().path(), "/dns-query", "{url}");
        }
    }

    /// Only a 200 of `application/dns-message` is an answer.
    #[tokio::test]
    async fn another_status_or_content_type_is_refused() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        for (script, error) in [
            (
                Doh3Script {
                    status: Some(500),
                    ..Doh3Script::default()
                },
                UpstreamError::Http("status 500 Internal Server Error".to_string()),
            ),
            (
                Doh3Script {
                    content_type: Some("text/html".to_string()),
                    ..Doh3Script::default()
                },
                UpstreamError::BadResponse("content-type `text/html`".to_string()),
            ),
        ] {
            let fake = FakeDoh3::spawn(&dns, script);
            assert_eq!(ask(&upstream(&fake, ""), 1, "a.test").await, Err(error));
        }
    }

    /// An answer whose content its `content-length` does not match is
    /// malformed (RFC 9114 4.1.2): it fails, its connection carries on.
    #[tokio::test]
    async fn an_answer_off_its_content_length_is_refused() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        for (length, how) in [(1, "longer"), (1000, "shorter")] {
            let fake = FakeDoh3::spawn(
                &dns,
                Doh3Script {
                    content_length: Some(length),
                    ..Doh3Script::default()
                },
            );
            let up = upstream(&fake, "");
            let malformed = UpstreamError::Http(format!(
                "h3: the server sent a malformed response ({how} than its content-length)"
            ));
            assert_eq!(ask(&up, 1, "a.test").await, Err(malformed.clone()));
            assert_eq!(ask(&up, 2, "a.test").await, Err(malformed));
            assert_eq!(streams(&fake), [(0, 0), (0, 4)], "{how}");
        }
    }

    /// After a GOAWAY the next query goes on a new connection — directly,
    /// or after the one it went on rejected it.
    #[tokio::test]
    async fn a_goaway_sends_the_next_query_to_a_new_connection() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoh3::spawn(
            &dns,
            Doh3Script {
                goaway_after: Some(1),
                ..Doh3Script::default()
            },
        );
        let up = upstream(&fake, "");
        ask(&up, 1, "a.test").await.unwrap();
        ask(&up, 2, "a.test").await.unwrap();
        assert_eq!(fake.connections(), 2);
        assert_eq!(streams(&fake), [(0, 0), (1, 0)]);
    }

    /// A server that restarted answers the query's packets with a stateless
    /// reset: the query goes again, once, on a new connection.
    #[tokio::test]
    async fn a_restarted_server_gets_the_query_again_on_a_new_connection() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoh3::spawn(&dns, Doh3Script::default());
        let up = upstream(&fake, "");
        ask(&up, 1, "a.test").await.unwrap();
        fake.restart();
        assert_eq!(ask(&up, 2, "a.test").await.unwrap().id, 2);
        assert_eq!(fake.connections(), 2);
        assert_eq!(streams(&fake), [(0, 0), (1, 0)]);
    }

    /// A server that never answers costs the query its deadline.
    #[tokio::test]
    async fn a_silent_server_is_bounded_by_the_deadline() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoh3::spawn(&dns, Doh3Script::default());
        let up = upstream(&fake, "");
        ask(&up, 1, "a.test").await.unwrap();
        dns.set_drop_all(true);
        let deadline = Instant::now() + Duration::from_millis(300);
        assert_eq!(
            up.query(&query(2, "a.test", Qtype::A), deadline).await,
            Err(UpstreamError::Timeout)
        );
    }
}
```

`crates/rurge-dns/src/resolver.rs`——把

```rust
use crate::upstream::doh::DohUpstream;
```

换成

```rust
use crate::upstream::doh::DohUpstream;
use crate::upstream::doh3::Doh3Upstream;
use crate::upstream::doq::DoqUpstream;
```

`crates/rurge-dns/src/resolver.rs`——把

```rust
    tls: Option<Arc<rustls::ClientConfig>>,
```

换成

```rust
    tls: Option<Arc<rustls::ClientConfig>>,
    /// The client configuration of the DoQ upstreams; `None` when it could
    /// not be built (W0026 was recorded): they are skipped then, as DoH is.
    doq: Option<quinn::ClientConfig>,
    /// The same of the DoH3 upstreams.
    doh3: Option<quinn::ClientConfig>,
```

`crates/rurge-dns/src/resolver.rs`——把

```rust
    /// The encrypted subsystem (`https://` / `tls://` / `tcp://`). It owns the
    /// primary set whenever it is non-empty and never depends on the network.
```

换成

```rust
    /// The encrypted subsystem (`https://` / `h3://` / `quic://` / `tls://` /
    /// `tcp://`). It owns the primary set whenever it is non-empty and never
    /// depends on the network.
```

`crates/rurge-dns/src/resolver.rs`——把

```rust
        // Encrypted subsystem: https:// tls:// tcp://
```

换成

```rust
        // Encrypted subsystem: https:// h3:// quic:// tls:// tcp://
```

`crates/rurge-dns/src/resolver.rs`——把

```rust
                None
            }
        };

        let hosts = HostMap::build(&cfg.hosts, deps.sets.as_ref(), &mut diags);
```

换成

```rust
                None
            }
        };
        let doq = available(
            DoqUpstream::client_config(cfg.skip_cert_verification),
            "DoQ",
            &mut diags,
        );
        let doh3 = available(
            Doh3Upstream::client_config(cfg.skip_cert_verification),
            "DoH3",
            &mut diags,
        );

        let hosts = HostMap::build(&cfg.hosts, deps.sets.as_ref(), &mut diags);
```

`crates/rurge-dns/src/resolver.rs`——把

```rust
            tls,
```

换成

```rust
            tls,
            doq,
            doh3,
```

`crates/rurge-dns/src/resolver.rs`——把

```rust
                .map(|http| Arc::new(DohUpstream::new(url.clone(), http.clone())) as UpstreamRef),
```

换成

```rust
                .map(|http| Arc::new(DohUpstream::new(url.clone(), http.clone())) as UpstreamRef),
            UpstreamSpec::Quic { host, port } => self.doq.as_ref().map(|config| {
                Arc::new(DoqUpstream::new(
                    host,
                    *port,
                    self.connector.clone(),
                    self.bootstrap.clone(),
                    config.clone(),
                )) as UpstreamRef
            }),
            UpstreamSpec::H3(url) => self.doh3.as_ref().map(|config| {
                Arc::new(Doh3Upstream::new(
                    url.clone(),
                    self.connector.clone(),
                    self.bootstrap.clone(),
                    config.clone(),
                )) as UpstreamRef
            }),
```

`crates/rurge-dns/src/resolver.rs`——把

```rust
    /// The upstreams that resolve the hostnames of URL-type (DoT / DoH / TCP)
    /// upstreams. Always plain UDP; see `bootstrap_set`.
```

换成

```rust
    /// The upstreams that resolve the hostnames of URL-type (DoT / DoH /
    /// DoH3 / DoQ / TCP) upstreams. Always plain UDP; see `bootstrap_set`.
```

`crates/rurge-dns/src/resolver.rs`——把

```rust
                    tracing::info!(path = %path.display(), entries = me.hosts.etc_hosts_len(), "hosts file reloaded");
                }
            }
        });
    }
}
```

换成

```rust
                    tracing::info!(path = %path.display(), entries = me.hosts.etc_hosts_len(), "hosts file reloaded");
                }
            }
        });
    }
}

/// A client configuration of the encrypted subsystem, or `None` with a
/// W0026 that says `what` is unavailable.
fn available<T>(built: io::Result<T>, what: &str, diags: &mut Diagnostics) -> Option<T> {
    match built {
        Ok(config) => Some(config),
        Err(e) => {
            diags.push(Diagnostic::warning(
                codes::W_DNS_UPSTREAM_UNSUPPORTED,
                format!("{what} unavailable: {e}"),
            ));
            None
        }
    }
}
```

引擎的 `PipelineConnector` 暂时直连（Task 7 改为按规则走），CLI 的说明：

`crates/rurge-engine/src/dns_pipeline.rs`——把

```rust
//! upstreams never use a connector.
```

换成

```rust
//! upstreams never use a connector; the QUIC carriers of the DoQ / DoH3
//! upstreams (`connect_udp`) go straight to the server through the
//! fallback, the rules aside.
```

`crates/rurge-engine/src/dns_pipeline.rs`——把

```rust
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
```

换成

```rust
use rurge_net::connector::{BoxedDatagram, BoxedStream, ConnectOpts, Connector, Target};
```

`crates/rurge-engine/src/dns_pipeline.rs`——把

```rust
            engine.dial_internal(session, &self.fallback).await
        })
    }
```

换成

```rust
            engine.dial_internal(session, &self.fallback).await
        })
    }

    /// A QUIC carrier of the DoQ / DoH3 upstreams: straight to the server,
    /// through the fallback.
    fn connect_udp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
        self.fallback.connect_udp(target, opts)
    }
```

`crates/rurge/src/cli/dns.rs`——把

```rust
/// (`[v6]:port`), or an encrypted URL (`https://`, `tls://`, `tcp://`).
```

换成

```rust
/// (`[v6]:port`), or an encrypted URL (`https://`, `h3://`, `quic://`,
/// `tls://`, `tcp://`).
```

要点：
- 主机名经引导解析、以 IP 目标经 `Path::open`；`BootstrapConnector::as_direct` 委托给内层连接器（`PipelineConnector` 的是 `None`，于是跟随出站模式时 QUIC 走 `connect_udp`）。
- DoQ 的查询 ID 置 0、EDNS(0) 填充到 128 字节的整数倍、写完即 FIN；应答 ID 不为 0 等协议错误以 0x2 关连接；被放弃的查询以 0x3 停流。
- DoH3 写完正文先 `shutdown`（HTTP/3 的写可能把半帧留到下一次写、`flush` 或 `shutdown`）。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-net quic` → 通过（新增 2 条：DNS 连接不保活、TLS 1.3 且只有一个 ALPN）。
Run: `cargo test -p rurge-dns` → 通过（新增 31 条：写法与配置 8 条——ID 0 与填充到 128、换掉已有的填充与 keepalive 并补上 OPT、`quic://` 只有主机与端口、`h3://` 的缺省端口与路径、名字与引导、`connect_udp` 连到解析出的地址、两种 scheme 不再告警（配置与上游各一）；DoQ 12 条——查询的形状、同时来的查询共用一次拨号、应答带 ID 即关连接、应答的帧不对即关连接、服务端重置流只让该查询失败、放弃的查询停它的流、不应答时受时限约束、空闲的连接被换、服务端重启后在新连接上再问一次、证书校验与跳过、名字经引导逐个地址、错误码的名字；DoH3 7 条——POST 与 ID 0、路径与 `:authority`、其它状态或内容类型被拒、与 `content-length` 不符被拒、GOAWAY 后用新连接、重启后再问一次、静默的服务端受时限约束；`tests/resolver.rs` 4 条——DoQ / DoH3 答 A 与 AAAA、名字写的 DoQ 服务器经引导、`[Host]` 指向 DoQ 服务器）。
Run: `cargo test -p rurge-engine --test pipeline` → 通过（新增 1 条：跟随出站模式的解析器照样到达 DoQ 服务器）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add Cargo.lock crates
git commit -m "feat(dns): DoQ 与 DoH3 上游——RFC 9250 与 RFC 8484 over HTTP/3、EDNS(0) 填充、连接复用与失效重试"
```

### Task 7: 跟随出站模式下 DoQ / DoH3 的 QUIC 载体按规则走

`encrypted-dns-follow-outbound-mode` 打开时，`PipelineConnector::connect_udp` 不再总是直连：QUIC 载体作为内部 UDP 会话经 `Engine::dial_internal_udp` 按规则选策略（P17），与 TCP / DoT / DoH 的内部会话同样进请求记录。`dial_internal` 的路由与防环判定、smart 回报抽成共用的函数，TCP 的行为不变。

**Files:**
- Modify: `crates/rurge-engine/src/{dns_pipeline.rs（与用例）, engine.rs, runtime.rs, stack.rs}`、`tests/pipeline.rs`

**Interfaces:**
- Consumes: Task 6 的 DoQ / DoH3 与 `BootstrapConnector::connect_udp`（到达 `PipelineConnector` 的目标总是 IP）、`FakeDoq` / `FakeDoh3`；`rurge_net::packet_datagram`；引擎既有的 `dial_internal`、`socket_opener`、`named_server`、`bypass_to_direct`、`FakeSocks5` 的 UDP ASSOCIATE。
- Produces: `Engine::dial_internal_udp(&self, session: SessionInfo, fallback: &Arc<dyn Connector>) -> io::Result<BoxedDatagram>`；`PipelineConnector::connect_udp`（按规则）；`dns_pipeline` 的 `quic_protocol_for(port)` 与 `wrap_internal_udp`

- [ ] **Step 1: 先写用例**

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
use rurge_config::config::{LoadOptions, from_text};
use rurge_config::session::{ListenerKind, SessionInfo};
use rurge_dns::system::StaticSystemDns;
use rurge_dns::testing::{DoqScript, FakeDoq, MockDns};
use rurge_engine::stack::StackOptions;
use rurge_engine::{Engine, EngineShared, ListenerSpec, RequestRecord, Runtime, RuntimeOptions};
use rurge_inbound::{DialError, Dialer, Running, SessionHandle, SessionOutcome};
use rurge_net::testing::TestServer;
use rurge_policy::GroupSelections;
use rurge_proto::testing::{FakeHttpProxy, HttpProxyScript};
use rurge_rules::{GeoUrls, OutboundMode};
use std::net::SocketAddr;
```

换成

```rust
use rurge_config::HostName;
use rurge_config::config::{LoadOptions, from_text};
use rurge_config::rule::ProtocolKind;
use rurge_config::session::{ListenerKind, SessionInfo, Transport};
use rurge_dns::Source;
use rurge_dns::system::StaticSystemDns;
use rurge_dns::testing::{Doh3Script, DoqScript, FakeDoh3, FakeDoq, MockDns};
use rurge_engine::stack::StackOptions;
use rurge_engine::{
    Engine, EngineShared, ListenerSpec, RecordStatus, RequestRecord, Runtime, RuntimeOptions,
};
use rurge_inbound::{DialError, Dialer, Running, SessionHandle, SessionOutcome};
use rurge_net::connector::Target;
use rurge_net::testing::TestServer;
use rurge_policy::GroupSelections;
use rurge_proto::testing::{FakeHttpProxy, FakeSocks5, HttpProxyScript, Socks5Script};
use rurge_rules::{GeoUrls, OutboundMode};
use std::net::{Ipv4Addr, SocketAddr};
```

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
/// In follow mode a DoQ server is reached all the same: the pipeline hands
/// the QUIC carrier to its direct fallback.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_follow_mode_resolver_reaches_a_doq_server() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\nencrypted-dns-follow-outbound-mode = true\n\
encrypted-dns-server = quic://127.0.0.1:{}\nencrypted-dns-skip-cert-verification = true\n\
ipv6 = false\n[Proxy]\n[Proxy Group]\n[Rule]\nFINAL,DIRECT\n",
        doq.addr().port()
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = tokio::time::timeout(
```

换成

```rust
/// A follow-mode profile whose encrypted DNS servers are `servers` (their
/// certificates, the fakes' own, not checked), with `proxies`, and `rules`
/// before `FINAL,DIRECT`.
fn follow_profile(servers: &str, proxies: &str, rules: &str) -> String {
    format!(
        "[General]\nencrypted-dns-follow-outbound-mode = true\n\
encrypted-dns-server = {servers}\nencrypted-dns-skip-cert-verification = true\n\
ipv6 = false\n[Proxy]\n{proxies}\n[Proxy Group]\n[Rule]\n{rules}\nFINAL,DIRECT\n"
    )
}

/// `target.test` through the engine's resolver, answered within the bound.
async fn lookup_target(engine: &Engine) -> rurge_dns::DnsResult {
    tokio::time::timeout(
```

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
    .expect("an answer over DoQ");
    assert_eq!(res.v4, ["127.0.0.1".parse::<std::net::Ipv4Addr>().unwrap()]);
    assert_eq!(doq.connections(), 1);
    assert!(!doq.queries().is_empty());
```

换成

```rust
    .expect("an answer")
}

/// The QUIC carriers recorded so far: the internal UDP sessions.
fn carrier_sessions(engine: &Engine) -> Vec<RequestRecord> {
    let mut all = internal_sessions(engine);
    all.retain(|r| r.transport == Transport::Udp);
    all
}

/// In follow mode a DoQ server's QUIC carrier goes by the rules (phase 2 M7
/// design 7.3): `PROTOCOL,DOQ` sends it through a SOCKS5 proxy, whose UDP
/// association takes the QUIC packets to the server. The carrier is an
/// internal UDP session of the request log, its bytes counted both ways.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_doq_carrier_follows_the_rules_through_a_socks5_proxy() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let proxy = FakeSocks5::spawn(Socks5Script::default()).await;
    let dir = tempfile::tempdir().unwrap();
    let profile = follow_profile(
        &format!("quic://{}", doq.addr()),
        &format!(
            "Socks = socks5, 127.0.0.1, {}, udp-relay=true",
            proxy.addr().port()
        ),
        "PROTOCOL,DOQ,Socks",
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = lookup_target(&engine).await;
    assert_eq!(res.v4, [Ipv4Addr::LOCALHOST]);
    assert_eq!(
        res.source,
        Source::Upstream(format!("quic://{}", doq.addr()))
    );
    // UDP associations only, and every packet through them to the server
    let requests = proxy.requests();
    assert!(
        !requests.is_empty() && requests.iter().all(|r| r.command == 3),
        "{requests:?}"
    );
    let server = Target::new(HostName::Ip(doq.addr().ip()), doq.addr().port());
    let datagrams = proxy.datagrams();
    assert!(
        !datagrams.is_empty() && datagrams.iter().all(|to| *to == server),
        "{datagrams:?}"
    );
    let carriers = carrier_sessions(&engine);
    assert!(
        carriers
            .iter()
            .any(|r| r.protocol == Some(ProtocolKind::Doq)
                && r.rule.as_deref() == Some("PROTOCOL,DOQ,Socks")
                && r.policy == ["Socks"]
                && r.dst == doq.addr().to_string()
                && r.up > 0
                && r.down > 0
                && r.error.is_none()),
        "{carriers:?}"
    );
}

/// The same for a DoH3 server, sent by an IP rule. Port 443 alone tags a
/// carrier `DOH3`, and a loopback server here has another: this one is
/// tagged `DOQ` (best effort, as the TCP connections' tags are).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_doh3_carrier_follows_an_ip_rule_through_a_socks5_proxy() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doh3 = FakeDoh3::spawn(&dns, Doh3Script::default());
    let proxy = FakeSocks5::spawn(Socks5Script::default()).await;
    let dir = tempfile::tempdir().unwrap();
    let profile = follow_profile(
        &format!("h3://{}", doh3.addr()),
        &format!(
            "Socks = socks5, 127.0.0.1, {}, udp-relay=true",
            proxy.addr().port()
        ),
        "IP-CIDR,127.0.0.1/32,Socks",
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = lookup_target(&engine).await;
    assert_eq!(res.v4, [Ipv4Addr::LOCALHOST]);
    assert_eq!(
        res.source,
        Source::Upstream(format!("h3://{}/dns-query", doh3.addr()))
    );
    assert!(!doh3.requests().is_empty());
    let server = Target::new(HostName::Ip(doh3.addr().ip()), doh3.addr().port());
    let datagrams = proxy.datagrams();
    assert!(
        !datagrams.is_empty() && datagrams.iter().all(|to| *to == server),
        "{datagrams:?}"
    );
    let carriers = carrier_sessions(&engine);
    assert!(
        carriers
            .iter()
            .any(|r| r.protocol == Some(ProtocolKind::Doq)
                && r.rule.as_deref() == Some("IP-CIDR,127.0.0.1/32,Socks")
                && r.policy == ["Socks"]
                && r.dst == doh3.addr().to_string()
                && r.up > 0
                && r.down > 0),
        "{carriers:?}"
    );
}

/// `PROTOCOL,DOQ,DIRECT`: the carrier goes out through DIRECT's own UDP —
/// with its socket settings, as a TCP DNS session goes through DIRECT's
/// TCP — and is recorded as any internal session is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_doq_carrier_routed_to_direct_is_an_internal_udp_session() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let dir = tempfile::tempdir().unwrap();
    let profile = follow_profile(&format!("quic://{}", doq.addr()), "", "PROTOCOL,DOQ,DIRECT");
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = lookup_target(&engine).await;
    assert_eq!(
        res.source,
        Source::Upstream(format!("quic://{}", doq.addr()))
    );
    assert_eq!(doq.connections(), 1);
    let carriers = carrier_sessions(&engine);
    assert!(
        carriers
            .iter()
            .any(|r| r.protocol == Some(ProtocolKind::Doq)
                && r.rule.as_deref() == Some("PROTOCOL,DOQ,DIRECT")
                && r.policy == ["DIRECT"]
                && r.up > 0
                && r.down > 0
                && r.connect_ms.is_some()
                && r.first_byte_ms.is_some()
                && r.error.is_none()),
        "{carriers:?}"
    );
}

/// A rule that rejects the carrier would cost the resolver its server: the
/// carrier goes directly instead, noted, as a TCP DNS session does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_doq_carrier_routed_to_reject_goes_directly() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let dir = tempfile::tempdir().unwrap();
    let profile = follow_profile(&format!("quic://{}", doq.addr()), "", "PROTOCOL,DOQ,REJECT");
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = lookup_target(&engine).await;
    assert_eq!(
        res.source,
        Source::Upstream(format!("quic://{}", doq.addr()))
    );
    assert_eq!(doq.connections(), 1);
    let carriers = carrier_sessions(&engine);
    assert!(
        carriers.iter().any(|r| r.policy == ["REJECT"]
            && r.error.as_deref() == Some("dns-follow: reject bypassed to keep DNS working")
            && r.up > 0
            && r.down > 0),
        "{carriers:?}"
    );
}

/// A proxy configured by host name would need the very lookup the carrier
/// is for: the carrier goes directly instead, noted, and the proxy's name
/// is never asked for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_doq_carrier_bypasses_a_proxy_configured_by_host_name() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    dns.set("proxy.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let dir = tempfile::tempdir().unwrap();
    let profile = follow_profile(
        &format!("quic://{}", doq.addr()),
        "Up = socks5, proxy.test, 1080, udp-relay=true",
        "PROTOCOL,DOQ,Up",
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = lookup_target(&engine).await;
    assert_eq!(
        res.source,
        Source::Upstream(format!("quic://{}", doq.addr()))
    );
    let questions = dns.queries();
    assert!(
        questions.iter().all(|(q, _)| q.name != "proxy.test"),
        "{questions:?}"
    );
    let carriers = carrier_sessions(&engine);
    assert!(
        carriers.iter().any(|r| r.policy == ["Up"]
            && r.error.as_deref()
                == Some(
                    "dns-follow: proxy configured by host name bypassed to avoid a resolution loop"
                )),
        "{carriers:?}"
    );
}

/// A proxy that carries no UDP cannot take the carrier: that server fails
/// this time — not sent around the rule — while the resolver's other server
/// answers (phase 2 M7 design 7.3).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_doq_server_routed_to_a_proxy_without_udp_fails_while_another_answers() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let blocked = FakeDoq::spawn(&dns, DoqScript::default());
    let open = FakeDoq::spawn(&dns, DoqScript::default());
    let dir = tempfile::tempdir().unwrap();
    let profile = follow_profile(
        &format!("quic://{}, quic://{}", blocked.addr(), open.addr()),
        "NoUdp = http, 127.0.0.1, 9",
        &format!("DEST-PORT,{},NoUdp", blocked.addr().port()),
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = lookup_target(&engine).await;
    assert_eq!(
        res.source,
        Source::Upstream(format!("quic://{}", open.addr()))
    );
    let failed = wait_for_record(&engine, Duration::from_secs(5), |r| {
        r.dst == blocked.addr().to_string() && r.status == RecordStatus::Failed
    })
    .await
    .expect("the blocked server's carrier failed");
    assert_eq!(failed.transport, Transport::Udp);
    assert_eq!(failed.policy, ["NoUdp"]);
    assert_eq!(failed.error.as_deref(), Some("policy does not support UDP"));
    assert_eq!(blocked.connections(), 0);
    assert!(blocked.queries().is_empty());
}

/// Without `encrypted-dns-follow-outbound-mode` a carrier goes straight to
/// its server whatever the rules say: no internal session, no proxy.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_follow_mode_a_doq_carrier_ignores_the_rules() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let proxy = FakeSocks5::spawn(Socks5Script::default()).await;
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\nencrypted-dns-server = quic://{}\nencrypted-dns-skip-cert-verification = true\n\
ipv6 = false\n[Proxy]\nSocks = socks5, 127.0.0.1, {}, udp-relay=true\n[Proxy Group]\n\
[Rule]\nPROTOCOL,DOQ,Socks\nFINAL,REJECT\n",
        doq.addr(),
        proxy.addr().port()
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = lookup_target(&engine).await;
    assert_eq!(
        res.source,
        Source::Upstream(format!("quic://{}", doq.addr()))
    );
    assert_eq!(doq.connections(), 1);
    assert!(proxy.requests().is_empty());
    let internal = internal_sessions(&engine);
    assert!(internal.is_empty(), "{internal:?}");
```

`crates/rurge-engine/src/dns_pipeline.rs`——把

```rust
        assert_eq!(protocol_for(53), ProtocolKind::Dns);
    }
}
```

换成

```rust
        assert_eq!(protocol_for(53), ProtocolKind::Dns);
    }

    #[test]
    fn a_quic_carriers_tag_follows_the_port() {
        assert_eq!(quic_protocol_for(443), ProtocolKind::Doh3);
        assert_eq!(quic_protocol_for(853), ProtocolKind::Doq);
        assert_eq!(quic_protocol_for(8853), ProtocolKind::Doq);
    }

    /// A loopback UDP socket that sends every datagram back; its port.
    async fn udp_echo() -> u16 {
        let echo = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = echo.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut buf = [0u8; 64];
            while let Ok((n, from)) = echo.recv_from(&mut buf).await {
                let _ = echo.send_to(&buf[..n], from).await;
            }
        });
        port
    }

    /// Sends `payload` on `carrier`; what comes back.
    async fn echoed(carrier: &BoxedDatagram, payload: &[u8]) -> Vec<u8> {
        std::future::poll_fn(|cx| carrier.poll_send(cx, payload))
            .await
            .unwrap();
        let mut storage = [0u8; 64];
        let mut buf = ReadBuf::new(&mut storage);
        tokio::time::timeout(
            Duration::from_secs(5),
            std::future::poll_fn(|cx| carrier.poll_recv(cx, &mut buf)),
        )
        .await
        .expect("the echo within the bound")
        .unwrap();
        buf.filled().to_vec()
    }

    /// Unattached, the connector takes a QUIC carrier from the fallback as
    /// well: the loopback echo answers through it.
    #[tokio::test]
    async fn without_an_engine_a_carrier_comes_from_the_fallback() {
        let port = udp_echo().await;
        let fallback: Arc<dyn Connector> = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
        let pc = PipelineConnector::new(fallback);
        let target = Target::new(HostName::parse("127.0.0.1"), port);
        let carrier = tokio::time::timeout(
            Duration::from_secs(5),
            pc.connect_udp(&target, &ConnectOpts::default()),
        )
        .await
        .expect("within the bound")
        .expect("a carrier from the fallback");
        assert_eq!(echoed(&carrier, b"ping").await, b"ping");
    }

    /// An internal carrier counts what it sends as `up` and what comes back
    /// as `down` — the first datagram back is the first byte —, says where
    /// it sends to, and finishes its session when it is dropped.
    #[tokio::test]
    async fn an_internal_carrier_counts_both_ways_and_finishes_when_dropped() {
        let port = udp_echo().await;
        let target = Target::new(HostName::parse("127.0.0.1"), port);
        let handle = SessionHandle::new(1, SessionInfo::udp(target.host.clone(), port));
        let direct = DirectConnector::new(Arc::new(SystemResolve));
        let datagram = direct
            .connect_udp(&target, &ConnectOpts::default())
            .await
            .unwrap();
        let carrier = wrap_internal_udp(datagram, handle.clone());
        assert!(handle.connect_time().is_some());
        assert_eq!(
            carrier.peer_addr(),
            Some(SocketAddr::from(([127, 0, 0, 1], port)))
        );
        assert_eq!(echoed(&carrier, b"ping!").await, b"ping!");
        assert_eq!(handle.bytes(), (5, 5));
        assert!(handle.first_byte_seen());
        assert!(!handle.is_finished());
        drop(carrier);
        assert_eq!(handle.outcome(), Some(SessionOutcome::Completed));
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-engine --test pipeline`
Expected: FAIL——载体还是总直连，代理与请求记录都看不到它：

```text
test a_doq_carrier_bypasses_a_proxy_configured_by_host_name ... FAILED
test a_doq_server_routed_to_a_proxy_without_udp_fails_while_another_answers ... FAILED
test a_doh3_carrier_follows_an_ip_rule_through_a_socks5_proxy ... FAILED
test a_doq_carrier_routed_to_direct_is_an_internal_udp_session ... FAILED
test a_doq_carrier_follows_the_rules_through_a_socks5_proxy ... FAILED
test a_doq_carrier_routed_to_reject_goes_directly ... FAILED
test result: FAILED. 32 passed; 6 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.10s
error: test failed, to rerun pass `-p rurge-engine --test pipeline`
```

（`without_follow_mode_a_doq_carrier_ignores_the_rules` 此时就通过：它钉住不开跟随模式时行为不变。）

- [ ] **Step 3: 实现**

`crates/rurge-engine/src/engine.rs`——把

```rust
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_policy::auto::SelectCtx;
use rurge_policy::{Note, PolicyRegistry, TerminalKind};
use rurge_proto::{OutboundError, OutboundRef};
```

换成

```rust
use rurge_net::connector::{BoxedDatagram, BoxedStream, ConnectOpts, Connector, Target};
use rurge_net::packet_datagram::packet_datagram;
use rurge_policy::auto::SelectCtx;
use rurge_policy::{Note, PolicyRegistry, Resolution, TerminalKind};
use rurge_proto::{OutboundError, OutboundRef, UdpSupport};
```

`crates/rurge-engine/src/engine.rs`——把

```rust
        _ => matches!(spec.server, Some(HostName::Domain(_))),
    }
}

/// The bypass every anti-loop arm of `dial_internal` takes (a REJECT, a
```

换成

```rust
        _ => matches!(spec.server, Some(HostName::Domain(_))),
    }
}

/// The note on an internal DNS session that went directly instead of to a
/// REJECT, or to a chain that ends at one.
const REJECT_BYPASSED: &str = "dns-follow: reject bypassed to keep DNS working";

/// The bypass every anti-loop arm of `dial_internal` takes (a REJECT, a
```

`crates/rurge-engine/src/engine.rs`——把

```rust
impl Engine {
    /// Dials a DNS upstream connection through the pipeline (Internal session).
    /// Never lets the routing break DNS: a REJECT, and a proxy that would have
    /// to be resolved first, each warn and connect directly instead.
    pub async fn dial_internal(
        &self,
        session: SessionInfo,
        fallback: &Arc<dyn Connector>,
    ) -> io::Result<BoxedStream> {
        let (rt, registry) = self.snapshot();
        let handle = self.new_handle(session);
        let policy = match self.choose_policy(&rt, &registry, &handle).await {
```

换成

```rust
/// `bypass_to_direct` for the QUIC carrier of `dial_internal_udp`: the
/// carrier is `fallback`'s.
async fn bypass_udp_to_direct(
    handle: Arc<SessionHandle>,
    fallback: &Arc<dyn Connector>,
    target: &Target,
    opts: &ConnectOpts,
    why: &str,
) -> io::Result<BoxedDatagram> {
    handle.set_error(why);
    match fallback.connect_udp(target, opts).await {
        Ok(datagram) => Ok(crate::dns_pipeline::wrap_internal_udp(datagram, handle)),
        Err(e) => {
            handle.finish(SessionOutcome::Failed(e.to_string()));
            Err(e)
        }
    }
}

/// Where the routing sends an internal DNS session (`Engine::route_internal`).
enum InternalRoute {
    /// Through its policy.
    Policy(Resolution),
    /// Directly instead, for the reason noted on the session.
    Bypass(&'static str),
}

/// What an internal DNS session tells a `smart` group of the member it went
/// through (M3c design 4.3): that it could not connect, or — once its
/// outbound is ready — its first byte back, or three seconds without one;
/// a DNS session tries no other member (M3c-D5). Silence tells: a DNS
/// server answers at once what a session sends first, a query or a QUIC
/// handshake.
fn report_internal(
    registry: &PolicyRegistry,
    resolution: &Resolution,
    handle: &Arc<SessionHandle>,
    connected: Result<(), &OutboundError>,
) {
    let Some(pick) = &resolution.smart else {
        return;
    };
    let (book, now) = (&registry.auto().smart, Instant::now());
    let host = handle.session().dst_host.to_string();
    let outbound = &resolution.outbound;
    match connected {
        Ok(()) => {
            book.used(&pick.group, &pick.member, now);
            crate::smart::watch(handle, book.clone(), &pick.member, outbound, &host, true);
        }
        Err(e) if crate::smart::retryable(e) => {
            book.report_failure(&pick.member, outbound, Some(&host), now);
        }
        Err(_) => {}
    }
}

impl Engine {
    /// Mode / rule → policy for an internal DNS session (`dial_internal`,
    /// `dial_internal_udp`), and the bypasses decided before anything is
    /// dialled, each warned about.
    async fn route_internal(
        &self,
        rt: &Runtime,
        registry: &PolicyRegistry,
        handle: &SessionHandle,
    ) -> InternalRoute {
        let policy = match self.choose_policy(rt, registry, handle).await {
```

`crates/rurge-engine/src/engine.rs`——把

```rust
        handle.set_policy_chain(resolution.chain.clone());
        let target = Target::new(handle.session().dst_host.clone(), handle.session().dst_port);
        let opts = ConnectOpts {
            timeout: CONNECT_TIMEOUT,
        };
```

换成

```rust
        handle.set_policy_chain(resolution.chain.clone());
```

`crates/rurge-engine/src/engine.rs`——把

```rust
            match socket_opener(&registry, terminal) {
```

换成

```rust
            match socket_opener(registry, terminal) {
```

`crates/rurge-engine/src/engine.rs`——把

```rust
                    return bypass_to_direct(
                        handle,
                        fallback,
                        &target,
                        &opts,
                        "dns-follow: proxy configured by host name bypassed to avoid a resolution loop",
                    )
                    .await;
```

换成

```rust
                    return InternalRoute::Bypass(
                        "dns-follow: proxy configured by host name bypassed to avoid a resolution loop",
                    );
```

`crates/rurge-engine/src/engine.rs`——把

```rust
                    return bypass_to_direct(
                        handle,
                        fallback,
                        &target,
                        &opts,
                        "dns-follow: reject bypassed to keep DNS working",
                    )
                    .await;
```

换成

```rust
                    return InternalRoute::Bypass(REJECT_BYPASSED);
```

`crates/rurge-engine/src/engine.rs`——把

```rust
        let connected = resolution.outbound.connect_tcp(&target, &opts).await;
        // the member of a `smart` group reports how it went (M3c design 4.3);
        // a DNS session tries no other member (M3c-D5)
        if let Some(pick) = &resolution.smart {
            let (book, now) = (&registry.auto().smart, Instant::now());
            let host = handle.session().dst_host.to_string();
            match &connected {
                Ok(_) => {
                    book.used(&pick.group, &pick.member, now);
                    let outbound = &resolution.outbound;
                    crate::smart::watch(&handle, book.clone(), &pick.member, outbound, &host, true);
                }
                Err(e) if crate::smart::retryable(e) => {
                    book.report_failure(&pick.member, &resolution.outbound, Some(&host), now);
                }
                Err(_) => {}
            }
        }
```

换成

```rust
        InternalRoute::Policy(resolution)
    }

    /// Dials a DNS upstream connection through the pipeline (Internal session).
    /// Never lets the routing break DNS: a REJECT, and a proxy that would have
    /// to be resolved first, each warn and connect directly instead.
    pub async fn dial_internal(
        &self,
        session: SessionInfo,
        fallback: &Arc<dyn Connector>,
    ) -> io::Result<BoxedStream> {
        let (rt, registry) = self.snapshot();
        let handle = self.new_handle(session);
        let target = Target::new(handle.session().dst_host.clone(), handle.session().dst_port);
        let opts = ConnectOpts {
            timeout: CONNECT_TIMEOUT,
        };
        let resolution = match self.route_internal(&rt, &registry, &handle).await {
            InternalRoute::Policy(resolution) => resolution,
            InternalRoute::Bypass(why) => {
                return bypass_to_direct(handle, fallback, &target, &opts, why).await;
            }
        };
        let connected = resolution.outbound.connect_tcp(&target, &opts).await;
        report_internal(
            &registry,
            &resolution,
            &handle,
            connected.as_ref().map(|_| ()),
        );
```

`crates/rurge-engine/src/engine.rs`——把

```rust
                bypass_to_direct(
                    handle,
                    fallback,
                    &target,
                    &opts,
                    "dns-follow: reject bypassed to keep DNS working",
                )
                .await
```

换成

```rust
                bypass_to_direct(handle, fallback, &target, &opts, REJECT_BYPASSED).await
```

`crates/rurge-engine/src/engine.rs`——把

```rust
                Err(io::Error::other(message))
```

换成

```rust
                Err(io::Error::other(message))
            }
        }
    }

    /// Opens the QUIC carrier of a DNS server (DoQ, DoH3) through the
    /// pipeline, as an Internal UDP session (phase 2 M7 design 7.3): the UDP
    /// of the policy the rules pick — DIRECT's with its socket settings, as
    /// a TCP session gets DIRECT's TCP. The routing keeps DNS working as
    /// `dial_internal`'s does: a REJECT, and a proxy that would have to be
    /// resolved first, each warn and go directly instead. A proxy that
    /// carries no UDP fails the server this time, with no DIRECT in its
    /// place: the resolver asks its other servers.
    pub async fn dial_internal_udp(
        &self,
        session: SessionInfo,
        fallback: &Arc<dyn Connector>,
    ) -> io::Result<BoxedDatagram> {
        let (rt, registry) = self.snapshot();
        let handle = self.new_handle(session);
        let target = Target::new(handle.session().dst_host.clone(), handle.session().dst_port);
        let opts = ConnectOpts {
            timeout: CONNECT_TIMEOUT,
        };
        let resolution = match self.route_internal(&rt, &registry, &handle).await {
            InternalRoute::Policy(resolution) => resolution,
            InternalRoute::Bypass(why) => {
                return bypass_udp_to_direct(handle, fallback, &target, &opts, why).await;
            }
        };
        // a TCP session learns of a REJECT from `connect_tcp`; `open_udp`
        // says of it only what it says of any outbound without UDP
        if resolution.terminal == TerminalKind::Reject {
            tracing::warn!(
                dst = %target.host,
                "DNS session routed to a reject/unsupported policy; connecting directly to keep DNS working"
            );
            return bypass_udp_to_direct(handle, fallback, &target, &opts, REJECT_BYPASSED).await;
        }
        // what the resolver hears names the policy: the text alone could
        // pass for the DNS server's own (`tls: …`)
        let policy = resolution
            .chain
            .last()
            .map(String::as_str)
            .unwrap_or_default();
        if resolution.outbound.udp() == UdpSupport::Unsupported {
            let why = "policy does not support UDP";
            handle.finish(SessionOutcome::Failed(why.to_string()));
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("via {policy}: {why}"),
            ));
        }
        let opened = resolution.outbound.open_udp(&opts).await;
        report_internal(&registry, &resolution, &handle, opened.as_ref().map(|_| ()));
        match opened {
            // the server is an address: nothing for the carrier to look up
            Ok(socket) => Ok(crate::dns_pipeline::wrap_internal_udp(
                packet_datagram(socket, target),
                handle,
            )),
            Err(e) => {
                handle.finish(SessionOutcome::Failed(e.to_string()));
                Err(io::Error::other(format!("via {policy}: {e}")))
```

`crates/rurge-engine/src/dns_pipeline.rs`——把

```rust
//! pipeline as `Internal` sessions. The resolver wraps it in its
//! `BootstrapConnector`, which resolves a domain-configured upstream name via
//! plain-UDP bootstrap first and hands this connector an IP target — so the
//! pipeline never needs to resolve anything itself and cannot re-enter the
//! resolver. The `Domain` arm below is only a defensive fallback. UDP
//! upstreams never use a connector; the QUIC carriers of the DoQ / DoH3
//! upstreams (`connect_udp`) go straight to the server through the
//! fallback, the rules aside.
```

换成

```rust
//! pipeline as `Internal` sessions — and the QUIC carriers of its DoQ / DoH3
//! upstreams as `Internal` UDP sessions (phase 2 M7 design 7.3). The resolver
//! wraps it in its `BootstrapConnector`, which resolves a domain-configured
//! upstream name via plain-UDP bootstrap first and hands this connector an IP
//! target — so the pipeline never needs to resolve anything itself and cannot
//! re-enter the resolver. The `Domain` arms below are only a defensive
//! fallback. Plain UDP upstreams never use a connector.
```

`crates/rurge-engine/src/dns_pipeline.rs`——把

```rust
use rurge_net::connector::{BoxedDatagram, BoxedStream, ConnectOpts, Connector, Target};
use std::io;
```

换成

```rust
use rurge_net::connector::{BoxedDatagram, BoxedStream, ConnectOpts, Connector, Datagram, Target};
use std::io;
use std::net::SocketAddr;
```

`crates/rurge-engine/src/dns_pipeline.rs`——把

```rust
        _ => ProtocolKind::Dns,
    }
}

impl Connector for PipelineConnector {
```

换成

```rust
        _ => ProtocolKind::Dns,
    }
}

/// DNS server port → the protocol tag of its QUIC carrier's session: the
/// only UDP this connector carries is DoH3's and DoQ's, told apart by the
/// port (best-effort, as `protocol_for`).
fn quic_protocol_for(port: u16) -> ProtocolKind {
    match port {
        443 => ProtocolKind::Doh3,
        _ => ProtocolKind::Doq,
    }
}

impl Connector for PipelineConnector {
```

`crates/rurge-engine/src/dns_pipeline.rs`——把

```rust
    /// A QUIC carrier of the DoQ / DoH3 upstreams: straight to the server,
    /// through the fallback.
```

换成

```rust
    /// The QUIC carrier of a DoQ / DoH3 upstream, routed as `connect` routes
    /// a TCP connection: an `Internal` UDP session. `as_direct` stays
    /// `None`, so the QUIC path always takes its carrier from here.
```

`crates/rurge-engine/src/dns_pipeline.rs`——把

```rust
        self.fallback.connect_udp(target, opts)
```

换成

```rust
        Box::pin(async move {
            let ip = match &target.host {
                HostName::Ip(ip) => *ip,
                // Defensive only, as for `connect`.
                HostName::Domain(_) => return self.fallback.connect_udp(target, opts).await,
            };
            let engine = self.engine.get().and_then(Weak::upgrade);
            let Some(engine) = engine else {
                // Not attached yet (bootstrap phase) → direct.
                return self.fallback.connect_udp(target, opts).await;
            };
            let mut session = SessionInfo::udp(HostName::Ip(ip), target.port);
            session.listener = ListenerKind::Internal;
            session.protocol = Some(quic_protocol_for(target.port));
            engine.dial_internal_udp(session, &self.fallback).await
        })
```

`crates/rurge-engine/src/dns_pipeline.rs`——把

```rust
/// Finishes the session handle when the DNS connection is dropped, so the
/// internal session leaves the active index and lands in the request log.
```

换成

```rust
/// Finishes the session handle when the DNS connection — or QUIC carrier —
/// is dropped, so the internal session leaves the active index and lands in
/// the request log.
```

`crates/rurge-engine/src/dns_pipeline.rs`——把

```rust
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Wraps a freshly dialed stream so bytes are counted (`Counting`) and the
```

换成

```rust
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl<S: Datagram> Datagram for FinishOnDrop<S> {
    fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.inner.poll_send(cx, buf)
    }
    fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        self.inner.poll_recv(cx, buf)
    }
    fn peer_addr(&self) -> Option<SocketAddr> {
        self.inner.peer_addr()
    }
    fn set_tos(&self, tos: u8) -> io::Result<()> {
        self.inner.set_tos(tos)
    }
}

/// Wraps a freshly dialed stream so bytes are counted (`Counting`) and the
```

`crates/rurge-engine/src/dns_pipeline.rs`——把

```rust
        Counting::new(stream, handle.clone()),
```

换成

```rust
        Counting::new(stream, handle.clone()),
        handle,
    ))
}

/// Counts a carrier's datagrams into a session handle, as `Counting` counts
/// a stream's bytes: what it receives is `down`, what it sends `up`.
struct CountingDatagram {
    inner: BoxedDatagram,
    handle: Arc<SessionHandle>,
}

impl Datagram for CountingDatagram {
    fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let sent = self.inner.poll_send(cx, buf);
        if let Poll::Ready(Ok(n)) = &sent {
            self.handle.add_up(*n as u64);
        }
        sent
    }
    fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let received = self.inner.poll_recv(cx, buf);
        if let Poll::Ready(Ok(())) = &received {
            let n = buf.filled().len() - before;
            self.handle.add_down(n as u64);
            if n > 0 {
                self.handle.mark_first_byte();
            }
        }
        received
    }
    fn peer_addr(&self) -> Option<SocketAddr> {
        self.inner.peer_addr()
    }
    fn set_tos(&self, tos: u8) -> io::Result<()> {
        self.inner.set_tos(tos)
    }
}

/// `wrap_internal` for a freshly opened QUIC carrier: its datagrams are
/// counted (`CountingDatagram`) and the handle is finished on drop
/// (`FinishOnDrop`); the outbound is ready as of now.
pub(crate) fn wrap_internal_udp(
    datagram: BoxedDatagram,
    handle: Arc<SessionHandle>,
) -> BoxedDatagram {
    handle.mark_connected();
    Box::new(FinishOnDrop::new(
        CountingDatagram {
            inner: datagram,
            handle: handle.clone(),
        },
```

`crates/rurge-engine/src/runtime.rs`——把

```rust
    /// The connector this generation's resolver dials TCP / DoT / DoH upstreams
    /// through, when `encrypted-dns-follow-outbound-mode` is on.
```

换成

```rust
    /// The connector this generation's resolver reaches its TCP / DoT / DoH /
    /// DoQ / DoH3 upstreams through, when `encrypted-dns-follow-outbound-mode`
    /// is on.
```

`crates/rurge-engine/src/stack.rs`——把

```rust
    /// Connector for the resolver's TCP / DoT / DoH upstreams; `None` = plain
    /// direct. `Runtime::build` injects the pipeline connector here when
```

换成

```rust
    /// Connector for the resolver's TCP / DoT / DoH upstreams and the QUIC
    /// carriers of its DoQ / DoH3 ones; `None` = plain direct.
    /// `Runtime::build` injects the pipeline connector here when
```

要点：
- DIRECT 用 DIRECT 出站自己的 UDP（`open_udp` + `packet_datagram`），与 TCP 的 DIRECT 一样带 interface / tos / hook 的设置；fallback 只用于绕过。
- 链底为 REJECT 时在打开载体之前判断（REJECT 出站的 `open_udp` 与"不载 UDP"分不开）。
- 不载 UDP 的代理只让这个上游这次失败；交给解析器的错误一律带 `via {策略}: `，免得代理自己的 `tls: …` 被当成 DNS 服务器的 TLS 错误。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-engine --test pipeline` → 通过（新增 7 条：DoQ 经规则走 socks5 代理、DoH3 经 IP 规则走 socks5 代理（标签按端口是 DOQ）、走 DIRECT 的是内部 UDP 会话、走 REJECT 的直连、名字写的代理被绕过、走不载 UDP 的代理时那个上游失败而另一个应答、不开跟随模式时不走规则；取代 Task 6 的 `a_follow_mode_resolver_reaches_a_doq_server`）。
Run: `cargo test -p rurge-engine --lib dns_pipeline` → 通过（新增 3 条）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-engine
git commit -m "feat(engine): 跟随出站模式下 DoQ / DoH3 的 QUIC 载体按规则走——内部 UDP 会话"
```

### Task 8: 引擎装配与能力表

去掉 Task 1 的门与过渡物：`masque` 行有了 spec，`h3=true` 不再是 `W0029`；引擎工厂为 `masque` 与 `h3=true` 的 `trust-tunnel` 构建 QUIC 上的出站，能力表加入 `masque`（P18）；经引擎的端到端用例。

**Files:**
- Create: `crates/rurge-engine/tests/outbounds_masque.rs`
- Modify: `crates/rurge-config/src/spec/{mod.rs, h2.rs}`（与用例）、`tests/policy_spec.rs`、`tests/snapshots/corpus__corpus__kitchen-sink.snap`、`crates/rurge-engine/src/outbounds.rs`（与用例）、`tests/{common/mod.rs, outbounds_http2.rs}`、`crates/rurge/src/capabilities.rs`、`crates/rurge/tests/cli.rs`

**Interfaces:**
- Consumes: Task 1–7 的全部；`MasqueOutbound::new` 与 `TrustTunnelH3Outbound::new`（参数次序同 `Hysteria2Outbound::new`）。
- Produces: 工厂的 `ProtoSpec::Masque` 分支与 `ProtoSpec::TrustTunnel(tt) if tt.h3` 分支；能力表的 `masque`；`tests/common` 再导出 `FakeH3Proxy` / `H3ProxyScript`

- [ ] **Step 1: 先写用例**

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
pub use rurge_proto_quic::testing::{FakeHysteria2, FakeTuic, Hysteria2Script, TuicScript};
```

换成

```rust
pub use rurge_proto_quic::testing::{
    FakeH3Proxy, FakeHysteria2, FakeTuic, H3ProxyScript, Hysteria2Script, TuicScript,
};
```

新建 `crates/rurge-engine/tests/outbounds_masque.rs`：

```rust
//! Sessions that leave through `masque` (phase 2 M7 design 7.1): TCP and
//! UDP through the engine to the loopback `FakeH3Proxy` — classic CONNECTs
//! and CONNECT-UDP streams on the policy's one QUIC connection, a refused
//! login, a server without CONNECT-UDP, a packet too long for a datagram,
//! port hopping, `underlying-proxy` (a SOCKS5 proxy's UDP), policy tests
//! (URL and UDP) and reloads.

mod common;
use common::*;
use rurge_config::HostName;
use rurge_config::spec::ProtoSpec;
use rurge_net::connector::Target;
use rurge_net::quic::testing::MultiPort;
use rurge_proto::testing::{echo_server, udp_echo_server};
use rurge_proto_quic::testing::RecordedH3Request;

/// The credentials the fake lets in, as a line writes them.
const LOGIN: &str = "username=u, password=p";
/// `proxy-authorization` for them: base64("u:p").
const BASIC: &str = "Basic dTpw";

/// A fake that lets the user `u` in with the password `p`.
fn script() -> H3ProxyScript {
    H3ProxyScript {
        users: vec![("u".into(), "p".into())],
        ..H3ProxyScript::default()
    }
}

/// `M = masque, 127.0.0.1, <the fake's port>, <params>`.
fn line(fake: &FakeH3Proxy, params: &str) -> String {
    format!("M = masque, 127.0.0.1, {}, {params}", fake.addr().port())
}

/// `line` with the credentials the fake lets in and nothing else.
fn plain_line(fake: &FakeH3Proxy) -> String {
    line(fake, LOGIN)
}

/// A profile that sends the name `target.test` and the loopback through
/// `M`.
fn profile(proxies: &str) -> Profile<'_> {
    Profile {
        proxies,
        rules: "DOMAIN,target.test,M\nIP-CIDR,127.0.0.1/32,M,no-resolve",
        ..Profile::default()
    }
}

/// What makes an engine trust `fake`'s certificate.
fn trusting(fake: &FakeH3Proxy) -> EngineShared {
    EngineShared {
        roots: Some(fake.roots()),
        ..EngineShared::default()
    }
}

/// An engine on `proxies` (`profile`) that trusts `fake`.
async fn through(fake: &FakeH3Proxy, proxies: &str) -> Harness {
    harness_with(profile(proxies), trusting(fake)).await
}

/// A classic CONNECT to `authority` on connection `n`, with the
/// credentials, as the fake records it.
fn connect(n: usize, authority: &str) -> RecordedH3Request {
    RecordedH3Request {
        connection: n,
        method: "CONNECT".into(),
        protocol: None,
        scheme: None,
        authority: Some(authority.into()),
        path: None,
        headers: vec![("proxy-authorization".into(), BASIC.into())],
    }
}

/// The CONNECT-UDP to `target` on connection 0, with the credentials, as
/// the fake records it: to the server's own authority, the target in the
/// default template's path.
fn connect_udp(fake: &FakeH3Proxy, target: SocketAddr) -> RecordedH3Request {
    RecordedH3Request {
        connection: 0,
        method: "CONNECT".into(),
        protocol: Some("connect-udp".into()),
        scheme: Some("https".into()),
        authority: Some(fake.addr().to_string()),
        path: Some(format!(
            "/.well-known/masque/udp/{}/{}/",
            target.ip(),
            target.port()
        )),
        headers: vec![
            ("proxy-authorization".into(), BASIC.into()),
            ("capsule-protocol".into(), "?1".into()),
        ],
    }
}

/// A CONNECT through the HTTP listener and one through the SOCKS5 listener
/// each leave as a classic CONNECT of their own on the policy's one QUIC
/// connection, both with the credentials; the server is asked for the name.
#[tokio::test]
async fn tcp_leaves_through_masque_on_one_connection() {
    let echo = echo_server().await;
    let fake = FakeH3Proxy::spawn(H3ProxyScript {
        connect_to: Some(echo),
        ..script()
    });
    let h = through(&fake, &plain_line(&fake)).await;
    let mut http = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut http, b"through masque").await;
    echo_through(&mut http, &vec![0x5a; 300_000]).await;
    let mut socks = connect_via_socks5(h.socks(), "target.test", 7).await;
    echo_through(&mut socks, b"and from SOCKS5").await;
    assert_eq!(fake.requests(), vec![connect(0, "target.test:7"); 2]);
    assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
    assert_eq!(fake.connections(), 1);
    drop((http, socks));
    for record in the_records(&h, 2).await {
        assert_eq!(record.policy, ["M"], "{record:?}");
        assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
    }
}

/// Credentials the proxy refuses fail the session with the text that says
/// so — the CONNECT is answered 502 —, and the password is nowhere in the
/// record.
#[tokio::test]
async fn refused_credentials_fail_the_session() {
    let fake = FakeH3Proxy::spawn(script());
    let h = through(&fake, &line(&fake, "username=u, password=wr0ngPw")).await;
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
        Some("masque: proxy authentication required")
    );
    assert!(!format!("{record:?}").contains("wr0ngPw"));
    assert_eq!(fake.requests().len(), 1);
}

/// Datagrams through one association to two echoes (design 7.1): each
/// target has a CONNECT-UDP stream of its own on the policy's one
/// connection, with the credentials; every packet goes in an HTTP/3
/// datagram with context id 0, and every answer counts as its stream's
/// target's. Every flow ends with the association.
#[tokio::test]
async fn udp_goes_through_masque_as_connect_udp() {
    let fake = FakeH3Proxy::spawn(script());
    let h = through(&fake, &plain_line(&fake)).await;
    let (one, two) = (udp_echo_server().await, udp_echo_server().await);
    let association = udp_associate(h.socks()).await;
    for (echo, payload) in [(one, &b"one"[..]), (two, b"two"), (one, b"again")] {
        association.send("127.0.0.1", echo.port(), payload).await;
        assert_eq!(association.recv().await, (echo, payload.to_vec()));
    }
    assert_eq!(
        fake.requests(),
        [connect_udp(&fake, one), connect_udp(&fake, two)]
    );
    let datagrams: Vec<(usize, u64, u64, usize)> = fake
        .datagrams()
        .iter()
        .map(|d| (d.connection, d.stream, d.context, d.len))
        .collect();
    assert_eq!(datagrams, [(0, 0, 0, 3), (0, 4, 0, 3), (0, 0, 0, 5)]);
    assert_eq!(fake.connections(), 1);
    drop(association);
    wait_until("both flows to finish", || udp_records(&h).len() == 2).await;
    for r in udp_records(&h) {
        assert_eq!(r.policy, ["M"], "{r:?}");
        assert_eq!(r.status, RecordStatus::Completed, "{r:?}");
    }
}

/// A server that does not take CONNECT-UDP — no extended CONNECT in its
/// SETTINGS — fails the flow with the text that says so, and no request
/// goes out; TCP goes on over the same connection (design 7.1).
#[tokio::test]
async fn a_server_without_connect_udp_fails_the_flow_and_carries_tcp() {
    let (echo, udp_echo) = (echo_server().await, udp_echo_server().await);
    let fake = FakeH3Proxy::spawn(H3ProxyScript {
        no_extended_connect: true,
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
        Some("masque: the server does not support CONNECT-UDP")
    );
    assert!(fake.requests().is_empty());
    let mut tunnel = connect_via_http(h.http(), &echo.to_string()).await;
    echo_through(&mut tunnel, b"TCP still").await;
    assert_eq!(fake.requests(), [connect(0, &echo.to_string())]);
    assert_eq!(fake.connections(), 1);
}

/// A packet longer than an HTTP/3 datagram of the connection carries is
/// dropped alone — no DATAGRAM capsule takes it instead (RFC 9297 3.5, RFC
/// 9298 6.1) —: the flow goes on, and its record says why.
#[tokio::test]
async fn a_packet_too_long_for_a_datagram_is_dropped_alone() {
    let fake = FakeH3Proxy::spawn(script());
    let h = through(&fake, &plain_line(&fake)).await;
    let echo = udp_echo_server().await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"one").await;
    assert_eq!(association.recv().await, (echo, b"one".to_vec()));
    // QUIC datagrams of a loopback path hold about 1,200 to 1,400 bytes
    association
        .send("127.0.0.1", echo.port(), &vec![0x4c; 3000])
        .await;
    association.send("127.0.0.1", echo.port(), b"after").await;
    assert_eq!(association.recv().await, (echo, b"after".to_vec()));
    let sent: Vec<usize> = fake.datagrams().iter().map(|d| d.len).collect();
    assert_eq!(sent, [3, 5]);
    drop(association);
    wait_until("the flow to finish", || !udp_records(&h).is_empty()).await;
    let record = &udp_records(&h)[0];
    assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
    assert!(
        record
            .error
            .as_deref()
            .is_some_and(|e| e.starts_with("masque: a datagram longer than ")),
        "{record:?}"
    );
}

/// Every `masque` policy of `cfg` hops every `interval`: the loader takes
/// no `port-hopping-interval` under 5 seconds.
fn hop_every(cfg: &mut rurge_config::Config, interval: Duration) {
    for spec in &mut cfg.specs {
        if let ProtoSpec::Masque(masque) = &mut spec.proto
            && let Some(hopping) = &mut masque.quic.port_hopping
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
    let fake = FakeH3Proxy::spawn_on(script(), ports.clone());
    let set: Vec<String> = ports.ports().iter().map(u16::to_string).collect();
    let proxies = line(&fake, &format!("{LOGIN}, port-hopping={}", set.join(";")));
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

/// `masque` over a SOCKS5 `underlying-proxy` (design 3.3): the QUIC
/// connection goes in datagrams of the entry's UDP association, to the
/// server as the line names it, and carries TCP and UDP alike.
#[tokio::test]
async fn masque_goes_over_an_underlying_socks5_proxy() {
    let (echo, udp_echo) = (echo_server().await, udp_echo_server().await);
    let fake = FakeH3Proxy::spawn(script());
    let entry = FakeSocks5::spawn(Socks5Script::default()).await;
    let proxies = format!(
        "Entry = socks5, 127.0.0.1, {}, udp-relay=true\n{}",
        entry.addr().port(),
        line(&fake, &format!("{LOGIN}, underlying-proxy=Entry"))
    );
    let h = through(&fake, &proxies).await;
    let mut tunnel = connect_via_http(h.http(), &echo.to_string()).await;
    echo_through(&mut tunnel, b"socks5 then masque").await;
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

/// A policy test fetches the policy's `test-url` through its connection:
/// one tunnel to the loopback origin, two HEADs on it.
#[tokio::test]
async fn a_policy_test_goes_through_the_connection() {
    let origin = TestServer::spawn().await;
    let fake = FakeH3Proxy::spawn(script());
    let h = through(
        &fake,
        &line(&fake, &format!("{LOGIN}, test-url={}", origin.url("/"))),
    )
    .await;
    let results = h
        .engine
        .test_policies(&["M".to_string()], None)
        .await
        .unwrap();
    let result = results[0].1.as_ref().expect("a masque policy is tested");
    assert!(result.outcome.is_ok(), "{:?}", result.outcome);
    let port = origin.url("/").port().unwrap();
    assert_eq!(fake.requests(), [connect(0, &format!("127.0.0.1:{port}"))]);
    let methods: Vec<String> = origin.requests().into_iter().map(|r| r.method).collect();
    assert_eq!(methods, ["HEAD", "HEAD"]);
}

/// The UDP test (`test-udp`; phase 2 M5 design 8.4) asks its question
/// through CONNECT-UDP, to the server the line names — which the fake
/// relays to a DNS server on loopback —, and the answer counts as that
/// server's.
#[tokio::test]
async fn a_udp_test_asks_through_connect_udp() {
    let dns = MockDns::spawn().await;
    let fake = FakeH3Proxy::spawn(H3ProxyScript {
        connect_to: Some(dns.addr()),
        ..script()
    });
    let h = through(
        &fake,
        &line(&fake, &format!("{LOGIN}, test-udp=probe.test@192.0.2.53")),
    )
    .await;
    let results = h.engine.test_udp(&["M".to_string()]).await;
    assert!(matches!(results[0].1, Some(Ok(_))), "{results:?}");
    let to = SocketAddr::from(([192, 0, 2, 53], 53));
    assert_eq!(fake.requests(), [connect_udp(&fake, to)]);
    let asked: Vec<String> = dns.queries().into_iter().map(|(q, _)| q.name).collect();
    assert_eq!(asked, ["probe.test"]);
}

/// The outbound — and with it its QUIC connection — is kept across a
/// reload that leaves the line alone, and rebuilt when its credentials,
/// `ecn`, port hopping or `sni` change (design 3.5, 8).
#[tokio::test]
async fn a_reload_keeps_an_unchanged_policy_and_its_connection() {
    let echo = echo_server().await;
    let fake = FakeH3Proxy::spawn(H3ProxyScript {
        connect_to: Some(echo),
        ..script()
    });
    let base = plain_line(&fake);
    let h = through(&fake, &base).await;
    let reload = |proxies: String| {
        let next = profile(&proxies).text(h.dns.addr());
        let dir = h.dir.path().to_path_buf();
        let shared = h.engine.shared();
        async move { runtime(&dir, &next, shared).await }
    };
    one_session(&h, 1, b"before the reload").await;
    let before = outbound_now(&h, "M");
    h.engine
        .swap_runtime(reload(format!("{base}\nOther = http, other.example, 8080")).await);
    assert!(
        Arc::ptr_eq(&before, &outbound_now(&h, "M")),
        "an unrelated reload rebuilt M"
    );
    one_session(&h, 2, b"after the reload").await;
    let places: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
    assert_eq!(places, [0, 0], "the QUIC connection was kept");
    assert_eq!(fake.connections(), 1);

    let mut previous = before;
    for changed in [
        "username=u, password=0ther",
        "username=v, password=0ther",
        "username=v, password=0ther, ecn=off",
        "username=v, password=0ther, ecn=off, port-hopping=5000-6000",
        "username=v, password=0ther, ecn=off, port-hopping=5000-6000, sni=localhost",
        // without credentials
        "ecn=off, port-hopping=5000-6000, sni=localhost",
    ] {
        h.engine.swap_runtime(reload(line(&fake, changed)).await);
        let now = outbound_now(&h, "M");
        assert!(!Arc::ptr_eq(&previous, &now), "kept after: {changed}");
        previous = now;
    }
}
```

`crates/rurge-engine/tests/outbounds_http2.rs`——把

```rust
//! over CONNECT-UDP (and where there is none), `underlying-proxy`, reloads.
```

换成

```rust
//! over CONNECT-UDP (and where there is none), `underlying-proxy`, reloads.
//! With `h3=true` a `trust-tunnel` goes over HTTP/3 instead (phase 2 M7
//! design 7.2), to the loopback `FakeH3Proxy`: tunnels, a refused login,
//! `max-streams` per QUIC connection, reloads, and no Shadow TLS.
```

`crates/rurge-engine/tests/outbounds_http2.rs`——把

```rust
        previous = now;
    }
}

```

换成

```rust
        previous = now;
    }
}

/// A fake TrustTunnel endpoint over HTTP/3, as v1.1.0 answers there,
/// `script` aside: quiche's grease, neither extended CONNECT nor HTTP
/// Datagrams, its challenge on a 407, and the user `u` with the password
/// `p`.
fn h3_endpoint(script: H3ProxyScript) -> FakeH3Proxy {
    FakeH3Proxy::spawn(H3ProxyScript {
        users: vec![("u".into(), "p".into())],
        challenge: Some("Basic realm=Authorization Required".into()),
        grease: true,
        no_extended_connect: true,
        no_h3_datagram: true,
        no_datagrams: true,
        ..script
    })
}

/// `P = trust-tunnel, …, h3=true` to `fake`, with `params` after it.
fn h3_line(fake: &FakeH3Proxy, params: &str) -> String {
    format!(
        "P = trust-tunnel, 127.0.0.1, {}, h3=true, {params}",
        fake.addr().port()
    )
}

/// An engine on `proxies`, as `through`, that trusts `fake`'s certificate.
async fn h3_through(fake: &FakeH3Proxy, proxies: &str) -> Harness {
    harness_trusting(
        Profile {
            proxies,
            rules: "DOMAIN,target.test,P\nIP-CIDR,127.0.0.1/32,P,no-resolve",
            ..Profile::default()
        },
        fake.roots(),
    )
    .await
}

/// With `h3=true` (phase 2 M7 design 7.2) the tunnel is a classic CONNECT
/// on the policy's QUIC connection, with the credentials and `user-agent`
/// as over HTTP/2; the name is left to the endpoint.
#[tokio::test]
async fn a_connect_leaves_through_trust_tunnel_over_http3() {
    let echo = rurge_proto::testing::echo_server().await;
    let fake = h3_endpoint(H3ProxyScript {
        require_user_agent: true,
        connect_to: Some(echo),
        ..H3ProxyScript::default()
    });
    let h = h3_through(&fake, &h3_line(&fake, "username=u, password=p")).await;
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"through trust-tunnel over HTTP/3").await;
    echo_through(&mut tunnel, &vec![0x5a; 300_000]).await;
    let seen = fake.requests();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(
        (seen[0].method.as_str(), seen[0].authority.as_deref()),
        ("CONNECT", Some("target.test:7")),
        "the endpoint resolves the name"
    );
    // base64("u:p")
    assert_eq!(seen[0].header("proxy-authorization"), Some("Basic dTpw"));
    assert_eq!(seen[0].header("user-agent"), Some("rurge"));
    assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
    drop(tunnel);
    let record = the_records(&h, 1).await[0].clone();
    assert_eq!(record.policy, ["P"]);
    assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
}

/// Over HTTP/3 as over HTTP/2: a login the endpoint refuses fails the
/// session with the text that says so, and the password is nowhere in the
/// record.
#[tokio::test]
async fn a_refused_trust_tunnel_login_over_http3_fails_the_session() {
    let fake = h3_endpoint(H3ProxyScript::default());
    let h = h3_through(&fake, &h3_line(&fake, "username=u, password=wr0ngPw")).await;
    let record = a_failed_session(&h).await;
    assert_eq!(
        record.error.as_deref(),
        Some("trust-tunnel: authentication failed")
    );
    assert!(!format!("{record:?}").contains("wr0ngPw"));
    assert_eq!(fake.requests().len(), 1);
}

/// Over HTTP/3, `max-streams` bounds the tunnels of each QUIC connection
/// (design 7.2): five open at once, two to a connection, take three.
#[tokio::test]
async fn max_streams_bounds_the_tunnels_of_each_quic_connection() {
    let echo = rurge_proto::testing::echo_server().await;
    let fake = h3_endpoint(H3ProxyScript {
        connect_to: Some(echo),
        ..H3ProxyScript::default()
    });
    let h = h3_through(
        &fake,
        &h3_line(&fake, "username=u, password=p, max-streams=2"),
    )
    .await;
    let mut tunnels = Vec::new();
    for n in 0..5u8 {
        let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
        echo_through(&mut tunnel, &[n; 16]).await;
        tunnels.push(tunnel);
    }
    // every tunnel is still open
    for tunnel in &mut tunnels {
        echo_through(tunnel, b"still here").await;
    }
    assert_eq!(fake.connections(), 3);
    let places: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
    assert_eq!(places, [0, 0, 1, 1, 2]);
}

/// Over HTTP/3 too, the outbound — and with it its QUIC connection — is
/// kept across a reload that leaves the line alone, and rebuilt when its
/// credentials, `headers`, `max-streams`, `ecn` or transport change.
#[tokio::test]
async fn a_reload_keeps_an_unchanged_trust_tunnel_over_http3_and_its_connection() {
    let echo = rurge_proto::testing::echo_server().await;
    let fake = h3_endpoint(H3ProxyScript {
        connect_to: Some(echo),
        ..H3ProxyScript::default()
    });
    let policy = |params: &str| {
        format!(
            "P = trust-tunnel, 127.0.0.1, {}, {params}",
            fake.addr().port()
        )
    };
    let base = "h3=true, username=u, password=p";
    let h = h3_through(&fake, &policy(base)).await;
    let reload = |proxies: String| {
        let next = Profile {
            proxies: &proxies,
            rules: "DOMAIN,target.test,P",
            ..Profile::default()
        }
        .text(h.dns.addr());
        let dir = h.dir.path().to_path_buf();
        let shared = h.engine.shared();
        async move { runtime(&dir, &next, shared).await }
    };
    one_session(&h, 1, b"before the reload").await;
    let before = outbound_now(&h, "P");
    h.engine.swap_runtime(
        reload(format!(
            "{}\nOther = http, other.example, 8080",
            policy(base)
        ))
        .await,
    );
    assert!(
        Arc::ptr_eq(&before, &outbound_now(&h, "P")),
        "an unrelated reload rebuilt P"
    );
    one_session(&h, 2, b"after the reload").await;
    assert_eq!(fake.connections(), 1, "the QUIC connection was kept");
    let places: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
    assert_eq!(places, [0, 0]);

    let mut previous = before;
    for changed in [
        "h3=true, username=u, password=0ther",
        "h3=true, username=u, password=0ther, headers=X-Id:1",
        "h3=true, username=u, password=0ther, headers=X-Id:1, max-streams=8",
        "h3=true, username=u, password=0ther, headers=X-Id:1, max-streams=8, ecn=off",
        // the same over HTTP/2
        "username=u, password=0ther, headers=X-Id:1, max-streams=8",
    ] {
        h.engine.swap_runtime(reload(policy(changed)).await);
        let now = outbound_now(&h, "P");
        assert!(!Arc::ptr_eq(&previous, &now), "kept after: {changed}");
        previous = now;
    }
}

/// Shadow TLS wraps a TCP connection (phase 2 M7 design 3.1): a
/// `trust-tunnel` line with `h3=true` and a Shadow TLS layer is refused at
/// load — an error at its line — and has nothing to build.
#[test]
fn trust_tunnel_over_http3_with_shadow_tls_is_refused_at_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.conf");
    std::fs::write(
        &path,
        "[Proxy]\nP = trust-tunnel, 127.0.0.1, 443, username=u, password=p, h3=true, shadow-tls-password=st, shadow-tls-version=3, shadow-tls-sni=site.test\n[Rule]\nFINAL,DIRECT\n",
    )
    .unwrap();
    let loaded = rurge_engine::load_checked(&path, &LoadOptions::for_tests()).unwrap();
    let found: Vec<(&str, u32, &str)> = loaded
        .diagnostics
        .iter()
        .map(|d| {
            let line = d.span.as_ref().map_or(0, |s| s.line);
            (d.code, line, d.message.as_str())
        })
        .collect();
    assert_eq!(
        found,
        [(
            rurge_config::codes::E_INVALID_POLICY_PARAM,
            2,
            "policy `P`: Shadow TLS cannot be combined with `h3=true` on a `trust-tunnel` policy"
        )]
    );
    assert!(loaded.config.spec("P").is_none());
}

```

门拆掉之后改变的用例：

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        // `h3=true` is inert: the policy still has a spec and uses HTTP/2
```

换成

```rust
        // `h3=true` takes effect: nothing is said of it
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        assert_eq!(o.inert, ["h3"]);
        assert!(matches!(
            o.spec.map(|s| s.proto),
            Some(ProtoSpec::TrustTunnel(_))
```

换成

```rust
        assert!(o.inert.is_empty(), "{:?}", o.inert);
        assert!(matches!(
            o.spec.map(|s| s.proto),
            Some(ProtoSpec::TrustTunnel(TrustTunnelSpec { h3: true, .. }))
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    /// `masque` lines are read and checked in full; they have no spec until
    /// the engine builds MASQUE (M7c task 8). `ecn` takes effect on them,
```

换成

```rust
    /// `masque` lines are read and checked in full, and a sound one has a
    /// spec, with the credentials or without. `ecn` takes effect on them,
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    fn a_masque_line_is_checked_but_has_no_spec_yet() {
        for def in [
            "masque, example.com, 443, username=user, password=pass",
            "masque, h.test, 443",
            "masque, h.test, 443, username=u, password=pw, port-hopping=1234;5000-6000, port-hopping-interval=10, ecn=off, alpn=h3, sni=edge.test, client-cert=cert1",
            "masque, h.test, 443, ecn=on, underlying-proxy=Pick",
            // DIRECT is no chain: the hops stay
            "masque, h.test, 443, port-hopping=5000-6000, underlying-proxy=DIRECT",
```

换成

```rust
    fn a_masque_line_is_checked_and_has_a_spec() {
        for (def, credentials, chained) in [
            (
                "masque, example.com, 443, username=user, password=pass",
                true,
                false,
            ),
            ("masque, h.test, 443", false, false),
            (
                "masque, h.test, 443, username=u, password=pw, port-hopping=1234;5000-6000, port-hopping-interval=10, ecn=off, alpn=h3, sni=edge.test, client-cert=cert1",
                true,
                false,
            ),
            (
                "masque, h.test, 443, ecn=on, underlying-proxy=Pick",
                false,
                true,
            ),
            // DIRECT is no chain: the hops stay
            (
                "masque, h.test, 443, port-hopping=5000-6000, underlying-proxy=DIRECT",
                false,
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
            let ProtoSpec::Masque(masque) = &spec.proto else {
                panic!("{def}: {:?}", spec.proto)
            };
            assert_eq!(masque.credentials.is_some(), credentials, "{def}");
            assert_eq!(spec.common.underlying_proxy.is_some(), chained, "{def}");
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    /// does not apply, and port hopping is no parameter of it. Until the
    /// engine builds it (M7c task 8) the load says `h3` has no effect yet,
    /// and the policy connects over HTTP/2.
```

换成

```rust
    /// does not apply, and port hopping is no parameter of it. `h3` itself
    /// takes effect: nothing is said of it.
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
                ),
            ]
        );
        assert_eq!(o.inert, ["h3"]);
```

换成

```rust
                ),
            ]
        );
        assert!(o.inert.is_empty(), "{:?}", o.inert);
```

`crates/rurge-config/src/spec/h2.rs`——把

```rust
    /// `h3=true` is read (the loader says once that it has no effect yet)
    /// and the policy connects over HTTP/2; `udp-relay` has no place here.
```

换成

```rust
    /// `h3=true` is read: the policy runs over HTTP/3 (phase 2 M7 design
    /// 7.2); `udp-relay` has no place here.
```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
            ),
        ]
    );
}

/// `h3=true` on `trust-tunnel` is said once per load and the policy keeps
/// HTTP/2 (phase 2 M6 design 5.1).
#[test]
fn trust_tunnel_h3_is_reported_once_per_load() {
    let loaded = load(
        "A = trust-tunnel, a.example, 443, username=u, password=p, h3=true
B = trust-tunnel, b.example, 443, username=u, password=p, h3=true",
        "",
    );
    assert!(!loaded.diagnostics.has_errors(), "{:?}", loaded.diagnostics);
    let inert: Vec<(String, u32)> = loaded
        .diagnostics
        .iter()
        .filter(|d| d.code == codes::W_PARAM_NOT_EFFECTIVE)
        .map(|d| (d.message.clone(), d.span.as_ref().unwrap().line))
        .collect();
    assert_eq!(
        inert,
        [(
            "policy parameter `h3` is parsed but has no effect in this version".to_string(),
            2
        )]
    );
    assert!(loaded.config.spec("A").is_some() && loaded.config.spec("B").is_some());
```

换成

```rust
            ),
        ]
    );
```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
/// other are errors; a hop interval under 5 seconds is raised to 5, and
/// `ecn` has an effect on them. They have no spec until the engine builds
/// them (M7c task 8).
```

换成

```rust
/// other are errors, which leave no spec; a hop interval under 5 seconds is
/// raised to 5, and `ecn` has an effect on them.
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
    let ProtoSpec::Masque(masque) = &d.proto else {
        panic!("{:?}", d.proto)
    };
    assert_eq!(
        masque.quic.port_hopping.as_ref().map(|h| h.interval),
        Some(Duration::from_secs(5))
    );
    assert_eq!(d.common.ecn, Tristate::Off);
```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
/// them. Until the engine builds them over HTTP/3 (M7c task 8), `h3` is
/// said once to have no effect yet.
```

换成

```rust
/// them; nothing is said of `h3` itself.
```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
                    .to_string(),
                2
            ),
            (
                codes::W_PARAM_NOT_EFFECTIVE,
                "policy parameter `h3` is parsed but has no effect in this version".to_string(),
```

换成

```rust
                    .to_string(),
```

`crates/rurge-config/tests/snapshots/corpus__corpus__kitchen-sink.snap`——把

```text
  - "warning[W0029] valid/kitchen-sink.conf:53: policy parameter `tfo` is parsed but has no effect in this version"
  - "warning[W0029] valid/kitchen-sink.conf:67: policy parameter `h3` is parsed but has no effect in this version"
```

换成

```text
  - "warning[W0029] valid/kitchen-sink.conf:53: policy parameter `tfo` is parsed but has no effect in this version"
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
    use rurge_proto::testing::{
        FakeH2Proxy, FakeSocks5, H2ProxyScript, Socks5Script, TlsFixture, echo_server,
    };
```

换成

```rust
    use rurge_proto::testing::{FakeSocks5, Socks5Script, echo_server};
    use rurge_proto_quic::testing::{FakeH3Proxy, H3ProxyScript};
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
TU5 = tuic-v5, proxy.test, 443, uuid=0233d11c-15a4-47d3-ade3-48ffca0ce119, password=pw, sni=tu.test\n\
```

换成

```rust
TU5 = tuic-v5, proxy.test, 443, uuid=0233d11c-15a4-47d3-ade3-48ffca0ce119, password=pw, sni=tu.test\n\
MQ = masque, proxy.test, 443, username=u, password=pw, port-hopping=5000-6000, ecn=off\n\
TT3 = trust-tunnel, proxy.test, 443, username=u, password=pw, h3=true, max-streams=2\n\
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            ("TU5", "TU5"),
```

换成

```rust
            ("TU5", "TU5"),
            ("MQ", "MQ"),
            ("TT3", "TT3"),
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
                "{m}"
            );
        }
    }

    #[test]
    fn a_dry_build_of_an_anytls_policy_leaves_no_task_behind() {
```

换成

```rust
                "{m}"
            );
        }
    }

    /// Sound `masque` lines and `trust-tunnel` lines with `h3=true` pass the
    /// dry build, which opens no connection (no tokio runtime here); a
    /// client certificate that cannot be read is a load error, and neither
    /// its password nor the policy's is quoted (phase 2 M7 design 7.1, 7.2).
    #[test]
    fn the_http3_policies_pass_the_dry_build() {
        let cfg = config(
            "[Proxy]\nM = masque, proxy.test, 443, username=u, password=pw, port-hopping=1234;5000-6000, port-hopping-interval=10, ecn=off, sni=mq.test\n\
N = masque, proxy.test, 443\n\
TT = trust-tunnel, proxy.test, 443, username=u, password=pw, h3=true, headers=X-Id:<random-string(8)>, max-streams=8, sni=tt.test\n\
[Rule]\nFINAL,DIRECT\n",
        );
        assert!(dry_build(&cfg).is_empty());

        let cfg = config(
            "[Proxy]\nM = masque, proxy.test, 443, username=u, password=s3same0pen, client-cert=cert1\n\
TT = trust-tunnel, proxy.test, 443, username=u, password=s3same0pen, h3=true, client-cert=cert1\n\
[Keystore]\ncert1 = type=p12, base64=QUJD, password=hunter2\n[Rule]\nFINAL,DIRECT\n",
        );
        let diags = dry_build(&cfg).sorted();
        let messages: Vec<String> = diags.iter().map(|d| d.message.clone()).collect();
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(messages[0].starts_with("policy `M` cannot be built: keystore item `cert1`"));
        assert!(messages[1].starts_with("policy `TT` cannot be built: keystore item `cert1`"));
        for m in &messages {
            assert!(!m.contains("hunter2") && !m.contains("s3same0pen"), "{m}");
        }
    }

    #[test]
    fn a_dry_build_of_an_anytls_policy_leaves_no_task_behind() {
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
TU = tuic, proxy.test, 443, token=pw, skip-cert-verify=true\n\
```

换成

```rust
TU = tuic, proxy.test, 443, token=pw, skip-cert-verify=true\n\
MQ = masque, proxy.test, 443, skip-cert-verify=true\n\
TT3 = trust-tunnel, proxy.test, 443, username=u, password=pw, h3=true, skip-cert-verify=true\n\
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
        assert_eq!(flagged, ["A", "V", "H2", "TT", "HY", "TU"]);
```

换成

```rust
        assert_eq!(flagged, ["A", "V", "H2", "TT", "HY", "TU", "MQ", "TT3"]);
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
    /// Until the engine builds `h3=true` over HTTP/3 (M7c task 8), such a
    /// `trust-tunnel` line connects over HTTP/2 as before: the `h3` its spec
    /// pins as the ALPN never reaches the TLS handshake over TCP.
    #[tokio::test]
    async fn a_trust_tunnel_line_with_h3_still_connects_over_http2() {
        let echo = echo_server().await;
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let fake = FakeH2Proxy::spawn(
            H2ProxyScript {
                users: vec![("u".into(), "p".into())],
                ..H2ProxyScript::default()
            },
            fixture.clone(),
        )
        .await;
        let cfg = config(&format!(
            "[Proxy]\nTT = trust-tunnel, 127.0.0.1, {}, username=u, password=p, h3=true\n[Rule]\nFINAL,DIRECT\n",
            fake.addr().port()
```

换成

```rust
    /// `masque`, and `trust-tunnel` with `h3=true`, are built over HTTP/3
    /// (phase 2 M7 design 7.1, 7.2): each tunnel is a CONNECT on its own
    /// policy's QUIC connection to the loopback `FakeH3Proxy`, the
    /// `trust-tunnel` one with its `user-agent`.
    #[tokio::test]
    async fn masque_and_trust_tunnel_with_h3_connect_over_http3() {
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            users: vec![("u".into(), "p".into())],
            ..H3ProxyScript::default()
        });
        let port = fake.addr().port();
        let cfg = config(&format!(
            "[Proxy]\nMQ = masque, 127.0.0.1, {port}, username=u, password=p\n\
TT = trust-tunnel, 127.0.0.1, {port}, username=u, password=p, h3=true\n[Rule]\nFINAL,DIRECT\n"
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            fixture.roots(),
        );
        let spec = cfg.spec("TT").unwrap();
        let out = f.build(spec, f.direct_connector(&spec.common)).unwrap();
        let target = Target::new(rurge_config::HostName::Ip(echo.ip()), echo.port());
        let mut stream = tokio::time::timeout(
            Duration::from_secs(5),
            out.connect_tcp(&target, &ConnectOpts::default()),
        )
        .await
        .expect("the tunnel opens in time")
        .unwrap();
        stream.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut buf))
            .await
            .expect("the echo arrives")
            .unwrap();
        assert_eq!(&buf, b"ping");
        assert_eq!(
            fixture.seen_at_least(1).await[0].alpn.as_deref(),
            Some("h2")
```

换成

```rust
            fake.roots(),
        );
        let target = Target::new(rurge_config::HostName::Ip(echo.ip()), echo.port());
        for name in ["MQ", "TT"] {
            let spec = cfg.spec(name).unwrap();
            let out = f.build(spec, f.direct_connector(&spec.common)).unwrap();
            let mut stream = tokio::time::timeout(
                Duration::from_secs(5),
                out.connect_tcp(&target, &ConnectOpts::default()),
            )
            .await
            .expect("the tunnel opens in time")
            .unwrap();
            // a write may leave the rest of its DATA frame for the next
            stream.write_all(b"ping").await.unwrap();
            stream.flush().await.unwrap();
            let mut buf = [0u8; 4];
            tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut buf))
                .await
                .expect("the echo arrives")
                .unwrap();
            assert_eq!(&buf, b"ping", "{name}");
        }
        let seen: Vec<(usize, Option<String>, Option<String>)> = fake
            .requests()
            .iter()
            .map(|r| {
                let agent = r.header("user-agent").map(str::to_string);
                (r.connection, r.authority.clone(), agent)
            })
            .collect();
        assert_eq!(
            seen,
            [
                (0, Some(echo.to_string()), None),
                (1, Some(echo.to_string()), Some("rurge".to_string())),
            ]
```

`crates/rurge/tests/cli.rs`——把

```rust
/// 5.1): neither is "not implemented"; `h3=true` is parsed but has no
/// effect, said once however many lines use it; a `trust-tunnel` without a
```

换成

```rust
/// 5.1): neither is "not implemented"; nothing is said of `h3=true`, which
/// takes effect (phase 2 M7 design 7.2); a `trust-tunnel` without a
```

`crates/rurge/tests/cli.rs`——把

```rust
    assert_eq!(out.matches("W0029").count(), 1, "{out}");
    assert!(
        out.contains("http2.conf:4")
            && out.contains("policy parameter `h3` is parsed but has no effect in this version"),
        "{out}"
    );
```

换成

```rust
    assert!(!out.contains("W0029"), "{out}");
```

`crates/rurge/tests/cli.rs`——把

```rust
            "{said}\n{out}"
        );
    }
    assert!(
        !out.contains("s3cretPw") && !out.contains("t0kenValue") && !out.contains("0233d11c"),
        "{out}"
    );
}
```

换成

```rust
            "{said}\n{out}"
        );
    }
    assert!(
        !out.contains("s3cretPw") && !out.contains("t0kenValue") && !out.contains("0233d11c"),
        "{out}"
    );
}

const MASQUE: &str = "[General]\n[Proxy]\n\
M = masque, proxy.test, 443, username=us3rName, password=s3cretPw, port-hopping=5000-6000, port-hopping-interval=2, ecn=off, udp-relay=true\n\
N = masque, proxy.test, 443, tfo=true\n\
Old = tailscale, section-name=ts\nE = socks5, proxy.test, 1080\n\
[Proxy Group]\nG = select, M, N, underlying-proxy=E\n[Rule]\nFINAL,DIRECT\n";
const MASQUE_BROKEN: &str = "[General]\n[Proxy]\n\
A = masque, proxy.test, 443, username=us3rName\n\
B = masque, proxy.test, 443, port-hopping=5000-6000, underlying-proxy=E\n\
C = masque, proxy.test, 443, username=us3rName, password=s3cretPw, shadow-tls-password=st, shadow-tls-version=3, shadow-tls-sni=site.test\n\
E = socks5, proxy.test, 1080\n[Rule]\nFINAL,DIRECT\n";

/// `rurge check` knows `masque` (phase 2 M7 design 7.1): it is not "not
/// implemented"; `ecn` has an effect on it, `tfo` and `udp-relay` do not
/// apply; a hop interval under 5 seconds is raised to 5, saying so; a group
/// that relays a member with `port-hopping` says it leaves that member out;
/// one credential without the other, `port-hopping` over an
/// `underlying-proxy` and Shadow TLS are errors; the credentials are never
/// printed.
#[test]
fn check_knows_masque() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "masque.conf", MASQUE))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // `tailscale` is still a later milestone; `masque` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("policy type `tailscale`") && !out.contains("policy type `masque`"),
        "{out}"
    );
    assert!(!out.contains("W0029"), "{out}");
    for (code, said) in [
        (
            "W0012",
            "masque.conf:3: policy `M`: `port-hopping-interval` is below the minimum of 5 seconds; using 5",
        ),
        (
            "W0028",
            "masque.conf:3: policy `M`: `udp-relay` does not apply to `masque` policies (UDP is always relayed); ignored",
        ),
        (
            "W0028",
            "masque.conf:4: policy `N`: `tfo` does not apply to `masque` policies; ignored",
        ),
        (
            "W0023",
            "masque.conf:8: policy group `G` relays its members through `underlying-proxy`: `M` uses `port-hopping`, which cannot go through it; `M (via E)` is left out",
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
        !out.contains("s3cretPw") && !out.contains("us3rName"),
        "{out}"
    );

    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "bad.conf", MASQUE_BROKEN))
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    for said in [
        "bad.conf:3: policy `A`: `password` is required with `username`",
        "bad.conf:4: policy `B`: `port-hopping` cannot be combined with `underlying-proxy`",
        "bad.conf:5: policy `C`: Shadow TLS cannot be combined with a `masque` policy",
    ] {
        assert!(
            out.lines()
                .any(|line| line.starts_with("error[E0018]") && line.ends_with(said)),
            "{said}\n{out}"
        );
    }
    assert!(
        !out.contains("s3cretPw") && !out.contains("us3rName"),
        "{out}"
    );
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-engine --test outbounds_masque`
Expected: FAIL——`masque` 行还没有 spec、工厂还不会构建，经它们的会话都被拒绝：

```text
test a_policy_test_goes_through_the_connection ... FAILED
test a_udp_test_asks_through_connect_udp ... FAILED
test tcp_leaves_through_masque_on_one_connection ... FAILED
test refused_credentials_fail_the_session ... FAILED
test tunnels_keep_working_while_the_port_hops ... FAILED
test a_reload_keeps_an_unchanged_policy_and_its_connection ... FAILED
test masque_goes_over_an_underlying_socks5_proxy ... FAILED
test a_server_without_connect_udp_fails_the_flow_and_carries_tcp ... FAILED
test a_packet_too_long_for_a_datagram_is_dropped_alone ... FAILED
test udp_goes_through_masque_as_connect_udp ... FAILED
test result: FAILED. 0 passed; 10 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.07s
error: test failed, to rerun pass `-p rurge-engine --test outbounds_masque`
```

- [ ] **Step 3: 实现**

`crates/rurge-config/src/spec/mod.rs`——把

```rust
                notes.inert.retain(|name| !matches!(*name, "ecn" | "tfo"));
                // the engine connects over HTTP/2 until M7c task 8
                notes.inert.push("h3");
```

换成

```rust
                notes.inert.retain(|name| !matches!(*name, "ecn" | "tfo"));
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    // the engine builds `masque` from M7c task 8 on: until then a valid line
    // is checked in full but has no spec
    let built = policy.kind != PolicyKind::Masque;
    let spec = (!failed && not_implemented.is_none() && built).then(|| PolicySpec {
```

换成

```rust
    let spec = (!failed && not_implemented.is_none()).then(|| PolicySpec {
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
use rurge_config::spec::{CommonOpts, IpVersion, PolicySpec, ProtoSpec, TlsOpts, TrustTunnelSpec};
```

换成

```rust
use rurge_config::spec::{CommonOpts, IpVersion, PolicySpec, ProtoSpec};
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
use rurge_proto_quic::{Hysteria2Outbound, TuicOutbound};
```

换成

```rust
use rurge_proto_quic::{Hysteria2Outbound, MasqueOutbound, TrustTunnelH3Outbound, TuicOutbound};
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            ProtoSpec::TrustTunnel(tt) => {
                // over HTTP/2 until M7c task 8 builds `h3=true` over HTTP/3:
                // such a line's spec pins `alpn` to `h3`, which an HTTP/2
                // connection must not offer
                let h2 = TrustTunnelSpec {
                    tls: TlsOpts {
                        alpn: vec!["h2".to_string()],
                        ..tt.tls.clone()
                    },
                    ..tt.clone()
                };
                Arc::new(TrustTunnelOutbound::new(
                    &spec.name,
                    server_of(spec)?,
                    &h2,
                    spec.shadow_tls.as_ref(),
                    &self.keystore,
                    self.roots.clone(),
                    connector,
                )?)
            }
```

换成

```rust
            // `h3=true`: over HTTP/3 (QUIC) instead of HTTP/2 over TLS; the
            // loader refuses Shadow TLS on such a line
            ProtoSpec::TrustTunnel(tt) if tt.h3 => Arc::new(TrustTunnelH3Outbound::new(
                &spec.name,
                server_of(spec)?,
                tt,
                spec.common.ecn,
                &self.keystore,
                self.roots.clone(),
                connector,
            )?),
            ProtoSpec::TrustTunnel(tt) => Arc::new(TrustTunnelOutbound::new(
                &spec.name,
                server_of(spec)?,
                tt,
                spec.shadow_tls.as_ref(),
                &self.keystore,
                self.roots.clone(),
                connector,
            )?),
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            // the loader makes no spec of these lines before M7c task 8
            ProtoSpec::Masque(_) => {
                return Err(BuildError::new(format!(
                    "`{}` is not implemented yet",
                    spec.kind.keyword()
                )));
            }
```

换成

```rust
            ProtoSpec::Masque(masque) => Arc::new(MasqueOutbound::new(
                &spec.name,
                server_of(spec)?,
                masque,
                spec.common.ecn,
                &self.keystore,
                self.roots.clone(),
                connector,
            )?),
```

`crates/rurge/src/capabilities.rs`——把

```rust
//! `trust-tunnel` over HTTP/2 only (phase 2 M6c), `hysteria2` (phase 2
//! M7a), `tuic` and `tuic-v5` (phase 2 M7b), `select` groups, `url-test` /
//! `fallback` / `load-balance` groups (phase 2 M3b), and `smart` groups
//! (phase 2 M3c).
```

换成

```rust
//! `trust-tunnel` over HTTP/2 (phase 2 M6c), `hysteria2` (phase 2 M7a),
//! `tuic` and `tuic-v5` (phase 2 M7b), `masque` and `trust-tunnel` over
//! HTTP/3 (phase 2 M7c), `select` groups, `url-test` / `fallback` /
//! `load-balance` groups (phase 2 M3b), and `smart` groups (phase 2 M3c).
```

`crates/rurge/src/capabilities.rs`——把

```rust
            PolicyKind::TuicV5,
```

换成

```rust
            PolicyKind::TuicV5,
            PolicyKind::Masque,
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-engine --test outbounds_masque` → 通过（10 条：TCP 共用一条连接、凭据被拒时会话失败、UDP 经 CONNECT-UDP、服务端不支持 CONNECT-UDP 时这条流失败而 TCP 照常、放不下的包单独丢弃、端口跳跃中隧道不断、经 `underlying-proxy`（socks5）、经该策略的 URL 测试、经 CONNECT-UDP 的 UDP 测试、重载时沿用未变的策略与它的连接）。
Run: `cargo test -p rurge-engine --test outbounds_http2` → 通过（新增 5 条：经 HTTP/3 的 `trust-tunnel`、登录被拒、`max-streams` 限住每条 QUIC 连接、重载沿用、`h3=true` 与 Shadow TLS 在加载时被拒）。
Run: `cargo test -p rurge --test cli check_knows_masque` → 通过；`cargo test -p rurge-config` → 通过。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates
git commit -m "feat(engine): 装配 masque 与 trust-tunnel 的 HTTP/3、能力表翻转"
```

### Task 9: 互操作、CI 与文档

masque-server v0.13.0、TrustTunnel endpoint v1.1.0 的 HTTP/3 监听、AdGuard dnsproxy v0.86.0 与 CoreDNS v1.14.7 的夹具与互操作用例（P20）；CI 按 SHA-256 安装新的参照服务端；兼容性清单、手工验收、两份 README、`CLAUDE.md`、总设计与互操作说明。

**Files:**
- Create: `tests/interop/src/{masque_server.rs, dnsproxy.rs, coredns.rs}`（自带用例）、`tests/interop/tests/{masque.rs, dns.rs}`
- Modify: `tests/interop/{Cargo.toml, README.md, src/lib.rs, src/trusttunnel.rs, tests/http2.rs}`、`.github/workflows/ci.yml`、`docs/surge-compatibility-matrix.md`、`docs/acceptance/phase2-manual.md`、`README.md`、`README_en.md`、`CLAUDE.md`、`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`、`Cargo.lock`

**Interfaces:**
- Consumes: 互操作夹具既有的 `Reference`、`free_port` / `free_udp_port`、`roundtrip` / `roundtrip_big` / `transfer`、测试 TLS 夹具（PKCS#8 私钥）、`trusttunnel` 夹具；Task 6 的 `Resolver` 对 `quic://` / `h3://` 的支持。
- Produces: `rurge_interop::{masque_server, dnsproxy, coredns}`（`RURGE_TEST_MASQUE_SERVER` / `RURGE_TEST_DNSPROXY` / `RURGE_TEST_COREDNS`；配置只监听 `127.0.0.1`，各有单元用例钉住）；`trusttunnel::Listen::{Http2, Http3}`（`render` / `spawn` 多一个参数）；`lib.rs` 的 `toml_string` / `toml_path` 与 DNS 记录常量

- [ ] **Step 1: 互操作夹具与用例**

`tests/interop/Cargo.toml`——把

```toml
rurge-config.workspace = true
```

换成

```toml
rurge-config.workspace = true
rurge-dns.workspace = true
```

`tests/interop/Cargo.toml`——把

```toml
rurge-proto-wireguard = { workspace = true, features = ["testing"] }
```

换成

```toml
rurge-proto-wireguard = { workspace = true, features = ["testing"] }
rurge-rules.workspace = true
```

`tests/interop/src/lib.rs`——把

```rust
//! WireGuard endpoint runs in user space).

pub mod shadowsocks_rust;
```

换成

```rust
//! WireGuard endpoint runs in user space).

pub mod coredns;
pub mod dnsproxy;
pub mod masque_server;
pub mod shadowsocks_rust;
```

`tests/interop/src/lib.rs`——把

```rust
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
```

换成

```rust
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
```

`tests/interop/src/lib.rs`——把

```rust
        .and_then(|s| s.local_addr())
        .expect("a free loopback port")
        .port()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
```

换成

```rust
        .and_then(|s| s.local_addr())
        .expect("a free loopback port")
        .port()
}

/// A port that was free a moment ago for TCP and for UDP alike: for a server
/// that binds both on one number.
pub fn free_tcp_and_udp_port() -> u16 {
    for _ in 0..100 {
        let port = free_port();
        if UdpSocket::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    panic!("no loopback port was free for TCP and UDP alike")
}

/// The one name the DNS references answer — dnsproxy from its hosts file,
/// CoreDNS from its templates — with one address of each family, both of
/// documentation ranges: nothing ever connects to them.
pub const DNS_NAME: &str = "interop.rurge.test";
pub const DNS_V4: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 10);
pub const DNS_V6: Ipv6Addr = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0x10);

/// A TOML basic string.
pub(crate) fn toml_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A path as a TOML basic string.
pub(crate) fn toml_path(p: &Path) -> String {
    toml_string(&p.to_string_lossy())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
```

`tests/interop/src/lib.rs`——把

```rust
        TcpListener::bind(("127.0.0.1", port)).expect("the port is free again");
    }
}
```

换成

```rust
        TcpListener::bind(("127.0.0.1", port)).expect("the port is free again");
    }

    #[test]
    fn a_port_free_for_tcp_and_udp_takes_both() {
        let port = free_tcp_and_udp_port();
        let _tcp = TcpListener::bind(("127.0.0.1", port)).expect("the TCP port is free again");
        UdpSocket::bind(("127.0.0.1", port)).expect("the UDP port is free too");
    }

    #[test]
    fn windows_paths_are_escaped() {
        assert_eq!(toml_path(Path::new(r"C:\t\x.pem")), r#""C:\\t\\x.pem""#);
        assert_eq!(toml_string("p\"w"), r#""p\"w""#);
    }
}
```

`tests/interop/src/trusttunnel.rs`——把

```rust
//! is set, which the loopback echo needs.

use crate::{REQUIRED_ENV, Reference, TlsFiles, free_port};
```

换成

```rust
//! is set, which the loopback echo needs.
//!
//! Its QUIC listener (`Listen::Http3`) serves the same CONNECTs over HTTP/3:
//! the reference for `trust-tunnel` with `h3=true`.

use crate::{
    REQUIRED_ENV, Reference, TlsFiles, free_port, free_tcp_and_udp_port, toml_path, toml_string,
};
```

`tests/interop/src/trusttunnel.rs`——把

```rust
pub const HOSTNAME: &str = "tt.test";
```

换成

```rust
pub const HOSTNAME: &str = "tt.test";

/// What the endpoint logs (at `info`) once its QUIC listener is bound.
const LISTENING_UDP: &str = "Listening to UDP";
```

`tests/interop/src/trusttunnel.rs`——把

```rust
    None
}

/// The endpoint's three files.
```

换成

```rust
    None
}

/// What the endpoint serves on its port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Listen {
    /// HTTP/2 over TLS, on the TCP port (`[listen_protocols.http2]`).
    Http2,
    /// HTTP/3 alone, on the UDP port (`[listen_protocols.quic]`). The
    /// endpoint binds the TCP port of the same number too, and serves
    /// nothing there.
    Http3,
}

/// The endpoint's three files.
```

`tests/interop/src/trusttunnel.rs`——把

```rust
/// A TOML basic string.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn path(p: &Path) -> String {
    quoted(&p.to_string_lossy())
}

/// The whole configuration: HTTP/2 only on `127.0.0.1:port`, IPv4 only,
/// direct forwarding that may reach the loopback, one client. The endpoint
/// resolves file paths against its working directory, so they must be
/// absolute.
pub fn render(
    port: u16,
```

换成

```rust
/// The whole configuration: `listen`'s one protocol on `127.0.0.1:port`,
/// IPv4 only, direct forwarding that may reach the loopback, one client. The
/// endpoint resolves file paths against its working directory, so they must
/// be absolute.
pub fn render(
    port: u16,
    listen: Listen,
```

`tests/interop/src/trusttunnel.rs`——把

```rust
) -> Rendered {
```

换成

```rust
) -> Rendered {
    let protocol = match listen {
        Listen::Http2 => "http2",
        Listen::Http3 => "quic",
    };
```

`tests/interop/src/trusttunnel.rs`——把

```rust
         \n\
         [listen_protocols.http2]\n\
```

换成

```rust
         \n\
         [listen_protocols.{protocol}]\n\
```

`tests/interop/src/trusttunnel.rs`——把

```rust
        path(credentials)
```

换成

```rust
        toml_path(credentials)
```

`tests/interop/src/trusttunnel.rs`——把

```rust
        quoted(HOSTNAME),
        path(&tls.certificate),
        path(&tls.key)
```

换成

```rust
        toml_string(HOSTNAME),
        toml_path(&tls.certificate),
        toml_path(&tls.key)
```

`tests/interop/src/trusttunnel.rs`——把

```rust
        quoted(username),
        quoted(password)
```

换成

```rust
        toml_string(username),
        toml_string(password)
```

`tests/interop/src/trusttunnel.rs`——把

```rust
    /// `binary` there and waits until it accepts connections. `tls` is a
    /// certificate for [`HOSTNAME`].
```

换成

```rust
    /// `binary` there and waits until it serves `listen`: its TCP port
    /// accepts connections — and, over HTTP/3, its log says the QUIC
    /// listener is bound. `tls` is a certificate for [`HOSTNAME`].
```

`tests/interop/src/trusttunnel.rs`——把

```rust
        dir: &Path,
```

换成

```rust
        dir: &Path,
        listen: Listen,
```

`tests/interop/src/trusttunnel.rs`——把

```rust
        let port = free_port();
```

换成

```rust
        // over HTTP/3 the endpoint binds both on one number
        let port = match listen {
            Listen::Http2 => free_port(),
            Listen::Http3 => free_tcp_and_udp_port(),
        };
```

`tests/interop/src/trusttunnel.rs`——把

```rust
        let rendered = render(port, tls, &credentials, username, password);
```

换成

```rust
        let rendered = render(port, listen, tls, &credentials, username, password);
```

`tests/interop/src/trusttunnel.rs`——把

```rust
        TrustTunnel(Reference::start(
```

换成

```rust
        let mut endpoint = Reference::start(
```

`tests/interop/src/trusttunnel.rs`——把

```rust
        ))
    }

    /// The loopback port.
```

换成

```rust
        );
        if listen == Listen::Http3 {
            endpoint.wait_for_output(LISTENING_UDP);
        }
        TrustTunnel(endpoint)
    }

    /// The loopback port: TCP over HTTP/2, UDP over HTTP/3.
```

`tests/interop/src/trusttunnel.rs`——把

```rust
    #[test]
    fn the_configuration_stays_on_the_loopback() {
        let rendered = render(
            4001,
            &files(),
            Path::new("/tmp/tt/credentials.toml"),
            "u",
            "pw",
        );
        let text = format!("{}{}{}", rendered.vpn, rendered.hosts, rendered.credentials);
        for forbidden in ["0.0.0.0", "::", "socks5", "listen_protocols.quic", "http1"] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
        }
        let listen: Vec<&str> = rendered
            .vpn
            .lines()
            .filter(|l| l.starts_with("listen_address"))
            .collect();
        assert_eq!(listen, ["listen_address = \"127.0.0.1:4001\""]);
        assert!(rendered.vpn.contains("[forward_protocol]\ndirect = {}\n"));
```

换成

```rust
    /// One protocol, on the loopback: HTTP/2 alone, or QUIC alone.
    #[test]
    fn the_configuration_stays_on_the_loopback() {
        for (listen, protocol) in [
            (Listen::Http2, "[listen_protocols.http2]"),
            (Listen::Http3, "[listen_protocols.quic]"),
        ] {
            let rendered = render(
                4001,
                listen,
                &files(),
                Path::new("/tmp/tt/credentials.toml"),
                "u",
                "pw",
            );
            let text = format!("{}{}{}", rendered.vpn, rendered.hosts, rendered.credentials);
            for forbidden in ["0.0.0.0", "::", "socks5", "http1"] {
                assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
            }
            let addresses: Vec<&str> = rendered
                .vpn
                .lines()
                .filter(|l| l.starts_with("listen_address"))
                .collect();
            assert_eq!(addresses, ["listen_address = \"127.0.0.1:4001\""]);
            let protocols: Vec<&str> = rendered
                .vpn
                .lines()
                .filter(|l| l.starts_with("[listen_protocols."))
                .collect();
            assert_eq!(protocols, [protocol]);
            assert!(rendered.vpn.contains("[forward_protocol]\ndirect = {}\n"));
        }
```

`tests/interop/src/trusttunnel.rs`——把

```rust
    fn the_configuration_is_written_as_the_endpoint_reads_it() {
        let rendered = render(
            4001,
            &files(),
            Path::new("/tmp/tt/credentials.toml"),
```

换成

```rust
    fn the_configuration_is_written_as_the_endpoint_reads_it() {
        let rendered = render(
            4001,
            Listen::Http2,
            &files(),
            Path::new("/tmp/tt/credentials.toml"),
```

`tests/interop/src/trusttunnel.rs`——把

```rust
                credentials: "[[client]]\nusername = \"u\"\npassword = \"p\\\"w\"\n".into(),
            }
        );
    }

    #[test]
    fn windows_paths_are_escaped() {
        assert_eq!(path(Path::new(r"C:\t\x.pem")), r#""C:\\t\\x.pem""#);
```

换成

```rust
                credentials: "[[client]]\nusername = \"u\"\npassword = \"p\\\"w\"\n".into(),
            }
        );
        // the QUIC listener with its defaults: an empty table turns it on
        let http3 = render(
            4001,
            Listen::Http3,
            &files(),
            Path::new("/tmp/tt/credentials.toml"),
            "u",
            "p\"w",
        );
        assert_eq!(
            http3.vpn,
            "listen_address = \"127.0.0.1:4001\"\n\
             ipv6_available = false\n\
             allow_private_network_connections = true\n\
             credentials_file = \"/tmp/tt/credentials.toml\"\n\
             \n\
             [listen_protocols.quic]\n\
             \n\
             [forward_protocol]\n\
             direct = {}\n"
        );
        assert_eq!(
            (http3.hosts, http3.credentials),
            (rendered.hosts, rendered.credentials)
        );
```

新建 `tests/interop/src/masque_server.rs`：

```rust
//! masque-server v0.13.0 (Vincent-bin/masque-server, MIT) as a child process
//! on the loopback: the reference for `masque` (phase 2 M7 design 7.4,
//! M7-D6). It is built for Linux only: elsewhere a missing binary is always
//! a skip, even with `RURGE_INTEROP_REQUIRED=1`. The same rules as for
//! sing-box apply: nothing is downloaded or installed here, and the rendered
//! configuration listens on 127.0.0.1 only — one HTTP/3 listener, no
//! observability endpoint, and no CONNECT-IP, which would open a TUN device.
//! It has no TCP port to probe: it is up once its log says so.
//!
//! The server lets nobody in without Basic credentials, checked against
//! Argon2id hashes: its one user is the one its own end-to-end tests use.
//! Its target policy refuses loopback and private addresses unless told
//! otherwise, which the loopback echoes need.

use crate::{REQUIRED_ENV, Reference, TlsFiles, free_udp_port, toml_path, toml_string};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BINARY_ENV: &str = "RURGE_TEST_MASQUE_SERVER";

/// The user the server lets in, and its password — those of masque-server's
/// own end-to-end tests (`tests/e2e/masque-e2e.toml`, `compose.yml`).
pub const USERNAME: &str = "test";
pub const PASSWORD: &str = "test-password";

/// The Argon2id hash of [`PASSWORD`] the configuration holds, from the same
/// file.
const PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$1xNVXhqKU7jJ6cqTBJKphQ$GXXAINVTW1qhloFtN1IR8lSr7pI7QEY79fq4K6d8scQ";

/// What the server logs (at `info`) once every listener is bound.
const READY: &str = "server ready";

/// `RURGE_TEST_MASQUE_SERVER`, else the first `masque-server` on `PATH`.
pub fn locate() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(BINARY_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join("masque-server"))
        .find(|candidate| candidate.is_file())
}

/// The binary — or `None` after saying why `test` is skipped. With
/// `RURGE_INTEROP_REQUIRED=1` (CI) a missing binary is a failure on Linux,
/// the only platform masque-server is built for.
pub fn masque_server_or_skip(test: &str) -> Option<PathBuf> {
    if let Some(path) = locate() {
        return Some(path);
    }
    if cfg!(target_os = "linux") && std::env::var(REQUIRED_ENV).as_deref() == Ok("1") {
        panic!("{REQUIRED_ENV}=1 but no masque-server was found ({BINARY_ENV} or PATH)");
    }
    eprintln!(
        "skipping {test}: no masque-server ({BINARY_ENV} or PATH; Linux only); see tests/interop/README.md"
    );
    None
}

/// The whole configuration: one HTTP/3 listener of one shard on
/// `127.0.0.1:port` with the certificate and key of `tls` (PEM paths;
/// BoringSSL reads a PKCS#8 key) and Basic authentication of [`USERNAME`];
/// TCP and UDP to any target, the loopback included; CONNECT-IP off. The
/// server refuses a key it does not know, so nothing else is written; its
/// other defaults stay (a 60 s idle timeout, the RFC 9298 default template
/// for CONNECT-UDP, QUIC datagrams on).
pub fn render(port: u16, tls: &TlsFiles) -> String {
    assert!(
        tls.client_ca.is_none(),
        "masque-server's Basic listener asks for no client certificate"
    );
    format!(
        "[tls]\n\
         cert_path = {}\n\
         key_path = {}\n\
         \n\
         [[listeners]]\n\
         listen_addr = \"127.0.0.1:{port}\"\n\
         transport = \"http3\"\n\
         shards = 1\n\
         \n\
         [listeners.auth]\n\
         enabled = true\n\
         mode = \"basic\"\n\
         \n\
         [[listeners.auth.users]]\n\
         username = {}\n\
         password_hash = {}\n\
         \n\
         [tcp_proxy]\n\
         allow_targets = [\"0.0.0.0/0\", \"::/0\"]\n\
         deny_targets = []\n\
         \n\
         [udp_proxy]\n\
         allow_targets = [\"0.0.0.0/0\", \"::/0\"]\n\
         deny_targets = []\n\
         \n\
         [ip_proxy]\n\
         enabled = false\n",
        toml_path(&tls.certificate),
        toml_path(&tls.key),
        toml_string(USERNAME),
        toml_string(PASSWORD_HASH)
    )
}

/// A running masque-server; killed and reaped on drop.
pub struct MasqueServer {
    server: Reference,
    port: u16,
}

impl MasqueServer {
    /// Writes the configuration into `dir`, starts `binary` there and waits
    /// until it says it is ready.
    pub fn spawn(binary: &Path, dir: &Path, tls: &TlsFiles) -> MasqueServer {
        let port = free_udp_port();
        let config = dir.join("masque.toml");
        std::fs::write(&config, render(port, tls)).expect("write the config");
        let mut command = Command::new(binary);
        command.arg("--config").arg(&config).current_dir(dir);
        // its own filter logs `server ready`; another in the environment
        // might not
        command.env_remove("RUST_LOG");
        let mut server = Reference::start(
            "masque-server",
            command,
            Vec::new(),
            dir.join("masque-server.log"),
        );
        server.wait_for_output(READY);
        MasqueServer { server, port }
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
            certificate: "/tmp/masque/leaf.pem".into(),
            key: "/tmp/masque/leaf.key".into(),
            client_ca: None,
        }
    }

    /// One listener, on the loopback; no endpoint or device beyond it.
    #[test]
    fn the_configuration_stays_on_the_loopback() {
        let text = render(4001, &files());
        let addresses: Vec<&str> = text.lines().filter(|l| l.contains("listen_addr")).collect();
        assert_eq!(addresses, ["listen_addr = \"127.0.0.1:4001\""]);
        assert_eq!(text.matches("[[listeners]]").count(), 1);
        for forbidden in ["observability", "http2", "tun_", "clients"] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
        }
        assert!(text.ends_with("[ip_proxy]\nenabled = false\n"), "{text}");
    }

    /// Every key is one v0.13.0 reads: it refuses any other.
    #[test]
    fn the_configuration_is_written_as_masque_server_reads_it() {
        assert_eq!(
            render(4001, &files()),
            "[tls]\n\
             cert_path = \"/tmp/masque/leaf.pem\"\n\
             key_path = \"/tmp/masque/leaf.key\"\n\
             \n\
             [[listeners]]\n\
             listen_addr = \"127.0.0.1:4001\"\n\
             transport = \"http3\"\n\
             shards = 1\n\
             \n\
             [listeners.auth]\n\
             enabled = true\n\
             mode = \"basic\"\n\
             \n\
             [[listeners.auth.users]]\n\
             username = \"test\"\n\
             password_hash = \"$argon2id$v=19$m=19456,t=2,p=1$1xNVXhqKU7jJ6cqTBJKphQ$GXXAINVTW1qhloFtN1IR8lSr7pI7QEY79fq4K6d8scQ\"\n\
             \n\
             [tcp_proxy]\n\
             allow_targets = [\"0.0.0.0/0\", \"::/0\"]\n\
             deny_targets = []\n\
             \n\
             [udp_proxy]\n\
             allow_targets = [\"0.0.0.0/0\", \"::/0\"]\n\
             deny_targets = []\n\
             \n\
             [ip_proxy]\n\
             enabled = false\n"
        );
    }
}
```

新建 `tests/interop/src/dnsproxy.rs`：

```rust
//! AdGuard dnsproxy v0.86.0 (Apache-2.0) as a child process on the loopback:
//! a reference for the `quic://` (DoQ) and `h3://` (DoH3) DNS upstreams
//! (phase 2 M7 design 7.4), built for every platform CI runs on. The same
//! rules as for sing-box apply: nothing is downloaded or installed here, and
//! everything it listens on is on 127.0.0.1 — DoQ, DoH3, and the DNS over
//! HTTPS (TCP) and plain DNS (UDP and TCP) it opens alongside, which the
//! tests leave alone. It answers [`DNS_NAME`] from a hosts file of its own;
//! its one upstream is the loopback's discard port, where anything else it
//! is asked goes nowhere. It has no port to probe that says QUIC is up: it
//! is up once its log says so.

use crate::{
    DNS_NAME, DNS_V4, DNS_V6, REQUIRED_ENV, Reference, TlsFiles, free_tcp_and_udp_port,
    free_udp_port,
};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BINARY_ENV: &str = "RURGE_TEST_DNSPROXY";

/// What it logs (at `info`) once its DoQ listener is bound, and once its
/// DoH3 one is.
const LISTENING_QUIC: &str = "listening quic";
const LISTENING_H3: &str = "listening to h3";

/// `RURGE_TEST_DNSPROXY`, else the first `dnsproxy` on `PATH`.
pub fn locate() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(BINARY_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let name = if cfg!(windows) {
        "dnsproxy.exe"
    } else {
        "dnsproxy"
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The binary — or `None` after saying why `test` is skipped. With
/// `RURGE_INTEROP_REQUIRED=1` (CI) a missing binary is a failure instead.
pub fn dnsproxy_or_skip(test: &str) -> Option<PathBuf> {
    if let Some(path) = locate() {
        return Some(path);
    }
    if std::env::var(REQUIRED_ENV).as_deref() == Ok("1") {
        panic!("{REQUIRED_ENV}=1 but no dnsproxy was found ({BINARY_ENV} or PATH)");
    }
    eprintln!("skipping {test}: no dnsproxy ({BINARY_ENV} or PATH); see tests/interop/README.md");
    None
}

/// The ports dnsproxy listens on, all on 127.0.0.1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ports {
    /// Plain DNS, on UDP and TCP: it takes 53 unless told otherwise.
    pub plain: u16,
    /// DNS over QUIC, on UDP.
    pub quic: u16,
    /// DNS over HTTPS on TCP, and DNS over HTTP/3 on the UDP port of the
    /// same number.
    pub https: u16,
}

/// The hosts file: [`DNS_NAME`]'s two addresses, its name in lower case as
/// the resolver asks for it.
pub fn hosts() -> String {
    format!("{DNS_V4} {DNS_NAME}\n{DNS_V6} {DNS_NAME}\n")
}

/// The command line: `ports` on 127.0.0.1, HTTP/3 on the HTTPS port, the
/// certificate and key of `tls` (PEM paths), the hosts file at `hosts` and
/// the discard port as the one upstream it must have.
pub fn args(ports: Ports, tls: &TlsFiles, hosts: &Path) -> Vec<OsString> {
    assert!(
        tls.client_ca.is_none(),
        "dnsproxy asks for no client certificate"
    );
    let mut args: Vec<OsString> = Vec::new();
    for (flag, value) in [
        ("--listen", "127.0.0.1".to_string()),
        ("--port", ports.plain.to_string()),
        ("--quic-port", ports.quic.to_string()),
        ("--https-port", ports.https.to_string()),
        ("--upstream", "127.0.0.1:9".to_string()),
    ] {
        args.extend([flag.into(), value.into()]);
    }
    args.push("--http3".into());
    for (flag, path) in [
        ("--tls-crt", tls.certificate.as_path()),
        ("--tls-key", tls.key.as_path()),
        ("--hosts-files", hosts),
    ] {
        args.extend([flag.into(), path.into()]);
    }
    args
}

/// A running dnsproxy; killed and reaped on drop.
pub struct Dnsproxy {
    server: Reference,
    ports: Ports,
}

impl Dnsproxy {
    /// Writes the hosts file into `dir`, starts `binary` there and waits
    /// until it says DoQ and DoH3 listen.
    pub fn spawn(binary: &Path, dir: &Path, tls: &TlsFiles) -> Dnsproxy {
        // the HTTPS port is TCP's and HTTP/3's (UDP) alike, and so is the
        // plain one
        let ports = Ports {
            plain: free_tcp_and_udp_port(),
            quic: free_udp_port(),
            https: free_tcp_and_udp_port(),
        };
        let hosts_file = dir.join("hosts.txt");
        std::fs::write(&hosts_file, hosts()).expect("write the hosts file");
        let mut command = Command::new(binary);
        command.args(args(ports, tls, &hosts_file)).current_dir(dir);
        let mut server =
            Reference::start("dnsproxy", command, Vec::new(), dir.join("dnsproxy.log"));
        server.wait_for_output(LISTENING_QUIC);
        server.wait_for_output(LISTENING_H3);
        Dnsproxy { server, ports }
    }

    /// The loopback UDP port of DNS over QUIC.
    pub fn quic_port(&self) -> u16 {
        self.ports.quic
    }

    /// The loopback UDP port of DNS over HTTP/3.
    pub fn h3_port(&self) -> u16 {
        self.ports.https
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
            certificate: "/tmp/dns/leaf.pem".into(),
            key: "/tmp/dns/leaf.key".into(),
            client_ca: None,
        }
    }

    fn ports() -> Ports {
        Ports {
            plain: 4001,
            quic: 4002,
            https: 4003,
        }
    }

    /// Every listener on 127.0.0.1, nothing upstream but the discard port,
    /// no debug endpoint.
    #[test]
    fn the_command_line_stays_on_the_loopback() {
        let args = args(ports(), &files(), Path::new("/tmp/dns/hosts.txt"));
        let text: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let value = |flag: &str| -> Vec<&str> {
            text.windows(2)
                .filter(|pair| pair[0] == flag)
                .map(|pair| pair[1].as_str())
                .collect()
        };
        assert_eq!(value("--listen"), ["127.0.0.1"]);
        assert_eq!(value("--upstream"), ["127.0.0.1:9"]);
        for forbidden in [
            "0.0.0.0",
            "--pprof",
            "--config-path",
            "--bootstrap",
            "--fallback",
        ] {
            assert!(
                !text.iter().any(|a| a.contains(forbidden)),
                "`{forbidden}` in {text:?}"
            );
        }
    }

    #[test]
    fn the_command_line_is_written_as_dnsproxy_reads_it() {
        let expected: Vec<OsString> = [
            "--listen",
            "127.0.0.1",
            "--port",
            "4001",
            "--quic-port",
            "4002",
            "--https-port",
            "4003",
            "--upstream",
            "127.0.0.1:9",
            "--http3",
            "--tls-crt",
            "/tmp/dns/leaf.pem",
            "--tls-key",
            "/tmp/dns/leaf.key",
            "--hosts-files",
            "/tmp/dns/hosts.txt",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert_eq!(
            args(ports(), &files(), Path::new("/tmp/dns/hosts.txt")),
            expected
        );
        assert_eq!(
            hosts(),
            "192.0.2.10 interop.rurge.test\n2001:db8::10 interop.rurge.test\n"
        );
    }
}
```

新建 `tests/interop/src/coredns.rs`：

```rust
//! CoreDNS v1.14.7 (Apache-2.0) as a child process on the loopback: the
//! strict reference for the `quic://` (DoQ) and `h3://` (DoH3) DNS upstreams
//! (phase 2 M7 design 7.4), built for every platform CI runs on. Its DoQ
//! server closes the connection over a message ID other than 0 or an
//! edns-tcp-keepalive option (RFC 9250 4.3.3), and its DoH3 server answers
//! on `/dns-query` alone. The same rules as for sing-box apply: nothing is
//! downloaded or installed here, and the rendered Corefile listens on
//! 127.0.0.1 only. It answers [`DNS_NAME`] from templates and nothing else —
//! CoreDNS runs only the plugins a Corefile names, and this one names no
//! `forward`. It has no TCP port to probe: it is up once it has listed its
//! servers.

use crate::{DNS_NAME, DNS_V4, DNS_V6, REQUIRED_ENV, Reference, TlsFiles, free_udp_port};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BINARY_ENV: &str = "RURGE_TEST_COREDNS";

/// `RURGE_TEST_COREDNS`, else the first `coredns` on `PATH`.
pub fn locate() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(BINARY_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let name = if cfg!(windows) {
        "coredns.exe"
    } else {
        "coredns"
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The binary — or `None` after saying why `test` is skipped. With
/// `RURGE_INTEROP_REQUIRED=1` (CI) a missing binary is a failure instead.
pub fn coredns_or_skip(test: &str) -> Option<PathBuf> {
    if let Some(path) = locate() {
        return Some(path);
    }
    if std::env::var(REQUIRED_ENV).as_deref() == Ok("1") {
        panic!("{REQUIRED_ENV}=1 but no coredns was found ({BINARY_ENV} or PATH)");
    }
    eprintln!("skipping {test}: no coredns ({BINARY_ENV} or PATH); see tests/interop/README.md");
    None
}

/// A Corefile token in quotes, which a path with a space needs: a quote is
/// the one character escaped there, a backslash stays as it is.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\\\""))
}

/// What CoreDNS prints once the server of `scheme` on `port` is up.
fn listening(scheme: &str, port: u16) -> String {
    format!("{scheme}://.:{port} on 127.0.0.1")
}

/// The whole Corefile: a DoQ server on UDP port `quic` and a DoH3 server on
/// UDP port `h3`, both bound to 127.0.0.1 with the certificate and key of
/// `tls` (PEM paths), answering [`DNS_NAME`]'s A and AAAA records from
/// templates.
pub fn render(quic: u16, h3: u16, tls: &TlsFiles) -> String {
    assert!(
        tls.client_ca.is_none(),
        "CoreDNS asks for no client certificate here"
    );
    let (certificate, key) = (
        quoted(&tls.certificate.to_string_lossy()),
        quoted(&tls.key.to_string_lossy()),
    );
    let mut text = String::new();
    for (scheme, port) in [("quic", quic), ("https3", h3)] {
        text +=
            &format!("{scheme}://.:{port} {{\n    bind 127.0.0.1\n    tls {certificate} {key}\n");
        for (qtype, address) in [("A", IpAddr::V4(DNS_V4)), ("AAAA", IpAddr::V6(DNS_V6))] {
            text += &format!(
                "    template IN {qtype} {DNS_NAME} {{\n        answer \"{{{{ .Name }}}} 60 IN {qtype} {address}\"\n    }}\n"
            );
        }
        text += "}\n";
    }
    text
}

/// A running CoreDNS; killed and reaped on drop.
pub struct CoreDns {
    server: Reference,
    quic: u16,
    h3: u16,
}

impl CoreDns {
    /// Writes the Corefile into `dir`, starts `binary` there and waits until
    /// it says both servers listen.
    pub fn spawn(binary: &Path, dir: &Path, tls: &TlsFiles) -> CoreDns {
        let (quic, h3) = (free_udp_port(), free_udp_port());
        let corefile = dir.join("Corefile");
        std::fs::write(&corefile, render(quic, h3, tls)).expect("write the Corefile");
        let mut command = Command::new(binary);
        command.arg("-conf").arg(&corefile).current_dir(dir);
        let mut server = Reference::start("coredns", command, Vec::new(), dir.join("coredns.log"));
        server.wait_for_output(&listening("quic", quic));
        server.wait_for_output(&listening("https3", h3));
        CoreDns { server, quic, h3 }
    }

    /// The loopback UDP port of DNS over QUIC.
    pub fn quic_port(&self) -> u16 {
        self.quic
    }

    /// The loopback UDP port of DNS over HTTP/3.
    pub fn h3_port(&self) -> u16 {
        self.h3
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
            certificate: "/tmp/dns/leaf.pem".into(),
            key: "/tmp/dns/leaf.key".into(),
            client_ca: None,
        }
    }

    /// Two servers, each bound to 127.0.0.1; nothing forwarded anywhere.
    #[test]
    fn the_corefile_stays_on_the_loopback() {
        let text = render(4001, 4002, &files());
        let servers: Vec<&str> = text
            .lines()
            .filter(|l| !l.starts_with(' ') && l.ends_with('{'))
            .collect();
        assert_eq!(servers, ["quic://.:4001 {", "https3://.:4002 {"]);
        let binds: Vec<&str> = text
            .lines()
            .filter(|l| l.trim_start().starts_with("bind"))
            .collect();
        assert_eq!(binds, ["    bind 127.0.0.1", "    bind 127.0.0.1"]);
        for forbidden in ["forward", "0.0.0.0", "hosts", "debug"] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
        }
    }

    #[test]
    fn the_corefile_is_written_as_coredns_reads_it() {
        let text = render(4001, 4002, &files());
        let block = |head: &'static str| {
            [
                head,
                "    bind 127.0.0.1",
                r#"    tls "/tmp/dns/leaf.pem" "/tmp/dns/leaf.key""#,
                "    template IN A interop.rurge.test {",
                r#"        answer "{{ .Name }} 60 IN A 192.0.2.10""#,
                "    }",
                "    template IN AAAA interop.rurge.test {",
                r#"        answer "{{ .Name }} 60 IN AAAA 2001:db8::10""#,
                "    }",
                "}",
            ]
        };
        let expected: Vec<&str> = block("quic://.:4001 {")
            .into_iter()
            .chain(block("https3://.:4002 {"))
            .collect();
        assert_eq!(text.lines().collect::<Vec<_>>(), expected);
        assert!(text.ends_with("}\n"));
        assert_eq!(listening("quic", 4001), "quic://.:4001 on 127.0.0.1");
    }

    /// Windows paths keep their backslashes: only a quote is escaped in a
    /// Corefile's quoted token.
    #[test]
    fn paths_are_quoted() {
        assert_eq!(quoted(r"C:\t\x.pem"), r#""C:\t\x.pem""#);
        assert_eq!(quoted("a b\"c"), r#""a b\"c""#);
    }
}
```

`tests/interop/tests/http2.rs`——把

```rust
//! every line sends `sni=tt.test` and trusts the fixture's CA.
```

换成

```rust
//! every line sends `sni=tt.test` and trusts the fixture's CA. With `h3=true`
//! a `trust-tunnel` goes over HTTP/3 instead (phase 2 M7 design 7.2), to an
//! endpoint that listens with QUIC alone (`[listen_protocols.quic]`).
```

`tests/interop/tests/http2.rs`——把

```rust
use rurge_interop::trusttunnel::{HOSTNAME, TrustTunnel, trusttunnel_or_skip};
```

换成

```rust
use rurge_interop::trusttunnel::{HOSTNAME, Listen, TrustTunnel, trusttunnel_or_skip};
```

`tests/interop/tests/http2.rs`——把

```rust
/// The endpoint with a certificate for its host, and the fixture whose CA
/// signed it.
fn endpoint(bin: &Path, dir: &Path) -> (TrustTunnel, Arc<TlsFixture>) {
    let fixture = TlsFixture::new(&[HOSTNAME]);
    let server = TrustTunnel::spawn(bin, dir, &leaf_files(&fixture, dir), USER, PASSWORD);
```

换成

```rust
/// The endpoint serving `listen`, with a certificate for its host, and the
/// fixture whose CA signed it.
fn endpoint(bin: &Path, dir: &Path, listen: Listen) -> (TrustTunnel, Arc<TlsFixture>) {
    let fixture = TlsFixture::new(&[HOSTNAME]);
    let server = TrustTunnel::spawn(bin, dir, listen, &leaf_files(&fixture, dir), USER, PASSWORD);
```

`tests/interop/tests/http2.rs`——把

```rust
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = endpoint(&bin, dir.path());
    let profile = profile(&[line(
        "TT",
```

换成

```rust
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = endpoint(&bin, dir.path(), Listen::Http2);
    let profile = profile(&[line(
        "TT",
```

`tests/interop/tests/http2.rs`——把

```rust
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = endpoint(&bin, dir.path());
    let profile = profile(&[line(
        "Wrong",
```

换成

```rust
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = endpoint(&bin, dir.path(), Listen::Http2);
    let profile = profile(&[line(
        "Wrong",
```

`tests/interop/tests/http2.rs`——把

```rust
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = endpoint(&bin, dir.path());
    let profile = profile(&[
        // the credentials positionally after the port, and named
```

换成

```rust
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = endpoint(&bin, dir.path(), Listen::Http2);
    let profile = profile(&[
        // the credentials positionally after the port, and named
```

`tests/interop/tests/http2.rs`——把

```rust
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = endpoint(&bin, dir.path());
    let profile = profile(&[line(
        "Shared",
```

换成

```rust
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = endpoint(&bin, dir.path(), Listen::Http2);
    let profile = profile(&[line(
        "Shared",
```

`tests/interop/tests/http2.rs`——把

```rust
    roundtrip(&out, echo).await;
}

```

换成

```rust
    roundtrip(&out, echo).await;
}

/// `h3=true` against an endpoint that listens with QUIC alone: one chunk and
/// many, each tunnel a classic CONNECT on the policy's QUIC connection.
#[tokio::test]
async fn trust_tunnel_over_http3_against_the_endpoint() {
    let Some(bin) = trusttunnel_or_skip("trust_tunnel_over_http3_against_the_endpoint") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = endpoint(&bin, dir.path(), Listen::Http3);
    let profile = profile(&[line(
        "TT3",
        "trust-tunnel",
        server.port(),
        &format!("username={USER}, password={PASSWORD}, h3=true"),
    )]);
    let (out, echo) = (
        outbound(&profile, "TT3", Some(&fixture)),
        echo_server().await,
    );
    roundtrip(&out, echo).await;
    roundtrip_big(&out, echo).await;
}

/// Refused over HTTP/3 as over HTTP/2: the endpoint answers 407.
#[tokio::test]
async fn a_wrong_trust_tunnel_password_over_http3_is_refused() {
    let Some(bin) = trusttunnel_or_skip("a_wrong_trust_tunnel_password_over_http3_is_refused")
    else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = endpoint(&bin, dir.path(), Listen::Http3);
    let profile = profile(&[line(
        "Wrong",
        "trust-tunnel",
        server.port(),
        &format!("username={USER}, password=nope, h3=true"),
    )]);
    let out = outbound(&profile, "Wrong", Some(&fixture));
    let refused = tokio::time::timeout(
        Duration::from_secs(10),
        out.connect_tcp(&target(echo_server().await), &ConnectOpts::default()),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "the endpoint answers within the bound\n{}",
            server.log_text()
        )
    });
    let Err(err) = refused else {
        panic!(
            "a wrong password must not open a tunnel\n{}",
            server.log_text()
        );
    };
    assert_eq!(
        err.to_string(),
        "trust-tunnel: authentication failed",
        "{}",
        server.log_text()
    );
}

```

新建 `tests/interop/tests/masque.rs`：

```rust
//! rurge's `masque` outbound against masque-server v0.13.0 (phase 2 M7
//! design 7.1, 7.4; M7-D6), on Linux only — the server is built for Linux
//! alone: TCP as classic CONNECTs (one chunk, many, and 9 MiB each way at
//! once), UDP as CONNECT-UDP with the packets in HTTP/3 datagrams (two
//! targets of one carrier, a stream for each), and a wrong password, which
//! the server refuses a tunnel and a CONNECT-UDP stream alike. The server
//! listens on a loopback UDP port with a certificate for 127.0.0.1 signed by
//! the fixture's CA, which rurge trusts; every target is a loopback IP
//! literal, which the server's target policy is told to allow. Port hopping
//! is not covered: the server would need a DNAT range.

mod common;

use common::*;
use rurge_interop::masque_server::{MasqueServer, PASSWORD, USERNAME, masque_server_or_skip};
use std::time::Duration;

/// What a refused login says, for a tunnel and a CONNECT-UDP stream alike.
const REFUSED: &str = "masque: proxy authentication required";

/// masque-server with a certificate for 127.0.0.1, and the fixture whose CA
/// signed it.
fn masque_server(bin: &Path, dir: &Path) -> (MasqueServer, Arc<TlsFixture>) {
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let server = MasqueServer::spawn(bin, dir, &leaf_files(&fixture, dir));
    (server, fixture)
}

/// The outbound of `M = masque, 127.0.0.1, <port>, username=test,
/// password=<password>`.
fn masque(port: u16, password: &str, fixture: &Arc<TlsFixture>) -> OutboundRef {
    let profile = format!(
        "[Proxy]\nM = masque, 127.0.0.1, {port}, username={USERNAME}, password={password}\n[Rule]\nFINAL,DIRECT\n"
    );
    outbound(&profile, "M", Some(fixture))
}

/// One carrier, two targets, a few rounds: each target gets a CONNECT-UDP
/// stream of its own with its first packet, and each answer comes from the
/// target it went to. `log` is the server's. Bounded like `roundtrip`.
async fn udp_two_targets(out: &OutboundRef, echoes: [SocketAddr; 2], log: &dyn Fn() -> String) {
    let bound = Duration::from_secs(10);
    let carrier = tokio::time::timeout(bound, out.open_udp(&ConnectOpts::default()))
        .await
        .expect("the carrier opens within the bound")
        .expect("the carrier opens");
    for round in 0..3u8 {
        for (n, echo) in (0u8..).zip(echoes) {
            let payload = [b'i', round, n];
            carrier
                .send_to(&payload, &target(echo))
                .await
                .unwrap_or_else(|e| panic!("{e}\n{}", log()));
            let mut buf = [0u8; 64];
            let (len, from) = tokio::time::timeout(bound, carrier.recv_from(&mut buf))
                .await
                .unwrap_or_else(|_| panic!("the echo comes back within the bound\n{}", log()))
                .unwrap();
            assert_eq!(
                (&buf[..len], from),
                (&payload[..], target(echo)),
                "{}",
                log()
            );
        }
    }
}

/// One chunk, many, then 9 MiB each way at once: past rurge's receive window
/// of a stream (8 MiB, phase 2 M7 design 3.2), what comes back needs the
/// credit rurge grants as it reads the tunnel's DATA frames.
#[tokio::test]
async fn tcp_against_masque_server() {
    let Some(bin) = masque_server_or_skip("tcp_against_masque_server") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = masque_server(&bin, dir.path());
    let (out, echo) = (
        masque(server.port(), PASSWORD, &fixture),
        echo_server().await,
    );
    roundtrip(&out, echo).await;
    roundtrip_big(&out, echo).await;
    transfer(&out, echo, (9 << 20) + 777).await;
}

/// The server announces extended CONNECT and HTTP datagrams; it reads no
/// capsule on HTTP/3, and rurge sends none.
#[tokio::test]
async fn udp_against_masque_server() {
    let Some(bin) = masque_server_or_skip("udp_against_masque_server") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = masque_server(&bin, dir.path());
    let out = masque(server.port(), PASSWORD, &fixture);
    let echoes = [udp_echo_server().await, udp_echo_server().await];
    udp_two_targets(&out, echoes, &|| server.log_text()).await;
}

/// The server answers a CONNECT without the right credentials 407: the
/// tunnel fails, and so does the first packet to a target, whose
/// CONNECT-UDP stream is refused alike.
#[tokio::test]
async fn a_wrong_masque_password_is_refused() {
    let Some(bin) = masque_server_or_skip("a_wrong_masque_password_is_refused") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = masque_server(&bin, dir.path());
    let out = masque(server.port(), "nope", &fixture);
    let bound = Duration::from_secs(10);
    let tcp = tokio::time::timeout(
        bound,
        out.connect_tcp(&target(echo_server().await), &ConnectOpts::default()),
    )
    .await
    .unwrap_or_else(|_| panic!("the server answers within the bound\n{}", server.log_text()));
    let Err(tcp) = tcp else {
        panic!(
            "a wrong password must not open a tunnel\n{}",
            server.log_text()
        );
    };
    assert_eq!(tcp.to_string(), REFUSED, "{}", server.log_text());
    // the carrier opens nothing until its first packet
    let carrier = tokio::time::timeout(bound, out.open_udp(&ConnectOpts::default()))
        .await
        .expect("the carrier opens within the bound")
        .expect("the carrier opens");
    let udp = tokio::time::timeout(
        bound,
        carrier.send_to(b"interop", &target(udp_echo_server().await)),
    )
    .await
    .unwrap_or_else(|_| panic!("the server answers within the bound\n{}", server.log_text()));
    let Err(udp) = udp else {
        panic!(
            "a wrong password must not open a CONNECT-UDP stream\n{}",
            server.log_text()
        );
    };
    assert_eq!(udp.to_string(), REFUSED, "{}", server.log_text());
}
```

新建 `tests/interop/tests/dns.rs`：

```rust
//! rurge's DNS over QUIC (`quic://`) and DNS over HTTP/3 (`h3://`) upstreams
//! against two references (phase 2 M7 design 7.3, 7.4), on every platform:
//! AdGuard dnsproxy v0.86.0, and CoreDNS v1.14.7, the strict one — a message
//! ID other than 0 or an edns-tcp-keepalive option closes its DoQ
//! connection, and its DoH3 server answers on `/dns-query` alone, the path an
//! `h3://` URL without one goes to. Each listens on loopback UDP ports with a
//! certificate for 127.0.0.1 and answers `DNS_NAME` (A and AAAA) by itself.
//! The resolver verifies a server's certificate against the shared roots,
//! which no test CA can join: every profile sets
//! `encrypted-dns-skip-cert-verification`.
//!
//! Each server is asked through two resolvers of one profile: one that
//! dials directly, as `rurge run` does, and one whose QUIC paths are the UDP
//! flows of a connector that counts them, as with
//! `encrypted-dns-follow-outbound-mode` — its two lookups, the second past
//! the cache, ask four questions on one connection.

mod common;

use common::*;
use rurge_dns::system::StaticSystemDns;
use rurge_dns::{LookupOpts, Resolver, ResolverConfig, ResolverDeps, Source};
use rurge_interop::coredns::{CoreDns, coredns_or_skip};
use rurge_interop::dnsproxy::{Dnsproxy, dnsproxy_or_skip};
use rurge_interop::{DNS_NAME, DNS_V4, DNS_V6};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedDatagram, BoxedStream, Connector, DirectConnector};
use rurge_net::http::{HttpClient, HttpClientConfig};
use rurge_net::resource::{ResourceManager, ResourceOptions};
use rurge_rules::SetRegistry;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// The system's direct connector, offering QUIC no socket of its own: a
/// connection's path is then a UDP flow of `connect_udp`, which counts them.
struct Counted {
    direct: DirectConnector,
    flows: AtomicUsize,
}

impl Counted {
    fn new() -> Counted {
        Counted {
            direct: DirectConnector::new(Arc::new(SystemResolve)),
            flows: AtomicUsize::new(0),
        }
    }

    fn flows(&self) -> usize {
        self.flows.load(Ordering::SeqCst)
    }
}

impl Connector for Counted {
    fn connect<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedStream>> {
        self.direct.connect(target, opts)
    }

    fn connect_udp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
        self.flows.fetch_add(1, Ordering::SeqCst);
        self.direct.connect_udp(target, opts)
    }
}

/// A certificate for 127.0.0.1 and its key, as PEM files in `dir`.
fn certificate(dir: &Path) -> TlsFiles {
    leaf_files(&TlsFixture::new(&["127.0.0.1"]), dir)
}

/// A resolver whose profile (in `dir`) has `url` for its one server —
/// certificates unverified, A and AAAA asked —, reaching it through
/// `connector`.
fn resolver(dir: &Path, url: &str, connector: Arc<dyn Connector>) -> Arc<Resolver> {
    let profile = format!(
        "[General]\nencrypted-dns-server = {url}\nencrypted-dns-skip-cert-verification = true\nipv6 = true\n[Rule]\nFINAL,DIRECT\n"
    );
    let loaded = from_text(&profile, &dir.join("dns.conf"), &LoadOptions::for_tests());
    assert!(
        !loaded.diagnostics.has_errors(),
        "{:?}",
        loaded
            .diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
    );
    let cfg = loaded.config;
    let client = Arc::new(HttpClient::new(connector.clone(), HttpClientConfig::default()).unwrap());
    let resources = ResourceManager::with_options(
        dir.to_path_buf(),
        client,
        ResourceOptions {
            offline: true,
            ..ResourceOptions::default()
        },
    );
    let (sets, _) = SetRegistry::build(&cfg, resources.clone(), dir);
    let system = StaticSystemDns {
        has_ipv6: true,
        ..StaticSystemDns::default()
    };
    let (resolver, diagnostics) = Resolver::new(
        ResolverConfig::from_config(&cfg),
        ResolverDeps {
            connector,
            sets,
            system: Arc::new(system),
            resources,
        },
    );
    assert!(
        diagnostics.is_empty(),
        "{:?}",
        diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
    );
    resolver
}

/// `DNS_NAME` through `resolver`: its A and AAAA records, from the server
/// called `name`. Bounded like `roundtrip`.
async fn resolves(resolver: &Resolver, opts: LookupOpts, name: &str, log: &dyn Fn() -> String) {
    let result = tokio::time::timeout(Duration::from_secs(10), resolver.lookup(DNS_NAME, opts))
        .await
        .unwrap_or_else(|_| panic!("no answer within the bound\n{}", log()))
        .unwrap_or_else(|e| panic!("{e}\n{}", log()));
    assert_eq!(
        (result.v4, result.v6),
        (vec![DNS_V4], vec![DNS_V6]),
        "{}",
        log()
    );
    assert_eq!(
        result.source,
        Source::Upstream(name.to_string()),
        "{}",
        log()
    );
}

/// `DNS_NAME` from the server at `url`, called `name`, through a resolver
/// that dials directly, then twice through one whose connector counts its
/// connections — the second time past the cache —: those four questions on
/// one connection. `log` is the server's.
async fn lookups(dir: &Path, url: &str, name: &str, log: &dyn Fn() -> String) {
    let direct: Arc<dyn Connector> = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
    resolves(
        &resolver(dir, url, direct),
        LookupOpts::default(),
        name,
        log,
    )
    .await;
    let counted = Arc::new(Counted::new());
    let carried = resolver(dir, url, counted.clone());
    resolves(&carried, LookupOpts::default(), name, log).await;
    let past_the_cache = LookupOpts {
        bypass_cache: true,
        ..LookupOpts::default()
    };
    resolves(&carried, past_the_cache, name, log).await;
    assert_eq!(counted.flows(), 1, "one connection\n{}", log());
}

#[tokio::test]
async fn doq_against_dnsproxy() {
    let Some(bin) = dnsproxy_or_skip("doq_against_dnsproxy") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = Dnsproxy::spawn(&bin, dir.path(), &certificate(dir.path()));
    let url = format!("quic://127.0.0.1:{}", server.quic_port());
    lookups(dir.path(), &url, &url, &|| server.log_text()).await;
}

/// No path in the URL: the requests go to `/dns-query`.
#[tokio::test]
async fn doh3_against_dnsproxy() {
    let Some(bin) = dnsproxy_or_skip("doh3_against_dnsproxy") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = Dnsproxy::spawn(&bin, dir.path(), &certificate(dir.path()));
    let url = format!("h3://127.0.0.1:{}", server.h3_port());
    let name = format!("{url}/dns-query");
    lookups(dir.path(), &url, &name, &|| server.log_text()).await;
}

/// CoreDNS answers no query with a message ID other than 0 or an
/// edns-tcp-keepalive option: it closes the connection instead.
#[tokio::test]
async fn doq_against_coredns() {
    let Some(bin) = coredns_or_skip("doq_against_coredns") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = CoreDns::spawn(&bin, dir.path(), &certificate(dir.path()));
    let url = format!("quic://127.0.0.1:{}", server.quic_port());
    lookups(dir.path(), &url, &url, &|| server.log_text()).await;
}

/// CoreDNS answers on `/dns-query` alone, where a URL without a path goes.
#[tokio::test]
async fn doh3_against_coredns() {
    let Some(bin) = coredns_or_skip("doh3_against_coredns") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = CoreDns::spawn(&bin, dir.path(), &certificate(dir.path()));
    let url = format!("h3://127.0.0.1:{}", server.h3_port());
    let name = format!("{url}/dns-query");
    lookups(dir.path(), &url, &name, &|| server.log_text()).await;
}
```

- [ ] **Step 2: 运行**

Run: `cargo test -p rurge-interop`
Expected: 本机没有这些服务端时，9 条新的互操作用例（`trust-tunnel` 经 HTTP/3 的往返与口令错误、masque-server 的 TCP / UDP / 口令错误、dnsproxy 与 CoreDNS 各自的 DoQ 与 DoH3）各打印一行 `skipping …` 后通过；夹具自己的单元用例（新增 8 条）照常断言并通过。互操作由 CI 证明（masque-server 只在 Linux；TrustTunnel 在 Linux / macOS；dnsproxy 与 CoreDNS 三个平台）。

- [ ] **Step 3: CI 与文档**

CI：按 SHA-256 安装 masque-server（Linux）、dnsproxy 与 CoreDNS（三个平台），设对应的环境变量：

`.github/workflows/ci.yml`——把

```yaml
          echo "RURGE_TEST_TRUSTTUNNEL=$bin" >> "$GITHUB_ENV"
```

换成

```yaml
          echo "RURGE_TEST_TRUSTTUNNEL=$bin" >> "$GITHUB_ENV"
      - name: Install masque-server for the MASQUE interoperability tests
        if: runner.os == 'Linux'
        shell: bash
        run: |
          set -euo pipefail
          version=0.13.0
          asset="masque-v$version-linux-x86_64.tar.gz"
          sha=0ae2e3359db5149670df0d084e86104bf0332d0fd43501c9072ec39a48a1f298
          cd "$RUNNER_TEMP"
          curl -fsSL --retry 3 --retry-all-errors -o "$asset" "https://github.com/Vincent-bin/masque-server/releases/download/v$version/$asset"
          actual=$(sha256sum "$asset" | cut -d' ' -f1)
          if [ "$actual" != "$sha" ]; then
            echo "masque-server checksum mismatch: expected $sha, got $actual"
            exit 1
          fi
          mkdir -p masque-server
          tar -xzf "$asset" -C masque-server
          bin=$(find "$PWD/masque-server" -type f -name masque-server | head -n 1)
          [ -n "$bin" ] || { echo "no masque-server binary in $asset"; exit 1; }
          chmod +x "$bin"
          echo "RURGE_TEST_MASQUE_SERVER=$bin" >> "$GITHUB_ENV"
      - name: Install dnsproxy for the DNS over QUIC and HTTP/3 interoperability tests
        shell: bash
        run: |
          set -euo pipefail
          version=0.86.0
          case "$RUNNER_OS" in
            Linux)   asset="dnsproxy-linux-amd64-v$version.tar.gz";  sha=17ac43f76ebfcedf547c663f6f31aa70e68b8eb6fc41d2fc37d3bfe21a2ced66 ;;
            Windows) asset="dnsproxy-windows-amd64-v$version.zip";   sha=dedc186ddd4b96bf92474f91d972f67da0824e0141d09e3102fd96db9f8a0dbc ;;
            macOS)   asset="dnsproxy-darwin-arm64-v$version.tar.gz"; sha=5964db3a45d39df4e41f74907f50657a80907c514355f9ad75b9666e3431d277 ;;
            *) echo "unexpected runner OS: $RUNNER_OS"; exit 1 ;;
          esac
          cd "$RUNNER_TEMP"
          curl -fsSL --retry 3 --retry-all-errors -o "$asset" "https://github.com/AdguardTeam/dnsproxy/releases/download/v$version/$asset"
          if command -v sha256sum >/dev/null 2>&1; then
            actual=$(sha256sum "$asset" | cut -d' ' -f1)
          else
            actual=$(shasum -a 256 "$asset" | cut -d' ' -f1)
          fi
          if [ "$actual" != "$sha" ]; then
            echo "dnsproxy checksum mismatch: expected $sha, got $actual"
            exit 1
          fi
          mkdir -p dnsproxy
          case "$asset" in
            *.zip) unzip -q "$asset" -d dnsproxy ;;
            *)     tar -xzf "$asset" -C dnsproxy ;;
          esac
          bin=$(find "$PWD/dnsproxy" -type f \( -name dnsproxy -o -name dnsproxy.exe \) | head -n 1)
          [ -n "$bin" ] || { echo "no dnsproxy binary in $asset"; exit 1; }
          chmod +x "$bin"
          if [ "$RUNNER_OS" = "Windows" ]; then bin=$(cygpath -w "$bin"); fi
          echo "RURGE_TEST_DNSPROXY=$bin" >> "$GITHUB_ENV"
      - name: Install CoreDNS for the DNS over QUIC and HTTP/3 interoperability tests
        shell: bash
        run: |
          set -euo pipefail
          version=1.14.7
          case "$RUNNER_OS" in
            Linux)   asset="coredns_${version}_linux_amd64.tgz";   sha=9a09356438c6d9591cb80011a2a7a52f87fc3a113f6690e41f243f98b98faac9 ;;
            Windows) asset="coredns_${version}_windows_amd64.tgz"; sha=19756e969e4ddd16ee0c189018e5659f6695113223e590709222433f7338504d ;;
            macOS)   asset="coredns_${version}_darwin_arm64.tgz";  sha=0005d154d49c73a88b2c56f429f35106e46c4ca37ee6de4e4d482d7de5c4b8ba ;;
            *) echo "unexpected runner OS: $RUNNER_OS"; exit 1 ;;
          esac
          cd "$RUNNER_TEMP"
          curl -fsSL --retry 3 --retry-all-errors -o "$asset" "https://github.com/coredns/coredns/releases/download/v$version/$asset"
          if command -v sha256sum >/dev/null 2>&1; then
            actual=$(sha256sum "$asset" | cut -d' ' -f1)
          else
            actual=$(shasum -a 256 "$asset" | cut -d' ' -f1)
          fi
          if [ "$actual" != "$sha" ]; then
            echo "CoreDNS checksum mismatch: expected $sha, got $actual"
            exit 1
          fi
          mkdir -p coredns
          tar -xzf "$asset" -C coredns
          bin=$(find "$PWD/coredns" -type f \( -name coredns -o -name coredns.exe \) | head -n 1)
          [ -n "$bin" ] || { echo "no coredns binary in $asset"; exit 1; }
          chmod +x "$bin"
          if [ "$RUNNER_OS" = "Windows" ]; then bin=$(cygpath -w "$bin"); fi
          echo "RURGE_TEST_COREDNS=$bin" >> "$GITHUB_ENV"
```

`tests/interop/README.md`——把

```markdown
`rurge-interop` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它把 [sing-box](https://sing-box.sagernet.org/)、[xray](https://github.com/XTLS/Xray-core)、[shadowsocks-rust](https://github.com/shadowsocks/shadowsocks-rust) 的 `ssserver`、Surge 官方的 `snell-server`、[TrustTunnel](https://github.com/TrustTunnel/TrustTunnel) 的 `trusttunnel_endpoint` 与 TUIC 官方 [tuic](https://github.com/tuic-protocol/tuic) 0.8.5 的 `tuic-server` 作为参照实现，以回环子进程的方式拉起来，驱动 rurge 的 `http` / `https` / `socks5` / `trojan` / `vmess` / `anytls` / `wireguard` / `ss` / `snell` / `h2-connect` / `trust-tunnel` / `hysteria2` / `tuic-v5` / `tuic` 出站，以及包在 Shadow TLS 里的 `trojan`，去连它们，验证 rurge 与真实的第三方实现互通。xray 只用来跑 `vmess`：VMess 协议由 xray 所在的这一脉实现定义，sing-box 的实现是重写，手写的编解码需要两个独立参照互相印证（M2 设计 M2-D5）。tuic-server 只用来跑 `tuic`（TUIC v4）：sing-box 只实现 v5（M7 设计 M7-D3）。
```

换成

```markdown
`rurge-interop` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它把 [sing-box](https://sing-box.sagernet.org/)、[xray](https://github.com/XTLS/Xray-core)、[shadowsocks-rust](https://github.com/shadowsocks/shadowsocks-rust) 的 `ssserver`、Surge 官方的 `snell-server`、[TrustTunnel](https://github.com/TrustTunnel/TrustTunnel) 的 `trusttunnel_endpoint`、TUIC 官方 [tuic](https://github.com/tuic-protocol/tuic) 0.8.5 的 `tuic-server`、[masque-server](https://github.com/Vincent-bin/masque-server)、AdGuard [dnsproxy](https://github.com/AdguardTeam/dnsproxy) 与 [CoreDNS](https://coredns.io/) 作为参照实现，以回环子进程的方式拉起来，驱动 rurge 的 `http` / `https` / `socks5` / `trojan` / `vmess` / `anytls` / `wireguard` / `ss` / `snell` / `h2-connect` / `trust-tunnel`（HTTP/2 与 `h3=true` 的 HTTP/3）/ `hysteria2` / `tuic-v5` / `tuic` / `masque` 出站、包在 Shadow TLS 里的 `trojan`，以及 `quic://`（DoQ）与 `h3://`（DoH3）两种 DNS 上游，去连它们，验证 rurge 与真实的第三方实现互通。xray 只用来跑 `vmess`：VMess 协议由 xray 所在的这一脉实现定义，sing-box 的实现是重写，手写的编解码需要两个独立参照互相印证（M2 设计 M2-D5）。tuic-server 只用来跑 `tuic`（TUIC v4）：sing-box 只实现 v5（M7 设计 M7-D3）。masque-server 只用来跑 `masque`，dnsproxy 与 CoreDNS 只用来跑 DoQ / DoH3（M7 设计 M7-D6 与第 7.4 节）。
```

`tests/interop/README.md`——把

```markdown
本地默认不安装 sing-box、xray、shadowsocks-rust、snell-server、TrustTunnel endpoint 与 tuic-server：`cargo test -p rurge-interop` 会正常通过，sing-box 的二十七个互操作用例、xray 的两个、shadowsocks-rust 的两个、snell-server 的四个、TrustTunnel endpoint 的四个与 tuic-server 的三个互操作用例各打印一行 `skipping …` 后直接返回（各夹具自身的单元测试——配置渲染、"配置绝不碰本机"的安全守卫、取空闲端口——照常运行并断言）。要在本机真正跑 sing-box 的用例，二选一：
```

换成

```markdown
本地默认不安装 sing-box、xray、shadowsocks-rust、snell-server、TrustTunnel endpoint、tuic-server、masque-server、dnsproxy 与 CoreDNS：`cargo test -p rurge-interop` 会正常通过，sing-box 的二十七个互操作用例、xray 的两个、shadowsocks-rust 的两个、snell-server 的四个、TrustTunnel endpoint 的六个、tuic-server 的三个、masque-server 的三个、dnsproxy 的两个与 CoreDNS 的两个互操作用例各打印一行 `skipping …` 后直接返回（各夹具自身的单元测试——配置渲染、"配置绝不碰本机"的安全守卫、取空闲端口——照常运行并断言）。要在本机真正跑 sing-box 的用例，二选一：
```

`tests/interop/README.md`——把

```markdown
xray、shadowsocks-rust、snell-server、TrustTunnel endpoint 与 tuic-server 的本机运行方式同理（tuic-server 只认环境变量，不在 `PATH` 上找），见下面「xray」「shadowsocks-rust」「snell-server」「TrustTunnel」「tuic-server」五节。
```

换成

```markdown
xray、shadowsocks-rust、snell-server、TrustTunnel endpoint、tuic-server、masque-server、dnsproxy 与 CoreDNS 的本机运行方式同理（tuic-server 只认环境变量，不在 `PATH` 上找），见下面「xray」「shadowsocks-rust」「snell-server」「TrustTunnel」「tuic-server」「masque-server」「dnsproxy 与 CoreDNS」七节。
```

`tests/interop/README.md`——把

```markdown
- `RURGE_INTEROP_REQUIRED=1`：找不到二进制时让用例直接失败，而不是打印 `skipping …` 后跳过；对 sing-box、xray、shadowsocks-rust 与 sshd 各夹具都生效，对 snell-server 只在 Linux 上生效（它只有 Linux 版），对 TrustTunnel endpoint 只在 Linux 与 macOS 上生效（它没有 Windows 版），对 tuic-server 只在 Linux 上生效（CI 只在 Linux 上装它）。CI 会设置它，本机一般不需要。
```

换成

```markdown
- `RURGE_TEST_MASQUE_SERVER`：`masque-server` 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_TEST_DNSPROXY`：AdGuard `dnsproxy` 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_TEST_COREDNS`：`coredns` 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_INTEROP_REQUIRED=1`：找不到二进制时让用例直接失败，而不是打印 `skipping …` 后跳过；对 sing-box、xray、shadowsocks-rust、dnsproxy、CoreDNS 与 sshd 各夹具都生效，对 snell-server 与 masque-server 只在 Linux 上生效（它们只有 Linux 版），对 TrustTunnel endpoint 只在 Linux 与 macOS 上生效（它没有 Windows 版），对 tuic-server 只在 Linux 上生效（CI 只在 Linux 上装它）。CI 会设置它，本机一般不需要。
```

`tests/interop/README.md`——把

```markdown
**不覆盖端口跳跃**：`port-hopping`（Hysteria 2 与 TUIC）要在服务端用防火墙把一段端口 DNAT 到它的监听端口（Hysteria 文档的部署方式），夹具不改本机的防火墙与网络设置，回环上做不到；端口跳跃由回环假服务端（`rurge_proto_quic::testing::FakeHysteria2` 与 `FakeTuic` 同时监听几个端口，跳跃间隔经只用于测试的入口缩短）与手工验收（`docs/acceptance/phase2-manual.md` 的 M7a 与 M7b 两节）覆盖。
```

换成

```markdown
**不覆盖端口跳跃**：`port-hopping`（Hysteria 2、TUIC 与 MASQUE）要在服务端用防火墙把一段端口 DNAT 到它的监听端口（Hysteria 文档的部署方式），夹具不改本机的防火墙与网络设置，回环上做不到；端口跳跃由回环假服务端（`rurge_proto_quic::testing::FakeHysteria2`、`FakeTuic` 与 `FakeH3Proxy` 同时监听几个端口，跳跃间隔经只用于测试的入口缩短）与手工验收（`docs/acceptance/phase2-manual.md` 的 M7a、M7b 与 M7c 三节）覆盖。
```

`tests/interop/README.md`——把

```markdown
[TrustTunnel](https://github.com/TrustTunnel/TrustTunnel) 的 endpoint 是 `trust-tunnel` 的参照实现；它的 TCP 模式就是标准的 HTTP/2 CONNECT（`:authority` 为 `host:port`，`proxy-authorization: Basic`，200 / 407 / 502），所以同时充当 `h2-connect` 普通 CONNECT 的参照服务端（sing-box 1.14.2 的 `http` 入站只说 HTTP/1，见上面「覆盖范围」末尾）。互操作测试固定 **v1.1.0**（2026-09-01 发布），只有 Linux 与 macOS 版。CI 下载并校验以下两个发布包（SHA-256 取自 GitHub 发布 API 每个资产的 `digest`）：
```

换成

```markdown
[TrustTunnel](https://github.com/TrustTunnel/TrustTunnel) 的 endpoint 是 `trust-tunnel` 的参照实现（HTTP/2，以及 `h3=true` 的 HTTP/3）；它的 TCP 模式就是标准的 HTTP/2 CONNECT（`:authority` 为 `host:port`，`proxy-authorization: Basic`，200 / 407 / 502），所以同时充当 `h2-connect` 普通 CONNECT 的参照服务端（sing-box 1.14.2 的 `http` 入站只说 HTTP/1，见上面「覆盖范围」末尾）；它的 QUIC 监听（`[listen_protocols.quic]`）在 HTTP/3 上收同样的普通 CONNECT（只有 `:method` 与 `:authority`，没有 extended CONNECT，也不开 QUIC 数据报）。互操作测试固定 **v1.1.0**（2026-09-01 发布），只有 Linux 与 macOS 版。CI 下载并校验以下两个发布包（SHA-256 取自 GitHub 发布 API 每个资产的 `digest`）：
```

`tests/interop/README.md`——把

```markdown
`tests/http2.rs` 驱动的四个用例覆盖：

- `trust-tunnel`：单块与跨多块（100 000 字节）的 TCP 往返；口令错误时 endpoint 回 407，出站报 `trust-tunnel: authentication failed`。
- `h2-connect`：凭据写在端口之后（位置参数）与写成 `username=` / `password=` 两种，各做单块与跨多块的 TCP 往返；`max-streams=3` 时同时打开七条隧道并全部保持（多于一条连接能承载的流，池要开新连接），每条各自回显自己的字节，之后再开一条新隧道。

**不覆盖 CONNECT-UDP**：TrustTunnel 的 UDP 走它自己的 `_udp2` 复用格式，不是 RFC 9298 的 CONNECT-UDP；目前找不到可用的稳定版参照服务端（sing-box 要到 1.15.0 才在 `http` 入站上提供，且它的发布版是否通告 extended CONNECT 未经核实）。`h2-connect` 的 CONNECT-UDP 由回环假服务端（`rurge_proto::testing::FakeH2Proxy`，开 `enable_connect_protocol`）与手工验收（`docs/acceptance/phase2-manual.md` 的 M6c 一节）覆盖。

endpoint 按 SNI 精确匹配它的主机（`main_hosts[].hostname`），所以夹具的证书签给 `tt.test`，rurge 的每一行都写 `sni=tt.test` 并信任夹具的 CA；ALPN 固定 `h2`（不发 ALPN 时 endpoint 按 HTTP/1 处理）。

CI 只在 Linux 与 macOS 上下载、校验并设置 `RURGE_TEST_TRUSTTUNNEL`；Windows 上这四个用例照常编译，运行时打印 `skipping …` 后返回（`RURGE_INTEROP_REQUIRED=1` 对它只在 Linux 与 macOS 上生效）。本机不安装它：`RURGE_TEST_TRUSTTUNNEL`（优先于 `PATH` 查找）没有指向可执行文件、`PATH` 上也找不到 `trusttunnel_endpoint` 时同样跳过；这个 crate 不会下载或安装它。

渲染出的配置（`rurge_interop::trusttunnel::render`）是三个 TOML 文件：`vpn.toml`（`listen_address = "127.0.0.1:<端口>"`、`ipv6_available = false`、只开 `[listen_protocols.http2]`、`[forward_protocol]` 为 `direct`，以及 `allow_private_network_connections = true`——默认值 `false` 会让 endpoint 拒绝连回环上的 echo）、`hosts.toml`（一个主机 `tt.test` 与证书、私钥的绝对路径）与 `credentials.toml`（一个用户）。夹具的单元用例 `the_configuration_stays_on_the_loopback` 断言只监听回环、只开 HTTP/2。启动命令是 `trusttunnel_endpoint vpn.toml hosts.toml`，就绪与否看 TCP 端口能否连上。
```

换成

```markdown
`tests/http2.rs` 驱动的六个用例覆盖：

- `trust-tunnel`：单块与跨多块（100 000 字节）的 TCP 往返；口令错误时 endpoint 回 407，出站报 `trust-tunnel: authentication failed`。
- `trust-tunnel` 的 `h3=true`（阶段 2 / M7c）：endpoint 只开 QUIC 监听（没有 HTTP/2 可以退回），单块与跨多块的 TCP 往返；口令错误时同样回 407，出站报同一个 `trust-tunnel: authentication failed`。
- `h2-connect`：凭据写在端口之后（位置参数）与写成 `username=` / `password=` 两种，各做单块与跨多块的 TCP 往返；`max-streams=3` 时同时打开七条隧道并全部保持（多于一条连接能承载的流，池要开新连接），每条各自回显自己的字节，之后再开一条新隧道。

**不覆盖 HTTP/2 上的 CONNECT-UDP**：TrustTunnel 的 UDP 走它自己的 `_udp2` 复用格式，不是 RFC 9298 的 CONNECT-UDP；目前找不到可用的稳定版参照服务端。`h2-connect` 的 CONNECT-UDP 由回环假服务端（`rurge_proto::testing::FakeH2Proxy`，开 `enable_connect_protocol`）与手工验收（`docs/acceptance/phase2-manual.md` 的 M6c 一节）覆盖；HTTP/3 上的 CONNECT-UDP（`masque`）由 masque-server 覆盖（见下面「masque-server」一节）。sing-box 的预发布版（1.15.0-alpha.7 起）给 `http` 入站加了 HTTP/2、HTTP/3 与 CONNECT-UDP，三平台都有发布包，是将来两种 CONNECT-UDP 与 HTTP/3 普通 CONNECT 的跨平台参照；它还是 alpha，现在不用。

endpoint 按 SNI 精确匹配它的主机（`main_hosts[].hostname`），所以夹具的证书签给 `tt.test`，rurge 的每一行都写 `sni=tt.test` 并信任夹具的 CA；ALPN 固定 `h2`（不发 ALPN 时 endpoint 按 HTTP/1 处理），`h3=true` 时固定 `h3`。

CI 只在 Linux 与 macOS 上下载、校验并设置 `RURGE_TEST_TRUSTTUNNEL`；Windows 上这六个用例照常编译，运行时打印 `skipping …` 后返回（`RURGE_INTEROP_REQUIRED=1` 对它只在 Linux 与 macOS 上生效）。本机不安装它：`RURGE_TEST_TRUSTTUNNEL`（优先于 `PATH` 查找）没有指向可执行文件、`PATH` 上也找不到 `trusttunnel_endpoint` 时同样跳过；这个 crate 不会下载或安装它。

渲染出的配置（`rurge_interop::trusttunnel::render`）是三个 TOML 文件：`vpn.toml`（`listen_address = "127.0.0.1:<端口>"`、`ipv6_available = false`、只开一种协议——HTTP/2 时 `[listen_protocols.http2]`，HTTP/3 时 `[listen_protocols.quic]`（空表即启用，各项用 endpoint 的缺省）——、`[forward_protocol]` 为 `direct`，以及 `allow_private_network_connections = true`——默认值 `false` 会让 endpoint 拒绝连回环上的 echo）、`hosts.toml`（一个主机 `tt.test` 与证书、私钥的绝对路径）与 `credentials.toml`（一个用户）。夹具的单元用例 `the_configuration_stays_on_the_loopback` 断言只监听回环、每种模式只开它那一种协议。启动命令是 `trusttunnel_endpoint vpn.toml hosts.toml`，就绪与否看 TCP 端口能否连上；HTTP/3 时再等它的日志（标准输出）里出现 `Listening to UDP`。endpoint 总在 `listen_address` 上开 TCP 监听（只开 QUIC 时它对 TCP 连接只接受、不处理），QUIC 监听绑在同一个端口号的 UDP 上，所以 HTTP/3 时夹具挑一个 TCP 与 UDP 都空闲的端口（不能写 0：两边会拿到不同的临时端口，日志打印的也只是配置里的地址）。
```

`tests/interop/README.md`——把

```markdown
渲染出的配置（`rurge_interop::tuic_server::render`）是一个 JSON 对象，只有 0.8.5 认识的七个键（它拒绝不认识的键）：`port`、`token`（一个 token）、`certificate` 与 `private_key`（夹具的证书与私钥，PEM 文件的路径）、`ip`（`127.0.0.1`）、`alpn`（`["h3"]`：它的缺省是不选 ALPN，rurge 的 QUIC 客户端于是握手失败）与 `log_level`（`info`）；其余用它的缺省（Cubic、空闲 15 秒、认证时限 1 秒）。启动命令是 `tuic-server -c <配置文件>`；它没有 TCP 端口可探，就绪与否看日志里出现 `Server started`（`info` 级，绑定端口之后才打印）。夹具的单元用例 `the_configuration_stays_on_the_loopback` 与 `the_configuration_is_written_as_tuic_server_reads_it` 断言只监听回环、只写这七个键。它转发 UDP 用的 socket 绑在所有地址上的一个空闲端口（`[::]:0`；0.8.5 总是这样，没有配置项）。以上命令行、七个键与就绪日志是按计划 P14 从 0.8.5 的源码读出的，还没有对真实的二进制跑过：第一次在 Linux CI 上运行时核对（M7b 计划「延后事项」#6）。

## sshd
```

换成

```markdown
渲染出的配置（`rurge_interop::tuic_server::render`）是一个 JSON 对象，只有 0.8.5 认识的七个键（它拒绝不认识的键）：`port`、`token`（一个 token）、`certificate` 与 `private_key`（夹具的证书与私钥，PEM 文件的路径）、`ip`（`127.0.0.1`）、`alpn`（`["h3"]`：它的缺省是不选 ALPN，rurge 的 QUIC 客户端于是握手失败）与 `log_level`（`info`）；其余用它的缺省（Cubic、空闲 15 秒、认证时限 1 秒）。启动命令是 `tuic-server -c <配置文件>`；它没有 TCP 端口可探，就绪与否看日志里出现 `Server started`（`info` 级，绑定端口之后才打印）。夹具的单元用例 `the_configuration_stays_on_the_loopback` 与 `the_configuration_is_written_as_tuic_server_reads_it` 断言只监听回环、只写这七个键。它转发 UDP 用的 socket 绑在所有地址上的一个空闲端口（`[::]:0`；0.8.5 总是这样，没有配置项）。以上命令行、七个键与就绪日志是按计划 P14 从 0.8.5 的源码读出的，还没有对真实的二进制跑过：第一次在 Linux CI 上运行时核对（M7b 计划「延后事项」#6）。

## masque-server

[masque-server](https://github.com/Vincent-bin/masque-server)（MIT）是 `masque` 的参照（阶段 2 / M7 设计 M7-D6）：经典 CONNECT 承载 TCP，RFC 9298 的 CONNECT-UDP 加 HTTP/3 数据报（RFC 9297）承载 UDP；每个 CONNECT 都要带 `proxy-authorization: Basic`（按 Argon2id 散列核对，不对就回 407）。它建在 quiche 0.29.3（BoringSSL）上，SETTINGS 通告 extended CONNECT 与 `H3_DATAGRAM`，并照 quiche 的缺省发 GREASE（保留的设置、保留类型的单向流、应答头之前的保留帧）。互操作测试固定 **v0.13.0**（2026-09-02 发布）。服务端只有 Linux 版（macOS 只发诊断工具 `masque-probe`，没有 Windows 版），CI 只在 Linux 上下载并校验这一个资产（SHA-256 取自 GitHub 发布 API 的 `digest`）：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (x86_64，静态链接的 musl 版) | `masque-v0.13.0-linux-x86_64.tar.gz` | `0ae2e3359db5149670df0d084e86104bf0332d0fd43501c9072ec39a48a1f298` |

发布包里是一个顶层目录 `masque-v0.13.0-linux-x86_64/`，服务端是其中的 `bin/masque-server`（另有 `masque-probe`、示例配置、systemd 与监控文件，不用）。

`tests/masque.rs` 的三个用例覆盖：TCP 的单块、跨多块（100 000 字节）与每个方向 9 MiB 同时往返（超过 rurge 每条流 8 MiB 的接收窗口：回来的数据要靠 rurge 读 DATA 帧时放出的流控额度）；UDP：同一个载体轮流发往两个回环 UDP 回显、各三轮——每个目标随它的第一个包开一条 CONNECT-UDP 流（对称型），包走 HTTP/3 数据报（masque-server 在 HTTP/3 上不读 capsule，rurge 也不发），每个回包都来自它所发往的目标；口令错误：服务端回 407，TCP 隧道与发往一个目标的第一个 UDP 包（它的 CONNECT-UDP 流同样被拒）都以 `masque: proxy authentication required` 失败。不覆盖端口跳跃（见上面「覆盖范围」末尾）、服务端不支持 CONNECT-UDP 的情形与流上收到的 DATAGRAM capsule：它们由回环假服务端（`rurge_proto_quic::testing::FakeH3Proxy`）覆盖。

CI 只在 Linux 上下载、校验并设置 `RURGE_TEST_MASQUE_SERVER`；Windows 与 macOS 上这三个用例照常编译，运行时打印 `skipping …` 后返回（`RURGE_INTEROP_REQUIRED=1` 对 masque-server 只在 Linux 上生效）。本机不安装它：`RURGE_TEST_MASQUE_SERVER`（优先于 `PATH` 查找）没有指向可执行文件、`PATH` 上也找不到 `masque-server` 时同样跳过；这个 crate 不会下载或安装它。

渲染出的配置（`rurge_interop::masque_server::render`）是一个 TOML 文件，它拒绝不认识的键，所以只写这些：`[tls]`（夹具的证书与 PKCS#8 私钥，PEM 文件的绝对路径——它经 BoringSSL 的 `PEM_read_bio_PrivateKey` 读私钥，PKCS#8 照收，不必像它的证书脚本那样转成 SEC1）；一个 `[[listeners]]`（`listen_addr = "127.0.0.1:<端口>"`、`transport = "http3"`、`shards = 1`）与它的 `[listeners.auth]`（`mode = "basic"`，一个用户：masque-server 自己的端到端测试用的 `test`，存口令 `test-password` 的 Argon2id 散列）；`[tcp_proxy]` 与 `[udp_proxy]` 都是 `allow_targets = ["0.0.0.0/0", "::/0"]`、`deny_targets = []`（它的缺省拒绝回环与私网目标，回环上的 echo 要放开）；`[ip_proxy] enabled = false`（不开 CONNECT-IP，免得它去建 TUN 网卡）；没有 `[observability]`（不开监控端口）。夹具的单元用例 `the_configuration_stays_on_the_loopback` 与 `the_configuration_is_written_as_masque_server_reads_it` 断言只有这一个监听、只听回环、各键逐字如上。启动命令是 `masque-server --config <配置文件>`，环境里的 `RUST_LOG` 去掉（用它自己的缺省日志级别）；它只开 UDP，没有 TCP 端口可探，就绪与否看日志（标准错误）里出现 `server ready`（全部监听绑定之后才打印）。策略里的服务器写成 `127.0.0.1`，rurge 不发 SNI（它的 TLS 也不看 SNI）；夹具的证书签给 `127.0.0.1`，rurge 信任夹具的 CA。

## dnsproxy 与 CoreDNS

DoQ（`quic://`）与 DoH3（`h3://`）上游有两个参照服务端（阶段 2 / M7 设计第 7.4 节），都是 Apache-2.0、三个平台都有发布包、都建在 quic-go 上：AdGuard 的 [dnsproxy](https://github.com/AdguardTeam/dnsproxy) 与 [CoreDNS](https://github.com/coredns/coredns)。dnsproxy 读到流的 FIN 才作答（rurge 写完查询即发 FIN），同一来源的第一条连接要先过地址校验（QUIC Retry，quinn 自己处理）；CoreDNS 是严格的那个——DoQ 上报文 ID 不是 0 或带 edns-tcp-keepalive 选项时它不作答、以 DOQ_PROTOCOL_ERROR 关掉整条连接（RFC 9250 4.3.3），DoH3 只在 `/dns-query` 上作答，`h3://` 不写路径时 rurge 用的正是它。互操作测试固定 dnsproxy **v0.86.0**（2026-10-01 发布）与 CoreDNS **v1.14.7**（2026-08-19 发布），CI 下载并校验以下六个发布包（SHA-256 取自 GitHub 发布 API 每个资产的 `digest`）：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (amd64) | `dnsproxy-linux-amd64-v0.86.0.tar.gz` | `17ac43f76ebfcedf547c663f6f31aa70e68b8eb6fc41d2fc37d3bfe21a2ced66` |
| Windows (amd64) | `dnsproxy-windows-amd64-v0.86.0.zip` | `dedc186ddd4b96bf92474f91d972f67da0824e0141d09e3102fd96db9f8a0dbc` |
| macOS (arm64) | `dnsproxy-darwin-arm64-v0.86.0.tar.gz` | `5964db3a45d39df4e41f74907f50657a80907c514355f9ad75b9666e3431d277` |
| Linux (amd64) | `coredns_1.14.7_linux_amd64.tgz` | `9a09356438c6d9591cb80011a2a7a52f87fc3a113f6690e41f243f98b98faac9` |
| Windows (amd64) | `coredns_1.14.7_windows_amd64.tgz` | `19756e969e4ddd16ee0c189018e5659f6695113223e590709222433f7338504d` |
| macOS (arm64) | `coredns_1.14.7_darwin_arm64.tgz` | `0005d154d49c73a88b2c56f429f35106e46c4ca37ee6de4e4d482d7de5c4b8ba` |

dnsproxy 的包里是一个 `<系统>-<架构>/` 目录，内有 `dnsproxy`（Windows 是 `dnsproxy.exe`）与 `LICENSE`、`README.md`、`config.dist.yaml`；CoreDNS 的包里只有一个 `coredns`（Windows 是 `coredns.exe`），在包的顶层（Windows 另有内容相同的 `.zip`，这里用 `.tgz`，Git Bash 的 `tar` 能解）。

`tests/dns.rs` 的四个用例——两个服务端各一个 DoQ、一个 DoH3——都问同一个名字（`rurge_interop::DNS_NAME`，A 与 AAAA 各一个文档地址，服务端从自己的配置作答、不往外转发），经同一份配置的两个解析器：一个直连（与 `rurge run` 相同）；另一个的连接器不给 QUIC 直连的 socket、只经 `connect_udp` 给载体并数它们（与 `encrypted-dns-follow-outbound-mode` 下 QUIC 走的路径相同），问两次（第二次越过缓存），这四个问题必须都在一条连接上（载体只开了一个）。每次都核对两个地址与答案的来源（`quic://127.0.0.1:<端口>`；`h3://127.0.0.1:<端口>` 不写路径，来源是 `…/dns-query`）。解析器只用共用的根证书库校验服务器的证书，没有加入测试 CA 的入口，所以配置写 `encrypted-dns-skip-cert-verification = true`（证书本身仍是夹具的 CA 签给 `127.0.0.1` 的）。

CI 在三个平台上都下载、校验并设置 `RURGE_TEST_DNSPROXY` 与 `RURGE_TEST_COREDNS`（`RURGE_INTEROP_REQUIRED=1` 对两者都生效）。本机不安装它们：`RURGE_TEST_DNSPROXY` / `RURGE_TEST_COREDNS`（优先于 `PATH` 查找）没有指向可执行文件、`PATH` 上也找不到 `dnsproxy` / `coredns`（Windows 上带 `.exe`）时打印 `skipping …` 后返回；这个 crate 不会下载或安装它们。

### dnsproxy

命令行（`rurge_interop::dnsproxy::args`）：`--listen 127.0.0.1`；`--port <端口>`（普通 DNS 的 UDP 与 TCP：不写时它要占 53；用例不用它）；`--quic-port <端口>`（DoQ）；`--https-port <端口>` 与 `--http3`（TCP 上的 DoH 与同一端口号 UDP 上的 DoH3，所以挑一个 TCP 与 UDP 都空闲的端口，普通 DNS 的端口同理）；`--tls-crt` / `--tls-key`（夹具的证书与私钥）；`--hosts-files <文件>`（`rurge_interop::dnsproxy::hosts`：`DNS_NAME` 的两个地址，名字小写）；`--upstream 127.0.0.1:9`（它必须有一个上游：回环的 discard 端口，hosts 文件答不了的问题到那里就没有下文）。就绪与否看日志（标准输出）里先后出现 `listening quic` 与 `listening to h3`。夹具的单元用例 `the_command_line_stays_on_the_loopback` 与 `the_command_line_is_written_as_dnsproxy_reads_it` 断言只听 `127.0.0.1`、唯一的上游是 discard 端口、没有 `--pprof` 之类的开关。

### CoreDNS

Corefile（`rurge_interop::coredns::render`）是两个服务块：`quic://.:<端口>`（DoQ）与 `https3://.:<端口>`（DoH3，1.13.2 起），各自 `bind 127.0.0.1`、`tls <证书> <私钥>`（路径加引号：Corefile 的引号里只转义引号，Windows 路径的反斜杠照写）与两个 `template`（`IN A` 与 `IN AAAA`，区域是 `DNS_NAME`，`answer "{{ .Name }} 60 IN A …"`）；没有 `forward`——CoreDNS 只运行 Corefile 写到的插件。启动命令是 `coredns -conf <Corefile>`；就绪与否看它启动完成后在标准输出列出的 `quic://.:<端口> on 127.0.0.1` 与 `https3://.:<端口> on 127.0.0.1` 两行。夹具的单元用例 `the_corefile_stays_on_the_loopback`、`the_corefile_is_written_as_coredns_reads_it` 与 `paths_are_quoted` 断言只有这两个服务块、各自只绑 `127.0.0.1`、没有 `forward`。

## sshd
```

`tests/interop/README.md`——把

```markdown
- 夹具渲染出的 sing-box 配置只有 `log` / `inbounds` / `outbounds` 三个顶层键（WireGuard 的配置另有 `endpoints`）；每个入站只监听 `127.0.0.1`（`hysteria2` 与 `tuic` 入站的 UDP 端口也是，同一份配置里另有一个只用来判断就绪的 `mixed` 入站；`rurge_interop::render_hysteria2` 与 `rurge_interop::render_tuic` 的单元测试 `the_hysteria2_configuration_never_touches_the_machine` 与 `the_tuic_configuration_never_touches_the_machine` 断言这些）；唯一的出站是 `direct`。WireGuard 端点在用户态运行（`system: false`），它的 UDP 端口开在所有地址上（端点没有监听地址这一项；`rurge_interop::render_wireguard` 的单元测试 `the_wireguard_configuration_never_touches_the_machine` 断言其余各项）。xray 配置同样只有这三个顶层键，唯一的出站是 `freedom`。`ssserver` 的配置只有 `servers`，每个服务端只听 `127.0.0.1`（TCP 与同号的 UDP）。`snell-server` 的配置只有 `[snell-server]` 一节，只听 `127.0.0.1`。TrustTunnel endpoint 只听 `127.0.0.1`、只开 HTTP/2，转发只有 `direct`；`allow_private_network_connections = true` 只是为了让它连回环上的 echo。tuic-server 的配置只有它认识的七个键，只听 `127.0.0.1`（它转发 UDP 的 socket 绑在所有地址上的一个空闲端口，0.8.5 没有配置项）。任何地方都不出现 `set_system_proxy`、`tun`、`auto_route` 这些键（`rurge_interop::render` 与 `rurge_interop::xray::render` 的单元测试 `the_configuration_never_touches_the_machine` 各自断言这一点）；`shadowtls` 入站的 `handshake.server` 恒为 `127.0.0.1`（夹具的单元用例断言）。
- 每个用例的连接目标都是回环 IP 字面量（`127.0.0.1` 上的 echo / 测试服务器；WireGuard 用例在隧道里连的是 sing-box 自己的隧道地址 `10.9.0.1`，由 sing-box 映射到它的 `127.0.0.1`），sing-box、xray、`ssserver`、`snell-server`、TrustTunnel endpoint 与 `tuic-server` 因此既不解析域名也不会访问公网。
- 这个 crate 本身不下载、不安装任何东西；本机是否装有 sing-box、xray、shadowsocks-rust、snell-server、TrustTunnel endpoint 或 tuic-server 由项目所有者决定，没装就跳过。
```

换成

```markdown
- 夹具渲染出的 sing-box 配置只有 `log` / `inbounds` / `outbounds` 三个顶层键（WireGuard 的配置另有 `endpoints`）；每个入站只监听 `127.0.0.1`（`hysteria2` 与 `tuic` 入站的 UDP 端口也是，同一份配置里另有一个只用来判断就绪的 `mixed` 入站；`rurge_interop::render_hysteria2` 与 `rurge_interop::render_tuic` 的单元测试 `the_hysteria2_configuration_never_touches_the_machine` 与 `the_tuic_configuration_never_touches_the_machine` 断言这些）；唯一的出站是 `direct`。WireGuard 端点在用户态运行（`system: false`），它的 UDP 端口开在所有地址上（端点没有监听地址这一项；`rurge_interop::render_wireguard` 的单元测试 `the_wireguard_configuration_never_touches_the_machine` 断言其余各项）。xray 配置同样只有这三个顶层键，唯一的出站是 `freedom`。`ssserver` 的配置只有 `servers`，每个服务端只听 `127.0.0.1`（TCP 与同号的 UDP）。`snell-server` 的配置只有 `[snell-server]` 一节，只听 `127.0.0.1`。TrustTunnel endpoint 只听 `127.0.0.1`、只开一种协议（HTTP/2，或 QUIC），转发只有 `direct`；`allow_private_network_connections = true` 只是为了让它连回环上的 echo。tuic-server 的配置只有它认识的七个键，只听 `127.0.0.1`（它转发 UDP 的 socket 绑在所有地址上的一个空闲端口，0.8.5 没有配置项）。masque-server 只有一个 HTTP/3 监听、只听 `127.0.0.1`，不开监控端口、不开 CONNECT-IP（不建 TUN 网卡）；它的目标策略放开到任何地址，只是为了让它连回环上的 echo。dnsproxy 的全部监听只在 `127.0.0.1`，唯一的上游是回环的 discard 端口；CoreDNS 的两个服务块只绑 `127.0.0.1`，没有 `forward`。任何地方都不出现 `set_system_proxy`、`tun`、`auto_route` 这些键（`rurge_interop::render` 与 `rurge_interop::xray::render` 的单元测试 `the_configuration_never_touches_the_machine` 各自断言这一点）；`shadowtls` 入站的 `handshake.server` 恒为 `127.0.0.1`（夹具的单元用例断言）。
- 每个用例的连接目标都是回环 IP 字面量（`127.0.0.1` 上的 echo / 测试服务器；WireGuard 用例在隧道里连的是 sing-box 自己的隧道地址 `10.9.0.1`，由 sing-box 映射到它的 `127.0.0.1`），sing-box、xray、`ssserver`、`snell-server`、TrustTunnel endpoint、`tuic-server` 与 masque-server 因此既不解析域名也不会访问公网；dnsproxy 与 CoreDNS 被问的只有它们自己配置里的那个名字，从配置作答，不往外转发。
- 这个 crate 本身不下载、不安装任何东西；本机是否装有 sing-box、xray、shadowsocks-rust、snell-server、TrustTunnel endpoint、tuic-server、masque-server、dnsproxy 或 CoreDNS 由项目所有者决定，没装就跳过。
```

兼容性清单（2.1 `encrypted-dns-server`、`PROTOCOL` 规则、4.2 的 `masque` / `trust-tunnel` 行、4.3 `tfo` / `underlying-proxy`、4.4 `alpn` 与 Shadow TLS 约束、4.5 UDP、4.6 参数行、DNS 的 `h3://` / `quic://` 与跟随出站模式、附录统计）：

`docs/surge-compatibility-matrix.md`——把

```markdown
| `encrypted-dns-server` | URL 列表：`https://` `h3://` `quic://` `tls://` `tcp://` | 全部 | 🟡 | 1 / 2 | `https` `tls` `tcp` 阶段 1；`h3` `quic` 依赖 QUIC 栈，阶段 2；阶段 1 对 `h3` / `quic` 条目告警 W0026 并忽略 |
| `encrypted-dns-follow-outbound-mode` | 布尔；默认 false | 全部 | 🟡 | 1 | 含"代理服务器为域名时回退 DIRECT 并告警"的防环逻辑；M3b：TCP/DoT/DoH 上游连接走流水线（成 Internal 会话，`PROTOCOL,DOH/DOT/DNS` 可匹配）；上游主机名由 Bootstrap 解析，流水线只见 IP 目标，故域名规则不匹配上游主机名；协议标签按端口启发（853→DoT，443→DoH，其余→DNS）；被规则 REJECT 时告警并直连以保 DNS；UDP 上游不经连接器；这类内部会话的 `SRC-IP` 恒为 `127.0.0.1`、`IN-PORT` 恒为 `0`，`SRC-IP,127.0.0.1/32` / `IN-PORT,0` 规则可能意外匹配到它们，且它们的 `kill` 是空操作（DNS 路径不监听取消令牌）；防环回退自阶段 2 / M1b 起实现：DNS 会话命中的代理（沿 underlying-proxy 找到真正打开 socket 的那一跳；底下是 DIRECT——direct 别名策略或当前选中 DIRECT 的组——时取它上面那一跳，因为 DIRECT 要在本机解析的正是那一跳的服务器名）若以域名配置，则告警并直连；以 IP 配置的代理照常承载 DNS 会话；阶段 2 / M3b 起，链底下是 REJECT 的代理（中继是当前选中 REJECT 的组等）同样告警并直连，不让 DNS 查询失败；开了 `encrypted-dns-follow-outbound-mode` 时，DNS 会话路由到的 `wireguard` 策略只要有一个 peer 的 endpoint 写成域名，就与以域名配置的代理一样告警并改走直连（防环：启动隧道先要解析那个域名；把 endpoint 写成地址即可避免） |
```

换成

```markdown
| `encrypted-dns-server` | URL 列表：`https://` `h3://` `quic://` `tls://` `tcp://` | 全部 | ✅ | 1 / 2 | `https` `tls` `tcp` 阶段 1；`h3`（DoH3）与 `quic`（DoQ）阶段 2 / M7c 起实现（缺省端口 443 / 853，`h3://` 没写路径时用 `/dns-query`；细节见 6.2 节），阶段 1 对它们告警 W0026 并忽略的做法随之取消；W0026 只剩解析不了的 URL（如带路径的 `quic://`、带用户名口令的 `h3://`）与建不出客户端的传输 |
| `encrypted-dns-follow-outbound-mode` | 布尔；默认 false | 全部 | 🟡 | 1 | 含"代理服务器为域名时回退 DIRECT 并告警"的防环逻辑；M3b：TCP/DoT/DoH 上游连接走流水线（成 Internal 会话，`PROTOCOL,DOH/DOT/DNS` 可匹配）；上游主机名由 Bootstrap 解析，流水线只见 IP 目标，故域名规则不匹配上游主机名；协议标签按端口启发（853→DoT，443→DoH，其余→DNS）；被规则 REJECT 时告警并直连以保 DNS；UDP 上游不经连接器；这类内部会话的 `SRC-IP` 恒为 `127.0.0.1`、`IN-PORT` 恒为 `0`，`SRC-IP,127.0.0.1/32` / `IN-PORT,0` 规则可能意外匹配到它们，且它们的 `kill` 是空操作（DNS 路径不监听取消令牌）；防环回退自阶段 2 / M1b 起实现：DNS 会话命中的代理（沿 underlying-proxy 找到真正打开 socket 的那一跳；底下是 DIRECT——direct 别名策略或当前选中 DIRECT 的组——时取它上面那一跳，因为 DIRECT 要在本机解析的正是那一跳的服务器名）若以域名配置，则告警并直连；以 IP 配置的代理照常承载 DNS 会话；阶段 2 / M3b 起，链底下是 REJECT 的代理（中继是当前选中 REJECT 的组等）同样告警并直连，不让 DNS 查询失败；开了 `encrypted-dns-follow-outbound-mode` 时，DNS 会话路由到的 `wireguard` 策略只要有一个 peer 的 endpoint 写成域名，就与以域名配置的代理一样告警并改走直连（防环：启动隧道先要解析那个域名；把 endpoint 写成地址即可避免）；阶段 2 / M7c 起 DoQ / DoH3 上游的 QUIC 载体同样走流水线，见 6.2 节同名一行 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `PROTOCOL` | `HTTP` `HTTPS` `TCP` `UDP` `QUIC` `STUN` `MTProto` `DOH` `DOH3` `DOQ` `DOT` `DNS`；区分大小写；`TCP` 覆盖 HTTP/HTTPS/MTProto，`UDP` 覆盖 QUIC/STUN | 全部 | 🟡 | 1 / 3 | M5a：`UDP` 匹配每条 UDP 流、`TCP` 匹配每个 TCP 会话（按传输层判断）；UDP 流只识别 QUIC（目标 UDP 443 上的 Initial 包），STUN 与 DNS 的嗅探未做（`PROTOCOL,STUN` 与 `PROTOCOL,DNS` 匹配不到客户端的 UDP 流，`PROTOCOL,UDP` 能）；`DOH*` `DOQ` `DOT` `DNS` 只匹配 rurge 自身发出的 DNS 请求且需 `encrypted-dns-follow-outbound-mode=true`；`MTProto` 依赖阶段 7；M3b：DoT/DoH/DNS 标签按上游端口启发（853/443/其余）；基于 SNI 的路由与 `PROTOCOL,HTTPS` 的 dial 前匹配随阶段 4 |
```

换成

```markdown
| `PROTOCOL` | `HTTP` `HTTPS` `TCP` `UDP` `QUIC` `STUN` `MTProto` `DOH` `DOH3` `DOQ` `DOT` `DNS`；区分大小写；`TCP` 覆盖 HTTP/HTTPS/MTProto，`UDP` 覆盖 QUIC/STUN | 全部 | 🟡 | 1 / 3 | M5a：`UDP` 匹配每条 UDP 流、`TCP` 匹配每个 TCP 会话（按传输层判断）；UDP 流只识别 QUIC（目标 UDP 443 上的 Initial 包），STUN 与 DNS 的嗅探未做（`PROTOCOL,STUN` 与 `PROTOCOL,DNS` 匹配不到客户端的 UDP 流，`PROTOCOL,UDP` 能）；`DOH*` `DOQ` `DOT` `DNS` 只匹配 rurge 自身发出的 DNS 请求且需 `encrypted-dns-follow-outbound-mode=true`；`MTProto` 依赖阶段 7；M3b：DoT/DoH/DNS 标签按上游端口启发（853/443/其余）；M7c：DoQ / DoH3 上游的 QUIC 载体是 UDP 会话（`PROTOCOL,UDP` 同样匹配），标签也按端口启发——443 是 `DOH3`，其余是 `DOQ`（不在 443 的 DoH3 服务器因此标成 `DOQ`）；基于 SNI 的路由与 `PROTOCOL,HTTPS` 的 dial 前匹配随阶段 4 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `masque` | MASQUE（HTTP/3 CONNECT + CONNECT-UDP，RFC 9298 / 9297） | iOS 5.22 / Mac 6.9+ | ✅ | 2 | |
| `anytls` | AnyTLS v2 | iOS 5.17 / Mac 6.4.3+ | ✅ | 2 | M2b（阶段 2）已实现（TCP）：一条会话同一时刻只承载一个流（与参考实现一致）；空闲超过 60 秒的会话每 30 秒回收一次；不等 `cmdSYNACK` 就返回流，被拒的流在第一次读上以 `anytls: <服务端文本>` 失败；连接超时只约束到 TCP 连接、TLS 握手、鉴权写出与首个会话层包（`cmdSettings ‖ cmdSYN ‖ cmdPSH`）**入队**为止，该包的物理写出由会话自己的任务完成，此后的卡顿由转发阶段的空闲超时兜底；**没有半关闭**：客户端方向的 EOF 以 `cmdFIN` 结束整条流（与 sing-box 一致）；不实现参考客户端的 3 秒 SYNACK 看门狗；`cmdHeartRequest` 一律不等写队列腾出空间就应答，队列满时应答直接丢弃（繁忙的写端本身就是存活证明）——如果服务端靠未应答的心跳计数判断连接已死，一个正在满负荷写的会话可能因此被这样的服务端误判为已死并关闭；服务端推送的 padding 方案做有界校验（原文 ≤ 8192 字节、`stop` ≤ 256、每包 ≤ 64 项、每项 1 ..= 16384），不合法就保留旧方案；`password` 只接受命名写法；默认不带 ALPN（未与真实 Surge 核对）；UDP（M5b，阶段 2）：UDP over TCP v2（sing 的 `uot`）——一条流的目标是 `sp.v2.udp-over-tcp.arpa:0`，请求非连接模式、写第一个包的目标，每个包带自己的地址（类型号 0 / 1 / 2）与长度；全锥；这条流占一个会话；目标主机名的字母表规则同 http / socks5 |
| `trust-tunnel` | Trust Tunnel（AdGuard，HTTP/2 或 HTTP/3） | Mac 6.4.4+ | 🟡 | 2 | M6c（阶段 2）已实现 TCP（HTTP/2）。HTTP/2 会话池（两种协议共用）：每个策略一个池，池里是若干条 TLS + HTTP/2 连接（可叠 Shadow TLS 与 `underlying-proxy`）；开流时选最早的一条正在承载的流少于上限、没收到 GOAWAY 的连接，没有就新建（在途的每次拨号算作 `max-streams` 个位置——拨号的请求自己一个、等它的请求其余几个——放不下的请求再拨一条，同时至多 8 条在途，所以同时到来的一批请求一起拨出所需的连接；请求等着的拨号都失败时，失败一并报给它们，不退避）；上限是 `max-streams`（缺省 3）与服务端 `SETTINGS_MAX_CONCURRENT_STREAMS` 中较小者，流数由 rurge 自己计（不靠 h2 的排队）；新连接在收到服务端的 SETTINGS 之前只承载一条流（连上后发一个 PING，回来即视为 SETTINGS 已知），同时进来的其它请求等它定下来再决定共用还是另开（它已承载 `max-streams` 条流时不等、直接另开）；这个 PING 5 秒内没有回应时连接不再分配新流（TLS 完成后、SETTINGS 到达前静默断掉的连接由此被换掉）；没有流的连接空闲 60 秒关闭（每 30 秒检查一次）；收到 GOAWAY 或出错的连接不再分配新流，现有流走完；请求没得到应答头（调用方超时放弃或流出错；服务端答了任何状态码都不算）时用 PING 检查那条连接，5 秒内没有回应就不再分配新流、下一个请求新建连接（休眠唤醒、换网后静默死掉的连接由此被换掉；只是慢的目标不受影响）；每条流的接收窗口 1 MiB、连接窗口为 `max-streams` × 1 MiB（至少 4 MiB：读端停住的流占不光整条连接的窗口；TrustTunnel 文档建议 131072，rurge 取更大的值，服务端都接受）。流即字节流：写按对方的流控窗口分段，读到数据即释放流控容量，本方向结束发 END_STREAM（真正的半关闭），丢弃未结束的流发 RST_STREAM(CANCEL)；整个建立过程（取连接、拨号、TLS 与 HTTP/2 握手、CONNECT 应答）受连接超时约束。**ALPN 固定 `h2`**：写了别的 `alpn` 时 `W0028` 并忽略；服务端不参与 ALPN 或选了别的协议时失败 `<协议>: the server does not speak HTTP/2`；服务端（如 rustls）坚持另一种 ALPN 时握手本身失败，看到的是 TLS 层的 `tls: received fatal alert: NoApplicationProtocol`。`headers` 里 HTTP/2 禁止的连接专用字段（`connection` `keep-alive` `proxy-connection` `transfer-encoding` `upgrade` `te`）加载时各 `W0028` 并去掉，`host` 也一样（`:authority` 就是 CONNECT 的目标，与它不同的 `Host` 让请求不合格，RFC 9113 8.3.1）；`headers` 里的字段替换 rurge 自己生成的同名字段（`proxy-authorization`、`user-agent`，不区分大小写），每个字段只发一个；`<random-string>` 占位按每个 CONNECT 请求（每条流）各自生成。目标写进 `:authority`（`host:port`，IPv6 带方括号，名字转成 A-label、交给服务端解析；写不出的名字不拨号即失败 `<协议>: the host name cannot be sent to the server`）。错误文本与日志不含凭据与 `headers` 的值。请求是标准的 HTTP/2 CONNECT：`proxy-authorization: Basic`（`username` / `password` 只读命名写法、必填，缺了是 `E0018`，不引用取值）与 `user-agent`——rurge 发固定的 `rurge`（协议文档写作必填、格式为 `<平台> <应用名>`，TrustTunnel endpoint 的实现并不检查；Surge 发什么未核对），`headers` 写了 `User-Agent` 时用它的值。2xx 后流即隧道；**407 是 `trust-tunnel: authentication failed`**，其它状态（endpoint 连不上目标时的 502、endpoint 把认证失败的状态码配成 405 / 404 / 403 时也在此列）是 `trust-tunnel: the server answered <状态码>`。endpoint 按 SNI 精确选主机：服务器写成 IP 时要写 `sni=<主机名>`。**`h3=true` 解析并 `W0029`（每次加载一条），M7 之前仍按 HTTP/2 连**；写了 `udp-relay` 时 `W0028` 并忽略。**不载 UDP**：`udp()` 为不支持，由 `udp-policy-not-supported-behaviour` 处理；不实现 TrustTunnel 的 `_udp2`（UDP 复用）、`_icmp` 与 `_check` 伪主机；测速按 URL 测速。互操作参照：TrustTunnel endpoint v1.1.0（Linux / macOS）。 |
```

换成

```markdown
| `masque` | MASQUE（HTTP/3 CONNECT + CONNECT-UDP，RFC 9298 / 9297） | iOS 5.22 / Mac 6.9+ | ✅ | 2 | M7c（阶段 2）已实现 TCP 与 UDP。**连接**与 `hysteria2` 相同（见那一行的「连接」）：每个策略一条 QUIC 连接，多路复用——每个 TCP 隧道一条 HTTP/3 请求流，UDP 每个目标一条 CONNECT-UDP 流、包走同一条连接的 HTTP/3 数据报；第一次用到时才拨号，同时到来的请求共用一次拨号与握手、失败一并报给等着的请求（不退避）；只用 TLS 1.3、不用 0-RTT，空闲超时 30 秒、保活 10 秒，没有流也没有 UDP 载体满 60 秒的连接主动关闭；收到 HTTP/3 GOAWAY 的连接不再接新请求，其上已有的隧道走完；刚取用的连接已被服务端丢弃时在新连接上重试一次；`underlying-proxy` 是组时跟随组的当前成员（日志 `masque: the underlying policy changed; a new connection is made`）。服务端的 QUIC 流数上限满了时，新隧道等有流释放（受连接超时约束），不另开连接（手册：多路复用在一条 QUIC 连接上）。HTTP/3 是 rurge 自写的客户端（M7-D5）：宣告 `H3_DATAGRAM`（QUIC 的数据报传输参数随之通告），跳过服务端的 GREASE（保留的设置、保留类型的单向流、应答头之前的保留帧）。**认证**：写了 `username` / `password` 时，**每个 CONNECT（TCP 隧道与 CONNECT-UDP 都是）主动带 `proxy-authorization: Basic base64(用户名:口令)`**（标准字母表、带填充，QPACK 不入索引），不等 407 质询；手册只说凭据"以 HTTP Basic 认证随每个 CONNECT 请求发送"、没写字段名，用 `proxy-authorization` 是 rurge 的选择（与 masque-server 一致）；不写凭据时不发这个字段。**TCP**：经典 CONNECT（RFC 9114 4.4：只有 `:method CONNECT` 与 `:authority host:port`，没有 `:scheme` / `:path` / `:protocol`；不用仍是草案的 `connect-tcp`），目标写进 `:authority`（IPv6 带方括号，名字转成 A-label、交给服务端解析；写不出的名字不拨号即失败 `masque: the host name cannot be sent to the server`）；不发 `user-agent`；2xx 后流即隧道（应答里的 `content-length` 不看），**407 是 `masque: proxy authentication required`**，其它状态 `masque: the proxy answered <状态码>`；整个建立过程（拨号、握手、等应答）受连接超时约束（`connect timed out`），QUIC 与 HTTP/3 层的错误原样（`quic: …`、`h3: …`）。**UDP**（自动，不需要参数）：CONNECT-UDP（RFC 9298），每个目标一条 extended CONNECT 流（`:protocol connect-udp`、`:scheme https`；`:authority` 是服务器自己——写了 `sni=` 时用那个名字，与 TLS 一致——加策略的端口，443 也写；`:path /.well-known/masque/udp/<主机>/<端口>/`，即 RFC 9298 的缺省模板（手册没有模板参数），IPv6 不带方括号、`:` 写作 `%3A`，名字转成 A-label；`capsule-protocol: ?1` 与凭据），因此是**对称型 NAT**（同 `h2-connect` 的 UDP）。打开载体不拨号：每个目标的流随发往它的第一个包打开（同一目标同时到来的包共用一次打开），**收到 2xx 之后才发包**（不乐观发送；据 masque-server 的说明 Surge 在请求头之后立即发第一个包，登记的差异，只多一个往返）；2xx 带 `content-length` / `content-type` / `transfer-encoding` 或是 204 / 205 / 206 时不成立（`masque: the proxy's response to CONNECT-UDP is malformed`，流以 H3_REQUEST_CANCELLED 取消）。包走 HTTP/3 数据报（QUIC DATAGRAM 帧：四分之一流号、context id 0、负载）；收的时候也接受流上的 DATAGRAM capsule（context id 0），其它 context id 与其它类型的 capsule 丢弃，畸形的 capsule 结束那条流（`masque: a malformed capsule (…)`），下一个包重开一条。**放不下的包丢弃、不退回 DATAGRAM capsule**（RFC 9297 3.5 与 RFC 9298 6.1 都让放不下的数据报被丢弃；masque-server 在 HTTP/3 上也不读 capsule；取代 M7 设计 7.1 的 capsule 退路，登记的差异）：超过 65527 字节的包是 `masque: a datagram longer than 65527 bytes`（什么流都不开），超过这条路径上一个数据报此刻装得下的负载（QUIC 初始的 MTU 下约 1150 字节，路径 MTU 探测之后更大，回环上 1412 字节；经 `underlying-proxy` 的载体不做探测）是 `masque: a datagram longer than <n> bytes`——都只丢那一个，流照常。**服务端须通告 extended CONNECT（`SETTINGS_ENABLE_CONNECT_PROTOCOL=1`）与 `H3_DATAGRAM=1`，连接还须协商了 QUIC 数据报**：缺一样时发往每个目标的第一个包以 `masque: the server does not support CONNECT-UDP` 失败（请求不发出），**同一连接上的 TCP 照常**——手册说 Surge 在建立连接时校验，rurge 只让 UDP 失败（登记的差异）；被拒的 CONNECT-UDP 与 TCP 同文本（407 是 `masque: proxy authentication required`，其它状态 `masque: the proxy answered <状态码>`）。每个目标的流空闲 60 秒（两个方向的包都算）后以 FIN 体面关闭，下一个包重开一条；服务端结束的流同样让出位置，刚打开就被结束时每个包至多重开一次，仍被结束时那个包失败（`masque: the server ended the UDP stream`）；连接关闭、收到 GOAWAY 或底层策略变了时，目标的流在会话当前的连接上重开（载体至多每秒比较一次底层策略的去处）。每个载体至多排 64 个回包等客户端来读，再多的丢弃；载体关闭时它的流都以 H3_REQUEST_CANCELLED 取消。错误文本、日志与 `Debug` 不含凭据与载荷。测速经出站本身（URL 测速的两次 `HEAD` 在同一条隧道里，`test-udp` 经 CONNECT-UDP）；重载时参数没变的策略沿用出站与它的 QUIC 连接（凭据、`ecn`、`port-hopping`、`sni` 变了就重建）。参数见 4.6；不能叠 Shadow TLS（4.4）。互操作参照：masque-server v0.13.0（只有 Linux 版，只在 Linux CI） |
| `anytls` | AnyTLS v2 | iOS 5.17 / Mac 6.4.3+ | ✅ | 2 | M2b（阶段 2）已实现（TCP）：一条会话同一时刻只承载一个流（与参考实现一致）；空闲超过 60 秒的会话每 30 秒回收一次；不等 `cmdSYNACK` 就返回流，被拒的流在第一次读上以 `anytls: <服务端文本>` 失败；连接超时只约束到 TCP 连接、TLS 握手、鉴权写出与首个会话层包（`cmdSettings ‖ cmdSYN ‖ cmdPSH`）**入队**为止，该包的物理写出由会话自己的任务完成，此后的卡顿由转发阶段的空闲超时兜底；**没有半关闭**：客户端方向的 EOF 以 `cmdFIN` 结束整条流（与 sing-box 一致）；不实现参考客户端的 3 秒 SYNACK 看门狗；`cmdHeartRequest` 一律不等写队列腾出空间就应答，队列满时应答直接丢弃（繁忙的写端本身就是存活证明）——如果服务端靠未应答的心跳计数判断连接已死，一个正在满负荷写的会话可能因此被这样的服务端误判为已死并关闭；服务端推送的 padding 方案做有界校验（原文 ≤ 8192 字节、`stop` ≤ 256、每包 ≤ 64 项、每项 1 ..= 16384），不合法就保留旧方案；`password` 只接受命名写法；默认不带 ALPN（未与真实 Surge 核对）；UDP（M5b，阶段 2）：UDP over TCP v2（sing 的 `uot`）——一条流的目标是 `sp.v2.udp-over-tcp.arpa:0`，请求非连接模式、写第一个包的目标，每个包带自己的地址（类型号 0 / 1 / 2）与长度；全锥；这条流占一个会话；目标主机名的字母表规则同 http / socks5 |
| `trust-tunnel` | Trust Tunnel（AdGuard，HTTP/2 或 HTTP/3） | Mac 6.4.4+ | ✅ | 2 | M6c（阶段 2）已实现 TCP（HTTP/2），M7c 起 `h3=true` 走 HTTP/3（见本行后半）。HTTP/2 会话池（两种协议共用）：每个策略一个池，池里是若干条 TLS + HTTP/2 连接（可叠 Shadow TLS 与 `underlying-proxy`）；开流时选最早的一条正在承载的流少于上限、没收到 GOAWAY 的连接，没有就新建（在途的每次拨号算作 `max-streams` 个位置——拨号的请求自己一个、等它的请求其余几个——放不下的请求再拨一条，同时至多 8 条在途，所以同时到来的一批请求一起拨出所需的连接；请求等着的拨号都失败时，失败一并报给它们，不退避）；上限是 `max-streams`（缺省 3）与服务端 `SETTINGS_MAX_CONCURRENT_STREAMS` 中较小者，流数由 rurge 自己计（不靠 h2 的排队）；新连接在收到服务端的 SETTINGS 之前只承载一条流（连上后发一个 PING，回来即视为 SETTINGS 已知），同时进来的其它请求等它定下来再决定共用还是另开（它已承载 `max-streams` 条流时不等、直接另开）；这个 PING 5 秒内没有回应时连接不再分配新流（TLS 完成后、SETTINGS 到达前静默断掉的连接由此被换掉）；没有流的连接空闲 60 秒关闭（每 30 秒检查一次）；收到 GOAWAY 或出错的连接不再分配新流，现有流走完；请求没得到应答头（调用方超时放弃或流出错；服务端答了任何状态码都不算）时用 PING 检查那条连接，5 秒内没有回应就不再分配新流、下一个请求新建连接（休眠唤醒、换网后静默死掉的连接由此被换掉；只是慢的目标不受影响）；每条流的接收窗口 1 MiB、连接窗口为 `max-streams` × 1 MiB（至少 4 MiB：读端停住的流占不光整条连接的窗口；TrustTunnel 文档建议 131072，rurge 取更大的值，服务端都接受）。流即字节流：写按对方的流控窗口分段，读到数据即释放流控容量，本方向结束发 END_STREAM（真正的半关闭），丢弃未结束的流发 RST_STREAM(CANCEL)；整个建立过程（取连接、拨号、TLS 与 HTTP/2 握手、CONNECT 应答）受连接超时约束。**ALPN 固定 `h2`**：写了别的 `alpn` 时 `W0028` 并忽略；服务端不参与 ALPN 或选了别的协议时失败 `<协议>: the server does not speak HTTP/2`；服务端（如 rustls）坚持另一种 ALPN 时握手本身失败，看到的是 TLS 层的 `tls: received fatal alert: NoApplicationProtocol`。`headers` 里 HTTP/2 禁止的连接专用字段（`connection` `keep-alive` `proxy-connection` `transfer-encoding` `upgrade` `te`）加载时各 `W0028` 并去掉，`host` 也一样（`:authority` 就是 CONNECT 的目标，与它不同的 `Host` 让请求不合格，RFC 9113 8.3.1）；`headers` 里的字段替换 rurge 自己生成的同名字段（`proxy-authorization`、`user-agent`，不区分大小写），每个字段只发一个；`<random-string>` 占位按每个 CONNECT 请求（每条流）各自生成。目标写进 `:authority`（`host:port`，IPv6 带方括号，名字转成 A-label、交给服务端解析；写不出的名字不拨号即失败 `<协议>: the host name cannot be sent to the server`）。错误文本与日志不含凭据与 `headers` 的值。请求是标准的 HTTP/2 CONNECT：`proxy-authorization: Basic`（`username` / `password` 只读命名写法、必填，缺了是 `E0018`，不引用取值）与 `user-agent`——rurge 发固定的 `rurge`（协议文档写作必填、格式为 `<平台> <应用名>`，TrustTunnel endpoint 的实现并不检查；Surge 发什么未核对），`headers` 写了 `User-Agent` 时用它的值。2xx 后流即隧道；**407 是 `trust-tunnel: authentication failed`**，其它状态（endpoint 连不上目标时的 502、endpoint 把认证失败的状态码配成 405 / 404 / 403 时也在此列）是 `trust-tunnel: the server answered <状态码>`。endpoint 按 SNI 精确选主机：服务器写成 IP 时要写 `sni=<主机名>`。写了 `udp-relay` 时 `W0028` 并忽略。**`h3=true`（M7c）**：隧道改走 QUIC 上的 HTTP/3（rurge 自写的客户端，不宣告 HTTP Datagram），不经上面的 HTTP/2 会话池；请求与 HTTP/2 相同（经典 CONNECT 只有 `:method` 与 `:authority`，`proxy-authorization`、`user-agent: rurge` 与 `headers` 的覆盖规则都一样，同样不能发的名字不拨号即失败），错误文本也相同（407 是 `trust-tunnel: authentication failed`，其它状态 `trust-tunnel: the server answered <状态码>`）。连接照 QUIC 的会话（见 `hysteria2` 一行的「连接」：只用 TLS 1.3、空闲超时 30 秒与保活 10 秒、没有流满 60 秒的连接关闭、GOAWAY 之后不再接新请求、刚取用的连接已失效时重试一次、跟随底层策略的当前成员），**`max-streams` 是每条 QUIC 连接同时承载的隧道数**：满了照 HTTP/2 会话池的规则另拨（最早的、有空位的连接优先；在途的每次拨号算作 `max-streams` 个位置，同时至多 8 条在途；等着一次拨号的请求一并得到它的失败），失效连接的那次重试落在另一条有空位的连接上（有的话）；**不看服务端的 QUIC 流数上限**（quinn 不公开对端的上限：它比 `max-streams` 小时，多出的隧道在那条连接上等流释放，受连接超时约束；HTTP/2 池取二者中较小的；登记的差异；TrustTunnel endpoint 的缺省是 4096）。ALPN 固定 `h3`；`headers` 里 HTTP/3 禁止的字段同样去掉，告警写 HTTP/3；`tfo` 是 `W0028`，`ecn` 生效；不能叠 Shadow TLS（`E0018`）；有 `underlying-proxy` 时 QUIC 经底层策略的 UDP 载体（见 4.3）；没有端口跳跃（参数见 4.6）。**没有 HTTP/3 → HTTP/2 的自动回落**：TrustTunnel 的协议文档允许客户端先试 HTTP/3、过一会儿改试 HTTP/2，手册只说"服务端须支持 HTTP/3 传输"，rurge 写了 `h3=true` 就只走 HTTP/3（登记的差异）。重载时参数没变的策略沿用出站与它的连接（口令、`headers`、`max-streams`、`ecn` 或 `h3` 变了就重建）。**不载 UDP**（两种传输都是）：`udp()` 为不支持，由 `udp-policy-not-supported-behaviour` 处理；不实现 TrustTunnel 的 `_udp2`（UDP 复用）、`_icmp` 与 `_check` 伪主机；测速按 URL 测速。互操作参照：TrustTunnel endpoint v1.1.0（Linux / macOS；`h3=true` 对它的 QUIC 监听）。 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| 策略指向 rurge 尚未实现的协议类型 | Surge 原生支持全部协议 | 全部 | 🟡 | 1 / 2 | 加载时告警 `W0007`；运行时该策略按 `REJECT` 处理，会话日志 `error = policy protocol not implemented: <type>`；随阶段 2 各里程碑逐协议移除：M1 已移除 `http` `https` `socks5` `socks5-tls`；M2a 已移除 `trojan`；M2b 已移除 `vmess`（写了 `vmess-aead=true` 的行）与 `anytls`；M4a 已移除 `ssh`；M4b 已移除 `wireguard`；M4c 已移除 `external`；M6a 已移除 `ss`（流式旧方法除外）；M6b 已移除 `snell`（v4 / v5）；M6c 已移除 `h2-connect` 与 `trust-tunnel`（HTTP/2；`h3=true` 是 `W0029`，不是 `W0007`）；M7a 已移除 `hysteria2`；M7b 已移除 `tuic` 与 `tuic-v5`（此前格式不对的这两种行只报 `W0007`，现在是加载错误 `E0018`）。`W0007` 与 `OutboundError::Unsupported` 在 M8 退役（M7 细化设计 M7-D12：M7 之后仍有 `tailscale`、ss 流式旧方法、vmess 旧握手与 Snell v1–v3 / v6）。例外有三个：没写 `vmess-aead=true` 的 `vmess` 行仍按 `W0007` 处理，但走专门的诊断文本 `` `vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet ``（每次加载一条，不是每行一条）与专门的会话日志文本 `policy protocol not implemented: vmess (legacy handshake)`；流式旧方法的 `ss` 行同样如此，诊断文本 `` `ss` stream cipher `<method>` is not implemented yet; such policies behave as REJECT ``（每种方法每次加载一条），会话日志 `policy protocol not implemented: ss (<method>)`；`version` 为 1–3 或 6（含没写 `version` 的，Surge 的缺省是 1）的 `snell` 行也是，诊断文本 `` `snell` version <n> is not implemented; rurge supports versions 4 and 5 (`version` must match the server); such policies behave as REJECT ``（每个版本每次加载一条），会话日志 `policy protocol not implemented: snell v<n>`；三者都不是这里的通用 `<type>` 模板 |
```

换成

```markdown
| 策略指向 rurge 尚未实现的协议类型 | Surge 原生支持全部协议 | 全部 | 🟡 | 1 / 2 | 加载时告警 `W0007`；运行时该策略按 `REJECT` 处理，会话日志 `error = policy protocol not implemented: <type>`；随阶段 2 各里程碑逐协议移除：M1 已移除 `http` `https` `socks5` `socks5-tls`；M2a 已移除 `trojan`；M2b 已移除 `vmess`（写了 `vmess-aead=true` 的行）与 `anytls`；M4a 已移除 `ssh`；M4b 已移除 `wireguard`；M4c 已移除 `external`；M6a 已移除 `ss`（流式旧方法除外）；M6b 已移除 `snell`（v4 / v5）；M6c 已移除 `h2-connect` 与 `trust-tunnel`（HTTP/2；`h3=true` 当时是 `W0029`，不是 `W0007`）；M7a 已移除 `hysteria2`；M7b 已移除 `tuic` 与 `tuic-v5`（此前格式不对的这两种行只报 `W0007`，现在是加载错误 `E0018`）；M7c 已移除 `masque`（格式不对的 `masque` 行同样从只报 `W0007` 变成加载错误 `E0018`），`trust-tunnel` 的 `h3=true` 从此生效、不再是 `W0029`。`W0007` 与 `OutboundError::Unsupported` 在 M8 退役（M7 细化设计 M7-D12：M7 之后仍有 `tailscale`、ss 流式旧方法、vmess 旧握手与 Snell v1–v3 / v6）。例外有三个：没写 `vmess-aead=true` 的 `vmess` 行仍按 `W0007` 处理，但走专门的诊断文本 `` `vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet ``（每次加载一条，不是每行一条）与专门的会话日志文本 `policy protocol not implemented: vmess (legacy handshake)`；流式旧方法的 `ss` 行同样如此，诊断文本 `` `ss` stream cipher `<method>` is not implemented yet; such policies behave as REJECT ``（每种方法每次加载一条），会话日志 `policy protocol not implemented: ss (<method>)`；`version` 为 1–3 或 6（含没写 `version` 的，Surge 的缺省是 1）的 `snell` 行也是，诊断文本 `` `snell` version <n> is not implemented; rurge supports versions 4 and 5 (`version` must match the server); such policies behave as REJECT ``（每个版本每次加载一条），会话日志 `policy protocol not implemented: snell v<n>`；三者都不是这里的通用 `<type>` 模板 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `tfo` | 布尔；默认 false | 🟡 | 2 | 解析并校验，`W0029`，三平台都不生效：`socket2` 0.6 没有 TFO 的安全封装，不为此引入 unsafe；有安全封装后再评估。QUIC 类（M7a 起 `hysteria2`，M7b 起 `tuic` / `tuic-v5`）不开 TCP 连接，写了是 `W0028` 并忽略 |
| `tos` | 0–255 或 `0x` 十六进制；默认 0 | 🟡 | 2 | M1 已实现；Windows 上对 IPv6 不生效；有 underlying-proxy 时无效（socket 选项属于真正打开 socket 的那一跳）；M2a 起加载时报 W0028。QUIC 类（M7a 起 `hysteria2`，M7b 起 `tuic` / `tuic-v5`）的直连 socket 同样带上它，代价是发出的包没有 ECN 标记、没有 GSO（见 `ecn` 一行） |
| `ecn` | `auto` `on` `off`；QUIC 类协议默认开启，WireGuard/Tailscale 默认关闭 | 🟡 | 2 | M1 解析并校验取值。M7a 起在 `hysteria2` 上、M7b 起在 `tuic` / `tuic-v5` 上生效（M7-D4，跟随 quinn）：`auto` 与 `on` 都取 quinn 的缺省——给发出的包标 ECT(0)，对端的 ACK 不带 ECN 计数或计数校验失败时自行停用；quinn 没有"强制开启"，**`on` 等同 `auto`**；`off` 在 UDP 层去掉发出与收到两侧的 ECN 标记，quinn 的 ECN 校验随之停用。**写了 `tos`（非 0）时**直连的 QUIC socket 发送不经 quinn-udp（它在 Linux / macOS 上给每个包写的 TOS 只有 ECN 两位，会把 DSCP 清零），所以**发出的包没有 ECN 标记、也没有 GSO**；接收照常经 quinn-udp（读对端的 ECN 标记，照常做路径 MTU 探测）；三个平台一样。经 `underlying-proxy` 的载体没有 ECN。TCP 类协议与 `wireguard` 上仍只解析（`W0029`；手册说它对纯 TCP 协议不起作用） |
```

换成

```markdown
| `tfo` | 布尔；默认 false | 🟡 | 2 | 解析并校验，`W0029`，三平台都不生效：`socket2` 0.6 没有 TFO 的安全封装，不为此引入 unsafe；有安全封装后再评估。QUIC 类（M7a 起 `hysteria2`，M7b 起 `tuic` / `tuic-v5`，M7c 起 `masque` 与 `h3=true` 的 `trust-tunnel`）不开 TCP 连接，写了是 `W0028` 并忽略 |
| `tos` | 0–255 或 `0x` 十六进制；默认 0 | 🟡 | 2 | M1 已实现；Windows 上对 IPv6 不生效；有 underlying-proxy 时无效（socket 选项属于真正打开 socket 的那一跳）；M2a 起加载时报 W0028。QUIC 类（M7a 起 `hysteria2`，M7b 起 `tuic` / `tuic-v5`，M7c 起 `masque` 与 `h3=true` 的 `trust-tunnel`）的直连 socket 同样带上它，代价是发出的包没有 ECN 标记、没有 GSO（见 `ecn` 一行） |
| `ecn` | `auto` `on` `off`；QUIC 类协议默认开启，WireGuard/Tailscale 默认关闭 | 🟡 | 2 | M1 解析并校验取值。M7a 起在 `hysteria2` 上、M7b 起在 `tuic` / `tuic-v5` 上、M7c 起在 `masque` 与 `h3=true` 的 `trust-tunnel` 上生效（M7-D4，跟随 quinn；手册没说 `trust-tunnel` 算不算"QUIC 类"，rurge 按它的传输算）：`auto` 与 `on` 都取 quinn 的缺省——给发出的包标 ECT(0)，对端的 ACK 不带 ECN 计数或计数校验失败时自行停用；quinn 没有"强制开启"，**`on` 等同 `auto`**；`off` 在 UDP 层去掉发出与收到两侧的 ECN 标记，quinn 的 ECN 校验随之停用。**写了 `tos`（非 0）时**直连的 QUIC socket 发送不经 quinn-udp（它在 Linux / macOS 上给每个包写的 TOS 只有 ECN 两位，会把 DSCP 清零），所以**发出的包没有 ECN 标记、也没有 GSO**；接收照常经 quinn-udp（读对端的 ECN 标记，照常做路径 MTU 探测）；三个平台一样。经 `underlying-proxy` 的载体没有 ECN。TCP 类协议与 `wireguard` 上仍只解析（`W0029`；手册说它对纯 TCP 协议不起作用） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `underlying-proxy` | 另一策略或策略组名；仅代理策略；不能与 `port-hopping` 同用 | ✅ | 2 | 目标代理主机名在上游远程解析；M1 已实现（TCP）：底层可以是策略或组，按名字在拨号时解析，组的选择变了链的入口随之变；成环是 `E0019`；链上某一跳失败时错误文本带 `via <名字>:` 前缀；中继是没有成员的组（订阅还没下载到、过滤滤光了）时，经它的连接一律失败（`via <组名>: policy group has no members`），不回退 DIRECT，也不看 `--empty-group-reject`（阶段 2 / M3b：设中继就是不让流量直出；Surge 未说明）。QUIC 类（M7a 起 `hysteria2`，M7b 起 `tuic` / `tuic-v5`）经底层策略的 UDP 载体连服务器（底层策略不载 UDP 时拨号失败，同 M5c；这种载体不做路径 MTU 探测，QUIC 包停在 1200 字节，也没有 ECN）；与 `port-hopping` 同写是 `E0018`（`` `port-hopping` cannot be combined with `underlying-proxy` ``；写 `underlying-proxy=DIRECT` 不算链，写组名算）；组级 `underlying-proxy` 派生的 `M (via R)` 里，成员 `M` 写了 `port-hopping` 时这个派生成员构建失败、从组里略去（日志 `policy cannot be built; it is left out`，同 M3a 里其它构建不了的派生成员），组照常用其余成员；配置本身给组的这类成员（行上写的、`include-other-group` 与 `include-all-proxies` 取来的）在加载时各报一条 `W0023`（`` policy group `G` relays its members through `underlying-proxy`: `M` uses `port-hopping`, which cannot go through it; `M (via R)` is left out ``，`rurge check` 可见），订阅带来的加载时不知道、只记日志；成员全被略去时组为空，与其它空组一样由 DIRECT 代替，`--empty-group-reject` 时 REJECT（见 5.1 节「嵌套与循环」） |
```

换成

```markdown
| `underlying-proxy` | 另一策略或策略组名；仅代理策略；不能与 `port-hopping` 同用 | ✅ | 2 | 目标代理主机名在上游远程解析；M1 已实现（TCP）：底层可以是策略或组，按名字在拨号时解析，组的选择变了链的入口随之变；成环是 `E0019`；链上某一跳失败时错误文本带 `via <名字>:` 前缀；中继是没有成员的组（订阅还没下载到、过滤滤光了）时，经它的连接一律失败（`via <组名>: policy group has no members`），不回退 DIRECT，也不看 `--empty-group-reject`（阶段 2 / M3b：设中继就是不让流量直出；Surge 未说明）。QUIC 类（M7a 起 `hysteria2`，M7b 起 `tuic` / `tuic-v5`，M7c 起 `masque` 与 `h3=true` 的 `trust-tunnel`）经底层策略的 UDP 载体连服务器（底层策略不载 UDP 时拨号失败，同 M5c；这种载体不做路径 MTU 探测，QUIC 包停在 1200 字节，也没有 ECN）；与 `port-hopping` 同写是 `E0018`（`` `port-hopping` cannot be combined with `underlying-proxy` ``；写 `underlying-proxy=DIRECT` 不算链，写组名算）；组级 `underlying-proxy` 派生的 `M (via R)` 里，成员 `M` 写了 `port-hopping` 时这个派生成员构建失败、从组里略去（日志 `policy cannot be built; it is left out`，同 M3a 里其它构建不了的派生成员），组照常用其余成员；配置本身给组的这类成员（行上写的、`include-other-group` 与 `include-all-proxies` 取来的）在加载时各报一条 `W0023`（`` policy group `G` relays its members through `underlying-proxy`: `M` uses `port-hopping`, which cannot go through it; `M (via R)` is left out ``，`rurge check` 可见），订阅带来的加载时不知道、只记日志；成员全被略去时组为空，与其它空组一样由 DIRECT 代替，`--empty-group-reject` 时 REJECT（见 5.1 节「嵌套与循环」） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `alpn` | 协议列表；TUIC / Hysteria 2 / MASQUE 默认 `h3` | ✅ | 2 | M1 已实现：`https` / `socks5-tls` 握手不带 ALPN，除非写了 `alpn`（未与真实 Surge 核对）；M6c：`h2-connect` / `trust-tunnel` 固定 `h2`，写了别的取值 `W0028` 并忽略；M7a：`hysteria2` 没写 `alpn` 时发 `h3`（照手册），写了就发所写的列表；QUIC 的握手只用 TLS 1.3；服务端不选 ALPN 时握手失败——rustls 一类的服务端没有相同的协议时直接拒绝（`tls: the server refused the handshake (NoApplicationProtocol)`），完成握手却不选协议的服务端同样被拒（登记的差异）；M7b：`tuic` / `tuic-v5` 同样没写时发 `h3`；TUIC 官方的服务端（0.8.5 与 tuic-server 1.x）缺省不配 ALPN、也就不选协议，这样的服务端 rurge 连不上，要在服务端配上 `h3`（手册说 `alpn` 须与服务端的设置相符） |
| `client-cert` | `[Keystore]` 条目名（p12） | ✅ | 2 | 双向 TLS；M1 已实现 |
| `shadow-tls-password` | 字符串；设置即启用 Shadow TLS | ✅ | 2 | M2c 已实现：所有 TCP 类出站（`http` `https` `socks5` `socks5-tls` `trojan` `vmess` `anytls`，M4a 起还有 `ssh`，M6c 起还有 `h2-connect` `trust-tunnel`）；顺序是 connect → shadow-tls → tls → ws → 协议。伪装握手照常校验证书（出站的根证书库），**不受本节其余六个 TLS 参数影响**（它们只作用于里层的真实 TLS）；伪装握手不带 ALPN。口令为空 `E0018`；没有口令时另外两个参数各报一条 `W0028` |
| `shadow-tls-sni` | 主机名；v3 必填 | ✅ | 2 | M2c 已实现：必须是 DNS 名（IDN 写成 `xn--` 形式；IP 字面量 `E0018`）；v3 缺它 `E0018`。v2 不写时**不发 SNI**（手册），证书按策略自己的 TLS 会用的名字校验——`sni` 写了名字用它，否则用服务器主机名；所以服务器写成 IP、伪装站点是别人的网站时必须写它，否则握手以 `shadow-tls: the camouflage handshake failed: …` 失败。`server-cert-verify-name` 刻意不参与这次校验——它命名的是代理自己的证书，不是伪装站点的证书 |
| `shadow-tls-version` | `2` / `3`；默认 2 | ✅ | 2 | M2c 已实现；其它取值 `E0018`。**v3**：用 stock rustls 两遍构造 ClientHello（SessionID 后 4 字节是对 ClientHello 自身的 HMAC）；伪装握手只提供 X25519 一个密钥交换组、关闭会话恢复；要求伪装站点支持 TLS 1.3（否则 `shadow-tls: the handshake server does not support TLS 1.3`）；服务端没有通过验证（口令不对，或对端根本不是 Shadow TLS v3 服务端）时，像普通访客那样经真实会话向伪装站点发一个 HTTP 请求、读完应答后离开，再以 `shadow-tls: the server did not authenticate itself` 失败（多出的时间以 2 秒为限）。**v2**：口令错误在握手期分辨不出，表现为第一次读取时的 `shadow-tls: the handshake server closed the session`；服务端靠"首帧到达时自己已转发的字节"比对摘要，首帧越晚越可能失败——rurge 的首帧在握手完成后立即发出，只有不带 TLS 的 `vmess` 会等最多 100 ms（请求头的合并窗口）。**两个版本**：每帧负载 ≤ 16384 字节，读取接受 16 位长度字段能表示的任何长度（sing-box 的帧可以大过一条 TLS 记录）；收到的 alert 记录跳过，流的结束以 TCP 连接结束为准（参考客户端把 alert 当作读方向的结束；sing-box 服务端对半关闭回 alert 之后仍可能继续发数据，照那样做会截断下行）；rurge 自己从不发 alert 记录 |
| 约束：Shadow TLS 不能与 TUIC / WireGuard / Tailscale / 其他 QUIC 类协议组合 | 配置错误 | ✅ | 2 | 判断函数已落地（`tuic` `tuic-v5` `hysteria2` `masque` `wireguard` `tailscale`：`` E0018 Shadow TLS cannot be combined with a `<type>` policy ``）；`wireguard` 自 M4b 起、`hysteria2` 自 M7a 起、`tuic` / `tuic-v5` 自 M7b 起真的报出，`masque` 的 spec 出现之前（M7c）与 `tailscale` 的 spec 出现之前（远期）该错误不会真的报出——它们的整行目前都还没有被读取；`h3=true` 的 `trust-tunnel` 眼下仍按 HTTP/2 连、照常可叠 Shadow TLS，M7c 让它走 QUIC 时须把它加进判断函数 |
```

换成

```markdown
| `alpn` | 协议列表；TUIC / Hysteria 2 / MASQUE 默认 `h3` | ✅ | 2 | M1 已实现：`https` / `socks5-tls` 握手不带 ALPN，除非写了 `alpn`（未与真实 Surge 核对）；M6c：`h2-connect` / `trust-tunnel` 固定 `h2`，写了别的取值 `W0028` 并忽略；M7a：`hysteria2` 没写 `alpn` 时发 `h3`（照手册），写了就发所写的列表；QUIC 的握手只用 TLS 1.3；服务端不选 ALPN 时握手失败——rustls 一类的服务端没有相同的协议时直接拒绝（`tls: the server refused the handshake (NoApplicationProtocol)`），完成握手却不选协议的服务端同样被拒（登记的差异）；M7b：`tuic` / `tuic-v5` 同样没写时发 `h3`；TUIC 官方的服务端（0.8.5 与 tuic-server 1.x）缺省不配 ALPN、也就不选协议，这样的服务端 rurge 连不上，要在服务端配上 `h3`（手册说 `alpn` 须与服务端的设置相符）；M7c：`masque` 同样没写时发 `h3`、写了就发所写的列表（照手册）；`trust-tunnel` 的 ALPN 按传输固定——没有 `h3` 时 `h2`，`h3=true` 时 `h3`——写了别的取值 `W0028` 并忽略，文本随之分成两条（`` `alpn` is always `h2` on `trust-tunnel` policies without `h3=true`; ignored `` 与 `` `alpn` is always `h3` on `trust-tunnel` policies with `h3=true`; ignored ``；`h2-connect` 的不变） |
| `client-cert` | `[Keystore]` 条目名（p12） | ✅ | 2 | 双向 TLS；M1 已实现 |
| `shadow-tls-password` | 字符串；设置即启用 Shadow TLS | ✅ | 2 | M2c 已实现：所有 TCP 类出站（`http` `https` `socks5` `socks5-tls` `trojan` `vmess` `anytls`，M4a 起还有 `ssh`，M6c 起还有 `h2-connect` `trust-tunnel`——`trust-tunnel` 只在 HTTP/2 上，`h3=true` 时是 `E0018`，见本节末尾的约束一行）；顺序是 connect → shadow-tls → tls → ws → 协议。伪装握手照常校验证书（出站的根证书库），**不受本节其余六个 TLS 参数影响**（它们只作用于里层的真实 TLS）；伪装握手不带 ALPN。口令为空 `E0018`；没有口令时另外两个参数各报一条 `W0028` |
| `shadow-tls-sni` | 主机名；v3 必填 | ✅ | 2 | M2c 已实现：必须是 DNS 名（IDN 写成 `xn--` 形式；IP 字面量 `E0018`）；v3 缺它 `E0018`。v2 不写时**不发 SNI**（手册），证书按策略自己的 TLS 会用的名字校验——`sni` 写了名字用它，否则用服务器主机名；所以服务器写成 IP、伪装站点是别人的网站时必须写它，否则握手以 `shadow-tls: the camouflage handshake failed: …` 失败。`server-cert-verify-name` 刻意不参与这次校验——它命名的是代理自己的证书，不是伪装站点的证书 |
| `shadow-tls-version` | `2` / `3`；默认 2 | ✅ | 2 | M2c 已实现；其它取值 `E0018`。**v3**：用 stock rustls 两遍构造 ClientHello（SessionID 后 4 字节是对 ClientHello 自身的 HMAC）；伪装握手只提供 X25519 一个密钥交换组、关闭会话恢复；要求伪装站点支持 TLS 1.3（否则 `shadow-tls: the handshake server does not support TLS 1.3`）；服务端没有通过验证（口令不对，或对端根本不是 Shadow TLS v3 服务端）时，像普通访客那样经真实会话向伪装站点发一个 HTTP 请求、读完应答后离开，再以 `shadow-tls: the server did not authenticate itself` 失败（多出的时间以 2 秒为限）。**v2**：口令错误在握手期分辨不出，表现为第一次读取时的 `shadow-tls: the handshake server closed the session`；服务端靠"首帧到达时自己已转发的字节"比对摘要，首帧越晚越可能失败——rurge 的首帧在握手完成后立即发出，只有不带 TLS 的 `vmess` 会等最多 100 ms（请求头的合并窗口）。**两个版本**：每帧负载 ≤ 16384 字节，读取接受 16 位长度字段能表示的任何长度（sing-box 的帧可以大过一条 TLS 记录）；收到的 alert 记录跳过，流的结束以 TCP 连接结束为准（参考客户端把 alert 当作读方向的结束；sing-box 服务端对半关闭回 alert 之后仍可能继续发数据，照那样做会截断下行）；rurge 自己从不发 alert 记录 |
| 约束：Shadow TLS 不能与 TUIC / WireGuard / Tailscale / 其他 QUIC 类协议组合 | 配置错误 | ✅ | 2 | 判断函数已落地（`tuic` `tuic-v5` `hysteria2` `masque` `wireguard` `tailscale`：`` E0018 Shadow TLS cannot be combined with a `<type>` policy ``）；`wireguard` 自 M4b 起、`hysteria2` 自 M7a 起、`tuic` / `tuic-v5` 自 M7b 起、`masque` 自 M7c 起真的报出，`tailscale` 的 spec 出现之前（远期）该错误不会真的报出——它的整行目前还没有被读取；`trust-tunnel` 按 `h3` 判断（M7c）：没有 `h3` 时照常可叠（HTTP/2 跑在 TCP 上），`h3=true` 时是 `` E0018 Shadow TLS cannot be combined with `h3=true` on a `trust-tunnel` policy ``——这一行只报这一条，三个 Shadow TLS 参数各自的问题（如口令为空、v3 缺 `shadow-tls-sni`）不再另报，按类型拒绝的那几种同样如此 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `udp-relay`（布尔；默认 false） | 适用 SOCKS5 / SOCKS5-TLS / Shadowsocks / External / HTTP/2 CONNECT（RFC 9298） | ✅ M5a：`socks5` / `socks5-tls` / `external` 已生效；M6a：Shadowsocks 已生效；M6c：HTTP/2 CONNECT（`h2-connect` 的 CONNECT-UDP，每个目标一条流，对称型；服务端须通告 extended CONNECT，见 4.2 `h2-connect` 一行）已生效；写在 `trust-tunnel` 行上是 `W0028` 并忽略；写在 `hysteria2` 行上同样是 `W0028`（UDP 自动支持，M7a），写在 `tuic` / `tuic-v5` 行上也是（M7b） | 2 |
| `udp-port`（端口；默认主端口） | 适用 Shadowsocks / Snell | ✅ M6a：Shadowsocks 已生效（1–65535，其它取值 `E0018`；没写 `udp-relay=true` 时不用）；M6b：Snell 只解析——写了是 `W0029`，UDP over TCP 一律走主端口（手册用它指服务端另开的 UDP 端口，即 v5 的 QUIC Proxy Mode，rurge 不实现；见 4.2 `snell` 一行） | 2 |
| 自动支持 UDP 的协议：Snell v3+、VMess、Trojan、TUIC、Hysteria 2、MASQUE、AnyTLS（UDP over TCP）、WireGuard、Tailscale | | ✅ M5b：VMess（对称型）、Trojan、AnyTLS 已生效；M5c：WireGuard 已生效；M6b：Snell v4 / v5 已生效（UDP over TCP，全锥；v1–v3 在 M8 之前不实现）；M7a：Hysteria 2 已生效（QUIC 数据报，全锥；服务端在认证时回 `Hysteria-UDP: false` 时打开载体即失败，见 4.2 `hysteria2` 一行）；M7b：TUIC v5 / v4 已生效（全锥；服务端接受 QUIC 数据报时走数据报，否则每个包一条单向流，自动选择；v4 放不进一个数据报的包丢弃，见 4.2 `tuic` / `tuic-v5` 一行）；其余随各自的协议 | 2 |
| 不支持 UDP 的协议：HTTP / HTTPS、Trust Tunnel、SSH | 受 `udp-policy-not-supported-behaviour` 控制 | ✅ M5a 已实现；`underlying-proxy` 的底层策略不支持 UDP 时，经它的 UDP 流失败并写 `via <底层策略>: the underlying policy cannot carry UDP` | 2 |
```

换成

```markdown
| `udp-relay`（布尔；默认 false） | 适用 SOCKS5 / SOCKS5-TLS / Shadowsocks / External / HTTP/2 CONNECT（RFC 9298） | ✅ M5a：`socks5` / `socks5-tls` / `external` 已生效；M6a：Shadowsocks 已生效；M6c：HTTP/2 CONNECT（`h2-connect` 的 CONNECT-UDP，每个目标一条流，对称型；服务端须通告 extended CONNECT，见 4.2 `h2-connect` 一行）已生效；写在 `trust-tunnel` 行上是 `W0028` 并忽略；写在 `hysteria2` 行上同样是 `W0028`（UDP 自动支持，M7a），写在 `tuic` / `tuic-v5` 行上也是（M7b），写在 `masque` 行上也是（M7c：`` `udp-relay` does not apply to `masque` policies (UDP is always relayed); ignored ``） | 2 |
| `udp-port`（端口；默认主端口） | 适用 Shadowsocks / Snell | ✅ M6a：Shadowsocks 已生效（1–65535，其它取值 `E0018`；没写 `udp-relay=true` 时不用）；M6b：Snell 只解析——写了是 `W0029`，UDP over TCP 一律走主端口（手册用它指服务端另开的 UDP 端口，即 v5 的 QUIC Proxy Mode，rurge 不实现；见 4.2 `snell` 一行） | 2 |
| 自动支持 UDP 的协议：Snell v3+、VMess、Trojan、TUIC、Hysteria 2、MASQUE、AnyTLS（UDP over TCP）、WireGuard、Tailscale | | ✅ M5b：VMess（对称型）、Trojan、AnyTLS 已生效；M5c：WireGuard 已生效；M6b：Snell v4 / v5 已生效（UDP over TCP，全锥；v1–v3 在 M8 之前不实现）；M7a：Hysteria 2 已生效（QUIC 数据报，全锥；服务端在认证时回 `Hysteria-UDP: false` 时打开载体即失败，见 4.2 `hysteria2` 一行）；M7b：TUIC v5 / v4 已生效（全锥；服务端接受 QUIC 数据报时走数据报，否则每个包一条单向流，自动选择；v4 放不进一个数据报的包丢弃，见 4.2 `tuic` / `tuic-v5` 一行）；M7c：MASQUE 已生效（CONNECT-UDP，每个目标一条流、对称型，包走 HTTP/3 数据报，放不下的包丢弃；服务端不支持 CONNECT-UDP 时只有 UDP 失败，见 4.2 `masque` 一行）；其余随各自的协议 | 2 |
| 不支持 UDP 的协议：HTTP / HTTPS、Trust Tunnel（`h3=true` 也是）、SSH | 受 `udp-policy-not-supported-behaviour` 控制 | ✅ M5a 已实现；`underlying-proxy` 的底层策略不支持 UDP 时，经它的 UDP 流失败并写 `via <底层策略>: the underlying policy cannot carry UDP` | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `masque` | `username` `password`（HTTP Basic）`port-hopping` `port-hopping-interval` | ✅ | 2 | 服务器须声明 extended CONNECT 与 HTTP Datagram，连接时校验 |
| `anytls` | `password` `reuse`（默认 true） | ✅ | 2 | 自动 UDP（udp-over-tcp v2，M5b 起生效）；`password` 只接受命名写法；`reuse=false` 时每个流各自新建会话、用完即关，不进池 |
| `trust-tunnel` | `username` `password` `headers` `max-streams`（默认 3）`h3`（默认 false） | 🟡 | 2 | M6c 已实现（HTTP/2）。`username` / `password` 只读命名写法、必填（`E0018`，不引用取值；写在端口之后的位置值是 `W0001`）；`headers` 与 `max-streams` 同 `h2-connect`（`headers` 可覆盖 rurge 的 `user-agent: rurge` 与 `proxy-authorization`）；TLS 参数适用，`alpn` 固定 `h2`；**`h3=true` 解析并 `W0029`，M7 之前按 HTTP/2 连**；`udp-relay` 是 `W0028`；无 UDP |
```

换成

```markdown
| `masque` | `username` `password`（HTTP Basic）`port-hopping` `port-hopping-interval` | ✅ | 2 | M7c 已实现。凭据可选、只读命名写法：`username` 与 `password` 要么都写、要么都不写——只写一个是 `E0018`（`` `password` is required with `username` `` / `` `username` is required with `password` ``），写了却为空是 `E0018`（`` `username` is empty `` / `` `password` is empty ``），都不引用取值；写在端口之后的位置值是 `W0001`，不当凭据。写了凭据就以 `proxy-authorization: Basic` 随每个 CONNECT 发出（见 4.2）。`port-hopping` / `port-hopping-interval` 的写法、诊断码与行为同 `hysteria2`（小于 5 秒 `W0012` 并按 5，没写 `port-hopping` 时 `W0028`；与 `underlying-proxy` 同写是 `E0018`；组级 `underlying-proxy` 的组里写了它的成员被略去、加载时 `W0023`，见 4.3）。手册没给 `masque` 列 `headers` 与 `max-streams`，写了按未知参数 `W0001`。`udp-relay` 与 `tfo` 写了是 `W0028` 并忽略（UDP 自动支持；QUIC 不开 TCP 连接）；`ecn` 生效（见 4.3）；TLS 参数适用（4.4，`alpn` 缺省 `h3`）；不能叠 Shadow TLS（`E0018`）。服务器须通告 extended CONNECT 与 HTTP Datagram：rurge 在每个目标的 CONNECT-UDP 流打开时核对，不满足时只让 UDP 失败（手册说连接时校验；见 4.2）。`username` 与 `password` 在 `profiles/current` 与 `policies/detail` 里脱敏（位置值也抹掉） |
| `anytls` | `password` `reuse`（默认 true） | ✅ | 2 | 自动 UDP（udp-over-tcp v2，M5b 起生效）；`password` 只接受命名写法；`reuse=false` 时每个流各自新建会话、用完即关，不进池 |
| `trust-tunnel` | `username` `password` `headers` `max-streams`（默认 3）`h3`（默认 false） | ✅ | 2 | M6c 已实现（HTTP/2），M7c 起 `h3=true` 走 HTTP/3（见 4.2）。`username` / `password` 只读命名写法、必填（`E0018`，不引用取值；写在端口之后的位置值是 `W0001`）；`headers` 与 `max-streams` 同 `h2-connect`（`headers` 可覆盖 rurge 的 `user-agent: rurge` 与 `proxy-authorization`）；TLS 参数适用，`alpn` 按传输固定（`h2`；`h3=true` 时 `h3`）。`h3` 是布尔（`h3=maybe` 之类是 `E0018`）；`h3=true` 时：`max-streams` 是每条 QUIC 连接的隧道数，`headers` 里 HTTP/3 禁止的字段（同 HTTP/2 的那几个）各 `W0028` 并去掉、文本写 HTTP/3（`` header `<名字>` in `headers` is not allowed in HTTP/3; ignored ``，`host` 是 `` header `host` in `headers` is not allowed in HTTP/3 (`:authority` names the target); ignored ``），`tfo` 是 `W0028`（`` `tfo` does not apply to `trust-tunnel` policies with `h3=true`; ignored ``），`ecn` 生效（见 4.3），不能叠 Shadow TLS（`E0018`，见 4.4）。手册没给 `trust-tunnel` 列 `port-hopping`：`port-hopping` 与 `port-hopping-interval` 在两种传输上都是 `W0028` 并忽略（`` `port-hopping` does not apply to `trust-tunnel` policies; ignored ``；M7c 之前按未知参数 `W0001`）；`udp-relay` 是 `W0028`；无 UDP |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `h3://`（DoH3，443）`quic://`（DoQ，853） | | ✅ | 2 | 依赖 QUIC 栈 |
```

换成

```markdown
| `h3://`（DoH3，443）`quic://`（DoQ，853） | | ✅ | 2 | M7c（阶段 2）已实现，`encrypted-dns-server` 与 `[Host]` 的 `server:` 都可用。**共同**：每个上游一条 QUIC 连接，由它的查询共用——第一个需要它的查询才拨号，同时到来的查询等这一次拨号、一并得到它的失败；只用 TLS 1.3、不用 0-RTT；证书照常用共用的根证书库校验，`encrypted-dns-skip-cert-verification` 时不校验；服务器写成名字时由传统 DNS 引导解析（引导豁免照旧），地址逐个试（IPv4 优先）直到连上；**不保活**：闲着的连接按两端空闲超时中较短的那个（rurge 是 30 秒）自行关闭，下一个查询重新拨号；连接已关闭或正在离开（HTTP/3 GOAWAY）时下一个查询换新连接；查询到来时已建好的连接在查询中途失效（例如服务端空闲时丢了它、回 stateless reset）时，在新连接上重问一次——DNS 查询可以重问，查询的数据已经发出也一样；查询的时限包住拨号、握手与交换；网络变化时上游重建、旧连接随之关闭。错误进 DNS 日志的形式同 DoH / DoT（`io: …`、`tls: …`、`http: …`、`bad response: …`、`timeout`）。**DoQ**（`quic://host[:port]`，RFC 9250）：ALPN 只发 `doq`（不发草案的 `doq-i0x`）；每个查询一条新的双向流，写出"2 字节长度 + 报文"后立即 FIN；报文 ID 置 0，去掉 edns-tcp-keepalive 选项，并加 EDNS(0) Padding 补到 128 字节的倍数（RFC 9250 5.4 的 MUST；rurge 看过的客户端——dnsproxy、hickory、sing-box、mihomo——都没做）；应答是流上到结尾为止的一个"长度 + 报文"，其 ID 必须是 0，交回调用方时换回原 ID。应答 ID 不是 0（`the server's reply has message ID <n>, not 0`）、流在应答中途结束（`the stream ended within the server's reply`）、应答之后还有字节（`the server sent more than its reply on the stream`）都是服务端的协议错误：查询失败（`bad response: …`），连接以 DOQ_PROTOCOL_ERROR（0x2）关闭；报文短于 DNS 头只算坏应答，连接照常。服务端重置或停止查询的流只让这个查询失败（`the server reset the query's stream (<码名>)` / `the server stopped the query's stream (<码名>)`；后者 RFC 9250 4.3.3 列为协议错误，rurge 宽松处理），应答里的 edns-tcp-keepalive 不检查，服务端开的单向流不拒；放弃的查询（时限到了）以 DOQ_REQUEST_CANCELLED（0x3）停止它的流。`quic://` 只写主机与端口，结尾单独一个 `/` 照收，带路径、查询串或片段的是 `W0026`（`` `<url>`: a quic:// URL names a host and a port, nothing else ``）。**DoH3**（`h3://host[:port][/path]`）：RFC 8484 的 POST，跑在 rurge 自写的 HTTP/3 客户端上（ALPN `h3`，不宣告 HTTP Datagram），`content-type` 与 `accept` 都是 `application/dns-message`，带 `content-length`；报文 ID 置 0、不加 padding；**没写路径时用 `/dns-query`**（RFC 8484 没定缺省路径，sing-box、hickory 与 CoreDNS 都用它，CoreDNS 的 DoH3 只在这个路径上作答；手册没写，Surge 的取法未核对），查询串照发、片段去掉；`:authority` 是 URL 的主机（端口是 443 时不写）。应答状态须是 200（否则 `http: status <状态>`）、`content-type` 须以 `application/dns-message` 开头（否则 `bad response: content-type …`），与 `content-length` 不符的是 HTTP/3 层的错误（流错误，连接照常）。`h3://` 带用户名口令的是 `W0026`（文本里的 URL 已去掉它们）。上游的名字写作 `quic://<主机>:<端口>`（IPv6 带方括号）与 `h3://<主机>[:<端口>]<路径>`。互操作参照：AdGuard dnsproxy v0.86.0 与 CoreDNS v1.14.7（三平台） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `encrypted-dns-follow-outbound-mode`：DNS 连接走规则；`PROTOCOL,DOH/DOH3/DOQ/DOT/DNS` 可匹配；命中的代理若以域名配置则告警并回退 DIRECT | 默认 false | 🟡 | 1 | M3b：TCP/DoT/DoH 上游走流水线（Internal 会话，`PROTOCOL` 可匹配）；上游主机名先由 Bootstrap 解析，流水线只见 IP 目标，域名规则不匹配上游主机名；协议标签按端口启发（853→DoT，443→DoH，其余→DNS）；被 REJECT 时告警并直连保底；UDP 上游不经连接器；这类会话的 `SRC-IP`/`IN-PORT` 为占位值（`127.0.0.1:0`/`0`），`kill` 对其无效；防环回退自阶段 2 / M1b 起实现：DNS 会话命中的代理（沿 underlying-proxy 找到真正打开 socket 的那一跳；底下是 DIRECT——direct 别名策略或当前选中 DIRECT 的组——时取它上面那一跳，因为 DIRECT 要在本机解析的正是那一跳的服务器名）若以域名配置，则告警并直连；以 IP 配置的代理照常承载 DNS 会话；阶段 2 / M3b 起，链底下是 REJECT 的代理（中继是当前选中 REJECT 的组等）同样告警并直连，不让 DNS 查询失败；开了 `encrypted-dns-follow-outbound-mode` 时，DNS 会话路由到的 `wireguard` 策略只要有一个 peer 的 endpoint 写成域名，就与以域名配置的代理一样告警并改走直连（防环：启动隧道先要解析那个域名；把 endpoint 写成地址即可避免） |
| `[Host]` 中 `server:<加密 URL>` 按域名指定加密 DNS | iOS 5.21 / Mac 6.8+ | ✅ | 1 | |
```

换成

```markdown
| `encrypted-dns-follow-outbound-mode`：DNS 连接走规则；`PROTOCOL,DOH/DOH3/DOQ/DOT/DNS` 可匹配；命中的代理若以域名配置则告警并回退 DIRECT | 默认 false | 🟡 | 1 | M3b：TCP/DoT/DoH 上游走流水线（Internal 会话，`PROTOCOL` 可匹配）；上游主机名先由 Bootstrap 解析，流水线只见 IP 目标，域名规则不匹配上游主机名；协议标签按端口启发（853→DoT，443→DoH，其余→DNS）；被 REJECT 时告警并直连保底；UDP 上游不经连接器；这类会话的 `SRC-IP`/`IN-PORT` 为占位值（`127.0.0.1:0`/`0`），`kill` 对其无效；防环回退自阶段 2 / M1b 起实现：DNS 会话命中的代理（沿 underlying-proxy 找到真正打开 socket 的那一跳；底下是 DIRECT——direct 别名策略或当前选中 DIRECT 的组——时取它上面那一跳，因为 DIRECT 要在本机解析的正是那一跳的服务器名）若以域名配置，则告警并直连；以 IP 配置的代理照常承载 DNS 会话；阶段 2 / M3b 起，链底下是 REJECT 的代理（中继是当前选中 REJECT 的组等）同样告警并直连，不让 DNS 查询失败；开了 `encrypted-dns-follow-outbound-mode` 时，DNS 会话路由到的 `wireguard` 策略只要有一个 peer 的 endpoint 写成域名，就与以域名配置的代理一样告警并改走直连（防环：启动隧道先要解析那个域名；把 endpoint 写成地址即可避免）；阶段 2 / M7c 起 DoQ / DoH3 上游的 QUIC 载体同样走流水线：每个载体是一个内部 UDP 会话（请求记录的 `transport` 是 UDP），目标是服务器的地址（名字已先由 Bootstrap 解析），协议标签按端口启发（443→`DOH3`，其余→`DOQ`，不在 443 的 DoH3 服务器因此标成 `DOQ`；`PROTOCOL,UDP` 也匹配它们）；规则选中 DIRECT 时用 DIRECT 策略自己的 UDP（与 TCP 的 DIRECT 一样带 `interface` / `tos` 等设置），选中载 UDP 的代理时经它的 UDP 载体（如 `udp-relay=true` 的 `socks5`、`masque`、`hysteria2`）；**选中不载 UDP 的代理（`http`、`trust-tunnel`、`ssh` 等）时这个上游这次失败**——请求记录写 `policy does not support UDP`，解析器拿到 `via <策略>: policy does not support UDP`，不改走直连（`udp-policy-not-supported-behaviour` 不适用），其它上游照常作答（手册没说这种情形）；链底是 REJECT 与以域名配置的代理照 TCP 告警并直连；载体打开失败交给解析器的错误都带 `via <策略>: ` 前缀。这样的会话随 QUIC 连接存续（连接关闭之后、下一个查询之前仍显示为活动，同 DoT），`kill` 同样无效；`smart` 组的回报同 TCP（服务器对握手立即回应，3 秒没有回包即计为失败）。没开这个选项时 QUIC 载体照旧直连、不过规则 |
| `[Host]` 中 `server:<加密 URL>` 按域名指定加密 DNS | iOS 5.21 / Mac 6.8+ | ✅ | 1 | M7c 起 `quic://` 与 `h3://` 也可用（引导豁免同样覆盖它们） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| 2. `[General]` 选项 | 60 | 43 | 8 | 9 | 0 | 0 |
| 3. 规则系统 | 61 | 47 | 10 | 3 | 1 | 0 |
| 4. 出站策略 | 85 | 76 | 6 | 1 | 0 | 2 |
```

换成

```markdown
| 2. `[General]` 选项 | 60 | 44 | 7 | 9 | 0 | 0 |
| 3. 规则系统 | 61 | 47 | 10 | 3 | 1 | 0 |
| 4. 出站策略 | 85 | 78 | 4 | 1 | 0 | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| **合计** | **529** | **424** | **59** | **29** | **10** | **7** |
```

换成

```markdown
| **合计** | **529** | **427** | **56** | **29** | **10** | **7** |
```

`docs/acceptance/phase2-manual.md`——把

```markdown
- [ ] `trust-tunnel` 不载 UDP：经 SOCKS5 UDP 发 DNS 查询，`udp-policy-not-supported-behaviour` 缺省（`REJECT`）时查询没有回答，会话记录写 `policy does not support UDP`；改成 `DIRECT` 后查询直连得到回答。写 `h3=true` 时 `rurge check` 报一条 `W0029`，运行时仍按 HTTP/2 连通。
```

换成

```markdown
- [ ] `trust-tunnel` 不载 UDP：经 SOCKS5 UDP 发 DNS 查询，`udp-policy-not-supported-behaviour` 缺省（`REJECT`）时查询没有回答，会话记录写 `policy does not support UDP`；改成 `DIRECT` 后查询直连得到回答。`h3=true`（HTTP/3）的各项见 M7c 一节。
```

`docs/acceptance/phase2-manual.md`——把

```markdown
- [ ] 脱敏：`GET /v1/policies/detail?policy_name=T5`（与 `T4`）与 `GET /v1/profiles/current` 里 `uuid`、`password` 与 `token` 都是 `***`。

```

换成

```markdown
- [ ] 脱敏：`GET /v1/policies/detail?policy_name=T5`（与 `T4`）与 `GET /v1/profiles/current` 里 `uuid`、`password` 与 `token` 都是 `***`。

## M7c　HTTP/3 族与 DoQ / DoH3

前置：同 M5a 一节的 SOCKS5 UDP 客户端；自己的 MASQUE 节点（要求用户名口令；记下服务端实现与版本，如 masque-server），最好再有一个 Surge 实际在用的 MASQUE 节点（对照一项用）；自己的 TrustTunnel 节点开了 QUIC 监听（endpoint 的 `vpn.toml` 里加一个 `[listen_protocols.quic]` 表，UDP 与 TCP 同一个端口号；记下版本）；Surge（Mac 6.9 / iOS 5.22 以上，对照一项用）；一个开了 UDP 的上游 SOCKS5 节点（`udp-relay=true`，跟随出站模式一项用）。策略行照 Surge 的写法：`M = masque, <服务器>, <端口>, username=<用户名>, password=<口令>` 与 `TT3 = trust-tunnel, <服务器>, <端口>, username=<用户名>, password=<口令>, h3=true`（证书不是公共 CA 签的时另加 `server-cert-fingerprint-sha256=` 或 `skip-cert-verify=true`；TrustTunnel 的服务器写成 IP 时另加 `sni=<节点的主机名>`）。每份配置 `[Rule]` 里 `FINAL,<策略名>`，`rurge check -c <配置>` 零错误（没有 `W0007`、`W0029`）。

- [ ] MASQUE 的 TCP：`curl -x http://127.0.0.1:<http-listen 端口> https://example.com/ -I` 与 `curl --socks5-hostname 127.0.0.1:<socks5-listen 端口> https://example.com/ -I` 都返回 200，请求记录里策略链是 `M`、`error` 为空；同时打开几个网站，服务端日志（或抓包）只看到一条 QUIC 连接，服务端日志能看到请求头时，每个 CONNECT 都带 `proxy-authorization: Basic …`。下载一个几百 MiB 的文件，记下速度。服务端先说话的协议（经代理连一个 SMTP / SSH 主机）能看到对端的欢迎行。
- [ ] MASQUE 的 UDP：不需要参数，经 SOCKS5 UDP 发 DNS 查询（如 Proxifier 代理 `nslookup example.com 8.8.8.8`）得到回答；轮流发往两个不同的 DNS 服务器都有回答，服务端日志（或抓包）看到每个目标一条 `connect-udp` 流（对称型，已知差异，见兼容性清单 `masque` 一行），路径是 `/.well-known/masque/udp/<目标>/<端口>/`，包走 QUIC DATAGRAM 帧；经它进行一次语音通话或联机游戏。查一个应答超过约 1.2 KB 的记录（如 `dig +bufsize=4096 <TXT 记录很多的域名> TXT @8.8.8.8`）：放不进一个数据报的应答被丢弃、得不到回答（rurge 不退回 DATAGRAM capsule，已知差异），记下结果与服务端实现。
- [ ] MASQUE 口令错误：把 `password` 改错一位，重载后 `curl` 得到 502，会话记录的 `error` 是 `masque: proxy authentication required`；经 SOCKS5 UDP 的 DNS 查询同样失败、记录写同一个错误；**错误文本与日志（含 `--log-level verbose`）里都没有用户名与口令**。只写 `username` 不写 `password` 时 `rurge check` 报 `E0018`（`` `password` is required with `username` ``）。
- [ ] 服务端不支持 CONNECT-UDP（如果找得到：只做普通 CONNECT 的 HTTP/3 代理，或关掉了 QUIC 数据报的服务端）：TCP 照常；经 SOCKS5 UDP 的查询失败，记录写 `masque: the server does not support CONNECT-UDP`（Surge 在建立连接时校验，rurge 只让 UDP 失败，已知差异）。
- [ ] MASQUE 的端口跳跃：服务端把一段 UDP 端口转发到它的监听端口（手工加 DNAT / REDIRECT 规则，如 `iptables -t nat -A PREROUTING -p udp --dport 20000:20100 -j REDIRECT --to-ports <监听端口>`），策略写 `port-hopping=20000-20100, port-hopping-interval=10`：经 `M` 的下载进行中，抓包看到发往服务端的目的端口约每 10 秒换一个、下载不中断。
- [ ] 与 Surge 对照（MASQUE）：同一行策略原样放进 Surge 的配置、连同同一个服务端，再加上那个 Surge 实际在用的节点：rurge 与 Surge 都连得上，TCP 与 UDP 都能往返；只有一边连得上时记下服务端实现与版本，在兼容性清单登记差异（留意认证头的字段名——rurge 发 `proxy-authorization`，手册没写 Surge 用哪个）。
- [ ] `trust-tunnel` 的 `h3=true`：`curl` 经 HTTP 与 SOCKS5 两个入口都返回 200；抓包看到的是到服务器 UDP 端口的 QUIC、没有 TCP 连接；服务端日志看到的 `User-Agent` 是 `rurge`。把 endpoint 的 `[listen_protocols.quic]` 去掉后重启它：请求失败，不会改走 HTTP/2（没有回落，已知差异）。口令错误时会话记录写 `trust-tunnel: authentication failed`。`max-streams=2` 时同时打开五六个下载，服务端日志（或抓包）看到三条左右的 QUIC 连接、每条上同时至多 2 条隧道。另写 `shadow-tls-password=…` 时 `rurge check` 报 `E0018`（`` Shadow TLS cannot be combined with `h3=true` on a `trust-tunnel` policy ``）。同一行策略放进 Surge 的配置，两边都连得上。
- [ ] 公共 DoQ / DoH3：`[General]` 写 `encrypted-dns-server = quic://dns.adguard-dns.com`，`rurge dns lookup -c <配置> example.com --trace` 得到 A / AAAA，`source` 是 `upstream(quic://dns.adguard-dns.com:853)`；换成 `h3://cloudflare-dns.com/dns-query` 与 `h3://dns.google/dns-query` 同样得到答案，再试不写路径的 `h3://dns.google`（rurge 用 `/dns-query`）；`rurge run` 下浏览网页正常。抓包看到的是到 853 / 443 的 QUIC，DoQ 的查询报文 ID 为 0、带 EDNS(0) padding（报文长度是 128 的倍数）。`[Host]` 写 `example.com = server:quic://dns.adguard-dns.com` 同样有效。记下各服务的结果（公共服务是否开着 HTTP/3 以实测为准）。
- [ ] 跟随出站模式：`encrypted-dns-follow-outbound-mode = true`、`encrypted-dns-server = quic://dns.adguard-dns.com`，规则 `PROTOCOL,DOQ,<开了 UDP 的 SOCKS5 节点>`（DoH3 写 `PROTOCOL,DOH3,…`，服务器须在 443 端口）：`rurge run` 下浏览网页正常，请求记录里有协议为 `DOQ`（`DOH3`）、策略链为那个节点、`transport` 为 UDP 的内部会话，SOCKS5 节点那边看到一条 UDP 关联；规则改指 `M` 时同样正常（经 CONNECT-UDP）。把规则的策略换成一条 `http` 策略（不载 UDP）后，经它的查询失败、请求记录写 `policy does not support UDP`（不改走直连，已知差异）；换成以域名配置的代理时告警并直连。
- [ ] 休眠唤醒与换网：经 `M`（与 `TT3`）的连接在用时，让电脑休眠再唤醒，或在 Wi-Fi 与有线 / 手机热点之间切换：约 30 秒内（QUIC 的空闲超时）之后的请求走新建的连接、恢复正常，不必重启 rurge；其间的请求可能超时失败。DoQ / DoH3 的查询随后照常（旧连接上失败的查询在新连接上重问一次）。
- [ ] 脱敏：`GET /v1/policies/detail?policy_name=M`（与 `TT3`）与 `GET /v1/profiles/current` 里 `username` 与 `password` 都是 `***`。

```

`README.md`——把

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；M4c（external）已完成——`external` 策略（TCP；第一次用到时拉起外部程序、经它在本机端口上的 SOCKS5 转发；程序退出后下次用到时再拉起；输出写进数据目录；rurge 退出时连同它启动的进程一起停掉——Unix 用进程组，Windows 用 Job Object；三平台都支持，Surge 只在 Mac 上有）已可用，订阅导入的 `external` 一律跳过；M5a（UDP 地基）已完成——SOCKS5 监听支持 UDP ASSOCIATE，UDP 按规则分流到 DIRECT、REJECT 或 `socks5` / `socks5-tls` / `external`（`udp-relay=true`，含 `underlying-proxy` 链），全锥 NAT，每条 UDP 流一条请求记录，`block-quic` 与 `udp-policy-not-supported-behaviour` 生效；M5b（TLS 族的 UDP）已完成——`trojan`（UDP ASSOCIATE）、`anytls`（UDP over TCP v2）全锥，`vmess`（命令 2，每个目标一条连接）对称型；M5c（WireGuard 的 UDP 与其余）已完成——`wireguard` 的 UDP（全锥）与经 `underlying-proxy` 的隧道、`test-udp` / `proxy-test-udp`、`smart` 组计入 UDP、`dns-follow-interface`；M6（Shadowsocks / Snell / HTTP/2 族）已完成：M6a（Shadowsocks）已完成——`ss` 的 AEAD、`none` 与 SS 2022（含多用户身份头）方法、simple-obfs（`http` / `tls`），TCP 与 UDP（`udp-relay=true`、`udp-port`，全锥），流式旧方法在 M8 之前按 REJECT 处理；M6b（Snell）已完成——`snell` v4 / v5 的 TCP 与 UDP（UDP over TCP，全锥）、`reuse`（连接回池给下一个请求用）、simple-obfs `http`，v1–v3 与 v6 按 REJECT 处理（没写 `version` 即 v1）；M6c（HTTP/2 族）已完成——`h2-connect`（HTTP/2 CONNECT，多条隧道共用连接、`max-streams`；`udp-relay=true` 时经 CONNECT-UDP（RFC 9298）转发 UDP，每个目标一条流、对称型）与 `trust-tunnel`（HTTP/2，TCP；`h3=true` 在 M7 之前仍按 HTTP/2 连）；M7（QUIC 族）进行中：M7a（QUIC 地基与 Hysteria 2）已完成——`hysteria2` 的 TCP 与 UDP（QUIC 数据报，全锥）、Salamander 与 Gecko 混淆、`port-hopping`、`download-bandwidth`（只上报给服务端）与 `ecn`，可经 `underlying-proxy` 连出去；M7b（TUIC）已完成——`tuic-v5`（TUIC v5）与 `tuic`（TUIC v4，tuic 0.8.5 的协议）的 TCP 与 UDP（服务端接受 QUIC 数据报时走数据报，v5 放不下的包分片，否则每个包一条单向流；全锥）、`port-hopping` 与 `ecn`，可经 `underlying-proxy` 连出去；M7c（MASQUE、`trust-tunnel` 的 HTTP/3、DoQ / DoH3）在后；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

换成

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；M4c（external）已完成——`external` 策略（TCP；第一次用到时拉起外部程序、经它在本机端口上的 SOCKS5 转发；程序退出后下次用到时再拉起；输出写进数据目录；rurge 退出时连同它启动的进程一起停掉——Unix 用进程组，Windows 用 Job Object；三平台都支持，Surge 只在 Mac 上有）已可用，订阅导入的 `external` 一律跳过；M5a（UDP 地基）已完成——SOCKS5 监听支持 UDP ASSOCIATE，UDP 按规则分流到 DIRECT、REJECT 或 `socks5` / `socks5-tls` / `external`（`udp-relay=true`，含 `underlying-proxy` 链），全锥 NAT，每条 UDP 流一条请求记录，`block-quic` 与 `udp-policy-not-supported-behaviour` 生效；M5b（TLS 族的 UDP）已完成——`trojan`（UDP ASSOCIATE）、`anytls`（UDP over TCP v2）全锥，`vmess`（命令 2，每个目标一条连接）对称型；M5c（WireGuard 的 UDP 与其余）已完成——`wireguard` 的 UDP（全锥）与经 `underlying-proxy` 的隧道、`test-udp` / `proxy-test-udp`、`smart` 组计入 UDP、`dns-follow-interface`；M6（Shadowsocks / Snell / HTTP/2 族）已完成：M6a（Shadowsocks）已完成——`ss` 的 AEAD、`none` 与 SS 2022（含多用户身份头）方法、simple-obfs（`http` / `tls`），TCP 与 UDP（`udp-relay=true`、`udp-port`，全锥），流式旧方法在 M8 之前按 REJECT 处理；M6b（Snell）已完成——`snell` v4 / v5 的 TCP 与 UDP（UDP over TCP，全锥）、`reuse`（连接回池给下一个请求用）、simple-obfs `http`，v1–v3 与 v6 按 REJECT 处理（没写 `version` 即 v1）；M6c（HTTP/2 族）已完成——`h2-connect`（HTTP/2 CONNECT，多条隧道共用连接、`max-streams`；`udp-relay=true` 时经 CONNECT-UDP（RFC 9298）转发 UDP，每个目标一条流、对称型）与 `trust-tunnel`（HTTP/2，TCP）；M7（QUIC 族）已完成：M7a（QUIC 地基与 Hysteria 2）已完成——`hysteria2` 的 TCP 与 UDP（QUIC 数据报，全锥）、Salamander 与 Gecko 混淆、`port-hopping`、`download-bandwidth`（只上报给服务端）与 `ecn`，可经 `underlying-proxy` 连出去；M7b（TUIC）已完成——`tuic-v5`（TUIC v5）与 `tuic`（TUIC v4，tuic 0.8.5 的协议）的 TCP 与 UDP（服务端接受 QUIC 数据报时走数据报，v5 放不下的包分片，否则每个包一条单向流；全锥）、`port-hopping` 与 `ecn`，可经 `underlying-proxy` 连出去；M7c（HTTP/3 族与 DoQ / DoH3）已完成——`masque` 的 TCP（HTTP/3 上的普通 CONNECT，每个 CONNECT 都带 Basic 凭据）与 UDP（CONNECT-UDP，HTTP/3 数据报，每个目标一条流、对称型）、`port-hopping` 与 `ecn`，`trust-tunnel` 的 `h3=true`（HTTP/3，`max-streams` 按每条 QUIC 连接计），`quic://`（DoQ）与 `h3://`（DoH3）加密 DNS 上游，开了 `encrypted-dns-follow-outbound-mode` 时 DoQ / DoH3 的连接也按规则走；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

`README.md`——把

```markdown
| DNS            | 普通 DNS、DoH / DoT / DoQ / DoH3、本地映射、劫持、fake-ip、always-real-ip（普通 DNS / DoH / DoT / `tcp://` / `[Host]` / 系统 hosts 已实现，M2b）                | 1 / 3 |
| 出站协议       | DIRECT / REJECT 系列 / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / 外部程序（DIRECT 与 REJECT 系四种已实现，M3a；HTTP（TCP）与 SOCKS5（含 `socks5-tls`，TCP 与 UDP）上游及 `underlying-proxy` 链已实现，阶段 2 / M1、M5a；Trojan（TCP 与 UDP；WebSocket 传输）已实现，阶段 2 / M2a、M5b；VMess（AEAD 握手，TCP 与 UDP；可叠加 TLS / WebSocket）与 AnyTLS（TCP 与 UDP，会话复用）已实现，阶段 2 / M2b、M5b；SSH（TCP，会话复用）已实现，阶段 2 / M4a；WireGuard（TCP 与 UDP，用户态隧道）已实现，阶段 2 / M4b、M5c；外部程序（TCP 与 UDP，三平台）已实现，阶段 2 / M4c、M5a；Shadowsocks（AEAD / 2022、obfs，TCP 与 UDP）已实现，阶段 2 / M6a；Snell v4 / v5（TCP 与 UDP、`reuse`、obfs `http`）已实现，阶段 2 / M6b；HTTP/2 CONNECT（`h2-connect`，TCP 与 CONNECT-UDP）与 Trust Tunnel（HTTP/2，TCP）已实现，阶段 2 / M6c；Hysteria 2（TCP 与 UDP、Salamander / Gecko 混淆、端口跳跃）已实现，阶段 2 / M7a；TUIC v5 与 v4（TCP 与 UDP、端口跳跃）已实现，阶段 2 / M7b） | 2     |
```

换成

```markdown
| DNS            | 普通 DNS、DoH / DoT / DoQ / DoH3、本地映射、劫持、fake-ip、always-real-ip（普通 DNS / DoH / DoT / `tcp://` / `[Host]` / 系统 hosts 已实现，M2b；DoQ / DoH3 已实现，阶段 2 / M7c）                | 1 / 3 |
| 出站协议       | DIRECT / REJECT 系列 / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / 外部程序（DIRECT 与 REJECT 系四种已实现，M3a；HTTP（TCP）与 SOCKS5（含 `socks5-tls`，TCP 与 UDP）上游及 `underlying-proxy` 链已实现，阶段 2 / M1、M5a；Trojan（TCP 与 UDP；WebSocket 传输）已实现，阶段 2 / M2a、M5b；VMess（AEAD 握手，TCP 与 UDP；可叠加 TLS / WebSocket）与 AnyTLS（TCP 与 UDP，会话复用）已实现，阶段 2 / M2b、M5b；SSH（TCP，会话复用）已实现，阶段 2 / M4a；WireGuard（TCP 与 UDP，用户态隧道）已实现，阶段 2 / M4b、M5c；外部程序（TCP 与 UDP，三平台）已实现，阶段 2 / M4c、M5a；Shadowsocks（AEAD / 2022、obfs，TCP 与 UDP）已实现，阶段 2 / M6a；Snell v4 / v5（TCP 与 UDP、`reuse`、obfs `http`）已实现，阶段 2 / M6b；HTTP/2 CONNECT（`h2-connect`，TCP 与 CONNECT-UDP）与 Trust Tunnel（HTTP/2，TCP）已实现，阶段 2 / M6c；Hysteria 2（TCP 与 UDP、Salamander / Gecko 混淆、端口跳跃）已实现，阶段 2 / M7a；TUIC v5 与 v4（TCP 与 UDP、端口跳跃）已实现，阶段 2 / M7b；MASQUE（TCP 与 CONNECT-UDP、端口跳跃）与 Trust Tunnel 的 HTTP/3（`h3=true`）已实现，阶段 2 / M7c） | 2     |
```

`README.md`——把

```markdown
3. **阶段 2** 出站协议全集、策略组、策略订阅（进行中：M1 出站地基与 HTTP / SOCKS5 上游已完成；M2a（Trojan，TCP + WebSocket）已完成；M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成；M3a（成员装配与订阅）已完成；M3b（测速与自动组）已完成；M3c（`smart`）已完成；M4a（SSH）已完成；M4b（WireGuard）已完成；M4c（external）已完成；M5a（UDP 地基）已完成；M5b（TLS 族的 UDP）已完成；M5c（WireGuard 的 UDP 与其余）已完成；M6a（Shadowsocks）已完成；M6b（Snell）已完成；M6c（HTTP/2 族）已完成，M6 至此完成；M7a（QUIC 地基与 Hysteria 2）已完成；M7b（TUIC）已完成，M7c（HTTP/3 族与 DoQ / DoH3）在后）
```

换成

```markdown
3. **阶段 2** 出站协议全集、策略组、策略订阅（进行中：M1 出站地基与 HTTP / SOCKS5 上游已完成；M2a（Trojan，TCP + WebSocket）已完成；M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成；M3a（成员装配与订阅）已完成；M3b（测速与自动组）已完成；M3c（`smart`）已完成；M4a（SSH）已完成；M4b（WireGuard）已完成；M4c（external）已完成；M5a（UDP 地基）已完成；M5b（TLS 族的 UDP）已完成；M5c（WireGuard 的 UDP 与其余）已完成；M6a（Shadowsocks）已完成；M6b（Snell）已完成；M6c（HTTP/2 族）已完成，M6 至此完成；M7a（QUIC 地基与 Hysteria 2）已完成；M7b（TUIC）已完成；M7c（HTTP/3 族与 DoQ / DoH3）已完成，M7 至此完成）
```

`README_en.md`——把

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; M4b (WireGuard) is done — the `wireguard` policy (TCP; a user-space tunnel, no virtual interface; several peers chosen by `allowed-ips`; `client-id` (WARP's reserved bytes); destination names looked up through the tunnel when the section has a `dns-server`, on this machine otherwise; tested by a handshake when there is neither a `dns-server` nor a `test-url`) is usable; M4c (external) is done — the `external` policy (TCP; the program is started on first use and reached as a SOCKS5 proxy on its local port; a program that exited is started again on the next use; its output goes to the data directory; when rurge exits, the program is stopped together with whatever it started — a process group on Unix, a Job Object on Windows; all three platforms, where Surge has it on the Mac only) is usable, and subscriptions never bring `external` policies in; M5a (the UDP foundation) is done — the SOCKS5 listener takes UDP ASSOCIATE, and UDP is routed by rule to DIRECT, REJECT or `socks5` / `socks5-tls` / `external` (`udp-relay=true`, `underlying-proxy` chains included), full-cone NAT, one request record per UDP flow, and `block-quic` and `udp-policy-not-supported-behaviour` take effect; M5b (UDP over the TLS family) is done — `trojan` (UDP ASSOCIATE) and `anytls` (UDP over TCP v2) with full-cone NAT, `vmess` (command 2, one connection per target) symmetric; M5c (UDP over WireGuard and the rest) is done — UDP over `wireguard` (full cone) and tunnels over an `underlying-proxy`, `test-udp` / `proxy-test-udp`, UDP in `smart` groups, and `dns-follow-interface`; M6 (Shadowsocks / Snell / the HTTP/2 family) is done: M6a (Shadowsocks) is done — `ss` with the AEAD, `none` and SS 2022 (multi-user identity headers included) methods, simple-obfs (`http` / `tls`), TCP and UDP (`udp-relay=true`, `udp-port`, full cone), with the legacy stream ciphers behaving as REJECT until M8; M6b (Snell) is done — `snell` v4 / v5 over TCP and UDP (UDP over TCP, full cone), `reuse` (a connection goes back to a pool for the next request) and simple-obfs `http`, with v1–v3 and v6 behaving as REJECT (a line without `version` is v1); M6c (the HTTP/2 family) is done — `h2-connect` (HTTP/2 CONNECT, tunnels sharing connections up to `max-streams`; with `udp-relay=true`, UDP over CONNECT-UDP (RFC 9298), one stream per target, symmetric) and `trust-tunnel` (HTTP/2, TCP; `h3=true` still connects over HTTP/2 until M7); M7 (the QUIC family) is in progress: M7a (the QUIC foundation and Hysteria 2) is done — `hysteria2` over TCP and UDP (QUIC datagrams, full cone), Salamander and Gecko obfuscation, `port-hopping`, `download-bandwidth` (only reported to the server) and `ecn`, optionally through an `underlying-proxy`; M7b (TUIC) is done — `tuic-v5` (TUIC v5) and `tuic` (TUIC v4, as tuic 0.8.5 speaks it) over TCP and UDP (in QUIC datagrams where the server takes them, v5 splitting a packet that does not fit, else a stream per packet; full cone), `port-hopping` and `ecn`, optionally through an `underlying-proxy`; M7c (MASQUE, `trust-tunnel` over HTTP/3, DoQ / DoH3) comes next; the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

换成

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; M4b (WireGuard) is done — the `wireguard` policy (TCP; a user-space tunnel, no virtual interface; several peers chosen by `allowed-ips`; `client-id` (WARP's reserved bytes); destination names looked up through the tunnel when the section has a `dns-server`, on this machine otherwise; tested by a handshake when there is neither a `dns-server` nor a `test-url`) is usable; M4c (external) is done — the `external` policy (TCP; the program is started on first use and reached as a SOCKS5 proxy on its local port; a program that exited is started again on the next use; its output goes to the data directory; when rurge exits, the program is stopped together with whatever it started — a process group on Unix, a Job Object on Windows; all three platforms, where Surge has it on the Mac only) is usable, and subscriptions never bring `external` policies in; M5a (the UDP foundation) is done — the SOCKS5 listener takes UDP ASSOCIATE, and UDP is routed by rule to DIRECT, REJECT or `socks5` / `socks5-tls` / `external` (`udp-relay=true`, `underlying-proxy` chains included), full-cone NAT, one request record per UDP flow, and `block-quic` and `udp-policy-not-supported-behaviour` take effect; M5b (UDP over the TLS family) is done — `trojan` (UDP ASSOCIATE) and `anytls` (UDP over TCP v2) with full-cone NAT, `vmess` (command 2, one connection per target) symmetric; M5c (UDP over WireGuard and the rest) is done — UDP over `wireguard` (full cone) and tunnels over an `underlying-proxy`, `test-udp` / `proxy-test-udp`, UDP in `smart` groups, and `dns-follow-interface`; M6 (Shadowsocks / Snell / the HTTP/2 family) is done: M6a (Shadowsocks) is done — `ss` with the AEAD, `none` and SS 2022 (multi-user identity headers included) methods, simple-obfs (`http` / `tls`), TCP and UDP (`udp-relay=true`, `udp-port`, full cone), with the legacy stream ciphers behaving as REJECT until M8; M6b (Snell) is done — `snell` v4 / v5 over TCP and UDP (UDP over TCP, full cone), `reuse` (a connection goes back to a pool for the next request) and simple-obfs `http`, with v1–v3 and v6 behaving as REJECT (a line without `version` is v1); M6c (the HTTP/2 family) is done — `h2-connect` (HTTP/2 CONNECT, tunnels sharing connections up to `max-streams`; with `udp-relay=true`, UDP over CONNECT-UDP (RFC 9298), one stream per target, symmetric) and `trust-tunnel` (HTTP/2, TCP); M7 (the QUIC family) is done: M7a (the QUIC foundation and Hysteria 2) is done — `hysteria2` over TCP and UDP (QUIC datagrams, full cone), Salamander and Gecko obfuscation, `port-hopping`, `download-bandwidth` (only reported to the server) and `ecn`, optionally through an `underlying-proxy`; M7b (TUIC) is done — `tuic-v5` (TUIC v5) and `tuic` (TUIC v4, as tuic 0.8.5 speaks it) over TCP and UDP (in QUIC datagrams where the server takes them, v5 splitting a packet that does not fit, else a stream per packet; full cone), `port-hopping` and `ecn`, optionally through an `underlying-proxy`; M7c (the HTTP/3 family and DoQ / DoH3) is done — `masque` over TCP (plain CONNECTs over HTTP/3, each carrying the Basic credentials) and UDP (CONNECT-UDP in HTTP/3 datagrams, one stream per target, symmetric), `port-hopping` and `ecn`, `trust-tunnel` with `h3=true` (HTTP/3, `max-streams` counted per QUIC connection), the `quic://` (DoQ) and `h3://` (DoH3) encrypted DNS upstreams, and, with `encrypted-dns-follow-outbound-mode`, DoQ / DoH3 connections routed by the rules too; the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

`README_en.md`——把

```markdown
| DNS                 | Plain DNS, DoH / DoT / DoQ / DoH3, local mapping, hijacking, fake IP, always-real-ip (plain DNS / DoH / DoT / `tcp://` / `[Host]` / system hosts implemented, M2b)     | 1 / 3 |
| Outbound            | DIRECT / REJECT family / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / external program (DIRECT and the four REJECT flavours implemented, M3a; HTTP (TCP) and SOCKS5 (including `socks5-tls`, TCP and UDP) upstreams and `underlying-proxy` chains implemented, phase 2 / M1, M5a; Trojan (TCP and UDP; WebSocket transport) implemented, phase 2 / M2a, M5b; VMess (AEAD handshake, TCP and UDP; optionally TLS / WebSocket) and AnyTLS (TCP and UDP, session reuse) implemented, phase 2 / M2b, M5b; SSH (TCP, session reuse) implemented, phase 2 / M4a; WireGuard (TCP and UDP, user-space tunnel) implemented, phase 2 / M4b, M5c; external program (TCP and UDP, all three platforms) implemented, phase 2 / M4c, M5a; Shadowsocks (AEAD / 2022, obfs, TCP and UDP) implemented, phase 2 / M6a; Snell v4 / v5 (TCP and UDP, `reuse`, obfs `http`) implemented, phase 2 / M6b; HTTP/2 CONNECT (`h2-connect`, TCP and CONNECT-UDP) and Trust Tunnel (HTTP/2, TCP) implemented, phase 2 / M6c; Hysteria 2 (TCP and UDP, Salamander / Gecko obfuscation, port hopping) implemented, phase 2 / M7a; TUIC v5 and v4 (TCP and UDP, port hopping) implemented, phase 2 / M7b) | 2     |
```

换成

```markdown
| DNS                 | Plain DNS, DoH / DoT / DoQ / DoH3, local mapping, hijacking, fake IP, always-real-ip (plain DNS / DoH / DoT / `tcp://` / `[Host]` / system hosts implemented, M2b; DoQ / DoH3 implemented, phase 2 / M7c)     | 1 / 3 |
| Outbound            | DIRECT / REJECT family / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / external program (DIRECT and the four REJECT flavours implemented, M3a; HTTP (TCP) and SOCKS5 (including `socks5-tls`, TCP and UDP) upstreams and `underlying-proxy` chains implemented, phase 2 / M1, M5a; Trojan (TCP and UDP; WebSocket transport) implemented, phase 2 / M2a, M5b; VMess (AEAD handshake, TCP and UDP; optionally TLS / WebSocket) and AnyTLS (TCP and UDP, session reuse) implemented, phase 2 / M2b, M5b; SSH (TCP, session reuse) implemented, phase 2 / M4a; WireGuard (TCP and UDP, user-space tunnel) implemented, phase 2 / M4b, M5c; external program (TCP and UDP, all three platforms) implemented, phase 2 / M4c, M5a; Shadowsocks (AEAD / 2022, obfs, TCP and UDP) implemented, phase 2 / M6a; Snell v4 / v5 (TCP and UDP, `reuse`, obfs `http`) implemented, phase 2 / M6b; HTTP/2 CONNECT (`h2-connect`, TCP and CONNECT-UDP) and Trust Tunnel (HTTP/2, TCP) implemented, phase 2 / M6c; Hysteria 2 (TCP and UDP, Salamander / Gecko obfuscation, port hopping) implemented, phase 2 / M7a; TUIC v5 and v4 (TCP and UDP, port hopping) implemented, phase 2 / M7b; MASQUE (TCP and CONNECT-UDP, port hopping) and Trust Tunnel over HTTP/3 (`h3=true`) implemented, phase 2 / M7c) | 2     |
```

`README_en.md`——把

```markdown
3. **Phase 2** All outbound protocols, policy groups, subscriptions (in progress: M1, the outbound foundation and HTTP / SOCKS5 upstreams, is done; M2a, Trojan (TCP + WebSocket), is done; M2b, VMess / AnyTLS, is done; M2c, Shadow TLS, is done; M3a, member assembly and subscriptions, is done; M3b, connectivity tests and automatic groups, is done; M3c, `smart` groups, is done; M4a, SSH, is done; M4b, WireGuard, is done; M4c, external, is done; M5a, the UDP foundation, is done; M5b, UDP over the TLS family, is done; M5c, UDP over WireGuard and the rest, is done; M6a, Shadowsocks, is done; M6b, Snell, is done; M6c, the HTTP/2 family, is done, which completes M6; M7a, the QUIC foundation and Hysteria 2, is done; M7b, TUIC, is done, with M7c, the HTTP/3 family and DoQ / DoH3, next)
```

换成

```markdown
3. **Phase 2** All outbound protocols, policy groups, subscriptions (in progress: M1, the outbound foundation and HTTP / SOCKS5 upstreams, is done; M2a, Trojan (TCP + WebSocket), is done; M2b, VMess / AnyTLS, is done; M2c, Shadow TLS, is done; M3a, member assembly and subscriptions, is done; M3b, connectivity tests and automatic groups, is done; M3c, `smart` groups, is done; M4a, SSH, is done; M4b, WireGuard, is done; M4c, external, is done; M5a, the UDP foundation, is done; M5b, UDP over the TLS family, is done; M5c, UDP over WireGuard and the rest, is done; M6a, Shadowsocks, is done; M6b, Snell, is done; M6c, the HTTP/2 family, is done, which completes M6; M7a, the QUIC foundation and Hysteria 2, is done; M7b, TUIC, is done; M7c, the HTTP/3 family and DoQ / DoH3, is done, which completes M7)
```

`CLAUDE.md`——把

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、发送时本机地址或路由失效或握手无回应时换新载体（网络变化后自愈）、网络变化入口、同一私钥与 peer 只留一条隧道，节与载体设置都相同的策略共用）、`stream`、`dns`（隧道内 DNS 与缓存；每个问题 2 秒内重发、先等隧道第一次握手）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。M4c（external）已完成——`rurge-config::spec::ExternalSpec`（`args` 是 `Secret`；两个 `external` 策略写了同一个 `local-port` 是 `E0018`；`args` 进内联参数的脱敏名单）、订阅导入的 `external` 行一律跳过（`W0023`）；`rurge-platform::process`（Unix 进程组与 `killpg`，Windows Job Object）；`rurge_proto::external`（`ExternalOutbound`：第一次用到时拉起、输出写 `<数据目录>/external/<策略名>.log` 且超过 1 MiB 时在拉起前轮转、子进程环境去掉代理变量并设 `NO_PROXY=*`、程序退出后下次用到时再拉起且两次拉起至少隔 2 秒、每 500 ms 连一次本机端口最多 6 次；`ProcessHook` / `ProcessGroup` 与 `NoProcessGroups`；程序自己退出时连同它启动的进程一起结束）；`rurge-engine` 的工厂分支、`EngineShared.processes` / `externals` 与 `Engine::stop_external_programs`；bin 的 `PlatformProcesses` 与退出流程最后停掉全部外部程序；只用于测试的工作区成员 `tests/external`（`rurge-external-tests`：极小的 SOCKS5 辅助程序 `socks-helper` 与真实拉起它的用例）；能力表翻转 `external`（`W0007` 不再因 `external` 出现）。M4 至此完成。M5（UDP 路径）按三份计划推进（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）：M5a 已完成——`rurge_net::connector::PacketSocket`（按包收发、带地址的 UDP 载体）与 `Connector::open_udp`（`DirectConnector`：每个地址族一个未连接的 socket，忽略 Windows 的 ICMP 不可达报错）；`Outbound::udp()` / `open_udp()` 与 `UdpSupport`；DIRECT 与 `socks5` / `socks5-tls` / `external` 的 UDP（`udp-relay`，`W0029` 退役）；`ChainConnector::open_udp`（链式 UDP 载体）；`rurge-inbound` 的 SOCKS5 UDP ASSOCIATE（`UdpClient`、`UdpAdmission`、`Dialer::admit_udp` / `associate`）；`rurge-engine` 的 UDP 流水线（`udp` 模块：一条流 = 关联 + 目标、按"关联 × 出站"共用载体的全锥、60 秒 / DNS 10 秒回收、1024 流 / 4096 关联的上限）、请求记录与 API 的 `transport`、`block-quic`（`W0029` 退役）与 `udp-policy-not-supported-behaviour`、QUIC Initial 识别、`PROTOCOL` 规则按传输层匹配 `TCP` / `UDP`；`FakeSocks5` 与 `tests/external` 辅助程序的 UDP ASSOCIATE；对 sing-box `socks` 入站的 UDP 互操作用例。M5b（TLS 族的 UDP）已完成——`rurge-proto` 的 `stream_udp`（一条字节流上按包收发：请求头随第一个包发出、写那个包的目标；`trojan` 的 UDP ASSOCIATE 与 `anytls` 的 UDP over TCP v2 两种封装）、`trojan` / `anytls` 的 `open_udp`（全锥）、`vmess` 的命令 2（`vmess::udp`：每个目标一条 VMess 连接、随它的第一个包建立、每个数据报一个分块，对称型）；`FakeTrojan` / `FakeAnyTls` / `FakeVmess` 的 UDP 与 `rurge_proto::testing::udp_echo_server`；经引擎的端到端用例（`tests/udp_tls_family.rs`）；对 sing-box（三种）与 xray（vmess）的 UDP 互操作用例。M5c（WireGuard 的 UDP 与其余）已完成——`rurge-proto-wireguard` 的 `TunnelUdp`（隧道里每个地址族一个 UDP socket、第一次发往该族时绑定、全锥，目标名经隧道 DNS 或本机解析）与 `Stack::udp_bind` / `check`；`rurge_net::packet_datagram`（把 `PacketSocket` 变成一条到固定目标的 `Datagram`）与 `ChainConnector::connect_udp`，`wireguard` 的载体经 `underlying-proxy`（底层策略不载 UDP 时拨号失败，`W0029` 退役）；启动时 peer 连不上的告警每个策略的每个 peer 5 分钟至多一次（M4b 延后事项 #15）；`rurge_policy::udp_probe`（经策略的 UDP 向 `hostname@ipv4` 问一次 A 记录）、`Engine::test_udp` 与 `POST /v1/policies/test` 结果里的 `udp` 键（`test-udp` / `proxy-test-udp`，不保存、不参与组的选择）；`smart` 计入 UDP（载体打不开算失败、第一个回包算首字节、3 秒无回包只在 53 / 443 端口算失败、UDP 不换成员）；`dns-follow-interface`（`rurge_net::connector::Via` / `ResolveVia`、`Resolver::lookup_via`：策略自己的解析经它的 `interface` 问普通 DNS 服务器、答案另存，配了加密 DNS 时不跟随；没有 `interface` 时 `W0028`）；对 sing-box WireGuard 端点的 UDP 互操作。M5 至此完成。M5 合并后的复查（2026-10-02，M5 设计第 23 节）改了：Windows 上写了 `interface` 的策略能开 UDP 载体；发给客户端失败只丢那个包；发往上游时出站不收或系统拒发的包只丢那一个（`lost_alone`），其余发送失败的流记为失败并歇 5 秒，一次发送至多 15 秒（`SEND_TIMEOUT`，超时作废载体）；TCP keepalive（`rurge_net::socket::keep_alive`，监听与 `DirectConnector` 都开）；名字目标的回包按端口归流，`smart` 计入 UDP 的发送失败、3 秒规则只看发往地址的流；重建注册表时沿用内置 DIRECT；`wireguard` 跟随底层策略的变化（`Connector::route` / `rurge_net::connector::Route`、`PolicyRegistry::relay_now`）；开 UDP 流时的解析限时 10 秒；每个关联排队至多 1 MiB（`Queued`）、一次打开失败等着它的流一起失败；双栈监听上 IPv4 客户端的回包发往映射地址。M6（Shadowsocks / Snell / HTTP/2 族）按三份计划推进（M6a Shadowsocks → M6b Snell → M6c HTTP/2 族）：M6a（Shadowsocks）已完成——`rurge-config::spec` 的 `SsSpec`（`SsMethod`：AEAD 五种、`none`、SS 2022 两种；2022 的 `password` 按冒号拆成逐层的 Base64 密钥、最后一段是用户密钥，不合法是 `E0018` 且不引用取值；`udp-relay`、`udp-port`）与 `ObfsOpts`（与 Snell 共用；`obfs-host` 缺省服务器主机名、`obfs-uri` 缺省 `/`）、`NotImplemented`（取代 `legacy_vmess` 标记：vmess 旧握手与 `ss` 流式旧方法都是 `W0007` + REJECT，流式方法每种每次加载一条，会话日志 `policy protocol not implemented: ss (<method>)`）、`obfs-host` 进内联参数的脱敏名单；`rurge_proto::transport::obfs`（simple-obfs 的 `http` / `tls`，自写模板，`Stack` 的一层：connect → shadow-tls → obfs → tls → ws）；`rurge_proto::shadowsocks`（`ShadowsocksOutbound`：AEAD 分块流与 `none`、SS 2022（BLAKE3 子密钥、请求头块与填充、应答的类型 / 时间戳 / 回显 salt 校验、SIP023 多用户身份头）、UDP（`udp-relay=true` 发往 `udp-port`，全锥；2022 的分离头、按服务端 session 的防重放窗口））；新依赖只有 `blake3`；`rurge_proto::testing` 的 `FakeShadowsocks`（AEAD、2022 含身份头、UDP、两种 obfs）与 `accept_obfs`；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_shadowsocks.rs`）；能力表翻转 `ss`（流式旧方法除外）；测试里"未实现的协议"的例子从 `ss` 换成 `hysteria2`；`tests/interop` 对 shadowsocks-rust `ssserver`（固定版本 v1.25.0，`RURGE_TEST_SSSERVER`）与 sing-box `shadowsocks` 入站的互操作用例。M6b（Snell）已完成——`rurge-config::spec` 的 `SnellSpec`（`SnellVersion` 只有 V4 / V5；`version` 1–6、缺省 1，1–3 与 6 是 `NotImplemented::SnellVersion`：`W0007` + REJECT，每个版本每次加载一条，会话日志 `policy protocol not implemented: snell v<n>`；`psk` 是 `Secret`；v4 / v5 的 `obfs` 只有 `http`；`mode` 只对 v6 校验）；`rurge_proto::snell`（`SnellOutbound`：`kdf`（Argon2id 取前 16 字节，在阻塞线程上算；工作区依赖 `argon2` 0.6，已在依赖树里）、`record`（`SnellStream`：每方向一个 salt、7 字节加密头、首帧 256–511 字节填充与负载密文交错、每帧至多 0x3FFF、空帧结束一个方向、复用时计数器延续）、`tunnel`（请求头随首段负载写出，`reuse=false` 发 Connect `0x01`、`reuse=true` 发 ConnectV2 `0x05`；应答 `00` / `02`；干净结束的连接在 `Drop` 时回池，没结束的在后台补完再回池；池里取出的失效连接换新连接重发一次）、`pool`（每个出站至多 8 条空闲连接、空闲 60 秒回收）、`udp`（命令 `0x06`：每个载体一条自己的连接、从不进池，每个数据报一帧，全锥；`udp-port` 只解析（`W0029`），UDP 一律走主端口））；`LazyHead` 改为泛型（`LazyHead<S = BoxedStream>`，加 `get_mut` / `into_inner`）；`rurge_proto::testing` 的 `FakeSnell`（`SnellScript`：TCP、ConnectV2 复用、UDP、obfs http、拒绝、每条连接的请求数上限）；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_snell.rs`）；能力表翻转 `snell`（v4 / v5）；`tests/interop` 对 sing-box `snell` 入站（`version: 5`，同时接受 v4 客户端；sing-box 固定版本从 1.14.1 升到 1.14.2）与 Surge 官方 snell-server v5.0.1（只有 Linux 版，`RURGE_TEST_SNELL_SERVER`，只在 Linux CI 上装）的互操作用例。M6c（HTTP/2 族）已完成——`rurge-config::spec::h2`（`H2ConnectSpec`：凭据写在端口之后或命名、命名的优先，`headers`、`max-streams` 缺省 3 且至少 1、`udp-relay`；`TrustTunnelSpec`：`username` / `password` 只读命名写法、必填（`E0018`）；两者 `alpn` 固定 `h2`（写了别的 `W0028`），`headers` 里 HTTP/2 禁止的连接专用字段与 `host` 各 `W0028` 并去掉；`trust-tunnel` 的 `h3=true` 是 `W0029`、M7 之前按 HTTP/2 连，`udp-relay` 是 `W0028`）；`rurge_proto::h2pool`（`H2Pool`：每个出站一个池，每条 TLS + HTTP/2 连接至多 `max-streams` 条流且不超过服务端的上限、流数自己计，新连接收到服务端 SETTINGS 之前只承载一条流（得知 SETTINGS 的 PING 5 秒内没有回应即不再分配新流），在途的每次拨号算作 `max-streams` 个位置、放不下的请求再拨（至多 8 条在途），连接窗口为 `max-streams` × 1 MiB（至少 4 MiB），没有流的连接空闲 60 秒关闭，收到 GOAWAY 的连接不再分配新流，请求没得到应答时用 PING 检查连接是否还活着（5 秒），死了就换新连接；`H2Stream`：按流控窗口分段写、读即释放容量、END_STREAM 半关闭；`StackDial`：ALPN 不是 `h2` 时 `<协议>: the server does not speak HTTP/2`）；`rurge_proto::h2connect`（`H2ConnectOutbound`：CONNECT + Basic，`headers` 替换同名的自生成字段、`<random-string>` 按请求生成；407 是 `h2-connect: proxy authentication required`；`capsule`（RFC 9297 的 capsule 与 varint）与 `udp`（`udp-relay=true` 时的 CONNECT-UDP：每个目标一条 extended CONNECT 流、对称型，masque 默认模板的路径，DATAGRAM capsule，其它 capsule 与 context id 丢弃，超过 65527 字节的数据报不发，服务端没通告 extended CONNECT 时发往每个目标的第一个包失败，空闲 60 秒关流））；`rurge_proto::trust_tunnel`（`TrustTunnelOutbound`：同一套 CONNECT，`user-agent: rurge`，407 是 `trust-tunnel: authentication failed`，不载 UDP）；`TlsClient::wrap_negotiated` / `Stack::open_negotiated`（取协商出的 ALPN）；`rurge_proto::testing` 的 `FakeH2Proxy`（`H2ProxyScript`：Basic 与 407、拒绝、服务端流数上限、GOAWAY、extended CONNECT 与 CONNECT-UDP、多余的 capsule、`user-agent` 必填、不参与 ALPN、双向 TLS）；`rurge-engine` 的两个工厂分支与经引擎的端到端用例（`tests/outbounds_http2.rs`）；能力表翻转 `h2-connect` 与 `trust-tunnel`；`tests/interop` 对 TrustTunnel endpoint v1.1.0（只有 Linux / macOS 版，`RURGE_TEST_TRUSTTUNNEL`；sing-box 1.14.2 的 `http` 入站只说 HTTP/1，所以 endpoint 同时充当 `h2-connect` 普通 CONNECT 的参照）的互操作用例；CONNECT-UDP 没有可用的参照服务端。M6 至此完成；合并后的复查（M6 设计第 18 节）又修了会话池的串行拨号与 SETTINGS 到达前静默断掉的连接、Snell 的 `udp-port`（只解析）与重拨时只唤醒最后一个轮询者、CONNECT-UDP 的 `:authority`（取 TLS 的名字）、`headers` 的值进 `Debug`、ss 首字节前被重置时的提示、`obfs-uri` 脱敏，并把 CI 钉在 1.89（`RUSTUP_TOOLCHAIN=1.89.0`；此前 CI 每次装最新 stable，三个平台都在 clippy 一步失败，测试与互操作从未执行过）。M7（QUIC 族）按三份计划推进（M7a QUIC 地基 + Hysteria 2 → M7b TUIC → M7c HTTP/3 族与 DoQ / DoH3）：M7a 已完成——新依赖只有 `quinn` 0.11.12（`default-features = false`，`runtime-tokio` + `rustls-ring`，随之引入 quinn-proto / quinn-udp；不用 crates.io 的 `h3`，M7-D5），`blake2` 0.10 沿用依赖树里的；`rurge-config::spec` 的 `QuicOpts`（只有 `port_hopping: Option<PortHopping>`；`alpn` 留在 `TlsOpts`、QUIC 类缺省 `h3`，`ecn` 留在 `CommonOpts`）、`Hysteria2Spec`（`password` 必填、`Secret`；`download-bandwidth` 整数 Mbps；`Hysteria2Obfs` 的 Salamander / Gecko 互斥，都是 `Secret`）与 `ProtoSpec::quic()`：`port-hopping` 写法错误是 `E0018`，`port-hopping-interval` 小于 5 是 `W0012` 并按 5、没有 `port-hopping` 时 `W0028`，`tfo` / `udp-relay` 在 `hysteria2` 上是 `W0028`，`ecn` 在它上面不再 `W0029`，`port-hopping` × `underlying-proxy` 与 Shadow TLS × `hysteria2` 两条 `E0018` 真正报出，组级 `underlying-proxy` 的组里带 `port-hopping` 的成员（配置本身给出的）加载时报 `W0023`，`salamander-password` / `gecko-password` 进内联参数的脱敏名单；`rurge-net` 的 `Connector::as_direct()`（只有 `DirectConnector` 覆盖：直连时 QUIC 自己建 socket，其余经 `connect_udp` 的载体）与 `quic` 模块（`Path::open`：直连 socket 复用 `DirectConnector::bind_udp`，`tos ≠ 0` 时发送不经 quinn-udp，`ecn=off` 去掉两侧的标记，链式载体（占位地址 `192.0.2.1:443`、不做 PMTU 探测、放不下就丢），端口跳跃层（每次换到另一个端口、上一个 socket 留到下一跳）；socket 层的发送错误一律丢包——非 `WouldBlock` 的错误会让 quinn 的连接挂死；`dial`：每条连接一个 quinn endpoint、`Transport::STANDARD`（空闲 30 秒、保活 10 秒、8 / 20 MiB 窗口、Cubic）、`connection_error` 的 `quic: …` / `tls: …` 文本；`testing` 的 `TestCert` / `EchoServer` / `MultiPort`）与 `quic::h3`（自写的最小 HTTP/3 客户端：只开控制流、SETTINGS 与等待、QPACK 静态表 + 字面量 + Huffman 解码（照 RFC 9204 附录 A 与 RFC 7541 附录 B 录入并独立核对）、HEADERS / DATA、可取消安全的写、GOAWAY；`testing` 的 `H3Server`）；`rurge-proto` 的 `QuicTls` / `build::quic_tls`（与 `TlsClient` 共用构造，只用 TLS 1.3）；新 crate `rurge-proto-quic`：`session`（`Dialer`（`with_layer`；`port-hopping` 遇上非直连的连接器在构建时失败，组中继派生的 `M (via R)` 因此被略去）、`Protocol`、`Session`：在请求自己的 future 里单飞拨号与握手、只关还开着的连接、按租约计的 60 秒空闲关闭、失效连接重试一次、跟随底层策略的去处（`Connector::route()`）；`testing::ServerPort`：能重启并回 stateless reset 的服务端端口）与 `hysteria2`（`Hysteria2Outbound`：HTTP/3 认证（失败以 0x101 关闭，`Hysteria-UDP` 按 Go 的 `ParseBool` 读）、TCP（`0x401`，任何非 0 状态都是拒绝）、UDP（会话号按出站递增、分片与重组、`Hysteria-UDP: false` 时打开载体即失败、换连接沿用会话号）、`obfs`（Salamander：BLAKE2b-256 用 RFC 7693 的自测核对；Gecko：长头包拆 2–8 片、512–1200 字节、至多 8 组在途、8 秒过期；混淆口令不对只表现为超时）；`testing::FakeHysteria2`（线上编解码与混淆都独立实现））；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_hysteria2.rs`；`common::harness_edited` 缩短跳跃间隔）与不作门禁的吞吐基准（Windows 回环、release：每方向明文约 70–80 MiB/s、Salamander 约 56–66 MiB/s）；bin 不输出 `quinn` / `quinn_proto` 的日志，`quinn_udp` 留 WARN；能力表翻转 `hysteria2`；测试里"未实现的协议"的例子从 `hysteria2` 换成 `tailscale`；`tests/interop` 对 sing-box 1.14.2 `hysteria2` 入站（无混淆、Salamander、Gecko）的互操作用例（CI 已在三平台装 sing-box，不用改）。M7b（TUIC）已完成——`rurge-config::spec::TuicSpec`（`tuic` 与 `tuic-v5` 共用一份：`TuicVersion::V4 { token }` / `V5 { uuid, password }`，都是 `Secret`；`uuid` 与 `vmess` 共用 `read_uuid`；缺凭据或 UUID 不对是 `E0018` 且不引用取值，另一个版本的凭据与 `udp-relay` / `tfo` 是 `W0028`，`ecn` 生效；`port-hopping` × `underlying-proxy` 与 Shadow TLS × `tuic` / `tuic-v5` 两条 `E0018` 真正报出；格式不对的这两种行从只报 `W0007` 变成加载错误；`required` 移到 `spec/mod.rs` 与 `trust-tunnel` 共用）；`rurge-proto-quic` 的 `tuic`（`TuicOutbound`：一份实现，按版本分支；认证在连接建好后写进一条单向流、不等回应——v5 的 token 是 TLS 导出的 32 字节（label 为 UUID 的原始字节、context 为口令；导出失败时拨号失败，不 panic），v4 是 BLAKE3(token)（只加 `blake3` 的依赖边）；TCP 开流即写 Connect，v5 立即交出隧道、v4 等 `04 ff REP`；v5 没有错误码，"码 0、空原因、服务端还没转发过任何东西"的关闭读作 `tuic: authentication failed`，v4 的 0xfffffff0 ～ 0xfffffff3 四个码三条文本（认证失败、认证超时、协议错误）；UDP：服务端接受 QUIC 数据报时 `native`，否则每个包一条单向流、经每条连接一个的写任务（至多 64 个排队、满了丢包、从不截断一个包），v5 分片与重组（每个关联至多 8 个包在重组）、v4 不分片（放不下的包 `InvalidInput`），关联号按连接分配（v5 从 0 递增、v4 随机 32 位，换连接时取新号），载体走时经同一个写任务的队列发 `Dissociate`（排在已交出的包之后，满了不发），认证被拒时一次发送失败，IPv4 映射的来源按 IPv4 读；心跳只在有转发时发（v5 每 3 秒一个数据报、服务端不收数据报时不发，v4 每 10 秒一条单向流）；`testing::FakeTuic`（v4 / v5、两种 UDP 模式、认证失败的关闭、流的扣留与放行，线上编解码独立实现）；测试用的日志捕获移到共用的 `test_log`）；`rurge-engine` 的工厂分支（两种关键字一个分支）与经引擎的端到端用例（`tests/outbounds_tuic.rs`；`tests/common` 收了四个与协议无关的辅助函数）；能力表翻转 `tuic` / `tuic-v5`；`tests/interop` 对 sing-box 1.14.2 `tuic` 入站（v5，只能覆盖 `native` 模式）与官方 tuic 0.8.5 服务端（v4；GPL，只作为外部进程运行；`RURGE_TEST_TUIC_SERVER`，只在 Linux CI 上装）的互操作用例——这个发布没有 GitHub 的 `digest`，CI 那一步的 SHA-256 是占位值，项目所有者填上之前 Linux 的 CI 任务在这一步失败。
```

换成

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、发送时本机地址或路由失效或握手无回应时换新载体（网络变化后自愈）、网络变化入口、同一私钥与 peer 只留一条隧道，节与载体设置都相同的策略共用）、`stream`、`dns`（隧道内 DNS 与缓存；每个问题 2 秒内重发、先等隧道第一次握手）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。M4c（external）已完成——`rurge-config::spec::ExternalSpec`（`args` 是 `Secret`；两个 `external` 策略写了同一个 `local-port` 是 `E0018`；`args` 进内联参数的脱敏名单）、订阅导入的 `external` 行一律跳过（`W0023`）；`rurge-platform::process`（Unix 进程组与 `killpg`，Windows Job Object）；`rurge_proto::external`（`ExternalOutbound`：第一次用到时拉起、输出写 `<数据目录>/external/<策略名>.log` 且超过 1 MiB 时在拉起前轮转、子进程环境去掉代理变量并设 `NO_PROXY=*`、程序退出后下次用到时再拉起且两次拉起至少隔 2 秒、每 500 ms 连一次本机端口最多 6 次；`ProcessHook` / `ProcessGroup` 与 `NoProcessGroups`；程序自己退出时连同它启动的进程一起结束）；`rurge-engine` 的工厂分支、`EngineShared.processes` / `externals` 与 `Engine::stop_external_programs`；bin 的 `PlatformProcesses` 与退出流程最后停掉全部外部程序；只用于测试的工作区成员 `tests/external`（`rurge-external-tests`：极小的 SOCKS5 辅助程序 `socks-helper` 与真实拉起它的用例）；能力表翻转 `external`（`W0007` 不再因 `external` 出现）。M4 至此完成。M5（UDP 路径）按三份计划推进（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）：M5a 已完成——`rurge_net::connector::PacketSocket`（按包收发、带地址的 UDP 载体）与 `Connector::open_udp`（`DirectConnector`：每个地址族一个未连接的 socket，忽略 Windows 的 ICMP 不可达报错）；`Outbound::udp()` / `open_udp()` 与 `UdpSupport`；DIRECT 与 `socks5` / `socks5-tls` / `external` 的 UDP（`udp-relay`，`W0029` 退役）；`ChainConnector::open_udp`（链式 UDP 载体）；`rurge-inbound` 的 SOCKS5 UDP ASSOCIATE（`UdpClient`、`UdpAdmission`、`Dialer::admit_udp` / `associate`）；`rurge-engine` 的 UDP 流水线（`udp` 模块：一条流 = 关联 + 目标、按"关联 × 出站"共用载体的全锥、60 秒 / DNS 10 秒回收、1024 流 / 4096 关联的上限）、请求记录与 API 的 `transport`、`block-quic`（`W0029` 退役）与 `udp-policy-not-supported-behaviour`、QUIC Initial 识别、`PROTOCOL` 规则按传输层匹配 `TCP` / `UDP`；`FakeSocks5` 与 `tests/external` 辅助程序的 UDP ASSOCIATE；对 sing-box `socks` 入站的 UDP 互操作用例。M5b（TLS 族的 UDP）已完成——`rurge-proto` 的 `stream_udp`（一条字节流上按包收发：请求头随第一个包发出、写那个包的目标；`trojan` 的 UDP ASSOCIATE 与 `anytls` 的 UDP over TCP v2 两种封装）、`trojan` / `anytls` 的 `open_udp`（全锥）、`vmess` 的命令 2（`vmess::udp`：每个目标一条 VMess 连接、随它的第一个包建立、每个数据报一个分块，对称型）；`FakeTrojan` / `FakeAnyTls` / `FakeVmess` 的 UDP 与 `rurge_proto::testing::udp_echo_server`；经引擎的端到端用例（`tests/udp_tls_family.rs`）；对 sing-box（三种）与 xray（vmess）的 UDP 互操作用例。M5c（WireGuard 的 UDP 与其余）已完成——`rurge-proto-wireguard` 的 `TunnelUdp`（隧道里每个地址族一个 UDP socket、第一次发往该族时绑定、全锥，目标名经隧道 DNS 或本机解析）与 `Stack::udp_bind` / `check`；`rurge_net::packet_datagram`（把 `PacketSocket` 变成一条到固定目标的 `Datagram`）与 `ChainConnector::connect_udp`，`wireguard` 的载体经 `underlying-proxy`（底层策略不载 UDP 时拨号失败，`W0029` 退役）；启动时 peer 连不上的告警每个策略的每个 peer 5 分钟至多一次（M4b 延后事项 #15）；`rurge_policy::udp_probe`（经策略的 UDP 向 `hostname@ipv4` 问一次 A 记录）、`Engine::test_udp` 与 `POST /v1/policies/test` 结果里的 `udp` 键（`test-udp` / `proxy-test-udp`，不保存、不参与组的选择）；`smart` 计入 UDP（载体打不开算失败、第一个回包算首字节、3 秒无回包只在 53 / 443 端口算失败、UDP 不换成员）；`dns-follow-interface`（`rurge_net::connector::Via` / `ResolveVia`、`Resolver::lookup_via`：策略自己的解析经它的 `interface` 问普通 DNS 服务器、答案另存，配了加密 DNS 时不跟随；没有 `interface` 时 `W0028`）；对 sing-box WireGuard 端点的 UDP 互操作。M5 至此完成。M5 合并后的复查（2026-10-02，M5 设计第 23 节）改了：Windows 上写了 `interface` 的策略能开 UDP 载体；发给客户端失败只丢那个包；发往上游时出站不收或系统拒发的包只丢那一个（`lost_alone`），其余发送失败的流记为失败并歇 5 秒，一次发送至多 15 秒（`SEND_TIMEOUT`，超时作废载体）；TCP keepalive（`rurge_net::socket::keep_alive`，监听与 `DirectConnector` 都开）；名字目标的回包按端口归流，`smart` 计入 UDP 的发送失败、3 秒规则只看发往地址的流；重建注册表时沿用内置 DIRECT；`wireguard` 跟随底层策略的变化（`Connector::route` / `rurge_net::connector::Route`、`PolicyRegistry::relay_now`）；开 UDP 流时的解析限时 10 秒；每个关联排队至多 1 MiB（`Queued`）、一次打开失败等着它的流一起失败；双栈监听上 IPv4 客户端的回包发往映射地址。M6（Shadowsocks / Snell / HTTP/2 族）按三份计划推进（M6a Shadowsocks → M6b Snell → M6c HTTP/2 族）：M6a（Shadowsocks）已完成——`rurge-config::spec` 的 `SsSpec`（`SsMethod`：AEAD 五种、`none`、SS 2022 两种；2022 的 `password` 按冒号拆成逐层的 Base64 密钥、最后一段是用户密钥，不合法是 `E0018` 且不引用取值；`udp-relay`、`udp-port`）与 `ObfsOpts`（与 Snell 共用；`obfs-host` 缺省服务器主机名、`obfs-uri` 缺省 `/`）、`NotImplemented`（取代 `legacy_vmess` 标记：vmess 旧握手与 `ss` 流式旧方法都是 `W0007` + REJECT，流式方法每种每次加载一条，会话日志 `policy protocol not implemented: ss (<method>)`）、`obfs-host` 进内联参数的脱敏名单；`rurge_proto::transport::obfs`（simple-obfs 的 `http` / `tls`，自写模板，`Stack` 的一层：connect → shadow-tls → obfs → tls → ws）；`rurge_proto::shadowsocks`（`ShadowsocksOutbound`：AEAD 分块流与 `none`、SS 2022（BLAKE3 子密钥、请求头块与填充、应答的类型 / 时间戳 / 回显 salt 校验、SIP023 多用户身份头）、UDP（`udp-relay=true` 发往 `udp-port`，全锥；2022 的分离头、按服务端 session 的防重放窗口））；新依赖只有 `blake3`；`rurge_proto::testing` 的 `FakeShadowsocks`（AEAD、2022 含身份头、UDP、两种 obfs）与 `accept_obfs`；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_shadowsocks.rs`）；能力表翻转 `ss`（流式旧方法除外）；测试里"未实现的协议"的例子从 `ss` 换成 `hysteria2`；`tests/interop` 对 shadowsocks-rust `ssserver`（固定版本 v1.25.0，`RURGE_TEST_SSSERVER`）与 sing-box `shadowsocks` 入站的互操作用例。M6b（Snell）已完成——`rurge-config::spec` 的 `SnellSpec`（`SnellVersion` 只有 V4 / V5；`version` 1–6、缺省 1，1–3 与 6 是 `NotImplemented::SnellVersion`：`W0007` + REJECT，每个版本每次加载一条，会话日志 `policy protocol not implemented: snell v<n>`；`psk` 是 `Secret`；v4 / v5 的 `obfs` 只有 `http`；`mode` 只对 v6 校验）；`rurge_proto::snell`（`SnellOutbound`：`kdf`（Argon2id 取前 16 字节，在阻塞线程上算；工作区依赖 `argon2` 0.6，已在依赖树里）、`record`（`SnellStream`：每方向一个 salt、7 字节加密头、首帧 256–511 字节填充与负载密文交错、每帧至多 0x3FFF、空帧结束一个方向、复用时计数器延续）、`tunnel`（请求头随首段负载写出，`reuse=false` 发 Connect `0x01`、`reuse=true` 发 ConnectV2 `0x05`；应答 `00` / `02`；干净结束的连接在 `Drop` 时回池，没结束的在后台补完再回池；池里取出的失效连接换新连接重发一次）、`pool`（每个出站至多 8 条空闲连接、空闲 60 秒回收）、`udp`（命令 `0x06`：每个载体一条自己的连接、从不进池，每个数据报一帧，全锥；`udp-port` 只解析（`W0029`），UDP 一律走主端口））；`LazyHead` 改为泛型（`LazyHead<S = BoxedStream>`，加 `get_mut` / `into_inner`）；`rurge_proto::testing` 的 `FakeSnell`（`SnellScript`：TCP、ConnectV2 复用、UDP、obfs http、拒绝、每条连接的请求数上限）；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_snell.rs`）；能力表翻转 `snell`（v4 / v5）；`tests/interop` 对 sing-box `snell` 入站（`version: 5`，同时接受 v4 客户端；sing-box 固定版本从 1.14.1 升到 1.14.2）与 Surge 官方 snell-server v5.0.1（只有 Linux 版，`RURGE_TEST_SNELL_SERVER`，只在 Linux CI 上装）的互操作用例。M6c（HTTP/2 族）已完成——`rurge-config::spec::h2`（`H2ConnectSpec`：凭据写在端口之后或命名、命名的优先，`headers`、`max-streams` 缺省 3 且至少 1、`udp-relay`；`TrustTunnelSpec`：`username` / `password` 只读命名写法、必填（`E0018`）；两者 `alpn` 固定 `h2`（写了别的 `W0028`），`headers` 里 HTTP/2 禁止的连接专用字段与 `host` 各 `W0028` 并去掉；`trust-tunnel` 的 `h3=true` 是 `W0029`、M7 之前按 HTTP/2 连，`udp-relay` 是 `W0028`）；`rurge_proto::h2pool`（`H2Pool`：每个出站一个池，每条 TLS + HTTP/2 连接至多 `max-streams` 条流且不超过服务端的上限、流数自己计，新连接收到服务端 SETTINGS 之前只承载一条流（得知 SETTINGS 的 PING 5 秒内没有回应即不再分配新流），在途的每次拨号算作 `max-streams` 个位置、放不下的请求再拨（至多 8 条在途），连接窗口为 `max-streams` × 1 MiB（至少 4 MiB），没有流的连接空闲 60 秒关闭，收到 GOAWAY 的连接不再分配新流，请求没得到应答时用 PING 检查连接是否还活着（5 秒），死了就换新连接；`H2Stream`：按流控窗口分段写、读即释放容量、END_STREAM 半关闭；`StackDial`：ALPN 不是 `h2` 时 `<协议>: the server does not speak HTTP/2`）；`rurge_proto::h2connect`（`H2ConnectOutbound`：CONNECT + Basic，`headers` 替换同名的自生成字段、`<random-string>` 按请求生成；407 是 `h2-connect: proxy authentication required`；`capsule`（RFC 9297 的 capsule 与 varint）与 `udp`（`udp-relay=true` 时的 CONNECT-UDP：每个目标一条 extended CONNECT 流、对称型，masque 默认模板的路径，DATAGRAM capsule，其它 capsule 与 context id 丢弃，超过 65527 字节的数据报不发，服务端没通告 extended CONNECT 时发往每个目标的第一个包失败，空闲 60 秒关流））；`rurge_proto::trust_tunnel`（`TrustTunnelOutbound`：同一套 CONNECT，`user-agent: rurge`，407 是 `trust-tunnel: authentication failed`，不载 UDP）；`TlsClient::wrap_negotiated` / `Stack::open_negotiated`（取协商出的 ALPN）；`rurge_proto::testing` 的 `FakeH2Proxy`（`H2ProxyScript`：Basic 与 407、拒绝、服务端流数上限、GOAWAY、extended CONNECT 与 CONNECT-UDP、多余的 capsule、`user-agent` 必填、不参与 ALPN、双向 TLS）；`rurge-engine` 的两个工厂分支与经引擎的端到端用例（`tests/outbounds_http2.rs`）；能力表翻转 `h2-connect` 与 `trust-tunnel`；`tests/interop` 对 TrustTunnel endpoint v1.1.0（只有 Linux / macOS 版，`RURGE_TEST_TRUSTTUNNEL`；sing-box 1.14.2 的 `http` 入站只说 HTTP/1，所以 endpoint 同时充当 `h2-connect` 普通 CONNECT 的参照）的互操作用例；CONNECT-UDP 没有可用的参照服务端。M6 至此完成；合并后的复查（M6 设计第 18 节）又修了会话池的串行拨号与 SETTINGS 到达前静默断掉的连接、Snell 的 `udp-port`（只解析）与重拨时只唤醒最后一个轮询者、CONNECT-UDP 的 `:authority`（取 TLS 的名字）、`headers` 的值进 `Debug`、ss 首字节前被重置时的提示、`obfs-uri` 脱敏，并把 CI 钉在 1.89（`RUSTUP_TOOLCHAIN=1.89.0`；此前 CI 每次装最新 stable，三个平台都在 clippy 一步失败，测试与互操作从未执行过）。M7（QUIC 族）按三份计划推进（M7a QUIC 地基 + Hysteria 2 → M7b TUIC → M7c HTTP/3 族与 DoQ / DoH3）：M7a 已完成——新依赖只有 `quinn` 0.11.12（`default-features = false`，`runtime-tokio` + `rustls-ring`，随之引入 quinn-proto / quinn-udp；不用 crates.io 的 `h3`，M7-D5），`blake2` 0.10 沿用依赖树里的；`rurge-config::spec` 的 `QuicOpts`（只有 `port_hopping: Option<PortHopping>`；`alpn` 留在 `TlsOpts`、QUIC 类缺省 `h3`，`ecn` 留在 `CommonOpts`）、`Hysteria2Spec`（`password` 必填、`Secret`；`download-bandwidth` 整数 Mbps；`Hysteria2Obfs` 的 Salamander / Gecko 互斥，都是 `Secret`）与 `ProtoSpec::quic()`：`port-hopping` 写法错误是 `E0018`，`port-hopping-interval` 小于 5 是 `W0012` 并按 5、没有 `port-hopping` 时 `W0028`，`tfo` / `udp-relay` 在 `hysteria2` 上是 `W0028`，`ecn` 在它上面不再 `W0029`，`port-hopping` × `underlying-proxy` 与 Shadow TLS × `hysteria2` 两条 `E0018` 真正报出，组级 `underlying-proxy` 的组里带 `port-hopping` 的成员（配置本身给出的）加载时报 `W0023`，`salamander-password` / `gecko-password` 进内联参数的脱敏名单；`rurge-net` 的 `Connector::as_direct()`（只有 `DirectConnector` 覆盖：直连时 QUIC 自己建 socket，其余经 `connect_udp` 的载体）与 `quic` 模块（`Path::open`：直连 socket 复用 `DirectConnector::bind_udp`，`tos ≠ 0` 时发送不经 quinn-udp，`ecn=off` 去掉两侧的标记，链式载体（占位地址 `192.0.2.1:443`、不做 PMTU 探测、放不下就丢），端口跳跃层（每次换到另一个端口、上一个 socket 留到下一跳）；socket 层的发送错误一律丢包——非 `WouldBlock` 的错误会让 quinn 的连接挂死；`dial`：每条连接一个 quinn endpoint、`Transport::STANDARD`（空闲 30 秒、保活 10 秒、8 / 20 MiB 窗口、Cubic）、`connection_error` 的 `quic: …` / `tls: …` 文本；`testing` 的 `TestCert` / `EchoServer` / `MultiPort`）与 `quic::h3`（自写的最小 HTTP/3 客户端：只开控制流、SETTINGS 与等待、QPACK 静态表 + 字面量 + Huffman 解码（照 RFC 9204 附录 A 与 RFC 7541 附录 B 录入并独立核对）、HEADERS / DATA、可取消安全的写、GOAWAY；`testing` 的 `H3Server`）；`rurge-proto` 的 `QuicTls` / `build::quic_tls`（与 `TlsClient` 共用构造，只用 TLS 1.3）；新 crate `rurge-proto-quic`：`session`（`Dialer`（`with_layer`；`port-hopping` 遇上非直连的连接器在构建时失败，组中继派生的 `M (via R)` 因此被略去）、`Protocol`、`Session`：在请求自己的 future 里单飞拨号与握手、只关还开着的连接、按租约计的 60 秒空闲关闭、失效连接重试一次、跟随底层策略的去处（`Connector::route()`）；`testing::ServerPort`：能重启并回 stateless reset 的服务端端口）与 `hysteria2`（`Hysteria2Outbound`：HTTP/3 认证（失败以 0x101 关闭，`Hysteria-UDP` 按 Go 的 `ParseBool` 读）、TCP（`0x401`，任何非 0 状态都是拒绝）、UDP（会话号按出站递增、分片与重组、`Hysteria-UDP: false` 时打开载体即失败、换连接沿用会话号）、`obfs`（Salamander：BLAKE2b-256 用 RFC 7693 的自测核对；Gecko：长头包拆 2–8 片、512–1200 字节、至多 8 组在途、8 秒过期；混淆口令不对只表现为超时）；`testing::FakeHysteria2`（线上编解码与混淆都独立实现））；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_hysteria2.rs`；`common::harness_edited` 缩短跳跃间隔）与不作门禁的吞吐基准（Windows 回环、release：每方向明文约 70–80 MiB/s、Salamander 约 56–66 MiB/s）；bin 不输出 `quinn` / `quinn_proto` 的日志，`quinn_udp` 留 WARN；能力表翻转 `hysteria2`；测试里"未实现的协议"的例子从 `hysteria2` 换成 `tailscale`；`tests/interop` 对 sing-box 1.14.2 `hysteria2` 入站（无混淆、Salamander、Gecko）的互操作用例（CI 已在三平台装 sing-box，不用改）。M7b（TUIC）已完成——`rurge-config::spec::TuicSpec`（`tuic` 与 `tuic-v5` 共用一份：`TuicVersion::V4 { token }` / `V5 { uuid, password }`，都是 `Secret`；`uuid` 与 `vmess` 共用 `read_uuid`；缺凭据或 UUID 不对是 `E0018` 且不引用取值，另一个版本的凭据与 `udp-relay` / `tfo` 是 `W0028`，`ecn` 生效；`port-hopping` × `underlying-proxy` 与 Shadow TLS × `tuic` / `tuic-v5` 两条 `E0018` 真正报出；格式不对的这两种行从只报 `W0007` 变成加载错误；`required` 移到 `spec/mod.rs` 与 `trust-tunnel` 共用）；`rurge-proto-quic` 的 `tuic`（`TuicOutbound`：一份实现，按版本分支；认证在连接建好后写进一条单向流、不等回应——v5 的 token 是 TLS 导出的 32 字节（label 为 UUID 的原始字节、context 为口令；导出失败时拨号失败，不 panic），v4 是 BLAKE3(token)（只加 `blake3` 的依赖边）；TCP 开流即写 Connect，v5 立即交出隧道、v4 等 `04 ff REP`；v5 没有错误码，"码 0、空原因、服务端还没转发过任何东西"的关闭读作 `tuic: authentication failed`，v4 的 0xfffffff0 ～ 0xfffffff3 四个码三条文本（认证失败、认证超时、协议错误）；UDP：服务端接受 QUIC 数据报时 `native`，否则每个包一条单向流、经每条连接一个的写任务（至多 64 个排队、满了丢包、从不截断一个包），v5 分片与重组（每个关联至多 8 个包在重组）、v4 不分片（放不下的包 `InvalidInput`），关联号按连接分配（v5 从 0 递增、v4 随机 32 位，换连接时取新号），载体走时经同一个写任务的队列发 `Dissociate`（排在已交出的包之后，满了不发），认证被拒时一次发送失败，IPv4 映射的来源按 IPv4 读；心跳只在有转发时发（v5 每 3 秒一个数据报、服务端不收数据报时不发，v4 每 10 秒一条单向流）；`testing::FakeTuic`（v4 / v5、两种 UDP 模式、认证失败的关闭、流的扣留与放行，线上编解码独立实现）；测试用的日志捕获移到共用的 `test_log`）；`rurge-engine` 的工厂分支（两种关键字一个分支）与经引擎的端到端用例（`tests/outbounds_tuic.rs`；`tests/common` 收了四个与协议无关的辅助函数）；能力表翻转 `tuic` / `tuic-v5`；`tests/interop` 对 sing-box 1.14.2 `tuic` 入站（v5，只能覆盖 `native` 模式）与官方 tuic 0.8.5 服务端（v4；GPL，只作为外部进程运行；`RURGE_TEST_TUIC_SERVER`，只在 Linux CI 上装）的互操作用例——这个发布没有 GitHub 的 `digest`，CI 那一步的 SHA-256 是占位值，项目所有者填上之前 Linux 的 CI 任务在这一步失败。M7c（HTTP/3 族与 DoQ / DoH3）已完成——没有新依赖（`rurge-dns` 多了 `quinn` 的依赖边）；`rurge-config::spec` 的 `MasqueSpec`（`tls`、`quic`、`credentials: Option<MasqueCredentials>`，凭据是 `Secret`；`username` / `password` 只读命名写法、要么都写要么都不写，缺一个或写了却为空是 `E0018` 且不引用取值；`headers` / `max-streams` 是未知参数，`udp-relay` / `tfo` 是 `W0028`，`ecn` 生效，端口跳跃同 `hysteria2`）与 `TrustTunnelSpec.h3`（`read_trust_tunnel` 先读 `h3`：ALPN 按传输钉成 `h2` / `h3`，`h3=true` 时 `tfo` 是 `W0028`、`headers` 里 HTTP/3 禁止的字段按 HTTP/3 的文本告警并去掉、Shadow TLS 是 `E0018`（`shadow_tls::refuse`）；两种传输上 `port-hopping` / `port-hopping-interval` 都是 `W0028`；`h3` 的 `W0029` 退役）；`rurge-net::quic::h3` 的 extended CONNECT（`Protocol::CONNECT_UDP`，先等服务端的 SETTINGS）与 HTTP Datagram（`Client::with_datagrams` 宣告 `H3_DATAGRAM`、`RequestStream::datagrams` 按流分发；`Client::new` 照旧不宣告，Hysteria 2 的 UDP 不受影响）、更严的应答检查（`content-length` 比对、SETTINGS 的 0x08 / 0x33 只能是 0 或 1、CONNECT 的 2xx 之后再来 HEADERS 是连接错误），`Transport::keep_alive` 改为 `Option`、新增不保活的 `Transport::DNS`，`ServerPort` 搬到 `rurge_net::quic::testing`；`rurge-proto` 的 `connect_udp`（CONNECT-UDP 的路径与 capsule 编解码，`h2-connect` 与 `masque` 共用）、`http::basic_authorization` 与 `trust_tunnel::{connect_request, refused}`（两种传输共用）；`rurge-proto-quic` 的 `masque`（`MasqueOutbound`：TCP 是经典 CONNECT，每个 CONNECT 都主动带 `proxy-authorization: Basic`（手册没写字段名，rurge 的选择）；UDP 是 CONNECT-UDP——每个目标一条 extended CONNECT 流（对称型）、随发往它的第一个包打开、2xx 之后才发、包走 HTTP/3 数据报（收的时候也接受流上的 DATAGRAM capsule），放不下的包丢弃而不退回 capsule，服务端不支持 CONNECT-UDP 时只让 UDP 失败（`masque: the server does not support CONNECT-UDP`），每个目标的流空闲 60 秒以 FIN 关闭，GOAWAY、连接关闭或底层策略变了时在当前连接上重开）、`trust_tunnel`（`TrustTunnelH3Outbound`：与 HTTP/2 版同样的请求头与错误文本，不宣告 HTTP Datagram，不载 UDP，没有 HTTP/2 回落）与两者共用的 `connect`（经典 CONNECT 隧道），`Session::with_stream_cap`（`trust-tunnel` 的 `max-streams` 按每条 QUIC 连接计，满了照 HTTP/2 池的规则另拨；不看服务端的 QUIC 流数上限），`testing::FakeH3Proxy`（MASQUE 与 TrustTunnel endpoint 两种服务端、quiche 式的 GREASE、CONNECT-UDP 的各种开关）；`rurge-dns` 的 `quic://`（`upstream::doq`，RFC 9250：ALPN `doq`、报文 ID 0、EDNS(0) padding 到 128 字节的倍数（`message::doq_query`）、写完即 FIN，违规的应答以 DOQ_PROTOCOL_ERROR 关连接，放弃的查询以 DOQ_REQUEST_CANCELLED 停流）与 `h3://`（`upstream::doh3`：POST，没写路径时 `/dns-query`）两种上游，共用 `upstream::session`（每个上游一条 QUIC 连接、单飞拨号、不保活、查询中途失效的连接换新连接重问一次），`BootstrapConnector::connect_udp`，`testing::{FakeDoq, FakeDoh3}`；`rurge-engine` 的 `PipelineConnector::connect_udp` 与 `Engine::dial_internal_udp`（`encrypted-dns-follow-outbound-mode` 下 DoQ / DoH3 的 QUIC 载体作为内部 UDP 会话走规则：协议标签按端口（443 → `DOH3`，其余 → `DOQ`），DIRECT 用 DIRECT 策略自己的 UDP，REJECT 与以域名配置的代理照 TCP 告警并直连，不载 UDP 的代理让该上游这次失败；路由与 `smart` 的回报与 TCP 共用 `route_internal` / `report_internal`）、`masque` 与 `h3=true` 的 `trust-tunnel` 两个工厂分支与经引擎的端到端用例（`tests/outbounds_masque.rs`；`trust-tunnel` 的 HTTP/3 在 `tests/outbounds_http2.rs`）；能力表翻转 `masque`（`trust-tunnel` 的 `h3=true` 不再是 `W0029`），`W0026` 不再因 `h3://` / `quic://` 出现；`tests/interop` 对 masque-server v0.13.0（`RURGE_TEST_MASQUE_SERVER`，只在 Linux）、TrustTunnel endpoint v1.1.0 的 QUIC 监听（`[listen_protocols.quic]`）、AdGuard dnsproxy v0.86.0 与 CoreDNS v1.14.7（`RURGE_TEST_DNSPROXY` / `RURGE_TEST_COREDNS`，三平台；解析器不收测试 CA，用 `encrypted-dns-skip-cert-verification`）的互操作用例。M7 至此完成。
```

`CLAUDE.md`——把

```markdown
- `docs/superpowers/specs/2026-10-02-phase2-m7-quic-design.md`：阶段 2 / M7 细化设计（QUIC 族：Hysteria 2、TUIC v5 / v4、MASQUE、`trust-tunnel` 的 h3、DoQ / DoH3），细化总设计的 M7 里程碑、不一致处以它为准，关闭总设计的开放问题 Q6 并处理风险 B。三份计划的拆分（M7a QUIC 地基 + Hysteria 2 → M7b TUIC → M7c HTTP/3 族与 DoQ / DoH3）；已决事项 M7-D1 ～ D12（全部都做、TUIC v4 也做、`ecn` 跟随 quinn、自写最小的 HTTP/3 客户端而不用 crates.io 的 `h3`、参考服务端、新依赖只有 `quinn` 0.11.12、`rurge-net::quic` 与新 crate `rurge-proto-quic` 的分工、每条连接一个 quinn endpoint、客户端一律 Cubic、Gecko 照官网规范而与 Surge 是否相同未证实、`W0007` 与 `Unsupported` 的退役改到 M8）；QUIC 共用部分（配置、拨号与 TLS、UDP 层、ECN、会话、错误文本）、HTTP/3 客户端、各协议的线上格式与三层测试；第 9 节是需登记的差异，第 11 节 V1 ～ V10 是写各份计划时必须核对的事项，第 12 节是任务草图，第 14 节是 M7a 计划期的订正，第 15 节是 M7a 实施期的订正，第 16 节是 M7b 计划期的订正，第 17 节是 M7b 实施期的订正。
- `docs/superpowers/plans/2026-10-02-phase2-m7a-quic-hysteria2-plan.md`：阶段 2 / M7a（QUIC 地基 + Hysteria 2）实施计划（9 个任务）。开头「计划期决定」表记录核对 quinn / quinn-proto / quinn-udp 源码、RFC 9000 / 9114 / 9204 / 7541 / 7693、Hysteria 官网规范与参考实现得出的结论和与设计文字不同的决定（`QuicOpts` 只放 `port-hopping`、`alpn` 与 `ecn` 留在原处、间隔小于 5 秒是 `W0012`、`tfo` / `udp-relay` 是 `W0028`；`tos ≠ 0` 只在发送一侧不经 quinn-udp、端口跳跃每次换到另一个端口、链式载体放不下就丢且不做 PMTU 探测、socket 层的发送错误一律丢包；HTTP/3 只开控制流、两张码表照原文录入并独立核对；单飞拨号在请求自己的 future 里、只关还开着的连接、失效连接只对请求到来时已建好的连接重试一次、跟随底层策略的去处；认证失败以 0x101 关闭、任何非 0 的 TCP 状态都是拒绝、会话号按出站递增、`Hysteria-UDP: false` 时打开载体即失败；混淆层在最外面、Salamander 丢弃不超过 8 字节的包、Gecko 的第 9 组挤掉最旧的；`port-hopping` 遇上非直连连接器在构建时失败；"未实现的协议"的例子换成 `tailscale` 等）；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/superpowers/plans/2026-10-03-phase2-m7b-tuic-plan.md`：阶段 2 / M7b（TUIC v5 / v4）实施计划（6 个任务）。开头「计划期决定」表记录核对 TUIC v5 的 SPEC、v4（tuic 0.8.5）的协议说明、sing-box 与 tuic-server 的源码（GPL，只取协议事实）、quinn / rustls 的导出密钥与本仓库得出的结论和与设计文字不同的决定（两个版本共用一份 `TuicSpec`、另一个版本的凭据是 `W0028`、认证写进单向流后不等回应、v5 的认证失败按"码 0、空原因、尚未转发"推断、Connect 开流即写、UDP 模式按连接自动选择、`quic` 模式的包经每条连接一个的写任务、关联号按连接分配且换连接时取新号、心跳只在有转发时发且 v5 不在流上发、v4 放不下的包丢弃、IPv4 映射的来源按 IPv4 读、tuic 0.8.5 只在 Linux CI 上跑且它的 SHA-256 待项目所有者填写等）；末尾「执行期修正记录」与「延后事项」两张表。
```

换成

```markdown
- `docs/superpowers/specs/2026-10-02-phase2-m7-quic-design.md`：阶段 2 / M7 细化设计（QUIC 族：Hysteria 2、TUIC v5 / v4、MASQUE、`trust-tunnel` 的 h3、DoQ / DoH3），细化总设计的 M7 里程碑、不一致处以它为准，关闭总设计的开放问题 Q6 并处理风险 B。三份计划的拆分（M7a QUIC 地基 + Hysteria 2 → M7b TUIC → M7c HTTP/3 族与 DoQ / DoH3）；已决事项 M7-D1 ～ D12（全部都做、TUIC v4 也做、`ecn` 跟随 quinn、自写最小的 HTTP/3 客户端而不用 crates.io 的 `h3`、参考服务端、新依赖只有 `quinn` 0.11.12、`rurge-net::quic` 与新 crate `rurge-proto-quic` 的分工、每条连接一个 quinn endpoint、客户端一律 Cubic、Gecko 照官网规范而与 Surge 是否相同未证实、`W0007` 与 `Unsupported` 的退役改到 M8）；QUIC 共用部分（配置、拨号与 TLS、UDP 层、ECN、会话、错误文本）、HTTP/3 客户端、各协议的线上格式与三层测试；第 9 节是需登记的差异，第 11 节 V1 ～ V10 是写各份计划时必须核对的事项，第 12 节是任务草图，第 14 节是 M7a 计划期的订正，第 15 节是 M7a 实施期的订正，第 16 节是 M7b 计划期的订正，第 17 节是 M7b 实施期的订正，第 18 节是 M7c 计划期的订正。
- `docs/superpowers/plans/2026-10-02-phase2-m7a-quic-hysteria2-plan.md`：阶段 2 / M7a（QUIC 地基 + Hysteria 2）实施计划（9 个任务）。开头「计划期决定」表记录核对 quinn / quinn-proto / quinn-udp 源码、RFC 9000 / 9114 / 9204 / 7541 / 7693、Hysteria 官网规范与参考实现得出的结论和与设计文字不同的决定（`QuicOpts` 只放 `port-hopping`、`alpn` 与 `ecn` 留在原处、间隔小于 5 秒是 `W0012`、`tfo` / `udp-relay` 是 `W0028`；`tos ≠ 0` 只在发送一侧不经 quinn-udp、端口跳跃每次换到另一个端口、链式载体放不下就丢且不做 PMTU 探测、socket 层的发送错误一律丢包；HTTP/3 只开控制流、两张码表照原文录入并独立核对；单飞拨号在请求自己的 future 里、只关还开着的连接、失效连接只对请求到来时已建好的连接重试一次、跟随底层策略的去处；认证失败以 0x101 关闭、任何非 0 的 TCP 状态都是拒绝、会话号按出站递增、`Hysteria-UDP: false` 时打开载体即失败；混淆层在最外面、Salamander 丢弃不超过 8 字节的包、Gecko 的第 9 组挤掉最旧的；`port-hopping` 遇上非直连连接器在构建时失败；"未实现的协议"的例子换成 `tailscale` 等）；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/superpowers/plans/2026-10-03-phase2-m7b-tuic-plan.md`：阶段 2 / M7b（TUIC v5 / v4）实施计划（6 个任务）。开头「计划期决定」表记录核对 TUIC v5 的 SPEC、v4（tuic 0.8.5）的协议说明、sing-box 与 tuic-server 的源码（GPL，只取协议事实）、quinn / rustls 的导出密钥与本仓库得出的结论和与设计文字不同的决定（两个版本共用一份 `TuicSpec`、另一个版本的凭据是 `W0028`、认证写进单向流后不等回应、v5 的认证失败按"码 0、空原因、尚未转发"推断、Connect 开流即写、UDP 模式按连接自动选择、`quic` 模式的包经每条连接一个的写任务、关联号按连接分配且换连接时取新号、心跳只在有转发时发且 v5 不在流上发、v4 放不下的包丢弃、IPv4 映射的来源按 IPv4 读、tuic 0.8.5 只在 Linux CI 上跑且它的 SHA-256 待项目所有者填写等）；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/superpowers/plans/2026-10-06-phase2-m7c-h3-dns-plan.md`：阶段 2 / M7c（HTTP/3 族与 DoQ / DoH3：MASQUE、`trust-tunnel` 的 h3、DoQ / DoH3 上游与跟随出站模式的 UDP）实施计划（9 个任务）。开头「计划期决定」表记录核对 RFC 9114 / 9220 / 9297 / 9298 / 9250 / 8484、Surge 手册、masque-server / TrustTunnel endpoint / dnsproxy / CoreDNS 与 masque-go / quic-go / hickory 的源码和本仓库得出的结论与和设计文字不同的决定（MASQUE 的 TCP 用经典 CONNECT 而不是 `connect-tcp`、`proxy-authorization: Basic` 随每个 CONNECT 发出、服务端不支持 CONNECT-UDP 时只让 UDP 失败、2xx 之前不发包、放不下的包丢弃而不退回 DATAGRAM capsule（取代设计 7.1）、流上收到的 capsule 照收；`Client::new` 照旧不宣告 `H3_DATAGRAM`、数据报只给 `with_datagrams` 的客户端、发给没有数据报语义的流的数据报丢弃；`trust-tunnel` 的 h3 与 HTTP/2 同一套请求与错误文本、`max-streams` 按每条 QUIC 连接计（会话的流数上限，照 HTTP/2 池的规则另拨）、不回落 HTTP/2、不载 UDP、`port-hopping` 是 `W0028`；DoQ 照 RFC 加 EDNS(0) padding、应答 ID 不是 0 即以 DOQ_PROTOCOL_ERROR 关连接、取消的查询以 DOQ_REQUEST_CANCELLED 停流、DNS 的 QUIC 连接不保活；DoH3 用 POST、只有 `h3://` 没写路径时用 `/dns-query`、`:authority` 不带 `:443`、带用户名口令的 URL 加载时拒绝；跟随出站模式下 QUIC 载体作为内部 UDP 会话走规则、不载 UDP 的代理让该上游失败而不改走直连、REJECT 与域名代理照 TCP 直连；互操作参照 masque-server v0.13.0、TrustTunnel endpoint 的 QUIC 监听、dnsproxy v0.86.0 与 CoreDNS v1.14.7，sing-box 1.15 的预发布版不用等）；末尾「执行期修正记录」与「延后事项」两张表。
```

`CLAUDE.md`——把

```markdown
cargo test -p rurge-engine --test outbounds_http2   # 经 h2-connect / trust-tunnel 的端到端用例：凭据与拒绝、max-streams、trust-tunnel 不载 UDP、CONNECT-UDP、没有 extended CONNECT、经 underlying-proxy、重载沿用与替换
cargo test -p rurge-net quic                    # QUIC 共用部分：UDP 层（直连、tos、ecn=off、链式载体、端口跳跃）、拨号与传输参数、错误文本；自写的 HTTP/3 客户端（QPACK 静态表与 Huffman、帧、SETTINGS / GOAWAY、可取消安全的写）对回环 H3Server
cargo test -p rurge-proto-quic                  # QUIC 会话（单飞、空闲关闭、失效重试、跟随底层策略）与 hysteria2（认证、TCP、UDP 分片与重组、Salamander / Gecko 与 BLAKE2b-256 的已知答案），出站对回环假服务端（FakeHysteria2）
```

换成

```markdown
cargo test -p rurge-engine --test outbounds_http2   # 经 h2-connect / trust-tunnel 的端到端用例：凭据与拒绝、max-streams、trust-tunnel 不载 UDP、CONNECT-UDP、没有 extended CONNECT、经 underlying-proxy、重载沿用与替换；trust-tunnel 的 h3=true（HTTP/3、max-streams 按每条 QUIC 连接、重载、与 Shadow TLS 的加载错误）
cargo test -p rurge-net quic                    # QUIC 共用部分：UDP 层（直连、tos、ecn=off、链式载体、端口跳跃）、拨号与传输参数（含不保活的 Transport::DNS）、错误文本；自写的 HTTP/3 客户端（QPACK 静态表与 Huffman、帧、SETTINGS / GOAWAY、可取消安全的写、extended CONNECT 与 HTTP Datagram、content-length 比对）对回环 H3Server
cargo test -p rurge-proto-quic                  # QUIC 会话（单飞、空闲关闭、失效重试、跟随底层策略、每条连接的流数上限）与 hysteria2（认证、TCP、UDP 分片与重组、Salamander / Gecko 与 BLAKE2b-256 的已知答案），出站对回环假服务端（FakeHysteria2）
```

`CLAUDE.md`——把

```markdown
cargo test -p rurge-external-tests              # external：真实拉起测试辅助程序（socks-helper）——按需拉起、参数顺序与代理变量、再拉起与 2 秒间隔、连接被拒的重试、日志、整棵进程树的停止、经引擎的端到端、检查不拉起、重载沿用与替换
cargo test -p rurge-interop                     # 对 sing-box 1.14.2（HTTP/2 族与 TUIC v4 之外的全部协议，含 hysteria2 的无混淆 / Salamander / Gecko 与 tuic-v5）、xray（只测 vmess）、shadowsocks-rust ssserver（只测 ss）、官方 snell-server v5.0.1（只测 snell，只在 Linux）、TrustTunnel endpoint v1.1.0（测 trust-tunnel 与 h2-connect 的 TCP，只在 Linux / macOS）、官方 tuic 0.8.5 的 tuic-server（只测 tuic，只在 Linux；只认环境变量）与 OpenSSH sshd（只测 ssh，只在 Unix）的互操作测试；没装就跳过（RURGE_TEST_SING_BOX / RURGE_TEST_XRAY / RURGE_TEST_SSSERVER / RURGE_TEST_SNELL_SERVER / RURGE_TEST_TRUSTTUNNEL / RURGE_TEST_TUIC_SERVER / RURGE_TEST_SSHD / RURGE_INTEROP_REQUIRED=1）
```

换成

```markdown
cargo test -p rurge-proto-quic masque           # masque：经典 CONNECT 的隧道与每个 CONNECT 的 Basic 凭据、CONNECT-UDP（每个目标一条流、HTTP/3 数据报与流上收到的 capsule、能力检查、超长的包、畸形的 2xx、空闲 FIN、GOAWAY 与换连接、路由比较），出站对回环假服务端（FakeH3Proxy）
cargo test -p rurge-proto-quic trust_tunnel     # trust-tunnel 的 HTTP/3：经典 CONNECT 与 user-agent、headers、407、max-streams 按每条 QUIC 连接（会话的流数上限另见 session::tests::under_a_cap）、GOAWAY、重启后的重试，出站对回环假服务端（FakeH3Proxy 扮 TrustTunnel endpoint）
cargo test -p rurge-engine --test outbounds_masque   # 经 masque 出站的端到端用例：TCP 与 CONNECT-UDP、口令错误、服务端不支持 CONNECT-UDP、超长的包只丢那一个、端口跳跃、经 underlying-proxy（socks5）、URL 测速与 UDP 测试、重载沿用与替换
cargo test -p rurge-dns doq                     # DoQ：查询报文（ID 0、padding 到 128 字节的倍数、去掉 keepalive）、每个查询一条流、协议错误关连接、取消、空闲与重启后的重连、证书校验、bootstrap 与地址轮换，对回环假服务端（FakeDoq）；DoH3 的用例用过滤词 doh3（FakeDoh3）
cargo test -p rurge-engine --test pipeline      # 流水线端到端，含跟随出站模式：DoT / DoH 连接与 DoQ / DoH3 的 QUIC 载体作为内部会话走规则（经 SOCKS5 的 UDP、DIRECT、REJECT 与以域名配置的代理改走直连、不载 UDP 的代理让该上游失败）
cargo test -p rurge-external-tests              # external：真实拉起测试辅助程序（socks-helper）——按需拉起、参数顺序与代理变量、再拉起与 2 秒间隔、连接被拒的重试、日志、整棵进程树的停止、经引擎的端到端、检查不拉起、重载沿用与替换
cargo test -p rurge-interop                     # 对 sing-box 1.14.2（HTTP/2 族、TUIC v4 与 HTTP/3 族之外的全部协议，含 hysteria2 的无混淆 / Salamander / Gecko 与 tuic-v5）、xray（只测 vmess）、shadowsocks-rust ssserver（只测 ss）、官方 snell-server v5.0.1（只测 snell，只在 Linux）、TrustTunnel endpoint v1.1.0（测 trust-tunnel 的 HTTP/2 与 h3=true、h2-connect 的 TCP，只在 Linux / macOS）、官方 tuic 0.8.5 的 tuic-server（只测 tuic，只在 Linux；只认环境变量）、masque-server v0.13.0（只测 masque，只在 Linux）、AdGuard dnsproxy v0.86.0 与 CoreDNS v1.14.7（测 DoQ / DoH3 上游）与 OpenSSH sshd（只测 ssh，只在 Unix）的互操作测试；没装就跳过（RURGE_TEST_SING_BOX / RURGE_TEST_XRAY / RURGE_TEST_SSSERVER / RURGE_TEST_SNELL_SERVER / RURGE_TEST_TRUSTTUNNEL / RURGE_TEST_TUIC_SERVER / RURGE_TEST_MASQUE_SERVER / RURGE_TEST_DNSPROXY / RURGE_TEST_COREDNS / RURGE_TEST_SSHD / RURGE_INTEROP_REQUIRED=1）
```

总设计（crate 布局的注释、第 13 节 M7 的参考实现、开放问题 Q6）：

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
├── rurge-proto-quic/        # ★ M7：quinn；QUIC 会话；hysteria2（M7a）tuic tuic-v5（M7b）masque trust-tunnel(h3)
```

换成

```markdown
├── rurge-proto-quic/        # ★ M7：quinn；QUIC 会话；hysteria2（M7a）tuic tuic-v5（M7b）masque trust-tunnel(h3)（M7c）
```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| M7 | 按 M7 细化设计 M7-D6：sing-box 1.14.2 的 `hysteria2` 入站（无混淆、Salamander、Gecko）与 `tuic` 入站（v5），三平台；官方 tuic 0.8.5（v4；GPL，只作为外部进程运行），只在 Linux CI；Vincent-bin/masque-server v0.13.0（MIT）：`masque`，只在 Linux / macOS CI；TrustTunnel endpoint v1.1.0 的 QUIC 监听：`trust-tunnel` 的 h3，只在 Linux / macOS CI。端口跳跃要在服务端做 DNAT，只有回环假服务端与手工验收。M7a 已接入 sing-box 的 `hysteria2` 入站（TCP、UDP、认证失败、两种混淆）；M7b 已接入 sing-box 的 `tuic` 入站（v5：TCP、UDP、认证失败；它总是接受 QUIC 数据报，`quic` 模式只有回环假服务端）与 tuic 0.8.5 的服务端（v4：TCP、UDP、认证失败；它的发布没有 GitHub 的 `digest`，CI 里的 SHA-256 待项目所有者填写） |
```

换成

```markdown
| M7 | 按 M7 细化设计 M7-D6：sing-box 1.14.2 的 `hysteria2` 入站（无混淆、Salamander、Gecko）与 `tuic` 入站（v5），三平台；官方 tuic 0.8.5（v4；GPL，只作为外部进程运行），只在 Linux CI；Vincent-bin/masque-server v0.13.0（MIT）：`masque`，服务端只有 Linux 版，只在 Linux CI；TrustTunnel endpoint v1.1.0 的 QUIC 监听：`trust-tunnel` 的 h3，只在 Linux / macOS CI。DoQ / DoH3 上游按 M7c 的计划：AdGuard dnsproxy v0.86.0 与 CoreDNS v1.14.7（都是 Apache-2.0），三平台。端口跳跃要在服务端做 DNAT，只有回环假服务端与手工验收。M7a 已接入 sing-box 的 `hysteria2` 入站（TCP、UDP、认证失败、两种混淆）；M7b 已接入 sing-box 的 `tuic` 入站（v5：TCP、UDP、认证失败；它总是接受 QUIC 数据报，`quic` 模式只有回环假服务端）与 tuic 0.8.5 的服务端（v4：TCP、UDP、认证失败；它的发布没有 GitHub 的 `digest`，CI 里的 SHA-256 待项目所有者填写）；M7c 已接入 masque-server（TCP 的普通 CONNECT、CONNECT-UDP、认证失败；它在 HTTP/3 上不读 capsule，服务端不支持 CONNECT-UDP 的情形只有回环假服务端）、TrustTunnel endpoint 的 QUIC 监听（`[listen_protocols.quic]`；`trust-tunnel` 的 h3：TCP、认证失败）与 dnsproxy、CoreDNS（DoQ 与 DoH3：A 与 AAAA、一条连接上的多个查询、`h3://` 的缺省路径；CoreDNS 对报文 ID 与 edns-tcp-keepalive 从严）。sing-box 1.15 的预发布版给 `http` 入站加了 HTTP/2、HTTP/3 与 CONNECT-UDP，三平台都有，是将来的跨平台参照，alpha 期间不用 |
```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| Q6 | MASQUE 与 trust-tunnel 的参考服务端 | 已决（2026-10-01）：`trust-tunnel` 的 h2 用 TrustTunnel endpoint v1.1.0（Linux / macOS；M6c），它也是 `h2-connect` 普通 CONNECT 的参照；M7 细化设计 M7-D6 定下另外两项——MASQUE 用 Vincent-bin/masque-server v0.13.0（MIT，Linux / macOS），`trust-tunnel` 的 h3 用 TrustTunnel endpoint v1.1.0 的 QUIC 监听（Linux / macOS；能否这样启用在写 M7c 的计划时核对，不行就退到假服务端与手工验收） |
```

换成

```markdown
| Q6 | MASQUE 与 trust-tunnel 的参考服务端 | 已决（2026-10-01）：`trust-tunnel` 的 h2 用 TrustTunnel endpoint v1.1.0（Linux / macOS；M6c），它也是 `h2-connect` 普通 CONNECT 的参照；M7 细化设计 M7-D6 定下另外两项——MASQUE 用 Vincent-bin/masque-server v0.13.0（MIT；M7c 核对：服务端只有 Linux 版，只在 Linux CI），`trust-tunnel` 的 h3 用 TrustTunnel endpoint v1.1.0 的 QUIC 监听（Linux / macOS；M7c 核对：`vpn.toml` 里一个 `[listen_protocols.quic]` 表即启用，UDP 与 TCP 同一个端口号） |
```

- [ ] **Step 4: 核对**

- `README.md` 与 `README_en.md` 的状态一段、特性表与路线图内容一致；PRD 不需要改（它没有写到这一层细节）。
- 兼容性清单的 `masque`、`trust-tunnel`、DNS 各行与 P1、P2、P8、P9、P12–P17 的文字一致；文档里引用的错误文本与代码一致。

- [ ] **Step 5: 门禁与提交**

跑门禁。副本上最后一次全工作区门禁：fmt、clippy 通过，`cargo test --workspace --no-fail-fast` 68 个测试二进制 2084 通过、0 失败、3 忽略。

```bash
git add Cargo.lock tests/interop .github docs README.md README_en.md CLAUDE.md
git commit -m "docs: M7c HTTP/3 族与 DNS——兼容性清单、手工验收、README 与 CLAUDE.md；masque-server、TrustTunnel h3、dnsproxy 与 CoreDNS 的互操作"
```

## 验收对照（设计第 1 节与第 7 节）

| # | 验收项 | 由谁保证 |
| - | ------ | -------- |
| 1 | `masque` 的 TCP 与 UDP 对参考实现转发通过 | Task 9 的 masque-server 用例（Linux CI）；Task 3 / 4 的回环往返；Task 8 经引擎 |
| 2 | `trust-tunnel` 的 `h3=true` 对参考实现转发通过 | Task 9 的 TrustTunnel endpoint 用例（Linux / macOS CI）；Task 5 的回环；Task 8 经引擎 |
| 3 | `h3://` 与 `quic://` 上游解析正常 | Task 9 的 dnsproxy 与 CoreDNS 用例（三平台 CI）；Task 6 的回环 |
| 4 | 跟随出站模式下 DoQ / DoH3 按规则走 | Task 7 |
| 5 | 能力表不再为 `masque` 出 `W0007`，`h3=true` 不再是 `W0029`，`h3://` / `quic://` 不再是 `W0026` | Task 8 `check_knows_masque` / `check_knows_http2`；Task 6 的配置用例 |
| 6 | 门禁全绿 | 各任务的门禁 |
| 7 | 需要真实节点的项目进手工验收清单 | Task 9：`docs/acceptance/phase2-manual.md` 的 M7c 一节 |

## 执行期修正记录

| # | 任务 | 与计划的出入 | 原因 |
| - | ---- | ------------ | ---- |
| 1 | 全部 | 门禁的测试一步自 Task 5 起加 `-j 8`（`timeout 1500 cargo test -j 8 --workspace --no-fail-fast`；Task 3、4 是第一次没跑成之后加 `-j 4` 重跑的）。只限制编译的并行度，跑的用例不变 | 本机在缺省的 32 个并行编译下内存不足：Task 3、4 的第一次全量测试都在编译阶段因 LLVM out of memory 没跑成 |
| 2 | Task 1 | 已知偶发失败的名单加上 `rurge-proto` 的 `socks5::tests::the_bound_address_may_be_a_domain_or_ipv6_and_unknown_types_are_refused`（它的连接时限只有 200 ms），遇到同样重跑一次 | 全量测试的负载下失败过一次，单跑与重跑都通过；M7c 不碰这段代码 |
| 3 | Task 2 | （5adb2e5）读连接数据报的任务还没被轮询就被丢弃时，各流的数据报同样结束（结束它们的守卫在建任务时建好：`datagram::read` 由 `async fn` 改为返回 future 的普通函数）；`RequestStream::datagrams` 只给 CONNECT 请求，其它请求以 `h3: HTTP datagrams need a CONNECT request` 拒绝，先于等服务端的 SETTINGS。补四条用例：`a_datagram_reading_dropped_before_it_ran_ends_the_flows`、`only_a_connect_has_datagrams`、`dropping_the_client_ends_the_datagram_flows`、`a_datagram_for_a_stream_whose_receive_side_ended_is_dropped` | Task 2 评审：守卫建在 future 里面，任务第一次运行之前被中止就不会结束各流；RFC 9297 2——数据报的含义由请求决定，而计划的代码对任何请求都交出数据报；两处既有行为没有用例 |
| 4 | Task 3 ～ 5 | （531243b）`Lost` 按发生了什么判定：HTTP/3 客户端把服务端对一个请求说了什么带在错误里（`h3::rejected`、`h3::going_away`、`Client::unprocessed`，不解析文本），`connect::failure` 只把连接已结束、因 GOAWAY 没有发出、被以 H3_REQUEST_REJECTED 拒绝、流号不低于 GOAWAY 界限的请求算作失效，GOAWAY 界限之下在自己流上的失败是终局；CONNECT 的隧道在读的时候也推进写了一半的帧（另一个任务正停在写上时除外）；有流数上限的会话里，失效重试只落在失效那条之后拨的连接上，"请求到来时已建好"按拨号序号判（等来空位的请求同样算）；MASQUE UDP 的长度边界（恰好放得下的包）与"什么也没发出"在线上核对（假服务端计 CONNECT-UDP 流上收到的 DATA 字节）。补 15 条用例；测试里可能挂住的等待加上时限，靠调度顺序与墙钟的几处断言换成不依赖它们的 | Task 3 评审：计划 P8 的"GOAWAY 之后被拒的请求重试"只在 GOAWAY 先被处理时成立（它与被拒的先后是竞态，两线程的用例复现了），而连接正在离开时任何失败又都被重试；"半帧只随写推进"的义务落在每个调用方身上。Task 4、5 评审：长度边界没有钉住；有上限时重试可能落到同样失效的旧连接上，等来空位的请求不重试。放到 Task 5 之后才做，是因为 Task 4、5 还在改这些文件，Task 5 又把判定与隧道挪进共用的 `connect.rs`（MASQUE 与 `trust-tunnel` 一并修到） |
| 5 | Task 4、5 | （5d040ad）重构，行为不变：`hysteria2`、`tuic` 与 `masque` 的 UDP 载体共用新模块 `rurge_proto_quic::carrier`（`RouteClock`、`Inbox`、`bounded` 与常量 `INBOX` / `ROUTE_EVERY`）；MASQUE 与 `trust-tunnel` 的 HTTP/3 共用 `connect` 的 `Proxy` / `Http3` / `tunnel`。三种协议原有的用例原样通过 | Task 4 评审：照计划写出的是载体辅助代码的第三份逐字副本，承接事项 C7 "三者没有共同的部分可抽"的前提不成立（本计划延后事项 #11 与 M7b 计划延后事项 #9 就此了结）；Task 5 评审：两处 `Http3` 与限时发 CONNECT 的几行重复 |
| 6 | Task 6 | （c737591）带用户名口令的 `quic://` / `tls://` / `tcp://` 上游被拒（任何位置出现 `@`），文本只引用最后一个 `@` 之后的部分；`h3://` 因口令而解析不了时文本不再引用输入；`quic://` 结尾只收一个 `/`。DoH3 的请求被以 H3_REQUEST_REJECTED 拒绝也算连接失效，查询因连接失效而失败时那条连接不再收新查询（重问才落得到新连接上）。悄悄失效的连接：查询的数据写出后没有答案地结束、连接自那时起过了 2 个往返加 25 ms 仍没收到任何数据报时，不再收新查询（`Progress` 与 `Unanswered`）。域名的多个地址各得"剩余时间 ÷ 剩余地址数"。DoH 与 DoH3 共用应答检查（`check_head` / `check_body`）。补 25 条用例（含按 IP 字面量校验证书的一条），假服务端加"重启后不回认得出的 stateless reset"等开关 | Task 6 评审：口令可能进入上游名、日志与 `W0026` 的文本；计划 P16 的判定漏了被拒的请求（531243b 的细化没有带到 rurge-dns 自己的那份会话）；连接悄悄失效后要等 30 秒的空闲超时、其间这个上游的每个查询都超时；第一个不应答的地址吃光整个时限；会话最难的几个分支没有用例 |
| 7 | Task 7 | （cce7859）跟随出站模式下 DNS 的 QUIC 载体被规则送到不载 UDP 的代理时照 `udp-policy-not-supported-behaviour` 办：`REJECT`（缺省）时与计划相同，`DIRECT` 时经 DIRECT 出站自己的 UDP 发出、请求记录写 `policy does not support UDP; sent through DIRECT`、不向 `smart` 组回报。**取代计划期决定 P17 的"不载 UDP 的代理让这个上游这次失败，不改走直连"**——Review Focus 第 5 条与 Task 7、Task 9 里照 P17 写的文字随之过时，以兼容性清单与设计第 19 节第 10 条为准。补 8 条用例：流水线 5 条（`DIRECT` 的设置、链底 REJECT 仍先判、DIRECT 别名带着自己的 socket 设置、交给解析器的 `via <策略>: ` 前缀两条），`smart` 3 条（UDP 载体与 TCP DNS 会话的回报、回落时不回报） | Task 7 评审：只配了 DoQ / DoH3、规则又把 DNS 送进 `http` / `ssh` / `trust-tunnel` 这类代理时 DNS 整个不可用，用户写了 `DIRECT` 也救不回来，而普通的 UDP 流照这个设置办；P17 的三处要点（DIRECT 用自己的 UDP、前缀、`smart` 回报）没有用例 |
| 8 | Task 7 | （cce7859）`rurge-engine` 的 `[dev-dependencies]` 加 `socket2.workspace = true`（`Cargo.lock` 多一行依赖边，没有新 crate） | 上一条的用例要写一个记下 TOS 的 `SocketHook`，它的方法签名里有 `&socket2::Socket`。Global Constraints 的"新增的依赖边只有 `rurge-dns → quinn`"因此多出这一条只用于测试的边 |
| 9 | Task 9 | 文档同步（接在 Task 9 之后的一次提交）：Task 9 的文档是照计划原文转写的，其中照 P17 等处写的已与代码不符——兼容性清单、手工验收、`CLAUDE.md` 与 `tests/interop/README.md` 照本表各条（含终审的 #12 ～ #15）改写，设计新增第 19 节；兼容性清单补登 MASQUE 的两处限制（UDP 负载的余量不到 1200 字节的后果、CONNECT-UDP 流与 TCP 隧道共用流额）、设计第 18 节第 3 条的两处宽松与不设上限的 QUIC 出站的流控限制；手工验收补 MASQUE 经 `underlying-proxy` 与混用加密上游两项；`docs/api/phase2.md` 补「跟随出站模式下 DNS 的内部会话」一节；CI 里 TrustTunnel 那一步的名字补上 HTTP/3 | Task 9 评审与全分支评审：登记不全；执行期的各条订正还没有进文档 |
| 10 | Task 9 | tuic 0.8.5 的 SHA-256 钉住：`.github/workflows/ci.yml` 里 M7b 留下的占位值换成 `e4b5d5dcc15209f28de1c8a9ea12edba8a71c35442b4c53177309f728406726c`，取自发布自带的校验文件 `tuic-server-0.8.5-x86_64-linux-musl.sha256sum`（只读了这份文本，没有下载二进制；GitHub 没有给这个发布的资产公布 `digest`）；`tests/interop/README.md`、`CLAUDE.md` 与设计第 19 节第 12 条随之更新，M7b 计划的延后事项 #1 与 #6 注明已钉住 | M7b 计划延后事项 #1 留给项目所有者的决定，按 2026-10-06 的交代（决定自行做出）定下：占位值让 Linux 的 CI 任务在这一步失败，它后面的各步——本计划的 masque-server、TrustTunnel h3、dnsproxy 与 CoreDNS，以及测试本身——在 Linux 上都跑不到。校验文件与二进制出自同一个发布，钉住的是此后下载到的仍是同一个文件 |
| 11 | 终审 | 全分支评审的结论是"修后可合并"：M7c 自己的代码没有必须修的缺陷；一处重要问题继承自 M7a（设计 3.2 的接收窗口：三条读端停住的隧道占满 20 MiB 的连接窗口，同一条连接上其余隧道随之停住）。裁决：设了流数上限的会话（`trust-tunnel` 的 HTTP/3）现在就修（#14），不设上限的（`masque`、`hysteria2`、`tuic`）登记为已知限制、留给 M8（延后事项 #14）。评审的其余几条与各任务评审排到最后的事项分三次提交修掉（#12 ～ #14） | 全分支评审与各任务评审排到最后的事项 |
| 12 | 终审 | （4f9bbbc）rurge-net 的载体。① QUIC 经载体时，载体此刻发不了就让 quinn 等：`Carried::try_send` 返回 `WouldBlock`，轮询器在载体可发时唤醒它（`Datagram::poll_send_ready`，缺省"总是就绪"，直连的载体转给它的 socket）；说不出何时可发的载体照旧丢这个包；发送的错误仍然只丢包、不交给 quinn。② `packet_datagram` 记下载体最近一次发送失败，由下一次接收报出一次——只丢这一个包的失败（`lost_alone`：放不下、系统拒发）除外，照旧丢弃、不告诉任何人；`Carried` 记下自己以什么错误结束，握手期间拨号以载体的原话失败（握手完成之后仍是 `quic: the UDP path to the server failed`）；跟随出站模式的内部 UDP 会话在载体报错时以失败结束——请求记录写载体的原话，解析器拿到 `via <策略>: <原话>`，并算作 `smart` 成员的一次失败。`lost_alone` 从引擎挪到 `rurge_net::connector`，UDP 流与载体共用 | 任务评审排到最后的两条：当前线程的运行时里，新 socket 还没被告知可写，连接的第一个包（1200 字节的 Initial）被当成"满了"丢掉，拨号多等约 1 秒（Task 6、8）；惰性载体到发送时才出现的错误（`masque: the server does not support CONNECT-UDP` 等）被吞掉，解析器只看到超时、请求记录写"完成"（Task 7）。比评审说的范围宽：只改 `packet_datagram` 时原话到不了解析器（quinn 把 socket 的失败一律报成 endpoint 没了）；不把 `lost_alone` 的失败排除在外，经 MASQUE 的 DoQ 第一次握手会从"慢一个重传"变成"拨号失败" |
| 13 | 终审 | （7f51cc7）rurge-dns。① DoQ / DoH3 的拨号由会话自己的后台任务完成，不随发起它的查询被放弃而中止；时限是发起它的那个查询的截止时间，到时限不算失败（不留结论），还在等、自己还有时间的查询重拨；会话结束时中止。② 连接一关闭会话就放手（连同它的 socket 与载体），不等下一个查询；它是怎样结束的，记给等那次拨号的查询当作共同的失败。③ 等拨号的查询遇到一建成就在离开（GOAWAY）的连接时仍拿到它、在上面失败，不再重拨。④ 最后一个地址不另设计时器；重试那一次遇到的失效同样让连接不再收新查询；悄悄失效的时间下限改为 max(3 × 往返时间 + 50 ms, 100 ms)。⑤ `h3://` 里任何位置的 `@` 都拒绝（文本同其它协议）；`https://` 的上游名、`Debug` 与错误文本不带用户名口令（请求照旧用原 URL）；DoH / DoH3 的错误里引用的 `content-type` 至多 200 个字符 | 全分支评审：混用加密上游时，别的上游先答、扇出放弃其余的查询，DoQ / DoH3 的冷拨号每次都随之中止——这个上游一直连不上，每次查找白发一次握手；`content-type` 原样进错误与日志（可长到 64 KiB）。任务评审排到最后的：已关闭的连接要等下一个查询才放手载体（内部 UDP 会话一直显示为活动，代理上的 UDP 关联一直开着）；最后一个地址的计时器把"被放弃"变成等待者共享的失败；下限的余量太薄；`h3://` 里"以数字开头、后接 `/` 的口令"混得过去；`https://` 的上游名带口令。② 的后半与 ③ 是 ① 带出来的：发起拨号的查询也成了等待者，不处理的话握手后立即关连接的服务器会被每个查询反复握手 |
| 14 | 终审 | （59a499a）① 设了流数上限的会话，连接的接收窗口取 max(上限 × 每流窗口, 20 MiB)（`capped_transport`，`Transport::apply`）：`trust-tunnel` 的 `h3=true` 在 `max-streams` 为 3（缺省）时 24 MiB、4 时 32 MiB、1 与 2 时仍是 20 MiB；不设上限的会话不变。② 测试：几处可能挂住的等待加上时限，超时用例的墙钟界统一为 5 秒，一条什么也没断言的断言换成真的（策略带一个错口令，断言它不出现在错误与记录里）。③ 小清理：三处与 `carrier::Packet` 相同的别名删掉，`spec/masque.rs` 的私有函数改名 `read_masque_credentials`，`hop_every` 与 `a_failed_session` 收进引擎测试的 `tests/common` | 全分支评审的那处重要问题（#11 的裁决）与各任务评审排到最后的测试、命名小问题 |
| 15 | 终审 | ee9d4ea：终审修正之后的一次小提交。① `https://` 的 DoH URL 里 `@` 只能出现在主机之前：路径、查询串或片段里出现 `@` 的是 `W0026`，文本只引用最后一个 `@` 之后的部分，这些字符要写成百分号编码（用例 `an_at_sign_beyond_the_authority_of_an_https_url_is_refused`、`an_https_server_with_an_at_sign_beyond_its_authority_is_no_upstream`）。② `hysteria2` 的用例 `obfuscation_that_does_not_match_fails_by_the_time_of_the_request` 的墙钟界由 2 秒放宽到 5 秒 | 对 #12 ～ #14 三次提交的复查：各条都已落实，没有必须修的新问题，结论是文档提交之后可以合并。留下的小问题里这两处顺手改掉——#13 的 ⑤ 留了一处同类的口子（`https://` 的用户名或口令里没有转义的 `/` 让 authority 提前结束，后半段连同 `@` 与主机被读成路径，进上游名、日志与 `Debug`），#14 的 ② 漏了这一处写成 `took <` 的断言——，其余登记在延后事项 #24、#25、#28 ～ #33 与 #35 |
| 16 | 门禁 | 最终门禁（#15 的提交之后）：fmt、clippy 通过，`timeout 1500 cargo test -j 8 --workspace --no-fail-fast` 68 个结果行 2163 通过、0 失败、3 忽略（三个吞吐基准），没有异常退出、没有重跑 | 合并之前在将要合并的那棵树上再跑一次（ee9d4ea 加上这次的文档提交；文档与 CI 文件不参与构建） |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | 新的互操作用例都还没有对真实二进制跑过（就绪日志、命令行写法、Corefile 的引号都只照源码读出）；masque-server 只有 Linux 版 | 第一次 CI 运行时核对 |
| 2 | MASQUE 不乐观发送（Surge 与 Google quiche 在 2xx 之前就发；masque-server 为此缓存 8 个 / 64 KiB）：每个目标的第一个包多等一个往返 | 有需要时再议 |
| 3 | 服务端不支持 QUIC 数据报时 MASQUE 的 UDP 直接失败，不像 sing-box 那样整条改用 DATAGRAM capsule（Surge 同样判为不可用） | 有用户报告再说 |
| 4 | 服务端能力的检查在每个目标开流时做，Surge 在建立连接时（P9，已登记） | 接受 |
| 5 | CONNECT-UDP 的畸形 2xx 以 `H3_REQUEST_CANCELLED` 取消而不是 RFC 要求的 `H3_MESSAGE_ERROR`（自写的 h3 客户端没有"以指定码中止"的公开接口） | 以后顺手补 |
| 6 | `trust-tunnel` 的 HTTP/3 不看服务端的 QUIC 流数上限（P12，已登记）；没有 HTTP/3 → HTTP/2 的自动回落（TrustTunnel 协议文档说客户端 MAY 做） | 接受 |
| 7 | 跟随出站模式下 QUIC 载体的协议标签只按端口推断，不在 443 的 DoH3 被标成 DOQ（P17，已登记） | 需要时让上游把协议告诉连接器 |
| 8 | DoQ 没做 RFC 9250 §4.4 的"复用前看空闲时间离超时是否够远"（quinn 不暴露最后收包的时刻），靠"已关闭就换 + 失效重试一次"与悄悄失效的判定（执行期修正记录 #6）兜住；不用 0-RTT | 接受 |
| 9 | `https://` 的 DoH 上游在没写路径时仍发往 `/`，与 `h3://` 的缺省 `/dns-query` 不一致（Surge 的缺省没有写明） | M8 再议 |
| 10 | RFC 8484 §5.1 的 `Age` 修正 TTL：DoH 与 DoH3 都没做 | M8 |
| 11 | ~~`tuic` 与 `hysteria2` 的载体辅助与错误映射几乎逐字相同（M7b 延后事项 #9）：MASQUE 的 UDP 结构不同，三者抽不出共同部分~~ 已在执行中解决：三者的载体共用 `rurge_proto_quic::carrier`（执行期修正记录 #5） | 已解决 |
| 12 | 互操作的 DNS 用例开着 `encrypted-dns-skip-cert-verification`（解析器没有注入测试根证书的入口） | 接受 |
| 13 | sing-box 1.15（alpha）的 `http` 入站开始支持 HTTP/3 CONNECT 与 CONNECT-UDP，可作 MASQUE 的三平台参照 | 发布正式版后再加 |
| 14 | 不设流数上限的 QUIC 出站（`masque`、`hysteria2`、`tuic`）的流控：接收窗口每条流 8 MiB、每条连接 20 MiB，连接级的额度要等数据被读走才放回，三条读端停住的隧道就把它占满，这个策略其余 TCP 隧道的下行随之停住（UDP 走数据报，照常）；设了流数上限的 `trust-tunnel` 的 HTTP/3 已把连接窗口取为 max(`max-streams` × 8 MiB, 20 MiB)，代价是一条连接最坏缓冲 `max-streams` × 8 MiB 的未读数据（3 时 24 MiB，16 时 128 MiB，是它的 HTTP/2 传输的 8 倍；执行期修正记录 #14，设计第 19 节第 11 条；兼容性清单已登记） | M8：在"给接收缓冲占的内存设上限"与"读端停住的隧道拖不住别的隧道"之间定取舍，对全部 QUIC 出站统一处理 |
| 15 | MASQUE 能发的 UDP 负载等于 QUIC 数据报的余量（P9 不改用 capsule）：路径 MTU 探测抬高之前不到 1200 字节，`masque` 自己经 `underlying-proxy`（链式载体不做探测）时一直不到——内层 QUIC 的 Initial（至少 1200 字节；跟随出站模式下经它的 DoQ / DoH3 握手也是）在后一种情形总被拒，满尺寸的 WireGuard 包在任何路径上都放不下（兼容性清单已登记） | M8：给 MASQUE 连接更大的初始 MTU，或在链式载体上改用 DATAGRAM capsule |
| 16 | 每个目标的 CONNECT-UDP 流（空闲 60 秒才关）与 TCP 隧道共用连接的流额，额满时新的 TCP CONNECT 等待（P8）：UDP 同时发往很多目标时可能挤住 TCP（兼容性清单已登记） | 有用户报告再议 |
| 17 | 被调用方放弃的内部 DNS 拨号（`dial_internal` 与 `dial_internal_udp`：DNS 一侧每个地址只给一份时间；DoT / DoH 的拨号还随被扇出放弃的查询中止，DoQ / DoH3 的到发起它的查询的截止时间为止）不留请求记录，也不向 `smart` 组回报；调用方给的时限没有传给出站（固定用 10 秒）。TCP 一侧是既有的做法，UDP 照搬 | M8：TCP 与 UDP 一起改（时限取两者较小的；被放弃时记一条失败） |
| 18 | 互操作夹具里 `locate()` 与 `…_or_skip()` 各有十份形状相同的副本（每个参照服务端一份，本计划加了三份） | 有空时收成 `lib.rs` 里的一对辅助函数 |
| 19 | `rurge-net` 的 `quic/h3/mod.rs` 约 2200 行，其中约 2000 行是用例 | 有空时把用例挪到 `h3/tests.rs` |
| 20 | 两份单飞会话——策略的（`rurge_proto_quic::session`）与 DNS 的（`rurge_dns::upstream::session`，因依赖方向另写）——规则已经分叉：前者在请求自己的 future 里拨号、按拨号序号判"请求到来时已建好"、只在失效那条之后拨的连接上重试，后者由会话的后台任务拨号、按"一取到就是现成的连接"判，另有悄悄失效的判定、连接关闭即放手与"为它建的连接"的规则（执行期修正记录 #6、#13） | M8：把共同的部分放进 `rurge_net::quic`（两边都依赖它，方向允许） |
| 21 | 三个 QUIC 假服务端（`FakeHysteria2`、`FakeTuic`、`FakeH3Proxy`）起服务、记录连接、关闭与重启的脚手架是三份相近的代码 | 有空时整理（同 #18） |
| 22 | 悄悄失效的 DNS 连接要到一次查找结束才被认出（执行期修正记录 #6）：同一次查找里每秒一次的重发仍发往它，恢复在下一次查找；要在同一次查找里恢复，得按最老的在途查询来判 | 接受（兼容性清单已登记） |
| 23 | `Client::request` 在请求头还没发完时就失败（HEADERS 被连接级的流控卡住）不带流号：这时流号不低于 GOAWAY 界限、又不是以 H3_REQUEST_REJECTED 被停止的请求判为终局而不是失效（执行期修正记录 #4） | 接受；要补就让请求的错误带上流号 |
| 24 | `rurge-config` 通用的 `invalid`（`W0012`：`` invalid value `<原文>` for `<键>`, using default ``）原样引用取值：`encrypted-dns-server` / `doh-server` / `dns-server` 里 scheme 不认识的项（写错成 `htps://user:pass@host/…`、`http://…`、`doq://…` 之类）整项进告警，`http-api` / `external-controller-access` 解析不了时整个取值（`key@host:port`，连同 key）进告警；`redact_profile` 不处理 DNS 服务器 URL 里的用户名口令，`GET /v1/profiles/current` 的脱敏输出里照样显示。都是阶段 1 就有的行为，在 rurge-dns 之外，终审的修正（执行期修正记录 #13、#15）没有动它们；写对了的 URL 走不到 `invalid`。它们与"凭据不进告警与错误文本"的约束相抵（告警打到 stderr，`rurge check` 与 `POST /v1/profiles/check` 也给出） | M8（裁决：不在合并前修）：`invalid` 对这几个键只引用最后一个 `@` 之后的部分；`redact_profile` 抹掉 DNS 服务器 URL 里的用户名口令 |
| 25 | `rurge_config::general::EncryptedDns` 派生 `Debug`，`url` 是配置里的原文：带用户名口令的 `https://` 项在配置对象的 `Debug` 里原样可见（没有找到把它打进日志的地方） | M8，同 #24 一并改 |
| 26 | 经载体的 QUIC 连接在握手完成之后载体才失效时，请求或查询看到的仍是 `quic: the UDP path to the server failed`；载体的原话只在握手期间报出，跟随出站模式下另记进内部 UDP 会话的请求记录（执行期修正记录 #12）。要把原话也带给握手之后的请求，得让连接的错误拿得到载体 | 有用户报告再说 |
| 27 | DoH3 的连接一建成就在离开（GOAWAY）时，等那次拨号的查询里，在别的等待者让会话放手这条连接之后才来看的，可能各自再拨一次（每个查询至多一次，与修正前相同）；一建成就被关闭的情形已做到所有等待者共用那一次拨号（执行期修正记录 #13） | 接受 |
| 28 | `ConnectionReset` 一类的载体接收错误，三处的处理不一致（执行期修正记录 #12 带出）：quinn 不理会它（`endpoint.rs` 对这种错误直接跳过），连接照用这条载体；引擎却已把跟随出站模式的内部 UDP 会话以失败结束并标记上游失败（`dns_pipeline.rs` 的 `CountingDatagram::poll_recv`；首字节之前还给 `smart` 的成员记一次失败），`Carried` 也已把它记作载体的失败（`quic/carrier.rs`）——之后握手若因别的原因失败，`quic::connect`（`quic/dial.rs`）对任何握手错误都先问载体，会拿这条陈旧的错误当原因。已确认的来源两处：`h2-connect` 的 CONNECT-UDP 流被服务端重置之后的写（下一个包另开一条流，连接照常）；Windows 上改走直连的载体（已连接的 UDP socket）收到 ICMP 端口不可达（os error 10054）。影响限于请求记录、活动列表与一次成员失败回报，DNS 本身照常 | M8：对 quinn 会忽略的错误不结束会话、不记失败（或把载体自己说的 `ConnectionReset` 换一种再交给 quinn）；`quic::connect` 只在握手错误是"endpoint 没了"时取载体的话 |
| 29 | 经 `underlying-proxy` 的 QUIC 策略，载体延后报出的发送错误没有 `via <策略>: ` 前缀：`ChainConnector::connect_udp`（`rurge-policy` 的 `cell.rs`）直接交出 `packet_datagram`，只有打开时的错误带前缀（引擎的 DNS 一侧已经加了）。`P = hysteria2, …, underlying-proxy=U` 的拨号错误于是是 U 的原话（如底层 `vmess` 连不上服务器时的 `tls: …`），读起来像 P 自己的握手失败（执行期修正记录 #12 带出） | M8：在 `ChainConnector::connect_udp` 里给载体的错误补上前缀 |
| 30 | `Carried::try_send` 的兜底在竞争窗口里仍会丢包：多线程的运行时上，载体在 `poll_send`（`Pending`）与紧接着的 `poll_send_ready` 之间变为可写时，这个包被丢弃而不是重试（`quic/carrier.rs`）。概率很小，QUIC 自己恢复（是连接的第一个包时多等一个重传，约 1 秒） | M8：在这里再试一次 `poll_send` |
| 31 | 包装载体对 `poll_send_ready` 的转发没有用例：删掉引擎里 `FinishOnDrop` 或 `CountingDatagram` 的转发（`dns_pipeline.rs`），改走直连的 DoQ / DoH3 载体会退回"第一个包丢失、慢约 1 秒"，现有用例都不会失败（它们不看耗时） | M8：补一条看首包或看耗时的用例 |
| 32 | `packet_datagram` 的接收一侧仍把载体自己的结束文本换成 `the carrier through the chain has closed`：载体自己的话（如 `socks5: the proxy closed the UDP association`）只有从发送一侧失败时才到得了上层 | M8：接收任务结束时留下载体的原话 |
| 33 | `rurge-proto-wireguard` 的用例里还有两处 `< 2 s` 的墙钟断言（`outbound.rs`），与执行期修正记录 #14 的 ② 同类，不在那次的范围里 | 顺手时放宽到 5 秒 |
| 34 | `https://` 的 DoH URL 里的用户名口令不起认证作用（阶段 1 / M2b 以来的行为，文档同步时发现）：请求原样用配置里的 URL，rurge 不从中生成 `Authorization` 头——HTTP/1.1 上它们根本不发，HTTP/2 上被连同 `user:pass@` 写进 `:authority`（RFC 9113 8.3.1 不允许，严格的服务端可能拒绝）。Surge 认不认这种写法没有核对 | M8：发请求之前从 URL 里取走用户名口令（改发 `Authorization: Basic`，或加载时告警并忽略） |
| 35 | 全量测试偶发的异常退出（Windows 的 `STATUS_HEAP_CORRUPTION`）：已三次落在 `rurge-engine` 的 `outbounds_tls_family` 上（Task 6 的门禁、终审修正的门禁与它的第一次重跑；后两次停在不同的用例上），另有一次在 `rurge-api` 的 `api`；再重跑都通过，复查时把崩过的两个二进制各跑 40 次没有复现。这个二进制只走 trojan / vmess / anytls / socks5 的 TCP，终审修正改动的运行期代码（QUIC 的载体、`packet_datagram`、DNS 的 QUIC 会话、流控窗口）不在它的路径上；工作区禁用 unsafe，来源只能在依赖或运行库里 | M8：用 WER 的 LocalDumps 或 PageHeap 抓一次现场 |
| 36 | DNS 上游 URL 的两处边角（终审补丁时看到，各 scheme 相同，本轮之前就有）：全角的 `＠`（U+FF20）不算 `@`，带它的写法不走「带用户名口令即拒绝」的路径；端口写错时 `invalid port` 的错误原样引用那一段，段里若有口令的一部分会被带出 | M8，与 #24 的 `[General]` 诊断回显取值一并处理 |
