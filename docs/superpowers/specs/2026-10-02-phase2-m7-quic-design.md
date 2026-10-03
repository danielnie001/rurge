# 阶段 2 / M7「QUIC 族」细化设计

细化阶段 2 总设计（`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`）第 1.4 节的 M7 里程碑、第 6 节的 `tuic` / `tuic-v5` / `hysteria2` / `masque` / `trust-tunnel` 五行、第 8.4 节的 DoH3 / DoQ 与第 13 节 M7 的互操作参考；与总设计不一致处以本文为准。开放问题 Q6（MASQUE 与 `trust-tunnel` h3 的参考服务端）在本文第 2 节关闭；风险 B（`h3` 的成熟度）在本文第 2 节与第 4 节处理。

调研只读（2026-10-01）：quinn / quinn-proto / quinn-udp、h3 / h3-quinn / h3-datagram 与 hickory 的源码与发布记录，Hysteria 2 官网协议规范，TUIC v5 的 `SPEC.md` 与 v4（0.8.5）的协议说明，RFC 9000 / 9001 / 9114 / 9204 / 9220 / 9221 / 9250 / 9297 / 9298，TrustTunnel 的协议与配置文档，Surge 手册各协议页面。没有下载或安装任何二进制。

## 1. 目标与范围

| 项目 | 本里程碑 | 延后 / 不做 |
| ---- | -------- | ----------- |
| `hysteria2` | TCP、UDP（QUIC 数据报，含分片）、`download-bandwidth`、Salamander、Gecko、`port-hopping` | 客户端的 Brutal 拥塞控制（Surge 没有上行带宽参数，不需要） |
| `tuic-v5` | TCP、UDP（`native` 数据报含分片 / `quic` 单向流，自动选择）、心跳、`port-hopping` | 0-RTT（Surge 没有参数） |
| `tuic`（v4） | 同上（v4 的线上格式：token 认证、Connect 应答、不分片） | 同上 |
| `masque` | HTTP/3 CONNECT（TCP）、CONNECT-UDP（RFC 9298，HTTP Datagram 与 DATAGRAM capsule）、Basic 认证、`port-hopping` | CONNECT-IP |
| `trust-tunnel` | `h3=true`：HTTP/3 上的 CONNECT（TCP） | UDP（Surge 也不支持） |
| DNS 上游 | `quic://`（DoQ）、`h3://`（DoH3） | — |
| 策略参数 | `port-hopping`、`port-hopping-interval`、QUIC 类的 `ecn` | `wireguard` 的 `ecn`（仍只解析） |

对应需求：FR-OUT-03（`ecn`）/ FR-OUT-05、FR-DNS-04。验收（总设计第 1.4 节）：QUIC 族的 TCP 与 UDP 转发对参考实现通过；`h3://` 与 `quic://` 上游解析正常。

## 2. 已决事项

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| M7-D1 | 范围 | Hysteria 2、TUIC v5 与 v4、MASQUE 与 `trust-tunnel` 的 h3、DoH3 / DoQ 全部做（项目所有者，2026-10-01） |
| M7-D2 | 拆分与顺序 | 三份计划：M7a QUIC 地基 + Hysteria 2 → M7b TUIC → M7c HTTP/3 族（MASQUE、`trust-tunnel` h3）+ DoQ / DoH3。风险最大的 HTTP/3 扩展放在最后，不拖累前两份（项目所有者，2026-10-01） |
| M7-D3 | TUIC v4 | 实现（与 v5 共用大部分代码）；自动化以回环假服务端为主，另在 Linux CI 上对官方 tuic 0.8.5 二进制做互操作（项目所有者，2026-10-01） |
| M7-D4 | `ecn` | 跟随 quinn 的能力：`auto` / `on` 取 quinn 的缺省行为，`off` 在 UDP 层去掉标记（项目所有者，2026-10-01；第 3.4 节） |
| M7-D5 | HTTP/3 | 自写最小的 HTTP/3 客户端（第 4 节），不用 crates.io 上的 h3。h3 0.0.8 / h3-datagram 0.0.2（2025-05 之后没有新版本）有挡住 MASQUE 与 `trust-tunnel` h3 的缺陷：普通 CONNECT 也带 `:scheme` / `:path`、HTTP Datagram 的流号恒为 0、写操作被取消时关掉整条连接、读服务端 SETTINGS 要开声明会有破坏性变更的 feature 且没有等待 SETTINGS 的接口（项目所有者，2026-10-01） |
| M7-D6 | 参考服务端 | sing-box 1.14.2：`hysteria2`（无混淆 / Salamander / Gecko）与 `tuic`（v5），三平台；官方 tuic 0.8.5：v4，只在 Linux CI；Vincent-bin/masque-server v0.13.0（MIT）：MASQUE，只在 Linux / macOS CI（项目所有者，2026-10-01）；TrustTunnel endpoint v1.1.0 的 QUIC 监听：`trust-tunnel` h3，只在 Linux / macOS CI。关闭总设计开放问题 Q6 |
| M7-D7 | 依赖 | 新增 `quinn` 0.11.12（`default-features = false`，features `runtime-tokio`、`rustls-ring`；不碰 aws-lc-rs），随之引入 `quinn-proto` 0.11.x 与 `quinn-udp` 0.5.x（MSRV 1.85，满足 1.89）。不用 `h3`、`h3-quinn`、`h3-datagram`、`hickory-net`。`blake2` 0.10（Salamander）与 `blake3`（TUIC v4）已在依赖树里，只加依赖边 |
| M7-D8 | crate 布局 | `rurge-net::quic`：UDP 层、拨号与 TLS、自写的 `h3`，DNS 与出站共用。新 crate `rurge-proto-quic`：会话与四种协议、回环假服务端。依赖方向：`rurge-engine → rurge-proto-quic → { rurge-proto, rurge-net, rurge-config }`；`rurge-dns → rurge-rules → rurge-net`（DoQ / DoH3 经 `rurge-net::quic`）。`rurge-dns` 不依赖协议 crate |
| M7-D9 | 连接与 socket | 每条 QUIC 连接一个 quinn `Endpoint`，配我们自己的 `AsyncUdpSocket` 层（第 3.3 节）；端口跳跃、混淆、链式载体都在这一层 |
| M7-D10 | 拥塞控制 | 客户端一律用 quinn 的 Cubic（quinn 的 BBR 标为实验性）；Hysteria 2 的 `download-bandwidth` 只上报给服务端，客户端不实现 Brutal |
| M7-D11 | Gecko | 按 Hysteria 官网公开规范实现；Surge 的 `gecko-password` 是否就是它尚未证实（名称与 Surge 更新说明强烈暗示是），登记并在手工验收里核对 |
| M7-D12 | `W0007` 与 `OutboundError::Unsupported` | 总设计说两者在 M7 结束时退役；M7 之后仍有留给 M8 的项目（`tailscale`、ss 流式旧方法、vmess 旧握手、Snell v1–v3 / v6），所以退役改到 M8 |

