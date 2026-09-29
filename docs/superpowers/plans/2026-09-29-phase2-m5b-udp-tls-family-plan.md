# 阶段 2 / M5b「TLS 族的 UDP」Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `trojan`、`anytls`、`vmess` 三种出站承载 UDP：`trojan` 在一条连接上做 UDP ASSOCIATE、`anytls` 走 UDP over TCP v2，两者全锥；`vmess` 用命令 2、每个目标一条连接，对称型。经 rurge 的 SOCKS5 UDP 进来的流量按规则分到这三种策略时都能往返。

**Architecture:** 引擎不动（M5a 的流水线只看 `Outbound::udp()` 与 `open_udp()`）。`rurge-proto` 新增 `stream_udp`：一条字节流上按包收发的 `PacketSocket`，请求头随第一个包发出、写那个包的目标，每个包带自己的地址与长度；`trojan` 与 `anytls` 各用它的一种封装（`Framing::Trojan` / `Framing::Uot`）。`vmess` 把拨号所需的状态放进一个共享的 `Dialer`，UDP 载体 `vmess::udp::VmessUdp` 在每个目标的第一个包时用命令 2 开一条连接，每个数据报一个分块。三个回环假服务端学会各自的 UDP，服务端一侧的中继（`testing::udp`）独立于生产代码的封装实现。

**Tech Stack:** Rust 1.89 / edition 2024；不新增任何依赖（`tokio`、`tokio-util` 已在 `rurge-proto` 的依赖里）。

**Spec:** `docs/superpowers/specs/2026-09-29-phase2-m5-udp-design.md`（M5-D1、D2、D5；第 7 节 `trojan` / `vmess` / `anytls` 三行；第 9 ～ 12 节中 M5b 的部分；第 15 节 V6；第 16 节 M5b 草图；第 17、18 节 M5a 的订正）与总设计 `docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`。与本计划「计划期决定」表不一致处，以该表为准；写计划时一并写进设计文档新增的第 19 节。

## Global Constraints

- MSRV **1.89**，edition 2024。`unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 的两个函数例外（本计划不碰）。**本计划不新增任何 unsafe**。
- 依赖方向不变：`rurge-proto → rurge-net → rurge-config`；引擎、入站、策略层都不改。**不新增依赖**。
- **测试绝不碰公网**：只用回环 + 端口 0 + 有界等待；不用固定 sleep 当同步手段（轮询 + 截止时间；只有"断言这段时间里什么也没发生"时才等一段固定时间）。UDP 回显、假服务端都只在 127.0.0.1 上。**任何带 `url-test` / `fallback` / `load-balance` / `smart` 组的测试配置，`proxy-test-url` 与 `internet-test-url` 都必须指向回环**——引擎用例的 `Profile::text` 已默认指向 `http://127.0.0.1:9/`，不要删掉。
- **测试与任何命令都不得修改本机的系统代理、注册表、网络设置，不得注册真实服务 / 计划任务**：不在 CLI 测试夹具之外运行 `rurge run --system-proxy`；不运行不带 `--dry-run` 的 `rurge service install | uninstall`。
- **不在本机下载或安装任何东西**（不装 sing-box、xray，不 `rustup target add`、不 `cargo install`）。互操作用例在本机没有二进制时按既有约定跳过。
- **载荷与凭据永不外泄**：UDP 载荷不进日志、错误文本与请求记录；trojan / anytls 的口令（及其哈希）、vmess 的 id 不进日志、错误文本与 `Debug`。
- rurge 专有的运行时选项只经命令行与环境变量提供，不扩展 Surge 配置格式（FR-CFG-17）。与 Surge 的每一处行为差异都登记进 `docs/surge-compatibility-matrix.md`（Task 4）。
- 日志、错误文本、CLI 输出、代码注释用英文；文档与提交标题用中文。注释密度、命名、惯用法与相邻代码保持一致；注释里不写评审轮次的标签。
- 提交信息结尾两行：`Co-Authored-By: <执行者自己的署名>` 与 `Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th`。**不 push、不 merge**，不 amend。
- 每个任务结束的门禁（全部通过才算完成）：

  ```bash
  RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check \
    && cargo clippy --all-targets -- -D warnings \
    && timeout 1500 cargo test --workspace --no-fail-fast
  ```

  `timeout` 不能省：`rurge-dns` 的一个用例曾让测试进程以 100% CPU 空转数小时（M3a「延后事项」#20）。测试二进制异常退出而没有失败用例时（`STATUS_ACCESS_VIOLATION`、`STATUS_HEAP_CORRUPTION` / `0xc0000374`、段错误——本机已知的既有问题，M3b 计划 P21），或整轮被 `timeout` 杀掉时，重跑一次并保留两次的日志，**不要在任务里去修它**。已知偶发失败的计时类用例（`rurge-dns` 的 `a_partial_result_completes_aaaa_in_the_background` 与 `bootstrap::tests::stale_entries_are_served_and_refreshed_once`、`rurge` 的 `run::watch_reloads_rules_on_change`）同样重跑。**编译器（`rustc` / 链接器）自己崩溃、报 PDB 损坏时多半是磁盘满了**：先看 `df -h /d`，删 `target/debug/incremental` 再重跑。
- 第三方 crate 的 API 以本机源码为准：`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`（`tokio-1.53.1`）。
- 本机的 bash 处理不了超过约 8 KB 或含反斜杠的 heredoc（`\\` 会被改写）：新文件与含反斜杠的改动一律用写文件的工具落盘，不用 heredoc。

## Review Focus

设计没有逐条写到、而最可能伤到使用者的五类输入或失败方式；每一条都在负责它的任务里配了用例。

1. **P2P 联机与语音经 trojan / anytls**（对方从另一个地址发来）：服务端那一端的任何来源都要原样送回客户端（全锥）。用例：Task 1 `anyone_may_answer_through_trojan`（出站库与经引擎各一条）；Task 2 `anyone_may_answer_through_anytls`（同上）。
2. **经 vmess 的回包归属**：服务端不说回包来自谁，回包只能算作那个目标的；同一个客户端发往两个目标要各开一条连接，而不是把第二个目标的包塞进第一条连接。用例：Task 3 `udp_opens_one_connection_per_target`、`every_answer_counts_as_the_targets`、`through_vmess_every_answer_is_the_targets`、`udp_goes_through_vmess_one_connection_per_target`。
3. **大包**：UDP 数据报最长约 64 KiB，而 VMess 一个分块只装 16368 字节——发不出去的包要以清楚的错误失败（流随之按失败处理、5 秒后重试，M5a 设计第 18 节第 6 条），不能拆成两个分块（服务端会当成两个数据报）；收到比缓冲区大的数据报时要整包跳过、不能把流读乱。用例：Task 3 `udp_over_tls_and_a_websocket_and_a_datagram_too_long`；Task 1 `datagrams_come_back_with_their_source`（第二个数据报放不下被跳过，后面的照常读到）。
4. **服务端关掉连接**（节点重启、空闲超时）：载体的收包要以错误结束，引擎据此不再复用它（M5a 设计第 18 节第 4 条），下一条流重新开载体。用例：Task 1 `the_servers_end_is_the_carriers_end`。
5. **名字目标与 IDN**：发往域名的 UDP 由服务器解析，名字按 A-label 发出；发不出去的名字不拨号、也不吃掉请求头（下一个包照常带上它）。用例：Task 1 `udp_over_a_websocket_with_a_name`、`an_unsendable_name_is_refused_and_the_head_waits`；Task 3 `udp_over_tls_and_a_websocket_and_a_datagram_too_long`。

## 计划期决定

写计划时对照设计、参考实现的源码与协议文档（sing `common/uot/protocol.go`、`common/uot/conn.go`、`common/metadata/serializer.go`，sing-vmess `client.go` 的 `clientPacketConn`，trojan-gfw 的协议文档 `trojan-gfw.github.io/trojan/protocol`，2026-09-29 查阅）与本仓库源码核对后定下的事；与设计文档文字不同的，写进设计文档第 19 节。

**本计划里的代码不是凭空写的。** 全部 4 个任务的改动在仓库的一份副本上按任务顺序真实做了一遍（副本用自己的构建目录，不与本仓库的 `target/` 混用），最后一次全工作区门禁见 Task 4 的 Step 5。计划里新文件的全文取自副本上该任务的提交，修改处的"把 … 换成 …"由脚本从相邻两个任务提交的差异生成，并在拼好之后按计划的顺序套到开工前的源码上逐字核对过——计划文本与验证过的代码一字不差。每个任务 Step 2 的"预期失败"是只把该任务的用例块（及写明的前置改动）套到上一个任务的状态上、真实跑出来的。

| # | 事项 | 决定与依据 |
| - | ---- | ---------- |
| P1 | V6：trojan 的 UDP | trojan-gfw 协议文档：请求头 `hex(SHA224(password)) CRLF CMD ATYP ADDR PORT CRLF`，UDP ASSOCIATE 的 `CMD` 是 `0x03`；此后每个数据报两个方向都是 `ATYP ADDR PORT LENGTH CRLF PAYLOAD`（`LENGTH` 两字节大端，地址与 SOCKS5 相同）。文档没说请求头里的地址对 UDP 有何意义：服务端按每个包自己的地址转发；参考客户端（sing-box）写的是第一个包的目标，rurge 照做——请求头等第一个包到了再与它一起写出（一次写），名字按 A-label 发出、由服务端解析 |
| P2 | V6：anytls 的 UDP over TCP v2 | sing 的 `uot`：流的目标是 `sp.v2.udp-over-tcp.arpa`、端口 0（`RequestDestination`）；流上先写请求 `isConnect`（1 字节布尔）加 `SocksaddrSerializer` 地址（SOCKS5 的类型号 1 / 4 / 3，地址在前、端口在后）；非连接模式下此后每个数据报两个方向都是 `AddrParser` 地址（**类型号 0 = IPv4、1 = IPv6、2 = 名字**，名字前一字节长度，端口在后）加两字节大端长度加载荷，没有 CRLF。rurge 用非连接模式（全锥），请求里的目标同样写第一个包的目标（非连接模式下它只是说明）。这条流像 TCP 的流一样占一个会话、用完归还会话池 |
| P3 | V6：vmess 的命令 2 | sing-vmess 的 `clientPacketConn`：命令 `CommandUDP`（2），选项与 TCP 相同（ChunkStream + ChunkMasking），每次写就是一个分块、一个数据报；读到的数据一律当作拨号时的目标发来的，写时不看地址。所以每个目标一条连接（M5-D5，对称型）；一个分块最多 16368 字节（`chunk::MAX_PAYLOAD`），更长的数据报以 `vmess: a datagram longer than 16368 bytes` 失败而不是拆开（服务端会当成两个数据报） |
| P4 | 两种流式封装共用一个载体 | 新模块 `rurge_proto::stream_udp`（设计没指定位置）：`StreamUdp` 持有拆开的读写两半；发送时锁住写的一半，请求头还没发就与这个包一起写出；接收时读地址、长度（trojan 还有 CRLF）与载荷，放不下的数据报读过去丢掉，来源不是主机名的丢掉，流的结束是载体的结束（`<协议>: the server closed the UDP connection`），引擎据此不再复用它（M5a 设计第 18 节第 4 条）。`resolve` 用默认实现：名字交给服务器 |
| P5 | vmess 的载体 | `open_udp` 不拨号：`VmessUdp` 在每个目标的第一个包时拨号（`get_or_try_init`，同一目标同时只拨一次），拨号失败或写失败时忘掉这条连接、错误交给引擎（流按失败处理，5 秒后下一个包重试）；一条连接的读结束时标记它，下一个包重新拨号。每条连接一个读任务，把回包放进容量 64 的队列（满了丢，同 socket 的行为），`recv_from` 从队列取。连接随载体结束（载体在它的最后一条流回收后释放）。为此 `VmessOutbound` 把 `stack` / `cmd_key` / `security` 挪进共享的 `Dialer`；`header::request_plain` 多一个 `command` 参数 |
| P6 | 假服务端 | 服务端一侧的 UDP 中继 `testing::udp`（`Wire::Trojan` / `Wire::Uot`）独立于生产代码的封装实现，免得用被测代码验证它自己；每条流一个回环 UDP socket（全锥），名字只发往 `connect_to`。`FakeTrojan` 遇到命令 3、`FakeAnyTls` 遇到发往魔术名字的流时交给它；`FakeVmess` 遇到命令 2 时每个分块一个数据报，目标是 IP 字面量时直接发往它（`connect_to` 只用于名字——引擎用例的辅助函数总是填 `connect_to`）。各自记录 `udp_outside()`（服务端那一端的地址，供全锥用例的"陌生人"发包）；`rurge_proto::testing::udp_echo_server` 供三个模块与互操作用例共用 |
| P7 | 引擎 | 不改：三种出站的 `udp()` 返回 `Native` 后，M5a 的流水线自然承载它们。经引擎的端到端用例放进新文件 `tests/udp_tls_family.rs`，随各协议的任务逐步加入（设计第 16 节草图的"4. 回环假服务端与经引擎的用例"拆进前三个任务，好让每个任务都有真实的失败步骤） |
| P8 | Shadow TLS 与 WebSocket | UDP 与 TCP 走同一个 `Stack`（connect → shadow-tls → tls → ws），不另做；用例覆盖 trojan 与 vmess 的 WebSocket（vmess 连 TLS）。Shadow TLS 上的 UDP 不单独测 |
| P9 | 任务的切分 | 设计第 16 节草图的 5 个任务合成 4 个：1 trojan（连同共用载体）、2 anytls、3 vmess，各自带假服务端与经引擎的用例；4 互操作与文档 |

## 承接事项

之前计划「延后事项」表里标给 M5b 的条目，及仍然有效的既有现象。

| # | 来源 | 事项 | 处理 | 任务 |
| - | ---- | ---- | ---- | ---- |
| C1 | M5 设计第 16 节 | trojan / anytls / vmess 的 UDP 与回环假服务端、经引擎的用例、互操作 | 本计划 | 1–4 |
| C2 | M5a 延后事项 | `ChainConnector::connect_udp`、WireGuard 的 UDP、`smart` 计入 UDP、UDP 测速、`dns-follow-interface` | 不在本计划：M5c | — |
| C3 | M3b #7（P21） | 测试二进制偶发崩溃 | 照旧：门禁遇到就重跑 | — |

## File Structure

新建：

| 文件 | 职责 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-proto/src/stream_udp.rs` | 一条字节流上按包收发的载体 `StreamUdp` 与两种封装 `Framing::{Trojan, Uot}`（与用例） | 1、2 |
| `crates/rurge-proto/src/testing/udp.rs` | 假服务端一侧的流式 UDP 中继 `relay`（`Wire::{Trojan, Uot}`、`UdpSeen`） | 1、2 |
| `crates/rurge-proto/src/vmess/udp.rs` | vmess 的 UDP 载体 `VmessUdp`：每个目标一条命令 2 的连接 | 3 |
| `crates/rurge-engine/tests/udp_tls_family.rs` | 经引擎的端到端用例：SOCKS5 UDP 进、三种出站出 | 1、2、3 |

修改：

| 文件 | 改动 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-proto/src/lib.rs` | `mod stream_udp` | 1 |
| `crates/rurge-proto/src/trojan.rs` | `udp()` / `open_udp()`（UDP ASSOCIATE），与用例 | 1 |
| `crates/rurge-proto/src/anytls/mod.rs` | `udp()` / `open_udp()`（UDP over TCP v2），与用例 | 2 |
| `crates/rurge-proto/src/vmess/{mod.rs, header.rs}` | 共享的 `Dialer`、`udp()` / `open_udp()`、`request_plain` 的 `command` 参数，与用例 | 3 |
| `crates/rurge-proto/src/testing/{mod.rs, trojan.rs, anytls.rs, vmess.rs}` | `udp_echo_server`（1）；三个假服务端的 UDP（1、2、3） | 1、2、3 |
| `tests/interop/{tests/common/mod.rs, tests/sing_box_tls_family.rs, tests/xray.rs, README.md}` | 对 sing-box（三种）与 xray（vmess）的 UDP 用例 | 4 |
| 文档（兼容性清单、两份 README、`CLAUDE.md`、手工验收） | 见 Task 4 | 4 |

## 任务一览

| 任务 | 交付物 | 依赖 |
| ---- | ------ | ---- |
| 1 | `stream_udp`（共用载体）、`trojan` 的 UDP ASSOCIATE、`FakeTrojan` 的 UDP、经引擎的 trojan 用例 | — |
| 2 | `anytls` 的 UDP over TCP v2、`FakeAnyTls` 的 UDP、经引擎的 anytls 用例 | 1 |
| 3 | `vmess` 的命令 2（每个目标一条连接）、`FakeVmess` 的 UDP、经引擎的 vmess 用例 | 2（`tests/udp_tls_family.rs`） |
| 4 | 对 sing-box / xray 的 UDP 互操作与文档 | 1–3 |

---

### Task 1: `stream_udp` 与 `trojan` 的 UDP ASSOCIATE

一条字节流上按包收发的载体（P4），与它的第一种封装：`trojan` 的 UDP ASSOCIATE（P1）。`TrojanOutbound::open_udp` 经传输阶梯开一条连接，请求头的固定部分（口令哈希、CRLF、命令 3）交给 `StreamUdp`，由第一个包补上它的目标一起写出。`FakeTrojan` 遇到命令 3 时把这条连接交给新的服务端中继 `testing::udp::relay`（P6）。经引擎的端到端用例从本任务开始放在 `tests/udp_tls_family.rs`（P7）。

**Files:**
- Create: `crates/rurge-proto/src/stream_udp.rs`（`StreamUdp`、`Framing::Trojan`，与用例）、`crates/rurge-proto/src/testing/udp.rs`、`crates/rurge-engine/tests/udp_tls_family.rs`
- Modify: `crates/rurge-proto/src/lib.rs`、`src/trojan.rs`（`udp()` / `open_udp()`，与用例）、`src/testing/mod.rs`（`udp_echo_server`）、`src/testing/trojan.rs`（命令 3）

