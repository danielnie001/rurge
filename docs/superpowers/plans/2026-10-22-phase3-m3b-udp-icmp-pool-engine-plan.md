# 阶段 3 / M3b「UDP、ICMP、fake-IP 池与引擎接入」实施计划

> **执行方式**：子代理驱动（superpowers:subagent-driven-development）——每个任务一个新的实现者，做完由新的评审者评审，全部做完后整分支评审一次。本计划写到"改哪里、接口是什么、要哪些用例、判据是什么"为止：实现者先写失败的用例、看到红，再实现到绿；与计划的出入记进末尾的「执行期修正记录」。

**目标**：在 M3a 的 TUN 内核上加 UDP 会话、ICMP 回显与 fake-IP 池，并把 TUN 接进引擎——经虚拟网卡来的 TCP 连接、UDP 数据报与回显请求照规则与策略走（发往 fake 地址的照它的名字走，被拒的 UDP 流以 ICMP"管理上禁止"回答），进程查找按会话自己的形状查；池记在数据目录里，上一次没有干净地停下时，接下来会交出的 2048 个地址隔离 120 秒。同时做掉 M3a 整分支终审交下来的两件（I1：TIME-WAIT 里会话没读的数据；I2：停止不结束卡在上游上的转发）与 `TunOptions` 的校验。

**架构**：池是 `rurge-dns` 的新模块 `fakeip`（一个编号同时给出 v4 与 v6 地址，`lru` 回收最久没用到的，有租约的编号从不回收，一个保存任务至多每 10 秒写一次 `fake-ip.txt`）。`rurge-tun` 对引擎只认三样：`Dialer`（TCP 的 `dial` / `relay`，UDP 的 `admit_udp` / `associate`）、`Hooks`（这一代的 `TunSettings`：解析器与 `icmp-forwarding`）、`Pinger`（M3b 里只有测试的假实现）。UDP 每个应用的源端点一个会话，会话实现 `UdpClient`，引擎现有的 UDP 流水线照旧管流、载体与回收；回包经新的 `answers` 写成应用认得的来源，被拒的流经新的 `refused` 回 ICMP。`Engine::start_tun` 打开池、起 TUN 与保存任务；`SessionInfo.local` 让进程查找按四元组查（关掉阶段 3 / M2 延后第 17 ～ 19 行）。应答器、预匹配与经监听器来的 fake 地址是 M3c。

**技术栈**：smoltcp 0.12.0 的 `wire`（UDP、ICMPv4 / ICMPv6 与 IPv6 分片首部的构造与解析）、`lru` 0.18（`rurge-dns` 已依赖）、tokio 与 tokio-util、`getrandom`；不新增包。

**依据**：设计 `docs/superpowers/specs/2026-10-18-phase3-m3-tun-core-design.md`（下面写"设计"指它）第 2、3.3、7 ～ 10、14、15.2、17、18 节与 M3-D8、D11 ～ D18、D22、D24 ～ D26；M3a 计划 `docs/superpowers/plans/2026-10-18-phase3-m3a-tun-tcp-plan.md`（下面写"M3a"）的延后第 7、8、10、12、16、17 行；阶段 3 / M2 计划 `docs/superpowers/plans/2026-10-17-phase3-m2-process-plan.md` 的延后第 17 ～ 19 行。本计划写在 M3a 合并之后（`f3c09d2`），引用的 M3a 名字都对着代码核对过（P1）。

## 全局约束

