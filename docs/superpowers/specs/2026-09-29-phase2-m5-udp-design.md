# 阶段 2 / M5 细化设计：UDP 路径

- 日期：2026-09-29
- 状态：项目所有者逐节确认（2026-09-29），待审阅全文
- 依据：`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`（下称"总设计"）第 1.4 节 M5 一行、第 5.1 节、第 6 节、第 8.2 / 8.3 节、第 9 节 `dns-follow-interface`、第 16 节 D5；需求文档 FR-IN-02（UDP）、FR-OUT-03 / 07 / 08 / 11、FR-DNS-10；M3c 计划「延后事项」#6、M4b 计划「延后事项」#2 / #15 / #24、M4c 计划「延后事项」#5、M1a / M1b 计划延后的 `dns-follow-interface` 与 `udp-relay`；Surge 手册 `profile/general.html`、`policies/udp.html`、`policies/parameters.html`、`policies/socks5.html`、`policies/anytls.html`、`policies/external.html`、`policies/reject.html`、`rules/protocol-and-network.html`（2026-09-29 读取）
- 与总设计的关系：本文细化总设计的 M5 里程碑。两者不一致处以本文为准，差异列在第 13 节。

## 1. 目标与范围

### 1.1 目标

rurge 的 UDP 真正可用：阶段 2 没有 TUN，UDP 从 SOCKS5 的 UDP ASSOCIATE 进来，按规则分流到 DIRECT 或代理，代理侧支持 `socks5`（`udp-relay`）、`external`（`udp-relay`）、`trojan`、`vmess`、`anytls`、`wireguard`。项目所有者的实际用途（2026-09-29）：浏览器的 QUIC / HTTP3、游戏 / 语音 / 视频通话、DNS 等短查询——所以 UDP 按**全锥（Full Cone）NAT** 做，QUIC 阻断与短查询的回收都要认真对待。

### 1.2 范围内

| 需求 | 内容 |
| ---- | ---- |
| FR-IN-02（UDP） | SOCKS5 UDP ASSOCIATE 入站 |
| FR-OUT-07 | UDP 中继：`udp-relay`；不支持时按 `udp-policy-not-supported-behaviour` |
| FR-OUT-08（UDP） | `underlying-proxy` 的 UDP 载体（含 `wireguard` 经 `underlying-proxy`） |
| FR-OUT-11 | `block-quic`（策略级与全局四种覆盖；阶段 2 对 SOCKS5 进来的 UDP 生效） |
| FR-OUT-03 | `test-udp` / `proxy-test-udp`、`dns-follow-interface` 生效；`ecn` 仍只解析（M5-D6） |
| FR-DNS-10 | `dns-follow-interface` |
| M3c #6 | `smart` 计入 UDP |
| M4b #2 / #15 / #24 | WireGuard 的 UDP 与 `underlying-proxy`；"peer 连不上"告警限频；`Unsupported` 说明不覆盖已有说明 |
| M4c #5 | `external` 的 `udp-relay` |

### 1.3 范围外

- TUN 与增强模式下的 UDP / QUIC 阻断（阶段 3）。
- `ss`（Shadowsocks）、`snell`、`h2-connect` 的 UDP：协议本身在 M6 实现，随 M6 一起做（`udp-port` 同样在 M6）。
- QUIC 族出站（TUIC、Hysteria 2、MASQUE）：M7。
- VMess 的 XUDP（全锥）：延后（M5-D5）。
- `ecn`（M5-D6）。
- UDP 流经 HTTP 引擎、MITM：不适用。

### 1.4 三份计划

| 计划 | 内容 |
| ---- | ---- |
| **M5a 地基** | `PacketSocket` 与 `Outbound` 的 UDP 方法；SOCKS5 UDP ASSOCIATE 入站；引擎 UDP 流水线（全锥、请求记录、回收、上限）；DIRECT / REJECT；`udp-policy-not-supported-behaviour`；`block-quic`；`socks5` / `socks5-tls` / `external` 的 `udp-relay`；`ChainConnector::connect_udp` |
| **M5b TLS 族** | `trojan`、`vmess`（命令 2，对称型）、`anytls`（UDP over TCP v2）的 UDP；回环假服务端与互操作 |
| **M5c WireGuard 与其余** | `wireguard` 的 UDP 与经 `underlying-proxy`；M4b #15 / #24；`test-udp` / `proxy-test-udp`；`smart` 计入 UDP；`dns-follow-interface` |

