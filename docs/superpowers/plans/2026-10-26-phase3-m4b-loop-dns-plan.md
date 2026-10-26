# 阶段 3 / M4b「防环与接管期间的 DNS」实施计划

> **执行方式**：交给 Pi，在 M4a 计划做完、项目所有者评审过之后，在一个会话里按任务的次序逐个做（技能 `superpowers:executing-plans`；没有子代理，不并行、不跳）。每个任务：先写失败的用例、看到红，再实现到绿；跑门禁；对照本任务的「判据」与「全局约束」逐条自查；再提交（**只在本地，不推送**，M4 设计第 2 节）。与计划的出入记进末尾的「执行期修正记录」，**不改计划的正文**。做完任务 10 停下，交项目所有者评审。

**目标**：增强模式一打开，rurge 自己建的每一个出站套接字都不回到虚拟网卡：出口判定（纯函数）、`rurge-net` 的唯一建法 `open_socket` 按出口绑网卡、`DirectPacket` 与 QUIC 的套接字按目的地址问出口、DNS 的纯 UDP 上游与截断之后的 TCP 经连接器、资源下载与 Profile 来源带平台钩子并在接管期间经 rurge 的解析器解析名字；接管期间 `server:system` / `syslib` / 单标签名字查物理网卡的服务器、`force-syslib` 与 `.local` 照旧交系统解析库；系统 DNS 名单与网络环境去掉 rurge 的空间与虚拟网卡；真实的 `Pinger`；外部程序与它的子进程的会话一律 DIRECT；一条源码扫描用例与一组运行时用例钉住"没有漏网的套接字"。平台后端仍答"这一版还不能"（M4c / M4d 才真的能开），所以这一份做完，使用者仍然开不了增强模式；接管期间的行为经 M4a 的文件后端与单元用例测。

**架构**：出口判定 `rurge_platform::socket::egress::decide(table, dst)` 是纯函数，表由管理器在打开、关闭、重载、网络变化时重建（M4a 已有表的槽与写它的时机，这一份给表加上直连子网），可执行文件的 `PlatformSockets` 读槽、实现 `SocketHook::egress`。`rurge-net` 里出站套接字只有 `open_socket` 一处建法，它在策略没有 `interface`（或绑不上而 `allow-other-interface`）时问 `egress(目的地址)`。`rurge-dns` 的纯 UDP 上游一律经 `ResolverDeps::direct`（带平台钩子的直连连接器，从不随出站模式走流水线）。`SystemDns::enhanced_mode()` 告诉解析器系统解析库此刻可能答 fake 地址。

**技术栈**：socket2 0.6（`all`，已有）、nix 0.31（`signal` 带上 `process`，已有）、if-addrs 0.15（已有）、tokio；不新增外部包，不加新的依赖边（`rurge-platform` 的 `icmp` 只交出 socket2 的套接字，异步由可执行文件包）。

**依据**：M4 设计 `docs/superpowers/specs/2026-10-26-phase3-m4-platform-design.md`（下面写"设计"指它）第 3.1 ～ 3.4、3.7、6.2、6.3、7、10、11 节与 M4-D7 ～ D9、D20、D21；M4a 计划（`docs/superpowers/plans/2026-10-26-phase3-m4a-skeleton-plan.md`）做出来的管理器、后端接口、出口表的槽（以它的「执行期修正记录」为准）；M3b 计划延后第 4 行（真实的 `Pinger`）、M3c 计划延后第 4 行（系统的 DNS 名单排除 rurge 的地址）。设计第 17 节的 V1、V8、V9、V11 已在写计划时对着 `ac0a98a` 核对，结果写进下面的任务与「计划期决定」；任务 0 再对着当时的代码核一遍。

## 全局约束

- 依赖边：本计划不加新的工作区依赖边，也不加外部包（`Cargo.lock` 的包数与 M4a 做完时相同）。`rurge-platform` 照旧不依赖任何工作区 crate；`rurge-net` 照旧不认识平台（出口经 `SocketHook` 问）；`rurge-dns` 照旧不依赖协议 crate。
- `unsafe_code`：本计划不加放开的函数（`LISTED` 仍是七个）。Windows 上 `ProcessTree::contains` 这一份答 `false`，`in_job` 在 M4d（P15）。
- **没有任何代码执行系统命令、建网卡、改路由或系统 DNS**；平台后端照旧打不开。读网卡与地址列表（`if_addrs`）只在平台后端里（这一份没有用例走到它）；测试经文件后端，网卡的子网写在文件里（P5）。ICMP 的用例只对 `127.0.0.1` 与 `::1` 发回显，开不了 ICMP 套接字（本机不是管理员、`ping_group_range` 不允许）时写明原因跳过（P13）。
- 测试只用回环与端口 0；CLAUDE.md「测试约定」整节适用：起守护进程的用例经 `crates/rurge/tests/cli.rs` 的 `rurge_command()` / `rurge_process()` 并给 `--data-dir`；"一个没有应答的地方"是测试自己持有的监听器或 UDP 套接字；等待都有时限；"断言什么都没发生"之外不用固定的 sleep，那种等待的注释写它等什么、余量多大；新写的计时用例提交前单独循环跑二十次以上；抓日志用 `LogCapture`；调真正的系统解析库的用例不新增（`OwnResolve` 的退路在用例里换成假的，P9）。
- 名字与地址只在 DEBUG（设计第 13 节）：出口判定、绑定、物理网卡的服务器、外部程序的 PID 都只在 DEBUG。错误文本（进请求记录与 API）不带网卡名：出口的错误说 "no network outside the enhanced mode to reach <目的地址>"，绑不上说 "cannot bind to the network outside the enhanced mode: <系统的原因>"（P3）。
- 锁不跨 `await`；出口表的槽是 std 的 `RwLock`，读在每个出站套接字上，只做一次查找。
- 先写失败的用例、看到红，再实现到绿。现有用例变红时：计划点了名的照计划改、在提交说明里说；没点名的停下来记进修正记录，不顺手改期望值。
- 门禁、本机的 rustfmt、只经整个工作区跑用例（不用 `-p`）、跑全量前 `df -h /d`、`STATUS_HEAP_CORRUPTION` 与增量编译的内部错误的处理：同 M4a 计划的「全局约束」。
- 只运行本仓库构建出来的二进制与系统自带的只读工具；不下载、不安装；不在开发机上执行读写网络的命令，也不读真实的网络环境。
- 参考实现：GPL 的只取事实，代码不抄；MIT / Apache 的可以简短引用并注明出处。
- 注释、日志与错误文本用英文；提交说明照 `git log` 的样子，标题用中文。
- 计划没有写到、要做决定的地方：能按设计与总设计的原则定的，自己定，记进修正记录并写"执行者的裁定"；去向是项目所有者的（设计第 20 节），或会改变 M4-D1 ～ D30 的，停下来列出。

## 评审要点

使用者真正开起增强模式（M4c / M4d）时，这一份决定 rurge 会不会把自己的流量送回虚拟网卡（总设计风险 E：漏一处就是一个核烧满、网络断）。最可能咬到使用者的五种情形与负责的用例：

