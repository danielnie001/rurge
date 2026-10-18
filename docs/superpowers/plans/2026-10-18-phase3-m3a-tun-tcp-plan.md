# 阶段 3 / M3a「设备、包的去向与 TCP」实施计划

> **执行方式**：子代理驱动（superpowers:subagent-driven-development）——每个任务一个新的实现者，做完由新的评审者评审，全部做完后整分支评审一次。本计划写到"改哪里、接口是什么、要哪些用例、判据是什么"为止：实现者先写失败的用例、看到红，再实现到绿；与计划的出入记进末尾的「执行期修正记录」。

**目标**：新 crate `rurge-tun` 在一块内存设备上把应用发来的 TCP 包经 smoltcp 0.12（按四元组分片）变成连接，以 `BoxedStream` 交给 `Dialer`；先过基准门槛（单流每方向不低于 0.7 Gbit/s、1 万条连接的峰值常驻内存不超过 429 MB），再补齐进协议栈之前的过滤、SYN 的去处与上限、smoltcp 0.12 隐患的对策、流的一生、握手之后的拒绝与停止。

**架构**：收包的任务只解析、过滤、按四元组的散列分给各片（从不等待，进各片的队列满了就丢并计数）；每片是一套独立的 smoltcp `Interface` + `SocketSet` + 设备队列，放在一把锁里，由自己的任务逐包推进、反复推进发送到没有出口；新的 SYN 得到一个只听它目的端点的套接字；建成的连接以 `TunStream` 交给会话的任务（`Dialer::dial` 再 `Dialer::relay`）；各片发出的段经一个写任务成批写回设备。UDP、ICMP、fake-IP 池、DNS 应答器与引擎接入是 M3b、M3c。

**技术栈**：smoltcp 0.12.0（工作区依赖，特性不加）、tokio 与 tokio-util（`CancellationToken`、`TaskTracker`）、`getrandom`；不新增包。

**依据**：设计 `docs/superpowers/specs/2026-10-18-phase3-m3-tun-core-design.md`（下面写"设计"指它；第 19 节 M3-D1 ～ D26）；总设计 `docs/superpowers/specs/2026-10-16-phase3-enhanced-mode-design.md` 第 3.1 节（基准门槛与"达不到就换自写的 TCP 被动一端"的授权）。照搬的模型是本仓库的 WireGuard 栈 `crates/rurge-proto-wireguard/src/stack.rs`（接口的建法、`Queues` 设备、`advance` 反复推进、`at` 换算时钟、`release` 与 `reap`）。

## 全局约束

- 不新增外部包：`rurge-tun` 的依赖全是工作区已有的依赖（`rurge-config`、`rurge-net`、`rurge-inbound`、`rurge-proto`、`smoltcp`、`tokio`、`tokio-util`、`tracing`、`getrandom`；dev-dependency `tracing-subscriber`），`Cargo.lock` 只多 `rurge-tun` 自己这一项。`rurge-tun` 不依赖 `rurge-platform`、`rurge-engine`、`rurge-dns`（M3a 用不到 `rurge-dns`，M3b 加）。
- `rurge-tun` 平台无关，`unsafe_code` 照工作区 `forbid`（`[lints] workspace = true`）；MSRV 1.89，edition 2024。
- 名字与地址只在 DEBUG 的行里，INFO 及以上的行里没有（设计第 14 节）；WARN 不带地址；`martian` 既不 WARN 也不逐包 DEBUG。
- 测试只用回环与端口 0；经虚拟网卡的目的地是文档地址（`192.0.2.0/24`、`2001:db8::/32`）或 rurge 地址空间里的地址，由测试的假 `Dialer` 映到回环上的测试服务器——假 `Dialer` 只拨用例写给它的回环地址（写了别的就 panic），没写的目的地一律答失败，所以 `rurge-tun` 没有一条用例的连接能离开回环。用例从不建网卡、不改路由与 DNS、不发真实的 ICMP。
- 等待都有时限，不用固定的 sleep 当同步（"断言什么都没发生"除外，注释写它等的是什么、余量多大）。新写的计时用例提交前单独循环跑二十次以上。
- 锁不跨 `await`；每片的状态一把 `std::sync::Mutex`，`TunStream` 只短暂地拿它；从不拿过期的 `SocketHandle`（smoltcp 0.12 的句柄从最小的号起复用、没有代数，拿过期的句柄 `get` / `remove` 会 panic 或拿到别的连接）：连接一律经连接号查表。
- 先写失败的用例、看到红，再实现到绿。现有用例变红时：计划点了名的照计划改并在提交说明里说；没点名的停下来报告，不顺手改期望值。
- 门禁（每个任务末尾一次）：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`（本机 `RUSTFMT` 指向 gnu 工具链的 rustfmt）。聚焦地跑用例也经整个工作区（`cargo test -j 8 --workspace --lib -- <过滤词>`），不用 `-p`；**唯一的例外是任务 3、6 的 release 基准**（计划期决定 P3）。测试二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出时单独重跑那一个。跑全量之前 `df -h /d`，剩余不足约 15 GiB 就停下。
- 只运行本仓库构建出来的二进制与系统自带的只读工具（基准读自己进程的峰值内存时起的 `powershell -NoProfile -Command "(Get-Process -Id <pid>).PeakWorkingSet64"`）；不下载、不安装任何东西。
- 参考实现：GPL 的（mihomo、sing-box、sing-tun）只取事实，代码不抄；MIT / Apache 的（netstack-smoltcp、shadowsocks-rust、clash-rs、leaf、tun2proxy）可以简短引用并注明出处；smoltcp 是 0BSD。
- 注释、日志与错误文本用英文，照所在文件的写法；提交说明的标题用中文。
- 没有新的诊断码，不碰配置格式、兼容性清单与 API（M3a 没有使用者看得到的行为；那些文档随 M3c，设计第 16 节）。

## 评审要点

计划的用例之外最可能咬到使用者的五种情形，每条的用例已加进负责的任务：

1. **浏览器同时向同一个服务器开几条连接**：几个来源不同的 SYN 发往同一个目的端点，各自得到自己的连接，数据不串（任务 5 `simultaneous_syns_to_one_endpoint_each_get_their_own_connection`）。
2. **读得慢的应用**（下载时应用一阵不读）：rurge 的发送缓冲满、转发循环停下不读上游，应用接着读时数据一字不少（任务 7 `a_reader_that_stops_and_resumes_loses_nothing`）。
3. **传到一半应用重置了连接**（用户关了页面、进程被杀）：会话随之结束、套接字拿掉、上游的连接关掉，什么也不挂着（任务 7 `an_app_reset_mid_transfer_ends_the_session`）。
4. **SYN-ACK 丢了、应用重发 SYN**：重发的 SYN 交给已有的套接字，连接照样建成，不另起会话（任务 5 `a_lost_syn_ack_is_recovered`）。
5. **连接一条接一条地开关**（短连接的 HTTP 客户端）：两千条之后套接字、半开的计数、缓冲预算与连接表都回到零，复用的句柄不串到别的连接（任务 7 `two_thousand_connections_in_a_row_leave_nothing_behind`）。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `crates/rurge-tun/Cargo.toml`（新） | 1 | 依赖、`testing` 特性 |
| `crates/rurge-tun/src/lib.rs`（新） | 1、2 | crate 文档；`Tun`、`TunOptions`、`TunHandle`、`Ending`、`Counters`、`RX_MAX` |
| `src/device.rs` | 1 | `Device`、`MemoryDevice`、`AppEnd` |
| `src/testing/{mod.rs,app.rs,dialer.rs,servers.rs}` | 1 | `AppStack` / `AppTcp`、`FakeDialer` / `Script`、回环上的 `Upstream` |
| `src/testing/scripted.rs` | 3 | `ScriptedTcp`（B2 的一万条空闲连接） |
| `src/stats.rs` | 2、4 | 计数、每种原因第一次的 WARN |
| `src/dispatch.rs` | 2、4 | 收包的任务：解析、过滤、分片 |
| `src/writer.rs` | 2、8 | 写任务 |
| `src/packet.rs` | 4、5、6 | 解析与过滤、rurge 自己构造的 RST、SACK-permitted 的改写 |
| `src/space.rs` | 5 | 地址的分类 |
| `src/limit.rs` | 5 | 滑动窗口的限频 |
| `src/tcp/{mod.rs,shard.rs,budget.rs,stream.rs,guard.rs}` | 2、5、6、7 | 片、SYN、缓冲预算、`TunStream`、窗口边缘的守卫、拿掉与重建 |
| `src/session.rs` | 2、8 | 会话的任务：建成 → 拨号 → 转发或拒绝 |
| `src/tests/{mod.rs,fixture.rs,tcp.rs,bench.rs,filter.rs,syn.rs,hazards.rs,lifecycle.rs,outcomes.rs,log_capture.rs}` | 各任务 | crate 里的单元用例（`cfg(test)`） |
| CLAUDE.md、设计第 18 节、本计划末尾两张表 | 9 | 文档 |

模块的名字与拆法可以照实际调整（设计第 3.1 节"计划可以合并或再拆"），调了在报告里说。

---

## Task 1：crate 骨架、设备与应用端夹具

**改哪里**

1. `crates/rurge-tun/Cargo.toml`（新；`members = ["crates/*"]` 自动收进工作区）：`description = "The virtual adapter's core for rurge: IP packets in, TCP connections out, on a userspace TCP/IP stack"`，`version` 等照 WireGuard crate 取工作区的；依赖见全局约束；`[features] testing = []`（注释：应用端夹具与假 `Dialer`，由依赖它的 crate 的 dev-dependencies 打开）；`[dev-dependencies] tracing-subscriber.workspace = true`；`[lints] workspace = true`。
2. `src/lib.rs`：crate 文档（一句话说它是什么，指向设计）；`pub mod device;`；`#[cfg(any(test, feature = "testing"))] pub mod testing;`；`#[cfg(test)] mod tests;`。
3. `src/device.rs`：
   - `Device`（设计第 4.1 节；`BoxFuture` 是 `rurge_net::BoxFuture`）：
     ```rust
     pub trait Device: Send + Sync {
         /// Waits for at least one packet, then moves at most `max` whole IP packets into `into`.
         /// An error ends the TUN.
         fn recv<'a>(&'a self, into: &'a mut Vec<Vec<u8>>, max: usize) -> BoxFuture<'a, io::Result<()>>;
         /// Writes `packets` in order and leaves it empty. The memory device waits for room.
         fn send<'a>(&'a self, packets: &'a mut Vec<Vec<u8>>) -> BoxFuture<'a, io::Result<()>>;
         fn mtu(&self) -> usize;
     }
     ```
   - `MemoryDevice::pair(mtu: usize, capacity: usize) -> (MemoryDevice, AppEnd)`：两个方向各一个 `tokio::sync::mpsc::channel::<Vec<u8>>(capacity)`。`MemoryDevice::recv` 在一把 `tokio::sync::Mutex` 里对接收端 `recv_many(into, max)`，返回 0（应用端已不在）时是 `io::Error::new(BrokenPipe, "the app end of the memory device is gone")`；`send` 逐个 `Sender::send().await`（等到收得下），对端不在时同样 `BrokenPipe`，写完清空 `packets`；`mtu()` 是 `pair` 给的值。
   - `AppEnd`（测试里"操作系统"的那一头）：`mtu(&self) -> usize`；`async fn send(&self, packet: Vec<u8>) -> io::Result<()>`（等到收得下；对端不在时 `BrokenPipe`）；`async fn recv(&mut self) -> Option<Vec<u8>>`（对端不在且读空时 `None`）；`fn try_recv(&mut self) -> Option<Vec<u8>>`；`AppEnd::crossed(mtu: usize, capacity: usize) -> (AppEnd, AppEnd)`——两个互相接着的应用端，一端 `send` 的包从另一端 `recv` 出来（任务 3 量夹具自己的上限用）。
