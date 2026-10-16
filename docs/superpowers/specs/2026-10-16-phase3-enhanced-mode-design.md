# 阶段 3「增强模式（TUN）」设计文档

| 项       | 内容 |
| -------- | ---- |
| 日期     | 2026-10-16 |
| 状态     | 草案。已决事项（第 16 节 P3-D1 ～ D24）是在项目所有者"自己决定、继续做"的常设授权下定的，所有者可以改 |
| 对应需求 | [需求文档](../../requirements.md) 第 7 节「阶段 3」；FR 编号见各节 |
| 兼容基线 | [Surge 兼容性清单](../../surge-compatibility-matrix.md) 第 2、3、5、6.4、9.1、9.4、9.10 节；手册文本取自 2026-10-14（Surge Mac 6.10.0 / iOS 5.23.0） |
| 前置     | 阶段 1、2 的里程碑都已合并（两个阶段的验收都没有完成：CI 没有绿过，手工清单一项没做）；手册基线重对、一行怎样拆分已合并 |
| 调研     | [`docs/reports/2026-10-16-phase3-manual-recheck.md`](../../reports/2026-10-16-phase3-manual-recheck.md)（阶段 3 那几节对应的手册页对着清单逐行重读，重对计划延后第 7 行）；[`docs/reports/2026-10-16-phase3-tun-survey.md`](../../reports/2026-10-16-phase3-tun-survey.md)（开源实现与三个平台的接口） |

本文档是阶段 3 的总设计：定目标与范围、读手册得到的事实与需求文档的出入、技术选型、crate 边界、跨里程碑共用的抽象、里程碑拆分、测试策略与验收。每个里程碑开工前再各写一份细化设计与实施计划（同阶段 2）；包的格式、命令的逐条写法、API 的 JSON 形状、常数的取值在那里定稿，不在本文档展开。

两份调研都是只读的助手写的（读手册文本、公开网页与本机 cargo 注册表里的源码，什么也没有运行）。调研报告里多数网页是经摘要读到的，报告自己写明数字（偏移、常数、基准）要在用到之前对着原文再核一遍；本文档引用它们时照此对待。

## 1. 目标与范围

### 1.1 阶段目标

接管全部系统流量，不遵守代理设置的应用也经规则分流（需求文档第 7 节）。落到使用上：`rurge run --enhanced-mode` 之后，浏览器、命令行工具与不理会系统代理的应用的 TCP、UDP 都出现在请求记录里并按规则走对应的策略；应用查到的是 fake IP，连接到来时按原域名匹配规则；`ping` 照常；换网络之后 30 秒内恢复；`PROCESS-NAME` 在三个平台上命中。

### 1.2 范围内（引用需求文档编号）

| 领域 | 需求 |
| ---- | ---- |
| 入站 | FR-IN-06（增强模式：三平台 TUN、路由、`tun-included-routes` / `tun-excluded-routes` / `ipv6-vif`、ICMP、用户态协议栈、权限提示、退出时恢复）、FR-IN-07（进程识别）、FR-IN-08（UDP 会话表、`udp-priority`）、FR-IN-09（网络环境探测与网络变化事件）、FR-IN-10（`[SSID Setting]`） |
| 出站 | FR-OUT-11 的 TUN 部分（`block-quic` 在增强模式下生效） |
| 策略组 | FR-GRP-02（`subnet` 组） |
| 规则 | FR-RULE-01 的 `PROCESS-NAME` 与 `SUBNET`、FR-RULE-08 的 QUIC / STUN 识别；预匹配在 DNS 与 SYN 层（需求文档第 7 节阶段 3 的范围） |
| DNS | FR-DNS-05 的 `force-syslib`、FR-DNS-08（fake-IP 应答器）、FR-DNS-09（`[SSID Setting]` 的 DNS 覆盖） |
| 包改写 | `[IP Rewrite]`（兼容性清单 9.10 节标阶段 3；需求文档第 4 节没有对应的行，本设计归在 FR-IN-06 的包处理之下，要不要另立一行由项目所有者定，第 16 节 Q8） |
| 控制面与命令行 | `GET` / `POST /v1/features/enhanced_mode`（清单 10.4 节，阶段 1 / M4a 起 `GET` 恒 `false`、`POST` 恒 501）；`rurge run --enhanced-mode` |

### 1.3 范围外

| 项 | 去向 |
| -- | ---- |
| 网关模式、DHCP、`DEVICE-NAME` / `MAC-ADDRESS`、UDP Fast Path、端口转发 | 阶段 7。本阶段的应答器与包处理不排斥它们（同一个应答器、同一条包的路径），不为它们做任何东西 |
| `network-changed` 事件脚本、脚本的 `$network` 与 `$request.processPath` | 阶段 5。本阶段产出它们要的数据（网络快照、进程路径） |
| Logbook、CLI 的 `feature` 与 `dump virtual-ip` / `dump performance` | 阶段 6。本阶段写日志行 |
| 基于 SNI 的规则匹配（`extended-matching`）、拨号之前的首包嗅探、`PROTOCOL,HTTPS` 的拨号前匹配 | 阶段 4。本阶段的 TUN 会话照监听器的做法：拿到连接就拨号，SNI 只在转发时记录（第 8.2 节） |
| `cellular-mode`（计费网络模式） | 远期（清单 9.4 节） |
| Windows 上防 DNS 泄漏的严格模式（WFP 阻断别的接口的 53 端口） | 开放问题（第 16 节 Q5） |
| `tailscale` | 远期 |
| iOS 专属的键（`compatibility-mode`、`include-*`、`auto-suspend`、`cellular-fallback` 等） | 照旧解析并忽略（清单已登记） |

### 1.4 里程碑拆分

阶段 3 拆成六个可独立验收的里程碑。顺序依据两点：**不需要虚拟网卡、在开发机上就能完整测试的部分在前**（网络环境、进程识别——它们对今天经系统代理进来的流量就有用）；**虚拟网卡的内核在内存里的设备上做完、测完，再接真实的平台设备**（开发机上不能建网卡、不能改路由，平台部分只能由 CI 与手工验收覆盖，第 13 节）。

| 里程碑 | 内容 | 需求 | 可验收的产出 |
| ------ | ---- | ---- | ------------ |
| **M1 网络环境与子网** | `rurge_platform::netenv`（三平台的网络快照与变化事件）；引擎对网络变化的下游（DNS 缓存与 AAAA 抑制、测速结果失效、WireGuard 重建、日志）；`subnet` 组（含临时覆盖）与 `SUBNET` 规则；`[SSID Setting]`（解析、`dns-server` / `encrypted-dns-server` 覆盖、接管暂停的系统代理一半、`tfo-behaviour` 只解析）；起守护进程的用例用的网络环境测试后端 | FR-IN-09、FR-IN-10（非 TUN 部分）、FR-GRP-02、FR-DNS-09、FR-RULE-01（`SUBNET`） | 假探测器驱动的网络变化端到端；`subnet` 组与 `SUBNET` 规则按快照选；DNS 覆盖生效与撤销；三平台快照读取的解析（文本夹具） |
| **M2 进程识别** | `rurge_platform::procinfo`（三平台）；引擎在规则之前给本机来源的会话填进程；请求记录带进程 | FR-IN-07、FR-RULE-01（`PROCESS-NAME`） | 经监听器从回环来的会话命中 `PROCESS-NAME`：三平台的 CI 上各查一次测试进程自己 |
| **M3 TUN 内核（内存里的设备）** | 新 crate `rurge-tun`：设备抽象与内存设备、包的去向、TCP（smoltcp 分片，第一个任务是基准门槛，第 3.1 节）、UDP 会话表、ICMP 转发、DNS 应答器与 fake-IP（`rurge-dns` 的池、映射的持久化与查询转发）、预匹配在 DNS 与 SYN 层、引擎接入 | FR-IN-06（协议栈部分）、FR-IN-08、FR-DNS-08 | 内存设备上的端到端：TCP、UDP、DNS、ping 经引擎按规则走到回环上的上游；基准数字 |
| **M4 平台接入** | 三平台的设备、地址与路由、系统 DNS 的接管与恢复、防环（rurge 自建的每个出站套接字收到一个统一的建法上）、开关（命令行、API）与生命周期、权限提示、崩溃之后的恢复、接管暂停的 TUN 一半、`external` 的 `addresses`（`W0029` 退役）与外部程序一律 DIRECT、`force-syslib`、`ipv6-vif = always` 的告警 | FR-IN-06（其余）、FR-IN-10（TUN 部分）、FR-DNS-05 | 命令序列（计划）对三平台的数据用例；手工验收；Linux CI 上的特权用例（是否在 CI 上启用见第 16 节 Q4） |
| **M5 识别、阻断与 `[IP Rewrite]`** | QUIC（443 以外的端口）/ STUN 识别、TUN 上的 `block-quic`、`[MITM]` 的 `auto-quic-block`（MITM 之前按 `hostname` 名单判断目标域名）、`udp-priority`、`[IP Rewrite]`、内部 DNS 会话带上 DNS 服务器的主机名（清单 L519）、`.local` 答案里带作用域号的 IPv6（清单 L501，做不到就登记延后）、性能基准对 NFR-01 | FR-RULE-08、FR-OUT-11（TUN）、FR-IN-08（`udp-priority`） | 内存设备上的用例；基准对 NFR-01 的 70% |
| **M6 收尾与验收** | 阶段 3 的需求逐句对照代码（照阶段 1 的做法，一句一行）、验收文档、手工清单、兼容性清单、README | — | 第 14 节 |

M3 体量最大，细化设计时多半拆成三份计划：M3a（设备抽象、包的去向、TCP 与基准门槛）→ M3b（UDP 会话表、ICMP、引擎接入）→ M3c（应答器与 fake-IP、预匹配）。M1、M4 也可能各拆两份。

## 2. 重读手册的结果，与需求文档的出入

### 2.1 重读的结论

阶段 3 开始时把阶段 3 那几节对应的手册页对着清单逐行重读了一遍（报告 [`2026-10-16-phase3-manual-recheck.md`](../../reports/2026-10-16-phase3-manual-recheck.md)）：清单里阶段格含 3 的 65 行，39 行与手册一致，25 行"手册多说了"（缺省、取值、语义、版本、平台、限制），**没有一行与手册相冲**，1 行是 rurge 自己的东西（手册不涉及）；有四处清单把自己的读法写得比手册的话更肯定（L266、L334、L712、L741，行号随清单改动会移动）。报告第 3 节是阶段 3 碰到的全部配置键与常量，第 4 节是手册写明的行为，第 5 节是 28 个手册含糊或 Mac 与 iOS 写法不同的地方（Q-01 ～ Q-28），附录 B 是需求文档与手册对不上的句子。这一遍同上一轮一样**没有观察过 Surge 的实际行为**。