1. **一类自建的连接没有问出口**：源码扫描（任务 9 `every_outbound_socket_is_made_in_one_place`）钉住建法只有一处；运行时用例（任务 9 `every_kind_of_dns_upstream_asks_the_hook_before_it_dials` 一族）钉住每一类 DNS 上游、资源下载、流水线的回退都问过。
2. **出口判定答错**：表 × 地址的组合（任务 1），尤其 `tun-included-routes` 把局域网送进虚拟网卡时 rurge 自己到局域网的连接仍走局域网的网卡（规则 3 先于规则 4）。
3. **接管期间 `server:system` 拿到 fake 地址**：任务 3 的去处表逐行；没有物理服务器时失败，不退回系统解析库。
4. **下载链接的名字拿到 fake 地址**：`OwnResolve`（任务 4）。
5. **外部程序的流量被规则送回它自己**：任务 8 的端到端用例（`FINAL` 指向外部程序的策略，外部程序自己的会话仍 DIRECT）。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `crates/rurge-platform/src/socket.rs`（或拆成 `socket/{mod.rs,egress.rs}`，照 M4a 的做法） | 1 | `EgressTable` 加子网、`decide`、`subnets` / `subnets_of` |
| `crates/rurge-net/src/{socket.rs,connector.rs,quic/socket.rs,quic/mod.rs}` | 2 | `SocketHook::egress`；`open_socket` 问出口；`bind_udp(family, dst)`；`DirectPacket` 按出口分套接字；QUIC 给出远端 |
| `crates/rurge-dns/src/{upstream/udp.rs,resolver.rs,system.rs,bootstrap.rs,testing.rs}` 与各处用例 | 3 | 纯 UDP 上游经连接器；`ResolverDeps::direct`；`SystemDns::enhanced_mode`；接管期间的去处；`reread_system_servers` |
| `crates/rurge-engine/src/{stack.rs,runtime.rs,shared.rs,engine.rs,profile.rs}` | 4 | DNS 的连接器与流水线回退带钩子；`resource_client_with`、`OwnResolve`；`Engine::system_dns_changed` |
| `crates/rurge-platform/src/{dns.rs,netenv/mod.rs}`、`crates/rurge/src/cli/network.rs` | 5 | 名单去掉 rurge 的空间；读网络时排除虚拟网卡 |
| `crates/rurge/src/cli/{runtime.rs,run.rs,enhanced/*}` | 6 | 有状态的 `PlatformSockets` / `PlatformSystemDns`；管理器写子网、标志、排除名单、调 `system_dns_changed` |
| `crates/rurge-platform/src/icmp.rs`（新）、`crates/rurge/src/cli/pinger.rs`（新） | 7 | ICMP 套接字与报文；`PlatformPinger` |
| `crates/rurge-proto/src/external.rs`、`crates/rurge-platform/src/process.rs`、`crates/rurge-engine/src/{shared.rs,engine.rs}`、`tests/external/` | 8 | 进程树的成员；外部程序一律 DIRECT |
| `crates/rurge/tests/sockets.rs`（新）、`crates/rurge-engine/tests/egress.rs`（新） | 9 | 源码扫描；运行时用例 |
| `docs/api/phase3.md`、`docs/testing.md`、`CLAUDE.md`、`docs/status.md`、兼容性清单、本计划末尾两张表 | 10 | 文档 |

---

## Task 0：开工前核对

**做什么**：对着当时的 `main` 核对下面每一项，出入记进「执行期修正记录」（一项一行），按记录调整后面的任务；不改计划的正文。没有出入也记一行。

1. `git status` 干净；`git log --oneline` 里 M4a 的提交都在；读 M4a 计划末尾两张表，凡是改了本计划要用的名字（`EgressTable`、`EgressSlot`、`Backend`、`Opened`、`EnhancedMode::follow`、`Starter`、`NoPinger`、文件后端的键）的，以它为准。
2. V1：`git grep -n "impl SocketHook for\|SocketHook for"`（写计划时：`NoopSocketHook`、可执行文件的 `PlatformSockets`、`rurge-net` 测试里的 `RecordingHook` / `BindingHook` / `TosHook`、`rurge-engine` 测试里的两个 `TosHook`）；`open_socket` 的三个调用者（`connect_one`、`bind_udp`、`connect_udp`，`connector.rs` 约第 317 ～ 530 行）；`bind_udp` 的调用者（`open_packet`、`quic/socket.rs` 的 `direct_socket`、一条测试）；`direct_socket` 的两个调用者（`quic/mod.rs` 约第 66、74 行，都知道远端地址）；`DirectPacket`（约第 580 行）。
3. V1（DNS）：`UdpUpstream::new` 的生产调用只有 `resolver.rs` 的 `build_upstream` 与 `build_plain_udp`（约第 493、1222 行），其余都在用例里（`bootstrap.rs`、`fanout.rs`、`upstream/{udp,doq,session}.rs`）；`UdpUpstream` 自己建套接字的两处（约第 182 行 `UdpSocket::bind`、约第 294 行 `TcpStream::connect`）；`ResolverDeps {` 的构造点（`stack.rs`、`benches/dns.rs`、`tests/resolver.rs`、`tests/interop/tests/dns.rs`、`resolver.rs` 的用例）。
4. V1（引擎）：生产代码里的 `DirectConnector::new(` 与它们带的钩子：`stack.rs` 约第 117 行（解析器的连接器，`NoopSocketHook`）与约第 189 行（`resource_client`）、`runtime.rs` 约第 69 行（流水线的回退）、`rurge-proto/src/external.rs` 约第 410 行（连回环的中继）、`rurge-proto/src/direct.rs` 的 `Direct::with_resolver`（只有用例调用）；`StackOptions {` 的全部构造点（`git grep -n "StackOptions {"`）；`ProfileSources::production` 的两个调用者（`run.rs` 约第 1190、1384 行）与 `EngineShared::new` 在 `run()` 里的位置（写计划时在 `ProfileSources::production` 之后）。
5. V8：`rurge-proto/src/external.rs` 的 `ProcessHook` / `ProcessGroup`、`Program`、`Slot`（tokio 的锁）、`start`（`hook.contain(pid)`）、`watch` 与 `Tree`；`rurge-engine/src/shared.rs` 的 `ExternalPrograms`（`list`、`ports`）；`rurge-platform/src/process.rs` 的 `ProcessTree`；`engine.rs` 的 `choose_policy`（约第 774 行）与 `SlotProcess`（`owner.rs`，`PROCESS_WAIT` 200 毫秒）；`look_up_owner` 的三个调用者（`engine.rs` 约第 1567 行、`prematch.rs` 约第 82 行、`udp.rs` 约第 408 行）——经虚拟网卡与经监听器来的会话都查进程；`tests/external/src/bin/socks-helper.rs` 的 `--record`（写自己的 PID）与 `--child`。
6. V9：`resolver.rs` 的 `HostAction::System(_)`（约第 774 行）、`.local`（约第 783 行）、单标签（约第 787 行）、`primary` 为空时的 `system_lookup`（约第 836 行）、`system_lookup` / `library_lookup`（约第 1047、1073 行）、`apply_system_servers` 与 `on_network_change`（约第 533、585 行）、`forward` 里 `HostAction::System` 用 `system_upstreams`（不变）；`DnsError` 的变体；`StaticSystemDns` 的字段（`library_asked` 是共享的）。
7. V11：`rurge-platform/src/socket.rs` 的 `bind_interface` 三个平台（Linux `SO_BINDTODEVICE`、macOS `IP_BOUND_IF` 经 `if_addrs` 的序号、Windows 绑源地址经 `if_addrs` 的友好名）；`netenv/windows.rs` 的主网卡名是 `friendly_name()`（与 `if_addrs` 同一种）；`if_addrs` 0.15 的 `Interface { name, addr, index }` 与 `Ifv4Addr` / `Ifv6Addr` 的 `prefixlen`。
8. `cli/network.rs` 的 `look`（`ReadOpts::default()`）、`start`、`spawn`；`netenv::read` 与 `ReadOpts`；`rurge_platform::dns::servers()` 三个平台的来路（Windows 遍历开着的网卡，Unix 读 `/etc/resolv.conf`）。
9. 生产代码里建套接字与问系统解析库的地方（任务 9 的名单的出发点）：`git grep -n -E "UdpSocket::bind\(|TcpStream::connect|TcpSocket::new_v[46]\(|Socket::new\(|Socket::new_raw\(|TcpListener::bind\(|build_http\(|lookup_host\(|DirectConnector::new\(" -- 'crates/*/src/*.rs' 'crates/*/src/**/*.rs'`，对每一处读它在不在测试代码里。写计划时生产代码里的是任务 9 名单上的那些；多出来的逐个读、记进记录。
10. 门禁跑一遍，记下结果行数、通过数与忽略数。