**Interfaces:**
- Consumes: M5a 的 `rurge_net::connector::{PacketSocket, BoxedPacketSocket}`、`Outbound::udp` / `open_udp` 与 `UdpSupport`；既有的 `crate::addr::{socks_addr, parse_socks_addr, AddrError}`、`transport::Stack::open`、`transport::prefixed::boxed`。
- Produces:
  - `pub(crate) enum stream_udp::Framing { Trojan }`（Task 2 加 `Uot`）；`pub(crate) struct StreamUdp`，`StreamUdp::new(stream: BoxedStream, framing: Framing, head: Vec<u8>) -> StreamUdp`（`head` 是请求头里目标之前的部分），实现 `PacketSocket`
  - `TrojanOutbound`：`udp()` 为 `Native`，`open_udp` 开一条连接（整个阶梯受 `opts.timeout` 约束，超时是 `OutboundError::Timeout`）
  - 测试设施：`pub async fn rurge_proto::testing::udp_echo_server() -> SocketAddr`；`pub(crate) mod testing::udp`（`Wire::Trojan`、`UdpSeen`、`relay(stream, wire, connect_to, seen)`）；`FakeTrojan::datagrams() -> Vec<String>`（每个包的目标，`host:port`）、`FakeTrojan::udp_outside() -> Vec<SocketAddr>`
  - 引擎用例的辅助函数（`tests/udp_tls_family.rs` 内）：`udp_records`、`through`、`two_echoes_through`、`a_stranger_writes`

- [ ] **Step 1: 先写用例（连同假服务端的 UDP）**

假服务端与共用的测试设施先放进去：

新建 `crates/rurge-proto/src/testing/udp.rs`：

```rust
//! The server side of UDP over one stream, for the fakes (trojan's UDP
//! ASSOCIATE): every datagram the client frames leaves by one loopback UDP
//! socket per connection, and whatever reaches that socket goes back framed
//! with its source — full cone, as the reference servers do. Names are never
//! resolved: a datagram for one goes to `connect_to`, or nowhere.

use rurge_net::connector::BoxedStream;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;

/// How the client frames a datagram.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Wire {
    /// `ATYP ADDR PORT LENGTH CRLF PAYLOAD`
    Trojan,
}

/// What the fake saw of its UDP relays.
#[derive(Default)]
pub(crate) struct UdpSeen {
    /// Every datagram's target, `host:port`, the name as it was on the wire.
    pub(crate) targets: Mutex<Vec<String>>,
    /// Each relay's own socket, in the order they opened.
    pub(crate) outside: Mutex<Vec<SocketAddr>>,
}

/// One datagram: `(host, port, payload)`; `None` at the stream's end.
async fn read_datagram<R: AsyncRead + Unpin>(
    reader: &mut R,
    wire: Wire,
) -> io::Result<Option<(String, u16, Vec<u8>)>> {
    let mut atyp = [0u8; 1];
    if reader.read(&mut atyp).await? == 0 {
        return Ok(None);
    }
    let host = match (wire, atyp[0]) {
        (Wire::Trojan, 1) => {
            let mut b = [0u8; 4];
            reader.read_exact(&mut b).await?;
            IpAddr::V4(Ipv4Addr::from(b)).to_string()
        }
        (Wire::Trojan, 4) => {
            let mut b = [0u8; 16];
            reader.read_exact(&mut b).await?;
            IpAddr::V6(Ipv6Addr::from(b)).to_string()
        }
        (Wire::Trojan, 3) => {
            let len = usize::from(reader.read_u8().await?);
            let mut name = vec![0u8; len];
            reader.read_exact(&mut name).await?;
            String::from_utf8_lossy(&name).into_owned()
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "an unknown address type",
            ));
        }
    };
    let port = reader.read_u16().await?;
    let len = usize::from(reader.read_u16().await?);
    match wire {
        Wire::Trojan => {
            let mut crlf = [0u8; 2];
            reader.read_exact(&mut crlf).await?;
        }
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    Ok(Some((host, port, payload)))
}

/// `payload` from `from`, framed for the client.
fn frame(wire: Wire, from: SocketAddr, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 24);
    match (wire, from.ip()) {
        (Wire::Trojan, IpAddr::V4(v4)) => {
            out.push(1);
            out.extend_from_slice(&v4.octets());
        }
        (Wire::Trojan, IpAddr::V6(v6)) => {
            out.push(4);
            out.extend_from_slice(&v6.octets());
        }
    }
    out.extend_from_slice(&from.port().to_be_bytes());
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    match wire {
        Wire::Trojan => out.extend_from_slice(b"\r\n"),
    }
    out.extend_from_slice(payload);
    out
}

/// Relays the client's datagrams on `stream` until either side ends.
pub(crate) async fn relay(
    stream: BoxedStream,
    wire: Wire,
    connect_to: Option<SocketAddr>,
    seen: Arc<UdpSeen>,
) -> io::Result<()> {
    let socket = UdpSocket::bind("127.0.0.1:0").await?;
    seen.outside
        .lock()
        .expect("outside")
        .push(socket.local_addr()?);
    let (mut reader, mut writer) = tokio::io::split(stream);
    let up = async {
        while let Some((host, port, payload)) = read_datagram(&mut reader, wire).await? {
            seen.targets
                .lock()
                .expect("targets")
                .push(format!("{host}:{port}"));
            let to = match (host.parse::<IpAddr>(), connect_to) {
                (Ok(ip), _) => SocketAddr::new(ip, port),
                (Err(_), Some(addr)) => addr,
                // never resolves: a name without `connect_to` is a dead end
                (Err(_), None) => continue,
            };
            socket.send_to(&payload, to).await?;
        }
        Ok::<(), io::Error>(())
    };
    let down = async {
        let mut buf = vec![0u8; 65536];
        loop {
            let (n, from) = match socket.recv_from(&mut buf).await {
                Ok(got) => got,
                // an ICMP "unreachable" for an earlier datagram (Windows)
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                Err(e) => return Err(e),
            };
            writer.write_all(&frame(wire, from, &buf[..n])).await?;
            writer.flush().await?;
        }
    };
    tokio::select! {
        done = up => done,
        done = down => done,
    }
}
```

`crates/rurge-proto/src/testing/mod.rs`——把

```rust
mod trojan;
```

换成

```rust
mod trojan;
mod udp;
```

`crates/rurge-proto/src/testing/mod.rs`——把

```rust
use tokio::net::TcpListener;
```

换成

```rust
use tokio::net::{TcpListener, UdpSocket};
```

`crates/rurge-proto/src/testing/mod.rs`——把

```rust
            });
```

换成

```rust
            });
        }
    });
    addr
}

/// Answers every datagram with itself, to wherever it came from.
pub async fn udp_echo_server() -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind loopback");
    let addr = socket.local_addr().expect("local addr");
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        loop {
            match socket.recv_from(&mut buf).await {
                Ok((n, from)) => {
                    let _ = socket.send_to(&buf[..n], from).await;
                }
                // an ICMP "unreachable" for an earlier answer (Windows)
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                Err(_) => return,
            }
```

`crates/rurge-proto/src/testing/trojan.rs`——把

```rust
//! protocol, the request head, then a relay. It never resolves a name.

use super::ws::{RecordedWs, accept_bytes};
```

换成

```rust
//! protocol, the request head, then a relay (UDP ASSOCIATE: `udp::relay`).
//! It never resolves a name.

use super::udp::{self, UdpSeen, Wire};
use super::ws::{RecordedWs, accept_bytes};
```

`crates/rurge-proto/src/testing/trojan.rs`——把

```rust
    rejected: Arc<AtomicUsize>,
    _task: AbortOnDrop,
```

换成

```rust
    rejected: Arc<AtomicUsize>,
    udp: Arc<UdpSeen>,
    _task: AbortOnDrop,
```

`crates/rurge-proto/src/testing/trojan.rs`——把

```rust
    rejected: Arc<AtomicUsize>,
}
```

换成

```rust
    rejected: Arc<AtomicUsize>,
    udp: Arc<UdpSeen>,
}
```

`crates/rurge-proto/src/testing/trojan.rs`——把

```rust
        .push(request.clone());
```

换成

```rust
        .push(request.clone());
    if request.command == 3 {
        let stream = crate::transport::prefixed::boxed(request.early, stream);
        return udp::relay(
            stream,
            Wire::Trojan,
            shared.script.connect_to,
            shared.udp.clone(),
        )
        .await;
    }
```

`crates/rurge-proto/src/testing/trojan.rs`——把

```rust
        let rejected = Arc::new(AtomicUsize::new(0));
```

换成

```rust
        let rejected = Arc::new(AtomicUsize::new(0));
        let udp = Arc::new(UdpSeen::default());
```

`crates/rurge-proto/src/testing/trojan.rs`——把

```rust
            rejected: rejected.clone(),
```

换成

```rust
            rejected: rejected.clone(),
            udp: udp.clone(),
```

`crates/rurge-proto/src/testing/trojan.rs`——把

```rust
            _task: AbortOnDrop(task),
        }
```

换成

```rust
            udp,
            _task: AbortOnDrop(task),
        }
    }

    /// Every UDP datagram's target, `host:port`, in arrival order.
    pub fn datagrams(&self) -> Vec<String> {
        self.udp.targets.lock().expect("targets").clone()
    }

    /// Where each UDP ASSOCIATE sends from: a datagram to one of these goes
    /// back to that association's client.
    pub fn udp_outside(&self) -> Vec<SocketAddr> {
        self.udp.outside.lock().expect("outside").clone()
```

出站的用例：

`crates/rurge-proto/src/trojan.rs`——把

```rust
    use crate::testing::{FakeTrojan, SeenHandshake, TlsFixture, TrojanScript, echo_server};
```

换成

```rust
    use crate::testing::{
        FakeTrojan, SeenHandshake, TlsFixture, TrojanScript, echo_server, udp_echo_server,
    };
```

`crates/rurge-proto/src/trojan.rs`——把

```rust
    use rurge_net::connector::{DirectConnector, SystemResolve};
```

换成

```rust
    use rurge_net::connector::{DirectConnector, PacketSocket, SystemResolve};
```

`crates/rurge-proto/src/trojan.rs`——把

```rust
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
```

换成

```rust
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    async fn udp_answer(carrier: &dyn PacketSocket) -> (Vec<u8>, Target) {
        let mut buf = [0u8; 1500];
        let (n, from) = tokio::time::timeout(Duration::from_secs(5), carrier.recv_from(&mut buf))
            .await
            .expect("an answer within the bound")
            .unwrap();
        (buf[..n].to_vec(), from)
    }

    async fn udp_roundtrip(carrier: &dyn PacketSocket, to: SocketAddr, payload: &[u8]) {
        carrier.send_to(payload, &target(to)).await.unwrap();
        assert_eq!(udp_answer(carrier).await, (payload.to_vec(), target(to)));
    }

    /// UDP ASSOCIATE: one connection whose head goes out with the first
    /// datagram and names its target; every datagram carries its own.
    #[tokio::test]
    async fn udp_goes_through_one_connection() {
        let (one, two) = (udp_echo_server().await, udp_echo_server().await);
        let (fixture, fake) = fake("pw", false, None).await;
        let out = outbound(
            &format!("trojan, 127.0.0.1, {}, password=pw", fake.addr().port()),
            &fixture,
        );
        assert_eq!(out.udp(), UdpSupport::Native);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), one, b"to one").await;
        udp_roundtrip(carrier.as_ref(), two, b"to two").await;
        let seen = fake.requests();
        assert_eq!(seen.len(), 1, "one connection");
        assert_eq!(
            (seen[0].command, seen[0].host.as_str(), seen[0].port),
            (3, "127.0.0.1", one.port())
        );
        assert_eq!(fake.datagrams(), [one.to_string(), two.to_string()]);
    }

    /// Full cone: whoever reaches the server's end of the association is
    /// heard, under its own address.
    #[tokio::test]
    async fn anyone_may_answer_through_trojan() {
        let echo = udp_echo_server().await;
        let (fixture, fake) = fake("pw", false, None).await;
        let out = outbound(
            &format!("trojan, 127.0.0.1, {}, password=pw", fake.addr().port()),
            &fixture,
        );
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), echo, b"hello").await;
        let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        stranger
            .send_to(b"unasked", fake.udp_outside()[0])
            .await
            .unwrap();
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"unasked".to_vec(), target(stranger.local_addr().unwrap()))
        );
    }

    /// Over a WebSocket too; a name goes to the server as its A-labels.
    #[tokio::test]
    async fn udp_over_a_websocket_with_a_name() {
        let echo = udp_echo_server().await;
        let (fixture, fake) = fake("pw", true, Some(echo)).await;
        let out = outbound(
            &format!(
                "trojan, 127.0.0.1, {}, password=pw, ws=true, ws-path=/u",
                fake.addr().port()
            ),
            &fixture,
        );
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        let name = Target::new(HostName::Domain("bücher.example".into()), 53);
        carrier.send_to(b"q", &name).await.unwrap();
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"q".to_vec(), target(echo))
        );
        assert_eq!(fake.requests()[0].host, "xn--bcher-kva.example");
        assert_eq!(fake.datagrams(), ["xn--bcher-kva.example:53"]);
        assert_eq!(fake.ws_seen()[0].path, "/u");
    }
}
```

经引擎的用例：

新建 `crates/rurge-engine/tests/udp_tls_family.rs`：

```rust
//! UDP through the TLS family (phase 2 M5b): a SOCKS5 UDP association into
//! rurge, out through `trojan` (UDP ASSOCIATE) to a loopback fake.

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

async fn through(policy: &str) -> Harness {
    harness(Profile {
        proxies: policy,
        rules: "IP-CIDR,127.0.0.1/32,P,no-resolve",
        ..Profile::default()
    })
    .await
}

/// Two echoes, asked three times through `policy` (a `P = …` line); the
/// flows leave through `P` and end with the association.
async fn two_echoes_through(policy: &str) {
    let h = through(policy).await;
    let ((one, _), (two, _)) = (udp_echo().await, udp_echo().await);
    let association = udp_associate(h.socks()).await;
    for (echo, payload) in [(one, &b"one"[..]), (two, b"two"), (one, b"again")] {
        association.send("127.0.0.1", echo.port(), payload).await;
        assert_eq!(association.recv().await, (echo, payload.to_vec()));
    }
    drop(association);
    wait_until("both flows to finish", || udp_records(&h).len() == 2).await;
    for r in udp_records(&h) {
        assert_eq!(r.policy, ["P"], "{r:?}");
        assert_eq!(r.status, RecordStatus::Completed, "{r:?}");
    }
}

/// After one exchange through `policy`, a stranger writes to the server's
/// end of the association (`server_end`, known once the association is
/// open); returns where the client says the stranger's datagram came from,
/// the stranger's own address and the echo's.
async fn a_stranger_writes(
    policy: &str,
    server_end: impl Fn() -> std::net::SocketAddr,
) -> (
    std::net::SocketAddr,
    std::net::SocketAddr,
    std::net::SocketAddr,
) {
    let h = through(policy).await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"hello").await;
    assert_eq!(association.recv().await, (echo, b"hello".to_vec()));
    let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    stranger.send_to(b"unasked", server_end()).await.unwrap();
    let (from, payload) = association.recv().await;
    assert_eq!(payload, b"unasked");
    (from, stranger.local_addr().unwrap(), echo)
}

#[tokio::test]
async fn udp_goes_through_trojan() {
    let origin = TestServer::spawn().await;
    let (upstream, params) = trojan_upstream(false, origin_addr(&origin)).await;
    two_echoes_through(&format!(
        "P = trojan, 127.0.0.1, {}, {params}",
        upstream.addr().port()
    ))
    .await;
    let seen = upstream.requests();
    assert_eq!(
        (seen.len(), seen[0].command),
        (1, 3),
        "one UDP ASSOCIATE carries both flows"
    );
    assert_eq!(upstream.datagrams().len(), 3);
}

/// Full cone: whoever reaches the server's end of the association reaches
/// the client, under its own address.
#[tokio::test]
async fn anyone_may_answer_through_trojan() {
    let origin = TestServer::spawn().await;
    let (upstream, params) = trojan_upstream(false, origin_addr(&origin)).await;
    let (from, stranger, _) = a_stranger_writes(
        &format!(
            "P = trojan, 127.0.0.1, {}, {params}",
            upstream.addr().port()
        ),
        || upstream.udp_outside()[0],
    )
    .await;
    assert_eq!(from, stranger);
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto --lib trojan`
Expected: FAIL——用例用到的 `UdpSupport` 由 Step 3 引入 `trojan.rs`，编译不过：

```text
error[E0433]: failed to resolve: use of undeclared type `UdpSupport`
   --> crates\rurge-proto\src\trojan.rs:487:31
For more information about this error, try `rustc --explain E0433`.
error: could not compile `rurge-proto` (lib test) due to 1 previous error
exit 101
```

Run: `cargo test -p rurge-engine --test udp_tls_family`
Expected: FAIL——引擎按 `udp-policy-not-supported-behaviour` 的默认拒绝这些流，客户端等不到回包（`common/mod.rs:277` 是 `UdpAssociation::recv` 的 `expect("a datagram comes back")`）：

```text
test anyone_may_answer_through_trojan ... FAILED
test udp_goes_through_trojan ... FAILED
thread 'anyone_may_answer_through_trojan' panicked at crates\rurge-engine\tests\common\mod.rs:277:18:
thread 'udp_goes_through_trojan' panicked at crates\rurge-engine\tests\common\mod.rs:277:18:
test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.04s
error: test failed, to rerun pass `-p rurge-engine --test udp_tls_family`
exit 101
```

