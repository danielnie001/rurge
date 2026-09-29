# 阶段 2 / M4 细化设计：WireGuard / SSH / external 出站

- 日期：2026-09-27
- 状态：项目所有者逐节确认（2026-09-27），待审阅全文
- 依据：`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`（下称"总设计"）第 1.4 节 M4 一行、第 2 节技术选型、第 4.3 / 4.4 节、第 5.1 / 5.5 节、第 6 节、第 7.3 节、第 13 节、第 15 节风险 C / G、第 16 节 D5 / D9 与 Q4 / Q5；需求文档 FR-CFG-11、FR-OUT-05 / 12 / 13；M3a 计划「延后事项」#25、M3b 计划「延后事项」#10；Surge 手册 `policies/wireguard.html`、`policies/ssh.html`、`policies/external.html`、`profile/keystore.html`（2026-09-27 读取）
- 与总设计的关系：本文细化总设计的 M4 里程碑。两者不一致处以本文为准，差异列在第 13 节。

## 1. 目标与范围

### 1.1 目标

实现三种出站：`ssh`（SSH 动态转发）、`wireguard`（用户态 WireGuard 隧道）、`external`（外部代理程序）。完成后它们不再报 `W0007`；WireGuard 能与带 `client-id` 的端点（WARP 类）握手并转发 TCP；SSH 经一条会话复用多个通道转发；外部程序按需拉起、退出后自动再拉起、rurge 退出时连同子进程一起清理。

### 1.2 范围内

| 需求 | 内容 |
| ---- | ---- |
| FR-CFG-11 | `[Keystore]` 的 `openssh-private-key` 条目被 `ssh` 的 `private-key` 引用，构建期解码 |
| FR-OUT-05 | `ssh`、`wireguard`、`external` 三种协议（TCP） |
| FR-OUT-12 | WireGuard：多 peer 最长前缀路由、握手 / 重协商 / keepalive / 过期定时器、分片重组、`client-id`、原生 RTT 探测与 URL 测试两种模式、额外 10 秒初始化、DSCP 0x88、网络变化重建的入口、本地隧道地址的 ICMP echo |
| FR-OUT-13 | 外部代理程序：按需拉起、自动再拉起、退出清理、日志写数据目录 |
| M3a 延后事项 #25 | 订阅导入的 `external` 一律跳过 |
| M3b 延后事项 #10 | 订阅行自己写的 `ssh` 的 `private-key=`、`wireguard` 的 `section-name=` 与 `client-cert` 同规则 |

### 1.3 范围外

- UDP：WireGuard 的 UDP 转发、`external` 的 `udp-relay`（M5，总设计 D5）。
- `wireguard` 经 `underlying-proxy`（加密后的 UDP 包经另一个策略转发）：需要 M5 的 UDP 载体链；M4 里按不可用处理（6.6）。
- `external` 的 `addresses`（从 TUN 路由里排除）与"外部进程的流量走 DIRECT"：阶段 3（要有 TUN 才有意义）。
- 触发 WireGuard 重建的网络变化探测：阶段 3（M4 只留入口）。
- `[Tailscale <name>]` 与 `tailscale` 策略：远期，照旧 `deferred`。

### 1.4 三份计划

| 计划 | 内容 | 新依赖 |
| ---- | ---- | ------ |
| **M4a SSH** | 新 crate `rurge-proto-ssh`；`SshSpec`；OpenSSH 私钥解码；会话、通道、主机密钥校验、空闲与保活；`FakeSsh`；能力表翻转 `ssh` | russh 0.63.3 |
| **M4b WireGuard** | 新 crate `rurge-proto-wireguard`；`[WireGuard <name>]` 类型化；`Datagram` 与 `DirectConnector::connect_udp`；协议栈、设备任务、流；隧道内 DNS；原生 RTT 探测；`FakeWgPeer`；吞吐基准；能力表翻转 `wireguard` | boringtun 0.7.1、smoltcp 0.12.0 |
| **M4c external** | `ExternalOutbound`（`rurge-proto`）；`rurge-platform` 的进程组 / Job Object；测试辅助程序；能力表翻转 `external` | 无（`nix` 已在依赖树里） |

每份计划开工前核对第 15 节里属于它的事项；WireGuard 的细节若在写 M4b 计划时仍然太多，照 M3c 的先例再补一份细化设计。

## 2. 已确认的决定

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| M4-D1 | 三部分的顺序 | SSH → WireGuard → external：先做项目所有者日常用的两种；external 不常用，放最后，订阅安全门随它一起（项目所有者，2026-09-27） |
| M4-D2 | SSH 库 | russh 0.63.3（已修复 RUSTSEC-2026-0154，MSRV 1.89）；`default-features = false`，开 `ring` 与 `rsa`；不用 aws-lc-rs（Windows 上编译要 cmake 与 NASM），不开 `des`、`dsa`。代价：带进第二代 RustCrypto（digest 0.11 等），`ssh-key 0.7.0-rc`、`rsa 0.10.0-rc` 是预发布版（由 russh 精确钉住）（项目所有者，2026-09-27） |
| M4-D3 | 用户态协议栈 | smoltcp 0.12.0（MSRV 1.80）；0.13 起要求 Rust 1.91，项目 MSRV 保持 1.89；接口收窄在 `rurge-proto-wireguard` 内，阶段 3 做 TUN 时复核（项目所有者，2026-09-27） |
| M4-D4 | WireGuard 协议 | boringtun 0.7.1，不开默认特性，只用 sans-IO 的 `noise::Tunn`（握手、加解密、定时器）；BSD-3-Clause，2026-05 仍在发布 |
| M4-D5 | 协议栈怎样接入 tokio | 方案 A：共享锁 + waker（第 6.1 节）（项目所有者，2026-09-27） |
| M4-D6 | Windows 上停止 external 的进程树 | Job Object（关闭即杀掉全部），rurge 正常退出或崩溃都不留孤儿；`rurge-platform` 里第二个 `#[allow(unsafe_code)]` 例外（项目所有者，2026-09-27） |
| M4-D7 | TCP 与 UDP | 只做 TCP；`wireguard` 带 `underlying-proxy` 时加载告警、拨号 REJECT 附说明，不静默改走直连（第 6.6 节） |
| M4-D8 | 订阅安全门 | 订阅导入的 `external` 一律跳过；订阅行自己的 `ssh private-key=`、`wireguard section-name=` 只认 `external-policy-modifier` 设上的值（第 4.8 节） |
| M4-D9 | WireGuard 没有 `dns-server` 时的域名目标 | 用 rurge 平常的解析器在本机解析（含 `[Host]`）；手册只说"一般不能经这个策略解析"，登记为差异 |
| M4-D10 | SSH 的空闲与保活 | "空闲"= 会话上没有打开的通道；会话存在期间每 30 秒 keepalive，连续 3 次无回应判定断开（手册没写，rurge 自定） |
| M4-D11 | external 的拉起 | 进程退出后下次用到时再拉起（照手册）；同一策略两次拉起至少间隔 2 秒（rurge 自加）；子进程环境去掉 `HTTP(S)_PROXY` / `ALL_PROXY` 并设 `NO_PROXY=*` |
| M4-D12 | 脱敏 | `external` 的 `args` 进内联参数的脱敏名单 |
| M4-D13 | 不支持的私钥 | 带口令的 OpenSSH 私钥（手册里 `password` 只用于 p12）与 DSA 私钥不支持，构建期 `E0022`，登记为差异 |

M4-D7 ～ D13 是按 M4-D1 ～ D6 逐节细化时由项目所有者确认的（2026-09-27）。

## 3. crate 改动一览

