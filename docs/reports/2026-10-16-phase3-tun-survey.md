# 阶段 3 增强模式（TUN）调研报告

- 日期：2026-10-16（所有"当前版本""最近推送"都以这一天读到的为准）
- 范围：rurge 阶段 3（FR-IN-06 ～ 10、FR-DNS-08 / 09、`PROCESS-NAME`）所需的公开资料调研，共 8 个问题（下文第 1 ～ 8 节），第 9 节是需要项目所有者定的事、建议先做的小实验与本次没有核实的事，末尾第 10 节是候选 crate 一览。
- 方法：只读公开网页、GitHub 原始文件、docs.rs 页面与本机 cargo 注册表里已有的源码（`smoltcp-0.12.0`、`nix-0.31.3`、`socket2-0.6.5`、`libc-0.2.189`）；没有下载、安装、运行任何第三方代码，没有改动仓库里除本报告外的任何文件，也没有派子代理。GPL 项目（sing-box / sing-tun / mihomo / wireguard-tools 等）只记工作方式，没有抄代码。

## 来源可靠性说明（先读这一节）

1. 绝大部分页面是经 `WebFetch` 读的。这个工具返回的是一个小模型对页面的**摘要**，不是逐字原文。我在每次提问里都要求它逐字引用关键句，文中带引号的短句出自这些摘要；数字（版本号、偏移量、常数、基准数字）应当在设计定稿之前对着原文再核一遍。读源码文件时它常常只看得到文件的一部分，凡是它自己说"没有看到"的地方，我都写成"未见"而不是"没有"。
2. 下列站点这次无法直接读取：`crates.io`（域名校验被拦）、`developer.apple.com`、`docs.github.com`、`support.mozilla.org`（要 JS）、`lists.zx2c4.com`（个别页面）。涉及它们的结论来自搜索引擎对这些页面的摘要，文中标"（搜索摘要）"，可信度低于直接读到的原文。GitHub 的 REST API 中途被限速，个别元数据改从网页或 raw 文件取。
3. 凡是我自己的判断（不是来源里写的），文中明确标"（推断）"或放在"对 rurge 的建议"里。数字估算（例如"线性扫描在 1 万个套接字下每包要多少微秒"）一律是量级估算，没有实测。
4. 来源编号按节重新编号：`[3.4]` 指第 3 节来源表里的第 4 条。每节末尾有该节的来源表（URL + 版本或日期）。

## 0. 摘要（结论先行）

下面每条的依据都在对应的节里。

1. **协议栈没有银弹，而且本年内最大的变化是 sing-box 自己写了一个新栈。** sing-box 1.15（测试版，2026-10 时最新是 1.15.0-alpha.12，稳定版 1.14.3）把 `system` / `gvisor` / `mixed` 三选一换成 sing-tun 自带的 Go TCP/IP 栈（`stack` 选项 1.15 起弃用、1.17 移除）；mihomo 也有了自己的 `mips` 栈（MPL-2.0，纯 Go，缺省）。Rust 一侧：shadowsocks-rust 与 clash-rs 用 smoltcp（0.14），tun2proxy 用 ipstack，leaf 用 lwIP 或 smoltcp。唯一找到的横向基准是 hev-socks5-tunnel 自己发布的（自报、同一台机器、未经第三方复核）：lwIP 的 hev 单流约 60 Gbit/s、内存 2.6 MB，sing-box 1.14.3 约 24 Gbit/s、内存约 84 MB，Rust 的 smoltcp 栈不在其中。（第 1 节）
2. **smoltcp 的 TCP 套接字查找是线性扫描**（0.12 与 0.14 都是：入包时遍历 `SocketSet` 里全部套接字调 `accepts`，每次 `poll` 的出口也遍历全部）。这对 NFR-01"1 万并发连接"是硬约束：单个 `SocketSet` 放不下；要么按四元组哈希分片成多个 `Interface`，要么换栈。（第 1 节，本机源码核实）
3. **rurge 在阶段 2 为 smoltcp 0.12 记下的几条限制，0.13 / 0.14 的更新日志里有对应的修复**：零窗口探测（0.13.0）、出口状态在一次 `poll` 里吐尽（0.13.0）、RFC 6298 退避（0.13.0）、Reno / CUBIC 按 RFC 重写与快速重传改进（0.14.0）。代价：**0.13 起 MSRV 是 1.91，而项目 MSRV 是 1.89**；要不要升是项目所有者的事。SACK 在 0.14 的 README 里仍写"未实现"。（第 1 节）
4. **"system 栈"（sing-box 多年的缺省）**：TUN 上读到的 TCP 包改写源 / 目的地址后写回 TUN，让本机内核把它当作发给本机监听器的连接，接受之后用 NAT 表反查原始目的地址。内核负责窗口缩放、SACK、重传、拥塞控制，各家文档都说它吞吐 / CPU / 内存最省（没有找到基准数字）；UDP 与 ICMP 仍要用户态处理。代价：Windows 上要给可执行文件加入站防火墙规则（sing-tun 自己加；mihomo 文档写"防火墙开着时 system / mixed 不能用，除非放行内核程序"），并且它**不是"用户态协议栈"**，与 FR-IN-06 的字面不一致，是否改字面要项目所有者定。（第 1 节）
5. **TUN 设备**：`tun-rs` 2.8.11（Apache-2.0，2026-10-16 当天还有推送）是三平台能力最全的 crate（Linux 的 TSO / GSO、多队列，Windows 经 `libloading` 动态加载 `wintun.dll`）；`tun` 0.8.14 的许可是 WTFPL，不在项目允许的许可名单上。macOS 的 utun 用已在依赖树里的 `nix` 0.31.3 就能**不写 unsafe**（`SysControlAddr::from_name` + `sockopt::UtunIfname`，nix 自己的用例演示了这套调用）；Linux 只差一个 `TUNSETIFF` ioctl。（第 2 节）
6. **路由**：Linux 用专用路由表 + `ip rule`（`not fwmark` + `suppress_prefixlength 0`，或 sing-tun 的保留优先级窗口），用 netlink（`rtnetlink` 0.23.0，MIT）可以零 unsafe；macOS 默认路由改写有三种做法（`0/1`+`128/1`、sing-tun 的八个前缀、clash-rs 的带 `-ifscope` 的默认路由），Windows 用 IP Helper 的 `CreateIpForwardEntry2`，接口 metric 设 0 并关自动 metric。"排除路由"几乎都是用 CIDR 求补集实现的，不依赖物理网关。**崩溃之后留下什么**因平台而异：Linux 只剩 `ip rule`（sing-tun 启动时清掉自己保留窗口里的全部规则）；macOS 与 Windows 的路由随接口消失，但 DNS 设置（`networksetup`）不会。（第 3 节）
7. **防环**：Linux `SO_MARK`（需要 `CAP_NET_ADMIN`，Linux 5.17 起 `CAP_NET_RAW` 也行）或 `SO_BINDTODEVICE` / `SO_BINDTOIFINDEX`；macOS `IP_BOUND_IF` / `IPV6_BOUND_IF`（socket2 已有安全封装，rurge 已在用）；Windows `IP_UNICAST_IF` / `IPV6_UNICAST_IF`（IPv4 的接口号要网络字节序，IPv6 要主机字节序；socket2 没有封装，要一处 unsafe）。rurge 现在在 Windows 上靠"绑定物理网卡的源地址"（强主机模型），**TUN 成为默认路由之后这个假设变成承重的**，WireGuard for Windows 自己就警告"网卡开了 weak host send 会造成路由环"。要在真机上验证，不行就改 `IP_UNICAST_IF`。（第 3 节）
8. **DNS / fake-IP**：Surge 手册的行为已经够具体（198.18.0.2、30 秒 TTL、AAAA 回空、type 65 默认拒绝、`always-real-ip`、`hijack-dns` 只劫持能解析成 DNS 查询的包、canary 域 NXDOMAIN）。mihomo 的池是"顺序环 + 持久化偏移量，绕一圈后复用最老的地址"。各平台把系统 DNS 指向应答器的办法不同，**崩溃后自动失效的有两种**：Linux `resolvectl` 的逐接口设置（接口消失即丢）和 macOS `SCDynamicStoreAddTemporaryValue`（会话关闭即删）；其余（`networksetup`、`resolv.conf`、Windows 接口 DNS）要自己记日志、下次启动恢复。DoH 绕过：Chrome 的自动升级只对名单内的服务商生效、不支持 canary 域；Firefox 的 canary 域对"默认启用 DoH"的用户有效。（第 4 节）
9. **进程识别**：Windows `GetExtendedTcpTable` / `GetExtendedUdpTable`（带 PID）+ `QueryFullProcessImageNameW`；Linux netlink `sock_diag` 精确查询拿到 uid 与 inode，再扫 `/proc/<pid>/fd` 找进程（扫描是主要开销，sing-box 缓存 1 秒，mihomo 不缓存）；macOS `net.inet.tcp.pcblist_n` 的结构偏移**随 Darwin 版本变**（mihomo 按 `kern.osrelease` 在 384 与 408 字节之间选），libproc 路径稳定但慢。没有一个许可合适又不需要 `bindgen` / libclang 的现成 crate（`libproc`、`netstat2` 的构建都依赖 `bindgen`；`sock2proc` 没有许可；`listeners` 只管监听端口）。（第 5 节）
10. **网络环境**：事件源 Windows `NotifyIpInterfaceChange` / `NotifyRouteChange2`，Linux netlink 多播组，macOS `AF_ROUTE` 套接字或 SCDynamicStore；sing-tun 在 Linux 上做 1 秒的"首发 + 尾发"去抖。**SSID / BSSID 是三平台里最脆弱的一项**：Windows 的 Wi-Fi API 在用户没有授予精确位置权限时返回 `ERROR_ACCESS_DENIED`（微软文档），而且提示只会给"在用户上下文里、不在 `System32` 下"的进程；macOS 14 起读 SSID 要位置服务授权，命令行工具与 launchd 守护进程的行为不稳定（搜索摘要）。要把"SSID 不可用"设计成正常状态。（第 6 节）
11. **Wintun**：`wintun.dll` 的"Prebuilt Binaries License"允许"随使用它的软件一起分发，但只能通过 `wintun.h` 里的接口使用"，禁止修改与反向、提取；**没有明说能不能嵌进可执行文件**（sing-tun 嵌入并从内存加载，是它自己的选择）。MIT 应用把 DLL 放在可执行文件旁边分发没有许可冲突。建议旁置分发 + 校验签名 + 固定版本与哈希；适配器用固定名字与确定性 GUID，遇到已存在就 `WintunOpenAdapter`。（第 7 节）
12. **测试**：本次读到的公开项目里，CI 里真的创建 TUN 的只有两个——sing-tun（Linux / macOS 上整套测试 `-exec sudo`；有内存 TUN `MemoryTun`，也有真实 TUN + 真实内核 TCP 做对端的集成测试，用 `tc netem` 注入丢包）和 tun-rs（只在 Linux 上 `sudo` 跑 `privileged_*` 用例）；tun2proxy 的 `rust.yml` 从不执行 `cargo test`，它的 `tests.yml` 依赖机密里的真实代理与公网 IP 服务；ipstack 的 CI 不创建 TUN。没有找到哪个项目在 CI 里用网络命名空间测 TUN。smoltcp 自带 `Loopback`、`FaultInjector`、`PcapWriter`。GitHub 托管的 Linux / macOS 运行器有免密 `sudo`，Windows 运行器是管理员（搜索摘要）。（第 8 节）

### 建议一览（我的判断；展开在各节的"对 rurge 的建议"里）

1. **协议栈**（1.4）：先由项目所有者决定 FR-IN-06 的字面；技术上首选 smoltcp 并按四元组分片（或每连接一个 `Interface`），每连接缓冲压到 8 ～ 32 KiB，最好升到 0.14（MSRV 1.91）；`system` 栈（NAT 改写 + 内核 TCP）是退路。用一个小实验（9.2 的 E1、E2）定，不要凭本报告里的量级估算定。接口抽成 trait，TCP 流以 `BoxedStream`、UDP 以现有的 `UdpClient` 进入流水线。
2. **TUN 设备**（2.4）：`tun-rs` 藏在 `TunDevice` trait 后面（rurge 代码零 unsafe）；不用 WTFPL 的 `tun`；macOS 将来可以换成 nix 的安全封装，Linux 只差一个 ioctl。
3. **路由与恢复**（3.5）：Linux 用 `rtnetlink` + 专用表 + 自己的优先级窗口；macOS 用 `/1` 一对；Windows 用 `CreateIpForwardEntry2` + 接口度量 0；排除路由用 CIDR 求补集；**所有会留下残渣的改动先写事务日志，下次启动先回放**（照 `SystemProxyManager`）。
4. **防环**（3.3）：Linux `SO_MARK`、macOS `IP_BOUND_IF`（已有）、Windows 先验证现有"绑源地址"，不行就加 `IP_UNICAST_IF`（一个 unsafe 函数）。
5. **DNS / fake-IP**（4.5）：应答器做在 TUN 栈这一层；池 = 顺序环 + 持久化 + 回绕时跳过仍在用的地址；type 65 → NOTIMP，AAAA → 空，canary → NXDOMAIN；系统 DNS 优先选崩溃后自动失效的机制（`resolvectl`、SCDynamicStore 临时键）。
6. **进程识别**（5.4）：`ProcessResolver` 放在 `rurge-platform`，`spawn_blocking` + 按流缓存；Windows 用文档化的连接表，Linux 用 sock_diag（nix 安全套接字），macOS 用带版本校验的 `pcblist_n`；不引入需要 `bindgen` 的 crate。
7. **网络环境**（6.4）：一个 `NetworkWatcher`，统一 1 秒去抖；SSID 作为一等的"可能不可用"状态设计。
8. **Wintun**（7.4）：官方 DLL 旁置分发、固定版本与哈希、绝对路径加载并校验签名；固定适配器名与确定性 GUID；先不嵌入。
9. **测试**（8.4）：L1 内存设备（无特权，三平台，CI 主力）+ L2 Linux 命名空间（`sudo`，门控）+ L3 macOS / Windows 冒烟（门控）；特权用例是"只用回环"规矩的明确例外。
10. **要项目所有者决定的**（9.1）：FR-IN-06 字面、MSRV 1.91、Wintun 嵌入、`tun-rs` 依赖、`rurge-platform` 的 unsafe 预算、macOS SSID、特权测试例外、ICMP 转发方式、NFR-01 口径。


---

## 1. TUN 里的包怎样变成 TCP 流与 UDP 数据报（问题 1）

### 1.1 各项目的做法

先把做法分成五类，再逐个项目说。

**甲类：内核协议栈 + 地址改写（sing-tun 的 `system` 栈）**

sing-tun 的 `system` 栈完全不在用户态实现 TCP，工作方式如下（读自 `stack_system.go`，主分支版本）[1.6]：

- 构造时取 TUN 的第一个 IPv4 / IPv6 前缀；这个前缀里在 TUN 自己的地址之后**至少还要有一个地址**，它被用作 NAT 的"源地址"。两个地址族都没有可用地址时构造失败。
- 启动时（Windows 先调 `fixWindowsFirewall()`，忽略错误），在 TUN 地址上监听 TCP，端口填 0 让内核挑，每个地址族一个监听器，遇到可重试的错误最多重试 3 次、每次睡 1 秒；每个监听器有自己的 accept 循环。
- 读包循环按设备类型分：Wintun 一次一个包（`ReadPacket`）；Linux TUN 批量读写；macOS 只有开启实验选项 `EXP_MultiPendingPackets` 才批量。
- TCP 去程：按（源，目的）在 NAT 表里取或分配一个 NAT 端口，把包改写成"源 = TUN 的下一个地址:NAT 端口，目的 = TUN 地址:监听器端口"，写回 TUN；内核把它交给本机监听器。accept 到的连接的对端端口就是 NAT 端口，`LookupBack(端口)` 返回保存的会话（原始源与原始目的），交给上层处理器。回程反向改写。回环目的地址、发往 TUN 自身的 ICMP 等少数情形不进 NAT。
- 校验和：没有发送校验和卸载时做增量更新（IPv4 头、TCP 伪首部、端口）；Linux 上有 TX 校验和卸载时把 TCP 校验和置 0 交给内核。
- UDP 与 ICMP 仍在用户态：UDP 走 `UDPNat`（映射 / 过滤 / 超时由选项控制）；只对 code 为 0 的回显请求（IPv4 type 8、IPv6 type 128）本地应答；**IPv4 分片一律丢弃，不重组**。
- 同类的文档说法：sing-box 文档把 `system` 描述为"用操作系统的网络栈做 L3 到 L4 的转换"，`gvisor` 是 gVisor 的虚拟栈，`mixed` 是系统 TCP + gVisor UDP [1.1]。

它在三个平台上的差别（读自同一文件）[1.6][1.9]：Windows 要加防火墙入站规则——sing-tun 的做法是为当前可执行文件加一条"名称 `sing-tun (<绝对路径>)`、TCP、入站、允许、所有配置文件"的规则，通过它内部的 `winfw` 包（名字像是 Windows 防火墙 COM API 的封装，源码里没展开）；mihomo 文档的说法是"防火墙开着时 `system` 与 `mixed` 不能用，必须在'允许应用通过防火墙'里放行内核程序" [1.10]。Linux 用批量读写（`BatchRead` / `BatchWrite`）并读取 TUN 的 TX 校验和卸载能力；sing-tun 在 Linux 上建好 TUN 后还会把该接口的 `rp_filter` 从严格（1）放宽到 2（见第 3 节），否则改写出来的"源地址在 TUN 子网里"的包可能被反向路径过滤丢掉（后半句是推断）；mihomo 文档说 Linux 通常无需额外配置，防火墙挡住时可加一条放行来自 TUN 接口的出站流量的 iptables 规则。macOS 在 sing-tun 里无额外步骤；mihomo 文档说 macOS 通常无需配置（防火墙缺省放行已签名软件），仍不通时把 Mihomo 应用加进防火墙选项 [1.10]。

**乙类：gVisor netstack（sing-tun 的 `gvisor`、tun2socks、Xray 等）**

- sing-tun 的 gVisor 栈：固定一个 NIC，**混杂模式与地址欺骗（spoofing）都打开**，IPv4 与 IPv6 各一条全覆盖的路由指向这个 NIC；启用 IPv4 / IPv6、TCP / UDP / ICMPv4 / ICMPv6；TCP 打开 SACK 与接收缓冲自动调节；Windows 上把 TCP 恢复选项置 0；iOS 上缓冲缩到缺省 32 KiB、上限 128 KiB [1.8]。
- tun2socks（xjasonlyu，MIT）：`CreateStack` 先注册传输层处理器、再建 NIC，然后 `SetPromiscuousMode` + `SetSpoofing`，取代旧的"给 NIC 加 0.0.0.0/0 地址段"的做法；TCP 用 `tcp.NewForwarder`（`maxInFlight` 为 2048），回调里 `CreateEndpoint` 才完成三次握手（失败则 `Complete(true)` 发 RST），连接的 `TransportEndpointID` 带着原始目的地址；连接上开 keepalive（空闲 60 秒、间隔 30 秒、9 次）[1.12]。
- tun2socks 的 wiki 基准（iperf3，10 秒，MTU 9000，客户端 i5-10500 / 服务端 Atom C3538，SOCKS5 服务器；**没有给 CPU 与内存**）：原始链路 上行 6.27 / 6.84 Gbit/s（1 / 10 流）、下行 9.41 / 9.42；tun2socks v2.3.2 上行 2.92 / 4.76、下行 3.03 / 7.38；go-tun2socks（lwIP）上行 331 / 335 Mbit/s、下行 2.25 / 2.58 Gbit/s；badvpn-tun2socks 上行 674 / 792 Mbit/s、下行 2.88 / 2.96 Gbit/s [1.12]。
- 只有 Go 能直接用 gVisor；Rust 项目里没有对应物。

**丙类：自己写的栈**

- **sing-tun 新栈**（`stack_go*.go`，sing-box 1.15.0-alpha.3 起，文档称"相较以前所有实现有显著改进"，改进方向写的是峰值性能、能耗与内存）[1.2][1.3][1.7]：每个平台队列一个 `goEngine`（多队列 TUN 对应多个引擎，引擎各有自己的流表 / 监听表 / UDP 套接字表，跨引擎的包用 `inject()` 转交）；读包后做可选的校验和验证、丢弃广播与非全球单播、IPv4 / IPv6 分片重组（表项按版本 + 协议 + 源 + 目的 + 标识符为键，3 秒超时，每项最多 8 个字节区间）；TCP 拥塞控制可选（文件里有 `cubic`、`bbr`），有定时器轮（`goWheel`）、ACK 合并（引擎忙度在 100 ms 窗口里 ≥ 70% 且至少 2 条流时打开）、内存竞技场与环形缓冲、`splice`；监听器按"精确地址:端口"登记，积压上限 4096；ICMP 回显在栈内改写成应答；有 `MemoryTun`（见第 8 节）。`dev` 分支的 `NewStack` 里空字符串或 `"go"` 取新栈，`gvisor` / `mixed` / `system` 仍保留 [1.5]。
- **sing-tun 的"流裁决"**：`dev` 分支有 `FlowVerdict`，动作是 `ActionAccept`、`ActionFlow`、`ActionReject`、`ActionDrop`、`ActionBypass`、`ActionHijackDNS`，每条流可挂一个 `FlowTracker`（`CountForward` / `CountReverse` / `FlowEstablished` / `CloseFlow`）；处理器接口里有 `JudgeFlow` [1.7]。源码摘要没说裁决发生在 SYN-ACK 之前还是之后，但新栈的引擎里有"已裁决、待放行（engage）才发出预先备好的 SYN-ACK"的状态 [1.7]——这与阶段 3 需要的"预匹配提前到 SYN 阶段"是同一类设计。
- **mihomo 的 `mips` 栈**（`github.com/metacubex/mipstack`，MPL-2.0，纯 Go，无 cgo，pkg.go.dev 显示版本 `v0.0.0-20261015155213-f8709938043b`、2026-10-15）：Reno / CUBIC（缺省）/ BBR / BBR3 可选，SACK，窗口缩放按接收上限逐连接选，IPv6 含扩展头与流标签，ICMP 回显与错误（含 RFC 4884 / 8883），无状态分片与增量重组，可选把校验和验证交给"可信输入链路"；`Config.Promiscuous` 放进目的地址不是本机的合法单播包；TCP 的 `Accept` 返回的连接的 `LocalAddr` 保留原始目的地址；批量收发最多 64 个包 [1.11]。mihomo 文档称它是缺省，且只有它支持 `congestion-controller` [1.10]。
- **ipstack**（narrowlink，Apache-2.0；主分支版本 `1.0.2-dev`，最近标签 `v.1.0.1`，2026-10-12 有推送，88 星）：自己实现 TCP，用 `etherparse` 解析包，会话表是 `AHashMap<NetworkTuple, 发送端>`（O(1) 查找），对外是 `accept()` 返回 `IpStackStream::{Tcp, Udp, UnknownTransport, UnknownNetwork}`，TCP 流可直接 `copy_bidirectional`；缺省 MTU 1280、UDP 超时 30 秒。**源码核对（tcb.rs）到的缺口**：没有拥塞控制，没有 SACK，没有时间戳，没有 RTT 估计（RTO 取配置值、至少 1 秒、每次翻倍、上限 max(60 s, 配置值)，最多重传 7 次、注释说约 123 秒），没有 MSS 协商（负载取 `min(发送窗口, MTU − 头)`），没有 keepalive 与零窗口探测定时器，只做被动打开（`SynSent` 被注释掉）；窗口缩放有（对端位移钳到 14），乱序段存 `BTreeMap`；通道全是无界的；lib.rs 里**不检查 SYN 标志**——任何未知四元组的第一个 TCP 包都会建出一条新流 [1.17]。