"手册多说了"的 25 行在实现它们的里程碑里改清单（第 17 节）；四处写得过于肯定的措辞在 M1 的文档任务里一起订正。

### 2.2 照手册而与需求文档字面不同的

| 需求文档 | 手册 | 本设计 |
| -------- | ---- | ------ |
| FR-IN-06："ICMP **本地应答**与 `icmp-forwarding`" | "Surge forwards ICMP packets directly and the VIF returns responses itself, so tools like ping keep working."；`icmp-forwarding` 关掉之后 "ping will no longer work through Surge"，理由是转发会泄漏 IP（`features/enhanced-mode.html`、`profile/general.html`） | **照手册转发**：应用的回显请求由 rurge 经本机的 ICMP 套接字发给真实目的地址，回来的应答再由虚拟网卡交回应用；不在本机伪造应答（发往 rurge 自己的地址的除外）。清单 9.1 节已这样登记。需求文档 FR-IN-06 的这几个字随本文档订正（P3-D9） |
| FR-IN-10："`suspend` 时停止接管并释放系统代理与 TUN，API 保持可用" | 只写了 iOS："suspending does not disconnect the VPN tunnel; Surge simply stops handling traffic"（`features/subnet-settings.html`） | 桌面上**照需求文档的读法**（第 9.7 节）：停止接管——撤掉虚拟网卡与系统代理——，监听器与 API 照常；手册没有写桌面，差异登记在清单（P3-D15） |
| FR-OUT-11："增强模式下对 UDP **443** 生效" | 没有把 QUIC 限在 443 端口（"not all QUIC traffic can be recognized"，`rules/protocol-and-network.html`） | M5 定（第 10 节）：识别的范围、误判的代价与阻断的方式在 M5 的细化设计里定，差异登记在清单 |
| FR-DNS-08："TTL 30 秒" | macOS 30 秒、iOS 5 秒 | 取桌面（macOS）的 30 秒，不改；清单已取 Mac 的值（L544） |
| FR-RULE-01：`PROCESS-NAME` 三种模式 | 文件名与全路径模式支持 `*` 与 `?`、区分大小写、只有 Mac、**不支持 `pre-matching`**（`rules/process.html`） | 阶段 1 的匹配器已经照这些做（通配、Windows 路径归一化）；不支持预匹配在加载期已是 `E0014`。本阶段只补进程的来源 |
| 第 4 节没有 `[IP Rewrite]` | `[IP Rewrite]`（iOS 5.23.0 / Mac 6.10.0 起）在包进入 VIF 时、早于 TCP / UDP 栈与规则按目的地址处理 | 归在 FR-IN-06 之下，M5 实现（第 8.7 节） |
| FR-IN-07 写"macOS 通过 libproc"；第 6 节平台矩阵写 Windows 用 WLAN API 取 Wi-Fi、Linux 用 nl80211、网络变化用 `NotifyAddrChange` / 网络列表管理器、netlink、SystemConfiguration 动态存储 | 手册不涉及实现 | 本设计的做法不同（第 5、7 节）：macOS 的进程识别用 `sysctl` 的 PCB 表（libproc 要自己定义 `socket_fdinfo` 一族结构，更慢）；Windows 的 Wi-Fi 经 `netsh`、Linux 经 `nmcli` / `iw`、macOS 经 `ipconfig getsummary`；网络变化在 Windows 与 Linux 上轮询、macOS 上读路由套接字。需求文档的这几处随本文档订正 |

### 2.3 手册没有写、由本设计定的

报告第 5 节的 28 个疑问，在本设计里的归宿（"M*n*"表示留给那个里程碑的细化设计，在那里定稿并登记清单）：

| 疑问 | 归宿 |
| ---- | ---- |
| Q-01 发往 fake 地址的 ping、ICMP 差错 | 回显请求发往 fake 地址时先反查域名、解析出真实地址再转发，应答的来源写回 fake 地址；ICMP 差错（超时、不可达）本阶段不转发，`traceroute` 经 rurge 看不到中间的跳（第 8.4 节） |
| Q-02 TCP / UDP / ICMP 以外的协议 | 进了虚拟网卡的一律丢弃并计数；只有目的地址在 `tun-excluded-routes` 里的才不进虚拟网卡 |
| Q-03 `tun-included-routes` 与 `tun-excluded-routes` 谁压谁 | 包含路由赢（第 9.2 节：排除路由从基础前缀里求补集之后，包含路由另外装上） |
| Q-04 缺省的接管范围 | 由路由决定（第 9.2 节）：直连子网（LAN）的路由比 `/1` 更具体，缺省不进虚拟网卡；回环从不进；进了虚拟网卡的组播与广播丢弃 |
| Q-05 "answers all DNS queries" 与 `hijack-dns` 的缺省 | 两句同时成立靠系统 DNS 指向应答器（第 9.3 节）；发往别的服务器的查询照手册作普通 UDP 流 |
| Q-06 哪些策略要真实 IP | 不变：引擎今天已按出站决定要不要解析（DIRECT 一类与 `wireguard` 在本机解析，代理把名字交给服务器） |
| Q-07 fake-IP 表的寿命、容量与未知地址 | 第 8.5 节：持久化，跨重启保留；表满时回收最久没用的；连到表里没有的 fake 地址回 RST 并记一行日志 |
| Q-08 预匹配对 UDP | 没有预匹配（清单 L334 已取这一句） |
| Q-09 SVCB（64）与 HTTPS（65） | 两种都按 `allow-dns-svcb` 拒绝（Not Implemented） |
| Q-10 `REJECT-TINYGIF` 在 DNS / SYN 阶段 | 同 `REJECT` |
| Q-11 系统 DNS 怎样指到应答器；简单主机名与 `.local` | 第 9.3 节；应答器对简单主机名与 `.local` 照常给 fake 地址（这样的查询多半不经单播 DNS 来到这里），连接到来时引擎照今天的规则解析（`.local` 交系统解析库） |
| Q-12 rurge 自己的出站怎样不回到 TUN | 第 9.4 节：rurge 建的每个出站套接字都绑在物理网卡上 |
| Q-13 `PROCESS-NAME` 的细节 | 匹配器不变（阶段 1）；外部程序与它的子进程一律 DIRECT 在 M4（第 9.4 节） |
| Q-14 "outgoing interface"、"primary network interface" | 读作默认路由所在的物理网卡（排除 rurge 自己的虚拟网卡）；策略的 `interface` 参数不改变 `SUBNET` 看的网络 |
| Q-15 `TYPE:WIRED` / `TYPE:CELLULAR` 在桌面 | 按操作系统报的网卡类型：无线网卡是 `WIFI`，以太网类（含 USB 共享网络）是 `WIRED`，`CELLULAR` 永不匹配（清单已登记） |
| Q-16 `MCCMNC:` 写在 `[SSID Setting]` 里 | 照常解析，永不匹配 |
| Q-17 `[SSID Setting]` 一行的拆法、几行同时命中 | 第 6.3 节：参数从第一个前面有空白的 `<认识的键>=` 开始，表达式是它之前的部分（所以 SSID 里可以有空格）；参数之间与列表的项之间都是逗号，逗号后面一段写成 `<认识的键>=` 的开始下一个参数，否则是上一个列表的下一项；几行同时命中当前网络时**取第一行**（照 `subnet` 组的 "the first match wins"） |
| Q-18 `suspend` 在桌面上的含义 | 第 9.7 节 |
| Q-19 Mac 与 iOS 的缺省值 | 一律取 Mac 的：应答器 `198.18.0.2`、TTL 30 秒、DNS 缓存 2000 条 |
| Q-20 `skip-proxy` / `exclude-simple-hostnames` 的两种语义 | 桌面取 Mac 的语义（写进系统代理的绕过列表）；增强模式开着时，绕过系统代理的主机的流量落到默认路由、照样进虚拟网卡，两种语义的效果相同 |
| Q-21 `ipv6-vif = auto` 的判据 | 物理网络有全局 IPv6 地址和 IPv6 默认路由；网络变化时重判 |
| Q-22 UDP 的会话寿命、`udp-priority`、QUIC 识别的范围 | 会话寿命沿用引擎的流（60 秒，DNS 10 秒）；`udp-priority` 与 QUIC 在 M5 |
| Q-23 来自虚拟网卡的流量的 `IN-PORT`、`SRC-PORT`、`SRC-IP` | `IN-PORT` 是 0，永不匹配；`SRC-PORT` 是应用的本地端口；`SRC-IP` 是包里的源地址（本机应用共用虚拟网卡的地址） |
| Q-24 `[SSID Setting]` 的 DNS 覆盖管到哪里 | 替换全局的 `dns-server` / `encrypted-dns-server` 用到的每一处（解析器的上游、应答器的转发、加密 DNS 的引导）；`[Host]` 里 `server:` 写明的服务器不受影响 |
| Q-25 启动、退出、崩溃时的清理 | 第 9.5 节 |
| Q-26 `[IP Rewrite]` 与 fake 地址 | 按包里的目的地址判，fake 地址也算（在反查域名之前） |
| Q-27 出站模式为直连 / 全局时 | 应答器照常给 fake 地址，连接到来时反查域名，再直连或交全局策略；这两种模式下 DNS 与 SYN 层都不做预匹配（同今天的规则引擎：直连与全局模式不看预匹配） |
| Q-28 运行中打开 / 关闭增强模式 | 关闭时虚拟网卡上的会话随之结束，系统 DNS 恢复；应用手里缓存的 fake 地址在 TTL（30 秒）之后重新查询（第 9.5 节） |

## 3. 技术选型

### 3.1 TCP 的用户态协议栈（P3-D2）

需求文档 FR-IN-06 写的是"TCP / UDP 通过用户态协议栈进入流水线"，PRD Q7 留给阶段 3 按基准复核。UDP 与 ICMP 不需要协议栈（逐包处理，第 8.3、8.4 节），要选的只是 TCP。调研（报告第 1 节）的候选：