| crate | 改动 |
| ----- | ---- |
| `rurge-config` | `[WireGuard <name>]` 从 `deferred` 转为类型化节 `WireGuardSection`（新错误码 `E0023`）；`SpecEnv` 能查到这些节；`SshSpec` / `WireGuardSpec` / `ExternalSpec` 与 `to_spec` 的三个分支；`args` 进脱敏名单 |
| `rurge-net` | `Datagram` trait 与 `BoxedDatagram`；`Connector::connect_udp`（默认返回"不支持"）；`DirectConnector::connect_udp` |
| `rurge-proto` | `Outbound` 加可选的原生探测方法；`ExternalOutbound` 与注入的进程控制 trait；复用 SOCKS5 客户端 |
| `rurge-proto-ssh`（新） | `SshOutbound`；私钥解码与主机密钥比对；`testing::FakeSsh` |
| `rurge-proto-wireguard`（新） | `WireGuardOutbound`；协议栈、设备任务、流；路由表、`client-id`；隧道内 DNS；原生探测；`testing::FakeWgPeer` |
| `rurge-policy` | 测速配置的两种模式（URL / 原生）；`wireguard` 的超时另加 10 秒；`TestBook` 调用原生探测；订阅安全门 |
| `rurge-engine` | 工厂的三个分支；DNS 会话的防环扩展到 endpoint 写成域名的 `wireguard`；退出流程显式停掉外部进程；原生探测的测试会话；外部进程日志的目录 |
| `rurge-platform` | `process` 模块：Unix 进程组（`nix` 的 `killpg`），Windows Job Object（第二个 unsafe 例外） |
| `rurge`（bin） | 进程控制的适配器（同 `PlatformSockets` 的做法）；能力表三次翻转 |
| `rurge-api` | 无（`policies/detail` 的脱敏随 `rurge-config`） |

依赖方向：`rurge-proto-{ssh,wireguard} → rurge-proto → rurge-net → rurge-config`；`rurge-engine` 依赖两个新 crate；`rurge-policy` 不依赖任何协议实现；`rurge-platform` 不依赖内部 crate。`tests/interop` 照旧只用于测试。

## 4. 配置层（`rurge-config`）

### 4.1 `[WireGuard <name>]`

从 `deferred` 转为类型化节 `WireGuardSection`，每个节在加载时校验（没被任何策略引用的节同样校验）：

| 键 | 取值 |
| -- | ---- |
| `private-key` | 必填；Base64 或 64 位十六进制，32 字节；存为 `Secret`，`Debug` 不打印 |
| `self-ip` / `self-ip-v6` | 至少一个；纯地址，不是前缀 |
| `dns-server` | 可选，逗号分隔：IPv4 / IPv6 地址、带端口的写法（`1.1.1.1:53`、`[2606:4700::1111]:53`）或 `system`；IPv4 组播地址与加密 DNS 的 URL 不接受 |
| `prefer-ipv6` | 布尔，默认 false |
| `mtu` | 576 ～ 1420，默认 1280 |
| `peer` | 至少一个；多行累加；每个 peer 是括号里的字段表，一行里的多个 peer 以逗号分隔 |

`peer` 的字段：`public-key`（必填，Base64 或十六进制，32 字节）、`allowed-ips`（必填，逗号分隔的 CIDR，含逗号时加引号）、`endpoint`（必填，主机与 UDP 端口，IPv6 写作 `[addr]:port`）、`preshared-key`（可选，32 字节，`Secret`）、`keepalive`（0 ～ 65535，0 表示关闭）、`client-id`（可选：`83/12/235`、3 字节十六进制、4 字符 Base64 三种写法，得到 3 个字节）。

诊断：节内的错误值、缺必填键、`section-name` 指向不存在的节 → `E0023`（新）；不认识的键 → `W0001`；节不再报 `W0016`。`[Tailscale <name>]` 照旧 `deferred`。

### 4.2 `ssh`

`SshSpec`：`username`（必填）；`password`（`Secret`）与 `private-key`（Keystore 条目名）至少一个，两个都写时先试密钥、再试口令；`idle-timeout`（秒，至少 1，默认 180）；`server-fingerprint`（按手册的写法：`算法 base64`，多个以逗号分隔，整值加引号；格式错误 → `E0018`）。`private-key` 必须指向存在的 `openssh-private-key` 条目（否则 `E0020`）。其余通用参数与其它 TCP 协议相同（`interface`、`tfo`、`ip-version`、`tos`、`underlying-proxy`、`test-url`、`test-timeout`），也可以叠 Shadow TLS；TLS 参数不适用（`refuse_tls`，同 `socks5`）。

### 4.3 `wireguard`

`WireGuardSpec`：`section-name`（必填）加上所引用节的完整内容。spec 里带着节的内容，节改了 spec 就不同，按指纹复用出站（总设计 5.5）自然失效。

- `test-url` 只接受 `http://`（否则 `E0018`）；`test-timeout` 照常。
- `interface`、`allow-other-interface`、`tfo`、`tos` 不适用 → `W0028`；`ip-version` 用于解析 endpoint；`ecn` → `W0029`（M5 生效）。
- `underlying-proxy` → `W0029`（M5 生效；第 6.6 节）。
- Shadow TLS 不能组合（M2c 已有的 `E0018`，此时真的报出）。

### 4.4 `external`

`ExternalSpec`：`exec`（必填，非空）、`args`（可重复，按出现顺序）、`local-port`（必填，1 ～ 65535）、`addresses`（可重复，只收 IP 地址，写主机名 → `E0018`；阶段 3 才起作用 → `W0029`）、`udp-relay`（→ `W0029`，M5）。

- 两个 `external` 策略写了同一个 `local-port` → `E0018`（转发会落到别人的进程上）。
- `interface`、`allow-other-interface`、`tfo`、`tos`、`ip-version`、`underlying-proxy` 对连本机端口没有意义 → `W0028`；`test-url` / `test-timeout` 照常。

### 4.5 Keystore 的 OpenSSH 私钥

在工厂构建期解码（干构建也做，`rurge check` 能报出来），失败 → `E0022`，文本不带任何材料：

- 支持 Ed25519、ECDSA（P-256 / P-384 / P-521）、RSA（签名用 rsa-sha2-256 / 512）。
- 带口令的私钥不支持：`key1 is encrypted; rurge cannot use a passphrase-protected key`（手册的 `password` 只用于 p12）。
- DSA 私钥不支持（russh 标为不安全，OpenSSH 9.8 起服务端已移除）。

### 4.6 诊断码小结

| 码 | 用途 |
| -- | ---- |
| `E0018` | 三种策略行的参数错误；`wireguard` 的 `https` `test-url`；`external` 的主机名 `addresses` 与重复 `local-port` |
| `E0020` | `ssh` 的 `private-key` 指向不存在或类型不对的 Keystore 条目 |
| `E0022` | 构建期失败（私钥解码、DSA、带口令） |
| `E0023`（新） | `[WireGuard <name>]` 节内的错误；`section-name` 指向不存在的节 |
| `W0028` | 不适用的通用参数（见 4.3、4.4） |
| `W0029` | 暂不生效：`wireguard` 的 `ecn` / `underlying-proxy`，`external` 的 `addresses` / `udp-relay` |

### 4.7 脱敏

`[WireGuard]` 的 `private-key` / `preshared-key` 已在名单里（阶段 1）。新增 `external` 的 `args`：参数里常带口令（如 `sshpass -p`），在 `profiles/current`（`sensitive=0`）、`policies/detail` 与 `lineHash` 里整体为 `***`（M4-D12）。`ssh` 的 `password`、`private-key` 已在名单里；`server-fingerprint` 是公钥，不脱敏。

### 4.8 订阅安全门（`rurge-policy` 装配）

- 订阅导入的 `external` 行一律跳过，`W0023`，说法固定：`` `external` policies are not imported from subscriptions ``（M3a #25）。
- 订阅行自己写的 `ssh` 的 `private-key=` 与 `wireguard` 的 `section-name=` 加入 `reaches_into_profile`，与 `client-cert` 同规则：只认 `external-policy-modifier` 设上的值，否则整行跳过（`W0023`，不引用取值）（M3b #10）。订阅内容里只取 `[Proxy]`，所以订阅的 `wireguard` 行只能引用主配置的节——正是要挡的情形。

## 5. SSH 出站（M4a，`rurge-proto-ssh`）

### 5.1 会话与通道

照手册，每个策略一条 SSH 会话，请求作为通道复用在上面。

- 第一次拨号时建会话；同时进来的拨号共用这一次握手（单飞）。
- 建会话：经策略自己的连接器连到服务器（`interface`、`underlying-proxy` 等都经这里）→ 配了 Shadow TLS 就先叠上 → russh `connect_stream` 握手 → 认证（先密钥、后口令）。
- 每次拨号开一个 `direct-tcpip` 通道，通道转成流交给引擎。
- 会话已断时（开通道失败且会话已关闭）：丢弃会话、重建一次、再试一次；拨号的 `ConnectOpts.timeout`（10 秒）覆盖整个过程。

