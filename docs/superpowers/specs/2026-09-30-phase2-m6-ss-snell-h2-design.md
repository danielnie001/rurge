# 阶段 2 / M6「Shadowsocks / Snell / HTTP/2 族」细化设计

细化阶段 2 总设计（`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`）第 1.4 节的 M6 里程碑、第 6 节的 `ss` / `snell` / `h2-connect` / `trust-tunnel` 四行与第 13 节 M6 的互操作参考；与总设计不一致处以本文为准。开放问题 Q7（Snell 各版本可依据的公开资料）在本文第 4 节关闭。

## 1. 目标与范围

四种出站协议的 TCP 与（支持的）UDP：

| 协议 | 本里程碑 | 延后 |
| ---- | -------- | ---- |
| `ss` | AEAD（`aes-128-gcm` `aes-192-gcm` `aes-256-gcm` `chacha20-ietf-poly1305` `xchacha20-ietf-poly1305`）、`none`、SS 2022（`2022-blake3-aes-128-gcm` `2022-blake3-aes-256-gcm`，含 `serverKey:userKey` 多用户）、`obfs`（`http` / `tls`）、`udp-relay`、`udp-port` | 流式旧方法（M8） |
| `snell` | v4 / v5 的 TCP、`reuse`、UDP over TCP、`obfs=http` | v1–v3（M8，需要时）、v5 的 QUIC Proxy Mode（QUIC 族之后另议）、v6 |
| `h2-connect` | HTTP/2 CONNECT 多路复用、`max-streams`、`headers`、`udp-relay`（CONNECT-UDP，RFC 9298 over RFC 8441） | — |
| `trust-tunnel` | HTTP/2 模式的 TCP（CONNECT + Basic 认证）、`headers`、`max-streams` | `h3=true`（M7） |

对应需求：FR-OUT-05 / FR-OUT-07。验收：四种协议对参考实现转发通过（总设计第 1.4 节）；支持 UDP 的协议 UDP 转发通过。

## 2. 已决事项

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| M6-D1 | 范围与顺序 | 四种协议都做（项目所有者都有真实节点，可手工验收）；拆成三份计划，顺序 M6a Shadowsocks → M6b Snell → M6c HTTP/2 族（项目所有者，2026-09-30）。obfs 层随 M6a 落地、M6b 复用 |
| M6-D2 | Snell 的版本 | 只实现 v4 / v5 的 TCP 线上格式（项目所有者的节点是 v5；v5 的 TCP 格式与 v4 相同、服务端向后兼容）。`version` 1–3 与 6 只解析：加载时 `W0007`、运行时 REJECT（与 vmess 旧握手相同）。Surge 里 `version` 缺省为 1，所以没写 `version` 的行按 v1 拒绝并说明（项目所有者，2026-09-30） |
| M6-D3 | 参考实现的许可 | Snell 的公开实现（missuo/opensnell、SagerNet/sing-snell）与 simple-obfs 都是 GPL：只取协议事实，不抄代码；模板与编解码自写 |
| M6-D4 | crate 布局 | 四种协议都在 `rurge-proto`（总设计第 6 节）；新增依赖只有 `blake3`（SS 2022）。`h2` 0.4.19、`aes-gcm`、`chacha20poly1305`（含 `XChaCha20Poly1305`）、`hkdf`、`sha1`、`md-5`、`argon2`、`aes` 已在依赖树里 |
| M6-D5 | 互操作参考 | shadowsocks-rust v1.25.0（三平台预编译）与 sing-box：`ss`；sing-box 的 `snell` 入站（1.14.0 起，只支持 v5 / v6）：`snell`，三平台；官方 snell-server v5.0.1：只有 Linux 版，只在 Linux CI 跑；sing-box 的 `http` 入站（HTTP/2 over TLS）：`h2-connect` 的普通 CONNECT；TrustTunnel endpoint v1.1.0（Apache-2.0）：只有 Linux / macOS 版，只在这两个平台的 CI 跑；CONNECT-UDP over HTTP/2 与 obfs 没有可用的预编译参考服务端，只有回环假服务端与手工验收 |
| M6-D6 | sing-box 的固定版本 | 从 1.14.1 升到 1.14.x 中有 `snell` 入站的最新版（写 M6b 计划时核对，互操作夹具与 CI 同步改） |
| M6-D7 | v5 的动态帧大小 | 只影响发送方怎么切帧：rurge 发送时 v4 / v5 都用固定上限，接收方照常；差异登记 |
| M6-D8 | `h2-connect` 的 UDP | 每个目标一条 CONNECT-UDP 流（RFC 9298 的语义就是一条流一个目标），NAT 类型为对称型；与 vmess 一样登记为差异 |