| 方案 | 要点 | 对 rurge |
| ---- | ---- | -------- |
| smoltcp | 已在依赖树里（0.12.0，0BSD），rurge 的 WireGuard 出站在用；**套接字查找是线性扫描**（入包与每次 `poll` 的出口都遍历整个 `SocketSet`），每个套接字的收发缓冲在创建时按固定容量分配；阶段 2 记下的 0.12 缺陷里，没有零窗口探测、一次 `poll` 每个套接字只发一段、Cubic 算错窗口三项在 0.13 / 0.14 修了（那两个版本要 Rust 1.91），回退 N 重传（没有 SACK）0.14 仍在，LAST-ACK 等状态里 ACK 清掉重传定时器、keep-alive 探测发出的字节两项在更新日志里没有明确的对应 | 可用；要分片（多个 `Interface`，每片几百个套接字）并控制每连接的缓冲 |
| 内核 TCP + 地址改写（sing-box 多年的 `system` 栈） | 读到的 TCP 包改写地址后写回虚拟网卡，让本机内核把它当作发给本机监听器的连接；吞吐、内存、正确性都交给内核 | 不是"用户态协议栈"（与 FR-IN-06 的字面冲突）；Windows 要加入站防火墙规则（受管机器上组策略可能不允许本地规则）、macOS 的应用防火墙可能拦未签名的程序；核心部分在开发机上测不了 |
| 自己写 TCP 的被动一端 | 本地链路几乎不丢包、不乱序，对端是操作系统内核；可以做到按四元组 O(1) 查找、缓冲按在途数据增长 | 工作量最大，正确性要自己负责 |
| ipstack / lwIP / gVisor | ipstack 的 TCP 过于简化（没有 RTT 估计、不检查 SYN、通道无界）；lwIP 绑定要 libclang、有全局锁；gVisor 只有 Go | 不考虑 |

**决定**：TCP 用 **smoltcp 0.12，按四元组分片**（若干个 `Interface` + `SocketSet`，每片各在自己的任务里推进），每连接的收发缓冲按一个总预算在创建时分配（连接少时大、连接多时小，上下限在 M3 定），拥塞控制用 Reno（0.12 的 Cubic 有缺陷，同 WireGuard），关 Nagle，反复 `poll` 到没有出口（同 WireGuard 的做法）；TCP 套接字的总数与半开的（收到 SYN、还没建成的）数各有硬上限，超出的 SYN 回 RST；每一片的 `Interface` 自带重组缓冲（工作区为 WireGuard 开的 `reassembly-buffer-size-16384` × `count-4`，约 64 KiB，特性在工作区里合并、裁不掉），片数计入内存预算。接口收窄成 `rurge-tun` 内部的一个边界（"包进、包出、连接以 `BoxedStream` 交出"），以便替换。

**基准门槛**：M3 的第一个任务先把 TCP 这一层与一个内存里的基准做出来，接上转发循环（每条会话两块 8 KiB 的缓冲，1 万条就约 156 MB）与回环上的上游，量进程的常驻内存，不只量协议栈：①单流吞吐（单核、当前线程运行时）；②1 万条空闲连接加 100 条活跃连接时的常驻内存与每包 CPU。判据取阶段 3 验收的"NFR-01 目标的 70%"：单流不低于 0.7 Gbit/s，1 万条连接的常驻内存不超过 300 MB ÷ 0.7 ≈ 429 MB（"70%"用在上限型的指标上读成除以 0.7，这是一种读法，第 16 节 Q7）。**达不到且调参之后仍达不到时，换成自己写的 TCP 被动一端**（上表第三行），这个转向不必再回来定；内核 TCP + 地址改写不做（理由见上表）。内存里的基准不含真实设备的系统调用，真实设备上的数字在手工验收里量。

**不升 smoltcp**：0.13 / 0.14 要把工作区的 MSRV 从 1.89 升到 1.91，这是项目所有者的决定（第 16 节 Q1）；本阶段按 0.12 设计，0.12 的缺陷在本地链路上触发得少（丢包只在虚拟网卡的队列溢出时发生），照阶段 2 的做法登记、用用例钉住能钉住的部分。

### 3.2 依赖（P3-D3）

**本阶段不引入新的包**（`Cargo.lock` 的包数不变）：锁文件里已有的足够——`smoltcp` 0.12（协议栈与包的解析、构造、校验和）、`socket2`（任意地址族的套接字都能安全地建与收发：ICMP、macOS 的路由套接字与 utun 的控制套接字；`bind_device` / `bind_device_by_index_v4/v6`）、`libc`（Linux 的 TUN 常数、macOS 的 `ctl_info` / `sockaddr_ctl` 与 `sysctlbyname`、`proc_pidpath`）、`windows-sys` 0.61（加 IP Helper、WinSock、库加载、WinTrust 等特性）、`if-addrs`、`ipconfig`、`hickory-proto`（DNS 报文）、`resolv-conf`。给已有的包开新特性不增加包——**但 `nix` 的 `socket` / `net` 特性要 `memoffset`，锁文件里没有，所以不开**：Linux 不用 netlink（网络变化轮询、进程识别读 `/proc/net`），macOS 的 utun 与路由套接字经 `socket2` 与 `libc`。

调研推荐的几个包（`tun-rs`、`rtnetlink`、`wintun-bindings`、`system-configuration`、`zbus`）各有好处，但新包要经项目所有者审过许可与供应链；设备、路由与网络环境都藏在 trait 后面，以后换成这些包只动 `rurge-platform`（第 16 节 Q2）。

### 3.3 设备层（P3-D5）

自己写一层薄的，放在 `rurge-platform::tun`（第 4 节）：

- **Linux**：打开 `/dev/net/tun`，一次 `ioctl(TUNSETIFF)`（`IFF_TUN | IFF_NO_PI`，`libc` 已有常数）——这是 Linux 一侧唯一的 unsafe；描述符非阻塞，经 tokio 的 `AsyncFd` 读写。不用多队列与卸载（GSO / GRO），留作以后的性能手段。
- **macOS**：utun 的控制套接字经 `socket2` 建（`AF_SYSTEM`、`SOCK_DGRAM`、`SYSPROTO_CONTROL`），按名字查控制号（`CTLIOCGINFO` 的 `ioctl`）与 `connect` 一个 `sockaddr_ctl` 放在一个 unsafe 函数里；单元号由 rurge 从小到大试，连上的那个就是 `utunN`，不必再读接口名；每个包前面的 4 字节地址族头在读写时加减。（`nix` 有现成的安全封装，但要开 `socket` 特性，见第 3.2 节。）
- **Windows**：Wintun。rurge **不带、也不下载** `wintun.dll`：从可执行文件所在目录以绝对路径加载（`LoadLibraryExW`，只搜应用目录与 System32），或由 `--wintun-dll <路径>`（环境变量 `RURGE_WINTUN_DLL`）指定；**加载之前校验它的 Authenticode 签名（`WinVerifyTrust`）且签名者是 WireGuard LLC**，两种来源都校验，不过就不加载（增强模式以管理员运行，可执行文件所在的目录可能是使用者可写的，不校验就等于让任何能放文件的人以管理员身份运行代码）；找不到或校验不过时以一句写明去哪里取（wintun.net）、放在哪里的错误结束。适配器用固定名字与由名字派生的固定 GUID（避免每次启动多一个"未识别的网络"），遇到已存在的就打开复用；会话环 8 MiB；读在一个专用线程上（`WintunReceivePacket`，没有包时等读事件），写满时丢包。Wintun 的再分发条款（调研报告第 7 节）：官方签名的 DLL 原样与使用它的软件一起分发是许可允许的，能否嵌进可执行文件没有明文；rurge 的发行包要不要附带 DLL 由项目所有者定（第 16 节 Q3，PRD 风险 R2）。

设备在 `rurge-tun` 里只是一个 trait（批量收包、发包、MTU、名字），测试用内存设备（第 13 节）。

### 3.4 地址、路由与系统 DNS（P3-D6）

照系统代理与服务安装的先例：**经系统自带的命令行工具做，先生成"计划"（一串带撤销步骤的命令），再由执行器逐条执行**；计划是纯数据，三个平台的计划都在任何平台上用数据用例测，测试从不执行它们。Linux 用 `ip` 与 `resolvectl`，macOS 用 `ifconfig`、`route`、`networksetup`，Windows 用 `netsh`（`store=active` 的地址与路由在重启后自动消失）。逐条的命令在 M4 的细化设计里定。

所有会在崩溃之后留下来的改动，先把撤销它的步骤写进 `state.json`（同 `system_proxy_backup`），做完再记下一条；下一次启动拿到实例锁之后、做任何新改动之前先撤销上一次留下的（第 9.5 节）。

### 3.5 进程识别与网络环境

见第 5、7 节。两者的平台差异最大、开发机上能测的最少：Windows 的实现在开发机上能跑（只读系统的表），Linux 的解析能用文本夹具测，macOS 的只能在 CI 与手工验收里核对。

### 3.6 unsafe（P3-D4）

工作区仍是 `forbid`；**新增的 unsafe 全部在 `rurge-platform`**，`rurge-tun` 与其余 crate 保持 `forbid`。阶段 3 要的 FFI（调研报告 2.3 与 9.1 节的估计，数字在各里程碑定稿）：

| 平台 | 函数 | 里程碑 |
| ---- | ---- | ------ |
| Linux | `TUNSETIFF` 的 `ioctl` | M4 |
| macOS | `sysctlbyname`（读 `net.inet.tcp.pcblist_n` / `udp.pcblist_n` 与 `kern.osrelease`）、`proc_pidpath` | M2 |
| macOS | utun：`CTLIOCGINFO` 的 `ioctl` 与 `connect` 一个 `sockaddr_ctl` | M4 |
| Windows | `GetExtendedTcpTable`、`GetExtendedUdpTable`、按 PID 取映像路径（`OpenProcess` + `QueryFullProcessImageNameW`） | M2 |
| Windows | Wintun 的加载与调用（签名校验 `WinVerifyTrust` 与签名者；加载库与取符号；建 / 开 / 关适配器、取 LUID；起 / 停会话、读事件；收 / 释放、分配 / 发送） | M4 |
| 三个平台 | 外部程序的进程树判断（Windows 上进程是否在外部程序的 Job Object 里，Unix 上取父进程或进程组；具体的调用在 M4 定） | M4 |
| Windows | `IP_UNICAST_IF` / `IPV6_UNICAST_IF` 的 `setsockopt`（防环，第 9.4 节） | M4 |

每个标 `#[allow(unsafe_code)]` 的函数只包一次 FFI 调用或一组紧密相关的调用，文档注释写明安全前提；`rurge-platform` 加一条用例，读自己的源码，把标了 `#[allow(unsafe_code)]` 的函数与一张固定的名单比对——新加一个必须同时改名单，名单的改动在评审里看得见。CLAUDE.md 的 `unsafe_code` 一段随各里程碑更新。