**丁类：lwIP**

- **hev-socks5-tunnel**（heiher，MIT；2.19.0，2026-10-15 发布）：`third-part/` 里是 lwIP 的分支、`hev-task-system`（协程）、`wintun`（目录，随仓库）与 `yaml`；配置缺省 MTU 8500、`task-stack-size` 86016 字节、`tcp-buffer-size` 65536 字节，有 `max-session-count` 与一个面向 iOS 的低内存档位；UDP 支持 fullcone、UDP-in-UDP 与 UDP-in-TCP；可本地应答 ICMP 回显 [1.13]。
- **hev 自己公布的横向基准**（wiki「Benchmarks」，**自报、未经第三方复核**）[1.23]：AMD Ryzen 9 7950X，Ubuntu 26.04.1（Linux 7.2.9），两个网络命名空间，SOCKS5 服务器是 hev-socks5-server 2.13.1（16 个 worker），`tun0` 的 MTU 8500，iperf3 3.20；上传用 `iperf3 -c`，下载用 `-R -Z`；表里每格是（Gbit/s；CPU %；内存）：hev-socks5-tunnel 2.19.0 上传 1 流 60.0（78%；2.6 MB）、10 流 98.0（344%；10.6 MB），下载 1 流 52.6（77%；2.5 MB）、10 流 184（316%；10.1 MB）；sing-box 1.14.3 上传 23.9（166%；83.7 MB）、10 流 19.3（156%；84.5 MB），下载 20.8（135%；84.2 MB）、10 流 16.6（124%；83.9 MB）；xjasonlyu-tun2socks 2.7.0 上传 14.8（155%；25.3 MB）、10 流 25.5（320%；30.6 MB），下载 12.0（150%；27.3 MB）、10 流 50.8（1313%；76.1 MB）；Xray-core 26.3.27 的 tun2socks 上传 15.2 / 21.2、下载 13.6 / 32.6（内存 53 ～ 77 MB）；badvpn-tun2socks 1.999.130 上传 4.26 / 3.95、下载 9.38 / 9.53（内存约 2.3 ～ 2.6 MB）；leaf-tun2socks 0.14.2 上传 7.70 / 6.55、下载 5.11 / 1.21（内存约 15 MB）。页面没说 sing-box 与 leaf 当时用的是哪个栈；Rust 的 smoltcp 栈（shadowsocks-rust、clash-rs）不在比较之列。
- **leaf**（eycorsican，Apache-2.0，2026-09-30 有推送）的 TUN 入站：用 `tun` crate 建设备，栈二选一——`settings.tun2socks == "smoltcp"` 用 `netstack-smoltcp`，否则用 `netstack-lwip`；TCP 每条连接一个 `tokio::spawn`；fake DNS 在栈这一层做（目的是 fake IP 且查到域名就把目的改成域名；查不到域名且端口不是 80 / 443 就丢弃，注释说 80 / 443 留着是为了让分发器去嗅探域名；UDP 53 直接用 `generate_fake_response` 应答）[1.14]。
- **netstack-lwip**（leaf 的栈）：构建时用 `bindgen` 生成 lwIP 绑定，**需要 libclang**；各平台的 `lwipopts.h`：iOS 约 512 条 TCP 连接、`MEM_SIZE` 1 MiB，桌面约 2048 条、`MEM_SIZE` 8 MiB，`TCP_WND` 32 × MSS、`TCP_SND_BUF` 16 × MSS，每个 PCB 约 256 字节；**整个 lwIP 在一把全局锁后面**，所以并发流越多聚合吞吐越低；UDP 没有流控；GitHub 元数据里**许可为空**（`license: null`）[1.14]。

**戊类：Rust + smoltcp**

三个项目用的是同一种套路（`netstack-smoltcp` 的 README 把 shadowsocks-rust 列为灵感来源，clash-rs 的 `watfaq-netstack` 又自述基于 `netstack-smoltcp`）：

- 给 smoltcp 的 `Interface` 配占位地址 `0.0.0.1/0` 与 `::1/0` 和指向它们的默认路由，再 `set_any_ip(true)`，于是接口接受发往任意目的地址的包；
- 自己先解析每个 TCP 包，**遇到 SYN 且无 ACK 就新建一个 `TcpSocket` 并 `listen(目的地址:端口)`**——一个 SYN 一个套接字，而不是一个目的地一个监听器；其余的包直接推进接口的入口队列；
- 一个专用线程循环：加入新套接字、`iface.poll`、把每个套接字与它的控制块同步、删掉已关闭的套接字；没有包排队时睡 `poll_delay`（`None` 时 5 ms）；读写者靠 `Notify` / `unpark` 与 `Waker` 唤醒；
- `netstack-smoltcp`（automesh-network，MIT OR Apache-2.0，0.1.4，MSRV 1.75，smoltcp 0.12，**已归档**，README 指向 `cavivie/netstack-smoltcp`，该分支 Apache-2.0、2026-07-11 有推送）：每条连接 4 个 327,660 字节（`0x3FFF * 20`）的缓冲（smoltcp 收发各一，加上控制块里的两个环形缓冲），约 1.25 MiB；轮询标志置位后从不复位，源码摘要里看起来像忙等；保活 28 秒、超时 7200 秒；没有连接数上限，通道无界 [1.18]。
- shadowsocks-rust 的 `local-tun`（1.25.0，MIT，`rust-version = "1.91"`）：`tun` 0.8 + `smoltcp` 0.14（特性含 `socket-tcp-cubic`、`auto-icmp-echo-reply`、`socket-icmp`）+ `etherparse` 0.21；每个套接字 128 KiB（`(0x3FFF * 5).next_power_of_two()`）收发缓冲，外加控制块里同样大小的环形缓冲，即每连接约 512 KiB；每个套接字显式设 Cubic；超时 7200 秒；**没有连接数上限**；不装路由（只配设备）；ICMP 包只交给 TCP 接口，靠 smoltcp 的自动回显应答 [1.15]。
- clash-rs 的 `tun`（Apache-2.0）：`tun-rs` 2.x（`async`、`async_framed`）+ 工作区内的 `watfaq-netstack` 0.1.2（MIT OR Apache-2.0；smoltcp 0.14，特性 `socket-tcp-cubic`、`auto-icmp-echo-reply`、`async`）；其 README 自述"基于 netstack-smoltcp、API 简化、**不打算用于生产**" [1.16]。

### 1.2 smoltcp 在 rurge 里的现状，和对阶段 3 的含义

**这一小节几乎全是本机源码与项目文档里的事实，对阶段 3 的设计影响最大。**

1. **套接字查找是线性的，出口也是线性的。** 本机 `smoltcp-0.12.0/src/iface/interface/tcp.rs` 的 `process_tcp` 对每个入站 TCP 包写的是 `for tcp_socket in sockets.items_mut().filter_map(downcast)`，逐个调 `accepts`，第一个接受的处理；`socket_egress`（`mod.rs:657`）与 `poll_at`（`mod.rs:540`）同样遍历全部套接字。`v0.14.0` 的 `tcp.rs` 经 WebFetch 复核仍是同样的循环，没有哈希或索引 [1.20]。（推断）量级估算：每个套接字的 `accepts` 判断按 10 ～ 20 ns 算，1 千个套接字是每包 10 ～ 20 µs，1 万个是 100 ～ 200 µs——后者只够每秒 5 ～ 10 千个包（约 60 ～ 120 Mbit/s），而且与连接是否活跃无关。rurge 现在的 WireGuard 出站里，每条经隧道的连接就是该隧道 `SocketSet` 里的一个套接字（`stack.rs`），一条隧道上的并发连接通常是几十到几百条（推断），还没碰到这个限制；阶段 3 的 NFR-01 写的是"1 万并发连接常驻内存 < 300 MB"。
   - 绕开的办法有两种（推断）：①按四元组哈希分片，N 个 `Interface` + `SocketSet`（每片一个线程或一个任务），每片只放几百到两三千个套接字；②每条连接一个只含一个套接字的 `Interface`，由外面的哈希表（四元组 → 连接）直接派发——这样查找是 O(1)，但每个 `Interface` 带自己的路由表与分片重组缓冲；rurge 现在的特性集开了 `reassembly-buffer-size-16384` 与 `reassembly-buffer-count-4`（工作区 `Cargo.toml`），（推断）smoltcp 把重组缓冲放在 `Interface` 结构体里，按这两个特性算每个接口至少约 64 KiB，阶段 3 要把分片相关特性裁掉或只给少数接口开。后一种做法我没有在任何公开项目里见过，需要小实验。
2. **阶段 2 记下的 smoltcp 0.12 限制，官方后来修了一部分。**
   - 项目文档 `docs/superpowers/plans/2026-09-28-phase2-m4b-wireguard-plan.md` 的 P2 / P3 条记录：一次 `Interface::poll` 对每个套接字只 `dispatch` 一次，所以 rurge 反复 `poll` 到 `PollResult::None`；Cubic 把窗口按字节算而不是按报文段，窗口一直是一两个报文段，改用 Reno；重传是回退 N（不按 SACK 补发）；发送方没有零窗口探测；CLOSE-WAIT / LAST-ACK / CLOSING 里一个 ACK 会把重传定时器清空；keep-alive 探测发 `seq − 1` 加一个 `0x00` 字节、状态不对时会被当数据交出去；"smoltcp 0.13 起要求 Rust 1.91（M4-D3），是否修了这些要到升级时再核对"。同一份文档的实测：回环上两端都是 smoltcp 的 64 MiB 双向回显约 6.5 MiB/s 每方向（后来一次 5.8），被 Windows UDP 系统调用主导，作者标为"偏保守的下限"。
   - smoltcp 的更新日志 [1.20]：**0.13.0（2026-03-20）**——MSRV 升到 1.91、edition 2024；加入 TCP 零窗口探测；重传指数退避按 RFC 6298，新确认数据时重启重传定时器，修了 CLOSING 状态的 FIN 重传；窗口外的字节被拒收，监听套接字不再接受 RST；"出口套接字状态在一次 `poll` 里耗尽，排队的数据不必等下一次 `poll`"；`any_ip` 现在在本地地址检查里生效；加了 `pause_synack` 特性和 `no-auto-icmp-echo-reply` 开关。**0.13.1（2026-05-01）**——修了监听套接字收到来自未指定地址的 SYN 时 panic、SACK 序号溢出 panic、IPv4 头长度检查。**0.14.0（2026-08-07）**——拥塞控制 API 重新设计，Reno 与 CUBIC 做成符合 RFC，快速重传改进；窗口外数据段的 ACK 不再限速；MSS 处理改进；校验和计算优化、单遍零分配的 TCP 选项解析；PHY 层支持 TCP 分段卸载。未发布：每套接字可设最小 / 初始 RTO。
   - 所以阶段 2 记下的 ①Cubic、③零窗口探测、"一次 `poll` 一个报文段"在 0.13 / 0.14 都有对应修复；②回退 N 重传（SACK）没见修复，0.14 的 README 仍写"Selective acknowledgements are **not** implemented"；④（CLOSE-WAIT / LAST-ACK / CLOSING 里 ACK 清掉重传定时器）可能被 0.13.0 的"新确认数据时重启重传定时器"与"修了 CLOSING 状态的 FIN 重传"部分覆盖（推断）；⑤（keep-alive 探测字节）更新日志里没有对应。两项都要升级时再用阶段 2 的用例核对。
   - **MSRV**：`smoltcp` 0.14.0 的 `Cargo.toml` 是 `edition = "2024"`、`rust-version = "1.91"`、`license = "0BSD"`；rurge 工作区是 `rust-version = "1.89"`，CI 钉 1.89.0。shadowsocks-rust 1.25.0 自己也已是 `rust-version = "1.91"`。升不升 MSRV 是项目所有者的决定。
3. **`pause_synack`**（0.14 的 `socket-tcp-pause-synack` 特性）：`pub fn pause_synack(&mut self, pause: bool)`，文档说套接字会"停在 SynReceived 状态而不发 SYN|ACK"直到清除标志，"对某些小众的 TCP 代理场景有用"。这是"SYN 阶段预匹配"的现成钩子；不升级也可以不用它——在自己的层上把 SYN 包扣住、裁决之后再喂给 smoltcp（客户端 1 秒后会重发 SYN），或者对要拒绝的流直接回 RST（推断）。
4. **窗口缩放由接收缓冲大小决定**：smoltcp 在 `new()` 与 `reset()` 里按接收缓冲容量的 log2 − 16 算位移，对端不通告窗口缩放就置 0；缓冲超过 1 GiB 会 panic；没有公开的设置接口（0.14 `tcp.rs`）[1.20]。
5. **rurge 已有可复用的壳**：`crates/rurge-proto-wireguard/src/stack.rs` 就是"全部状态在一把锁后面、调用方喂包取包、`advance` 反复 `poll` 到 `PollResult::None`、`register_recv_waker` / `register_send_waker` 挂起读写者、`poll_delay` 排定时器"的结构；它是主动打开（TCP_BUFFER 每方向 256 KiB），阶段 3 需要的是被动打开 + `any_ip`。
6. **内存预算**：NFR-01 的 300 MB / 1 万连接，折算每条连接 30 KB 左右（含 rurge 自己的转发缓冲，转发循环缓冲 8 KiB，见 CLAUDE.md M8c 任务 9）。smoltcp 的收发缓冲在创建套接字时按固定容量分配，所以 rurge 现有的每方向 256 KiB 完全不能照搬；上面几个 Rust 项目的做法（128 KiB ～ 327,660 字节 × 多份）也都远超预算。本地 TUN 这一段的往返时延只有几十微秒，对窗口的需求很小（推断），把收发缓冲压到 8 ～ 32 KiB 级别不会限制吞吐，但需要实测。

### 1.3 比较

下表的"吞吐"一栏只填来源里有数字的；没有数字的写"无数据"。"每连接内存"是用户态进程里为一条连接分配的缓冲（不含内核）。

| 方案 | TCP 实现 | 语言 / 许可 | 吞吐 / CPU（有数据者） | 每连接内存 | 正确性要点 | IPv6 / ICMP | 成熟度（本次读到的） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| sing-tun `system` | 内核 TCP，包改写 + NAT 表 | Go，GPL-3.0+（附加条款） | 无数据；mihomo 文档称 `system`"更稳定、更全面、资源占用更低" [1.10]，第三方文档称最快、CPU 低 [1.22] | 用户态几乎为 0（内核另算） | 内核负责 SACK / 窗口缩放 / 重传 / 拥塞控制；IPv4 分片被丢弃 | 都有；ICMP 回显本地应答 | sing-box 多年缺省；1.15 起被新栈取代为推荐 |
| sing-tun `gvisor` | gVisor netstack | Go，GPL（gVisor 本身 Apache-2.0） | tun2socks（同为 gVisor）：1 流上行 2.92 Gbit/s、下行 3.03，10 流 4.76 / 7.38 | 无数据 | SACK、接收缓冲自动调节 | 都有 | 成熟 |
| sing-tun 新栈 / mihomo `mips` | 自写 | Go，GPL-3.0+ / MPL-2.0 | 无数据（文档只给定性说法） | 无数据（用了内存竞技场） | CUBIC / BBR，SACK，窗口缩放，分片重组 | 都有 | 2026 年新；sing-box 里仍是测试版 |
| lwIP（hev / netstack-lwip） | lwIP | C（hev）/ Rust+C 绑定；hev MIT，netstack-lwip 许可未标 | hev 自报：1 流上传 60.0 / 下载 52.6 Gbit/s，CPU 约 78%，内存 2.5 MB（见 1.1，横向比较表在 [1.23]）；netstack-lwip 全局一把锁；leaf 在同一张表里上传 7.7 / 下载 5.1 Gbit/s（栈未说明） | hev：`tcp-buffer-size` 65536 + 协程栈 86016；lwIP：每 PCB ~256 B + `TCP_WND` 32×MSS | lwIP 的 TCP 比较完整，但配置参数多 | 都有 | 老牌；Rust 绑定要 libclang |
| Rust + smoltcp（shadowsocks-rust / clash-rs / leaf 可选） | smoltcp 0.12 / 0.14 | Rust，MIT / Apache-2.0；smoltcp 0BSD | smoltcp 自己的基准（i5-13500H）：tap 读 3.67 Gbit/s、写 7.9 Gbit/s，回环 15.4 Gbit/s；rurge 的 WireGuard 回环 6.5 MiB/s（受其他因素限制） | 128 KiB ～ 327,660 B 的多份（见上） | 0.14 起 Reno / CUBIC 合规、零窗口探测；无 SACK 重传；**线性套接字查找** | 都有；0.14 `auto-icmp-echo-reply` | 项目多但实现都是"被动打开拼装"；`watfaq-netstack` 自称不用于生产；`netstack-smoltcp` 已归档 |
| ipstack | 自写 | Rust，Apache-2.0 | 无数据 | 缺省读缓冲 16 KB；通道无界 | 无拥塞控制 / SACK / RTT 估计 / MSS 协商；不检查 SYN | IPv6 在解析器里；ICMP 不处理（交给应用） | 1.0.1；被 tun2proxy 使用 |

### 1.4 对 rurge 的建议（我的判断）

1. **先决定的不是 crate，而是 FR-IN-06 的字面。** PRD 写的是"TCP / UDP 通过用户态协议栈进入流水线"；`system` 栈用的是操作系统的 TCP，严格说不满足。如果项目所有者愿意把这一句改成"TCP 经用户态协议栈或经系统 TCP 栈的地址改写"，`system` 栈就是候选。
2. **我的排序：**
   - **第一选择：smoltcp，分片使用。** 理由：已在依赖树里（0BSD），rurge 已积累了它的坑；纯 Rust、全平台同一份代码、没有防火墙入站规则问题（用户态栈不需要监听器）；可以用内存里的设备在没有 root 的 CI 上完整测试；SYN 阶段的裁决点自然（见 1.2 第 3 条）。条件：必须按 1.2 第 1 条分片（或每连接一个 `Interface`），并把每连接缓冲压到 8 ～ 32 KiB；最好升 smoltcp 到 0.14（需要 MSRV 1.91）。
   - **第二选择（也是第一选择不达标时的退路）：`system` 栈。** 它在吞吐 / CPU / 内存 / TCP 正确性上有最强的论据（全交给内核），也是同类产品里验证最久的做法；代价是 Windows 防火墙规则（推断：企业组策略不允许合并本地规则时，这条规则会失效，`system` 栈就起不来）与 PRD 字面。包的解析、改写、校验和增量更新可以直接用 `smoltcp::wire`（已在依赖树里）或 `etherparse`，不必另写。
   - **不建议：** ipstack 做主栈（TCP 太简化：RTO 固定、无 RTT 估计、不检查 SYN、通道无界，做 1 万连接的内存上界不好控）；lwIP 绑定（需要 libclang，netstack-lwip 许可不明，全局锁）；gVisor（只有 Go）。
3. **接口要抽象成一个 trait**（PRD R8 已要求"通过 trait 抽象隔离第三方库"）：输入是"TUN 读到的 IP 包"，输出是"新 TCP 流（`BoxedStream`，带原始四元组）"与"UDP 数据报（实现现有的 `UdpClient`：`recv` 返回负载与目标、`send` 以某个来源发回）"，这样 smoltcp 栈与 `system` 栈能互换。rurge 现成的 `Dialer::dial(SessionInfo) -> Dialed{stream: BoxedStream}`、`relay`、`associate(Arc<dyn UdpClient>, SessionInfo, CancellationToken)` 已经是这个形状（`crates/rurge-inbound/src/session.rs`）。
4. **先做一个小实验再定**（判据来自 PRD 阶段 3 验收："性能基准达到 NFR-01 目标的 70% 以上"）：同一份回环 TUN 夹具下，量 ①10k 条空闲连接 + 100 条活跃连接时 smoltcp 分片栈的每包 CPU 与 RSS；②单流吞吐；③同样场景的 `system` 栈。数字出来之前，1.2 里的量级估算不要当结论用。
5. **ICMP**：项目的兼容性清单 9.1 节已经按 Surge 手册定了"转发出去、应答由 VIF 交回"，不是在本机伪造。可行的转发办法见 sing-tun 的 `ping/` 包：Linux 与 macOS 无特权时用 `SOCK_DGRAM` + `IPPROTO_ICMP`（macOS 无特权可用；Linux 受 `net.ipv4.ping_group_range` 约束），有特权时 `SOCK_RAW`；Windows 用原始 ICMP 套接字（需要管理员）；为了让 traceroute 的 Time Exceeded 能回来，套接字刻意**不 connect** [1.21]。（推断）Windows 上还可以用 IP Helper 的 `IcmpSendEcho2`，不要求原始套接字，但源码摘要里 sing-tun 没用它。
6. **Linux 还有一条旁路**：sing-box 的 `auto_redirect`（nftables 把 TCP 重定向到本机监听器，不经 TUN 的用户态栈）只在 Linux 可用，要求 `auto_route`，与 `route.default_mark` 冲突 [1.1]。它是 Linux 上的性能优化选项，不能替代三平台的 TUN 方案，阶段 3 不必做。

### 本节来源

