# 阶段 2「出站协议与策略组」设计文档

| 项       | 内容                                                                                                   |
| -------- | ------------------------------------------------------------------------------------------------------ |
| 日期     | 2026-09-19                                                                                             |
| 状态     | 草案，待评审                                                                                           |
| 对应需求 | [需求文档](../../requirements.md) 第 7 节「阶段 2」；FR 编号见各节                                      |
| 兼容基线 | [Surge 兼容性清单](../../surge-compatibility-matrix.md) 第 4、5 节与 6.2、10.4 节，Surge Mac 6.9 / iOS 5.22 手册 |
| 前置     | 阶段 1（M1 ～ M4b）已合并：配置解析、规则引擎、DNS、连接流水线、控制面、系统代理与服务安装             |

> 2026-09-19 订正：M1 细化设计（`2026-09-19-phase2-m1-outbound-foundation-design.md` 第 11 节 C1–C8）对本文档做了八处订正，下文已按订正后的内容改写。

本文档是阶段 2 的总设计：定目标与范围、技术选型、crate 边界、跨里程碑共用的抽象、里程碑拆分、测试策略与验收。每个里程碑开工前再各写一份细化设计与实施计划（同阶段 1 的 M2 / M3 / M4）；各协议的线上格式、`smart` 的评分公式、API 的 JSON 形状等细节在那里定稿，不在本文档展开。

## 1. 目标与范围

### 1.1 阶段目标

全部出站协议与策略组可用，订阅可加载。落到使用上：拿一份真实的 Surge 配置（带订阅、自动组、链式代理），`rurge run` 之后经 HTTP / SOCKS5 入站的流量按规则走对应的代理，策略组按手册描述的算法选择成员，并能通过 HTTP API 测试与切换。

### 1.2 范围内（引用需求文档编号）

| 领域 | 需求 |
| ---- | ---- |
| 配置 | FR-CFG-11（`[Keystore]`） |
| 入站 | FR-IN-02 的 UDP ASSOCIATE 部分 |
| 出站 | FR-OUT-03 ～ 13、15（FR-OUT-11 只含非 TUN 部分） |
| 策略组 | FR-GRP-01、03 ～ 07 |
| DNS | FR-DNS-04 的 `h3://` 与 `quic://`、FR-DNS-07、FR-DNS-10 |
| 控制面 | 兼容性清单 10.4 中标注阶段 2 的六个端点（策略详情、策略测试、组列表、组测试结果、组选择、组测试） |

### 1.3 范围外

| 项 | 去向 |
| -- | ---- |
| `subnet` 组（FR-GRP-02）、网络变化探测 | 阶段 3。本阶段只留一个"网络已变化"的内部入口，供测试结果失效与 WireGuard 重建调用，没有探测器触发它 |
| TUN 下的 `block-quic`、`external` 的 `addresses` 路由排除 | 阶段 3 |
| 策略 / 组变更通知（FR-OUT-14、FR-GRP-08）与 `no-error-alert` `no-alert` `icon-url` | 阶段 6；参数照常解析保留 |
| `tailscale` | 远期；只在 M8 出可行性报告 |
| 策略相关的 CLI 命令 | 阶段 6（清单 10.3）；本阶段经 HTTP API 操作 |

### 1.4 里程碑拆分

阶段 2 拆成八个可独立验收的里程碑。顺序依据两点：项目所有者日常使用的协议优先（TLS 族、WireGuard / SSH / HTTP·SOCKS5 上游）；**所有协议先做 TCP，UDP 路径集中到 M5**——阶段 2 没有 TUN，UDP 只能从 SOCKS5 UDP ASSOCIATE 进来，日常几乎没有应用使用，不应挡在常用协议前面。

| 里程碑 | 内容 | 需求 | 可验收的产出 |
| ------ | ---- | ---- | ------------ |
| **M1 出站地基与 HTTP / SOCKS5 上游** | `PolicySpec` 类型化（通用参数、TLS 参数）、`[Keystore]`、`Outbound` 与演进后的 `Connector` 抽象、`DirectConnector`（`interface` `allow-other-interface` `ip-version` `tfo` `tos`）、`ChainConnector`（`underlying-proxy`）、TLS 层、`http` `https` `socks5` `socks5-tls`（TCP，含明文 HTTP 的绝对 URI 转发）、`OutboundFactory` 与注册表接入真实出站、`select` 组语义补全、`rurge check` 干构建、策略 / 组只读与 select 的 API、三层测试的基础设施 | FR-CFG-11（p12）、FR-OUT-03（部分）/ 04（TLS）/ 05 / 08 / 09、FR-GRP-05（部分）/ 06、FR-DNS-07 | 真实配置经 HTTP(S) / SOCKS5 上游与两级链式代理转发；对 sing-box 的互操作测试通过 |
| **M2 TLS 族** | WebSocket 层、Shadow TLS v2 / v3、`trojan`、`vmess`（AEAD）、`anytls`；细化设计（`2026-09-20-phase2-m2-tls-family-design.md`）把它拆成三份计划：M2a（Trojan 优先，含 WebSocket 层）→ M2b（VMess / AnyTLS，含按指纹复用出站）→ M2c（Shadow TLS） | FR-OUT-04（Shadow TLS）/ 05 | 三种协议各自带 / 不带 WebSocket、Shadow TLS 对参考实现转发通过 |
| **M3 策略组、订阅与连通性测试** | 连通性测试、`url-test` `fallback` `load-balance` `smart`、全部组参数、`policy-path` / `include-*` 装配、嵌套 / 环 / 兜底、临时覆盖、组级 `underlying-proxy`、订阅更新热重建、测试与切换 API | FR-OUT-03（`test-url` `test-timeout`）/ 10、FR-GRP-01 / 03 ～ 07 | 订阅样本解析正确；组算法单测与端到端测试；API 测试与切换正确。M3 细化设计 `2026-09-23-phase2-m3-groups-subscriptions-design.md`把它拆成三份计划：M3a（成员装配与订阅，订阅优先）→ M3b（测速与 `url-test` / `fallback` / `load-balance`）→ M3c（`smart`） |
| **M4 WireGuard / SSH / external** | `rurge-proto-wireguard`（多 peer 路由、定时器、分片重组、`client-id`、RTT 探测、ICMP echo、DSCP）、`DirectConnector` 的 UDP 载体、`rurge-proto-ssh`、`external` 进程监管、`[WireGuard <name>]` 类型化、Keystore 的 OpenSSH 私钥 | FR-CFG-11（openssh）、FR-OUT-05 / 12 / 13 | WireGuard 与带 `client-id` 的端点握手并转发 TCP；SSH 动态转发；`external` 进程退出自动重启 |
| **M5 UDP 路径** | SOCKS5 UDP ASSOCIATE 入站、引擎 UDP 流水线、DIRECT UDP、已有协议的 UDP（`socks5` `trojan` `vmess` `anytls` `wireguard` `external`）、`udp-relay` `udp-port` `udp-policy-not-supported-behaviour`、UDP 测试（`test-udp` / `proxy-test-udp`）、`block-quic`、UDP 载体的链式拨号、`dns-follow-interface` | FR-IN-02（UDP）、FR-OUT-03（`test-udp` `block-quic`）/ 07 / 08（UDP）/ 11、FR-DNS-10 | 各协议 UDP 对参考服务器转发通过；不支持 UDP 的策略按全局设置处理 |
| **M6 Shadowsocks / Snell / HTTP/2 族** | `ss`（AEAD、2022、obfs，含 UDP）、`snell` v1 ～ v4（obfs、reuse，v3+ UDP）、`h2-connect`（`max-streams`、CONNECT-UDP）、`trust-tunnel`（h2） | FR-OUT-05 / 07 | 四种协议对参考实现转发通过 |
| **M7 QUIC 族** | `rurge-net::quic` 公共件、`rurge-proto-quic`（`tuic` `tuic-v5` `hysteria2` `masque`、`trust-tunnel` 的 h3 模式）、`port-hopping`、`ecn`、DoH3 / DoQ 上游 | FR-OUT-03（`ecn`）/ 05、FR-DNS-04 | QUIC 族 TCP 与 UDP 转发通过；`h3://` `quic://` 上游解析正常 |
| **M8 收尾与验收** | P2 项（Shadowsocks 流式旧方法、VMess 旧握手）、Snell v5 / v6 · Gecko · Tailscale 可行性报告、阶段验收清单、文档同步 | FR-OUT-06 / 15 | 第 14 节验收标准全部通过 |