能不用 unsafe 的地方不用：macOS 的路由套接字与 ICMP 套接字经 `socket2`；Linux 的网络变化与进程识别读 `/proc`；地址、路由、DNS 经命令；Windows 的 Wi-Fi 信息经 `netsh wlan show interfaces`（不链接 `wlanapi.dll`——Windows Server 上没有装无线服务时它不存在，链接它会让 `rurge.exe` 起不来）。

## 4. Workspace 与 crate 边界

| crate | 本阶段的变化 |
| ----- | ------------ |
| `rurge-tun`（新） | 平台无关，`forbid(unsafe_code)`：设备 trait 与内存设备、包的去向、TCP（smoltcp 分片）、UDP 会话（实现 `rurge_inbound::UdpClient`）、ICMP 转发的逻辑、DNS 应答器（fake 应答、转发、预匹配）、`[IP Rewrite]`、测试用的"应用端"夹具。依赖 `rurge-inbound`（`Dialer` 等）、`rurge-dns`、`rurge-config`、`rurge-net`；**不依赖** `rurge-platform` 与 `rurge-engine`——设备、ICMP 发送、进程查找、预匹配都经 trait 由可执行文件接上（同 `SocketHook`、`ProcessHook` 的先例） |
| `rurge-platform` | 新模块 `netenv`（网络快照与事件，M1）、`procinfo`（M2）、`tun`（设备，M4）、`route`（地址、路由、系统 DNS 的计划与执行，M4）、`icmp`（ICMP 套接字，M3）；`socket` 加 Windows 的 `IP_UNICAST_IF`（M4）；`dns::servers()` 在 Windows 上遍历所有开着的网卡，接管之后要排除虚拟网卡与应答器的地址，否则 `on_network_change` 一调就把应答器当成上游（M4） |
| `rurge-dns` | `fakeip`（池、双向映射、持久化，M3）；`Resolver::forward`（原样转发一个查询报文、遵守 `[Host]` 的 `server:`，M3）；纯 UDP / TCP 上游今天自己建套接字（`upstream/udp.rs` 的 `UdpSocket::bind`、截断之后的 `TcpStream::connect`），不经连接器也不经钩子，M4 把它们收到统一的建法上（第 9.4 节） |
| `rurge-config` | `[SSID Setting]` 类型化（M1）；`subnet` 组的条件进 `GroupSpec`（M1）；`[IP Rewrite]` 类型化（M5）；增强模式用到的 `[General]` 键大都已经解析，补上未类型化的 |
| `rurge-rules` | `SUBNET` 从"永不匹配"变成按当前网络匹配（M1）；扩展现有的预匹配入口 `pre_match_domain` / `pre_match_ip`（今天按 `SessionInfo::tcp(..)` 求值、必须带端口）：端口可以没有、带上会话真实的来源与网络（M3） |
| `rurge-policy` | `subnet` 组按当前网络选成员与它的临时覆盖（M1）；网络变化时测速结果失效（M1） |
| `rurge-engine` | 网络变化的下游（M1）、`[SSID Setting]` 的应用（M1）、进程查找的钩子（M2）、TUN 会话接入与预匹配钩子（M3）、接管暂停（M1 / M4；与今天托管配置 `strict` 的 `Engine::suspend`——拒绝新会话——是两回事，名字上分开） |
| `rurge-api` | `GET` / `POST /v1/features/enhanced_mode` 生效（M4） |
| `rurge`（bin） | `--enhanced-mode`、`--wintun-dll`；增强模式的管理器（设备、计划、`state.json` 里的备份、恢复，照 `SystemProxyManager`）；三个平台适配器（网络环境、进程查找、设备） |

需求文档 3.2 节把设备驱动与路由管理写在 `rurge-tun` 里；本设计把它们放进 `rurge-platform`，让 unsafe 只在一个 crate 里出现、`rurge-tun` 在三个平台上都是同一份可测的代码。AR-02（平台代码只在 `rurge-platform` 与 `rurge-tun`）不变。需求文档 3.2 节的表随本文档订正。

依赖方向：`rurge (bin) → { rurge-tun → { rurge-inbound, rurge-dns, rurge-config, rurge-net }, rurge-engine, rurge-platform }`；`rurge-tun` 与 `rurge-engine` 互不依赖，引擎经 `Dialer` 接上 TUN，同今天的 HTTP / SOCKS5 监听器。

## 5. 网络环境（FR-IN-09，M1）

**快照**（`rurge_platform::netenv::Snapshot`）：网卡（名字、序号、类型——无线 / 以太网 / 其它、地址、是否开着）；主网卡（IPv4 与 IPv6 各一：默认路由所在的物理网卡、它的地址、默认网关）；系统 DNS 服务器（不含 rurge 自己的应答器地址）；Wi-Fi 状态——已连接（SSID、BSSID，各自可能是"拿不到"）、没有连接、平台给不出（带原因）。**排除 rurge 自己的虚拟网卡与它的地址、路由**（M4 之后虚拟网卡成了默认路由，主网卡仍是物理的那一块）。

**来源**（调研报告第 6 节；逐条在 M1 定稿）：

| 平台 | 网卡、地址、网关、DNS | Wi-Fi | 变化事件 |
| ---- | --------------------- | ----- | -------- |
| Windows | `ipconfig::get_adapters()`（已有依赖：地址、网关、DNS、网卡类型、状态、度量） | `netsh wlan show interfaces`：只在 `ipconfig` 的快照变了、且有无线网卡连着时读，另外无线网卡连着时每 30 秒读一次（同一个 SSID 下换 AP 不改地址）；从不每 2 秒起一个进程（NFR-02：空闲时 CPU 接近 0）。`subnet-exp-wifi-always-match` 要无线网卡不是主网卡时的 SSID，所以不只在主网卡是无线时读 | 轮询（每 2 秒比一次快照，`ipconfig` 的调用是毫秒级） |
| Linux | `if-addrs`；`/proc/net/route` 与 `/proc/net/ipv6_route` 取默认路由；`/sys/class/net/<网卡>/wireless` 判无线；DNS 取 `/etc/resolv.conf`，它指向 systemd-resolved 的本机桩时改取 `resolvectl` 给物理网卡的服务器 | `nmcli`，没有时 `iw` | 轮询（每 2 秒比一次快照，读的都是 `/proc` 与 `/sys` 下的文件；netlink 的多播要 `nix` 的 `socket` 特性或一处 unsafe 的 `bind`，不做） |
| macOS | `if-addrs`；`route -n get default`（IPv6 同样）；`networksetup -listallhardwareports` 判无线；DNS 取 `scutil --dns` | `ipconfig getsummary <网卡>`（拿不到或被隐去时是"拿不到"） | `PF_ROUTE` 套接字（经 `socket2` 建与收，零 unsafe；收到任何路由消息就重读快照），另加 30 秒一次的兜底轮询 |

**SSID 拿不到是正常状态**：Windows 在没有授予精确位置权限时不给 SSID / BSSID，以 SYSTEM 身份运行的进程连提示都没有；macOS 14 起读 SSID 要位置授权，命令行守护进程多半拿不到（调研报告 6.3 节）。拿不到时 `SSID:` / `BSSID:` 表达式不匹配，日志里说一次原因；`ROUTER:`、`TYPE:` 照常。macOS 上要拿到 SSID 只能另做一个带位置权限的 App 包，留给项目所有者（第 16 节 Q6）。

**变化**：快照里主网卡、它的地址、网关、DNS 服务器、Wi-Fi 的 SSID / BSSID、网卡类型任何一项不同即是一次变化；1 秒的"首发 + 尾发"去抖（第一个变化立刻发出，1 秒内再来的合并成 1 秒后的一次）。

**下游**（按这个顺序；手册写了的八处见调研报告 4.12 节）：

1. 重新选物理网卡给防环用（M4）；
2. 重判 `[SSID Setting]`：进出带 `suspend` 的网络暂停 / 恢复接管，DNS 覆盖变了就按新的覆盖重建解析器；
3. DNS：`Resolver::on_network_change()`——清缓存、AAAA 抑制复位、`system` 重新展开（阶段 1 已有）；
4. 策略组：`url-test`、`fallback`、`load-balance` 的测试结果作废，下一次使用时重测（手册："A network change also invalidates the result."）；`smart` 不动（手册没有写）；
5. `subnet` 组：没有要存的东西，下一次使用时按新快照选；
6. WireGuard：每个 `wireguard` 出站 `network_changed()`（阶段 2 已有入口）；
7. 虚拟网卡：`ipv6-vif = auto` 重判，路由按需重装（M4）；
8. 一行 INFO 日志（主网卡与类型；SSID 与 BSSID 只在 DEBUG 里——它们说明使用者在哪里）。

`network-changed` 事件脚本（阶段 5）与 Logbook（阶段 6）在这里接上，本阶段只留入口。

验收"网络切换后 30 秒内恢复"：事件路径最坏是轮询的 2 秒（Windows、Linux）或兜底的 30 秒（macOS 的路由套接字失效时），加 1 秒去抖；能在测试里量的部分用假探测器与暂停的时钟钉住，真实的切换在手工验收里量。

## 6. `subnet` 组、`SUBNET` 规则与 `[SSID Setting]`（M1）

### 6.1 子网表达式的求值

照清单 3.4 节：`SSID:`（通配、区分大小写）、`BSSID:`（通配、不区分大小写）、`ROUTER:`（主网卡默认网关的精确匹配，IPv4 或 IPv6）、`TYPE:WIFI` / `TYPE:WIRED` / `TYPE:CELLULAR`（后者永不匹配）、`MCCMNC:`（永不匹配）、裸值（含通配符按 SSID 通配，否则依次精确匹配 SSID、BSSID、网关）。"当前网络"是主网卡的网络；`subnet-exp-wifi-always-match`（缺省 `true`）时，`SSID:` / `BSSID:` 在无线网卡不是主网卡时也看它连着的那个网络，`false` 时只在无线网卡是主网卡时看。

### 6.2 `subnet` 组与 `SUBNET` 规则

`subnet` 组按声明顺序取第一个命中的条件，没有命中用 `default`（`cellular` 参数在桌面上从不生效）；每次使用时按当前快照选，不存状态。`GET /v1/policy_groups` 给出当前的选择。**临时覆盖**：手册说自动类型的组——`url-test`、`fallback`、`load-balance`、`subnet`、`smart`——都能临时覆盖（`policy-groups/parameters.html`），清单 L455、L870 也写着随 `subnet` 组在阶段 3 做：`POST /v1/policy_groups/select` 对 `subnet` 组设临时覆盖，同自动组（M3b）；网络变化时清除（`subnet` 组按网络选，覆盖说的是"在这个网络上先用它"——rurge 的读法，手册没有写）。**没写 `default`**（清单 L453 留给阶段 3 定）：加载错误（同手册：`default` 必填），不再按空组兜底。`SUBNET` 规则按当前网络匹配，可以带 `pre-matching`（手册的名单里有它）。阶段 2 起登记在清单里的两条差异（"整组代表它的 `default`"与"`SUBNET` 永不匹配"）随 M1 撤下。