## 3. QUIC 共用部分（`rurge-net::quic` 与 `rurge-proto-quic::session`）

### 3.1 配置

`rurge-config::spec` 新增 `QuicOpts`，四种协议共用：

- `alpn`：缺省 `h3`（手册："TUIC and Hysteria 2 use `h3` by default"，MASQUE 页面同）；`trust-tunnel` 的 h3 固定 `h3`（第 7.2 节）。
- `port-hopping`：写法 `1234;5000-6000`，分号分隔单个端口或闭区间；写了它，声明里的主端口被忽略。写法错误（空、端口超出 1–65535、区间颠倒）是 `E0018`。
- `port-hopping-interval`：秒，缺省 30；小于 5 时 `W0028` 并按 5（参考实现的下限）。
- `ecn`：`auto` / `on` / `off`，缺省 `auto`（第 3.4 节）。
- 两条约束真正报出 `E0018`：`underlying-proxy` 不能与 `port-hopping` 同用；Shadow TLS 不能与 QUIC 类组合（`tuic` `tuic-v5` `hysteria2` `masque`，以及 `h3=true` 的 `trust-tunnel`；判断函数 M2c 已落地）。

### 3.2 拨号与 TLS

- **服务器地址**：每次新建连接时经策略自己的解析路径（含 `[Host]`、`dns-follow-interface`）解析，按 `ip-version` 选地址，规则与 `DirectConnector::connect_udp`（M4b）相同；有 `port-hopping` 时主机名每条连接只解析一次。
- **TLS**：只用 TLS 1.3（QUIC 的要求）；沿用现有的根证书、`skip-cert-verify`、`server-cert-fingerprint-sha256`、`sni` 与 `client-cert`。IP 字面量的服务器不发 SNI，除非写了 `sni=`。不用 0-RTT。服务端不选 `alpn` 时 rustls 以 `NoApplicationProtocol` 失败（登记）。
- **传输参数**：空闲超时 30 秒、保活 10 秒（断掉的路径——休眠唤醒、换网——在 30 秒内被发现）；接收窗口每条流 8 MiB、每条连接 20 MiB（同 Hysteria 参考客户端）；不允许服务端开双向流；单向流照 quinn 缺省（HTTP/3 的控制流与 QPACK 流、TUIC 的 `quic` 模式要用）；拥塞控制 Cubic（M7-D10）。

### 3.3 UDP 层

都实现 quinn 的 `AsyncUdpSocket`，可以层层叠加：

- **直连**：用现有的 `SocketOpts` 建 socket（网卡绑定、`ip-version`、收发缓冲），交给 quinn 自己的收发实现（GSO、逐包 ECN）。`tos ≠ 0` 时改用不经 quinn-udp 的普通 socket：quinn-udp 在 Linux / macOS 上每个包都写 TOS/TCLASS（只有 ECN 两位），会把 socket 上设的 DSCP 清零；代价是没有 ECN 与 GSO（登记）。
- **`underlying-proxy`**：用 `ChainConnector::connect_udp` 给出的 `Datagram` 做载体，目标是服务器名原样（交给底层远程解析）；quinn 拿到的是一个占位地址，这一层忽略发送目的地址。这种载体不做 PMTU 探测（`may_fragment()` 为真），没有 ECN。底层策略不载 UDP 时拨号失败（同 M5c）。
- **端口跳跃**：第一个端口随机取，之后每个间隔换一个随机端口，同时新开一个本地 socket；上一个 socket 保留到下一次跳跃，期间继续收迟到的包；发送时忽略 quinn 给的目的地址、发往当前端口；交给 quinn 的来源地址总是固定的服务器地址（quinn 客户端会丢掉来源不是对端地址的包）。
- **混淆**（Hysteria 2 专用，第 5.4 节）：套在以上任一层外面；声明每次只收发一个包（不做 GSO 批量）。
- **`ecn=off`**：在这一层去掉发出与收到两侧的 ECN 标记，quinn 的 ECN 校验随之停用。

### 3.4 ECN

`auto` 与 `on` 都取 quinn 的缺省：先发 ECT，ACK 不带 ECN 计数或校验失败就自行停用；quinn 没有"强制开启"（`on` 等同 `auto`，登记）。`off` 见第 3.3 节。链式载体与 `tos ≠ 0` 的直连没有 ECN。`wireguard` 上的 `ecn` 仍只解析（`W0029`）。

### 3.5 会话（`rurge-proto-quic::session`）

- 每个策略一条 QUIC 连接，多路复用；第一次用时才拨号，同时到来的请求共用一次拨号与认证（单飞，失败由等待者共享）。
- 可选的每条连接流数上限：只有 `trust-tunnel` 用（`max-streams`，第 7.2 节）；设了上限时规则同 HTTP/2 会话池（M6 设计第 16 节第 4 条与第 18 节第 1 条）：选最早的、有空位且没在排空的连接；都满时按需求并行拨号（在途的每次拨号算作 `max-streams` 个位置，同时至多 8 条在途）。
- 连接关闭（出错、空闲超时、对端关闭、HTTP/3 的 GOAWAY）后不再分配；下一个请求重新拨号。旧连接上已有的流随之以错误结束（GOAWAY 时已有的流走完）。
- **失效的连接重试一次**：刚取用的连接已经被对端丢弃（例如服务端空闲超时后回了 stateless reset）、请求在任何数据发出之前就失败时，在新连接上重试一次。
- 没有流也没有 UDP 会话满 60 秒，主动关闭连接（同 M6c）。
- 重载：参数与指纹不变的策略沿用上一代的出站与它的连接（总设计的按指纹复用）。
- 每个请求用 `opts.timeout` 限时，包括拨号、握手与认证；会话本身不另设时限。
- 后台任务（连接驱动、数据报分发、心跳、端口跳跃）随连接或出站对象结束，不留孤儿任务；锁里只做内存操作，不跨 `.await`。