每份计划开工前核对第 15 节里属于它的事项。

## 2. 已确认的决定

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| M5-D1 | 用途与优先级 | 浏览器 QUIC、游戏 / 语音 / 视频、DNS 短查询都要支持；UDP 出站经 `trojan` / `vmess` / `anytls`、`wireguard`、`socks5` / `external`（项目所有者，2026-09-29） |
| M5-D2 | NAT 类型 | 全锥：同一客户端关联在同一出站上共用一个载体，任何来源的回包都送回客户端（项目所有者，2026-09-29） |
| M5-D3 | 拆分 | 三份计划：M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余（项目所有者，2026-09-29） |
| M5-D4 | 抽象 | 出站侧新增按包收发、带地址的 `PacketSocket`；`Datagram`（到固定对端）保留给 WireGuard 与 M7 的 QUIC 当载体（第 4 节） |
| M5-D5 | VMess 的 UDP | 按标准命令 2 做，每个目标一条 VMess 连接，对称型；XUDP 延后（项目所有者，2026-09-29） |
| M5-D6 | `ecn` | M5 里仍只解析、`W0029`，延后（项目所有者，2026-09-29） |
| M5-D7 | 空闲回收 | 流 60 秒无收发回收；目标端口 53 的流收到回答后 10 秒回收；rurge 自定，登记为差异 |
| M5-D8 | QUIC 识别 | 目标 UDP 443 且首包是 QUIC 长首部 Initial；其它端口不识别 |
| M5-D9 | 全局 `block-quic` 的含义 | 按取值名理解（第 6.2 节），手册未解释，登记为"未与 Surge 核对" |
| M5-D10 | REJECT 对 UDP | 四种 REJECT 一律丢包并记录；SOCKS5 回不了 ICMP，登记为差异 |
| M5-D11 | 上限 | 每个关联最多 1024 条流，全进程最多 4096 个关联；超出时丢弃新流并限频告警 |

## 3. crate 改动一览

| crate | 改动 |
| ----- | ---- |
| `rurge-config` | `BlockQuicGlobal` 补 `All`、`AlwaysAllow`；`W0029` 随各计划退役（`udp-relay`、`block-quic`，M5a；`test-udp`、`dns-follow-interface`，M5c；`wireguard` 的 `underlying-proxy` 专用告警，M5c）；`SessionInfo::udp` 构造 |
| `rurge-net` | `PacketSocket` 与 `BoxedPacketSocket`；`DirectConnector::open_udp`（未连接的 UDP socket，带 socket 选项）；QUIC Initial 的识别函数（纯函数，供引擎用） |
| `rurge-proto` | `Outbound::udp()` 与 `Outbound::open_udp()`（默认不支持）；DIRECT 的 UDP；SOCKS5 UDP 客户端（`socks5`、`socks5-tls`、`external` 共用）；M5b：trojan、vmess、anytls 的 UDP；`testing` 的假服务端加 UDP |
| `rurge-proto-wireguard` | M5c：协议栈的 UDP socket 作为包载体；peer 载体经 `Connector::connect_udp`（链式）；告警限频 |
| `rurge-policy` | `ChainConnector::connect_udp`（把底层出站的包载体包成到固定服务器的 `Datagram`）；`TestBook` 的 UDP 测试（M5c）；`smart` 的 UDP 回报（M5c） |
| `rurge-inbound` | SOCKS5 UDP ASSOCIATE：关联、UDP 端口、客户端地址校验、包的解析与封装；与引擎之间的 UDP 接口 |
| `rurge-engine` | UDP 流水线（第 5 节）；请求记录的 `transport`；`block-quic`；`udp-policy-not-supported-behaviour`；上限与回收；`dns-follow-interface` 的解析路径（M5c） |
| `rurge-dns` | M5c：按网卡的一组上游连接（`dns-follow-interface`） |
| `rurge-api` | 请求记录的 `transport` 字段（JSON 形状加一个键） |
| `rurge`（bin） | 无新命令；能力表不变（UDP 不是新的策略类型） |

依赖方向不变：`rurge-proto → rurge-net → rurge-config`；`rurge-inbound` 不依赖 `rurge-proto`（UDP 与 TCP 一样经 `Dialer` 一类的 trait 交给引擎）。

## 4. 抽象（M5a）

