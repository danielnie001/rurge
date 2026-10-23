# 阶段 3 / M3c「DNS 应答器、预匹配与经监听器来的 fake 地址」实施计划

> **执行方式**：子代理驱动（superpowers:subagent-driven-development）——每个任务一个新的实现者，做完由新的评审者评审，全部做完后整分支评审一次。本计划写到"改哪里、接口是什么、要哪些用例、判据是什么"为止：实现者先写失败的用例、看到红，再实现到绿；与计划的出入记进末尾的「执行期修正记录」。

**目标**：先把 M3b 留下的拆分做掉（`rurge-tun` 的大测试文件与基准、`dispatch` 与 `udp` 的模块环，不改行为），再在 M3a 的 TUN 内核与 M3b 的 fake-IP 池、引擎接入之上，做出虚拟网卡的 DNS 应答器（UDP 与 TCP、`hijack-dns`、fake 应答、`always-real-ip`、SVCB / HTTPS、canary、其余类型原样转发）、预匹配在 DNS 与 SYN 两层的拒绝（规则引擎的新入口、引擎的钩子、自动升级、每条规则 5 分钟记一次），经 HTTP / SOCKS5 监听器来的、目的是 fake 地址的会话换回名字，以及收尾 M3 的文档。

**架构**：任务 0 只挪代码、不改行为。`rurge-dns` 多一个原样转发的入口 `Resolver::forward`（按名字选上游、换 ID、不进缓存）和应答器要的报文辅助函数（读请求、造应答），UDP 上游改为自己分配不冲突的 ID。`rurge-rules` 的预匹配换成一个入口 `RuleEngine::pre_match(session, layer)`：DNS 层没有连接，要看端口、目的地址、传输与 HTTP 的条件答"要连接才知道"（三值，`Verdict::NeedsConnection`），逻辑规则与集合按它组合，所以带端口条件的规则在 DNS 层既不命中、也不因 `NOT` 翻成命中。`rurge-tun` 新加 `dns` 模块：应答器的判定在一处（`Responder::answer`），UDP 与 TCP、应答器地址与被劫持的服务器都经它；SYN 在建套接字之前问一次钩子。引擎实现 `Hooks::pre_match`（规则模式才看、自动升级、记录限频），并在 `Engine::dial` 与 SOCKS5 的 UDP 流里把 rurge 地址空间里的目的地经池换回名字。

**技术栈**：hickory-proto 0.26.2（`rurge-dns` 已依赖：`Message`、`Edns`、`Name`、`Record`）、smoltcp 0.12.0 的 `wire`、tokio；不新增包，也不新增依赖边。

**依据**：设计 `docs/superpowers/specs/2026-10-18-phase3-m3-tun-core-design.md`（下面写"设计"指它）第 2、3.3、9.4、11 ～ 16、17 节（V7 ～ V9、V12）与 M3-D4、D8、D19 ～ D21、D23、D24、D26；M3a 计划 `docs/superpowers/plans/2026-10-18-phase3-m3a-tun-tcp-plan.md`（下面写"M3a"）与 M3b 计划 `docs/superpowers/plans/2026-10-22-phase3-m3b-udp-icmp-pool-engine-plan.md`（下面写"M3b"）——两者的接口以合并之后的代码为准（计划期决定 P1）；M3b「延后事项」第 2、3、5、7、9、12、13、15、29 行与第 11 行的一条的去向是本计划（第 9、15 行与第 11 行的一条是任务 0，第 13、29 行是任务 4，第 12 行是任务 5；计划期决定 P27、P28），第 16 ～ 22 行（"下次动到时"）里落在本计划要改的文件上的几处顺带做掉，写在各任务的"顺带"一条里。

## 全局约束

- 不新增外部包，不新增依赖边（`rurge-tun → rurge-dns`、`rurge-engine → rurge-tun` 是 M3b 加的）。`Cargo.lock` 不变（518 个包）。`rurge-tun` 照旧不依赖 `rurge-engine`、`rurge-rules`（经 `rurge-dns` 间接可达也不直接用）与 `rurge-platform`。
- `rurge-platform` 放开 `unsafe_code` 的函数名单（`LISTED`，七个）不变；别的 crate 照旧 `forbid`。MSRV 1.89，edition 2024。
- `TunOptions` 新加的字段一律进 `TunOptions::check`（M3b 任务 4 的规矩：范围照现有字段的写法，越界按字段的次序报第一个），`tests::options::` 加它们的边界用例；`Tun::start` 对不合格的选项 panic 的规矩不变（起 TUN 的调用者先调 `check`）。
- 名字与地址只在 DEBUG 的行与请求记录里（设计第 14 节、M3-D26）；WARN 不带名字与地址；引擎的会话日志行照今天的写法不改（计划期决定 P18）。DNS 查询的名字、转发的上游与应答码只在 DEBUG。
- 测试只用回环与端口 0。`rurge-tun` 的用例经假 `Dialer`、假 `Hooks` 与假的转发（`FakeDns`，什么也不发出去）；经引擎的端到端用例守设计第 15.2 节的两道防线（计划期决定 P24）；`rurge-dns` 的转发用例只问回环上的 `MockDns` 或内存里的上游（`MockDns::upstream()`）。用例从不建网卡、不改路由与 DNS、不问真实的 DNS 服务器。
- 等待都有时限，不用固定的 sleep 当同步（"断言什么都没发生"除外，注释写它等的是什么、余量多大）。新写的计时用例提交前单独循环跑二十次以上。跑 TUN 内核的用例不用暂停的时钟（M3a 的规矩）；`rurge-dns` 里依赖时间的转发用例走暂停的时钟与内存里的上游（M8c 的规矩）；碰到进程查找的用例不用暂停的时钟（M2 的规矩）。
- 锁不跨 `await`。钩子 `pre_match` 是同步的、从不等待（设计第 12.2 节），可以在片的锁里调用（P16）。
- 先写失败的用例、看到红，再实现到绿。现有用例变红时：计划点了名的照计划改并在提交说明里说；没点名的停下来报告，不顺手改期望值。
- 门禁（每个任务末尾一次）：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`（本机 `RUSTFMT` 指向 gnu 工具链的 rustfmt）。聚焦地跑用例也经整个工作区（`cargo test -j 8 --workspace --lib -- <过滤词>`，集成测试 `cargo test -j 8 --workspace --test <名字> -- <过滤词>`），不用 `-p`（唯一的例外是 release 下单独跑的忽略基准）。测试二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出时单独重跑那一个。跑全量之前 `df -h /d`，剩余不足约 15 GiB 就停下。
- 只运行本仓库构建出来的二进制与系统自带的只读工具；不下载、不安装任何东西。
- 参考实现：GPL 的（mihomo、sing-box、sing-tun）只取事实，代码不抄；MIT / Apache 的可以简短引用并注明出处（hickory 是 MIT / Apache）；smoltcp 是 0BSD。
- 注释、日志与错误文本用英文，照所在文件的写法；提交说明的标题用中文。
- 没有新的诊断码，不碰配置格式；`[General]` 的 `allow-dns-svcb`、`hijack-dns`、`always-real-ip` 的读法不变（它们今天只被解析，这一份才有读的人）。

## 评审要点

计划的用例之外最可能咬到使用者的五种情形，每条的用例已加进负责的任务：

1. **用 0x20 大小写混排的解析器**（Unbound、一些路由器与浏览器内置的解析器）：应答照原样回显问题的每个字节（大小写不变），同一个名字不论大小写拿同一个 fake 地址；转发出去之后，上游应答里的问题只差大小写也收下（任务 1 `forward_accepts_a_reply_whose_question_differs_only_in_case`，任务 3 `a_mixed_case_query_gets_its_own_case_back`）。
2. **大于 UDP 限度的应答**（大的 TXT、带签名的记录、一长串地址）：UDP 上置 TC、不带记录、应答码照上游的，应用改用 TCP 之后拿到整份（任务 5 `a_large_answer_is_truncated_and_tcp_gets_it_whole`）。
3. **带端口条件的预匹配规则**（`AND,((DOMAIN-SUFFIX,ads.example),(DEST-PORT,443)),REJECT,pre-matching`，或含 `NOT,((DEST-PORT,…))`、`NOT,((IP-CIDR,…))` 的）：DNS 层不拒绝，也不因 `NOT` 把"不知道"翻成拒绝一切；SYN 层只拒绝条件成立的连接（任务 2 `a_port_condition_keeps_a_rule_from_the_dns_layer`、`not_never_turns_not_knowing_into_a_hit`，任务 6 `pre_matching_at_the_syn_layer_resets_only_the_port_it_names`）。
4. **`hijack-dns` 劫持的服务器的 53 端口上不是 DNS 查询的包**（QUIC、游戏、应答方向的包、别的 opcode）**与 TCP**：照常作 UDP 会话与 TCP 连接，不被应答器吞掉；IPv6 的从不劫持（任务 4 `a_packet_to_a_hijacked_server_that_is_not_a_query_is_a_session`、`ipv6_is_never_hijacked`，任务 5 `tcp_to_a_hijacked_server_is_a_connection`）。
5. **一阵转发**（浏览器为几百个名字预取 HTTPS 记录、邮件客户端查 MX / TXT）：同时在转发的至多 256 个，其余当场 SERVFAIL，不排队、不拖住 fake 应答（任务 3 `forwards_in_flight_are_capped_at_256`、`a_fake_answer_does_not_wait_for_forwards`）。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `crates/rurge-tun/src/tests/{bench,udp,fixture,lifecycle,outcomes,icmp}/`（由同名的 `.rs` 拆成目录）、`src/tests/support.rs` | 0 | 拆测试文件与基准；用例的路径前缀不变 |
| `crates/rurge-tun/src/context.rs`（新）、`src/{dispatch.rs,icmp.rs,packet.rs,lib.rs}`、`src/udp/{mod.rs,client.rs}` | 0 | `Context` 挪出 `dispatch`；造 UDP 包的函数挪进 `packet.rs`；`Session::ending()` 改名 |
| `crates/rurge-dns/src/message.rs` | 1 | 报文头与问题的读取、请求的读取、应答的构造、OPT 的大小；`Rcode::FormErr` / `NotImp` |
| `crates/rurge-dns/src/fanout.rs`、`src/resolver.rs` | 1 | 原样转发的扇出 `forward_raw`；`Resolver::forward` 与上游的选法 |
| `crates/rurge-dns/src/upstream/udp.rs` | 1 | UDP 上游给每个在途的查询一个不冲突的 ID |
| `crates/rurge-dns/src/testing.rs` | 1 | `query_wire`；`MockDns` 回答任何类型、`set_txt`、`wires` |
| `crates/rurge-config/src/session.rs` | 2 | `PreMatchLayer` |
| `crates/rurge-rules/src/{matcher.rs,set.rs,engine.rs,pre_matching.rs,lib.rs}` | 2 | `Verdict::NeedsConnection`、`EvalCtx` 的层、逻辑规则与集合的组合、`RuleEngine::pre_match` |
| `crates/rurge-tun/src/hooks.rs`、`src/space.rs` | 3 | `TunDns`、`TunSettings` 的新字段、`Hooks::pre_match`；`198.18.0.244` 的常量 |
| `crates/rurge-tun/src/dns/{mod.rs,answer.rs}`（新） | 3 | `Responder`：一份判定，UDP 与 TCP 共用 |
| `crates/rurge-tun/src/testing/{hooks.rs,dns.rs（新）}` | 3 | 假 `Hooks` 的预匹配脚本、`FakeDns` |
| `crates/rurge-engine/src/{runtime.rs,tun.rs}` | 3、6 | `Runtime::build` 填 `TunSettings` 的新字段（任务 3）；`pre_match` 的实现（任务 6） |
| `crates/rurge-tun/src/{dispatch.rs,udp/mod.rs,lib.rs,fragment.rs}`、`src/dns/udp.rs`（新）、`src/testing/dialer.rs` | 4 | UDP 的去处：应答器、被劫持的查询；应答器的队列与每个查询的任务；只问 DNS 的会话早结束；排队的 UDP 的字节预算；拿着客户端不放的假 dialer |
| `crates/rurge-tun/src/{tcp/shard.rs,session.rs}`、`src/dns/tcp.rs`（新） | 5 | 应答器 53 端口的 TCP 在本地应答；SYN 的预匹配 |
| `crates/rurge-tun/src/tests/dns/`（新：`mod.rs`、`answer.rs`、`udp.rs`、`tcp.rs`）、`src/tests/prematch.rs`（新）、`src/tests/support.rs`、`src/tests/bench/`（任务 0 拆出的目录） | 3、4、5 | crate 里的用例；fake 应答延迟的基准 |
| `crates/rurge-engine/src/{engine.rs,prematch.rs（新）,runtime.rs,observe.rs,udp.rs}` | 6 | 钩子、自动升级、记录限频；经监听器来的 fake 地址；SOCKS5 的 UDP 流 |
| `crates/rurge-dns/src/fakeip/mod.rs` | 6 | `in_space` |
| `crates/rurge-inbound/src/udp.rs` | 6 | SOCKS5 的 UDP 回包在 `answers` 是 rurge 地址时以它为来源（SOCKS5 的 `UdpClient` 在这里，今天不看 `_answers`） |
| `crates/rurge-engine/tests/tun_dns.rs`（新）、`tests/common/tun.rs`（新：`tests/tun.rs` 的接线挪过来）、`tests/tun.rs` | 6 | 端到端用例；两个测试文件共用的接线 |
| 兼容性清单、`docs/api/phase3.md`、`docs/acceptance/phase3-manual.md`、CLAUDE.md、README 两份、设计第 18 节、M3b 计划「延后事项」、本计划末尾两张表 | 7 | 文档 |

模块的名字与拆法可以照实际调整，调了在报告里说。

---

## Task 0：拆测试文件与基准、解开模块环（`rurge-tun`，不改行为）

**改哪里**（计划期决定 P25；M3b 延后第 9、15 行）

1. `src/tests/bench.rs` → `src/tests/bench/`：`mod.rs` 留两个基准的测试函数（路径一字不变：`tests::bench::b1_single_stream_throughput`、`tests::bench::b2_ten_thousand_connections`，CLAUDE.md 的 `--exact` 命令照旧能用）与它们共用的读数；B1 的机器、B2 的机器、读进程峰值内存的平台分支各进一个文件（名字照实）。任务 4 的 b3 加进同一个目录。
2. `src/tests/udp.rs`、`fixture.rs`、`lifecycle.rs`、`outcomes.rs`、`icmp.rs` 各拆成同名的目录：`mod.rs` 放这个文件里几处共用的辅助，用例按主题分进子模块（名字照实；例如 `udp/` 分"往返与回包的来源""会话的一生与上限""被拒与 ICMP""分片"）。每个新文件不超过约 800 行。`filter.rs`、`hazards.rs`、`syn.rs` 这一轮不动。
3. `src/context.rs`（新）：`dispatch::Context` 挪过来，字段与构造不变；`dispatch.rs`、`udp/mod.rs`、`udp/client.rs`、`icmp.rs`、`lib.rs` 改从它取。`udp` 与 `icmp` 从此不引用 `dispatch`（模块环解开）。
4. `udp/mod.rs` 的 `datagram(src, dst, payload) -> Option<Vec<u8>>`（造一个 UDP 包）挪进 `packet.rs`，`pub(crate)`；名字照旧或改成 `udp_datagram`，照实。任务 4 的应答器造应答包也用它。
5. `udp/mod.rs` 的 `Session::ending()` 改名 `mark_ending()`，说明写明它只置位、不清目标（`end()` 才清目标、放租约）。
6. 顺带（M3b 延后第 19、21、22 行里落在这些文件上的）：`lifecycle.rs` 里 `send_pattern` 的说明写清楚；`support.rs` 的 `rig_extras` 不再抄一遍 `rig_on` 的接线（一个调另一个）；`stop_ends_a_reply_that_waits_for_room` 的说明不写"一秒"，写 `STOP_GRACE`；`TunHandle::session_tasks()`（`cfg(test)`）开头加 `debug_assert!`：TUN 还没有停止（停止之后读它会 panic 或少算，停止之后读 `live_tasks()`；M3b 延后第 11 行的一条，计划期决定 P28）。
7. CLAUDE.md 里指到被挪动的文件或函数的活的指引（测试约定里"`crates/rurge-tun/src/tests/udp.rs` 的 `counting_icmp()`"一类，常用命令的注释）照改；历史段落不改。

**接口**

- 消费：没有。
- 产出：`crate::context::Context`（任务 3 ～ 5）；`packet.rs` 里造 UDP 包的函数（任务 4）；`src/tests/bench/`（任务 4 的 b3）。

**用例**：只加一条，`session_tasks_is_not_read_after_the_stop`（`#[should_panic]`，期望的信息是上面那个 `debug_assert!` 的）；别的不加，也不改任何断言（这一项只挪代码）。