- [1.1] sing-box TUN 入站文档（`stack` 选项、`auto_redirect`、`strict_route`、`dns_mode`、`gso` 等）：https://sing-box.sagernet.org/configuration/inbound/tun/
- [1.2] sing-box 更新日志（1.15.0-alpha.3 "Significantly improve TUN performance with a new TCP/IP stack"；版本顺序 1.15.0-alpha.12、alpha.11、1.14.3 …）：https://sing-box.sagernet.org/changelog/
- [1.3] sing-box 迁移指南 "Migrate TUN stack"（1.15 起 `stack` 弃用、1.17 移除）：https://sing-box.sagernet.org/migration/
- [1.4] sing-tun 仓库与元数据（默认分支 `dev`，2026-10-13 推送，许可 GPL-3.0 或更新版本加附加条款，GitHub 标 NOASSERTION）：https://github.com/SagerNet/sing-tun 、https://api.github.com/repos/SagerNet/sing-tun
- [1.5] `stack.go`（主分支 / `dev` 分支）：https://raw.githubusercontent.com/SagerNet/sing-tun/main/stack.go 、https://raw.githubusercontent.com/SagerNet/sing-tun/dev/stack.go
- [1.6] `stack_system.go`：https://raw.githubusercontent.com/SagerNet/sing-tun/main/stack_system.go
- [1.7] 新栈：https://raw.githubusercontent.com/SagerNet/sing-tun/dev/stack_go.go 、`stack_go_engine.go`、`stack_go_listen.go`、`flow.go`（同目录）
- [1.8] `stack_gvisor.go`：https://raw.githubusercontent.com/SagerNet/sing-tun/dev/stack_gvisor.go
- [1.9] `stack_system_windows.go`（`fixWindowsFirewall`）：https://raw.githubusercontent.com/SagerNet/sing-tun/main/stack_system_windows.go
- [1.10] mihomo TUN 文档（stack 取值、防火墙说明、`dns-hijack` 限制）：https://wiki.metacubex.one/en/config/inbound/tun/
- [1.11] mipstack：https://pkg.go.dev/github.com/metacubex/mipstack
- [1.12] tun2socks：https://github.com/xjasonlyu/tun2socks 、`core/stack.go`、`core/tcp.go`（https://raw.githubusercontent.com/xjasonlyu/tun2socks/main/core/stack.go）、基准 https://github.com/xjasonlyu/tun2socks/wiki/Benchmarks
- [1.13] hev-socks5-tunnel：https://github.com/heiher/hev-socks5-tunnel 、`third-part` 清单 https://api.github.com/repos/heiher/hev-socks5-tunnel/contents/third-part 、最新发布 2.19.0（2026-10-15）https://api.github.com/repos/heiher/hev-socks5-tunnel/releases/latest
- [1.14] netstack-lwip：https://github.com/eycorsican/netstack-lwip ；leaf：https://raw.githubusercontent.com/eycorsican/leaf/master/leaf/src/proxy/tun/inbound.rs 、https://api.github.com/repos/eycorsican/leaf
- [1.15] shadowsocks-rust（1.25.0）：https://raw.githubusercontent.com/shadowsocks/shadowsocks-rust/master/crates/shadowsocks-service/src/local/tun/tcp.rs 、同目录 `mod.rs`、`../../Cargo.toml`
- [1.16] clash-rs：https://raw.githubusercontent.com/Watfaq/clash-rs/master/clash-lib/Cargo.toml 、https://raw.githubusercontent.com/Watfaq/clash-rs/master/clash-netstack/Cargo.toml 、`clash-netstack/README.md`、LICENSE（Apache-2.0）
- [1.17] ipstack：https://raw.githubusercontent.com/narrowlink/ipstack/main/Cargo.toml 、`src/lib.rs`、`src/stream/tcb.rs`、元数据 https://api.github.com/repos/narrowlink/ipstack
- [1.18] netstack-smoltcp：https://github.com/automesh-network/netstack-smoltcp 、https://raw.githubusercontent.com/automesh-network/netstack-smoltcp/master/src/tcp.rs 、`Cargo.toml`、分支 https://api.github.com/repos/cavivie/netstack-smoltcp
- [1.19] tun2proxy：https://github.com/tun2proxy/tun2proxy 、`Cargo.toml`（0.8.5，MIT，`ipstack = "1.0.1"`）
- [1.20] smoltcp：本机 cargo 注册表里的 `smoltcp-0.12.0` 源码（`README.md`、`src/iface/interface/tcp.rs`、`mod.rs`）；更新日志 https://raw.githubusercontent.com/smoltcp-rs/smoltcp/main/CHANGELOG.md ；v0.14.0 的 `Cargo.toml`、`README.md`、`src/socket/tcp.rs`、`src/iface/interface/tcp.rs`（https://raw.githubusercontent.com/smoltcp-rs/smoltcp/v0.14.0/ 下）
- [1.21] sing-tun 的 `ping/` 包：https://raw.githubusercontent.com/SagerNet/sing-tun/dev/ping/socket_unix.go 、https://raw.githubusercontent.com/SagerNet/sing-tun/dev/ping/socket_windows.go
- [1.22]（搜索摘要，未能直接读取）第三方文档对各栈的定性比较：https://core-tutorial.argsment.com/singbox/tun 、https://singbox-internals.hidandelion.com/protocols/tun.html（后者两次读取都因连接被关闭失败）
- [1.23] hev-socks5-tunnel 自己的横向基准（自报）：https://github.com/heiher/hev-socks5-tunnel/wiki/Benchmarks
- 本仓库：`docs/requirements.md`（FR-IN-06、NFR-01、阶段 3 验收）、`docs/surge-compatibility-matrix.md` 9.1 / 9.10 节、`docs/superpowers/plans/2026-09-28-phase2-m4b-wireguard-plan.md`（P2 / P3 / P19）、`crates/rurge-proto-wireguard/src/stack.rs`、`crates/rurge-inbound/src/session.rs`

---

## 2. Rust 里的 TUN 设备 API，与三个平台的原始接口（问题 2）

### 2.1 Rust crate

| crate | 版本（读到的） | 许可 | 平台 | 要点 |
| --- | --- | --- | --- | --- |
| `tun-rs` | 2.8.11；2026-10-16 当天有推送，231 星 | Apache-2.0 | Linux、Windows、macOS、FreeBSD / OpenBSD / NetBSD、Android、iOS、tvOS、OpenHarmony | `DeviceBuilder`（`name` / `layer(L3 或 L2)` / `ipv4` / `ipv6` / `mtu` / `enable` / `build_sync` / `build_async`）；Linux 有 `offload(true)`（TSO / GSO，配合 `recv_multiple()`）与 `multi_queue(true)`；Windows 的 TUN 要把 `wintun.dll` 放在可执行文件同一目录，程序须以管理员运行；`from_fd` 一类入口是 `unsafe`；缺省特性 `address-management`（`netconfig-rs`、`ipnet`、`getifaddrs`；其中 `netconfig-rs` 0.1.6 是 tun-rs 组织对 GamePad64/netconfig 的分支，MIT，依赖 `windows` ^0.61.3、`nix` ^0.30.1、`netlink-*`，会与 rurge 现有的 `windows` 0.62.2、`nix` 0.31.3 并存成两个版本；`getifaddrs` 0.6.2 是 MIT OR Apache-2.0）[2.17]；Unix 上依赖 `nix` 0.31（`ioctl`、`net`），Windows 上依赖 `windows-sys` 0.61、`libloading` 0.9、`winreg`、`widestring`，macOS / BSD 上依赖 `route_manager` 0.2.9；Rust edition 2021，未设 `rust-version` [2.1] |
| `tun` | 0.8.14；2026-09-26 有推送，685 星 | **WTFPL**（README 与 `Cargo.toml` 都写 WTFPL，GitHub 的许可检测为空） | Windows、Linux、macOS、FreeBSD、Android、iOS、tvOS、OpenHarmony | Windows 经 `wintun-bindings` 0.7.39；Unix 经 `nix` 0.31.3（`ioctl`）；`async` 特性给 tokio；edition 2024，未设 `rust-version`；Linux 需 root、Windows 需把 `wintun.dll` 放在可执行文件旁并以管理员运行；macOS / FreeBSD 按配置自动加一条路由 [2.2] |
| `wintun` | 0.5.1（页面日期 2026-08-30） | MIT | Windows（aarch64 / i686 / x86_64 MSVC） | Wintun 的安全封装：`load` / `load_from_path` / `load_from_library`（三个都是 `unsafe`，因为是加载任意 DLL）、`Adapter::create` / `open`、`start_session(容量)`、`allocate_send_packet` / `send_packet`；**不内嵌 DLL**；没有异步；依赖 `libloading` 0.8、`windows-sys` 0.52 [2.3] |
| `wintun-bindings` | 0.7.40（页面日期 2026-08-17） | MIT | Windows | 同上的分支（tun2proxy 组织维护，`tun` 0.8 用它）；多了 `AsyncSession`、`verify_binary_signature` 特性（加载前校验 DLL 签名）、`get_running_driver_version`、`get_active_network_interface_gateways` [2.3] |
| `tun2` | 4.0.0（2024-10-27） | WTFPL | Windows、Linux、macOS 等 | 页面写明"不再维护"，代码已并回 `tun`；依赖 `wintun-bindings` ^0.7.7 [2.18] |

补充：`tun` 与 `tun-rs` 都不装路由、不改 DNS（`tun` 在 macOS / FreeBSD 上会按配置自动加一条路由），这些要自己做（第 3、4 节）。

### 2.2 原始接口

**Linux：`/dev/net/tun`**

- 打开 `/dev/net/tun`，对它 `ioctl(TUNSETIFF)` 注册设备；`IFF_TUN` 得到 L3 设备（"TUN works with IP frames"），`IFF_TAP` 得到 L2 设备；`IFF_NO_PI` 去掉每帧前面 4 字节的包信息（`struct tun_pi { __u16 flags; __be16 proto; }`）[2.4]。
- 头文件里的常数（`include/uapi/linux/if_tun.h`）：`TUNSETIFF = _IOW('T', 202, int)`、`TUNSETPERSIST = 203`、`TUNSETOWNER = 204`、`TUNGETFEATURES = _IOR('T', 207, unsigned int)`、`TUNSETOFFLOAD = _IOW('T', 208, unsigned int)`、`TUNSETVNETHDRSZ = 216`、`TUNSETQUEUE = 217`；`IFF_TUN = 0x0001`、`IFF_TAP = 0x0002`、`IFF_NAPI = 0x0010`、`IFF_NO_CARRIER = 0x0040`、`IFF_MULTI_QUEUE = 0x0100`、`IFF_NO_PI = 0x1000`、`IFF_VNET_HDR = 0x4000`；`TUN_F_CSUM = 0x01`、`TUN_F_TSO4 = 0x02`、`TUN_F_TSO6 = 0x04`、`TUN_F_TSO_ECN = 0x08`、`TUN_F_UFO = 0x10`、`TUN_F_USO4 = 0x20`、`TUN_F_USO6 = 0x40` [2.4]。本机 `libc-0.2.189` 已经定义了 `TUNSETIFF`、`IFF_TUN`、`IFF_NO_PI`、`IFF_VNET_HDR`、`IFF_MULTI_QUEUE`、`TUNSETOFFLOAD`，不必自己抄 [2.15]。
- 生命周期：缺省不持久——"程序关闭文件描述符时，网络设备与相应的路由都会消失"（内核文档）；要持久并交给非特权用户，需要 `TUNSETPERSIST` / `TUNSETOWNER` [2.4]。tun2proxy 的 README 也说：预先建好的设备可以让程序本身不带特权运行（`--setup` 才需要 root）[2.16]。
- 多队列：内核 3.8 起；对同一设备名重复 `TUNSETIFF` 并带 `IFF_MULTI_QUEUE`；`TUNSETQUEUE` 用 `IFF_ATTACH_QUEUE` / `IFF_DETACH_QUEUE` 启停某个队列 [2.4]。
- 卸载（offload）：wireguard-go 的 Linux 实现用 `IFF_TUN | IFF_NO_PI | IFF_VNET_HDR` 建设备（不用多队列），再 `TUNSETOFFLOAD`：`TUN_F_CSUM | TUN_F_TSO4 | TUN_F_TSO6` 失败就报错，再加 `TUN_F_USO4 | TUN_F_USO6` 失败则只是关掉 UDP GSO；读缓冲是"virtio 头 + 65535"；读到的大段要自己按 GSO 切开，写回时自己按 GRO 合并；有 vnet 头时批量大小取 `IdealBatchSize`；写时遇到 `EBADFD` 当作已关闭 [2.5]。Tailscale 的博客给出的效果：wireguard-go 在 TUN 上用 TSO / GRO、UDP 上用 GSO / GRO 之后，裸机（i5-12400、25G 网卡）从 8.36 Gbit/s 到 13.0，AWS c6i.8xlarge 从 5.34 到 7.32，老 CPU（E3-1230-V2）从 3.36 到 4.86（打开网卡 `tx-udp-segmentation` 为 6.59）；UDP GSO 需要 Linux 4.18、GRO 需要 5.0 [2.6]。
- 链路状态探测的小技巧（wireguard-go）：对 TUN 描述符每秒写一个零长度包，`EINVAL` 表示"上"，`EIO` 表示"下"，据称跨网络命名空间也有效 [2.5]。
- 地址 / MTU / 起停：sing-tun 在 Linux 上用 netlink 做（加地址时带 `IFA_F_NODAD` 跳过 IPv6 DAD；`LinkSetUp`；MTU 在启动前设置）[2.13]；wireguard-go 用 `SIOCSIFMTU`（在一个 `AF_INET` 数据报套接字上）读写 MTU [2.5]；wg-quick 与 clash-rs 用 `ip` 子进程 [3.4][3.8]。
- 权限：root 或 `CAP_NET_ADMIN`，内核要有 `tun` 模块（`modprobe tun`）[2.1]。

**macOS：utun 控制套接字**

- 没有设备节点，要经 `PF_SYSTEM` 控制套接字：`socket(AF_SYSTEM, SOCK_DGRAM, SYSPROTO_CONTROL)`（`SYSPROTO_CONTROL = 2`）→ `ioctl(CTLIOCGINFO)` 用名字 `com.apple.net.utun_control` 查控制 id → `connect` 一个 `sockaddr_ctl`（`sc_id` 为查到的 id，`sc_unit = N + 1`；`0` 让内核挑一个空闲的）→ 设为非阻塞 → 用 `getsockopt(SYSPROTO_CONTROL, UTUN_OPT_IFNAME)` 读回接口名 [2.7][2.8]。
- 名字必须是 `utunN`（mihomo 文档："On macOS, names must start with `utun`"）[3.9]。
- 每个包前面有 **4 字节地址族头**：wireguard-go 写时头的前三个字节为 0，第四个字节是 `AF_INET` 或 `AF_INET6`（由负载首字节的高 4 位 IP 版本决定，其它版本返回 `EAFNOSUPPORT`）；读时它不解析头，只是 `sizes[0] = n − 4` [2.7]；sing-tun 的批量读写同样截 4 字节 [2.8]。
- 苹果开源的 `if_utun.h`：`UTUN_CONTROL_NAME = "com.apple.net.utun_control"`；`UTUN_OPT_FLAGS = 1`、`UTUN_OPT_IFNAME = 2`、`UTUN_OPT_MAX_PENDING_PACKETS = 16`（"限制控制套接字上可以排队等读的包数"）、`UTUN_OPT_ENABLE_CHANNEL = 17` 等；`UTUN_FLAGS_NO_OUTPUT = 0x1`、`UTUN_FLAGS_NO_INPUT = 0x2` [2.9]。sing-tun 对 `UTUN_OPT_MAX_PENDING_PACKETS` 的使用放在实验选项 `EXP_MultiPendingPackets` 后面（先把 `SO_RCVBUF` 从 8 MB 试起、对半降到 1 MB，再据 MTU 算出 1 ～ 64 的上限）[2.8]。
- **用本机已有的 `nix` 0.31.3 可以不写 unsafe 建出 utun**：`socket(AddressFamily::System, SockType::Datagram, SockFlag::empty(), SockProtocol::KextControl)`（`KextControl = SYSPROTO_CONTROL`）→ `SysControlAddr::from_name(fd, "com.apple.net.utun_control", unit)`（内部做 `CTLIOCGINFO`）→ `connect` → `getsockopt(&fd, sockopt::UtunIfname)`。nix 自己的用例 `test_syscontrol`（`test/sys/test_socket.rs`）就演示了前两步，注释写明 `connect` 需要 root；`UtunIfname` 在 `apple_targets` + `net` 特性下 [2.10]。`libc` 有 `sockaddr_ctl`、`ctl_info`、`CTLIOCGINFO`、`AF_SYSTEM`、`SYSPROTO_CONTROL`、`UTUN_OPT_FLAGS` / `UTUN_OPT_IFNAME`，但**没有** `ifaliasreq` / `SIOCAIFADDR`（本机 `libc-0.2.189` 的 Apple 部分 grep 无结果）[2.15]。
- 地址：sing-tun 直接用 `SIOCAIFADDR`（IPv4：`ifaliasreq`，地址同时填 `Addr` 与 `Dstaddr`，掩码按前缀长度）与 `SIOCAIFADDR_IN6`（IPv6：`NODAD | SECURED`，生存期无限，/128 时目的地址填下一个地址），不用 `ifconfig` [2.8]；wg-quick 用 `ifconfig <utun> inet <addr> <addr> alias` / `inet6 <addr> alias` 再 `ifconfig <utun> up`，MTU 缺省取默认路由所在接口的 MTU 减 80 [3.5]。MTU：sing-tun 与 wireguard-go 都是在一个临时 `AF_INET` 数据报套接字上对接口名做 `ioctl`（wireguard-go 用 `IoctlSetIfreqMTU`）[2.7][2.8]。
- 权限：root（nix 用例注释；tun-rs 的 README）。

**Windows：Wintun**

- 接口都在 `wintun.h`（`SPDX: GPL-2.0 OR MIT`，版权 2018-2021 WireGuard LLC）；DLL 要 `LoadLibraryEx` + `GetProcAddress` 动态加载。函数：`WintunCreateAdapter(名字, 隧道类型, 请求的GUID)`、`WintunOpenAdapter`、`WintunCloseAdapter`（"释放适配器资源，并且如果适配器是用 `WintunCreateAdapter` 创建的，就删除它"）、`WintunDeleteDriver`（没有适配器在用时删驱动）、`WintunGetAdapterLuid`、`WintunGetRunningDriverVersion`、`WintunSetLogger`、`WintunStartSession`（环容量必须是 2 的幂，`WINTUN_MIN_RING_CAPACITY = 0x20000`（128 KiB）～ `WINTUN_MAX_RING_CAPACITY = 0x4000000`（64 MiB））、`WintunEndSession`、`WintunGetReadWaitEvent`（不能 `CloseHandle`）、`WintunReceivePacket` / `WintunReleaseReceivePacket`、`WintunAllocateSendPacket` / `WintunSendPacket`（`WINTUN_MAX_IP_PACKET_SIZE = 0xFFFF`）[2.11]。
- 包是不带任何头的 IPv4 / IPv6 包；`WintunReceivePacket` 返回环里的指针，没有包时返回 `ERROR_NO_MORE_ITEMS`，这时等 `WintunGetReadWaitEvent`（README 说高性能场景可以先短暂自旋）；适配器正在终止是 `ERROR_HANDLE_EOF`；发送 `WintunSendPacket` 线程安全，但发送顺序按分配顺序，且返回时不保证已发出；环满时 `WintunAllocateSendPacket` 返回 `ERROR_BUFFER_OVERFLOW` [2.11]。sing-tun 的做法：会话环 8 MiB（`0x800000`），读到 `ERROR_NO_MORE_ITEMS` 时等事件，吞吐 ≥ 约 800 Mbit/s 时改为短暂自旋；写遇 `ERROR_BUFFER_OVERFLOW` 就悄悄丢包 [2.13]。
- 地址、路由、MTU、DNS 不在 Wintun 里，用 IP Helper 经适配器的 LUID（`WintunGetAdapterLuid`）配置。WireGuard for Windows 的封装（`winipcfg`，MIT）：加地址 `CreateUnicastIpAddressEntry`；加路由 `CreateIpForwardEntry2`；删路由 `DeleteIpForwardEntry2`；取表 `GetIpForwardTable2` / `GetUnicastIpAddressTable`；DNS `SetInterfaceDnsSettings`，仅当它返回 `ERROR_PROC_NOT_FOUND`（早于 Windows 10 1809）时回落到写注册表 `Tcpip\Interfaces` + 给 `dnscache` 服务发 ParamChange；`AddIPAddress` 把 DAD 状态直接设为 preferred、生存期无限，不等 DAD [2.12]。微软文档：`SetInterfaceDnsSettings` 最低 Windows 10 build 19041 [4.12]。
- WireGuard for Windows 与 sing-tun 在接口上设置的项（几乎一致）：关路由器发现、`DadTransmits = 0`、关 managed / other-stateful 配置、`NLMTU` = 配置的 MTU；IPv4 开 `ForwardingEnabled`（sing-tun）；有默认路由时 `UseAutomaticMetric = false`、`Metric = 0` [2.12][2.13]。
- 权限：创建适配器需要管理员（`wintun` crate 的示例注释、`tun-rs` 与 `tun` 的 README 都这么写）。

### 2.3 哪些要 unsafe，大概多少（我的估计，标"推断"）

| 环节 | 自己写（rurge-platform） | 用现成 crate |
| --- | --- | --- |
| Linux 建设备 | 1 个 `ioctl(TUNSETIFF)` 调用点（`open` 用 `std::fs::OpenOptions`，`libc` 已有常数；nix 的 `ioctl_*!` 宏生成的是 `unsafe fn`）[2.10] | `tun-rs`：0 |
| macOS 建设备 | **0**（nix 的安全封装，见上）；读写用 `File::from(OwnedFd)` 之类 | `tun-rs`：0 |
| Windows 建设备 | `libloading` 加载 DLL 与取符号、再调用，各是 unsafe；包一层后约 3 ～ 5 个 FFI 调用点（推断） | `wintun-bindings` / `wintun`：1（`load_from_path`） |
| Linux 地址 / 路由 / 规则 | 用 `rtnetlink`（0.23.0，MIT）：0 | 同左 |
| macOS 地址 / 路由 | 子进程 `ifconfig` / `route`：0；自己拼 `PF_ROUTE` 消息和 `SIOCAIFADDR`：要手写结构体和 unsafe，易错 | `route_manager`（Apache-2.0，构建依赖可选的 `bindgen`）：0 |
| Windows 地址 / 路由 / DNS | `windows-sys`（已在依赖树里，现用特性只有 `Win32_Networking_WinInet`、`Win32_Security`、`Win32_System_JobObjects`、`Win32_System_Threading`）每个调用都是 unsafe。同类实现里的数：clash-rs 的 `routes/windows.rs` 有 16 个 `unsafe` 块（活代码 7 个：加路由 2、取 GUID 2、设 DNS v4 / v6 各 1、加地址 1，其余 9 个在一个注释标明"不能用"的 RTM 路径里）；tproxy-config 的 `windows.rs` 16 个 `unsafe` 块。（推断）rurge 需要约 6 ～ 10 个 FFI 调用点：加 / 删 / 查路由、加地址、设 MTU 与 metric、设 DNS、变化通知 | `net-route`（MIT）：0，但构建依赖 `bindgen` |