### 4.1 `PacketSocket`

```rust
/// 一个客户端关联在一个出站上的 UDP 载体：按包收发，每个包带目的地址或来源地址。
pub trait PacketSocket: Send + Sync {
    fn poll_send_to(&self, cx: &mut Context<'_>, buf: &[u8], to: &Target) -> Poll<io::Result<()>>;
    /// 来源地址：代理协议回的地址（可能是域名），DIRECT 是真实的对端地址。
    fn poll_recv_from(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<Target>>;
}
pub type BoxedPacketSocket = Box<dyn PacketSocket>;
```

轮询式（与 M4b 的 `Datagram` 一致）：引擎的一个任务要同时等客户端与几个载体。收到的包截断时丢弃，不交给客户端。

### 4.2 `Outbound`

```rust
pub enum UdpSupport { Native, Unsupported }

pub trait Outbound: Send + Sync {
    // 既有方法……
    fn udp(&self) -> UdpSupport { UdpSupport::Unsupported }
    /// 为一个客户端关联开一个载体；`opts.timeout` 管开载体本身。
    fn open_udp<'a>(&'a self, opts: &'a ConnectOpts)
        -> BoxFuture<'a, Result<BoxedPacketSocket, OutboundError>> { /* Unsupported */ }
}
```

`udp()` 由构建时的参数决定（例如 `socks5` 没写 `udp-relay=true` 时是 `Unsupported`）。`http` / `https` / `ssh` 不支持；DIRECT 与第 7 节列出的协议支持。

### 4.3 链式 UDP 载体

`ChainConnector::connect_udp(to)`：从当前一代注册表解析底层策略（与 TCP 相同），调用其出站的 `open_udp`，再包成"只发往 `to`、只收来自 `to` 的包"的 `Datagram`。底层出站不支持 UDP 时返回 `Unsupported`（文本 `the underlying policy cannot carry UDP`）。用于：`socks5` 的 UDP 中继段、`wireguard` 的 peer 载体。

## 5. 入站与引擎流水线（M5a）

### 5.1 SOCKS5 UDP ASSOCIATE

- 收到命令 0x03：在 TCP 控制连接的本机地址上开一个 UDP 端口（端口 0），应答里给出这个地址与端口（RFC 1928）。
- 只接受来自控制连接那个客户端 IP 的包：请求里的端口不为 0 时只认那个端口，为 0 时以第一个包的源端口为准；其它来源的包丢弃。
- `FRAG ≠ 0` 的包丢弃（不支持分片）。
- 关联跟随 TCP 控制连接：控制连接关闭（或读到 EOF）时，关联的流与载体全部关闭。控制连接上客户端之后发来的任何数据都丢弃。
- 监听地址与 TCP 相同，不新增配置项；`socks5-listen` 的访问限制（阶段 1 的 `restrict`）同样作用于 UDP ASSOCIATE。

### 5.2 流

- 一条流 = 关联 + 目标（主机与端口，原样，不先解析）。
- 首包：与 TCP 相同的顺序——规则匹配（域名规则不触发 DNS，IP 规则按需解析；没有预匹配阶段）→ 出站模式 → 策略解析（组、链、别名）得到终端出站。`SessionInfo` 的 `transport = Udp`；协议嗅探对首包识别 QUIC（M5-D8）、STUN、DNS，写进 `protocol`，`PROTOCOL` 规则据此匹配。
- 判定顺序：`block-quic`（第 6.1 节）→ 终端出站 `udp()`，不支持时按 `udp-policy-not-supported-behaviour`（第 6.3 节）→ 取或开载体。
- 每条流一条请求记录：`transport: "udp"`，`dst` 是目标，上下行字节，状态与说明。
- 开载体失败：这条流记为失败并写说明，之后发往这个目标的包直接丢弃，直到流被回收。

### 5.3 载体与全锥

- 载体按"关联 × 终端出站对象"共用，开一次、到关联结束或其上的流都回收为止。
- 回包：来源等于某条流的目标时计入那条流；否则（全锥的新来源）计入这个载体上最早的一条流，并在该记录上标注"收到其它来源的包"。回包一律以其来源地址封装后发给客户端。
- DIRECT 的流在建流时解析一次目标域名（含 `[Host]`），这条流固定发往那个地址；回包按真实地址归流，发给客户端时来源写真实地址。

### 5.4 回收与上限