**判据**：

- 拆之前与之后各存一份 `cargo test -j 8 --workspace --lib -- --list` 的输出：拆之后只多 `session_tasks_is_not_read_after_the_stop` 一行；每行只留最后一段（用例的名字）、排序之后，除这一行外逐行相同；`tests::bench::b1_single_stream_throughput` 与 `tests::bench::b2_ten_thousand_connections` 两行原样在。两份输出与比较的结果写进报告。
- `git grep -n "dispatch::Context" crates/rurge-tun` 什么也没有；`git grep -n "fn ending" crates/rurge-tun/src/udp` 什么也没有。
- 生产代码的改动只有挪动与改名（评审逐处核对）；`session_tasks()` 是只在测试构建里的代码。
- 门禁绿，`rurge-tun` 的用例个数比拆之前多一条。

---

## Task 1：`Resolver::forward`、UDP 上游的 ID 与报文辅助函数（`rurge-dns`）

**改哪里**

1. `src/message.rs`：
   - `Rcode` 加 `FormErr`（1）与 `NotImp`（4）；`from_hickory`、`to_hickory`、`Display`（`FORMERR`、`NOTIMP`）照加。工作区里对 `Rcode` 的穷举 `match`（`git grep -n "Rcode::"`）照补。
   - 只读报文头与问题、不解析整份的四个函数：
     ```rust
     pub struct Header { pub id: u16, pub opcode: u8, pub recursion_desired: bool }
     /// The header's fields, or `None` for fewer than 12 bytes.
     pub fn wire_header(bytes: &[u8]) -> Option<Header>
     /// Writes `id` over the message's ID (a no-op on fewer than 2 bytes).
     pub fn set_wire_id(bytes: &mut [u8], id: u16)
     /// The first question: its name lowercased, without the trailing dot, in
     /// presentation form; its type; its class.
     pub fn wire_question(bytes: &[u8]) -> Option<(String, u16, u16)>
     /// The header's response code.
     pub fn wire_rcode(bytes: &[u8]) -> Option<Rcode>
     ```
   - OPT 的大小：
     ```rust
     /// The UDP payload size the message's OPT record announces, if it has one.
     pub fn opt_size(bytes: &[u8]) -> Option<u16>
     /// Lowers the OPT record's UDP payload size to `max` in place when it is
     /// larger; nothing else in the message changes (no other byte moves).
     pub fn cap_udp_size(bytes: &mut [u8], max: u16)
     ```
   - 应答器读请求、造应答：
     ```rust
     pub const ANSWER_UDP_SIZE: u16 = 1232;
     pub struct Request {
         pub id: u16,
         pub recursion_desired: bool,
         /// Lowercased, without the trailing dot (as `wire_question` gives it).
         pub name: String,
         pub qtype: u16,
         pub qclass: u16,
         /// The OPT record's UDP payload size, if the query has one.
         pub udp_size: Option<u16>,
         // private: the question section exactly as it came, echoed in answers
     }
     pub enum RequestError {
         /// Fewer than 12 bytes, or QR is set: not answered at all.
         NotAQuery,
         /// An opcode other than QUERY (0): NOTIMP, header only.
         NotImplemented(Header),
         /// Does not parse, or not exactly one question: FORMERR, header only.
         Malformed(Header),
     }
     pub fn read_request(bytes: &[u8]) -> Result<Request, RequestError>
     /// An answer built here: QR, AA and RA set, RD copied, the question echoed
     /// byte for byte, `addrs` as A records for an A question and AAAA records
     /// for an AAAA question (an address of the other family is skipped; for any
     /// other type none is written), each with `ttl`; an OPT with payload size
     /// `ANSWER_UDP_SIZE` when the request had one.
     pub fn answer(req: &Request, rcode: Rcode, addrs: &[IpAddr], ttl: u32) -> Vec<u8>
     /// TC set, no records, `rcode`, the question echoed, AA not set, RA set,
     /// the OPT as `answer` writes it.
     pub fn answer_truncated(req: &Request, rcode: Rcode) -> Vec<u8>
     /// Twelve bytes: the ID, QR, the opcode and RD copied, RA set, `rcode`, all
     /// counts zero.
     pub fn answer_header_only(h: &Header, rcode: Rcode) -> Vec<u8>
     ```
     `read_request` 的次序：不足 12 字节或 QR 置位 → `NotAQuery`；opcode 不是 0 → `NotImplemented`（只看报文头，后面解不解析得出都一样）；解析不出或问题数不是 1 → `Malformed`；其余是 `Ok`（class 不在这里看）。
2. `src/upstream/udp.rs`：`UdpUpstream::query` 收下的报文的 ID 已有在途的查询占着时，从一个随机的起点往后找第一个空着的 ID，写进发出的那一份、收到应答后换回调用方的 ID；65,536 个 ID 都在途时 `Err(UpstreamError::Io("too many queries in flight"))`。应答里有 TC、改走 TCP 重问的那一步用调用方原来的报文（`exchange_framed` 自己比对 ID）。
3. `src/fanout.rs`：
   ```rust
   /// Sends `wire` to every upstream at once, again every `opts.resend` to those
   /// that have not answered, `opts.attempts` rounds in all, each send with a
   /// fresh random ID; takes a reply only when QR is set and its question equals
   /// the query's (the name compared without regard to case). The first NOERROR
   /// or NXDOMAIN reply wins; a reply with another code is kept and the first of
   /// them is handed back once every upstream has replied or the last round's
   /// time is up. The reply carries the query's own ID.
   pub(crate) async fn forward_raw(upstreams: &[UpstreamRef], wire: &[u8], opts: &FanoutOpts) -> io::Result<Vec<u8>>
   ```
   没有任何应答：`Err(TimedOut)`。
4. `src/resolver.rs`：
   ```rust
   /// Forwards a client's query, as it is, to the upstreams this resolver would
   /// ask for its name and hands back the reply with the query's ID. Not cached;
   /// AAAA suppression is neither consulted nor counted.
   pub async fn forward(&self, query: &[u8]) -> io::Result<Vec<u8>>
   ```
   - 读不出问题：`Err(InvalidInput)`。
   - 选上游（计划期决定 P4）：`self.hosts.lookup(name)`（`Option<HostLookup>`）的 `action` 是 `HostAction::Servers(specs)` → `host_upstreams_for(&specs)`；是 `System(_)` → `system_upstreams`（空的 → `Err(NotFound)`）；`Ips`、`Alias` 或没有命中 → `primary`（空的 → `Err(NotFound)`）。
   - 发出之前把查询的 OPT 降到 1232（`cap_udp_size`，只降不升）。
   - 经 `forward_raw` 发出，`FanoutOpts` 取 `cfg.fanout`。
   - 一行 DEBUG（字段 `name`、`qtype`、`rcode`、`elapsed_ms`；失败时 `error`）。
5. `src/testing.rs`：
   - `pub fn query_wire(id: u16, name: &str, qtype: u16, udp_size: Option<u16>) -> Vec<u8>`：一个标准查询（RD 置位），名字照写的大小写，`udp_size` 有值时带 OPT。
   - `MockDns` 回答任何类型：认得的名字对 A / AAAA 照旧，对别的类型答 NOERROR、没有记录，`set_txt(name, text)` 设过的答那条 TXT；不认得的名字 NXDOMAIN；`set_rcode` 对一切类型生效；应答里的问题照查询原样回显。`pub fn wires(&self) -> Vec<Vec<u8>>` 给出收到的每一份报文。现有的 `queries()` / `query_count` 只数 A / AAAA 的照旧。

**接口**

- 产出：`message::{Header, wire_header, set_wire_id, wire_question, wire_rcode, opt_size, cap_udp_size, Request, RequestError, read_request, answer, answer_truncated, answer_header_only, ANSWER_UDP_SIZE}`、`Rcode::{FormErr, NotImp}`（任务 3）；`Resolver::forward`（任务 3 经 `TunDns`）；`testing::query_wire`、`MockDns::{set_txt, wires}`（任务 3 ～ 6）。

**用例**

- `message.rs`：`read_request_tells_the_three_refusals_apart`（11 字节、QR 置位 → `NotAQuery`；opcode 2 → `NotImplemented` 带 ID 与 RD；两个问题、截断的问题 → `Malformed`）；`answer_echoes_the_question_byte_for_byte`（名字 `ExAmPle.COM`：应答的问题段与请求的逐字节相同，`Request.name` 是 `example.com`）；`answer_writes_only_the_family_asked`（A 问题带一个 v4 一个 v6 地址 → 一条 A；AAAA 同理；TXT 问题 → 没有记录）；`answer_sets_aa_and_ra_and_copies_rd`；`answer_has_an_opt_only_when_the_request_had_one`（大小 1232）；`answer_truncated_has_tc_and_no_records`；`answer_header_only_is_twelve_bytes`；`cap_udp_size_only_lowers`（4096 → 1232；512 不动；没有 OPT 的报文不动；长度不变）；`wire_question_lowercases_and_drops_the_dot`；`formerr_and_notimp_round_trip`（`Display` 与 hickory 的码）。
- `upstream/udp.rs`：`udp_upstream_gives_each_query_its_own_id`（两个 ID 相同的查询同时经一个 `UdpUpstream` 发给有延迟的 `MockDns`：两个都拿到自己的应答、ID 是各自的；`MockDns::wires()` 里两份的 ID 不同）。
- `fanout.rs`（暂停的时钟、内存里的上游）：`forward_takes_the_first_noerror_or_nxdomain`（一个上游快答 SERVFAIL、另一个慢答 NOERROR → NOERROR）；`forward_returns_another_code_when_nothing_better_comes`（都答 SERVFAIL → SERVFAIL，不等到截止）；`forward_resends_to_a_silent_upstream`（`set_drop_first(1)`）；`forward_times_out_without_any_reply`；`forward_ignores_a_reply_to_another_question`；`forward_accepts_a_reply_whose_question_differs_only_in_case`（评审要点 1）；`each_send_has_a_fresh_id`。
- `resolver.rs`（模块里的用例，`resolver_in_memory` 一类的构造）：`forward_hands_back_the_reply_with_the_clients_id`（TXT）；`forward_asks_the_servers_the_host_section_names`（`[Host]` 的 `*.corp.test = server:<第二个 MockDns>`：只有第二个收到）；`forward_sends_system_names_to_the_system_servers`（`server:system`，`StaticSystemDns` 的服务器是第二个 `MockDns`）；`forward_without_system_servers_fails_for_system_names`；`forward_does_not_cache`（同一个查询两次 → 上游收到两次）；`forward_caps_the_opt_at_1232`（上游收到的 OPT 是 1232）；`forward_rejects_a_query_it_cannot_read`。
- `testing.rs`：`mock_dns_answers_other_types`（TXT 设过的、没设过的、不认得的名字）。

**判据**：用例绿；现有的 `rurge-dns` 用例照旧绿；门禁绿。

---

## Task 2：预匹配的新入口（`rurge-rules`）

**改哪里**

1. `crates/rurge-config/src/session.rs`：
   ```rust
   /// Where a session is pre-matched: a DNS query for its name (no connection
   /// yet: no port, no address), or the first packet of a TCP connection.
   #[derive(Clone, Copy, Debug, PartialEq, Eq)]
   pub enum PreMatchLayer { Dns, Syn }
   ```
2. `src/matcher.rs`：
   - `Verdict` 加 `NeedsConnection`（"要有连接才知道"）。
   - `EvalCtx` 加私有的 `before_connection: bool`（只由第 4 条的入口在 DNS 层置位；`EvalCtx::new` / `with_network` 照旧是 `false`）。
   - 置位时：`DEST-PORT`、`SRC-PORT`、`IN-PORT`、`PROTOCOL`、`USER-AGENT`、`URL-REGEX` 答 `NeedsConnection`；`IP-CIDR`、`IP-CIDR6`、`GEOIP`（含 `UNKNOWN`）、`IP-ASN`（含 `UNKNOWN`）对名字答 `NeedsConnection`（在 `target_ip` 里、`no_resolve` 之前判）；其余照旧（`DOMAIN` 一族、`HOSTNAME-TYPE`、`SRC-IP`、`SUBNET` 照常判定，`PROCESS-NAME` 照旧 `NeedsProcess`）。
   - 组合（不论置不置位都是这样，不置位时没有 `NeedsConnection` 可组合）：`AND`——有 `NoMatch` 就 `NoMatch`，否则有 `NeedsConnection` 就 `NeedsConnection`，否则照旧（要进程优先于要解析，再是 `Match`）；`OR`——有 `Match` 就 `Match`，否则有 `NeedsConnection` 就 `NeedsConnection`，否则照旧；`NOT`——`Match` 与 `NoMatch` 互换，三种 `Needs…` 原样传出；`Set`——深度守卫照旧，集合的结论照第 3 条。
3. `src/set.rs`：`CompiledSet::eval` 置位时：域名索引、逐条求值的行、号码表、前缀树里任何一处 `Match` 就 `Match`；否则有一处答了 `NeedsConnection` 就 `NeedsConnection`（目的是名字时，集合里有 IP 或 IP-ASN 的条目就算号码表 / 前缀树答了它）；否则照旧。
4. `src/engine.rs`：
   ```rust
   /// The first rule written with `pre-matching` that refuses `s` at `layer`,
   /// without resolving anything and without waiting: at `Dns` a condition that
   /// needs the connection (ports, addresses, transport, HTTP) is not known and
   /// a rule that depends on one is not a hit; at `Syn` the session is the
   /// connection. Counts the hit. Does not look at the outbound mode.
   pub fn pre_match(&self, s: &SessionInfo, layer: PreMatchLayer) -> Option<PreMatch>
   ```
   照 `pre_pass` 的做法：`positions()` 上逐条，一份 `NetworkCell` 快照与 `EvalCtx::with_network`，`no_resolve` 为真，每条之前清 `sub_hit`，`pre_matches` 照旧（只认 REJECT 一族的策略），命中的那条 `hits` 加一；运行期的说明（`ctx.notes`）丢掉，同今天的 `pre_match_domain`。去掉 `pre_match_domain`、`pre_match_ip` 与私有的 `pre_match(SessionInfo)`，它们的用例改走新入口（会话照原来的 `SessionInfo::tcp(..)`，层是 `Syn`）。
   `run` 里不会出现 `NeedsConnection`（只有新入口在 DNS 层置位）；它的 `match` 把它当作 `NoMatch` 并 `debug_assert!` 不会走到。
5. `src/pre_matching.rs`：`PreMatch` 加 `pub sub_rule: Option<SubRuleHit>`；`lib.rs` 在 crate 根重导出 `PreMatch`。

**接口**

- 消费：没有。
- 产出：`rurge_config::PreMatchLayer`（任务 3 ～ 6）；`RuleEngine::pre_match`、`rurge_rules::PreMatch { rule, policy, sub_rule }`（任务 6）。

**用例**（`engine.rs` 的测试模块或新的 `tests/pre_match.rs`；DNS 层的会话是 `SessionInfo { src: 198.18.0.1:53000, in_port: 0, listener: Tun, transport: Udp, ..SessionInfo::udp(HostName::Domain(名字), 0) }`）