### 3.6 错误文本

协议层形如 `hysteria2: …`、`tuic: …`、`masque: …`、`trust-tunnel: …`；QUIC 传输层的错误形如 `quic: …`（握手超时、对端关闭及其错误码）；TLS 错误沿用 `tls: …`。凭据（口令、token、UUID、混淆口令、`proxy-authorization` 的值、`headers` 的值）与载荷不进日志、错误文本与 `Debug`（配置里用 `Secret<T>`）。

## 4. 自写的 HTTP/3 客户端（`rurge-net::quic::h3`）

基本请求在 M7a（Hysteria 2 的认证要用），extended CONNECT 与 HTTP Datagram 在 M7c。

- **控制流**：开一条控制流（单向流类型 0x00），首帧 SETTINGS：`QPACK_MAX_TABLE_CAPACITY=0`、`QPACK_BLOCKED_STREAMS=0`、`MAX_FIELD_SECTION_SIZE=65536`；用 HTTP Datagram 时另加 `H3_DATAGRAM=1`（0x33）。
- **服务端开的单向流**：控制流——读 SETTINGS（必须是首帧）与 GOAWAY；QPACK 编码器流（0x02）与解码器流（0x03）——读了丢弃（动态表容量为 0，服务端不能插入）；推送流（0x01）——我们从不发 `MAX_PUSH_ID`，出现即为协议错误；不认识的流类型——拒收（`STOP_SENDING`）。
- **等 SETTINGS**：提供"等服务端 SETTINGS 到达"的入口，受请求时限约束；extended CONNECT 要求 `SETTINGS_ENABLE_CONNECT_PROTOCOL=1`（0x08），HTTP Datagram 另要求 `H3_DATAGRAM=1` 且 QUIC 层支持数据报。
- **请求**：每个请求一条双向流，HEADERS 帧加 DATA 帧。QPACK 编码只用静态表：能精确匹配就用静态索引，否则静态名加字面值，或全字面量；字段节前缀恒为 Required Insert Count 0；发送不做 Huffman。普通 CONNECT 只有 `:method` 与 `:authority`（RFC 9114 §4.4）；extended CONNECT 带 `:protocol` `:scheme` `:path` `:authority`。
- **应答**：解码 HEADERS（静态表、字面量、Huffman 解码；引用动态表或 post-base 形式即为 `QPACK_DECOMPRESSION_FAILED`），跳过 1xx 与不认识的帧类型，DATA 帧组成字节流，忽略 trailers。
- **作为隧道的字节流**：写入时把数据包进 DATA 帧；一帧写到一半被取消时，剩下的部分下次先写完再接受新数据——不写坏帧，也不因取消关掉整条连接。`shutdown` 发 FIN；背压靠 quinn 的流控；读到多少交出多少。
- **GOAWAY**：收到后这条连接不再接新请求（会话另建连接），已有的流走完。
- **HTTP Datagram**（M7c）：直接建在 quinn 数据报上，格式 `varint(流号 / 4) ‖ varint(上下文号) ‖ 负载`；收到的按流号分给对应的请求流，没有对应的丢弃；上下文号不为 0 的丢弃。
- **错误**：连接级错误以 RFC 9114 的错误码关闭连接；流级错误重置该流。

## 5. M7a：Hysteria 2

### 5.1 配置

- `password`：必填，`Secret`；缺了是 `E0018`，报错不引用取值。
- `download-bandwidth`：Mbps，非负整数，可不写。
- `salamander-password` / `gecko-password`：`Secret`，不能为空；两个都写了是 `E0018`（手册写明二者互斥）。
- 另外适用 `QuicOpts` 与 TLS 参数。

### 5.2 认证

连接建好后发 HTTP/3 `POST https://hysteria/auth`（`:authority hysteria`、`:path /auth`），请求头：

- `Hysteria-Auth`：口令；
- `Hysteria-CC-RX`：`download-bandwidth × 125000`（字节/秒），没写就是 `0`；
- `Hysteria-Padding`：256–2047 个随机 `[a-zA-Z0-9]`。

应答 233 算成功，记下 `Hysteria-UDP`（`true` / `false`）。其它状态（服务端此时表现为普通网站）一律是 `hysteria2: authentication failed`。服务端回的 `Hysteria-CC-RX` 不改变客户端行为（M7-D10）。

### 5.3 TCP 与 UDP

- **TCP**：每个连接一条双向流。请求：`varint 0x401`（线上 `44 01`）‖ `varint 地址长度` ‖ `"host:port"`（IPv6 带方括号，域名转 A-label）‖ `varint 填充长度` ‖ 64–511 字节填充。等服务端应答（不用 fast-open，参考客户端的缺省）：`u8 状态 ‖ varint 消息长度 ‖ 消息 ‖ varint 填充长度 ‖ 填充`；状态 0 开始转发；状态 1 是 `hysteria2: the server refused the connection: <消息>`（消息截到 200 字符、只保留可打印字符）。
- **UDP**（自动支持，走 QUIC 数据报）：`u32 会话号 ‖ u16 包号 ‖ u8 片号 ‖ u8 片数 ‖ varint 地址长度 ‖ "host:port" ‖ 负载`。
  - 认证时服务端回 `Hysteria-UDP: false`：UDP 流以 `hysteria2: the server does not allow UDP` 失败。
  - 每个 UDP 载体（"关联 × 出站"）占一个会话号，每条连接内从 1 递增；全锥。连接重建后载体在新连接上沿用同一会话号（服务端当作新会话，映射随之改变）。
  - 放不进一个数据报才分片，每片都带完整的头；分片的包取 1–65535 的随机包号，不分片的包号为 0。
  - 重组：每个会话同一时刻只重组一个包，新包号到来就丢掉旧的；任一片丢失，整个包丢弃。
  - 每条连接一个读任务按会话号分发；载体收件箱满了就丢包；重组占用的内存有上限。
  - 协议没有关闭会话的消息：载体关闭后本地忘掉会话号，之后到达的数据报丢弃；服务端空闲回收。