### 5.2 主机密钥

- 配了 `server-fingerprint`：服务器的公钥（主机证书则取证书里的公钥）按"算法 + 公钥数据"与列表比对，不在列表里 → 握手失败。
- 没配：照 Surge 接受；每个策略每个进程只告警一次，日志只写策略名：`` ssh: policy `P` has no server-fingerprint; the server's host key is not verified ``。

### 5.3 认证

密钥来自 Keystore（构建期已解码）；RSA 按服务器通告的 `server-sig-algs` 选 rsa-sha2-256 / 512。口令认证用 `password`。认证失败的文本固定为 `ssh: authentication failed`，不带用户名与凭据。

### 5.4 空闲与保活（M4-D10）

- `idle-timeout`：会话上没有打开的通道持续这么久，主动断开会话，下次拨号重建。"空闲"按通道而不是流量算，长时间静默的连接（挂着的 WebSocket）不会被切断。
- 会话存在期间每 30 秒发一次 SSH keepalive，连续 3 次没有回应判定会话已断（russh `Config` 的 `keepalive_interval` / `keepalive_max`）。

### 5.5 算法

用 russh 的默认列表（覆盖 Surge 要求的 `curve25519-sha256` + `aes128-gcm@openssh.com`，另有 `mlkem768x25519-sha256`、`chacha20-poly1305@openssh.com` 等）；写计划时核对默认列表，去掉 SHA-1 类算法（第 15 节 V1）。

### 5.6 错误文本

握手、认证、主机密钥、开通道的失败都用固定说法，不带服务器发来的原文与凭据：`ssh: the handshake failed (<阶段>)`、`ssh: authentication failed`、`ssh: the server's host key is not one of server-fingerprint`、`ssh: the server refused the channel (<RFC 4254 的原因名>)`。TCP 层的错误沿用现有映射（I/O、超时）。

### 5.7 链式与 UDP

`ssh` 可以作为别的策略的 `underlying-proxy`（它的 `connect_tcp` 给出流），自己也可以经别的策略连出去（连接器）。不支持 UDP（照手册）。

## 6. WireGuard 出站（M4b，`rurge-proto-wireguard`）

### 6.1 结构（M4-D5，方案 A）

- **协议栈**（放在一把 `std::sync::Mutex` 里）：smoltcp 的 `Interface` 与 `SocketSet`，内存里的收、发两个包队列（实现 smoltcp 的 `Device`）；每个 peer 一个 boringtun `Tunn`；按全部 peer 的 `allowed-ips` 建的 IPv4、IPv6 两张最长前缀路由表（复用工作区的 `prefix-trie`）。**锁内只做加解密、推进协议栈、搬包等内存操作，收发 UDP 一律在锁外。**
- **设备任务**：每个 WireGuard 出站一个，持有每个 peer 的 UDP 载体：
  - 收：清掉 `client-id` 字节 → `decapsulate`（要求回写网络的包照办，直到 `Done`）→ 解出的 IP 包，核对内层源地址落在该 peer 的 `allowed-ips` 里（WireGuard 的密钥路由规则），不在则丢弃 → 进收队列 → 推进协议栈。
  - 发：协议栈吐出的包按目的地址查路由选 peer → `encapsulate` → 写上 `client-id` → 从该 peer 的载体发出。
  - 定时：每 250 ms 推进 boringtun 的定时器；smoltcp 自己的定时（重传等）按 `poll_delay`。
  - 被流的读写唤醒时立即推进一次协议栈。
- **流**：每条 TCP 连接一个 smoltcp TCP socket。`poll_read` / `poll_write` 加锁直接操作自己的缓冲；数据或空间不够就注册 waker（smoltcp 的 `async` 特性）；读写之后唤醒设备任务。关闭写端发 FIN；流被丢弃后 socket 由设备任务回收。

### 6.2 建连（`connect_tcp`）

1. 确保设备已启动（6.5）。
2. 目标是 IP：直接用。目标是域名：配了 `dns-server` → 隧道内 DNS（6.3）；没配或是 `system` → rurge 平常的解析器在本机解析（含 `[Host]`，M4-D9）。
3. 按目标的地址族选 `self-ip` / `self-ip-v6`（该族没有本端地址 → 错误）；路由表里没有覆盖目标的前缀 → 立即失败：`wireguard: no peer's allowed-ips covers <地址>`（手册：没有路由就丢弃，绝不直连兜底；对 TCP 建连直接报错比等超时清楚）。
4. 建 smoltcp TCP 连接（本端端口在本出站内分配、不冲突），等到连接建立、失败或 `ConnectOpts.timeout`。握手还没完成时，boringtun 先把包排队。

### 6.3 隧道内 DNS

- 经隧道向 `dns-server` 发 UDP 查询（smoltcp 的 UDP socket，源地址为对应族的本端地址）；A 与 AAAA 并发；报文编解码用工作区已有的 `hickory-proto`。
- 每个服务器单次等待有上限，按列表顺序换服务器，整体受拨号时限约束。
- 两族都有结果时，`prefer-ipv6` 决定先用哪个（前提是两族本端地址都配了）。
- 每个出站一个按 TTL 过期、有容量上限的小缓存（只存成功的结果）。
- DNS 服务器的地址族要有对应的本端地址（手册：IPv4 的 DNS 服务器需要 `self-ip`）；没有则跳过该服务器。

### 6.4 包级行为

- **MTU**：设给 smoltcp；超过 MTU 的包不发（手册：丢弃）。
- **分片重组**：开启 smoltcp 的 IPv4 / IPv6 分片重组（缓冲大小由特性决定，第 15 节 V4）。
- **ICMP**：只回应发给本端隧道地址的 echo 请求（smoltcp 自带）；其它 ICMP 丢弃。
- **`client-id`**：发出的每个 WireGuard 报文把第 1 ～ 3 字节（保留字段）写成 `client-id`；收到的报文先把这 3 字节清零再交给 boringtun（照手册）。
- **DSCP**：握手发起包的 DSCP 标为 0x88（AF41），其它包不标（照手册）。经载体的 `set_dscp` 在发握手包前后切换（同一个 socket 只有设备任务在发，没有竞争）。Windows 通常忽略应用设置的 DSCP，登记为平台差异。

### 6.5 生命周期

- **构建**（含干构建）：只把密钥解析成 x25519 格式，不开 socket，不解析域名。
- **按需启动**：第一次拨号或测速时启动设备任务：解析每个 peer 的 endpoint（rurge 的解析器；`ip-version` 决定优先的地址族）→ 每个 peer 经连接器建一个已连接的 UDP 载体 → 向所有 peer 各发一次握手。
- **endpoint 重新解析**：主机名写的 endpoint 每 5 分钟重新解析；地址变了就重连该载体，地址族变了就重建。
- **网络变化**：设备留一个"网络已变化"的入口（重建全部载体、重新握手）；阶段 2 没有触发它的探测器。
- **结束**：出站对象被释放（重载时删掉或改了这条策略）→ 设备任务结束，载体关闭，进行中的流读写返回错误。节的内容没变的重载按指纹沿用原来的出站与设备，不重新握手。

### 6.6 UDP 载体与 `underlying-proxy`

- `rurge-net` 定义 `Datagram` trait（收、发，以及 `set_dscp`，后者默认什么也不做）与 `BoxedDatagram`；`Connector` 加 `connect_udp`（默认返回"不支持"）；`DirectConnector` 实现它：按 `ip-version` 解析、已连接的 UDP socket、`set_dscp` 经平台 socket 函数设置 TOS / TCLASS。
- M5 再给 `ChainConnector` 与 `Outbound` 加 UDP。因此 `wireguard` 带 `underlying-proxy` 时，设备建不起载体：`connect_tcp` 返回 `OutboundError::Unsupported("wireguard over underlying-proxy")`，引擎照现有处理——REJECT，附说明 `policy protocol not implemented: wireguard over underlying-proxy`，不重试；加载时另有 `W0029`（M4-D7）。

### 6.7 测速