- `the_dns_layer_needs_the_connection_for_ports_addresses_and_http`：`DEST-PORT`、`SRC-PORT`、`IP-CIDR`、`IP-CIDR6`、`GEOIP`、`GEOIP,UNKNOWN`、`IP-ASN` 带 `pre-matching` 的规则，与经 `RULE-SET` 带进来的 `IN-PORT`、`PROTOCOL`、`USER-AGENT`、`URL-REGEX` 行，在 DNS 层都不命中；同样的规则在 `Syn` 层、端口与地址成立时命中（`USER-AGENT` 与 `URL-REGEX` 在 `Syn` 层照今天的预匹配那一遍不命中）。
- `a_port_condition_keeps_a_rule_from_the_dns_layer`（评审要点 3）：`AND,((DOMAIN,a.test),(DEST-PORT,443)),REJECT,pre-matching` 对 `a.test` 在 DNS 层不命中；`Syn` 层端口 443 命中、80 不命中。
- `not_never_turns_not_knowing_into_a_hit`（评审要点 3）：`NOT,((DEST-PORT,443))`、`NOT,((IP-CIDR,10.0.0.0/8))`、`NOT,((AND,((DOMAIN,a.test),(DEST-PORT,443))))` 带 `pre-matching`，在 DNS 层对任何名字都不命中。
- `an_or_decided_by_its_name_is_a_hit_at_the_dns_layer`：`OR,((DOMAIN,a.test),(DEST-PORT,443))` 对 `a.test` 命中、对 `b.test` 不命中；`AND,((DOMAIN,a.test),(NOT,((DOMAIN,b.test))))` 对 `a.test` 命中。
- `src_ip_and_subnet_decide_at_the_dns_layer`：`SRC-IP,198.18.0.1/32` 命中；`SUBNET` 照网络（`with_network` 的网络）命中与不命中。
- `set_lines_that_need_the_connection_keep_the_set_from_the_dns_layer`：一个 `RULE-SET` 有 `DOMAIN-SUFFIX,b.test` 与 `DEST-PORT,443` 两行 → `b.test` 命中（子规则是 `DOMAIN-SUFFIX,b.test`）、`c.test` 不命中；只有域名行的集合对不在里面的名字不命中；`DOMAIN-SET` 照常。
- `pre_match_counts_a_hit_and_names_the_sub_rule`。
- `the_rule_engine_never_sees_needs_connection`：`evaluate` 走过同一批规则，结论与改动之前相同（选几条有代表性的：端口、IP、`NOT`、集合）。
- 迁过来的 `pre_match_domain` / `pre_match_ip` 用例照旧绿。

**判据**：`git grep -n "pre_match_domain\|pre_match_ip" crates/` 什么也没有；用例绿；门禁绿。

---

## Task 3：应答器的判定（`rurge-tun`）

**改哪里**

1. `src/hooks.rs`（M3b）：
   ```rust
   /// What the responder asks of this generation's resolver.
   pub trait TunDns: Send + Sync {
       /// The addresses `[Host]` (or the system hosts file) maps `name` to.
       fn mapped(&self, name: &str) -> Option<Vec<IpAddr>>;
       /// The reply of the upstreams this generation asks for the query's name,
       /// with the query's ID.
       fn forward<'a>(&'a self, query: &'a [u8]) -> BoxFuture<'a, io::Result<Vec<u8>>>;   // rurge_net::BoxFuture
   }
   impl TunDns for rurge_dns::Resolver { … }   // mapped: host_lookup(name) whose action is HostAction::Ips(addrs) → addrs; forward: Resolver::forward
   ```
   `TunSettings` 加 `pub dns: Arc<dyn TunDns>`、`pub always_real_ip: HostList`、`pub allow_dns_svcb: bool`、`pub hijack_dns: Vec<HijackTarget>`。`Hooks` 加（没有缺省实现）：
   ```rust
   /// Asked with the session of a DNS query for A or AAAA (`Dns`) or of a new TCP
   /// connection (`Syn`): the kind to refuse it with when a pre-matching rule
   /// refuses it, escalation already applied. Synchronous; never waits.
   fn pre_match(&self, layer: PreMatchLayer, session: &SessionInfo) -> Option<RejectKind>;
   ```
2. `crates/rurge-engine/src/runtime.rs` 与 `src/tun.rs`（M3b）：`Runtime::build` 建 `TunSettings` 时填四个新字段（`dns` 与 M3b 的 `resolver` 是同一个 `Arc<Resolver>`，其余取 `config.general`）；`impl Hooks for Engine` 的 `pre_match` 先答 `None`（任务 6 实现）。M3b 的假钩子 `FakeHooks::new(TunSettings)` / `set` 照收新字段，`Extras::default()` 给一个没有映射的 `FakeDns`、空的 `HostList`、`allow_dns_svcb: false`、空的 `hijack_dns`。
3. `src/space.rs`：`pub(crate) const REFUSED_V4: Ipv4Addr = Ipv4Addr::new(198, 18, 0, 244);`（`REJECT-NO-DROP` 的 A 应答；它已经是 `Own`）。
4. `src/dns/mod.rs`（新）：
   ```rust
   pub(crate) const DNS_PORT: u16 = 53;
   pub(crate) const CANARY: &str = "use-application-dns.net";
   pub(crate) const MAX_FORWARDS: usize = 256;
   /// Who asked whom, and over what.
   pub(crate) struct Ask { pub(crate) src: SocketAddr, pub(crate) server: SocketAddr, pub(crate) transport: Transport }
   pub(crate) struct Responder { … }
   impl Responder {
       pub(crate) fn new(hooks: Arc<dyn Hooks>, pool: Option<Arc<FakeIpPool>>, stats: Arc<Stats>) -> Responder;
       /// The reply to `query`, or `None` when nothing is to be sent back.
       pub(crate) async fn answer(&self, query: &[u8], ask: &Ask) -> Option<Vec<u8>>;
   }
   ```
   判定的次序（第一条成立的决定应答；`settings` 在每个查询开始时取一次）：
   1. `read_request`：`NotAQuery` → `None`；`NotImplemented(h)` → `answer_header_only(h, NotImp)`；`Malformed(h)` → `answer_header_only(h, FormErr)`。
   2. class 不是 IN（1）→ `answer(req, NotImp, &[], 0)`。
   3. 名字是 `CANARY`（不分大小写，任何类型）→ `answer(req, NxDomain, &[], 0)`。
   4. 类型是 SVCB（64）或 HTTPS（65）：`allow_dns_svcb` 为假 → `answer(req, NotImp, &[], 0)`；为真 → 转发。
   5. 类型是 A（1）或 AAAA（28）：
      1. `hooks.pre_match(PreMatchLayer::Dns, &session)`（会话见计划期决定 P11）：`Reject` / `TinyGif` → `answer(req, NoError, &[], 0)`；`Drop` → `None`；`NoDrop` → A 答 `[REFUSED_V4]`、TTL 30，AAAA 答空。
      2. A 经 IPv6 来（`ask.server` 是 IPv6）、AAAA 经 IPv4 来 → `answer(req, NoError, &[], 0)`。
      3. `always_real_ip.matches(&HostName::Domain(name), 0) == Some(true)`：`dns.mapped(name)` 有地址 → `answer(req, NoError, &addrs, 30)`；没有 → 转发。
      4. 没有池 → 转发。
      5. `pool.assign(name, now)`：`Ok(i)` → A 答 `v4_of(i)`、AAAA 答 `v6_of(i)`，TTL 30；`Err(NotAName)` → 转发；`Err(Exhausted)` → `answer(req, ServFail, &[], 0)`，这个 TUN 的一生里第一次另记一行 WARN `tun: every fake address is in use; new names get SERVFAIL`。
   6. 其余类型 → 转发。

   转发：`forwards.try_acquire()` 拿不到（已有 256 个在途）→ `answer(req, ServFail, &[], 0)`；拿到 → `dns.forward(query)`：`Ok(reply)` 时，UDP 上 `reply.len()` 超过限度（计划期决定 P8）→ `answer_truncated(req, wire_rcode(&reply))`，否则原样交回；`Err` → `answer(req, ServFail, &[], 0)`。名字、类型、结论只在一行 DEBUG 里。
5. `src/testing/hooks.rs`（M3b 的假 `Hooks`）：可设 `TunSettings` 的新字段；`script_pre_match(layer: PreMatchLayer, dst: HostName, kind: RejectKind)`；`pre_matches_asked() -> Vec<(PreMatchLayer, SessionInfo)>`。
6. `src/testing/dns.rs`（新）：
   ```rust
   pub enum ForwardScript { Echo { pad: usize }, Fail, Hold }
   pub struct FakeDns { … }   // impl TunDns
   impl FakeDns {
       pub fn new() -> Arc<FakeDns>;
       pub fn map(&self, name: &str, addrs: &[IpAddr]);
       pub fn script(&self, name: &str, script: ForwardScript);   // unscripted names: Echo { pad: 0 }
       pub fn forwarded(&self) -> Vec<Vec<u8>>;
       pub fn release(&self);                                     // every Hold answers as Echo { pad: 0 }
   }
   ```
   `Echo { pad }` 的应答是查询本身、QR 与 RA 置位，末尾多 `pad` 个零字节（只为把应答撑大）；`Fail` 是 `Err(Other)`；`Hold` 等到 `release`。

**接口**

- 消费：任务 1 的 `message` 辅助函数与 `Resolver::forward`；任务 2 的 `PreMatchLayer`；M3b 的 `Hooks` / `TunSettings` / `FakeIpPool::{assign, v4_of, v6_of}` / `Stats`。
- 产出：`Responder::new`、`Responder::answer`、`Ask`（任务 4、5）；`TunDns`、`Hooks::pre_match`（任务 6）；`FakeDns`、`ForwardScript`、假 `Hooks` 的预匹配脚本（任务 4、5）。

**用例**（`src/tests/dns/answer.rs`，直接调 `Responder::answer`，查询用 `query_wire` 造，池用 M3b 的 `with_count` / `open_with`）

- `a_reply_or_a_runt_gets_no_answer`；`a_message_that_does_not_parse_gets_formerr`（只有报文头、ID 照旧）；`two_questions_get_formerr`；`another_opcode_gets_notimp`；`another_class_gets_notimp_with_the_question`。
- `the_canary_is_nxdomain_for_every_type`（A、AAAA、TXT；`USE-Application-DNS.net`）。
- `svcb_and_https_are_notimp_unless_allowed`（允许时 `FakeDns::forwarded()` 里有它）。
- `a_and_aaaa_get_fake_addresses`（TTL 30、AA；同一个名字两次同一个地址；A 与 AAAA 是同一个编号的 v4 与 v6）。
- `a_mixed_case_query_gets_its_own_case_back`（评审要点 1：`ExAmPle.TEST` 与 `example.test` 拿同一个地址，应答的问题段与各自的请求逐字节相同）。
- `the_family_must_match_the_transport`（A 经 IPv6、AAAA 经 IPv4 → NOERROR、没有记录；池没有分配）。
- `pre_matching_answers_by_kind`（四种；`pre_matches_asked()` 里的会话：`src` 是应用、`dst_host` 是小写的名字、`dst_port` 0、`listener` `Tun`、`transport` 照查询、`local` 是服务器端点）；`pre_matching_is_asked_only_for_a_and_aaaa`（TXT、SVCB、canary 都不问）。
- `always_real_ip_answers_from_the_host_section_or_forwards`（`map` 过的名字 → 权威应答、TTL 30、只写所问的地址族；没 `map` 的 → 转发；`always_real_ip` 不含的名字照常拿 fake 地址）。
- `a_name_the_pool_does_not_take_is_forwarded`（含 `*` 或空格的名字）；`without_a_pool_a_and_aaaa_are_forwarded`。
- `an_exhausted_pool_gets_servfail_and_one_warn`（`with_count(2)`，两个租约拿着；第三、第四个名字 SERVFAIL；WARN 只有一行、不带名字）。
- `other_types_are_forwarded_and_handed_back_as_they_are`（TXT：交回的字节等于 `FakeDns` 的应答）；`a_failed_forward_is_servfail`。
- `a_large_udp_answer_is_truncated`（`Echo { pad: 2000 }`：没有 OPT → 限度 512、TC；OPT 4096 → 限度 1232；OPT 300 → 限度 512；应答码照上游的；TCP 的 `Ask` 不截）。
- `forwards_in_flight_are_capped_at_256`（评审要点 5：256 个 `Hold`，第 257 个当场 SERVFAIL；`release` 之后 256 个都答了）；`a_fake_answer_does_not_wait_for_forwards`（256 个 `Hold` 在途时，A 照样当场拿到 fake 地址）。
- `an_answer_to_a_query_with_an_opt_has_one_of_1232`。

**判据**：用例绿；M3a / M3b 的用例照旧绿；门禁绿。

---

## Task 4：UDP 的去处——应答器与 `hijack-dns`

**改哪里**

1. `src/lib.rs`：`TunOptions` 加 `dns_queue: usize`（缺省 1024）与 `udp_dns_idle: Duration`（缺省 10 秒）。两个都进 `check`：`dns_queue` 不为 0；`udp_dns_idle` 在 1 毫秒到 24 小时之间、不大于 `udp_idle`。顺带（M3b 延后第 19 行）：`TunOptions` 现有字段的说明补上 `check` 强制的范围，与新字段同一种写法。
2. `src/dispatch.rs` 与 `src/udp/mod.rs`（M3b）：一个 UDP 包在分类（M3a）之后，按这个次序（第一条成立的决定去处）：
   1. 目的是 `Responder` 的 53 端口 → 应答器（应答的来源是那个端点）；
   2. 目的是 `Own` 或 `Responder` 的别的端口 → ICMP 管理上禁止（M3b，不变）；
   3. 目的是 IPv4 的 `Fake` 或 `Real`、端口与 `hijack_dns` 的某一项相同且那一项的地址是 `*` 或等于目的地址、负载 `read_request` 得 `Ok` → 应答器（应答的来源是被劫持的那个端点）；
   4. 其余 → UDP 会话（M3b）。
3. `src/dns/udp.rs`（新）：应答器的队列是一个 `dns_queue` 大小的有界通道，收包的任务 `try_send`，满了丢弃并计 `queue_full`（经 `Stats::drop`）；一个任务从通道取出查询、在 TUN 的任务集里为每个查询起一个任务：`responder.answer(&payload, &ask)` 得 `Some(reply)` 时造 UDP 包（M3b 的构造，来源是 `ask.server`、目的是 `ask.src`）交给写任务。TUN 停止时这些任务随之结束（同 M3a 的规矩）。
4. M3b 的 UDP 会话：会话发过包的目标全都是 53 端口时，空闲 `udp_dns_idle` 就结束（其余照旧 `udp_idle`）。
5. TUN 里排队的 UDP 的字节预算（M3b 延后第 29 行；计划期决定 P27）：`TunOptions` 加 `udp_bytes: usize`（缺省 64 MiB），进 `check`（不为 0）。所有 UDP 会话排着的数据报共用这一份：一个数据报进会话的队列时占它负载的字节数，引擎取走它或它随队列被丢掉时还回（一个 `Drop` 时还回的小守卫跟着数据报走，丢弃的各条路径不必各自记账）；放不下的数据报丢弃并计 `queue_full`，同队列满；会话的第一个数据报放不下时不开会话（不问 `admit_udp`）。一个 `cfg(test)` 的读数给用例看占了多少。
6. M3b 任务 7 留下的两处（M3b 延后第 13 行）：`udp::receive` 在 `parts(packet)`（同一个文件里的私有函数）失败时计 `Malformed`——`classify` 已验过 UDP 的头与校验和，够不到——加一句注释与 `debug_assert!`（release 里照旧丢弃、计 `Malformed`）；`Ids::new`（`src/fragment.rs`）在 `getrandom` 失败时照 `Tun::start` 对各片种子的写法 WARN 一次（不带地址），仍用 `0x5eed`。

**接口**

- 消费：任务 3 的 `Responder`、`Ask`、`DNS_PORT`；M3a 的 `Place` / `place`、`Stats::drop`、写任务；M3b 的 UDP 包构造与会话。
- 产出：`TunOptions::{dns_queue, udp_dns_idle, udp_bytes}`。

**用例**（`src/tests/dns/udp.rs`，经内存设备与应用端夹具的 UDP 包；`FakeHooks` 带 `FakeDns` 与池）