**判据**：修正记录里有任务 0 的行；门禁的数字记下。

---

## Task 1：出口判定（`rurge-platform`）

**改哪里**（设计第 7.1 节、M4-D8）

M4a 的 `EgressTable { routes, primary }` 加子网，加判定：

```rust
/// What decides where rurge's own sockets go while the enhanced mode is on
/// (M4 design 7.1). Built by the manager when the enhanced mode opens, closes,
/// follows a reload or a change of network.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EgressTable {
    pub routes: RouteSet,
    /// The interface the network environment calls primary (never the virtual adapter).
    pub primary: Option<String>,
    /// The directly connected subnets of the interfaces but the virtual adapter,
    /// longest prefix first: (subnet, interface).
    pub subnets: Vec<(IpNet, String)>,
}

/// No interface outside the enhanced mode reaches the address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoRoute;

/// The interface rurge's own socket to `dst` is bound to; `None` leaves it to
/// the system's routes.
pub fn decide(table: Option<&EgressTable>, dst: IpAddr) -> Result<Option<&str>, NoRoute>;

/// `subnets` out of addresses: (interface, address, prefix length). Loopback and
/// link-local addresses, a prefix of 0 and the interfaces in `exclude` are left
/// out; each subnet once, longest first.
pub fn subnets_of(addrs: &[(String, IpAddr, u8)], exclude: &[String]) -> Vec<(IpNet, String)>;

/// The subnets of this machine's interfaces but `exclude` (`if_addrs`, read only).
pub fn subnets(exclude: &[String]) -> io::Result<Vec<(IpNet, String)>>;
```

`decide` 的次序（设计第 7.1 节）：

1. `table` 是 `None`（增强模式没开）：`Ok(None)`。
2. 回环、未指定、组播、链路本地（`169.254.0.0/16`、`fe80::/10`）、`255.255.255.255`：`Ok(None)`。
3. 落在 `subnets` 的某一项里（取最长的）：那块网卡。
4. `routes.contains(dst)`（含 rurge 的空间，M4a 的 `RouteSet::contains`）：`primary`，没有主网卡时 `Err(NoRoute)`。
5. 其余（被排除的、`external` 的地址、IPv6 没开时的 IPv6）：`Ok(None)`。

`subnets` 只是 `if_addrs::get_if_addrs()` 之后调 `subnets_of`（`Ifv4Addr` / `Ifv6Addr` 的 `prefixlen`）；它没有用例（读本机），`subnets_of` 有。

**用例**（全是数据，任何平台都跑）

- `nothing_is_bound_while_the_enhanced_mode_is_off`。
- `loopback_unspecified_multicast_link_local_and_broadcast_are_never_bound`（`127.0.0.1`、`::1`、`0.0.0.0`、`224.0.0.251`、`ff02::fb`、`169.254.1.1`、`fe80::1`、`255.255.255.255`）。
- `a_directly_connected_address_goes_out_of_its_interface_before_the_routes_are_asked`：子网 `192.168.1.0/24 → eth0`，包含 `192.168.0.0/16`（在路由里）：`192.168.1.5 → eth0`；`192.168.2.5 → primary`。
- `the_longest_subnet_wins`：`10.0.0.0/8 → a`、`10.1.0.0/16 → b`：`10.1.2.3 → b`。
- `an_address_in_the_routes_goes_out_of_the_primary_interface`（含 `198.18.0.5` 与 `fd00:6152::5`：rurge 的空间）；`without_a_primary_interface_it_is_no_route`。
- `an_excluded_address_is_left_to_the_system`。
- `subnets_of_leaves_out_loopback_link_local_the_excluded_and_a_zero_prefix`；`subnets_of_lists_each_subnet_once_longest_first`。

**判据**：`decide` 与 `subnets_of` 里没有 `cfg`；门禁绿。

---

## Task 2：`rurge-net` 的出口

**改哪里**（设计第 3.2 节；P2 ～ P4）

1. `SocketHook` 加一个有缺省实现的方法：
   ```rust
   /// The interface rurge's own socket to `dst` has to be bound to, so that it
   /// does not go into the virtual adapter of the enhanced mode (M4 design 7.1).
   /// `Ok(None)`, the default: no binding. `Err`: this connection is not to be
   /// made, and its error is what the caller sees.
   fn egress(&self, dst: IpAddr) -> io::Result<Option<String>> {
       let _ = dst;
       Ok(None)
   }
   ```
   `NoopSocketHook` 不改（用缺省的）；测试里的假实现不改（用缺省的），要记录的那几个加上记录。
2. `open_socket` 改成
   ```rust
   fn open_socket(
       family: Family,
       dst: Option<IpAddr>,
       kind: socket2::Type,
       opts: &SocketOpts,
       hook: &dyn SocketHook,
       fallback_logged: &AtomicBool,
   ) -> io::Result<socket2::Socket>
   ```
   次序：建套接字、非阻塞、IPv6 的 UDP 只收 IPv6（今天的）、TOS（今天的）；策略的 `interface`（今天的）；**策略没有 `interface`，或绑不上而 `allow-other-interface` 退回不绑时**，`dst` 是 `Some` 就问 `hook.egress(dst)`：`Err(e)` → 原样返回；`Some(网卡)` → `hook.bind_interface(&socket, &网卡, family)`，失败 → `io::Error::new(e.kind(), format!("cannot bind to the network outside the enhanced mode: {e}"))`；`None` → 不绑。DEBUG 一行带目的地址与网卡（INFO 以上不带）。
3. 三个调用者：`connect_one` 与 `connect_udp` 给 `Some(addr.ip())`；`bind_udp(family, dst: Option<IpAddr>)` 把 `dst` 递下去（绑未指定地址照旧在 `open_socket` 之后、`is_bound` 照旧）。
4. `quic/socket.rs`：`direct_socket(connector, remote: SocketAddr)`，`bind_udp(Family::of(&remote.ip()), Some(remote.ip()))`；`quic/mod.rs` 的两个调用者给出 `remote` 与 `SocketAddr::new(server, 0)`（端口跳跃只换端口，出口按地址定）。
5. `DirectPacket`（P4）：套接字按（地址族，出口）各一个。
   - `open_packet` 照今天打开每个地址族的不绑的那个（出口 `None`），保留今天"开不了的地址族略过、都开不了才失败"的语义。
   - `send_to`：地址定了之后问 `hook.egress(ip)`：`Err` → 这个数据报失败；`Some(网卡)` → 找（地址族，网卡）的套接字，没有就用 `bind_udp(family, Some(ip))` 建一个加进去；`None` → 那个地址族的不绑的套接字。
   - `recv_from`：轮询全部套接字；加了套接字就唤醒正在等的 `recv_from`（存下等着的 waker，或一个 `Notify`，照实），否则新套接字上的回包要等别的套接字有包才被看到。
   - 套接字的列表在 std 的锁里，锁不跨 `await`；`send_to` 在锁外 `await`（克隆 `Arc` 再发）。
   - 列表不缩：一个 `DirectPacket` 活多久，它建过的套接字就活多久（UDP 流的寿命由引擎的空闲超时界住）。

**用例**（`connector.rs`、`quic/mod.rs` 的 `tests`；记录的钩子"绑定"时只记下、不真的绑）

