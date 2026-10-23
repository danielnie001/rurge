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

| # | 任务 | 修正 | 提交 |
| - | ---- | ---- | ---- |

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