### 5.4 混淆

- **Salamander**：每个包前加 8 字节随机 salt，`key = BLAKE2b-256(口令 ‖ salt)`，负载与 key 循环异或；短于 8 字节或解不开的包丢弃。BLAKE2b-256 指输出长度参数为 32 的 BLAKE2b（与 Go 的 `blake2b.Sum256` 相同，不是截断的 BLAKE2b-512），用已知答案核对。
- **Gecko**：短头的包（首字节最高位为 0）只做 Salamander（口令取 `gecko-password`）；长头的握手包随机拆成 2–8 片，每片加 5 字节头（`0x80` ‖ 消息号 ‖ `片号 << 4 | 片数` ‖ `u16` 填充长度）与随机填充，使整个 UDP 包落在 512–1200 字节（参考实现的缺省；分片本身超过上限时填充为 0），每片各做一次 Salamander。收到时按消息号重组：至多 8 组、8 秒过期、重复的片丢弃。

### 5.5 测试

- 第 1 层：Salamander 与 Gecko 的编解码（BLAKE2b-256 已知答案）、请求与 UDP 帧的布局、分片与重组、端口跳跃的端口集合解析。
- 第 2 层：`FakeHysteria2`（quinn 服务端 + 第 4 节的 HTTP/3 服务端一侧，混淆与帧的编解码独立实现）：认证成功与失败、TCP 与拒绝、UDP 与分片、`Hysteria-UDP: false`、两种混淆、端口跳跃（假服务端同时监听几个端口；跳跃间隔由只用于测试的入口缩短，不用固定 sleep 做同步）、连接断开后重连、失效连接的重试；经引擎的端到端用例。
- 第 3 层：sing-box 1.14.2 的 `hysteria2` 入站（三平台；无混淆、Salamander、Gecko 三种）：TCP、UDP、认证失败。端口跳跃要在服务端配 DNAT，只有假服务端与手工验收。
- 不作门禁的吞吐基准：经回环假服务端双向回显（同 WireGuard 的基准做法），衡量 UDP 层叠加后的开销。

## 6. M7b：TUIC

### 6.1 配置

- `tuic-v5`：`uuid=`（带连字符的 UUID）与 `password=` 必填；`tuic`（v4）：`token=` 必填。缺了或 UUID 格式不对是 `E0018`，报错不引用取值；凭据都是 `Secret` 并在脱敏名单里。
- 另外适用 `QuicOpts` 与 TLS 参数。Surge 没有 UDP 模式、心跳、0-RTT 参数，下文都取内定值。tuic 行上的 `udp-relay` 是 `W0028`（UDP 自动支持）。

### 6.2 v5

- **认证**：握手完成后开一条单向流：`05 00 ‖ UUID（16 字节原始值）‖ TOKEN（32 字节）`；TOKEN 由 TLS 导出密钥算出（label = UUID 原始字节，context = 口令，长度 32）。协议允许与其它命令并行发出，不等。
- **TCP**：每个连接一条双向流：`05 01 ‖ ADDR`；服务端不回应，发完即转发（目标连不上表现为流被重置或提前结束）。
- **ADDR**：`u8 类型 ‖ 地址 ‖ u16 端口`；`00` 域名（长度字节 + 名字）、`01` IPv4、`02` IPv6；`ff` 没有地址也没有端口（只用于分片）。
- **UDP**：每个载体占一个关联号（每条连接内递增的 `u16`，跳过正在用的），全锥。服务端支持 QUIC 数据报时用 `native`：`05 02 ‖ ASSOC u16 ‖ PKT u16 ‖ FRAG_TOTAL u8 ‖ FRAG_ID u8 ‖ SIZE u16 ‖ ADDR ‖ 负载`，放不进一个数据报就分片，只有第一片带地址；否则用 `quic` 模式（每个包一条单向流）。收到时重组：每个关联同时至多重组 8 个包，更早的丢弃。载体关闭时发 `Dissociate`（`05 03 ‖ ASSOC`，单向流）。
- **心跳**：有转发在进行时每 3 秒发一个心跳数据报（`05 04`），同 tuic 1.0 客户端（tuic-server 的空闲超时缺省 10 秒，QUIC 自己 10 秒一次的保活来不及）。
- **服务端关闭连接**：v5 规范没有定义错误码，报 `tuic: the server closed the connection (<码>)`；sing-box 认证失败时怎样关连接写计划时核对（V4），能认出就报 `tuic: authentication failed`。

### 6.3 v4

- **认证**：单向流 `04 00 ‖ BLAKE3(token)`（32 字节），写完即关。
- **TCP**：双向流 `04 01 ‖ ADDR`，等同一条流上的应答 `04 ff ‖ REP`：`00` 成功，`ff` 是 `tuic: the server could not connect to the target`。
- **UDP**：`04 02 ‖ ASSOC u32（随机）‖ LEN u16 ‖ ADDR ‖ 负载`，不分片；走数据报时放不进一个数据报的包丢弃（记 debug 日志）；服务端不支持数据报时走单向流。`Dissociate` 用 `u32` 关联号。ADDR 只有 `00` `01` `02` 三种类型。
- **心跳**：单向流 `04 04`，有转发时每 10 秒一次（0.8.5 客户端的缺省）。
- **服务端关闭连接的错误码**：`0xfffffff1` → `tuic: authentication failed`；`0xfffffff2` → `tuic: authentication timed out`；`0xfffffff0` / `0xfffffff3` → `tuic: protocol error`。

### 6.4 测试