- 流：60 秒没有收发即回收；目标端口 53 的流收到第一个回答后 10 秒回收（M5-D7）。
- 载体：关联结束或其上的流都已回收时关闭。
- 每个关联最多 1024 条流，全进程最多 4096 个关联；超出时新流（或新关联的 UDP ASSOCIATE，回 0x01 一般失败）被拒绝，每分钟至多一条 `warn`（M5-D11）。

### 5.5 `smart` 组

M5a / M5b 里 UDP 流只用组当前选中的成员，不打分、不换成员；M5c 接上回报（第 8.3 节）。

## 6. QUIC 阻断与相关配置（M5a）

### 6.1 识别

目标端口 UDP 443，且首包首字节高两位为 `11`（长首部）、版本号（第 2 ～ 5 字节）不为 0 且包类型为 Initial（M5-D8）。

### 6.2 覆盖规则

- 策略级 `block-quic`：`auto`——终端是代理时阻断、DIRECT 时不阻断；`on`——阻断；`off`——不阻断。组按解析出的终端策略判断。
- 全局 `block-quic`（M5-D9）：`per-policy`（默认）用策略自己的设置；`all-proxy` 凡经代理一律阻断；`all` 连 DIRECT 也阻断；`always-allow` 一律不阻断。配置层补上 `all` 与 `always-allow`。
- 被阻断的流：丢包，请求记录为 REJECT、说明 `QUIC blocked`；不计入阶段 1 的 REJECT 自动升级。

### 6.3 `udp-policy-not-supported-behaviour`

终端出站 `udp()` 为 `Unsupported` 时：`REJECT`（默认）——丢包并记录 `policy does not support UDP`；`DIRECT`——改用 DIRECT 的载体，请求记录注明回退。

### 6.4 REJECT 系列

REJECT、REJECT-DROP、REJECT-NO-DROP、REJECT-TINYGIF 对 UDP 一律丢包并记录（M5-D10）。

## 7. 各协议的 UDP

| 协议 | 计划 | 载体 | NAT |
| ---- | ---- | ---- | ---- |
| DIRECT | M5a | 未连接的 UDP socket，带策略的 socket 选项（`interface`、`tos`、`ip-version` 决定地址族） | 全锥 |
| `socks5` / `socks5-tls` | M5a | 对服务器 UDP ASSOCIATE：TCP 控制连接 + 服务器给出的 UDP 中继地址；每个包带目的地址；中继地址是 `0.0.0.0` / `::` 时改用控制连接的对端地址；控制连接断开即载体失效；`socks5-tls` 的 UDP 是明文 UDP（只有控制连接是 TLS）；有 `underlying-proxy` 时 UDP 段经链式载体 | 全锥 |
| `external` | M5a | 同 `socks5`，连 `127.0.0.1:<local-port>`；拉起与重试沿用 M4c | 全锥 |
| `trojan` | M5b | 一条 TLS 连接上的 UDP ASSOCIATE（命令 0x03），每个包 `地址 + 长度 + CRLF + 载荷`；WebSocket、Shadow TLS 照常 | 全锥 |
| `vmess` | M5b | 命令 2，每个目标一条 VMess 连接（M5-D5） | 对称 |
| `anytls` | M5b | UDP over TCP v2（`sp.v2.udp-over-tcp.arpa`，非连接模式），每个包带地址，走会话复用 | 全锥 |
| `wireguard` | M5c | 隧道协议栈里的 UDP socket（本端隧道地址 + 空闲端口），每个包按目的地址选 peer；每个 socket 64 个包、256 KiB 缓冲 | 全锥 |

目标是域名时：代理协议把域名原样发给服务器（远端解析，与 TCP 一致）；DIRECT 在本机解析；`wireguard` 与 TCP 相同（隧道内 DNS 或本机解析）。

## 8. M5c 的其余部分

### 8.1 `wireguard` 经 `underlying-proxy`

peer 的载体改用 `ChainConnector::connect_udp`；中继不支持 UDP 时照旧 REJECT 并附说明。`wireguard` 专用的 `W0029`（"本版本不可用"）退役，改为只在中继不支持 UDP 时于拨号期说明。

### 8.2 M4b 延后事项

- #15："the peer cannot be reached" 按策略与 peer 限频，每 5 分钟至多一条。
- #24：拨号期写 `Unsupported` 说明时追加在已有说明之后，不覆盖 `smart` 的 "tried …"。