4. `src/testing/app.rs`：`AppStack`——一个 smoltcp 0.12 栈，接在一个 `AppEnd` 上，由它自己的任务推进（建法与推进照 WireGuard 栈：`Medium::Ip`、`set_any_ip` 不开、经自己地址的缺省路由、反复推进到没有出口；接收缓冲 `RX_MAX`（65535，设计第 15.1 节：它也是 smoltcp，H2 对它一样成立）、发送缓冲 256 KiB、关 Nagle、Reno）：
   ```rust
   pub struct AppStack { /* Clone; the driver task owns the AppEnd */ }
   pub enum Direction { ToTun, FromTun }
   impl AppStack {
       /// The operating system's side: 198.18.0.1/32 and fd00:6152::1/128, a default route through each.
       pub fn start(end: AppEnd) -> AppStack;
       /// The same with other addresses (the listening side of the fixture's own ceiling, task 3).
       pub fn start_with(end: AppEnd, addresses: &[IpCidr]) -> AppStack;
       /// From the next free port of `dst`'s family (49152 up); resolves once established.
       /// `ConnectionRefused` when the SYN is answered with a reset, `TimedOut` after 5 s.
       /// Once the SYN-ACK is taken the connect has succeeded, as on a real system: a reset
       /// right after it shows on the first read or write (`ConnectionReset`).
       pub async fn connect(&self, dst: SocketAddr) -> io::Result<AppTcp>;
       pub async fn connect_from(&self, port: u16, dst: SocketAddr) -> io::Result<AppTcp>;
       /// Listens on `local` at once (before the future is first polled) and resolves
       /// with the first connection.
       pub fn accept(&self, local: SocketAddr) -> impl Future<Output = io::Result<AppTcp>> + Send + 'static;
       /// Packets from the other end whose TCP or UDP destination port is in `ports`
       /// go to the receiver instead of the stack.
       pub fn divert(&self, ports: RangeInclusive<u16>) -> mpsc::UnboundedReceiver<Vec<u8>>;
       /// Sends `packet` to the other end as it is.
       pub fn inject(&self, packet: Vec<u8>);
       /// Sees every packet both ways before anything else: `false` drops it; it may change it.
       pub fn set_filter(&self, filter: impl FnMut(Direction, &mut Vec<u8>) -> bool + Send + 'static);
       /// Packets the stack took in and gave out so far (B2 reads it).
       pub fn packets(&self) -> (u64, u64);
       /// Ends the driver now and drops the AppEnd (the memory device then fails);
       /// open `AppTcp`s fail with `ConnectionAborted`.
       pub fn close(&self);
   }
   pub struct AppTcp; // AsyncRead + AsyncWrite + Unpin + Send
   impl AppTcp { pub fn local(&self) -> SocketAddr; /// Sends a reset now. pub fn abort(&mut self); }
   ```
   `AppTcp` 的读写规矩与 `TunStream` 相同（计划期决定 P12、P13）：对端的 FIN 读空之后读是 `Ok(0)`；smoltcp 照着做了的 RST 之后读写是 `ConnectionReset`；`poll_flush` 立刻返回；`poll_shutdown` 是 `close()`、立刻返回；没 `shutdown` 就丢掉是 `abort()`。`AppTcp` 被丢掉之后，驱动在套接字到了 `TimeWait` 又过了 20 毫秒（它可能还欠着一个延迟的 ACK，见任务 2 第 5 点"拿掉套接字"），或到了 `Closed` 且 `remote_endpoint()` 是 `None`（`abort()` 的那一个 RST 已经发出去，smoltcp `tcp.rs` 第 2537 ～ 2540 行）时把它从套接字集里拿掉、端口可以再用——夹具不守 TIME-WAIT 的 10 秒；不拿掉的话任务 7 一条接一条的 2000 条连接会在应用一侧攒下 2000 个各带 320 KiB 缓冲的套接字。
5. `src/testing/dialer.rs`：
   ```rust
   pub enum Script {
       /// Connect to this address and relay to it; `script` panics unless it is a loopback address.
       Upstream(SocketAddr),
       /// `tokio::io::duplex(buffer)`; the far end writes `send` bytes of `pattern` and, all the
       /// while, reads and discards until the session ends.
       Memory { buffer: usize, send: usize },
       /// `tokio::io::duplex(buffer)`; the far end neither reads nor writes until
       /// `FakeDialer::release(dst)`, then reads and discards until the session ends.
       Hold { buffer: usize },
       Reject(RejectKind),
       /// `DialError::Failed { kind: FailKind::Connect, message: "scripted failure", .. }`.
       Fail,
   }
   pub struct FakeDialer;
   impl FakeDialer {
       pub fn new() -> Arc<FakeDialer>;
       /// By the destination the app connected to.
       pub fn script(&self, dst: SocketAddr, script: Script);
       /// Every session dialed, in order.
       pub fn dialed(&self) -> Vec<SessionInfo>;
       /// How each relay ended, in order: `Ok(())` or the error's text.
       pub fn relay_ends(&self) -> Vec<Result<(), String>>;
       /// Lets the far ends of `Hold` sessions to `dst` start reading.
       pub fn release(&self, dst: SocketAddr);
       /// Bytes the far ends of `Memory` and `Hold` sessions to `dst` have read so far.
       pub fn received(&self, dst: SocketAddr) -> u64;
   }
   impl Dialer for FakeDialer { /* dial, relay */ }
   ```
   没写的目的地答 `Failed { kind: Connect, message: "not scripted" }`；`dial` 建 `SessionHandle::new(id, session)`（`id` 从 1 递增），拒绝与失败先 `handle.finish(Rejected(kind) / Failed(message))` 再答；`relay` 是 `tokio::io::copy_bidirectional_with_sizes(&mut client, &mut upstream, 8192, 8192)`（与引擎的 `BUF` 相同的两块 8 KiB，B1 的"转发循环的替身"），结束时 `handle.finish(Completed / Failed(..))` 并记进 `relay_ends`。
6. `src/testing/servers.rs`：`pub fn pattern(i: u64) -> u8`（`(i % 251) as u8`，用例逐字节比对用）；`Upstream`（绑 `127.0.0.1:0`、接任意多条连接、丢掉时结束它的任务）：`Upstream::echo().await`（读多少回多少，读到 EOF 后关写端）、`Upstream::sink().await`（读并计数：`received(&self) -> u64`、`wait_for(&self, bytes: u64, within: Duration) -> bool`）、`Upstream::source(bytes: u64).await`（写 `bytes` 个 `pattern` 字节后关写端；`sent(&self) -> u64` 是已经写出的字节数，任务 7 用）；`addr(&self) -> SocketAddr`。

**接口**

- 产出：`rurge_tun::device::{Device, MemoryDevice, AppEnd}`；`rurge_tun::testing::{AppStack, AppTcp, Direction, FakeDialer, Script, Upstream, pattern}`（签名如上）；`rurge_tun::RX_MAX`（任务 2 定义，这一任务先在 `lib.rs` 里放下 `pub const RX_MAX: usize = 65_535;`）。

**用例**（`src/tests/fixture.rs`）

- `two_app_stacks_carry_a_connection`：`AppEnd::crossed(1500, 4096)` 两端各一个 `AppStack`（一个 `start`、一个 `start_with(&[192.0.2.1/32, 2001:db8::1/128])`），v4 与 v6 各一遍：`accept` 与 `connect` 配对，`hello` 往返；1 MiB 每方向，逐字节等于 `pattern`。
- `fin_and_reset_are_told_apart`：对端 `shutdown()` 之后读到 `Ok(0)`；对端 `abort()` 之后读是 `ConnectionReset`。
- `divert_and_inject_go_around_the_stack`：一端 `inject` 的包逐字节到另一端 `divert` 的接收端；另一端的栈对它什么也没发（`packets().1` 不变）。
- `a_filter_sees_both_ways_and_can_drop`：过滤器丢掉第一个带数据的段 → 数据照样到（重传）；两个方向都见过包。
- `a_memory_device_fails_once_its_app_end_is_gone`：丢掉 `AppEnd` → `recv` 与 `send` 都是 `BrokenPipe`；`an_app_end_sees_its_device_gone`：丢掉 `MemoryDevice` → `recv` 是 `None`、`send` 是 `BrokenPipe`；`a_memory_device_keeps_order_and_waits_for_room`：`recv` 至多给 `max` 个、次序不变；容量 1 时第二个 `send` 在应用端取走之前不完成（"什么都没发生"的有界等待，50 毫秒）。
- `the_fake_dialer_answers_what_it_was_told`：`Upstream(echo)` 拨通并转发；`Reject(kind)` 答 `DialError::Reject` 带那个 `kind`；`Fail` 与没写的目的地答 `Failed`；`dialed()` 依次列出。
- `a_held_upstream_reads_only_once_released`：经 `Script::Hold { buffer: 1024 }` 写 4 KiB → `release` 之前 `received` 是 0（50 毫秒的有界等待），之后 5 秒内到 4096。
- `closing_an_app_stack_fails_its_device`：`close()` 之后 `MemoryDevice::recv` 是 `BrokenPipe`，开着的 `AppTcp` 读是 `ConnectionAborted`。
- `a_reset_right_after_the_handshake_fails_the_first_read`（执行期修正记录第 4 行）：应用端 `connect` 发出 SYN，用例从内存设备的另一端读到它，回一个 SYN-ACK、紧跟一个 RST（同一次 `send`；用例跑在当前线程的运行时上，驱动收下这两个包之前 `connect` 的任务不会被轮询）→ `connect` 是 `Ok`，第一次读是 `ConnectionReset`（只看套接字的终态就会答 `ConnectionRefused`）。
- `the_fake_dialer_dials_only_the_loopback`（`#[should_panic]`）：`script(dst, Script::Upstream("192.0.2.1:80"))`。