M1 体量大，细化设计时可按阶段 1 的先例拆成 M1a（配置与抽象）/ M1b（协议、注册表、API）。

## 2. 技术选型

**依赖策略（已决，D2）**：重活复用成熟库，代理协议本身的封帧自研。rurge 必须自己掌控拨号链（`underlying-proxy`、网卡绑定、`ip-version`）与 Surge 语义，嵌入式的协议客户端库不暴露这些扩展点；而 TLS、QUIC、SSH、Noise 这类密码学与状态机密集的部分自研的风险与审计成本不可接受。所有新增依赖须在 NFR-11 的许可证白名单内，并经 trait 与第三方库隔离（PRD R8）。

| 领域 | 选择 | 里程碑 | 细化设计时必须验证的点 |
| ---- | ---- | ------ | ---------------------- |
| TLS | 沿用 rustls 0.23（ring）+ tokio-rustls；自定义 `ServerCertVerifier` 实现 `skip-cert-verify` / `server-cert-verify-name` / `server-cert-fingerprint-sha256`；`sni = off` 关闭 SNI 扩展 | M1 | — |
| PKCS#12 | 纯 Rust 解析库（候选 `p12-keystore`） | M1 | 旧式 3DES-SHA1 与 PBES2-AES 两类 p12 都能解；许可证 |
| socket 选项 | `socket2`：Linux `SO_BINDTODEVICE`、macOS `IP_BOUND_IF`、TOS、TFO；Windows 的网卡绑定先用"绑定该网卡的源地址"（`if-addrs` 已在依赖里） | M1 | Windows 的 `TCP_FASTOPEN` 可用性；`IP_UNICAST_IF` 需要 unsafe FFI（见 Q1） |
| WebSocket | `tokio-tungstenite`，在任意 `BoxedStream` 上做客户端握手 | M2 | — |
| Shadow TLS | v2 与 v3 都在 stock rustls 上实现：v2 旁路哈希服务端握手字节；v3 用"两遍构造 ClientHello"签名 SessionID（只用公开的 `CryptoProvider` 扩展点，不 fork、不用 unsafe；M2 细化设计附录 A） | M2 | 已验证（2026-09-20 的 spike，见风险 A） |
| 加密原语 | RustCrypto：`aes-gcm` `chacha20poly1305` `hkdf` `hmac` `sha1` `sha2` `md-5` `blake3` | M2 / M6 | — |
| HTTP/2 | `h2`（extended CONNECT、多路复用、`max-streams`）；CONNECT-UDP（RFC 9298）的 capsule 编码自研 | M6 | — |
| QUIC / HTTP/3 | `quinn`（自定义 UDP 载体供链式拨号、自定义拥塞控制供 Hysteria 2、datagram 供 TUIC / MASQUE）+ `h3` | M7 | 风险 B：`h3` 的 extended CONNECT 与 HTTP Datagram 的成熟度；`ecn` 的支持程度 |
| SSH | `russh` 客户端，动态转发用 direct-tcpip 通道；OpenSSH 私钥解析用 `ssh-key` | M4 | 是否覆盖 `curve25519-sha256` + `aes128-gcm`（清单 4.6 的下限） |
| WireGuard | `boringtun` 的 sans-IO `Tunn`（Noise 握手与定时器）；`client-id` 在包头保留字节上处理；多 peer 的最长前缀路由复用现有 `prefix-trie` | M4 | 风险 C：boringtun 的维护节奏 |
| 用户态协议栈 | `smoltcp`，作为 `rurge-proto-wireguard` 的内部模块（见 D9） | M4 | TCP 吞吐基准 |
| Snell | 协议非公开（PRD R1 / R7）：只依据公开的第三方资料实现 v1 ～ v4 | M6 | 各版本可得的公开资料范围 |
| 测试证书 | `rcgen`（已是 dev 依赖）现场生成自签链 | M1 | — |

## 3. Workspace 与 crate 边界