### 8.3 `smart` 计入 UDP

- 开载体失败：这个成员失败（与 TCP 连接失败同样计分）。
- 一条流从载体就绪到第一个回包的时间，当作首字节时间计分。
- 发出后 3 秒没有任何回包：只对目标端口 53 与 443 的流算失败；其它端口不算（游戏之类可能只发不收）。
- UDP 流不触发 `smart` 的换成员重试（UDP 没有"连接失败后换下一个"的时机；下一条流会用新的排序）。

### 8.4 UDP 测试

`test-udp`（策略）/ `proxy-test-udp`（全局，前者优先）：经该策略的 UDP 载体，向 `hostname@ipv4` 的 53 端口发一次 A 查询，得到回答即成功，限时 `test-timeout`。结果只用于 `POST /v1/policies/test`（响应里多一个 UDP 结果）与测试结果的展示；不改变自动组的选择（手册：延迟测试只测 TCP）。

### 8.5 `dns-follow-interface`

策略写了 `interface` 且 `dns-follow-interface=true` 时，解析这条策略的服务器名：查询从该网卡发出，经 `rurge-dns` 为该网卡单独建的一组上游连接，结果不写进全局缓存（该组连接自己的缓存按网卡分区）。规则匹配阶段的解析不受影响（手册）。没写 `interface` 时这个参数无效果（`W0028`）。

## 9. 错误处理、可观测性与安全

- 固定说法：`policy does not support UDP`、`QUIC blocked`、`the underlying policy cannot carry UDP`、`udp: too many flows on this association`、`udp: too many associations`；协议各自的错误沿用其 TCP 的文本前缀（`socks5: …`、`trojan: …`）。
- 日志与错误文本不带 UDP 载荷；DNS 流的查询名不进日志（请求记录照常记目标）。
- UDP 关联只接受控制连接那个客户端 IP 的包；上限防止洪泛拖垮进程（第 5.4 节）。
- 锁：载体与流表的锁里只做内存操作，不跨 `.await`、不做 I/O。
- 测试只用回环、不碰公网，不改本机的网络与代理设置。

## 10. 测试策略

三层沿用总设计 D3。

| 部分 | 第 1 层（向量 / 单元） | 第 2 层（回环） | 第 3 层（互操作） |
| ---- | ---------------------- | --------------- | ----------------- |
| 入站与流水线 | SOCKS5 UDP 头的解析与封装、QUIC Initial 的识别、流表与回收（暂停的时钟） | 经引擎的端到端：客户端经 rurge 的 SOCKS5 UDP 发往回环 UDP 回显；全锥（第二个回环端口主动发包，客户端收到）；控制连接一断关联即关；上限；`block-quic` 四种全局值 × 三种策略值；`udp-policy-not-supported-behaviour` 两种；REJECT；请求记录的 `transport` | — |
| socks5 / external | SOCKS5 UDP 客户端的包格式 | `FakeSocks5` 加 UDP ASSOCIATE；`tests/external` 的辅助程序加 UDP ASSOCIATE；链式 UDP | sing-box 的 socks 入站 |
| trojan / vmess / anytls | 各自的包格式向量 | `FakeTrojan` / `FakeVmess` / `FakeAnyTls` 支持 UDP | sing-box（三种）、xray（vmess） |
| wireguard | — | `FakeWgPeer` 的 UDP 回显端口；经 `underlying-proxy` 的 UDP 载体 | sing-box WireGuard 端点的 UDP |
| M5c 其余 | — | UDP 测试、`smart` 的 UDP 回报、`dns-follow-interface`（回环上的两个假 DNS） | — |

## 11. 验收标准

1. 经 rurge 的 SOCKS5 UDP，DIRECT 与 `socks5` / `external` / `trojan` / `vmess` / `anytls` / `wireguard` 都能对回环假服务端往返；全锥用例通过（vmess 除外）；对 sing-box / xray 的互操作在 CI 上通过。
2. `block-quic` 按第 6 节阻断 QUIC；手工验收：Chrome 经 SOCKS5 访问支持 HTTP/3 的网站时回落到 TCP。
3. 不支持 UDP 的策略按 `udp-policy-not-supported-behaviour` 处理。
4. `W0029` 不再因 `udp-relay`、`block-quic`、`test-udp`、`dns-follow-interface` 出现（`ecn` 除外）。
5. 门禁全绿（fmt / clippy 零警告 / `cargo test --workspace`）。
6. 需要真实环境的项目进 `docs/acceptance/phase2-manual.md` 的 M5 三节：真实节点上的游戏或语音、`nslookup` / `dig` 经 SOCKS5、Chrome 的 QUIC、WARP 上的 UDP。

