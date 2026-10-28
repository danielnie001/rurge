# 阶段 3 / M4「平台接入」细化设计

| 项       | 内容 |
| -------- | ---- |
| 日期     | 2026-10-26 |
| 状态     | 草案。已决事项 M4-D1 ～ D30 是在项目所有者"自己决定、继续做"的常设授权下定的（代项目所有者作的裁定），所有者可以改。执行交给 Pi：四份计划按 M4a → M4b → M4c → M4d 的次序，在一个会话里逐个任务做（`superpowers:executing-plans`，没有子代理），每份计划做完停下交项目所有者（第 2 节） |
| 上位设计 | [阶段 3 总设计](2026-10-16-phase3-enhanced-mode-design.md) 第 1.4、3.2 ～ 3.4、3.6、4、9、11 ～ 13、16 节；[M3 细化设计](2026-10-18-phase3-m3-tun-core-design.md) 第 14、18、19 节。不一致处以本文为准，订正列在第 18 节 |
| 需求     | FR-IN-06（其余：三个平台的虚拟网卡、路由、`tun-included-routes` / `tun-excluded-routes` / `ipv6-vif`、权限提示、退出时恢复；ICMP 转发的真实一半）、FR-IN-10（接管暂停的 TUN 一半）、FR-DNS-05（`force-syslib` 与接管期间的 `system` / `syslib`） |
| 清单     | 9.1 节（增强模式的各行）；6.3 节 `server:system` 一类、6.4 节；`[General]` 的 `tun-excluded-routes`、`tun-included-routes`、`ipv6-vif`、`icmp-forwarding`；2.x 节 `external` 的 `addresses`；10.3 节 `rurge run`（`--tun` → `--enhanced-mode`）、10.4 节 `/v1/features/enhanced_mode` |
| 前置     | 阶段 3 / M1 ～ M3 已合并（`main` = `origin/main` = `ac0a98a`）。CI 在 `ac0a98a` 上**第一次三个平台全绿**（运行 40283632986，2026-10-26：`check` 在 ubuntu / macos / windows 上与 `bench` 都通过，注解只有 Node 20 的弃用提示）——M3 的全部用例第一次在三个平台上跑过 |
| 调研     | 总设计引的两份报告；本文另读了 sing-tun（`dev` 分支）的 `tun_linux.go`、`tun_darwin.go`、`tun_windows.go`、`tun_bind_windows.go` 与 sing-box 的 `route/network.go`（GPL：只取事实，代码不抄），wireguard-go 的 `tun/tun_darwin.go`（MIT） |

M4 把 M3 做完的虚拟网卡内核接到三个平台的真实设备上：`rurge run --enhanced-mode`（或运行中经 API）建虚拟网卡、配地址、装路由、把系统 DNS 指向应答器，rurge 自己的每一个出站连接绑在物理网卡上不回到虚拟网卡，退出、关闭、崩溃之后都恢复使用者原来的网络。做完之后，增强模式第一次对使用者可见；虚拟网卡的内核、应答器、预匹配与经监听器来的 fake 地址也随之第一次可见。

开发机（Windows 11）上能测的是：路由的计算、三个平台的步骤（作为数据）、输出的解析、备份与恢复、管理器的整个生命周期（经一个只供测试的后端，第 9.10 节）、防环的出口判定与"没有漏网的套接字"、Windows 一侧的编译与不建网卡的部分。**Linux 与 macOS 的代码只在 CI 上编译**（开发机只装了 Windows 的目标），**建网卡、改路由与 DNS 的部分在任何自动化的地方都不运行**（L2 特权用例写好、是否在 CI 上启用归项目所有者，第 14 节），真实的效果只由手工验收证明。

## 1. 范围

| 做 | 不做（去向） |
| -- | ------------ |
| 三个平台的设备：Linux `/dev/net/tun`、macOS utun、Windows Wintun（旁置、校验签名者、动态加载）；MTU 1500 | 多队列、卸载（GSO / GRO）与大于 1500 的 MTU（以后的性能手段）；附带或下载 `wintun.dll`（总设计 Q3，归项目所有者） |
| 地址、路由（基础前缀、排除、包含、rurge 的地址空间）、`ipv6-vif` 的三种取值；系统 DNS 的接管与交还；Linux 的 `rp_filter` | Windows 防 DNS 泄漏的严格模式（WFP，总设计 Q5，归项目所有者）；不用 systemd-resolved 的 Linux 上改 `/etc/resolv.conf`（不改，告警，P3-D8） |
| 防环：rurge 自建的每一个出站套接字经一处建法、按出口判定绑到物理网卡；DNS 上游、资源下载、Profile 来源、QUIC、ICMP 都收进来；源码扫描的用例 | 读操作系统的整张路由表来判定出口（不做；非直连的 VPN 路由的限制登记，第 7.1 节） |
| 接管期间的 `system` / `syslib` / 单标签名字查物理网卡的服务器；`force-syslib` 照旧交系统解析库并在加载时告警 | `.local` 照旧交系统解析库（不变） |
| 生命周期：开关（`--enhanced-mode`、`RURGE_ENHANCED_MODE`、`GET` / `POST /v1/features/enhanced_mode`）、打开与关闭的次序、备份与崩溃之后的恢复、重载、网络变化、接管暂停的 TUN 一半、权限的错误 | 跨重启保留开关（不做，同系统代理，M4-D15） |
| 真实的 `Pinger`（ICMP 套接字，按出口绑定） | ICMP 差错的转发与 `traceroute`（不做，总设计 Q-01） |
| `external` 的 `addresses` 从路由里排除（不再报 `W0029`）；外部程序与它的子进程的会话一律 DIRECT | 预匹配在 DNS 与 SYN 层先于进程查找（外部程序发往预匹配规则拒绝的目的地的连接仍被拒，登记，第 11 节） |
| M3 延后事项里去向是 M4 的（第 15 节）：`rurge-tun` 的三条、`start_tun` 的三条、`join` 的时限、系统 DNS 名单排除 rurge 的地址 | R27 的窄修（不做，阶段 4，M4-D24） |
| `rurge check` 的四条新诊断；测试后端；L2 特权用例（Linux 网络命名空间）；手工清单的 M4 一节；文档 | L2 在 CI 上启用、macOS 与 Windows 的冒烟（总设计 Q4，归项目所有者） |

## 2. 四份计划与执行

| 计划 | 内容 | 可验收的产出 |
| ---- | ---- | ------------ |
| **M4a 平台无关的骨架** | 路由集的计算；三个平台的步骤生成（纯数据，任何平台上都编译、都测）；`StateStore::try_update` 与增强模式的备份；可执行文件的管理器与后端接口（平台后端此时在每个平台上都答"这一版还不能"）；测试后端；`--enhanced-mode`、`Command::EnhancedMode`、API；接管暂停的 TUN 一半；重载与网络变化的重算；退出的次序与 `join` 的时限；崩溃之后的恢复；`rurge check` 的诊断；`external` 的 `addresses` 进路由的排除；`rurge-tun` 的三条与 `start_tun` 的三条 | 经测试后端的守护进程端到端：开、关、失败的回退、`kill` 之后再起的恢复、暂停、重载与网络变化的增量；三个平台的步骤与路由集的数据用例 |
| **M4b 防环与接管期间的 DNS** | `SocketHook::egress` 与出口判定；`open_socket`、`DirectPacket`、QUIC 的套接字按出口绑定；DNS 的纯 UDP 上游与截断后的 TCP 经连接器；资源下载与 Profile 来源带平台钩子、名字经 rurge 的解析器；源码扫描用例与运行时用例；`SystemDns` 知道接管；`system` / `syslib` / 单标签名字的去处；系统 DNS 名单与网络环境排除 rurge 的地址；真实的 `Pinger`；外部程序一律 DIRECT | 带记录的钩子看到每一类 rurge 自建的套接字都问过出口；扫描用例；接管期间 `syslib` 不进系统解析库；Pinger 在回环上的用例 |
| **M4c Linux 与 macOS** | 两个平台的设备（各一个 unsafe 函数）、步骤的执行与核对、系统 DNS（resolved、`networksetup`）、`rp_filter`、DNS 的物理服务器的读法；L2 用例（Linux 网络命名空间，门控） | CI 上两个平台编译、用例通过；L2 用例写好（没有运行过，照实写）；**这一份的代码要等项目所有者推送、CI 回来才知道能不能编译** |
| **M4d Windows 与收尾** | Wintun（加载、签名者、适配器、会话、读线程、发送）、`netsh` 的执行与读回核对、`IP_UNICAST_IF`、虚拟网卡自己的接口设置、`--wintun-dll`；文档收尾（兼容性清单、API 参考、手工清单的 M4 一节、`docs/testing.md`、README、`CLAUDE.md`、`docs/status.md`） | Windows 上编译与不建网卡的用例；签名校验拒绝未签名的文件；手工清单 |

**次序的理由**：M4b 在两份平台计划之前——真实的设备一能起来，rurge 自己的连接就必须已经不回到虚拟网卡（漏一处就是一个核烧满、网络断，总设计风险 E）。M4a 的平台后端在 M4c / M4d 之前对每个平台都答"这一版还不能"，所以四份计划之间任何一个提交上 `rurge run --enhanced-mode` 都不会做半截的系统改动。

**交给 Pi 执行的约定**（每份计划的开头重申）：

- 一个会话里按任务的次序做，不跳、不并行；每个任务先写失败的用例、看到红，再实现到绿，跑门禁，自查任务的判据与计划的全局约束，再提交（提交说明的标题用中文，末尾不加署名行以外的东西——署名行照项目所有者的规矩）。
- **每份计划的第一个任务是"开工前核对"**：对着当时的代码核对计划引用的接口、文件与行号（写计划之后代码可能变了），出入记进「执行期修正记录」，按记录改后面的任务，不改计划的正文。
- 计划没有写到、要做决定的地方：能按本文与总设计的原则定的，自己定，记进修正记录并写"执行者的裁定"；去向写着"项目所有者"的、或会改变本文已决事项的，停下来列出，交给项目所有者。
- 每份计划做完停下：项目所有者评审、决定合并与推送。M4c 的代码任务做完之后有一个"等 CI"的停点：Pi 不推送，请所有者推送，读回 CI 的注解（`CLAUDE.md` 写了怎么读）再继续。
- 仓库的规矩（`CLAUDE.md` 的「必须遵守的规矩」「测试约定」「这台开发机」）一律适用；尤其：测试只用回环、缺省的 `cargo test` 不改系统设置、从不执行路由与 DNS 的命令、从不建网卡；不下载、不安装；Windows 开发机上从不执行 `netsh`、`route`、`ipconfig` 这类读写网络的命令（核对它们的写法按微软文档）。

## 3. crate 与接口

### 3.1 `rurge-platform`

依赖方向不变：不依赖任何工作区 crate，用自己的类型，可执行文件把它们映射成引擎与 `rurge-tun` 的类型。新加依赖边 `tokio`（设备的异步读写）与 `ipnet`（路由集），都是工作区已有的包，`Cargo.lock` 的包数不变；`windows-sys` 多开几个特性（`Win32_Networking_WinSock`、`Win32_System_LibraryLoader`、`Win32_Security_WinTrust`、`Win32_Security_Cryptography`，写计划时核对确切的特性名，第 17 节 V3）；`nix` 不开新特性（`ioctl` 用 `nix::libc`；`socket` / `net` 要 `memoffset`，P3-D3 不开）。

| 模块 | 新的或改的 | 计划 |
| ---- | ---------- | ---- |
| `route`（新） | 路由集的计算（`RouteSet`，第 4 节，纯函数）；步骤的类型与三个平台的步骤生成（第 5 节，**不带 `cfg`**，三个平台的生成在任何平台上编译与测试）；三个平台各自的执行与读回（带 `cfg`，第 5.5 节） | 计算与生成 M4a；执行 M4c、M4d |
| `tun`（新） | 设备：`TunDevice::open(&DeviceSpec)`、`name`、`mtu`、`recv`、`send`（第 8 节） | M4c、M4d |
| `icmp`（新） | ICMP 套接字的建法与回显的收发（第 10 节） | M4b |
| `socket` | 出口判定 `egress::decide`（纯函数，第 7.1 节）；Windows 的 `IP_UNICAST_IF` / `IPV6_UNICAST_IF`（`bind_interface` 里与绑源地址一起做） | M4b；Windows 一半 M4d |
| `dns` | 名单排除给定的前缀；Linux 在 resolved 的存根之后读上游的名单；macOS 读 DHCP 给的服务器（第 6.3 节） | M4b（排除）、M4c（两个平台的读法） |
| `netenv` | `ReadOpts` 多一项要排除的 DNS 前缀（`exclude` 已有，放虚拟网卡的名字） | M4b |
| `process` | `ProcessTree::contains(pid)`：Unix `getpgid(pid) == 组长`（`nix`，不用 unsafe），Windows 新的 unsafe `in_job`（第 11 节） | M4b；Windows 一半 M4d |