**crate 布局（已决，D4）**：按传输族拆分，把 quinn / h3、russh、boringtun / smoltcp 三棵重依赖树隔开；后续里程碑各自主要只改自己的 crate。

```
crates/
├── rurge-proto/             # 核心 trait（Outbound / Datagram）、DIRECT / REJECT、
│                            # 传输层（tls、shadow-tls、ws、obfs、h2 连接池）、
│                            # http(s) socks5(-tls) trojan vmess anytls ss snell
│                            # h2-connect trust-tunnel(h2) external
├── rurge-proto-quic/        # ★ M7：quinn + h3；tuic tuic-v5 hysteria2 masque trust-tunnel(h3)
├── rurge-proto-ssh/         # ★ M4：russh；ssh
├── rurge-proto-wireguard/   # ★ M4：boringtun + smoltcp；wireguard
├── rurge-net/               # + quic 公共件（M7；rurge-dns 的 DoH3 / DoQ 与 rurge-proto-quic 共用）
├── rurge-policy/            # 类型化注册表、组运行时、连通性测试、订阅装配；定义 OutboundFactory
└── rurge-engine/            # 实现 OutboundFactory，装配四个协议 crate；UDP 流水线
```

依赖方向（只能向下；★ 为新增）：

```
rurge (bin) → rurge-api → rurge-engine → { rurge-inbound, rurge-policy, rurge-rules, rurge-dns,
                                           rurge-proto, ★rurge-proto-quic, ★rurge-proto-ssh, ★rurge-proto-wireguard }
★rurge-proto-{quic,ssh,wireguard} → rurge-proto → rurge-net → rurge-config
rurge-policy → rurge-proto（只用 trait）、rurge-net（资源管理器、HTTP 客户端）、rurge-config
rurge-dns    → rurge-net（含 quic 公共件）
```

- 协议 crate 与 `rurge-policy` 都不依赖 `rurge-platform`：网卡绑定等平台相关代码经 trait 注入（AR-02，5.2）。
- `rurge-policy` 不依赖任何协议实现 crate：出站由注入的 `OutboundFactory` 构造（5.6），注册表与组算法的单元测试用假工厂与假出站。
- 只创建当前里程碑需要的 crate，不预建空壳（沿用阶段 1 的约定）。

## 4. 配置层（`rurge-config`）

### 4.1 `PolicySpec`

M1（阶段 1）把策略解析成 `ProxyPolicy { kind, server, port, positional, params: ParamMap }`，参数是无类型的表。阶段 2 在其上增加类型化的一层，仍放在 `rurge-config`：策略参数属于配置语义，订阅导入的策略行在运行期也要走同一套解析与校验。

```rust
pub struct PolicySpec {
    pub name: String,
    pub common: CommonOpts,        // FR-OUT-03 的 14 个通用参数
    pub transport: TransportOpts,  // 该协议适用的 tls / shadow-tls / ws / obfs
    pub proto: ProtoSpec,
    pub span: Span,
}
pub enum ProtoSpec {
    Http(..), Socks5(..), Trojan(..), Vmess(..), AnyTls(..), Shadowsocks(..), Snell(..), H2Connect(..),
    TrustTunnel(..), Tuic(..), Hysteria2(..), Masque(..), Ssh(..), WireGuard(..), External(..),
}
```

`ProxyPolicy` 保持不变（API 的策略详情、`redact_profile`、快照仍基于它）；`PolicySpec` 由它派生。各协议的 spec 随各自里程碑加入：M1 落 `CommonOpts`、`TlsOpts` 与 http / socks5；没轮到的协议继续产生 `W0007`，运行期按 REJECT 处理（阶段 1 的约定），`W0007` 在 M7 结束时消失。`capabilities::current()` 随里程碑增长，`#!REQUIREMENT` 的求值随之正确（PRD Q3）。

### 4.2 诊断规则

沿用兼容性原则（"Surge 语法就是 rurge 语法"）：

| 情况 | 处理 |
| ---- | ---- |
| 不认识的参数；对该协议不适用的参数 | WARN 并忽略 |
| iOS 专属参数（`hybrid`） | 解析并忽略（清单 4.3 的 🔁） |
| 认识的参数取值非法；缺必填项（`password` `psk` `section-name`…） | 带文件与行号的错误 |
| 违反清单登记的约束：Shadow TLS × QUIC 类 / WireGuard，`underlying-proxy` × `port-hopping`，`underlying-proxy` 成环 | 错误 |
| 订阅（`policy-path`）里的坏行 | 跳过该行并 WARN（计数进日志与 API），不影响其余成员 |
| Shadowsocks 流式旧方法 | WARN"不推荐"（M8 实现前同时是 `W0007`） |

### 4.3 `[Keystore]`（FR-CFG-11）

阶段 1 已解析条目（`p12` / `openssh-private-key`、类型推断、密码）。阶段 2：

- 配置层校验引用：`client-cert` 必须指向 `p12` 条目，`ssh` 的 `private-key` 必须指向 `openssh-private-key` 条目；Base64 必须合法。`ca-keystore-name` 属于阶段 4。
- 材料解码（PKCS#12 → 证书链与私钥；OpenSSH 私钥）放在工厂构建期（5.6），错误经干构建回到 `rurge check`。
- 材料不进日志；`redact_profile` 覆盖 `[Keystore]` 的 `base64` 与 `password`（NFR-03）。

### 4.4 `[WireGuard <name>]`

M4 把它从 `deferred` 转为类型化节：`private-key`（Base64 或 64 位十六进制）、`self-ip` / `self-ip-v6`（至少一个）、`dns-server`、`prefer-ipv6`、`mtu`（576 ～ 1420，默认 1280）、多个 `peer`（`public-key` `allowed-ips` `endpoint` `preshared-key` `keepalive` `client-id` 三种写法）。`wireguard` 策略行的 `section-name` 指向不存在的节是错误。`[Tailscale <name>]` 继续留在 `deferred`。

### 4.5 本阶段生效的 `[General]` 键

`proxy-test-url`、`test-timeout`、`proxy-test-udp`、`udp-policy-not-supported-behaviour`、`block-quic`、`use-local-host-item-for-proxy`。它们在阶段 1 已解析为强类型，本阶段接上行为；逐键的状态随里程碑在兼容性清单里更新。

