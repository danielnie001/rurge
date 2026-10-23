# 阶段 3 / M3「内存设备上的 TUN 内核」细化设计

| 项       | 内容 |
| -------- | ---- |
| 日期     | 2026-10-18 |
| 状态     | 草案。已决事项 M3-D1 ～ D26 是在项目所有者"自己决定、继续做"的常设授权下定的（代项目所有者作的裁定），所有者可以改。M3a 已做完（2026-10-19，代码在 `crates/rurge-tun`；同一天做了整分支终审，结论是改了再合并：没有 Critical，两条 Important 在 M3a 里都够不到、是 M3b 与 M4 要靠的东西，已按第 18 节第 15 条和 M3a 计划延后事项第 8、12 行改了说明与去向，修正波只动文档、说明、注释和几处不改行为的守卫，留下的次要项各有去向，在 M3a 计划延后事项第 16、17 行）。M3b 的十个任务做完了（2026-10-23，计划 `docs/superpowers/plans/2026-10-22-phase3-m3b-udp-icmp-pool-engine-plan.md`，代码在 `crates/rurge-tun`、`crates/rurge-dns/src/fakeip` 与 `crates/rurge-engine/src/tun.rs`；整分支终审还没有做）。实施期与本文不一致处以第 18 节第 11 ～ 22 条为准（11 ～ 15 是 M3a 的，16 ～ 22 是 M3b 的），正文里被它们订正的地方各有一个指到那里的括号 |
| 上位设计 | [阶段 3 总设计](2026-10-16-phase3-enhanced-mode-design.md) 第 1.4、2.3、3.1、3.2、4、8、12、13 节；不一致处以本文为准，订正列在第 18 节 |
| 需求     | FR-IN-06（协议栈部分：TCP / UDP 经用户态协议栈进入流水线）、FR-IN-08（UDP 会话）、FR-DNS-08（fake-IP 应答器）；FR-RULE-01 的 `pre-matching` 在 DNS 与 SYN 层的一半 |
| 清单     | 3.2 节 `pre-matching`（L266）；4.1 节 REJECT 一族（L325、L333 ～ L336）；6.4 节应答器与 fake-IP（L543 ～ L551）；9.1 节 ICMP 与 fake 段（L709、L710）；`[General]` 的 `allow-dns-svcb`、`hijack-dns`、`always-real-ip`、`icmp-forwarding`（L149 ～ L159） |
| 前置     | 阶段 3 / M1（网络环境）、M2（进程识别）已合并；M2 计划「延后事项」第 17 ～ 19 行的去向是本里程碑 |

M3 做出虚拟网卡的内核：一个平台无关的新 crate `rurge-tun`，在一块**内存里的设备**上把进来的 IP 包分到 TCP、UDP、ICMP 与 DNS 应答器，TCP 经用户态协议栈（smoltcp 0.12，分片）变成连接、UDP 按应用的源端点变成会话，再经引擎现有的 `Dialer` 走规则与策略；连同 fake-IP 池、DNS 应答器与预匹配在 DNS / SYN 两层的拒绝。真实的设备、地址、路由、系统 DNS 与防环是 M4。开发机上能测的是这一里程碑的全部：设备是内存里的，"应用"是测试里的另一个 smoltcp 栈，上游是回环上的测试服务器，什么也不改系统。

## 1. 范围

| 做 | 不做（去向） |
| -- | ------------ |
| 新 crate `rurge-tun`（`forbid(unsafe_code)`）：设备 trait 与内存设备、包的去向、进协议栈之前的过滤与计数、写任务 | 三个平台的真实设备（Linux `/dev/net/tun`、macOS utun、Windows Wintun）、地址与路由、系统 DNS 的接管（M4） |
| TCP：smoltcp 0.12 按四元组分片、SYN 的去处与上限、smoltcp 0.12 隐患的对策、按预算分配的缓冲、连接以 `BoxedStream` 交给引擎 | 拨号之前嗅探第一段数据（阶段 4）；ICMP 差错的转发、`traceroute`（不做，总设计 Q-01） |
| 基准门槛（M3a 的第一个任务）：单流吞吐与 1 万连接的内存；达不到就换自写的 TCP 被动一端 | 真实设备上的数字（手工验收） |
| UDP：按应用源端点的会话，实现 `rurge_inbound::UdpClient`，交给引擎现有的 UDP 流水线；回包的来源写回 fake 地址；`REJECT` 一族回 ICMP"管理上禁止"；大于 MTU 的回包分片写回 | 进来的 IP 分片的重组（不做，丢弃并计数）；`udp-priority`、443 以外端口的 QUIC 与 STUN 识别（M5） |
| ICMP 回显：本机地址本地应答、fake 地址先解析、其余经 `Pinger` trait 转发；`icmp-forwarding` | `rurge_platform::icmp`（真实的 ICMP 套接字，M4） |
| 引擎接入：`Engine::start_tun`（只有测试调用）、`ListenerKind::Tun` 的会话、请求记录与流量、进程查找对 TUN 会话生效（M2 延后第 17 ～ 19 行） | 可执行文件的 `--enhanced-mode`、`--wintun-dll`、增强模式的管理器与 `state.json` 的备份（M4）；起守护进程的用例用的内存设备后端（M4） |
| `rurge-dns::fakeip`：池、双向映射、租约、持久化 | `GET` / `POST /v1/features/enhanced_mode` 生效（M4；计数的形状本文定，第 14 节） |
| DNS 应答器：fake 应答、地址族、`always-real-ip`、SVCB / HTTPS、canary、`hijack-dns`、其余类型经 `Resolver::forward` 转发、DNS over TCP | `[IP Rewrite]`（M5）；`[SSID Setting]` 的 DNS 覆盖对转发的作用照 M1 已有的解析器（转发用这一代的解析器，自然覆盖） |
| 预匹配在 DNS 与 SYN 层：`rurge-rules` 的新入口（DNS 层没有端口）、引擎的钩子（自动升级、记录 5 分钟一次） | 网关模式、DHCP（阶段 7） |
| 经 HTTP / SOCKS5 监听器来的、目的是 fake 地址的会话也换回域名（有池时） | |
| 文档：兼容性清单、`docs/api/phase3.md`、`docs/acceptance/phase3-manual.md`、CLAUDE.md、README | |

## 2. 三份计划

| 计划 | 内容 | 可验收的产出 |
| ---- | ---- | ------------ |
| **M3a 设备、包的去向与 TCP** | `rurge-tun` 的骨架、设备 trait 与内存设备、包的去向（这一份只把 TCP 交出去，UDP 与 ICMP 丢弃并计数）、过滤与计数、写任务、TCP 分片与 `TunStream`、应用端夹具；**第一个任务是基准门槛**（第 6 节）；然后是 SYN 的上限与去处、窗口边缘的守卫与分片的重建、缓冲的预算、拒绝 | 经假 `Dialer` 的 TCP 往返、半关、拒绝的 SYN、上限、守卫；基准数字与门槛的结论 |
| **M3b UDP、ICMP、池与引擎接入** | UDP 会话与引擎 UDP 流水线的两处改动（`answers`、`refused`）、ICMP 与 `Pinger`、回包分片、`rurge-dns::fakeip`、`Engine::start_tun`、`SessionInfo.local` 与进程查找（M2 延后第 17 ～ 19 行）、记录与统计 | 经真实引擎的端到端：TCP、UDP、ping 经 fake 地址（池里预先分配）连到 `[Host]` 钉到回环的名字；拒绝在虚拟网卡上的样子 |
| **M3c 应答器与预匹配** | DNS 应答器与 `Resolver::forward`、DNS over TCP、预匹配在 DNS 与 SYN 层（规则引擎的入口、引擎的钩子、限频、记录）、经监听器来的 fake 地址、fake 应答延迟的基准、文档 | 应答器的每一种应答与转发；预匹配的三种拒绝与限频；从 DNS 查询起的完整一程（查名字 → 得 fake 地址 → 连过去 → 经引擎到回环上的上游） |

池放在 M3b 而不是 M3c（总设计 1.4 节草拟的是 M3c），为的是 M3b 的端到端用例从一开始就能经 fake 地址连到名字、名字由 `[Host]` 钉到回环——目的地是真实 IP 的会话在测试里要格外小心才不会离开回环（第 15.2 节），尽量少写（M3-D1）。

## 3. crate 与接口

### 3.1 `rurge-tun`

平台无关、`forbid(unsafe_code)`，三个平台是同一份代码。依赖 `rurge-inbound`（`Dialer`、`SessionHandle`、`UdpClient`、`UdpAdmission`、`DialError`）、`rurge-proto`（`RejectKind`；若 `rurge-inbound` 已重导出则不另加）、`rurge-dns`（池、报文、解析器的类型）、`rurge-config`（`SessionInfo`、`HostName`、`ListenerKind`）、`rurge-net`（`BoxedStream`、`Target`、`BoxFuture`）、`smoltcp`（工作区依赖，特性不另加）、`tokio`、`tokio-util`（`CancellationToken`：停止，与 `Dialer::associate` 的 `closed`）、`tracing`；不依赖 `rurge-platform`，也不依赖 `rurge-engine`。`RejectKind` 只在 `rurge-proto` 里（`rurge-inbound` 不重导出它），所以要那一条依赖边。`Cargo.lock` 多的只有 `rurge-tun` 自己这一项，没有新的外部包。

模块（计划可以合并或再拆，名字以计划为准）：`device`（trait 与内存设备）、`space`（地址的分配，第 4.2 节）、`packet`（解析、过滤、构造 RST / ICMP / UDP 包、分片）、`tcp`（分片、`TunStream`、守卫、缓冲预算）、`writer`、`udp`、`icmp`、`dns`（应答器，M3c）、`hooks`、`testing`（`testing` 特性：应用端夹具与假的 `Dialer` / `Hooks` / `Pinger`）。

### 3.2 依赖方向

`rurge (bin) → rurge-engine → { rurge-tun → { rurge-inbound, rurge-dns, rurge-config, rurge-net }, rurge-inbound, rurge-policy, rurge-dns, … }`。**引擎依赖 `rurge-tun`**，照它今天自己绑 HTTP / SOCKS5 监听器（`Engine::bind_listeners` 调 `HttpListener::bind`）；`rurge-tun` 从不依赖引擎，引擎经 trait 给它规则、设置与拨号。M3 里可执行文件不变（M3-D2）。

### 3.3 两边之间的接口

`rurge-tun` 起来的样子（签名由计划核对后定稿，这里是形状）：

```rust
pub struct Tun;
impl Tun {
    pub fn start(
        device: Arc<dyn Device>,
        dialer: Arc<dyn Dialer>,     // 引擎：TCP 的 dial / relay，UDP 的 admit_udp / associate
        hooks: Arc<dyn Hooks>,       // 引擎：这一代的设置、预匹配（M3c）
        pinger: Arc<dyn Pinger>,     // M3：测试的假实现；M4：rurge_platform::icmp
        pool: Option<Arc<FakeIpPool>>, // M3b 起由引擎给出
        options: TunOptions,
    ) -> TunHandle;
}

pub trait Hooks: Send + Sync {
    /// 正在用的这一代的设置（廉价的克隆）。
    fn settings(&self) -> Arc<TunSettings>;
    /// 预匹配（M3c）；`None` 是不拒绝。
    fn pre_match(&self, at: &PreMatchAt<'_>) -> Option<RejectKind>;
}

pub trait Pinger: Send + Sync {
    /// 向 `dst` 发一个回显请求、等它的应答，返回应答的负载。
    fn echo<'a>(&'a self, dst: IpAddr, payload: &'a [u8]) -> BoxFuture<'a, io::Result<Vec<u8>>>;
}
```

- `TunSettings`（每一代一份，由引擎在发布新一代时建好、`Hooks::settings` 交出）：这一代的解析器（`Arc<Resolver>`，转发、`[Host]` 的地址映射、ICMP 发往 fake 地址时的解析都经它）、`always-real-ip`、`allow-dns-svcb`、`hijack-dns`、`icmp-forwarding`、出站模式是不是规则模式（预匹配只在规则模式下做）。（M3b 里只有解析器与 `icmp-forwarding`，解析器是 `Arc<dyn Resolve>`，第 18 节第 16 条）
- `TunOptions`（整个 TUN 一份）：是否收 IPv6（`ipv6`，M4 按 `ipv6-vif` 给出；为假时 IPv6 包丢弃并计数）、`drop_hold`（引擎的 `DROP_HOLD`，30 秒）、片数、缓冲与各种上限的常数（都有缺省值，测试可以调小）。
- `TunHandle`：`stop()`（不再收新的连接与会话，已有的随之结束——总设计 Q-28；会话怎样随连接结束，第 18 节第 15 条）、`join()`、`counters()`（第 14 节）。
- 引擎一侧：`Engine::start_tun(self: &Arc<Self>, device, pinger, options) -> TunHandle`：建池（数据目录里的文件，第 10.3 节）、实现 `Hooks`、把自己作为 `Dialer` 交出；引擎停下时停 TUN（取消它的会话，或限时地等 `join`，第 18 节第 15 条）并让池写最后一次（做到的是取消会话，拨号那一半留给 M4，第 18 节第 22 条）。M3 里只有测试调用它，M4 由可执行文件调用。