## 3. M6a：Shadowsocks

### 3.1 配置（`rurge-config::spec`）

- `SsSpec`：`method`（枚举）、`password: Secret<String>`、`udp_relay`、`udp_port: Option<u16>`、`obfs: Option<ObfsOpts>`。
- `encrypt-method` 必填；不认识的方法 `E0018`。流式旧方法（`rc4` `rc4-md5` `aes-128/192/256-cfb` `aes-128/192/256-ctr` `salsa20` `chacha20` `chacha20-ietf` 等）只解析：`W0007`（每次加载一条）+ 运行时 REJECT，会话日志 `policy protocol not implemented: ss (<method>)`。
- `password` 除 `none` 外必填（`E0018`）。2022 方法：按冒号拆成若干段，每段是 Base64 密钥、长度与方法相符（16 / 32 字节）；不合法 `E0018`，错误文本不引用取值。最后一段是用户密钥，前面的是逐层的身份密钥（SIP023）。
- `ObfsOpts`（与 Snell 共用）：`mode`（`http` / `tls`）、`host`（缺省服务器主机名）、`uri`（缺省 `/`，只对 `http` 有意义，`tls` 时写了 `W0028`）。缺省值手册没写，是 rurge 的决定（写计划时再核对手册，V1）。

### 3.2 obfs 层（`rurge_proto::transport::obfs`）

传输阶梯变为 connect → shadow-tls → **obfs** → 协议。obfs 是字节流包装：

- `http`：客户端第一个包是 `GET <uri> HTTP/1.1` 请求头（`Host: <obfs-host>`、`Upgrade: websocket`、`Connection: Upgrade`、随机 `Sec-WebSocket-Key`、`Content-Length: <首段负载长度>`）后紧跟首段负载；读到服务端响应头（`101`）的结尾后，两个方向原样透传。
- `tls`：客户端第一个包是伪造的 TLS ClientHello，首段负载放在 session_ticket 扩展里，SNI 为 `obfs-host`；服务端回伪 ServerHello + ChangeCipherSpec + 一个携带首段响应的握手记录；此后每段数据两个方向都包成 `17 03 03 <len>` 记录。
- 模板字节自写（M6-D3）；精确布局写计划时核对（V3）。

### 3.3 协议（`rurge_proto::shadowsocks`）

**AEAD（旧式）**：主密钥 `EVP_BytesToKey(MD5)`（口令 → 密钥）；每个方向一个随机 salt（长度同密钥），子密钥 HKDF-SHA1（info `ss-subkey`）；流是"加密的 2 字节长度 + 加密的负载"的块，负载每块最多 0x3FFF，nonce 是每次 AEAD 操作递增的计数。请求头是 SOCKS5 地址，与首段负载合并写出（沿用 `LazyHead`：客户端 100 ms 内不发数据时单独发出请求头）。`none`：只写地址、不加密。

**SS 2022（SIP022）**：子密钥 = BLAKE3 `derive_key("shadowsocks 2022 session subkey", PSK ‖ salt)`；负载每块最多 0xFFFF；请求 = salt、（多用户时）身份头、固定长度头块（类型 0、8 字节时间戳、变长头长度）、变长头块（地址、2 字节填充长度 + 0–900 字节随机填充、首段负载）；响应 = salt、固定长度头块（类型 1、时间戳、请求 salt、长度）。客户端校验：类型为 1、时间戳与本机相差不超过 30 秒、回显的请求 salt 与自己的一致；任一不符以 `ss: …` 失败（文本不带密钥与 salt）。身份头（SIP023）：每一层 16 字节，由下一层 PSK 的哈希经 `derive_key("shadowsocks 2022 identity subkey", iPSK ‖ salt)` 的子密钥 AES 单块加密。