- `open_socket_asks_egress_for_the_destination_and_binds_there`：钩子答 `Some("test0")`：记录是 `egress 127.0.0.1`、`bind test0 V4`。
- `no_egress_no_binding`：`Ok(None)`：只有 `egress`，没有 `bind`。
- `a_policy_interface_is_used_and_egress_is_not_asked`。
- `allow_other_interface_falls_back_to_egress`：策略的网卡绑不上（照 `BindingHook` 的写法）：再问 `egress`、绑它答的网卡。
- `an_egress_error_fails_the_dial_and_nothing_reaches_the_server`：钩子答 `Err`：`connect` 失败、错误文本是钩子的；测试自己的监听器在一段短时间里没有收到连接（"断言什么都没发生"的等待，注释写它等的是什么）。`connect_udp` 同样（UDP 套接字什么也没收到）。
- `a_binding_that_fails_fails_the_dial_with_no_interface_name_in_the_error`。
- `direct_packet_opens_a_socket_for_each_egress_and_hears_on_all`：钩子对同一个回环地址先后答 `Some("a")`、`Some("b")`（像增强模式开了、网络变了）：两个数据报从两个不同的本地端口出去（回显服务端看到两个来源），两个回包都由 `recv_from` 收到；一个 `recv_from` 在第二个套接字建出来之前就在等，也收得到第二个套接字上的回包。
- `direct_packet_fails_a_datagram_whose_egress_is_an_error`。
- `bind_udp_passes_the_destination_to_egress`（改现有的 `bind_udp` 用例：`bind_udp(Family::V4, None)` 不问出口；`Some(ip)` 问）。
- `quic_direct_socket_asks_egress_for_the_server`（照 `quic/mod.rs` 现有的 `BindingHook` / `TosHook` 用例）。

**判据**：`git grep -n "fn open_socket" crates/rurge-net` 只有一处；门禁绿；改动之外的 `rurge-net` 用例照旧通过（钩子缺省 `Ok(None)`，一切不变）。

---

## Task 3：`rurge-dns`：纯 UDP 上游经连接器、接管期间的去处

**改哪里**（设计第 3.3、6.2 节、M4-D7、D9；P5 ～ P8）

1. `upstream/udp.rs`：`UdpUpstream` 只经连接器：
   ```rust
   /// The server at `addr`, asked directly through `connector`: a direct
   /// connector with the platform's socket hook (M4 design 7.2), never the
   /// pipeline's. Its name is `udp://<addr>`.
   pub fn new(addr: SocketAddr, connector: Arc<dyn Connector>) -> UdpUpstream
   ```
   `via` 不变（名字 `udp://<addr> via <key>`）。`Socket::Own` 与它的 `UdpSocket::bind`、截断之后的 `TcpStream::connect` 去掉：问题经 `connector.connect_udp`、截断之后经 `connector.connect`（今天 `via` 的那条路）。模块说明改。
2. `ResolverDeps` 加
   ```rust
   /// What plain UDP servers are asked through — the configured ones, the
   /// system's, the bootstrap's, `[Host]`'s `server:` addresses: a direct
   /// connector with the platform's socket hook. Never the pipeline's: plain
   /// UDP does not follow `encrypted-dns-follow-outbound-mode`.
   pub direct: Arc<dyn Connector>,
   ```
   `Resolver` 存下它；`build_upstream` 的 `UpstreamSpec::Udp`、`build_plain_udp`、`bootstrap_set`、`host_upstreams_for`（经 `build_upstream`）都用它。
3. `testing.rs`（或用例共用的地方）加 `pub fn direct() -> Arc<dyn Connector>`（`DirectConnector::new(Arc::new(SystemResolve))`）；全部用例里的 `UdpUpstream::new(addr)` 改成 `UdpUpstream::new(addr, direct())`，`ResolverDeps {` 加 `direct`（`benches/dns.rs`、`tests/resolver.rs`、`tests/interop/tests/dns.rs` 一起改）。
4. `system.rs`：`SystemDns` 加
   ```rust
   /// The enhanced mode is on: the system resolver library may answer with
   /// fake addresses (the system's DNS points at rurge, or `hijack-dns` takes
   /// its queries). The default is `false`.
   fn enhanced_mode(&self) -> bool {
       false
   }
   ```
   `StaticSystemDns` 加 `pub enhanced: Arc<AtomicBool>`（克隆之间共享，照 `library_asked`；缺省 `false`），实现它。
5. `resolver.rs` 的去处（设计第 6.2 节的表；`self.system.enhanced_mode()` 每次查找时读）：
   - `HostAction::System(SystemMode::System | SystemMode::Syslib)`：接管期间查 `system_upstreams`（物理网卡的服务器）；是空的就失败，错误文本 `no DNS server outside the enhanced mode`（用现有 `DnsError` 里合适的变体，照实），**不退回系统解析库**。`SystemMode::ForceSyslib`：照旧 `library_lookup`。不接管时三种照旧 `library_lookup`（`library_lookup` 的说明改成写清这两种情形）。
   - `.local`：照旧 `library_lookup`。
   - 单标签名字：接管期间只查 `system_upstreams`，没有就失败；不接管时照旧 `system_lookup`。
   - `primary` 是空的：接管期间只查 `system_upstreams`，没有就失败；不接管时照旧。
   - `forward`（应答器转发）不变。
6. `reread_system_servers`（P8）：
   ```rust
   /// Reads the system's DNS servers again and rebuilds what is made of them
   /// (the system set, the bootstrap, a primary set that is the system's),
   /// keeping the cache: the enhanced mode took the system's DNS or gave it back.
   pub fn reread_system_servers(&self)
   ```
   `on_network_change` 改成"清缓存、读 `has_ipv6`、`reread_system_servers`、清 `host_upstreams`"，行为不变。

**用例**（`resolver.rs` 的 `tests`，照现有的 `StaticSystemDns` 与回环上的 `MockDns` 写法）

- `in_the_enhanced_mode_system_and_syslib_ask_the_physical_servers`：`[Host] a.test = server:system`、`b.test = server:syslib`，`servers` 是回环上的假服务器，`enhanced` 为真：两个名字由服务器答，`library_asked` 是空的。
- `in_the_enhanced_mode_system_without_servers_fails_and_never_asks_the_library`。
- `force_syslib_asks_the_library_in_the_enhanced_mode_too`。
- `local_names_ask_the_library_in_the_enhanced_mode_too`。
- `a_single_label_asks_only_the_servers_in_the_enhanced_mode`；`…_and_fails_without_them`。
- `a_resolver_without_servers_does_not_turn_to_the_library_in_the_enhanced_mode`。
- `outside_the_enhanced_mode_system_syslib_and_single_labels_are_as_before`（现有用例覆盖的话，点名它们，不重写）。
- `reread_system_servers_takes_the_new_servers_and_keeps_the_cache`：先查一个名字进缓存；换 `servers`、`reread_system_servers`；缓存里的仍从缓存答，`server:system` 的名字问新的服务器。
- `upstream/udp.rs`：`a_udp_upstream_asks_through_its_connector`（记录的连接器看到 `connect_udp` 与截断之后的 `connect`）；现有的 UDP 上游用例经 `direct()` 照旧通过。

**判据**：`git grep -n "UdpSocket::bind\|TcpStream::connect" crates/rurge-dns/src` 只剩测试代码里的；门禁绿。

---

## Task 4：`rurge-engine`：连接器带钩子、rurge 自己的名字解析

**改哪里**（设计第 3.4、7.2、7.6 节、M4-D9；P9 ～ P11）

1. `stack.rs`：解析器的直连连接器带钩子：
   `let direct: Arc<dyn Connector> = Arc::new(DirectConnector::with_opts(Arc::new(SystemResolve), SocketOpts::default(), opts.socket_hook.clone()));`（名字从不经它解析：加密上游的名字经引导、纯 UDP 是地址）——`DnsConnectors::all(direct.clone())`（没有流水线时）与 `ResolverDeps::direct` 都用它。
2. `runtime.rs` 的流水线回退连接器（约第 69 行）同样带 `opts.stack.socket_hook`。
3. 资源下载：
   ```rust
   /// The HTTP client of downloads (external resources, the profile's
   /// sources): direct connections with `hook`, names looked up by `resolve`.
   pub fn resource_client_with(hook: Arc<dyn SocketHook>, resolve: Arc<dyn Resolve>) -> anyhow::Result<Arc<HttpClient>>
   /// `resource_client_with` with no hook and the system's library: for the
   /// commands that run no engine, and tests.
   pub fn resource_client() -> anyhow::Result<Arc<HttpClient>>
   ```
   `StackOptions` 加 `pub resource_resolve: Option<Arc<dyn Resolve>>`（`None`：系统解析库）；`build_stack_with` 用 `resource_client_with(opts.socket_hook.clone(), …)`。全部 `StackOptions {` 的构造点补上 `resource_resolve: None`。