### 3.2 `rurge-net`

- `SocketHook` 多一个有缺省实现的方法 `egress(&self, dst: IpAddr) -> io::Result<Option<String>>`：rurge 自己到 `dst` 的套接字要绑的网卡；缺省 `Ok(None)`（不绑），`NoopSocketHook` 与今天的一切测试照旧；`Err` 表示这条连接不该建（接管期间没有物理网络，第 7.1 节）。
- `open_socket`：策略写了 `interface` 时照旧；没写时问 `egress(目的地址)`，`Some(网卡)` 就经 `bind_interface` 绑上，绑不上就是这次连接失败（不退回不绑：不绑就会回到虚拟网卡）。`allow-other-interface` 只管策略的 `interface`，不管出口的绑定。
- `bind_udp(family)` 改成 `bind_udp(family, dst: Option<IpAddr>)`：QUIC 的直连套接字（`quic/socket.rs` 的 `direct_socket`，两处调用都知道远端地址）给出远端；`DirectPacket`（DIRECT 的 UDP 载体，不连接的套接字）按每个数据报的目的地址问出口，绑定不同的目的地用不同的套接字（按（地址族，出口）各一个、用到时才建），收包时轮询全部。
- 这一改之后，`rurge-net` 里建出站套接字的只有 `open_socket` 一处（第 7.2 节的扫描用例钉住）。

### 3.3 `rurge-dns`

- 纯 UDP 上游一律经连接器：`UdpUpstream::new(addr)` 自己 `UdpSocket::bind` 与截断之后 `TcpStream::connect` 的那条路去掉，换成经一个直连连接器（与今天 `UdpUpstream::via` 的路相同；名字仍是 `udp://<地址>`）。`ResolverDeps` 多一个直连连接器（带平台钩子），专给纯 UDP 上游与引导（`bootstrap_set`）用——**纯 UDP 上游不随 `encrypted-dns-follow-outbound-mode` 走流水线**（今天就不随，不改）。
- `SystemDns` 多一个有缺省实现的方法 `enhanced_mode(&self) -> bool`（缺省 `false`）：增强模式此刻开着，系统解析库的回答可能是 fake 地址——接管了系统 DNS，或 `hijack-dns` 劫到了系统的服务器（没有 resolved 的 Linux 上不接管，但后者仍可能）。解析器在查找时读它（第 6.2 节）。
- `system_lookup` 与 `HostAction::System(mode)` 的去处按它分开（第 6.2 节）。

### 3.4 `rurge-engine`

- `StateStore::try_update(f) -> io::Result<State>`：先在副本上改、写盘成功才换进内存；写不进返回错误、内存不变（第 9.2 节）。`State` 多一个 `enhanced_backup: Option<serde_json::Value>`（引擎不认识它的形状，同 `system_proxy_backup`）。
- `Engine::start_tun` 返回 `Result<TunHandle, String>`：先 `options.check()`、再开池的文件（M3b 延后第 24 行）；这个引擎上已有一个 TUN 时返回错误（第 25 行）；`fake-ip.txt` 与它的临时文件在 Unix 上以 `0600` 建（第 23 行）。
- `resource_client()` 与 Profile 来源的客户端接受平台钩子与一个"rurge 自己的名字解析"（第 7.6 节）。
- 外部程序的判定入口（第 11 节）。

### 3.5 `rurge-tun`

M3a 延后第 17 行的第二 ～ 四条（第 15 节）：整个 TUN 一个排队字节的计数与上限；`shard_of` 用每个 TUN 一个 `RandomState`；片重建的 WARN 限频。`Device` 的约定加一句"包按它的长度分配"。不碰别的。

### 3.6 `rurge-api`

`GET` / `POST /v1/features/enhanced_mode` 生效（今天 `GET` 恒 `false`、`POST` 恒 501）：`Control` 多两个方法——开关（回 `Result<(), String>`，失败 500 带原因，同 `system_proxy`）与读状态（真实的开关状态与计数）。`GET` 的形状照 M3-D22：`{"enabled": <bool>, "dropped": {"malformed": <n>, …}, "stackPanics": <n>}`；没有开着时 `dropped` 各项与 `stackPanics` 是 0（计数随 TUN 一起结束，下一次打开从 0 起）。暂停没有单独的字段：暂停时 `enabled` 是 `false`（总设计 9.7 节，同 `system_proxy`）。

### 3.7 可执行文件

- `cli/enhanced/`（新）：管理器 `EnhancedMode` 与后端接口 `Backend`（第 9.1 节）；平台后端（`route` 的执行 + `tun` 的设备 + 读系统的状态）、文件后端、`off`。
- `cli/runtime.rs`：`PlatformSockets` 带一份共享的出口表（管理器写、钩子读，第 7.1 节）；`PlatformSystemDns` 变成有状态的（第 6.2、6.3 节）；`PlatformPinger`（第 10 节）。
- `cli/run.rs`：`--enhanced-mode` / `RURGE_ENHANCED_MODE`、`--wintun-dll` / `RURGE_WINTUN_DLL`；`Command::EnhancedMode`；`Takeover` 多一半（第 9.9 节）；退出的次序（第 9.4 节）；锁之后的恢复（第 9.5 节）；重载与网络变化（第 9.7、9.8 节）。
- `cli/network.rs`：读网络环境时排除虚拟网卡与 rurge 的地址（第 6.3 节）。

### 3.8 unsafe

工作区仍是 `forbid`，新增的全部在 `rurge-platform`，逐函数放开、`tests/unsafe_functions.rs` 的 `LISTED` 同步、`CLAUDE.md` 的 `unsafe_code` 一段同步（P3-D4）。新增 16 个（名字在计划里可以细调，每个只包一次 FFI 调用或一组紧密相关的调用，前一行 `// SAFETY:` 写明前提）：

| 平台 | 文件（计划定） | 函数 | 包的调用 | 计划 |
| ---- | -------------- | ---- | -------- | ---- |
| Linux | `src/tun/linux.rs` | `set_iff` | `ioctl(TUNSETIFF)`（`IFF_TUN \| IFF_NO_PI`） | M4c |
| macOS | `src/tun/macos.rs` | `utun_connect` | `ioctl(CTLIOCGINFO)`、`connect(sockaddr_ctl{sc_unit: 0})`、`getsockopt(UTUN_OPT_IFNAME)` | M4c |
| Windows | `src/socket.rs` | `unicast_if` | `setsockopt(IP_UNICAST_IF / IPV6_UNICAST_IF)` | M4d |
| Windows | `src/process.rs` | `in_job` | `OpenProcess` + `IsProcessInJob` | M4d |
| Windows | `src/tun/windows/wintun.rs` | `verify_signer` | `WinVerifyTrust`（校验与关闭两次）、取签名者证书、`CertGetNameStringW` | M4d |
| Windows | 同上 | `load` | `LoadLibraryExW`、`GetProcAddress` × 11 与函数指针的转换 | M4d |
| Windows | 同上 | `create_adapter`、`open_adapter`、`close_adapter`、`adapter_index`（`WintunGetAdapterLUID` + `ConvertInterfaceLuidToIndex`）、`start_session`、`end_session`、`read_event`、`wait_readable`（`WaitForSingleObject`）、`receive`（收、拷出、释放）、`send`（分配、拷入、发送） | 各一个 Wintun 入口（或上面写的一组） | M4d |

`unsafe_functions.rs` 只读源码，Linux 与 macOS 的两个函数在开发机上编译不到，第一次 CI 核对（同 M2 的两个）。

## 4. 地址与路由

### 4.1 地址与 MTU

虚拟网卡 `198.18.0.1/15`；IPv6 开着时（第 4.6 节）另有 `fd00:6152::1/64`。MTU 1500（M3-D3）。应答器、协议栈与 fake 地址的分配照 M3（`rurge-tun/src/space.rs`）不变。

### 4.2 基础前缀（订正 P3-D7）

**三个平台同一种形式**，照 sing-tun 在 macOS 上的做法用八个前缀而不是一对 `/1`（M4-D2）：

- IPv4：`1.0.0.0/8`、`2.0.0.0/7`、`4.0.0.0/6`、`8.0.0.0/5`、`16.0.0.0/4`、`32.0.0.0/3`、`64.0.0.0/2`、`128.0.0.0/1`——`0.0.0.0/8` 从不进虚拟网卡。
- IPv6：`100::/8`、`200::/7`、`400::/6`、`800::/5`、`1000::/4`、`2000::/3`、`4000::/2`、`8000::/1`，另加 `64:ff9b::/96`（NAT64 的知名前缀，落在 `::/8` 里）——`::/8` 的其余部分（回环、未指定、映射的 IPv4）从不进虚拟网卡。

理由：总设计 9.4 节把"`/1` 压过全局默认路由之后，绑了 `IP_BOUND_IF` 的套接字能不能靠物理网卡的作用域默认路由出去"记成 macOS 上没有验证过的假设，并留了"改用 sing-tun 式的多个前缀"的退路；sing-tun 在 macOS 上用的就是这八个前缀，同 `IP_BOUND_IF` 一起用了多年。一开始就用退路，三个平台同一种形式（计算与用例只有一份），代价是多六条路由。另一个退路（给代理与 DNS 服务器的地址加经物理网关的主机路由）不做。

### 4.3 总是排除的

从基础前缀里总是减去：IPv4 `127.0.0.0/8`、`169.254.0.0/16`、`224.0.0.0/4`、`255.255.255.255/32`；IPv6 `fe80::/10`、`ff00::/8`。组播与广播即使进了虚拟网卡也被丢弃（P3-D23），减去它们是为了不让系统把它们送进来。

### 4.4 排除与包含

- **排除**：`tun-excluded-routes` 与全部 `external` 策略的 `addresses`（每个地址作 `/32` / `/128`）从基础前缀里减去（求补集：对每个基础前缀，没有洞与它相交就留下，被某个洞包含就扔掉，否则对半分开递归，最后合并相邻的；约三十行，`ipnet` 已有合并）。
- **包含**：`tun-included-routes` 原样加上（总设计 Q-03：包含赢）。IPv6 没开时，包含与排除里的 IPv6 项都不用（`rurge check` 不为此告警：同一份 Profile 在 `ipv6-vif = auto` 下时开时关）。
- 整段私网写进 `tun-included-routes` 时 `rurge check` 告警（第 12 节）。

### 4.5 rurge 的地址空间

`198.18.0.0/15` 与 `fd00:6152::/64`（IPv6 开着时）必须进虚拟网卡：

- Linux 与 Windows 由网卡的地址带出（地址带前缀长度，系统自己装连接路由）；**macOS 显式装一条**（utun 是点对点接口，地址不带出子网路由）。
- 求出的前缀里整个落在这个空间里的去掉（它已经被上面那条覆盖；Linux 上与内核的连接路由重复会让成批执行失败，第 5.2 节）。
- 排除项与这个空间相交时不剥掉它（剥掉的话应答器与 fake 地址的连接都走物理网卡，增强模式整个失效），`rurge check` 告警（第 12 节）。

### 4.6 `ipv6-vif`

`disabled`（缺省）：只有 IPv4。`always`：IPv6 总开（`rurge check` 照手册告警，第 12 节）。`auto`：主网络有全局的 IPv6 地址与 IPv6 默认路由时开（总设计 Q-21），网络变化时重判。IPv6 开着时：网卡加 IPv6 地址、装 IPv6 的路由、系统 DNS 多一个 `fd00:6152::2`。

**`rurge-tun` 总以 `ipv6: true` 起**（M4-D17）：`TunOptions::ipv6` 只决定 IPv6 的包处理不处理，IPv6 没有路由进来时它不起作用；这样 `ipv6-vif` 随重载与网络变化开关时只动地址、路由与 DNS，不重起内核。

### 4.7 计算

`RouteSet::compute(&RouteInputs) -> RouteSet`：输入是 IPv6 开不开、排除、包含、外部程序的地址、平台要不要显式装 rurge 的空间；输出是 IPv4 与 IPv6 两组排好序的前缀。另有 `RouteSet::diff(&old, &new) -> (加, 减)`（重载与网络变化用）与 `RouteSet::contains(ip)`（出口判定用，含 rurge 的空间）。纯函数，任何平台上用数据测：基础前缀的形状、总是排除的、求补集（一个 `/32` 洞得多少条、相邻的洞合并、洞包含整个前缀、洞跨两个基础前缀）、包含赢、rurge 空间的去重与保留、IPv6 的有无、`64:ff9b::/96`。

## 5. 三个平台的步骤

### 5.1 步骤是数据