- 第 1 层：两个版本的命令与地址编码、分片与重组、BLAKE3 token 的已知答案。
- 第 2 层：`FakeTuic`（v4 / v5）：认证成功与失败、TCP（v4 的应答与拒绝）、UDP 的两种模式、v5 的分片、`Dissociate`、能看到心跳、失效连接的重试、重连；经引擎的端到端用例。
- 第 3 层：v5 对 sing-box 1.14.2 的 `tuic` 入站（服务端配 `alpn: ["h3"]`，三平台）：TCP、UDP、认证失败；v4 对官方 tuic 0.8.5 二进制（GPL，只作为外部进程运行，只在 Linux CI 上按 SHA-256 安装）。

## 7. M7c：HTTP/3 族与 DNS

### 7.1 MASQUE

- **配置**：`username` / `password` 可选，要么都写、要么都不写（只写一个是 `E0018`）；另外适用 `QuicOpts` 与 TLS 参数（手册的 MASQUE 页面列了 `sni`、`skip-cert-verify`）。
- **TCP**：HTTP/3 普通 CONNECT（`:method CONNECT`、`:authority host:port`）；有凭据时主动带 `proxy-authorization: Basic …`，不等 407 质询。2xx 即隧道；407 是 `masque: proxy authentication required`；其它是 `masque: the proxy answered <状态码>`。
- **UDP**（自动支持，RFC 9298）：每个目标一条 `connect-udp` 流（对称型，同 M6c 的 `h2-connect`）：`:protocol connect-udp`、`:scheme https`、`:authority` 为 TLS 的名字（写了 `sni=` 时是它，否则是服务器；同 M6 设计第 18 节第 4 条）、`:path /.well-known/masque/udp/{host}/{port}/`（缺省模板；IPv6 不带方括号、冒号写成 `%3A`，名字转 A-label）、`capsule-protocol: ?1`；2xx 之后才发数据。数据报走 HTTP Datagram（第 4 节）；超过 QUIC 数据报上限的包改用流上的 DATAGRAM capsule（RFC 9297 允许）；收的时候两种都接受。capsule 编解码复用 M6c 的。每个目标的流空闲 60 秒关闭；被服务端立刻结束的流每个数据报只重开一次（同 M6c 终审修正）。
- **服务端能力**：SETTINGS 里没声明 extended CONNECT 与 `H3_DATAGRAM`，或 QUIC 层不支持数据报时，UDP 流以 `masque: the server does not support CONNECT-UDP` 失败，同一连接上的 TCP 照常（手册说"连接时校验"；只让 UDP 失败，登记）。

### 7.2 `trust-tunnel` 的 h3

- `h3=true` 的 `W0029` 退役；引擎工厂为它构建 `rurge-proto-quic` 里的 h3 实现。
- ALPN 固定 `h3`，写了别的 `alpn` 是 `W0028`；手册没给 `trust-tunnel` 列 `port-hopping`，写了是 `W0028`。
- HTTP/3 普通 CONNECT，请求头与 HTTP/2 版相同（`proxy-authorization`、`user-agent: rurge`、`headers` 可覆盖），错误文本也相同（407 → `trust-tunnel: authentication failed`；其它 → `trust-tunnel: the server answered <状态码>`）。
- `max-streams`：每条 QUIC 连接上的流数上限，满了另建一条（第 3.5 节）。
- 不载 UDP（同 HTTP/2 版）；不做 HTTP/3 → HTTP/2 的自动回落（登记）。

### 7.3 DoQ / DoH3

- **`quic://host[:853]`（DoQ，RFC 9250，自写）**：ALPN `doq`；每个上游一条 QUIC 连接（空闲 60 秒关闭，失效后重连）；每个查询一条新的双向流：2 字节长度 + 报文（报文 ID 置 0），写完即发 FIN；应答回来后恢复原 ID。
- **`h3://host[:443]/path`（DoH3）**：与阶段 1 的 DoH 一样用 RFC 8484 的 POST（`application/dns-message`），跑在第 4 节的 HTTP/3 上，ALPN `h3`。
- 沿用阶段 1 的语义：引导解析（域名写的上游先经普通 UDP 解析）、`encrypted-dns-skip-cert-verify`、每个上游的时限与重试。
- **`encrypted-dns-follow-outbound-mode`**：阶段 1 的 `PipelineConnector` 只把 TCP / DoT / DoH 送进流水线；给它补上 UDP：QUIC 的载体作为内部 UDP 会话走规则与策略；策略不载 UDP 时这个上游这次失败，其它上游照常；链底为 REJECT 时沿用现有的旁路。具体接口写计划时核对（V6）。
- `W0026` 不再因 `h3://` 与 `quic://` 条目出现。

### 7.4 测试

- 第 1 层：QPACK（静态表、字面量、Huffman 解码，用 RFC 9204 与 RFC 7541 的例子）、帧编解码、取消写入时的帧完整性、HTTP Datagram 的编码、MASQUE 路径。
- 第 2 层：`FakeH3Proxy`（quinn 服务端 + HTTP/3 服务端一侧）：CONNECT 与 Basic、407、`connect-udp`（数据报与 capsule 两种）、SETTINGS 里没声明 extended CONNECT / `H3_DATAGRAM`、GOAWAY 后另建连接、`max-streams`；回环的假 DoQ / DoH3 服务端：查询、ID 恢复、超时、连接重建、跟随出站模式；经引擎的端到端用例。
- 第 3 层（只在 Linux / macOS CI 上按 SHA-256 安装）：masque-server v0.13.0（TCP、UDP、认证失败）；TrustTunnel endpoint v1.1.0 的 `[listen_protocols.quic]`（`trust-tunnel` h3；能否这样启用写计划时核对，不行就退到假服务端与手工验收）。公共解析服务（例如 AdGuard 的 DoQ、Google 的 DoH3）只进手工验收清单。

## 8. 跨里程碑的部分

