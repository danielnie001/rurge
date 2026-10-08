# 可行性报告：Snell v6、Snell v5 的 QUIC Proxy Mode、`tailscale` 策略

| 项       | 内容 |
| -------- | ---- |
| 日期     | 2026-10-08 |
| 对应需求 | 需求文档 FR-OUT-15；阶段 2 总设计第 1.4 节 M8 一行；M8 细化设计（`docs/superpowers/specs/2026-10-08-phase2-m8-wrapup-design.md`）第 6 节 |
| 结论     | 三项都不在阶段 2 实现。Snell v6：技术上可行，等正式版并需项目所有者对"照逆向所得实现"表态；v5 的 QUIC Proxy Mode：不做（Surge 在 v6 已取消）；`tailscale`：现在用本机的 tailscaled 经 `socks5` / `external` 策略接入，原生支持留待官方 Rust 库成熟后复评 |

调研只读：Surge 手册（<https://manual.nssurge.com/>，2026-10-07 版）、知识库与博客，下列开源项目的源码、文档与发布记录，crates.io 的元数据。没有下载、安装、构建或运行任何东西。GPL 的实现（sing-snell、sing-box、opensnell、mihomo）只取协议事实，用自己的话描述。

## 1. Snell v6

### 1.1 公开了什么

- **手册**（`policies/snell.html`）：Surge 支持协议版本 1 ～ 6；`version=6` 自 iOS 5.20 / Mac 6.7 起。v6 没有 `obfs`，另有 `mode`：`default`（加密，并按 PSK 派生的 profile 做流量整形）、`unshaped`（只加密）、`unsafe-raw`（不加密也不整形），两端必须一致；v6 不支持 QUIC Proxy Mode。手册至今写着 v6 "is currently in beta and may receive incompatible protocol changes"。
- **博客**《Introducing Snell v6》（2026-06-12）：v4 的线上特征已被外界研究清楚，所以 v6 改成"每个 PSK 一套协议特征"——profile 含 42 个参数、13 类填充与整形策略。博客给了一份示例 profile 的字段名，**没有给出从 PSK 到 profile 的算法，也没有线上格式**；Surge 写明有意不公开。
- **服务端**：独立的 `snell-server` v6 只有 Linux 版，最新 v6.0.0rc2（2026-08-07），其后没有新包；另有 Surge Mac 6.8 起内置的 `[Snell Server]`（`version = 6`）。

### 1.2 可依据的实现

| 项目 | 许可证 | v6 | 说明 |
| ---- | ------ | -- | ---- |
| SagerNet/sing-snell（sing-box 自 1.14.0 起的 `snell` 入站 / 出站） | GPL-3.0-or-later | 客户端与服务端 | 由 Surge 6.7.0 与 snell-server 的二进制逆向得来（源码注释逐处标着反编译函数），跟到了官方 RC；它自己的测试对着官方 v6.0.0rc 服务端跑 |
| missuo/opensnell | GPL-3.0 | 声称已实现、但不开源 | 作者以"与 v6 反指纹的目的一致"为由不公开 |
| MetaCubeX/mihomo、icpz/open-snell、cfal/shoes | — | 没有 | |
| ShinyNito/snell-rs | 标 MIT | 有 | 2026-09 才出现的个人仓库，协议事实从何而来没有说明，不能当作许可证干净的来源 |

有据可查的来源只有 sing-snell 一家，是逆向所得，没有规范。

### 1.3 协议概貌（按 sing-snell，只述事实）

v6 = v4 / v5 的记录核心（Argon2id 派生 AES-128-GCM 密钥、7 字节帧头、0-RTT、PSK）加三处变化：

1. 每帧负载上限从 0x3FFF 升到 0xFFFF；TCP 请求一律用带复用的命令（`0x05`）；PSK 须 12 ～ 255 字节。
2. 三种模式在线上不协商。`unsafe-raw`：没有 salt、不加密；`unshaped`：等于 v4 的记录流去掉首帧填充与混合；`default`：加整形层。
3. `default` 的整形层：由 PSK 经 BLAKE2b 与一组混合常数派生出约 40 个参数；每个方向的 16 字节 salt 散布在一个 32 ～ 144 字节的块里；每帧前有变长前缀（作为帧头的附加认证数据），帧里有按 profile 生成的填充，填充与负载密文按 profile 给出的位置互换；发送方另按 profile 决定怎样分块、每次写多少。