规则与组读网络快照的方式（会话信息带上，还是求值时另给一个视图）在 M1 定。

### 6.3 `[SSID Setting]`

一行是"子网表达式 + 空白 + 参数"：参数从第一个前面有空白的 `<认识的键>=` 开始，表达式是它之前的部分（SSID 里可以有空格：`SSID:My Home suspend=true`）；参数以逗号分开，`dns-server` 与 `encrypted-dns-server` 的值本身是逗号分开的列表——逗号后面一段写成 `<认识的键>=…` 的开始下一个参数，否则是上一个列表的下一项（清单 9.4 节记下的读法）。用阶段 2 末"一行怎样拆成各项"的拆分（引号只在一个值的开头开启引用）。认识的键：`suspend`、`dns-server`、`encrypted-dns-server`、`tfo-behaviour`、`cellular-fallback`（iOS 专属，忽略）、`cellular-mode`（远期，告警并忽略）。几行同时命中取第一行。

- **DNS 覆盖**：在命中的网络上替换全局的 `dns-server` / `encrypted-dns-server`（替换而不是合并；全局配了加密 DNS、而这个网络要用传统 DNS 时必须写 `encrypted-dns-server=off`，同手册），按新的配置重建解析器；离开时恢复。
- **`suspend`**：接管暂停，第 9.7 节。M1 先做系统代理的一半。
- **`tfo-behaviour`**：rurge 没有实现 TCP Fast Open（阶段 2：`tfo` 是没有效果的参数），这个键照收、没有效果，诊断同 `tfo`。

## 7. 进程识别（FR-IN-07，M2）

`rurge_platform::procinfo`：给出（传输层，本地端点，可选的远端端点）→ 进程（PID、路径、文件名）。同步实现，由调用方放进 `spawn_blocking`。`SessionInfo.process` 今天只有文件名与路径（`rurge-config/src/session.rs`），M2 加上 PID（外部程序的进程树判断要它，第 9.4 节）。

| 平台 | 做法（调研报告第 5 节） |
| ---- | ----------------------- |
| Windows | `GetExtendedTcpTable`（带 PID 的连接表）/ `GetExtendedUdpTable`，IPv4 与 IPv6 各一张；按本地端点找行（UDP 的本地地址可以是未指定地址）；PID → `QueryFullProcessImageNameW`；PID 0 与 4 是系统 |
| Linux | 读 `/proc/net/tcp`、`tcp6`、`udp`、`udp6` 按本地端点找到 inode 与 uid（需求文档的写法；`sock_diag` 的精确查询更快，但要 netlink，见第 3.2 节）；再扫 `/proc/<pid>/fd` 找持有这个 inode 的进程（先扫同一 uid 的），读 `/proc/<pid>/exe` |
| macOS | `sysctlbyname` 读 `net.inet.tcp.pcblist_n` / `udp.pcblist_n`，按 `kern.osrelease` 选记录大小、解析之前校验长度（记录布局随 Darwin 版本变）；PID → `proc_pidpath`；布局对不上时当作查不到并说一次 |

**什么时候查**：来源是本机的会话——经监听器从回环或本机地址来的、经虚拟网卡来的——在规则评估之前查一次；表按快照缓存并限频（没找到时重取一次，同一种表 100 毫秒内至多重取一次），一次查找超过 200 毫秒当作不知道（DEBUG 里说一次）；查到的进程进 `SessionInfo.process`，`PROCESS-NAME` 用它，请求记录也带上。来自别的设备的会话从不查。每次都查还是只在规则用得到 `PROCESS-NAME` 时查，在 M2 按基准定（Windows 上取整张表的代价随连接数增长）。

**看不见的来源**：虚拟机、WSL2、容器与经过转发的流量在本机没有对应的套接字，进程是"不知道"；来源是 rurge 自己的连接不查。

## 8. TUN 内核（`rurge-tun`，M3 / M5）

### 8.1 包的去向

```
设备批量收包
  → 解析 IPv4 / IPv6（smoltcp::wire）；坏包、组播、广播、分片、TCP / UDP / ICMP 以外的协议：丢弃并计数
  → [IP Rewrite]（M5）：reflect / reject / drop
  → TCP：按四元组分到一片 → SYN 先过预匹配 → 协议栈 → 发往应答器 53 端口的连接在本地当 DNS over TCP 应答，其余交给引擎
  → UDP：发往应答器或命中 hijack-dns 的合法 DNS 查询 → 应答器
         其余 → 按应用的源端点归入 UDP 会话 → 引擎
  → ICMP / ICMPv6 回显请求 → ICMP 转发（icmp-forwarding 关掉时丢弃）
所有出去的包经一个写任务成批写回设备
```

IPv4 / IPv6 分片本阶段不重组、丢弃并计数（虚拟网卡的 MTU 大于应用实际发的包时分片很少出现；sing-tun 的系统栈同样丢弃 IPv4 分片）。MTU（候选 1500 与 9000）随 M3 的基准定：越大协议栈处理的段越少，但应用的 QUIC 会按它探测路径 MTU，发出的大包到物理网卡上要被分片。

### 8.2 TCP

第 3.1 节的协议栈。新的 SYN 先按四元组决定落在哪一片，先过预匹配（第 8.6 节）：拒绝的回 RST 或丢弃；放行的给它建一个只接这个四元组的套接字再把 SYN 交给协议栈。握手照常完成之后建会话：

- `SessionInfo`：`src` 是应用的地址与端口，`in_port` 0，`listener` `Tun`，`dst_host` 是 fake 地址反查出的域名或目的 IP，`dst_port`，`transport` TCP，`process`（第 7 节）；
- 交给引擎 `dial`，成功则 `relay`（虚拟网卡一侧的连接以 `BoxedStream` 交出），失败（拒绝或拨不通）回 RST；
- 阶段 3 不等应用的第一段数据再拨号（同今天的监听器：SNI 只在转发时记录）；阶段 4 的拨号前嗅探在这里插入。

连到 fake 地址而表里没有这个地址（重启前的地址已被回收、或从来不是 rurge 给的）：回 RST，记一行日志。

### 8.3 UDP 会话表（FR-IN-08）

应用的每个源端点（地址 + 端口）一个 UDP 会话，实现 `rurge_inbound::UdpClient`，交给引擎现有的 UDP 流水线（`Dialer::associate`）：一个会话里每个目的地是一条流（引擎已有：全锥、同一个出站共用载体、流空闲 60 秒、DNS 流应答后 10 秒回收、流量统计进请求记录）。目的是 fake 地址的流以域名为目标，回包的来源写回应用当初发往的那个 fake 地址。会话里没有流且 60 秒没有包时结束。会话数的上限与引擎现有的 4096 个关联（与 SOCKS5 共用）的关系在 M3 定。`udp-priority` 在 M5（第 10 节）。

### 8.4 ICMP

回显请求（ICMPv4 type 8、ICMPv6 type 128）：

- `icmp-forwarding = false`：丢弃（ping 经 rurge 不通，同手册）；
- 发往 rurge 自己的地址（虚拟网卡的地址、应答器）：本机应答；
- 发往 fake 地址：反查域名、经解析器得到真实地址；
- 其余按目的地址经 ICMP 套接字发出（`rurge_platform::icmp`：Linux 与 macOS 优先无特权的数据报型 ICMP 套接字、不行再用原始套接字，Windows 用原始套接字——增强模式本来就要管理员），套接字同样绑在物理网卡上（第 9.4 节）；按（标识，序号）记下应用与原目的地，应答回来时写一个来源是原目的地（fake 地址也一样）的回显应答交回应用；5 秒没有应答的记录丢掉。

ICMP 不经规则与策略（"ICMP cannot be proxied"）。rurge 自己要合成的 ICMP：UDP 的 `REJECT` 与 `[IP Rewrite]` 的 `reject` 回"管理上禁止"（ICMPv4 type 3 code 13 / ICMPv6 type 1 code 1），限频照清单 4.1 节。ICMP 差错不转发（第 2.3 节 Q-01）。

### 8.5 DNS 应答器与 fake-IP（FR-DNS-08）

**地址**：`198.18.0.2` ～ `198.18.0.9` 与 `fd00:6152::2` 的 53 端口（UDP 与 TCP）；`hijack-dns` 的每项（IPv4 地址或 `*`，可带端口，缺省 53）也交给应答器，只对 UDP，且只有解析得出合法 DNS 查询的包才算，别的照常作 UDP 流（同手册）；发往别的服务器 53 端口的 TCP 照常作 TCP 连接。虚拟网卡自己的地址是 `198.18.0.1`，`198.18.0.0/15` 整段经虚拟网卡（一条显式的路由，第 9.2 节，M4）。

**应答**：

| 查询 | 应答 |
| ---- | ---- |
| A，经 IPv4 来 | `198.18.1.1` ～ `198.19.255.254` 里的 fake 地址（共 130,814 个），TTL 30 秒 |
| AAAA，经 IPv6 来（`ipv6-vif` 开着） | `fd00:6152::/64` 里的 fake 地址（具体的一段在 M3 定，不与虚拟网卡与应答器自己的地址重叠） |
| 与查询所经的地址族不一致的 A / AAAA | 空应答（NOERROR、没有记录） |
| `always-real-ip` 命中的名字 | 经解析器查真实地址；名字在 `[Host]` 里有地址映射的由应答器直接权威应答 |
| SVCB（64）与 HTTPS（65） | `allow-dns-svcb` 关着时 Not Implemented |
| `use-application-dns.net` | NXDOMAIN |
| 其余类型 | 经 `Resolver::forward` 原样转发上游（遵守 `[Host]` 的 `server:`），应答原样交回 |
| 预匹配拒绝的名字 | 第 8.6 节 |

`[Host]` 的地址映射对没有被 `always-real-ip` 豁免的名字不在应答器上生效，在连接到来、引擎解析时生效（同手册）。