管理器只认识抽象的动作——建设备、配网卡（地址、MTU、起来、网卡自己的设置）、加 / 减一组路由、接管 / 交还系统 DNS、调整（Linux 的 `rp_filter`）、清 DNS 缓存——；平台后端把动作翻成命令。翻译的函数是纯的：输入动作与网卡的名字或序号，输出 `Vec<Step>`，`Step` 是"一个程序与它的参数"或"一个程序与它读的一个脚本文件的内容"（`ip -batch <文件>`、`netsh -f <文件>`；文件写在数据目录里、用完删掉），连同"失败算不算错"。三个平台的翻译都**不带 `cfg`**，用例在任何平台上把输出逐字比对（快照或字面量，计划定）。执行在带 `cfg` 的函数里，经 `command::CommandRunner`（失败的文本经 `command::failure`）。**程序一律用系统目录里的绝对路径，不经 `PATH`**（以 root / 管理员运行，`PATH` 可能被改过）：Windows `%SystemRoot%\System32\` 下的 `netsh.exe`、`ipconfig.exe`；macOS `/sbin/ifconfig`、`/sbin/route`、`/usr/sbin/networksetup`、`/usr/bin/dscacheutil`、`/usr/bin/killall`、`/usr/sbin/ipconfig`；Linux 的 `ip` 与 `resolvectl` 在 `/usr/sbin`、`/sbin`、`/usr/bin`、`/bin` 里按次序找第一个有的（计划核对，第 17 节 V2）。

撤销同样是数据（第 9.2 节）：**备份里存的是类型化的撤销动作，从不存命令行**——恢复时由当时的代码把动作翻成命令，备份文件被改了也只能让 rurge 做它本来就会做的那几种撤销。

### 5.2 Linux

- 配网卡（第 9.3 节第 2 步）一批 `ip -batch <文件>`（遇到第一个错误就停、返回非零）：`link set dev rurge mtu 1500`、`addr add 198.18.0.1/15 dev rurge`、`link set dev rurge up`；IPv6 开着时另一批 `addr add fd00:6152::1/64 dev rurge`。路由（第 5 步）再一批：每个前缀一行 `route replace <前缀> dev rurge`（`replace` 而不是 `add`：重复的、与内核连接路由相同的都不报错；IPv6 的前缀同样写，`ip` 按前缀认地址族）。**IPv6 的那几行失败时**（比如系统关了 IPv6）一行 WARN、这一次按 IPv6 没开处理，不让整个打开失败（M4-D17）——所以 IPv6 的地址与路由各在自己的一批里。
- 减路由：`ip -force -batch <文件>`（遇错继续），每行 `route del <前缀> dev rurge`。设备关闭时它的地址与路由随之消失，不必逐条减。
- 系统 DNS：`/etc/resolv.conf` 里是 systemd-resolved 的存根（`nameserver 127.0.0.53`）且有 `resolvectl` 时：`resolvectl dns rurge 198.18.0.2 [fd00:6152::2]`、`resolvectl domain rurge '~.'`、`resolvectl default-route rurge yes`、`resolvectl flush-caches`；交还：`resolvectl revert rurge`（设备已经没了时失败不算错）。不是存根时不改，一行 WARN（P3-D8）。逐链路的设置随设备消失，不进备份。
- `rp_filter`：第 7.3 节，直接读写 `/proc/sys/net/ipv4/conf/<名字>/rp_filter`（不经 `sysctl` 程序：名字里的点不用换写法）。

### 5.3 macOS

- 配网卡（第 9.3 节第 2 步）：`ifconfig <utunN> inet 198.18.0.1 198.18.0.1 netmask 255.254.0.0 mtu 1500 up`；IPv6 开着时 `ifconfig <utunN> inet6 fd00:6152::1 prefixlen 64`。
- 路由（第 5 步）一条一个进程（没有成批的工具；三十条上下，各几毫秒）：`route -n add -net 198.18.0.0/15 -interface <utunN>`（第 4.5 节），每个 IPv4 前缀 `route -n add -net <前缀> -interface <utunN>`，IPv6 加 `-inet6`；减：`route -n delete …`，"不存在"不算错。设备关闭时随之消失。
- 系统 DNS：`networksetup -listallnetworkservices` 列出服务（第一行是说明、以 `*` 开头的是停用的，跳过）；对每个启用的服务先 `networksetup -getdnsservers <服务>` 读原值（"There aren't any DNS Servers set on …"那一句读作"空"，第 17 节 V6 核对这句的原文），写进备份，再 `networksetup -setdnsservers <服务> 198.18.0.2 [fd00:6152::2]`；全部设完 `dscacheutil -flushcache` 与 `killall -HUP mDNSResponder`。交还：`networksetup -setdnsservers <服务> <原值…>`，原值是空的写 `Empty`。**这是三个平台里唯一会在崩溃之后留下的 DNS 改动**（总设计 9.3 节、风险 F），所以先写备份、写不进就不改。网络变化时新出现的启用服务同样先备份再接管。

### 5.4 Windows

- 设备由 Wintun 建（第 8.4 节），网卡以序号指称（`adapter_index`，不用名字：残留的同名网卡不会被认错）。每一步一份 `netsh -f <文件>` 的脚本，能带 `store=active`（重启即失）的都带上：配网卡（第 9.3 节第 2 步）——`interface ipv4 add address <序号> 198.18.0.1 255.254.0.0 store=active`、`interface ipv4 set interface <序号> metric=0 forwarding=disabled weakhostsend=disabled weakhostreceive=disabled store=active`（接口度量 0、关自动度量；关转发与 weak host send 是防环的一半，第 7.4 节）、`interface ipv4 set subinterface <序号> mtu=1500 store=active`；路由（第 5 步）——每个前缀一行 `interface ipv4 add route <前缀> <序号> 0.0.0.0 metric=0 store=active`；系统 DNS（第 6 步）——`interface ipv4 set dnsservers <序号> static 198.18.0.2 register=none validate=no`，然后 `ipconfig /flushdns`。IPv6 开着时各步另有同样的 `interface ipv6 …`（DNS `fd00:6152::2`）。DNS 不能早于第 6 步：协议栈与路由没好之前把系统 DNS 指过去，这段时间里的查询都没有应答。
- **读回核对**：`netsh` 的退出码与输出的文字（随系统语言变）都靠不住，脚本跑完之后读 `netsh interface ipv4 show route`（与 `ipv6`），按"前缀 / 长度"与"序号"两个数字取行（不看表头与文字），每条计划里的路由都要在，缺的列在错误里、整个打开回退。
- 减路由：`interface ipv4 delete route <前缀> <序号> store=active`，遇错继续。网卡上的 DNS 设置写在网卡自己的配置里，随网卡消失（重用残留的网卡时被重新设置），不进备份。
- 写法（参数名、`store=active` 在哪些命令上可用、按序号指称网卡）在写计划时按微软文档核对（第 17 节 V2），**不在开发机上执行 `netsh`**。

### 5.5 执行的位置

`rurge_platform::route::{linux, macos, windows}` 各有：翻译（不带 `cfg`）、执行一组步骤、读系统的状态（macOS 的服务与原值、Linux 的 resolved 与 `rp_filter`、Windows 的路由表）——后两样带 `cfg`。读系统状态的解析（`networksetup` 的输出、`/etc/resolv.conf` 是不是存根、`netsh … show route` 的行）是不带 `cfg` 的纯函数，用文本夹具测（夹具里含非英文系统的输出，Windows 的路由表至少一份中文的）。

## 6. 系统 DNS

### 6.1 接管与交还

P3-D8 与第 5.2 ～ 5.4 节。接管在打开的倒数第二步（路由之后），交还在关闭的第一步（第 9.3、9.4 节）。没有 systemd-resolved 的 Linux 上不接管，一行 WARN，说可以写 `hijack-dns = *:53` 或自己把系统 DNS 指到 `198.18.0.2`。

### 6.2 接管期间的 `system`、`syslib`、`force-syslib` 与单标签名字

今天三种 `server:system` / `syslib` / `force-syslib` 都交系统解析库（`resolver.rs` 的 `library_lookup` 的说明："all three are the library in the normal mode"），单标签名字先查系统的服务器、没有服务器才交系统解析库。增强模式开着时系统解析库的查询可能经应答器、拿到 fake 地址，所以（`SystemDns::enhanced_mode()` 为真时，M4-D7；下表"接管期间"指这段时间）：

| 查找 | 不接管时（不变） | 接管期间 |
| ---- | ---------------- | -------- |
| `server:system`、`server:syslib` | 系统解析库 | 物理网卡的服务器（第 6.3 节），纯 UDP、经直连连接器；没有已知的服务器时这次查找失败，**不退回系统解析库** |
| `server:force-syslib` | 系统解析库 | 系统解析库（照手册；会拿到 fake 地址——加载时的告警，第 12 节） |
| 单标签名字（加上搜索域之后） | 系统的服务器，没有时系统解析库 | 物理网卡的服务器，没有时失败 |
| `.local` | 系统解析库 | 系统解析库（mDNS 不经单播 DNS，接管不影响它） |

### 6.3 物理网卡的服务器

`PlatformSystemDns::servers()` 一律去掉 rurge 的地址空间里的服务器（M3c 延后第 4 行），接管期间另按平台：

- **Windows**：今天就遍历全部开着的网卡、取各自的 DNS（`dns::servers()`）；虚拟网卡上的 `198.18.0.2` 落在 rurge 的空间里被去掉，剩下的就是物理网卡的。
- **Linux**：`/etc/resolv.conf` 是 resolved 的存根时读 `/run/systemd/resolve/resolv.conf`（resolved 写的上游名单，含各链路的服务器；虚拟网卡那一条被去掉）——今天读 `/etc/resolv.conf` 得到的 `127.0.0.53` 在接管期间会把查询送回应答器（回环）。不是存根时 rurge 没有接管 DNS，`/etc/resolv.conf` 照旧。
- **macOS**：`/etc/resolv.conf` 在接管期间只剩 `198.18.0.2`（被去掉之后是空的）。物理网卡的服务器取：主网卡所在的服务在备份里的原值，不是空的就用它；是空的（使用者没有手动设，用 DHCP 给的）就读 `ipconfig getpacket <主网卡的设备名>` 里的 `domain_name_server`。打开时先记下接管之前 `/etc/resolv.conf` 的服务器，网络变化之前一直用它。

网络环境（`cli/network.rs` 读快照）同样去掉 rurge 的空间里的服务器、排除虚拟网卡（`ReadOpts`）；Windows 与 Linux 上接管因此不引起网络变化。**macOS 上打开增强模式之后会有一次多出来的网络变化**（快照里系统 DNS 从原来的变成空），它的下游（清 DNS 缓存、重测、重建 WireGuard）照常做一遍，登记在 API 参考里，不另做处理（M4-D7）。

## 7. 防环

### 7.1 出口的判定

纯函数 `rurge_platform::socket::egress::decide(table, dst) -> Result<Option<网卡名>, NoRoute>`（M4-D8）。`table` 由管理器在打开、关闭、重载、网络变化时重建，可执行文件的 `PlatformSockets::egress` 读它（`RwLock<Option<Arc<EgressTable>>>` 一类，读在每个出站套接字上，只做查找）：

1. 增强模式没开（表是空的）：不绑。
2. 回环、未指定、组播、链路本地的目的地址：不绑。
3. 目的地址落在某个**非虚拟网卡**的直连子网里（取最长的前缀；子网来自 `if_addrs`，建表时读一次）：绑那块网卡。——这条在第 4 条之前：`tun-included-routes` 把一段局域网送进了虚拟网卡时，rurge 自己到那里的连接仍走局域网的网卡；不在包含里时系统本来也走那块网卡，绑了无害。
4. 目的地址在路由集里（`RouteSet::contains`，含 rurge 的空间）：绑主网卡（网络环境给出的、排除虚拟网卡的那一块）；没有主网卡时 `Err(NoRoute)`——错误说"no network outside the enhanced mode to reach <地址>"，这条连接失败。
5. 其余（被排除的、`external` 的地址）：不绑，照系统的路由走。

**登记的限制**：不读操作系统的整张路由表，所以**不是直连的 VPN 路由**（VPN 推下来的 `10.0.0.0/8 via 10.8.0.1 dev tun0`）上的目的地，应用的流量照系统的路由进 VPN（它比基础前缀更具体），rurge 自己到那里的连接却被绑到主网卡、到不了；办法是把那一段写进 `tun-excluded-routes`（第 5 条，不绑）。写进 API 参考、手工清单与兼容性清单 9.1 节。

### 7.2 一处建法与漏网的检查

rurge 自建的出站套接字一律经 `rurge-net` 的 `open_socket`（第 3.2 节）。今天绕过它的（写设计时读到的，M4b 的开工前核对再全工作区搜一遍）：

| 地方 | 今天 | M4b |
| ---- | ---- | --- |
| `rurge-dns/src/upstream/udp.rs` 的 `UdpUpstream::new`（`UdpSocket::bind` 与截断后的 `TcpStream::connect`） | 自己建 | 经直连连接器（第 3.3 节） |
| `rurge-engine/src/stack.rs` 的解析器连接器与 `resource_client()`、`runtime.rs` 的流水线回退连接器 | `DirectConnector::new(SystemResolve)`，即 `NoopSocketHook` | 带平台钩子（`StackOptions.socket_hook`）；名字经第 7.6 节 |
| `rurge-engine/src/profile.rs` 的 Profile 来源（托管配置、远程 include）的客户端 | 同上（`resource_client()`） | 同上 |
| `rurge-net/src/quic/socket.rs` 的 `direct_socket` | 经 `bind_udp`，不知道目的地址 | 给出远端地址 |
| `rurge-net/src/connector.rs` 的 `DirectPacket` | 两个不绑的套接字 | 按数据报的出口（第 3.2 节） |
| ICMP 转发 | 没有（假实现） | `PlatformPinger` 按出口绑（第 10 节） |

**用例**（M4-D9）：

- **源码扫描**（照 `unsafe_functions.rs` 的写法，放在 `crates/rurge/tests/`）：读工作区每个 crate 的 `src/` 下的源码，跳过 `#[cfg(test)]` 紧跟的 `mod tests` 块之后的部分与 `testing` 模块，找建套接字的写法（`UdpSocket::bind(`、`TcpStream::connect(`、`TcpSocket::new_v4(` / `new_v6(`、`Socket::new(`、`Endpoint::client(` 等，计划给全表），与一张名单比对：名单上只有不出站或另有出口的地方（`open_socket` 本身、监听器、SOCKS5 UDP 关联的本地中继、`external` 连回环、`netenv` 的路由套接字、`tun` 与 `icmp` 的套接字），每项写理由；多一处、少一处都变红。
- **运行时**：带记录的钩子（假的 `SocketHook`，记下 `egress` 被问到的地址）经真实的解析器与连接器：每一类 DNS 上游（UDP、截断后的 TCP、`tcp://`、DoT、DoH、DoQ、DoH3，对回环上的参照服务端）、资源下载、DIRECT 的 TCP、UDP 与 QUIC，都在建套接字之前问过 `egress`，问的是回环上那个服务端的地址；`egress` 回 `Err` 时连接失败、不退回不绑。