- **能力表**：M7a 后 `hysteria2`；M7b 后 `tuic`、`tuic-v5`；M7c 后 `masque` 与 `trust-tunnel` 的 `h3=true`。`W0007` 剩下的项目见 M7-D12。
- **未实现协议的测试例子**：现有用例以 `hysteria2` 作"未实现的协议"的例子（M6a 起）；M7a 起换成仍未实现的类型（V8）。
- **重载**：新 spec 的指纹含全部参数（含 `Secret` 的取值）；参数没变的策略沿用出站与它的 QUIC 连接。
- **脱敏**：`password`、`token`、`uuid` 已在内联参数的脱敏名单里；增加 `salamander-password` 与 `gecko-password`。
- **订阅**：四种协议的订阅导入行照常生效，不引用 Keystore 或本机程序，不需要新的订阅安全门。
- **测速**：按 URL 测速，经出站本身；UDP 测试（`test-udp`）经出站的 UDP。
- **测试约束**：只用回环；不改本机网络与代理设置；不在本机安装参考二进制（缺了就跳过，CI 设 `RURGE_INTEROP_REQUIRED=1`）。

## 9. 需登记进兼容性清单的差异

1. QUIC 层：`tos ≠ 0` 时 QUIC 类没有 ECN 与 GSO；`ecn=on` 等同 `auto`；拥塞控制固定 Cubic，`download-bandwidth` 只上报给服务端；服务端不选 ALPN 时握手失败；不用 0-RTT。
2. Gecko 与 Surge 的 `gecko-password` 是否同一算法未证实；包长取 512–1200。
3. TUIC 的 UDP 模式自动选择；心跳间隔为内定值；v5 认证失败的错误文本可能不是 `authentication failed`；v4 走数据报时放不下的包丢弃。
4. MASQUE：不支持 CONNECT-UDP 的服务端只让 UDP 失败；凭据主动发送；TCP 用普通 CONNECT；URI 模板用缺省值；UDP 为对称型（每个目标一条流）。
5. `trust-tunnel` 的 h3：`max-streams` 按每条 QUIC 连接计；没有 HTTP/3 → HTTP/2 的自动回落。
6. Hysteria 2 的会话号在连接重建后沿用（服务端视为新会话，映射改变）。

## 10. 风险

| 风险 | 影响 | 应对 |
| ---- | ---- | ---- |
| 自写的 HTTP/3 只对 quic-go（sing-box）与 quiche（TrustTunnel、masque-server）做过互操作 | 其它服务端（例如 nginx）可能暴露兼容问题 | 第 1 层的 QPACK 例子要全；手工验收覆盖项目所有者的真实节点 |
| quinn 的 `AsyncUdpSocket` 叠加（端口跳跃、混淆、链式载体）的性能 | 吞吐与 CPU | M7a 的吞吐基准（不作门禁） |
| TUIC v4 与 Gecko 只有单一参考实现 | 自洽而与真实服务端不通 | 以 CI 上的互操作为准；手工验收 |
| masque-server 项目新、维护者少 | 参考实现自身的缺陷 | 以 RFC 为准；有分歧时登记而不迁就 |
| crates.io 后来发布修好缺陷的 h3 | 自写层的维护成本 | 接口只在 `rurge-net::quic::h3` 内部；M8 再评估是否替换 |

## 11. 写各份计划时必须核对的事项

| 编号 | 事项 | 属于 |
| ---- | ---- | ---- |
| V1 | quinn 0.11.12：`AsyncUdpSocket` / `UdpPoller`、`TokioRuntime::wrap_udp_socket`、`Endpoint::new_with_abstract_socket`、数据报 API、`export_keying_material`、ECN 校验失败时的行为、连接到占位地址是否被拒 | M7a / M7b |
| V2 | BLAKE2b-256（`blake2::Blake2b<U32>`）与 Go `blake2b.Sum256` 的一致性（已知答案） | M7a |
| V3 | Gecko 帧与 Salamander 的逐字节细节（官网规范原文） | M7a |
| V4 | sing-box 的 TUIC 入站在认证失败时怎样关连接 | M7b |
| V5 | tuic 0.8.5、masque-server v0.13.0、TrustTunnel QUIC 监听的发布资产名、SHA-256 与配置写法（只读核对，不下载） | M7b / M7c |
| V6 | `PipelineConnector` 补 UDP 的接口与引擎内部 UDP 会话的记录 | M7c |
| V7 | Surge 手册：`hysteria2` `tuic` `tuic-v5` `masque` `trust-tunnel` 的参数全集与缺省值、`port-hopping` 写法 | 各份 |
| V8 | 仍未实现、可作测试例子的策略类型（例如 `tailscale`）在 `rurge-config` 里的现状 | M7a |
| V9 | RFC 9204 的静态表全表与 RFC 7541 的 Huffman 码表（照原文录入并用例子核对） | M7a |
| V10 | quinn 的单向流与数据报缓冲的缺省值是否满足 HTTP/3 与 TUIC 的 `quic` 模式 | M7a / M7b |

## 12. 任务草图

**M7a QUIC 地基 + Hysteria 2**

1. `QuicOpts` 与 `Hysteria2Spec`（两条 `E0018` 约束真正报出、脱敏名单）。
2. `rurge-net::quic` 的 UDP 层（直连、`tos`、`ecn=off`、端口跳跃、链式载体）与拨号 / TLS。
3. `rurge-net::quic::h3` 的基本请求（控制流、SETTINGS、QPACK 静态表与 Huffman、HEADERS / DATA、GOAWAY）。
4. `rurge-proto-quic` 与 `session`（单飞、重连、空闲关闭、失效重试）。
5. Hysteria 2 的认证与 TCP、`FakeHysteria2`。
6. Hysteria 2 的 UDP（分片与重组）。
7. Salamander 与 Gecko。
8. 引擎装配、能力表翻转、经引擎的用例、吞吐基准。
9. 互操作（sing-box）与文档。

**M7b TUIC**

1. `TuicSpec`（v4 / v5）。
2. v5 的认证与 TCP、`FakeTuic`。
3. v5 的 UDP（两种模式、分片、`Dissociate`、心跳）。
4. v4。
5. 引擎装配、互操作（sing-box、tuic 0.8.5）与文档。

**M7c HTTP/3 族与 DNS**