**UDP**：必须写 `udp-relay=true`；发往 `udp-port`（缺省主端口）；全锥 `PacketSocket`。AEAD：每个包一个随机 salt、全零 nonce，包体是地址 + 负载。2022：每个载体一个随机 session id，包号递增；16 字节分离头（session id + 包号）以 PSK 做 AES 单块加密，包体用 AEAD（密钥由 PSK 与 session id 派生）；回包校验类型、时间戳、回显的客户端 session id，按服务端 session 维护包号滑动窗口防重放。精确的 nonce 与窗口写计划时对照 SIP022 原文（V2）。经 `underlying-proxy` 时走链上的 UDP（M5c 的 `packet_datagram`）。

**错误**：Shadowsocks 在连接期没有鉴权应答；口令错时服务端通常一直不回或直接关闭。第一次读到 EOF 时是 `ss: the server closed the connection without answering`（口令错与服务端问题分辨不出，登记）。

### 3.4 测试

- 第 1 层：KDF、分块、2022 头的编解码往返，按任意切片方式喂入；对照参考实现抓取的字节 fixture（来源写计划时定，带出处说明）。
- 第 2 层：`rurge_proto::testing::FakeShadowsocks`——服务端独立实现（AEAD、2022 含 EIH、UDP、obfs 两种）；经引擎的 TCP 与 UDP 端到端用例。
- 第 3 层：shadowsocks-rust 与 sing-box 的 `shadowsocks` 入站：AEAD、2022、多用户、UDP（三平台 CI）。

## 4. M6b：Snell v4 / v5

### 4.1 公开资料（关闭总设计 Q7）

v4 / v5 的 TCP 线上格式由 missuo/opensnell（GPLv3，对官方 snell-server v5.0.1 做过 TCP、UDP、reuse 的互通验证）与 SagerNet/sing-snell（GPL）公开描述；sing-box 1.14.0 起的 `snell` 入站实现 v5 / v6。v1–v3 另有 icpz/open-snell。v6 细节未公开（只有逆向描述），不做。

### 4.2 配置

- `SnellSpec`：`psk: Secret<String>`（必填，`E0018`）、`version`（1–6，缺省 1；超出范围 `E0018`）、`reuse`（缺省 false）、`obfs: Option<ObfsOpts>`、`udp_port`。
- `version` 4、5 实现；1–3 与 6 只解析：`W0007` + REJECT（M6-D2），会话日志 `policy protocol not implemented: snell v<n>`，告警提示 `version must match the server; rurge supports 4 and 5`。
- `obfs`：v4 / v5 只允许 `http`（手册），`tls` 为 `E0018`。`mode` 只对 v6 有意义，其它版本写了 `W0028`。`reuse` 对 v4 / v5 生效。

### 4.3 协议（`rurge_proto::snell`）

- **密钥**：每个方向一个 16 字节随机 salt；Argon2id(psk, salt, t=3, m=8 KiB, p=1) 得 32 字节，取前 16 字节作 AES-128-GCM 密钥；nonce 是 12 字节小端计数。每次握手都要算一次 Argon2id：放在 `spawn_blocking` 里，不阻塞运行时。
- **帧**：加密的 7 字节头（类型、2 字节填充长度、2 字节负载长度）→ 填充 → 加密的负载；填充与负载密文交错（公开描述："填充区偶数下标的字节与负载密文开头的字节交换"），精确语义写计划时核对（V5）、用互操作钉死。负载每帧最多 0x3FFF；空负载帧表示本方向结束（半关闭）。
- **请求**：`[0x01, 命令, client-id 长度, client-id, 地址]`；命令 Connect 0x01、ConnectV2 0x05（`reuse=true` 时）、UDP 0x06。应答 Tunnel 0x00 / Error 0x02 + 文本：`snell: the server refused: <文本>`（文本限长，不带 psk）。请求头与首段负载合并写出（`LazyHead`）。
- **reuse**：一条流结束（双方都发过空帧）后，连接回到每个出站自己的池，下一个请求在它上面发 ConnectV2；空闲 60 秒回收（与 anytls 池一致）；服务端关闭或出错的连接不回池。
- **UDP**：v4 / v5 自动支持；发往 `udp-port`（缺省主端口）；一条命令 0x06 的 Snell 流承载，每个数据报带目标地址，回包带来源地址，全锥；沿用 M5b 的 `stream_udp`，新增 `Framing::Snell`。字节布局写计划时核对（V5）。
- **obfs=http**：复用第 3.2 节的 obfs 层；Snell 的 http obfs 是否与 simple-obfs 的完全相同写计划时核对（V4），不同则作为同一层的一个变体。