**判据**：用例绿；门禁绿；`git diff Cargo.lock` 只有 `rurge-tun` 这一项。

## Task 2：最小的一条 TCP 路——包的去向、分片、写任务、`TunStream`、会话

**改哪里**

1. `src/lib.rs`：`Tun::start`、`TunOptions`、`TunHandle`、`Ending`、`Counters`（见接口）。`Tun::start` 在当前的 tokio 运行时上起它的任务，全部任务（含会话的）由一个 `tokio_util::task::TaskTracker` 跟踪；停止经一个 `CancellationToken`。`device.mtu()` 小于 1280 时 `Tun::start` panic，文本 `the device's MTU must be at least 1280`（设计第 5.3 节 H8：MTU 为 0 或太小时 smoltcp 有几处减法下溢；1280 是 IPv6 的最小 MTU，也盖过 IPv4 的 576；设备的 MTU 由 rurge 自己定，小于它是编程错误）。
2. `src/stats.rs`：每种原因一个 `AtomicU64`，`Counters` 是它们的快照。这一任务只数 `malformed`、`unsupported`、`queue_full`（任务 4 接上 WARN 与其余原因）。
3. `src/dispatch.rs`：收包的任务：`device.recv(&mut batch, RECV_BATCH)`（`RECV_BATCH` = 256）；每个包：`Ipv4Packet` / `Ipv6Packet::new_checked` 解析不了 → `malformed`；协议是 TCP → `TcpPacket::new_checked`（解析不了 → `malformed`）→ 四元组 `(源地址, 源端口, 目的地址, 目的端口)` 经 `DefaultHasher` 散列、对片数取模 → 那一片的有界队列 `try_send`，满了计 `queue_full`；其余协议计 `unsupported`。从不等待。`recv` 出错时 TUN 以 `Ending::DeviceFailed` 结束。
4. `src/writer.rs`：写任务：全局的 `mpsc::channel::<Vec<Vec<u8>>>(writer_queue)`；`recv_many` 收下排着的批，接成一个 `Vec<Vec<u8>>` 调 `device.send`；出错时 TUN 以 `DeviceFailed` 结束。
5. `src/tcp/`：
   - 片：`Arc<Mutex<ShardState>>`（`std::sync::Mutex`），`ShardState { iface, sockets: SocketSet<'static>, device: ShardDevice, conns: HashMap<ConnId, Conn>, by_tuple: HashMap<Tuple, ConnId> }`。`ShardDevice`：收的一侧一次只放一个包（逐包推进）、发的一侧是一个 `Vec<Vec<u8>>`；`capabilities()`：`Medium::Ip`、`max_transmission_unit = device.mtu()`、`checksum.ipv4 = Checksum::Tx`、`checksum.tcp = Checksum::Tx`（设计第 4.4 节：TCP 的校验和不验，IPv4 首部的由收包的任务验——任务 4）；`transmit` 总给出令牌（`TxToken::consume` 分配 `vec![0; len]`、推进发的一侧，同 WireGuard 栈的 `Queues`）。
   - 接口：`Config::new(HardwareAddress::Ip)`，`random_seed` 每片各取一次 `getrandom`（同 WireGuard 栈的 `random_u64`；在 `Tun::start` 里取、经 `Shard::new` 交给片，执行期修正记录第 7 行），`set_any_ip(true)`，地址 `198.18.0.253/32` 与 `fd00:6152::fd/128`，各一条经它的缺省路由（设计第 4.2 节）；时钟以 TUN 启动时的 `std::time::Instant` 为零点（同 WireGuard 栈的 `at`），不用 smoltcp 的 `Instant::now()`。
   - 片的任务：等进片的队列（`recv_many`）、`TunStream` 的踢（`tokio::sync::Notify`）、`poll_delay` 的定时与停止；醒来后在锁里：**先推进一次发送**（`poll_egress` 一遍：上一次放锁之后 `TunStream` 读走了数据的话，窗口更新先于新收的包发出去——零窗口之后对端带一个字节的探测段若赶在窗口更新之前进了协议栈，smoltcp 会收下它、在下一次推进时 panic，设计第 5.3 节 H2；任务 6 的守卫兜底），再至多 `INGRESS_PER_WAKE`（256）个包逐个推进，然后 `poll_egress` 反复到返回 `PollResult::None` 或做满 `EGRESS_PASSES`（64）次，取出发的一侧，算下一次的定时；放锁之后把这一批交给写任务（队列满了等，不拿着锁等）；做满了任一个界限就 `yield_now` 之后接着做。
   - 推进一个包：四元组在 `by_tuple` 里 → 放进 `ShardDevice`、`poll_ingress_single`；不在、且只带 SYN 不带 ACK → 新的 SYN（下一条）；其余也交给 smoltcp（它对游离段回 RST）。
   - 新的 SYN（这一任务只做设计第 5.2 节的第 5 步，第 1 ～ 3 步在任务 5）：按预算分配两块缓冲（`budget.rs`：接收 = clamp(剩余 ÷ `DIVISOR`, `buffer_min`, `RX_MAX`)，发送 = clamp(剩余 ÷ `DIVISOR`, `buffer_min`, `tx_max`)，`DIVISOR` = 32，剩余 = `budget` − 全部套接字的缓冲之和、饱和为 0；全局一个 `AtomicUsize`）；`tcp::Socket::new`，关 Nagle，`set_keep_alive(Some(keep_alive))`，`set_timeout(Some(timeout))`（执行期修正记录第 3 行：smoltcp 0.12 的超时不看有没有在途的数据，应用最后一个段之后 `timeout` 里什么也没来就断开，`tcp.rs` 第 2118 ～ 2123、2274 行；保活让活着的应用每 `keep_alive` 至少答一个 ACK，空闲的连接不被断开），`CongestionControl::Reno`，ACK 延迟取缺省；`listen(IpListenEndpoint { addr: Some(目的地址), port: 目的端口 })`（出错就丢掉这个 SYN、还回预算）；加进 `sockets`、推进这个包；推进之后不在 `SynReceived` 就拿掉、还回预算；在的建 `Conn`（连接号 `ConnId` 是全局递增的 `u64`；`by_tuple` 与 `conns` 各一项）并起会话的任务。
   - 拿掉套接字（这一任务只拿掉 smoltcp 报 `Closed` 或 `TimeWait` 的，`abort()` 过的要等到 `remote_endpoint()` 成了 `None`——它的 RST 已经发出，见任务 7 第 2 点；`TimeWait` 的要等到第一次看到它在 `TimeWait` 之后过了 20 毫秒——它可能还欠着一个延迟的 ACK：smoltcp 的 ACK 延迟缺省 10 毫秒（`tcp.rs` 第 268 行），收到带负载的段时起表（第 2080 行起），到时才发（第 2308 行），对方的数据与 FIN 在 10 毫秒之内先后到达时，套接字带着这个没发的 ACK 进了 `TimeWait`，提前拿掉的话对方收不到它、重发 FIN 之后得到 RST；片醒来时看本轮碰过的连接，另每秒扫一遍全片）：删两张表的项、还回预算、`ConnShared` 记下结局并唤醒等着它的读写。
6. `src/tcp/stream.rs`：`TunStream`（`AsyncRead + AsyncWrite + Unpin + Send`，持有片的 `Arc`、`ConnId` 与 `Arc<ConnShared>`）：每次读写先按连接号在 `conns` 里找——找不到就照 `ConnShared` 记下的结局答（P12）；读：`recv_slice`，没有数据而 `may_recv` 时登记接收 waker（smoltcp 的一份、`ConnShared` 里一份，拿掉连接时由后者唤醒）、`Pending`，FIN 读空之后 `Ok(0)`；写：`send_slice`，满了登记发送 waker；`poll_flush` 立刻 `Ok`（P13）；`poll_shutdown`：`close()`、立刻 `Ok`；读写之后踢这一片。`pub(crate) async fn established(&self) -> io::Result<()>`：到 `Established` 时 `Ok`，在那之前就结束了是 `ConnectionReset`。`Drop`：照任务 7 第 1 点的条件（写的一侧已 `shutdown`、`!may_recv()`、`recv_queue() == 0` 时不动它，否则 `abort()`），`linger` 在任务 7（执行期修正记录第 5 行）。
7. `src/session.rs`：会话的任务：`established()` → `SessionInfo { src: 应用的地址与端口, in_port: 0, listener: ListenerKind::Tun, ..SessionInfo::tcp(HostName::Ip(目的地址), 目的端口) }`（`SessionInfo` 还没有 `local` 字段，M3b 加）→ `dialer.dial` → 拨通：`dialer.relay(Box::new(stream), dialed.stream, dialed.handle)`（`Dialed.forward` 不用）；`Reject` 与 `Failed`：丢掉 `TunStream`（RST；`REJECT-DROP` 的拿着在任务 8）。

**接口**

- 产出：
  ```rust
  pub struct Tun;
  impl Tun {
      /// Starts the TUN's tasks on the current tokio runtime.
      pub fn start(device: Arc<dyn Device>, dialer: Arc<dyn Dialer>, options: TunOptions) -> TunHandle;
  }
  #[derive(Clone, Debug)]
  pub struct TunOptions {
      pub ipv6: bool,                  // true
      pub drop_hold: Duration,         // 30 s: the engine's DROP_HOLD
      pub shards: usize,               // 64
      pub buffer_min: usize,           // 8 KiB until task 3 settles it
      pub tx_max: usize,               // 256 KiB until task 3 settles it
      pub budget: usize,               // 64 MiB until task 3 settles it
      pub max_sockets: usize,          // 16 384
      pub max_half_open: usize,        // 2 048
      pub half_open_timeout: Duration, // 10 s
      pub linger: Duration,            // 60 s
      pub keep_alive: Duration,        // 30 s
      pub timeout: Duration,           // 600 s: the relay's idle timeout by default
      pub shard_queue: usize,          // 1 024 packets
      pub writer_queue: usize,         // 256 batches
  }
  impl Default for TunOptions { /* the values above */ }
  #[derive(Clone)]
  pub struct TunHandle;
  impl TunHandle {
      pub fn stop(&self);
      /// Every task of the TUN, sessions included, has ended. Answers the same every time.
      pub async fn join(&self) -> Ending;
      pub fn counters(&self) -> Counters;
  }
  #[derive(Clone, Debug, PartialEq, Eq)]
  pub enum Ending { Stopped, DeviceFailed(String) }
  #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
  pub struct Counters {
      pub malformed: u64, pub fragment: u64, pub martian: u64, pub unsupported: u64,
      pub queue_full: u64, pub unknown_fake: u64, pub busy: u64, pub beyond_window: u64,
      pub rst_limited: u64, pub icmp_limited: u64, pub stack_panics: u64,
  }
  pub const RX_MAX: usize = 65_535;
  ```