## 4. 设备与包的去向（M3a）

### 4.1 设备

```rust
pub trait Device: Send + Sync {
    /// 等到至少一个包，把至多 `max` 个完整的 IP 包移进 `into`；出错时 TUN 结束。
    fn recv<'a>(&'a self, into: &'a mut Vec<Vec<u8>>, max: usize) -> BoxFuture<'a, io::Result<()>>;
    /// 按顺序写出 `packets`（写完后清空）；内存设备等到收得下，真实设备收不下就丢（M4）。
    fn send<'a>(&'a self, packets: &'a mut Vec<Vec<u8>>) -> BoxFuture<'a, io::Result<()>>;
    fn mtu(&self) -> usize;
}
```

内存设备 `MemoryDevice::pair(mtu, capacity) -> (MemoryDevice, AppEnd)`：两个方向各一个有界的通道，`AppEnd` 是测试里"操作系统"的那一头。包是 `Vec<u8>`（每包一次分配；0.7 Gbit/s 在 1500 字节时约每秒 5.8 万个包，分配不是瓶颈，基准不过时作为调参的一项）。

MTU：候选 1500 与 9000，由基准定（第 6 节）；两个都过门槛时取 1500——与物理网络一致，应用的 UDP 不会按虚拟网卡探测出比物理路径大的包（M3-D3）。M3a 的基准里两个都过，定为 1500（第 18 节第 11 条）。

### 4.2 地址

| 段 | 用途 |
| -- | ---- |
| `198.18.0.0/15`、`fd00:6152::/64` | rurge 的地址空间（M4 经虚拟网卡路由）；这里的地址 rurge 从不拨出去 |
| `198.18.0.0/24`、`fd00:6152::/120` | rurge 自己的地址：`.1` / `::1` 是虚拟网卡（应用的源地址）、`.2` ～ `.9` 与 `::2` 是应答器（第 11 节）、`.244` 是 `REJECT-NO-DROP` 在 DNS 层的答案、`.253` / `::fd` 是协议栈自己的地址（下一条）；其余不用 |
| `198.18.1.1` ～ `198.19.255.254` | v4 fake 地址：第 *i* 个是 `198.18.1.1 + i`，共 130,814 个 |
| `fd00:6152::` + 32 位 | v6 fake 地址：低 32 位是同一编号的 v4 fake 地址（`198.18.1.1` ↔ `fd00:6152::c612:101`），所以一个编号同时给出两个地址族的地址；不与 `::/120` 重叠 |

smoltcp 的 `any_ip` 只接受"有一条经自己地址的路由"的目的地（`iface/interface/ipv4.rs` 第 178 ～ 207 行，IPv6 在 `ipv6.rs` 第 211 ～ 237 行），所以每一片的接口带地址 `198.18.0.253/32` 与 `fd00:6152::fd/128`，各加一条经它的缺省路由（M3-D4）。

### 4.3 包的去向

```
设备批量收包（一个任务）
  → 解析 IPv4 / IPv6（smoltcp::wire），过滤（第 4.4 节）
  → TCP  → 按四元组分到一片（第 5 节）
  → UDP  → 发往应答器 53 端口、或命中 hijack-dns 的合法 DNS 查询 → 应答器（M3c）
           其余 → 按应用的源端点归入 UDP 会话（M3b）
  → ICMPv4 type 8 / ICMPv6 type 128 → ICMP 回显（M3b）
  → 其余（含 ICMPv6 的邻居发现与 MLD、别的 ICMP 类型、别的协议）→ 丢弃并计数
所有出去的包经一个写任务成批写回设备（第 4.5 节）
```

收包的任务只做解析、过滤与分发，从不等待：发往各片、各会话、应答器的队列都是有界的，满了丢弃并计数（`queue_full`）。

### 4.4 进协议栈之前的过滤与计数

smoltcp 0.12 对包的内容有几处会 panic 或读错的地方（第 5.3 节），所以进任何一处之前先过这几道：

| 情形 | 处理 | 计数 |
| ---- | ---- | ---- |
| 解析不了：长度不对、IPv4 首部长度小于 20（IHL < 5，H4）、IPv4 首部校验和不对、版本不是 4 / 6、传输层首部截断 | 丢弃 | `malformed` |
| IPv4 分片（MF 置位或偏移不为 0）、IPv6 带分片首部 | 丢弃（H5；不重组） | `fragment` |
| 源或目的是未指定地址（H1）、回环、组播；目的是 `255.255.255.255`；IPv4 映射的 IPv6 地址 | 丢弃，不逐包写 DEBUG（组播与广播在真实网卡上很多） | `martian` |
| IPv6 的下一个首部不直接是 TCP / UDP / ICMPv6（扩展首部）；`ipv6` 关着时的全部 IPv6 包；TCP / UDP / ICMP 以外的协议；不是回显请求的 ICMP | 丢弃 | `unsupported` |

TCP 的校验和不验（接口的 `ChecksumCapabilities` 对 TCP 取 `Checksum::Tx`：只算发出的，不验收到的；本地链路上的包由操作系统自己的协议栈算好，验它只花时间，ss-rust 量过关掉之后吞吐多了约三成）；IPv4 首部的校验和由收包的任务验（它很短，坏了会让后面读错）。UDP 与 ICMP 的校验和在交给会话或应答之前验，坏的丢弃并计入 `malformed`（M3-D5）。

### 4.5 写回

一个写任务。每一片推进协议栈时，smoltcp 发出的段写进这一片自己的一个 `Vec`（`TxToken` 从不拒绝，所以 ACK 不会因为设备忙而丢——参考实现里"先取收包令牌、再取发包令牌失败"导致 ACK 丢失、下载停住的教训）；推进完一轮，这一片把攒下的一批交给写任务的全局有界队列，队列满时这一片等（不拿着锁等）；UDP 会话、ICMP、应答器合成的包同样经这个队列。写任务把队列里的批次接起来调 `Device::send`。设备出错时 TUN 结束，全部会话随之结束（M3-D7；会话怎样随连接结束，第 18 节第 15 条）。停止时每片还走最后一轮、写任务给设备一个宽限，见第 18 节第 15 条。

## 5. TCP（M3a）

### 5.1 分片

若干片，每片是独立的一套 smoltcp `Interface` + `SocketSet` + 设备队列，各在自己的任务里推进（smoltcp 的套接字查找是线性扫描，入包与每次推进的出口都遍历整片；维护者建议的做法就是各自独立的几套栈）。新的 SYN 按四元组的散列落到一片，这条连接以后的段都去那一片。片数的候选是 16、64、256，由基准定，缺省 64（16384 个套接字时每片 256 个）。M3a 定为 256 片，16384 个套接字时平均每片 64 个（第 18 节第 11 条）。每片的状态放在一把锁里（`Arc<Mutex<…>>`），这一片的任务推进协议栈时拿着它，`TunStream` 读写时短暂地拿它——同 WireGuard 栈"共享锁 + waker"的做法（M3-D3）。

推进的规矩：收进来的包逐个推进（`Interface::poll_ingress_single`，一次只处理一个包），一批处理完再推进发送（`poll_egress`）——smoltcp 0.12 每次推进发送每个套接字只发一段，所以照 WireGuard 栈（`crates/rurge-proto-wireguard/src/stack.rs` 的 `advance`）反复推进到没有出口为止；每次唤醒至多处理一定数量的包（常数在计划里定），然后让出（`yield_now`），免得一片饿着别的任务。时钟以 TUN 启动时的 `std::time::Instant` 为零点换算成 smoltcp 的 `Instant`（同 WireGuard 栈的 `at`）；不用 smoltcp 自己的 `Instant::now()`（它读墙钟）。

### 5.2 SYN

一个没有对应套接字的、只带 SYN（不带 ACK）的段是新的 SYN。依次：

1. TUN 在停止：回 RST（停止时每片还走最后一轮，第 18 节第 15 条）。
2. 上限（第 5.4 节）：TCP 套接字已有 16384 个、或半开的已有 2048 个：回 RST，计入 `busy`。
3. 目的地：
   - 应答器的地址的 53 端口：本地的 DNS over TCP（第 11.4 节，M3c；M3a、M3b 里同下一条）；
   - rurge 自己的其余地址（含 `.244`）：回 RST；
   - rurge 地址空间里的其余地址（fake 地址）：经池反查域名并取一个租约（第 10 节，M3b）；池里没有：回 RST，计入 `unknown_fake`；M3a 里没有池，一律如此；
   - 其余（真实地址）：原样。
4. 预匹配（第 12 节，M3c，只在规则模式）：`REJECT`、`REJECT-NO-DROP`、`REJECT-TINYGIF` 回 RST，`REJECT-DROP` 丢弃这个 SYN。
5. 建一个只听这个目的端点的套接字（`listen(目的地址:端口)`，不用"监听任意地址"的套接字——smoltcp 有几处目的地检查在没有 `any_ip` 时也放行，H7），再推进这一个包（`poll_ingress_single`）；推进之后套接字不在 SYN-RECEIVED 就拿掉它（smoltcp 没接受这个 SYN）。

**新的 SYN 一个一个地进协议栈**：包是逐个推进的（第 5.1 节），所以建一个监听套接字与推进它的 SYN 之间没有别的包。为什么要紧：smoltcp 的监听套接字只看目的端点（`S/src/socket/tcp.rs` 第 1449 ～ 1474 行的 `accepts`），两个不同来源的 SYN 发往同一个目的端点（浏览器同时开几条连接到同一个服务器）时，若两个监听套接字同时在、两个 SYN 一起进去，排在套接字表前面的那个会接下先到的 SYN，四元组表就对不上了；进了 SYN-RECEIVED 的套接字只接受自己的四元组（M3-D4）。

每片一张四元组 → 套接字的表。重发的 SYN（四元组已有套接字）照普通的段交给 smoltcp；没有套接字的非 SYN 段也交给它，它对这样的段回 RST。第 1 ～ 4 步由 rurge 自己构造的 RST（序号 0、确认号是 SYN 的序号加一）经一个全局的限频：3 秒内至多 100 个，超出的改为丢弃并计入 `rst_limited`（清单 L334；smoltcp 自己对游离段回的 RST 不经它）。

### 5.3 smoltcp 0.12 的隐患与对策

读 smoltcp 0.12.0 的源码得出（`S/` 是它在 cargo 缓存里的目录）：

| 编号 | 隐患 | 对策 |
| ---- | ---- | ---- |
| H1 | 源地址是 `0.0.0.0` 的 IPv4 SYN 被监听套接字接下，下一轮发 SYN-ACK 时 `dispatch_ip` 的 `assert!(!dst.is_unspecified())` panic（`S/src/iface/interface/mod.rs` 第 1134 行；`ipv4.rs` 第 94 ～ 98 行为 DHCP 放行未指定的源） | 过滤（第 4.4 节的 `martian`） |
| H2 | 对端发了超出我方最近一次通告的边缘、但还在接收缓冲物理容量之内的按序数据时，`last_scaled_window` 的 `last_ack + last_win - next_ack`（`S/src/socket/tcp.rs` 第 693 ～ 701 行）对序号做减法而下溢，`SeqNumber` 的减法 panic（`S/src/wire/tcp.rs` 第 75 ～ 85 行），从 `poll_at` / `dispatch` 里冒出来。smoltcp 接受数据到物理容量为止（`tcp.rs` 第 1598 ～ 1600 行），不是到通告的边缘。**窗口缩放一开就是常态**：缩放位数由接收缓冲的容量定死（`tcp.rs` 第 517、538 行，大于 65535 字节就不为 0），每次通告的窗口是空闲空间右移缩放位数，缓冲没被读走时右边缘可能比上一次后退至多 2^缩放位数 − 1 个字节，任何受窗口限制的发送方在途的数据就落到它外面（smoltcp 自己的发送方恰好填满窗口）；缩放位数为 0 时右边缘是"缓冲的起点 + 容量"，只进不退，守规矩的发送方碰不到它 | **接收缓冲不超过 65535 字节**（缩放位数为 0，第 5.4 节）；**窗口边缘的守卫**（下面）与最后一道的 `catch_unwind` 是兜底 |
| H3 | 有乱序数据待收、对端允许 SACK 时，生成 SACK 块的加法在 debug 构建里溢出 panic（`tcp.rs` 第 1404 ～ 1424 行、`storage/assembler.rs` 第 358 ～ 374 行；release 恰好得出对的数） | 进协议栈之前把 SYN 里的 SACK-permitted 选项换成 NOP：smoltcp 就不发 SACK 块（本地链路几乎不丢包，用不着；收到的校验和不验，所以改写选项不必重算） |
| H4 | IPv4 首部长度小于 20 也被接受，"TCP 首部"从 IP 首部里读 | 过滤（`malformed`） |
| H5 | IPv4 分片在检查目的地之前就被缓存（4 个槽、每个至多约 128 KB，稀疏的分片让一个槽搁 60 秒） | 过滤（`fragment`） |
| H6 | 非 TCP 的包让 smoltcp 自己回应（ICMP 回显、端口不可达） | 只把 TCP 交给 smoltcp |
| H7 | 几处目的地检查（`255.255.255.255`、`224.0.0.1`、`ff02::1`、`::1`）在没有 `any_ip` 时也放行，"监听任意地址"的套接字会接下发往它们的 SYN | 按目的端点建监听套接字（第 5.2 节第 5 步）且只为过了过滤的单播 SYN 建 |
| H8 | `DeviceCapabilities::default()` 的 MTU 是 0，几处 `usize` 减法下溢 | 明确设 MTU（IPv4 至少 576、IPv6 至少 1280） |
| H9 | 接收缓冲大于 1 GiB 时 `Socket::new` panic；拿过期的或类型不对的句柄 `get` / `remove` panic | 缓冲的上限远小于 1 GiB；句柄只经四元组表拿，拿掉套接字时同时删表项 |