- `a_query_to_the_responder_is_answered_v4` / `_v6`（`198.18.0.2:53` 与 `[fd00:6152::2]:53`；应答的来源是那个端点、目的是应用的源端点、ID 照旧）；`every_responder_address_answers`（`198.18.0.2` ～ `198.18.0.9`）。
- `other_ports_of_the_responder_get_icmp`（`198.18.0.2:54`，M3b 的行为不变）。
- `a_hijacked_query_is_answered_from_the_hijacked_server`（`hijack-dns = 8.8.8.8:53`：发往 `8.8.8.8:53` 的查询得到来源 `8.8.8.8:53` 的 fake 应答，`FakeDialer` 没有收到 UDP 关联）；`hijack_star_takes_every_server`（`*:53`）；`hijack_with_a_port`（`*:5353`：发往 5353 的被劫持、发往 53 的不被）。
- `a_packet_to_a_hijacked_server_that_is_not_a_query_is_a_session`（评审要点 4：QR 置位的、opcode 4 的、乱码的负载 → 交给 UDP 会话，`FakeDialer` 收到关联与那些包）；`ipv6_is_never_hijacked`（`*:53` 之下发往 `[2001:db8::53]:53` 的查询 → 会话）。
- `a_burst_of_queries_is_answered`（一个源端点连发 1000 个不同 ID 的查询 → 1000 个应答、ID 一一对上）。
- `a_full_responder_queue_counts_queue_full`（单元用例：去处函数接一个容量 1、没有人取的通道，第二个查询计 `queue_full`）。
- `tests::options::` 的边界用例：`dns_queue` 为 0、`udp_dns_idle` 为 0 与超过 24 小时、`udp_dns_idle` 大于 `udp_idle` 各报这个字段；等于 `udp_idle` 通过。
- `a_session_that_only_talks_dns_ends_sooner`（`udp_dns_idle` 300 毫秒、`udp_idle` 30 秒：只发往 53 端口的会话 300 毫秒后结束（`FakeDialer` 看到关联的 `closed`），另发过 443 的会话还在）。
- `queued_udp_is_bounded_by_its_byte_budget`（`udp_bytes` 4096、一个不取数据报的关联、1000 字节的数据报：第五个计 `queue_full`；关联取走之后又放得下）；`queued_bytes_come_back_when_a_session_ends`（会话带着排着的数据报结束之后，读数回到 0）；`a_first_datagram_that_does_not_fit_begins_no_session`；`tests::options::` 加 `udp_bytes` 为 0 报这个字段。
- `a_send_waiting_for_room_ends_at_the_stop`（M3b 延后第 13 行：`GatedDevice`（`src/tests/support.rs`）卡住设备、写任务的队列满了，引擎一侧的 `send` 等空位；`stop()` 之后约 1.5 秒内以 `BrokenPipe` 结束）。
- `a_session_over_lets_go_of_its_leases_though_its_client_is_kept`（M3b 延后第 13 行：`with_count(1)` 的池；一个在 `associate` 返回之后仍拿着客户端的假 dialer——`FakeDialer` 加这样一种脚本；会话结束之后 `assign("b.test")` 成功，是 `Session::end()` 里的 `targets.clear()` 放了租约；去掉那一行时变红）。

**基准**：`b3_fake_answer_latency`（忽略的用例，放进任务 0 拆出来的 `src/tests/bench/`：测试函数写在 `bench/mod.rs`、机器进它自己的文件）：十万个不同的名字，每个 A 查询计时 `Responder::answer` 加造应答包（不含设备，设计第 15.3 节），打印中位数、p99 与最大值和 NFR-01 的 0.2 毫秒，不断言；`cargo test -p rurge-tun --release --lib -- --ignored --exact tests::bench::b3_fake_answer_latency --nocapture`。数字记进执行期修正记录。同一个任务里把 B2 用缺省设置再跑一次（M3b 记录第 21 行：M3b 的最终修正波让每条转发中的会话多了一个子令牌，B2 没有重跑；复审估一万条约 2 MB），两个读数与 M3a 的 390.4 MB、429,000,000 字节的线一起记进执行期修正记录。

**判据**：用例绿；门禁绿；b3 与 B2 各跑过一次、数字进了报告。

---

## Task 5：TCP——应答器的 53 端口与 SYN 层的预匹配

**改哪里**

1. `src/lib.rs`：`TunOptions` 加 `dns_tcp_idle: Duration`（缺省 10 秒），进 `check`（1 毫秒到 24 小时）。
2. `src/tcp/shard.rs`（M3a 任务 5 的 SYN 去处）：目的是 `Responder` 的 53 端口的 SYN 照常建套接字（不再 RST）；`Responder` 的别的端口与 `Own` 照旧 RST。
3. `src/session.rs`：建成的连接目的是 `Responder` 的 53 端口时不交给 `Dialer`，交给
   ```rust
   /// Serves DNS over TCP on a connection to the responder: a two-byte length,
   /// then the message, answered in order (never truncated); closes after `idle`
   /// without a new query, when the app closes, or at a message that gets no
   /// answer at all.
   pub(crate) async fn serve_dns(stream: TunStream, responder: Arc<Responder>, ask: Ask, idle: Duration)
   ```
   `ask.transport` 是 `Tcp`；转发的上限（256）与 UDP 共用。长度为 0 的消息与 `NotAQuery` 一样关连接。关的时候先 `shutdown` 再丢（FIN）。
4. `src/tcp/shard.rs`：新的 SYN 依次（设计 M3-D4）：停止 → 上限 → 去处（M3a）→ fake 地址换回名字并拿租约（M3b）→ **预匹配** → 建。预匹配：SYN 层的会话（计划期决定 P11）问 `hooks.pre_match(PreMatchLayer::Syn, &session)`：`None` → 照常建；`Some(Drop)` → 丢掉这个 SYN（一行 DEBUG，不计数）；其余三种 → RST，经 M3a 的 RST 限频（超出时计 `rst_limited`）。被拒的 SYN 不留租约。目的是 `Responder` 的 53 端口的 SYN 不问。

**接口**

- 消费：任务 3 的 `Responder`、`Ask`、`Hooks::pre_match`；M3a 的 SYN 去处与 RST 限频；M3b 的租约。
- 产出：`TunOptions::dns_tcp_idle`；`serve_dns`。

**用例**（`src/tests/dns/tcp.rs` 与新的 `src/tests/prematch.rs`，经应用端夹具的 TCP）

- `a_query_over_tcp_is_answered`（一条连接上两个查询，按次序两个应答，长度前缀对得上）。
- `a_large_answer_is_truncated_and_tcp_gets_it_whole`（评审要点 2：`Echo { pad: 2000 }` 的 TXT 经 UDP 得 TC、没有记录；同一个查询经 TCP 得到整份）。
- `a_dns_connection_idle_is_closed`（`dns_tcp_idle` 300 毫秒：之后应用读到 FIN）；`garbage_on_a_dns_connection_closes_it`（QR 置位的报文、长度 0）。
- `the_responder_on_another_tcp_port_is_reset`（`198.18.0.2:54` → RST）。
- `tcp_to_a_hijacked_server_is_a_connection`（评审要点 4：`hijack-dns = *:53` 时发往 `8.8.8.8:53` 的 TCP → `FakeDialer` 收到 `dial`）。
- `a_syn_refused_by_pre_matching_is_reset`（脚本 `Syn` + 名字 → `Reject`：应用的 `connect` 得 `ConnectionRefused`，`dialed()` 是空的）；`a_syn_dropped_by_pre_matching_gets_nothing`（`Drop`：一段有界的等待里应用一侧什么包也没收到，注释写它等的是 SYN 的第一次重发之前，`dialed()` 是空的）。
- `the_syn_layer_session_is_the_connection`（`pre_matches_asked()`：fake 地址换回的名字、目的端口、`src` 是应用的端点、`local` 是原目的端点、`transport` `Tcp`）。
- `resets_for_pre_matching_are_limited`（101 个被拒的 SYN 在限频的窗口里：100 个 RST，`rst_limited` 是 1）。
- `a_refused_syn_holds_no_lease`（拒绝之后池里那个编号没有租约）。
- `a_syn_the_stack_does_not_take_gives_its_lease_back`（M3b 延后第 12 行：`open` 在 `listen` 失败或协议栈没有收下 SYN 时提前返回，租约随参数 `name` 被丢掉，没有用例钉住；`with_count(1)` 的池，一个发往 fake 地址、过了 `classify` 与上限、smoltcp 0.12 的监听套接字却不收的 SYN——读 `smoltcp-0.12.0/src/socket/tcp.rs` 的 `State::Listen` 一支去找；从应用一侧造不出时，照 `PANIC_MARKER` 的先例加一个只在测试构建里的触发点——，之后 `assign("b.test")` 成功）。
- `tests::options::` 的边界用例：`dns_tcp_idle` 为 0 与超过 24 小时各报这个字段。

顺带（M3b 延后第 20 行）：改到 `session::serve` 时，去掉它的 `stopped` 标志。

**判据**：用例绿；M3a / M3b 的用例照旧绿（点了名的 `Responder:53` 不再 RST 的那一条照改并在提交说明里说）；门禁绿。

---

## Task 6：引擎——预匹配的钩子与经监听器来的 fake 地址

**改哪里**

1. `src/prematch.rs`（新）与 `src/tun.rs`：`impl Hooks for Engine` 的 `pre_match`：
   - `self.mode()` 不是 `Mode::Rule` → `None`（出站模式是活的，不随代）。
   - `let rt = self.runtime(); let hit = rt.rules.pre_match(session, layer)?;` 策略 `PolicyRef::Builtin(b)` 经 `RejectKind::from_builtin(b)`（`crates/rurge-proto/src/outbound.rs`，与 `escalates()` 同在那里、都是 `pub`）得到种类（别的 → `None`，`RuleEngine::pre_matches`（`crates/rurge-rules/src/engine.rs`）已保证不会）。
   - 种类 `escalates()` 时照 `connect_session` 的做法：`escalation.record(&主机, now)`，`should_drop` 为真就改答 `Drop`；主机是 `session.dst_host.to_string()`（DNS 层是名字，SYN 层是名字或地址）。`Escalation` 与它的两个方法改为 `pub(crate)`。
   - 记录（设计第 12.4 节）：同一代、同一条规则（`hit.rule`）5 分钟只记一次——`Runtime` 加一张 `Mutex<HashMap<usize, Instant>>`，判断写成带 `now` 参数的纯函数 `due(&self, rule: usize, now: Instant) -> bool`（不在表里或已过 5 分钟 → 记下 `now` 并答真）；该记时：`look_up_owner(session)`、`new_handle_with(session.clone(), process)`、`set_rule(rule_raw(&rt.rules, hit.rule))`、策略链是那一个内置策略、`add_note("pre-matching at DNS")` 或 `"pre-matching at SYN"`、有子规则时同 `choose_policy` 的写法再加一条 `Sub-rule matched: … (in …)` 并记 DEBUG，最后 `finish(SessionOutcome::Rejected(effective))`。不该记时什么也不建（自动升级照样计）。
   - 返回 `Some(effective)`。
2. `src/observe.rs`：记录的 `dst` 与会话日志行的 `dst` 在端口是 0 时只写主机（只有 DNS 层的记录是 0），一个共用的函数。
3. `crates/rurge-dns/src/fakeip/mod.rs`：`pub fn in_space(ip: IpAddr) -> bool`（先 `to_canonical`；`198.18.0.0/15` 或 `fd00:6152::/64`）。
4. `src/engine.rs`：`Engine` 加 `fake_pool`（`ArcSwapOption<FakeIpPool>`）：`start_tun` 打开池之后放进去，TUN 结束时清掉（M3b 的收尾任务里）。`Engine::dial`：会话的监听器是 HTTP 或 SOCKS5、`dst_host` 是 `in_space` 的地址、`fake_pool` 有池时，在 `look_up_owner` 之前 `pool.lease(ip)`：有 → `dst_host` 换成 `HostName::Domain(名字)`，租约挂在 `handle.on_finish` 上随会话结束释放；没有 → 照常建 handle，再 `fail(handle, FailKind::Other, "fake address not known")`（HTTP 监听器答 502）。没有池时照普通地址处理。
5. `src/udp.rs`：`run_flow` 里 SOCKS5 关联的流做同样的换（换不了的流以 `fake address not known` 失败），租约随流；`Flow` 记下换之前客户端发往的目标，`answers` 给它。`crates/rurge-inbound/src/udp.rs`（SOCKS5 的 `UdpClient`，今天不看 `_answers`）：UDP 回包在 `answers` 是 `in_space` 的地址时以它为来源（其余照 M3b）。
6. `crates/rurge-engine/tests/common/tun.rs`（新）：`tests/tun.rs` 的 `guarded`、`profile` 与收包、等结局的几个函数挪过来（照 `tests/common/` 现有子模块的写法），`tun.rs` 与 `tun_dns.rs` 共用；`tun.rs` 的用例个数不变、照旧绿（计划期决定 P26）。

**接口**

- 消费：任务 2 的 `RuleEngine::pre_match` / `PreMatch`；任务 3 的 `Hooks::pre_match`；M3b 的 `Engine::start_tun`、`FakeIpPool::lease`、`query_for` 与 `answers`。
- 产出：`fakeip::in_space`；`FAKE_ADDRESS_NOT_KNOWN`（`"fake address not known"`，引擎里一个常量，用例引用它）。

**用例**（`crates/rurge-engine/tests/tun_dns.rs`，接线取自 `tests/common/tun.rs`；经 `Engine::start_tun` 接内存设备；名字由 harness 的 `MockDns` 钉到 `127.0.0.1`；两道防线见计划期决定 P24）

- `a_name_asked_through_the_responder_connects_to_its_upstream`（应用经 `198.18.0.2:53` 问 `target.test` 的 A → fake 地址 → TCP 连过去 → 引擎拨 `target.test` → 回环上游的回显；记录的 `dst` 是 `target.test:<端口>`）。
- `pre_matching_at_the_dns_layer_answers_empty_and_records_once`（`DOMAIN-SUFFIX,ads.test,REJECT,pre-matching`：问两次都是空应答；请求记录里只有一条，`notes` 含 `pre-matching at DNS`，`dst` 是 `x.ads.test`，没有 `:0`）。
- `pre_matching_with_reject_drop_answers_nothing`；`pre_matching_with_reject_no_drop_answers_244`。
- `pre_matching_escalates_to_silence`（同一个名字连问 `ESCALATE_COUNT` + 1 次：最后一次没有应答）。
- `pre_matching_at_the_syn_layer_resets_only_the_port_it_names`（评审要点 3：`AND,((DOMAIN,x.test),(DEST-PORT,<端口 A>)),REJECT,pre-matching`：DNS 层答 fake 地址；连 fake 地址的端口 A → RST，记录 `notes` 含 `pre-matching at SYN`；端口 B → 连上回环上游）。
- `pre_matching_follows_the_live_outbound_mode`（`set_mode(Mode::Direct)` 之后同一个名字得 fake 地址；回到 `Rule` 之后又是空应答）。
- `a_new_generation_records_again`（重载之后 5 分钟之内同一条规则再记一条）；`due` 的单元用例（`prematch.rs`）：同一条规则 5 分钟内只答一次真、过了再答真、不同的规则各算各的。
- `a_listener_session_to_a_fake_address_goes_to_its_name`（池里为 `target.test` 取地址；经 HTTP 监听器 `CONNECT <fake>:<端口>` → 引擎拨 `target.test`；记录的 `dst` 是名字）。
- `a_listener_session_to_an_unknown_fake_address_fails`（`CONNECT 198.18.200.1:<端口>` → 502，记录的 `error` 是 `fake address not known`，没有拨号）。
- `without_a_tun_a_fake_address_is_an_address`（不起 TUN：同样的 `CONNECT` 照规则走，被第二道防线拒绝，不是 `fake address not known`）。
- `a_socks5_udp_flow_to_a_fake_address_goes_to_its_name_and_answers_from_it`（SOCKS5 UDP 发往 `target.test` 的 fake 地址 → 引擎的流发往回环上的 UDP 回显；客户端收到的回包来源是那个 fake 地址）。
- `crates/rurge-dns`：`in_space_is_rurges_space`（两个前缀的边界、IPv4 映射的 IPv6 地址）。

**判据**：用例绿；`pipeline.rs` 的 `repeated_rejects_escalate_to_drop` 照旧绿；门禁绿。

---

## Task 7：文档（M3 收尾）