**池与映射**（`rurge-dns::fakeip`）：域名（小写、去掉末尾的点）↔ 地址的双向表；按顺序分配，回绕之后回收最久没被用到的地址；每次反查（TCP 连接到来、UDP 包到来）刷新"最近用到"，**正在用着这个地址的会话（TCP 连接、UDP 会话）另记一个租约，有租约的地址从不回收**——TCP 只在连接到来时反查一次，单靠"最近用到"挡不住长连接的地址被回收给别的名字（调研报告 4.3 节末尾指出的风险）。表持久化到数据目录里的一个文件（有改动时定期写、正常退出时写；读不了就从空表开始），跨重启保留（手册："Surge remembers the fake IP ⇄ domain mapping persistently"）。文件的格式、写的时机与大小上限在 M3 定。

### 8.6 预匹配在 DNS 与 SYN 层

规则引擎今天已有"先把带 `pre-matching` 的规则不解析地评估一遍"（阶段 1 补遗（M5a）），也已有只做这一步的入口 `pre_match_domain` / `pre_match_ip`（`rurge-rules/src/engine.rs`），但它们按 `SessionInfo::tcp(..)` 求值（来源是 `127.0.0.1:0`、监听器是 HTTP）且必须带端口，`SRC-IP`、`SRC-PORT`、`SUBNET` 与"DNS 层没有端口"都评不对；本阶段扩展它们（端口可以没有、带上会话真实的来源与当前网络），经引擎交给 `rurge-tun`（这一代的规则，出站模式为规则模式时才看）。

- **DNS 层**（A / AAAA 查询）：会话是被查的名字（没有端口：`DEST-PORT` 一类的条件不命中）。`REJECT` 回没有记录的应答（触发限频之后不应答）、`REJECT-DROP` 不应答、`REJECT-NO-DROP` 回 `198.18.0.244` 并对连往它的所有 TCP 回 RST（清单 4.1 节）；`REJECT-TINYGIF` 同 `REJECT`。
- **SYN 层**：会话是这条连接（域名经 fake 地址反查，或目的 IP）。`REJECT` 与 `REJECT-NO-DROP` 回 RST、`REJECT-DROP` 丢 SYN；3 秒内回了 100 个 RST 之后暂停回 RST、改为丢弃（清单 4.1 节）。
- UDP 没有预匹配。被预匹配拒绝的连接在请求记录里同一条规则 5 分钟内只出现一次（清单 L335 把它标在阶段 4，本阶段一起做，并订正那一行的阶段）。

### 8.7 `[IP Rewrite]`（M5）

照清单 9.10 节：`<IP 地址或 CIDR> = reflect | reject | drop`，自上而下第一条命中；包一进虚拟网卡就判，早于协议栈、应答器与规则，命中的包不成为请求；`reject` 对 TCP 回 RST、对其余回"管理上禁止"；`reflect` 对本机应用的含义在 M5 对着手册定。只管进了虚拟网卡的包，经代理端口来的请求不受影响。

## 9. 平台接入（FR-IN-06，M4）

### 9.1 设备

第 3.3 节。设备的创建与权限检查在 `rurge-platform::tun`，`rurge-tun` 只看到 trait。

### 9.2 地址与路由

- **地址**：虚拟网卡 `198.18.0.1`、前缀 `/15`（fake 地址与应答器都在这一段）；`ipv6-vif` 开着时另有 `fd00:6152::1`。
- **基础前缀**：`0.0.0.0/1` 与 `128.0.0.0/1`（`ipv6-vif` 开着时 `::/1` 与 `8000::/1`），比物理网卡的默认路由更具体，所以不必改动它；三个平台同一种形式。
- **排除**：从基础前缀里减去 `tun-excluded-routes` 与 `external` 策略的 `addresses`（作 `/32` / `/128`），求出最小的前缀集合装到虚拟网卡上（CIDR 求补集，sing-tun 的做法），被排除的地址没有虚拟网卡的路由、自然走原来的默认路由，网络切换时不用重装。
- **fake 段与应答器**：`198.18.0.0/15`（`ipv6-vif` 开着时还有 fake 的 IPv6 段）单独装一条显式的路由，不参与求补集——macOS 的 utun 是点对点接口，不能指望地址带出一条连接路由；`tun-excluded-routes` 与它重叠时 `rurge check` 告警，排除项不会把它剥掉（剥掉的话应答器与 fake 地址的连接都走物理网卡，增强模式整个失效）。
- **包含**：`tun-included-routes` 原样另装到虚拟网卡上——比直连子网与被排除的地址更具体时它们赢（第 2.3 节 Q-03；手册的例子）。整段私网（`10.0.0.0/8`、`172.16.0.0/12`、`192.168.0.0/16`）写进 `tun-included-routes` 时 `rurge check` 给一条告警（手册："it is normally unnecessary and can cause system issues"）。
- **其它**：Linux 把虚拟网卡的 `rp_filter` 放宽到 2；Windows 把虚拟网卡的接口度量设为 0、关自动度量；IPv6 路由只在 `ipv6-vif` 开着时装。逐条的命令在 M4 定。

### 9.3 系统 DNS

| 平台 | 做法 | 崩溃之后 |
| ---- | ---- | -------- |
| Windows | 在虚拟网卡上设 DNS 为 `198.18.0.2`（接口度量 0，系统优先用它） | 随适配器消失；适配器残留时下一次启动复用 / 关闭 |
| Linux | systemd-resolved：`resolvectl dns <虚拟网卡> 198.18.0.2`、`domain <虚拟网卡> '~.'`、`default-route <虚拟网卡> yes` | 随虚拟网卡消失（`resolvectl` 的逐网卡设置不持久） |
| Linux（没有 systemd-resolved） | 不改 `/etc/resolv.conf`；启动时告警：系统 DNS 没有接管，写 `hijack-dns = *:53` 或自己把系统 DNS 指向 `198.18.0.2` | — |
| macOS | 对每个启用的网络服务 `networksetup -setdnsservers <服务> 198.18.0.2`，原值先写进 `state.json` | 下一次启动恢复（第 9.5 节） |

macOS 不用调研推荐的 SystemConfiguration 动态存储临时键（崩溃之后自动失效）：没有现成的包，要自己声明 SystemConfiguration 的 C 接口，又是一批只能在 macOS 上测的 unsafe；代价是 rurge 被强行结束之后，系统 DNS 一直指向没人应答的 `198.18.0.2`，直到 rurge 再次启动并恢复——手工恢复的办法（`networksetup -setdnsservers <服务> Empty`）写进 API 参考与手工清单。设完之后清系统的 DNS 缓存。**已知的限制**：发往直连子网（LAN）里的 DNS 服务器的查询走直连子网的路由、不进虚拟网卡，`hijack-dns = *:53` 也劫不到（同调研报告 3.2 节记下的 mihomo 的限制），所以系统 DNS 必须指向应答器。**已知的泄漏**：Windows 的"智能多宿主名字解析"可能同时从物理网卡发查询（调研报告 4.2 节），浏览器自己的 DoH 也绕过应答器；本阶段只做 canary 域、拒绝 type 65，在文档里说明，Windows 的严格模式是开放问题（第 16 节 Q5）。

**接管期间的 `system`**：`dns-server = system` 与 `[Host]` 的 `server:system` / `server:syslib` 指"操作系统当前配置的服务器"；系统 DNS 被改成应答器之后，取的是物理网卡自己的服务器（Windows 读物理网卡的设置；Linux 读物理网卡在 resolved 里的设置；macOS 用接管之前记下的原值，网络变化时读 DHCP 给的服务器），从不取应答器，免得回环。`server:force-syslib` 照旧交系统解析库——系统 DNS 已指向应答器时它会拿到 fake 地址，手册对此有警告，rurge 在加载时对它给同样的告警（FR-DNS-05）。

### 9.4 防环

rurge 自己建的每一个出站套接字——DIRECT 与直连类策略、DNS 上游（UDP、TCP、DoT、DoH、DoQ、DoH3）、QUIC 的端点、WireGuard 的载体、外部资源的下载、ICMP 转发——在增强模式开着时绑到物理主网卡（网络环境给出，排除虚拟网卡）：Linux `SO_BINDTODEVICE`（`socket2::bind_device`，已有）、macOS `IP_BOUND_IF` / `IPV6_BOUND_IF`（已有）、Windows `IP_UNICAST_IF` / `IPV6_UNICAST_IF`（新的一处 unsafe；阶段 2 D10 的"绑源地址"依赖强主机模型，网卡开了转发或 weak host send——Hyper-V、WSL、ICS 常这样——时不成立，在虚拟网卡成了默认路由之后这个前提是承重的，所以两样都做）。经 `SocketHook` 接上，主网卡变了之后新的套接字绑新的。写了 `interface` 参数的策略照它自己的。**今天并不是每个出站套接字都经过钩子**：连接器只在策略写了 `interface` 时才调它（`rurge-net/src/connector.rs`），解析器的纯 UDP / TCP 上游自己建套接字、不经连接器；M4 把 rurge 自建的出站套接字收到一个统一的建法上（增强模式开着时，没写 `interface` 的也绑物理网卡），并用一条用例断言没有漏网的——漏一处就是解析器自己的查询进了虚拟网卡、被应答器答成 fake 地址再被当成真地址拨号。

**macOS 上是没有验证过的假设**：`0/1` + `128/1` 压过全局默认路由之后，绑了 `IP_BOUND_IF` 的套接字靠物理网卡的作用域默认路由出去——调研报告（3.3 节、9.2 节 E4）说这一组合要在真机上验证，sing-tun 在 macOS 上用的是八个前缀而不是一对 `/1`（理由没有读到）。M4 的细化设计要带着退路：给 rurge 要连的代理服务器与 DNS 服务器的地址加经物理网关的主机路由，或改用 sing-tun 式的多个前缀；手工验收有专门的一项。Windows 的"绑源地址"与 `IP_UNICAST_IF` 同样只在真机上验得了（调研 E3）。

外部程序（`external` 策略拉起的进程）：它们的服务器地址（`addresses`）从虚拟网卡的路由里排除；它们自己与它们的子进程发出的、仍进了虚拟网卡的连接一律 DIRECT（手册），按进程识别得到的 PID 与外部程序的进程树判断（M4）。

系统解析库（`.local`、`force-syslib`）不经 rurge 的套接字，管不了，见第 9.3 节。

### 9.5 开关、生命周期与恢复