### 4.4 测试

- 第 1 层：Argon2id 对标准向量；帧编解码往返（任意切片喂入）；请求头与 UDP 包格式。
- 第 2 层：`rurge_proto::testing::FakeSnell`——服务端独立实现 TCP、ConnectV2 复用、UDP、obfs http；经引擎的用例。
- 第 3 层：sing-box `snell` 入站（`version: 5`，含 `obfs_mode: http`），三平台 CI；官方 snell-server v5.0.1，只在 Linux CI。两者互相印证帧格式。

## 5. M6c：HTTP/2 族

### 5.1 配置

- `h2-connect`：`headers`、`max-streams`（缺省 3，至少 1）、`udp-relay`、TLS 参数（M1 的 TLS 层）。凭据：手册的 `h2-connect` 语法没列 `username` / `password`，兼容性清单写的是支持——写计划时核对手册（V1），手册没有就 `W0028` 并忽略。
- `trust-tunnel`：`username` / `password`（必填，`E0018`）、`headers`、`max-streams`、TLS 参数；`h3=true` 解析并 `W0029`（M7 生效），在那之前按 h2 连。
- ALPN 固定 `h2`；服务端没协商出 h2 时失败：`<协议>: the server does not speak HTTP/2`。

### 5.2 共用的会话池（`rurge_proto::h2pool`）

每个出站一个池，池里是若干条 TLS + HTTP/2 连接（可叠 `underlying-proxy`）。开流时找一条正在承载的流少于 `max-streams`、没收到 GOAWAY 的连接，没有就新建一条（同时进来的请求共用一次握手）；没有流的连接空闲 60 秒关闭；收到 GOAWAY 或出错的连接不再分配新流，现有流走完。每条流包装成字节流：写时按对方的流控窗口分段、等窗口时让出；读时读到数据即释放流控容量；本方向结束发 END_STREAM（真正的半关闭）。

### 5.3 `h2-connect`

- **TCP**：`:method CONNECT`、`:authority host:port`，加 `headers`，有凭据时加 `proxy-authorization: Basic`；2xx 后流即字节流；407 是 `h2-connect: proxy authentication required`，其它状态 `h2-connect: the proxy answered <状态码>`。
- **UDP（`udp-relay=true`）**：开流前要求服务端的 `SETTINGS_ENABLE_CONNECT_PROTOCOL=1`，否则载体打不开（`h2-connect: the server does not support extended CONNECT`）；每个目标一条流（M6-D8）：`:protocol connect-udp`、`:scheme https`、`:path /.well-known/masque/udp/{host}/{port}/`、`capsule-protocol: ?1`；数据报在 DATAGRAM capsule（类型 0、context id 0）里，其它 capsule 丢弃；每目标的流空闲 60 秒关闭（同 M5b 的 vmess）。

### 5.4 `trust-tunnel`

同一套 CONNECT + Basic 认证 + `headers` + `user-agent`；407 是 `trust-tunnel: authentication failed`。不载 UDP（`udp()` 为 `Unsupported`，交给 `udp-policy-not-supported-behaviour`）；不实现 `_udp2` / `_icmp` / `_check`（Surge 也不用）。测速按 URL 测速。

### 5.5 测试

- 第 1 层：capsule 与 varint 编解码；masque 路径的编码（IPv6 与名字）；`headers` 占位符。
- 第 2 层：`rurge_proto::testing::FakeH2Proxy`（基于 `h2`、开 `enable_connect_protocol`）：CONNECT、Basic 认证与 407、`max-streams` 满了开新连接、GOAWAY、CONNECT-UDP 的 capsule、没开 extended CONNECT 时的失败；经引擎的 TCP 与 UDP 用例。
- 第 3 层：sing-box `http` 入站（HTTP/2 over TLS）——`h2-connect` 的普通 CONNECT，三平台；TrustTunnel endpoint——只在 Linux / macOS CI。

## 6. 跨里程碑的部分