- 给后面的任务（`pub(crate)`）：`ShardState`、`Conn`、`ConnShared`、`ConnId`、`Tuple`、`Stats`、预算；用例用的 `TunHandle::{sockets_in_use, budget_in_use}`（`cfg(test)`；`half_open` 随任务 5）。片的一次唤醒写成一个不起任务、不读时钟的同步方法（例如 `turn(now, 进来的包) -> (发出的包, 新建成的连接)`）：片的任务调它、把新连接交给会话的任务；任务 5 ～ 7 的"片一级的用例"直接调它，自己决定包的次序。

**用例**（`src/tests/tcp.rs`；`AppStack` 接在 `MemoryDevice::pair(1500, 4096)` 的应用端上；起 TUN 经这个文件里的一个辅助函数，M3b 改签名时只改它）

- `a_connection_is_relayed_v4`：`192.0.2.1:80` → `Upstream::echo`；应用连过去、`hello` 往返；`dialed()` 恰好一项：`src` 是 `198.18.0.1` 加应用的端口、`in_port` 0、`listener` `Tun`、`dst_host` `Ip(192.0.2.1)`、`dst_port` 80、`transport` `Tcp`。
- `a_connection_is_relayed_v6`：`[2001:db8::1]:443`，来源 `fd00:6152::1`，同上。
- `a_mebibyte_crosses_each_way`：上行 1 MiB 到 `sink`、下行 1 MiB 从 `source`（各一条连接），逐字节等于 `pattern`，30 秒的时限里完成。
- `half_close_keeps_the_other_direction`：应用写 64 KiB 后 `shutdown()`，`echo` 读到 EOF、回完关写端；应用读回 64 KiB 再读到 `Ok(0)`。
- `a_failed_dial_resets_the_connection`：没写给 `FakeDialer` 的目的地 → `connect` 成功（握手照常完成，设计第 5.6 节），随后读是 `ConnectionReset`；`dialed()` 有这一次。
- `a_rejected_session_resets_the_connection`：`Script::Reject(RejectKind::Reject)`，同上。
- `nothing_is_dialed_before_the_handshake_completes`：过滤器扣住应用发往 TUN 的握手的 ACK → 等 200 毫秒（断言什么都没发生的固定等待），`dialed()` 是空的；经 `inject` 放出它 → 有界时间里 `dialed()` 有一项，`hello` 往返。
- `an_idle_connection_outlives_the_timeout`（`keep_alive: 100 ms`、`timeout: 3 s`，执行期修正记录第 6 行）：连接建好、`hello` 往返之后两头都不发，等 6 秒（断言什么都没发生的固定等待，两倍于超时）→ `hello` 照样往返。
- `a_silent_app_is_reset_after_the_timeout`（同样的选项）：连接建好之后过滤器丢掉 TUN 发往应用的每个包 → 有界时间里 `sockets_in_use()` 0、`budget_in_use()` 0，`relay_ends()` 有这一条。
- `stop_ends_the_tun`：`stop()` 之后 `join()` 在 5 秒内答 `Ending::Stopped`，再调一次答同样的。
- 以上每条末尾 `counters()` 全是 0。
- `a_device_mtu_below_1280_is_refused`（`#[should_panic(expected = "at least 1280")]`）：`MemoryDevice::pair(1279, 16)`。

**判据**：用例绿（计时的单独循环跑二十次）；门禁绿。这是最该细读的一个任务（锁、唤醒、连接号、背压）。

## Task 3：基准门槛

**改哪里**

1. `src/testing/scripted.rs`：`ScriptedTcp`——经 `AppStack::divert` 收 TUN 发往 `ports` 的包、经 `AppStack::inject` 发自己的包的最小 TCP（只有 IPv4、来源 `198.18.0.1`，用 `smoltcp::wire` 构造，IPv4 首部校验和照算）：
   ```rust
   impl ScriptedTcp {
       pub fn new(app: &AppStack, ports: RangeInclusive<u16>) -> ScriptedTcp;
       /// SYN (MSS 1460, no other option) from each port to `dst`; on its SYN-ACK an ACK carrying
       /// `hello` bytes; resolves once every connection is established and its bytes acked.
       pub async fn open(&mut self, ports: RangeInclusive<u16>, dst: SocketAddr, hello: usize) -> io::Result<()>;
       /// Acknowledges everything from the TUN (window 65535) until every open connection has had `bytes`.
       pub async fn take(&mut self, bytes: usize) -> io::Result<()>;
   }
   ```
2. `src/tests/bench.rs`：两个忽略的用例（`#[ignore = "a measurement, not a gate: run on its own, in release"]`）与 `fn peak_resident_bytes() -> Option<u64>`（Windows：起 `powershell -NoProfile -Command "(Get-Process -Id <自己的 pid>).PeakWorkingSet64"` 读一个数；Linux：`/proc/self/status` 的 `VmHWM`；其余 `None`）。照 `crates/rurge-rules/tests/memory.rs` 的文件头写清怎样跑、读哪个数。
   - **B1 `b1_single_stream_throughput`**：每一行一个新的 `MemoryDevice::pair(mtu, 8192)` 与新的 TUN；rurge 一侧（TUN、`FakeDialer` 的转发、到上游的连接）全在第一个线程的 `current_thread` 运行时上；`AppStack` 与用例对它的读写在第二个线程的 `current_thread` 运行时上；`Upstream::sink` / `source` 在第三个线程上。上行：应用写 64 MiB（环境变量 `TUN_BENCH_MIB` 可改，同 `RELAY_BENCH_MIB` 的写法）到 `sink`，计时到 `sink` 收齐；下行：`source` 发同样多，计时到应用读齐。行：MTU 1500 / 9000 × `tx_max` 256 KiB / 1 MiB × 上行 / 下行；每个 MTU × 方向另一行**夹具自己的上限**：两个 `AppStack` 经 `AppEnd::crossed` 直接相连（发起的一个在第二个线程，只监听的一个在第一个线程，同样的传送）。每行打印 MTU、`tx_max`、方向、字节、用时、Gbit/s、是否不低于 0.7，与 `beyond_window`、`queue_full`、`rst_limited`、`stack_panics`；四个里有不为 0 的，那一行写 `not counted`（设计第 6 节：丢一个段会让 smoltcp 的发送方退回整个窗口重发），整张表照样打完。
   - **B2 `b2_ten_thousand_connections`**：一次一种设置，取自 `TUN_BENCH_SHARDS`、`TUN_BENCH_MIN`、`TUN_BENCH_TX_MAX`、`TUN_BENCH_BUDGET`、`TUN_BENCH_MTU`（没设的用 `TunOptions::default()` 与 1500），每种设置单独一个进程。rurge 一侧同 B1；应用一侧先经 `ScriptedTcp` 开 10,000 条连接（源端口 10000 ～ 19999、目的 `192.0.2.10:80`，`FakeDialer` 给 `Script::Memory { buffer: 1024, send: 8192 }`），每波至多 1000 次握手（半开的上限是 2048）、这一波都建成再开下一波，`hello` 1 KiB，`take(8192)`；然后 100 条活跃连接经 `AppStack`（源端口 40000 ～ 40099，目的 `192.0.2.20:80` → 回环上的 `echo`）各双向传 4 MiB。打印：设置、峰值常驻内存、测试一侧自己的估计（`AppStack` 的套接字缓冲、`ScriptedTcp` 的表、内存管道，各多少）、活跃传送期间每秒经 `AppEnd` 的包数（两个方向之和，即 rurge 那个线程每秒推过的包；每包的时间是它的倒数）、`queue_full`、`busy`、`rst_limited`、`beyond_window`、`stack_panics`（有不为 0 的写 `not counted`）。不报 CPU 时间（开发机上进程的 CPU 时间不可信）。
3. 选缺省值、改 `TunOptions::default()`（任务 2 的用例照旧绿）：MTU 两个都过门槛时取 1500（设计 M3-D3）；`tx_max` 取过 B1 的两个候选里较小的；`buffer_min`（4 / 8 KiB）与 `budget`（64 / 128 MiB）在 B2 不超过判据的组合里先取较大的 `buffer_min`、再取较大的 `budget`；片数（16 / 64 / 256）取 B2 里每秒包数最多的，与 64 相差不到一成时取 64。

**用例**

- `scripted_tcp_opens_and_takes`（不忽略、debug 下快）：100 条连接经 `ScriptedTcp` 开、各收齐 8 KiB；`dialed()` 有 100 项。
- 两个基准在 release 下跑：`cargo test -p rurge-tun --release --lib -- --ignored --exact tests::bench::b1_single_stream_throughput --nocapture`；B2 每种设置单独一个进程（设置经 `TUN_BENCH_*` 给出；读的峰值是进程到那一刻为止的峰值）。

**判据**

- B1：MTU 1500 与 9000 至少有一个，两个方向都不低于 0.7 Gbit/s，或不低于同设置下夹具上限的 80%（P8）；算数的行四个计数都是 0。
- B2：峰值常驻内存不超过 429,000,000 字节（"300 MB ÷ 0.7 ≈ 429 MB"取十进制，两种读法里较严的那一个），五个计数都是 0。
- 每一行的数字、选定的缺省值与量的机器写进报告，主持这一轮的会话记进「执行期修正记录」。
- 达不到时依次调：每次唤醒的界限（`INGRESS_PER_WAKE`、`EGRESS_PASSES`）、进片与写任务的队列、ACK 延迟（`set_ack_delay`）、`tx_max`、包的 `Vec` 复用；调过之后仍达不到：停下来交回数字。由主持这一轮的会话裁定转向自写的 TCP 被动一端（设计 M3-D9，总设计第 3.1 节已授权），改写任务 4 ～ 9；接口（包进、包出、连接以 `BoxedStream` 交出）不变。
- 两个基准不进门禁；门禁绿。

## Task 4：进协议栈之前的过滤与计数