没有新的握手往返，也没有新的密码算法：rurge 已有的 Argon2、AES-GCM、BLAKE2 都够用，不需要新依赖。

### 1.4 互操作参照

- sing-box 1.14.2 的 `snell` 入站接受 `version: 6` 与三种 `mode`——rurge 的 CI 已在三个平台装了它，不用改安装步骤。
- 官方 `snell-server` v6.0.0rc2：只有 Linux，可仿照现有的 v5.0.1 一步只在 Linux CI 上装。
- 没有可用于 CI 的官方 v6 **客户端**：能验证的是"服务端解得开、转发正确"，验证不了"rurge 发出的流量与官方客户端长得一样"。

### 1.5 评估

- **规模**：M。`unsafe-raw` 与 `unshaped` 约 250 ～ 350 行加测试；`default` 的 profile 与记录流约 700 ～ 900 行；假服务端、单元与端到端测试约 900 ～ 1300 行。合计约 1.5 ～ 2.2 千行，一份 5 ～ 6 个任务的计划。
- **风险**
  1. 没有规范，`default` 模式的每一个常数与几段曲折的逻辑都只能从一份 GPL 的逆向实现得知；Surge 明说有意不公开。常数与算法是事实，但这里的"事实"量大、与表达贴得近——**是否照此实现是需要项目所有者表态的事，不只是工程问题**。
  2. 仍是 beta，协议变过（beta 2 换了 profile 的派生，RC 改了 UDP 的装帧），正式版若再改，rurge 要跟着改且无规范可依。
  3. 整形的保真度无法在 CI 里验证：与官方有出入的整形反而可能成为可识别的特征，这恰好违背 v6 的设计目的。
- **建议**
  1. 现在：不实现。`version=6` 的行照旧 `W0007` + REJECT；想用 v6 的人可以让 `external` 策略拉起 sing-box（`snell` 出站 `version: 6` 加本机 `socks` 入站），或用 `socks5` 策略指向这样的 sing-box。
  2. 若项目所有者决定实现：分两步。先做 `unshaped` 与 `unsafe-raw`（S，不涉及逆向得来的 profile 常数，可对 sing-box 与官方服务端完整验证），`mode=default` 的行保持 REJECT；`default` 等 v6 出正式版再做，先把协议写成自己的规格文字、再照规格实现。

## 2. Snell v5 的 QUIC Proxy Mode

### 2.1 公开了什么

知识库（v5.0.0 发行说明）：专为 QUIC 流量设计的"UDP over UDP"，服务端要开一个 UDP 端口；只在 Surge 识别出 QUIC 流量时启用，其它 UDP 仍走 UDP over TCP；只对 QUIC 握手包做强加密（保护 SNI 与目标主机名并完成认证），其后的包原样转发。手册："a runtime behavior with no profile parameter"——客户端没有开关，想不用就写 `version=4`。

Surge 自己的事后评价（v6 的博客）："In practice, the results did not meet our expectations. For most proxy scenarios, the better strategy for QUIC traffic remains to block QUIC and allow applications to fall back to HTTP/2 over TCP. For this reason, Snell v6 no longer supports QUIC Proxy Mode."

没有公开线上格式。

### 2.2 可依据的实现

只有 missuo/opensnell 的**服务端**（GPL-3.0，对着官方 v5.0.1 服务端抓包逆向）；sing-snell / sing-box 明说有意不做，mihomo 把 `version: 5` 当 4 用。按 opensnell 的服务端：新来源地址的第一个数据报是一个信封（16 字节 salt、密封的帧头、密封的负载——请求头后面紧跟原始的 QUIC 包，每个信封一次 Argon2id），此后这个来源地址的数据报与回程一律原样转发。官方客户端到底封装哪些包、流的超时、来源端口变化后的行为等都没有公开依据。

### 2.3 评估

技术上 S ～ M（约 400 ～ 700 行加假服务端与测试），官方 v5.0.1 服务端（rurge 的 Linux CI 已装）可作参照。**不做**，理由：

