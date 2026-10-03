# rurge-interop

`rurge-interop` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它把 [sing-box](https://sing-box.sagernet.org/)、[xray](https://github.com/XTLS/Xray-core)、[shadowsocks-rust](https://github.com/shadowsocks/shadowsocks-rust) 的 `ssserver`、Surge 官方的 `snell-server`、[TrustTunnel](https://github.com/TrustTunnel/TrustTunnel) 的 `trusttunnel_endpoint` 与 TUIC 官方 [tuic](https://github.com/tuic-protocol/tuic) 0.8.5 的 `tuic-server` 作为参照实现，以回环子进程的方式拉起来，驱动 rurge 的 `http` / `https` / `socks5` / `trojan` / `vmess` / `anytls` / `wireguard` / `ss` / `snell` / `h2-connect` / `trust-tunnel` / `hysteria2` / `tuic-v5` / `tuic` 出站，以及包在 Shadow TLS 里的 `trojan`，去连它们，验证 rurge 与真实的第三方实现互通。xray 只用来跑 `vmess`：VMess 协议由 xray 所在的这一脉实现定义，sing-box 的实现是重写，手写的编解码需要两个独立参照互相印证（M2 设计 M2-D5）。tuic-server 只用来跑 `tuic`（TUIC v4）：sing-box 只实现 v5（M7 设计 M7-D3）。

## 固定版本

互操作测试固定 sing-box **1.14.2**（2026-09-24 发布的稳定版；阶段 2 / M6b 从 1.14.1 升上来：`snell` 入站 1.14.0 起才有，1.14.2 是 1.14.x 的最新版；`hysteria2` 入站的 Gecko 混淆同样 1.14.0 起才有，QUIC 支持（构建标签 `with_quic`）是发布包的缺省）。CI 下载并校验以下三个发布包（SHA-256 取自 GitHub 发布 API 每个资产的 `digest`）：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (amd64) | `sing-box-1.14.2-linux-amd64.tar.gz` | `a684484d7477d1437282ee411f4d131d0340aaad60a7868841ebd5d87dd8a0c6` |
| Windows (amd64) | `sing-box-1.14.2-windows-amd64.zip` | `c2d8bfff918755808781dfdeeb8581b6c91eb3a243d9a7b55483cfc0c0684d32` |
| macOS (arm64) | `sing-box-1.14.2-darwin-arm64.tar.gz` | `925c5382eca8492b0150f868a6db20b18290a38700e621724b3703fd453e032d` |

## 本地运行

本地默认不安装 sing-box、xray、shadowsocks-rust、snell-server、TrustTunnel endpoint 与 tuic-server：`cargo test -p rurge-interop` 会正常通过，sing-box 的二十七个互操作用例、xray 的两个、shadowsocks-rust 的两个、snell-server 的四个、TrustTunnel endpoint 的四个与 tuic-server 的三个互操作用例各打印一行 `skipping …` 后直接返回（各夹具自身的单元测试——配置渲染、"配置绝不碰本机"的安全守卫、取空闲端口——照常运行并断言）。要在本机真正跑 sing-box 的用例，二选一：

- 自行安装 sing-box 1.14.2，让 `sing-box` / `sing-box.exe` 出现在 `PATH` 上；或
- 不安装到 `PATH`，改用 `RURGE_TEST_SING_BOX=<sing-box 可执行文件路径> cargo test -p rurge-interop`。

xray、shadowsocks-rust、snell-server、TrustTunnel endpoint 与 tuic-server 的本机运行方式同理（tuic-server 只认环境变量，不在 `PATH` 上找），见下面「xray」「shadowsocks-rust」「snell-server」「TrustTunnel」「tuic-server」五节。

## 环境变量

- `RURGE_TEST_SING_BOX`：sing-box 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_TEST_XRAY`：xray 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_TEST_SSSERVER`：shadowsocks-rust 的 `ssserver` 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_TEST_SNELL_SERVER`：Surge 官方 `snell-server` 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_TEST_TRUSTTUNNEL`：TrustTunnel `trusttunnel_endpoint` 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_TEST_TUIC_SERVER`：tuic 0.8.5 的 `tuic-server` 可执行文件的路径；只认这个变量，不在 `PATH` 上找（`PATH` 上的 `tuic-server` 多半是 1.x 版，只说 TUIC v5，配置也不同）。
- `RURGE_INTEROP_REQUIRED=1`：找不到二进制时让用例直接失败，而不是打印 `skipping …` 后跳过；对 sing-box、xray、shadowsocks-rust 与 sshd 各夹具都生效，对 snell-server 只在 Linux 上生效（它只有 Linux 版），对 TrustTunnel endpoint 只在 Linux 与 macOS 上生效（它没有 Windows 版），对 tuic-server 只在 Linux 上生效（CI 只在 Linux 上装它）。CI 会设置它，本机一般不需要。

## 覆盖范围

`tests/sing_box.rs` 驱动的五个用例（另一个是夹具自检）覆盖：

- `http`：CONNECT 隧道，含无认证 / 正确凭据 / 错误凭据三种；以及明文 HTTP 请求走绝对 URI 转发（不经 CONNECT）。
- `https`：私有 CA 校验、`sni=` 覆盖、`server-cert-fingerprint-sha256` 指纹钉定（含钉错的情形）、`client-cert=`（p12 客户端证书）双向 TLS，以及服务端要求客户端证书但客户端未提供的情形。
- `socks5` 的 UDP（阶段 2 / M5a）：`udp-relay=true` 时经 sing-box `socks` 入站的 UDP ASSOCIATE 往返一个回环 UDP 回显。
- `socks5`：无认证 / 正确凭据 / 错误凭据，以及 sing-box 的 `mixed` 入站同时按 SOCKS5 和 HTTP 两种协议接受连接。

`tests/sing_box_tls_family.rs` 驱动的六个用例覆盖：

- `trojan`：TLS 传输，以及可选的 V2Ray WebSocket 传输（`ws=true`, `ws-path=`）。trojan 协议对请求头没有任何应答，连接本身在密码错误时也会建立成功，所以"密码错误"用例断言的是负载不被回显，而不是连接失败。
- `vmess`：AEAD 握手（`vmess-aead=true`），两种 `encrypt-method`（默认的 `aes-128-gcm` 与 `chacha20-ietf-poly1305`），可选的 TLS 与 V2Ray WebSocket 传输及其组合，单块与跨多块（100 000 字节）的往返，以及 UUID 错误时的行为——sing-box 只应答它接受的请求，所以连接本身能建立，但读不到回显。
- `anytls`：同一个出站发起多条流默认复用同一个会话（`reuse` 的协议默认值），以及 `reuse=false` 时每条流各开一个新会话。
- 三种协议的 UDP（阶段 2 / M5b）：`trojan` 的 UDP ASSOCIATE、`vmess` 的命令 2、`anytls` 的 UDP over TCP v2，各经 sing-box 往返一个回环 UDP 回显两次。

- Shadow TLS（`tests/sing_box_shadow_tls.rs`）：sing-box 的 `shadowtls` 入站（v2 与 v3，v3 开 `strict_mode`）把握手转发给夹具自己在回环上起的 TLS 服务端（伪装站点，证书由夹具的 CA 签发），解出来的流量经 `detour` 交给同一个 sing-box 里的 `trojan` 入站；覆盖小负载与跨多帧的往返，以及口令错误（v3 的会话文本、伪装站点确实收到了那个 HTTP 请求）。v2 的用例让伪装站点不发 session ticket（sing-box 对"首帧之前又转发了字节"只多容忍一次写），v3 的发两张（覆盖数据阶段开头的残留记录）。

- WireGuard（`tests/sing_box_wireguard.rs`）：sing-box 的 WireGuard 端点（`endpoints`，sing-box 1.11 起；`system: false`，在用户态运行，不建网卡、不改路由）以 rurge 为唯一的 peer，给发往它的每个报文写上保留字节 `1/2/3`；rurge 的 `wireguard` 出站带 `client-id = 1/2/3` 与它握手，经隧道连 sing-box 自己的隧道地址 `10.9.0.1` 上的 echo 端口（节里 `allowed-ips = 10.9.0.1/32`）：sing-box 把发往端点自身地址的连接改写到它的回环 `127.0.0.1` / `::1`，echo 就听在那里（sing-box 1.14.1 `protocol/wireguard/endpoint.go` 的 `NewConnectionEx`）——直接发往 `127.0.0.1` 的目标经隧道进来，可能被它的用户态协议栈丢弃；隧道里出来的连接交给 `direct`。覆盖单块与跨多块的往返、原生测速（强制握手），以及经隧道往返一个 UDP 回显（阶段 2 / M5c；发往隧道地址 `10.9.0.1` 上回显的端口，同样被改写到回环）。rurge 收到的报文里 sing-box 写的保留字节必须先清零，否则 boringtun 认不出报文类型、握手不成。端点没有监听地址这一项，**它的 UDP 端口开在所有地址上**；就绪与否看同一份配置里一个只听 `127.0.0.1` 的 `mixed` 入站（UDP 端口无从探测）。

- Shadowsocks（`tests/shadowsocks.rs` 里的 `ss_against_sing_box_aead_2022_and_a_user`，阶段 2 / M6a）：sing-box 的 `shadowsocks` 入站，`aes-192-gcm` 与 `xchacha20-ietf-poly1305` 两种 AEAD 方法（shadowsocks-rust 的发布包不带这两种，见下面「shadowsocks-rust」一节）、单密钥的 `2022-blake3-aes-256-gcm`，以及 `2022-blake3-aes-128-gcm` 两个用户里的第二个（`password=服务端密钥:用户密钥`，一层身份头）；每种都做单块与跨多块（100 000 字节）的 TCP 往返，以及经 `udp-relay=true` 往返一个回环 UDP 回显两次。

- Snell（`tests/snell.rs` 里 `…_against_sing_box` 的四个用例，阶段 2 / M6b）：sing-box 的 `snell` 入站（1.14.0 起），`version: 5`、一个 `psk`、不配 `users`（服务端不看 client id）。sing-box 没有"version 4"的入站；`version: 5` 走 v4 / v5 共用的线上格式，所以同时接受 v4 客户端。覆盖 `version=5` 与 `version=4` 两种客户端的单块与跨多块（100 000 字节）TCP 往返；`reuse=true` 时一连四个请求（每个都两个方向干净结束，连接回池给下一个请求用）再加一个大负载；`obfs=http`（入站的 `obfs_mode: http`；含 `reuse=true` 与 UDP）；以及 UDP over TCP（命令 `0x06`）经同一个载体往返一个回环 UDP 回显两次。

- Hysteria 2（`tests/sing_box_hysteria2.rs` 的五个用例，阶段 2 / M7a）：sing-box 的 `hysteria2` 入站只听回环上的一个 UDP 端口；证书签给 `127.0.0.1`、由夹具的 CA 签发，rurge 信任这个 CA（服务器写成 IP，不发 SNI）；`tls.alpn` 写 `["h3"]`；一个用户（`users[].password`）；不配 `masquerade`（口令不对时 sing-box 回 404）、不限带宽。覆盖 TCP 的单块、跨多块（100 000 字节）与每个方向 9 MiB 同时往返（超过 rurge 每条流 8 MiB 的接收窗口：回来的数据要靠 rurge 边读边放出的流控额度），这三项再用写了 `download-bandwidth=100` 的策略做一遍（sing-box 于是按这个速率用 Brutal 发，不用 BBR）；UDP：同一条连接上两个载体（各占一个会话号）各自往返几个字节与 3000 字节的数据报，后者两个方向都按 Hysteria 的分片走（sing-box 把应答切成约 1200 字节一片，不回超过 4096 字节的消息；macOS 的回环不送超过 9216 字节的数据报）；口令错误时 TCP 与 UDP 都以 `hysteria2: authentication failed` 失败；`obfs` 的 `salamander` 与 `gecko`（包长用 sing-box 的缺省 512–1200 字节，与 rurge 相同）两种混淆下的 TCP（单块与跨多块）与 UDP。

- TUIC v5（`tests/tuic.rs` 里 `v5_…_against_sing_box` 的三个用例，阶段 2 / M7b）：sing-box 的 `tuic` 入站（只实现 v5）只听回环上的一个 UDP 端口，证书同 Hysteria 2 一节；`tls.alpn` 写 `["h3"]`（rurge 没写 `alpn` 时发 `h3`，QUIC 服务端不选协议时握手失败）；一个用户（`users[]` 的 `uuid` 与 `password`）；拥塞控制、认证时限（3 秒）、心跳（10 秒）与 0-RTT（关）都用 sing-box 的缺省。覆盖 TCP 的单块、跨多块（100 000 字节）与每个方向 9 MiB 同时往返（超过 rurge 每条流 8 MiB 的接收窗口）；UDP：同一条连接上两个载体（各占一个关联号）各自往返几个字节与 3000 字节的数据报，后者两个方向都按 TUIC 的分片走（sing-box 把应答切成至多 1197 字节的数据报）；口令错误：sing-box 不回应 v5 的认证，令牌不对时以码 0、空原因关闭连接（它因任何错误关连接都是这样），而且常常先结束手上挂着的 Connect（STOP_SENDING 0 加 FIN），所以 TCP 隧道照样先建立（v5 的隧道一律立即交出），然后什么也没转发就结束——关闭先到时读到 `tuic: authentication failed`，Connect 的结束先到时读到流的结束，写可能遇到 `tuic: the server stopped the stream (code 0x0)`；UDP 没有这种竞争，发现连接被这样关闭的那次发送以 `tuic: authentication failed` 失败。**只覆盖 `native` 模式**：sing-box 的 TUIC 服务端总是接受 QUIC 数据报（配置里没有关掉它的项），`quic` 模式（服务端不接受数据报时每个包一条单向流）只由回环假服务端（`rurge_proto_quic::testing::FakeTuic` 的 `no_datagrams`）覆盖。

**不覆盖端口跳跃**：`port-hopping`（Hysteria 2 与 TUIC）要在服务端用防火墙把一段端口 DNAT 到它的监听端口（Hysteria 文档的部署方式），夹具不改本机的防火墙与网络设置，回环上做不到；端口跳跃由回环假服务端（`rurge_proto_quic::testing::FakeHysteria2` 与 `FakeTuic` 同时监听几个端口，跳跃间隔经只用于测试的入口缩短）与手工验收（`docs/acceptance/phase2-manual.md` 的 M7a 与 M7b 两节）覆盖。

**不覆盖 HTTP/2 的 `h2-connect`**：sing-box 1.14.2 的 `http` 入站只说 HTTP/1（TLS 握手之后按 HTTP/1.x 读请求，不设 ALPN、没有 h2 代码路径；HTTP/2 要到 1.15.0，目前仍是预发布），所以 `h2-connect` 的普通 CONNECT 改由 TrustTunnel endpoint 覆盖（见下面「TrustTunnel」一节）。

**不覆盖 `socks5-tls`**：sing-box 的 `socks` 与 `mixed` 入站没有 `tls` 字段（参见 sing-box 文档 `configuration/inbound/{socks,mixed}`），无法用它搭建一个会说 TLS 的 SOCKS5 服务端。`socks5-tls` 的覆盖仍由 M1a 的回环假上游（`rurge_proto::testing::FakeSocks5`）承担。

## xray

xray 只用来验证 `vmess`：VMess 协议由 xray 所在的这一脉实现（v2ray / xray）定义，sing-box 的实现是重写，手写的编解码需要两个独立参照互相印证（M2 设计 M2-D5）。

互操作测试固定 xray **v26.3.27**。CI 下载并校验以下三个发布包：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (amd64) | `Xray-linux-64.zip` | `23cd9af937744d97776ee35ecad4972cf4b2109d1e0fe6be9930467608f7c8ae` |
| Windows (amd64) | `Xray-windows-64.zip` | `d004c39288ce9ada487c6f398c7c545f7d749e44bdfdd59dbc9f865afba4e1ad` |
| macOS (arm64) | `Xray-macos-arm64-v8a.zip` | `2e93a67e8aa1936ecefb307e120830fcbd4c643ab9b1c46a2d0838d5f8409eaf` |

`tests/xray.rs` 驱动的两个用例覆盖 `vmess`：两种 `encrypt-method`（默认的 `aes-128-gcm` 与 `chacha20-ietf-poly1305`）、可选的 V2Ray WebSocket 传输，以及单块与跨多块（100 000 字节）的往返；另一个用例覆盖命令 2 的 UDP（阶段 2 / M5b）：两种 `encrypt-method`，经同一个载体轮流发往两个回环 UDP 回显（每个目标一条连接）。

本机不安装 xray：`RURGE_TEST_XRAY`（优先于 `PATH` 查找）没有指向可执行文件、`PATH` 上也找不到 `xray` / `xray.exe` 时，用例打印一行 `skipping …` 后直接返回；这个 crate 不会下载或安装 xray。互操作由首次推送后的 CI 证明（CI 安装 xray v26.3.27 并设置 `RURGE_TEST_XRAY` 与 `RURGE_INTEROP_REQUIRED=1`）。

渲染出的配置（`rurge_interop::xray::render`）只有 `log` / `inbounds` / `outbounds` 三个顶层键：每个入站是一个只监听 `127.0.0.1` 的 `vmess`（可选 `ws` 传输），唯一的出站是 `freedom`。

## shadowsocks-rust

互操作测试固定 shadowsocks-rust **v1.25.0**（2026-08-26 发布），只用其中的 `ssserver`。CI 下载并校验以下三个发布包（SHA-256 取自 GitHub 发布页每个资产的 `digest`）：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (amd64) | `shadowsocks-v1.25.0.x86_64-unknown-linux-gnu.tar.xz` | `874f817fcf3e6d7681ec715a1c13c686c6eaae936524d102639b38364f3966ae` |
| Windows (amd64) | `shadowsocks-v1.25.0.x86_64-pc-windows-msvc.zip` | `882151ea5c52941d4a3360ebd12c74c6d6bd1b599596089ef1811b379c705666` |
| macOS (arm64) | `shadowsocks-v1.25.0.aarch64-apple-darwin.tar.xz` | `58e0caf0cc9266c4ea226f38aa20fb28c1be12efc87a73cf5903197867555208` |

`tests/shadowsocks.rs` 里对 `ssserver` 的两个用例覆盖 `ss`：

- `aead_methods_against_ssserver`：`aes-128-gcm`、`aes-256-gcm`、`chacha20-ietf-poly1305`。
- `ss_2022_against_ssserver_with_one_key_and_as_a_user`：单密钥的 `2022-blake3-aes-128-gcm` 与 `2022-blake3-aes-256-gcm`，以及 `2022-blake3-aes-256-gcm` 两个用户里的第二个（`ssserver` 配置的 `users`，SIP023 的身份头）。

每种方法都做单块与跨多块（100 000 字节）的 TCP 往返，以及经 `udp-relay=true` 往返一个回环 UDP 回显两次（`ssserver` 的 `mode` 是 `tcp_and_udp`，UDP 端口与 TCP 端口同号）。

`ssserver` 的发布包（默认的 `full` 特性）不带 `aes-192-gcm` 与 `xchacha20-ietf-poly1305`（它们在 `aead-cipher-extra` 特性里），这两种由 sing-box 的 `shadowsocks` 入站覆盖（见上面「覆盖范围」），另有回环假服务端（`rurge_proto::testing::FakeShadowsocks`）。`none` 只由假服务端覆盖。

**不覆盖 obfs**：simple-obfs 已停止维护，sing-box 不带插件，`http` / `tls` 两种 obfs 都没有可用的预编译参考服务端；它们由假服务端（`rurge_proto::testing::accept_obfs`）与手工验收（`docs/acceptance/phase2-manual.md` 的 M6a 一节）覆盖。

本机不安装 shadowsocks-rust：`RURGE_TEST_SSSERVER`（优先于 `PATH` 查找）没有指向可执行文件、`PATH` 上也找不到 `ssserver` / `ssserver.exe` 时，用例打印一行 `skipping …` 后直接返回；这个 crate 不会下载或安装它。互操作由首次推送后的 CI 证明（CI 安装 shadowsocks-rust v1.25.0 并设置 `RURGE_TEST_SSSERVER` 与 `RURGE_INTEROP_REQUIRED=1`）。

渲染出的配置（`rurge_interop::shadowsocks_rust::render`）只有 `servers` 一个顶层键：每个服务端只监听 `127.0.0.1`，`mode` 为 `tcp_and_udp`，没有 `manager`、`locals`、插件、ACL 与 `outbound_*` 这些键（夹具的单元用例 `the_configuration_stays_on_the_loopback` 断言这一点）。

## snell-server

Surge 官方的 `snell-server`（发布在 Surge 知识库）是 `snell` 的第二个参照：sing-box 的实现是重写，线上格式需要两个独立参照互相印证（阶段 2 / M6 设计 M6-D5）。互操作测试固定 **v5.0.1**，只有 Linux (amd64) 版：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (amd64) | `snell-server-v5.0.1-linux-amd64.zip`（`https://dl.nssurge.com/snell/`） | `9bea1c2b9e35b73b31634856c04d18c393072b9e5dcde6a32781d8b8f908c539` |

知识库不公布校验和；上面的值是计划期对该 zip 自行计算的（解压出的 `snell-server` 的 SHA-256 为 `5b2e221f2c6e29b1db8e47053e1221be29d5627da807cb932b089f514a3609f0`，与第三方项目钉住的值一致）。二进制经 UPX 压缩。

`tests/snell.rs` 里 `…_against_snell_server` 的四个用例覆盖：`version=5` 与 `version=4` 两种客户端的单块与跨多块 TCP 往返；`reuse=true` 时一连四个请求加一个大负载；`obfs = http`（含 `reuse=true` 与 UDP）；UDP over TCP 往返一个回环 UDP 回显两次。

CI 只在 Linux 上下载、校验并设置 `RURGE_TEST_SNELL_SERVER`；Windows 与 macOS 上这四个用例照常编译，运行时打印 `skipping …` 后返回（`RURGE_INTEROP_REQUIRED=1` 对 snell-server 只在 Linux 上生效）。本机不安装它：`RURGE_TEST_SNELL_SERVER`（优先于 `PATH` 查找）没有指向可执行文件、`PATH` 上也找不到 `snell-server` 时同样跳过；这个 crate 不会下载或安装它。

渲染出的配置（`rurge_interop::snell_server::render`）是一个 `[snell-server]` 节：`listen = 127.0.0.1:<端口>`、`psk`、`ipv6 = false`，按需加 `obfs = http`；没有 `dns`、`egress-interface` 这些键（夹具的单元用例 `the_configuration_stays_on_the_loopback` 断言只监听回环）。启动命令是 `snell-server -c <配置文件>`，就绪与否看 TCP 端口能否连上。

## TrustTunnel

[TrustTunnel](https://github.com/TrustTunnel/TrustTunnel) 的 endpoint 是 `trust-tunnel` 的参照实现；它的 TCP 模式就是标准的 HTTP/2 CONNECT（`:authority` 为 `host:port`，`proxy-authorization: Basic`，200 / 407 / 502），所以同时充当 `h2-connect` 普通 CONNECT 的参照服务端（sing-box 1.14.2 的 `http` 入站只说 HTTP/1，见上面「覆盖范围」末尾）。互操作测试固定 **v1.1.0**（2026-09-01 发布），只有 Linux 与 macOS 版。CI 下载并校验以下两个发布包（SHA-256 取自 GitHub 发布 API 每个资产的 `digest`）：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (x86_64) | `trusttunnel-v1.1.0-linux-x86_64.tar.gz` | `91c2ea3db7416a01b5258a4c047ec22890490bc55e1b194206031aa75144f0e7` |
| macOS (universal) | `trusttunnel-v1.1.0-macos-universal.tar.gz` | `126a5688e922ce8f83d4de2d8a2659b3aab97820184d30f2edbecc0ec4a32ae5` |

发布包里是一个以资产名命名的目录，内有 `trusttunnel_endpoint`（另有 `setup_wizard`、签名文件与 `LICENSE`，不用）。

`tests/http2.rs` 驱动的四个用例覆盖：

- `trust-tunnel`：单块与跨多块（100 000 字节）的 TCP 往返；口令错误时 endpoint 回 407，出站报 `trust-tunnel: authentication failed`。
- `h2-connect`：凭据写在端口之后（位置参数）与写成 `username=` / `password=` 两种，各做单块与跨多块的 TCP 往返；`max-streams=3` 时同时打开七条隧道并全部保持（多于一条连接能承载的流，池要开新连接），每条各自回显自己的字节，之后再开一条新隧道。

**不覆盖 CONNECT-UDP**：TrustTunnel 的 UDP 走它自己的 `_udp2` 复用格式，不是 RFC 9298 的 CONNECT-UDP；目前找不到可用的稳定版参照服务端（sing-box 要到 1.15.0 才在 `http` 入站上提供，且它的发布版是否通告 extended CONNECT 未经核实）。`h2-connect` 的 CONNECT-UDP 由回环假服务端（`rurge_proto::testing::FakeH2Proxy`，开 `enable_connect_protocol`）与手工验收（`docs/acceptance/phase2-manual.md` 的 M6c 一节）覆盖。

endpoint 按 SNI 精确匹配它的主机（`main_hosts[].hostname`），所以夹具的证书签给 `tt.test`，rurge 的每一行都写 `sni=tt.test` 并信任夹具的 CA；ALPN 固定 `h2`（不发 ALPN 时 endpoint 按 HTTP/1 处理）。

CI 只在 Linux 与 macOS 上下载、校验并设置 `RURGE_TEST_TRUSTTUNNEL`；Windows 上这四个用例照常编译，运行时打印 `skipping …` 后返回（`RURGE_INTEROP_REQUIRED=1` 对它只在 Linux 与 macOS 上生效）。本机不安装它：`RURGE_TEST_TRUSTTUNNEL`（优先于 `PATH` 查找）没有指向可执行文件、`PATH` 上也找不到 `trusttunnel_endpoint` 时同样跳过；这个 crate 不会下载或安装它。

渲染出的配置（`rurge_interop::trusttunnel::render`）是三个 TOML 文件：`vpn.toml`（`listen_address = "127.0.0.1:<端口>"`、`ipv6_available = false`、只开 `[listen_protocols.http2]`、`[forward_protocol]` 为 `direct`，以及 `allow_private_network_connections = true`——默认值 `false` 会让 endpoint 拒绝连回环上的 echo）、`hosts.toml`（一个主机 `tt.test` 与证书、私钥的绝对路径）与 `credentials.toml`（一个用户）。夹具的单元用例 `the_configuration_stays_on_the_loopback` 断言只监听回环、只开 HTTP/2。启动命令是 `trusttunnel_endpoint vpn.toml hosts.toml`，就绪与否看 TCP 端口能否连上。

## tuic-server

TUIC 官方 [tuic](https://github.com/tuic-protocol/tuic) 的服务端 `tuic-server` 是 `tuic`（TUIC v4）的参照：sing-box 只实现 v5（阶段 2 / M7 设计 M7-D3、M7-D6）。它是 GPL-3.0 程序，只作为外部进程运行，rurge 不链接、不复制它的任何代码。互操作测试固定 **0.8.5**（2022-07-30 发布），只在 Linux 上用这一个资产：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (x86_64，静态链接的 musl 版) | `tuic-server-0.8.5-x86_64-linux-musl`（可执行文件本身，不是压缩包） | **尚未钉住，待项目所有者填写**（见下） |

**SHA-256 尚未钉住**：GitHub 发布 API 给这个版本的每个资产的 `digest` 都是 `null`（2022 年上传的资产没有），发布页也不显示校验和，而本仓库的规矩是不在本机下载二进制来自己算。所以 `.github/workflows/ci.yml` 里这一步的 `sha=` 是一个占位值，**在项目所有者填上真实的值之前，Linux 的 CI 任务会在这一步失败**（Windows 与 macOS 的任务不受影响）。发布里每个二进制旁边都有一个 `sha256sum` 格式的同名校验文件（`tuic-server-0.8.5-x86_64-linux-musl.sha256sum`）：在可信的机器上读它，或下载该二进制自己算一次，把 64 位十六进制填进 `sha=`，再填进上表。

`tests/tuic.rs` 里 `v4_…_against_tuic_server` 的三个用例覆盖：TCP 的单块与跨多块（100 000 字节）往返（每条隧道都在服务端应答 `04 ff 00` 之后才交出）；UDP 往返一个回环 UDP 回显两次（v4 不分片，整包走 QUIC 数据报；服务端经它那一个双栈 socket 把 IPv4 来源写成 IPv4 映射的 IPv6 地址，rurge 按 IPv4 读，用例核对来源就是回显的地址）；token 错误时 TCP 请求以 `tuic: authentication failed` 失败（服务端以 0xfffffff1 关闭连接），UDP 发送同样。0.8.5 同样总是接受 QUIC 数据报，**`quic` 模式只由回环假服务端覆盖**。

CI 只在 Linux 上下载、校验并设置 `RURGE_TEST_TUIC_SERVER`；Windows 与 macOS 上这三个用例照常编译，运行时打印 `skipping …` 后返回（`RURGE_INTEROP_REQUIRED=1` 对 tuic-server 只在 Linux 上生效）。本机不安装它：只认 `RURGE_TEST_TUIC_SERVER`，不在 `PATH` 上找（`PATH` 上的 `tuic-server` 多半是 1.x 版，只说 v5，配置也不同），没设就跳过；这个 crate 不会下载或安装它。

渲染出的配置（`rurge_interop::tuic_server::render`）是一个 JSON 对象，只有 0.8.5 认识的七个键（它拒绝不认识的键）：`port`、`token`（一个 token）、`certificate` 与 `private_key`（夹具的证书与私钥，PEM 文件的路径）、`ip`（`127.0.0.1`）、`alpn`（`["h3"]`：它的缺省是不选 ALPN，rurge 的 QUIC 客户端于是握手失败）与 `log_level`（`info`）；其余用它的缺省（Cubic、空闲 15 秒、认证时限 1 秒）。启动命令是 `tuic-server -c <配置文件>`；它没有 TCP 端口可探，就绪与否看日志里出现 `Server started`（`info` 级，绑定端口之后才打印）。夹具的单元用例 `the_configuration_stays_on_the_loopback` 与 `the_configuration_is_written_as_tuic_server_reads_it` 断言只监听回环、只写这七个键。它转发 UDP 用的 socket 绑在所有地址上的一个空闲端口（`[::]:0`；0.8.5 总是这样，没有配置项）。

## sshd

`tests/sshd.rs` 用 OpenSSH 的 `sshd` 验证 `ssh` 出站（阶段 2 M4 设计第 10 节）：OpenSSH 的默认算法，以及只开 Surge 手册要求的 `curve25519-sha256` 与 `aes128-gcm@openssh.com` 两种。不以 root 运行的 `sshd` 只能让运行它的用户登录，所以用例用密钥登录（用户名取环境变量 `USER`），并且只在 Unix 上编译。

查找顺序：`RURGE_TEST_SSHD`，然后 `/usr/sbin/sshd`，然后 `PATH` 上的 `sshd`；都没有就打印 `skipping …` 后返回（`RURGE_INTEROP_REQUIRED=1` 时改为失败）。CI 在 Linux 与 macOS 上设置 `RURGE_TEST_SSHD=/usr/sbin/sshd`，Linux 上缺它时先装 `openssh-server`；Windows 上不编译这两个用例。

渲染出的 `sshd_config`（`rurge_interop::sshd::render`）只监听 `127.0.0.1`，只认用例现场生成的那一把公钥，关掉口令、PAM 与终端，只允许本地端口转发（夹具的单元用例 `the_configuration_stays_on_the_loopback_with_one_key` 断言这一点）；主机密钥同样现场生成，写进临时目录。

## 安全约束

- 夹具渲染出的 sing-box 配置只有 `log` / `inbounds` / `outbounds` 三个顶层键（WireGuard 的配置另有 `endpoints`）；每个入站只监听 `127.0.0.1`（`hysteria2` 与 `tuic` 入站的 UDP 端口也是，同一份配置里另有一个只用来判断就绪的 `mixed` 入站；`rurge_interop::render_hysteria2` 与 `rurge_interop::render_tuic` 的单元测试 `the_hysteria2_configuration_never_touches_the_machine` 与 `the_tuic_configuration_never_touches_the_machine` 断言这些）；唯一的出站是 `direct`。WireGuard 端点在用户态运行（`system: false`），它的 UDP 端口开在所有地址上（端点没有监听地址这一项；`rurge_interop::render_wireguard` 的单元测试 `the_wireguard_configuration_never_touches_the_machine` 断言其余各项）。xray 配置同样只有这三个顶层键，唯一的出站是 `freedom`。`ssserver` 的配置只有 `servers`，每个服务端只听 `127.0.0.1`（TCP 与同号的 UDP）。`snell-server` 的配置只有 `[snell-server]` 一节，只听 `127.0.0.1`。TrustTunnel endpoint 只听 `127.0.0.1`、只开 HTTP/2，转发只有 `direct`；`allow_private_network_connections = true` 只是为了让它连回环上的 echo。tuic-server 的配置只有它认识的七个键，只听 `127.0.0.1`（它转发 UDP 的 socket 绑在所有地址上的一个空闲端口，0.8.5 没有配置项）。任何地方都不出现 `set_system_proxy`、`tun`、`auto_route` 这些键（`rurge_interop::render` 与 `rurge_interop::xray::render` 的单元测试 `the_configuration_never_touches_the_machine` 各自断言这一点）；`shadowtls` 入站的 `handshake.server` 恒为 `127.0.0.1`（夹具的单元用例断言）。
- 每个用例的连接目标都是回环 IP 字面量（`127.0.0.1` 上的 echo / 测试服务器；WireGuard 用例在隧道里连的是 sing-box 自己的隧道地址 `10.9.0.1`，由 sing-box 映射到它的 `127.0.0.1`），sing-box、xray、`ssserver`、`snell-server`、TrustTunnel endpoint 与 `tuic-server` 因此既不解析域名也不会访问公网。
- 这个 crate 本身不下载、不安装任何东西；本机是否装有 sing-box、xray、shadowsocks-rust、snell-server、TrustTunnel endpoint 或 tuic-server 由项目所有者决定，没装就跳过。