- 兼容性清单：4.1 节 L333 ～ L336（L336 的阶段 4 → 3）、6.4 节 L543 ～ L551、9.1 节 L709 ～ L710、`[General]` 的 `allow-dns-svcb`、`hijack-dns`、`always-real-ip`、`icmp-forwarding` 的"目前"备注（虚拟网卡的内核在内存设备上做完、测完，真实设备随 M4）；登记与手册不同或手册没写的读法：`REJECT-NO-DROP` 的 AAAA 空应答、`syslib` 名字的其余类型、转发不跟随 `[Host]` 的别名（P4）、DNS 层"要连接才知道"的三值读法（P2）、P2P 的每个会话 1024 个目标（M3b 延后第 3 行）、同一个端口上两个名字的回包来源（M3b 延后第 2 行）、握手之后 `REJECT-TINYGIF` 回 RST、`REJECT-DROP` 拿 30 秒再 FIN；附录重数（行数照实）。
- `docs/api/phase3.md` 的 M3 一节：虚拟网卡的会话在请求记录里的样子（`listener`、`listenPort` 是 `null`、`pid` / `processPath`）、`notes` 的两句、DNS 层的记录没有端口、`fake address not known`、计数的形状与它们 M4 起在哪里出现、应答器的日志行（只在 DEBUG 的与 WARN 的两行）。
- `docs/acceptance/phase3-manual.md` 的 M3 一节：内存基准与真实设备上数字的差别要在 M4 量（B1、B2、b3）；真实的应用（浏览器、`dig`、`nslookup`、带 0x20 的解析器）经真实设备问应答器的几项留给 M4。
- CLAUDE.md：状态一段加 M3（做了什么、门槛的数字与缺省值、没做的与不要当成已经有的）；「先读这些文档」加 M3c 计划；依赖方向照实；常用命令加三个基准与几个过滤词。README 两份的特性表与路线图。
- 设计第 18 节：照执行期修正记录补"实施期的订正"（P2 的三值读法取代"跳过含端口的规则"等）。
- M3b 计划「延后事项」第 2、3、5、7、9、12、13、15、29 行与第 11 行的一条，与本计划顺带做掉的第 19 ～ 22 行里的几处，标上去向（本计划的哪个提交）。
- 本计划末尾两张表照实填。

**判据**：`git grep` 核对文档里引用的用例名与路径都在；禁用词检查 `hits 0`；门禁绿。

---

## 计划期决定

| # | 事项 | 决定 |
| - | ---- | ---- |
| P1 | 计划引用的 M3a / M3b 的名字 | 写定时对着合并之后的 `main` 核对过：`Place` / `place`、`Stats::drop`、RST 限频、`Hooks` / `TunSettings`（今天只有 `resolver` 与 `icmp_forwarding`）、`FakeHooks::{new, set}` 与测试里的 `Extras`、`FakeIpPool` 的方法与 `NotAName` / `Exhausted`、`query_for`、`answers`、`Engine::start_tun`、`guarded`、解析器的 `primary` 与 `system_upstreams` 两个字段、`HostMap::lookup`、`HostList::matches`、`HijackTarget { addr, port }`；执行中名字不同的照实改进简报，不改语义 |
| P2 | DNS 层的"没有端口"怎样读（设计第 12.1 节、M3-D20） | 设计写的是"编译时记下含端口的规则、DNS 层跳过它们"。计划改成三值：要连接才知道的条件答 `NeedsConnection`，逻辑规则按"能定就定"组合（`AND` 有一个不成立就不成立，`OR` 有一个成立就成立，`NOT` 不翻转"不知道"）。理由：集合的内容运行中会换（`SetHandle::store`），编译时的标记要跟着每个集合的每一行走；三值在集合里自然成立；`NOT,((DEST-PORT,443))` 一类的写法照设计要的"不当作不命中"。与"跳过"相比只多一种命中：`OR` 的另一支不靠连接就成立时（如 `OR,((DOMAIN,a.test),(DEST-PORT,443))` 对 `a.test`），这种情况它本来就命中，任何端口都一样。IP 类规则在 DNS 层也走它（目的永远是名字），`PROTOCOL`、`USER-AGENT`、`URL-REGEX` 同样（查询自己的传输不是连接的） |
| P3 | `PreMatchLayer` 放在哪里 | `rurge_config::session`：`rurge-tun` 不直接依赖 `rurge-rules` |
| P4 | 转发选哪些上游 | 照 `[Host]` 的写法：`server:` → 那些服务器；`server:system` / `syslib` / `force-syslib` → 系统当前的 DNS 服务器（设计第 11.3 节；三种写法同一件事，同阶段 1）；其余（地址映射、别名、没有）→ 主上游。别名不跟随（转发的是原来的问题，问主上游）；`proxy_hostnames` 的豁免与 `localhost` 的规则不适用（它们管的是 rurge 自己的查找）；`.local` 与单标签名字照主上游（不交系统解析库：解析库只答地址，不能原样转发）。登记 |
| P5 | 转发的扇出与收下哪个应答 | 同时问所选的每个上游，每 `resend` 对还没答的重发，至多 `attempts` 轮（缺省 1 秒 × 5），每次发出换新的随机 ID；只收 QR 置位、问题与查询相同（名字不分大小写）的应答；第一个 NOERROR 或 NXDOMAIN 胜出，其余的码留着、都答了或到截止时交回第一个；不进缓存、不计 AAAA 抑制 |
| P6 | UDP 上游的 ID | 上游自己给每个在途的查询一个不冲突的 ID（今天两个同 ID 的查询同时在途时第一个失败）；顺带修掉 `fanout` 里潜在的同一个碰撞 |
| P7 | 应答器在设计之外定的细节 | opcode 先看（只看报文头）；class 不是 IN 答 NOTIMP 带问题；canary 对任何类型；本地造的应答置 AA 与 RA、照抄 RD；`REJECT-NO-DROP` 的 A 记录 TTL 30；`always-real-ip` 按 `matches(&Domain(名字), 0)` 判，写了端口的条目在 DNS 层永不命中（没有端口）；系统 hosts 文件里的映射同 `[Host]` 的地址映射（`host_lookup` 就是这样）；名字不合池的样子（M3b 的 `NotAName`）照转发 |
| P8 | UDP 的限度与截断 | 有 OPT：min(max(它的值, 512), 1232)；没有：512。超出时 `answer_truncated`，应答码照上游的；TCP 从不截断。本地造的应答（最大是 `always-real-ip` 的一串地址）同样受限度：超出时置 TC |
| P9 | 转发的上限 | 256 个许可，`try_acquire`；拿不到当场 SERVFAIL，不排队（M3-D19） |
| P10 | 池耗尽的 WARN | 每个 TUN 的一生一次，不带名字 |
| P11 | 两层的会话 | DNS 层：`src` 是应用的源端点，`in_port` 0，`listener` `Tun`，`dst_host` 是小写的名字，`dst_port` 0，`transport` 照查询，`local` 是服务器端点（应答器或被劫持的那个）。SYN 层：`src` 是应用的源端点，`dst_host` 是 fake 地址换回的名字或目的 IP，`dst_port` 是目的端口，`transport` `Tcp`，`local` 是原目的端点 |
| P12 | `hijack-dns` 认什么 | 只对 UDP、只对 IPv4、目的不是 rurge 自己的地址；"合法的查询"是 `read_request` 得 `Ok`（class 不看：class 不对的照样由应答器答 NOTIMP）；别的照常作会话 |
| P13 | 应答器的队列与任务 | 一个 1024 的有界通道（满了计 `queue_full`），每个查询一个短任务（fake 应答当场答完，转发等它自己的结果），任务在 TUN 的任务集里 |
| P14 | 只问 DNS 的会话 | 会话发过包的目标全是 53 端口时空闲 10 秒结束（M3b 延后第 7 行；引擎对 DNS 的流也是 10 秒） |
| P15 | DNS over TCP | 在本地应答、按次序一个接一个（流水线发来的查询照收、照次序答）、空闲 10 秒关、收到没有应答的消息关；转发的上限与 UDP 共用 |
| P16 | SYN 层的预匹配在哪里调 | 在片的锁里、建套接字之前（M3-D4 的次序）；钩子同步、不碰 TUN；`Drop` 不回包、不计数、一行 DEBUG；其余 RST 经 M3a 的限频；被拒的 SYN 不留租约；应答器的 53 端口不问 |
| P17 | 钩子与记录 | 出站模式取活的 `Engine::mode()`；自动升级的键是 `dst_host` 的文字（同 `connect_session`）；记录的限频表在 `Runtime` 里（换代即从头记），时间是 `std::time::Instant`，判断是带 `now` 的纯函数；不该记时不建 handle（也不查进程），自动升级照样计 |
| P18 | 会话的日志行（设计第 17 节 V12） | 照 M3b P13：引擎今天对 `Rejected` 的会话写 INFO 并带 `dst` 与 `src`，预匹配的记录也经它，所以 DNS 层的名字出现在 INFO 里——但每条规则 5 分钟至多一行。设计第 14 节要的"名字只在 DEBUG"与它不一致，照 V12 以今天的会话日志为准，登记进延后事项（项目所有者） |
| P19 | 经监听器来的 fake 地址 | 只对 HTTP 与 SOCKS5 的会话，在 `look_up_owner` 之前换；租约挂在会话的结束钩子上；池里没有 → `fake address not known`（HTTP 502）、不拨；没有池时照普通地址（M3-D21） |
| P20 | SOCKS5 的 UDP 流 | 同样在 `run_flow` 里换；`Flow` 记下客户端当初发往的目标作 `answers`；SOCKS5 的回包在 `answers` 是 rurge 地址时以它为来源（客户端认得的是它） |
| P21 | 记录里没有端口的目的地 | 端口 0 时只写主机，不写 `:0`（只有 DNS 层的记录是 0） |
| P22 | b3 量什么 | `Responder::answer` 加造应答包，不含设备与写任务（设计第 15.3 节"在应答器的入口与写回之间量，不含设备"）；不作门禁 |
| P23 | `Rcode` 的两个新值 | `FormErr`、`NotImp` 是应答器自己要造的码，加进枚举比到处写 `Other(1)` / `Other(4)` 清楚 |
| P24 | 端到端用例的两道防线（设计第 15.2 节） | TUN 一侧经应答器或池问到的名字由 `MockDns` 钉到回环；监听器一侧要测换名字的用例，规则先放过那个名字（`DOMAIN,target.test,DIRECT`），紧跟 `IP-CIDR,198.18.0.0/15,REJECT,no-resolve` 与 `DEST-PORT,<上游端口>,REJECT` 两条——换名字失灵时会话被拒，到不了 harness 追加的 `FINAL,DIRECT`；TUN 一侧的规则照 M3b 的 `guarded()`（`SRC-IP,198.18.0.1/32,REJECT` 与 `SRC-IP,fd00:6152::1/128,REJECT`，在 harness 追加的 `FINAL,DIRECT` 之前）收尾 |
| P25 | 任务 0 怎样拆 | 只挪代码、不改行为：每个用例的路径前缀不变（`tests::udp::x` 可以变成 `tests::udp::<子模块>::x`，CLAUDE.md 与各计划里的过滤词照旧选得中），`--exact` 用到的两个基准的路径一字不变（所以基准的测试函数留在 `bench/mod.rs`、机器挪进子模块）；拆之前与之后用例的个数相同、名字一一对得上；每个新文件不超过约 800 行；`Context` 挪进新的 `src/context.rs`；造 UDP 包的函数挪进 `packet.rs`；`Session::ending()` 改名 `mark_ending()`。理由：后面三个任务往 `rurge-tun` 的测试里加的用例不少，1500 行以上的文件实现者与评审者都读不全（M3b 延后第 15 行）；先拆再加，评审只看到挪动 |
| P26 | 引擎的 TUN 用例共用接线 | `tests/tun.rs` 的 `guarded`、`profile` 与收包、等结局的几个函数挪进 `tests/common/tun.rs`，`tun.rs` 与 `tun_dns.rs` 共用；`tun.rs` 的用例照旧绿、个数不变 |
| P27 | TUN 里排队的 UDP 的字节预算（M3b 延后第 29 行，整分支终审） | 缺省 64 MiB，所有会话共用；每个数据报占它负载的字节，引擎取走或它被丢掉时还（守卫跟着数据报走）；放不下计 `queue_full`；第一个数据报放不下时不开会话。理由：每个会话至多 256 个数据报、会话至多 4096 个（引擎的关联上限），MTU 1500 时最坏约 1.5 GB、9000 时约 9.4 GB；64 MiB 与 TCP 的缓冲预算同一个数，够约 45,000 个满长的数据报。引擎一侧每个关联的 1 MiB 不在这里（阶段 3 / M6 的验收） |
| P28 | M3b 整分支终审留给本计划的小项 | 分到动那一处代码的任务：`session_tasks()` 的 `debug_assert!` 进任务 0（只动测试的支撑）；阻塞在写任务队列上的 `send` 被停止结束、`Session::end()` 的 `targets.clear()`、够不到的 `Malformed` 与 `Ids::new` 的 WARN 进任务 4（UDP 的路径）；SYN 在拿了租约之后没被协议栈收下时还租约的用例进任务 5（SYN 的路径） |

## 执行期修正记录

下面写"R N"的是执行期的第 N 号裁定。每一条都是主持这一轮的会话代项目所有者作的裁定，所有者可以改（R18 没有用过）；写"M3b 延后第 N 行"指 M3b 计划末尾「延后事项」的行。计划的正文没有改：正文里说得不对的地方，这张表的各行覆盖它（P2 的一句在第 9 行，任务 3 的用例都在 `answer.rs` 一句与 `Responder::new` 的参数在第 12 行，SOCKS5 的回包来源在 `rurge-inbound` 里改在第 21 行，任务 5 的"先 `shutdown` 再丢"在第 19 行）。