- [ ] **Step 3: 实现**

新模块（自带用例）：

新建 `crates/rurge-proto/src/stream_udp.rs`：

```rust
//! UDP over one byte stream (M5 design §7): `trojan`'s UDP ASSOCIATE
//! carries every datagram, each way, as its address, its length and the
//! payload. The request head goes out with the first datagram and names that
//! datagram's target, as the reference clients do; the server answers
//! nothing until then.

use crate::addr::{AddrError, parse_socks_addr, socks_addr};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, PacketSocket, Target};
use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::Mutex;

/// How a datagram is framed on the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Framing {
    /// `ATYP ADDR PORT LENGTH CRLF PAYLOAD` (trojan-gfw's protocol document).
    Trojan,
}

impl Framing {
    /// The protocol's name in error texts.
    fn label(self) -> &'static str {
        match self {
            Framing::Trojan => "trojan",
        }
    }

    fn unsendable(self, e: AddrError) -> io::Error {
        let what = match e {
            AddrError::Unsendable => "the host name cannot be sent to the server",
            AddrError::TooLong => "the host name is longer than 255 bytes",
        };
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{}: {what}", self.label()),
        )
    }

    /// What follows the head's fixed part: the first datagram's target.
    fn head_tail(self, first: &Target, out: &mut Vec<u8>) -> io::Result<()> {
        out.extend(socks_addr(first).map_err(|e| self.unsendable(e))?);
        match self {
            Framing::Trojan => out.extend_from_slice(b"\r\n"),
        }
        Ok(())
    }

    /// Appends `payload` for `to`, framed.
    fn encode(self, to: &Target, payload: &[u8], out: &mut Vec<u8>) -> io::Result<()> {
        let len = u16::try_from(payload.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{}: a datagram longer than 65535 bytes", self.label()),
            )
        })?;
        out.extend(socks_addr(to).map_err(|e| self.unsendable(e))?);
        out.extend_from_slice(&len.to_be_bytes());
        match self {
            Framing::Trojan => out.extend_from_slice(b"\r\n"),
        }
        out.extend_from_slice(payload);
        Ok(())
    }

    /// Bytes between the length and the payload.
    fn gap(self) -> usize {
        match self {
            Framing::Trojan => 2,
        }
    }
}

struct Writer {
    half: WriteHalf<BoxedStream>,
    /// The head's fixed part, until the first datagram takes it along.
    head: Option<Vec<u8>>,
}

/// A carrier on one stream to the server: every target through it (full
/// cone, as far as the server goes).
pub(crate) struct StreamUdp {
    framing: Framing,
    writer: Mutex<Writer>,
    reader: Mutex<ReadHalf<BoxedStream>>,
}

impl StreamUdp {
    /// `head` is the request head up to the target, which the first
    /// datagram supplies.
    pub(crate) fn new(stream: BoxedStream, framing: Framing, head: Vec<u8>) -> StreamUdp {
        let (reader, half) = tokio::io::split(stream);
        StreamUdp {
            framing,
            writer: Mutex::new(Writer {
                half,
                head: Some(head),
            }),
            reader: Mutex::new(reader),
        }
    }

    fn closed(&self) -> io::Error {
        io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!(
                "{}: the server closed the UDP connection",
                self.framing.label()
            ),
        )
    }
}

/// Fills `buf` from `reader`; the stream's end, even between datagrams, is
/// the carrier's end.
async fn fill(
    reader: &mut ReadHalf<BoxedStream>,
    buf: &mut [u8],
    closed: impl Fn() -> io::Error,
) -> io::Result<()> {
    match reader.read_exact(buf).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Err(closed()),
        Err(e) => Err(e),
    }
}

impl PacketSocket for StreamUdp {
    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            let mut writer = self.writer.lock().await;
            let mut out = Vec::with_capacity(buf.len() + 300);
            if let Some(head) = &writer.head {
                out.extend_from_slice(head);
                self.framing.head_tail(to, &mut out)?;
            }
            self.framing.encode(to, buf, &mut out)?;
            writer.head = None;
            writer.half.write_all(&out).await?;
            writer.half.flush().await
        })
    }

    /// A datagram longer than `buf` is skipped: give it 64 KiB.
    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            let mut reader = self.reader.lock().await;
            let closed = || self.closed();
            loop {
                // ATYP, then as much of the address as it says
                let mut addr = vec![0u8; 2];
                fill(&mut reader, &mut addr, closed).await?;
                let rest = match addr[0] {
                    1 => 4 - 1,
                    4 => 16 - 1,
                    3 => usize::from(addr[1]),
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "{}: a datagram of an unknown address type",
                                self.framing.label()
                            ),
                        ));
                    }
                };
                // the rest of the address, the port, the length and the gap
                let mut tail = vec![0u8; rest + 2 + 2 + self.framing.gap()];
                fill(&mut reader, &mut tail, closed).await?;
                addr.extend_from_slice(&tail[..rest + 2]);
                let len_at = rest + 2;
                let len = usize::from(u16::from_be_bytes([tail[len_at], tail[len_at + 1]]));
                let from = parse_socks_addr(&addr).map(|(from, _)| from);
                if len > buf.len() {
                    // read past what does not fit
                    let mut skip = vec![0u8; len];
                    fill(&mut reader, &mut skip, closed).await?;
                    continue;
                }
                fill(&mut reader, &mut buf[..len], closed).await?;
                // a source that is no host name cannot be answered: dropped
                if let Some(from) = from {
                    return Ok((len, from));
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;
    use std::time::Duration;
    use tokio::io::DuplexStream;

    fn carrier() -> (StreamUdp, DuplexStream) {
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        (
            StreamUdp::new(Box::new(ours), Framing::Trojan, b"HEAD".to_vec()),
            theirs,
        )
    }

    async fn written(server: &mut DuplexStream, n: usize) -> Vec<u8> {
        let mut buf = vec![0u8; n];
        tokio::time::timeout(Duration::from_secs(5), server.read_exact(&mut buf))
            .await
            .expect("written within the bound")
            .unwrap();
        buf
    }

    #[tokio::test]
    async fn the_head_rides_with_the_first_datagram_and_names_its_target() {
        let (udp, mut server) = carrier();
        let one = Target::new(HostName::parse("1.2.3.4"), 53);
        udp.send_to(b"q", &one).await.unwrap();
        let expected: &[u8] =
            b"HEAD\x01\x01\x02\x03\x04\x00\x35\r\n\x01\x01\x02\x03\x04\x00\x35\x00\x01\r\nq";
        assert_eq!(written(&mut server, expected.len()).await, expected);
        // the head goes out once
        let name = Target::new(HostName::Domain("bücher.example".into()), 443);
        udp.send_to(b"xy", &name).await.unwrap();
        let mut expected = vec![3, 21];
        expected.extend_from_slice(b"xn--bcher-kva.example");
        expected.extend_from_slice(&[1, 187, 0, 2, b'\r', b'\n', b'x', b'y']);
        assert_eq!(written(&mut server, expected.len()).await, expected);
    }

    #[tokio::test]
    async fn datagrams_come_back_with_their_source() {
        let (udp, mut server) = carrier();
        // an IPv6 source, one that does not fit, then a name
        let mut wire = vec![4];
        wire.extend_from_slice(&[0; 15]);
        wire.push(1);
        wire.extend_from_slice(&[0, 7, 0, 3, b'\r', b'\n', b'a', b'b', b'c']);
        wire.extend_from_slice(&[1, 9, 9, 9, 9, 0, 9, 0, 100, b'\r', b'\n']);
        wire.extend_from_slice(&[0u8; 100]);
        wire.extend_from_slice(&[3, 6]);
        wire.extend_from_slice(b"s.test");
        wire.extend_from_slice(&[0, 80, 0, 1, b'\r', b'\n', b'z']);
        server.write_all(&wire).await.unwrap();
        let mut buf = [0u8; 64];
        let (n, from) = udp.recv_from(&mut buf).await.unwrap();
        assert_eq!(
            (&buf[..n], from),
            (&b"abc"[..], Target::new(HostName::parse("::1"), 7))
        );
        let (n, from) = udp.recv_from(&mut buf).await.unwrap();
        assert_eq!(
            (&buf[..n], from),
            (&b"z"[..], Target::new(HostName::parse("s.test"), 80)),
            "the 100-byte datagram did not fit and was skipped"
        );
    }

    #[tokio::test]
    async fn the_servers_end_is_the_carriers_end() {
        let (udp, server) = carrier();
        drop(server);
        let mut buf = [0u8; 64];
        let err = udp.recv_from(&mut buf).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "trojan: the server closed the UDP connection"
        );
    }

    #[tokio::test]
    async fn an_unsendable_name_is_refused_and_the_head_waits() {
        let (udp, mut server) = carrier();
        let err = udp
            .send_to(b"x", &Target::new(HostName::Domain("a@b.test".into()), 53))
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "trojan: the host name cannot be sent to the server"
        );
        udp.send_to(b"x", &Target::new(HostName::parse("1.2.3.4"), 53))
            .await
            .unwrap();
        assert_eq!(&written(&mut server, 4).await, b"HEAD");
    }
}
```

`crates/rurge-proto/src/lib.rs`——把

```rust
pub mod socks5;
```

换成

```rust
pub mod socks5;
mod stream_udp;
```

`crates/rurge-proto/src/trojan.rs`——把

```rust
//! shows once the relay starts, as whatever the server's fallback site says.
```

换成

```rust
//! shows once the relay starts, as whatever the server's fallback site says.
//! UDP goes through one connection as UDP ASSOCIATE (`stream_udp`).
```

`crates/rurge-proto/src/trojan.rs`——把

```rust
use crate::build::{shadow_tls_client, tls_client};
```

换成

```rust
use crate::build::{shadow_tls_client, tls_client};
use crate::stream_udp::{Framing, StreamUdp};
```

`crates/rurge-proto/src/trojan.rs`——把

```rust
use crate::{BuildError, Outbound, OutboundError};
```

换成

```rust
use crate::{BuildError, Outbound, OutboundError, UdpSupport};
```

`crates/rurge-proto/src/trojan.rs`——把

```rust
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
```

换成

```rust
use rurge_net::connector::{BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, Target};
```

`crates/rurge-proto/src/trojan.rs`——把

```rust
const CONNECT: u8 = 1;
```

换成

```rust
const CONNECT: u8 = 1;
const UDP_ASSOCIATE: u8 = 3;
```

`crates/rurge-proto/src/trojan.rs`——把

```rust
        })
    }
}
```

换成

```rust
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
            let stream = match tokio::time::timeout(opts.timeout, self.stack.open(opts)).await {
                Ok(result) => result?,
                Err(_) => return Err(OutboundError::Timeout),
            };
            // the target comes with the first datagram
            let mut head = Vec::with_capacity(56 + 2 + 1);
            head.extend_from_slice(&self.hash);
            head.extend_from_slice(b"\r\n");
            head.push(UDP_ASSOCIATE);
            Ok(Box::new(StreamUdp::new(stream, Framing::Trojan, head)) as BoxedPacketSocket)
        })
    }
}
```

要点：
- 请求头只随第一个**成功编码**的包发出：目标名发不出去（`trojan: the host name cannot be sent to the server`）时什么也不写，下一个包照常带上请求头。
- 发送整个包（请求头也在内）一次 `write_all` 后 `flush`：一个 TLS 记录，服务端从一次读里拿到请求头与第一个包。
- 接收只有引擎的一个收包任务在调用；锁只是为了让 `&self` 能拿到可变的读半边。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto --lib -- trojan stream_udp` → 通过（新增 `udp_goes_through_one_connection`、`anyone_may_answer_through_trojan`、`udp_over_a_websocket_with_a_name`，`stream_udp` 模块 4 条）。
Run: `cargo test -p rurge-engine --test udp_tls_family` → 通过（`udp_goes_through_trojan`、`anyone_may_answer_through_trojan`）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-proto crates/rurge-engine/tests/udp_tls_family.rs
git commit -m "feat(proto): trojan 的 UDP ASSOCIATE——一条字节流上按包收发的载体 stream_udp；FakeTrojan 的 UDP"
```


### Task 2: `anytls` 的 UDP over TCP v2

`stream_udp` 的第二种封装 `Framing::Uot`（P2）：请求头是 `isConnect = 0` 加第一个包的目标（SOCKS5 的类型号），此后每个数据报的地址用 sing 的类型号 0 / 1 / 2、没有 CRLF。`AnyTlsOutbound::open_udp` 开一条发往 `sp.v2.udp-over-tcp.arpa:0` 的流（与 TCP 的流一样经会话与会话池）。服务端中继学会这种封装；`FakeAnyTls` 把发往这个名字的流经一条内存管道交给中继（P6）。

**Files:**
- Modify: `crates/rurge-proto/src/stream_udp.rs`（`Framing::Uot`，与用例）、`src/anytls/mod.rs`（`UOT_MAGIC`、`udp()` / `open_udp()`，与用例）、`src/testing/udp.rs`（`Wire::Uot`、请求的记录）、`src/testing/anytls.rs`
- Modify: `crates/rurge-engine/tests/udp_tls_family.rs`

**Interfaces:**
- Consumes: Task 1 的 `StreamUdp::new`、`Framing`、`testing::udp::{relay, Wire, UdpSeen}`、`udp_echo_server` 与引擎用例的辅助函数；既有的 `AnyTlsOutbound::open(address, opts)`（私有，本模块内）。
- Produces:
  - `Framing::Uot`
  - `AnyTlsOutbound`：`udp()` 为 `Native`，`open_udp` 开一条 UDP over TCP 的流（受 `opts.timeout` 约束）
  - 测试设施：`Wire::Uot`；`UdpSeen.requests`；`FakeAnyTls::uot_requests() -> Vec<(u8, String)>`（`isConnect` 与目标）、`FakeAnyTls::datagrams()`、`FakeAnyTls::udp_outside()`

- [ ] **Step 1: 先写用例（连同假服务端的 UDP）**

`crates/rurge-proto/src/testing/udp.rs`——把

```rust
//! ASSOCIATE): every datagram the client frames leaves by one loopback UDP
//! socket per connection, and whatever reaches that socket goes back framed
//! with its source — full cone, as the reference servers do. Names are never
//! resolved: a datagram for one goes to `connect_to`, or nowhere.
```

换成

```rust
//! ASSOCIATE, AnyTLS's UDP over TCP): every datagram the client frames leaves
//! by one loopback UDP socket per stream, and whatever reaches that socket
//! goes back framed with its source — full cone, as the reference servers do.
//! Names are never resolved: a datagram for one goes to `connect_to`, or
//! nowhere.
```

`crates/rurge-proto/src/testing/udp.rs`——把

```rust
    Trojan,
```

换成

```rust
    Trojan,
    /// `isConnect ATYP ADDR PORT` once (SOCKS5's types), then every datagram
    /// as `TYPE ADDR PORT LENGTH PAYLOAD` with types 0 / 1 / 2.
    Uot,
}

impl Wire {
    /// The type numbers of IPv4, IPv6 and a name in a datagram.
    fn types(self) -> (u8, u8, u8) {
        match self {
            Wire::Trojan => (1, 4, 3),
            Wire::Uot => (0, 1, 2),
        }
    }
```

`crates/rurge-proto/src/testing/udp.rs`——把

```rust
pub(crate) struct UdpSeen {
```

换成

```rust
pub(crate) struct UdpSeen {
    /// Every UDP-over-TCP request: `(isConnect, target)`.
    pub(crate) requests: Mutex<Vec<(u8, String)>>,
```

`crates/rurge-proto/src/testing/udp.rs`——把

```rust
    pub(crate) outside: Mutex<Vec<SocketAddr>>,
```

换成

```rust
    pub(crate) outside: Mutex<Vec<SocketAddr>>,
}

/// An address of type `atyp` (numbered as `types` says) and its port, as
/// `host:port`; `None` at the stream's end before the type.
async fn read_addr<R: AsyncRead + Unpin>(
    reader: &mut R,
    (v4, v6, name): (u8, u8, u8),
) -> io::Result<Option<(String, u16)>> {
    let mut atyp = [0u8; 1];
    if reader.read(&mut atyp).await? == 0 {
        return Ok(None);
    }
    let host = if atyp[0] == v4 {
        let mut b = [0u8; 4];
        reader.read_exact(&mut b).await?;
        IpAddr::V4(Ipv4Addr::from(b)).to_string()
    } else if atyp[0] == v6 {
        let mut b = [0u8; 16];
        reader.read_exact(&mut b).await?;
        IpAddr::V6(Ipv6Addr::from(b)).to_string()
    } else if atyp[0] == name {
        let len = usize::from(reader.read_u8().await?);
        let mut bytes = vec![0u8; len];
        reader.read_exact(&mut bytes).await?;
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "an unknown address type",
        ));
    };
    Ok(Some((host, reader.read_u16().await?)))
```

`crates/rurge-proto/src/testing/udp.rs`——把

```rust
    let mut atyp = [0u8; 1];
    if reader.read(&mut atyp).await? == 0 {
        return Ok(None);
    }
    let host = match (wire, atyp[0]) {
        (Wire::Trojan, 1) => {
            let mut b = [0u8; 4];
            reader.read_exact(&mut b).await?;
            IpAddr::V4(Ipv4Addr::from(b)).to_string()
        }
        (Wire::Trojan, 4) => {
            let mut b = [0u8; 16];
            reader.read_exact(&mut b).await?;
            IpAddr::V6(Ipv6Addr::from(b)).to_string()
        }
        (Wire::Trojan, 3) => {
            let len = usize::from(reader.read_u8().await?);
            let mut name = vec![0u8; len];
            reader.read_exact(&mut name).await?;
            String::from_utf8_lossy(&name).into_owned()
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "an unknown address type",
            ));
        }
    };
    let port = reader.read_u16().await?;
    let len = usize::from(reader.read_u16().await?);
    match wire {
        Wire::Trojan => {
            let mut crlf = [0u8; 2];
            reader.read_exact(&mut crlf).await?;
        }
```