## 12. 兼容性清单需登记的差异

- UDP 只经 SOCKS5 UDP ASSOCIATE（阶段 2 没有 TUN）。
- REJECT 系列对 UDP 一律丢包，没有 ICMP。
- UDP 流 60 秒空闲回收、DNS 流收到回答后 10 秒回收（rurge 自定）。
- QUIC 只在 UDP 443 上识别；全局 `block-quic` 四个值的含义未与 Surge 核对。
- `socks5-tls` 的 UDP 是明文 UDP。
- `vmess` 的 UDP 是对称型（没有 XUDP）。
- 每个关联 1024 条流、全进程 4096 个关联的上限。
- UDP 测试不影响自动组的选择。

## 13. 对其它文档的订正

| 文档 | 订正 |
| ---- | ---- |
| 总设计 5.1 | `Outbound` 的 UDP 方法是 `udp()` 与 `open_udp()`（返回按包收发的 `PacketSocket`），不是 `connect_udp(target)` 返回 `Datagram`；`UdpSupport` 没有 `OverTcp`（UDP over TCP 是协议内部的事，对引擎仍是 `Native`） |
| 总设计 8.2 | "每条流调用一次 `Outbound::connect_udp(target)`"改为按"关联 × 出站"共用载体（全锥，M5-D2） |
| 兼容性清单 `block-quic`（策略级） | "M7 生效"改为 M5 |
| 兼容性清单 `ss` / `snell` / `h2-connect` 的 UDP | 随 M6 |

## 14. 风险

| 风险 | 影响 | 应对 |
| ---- | ---- | ---- |
| Windows 的 UDP 系统调用开销大（M4b 基准已见） | 大流量 UDP（视频通话）的吞吐 | 批量收；先测回环基准，数字记进 M5a 计划 |
| 全锥载体共用后，一个慢的回包消费者拖住整个关联 | 延迟 | 每个载体一个收包任务，发给客户端不等待（客户端 socket 满时丢包） |
| VMess 只有对称型 | P2P 场景经 vmess 不通 | 登记为差异；XUDP 延后 |
| 全局 `block-quic` 含义未与 Surge 核对 | 行为可能与 Surge 不同 | 登记；手工验收对照 |
| 互操作本机验证不了 | 与真实实现的兼容 | CI 必跑；手工验收兜底 |

## 15. 写计划时必须核对的事项

| 编号 | 事项 | 属于 |
| ---- | ---- | ---- |
| V1 | RFC 1928 UDP ASSOCIATE 的请求 / 应答与 UDP 头；`rurge-inbound` 的 SOCKS5 状态机怎样接入 UDP 端口与控制连接的寿命；`restrict` 的访问限制怎样作用于 UDP | M5a |
| V2 | `Dialer` 一侧的 UDP 接口形状（入站→引擎）；请求记录与 `SessionHandle` 对 UDP 流的复用程度（字节计数、结束钩子、`first_byte`） | M5a |
| V3 | `DirectConnector` 的 socket 选项在未连接 UDP socket 上的用法（`ip-version` 选地址族、双栈 socket 与 IPv4 映射地址） | M5a |
| V4 | sing-box `socks` 入站对 UDP ASSOCIATE 的实现细节（中继地址、`0.0.0.0` 应答）；常见服务端的行为 | M5a |
| V5 | QUIC v1 / v2 与 draft 版本的 Initial 包类型位（RFC 9000 / 9369） | M5a |
| V6 | trojan UDP 的包格式（trojan-gfw 文档、sing-box 实现）；anytls 的 UoT v2 请求头与非连接模式包格式（sing-box `uot`）；vmess 命令 2 的 UDP 分块（长度前缀、AEAD）与 xray / sing-box 的兼容 | M5b |
| V7 | smoltcp 0.12 UDP socket 的缓冲、端口分配与现有 DNS 用法的共存；`Device` 怎样把回包交给多个载体 | M5c |
| V8 | `rurge-dns` 的上游是否能按网卡另建一组（`SocketHook` 已能绑网卡）；缓存分区的做法 | M5c |
| V9 | `smart` 的回报接口（M3c 的 `SessionHandle` 钩子）对 UDP 流的适配 | M5c |
| V10 | `POST /v1/policies/test` 的响应形状加 UDP 结果是否破坏现有客户端（`docs/api/phase2.md`） | M5c |