## 5. 出站抽象（`rurge-proto`）

### 5.1 三个核心抽象

"出站怎样够到自己的服务器"这一抽象不新增 trait，而是**演进现有的 `rurge_net::Connector`**：它的签名本来就吻合，DoH / DoT / 内部 HTTP 客户端 / 资源下载已经都接受 `Arc<dyn Connector>`，而工作区里已有一个 `rurge_inbound::Dialer`（入站→引擎边界），再造同名 trait 只会混淆。

```rust
/// 一条到固定目标的 UDP 流。引擎面向它转发；QUIC / WireGuard 也用它当载体。
pub trait Datagram: Send + Sync {
    fn send<'a>(&'a self, payload: &'a [u8]) -> BoxFuture<'a, io::Result<()>>;
    fn recv<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<usize>>;
}

/// rurge-net 里现有的 trait：出站怎样够到"自己的服务器"（对 DIRECT 而言就是目标本身）。
pub trait Connector: Send + Sync {
    fn connect<'a>(&'a self, to: &'a Target, opts: &'a ConnectOpts) -> BoxFuture<'a, io::Result<BoxedStream>>;
    // M4 / M5 起增加（带默认实现，返回 Unsupported）：
    fn connect_udp<'a>(&'a self, to: &'a Target, opts: &'a ConnectOpts) -> BoxFuture<'a, io::Result<BoxedDatagram>>;
}

pub trait Outbound: Send + Sync {
    fn name(&self) -> &str;
    fn udp(&self) -> UdpSupport; // Native | OverTcp | Unsupported
    fn connect_tcp<'a>(&'a self, target: &'a Target, opts: &'a ConnectOpts)
        -> BoxFuture<'a, Result<BoxedStream, OutboundError>>;
    fn connect_udp<'a>(&'a self, target: &'a Target, opts: &'a ConnectOpts)
        -> BoxFuture<'a, Result<BoxedDatagram, OutboundError>>;
}
```

`ConnectOpts` 只保留超时；地址族偏好改由策略的 `ip-version` 决定，不再由调用方传入。`Outbound` 的 UDP 方法（`udp` / `connect_udp`）在 M5 以带默认实现的新方法加入；M1 另加一个 `http_forward()`，供 HTTP 代理按绝对 URI 转发明文请求（M1 细化设计 5.2）。

UDP 一侧分三步落地：`DirectConnector::connect_udp`（带 socket 选项的裸 UDP socket）在 M4 随 WireGuard 的载体需求落地；`Outbound::connect_udp`（引擎面向的 UDP 流）与 `ChainConnector::connect_udp`（经底层代理的 UDP 载体）在 M5 落地。

### 5.2 `DirectConnector` 与 socket 选项（FR-OUT-03 / 09）

真实 socket 加策略的 socket 选项：`interface` 与 `allow-other-interface`（指定网卡不可用时是否允许其它网卡）、`ip-version`（五种取值；`prefer-*` 模式先试偏好的地址族，3 秒后再试另一族；有 `underlying-proxy` 时无效）、`tfo`、`tos`。代理服务器主机名经 `rurge-dns` 解析；`dns-follow-interface = true` 时，这次解析的查询也从该策略的网卡发出（FR-DNS-10，M5：DNS 的 UDP 上游是每个上游一个共享的已连接 socket，按策略换网卡需要另一组 socket 与按网卡分区的缓存）。连接由顺序尝试改为竞速：`dual` 按地址族交错、每 250 ms 发起下一个尝试、先成功者胜。

网卡绑定、TFO 与 TOS 是平台相关的，经注入的 trait 完成（定义在 `rurge_net::socket`，`rurge-dns` 以后也用它）：

```rust
pub trait SocketHook: Send + Sync {
    fn bind_interface(&self, socket: &socket2::Socket, interface: &str, family: Family) -> io::Result<()>;
    fn enable_tfo(&self, socket: &socket2::Socket) -> io::Result<bool>;
    fn set_tos(&self, socket: &socket2::Socket, family: Family, tos: u8) -> io::Result<()>;
}
```

平台函数放在 `rurge-platform::socket`（Linux `SO_BINDTODEVICE`、macOS `IP_BOUND_IF`、Windows 绑定该网卡的源地址）；`rurge-platform` 不依赖内部 crate，所以实现该 trait 的适配器放在 bin 并由它注入；测试用记录调用的假实现。WireGuard 不支持 `interface`（与 Surge 一致）。

`Datagram` 是"到固定目标"的流，而 QUIC 类协议的 `port-hopping` 要更换对端端口：直连时由 `DirectConnector` 另行提供带同样 socket 选项的未连接 UDP socket（M7）；链式载体换不了端口，这正是 `underlying-proxy` 与 `port-hopping` 互斥的原因。

### 5.3 `ChainConnector`（`underlying-proxy`，FR-OUT-08）

持有"底层策略的名字 + 跨代稳定的注册表单元（`RegistryCell`）"，**每次拨号时**从当前一代注册表解析：底层是策略组时跟随该组的当前选择。它把本策略的服务器主机名原样交给底层出站（远程解析）。环在构建期按静态引用图检测（底层是组时，组到每个成员都算一条边）并作为配置错误报告；订阅可能在运行期引入新的环，M3 起拨号时另有深度上限兜底（超限的连接以 `Unavailable` 失败）。`underlying-proxy` 只对代理策略有效，与 `port-hopping` 互斥。M1 只有 TCP 链；UDP 载体的链式拨号（QUIC 类、WireGuard 经底层代理）在 M5 随 `connect_udp` 落地。

### 5.4 传输层

传输层是函数而不是对象：`transport::{tls, shadow_tls, ws, obfs}` 的形状都是 `async fn(BoxedStream, &Opts) -> io::Result<BoxedStream>`。每个协议按自己固定的顺序叠层：

```
connector.connect(server) → shadow-tls? → tls? → ws? → obfs? → 协议握手
```

TLS 层实现清单 4.4 的六个参数；`alpn` 的默认值由协议给出。多路复用型协议（`h2-connect`、`trust-tunnel`、`anytls`、Snell reuse）在传输层之上维护连接池，池属于出站对象。

### 5.5 出站的生命周期与按指纹复用