4. `shared.rs`：
   ```rust
   /// How rurge looks up the names it dials itself — downloads, the profile's
   /// sources: through `library` (the system's), except while the enhanced
   /// mode is on, when the library may answer with fake addresses and the
   /// current generation's resolver is asked instead (M4 design 7.6).
   pub struct OwnResolve { system: Arc<dyn SystemDns>, cell: Arc<ResolverCell>, library: Arc<dyn Resolve> }
   impl OwnResolve { pub fn new(system: Arc<dyn SystemDns>, cell: Arc<ResolverCell>, library: Arc<dyn Resolve>) -> OwnResolve }
   impl Resolve for OwnResolve { … }   // enhanced_mode() 且 cell.active() → cell；否则 library
   impl ResolverCell { pub fn active(&self) -> bool }
   ```
   `Runtime::build` 在 `build_stack` 之前设 `opts.stack.resource_resolve = Some(Arc::new(OwnResolve::new(opts.stack.system.clone(), opts.shared.resolver.clone(), Arc::new(SystemResolve))))`（`resource_resolve` 已经有值时不覆盖：用例可以自己给）。
5. Profile 来源：`ProfileSources::production(profile, load, data_dir, offline, client: Arc<HttpClient>)`。`run.rs` 里守护进程的那个调用（约第 1384 行）：把建 `shared` 的几行挪到它之前，传 `resource_client_with(<平台钩子>, OwnResolve(<平台的 SystemDns>, shared.resolver.clone(), SystemResolve))`（平台钩子与 `SystemDns` 在任务 6 变成共享的，这一任务先用今天的值，任务 6 换）；另一个调用（约第 1190 行，不起引擎的路）传 `resource_client()?`。
6. `engine.rs`：
   ```rust
   /// The enhanced mode took the system's DNS or gave it back: the resolver in
   /// use reads the system's servers again (`Resolver::reread_system_servers`),
   /// keeping its cache. Not a change of network: `network_changed` is that.
   pub fn system_dns_changed(&self)
   ```

**用例**

- `shared.rs`：`own_resolve_asks_the_library_outside_the_enhanced_mode`、`own_resolve_asks_the_generation_while_the_enhanced_mode_is_on`、`own_resolve_asks_the_library_before_any_generation_is_published`（`library` 与 `cell` 里都是假的解析，记下谁被问了；不调真正的系统解析库）。
- `stack.rs` 或 `tests/`：`the_stack_s_connectors_carry_the_socket_hook`（记录的钩子：一个 `dns-server = 127.0.0.1:<测试自己的 UDP 套接字>` 的查找让钩子被问到 `127.0.0.1`；全面的在任务 9）。
- `engine.rs` 或 `tests/`：`system_dns_changed_rereads_the_servers_and_keeps_the_cache`（照 `network_changed` 现有用例的写法，用 `StaticSystemDns` 换服务器）。

**判据**：`git grep -n "DirectConnector::new(" crates/rurge-engine/src` 只剩 `resource_client()` 那一处与测试代码；门禁绿。

---

## Task 5：系统 DNS 名单与网络环境去掉 rurge 的空间

**改哪里**（设计第 6.3 节、M3c 延后第 4 行；P11、P12）

1. `rurge-platform/src/dns.rs`：`servers()` 去掉落在 rurge 空间（`route::SPACE_V4` / `SPACE_V6`）里的服务器，三个平台一样；去掉的写法是一个纯函数（`without_rurge_space` 一类），有用例。
2. `netenv`：快照的 `dns` 同样去掉 rurge 空间里的（在 `read` / `read_quick` 共同的路上做，三个平台与文件后端一样）。`ReadOpts::exclude`（已有）用来排除虚拟网卡，说明里"until there is one the list is empty"改成"the virtual adapter while the enhanced mode is on"。**不加**设计第 3.1 节说的"要排除的 DNS 前缀"一项：rurge 的空间是常数（P12）。
3. `cli/network.rs`：读网络时用一份共享的排除名单：
   ```rust
   /// The interfaces the network task leaves out of what it reads: the virtual
   /// adapter while the enhanced mode is on (the manager sets it).
   pub type Exclude = Arc<std::sync::RwLock<Vec<String>>>;
   ```
   `look(backend, exclude)` 每次读之前取一份；`start` 与 `spawn` 多这个参数（照实）。

**用例**

- `dns.rs`：`servers_in_rurges_space_are_left_out`（`198.18.0.2`、`198.19.255.255`、`fd00:6152::2` 去掉；`198.17.255.255`、`198.20.0.1`、`8.8.8.8`、`fd00:6153::1` 留下）。
- `netenv`：`a_snapshot_leaves_out_dns_servers_in_rurges_space`（经文件后端：文件里写着 `198.18.0.2` 与一个别的服务器，读出来只剩后者）。
- `cli/network.rs`：`the_network_is_read_without_the_excluded_interfaces`（若 `look` 的参数能在单元用例里观察到；观察不到就只在任务 6 的管理器用例里断言名单被设、被清，记进修正记录）。

**判据**：门禁绿。

---

## Task 6：可执行文件：有状态的钩子与系统 DNS、管理器接上

**改哪里**（设计第 3.7、6.3、7.1 节；P5、P6、P8）

1. `cli/runtime.rs`：
   ```rust
   /// `rurge-platform::socket` behind `SocketHook`, and where rurge's own sockets
   /// go while the enhanced mode is on (the manager writes the table).
   pub struct PlatformSockets { egress: EgressSlot }
   impl PlatformSockets { pub fn new(egress: EgressSlot) -> PlatformSockets }
   // SocketHook::egress：decide(slot.read().as_deref(), dst)，`NoRoute` → io::Error::new(ErrorKind::NetworkUnreachable, format!("no network outside the enhanced mode to reach {dst}"))

   /// `rurge-platform::dns` behind `SystemDns`, told by the manager whether the
   /// enhanced mode is on.
   pub struct PlatformSystemDns { enhanced: Arc<AtomicBool>, physical: Arc<RwLock<Option<Vec<SocketAddr>>>> }
   ```
   `servers()`：增强模式开着且 `physical` 有值时给它，否则 `rurge_platform::dns::servers()`（已去掉 rurge 的空间）。`physical` 这一份没有人写（M4c 的 macOS 写备份里的原值与 DHCP 给的、Linux 写 resolved 的上游名单）；Windows 不需要它（设计第 6.3 节）。`enhanced_mode()` 读 `enhanced`。
   可执行文件的 `Runtime` 建一次这两样（`Arc`），`stack_options` 每次克隆同一份——每一代的解析器与连接器看到同一个出口表与同一个标志。现有用例 `platform_sockets_delegates_to_rurge_platform` 照新的构造改（点名）。
2. `run.rs`：任务 4 的 `resource_client_with` 用这两样；M4a 的出口表槽与管理器用同一个 `EgressSlot`。
3. 后端接口加（M4a 的 `Backend`）：
   ```rust
   /// The directly connected subnets of the interfaces but `exclude` (M4 design 7.1, rule 3).
   fn subnets(&self, exclude: &[String]) -> Result<Vec<(IpNet, String)>, String>;
   ```
   平台后端：`rurge_platform::socket::egress::subnets`；文件后端：文件里的 `"subnets": [["192.168.1.0/24", "eth0"], …]`（缺省空的）；`off`：`Ok(vec![])`。