## 16. 任务草图

**M5a 地基**

1. `rurge-net`：`PacketSocket`、`DirectConnector::open_udp`、QUIC Initial 识别；`rurge-config`：`BlockQuicGlobal` 补值、`SessionInfo::udp`。
2. `rurge-proto`：`Outbound::udp` / `open_udp`、DIRECT 的 UDP、SOCKS5 UDP 客户端；`FakeSocks5` 的 UDP ASSOCIATE。
3. `rurge-inbound`：SOCKS5 UDP ASSOCIATE（关联、端口、地址校验、包的封装）与 UDP 的 `Dialer` 接口。
4. `rurge-engine`：UDP 流水线（流、载体、全锥、回收、上限、请求记录 `transport`）与经引擎的端到端。
5. `block-quic` 与 `udp-policy-not-supported-behaviour`、REJECT 对 UDP。
6. `socks5` / `socks5-tls` / `external` 的 `udp-relay`；`ChainConnector::connect_udp`；`tests/external` 辅助程序的 UDP。
7. 互操作（sing-box socks UDP）与文档（兼容性清单、README、`CLAUDE.md`、API 文档、手工验收）。

**M5b TLS 族**

1. trojan UDP；2. anytls UoT v2；3. vmess 命令 2；4. 回环假服务端与经引擎的用例；5. 互操作与文档。

**M5c WireGuard 与其余**

1. wireguard UDP；2. 经 `underlying-proxy`、M4b #15 / #24；3. UDP 测试；4. `smart` 计入 UDP；5. `dns-follow-interface`；6. 互操作与文档。

## 17. M5a 计划期的订正

写 M5a 实施计划（`docs/superpowers/plans/2026-09-29-phase2-m5a-udp-foundation-plan.md`）时核对源码、手册与 RFC 后，与上文不一致处以本节为准（括号里是计划「计划期决定」的编号）。

1. **4.1 `PacketSocket` 的形状（P3）**：不是轮询式，而是异步方法 `send_to` / `recv_from`（返回 `BoxFuture`，`recv_from` 给出长度与来源 `Target`）加一个 `resolve(to)`（发往 `to` 的包实际去的地址：DIRECT 在此解析名字，其余协议原样交回）。每个载体有自己的收包任务，不需要一个任务同时轮询几个载体。`Connector` 也多一个 `open_udp`（默认"不支持"）。
2. **4.3 链式 UDP（P15）**：M5a 只做 `ChainConnector::open_udp`（底层策略的包载体原样交出；底层不支持时报 `via <底层>: the underlying policy cannot carry UDP`）。包成固定对端 `Datagram` 的 `ChainConnector::connect_udp` 留给 M5c 的 `wireguard`。
3. **5.3 全锥的标注（P9）**：回包计入"这个载体上最早的、还在的一条流"，但不在记录上标注"收到其它来源的包"——请求记录能写说明的只有 `error`，写在那里会被读成失败。
4. **6.1 QUIC 识别（P6）**：另加两个条件——首包至少 1200 字节（RFC 9000 §14.1），v2（`0x6b3343cf`）的 Initial 类型是 1（RFC 9369）。识别放在 `rurge-engine` 的 `sniff` 模块而不是 `rurge-net`（只有引擎用它）。
5. **6.2 全局 `block-quic`（P8）**：`BlockQuicGlobal` 在阶段 1 就有全部四个值，配置层不用"补上 `all` 与 `always-allow`"。第 16 节 M5a 草图第 1 项的"`BlockQuicGlobal` 补值"随之取消。
6. **`PROTOCOL` 规则（P7）**：`PROTOCOL,UDP` 匹配每条 UDP 流（含 QUIC），`PROTOCOL,TCP` 匹配每个 TCP 会话（此前从不命中）。
7. **任务切分（P16）**：SOCKS5 的 UDP 客户端与 `socks5` / `socks5-tls` 的 `udp-relay` 都在 Task 2；Task 6 是 `external` 的 `udp-relay`、链式 UDP 与测试辅助程序；QUIC 识别在 Task 5。