带连接池、隧道或子进程的出站（`h2-connect`、`trust-tunnel`、`anytls`、Snell reuse、QUIC 族、SSH、WireGuard、`external`）自己持有后台任务，最后一个 `Arc` 释放时中止；WireGuard 加载时只做准备，按需握手。

从 M2 起（M1 的四种协议没有长生命周期状态），注册表重建（配置重载、订阅更新）时按**指纹**复用上一代的出站：指纹 = 规范化的 `PolicySpec` + 连接器的指纹（socket 选项，或底层策略的名字）+ 被引用的 Keystore 条目 / WireGuard 节的内容。指纹未变就沿用同一个对象——与某条策略无关的重载不打断它的隧道与连接池，也不丢它的测试结果。已有会话持有旧对象的 `Arc`，随会话结束自然释放（AR-04）。

### 5.6 `OutboundFactory` 与干构建

```rust
// 定义在 rurge-policy，由 rurge-engine 实现
pub trait OutboundFactory: Send + Sync {
    fn direct_connector(&self, common: &CommonOpts) -> Arc<dyn Connector>;
    fn build(&self, spec: &PolicySpec, connector: Arc<dyn Connector>) -> Result<OutboundRef, BuildError>;
}
```

`build` 同步、不碰网络（解码 Keystore 材料、校验密钥长度、准备 TLS 配置）。**主配置里的策略构建失败是配置错误**：`rurge check`、`POST /v1/profiles/check` 与 `run` / `reload` 共用同一工厂做一次干构建，把 `BuildError` 作为带行号的错误（`E0022`）处理——`run` 拒绝启动，`reload` 保留旧一代。"策略存在但不可用 → 解析为 REJECT，`error = "policy unavailable: <原因>"`，WARN 一次"这条路只用于 M3 的订阅导入项：一条坏的导入策略不应拖垮整份配置。

### 5.7 错误模型

`OutboundError` 增加 `Proxy(String)`（代理侧握手 / 鉴权失败，文本形如 `socks5: authentication failed`）、`Tls(String)`、`Unavailable(String)`；`Unsupported` 随 `W0007` 一起在 M7 结束时退役。凭据不出现在任何错误文本与日志里（FR-OBS-01）。

## 6. 协议一览

| 协议 | crate | 里程碑 | 叠层 | UDP（清单 4.5） |
| ---- | ----- | ------ | ---- | --------------- |
| `http` / `https` | `rurge-proto` | M1 | TCP（+ TLS） | 不支持 |
| `socks5` / `socks5-tls` | `rurge-proto` | M1（UDP：M5） | TCP（+ TLS） | `udp-relay`，UDP ASSOCIATE |
| `trojan` | `rurge-proto` | M2（UDP：M5） | TLS（+ WS）（+ Shadow TLS） | 自动 |
| `vmess` | `rurge-proto` | M2（UDP：M5；旧握手：M8） | TCP（+ TLS）（+ WS） | 自动 |
| `anytls` | `rurge-proto` | M2（UDP：M5） | TLS，会话复用 | 自动（UDP over TCP） |
| `wireguard` | `rurge-proto-wireguard` | M4（UDP：M5） | UDP 载体上的 L3 隧道 + 用户态协议栈 | 自动 |
| `ssh` | `rurge-proto-ssh` | M4 | SSH 通道 | 不支持 |
| `external` | `rurge-proto` | M4（UDP：M5） | 到子进程本地端口的 SOCKS5 | `udp-relay` |
| `ss` | `rurge-proto` | M6（流式旧方法：M8） | TCP（+ obfs） | `udp-relay`、`udp-port` |
| `snell` v1 ～ v4 | `rurge-proto` | M6 | TCP（+ obfs），v4 reuse | v3+ 自动、`udp-port` |
| `h2-connect` | `rurge-proto` | M6 | TLS + HTTP/2 多路复用 | `udp-relay`，CONNECT-UDP |
| `trust-tunnel` | `rurge-proto`（h2）/ `rurge-proto-quic`（h3） | M6 / M7 | HTTP/2 或 HTTP/3 | 不支持 |
| `tuic` / `tuic-v5` | `rurge-proto-quic` | M7 | QUIC | 自动 |
| `hysteria2` | `rurge-proto-quic` | M7 | QUIC（+ Salamander） | 自动 |
| `masque` | `rurge-proto-quic` | M7 | HTTP/3 CONNECT + CONNECT-UDP | 自动 |

不支持 UDP 的策略收到 UDP 流时按 `udp-policy-not-supported-behaviour` 处理（M5）。各协议的专属参数以清单 4.6 为准，逐协议在里程碑细化设计里展开。

## 7. 策略运行时（`rurge-policy`）

### 7.1 注册表分两层

- **每一代的 `PolicyRegistry`**：不可变，`ArcSwap` 原子切换；配置重载或订阅更新时重建，按 5.5 复用出站。
- **跨代存活的 `GroupState`**：按组名保存当前选择、测试结果、临时覆盖、`smart` 统计；组的定义没变就带到下一代。

`resolve(policy, &SelectCtx)` 带上目标主机名等上下文：`load-balance` 的 `persistent` 按主机名哈希，`smart` 按站点记忆。解析结果仍是"策略链 + 终端出站"，链写入请求记录（AR-05）。

### 7.2 成员装配与订阅（FR-GRP-04）

装配顺序：显式成员 → `include-other-group`（递归展开）→ `include-all-proxies`（`[Proxy]` 的全部代理策略，不含内置与组）→ `policy-path`；重名保留首个。导入项依次经过 `policy-regex-filter` → `external-policy-name-prefix` → `external-policy-modifier`，过滤不作用于显式成员。

`policy-path` 走 `rurge-net` 现有的外部资源管理器（与规则集同一套：数据目录缓存、`update-interval`、后台刷新、本地文件监视）。内容既可以是策略行列表，也可以是含 `[Proxy]` 的完整配置（只取该节）。每一行经 `parse_policy` 与 `PolicySpec` 校验。启动先用缓存，首次下载不阻塞启动（NFR-02）；资源更新后去抖重建一代注册表。（M3 细化设计 `2026-09-23-phase2-m3-groups-subscriptions-design.md`的订正：已有缓存时每一代构建注册表前先同步载入，避免空组窗口；订阅内容的问题只告警、逐条跳过，绝不让加载或重建失败；`policy-path` 的值加入脱敏名单，日志只写组名、不写 URL，订阅行永不进日志。见该文件 M3-D5 ～ M3-D7。）