### 7.3 Linux：`SO_BINDTODEVICE` 与 `rp_filter`

绑定照今天的 `bind_device`（`SO_BINDTODEVICE`）：绑了网卡的套接字查路由时只看那块网卡的路由，走物理网卡的默认路由出去。**回包的问题**：严格的反向路径过滤（`rp_filter = 1`）对进来的包查"回到源地址的路由"时不看套接字的绑定，查到的是虚拟网卡上的基础前缀，与进来的网卡不同，丢弃——rurge 自己连接的回包全被丢掉。有效值是 `all` 与那块网卡两者的大者，RHEL 一族缺省是严格的。

做法（M4-D10）：打开时与每次网络变化时，若主网卡的有效值是 1，把 `net.ipv4.conf.all.rp_filter` 设成 2（宽松：只丢源地址根本不可达的包），旧值先写进备份（`Undo::RpFilter { value }`，只记第一次的旧值），关闭时写回。改 `all` 而不改那块网卡：有效值取大者，`all = 2` 之后换了网络也不用再改。INFO 一行说改了什么、为什么。IPv6 没有这个开关（firewalld 的 IPv6 反向路径检查按进来的网卡查，不受影响）。

**不照 sing-tun 的策略路由**（它在单独的路由表与 `ip rule` 里装路由）：单表最长前缀加上绑网卡已经够，规则表的增删、与别的软件（VPN、容器）的规则相处、崩溃之后的残留都省掉；代价就是上面的 `rp_filter`。

### 7.4 Windows

阶段 2 的"绑源地址"（`bind_interface` 绑物理网卡的地址）依赖强主机模型：发出包的网卡必须拥有源地址——虚拟网卡不拥有物理网卡的地址，所以包不会从虚拟网卡出去。M4（M4-D11）：

- 绑源地址照旧；
- 再加 `IP_UNICAST_IF` / `IPV6_UNICAST_IF`（新的 unsafe `unicast_if`；IPv4 的值是网络字节序的网卡序号，IPv6 是主机字节序，计划核对，第 17 节 V3）；
- **虚拟网卡自己**关转发与 weak host send（第 5.4 节的 `set interface`）：开了 weak host send 的网卡可以发出源地址不属于它的包，那正是绑源地址要排除的；
- **不动别的网卡**。sing-tun 在运行期间关掉所有别的网卡的 weak host send（它的注释说 `IP_UNICAST_IF` 在绑定的网卡开了转发或 weak host send 时被忽略），那要读写别的网卡的设置、进备份；rurge 有绑源地址这一层，那种情形下仍由强主机模型挡住。这是读文档与源码得出的推理，**没有在真机上验证**：手工清单在开了 Hyper-V、WSL 或网络共享（ICS）的机器上专门核对一项；不成立时的退路是 sing-tun 的做法（记在这里，不做）。

### 7.5 macOS

`IP_BOUND_IF` / `IPV6_BOUND_IF`（`bind_device_by_index_v4/v6`，已有）：绑了网卡的套接字走那块网卡的作用域默认路由（系统为每块有网关的网卡装一条 `-ifscope` 的默认路由）。配八个前缀（第 4.2 节）。

### 7.6 rurge 自己的名字解析

接管期间系统解析库（`SystemResolve`，即 `getaddrinfo`）问的是应答器、拿到 fake 地址。rurge 自己的连接器里用它解析名字的，今天有资源下载与 Profile 来源（下载链接里的主机名）；解析器的加密上游经引导解析，代理服务器的名字经引擎的解析器，都不受影响。M4（M4-D9）：资源下载与 Profile 来源的连接器用一个"rurge 自己的解析"：增强模式开着时（`SystemDns::enhanced_mode()`）经引擎当前这一代的解析器（`ResolverCell`）解析，否则照旧 `SystemResolve`——不接管时的行为一点不变；这一代还没有解析器时（启动的头一下，增强模式还没开）照旧 `SystemResolve`。

## 8. 设备

### 8.1 共同的

`rurge_platform::tun::TunDevice`：`open(&DeviceSpec) -> io::Result<TunDevice>`、`name()`、`mtu()`、`async recv(&self, into: &mut Vec<Vec<u8>>, max)`（等到至少一个包，再不等待地读到 `max` 个或读空）、`async send(&self, packets: &mut Vec<Vec<u8>>)`（逐个写，写不进——内核或环满了——就丢弃这一个、计数，第一次一行 WARN，其余 DEBUG；M3 设计 4.1 节"真实设备收不下就丢"）。读出的包按它的长度分配（第 3.5 节的约定）。可执行文件把它包成 `rurge_tun::Device`。MTU 1500，不卸载。**一台机器一个增强模式**（M4-D12）：设备名与系统 DNS 都是全机的，第二个 rurge 打开时得到"已被占用"的错误。

### 8.2 Linux

以 `O_RDWR | O_NONBLOCK | O_CLOEXEC` 打开 `/dev/net/tun`（`OpenOptionsExt::custom_flags`），`set_iff`（`ifreq` 的名字 `rurge`、`IFF_TUN | IFF_NO_PI`，不持久：描述符关闭、进程结束时网卡与它的地址、路由一起消失），经 `tokio::io::unix::AsyncFd<File>` 读写；一次读一个包，缓冲 MTU 大。`EBUSY`：名字被占用（另一个 rurge）。

### 8.3 macOS

`socket2` 建 `AF_SYSTEM` / `SOCK_DGRAM` / `SYSPROTO_CONTROL` 的套接字（非阻塞、`CLOEXEC`），`utun_connect`：按名字 `com.apple.net.utun_control` 查控制号、`connect` 一个 `sc_unit = 0` 的 `sockaddr_ctl`（**内核挑单元号**）、`getsockopt(UTUN_OPT_IFNAME)` 读回 `utunN`（照 wireguard-go；订正总设计 3.3 节"由 rurge 从小到大试"，M4-D12）。每个包前面 4 字节的地址族（网络字节序的 `AF_INET` / `AF_INET6`），读时去掉、写时加上（`send_vectored`，不拷贝包）。套接字关闭、进程结束时网卡与它的路由消失。

### 8.4 Windows（Wintun）

- **找 DLL**：`--wintun-dll <路径>`（环境变量 `RURGE_WINTUN_DLL`），否则可执行文件所在目录的 `wintun.dll`；绝对路径，`LoadLibraryExW(<绝对路径>, NULL, LOAD_LIBRARY_SEARCH_SYSTEM32)`（DLL 本身从给的路径载入，它的依赖只从 System32 找；计划核对标志的这层含义，第 17 节 V3）。找不到：错误说去哪里取（`https://www.wintun.net/`）、放在哪里、或用 `--wintun-dll` 指（P3-D17）。
- **校验签名者**（加载之前，两种来源都校验）：`WinVerifyTrust`（`WINTRUST_ACTION_GENERIC_VERIFY_V2`，不查吊销、只用缓存——校验从不联网，第 17 节 V3），从状态数据取签名者的证书，`CertGetNameStringW` 取简单显示名，必须恰好是 `WireGuard LLC`；不过就不加载，错误说"not signed by WireGuard LLC"并写出读到的签名者（或"not signed"）。用例：拿测试二进制自己（没有签名）断言被拒；有真的 `wintun.dll` 时（环境变量 `RURGE_TEST_WINTUN_DLL` 指着，缺省没有）断言通过。
- **适配器**：名字 `rurge`、隧道类型 `rurge`、固定的 GUID（一个写死的常数，计划给出），先 `WintunOpenAdapter` 复用残留的，没有再 `WintunCreateAdapter`（Wintun 0.14 起适配器随建它的进程结束而移除，残留只在老版本或异常时出现）；`adapter_index` 取序号给 `netsh` 用。建好之后等它在 IP Helper 里出现（按序号能查到、至多 5 秒）再配置。
- **会话**：环 8 MiB（`0x800000`）；一个专用线程读：`receive` 有包就拷出交给一个有界的通道（1024 个包，满了线程等），没有包就 `wait_readable`（读事件，100 毫秒为限，醒来看停止的标志）；`send`：`WintunAllocateSendPacket` 失败（环满，`ERROR_BUFFER_OVERFLOW`）就丢这一个。关闭：置停止标志、等读线程结束、`end_session`、`close_adapter`。
- `ERROR_ACCESS_DENIED`：权限（第 8.5 节）。

### 8.5 权限

不预先检查（M4-D14）：直接做，把第一个失败的权限错误映射成一句英文说明——Linux `EPERM` / `EACCES`："the enhanced mode needs root or CAP_NET_ADMIN"；macOS `EPERM`："the enhanced mode needs root"；Windows `ERROR_ACCESS_DENIED`："the enhanced mode needs to run as Administrator"。不尝试提权（总设计 9.6 节）。防环的 `SO_BINDTODEVICE` 在老内核（5.7 之前）上要 `CAP_NET_RAW`：以 root 运行时有；只给了 `CAP_NET_ADMIN` 时绑定失败，那条连接失败、错误里带系统的原因。

## 9. 生命周期（可执行文件）

### 9.1 管理器与后端

`EnhancedMode`（`cli/enhanced/mod.rs`，照 `SystemProxyManager` 的样子：`recover` / `enable` / `disable` / `enabled` / `status`）持有后端、`StateStore`、引擎、出口表与当前已做的（设备、路由集、DNS 的接管、`rp_filter`）。后端接口 `Backend`（可执行文件内部的 trait）只有抽象的动作（第 5.1 节）：

- 每个会在崩溃之后留下的动作分两步：后端先说"撤销它要什么"（读系统的状态，比如 macOS 各服务的原值、Linux 的 `rp_filter`），管理器把它写进备份（`try_update`，写不进就不做这一步，整个打开回退），后端再做。不会留下的动作（设备、地址、路由、虚拟网卡上的 DNS、resolved 的逐链路设置）不进备份。
- 三种后端：**平台**（缺省：`route` 的执行 + `tun` 的设备；M4a 里对每个平台都答"the enhanced mode is not available on this platform in this version"，M4c / M4d 换成真的）；**文件**（`RURGE_ENHANCED_BACKEND=file:<路径>`，第 9.10 节）；**`off`**（`RURGE_ENHANCED_BACKEND=off`：打开即失败，"the enhanced mode backend is off"）。

### 9.2 备份（`state.json`）

`state.json` 的 `enhanced_backup` 是管理器自己的结构（serde，引擎不解释）：`{ "undo": [ … ] }`，撤销动作是类型化的枚举——`MacDns { service, servers }`、`RpFilter { value }`、文件后端的 `Test { step }`（M4-D5）。顺序即做的顺序，恢复时倒着做。

- 每加一条用 `try_update`（第 3.4 节）；写不进：不做这一步，已做的倒着撤销，打开失败（错误说"cannot write state.json"，带系统的原因）。
- 全部撤销成功之后整个 `enhanced_backup` 清掉（`try_update`；写不进只记 ERROR：下次启动会再撤销一遍，撤销对已经撤销的是无害的）。
- 撤销一条失败时：记 ERROR，继续撤销其余的；失败的那几条留在备份里，下次启动再试。

### 9.3 打开