**改哪里**

1. `src/packet.rs`：`pub(crate) fn classify(packet: &[u8], ipv6: bool) -> Result<Parsed, Reason>`，`Parsed` 带地址族、源与目的地址、协议与传输层的要素（TCP：端口、标志、序号、确认号、首部长度、负载长度；UDP：端口；ICMP：类型），`Reason` 是丢弃的原因（十种，与 `Counters` 里 `stack_panics` 之外的十个字段一一对应、同名；`icmp_limited` 在 M3a 里还没有谁用到）。依次检查（第一条成立的决定；P10）：

   | 检查 | 原因 |
   | ---- | ---- |
   | 版本不是 4 / 6；IPv4 的 IHL < 5（H4）、总长度小于首部或大于收到的字节数、首部校验和不对；IPv6 的负载长度大于收到的字节数 | `malformed` |
   | `ipv6` 关着时的 IPv6 包 | `unsupported` |
   | IPv4 的 MF 置位或偏移不为 0；IPv6 的下一个首部是分片首部（44）（H5） | `fragment` |
   | 源或目的是未指定地址（H1）、回环、组播；目的是 `255.255.255.255`；源或目的是 IPv4 映射的 IPv6 地址 | `martian` |
   | IPv6 的下一个首部不是 6 / 17 / 58（扩展首部）；IPv4 的协议不是 6 / 17 / 1 | `unsupported` |
   | TCP 首部截断（数据偏移 < 5 或越界）；UDP 首部截断或长度字段不对；ICMP 首部截断；TCP / UDP 的源或目的端口是 0 | `malformed` |
   | ICMP 不是回显请求（ICMPv4 type 8、ICMPv6 type 128） | `unsupported` |

   IPv4 总长度之后多出来的字节去掉。M3a 里 UDP 与 ICMP 回显请求过了这些检查之后计 `unsupported`（P11）。
2. `src/stats.rs`：`Stats::drop(&self, reason: Reason, src: Option<IpAddr>, dst: Option<IpAddr>)`：计数；每种原因第一次（判断写成纯函数 `Warnings::first_time(&self, reason) -> bool`）一行 WARN，文本恰好是 `tun: a packet was dropped (reason: {reason}); later drops for this reason are logged at debug level`（`{reason}` 是字段名，不带地址）；其余一行 DEBUG `tun: packet dropped`，字段 `reason`、`src`、`dst`（解析到了的）；`martian` 什么也不写。`queue_full` 与后面任务的原因同样经它。
3. `src/dispatch.rs` 改用 `classify`。

**接口**

- 产出：`packet::{classify, Parsed, Reason}`、`Stats::drop`（任务 5、6 用）；`TunOptions.ipv6` 生效。

**用例**（`src/tests/filter.rs`，包用 `smoltcp::wire` 现场构造）

- `classify` 的纯用例，表的每一行一条：`a_bad_version_or_header_is_malformed`、`ipv6_off_is_unsupported`、`fragments_are_dropped`、`martians_are_dropped`（未指定、回环、组播、广播、映射的各一）、`extension_headers_and_other_protocols_are_unsupported`、`truncated_transport_headers_and_port_0_are_malformed`、`icmp_other_than_echo_is_unsupported`、`the_checks_run_in_order`（发往组播地址的 IPv4 分片计 `fragment`，`ipv6` 关着时 IPv6 的分片计 `unsupported`）、`trailing_bytes_after_the_ip_packet_are_ignored`。
- 经 TUN（`AppStack::inject`）：`a_syn_from_the_unspecified_address_is_dropped`（H1：smoltcp 收下它会在发 SYN-ACK 时 panic）、`a_short_ipv4_header_is_dropped`（H4）、`a_bad_packet_harms_nothing`（每种坏包各注入一次 → 各自的计数各 1，应用一侧什么也没收到，之后一条正常的连接照常往返）、`ipv6_off_serves_ipv4_only`（`ipv6: false`：IPv6 的 SYN 计 `unsupported`、得不到回应，IPv4 照常）、`udp_and_icmp_are_not_handled_yet`。
- `each_reason_warns_once`（`src/tests/log_capture.rs` 照 `crates/rurge-engine/tests/common/log_capture.rs` 写一份；当前线程的运行时）：`malformed`、`fragment`、`martian`、`unsupported` 各两个包 → `martian` 之外每种恰好一行 WARN、第二个是 DEBUG，`martian` 什么也不写；WARN 的行里没有 `192.0.2.`、`198.18.`、`2001:db8:`、`fd00:`。`each_reason_is_first_once`：`Warnings::first_time` 的纯用例，十种原因里 `martian` 之外的九种各自第一次是 `true`、之后是 `false`。

**判据**：用例绿；门禁绿。

## Task 5：SYN 的去处与上限

**改哪里**

1. `src/space.rs`：`pub(crate) enum Place { Responder, Own, Fake, Real }` 与 `pub(crate) fn place(ip: IpAddr) -> Place`（设计第 4.2 节）：`198.18.0.2` ～ `198.18.0.9` 与 `fd00:6152::2` 是 `Responder`；`198.18.0.0/24` 与 `fd00:6152::/120` 里其余的是 `Own`（含虚拟网卡的 `.1` / `::1`、`.244`、协议栈的 `.253` / `::fd`）；`198.18.0.0/15` 与 `fd00:6152::/64` 里其余的是 `Fake`；其余 `Real`。常数 `ADAPTER_V4`、`ADAPTER_V6`、`STACK_V4`、`STACK_V6` 放在这里（任务 2 的接口地址与任务 1 的 `AppStack::start` 改用它们）。
2. `src/limit.rs`：`pub(crate) struct Limiter`：`new(max: usize, window: Duration)`、`allow(&mut self, now: Instant) -> bool`（滑动窗口：`now` 之前 `window` 之内已放行的不到 `max` 个才放行，放行时记下 `now`；至多记 `max` 个时刻）。rurge 自己的 RST 共用一个 `Limiter::new(100, 3 s)`（清单 L334）。
3. `src/packet.rs`：`pub(crate) fn rst_for(syn: &[u8]) -> Vec<u8>`：地址与端口对调，`RST|ACK`，序号 0，确认号 = SYN 的序号 + 1 + 负载长度（回绕），窗口 0，没有选项；IPv4 的 TTL 与 IPv6 的跳数 64；IPv4 首部与 TCP 的校验和照算（应用的内核要验）。
4. 新的 SYN 在第 5 步之前依次（设计第 5.2 节第 1 ～ 3 步；第 4 步预匹配在 M3c）：TUN 在停止 → RST；套接字已有 `max_sockets` 个或半开的已有 `max_half_open` 个 → RST、计 `busy`；目的地是 `Responder` 或 `Own` → RST（应答器的 53 端口在 M3c 另有去处），是 `Fake` → RST、计 `unknown_fake`（M3a 没有池），是 `Real` → 第 5 步。这几处的 RST 都经限频，超出的不发、计 `rst_limited`；RST 经写任务写出。
5. 半开：SYN 得到套接字时加一；`established()` 看到 `Established`、或连接在那之前被拿掉时减一。建成之前过了 `half_open_timeout` 的 `abort()`（smoltcp 对应用回 RST）并拿掉。
6. 推进一个包之后，这条连接的套接字回到了 `Listen`（`SynReceived` 时收到 RST，smoltcp 把它放回监听；它的四元组没了，会接下另一个来源的 SYN）→ 当场拿掉。
7. 四元组已有套接字的 SYN（重发的）照普通的段交给 smoltcp（任务 2 已是这样，这里加用例）；MSS 选项为 0 的 SYN smoltcp 不接受，按任务 2 的"推进之后不在 `SynReceived` 就拿掉"处理（加用例）。

**接口**

- 消费：任务 2 的片与会话、任务 4 的 `classify` / `Stats::drop`。产出：`space::{place, Place}` 与四个地址常数、`limit::Limiter`（M3b 的 ICMP 限频照用）、`packet::rst_for`。

**用例**（`src/tests/syn.rs`；要看 RST 的用例经 `AppStack::divert` 收、`inject` 发）

- `simultaneous_syns_to_one_endpoint_each_get_their_own_connection`（评审要点 1）：8 条连接同时开往 `192.0.2.1:443` → 都建成；`dialed()` 有 8 项、8 个不同的源端口；第 *i* 条发 1000 个字节 *i*，读回的恰是自己的。
- `a_lost_syn_ack_is_recovered`（评审要点 4）：过滤器丢掉第一个 SYN-ACK → `connect` 在 5 秒内照样成功；`dialed()` 只有 1 项。
- `stopping_answers_a_syn_with_a_reset`：片一级的用例（停止的标志已置）：新的 SYN 得到 RST、不建套接字。
- `the_socket_cap_resets_new_syns`（`max_sockets: 4`）：第 5 个 SYN 得到 RST，`busy` 1，前 4 条照常往返。
- `the_half_open_cap_resets_new_syns`（`max_half_open: 3`）：4 个不完成握手的 SYN，第 4 个得到 RST，`busy` 1。
- `a_half_open_connection_times_out`（`half_open_timeout: 300 ms`）：一个不完成握手的 SYN → 有界时间里应用一侧收到 RST，`sockets_in_use()` 回到 0。
- `syns_to_rurge_addresses_are_reset`：`198.18.0.1:80`、`198.18.0.2:53`、`198.18.0.244:443`、`198.18.0.253:80`、`[fd00:6152::fd]:80` 各得到 RST、没有拨号；`198.18.1.1:80` 与 `[fd00:6152::c612:101]:80` 各得到 RST、`unknown_fake` 2。
- `resets_are_limited_to_100_in_3_seconds`：`Limiter` 的纯用例（造出来的时刻：100 个放行、第 101 个不放、最早的一个出了窗口之后再放一个）；经 TUN：101 个发往 `198.18.0.1` 的 SYN 得到 100 个 RST，`rst_limited` 1。
- `a_reset_in_syn_received_removes_the_socket`：一个 SYN，再一个 RST（序号是 SYN 的加一）→ `sockets_in_use()` 立刻是 0（不等扫描）；之后另一个源端口发往同一个目的端点的 SYN 是它自己的连接（`dialed()` 的源端口对得上）。
- `a_syn_with_mss_zero_gets_no_connection`：没有套接字、没有拨号。
- `the_reset_answers_the_syn`：`rst_for` 的纯用例：地址与端口对调、`RST|ACK`、序号 0、确认号是 SYN 的序号加一加负载长度（含序号回绕）、窗口 0、没有选项、IPv4 与 TCP 的校验和对（用 `smoltcp::wire` 验）；IPv6 同样。