**与项目规矩的关系**：工作区 `unsafe_code = "forbid"`，只有 `rurge-platform` 放宽到 `deny` 并且只有两个函数标了 `#[allow(unsafe_code)]`（`CLAUDE.md`）。Windows 一侧不论怎么做都要再放开若干函数；Linux 与 macOS 的设备和路由可以保持零 unsafe（Linux 设备除外的一处）。

### 2.4 对 rurge 的建议

1. **设备层用 `tun-rs`（`default-features = false` 起步），藏在一个 `TunDevice` trait 后面**：它是本次读到的唯一三平台齐全、许可合规、仍在活跃推送的 crate，rurge 代码里零 unsafe，Linux 的 TSO / GSO 与多队列留作以后的性能手段；trait 让以后能换成自己的实现（macOS 的 nix 安全封装、Linux 的一个 ioctl、Windows 的 `wintun-bindings`）。传递依赖的许可：`netconfig-rs`（MIT）、`getifaddrs`（MIT OR Apache-2.0）、`route_manager`（Apache-2.0）、`libloading` 0.9.0（ISC）都在允许名单内 [2.17]；`winreg`、`widestring`、`encoding_rs`、`scopeguard`、`ahash`、`blocking` 这次没有逐个读（据我所知都是 MIT 或 Apache-2.0，未核实），定稿时用 `cargo-deny` 过一遍。`default-features = false` 可以去掉 `netconfig-rs` 与 `getifaddrs`，避免依赖树里多出一份 `windows` 0.61 与 `nix` 0.30。
2. **不要用 `tun`**：WTFPL 不在项目允许的许可名单（MIT / Apache-2.0 / BSD / ISC / MPL-2.0）上，除非项目所有者特批。
3. **地址、MTU、路由、DNS 自己做**（第 3、4 节），因为这些需要事务性的日志与崩溃恢复，现成 crate 都没有。
4. **权限不足要给明确提示**（FR-IN-06）。建议按平台把底层错误翻译成可执行的话（错误码对应是我的推断，没有逐个实测）：Linux 打开 `/dev/net/tun` 或 `TUNSETIFF` 失败（`EPERM` / `EACCES`）→ "需要 root 或 `CAP_NET_ADMIN`（可用 `setcap cap_net_admin+ep`）"，`/dev/net/tun` 不存在 → "请 `modprobe tun`"；macOS `connect` 到 utun 控制套接字失败 → "需要 root"；Windows `WintunCreateAdapter` 报 `ERROR_ACCESS_DENIED` → "需要以管理员运行"，`wintun.dll` 找不到或签名校验失败 → 指出期望的路径与版本（第 7 节）；路由 / DNS 步骤失败时说明已经撤销了哪些、事务日志在哪里（第 3.5 节）。
5. **MTU 缺省**：mihomo 文档说 MTU "在极端条件下影响传输速率，缺省值对大多数用户够用"，没有给数字 [3.9]；hev-socks5-tunnel 缺省 8500，tun2socks 的基准用 9000 [1.12][1.13]。用户态栈的 MTU 越大包越少、CPU 越省，但 UDP 数据报（QUIC）会被应用按这个 MTU 做路径探测，需要实测（推断）。

### 本节来源

- [2.1] tun-rs：https://raw.githubusercontent.com/tun-rs/tun-rs/main/Cargo.toml 、https://raw.githubusercontent.com/tun-rs/tun-rs/main/README.md 、https://api.github.com/repos/tun-rs/tun-rs 、https://docs.rs/tun-rs/latest/tun_rs/struct.DeviceBuilder.html
- [2.2] tun：https://raw.githubusercontent.com/meh/rust-tun/master/Cargo.toml 、https://raw.githubusercontent.com/meh/rust-tun/master/README.md 、https://api.github.com/repos/meh/rust-tun
- [2.3] wintun：https://raw.githubusercontent.com/nulldotblack/wintun/main/Cargo.toml 、https://docs.rs/wintun/latest/wintun/ ；wintun-bindings：https://docs.rs/wintun-bindings/
- [2.4] 内核文档 https://docs.kernel.org/networking/tuntap.html ；头文件 https://raw.githubusercontent.com/torvalds/linux/master/include/uapi/linux/if_tun.h
- [2.5] wireguard-go（MIT）：https://raw.githubusercontent.com/WireGuard/wireguard-go/master/tun/tun_linux.go
- [2.6] Tailscale 博客：https://tailscale.com/blog/more-throughput
- [2.7] wireguard-go：https://raw.githubusercontent.com/WireGuard/wireguard-go/master/tun/tun_darwin.go
- [2.8] sing-tun：https://raw.githubusercontent.com/SagerNet/sing-tun/dev/tun_darwin.go
- [2.9] xnu：https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/net/if_utun.h
- [2.10] 本机 `nix-0.31.3`（`src/sys/socket/addr.rs` 的 `SysControlAddr`、`sockopt.rs` 的 `UtunIfname`、`test/sys/test_socket.rs` 的 `test_syscontrol`、`Cargo.toml`：MIT、`rust-version = "1.69"`）
- [2.11] Wintun：https://www.wintun.net/ 、https://git.zx2c4.com/wintun/tree/README.md 、https://git.zx2c4.com/wintun/tree/api/wintun.h 、https://git.zx2c4.com/wintun/plain/api/wintun.h（SPDX 行）
- [2.12] WireGuard for Windows（MIT）：https://raw.githubusercontent.com/WireGuard/wireguard-windows/master/tunnel/winipcfg/luid.go 、https://raw.githubusercontent.com/WireGuard/wireguard-windows/master/tunnel/addressconfig.go
- [2.13] sing-tun：https://raw.githubusercontent.com/SagerNet/sing-tun/dev/tun_windows.go 、https://raw.githubusercontent.com/SagerNet/sing-tun/dev/tun_linux.go
- [2.15] 本机 `libc-0.2.189`（`src/unix/linux_like/mod.rs`、`src/unix/bsd/apple/mod.rs`）
- [2.16] tun2proxy README：https://github.com/tun2proxy/tun2proxy
- [2.17] tun-rs 的传递依赖：`netconfig-rs` https://docs.rs/crate/netconfig-rs/latest 、`getifaddrs` https://docs.rs/crate/getifaddrs/latest 、`libloading` https://docs.rs/crate/libloading/latest 、`route_manager` https://docs.rs/route_manager/latest/route_manager/
- [2.18] `tun2`：https://docs.rs/crate/tun2/latest
- 其它编号（[3.x]、[4.12]）见第 3、4 节来源表。

---

## 3. 路由、防环、包含 / 排除路由、退出与崩溃后的恢复（问题 3）

### 3.1 默认路由怎么装（逐项目、逐平台）

**Linux**

- **wg-quick**（wireguard-tools，GPLv2，只记事实）[3.4]：`AllowedIPs` 含 `/0` 时走 `add_default`——选一个路由表号（缺省 51820，该表里已有路由就递增），同时把它设成 WireGuard 的 fwmark；`ip rule add not fwmark T table T`（没带标记的流量进新表，WireGuard 自己发出的加密包带标记，所以不会回到隧道里）；`ip rule add table main suppress_prefixlength 0`（忽略主表里的默认路由，只让更具体的路由生效）；新表里放一条指向设备的 `/0`；IPv4 把 `net.ipv4.conf.all.src_valid_mark` 设为 1；用 nft（没有就用 iptables）加两类规则（丢弃从别的接口来、目的是隧道自己地址的非本机包；给 UDP 保存 / 恢复连接标记）。进程退出的 `trap`（INT / TERM / EXIT）调 `del_if`：删规则、`ip link delete`。DNS 用 `resolvconf -a`。
- **sing-tun**（GPL，只记事实）[3.1]：不用 `ip`，用 netlink（它的 `sagernet/netlink` 分支），唯一调用的外部命令是 `resolvectl`。启动顺序：设 MTU → 加地址（`EEXIST` 忽略）→ 起接口 → 读 `rp_filter`（`all` 与本接口取大者），**有效值为 1（严格）就把本接口设为 2（宽松）** → 选路由表号（缺省 2022）→ 加路由 → **先删除保留优先级窗口（缺省从 9000 起）里的全部现存规则**，再加新规则 → 若启用 DNS，异步调 `resolvectl` 设 `~.`、默认路由和 DNS 服务器。规则从 `iproute2_rule_index`（缺省 9000）起编号，用 `goto` 跳到 `ruleStart + 10` 处的空规则：UID 排除、接口包含 / 排除、`strict_route` 时对没有地址的地址族加 `unreachable` 规则、每个本机 IPv4 地址的目的规则进新表、`lookup main suppress_prefixlength 0`、DNS（53 端口）规则、`lo` 与 TUN 源地址的规则等。`Close` 时删地址、删路由（错误忽略）、删规则（错误汇总后返回）。`setRoute` 遇到 `RouteAdd` 错误一律当失败，只有启动失败时才清理已加的路由。
- **clash-rs**（Apache-2.0）[3.8]：`ip` 子进程；`ip route add default dev <设备> table <表>`、`ip rule add not fwmark <mark> table <表>`、`ip rule add table main suppress_prefixlength 0`、可选 `ip rule add dport 53 table <表>`；某一步失败时前面的不回滚；清理时依次删这几条规则，但**不删表里的默认路由**，遇到第一个失败就停，后面的规则可能残留。
- **tproxy-config**（tun2proxy 用，MIT）[3.6]：rtnetlink；有 `SOCKET_FWMARK_TABLE` 时加 IPv4 / IPv6 的 fwmark → 表规则；对每个旁路地址（或代理主机）在主表里找覆盖它的路由，若是默认路由就复制一条指向同一下一跳的更具体路由；主表里**已有默认路由就加 `0.0.0.0/1` 与 `128.0.0.0/1`（IPv6 是 `::/1` 与 `8000::/1`）**，没有才加 `0.0.0.0/0`；DNS 用 bind-mount 一个临时文件覆盖 `/etc/resolv.conf`（失败就改写原文件）；最后检查"到代理的路由是否出口为 TUN"，是则整体失败。

**macOS**

- **wg-quick** [3.5]：`wireguard-go utun` 建设备；`ifconfig` 配地址；`/0` 时 `route -q -n add -inet 0.0.0.0/1 -interface <utun>` 与 `128.0.0.0/1`（IPv6 `::/1`、`8000::/1`）；对端端点加主机路由经默认网关（网关从 `netstat -nr` 取；取不到就指向 `127.0.0.1` / `::1` 做黑洞防环；**没有用 `-ifscope`**）；一个后台子 shell 读 `route -n monitor`，每次路由变化就重加端点路由、重设 MTU 和 DNS；DNS 用 `networksetup -setdnsservers` / `-setsearchdomains` 逐网络服务设置并保存原值。
- **sing-tun** [3.7][2.8]：非 Network Extension 下，自动路由是**八个前缀 `1.0.0.0/8`、`2.0.0.0/7`、……、`128.0.0.0/1`**（覆盖除 `0.0.0.0/8` 外的全部地址），IPv6 是 `0100::/8` 到 `8000::/1`，而不是常见的 `/1` 一对；其它平台（含 Linux、Windows）直接是 `0.0.0.0/0` / `::/0`。路由经 `AF_ROUTE` 套接字发 `RouteMessage`（`RTF_STATIC | RTF_GATEWAY`，设了 `InterfaceScope` 就加 `RTF_IFSCOPE`）；`EEXIST` 时先删再重加；`Close` 时对每条发 `RTM_DELETE`。这个文件里没有 DNS 配置，只在设完路由和关闭时执行 `dscacheutil -flushcache`。
- **clash-rs** [3.8]：全部 `route` 子进程；加具体路由 `route add -net <dest> -interface <接口>`（IPv6 `-inet6`）；"默认路由"是 `route add -ifscope <物理接口> 0/0 <网关>`（带作用域），网关来自 `route -n get default`（IPv6 `-inet6`）；清理只删这条带作用域的默认路由。物理接口靠枚举网卡并按名字优先级猜（`en`、`pdp_ip`……，名字含 `tun` 的排除），不缓存、不刷新。
- **tproxy-config** [3.6]：SCDynamicStore 读 `State:/Network/Global/IPv4` 的 `Router`、`PrimaryInterface`、`PrimaryService`；`sysctl -w net.inet.ip.forwarding=1`；删原默认路由与带作用域的默认路由，加指向 TUN 网关的默认路由，**再把原网关作为带 `-ifscope` 的默认路由加回去**；DNS：读 `State:/Network/Service/<id>/DNS` 的 `ServerAddresses` 备份，改写 `/etc/resolv.conf`，往 SCDynamicStore 写 TUN 网关作为该服务的 `ServerAddresses`，再 `networksetup -setdnsservers` 让界面可见；`killall -HUP mDNSResponder` 清缓存。

**Windows**

- **WireGuard for Windows**（MIT）[2.12]：路由来自各 peer 的 `AllowedIPs`，度量值 `Metric: 0`，下一跳填未指定地址（即在链路上）；**`0.0.0.0/0` 不拆成两个 `/1`**；只有存在默认路由时才把接口的自动度量关掉并设度量为 0；接口上关路由器发现、`DadTransmits = 0`；顺序是路由 → 地址 → 接口参数 → DNS；开机启动时遇到 `ERROR_NOT_FOUND` 最多重试 15 次、每次 1 秒。
- **sing-tun** [2.13]：`MibIPforwardRow2`，每个前缀一条，度量 0；有自动路由时接口度量也设 0（关自动度量）；设完路由后 `FlushResolverCache`；`Close` 里没有显式删路由（适配器关闭后随之消失）；`UpdateRouteOptions` 会先冲掉该适配器全部路由再重加。`strict_route` 用 Windows 过滤平台（WFP）：开一个"动态会话"的引擎，加一个最高权重的子层，规则依权重由高到低是：放行本进程的应用 ID（权重 13）；没有 IPv6 地址时放行路由器请求、邻居请求 / 通告并阻断其余 IPv6 连接（12）；放行本地接口为 TUN 的连接（11）；DNS 劫持模式下阻断远端端口 53（10）。IPv4 的"全阻断"在源码里是注释掉的。**动态会话的过滤器随引擎句柄关闭而消失**。
- **clash-rs** [3.8]：`windows` crate 的 `CreateIpForwardEntry2`（度量 0，注释说"可能太狠"）、`GetIfEntry2`、`SetInterfaceDnsSettings`、`CreateUnicastIpAddressEntry`；没有删路由函数、没有 `Drop`、没有崩溃恢复；物理接口同样靠名字优先级猜（`Ethernet`、`Wi-Fi`、`Tailscale`）。
- **tproxy-config** [3.6]：`route add` 全零目的 / 掩码经 TUN 网关、度量 6；原网关用 `GetAdaptersAddresses`（只看已启用的 802.11 / 以太网）取，失败回落到 PowerShell；旁路路由 `route add` 度量 1；DNS 用动态加载的 `SetInterfaceDnsSettings`，没有就 `netsh interface ip set dns`；`ipconfig /flushdns`；状态文件只在特性 `unsafe-state-file` 下才写；**不恢复原 DNS**。

### 3.2 包含路由与排除路由

- **Surge 的语义**（手册 `profile/general.html`）[3.17]：`tun-excluded-routes`（可选，逗号分隔的 IPv4 / IPv6 CIDR，无缺省）"绕开指定地址段，让所有流量直接通过"；`tun-included-routes`（可选，无缺省）"VIF 缺省声明自己是默认路由"；增强模式页补充：included 是"在 VIF 上发布更小的路由，使它们优先于接口本地路由"，所以地址落在本地网络路由或被排除地址覆盖时，要写进 `tun-included-routes`（项目兼容性清单 9.10 节引了这句）；`ipv6-vif`（`disabled` / `auto` / `always`，缺省 `disabled`）决定 VIF 上有没有 IPv6。
- **做法一：求补集。** sing-tun 的 `BuildAutoRouteRanges`：基础范围（用户给的 `route_address`，或 `0.0.0.0/0`，或 macOS 上的八个前缀）减去每个 `route_exclude_address`，用 `netipx.IPSetBuilder` 求出最小前缀集合，只把结果装到 TUN 上；被排除的地址因此没有 TUN 路由，自然走原来的默认路由。不依赖物理网关，网络切换时不用重装。它**不默认排除** RFC 1918 等私网段 [3.7]。
- **做法二：显式旁路路由。** wg-quick（端点主机路由经默认网关）、tproxy-config（每个旁路 IP 一条）、clash-rs 都是给物理网关加更具体的路由。网关变了（切 Wi-Fi）就要监听并重装（wg-quick 靠 `route -n monitor`）。
- **mihomo 文档的限制**：`dns-hijack` 在 macOS / Windows 上"无法自动劫持发往局域网的 DNS 请求"——因为发往 LAN 的包走的是链路本地路由、根本不进 TUN；要靠包含路由把那个 DNS 服务器的地址拉进来 [1.10]。
- **Linux 的另一种排除办法**：`ip rule` 里按 UID、接口、fwmark 排除（sing-tun 的 `exclude_uid` / `exclude_interface`）；Surge 没有对应的配置项。

### 3.3 防环：让 rurge 自己的出站连接不进 TUN

| 平台 | 办法 | 权限 | 备注 |
| --- | --- | --- | --- |
| Linux | `SO_MARK` + 规则 `not fwmark`（wg-quick、clash-rs、tproxy-config 的做法）；或 `SO_BINDTODEVICE` / `SO_BINDTOIFINDEX` 绑到物理网卡 | `SO_MARK` 需要 `CAP_NET_ADMIN`，**Linux 5.17 起 `CAP_NET_RAW` 也行**（`socket(7)`）[3.14]；`SO_BINDTODEVICE` 的权限要求这次没读到明确说法（历史上要 `CAP_NET_RAW`） | sing-box 的 `routing_mark`（Linux 专用，`default_mark`）与 `auto_detect_interface` [3.16]；sing 的控制包优先 `SO_BINDTOIFINDEX`，遇到 `ENOPROTOOPT` / `EINVAL` 就永久回落 `SO_BINDTODEVICE` [3.10]。socket2 0.6.5 已有安全的 `set_mark`、`bind_device`、`bind_device_by_index_v4/v6`（在 Linux 上是 `SO_BINDTOIFINDEX`，在 Apple 上是 `IP_BOUND_IF`）[3.14]。**rurge 现在用 `bind_device(name)`**（`crates/rurge-platform/src/socket.rs`） |
| macOS | `IP_BOUND_IF`（IPv4）/ `IPV6_BOUND_IF`（IPv6），参数是接口号 | 资料里未提特权要求（推断：不需要） | sing 的实现按网络字符串选族（`tcp6` / `udp6` / `ip6` 用 IPv6 选项，其余用 IPv4）[3.10]。**rurge 现在用 `bind_device_by_index_v4/v6`**，接口号从 `if_addrs` 取，5 秒缓存。（搜索摘要）`-ifscope` 的路由是"接口作用域"路由，绑了接口的套接字查的是对应作用域的路由；苹果开发者论坛上有人报告绑到 `utun` 的 `IP_BOUND_IF` 没有效果、也有人说有效，没有定论 [3.15]——所以"用 `/1` 一对压过全局默认路由、同时依赖作用域默认路由放行绑了接口的套接字"这条组合要在真机上验证 |
| Windows | **`IP_UNICAST_IF`**（`IPPROTO_IP`，选项号 31）与 **`IPV6_UNICAST_IF`**（`IPPROTO_IPV6`，31） | 微软文档未提特权要求（推断：不需要） | 微软文档：IPv4 的接口号"必须是网络字节序"（实现上是把号写成大端 4 字节再当 `u32` 读出），IPv6 是主机字节序；"不改变接收接口"；`GetAdaptersAddresses` 可取接口号；Vista 起支持 [3.11][3.12]。sing 的实现：没有地址时 v4 和 v6 都试，任一成功即成功 [3.10]。**socket2 没有封装**，要 `unsafe` 的 `setsockopt` |
| Windows（rurge 现状） | 绑定物理网卡的**源地址**（`pick_source`，强主机模型：绑定的源地址把出接口钉住） | 不需要（普通 `bind`） | `crates/rurge-platform/src/socket.rs`，零 unsafe。依据：微软的资料（搜索摘要）说强主机模型下主机"只能在持有该包源地址的接口上发送"，Vista / Server 2008 起的栈对 IPv4 与 IPv6 缺省都是强主机模型，选路时同时看源地址与目的地址 [3.18]。WireGuard for Windows 的 `pitfalls.go` 会在"到端点的路由经过隧道自己，而承载端点流量的接口开了转发或 `WeakHostSend`"时警告"会造成路由环"[3.13]——也就是说这个办法的前提是**这块网卡没开 weak host send / 转发**（Hyper-V、WSL、Docker、ICS 常会开）。TUN 成为默认路由之后这个前提是承重的，要在真机上验证；不够就改 `IP_UNICAST_IF` |

**自动探测物理接口**：clash-rs 按名字猜（不可取）；sing-tun 在 Windows 上取 `GetIPForwardTable2(AF_INET)` 里前缀长度 0 的路由，跳过未启用 / 虚拟 / 回环 / 未连接的接口，按（路由度量 + 接口度量）最小者；macOS 上取路由表里第一条 `RTF_UP | RTF_GATEWAY` 的 IPv4 默认路由 [6.3][6.4]（见第 6 节）。leaf 在 Windows 上的办法更土：向 `1.1.1.1:53` 发起一个 UDP "连接"，读本地地址，据此设置 `OUTBOUND_INTERFACE` 环境变量 [1.14]。

### 3.4 退出与崩溃之后