### 7.3 连通性测试（FR-OUT-10）

- 同一连接上发两次 HEAD，取第二次的耗时（第一次摊掉 TCP / TLS / 代理握手）。
- 测试 URL：策略的 `test-url` → 全局 `proxy-test-url`（直连类用 `internet-test-url`）；超时：策略的 `test-timeout` → 全局 `test-timeout`（默认 5 秒，直连类 10 秒；WireGuard 另加 10 秒 L3 初始化）。
- 实现：`rurge-net` 的内部 HTTP 客户端 + "经该策略出站拨号"的连接器（阶段 1 做成可插拔正是为此）。（M3 细化设计 `2026-09-23-phase2-m3-groups-subscriptions-design.md`订正，M3-D8：不用池化的 `HttpClient`，连接池会让"第一次建连、第二次复用"失去控制；探针经 `Outbound::connect_tcp` 拿流，自己套 TLS，再用 `hyper` 的 HTTP/1 连接层发两次 HEAD。）
- 结果按策略缓存，各组按自己的 `interval` 判断是否过期；**用到且已过期才重测**；`evaluate-before-use` 时首次使用前先测完；并发有上限；"网络已变化"入口使全部结果失效。
- UDP 测试（M5）：经中继向 `hostname@ipv4` 做一次 DNS 查询。
- 测试会话经注入的观察者 trait 写入请求记录并带 `test` 标记（`rurge-policy` 不依赖 `rurge-engine`）。

### 7.4 组算法（FR-GRP-01）

| 类型 | 算法 |
| ---- | ---- |
| `select` | 已存选择（按 Profile 持久化，阶段 1 已有）；无记录或成员消失时取第一个成员 |
| `url-test` | 延迟最低者；`tolerance` 做迟滞（另一成员比当前快超过容差才切换）；`timeout` 是可用性门槛 |
| `fallback` | 按声明顺序第一个可用；全部不可用取第一个 |
| `load-balance` | 可用集合内随机；`persistent` 时按目标主机名哈希；嵌套时分数取均值 |
| `smart` | 按手册描述近似：首响应延迟的时间加权均值 + 失败惩罚，乘 `policy-priority` 因子；接近最优者构成优选集，其余为重试列表；按站点记忆约 1 小时；固定 5 分钟重测（`interval` 无效）；超过 12 个成员只抽样测试；忽略嵌套组与内置策略 |

`smart` 的输入来自真实会话：引擎把每条会话的建连耗时、首字节耗时与失败回报给 `rurge-policy`。手册里的"重传惩罚"需要 TCP 重传统计，那是平台相关的，本阶段用失败率近似；清单保持 🟡 并写明差异（PRD R1）。

### 7.5 嵌套、环与兜底（FR-GRP-05）

组可以嵌套。环只有在运行期才能完全确定（`include-other-group` 与订阅都可能引入），因此阶段 1 的加载期错误 `E0009` 在 M3 降级为告警，成环的组在运行期表现为 REJECT。组内没有可用成员时回退 DIRECT 并 WARN（Surge 的行为）。阶段 1 的占位告警 `W0008`（自动组取第一个成员）在 M3 移除。（M3 细化设计 `2026-09-23-phase2-m3-groups-subscriptions-design.md`订正：`E0009` 退役，加载期的组环改报新告警 `W0030`（M3-D9）；"没有成员"指装配后成员表为空，命令行 `--empty-group-reject` / 环境变量 `RURGE_EMPTY_GROUP_REJECT=1` 把兜底从 DIRECT 改为 REJECT（M3-D3）；`W0008` 在 M3 之后只因 `subnet` 出现，阶段 3 移除。）

### 7.6 组级 `underlying-proxy`（FR-GRP-07）

为全部成员（含导入的）派生名为 `Name (via Relay)` 的策略：spec 相同，连接器换成指向中继的 `ChainConnector`。派生策略出现在 API 的列表里。

### 7.7 选择持久化与临时覆盖（FR-GRP-06）

`select` 的选择按 Profile 写入 `state.json`（阶段 1 已有）。自动类型的组可经 API 临时指定成员，期间停止该组的自动测试；覆盖可经 API 清除。覆盖保存在 `GroupState` 里，不写入 `state.json`：组定义未变的重载会保留它，组定义变了则随 `GroupState` 一起丢弃，进程重启后不保留。（M3 细化设计 `2026-09-23-phase2-m3-groups-subscriptions-design.md`订正，M3-D10：设置与清除都经 `POST /v1/policy_groups/select`——对自动组调用即临时覆盖，`policy` 为空字符串即清除，不新增端点。）

## 8. 引擎集成（`rurge-engine`、`rurge-inbound`）

### 8.1 TCP 拨号的变化

- 走代理时目标域名**不在本地解析**，原样交给代理（远程解析）；IP 类规则为匹配而触发的本地解析不改变这一点。`use-local-host-item-for-proxy = true` 且 `[Host]` 对该域名有本地映射时，把映射到的 IP 交给代理（FR-DNS-07）。
- 拨号流程：`choose_policy` → `registry.resolve(policy, &SelectCtx)` → `outbound.connect_tcp(target, &ConnectOpts)`；失败写入请求记录，M3 起还反馈给 `smart`。
- 请求记录新增（M3，测试与 `smart` 才需要）：终端出站的协议、拨号路径（链式代理时形如 `Exit ← Entry`）、`connect_ms` / `handshake_ms` / `first_byte_ms`、`test` 标记。
- HTTP 入站的明文请求遇到 `always-use-connect = false` 的 `http` / `https` 上游时按绝对 URI 转发（M1）。

### 8.2 UDP 流水线（M5）

- 入站：SOCKS5 UDP ASSOCIATE（`rurge-inbound`）。关联的生命周期跟随其 TCP 控制连接。
- 一条流 = (入站客户端地址, 目标地址)。首包做一次规则匹配与策略解析，对应请求记录里的一条会话；空闲超时回收。
- 每条流调用一次 `Outbound::connect_udp(target)`。多条流怎样共用一个载体（一条 SOCKS5 关联、一条 QUIC 连接、一个 WireGuard 隧道）是协议内部的事，引擎看不见。
- 策略不支持 UDP 时按 `udp-policy-not-supported-behaviour`（`DIRECT` / `REJECT`）处理。