换成

```rust
    let Some((host, port)) = read_addr(reader, wire.types()).await? else {
        return Ok(None);
    };
    let len = usize::from(reader.read_u16().await?);
    if let Wire::Trojan = wire {
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf).await?;
```

`crates/rurge-proto/src/testing/udp.rs`——把

```rust
    let mut out = Vec::with_capacity(payload.len() + 24);
    match (wire, from.ip()) {
        (Wire::Trojan, IpAddr::V4(v4)) => {
            out.push(1);
            out.extend_from_slice(&v4.octets());
        }
        (Wire::Trojan, IpAddr::V6(v6)) => {
            out.push(4);
            out.extend_from_slice(&v6.octets());
```

换成

```rust
    let (v4, v6, _) = wire.types();
    let mut out = Vec::with_capacity(payload.len() + 24);
    match from.ip() {
        IpAddr::V4(ip) => {
            out.push(v4);
            out.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            out.push(v6);
            out.extend_from_slice(&ip.octets());
```

`crates/rurge-proto/src/testing/udp.rs`——把

```rust
    match wire {
        Wire::Trojan => out.extend_from_slice(b"\r\n"),
```

换成

```rust
    if let Wire::Trojan = wire {
        out.extend_from_slice(b"\r\n");
```

`crates/rurge-proto/src/testing/udp.rs`——把

```rust
    let (mut reader, mut writer) = tokio::io::split(stream);
```

换成

```rust
    let (mut reader, mut writer) = tokio::io::split(stream);
    if let Wire::Uot = wire {
        let connect = reader.read_u8().await?;
        let Some((host, port)) = read_addr(&mut reader, (1, 4, 3)).await? else {
            return Ok(());
        };
        seen.requests
            .lock()
            .expect("requests")
            .push((connect, format!("{host}:{port}")));
    }
```

`crates/rurge-proto/src/testing/anytls.rs`——把

```rust
//! request). It never resolves a name.

use super::{AbortOnDrop, TlsFixture};
```

换成

```rust
//! request); a stream to the UDP-over-TCP name relays datagrams
//! (`udp::relay`). It never resolves a name.

use super::udp::{self, UdpSeen, Wire};
use super::{AbortOnDrop, TlsFixture};
```

`crates/rurge-proto/src/testing/anytls.rs`——把

```rust
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedWriteHalf;
```

换成

```rust
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
```

`crates/rurge-proto/src/testing/anytls.rs`——把

```rust
    settings: Mutex<Vec<String>>,
```

换成

```rust
    settings: Mutex<Vec<String>>,
    udp: Arc<UdpSeen>,
```

`crates/rurge-proto/src/testing/anytls.rs`——把

```rust
            settings: Mutex::default(),
```

换成

```rust
            settings: Mutex::default(),
            udp: Arc::default(),
```

`crates/rurge-proto/src/testing/anytls.rs`——把

```rust
struct Live {
    to_upstream: OwnedWriteHalf,
    _reader: AbortOnDrop,
}
```

换成

```rust
type Reader = Box<dyn AsyncRead + Send + Unpin>;
type Writer = Box<dyn AsyncWrite + Send + Unpin>;

struct Live {
    to_upstream: Writer,
    _reader: AbortOnDrop,
}

/// The UDP-over-TCP name (sing's `uot.MagicAddress`).
const UOT_MAGIC: &str = "sp.v2.udp-over-tcp.arpa";
```

`crates/rurge-proto/src/testing/anytls.rs`——把

```rust
                    let addr = match (script.connect_to, host.parse::<IpAddr>()) {
                        (Some(addr), _) => Some(addr),
                        (None, Ok(ip)) => Some(SocketAddr::new(ip, port)),
                        // never resolves: a name without `connect_to` is a dead end
                        (None, Err(_)) => None,
                    };
                    let upstream = match addr {
                        Some(addr) => TcpStream::connect(addr).await.ok(),
                        None => None,
```

换成

```rust
                    let upstream: Option<(Reader, Writer)> = if host == UOT_MAGIC {
                        // datagrams through a pipe to the UDP relay
                        let (ours, relay) = tokio::io::duplex(1 << 16);
                        let (connect_to, seen) = (script.connect_to, seen.udp.clone());
                        tokio::spawn(udp::relay(Box::new(relay), Wire::Uot, connect_to, seen));
                        let (from, to) = tokio::io::split(ours);
                        Some((Box::new(from), Box::new(to)))
                    } else {
                        let addr = match (script.connect_to, host.parse::<IpAddr>()) {
                            (Some(addr), _) => Some(addr),
                            (None, Ok(ip)) => Some(SocketAddr::new(ip, port)),
                            // never resolves: a name without `connect_to` is a dead end
                            (None, Err(_)) => None,
                        };
                        match addr {
                            Some(addr) => TcpStream::connect(addr).await.ok().map(|tcp| {
                                let (from, to) = tcp.into_split();
                                (Box::new(from) as Reader, Box::new(to) as Writer)
                            }),
                            None => None,
                        }
```

`crates/rurge-proto/src/testing/anytls.rs`——把

```rust
                    let (mut from_upstream, mut to_upstream) = upstream.into_split();
```

换成

```rust
                    let (mut from_upstream, mut to_upstream) = upstream;
```

`crates/rurge-proto/src/testing/anytls.rs`——把

```rust
        self.seen.waste.load(Ordering::SeqCst)
    }
}
```

换成

```rust
        self.seen.waste.load(Ordering::SeqCst)
    }

    /// Every UDP-over-TCP request: `(isConnect, target)`, the target as
    /// `host:port`.
    pub fn uot_requests(&self) -> Vec<(u8, String)> {
        self.seen.udp.requests.lock().expect("requests").clone()
    }

    /// Every UDP datagram's target, `host:port`, in arrival order.
    pub fn datagrams(&self) -> Vec<String> {
        self.seen.udp.targets.lock().expect("targets").clone()
    }

    /// Where each UDP-over-TCP stream sends from: a datagram to one of these
    /// goes back to that stream's client.
    pub fn udp_outside(&self) -> Vec<SocketAddr> {
        self.seen.udp.outside.lock().expect("outside").clone()
    }
}
```

`crates/rurge-proto/src/stream_udp.rs`——把

```rust
        assert_eq!(&written(&mut server, 4).await, b"HEAD");
    }
}
```

换成

```rust
        assert_eq!(&written(&mut server, 4).await, b"HEAD");
    }

    /// UDP over TCP numbers a datagram's address types its own way (0 / 1 /
    /// 2) and has no CRLF; the head's address keeps SOCKS5's numbers.
    #[tokio::test]
    async fn udp_over_tcp_has_its_own_address_types() {
        let (ours, mut server) = tokio::io::duplex(1 << 16);
        let udp = StreamUdp::new(Box::new(ours), Framing::Uot, vec![0]);
        udp.send_to(b"q", &Target::new(HostName::parse("1.2.3.4"), 53))
            .await
            .unwrap();
        let expected: &[u8] = &[
            0, 1, 1, 2, 3, 4, 0, 53, // not connect mode, the first target
            0, 1, 2, 3, 4, 0, 53, 0, 1, b'q', // the datagram
        ];
        assert_eq!(written(&mut server, expected.len()).await, expected);
        let mut wire = vec![2, 6];
        wire.extend_from_slice(b"s.test");
        wire.extend_from_slice(&[0, 80, 0, 1, b'z', 1]);
        wire.extend_from_slice(&[0; 15]);
        wire.extend_from_slice(&[1, 0, 7, 0, 2, b'a', b'b']);
        server.write_all(&wire).await.unwrap();
        let mut buf = [0u8; 64];
        let (n, from) = udp.recv_from(&mut buf).await.unwrap();
        assert_eq!(
            (&buf[..n], from),
            (&b"z"[..], Target::new(HostName::parse("s.test"), 80))
        );
        let (n, from) = udp.recv_from(&mut buf).await.unwrap();
        assert_eq!(
            (&buf[..n], from),
            (&b"ab"[..], Target::new(HostName::parse("::1"), 7))
        );
        // SOCKS5's 3 is no type of its own
        server.write_all(&[3, 0]).await.unwrap();
        let err = udp.recv_from(&mut buf).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "anytls: a datagram of an unknown address type"
        );
    }
}
```

`crates/rurge-proto/src/anytls/mod.rs`——把

```rust
    use crate::testing::{AnyTlsScript, FakeAnyTls, TlsFixture, echo_server};
```

换成

```rust
    use crate::testing::{AnyTlsScript, FakeAnyTls, TlsFixture, echo_server, udp_echo_server};
```

`crates/rurge-proto/src/anytls/mod.rs`——把

```rust
    use rurge_net::connector::{DirectConnector, SystemResolve};
```

换成

```rust
    use rurge_net::connector::{DirectConnector, PacketSocket, SystemResolve};
```

`crates/rurge-proto/src/anytls/mod.rs`——把

```rust
        assert_eq!(fake.sessions(), 0);
    }
}
```

换成

```rust
        assert_eq!(fake.sessions(), 0);
    }

    async fn udp_answer(carrier: &dyn PacketSocket) -> (Vec<u8>, Target) {
        let mut buf = [0u8; 1500];
        let (n, from) = tokio::time::timeout(Duration::from_secs(5), carrier.recv_from(&mut buf))
            .await
            .expect("an answer within the bound")
            .unwrap();
        (buf[..n].to_vec(), from)
    }

    async fn udp_roundtrip(carrier: &dyn PacketSocket, to: SocketAddr, payload: &[u8]) {
        carrier.send_to(payload, &target(to)).await.unwrap();
        assert_eq!(udp_answer(carrier).await, (payload.to_vec(), target(to)));
    }

    /// UDP over TCP v2: one stream to the magic name, not in connect mode,
    /// whose request names the first datagram's target; every datagram
    /// carries its own.
    #[tokio::test]
    async fn udp_goes_through_a_stream_of_its_own() {
        let (one, two) = (udp_echo_server().await, udp_echo_server().await);
        let (fixture, fake) = server(script("pw")).await;
        let out = outbound(&line(&fake, ""), &fixture);
        assert_eq!(out.udp(), UdpSupport::Native);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), one, b"to one").await;
        udp_roundtrip(carrier.as_ref(), two, b"to two").await;
        let streams = fake.streams();
        assert_eq!(streams.len(), 1);
        assert_eq!(
            (streams[0].atyp, streams[0].host.as_str(), streams[0].port),
            (3, "sp.v2.udp-over-tcp.arpa", 0)
        );
        assert_eq!(fake.uot_requests(), [(0, one.to_string())]);
        assert_eq!(fake.datagrams(), [one.to_string(), two.to_string()]);
    }

    /// Full cone: whoever reaches the server's end of the stream is heard,
    /// under its own address.
    #[tokio::test]
    async fn anyone_may_answer_through_anytls() {
        let echo = udp_echo_server().await;
        let (fixture, fake) = server(script("pw")).await;
        let out = outbound(&line(&fake, ""), &fixture);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), echo, b"hello").await;
        let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        stranger
            .send_to(b"unasked", fake.udp_outside()[0])
            .await
            .unwrap();
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"unasked".to_vec(), target(stranger.local_addr().unwrap()))
        );
    }
}
```

`crates/rurge-engine/tests/udp_tls_family.rs`——把

```rust
//! rurge, out through `trojan` (UDP ASSOCIATE) to a loopback fake.
```

换成

```rust
//! rurge, out through `trojan` (UDP ASSOCIATE) and `anytls` (UDP over TCP)
//! to loopback fakes.
```

`crates/rurge-engine/tests/udp_tls_family.rs`——把

```rust
    assert_eq!(from, stranger);
}

```

换成

```rust
    assert_eq!(from, stranger);
}

#[tokio::test]
async fn udp_goes_through_anytls() {
    let origin = TestServer::spawn().await;
    let (upstream, params) = anytls_upstream(origin_addr(&origin)).await;
    two_echoes_through(&format!(
        "P = anytls, 127.0.0.1, {}, {params}",
        upstream.addr().port()
    ))
    .await;
    assert_eq!(
        upstream.uot_requests().len(),
        1,
        "one stream carries both flows"
    );
    assert_eq!(upstream.datagrams().len(), 3);
}

#[tokio::test]
async fn anyone_may_answer_through_anytls() {
    let origin = TestServer::spawn().await;
    let (upstream, params) = anytls_upstream(origin_addr(&origin)).await;
    let (from, stranger, _) = a_stranger_writes(
        &format!(
            "P = anytls, 127.0.0.1, {}, {params}",
            upstream.addr().port()
        ),
        || upstream.udp_outside()[0],
    )
    .await;
    assert_eq!(from, stranger);
}

```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto --lib -- anytls stream_udp`
Expected: FAIL——`Framing::Uot` 还不存在，用例用到的 `UdpSupport` 由 Step 3 引入 `anytls/mod.rs`：

```text
error[E0433]: failed to resolve: use of undeclared type `UdpSupport`
   --> crates\rurge-proto\src\anytls\mod.rs:596:31
error[E0599]: no variant or associated item named `Uot` found for enum `stream_udp::Framing` in the current scope
   --> crates\rurge-proto\src\stream_udp.rs:293:59
Some errors have detailed explanations: E0433, E0599.
For more information about an error, try `rustc --explain E0433`.
error: could not compile `rurge-proto` (lib test) due to 2 previous errors
exit 101
```

- [ ] **Step 3: 实现**

`crates/rurge-proto/src/stream_udp.rs`——把

```rust
//! UDP over one byte stream (M5 design §7): `trojan`'s UDP ASSOCIATE
//! carries every datagram, each way, as its address, its length and the
```

换成

```rust
//! UDP over one byte stream (M5 design §7): `trojan`'s UDP ASSOCIATE and
//! AnyTLS's UDP over TCP (sing's `uot`, version 2, not in connect mode)
//! carry every datagram, each way, as its address, its length and the
```

`crates/rurge-proto/src/stream_udp.rs`——把

```rust
    Trojan,
```

换成

```rust
    Trojan,
    /// `TYPE ADDR PORT LENGTH PAYLOAD`, the types numbered 0 = IPv4,
    /// 1 = IPv6, 2 = name (sing's `uot.AddrParser`); the head's own address
    /// keeps SOCKS5's numbers.
    Uot,
```

`crates/rurge-proto/src/stream_udp.rs`——把

```rust
            Framing::Trojan => "trojan",
```

换成

```rust
            Framing::Trojan => "trojan",
            Framing::Uot => "anytls",
        }
    }

    /// A datagram's address type on the wire, for SOCKS5's `atyp`.
    fn wire_type(self, atyp: u8) -> u8 {
        match (self, atyp) {
            (Framing::Uot, 1) => 0,
            (Framing::Uot, 4) => 1,
            (Framing::Uot, 3) => 2,
            _ => atyp,
        }
    }

    /// SOCKS5's `atyp` for a datagram's address type on the wire.
    fn socks_type(self, wire: u8) -> Option<u8> {
        match (self, wire) {
            (Framing::Trojan, 1 | 3 | 4) => Some(wire),
            (Framing::Uot, 0) => Some(1),
            (Framing::Uot, 1) => Some(4),
            (Framing::Uot, 2) => Some(3),
            _ => None,
```

`crates/rurge-proto/src/stream_udp.rs`——把

```rust
        match self {
            Framing::Trojan => out.extend_from_slice(b"\r\n"),
        }
        Ok(())
```

换成

```rust
        match self {
            Framing::Trojan => out.extend_from_slice(b"\r\n"),
            Framing::Uot => {}
        }
        Ok(())
```

`crates/rurge-proto/src/stream_udp.rs`——把

```rust
        out.extend(socks_addr(to).map_err(|e| self.unsendable(e))?);
```

换成

```rust
        let mut addr = socks_addr(to).map_err(|e| self.unsendable(e))?;
        addr[0] = self.wire_type(addr[0]);
        out.extend(addr);
```

`crates/rurge-proto/src/stream_udp.rs`——把

```rust
        match self {
            Framing::Trojan => out.extend_from_slice(b"\r\n"),
        }
        out.extend_from_slice(payload);
```

换成

```rust
        match self {
            Framing::Trojan => out.extend_from_slice(b"\r\n"),
            Framing::Uot => {}
        }
        out.extend_from_slice(payload);
```

`crates/rurge-proto/src/stream_udp.rs`——把

```rust
            Framing::Trojan => 2,
```

换成

```rust
            Framing::Trojan => 2,
            Framing::Uot => 0,
```

`crates/rurge-proto/src/stream_udp.rs`——把

```rust
                let rest = match addr[0] {
                    1 => 4 - 1,
                    4 => 16 - 1,
                    3 => usize::from(addr[1]),
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "{}: a datagram of an unknown address type",
                                self.framing.label()
                            ),
                        ));
                    }