- **模式**由配置静态决定：没有 `dns-server` 也没写 `test-url` → 原生 RTT 探测；否则标准的两次 HEAD 测试（经 `connect_tcp`）。
- **原生探测**：启动或唤醒设备，向所有 peer 强制握手，第一个有效握手回应的往返时间就是结果（多个 peer 时最快者胜）。它只证明 peer 可达、握手成功，不证明路由与出口（照手册）。
- **超时**：两种模式都在 `test-timeout` 之外另加 10 秒，留给首次初始化（照手册）。
- 实现：`Outbound` 加可选的原生探测方法（默认没有）；测速配置有 URL 与原生两种模式；`TestBook` 对原生模式调用它；结果照常存进 `TestBook`、推给 `SmartBook`；测试会话照常进请求记录，目标记为第一个 peer 的 endpoint（`host:port`）。

## 7. external 出站（M4c，`rurge-proto`）

### 7.1 拉起与转发

- 第一次用到这个策略时拉起：`exec` 加 `args`（按原顺序），标准输入为空；标准输出与错误追加写入 `<数据目录>/external/<策略名>.log`（策略名里不能出现在文件名里的字符替换掉），每次拉起先写一行分隔；超过 1 MiB 时在拉起前轮转一次，只留一个旧文件。
- 子进程环境去掉 `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY`（大小写两种）并设 `NO_PROXY=*`，防止它按代理设置绕回 rurge（M4-D11）。
- 转发：用现有的 SOCKS5 客户端连 `127.0.0.1:<local-port>`。连接被拒时每 500 ms 重试，一个请求最多 6 次（照手册）；其余错误照 SOCKS5 出站的映射。

### 7.2 再拉起

- 进程退出后，下次用到时再拉起（照手册）。
- 同一策略两次拉起至少间隔 2 秒：程序一启动就崩时，不会每个请求都去拉起一次；间隔内的请求仍按 7.1 的重试等待（M4-D11）。

### 7.3 清理

- rurge 正常退出（Ctrl-C、`rurge stop`、服务停止）：退出流程先停掉全部外部进程再退出，不指望进程退出时的析构（第 8.2 节）。
- **Unix**：子进程放进单独的进程组（tokio 的 `process_group`），停止时整组 SIGTERM，2 秒后仍在就 SIGKILL（`nix` 的安全接口 `killpg`；`nix` 已因 boringtun 进了依赖树，只多开特性，不新增 crate）。rurge 崩溃后留下的孤儿进程不处理（延后事项）。
- **Windows**：子进程加入一个设了"关闭即杀掉全部"（`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`）的 Job Object；停止时关闭 Job，整棵进程树被系统结束；rurge 崩溃时 Job 句柄随进程关闭，同样不留孤儿（M4-D6）。
- 平台实现放在 `rurge-platform` 的 `process` 模块，经 bin 的适配器注入 `rurge-proto`（同 `PlatformSockets`）。

### 7.4 重载

按指纹复用：`exec`、`args`、`local-port` 都没变，沿用原来的出站与进程；变了的话，旧进程在旧出站释放时（进行中的会话结束后）停掉。

### 7.5 安全

- 订阅导入的 `external` 一律跳过（4.8）。
- 干构建、`rurge check`、`POST /v1/profiles/check` 从不拉起进程（构建只校验，拉起只在拨号时）。
- 日志只写策略名与 pid，从不写 `args`；`args` 在 API 输出里脱敏（4.7）。

## 8. 与其它部分的衔接

### 8.1 `rurge-policy`

- 测速配置（`TestSpec`）有两种模式：URL（照旧）与原生（`wireguard` 且没有 `dns-server` 也没写 `test-url`）；`wireguard` 的超时一律另加 10 秒。`test_slot` / `test_case` / `round_timeout` 与 M3b / M3c 的抽样都按新的模式处理（原生模式不需要 URL）。
- `TestBook` 对原生模式调用出站的探测方法。
- 订阅安全门（4.8）。

### 8.2 `rurge-engine`

- 工厂新增三个分支：`Ssh` → `rurge_proto_ssh::SshOutbound`；`WireGuard` → `rurge_proto_wireguard::WireGuardOutbound`（带解析器单元与直连连接器）；`External` → `rurge_proto::ExternalOutbound`（带数据目录与进程控制）。
- DNS 会话的防环：M1b 的 `socket_opener` 遇到"以域名配置的代理"时绕开；扩展到 endpoint 写成域名的 `wireguard`（DNS 查询经 WireGuard 出去、WireGuard 解析 endpoint 又要 DNS，就成了环）。
- 退出流程：优雅退出时显式停掉全部外部进程（经注册表里的出站），再结束运行时。
- 原生探测的测试会话（6.7）。

### 8.3 `rurge-platform` 与 bin

- `rurge-platform::process`：Unix 的进程组与 `killpg`；Windows 的 Job Object——第二个 `#[allow(unsafe_code)]` 例外，只限创建 Job、设限制、关联进程的那一个函数，配专门用例。
- bin：进程控制的适配器；把数据目录交给工厂；能力表三次翻转（每份计划末尾各一次）。

### 8.4 能力表

三份计划各在末尾翻转一种：`ssh`、`wireguard`、`external` 依次不再报 `W0007`。翻转前核对本文承诺的行为都已存在（M2 设计第 8 节的教训）。

## 9. 错误处理、可观测性与安全

- 错误文本都是固定说法，不带服务器原文、凭据、私钥、`args`：SSH 见 5.6；WireGuard：`wireguard: no peer's allowed-ips covers <地址>`、`wireguard: the tunnel has no <IPv4|IPv6> address`、`wireguard: dns lookup of <name> failed`、握手超时归为超时；external：`external: could not start <策略名>`（原因只取 I/O 错误的种类）、`external: the local SOCKS5 port refused the connection`。
- 日志：SSH 没配指纹的一次性告警（策略名）；WireGuard 的握手成功 / 失败（策略名与 peer 序号，不写公钥与 endpoint 之外的材料）；external 的拉起与退出（策略名、pid、退出码）。
- 锁：WireGuard 协议栈的锁内不做 I/O、不解析域名、不拨号；SSH 会话槽与 external 进程槽用 tokio 锁（跨 `.await` 等握手或拉起），只包住"建或取"本身。
- 测试只用回环地址、不碰公网；external 的测试只拉起测试自带的辅助程序；不改本机的系统代理与网络设置。

## 10. 测试策略

三层沿用总设计 D3。

| 部分 | 第 1 层（向量 / 单元） | 第 2 层（回环） | 第 3 层（互操作） |
| ---- | ---------------------- | --------------- | ----------------- |
| SSH | 指纹解析与比对、私钥解码（三种类型、带口令、DSA 的拒绝） | `FakeSsh`（russh 服务端）：口令与三种密钥认证、指纹匹配与不匹配、没配指纹只告警一次、多次拨号共用会话、服务器断开后重连、通道被拒的错误文本、空闲断开（暂停的时钟）、经引擎的端到端 | 本机有 `sshd` 时在回环端口起一个临时实例，没有则跳过 |
| WireGuard | 路由表（最长前缀、两族分表）、`client-id` 三种写法与读写、节的解析 | `FakeWgPeer`（boringtun 响应端 + 它自己的 smoltcp 协议栈，隧道地址上有 TCP 回显、极小的 HTTP 响应端，可选 DNS 响应端；可配置成要求并清除 `client-id`、黑洞、发分片包、ping 本端）：握手与回显、两个 peer 的选路、没有路由的错误、线上的 `client-id`、内层源地址检查、分片重组、ICMP 回应、隧道内 DNS、原生探测、经隧道的 URL 测试、`underlying-proxy` 时 REJECT、出站释放时设备停止、重载沿用、经引擎的端到端 | sing-box 的 WireGuard 端点（含保留字节，对应 `client-id`，总设计 Q4）；本机没有则跳过，CI 必跑 |
| external | 日志路径与轮转、拉起限速的判定 | 测试自带的辅助程序（极小的 SOCKS5 服务端）：按需拉起与转发、`args` 顺序、退出后再拉起、连接被拒时的重试、日志内容、出站释放时整个进程树被停掉（辅助程序再拉一个子进程）、干构建不拉起 | — |

另外：订阅安全门的装配用例（`rurge-policy`）；三次能力翻转的 CLI 用例；WireGuard 的吞吐基准（经回环 peer 传一大块数据，实测数字记进 M4b 计划，不作门禁；总设计风险 G）。

## 11. 验收标准