另有两件不是 panic 的已知缺陷照阶段 2 的做法登记：0.12 的 Cubic 算错窗口（用 Reno）、一次推进每个套接字只发一段（反复推进）。参考实现报过 0.12 在反复的 iperf3 下 `SeqNumber` 减法的 panic（smoltcp #1048，0.13 修了），很可能就是 H2 一类；升到 0.13 要把工作区的 MSRV 从 1.89 升到 1.91，是项目所有者的决定（总设计 Q1）。

**接收缓冲不超过 65535 字节**是对 H2 的主要对策：缩放位数为 0，每次通告的右边缘是"缓冲里第一个没读的字节 + 容量"，读走数据只让它前进（`remote_seq_no` 随读取前移，`tcp.rs` 第 1234 行），发送方只按它听到的最新边缘发，在途的数据就永远在 smoltcp 记下的边缘之内。代价是一条连接上行的窗口至多 64 KiB（本地链路的往返在 0.1 毫秒以下时约 5 Gbit/s，基准量）；发送缓冲与缩放无关，照样可以大（第 5.4 节）。

**窗口边缘的守卫**（H2 的兜底：零窗口时的探测段、构造或有缺陷的段、将来有人把接收缓冲调到 65535 字节以上）：每一片的 `TxToken` 在写出一段的那一刻就读它，记下这条连接最新的边缘 = 确认号 + (窗口字段 << 缩放位数)；缩放位数取我方 SYN-ACK 里的窗口缩放选项（没有就是 0；带 SYN 的段本身不缩放，所以握手刚完成时守卫的边缘比 smoltcp 自己记的小，等于对端眼里的窗口）。进来的段带负载、且负载的末端（序号 + 负载长度）在序号空间里越过这条连接最新的边缘时，丢弃并计入 `beyond_window`（实施期订正：只在这条连接的接收缓冲里还有任何空位时才丢，缓冲满时交给协议栈去答，第 18 节第 13 条）；不带负载的段（ACK、只带 FIN 的段）照常进去。这就是 smoltcp 自己算 `last_scaled_window` 用的那个边缘：它在发出每一段时记下 `remote_last_ack` 与 `remote_last_win`（`S/src/socket/tcp.rs` 第 2520、2521 行；接收时直接回的 ACK 在第 1385、1391 行，同样经 `TxToken` 写出），缩放位数是 `remote_win_shift`（按接收缓冲的容量算，对端不缩放时为 0，第 840、1778 行）。边缘**在写出的那一刻**更新（缩放位数不为 0 时新边缘可能比上一次的小，在一批写完之后才读就会漏掉中间那一次）。这样守卫放过的段永远不会让 `next_ack` 越过 smoltcp 记下的边缘，那个减法就不会下溢；只带 FIN 的段越过边缘不会触发它（之后的状态里 `window_to_update` 不求那个值）。被丢的段对端会重发；smoltcp 自己作发送方时是退回整个窗口重发（`tcp.rs` 第 2286 行，最短的重传超时 10 毫秒，第 152 行），所以守卫在正常传输里动一次就是吞吐的缺陷：基准要求它一次也不动（第 6 节）（M3-D5）。

**最后一道**：一片的推进包在 `catch_unwind` 里（工作区没有 `[profile]` 设 `panic = "abort"`，展开是缺省）。真的 panic 了，这一片整个重建：它的全部套接字作废，这些连接的 `TunStream` 读写以 `ConnectionReset` 结束，应用随后的段成了游离段、由新的一片回 RST；一行 WARN（`tun: the TCP stack of a shard panicked; its connections are reset`，带片号，不带 panic 的文本——那一句在 DEBUG），计入 `stack_panics`。缺省的 panic 钩子同时会往标准错误打印一段，这是最后一道，不为它装钩子（M3-D5）。

### 5.4 缓冲、上限与套接字的设置

- **缓冲**：套接字建立时按一个总预算分配，以后不变（smoltcp 的环形缓冲不能换）：接收 = clamp(剩余预算 ÷ D, MIN, 65535)，发送 = clamp(剩余预算 ÷ D, MIN, MAX)，剩余预算 = 预算 − 已有套接字的缓冲之和（可以为负，那时取 MIN）；所以最坏的总量是预算 + 上限 × 2 × MIN。接收缓冲的上限钉在 65535 字节（缩放位数为 0，第 5.3 节 H2），不是候选；发送缓冲（下行的在途数据）与缩放无关。连接少时每条拿到上限（单流吞吐靠它），连接多时新连接拿到 MIN。候选：MIN 4 KiB 或 8 KiB、发送的 MAX 256 KiB 或 1 MiB、预算 64 MiB 或 128 MiB、D = 32，由基准定（M3-D6）：M3a 定为 MIN 4 KiB、发送的 MAX 256 KiB、预算 64 MiB、D = 32，最坏的总量 192 MiB（第 18 节第 11 条）。本地链路的往返在 0.1 毫秒以下，4 KiB 的窗口也有约 300 Mbit/s（没有在负载下验证过，同一条）。
- **上限**：TCP 套接字 16384 个；半开（收到 SYN、还没建成）2048 个，建成之前过了 10 秒的由 rurge 拿掉（smoltcp 自己不限 SYN-RECEIVED 的时长）；超出时新的 SYN 回 RST（第 5.2 节）。
- **套接字的设置**：关 Nagle；不开保活；`set_timeout` 120 秒（发出的数据 120 秒没有确认就断开——应用在本机，死掉的进程由它的内核回 RST，这一条只兜底）；拥塞控制 Reno；ACK 延迟取 smoltcp 的缺省，基准不过时作为调参的一项。**实施期订正**：开保活（30 秒）、`set_timeout` 600 秒——smoltcp 0.12 的超时不看有没有在途的数据，这里"不开保活、120 秒"的写法会让空闲的连接两分钟就断（第 18 节第 12 条）。

### 5.5 `TunStream`

建成的连接以 `TunStream`（实现 `AsyncRead` / `AsyncWrite`，装进 `BoxedStream`）交给引擎：

- 读：从套接字的接收缓冲取；没有数据时登记 smoltcp 的接收 waker（`register_recv_waker`，`async` 特性已开）并返回 `Pending`；应用发了 FIN 且缓冲读空时返回 0。
- 写：写进发送缓冲；满了登记发送 waker。
- 读写之后"踢"这一片一下（让它推进，好发窗口更新与数据）。
- `poll_shutdown`：`close()`，缓冲里的数据发完之后发 FIN。
- 没有 `shutdown` 就被丢掉：`abort()`，回 RST。
- 拿掉套接字：smoltcp 报 CLOSED 或 TIME-WAIT 时，或流被丢掉之后过了 60 秒（总有一个先到）；拿掉时同时删四元组表项、还回缓冲预算、放掉 fake 地址的租约（第 10.2 节）。（套接字到 TIME-WAIT 或 Closed 而流还有没读的字节时，字节先交给流、套接字照样拿掉，第 18 节第 21 条）

### 5.6 握手之后

握手照常完成之后建会话（总设计 8.2 节；应用先看到连接建立，拨号失败再看到 RST——sing-box 的系统栈同样如此）：`SessionInfo` 的 `src` 是应用的地址与端口、`local` 是应用当初连的目的端点（fake 地址或真实地址，第 9.2 节）、`in_port` 0、`listener` `Tun`、`dst_host` 是反查出的域名或目的 IP、`dst_port`、`transport` TCP。交给 `Dialer::dial`：

| 结果 | 虚拟网卡上 |
| ---- | ---------- |
| 拨通 | `Dialer::relay(TunStream, 上游, handle)` |
| `REJECT-DROP`（含自动升级来的） | 不读不写地拿着连接 `drop_hold`（30 秒，同 HTTP 监听器），然后 FIN，再读掉应用还在发的字节直到它的 FIN，至多 `linger`，然后放手（第 18 节第 14 条） |
| `REJECT`、`REJECT-NO-DROP`、`REJECT-TINYGIF` | RST（没有嗅探，不知道是不是 HTTP，所以 TINYGIF 不回图片） |
| 拨不通（拒绝连接、超时、DNS 失败） | RST |

不等应用的第一段数据再拨号（同今天的监听器）；SNI 照今天由转发循环记录（M3-D10）。

## 6. 基准门槛（M3a 的第一个任务）

第一个任务只做出走得通的最小的一条 TCP 路（设备 trait 与内存设备、只交 TCP 的去向、分片、写任务、`TunStream`、应用端夹具、假的 `Dialer`），接上转发循环的替身与回环上的上游，量两样（总设计 3.1 节）：

- **B1 单流吞吐**：rurge 一侧（收包、各片、写任务、转发循环的替身、到上游的连接）全部在一个线程的当前线程运行时上；应用端的 smoltcp 栈与回环上的上游服务器在别的线程上。上行与下行各量一次，各 64 MiB，MTU 1500 与 9000 各一遍。转发循环的替身是 `tokio::io::copy_bidirectional_with_sizes(…, 8192, 8192)`（与引擎的 `BUF` 相同的两块 8 KiB）。**判据：每个方向不低于 0.7 Gbit/s**（总设计的"NFR-01 目标的 70%"）。量的那一程里 `beyond_window`、`queue_full`、`rst_limited` 与 `stack_panics` 都是 0，否则这一次不算数：丢一个段会让 smoltcp 的发送方退回整个窗口重发，量到的就不是协议栈的速度，拿它判门槛会为错的原因去重写 TCP。所以各片的收包队列与写任务的队列至少装得下一个窗口的包（65535 字节 ÷ MSS，MTU 1500 时约 45 个；发送的 MAX 是 1 MiB 时下行约 720 个）。同时量一次夹具自己的上限：应用端的栈与另一个只监听的 smoltcp 栈直接相连、不经 rurge，同样的线程分布；rurge 的数字接近它时瓶颈在夹具，不算 smoltcp 不够快。
- **B2 1 万条连接**：1 万条空闲连接——应用一侧是测试里写的最小 TCP（握手、发一段 1 KiB、确认收到的一切），上游一侧在内存里（`tokio::io::duplex`，缓冲至多 1 KiB；每条向应用发 8 KiB 之后不再动，好让各块缓冲真的被用过）——加 100 条活跃的（应用一侧是 smoltcp、上游是回环上的 TCP，持续双向传送）。量进程的峰值常驻内存（Windows 由测试自己起只读的 `powershell -NoProfile -Command "(Get-Process -Id <pid>).PeakWorkingSet64"`，Linux 读 `/proc/self/status` 的 `VmHWM`），并报 1 万条空闲连接在场时 rurge 那个线程每秒推过的包数（每包的时间是它的倒数；开发机上进程的 CPU 时间不可信，所以不报 CPU 时间）。**判据：峰值常驻内存不超过 300 MB ÷ 0.7 ≈ 429 MB**；每包的时间只报告、不作判据。测试一侧自己的内存（应用端的栈、内存管道）压到最小，并在结果里写明它占多少。

两个都是忽略的用例（`#[ignore]`，release 下手动跑，`--nocapture` 打印数字），不作门禁。片数、MTU、缓冲的候选在这一步定下来，数字记进 M3a 计划的执行期修正记录与本文第 18 节（M3a 量完了：门槛怎样读、数字、选定的缺省值和没有转向自写的 TCP，见第 18 节第 11 条）。**调参之后仍达不到时，换成自己写的 TCP 被动一端**（总设计 3.1 节已授权这个转向，不必再回来定）：接口不变（包进、包出、连接以 `BoxedStream` 交出），M3a 计划在那时改写其余任务（M3-D9）。

真实设备上还有系统调用的开销，内存里的基准不含它；真实设备上的数字在手工验收里量。

## 7. UDP（M3b）