次序（M4-D16；任何一步失败，倒着撤销已做的，`rurge run` 以 1 退出——同系统代理——，API 回 500 带原因）：

1. 设备（`TunDevice::open`；Windows 含 DLL 的查找与校验）。
2. 配网卡：地址、MTU、起来、网卡自己的设置（Windows 的度量、转发、weak host send）；IPv6 开着时它的地址（失败按第 5.2 节的规矩）。
3. `TunOptions::check`，`Engine::start_tun`（`ipv6: true`，第 4.6 节；`Err` 即失败）——先能收包，再把流量引过来。
4. 出口表写好（第 7.1 节）——在路由之前：路由一装上，rurge 自己的连接就必须已经绑物理网卡。
5. 路由（第 4 节，成批）。
6. 系统 DNS 接管（先备份）。
7. Linux：`rp_filter`（先备份）。

完成后 INFO 一行"enhanced mode on"（带路由的条数与 IPv6 开没开；名字、地址只在 DEBUG，第 13 节）。

### 9.4 关闭与退出

关闭（API、接管暂停、退出时）：交还系统 DNS → 减路由（流量先回到物理网卡）→ 清出口表 → 停 TUN：`TunHandle::stop()`，再等 `join()` **至多 3 秒**（M3b 延后第 8 行：停止不取消正在进行的拨号，一个永不回来的拨号会让 `join` 一直等；超时一行 WARN、照常往下）→ 关设备 → 写回 `rp_filter` → 清备份。会留下的那几样（macOS 的 DNS、`rp_filter`）按备份里的撤销动作做，做成一条从备份里去掉一条（第 9.2 节）。每一步失败记 ERROR、继续下一步（关闭从不半途停下）。

**退出的次序**（M4-D16）：增强模式先关，再系统代理，再照今天的次序（取消 API 的令牌 → 监听器停止接受 → 等会话 → 停外部程序）。理由：使用者的网络越早恢复越好，虚拟网卡上的会话在停 TUN 时就结束了。

### 9.5 崩溃之后

拿到数据目录的实例锁之后、做任何新改动之前（与系统代理的恢复同一个位置，配置读不了、有错误时也做，阶段 2 / M8c 任务 8 的规矩）：`enhanced_backup` 里有撤销动作就倒着做，成功的从备份里去掉，INFO 一行"restored what the enhanced mode left last time"。设备、地址、路由随上一个进程消失，不用管；Windows 上残留的适配器在下一次打开时复用。

### 9.6 开关

- `rurge run --enhanced-mode`（环境变量 `RURGE_ENHANCED_MODE`，取值同 `RURGE_SYSTEM_PROXY` 的写法）；`--wintun-dll <路径>` 只在 Windows 上有（别的平台上给出它是用法错误）。
- 运行中 `POST /v1/features/enhanced_mode`（`{"enabled": true|false}`）经 `Command::EnhancedMode(bool, 回复)` 进主循环，与重载、网络变化一次一个。已经是要的状态：直接回成功。
- **不跨重启保留**（M4-D15）：同系统代理，只有启动参数决定启动时开不开；`state.json` 的 `features.enhanced_mode` 照系统代理那一项的写法（计划核对它今天怎样写、读不读）。

### 9.7 重载

`TunSettings`（应答器、`hijack-dns`、`always-real-ip`、`icmp-forwarding`）已随引擎的代切换。管理器在重载成功之后重算路由集（排除、包含、外部程序的地址、`ipv6-vif`），只做差：IPv6 从关到开加地址、路由、DNS，从开到关反之；IPv4 与 IPv6 的路由按 `RouteSet::diff` 加减（先加后减）；出口表重建。做差失败：记 ERROR（带失败的步骤），增强模式照开，已做的保留——不回滚到旧集合（旧集合本身也可能已经不对）；`GET` 照实给出 `enabled: true`。

### 9.8 网络变化

在主循环里、照今天的 `network_changed` 之后：重建出口表（主网卡、直连子网）；`ipv6-vif = auto` 重判（变了就照第 9.7 节加减 IPv6）；macOS 上新出现的启用服务接管 DNS（先备份），消失的服务不管（恢复时它若回来了就写回）；Linux 重查 `rp_filter`。路由不用动（基础前缀不经物理网关）。

### 9.9 接管暂停的 TUN 一半

`Takeover`（`run.rs`）今天只管系统代理（`wanted`、`paused`）。M4 照同一种语义加增强模式的一半（M4-D18）：进入暂停时，开着的增强模式按第 9.4 节关闭、记下"想要开着"；离开暂停时，想要开着就按第 9.3 节打开（失败记 ERROR，不重试）；暂停期间的 `POST` 回成功、只记下想要的，暂停结束时照做；`GET` 给真实状态（暂停时 `false`）。启动时所在的网络就暂停：不打开，记下想要。

### 9.10 测试后端

`RURGE_ENHANCED_BACKEND=file:<路径>`（M4-D19）：设备是 `rurge_tun::MemoryDevice`（另一端由后端拿着，丢弃写来的包、从不送包进来）；每个动作记进那个 JSON 文件（当前开着的：设备名、地址、路由集、DNS 接管、调整；做过的动作按次序），每个"会留下的"动作给出 `Test { step }` 撤销动作；文件里可以写 `"fail": "<动作名>"` 让那一步失败、`"fail_undo": "<动作名>"` 让那一个撤销失败。用例经它端到端地断言：打开的次序、失败时的回退、备份先于动作写进 `state.json`、写不进 `state.json` 时不做、`kill` 之后再起时的恢复、关闭与退出的次序、暂停、重载与网络变化的增量、API 的形状。**起 `rurge` 二进制的辅助函数（`rurge_command()`、`rurge_process()`）缺省设 `RURGE_ENHANCED_BACKEND=off`**，要用的用例自己设 `file:`——没有一条缺省的用例能走到平台后端。

## 10. 真实的 `Pinger`

`rurge_platform::icmp`（建法与收发）+ 可执行文件的 `PlatformPinger`（实现 `rurge_tun::Pinger`，按出口绑定）（M4-D20）：

- 套接字：Linux 先 `SOCK_DGRAM` + `IPPROTO_ICMP`（不要特权，受 `net.ipv4.ping_group_range` 管），不行退 `SOCK_RAW`（root 有）；macOS `SOCK_DGRAM`；Windows `SOCK_RAW`（管理员，增强模式本来就要）。IPv6 同理（`ICMPV6`）。全经 `socket2`，不用 unsafe。
- 每个（地址族，出口）一个套接字、用到时才建，一个收包任务按序号把应答交给等着的那一个（标识符是 Pinger 自己的；Linux 的 `DGRAM` 由内核改写标识符、只把这个套接字的应答交上来，所以只按序号对；`RAW` 收到全部 ICMP，按标识符与序号对）。超时由 `rurge-tun` 管（5 秒，M3-D8），`echo` 的 future 被丢掉时取消等待。
- 用例：回环上 ping `127.0.0.1` 与 `::1`（Linux 与 macOS 的 `DGRAM` 在 CI 上；建不了套接字——Windows 不是管理员、Linux 的 `ping_group_range` 不允许——时用例写明原因跳过）；序号的分派与超时取消用假的收发测。

## 11. 外部程序

- **`addresses`**：全部 `external` 策略的 `addresses` 作 `/32` / `/128` 从路由里排除（第 4.4 节；重载时随之做差）。`spec/mod.rs` 不再把 `addresses` 放进"这一版没有效果"（`W0029`）——它在增强模式开着时有效果，不开时本来也不需要（M4-D21）。
- **一律 DIRECT**（手册："Surge always uses the DIRECT policy for requests from external processes. Children … handled the same way"）：会话的进程（`SessionHandle` 的进程槽，M2）的 PID 在某个正在运行的外部程序的进程树里时，不看规则，走 DIRECT，请求记录的 `notes` 写一句原因。进程树：`rurge-proto` 的 `ProcessGroup` 多一个 `contains(pid) -> bool`（缺省 `false`）；平台一侧 `ProcessTree::contains`——Unix `getpgid(pid) == 组长`（外部程序以自己为组长起，子进程继承；子进程自己 `setsid` 的不算，登记），Windows `in_job`（`IsProcessInJob`）；"正在运行的外部程序"的名单跨代（`LocalPorts` 一类，计划定）。经监听器来的会话同样适用（今天进程查找也覆盖它们）。
- **登记的限制**：预匹配在 DNS 与 SYN 层先于进程查找（M3c：只在要记录时才查进程），所以外部程序发往预匹配规则拒绝的名字或地址的查询与连接仍被拒绝。

## 12. `rurge check` 的诊断

四个新码（M4-D26），各在 `codes::hint` 的表里做决定（`every_code_is_decided_once`）；加之前先问 `ParamReader::has_unrejected`（已经报错的参数不再多一条告警）：

| 码 | 条件 | 文字（英文，计划定稿） |
| -- | ---- | ---------------------- |
| `W0035` | `ipv6-vif = always` | 手册："This may break apps on networks without IPv6 connectivity, and Surge warns about it during profile verification" |
| `W0036` | `tun-included-routes` 里有整段私网（`10.0.0.0/8`、`172.16.0.0/12`、`192.168.0.0/16`，或包含它们的更大的前缀） | 手册："it is normally unnecessary and can cause system issues" |
| `W0037` | `tun-excluded-routes` 与 rurge 的地址空间相交 | 这一段仍进虚拟网卡（应答器与 fake 地址在里面） |
| `W0038` | `[Host]` 里有 `server:force-syslib` | 增强模式开着时系统解析库会拿到 fake 地址（手册的警告） |

## 13. 日志与个人信息

- INFO：增强模式开、关、恢复了上一次留下的、改了 `rp_filter`、没有接管系统 DNS 的原因；带路由的条数、IPv6 开没开。**网卡名、地址、路由的前缀、DNS 服务器、网络服务的名字只在 DEBUG**（`CLAUDE.md` 的「日志的分级」：虚拟网卡与应答器的行里的名字与地址只在 DEBUG）。
- 错误文本：命令失败经 `command::failure`（参数里的凭据已被遮住；这些命令没有凭据）；权限的错误照第 8.5 节；错误进 API 的 500 应答，所以同样不带地址以外的个人信息。
- `rurge-tun` 的计数照 M3 不变，经 `GET /v1/features/enhanced_mode` 给出；设备写不进的丢包计在设备自己的计数里（DEBUG 行与第一次的 WARN），不进 `dropped`（那是 `rurge-tun` 的原因表）。

## 14. 测试

**L1（缺省 `cargo test`，三个平台，主力）**：

- `rurge-platform`：路由集（第 4.7 节）；三个平台的步骤翻译（逐字）；输出的解析（夹具，含中文）；出口判定（表 × 地址的组合）；ICMP 的分派。
- `rurge-net` / `rurge-dns` / `rurge-engine`：出口在 `open_socket`、`DirectPacket`、QUIC 上；DNS 上游经连接器；`taken_over` 的去处（第 6.2 节的表逐行）；`try_update`；`start_tun` 的三条；`rurge-tun` 的三条。
- `rurge`（可执行文件）：源码扫描（第 7.2 节）；管理器的状态机（用文件后端，单元测试里直接驱动）；守护进程端到端（`rurge_process()` + `RURGE_ENHANCED_BACKEND=file:`，第 9.10 节的那张单子）；`rurge check` 的四个码。
- 不变的规矩：只用回环；缺省的 `cargo test` 从不执行地址、路由、DNS 的命令，从不建网卡（平台后端的执行函数没有任何用例调用；有一条用例断言起 `rurge` 的辅助函数缺省是 `off`）。

**L2（特权，只在一次性的虚拟机里，`RURGE_TEST_PRIVILEGED=1` 门控）**（M4-D27）：新的测试 crate `tests/privileged`（照 `tests/interop`、`tests/external` 的样子，工作区成员）：Linux 上 `unshare` 出新的网络命名空间，一对 veth 通到另一个命名空间里的服务端（"物理网卡"与"互联网"），在里面起 `rurge run --enhanced-mode`，断言：一个 TCP 往返与一次 ping 经虚拟网卡与 DIRECT 到达服务端、rurge 自己的 DNS 查询不进虚拟网卡（应答器的计数不涨）、`kill -9` 之后再起能恢复（`rp_filter` 写回）。**这些用例写好之后没有在任何地方运行过**，照实写；在 CI 上启用、macOS 与 Windows 的冒烟是总设计 Q4，归项目所有者。

**L3（手工）**：`docs/acceptance/phase3-manual.md` 的 M4 一节（第 16 节）；M3 一节里等 M4 的各项此时可以做。

**计时**：新的计时用例（`join` 的时限、"引擎放开客户端以 1 秒为界"）提交前单独循环跑二十次以上；等待都有时限。

## 15. M3 延后事项的去向