```

换成

```rust
                let Some(atyp) = self.framing.socks_type(addr[0]) else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "{}: a datagram of an unknown address type",
                            self.framing.label()
                        ),
                    ));
                };
                addr[0] = atyp;
                let rest = match atyp {
                    1 => 4 - 1,
                    4 => 16 - 1,
                    _ => usize::from(addr[1]),
```

`crates/rurge-proto/src/anytls/mod.rs`——把

```rust
//! like a web site, and the stream ends in the relay.
```

换成

```rust
//! like a web site, and the stream ends in the relay.
//!
//! UDP rides a stream of its own to `UOT_MAGIC` (`stream_udp`).
```

`crates/rurge-proto/src/anytls/mod.rs`——把

```rust
use crate::task::AbortOnDrop;
use crate::transport::Stack;
use crate::{BuildError, Outbound, OutboundError};
```

换成

```rust
use crate::stream_udp::{Framing, StreamUdp};
use crate::task::AbortOnDrop;
use crate::transport::Stack;
use crate::{BuildError, Outbound, OutboundError, UdpSupport};
```

`crates/rurge-proto/src/anytls/mod.rs`——把

```rust
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
```

换成

```rust
use rurge_net::connector::{BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, Target};
```

`crates/rurge-proto/src/anytls/mod.rs`——把

```rust
use tokio::io::AsyncWriteExt;
```

换成

```rust
use tokio::io::AsyncWriteExt;

/// The target that asks the server for UDP over TCP, version 2 (sing's
/// `uot.MagicAddress`; port 0).
const UOT_MAGIC: &str = "sp.v2.udp-over-tcp.arpa";
```

`crates/rurge-proto/src/anytls/mod.rs`——把

```rust
        })
    }
}
```

换成

```rust
        })
    }

    fn udp(&self) -> UdpSupport {
        UdpSupport::Native
    }

    /// UDP over TCP, version 2 (sing's `uot`): a stream to the magic name,
    /// not in connect mode, each datagram with its address.
    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedPacketSocket, OutboundError>> {
        Box::pin(async move {
            let mut address = vec![3, UOT_MAGIC.len() as u8];
            address.extend_from_slice(UOT_MAGIC.as_bytes());
            address.extend_from_slice(&[0, 0]);
            let stream = match tokio::time::timeout(opts.timeout, self.open(&address, opts)).await {
                Ok(result) => result?,
                Err(_) => return Err(OutboundError::Timeout),
            };
            // `isConnect` false; the target comes with the first datagram
            Ok(Box::new(StreamUdp::new(stream, Framing::Uot, vec![0])) as BoxedPacketSocket)
        })
    }
}
```

要点：
- 请求头里的目标用 SOCKS5 的类型号（`SocksaddrSerializer`），每个数据报的地址用 0 / 1 / 2（`AddrParser`）——两者不同，别混用。
- 收到 SOCKS5 的类型号 3（在这里不是合法的类型）是 `anytls: a datagram of an unknown address type`，载体随之结束：流上的字节已经对不齐了。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto --lib -- anytls stream_udp trojan` → 通过（新增 `udp_goes_through_a_stream_of_its_own`、`anyone_may_answer_through_anytls`、`udp_over_tcp_has_its_own_address_types`）。
Run: `cargo test -p rurge-engine --test udp_tls_family` → 通过（新增 `udp_goes_through_anytls`、`anyone_may_answer_through_anytls`）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-proto crates/rurge-engine/tests/udp_tls_family.rs
git commit -m "feat(proto): anytls 的 UDP over TCP v2；FakeAnyTls 的 UDP"
```


### Task 3: `vmess` 的命令 2（每个目标一条连接）

VMess 的 UDP（P3、P5，M5-D5）：`header::request_plain` 多一个命令参数；`VmessOutbound` 把拨号需要的东西挪进共享的 `Dialer`（`connect_tcp` 与 UDP 载体共用）；`vmess::udp::VmessUdp` 在每个目标的第一个包时用命令 2 开一条连接，每个数据报一个分块，回包一律算作那个目标的。`FakeVmess` 遇到命令 2 时按分块收发数据报（P6）。

**Files:**
- Create: `crates/rurge-proto/src/vmess/udp.rs`
- Modify: `crates/rurge-proto/src/vmess/header.rs`（`COMMAND_TCP` / `COMMAND_UDP`、`request_plain` 的 `command` 参数，与用例）、`src/vmess/mod.rs`（`Dialer`、`udp()` / `open_udp()`，与用例）、`src/testing/vmess.rs`（命令 2）
- Modify: `crates/rurge-engine/tests/udp_tls_family.rs`

**Interfaces:**
- Consumes: Task 1 的 `udp_echo_server` 与引擎用例的辅助函数；既有的 `vmess::chunk::MAX_PAYLOAD`、`stream::VmessStream`、`LazyHead`。
- Produces:
  - `pub(crate) const header::{COMMAND_TCP, COMMAND_UDP}`；`header::request_plain(session, security, command: u8, address, padding)`
  - `vmess::Dialer`（私有）：`connect(command, target, opts) -> Result<BoxedStream, OutboundError>`
  - `VmessOutbound`：`udp()` 为 `Native`；`open_udp` 立即返回、不拨号
  - 测试设施：`FakeVmess::udp_outside() -> Vec<SocketAddr>`

- [ ] **Step 1: 先写用例（连同假服务端的 UDP）**

`crates/rurge-proto/src/testing/vmess.rs`——把

```rust
//! below the protocol, the sealed request head, then a chunked relay. It never
//! resolves a name.
```

换成

```rust
//! below the protocol, the sealed request head, then a chunked relay — of a
//! TCP stream, or for command 2 of datagrams, one per chunk, through a UDP
//! socket of the connection's own. It never resolves a name.
```

`crates/rurge-proto/src/testing/vmess.rs`——把

```rust
use tokio::net::{TcpListener, TcpStream};
```

换成

```rust
use tokio::net::{TcpListener, TcpStream, UdpSocket};
```

`crates/rurge-proto/src/testing/vmess.rs`——把

```rust
    rejected: Arc<AtomicUsize>,
    _task: AbortOnDrop,
```

换成

```rust
    rejected: Arc<AtomicUsize>,
    udp_outside: Arc<Mutex<Vec<SocketAddr>>>,
    _task: AbortOnDrop,
```

`crates/rurge-proto/src/testing/vmess.rs`——把

```rust
    rejected: Arc<AtomicUsize>,
}
```

换成

```rust
    rejected: Arc<AtomicUsize>,
    udp_outside: Arc<Mutex<Vec<SocketAddr>>>,
}
```

`crates/rurge-proto/src/testing/vmess.rs`——把

```rust
    };
    let upstream = TcpStream::connect(upstream_addr).await?;
```

换成

```rust
    };
```

`crates/rurge-proto/src/testing/vmess.rs`——把

```rust
    let (mut from_upstream, mut to_upstream) = upstream.into_split();
    let mut up = ChunkCipher::new(security, &session.body_key, &session.body_iv);
    let mut down = ChunkCipher::new(security, &key, &iv);
```

换成

```rust
    let mut up = ChunkCipher::new(security, &session.body_key, &session.body_iv);
    let mut down = ChunkCipher::new(security, &key, &iv);
    if parsed.record.command == 2 {
        // one datagram per chunk each way, to the target itself when it is
        // an address (`connect_to` is for names); answers from anyone go back
        let to = match parsed.record.host.parse::<IpAddr>() {
            Ok(ip) => SocketAddr::new(ip, parsed.record.port),
            Err(_) => upstream_addr,
        };
        let socket = UdpSocket::bind("127.0.0.1:0").await?;
        shared
            .udp_outside
            .lock()
            .expect("outside")
            .push(socket.local_addr()?);
        let upward = async {
            loop {
                let mut len = [0u8; 2];
                from_client.read_exact(&mut len).await?;
                let mut sealed = vec![0u8; up.open_len(len)];
                from_client.read_exact(&mut sealed).await?;
                match up.open(&mut sealed) {
                    Some(0) | None => return Ok::<(), io::Error>(()),
                    Some(n) => {
                        socket.send_to(&sealed[..n], to).await?;
                    }
                }
            }
        };
        let downward = async {
            to_client.write_all(&answer).await?;
            let mut buf = vec![0u8; 65536];
            loop {
                let n = match socket.recv_from(&mut buf).await {
                    Ok((n, _)) => n,
                    // an ICMP "unreachable" for an earlier datagram (Windows)
                    Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                    Err(e) => return Err::<(), io::Error>(e),
                };
                let mut out = Vec::new();
                down.seal(&buf[..n], &mut out);
                to_client.write_all(&out).await?;
            }
        };
        tokio::select! {
            _ = upward => {}
            _ = downward => {}
        }
        return Ok(());
    }
    let upstream = TcpStream::connect(upstream_addr).await?;
    let (mut from_upstream, mut to_upstream) = upstream.into_split();
```

`crates/rurge-proto/src/testing/vmess.rs`——把

```rust
        let rejected = Arc::new(AtomicUsize::new(0));
```

换成

```rust
        let rejected = Arc::new(AtomicUsize::new(0));
        let udp_outside: Arc<Mutex<Vec<SocketAddr>>> = Arc::default();
```

`crates/rurge-proto/src/testing/vmess.rs`——把

```rust
            rejected: rejected.clone(),
```

换成

```rust
            rejected: rejected.clone(),
            udp_outside: udp_outside.clone(),
```

`crates/rurge-proto/src/testing/vmess.rs`——把

```rust
            rejected,
```

换成

```rust
            rejected,
            udp_outside,
```

`crates/rurge-proto/src/testing/vmess.rs`——把

```rust
        self.rejected.load(Ordering::SeqCst)
    }
}
```

换成

```rust
        self.rejected.load(Ordering::SeqCst)
    }

    /// Where each UDP connection (command 2) sends from: a datagram to one
    /// of these goes back to that connection's client.
    pub fn udp_outside(&self) -> Vec<SocketAddr> {
        self.udp_outside.lock().expect("outside").clone()
    }
}
```

`crates/rurge-proto/src/vmess/header.rs`——把

```rust
            Security::Aes128Gcm,
```

换成

```rust
            Security::Aes128Gcm,
            COMMAND_TCP,
```

`crates/rurge-proto/src/vmess/header.rs`——把

```rust
    fn the_security_nibble_follows_the_cipher() {
```

换成

```rust
    fn the_security_nibble_follows_the_cipher_and_the_command_the_transport() {
```

`crates/rurge-proto/src/vmess/header.rs`——把

```rust
            Security::ChaCha20Poly1305,
```

换成

```rust
            Security::ChaCha20Poly1305,
            COMMAND_UDP,
```

`crates/rurge-proto/src/vmess/header.rs`——把

```rust
        assert_eq!(plain[34], OPTIONS);
```

换成

```rust
        assert_eq!(plain[34], OPTIONS);
        assert_eq!(plain[37], 2, "UDP");
```

`crates/rurge-proto/src/vmess/mod.rs`——把

```rust
    use crate::testing::{FakeVmess, TlsFixture, VmessScript, echo_server};
```

换成

```rust
    use crate::testing::{FakeVmess, TlsFixture, VmessScript, echo_server, udp_echo_server};
```

`crates/rurge-proto/src/vmess/mod.rs`——把

```rust
    use rurge_net::connector::{DirectConnector, SystemResolve};
```

换成

```rust
    use rurge_net::connector::{DirectConnector, PacketSocket, SystemResolve};
```

`crates/rurge-proto/src/vmess/mod.rs`——把

```rust
        assert_eq!(got, b"world");
    }
}
```

换成

```rust
        assert_eq!(got, b"world");
    }

    async fn udp_answer(carrier: &dyn PacketSocket) -> (Vec<u8>, Target) {
        let mut buf = vec![0u8; 65536];
        let (n, from) = tokio::time::timeout(Duration::from_secs(10), carrier.recv_from(&mut buf))
            .await
            .expect("an answer within the bound")
            .unwrap();
        (buf[..n].to_vec(), from)
    }

    async fn udp_roundtrip(carrier: &dyn PacketSocket, to: &Target, payload: &[u8]) {
        carrier.send_to(payload, to).await.unwrap();
        assert_eq!(udp_answer(carrier).await, (payload.to_vec(), to.clone()));
    }

    /// Command 2: one connection per target, opened by its first datagram;
    /// each datagram one chunk.
    #[tokio::test]
    async fn udp_opens_one_connection_per_target() {
        let (one, two) = (udp_echo_server().await, udp_echo_server().await);
        let fake = FakeVmess::spawn(VmessScript::new(ID), None).await;
        let out = outbound(
            &format!(
                "vmess, 127.0.0.1, {}, username={ID}, vmess-aead=true",
                fake.addr().port()
            ),
            no_roots(),
        );
        assert_eq!(out.udp(), UdpSupport::Native);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        assert_eq!(
            fake.connections(),
            0,
            "nothing is dialled before a datagram"
        );
        udp_roundtrip(carrier.as_ref(), &target(one), b"to one").await;
        udp_roundtrip(carrier.as_ref(), &target(two), b"to two").await;
        udp_roundtrip(carrier.as_ref(), &target(one), b"one again").await;
        let seen: Vec<_> = fake
            .requests()
            .iter()
            .map(|r| (r.command, r.options, r.port))
            .collect();
        assert_eq!(seen, [(2, 0x05, one.port()), (2, 0x05, two.port())]);
    }

    /// Symmetric: the server cannot say who answered, so whatever comes back
    /// on a target's connection counts as that target's.
    #[tokio::test]
    async fn every_answer_counts_as_the_targets() {
        let echo = udp_echo_server().await;
        let fake = FakeVmess::spawn(VmessScript::new(ID), None).await;
        let out = outbound(
            &format!(
                "vmess, 127.0.0.1, {}, username={ID}, vmess-aead=true",
                fake.addr().port()
            ),
            no_roots(),
        );
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), &target(echo), b"hello").await;
        let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        stranger
            .send_to(b"unasked", fake.udp_outside()[0])
            .await
            .unwrap();
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"unasked".to_vec(), target(echo))
        );
    }

    /// Over TLS and a WebSocket, a name going to the server; a datagram too
    /// long for one chunk is refused without a connection.
    #[tokio::test]
    async fn udp_over_tls_and_a_websocket_and_a_datagram_too_long() {
        let echo = udp_echo_server().await;
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let fake = FakeVmess::spawn(
            VmessScript {
                ws: true,
                connect_to: Some(echo),
                ..VmessScript::new(ID)
            },
            Some(fixture.clone()),
        )
        .await;
        let out = outbound(
            &format!(
                "vmess, 127.0.0.1, {}, username={ID}, vmess-aead=true, encrypt-method=chacha20-ietf-poly1305, tls=true, ws=true, ws-path=/v",
                fake.addr().port()
            ),
            fixture.roots(),
        );
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        let err = carrier
            .send_to(&vec![0u8; 16369], &target(echo))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "vmess: a datagram longer than 16368 bytes");
        assert_eq!(fake.connections(), 0);
        let name = Target::new(HostName::Domain("bücher.example".into()), 53);
        carrier.send_to(b"q", &name).await.unwrap();
        assert_eq!(udp_answer(carrier.as_ref()).await, (b"q".to_vec(), name));
        let seen = fake.requests();
        assert_eq!(
            (seen[0].command, seen[0].security, seen[0].host.as_str()),
            (2, 4, "xn--bcher-kva.example")
        );
    }
}
```

`crates/rurge-engine/tests/udp_tls_family.rs`——把

```rust
//! rurge, out through `trojan` (UDP ASSOCIATE) and `anytls` (UDP over TCP)
//! to loopback fakes.
```

换成

```rust
//! rurge, out through `trojan` (UDP ASSOCIATE), `anytls` (UDP over TCP) and
//! `vmess` (command 2, one connection per target) to loopback fakes.
```

`crates/rurge-engine/tests/udp_tls_family.rs`——把

```rust
            "P = anytls, 127.0.0.1, {}, {params}",
            upstream.addr().port()
        ),
        || upstream.udp_outside()[0],
    )
    .await;
    assert_eq!(from, stranger);
}

```

换成

```rust
            "P = anytls, 127.0.0.1, {}, {params}",
            upstream.addr().port()
        ),
        || upstream.udp_outside()[0],
    )
    .await;
    assert_eq!(from, stranger);
}

#[tokio::test]
async fn udp_goes_through_vmess_one_connection_per_target() {
    let origin = TestServer::spawn().await;
    let (upstream, params) = vmess_upstream(true, true, origin_addr(&origin)).await;
    two_echoes_through(&format!(
        "P = vmess, 127.0.0.1, {}, {params}",
        upstream.addr().port()
    ))
    .await;
    let commands: Vec<u8> = upstream.requests().iter().map(|r| r.command).collect();
    assert_eq!(commands, [2, 2], "one connection per target");
}

/// Symmetric through vmess: an answer can only come back on its target's
/// connection, and counts as the target's.
#[tokio::test]
async fn through_vmess_every_answer_is_the_targets() {
    let origin = TestServer::spawn().await;
    let (upstream, params) = vmess_upstream(false, false, origin_addr(&origin)).await;
    let (from, _, echo) = a_stranger_writes(
        &format!("P = vmess, 127.0.0.1, {}, {params}", upstream.addr().port()),
        || upstream.udp_outside()[0],
    )
    .await;
    assert_eq!(from, echo);
}

```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto --lib vmess`
Expected: FAIL——`request_plain` 还没有命令参数，`COMMAND_UDP` 还不存在，用例用到的 `UdpSupport` 由 Step 3 引入 `vmess/mod.rs`：

```text
error[E0425]: cannot find value `COMMAND_UDP` in this scope
   --> crates\rurge-proto\src\vmess\header.rs:262:13
error[E0433]: failed to resolve: use of undeclared type `UdpSupport`
   --> crates\rurge-proto\src\vmess\mod.rs:616:31
error[E0061]: this function takes 4 arguments but 5 arguments were supplied
   --> crates\rurge-proto\src\vmess\header.rs:238:21
   --> crates\rurge-proto\src\vmess\header.rs:67:15
error[E0061]: this function takes 4 arguments but 5 arguments were supplied
   --> crates\rurge-proto\src\vmess\header.rs:259:21
   --> crates\rurge-proto\src\vmess\header.rs:67:15
Some errors have detailed explanations: E0061, E0425, E0433.
For more information about an error, try `rustc --explain E0061`.
error: could not compile `rurge-proto` (lib test) due to 4 previous errors
exit 101
```

- [ ] **Step 3: 实现**

`crates/rurge-proto/src/vmess/header.rs`——把

```rust
const COMMAND_TCP: u8 = 1;
```

换成