- **开关**：`rurge run --enhanced-mode`（环境变量 `RURGE_ENHANCED_MODE`），运行中经 `POST /v1/features/enhanced_mode` 开关，`GET` 读真实状态；不写进 Profile（清单 9.1 节）。清单 10.3 节原来计划的 `--tun` 改为 `--enhanced-mode`（与 Surge 的叫法与 API 的名字一致）。是否像系统代理一样只在启动参数里指定、不跨重启保留，在 M4 定。
- **打开**：权限检查 → 设备 → 地址、MTU、起网卡 → 起 `rurge-tun`（先能收包）→ 路由 → 系统 DNS；每一步之前把撤销它的步骤写进 `state.json`，**写不进就不做这一步**（今天的 `StateStore::update` 写失败只记日志、不报错，M4 要用一个返回错误的写入口）；任何一步失败就按相反的顺序撤销已做的，`rurge run` 以 1 退出（同系统代理），API 回 500 并带原因。
- **关闭与正常退出**：恢复系统 DNS → 撤路由（流量先回到物理网卡）→ 停 `rurge-tun`（虚拟网卡上的会话结束）→ 关设备 → 清掉备份。
- **路由的条数**：从 `/1` 里减掉一个 `/32` 就多出三十来条前缀，排除项一多就是几百条；一条路由一个子进程在 Windows 上要很久，M4 要成批执行（Linux `ip -batch`、Windows 一个 `netsh` 脚本）或量过再定。
- **崩溃之后**：下一次 `rurge run` 拿到数据目录的实例锁之后、做任何新改动之前，按备份撤销上一次留下的（删不存在的东西不算失败）；配置读不了、有错误时也先做这一步（同阶段 2 / M8c 任务 8 的系统代理恢复）。
- **重载**：Profile 重载时比较新旧的路由、`hijack-dns`、`ipv6-vif` 等，只改变了的部分，不重建设备。
- **网络变化**：`ipv6-vif = auto` 重判；macOS 上新出现的网络服务也接管 DNS。

### 9.6 权限

Windows 要管理员（Wintun 建适配器回 `ERROR_ACCESS_DENIED`），Linux 要 root 或 `CAP_NET_ADMIN`（`TUNSETIFF` 回 `EPERM`；防环的 `SO_BINDTODEVICE` 在老内核上还要 `CAP_NET_RAW`），macOS 要 root（`connect` utun 回 `EPERM`）。权限不足时一句英文错误说明要什么（FR-IN-06、NFR-10），不尝试提权。

### 9.7 `suspend`（接管暂停）

进入一个 `[SSID Setting]` 里带 `suspend=true` 的网络时停止接管（下文与代码里叫"接管暂停"，与托管配置 `strict` 的 `Engine::suspend` 分开）：开着的系统代理恢复成使用者原来的设置，开着的增强模式关闭（第 9.5 节的关闭，备份照写）；监听器与 API 照常，经代理端口来的请求照常处理（它们是使用者明确指过来的）。离开这个网络时恢复暂停之前开着的那些。暂停只在内存里，不进 `state.json`；`GET /v1/features/system_proxy` 与 `enhanced_mode` 给出的是真实状态（暂停时是 `false`），暂停的原因另有说明（形状在 M1 / M4 定）。手册对桌面的 `suspend` 没有写，iOS 上是"隧道不断、Surge 只是不处理"，差异登记在清单。

## 10. 识别与阻断（FR-RULE-08、FR-OUT-11，M5）

- **QUIC**：引擎在 UDP 流的第一个包上已识别 QUIC Initial（长包头、固定位、v1 / v2 / draft 版本、Initial 类型、不少于 1200 字节），今天只在目的端口 443 上识别，识别出的流在拨号之前带上 `PROTOCOL` 的 `QUIC`、并用于 `block-quic`（`rurge-engine/src/udp.rs`）；M5 定识别是否扩到所有端口（要权衡误判：别的 UDP 协议恰好像 QUIC Initial 的可能）与阻断的方式（经虚拟网卡时可以回 ICMP 不可达让客户端更快回落，经 SOCKS5 时只能丢弃）。
- **STUN**：按 RFC 5389 的魔数（第 4 ～ 7 字节 `0x2112A442`）、前两位为 0、长度字段一致识别，`PROTOCOL,STUN` 匹配。
- **`block-quic`**：引擎已有的 `quic_blocked`（全局四种覆盖 × 策略参数 × 直连与否）对经虚拟网卡来的 UDP 流同样生效。
- **`udp-priority`**：系统负载高时先处理 UDP——包的去向里每批先处理 UDP，写回设备时 UDP 的队列优先；"尽力实现"（需求文档）。
- **`auto-quic-block`**（`[MITM]`，清单 L582 标 3 / 4）：MITM 在阶段 4，本阶段按 `[MITM] hostname` 的名单判断目标域名（经虚拟网卡来、目标是 fake 地址反查出的域名时才知道），命中就阻断这条 QUIC 流；QUIC 里的 SNI 随阶段 4 的嗅探。
- 发行说明里手册还没写的三件（`block-quic=adaptive`、跨几个包的 QUIC ClientHello、v2 与 draft-29）不在本阶段（v2 与 draft-29 的 Initial 今天已识别）。

## 11. 控制面与命令行

- `GET` / `POST /v1/features/enhanced_mode`：读真实状态、开关（Surge 只有 Mac 有，rurge 三平台都有）；失败时 500 带原因，同 `system_proxy`。
- `rurge run --enhanced-mode`、`--wintun-dll <路径>`（只有 Windows）。
- 请求记录：经虚拟网卡来的会话 `listener` 为 TUN；带进程（M2）；形状与 API 参考在各里程碑写进 `docs/api/phase3.md`（新建）。
- `rurge check`：增强模式相关键的诊断（私网整段写进 `tun-included-routes`、`ipv6-vif = always`——手册："This may break apps on networks without IPv6 connectivity, and Surge warns about it during profile verification"——、`force-syslib` 的回环风险等）。

## 12. 错误处理与可观测性

- 错误与日志以英文为主（NFR-10）；权限、Wintun 找不到、命令失败（`rurge_platform::command::failure` 已把参数里最后一个 `@` 之前换成 `***`）各有一句说明怎么办的话。
- 不进日志、错误、API 与 `Debug` 的仍是凭据（项目的规矩不变）。另有两类个人信息只在 DEBUG 里出现：SSID / BSSID（使用者在哪里）、进程路径（Windows 上常带用户名）——请求记录与 API 照 Surge 带进程路径。
- 计数：丢弃的包按原因计数（坏包、分片、不支持的协议、队列满、未知 fake 地址），在日志与 API（形状在 M3 定）里看得到。

## 13. 测试策略

**三层**（调研报告第 8 节）：

1. **L1：内存里的设备（缺省 `cargo test`，三平台，无特权，主力）**。设备抽象成 trait，测试用一对内存设备；包用 `smoltcp::wire` 现场构造；"应用端"的 TCP 用另一个 smoltcp 栈（同 WireGuard 的测试），可以注入丢包与损坏；ICMP 发送、进程查找、预匹配、网络环境都用假实现；上游是回环上的测试服务器。覆盖：TCP 往返与半关、RST、拒绝；UDP 会话、fake 地址的回包来源；DNS 应答器的每一种应答、池的回绕与持久化、`hijack-dns`；预匹配的三种拒绝；ping 的转发；`[IP Rewrite]`；网络变化的下游；`subnet` 与 `SUBNET`；`[SSID Setting]`。路由、地址、DNS 的**计划**作为数据测（三个平台的计划在任何平台上测，测试从不执行它们）；网络环境与 SSID 的读取用文本夹具测解析；进程识别在各平台上**只读**地查测试进程自己的回环连接。
2. **L2：特权用例（只在一次性的 CI 虚拟机里，环境变量门控）**。独立的测试 crate（照 `tests/interop`、`tests/external`），只在 `RURGE_TEST_PRIVILEGED=1` 时运行：Linux 上在新的网络命名空间里建真实的虚拟网卡、装路由、过一个 TCP 往返与一次 ping、`kill -9` 之后再启动能恢复。它们按定义要改系统的网络设置，是"只用回环"规矩的明确例外，**开发机上从不运行**；是否在 CI 上启用、macOS 与 Windows 是否加冒烟由项目所有者定（第 16 节 Q4）。
3. **L3：手工验收**（`docs/acceptance/phase3-manual.md`，新建）：三个平台的真实网卡、路由、DNS、换网络、睡眠唤醒、SSID、`PROCESS-NAME`、`ping`、性能。

**沿用的规矩**：测试只用回环地址和端口 0（L2 是上面写明的例外）；在失败时也不得离开回环与自己的临时目录；有界等待；起 `rurge` 二进制的用例经 `rurge_command()` / `rurge_process()` 并给 `--data-dir`。**新的规矩**：缺省的 `cargo test` 从不执行地址、路由、系统 DNS 与设备的命令或 FFI（只断言计划），从不建网卡——L2 是唯一的例外，只在 CI 虚拟机里；进程识别的用例只读系统的表；网络环境的用例不执行会改设置的命令（只读的命令——如 `netsh wlan show interfaces`——也只在专门的、可跳过的用例里跑，缺省用夹具）。**测试后端**：照 `RURGE_SYSTEM_PROXY_BACKEND=file:<路径>` 的先例，网络环境有一个只供测试的文件后端（M1，用例改写文件来模拟网络变化），起 `rurge` 二进制的辅助函数缺省就设它——否则真实的网络变化会在用例中途清 DNS 缓存、作废测速、重建 WireGuard；虚拟网卡同样有一个内存后端供起守护进程的用例用（M3 / M4）。

**基准**（忽略的用例，不作门禁）：TCP 单流吞吐与 1 万连接的内存（M3 的门槛）、fake 应答的延迟（NFR-01：< 0.2 ms）、进程查找的延迟（M2）。

每个任务结束的门禁不变：`cargo fmt --all --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`。

## 14. 阶段 3 验收标准（对应需求文档第 7 节）

1. 开启增强模式后，浏览器、命令行工具与不支持代理的应用的流量出现在请求记录里并按规则分流——L1 的端到端用例（TCP、UDP、DNS 经内存设备与引擎到回环上游）通过；三个平台的手工验收。
2. `ping` 正常——L1（假 ICMP 发送）通过；手工验收。
3. fake-IP 映射与真实连接一致——L1（应答器给出的地址、连接到来时反查的域名、规则看到的目标、回包的来源）通过；手工验收。
4. 网络切换后 30 秒内恢复——L1（假探测器：去抖与下游的时序）通过；手工验收（真实的换网络与睡眠唤醒）。
5. `PROCESS-NAME` 在三个平台命中——三个平台的 CI 上查测试进程自己的用例通过；手工验收。
6. 性能基准达到 NFR-01 目标的 70% 以上——内存里的基准（第 3.1 节的门槛与 fake 应答延迟）达到；真实设备上的数字在手工验收里量。
7. 阶段 3 的需求逐句对照过代码（照阶段 1 的做法），三个数照实写。
8. 三平台 CI 绿；没有证据的照实写，不写成通过（阶段 1、2 的验收至今卡在这一条）。