### 7.1 会话

应用的每个源端点（地址 + 端口）一个 UDP 会话，实现 `rurge_inbound::UdpClient`，以引擎现有的 UDP 流水线服务它（`Dialer::admit_udp` 再 `Dialer::associate`）：一个会话里每个目的地是引擎的一条流（全锥、同一个出站共用载体、流空闲 60 秒、DNS 流应答之后 10 秒回收、流量进请求记录，都是引擎已有的）。会话的模板 `SessionInfo`：`src` 是应用的源端点、`listener` `Tun`、`transport` UDP、`in_port` 0、`local` 没有。

- 两个方向都 60 秒没有包时会话结束（取消交给 `associate` 的 `closed`）。
- **计入引擎的 4096 个关联**（与 SOCKS5 共用）：`admit_udp` 答 `Busy` 时这个包丢弃并计入 `busy`；每个会话的流仍至多 1024 条（P2P 应用一个端口对很多对端时会碰上，登记）。
- 应用发来的包进每个会话自己的有界队列（至多 256 个），满了丢弃并计入 `queue_full`；`UdpClient::recv` 从它取。

### 7.2 目的地与回包的来源

应用发往 fake 地址的包，目标是 `域名:端口`（经池反查，会话为这个目标拿一个租约，会话结束时放掉；目标两个方向都空闲 `udp_idle` 时早放，第 18 节第 19 条）；发往真实地址的，目标是那个地址。池里没有的 fake 地址：丢弃，计入 `unknown_fake`。会话为每个目标记下原目的端点（应用当初发往的地址与端口）和最近一个包的 IP 首部与 UDP 首部（合成 ICMP 时引用，第 7.3 节）。

回包的来源要是应用能认的地址。引擎的 `receive()` 今天按载体报回的来源找流（`Routes::flow_for`：先找目标恰好相同的流，再找同端口的第一条域名流，最后随便一条）只为记流量。改动：**`UdpClient::send(payload, from, answers: Option<&Target>)`**，`answers` 是这个回包应答的那条流的目标（应用写的样子）——目标恰好等于 `from` 时，或同端口的域名流恰好只有一条时给出，否则没有（M3-D13）。虚拟网卡一侧：

1. 有 `answers`：来源写成那个目标的原目的端点（fake 地址也一样）；
2. 没有、`from` 是 IP：来源就是它（全锥：第三方发来的包，如 STUN 的对端）；
3. 没有、`from` 是域名：经池正查这个名字在会话所用地址族的 fake 地址；池里没有就丢弃，计入 `unknown_fake`。

已知的限制：一个应用的套接字在同一个端口上同时对两个域名收发、出站又是全锥的载体时，回包可能写成另一个名字的 fake 地址（登记）。SOCKS5 的 `UdpClient` 不看 `answers`。

### 7.3 拒绝

引擎对一条流作出 `REJECT` 一族的决定时（规则、`block-quic`、策略不载 UDP 而 `udp-policy-not-supported-behaviour` 是 `REJECT`），对这条流被丢掉的每个包调 **`UdpClient::refused(&self, target: &Target, kind: RejectKind)`**（有缺省的空实现；SOCKS5 不改，照旧静默丢弃）。虚拟网卡一侧（清单 L335）：

| 决定 | 回应用 |
| ---- | ------ |
| `REJECT`、`REJECT-TINYGIF` | ICMP"管理上禁止"（ICMPv4 type 3 code 13、ICMPv6 type 1 code 1），经一个全局的限频：3 秒内至多 100 个，超出的不回、计入 `icmp_limited` |
| `REJECT-NO-DROP` | 同上，不限频（清单的读法：手册只对 TCP 写了"3 秒 100 次"） |
| `REJECT-DROP` | 什么也不回 |

ICMP 的来源是那个目标的原目的地址，引用的是这个目标最近一个包的 IP 首部加 UDP 首部（应用的内核按引用的首部找到它的套接字）。UDP 的 `REJECT` 照旧不计入自动升级（阶段 2 / M5a）。

### 7.4 大于 MTU 的回包

回包连同 IP 与 UDP 首部大于设备的 MTU 时，按 IPv4（清掉 DF，偏移以 8 字节为单位）或 IPv6（分片首部）的规矩分片写回，应用的内核负责重组（同普通网卡上内核的做法；DNS over UDP 的大应答、游戏的大包靠它）。大于 65535 字节的不可能出现（载体给出的数据报至多 64 KiB）。进来的分片照旧丢弃（第 4.4 节）（M3-D15）。

## 8. ICMP（M3b）

回显请求（ICMPv4 type 8、ICMPv6 type 128），不经规则与策略（"ICMP cannot be proxied"，清单 L709）：

| 目的 | 处理 |
| ---- | ---- |
| `icmp-forwarding = false` | 丢弃 |
| rurge 自己的地址（应答器、`.244`、协议栈自己的地址） | 本地应答 |
| fake 地址 | 经池反查域名，经这一代的解析器解析（同地址族优先），向真实地址转发；应答的来源写回 fake 地址；池里没有：丢弃，计入 `unknown_fake`；解析不出：丢弃 |
| 其余 | 经 `Pinger` 转发 |

`Pinger::echo` 发一个回显请求、返回应答的负载；标识与序号由它自己管（M4 的真实实现另起），虚拟网卡一侧记下应用的（标识，序号）与原目的地，按它们写回显应答交回应用。5 秒没有应答的不再等；在途的至多 1024 个，超出的丢弃并计入 `busy`。M3 只有测试里的假 `Pinger`（照负载回、失败、迟迟不答）；`rurge_platform::icmp`（真实的 ICMP 套接字，Linux 与 macOS 优先无特权的数据报型、Windows 用原始套接字）与它绑在物理网卡上都是 M4（M3-D16）。

rurge 自己合成的 ICMP 只有第 7.3 节的"管理上禁止"。ICMP 差错不转发（总设计 Q-01）。

## 9. 引擎接入（M3b）

### 9.1 `Engine::start_tun`

见第 3.3 节。引擎实现 `Hooks`：`settings()` 交出这一代的 `TunSettings`（引擎在发布新一代时重建它，读的一方廉价地克隆一个 `Arc`）；`pre_match` 在 M3c。TUN 的 TCP 会话经 `Dialer::dial` 进流水线（`Engine::dial` 里的暂停检查、进程查找、规则、策略、拨号与记录都与监听器的会话相同）；UDP 会话经 `admit_udp` / `associate`。

### 9.2 `SessionInfo.local` 与进程查找

`SessionInfo` 加 **`local: Option<SocketAddr>`**：会话在客户端连接上 rurge 这一侧的端点。HTTP / SOCKS5 监听器的会话是**那一条被接受的连接自己的**本地地址：接受时读 `TcpStream::local_addr()`，经 `to_canonical()` 归一。不是监听器的 `ctx.local`——那是绑定时取的地址，`http-listen = 0.0.0.0:6152`（常见的写法）与 `allow-wifi-access` 的缺省监听都是 `0.0.0.0:6152`，双栈是 `[::]:6152`，照"未指定地址只比端口"的规矩 M2 延后第 17 行对它们就没有关掉。SOCKS5 的 UDP 关联的模板同样带它控制连接的本地地址（关联按控制连接查进程，阶段 3 / M2）。虚拟网卡的 TCP 会话是应用当初连的目的端点；虚拟网卡的 UDP 会话模板与内部会话没有（M3-D12）。

用它关掉 M2 延后第 17 行：引擎的 `OwnerQuery` 与平台的 `procinfo::Query` 把 `remote_port: Option<u16>` 换成 **`remote: Option<SocketAddr>`**，TCP 的行要远端的地址与端口都相等（地址经 `endpoint` 归一；查询里的地址是未指定地址时只比端口）。查询的形状按监听器与传输定（`Engine::look_up_owner` 今天只有一种：TCP、`local` 是 `src`、`remote_port` 是 `in_port`）：HTTP 与 SOCKS5 的会话（含按控制连接查的 SOCKS5 UDP 关联）与虚拟网卡的 TCP 会话发 TCP 的查询，`local` 是会话的 `src`、`remote` 是会话的 `local`——监听器的会话是"应用连到 rurge 的监听端口"那一行，虚拟网卡的会话是"应用连到原目的地"那一行，两种都由四元组唯一确定；虚拟网卡的 UDP 会话发 UDP 的查询，只按本地端点（应用的源端点），没有 `remote`（平台层的 UDP 查找早就有，引擎此前从没发过）。`looked_up` 对 `Tun` 改为真（它的固定用例随之改；`looked_up` 由 `query_for` 取代，第 18 节第 20 条）：虚拟网卡的会话来自本机的应用，源地址是虚拟网卡的地址（M4 起是本机网卡上的地址，`is_local` 答"是"；M3 的用例用假的 `OwnerLookup`）。

M2 延后第 18 行（Linux 上整张表一行也解析不出时报 `Layout` 加一次 WARN，并改 `ReadError::Layout` 的注释）与第 19 行（macOS 双栈的 pcb——`inp_vflag` 同时有 IPv4 与 IPv6——出两行，一行 IPv4、一行 IPv6，好让从 IPv6 地址发出的 UDP 查得到）随这一份计划做；两处都只改平台层的纯函数与它们的夹具用例。

### 9.3 记录、流量与计数

虚拟网卡的会话照常进请求记录与 API：`listenPort` 是 `null`（`in_port` 为 0，`crates/rurge-api/src/routes/requests.rs` 已是这样）、`listener` 是 TUN 的那个名字、`pid` / `processPath` 照 M2。按监听器分的流量统计加上 `Tun`（写计划时核对统计的键，第 17 节 V4）。

### 9.4 拒绝在虚拟网卡上的样子

| 层 | `REJECT` | `REJECT-DROP` | `REJECT-NO-DROP` | `REJECT-TINYGIF` |
| -- | -------- | ------------- | ---------------- | ---------------- |
| DNS（预匹配，M3c） | 没有记录的应答；同一主机自动升级之后不应答 | 不应答 | A 答 `198.18.0.244`、AAAA 答空 | 同 `REJECT` |
| SYN（预匹配，M3c） | RST；自动升级之后丢 SYN | 丢 SYN | RST | 同 `REJECT` |
| 握手之后（规则） | RST | 拿着 30 秒再 FIN，再读掉应用还在发的字节（第 18 节第 14 条） | RST | RST |
| UDP（规则） | ICMP 管理上禁止（限频） | 不回 | ICMP 管理上禁止（不限频） | 同 `REJECT` |

"3 秒 100 个"的 RST 限频与 ICMP 限频各是一个全局的计数；自动升级（同一主机 30 秒 50 次，清单 L325）用引擎已有的 `Escalation`，DNS 层与 SYN 层的 `REJECT` 与 `REJECT-TINYGIF` 计入它（UDP 照旧不计）。连往 `198.18.0.244` 的 TCP 回 RST（它是 rurge 自己的地址），UDP 回 ICMP 管理上禁止（不限频）。

## 10. fake-IP 池（M3b）

`rurge-dns::fakeip`（`rurge-dns` 已依赖 `lru` 0.18.4，不加包）。

### 10.1 地址与名字

编号 *i*（0 ～ 130,813）同时给出 v4 地址 `198.18.1.1 + i` 与 v6 地址 `fd00:6152::` 加那个 v4 地址（第 4.2 节），所以一个名字只占一个编号。名字先规范化（小写、去掉末尾的点），每个标签只含 `a`–`z`、`0`–`9`、`-`、`_`，整个至多 253 字节；不合这个样子的名字不给 fake 地址，查询照其余类型转发（第 11.2 节）。简单主机名与 `.local` 照常给（总设计 Q-11）。

### 10.2 分配、回收与租约

- 正查（应答器要一个地址）：名字已有编号就用它并刷新"最近用到"；没有就先用游标往后的新编号，游标到头（回绕）之后回收最久没被用到的编号（`lru`）。
- 反查（TCP 连接到来、UDP 包到来、ICMP 到来）：刷新"最近用到"。
- **租约**：TCP 连接在它的一生里、UDP 会话为它的每个目标在会话的一生里（目标空闲 `udp_idle` 时早放，第 18 节第 19 条）各拿一个租约（释放时自动还回的守卫）；**有租约的编号从不回收**——TCP 只在连接到来时反查一次，单靠"最近用到"挡不住长连接的地址被回收给别的名字。
- 全部编号都有租约时（耗尽）：应答器对要新地址的查询回 SERVFAIL，一次 WARN（`tun: every fake address is in use; new names get SERVFAIL`）。
- fake 应答的 TTL 30 秒（清单 L545；总设计 Q-19 取 Mac 的缺省）。

### 10.3 持久化