1. SSH 经 `FakeSsh` 动态转发（口令与三种密钥），主机密钥校验按 5.2；经引擎的端到端通过。
2. WireGuard 与带 `client-id` 的端点握手并转发 TCP：回环 `FakeWgPeer` 必过；对 sing-box 的互操作在 CI 上通过。
3. `external` 进程退出后下次使用时自动再拉起；rurge 退出时整个进程树被清理（Unix 进程组、Windows Job Object）。
4. `W0007` 不再因 `ssh`、`wireguard`、`external` 出现；订阅安全门有用例。
5. 门禁全绿（fmt / clippy 零警告 / `cargo test --workspace`）。
6. 需要真实环境的项目进 `docs/acceptance/phase2-manual.md` 的 M4 三节：真实 sshd（含 RSA 与 Ed25519 密钥、指纹）、自建 WireGuard 与 WARP（`client-id`）、用 `ssh -D` 当外部程序。

## 12. 兼容性清单需登记的差异

- `ssh`：DSA 私钥与带口令的 OpenSSH 私钥不支持；`idle-timeout` 的"空闲"指没有打开的通道；会话保活 30 秒 × 3；算法是 Surge 要求的超集；错误文本见 5.6。
- `wireguard`：没有 `dns-server`（或写 `system`）时目标域名在本机解析（含 `[Host]`）；`underlying-proxy` 到 M5 才生效，M4 里 REJECT 附说明；`interface` / `tfo` / `tos` 不适用、`ecn` M5 生效；Windows 通常忽略握手包的 DSCP；TCP 建连遇到没有路由立即报错；endpoint 每 5 分钟重新解析。
- `external`：三平台都支持（Surge 仅 Mac）；日志在数据目录；两次拉起至少间隔 2 秒；子进程环境去掉代理变量；`args` 脱敏；重复的 `local-port` 是错误；Windows 用 Job Object 清理进程树、Unix 用进程组；Unix 上 rurge 崩溃后的孤儿进程不处理；`addresses` 阶段 3。

## 13. 对其它文档的订正

| 文档 | 订正 |
| ---- | ---- |
| 总设计第 2 节 | SSH：russh 0.63.3，`ring` 后端；用户态协议栈：smoltcp 0.12.0（MSRV 限制）；WireGuard：boringtun 0.7.1 |
| 总设计第 5.1 节 | `Datagram` 多一个 `set_dscp`（默认无操作）；`DirectConnector::connect_udp` 在 M4 落地（原文如此），`ChainConnector::connect_udp` 在 M5 |
| 总设计第 6 节 | `wireguard` 经 `underlying-proxy` 在 M5 |
| 总设计第 16 节 | Q5 已决：russh 0.63.3 覆盖 `curve25519-sha256` + `aes128-gcm`（超集）；Q4 在 M4b 计划核对（第 15 节 V12） |
| `CLAUDE.md` | unsafe 规则的说明加上 `rurge-platform::process` 的 Job Object 例外（M4c） |

## 14. 风险

| 风险 | 影响 | 应对 |
| ---- | ---- | ---- |
| russh 的预发布依赖（`ssh-key 0.7.0-rc`、`rsa 0.10.0-rc`） | 依赖正式版之前可能有破坏性变化 | 由 russh 精确钉住；等它们出正式版后随 russh 升级 |
| RSA 的 Marvin 时序旁路（RUSTSEC-2023-0071，`rsa` 所有版本） | 影响 RSA 解密；SSH 客户端只用 RSA 签名 | 接受；手工验收推荐 Ed25519 密钥 |
| smoltcp 0.12 的 TCP 吞吐（总设计风险 G） | WireGuard 出站的速度 | 先测基准；协议栈接口收窄，以后可换 |
| boringtun 的维护节奏（总设计风险 C） | 长期维护 | 0.7 系列 2026 年仍在发布；经 trait 隔离 |
| 新增的 unsafe 例外 | 平台代码的内存安全 | 只有一个函数，配专门用例，调用参数都来自安全句柄 |
| 互操作本机验证不了（没有 sing-box 与 sshd） | 与真实实现的兼容 | CI 必跑；手工验收兜底 |
| 首次构建需要下载约百个新 crate | 构建环境要能访问 crates.io | 项目所有者已同意（M4-D2 / D3） |

## 15. 写计划时必须核对的事项

| 编号 | 事项 | 属于 |
| ---- | ---- | ---- |
| V1 | russh 0.63.3：默认 `Preferred`（kex / 加密 / MAC / 主机密钥算法）并去掉 SHA-1 类；`connect_stream` 与 `Handler`（`check_server_key` 的参数类型）；`channel_open_direct_tcpip` 的失败形态与原因码；`Channel::into_stream`；`Config` 的 `keepalive_interval` / `keepalive_max` / `inactivity_timeout` 语义；RSA 哈希的选择；服务端 API（`FakeSsh`）；`ring` + `rsa` 特性组合在 Windows、MSRV 1.89 上能编译；`Cargo.lock` 新增条目数 | M4a |
| V2 | OpenSSH 私钥解码：加密私钥怎样识别（在不给口令时报"已加密"而不是泛泛的解析错误）；DSA 怎样拒绝；指纹串（`算法 base64`）解析成公钥与主机证书取公钥的接口 | M4a |
| V3 | boringtun 0.7.1：`Tunn::new` 的参数（每个 peer 的 index、限速器）、`encapsulate` / `decapsulate` 的缓冲大小（`dst` 至少 `src + 32` 且不小于 148）、`decapsulate` 的重复调用约定、`update_timers`、`format_handshake_initiation(force)`、判断"握手已完成"（原生探测）的办法 | M4b |
| V4 | smoltcp 0.12：以内存队列实现 `Device`；`Interface` 的地址与路由配置（`medium-ip`）；TCP socket 的连接（本端端口分配）、收发、`register_recv_waker` / `register_send_waker`、关闭与回收；`poll` / `poll_delay`；ICMP echo 自动回应的条件；分片重组与缓冲大小的特性（只开需要的特性，不开默认特性）；IPv6 | M4b |
| V5 | `hickory-proto` 0.26 构造 A / AAAA 查询与解析回答的接口 | M4b |
| V6 | `Datagram` / `connect_udp` 在 `rurge-net` 的形状与 `DirectConnector` 的实现；TOS / TCLASS 的平台函数（M1a 的 `tos` 已有）在 UDP socket 上的用法 | M4b |
| V7 | DNS 会话防环（`socket_opener`）判断"以域名配置的代理"的位置，怎样把 `wireguard` 的 endpoint 纳入 | M4b |
| V8 | `TestSpec` / `TestCase` 的形状（原生模式没有 URL）与 M3b / M3c 里读它们的每一处（`test_slot`、`sample_of`、`round_timeout`、`test_results`） | M4b |
| V9 | windows-sys 0.61 的 Job Object 函数与所需特性；从 tokio `Child` 取进程句柄（安全接口）；unsafe 只包 FFI 调用 | M4c |
| V10 | `nix` 0.31 的 `signal` / `process` 特性与 `killpg`；tokio `Command::process_group` | M4c |
| V11 | 引擎的退出路径：优雅退出、Ctrl-C、服务停止各自在哪里结束运行时，确保在进程退出前停掉外部进程；`rurge run` 的退出点没有先于它的 `std::process::exit` | M4c |
| V12 | sing-box WireGuard 端点的配置写法（含保留字节，总设计 Q4）；Linux / macOS CI 上以普通用户在回环端口跑临时 `sshd` 的写法 | M4a / M4b |
| V13 | `ssh` 的 `private-key` 引用校验（`E0020`）在 M1 是否已覆盖类型检查 | M4a |
| V14 | 测试辅助程序（SOCKS5 服务端）放在哪里：只用于测试的工作区成员里的二进制目标，不进发行的二进制 | M4c |

## 16. 任务草图

**M4a SSH**