**判据**：用例绿（计时的循环跑二十次）；门禁绿。

## Task 6：smoltcp 0.12 的隐患——SACK、窗口边缘的守卫、片的重建

**改哪里**

1. `src/packet.rs`：`pub(crate) fn strip_sack_permitted(packet: &mut [u8])`（整个 IP 包）：带 SYN 的 TCP 首部里每个 SACK-permitted 选项（kind 4、长度 2）换成两个 NOP（kind 1）；选项表本身不合规矩的原样不动（H3）。片推进任何带 SYN 的段之前调它（新的与重发的）；TCP 的校验和不验，所以改了不重算。
2. `src/tcp/guard.rs`（H2 的兜底，设计第 5.3 节）：`ShardDevice` 记每个四元组最新的边缘；发的一侧的 `TxToken::consume` 写完每一个 TCP 段就读它：边缘 = 确认号 + (窗口字段 << 缩放位数)，缩放位数取 rurge 那个 SYN-ACK 里的窗口缩放选项（没有是 0；带 SYN 的段本身不缩放）。推进一个带负载的段之前：知道边缘、且负载的末端（序号 + 负载长度）在序号空间里越过它 → 丢弃、计 `beyond_window`；不带负载的段（ACK、只带 FIN 的）与还不知道边缘的照常进去。拿掉连接时删它的边缘。比较写成纯函数 `fn past(end: u32, edge: u32) -> bool`（回绕：`end.wrapping_sub(edge) as i32 > 0`）。
3. 重建：片在锁里做的事在 `std::panic::catch_unwind(AssertUnwindSafe(..))` 里；panic 了，这一片换一套新的 `Interface` / `SocketSet` / `ShardDevice`（新的 `Interface` 另取一个种子：沿用旧的会重复旧栈的初始序号，执行期修正记录第 7 行），它的每条连接以 `ConnectionReset` 结束（`ConnShared` 记下、唤醒等待者）、还回预算、套接字与半开的计数随之减；`stack_panics` 加一；一行 WARN，文本恰好是 `tun: the TCP stack of a shard panicked; its connections are reset`，字段 `shard`（片号），不带 panic 的文本（那一句在一行 DEBUG 里）。任何一处拿这一片的锁时发现它中毒了（别处拿着它时 panic 了）：先照样重建、`clear_poison`，再接着做。不装 panic 钩子（设计第 5.3 节"最后一道"）。
4. 只在 `cfg(test)` 下编译的两个触发点（P20）：片推进的一个段的负载以 `PANIC_MARKER`（`b"rurge-tun test: panic"`）开头时在受保护的那一段里 panic；`TunStream::poll_write` 的缓冲以它开头时拿着锁 panic。

**接口**

- 消费：任务 2 的片、任务 4 的 `Stats::drop`。产出：`packet::strip_sack_permitted`、`guard::past`。

**用例**（`src/tests/hazards.rs`）

- `sack_permitted_is_not_offered_back`：`AppStack` 的 SYN 带 SACK-permitted（smoltcp 作发起方总是带，`tcp.rs` 第 2382 行）→ 过滤器看到的 SYN-ACK 没有它。
- `out_of_order_data_gets_no_sack_blocks`：过滤器扣下一个数据段、放过它后面的一个再放它（乱序）→ 数据完整到达；TUN 发出的 ACK 没有一个带 SACK 选项；debug 构建里不 panic。
- `strip_sack_permitted_leaves_other_options`：MSS、窗口缩放、SACK-permitted、时间戳 → 只换了 SACK-permitted；长度字段越界的选项表原样。
- `a_segment_past_the_edge_is_dropped`：一条连接建好之后注入一个这个四元组的数据段，序号越过 TUN 最近一次通告的边缘（过滤器从 TUN 发出的段读）→ `beyond_window` 1；这条连接照常往返。
- `past_wraps_around`：`past` 的纯用例（`2^32` 附近）。
- `an_upload_into_a_stalled_upstream_resumes`：`Script::Hold { buffer: 1024 }`；应用写 1 MiB；5 秒之内过滤器看到 TUN 为这条连接通告窗口 0；`release` → 30 秒内 `received` 到 1 MiB；`beyond_window`、`stack_panics` 都是 0。
- `a_window_probe_into_a_full_buffer_is_dropped`：同上停在窗口 0；注入这个四元组的一个段：序号是 TUN 最近一次的确认号（零窗口的边缘）、确认号是 TUN 的下一个序号、负载 1 个字节（Windows 与 BSD 的协议栈的零窗口探测段就是这样；smoltcp 作发送方不发探测段，所以由用例构造）→ `beyond_window` 1、`stack_panics` 0；`release` 之后 1 MiB 照样到齐。
- `a_probe_after_a_read_is_inside_the_new_window`：片一级的用例（用例自己推进这一片）：接收缓冲满、TUN 已通告窗口 0；读走 1 KiB（`TunStream` 的读，不经转发），下一次唤醒里的第一个包是上一条那样的探测段 → 发出的包里一个窗口不为 0 的 ACK 排在探测段被推进之前，探测段在新窗口之内、被收下；`beyond_window` 0、`stack_panics` 0（去掉任务 2 的"先推进一次发送"时这一条变红）。
- `a_panicking_shard_resets_its_connections_and_recovers`（`shards: 1`）：连接 A 建好；注入 A 的、负载以 `PANIC_MARKER` 开头的段 → `stack_panics` 1；A 的转发结束（`relay_ends()` 多一项错误）；之后新开的连接 B 照常往返；WARN 恰好一行、带 `shard=0`、不含标记的文字，DEBUG 里有它（日志捕获）。
- `a_poisoned_shard_is_rebuilt`（`shards: 1`）：上游（用例自己在回环上起的服务器，经 `Script::Upstream`）发出以 `PANIC_MARKER` 开头的数据 → 转发往 `TunStream` 写时 panic、锁中毒 → 片下一次拿锁时重建：`stack_panics` 1，同片的另一条连接以 `ConnectionReset` 结束，新连接照常。
- 基准复跑：B1 在缺省设置、MTU 1500 下再跑一遍（release），`beyond_window` 是 0、两个方向不低于任务 3 记下的数的 90%，数字写进报告。

**判据**：用例绿；门禁绿；基准复跑的结论在报告里。

## Task 7：流的一生

**改哪里**（`src/tcp/stream.rs`、`src/tcp/shard.rs`）

1. `TunStream` 被丢掉：没有 `shutdown`、或应用还可能发（`may_recv`）、或还有没读的数据（`recv_queue() > 0`）→ `abort()`（RST）；否则不动它（`poll_shutdown` 已经排了 FIN），记下丢掉的时刻。
2. 拿掉套接字：smoltcp 报 `Closed` 或 `TimeWait`（`TimeWait` 照任务 2 等 20 毫秒）；或流被丢掉之后过了 `linger`（那时先 `abort()`）；`abort()` 过的套接字（这里的 `linger`、第 1 点的丢掉、任务 8 的拒绝）要等它的 RST 发了出去——`Closed` 且 `remote_endpoint()` 是 `None`（smoltcp `tcp.rs` 第 2537 ～ 2540 行：发出那一个 RST 之后才忘掉四元组）——才拿掉，否则应用收不到 RST（片重建时整个套接字集一起丢掉，不在此列，见任务 6）——片醒来时看本轮碰过的连接，另每秒扫一遍全片。拿掉时删两张表的项与它的边缘、还回预算、半开的照任务 5 减、唤醒等待者。
3. 结局（P12）：应用的 FIN 读空之后读是 `Ok(0)`；应用的 RST（smoltcp 照着做了：这个段之后套接字从别的状态变成 `Closed`、段带 RST）之后读写都是 `ConnectionReset`；连接因别的缘故被拿掉（重建、`linger`、停止）之后同样 `ConnectionReset`；应用关了接收方向之后的写是 `BrokenPipe`。

**接口**

- 消费：任务 2 ～ 6 的片、`ConnShared`、预算。产出：`TunStream` 的完整规矩（任务 8 的 `REJECT-DROP` 照用它的 `poll_shutdown`）。

**用例**（`src/tests/lifecycle.rs`）

- `a_graceful_close_leaves_no_socket`：双方各自关写端 → 有界时间里 `sockets_in_use()` 0、`budget_in_use()` 0。
- `a_reader_that_stops_and_resumes_loses_nothing`（评审要点 2）：从 `source` 下行 4 MiB；应用读 64 KiB 后停下，等到 `source` 的已写字节数 200 毫秒不再增长（"什么都没发生"的等待：TUN 的发送缓冲满了、转发循环停了），再读完 → 4 MiB 逐字节等于 `pattern`。
- `an_app_reset_mid_transfer_ends_the_session`（评审要点 3）：上行到 `sink` 的中途应用 `abort()` → 5 秒内 `relay_ends()` 多一项错误、`sockets_in_use()` 0、`sink` 那条连接结束。
- `two_thousand_connections_in_a_row_leave_nothing_behind`（评审要点 5；`shards: 1`，好让句柄真的被复用）：2000 条连接一条接一条（各 1 KiB 往返、`shutdown`、读到 `Ok(0)`）→ 最后一条之后有界时间里 `sockets_in_use()`、`half_open()`、`budget_in_use()` 都是 0，连接表是空的；`dialed()` 2000 项、每条读回的都是自己的数据。
- `a_stream_whose_connection_is_gone_touches_no_other`：片一级的用例：A 的 `TunStream` 留在手里，A 被拿掉，B 建在复用了 A 的句柄的套接字上；A 读写都以 `ConnectionReset` 结束，B 的数据不受影响。
- `a_dropped_stream_lingers_then_goes`（`linger: 300 ms`）：`shutdown` 之后丢掉流、过滤器吞掉 TUN 发往这条连接的一切（FIN 得不到确认）→ 有界时间里套接字被拿掉、计数回到 0。
- `fin_and_reset_reach_the_relay_as_eof_and_error`：应用 `shutdown` → `echo` 读到 EOF、转发以 `Ok` 结束；应用 `abort` → 转发以错误结束。

**判据**：用例绿（计时的循环跑二十次）；门禁绿。

## Task 8：握手之后的拒绝、停止与设备出错

**改哪里**