数据目录里的 `fake-ip.txt`（UTF-8 文本）：第一行 `rurge-fake-ip 1`；第二行 `cursor <n> wrapped <0|1> clean <0|1>`；其余每行 `<编号> <名字>`，最久没用到的在前。有改动时至多每 10 秒写一次，TUN 停下时写一次（`clean 1`）；先写临时文件再改名，不 fsync，在阻塞线程上写（同 `StateStore` 的做法）。读不了、读不懂（第一行不对、编号越界、名字不合第 10.1 节）：从空表开始，一次 WARN，文件留着不动，下一次写时覆盖。文件至多约 130,814 行、约 4 MB（M3-D18）。（打开时就写 `clean 0`、读不了的文件也隔离、日志在谁那里写，第 18 节第 17 条）

**上一次没有干净地停下**（`clean 0`：崩溃或被强杀）：最后一次写之后发出去的地址没进文件，应用手里可能还有。所以载入之后，分配器接下来会交出的 2048 个编号隔离 120 秒——没回绕时是游标往后的 2048 个，回绕之后是最久没用到的 2048 个（它们在文件里的映射也丢掉：可能已被没记下的分配换了名字）。120 秒之后隔离解除。2048 个大约是每秒 200 次新分配在 10 秒里的量；120 秒覆盖应用自己的缓存（M3-D18）。（文件里没有名字的编号另隔离，与这 2048 个叠加，第 18 节第 18 条）

### 10.4 未知的 fake 地址

连到 rurge 地址空间里而池里没有的地址（重启之前的地址已被回收、从来不是 rurge 给的、或没有池）：TCP 回 RST，UDP 与 ICMP 丢弃，都计入 `unknown_fake`；每次一行 DEBUG（带地址），第一次另有一行 WARN（不带地址）（总设计 Q-07 写的"记一行日志"按这个读，M3-D11）。这些地址从不交给引擎去拨。

## 11. DNS 应答器（M3c）

### 11.1 哪些包交给它

- 发往 `198.18.0.2` ～ `198.18.0.9` 或 `fd00:6152::2` 的 53 端口的 UDP 与 TCP（清单 L543）；
- `hijack-dns` 的每一项（IPv4 地址或 `*`，可带端口，缺省 53）：只对 UDP、只对 IPv4，且只有解析得出合法 DNS 查询的包才算，别的照常作 UDP 会话；应答的来源是被劫持的那个服务器的地址与端口（清单 L547）。发往别的服务器 53 端口的 TCP 照常作 TCP 连接。

### 11.2 应答

按顺序（第一条成立的决定应答）：

1. 不是标准查询（opcode 不是 0）、class 不是 IN、问题数不是 1：FORMERR 或 NOTIMP；
2. `use-application-dns.net`：NXDOMAIN（清单 L551）；
3. SVCB（64）与 HTTPS（65）：`allow-dns-svcb` 关着时 NOTIMP（清单 L549）；
4. A / AAAA：
   1. 预匹配（第 12 节，只在规则模式）：`REJECT` / `REJECT-TINYGIF` 回没有记录的 NOERROR（同一主机自动升级之后不应答）、`REJECT-DROP` 不应答、`REJECT-NO-DROP` 对 A 答 `198.18.0.244`、对 AAAA 答空（rurge 的读法，登记）；
   2. 地址族：A 经 IPv6 来、AAAA 经 IPv4 来的，空应答（NOERROR、没有记录；手册："an AAAA question arriving over IPv4 gets an empty answer, and vice versa"）；
   3. `always-real-ip` 命中：名字在 `[Host]` 里有地址映射的，由应答器直接权威应答（TTL 30 秒，清单 L548）；没有的，原样转发（第 11.3 节），应答里是真实地址；
   4. 名字不合第 10.1 节：转发；
   5. fake 地址，TTL 30 秒；池耗尽时 SERVFAIL。
5. 其余类型：经 `Resolver::forward` 原样转发，应答原样交回（ID 换回查询的）。

查询带 OPT 的，应答带 OPT，UDP 负载大小 1232。UDP 上的应答大于客户端允许的大小（有 OPT 时取它的值与 1232 的较小者，没有时 512）时置 TC、去掉记录，客户端改用 TCP。同时在转发的查询至多 256 个，超出的回 SERVFAIL（M3-D19）。

### 11.3 `Resolver::forward`

`rurge-dns` 新加 `Resolver::forward(&self, query: &[u8]) -> io::Result<Vec<u8>>`（异步）：把查询报文发给这一代的主上游（与内部 DNS 客户端相同的服务器配置，`[SSID Setting]` 的覆盖随这一代的解析器自然生效），遵守 `[Host]` 的 `server:`（清单 L545），发出时换一个新的 ID、收到后换回；不进缓存。`server:syslib` 与 `server:force-syslib` 写明的名字：系统解析库只能问 A / AAAA，所以这样的名字的其余类型转发给系统当前的 DNS 服务器（登记；M4 起系统 DNS 指向应答器，那时系统的服务器列表要排除应答器与虚拟网卡的地址，总设计第 4 节已记）。

### 11.4 DNS over TCP

发往应答器 53 端口的 TCP 连接照常完成握手，之后不交给引擎，在本地按长度前缀读查询、逐个应答（同第 11.2 节），空闲 10 秒关掉。

## 12. 预匹配在 DNS 与 SYN 层（M3c）

### 12.1 规则引擎的入口

今天的 `pre_match_domain` / `pre_match_ip`（`crates/rurge-rules/src/engine.rs`）按 `SessionInfo::tcp(..)` 求值（来源 `127.0.0.1:0`、监听器 HTTP），必须带端口，而且没有调用者。换成一个入口：给出会话（`SessionInfo`：真实的来源、`ListenerKind::Tun`、目的是名字或 IP）和这一次是哪一层（DNS 或 SYN），照现有的预匹配那一遍（`positions()` 上逐条、`EvalCtx::with_network`、从不解析）求值，返回命中的规则与策略。

**DNS 层没有端口**：编译规则时记下每条规则（连同逻辑规则的每层子规则）是否含 `DEST-PORT` 或 `SRC-PORT`；DNS 层跳过含它们的规则（不是当作"不命中"——`NOT,((DEST-PORT,443))` 当作不命中会让 `NOT` 成立而拒绝一切）；集合里逐条求值的行同样，含端口的行在 DNS 层不命中。`SRC-IP` 是查询的源地址（本机的应用共用虚拟网卡的地址），`SUBNET` 看当前网络；IP 类的规则在 DNS 层永不命中（只有名字、从不解析）；SYN 层的会话是这条连接（域名经 fake 地址反查，或目的 IP），端口齐全（M3-D20）。

### 12.2 引擎的钩子

`Hooks::pre_match` 由引擎实现：

- 出站模式不是规则模式时不看（总设计 Q-27）；
- 用这一代的规则引擎求值；
- 命中 `REJECT` 或 `REJECT-TINYGIF` 的，记进 `Escalation`，过了同一主机 30 秒 50 次就改答 `REJECT-DROP`；
- 记请求记录（第 12.4 节）；
- 同步、不等任何东西（预匹配从不解析；`PROCESS-NAME` 不能带 `pre-matching`），在收包的任务或应答器的任务上直接调。

### 12.3 两层的行为

见第 9.4 节的表。SYN 层的 RST 经第 5.2 节的限频（3 秒 100 个）；DNS 层"不应答"就是什么也不写回。UDP 没有预匹配（清单 L335）。

### 12.4 请求记录

被预匹配拒绝的会话进请求记录，`notes` 写 `pre-matching at DNS` 或 `pre-matching at SYN`；同一代里同一条规则 5 分钟内只记一次（清单 L336，它的阶段从 4 改为 3）；换代时从头记。DNS 层的记录是那个名字、没有端口；SYN 层的是那条连接。

## 13. 经监听器来的 fake 地址（M3c）

有池时（TUN 起着），经 HTTP / SOCKS5 监听器来的、目的是 rurge 地址空间里的 IP 的会话，引擎在规则之前经池换回域名（同虚拟网卡的会话，拿租约到会话结束）；池里没有：会话失败，`error` 是 `fake address not known`，不拨。没有池时（不开增强模式）照普通地址处理（M4 定是否在不开增强模式时也载入池）（M3-D21）。

## 14. 日志、计数与个人信息

- 计数（`TunHandle::counters()`，一份快照）：丢弃的包按原因——`malformed`、`fragment`、`martian`、`unsupported`、`queue_full`、`unknown_fake`、`busy`、`beyond_window`、`rst_limited`、`icmp_limited`——和 `stack_panics`。M4 起由 `GET /v1/features/enhanced_mode` 给出，形状 `{"enabled": <bool>, "dropped": {"malformed": <n>, …}, "stackPanics": <n>}`（M3-D22）。
- 每种原因第一次出现时一行 WARN（不带地址；`martian` 不报——真实网卡上的组播与广播很多），其余 DEBUG。
- 名字（DNS 查询、fake 地址反查出的域名）与地址只在 DEBUG 的行与请求记录里；INFO 及以上的行里没有（同会话的日志行今天的做法，计划核对，第 17 节 V12）。
- 凭据的规矩不变；虚拟网卡一侧不碰凭据。

## 15. 测试

沿用的规矩全都适用（只用回环与端口 0、失败时也不离开回环与自己的临时目录、有界等待、新写的计时用例提交前单独循环跑二十次以上）。M3 的用例从不建网卡、不改路由与 DNS、不发真实的 ICMP。

### 15.1 应用端夹具

`rurge_tun::testing`（`testing` 特性）：

- `AppStack`：测试里"操作系统"一侧的 smoltcp 栈，接在内存设备的 `AppEnd` 上，地址是虚拟网卡的 `198.18.0.1` / `fd00:6152::1`，能对任意目的地开 TCP 连接（它的套接字交给用例读写；接收缓冲同样不超过 65535 字节——它也是 smoltcp，第 5.3 节的 H2 对它一样成立），也能用 `smoltcp::wire` 现场构造任意的 IP 包（UDP、ICMP、坏包、分片、越过窗口的段）并收下 rurge 写回的包；可以注入丢包与损坏。推进的规矩同第 5.1 节（反复推进到没有出口）。
- 假的 `Dialer`（记下每次 `dial` 的 `SessionInfo`；按用例的安排接到回环上的上游或内存管道、或拒绝、或失败）、假的 `Hooks`、假的 `Pinger`。

### 15.2 经引擎的端到端用例与回环

在 `crates/rurge-engine/tests/tun*.rs`，引擎经 `Engine::start_tun` 接上内存设备。**经虚拟网卡的会话不能以回环为目的**（过滤会丢掉），所以：

- 目的是名字的：经 fake 地址——用例先从池里为名字取一个地址（M3b），或经应答器问到（M3c）——连过去，名字由 `[Host]` 钉到 `127.0.0.1`（或 MockDns 的 `target.test`），引擎拨的是回环上的测试服务器。
- 目的是真实 IP 的（只在要看 `dst_host` 是 IP 的用例里）：用文档地址（`192.0.2.0/24`、`2001:db8::/32`），**两道防线**：第一道是要测的规则把它交给一个只记下目标、从不往外拨的回环 SOCKS5 假上游，或交给 `REJECT` 一族；第二道是紧跟其后的 `SRC-IP,198.18.0.1/32,REJECT`（与第一道是不同的匹配器）——规则失灵时会话被拒，到不了 harness 追加的 `FINAL,DIRECT`。UDP 与 ICMP 同样。
- ICMP 用假 `Pinger`，什么也不发出去。
- 碰到进程查找的用例用假的 `OwnerLookup`、不用暂停的时钟（M2 的规矩）。

### 15.3 基准（忽略的用例）

B1、B2（第 6 节）；fake 应答的延迟（M3c；NFR-01：小于 0.2 毫秒；在应答器的入口与写回之间量，不含设备）。都不作门禁，数字记进各计划的执行期修正记录。

## 16. 文档

每份计划的最后一个任务更新它碰到的文档；M3c 收尾时：

- 兼容性清单：4.1 节 L333 ～ L336（L336 的阶段 4 → 3）、6.4 节 L543 ～ L551、9.1 节 L709 ～ L710、`[General]` 的 `allow-dns-svcb`、`hijack-dns`、`always-real-ip`、`icmp-forwarding` 的"目前"备注（虚拟网卡的内核在内存设备上做完、测完，真实设备随 M4）；与手册不同的读法登记（NO-DROP 的 AAAA 空应答、`syslib` 名字的其余类型、P2P 的 1024 条流、同端口两个名字的回包来源、握手之后 TINYGIF 回 RST、REJECT-DROP 拿 30 秒再 FIN 并读掉应用还在发的字节，见第 18 节第 14 条）。
- `docs/api/phase3.md`：M3 一节（虚拟网卡的会话在请求记录里的样子、`notes` 的两句、计数的形状与它们 M4 起在哪里出现、`fake address not known`）。
- `docs/acceptance/phase3-manual.md`：M3 一节（真实设备之前能做的不多：计数与日志行的核对放到 M4；这一节至少记下内存基准与真实设备上数字的差别要在 M4 量）。
- CLAUDE.md（状态、先读这些文档、依赖方向、常用命令）、README 两份。