- **正常退出**：wg-quick 靠 shell `trap`；sing-tun 的 `Close` 删地址 / 路由 / 规则（macOS 逐条发 `RTM_DELETE`，Windows 靠结束会话、关适配器与关 WFP 引擎）；tproxy-config 有幂等的 `_tproxy_remove`（先恢复保存的路由，再删加过的路由、iptables、fwmark 规则、关转发、删 TUN、恢复 `resolv.conf`）。
- **崩溃之后**（谁留下什么，部分是推断）：
  - Linux：TUN 设备随描述符关闭消失，表里指向它的路由随之消失（内核文档："设备和所有相应的路由都会消失"）[2.4]；**`ip rule` 不会消失**，尤其是 `strict_route` 的 `unreachable` 规则会把网络切断；sing-tun 的对策是启动时先删保留窗口里的全部规则 [3.1]；tproxy-config 没有磁盘状态，崩溃后路由、规则、iptables 项和 `resolv.conf` 挂载都留着（源码里还有一条注释说旧版本把 `resolv.conf` 设成 0444，导致容器里下次启动 `EACCES`）[3.6]；`resolvectl` 的逐接口设置随接口消失而丢失，不用恢复 [4.9]。
  - macOS：utun 随描述符关闭消失，其上的路由随接口消失（推断）；**DNS 的 `networksetup` 设置会留下**（wg-quick、tproxy-config 都是要恢复的）；`resolv.conf` 若被改写也留下。
  - Windows：路由 / 地址跟着适配器走；适配器在进程崩溃后是否留下，`wintun.h` 没写（"未说明"），邮件列表里有 2021 年"服务死掉后 Wintun 接口有时起不来、只有重启能恢复"的报告（搜索摘要，原文无法读取），sing-tun 的做法是创建时若报"已存在"就直接打开复用 [2.13]；WFP 动态会话的过滤器随句柄消失；`SetInterfaceDnsSettings` 设在适配器上，适配器没了就没了，设在物理网卡上的则要恢复（tproxy-config 在 Windows 上根本不恢复）。
- **rurge 已有的先例**：`rurge run` 的系统代理管理器（`SystemProxyManager`）已经有"快照 / 备份 / 应用 / 退出与崩溃恢复"，崩溃留下的备份由下一次启动在拿到数据目录实例锁之后恢复，并且在配置读不了时也先恢复（`CLAUDE.md` M4b、M8c 任务 8）。TUN 的路由 / 规则 / DNS 可以照同一个模式做。

### 3.5 对 rurge 的建议

1. **事务日志 + 恢复先行**：每做一步改动之前先把"撤销这一步所需的信息"追加写进数据目录里的日志文件；`rurge run` 在拿到实例锁之后、做任何新改动之前先回放日志撤销上一次留下的东西（幂等：删不存在的东西不算错——Wintun 维护者也是这个态度："failure to remove something that is already removed is not a failure"[7.7]）。这样 Linux 的 `ip rule`、macOS / Windows 的 DNS 设置这些崩溃后留下的东西都有了统一的归宿。
2. **Linux**：用 `rtnetlink`（零 unsafe）；专用路由表 + 自己的保留优先级窗口（照 sing-tun，启动时先清窗口）+ 唯一的 fwmark；rurge 自己的出站用 `set_mark`（需要 `CAP_NET_ADMIN`，`tun-rs` 的 README 推荐的 `setcap cap_net_admin+ep` 正好够；5.17 以前的内核上 `SO_BINDTODEVICE` 可能还要 `CAP_NET_RAW`）；把 TUN 接口的 `rp_filter` 放宽到 2；`strict_route` 的 `unreachable` 规则只在用户要求时加，且必须进日志。
3. **macOS**：保留物理默认路由不动，在 utun 上加 `0.0.0.0/1` + `128.0.0.0/1`（启用 `ipv6-vif` 时再加 `::/1`、`8000::/1`），形式最简单、wg-quick 与 tproxy-config 的经验可用；sing-tun 的八前缀做法没有读到理由，先不跟。出站套接字用 `IP_BOUND_IF`（已有）。DNS 用 SCDynamicStore 的**临时**键（见第 4 节），不用 `networksetup`。
4. **Windows**：`CreateIpForwardEntry2` 加 `0.0.0.0/0`，度量 0，接口度量 0 且关自动度量（WireGuard for Windows 与 sing-tun 都这么做），IPv6 只在 `ipv6-vif` 时加；出站套接字先验证现有"绑源地址"在 TUN 之下是否可靠，不可靠就加 `IP_UNICAST_IF`（一个 unsafe 函数）；需要 DNS 防泄漏时用 WFP 动态会话（崩溃后自动消失）。
5. **包含 / 排除**：照 sing-tun 做 CIDR 求补集（`ipnet` 已在依赖树里，但据我所知它没有集合减法——这一条没有核实——需要自己写一个很小的递归拆分，或用 `prefix-trie`——也已经在工作区依赖里——辅助）；`tun-included-routes` 作为额外的、更具体的路由加到 TUN 上。
6. **IPv6**：Surge 的 `ipv6-vif` 缺省 `disabled`，此时 VIF 不接管 IPv6、也不拦截它（IPv6 流量照常走物理网卡）；sing-tun 的 `strict_route` 则在 TUN 没有 IPv6 地址时给 IPv6 加 `unreachable`（Linux）或 WFP 阻断（Windows）以免泄漏——这是 sing-box 的行为，不是 Surge 的，rurge 若要提供只能是可选项。`ipv6-vif` 为 `auto` / `always` 时才在 TUN 上配 IPv6 地址（NODAD / DAD 关闭，见第 2 节）并装 `::/1` + `8000::/1`（macOS、wg-quick 的写法）或 `::/0`。
7. **监听网络变化后重装 / 重选接口**，见第 6 节。

### 本节来源

- [3.1] sing-tun `tun_linux.go`：https://raw.githubusercontent.com/SagerNet/sing-tun/dev/tun_linux.go
- [3.4] wg-quick（Linux）：https://raw.githubusercontent.com/WireGuard/wireguard-tools/master/src/wg-quick/linux.bash
- [3.5] wg-quick（macOS）：https://raw.githubusercontent.com/WireGuard/wireguard-tools/master/src/wg-quick/darwin.bash
- [3.6] tproxy-config 7.0.9（MIT，2026-09-21）：https://docs.rs/tproxy-config 、https://raw.githubusercontent.com/ssrlive/tproxy-config/master/src/linux.rs 、`macos.rs`、`windows.rs`（同目录）
- [3.7] sing-tun `tun_rules.go`（`BuildAutoRouteRanges`）：https://raw.githubusercontent.com/SagerNet/sing-tun/dev/tun_rules.go ；`tun.go`（Options 的路由字段与网关缺省）同目录
- [3.8] clash-rs：https://raw.githubusercontent.com/Watfaq/clash-rs/master/clash-lib/src/proxy/tun/routes/windows.rs 、`macos.rs`、`linux.rs`（同目录）、https://raw.githubusercontent.com/Watfaq/clash-rs/master/clash-lib/src/app/net/mod.rs
- [3.9] mihomo TUN 文档：https://wiki.metacubex.one/en/config/inbound/tun/
- [3.10] sing 控制包：https://raw.githubusercontent.com/SagerNet/sing/dev/common/control/bind_windows.go 、`bind_darwin.go`、`bind_linux.go`（同目录；默认分支是 `dev`）
- [3.11] 微软 `IPPROTO_IP` / `IPPROTO_IPV6` 套接字选项（页面更新于 2026-05-07）：https://learn.microsoft.com/en-us/windows/win32/winsock/ipproto-ip-socket-options 、https://learn.microsoft.com/en-us/windows/win32/winsock/ipproto-ipv6-socket-options
- [3.12] wireguard-go 的 Windows 绑定：https://raw.githubusercontent.com/WireGuard/wireguard-go/master/conn/bind_windows.go
- [3.13] WireGuard for Windows 的 `pitfalls.go` 与 `tunnel/firewall/` 目录：https://raw.githubusercontent.com/WireGuard/wireguard-windows/master/tunnel/pitfalls.go
- [3.14] 本机 `socket2-0.6.5`（`src/sys/unix.rs`）；`socket(7)`：https://man7.org/linux/man-pages/man7/socket.7.html
- [3.15]（搜索摘要）`IP_BOUND_IF`、`-ifscope` 与作用域默认路由：苹果开发者论坛线程 https://developer.apple.com/forums/thread/736083 、Viscosity 论坛 https://www.sparklabs.com/forum/t/how-does-viscosity-impact-os-x-routing-also-default-route-q/715 、https://djangocas.dev/blog/linux/linux-SO_BINDTODEVICE-and-mac-IP_BOUND_IF-to-bind-socket-to-a-network-interface/ （苹果站点本次无法直接读取）
- [3.16] sing-box 路由文档：https://sing-box.sagernet.org/configuration/route/
- [3.17] Surge 手册：https://manual.nssurge.com/profile/general.html 、https://manual.nssurge.com/features/enhanced-mode.html
- [3.18]（搜索摘要）Windows 强 / 弱主机模型：TechNet Magazine https://learn.microsoft.com/en-us/previous-versions/technet-magazine/cc137807(v=msdn.10) 、The Cable Guy（2005-09）https://technet.microsoft.com/library/bb878108 （原文本次没有直接读到）
- 交叉引用：[1.10]、[1.14]、[2.4]、[2.8]、[2.12]、[2.13]、[4.9]、[6.3]、[6.4]、[7.7] 见各自小节。

---

## 4. DNS 劫持与 fake-IP（问题 4）

### 4.1 DNS 查询怎样到达应答器

- **Surge 的行为**（手册 `dns/advanced.html`）[4.1]：增强模式开着时 Surge 自己跑一个 DNS 应答器——macOS `198.18.0.2`、iOS `198.18.0.4`、IPv6 `fd00:6152::2`；"发往 `198.18.0.2` ～ `198.18.0.9` 的任何查询都视为发给 Surge 的应答器"；缺省**只有发往这些地址的查询**才拿到 fake IP，要让发往别处的查询也被截获，用 `[General] hijack-dns`——每项是 IPv4 地址或 `*`，可带端口（缺省 53），`*:53` 劫持所有 DNS 查询，**只有能解析成合法 DNS 查询的包才被劫持，别的流量原样通过**（项目兼容性清单 6.4 节已抄下这些行为）。A / AAAA 查询立即返回 `198.18.0.0/15` 里的 fake IPv4（IPv6 用 `fd00:6152::` 下的专用前缀）；"经 IPv4 来的 AAAA 查询得到空应答"，这样客户端只拿得到它能路由到 Surge 的 fake 地址；其它类型（TXT、MX 等）转发上游，遵守 `[Host]` 的 `server:` 条目。
- **sing-box**（`dns_mode`）[1.1]：`disabled`（不配平台原生 DNS，不劫持）、`native`（设平台原生的接口 DNS）、`hijack`（原生 + 劫持 53 端口）；开了 `auto_route` 时缺省 `hijack`；Linux 上发往本机接口地址的 DNS 不被劫持，`auto_redirect` 时 53 端口的流量转给 `dns_address`；Windows 上 `strict_route` 会阻断经非 TUN 接口发出的 53 端口流量；`dns_address` 没设时取 `address` 里第一个 IPv4 / IPv6 地址之后的那个地址。路由动作 `hijack-dns` 把 DNS 请求交给 sing-box 的 DNS 模块；`sniff` 动作在没嗅探出协议时按端口兜底：53 → dns、123 → ntp、443 → quic、3478 → stun，嗅探超时缺省 300 ms [4.5]。
- **mihomo**：`dns-hijack` 把匹配的连接重定向到内部 DNS 模块，未写协议时缺省 `udp://`；"在 macOS / Windows 上无法自动劫持发往局域网的 DNS 请求"；Android 开了私人 DNS 时同样不行 [1.10]。
- **应用层面的做法**：leaf 在栈这一层直接截 UDP 53 并用 `generate_fake_response` 应答 [1.14]；tun2proxy 的 `--dns virtual` 检测 53 端口的包，从 `198.18.0.0/15`（`--virtual-dns-pool` 可改）里分配地址并映射到查询的名字 [1.19]。这两个都不开系统监听端口，不会与系统里已有的 53 端口服务冲突（推断）。

### 4.2 各平台怎样让系统 DNS 指向应答器

- **Windows**：`SetInterfaceDnsSettings`（最低 Windows 10 build 19041）设在 TUN 适配器上 [4.12]；WireGuard for Windows 的封装在它返回 `ERROR_PROC_NOT_FOUND` 时回落写注册表，并且**没有用 NRPT**，"域"只当搜索列表用 [2.12]。sing-tun 还调 `DisableDNSRegistration()`（避免把 TUN 地址注册进 DNS）并在设完路由后冲洗解析器缓存 [2.13]。**泄漏问题**：Windows 的"智能多宿主名字解析"会同时从所有接口发查询、取最先返回的（第三方资料；微软组策略项"Turn off smart multi-homed name resolution"对应注册表 `HKLM\Software\Policies\Microsoft\Windows NT\DNSClient\DisableSmartNameResolution`，较新的 Windows 10 / 11 是否还并发有争议）[4.15]；sing-box 与 mihomo 的 `strict_route` 在 Windows 上就是为防这种泄漏，用 WFP 阻断经非 TUN 接口出去的 53 端口流量 [1.1][1.10][2.13]。WireGuard for Windows 全隧道时也用 WFP 防火墙（含只允许隧道 DNS）[3.13]。另一个机制是名字解析策略表（NRPT）：`Add-DnsClientNrptRule -Namespace <域名后缀> -NameServers <地址>` 按命名空间指定 DNS 服务器，不带 `-GpoName` / `-Server` 时规则加在本机客户端，对象是 CIM 类 `DnsClientNrptRule` [4.16]；它按域名后缀分流，能否用来做"全部域名"的兜底这次没有核实，WireGuard for Windows、sing-tun、clash-rs 也都没有用它。
- **macOS**：三种机制。①`networksetup -setdnsservers <服务> <ip>`（逐网络服务，界面可见，**崩溃与重启后仍留着**；wg-quick 与 tproxy-config 都保存原值并在退出时恢复）[3.5][3.6]。②SCDynamicStore：tproxy-config 往 `State:/Network/Service/<id>/DNS` 写 `ServerAddresses`，同时改写 `/etc/resolv.conf` [3.6]；苹果 `configd` 头文件里的 `SCDynamicStoreAddTemporaryValue` 说明是"除非别的会话更新了这个键，键值对会在会话关闭时自动删除"[4.10]——（推断）用它做 DNS 覆盖，进程崩溃后会话关闭，覆盖自动失效；头文件没说"进程退出"是否等同于会话关闭，也没验证 mDNSResponder 是否采纳这样的覆盖，要在真机上验。③`/etc/resolver/<域名>` 文件（`resolver(5)`）：文件名即域名，`nameserver` 最多三个，"Super 客户端"选域名成分匹配最多的那个、没有匹配的才用缺省客户端；`/etc/resolv.conf` 由系统自动维护、"不应手动编辑"[4.11]——它按域名分流，不适合接管全部查询。sing-tun 的 macOS 设备文件里没有 DNS 配置，只有 `dscacheutil -flushcache` [2.8]；tproxy-config 用 `killall -HUP mDNSResponder` 清缓存 [3.6]。
- **Linux**：systemd-resolved 的逐接口设置：`resolvectl dns <接口> <ip>`、`resolvectl domain <接口> '~.'`（仅路由域，让这个接口成为首选）、`resolvectl default-route <接口> yes`、`resolvectl revert <接口>`；手册写明"接口消失时这些设置自动丢失，所以不必显式 revert"[4.9]。sing-tun 正是这么做的（异步调 `resolvectl` 设 `~.`、默认路由与 DNS 服务器，`Close` 时 `resolvectl revert`）[3.1]。没有 resolved 的系统：tproxy-config 把一个临时文件 bind-mount 到 `/etc/resolv.conf` 上（失败就改写原文件并保存原内容）[3.6]；wg-quick 用 `resolvconf -a` [3.4]；tun2proxy 的 README 提醒 NetworkManager 之类可能改写 `/etc/resolv.conf` 造成泄漏 [1.19]。

### 4.3 fake-IP 池

- **Surge**：池是 `198.18.0.0/15` 里的 `198.18.1.1` ～ `198.19.255.254`（项目兼容性清单 6.4 节），共 130,814 个地址（我按首尾地址算的）；映射持久保存；回答 TTL macOS 30 秒、iOS 5 秒；`always-real-ip`（Host List）匹配的域名拿真实 IP，且这类域名的 `[Host]` 映射由应答器直接权威应答，其它域名的 `[Host]` 映射在建立真实连接时才用；预匹配 REJECT 在 DNS 层执行——REJECT 不返回记录、REJECT-DROP 悄悄丢弃查询、REJECT-NO-DROP 回答汇点地址 `198.18.0.244` [4.1]。
- **mihomo 的池**（`component/fakeip/pool.go`，只记事实）[4.3]：顺序分配，一个 `offset` 每次加一；`gateway` 是网络地址 + 1，`first` 是网络地址 + 4（注释"default start with 198.18.0.4"），最后一个地址（广播）也不发；`offset` 到 `last` 就回绕到 `first` 并置 `cycle = true`；回绕之后（或者目标地址已被映射时）删掉旧映射、把地址重新分配给新主机——因为顺序分配，这就等于复用最早分配的地址；主机名按 RFC 4343 小写化；映射存两份（主机 → IP、IP → 主机）；存储是内存或 `cachefile`（持久化时），持久化的状态是 `key-offset-fake-ip` 与 `key-cycle-fake-ip` 两个键；恢复时若保存的偏移量落在前缀之外、或没有保存偏移量而 `first` 已被占用，就清空重来；`fake-ip-filter` 不在池里实现（只跳过网关与广播）。配置层面：`fake-ip-filter-mode` 有 `blacklist` / `whitelist` / `rule`，`fake-ip-ttl` 可配、文档没给缺省值，`enhanced-mode` 缺省 `redir-host` [4.4]。
- **sing-box（旧写法）**：`inet4_range` 示例 `198.18.0.0/15`，`inet6_range` 示例 `fc00::/18`；这种写法 1.12.0 弃用、1.14.0 移除，页面没有讲持久化 [4.5]。
- **leaf**：目的是 fake IP 但查不到域名时，端口不是 80 / 443 就丢弃，是 80 / 443 就放行让分发器嗅探域名 [1.14]。
- （推断）池约 13 万个地址、TTL 30 秒，回绕本身要很长时间；真正的风险是**长连接占着的地址被回绕复用**，那时同一个 fake IP 对应了两个域名，新连接会被按旧域名分流。mihomo 的做法（回绕就删旧映射）没有这个保护。

### 4.4 浏览器的 DoH、canary 域、SVCB / HTTPS 记录

- **Firefox canary 域** `use-application-dns.net`（搜索摘要，Mozilla 支持文章 https://support.mozilla.org/en-US/kb/canary-domain-use-application-dnsnet 本次读不到）[4.8]：Firefox 经**操作系统配置的 DNS 服务器**解析这个名字；返回码不是 `NOERROR`（如 `NXDOMAIN`、`SERVFAIL`），或者是 `NOERROR` 但既没有 A 也没有 AAAA，都算"负面"，Firefox 因此关掉应用层 DNS（DoH）；**只对 DoH 是缺省开启的用户有效，不管用户自己打开 DoH 的人**；文章称这是在互联网标准机构定出内容过滤信号方法之前的限期措施。Surge 手册："应答器对 `use-application-dns.net` 回 NXDOMAIN"[4.1]。Mozilla wiki 的 TRR 模式：0 关（缺省）、2 先 TRR 后回落、3 只用 TRR（仍用原生 DNS 做门户检测与遥测）、5 用户选择关闭 [4.8]。
- **Chrome**：Chromium 文档说它的"自动升级"是升级到**当前 DNS 服务商的** DoH 服务器，名单在源码 `doh_provider_entry.cc`，"不改变 DNS 服务商"，并且"没有计划支持"Mozilla 的 canary 域做法 [4.13]。（推断）系统 DNS 是 `198.18.0.2` 这种不在名单里的地址时不会自动升级；但用户手动配了 Secure DNS 模板、或 Firefox 设成 TRR only 时，查询直接走 443 端口的 HTTPS，DNS 劫持截不到，这时能依赖的只有协议嗅探（SNI / Host）与"把已知 DoH 端点按规则拒绝"。
- **SVCB / HTTPS（type 65）**：Surge 缺省（`allow-dns-svcb = false`）拒绝这类查询，手册的理由是"SVCB 应答可能带 IP 提示，从而绕开 fake-IP 机制"，迫使系统改查 A 记录；项目兼容性清单 6.4 节写的是"返回 Not Implemented"[4.1]。RFC 9460 §7.3：`ipv4hint` / `ipv6hint` 只是提示，客户端本地已有 A / AAAA 记录时"应当忽略这些提示"；HTTPS 记录类型号 65；`ech` 的 SvcParamKey 号 5 在这份 RFC 里只是保留（格式不在其中）[4.14]。（推断）拒绝 type 65 的副作用是浏览器也拿不到经 DNS 下发的 ECH 配置。

### 4.5 对 rurge 的建议

1. 应答器做在 TUN 栈这一层（截 UDP / TCP 53 的包），不开系统监听端口；监听地址 `198.18.0.2` 与 `fd00:6152::2`，`198.18.0.2` ～ `198.18.0.9` 都当发给应答器（照 Surge，FR-DNS-08）。
2. 池：顺序环 + 持久化的（域名 → 地址）表与下一个偏移量，写进 `state.json`；恢复时偏移量越界就清空（mihomo）；键小写；**回绕时跳过仍有存活会话或映射年龄不足"TTL + 最长空闲"的地址**（我的补充，上面 4.3 末尾说的风险）。
3. 答复规则：A → fake（`always-real-ip` / `[Host]` / 预匹配 REJECT 另论）；AAAA → 空应答（`ipv6-vif` 关闭时）；type 65 → NOTIMP（`allow-dns-svcb` 关闭时）；canary 域 → NXDOMAIN；TTL 30 秒；非 A / AAAA 转发上游。
4. 无映射的 fake IP 连接（重启后、映射被回收）：先靠持久化避免，再靠 SNI / Host 嗅探补救，否则 RST——leaf 的"80 / 443 放行嗅探、其它丢弃"是一个现成的先例。
5. 系统 DNS 设置：Windows `SetInterfaceDnsSettings`（设在 TUN 适配器，度量低于物理网卡）+ 严格模式时 WFP 动态会话阻断其它接口的 53 端口；Linux `resolvectl`（崩溃后自动失效）；macOS 用 SCDynamicStore **临时**键（崩溃后自动失效，但要真机验证），不用 `networksetup`；凡是崩溃后不会自动失效的，都进第 3.5 节建议的事务日志。
6. DoH 绕过不要追求"全拦"：只做 canary 域、type 65 拒绝、SNI 嗅探，其余在文档里说明（Chrome 手动配置的 Secure DNS 与 Firefox TRR only 拦不住）。