4. 管理器（M4a 的 `EnhancedMode`）：
   - 写出口表时（打开的第 4 步、`follow`）带上 `subnets(&[虚拟网卡的名字])`；读不到子网：WARN 一行，表照写、子网是空的（规则 3 不起作用，规则 4、5 照旧）。
   - 增强模式的标志（`PlatformSystemDns::enhanced`）：出口表写好的同时置真（第 4 步）；关闭时清出口表的同时置假。
   - 网络任务的排除名单（任务 5 的 `Exclude`）：打开的第 1 步之后放进虚拟网卡的名字，关闭的最后清掉。
   - `system_dns_changed`：管理器多一个回调（`Arc<dyn Fn() + Send + Sync>`，生产里调 `engine.system_dns_changed()`），在标志变了之后、接管系统 DNS 之后、交还之后各调一次；单元用例里记次数。

**用例**

- `cli/runtime.rs`：`platform_sockets_egress_follows_the_table`（槽是空的 → `Ok(None)`；有表 → 主网卡；没有主网卡 → 错误的类别是 `NetworkUnreachable`、文字带目的地址、不带网卡名）。`platform_system_dns_is_in_the_enhanced_mode_while_the_flag_is_up`。
- 管理器单元用例（`cli/enhanced/` 的 `tests`，M4a 的假后端加上 `subnets`）：`opening_writes_the_egress_table_with_the_subnets_and_closing_clears_it`、`the_enhanced_flag_is_up_from_the_egress_table_until_it_is_cleared`、`the_virtual_adapter_is_left_out_of_the_network_reads_while_open`、`system_dns_changed_is_called_after_the_flag_the_takeover_and_the_giving_back`、`subnets_that_cannot_be_read_leave_rule_three_out_and_open_all_the_same`。
- 端到端（`run/enhanced_mode.rs`，文件后端）：`the_daemon_s_own_dns_and_downloads_keep_working_while_the_enhanced_mode_is_on`：开着增强模式（文件里 `subnets` 是空的、网络文件里有一个主网卡），`dns-server` 与一个 `RULE-SET` 的下载都在回环上：照常能查、能下（出口判定的第 2 条：回环不绑）。

**判据**：`git grep -n "Arc::new(PlatformSockets)\|Arc::new(PlatformSystemDns)" crates/rurge/src` 只有建一次的那一处；门禁绿。

---

## Task 7：真实的 `Pinger`

**改哪里**（设计第 10 节、M4-D20；M3b 延后第 4 行；P13、P14）

1. `crates/rurge-platform/src/icmp.rs`（新，`pub mod icmp`；不带 unsafe）：
   ```rust
   #[derive(Clone, Copy, Debug, PartialEq, Eq)]
   pub enum Kind { Datagram, Raw }

   /// An ICMP (ICMPv6) socket for echo requests, non-blocking: Linux tries a
   /// datagram socket (no privilege, within `net.ipv4.ping_group_range`) and
   /// then a raw one; macOS a datagram socket; Windows a raw one (Administrator).
   pub fn open(family: Family) -> io::Result<(socket2::Socket, Kind)>;

   /// An echo request: type 8 (ICMPv6: 128), code 0, `ident`, `seq`, `payload`.
   /// The ICMPv4 checksum is filled in; the ICMPv6 one is the kernel's.
   pub fn request(family: Family, ident: u16, seq: u16, payload: &[u8]) -> Vec<u8>;

   pub struct Reply { pub ident: u16, pub seq: u16, pub payload: Vec<u8> }

   /// The echo reply in what a socket received — after an IPv4 header (raw
   /// sockets, and macOS's datagram ones, give it), or without one. Anything
   /// that is not an echo reply is `None`.
   pub fn reply(family: Family, bytes: &[u8]) -> Option<Reply>;
   ```
   `Family` 用 `socket::Family`。IPv4 头的有无按第一个字节的高四位是不是 4 判（回显应答的类型是 0，不会是 `0x4_`）；IPv6 的套接字从不带 IPv6 头。
2. `crates/rurge/src/cli/pinger.rs`（新）：
   ```rust
   /// `rurge_tun::Pinger` on the platform's ICMP sockets, each bound where rurge's
   /// own sockets go (the egress of `PlatformSockets`).
   pub struct PlatformPinger { … }
   impl PlatformPinger { pub fn new(sockets: Arc<PlatformSockets>) -> PlatformPinger }
   ```
   - 每个（地址族，出口）一个套接字，用到时才建：`icmp::open`，出口是 `Some(网卡)` 就 `rurge_platform::socket::bind_interface`，转成 tokio 的 `UdpSocket`（`from_std`），起一个收包任务。
   - 标识符是这个 Pinger 自己的（建时随机一个 `u16`）；序号逐个加（回绕，跳过还在等的）。
   - 收包任务：`reply(...)` 解出来之后，`Raw` 按标识符与序号对，`Datagram` 只按序号对（Linux 的内核改写标识符、只交上这个套接字的应答）；对上的交给等着的那一个，对不上的丢掉。
   - `echo` 的 future 被丢掉（`rurge-tun` 的 5 秒超时）时从等待表里去掉自己（守卫的 `Drop`）。
   - 出口答 `Err` → 这次回显失败（错误文本照 `PlatformSockets`）。
   - 分派的部分（等待表、对号）写成不碰套接字的一小块，单独测。
3. M4a 的 `Starter` 起 TUN 时用 `PlatformPinger::new(<共享的 PlatformSockets>)`，`NoPinger` 去掉（还有用例用它的话留在测试代码里）。

**用例**

- `icmp.rs`：`request_and_reply_round_trip_for_both_families`、`the_icmpv4_checksum_is_right`（一个手算的字面量）、`a_reply_after_an_ipv4_header_is_read`、`anything_but_an_echo_reply_is_none`（回显请求、目的不可达、太短的）。
- `pinger.rs`：`replies_go_to_the_waiter_of_their_sequence_number`、`a_raw_socket_matches_the_identifier_too`、`a_dropped_echo_leaves_the_table`、`sequence_numbers_skip_the_ones_still_waited_for`（不碰套接字）。
- `pinger.rs`：`pings_the_loopback`（`127.0.0.1` 与 `::1`，出口表是空的；`icmp::open` 失败时打印原因并返回——本机 Windows 不是管理员时就是这样，CI 的 Linux 与 macOS 上能开数据报套接字；**这条用例在本机跳过，照实写进报告**）。

**判据**：门禁绿；`pings_the_loopback` 在本机跳过的原因写进修正记录。

---

## Task 8：外部程序一律 DIRECT

**改哪里**（设计第 11 节、M4-D21；P15、P16）

1. `rurge-proto/src/external.rs`：
   - `ProcessGroup` 加
     ```rust
     /// Whether process `pid` is the program or one it started that is still
     /// in its group. `false` when the platform cannot tell.
     fn contains(&self, pid: u32) -> bool {
         let _ = pid;
         false
     }
     ```
   - 程序在跑时它的进程树要能被问到：`Program` 多一个 std 的锁住的 `Option<Arc<std::sync::Mutex<Tree>>>`（`start` 放进、`watch` 结束时取走；`watch` 拿同一个 `Arc`）。`ExternalOutbound` 加 `pub fn runs(&self, pid: u32) -> bool`（只读那个 std 的锁，不等 tokio 的 `slot`）与 `pub fn running(&self) -> bool`。名字与拆法照实，记进修正记录。
2. `rurge-platform/src/process.rs`：`ProcessTree::contains(&self, pid: u32) -> bool`——Unix `nix::unistd::getpgid(Some(Pid::from_raw(pid)))` 等于组长（读不到的进程答 `false`）；Windows 这一份答 `false`，说明写"M4d：`IsProcessInJob`"。可执行文件的 `PlatformTree` 实现 `contains`。
3. `rurge-engine/src/shared.rs`：`ExternalPrograms` 加 `pub fn any_running(&self) -> bool` 与 `pub fn runs(&self, pid: u32) -> bool`（遍历还活着的外部策略）。
4. `rurge-engine/src/engine.rs` 的 `choose_policy`：**在看出站模式之前**：`self.shared.externals.any_running()` 时等这个会话的进程（`SlotProcess`，上限照旧是 `PROCESS_WAIT`；建一个、后面的规则评估复用同一个，不等两次），PID 在某个外部程序的进程树里 → 注一句 `Requested by an external program: always DIRECT`（`handle.add_note`，DEBUG 一行带 PID）→ `Chosen::Policy(PolicyRef::Builtin(Builtin::Direct))`。没有外部程序在跑时不等。等不到（超时）照规则走（登记，P16）。