## 15. 风险

| 编号 | 风险 | 影响 | 应对 |
| ---- | ---- | ---- | ---- |
| A | smoltcp 0.12 的线性查找、固定缓冲与已知缺陷 | 吞吐、内存、少数连接卡住 | 分片与缓冲预算；M3 的基准门槛；不达标换自写 TCP（第 3.1 节）；升 0.14 等所有者定 |
| B | 平台代码在开发机上测不了 | 三个平台的设备、路由、DNS、进程识别（macOS）、网络环境的缺陷要到 CI 与手工验收才暴露 | 计划作为数据测；平台适配层薄；L2 与手工清单；CI 一直没绿过，这条风险现在最大 |
| C | SSID / BSSID 拿不到 | `SSID:` / `BSSID:` 条件不生效 | 设计成正常状态并说明原因；`ROUTER:`、`TYPE:` 照常；macOS 的 App 包留给所有者 |
| D | DNS 泄漏（Windows 多宿主解析、浏览器 DoH） | 规则看到的是 IP 而不是域名 | canary 域、拒绝 type 65；文档说明；严格模式是开放问题 |
| E | 回环（rurge 自己的连接进了虚拟网卡） | 一个核烧满、网络断 | 每个出站套接字收到统一的建法上、绑物理网卡，一条用例断言没有漏网的；Windows 两样都做；macOS 的组合没有验证过，M4 带着退路（第 9.4 节）；手工验收专门的一项 |
| F | 崩溃之后留下的系统改动 | 使用者的网络或 DNS 坏掉（macOS 上被强行结束之后整机解析失效，直到 rurge 再次启动） | 先写撤销步骤（写不进就不做）再改；下一次启动先恢复；配置坏了也恢复；手工恢复的办法写进文档 |
| G | Wintun 的分发与许可 | Windows 上装不上或许可问题 | 不进仓库、不下载；旁置加载；分发由所有者定 |
| H | 受管机器上的安全软件拦虚拟网卡或驱动 | 开发机 / 所有者的机器上增强模式起不来 | 错误说清楚；手工验收记录；不尝试绕过 |
| I | 范围大 | 迟迟没有可用版本 | 不需要网卡的 M1、M2 先交付；M3 在内存里完成；M4 接上就能手工验收 |

## 16. 已决事项与开放问题

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| P3-D1 | 里程碑与顺序 | 第 1.4 节：网络环境 → 进程识别 → TUN 内核（内存设备）→ 平台接入 → 识别与阻断 → 收尾 |
| P3-D2 | TCP 的用户态协议栈 | smoltcp 0.12 分片 + 缓冲预算 + Reno；M3 第一个任务的基准门槛；不达标换自写 TCP 被动一端；不做内核 TCP + 地址改写（第 3.1 节） |
| P3-D3 | 依赖 | 不引入新的包；只给已有的包开不带新依赖的特性——`nix` 的 `socket` / `net` 要 `memoffset`，不开（第 3.2 节） |
| P3-D4 | unsafe | 新增的全部在 `rurge-platform`，逐函数放开，一条用例把名单钉住；`rurge-tun` 保持 `forbid`（第 3.6 节） |
| P3-D5 | 设备层 | 自己写薄的一层：Linux 一处 `ioctl`、macOS 经 `socket2` 加一处 unsafe（`ioctl` 与 `connect`）、Windows 动态加载并校验签名的 Wintun；不带、不下载 `wintun.dll`（第 3.3 节） |
| P3-D6 | 地址、路由、系统 DNS | 系统命令；计划 + 执行器；撤销步骤先写进 `state.json`（第 3.4 节） |
| P3-D7 | 路由模型 | 虚拟网卡上装 `0/1` + `128/1`（IPv6 `::/1` + `8000::/1`）；排除路由求补集；包含路由另装（第 9.2 节） |
| P3-D8 | 系统 DNS | Windows 设在虚拟网卡上；Linux 经 systemd-resolved，没有时不改并告警；macOS 逐网络服务设并备份；接管期间的 `system` 取物理网卡自己的服务器（第 9.3 节） |
| P3-D9 | ICMP | 照手册转发，不本机伪造；需求文档 FR-IN-06 订正（第 2.2、8.4 节） |
| P3-D10 | 经虚拟网卡来的 TCP | 握手照常完成（预匹配在 SYN 处拒绝的除外）后拨号；阶段 3 不等首包；失败回 RST（第 8.2 节） |
| P3-D11 | 经虚拟网卡来的 UDP | 应用的每个源端点一个会话，交给引擎现有的 UDP 流水线（第 8.3 节） |
| P3-D12 | fake-IP | 顺序分配、回收最久没用的、持久化（第 8.5 节） |
| P3-D13 | 网络环境 | 快照 + 事件（Windows 与 Linux 轮询、macOS 路由套接字）+ 1 秒首发尾发去抖；SSID 拿不到是正常状态；只供测试的文件后端（第 5、13 节） |
| P3-D14 | 进程识别 | Windows 连接表、Linux `/proc/net` 与 inode、macOS `sysctl` 的 PCB 表；本机来源的会话在规则之前查，缓存与限频，超时当作不知道；`SessionInfo.process` 加 PID（第 7 节） |
| P3-D15 | `suspend` | 需求文档的读法：撤掉虚拟网卡与系统代理，监听器与 API 照常；叫"接管暂停"，与 `strict` 的 `Engine::suspend` 分开（第 9.7 节） |
| P3-D16 | `[SSID Setting]` 与 `subnet` 组 | 参数从第一个前面有空白的 `<认识的键>=` 开始；列表与参数的分法照清单 9.4 节；几行命中取第一行；`subnet` 组能临时覆盖、网络变化时清除，没写 `default` 是加载错误（第 6.2、6.3 节） |
| P3-D17 | Wintun | 旁置、绝对路径加载、加载之前校验签名者是 WireGuard LLC、固定名字与 GUID、复用残留的适配器（第 3.3 节） |
| P3-D18 | 开关 | `--enhanced-mode`（取代清单里计划的 `--tun`）与 API；不写进 Profile（第 9.5 节） |
| P3-D19 | 测试 | 三层；L1 是主力；L2 只在 CI 虚拟机里、环境变量门控（第 13 节） |
| P3-D20 | IPv6 | `ipv6-vif` 缺省 `disabled`；`auto` 按物理网络有没有全局 IPv6 判；fake 地址在 `fd00:6152::/64` 里（第 8.5 节） |
| P3-D21 | 防环 | rurge 自建的每个出站套接字收到统一的建法上、绑物理网卡，用例断言没有漏网的；Windows 加 `IP_UNICAST_IF`；macOS 的组合未验证、带退路（第 9.4 节） |
| P3-D22 | 网络变化的下游 | 第 5 节的八项；`smart` 不动 |
| P3-D23 | 分片与组播 | 进了虚拟网卡的 IP 分片、组播、广播丢弃并计数（第 8.1 节） |
| P3-D24 | `[IP Rewrite]` | 本阶段实现（M5），归在 FR-IN-06 之下 |

开放问题（留给项目所有者；本阶段按上表做，所有者的答复改变的部分在当时的里程碑里改）：

| 编号 | 问题 |
| ---- | ---- |
| Q1 | 把 MSRV 升到 1.91、换 smoltcp 0.14（零窗口探测、一次 `poll` 吐尽出口、Reno / CUBIC 合规、`pause_synack`），要不要？ |
| Q2 | 设备、路由、网络环境改用 `tun-rs`、`rtnetlink`、`wintun-bindings`、`system-configuration`、`zbus` 等现成的包，要不要？（要审许可与传递依赖）退一步：开 `nix` 的 `socket` / `net` 特性（多一个 `memoffset` 包，MIT），换来 macOS utun、Linux netlink 与 `sock_diag` 的安全封装，要不要？ |
| Q3 | Windows 的发行包附带官方签名的 `wintun.dll`（原样、旁置、带许可文本）还是要使用者自己取？是否先向 WireGuard LLC 书面确认能否嵌进可执行文件？ |
| Q4 | L2 特权用例在 CI 上启用（Linux 命名空间；macOS / Windows 冒烟要在 CI 里取 `wintun.dll`）？ |
| Q5 | Windows 防 DNS 泄漏的严格模式（WFP 动态会话阻断别的接口的 53 端口）要不要做？ |
| Q6 | macOS 上拿 SSID：接受"命令行守护进程拿不到"，还是以后做一个带位置权限的 App 包辅助程序？ |
| Q7 | NFR-01 的"1 万连接 < 300 MB"在用户态栈下怎么量（进程常驻内存）；真实设备上的数字由谁、在哪台机器上量？ |
| Q8 | 需求文档第 4 节要不要为 `[IP Rewrite]` 另立一行；FR-IN-10 的 `suspend` 与 FR-OUT-11 的"UDP 443"按本设计的读法改字面吗？ |

## 17. 需同步的文档

| 文档 | 改动 | 时机 |
| ---- | ---- | ---- |
| `docs/requirements.md` | 3.2 节 `rurge-tun` 与 `rurge-platform` 两行的职责；FR-IN-06 的"ICMP 本地应答"改为"ICMP 转发"；FR-IN-07 的"macOS 通过 libproc"与第 6 节平台矩阵里进程识别、Wi-Fi 信息、网络变化事件三行的做法（第 2.2 节）；9.2 节 Q7 写上本设计的决定；风险 R2 写上调研的结论 | 随本文档 |
| `CLAUDE.md` | 「先读这些文档」加入本文档与两份报告；状态一段记下阶段 3 开始 | 随本文档 |
| `docs/surge-compatibility-matrix.md` | 各行的状态随里程碑更新；调研报告里"手册多说了"的 25 行与写得过于肯定的四处；L335 的阶段；L453 没写 `default` 的 `subnet` 组；10.3 节 `--tun` 改为 `--enhanced-mode` | 各里程碑收尾（四处措辞与 L453 在 M1） |
| `README.md`（中英） | 状态、特性表与路线图 | 各里程碑收尾 |
| `docs/api/phase3.md` | 新建：增强模式的开关、请求记录的进程与 TUN 字段、网络状态（若有） | M1 / M2 / M4 |
| `docs/acceptance/phase3-manual.md` | 新建：三个平台的真实网卡、路由、DNS、换网络、SSID、`PROCESS-NAME`、`ping`、性能 | M1 起逐里程碑加 |
| `docs/acceptance/phase3-acceptance.md` | 新建：第 14 节逐条的证据与需求逐句对照代码的结果 | M6 |