## 17. 写计划时要核对的事项

- V1：smoltcp 0.12 的接口：`Interface::new(Config, &mut Device, Instant)`、`set_any_ip`、`routes_mut().add_default_ipv4_route` / `add_default_ipv6_route`、`update_ip_addrs`、`poll` 的返回值、`poll_delay`；`tcp::Socket` 的 `new` / `listen` / `set_nagle_enabled` / `set_timeout` / `set_keep_alive` / `set_ack_delay` / `register_recv_waker` / `register_send_waker` / `recv_slice` / `send_slice` / `close` / `abort` / `state` / `may_recv` / `can_send`；`SocketSet` 的 `add` / `remove` / `get_mut`；`DeviceCapabilities`（`Medium::Ip`、`max_transmission_unit`、`checksum`）、`RxToken` / `TxToken` 的签名；`SocketBuffer::new`。
- V2：`any_ip` 加经自己地址的路由的接受条件（`ipv4.rs` 第 178 ～ 207 行、`ipv6.rs` 第 211 ～ 237 行）；没有套接字接下的段 smoltcp 回不回 RST、`any_ip` 下回给谁；SYN-RECEIVED 的套接字不接受别的来源的 SYN（`accepts`）。
- V3：窗口边缘：`tcp.rs` 里 `remote_last_ack`、`remote_last_win`、`remote_win_shift` 在发出每一段时怎样更新，与守卫从发出的段读出的边缘是否逐字节相同；smoltcp 接受数据到哪里（第 1598 ～ 1600 行）。
- V4：`Dialer` 的约定对一个不是套接字的入口：`SessionHandle` 由谁建（`Engine::dial` 里）、`Dialed.forward` 不用、`DialError` 的各种情形；按监听器分的流量统计的键与 `ListenerKind::Tun` 在记录与 API 里的名字。
- V5：`SessionInfo` 在整个工作区的每一处结构体字面量（加 `local` 字段要改的地方）；`OwnerQuery` 与 `procinfo::Query` 的每个构造点与用例（`udp_falls_back_to_the_unspecified_address`、`looked_up` 的固定用例、`procinfo/mod.rs` 第 421 行的比较）；`Engine::look_up_owner` 唯一的那种查询形状（`engine.rs` 第 430 ～ 456 行）与 SOCKS5 UDP 关联的模板（`socks5.rs` 第 264、276 ～ 279 行）；两个监听器在哪里接受连接、`ctx.local` 在哪里取（`http.rs` 第 64、72 ～ 79、159 行，`socks5.rs` 第 44、51 ～ 55、205 行），每条连接的 `local_addr()` 放进 `SessionInfo.local`；按监听器分的流量统计今天是定长的 `[_; 2]`（`by_listener()`）。
- V6：`UdpClient` 的每个实现（`rurge-inbound` 的 SOCKS5、引擎用例里的假实现）；`Routes::flow_for` 与它的调用者；引擎在哪里把一条流结束成 `Rejected`（`run_flow` 的 `Err(outcome)` 与 `drain`）、`block-quic` 阻断的流走不走那里。
- V7：`rurge-dns`：查询怎样发给主上游（`fanout`、上游 trait 收的是报文还是名字与类型）、`[Host]` 的 `server:` 怎样选上游、`message.rs` 已有的辅助函数与 hickory-proto 的 `Message` / `Edns`；`lru` 在 `rurge-dns` 里的用法。
- V8：`rurge-rules`：`pre_match` / `pre_matches` / `PreMatchingSet`、编译后的规则怎样带上"含端口条件"的标记、集合里逐条求值的行在哪里求值、`EvalCtx::with_network` 的构造。
- V9：引擎的 `Escalation`（`record` / `should_drop`）、没有拨号的会话怎样进请求记录（预匹配的记录）、`notes` 的写法（`add_note`）。
- V10：引擎的数据目录（`StackOptions.data_dir`）、`StateStore` 的临时文件加改名的写法；引擎停下的路径（池写最后一次的地方）。
- V11：tokio 1.53.1 的 `copy_bidirectional_with_sizes` 与 `io::duplex`；当前线程运行时在单独线程上起的写法。
- V12：会话的日志行今天在 INFO 里带不带目的主机（第 14 节"名字只在 DEBUG"要与它一致；不一致时以今天的会话日志为准，并在计划里写明）。
- V13：WireGuard 栈与它的测试（`stack.rs` 推进到没有出口的循环、`FakeWgPeer`）可以照搬的部分；应用端的 smoltcp 在 `Medium::Ip` 下对任意目的地发起连接要的地址与路由。

## 18. 对总设计的订正，与实施期对本文的订正

1. 第 1.4 节"M3b（UDP 会话表、ICMP、引擎接入）→ M3c（应答器与 fake-IP、预匹配）" → 池（`rurge-dns::fakeip`）移到 M3b（第 2 节，M3-D1）。
2. 第 4 节"`rurge-tun` 与 `rurge-engine` 互不依赖" → 引擎依赖 `rurge-tun`，照它今天自己绑监听器；`rurge-tun` 不依赖引擎（第 3.2 节，M3-D2）。依赖方向的那一句随之改。
3. 第 4 节 `rurge-platform` 一行的"`icmp`（ICMP 套接字，M3）" → M4；M3 只有 `rurge-tun` 里的 `Pinger` trait 与测试的假实现（第 8 节，M3-D16）。
4. 第 8.2 节的 `SessionInfo` → 多一个 `local`（第 9.2 节）；"失败（拒绝或拨不通）回 RST" → `REJECT-DROP` 拿着连接 30 秒再 FIN（再读掉应用还在发的字节，第 18 节第 14 条），其余 RST（第 5.6 节）。
5. 第 8.3 节"会话数的上限与引擎现有的 4096 个关联的关系在 M3 定" → 计入那 4096 个（第 7.1 节）；回包来源的做法见第 7.2 节。
6. 第 8.5 节"`fd00:6152::/64` 里的 fake 地址（具体的一段在 M3 定）" → 低 32 位是同编号的 v4 地址（第 4.2 节）；"文件的格式、写的时机与大小上限在 M3 定" → 第 10.3 节；`REJECT-NO-DROP` 的 AAAA 答空（第 11.2 节）。
7. 第 8.6 节"端口可以没有" → DNS 层跳过含端口条件的规则（第 12.1 节）。
8. 第 12 节"丢弃的包按原因计数……形状在 M3 定" → 第 14 节。
9. 第 13 节"虚拟网卡同样有一个内存后端供起守护进程的用例用（M3 / M4）" → M4（M3 的用例都在引擎与 `rurge-tun` 这一层）。
10. 第 2.3 节 Q-07"回 RST 并记一行日志" → 每次一行 DEBUG、第一次另有一行 WARN（第 10.4 节）。

下面五条订正的是本文自己的第 4 ～ 6 节与已决事项，是 M3a 实施时量到、读到的；与第 1 ～ 10 条一样是代项目所有者作的裁定，所有者可以改，M3a 的代码以它们为准。

11. 第 4.1、5.1、5.4 节与 M3-D3、D6、D9 的候选，与基准门槛的结论（第 6 节）。M3a 量完了：数字、读法和量的条件在 M3a 计划执行期修正记录第 8 ～ 11、17、21、30 行，都是一台 Windows 11 x86-64 的开发机上用 release 构建量的（Linux 与 macOS 没有量过）。**读法**：B1 看远端在内存里的行（`TUN_BENCH_FAR=memory`：TUN 加两块 8 KiB 的转发循环，远端是内存里的管道），每个方向不低于 0.7 Gbit/s 或同设置下夹具上限的 80%，四个计数为 0；远端是回环 TCP 的行（第 6 节写的样子，基准的缺省）留在表里作那一跳的上限、不下判语——引擎的转发循环接一段回环 TCP，不经 TUN 就只有 0.7 ～ 0.9 Gbit/s，低的是那一跳，不是 TUN。B2 看 rurge 自己的内存，即进程跑完时的峰值减去测试一侧自己的内存（应用端的套接字缓冲量得准，内存管道与远端的缓冲取上界，`ScriptedTcp` 的表是估计），对着 429,000,000 字节，五个计数为 0；"只减去准的应用端套接字缓冲"的保守读法一起打印，它是一次测量的结果、不是保证。这比第 6 节与计划期决定 P9 的字面（按进程的峰值）松，按字面缺省设置的峰值 460 MB 不过。**数字**：B1 远端在内存里的 80 行（8 种 × 10 次）全部过线、最低 4.53 Gbit/s，中位数 MTU 1500 上行 7.36 / 7.13、下行 5.67 / 5.56，MTU 9000 上行 10.47 / 11.94、下行 6.78 / 6.56（`tx_max` 256 KiB / 1 MiB）；换成下面的缺省之后 24 行全部过线、最低 5.25；加了守卫与片的重建之后（各 40 次）MTU 1500、`tx_max` 256 KiB 的中位数上行 7.175、下行 5.460，八个设置的 320 行里最低 3.959。B2（缺省设置）：一万条连接开好之后 423.8 MB，跑完时 460.1 ～ 461.1 MB，减去测试一侧约 386 MB，保守读法 427.3 ～ 428.4 MB；加了守卫之后 11 次：门禁读数 386.3 ～ 387.9 MB（余量约 41 MB），保守读数 428.0 ～ 429.6 MB（5 次超过线、都不到 0.6 MB）；全部做完之后复跑一次：门禁读数 390.4 MB（余量 38.6 MB），保守读数 432.1 MB（超过线 3.1 MB，原因没有查）。峰值里是什么（片 64、同一设置、一万条开好之后 425 MB）：进程本身 15 MB，TUN 的套接字缓冲 147 MB，转发循环每条会话两块 8 KiB 共 164 MB，测试一侧约 45 MB，其余约 55 MB（每条连接约 5.5 KiB）。**结论**：不转向自写的 TCP 被动一端（M3-D9）——按字面的两处不足都在 smoltcp 与 `rurge-tun` 之外（回环那一跳与转发循环的缓冲；B2 这一句只在减去测试一侧之后成立，峰值里 TUN 自己的那一份——套接字缓冲 147 MB、结构与任务约 55 MB——是 `rurge-tun` 的，M3a 计划执行期修正记录第 9 行）。**定下的缺省值**（`TunOptions::default()`）：MTU 1500（两个候选都过，取 1500，M4 的设备取它）；`shards` 256（候选 16 / 64 / 256，原写缺省 64；B2 里每秒包数最多，4 KiB / 64 MiB 下 52,932，片 64 是 36,428、片 16 是 30,479，包率里有回环那一跳，只作相对值；16384 个套接字时平均每片 64 个，`max_sockets` 是整个 TUN 的一个上限、套接字按散列落到片上；跑完时的峰值比 64 片多约 4 MB，一万条开好之后不受影响）；`buffer_min` 4 KiB（4 KiB / 64 MiB 是 B2 里唯一在线以内的组合）；`tx_max` 256 KiB（两个候选在 B1 里没有差别，取较小的）；预算 64 MiB；D = 32 不变；最坏的总量 64 MiB + 16384 × 2 × 4 KiB = 192 MiB。**代价**：256 个片的任务、锁与接口；套接字的缓冲在建立时按剩下的预算定、以后不变，64 MiB 的预算被最先建的约 250 个套接字用完，此后建的套接字一辈子只有 4 KiB——同片数同预算的六组对比里 8 KiB 的活跃期比 4 KiB 快 1.6 ～ 2.6 倍，第 5.4 节"4 KiB 的窗口也有约 300 Mbit/s"没有在负载下验证过；这是内存线的代价（8 KiB 要多约 80 MB），留给阶段 3 / M6 量一次预算用完之后建的一条高吞吐连接（M3a 计划延后事项第 6 行）。**转发循环的缓冲**（引擎的 `BUF`，每条会话两块 8 KiB，一万条 164 MB）是 B2 里最大的一项，同时管内存与吞吐：按这里的读法 B2 里 rurge 自己约 386 MB，NFR-01 的线是 300 MB（429 MB 是它除以 0.7），转发缓冲 4 KiB 约 303 MB、1 KiB 约 240 MB，不改转发循环验不到 NFR-01 本身；自适应的缓冲（阶段 2 / M8c 计划延后事项第 21 行）M3a 不做（M3a 不碰引擎），移交阶段 3 / M6（M3a 计划延后事项第 5 行）。