1. `rurge-config`：`SshSpec` 与 `to_spec` 分支、`server-fingerprint` 解析、`idle-timeout`；订阅行自己的 `private-key=` 进 `reaches_into_profile`。
2. `rurge-proto-ssh` 骨架：Keystore 私钥解码（三种类型、带口令与 DSA 的拒绝）、指纹比对（纯函数与向量）。
3. `SshOutbound`：建会话（连接器 → Shadow TLS → `connect_stream` → 认证）、开通道、断线重建、单飞；`testing::FakeSsh` 与用例。
4. 空闲断开与保活（暂停的时钟）；没配指纹的一次性告警。
5. 引擎：工厂分支与经引擎的端到端、经 SSH 的测速。
6. 互操作：本机 `sshd`（没有则跳过）。
7. 能力表翻转 `ssh` 与文档（兼容性清单、README、`CLAUDE.md`、手工验收的 SSH 一节、本设计与总设计的订正）。

**M4b WireGuard**

1. `rurge-config`：`WireGuardSection`（`E0023`）、`SpecEnv` 查节、`WireGuardSpec` 与参数规则；订阅行自己的 `section-name=` 进 `reaches_into_profile`。
2. `rurge-net`：`Datagram`、`Connector::connect_udp`、`DirectConnector::connect_udp` 与 `set_dscp`。
3. `rurge-proto-wireguard` 骨架：密钥、路由表、`client-id`、包头辅助（纯函数与向量）。
4. 协议栈、设备任务与流；`testing::FakeWgPeer`；握手与回显。
5. 目标解析：隧道内 DNS 与缓存、本机解析、没有路由的错误。
6. 生命周期：按需启动、endpoint 重新解析、网络变化入口、释放时停止；分片重组、ICMP 回应、内层源地址检查、DSCP。
7. 测速：原生探测、`TestSpec` 的两种模式与另加的 10 秒（`rurge-policy`）、测试会话。
8. 引擎：工厂分支、DNS 会话防环扩展、经引擎的端到端、`underlying-proxy` 的 REJECT；吞吐基准。
9. 互操作：sing-box WireGuard 端点（含保留字节）。
10. 能力表翻转 `wireguard` 与文档。

**M4c external**

1. `rurge-config`：`ExternalSpec`、重复 `local-port` 的错误、`args` 脱敏；订阅导入的 `external` 跳过。
2. `rurge-platform::process`：Unix 进程组与 `killpg`、Windows Job Object（unsafe 例外）与用例。
3. `ExternalOutbound`：拉起、日志与轮转、环境变量、再拉起与限速、连接被拒的重试；进程控制 trait；测试辅助程序与用例。
4. 引擎：工厂分支、数据目录、退出流程停掉外部进程、经引擎的端到端。
5. 能力表翻转 `external` 与文档（含 `CLAUDE.md` 的 unsafe 规则）。

## 17. 计划期的订正

写 M4a 计划（`docs/superpowers/plans/2026-09-27-phase2-m4a-ssh-plan.md`）时核对 russh 0.63.3 与本仓库源码得出、与上文不同的地方；P 编号是该计划「计划期决定」表的编号。

| 本文原文 | 计划 | 依据 |
| -------- | ---- | ---- |
| 4.5 带口令私钥的报错示例 `key1 is encrypted; rurge cannot use a passphrase-protected key` | ``keystore item `key1` is protected by a passphrase, which rurge cannot use; remove the passphrase``（P7） | 与既有 Keystore 报错同样以 "keystore item `名字`" 开头，并说明怎么办 |
| 4.5 DSA 私钥不支持（russh 标为不安全） | 不开 `dsa` 特性时 DSA 私钥照样能解码（只是签不了名）：解码后按算法只收 Ed25519 / ECDSA / RSA，其余（DSA、要硬件的 `sk-*` 安全密钥）同为 `E0022`（P7） | russh / ssh-key 源码 |
| 4.2 / V13 `private-key` 的 `E0020` | M1 的 Keystore 引用校验只针对 `client-cert`；`read_ssh` 自己查（条目不存在、条目是 p12）（P9） | `spec/tls.rs` |
| — | M2b 重载指纹里"引用的 Keystore 条目"只取 TLS 的 `client-cert`；新增 `ProtoSpec::keystore_item`，`ssh` 取 `private-key`（P10） | 否则换了私钥（名字不变）的重载沿用旧出站、仍用旧私钥 |
| 5.1 会话已断的判断"开通道失败且会话已关闭" | 开通道时除"服务器拒绝"（`ChannelOpenFailure`）以外的错误都当作会话已断（P5） | russh 在会话已关时给的是发送失败一类的错误，分不出更细 |
| 5.2 告警文本 ``ssh: policy `P` has no server-fingerprint; …``，"每个策略每个进程只告警一次" | 结构化字段 `policy` 加固定消息 `ssh: no server-fingerprint; the server's host key is not verified`；按出站对象计一次：重载时参数没变的策略沿用原出站、不再告警，参数变了而重建的出站再告警一次（P12） | 与 `registry.rs` 里 `policy cannot be built` 的写法一致；按进程记要另设一张全局表 |
| 5.3 RSA 按 `server-sig-algs` 选 rsa-sha2-256 / 512 | 服务器列了 rsa-sha2-512 用它，否则一律 rsa-sha2-256——服务器只列 `ssh-rsa` 或什么都没说时也是（P3） | russh 的 `best_supported_rsa_hash` 在服务器只列 `ssh-rsa` 时给出 SHA-1 |
| 5.5 "用 russh 的默认列表（覆盖 … `aes128-gcm@openssh.com`）" | russh 0.63.3 的默认加密列表没有 `aes128-gcm@openssh.com`，补在 `aes256-gcm` 之后；主机密钥算法去掉 `ssh-rsa`；kex 与 MAC 的默认列表本来就没有 SHA-1（P2） | `negotiation.rs` |
| 5.6 ``ssh: the handshake failed (<阶段>)`` | 只有"没有共同算法"带括号说明；其余握手失败（对端不是 SSH、握手中的 I/O 错误等）都是 `ssh: the handshake failed`；会话已断而重建后仍开不了通道时是 `ssh: the session closed`（P14） | russh 的错误分不出握手的阶段 |
| §10 SSH 第 2 层"空闲断开（暂停的时钟）" | 空闲断开用真实时钟（`idle-timeout` 取 1 秒、有界等待）；保活只断言 `session_config()` 的取值（P16） | 回环上的真实连接与暂停的时钟不能共存：运行时空闲时自动拨快时钟，russh 自己的计时随之提前到期 |
| §10 SSH 第 3 层"本机有 `sshd` 时起临时实例" | 只在 Unix 上编译与运行；非 root 的 `sshd` 只让运行它的用户登录，用例用现场生成的 Ed25519 密钥登录（P15） | Windows 上没有可用的 `sshd`；CI 在 Linux / macOS 上跑 |

## 18. M4a 实施期的订正

按计划实施时由任务评审与终审发现、与上文不同的地方；计划「执行期修正记录」里有对应的行。