```rust
/// The request's command: a TCP stream, or UDP to one target (each chunk
/// one datagram).
pub(crate) const COMMAND_TCP: u8 = 1;
pub(crate) const COMMAND_UDP: u8 = 2;
```

`crates/rurge-proto/src/vmess/header.rs`——把

```rust
/// The head before sealing. `address` is `port ‖ type ‖ address`
/// (`addr::vmess_addr`); `padding` is 0 – 15 random bytes.
```

换成

```rust
/// The head before sealing. `command` is `COMMAND_TCP` or `COMMAND_UDP`;
/// `address` is `port ‖ type ‖ address` (`addr::vmess_addr`); `padding` is
/// 0 – 15 random bytes.
```

`crates/rurge-proto/src/vmess/header.rs`——把

```rust
    security: Security,
```

换成

```rust
    security: Security,
    command: u8,
```

`crates/rurge-proto/src/vmess/header.rs`——把

```rust
    out.push(COMMAND_TCP);
```

换成

```rust
    out.push(command);
```

`crates/rurge-proto/src/vmess/mod.rs`——把

```rust
//! both directions (`stream`).
```

换成

```rust
//! both directions (`stream`). UDP is command 2, one connection per target
//! (`udp`).
```

`crates/rurge-proto/src/vmess/mod.rs`——把

```rust
mod stream;
```

换成

```rust
mod stream;
mod udp;
```

`crates/rurge-proto/src/vmess/mod.rs`——把

```rust
use crate::{BuildError, Outbound, OutboundError};
```

换成

```rust
use crate::{BuildError, Outbound, OutboundError, UdpSupport};
```

`crates/rurge-proto/src/vmess/mod.rs`——把

```rust
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
```

换成

```rust
use rurge_net::connector::{BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, Target};
```

`crates/rurge-proto/src/vmess/mod.rs`——把

```rust
use stream::VmessStream;
```

换成

```rust
use stream::VmessStream;
use udp::VmessUdp;
```

`crates/rurge-proto/src/vmess/mod.rs`——把

```rust
    name: String,
```

换成

```rust
    name: String,
    dialer: Arc<Dialer>,
}

/// What opening a VMess connection takes; shared with the UDP carrier,
/// which opens one per target.
struct Dialer {
```

`crates/rurge-proto/src/vmess/mod.rs`——把

```rust
            stack: Stack::new(connector, server, shadow_tls, tls, ws),
            cmd_key: header::cmd_key(spec.uuid.expose()),
            security: match spec.cipher {
                VmessCipher::Aes128Gcm => Security::Aes128Gcm,
                VmessCipher::ChaCha20Poly1305 => Security::ChaCha20Poly1305,
            },
        })
    }

    /// The sealed request head for `target`, and the secrets it announces.
    fn head(&self, target: &Target) -> Result<(Vec<u8>, Session), OutboundError> {
```

换成

```rust
            dialer: Arc::new(Dialer {
                stack: Stack::new(connector, server, shadow_tls, tls, ws),
                cmd_key: header::cmd_key(spec.uuid.expose()),
                security: match spec.cipher {
                    VmessCipher::Aes128Gcm => Security::Aes128Gcm,
                    VmessCipher::ChaCha20Poly1305 => Security::ChaCha20Poly1305,
                },
            }),
        })
    }
}

impl Dialer {
    /// The sealed request head for `command` to `target`, and the secrets it
    /// announces.
    fn head(&self, command: u8, target: &Target) -> Result<(Vec<u8>, Session), OutboundError> {
```

`crates/rurge-proto/src/vmess/mod.rs`——把

```rust
        let plain = header::request_plain(&session, self.security, &address, padding);
        let head = header::seal_request(&self.cmd_key, &auth_id, &random()?, &plain);
        Ok((head, session))
```

换成

```rust
        let plain = header::request_plain(&session, self.security, command, &address, padding);
        let head = header::seal_request(&self.cmd_key, &auth_id, &random()?, &plain);
        Ok((head, session))
    }

    /// A connection for `command` to `target`: the chunked stream, its head
    /// queued for the first payload.
    async fn connect(
        &self,
        command: u8,
        target: &Target,
        opts: &ConnectOpts,
    ) -> Result<BoxedStream, OutboundError> {
        // never dial for a target whose name cannot be sent
        let (head, session) = self.head(command, target)?;
        // one budget for the connection, TLS and the WebSocket handshake;
        // the server's answer comes with its first payload, in the relay
        let transport = match tokio::time::timeout(opts.timeout, self.stack.open(opts)).await {
            Ok(result) => result?,
            Err(_) => return Err(OutboundError::Timeout),
        };
        let lazy: BoxedStream = Box::new(LazyHead::new(transport, head));
        Ok(Box::new(VmessStream::new(lazy, session, self.security)) as BoxedStream)
```

`crates/rurge-proto/src/vmess/mod.rs`——把

```rust
        Box::pin(async move {
            // never dial for a target whose name cannot be sent
            let (head, session) = self.head(target)?;
            // one budget for the connection, TLS and the WebSocket handshake;
            // the server's answer comes with its first payload, in the relay
            let transport = match tokio::time::timeout(opts.timeout, self.stack.open(opts)).await {
                Ok(result) => result?,
                Err(_) => return Err(OutboundError::Timeout),
            };
            let lazy: BoxedStream = Box::new(LazyHead::new(transport, head));
            Ok(Box::new(VmessStream::new(lazy, session, self.security)) as BoxedStream)
        })
```

换成

```rust
        Box::pin(self.dialer.connect(header::COMMAND_TCP, target, opts))
    }

    fn udp(&self) -> UdpSupport {
        UdpSupport::Native
    }

    /// Nothing is dialled yet: each target gets its connection with its
    /// first datagram.
    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedPacketSocket, OutboundError>> {
        let udp = VmessUdp::new(self.dialer.clone(), opts.clone());
        Box::pin(std::future::ready(Ok(Box::new(udp) as BoxedPacketSocket)))
```

新建 `crates/rurge-proto/src/vmess/udp.rs`：

```rust
//! VMess UDP (M5-D5): command 2, one connection per target, each chunk one
//! datagram. The server answers on the target's connection only, so every
//! answer counts as the target's (symmetric; no XUDP). A connection is
//! opened with its target's first datagram; one that ends is opened again
//! by the next datagram.

use super::Dialer;
use super::chunk::MAX_PAYLOAD;
use super::header::COMMAND_UDP;
use crate::task::AbortOnDrop;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, PacketSocket, Target};
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt, WriteHalf};
use tokio::sync::{Mutex, OnceCell, mpsc};
use tokio_util::sync::CancellationToken;

/// Answers waiting for the engine; more are dropped, as a socket would.
const INBOX: usize = 64;

/// One target's connection.
struct Conn {
    writer: Mutex<WriteHalf<BoxedStream>>,
    /// Fires when the connection's reading ends: it is dialled again.
    ended: CancellationToken,
    _reader: AbortOnDrop,
}

type Slot = Arc<OnceCell<Arc<Conn>>>;

pub(crate) struct VmessUdp {
    dialer: Arc<Dialer>,
    opts: ConnectOpts,
    conns: std::sync::Mutex<HashMap<Target, Slot>>,
    answers: mpsc::Sender<(Vec<u8>, Target)>,
    inbox: Mutex<mpsc::Receiver<(Vec<u8>, Target)>>,
}

impl VmessUdp {
    pub(super) fn new(dialer: Arc<Dialer>, opts: ConnectOpts) -> VmessUdp {
        let (answers, inbox) = mpsc::channel(INBOX);
        VmessUdp {
            dialer,
            opts,
            conns: std::sync::Mutex::default(),
            answers,
            inbox: Mutex::new(inbox),
        }
    }

    /// `to`'s slot; a slot whose connection has ended is replaced.
    fn slot(&self, to: &Target) -> Slot {
        let mut conns = self.conns.lock().expect("conns");
        let slot = conns.entry(to.clone()).or_default();
        if slot.get().is_some_and(|c| c.ended.is_cancelled()) {
            *slot = Slot::default();
        }
        slot.clone()
    }

    /// Forgets `slot` for `to`, unless another has taken its place.
    fn forget(&self, to: &Target, slot: &Slot) {
        let mut conns = self.conns.lock().expect("conns");
        if conns.get(to).is_some_and(|s| Arc::ptr_eq(s, slot)) {
            conns.remove(to);
        }
    }

    async fn dial(&self, to: &Target) -> io::Result<Arc<Conn>> {
        let stream = self
            .dialer
            .connect(COMMAND_UDP, to, &self.opts)
            .await
            .map_err(|e| io::Error::other(e.to_string()))?;
        let (mut reader, writer) = tokio::io::split(stream);
        let ended = CancellationToken::new();
        let (answers, from, done) = (self.answers.clone(), to.clone(), ended.clone());
        let task = tokio::spawn(async move {
            // a read returns one chunk when the buffer holds a whole one
            let mut buf = vec![0u8; MAX_PAYLOAD];
            while let Ok(n) = reader.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                // a full inbox drops the answer
                let _ = answers.try_send((buf[..n].to_vec(), from.clone()));
            }
            done.cancel();
        });
        Ok(Arc::new(Conn {
            writer: Mutex::new(writer),
            ended,
            _reader: AbortOnDrop(task),
        }))
    }
}

impl PacketSocket for VmessUdp {
    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            // one datagram, one chunk
            if buf.len() > MAX_PAYLOAD {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("vmess: a datagram longer than {MAX_PAYLOAD} bytes"),
                ));
            }
            if buf.is_empty() {
                // an empty chunk would end the connection
                return Ok(());
            }
            let slot = self.slot(to);
            let conn = match slot.get_or_try_init(|| self.dial(to)).await {
                Ok(conn) => conn.clone(),
                Err(e) => {
                    self.forget(to, &slot);
                    return Err(e);
                }
            };
            let mut writer = conn.writer.lock().await;
            let written = async {
                writer.write_all(buf).await?;
                writer.flush().await
            };
            if let Err(e) = written.await {
                self.forget(to, &slot);
                return Err(e);
            }
            Ok(())
        })
    }

    /// An answer longer than `buf` is dropped: give it 64 KiB.
    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            let mut inbox = self.inbox.lock().await;
            loop {
                // `self` holds a sender: the inbox never closes
                let Some((answer, from)) = inbox.recv().await else {
                    return Err(io::ErrorKind::BrokenPipe.into());
                };
                if let Some(space) = buf.get_mut(..answer.len()) {
                    space.copy_from_slice(&answer);
                    return Ok((answer.len(), from));
                }
            }
        })
    }
}
```

要点：
- `VmessStream` 的一次写（不超过 `MAX_PAYLOAD`）恰好是一个分块；一次读在缓冲区装得下一个分块时恰好读回一个分块。所以读任务用 `MAX_PAYLOAD` 大小的缓冲区，一次读就是一个数据报。
- 空载荷不写（一个空分块在 VMess 里是"流结束"）。
- 表里一个目标对应一个 `OnceCell`：同一目标同时到达的两个包只拨一次号；拨号失败或写失败时把这个格子从表里拿掉（除非别人已经换上了新的），下一个包重新拨号。
- `Dialer::head` 与原来的 `VmessOutbound::head` 相同，只多了命令参数；`connect_tcp` 的行为不变。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto --lib vmess` → 通过（新增 `udp_opens_one_connection_per_target`、`every_answer_counts_as_the_targets`、`udp_over_tls_and_a_websocket_and_a_datagram_too_long`；`the_security_nibble_follows_the_cipher` 改名为 `the_security_nibble_follows_the_cipher_and_the_command_the_transport` 并多了命令字节的断言）。
Run: `cargo test -p rurge-engine --test udp_tls_family` → 通过（新增 `udp_goes_through_vmess_one_connection_per_target`、`through_vmess_every_answer_is_the_targets`）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-proto crates/rurge-engine/tests/udp_tls_family.rs
git commit -m "feat(proto): vmess 的 UDP——命令 2，每个目标一条连接、每个数据报一个分块；FakeVmess 的 UDP"
```


### Task 4: 对 sing-box / xray 的 UDP 互操作；文档

三种出站的 UDP 对 sing-box 的 `trojan` / `vmess` / `anytls` 入站各往返两次，`vmess` 的命令 2 另对 xray（两种 `encrypt-method`、两个目标轮流）。本机没有 sing-box 与 xray：用例按既有的约定跳过，只在装了它们的 CI 上真正跑（`RURGE_TEST_SING_BOX` / `RURGE_TEST_XRAY`，`RURGE_INTEROP_REQUIRED=1` 时没装即失败）。文档：兼容性清单登记三种协议 UDP 的做法与差异（`vmess` 对称型）、手工验收的 M5b 一节、两份 README、`CLAUDE.md` 与 `tests/interop/README.md`。

**Files:**
- Modify: `tests/interop/tests/common/mod.rs`（`udp_roundtrip`）、`tests/interop/tests/sing_box_tls_family.rs`、`tests/interop/tests/xray.rs`、`tests/interop/README.md`
- Modify: `docs/surge-compatibility-matrix.md`、`docs/acceptance/phase2-manual.md`、`README.md`、`README_en.md`、`CLAUDE.md`

**Interfaces:**
- Consumes: Task 1–3 的 `open_udp`、`PacketSocket::{send_to, recv_from}`、`rurge_proto::testing::udp_echo_server`；`tests/interop` 既有的 `sing_box_or_skip`、`SingBox::spawn`、`trojan_inbound`、`vmess_inbound`、`Inbound { kind: InboundKind::AnyTls, .. }`、`leaf_files`、`outbound`、`target`，`xray_or_skip`、`Xray::spawn`、`XrayInbound`。
- Produces: 无（用例与文档）。

- [ ] **Step 1: 写互操作用例**

`tests/interop/tests/common/mod.rs`——把

```rust
pub use rurge_proto::testing::{TlsFixture, echo_server};
```

换成

```rust
pub use rurge_proto::testing::{TlsFixture, echo_server, udp_echo_server};
```

`tests/interop/tests/common/mod.rs`——把

```rust
    assert!(back == payload, "the echo differs");
}

```

换成

```rust
    assert!(back == payload, "the echo differs");
}

/// One datagram to `echo` through a fresh UDP carrier of `out`, and its
/// answer. Bounded like `roundtrip`.
pub async fn udp_roundtrip(out: &OutboundRef, echo: SocketAddr) {
    let bound = std::time::Duration::from_secs(10);
    let carrier = tokio::time::timeout(bound, out.open_udp(&ConnectOpts::default()))
        .await
        .expect("the carrier opens within the bound")
        .expect("the carrier opens");
    for payload in [&b"interop"[..], b"again"] {
        carrier.send_to(payload, &target(echo)).await.unwrap();
        let mut buf = [0u8; 64];
        let (n, from) = tokio::time::timeout(bound, carrier.recv_from(&mut buf))
            .await
            .expect("the echo comes back within the bound")
            .unwrap();
        assert_eq!((&buf[..n], from), (payload, target(echo)));
    }
}

```

`tests/interop/tests/sing_box_tls_family.rs`——把

```rust
        roundtrip(&once, echo).await;
    }
}

```

换成

```rust
        roundtrip(&once, echo).await;
    }
}

/// UDP (phase 2 M5b): trojan's UDP ASSOCIATE, vmess command 2 and AnyTLS's
/// UDP over TCP v2, each to a loopback UDP echo through sing-box.
#[tokio::test]
async fn udp_through_trojan_vmess_and_anytls() {
    let Some(bin) = sing_box_or_skip("udp_through_trojan_vmess_and_anytls") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let sb = SingBox::spawn(
        &bin,
        dir.path(),
        vec![
            trojan_inbound(leaf_files(&fixture, dir.path()), None),
            vmess_inbound(None, None),
            Inbound {
                kind: InboundKind::AnyTls,
                users: vec![("u".into(), "s3same".into())],
                tls: Some(leaf_files(&fixture, dir.path())),
                ws_path: None,
            },
        ],
    );
    let echo = udp_echo_server().await;
    let profile = format!(
        "[Proxy]\nT = trojan, 127.0.0.1, {}, password=s3same\nV = vmess, 127.0.0.1, {}, username={VMESS_ID}, vmess-aead=true\nA = anytls, 127.0.0.1, {}, password=s3same\n[Rule]\nFINAL,DIRECT\n",
        sb.port(0),
        sb.port(1),
        sb.port(2)
    );
    for name in ["T", "V", "A"] {
        udp_roundtrip(&outbound(&profile, name, Some(&fixture)), echo).await;
    }
}

```

`tests/interop/tests/xray.rs`——把

```rust
use rurge_proto::testing::echo_server;
```

换成

```rust
use rurge_proto::testing::{echo_server, udp_echo_server};
```

`tests/interop/tests/xray.rs`——把

```rust
        roundtrip(&out, echo, &big).await;
    }
}

```

换成

```rust
        roundtrip(&out, echo, &big).await;
    }
}

/// UDP (phase 2 M5b): command 2, one connection per target, each datagram
/// one chunk; either cipher.
#[tokio::test]
async fn vmess_udp_with_either_cipher() {
    let Some(bin) = xray_or_skip("vmess_udp_with_either_cipher") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let xray = Xray::spawn(
        &bin,
        dir.path(),
        vec![XrayInbound {
            uuid: VMESS_ID.into(),
            ws_path: None,
        }],
    );
    let (one, two) = (udp_echo_server().await, udp_echo_server().await);
    let profile = format!(
        "[Proxy]\nPlain = vmess, 127.0.0.1, {0}, username={VMESS_ID}, vmess-aead=true\nChacha = vmess, 127.0.0.1, {0}, username={VMESS_ID}, vmess-aead=true, encrypt-method=chacha20-ietf-poly1305\n[Rule]\nFINAL,DIRECT\n",
        xray.port(0)
    );
    let bound = Duration::from_secs(10);
    for name in ["Plain", "Chacha"] {
        let carrier = tokio::time::timeout(
            bound,
            outbound(&profile, name).open_udp(&ConnectOpts::default()),
        )
        .await
        .expect("the carrier opens within the bound")
        .expect("the carrier opens");
        for echo in [one, two, one] {
            let to = Target::new(rurge_config::HostName::Ip(echo.ip()), echo.port());
            carrier.send_to(b"interop", &to).await.unwrap();
            let mut buf = [0u8; 64];
            let (n, from) = tokio::time::timeout(bound, carrier.recv_from(&mut buf))
                .await
                .expect("the echo comes back within the bound")
                .unwrap();
            assert_eq!((&buf[..n], from), (&b"interop"[..], to));
        }
    }
}

```