| 来源 | 内容 | 去向 |
| ---- | ---- | ---- |
| M3a 延后第 2、11、13 行 | 真实设备上的数字、零窗口探测、`STOP_GRACE` 与最后一轮的复位 | 手工验收（手工清单 M3 一节已有 W16、W17、M14、M15、L15、L16 与 P19、P20；M4 之后可以做） |
| M3a 延后第 17 行第二条 | 进片的队列按个数界、最坏约 393 MB | M4a：整个 TUN 一个排队字节数的计数（包进片的队列时加、出队时减），超过 `TunOptions::queue_bytes`（缺省 32 MiB，进 `check`）就按 `queue_full` 丢；`Device` 的约定写上"包按它的长度分配"（M4-D22） |
| 同上第三条 | `shard_of` 用固定键的散列 | M4a：每个 TUN 一个 `RandomState`（`BuildHasher::hash_one`） |
| 同上第四条 | 片重建的 WARN 不限频 | M4a：`stack_panics` 照数；WARN 每片每分钟至多一行，其余 DEBUG |
| M3b 延后第 4 行 | 真实的 `Pinger` | M4b（第 10 节） |
| M3b 延后第 6 行 | 未指定地址上的监听器的 `local` | 手工验收（P27 已有） |
| M3b 延后第 8 行与第 14 行的一条 | 停止不取消拨号；"引擎放开客户端"以 1 秒为界的证据 | M4a：等 `join` 至多 3 秒（第 9.4 节）；引擎用例以 `STOP_GRACE` 加余量为界 |
| M3b 延后第 23 ～ 25 行 | `fake-ip.txt` 的权限、`start_tun` 先检查选项、一个数据目录一次一个 TUN | M4a（第 3.4 节） |
| M3c 延后第 4 行 | 系统的 DNS 服务器名单排除 rurge 的地址 | M4b（第 6.3 节） |
| M3c 延后第 5、14、24 行 | 应答的延迟、一阵查询、M3 一节的手工验收 | 手工验收（P21、P22 与 M3 一节已有） |
| M3c 延后第 19 行 | R27：经监听器来的明文请求保留 fake 地址 | 阶段 4（M4-D24）：不做窄修。理由：浏览器一类按代理设置走的应用把名字交给代理、不先解析，拿着 fake 地址的绝对形式请求只来自先查了系统 DNS 再去连代理端口的应用，少见；窄修要在监听器的路径上再解析、改写一遍请求行与 `Host`，阶段 4 的 HTTP 引擎会把这件事整个做对。登记照旧 |
| M3c 延后第 27 行 | `dns_tcp_max` 的缺省 128 在真实设备上没有试过 | 手工清单加一项（M4-D25，M4d 的文档任务） |

## 16. 文档

M4a ～ M4c 各自的最后一个任务更新它碰到的文档（`docs/api/phase3.md`、`docs/testing.md`、`CLAUDE.md` 的 `unsafe_code` 一段与常用命令）；M4d 收尾时：

- 兼容性清单：9.1 节各行（增强模式、基础前缀与 rurge 空间、`tun-*` 与 `ipv6-vif`、开关入口、实现基础、权限）、6.3 节 `server:system` 一类（接管期间的去处）、`external` 的 `addresses`、10.3 节 `rurge run`（`--tun` → `--enhanced-mode`、`--wintun-dll`）、10.4 节 `/v1/features/enhanced_mode`；登记：非直连的 VPN 路由、外部程序与预匹配、`setsid` 的子进程、macOS 上多一次网络变化、Linux 改 `rp_filter`、没有 resolved 的 Linux 不接管 DNS、Windows 的多宿主解析与浏览器的 DoH（总设计已登记的照旧）。
- `docs/api/phase3.md`：M4 一节（开关的两个端点与 `GET` 的形状、启动参数与环境变量、日志行、`state.json` 的 `enhanced_backup`、手工恢复 macOS DNS 的办法 `networksetup -setdnsservers <服务> Empty`、测试后端）。
- `docs/acceptance/phase3-manual.md`：M4 一节——每个平台：开与关（命令行、API）、路由表的样子、系统 DNS 的接管与交还、强行结束之后再起的恢复（Linux `kill -9`、macOS `kill -9`、Windows `taskkill /F`）、防环（代理服务器、DNS 上游、资源下载都通，虚拟网卡的计数不因 rurge 自己的流量涨）、睡眠唤醒与换网络、接管暂停、`ping`、外部程序一律 DIRECT、`ipv6-vif` 的三种取值、`tun-excluded-routes` 与 `tun-included-routes`、与全隧道 VPN 共存；平台专有的：Windows 上开了 Hyper-V / WSL / ICS 的机器（第 7.4 节）、`wintun.dll` 缺失与签名不对、管理员与非管理员；Linux 上严格 `rp_filter` 的发行版（RHEL 一族）、有无 resolved；macOS 上多个网络服务与手动设过 DNS 的服务；与平台无关的：`dns_tcp_max`（M3c 延后第 27 行）、非直连的 VPN 路由的限制与它的办法。
- `docs/testing.md`：`RURGE_ENHANCED_BACKEND`、L2 的门控与跑法、扫描用例的名单怎样改。
- `CLAUDE.md`：`unsafe_code` 一段（新的 16 个函数）、常用命令（`--enhanced-mode`）、「现在在哪里」；`docs/status.md` 的 M4 一节；README 两份；需求文档第 7 节阶段 3 的状态行。

## 17. 写计划时要核对的事项

- V1：`rurge-net` 的 `SocketHook` 的全部实现（`NoopSocketHook`、`PlatformSockets`、测试里的假实现）、`open_socket` 的三个调用者、`bind_udp` 与 `direct_socket` 的调用者、`DirectPacket` 的用例；`rurge-dns` 里 `UdpUpstream::new` 在生产代码里的调用（`resolver.rs` 的 `build_upstream` 与 `build_plain_udp`）与它的用例；`ResolverDeps` 的构造点；`DirectConnector::new(SystemResolve)` 与 `resource_client()` 在生产代码里的每一处。
- V2：Windows `netsh` 的写法（`add address` / `add route` / `set interface` / `set subinterface` / `set dnsservers` 的参数名与 `store=active`、按序号指称网卡、`-f` 读脚本）、`show route` 的行的格式；macOS `networksetup` 与 `route`、`ifconfig` 的写法与输出（"There aren't any DNS Servers set on"、`-listallnetworkservices` 的第一行）；Linux `ip -batch` 在 `replace` 与 `-force` 下的行为、`resolvectl` 的子命令。只读文档与源码，不在开发机上执行。
- V3：`windows-sys` 0.61.2 里要用的符号各在哪个特性下（`IP_UNICAST_IF`、`IsProcessInJob`、`LoadLibraryExW` 与它的标志、`WinVerifyTrust` 与 `WINTRUST_DATA` 的字段、`WTHelperProvDataFromStateData`、`CertGetNameStringW`、`ConvertInterfaceLuidToIndex`、`WaitForSingleObject`）；`IP_UNICAST_IF` 与 `IPV6_UNICAST_IF` 的值的字节序；`WinVerifyTrust` 不联网的标志；Wintun 0.14 的函数签名（`wintun.h`）与适配器随进程移除的说法。
- V4：`libc` 0.2.189 的 `TUNSETIFF`、`IFF_TUN`、`IFF_NO_PI`、`ifreq`（Linux）与 `ctl_info`、`sockaddr_ctl`、`CTLIOCGINFO`、`AF_SYSTEM`、`SYSPROTO_CONTROL`、`UTUN_OPT_IFNAME`、`AF_SYS_CONTROL`（macOS）经 `nix::libc` 可用；`nix` 0.31.3 的 `getpgid` 在 `signal` 特性下可用（它带上 `process`）。
- V5：`State` 与 `Features` 今天怎样写、读（系统代理那一项）；`StateStore::update` 的每个调用者；`state.json` 的版本号要不要涨。
- V6：`run.rs` 的 `Takeover`、`Command`、`LoopControl`、`network_changed`、退出的次序、锁之后的恢复（`restore_what_a_crash_left` 一族）的现状；`Control` 与 `rurge-api` 的 `features.rs` 的现状；`rurge_command()` / `rurge_process()` 设了哪些变量。
- V7：`Engine::start_tun` 的调用者（今天只有用例）；`TunOptions::check` 的写法（新字段 `queue_bytes` 照它）；`dispatch.rs` 的 `shard_of` 与片的队列；片重建的 WARN 在哪里。
- V8：`rurge-proto` 的 `external` 里程序与进程组怎样保存（`Program`、`LocalPorts`）、谁持有 `LocalPorts`；引擎在哪里拿到会话的进程、在哪里开始评估规则（"一律 DIRECT"放在那里）。
- V9：解析器里 `HostAction::System(mode)`、`system_lookup`、`.local` 的分支（`resolver.rs` 约第 760 ～ 800、1040 ～ 1110 行）；`apply_system_servers` 在什么时候重读系统的服务器。
- V10：`rurge-config` 的诊断码表与 `codes::hint`；`ParamReader::has_unrejected`；`W0029` 的 `notes.inert` 对 `addresses` 的那一处（`spec/mod.rs` 约第 599 行）。
- V11：网络环境的快照里主网卡的 IPv6（地址、默认路由）怎样给出（`ipv6-vif = auto` 的判据）；`if_addrs` 0.15 的子网掩码字段；出口表里的网卡名与 `socket::bind_interface` 认的名字是同一种（Windows 上 `if_addrs` 给的是友好名，网络环境的主网卡名要与它一致）。

## 18. 对总设计与 M3 设计的订正

1. 总设计 3.3 节 macOS"单元号由 rurge 从小到大试，连上的那个就是 `utunN`，不必再读接口名" → 内核挑（`sc_unit = 0`），`UTUN_OPT_IFNAME` 读回名字（第 8.3 节，M4-D12）。
2. 总设计 9.2 节"基础前缀 `0.0.0.0/1` 与 `128.0.0.0/1`（`::/1` 与 `8000::/1`）"与 P3-D7 → 八个前缀，IPv6 另加 `64:ff9b::/96`（第 4.2 节，M4-D2）。
3. 总设计 9.2 节"fake 段与应答器单独装一条显式的路由" → 只在 macOS 显式装；Linux 与 Windows 由网卡的地址带出；求出的前缀落在里面的去掉（第 4.5 节，M4-D3）。
4. 总设计 9.2 节"Linux 把虚拟网卡的 `rp_filter` 放宽到 2" → 主网卡的有效值是严格时放宽 `all`（第 7.3 节，M4-D10）。
5. 总设计 9.4 节"macOS 上是没有验证过的假设……M4 的细化设计要带着退路" → 退路一开始就用上（八个前缀）；主机路由的退路不做（第 4.2 节）。
6. 总设计 9.4 节 Windows"两样都做" → 再加虚拟网卡自己关转发与 weak host send；不动别的网卡（第 7.4 节，M4-D11）。
7. 总设计 9.5 节"打开：权限检查 → 设备 → …" → 没有单独的权限检查（第 8.5 节，M4-D14）；次序见第 9.3 节（出口表在路由之前；Linux 的 `rp_filter` 在最后）。
8. 总设计 9.5 节"每一步之前把撤销它的步骤写进 `state.json`" → 只有会在崩溃之后留下的改动才写（macOS 的 DNS、Linux 的 `rp_filter`）；设备、地址、路由与虚拟网卡上的 DNS 随设备消失（第 9.1、9.2 节，M4-D5）。
9. 总设计 9.5 节"是否像系统代理一样只在启动参数里指定、不跨重启保留，在 M4 定" → 不跨重启保留（M4-D15）。
10. 总设计 9.3 节"接管期间的 `system`" → 细化：`syslib` 与单标签名字同样查物理网卡的服务器，没有服务器时失败、不退回系统解析库（第 6.2 节，M4-D7）。
11. 总设计 3.6 节"外部程序的进程树判断（Windows 上 Job Object，Unix 上取父进程或进程组）" → Unix 用进程组（`getpgid`，不用 unsafe），Windows 一个 unsafe 函数（第 11 节）。
12. 总设计第 13 节"虚拟网卡同样有一个内存后端供起守护进程的用例用"（M3 设计第 18 节第 9 条改到 M4） → `RURGE_ENHANCED_BACKEND`（第 9.10 节，M4-D19）。
13. M3 设计 3.3 / 9.1 节 `Engine::start_tun` 返回 `TunHandle`、对不合格的选项由 `Tun::start` panic → 返回 `Result`，先检查选项（第 3.4 节，M4-D23）。
14. M3-D8 的进片队列只按个数界 → 另有整个 TUN 的字节上限 `queue_bytes`（第 15 节，M4-D22）。