| 本文原文 | 实际 | 依据 |
| -------- | ---- | ---- |
| 5.1 "同时进来的拨号共用这一次握手（单飞）" | 握手失败时，等在锁上的拨号共享这次失败（同一种错误、同一句固定文本），不各自重新登录（d95a6e0） | 任务评审：口令写错时，浏览器一次开几十个连接会变成连续几十次失败的登录，正好触发服务器的 fail2ban；共用一次握手本就包括它的结果 |
| 5.1 / 5.3 没有写失败之后的下一次拨号 | 认证失败、主机密钥不在 `server-fingerprint` 里、没有共同算法这三种失败之后退避：60 秒内的拨号不连服务器、立即得到同一个失败，接连再失败时翻倍、最长 10 分钟，只有握手成功才清零；其余失败（连不上、超时、对端不是 SSH、握手中途断开等）不退避，下一次拨号立即重试；认证得到"未接受"而会话已经关闭时算握手失败（`ssh: the handshake failed`），不算认证失败；退避记在出站对象上，重载时重建的出站从头开始，沿用的出站连同退避一起沿用 | 终审：rurge 做系统代理而口令写错（或服务器上改了口令）时，每次打开网页、每批后台请求都多一次失败的登录，几批之内 fail2ban 的默认规则（OpenSSH 9.8 起还有默认开启的 `PerSourcePenalties`）就会封掉用户自己的 IP，连同他自己的 ssh；russh 在会话任务结束时也把登录报成"未接受" |
| 5.2 "服务器的公钥（主机证书则取证书里的公钥）按'算法 + 公钥数据'与列表比对" | 另外，配了 `server-fingerprint` 时，钉住的公钥的算法在协商时排在最前（按书写顺序；`ssh-rsa` 公钥对应 rsa-sha2-512、rsa-sha2-256，从不用 `ssh-rsa`；不在 5.5 列表里的算法不起作用），其余照 5.5 的顺序；rurge 不通告主机证书算法，服务器出示的都是普通公钥 | 终审：russh 按自己列表的顺序取服务器也支持的第一个算法，默认 Ed25519 在前；同时有 RSA、ECDSA、Ed25519 主机密钥的 OpenSSH 服务器因此总是出示 Ed25519 那把，只钉它的 ECDSA 或 RSA 公钥时每次握手都失败 |
| 5.4 保活用 russh `Config` 的 `keepalive_interval` / `keepalive_max`；计划 P6 不用 `inactivity_timeout` | `inactivity_timeout` 设为 5 分钟（它给每次写出限时：服务器不再收数据的连接 5 分钟内结束）；登录（密钥交换加认证）最多 20 秒，拨号放弃或到时 rurge 立即关掉这条连接 | 终审与合并后的修正：russh 0.63.3 在登录之前到点的保活计时器不再重置，会话任务会反复空转；而密钥交换期间它不看 `Handle`，拨号放弃后也不会自己结束。现在交给 russh 的连接在密钥交换期间可被放弃（读取立即失败，任务随之结束），登录又在第一次保活（30 秒）之前结束，未登录的会话不会待到保活到点。已建立的会话上每次保活应答都会重置 `inactivity_timeout`，"空闲 = 没有打开的通道"不变 |
| — | 服务器向 rurge 开的通道（`forwarded-tcpip`、`forwarded-streamlocal`、agent 转发、`session`、`direct-tcpip`、`direct-streamlocal`、`x11`）一律以 administratively prohibited 拒绝，与 OpenSSH 相同 | 终审：rurge 不请求任何转发，russh 的客户端却默认接受这些通道；不怀好意的服务器（比如第三方订阅里的）能让 rurge 为每个这样的通道分配状态 |
| 5.3 "口令认证用 `password`" | 口令只经 SSH 的 `password` 方法发送，不走 `keyboard-interactive`：只经 `keyboard-interactive` 收口令的服务器（FreeBSD 的默认配置、部分 PAM 配置）登录失败（`ssh: authentication failed`）；已登记为差异 | 终审 |

## 19. M4b 计划期的订正

写 M4b 计划（`docs/superpowers/plans/2026-09-28-phase2-m4b-wireguard-plan.md`）时核对 boringtun 0.7.1、smoltcp 0.12.0、hickory-proto 与本仓库源码，并把全部任务在仓库副本上真实做过一遍之后，与上文不同的地方；P 编号是该计划「计划期决定」表的编号。

| 本文原文 | 计划 | 依据 |
| -------- | ---- | ---- |
| 6.6 `Datagram` 有收、发与 `set_dscp` | 收发是 `poll_send` / `poll_recv`（一个任务同时等几个载体）；`set_tos` 取 TOS 字节（0x88 即 DSCP AF41），0 回到策略自己的 `tos`；另有 `peer_addr`；`DirectConnector::connect_udp` 取解析结果的第一个地址（UDP 没有"连上"可比），收发缓冲尽量设为 7 MiB（P5） | 回环基准：Windows 默认的 64 KiB 收缓冲装不下一个 TCP 窗口的突发，丢包后 smoltcp 整窗重发 |
| 6.1 "设备任务：每个 WireGuard 出站一个" | 每个节与载体设置（`ip-version`、`underlying-proxy`、`[General] ipv6`）一条隧道，两者都相同的策略共用；一条隧道启动时结束同一私钥、有共同 peer、来自更早配置的那条，更早配置的策略不能再把它抢回去；启动不排队：先在锁外拨好载体，再在隧道表的一次临界区里决定共用、拒绝或接替，被接替的隧道不再发送任何报文（P10；载体设置与不排队是实施时的修正） | 同一私钥连着同一 peer 的两条隧道会互相抢 peer 记住的地址（peer 回应最后写来的地址）：重载改了节、两条策略指向同一个节时都会出现；只按节共用会让带 `underlying-proxy` 的策略共用直连隧道、绕过 REJECT，也让只改策略行的重载不生效；全局启动锁会让两条隧道的启动互等到超时 |
| 6.5 "出站对象被释放 → 设备任务结束，载体关闭，进行中的流读写返回错误" | 隧道活到出站与经它的连接都释放为止；被更新的配置接替时才立即结束、连接随之失败（P11） | 重载不打断无关的连接（M3a）；隧道之间的冲突已由 P10 处理 |
| 6.5 "地址变了就重连该载体，地址族变了就重建" | 两种一样：每 5 分钟为写成域名的 endpoint 新拨一条载体，它去的地址不同就换上并立即握手；启动时连不上的 peer 也每 5 分钟再拨（P12） | 新载体总是新 socket，两种情形没有区别 |
| 6.1 "被流的读写唤醒时立即推进一次协议栈" | 推进时反复 `poll` 到没有东西可发；收到报文时先把载体上已到的全部收下（最多 256 个）再推进（P2、P9） | smoltcp 0.12 的一次 `poll` 每条连接最多发一个报文段 |
| —（没写拥塞控制） | Reno（只开 `socket-tcp-reno`，每条连接显式设置）（P3） | smoltcp 0.12 从不拿拥塞窗口与在途字节数比较（只与对端窗口的余量比较），两种算法都不真正限制发送；显式设 Reno，免得以后按窗口限速的版本落到 0.12 的 Cubic（它把 RFC 8312 以报文段计的窗口按字节算） |
| 6.4 "超过 MTU 的包不发（手册：丢弃）" | TCP 报文段按 MSS 切分，本来不会超出；更大的 IPv4 外发包由 smoltcp 分片后发出，只到 1500 字节的分片缓冲为止（P2） | 开了分片重组的特性，分片随之开启 |
| 6.3 的细节 | 第一个作答的服务器为准（"没有这个名字"也算）；该族没有本端地址或没有 peer 覆盖的服务器直接跳过；每个服务器最多等 2 秒；缓存最多 256 个名字、最长 1 小时、只存有地址的结果（P14） | 设计只写了原则 |
| 第 9 节"WireGuard 的握手成功 / 失败" | 成功只在第一次与失败之后恢复时记（`info`），失败在 boringtun 放弃重试时记一次（约 90 秒后，`warn`）；boringtun 自己的日志在 bin 里关掉（P13） | 流量不断时每两分钟换一次密钥，每次都记会刷屏；boringtun 的日志不带策略名 |
| 4.3 "`underlying-proxy` → `W0029`" | `W0029` 用专门的说法：`` `underlying-proxy` does not work with `wireguard` policies in this version; the policy rejects every connection ``；拨号时的 `Unsupported` 由引擎写进请求记录（此前引擎只在解析期写这类说明）（P17） | 通用说法"没有效果"不对：策略会 REJECT |
| 4.7 "`preshared-key` 已在名单里" | 不在，M4b 加上（P16） | `redact.rs` 的名单 |
| 4.1 没写重名的节 | 同名的节只用第一个，`W0020`（P15）；peer 的 endpoint 主机名进"代理服务器主机名"集合，`[Host]` 不作用于它 | 与重名策略、代理服务器主机名一致 |
| 第 16 节草图：7 测速、8 引擎 | 7 引擎、8 测速（P18） | 原生测速的端到端用例要经引擎 |
| 4.1 `self-ip` / `self-ip-v6` "纯地址，不是前缀" | 还须是单播地址：组播、广播、未指定地址是 `E0023`（实施时加） | smoltcp 对非单播的接口地址直接 panic，能通过 `rurge check` 的配置会在第一次拨号时 panic |
| 4.1 `dns-server` "IPv4 组播地址不接受" | 两族的组播地址、未指定地址与端口 0 都是 `E0023`（实施时加）；运行时发不出去的问题立即换下一个服务器 | P15 写的是组播一律不收；这些地址发不出问题，而旧代码在发送失败时会在持锁时再次加锁 |

## 20. M4b 实施期的订正

按计划实施时由终审与合并后的修正发现、与上文不同的地方；计划「执行期修正记录」里有对应的行。任务评审阶段的订正（6.1 的共用条件与启动不排队、4.1 `self-ip` / `dns-server` 的取值等）已写进第 19 节。