`tests/interop/README.md`——把

```markdown
本地默认不安装 sing-box 与 xray：`cargo test -p rurge-interop` 会正常通过，sing-box 的十二个互操作用例与 xray 的一个互操作用例各打印一行 `skipping …` 后直接返回（两个夹具自身的单元测试——配置渲染、"配置绝不碰本机"的安全守卫、取空闲端口——照常运行并断言）。要在本机真正跑 sing-box 的用例，二选一：
```

换成

```markdown
本地默认不安装 sing-box 与 xray：`cargo test -p rurge-interop` 会正常通过，sing-box 的十三个互操作用例与 xray 的两个互操作用例各打印一行 `skipping …` 后直接返回（两个夹具自身的单元测试——配置渲染、"配置绝不碰本机"的安全守卫、取空闲端口——照常运行并断言）。要在本机真正跑 sing-box 的用例，二选一：
```

`tests/interop/README.md`——把

```markdown
`tests/sing_box_tls_family.rs` 驱动的五个用例覆盖：
```

换成

```markdown
`tests/sing_box_tls_family.rs` 驱动的六个用例覆盖：
```

`tests/interop/README.md`——把

```markdown
- `anytls`：同一个出站发起多条流默认复用同一个会话（`reuse` 的协议默认值），以及 `reuse=false` 时每条流各开一个新会话。
```

换成

```markdown
- `anytls`：同一个出站发起多条流默认复用同一个会话（`reuse` 的协议默认值），以及 `reuse=false` 时每条流各开一个新会话。
- 三种协议的 UDP（阶段 2 / M5b）：`trojan` 的 UDP ASSOCIATE、`vmess` 的命令 2、`anytls` 的 UDP over TCP v2，各经 sing-box 往返一个回环 UDP 回显两次。
```

`tests/interop/README.md`——把

```markdown
`tests/xray.rs` 驱动的一个用例覆盖 `vmess`：两种 `encrypt-method`（默认的 `aes-128-gcm` 与 `chacha20-ietf-poly1305`）、可选的 V2Ray WebSocket 传输，以及单块与跨多块（100 000 字节）的往返。
```

换成

```markdown
`tests/xray.rs` 驱动的两个用例覆盖 `vmess`：两种 `encrypt-method`（默认的 `aes-128-gcm` 与 `chacha20-ietf-poly1305`）、可选的 V2Ray WebSocket 传输，以及单块与跨多块（100 000 字节）的往返；另一个用例覆盖命令 2 的 UDP（阶段 2 / M5b）：两种 `encrypt-method`，经同一个载体轮流发往两个回环 UDP 回显（每个目标一条连接）。
```

- [ ] **Step 2: 运行**

Run: `cargo test -p rurge-interop`
Expected: 本机没有 sing-box 与 xray 时全部通过，新用例在输出里说明跳过（`skipping udp_through_trojan_vmess_and_anytls: ...`、`skipping vmess_udp_with_either_cipher: ...`）——本任务没有能在本机跑出来的失败。装了它们的环境上用例真正经 sing-box / xray 往返。

- [ ] **Step 3: 提交用例**

```bash
git add tests/interop
git commit -m "test(interop): trojan / vmess / anytls 的 UDP 对 sing-box，vmess 的命令 2 对 xray"
```

- [ ] **Step 4: 文档**

兼容性清单（三种协议的 UDP 与 `vmess` 的对称型差异）：

`docs/surge-compatibility-matrix.md`——把

```markdown
| `vmess` | VMess（AEAD / 旧握手、TLS、WebSocket） | 全部 | ✅ | 2 | M2b（阶段 2）已实现 AEAD 握手（TCP）：没写 `vmess-aead=true` 的行在 M8 之前按 `W0007`（每次加载一条）+ `REJECT` 处理，会话日志 `policy protocol not implemented: vmess (legacy handshake)`；请求头与首段负载合并成一次写出，客户端 100 ms 内不发数据时（服务端先说话的协议）请求头单独发出、这类协议的首字节因此晚 100 ms；只开 ChunkStream + ChunkMasking（Surge 的实际取值未公开）；UUID 错与本机时钟偏差超过约 120 秒都表现为 `vmess: the server closed the connection without answering`，两者分辨不出；`username` 只接受命名写法；`tls=false` 时写的 TLS 参数按 `W0028` 处理；默认不带 ALPN（未与真实 Surge 核对）；UDP 属 M5；目标主机名的字母表规则同 http / socks5 |
| `trojan` | Trojan（TLS、WebSocket） | 全部 | ✅ | 2 | M2a（阶段 2）已实现（TCP）：TLS 必有，可叠加 WebSocket；请求头与首段负载合并成一次写出，客户端 100 ms 内不发数据时（服务端先说话的协议）请求头单独发出、这类协议的首字节因此晚 100 ms；密码错误在连接期无法识别（协议没有应答，服务端把连接交给它的回落站点）；默认不带 ALPN（未与真实 Surge 核对）；口令只接受 password=（手册的写法），位置参数不读；UDP 属 M5；目标主机名的字母表规则同 http / socks5 |
```

换成

```markdown
| `vmess` | VMess（AEAD / 旧握手、TLS、WebSocket） | 全部 | ✅ | 2 | M2b（阶段 2）已实现 AEAD 握手（TCP）：没写 `vmess-aead=true` 的行在 M8 之前按 `W0007`（每次加载一条）+ `REJECT` 处理，会话日志 `policy protocol not implemented: vmess (legacy handshake)`；请求头与首段负载合并成一次写出，客户端 100 ms 内不发数据时（服务端先说话的协议）请求头单独发出、这类协议的首字节因此晚 100 ms；只开 ChunkStream + ChunkMasking（Surge 的实际取值未公开）；UUID 错与本机时钟偏差超过约 120 秒都表现为 `vmess: the server closed the connection without answering`，两者分辨不出；`username` 只接受命名写法；`tls=false` 时写的 TLS 参数按 `W0028` 处理；默认不带 ALPN（未与真实 Surge 核对）；UDP（M5b，阶段 2）：命令 2，每个目标一条 VMess 连接，随它的第一个包建立、断了由下一个包重建，每个数据报一个分块，超过 16368 字节的数据报发不出去；对称型——服务端不说回包来自谁，回包一律算作那个目标的（没有 XUDP，M5-D5）；目标主机名的字母表规则同 http / socks5 |
| `trojan` | Trojan（TLS、WebSocket） | 全部 | ✅ | 2 | M2a（阶段 2）已实现（TCP）：TLS 必有，可叠加 WebSocket；请求头与首段负载合并成一次写出，客户端 100 ms 内不发数据时（服务端先说话的协议）请求头单独发出、这类协议的首字节因此晚 100 ms；密码错误在连接期无法识别（协议没有应答，服务端把连接交给它的回落站点）；默认不带 ALPN（未与真实 Surge 核对）；口令只接受 password=（手册的写法），位置参数不读；UDP（M5b，阶段 2）：一条连接上的 UDP ASSOCIATE（命令 3），请求头随第一个包发出、写的是那个包的目标（与参考客户端相同），之后每个包带自己的地址与长度；全锥（服务端那一端的任何来源都送回客户端，来源原样）；目标主机名的字母表规则同 http / socks5 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `anytls` | AnyTLS v2 | iOS 5.17 / Mac 6.4.3+ | ✅ | 2 | M2b（阶段 2）已实现（TCP）：一条会话同一时刻只承载一个流（与参考实现一致）；空闲超过 60 秒的会话每 30 秒回收一次；不等 `cmdSYNACK` 就返回流，被拒的流在第一次读上以 `anytls: <服务端文本>` 失败；连接超时只约束到 TCP 连接、TLS 握手、鉴权写出与首个会话层包（`cmdSettings ‖ cmdSYN ‖ cmdPSH`）**入队**为止，该包的物理写出由会话自己的任务完成，此后的卡顿由转发阶段的空闲超时兜底；**没有半关闭**：客户端方向的 EOF 以 `cmdFIN` 结束整条流（与 sing-box 一致）；不实现参考客户端的 3 秒 SYNACK 看门狗；`cmdHeartRequest` 一律不等写队列腾出空间就应答，队列满时应答直接丢弃（繁忙的写端本身就是存活证明）——如果服务端靠未应答的心跳计数判断连接已死，一个正在满负荷写的会话可能因此被这样的服务端误判为已死并关闭；服务端推送的 padding 方案做有界校验（原文 ≤ 8192 字节、`stop` ≤ 256、每包 ≤ 64 项、每项 1 ..= 16384），不合法就保留旧方案；`password` 只接受命名写法；默认不带 ALPN（未与真实 Surge 核对）；UDP（udp-over-tcp v2）属 M5；目标主机名的字母表规则同 http / socks5 |
```

换成

```markdown
| `anytls` | AnyTLS v2 | iOS 5.17 / Mac 6.4.3+ | ✅ | 2 | M2b（阶段 2）已实现（TCP）：一条会话同一时刻只承载一个流（与参考实现一致）；空闲超过 60 秒的会话每 30 秒回收一次；不等 `cmdSYNACK` 就返回流，被拒的流在第一次读上以 `anytls: <服务端文本>` 失败；连接超时只约束到 TCP 连接、TLS 握手、鉴权写出与首个会话层包（`cmdSettings ‖ cmdSYN ‖ cmdPSH`）**入队**为止，该包的物理写出由会话自己的任务完成，此后的卡顿由转发阶段的空闲超时兜底；**没有半关闭**：客户端方向的 EOF 以 `cmdFIN` 结束整条流（与 sing-box 一致）；不实现参考客户端的 3 秒 SYNACK 看门狗；`cmdHeartRequest` 一律不等写队列腾出空间就应答，队列满时应答直接丢弃（繁忙的写端本身就是存活证明）——如果服务端靠未应答的心跳计数判断连接已死，一个正在满负荷写的会话可能因此被这样的服务端误判为已死并关闭；服务端推送的 padding 方案做有界校验（原文 ≤ 8192 字节、`stop` ≤ 256、每包 ≤ 64 项、每项 1 ..= 16384），不合法就保留旧方案；`password` 只接受命名写法；默认不带 ALPN（未与真实 Surge 核对）；UDP（M5b，阶段 2）：UDP over TCP v2（sing 的 `uot`）——一条流的目标是 `sp.v2.udp-over-tcp.arpa:0`，请求非连接模式、写第一个包的目标，每个包带自己的地址（类型号 0 / 1 / 2）与长度；全锥；这条流占一个会话；目标主机名的字母表规则同 http / socks5 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| 自动支持 UDP 的协议：Snell v3+、VMess、Trojan、TUIC、Hysteria 2、MASQUE、AnyTLS（UDP over TCP）、WireGuard、Tailscale | | ✅ | 2 |
```

换成

```markdown
| 自动支持 UDP 的协议：Snell v3+、VMess、Trojan、TUIC、Hysteria 2、MASQUE、AnyTLS（UDP over TCP）、WireGuard、Tailscale | | ✅ M5b：VMess（对称型）、Trojan、AnyTLS 已生效；WireGuard 随 M5c，其余随各自的协议 | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `vmess` | `username`（UUID）`encrypt-method`（`aes-128-gcm` / `chacha20-ietf-poly1305`，默认前者）`vmess-aead`（默认 false）`tls` `ws` `ws-path`（默认 `/`）`ws-headers`（`\|` 分隔） | ✅ | 2 | 自动 UDP（属 M5）；`username` 只接受命名写法，取常见的 8-4-4-4-12 或不带连字符的 32 位十六进制两种写法；`vmess-aead=false`（默认）即请求旧式握手，加载时 `W0007`、运行时 `REJECT`（见 4.2 与 `W0007` 行）；`tls=false` 时写的 TLS 参数按 `W0028` 处理；`ws-path` / `ws-headers` 与 trojan 同一套规则（见下一行） |
| `trojan` | `password` `ws` `ws-path` `ws-headers` | ✅ | 2 | 自动 UDP；`ws-path` 必须是以 `/` 开头的 ASCII 路径（无空白与控制字符）；`ws-headers` 的名字必须是 HTTP token、值只允许 HTAB 一种控制字符；`Connection` / `Upgrade` / `Sec-WebSocket-*` 由握手自己写，出现时 `W0012` 并忽略；`Host` 缺省取服务器主机名（端口非 443 时带端口；未与真实 Surge 核对）；不支持 early data；入站帧上限 1 MiB；经 WebSocket 时，半关闭（客户端关闭写方向）发出的是 WebSocket Close 帧，多数服务端把它当作整条连接的结束——关闭写方向后还在等响应的客户端可能拿不到完整响应；不带 ws 的 trojan 保持真正的半关闭（close_notify + FIN） |
```

换成

```markdown
| `vmess` | `username`（UUID）`encrypt-method`（`aes-128-gcm` / `chacha20-ietf-poly1305`，默认前者）`vmess-aead`（默认 false）`tls` `ws` `ws-path`（默认 `/`）`ws-headers`（`\|` 分隔） | ✅ | 2 | 自动 UDP（M5b 起生效，对称型）；`username` 只接受命名写法，取常见的 8-4-4-4-12 或不带连字符的 32 位十六进制两种写法；`vmess-aead=false`（默认）即请求旧式握手，加载时 `W0007`、运行时 `REJECT`（见 4.2 与 `W0007` 行）；`tls=false` 时写的 TLS 参数按 `W0028` 处理；`ws-path` / `ws-headers` 与 trojan 同一套规则（见下一行） |
| `trojan` | `password` `ws` `ws-path` `ws-headers` | ✅ | 2 | 自动 UDP（M5b 起生效）；`ws-path` 必须是以 `/` 开头的 ASCII 路径（无空白与控制字符）；`ws-headers` 的名字必须是 HTTP token、值只允许 HTAB 一种控制字符；`Connection` / `Upgrade` / `Sec-WebSocket-*` 由握手自己写，出现时 `W0012` 并忽略；`Host` 缺省取服务器主机名（端口非 443 时带端口；未与真实 Surge 核对）；不支持 early data；入站帧上限 1 MiB；经 WebSocket 时，半关闭（客户端关闭写方向）发出的是 WebSocket Close 帧，多数服务端把它当作整条连接的结束——关闭写方向后还在等响应的客户端可能拿不到完整响应；不带 ws 的 trojan 保持真正的半关闭（close_notify + FIN） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `anytls` | `password` `reuse`（默认 true） | ✅ | 2 | 自动 UDP（udp-over-tcp v2，属 M5）；`password` 只接受命名写法；`reuse=false` 时每个流各自新建会话、用完即关，不进池 |
```

换成

```markdown
| `anytls` | `password` `reuse`（默认 true） | ✅ | 2 | 自动 UDP（udp-over-tcp v2，M5b 起生效）；`password` 只接受命名写法；`reuse=false` 时每个流各自新建会话、用完即关，不进池 |
```

手工验收的 M5b 一节（真实节点，项目所有者验收）：

`docs/acceptance/phase2-manual.md`——把

```markdown
- [ ] NAT 后面的中继：上游 SOCKS5 节点放在带 NAT 的云主机上（本机私网地址、另有公网地址），看它对 UDP ASSOCIATE 回的中继地址：回未指定地址（`0.0.0.0`）或公网地址时 UDP 能往返；回私网地址时 UDP 不通（已知限制，见兼容性清单 `socks5` 一行），记下节点软件与它的设置。

```

换成

```markdown
- [ ] NAT 后面的中继：上游 SOCKS5 节点放在带 NAT 的云主机上（本机私网地址、另有公网地址），看它对 UDP ASSOCIATE 回的中继地址：回未指定地址（`0.0.0.0`）或公网地址时 UDP 能往返；回私网地址时 UDP 不通（已知限制，见兼容性清单 `socks5` 一行），记下节点软件与它的设置。

## M5b　TLS 族的 UDP

前置：同 M5a 一节的 SOCKS5 UDP 客户端；自己的 `trojan`、`vmess`（`vmess-aead=true`）、`anytls` 节点各一个（服务端是 sing-box、xray 或其它常见实现，记下是哪个与版本）。

- [ ] DNS：规则把 UDP 分到每个节点各一次，经 SOCKS5 UDP 发 DNS 查询（如 Proxifier 代理 `nslookup example.com 8.8.8.8`），都得到回答；请求记录的策略一列是那个节点。
- [ ] 游戏或语音：经每个节点各进行一次语音通话或联机游戏，都能通话 / 联机。
- [ ] 全锥与对称：用 NAT 类型检测工具（STUN）经 rurge 的 SOCKS5 检测——经 `trojan` 与 `anytls` 节点是 Full Cone（节点本身的出口须是全锥）；经 `vmess` 节点是 Symmetric（M5-D5，已知差异）。
- [ ] WebSocket 与 TLS：`trojan` 或 `vmess` 节点开 `ws=true` 时 UDP 照常往返。

```

两份 README 与 `CLAUDE.md`：