12. 第 5.4 节"套接字的设置"与 M3-D6 的"不开保活、超时 120 秒" → 开保活（`TunOptions::keep_alive`，缺省 30 秒），超时 600 秒（`TunOptions::timeout`）。原写法以为超时只管"发出的数据 120 秒没有确认"；smoltcp 0.12 的超时不看有没有在途的数据：应用最后一个段之后 `timeout` 里什么也没来就断开（`socket/tcp.rs` 第 2118 ～ 2123、2274 行；发送缓冲从空变非空时才从发出的第一段重新起算，第 1170 ～ 1175、2258 ～ 2267 行），一个片一级的探针量到握手之后什么也不发的连接在 120 秒时被 RST。照原写法，经 TUN 的空闲长连接（SSH、数据库连接池、空闲的 HTTP/2）两分钟就断。保活每空闲 `keep_alive` 发一个保活段（它在保活计时器到期而没有别的可发时做出来：序号小一、一个字节，`socket/tcp.rs` 第 2449 ～ 2459 行的 `should_keep_alive`；发出任何一段之后计时器重新起算，第 2496 ～ 2498 行），活着的应用（它的内核）总会答 ACK，空闲的连接就不再被断开，应用那一侧没了的在 `timeout` 之后断开；保活段的回答带着应用当前的窗口，smoltcp 作发送方在对方窗口为 0 时不发零窗口探测段，应用的窗口更新丢了时下行至多停 `keep_alive`。600 秒与转发循环的空闲超时缺省（`--idle-timeout`）相同：Windows 的单调时钟把睡眠算进去，机器睡过 `timeout` 之后醒来时这一条断开每条连接，取 600 秒时它不比转发循环断得多。代价：应用那一侧没了的连接占一个套接字最多 10 分钟；每条空闲连接每 30 秒一个保活段；`--idle-timeout` 设得比 600 秒大时，TUN 仍在应用沉默 600 秒之后断开。（M3a 计划执行期修正记录第 3 行）

13. 第 5.3 节"窗口边缘的守卫"与 M3-D5：守卫只在这条连接的接收缓冲里还有任何空位时丢越过边缘的段（计 `beyond_window`），不论段多长；缓冲满了，越过边缘的段照常交给协议栈。原因：smoltcp 0.12 的窗口是物理的（`socket/tcp.rs` 第 1598 ～ 1599 行：窗口的末端是缓冲的容量，不是告知的边缘），缓冲里有空位时它收下段里放得进窗口的那一部分（第 1660 ～ 1672 行），所以比空位长的段照样把收下的数据的末端带过边缘，H2 的下溢（第 693 ～ 701 行）要边缘之外有空位才会发生；缓冲满时带数据的段它一个字节也不收，以 ACK 回答（第 1636 ～ 1641 行；`challenge_ack_reply`，每个套接字每秒至多一个，第 1433 ～ 1446 行）。零窗口探测（Windows 与 BSD 的发送方在接收方的确认号上发一个新字节）总碰上满的缓冲：悄悄丢掉它，发送方就得不到 RFC 9293 3.8.6.1 让它继续探测的确认，所以交给协议栈去答。代价与风险：越过边缘的每个段多一次套接字查找（少见）；若 smoltcp 在缓冲满时其实收下了一点什么，H2 的 panic 就会发生，被最后一道接住、这一片整片重建；空位之内的丢弃由片一级的用例 `a_segment_past_the_edge_into_room_is_dropped` 钉住。与 H2 有关的另三件实施期的事实：每次拿锁做事先推进一次发送（计划期决定 P21：读走数据之后的窗口更新排在新收的包之前）；`lib.rs` 在编译期断言 `RX_MAX <= 65_535`，用例 `the_tun_never_scales_its_window` 钉住 SYN-ACK 的缩放位数为 0（守卫的边缘只在缩放位数为 0 时只进不退）；拿掉连接时边缘与 `by_tuple` 项在那一项还指着这条连接时一起删。真实的 Windows、macOS 与 Linux 发送方在长停顿里怎样探测，没有观察过（M3a 计划延后事项第 11 行）。（M3a 计划执行期修正记录第 18、19 行）

14. 第 5.6 节"握手之后"表中 `REJECT-DROP` 一行与 M3-D10 → 不读不写地拿着连接 `drop_hold`（30 秒；停止立刻结束它），然后 FIN（`shutdown`），然后读掉丢弃应用还在发的字节，直到应用的 FIN 或出错，至多 `linger`（60 秒），然后放手（应用的 FIN 到了则好好结束，没到则放手即 RST）。原因：丢掉 `TunStream` 会复位一条应用还可能再发的连接（流被丢掉的规矩，M3a 计划执行期修正记录第 5 行），`shutdown` 之后马上丢流会把 FIN 变成 RST，应用读到的是 `ConnectionReset`，不是 `Ok(0)`。代价：拿完之后应用发的字节被确认再丢掉，而系统自己的套接字在这时关掉会对它们回 RST；这样的连接要占一个套接字与它的两块缓冲 `drop_hold` 再加至多 `linger`，对 `max_sockets` 的压力比立刻 RST 大。`REJECT`、`REJECT-NO-DROP`、`REJECT-TINYGIF` 与拨不通照旧 RST。（M3a 计划执行期修正记录第 25 行）

15. 第 4.5 节与第 5.2 节第 1 步的"停止"：停止时每一片走最后一轮——排着的包全部进栈（不受每次唤醒的包数限制；其中的新 SYN 因 TUN 在停止回 RST）、每个套接字 `abort()`、推进一次发送（每个套接字发出它欠应用的那一个 RST），这一轮发出的包交给写任务，然后 `end_all()`；与每一轮一样包在 `catch_unwind` 里；写任务已经结束时这一轮省去。对写任务的发送从不被停止取消，写任务结束了才失败：一轮在停止来的时候已经在做，它做出的 RST 也是停止的 RST。写任务在停止之后继续写片交来的东西，直到队列关上（所有的片都结束），但从它看到停止起只给设备 `STOP_GRACE`（1 秒，`src/writer.rs`）：内存设备会等到收得下，一个什么也不收的设备会让写任务卡在写里、每个等队列空位的片和 `join` 永远等下去；写出错之后什么也不再写。`Ending` 只有两种：`Stopped`（`stop()`）与 `DeviceFailed(<错误的文字>)`，第一个结局不变；`join` 等全部任务，会话的也在内，每次答同一个结局；设备出错说且只说一行 WARN：`tun: the device failed; the TUN has ended`，字段 `error`（设备错误的文字，不带地址）；TUN 已在停止时设备才出错就不说，结局仍是 `Stopped`。停止不取消正在进行的拨号（M3a 计划期决定 P15）：拨号回来时 `TunStream` 已被复位，转发当场结束，`join` 等它；所以引擎停下时等 `join`，`Engine::dial` 的每一条路都要有自己的时限（M3b）。**停止也不取消正在转发的会话**：会话只在转发循环去碰 `TunStream` 的时候才发现连接被复位，两个方向都在等上游的转发（上行卡在写一个不收的上游，下行卡在读一个不出声的上游）一次也不碰，要等到上游动一下、转发循环自己的空闲超时（引擎的 `--idle-timeout`，缺省 600 秒）到点，或有人取消会话的令牌，`join` 同样等着它（整分支终审之后用一条临时用例在假 `Dialer` 的转发上观察到，没有提交）；所以 M3b 停 TUN 时要取消它的会话——`Engine::cancel_sessions`，或每个 TUN 在 `sessions_root` 下有自己的子令牌、由 `TunHandle::stop` 触发——，或给等 `join` 的那一步一个时限，并加一条上游既不读也不写时 `join` 在界限内结束的端到端用例；不要在 TUN 里把转发的 future 丢掉来结束它，`relay` 要结束 `SessionHandle`，丢掉它请求记录会一直开着（M3a 计划延后事项第 12 行）。代价：设备卡死时停止最长多等一个 `STOP_GRACE`；比这慢的设备收不到最后的复位（应用自己的协议栈会让那些连接超时）。真实设备上这一秒够不够、最后一轮的复位会不会因为设备满了被丢，要到 M4 才知道（M3a 计划延后事项第 12、13 行）。（M3a 计划执行期修正记录第 26 ～ 28 行；M3b 做掉了取消会话的一半，拨号那一半留给 M4，第 22 条）

下面七条订正的是本文自己的第 3、5、7、9、10 节，是 M3b 实施时读到、做到的（计划 `docs/superpowers/plans/2026-10-22-phase3-m3b-udp-icmp-pool-engine-plan.md`，下面写"M3b 记录第 N 行""M3b 延后第 N 行"指它末尾的「执行期修正记录」与「延后事项」两张表，写"P N"指它的「计划期决定」）；与第 1 ～ 15 条一样是代项目所有者作的裁定，所有者可以改，M3b 的代码以它们为准。

16. 第 3.3 节 `TunSettings`：设计写这一代的解析器（`Arc<Resolver>`）、`always-real-ip`、`allow-dns-svcb`、`hijack-dns`、`icmp-forwarding`、出站模式是不是规则模式 → M3b 里 `TunSettings` 只有 `resolver: Arc<dyn Resolve>`（`rurge_net::connector::Resolve`，`rurge_dns::Resolver` 实现它，所以 `rurge-tun` 的用例用假解析器、不必建真的）与 `icmp_forwarding`（`crates/rurge-tun/src/hooks.rs`）；`always-real-ip`、`allow-dns-svcb`、`hijack-dns`、规则模式与 `Hooks::pre_match` 随 M3c（应答器与预匹配才用到，M3b 延后第 5 行）。`Tun::start` 的参数照第 3.3 节（`device`、`dialer`、`hooks`、`pinger`、`pool`、`options`）；引擎一侧的 `Hooks` 实现读 `Runtime.tun`，每一代的 `Runtime::build` 重建它，所以一次重载在 TUN 跑着的时候被它的下一次询问看到。（P2）

17. 第 10.3 节"读不了、读不懂……从空表开始，一次 WARN，文件留着不动，下一次写时覆盖"：一、打开时就写：`clean 1` 的文件载入后立刻改写成 `clean 0`，没有文件时立刻写一份空表的 `clean 0`——一次在第一次保存之前崩溃的运行，下次也认得出它是不干净的（M3b 计划的评审要点第 4 条）；读不了的文件仍留着不动，第一次保存时覆盖。二、读不了、读不懂的文件，从空表开始时同样隔离从 0 起的 2048 个编号 120 秒：那份文件里的映射可能还在应用手里（设计没有写这一处）。三、"接下来会交出的 2048 个"照分配的次序数：隔离期已过的编号（先进先出）→ 游标 → 最久没用到的（LRU），所以隔离不会漏掉下一次会交出的编号。`FakeIpPool::open` 返回 `Loaded`（`Missing`、`Clean`、`Unclean`、`NotRead(原因)`）而自己不写日志，日志由 `Engine::start_tun` 在调用它的线程上写：`NotRead` 一行 WARN（原因是池自己的固定文字加行号、或系统的错误文本，不引用文件的任何内容）、`Unclean` 一行 INFO，别的不说；一次保存失败的 WARN 由 `keep_saved` 写（不带名字）。读文件严格：第一行、第二行的三对、编号在范围内、名字不重复且是规范形式、行以 LF 结尾、末尾至多一个空行。（P4、P6、P24；M3b 记录第 4 行）

18. 第 10.3 节"上一次没有干净地停下"，文件里没有名字的编号：文件的格式分不出"被隔离的"和"从没用过的"，一个被上一次运行隔离着、没来得及交出就结束了的编号既没有名字、又不在游标之后、也不在使用次序里，不处理就永远丢了。所以打开时每个没有名字的编号（低于游标的，回绕之后任何位置的）再隔离 120 秒，排在其余之前，**并且**另隔离接下来的 2048 个——叠加，不抵扣：一次在不干净的打开之后 120 秒内崩溃的运行是从游标交出它没来得及存的编号的，抵扣就漏了它们（评审时指出，`0efac97` 改）。代价：连续崩溃时每次 120 秒内的重启多隔离 2048 个，大约重启 64 次之后所有编号都在隔离里，`assign` 在每次运行的头 120 秒答 `Exhausted`，直到某一次运行活过 120 秒；没有编号永远丢：最后一次打开之后 120 秒全部放出。（M3b 记录第 3 行）

19. 第 7.2 节与第 10.2 节的目标与租约：设计写 UDP 会话为它的每个目标在会话的一生里各拿一个租约 → 每个会话一张目标表（端点与目标互查，两个方向 O(1)），至多 `max_udp_targets`（1024，与引擎每个关联的流数相同）；一个目标两个方向都过了 `udp_idle` 没有包就拿掉、放掉租约（扫描每 `udp_idle / 4` 一次）。原因：引擎的流同样在空闲 60 秒后回收，一个长寿的会话若一直攒着目标，会在 1024 处卡住、租约也一直占着。会话本身也是两个方向都 `udp_idle` 没有包才结束，只有下行的会话（服务器推送、应用只收不发）不结束；这个应用在表里的会话正在结束时，新来的包建一个新会话替换它；结束的会话只在表里那一项仍是自己（`Arc::ptr_eq`）时拿掉。（P11、P20、P21）