### 本节来源

- [4.1] Surge 手册：https://manual.nssurge.com/dns/advanced.html 、https://manual.nssurge.com/features/enhanced-mode.html 、https://manual.nssurge.com/profile/general.html
- [4.3] mihomo `component/fakeip/pool.go`（Alpha 分支，GPL，只记事实）：https://raw.githubusercontent.com/MetaCubeX/mihomo/Alpha/component/fakeip/pool.go
- [4.4] mihomo DNS 文档：https://wiki.metacubex.one/en/config/dns/
- [4.5] sing-box：https://sing-box.sagernet.org/configuration/dns/fakeip/ 、https://sing-box.sagernet.org/configuration/route/rule_action/
- [4.8]（搜索摘要）Mozilla canary 域；TRR 模式 https://wiki.mozilla.org/Trusted_Recursive_Resolver
- [4.9] `resolvectl(1)`：https://man7.org/linux/man-pages/man1/resolvectl.1.html
- [4.10] `SCDynamicStore.h`（苹果开源 configd）：https://raw.githubusercontent.com/apple-oss-distributions/configd/main/SystemConfiguration.fproj/SCDynamicStore.h
- [4.11] `resolver(5)`（macOS）：https://www.manpagez.com/man/5/resolver/
- [4.12] 微软 `SetInterfaceDnsSettings`：https://learn.microsoft.com/en-us/windows/win32/api/netioapi/nf-netioapi-setinterfacednssettings
- [4.13] Chromium DoH：https://www.chromium.org/developers/dns-over-https/
- [4.14] RFC 9460：https://www.rfc-editor.org/rfc/rfc9460.html
- [4.15]（搜索摘要）智能多宿主名字解析：https://www.sans.org/reading-room/whitepapers/dns/preventing-windows-10-smhnr-dns-leakage-40165 、https://gp.kapilarya.com/turn-off-smart-multi-homed-name-resolution 、https://docs.fortinet.com/document/forticlient/7.2.6/administration-guide/329321 （微软的原始文档本次没有读到）
- [4.16] 微软 `Add-DnsClientNrptRule`：https://learn.microsoft.com/en-us/powershell/module/dnsclient/add-dnsclientnrptrule
- 交叉引用：[1.1]、[1.10]、[1.14]、[1.19]、[2.8]、[2.12]、[2.13]、[3.1]、[3.4]、[3.5]、[3.6]、[3.13]、[3.17]。

---

## 5. 进程识别（问题 5）

目标：给 `PROCESS-NAME` 规则（匹配文件名、以 `/` 开头的全路径、以 `/` 结尾的路径前缀，区分大小写，项目兼容性清单 3.1 节）与脚本的 `$request.processPath` 提供"发起连接的进程"的路径与名字。

### 5.1 各平台的办法与代价

**Windows**