## 19. 已决事项

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| M4-D1 | 计划的拆分与执行 | M4a 骨架 → M4b 防环与接管期间的 DNS → M4c Linux 与 macOS → M4d Windows 与收尾；Pi 在一个会话里逐个任务做，每份计划第一个任务是开工前核对，做完一份停下交所有者；M4c 有一个等 CI 的停点（第 2 节） |
| M4-D2 | 基础前缀 | 三个平台同一种：IPv4 八个前缀，IPv6 八个加 `64:ff9b::/96`；总是减去回环、链路本地、组播、广播（第 4.2、4.3 节） |
| M4-D3 | rurge 的地址空间的路由 | Linux 与 Windows 由地址带出，macOS 显式装；求出的前缀落在里面的去掉；排除项不剥它；Linux 用 `route replace`（第 4.5、5.2 节） |
| M4-D4 | 步骤 | 抽象的动作 → 纯的翻译（不带 `cfg`，三个平台在任何平台上测）→ 带 `cfg` 的执行；Linux `ip -batch`、Windows `netsh -f` 与读回核对、macOS 逐条（第 5 节） |
| M4-D5 | 备份 | 只记会在崩溃之后留下的（macOS 的 DNS、Linux 的 `rp_filter`）；类型化的撤销动作，从不存命令行；`try_update` 写不进就不做（第 9.2 节） |
| M4-D6 | 系统 DNS | P3-D8 不变；Linux 以 `/etc/resolv.conf` 是 resolved 的存根为判据；macOS 每个启用的服务、网络变化时新出现的也接管；各平台清缓存（第 5、6.1 节） |
| M4-D7 | 接管期间的系统 DNS | `SystemDns::enhanced_mode`（增强模式开着就为真，不只是接管了系统 DNS 时）；`system` / `syslib` / 单标签名字查物理网卡的服务器、没有就失败；`force-syslib` 与 `.local` 照旧交系统解析库；物理服务器的读法按平台；macOS 上多一次网络变化，登记不处理（第 6.2、6.3 节） |
| M4-D8 | 出口的判定 | 纯函数；不绑 / 直连子网的网卡 / 主网卡 / 没有主网卡则失败；不读系统的整张路由表，非直连的 VPN 路由的限制登记（第 7.1 节） |
| M4-D9 | 一处建法 | 出站套接字只在 `open_socket`；DNS 纯 UDP 上游经直连连接器；资源下载与 Profile 来源带钩子、接管期间名字经 rurge 的解析器；源码扫描 + 运行时用例（第 7.2、7.6 节） |
| M4-D10 | Linux 的 `rp_filter` | 单表最长前缀 + `SO_BINDTODEVICE`，不用策略路由；主网卡的有效值是 1 时把 `all` 设成 2、进备份、关闭时写回（第 7.3 节） |
| M4-D11 | Windows 的防环 | 绑源地址 + `IP_UNICAST_IF` + 虚拟网卡自己关转发与 weak host send；不动别的网卡；sing-tun 的做法记作退路；真机核对（第 7.4 节） |
| M4-D12 | 设备 | Linux `rurge`、不持久；macOS 内核挑单元号、读回名字；Windows 名字 `rurge`、固定 GUID、先开后建、环 8 MiB、读线程；MTU 1500，不卸载；写不进就丢；一台机器一个增强模式（第 8 节） |
| M4-D13 | Wintun 的加载 | P3-D17；`--wintun-dll` / `RURGE_WINTUN_DLL` 或可执行文件旁边；签名者恰好是 `WireGuard LLC`，校验不联网（第 8.4 节） |
| M4-D14 | 权限 | 不预先检查，第一个权限错误映射成一句英文说明，不提权（第 8.5 节） |
| M4-D15 | 开关 | `--enhanced-mode` / `RURGE_ENHANCED_MODE` 与 API；不跨重启保留（第 9.6 节） |
| M4-D16 | 次序 | 打开：设备 → 配网卡 → `check` 与 `start_tun` → 出口表 → 路由 → 系统 DNS → `rp_filter`；关闭倒过来，等 `join` 至多 3 秒；退出时增强模式先于系统代理（第 9.3、9.4 节） |
| M4-D17 | IPv6 | `rurge-tun` 总以 `ipv6: true` 起；`ipv6-vif` 只管地址、路由、DNS；IPv6 一批失败只告警、按没开处理（第 4.6、5.2 节） |
| M4-D18 | 接管暂停 | 照系统代理一半的语义：进入时关、记下想要；离开时照想要的打开；暂停中的 `POST` 只记下（第 9.9 节） |
| M4-D19 | 测试后端 | `RURGE_ENHANCED_BACKEND=file:<路径>` / `off`；起 `rurge` 的辅助函数缺省 `off`（第 9.10 节） |
| M4-D20 | 真实的 Pinger | `socket2` 的 ICMP 套接字，按出口绑定；Linux DGRAM 不行退 RAW，macOS DGRAM，Windows RAW（第 10 节） |
| M4-D21 | 外部程序 | `addresses` 进路由的排除、不再报 `W0029`；进程树里的会话一律 DIRECT；Unix 进程组、Windows Job Object；预匹配先于它的限制登记（第 11 节） |
| M4-D22 | `rurge-tun` 的三条 | 整个 TUN 的排队字节上限 `queue_bytes`（缺省 32 MiB）；每个 TUN 一个 `RandomState`；片重建的 WARN 每片每分钟一行（第 15 节） |
| M4-D23 | `start_tun` | 返回 `Result`；先检查选项再开池；同一个引擎第二个 TUN 是错误；池的文件在 Unix 上 `0600`（第 3.4 节） |
| M4-D24 | R27 | 不做窄修，阶段 4（第 15 节） |
| M4-D25 | `dns_tcp_max` | 手工清单加一项（第 15、16 节） |
| M4-D26 | 诊断 | `W0035` ～ `W0038`（第 12 节） |
| M4-D27 | L2 | Linux 网络命名空间里的特权用例写好、门控；不在 CI 上启用（Q4 归所有者）（第 14 节） |
| M4-D28 | 日志 | INFO 只说开关、恢复、`rp_filter` 与条数；名字、地址、路由、DNS 服务器、网络服务只在 DEBUG（第 13 节） |
| M4-D29 | unsafe | 新增 16 个，全在 `rurge-platform`，名单与 `CLAUDE.md` 同步（第 3.8 节） |
| M4-D30 | 依赖 | 不加包；`rurge-platform` 新加 `tokio`、`ipnet` 两条边（工作区已有），`windows-sys` 多开几个特性，`nix` 不开新特性（第 3.1 节） |

## 20. 留给项目所有者的

本文没有定、去向是项目所有者的（后来的会话与 Pi 都不要自己定）：

| 问题 | 与 M4 的关系 |
| ---- | ------------ |
| 总设计 Q2：设备、路由、网络环境改用现成的包（`tun-rs`、`rtnetlink`、`wintun-bindings` 等），或开 `nix` 的 `socket` / `net` 特性 | M4 照 P3-D3 自己写；换的话只动 `rurge-platform` |
| 总设计 Q3：发行包附带官方签名的 `wintun.dll`，还是要使用者自己取 | M4 照"使用者放在可执行文件旁边或用 `--wintun-dll` 指"做，错误文字指向 wintun.net |
| 总设计 Q4：L2 特权用例在 CI 上启用；macOS 与 Windows 的冒烟（要在 CI 里取 `wintun.dll`） | M4c 写好 Linux 的 L2，不改 CI |
| 总设计 Q5：Windows 防 DNS 泄漏的严格模式（WFP） | M4 不做，泄漏照旧登记 |
| 总设计 Q1、Q6、Q7、Q8 | 与 M4 无直接关系，照旧开着 |

另有两件事要项目所有者做：**推送**（M4c 的 Linux / macOS 代码只有 CI 能编译，M4c 计划里有一个等 CI 的停点）；**手工验收**（M4 之后增强模式第一次能在真机上开，手工清单 M3、M4 两节都要真机）。

所有者可以改的裁定：本文 M4-D1 ～ D30，尤其 M4-D10（Linux 上改全机的 `rp_filter`）、M4-D11（Windows 不动别的网卡）、M4-D2（八个前缀，`0.0.0.0/8` 与 `::/8` 的大部分不进虚拟网卡）与 M4-D24（R27 不做窄修）。

## 21. 写计划时的订正

四份计划（`docs/superpowers/plans/2026-10-26-phase3-m4a-skeleton-plan.md`、`…-m4b-loop-dns-plan.md`、`…-m4c-linux-macos-plan.md`、`…-m4d-windows-wrapup-plan.md`）在写的时候对着文档与源码核对，改了正文下面这些地方。每一条的理由与出处在那份计划的「计划期决定」一表里；正文没有逐处加指到这里的括号，读第 2、3、5、8、9、16 节时对照本节。执行期的改动记在各计划的「执行期修正记录」里，M4d 收尾时另写一节。这些改动同样是代项目所有者作的裁定，所有者可以改。

**Windows（第 3.8、5.4、8.4 节）**

1. 第 5.4 节的 `netsh` 写法 → 按微软文档带参数名写（`name=` / `interface=`、`address=`、`mask=`、`source=static`）；IPv6 的哪些命令用 `interface=`、哪些用 `name=`，见 M4a 计划 P4 与 M4d 计划 P20。
2. 第 5.4 节的接口度量与路由度量 `0` → `1`；路由不写下一跳 `0.0.0.0`（M4a 计划 P2、P3）。
3. 第 5.4 节 `set interface` 一行里同时设度量、转发与 weak host → 拆成两行，度量单独一行：现行文档的 `set interface` 没有 `metric`，它不被接受时只丢度量（M4d 计划 P9）。
4. 第 5.4 节"读回核对"只读路由表 → 地址、MTU、DNS 服务器也读回。`netsh` 的退出码一律不看，成败只由读回定。缺的路由在错误里只写条数，前缀在 DEBUG（M4-D28）（M4d 计划 P10、P11）。
5. 第 8.4 节与 M4-D12 的"先 `WintunOpenAdapter` 复用残留的，没有再建"、第 5.4 节的"重用残留的网卡时被重新设置" → 先探后建、不复用：`WintunOpenAdapter` 找得到 `rurge`，就答"正在被使用"。理由：Wintun 用 `SwDeviceCreate` 建适配器、不改它的寿命，所以它在建它的进程结束后不再被枚举；复用来的适配器又永远不会被移除（M4d 计划 P8；"不再被枚举"是推论，手工清单核对）。
6. 第 8.4 节校验签名者 → 校验的是以只许读的共享方式开着的那个文件（`WINTRUST_FILE_INFO` 的 `hFile`），一直开到加载完（M4d 计划 P7）。
7. 第 8.4 节关闭的次序"置停止标志、等读线程结束" → 两步之间先关通道的接收端，让卡在满通道上的读线程醒来（M4d 计划 P13）。
8. 第 3.8 节 `in_job` 包 `OpenProcess` + `IsProcessInJob` → 中间加一次零等待的 `WaitForSingleObject`：已经结束的进程不算在里面（M4d 计划任务 1）。

**Linux 与 macOS（第 5.2、5.3、8.1、8.2 节）**

9. 第 5.2 节与 M4-D3 的"Linux 用 `route replace`" → `route add`。同样的前缀已经在时（例如另一个全隧道 VPN），打开失败、说清原因，不换掉别人的路由（M4c 计划 P3、P22）。
10. 第 5.2 节 `resolvectl default-route` 失败 → 不算错（M4c 计划 P4）。
11. 第 5.3 节 macOS 的 IPv6 地址 `prefixlen 64` → `prefixlen 128 alias`（M4c 计划 P5）。
12. 第 5.3 节与第 16 节的 `networksetup -setdnsservers <服务> Empty` → 小写 `empty`（M4c 计划 P6）。
13. 第 5.3 节补：读不了原值的服务不接管（WARN 一行）；一个服务也接管不了时，按没有接管系统 DNS 处理（M4c 计划 P8）。
14. 第 8.1 节 `TunDevice::name()` → `adapter()`，给名字与序号（M4c 计划任务 3）。
15. 第 8.2 节"缓冲 MTU 大" → 64 KiB，大于 MTU 的包不会被截断成别的样子（M4c 计划 P14）。

**共同的（第 3、5.5、9 节）**