- 不新增外部包。新的依赖边只有：`rurge-tun → rurge-dns`（`FakeIpPool`；dev-dependency 另开 `rurge-dns` 的 `testing` 特性）、`rurge-engine → rurge-tun`（dev-dependency 另开 `rurge-tun` 的 `testing` 特性），根 `Cargo.toml` 的 `[workspace.dependencies]` 加 `rurge-tun = { path = "crates/rurge-tun" }`。`Cargo.lock` 只有依赖边的变化，没有新的包（518 个）。`rurge-tun` 照旧不依赖 `rurge-engine` 与 `rurge-platform`。
- `rurge-platform` 放开 `unsafe_code` 的函数名单（`LISTED`，七个）不变；别的 crate 照旧 `forbid`。MSRV 1.89，edition 2024。
- 名字与地址只在 DEBUG 的行与请求记录里（设计第 14 节、M3-D26）；WARN 不带名字与地址；池的文件里有名字（它在数据目录里）。引擎的会话日志行照今天的写法不改（P13）。
- 测试只用回环与端口 0。`rurge-tun` 的用例经假 `Dialer`（只拨用例写给它的回环地址，M3a P19）与假 `Pinger`（什么也不发出去）；经引擎的端到端用例守设计第 15.2 节的两道防线（P17）。用例从不建网卡、不改路由与 DNS、不发真实的 ICMP。
- 等待都有时限，不用固定的 sleep 当同步（"断言什么都没发生"除外，注释写它等的是什么、余量多大）。新写的计时用例提交前单独循环跑二十次以上。跑 TUN 内核的用例不用暂停的时钟（M3a 的规矩：内核用 `std::time::Instant`）；池的时间由调用方传 `now`，用例不等 120 秒。
- std 的锁不跨 `await`；要跨的只有 UDP 会话收队列的那一把，用 `tokio::sync::Mutex`（任务 7）。
- 先写失败的用例、看到红，再实现到绿。现有用例变红时：计划点了名的照计划改并在提交说明里说；没点名的停下来报告，不顺手改期望值。
- 门禁（每个任务末尾一次）：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`（本机 `RUSTFMT` 指向 gnu 工具链的 rustfmt）。聚焦地跑用例也经整个工作区（`cargo test -j 8 --workspace --lib -- <过滤词>`，集成测试 `cargo test -j 8 --workspace --test <名字> -- <过滤词>`），不用 `-p`。测试二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出时单独重跑那一个。跑全量之前 `df -h /d`，剩余不足约 15 GiB 就停下。
- 只运行本仓库构建出来的二进制与系统自带的只读工具；不下载、不安装任何东西。
- 参考实现：GPL 的（mihomo、sing-box、sing-tun）只取事实，代码不抄；MIT / Apache 的可以简短引用并注明出处；smoltcp 是 0BSD。
- 注释、日志与错误文本用英文，照所在文件的写法；提交说明的标题用中文。
- 没有新的诊断码，不碰配置格式。兼容性清单、手工清单与 README 随 M3c（设计第 16 节）；`docs/api/phase3.md` 只记这一份里使用者看得到的变化（任务 10）。

## 评审要点

计划的用例之外最可能咬到使用者的五种情形，每条的用例已加进负责的任务：

1. **开着很久的连接**（SSH、数据库连接池、长连接的 HTTP/2）连的是 fake 地址，其间别的应用查了很多新名字：这条连接的地址不被回收给别的名字，数据照常往返（任务 6 `a_connection_holds_its_name_for_its_life`）。
2. **第三方发来的 UDP 回包**（STUN 的另一个对端、P2P）：回包的来源是真正发它的地址，不改写成应用当初发往的地址（任务 7 `a_third_party_reply_keeps_its_source`）。
3. **大于 MTU 的 UDP 回包**（DNS over UDP 的大应答、游戏的大包）：按 IPv4 / IPv6 的规矩分片写回，应用一侧重组后逐字节相同（任务 7 `a_reply_larger_than_the_mtu_is_fragmented`）。
4. **rurge 崩溃或被强杀**，包括第一次保存之前：重启之后接下来会交出的 2048 个地址隔离 120 秒，应用手里的旧地址不会指到新的名字（任务 2 `an_unclean_stop_quarantines_the_next_2048`、`a_crash_before_the_first_save_is_still_unclean`）。
5. **应用对一个被拒的 UDP 目的地不停地重发**（QUIC 的重试、游戏的心跳）：`REJECT` 的 ICMP 3 秒至多 100 个，`REJECT-NO-DROP` 不限，`REJECT-DROP` 一个也不回（任务 7 `icmp_for_refused_flows_is_limited`）。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `crates/rurge-config/src/session.rs` | 1 | `SessionInfo.local` |
| `crates/rurge-inbound/src/{listener.rs,http.rs,socks5.rs}` | 1 | 被接受的连接的本地地址进 `SessionInfo.local` |
| `crates/rurge-engine/src/{owner.rs,engine.rs}` | 1 | `OwnerQuery.remote`、`query_for` |
| `crates/rurge-platform/src/procinfo/{mod.rs,linux.rs,macos.rs,windows.rs}` | 1 | `Query.remote`；Linux 的 `Layout`；macOS 双栈的两行 |
| `crates/rurge/src/cli/runtime.rs` | 1 | 引擎的问题到平台的问题 |
| `crates/rurge-dns/src/fakeip/{mod.rs,file.rs}`（新）、`src/lib.rs` | 2 | 池：地址与名字、分配与回收、租约、隔离、文件、保存任务 |
| `crates/rurge-inbound/src/{session.rs,udp.rs,socks5.rs}` | 3 | `UdpClient::send` 的 `answers`、`UdpClient::refused` |
| `crates/rurge-engine/src/udp.rs` | 3 | `Routes::flow_for` 给出 `answers`；被拒的流调 `refused` |
| `crates/rurge-tun/src/tcp/{mod.rs,shard.rs,stream.rs}` | 4、5、6 | 没读的数据随结局留下；`TunStream::closed`；SYN 发往 fake 地址 |
| `crates/rurge-tun/src/lib.rs` | 4、5、6 | `TunOptions::check` 与新字段；`stop` 的说明；`Tun::start` 的新参数、`TunHandle::pool` |
| `crates/rurge-tun/src/session.rs` | 5、6 | 停止取消转发；REJECT-DROP 的拿着；fake 地址的会话 |
| `crates/rurge-tun/Cargo.toml`、`src/hooks.rs`（新） | 6 | `Hooks`、`TunSettings`、`Pinger` |
| `crates/rurge-tun/src/{packet.rs,dispatch.rs}` | 6、7、8 | UDP 与 ICMP 的校验和；`tcp_start` 改用 `wire`；UDP 与回显请求交给各自的去处 |
| `crates/rurge-tun/src/udp/{mod.rs,client.rs,targets.rs}`（新）、`src/fragment.rs`（新） | 7 | UDP 会话、目标表、回包与分片 |
| `crates/rurge-tun/src/icmp.rs`（新） | 7、8 | "管理上禁止"的构造；回显 |
| `crates/rurge-tun/src/testing/{hooks.rs,packets.rs}`（新）、`dialer.rs`、`app.rs` | 5 ～ 8 | 假 `Hooks` / 解析器 / `Pinger`；包的构造与读取；假 `Dialer` 的取消与 UDP；`AppStack::divert_udp` / `divert_icmp` |
| `crates/rurge-tun/src/tests/{options.rs,fake.rs,udp.rs,icmp.rs}`（新）、`support.rs`、`lifecycle.rs`、`outcomes.rs`、`filter.rs` | 4 ～ 8 | crate 里的单元用例 |
| 根 `Cargo.toml`、`crates/rurge-engine/Cargo.toml`、`src/runtime.rs`、`src/tun.rs`（新）、`src/observe.rs` | 9 | `TunSettings` 随每一代、`impl Hooks for Engine`、`Engine::start_tun`、按监听器的流量加 `Tun` |
| `crates/rurge-engine/tests/{tun.rs（新）,common/mod.rs,udp.rs}`、`crates/rurge-api/tests/api.rs` | 3、9 | 端到端用例 |
| CLAUDE.md、`docs/api/phase3.md`、M2 计划、M3a 计划、设计第 18 节、本计划末尾两张表 | 10 | 文档 |

模块的名字与拆法可以照实际调整，调了在报告里说。

---

## Task 1：`SessionInfo.local` 与进程查询的形状

**改哪里**

1. `crates/rurge-config/src/session.rs`：`SessionInfo` 在 `in_port` 之后加
   ```rust
   /// rurge's end of the client's connection: for the HTTP and SOCKS5 listeners the
   /// accepted connection's own local address (canonical; not the address bound, which
   /// may be unspecified), for a SOCKS5 UDP association its control connection's; for a
   /// TCP connection through the virtual adapter the endpoint the app connected to.
   /// `None` for the adapter's UDP sessions and rurge's own.
   pub local: Option<SocketAddr>,
   ```
   `SessionInfo::tcp` 与 `SessionInfo::udp` 给 `None`；工作区里写全字段的字面量（`git grep -n 'SessionInfo {'` 列出的，用 `..` 的不用改）照改，`crates/rurge-tun/src/tests/fixture.rs` 那一处给 `None`。
2. `crates/rurge-tun/src/session.rs`：TCP 会话的 `local` 是 `Some(tuple.dst)`（应用当初连的目的端点）。
3. `crates/rurge-inbound`：`listener.rs` 的 `canonical_peer` 改为 `pub(crate)`。HTTP：`serve` 的闭包里读这条连接的 `stream.local_addr().ok().map(canonical_peer)`，放进 `Ctx` 的新字段 `here: Option<SocketAddr>`；`session_for` 设 `local = ctx.here`（CONNECT 与绝对 URI 的转发都经它）。SOCKS5：同样读一次，CONNECT 的会话与 UDP 关联的模板都带它。
4. `crates/rurge-engine/src/owner.rs`：`OwnerQuery.remote_port: Option<u16>` 换成 `remote: Option<SocketAddr>`（TCP 的远端端点；地址是未指定地址时只比端口）；`looked_up` 换成
   ```rust
   /// The question for the process of `session`, or `None` when nobody is asked:
   /// HTTP and SOCKS5 (a SOCKS5 UDP template too: its control connection) and the
   /// adapter's TCP → TCP, local = `src`, remote = `local` (a listener's session without
   /// it: `0.0.0.0:<in_port>`); the adapter's UDP → UDP, local = `src`, no remote;
   /// rurge's own and forwarded sessions → `None`.
   pub(crate) fn query_for(session: &SessionInfo, since: Instant) -> Option<OwnerQuery>
   ```
   `Engine::look_up_owner` 的次序：没有 `owners` → `query_for` 答 `None` → `is_local` 答"不是本机"，任一成立都是"不知道"的槽，其余照旧起查找。查找的 DEBUG 行（`process lookup found the process`）的字段 `listen_port` 换成 `remote`（P23）。
5. `crates/rurge-platform/src/procinfo/mod.rs`：`Query.remote_port` 同样换成 `remote: Option<SocketAddr>`；`Snapshot::find` 里 TCP 的行要端口相等，且查询的地址是未指定地址、或与行的远端地址相等（两边都先 `to_canonical`）。`crates/rurge/src/cli/runtime.rs` 的 `platform_query` 照搬 `remote`。
6. `crates/rurge-platform/src/procinfo/linux.rs`（M2 延后第 18 行）：`parse_proc_net` 返回 `Result<Vec<Row>, ReadError>`——有数据行而一行也解析不出时是 `Err(ReadError::Layout)`（inode 为 0 的行，`TIME_WAIT` 等，解析得出、只是不成为行，算读到了；只有表头的表是空表）；`ReadError::Layout` 的注释照改。读取失败照 M2 的规矩是"不知道"、每种失败一生 WARN 一次（经已有的那一处，不新加 WARN 的代码）。
7. `crates/rurge-platform/src/procinfo/macos.rs`（M2 延后第 19 行）：`inp_vflag` 同时有 `INP_IPV4` 与 `INP_IPV6`（0x3）的记录出两行：照旧的 IPv4 视图一行，另加完整的 IPv6 一行；`row_of` 改为至多给两行。

**接口**

- 产出：`SessionInfo.local`（任务 6、7、9）；`OwnerQuery { transport, local, remote: Option<SocketAddr>, since }` 与 `query_for`（任务 9 的端到端用例看它）；`procinfo::Query.remote`。

**用例**

- `crates/rurge-engine/src/owner.rs`：`the_query_follows_the_listener_and_the_transport`（取代 `only_the_two_proxy_listeners_are_looked_up`）：HTTP 与 SOCKS5 的会话 → TCP、`remote == session.local`；没有 `local` 的监听器会话 → `0.0.0.0:<in_port>`；SOCKS5 的 UDP 模板（`transport` 是 UDP）→ TCP；`Tun` 的 TCP → TCP、`remote` 是原目的端点；`Tun` 的 UDP → UDP、`remote` 是 `None`；`Forward` 与 `Internal` → `None`。
- `crates/rurge-platform/src/procinfo/mod.rs`：`a_tcp_query_compares_the_far_address_too`：两行本地端点相同、远端端口相同而远端地址不同 → 带地址的查询找到对的那一行，未指定地址的查询找到第一行；IPv4 映射的远端地址与 IPv4 的查询相等。现有用例里的 `remote_port` 照改。
- `linux.rs`：`a_table_whose_lines_cannot_be_read_is_a_layout_error`、`time_wait_lines_are_read`（只有 inode 为 0 的行 → `Ok` 的空表）、`a_table_with_only_its_header_is_empty`。
- `macos.rs`：`inp_vflag_says_which_addresses_a_record_has` 的 0x3 一例改为期望两行（IPv4 视图与 IPv6）。
- `crates/rurge-inbound`：现有的 `a_session_names_the_client_and_the_listener_port`（HTTP 与 SOCKS5 各一）加断言 `local == Some(监听器的地址)`；SOCKS5 UDP 模板的用例（`socks5.rs` 里看模板的那一条）加 `local`；绝对 URI 转发的会话同样带 `local`。
- `crates/rurge-engine/tests/process.rs`：看查询的两处改为 `remote == Some(h.http())` / `Some(h.socks())`。
- `crates/rurge/src/cli/runtime.rs`：照搬的用例的取值改为 `Some("127.0.0.1:7890".parse().unwrap())` 与 `None`。
- `crates/rurge-tun`：看 `dialed()` 的会话的用例（`tests::tcp::` 里 v4 与 v6 的往返）加断言 `local == Some(目的端点)`。

**判据**：`git grep -n remote_port crates/` 什么也没有；用例绿；门禁绿。

## Task 2：fake-IP 池（`rurge_dns::fakeip`）

**改哪里**（`crates/rurge-dns/src/fakeip/`：`mod.rs` 地址、名字、池与租约，`file.rs` 文件的格式与读写；`src/lib.rs` 加 `pub mod fakeip;`）

1. 常数与纯函数（设计第 10.1 节、M3-D17）：
   ```rust
   pub const FAKE_COUNT: u32 = 130_814;
   pub const FAKE_TTL: Duration = Duration::from_secs(30);
   pub const QUARANTINE: Duration = Duration::from_secs(120);
   pub const QUARANTINE_COUNT: u32 = 2_048;
   pub const FILE_NAME: &str = "fake-ip.txt";
   pub fn v4_of(index: u32) -> Ipv4Addr;             // 198.18.1.1 + index
   pub fn v6_of(index: u32) -> Ipv6Addr;             // fd00:6152:: + the v4 address's bits
   pub fn index_of(ip: IpAddr) -> Option<u32>;       // only those two forms; IPv4-mapped is not one
   pub fn normalize(name: &str) -> Option<String>;   // one trailing dot off, ASCII lowercase,
                                                     // non-empty [a-z0-9_-] labels, at most 253 bytes
   ```
2. 池：
   ```rust
   pub enum AssignError { NotAName, Exhausted }
   pub enum Loaded { Missing, Clean, Unclean, NotRead(String) }   // the reason quotes nothing of the file
   pub struct FakeIpPool { /* one std::sync::Mutex */ }           // Debug: counts only
   pub struct Lease { /* the pool and the index */ }              // Drop gives it back; Debug: the index
   impl FakeIpPool {
       pub fn new() -> FakeIpPool;                                 // empty, every index, no file
       pub fn open(path: &Path, now: Instant) -> (Arc<FakeIpPool>, Loaded);   // blocking
       pub fn assign(&self, name: &str, now: Instant) -> Result<u32, AssignError>;
       pub fn name_of(&self, ip: IpAddr) -> Option<String>;        // refreshes
       pub fn lease(self: &Arc<Self>, ip: IpAddr) -> Option<(String, Lease)>;
       pub fn address_of(&self, name: &str, v6: bool) -> Option<IpAddr>;  // never assigns, never refreshes
       pub async fn keep_saved(self: Arc<Self>, path: PathBuf, every: Duration,
                               done: impl Future<Output = ()> + Send);
       #[cfg(any(test, feature = "testing"))] pub fn with_count(count: u32) -> FakeIpPool;
       #[cfg(any(test, feature = "testing"))]
       pub fn open_with(path: &Path, now: Instant, count: u32, quarantine: u32) -> (Arc<FakeIpPool>, Loaded);
   }
   ```
   里面：名字是 `Arc<str>`；编号 → 名字、名字 → 编号两张表；没有租约的用过的编号在一个 `LruCache<u32, ()>::unbounded()` 里（有租约的拿出来，最后一个租约放掉时以"最近用到"放回）；每个编号的租约数；游标与 `wrapped`；隔离的编号（先进先出）与隔离到的时刻；一个"映射变了"的记号与一个 `tokio::sync::Notify`（给保存任务）。`assign` 与 `name_of` 都刷新"最近用到"；`lease` 把编号移出 LRU，最后一个租约放掉时以"最近用到"放回——反查因此同样刷新（设计第 10.2 节）。这把锁是叶子：拿着它时不拿别的锁、不调用调用方给的任何东西。
3. 分配的次序（P6）：隔离期已过的编号（先进先出）→ 游标往后的新编号（到头时 `wrapped` 置真）→ LRU 里最久没用到的（丢掉它原来的名字）→ `Exhausted`。全部编号都有租约或在隔离里时就是耗尽。
4. 文件（设计第 10.3 节）：第一行 `rurge-fake-ip 1`；第二行 `cursor <n> wrapped <0|1> clean <0|1>`；其余每行 `<编号> <名字>`，没有租约的从最久没用到的到最近的在前、有租约的在最后。读的时候：第一行恰好如此；第二行合法、`cursor` 不大于编号总数；编号小于总数、没回绕时小于 `cursor`；编号与名字都不重复；名字是 `normalize` 过的样子；末尾至多一个空行；不是 UTF-8 的也是读不懂。
5. `open`（P4）：

   | 文件 | 结果 | 写 |
   | ---- | ---- | -- |
   | 没有 | 空表，不隔离，`Missing` | 立刻写一份空表的 `clean 0` |
   | 读不了、读不懂 | 空表，隔离从 0 起的 `QUARANTINE_COUNT` 个，`NotRead(原因)` | 不写；文件留到下一次写 |
   | `clean 1` | 载入，`Clean` | 立刻改写成 `clean 0` |
   | `clean 0` | 载入，隔离分配器接下来会交出的 `QUARANTINE_COUNT` 个（照第 3 点的次序数：游标往后的，到头之后 LRU 里最久没用到的，丢掉它们的名字），`Unclean` | 立刻改写成 `clean 0` |

   隔离的编号从分配器那里拿走：游标往后的，游标前进到它们之后（到头时 `wrapped` 置真）；LRU 里的，名字丢掉、编号移出 LRU。隔离到 `now + QUARANTINE` 为止，之后它们排在分配次序的最前（第 3 点）；隔离里的编号没有名字，`name_of`、`lease` 都答 `None`。`open` 在调用者的线程上阻塞地读写（引擎在阻塞线程上调它），不写日志——由调用方照 `Loaded` 写（P24）。
6. `keep_saved`：映射或游标有变化（分配、回收）时至多每 `every` 写一次，`done` 完成时写最后一次（`clean 1`）然后返回；只是"最近用到"变了不写。在锁里只取快照，锁外拼文本，在阻塞线程上先写 `fake-ip.txt.tmp` 再改名（同 `StateStore::write_atomic`；需要时建目录；不 fsync）。写失败一行 WARN，文本恰好是 `fake-ip: the addresses could not be saved`，字段 `error`（不带名字）；下一次变化再试。

**接口**

- 产出：上面的全部（任务 6 ～ 9）；`Lease` 随 TCP 连接与 UDP 目标的一生。

**用例**（`cargo test -j 8 --workspace --lib -- fakeip`；文件都在 `tempfile` 的目录里）

- `addresses_and_indices_map_both_ways`：`v4_of(0)` 是 `198.18.1.1`、`v4_of(130_813)` 是 `198.19.255.254`、`v6_of(0)` 是 `fd00:6152::c612:101`；`index_of` 两种都认回去；`198.18.0.1`、`198.19.255.255`、`::ffff:198.18.1.1`、`fd00:6152::1` 都是 `None`。
- `names_are_normalized`：`Example.COM.` → `example.com`；`a_b-c.test`、`localhost` 照收；空、`.`、`a..b`、`ä.test`、254 字节的名字是 `None`，253 字节的照收。
- `a_name_keeps_its_address`：同一个名字（大小写与末尾的点不同）再 `assign` 得同一个编号。
- `the_cursor_comes_first_then_the_least_recently_used`（`with_count(3)`）：a、b、c 得 0、1、2；`name_of(v4_of(0))` 刷新 a；d 得 1（b 的），`address_of("b.test", false)` 是 `None`。
- `a_leased_address_is_never_recycled`（`with_count(2)`）：a 得 0 并拿着它的租约；再分配 20 个名字都得 1，其间 `name_of(v4_of(0))` 总是 a；放掉租约之后再分配两个名字，0 号被回收。
- `every_address_leased_is_exhausted`（`with_count(2)`）：两个编号都有租约 → `Exhausted`；放掉一个之后照常分配。
- `the_file_round_trips`（`open_with(.., 4, 0)`）：a、b、c、d 之后刷新 a；`done` 之后文件是 `clean 1`；重开得 `Clean`，各名字的地址照旧，下一个新名字回收 b 的编号（最久没用到的那个仍是 b）。
- `the_file_is_written_only_when_mappings_change`（`every` 20 毫秒）：保存任务起来之后删掉文件，等 200 毫秒（"什么都没发生"的等待：没有变化，`every` 的十倍）文件仍不在；`assign` 一个新名字 → 有界时间里文件回来、带着它；只调 `name_of` 不让文件回来。
- `a_bad_file_gives_an_empty_pool_and_is_left_alone`：第 4 点每一种读不懂各一份文件（第一行不对、第二行不对、`cursor` 越界、编号越界、没回绕而编号不小于 `cursor`、编号重复、名字重复、名字不是规范化的样子、不是 UTF-8）→ `NotRead`，原因里没有文件里的名字，`open` 之后文件逐字节不变；`open_with(.., 16, 4)` 的第一个名字得 4，`now + 121 s` 时下一个得 0。
- `an_unclean_stop_quarantines_the_next_2048`（评审要点 4；`open_with(.., 16, 4)`）：（a）`cursor 5`、没回绕、有 5 个名字 → 新名字依次得 9、10……，`now + 121 s` 之后先得 5 ～ 8；（b）回绕了、16 个名字 → 最久没用到的 4 个名字没有了（`address_of` 与 `name_of` 都答不出），120 秒之内新名字得的是第 5 久的编号；（c）`cursor 14`、没回绕 → 隔离 14、15 与 LRU 里最久的 2 个。
- `the_real_quarantine_is_2048_for_120_seconds`（`open`，一份 `cursor 0`、`clean 0` 的空表）：第一个名字得 2048；`now + 119 s` 时下一个得 2049；`now + 121 s` 时下一个得 0。
- `a_crash_before_the_first_save_is_still_unclean`（评审要点 4）：没有文件时 `open` 之后文件立刻在、是 `clean 0`；分配几个名字后丢掉池（不经 `keep_saved`）、重开 → `Unclean`，第一个名字得 2048。另一份 `clean 1` 的文件 `open` 之后已改写成 `clean 0`。
- `a_stop_writes_clean`：`keep_saved` 的 `done` 完成 → 它返回，文件第二行是 `clean 1`。
- `a_failed_save_warns_without_names`（`LogCapture` 照 `rurge-dns` 已有的写法；路径指到一个文件下面，写不了）：`assign` 之后有界时间里 WARN 恰好一行、文本是上面那句、不带名字。
- `quarantined_addresses_are_unknown`：隔离里的编号的两个地址 `name_of` 与 `lease` 都答 `None`。
- `address_of_never_assigns`：不认识的名字答 `None`、池里的名字数不变；`with_count(2)` 里 a、b 之后 `address_of("a.test", false)`，新名字 c 仍回收 a 的编号（没有刷新）。
- `the_debug_of_a_pool_shows_no_names`。

**判据**：用例绿（计时的循环跑二十次）；门禁绿。

## Task 3：引擎的 UDP——回包应答哪个目标、被拒的流

**改哪里**

1. `crates/rurge-inbound/src/session.rs`：
   ```rust
   pub trait UdpClient: Send + Sync {
       fn recv<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>>;
       /// Sends `payload` to the client as a datagram from `from`. `answers`: the target of
       /// the flow this datagram answers, as the client named it, when that is known.
       fn send<'a>(&'a self, payload: &'a [u8], from: &'a Target, answers: Option<&'a Target>)
           -> BoxFuture<'a, io::Result<()>>;
       /// The engine refused the flow to `target` (a rule, `block-quic`, a policy that
       /// carries no UDP): called for each of its datagrams that is dropped.
       fn refused(&self, target: &Target, kind: RejectKind) {}
   }
   ```
   SOCKS5 的 `UdpClient` 不看 `answers`、不实现 `refused`（照旧静默丢弃）；`rurge-inbound` 用例里的 `UdpEcho` 与引擎 `udp.rs` 用例里的 `Hears`、`NoClient` 照新签名改（`Hears` 记下 `answers` 与 `refused`）。
2. `crates/rurge-engine/src/udp.rs`：`Routes::flow_for` 返回 `Option<(Arc<Flow>, bool)>`，第二项是"这一条流就是它应答的"（P7）：载体发往的目标恰好等于 `from` 的活着的流恰有一条 → 它、真；不止一条 → 最旧的、假；否则同端口的域名流恰有一条 → 它、真；不止一条 → 最旧的、假；否则最旧的任何一条、假。死了的流不算。`receive` 在真时把那条流的目标（`Target::new(会话的 dst_host, dst_port)`）作为 `answers` 交给 `client.send`。
3. 被拒的流：`run_flow` 里 `open` 答 `Err(SessionOutcome::Rejected(kind))` 时，先对第一个数据报调 `association.client.refused(&to, kind)`，`drain` 对之后丢掉的每一个数据报也调（`drain` 多收一个 `Option<(&Target, RejectKind)>`）；`Failed`（拨不通、暂停）不调（P8）。

**接口**

- 产出：`UdpClient::send(.., answers)`、`UdpClient::refused`（任务 7 的虚拟网卡一侧实现它们）。

**用例**

- `crates/rurge-engine/src/udp.rs`：`which_flow_a_reply_answers`（`Routes` 的纯用例）：第 2 点的每一种情形一例，含两条键相同的流（DIRECT 把两个名字解析到同一个地址）、两条同端口的域名流、死了的流被跳过。
- `crates/rurge-engine/tests/udp.rs`：一个记下 `send` 的 `answers` 与 `refused` 的 `UdpClient`（写在这个文件里），经 `h.engine.associate(..)` 直接用它：
  - `a_reply_says_which_flow_it_answers`：DIRECT 发往 `udp_echo_server` 的名字目标（`target.test`）与 IP 目标 → 回包的 `answers` 是那个目标照写的样子；经 `FakeSocks5`（`udp_names_to: Some(127.0.0.1)`）发往一个名字 → `answers` 是那个名字；同一个端口上两个名字 → `answers` 是 `None`。
  - `a_refused_flow_tells_the_client_for_every_datagram`：规则 `DOMAIN,rej.test,REJECT` → 两个数据报 → 两次 `refused(rej.test:<端口>, Reject)`；`REJECT-DROP`、`REJECT-NO-DROP`、`REJECT-TINYGIF` 各得各的种类；`block-quic = all` 下发往 443 的 QUIC Initial → `Reject`；不载 UDP 的策略（`http` 代理）在缺省的 `udp-policy-not-supported-behaviour` 下 → `Reject`；拨不通的流（`socks5` 指向关着的端口）与暂停（`Engine::suspend`）→ 一次也不调。
- 现有的 SOCKS5 UDP 用例照旧绿。

**判据**：用例绿；门禁绿。

## Task 4：TIME-WAIT 里没读的数据（I1）与 `TunOptions` 的校验

**改哪里**（都在 `crates/rurge-tun`）

1. `src/tcp/mod.rs`：`SharedInner` 加 `leftover: Option<Leftover>`：
   ```rust
   /// What the app sent that the stream had not read when the shard let the
   /// connection go: the stream reads it before the outcome (M3a deferred row 8).
   pub(crate) struct Leftover {
       bytes: Vec<u8>,
       read: usize,
       /// The receive buffer's share of the budget, given back when this goes.
       share: Sizes,
       budget: Arc<Budget>,
   }
   ```
   `Drop for Leftover` 把 `share` 还给 `budget`。`ConnShared` 加 `keep(&self, bytes: Vec<u8>, share: Sizes, budget: Arc<Budget>)`、`read_leftover(&self, buf: &mut ReadBuf<'_>) -> bool`（拷出放得下的字节、答 `true`；拷完最后一个字节的那一次把 `Leftover` 拿出来、在 `SharedInner` 的锁之外丢掉；没有时答 `false`）与 `discard_leftover(&self)`（同样在锁外丢掉）。
2. `src/tcp/shard.rs` 的 `reap`（P26）：TIME-WAIT 过了 `TIME_WAIT_GRACE` 而 `unread`，或 `Closed`、没有远端端点而 `unread` 时，不再留着连接：在片的锁里用 `recv_slice` 把接收缓冲里的字节全部拷进一个 `Vec`，再以 `recv_slice(&mut [])` 定结局（`Err(Finished)` → `Closed`，其余 → `Reset`），把接收缓冲的份额（`Sizes { rx: conn.sizes.rx, tx: 0 }`）连同字节交给 `conn.shared.keep(..)`、`conn.sizes.rx` 置 0（`remove` 只还发送缓冲的份额），然后 `remove(id, 结局)`。套接字、`by_tuple` 的项与守卫的边缘随之放掉：同一个四元组上新的 SYN 照常建新连接（M3a 延后第 8 行的后半）。`reap` 的说明照改。
3. `src/tcp/stream.rs`：`poll_read` 找不到连接时先 `shared.read_leftover(buf)`，答 `true` 就 `Ready(Ok(()))`，否则照旧按结局答；`Drop` 先 `shared.discard_leftover()`。写与 `shutdown` 不变（结局是 `Closed` 时写是 broken pipe）。
4. `src/lib.rs`（P27）：
   ```rust
   impl TunOptions {
       /// Whether the options can run a TUN: the first that cannot, in the order of the fields.
       pub fn check(&self) -> Result<(), String>;
   }
   ```
   文本恰好是：`shards must be more than zero`、`buffer_min must be more than zero`、`tx_max must be more than zero`、`keep_alive must be at least 1 ms and at most 24 hours`、`timeout must be more than keep_alive and at most 24 hours`、`shard_queue must be more than zero`、`writer_queue must be more than zero`（按字段的次序查，报第一个）。`budget`、`max_sockets`、`max_half_open` 可以是 0；`half_open_timeout`、`linger`、`drop_hold` 已是全函数（M3a），不查。`Tun::start` 开头 `if let Err(error) = options.check() { panic!("the TUN's options: {error}") }`，去掉 `writer_queue`、`shards`、`shard_queue` 的 `.max(1)`；`# Panics` 一节补上这一条。`lone_shard` 照旧不经它。

**接口**

- 产出：`TunOptions::check`（任务 6 加五个字段的检查；M4 的可执行文件在起 TUN 之前调它）。`ConnShared` 的三个新方法只在 `tcp` 里用。

**用例**

- `src/tests/lifecycle.rs`（片一级，照 `ByHand` 与合成时间）：
  - `unread_bytes_outlive_the_time_wait`：流先 `shutdown`，`turn` 送出 FIN，应用确认它；应用发 `abc` 再发它的 FIN（套接字到 TIME-WAIT）；流不读；`turn` 到 FIN 之后 30 毫秒 → 片已放掉这条连接（`state_of` 是 `None`、`socket_count` 0、`records()` 是缺省值）、结局 `Closed`、`lone.budget.used()` 大于 0（接收缓冲的份额还留着）；再 `turn` 到第一次 `turn` 之后 10.1 秒（越过 smoltcp 的 `CLOSE_DELAY`）；然后读到 `abc`、再两次读到 `Ok(0)`，之后 `lone.budget.used()` 是 0；同一个应用端口到同一个目的的新 SYN 建出一条新连接（`opened` 恰有一项）。
  - `a_dropped_stream_gives_back_what_it_kept`：同上到"片已放掉这条连接"，丢掉流（不读）→ `lone.budget.used()` 是 0。
  - `bytes_unread_at_the_timeout_are_read_and_then_a_reset`（`timeout: 100 ms`）：应用发 `abc`（不发 FIN），流不读；`turn` 越过超时（smoltcp 发出 RST），再 `turn` 到下一次看全部连接（`SWEEP` 之后）→ 片已放掉这条连接、结局 `Reset`；读到 `abc`，再读是 `ConnectionReset`；之后 `lone.budget.used()` 是 0。
  - 改：`a_stream_answers_by_how_its_connection_ended` 的第二段（应用端口 40001）——连接好好结束而流还没读完时，片当场放掉它：`state_of` 是 `None`、结局 `Closed`（此前是 `Some(Closed)` 与 `None`），写照旧是 broken pipe，读到 `abc` 再读到 `Ok(0)`；注释照改。
- `src/tests/options.rs`（新）：
  - `the_defaults_pass_the_check`。
  - `options_out_of_range_are_refused`：每一条文本一例（`keep_alive` 为 0 与 24 小时加 1 毫秒、`timeout` 等于 `keep_alive` 与 24 小时加 1 毫秒、五个"大于零"的字段为 0）→ `Err(那句文本)`；边界上的 `keep_alive: 1 ms` 与 `timeout: 24 h` → `Ok`。
  - `zero_budgets_and_limits_pass`：`budget`、`max_sockets`、`max_half_open` 为 0 → `Ok`。
  - `a_tun_with_bad_options_does_not_start`：`#[should_panic(expected = "the TUN's options: shards must be more than zero")]`，经 `support.rs` 起 TUN 的那一处。

**判据**：M3a 的用例照旧绿（点了名的那一段照改）；新用例绿；门禁绿。

## Task 5：停止结束卡在上游上的转发（I2）；REJECT-DROP 的拿着随连接结束

**改哪里**（都在 `crates/rurge-tun`）

1. `src/session.rs` 的 `serve`（M3a 延后第 12 行、设计第 18 节第 15 条）：拨通之后先取 `let token = dialed.handle.token().clone();`，把 `dialer.relay(..)` 的 future 钉住，与 `life.stopped()` 赛跑；停止先到时 `token.cancel()`，再把同一个转发 `await` 到底。不丢掉转发的 future（它要结束 `SessionHandle`，丢掉了请求记录会一直开着）；拨号不与停止赛跑（P29）。
2. `hold_in_silence`（M3a 延后第 16 行"留下的第二件"与第 17 行第五条，P30）：头一段的 `select!` 加一支 `stream.closed()`——片放掉了连接（应用复位了它，或片因别的原因放掉）就返回；读掉丢弃的那一段把 `tokio::io::copy(.., sink())` 换成在一块 512 字节的数组上循环读（`Ok(0)` 或出错就停），仍以 `linger` 为限。
3. `src/tcp/stream.rs`：`pub(crate) async fn closed(&self) -> Outcome`：片放掉连接时完成，答结局；经 `ConnShared::poll_outcome(&self, cx: &mut Context<'_>) -> Poll<Outcome>`（同一次加锁里看结局、没有就登记读的唤醒者；拿着期间没有人读，读的唤醒者是空着的）。
4. `src/lib.rs`：`TunHandle::stop` 的说明改写——拨通了的会话被取消：它的令牌被取消，转发不论在等什么都结束，请求记录照引擎的规矩记为完成；正在进行的拨号不被取消（P29）：它回来时流已被复位、转发当场结束，`join` 因此至多等到每个拨号自己的时限。`join` 的说明同改。加 `#[cfg(test)] pub(crate) fn session_tasks(&self) -> usize`：TUN 的任务里除去片、写任务与读设备的任务之后的个数（会话；任务 7、8 起还有 UDP 会话与回显）。
5. `src/testing/dialer.rs`：`FakeDialer::relay` 与 `handle.token().cancelled()` 赛跑：取消先到时记 `Err("cancelled")`、以 `SessionOutcome::Completed` 结束 handle（同引擎：令牌被取消的转发是完成）。这一处先改，用例才能在没改 `serve` 时变红。

**接口**

- 产出：`TunStream::closed`；`TunHandle::session_tasks`（任务 7、8 的用例用）；`FakeDialer` 的取消（任务 9 不用它，引擎的转发本来就看令牌）。

**用例**（`src/tests/outcomes.rs`）

- `stop_ends_a_relay_stuck_on_its_upstream`：`Script::Hold { buffer: 64 KiB }`；应用连上，一个任务往里写 1 MiB 并记下已写出的字节数；等到这个数不少于 64 KiB、且在 300 毫秒里没有变（"什么都没发生"的等待：远端不读，转发停在写上游上、应用一侧的缓冲满了；整体以 10 秒为界）；`stop()` → `join()` 5 秒内答 `Ending::Stopped`；`relay_ends()` 恰是 `[Err("cancelled")]`。没改 `serve` 时 `join` 5 秒内不结束（红）。
- `a_drop_hold_ends_with_its_connection`：`Script::Reject(RejectKind::Drop)`、`drop_hold: 30 s`；应用连上，等到 `session_tasks()` 是 1；应用 `abort()` → 5 秒内 `session_tasks()` 回到 0。没改时要等 30 秒（红）。
- 现有的停止用例与 REJECT-DROP 的用例（拿着再关、拿完读掉应用还在发的字节、`a_hold_or_a_linger_that_never_ends_ends_with_the_stop`）照旧绿。

**判据**：用例绿（计时的循环跑二十次）；门禁绿。

## Task 6：TUN 的钩子、池与发往 fake 地址的 TCP；UDP 与 ICMP 的校验和

**改哪里**（都在 `crates/rurge-tun`）

1. `Cargo.toml`：依赖 `rurge-dns.workspace = true`；dev-dependency `rurge-dns = { workspace = true, features = ["testing"] }`。
2. `src/hooks.rs`（新；在 crate 根重新导出）：
   ```rust
   /// What the TUN asks of whoever started it, when it needs it.
   pub trait Hooks: Send + Sync {
       /// The settings of the generation in use now (a cheap clone).
       fn settings(&self) -> Arc<TunSettings>;
   }
   /// One generation's settings for the TUN.
   pub struct TunSettings {
       /// Resolves a fake address's name for an echo request.
       pub resolver: Arc<dyn Resolve>,          // rurge_net::connector::Resolve
       /// `icmp-forwarding`: echo requests to addresses other than rurge's own are forwarded.
       pub icmp_forwarding: bool,
   }
   /// Sends echo requests for the TUN.
   pub trait Pinger: Send + Sync {
       /// One echo request to `dst`; resolves with the reply's payload.
       fn echo<'a>(&'a self, dst: IpAddr, payload: &'a [u8]) -> BoxFuture<'a, io::Result<Vec<u8>>>;
   }
   ```
3. `src/lib.rs`：
   ```rust
   pub fn start(device: Arc<dyn Device>, dialer: Arc<dyn Dialer>, hooks: Arc<dyn Hooks>,
                pinger: Arc<dyn Pinger>, pool: Option<Arc<FakeIpPool>>, options: TunOptions) -> TunHandle;
   impl TunHandle { pub fn pool(&self) -> Option<Arc<FakeIpPool>>; }
   ```
   `TunOptions` 加（缺省值来自设计 M3-D8 与第 7、8 节）：`udp_idle: Duration`（60 秒）、`udp_queue: usize`（256）、`max_udp_targets: usize`（1024）、`echo_timeout: Duration`（5 秒）、`max_echoes: usize`（1024），各带说明；`check` 接着查（文本恰好是 `udp_idle must be at least 1 ms and at most 24 hours`、`udp_queue must be more than zero`、`max_udp_targets must be more than zero`、`echo_timeout must be at least 1 ms and at most 24 hours`、`max_echoes must be more than zero`）。这一任务只把钩子、`Pinger` 与这五个字段接上，任务 7、8 使用。
4. TCP 发往 fake 地址（M3a 的 `ShardState::new_syn` 里 `Place::Fake` 的那一步）：`ShardState` 带着 `pool: Option<Arc<FakeIpPool>>`（`Shard::new` 收它，重建时照旧带着）；有池且 `pool.lease(目的地址)` 有答案 → 照 `Real` 往下走，`Opened` 多一项 `name: Option<(String, Lease)>`；没有池或池里没有 → 照 M3a 回 RST、计 `unknown_fake`。`session::serve` 多收这一项：有名字时会话的 `dst_host` 是 `HostName::Domain(名字)`，`local` 照旧是原目的端点；租约在 `serve` 里拿到它返回为止。池的锁是叶子（任务 2），在片的锁里拿它不会反过来。
5. `src/packet.rs` 的 `classify`：UDP 与 ICMP 的校验和在这里验（设计第 4.4 节、M3a P11）：UDP over IPv4 的校验和 0 是"没有"、照收；UDP over IPv6 的校验和 0 是 `malformed`（smoltcp 的 `verify_checksum` 对 0 答真，要自己拒）；其余不对的 UDP、ICMPv4、ICMPv6（带伪首部）都是 `malformed`。同一处（M3a 延后第 10 行，P31）：`tcp_start` 改用 smoltcp 的 `Ipv4Packet` / `Ipv6Packet` 读首部长度与协议，去掉常数 `IP_TCP` 与手写的偏移；结论不变（带扩展首部的 IPv6 不改写），现有的模糊用例与 `strip_sack_permitted` 的用例照旧绿。
6. `src/testing/hooks.rs`（新）：`FakeHooks`（`new(TunSettings) -> Arc<FakeHooks>`、`set(&self, TunSettings)`）、`FakeResolve`（`new() -> Arc<FakeResolve>`、`insert(&self, name: &str, addrs: Vec<IpAddr>)`；不认识的名字是 `NotFound`）、`FakePinger`（`new() -> Arc<FakePinger>`、`script(&self, dst: IpAddr, PingScript)`，`PingScript::{Echo, Fail, Silent}`，没写的目的地 `Fail`；`pinged(&self) -> Vec<(IpAddr, Vec<u8>)>`）。
7. `src/testing/dialer.rs`：`FakeDialer` 先按 `session.local` 找用例写给它的安排，没有 `local` 时照旧按 `dst_host` 与 `dst_port`（fake 地址的会话的 `dst_host` 是名字）。
8. `src/tests/support.rs`：起 TUN 照 M3a 只有一处调 `Tun::start`：`tun_dialing(device, dialer, extras: &Extras, options)`，`Extras { hooks: Arc<FakeHooks>, pinger: Arc<FakePinger>, pool: Option<Arc<FakeIpPool>> }`，`Extras::default()` 是 `FakeHooks` 带一个空的 `FakeResolve` 与 `icmp_forwarding: true`、没写过安排的 `FakePinger`、`Some(Arc::new(FakeIpPool::new()))`；`rig_extras(options, extras) -> (Rig, Extras)`，现有的 `rig`、`rig_on`、`rig_with`、`tun_on`、`tun_with` 传 `Extras::default()`。

**接口**

- 消费：任务 1 的 `local`、任务 2 的池、任务 4 的 `check`；M3a 的片、SYN 的去处与 `Stats::drop`。产出：`Hooks`、`TunSettings`、`Pinger`、新的 `Tun::start`、`TunHandle::pool`、`TunOptions` 的五个新字段、假的钩子与 `Pinger`、`Extras` 与 `rig_extras`（任务 7 ～ 9）。

**用例**（`src/tests/fake.rs`；校验和与选项的用例分别在 `src/tests/filter.rs` 与 `src/tests/options.rs`）

- `a_connection_to_a_fake_address_dials_its_name`：`i = pool.assign("a.test", Instant::now())` → 连 `v4_of(i):80` 与 `[v6_of(i)]:80` → `dialed()` 的 `dst_host` 是 `Domain("a.test")`、`local` 是 fake 端点；数据往返。
- `a_connection_holds_its_name_for_its_life`（评审要点 1；`with_count(2)` 的池）：连到 a 的 fake 地址、连接开着；再分配 20 个名字都得另一个编号，`name_of` 对 a 的地址总是 a；连接照常往返；连接结束、套接字拿掉之后再分配两个名字，a 的编号被回收。
- `an_address_the_pool_does_not_know_is_reset`：从没分配过的 fake 地址 → `ConnectionRefused`、`unknown_fake` 1、没有拨号；`pool: None` 时同样。
- `bad_udp_and_icmp_checksums_are_malformed`、`udp_over_ipv4_without_a_checksum_is_accepted`、`udp_over_ipv6_with_checksum_zero_is_malformed`（`classify` 的纯用例）。
- `options_out_of_range_are_refused` 加五个新字段各一例。

**判据**：M3a 的用例照旧绿；新用例绿；门禁绿。

## Task 7：UDP 会话

**改哪里**（`src/udp/`、`src/fragment.rs`、`src/icmp.rs`；`src/dispatch.rs`）

1. **去向**（P28）：`dispatch::route` 返回 `Option<(Parsed, Vec<u8>)>`——过了 `classify` 的 UDP 包与回显请求（截到 IP 包自己的长度）交回调用方，TCP 照旧进片、丢弃照旧计数；`dispatch::run` 带着一个上下文（会话表、池、钩子、`Pinger`、写任务的发送端、计数、`Life`、选项、任务表），把 UDP 交给 `udp`（回显请求在任务 8 之前照旧计 `unsupported`）。`filter.rs` 里直接调 `route` 的三处断言它答 `None`。收包的任务从不等待。
2. **会话**（设计第 7.1 节）：每个应用的源端点一个，在一张以源端点为键、`std::sync::Mutex` 护着的表里（收包的任务加、会话自己拿掉）。模板 `SessionInfo`：`src` 是源端点、`in_port` 0、`listener` `Tun`、`transport` UDP、`dst_host` 是协议栈自己那个地址族的地址（`198.18.0.253` / `fd00:6152::fd`）、`dst_port` 0、`local` `None`（P16）。会话的任务在 TUN 的 `TaskTracker` 上，跑 `dialer.associate(client, 模板, closed)`。
3. **收包的任务里**对一个 UDP 包依次：
   1. 目的是 `Own` 或 `Responder` → ICMP 管理上禁止（第 7 点）、不限频、不建会话（P9；应答器的 53 端口在 M3c 另有去处）。
   2. 找这个源端点的会话（已在结束的不算，P21）与它的目标表里这个目的端点的项。
   3. 没有项时定目标：`Fake` → `pool.lease`，没有池或池里没有 → 丢弃、计 `unknown_fake`（不建会话）；`Real` → `Target::new(HostName::Ip(目的地址), 端口)`。
   4. 没有会话时：`dialer.admit_udp()` 答 `Busy` → 丢弃、计 `busy`；答 `NotSupported` → 丢弃、计 `unsupported`；答 `Accepted` → 建会话（替换表里已在结束的那一个）。
   5. 没有项时加项：已有 `max_udp_targets` 项 → 丢弃、计 `busy`（租约随之放掉）。
   6. 项记下这个包的 IP 首部与 UDP 首部（合成 ICMP 时引用）与此刻。
   7. `(负载, 目标)` 进会话的有界队列（`udp_queue`），满了丢弃、计 `queue_full`（P22）。
4. **目标表**（`targets.rs`，P11）：原目的端点 → 目标、目标 → 项（原目的端点、最近一个包的首部、租约、两个方向最后一次有包的时刻），两个方向都是 O(1)。会话的任务至多每 `udp_idle` 的四分之一扫一遍：两个方向都过了 `udp_idle` 没有包的项拿掉（放掉租约）。
5. **`UdpClient` 的实现**（`client.rs`）：
   - `recv`：从队列（`tokio::sync::Mutex<mpsc::Receiver<_>>`，引擎只从一个任务调 `recv`）取下一个，拷进 `buf`，答 `(长度, 目标)`。
   - `send(payload, from, answers)`：回包的来源（设计第 7.2 节）——`answers` 在表里 → 那一项的原目的端点（并记下这一项的下行时刻）；`answers` 不在表里 → 是名字的经 `pool.address_of(名字, 会话是 v6)`（查不到丢弃、计 `unknown_fake`），是 IP 的照用；没有 `answers` → `from` 是 IP 的照用（地址族与会话不同的丢弃、计 `unsupported`，P10），是名字的同上经池正查。目的是会话的源端点。按第 6 点拼包并分片，交给写任务时**等**到队列收得下（这是引擎接收回包的任务，等待就是背压）。
   - `refused(target, kind)`：`Reject`、`TinyGif` → 经一个全 TUN 共用的 `Limiter::new(100, 3 s)`（M3a 的 `limit`），超出的不回、计 `icmp_limited`；`NoDrop` → 不限频；`Drop` → 什么也不回。回的是 ICMP 管理上禁止：来源是这一项的原目的地址，引用这一项最近一个包的首部；表里没有这个目标时什么也不回。交给写任务用 `try_send`，满了丢弃、计 `queue_full`。
6. **回包与分片**（`fragment.rs`，设计第 7.4 节、M3-D15）：`pub(crate) fn ip_packets(src: IpAddr, dst: IpAddr, protocol: IpProtocol, transport: &[u8], mtu: usize, ids: &Ids) -> Vec<Vec<u8>>`——传输层的部分（UDP 首部与校验和已算好）放得下一个包就一个；放不下时 IPv4 清掉 DF、偏移以 8 字节为单位、同一个标识、除最后一片外都置 MF；IPv6 加分片首部（下一个首部 17）、同样的规矩。TTL 与跳数 64，IPv4 首部的校验和照算。`Ids`：每个 TUN 一个计数器，起点随机，IPv4 取低 16 位、IPv6 取 32 位（P14）。
7. **ICMP 管理上禁止**（`icmp.rs`）：`pub(crate) fn prohibited(src: IpAddr, dst: IpAddr, quoted: &[u8]) -> Vec<u8>`——ICMPv4 type 3 code 13、ICMPv6 type 1 code 1，负载是引用的 IP 首部加 UDP 首部，校验和照算（ICMPv6 带伪首部）。
8. **会话的一生**：两个方向都 `udp_idle` 没有包（P20）、TUN 停止、`associate` 自己返回（引擎停了、刚好满了）三者之一 → 取消 `closed`、等 `associate` 返回、表里那一项仍是自己（`Arc::ptr_eq`）才拿掉，目标表随之放掉全部租约（P21）。
9. **夹具**：`src/testing/packets.rs`（新，公开给引擎的端到端用例）：
   ```rust
   pub fn udp(src: SocketAddr, dst: SocketAddr, payload: &[u8]) -> Vec<u8>;
   pub fn echo_request(src: IpAddr, dst: IpAddr, ident: u16, seq: u16, payload: &[u8]) -> Vec<u8>;
   pub struct UdpSeen { pub src: SocketAddr, pub dst: SocketAddr, pub payload: Vec<u8> }
   pub fn read_udp(packet: &[u8]) -> Option<UdpSeen>;          // checks both checksums
   pub enum IcmpSeen {
       EchoReply { src: IpAddr, dst: IpAddr, ident: u16, seq: u16, payload: Vec<u8> },
       Prohibited { src: IpAddr, dst: IpAddr, quoted: Vec<u8> },
   }
   pub fn read_icmp(packet: &[u8]) -> Option<IcmpSeen>;        // checks the checksums
   pub fn reassemble(fragments: &[Vec<u8>]) -> Option<Vec<u8>>; // the whole IP packet
   ```
   `AppStack::divert_udp(&self) -> mpsc::UnboundedReceiver<Vec<u8>>` 与 `divert_icmp(&self)`（同上）：从 TUN 来的每一个 UDP 包——分片也算（IPv4 协议号 17 的每一片，IPv6 分片首部后面是 17 的每一片）——与每一个 ICMP 包交给它，不进栈（P33：`divert(端口)` 只认第一片）。`FakeDialer` 的 UDP：`admit(&self, UdpAdmission)`（缺省 `Accepted`）、`script_udp(&self, Target, UdpScript)`（`UdpScript::{Echo, Reply(Vec<u8>), EchoFrom(Target), Refuse(RejectKind)}`：`Echo` 与 `Reply` 以那个目标为 `from` 与 `answers`，`EchoFrom` 以给的目标为 `from`、没有 `answers`；没写的目标只记下）、`associations(&self) -> Vec<SessionInfo>`（模板）、`associations_open(&self) -> usize`、`udp_sent(&self) -> Vec<(Target, Vec<u8>)>`、`hold_udp(&self)` / `release_udp(&self)`（扣着时不调 `recv`）、`send_to_app(&self, app: SocketAddr, from: Target, answers: Option<Target>, payload: Vec<u8>)`。
10. `packet.rs` 的 `Reason::IcmpLimited` 去掉 `expect(dead_code)`；`Counters::icmp_limited` 的说明改为"被拒的 UDP 流的 ICMP 被限频挡下的个数（3 秒 100 个）"，`unsupported` 的说明去掉"UDP 还没有人处理"。
11. 现有用例照改（点名）：`filter.rs` 的 `udp_and_icmp_are_not_handled_yet` 去掉 UDP 那一半（剩下回显请求的两个，`unsupported` 2；任务 8 整条去掉）；`each_reason_warns_once` 里当"没有人处理"的例子的两个 `udp_packet` 换成 `ipv4_packet(47, ..)` 与 `ipv6_packet(47, ..)`（GRE），期望不变（`unsupported` 2）。

**接口**

- 消费：任务 3 的 `UdpClient`、任务 5 的 `session_tasks`、任务 6 的钩子与池；M3a 的 `classify`、`space::place`、`Stats::drop`、`Limiter`、写任务。产出：`fragment::ip_packets` 与 `Ids`、`icmp::prohibited`（任务 8）；`testing::packets`、`divert_udp`、`divert_icmp`、`FakeDialer` 的 UDP（任务 8、9）。

**用例**（`src/tests/udp.rs`；应用一侧经 `AppStack::inject` 发 `packets::udp`、经 `divert_udp` / `divert_icmp` 收；数 ICMP 的用例把 `writer_queue` 设成不小于 400——收包的任务与 `refused` 合成的包一律 `try_send`，一阵 150 个不能因写队列满而少）

- `a_datagram_goes_out_and_its_answer_comes_back_v4` / `_v6`：发往 `192.0.2.10:7` / `[2001:db8::10]:7`（`Echo`）→ `udp_sent()` 有它；应用收到来源是那个端点、负载相同、两个校验和都对的回包；模板是第 2 点的样子。
- `a_datagram_to_a_fake_address_goes_to_its_name_and_comes_back_from_it`：发往 a 的 fake 地址的 53 端口 → 目标是 `a.test:53`；回包来自那个 fake 端点（v4 与 v6）。
- `a_third_party_reply_keeps_its_source`（评审要点 2）：`EchoFrom(192.0.2.99:3478)` → 回包的来源是 `192.0.2.99:3478`。
- `a_reply_from_a_name_comes_from_its_fake_address`：`send_to_app(.., from: b.test:53, answers: None, ..)`，b 分配过 → 来源是 b 的 fake 地址；没分配过的名字 → 丢弃、`unknown_fake` 1。
- `one_session_per_source_endpoint`：两个源端口 → 两个关联；同一个源端口发往两个目标 → 一个关联。
- `a_reply_larger_than_the_mtu_is_fragmented`（评审要点 3；MTU 1500）：`Reply(4000 个字节)` → 应用收到 3 片（IPv4：同一个标识、偏移是 8 的倍数、MF 只在最后一片没有、DF 清；IPv6：分片首部）→ `reassemble` 之后 UDP 的负载逐字节相同、校验和对；v4 与 v6 各一。
- `a_datagram_queue_holds_256`：`hold_udp()`；同一个源端点发往同一个目标 300 个 → `queue_full` 44；`release_udp()` 之后 `udp_sent()` 恰好 256 个。
- `busy_admission_drops_and_counts`：`admit(Busy)` → 每个包 `busy` 加一、没有关联；`admit(NotSupported)` → `unsupported`。
- `a_session_ends_after_the_idle_time`（`udp_idle: 300 ms`）：一个包之后静着 → 有界时间里 `associations_open()` 与 `session_tasks()` 回到 0；之后的包起一个新关联。
- `a_session_with_traffic_stays`（`udp_idle: 300 ms`）：只有下行（`send_to_app` 每 100 毫秒一个，持续 1 秒）→ 其间关联一直开着；停下之后关掉。
- `targets_are_at_most_max_udp_targets`（`max_udp_targets: 4`）：同一个源端点发往 5 个目标 → 第 5 个丢弃、`busy` 1。
- `refused_targets_answer_with_icmp`：`Refuse(Reject)` → 一个 ICMP 管理上禁止，来源是原目的地址（真实地址与 fake 地址各一），引用的是那个包的 IP 首部与 UDP 首部；`NoDrop`、`TinyGif` 同样；`Drop` → 200 毫秒里什么也没有（"什么都没发生"的等待）。
- `icmp_for_refused_flows_is_limited`（评审要点 5）：`Refuse(Reject)` 101 个包 → 100 个 ICMP、`icmp_limited` 1；之后 `Refuse(NoDrop)` 150 个包 → 150 个 ICMP、`icmp_limited` 仍是 1。
- `udp_to_rurge_addresses_is_prohibited_unlimited`：发往 `198.18.0.1:9`、`198.18.0.2:53`、`198.18.0.244:9`、`198.18.0.253:9`、`[fd00:6152::fd]:9` 各得一个 ICMP、没有关联；发往 `198.18.0.244:9` 150 个 → 150 个 ICMP。
- `udp_to_an_unknown_fake_address_is_dropped`：`unknown_fake` 1、没有关联、没有 ICMP。
- `a_session_releases_its_leases`（`with_count(1)` 的池、`udp_idle: 300 ms`）：发往 a 的 fake 地址 → 会话拿着租约，`assign("b.test", Instant::now())` 是 `Exhausted`；会话结束之后同样的 `assign` 得 0。
- `stop_ends_every_udp_session`：3 个会话 → `stop()` → `join()` 5 秒内答 `Stopped`，`associations_open()` 是 0。
- `fragments_reassemble_to_the_datagram`、`the_prohibited_message_quotes_the_header`：`ip_packets` 与 `prohibited` 的纯用例（用 `smoltcp::wire` 验长度、偏移、标志与校验和）。

**判据**：用例绿（计时的循环跑二十次）；门禁绿。

## Task 8：ICMP 回显

**改哪里**（`src/icmp.rs`；`src/dispatch.rs` 把交回的回显请求交给它）

1. 次序（设计第 8 节、M3-D16）：
   1. 这一代的 `icmp_forwarding`（`hooks.settings()`）为假 → 丢弃、计 `unsupported`。
   2. 目的是 `Own` 或 `Responder` → 本地应答（来源是那个地址，标识、序号、负载照请求）；在收包的任务里合成，交给写任务用 `try_send`，满了丢弃、计 `queue_full`。
   3. 在途的已有 `max_echoes` 个 → 丢弃、计 `busy`。
   4. `Fake` → `pool.name_of`（没有池或池里没有 → 丢弃、计 `unknown_fake`）→ 起一个任务：`settings.resolver.resolve(名字)`，先取与请求同一个地址族的地址、没有就取另一族的；解析不出或没有地址 → 丢弃、不计数、一行 DEBUG（P10）；有 → `pinger.echo(地址, 负载)`。
   5. `Real` → 起一个任务：`pinger.echo(目的地址, 负载)`。
2. `echo_timeout` 从起任务时算起（解析与回显合计）；到了、或 `Pinger` 出错 → 什么也不回（一行 DEBUG）；有应答 → 回显应答：来源是应用当初发往的地址（fake 地址也一样）、目的是应用、标识与序号照应用的、负载是 `Pinger` 给的；经 `fragment::ip_packets` 交给写任务（等到收得下）。任务在 TUN 的 `TaskTracker` 上，停止时随之结束。
3. `filter.rs` 的 `udp_and_icmp_are_not_handled_yet` 整条去掉。

**接口**

- 消费：任务 6 的 `Hooks` / `Pinger` 与池、任务 7 的 `ip_packets` 与去向。产出：回显的完整规矩（任务 9）。

**用例**（`src/tests/icmp.rs`；应用一侧经 `inject` 发 `packets::echo_request`、经 `divert_icmp` 收）

- `a_ping_to_a_real_address_goes_through_the_pinger_v4` / `_v6`：`Echo` → `pinged()` 有它；回显应答的来源是那个地址、标识与序号照应用的、负载相同、校验和对。
- `a_ping_to_a_fake_address_goes_to_its_names_address`：`FakeResolve` 给 a 一个 v4 地址 → `pinged()` 的目的是它、应答的来源是 fake 地址；只有另一族的地址时照样发往它。
- `a_ping_to_rurge_itself_is_answered_here`：`198.18.0.2`、`198.18.0.244`、`198.18.0.253`、`fd00:6152::2`、`fd00:6152::fd` 各得应答、`pinged()` 是空的。
- `icmp_forwarding_off_drops_echo_requests`：`FakeHooks::set` 成 `icmp_forwarding: false` 之后 → 没有应答、`unsupported` 加一、`pinged()` 不变（下一个请求就看新设置）。
- `an_unknown_fake_address_is_not_pinged`：`unknown_fake` 1。
- `a_name_that_does_not_resolve_is_not_pinged`：200 毫秒里没有应答（"什么都没发生"的等待）、没有计数。
- `a_silent_pinger_gives_up_after_the_timeout`（`echo_timeout: 300 ms`）：`Silent` → 没有应答；有界时间里 `session_tasks()` 回到 0（之后另一个请求照常得到应答）。
- `echoes_in_flight_are_capped`（`max_echoes: 2`）：3 个 `Silent` 的请求 → 第 3 个计 `busy`。
- `a_failing_pinger_answers_nothing`：`Fail` → 没有应答、没有计数。
- `stop_ends_the_echoes_in_flight`：2 个 `Silent` 的请求在途 → `stop()` → `join()` 5 秒内答 `Stopped`。

**判据**：用例绿（计时的循环跑二十次）；门禁绿。

## Task 9：引擎接入

**改哪里**

1. 根 `Cargo.toml`：`[workspace.dependencies]` 加 `rurge-tun = { path = "crates/rurge-tun" }`；`crates/rurge-engine/Cargo.toml`：依赖 `rurge-tun.workspace = true`，dev-dependency `rurge-tun = { workspace = true, features = ["testing"] }`。
2. `crates/rurge-engine/src/runtime.rs`：`Runtime` 加 `pub(crate) tun: Arc<TunSettings>`，`Runtime::build` 建好解析器之后建它（`resolver` 是 `stack.resolver` 转成 `Arc<dyn Resolve>`，`icmp_forwarding` 是 `config.general.icmp_forwarding`）。
3. `crates/rurge-engine/src/tun.rs`（新）：`impl rurge_tun::Hooks for Engine`（`settings()` 是 `self.runtime().tun.clone()`）与
   ```rust
   /// How often the pool's file is written at most while mappings change.
   const POOL_SAVE_EVERY: Duration = Duration::from_secs(10);
   impl Engine {
       /// Opens the fake-address pool in the data directory (on a blocking thread), starts a
       /// TUN on `device` with this engine as its dialer and hooks, and keeps the pool saved
       /// until the TUN has ended. The TUN stops when the engine stops accepting.
       pub async fn start_tun(self: &Arc<Self>, device: Arc<dyn Device>, pinger: Arc<dyn Pinger>,
                              options: TunOptions) -> TunHandle;
   }
   ```
   `# Panics` 同 `Tun::start`。池的文件是 `self.data_dir().join(rurge_dns::fakeip::FILE_NAME)`。照 `Loaded` 在调用者的线程上写日志（P24）：`NotRead(原因)` → 一行 WARN，文本恰好是 `fake-ip: the saved addresses could not be read; starting with none`，字段 `error`；`Unclean` → 一行 INFO，文本恰好是 `fake-ip: rurge did not stop cleanly last time; 2048 fake addresses are held back for 120 seconds`；其余不写。两个任务在引擎的 `TaskTracker` 上（P3）：保存任务（`keep_saved`，`done` 是 TUN 的 `join`）；另一个在引擎停止接受（`stop_accepting`）时 `stop()` 这个 TUN、TUN 先结束时就结束。
4. `crates/rurge-engine/src/observe.rs`：`TrafficStats` 加 `tun_up` / `tun_down`，`ListenerKind::Tun` 的会话计入；`by_listener()` 返回 `[(ListenerKind, u64, u64); 3]`（次序 Http、Socks5、Tun）；`GET /v1/traffic` 的 `listener` 因此多一个键 `tun`（`listener_name(Tun)` 已是 `"tun"`）。
5. `crates/rurge-engine/tests/common/mod.rs`：`tests/udp.rs` 的 `quic_initial()` 挪到这里（两个文件共用）。
6. 读一遍 `Engine::dial` 的每一条会等待的路（P29），在报告里列出各自的时限（规则里的进程查找、解析、`evaluate-before-use` 的等待、`CONNECT_TIMEOUT` 交给出站之后各协议的握手）；找到没有时限的路，报告，不在这一任务里改。

**接口**

- 消费：任务 1 ～ 8 的全部。产出：`Engine::start_tun`（M4 的可执行文件调用它）。

**用例**（`crates/rurge-engine/tests/tun.rs`；起 TUN 的辅助写在这个文件里：harness 照 `common` 建，`MemoryDevice::pair(1500, 4096)`，`AppStack::start` 接应用一端，`FakePinger` 交给 `start_tun`；规则照 P17 以 `SRC-IP,198.18.0.1/32,REJECT` 收尾（开 IPv6 的另加 `SRC-IP,fd00:6152::1/128,REJECT`），名字由规则显式交给 `DIRECT`；fake 地址取自 `tun.pool()`）

- `tcp_to_a_fake_address_reaches_its_name`：`DOMAIN,target.test,DIRECT`；连 `target.test` 的 fake 地址（v4 与 v6）的 echo 端口 → 数据往返；请求记录的 `host` 是 `target.test`、`listener` 是 `Tun`、`in_port` 0。
- `tcp_to_a_documentation_address_follows_the_rules`：`IP-CIDR,192.0.2.0/24,Proxy`，`Proxy` 是指向 `FakeSocks5 { connect_to: Some(echo) }` 的 `socks5` → 数据往返；`FakeSocks5` 记下的目标是 `192.0.2.10:80`。
- `a_rejected_tcp_connection_is_reset`：`DOMAIN,rej.test,REJECT` → `connect` 成功、第一次读是 `ConnectionReset`。
- `udp_to_a_fake_address_reaches_its_name_and_answers_from_it`：`udp_echo_server`；发往 `target.test` 的 fake 地址的那个端口 → 回包来自那个 fake 端点、负载相同。
- `a_rejected_udp_flow_answers_with_icmp`：`DOMAIN,rej.test,REJECT` → ICMP 管理上禁止、来源是 `rej.test` 的 fake 地址。
- `a_blocked_quic_flow_answers_with_icmp`：`block-quic = all`，`DOMAIN,target.test,DIRECT`；发往 fake 地址 443 的 QUIC Initial → ICMP 管理上禁止；记录的 `error` 是 `QUIC blocked`。
- `a_ping_to_a_fake_address_goes_to_the_names_address`：`FakePinger` 给 `127.0.0.1` 写 `Echo` → ping `target.test` 的 fake 地址 → `pinged()` 的目的是 `127.0.0.1`、应答来自 fake 地址；Profile 写 `icmp-forwarding = false` 重载之后 → 没有应答。
- `tun_sessions_are_looked_up_by_their_own_shape`：照 `tests/process.rs` 建带假 `OwnerLookup` 的 harness → TCP 会话的查询是 TCP、`local` 是 `198.18.0.1:<源端口>`、`remote` 是原目的端点；UDP 会话的查询是 UDP、`remote` 是 `None`；记录的 `pid` 是假查找给的。
- `tun_traffic_is_counted_under_its_listener`：一条 TCP 会话结束之后 `by_listener()` 的 `Tun` 两项都不为 0，`Http` 与 `Socks5` 不变。
- `stopping_the_tun_ends_a_relay_stuck_on_its_upstream`（I2 的端到端，设计第 18 节第 15 条）：`DOMAIN,sink.test,DIRECT`，`sink.test` 指到用例自己的一个回环监听器（接受连接之后既不读也不写）；应用连 `sink.test` 的 fake 地址，一直写，直到一次写在 500 毫秒里没有完成（"什么都没发生"的等待：上游不读，各处的缓冲都满了；以 64 MiB 为限，超过就让用例失败）；`tun.stop()` → `join()` 5 秒内答 `Stopped`；这条会话的请求记录以完成结束。
- `the_pool_is_saved_in_the_data_directory`：分配一个名字、`stop()`、`join()` → 有界时间里 `<数据目录>/fake-ip.txt` 的第二行是 `clean 1`、有这个名字。
- `the_engine_reopens_the_pool`：同一个数据目录上的第二个引擎 `start_tun` → `pool().address_of` 给同一个地址。
- `stopping_the_engine_stops_the_tun`：`stop_accepting()` → `join()` 5 秒内答 `Ending::Stopped`，有界时间里文件的第二行是 `clean 1`。
- `an_unreadable_pool_file_is_left_and_warned_once`（`LogCapture`，当前线程的运行时）：`fake-ip.txt` 写成别的东西 → WARN 恰好一行、不带文件的内容；`start_tun` 之后文件逐字节不变。
- `crates/rurge-engine/src/observe.rs` 的用例与 `crates/rurge-api/tests/api.rs` 的 `traffic_reports_totals_by_policy_and_listener` 加 `tun`（HTTP 的会话不计入它，`listener.tun.in` 与 `out` 是 0）。

**判据**：用例绿（计时的循环跑二十次）；门禁绿；报告里有第 6 点的清单。

## Task 10：文档

- CLAUDE.md：状态一段加 M3b 一段（做了什么；没做的与不要当成已经有的：应答器、预匹配、经监听器来的 fake 地址、真实的 `Pinger` 与真实设备都还没有，`rurge run` 不变，TUN 只在测试里起；`GET /v1/traffic` 多了 `listener.tun`、进程查找的 DEBUG 行字段 `listen_port` 换成 `remote`；M3a 里"必须知道的事"的第三、四件改写成做掉之后的样子）；「先读这些文档」加本计划；依赖方向加 `rurge-tun → rurge-dns` 与 `rurge-engine → rurge-tun`；常用命令加过滤词（`fakeip`、`tests::options::`、`tests::fake::`、`tests::udp::`、`tests::icmp::`、`--test tun`、`which_flow_a_reply_answers`、`a_refused_flow_tells`、`query_follows`、`compares_the_far_address`、`unread_bytes`、`stop_ends_a_relay`）。
- `docs/api/phase3.md`：M3b 一节（`GET /v1/traffic` 的 `listener.tun`；进程查找的 DEBUG 行的 `remote`；M2 一节里写 `listen_port` 的那一句照改；池的文件与它的三行日志——M4 起使用者才看得到）。
- 阶段 3 / M2 计划延后第 17 ～ 19 行的去向写"做掉（阶段 3 / M3b 任务 1，`<提交>`）"（那份文件是 CRLF）。
- M3a 计划延后第 7、8、10、12、16、17 行的去向照实写（第 8 行做掉：任务 4；第 10 行做掉：任务 6；第 12 行做掉：任务 5 与 9，拨号那一半见本计划延后第 8 行；第 16 行"留下的第二件"做掉：任务 5；第 17 行第一条做掉：任务 4，第五条做掉：任务 5；第 7 行移到 M3c）。
- 设计第 18 节加 M3b 与设计不同的几处（P2、P4、P11、P23、P26、P29）与第 15 条的了结。
- 本计划末尾两张表填齐。

**判据**：`git grep` 核对文档里引用的用例名、路径与常数都存在；门禁绿。

---

## 计划期决定

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| P1 | 计划写在 M3a 合并之后（`f3c09d2`）；任务的拆分 | 引用的 M3a 名字（`classify` / `Parsed` / `Reason`、`Stats::drop`、`space::{place, Place}`、`limit::Limiter`、`ShardState::{new_syn, reap, remove}`、`ConnShared`、`TunStream`、`session::{serve, hold_in_silence}`、`Life`、`TunHandle::{stop, join}`、`tests/support.rs` 的 `tun_dialing`）都对着代码核对过；名字与代码不同的照代码改本计划并记进执行期修正记录。拆分：先改调用方（任务 1、3）与池（任务 2），再做掉 M3a 交下来的两件（任务 4、5），再加 TUN 的新东西（任务 6 ～ 8），最后引擎（任务 9），每个任务各自能被评审拒绝 |
| P2 | `TunSettings` 的字段 | 只有解析器与 `icmp_forwarding`。设计第 3.3 节写的解析器是 `Arc<Resolver>`，这里是 `Arc<dyn Resolve>`（`rurge_net::connector::Resolve`；`rurge_dns::Resolver` 实现它）：`rurge-tun` 的用例用假解析器，不必建真的；`always-real-ip`、`allow-dns-svcb`、`hijack-dns`、规则模式与 `Hooks::pre_match` 随 M3c（应答器与预匹配才用到） |
| P3 | `Engine::start_tun` | `async`：打开池要读写文件（阻塞线程）。保存任务与"引擎停止接受时停 TUN"的任务都在引擎的 `TaskTracker` 上；保存任务在 TUN 的 `join` 之后写最后一次，所以等引擎的 tracker 就等到了 `clean 1` |
| P4 | 打开池时写什么 | `clean 1` 的文件载入后立刻改写成 `clean 0`、没有文件时立刻写一份空表的 `clean 0`——第一次保存之前崩溃的也认得出（评审要点 4）。读不了、读不懂的文件照设计第 10.3 节留着不动，但从空表开始时同样隔离从 0 起的 2048 个编号：那份文件里的映射可能还在应用手里（设计没有写这一处） |
| P5 | 测试的构造与时间 | `with_count` 与 `open_with` 只在 `cfg(any(test, feature = "testing"))` 下；池的时间全由调用方传 `now`（`assign`、`open`），用例不用暂停的时钟、也不等 120 秒 |
| P6 | 分配的次序与隔离的范围 | 隔离期已过的编号（先进先出）→ 游标 → LRU；"接下来会交出的 2048 个"照同一个次序数（游标往后的，到头之后 LRU 里最久没用到的，丢掉它们的名字），所以隔离不会漏掉下一次会交出的编号 |
| P7 | `answers` 的判据 | 引擎的 `Routes` 记的是载体实际发往的目标（DIRECT 已把名字解析成地址），"目标恰好相同"按它比；同一个键或同一个端口上不止一条流时不给 `answers`，以免把回包写成另一个目标的来源（设计第 7.2 节"恰好只有一条"）。限制：一个应用的套接字在同一个端口上同时对两个名字收发、出站又是全锥的载体时，回包的来源可能是另一个名字的 fake 地址，或不是 fake 地址（延后事项第 2 行） |
| P8 | 哪些流调 `refused` | `REJECT` 一族的每一种都调（含 `REJECT-DROP`），怎样回由虚拟网卡定；只对 `Rejected` 调，`Failed`（拨不通、暂停）不调：那些流 5 秒后重试，回 ICMP 会让应用以为目的地永远不通 |
| P9 | 发往 rurge 自己地址的 UDP | `Own` 与 `Responder` 都回 ICMP 管理上禁止、不限频、不建会话（设计 M3-D24 写了 `.244`，其余自己的地址同样对待）；应答器的 53 端口在 M3c 另有去处 |
| P10 | 计数的归属 | `admit_udp` 答 `NotSupported` 计 `unsupported`；`icmp-forwarding` 为假时的回显请求计 `unsupported`（设计第 8 节只写"丢弃"；它与"不是回显请求的 ICMP"同属"rurge 不处理的包"）；回包来源的地址族与会话不同计 `unsupported`；写任务的队列满时 ICMP 丢弃计 `queue_full`；回显请求的名字解析不出不计数（设计第 8 节"丢弃"，一行 DEBUG） |
| P11 | 目标表 | 每个会话一张，两个方向 O(1)；至多 `max_udp_targets`（1024，与引擎每个关联的流数相同）；两个方向都过了 `udp_idle` 没有包的项拿掉、放掉租约。设计第 10.2 节写目标的租约"在会话的一生里"，这里提前放：引擎的流同样在空闲 60 秒后回收，一个长寿的会话若一直攒着项会在 1024 处卡住 |
| P12 | 丢包的写法 | 一律经 M3a 的 `Stats::drop`：每种原因第一次 WARN（不带地址）、其余 DEBUG（带地址） |
| P13 | 会话的日志行（设计第 17 节 V12） | 引擎今天对 `Rejected` 与 `Failed` 的会话写 INFO 并带 `dst` 与 `src`（`crates/rurge-engine/src/engine.rs` 的 `log_session`），与设计第 14 节"名字与地址只在 DEBUG"不一致；照 V12 以今天的会话日志为准、不改，经虚拟网卡来的被拒会话的 INFO 行因此带名字（延后事项第 1 行） |
| P14 | 分片的标识 | 每个 TUN 一个计数器，起点随机（`getrandom`），IPv4 取低 16 位、IPv6 取 32 位：不同的回包用不同的标识，应用的内核才不会把两个数据报的分片拼在一起 |
| P15 | 没有 `local` 的监听器会话 | 查询的远端退回 `0.0.0.0:<in_port>`，与 M2 的"只比端口"相同；任务 1 之后监听器的会话总有 `local`，退回只为自己构造的 `SessionInfo` |
| P16 | UDP 会话模板的目的 | 协议栈自己那个地址族的地址、端口 0：模板要有一个目的，每条流复制它再换成自己的；用 rurge 自己的地址，规则不会因模板命中别的东西 |
| P17 | 端到端用例的两道防线（设计第 15.2 节） | 规则以 `SRC-IP,198.18.0.1/32,REJECT` 收尾（开 IPv6 的另加 `SRC-IP,fd00:6152::1/128,REJECT`），在 harness 追加的 `FINAL,DIRECT` 之前；名字经 `TunHandle::pool()` 取 fake 地址、由 MockDns 或 `[Host]` 钉到 `127.0.0.1`，规则显式交给 `DIRECT`；真实 IP 的 TCP 只用文档地址、经 `FakeSocks5 { connect_to: Some(回环上的 echo) }`；发往真实 IP 的 UDP 只交给 `REJECT` 一族；ICMP 只经假 `Pinger` |
| P18 | smoltcp 0.12.0 与 lru 0.18.4 的核对 | `wire::{Ipv6FragmentHeader, Ipv6FragmentRepr}`（`wire/mod.rs` 第 106、220 行，只要 `proto-ipv6`）、`Icmpv4DstUnreachable`（第 232 行，`CommProhibited` = 13）、`Icmpv6DstUnreachable`（第 242 行，`AdminProhibit` = 1）；`UdpPacket::verify_checksum` 对校验和 0 在两个地址族都答真；`fill_checksum` 在和为 0 时写 0xffff；`Ipv4Packet::set_frag_offset` 收字节数；IPv6 分片首部的偏移以 8 字节为单位。`lru`：`unbounded`、`put`、`push`、`pop_lru`、`peek_lru`、`promote`、`contains`；`iter()` 从最近用到的到最久的 |
| P19 | 设计第 17 节 V4 ～ V7、V9、V10 的核对 | `SessionInfo` 写全字段的字面量在 `rurge-config` 的 `tcp` / `udp`、`rurge-tun` 的 `session.rs` 与 `tests/fixture.rs`；`OwnerQuery` 唯一的构造点在 `Engine::look_up_owner`、`procinfo::Query` 的在 `runtime.rs` 与各平台的用例；`UdpClient` 的实现是 `Socks5UdpClient` 与引擎 `udp.rs` 用例里的 `Hears`、`NoClient`，`rurge-inbound` 的 `UdpEcho` 调 `send`；`Routes::flow_for` 只有 `receive` 调；`Rejected` 只出自 `open`（规则的 REJECT 一族、`block-quic`、不载 UDP 的策略），`drain` 同时服务 `Failed`；`Engine::data_dir()` 是资源管理器的根，即数据目录；按监听器的流量是定长的 `[_; 2]`，`GET /v1/traffic` 照它出键；V8 是 M3c 的；V9：UDP 的 `REJECT` 照旧不计入 `Escalation` |
| P20 | 会话的空闲 | 两个方向都 `udp_idle` 没有包才结束：只有下行的会话（服务器推送、应用只收不发）不结束 |
| P21 | 表里的会话在结束 | 新来的包建一个新会话替换它；结束的会话只在表里那一项仍是自己（`Arc::ptr_eq`）时拿掉 |
| P22 | 会话的队列项 | `(Vec<u8>, Target)`：目标在收包的任务里定（fake 地址在那里换成名字、拿租约），`recv` 只搬运 |
| P23 | `query_for` 取代 `looked_up` | 查不查与怎样查是同一个决定；`Tun` 改为查（设计第 9.2 节）。查找的 DEBUG 行字段 `listen_port` 换成 `remote`：虚拟网卡的会话的远端不是监听端口 |
| P24 | 池的 `open` 与日志 | `open` 返回 `Loaded`、自己不写日志；引擎在调用 `start_tun` 的那个线程上写（`LogCapture` 抓得到）。保存失败的 WARN 由 `keep_saved` 自己写（不带名字） |
| P25 | 监听器绑在未指定地址上 | `rurge-inbound` 的用例只绑回环，所以"会话带的是被接受的那条连接的本地地址、不是绑定的 `0.0.0.0`"只由读代码与回环上的用例保证（延后事项第 6 行） |
| P26 | TIME-WAIT 里没读的数据（M3a 延后第 8 行） | 照那一行写的形状：片放掉连接时把没读的字节拷进连接与流共用的记录（`ConnShared` 的 `Leftover`），接收缓冲的预算份额留到读完或流被丢掉再还，套接字与四元组立刻放掉；读先从记录里答，答完是结局。两种情形一并这样办：TIME-WAIT 过了宽限（不等 smoltcp 的 10 秒清空）、`Closed` 而没有远端端点（应用先关、rurge 后关，或 smoltcp 自己的超时）；结局取 `recv_slice(&mut [])` 的回答，收到过 FIN 的是 `Closed`、没有的是 `Reset`，所以超时的连接读完字节之后照旧是复位，不变成干净的 EOF。不只修结局：只把结局改成 `Closed` 会把看得见的丢失变成悄悄的截断 |
| P27 | `TunOptions` 的校验（M3a 延后第 17 行第一条） | `check` 是公开的，M4 的可执行文件在起 TUN 之前调它、把错误交给使用者；`Tun::start` 照 MTU 的写法在选项不对时 panic（`# Panics` 写明），不再悄悄地夹住。24 小时的上限远在 smoltcp 的换算会溢出的地方之下，又远大于任何合理的设置；`budget`、`max_sockets`、`max_half_open` 为 0 是有意义的设置（全用最小缓冲、拒绝一切 TCP、不许半开），照收 |
| P28 | 收包的任务怎样分去 UDP 与回显 | `route` 只管 `classify`、TCP 进片与丢弃的计数，把过了检查的 UDP 包与回显请求交回 `run`；`run` 带着上下文分给 `udp` 与 `icmp`。`route` 的纯用例因此不必建会话表 |
| P29 | 拨号不与停止赛跑 | 拨号进行中会话的令牌还在引擎手里（`Engine::dial` 里才建 `SessionHandle`），TUN 够不到；丢掉拨号的 future 会留下开着的请求记录。拨号的每一条会等待的路都有引擎自己的时限（任务 9 第 6 点列出），停止之后 `join` 至多等到它们到点；一个永不回来的拨号仍让 `join` 一直等（延后事项第 8 行） |
| P30 | REJECT-DROP 的两件（M3a 延后第 16 行"留下的第二件"、第 17 行第五条） | 两件都写着"M4 之前 / 与对方一起"，而 M3b 的任务 5 本来就要动 `session.rs`：一并做掉。拿着与连接的结束赛跑，读掉丢弃改用 512 字节的数组（这条路上的 future 是装箱的，数组只在走这条路的会话上占地方） |
| P31 | M3a 延后第 10 行（读 IP 首部的两种写法） | 去向是"下次动到 `packet.rs` 时"，任务 6 动它：`tcp_start` 改用 `wire`。只换读法、不改结论，现有的用例钉住 |
| P32 | M3a 延后第 7 行（拆 `bench.rs`） | 去向是"加基准之前"；M3b 不加基准，移到 M3c（M3c 计划在加任何基准之前先拆） |
| P33 | 应用一侧怎样收 UDP 与 ICMP | `AppStack::divert(端口)` 靠第一片里的端口认包，后面的分片认不出、会进应用的栈被丢掉；新加 `divert_udp` 与 `divert_icmp` 按协议认（IPv6 看分片首部后面的协议），分片的用例才收得齐 |

## 执行期修正记录

| # | 任务 | 修正 | 提交 |
| - | ---- | ---- | ---- |
| 1 | 1 | DEBUG 查找行的字段 `listen_port` 换成 `remote`（P23）与 `parse_proc_net` 返回 `Result`（M2 延后第 18 行）逼出三处现有用例的改动，都是计划要的行为的直接后果：`crates/rurge-engine/tests/process.rs` 的 `a_finished_lookup_is_logged_at_debug_and_nowhere_higher` 现在找 ` remote=<监听器的地址>`；Linux 的八条解析用例包进 `Ok(..)`；`an_address_of_another_family_or_width_is_not_read`（一张一行也解析不出的表）原来期望"没有行"，现在期望 `Err(ReadError::Layout)`——那正是 M2 延后第 18 行要的规则。macOS 的 `inp_vflag_says_which_addresses_a_record_has` 计划点了名（0x3 出两行），另加了 0xFF 一例 | `af34beb` |
| 2 | 1 | 没有 `local` 的虚拟网卡 TCP 会话（够不到：`serve` 总是设它）查进程时 `remote` 取 `None`，只凭应用自己的端点去找；P15 的 `0.0.0.0:<in_port>` 只给监听器的会话，计划对这一种没有说。`the_query_follows_the_listener_and_the_transport` 钉住。主持这一轮的会话代项目所有者作的裁定。代价：万一有这样的会话，它只凭本地端点匹配一行，同一个本地端点的几行里第一条有主人的胜出 | `af34beb` |
| 3 | 2 | 池打开时，文件里没有名字的编号（低于游标的，回绕之后是任何位置的）再隔离 120 秒，排在其余之前。一个编号被上一次运行隔离着、没来得及交出就结束了，它既没有名字、又不在游标之后、也不在使用次序里，文件的格式分不出"被隔离的"和"从没用过的"，不处理就永远丢了——连着几十次很快的重启（或启动后 120 秒内停下）就把池用光。设计第 10.3 节没有写这一处（设计第 18 节第 18 条）。第一次写成与接下来的 2048 个**抵扣**（`5546384`，提交说明还这么写，历史没有改）；评审指出（Important）一次在不干净的打开之后 120 秒内崩溃的运行是从游标交出它没来得及存的编号的，不是从上一次的隔离里，抵扣就漏了它们，`0efac97` 改成**叠加**：空洞之外另隔离接下来的 2048 个，用例 `a_crash_inside_the_quarantine_holds_back_what_the_crashed_run_gave_out` 与 `holes_in_the_file_are_held_back_with_the_rest`。主持这一轮的会话代项目所有者作的裁定。代价：连续崩溃时每次 120 秒内的重启多隔离 2048 个，大约重启 64 次之后所有编号都在隔离里，`assign` 在每次运行的头 120 秒答 `Exhausted`，直到某一次运行活过 120 秒（没有编号永远丢：最后一次打开之后 120 秒全部放出）；代码里的说明没有写这笔代价（延后第 17 行） | `5546384`、`0efac97` |
| 4 | 2 | 计划没有定的几处：`open` 打开 `clean 1` 的文件后立刻改写成 `clean 0`、没有文件时立刻写一份空表（P4），这第一次写失败时 `open` 没有地方说（`Loaded` 里没有放它的位置）——写错误被吞掉、`dirty` 留着，第一次改动时重试并 WARN（延后第 17 行）；`wrapped` 在发出最后一个新编号时（游标到头）置位，不是第一次回收时，只在文件的文字里看得见（`cursor 4 wrapped 1`），两条用例钉住；读文件对行尾严格（CRLF 的文件读作 `NotRead`，末尾至多一个空行），名字不设每个标签 63 字节的上限（计划只写了"非空标签、整个至多 253 字节"）；`Debug` 只显示个数；`Lease` 的 `Drop` 把编号还回去、算最近用到的 | `5546384` |
| 5 | 3 | 端到端用例 `a_reply_says_which_flow_it_answers` 用同一个关联里的两台回显服务器（两个不同的键）代替"DIRECT 的名字目标与 IP 目标各一个关联"，效果相同；同一个键上两条流给 `None` 的情形由 `which_flow_a_reply_answers` 钉住。`an_answer_from_a_source_in_ipv6_form_finds_its_flow` 多断言 `answers` 是 `Some(10.0.0.1:53)`，是 `Hears` 记 `answers` 所逼，期望没有放宽。`flow_for` 在恰好一条匹配时要扫完那一档才知道它是唯一一条（至多 1024 条，持互斥锁几微秒） | `a67aecc` |
| 6 | 4 | 留着没读字节的连接不再计入 `max_sockets`，它的 `Leftover` 拿着整个接收缓冲的预算份额（至多 65,535 字节）到读完或流被丢掉（P26；每份受那条会话的一生所限）。超时发生在应用的 FIN 之后时，流读到字节再读到 EOF（`Closed`，写是 `BrokenPipe`）——`recv_slice(&mut [])` 的规则；只有没收到 FIN 的超时读成复位。计划之外三条用例：环形缓冲的回绕（`unread_bytes_across_the_end_of_the_receive_buffer_are_kept_in_order`）、没读完时同一个四元组上的新 SYN（`a_new_connection_of_the_same_tuple_is_not_read_by_the_old_stream`）、复位的连接什么也不留（`bytes_unread_when_the_app_resets_are_not_kept`），和 `TunOptions::check` 的三条小用例 | `03f01c7` |
| 7 | 5 | 停止与转发赛跑时停止居先（`biased`）：转发居先的写法会让停止之后的那次轮询碰到复位，会话于是看谁先到被记成 `Completed` 或 `Failed("the connection was reset")`；停止居先则每个已拨通的会话都被取消，拨号在停止之后才回来的会话在转发开始之前就被取消。因此 `a_dial_under_way_is_not_cancelled_by_the_stop` 里转发以 `Err("cancelled")` 结束（它的期望 `is_err()` 没变，说明与断言信息改了）。`serve` 的 future 大了 112 字节（debug 构建 424 → 536，一万条会话约 1.1 MB）；装箱的 `REJECT-DROP` 路径约 700 字节（原先写的"270 字节"是 `tokio::io::copy` 那一版）。`session_tasks()`（`cfg(test)` 的访问器）在 TUN 停止之后会 panic（或少算），只在 TUN 还在跑时读；停止之后读任务数用 `live_tasks()`（任务 8 加） | `7828455` |
| 8 | 6 | 校验 UDP 与 ICMP 回显的校验和（`classify`）使 M3a 的 `truncated_transport_headers_and_port_0_are_malformed` 在三处变红，计划没点名：长度字段写 10 而实际 12 字节的 UDP 报文、截成前 8 字节的回显请求、同一段 UDP 字节放进 IPv4 与 IPv6 两个包（`in_both(17, &udp)`）——它们原来的校验和是对着别的长度或别的地址的。三个声明与每一条期望都不变（仍是"通过"），输入换成对自己的地址与长度校验和正确的报文（`echo_bytes_with`、各族的 `udp_packet`）。主持这一轮的会话代项目所有者作的裁定（规矩是没点名的红用例停下来报告，这里的红是计划要的行为的直接后果）。代价：一条用例的输入要改回去，没有行为 | `9024639` |
| 9 | 6 | 不是回显请求的 ICMP 先按类型判（仍是 `unsupported`）、再看校验和：没有谁读它也没有谁转发它，校验和不对的非回显 ICMP 因此不计 `malformed`。主持这一轮的会话代项目所有者作的裁定。代价：损坏的非回显 ICMP 少计一个 `malformed`、多计一个 `unsupported`，别的没有区别；`icmp_other_than_echo_is_unsupported` 与 `the_checks_run_in_order` 不改就继续绿 | `9024639` |
| 10 | 6、7、8 | `hooks` 与 `pinger` 在任务 6 里留在 `Inner` 上、标 `#[expect(dead_code)]`（M3a 的先例）；任务 7 随接收任务的上下文 `dispatch::Context` 搬过去，仍带着这两个标记；任务 8 读它们时去掉。主持这一轮的会话代项目所有者作的裁定。代价：两个字段搬回 `Inner` | `9024639`、`22911ae`、`9c1991c` |
| 11 | 6 | `Shard::new` 有 8 个参数（加了池），标 `#[allow(clippy::too_many_arguments)]`（仓库里另有四处同样），没有为它造一个结构；任务 6 写的 `TunOptions` 字段说明里有任务 7、8 才成真的话（`queue_full`、`busy` 的含义），7、8 把它们做成真的，并订正了与各自的任务要求不一致的几处 | `9024639` |
| 12 | 7 | `UdpClient::refused` 的说明多一句"在流的任务里同步地调，实现不得阻塞或等待"（`crates/rurge-inbound/src/session.rs`）；`Counters::queue_full` 与 `busy` 的含义扩到 UDP（会话的队列满、会话的目标满）；`FakeDialer::send_to_app` 是 `async` 返回 `io::Result<()>`（`send` 要等写任务的队列有空位）；直接调 `route` 的现有用例是四处（计划说三处），四处都改成断言 `None`；`udp_and_icmp_are_not_handled_yet` 去掉 UDP 一半（任务 8 把整条去掉），`each_reason_warns_once` 的两个 UDP 包换成协议 47 的包，期望不变。停止之后只要还有 UDP 会话的客户端活着（引擎可能比 `associate` 多拿它一会儿），写任务就最多还要 `STOP_GRACE`（1 秒）才结束 | `22911ae` |
| 13 | 8 | 假 `Pinger` 多一个 `PingScript::Reply(&'static [u8])`（"应答的负载是 `Pinger` 给的"要它）；装不进一个 IP 包的 `Pinger` 应答（IPv4 的负载超过 65515、IPv6 的超过 65535 字节）带一行 DEBUG 丢弃、不计数（计划与 P10 没有给它计数）；检查的次序由用例钉住：`icmp-forwarding = false` 居先——连 rurge 自己的地址的回显请求也丢弃，计 `unsupported`（设计第 8 节的表把它放在第一行）——，在途上限先于 fake 地址的查找（上限到了时，一个不认识的 fake 地址也计 `busy`、不计 `unknown_fake`）。评审的 Important（`Gated` 设备与 `in_five_seconds` 在 `tests/udp.rs`、`tests/icmp.rs`、`tests/outcomes.rs` 里重复）在 `8de71f1` 改：`GatedDevice` 与 `in_five_seconds` 只在 `tests/support.rs` 有一份，`TunHandle::live_tasks()`（`cfg(test)`）取代够进私有字段 `inner.tasks.len()` 的写法，`STOP_WELL_BEFORE` 由 `writer::STOP_GRACE` 推出 | `9c1991c`、`8de71f1` |
| 14 | 9 | `Engine::accepting_token()`（`pub(crate)`，`accept` 令牌的子令牌：停止接受之后才造的也立刻是已取消的）加在 `engine.rs` 里，因为 `accept` 在那里是私有的而 `tun.rs` 是同级模块；INFO 行里的两个数从 `QUARANTINE_COUNT` 与 `QUARANTINE.as_secs()` 格式化出来（文本与计划逐字相同）；计划之外两条用例 `a_pool_left_unclean_is_told_of_once` 与 `an_association_returns_when_closed_and_lets_go_of_its_client`（后者是下面第 16 行的证据）；`rurge-tun` 没有动（见下一行） | `ec93297` |
| 15 | 9 | 读过 `Engine::dial` 每一条会等待的路（P29），**没有一条没有时限**，所以 `TunHandle::stop` 的说明与 `session.rs` 拨号处的注释照旧：规则里的进程查找 200 毫秒（`PROCESS_WAIT`）；规则要的名字解析 5 秒（解析器并发查询的 1 秒重发 × 5 轮），等另一个调用者已在途而那个调用者不见了的查询时至多约 10 秒；`evaluate-before-use` 的等待是组的 `round_timeout`（最长的成员 `test-timeout` × ⌈测试数 ÷ 8⌉，缺省 5 秒、直连类成员 10 秒），**这是 Profile 自己定的唯一一个**，`test-timeout` 可以写任意多秒；连接 10 秒（`CONNECT_TIMEOUT`），每个出站把它的整个握手（连接、Shadow TLS、obfs、TLS、WebSocket、CONNECT 或认证；`external` 是程序的启动，`wireguard` 是隧道的启动）包在 `timeout(opts.timeout)` 里，链共用一份预算，`smart` 组的每次尝试再按"剩余时间 ÷ 剩余尝试次数"限时。故意不在预算里的：懒请求头的协议（trojan、vmess、shadowsocks、snell）等服务端的第一个回答，那是转发循环在等，停止经会话的令牌取消它。UDP 一侧：流的每个等待都与关联的 `ended` 令牌赛跑。所以停止之后 `join` 最长等多久取决于 Profile，不是一个常数；可执行文件停 TUN 时仍给等 `join` 的那一步一个自己的时限（延后第 8 行） | `ec93297` |
| 16 | 9 | TUN 的 UDP 路径靠引擎的三件事，都核对过：`UdpClient::refused` 是在流的任务里同步调的（`run_flow` 与 `drain`）；`Engine::associate` 在 `closed` 取消时返回（`udp::serve` 的第一个分支）；`associate` 返回之后引擎不再长久地拿着客户端的克隆（`Arc::strong_count` 回到 1，用例 `an_association_returns_when_closed_and_lets_go_of_its_client`；删掉那个分支、留一个克隆的变异都使它变红）。没有量过的：停止之后还有一条活着的 UDP 流时写任务多久结束（TUN 一侧的 `STOP_GRACE`） | `ec93297` |
| 17 | 1 ～ 9 | 各任务的评审（每个任务一位新的评审者）：规格全部 ✅，质量没有 Critical；Important 两条，都在下一个任务之前的修正轮里改了，复审认定 ADDRESSED：任务 2 的（空洞与接下来的 2048 个抵扣，第 3 行，`0efac97`）、任务 8 的（测试里的重复，第 13 行，`8de71f1`）。门禁每个任务一次，一台 Windows 11 x86-64 的开发机上，`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 干净，`cargo test --workspace --no-fail-fast` 的结果行数与通过数：任务 1 79、3979（含 `rurge-api` 的 `api` 测试二进制单独重跑的 56 个），任务 2 79、4018（修正轮 4019），任务 3 79、4024，任务 4 79、4037，任务 5 79、4042（全量里 78 行、3807 个，挂住被杀的 `rurge-dns` 库测试二进制单独重跑的 235 个算作第 79 行，合起来），任务 6 79、4053，任务 7 79、4088，任务 8 79、4108，任务 9 80、4125（`rurge-engine` 多了集成测试二进制 `tun`），都是 0 个失败、12 个忽略。三次不是代码的事：任务 1 的 `rurge-api` 的 `api` 测试二进制与任务 9 一次早先运行的 `outbounds_masque` 以 `STATUS_HEAP_CORRUPTION` 退出（开发机终端管控软件注入的钩子 DLL，单独重跑 56 个与 11 个通过）；任务 5 的门禁里 `rurge-dns` 的库测试二进制挂住（延后第 28 行） | — |
| 18 | 10 | 测试约定。主持这一轮的会话代项目所有者作的裁定：CLAUDE.md 的「测试约定」一段加一句"阶段 3 / M3b 起另有几条"——经引擎的 TUN 用例守 P17 的两道防线（`guarded()`）、塞进 TUN 的 UDP 与 ICMP 包带正确的校验和并用 `rurge_tun::testing::packets` 造、数 ICMP 条数的用例把 `writer_queue` 设到至少 400、`session_tasks()` 只在 TUN 还在跑时读而停止之后读 `live_tasks()`——。计划的任务 10 没有列它；下一个写 TUN 用例的会话要这些规矩，它们该在约定所在的地方。代价：一句话要挪 | — |
| 19 | — | 计划的**目标**一句（第 5 行，`34c477c` 起）写"经虚拟网卡来的 TCP 连接、UDP 数据报与回显请求照规则与策略走"；回显请求**不经规则与策略**（设计第 8 节；`crates/rurge-tun/src/icmp.rs` 的 `receive`：rurge 自己的地址本地应答，fake 地址先解析它的名字、再经 `Pinger` 向名字的地址发，其余经 `Pinger` 发；`icmp-forwarding = false` 时一律丢弃，rurge 自己的地址的也是）。照项目的做法计划的正文不改写，这一行覆盖那一句 | — |
| 20 | — | 整分支终审（2026-10-23；一位新的评审者，只读代码与文档，什么也没运行）：没有 Critical，没有 Important，结论"改了再合并"。五条次要项：握手在停止之后、片的最后一轮之前才结束的会话还会拨号；三条 UDP 保活用例的余量只有 3 倍（推送 100 毫秒一次，空闲 300 毫秒）；设计第 18 节第 22 条与 CLAUDE.md 说 `stop()` 等转发（它只发出停止，`join` 才等）；`Loaded::NotRead` 的说明说得比做的多；TUN 里排队的 UDP 没有字节上限。另分拣了各任务评审留下的小项（做的、可以等的、给 M4 的）。最终修正波做了：握手在停止之后才结束的会话不再拨号（`session::serve`）；转发中的会话等停止令牌的子令牌（`Life::session_stop`，延后第 26 行）；`keep_unread` 的说明，读失败或读得比排队的少时结局是复位并加 `debug_assert_eq!`（延后第 10 行）；`TunSettings.icmp_forwarding` 的字段说明（`856db45`）；停止居先的断言收紧到 `Err("cancelled")`（把 `Life::session_stop` 换成新造的令牌，这条用例与 `stop_ends_a_relay_stuck_on_its_upstream` 都变红），`FakeDialer::relay_ends` 的说明，三条 UDP 保活用例的空闲时间改 1 秒、推送各 25 次（`4be3034`）；`QUARANTINE`、`QUARANTINE_COUNT`（含记录第 3 行的代价）与 `Loaded::NotRead` 的说明，`MockDns` 遇到错误停 10 毫秒（延后第 28 行）（`2695010`）；五处起 TUN 的辅助带上两道防线，池打开时 `Missing` 与 `Clean` 什么也不说（让 `start_tun` 对它们记一行 INFO，`the_engine_reopens_the_pool` 变红）和 WARN 的 `error` 字段两条断言，三处文件开头指到 M3 设计（`880482e`）；文档：本行、第 19 行、延后事项的去向与第 29 行、设计的状态与第 18 节第 20、22 条、CLAUDE.md。主持这一轮的会话代项目所有者作的裁定：终审的分拣照单收下，可以等的各给一个去向（延后第 10 ～ 14 行）；延后第 26、28 行不移走、在这一波里做掉；第 29 行是新的；握手在停止之后才结束的会话不拨号这一条不带用例（停止信号与片的最后一轮之间的那一段，用夹具排不出来）；第 1 行仍归项目所有者，带上终审的建议（在 M4 之前定：从 M4 起本机的每条连接都经虚拟网卡来）；延后第 17 行的两条说明也进这一波。代价：握手之后看停止的那一行没有用例看着；每个转发中的会话多一个令牌节点，造它和丢它时各拿一次停止令牌的锁（没有量过，基准是忽略的 release 用例，M3b 的规矩不许 `-p`）。门禁（一台 Windows 11 x86-64 的开发机上）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 干净，`cargo test --workspace --no-fail-fast` 80 个结果行、4125 个通过、0 个失败、12 个忽略，没有 `STATUS_*` 退出、没有挂住 | `856db45`、`4be3034`、`2695010`、`880482e` |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | 经虚拟网卡来的被拒、失败的会话，引擎的 INFO 行带 `dst`（名字）与 `src`（P13）；设计第 14 节要它们只在 DEBUG | 项目所有者（整分支终审建议在 M4 之前定：从 M4 起本机的每条连接都经虚拟网卡来） |
| 2 | 同一个端口上同时对两个名字收发、出站是全锥的载体时，回包的来源可能是另一个名字的 fake 地址或真实地址（P7） | 兼容性清单，M3c |
| 3 | P2P 应用一个源端点对很多对端时，每个会话至多 1024 个目标、引擎每个关联至多 1024 条流 | 兼容性清单，M3c |
| 4 | 真实的 `Pinger`（ICMP 套接字，绑在物理网卡上） | M4 |
| 5 | `TunSettings` 的其余字段与 `Hooks::pre_match` | M3c |
| 6 | 绑在 `0.0.0.0` / `[::]` 上的监听器的会话的 `local`（P25） | M4 的手工验收 |
| 7 | 每次查询都换源端口的 UDP 应用（自带随机源端口的 DNS 解析器）每个源端口占一个会话 60 秒，一秒约 68 个以上时碰到引擎的 4096 个关联、丢包计 `busy`；要不要让只有 53 端口目标的会话早一点结束（同引擎的 DNS 流 10 秒） | M3c（与应答器、`hijack-dns` 一起看） |
| 8 | 停止不取消正在进行的拨号（P29）：一个永不回来的拨号让 `join` 一直等；可执行文件停 TUN 时给等 `join` 的那一步一个时限 | M4 |
| 9 | 拆 `src/tests/bench.rs`（M3a 延后第 7 行，P32） | M3c（加任何基准之前） |
| 10 | 任务 4 的三条小项：`keep_unread`（`crates/rurge-tun/src/tcp/shard.rs`）的说明说"超时关掉的套接字不是干净的结束"，应当说"在应用的 FIN 之前超时关掉的"；没有用例钉住应用的 FIN 之后的超时（读到字节再读到 `Ok(0)`，写是 `BrokenPipe`）；`keep_unread` 里的 `recv_slice(..).unwrap_or(0)` 会吞掉将来某个 smoltcp 可能给的错误（P26 禁止的悄悄的截断）：先取 `recv_queue()`，取出的字节数用 `debug_assert_eq!` 对上 | 做掉两条（说明；读失败或读得比排队的少时结局是复位，加了 `debug_assert_eq!`），最终修正波，`856db45`；应用的 FIN 之后超时的用例：下次动到 `keep_unread` 时 |
| 11 | 任务 5 的小项：停止居先没有被钉住（`a_dial_under_way_is_not_cancelled_by_the_stop` 收紧成 `assert_eq!(ends, [Err("cancelled".to_string())])`）；`FakeDialer::relay_ends`（`crates/rurge-tun/src/testing/dialer.rs`）的说明没有提到 `Err("cancelled")`；`session_tasks()` 加一个"TUN 还没有停止"的 `debug_assert!` | 做掉两条（停止居先的断言收紧到 `Err("cancelled")`；`relay_ends` 的说明），最终修正波，`4be3034`；`session_tasks()` 的 `debug_assert!`：M3c（拆测试文件的那个任务） |
| 12 | 任务 6：没有用例钉住"SYN 在租约拿到之后被拒时把租约还回去"（`crates/rurge-tun/src/tcp/shard.rs` 的 `open`：`listen` 失败或协议栈没有收下 SYN 时提前返回，两处 `release_held` 在那里，租约随参数 `name` 一起被丢掉；`new_syn` 自己的两处 `release_held` 在租约拿到之前就拒了）：`with_count(1)` 的池，一个 smoltcp 会丢掉的、发往 fake 地址的 SYN，然后 `assign` 成功 | M3c（TCP——应答器的 53 端口与 SYN 层的预匹配：动到 `open` 时加这条用例） |
| 13 | 任务 7：没有用例钉住"阻塞在写任务的队列上的 `send` 被停止结束"（`GatedDevice` 卡住设备，停止，约 1.5 秒内 `BrokenPipe`）；`Over::drop` 调的 `Session::end()` 里的 `targets.clear()`（`crates/rurge-tun/src/udp/mod.rs`，第 111 行一带）没有被钉住（一个在 `associate` 返回之后仍拿着客户端的 dialer，加 `with_count(1)` 的池）；`receive` 在 `parts()` 失败时计 `Malformed`，过了 `classify` 就够不到，加一个 `debug_assert!` 或一句注释；`Ids::new`（`crates/rurge-tun/src/fragment.rs`）在 `getrandom` 失败时悄悄用 `0x5eed`，而 `Tun::start` 对各片的种子是 WARN 的 | M3c（UDP 的去处——应答器与 `hijack-dns`） |
| 14 | 任务 9：`crates/rurge-engine/tests/tun.rs` 里五处起 TUN 的辅助（池与日志的用例）从没有两道防线的 `Profile::default()` 起（它们不移动任何包；P17 要每条引擎端到端用例都带防线）；没有用例钉住 `Loaded::Missing` 与 `Clean` 什么也不说，WARN 的 `error` 字段也没有断言；"引擎放开客户端"的证据以 5 秒为界，而 TUN 的宽限是 `STOP_GRACE`（1 秒）；`runtime.rs`、`tun.rs` 与 `tests/tun.rs` 的开头写"phase 3 M3b design"，文档其实是 M3 设计 | 做掉三条（五处辅助带上两道防线；池打开时 `Missing` 与 `Clean` 什么也不说和 WARN 的 `error` 字段；三处文件开头指到 M3 设计），最终修正波，`880482e`；"引擎放开客户端"以 1 秒为界的证据：M4（与第 8 行一起） |
| 15 | 拆测试文件（与第 9 行同一遍）：`src/tests/outcomes.rs`、`lifecycle.rs`（1331 行以上）、`udp.rs`（1581）、`fixture.rs`（1650）、`icmp.rs`；`dispatch::Context` 被 `udp/*` 与 `icmp.rs` 引用而 `dispatch` 引用 `udp`（模块环）；`datagram()` 是个造包的函数却在 `udp/mod.rs` 里；`Session::ending()`（一个设置函数）与 `State.ending`、`Session::end()` 并排 | M3c（与第 9 行同一遍，加任何基准之前） |
| 16 | 任务 1：`query_for`（`crates/rurge-engine/src/owner.rs`）的说明没有说没有 `local` 的虚拟网卡 TCP 会话以 `remote: None` 去问（只有用例钉住）；`crates/rurge-platform/src/procinfo/linux.rs` 里 `Layout` 那句说明（第 70 行一带）读起来别扭 | 下次动到时 |
| 17 | 任务 2：`open` 的第一次改写（`clean 1` → `clean 0`）失败时被吞掉（`Loaded` 里没有放它的位置；第一次改动时重试并 WARN，在那之前无害）；没有"一次保存失败、下一次成功并带着先前的名字"的用例；池的 `Debug` 用例断言的是 `contains('1')` / `contains('0')`；`save` 的两条 WARN 分支只有错误类型不同；只在测试里用的 `AtWarn` 抄了 `rurge_net::testing::capture_events`；`QUARANTINE` 的说明（`crates/rurge-dns/src/fakeip/mod.rs`）只说"不干净地停下之后"，而干净的打开也把空洞隔离 120 秒；代码里的说明没有写记录第 3 行的连续崩溃的代价 | `QUARANTINE` 的说明与连续崩溃的代价：最终修正波做掉，`2695010`；其余：下次动到时 |
| 18 | 任务 3：纯用例没有钉住"几个键恰好相同的流压过同端口的域名流"；`answers` 对每个被应答的回包克隆一次 `HostName`，即使客户端不看它，一档里匹配到的要扫到底（至多 1024 个，在互斥锁里）；单元用例里六个字段的 `Association` 字面量重复了一份 | 下次动到时 |
| 19 | 任务 4：`TunStream::drop` 在 `stream_dropped` 之前丢掉没读的记录（对，但没有注释）；说明的小毛病：`remove` 的说明还说"its buffers"，`lifecycle.rs` 里 `send_pattern` 的说明写得乱，`TunOptions` 的字段说明没有写 `check` 强制的范围 | 下次动到时 |
| 20 | 任务 5：`serve` 里的 `stopped` 标志可以去掉（风格） | 下次动到时 |
| 21 | 任务 6：夹具里三条测假件的用例没有先红；`rig_extras` 重复了 `rig_on` 的接线；`icmp()` 里 `match (src, dst)` 的 `_` 臂也盖住了不可能的混族情形 | 下次动到时 |
| 22 | 任务 8：`Echo::read` 在 `busy` / `unknown_fake` 丢弃之前就复制了负载；`Slot` 可以换成 `Semaphore::try_acquire_owned`；`PingScript::Reply` 在假件的夹具用例里没有一例（两条 icmp 用例盖住它）；`stop_ends_a_reply_that_waits_for_room` 的说明写"停止后一秒"，是和 `STOP_GRACE` 的值绑着的话；`live_tasks` 的说明里"as the tasks that end then unwind"一句含糊 | 下次动到时 |
| 23 | `fake-ip.txt` 用缺省权限建出来，里面列着应用查过、拿到 fake 地址的名字（数据目录的权限）（任务 2） | M4 |
| 24 | 任务 9：`Engine::start_tun` 在 `Tun::start` 之前打开池的文件，所以 `Tun::start` 的 panic（选项不对或 MTU 小于 1280：调用者的错，`# Panics` 写明）会让文件留在 `clean 0`，下次启动隔离 2048 个地址 120 秒。可执行文件在起 TUN 之前调 `TunOptions::check`（P27），所以只有编程错误会碰到它，代价是下一次启动多隔离 120 秒，不进最终修正波。主持这一轮的会话代项目所有者作的裁定；代价：以后在 `start_tun` 里多一个检查 | M4 |
| 25 | 任务 9：`start_tun` 在同一个引擎上调两次（或两个引擎用同一个数据目录）会让一个文件对着两个池；说明写了"同一个数据目录一次一个 TUN"，代码没有拦 | M4（可执行文件只起一个） |
| 26 | 任务 5：转发中的会话每次被唤醒都多轮询一次 TUN 的停止令牌（`session::serve` 里的 `life.stopped()`）；tokio 1.53.1 的 `Notified::poll` 在等待时每次轮询都拿一次 `Notify` 的锁，片本来每一轮也这样做。没量过（基准是忽略的 release 用例，要 `-p` 构建，M3b 的规矩不许）。下次 B1 / B2 的数字动了时再看，要的话给每个会话一个自己的子令牌 | 最终修正波做掉，`856db45`：转发中的会话等 `Life::session_stop()` 给的停止令牌的子令牌（各有自己的 `Notify`）；片、UDP 会话、回显、写任务与收包任务等停止令牌的地方没有动 |
| 27 | 任务 1：Linux 的 `Layout` 规则、macOS 双栈的记录（`inp_vflag` 0x3 出两行）与远端地址的比较只对着合成的表测过，两处真读用例里的 `Query` 字面量（`procinfo/linux.rs` 与 `procinfo/macos.rs` 里 `#[cfg(target_os = "linux")]` / `"macos"` 的模块）从没有在任何地方编译过。任务 2：读一个目录失败（EISDIR）、在一个文件下 `create_dir_all` 失败（ENOTDIR）在 Linux 与 macOS 上也是如此，用例这样假定。任务 3：两条端到端用例只在 Windows 上跑过（关着的回环端口在这里约 2 秒才拒绝连接，别处立刻拒绝）。任务 9：`stopping_the_tun_ends_a_relay_stuck_on_its_upstream` 里"写到一次写停住"要多少回环 TCP 是在这台机器上量的，Linux 与 macOS 的缓冲可能更大，64 MiB 的上限为此而设。`rurge-tun` 的全部用例与引擎的 TUN 用例只在一台 Windows 11 x86-64 的开发机上跑过，CI 没有跑过它们 | 第一次 CI 运行时核对 |
| 28 | 任务 5 的门禁里 `rurge-dns` 的库测试二进制挂住一次（31 个用例"running for over 60 seconds"，约 19 分钟，杀掉；都用回环 TCP——`MockDns` 绑 TCP 与 UDP、引导连接器的用例、一条 DoH 重定向用例；同一个二进制里只用 UDP 的 QUIC 用例当时通过了），就在一批开关过上千条短命回环连接的循环之后；单独跑 235 个、1.89 秒通过。当作机器（终端管控软件或回环资源耗尽）而不是改动（`rurge-dns` 自任务 2 起没动过）。任务 6 ～ 9 的门禁在循环之前跑，没有再出现。再出现在没有跟着循环的门禁上时去查：`MockDns` 的 accept / recv 循环遇到错误就 `continue`、不停顿 | `MockDns` 遇到错误停 10 毫秒（最终修正波做掉，`2695010`）；那是不是挂住的原因没有证明：再出现时再查 |
| 29 | 整分支终审：TUN 里排队的 UDP 没有字节上限。每个会话至多排 `udp_queue`（256）个数据报，会话至多 4096 个（引擎的关联上限 `ASSOCIATIONS`；`admit_udp` 只检查、不预留，一阵新会话可以同时通过），所以至多约 1.5 GB（MTU 1500：4096 × 256 × 1472 字节），MTU 9000 时约 9.4 GB（4096 × 256 × 8972 字节）；引擎一侧每个关联另排至多 1 MiB（`QUEUED_BYTES`，自阶段 2 起）。TUN 要一个总的字节预算（满了丢弃、计 `queue_full`）；引擎一侧留给 NFR-01 在 UDP 洪水下的验收 | M3c（UDP 的去处——应答器与 `hijack-dns`）；引擎一侧：阶段 3 / M6 |