### 8.3 QUIC 阻断（FR-OUT-11，M5）

对目标端口 443 的 UDP 流识别 QUIC 初始包；策略级 `block-quic`（`auto` `on` `off`，`auto` 对代理策略阻断、对 DIRECT 不阻断）与全局 `block-quic` 的四种覆盖联动。被阻断的流按 REJECT 记录，促使客户端回落到 TCP。TUN 下的部分属于阶段 3。

## 9. DNS 增量（`rurge-dns`）

- **DoH3 / DoQ 上游**（FR-DNS-04，M7）：`h3://` 与 `quic://`（RFC 9250）。QUIC 端点的构造、TLS 配置与自定义 UDP 载体放在 `rurge-net::quic`，`rurge-dns` 与 `rurge-proto-quic` 共用。引导豁免、`encrypted-dns-skip-cert-verification`、`encrypted-dns-follow-outbound-mode` 的语义与阶段 1 的 DoH / DoT 一致。
- **`use-local-host-item-for-proxy`**（FR-DNS-07，M1）：见 8.1。
- **`dns-follow-interface`**（FR-DNS-10，M5）：见 5.2。

## 10. 控制面（`rurge-api`）

六个端点经 `Engine` 的方法提供（与现有的 `set_global_policy` 一致；`Control` 只管 reload / stop / 日志级别 / 系统代理这类守护进程级命令），`rurge-api` 仍不认识协议 crate：

| 端点 | 里程碑 |
| ---- | ------ |
| `GET /v1/policies/detail?policy_name=` | M1 |
| `GET /v1/policy_groups` | M1 |
| `GET` / `POST /v1/policy_groups/select` | M1 |
| `POST /v1/policies/test` | M3 |
| `GET /v1/policy_groups/test_results` | M3 |
| `POST /v1/policy_groups/test` | M3 |

Surge 手册没有定义这些端点的响应结构（PRD R5）：以收集到的真实样本为准，形状登记在 `docs/api/phase2.md`，拿不到样本的字段标注"暂定"（沿用阶段 1 的 D7）。策略详情经 `redact_profile` 的同一套规则脱敏。

## 11. 重载与运行时状态

- 触发注册表重建的来源：配置重载、`policy-path` 资源更新；阶段 3 起增加网络变化。
- 重建是"构建新一代 → 原子切换"，失败时保留旧一代（AR-04，与阶段 1 的配置重载一致）。
- 跨代保留：按指纹复用的出站（5.5）、`GroupState`（7.1）。
- `state.json` 不新增字段：临时覆盖与测试结果都不持久化。

## 12. 错误处理与可观测性

| 层 | 策略 |
| -- | ---- |
| 配置 | 4.2 的分级；错误时 `run` 拒绝启动、`reload` 保留旧配置 |
| 构建 | `BuildError` → 策略不可用（REJECT + WARN），`rurge check` 报错（5.6） |
| 拨号 | `OutboundError` 转成请求记录的 `error` 与一条 WARN；不自动换成员重试（`smart` 的重试列表除外） |
| 订阅 | 下载失败沿用资源管理器的退避重试；坏行跳过并计数 |
| 后台任务 | 连接池、隧道、子进程监管各自带退避；panic 由任务边界隔离并记 ERROR |
| 日志 | 结构化字段增加策略链与终端出站；凭据、密钥、Keystore 材料永不落日志 |

## 13. 测试策略

**三层（已决，D3）**：

1. **向量 / 单元**：封帧编解码往返；KDF 对规范向量；从参考实现抓取的字节作为带出处说明的 fixture；对流式解析器做属性测试（任意切片方式喂入，结果一致）。
2. **回环测试服务器**：各协议 crate 的 `testing` 模块提供最小服务端，三平台 `cargo test` 都跑；覆盖鉴权失败、握手截断、超时等失败路径。回环 + 端口 0 + 有界等待。
3. **互操作**：`tests/interop` 夹具按环境变量或 `PATH` 找参考二进制（sing-box、xray、shadowsocks-rust 等），把服务端配置渲染到临时目录，作为回环子进程拉起，`Drop` 时杀掉并等待退出。**本机缺二进制 → 打印说明并跳过；CI 设 `RURGE_INTEROP_REQUIRED=1`，缺了就失败**，避免"全部跳过仍然全绿"。CI 在三平台安装固定版本并校验 SHA256。

单靠第 2 层不够：客户端与自写服务端共用封帧代码，会出现"自洽但与真实实现不兼容"而测不出来的情况，第 1 层的外部向量与第 3 层负责兜住。

| 里程碑 | 第 3 层的参考实现 |
| ------ | ----------------- |
| M1 | sing-box（http / socks 入站，含 TLS 与用户名密码） |
| M2 | sing-box（trojan / vmess / anytls / shadowtls，含 ws 传输）；xray 只用于 vmess（M2 细化设计 M2-D5） |
| M4 | sing-box 的用户态 WireGuard 端点（含保留字节，对应 `client-id`）；系统 `sshd`（没有则跳过） |
| M5 | 同上各服务端的 UDP |
| M6 | shadowsocks-rust（含 2022）、sing-box；Snell 官方二进制只有 Linux，互操作只在 Linux CI 跑 |
| M7 | sing-box（tuic / hysteria2）；MASQUE 与 trust-tunnel 的参考服务端在 M7 细化设计里选定 |

**沿用的安全约束**：测试只用回环地址、不碰公网；不修改本机的系统代理，不注册真实服务；`external` 的测试只拉起测试自带的辅助程序。策略组算法用假出站与可控时钟做单元测试；订阅装配用本地文件与回环 HTTP 服务器。