1. `MasqueSpec` 与 `trust-tunnel` 的 h3 配置。
2. h3 的 extended CONNECT 与 HTTP Datagram、`FakeH3Proxy`。
3. MASQUE 的 TCP 与 CONNECT-UDP。
4. `trust-tunnel` 的 h3（含 `max-streams` 的多连接）。
5. DoQ 与 DoH3、`PipelineConnector` 的 UDP。
6. 引擎装配、互操作（masque-server、TrustTunnel）与文档。

## 13. 需同步的文档

- 总设计：第 1.4 节 M7 一行（拆分）；第 6 节五行的 crate 与说明；第 13 节 M7 的参考实现按 M7-D6；风险 B 按 M7-D5 处理；开放问题 Q6 标为已决；`W0007` / `Unsupported` 的退役改到 M8（M7-D12）。
- 兼容性清单：各协议行、4.4 节 `alpn`、4.5 节 UDP、4.6 节参数行（`port-hopping`、`ecn`、各协议参数）、`encrypted-dns-server` 与 `h3://` / `quic://` 两行；第 9 节的差异。
- `docs/acceptance/phase2-manual.md`：M7a / M7b / M7c 各一节（真实节点上的 TCP 与 UDP、两种混淆、端口跳跃、TUIC v4、MASQUE、`trust-tunnel` h3、公共 DoQ / DoH3 服务）。
- README（两份）与 `CLAUDE.md`：随各计划的文档任务更新。

## 14. M7a 计划期的订正

写 M7a 实施计划（`docs/superpowers/plans/2026-10-02-phase2-m7a-quic-hysteria2-plan.md`）时核对 quinn / rustls 的本机源码、RFC、Hysteria 2 官网规范与参考实现、Surge 手册与本仓库源码后，与上文不一致或上文没写到的，以本节为准（括号里是计划「计划期决定」的编号）。

1. **基线（新增）**：M7a 建在 M5 合并后的复查分支（272f624）之上：会话跟随 `Connector::route()`，QUIC 直连 socket 与 DIRECT 的 UDP 载体共用 `DirectConnector::bind_udp`（含 Windows `interface` 的绑定修正）（P6、P15）。
2. **配置的落点（细化 3.1）**：`QuicOpts` 只有 `port_hopping`；`alpn` 仍是 TLS 参数（QUIC 类没写时为 `h3`），`ecn` 仍在 `CommonOpts`（P1）。`port-hopping-interval` 小于 5 是 `W0012`（取值无效，按 5），不是 `W0028`；没写 `port-hopping` 时它是 `W0028`（P2）。`tfo` 与 `udp-relay` 在 `hysteria2` 上是 `W0028`，`ecn` 在 QUIC 类上不再是 `W0029`（P3）。`h3=true` 的 `trust-tunnel` 与 Shadow TLS 的 `E0018` 留给 M7c（P4）。
3. **`tos ≠ 0`（订正 3.3 与第 9 节第 1 条）**：只有发送绕开 quinn-udp（保住 DSCP，发出的包没有 ECN 标记与 GSO）；接收仍经 quinn-udp，照样读服务端的 ECN 标记、保留 PMTU 探测（P7）。
4. **UDP 层（细化 3.3）**：链式载体用占位地址、不做 PMTU 探测、放不下的包丢弃而从不报 `WouldBlock`；端口跳跃每次跳到另一个端口、端口集合去重、开不了 socket 的那次跳过；各层发送失败一律丢包、不报给 quinn（P8、P9）。
5. **HTTP/3（细化第 4 节）**：只开控制流，不开 QPACK 流、不发 GREASE；`Client` 必须与连接同生共死；响应头超限与畸形应答只结束该流，QPACK 与帧序违规关整条连接；丢掉没读完的请求以 H3_REQUEST_CANCELLED 重置。应答 `Content-Length` 的比对与 SETTINGS 0x08 / 0x33 的取值校验留给 M7c（P11）。
6. **会话（细化 3.5）**：单飞拨号在请求自己的 future 里，失败只交给等它的请求；连接只在仍开着时由我们关闭（quinn 的 `close` 会覆盖 stateless reset）；空闲关闭在 60–90 秒；"失效的连接重试一次"只对请求到来时就已就绪的连接、且请求自己的数据还没发出时（P14）。底层策略变了的连接不再接新的工作（取用时比较，不设计时器，变回也不再用），每条连接记一次日志（P15）。
7. **Hysteria 2 的认证与 TCP（细化 5.2、5.3）**：认证失败以 0x101 关闭连接；`Hysteria-UDP` 按 Go 的 `ParseBool` 读（P16）。应答状态非 0 即拒绝（上文写的是 1）；填充一律是字母与数字（上文写的是"填充字节"）；地址 ≤ 2048、消息 ≤ 2048、填充 ≤ 4096（P17）。
8. **Hysteria 2 的 UDP（订正 5.3）**：会话号按出站从 1 计、载体终生不变（上文写的是每条连接内递增）；`Hysteria-UDP: false` 或服务端不支持 QUIC 数据报时 `open_udp` 本身失败；载体在连接关闭、正在离开或底层策略变了时迁到会话当前的连接（P18）。
9. **混淆（细化 5.4）**：Salamander 丢掉 8 字节及以下的包；Gecko 的每个长头包都分片，待重组的包满 8 个时挤掉最旧的（参考实现拒绝新的）（P19）。
10. **引擎（细化 3.1 的第二条约束）**：`port-hopping` 遇上非直连的连接器在 `Dialer::new` 里拒绝，经组级 `underlying-proxy` 派生出的 `M (via R)` 因此只略去这一条（P20）。

## 15. M7a 实施期的订正

执行 M7a 计划（`docs/superpowers/plans/2026-10-02-phase2-m7a-quic-hysteria2-plan.md`）时，任务评审与全分支评审改动或补充了若干做法；与上文及第 14 节不一致处以本节为准（明细见计划的「执行期修正记录」）。