20. 第 9.2 节 `looked_up`：设计写"`looked_up` 对 `Tun` 改为真" → `looked_up` 去掉了，`query_for(session, since) -> Option<OwnerQuery>`（`crates/rurge-engine/src/owner.rs`）取代它：查不查与怎样查是同一个决定。HTTP 与 SOCKS5 的会话（SOCKS5 的 UDP 关联的模板也是，查它的控制连接）与虚拟网卡的 TCP 会话发 TCP 的查询（`local` 是 `src`，`remote` 是会话的 `local`；监听器的会话没有 `local` 时退回 `0.0.0.0:<in_port>`），虚拟网卡的 UDP 会话发 UDP 的查询、没有 `remote`，`Forward` 与 `Internal` 不查；`Engine::look_up_owner` 里依次是：没有 `owners`、`query_for` 为 `None`、`is_local` 答"不是本机"，各得到"不知道"的槽。DEBUG 查找行的字段 `listen_port` 换成 `remote`（虚拟网卡的会话的远端不是监听端口；`None` 时省略）。没有 `local` 的虚拟网卡 TCP 会话（够不到）以 `remote: None` 去问，只凭应用自己的端点找。（P23；M3b 记录第 2 行）

21. 第 5.5 节"拿掉套接字：smoltcp 报 CLOSED 或 TIME-WAIT 时"：套接字到 TIME-WAIT（rurge 先关了自己的一侧）或 `Closed` 时，流还在而 `recv_queue` 里有没读的字节，则片把它们拷进连接与流共用的记录（`ConnShared` 的 `Leftover`，`crates/rurge-tun/src/tcp/mod.rs`），接收缓冲的预算份额留到读完或流被丢掉再还，套接字、四元组的表项和 `max_sockets` 的计数立刻放掉；读先从记录里答，答完是结局。结局取 `recv_slice(&mut [])` 的回答：收到过 FIN 的是 `Closed`，读完字节之后是 `Ok(0)`；没有的是 `Reset`，读完字节之后仍是复位。所以在应用的 FIN 之前超时的连接照旧以复位结束，不会变成干净的 EOF；FIN 之后才超时的，读完字节是 `Ok(0)`。两种情形一并这样办：TIME-WAIT 过了宽限（不等 smoltcp 自己 10 秒之后的 `reset()` 清空缓冲）、`Closed` 而没有远端端点（应用先关、rurge 后关，或 smoltcp 自己的超时）。原因：应用的 TCP 看到这些字节都被确认了，被 `reset()` 清掉是它看不见的丢数据，转发循环看到的只是一个复位。**不只修结局**——只把结局改成 `Closed` 会把看得见的丢失变成悄悄的截断。代价：留着字节的连接占着至多 65,535 字节的预算份额，直到读完或流被丢掉。（P26；M3b 记录第 6 行）

22. 第 3.3 节"引擎停下时停 TUN（取消它的会话，或限时地等 `join`）"与第 15 条的了结：做到了"取消它的会话"。`TunHandle::stop` 取消每个已拨通的会话的令牌并等它的转发结束，停止与转发赛跑时停止居先（`session::serve`，`biased`）；`REJECT-DROP` 的拿着与连接的结束赛跑（`TunStream::closed`），读掉丢弃改用 512 字节的数组，不再用 `tokio::io::copy` 的 8 KiB 堆缓冲；`Engine::start_tun` 在引擎停止接受时 `stop()` 这个 TUN；第 15 条要的端到端用例是 `stopping_the_tun_ends_a_relay_stuck_on_its_upstream`（`crates/rurge-engine/tests/tun.rs`）。**没有做、也不会在 `rurge-tun` 里做的**：取消正在进行的拨号——拨号进行中会话的令牌还在引擎手里（`Engine::dial` 里才建 `SessionHandle`），TUN 够不到，丢掉拨号的 future 会留下开着的请求记录。改由引擎的拨号自己的时限界住：`Engine::dial` 每一条会等待的路都读过，没有一条没有时限（M3b 记录第 15 行），但其中一个由 Profile 定（`evaluate-before-use` 的等待，`test-timeout` 可以写任意多秒），所以停止之后 `join` 最长等多久不是一个常数；可执行文件停 TUN 时仍给等 `join` 的那一步一个自己的时限（M3b 延后第 8 行，M4）。（P29、P30）

## 19. 已决事项

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| M3-D1 | 计划的拆分 | M3a 设备、包的去向与 TCP（第一个任务是基准门槛）→ M3b UDP、ICMP、池与引擎接入 → M3c 应答器与预匹配；池随 M3b，好让端到端用例经 fake 地址连到钉在回环上的名字 |
| M3-D2 | crate 与依赖 | 新 crate `rurge-tun`，平台无关、`forbid(unsafe_code)`；引擎依赖它（`Engine::start_tun`），它不依赖引擎与 `rurge-platform`；可执行文件在 M3 不变；没有新的外部包 |
| M3-D3 | TCP 协议栈与设备 | smoltcp 0.12 按四元组分片（候选 16 / 64 / 256，缺省 64，基准定；M3a 定为 256，第 18 节第 11 条），每片一把锁、一个任务，收进来的包逐个推进、发送反复推进到没有出口、每次唤醒做有界的活；只把 TCP 交给 smoltcp；时钟以 TUN 启动时为零点；设备 trait 批量收发，包是 `Vec<u8>`；MTU 候选 1500 / 9000，都过门槛时取 1500 |
| M3-D4 | SYN | 按目的端点建监听套接字，建好之后紧接着推进它的 SYN（包逐个推进，中间没有别的包）；推进之后不在 SYN-RECEIVED 就拿掉；次序是停止 → 上限 → 去处 → 预匹配 → 建；rurge 自己发的 RST 经 3 秒 100 个的限频；协议栈的地址 `198.18.0.253` / `fd00:6152::fd` 与经它们的缺省路由 |
| M3-D5 | smoltcp 0.12 的隐患 | 进协议栈之前过滤坏包、分片、`martian`、扩展首部（H1、H4 ～ H6）；SYN 的 SACK-permitted 换成 NOP（H3）；窗口边缘的守卫（H2；丢的条件见第 18 节第 13 条）；推进包在 `catch_unwind` 里、panic 了重建这一片（最后一道）；TCP 只算发出的校验和，IPv4 首部、UDP、ICMP 的校验和由 rurge 验 |
| M3-D6 | 缓冲与套接字的设置 | 建立时按预算分配：接收 clamp(剩余 ÷ 32, MIN, 65535)（缩放位数为 0，H2 在正常传输里不发生；写计划之前的评审指出更大的接收缓冲会让守卫丢掉合规的在途数据），发送 clamp(剩余 ÷ 32, MIN, MAX)；候选 MIN 4 / 8 KiB、发送的 MAX 256 KiB / 1 MiB、预算 64 / 128 MiB，基准定；关 Nagle、不开保活、超时 120 秒、Reno（保活 30 秒、超时 600 秒，第 18 节第 12 条；候选定为 MIN 4 KiB、MAX 256 KiB、预算 64 MiB，第 11 条） |
| M3-D7 | 写回与背压 | 一个写任务；每片推进时攒在自己的 `Vec` 里（`TxToken` 从不拒绝），推进完交给全局有界队列，满了这一片等；进各片、各会话、应答器的队列有界，满了丢弃计数；设备出错时 TUN 结束（停止时的最后一轮与写任务的宽限见第 18 节第 15 条） |
| M3-D8 | 上限 | TCP 套接字 16384；半开 2048、10 秒；UDP 会话计入引擎的 4096 个关联、每会话 1024 条流与 256 个排队的包；ICMP 在途 1024、5 秒；DNS 转发在途 256；DNS over TCP 空闲 10 秒 |
| M3-D9 | 基准门槛 | B1 单流每方向不低于 0.7 Gbit/s（rurge 一侧单线程，MTU 1500 与 9000；那一程里 `beyond_window`、`queue_full`、`rst_limited`、`stack_panics` 都是 0 才算数，另量夹具自己的上限）；B2 1 万条用过的空闲连接加 100 条活跃的，峰值常驻内存不超过 429 MB，每包的时间只报告；忽略的用例；调参之后仍达不到就换自写的 TCP 被动一端（总设计已授权）；M3a 量完了，怎样读、数字与结论见第 18 节第 11 条，没有转向 |
| M3-D10 | 握手之后 | 先完成握手再拨号；拨通则 `relay`；`REJECT-DROP` 拿着 30 秒再 FIN（再读掉应用还在发的字节，第 18 节第 14 条）；其余拒绝与拨不通回 RST；不嗅探 |
| M3-D11 | fake 地址在哪里换回名字 | 在 `rurge-tun`（它持有池）：TCP 的 `dst_host` 是名字、连接持租约；UDP 每会话一张目标表、目标持租约；池里没有的 rurge 地址：TCP 回 RST、UDP 与 ICMP 丢弃，计数，每次 DEBUG、第一次 WARN；从不拨 rurge 地址空间 |
| M3-D12 | `SessionInfo.local` 与进程查找 | 新字段 `local`：监听器的会话是那一条被接受的连接的 `local_addr()`（归一；不是绑定的地址），SOCKS5 的 UDP 关联带控制连接的，虚拟网卡的 TCP 会话是原目的端点；进程查询的 `remote_port` 换成 `remote: Option<SocketAddr>`（地址与端口都比，未指定地址只比端口），查询的形状按监听器与传输定（虚拟网卡的 UDP 会话按本地端点发 UDP 的查询）；`Tun` 的会话查进程；Linux 的 `Layout` 与 macOS 双栈两行随 M3b |
| M3-D13 | UDP 回包的来源 | `UdpClient::send` 加 `answers`（目标恰好相同、或同端口的域名流只有一条时给出）；有它写回原目的端点，没有时 IP 来源照用、域名来源经池正查，查不到丢弃 |
| M3-D14 | UDP 的拒绝 | `UdpClient::refused(target, kind)`，有缺省的空实现；`REJECT` / `REJECT-TINYGIF` 回 ICMP 管理上禁止、全局 3 秒 100 个；`REJECT-NO-DROP` 不限频；`REJECT-DROP` 不回；来源是原目的地址，引用最近一个包的首部 |
| M3-D15 | 分片 | 进来的分片丢弃并计数；大于 MTU 的 UDP 回包按 IPv4 / IPv6 分片写回 |
| M3-D16 | ICMP | 回显不经规则；rurge 自己的地址本地应答、fake 地址先解析再转发、其余经 `Pinger`；保留应用的标识与序号；`Pinger` 是 trait，真实的 ICMP 套接字在 M4 |
| M3-D17 | fake 地址的编址 | v4 `198.18.1.1 + i`（130,814 个），v6 `fd00:6152::` 加同编号的 v4 地址；rurge 自己的地址 `198.18.0.0/24` 与 `fd00:6152::/120`；整个地址空间 `198.18.0.0/15` 与 `fd00:6152::/64` |
| M3-D18 | 池 | 名字规范化、只收 `[a-z0-9_-]` 的标签与 253 字节以内；先游标后 LRU；租约挡住回收；耗尽时 SERVFAIL；TTL 30 秒；`fake-ip.txt` 文本、至多每 10 秒与停下时写、不 fsync、读不了从空表开始；上一次没有干净停下时隔离接下来的 2048 个编号 120 秒（打开时的写、读不了的文件与文件里没有名字的编号，第 18 节第 17、18 条） |
| M3-D19 | 应答器 | 第 11.2 节的次序；OPT 1232；超出时置 TC；同时转发至多 256 个；`hijack-dns` 只对 UDP 与 IPv4、只认合法的查询、应答的来源是被劫持的服务器 |
| M3-D20 | 预匹配的入口 | 一个入口给出会话与层；DNS 层跳过含 `DEST-PORT` / `SRC-PORT` 的规则、集合里含端口的行不命中；IP 类规则在 DNS 层不命中；从不解析 |
| M3-D21 | 经监听器来的 fake 地址 | 有池时在规则之前换回名字（持租约），池里没有就失败、不拨；没有池时照普通地址 |
| M3-D22 | 计数与 API | 按原因的丢包计数与 `stack_panics`；M4 起在 `GET /v1/features/enhanced_mode` 里给出，形状见第 14 节 |
| M3-D23 | 预匹配的钩子与记录 | 规则模式才看；`REJECT` 与 `REJECT-TINYGIF` 计入 `Escalation`；同一代里同一条规则 5 分钟只记一次，`notes` 写层；清单 L336 的阶段改为 3 |
| M3-D24 | 拒绝在虚拟网卡上的样子 | 第 9.4 节的表；`198.18.0.244` 的 TCP 回 RST、UDP 回 ICMP（不限频） |
| M3-D25 | 测试 | 应用端是另一个 smoltcp 栈；经虚拟网卡不以回环为目的：名字经 fake 地址钉到回环，真实 IP 用文档地址并有两道防线（要测的规则、`SRC-IP` 的 `REJECT`）；ICMP 用假 `Pinger`；M3 的用例不建网卡、不改系统 |
| M3-D26 | 名字与地址的日志 | 名字与地址只在 DEBUG 与请求记录里；每种丢包原因第一次 WARN（`martian` 不报），其余 DEBUG |