| 本文原文 | 实际 | 依据 |
| -------- | ---- | ---- |
| 6.7 "第一个有效握手回应的往返时间就是结果（多个 peer 时最快者胜）" | 从强制发起握手到任一 peer 第一次完成握手的时间：通常是一个往返，已有发起在途时可能更短 | 终审：boringtun 接受对上一次发起的回应；设备只记最近一次握手完成的时刻 |
| 6.5 "网络变化：设备留一个'网络已变化'的入口……阶段 2 没有触发它的探测器" | 另外：某个 peer 的载体发送时本机地址或路由已失效（地址不可用、网络或主机不可达、网络已断；每个 peer 至多每 10 秒一次），或 peer 不再回应握手（boringtun 约 90 秒后放弃）时，为它新拨一条载体（去的地址相同也换）并立即握手；peer 一直不回应时每约 90 秒这样重试一次 | 终审：载体是已连接的 UDP 套接字，本机地址变了之后它不会自己恢复，5 分钟重拨又跳过写成地址的 endpoint，只有重启 rurge 才好。合并后的修正：只认地址或路由失效——别的发送错误（如 macOS 上发送缓冲区满时的 `ENOBUFS`）只丢掉那个报文，不换载体 |
| 6.3 "每个服务器单次等待有上限" | 每个问题在 2 秒内每隔三分之一重发一次（最多三次）；问之前先等隧道第一次握手完成（受拨号时限约束）；A 与 AAAA 都问时，一族没有回应而另一族没有地址不算作答，接着问下一个服务器；一族有地址而另一族没有回应算作答，只缓存有地址的那一族 | 终审：原来每个问题只发一次，丢一个包就让这次拨号失败；刚启动时第一次握手的发起包丢了，boringtun 5 秒后才重试，而每个服务器只等 2 秒 |
| 第 10 节第 3 层"sing-box 的 WireGuard 端点" | 经隧道连 sing-box 端点自己的地址，sing-box 把它改写到自己的 127.0.0.1；不经隧道直接连 127.0.0.1 | 终审：sing-box 的 `system: false` 端点是 gVisor 协议栈，从外面进来的回环目的地址可能被丢弃；sing-box v1.14.1 `protocol/wireguard/endpoint.go` 的 `NewConnectionEx` 把端点 `address` 前缀里的目的地址改写到回环 |
| 4.1 "不认识的键 → `W0001`" | 键名（或 peer 字段名）只在像名字时（1 ～ 32 个小写字母、数字与 `-`）写进诊断，否则只说"有一行（一个字段）不认识" | 终审：一行 Base64 密钥以 `=` 结尾，会被当成"键名 = 空值"，整把密钥进了 `W0001` 的文本 |

## 21. M4c 计划期的订正

写 M4c 计划（`docs/superpowers/plans/2026-09-29-phase2-m4c-external-plan.md`）时核对 windows-sys 0.61.2、nix 0.31.3、tokio 1.53.1 与本仓库源码，并把全部任务在仓库副本上真实做过一遍之后，与上文不同的地方；P 编号是该计划「计划期决定」表的编号。

| 本文原文 | 计划 | 依据 |
| -------- | ---- | ---- |
| 7.1 "连接被拒时每 500 ms 重试，一个请求最多 6 次" | 每次尝试以它自己的 500 ms 为限：到时没连上与被拒一样处理，下一次在这 500 ms 结束时开始；6 次最多约 3 秒（P5） | Windows 上连一个没人监听的回环端口要约 2 秒才被拒（实测 2.03 秒），照原样重试要约 15 秒，超过默认 10 秒的拨号时限 |
| 7.2 "同一策略两次拉起至少间隔 2 秒……间隔内的请求仍按 7.1 的重试等待" | 间隔从上一次拉起（成功或失败）算起；上一次拉起失败的，间隔内的请求立即得到同一个错误，不再尝试；程序在间隔内自己退出的，交给请求的重试等过间隔（P7） | 拉起失败（路径写错等）时重试没有意义，请求应当立即失败 |
| 7.3 / 7.4 "停止时整组 SIGTERM，2 秒后仍在就 SIGKILL""旧进程在旧出站释放时停掉" | 每个程序由一个看守任务持有：程序自己退出时立即结束它的整个组；被要求停止或出站被丢弃时先要求整个组结束，2 秒内没结束就强制结束；看守任务被丢弃（运行时结束）时强制结束组与程序（P6） | 程序退出后留下的子进程只会占着下一次拉起要用的端口；出站被丢弃即停止，重载不需要额外的代码 |
| 7.3 "子进程加入一个 Job Object" | 按 pid（`OpenProcess`）打开刚拉起的程序再放进 Job；程序在被放进 Job 之前的几微秒里启动的子进程不在 Job 里，登记为已知限制（P1） | 两个平台的接口因此都是 `contain(pid)`；stable Rust 没有直接在 Job 里创建进程的办法 |
| 7.3 "子进程放进单独的进程组（tokio 的 `process_group`）" | 用 std 的 `CommandExt::process_group(0)`，配好 std 的命令再转成 tokio 的（P2） | `rurge-platform` 只认 `std::process::Command`，不依赖 tokio |
| 7.1 "策略名里不能出现在文件名里的字符替换掉" | 替换之外，名字因此变了、变空了或是 Windows 的设备名时，再加上原名 SHA-256 的前 8 个十六进制字符；分隔行是 `--- rurge: starting the program (unix time <秒>) ---`（P10） | 两个策略不能共用一个日志；工作区没有日期格式化的 crate |
| 4.4 没写 Shadow TLS 与 `tfo` 的两种告警 | Shadow TLS 不能叠在 `external` 上（`E0018`）；`tfo` 只报 `W0028`，不再同时报 `W0029`（P9） | 连的是本机上的程序 |
| 第 9 节 "`external: could not start <策略名>`（原因只取 I/O 错误的种类）" | `external: could not start <策略名> (<io::ErrorKind 的显示>)`；日志 `external: the program started` / `exited` / `stopped` / `could not be started`（P11） | 固定说法，从不写 `exec` 与 `args` |
| V14 "测试辅助程序……只用于测试的工作区成员里的二进制目标" | 新成员 `tests/external`（`rurge-external-tests`），真实拉起辅助程序的用例（`ExternalOutbound` 与经引擎的）都在这个包里；CLI 测试拿第二个 `rurge run` 当外部程序（P4） | `CARGO_BIN_EXE_<名字>` 只对同一个包的集成测试可见 |
| 第 16 节 M4c 草图 1 "`ExternalSpec`、重复 `local-port` 的错误" | `read_external` 在 Task 1；`ProtoSpec::External`、`to_spec` 分支与重复 `local-port` 在 Task 4 随引擎工厂一起落地（P8） | 有了 spec 却没有工厂分支时，干构建会把每一条 `external` 行报成加载错误 |

## 22. M4c 实施期的订正

按计划实施时由终审发现、与上文不同的地方；计划「执行期修正记录」里有对应的行。

| 本文原文 | 实际 | 依据 |
| -------- | ---- | ---- |
| 7.4 "变了的话，旧进程在旧出站释放时（进行中的会话结束后）停掉" | 会话拨号之后只持有流，旧出站通常在重载发布时就被释放、旧程序立即停掉（经它的连接随之断开）；旧出站仍被拿住时，新出站第一次拉起前让同一 `local-port` 上更早构建的出站退役（停掉程序、之后的拨号立即失败） | 终审：`ssh -D` 绑不上端口也不退出，新旧两个程序抢一个端口时策略会一直不可用 |
| 7.3 "Windows：子进程加入一个 Job Object" | 另外以 `CREATE_NEW_PROCESS_GROUP` 拉起，Ctrl-C 不直接送到外部程序 | 终审：否则外部程序先于 rurge 的退出流程自己退出 |
| 7.1 没写交互式提示 | 外部程序不能交互式提问（Unix 终端前台运行时会被挂起），推荐 `ssh -o BatchMode=yes -o ExitOnForwardFailure=yes` | 终审；代码修法要 unsafe |
| 4.4 没写与 rurge 自己的监听同端口 | `local-port` 等于回环或全零地址上的 `http-listen` / `socks5-listen` 端口是 `E0018` | 终审：会连回自己 |