- **能力表**：M6a 后 `ss` 不再是 `W0007`（流式方法除外）；M6b 后 `snell` v4 / v5；M6c 后 `h2-connect`、`trust-tunnel`（h2）。
- **重载**：新 spec 的指纹含全部参数（含 `Secret` 的取值）；Snell 的复用池与 h2 会话池随出站对象，参数没变的策略重载后沿用。
- **脱敏**：`psk`、`password` 已在名单里；`/v1/profiles/current` 的脱敏名单增加 `obfs-host`（伪装域名可能是用户的指纹）。
- **订阅**：四种协议的订阅导入行照常生效，不引用 Keystore 或本机程序，不需要新的订阅安全门。
- **日志与错误**：`psk`、`password`、2022 密钥、`proxy-authorization` 的值与 `headers` 的值不进日志、错误文本与 `Debug`；载荷不进日志。
- **锁**：池与会话的锁里只做内存操作，不跨 `.await`。
- **测试约束**：只用回环；不改本机网络与代理设置；不在本机安装参考二进制（缺了就跳过，CI 设 `RURGE_INTEROP_REQUIRED=1`）。

## 7. 需登记进兼容性清单的差异

- `ss` 流式旧方法到 M8 才实现（`W0007` + REJECT）。
- `ss` 口令错在连接期无法识别。
- `obfs-host` / `obfs-uri` 的缺省值（手册未写）。
- `snell` 只实现 v4 / v5；`version` 缺省为 1，因此没写 `version` 的行被拒绝；v5 的 QUIC Proxy Mode 与动态帧大小不实现。
- `h2-connect` 的 UDP 是对称型（每个目标一条流）。
- `trust-tunnel` 的 `h3=true` 在 M7 之前按 h2 连。

## 8. 风险

| 风险 | 影响 | 应对 |
| ---- | ---- | ---- |
| Snell 帧交错与 UDP 布局只有 GPL 实现与第三方描述可依据 | 自洽而与真实服务端不通 | 以 sing-box 与官方 snell-server 的互操作为最终裁判；本机没有二进制，CI 首跑证明 |
| obfs 没有可用的预编译参考服务端（simple-obfs 已停止维护，sing-box 不带插件） | obfs 的兼容性只能靠手工验收 | 模板照协议事实自写；Snell 的 http obfs 由 sing-box 的 `obfs_mode: http` 覆盖 |
| CONNECT-UDP over HTTP/2 没有可用的参考服务端 | 与真实服务端的兼容性 | 只有假服务端与手工验收；严格按 RFC 9298 / 9297 / 8441 |
| 官方 snell-server 与 TrustTunnel endpoint 只有部分平台 | 这两项互操作只在部分 CI 平台跑 | 另有 sing-box（Snell）与假服务端覆盖三平台 |
| 每次 Snell 握手一次 Argon2id | 大量短连接时的 CPU | `spawn_blocking`；`reuse=true` 可摊薄 |

## 9. 写各份计划时必须核对的事项

| 编号 | 事项 | 属于 |
| ---- | ---- | ---- |
| V1 | Surge 手册：`h2-connect` 是否接受凭据；`obfs-host` / `obfs-uri` 的缺省值；各协议参数的最新写法 | M6a / M6c |
| V2 | SS 2022 UDP：AES 包体的 nonce、防重放窗口、EIH 的哈希（SIP022 / SIP023 原文） | M6a |
| V3 | simple-obfs `http` / `tls` 的精确布局（自写模板） | M6a |
| V4 | Snell 的 http obfs 与 simple-obfs 是否相同 | M6b |
| V5 | Snell 填充交错的语义、请求的地址格式、UDP 数据报布局 | M6b |
| V6 | v5 的动态帧大小只影响发送方（M6-D7） | M6b |
| V7 | `h2` 0.4.19：客户端 extended CONNECT 的 API、流控与 GOAWAY 的处理 | M6c |
| V8 | TrustTunnel 的 `user-agent` 与 200 响应的处理 | M6c |
| V9 | 有 `snell` 入站的 sing-box 1.14.x 版本号与其 `http` 入站做 h2 CONNECT 的配置 | M6b / M6c |
| V10 | `chacha20poly1305` / `aes-gcm` / `hkdf` / `sha1` 在依赖树里对应的 `aead` / `digest` 版本，避免引入重复的大版本 | M6a |