1. 厂商已经放弃：v6 取消了它，并写明效果不及预期、更好的做法仍是阻断 QUIC。
2. 规格只来自一家第三方的抓包逆向，关键行为没有公开依据。
3. 默认路径上用不到：代理策略缺省阻断 QUIC，要用户显式 `block-quic=off`。
4. 握手之后是裸 QUIC，客户端到服务端这一段对旁观者暴露为"发往代理服务器的 QUIC 流"。

rurge 维持现状：`snell` 的 `udp-port` 只解析（`W0029`），`block-quic=off` 时 QUIC 经普通的 UDP over TCP 转发。有明确的用户需求时再评估。

## 3. `tailscale` 策略

### 3.1 Surge 做了什么

手册（`policies/tailscale.html`，iOS 5.20 / Mac 6.7 起）：`tailscale` 策略让 Surge 作为一个节点加入 tailnet，把 tailnet 变成一个出站策略（TCP 与 UDP）——向控制服务器注册、取得 peer / 路由 / DNS / DERP 配置、与各 peer 建 WireGuard 隧道、能直连就直连否则走 DERP 中继、在策略里解析 MagicDNS，"all without installing a separate system VPN"。`[Tailscale <名>]` 节有 `auth-key` / `interactive-login`、`control-url`、`hostname`、`derp-only`、`auto-add-magic-dns-rule`、`exit-node`、`idle-keepalive`、`prefer-ipv6`、`dns-server`、`mtu`。手册称之为"Surge's compatible policy implementation"；6.9 又加了 peer relay——这是厂商持续数个版本的投入。

### 3.2 让 rurge 成为 tailnet 节点的几条路

| 路径 | 今天能否 | 成本 | 主要问题 |
| ---- | -------- | ---- | -------- |
| A. 借用本机的 tailscaled（或 sing-box 的 `tailscale` 端点），经 `socks5` / `external` 策略接入 | 能 | 只有文档 | 不是 Surge 语法的 `tailscale` 策略；节里的字段要在 tailscaled 一侧配置 |
| B. 嵌 libtailscale（官方的 C 库，内含 Go 运行时） | 不能 | — | 要 Go 与 C 工具链、要 FFI（`unsafe`），上游不支持 Windows——三条都与 rurge 的约束相抵 |
| C. 嵌官方的 Rust 库 `tailscale`（tailscale-rs，BSD-3-Clause） | **现在不能** | 中 | 仍是预览（0.6.1，pre-1.0，每月可能有破坏性变更）；不支持 MagicDNS、exit node、子网路由、peer relay；MSRV 1.92 且滚动（rurge 是 1.89）；TLS 后端写死 aws-lc-rs（rurge 用 ring）；没有自定义拨号的钩子（做不了 `underlying-proxy`、`derp-only`、让控制面连接走 rurge 的规则） |
| D. 原生自研 | 能开工 | XL（约 1.5 ～ 2.5 万行，相当于或超过整个 QUIC 族），外加跟随协议演进的持续维护 | 协议没有规范，只由 Go 源码定义且持续演进（控制面的 capability version 四个月里加了 9 个）；NAT 穿透难在回环上测 |
| D′. 原生自研，只走 DERP 起步 | 能开工 | L（约 6 ～ 9 千行） | 全部流量走中继；仍要跟随 netmap 的演进 |

同类的 Go 代理核心（sing-box、mihomo）都没有重写，嵌的是官方 Go 实现的分叉；可信的第三方纯 Rust 实现一个也没有。

原生自研缺的部分（rurge 已有每个 peer 一个 boringtun 隧道、最长前缀选 peer、smoltcp 协议栈、隧道内 DNS）：控制通道（Noise IK 之上的 HTTP/2）、注册与登录、netmap 长轮询、DERP 客户端、netcheck / STUN、disco 与选路（NAT 穿透）、数据面的重新装配、包过滤、MagicDNS、exit node 与子网路由、密钥状态与轮换、配置与生命周期、回环上的假控制服务器 / 假 DERP / 假 peer。

### 3.3 路径 A 的写法（现有功能，未实测）