- 文档化的接口：`GetExtendedTcpTable`（`TCP_TABLE_OWNER_PID_ALL` / `_CONNECTIONS` / `_LISTENER` 与带 `OWNER_MODULE` 的变体，IPv4 与 IPv6 各一种表结构）和 `GetExtendedUdpTable`；两次调用的写法——第一次拿所需大小（`ERROR_INSUFFICIENT_BUFFER`），第二次取表，表在两次调用之间可能变大，要循环重试；`bOrder` 可要求按本地地址排序；`TCP_TABLE_BASIC_*` 不支持 IPv6 [5.1]。行里的端口放在低 16 位、网络字节序（mihomo 用 `Ntohs` 读）[5.4]。
- PID → 路径：`OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `QueryFullProcessImageNameW`；PID 0 与 4 是 `:System Idle Process` / `:System` 这样的哨兵，不是文件路径；`svchost` 之类的服务宿主，sing-box 用"所有者模块"表项里的服务标签，再去注册表 `...\Services\<名>\Parameters` 或服务键本身读 `ServiceDll` [5.3]。
- **mihomo**：TCP 用表类 4（`TCP_TABLE_OWNER_PID_CONNECTIONS`）、UDP 用类 1（`UDP_TABLE_OWNER_PID`）；缓冲从 8 字节起、不够就按返回大小重分配；TCP 只看状态 5（ESTABLISHED）的行；UDP 的本地地址是未指定地址（`0.0.0.0` / `::`）也算匹配（对应未绑定 UDP 套接字的第一次 `sendto`）；**每次查找都重新取整张表，不缓存**，只缓存函数地址 [5.4]。
- **sing-box**：先试一个"带键"的 TCP 查询（`nsiGetTCPConnection(源, 目的)`，来自它的 `winiphlpapi` 辅助包里的 `NsiGetParameter`——函数名与查找符号像是 `nsi.dll` 的**未文档化**接口，源码摘要里没展开，这是推断），失败或不是 TCP 再回落到全表扫描（owner-module 类的表）；TCP 与 UDP 都只比本地端点；UDP 的"本地地址相同"和"未指定地址"两个条件是 OR，所以前面的通配行可能压过后面的精确行；不缓存 [5.3]。
- 代价：每次 O(表大小)（几千行的量级，推断）；NSI 路径是 O(1) 但未文档化。

**Linux**

- **sock_diag（netlink）**：`socket(AF_NETLINK, SOCK_RAW 或 SOCK_DGRAM, NETLINK_SOCK_DIAG)`，`nlmsg_type = SOCK_DIAG_BY_FAMILY`（20），`NLM_F_DUMP` 表示列表、否则查单个；`struct inet_diag_req_v2`：`sdiag_family`（`AF_INET` / `AF_INET6`）、`sdiag_protocol`（`IPPROTO_TCP` / `_UDP` / `_UDPLITE`）、`idiag_ext`、`idiag_states`（只对列表请求有意义）、四元组与 `idiag_cookie`（单个查询时用它命名套接字，列表请求忽略）；应答里有 `idiag_uid` 与 `idiag_inode`（`sock_diag(7)`）[5.2]。sing-box 的实现细节（读自源码摘要）[5.3]：请求总长 72 字节（16 字节头 + 56 字节体）；精确查询填满四元组，UDP 的精确查询要把源与目的**对调**（注释："udp_dump_one expects the exact-match endpoints reversed for historical reasons"）；没有目的地址（未连接 UDP）就回落到 dump——内核只按端口过滤，地址要在客户端比；应答体里 uid 在偏移 64、inode 在偏移 68；每个（族，协议）复用一个 netlink 套接字，收发超时 100 ms，出错重试一次；`ENOENT` / `ESRCH` 当作"没找到"；双栈套接字携带 IPv4 映射地址时只在 `AF_INET6` 下列出。
- **inode → 进程**：扫 `/proc/<pid>/fd/*`，找 `socket:[inode]` 的链接，再读 `/proc/<pid>/exe`。sing-box 只存"inode → 路径"，按 UID 分块缓存（64 项、1 秒），先在套接字的 UID 范围里找、找不到再扫全部用户（"套接字保留创建时的 uid，而 `/proc` 反映进程当前的 uid"）；对 `NotExist` 与权限错误跳过，其它读 `exe` 的错误让整次扫描失败；缓存命中但缺这个 inode 时仍重扫 [5.3]。mihomo：netlink 之后同样扫 `/proc/<pid>/fd`，**不缓存**，`/proc/net/{tcp,tcp6,udp,udp6}` 解析只在 Android 上作后备 [5.4]。
- 代价：扫描是 O(进程数 × 每进程描述符数)，是主要开销（推断：桌面上几百个进程、上万个描述符，量级在几十毫秒，没有实测）；sock_diag 精确查询本身是微秒级（推断）。

**macOS**

- **sysctl `net.inet.tcp.pcblist_n` / `net.inet.udp.pcblist_n`**（sing-box 与 mihomo 都用）[5.3][5.4]：整张 PCB 表一次读出；先跳过 24 字节的 `xinpgen`；每条记录以 inpcb 开头：外端口在 +16、本地端口在 +18（大端 `u16`），`vflag` 在 +44（`0x1` IPv4、`0x2` IPv6），外地址在 +48、本地地址在 +64（IPv4 取每个 16 字节里的 +12，即 +60 / +76），`xsocket` 从 +104 开始，其中 `so_uid` 在 +64、`so_last_pid` 在 +68、`e_pid`（有效 pid）在 +72（本机字节序）。**记录大小随 Darwin 版本变**：mihomo 读 `kern.osrelease`，主版本 ≥ 22 用 408 字节，否则 384 字节，TCP 记录再加 208 字节；sing-box 用"结构大小 + 208"（常量定义在别处）。路径：sing-box 与 mihomo 都用裸 `SYS_PROC_INFO`（调用号 2，flavor `0xb` 即 `PROC_PIDPATHINFO`）读进 1024 字节缓冲，不经 libproc。
- sing-box 的缓存：每个网络名一份快照，200 ms；未命中、非精确匹配（如 UDP 回落）或取路径失败时强制刷新一次再试；快照读取时持锁；`so_last_pid` 与 `e_pid` 不同时用有效 pid，路径列表是 `[有效进程路径, 最后使用者路径]` [5.3]。
- **libproc 路线**（稳定、文档化，但慢）：`proc_listpids` → 对每个 pid `proc_pidinfo(PROC_PIDLISTFDS)` → 对每个套接字描述符 `proc_pidfdinfo(PROC_PIDFDSOCKETINFO)`，读 `socket_fdinfo` 里的 `in_sockinfo` / `tcp_sockinfo` 得到本地 / 外部地址端口。本机 `libc-0.2.189` 的 Apple 部分有 `proc_listpids`、`proc_pidinfo`、`proc_pidfdinfo`、`proc_pidpath`、`proc_name`、`sysctlbyname` 与 `proc_fdinfo`、`PROC_PIDLISTFDS`，**但没有** `socket_fdinfo` / `in_sockinfo` / `PROC_PIDFDSOCKETINFO`（要自己写 `repr(C)` 结构）[5.7]。`libproc` crate（0.14.11，MIT，`rust-version` 1.85）有 `net_info` 模块（`SocketFDInfo`、`TcpSockInfo`、`InSockInfo`……），但它的构建依赖 `bindgen` 0.73.2（带 `runtime` 特性，需要 libclang）[5.6]。
- 权限与陷阱：`so_last_pid` 是"最后使用这个套接字的进程"，不一定是创建者；偏移与记录大小是硬编码，OS 升级可能变；解析代码信任缓冲长度，缓冲损坏会 panic（sing-box 摘要里的评语）。

**clash-rs**：用的是 Watfaq 自己的 `sock2proc`（git 依赖，0.1.0）；从它的 `Cargo.toml` 看，Linux 用 `rtnetlink` + `netlink-packet-sock-diag`，macOS 用 `sysctl` crate，Windows 用 `windows-sys` 的 `Win32_NetworkManagement_IpHelper`（实现与上面 sing-box / mihomo 同类，是推断）；`Cargo.toml` 与 GitHub 元数据里**许可都为空**、0 星、最后推送 2025-11-06——不能用于 rurge [1.16][5.5]。

### 5.2 现成的 Rust crate

| crate | 版本 | 许可 | 说明 |
| --- | --- | --- | --- |
| `netstat2` | 0.11.2 | MIT OR Apache-2.0 | `get_sockets_info` / `iterate_sockets_info`（带 PID）；Linux 经 `netlink-packet-sock-diag`；其它平台的实现从依赖（`bindgen` 0.72 是构建依赖）看是 C 绑定（推断）；README 页面没读到，仓库路径未核实 [5.5] |
| `listeners` | 0.6 | MIT | **只列出监听端口**的进程，不能按已连接套接字的四元组查 [5.5] |
| `libproc` | 0.14.11 | MIT | 见上，构建需要 libclang [5.6] |
| `sock2proc` | git | 无 | 不可用 |

### 5.3 陷阱

1. **短命连接**：TCP 的 `connect` 一调用套接字就进了表（`SYN_SENT`），所以在**第一个 SYN 到达 TUN 时**立刻查最可靠（推断）；晚到连接结束之后，进程可能已退出，PID 查不到路径。查到之后按流缓存到流结束。
2. **UDP**：表里只有本地端点；未连接套接字的第一次 `sendto` 才隐式绑定端口，表可能还没更新；多个进程共用端口（`SO_REUSEADDR`、多播）时有歧义；Windows 与 macOS 的通配地址要特殊匹配（上面各平台已写）。
3. **看不见的来源**：来自虚拟机、WSL2、容器（独立网络命名空间）或经过 NAT 转发的流量在本机没有对应的套接字，只能是"未知"；rurge 自己的出站连接也要识别并排除（防环的另一面）。
4. **本地代理端口**：HTTP / SOCKS5 入站的连接，对端是回环地址，查表时要用客户端一侧的本地端口（Windows 回环 TCP 在表里有两行）。
5. **IPv4 映射地址**（`::ffff:a.b.c.d`）要先还原再比，sing-box 与 mihomo 都做了。

### 5.4 对 rurge 的建议

1. 在 `rurge-platform` 里放一个 `ProcessResolver`，签名大致是"（协议，本地端点，可选远端端点）→ `Option<ProcessInfo { pid, path, name, uid }>`"，同步实现、由调用方放进 `spawn_blocking`；上面加"同键并发合并"（singleflight）和缓存：命中结果按流缓存到流结束；未命中的负结果缓存 1 ～ 2 秒，并限制刷新频率（例如同一网络协议 100 ms 内至多全表刷新一次）。
2. **Windows**：`GetExtendedTcpTable(TCP_TABLE_OWNER_PID_CONNECTIONS)` / `GetExtendedUdpTable(UDP_TABLE_OWNER_PID)` + `OpenProcess` + `QueryFullProcessImageNameW`；`windows-sys` 已在依赖树里，要加 `Win32_NetworkManagement_IpHelper`、`Win32_Networking_WinSock`、`Win32_System_Threading` 特性；（推断）约 5 个 FFI 调用点，缓冲解析全用安全的切片代码；不碰未文档化的 NSI。
3. **Linux**：sock_diag 精确查询——用已在依赖树里的 `nix`（`socket` 特性；本机 0.31.3 有 `AddressFamily::Netlink` 与 `SockProtocol::NetlinkSockDiag` / `NetlinkRoute`，已核对）开 netlink 套接字、手拼 72 字节请求、解析应答里的 uid / inode，零 unsafe；只在未命中时扫 `/proc`，并仿 sing-box 做 1 秒的（UID → inode → 路径）缓存与"先本 UID 后全部"的两级扫描。
4. **macOS**：推荐 sysctl `pcblist_n` 路线（一次 `sysctlbyname` 一个 unsafe 调用点，其余是安全的字节解析；`proc_pidpath` 是 `libc` 里已有的另一个调用点），但**必须按 `kern.osrelease` 选记录大小并在解析前校验长度**（mihomo 的做法），失败则回落到 libproc 路线（需要自己定义 `socket_fdinfo` 相关结构）。两者都放在手工验收清单里（每次大版本的 macOS 都要验）。
5. 不引入 `libproc` / `netstat2`（构建期 `bindgen` + libclang，在 Windows 与 CI 上增加工具链负担），不用 `sock2proc`。

### 本节来源

- [5.1] 微软 `GetExtendedTcpTable`：https://learn.microsoft.com/en-us/windows/win32/api/iphlpapi/nf-iphlpapi-getextendedtcptable
- [5.2] `sock_diag(7)`：https://man7.org/linux/man-pages/man7/sock_diag.7.html
- [5.3] sing-box `common/process/`（默认分支 `testing`，GPL，只记事实）：https://raw.githubusercontent.com/SagerNet/sing-box/testing/common/process/searcher_linux.go 、`socket_diag_linux.go`、`searcher_windows.go`、`searcher_darwin_shared.go`（同目录）；Windows 辅助包 https://raw.githubusercontent.com/SagerNet/sing/dev/common/winiphlpapi/helper.go 、`iphlpapi.go`
- [5.4] mihomo `component/process/`（Alpha 分支，GPL，只记事实）：https://raw.githubusercontent.com/MetaCubeX/mihomo/Alpha/component/process/process_linux.go 、`process_darwin.go`、`process_windows.go`
- [5.5] `sock2proc` 元数据 https://api.github.com/repos/Watfaq/sock2proc 、https://raw.githubusercontent.com/Watfaq/sock2proc/master/Cargo.toml ；`netstat2` https://docs.rs/netstat2/latest/netstat2/ ；`listeners` https://github.com/GyulyVGC/listeners
- [5.6] `libproc`：https://docs.rs/libproc/latest/libproc/ 、https://raw.githubusercontent.com/andrewdavidmackenzie/libproc-rs/master/Cargo.toml 、https://docs.rs/libproc/latest/libproc/net_info/index.html
- [5.7] 本机 `libc-0.2.189`（`src/unix/bsd/apple/mod.rs`）
- 本仓库：`docs/surge-compatibility-matrix.md`（`PROCESS-NAME` 行、`$request.processPath`）

---

## 6. 网络环境探测与网络变化事件（问题 6）

对应 FR-IN-09：当前 SSID / BSSID / 默认网关 / 网络类型，和"网络变化事件"（触发缓存清理、重测、`network-changed` 脚本、WireGuard 重建）。

### 6.1 变化事件

| 平台 | 事件源 | 细节 |
| --- | --- | --- |
| Windows | `NotifyIpInterfaceChange(Family, Callback, Context, InitialNotification, &Handle)`，另有路由与单播地址的同类函数（本次只读了前一个的文档） | 回调是**串行**的；回调收到的 `MIB_IPINTERFACE_ROW` 内容不全，要用 `Family` / `InterfaceLuid` / `InterfaceIndex` 再调 `GetIpInterfaceEntry` 取完整信息；`InitialNotification = TRUE` 时注册完成后立刻回调一次（`NotificationType` 为 `MibInitialNotification`、`Row` 为 NULL）；**不能在回调线程里调 `CancelMibChangeNotify2`**（会死锁），要在别的线程取消；进程结束时系统自动注销；注册不跨关机 / 重启 [6.1]。sing-tun 的 Windows 监视器用 `winipcfg` 的路由变化与接口变化回调（封装，源码里没点名底层函数），外加一个注册表监视器（`RegNotifyChangeKeyValue`）盯 `Tcpip` / `Tcpip6` 的 `Parameters\Interfaces`（DNS 变化），回调里直接 `emit()`，**没有去抖**；每次通知都重新读 `GetIPForwardTable2(AF_INET)` 做完整比较 [6.3]。另一类事件源是网络列表管理器（COM）：`INetworkListManagerEvents::ConnectivityChanged(NLM_CONNECTIVITY)` 报整机的连通性变化，`INetworkConnectionEvents::NetworkConnectionConnectivityChanged(连接 GUID, NLM_CONNECTIVITY)` 报某个网络连接的变化；Vista / Server 2008 起可用，头文件 `netlistmgr.h`（搜索摘要）[6.13]——它报的是"连通性分类"，不是路由 / 地址的变化，要靠 COM 连接点注册（`windows` 0.62.2 里有对应绑定，推断）。 |
| Linux | `NETLINK_ROUTE` 套接字绑定多播组 `RTMGRP_LINK`、`RTMGRP_IPV4_IFADDR`、`RTMGRP_IPV6_IFADDR`、`RTMGRP_IPV4_ROUTE`、`RTMGRP_IPV6_ROUTE` | sing-tun：1 MiB 接收缓冲（`SO_RCVBUFFORCE`，不行就 `SO_RCVBUF`）；`ENOBUFS` 忽略；更新通道容量 1，突发自然合并；**1 秒的"首发 + 尾发"去抖**（第一个更新立刻发出并启 1 秒定时器，窗口内再来的置 `pending`，定时器到点若 `pending` 就再发一次）[6.5]。Rust：`rtnetlink` 0.23.0（MIT，2026-08-18）提供多播连接辅助函数，行为同 `ip monitor` [6.8]；`if-watch` 3.2.2（MIT OR Apache-2.0，2026-09-15）只给**地址**的上 / 下事件（`IfEvent`），不含路由与默认网关变化 [6.9] |
| macOS | `AF_ROUTE` 原始套接字（任何 `RouteMessage` 都触发，每次发出后关掉重开）；DNS 变化另由 `dnsinfo` 监视器负责 | sing-tun [6.4]；也可用 SCDynamicStore 订阅 `State:/Network/Global/IPv4` 之类的键（`system-configuration` 0.8.0，MIT OR Apache-2.0，2026-10-15；有 `dynamic_store`、`network_reachability` 模块 [6.10]）；sing-tun 也没有去抖 |

### 6.2 默认网关与默认接口

- sing-tun 的做法：Windows 取 `GetIPForwardTable2(AF_INET)` 里前缀长度为 0 的路由，跳过未启用 / 虚拟或软件回环类型 / IPv4 接口数据里未标"已连接"的接口，按（路由度量 + 接口度量）最小者；**不显式排除自己建的 TUN**，只靠"虚拟类型"过滤；变化时只在"接口变了或 DNS 服务器列表变了"才发事件 [6.3]。macOS 取路由表 `FetchRIB` 里第一条目的为 `0.0.0.0/0`、带 `RTF_UP | RTF_GATEWAY` 的 IPv4 默认路由（`RTF_IFSCOPE` 检查被注释掉了）；在 Network Extension 里则向 `10.255.255.255:80` 发起 TCP 连接、读内核选的本地地址（最多等 1 秒）[6.4]。
- clash-rs 按网卡名字优先级猜，名字含 `tun` 的排除，不缓存 [3.8]——不可取。
- `netdev` 0.46.3（MIT，2026-09-13，owner shellrow）：`get_default_interface()` / `get_default_gateway()`（在缺省开启的 `gateway` 特性后面）；从依赖看 Linux 用 netlink crate、macOS 用 objc2 与 SystemConfiguration 绑定、Windows 用 windows-sys（页面没写实现，是推断）；未说明 MSRV [6.9]。
- **网络类型**（Wi-Fi / 有线 / 蜂窝）：这次没有读到专门的资料，以下全是推断。Windows 看接口的 `IfType`（IANA 登记：以太网 6、IEEE 802.11 为 71、移动宽带 243 / 244），来自 `GetAdaptersAddresses` 或 `GetIfEntry2`；sing-tun 的监视器已经用接口类型过滤掉虚拟 / 软件回环接口 [6.3]。Linux 看 `/sys/class/net/<接口>/wireless` 目录是否存在（或 `uevent` 里的 `DEVTYPE=wlan`）。macOS 看 SCNetworkInterface 的类型（`kSCNetworkInterfaceTypeIEEE80211` 等），`system-configuration` 的 `network_configuration` 模块对应 `SCNetworkConfiguration` [6.10]。
- **TUN 成为默认路由之后**，"默认接口"必须是**物理**接口：要么在选择时排除自己的 TUN（按接口号 / 名字），要么像 Surge 那样直接记住 TUN 装上去之前的结果并随事件更新。

### 6.3 SSID / BSSID

**Windows**

- 微软文档《Changes to API behavior for Wi-Fi access and location》（页面更新于 2025-06-06）：涉及 BSSID 的 `wlanapi.h` 接口将受"精确位置"权限限制；用户没授权时下列函数返回 `ERROR_ACCESS_DENIED`：`WlanGetAvailableNetworkList`、`WlanGetNetworkBssList`、`WlanQueryInterface`（当 `OpCode` 是 `wlan_intf_opcode_current_connection` 时——这正是取当前连接 SSID / BSSID 的那个调用）、`WlanScan`；`WlanRegisterNotification` 带 `WLAN_NOTIFICATION_SOURCE_MSM` 需要 `wiFiControl` 设备能力，否则也是 `ERROR_ACCESS_DENIED`。**首次被拦时只有"以用户上下文运行、且不在 `C:\Windows\System32` 下"的进程才会弹出一次性提示**；文档建议：只要 SSID 就用 WinRT 的 `WlanConnectionProfileDetails.GetConnectedSsid`，用 `AppCapability.CheckAccess`（`wiFiControl`）查权限状态，提示用户去 `ms-settings:privacy-location`。文档写的是计划（"将"），测试要用 Canary 25976 以上，落地在"2024 年秋季的功能更新"（即哪个版本、实际行为如何，需要在真机上核实）[6.2]。
- 含义（推断）：以 SYSTEM 服务运行的 rurge 拿不到提示、很可能一直拿不到 SSID；以用户身份运行的前台进程可能弹一次提示。

**Linux**

- NetworkManager 的 D-Bus 接口：设备对象 `org.freedesktop.NetworkManager.Device.Wireless` 的 `ActiveAccessPoint`（`o`，"设备当前使用的接入点的对象路径"），接入点对象的 `Ssid`（`ay`）与 `HwAddress`（`s`，即 BSSID）[6.6]。Rust 侧可用 `zbus` 5.19.0（MIT，2026-10-14，可选 tokio 运行时）[6.12]。
- 没有 NetworkManager 时用 nl80211：`NL80211_CMD_GET_INTERFACE`（可 dump 全部或按接口号取一个）；`NL80211_ATTR_SSID`（二进制属性，0 ～ 32 个八位组）[6.7]；`wl-nl80211` 0.8.0（MIT，2026-10-01）有 `Nl80211InterfaceGetRequest`、`Nl80211StationGetRequest`、`Nl80211ScanGetRequest` 与 BSS 相关类型，页面没写 SSID / BSSID 的取法，维护状态与 MSRV 未写 [6.8]。（推断）BSSID 通常要从站点或扫描结果里取已关联的那一项。

**macOS**

- （搜索摘要，苹果站点本次无法直接读取）苹果开发者论坛上 DTS 工程师确认：macOS 14（Sonoma）起读 SSID 需要位置服务授权，是**有意的改动**；`CWInterface.h` 里写"除非位置服务开着且用户已授权调用方，否则拿不到 SSID 信息"；命令行工具与 launchd 守护进程的表现不稳定（有人在 launchd 管理下行为不一致、用 `SMAppService` 装守护进程也没解决）；Sequoia 15.1 上有"授权了也拿到空 SSID / BSSID"的报告；论坛里的实际建议是把代码包进带 `Info.plist` 位置用途说明的 App 包里 [6.11]。
- （搜索摘要，Jamf 社区）macOS 14.4 起 `airport` 工具不再给出当前 Wi-Fi 信息；14.5 起 `wdutil info` 对 MAC 地址、SSID、BSSID 返回 "redacted"；`networksetup -getairportnetwork` 在 macOS 15 上返回"未关联"（有人说是 Apple 为堵位置隐私绕过而关掉了）；有人报告 `ipconfig getsummary en0` 配合 awk 可取到 SSID（未核实）[6.11]。
- Rust：`objc2-core-wlan` 0.3.2（Zlib OR Apache-2.0 OR MIT，2026-10-10）有 `CWInterface` / `CWWiFiClient`，页面没列 `ssid()` / `bssid()`，也没说是否标 `unsafe` [6.12]。
- 含义（推断）：rurge 作为命令行守护进程在 macOS 上很可能拿不到 SSID，除非另有一个带位置权限的 App 包（辅助程序）；这是个产品形态问题（和 Surge Mac 是 App 的前提不同）。

### 6.4 对 rurge 的建议

1. 抽一个 `NetworkWatcher`：对外只发"网络变了"这一个事件（带最新的接口 / 网关 / DNS 快照），内部按平台选事件源，**统一去抖成 1 秒的首发 + 尾发**（sing-tun 的 Linux 做法；它的 Windows 与 macOS 监视器没有去抖，每次通知都全表重算，那是已知的粗糙处）。Windows 的 `NotifyIpInterfaceChange` / 路由变化回调是 unsafe 的 FFI 注册（约 3 个调用点，推断）；Linux 用 `rtnetlink` 多播；macOS 先用 `AF_ROUTE` 套接字（本机 `nix-0.31.3` 有 `AddressFamily::Route`，已核对，开套接字不需要 unsafe）或 `system-configuration`。
2. 默认接口 / 网关：用 `netdev` 或自己读（Windows `GetIpForwardTable2`、Linux 路由表 dump、macOS `FetchRIB`），**排除自己的 TUN**；结果要供第 3 节的防环绑定使用（现有 `socket.rs` 里的接口表缓存是 5 秒）。验收要求"网络切换后 30 秒内恢复"（阶段 3），去抖 1 秒、重装路由的耗时都在预算之内。
3. SSID / BSSID：**把"不可用"设计成一等状态**——API 与 `[SSID Setting]` 匹配要能区分"没连 Wi-Fi""没有权限""平台不支持"，并在日志里给出明确提示；Windows 优先考虑 WinRT 的 `GetConnectedSsid`（要验证是否也受位置权限限制）；Linux 先做 NetworkManager（D-Bus），再做 nl80211；macOS 放到手工验收与"需要位置授权"的说明里，不要承诺命令行守护进程能拿到。

### 本节来源

- [6.1] 微软 `NotifyIpInterfaceChange`：https://learn.microsoft.com/en-us/windows/win32/api/netioapi/nf-netioapi-notifyipinterfacechange
- [6.2] 微软 Wi-Fi 与位置权限变化：https://learn.microsoft.com/en-us/windows/win32/nativewifi/wi-fi-access-location-changes
- [6.3] sing-tun `monitor_windows.go`：https://raw.githubusercontent.com/SagerNet/sing-tun/dev/monitor_windows.go
- [6.4] sing-tun `monitor_darwin.go`：https://raw.githubusercontent.com/SagerNet/sing-tun/dev/monitor_darwin.go
- [6.5] sing-tun `monitor_linux.go`：https://raw.githubusercontent.com/SagerNet/sing-tun/dev/monitor_linux.go
- [6.6] NetworkManager D-Bus 文档：https://networkmanager.dev/docs/api/latest/gdbus-org.freedesktop.NetworkManager.AccessPoint.html 、https://networkmanager.dev/docs/api/latest/gdbus-org.freedesktop.NetworkManager.Device.Wireless.html
- [6.7] `nl80211.h`：https://raw.githubusercontent.com/torvalds/linux/master/include/uapi/linux/nl80211.h
- [6.8] `rtnetlink`：https://docs.rs/rtnetlink/latest/rtnetlink/ ；`wl-nl80211`：https://docs.rs/wl-nl80211/latest/wl_nl80211/
- [6.9] `if-watch`：https://docs.rs/if-watch/latest/if_watch/ ；`netdev`：https://docs.rs/netdev/latest/netdev/
- [6.10] `system-configuration`：https://docs.rs/system-configuration/latest/system_configuration/
- [6.11]（搜索摘要）macOS SSID：苹果开发者论坛 https://developer.apple.com/forums/thread/732431 、https://developer.apple.com/forums/thread/737455 ；Jamf 社区 https://community.jamf.com/general-discussions-2/collecting-active-ssid-with-macos-sonoma-14-4-and-later-32298 （苹果站点本次无法直接读取）
- [6.12] `objc2-core-wlan`：https://docs.rs/objc2-core-wlan/latest/objc2_core_wlan/ ；`zbus`：https://docs.rs/zbus/latest/zbus/
- [6.13]（搜索摘要）网络列表管理器事件：https://learn.microsoft.com/en-us/windows/win32/api/netlistmgr/nn-netlistmgr-inetworklistmanagerevents 、https://learn.microsoft.com/en-us/windows/win32/api/netlistmgr/nf-netlistmgr-inetworkconnectionevents-networkconnectionconnectivitychanged
- 交叉引用：[3.8]。

---

## 7. Wintun：许可、签名、分发与适配器管理（问题 7）

### 7.1 许可

- 站点与仓库 README 的概括：源码以 GPL 2.0 提供；预编译的 DLL "在 ZIP 里附带的、比 GPL 2.0 更宽松的许可下发布"；站点还说"由于微软的驱动签名要求，我们提供预编译且已签名的版本，可以与你的软件一起分发"，"下面这些签名的 DLL 是分发 Wintun **唯一受支持**的方式"[7.1][7.2]。`api/wintun.h` 的 SPDX 行是 `GPL-2.0 OR MIT`（版权 2018-2021 WireGuard LLC）[7.4]。当前版本 0.14.1（站点没给发布日期）[7.1]。
- "Prebuilt Binaries License"（仓库里的 `prebuilt-binaries-license.txt`，共 8 节；以下是 WebFetch 对原文的概括，不是法律意见）[7.3]：
  1. **定义**："软件"指下载的 ZIP 里 `wintun.dll` 文件的**确切内容**；"允许的 API"是随附 `wintun.h` 里的接口。
  2. **授权**：WireGuard LLC 授予非独占、不可转让的、为合法目的使用软件的权利，受所述义务与限制约束。
  3. **限制**：软件归 WireGuard LLC 所有，是授权而非出售；不得反向工程、修改或制作衍生品（**除非只通过"允许的 API"使用**）、不得去掉版权等声明；不得转售 / 再分发 / 出租 / 转让 / 再授权，**未经书面同意不可，除非与"仅通过允许的 API 使用该软件"的其它软件一起分发**；不得借用 WireGuard 或贡献者的名字为衍生产品背书。
  4. **担保**：按现状提供。5. **责任限制**。6. **终止**：你可随时终止；违反条款则自动立即终止，终止后必须删除全部副本并停止分发。7. **可分割**。8. **保留权利**：WireGuard LLC 可随时更改或停止软件及其许可条款与 API。
  - 许可**没有明说**：随附时是否必须带上许可文本（第 3(c) 条只要求不去掉专有声明）；能不能**嵌入**可执行文件（"第 3(d) 条允许'与其它软件一起分发'，没有直接讨论嵌入"）。
- 对 MIT 应用的含义（推断，非法律意见）：把官方签名的 `wintun.dll` **原样**放在 rurge 的安装目录里分发，rurge 只经 `wintun.h` 的接口使用它，属于第 3 条允许的"一起分发"；rurge 自己的 MIT 许可不受影响，但许可文本应随 DLL 一起放进去；**把 DLL 的字节嵌进 `rurge.exe` 再从内存加载**（sing-tun 的做法，见下）是否算"提取 / 修改"没有明文，建议在项目所有者确认之前不做。PRD 的风险 R2 要求"阶段 3 确认 Wintun 再分发条款"——这一条就是确认的结果与剩余不确定处。

### 7.2 签名与分发方式

- Wintun 自己的 README：把 `wintun.dll` 与应用**并排**分发，用 `LoadLibraryEx()` + `GetProcAddress()` 动态加载；"不要分发名叫 'Wintun' 的驱动或文件（会与官方部署冲突），请原样分发从 wintun.net 下载的 `wintun.dll`"[7.2]。
- 各项目怎么做：tun2proxy（把 DLL 复制到可执行文件同一目录，`cargo install` 时缺省是 `%USERPROFILE%\.cargo\bin`）[1.19]；`tun-rs`、`tun` 的 README 同样要求放在可执行文件旁并以管理员运行 [2.1][2.2]；clash-rs 的 README 也是"与架构匹配的 `wintun.dll` 放在可执行文件旁，以管理员运行"[1.16]；hev-socks5-tunnel 把 `wintun` 目录放进仓库的 `third-part/`（用于 MSYS2 构建）[1.13]；leaf 的 Windows 配置里可以指定 wintun 文件（可选项）[1.14]；**sing-tun** 把每种架构（`amd64`、`arm`、`arm64`、`x86`）的 DLL（README 写"DLL version: 0.14.1"）嵌在二进制里，用自带的 `memmod` 从内存字节加载，源码里**没有签名校验**也没有回落 [7.5]。
- 加载安全：`wintun` crate 的 `load()` 从**当前目录**加载 `wintun.dll`；`load_from_path` 指定路径；三个加载函数都标 `unsafe`，因为是加载任意 DLL [2.3]。`wintun-bindings` 提供 `verify_binary_signature` 特性（加载前校验 DLL 签名）[2.3]。（推断）rurge 以管理员运行，必须用**绝对路径**（可执行文件所在目录）加载，避免当前目录 / 搜索路径被植入 DLL，并开签名校验。

### 7.3 适配器：名字、GUID、权限、清理

- `WintunCreateAdapter(Name, TunnelType, RequestedGUID)`：名字与隧道类型都是以零结尾的字符串，至多 `MAX_ADAPTER_NAME − 1` 个字符；`RequestedGUID` 决定网络位置感知（NLA）条目的生成：给了固定 GUID 就是确定性的，**传 NULL 由系统随机选 GUID，每个新适配器都会产生新的 NLA 条目**（"未识别的网络"会越积越多）；文档自己也说这个 API 没有文档化、用请求 GUID 可能有"小的有趣的麻烦"[7.2][7.4]。sing-tun 的做法：GUID 取 `MD5("wintun" + 适配器名)`，所以对同一个名字是确定的；隧道类型字符串固定 `sing-tun` [2.13]。WireGuard for Windows 的 `tunnel/` 目录里也有一个 `deterministicguid.go`（本次没读内容）。
- 创建需要管理员（`wintun` / `wintun-bindings` 的文档与示例注释）[2.3]。
- `WintunCloseAdapter`："释放适配器资源，并且如果适配器是用 `WintunCreateAdapter` 创建的，就删除它"（用 `WintunOpenAdapter` 打开的不删）；`WintunDeleteDriver` 只在没有适配器在用时才删驱动 [7.4]。
- **崩溃 / 残留**：`wintun.h` 没有写进程退出或崩溃之后适配器会怎样，也没写 `WintunCreateAdapter` 遇到同名适配器时的行为 [7.4]（"未说明"）。sing-tun 遇到"已存在"就 `OpenAdapter` 复用，既不删除也不重建 [2.13]。（搜索摘要，邮件列表）2021 年有报告说 `wireguard-windows` 服务死掉后 Wintun 接口有时起不来，只有重启能恢复；2025 年 5 月有报告说删除适配器实例时出现"路径未找到"的竞态，维护者回复"删除一个已经不存在的东西不算失败"，并改成忽略这个错误（提交 `28627e00f0`）[7.7][7.8]。
- 驱动本身：`WintunDeleteDriver`、`WintunGetRunningDriverVersion` 可用；Wintun 驱动由官方用微软认证签名，不需要测试签名模式（`README` 里的 `bcdedit /set testsigning on` 只针对**自己构建**驱动的开发者）[7.2]。

### 7.4 对 rurge 的建议

1. **分发**：把官方 `wintun.dll`（固定 0.14.1 与 SHA-256，按架构 amd64 / arm64 / x86 / arm）原样放在安装目录，旁边放它的许可文本；以绝对路径加载并开 `verify_binary_signature`；构建脚本从 wintun.net 取并校验哈希（CI 里同理）。**先不嵌入**；要嵌入的话先让项目所有者向 WireGuard LLC 确认。
2. **适配器**：固定名字（如 `rurge`）、固定隧道类型、由名字派生的确定性 GUID（避免 NLA 堆积）；`WintunCreateAdapter` 遇到已存在就 `WintunOpenAdapter` 复用；退出时调 `WintunCloseAdapter`；残留适配器的处理写进第 3.5 节的恢复流程（下次启动先复用 / 清理）。
3. **权限**：创建适配器失败（`ERROR_ACCESS_DENIED`）时给出"需要以管理员运行"的明确提示（FR-IN-06）。
4. **Rust 侧**：`wintun-bindings`（MIT，有异步会话与签名校验）优于 `wintun`（无异步）；或者通过 `tun-rs` 间接用（它也是动态加载 `wintun.dll`）。

### 本节来源

- [7.1] https://www.wintun.net/
- [7.2] https://git.zx2c4.com/wintun/tree/README.md
- [7.3] https://git.zx2c4.com/wintun/plain/prebuilt-binaries-license.txt （目录页 https://git.zx2c4.com/wintun/tree/ 列出 `COPYING` 与 `prebuilt-binaries-license.txt`）
- [7.4] https://git.zx2c4.com/wintun/tree/api/wintun.h ；SPDX 行见 https://git.zx2c4.com/wintun/plain/api/wintun.h
- [7.5] sing-tun `internal/wintun/`：https://raw.githubusercontent.com/SagerNet/sing-tun/dev/internal/wintun/dll_windows.go 、`README.md`（同目录）
- [7.7] Wintun 邮件列表（2025-05）：https://lists.zx2c4.com/pipermail/wireguard/2025-May/008792.html
- [7.8]（搜索摘要）2020 ～ 2021 的残留适配器报告：https://lists.zx2c4.com/pipermail/wireguard/2021-June/006825.html 、https://lists.zx2c4.com/pipermail/wireguard/2020-December/006171.html （原文本次无法读取）
- 交叉引用：[1.13]、[1.14]、[1.16]、[1.19]、[2.1]、[2.2]、[2.3]、[2.13]。

---

## 8. 测试：没有 root 与有 root（问题 8）

### 8.1 不用 root：内存里的设备对与包夹具

- **sing-tun 的 `MemoryTun`**（`tun_memory.go`）[8.1]：一个纯内存的 TUN 替身，方法与真设备相同（`Name`、`Start`、`MTU`、`Read`、`Write`、`WritePackets`、`Close`……）；入站路径 `WritePackets` 把包拷进带头部余量的缓冲，推进一个大小为 4 倍批量的环，环空时才唤醒引擎；出站包按 `Route` 回调选队列，由 `Outbound` 处理器成批取走；缺省批量 128；**不模拟卸载（GSO / GRO）**；`Read` 一律返回 `os.ErrInvalid`（不能当字节流读）；空包、超 65535 字节的包、环满时的包被悄悄丢弃。新栈检测到 `*MemoryTun` 就换上内存版 IO 并打开校验和验证 [1.7]。
- **sing-tun 的集成测试夹具** [8.1]：`kernelStackFixture` 实现处理器接口；既可以建真实 TUN 并用 netlink 配地址 / 起接口，也可以用 `memoryBridge` 在真实 TUN 与进程内的 `MemoryTun` 之间搬包；对端是**操作系统的内核 TCP**（用 `net.Dialer` 拨）；用例包括：4 MiB 随机负载双向逐字节比较、`CloseWrite` 后读到恰好的 EOF、读端停住时写端必须在 100 ms 内阻塞（反压）、传输约 4 GiB 加 123 字节验证序号回绕、丢弃接收窗口更新 ACK 后上传仍完整、让内核 SACK 之后再丢段以触发 SACK reneging、零窗口与半关闭的恢复；丢包用 `outboundDrop`（丢下 N 个出站包）与 `filterOutbound`，延迟用 `pauseInbound` / `pauseOutbound` 的闸门，没有刻意乱序；Linux 的一部分用例用 `tc netem` 给 TUN 加丢包 / 延迟（需要 root 或 `CAP_NET_ADMIN`）[8.1]。
- **smoltcp 自带的测试件**（本机 0.12.0 源码 `src/phy/`）：`Loopback`、`FaultInjector`（丢包百分比、损坏百分比、最大包长、收发速率限制）、`FuzzInjector`、`Tracer`、`PcapWriter`；`examples/` 里有 `benchmark.rs`、`loopback_benchmark.rs`、`tcpdump.rs`、`ping.rs` 等 [8.3]。0.14 的 README 给出的基准（i5-13500H，Linux 6.9.9）：tap0 读端 3.673 Gbit/s、写端 7.905 Gbit/s，`loopback_benchmark` 15.395 Gbit/s [1.20]。
- **netstack-lwip**：`examples/throughput.rs` 与 `concurrent.rs` 用 smoltcp 当进程内对端，不需要 TUN 设备也不需要 root [1.14]。
- **ipstack**：CI 只在 ubuntu 上跑 `cargo test --all-features --examples`，没有 TUN [8.4]。
- **rurge 内部先例**：`rurge_proto::testing` 里 `FakeWgPeer` 等"回环假对端"一路都是内存 / 回环里的对端，没有真设备。

### 8.2 要 root：公开项目的做法

| 项目 | 做法 |
| --- | --- |
| tun-rs | 三个系统都构建并跑常规用例；**只有 Linux** 上 `sudo modprobe tun \|\| true` 并列出 `/dev/net/tun`，再用 `sudo -E env ...` 跑名字以 `privileged_` 开头的 `#[ignore]` 用例（`-- --ignored privileged_ --test-threads=1`）；Windows 与 macOS 没有特殊步骤、也没有下载 `wintun.dll`；另有 Android / iOS 模拟器 / FreeBSD（虚拟机动作）/ OpenHarmony 的交叉检查和 `cargo-semver-checks` [8.2] |
| sing-tun | `ubuntu-latest`（Go 1.25.x、1.26.x、stable）、`windows-latest`、`macos-latest`；Linux 先 `apt-get install iproute2`；Linux 与 macOS 用 `make test TEST_FLAGS=-exec sudo`（整套测试以 root 运行），Windows 不带 `sudo`；ping 包的特权用例单独编译后 `sudo` 运行；工作流里没有下载 Wintun 的步骤（它把 DLL 嵌在二进制里）[8.1] |
| tun2proxy | `rust.yml` 在三个系统上构建但**从不执行 `cargo test`**；`tests.yml` 只在 ubuntu 上、用机密里的真实 SOCKS5 / HTTP 代理，用 Python 脚本以 `sudo` 起 `tun2proxy-bin --setup`，比较起隧道前后的公网 IP（ipify）来判断是否生效——非自包含 [8.5] |
| ipstack | 见上，不建 TUN [8.4] |
| 网络命名空间 | 本次没有找到哪个公开项目在 GitHub Actions 里用 `ip netns` 做 TUN 测试。sing-tun 有生产用的命名空间辅助（`execInNetworkNamespace`：锁定 OS 线程、`unix.Setns(CLONE_NEWNET)` 进入 `/run/netns/<名>`、监听 / 接口查找都能在命名空间里做）[8.6]；tun2proxy 的 README 有 `--unshare`（在非特权命名空间里建 TUN）[1.19] |

### 8.3 GitHub 托管运行器上能做什么

- 权限（搜索摘要，GitHub 文档的镜像与旧版页面；`docs.github.com` 本次读不到，请对着现行页面确认）：Linux 与 macOS 虚拟机使用免密 `sudo`；Windows 虚拟机以管理员运行、UAC 关闭 [8.7]。sing-tun 与 tun-rs 的工作流在 Linux 上直接用 `sudo`，sing-tun 也在 macOS 上 `-exec sudo`，与此一致。
- 镜像（`actions/runner-images` 的 README）：`ubuntu-latest` = ubuntu-24.04；`macos-latest` = macos-26（arm64；macos-14 标"已弃用"）；`windows-latest` = Windows Server 2025 [8.8]。
- `/dev/net/tun`：tun-rs 的工作流是 `modprobe tun || true` 后跑特权用例，说明至少在它的 Ubuntu 运行器上可用（推断，没有看到运行日志）。macOS：sing-tun 在 `macos-latest` 上以 `sudo` 跑含 Darwin 集成测试文件（`stack_go_integration_darwin_test.go`）的整套测试，推断创建 utun 可行。Windows：未见任何公开项目的工作流下载并安装 Wintun；sing-tun 的 Windows 作业有 Windows 集成测试文件，因为 DLL 嵌在二进制里不需要下载。托管运行器上能否加载 Wintun 驱动、创建适配器，没有证据（推断：驱动经微软认证签名，不需要测试签名模式）。

### 8.4 对 rurge 的建议

1. **三层**，层层递进、层与层之间用不同的门控：
   - **L1（缺省 `cargo test`，三个系统，无特权）**：把设备抽象成 trait（读一个 IP 包 / 写一个 IP 包 / 批量），用内存里的一对设备；包用 `smoltcp::wire` 或 `etherparse` 的 `PacketBuilder` 现场构造，不存二进制夹具；覆盖 NAT 改写 / 校验和、fake-IP 应答器（含 A / AAAA / type 65 / canary / 池回绕 / 持久化）、`hijack-dns`、预匹配 RST、`[IP Rewrite]` 三种动作、UDP 关联、ICMP 回显转发、路由求补集；端到端用"另一个 smoltcp 栈当应用端"跑 TCP 往返，用 `FaultInjector` 注入丢包 / 损坏；吞吐 / 内存基准做成忽略的用例（照 rurge 现有的 `throughput` 基准模式）。这一层不需要 root，是 CI 的主力。
   - **L2（Linux CI，`sudo`，门控）**：真实 `/dev/net/tun` + `unshare -n`（新网络命名空间）+ veth，断言 `ip rule` / 路由被装上、退出后被删、`kill -9` 之后再启动能把残留规则清掉（事务日志的恢复路径）、DNS 劫持端到端、用 `tc netem` 做丢包；用 tun-rs 式的 `#[ignore]` + `privileged_` 命名 + `--ignored privileged_ --test-threads=1`。
   - **L3（macOS / Windows CI，`sudo` / 管理员，门控）**：冒烟——建 utun / Wintun（CI 里按固定 URL 与 SHA-256 取 `wintun.dll`）、配地址、加一条路由、过一个 TCP 往返与一个 ping、拆除后核对系统状态已恢复。
2. **门控与项目现有规矩的关系**：`CLAUDE.md` 的测试约定是"测试只用回环地址和端口 0""在失败时也不得离开回环、离开自己的临时目录"；特权 TUN 用例**按定义就要改系统路由与 DNS**，必须作为明确的例外：独立的测试 crate（照 `tests/interop`、`tests/external`），只在设了环境变量（如 `RURGE_TEST_PRIVILEGED=1`，沿用互操作用例的 `RURGE_INTEROP_REQUIRED` 模式）时运行，缺省与开发机上都不跑，只在一次性的 CI 虚拟机里跑；事务日志的恢复是兜底。
3. **L1 先于一切**：栈的选择（第 1 节）与 L1 夹具的设计应当一起定——用户态栈天然适合 L1，`system` 栈的 NAT 改写层本身也可以在 L1 里用纯函数测，但"内核 TCP 对端"那一半只能在 L2 / L3 里测。

### 本节来源

- [8.1] sing-tun：https://raw.githubusercontent.com/SagerNet/sing-tun/dev/.github/workflows/test.yml 、`Makefile`、`tun_memory.go`、`stack_go_integration_test.go`、`stack_go_integration_flow_linux_test.go`（同仓库 `dev` 分支）
- [8.2] tun-rs：https://raw.githubusercontent.com/tun-rs/tun-rs/main/.github/workflows/rust.yml
- [8.3] 本机 `smoltcp-0.12.0/src/phy/`、`examples/`
- [8.4] ipstack：https://raw.githubusercontent.com/narrowlink/ipstack/main/.github/workflows/rust.yml
- [8.5] tun2proxy：https://raw.githubusercontent.com/tun2proxy/tun2proxy/master/.github/workflows/tests.yml 、`rust.yml`、`tests/tests.py`
- [8.6] sing-tun `netns_linux.go`：https://raw.githubusercontent.com/SagerNet/sing-tun/dev/netns_linux.go
- [8.7]（搜索摘要）GitHub 托管运行器权限：现行页面 https://docs.github.com/en/actions/reference/runners/github-hosted-runners （本次读不到；摘要来自该文档的镜像与旧版页面）
- [8.8] 运行器镜像清单：https://github.com/actions/runner-images/blob/main/README.md
- 交叉引用：[1.7]、[1.14]、[1.19]、[1.20]。

---

## 9. 需要项目所有者决定的事，和这次没有核实的事

### 9.1 需要项目所有者定的（我不替它们做主）

1. **FR-IN-06 的字面**：要不要允许 TCP 经"系统栈 + 地址改写"（`system` 栈）进入流水线（第 1.4 节）。
2. **MSRV**：smoltcp 0.13 / 0.14 要 Rust 1.91，而工作区与 CI 是 1.89（第 1.2 节）。不升就只能留在 0.12，并继续承受阶段 2 记下的那几条限制。
3. **Wintun 的分发形态**：旁置分发（建议）还是嵌入可执行文件；是否要先向 WireGuard LLC 书面确认（第 7 节）。
4. **设备层依赖**：采用 `tun-rs`（Apache-2.0；传递依赖的许可大体已核，见 2.4；`default-features = false` 可避开 `netconfig-rs`）还是自己写薄薄的一层（macOS 零 unsafe、Linux 一处 `ioctl`、Windows 经 `wintun-bindings`）；`tun`（WTFPL）是否特批（第 2 节）。
5. **`rurge-platform` 的 unsafe 预算**：Windows 的 IP Helper 路由 / 地址 / DNS / 变化通知（约 6 ～ 10 个调用点）、进程识别（约 5 个）、`IP_UNICAST_IF`（1 个）、WFP（若做严格模式）；Linux `TUNSETIFF`（1 个）；macOS `sysctlbyname`（1 个）、`proc_pidpath`（1 个）。数字都是我的估计。
6. **macOS 上的 SSID**：接受"命令行守护进程拿不到 SSID"，还是以后做一个带位置权限的 App 包辅助程序（第 6.3 节）。
7. **特权 TUN 测试作为"只用回环"规矩的例外**，以及它们只在一次性 CI 虚拟机里跑的门控（第 8.4 节）。
8. **ICMP**：按 Surge 手册转发（兼容性清单 9.1 节已这样登记）还是退而本机应答；Windows 转发需要管理员级的原始 ICMP 套接字或 `IcmpSendEcho2`（第 1.4 节第 5 条）。
9. **NFR-01 的 1 万连接 / 300 MB**：用户态栈的每连接缓冲要压到 8 ～ 32 KiB 才可能达标（第 1.2 节第 6 条）；若不现实，要不要把验收口径改成"进程常驻内存"以外的口径（`system` 栈的内核内存不计入进程 RSS）。

### 9.2 建议在定稿前先做的小实验（都是我提出的，没做过）

| 编号 | 实验 | 要回答的问题 |
| --- | --- | --- |
| E1 | smoltcp 分片栈微基准 | 1 万条空闲连接 + 100 条活跃连接时的每包 CPU、RSS；分片数与每片套接字数的取舍；对比"每连接一个 `Interface`"（第 1.2 节） |
| E2 | `system` 栈最小原型（Linux 命名空间里） | NAT 改写 + 内核 TCP 的单流 / 多流吞吐与内存，作为 E1 的对照 |
| E3 | Windows：`TUN` 是默认路由时，"绑源地址"与 `IP_UNICAST_IF` 各自能不能让出站连接走物理网卡；再试网卡开着 weak host send / 转发（Hyper-V、WSL、ICS）的情形 | 现有 `socket.rs` 的假设是否成立（第 3.3 节） |
| E4 | macOS：`/1` 一对路由压过全局默认路由时，`IP_BOUND_IF` 绑到物理网卡的套接字是否仍能出去（依赖作用域默认路由） | 同上 |
| E5 | macOS：SCDynamicStore 临时键做 DNS 覆盖——mDNSResponder 是否采纳；`kill -9` 之后是否自动失效 | 第 4.2 节的推断 |
| E6 | Windows：`kill -9` 之后 Wintun 适配器是否残留、下次 `WintunCreateAdapter` 的返回值 | 第 7.3 节"未说明"的部分 |
| E7 | GitHub 托管运行器：能否创建 utun（macOS）、能否加载 Wintun 并创建适配器（Windows）、`/dev/net/tun` 是否可用（Ubuntu） | 第 8.3 节的推断 |
| E8 | 当前各 macOS 版本上 `net.inet.tcp.pcblist_n` 的记录大小与偏移是否和 mihomo 的 384 / 408 一致 | 第 5.1 节 |
| E9 | Windows 11 上 `WlanQueryInterface(current_connection)` 在不同进程形态（用户前台进程、SYSTEM 服务）下的返回值；WinRT `GetConnectedSsid` 是否同样受位置权限限制 | 第 6.3 节 |

### 9.3 本次没有核实 / 没有读到的

- 没有读：Windows 的 `NotifyRouteChange2` / `NotifyUnicastIpAddressChange` 文档（只读了 `NotifyIpInterfaceChange`；网络列表管理器只有搜索摘要）；WinRT `GetConnectedSsid`；Windows 强 / 弱主机模型的微软原文（只有搜索摘要，见 3.3）；sing-tun 新栈的任何基准数字；`tun-rs` 传递依赖里 `winreg`、`widestring`、`encoding_rs`、`scopeguard`、`ahash`、`blocking` 的许可；`ipstack` 的 `tcp.rs` 全文（只读了 `tcb.rs` 与 `lib.rs`）；mihomo 的 TUN 配置源码（只读了文档）。
- 无法直接读（只有搜索摘要，可信度低一档）：Mozilla canary 域文章、苹果开发者论坛、Jamf 社区、GitHub 官方文档的"管理员权限"一节、Wintun 邮件列表 2020 ～ 2021 的几条、SMHNR 的微软原文。
- WebFetch 的摘要风险：所有数字（偏移、常数、版本、基准）和"源码里有 / 没有某某"的判断都经过一个小模型的转述；定稿前对着原文再核一遍，尤其是第 5.1 节的 macOS 偏移表、第 2.2 节的常数表和第 1.1 节里 ipstack 的缺口清单。
- 范围之外没有展开：Android / iOS 的 VPN 服务形态；macOS 的 Network Extension（Surge Mac 5.8 起用它，rurge 的兼容性清单 9.1 节已登记为直接操作 utun）；Linux 的 TPROXY / `auto_redirect` 旁路；Windows 的 WinDivert / WFP 重定向（兼容性清单 9.2 节为网关模式留作"待评估"）；网关模式与 DHCP（阶段 7）。

---

## 10. 候选 crate 一览

说明：①"版本（日期）"是本次在 docs.rs / `Cargo.toml` / GitHub 上读到的最新版本，日期是页面显示的发布日期或最近推送日期；②"MSRV"写"未写"表示 `Cargo.toml` 或页面没有 `rust-version`；③"unsafe 增量"指**把它用起来之后 rurge 自己的代码里要新增多少 `unsafe`**（crate 内部的不算），带"（推断）"的是我的估计；④"Cargo.lock"一栏标明本仓库当前是否已经依赖它（读 `Cargo.lock` 得到）。项目的许可名单是 MIT / Apache-2.0 / BSD / ISC / MPL-2.0（用户给的约束），`0BSD` 属 BSD 家族，smoltcp 已经在依赖树里。

| crate | 版本（日期） | 许可 | MSRV | 维护状态 | 用途 | unsafe 增量 | Cargo.lock / 备注 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `smoltcp` | 0.14.0（2026-08-07）；0.13.1（2026-05-01）；本仓库在用 0.12.0 | 0BSD | 0.14 / 0.13：**1.91**；0.12：1.80 | 活跃 | 用户态 TCP / UDP / ICMP 栈 | 0 | 已有 0.12.0。套接字查找线性；升级需 MSRV 1.91；0.14 有 `pause_synack`、零窗口探测、Reno / CUBIC 合规 |
| `etherparse` | 0.21.0（2026-10-01） | MIT OR Apache-2.0 | 未写 | 活跃 | IPv4 / IPv6 / TCP / UDP / ICMP 解析与构造、校验和、分片重组（`std`） | 0（页面没说它是否禁用 unsafe） | 不在。`smoltcp::wire` 已能做同样的事，可以不引入 |
| `ipstack` | 标签 `v.1.0.1`，主分支 `1.0.2-dev`（2026-10-12 推送，88 星） | Apache-2.0 | 未写（edition 2024，故至少 1.85） | 活跃 | 备选的用户态 TCP 栈（O(1) 会话查找，异步流） | 0 | 不在。TCP 简化（见 1.1）；被 tun2proxy 使用 |
| `netstack-smoltcp` | 0.1.4（automesh-network，已归档）；分支 `cavivie/netstack-smoltcp`（2026-07-11 推送） | MIT OR Apache-2.0 / Apache-2.0（分支） | 1.75 | 原仓库归档 | 参考设计 | — | 不要依赖；只借鉴"`any_ip` + 每个 SYN 一个监听套接字" |
| `watfaq-netstack` | 0.1.2（clash-rs 工作区内） | MIT OR Apache-2.0 | 未写 | 随 clash-rs | 参考设计 | — | 自述"不打算用于生产" |
| `tun-rs` | 2.8.11（2026-10-16 推送，231 星） | Apache-2.0 | 未写 | 很活跃 | 三平台 TUN 设备（Windows 经 `libloading` 动态加载 `wintun.dll`）、Linux 卸载与多队列 | 0（`from_fd` 例外） | 不在。传递依赖 `netconfig-rs`（MIT）、`getifaddrs`（MIT OR Apache-2.0）、`route_manager`（Apache-2.0）、`libloading`（ISC）已核，其余未逐个读 |
| `netconfig-rs` | 0.1.6（2026-03-20） | MIT | 未写 | tun-rs 组织接手维护的分支 | 跨平台的接口 / 地址管理（`tun-rs` 缺省特性拉入） | 0 | 不在；依赖 `windows` ^0.61.3、`nix` ^0.30.1，会在依赖树里造成第二个版本 |
| `libloading` | 0.9.0（2025-11-05） | ISC | 未写 | 活跃 | 动态加载 `wintun.dll`（`tun-rs`、`wintun` 内部用） | 加载与取符号是 unsafe（在 crate 内部） | 不在 |
| `tun` | 0.8.14（2026-09-26 推送，685 星） | **WTFPL** | 未写 | 活跃 | TUN 设备 | 0 | 不在。许可不在名单 |
| `wintun-bindings` | 0.7.40（2026-08-17） | MIT | 未写 | 活跃 | Wintun 封装、异步会话、签名校验特性 | 1（`load_from_path`） | 不在。tun2proxy 组织维护 |
| `wintun` | 0.5.1（2026-08-30） | MIT | 未写 | 一般 | Wintun 封装 | 1 | 不在。没有异步 |
| `nix` | 0.31.3 | MIT | 1.69 | 活跃 | macOS utun 的**安全**建立（`SysControlAddr`、`UtunIfname`）、netlink / 路由套接字、`ioctl` 宏 | utun：0；每个 `ioctl` 宏调用点 1 | **已有**（`rurge-platform` 只开了 `signal` 特性，要加 `socket`、`net`，Linux 设备还要 `ioctl`） |
| `socket2` | 0.6.5 | MIT OR Apache-2.0 | 1.70 | 活跃 | 安全的 `set_mark`、`bind_device`、`bind_device_by_index_v4/v6`、`set_tos_v4`、`set_tclass_v6` | 0 | **已有**。没有 Windows 的 `IP_UNICAST_IF` |
| `libc` | 0.2.189 | MIT OR Apache-2.0 | 1.65 | 活跃 | Linux TUN 常数；macOS `sockaddr_ctl`、`proc_pidpath`、`sysctlbyname` | 调用即 unsafe | **已有**。缺 Apple 的 `ifaliasreq` / `SIOCAIFADDR`、`socket_fdinfo` / `PROC_PIDFDSOCKETINFO` |
| `windows-sys` | 0.61.2 | MIT OR Apache-2.0 | 1.71 | 活跃 | IP Helper（路由 / 地址 / DNS / 通知 / 连接表）、WLAN、WFP | 每个调用都 unsafe（约 6 ～ 10 个调用点做路由，约 5 个做进程识别，推断） | **已有**（`rurge-platform` 开了 `Win32_Security`、`Win32_System_JobObjects`、`Win32_System_Threading`；工作区开了 `Win32_Networking_WinInet`）；要加 `Win32_NetworkManagement_IpHelper`、`Win32_NetworkManagement_Ndis`、`Win32_Networking_WinSock`，可选 `Win32_NetworkManagement_WiFi`、`Win32_NetworkManagement_WindowsFilteringPlatform`（这些特性名在本机 0.61.2 的 `Cargo.toml` 里核对过） |
| `windows` | 0.62.2 | MIT OR Apache-2.0 | 1.82 | 活跃 | 同上，更高层的封装 | 多数仍是 unsafe | **已有**（传递依赖） |
| `rtnetlink` | 0.23.0（2026-08-18） | MIT | 未写 | 活跃 | Linux 地址 / 路由 / 规则 / 链路管理与多播监听（异步） | 0 | 不在；带 `netlink-packet-route` 0.33、`netlink-proto` 0.13、`netlink-sys` 0.9 |
| `route_manager` | 0.2.16（2026-10-12） | Apache-2.0 | 未写 | 活跃 | 路由增删与监听（Linux / macOS / BSD / Windows） | 0 | 不在；`bindgen` 为可选构建依赖 |
| `net-route` | 0.4.6（2026-10-03） | MIT | 未写 | 活跃 | 路由增删、默认路由、变化流 | 0 | 不在；`bindgen` 为构建依赖 |
| `tproxy-config` | 7.0.9（2026-09-21） | MIT | 未写 | 活跃 | 一整套"装路由 + 改 DNS"（tun2proxy 用） | 0 | 不建议依赖：崩溃后不恢复、Windows 不恢复 DNS；可当参考 |
| `system-configuration` | 0.8.0（2026-10-15） | MIT OR Apache-2.0 | 未写 | 活跃 | macOS SCDynamicStore / SCNetworkReachability / SCPreferences | 0 | 不在；`core-foundation` 0.10.1 已有 |
| `if-watch` | 3.2.2（2026-09-15） | MIT OR Apache-2.0 | 未写 | 活跃（libp2p 维护者） | 跨平台的 IP 地址变化事件 | 0 | 不在；只有地址事件，没有路由 / 默认网关 |
| `netdev` | 0.46.3（2026-09-13） | MIT | 未写 | 活跃 | 接口枚举、默认接口与网关 | 0 | 不在 |
| `if-addrs` | 0.15.0 | （本次未读） | — | — | 接口枚举 | 0 | **已有**（`rurge-platform` 在用） |
| `wl-nl80211` | 0.8.0（2026-10-01） | MIT | 未写 | 未说明 | Linux nl80211 | 0 | 不在 |
| `zbus` | 5.19.0（2026-10-14） | MIT | 未写 | 活跃 | D-Bus（NetworkManager、systemd-resolved） | 0 | 不在 |
| `objc2-core-wlan` | 0.3.2（2026-10-10） | Zlib OR Apache-2.0 OR MIT | 未写 | 活跃 | macOS CoreWLAN | 未知（objc2 的框架绑定里多数方法标 `unsafe`，这个 crate 没读到） | 不在 |
| `libproc` | 0.14.11 | MIT | 1.85 | 活跃 | macOS libproc（含 `net_info`） | 0 | 不在；构建依赖 `bindgen` 0.73.2（`runtime`，需要 libclang）。不建议 |
| `netstat2` | 0.11.2 | MIT OR Apache-2.0 | 未写 | 一般 | 套接字表 + 所属 PID | 0 | 不在；构建依赖 `bindgen` 0.72。不建议 |
| `surge-ping` | 0.9.1（2026-09-16） | MIT | 未写 | 一般 | ICMP 回显（socket2 + tokio） | 0 | 不在；只是发 ping，不是"转发应用自己的回显" |
| `sock2proc` | git（2025-11-06 推送，0 星） | **无** | — | 停滞 | 套接字 → 进程（clash-rs 用） | — | 不可用 |

---

（报告结束）