`README.md`——把

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；M4c（external）已完成——`external` 策略（TCP；第一次用到时拉起外部程序、经它在本机端口上的 SOCKS5 转发；程序退出后下次用到时再拉起；输出写进数据目录；rurge 退出时连同它启动的进程一起停掉——Unix 用进程组，Windows 用 Job Object；三平台都支持，Surge 只在 Mac 上有）已可用，订阅导入的 `external` 一律跳过；M5a（UDP 地基）已完成——SOCKS5 监听支持 UDP ASSOCIATE，UDP 按规则分流到 DIRECT、REJECT 或 `socks5` / `socks5-tls` / `external`（`udp-relay=true`，含 `underlying-proxy` 链），全锥 NAT，每条 UDP 流一条请求记录，`block-quic` 与 `udp-policy-not-supported-behaviour` 生效（`trojan` / `vmess` / `anytls` / `wireguard` 的 UDP 在 M5b / M5c）；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

换成

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；M4c（external）已完成——`external` 策略（TCP；第一次用到时拉起外部程序、经它在本机端口上的 SOCKS5 转发；程序退出后下次用到时再拉起；输出写进数据目录；rurge 退出时连同它启动的进程一起停掉——Unix 用进程组，Windows 用 Job Object；三平台都支持，Surge 只在 Mac 上有）已可用，订阅导入的 `external` 一律跳过；M5a（UDP 地基）已完成——SOCKS5 监听支持 UDP ASSOCIATE，UDP 按规则分流到 DIRECT、REJECT 或 `socks5` / `socks5-tls` / `external`（`udp-relay=true`，含 `underlying-proxy` 链），全锥 NAT，每条 UDP 流一条请求记录，`block-quic` 与 `udp-policy-not-supported-behaviour` 生效；M5b（TLS 族的 UDP）已完成——`trojan`（UDP ASSOCIATE）、`anytls`（UDP over TCP v2）全锥，`vmess`（命令 2，每个目标一条连接）对称型（`wireguard` 的 UDP 在 M5c）；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

`README_en.md`——把

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; M4b (WireGuard) is done — the `wireguard` policy (TCP; a user-space tunnel, no virtual interface; several peers chosen by `allowed-ips`; `client-id` (WARP's reserved bytes); destination names looked up through the tunnel when the section has a `dns-server`, on this machine otherwise; tested by a handshake when there is neither a `dns-server` nor a `test-url`) is usable; M4c (external) is done — the `external` policy (TCP; the program is started on first use and reached as a SOCKS5 proxy on its local port; a program that exited is started again on the next use; its output goes to the data directory; when rurge exits, the program is stopped together with whatever it started — a process group on Unix, a Job Object on Windows; all three platforms, where Surge has it on the Mac only) is usable, and subscriptions never bring `external` policies in; M5a (the UDP foundation) is done — the SOCKS5 listener takes UDP ASSOCIATE, and UDP is routed by rule to DIRECT, REJECT or `socks5` / `socks5-tls` / `external` (`udp-relay=true`, `underlying-proxy` chains included), full-cone NAT, one request record per UDP flow, and `block-quic` and `udp-policy-not-supported-behaviour` take effect (UDP over `trojan` / `vmess` / `anytls` / `wireguard` comes in M5b / M5c); the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

换成

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; M4b (WireGuard) is done — the `wireguard` policy (TCP; a user-space tunnel, no virtual interface; several peers chosen by `allowed-ips`; `client-id` (WARP's reserved bytes); destination names looked up through the tunnel when the section has a `dns-server`, on this machine otherwise; tested by a handshake when there is neither a `dns-server` nor a `test-url`) is usable; M4c (external) is done — the `external` policy (TCP; the program is started on first use and reached as a SOCKS5 proxy on its local port; a program that exited is started again on the next use; its output goes to the data directory; when rurge exits, the program is stopped together with whatever it started — a process group on Unix, a Job Object on Windows; all three platforms, where Surge has it on the Mac only) is usable, and subscriptions never bring `external` policies in; M5a (the UDP foundation) is done — the SOCKS5 listener takes UDP ASSOCIATE, and UDP is routed by rule to DIRECT, REJECT or `socks5` / `socks5-tls` / `external` (`udp-relay=true`, `underlying-proxy` chains included), full-cone NAT, one request record per UDP flow, and `block-quic` and `udp-policy-not-supported-behaviour` take effect; M5b (UDP over the TLS family) is done — `trojan` (UDP ASSOCIATE) and `anytls` (UDP over TCP v2) with full-cone NAT, `vmess` (command 2, one connection per target) symmetric (UDP over `wireguard` comes in M5c); the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

`CLAUDE.md`——把

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、发送时本机地址或路由失效或握手无回应时换新载体（网络变化后自愈）、网络变化入口、同一私钥与 peer 只留一条隧道，节与载体设置都相同的策略共用）、`stream`、`dns`（隧道内 DNS 与缓存；每个问题 2 秒内重发、先等隧道第一次握手）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。M4c（external）已完成——`rurge-config::spec::ExternalSpec`（`args` 是 `Secret`；两个 `external` 策略写了同一个 `local-port` 是 `E0018`；`args` 进内联参数的脱敏名单）、订阅导入的 `external` 行一律跳过（`W0023`）；`rurge-platform::process`（Unix 进程组与 `killpg`，Windows Job Object）；`rurge_proto::external`（`ExternalOutbound`：第一次用到时拉起、输出写 `<数据目录>/external/<策略名>.log` 且超过 1 MiB 时在拉起前轮转、子进程环境去掉代理变量并设 `NO_PROXY=*`、程序退出后下次用到时再拉起且两次拉起至少隔 2 秒、每 500 ms 连一次本机端口最多 6 次；`ProcessHook` / `ProcessGroup` 与 `NoProcessGroups`；程序自己退出时连同它启动的进程一起结束）；`rurge-engine` 的工厂分支、`EngineShared.processes` / `externals` 与 `Engine::stop_external_programs`；bin 的 `PlatformProcesses` 与退出流程最后停掉全部外部程序；只用于测试的工作区成员 `tests/external`（`rurge-external-tests`：极小的 SOCKS5 辅助程序 `socks-helper` 与真实拉起它的用例）；能力表翻转 `external`（`W0007` 不再因 `external` 出现）。M4 至此完成。M5（UDP 路径）按三份计划推进（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）：M5a 已完成——`rurge_net::connector::PacketSocket`（按包收发、带地址的 UDP 载体）与 `Connector::open_udp`（`DirectConnector`：每个地址族一个未连接的 socket，忽略 Windows 的 ICMP 不可达报错）；`Outbound::udp()` / `open_udp()` 与 `UdpSupport`；DIRECT 与 `socks5` / `socks5-tls` / `external` 的 UDP（`udp-relay`，`W0029` 退役）；`ChainConnector::open_udp`（链式 UDP 载体）；`rurge-inbound` 的 SOCKS5 UDP ASSOCIATE（`UdpClient`、`UdpAdmission`、`Dialer::admit_udp` / `associate`）；`rurge-engine` 的 UDP 流水线（`udp` 模块：一条流 = 关联 + 目标、按"关联 × 出站"共用载体的全锥、60 秒 / DNS 10 秒回收、1024 流 / 4096 关联的上限）、请求记录与 API 的 `transport`、`block-quic`（`W0029` 退役）与 `udp-policy-not-supported-behaviour`、QUIC Initial 识别、`PROTOCOL` 规则按传输层匹配 `TCP` / `UDP`；`FakeSocks5` 与 `tests/external` 辅助程序的 UDP ASSOCIATE；对 sing-box `socks` 入站的 UDP 互操作用例。
```

换成

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、发送时本机地址或路由失效或握手无回应时换新载体（网络变化后自愈）、网络变化入口、同一私钥与 peer 只留一条隧道，节与载体设置都相同的策略共用）、`stream`、`dns`（隧道内 DNS 与缓存；每个问题 2 秒内重发、先等隧道第一次握手）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。M4c（external）已完成——`rurge-config::spec::ExternalSpec`（`args` 是 `Secret`；两个 `external` 策略写了同一个 `local-port` 是 `E0018`；`args` 进内联参数的脱敏名单）、订阅导入的 `external` 行一律跳过（`W0023`）；`rurge-platform::process`（Unix 进程组与 `killpg`，Windows Job Object）；`rurge_proto::external`（`ExternalOutbound`：第一次用到时拉起、输出写 `<数据目录>/external/<策略名>.log` 且超过 1 MiB 时在拉起前轮转、子进程环境去掉代理变量并设 `NO_PROXY=*`、程序退出后下次用到时再拉起且两次拉起至少隔 2 秒、每 500 ms 连一次本机端口最多 6 次；`ProcessHook` / `ProcessGroup` 与 `NoProcessGroups`；程序自己退出时连同它启动的进程一起结束）；`rurge-engine` 的工厂分支、`EngineShared.processes` / `externals` 与 `Engine::stop_external_programs`；bin 的 `PlatformProcesses` 与退出流程最后停掉全部外部程序；只用于测试的工作区成员 `tests/external`（`rurge-external-tests`：极小的 SOCKS5 辅助程序 `socks-helper` 与真实拉起它的用例）；能力表翻转 `external`（`W0007` 不再因 `external` 出现）。M4 至此完成。M5（UDP 路径）按三份计划推进（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）：M5a 已完成——`rurge_net::connector::PacketSocket`（按包收发、带地址的 UDP 载体）与 `Connector::open_udp`（`DirectConnector`：每个地址族一个未连接的 socket，忽略 Windows 的 ICMP 不可达报错）；`Outbound::udp()` / `open_udp()` 与 `UdpSupport`；DIRECT 与 `socks5` / `socks5-tls` / `external` 的 UDP（`udp-relay`，`W0029` 退役）；`ChainConnector::open_udp`（链式 UDP 载体）；`rurge-inbound` 的 SOCKS5 UDP ASSOCIATE（`UdpClient`、`UdpAdmission`、`Dialer::admit_udp` / `associate`）；`rurge-engine` 的 UDP 流水线（`udp` 模块：一条流 = 关联 + 目标、按"关联 × 出站"共用载体的全锥、60 秒 / DNS 10 秒回收、1024 流 / 4096 关联的上限）、请求记录与 API 的 `transport`、`block-quic`（`W0029` 退役）与 `udp-policy-not-supported-behaviour`、QUIC Initial 识别、`PROTOCOL` 规则按传输层匹配 `TCP` / `UDP`；`FakeSocks5` 与 `tests/external` 辅助程序的 UDP ASSOCIATE；对 sing-box `socks` 入站的 UDP 互操作用例。M5b（TLS 族的 UDP）已完成——`rurge-proto` 的 `stream_udp`（一条字节流上按包收发：请求头随第一个包发出、写那个包的目标；`trojan` 的 UDP ASSOCIATE 与 `anytls` 的 UDP over TCP v2 两种封装）、`trojan` / `anytls` 的 `open_udp`（全锥）、`vmess` 的命令 2（`vmess::udp`：每个目标一条 VMess 连接、随它的第一个包建立、每个数据报一个分块，对称型）；`FakeTrojan` / `FakeAnyTls` / `FakeVmess` 的 UDP 与 `rurge_proto::testing::udp_echo_server`；经引擎的端到端用例（`tests/udp_tls_family.rs`）；对 sing-box（三种）与 xray（vmess）的 UDP 互操作用例。
```

`CLAUDE.md`——把

```markdown
- `docs/superpowers/specs/2026-09-29-phase2-m5-udp-design.md`：阶段 2 / M5 细化设计（UDP 路径），细化总设计的 M5 里程碑、不一致处以它为准。三份计划的拆分（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）；已决事项 M5-D1 ～ D11（三类用途都要、全锥 NAT、按包收发的 `PacketSocket`、VMess 先做对称型、`ecn` 延后、60 秒 / DNS 10 秒回收、QUIC 只在 UDP 443 上识别、全局 `block-quic` 按取值名理解、REJECT 对 UDP 一律丢包、1024 流 / 4096 关联的上限）；第 15 节 V1 ～ V10 是写各份计划时必须核对的事项，第 16 节是任务草图，第 17 节是 M5a 计划期的订正。
- `docs/superpowers/plans/2026-09-29-phase2-m5a-udp-foundation-plan.md`：阶段 2 / M5a（UDP 地基）实施计划（7 个任务）。开头「计划期决定」表记录核对源码与手册得出的结论和与设计文字不同的决定；末尾「执行期修正记录」与「延后事项」两张表。
```

换成

```markdown
- `docs/superpowers/specs/2026-09-29-phase2-m5-udp-design.md`：阶段 2 / M5 细化设计（UDP 路径），细化总设计的 M5 里程碑、不一致处以它为准。三份计划的拆分（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）；已决事项 M5-D1 ～ D11（三类用途都要、全锥 NAT、按包收发的 `PacketSocket`、VMess 先做对称型、`ecn` 延后、60 秒 / DNS 10 秒回收、QUIC 只在 UDP 443 上识别、全局 `block-quic` 按取值名理解、REJECT 对 UDP 一律丢包、1024 流 / 4096 关联的上限）；第 15 节 V1 ～ V10 是写各份计划时必须核对的事项，第 16 节是任务草图，第 17 节是 M5a 计划期的订正，第 18 节是 M5a 实施期的订正，第 19 节是 M5b 计划期的订正。
- `docs/superpowers/plans/2026-09-29-phase2-m5a-udp-foundation-plan.md`：阶段 2 / M5a（UDP 地基）实施计划（7 个任务）。开头「计划期决定」表记录核对源码与手册得出的结论和与设计文字不同的决定；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/superpowers/plans/2026-09-29-phase2-m5b-udp-tls-family-plan.md`：阶段 2 / M5b（TLS 族的 UDP）实施计划（4 个任务）。开头「计划期决定」表记录核对参考实现源码（sing 的 `uot`、sing-vmess、trojan-gfw 协议文档）得出的逐字节细节与和设计文字不同的决定；末尾「执行期修正记录」与「延后事项」两张表。
```

`CLAUDE.md`——把

```markdown
cargo test -p rurge-engine --test udp          # UDP 流水线：SOCKS5 UDP ASSOCIATE → DIRECT / REJECT / socks5（含链）、全锥、QUIC 阻断、不支持 UDP 的策略、关联结束与流数上限
```

换成

```markdown
cargo test -p rurge-engine --test udp          # UDP 流水线：SOCKS5 UDP ASSOCIATE → DIRECT / REJECT / socks5（含链）、全锥、QUIC 阻断、不支持 UDP 的策略、关联结束与流数上限
cargo test -p rurge-engine --test udp_tls_family   # 经 trojan / anytls（全锥）与 vmess（每个目标一条连接、对称）的 UDP 端到端用例（回环假服务端）
```

- [ ] **Step 5: 门禁与提交**

跑门禁。写本计划时副本上最后一次全工作区门禁：fmt / clippy 通过，`cargo test --workspace` 53 个测试二进制、1238 通过、0 失败、2 忽略（`rurge-engine --test outbounds_tls_family` 首轮以 `STATUS_HEAP_CORRUPTION` 异常退出——本机已知的既有问题——重跑通过）。

```bash
git add docs README.md README_en.md CLAUDE.md
git commit -m "docs: M5b TLS 族的 UDP——兼容性清单、手工验收、README 与 CLAUDE.md"
```

---

## 验收对照（设计第 11 节，M5b 部分）

| # | 验收项 | 由谁保证 |
| - | ------ | -------- |
| 1 | 经 rurge 的 SOCKS5 UDP，`trojan` / `vmess` / `anytls` 对回环假服务端往返；全锥用例通过（vmess 除外，它是对称型）；对 sing-box / xray 的互操作在 CI 上通过 | Task 1：`udp_goes_through_trojan`、`anyone_may_answer_through_trojan`（引擎）与出站库的三条；Task 2：`udp_goes_through_anytls`、`anyone_may_answer_through_anytls` 与出站库的两条；Task 3：`udp_goes_through_vmess_one_connection_per_target`、`through_vmess_every_answer_is_the_targets` 与出站库的三条；Task 4：`udp_through_trojan_vmess_and_anytls`（sing-box）、`vmess_udp_with_either_cipher`（xray），CI。`wireguard` 在 M5c |
| 2 | `block-quic` | 不在本计划（M5a）；三种协议的 QUIC 流照 M5a 的规则判定（它们都是代理） |
| 3 | 不支持 UDP 的策略按 `udp-policy-not-supported-behaviour` 处理 | 不在本计划（M5a）；三种协议从此支持 UDP，不再落到这条规则 |
| 4 | `W0029` | 本计划不涉及（三种协议的 UDP 没有配置项） |
| 5 | 门禁全绿 | 各任务的门禁 |
| 6 | 需要真实环境的项目进手工验收清单 | Task 4：`docs/acceptance/phase2-manual.md` 的 M5b 一节 |
| — | 第 10 节第 1 层（三种协议的包格式向量） | Task 1：`the_head_rides_with_the_first_datagram_and_names_its_target`、`datagrams_come_back_with_their_source`、`an_unsendable_name_is_refused_and_the_head_waits`、`the_servers_end_is_the_carriers_end`；Task 2：`udp_over_tcp_has_its_own_address_types`；Task 3：`the_security_nibble_follows_the_cipher_and_the_command_the_transport` |
| — | 第 10 节第 2 层（`FakeTrojan` / `FakeVmess` / `FakeAnyTls` 支持 UDP） | Task 1–3 的出站库用例与引擎用例 |

## 执行期修正记录

| # | 任务 | 与计划的出入 | 原因 |
| - | ---- | ------------ | ---- |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | vmess 的 XUDP（全锥）（M5-D5） | 需要时另议 |
| 2 | vmess 的每目标连接随载体存续：一条流回收后，它那条连接要等载体（关联在这个出站上的最后一条流）结束才关（P5） | 需要时另议（例如按空闲时间单独回收） |
| 3 | vmess 的请求记录 `connectMs` 记的是载体就绪（不拨号）的时刻，而真正的拨号发生在第一个包（P5） | 需要时另议 |
| 4 | Shadow TLS 上的 UDP 没有单独的用例（P8） | 需要时另议 |