tailscaled 自带 SOCKS5 与 HTTP 代理，且有不建虚拟网卡的用户态模式（官方文档 <https://tailscale.com/docs/concepts/userspace-networking>）；它的 SOCKS5 支持 UDP ASSOCIATE。

```shell
tailscaled --tun=userspace-networking --socks5-server=127.0.0.1:1055 --statedir=<目录> --socket=<路径>
tailscale --socket=<路径> up --auth-key=<key>      # 一次性登录；自建控制服务器加 --login-server=<URL>
```

```ini
[Proxy]
# 已有 tailscaled 在跑（系统里以 TUN 方式运行的 tailscaled 也可以加 --socks5-server）
TS = socks5, 127.0.0.1, 1055, udp-relay=true
# 或由 rurge 按需拉起（登录状态在 --statedir 里；首次启动慢于 3 秒时，下一次用到会再连）
TS = external, exec = "/usr/sbin/tailscaled", args = "--tun=userspace-networking", args = "--socks5-server=127.0.0.1:1055", args = "--statedir=/var/lib/rurge-ts", args = "--socket=/run/rurge-ts.sock", local-port = 1055, udp-relay=true

[Rule]
DOMAIN-SUFFIX,<tailnet 名>.ts.net,TS
IP-CIDR,100.64.0.0/10,TS,no-resolve
```

域名交给 tailscaled 解析（MagicDNS）。与 Surge 策略的差距：`exit-node`、`derp-only`、`mtu` 等要在 tailscaled 一侧配置（`tailscale set --exit-node=…`）；没有自动的 MagicDNS / peer 地址规则；`underlying-proxy`（让 DERP 走另一个策略）做不到；没有原生的连通性探测与状态视图。

### 3.4 建议

1. **现在**：走 A。`tailscale` 策略行与 `[Tailscale <名>]` 节照旧解析，策略按 `W0007` + REJECT；兼容性清单与手工验收清单写上 A 的写法。
2. **跟踪 tailscale-rs**，下列条件具备时复评路径 C：出 1.0 或给出 API 稳定性承诺；有 MagicDNS 与使用 exit node / 子网路由的能力；可以换 TLS 密码后端（或 rurge 接受 aws-lc-rs）；有自定义拨号 / 传输的钩子与 DERP-only；MSRV 政策与 rurge 相容（或 rurge 抬高 MSRV）。届时作为一个可选特性的独立 crate 接入，接受它自带的 WireGuard 与网络栈。
3. **不建议现在走 D / D′**。项目所有者若仍要原生支持，从 D′ 起步，并把它当作单独的一个阶段来设计。测试上，headscale（BSD-3-Clause 的自建控制服务器，没有 Windows 发布包）加 sing-box 的 `tailscale` 端点作第二个节点，原则上可以全在回环上跑，但要走 headscale 内嵌的 DERP（需要测试证书）；这一点没有实测。

## 4. 来源

- Surge 手册：<https://manual.nssurge.com/policies/snell.html>、<https://manual.nssurge.com/features/snell-server.html>、<https://manual.nssurge.com/policies/tailscale.html>、<https://manual.nssurge.com/policies/udp.html>、<https://manual.nssurge.com/policies/parameters.html>
- Surge 知识库与博客：<https://kb.nssurge.com/surge-knowledge-base/release-notes/snell>、<https://nssurge.com/blog/snell-v6/>
- SagerNet/sing-snell（提交 `bc5a12ac`）与 sing-box v1.14.2 的 `docs/configuration/{inbound,outbound}/snell.md`；missuo/opensnell（`3100984f`）的 `README.md`、`SNELL_V6.md`；MetaCubeX/mihomo（`88dcbf7f`）的 `adapter/outbound/snell.go`
- tailscale/tailscale（`a5ae05dd`）的 `cmd/tailscaled/proxy.go`、`net/socks5/socks5.go`；tailscale/libtailscale（`59d4bb82`）；tailscale/tailscale-rs（`b44db0da`，crate `tailscale` 0.6.1）的 `README.md`、`CHANGELOG.md`、`Cargo.toml`；公告 <https://tailscale.com/blog/tailscale-rs-rust-tsnet-library-preview>；juanfont/headscale v0.29.4