1. **同一载体的分片不交错（细化 5.3）**：每个 UDP 载体一把锁，一个包的全部分片连着交给 QUIC（锁只包住同步的发送，不跨 `.await`）。参考实现的服务端每个会话同一时刻只重组一个包号，两个包的分片交错时两个包都会丢；多线程运行时下并发发送很常见（经 Hysteria UDP 的 QUIC Initial 就有 1200 字节以上）。用例 `concurrent_sends_keep_each_datagrams_fragments_together`。
2. **UDP 至多每秒比较一次底层策略（细化第 14 节第 6 条，P15）**：请求照旧每次取用连接时比较 `Connector::route()`；UDP 载体的发送只看连接是否开着、是否正在离开、是否已被标成换了路（`Session::serves_without_route`），完整的比较距上次满 1 秒才做一次——经 `underlying-proxy` 的每个数据报都做一次（注册表读取、组解析、分配）代价太高。代价：没有请求先发现时，UDP 至多晚约 1 秒跟上底层组换的成员。用例 `the_route_is_compared_at_most_once_a_second_on_udp`、`a_carrier_finds_a_changed_route_by_itself`。
3. **服务端不支持 QUIC 数据报时的文本（订正第 14 节第 8 条，P18）**：`Hysteria-UDP: false` 是 `hysteria2: the server does not allow UDP`；服务端在 QUIC 层不支持数据报是另一条 `hysteria2: the server does not support QUIC datagrams`（P18 写的是同一条）。两种情况都是打开载体即失败，同一连接上的 TCP 照常。
4. **放弃了拨号的调用方（第 14 节第 6 条的单飞拨号）**：发起拨号的请求放弃时（它的 future 被丢掉，如超时），等着这次拨号的请求接过拨号，会话随后用它建的连接；补上用例 `a_dial_its_caller_gave_up_on_is_taken_over_by_a_waiter`（只加测试，做法不变）。
5. **组中继里带端口跳跃的成员（细化第 14 节第 10 条，P20）**：加载时对每个有 `underlying-proxy` 的组，按注册表装配的顺序算出配置本身给它的成员（行上写的、`include-other-group` 与 `include-all-proxies` 取来的，各按组的过滤器），其中用了 `port-hopping` 的各报一条 `W0023`（沿用"组不得不略去一个成员"的码），在组的那一行：`` policy group `G` relays its members through `underlying-proxy`: `M` uses `port-hopping`, which cannot go through it; `M (via R)` is left out ``；`rurge check` 由此报出。`policy-path` 带来的成员加载时不知道，仍只由注册表记日志略去。成员全被略去时组为空，按 M3-D3 由 DIRECT 代替（`--empty-group-reject` 时 REJECT）。用例 `a_relayed_group_says_which_member_with_port_hopping_it_leaves_out`、`a_hopping_member_of_a_relayed_group_is_left_out`、`check_knows_hysteria2`。
6. **HTTP/3 客户端只关闭仍开着的连接（细化第 14 节第 5 条）**：因违反 HTTP/3 关闭连接之前，除了任务记下的结束，还看 quinn 的 `close_reason()`：连接已经结束（服务端的关闭、stateless reset）而任务还没看到时不再关闭，也不记下失败——quinn 的 `close` 会把已记下的结束原因改成本地关闭。用例 `a_failure_after_the_end_leaves_how_the_connection_ended`。

## 16. M7b 计划期的订正

写 M7b 实施计划（`docs/superpowers/plans/2026-10-03-phase2-m7b-tuic-plan.md`）时核对 TUIC v5 的 `SPEC.md` 与 v4（0.8.5）的协议说明、sing-box 与 tuic-server 的源码（GPL，只读、只取协议事实）、Surge 手册与本仓库源码后，与上文不一致或上文没写到的，以本节为准（括号里是计划「计划期决定」的编号）。

1. **配置（细化 6.1）**：一个 `TuicSpec` 管两个关键字（`TuicVersion::V4 { token }` / `V5 { uuid, password }`）；UUID 也接受 32 位裸十六进制（同 vmess）；写了另一个版本的凭据是 `W0028`；写错的 TUIC 行从此是加载错误（P1、P2）。
2. **v5 的"口令错"（订正 6.2 的"能认出就报"）**：sing-box 与 tuic-server 对一切失败都以码 0 关连接；只在服务端还没转发过任何字节就关闭时才报 `tuic: authentication failed`，否则报 `tuic: the server closed the connection (code 0x…)`；经引擎时 CONNECT 先回 200、隧道随后断开（P5）。
3. **v5 的 UDP（细化 6.2）**：关联号按连接从 0 计、跳过正在用的，载体换到新连接时另取；`Dissociate` 由连接自己的任务发出；坏消息丢弃、连接照旧；口令被拒时一次 UDP 发送报认证失败（P7）。
4. **单向流由连接的写入任务写（新增）**：服务端不收数据报时，每个包交给连接的写入任务逐条写完（`OUTBOX` = 64，满了丢包），因为被取消而截短的单向流会让 sing-box 与 0.8.5 关掉整条连接；代价是这种模式下引擎的发送时限不再起作用（P9）。
5. **心跳（细化 6.2、6.3）**：v5 的心跳在有 TCP 隧道或 UDP 关联时都发、服务端不收数据报时不发；v4 的心跳走单向流、不论服务端收不收数据报（P10）。
6. **v4（细化 6.3）**：两个版本共用一套实现；token 是原文 UTF-8 的 BLAKE3；关闭码 `0xfffffff1` / `0xfffffff2` 的错误种类是 `PermissionDenied`；放不进一个数据报的包被拒而不改走单向流；关联号是随机 `u32`（P11）。
7. **IPv4 映射的来源（新增）**：回包来源里 IPv4 映射的 IPv6 地址按 IPv4 读，v4 与 v5 都是（P12）。
8. **互操作（细化 6.4 / M7-D6）**：tuic 0.8.5 只经 `RURGE_TEST_TUIC_SERVER` 找，配置必须写 `alpn: ["h3"]`；两个参考服务端都总收数据报，`quic` 模式只靠假服务端与手工验收；0.8.5 的发布资产没有公布 `digest`，CI 里的 SHA-256 先留占位，由项目所有者决定怎样取得（P14、P15）。