**用例**

- `rurge-platform`：`#[cfg(unix)] a_process_tree_contains_its_leader_and_what_it_started_and_nothing_else`（起一个 `sleep` 一类的系统自带程序作组长、再让它起一个子进程；本进程不在其中）——**只在 CI 的 Linux / macOS 上运行**；`#[cfg(windows)] contains_is_false_until_m4d`。
- `rurge-proto`：`a_running_program_answers_runs_for_its_own_pid`（经 `socks-helper` 或假的 `ProcessHook`：假钩子的组对给定的 PID 答真，照现有的假钩子写）；`a_stopped_program_runs_nothing`。
- `tests/external/tests/engine.rs`（照那里的端到端写法；进程查找用假的 `OwnerLookup`，照 `crates/rurge-engine/tests/process.rs` 的 `Fake`）：
  - `the_sessions_of_an_external_program_go_direct_whatever_the_rules`：规则 `FINAL,<外部策略>`；先经外部策略发一个会话让程序起来、从 `--record` 读出它的 PID；假的 `OwnerLookup` 答这个 PID 的会话 → 请求记录是 DIRECT、`notes` 有那句；答别的 PID（本测试进程自己的）的会话 → 外部策略。
  - `in_the_global_mode_too`：出站模式 `proxy`、全局策略是外部策略：程序自己的会话仍 DIRECT。
  - `#[cfg(unix)] what_the_program_started_goes_direct_too`：`--child` 起的子进程的 PID（它怎样把 PID 写出来，照 `--hold` 的写法加一个，只在测试构建里）。
  - `no_wait_without_a_running_program`：没有外部程序在跑时，一个进程槽永不落定的会话不因此多等（照 `process.rs` 里"慢的查找"的用例写；等待的上限写明）。

**判据**：门禁绿；新写的计时用例单独循环跑二十次。

---

## Task 9：漏网的检查

**改哪里**（设计第 7.2 节、M4-D9；P17、P18）

1. **源码扫描**：`crates/rurge/tests/sockets.rs`（新，照 `crates/rurge-platform/tests/unsafe_functions.rs` 的写法）：
   - 读工作区 `crates/*/src/` 下的每个 `.rs`（`tests/interop`、`tests/external` 与以后的 `tests/privileged` 是只供测试的成员，不读）。
   - 跳过测试代码：文件里从 `#[cfg(test)]` 紧跟 `mod <名字>` 的那一行起到文件末尾（仓库的约定：测试模块在文件末尾；不在末尾的，扫描用例报错、说出文件与行，由人看）；路径里有名为 `tests` 或 `testing` 的目录、或文件名是 `testing.rs` 的整个跳过（它们由 `#[cfg(test)]` 或 `testing` 特性门住，任务 0 核对每一个）。
   - 每行只看 `//` 之前的部分（文档注释整行跳过）。
   - 找的写法：`UdpSocket::bind(`、`TcpStream::connect`、`TcpSocket::new_v4(`、`TcpSocket::new_v6(`、`Socket::new(`、`Socket::new_raw(`、`TcpListener::bind(`、`build_http(`、`lookup_host(`、`DirectConnector::new(`。
   - 名单 `LISTED: [(&str 文件, &str 写法, &str 理由); N]`，按（文件，写法）比对：找到的集合与名单相等；多一处、少一处都变红，信息列出差别。写计划时名单是（任务 0 照实增减，每项一句理由）：

     | 文件 | 写法 | 理由 |
     | ---- | ---- | ---- |
     | `crates/rurge-net/src/connector.rs` | `Socket::new(` | `open_socket`：出站套接字唯一的建法，问出口 |
     | `crates/rurge-net/src/connector.rs` | `lookup_host(` | `SystemResolve`：系统解析库；增强模式开着时 rurge 自己的名字经 `OwnResolve` |
     | `crates/rurge/src/cli/runtime.rs` | `lookup_host(` | `PlatformSystemDns::library_lookup`：`force-syslib` 与 `.local`（设计第 6.2 节） |
     | `crates/rurge-api/src/lib.rs` | `TcpListener::bind(` | HTTP API 的监听器，不出站 |
     | `crates/rurge-inbound/src/listener.rs` | `TcpListener::bind(` | 监听器，不出站 |
     | `crates/rurge-inbound/src/socks5.rs` | `UdpSocket::bind(` | SOCKS5 UDP 关联在监听地址上的中继，只与本地的客户端来往 |
     | `crates/rurge-proto/src/external.rs` | `TcpStream::connect` | 连外部程序在回环上的端口 |
     | `crates/rurge-proto/src/external.rs` | `DirectConnector::new(` | 外部程序的 UDP 中继在回环上 |
     | `crates/rurge-proto/src/direct.rs` | `DirectConnector::new(` | `Direct::with_resolver`：只有用例调用 |
     | `crates/rurge-engine/src/stack.rs` | `DirectConnector::new(` | `resource_client()`：不起引擎的命令（`check` 一类）与用例；守护进程用 `resource_client_with` |
     | `crates/rurge-platform/src/netenv/watch.rs` | `Socket::new(` | macOS 的路由套接字（`AF_ROUTE`），不出站 |
     | `crates/rurge-platform/src/icmp.rs` | `Socket::new(` | ICMP 套接字，`PlatformPinger` 按出口绑 |
     | `crates/rurge-tun/src/tcp/shard.rs` | `Socket::new(` | smoltcp 的套接字，不是系统的 |
     | `crates/rurge-proto-wireguard/src/stack.rs` | `Socket::new(` | smoltcp 的套接字，不是系统的 |
     | `crates/rurge/src/cli/api_client.rs` | `build_http(` | `rurge status` / `reload` / `stop` 的进程，不是守护进程 |

   - 说明写明：加一处建套接字的地方要改这张名单并写理由；M4c / M4d 加设备的套接字时同样（macOS 的 utun 控制套接字）。
2. **运行时用例**：`crates/rurge-engine/tests/egress.rs`（新；一个记录的 `SocketHook`：记下 `egress` 被问到的地址，答 `Ok(None)`，或按开关答 `Err`）：
   - `every_kind_of_dns_upstream_asks_the_hook_before_it_dials`：每一类上游一份 Profile（`dns-server = 127.0.0.1:<端口>`；截断之后的 TCP（回环上答截断的假服务器，照 `upstream/udp.rs` 的截断用例）；`encrypted-dns-server = tcp://`、`tls://`、`https://…/dns-query`、`quic://`、`h3://`，地址都是 `127.0.0.1` 上测试自己持有的、不应答的监听器或 UDP 套接字），`build_stack`（`socket_hook` 是记录的钩子）之后查一个名字：查找成败不论，钩子被问过 `127.0.0.1`。
   - `resource_downloads_ask_the_hook`：一个 `RULE-SET` 的地址在回环的 `TestServer` 上。
   - `the_pipeline_fallback_asks_the_hook`（`encrypted-dns-follow-outbound-mode = true`，照 `dns_pipeline.rs` 的用例）。
   - `an_egress_error_stops_each_kind_before_anything_is_sent`：钩子答 `Err`：每一类的查找都失败，测试持有的监听器没有收到连接、UDP 套接字没有收到数据报（"断言什么都没发生"的等待，注释写明）。