| # | 任务 | 修正 | 提交 |
| - | ---- | ---- | ---- |
| 1 | 0 | 只挪代码、不改行为：`src/tests/{bench,udp,fixture,lifecycle,outcomes,icmp}.rs` 六个文件拆成同名的目录（33 个新文件，每个不到 540 行，最大的 `udp/sessions.rs` 536 行；用例的路径前缀不变，两个基准的路径 `tests::bench::b1_single_stream_throughput` 与 `tests::bench::b2_ten_thousand_connections` 一字不变），`Context` 挪进新的 `src/context.rs`（`udp` 与 `icmp` 从此不引用 `dispatch`，模块环解开），造 UDP 包的函数从 `udp/mod.rs` 挪进 `packet.rs` 改名 `udp_datagram`，`Session::ending()` 改名 `mark_ending()`，`TunHandle::session_tasks()`（`cfg(test)`）开头加 `debug_assert!`（停止之后读它在调试构建里 panic），并加用例 `session_tasks_is_not_read_after_the_stop`（`#[should_panic]`，带 `#[cfg(debug_assertions)]`：发行构建里不编译、不列出）。拆之前与之后各存一份 `--list`，逐行比较：只多这一条。**R1**（主持这一轮的会话代项目所有者作的裁定，所有者可以改）：加了 `debug_assert!` 之后若有现有用例在停止之后读 `session_tasks()` 而变红，把那一处换成 `live_tasks()`，在报告与提交说明里点名。**没有用到**：没有哪条现有用例在停止之后读它。代价（若用到）：本来要等会话的任务数的用例，换了计数会等得不准，评审逐处核对 | `2aee55e` |
| 2 | 0 | 评审接受的出入：去掉 15 处 `// ----` 段落标记（一节成了一个文件时，说法进了新文件的 `//!`；一个文件里几节之一的标记留着）；`counting_icmp()` 随唯一用它的子模块进 `udp/refused.rs`、私有（CLAUDE.md 里指到它的一处改了；任务 4 没有新用例要数 ICMP，没有再挪）；`tests/bench/peak.rs` 里读峰值内存的 Linux 与其余平台两段从没有编译过（Windows 的一段在 B2 里跑过）。顺带做掉 M3b 延后第 19、21、22 行里落在这些文件上的几处：`send_pattern` 与 `stop_ends_a_reply_that_waits_for_room` 的说明，`rig_extras` 与 `rig_on` 共用一个私有的 `rig_on_extras`（接线只写一遍） | `2aee55e` |
| 3 | 0 | 任务 0 的评审（一位新的评审者，只读，逐处核对"只挪不改"）：规格符合，质量 Approved，没有 Critical 与 Important。次要项的去向：`tests/support.rs` 里 `rig_on` 与 `tun_on` 的说明过时、`tests/outcomes/stop.rs` 里新用例说明的最后一句有歧义——最终修正波；`counting_icmp()` 私有——见第 2 行；CLAUDE.md 讲 `session_tasks()` 的一句要提那个断言——任务 7（第 27 行）；`context` 与 `udp` 在类型上互相引用——只是告知。门禁：80 个结果行、4126 个通过、0 个失败、12 个忽略 | 最终修正波 |
| 4 | 1 | `rurge-dns` 的转发与应答器要的报文辅助：`Resolver::forward` 与 `fanout::forward_raw`（P4、P5：同时问所选的每个上游，每 `resend` 对还没答的重发，每次发出换新的随机 ID，只收 QR 置位、问题与查询相同的应答，第一个 NOERROR 或 NXDOMAIN 胜出，不进缓存、不计 AAAA 抑制），`message` 的只读报文头与问题的函数（`wire_header`、`wire_question`、`wire_rcode`、`set_wire_id`、`opt_size`、`cap_udp_size`）与应答器读请求、造应答的 `read_request`、`answer`、`answer_truncated`、`answer_header_only`，`Rcode::{FormErr, NotImp}`（P23），UDP 上游自己给每个在途的查询一个不冲突的 ID（P6：`Pending` 加归还 ID 的守卫 `Claim`），`MockDns` 回答任何类型（`set_txt`、`wires`）与 `query_wire`；37 个新用例。计划没有说、评审接受的读法：一、换 ID 只在"已有在途的查询占着"时（计划的任务 1 第 2 条）：调用方的 ID 空着就留着，撞了才从随机起点往后找第一个空着的，这样不撞车的查询在线上的字节不变；二、问题段里的压缩指针与超过 255 字节的名字读作 `Malformed`；三、"解析得出"指报文头、那一个问题与每条声明的记录都能走到头（与 `lower_ttls` 同一种走法），不是 hickory 的整份解析；四、`Rcode::to_hickory` 与 `wire_is_response` 是 `pub(crate)`；五、旧的 UDP 上游里，被丢掉的 `query` future 会让它的 ID 一直占着（扇出有了答案之后 `abort_all` 还在等的查询）：旧代码上用例 `a_query_that_is_dropped_gives_its_id_back` 红，守卫在每条出路上还 ID；六、`forward_raw` 对读不了的查询立刻答 `InvalidInput`、上游名单为空立刻答 `NotFound`，只有错误、没有应答时仍到截止时间才 `TimedOut` | `7aaba4c` |
| 5 | 1 | 应答码的文字变了：`FORMERR` 与 `NOTIMP` 在 `rurge dns lookup --trace` 一类的输出里原来写 `RCODE1` 与 `RCODE4`（`Rcode` 的 `Display`），现在写它们的名字；没有用例依赖旧文字。兼容性清单 10.3 节 `rurge dns lookup` 一行只写"`--trace` 打印每次尝试"，没有描述应答码的文字，所以这一处只记在这里，不进清单 | `7aaba4c` |
| 6 | 1 | **R7**（主持这一轮的会话代项目所有者作的裁定，所有者可以改）：`MockDns::wires()` 记下它收到的每一份报文，不论经回环上的 UDP / TCP 还是经内存里的上游（`MockDns::upstream()`）——`fanout.rs` 的用例走内存里的上游，`each_send_has_a_fresh_id` 要读它；计划只写"给出收到的每一份报文"。代价：没有 | `7aaba4c` |
| 7 | 1 | 任务 1 的评审：规格符合，质量 Approved，没有 Critical 与 Important。次要项：`Pending::claim` 返回裸 ID（调用方下一行才建 `Claim`）、`fanout.rs` 开头的模块说明只写了 `resolve_name`——最终修正波；`record_types` 与 `ttl_offsets` 各走一遍记录——不改，延后第 9 行；出错的上游不算"答过"——**R20**（主持这一轮的会话代项目所有者作的裁定，所有者可以改）：`forward_raw` 对出错的上游照旧每轮重发、不算"答过"、不提前结束，这是接口说明（"都答了或到截止时交回第一个"）的读法，也与 `resolve_name` 的结束条件一致（出错可能是一时的）。代价：上游全挂（或全部立刻出错）时应用要等至多 5 秒（`resend` 1 秒 × `attempts` 5）才拿到 SERVFAIL（应答器对 `Err(TimedOut)` 本来就答 SERVFAIL），期间每个查询占一个转发名额，名额满了之后的查询立刻得 SERVFAIL，fake 地址的回答不等转发；一个死掉的上游旁边只答 SERVFAIL 的那个也要等满。延后第 10 行；`Request` 派生的 `Debug` 打印被问的名字——**R21**：留着，名字不是凭据，规矩是名字不进 INFO 及以上，DEBUG 行本来就带名字。代价：以后有人在 INFO 里 `{:?}` 它就漏出名字，延后第 11 行。门禁：80 个结果行、4163 个通过、0 个失败、12 个忽略（第二遍里 `rurge-dns` 的 `tests/resolver.rs` 以 `STATUS_HEAP_CORRUPTION` 退出一次，单独重跑 8 次都过）；26 个用到套接字、时间或暂停时钟的新用例各单独跑 25 次、合起来跑 20 次，0 失败 | 最终修正波 |
| 8 | 2 | **R2**（主持这一轮的会话代项目所有者作的裁定，所有者可以改）：`PreMatchLayer` 加进 `rurge-config/src/lib.rs` 的 `pub use session::{…}`（计划期决定 P3 只说它在 `rurge_config::session`）。代价：没有 | `221a5d9` |
| 9 | 2 | **计划期决定 P2 的一句说得不全（R17，主持这一轮的会话代项目所有者作的裁定，所有者可以改）**：P2 说"与'跳过'相比只多一种命中"（`OR` 的另一支不靠连接就成立），漏了另一种：`NOT` 套在"名字使之不成立"的 `AND` 上——`NOT,((AND,((DOMAIN,a.test),(DEST-PORT,443))))` 对 `b.test`：那个 `AND` 不论端口都不成立，`NOT` 成立，是命中。三值的结论确定时对每个端口都成立（在 SYN 层同一条规则对 `b.test` 的每个端口都命中，所以这不是误拒，满足设计第 12.1 节要的"不把不知道翻成拒绝一切"）。计划任务 2 的用例 `not_never_turns_not_knowing_into_a_hit` 说这一条对任何名字都不命中，与它自己的组合规则不一致：照 P2 的三值，那一条断言只对 `a.test`，另加两条用例：`not_over_an_and_the_name_makes_false_is_a_hit`（`AND` 的两种次序，第二种守住循环不在第一个 `NeedsConnection` 处停）与 `not_over_a_set_is_a_hit_only_when_the_set_decides`（只有域名行的集合对不在里面的名字答 `NoMatch`，`NOT` 之下命中；带 IP 行或端口行的集合答 `NeedsConnection`，不命中）。代价：若项目所有者要设计原来的"跳过"，DNS 层少拒绝这一类名字，它们在 SYN 层照样被拒 | `221a5d9` |
| 10 | 2 | 任务 2 的其余：为让用例能红，加了对端口 0 也成立的端口表达式的用例（`DEST-PORT,0-1023`、`<1024`、`0-65535`，`SRC-PORT,53000-53999`，`IN-PORT,0`，`PROTOCOL,UDP`）——查询自己的值（端口 0、UDP、应用的源端口）使这些条件"成立"，今天的行为下会命中，三值读法下是 `NeedsConnection`——，所以 11 个新用例里 9 个对今天的行为红；`run` 里不会出现 `NeedsConnection`（`debug_assert_ne!`）；`pre_match_domain` / `pre_match_ip` 的 4 条用例迁到新入口（会话照原来的 `SessionInfo::tcp(..)`，层是 `Syn`，断言不改），`PreMatch` 多 `sub_rule`。评审（规格符合，质量 Approved，没有 Critical 与 Important）的次要项：`still_needs(connection, process, resolve)` 三个相邻的布尔容易写反——不改（变异盖住了几种对调；代价：以后改它的人要小心）；新入口照收到的会话用、不规范化名字——任务 3 在问之前把名字小写（P11）；文档里提着去掉的两个函数——任务 7（兼容性清单 3.1、3.2 节，阶段 2 验收文档的需求审计表一行）；`ctx.sub_hit = None`（每条规则之前清）是不可观察的防御（变异检查里唯一活下来的一个），照计划与 `pre_pass` 留着。门禁：80 个结果行、4174 个通过、0 个失败、12 个忽略 | `221a5d9` |
| 11 | 3 | **R3**（主持这一轮的会话代项目所有者作的裁定，所有者可以改）：计划没说 `Responder` 在哪里建、谁拿着；每个 TUN 只有一个，UDP 与 TCP 才共用 256 个转发许可（P9、P15）、池耗尽的 WARN 才是"每个 TUN 一次"（P10）：任务 3 只写这个类型，任务 4 在 `Tun::start` 里、建片之前为每个 TUN 建一个 `Arc<Responder>` 放进 `Context`，任务 5 把同一个 `Arc` 与 `hooks` 交给片（`Shard::new` 今天不拿 `hooks`）。用例 `one_responder_answers_over_udp_and_tcp` 钉住。代价：若实际接线要别的放法，改动只在 `lib.rs` 与片的构造上。**R6**：`FakeDns` 在 `rurge_tun::testing` 里，不用 `rurge_dns::testing`（`rurge-tun` 的 `testing` 特性不打开它，引擎的 dev-dependencies 恰好打开不算数）；造 `Echo` 应答直接改报文头的位，或用 `rurge_dns::message`。代价：没有 | `d3f9b38`、`dbe4f80`、`fd6ce07` |
| 12 | 3 | **R22**（主持这一轮的会话代项目所有者作的裁定，所有者可以改）：`Responder::new(hooks, pool)`，去掉计划写的 `stats` 参数与字段——计数只记丢掉的包（设计第 14 节），应答器的每条出路要么回一个应答、要么是不回的 `Drop`（P16：不计数），转发名额满了答 SERVFAIL 也不是丢包；只为将来可能计数而留一个靠 `allow(dead_code)` 活着的字段是预设；任务 4 的队列满了经交接函数自己的 `&Stats` 计 `queue_full`。代价：以后要应答器计数时把参数加回来（一个调用处）。**R23**：`src/tests/dns/answer.rs` 拆成两份（九个用例挪进 `forward.rs`，同一个 `tests::dns::` 下，名字不变）：任务 3 交的 933 行超过任务 0 的约 800 行。**计划的任务 3 一节写"用例 `src/tests/dns/answer.rs`"，现在转发的名额、限度与截断、OPT、设置只取一次与日志几节的用例在 `forward.rs`**，正文没有改。代价：只是测试文件的搬动 | `c33543b` |
| 13 | 3、5 | **R19**（主持这一轮的会话代项目所有者作的裁定，所有者可以改）：经 TCP 的 DNS 以两字节的长度给报文分帧，转发回来的应答长过 65,535 字节（DoH 的服务端可以回任意大的正文）就发不出去：应答器对 TCP 的 `Ask` 答 SERVFAIL（`fit`；UDP 上限度本来就截断它），`serve_dns` 写长度用 `u16::try_from`，失败是缺陷（`debug_assert!`，发行构建里像没有应答的消息那样关连接），从不 `as u16` 回绕。用例 `a_reply_too_long_for_tcp_gets_servfail`（65,535 字节整份、65,536 字节 SERVFAIL）；`try_from` 失败的那条路没有用例（够不到：应答器已经把长的换成了 SERVFAIL），变异成 `as u16` 也没有用例变红。代价：这样的上游在 TCP 上得到 SERVFAIL 而不是一份坏的应答；没有别的 | `d3f9b38`、`fd6ce07` |
| 14 | 3 | 任务 3 的其余出入与评审：`Extras` 多一个 `dns: Arc<FakeDns>`（任务 4、5 的用例要教它名字与应答），引擎一条用例 `the_tuns_settings_carry_the_dns_settings_of_the_profile`（`crates/rurge-engine/tests/tun.rs`，钉住 `Runtime::build` 填的四个新字段与 `impl TunDns for Resolver`；`mapped` 只认 `[Host]` 的地址映射，别名与没有命中的都是 `None`），都在计划的文件名单之外；应答器的 DEBUG 行一开始写在套用限度之前，不说应答随后被截断——评审的次要项，修正轮改到之后，另有一个 `fitted` 字段说出"被截断"或"TCP 上换成 SERVFAIL"。评审（规格符合，质量 Approved，没有 Critical 与 Important）的 7 条次要项：四对相邻的判定步骤没有用例钉住（类型类别在暗号之前、暗号在 SVCB / HTTPS 之前、地址族在 `always-real-ip` 之前、`always-real-ip` 在没有池之前）、转发中途被丢掉的 `answer` 的名额还回没有用例、DEBUG 行的位置——修正轮补了五条用例（`the_class_comes_before_the_canary`、`the_canary_comes_before_svcb_and_https`、`the_family_check_comes_before_always_real_ip`、`always_real_ip_comes_before_the_lack_of_a_pool`、`a_forward_dropped_midway_gives_its_place_back`），每对被对调它的变异杀死；`stats` 与 933 行见第 12 行。限定范围的复审：五项都 ADDRESSED，搬动的用例十项逐字节相同、两项只差第 12 行要的改动。门禁：80 个结果行，4203 个通过（修正轮 4208）、0 个失败、12 个忽略；47 个变异里 46 个被杀，活下来的一个是等价的（`REJECT-NO-DROP` 分支里多余的地址族判断，已删，靠 `message::answer` 只写所问地址族的约定） | `d3f9b38`、`c33543b` |
| 15 | 4 | 实现者做的、与计划不同的放法，评审接受。**R9**（主持这一轮的会话代项目所有者作的裁定，所有者可以改）：用例放处——应答器、`hijack-dns`、一阵查询与只问 DNS 的会话的空闲进 `src/tests/dns/udp.rs`，三条字节预算的用例进新的 `src/tests/udp/budget.rs`，`a_send_waiting_for_room_ends_at_the_stop` 进 `src/tests/udp/writer_queue.rs`，`a_session_over_lets_go_of_its_leases_though_its_client_is_kept` 进 `src/tests/udp/sessions.rs`：计划写的是全都进 `dns/udp.rs`，那样 CLAUDE.md 的 `tests::udp::` 过滤词选不中 UDP 会话自己的用例。代价：没有。**R10**：应答器的队列任务是 TUN 自己的任务（在任务集上起，等 `life.stopped()` 结束，队列不会自己关：`Context` 拿着它的发送端），`session_tasks()` 减 `shards.len() + 3`（片、写任务、读设备的任务与这个任务），每个查询的任务算会话的任务（同回显）。代价：若漏改，现有的 `session_tasks()` 断言差一（把 `+ 3` 改成 `+ 2` 使 19 个用例变红）。**R11**：`hijack-dns` 的用例用文档地址 `192.0.2.53:53` 代替计划写的 `8.8.8.8:53`（用例名不变），假 dialer 的 UDP 在内存里、不开套接字。代价：没有。**R12**：应答器在 UDP 上的应答是本地应答，同 rurge 自己地址的回显应答：`try_send` 给写任务，满了丢、计 `queue_full`，从不等空位——假应答不拿许可，等空位的任务在卡住的设备后面会无界地堆。代价：设备跟不上时应用重发查询（约 1 秒）；`a_burst_of_queries_is_answered` 因此要 `writer_queue` 1,100。顺带：任务 3 留在 `mod dns;` 上的死代码标记去掉了，一个不留；`Ids::new` 在 `getrandom` 失败时 WARN 一次（不带地址）；`udp::receive` 里 `parts` 失败的那一支加了说明与 `debug_assert!`（M3b 延后第 13 行） | `dbe4f80` |
| 16 | 4 | `udp_dns_idle` 不得大于 `udp_idle`（缺省 10 秒）带来的改动：把 `udp_idle` 调到 10 秒以下的现有用例要同时调 `udp_dns_idle`，`tests/udp/sessions.rs` 七处与 `tests/options.rs` 一处都设成同一个值，断言一条没改（行为，含只问 53 端口的会话，不变）。**R4**（主持这一轮的会话代项目所有者作的裁定，所有者可以改）：M3b 的 `udp_to_rurge_addresses_is_prohibited_unlimited` 往 `198.18.0.2:53` 与 `[fd00:6152::2]:53` 发包，任务 4 之后这是发往应答器，得不到 ICMP；计划没点名，那两项改成 54 端口（仍是应答器的地址、别的端口，照旧回 ICMP，用例的意思不变；提交说明点名），53 端口的行为由任务 4 自己的用例钉住。代价：没有。**R24**：`dns_queue` 与现有的 `shard_queue`、`writer_queue`、`udp_queue` 一样只要求大于 0、不加上限——这些选项由代码给（M4 的可执行文件用缺省值），不是使用者写的配置。代价：有调用方给出大于 2^61 的值时 `Tun::start` 在建通道时 panic。`udp_dns_idle` 的检查里没有"不大于 24 小时"一支：`udp_idle` 先查、至多一天，那一支决定不了什么（变异证明它是等价的），说明里仍写 24 小时 | `dbe4f80` |
| 17 | 4 | 任务 4 的评审：规格符合，质量 Approved，没有 Critical 与 Important，评审点名核对的十一项风险都 OK。次要项：`serve` 的扫除用 `udp_idle` 没有用例钉住（换成 `state.idle(options)` 什么都不红）、`tests/dns/udp.rs` 的 `Wire` 是 `tests/udp/mod.rs` 的 `Lab` 的近似副本、`reply_packets` 重复 `TunUdpClient::send` 的两步造包、两处措辞——修正轮做掉（新用例 `a_resolvers_destinations_are_kept_for_udp_idle`，扫除换成 `state.idle(options)` 时只有它红，变异的扫除在数据报之后 553 毫秒放手；DNS 的用例用 `Lab`；会话的回包与应答器的应答共用 `packet::udp_packets`）；队列字段没有上限——R24；真实设备上一阵查询丢应答——延后第 14 行；b3 用空钩子——第 26 行。复审：四项都 ADDRESSED（逐个用例按改名比对：17 个逐字相同，1 个只差注释）。范围之外的两条留给最终修正波：`udp_datagram` 只在 `packet.rs` 里用、仍是 `pub(crate)`；`tests/dns/mod.rs` 里另有一个私有的 `Lab`（与 `tests::udp::Lab` 同名、无关），要改个名字。不可避免的时限，本机上 100 多次稳定，CI 上没有量过——延后第 23 行。门禁：80 个结果行（79 个加 `rurge-policy` 的库测试二进制以 `STATUS_HEAP_CORRUPTION` 退出一次、单独重跑 181 个通过）、4235 个通过（修正轮 4236）、0 个失败、13 个忽略（多的一个是 b3）；31 个变异都被杀；新用例各单独跑 20 ～ 30 次，0 失败 | `dbe4f80`、`dd676ec` |
| 18 | 5 | **R5**（主持这一轮的会话代项目所有者作的裁定，所有者可以改）：计划写"那一条"，数少了：任务 5 让发往应答器 53 端口的 SYN 建套接字，M3a 的两条用例 `syns_turned_away_are_said_like_any_other_drop`（102 个 SYN 发往 `198.18.0.2:53` 量 RST 限频）与 `syns_to_rurge_addresses_are_reset`（`own` 里有 `198.18.0.2:53`）里的 `198.18.0.2:53` 都改成 `198.18.0.2:54`（照旧 RST；提交说明点名）。代价：没有。**R13**：`serve_dns` 照计划的签名一字不改；跑它的任务拿它与这条连接自己的停止令牌（`life.session_stop()`）赛跑，停止居先——计划给的签名里没有停止，被上游扣住的转发会让 `join` 一直等；丢掉 `serve_dns` 是安全的（它不拿会话的 handle、没有记录，片的最后一轮反正复位这条连接）。代价：没有。**R14**：`tcp_to_a_hijacked_server_is_a_connection` 同 R11 用 `192.0.2.53:53`。代价：没有。**R15**：SYN 层问预匹配的会话与 `session::serve` 拨号的会话由同一个函数（`session::tcp_session`）造，两处永远一样。代价：没有。接线与计划写的"交给 `serve_dns`"有一处不同：`session::serve` 仍是片起的唯一函数，它把 `Purpose::Dns` 的连接交给 `answer_dns`（再到 `dns::tcp::serve_dns`），等握手完成、停止之后不再服务这两件事因此只写一遍 | `fd6ce07` |
| 19 | 5 | **R25**（主持这一轮的会话代项目所有者作的裁定，所有者可以改）：`serve_dns` 关连接时先 `shutdown`（FIN）、再读掉丢弃应用还在发的字节直到应用的 FIN 或 `dns_tcp_idle`、然后才丢流——计划（"关的时候先 `shutdown` 再丢（FIN）"）的前提错了：`ShardState::stream_dropped` 在 smoltcp 的 `may_recv()` 仍为真时（FIN-WAIT-1 与 FIN-WAIT-2）复位被丢掉的流，`shutdown` 之后马上丢流把 FIN 变成 RST，最后一个应答与 FIN 一起丢了；照字面写的第一个版本里四条关连接的用例全红（应用读到 `ConnectionReset`，不是流的结束），`session::hold_in_silence`（`REJECT-DROP`）本来就是这样做的。应答的写也以 `dns_tcp_idle` 为限（超时或出错就丢流）。修正轮把 `close` 与 `hold_in_silence` 的后半合成 `TunStream::close_and_wait_for_the_app(limit)`（两个调用者共用；放在 `TunStream` 上免得 `session` 与 `dns::tcp` 互相引用）。代价：应用不关自己那一半时，连接与它的任务多占至多 `dns_tcp_idle`（10 秒）；写不出去的应答至多等 10 秒。用例 `an_app_that_closes_its_side_ends_the_connection`、`an_app_that_reads_nothing_has_its_connection_reset`、`an_app_that_never_closes_is_let_go_after_the_idle_time` | `fd6ce07`、`5c993d1` |
| 20 | 5 | 任务 5 的其余与评审。计划为"SYN 在拿了租约之后没被协议栈收下"备了一个"只在测试构建里的触发点"：**没有用到**——smoltcp 0.12 在 `State::Listen` 里忽略 MSS 为 0 的 SYN（`socket/tcp.rs` 的 `(State::Listen, TcpControl::Syn)` 一支），套接字留在 `Listen`，`open` 拿掉它、还租约，应用一侧手造一个 MSS 为 0 的 SYN 就够了；用例 `a_syn_the_stack_does_not_take_gives_its_lease_back`（M3b 延后第 12 行），`open` 里提前返回之前 `std::mem::forget(purpose)` 时红。`serve` 里的 `stopped` 标志去掉了（M3b 延后第 20 行）。循环跑出并改掉了自己两条用例的偶发：`an_app_that_reads_nothing_has_its_connection_reset`（30 次里 2 次读到 `Ok(1)`）、`tcp_to_a_hijacked_server_is_a_connection`（40 次里 4 次在应用的 `write_all` 处 `ConnectionReset`，现在不发数据）。评审：规格符合，质量 Needs fixes (small)，十一项点名核对的风险都 OK；一条 Important——TUN 发出 FIN 之后等应用关闭的那一段的时限没有用例钉住，去掉它什么都不红——，修正轮补 `an_app_that_never_closes_is_let_go_after_the_idle_time`（去掉共用关闭函数里的 `timeout` 时红），并做了次要项：DNS 那一臂装箱（每个 TCP 会话的 future 在调试构建里少 664 字节：856 对 1520）、`close` 与 `hold_in_silence` 合成一个函数、`Serving` 的字段私有、`dns_tcp_idle` 的说明补上第三种用途、两处"什么都不发生"的等待写明余量、`next_query` 随收到的字节读（`stream.take(length).read_to_end`，应用说 65,535 而什么也不发时不占预分配的内存；`a_query_that_stops_halfway_ends_the_connection` 加了第二条连接盖住新的短读那条路）。复审：七项都 ADDRESSED；一条措辞（`close_and_wait_for_the_app` 的说明说"The stream is let go after that"，而方法拿 `&mut self`、放手的是调用方）与一条确定性的缺口（R25 的前提在 DNS 一侧没有确定地钉住：应用发查询与 `[0, 0]`、过约 200 毫秒才读，`serve_dns` 在 `shutdown` 之后立刻放手时确定地红）留给最终修正波。**R26**（主持这一轮的会话代项目所有者作的裁定，所有者可以改；与本计划无关的偶发失败）：修正轮的三次全量运行里有一次，`crates/rurge/tests/cli.rs` 的 `run::run_proxies_http_and_rejects_by_rule` 用非阻塞的 `try_iter` 找守护进程的 `running:` 行，而守护进程在两个 `listening on` 之后、先写 `state.json`（阶段 1 / M5a 起）才打印它，监听器那时已经在应答，两次 HTTP 请求可以先做完——用例自己的竞争，自 2026-10-11 起就有，与本计划的改动无关（`rurge run` 不起 TUN）；单独跑 40 次、整个 `cli` 二进制 6 次都过。最终修正波里改成有界地等这一行，并看同一文件里另三处 `try_iter` 有没有同样的竞争、有才改。代价：一处只动测试的改动落在本计划的 crate 之外。门禁：80 个结果行、4260 个通过（修正轮 4261）、0 个失败、13 个忽略；20 个变异都被杀（修正轮三个）；24 个新用例各单独跑 25 次，0 失败 | `fd6ce07`、`5c993d1`；R26：最终修正波 |
| 21 | 6 | **R16**（主持这一轮的会话代项目所有者作的裁定，所有者可以改）：经 SOCKS5 来的 UDP 流，回包的来源在引擎里换回 fake 地址：`Flow` 记下客户端写的目标（换之前的）`asked` 与有没有换过 `swapped`，`answers` 给的就是 `asked`；回包应答一条换过的流时，交给客户端的来源与 `answers` 都是那个 fake 端点；`answers` 为 `None`（不知道应答哪一条）的回包保留真实的来源。计划写的是改 `crates/rurge-inbound/src/udp.rs` 里 SOCKS5 的 `UdpClient`（它今天不看 `_answers`），那要让 `rurge-inbound` 依赖 `rurge-dns` 才认得 rurge 的地址空间，依赖方向里没有这条边；`rurge-inbound` 不动。用例 `a_socks5_udp_flow_to_a_fake_address_goes_to_its_name_and_answers_from_it`、`a_reply_that_may_be_for_either_flow_keeps_its_real_source`、`a_socks5_udp_flow_to_an_unknown_fake_address_fails`。代价：没有（TUN 的流不换，目标照旧）。限制：两条流发往同一个目标时回包保留真实的来源，SOCKS5 客户端对不上（兼容性清单 6.4 节） | `e5c4b0d` |
| 22 | 6 | **R8**（主持这一轮的会话代项目所有者作的裁定，所有者可以改）：`pre_matching_escalates_to_silence` 照 `connect_session` 的次序（先 `record` 后 `should_drop`，所以第 `ESCALATE_COUNT`（50）次本身就静默）：前 `ESCALATE_COUNT - 1` 次是空应答，第 `ESCALATE_COUNT` 次起没有应答；用例断言前 `ESCALATE_COUNT - 1` 次有应答、第 `ESCALATE_COUNT + 1` 次没有（有界的等待，注释写它等的是什么）——计划的"最后一次没有应答"没说边界。代价：没有。另加 `pre_matching_with_reject_tinygif_escalates_to_silence` 与 `pre_matching_with_reject_no_drop_is_never_upgraded`（变异"`REJECT-TINYGIF` 不计入升级"曾活下来，补了这两条才杀死，并加了变异"每一种都计入"） | `e5c4b0d` |
| 23 | 6 | 实现者在经 HTTP 监听器的明文请求上看到的（一条临时用例，没有提交）：`GET http://<fake 地址>:<端口>/…` 换回名字之后，会话的 `dst_host`（记录、规则）是名字，而 `session.url`、请求行与 `Host` 仍是 fake 地址——经 `http` 策略（绝对形式）时上游代理被要求去连 `http://198.18.x.y/…`（连不上），直连时源站看到 `Host: 198.18.x.y:<端口>`；`CONNECT` 与 SOCKS5 的目标里是名字，不受影响。**R27**（主持这一轮的会话代项目所有者作的裁定，所有者可以改）：M3c 不改，登记进兼容性清单与延后第 19 行，去向阶段 4（HTTP 引擎在入站 / HTTP 层改写请求行与 `Host`）。理由：要一个先经 TUN 应答器解析、再把 IP 字面量写进明文请求交给代理的应用，少见，且 M4 之前没有应用拿得到 fake 地址；完整的修法要改写请求行与 `Host`，是阶段 4 的事；只改 `session.url` 会让记录与 `URL-REGEX` 跟转发出去的不一致。代价（若错）：M4 到阶段 4 之间这样的请求经 `http` 策略失败，`URL-REGEX` 与扩展匹配看到的是 fake 地址、漏判 | `e5c4b0d` |
| 24 | 6 | 任务 6 的其余：`Engine::pre_match_refusal` 与 `Recorded`（`due(rule, now)` 是带 `now` 的纯函数，4 条单元用例；该记时才查进程、建 handle，不该记时只计升级）在新文件 `crates/rurge-engine/src/prematch.rs`；`fake_pool` 是 `ArcSwapOption<FakeIpPool>`，`start_tun` 在池打开之后、`Tun::start` 之前放进去，TUN 收尾的任务在最后一次保存之后用 `let_go` 清（只清自己放的那一个池：其间起来的另一个 TUN 的池留着，单元用例 `a_pool_is_let_go_by_its_own_end_only`）；`Dialer::dial` 在 `look_up_owner` 之前 `turn_back`，换回名字的租约挂在 `handle.on_finish` 上，池里没有的是 `fail(handle, FailKind::Other, FAKE_ADDRESS_NOT_KNOWN)`（HTTP 502，SOCKS5 `0x05`）；记录与会话日志的 `dst` 在端口为 0 时只写主机（`observe::destination` 一个函数，`record_of` 与 `log_session` 共用）；`choose_policy` 里写子规则说明的几行成了 `note_sub_rule`，预匹配的记录共用它。计划列的 12 条用例加 11 条；八条在第一次转绿之后补，没有先红，用变异证明能红（34 个变异全部被杀，第一遍 33 个里活下来一个，补了两条升级的用例才杀死）；`tests/tun.rs` 的接线挪进 `tests/common/tun.rs`（P26），`--list` 前后相同（17 个）；`tests/tun_dns.rs` 847 行，超过 P25 的约 800 行——延后第 16 行 | `e5c4b0d` |
| 25 | 6 | 任务 6 的评审：规格符合，质量 Approved，Critical 0、Important 0、Minor 7（升级的一段在 `prematch.rs` 与 `engine.rs` 各写一遍、五分钟没有字面钉住、`tun_dns.rs` 里一条按到达的次序断言、`leased()` 读池的 `Debug` 文字、`Flow::new` 先算 `asked` 再由 `open` 改写、八条用例不是先红、记录与 INFO 在片的锁里写）。**R28**（主持这一轮的会话代项目所有者作的裁定，所有者可以改）：次要项 1 ～ 5 进最终修正波；6 不改（34 个变异全杀已补偿"先红"）；7 不改——SYN 层的钩子在片的锁里建记录并写 INFO，stdout 被堵时每个写日志的线程本来都停，这一行至多每条规则每 5 分钟每一代一次。代价（若错）：控制台被选中而 stdout 停住时，那个片在写这一行时停住（延后第 22 行）。评审对任务 6 报告里关切的逐条判断：决定 6 的观察不是任务 6 的缺陷（它的范围里不改），交文档登记——第 23 行与兼容性清单；`tun_dns.rs` 847 行——延后第 16 行；钩子不看 `strict` 暂停、`Proxy` 模式而全局策略不可用时引擎在拨号处按规则走——延后第 17 行；30 秒 50 次按每个被拒的 A / AAAA 查询数——延后第 18 行；`NOTHING` 的 300 毫秒在 CI 上没见过——延后第 23 行。另交文档的：运行 TUN 时整个 rurge 地址空间都按 fake 地址认、两条流同一目标时回包保留真实的来源、换回名字的 UDP 流拿着租约到它结束（空闲 60 秒）——延后第 20、21 行与 `docs/api/phase3.md`。门禁：81 个结果行（80 个加 `rurge-api` 的 `api` 测试二进制以 `STATUS_HEAP_CORRUPTION` 退出一次、单独重跑 56 个通过）、4292 个通过、0 个失败、13 个忽略；循环：23 个新用例各单独跑 25 次、整个 `tun_dns` 20 次，0 失败 | `e5c4b0d`；次要项 1 ～ 5：最终修正波 |
| 26 | 4、7 | 基准（一台 Windows 11 x86-64 的开发机，release 构建，每次一个进程；不作门禁）。**b3**（`cargo test -p rurge-tun --release --lib -- --ignored --exact tests::bench::b3_fake_answer_latency --nocapture`）：十万个不同的名字、一个线程，量 `Responder::answer` 加造应答包，不含设备与写任务；钩子是什么也不记的空钩子，**所以是整条路的下限**，不含引擎的规则评估（`pre_match`）、进程查找和设备的系统调用；NFR-01 要小于 0.2 毫秒。任务 4 三次：中位数 0.8 / 0.8 / 0.9 微秒，p99 3.5 / 3.5 / 4.1 微秒，最大值 4.0 / 5.5 / 7.0 毫秒；修正轮一次：0.9 微秒、4.7 微秒、5.7 毫秒；任务 7 在 `e5c4b0d` 上一次：**中位数 0.9 微秒、p99 3.2 微秒、最大 5.8 毫秒**。中位数与 p99 比线低两百多倍与五六十倍；最大值在线之上，打印了下标的四次（任务 4 的后两次、修正轮一次、任务 7 一次；任务 4 的第一次没有打印下标）都落在第 57,345 个不同的名字上：57,344 是 65,536 的八分之七，`hashbrown` 的 `HashMap` 翻倍的地方，`FakeIpPool` 的 `indices` 没有预分配（下一次在第 114,689 个名字）——读代码得出的解释，没有改池去验证（延后第 12 行）。**B2**（缺省设置：256 片、`buffer_min` 4 KiB、`tx_max` 256 KiB、预算 64 MiB、MTU 1500；三次，第一次是任务 4 在 `dbe4f80` 上跑的，后两次是任务 7 在 `e5c4b0d` 上跑的）：峰值 472,125,440 / 476,844,032 / 476,901,376 字节；**门槛读数**（峰值减去全部测试一侧的项）397,695,076 / 402,413,668 / 402,471,012 字节（397.7 / 402.4 / 402.5 MB），对着 429,000,000 字节的线都在线以内，余量 31.3 / 26.6 / 26.5 MB；**保守读数**（只减去准的应用端套接字缓冲）439,357,540 / 444,076,132 / 444,133,476 字节，**超线** 10.4 / 15.1 / 15.1 MB；五个计数全 0；一万条连接开好用 0.52 / 0.46 / 0.44 秒；一百条活跃连接各 4 MiB 两个方向用 19.23 / 19.57 / 15.06 秒。M3a 最后一次的门槛读数是 390.4 MB（余量 38.6 MB）、保守读数超线 3.1 MB。三次里只有同在 `e5c4b0d` 上的后两次可以互相比（门槛读数差 0.06 MB）；第一次在 `dbe4f80` 上，与它们隔着 `dd676ec`、`fd6ce07`、`5c993d1`、`e5c4b0d` 四个提交（`session.rs` 与 `tcp/shard.rs` 共加 223 行、减 91 行，改的正是每条连接走的代码），门槛读数比后两次低 4.7 MB。所以自 M3a 起的涨幅要拿 `e5c4b0d` 上的那两次与 M3a 最后一次比：门槛读数高了 12.0 与 12.1 MB，保守读数超线多了 12.0 MB（第一次的 7.3 MB 是在更早的提交上，不与它们放在一起算）。M3b 最终修正波给每条转发中的会话多的子令牌，复审估一万条约 2 MB（M3b 记录第 21 行），这里多出的比它大，**原因没有查**。保守读法要不要当判据、预算要不要调到 48 MiB 仍归项目所有者（M3a 延后第 15 行） | — |
| 27 | 7 | 文档（M3 收尾）：兼容性清单、`docs/api/phase3.md` 的「M3c」一节、`docs/acceptance/phase3-manual.md` 的 M3 一节（19 项：Windows 3、macOS 3、Linux 3、与平台无关 10；七个 Python 小脚本都在回环上对着假服务器跑通过）、`docs/requirements.md` 阶段 3 的状态行、两份 README、CLAUDE.md、设计第 18 节第 23 ～ 33 条与正文各处指过去的括号、M3b 计划延后事项第 2、3、5、7、9、11、12、13、15、19 ～ 22、29 行的去向、`docs/acceptance/phase2-acceptance.md` 里提着去掉的 `pre_match_domain` / `pre_match_ip` 的一行、本计划末尾这两张表。兼容性清单：4.1 节 `REJECT-TINYGIF`、`REJECT-DROP`、pre-matching 下的 DNS 与 TCP 两行、UDP 行为和"5 分钟只记一次"一行（那一行的阶段栏原写 4，改成 3），3.1 节 `SUBNET` 与 3.2 节 `pre-matching` 里提着去掉的两个函数的两处，6.3 节 `server:system` 与 `server:force-syslib`，6.4 节原有的九行加新的一行（经监听器来的 fake 地址，🟡），9.1 节 ICMP 与 fake IP 两行，`[General]` 的四个键；9.1 节"只处理 TCP / UDP / ICMP"一行从 ✅ 改成 🟡（登记了每个 UDP 会话 1024 个目标和同一个端口上两个名字的回包来源）。附录整张重数：用一个不在仓库里的脚本数每个 `## N.` 节里表格的每一行的第一个状态标记（表头行、分隔行和 10.3 节命令表里没有状态标记的 17 行不计），先在没有改动的清单上数出与旧附录逐格相同的数字再改：566 行变成 567 行，✅ 394 变成 393，🟡 123 变成 125。两处读代码得出的说法：一、使用者在 M4 之前看得到什么——连经监听器来的那一半也要有 TUN 的池（`Engine.fake_pool` 只由 `start_tun` 放），`rurge run` 今天从不放，所以使用者今天看不到这个计划的任何 TUN 方面的东西；能看到的只有两处小的：`rurge dns lookup --trace` 一类输出里 FORMERR 与 NOTIMP 的文字变了（第 5 行），和 UDP 上游给在途的同 ID 的第二个查询换一个空着的 ID（原来第二个查询顶掉第一个的等待者，第一个以 `waiter replaced or dropped` 失败，两个同时在途的查询碰巧带着同一个 ID 时才碰得到；`Pending::claim`，第 4 行）；API 文档、README 与需求文档照这样写，说到 M3 整体的地方还加 M3b 的三处（`GET /v1/traffic` 的 `listener.tun`，恒为 0；进程查找 DEBUG 行的 `remote`；`rurge run` 的 TCP 进程查找连远端地址一起比，`platform_query`）；二、手工清单 P27 的监听器那一半不需要 TUN，可以在 M4 之前先做，其余每一项都要真实设备上的 TUN。基准见第 26 行。门禁（一台 Windows 11 x86-64 的开发机上的一次运行，不是 CI）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 干净，`cargo test --workspace --no-fail-fast` 80 个结果行、4236 个通过、0 个失败、13 个忽略，`rurge-api` 的 `api` 测试二进制以 `STATUS_HEAP_CORRUPTION` 退出一次（开发机终端管控软件注入的钩子 DLL，不是 rurge 的），单独重跑 56 个通过，合起来 81 个结果行、4292 个通过，与 `e5c4b0d` 上任务 6 的那一次相同。评审（两位新的评审者，各看一半）要改的是措辞、计数、出处和几处手工清单的步骤，结论没有变：B2 三次读数的出处（第一次在 `dbe4f80` 上）、b3 最大值的下标与解释、使用者在 M4 之前看得到什么（多了 UDP 上游的 ID 和 M3b 的三处）、R12 的 `try_send` 只在 UDP 上、AA 与 RA 只在 `message::answer` 造的应答里、几份文档里已经过时的前向引用、设计第 18 节的几处缺口，和手工清单 P20、P22、P25、P27、P28、M15 与 `dns_latency.py`；改正在紧随其后的提交里 | `d0147ec`；评审之后的改正：紧随其后的提交 |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | SYN 层与监听器的预匹配那一遍，把 `USER-AGENT`、`URL-REGEX` 与对名字的 IP 类规则（`no-resolve`）读成不命中，所以它们外面的 `NOT` 成立（今天的行为，没有改） | 有用户报告再说 |
| 2 | `.local` 与单标签名字的其余类型转发给主上游，不经系统解析库与搜索域（P4） | 有用户报告再说 |
| 3 | 应答器的队列满只在单元用例里量过 | 下次动到时 |
| 4 | 系统的 DNS 服务器名单要排除应答器与虚拟网卡的地址（设计第 11.3 节） | M4 |
| 5 | fake 应答的延迟与转发的往返在真实设备上的数字 | M4 的手工验收 |
| 6 | 预匹配的记录写的 INFO 行带名字（P18；M3b 延后第 1 行） | 项目所有者 |
| 7 | DNS over TCP 流水线发来的查询照次序一个接一个答，不并发（P15） | 有用户报告再说 |
| 8 | 任务 1：`crates/rurge-dns/src/resolver.rs` 约 3,900 行，`message.rs` 翻了一倍（都是计划定的位置，没有拆） | 下次动到时先拆 |
| 9 | 任务 1 评审：`record_types` 与 `ttl_offsets`（`crates/rurge-dns/src/message.rs`）各走一遍报文里的记录，两个走法要一起改 | 下次动到时 |
| 10 | R20：`forward_raw` 不把出错的上游算作答过，上游全挂（或全部立刻出错）时应用要等至多 5 秒才拿到 SERVFAIL，期间每个查询占一个转发名额 | 有用户报告再说 |
| 11 | R21：`rurge_dns::message::Request` 派生的 `Debug` 打印被问的名字，以后有人在 INFO 里 `{:?}` 它就漏出名字 | 下次动到时 |
| 12 | 任务 4：b3 的最大值（4 ～ 7 毫秒；打印了下标的四次都在第 57,345 个不同的名字上，第一次没有打印下标）落在 fake-IP 池的 `HashMap` 翻倍处，`indices` 没有预分配（下一次在第 114,689 个名字）；预分配 130,814 个名字约多占 8 MB。这是读代码得出的解释，没有改池验证 | 有用户报告再说；M4 在真实设备上量应答的延迟时一并看 |
| 13 | B2 的读数（记录第 26 行；三次里第一次在 `dbe4f80` 上，后两次在 `e5c4b0d` 上，只有后两次可以互相比，差 0.06 MB）：门槛读数 402.5 MB，余量 26.5 MB（M3a 最后一次 38.6 MB），保守读数超线 15.1 MB（M3a 最后一次 3.1 MB），自 M3a 起高了约 12.0 MB（`e5c4b0d` 上的两次对 M3a 最后一次）；多出的部分原因没有查；保守读法要不要当判据、预算要不要调到 48 MiB（约省 17 MB，没有量过） | 项目所有者（M3a 延后第 15 行，数字以这一行为准）；阶段 3 / M6 的验收 |
| 14 | 真实设备上一阵来上千个查询时，写任务的队列（缺省 `writer_queue` 256）放不下的应答被丢（`try_send`，R12），应用约 1 秒后重问；用例用 1,100 的队列 | M4 的手工验收（手工清单 P22） |
| 15 | 任务 5：`crates/rurge-tun/src/tcp/shard.rs` 约 1,200 行（新的 `refused_by_rules` 要用片的私有字段），M3b 延后第 15 行说的同一件事 | 下次动到时拆 |
| 16 | 任务 6：`crates/rurge-engine/tests/tun_dns.rs` 847 行（P25 的约 800 行） | 下次动到时拆 |
| 17 | 预匹配的钩子不看 `strict` 暂停，只在出站模式是规则模式时答；模式是全局而全局策略不可用时，引擎在拨号处按规则走（含预匹配那一遍），这样的拒绝落在拨号，不在 DNS 或 SYN | 有用户报告再说（兼容性清单 4.1 节登记） |
| 18 | 30 秒 50 次的自动升级按每个被拒的 A / AAAA 查询数（一次查找问 A 与 AAAA 的应用比按会话数更早静默） | 有用户报告再说（兼容性清单 4.1 节登记） |
| 19 | R27：经 HTTP 监听器来的明文请求 `GET http://<fake 地址>/…`，换回名字之后请求行、`Host` 与 `session.url` 仍是 fake 地址（`URL-REGEX` 与扩展匹配看到的也是） | 阶段 4（HTTP 引擎改写请求行与 `Host`；兼容性清单 6.4 节登记） |
| 20 | 运行 TUN 时 rurge 的整个地址空间（`198.18.0.0/15` 与 `fd00:6152::/64`）都按 fake 地址认：经代理端口去 rurge 自己的地址（`198.18.0.1`、`.2`、`.244`）或这个空间里池不认识的地址，得 `fake address not known`（HTTP 502 / SOCKS5 `0x05`）；这个空间里真实的主机经代理端口到不了 | 有用户报告再说（兼容性清单 6.4 节登记；`docs/api/phase3.md`） |
| 21 | 两条流发往同一个目标（两个 fake 地址，或一个名字与它自己的真实地址，同 IP 同端口）时，回包保留真实的来源，SOCKS5 客户端对不上（R16 的限制，用例 `a_reply_that_may_be_for_either_flow_keeps_its_real_source`）；换回名字的 UDP 流在它的一生里拿着租约（空闲 60 秒结束） | 有用户报告再说（兼容性清单 6.4 节登记） |
| 22 | R28：预匹配被拒的名字或连接进 INFO 的那一行（P18）与请求记录，在 SYN 层是在片的锁里同步写出的（至多每条规则每 5 分钟每一代一次）；stdout 被堵时那个片停住 | 下次动到时（与第 6 行一起看） |
| 23 | **第一次 CI 运行时核对**：本计划的提交没有一个被 CI 见过（分支 `phase3-m3c`，没有推送）。`rurge-tun` 的全部用例、引擎的 TUN 用例（`--test tun`、`--test tun_dns`）、三个基准、`rurge-dns` 与 `rurge-rules` 里新加的用例，只在一台 Windows 11 x86-64 的开发机上编译、运行过（代码平台无关，没有只在 Linux 或 macOS 上编译的行，除了 `tests/bench/peak.rs` 里读峰值内存的 Linux 与其余平台两段）。计时用例的余量在慢的机器上没有量过：`a_send_waiting_for_room_ends_at_the_stop`（停止后 `STOP_GRACE` 加 500 毫秒）、`a_swept_destination_does_not_make_a_session_a_resolvers_again`（400 ～ 700 毫秒的余量）、`a_resolvers_destinations_are_kept_for_udp_idle`（约 2.4 秒，700 与 800 毫秒的余量）、`a_syn_dropped_by_pre_matching_gets_nothing`（等 200 毫秒，smoltcp 的第一次重发在约 700 毫秒）、`a_forward_in_progress_is_not_idle_time`（等 600 毫秒，空闲时间 300 毫秒），和 `tun_dns.rs` 里"什么都不发生"的等待（`NOTHING` = 300 毫秒，决定在读到查询时就做出，微秒级）。`rurge-dns` 里依赖时钟的新用例走暂停的时钟与内存里的上游，不在此列 | 第一次 CI 运行时核对 |
| 24 | M4 的手工验收（`docs/acceptance/phase3-manual.md` 的 M3 一节，19 项，一项也没有做过）：B1 的数字 → P19；B2 的数字 → P20；应答的延迟与转发的往返（本计划延后第 5 行）→ P21；一阵查询（第 14 行）→ P22；真实应用（浏览器、`dig`、`nslookup`、带 0x20 的解析器）问应答器 → W18、M16、L17、P23；`hijack-dns` → P24；预匹配在真实应用上 → P25；经监听器来的 fake 地址 → P26；`local`（M3b 延后第 6 行）→ P27；计数与日志行 → P28；零窗口探测（M3a 延后第 11 行）→ W16、M14、L15；`STOP_GRACE` 与最后一轮的复位（M3a 延后第 13 行）→ W17、M15、L16 | M4 的手工验收 |