16. 第 5.5 节"执行一组步骤与读系统的状态带 `cfg`" → 系统一侧（`LinuxSystem`、`MacSystem`、`WindowsSystem`）不带 `cfg`：程序经 `CommandRunner` 给进来，文件经根目录给进来，在开发机上用假的执行器测。设备、`unicast_if`、`in_job` 与 `system()` 里挑平台的几行仍带 `cfg`（M4c 计划 P2，M4d 计划的「架构」）。
17. 第 3 节 `netenv` 一行的"`ReadOpts` 多一项要排除的 DNS 前缀" → 不加：rurge 的空间是常数，在共同的路上去掉；`exclude` 只放虚拟网卡的名字（M4b 计划 P12）。
18. 第 3.4 节"`start_tun` 先检查选项" → 也拒绝 MTU 小于 1280 的设备（M4a 计划 P8）。
19. 第 9.1 节后端的接口：`backend()` → `backend(data_dir, wintun_dll)`；`Backend` 加 `physical_dns`（M4c 计划 P13，M4d 计划任务 6）。
20. 第 9.2 节"全部撤销之后用 `try_update` 清掉备份" → 去掉用 `update`，写不进只记 ERROR（M4a 计划 P11）。
21. 第 9.3 节补：启动时增强模式先于系统代理打开；系统代理打开失败、以 1 退出之前，先关掉增强模式（M4a 计划 P25）。
22. 第 9.4 节补一个情形：设备自己坏了（TUN 以 `Ending::DeviceFailed` 结束）时，管理器把增强模式整个关掉（交还 DNS、撤销会留下的），ERROR 一行，不重试，`Takeover` 记的"想要"不变（M4a 计划 P24）。
23. 第 9.10 节测试后端的设备 `MemoryDevice` → 一个空设备，从不送包、丢掉写来的包（M4a 计划 P19）。
24. 第 16 节"`CLAUDE.md` 的常用命令加 `--enhanced-mode`" → 不加：它改系统的路由与 DNS，以后的会话不应当照着手工跑；`CLAUDE.md` 收尾时只改「现在在哪里」与 `unsafe_code` 一段（M4d 计划任务 9）。
25. 第 2 节与 M4-D1 的"M4c 有一个等 CI 的停点" → M4d 也有一个（它的任务 8）：M4d 不带 `cfg` 的改动在 Linux 与 macOS 上同样只有 CI 编译（M4d 计划 P18）。

## 22. 执行期的订正

四份计划（`…-m4a-skeleton-plan.md`、`…-m4b-loop-dns-plan.md`、`…-m4c-linux-macos-plan.md`、`…-m4d-windows-wrapup-plan.md`）的「执行期修正记录」在执行期改了正文下面这些地方；写法照第 21 节，每条末尾写明是哪份计划的第几行。正文不改。这些同样是代项目所有者作的裁定，所有者可以改。

**Windows（第 5.4、6.3、8.4、9.5 节）**

1. 第 5.4 节的 `set dnsservers` → IPv4 与 IPv6 两张名单分开改：IPv6 没开时另跑一行 `address=none` 清掉 IPv6 的那一份（否则网卡上不再拥有的地址还在被问），读回也分两张（M4a 记录第 96 行、M4d 记录第 62 行）。
2. 第 13 节只有"IPv6 未配置"一行 WARN 的读法 → 分成三处说：地址那一半由管理器写 `IPv6 could not be configured; the enhanced mode goes on without it`，路由那一半由 `run_routes` 写 `the IPv6 routes could not be added to the virtual adapter; they are left out`，系统 DNS 的 IPv6 那一半写 `the system's IPv6 DNS could not be pointed at the enhanced mode; it is left as it is`；标 `Ipv6Off` 的步骤本身失败只写 DEBUG（M4c 记录第 29 行、M4d 记录第 13、23、62 行）。
3. 第 8.4 节与第 9.5 节的"先 `WintunOpenAdapter` 复用残留的适配器" → 只是探一下：探到就答"正在被使用"，不复用（第 21 节第 5 条改了 8.4 与 M4-D12，第 9.5 节末句同样作废；M4d 记录第 68 行）。
4. 第 8.4 节的 `send` 只由"环满"决定丢包 → 先检查包长不超过 `wintun.h` 的 `WINTUN_MAX_IP_PACKET_SIZE`（65,535）；超长的在放开的函数之前就以 `a packet for the virtual adapter is longer than 65535 bytes` 拒绝（用户态不查的话，驱动会把整个发送环当成坏的）（M4d 记录第 73 行）。
5. 第 8.4 节与 M4-D13 的 `wintun.dll` → 要 0.14 或更新：0.13 没有 `WintunCloseAdapter`，缺入口就以 `wintun.dll has no <入口>: is it wintun.dll?` 被拒；加载之前的错误带自己的上下文（`cannot find wintun.dll (…): get it from https://www.wintun.net/`、`cannot open wintun.dll: …`、`wintun.dll is not signed …`、`wintun.dll is not signed by WireGuard LLC (it is signed by …)`），不经第 8.5 节那句权限的映射（M4d 记录第 59 行）。
6. 第 9.4 节的"关设备" → 关在最后一个 `Arc` 放掉时（`Drop`）：正常的关闭经 `call`（`spawn_blocking`），TUN 3 秒内没结束的那一种最后由 TUN 的任务放掉、`Drop` 占住一个工作线程；次序是置停止标志 → 关通道接收端（第 21 节第 7 条）→ 等读线程 → `end_session` → `close_adapter`，`open` 与 `start` 用同一枚停止标志（M4d 记录第 29、72 行）。
7. 第 13 节 Windows 的日志 → 加载好的那一行 DEBUG `wintun.dll is loaded` 带模块句柄，它也是 `Wintun::module` 唯一的生产读者；读回的每一步另有 `reading back a setting of the enhanced mode` 与 `what the readback printed`（M4d 记录第 69 行）。
8. 第 9.6 节的 `--wintun-dll` 在别的平台上是用法错误 → 文字固定为 `--wintun-dll is only for Windows`，退出 2，在 `Overrides::parse` 与读配置之前；`--help` 与别的路径选项一样连 `RURGE_WINTUN_DLL` 的值一起显示（M4d 记录第 74、75、76 行）。

**Linux 与 macOS（第 5.2、5.3、6.1、6.3、7.3、8.5、9.5、9.8 节）**

9. 第 8.5 节 Linux 的 "root or CAP_NET_ADMIN" → `the enhanced mode needs root (in a container, root with CAP_NET_ADMIN)`（M4c 记录第 46 行）。
10. 第 5.2 节接管系统 DNS 的四步之外 → 先把 rurge 链路上的 LLMNR、mDNS、DNSSEC、DNSOverTLS 各设成 `no`（都是尽力而为，老的 resolv.conf 全局设置会让发往应答器的查询走 853 的 TLS），再设服务器；`flush-caches` 失败不算错（M4c 记录第 45 行）。
11. 第 6.1 节的读法 → `/etc/resolv.conf` 尽力读：认识的选项照用，不认识的合成一行 DEBUG，不让一份名单因一个选项变空（M4c 记录第 19 行）。
12. 第 5.3 与第 6.3 节 macOS 的空名单 → `-getdnsservers` 输出全空是 `Some(空表)`（写回用 `empty`），不是"读不到"；接管之前的 `/etc/resolv.conf` 里只有应答器一类的空名单当作"不知道"，往下读服务原值与 DHCP（M4c 记录第 25、47 行）。
13. 第 6.3 节的 `physical` → Linux 是 resolved 的上游名单，macOS 是接管之前的服务器、服务原值与 `ipconfig getpacket`；读的那一步没回来是 `Some(空)`（查找失败），`None` 只表示"机器自己的列表是对的"（Windows、文件后端没有那个键、`off`、Linux 还没接管）（M4c 记录第 31、33 行）。
14. 第 5.2 与第 6.2 节"做了一半的接管" → 也算接管过：`take_dns` 先记标志、再跑步骤，失败时 `undo` 能把已改的还回去（M4c 记录第 42 行）。
15. 第 7.3 节、第 9.3 节第 7 步与第 9.8 节"打开时与每次网络变化时" → `rp_filter` 只在需要时放宽：这一次 `adjust_undo` 回了 `Some`、或备份里已有那一项时（不需要的机器既不改、也不留下没有备份的改动）（M4c 记录第 40 行）。
16. 第 9.5 节的恢复位置 → 多一处：`wait_for_the_sources` 在拿到锁之后、下载 Profile 的远程来源之前也恢复一次（macOS 崩溃留下的 DNS 还指着应答器时，那些下载要等到时限）（M4c 记录第 32 行）。
17. 第 5.2 节带脚本的步骤失败时的错误 → 脚本的路径每一处（命令行的与工具原文里的）换成 `…`，路径只在 DEBUG（M4c 记录第 44 行）。

**共同的（第 7、9、10、11、13、14 节）**

18. 第 9.7 节 IPv6 从开到关的"反之" → 次序是"先交还系统 DNS、再按差减路由、最后撤地址"（网卡还有那个地址时交还最稳）（M4a 记录第 79 行）。
19. 第 9.7 节路由的 IPv6 一半失败（`routes` 回 `Ok(false)`）→ 别当空集：记下的是旧集减去要减的（后端只撤新加的那一半），出口表换成它；这一次之前 IPv6 没开着的这一次按没开往下做（M4a 记录第 95 行）。
20. 第 9.6 与第 9.9 节的关闭 → `POST false` 不论成败都把"想要的"记成关（关从不半途停下，回 500 只表示某条撤销没成）；关闭失败不影响"想要"（M4a 记录第 97 行）。
21. 第 9.2 与第 9.5 节的恢复 → 备份为空而 `features.enhanced_mode` 为真时（Windows 的备份总是空、Linux 不用改 `rp_filter` 时也是），恢复把它写回 `false`（M4a 记录第 99 行）。
22. 第 9.4 节的"关闭从不停在半路" → 等 TUN 用的 `ending()` 只看"被告诉结束"（停止或设备坏了），不等 TUN 的任务；关闭仍等 `join` 至多 3 秒（M4a 记录第 100 行）。
23. 第 13 节的 ERROR 行 `step` → 只写种类（`mac_dns` 不带服务名，服务名在紧跟的 DEBUG 行），因为它可以是使用者改的任何文字（M4a 记录第 98 行）。
24. 第 7.4 节的 `IP_UNICAST_IF` → 设不上只写一行 DEBUG、连接照走源地址绑定的那一层，不因此失败（M4d 记录第 39 行）。
25. 第 10 节的 `Pinger` 载体 → 发送失败的载体按 `Arc` 指针从表里移出（下一次回显新建一个），它的接收任务**退休**：再读一个回显时限（TUN 的 `echo_timeout`）才停，同时退休着的至多 4 个；Linux 的 ICMP 数据报套接字只收得到自己那些请求的回复，所以已经发出、正在等回复的回显得靠它收（M4d 记录第 48 行，由终审后的 R24 改成最终的样子，第 88 行）。
26. 第 7.1 节的直连子网 → 同一个子网挂在两块网卡上时留输入里先出现的那一块；`decide` 先把目的地址规范化（`::ffff:a.b.c.d` 按 IPv4 判）（M4b 记录第 13、15、71 行）。
27. 第 7.2 节"活得比一次会话长的套接字" → 直连连接器的 `route()` 说它按哪一张出口表绑；WireGuard 的载体每个 `TICK`（250 毫秒）、QUIC 的连接每次取用都比一次，不同就重拨（INFO 一行）；管理器只在表的内容变了时才换 `Arc`（M4b 记录第 68、69 行）。
28. 第 7.2 与第 7.6 节的重读 → `reread_system_servers` 也清 `[Host]` 的 `server:` 上游与经接口的上游；跟随写了另一张表而没有重新接管 DNS 时也调一次 `system_dns_changed`；启动时等 Profile 来源的那一次下载仍用 `resource_client()`（M4b 记录第 67、78 行）。
29. 第 7.2 节的扫描用例 → 与名单**逐处**比对（同一个文件里同一种写法的第二处也要一行），认的写法扩到 15 种，名单 25 行（M4b 记录第 73 行）。
30. 第 11 节的外部程序 → 程序拉起的子进程算在里面（Unix 的进程组、Windows 的 Job Object，`setsid` 出去的不算）；`runs(pid)` 也认程序自己的 PID；程序在跑时直连与全局模式的会话也等一次进程（没有程序在跑时一个字也不多等）（M4b 记录第 47、51 行、M4d 记录第 42 行）。
31. 第 14 节的 L2 → 服务端放在只经默认路由可达的地址上（`10.231.1.1/32`）：rurge 的基础前缀比内核的直连路由短，直连网段上的流量不进虚拟网卡（M4c 记录第 34 行）。

**终审之后（第 4.3 节）**

32. 第 4.3 节“总要减去的” → Windows 上另减 `240.0.0.0/4`：Windows 所有版本缺省都挡这一段的流量（草案 draft-schoen-intarea-unicast-240 的 Windows 一段；`mstcpip.h` 的 `Ipv4AddressType` 把它判成 `NlatInvalid`），`netsh` 加不加得进没有人核实过；`RouteInputs.class_e` 为假（Windows）时 `RouteSet::compute` 在最后把它挖掉，包含项也带不回来（缺省的 Profile 下 27 条，Linux 与 macOS 仍是 55 条）（M4d 记录第 87 行）。

**推送之后（M4c 任务 10、M4d 任务 8）**

33. 第 2 节与 M4-D1 的“M4c 有一个等 CI 的停点”（与第 25 条加的 M4d 的那一个）→ 两个停点合成一次做完：运行 40611702392（提交 `3a76b80`，2026-10-28）三个平台的 `check` 与 `bench` 都绿，Linux 与 macOS 专有的代码第一次被编译、缺省用例通过，没有要修的；缺省的用例不建网卡、不改系统设置，设备与系统改动照旧只能由手工验收（第 16 节）与 L2（第 14 节）验证（M4c 记录第 51 行、M4d 记录第 91 行）。