1. `src/session.rs`：`Reject { kind: RejectKind::Drop, .. }` → 拿着 `TunStream` 不读不写 `drop_hold`（与停止 `select`），然后 `shutdown`（FIN）再丢掉；其余拒绝与拨不通照旧 RST（设计第 5.6 节）。
2. `TunHandle::stop`：置停止（新的 SYN 回 RST，任务 5）；每一片 `abort()` 它的全部套接字、再推进一次发送、把这些 RST 交给写任务；写任务写完排着的再结束；收包的任务不再读设备；全部任务（含会话的）结束时 `join` 答 `Ending::Stopped`（`TaskTracker::close` 加 `wait`）。不取消正在进行的拨号（P15）。
3. 设备出错（`recv` 或 `send`）：同样的结束流程，发送出错之后不再写；一行 WARN，文本恰好是 `tun: the device failed; the TUN has ended`，字段 `error`；`join` 答 `Ending::DeviceFailed(<错误的文字>)`。

**接口**

- 消费：任务 2 的 `TaskTracker`、写任务与片，任务 7 的 `TunStream`。产出：`stop` / `join` / `Ending` 的完整规矩（M3b 的引擎停下时照用）。

**用例**（`src/tests/outcomes.rs`）

- `reject_drop_holds_then_closes`（`drop_hold: 300 ms`）：`Script::Reject(RejectKind::Drop)` → `connect` 成功；读到 `Ok(0)` 不早于 300 毫秒、5 秒之内；其间没有数据。
- `the_other_rejections_reset`（`drop_hold: 10 s`）：`Reject`、`NoDrop`、`TinyGif` 各一条 → 1 秒之内 `ConnectionReset`。
- `stop_during_a_drop_hold_ends_it`（`drop_hold: 30 s`）：拿着的时候 `stop()` → `join()` 5 秒之内答 `Stopped`。
- `stop_resets_every_open_connection`：3 条空闲的连接 → `stop()` → 每条在应用一侧都是 `ConnectionReset`；`join()` 5 秒之内答 `Stopped`；`relay_ends()` 3 项。
- `a_device_that_fails_ends_the_tun`：开着 2 条连接时 `AppStack::close()`（丢掉 `AppEnd`）→ `join()` 5 秒之内答 `DeviceFailed(_)`；`relay_ends()` 2 项；WARN 恰好一行（日志捕获，当前线程的运行时）。
- `join_answers_the_same_again`：两次 `join()` 答同样的 `Ending`。

**判据**：用例绿（计时的循环跑二十次）；门禁绿。

## Task 9：文档

- CLAUDE.md：状态一段加 M3a 一段（做了什么；门槛的数字与选定的缺省值；没做的与不要当成已经有的——UDP、ICMP、fake-IP 池、DNS 应答器、引擎接入、真实设备都还没有，`rurge run` 不变，数字只在一台 Windows 11 x86-64 的开发机上量过）；「先读这些文档」加 M3 设计与本计划；依赖方向加 `rurge-tun → { rurge-inbound, rurge-proto, rurge-net, rurge-config }`（不依赖引擎；M3b 起引擎依赖它）；常用命令加 `rurge-tun` 的过滤词（`tests::tcp`、`tests::syn`、`tests::filter`、`tests::hazards`、`tests::lifecycle`、`tests::outcomes`）与两个基准的跑法。
- 设计第 18 节加第 11 条：基准门槛的结论与数字、选定的缺省值（设计第 4.1、5.1、5.4 节与 M3-D3、D6 的候选在这里定下）。
- 本计划末尾两张表填齐。
- 兼容性清单、`docs/api/phase3.md`、手工清单与 README 随 M3c（设计第 16 节），这一份不改。

**判据**：`git grep` 核对文档里引用的用例名、路径与常数都存在；门禁绿。

---

## 计划期决定

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| P1 | 设计写"第一个任务是基准门槛" | 拆成三个任务：夹具（任务 1）、最小的一条路（任务 2）、门槛（任务 3）——夹具与 TUN 的最小路径各自能被评审拒绝；门槛不过时只改写任务 4 ～ 9，前三个任务的设备、夹具、分发与写任务留用 |
| P2 | `Tun::start` 的参数（设计第 3.3 节有 `hooks`、`pinger`、`pool`） | M3a 只有 `device`、`dialer`、`options`：那三样在 M3a 里没有用处；M3b 加参数时只改各测试文件里起 TUN 的辅助函数 |
| P3 | 用例放在哪里、基准怎样跑 | 全是 crate 里的单元用例（`src/tests/`），`testing` 模块在 `cfg(any(test, feature = "testing"))` 下编译，`cfg(test)` 的触发点只在 crate 自己的测试构建里；M3b 起引擎的用例经 `testing` 特性用夹具。两个基准在 release 下用 `cargo test -p rurge-tun --release --lib -- --ignored --exact tests::bench::<名字> --nocapture` 跑——全计划唯一用 `-p` 的地方，只为不把整个工作区编成 release（`target/` 长得很快） |
| P4 | 夹具自己的上限（设计第 6 节"应用端的栈与另一个只监听的 smoltcp 栈直接相连"） | 两个 `AppStack` 经 `AppEnd::crossed` 相连，线程的分布同 rurge 的那一行 |
| P5 | 两个基准一次跑几种设置 | B1 一个用例打完整张表（每行一个新的 TUN；峰值内存不是它的判据）；B2 每种设置一个进程，设置取自 `TUN_BENCH_*`（同 `RELAY_BENCH_MIB` 的写法），读的峰值是进程到那一刻为止的峰值 |
| P6 | 缺省值的选法 | MTU：两个都过取 1500（M3-D3）；`tx_max`：过 B1 的较小者；`buffer_min` 与 `budget`：B2 不超过判据的组合里先取较大的 `buffer_min`、再取较大的 `budget`；片数：B2 里每秒包数最多的，与 64 相差不到一成时取 64 |
| P7 | rurge 自己构造的 RST | 回应 SYN：地址与端口对调，`RST|ACK`，序号 0，确认号 = SYN 的序号 + 1 + 负载长度（回绕），窗口 0，没有选项；IPv4 TTL 64、IPv6 跳数 64；IPv4 与 TCP 的校验和照算（应用的内核要验） |
| P8 | 门槛怎样算过、怎样调 | 每个方向不低于 0.7 Gbit/s，或不低于同设置下夹具上限的 80%（设计"rurge 的数字接近它时瓶颈在夹具"按 80% 读：rurge 那一程多一次转发与一段回环 TCP）。调的次序：每次唤醒的界限（`INGRESS_PER_WAKE`、`EGRESS_PASSES`）、进片与写任务的队列、ACK 延迟、`tx_max`、包的 `Vec` 复用；调过之后仍不过，由主持这一轮的会话裁定转向（M3-D9） |
| P9 | B2 的判据"300 MB ÷ 0.7 ≈ 429 MB" | 取十进制：429,000,000 字节（约 409 MiB）——两种读法里较严的那一个 |
| P10 | 过滤的次序 | 任务 4 的表的次序。分片在 `martian` 与传输层首部之前（偏移不为 0 的分片里没有传输层首部，先认分片才不会把它算成 `malformed`）；`ipv6` 关着时在读 IPv6 首部的其余部分之前就丢；端口 0 计 `malformed` 是计划加的（设计的表里没有）：它不是可用的端口，smoltcp 的 `listen` 对目的端口 0 出错，M3b 的 UDP 会话也不该为它建 |
| P11 | UDP 与 ICMP 在 M3a | 过了过滤之后计 `unsupported`（M3b 起分发）；UDP 与 ICMP 的校验和随 M3b 验 |
| P12 | `TunStream` 与 `AppTcp` 的结局 | FIN 读空之后读是 `Ok(0)`；smoltcp 照着做了的 RST（段带 RST、这个段之后套接字从别的状态变成 `Closed`）之后读写都是 `ConnectionReset`；因别的缘故被拿掉之后也是 `ConnectionReset`；对端关了接收方向之后的写是 `BrokenPipe`。smoltcp 自己不说"是被重置的"，所以由片在推进那个段时看 |
| P13 | `poll_flush` | 立刻返回：引擎的转发循环每次写后 flush（`crates/rurge-engine/src/relay.rs`），flush 若等对端确认，每次写都要等一个往返 |
| P14 | 半开的计数、拿掉套接字 | 半开由会话的任务的 `established()` 减（它看到 `Established`，或连接在那之前结束）；拿掉由片做：本轮碰过的连接当场看，另每秒扫一遍全片（smoltcp 自己的定时器把连接推进 `Closed` 时没有包可看） |
| P15 | 停止与正在进行的拨号 | 停止不取消拨号：拨号有自己的时限，中途丢掉引擎的 `dial` 会留下没结束的 `SessionHandle`；拨号回来时 `TunStream` 已被 `abort`，转发当场结束。`REJECT-DROP` 的拿着随停止结束。`join` 等 `TaskTracker` 里的全部任务 |
| P16 | 队列与每次唤醒的界限 | `shard_queue` 1024 个包（不少于设计第 6 节"下行约 720 个"）、`writer_queue` 256 批、`RECV_BATCH` 256、`INGRESS_PER_WAKE` 256、`EGRESS_PASSES` 64；写任务的队列满时片等着、不丢，所以 `queue_full` 只出在进片的队列上 |
| P17 | 设计 V1 ～ V4、V11 ～ V13 的核对 | smoltcp 0.12.0：`poll_ingress_single` / `poll_egress` / `poll_delay` 的签名与 `PollIngressSingleResult { None, PacketProcessed, SocketStateChanged }`、`PollResult { None, SocketStateChanged }`（`iface/interface/mod.rs` 第 73 ～ 100、436 ～ 569 行）；`RxToken::consume` 收 `&[u8]`、`TxToken::consume(len, &mut [u8])`（`phy/mod.rs` 第 370 ～ 395 行）；`ChecksumCapabilities { ipv4, udp, tcp, icmpv4, icmpv6 }`；没有套接字接下的 TCP 段由接口回 RST、来源是那个包的目的地址，RST 与来源或目的是未指定地址的段不回（`iface/interface/tcp.rs` 第 21 ～ 43 行）；`tcp::Socket` 的 `set_state` 每次都唤醒收发两个 waker（`tcp.rs` 第 1315 ～ 1330 行），所以 `established()` 登记一个 waker 就够；作发起方的 smoltcp 总带 SACK-permitted、作监听方只在对方带了时回（第 2382 ～ 2384 行）；发出每一段时记下 `remote_last_ack` 与 `remote_last_win`（第 2520、2521 行；接收时直接回的段在第 1385、1391 行，同样经 `TxToken` 写出），所以守卫从写出的段读的边缘就是 smoltcp 记的那一个；读走数据只让 `remote_seq_no` 前移、什么也不发（第 1234 行），窗口更新等下一次推进发送（`window_to_update`，第 2232 行起：上次通告的窗口是 0 时任何新窗口都发）——这是计划期决定 P21 的由来；作发送方的 smoltcp 在对方窗口为 0 时什么也不发、不发零窗口探测段（`seq_to_transmit`，第 2147 ～ 2162 行），所以应用端夹具不会发探测段，探测段的用例由 `inject` 构造；`register_recv_waker` / `register_send_waker`、`recv_slice`、`send_slice`、`recv_queue`、`send_queue`、`may_recv`、`may_send`、`close`、`abort`、`set_ack_delay`、`set_keep_alive`、`set_timeout`、`set_nagle_enabled`、`set_congestion_control` 都在（第 591 ～ 1311 行）。tokio 1.53.1 有 `copy_bidirectional_with_sizes`（`io/util/copy_bidirectional.rs` 第 94 行）与 `io::duplex`（`io/util/mem.rs` 第 104 行）。WireGuard 栈可照搬的是接口的建法（第 111 ～ 135 行）、`at`（第 168 行）、`advance`（第 249 行）、`Queues`（第 464 ～ 535 行）；它的接收缓冲是 256 KiB（`TCP_BUFFER`），本 crate 不照搬这一处（延后事项第 1 行）。V4 的引擎一侧（`SessionHandle` 由 `Engine::dial` 建）与 V12（会话的日志行由引擎写）随 M3b；M3a 的假 `Dialer` 自己建 `SessionHandle`、拒绝与失败时先结束它再答，同引擎 |
| P18 | `AppStack` 的缓冲 | 接收 `RX_MAX`、发送 256 KiB：它也是 smoltcp，接收缓冲超过 65535 时它自己就会碰上 H2。所以下行的窗口受它的 64 KiB 所限，`tx_max` 的两个候选在 B1 里的差别主要在 B2 的内存，不在吞吐 |
| P19 | 假 `Dialer` 与回环 | 只拨用例写给它的回环地址（`script` 写了别的就 panic），没写的目的地答失败；`rurge-tun` 的用例因此不需要设计第 15.2 节的"两道防线"（那是 M3b 起经真实引擎的用例的规矩） |
| P20 | `cfg(test)` 的 panic 触发点 | 只在 crate 自己的测试构建里编译，产品代码与依赖它的 crate 的构建里都没有；用它而不用别的办法，是因为 smoltcp 0.12 的已知 panic 都被前面几道挡住了，没有一种包能在正常构建里稳定地让它 panic |
| P21 | 每次拿锁做事先推进一次发送 | 设计第 5.1 节写的是"一批处理完再推进发送"；计划在每次拿锁做事的开头先加一遍 `poll_egress`。缩放位数为 0 时 H2 仍有一条路：零窗口之后应用读走了数据、窗口更新还没发出，对端（Windows 与 BSD 的协议栈）带一个字节的探测段先进了协议栈，smoltcp 收下它（缓冲里有了空位），`next_ack` 越过上次通告的边缘，下一次推进时 `last_scaled_window` 下溢而 panic。读只在片放锁的时候发生，所以先推进一次发送，窗口更新就总排在新收的包之前；守卫照样兜底不守规矩的段。代价是每次唤醒多扫一遍这一片的套接字（缺省每片至多 256 个） |
| P22 | 设备的 MTU | `Tun::start` 对小于 1280 的 MTU panic（H8）：MTU 由 rurge 自己定（M4 建设备时），小于它是编程错误，不是运行期会遇到的情形 |