## 10. 任务草图

**M6a Shadowsocks**

1. `SsSpec` 与 `ObfsOpts`（诊断、脱敏、能力表的流式方法分支）。
2. obfs 层（`http` / `tls`）与 `Stack` 的一层。
3. AEAD 的 TCP（含 `none`）与 `FakeShadowsocks`。
4. SS 2022 的 TCP（含 EIH）。
5. UDP（AEAD 与 2022，`udp-port`）。
6. 引擎装配、能力表翻转、经引擎的用例。
7. 互操作（shadowsocks-rust、sing-box）与文档。

**M6b Snell**

1. `SnellSpec`（版本与 obfs 的诊断）。
2. KDF 与帧。
3. TCP 与复用池、`FakeSnell`。
4. UDP。
5. 引擎装配、互操作（sing-box、snell-server）与文档。

**M6c HTTP/2 族**

1. 配置（`h2-connect` / `trust-tunnel` 的 spec）。
2. `h2pool`。
3. `h2-connect` 的 TCP 与 `FakeH2Proxy`。
4. `trust-tunnel`。
5. CONNECT-UDP。
6. 引擎装配、互操作（sing-box、TrustTunnel endpoint）与文档。

## 11. 需同步的文档

- 总设计：第 6 节 `snell` 一行改为"v4 / v5"；第 13 节 M6 的参考实现按 M6-D5；开放问题 Q7 标为已决（本文第 4.1 节）。
- 兼容性清单：各协议行与 4.6 节参数行随各计划更新；第 7 节的差异。
- `docs/acceptance/phase2-manual.md`：M6a / M6b / M6c 各一节（真实节点上的 TCP 与 UDP、obfs、reuse、CONNECT-UDP）。
- README（两份）与 `CLAUDE.md`：随各计划的文档任务更新。

## 12. M6a 计划期的订正

写 M6a 实施计划（`docs/superpowers/plans/2026-09-30-phase2-m6a-shadowsocks-plan.md`）时核对规范、参考实现与本仓库源码后，与上文不一致或上文没写到的，以本节为准（括号里是计划「计划期决定」的编号）。

1. **加密 crate（P1，细化 M6-D4）**：ChaCha20 / XChaCha20-Poly1305 用依赖树里已有的 `chacha20poly1305` 0.10.1（不引入 0.11），AES-GCM 用 `aes-gcm` 0.11.1，HKDF 用 `hkdf` 0.13 + `sha1` 0.11，AES 单块用 `aes` 0.8；新依赖只有 `blake3`。
2. **"未实现"的说明（P2）**：vmess 旧握手的标志推广为 `NotImplemented`，流式方法与之共用：加载时每种方法一条 `W0007`，会话日志 `policy protocol not implemented: ss (<method>)`。
3. **2022 的密钥（P4，细化 3.1）**：解出的密钥另存进 `SsSpec.keys`；`2022-blake3-chacha20-poly1305` 不接受（手册不列）。
4. **obfs（P6、P7，细化 3.2）**：`http` 的 `Host` 在端口不是 80 时带端口；伪装头随第一次非空写出去；首包最多 16384 字节；读 `tls` 时逐个解析记录头；服务端不回一个字节就关闭是普通 EOF。
5. **SS 2022 的填充（P10，订正 3.3 的"随机填充"）**：只有长度随机（1..=900，仅在没有首段负载时），内容是 0——它在加密里。
6. **应答校验（P11）**：在应答的固定长度头块处就校验类型、回显的请求 salt 与时间（正好 30 秒接受）。
7. **SS 2022 的 UDP（P12，细化 3.3）**：包号从 0 开始；窗口 8128，只在包校验通过后前移；每个载体最多记 8 个服务端 session；回包的分离头用用户密钥解；回显的客户端 session id 要校验；坏包静默丢弃。
8. **UDP 不经 Shadow TLS 与 obfs（P13）**：它们是 TCP 层；UDP 经策略的连接器走 DIRECT 或 `underlying-proxy` 的链。
9. **互操作（P15，细化 M6-D5）**：`aes-192-gcm` 与 `xchacha20-ietf-poly1305` 不在 shadowsocks-rust 的发布版里，改由 sing-box 覆盖。