**判据**：扫描用例绿，并且做一次"改一行看它变红"的验证：在 `rurge-dns` 的 `upstream/udp.rs` 生产代码里临时加一行 `const _PROBE: &str = "UdpSocket::bind(";`（一个从不被用到的字符串常量：只有源码扫描看得到它，它不让任何会话、查找、下载与文件去别处）→ 扫描用例红，信息指出那个文件与写法；撤回 → 绿；门禁绿。

---

## Task 10：文档

- `docs/api/phase3.md` 的 M4 一节（接 M4a 写的）：出口判定的五条与它的日志（只在 DEBUG）；**非直连的 VPN 路由的限制**与办法（写进 `tun-excluded-routes`）；接管期间 `server:system` / `syslib` / 单标签名字 / `force-syslib` / `.local` 的去处（设计第 6.2 节的表）；系统 DNS 名单去掉 rurge 的空间；资源下载与 Profile 来源接管期间经 rurge 的解析器；ICMP 回显（套接字的种类、Windows 要管理员）；外部程序一律 DIRECT 与它的三个限制（预匹配先于它、`setsid` 的子进程不算、进程查找超时照规则走；Windows 上 M4d 之前只认程序本身——**这一份 Windows 上连程序本身也认不出**，照实写）。
- `docs/testing.md`：`crates/rurge/tests/sockets.rs` 的名单怎样改；`pings_the_loopback` 在哪里跳过；`tests/egress.rs`。
- `CLAUDE.md`「必须遵守的规矩」加一条（新的规矩）："rurge 自建的出站套接字一律经 `rurge-net` 的 `open_socket`（它问出口）；新加建套接字或问系统解析库的地方，进 `crates/rurge/tests/sockets.rs` 的名单并写理由"。
- `docs/status.md`：阶段 3 下新的一节"M4b（防环与接管期间的 DNS）"——做了什么；没做的与不要当成已经有的（平台后端仍打不开；`physical` 没有人写，M4c；Windows 的 `contains` 恒假，M4d；`pings_the_loopback` 在本机跳过）；以后的会话必须知道的事（出口判定的次序；`SocketHook::egress` 的缺省；`ResolverDeps::direct` 不随出站模式；`OwnResolve`；`system_dns_changed` 与 `network_changed` 的分别；扫描用例的名单）。
- 兼容性清单：`server:system` 一类（6.3 节）接管期间的去处；`external` 的"一律 DIRECT"一行；登记非直连的 VPN 路由、外部程序与预匹配、`setsid` 的子进程。
- 本计划末尾两张表照实填。

**判据**：`git grep` 核对文档里引用的用例名、文件与函数都在；门禁绿。

---

## 计划期决定

| # | 事项 | 决定 |
| - | ---- | ---- |
| P1 | 计划引用的名字与行号 | 对着 `ac0a98a` 核对（任务 0 的各项）；M4a 做出来的名字以 M4a 计划的修正记录为准 |
| P2 | 出口在哪里问 | 只在 `open_socket`：策略没有 `interface` 时；策略的网卡绑不上而 `allow-other-interface` 退回时也问（不问的话退回的套接字不绑，增强模式开着时会进虚拟网卡）。策略的 `interface` 绑上了就不问：使用者指定的网卡优先（设计第 3.2 节） |
| P3 | 错误文本 | 出口的错误带目的地址、不带网卡名；绑不上的错误不带网卡名。进请求记录与 API 的文字照"名字与地址只在 DEBUG"的精神不加网卡名；目的地址本来就在会话的记录里 |
| P4 | `DirectPacket` 的套接字 | 按（地址族，出口）各一个、用到时才建；不绑的那几个照今天在打开时就建（保留"开不了的地址族"的语义）；加套接字时唤醒等着的 `recv_from` |
| P5 | 子网从哪里来 | 后端接口的 `subnets`：平台后端读 `if_addrs`，文件后端读它的文件——端到端用例不读本机的网卡 |
| P6 | 共享的状态 | 可执行文件的 `Runtime` 建一次 `PlatformSockets` 与 `PlatformSystemDns`，每一代克隆同一份 `Arc`；出口表的槽是 M4a 的 `EgressSlot` |
| P7 | 纯 UDP 上游 | `UdpUpstream::new(addr, connector)`，自己建套接字的那条路去掉；连接器是 `ResolverDeps::direct`，与 `connectors`（可能是流水线的）分开：纯 UDP 不随 `encrypted-dns-follow-outbound-mode`（设计第 3.3 节，今天就不随） |
| P8 | 重读系统的服务器 | `Resolver::reread_system_servers`（不清缓存）+ `Engine::system_dns_changed`；管理器在标志变了、接管、交还之后调。`network_changed` 照旧只在网络真的变了时调（CLAUDE.md 的规矩） |
| P9 | `OwnResolve` 的退路可以换 | `library` 是参数：生产里是 `SystemResolve`，用例里是假的——不新增调真正的系统解析库的用例 |
| P10 | `resource_client()` 留着 | 不起引擎的命令与用例用它（不绑、系统解析库），守护进程的两个客户端用 `resource_client_with`；扫描用例的名单写明这一处，运行时用例钉住守护进程那两条路 |
| P11 | 名单去掉 rurge 的空间 | 一律去掉，不只在接管期间：没有真的 DNS 服务器在 `198.18.0.0/15` 或 `fd00:6152::/64` 上（设计第 6.3 节"一律"）；崩溃之后系统 DNS 还指着应答器的那段时间，这样也不会把查询送回一个不在的应答器 |
| P12 | `ReadOpts` 不加 DNS 前缀一项 | 订正设计第 3.1 节：rurge 的空间是常数，`netenv` 在共同的路上去掉；`ReadOpts::exclude` 只放虚拟网卡的名字 |
| P13 | ICMP 套接字 | Linux 先数据报后原始，macOS 数据报，Windows 原始（设计第 10 节）；`rurge-platform` 只交出 socket2 的套接字与报文的读写（不加 `tokio` 的依赖边），异步由可执行文件包。原始套接字与 macOS 的数据报套接字收到的 IPv4 应答前面带 IPv4 头，按首字节判；IPv6 不带。本机不是管理员，回环的用例跳过 |
| P14 | 回显的对号 | 标识符每个 Pinger 一个随机数；原始套接字按标识符与序号对，数据报套接字只按序号对（Linux 的内核改写标识符） |
| P15 | Windows 的进程树 | `contains` 这一份恒为 `false`；`in_job`（`OpenProcess` + `IsProcessInJob`）是 M4d 的 unsafe 函数（设计第 3.8 节）。平台后端在 M4d 之前打不开，使用者碰不到 |
| P16 | 等进程 | 只在有外部程序在跑时等，上限 `PROCESS_WAIT`（200 毫秒，与规则同一个，同一个 `SlotProcess`）；等不到照规则走、注 `NOT_IN_TIME`（今天的写法），登记为限制 |
| P17 | 扫描的范围与跳过 | `crates/*/src/` 的生产代码；测试模块（`#[cfg(test)] mod` 起到文件末尾）、`tests` / `testing` 目录与 `testing.rs` 跳过；`//` 之后不看。多找了 `lookup_host(`（系统解析库）、`DirectConnector::new(`（不带钩子的直连）、`build_http(`（hyper 自己建套接字）三种写法：它们都是不经 `open_socket` 出站或问名字的路 |
| P18 | 运行时用例不用参照服务端 | 出口在建套接字之前问，所以每一类上游对着回环上一个不应答的地方查一次就够看到问没问；不依赖本机没有的参照服务端 |
| P19 | 停点 | 做完任务 10 停下；中途遇到设计第 20 节的问题或会改变 M4-D1 ～ D30 的，停下来列出 |

## 执行期修正记录

下面写"执行者的裁定"的，是 Pi 在设计与计划的原则下定的，项目所有者可以改。计划的正文不改：正文里说得不对的地方，这张表的行覆盖它。

| # | 任务 | 修正 | 提交 |
| - | ---- | ---- | ---- |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