## 执行期修正记录

| # | 任务 | 修正 | 提交 |
| - | ---- | ---- | ---- |
| 1 | 1 | `AppStack::accept` 由 `async fn` 改为立刻登记监听、返回 future 的普通函数：`async fn` 的监听要到第一次被轮询才建，在那之前到达的 SYN 得到 RST，多线程运行时上"起一个任务去 `accept`、随即 `connect`"会偶发失败；调用处的 `.await` 不变 | `aee6cb8` |
| 2 | 1、2、7 | 拿掉 `TimeWait` 的套接字之前等 20 毫秒（应用端夹具与 TUN 的片都是）：smoltcp 的 ACK 延迟缺省 10 毫秒，对方的数据与 FIN 在 10 毫秒之内先后到达时，套接字带着一个没发的 ACK 进 `TimeWait`，当场拿掉的话对方收不到确认、重发 FIN 之后得到 RST。任务 1 的评审读 smoltcp 0.12 的源码查出 | `aee6cb8`（夹具）、`bd90bd9`（片）；任务 7 |
| 3 | 2 | TUN 的套接字开保活（`TunOptions::keep_alive`，缺省 30 秒），超时取 600 秒（`TunOptions::timeout`）。设计第 5.4 节与 M3-D6 写的是"不开保活、超时 120 秒"，以为超时只管"发出的数据 120 秒没有确认"；smoltcp 0.12 的超时不看有没有在途的数据：应用最后一个段之后 `timeout` 里什么也没来就断开（`tcp.rs` 第 2118 ～ 2123、2274 行；发送缓冲从空变非空时才从发出的第一段重新起算，第 1170 ～ 1175、2258 ～ 2267 行），任务 2 的实现者用片一级的探针量到握手之后什么也不发的连接在 120 秒时被 RST。照设计的写法，经 TUN 的空闲长连接（SSH、数据库连接池、空闲的 HTTP/2）两分钟就断。保活每空闲 `keep_alive` 发一个保活段（第 2496 ～ 2498 行），活着的应用（它的内核）总会答 ACK，空闲的连接就不再被断开，应用那一侧没了的在 `timeout` 之后断开；保活段的回答带着应用当前的窗口，smoltcp 不发零窗口探测段（P17），应用的窗口更新丢了时下行至多停 `keep_alive`。600 秒与转发循环的空闲超时缺省（`--idle-timeout`）相同：Windows 的单调时钟把睡眠算进去，机器睡过 `timeout` 之后醒来时这一条断开每条连接，取 600 秒时它不比转发循环断得多。参照：netstack-smoltcp 与 shadowsocks-rust 的 TUN 都配保活（前者 28 秒），超时 7200 秒 | `6d02ec8` |
| 4 | 1、2、5、8 | 应用端夹具的 `connect` 在收下 SYN-ACK 时就算成功（同真实系统），紧随其后的 RST 在第一次读写时以 `ConnectionReset` 出现：此前只看套接字的终态，TUN 握手之后立刻发来的 RST 若在用例的任务第一次看套接字之前被收下，`connect` 答 `ConnectionRefused`（任务 2 的两条重置用例单独跑 30 次红 3 次）；任务 5 的上限与任务 8 的拒绝同样是"握手之后立刻重置"。拨号在握手完成之后才发生，由任务 2 的 `nothing_is_dialed_before_the_handshake_completes` 钉住 | `6d02ec8` |
| 5 | 2、7 | 任务 2 的 `TunStream` 被丢掉时照任务 7 第 1 点的条件（写的一侧已 `shutdown`、应用不会再发、没有没读的数据时不动它，否则 `abort()`），不是"一律 `abort()`"：转发循环关了写的一侧随即丢掉流，一律 `abort()` 的话 RST 顶掉还没发出的数据与 FIN，任务 2 自己的 `half_close_keeps_the_other_direction` 与 `a_mebibyte_crosses_each_way` 红；`linger` 照旧在任务 7 | `bd90bd9` |
| 6 | 2 | `an_idle_connection_outlives_the_timeout` 与 `a_silent_app_is_reset_after_the_timeout` 用保活 100 毫秒、超时 3 秒，空闲等 6 秒（两倍于超时），不是 1 秒与 3 秒：应用端夹具是 smoltcp，TUN 的保活段（序号小一、一个字节）落在它的接收窗口之外，它以 challenge ACK 回答，而 challenge ACK 每秒至多一个（smoltcp 0.12 `tcp.rs` 第 1433 ～ 1446、1642 ～ 1656、1680 行），超时 1 秒时 TUN 恰在它下一次能答之前断开（实现者的临时探针：应用只答了 121 毫秒那一个保活，TUN 在 1137 毫秒发出 RST）；超时 3 秒给应用一侧留约 2 秒的调度余量（2 秒只留约 0.9 秒）。真实系统把带一个字节的保活段当作重复的段答 ACK，产品的缺省（30 秒 / 600 秒）离这样的限频很远；代价是 `rurge-tun` 的单元测试二进制至少跑 6 秒 | `6d02ec8`、`a923c45` |
| 7 | 2、6、7 | 任务 2 的第二轮修正改了几处后面的任务要知道的地方：预算的"取"是一个原子步骤（`fetch_update`，按被替换的值算份额），文档写的最坏总量在各片于不同的工作线程上同时取时也成立（此前一次 `load` 加一次 `fetch_add`，缺省设置下最多多出约 12 MiB）；smoltcp 的种子在 `Tun::start` 里每片各取一次、经 `Shard::new` 交给片，随机源失败时每次起动一行 WARN（`tun: the system's random source failed; the TUN's initial sequence numbers are predictable`），不再悄悄用一个常数，任务 6 重建一片时另取种子；`try_state()` 改名 `state_unless_poisoned()`（与 `state()` 一样等锁，只在锁中毒时不同），任务 6 的中毒路径照用；smoltcp 自己因超时关掉的套接字上 `poll_shutdown` 答 `ConnectionReset`，与读的一侧相同（P12）；起 TUN 的测试辅助多了 `rig_on(device, options)`，`rig` 经它；`src/tests/lifecycle.rs` 已建，任务 7 的用例接着写在里面 | `a923c45` |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | WireGuard 出站（阶段 2 / M4b）的 smoltcp 接收缓冲是 256 KiB（`crates/rurge-proto-wireguard/src/stack.rs` 的 `TCP_BUFFER`，缩放位数 3）。照设计第 5.3 节 H2 的读法，经隧道下行而本机一侧读得慢（接收缓冲接近满）时，远端发方在途的数据可能落到 smoltcp 最近一次通告的边缘之外而让它 panic；隧道会因此结束、下一个连接时重建（阶段 2 / M8c 任务 6）。读源码得出，没有观察过。修法二选一：接收缓冲不超过 65535（经隧道的往返 50 毫秒时下行至多约 10 Mbit/s），或照 M3 加窗口边缘的守卫 | 项目所有者；下次动到 WireGuard 时 |
| 2 | 真实设备上的数字（系统调用的开销不在内存基准里） | M4 的手工验收 |
| 3 | smoltcp 0.13（修了 #1048 与 Cubic；要把 MSRV 升到 1.91） | 项目所有者（总设计 Q1） |
| 4 | 两个基准只在 Windows 上量（`peak_resident_bytes` 在 macOS 上是 `None`） | 有人在 Linux 上手工跑时补 |