每个任务结束的门禁不变：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace`。

## 14. 阶段 2 验收标准（对应需求文档第 7 节）

1. 每种协议对参考服务器的 TCP 转发通过；支持 UDP 的协议 UDP 转发通过。
2. 链式代理通过：两级 TCP 链；QUIC 类与 WireGuard 经底层代理的 UDP 载体链。
3. 策略组五种算法的单元测试与端到端测试通过；六个 API 端点的测试与切换行为正确，`select` 的选择重启后保留。
4. 订阅样本（策略行列表与含 `[Proxy]` 的完整配置两种形式）解析正确；过滤、前缀、修饰与装配顺序符合清单 5.2。
5. Shadow TLS v2（以及 v3，取决于风险 A 的结论）与六个 TLS 参数各有测试。
6. WireGuard 与带 `client-id` 的端点握手成功并转发。
7. `h3://` 与 `quic://` 上游解析正常。
8. 三平台 CI 绿（含互操作 job）；clippy 零警告；`W0007` 与 `W0008` 不再出现。
9. 需要真实公网节点的项目进入 `docs/acceptance/phase2-manual.md`，由项目所有者用自己的节点手工验收。

## 15. 风险

| 编号 | 风险 | 影响 | 应对 |
| ---- | ---- | ---- | ---- |
| A | Shadow TLS v3 要求自定 ClientHello 的 SessionID，stock rustls 没有这个钩子 | v3 可能做不了或需要维护补丁 | **已解除（2026-09-20）**：spike 证实可以只用 rustls 的公开扩展点（可替换的随机源与密钥交换组）两遍构造 ClientHello，不需要补丁；残余风险是 rustls 升级改变 ClientHello 的生成方式——实现带运行期自检与单元测试绊线（M2 细化设计附录 A） |
| B | `h3` 的 extended CONNECT 与 HTTP Datagram 仍属实验特性 | MASQUE 与 trust-tunnel 的 h3 模式 | M7 细化设计先验证；不成熟则该两项延后并登记，不阻塞 TUIC / Hysteria 2 |
| C | boringtun 维护节奏慢 | WireGuard 的长期维护 | 经 trait 隔离；备选同源 fork（NepTUN） |
| D | Snell 与 `smart` 的细节非公开（PRD R1） | 行为无法完全一致 | 近似实现，差异登记在清单；Snell v5 / v6 只出可行性报告 |
| E | 范围过大（PRD R6） | 迟迟没有可用版本 | 按使用优先级排里程碑、TCP 优先、P2 放最后；M1 + M2 结束即可用手写的 `[Proxy]` 与 `select` 组日常使用，订阅与自动组随 M3 到位 |
| F | 参考二进制的版本漂移与各平台可得性 | 互操作测试不稳定 | 固定版本 + SHA256；缺失时本机跳过、CI 失败 |
| G | smoltcp 的 TCP 吞吐 | WireGuard 出站性能 | M4 做基准；接口收窄以便替换；阶段 3 复核 |

## 16. 已决事项与开放问题

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| D1 | 里程碑的优先顺序 | 项目所有者日常使用 TLS 族与 WireGuard / SSH / HTTP·SOCKS5 上游，它们排在前面；Shadowsocks / Snell 与 QUIC 族靠后（2026-09-19） |
| D2 | 第三方库的复用程度 | 重活复用、封帧自研（第 2 节） |
| D3 | 集成测试环境 | 三层：向量 + 回环服务器 + 参考二进制回环子进程；不使用 Docker。偏离 PRD 第 8 节的"容器化"措辞，PRD 同步订正 |
| D4 | 协议的 crate 布局 | 按传输族拆 4 个 crate（第 3 节）；PRD 3.2 的模块表同步增加 3 个 crate |
| D5 | TCP 与 UDP 的先后 | 全部协议先做 TCP，UDP 路径集中在 M5 |
| D6 | `PolicySpec` 的位置 | `rurge-config`；订阅导入行复用同一套校验 |
| D7 | 出站的构造方 | `rurge-policy` 定义 `OutboundFactory`，`rurge-engine` 实现；`rurge check` 复用它做干构建 |
| D8 | 重载时的出站 | 按指纹复用，未变的策略不重建 |
| D9 | 用户态协议栈（PRD Q7 原定阶段 3 决定） | 提前到 M4：`smoltcp`，先作为 `rurge-proto-wireguard` 的内部模块、接口收窄；阶段 3 做 TUN 时按基准复核，合适再抽成公共 crate |
| D10 | Windows 的网卡绑定 | 先用"绑定该网卡的源地址"，不需要 unsafe |
| D11 | 组的环 | `E0009` 在 M3 降级为告警 + 运行期 REJECT（FR-GRP-05） |
| Q1 | Windows 上是否改用 `IP_UNICAST_IF` | 已决（2026-09-19）：不改，继续用"绑定该网卡的源地址"，不引入 unsafe；差异登记在兼容性清单 |
| Q2 | Shadow TLS v3 的实现路径 | 已决（2026-09-20，M2-D3）：stock rustls，两遍构造 ClientHello |
| Q3 | 六个 API 端点的 JSON 形状 | M1 / M3 细化设计时按真实样本定（PRD R5） |
| Q4 | sing-box 的用户态 WireGuard 端点能否充当带保留字节的对端 | M4 细化设计时验证；不行则用 boringtun 写回环对端 |
| Q5 | `russh` 的算法覆盖 | M4 细化设计时验证 |
| Q6 | MASQUE 与 trust-tunnel 的参考服务端 | M7 细化设计时选定 |
| Q7 | Snell 各版本可依据的公开资料 | M6 细化设计时确认 |

## 17. 需同步的文档

| 文档 | 改动 | 时机 |
| ---- | ---- | ---- |
| `docs/requirements.md` | 3.2 模块表增加三个协议 crate；第 8 节集成测试一行改为三层方案；9.2 的 Q7 改为"阶段 2（M4）" | 随本文档 |
| `CLAUDE.md` | 「先读这些文档」加入本文档 | 随本文档 |
| `docs/surge-compatibility-matrix.md` | 各行状态随里程碑更新；行为差异（`smart` 近似、Windows 网卡绑定方式、Shadow TLS v3 的结论等）逐条登记 | 各里程碑收尾 |
| `README.md`（中英） | 状态、特性表与路线图 | 各里程碑收尾 |
| `docs/api/phase2.md`、`docs/acceptance/phase2-manual.md` | 新建 | M1 / M3；M2a（原定 M8，提前：Trojan 合并后项目所有者即可用真实节点手工验收） |
