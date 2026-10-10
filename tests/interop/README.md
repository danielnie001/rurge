# rurge-interop

`rurge-interop` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它把 [sing-box](https://sing-box.sagernet.org/)、[xray](https://github.com/XTLS/Xray-core)、[shadowsocks-rust](https://github.com/shadowsocks/shadowsocks-rust) 的 `ssserver`、Surge 官方的 `snell-server`、[TrustTunnel](https://github.com/TrustTunnel/TrustTunnel) 的 `trusttunnel_endpoint`、TUIC 官方 [tuic](https://github.com/tuic-protocol/tuic) 0.8.5 的 `tuic-server`、[masque-server](https://github.com/Vincent-bin/masque-server)、AdGuard [dnsproxy](https://github.com/AdguardTeam/dnsproxy)、[CoreDNS](https://coredns.io/) 与 [mihomo](https://github.com/MetaCubeX/mihomo) 作为参照实现，以回环子进程的方式拉起来，驱动 rurge 的 `http` / `https` / `socks5` / `trojan` / `vmess` / `anytls` / `wireguard` / `ss` / `snell` / `h2-connect` / `trust-tunnel`（HTTP/2 与 `h3=true` 的 HTTP/3）/ `hysteria2` / `tuic-v5` / `tuic` / `masque` 出站、包在 Shadow TLS 里的 `trojan`，以及 `quic://`（DoQ）与 `h3://`（DoH3）两种 DNS 上游，去连它们，验证 rurge 与真实的第三方实现互通。xray 只用来跑 `vmess`：VMess 协议由 xray 所在的这一脉实现定义，sing-box 的实现是重写，手写的编解码需要两个独立参照互相印证（M2 设计 M2-D5）。tuic-server 只用来跑 `tuic`（TUIC v4）：sing-box 只实现 v5（M7 设计 M7-D3）。masque-server 只用来跑 `masque`，dnsproxy 与 CoreDNS 只用来跑 DoQ / DoH3（M7 设计 M7-D6 与第 7.4 节）。mihomo 只用来跑别的参照都没有的三样：`snell` 的 v1 ～ v3、`ss` 的流式方法 `chacha20`，以及挡在这两种协议前面的 simple-obfs（M8 设计 M8-D9、第 4.1 与 4.3 节；simple-obfs 一项是执行期加的，见该设计第 12 节）。

## 固定版本

互操作测试固定 sing-box **1.14.2**（2026-09-24 发布的稳定版；阶段 2 / M6b 从 1.14.1 升上来：`snell` 入站 1.14.0 起才有，1.14.2 是 1.14.x 的最新版；`hysteria2` 入站的 Gecko 混淆同样 1.14.0 起才有，QUIC 支持（构建标签 `with_quic`）是发布包的缺省）。CI 下载并校验以下三个发布包（SHA-256 取自 GitHub 发布 API 每个资产的 `digest`）：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (amd64) | `sing-box-1.14.2-linux-amd64.tar.gz` | `a684484d7477d1437282ee411f4d131d0340aaad60a7868841ebd5d87dd8a0c6` |
| Windows (amd64) | `sing-box-1.14.2-windows-amd64.zip` | `c2d8bfff918755808781dfdeeb8581b6c91eb3a243d9a7b55483cfc0c0684d32` |
| macOS (arm64) | `sing-box-1.14.2-darwin-arm64.tar.gz` | `925c5382eca8492b0150f868a6db20b18290a38700e621724b3703fd453e032d` |

## 本地运行

本地默认不安装 sing-box、xray、shadowsocks-rust、snell-server、TrustTunnel endpoint、tuic-server、masque-server、dnsproxy、CoreDNS 与 mihomo：`cargo test -p rurge-interop` 会正常通过，sing-box 的二十八个互操作用例、xray 的两个、shadowsocks-rust 的三个、snell-server 的五个、TrustTunnel endpoint 的六个、tuic-server 的三个、masque-server 的三个、dnsproxy 的两个、CoreDNS 的两个与 mihomo 的七个互操作用例各打印一行 `skipping …` 后直接返回（各夹具自身的单元测试——配置渲染、"配置绝不碰本机"的安全守卫、取空闲端口——照常运行并断言；`tests/mihomo.rs`、`tests/shadowsocks.rs` 与 `tests/snell.rs` 另有不需要参照服务端的用例：各有一个把文件里新写的策略行在 rurge 这一侧加载并构建一遍，`tests/mihomo.rs` 还有一个对着回环上的四种服务端核对"PSK 或版本不符"那类用例收什么、不收什么，`tests/shadowsocks.rs` 还有两个核对一张表里失败的方法是不是全部报出，见下面「mihomo」「shadowsocks-rust」两节与「覆盖范围」里 Snell 的一条）。要在本机真正跑 sing-box 的用例，二选一：

- 自行安装 sing-box 1.14.2，让 `sing-box` / `sing-box.exe` 出现在 `PATH` 上；或
- 不安装到 `PATH`，改用 `RURGE_TEST_SING_BOX=<sing-box 可执行文件路径> cargo test -p rurge-interop`。

xray、shadowsocks-rust、snell-server、TrustTunnel endpoint、tuic-server、masque-server、dnsproxy、CoreDNS 与 mihomo 的本机运行方式同理（tuic-server 只认环境变量，不在 `PATH` 上找），见下面「xray」「shadowsocks-rust」「snell-server」「TrustTunnel」「tuic-server」「masque-server」「dnsproxy 与 CoreDNS」「mihomo」八节。

## 环境变量

- `RURGE_TEST_SING_BOX`：sing-box 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_TEST_XRAY`：xray 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_TEST_SSSERVER`：shadowsocks-rust 的 `ssserver` 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_TEST_SNELL_SERVER`：Surge 官方 `snell-server` 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_TEST_TRUSTTUNNEL`：TrustTunnel `trusttunnel_endpoint` 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_TEST_TUIC_SERVER`：tuic 0.8.5 的 `tuic-server` 可执行文件的路径；只认这个变量，不在 `PATH` 上找（`PATH` 上的 `tuic-server` 多半是 1.x 版，只说 TUIC v5，配置也不同）。
- `RURGE_TEST_MASQUE_SERVER`：`masque-server` 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_TEST_DNSPROXY`：AdGuard `dnsproxy` 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_TEST_COREDNS`：`coredns` 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_TEST_MIHOMO`：`mihomo` 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_INTEROP_REQUIRED=1`：找不到二进制时让用例直接失败，而不是打印 `skipping …` 后跳过；对 sing-box、xray、shadowsocks-rust、dnsproxy、CoreDNS、mihomo 与 sshd 各夹具都生效，对 snell-server 与 masque-server 只在 Linux 上生效（它们只有 Linux 版），对 TrustTunnel endpoint 只在 Linux 与 macOS 上生效（它没有 Windows 版），对 tuic-server 只在 Linux 上生效（CI 只在 Linux 上装它）。本机一般不需要。CI 在工作流一级的 `env` 里设置它（`.github/workflows/ci.yml`，与 `RUSTUP_TOOLCHAIN` 并排），不由哪一个安装步骤设置：那样的话那一步一失败变量就没有了，全部互操作用例会不声不响地跳过。参照服务端的每个安装步骤（连同把 `RURGE_TEST_SSHD` 指向 `sshd` 的那一步）都带 `continue-on-error: true`：某个服务端装不上（下载失败、资产挪了地方）时任务不在那一步结束，要用它的用例各自以"找不到二进制"失败，其余的用例照常运行、照常上报。

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

- Snell（`tests/snell.rs` 里 `…_against_sing_box` 的五个用例，阶段 2 / M6b，第五个是 M8b 终审时加的）：sing-box 的 `snell` 入站（1.14.0 起），`version: 5`、一个 `psk`、不配 `users`（服务端不看 client id）。sing-box 没有"version 4"的入站；`version: 5` 走 v4 / v5 共用的线上格式，所以同时接受 v4 客户端。覆盖 `version=5` 与 `version=4` 两种客户端的单块与跨多块（100 000 字节）TCP 往返；`reuse=true` 时一连四个请求（每个都两个方向干净结束，连接回池给下一个请求用）再加一个大负载；`obfs=http`（入站的 `obfs_mode: http`；含 `reuse=true` 与 UDP）；以及 UDP over TCP（命令 `0x06`）经同一个载体往返一个回环 UDP 回显两次。第五个用例 `a_line_of_another_version_or_psk_against_sing_box` 问的是连不通的行在真实服务端上会怎样：两行策略——不写 `version` 的一行（缺省是 1，发的是 v1 的分块，这个入站读不出）与 PSK 不对的 `version=4` 一行——各发一个请求，会话必须在 10 秒内以 rurge 带提示的两句话之一失败（`snell: the server closed the connection without answering (wrong psk or version?)` 或 `snell: the server reset the connection without answering (wrong psk or version?)`，逐字比对）。服务端读不出请求就关闭或重置连接时正是这样；它要是留着连接不出声，会话就得等到空闲超时，用例于是失败，失败信息里是每一行实际发生了什么（没有在时限内失败、以别的文本失败——回了解不开的数据时是第三句——、隧道没建起来……）与 sing-box 的日志。两行都试，不因第一行失败而略过第二行。判据是 `common::none_is_answered`，与 mihomo 的同类用例共用（见下面「mihomo」一节）。按源码看 sing-box 读不出请求就关连接，实际如何**等第一次 CI 运行见分晓**。`the_mismatched_lines_load_and_build` 不需要 sing-box：把这两行（端口换成固定的数）在 rurge 这一侧加载并构建一遍，不许有任何诊断。

- Hysteria 2（`tests/sing_box_hysteria2.rs` 的五个用例，阶段 2 / M7a）：sing-box 的 `hysteria2` 入站只听回环上的一个 UDP 端口；证书签给 `127.0.0.1`、由夹具的 CA 签发，rurge 信任这个 CA（服务器写成 IP，不发 SNI）；`tls.alpn` 写 `["h3"]`；一个用户（`users[].password`）；不配 `masquerade`（口令不对时 sing-box 回 404）、不限带宽。覆盖 TCP 的单块、跨多块（100 000 字节）与每个方向 9 MiB 同时往返（超过 rurge 每条流 8 MiB 的接收窗口：回来的数据要靠 rurge 边读边放出的流控额度），这三项再用写了 `download-bandwidth=100` 的策略做一遍（sing-box 于是按这个速率用 Brutal 发，不用 BBR）；UDP：同一条连接上两个载体（各占一个会话号）各自往返几个字节与 3000 字节的数据报，后者两个方向都按 Hysteria 的分片走（sing-box 把应答切成约 1200 字节一片，不回超过 4096 字节的消息；macOS 的回环不送超过 9216 字节的数据报）；口令错误时 TCP 与 UDP 都以 `hysteria2: authentication failed` 失败；`obfs` 的 `salamander` 与 `gecko`（包长用 sing-box 的缺省 512–1200 字节，与 rurge 相同）两种混淆下的 TCP（单块与跨多块）与 UDP。

- TUIC v5（`tests/tuic.rs` 里 `v5_…_against_sing_box` 的三个用例，阶段 2 / M7b）：sing-box 的 `tuic` 入站（只实现 v5）只听回环上的一个 UDP 端口，证书同 Hysteria 2 一节；`tls.alpn` 写 `["h3"]`（rurge 没写 `alpn` 时发 `h3`，QUIC 服务端不选协议时握手失败）；一个用户（`users[]` 的 `uuid` 与 `password`）；拥塞控制、认证时限（3 秒）、心跳（10 秒）与 0-RTT（关）都用 sing-box 的缺省。覆盖 TCP 的单块、跨多块（100 000 字节）与每个方向 9 MiB 同时往返（超过 rurge 每条流 8 MiB 的接收窗口）；UDP：同一条连接上两个载体（各占一个关联号）各自往返几个字节与 3000 字节的数据报，后者两个方向都按 TUIC 的分片走（sing-box 把应答切成至多 1197 字节的数据报）；口令错误：sing-box 不回应 v5 的认证，令牌不对时以码 0、空原因关闭连接（它因任何错误关连接都是这样），而且常常先结束手上挂着的 Connect（STOP_SENDING 0 加 FIN），所以 TCP 隧道照样先建立（v5 的隧道一律立即交出），然后什么也没转发就结束——关闭先到时读到 `tuic: authentication failed`，Connect 的结束先到时读到流的结束，写可能遇到 `tuic: the server stopped the stream (code 0x0)`；UDP 没有这种竞争，发现连接被这样关闭的那次发送以 `tuic: authentication failed` 失败。**只覆盖 `native` 模式**：sing-box 的 TUIC 服务端总是接受 QUIC 数据报（配置里没有关掉它的项），`quic` 模式（服务端不接受数据报时每个包一条单向流）只由回环假服务端（`rurge_proto_quic::testing::FakeTuic` 的 `no_datagrams`）覆盖。

**不覆盖端口跳跃**：`port-hopping`（Hysteria 2、TUIC 与 MASQUE）要在服务端用防火墙把一段端口 DNAT 到它的监听端口（Hysteria 文档的部署方式），夹具不改本机的防火墙与网络设置，回环上做不到；端口跳跃由回环假服务端（`rurge_proto_quic::testing::FakeHysteria2`、`FakeTuic` 与 `FakeH3Proxy` 同时监听几个端口，跳跃间隔经只用于测试的入口缩短）与手工验收（`docs/acceptance/phase2-manual.md` 的 M7a、M7b 与 M7c 三节）覆盖。

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

`tests/shadowsocks.rs` 里对 `ssserver` 的三个用例覆盖 `ss`：

- `aead_methods_against_ssserver`：`aes-128-gcm`、`aes-256-gcm`、`chacha20-ietf-poly1305`。
- `ss_2022_against_ssserver_with_one_key_and_as_a_user`：单密钥的 `2022-blake3-aes-128-gcm` 与 `2022-blake3-aes-256-gcm`，以及 `2022-blake3-aes-256-gcm` 两个用户里的第二个（`ssserver` 配置的 `users`，SIP023 的身份头）。
- `stream_methods_against_ssserver`（阶段 2 / M8b）：九种流式方法 `rc4`、`rc4-md5`、`aes-128-cfb`、`aes-192-cfb`、`aes-256-cfb`、`aes-128-ctr`、`aes-192-ctr`、`aes-256-ctr`、`chacha20-ietf`，每种一个服务端，各做单块与跨多块（100 000 字节）的 TCP 往返；其中 `aes-256-cfb`（16 字节 IV）与 `chacha20-ietf`（12 字节 IV）另经 `udp-relay=true` 往返一个回环 UDP 回显两次。流式方法没有任何校验，字节原样回来就是密钥、IV 与密钥流都对上的证明。九种方法逐个都试，不因前面的失败而停：失败的方法全部记下，用例在最后失败一次——先是"几种里失败了几种"，再是每种失败的方法一行（方法名与那一步自己的 panic 文本），最后是 `ssserver` 的日志，只一遍（`common::attempt` 与 `common::Failures`，见下面「mihomo」一节「覆盖」里讲 `common::attempt` 的那一段）。

前两个用例的每种方法都做单块与跨多块（100 000 字节）的 TCP 往返，以及经 `udp-relay=true` 往返一个回环 UDP 回显两次（`ssserver` 的 `mode` 是 `tcp_and_udp`，UDP 端口与 TCP 端口同号）。

`ssserver` 的发布包按 `full` 特性构建（v1.25.0 的 `.github/workflows/build-release.yml`：Linux 的交叉构建传 `-f full`，macOS 设 `BUILD_EXTRA_FEATURES: "full"`，Windows 传 `"full winservice"`；`build/build-release`、`build/build-host-release` 与 `build/build-host-release.ps1` 把它交给 `cargo build --features`）。`full`（`Cargo.toml`）含 `stream-cipher`，所以流式方法在发布包里；它不含 `aead-cipher-extra`，所以发布包不带 `aes-192-gcm` 与 `xchacha20-ietf-poly1305`，这两种由 sing-box 的 `shadowsocks` 入站覆盖（见上面「覆盖范围」），另有回环假服务端（`rurge_proto::testing::FakeShadowsocks`）。`none` 只由假服务端覆盖。

流式方法在 `ssserver` 的配置里只是换一个 `method`，没有别的开关：方法名由 shadowsocks-crypto 0.8.0（`crates/shadowsocks/Cargo.toml` 钉的版本，`stream-cipher` 即它的 `v1-stream` 特性）的 `src/kind.rs` 按小写比对，上面九个名字逐字都在（`aes-*-cfb` 是 CFB128，`chacha20-ietf` 是 12 字节 nonce 的那种，`rc4` 的密钥 16 字节、没有 IV）；服务端启动时只对流式方法记一条告警（`crates/shadowsocks-service/src/server/mod.rs`：`stream cipher … have inherent weaknesses … DO NOT USE`）。它没有 `salsa20`，也没有 8 字节 nonce 的 `chacha20`：后者由 mihomo 覆盖（见下面「mihomo」一节）；**`salsa20` 没有参照**——只有 shadowsocks-libev 支持它，而 libev 没有官方的预编译二进制——它由已知答案的向量（ECRYPT 的 Salsa20/20）与回环假服务端覆盖。以上各项读自 v1.25.0 与 shadowsocks-crypto v0.8.0 的源码，还没有对真实的二进制跑过，第一次 CI 运行时核对。

`the_stream_method_policies_load_and_build` 不需要 `ssserver`：它把上面那个用例的九行策略（端口换成固定的数）在 rurge 这一侧加载一遍——除了"用的是流式方法"（`W0031`）不许有别的诊断——并各构建出出站，本机与 CI 都跑；参数名写错这类只在 CI 上才露头的毛病在这里就拦住。`every_method_that_fails_is_told_not_the_first_alone` 与 `a_method_that_fails_fails_its_test_at_the_end` 同样不需要 `ssserver`：前者让三步里的两步失败，核对两条失败都在报告里、各占一行（panic 文本里的换行并成一行）、日志在最后且只有一遍，没有失败时不读日志；后者核对带着失败的用例确实在最后以这份报告失败。

**流式方法不写"口令错误"的用例**：流式方法没有校验，口令不对时服务端解出的是一串乱码。`ssserver` 读不出目标地址时并不断开，而是一直读到客户端自己结束（`crates/shadowsocks-service/src/server/tcprelay.rs` 的 `tcp silent-drop`，为了不给主动探测留下特征）；乱码碰巧像一个地址时，它会真的去解析、去连那个地址。前一种没有"限时失败"可断言，后一种会让参照服务端碰公网，所以这种情形只由回环假服务端覆盖（它只连回环地址本身）。

**`ssserver` 不覆盖 obfs**：simple-obfs 已停止维护，是 `ssserver` 之外的一个插件，sing-box 也不带；`http` / `tls` 两种 obfs 改由 mihomo 的监听端覆盖（见下面「mihomo」一节；阶段 2 / M8b 之前它们只有假服务端 `rurge_proto::testing::accept_obfs` 与手工验收 `docs/acceptance/phase2-manual.md` 的 M6a 一节）。

本机不安装 shadowsocks-rust：`RURGE_TEST_SSSERVER`（优先于 `PATH` 查找）没有指向可执行文件、`PATH` 上也找不到 `ssserver` / `ssserver.exe` 时，用例打印一行 `skipping …` 后直接返回；这个 crate 不会下载或安装它。互操作由首次推送后的 CI 证明（CI 安装 shadowsocks-rust v1.25.0 并设置 `RURGE_TEST_SSSERVER` 与 `RURGE_INTEROP_REQUIRED=1`）。

渲染出的配置（`rurge_interop::shadowsocks_rust::render`）只有 `servers` 一个顶层键：每个服务端只监听 `127.0.0.1`，`mode` 为 `tcp_and_udp`，没有 `manager`、`locals`、插件、ACL 与 `outbound_*` 这些键（夹具的单元用例 `the_configuration_stays_on_the_loopback` 断言这一点）。一个 `ssserver` 进程的全部端口由 `rurge_interop::free_ports` 一次取齐：每个都对 TCP 与同号的 UDP 空闲（`tcp_and_udp` 两个都要绑），彼此不同（流式方法的用例一次开九个服务端）。

## snell-server

Surge 官方的 `snell-server`（发布在 Surge 知识库）是 `snell` 的第二个参照：sing-box 的实现是重写，线上格式需要两个独立参照互相印证（阶段 2 / M6 设计 M6-D5）。互操作测试固定 **v5.0.1**，只有 Linux (amd64) 版：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (amd64) | `snell-server-v5.0.1-linux-amd64.zip`（`https://dl.nssurge.com/snell/`） | `9bea1c2b9e35b73b31634856c04d18c393072b9e5dcde6a32781d8b8f908c539` |

知识库不公布校验和；上面的值是计划期对该 zip 自行计算的（解压出的 `snell-server` 的 SHA-256 为 `5b2e221f2c6e29b1db8e47053e1221be29d5627da807cb932b089f514a3609f0`，与第三方项目钉住的值一致）。二进制经 UPX 压缩。

`tests/snell.rs` 里 `…_against_snell_server` 的五个用例覆盖：`version=5` 与 `version=4` 两种客户端的单块与跨多块 TCP 往返；`reuse=true` 时一连四个请求加一个大负载；`obfs = http`（含 `reuse=true` 与 UDP）；UDP over TCP 往返一个回环 UDP 回显两次；以及 `a_line_of_another_version_or_psk_against_snell_server`（阶段 2 / M8b 终审时加）——与对 sing-box 的那一个相同的两行（不写 `version` 的，与 PSK 不对的 `version=4`）、相同的判据（见上面「覆盖范围」里 Snell 的一条）。官方服务端不开源：它读不出请求时是关闭连接、重置连接，还是留着连接不出声，**只有第一次 CI 运行才知道**；兼容性清单 4.2 `snell` 一行说没写 `version` 的行对 v4 / v5 服务端会很快失败，这对真实的服务端是否成立，就看这个用例与对 sing-box 的那一个。

CI 只在 Linux 上下载、校验并设置 `RURGE_TEST_SNELL_SERVER`；Windows 与 macOS 上这五个用例照常编译，运行时打印 `skipping …` 后返回（`RURGE_INTEROP_REQUIRED=1` 对 snell-server 只在 Linux 上生效）。本机不安装它：`RURGE_TEST_SNELL_SERVER`（优先于 `PATH` 查找）没有指向可执行文件、`PATH` 上也找不到 `snell-server` 时同样跳过；这个 crate 不会下载或安装它。

渲染出的配置（`rurge_interop::snell_server::render`）是一个 `[snell-server]` 节：`listen = 127.0.0.1:<端口>`、`psk`、`ipv6 = false`，按需加 `obfs = http`；没有 `dns`、`egress-interface` 这些键（夹具的单元用例 `the_configuration_stays_on_the_loopback` 断言只监听回环）。启动命令是 `snell-server -c <配置文件>`，就绪与否看 TCP 端口能否连上。

## TrustTunnel

[TrustTunnel](https://github.com/TrustTunnel/TrustTunnel) 的 endpoint 是 `trust-tunnel` 的参照实现（HTTP/2，以及 `h3=true` 的 HTTP/3）；它的 TCP 模式就是标准的 HTTP/2 CONNECT（`:authority` 为 `host:port`，`proxy-authorization: Basic`，200 / 407 / 502），所以同时充当 `h2-connect` 普通 CONNECT 的参照服务端（sing-box 1.14.2 的 `http` 入站只说 HTTP/1，见上面「覆盖范围」末尾）；它的 QUIC 监听（`[listen_protocols.quic]`）在 HTTP/3 上收同样的普通 CONNECT（只有 `:method` 与 `:authority`，没有 extended CONNECT，也不开 QUIC 数据报）。互操作测试固定 **v1.1.0**（2026-09-01 发布），只有 Linux 与 macOS 版。CI 下载并校验以下两个发布包（SHA-256 取自 GitHub 发布 API 每个资产的 `digest`）：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (x86_64) | `trusttunnel-v1.1.0-linux-x86_64.tar.gz` | `91c2ea3db7416a01b5258a4c047ec22890490bc55e1b194206031aa75144f0e7` |
| macOS (universal) | `trusttunnel-v1.1.0-macos-universal.tar.gz` | `126a5688e922ce8f83d4de2d8a2659b3aab97820184d30f2edbecc0ec4a32ae5` |

发布包里是一个以资产名命名的目录，内有 `trusttunnel_endpoint`（另有 `setup_wizard`、签名文件与 `LICENSE`，不用）。

`tests/http2.rs` 驱动的六个用例覆盖：

- `trust-tunnel`：单块与跨多块（100 000 字节）的 TCP 往返；口令错误时 endpoint 回 407，出站报 `trust-tunnel: authentication failed`。
- `trust-tunnel` 的 `h3=true`（阶段 2 / M7c）：endpoint 只开 QUIC 监听（没有 HTTP/2 可以退回），单块与跨多块的 TCP 往返；口令错误时同样回 407，出站报同一个 `trust-tunnel: authentication failed`。
- `h2-connect`：凭据写在端口之后（位置参数）与写成 `username=` / `password=` 两种，各做单块与跨多块的 TCP 往返；`max-streams=3` 时同时打开七条隧道并全部保持（多于一条连接能承载的流，池要开新连接），每条各自回显自己的字节，之后再开一条新隧道。

**不覆盖 HTTP/2 上的 CONNECT-UDP**：TrustTunnel 的 UDP 走它自己的 `_udp2` 复用格式，不是 RFC 9298 的 CONNECT-UDP；目前找不到可用的稳定版参照服务端。`h2-connect` 的 CONNECT-UDP 由回环假服务端（`rurge_proto::testing::FakeH2Proxy`，开 `enable_connect_protocol`）与手工验收（`docs/acceptance/phase2-manual.md` 的 M6c 一节）覆盖；HTTP/3 上的 CONNECT-UDP（`masque`）由 masque-server 覆盖（见下面「masque-server」一节）。sing-box 的预发布版（1.15.0-alpha.7 起）给 `http` 入站加了 HTTP/2、HTTP/3 与 CONNECT-UDP，三平台都有发布包，是将来两种 CONNECT-UDP 与 HTTP/3 普通 CONNECT 的跨平台参照；它还是 alpha，现在不用。

endpoint 按 SNI 精确匹配它的主机（`main_hosts[].hostname`），所以夹具的证书签给 `tt.test`，rurge 的每一行都写 `sni=tt.test` 并信任夹具的 CA；ALPN 固定 `h2`（不发 ALPN 时 endpoint 按 HTTP/1 处理），`h3=true` 时固定 `h3`。

CI 只在 Linux 与 macOS 上下载、校验并设置 `RURGE_TEST_TRUSTTUNNEL`；Windows 上这六个用例照常编译，运行时打印 `skipping …` 后返回（`RURGE_INTEROP_REQUIRED=1` 对它只在 Linux 与 macOS 上生效）。本机不安装它：`RURGE_TEST_TRUSTTUNNEL`（优先于 `PATH` 查找）没有指向可执行文件、`PATH` 上也找不到 `trusttunnel_endpoint` 时同样跳过；这个 crate 不会下载或安装它。

渲染出的配置（`rurge_interop::trusttunnel::render`）是三个 TOML 文件：`vpn.toml`（`listen_address = "127.0.0.1:<端口>"`、`ipv6_available = false`、只开一种协议——HTTP/2 时 `[listen_protocols.http2]`，HTTP/3 时 `[listen_protocols.quic]`（空表即启用，各项用 endpoint 的缺省）——、`[forward_protocol]` 为 `direct`，以及 `allow_private_network_connections = true`——默认值 `false` 会让 endpoint 拒绝连回环上的 echo）、`hosts.toml`（一个主机 `tt.test` 与证书、私钥的绝对路径）与 `credentials.toml`（一个用户）。夹具的单元用例 `the_configuration_stays_on_the_loopback` 断言只监听回环、每种模式只开它那一种协议。启动命令是 `trusttunnel_endpoint vpn.toml hosts.toml`，就绪与否看 TCP 端口能否连上；HTTP/3 时再等它的日志（标准输出）里出现 `Listening to UDP`。endpoint 总在 `listen_address` 上开 TCP 监听（只开 QUIC 时它对 TCP 连接只接受、不处理），QUIC 监听绑在同一个端口号的 UDP 上，所以 HTTP/3 时夹具挑一个 TCP 与 UDP 都空闲的端口（不能写 0：两边会拿到不同的临时端口，日志打印的也只是配置里的地址）。QUIC 监听的这一部分——空的 `[listen_protocols.quic]` 表、TCP 与 UDP 同一个端口号、就绪日志 `Listening to UDP`——是按计划 P20 从 v1.1.0 的源码读出的，还没有对真实的二进制跑过：第一次在 Linux / macOS CI 上运行时核对（M7c 计划「延后事项」#1；CONNECT 的 2xx 之后的 trailers 见下面「masque-server」一节末尾）。

## tuic-server

TUIC 官方 [tuic](https://github.com/tuic-protocol/tuic) 的服务端 `tuic-server` 是 `tuic`（TUIC v4）的参照：sing-box 只实现 v5（阶段 2 / M7 设计 M7-D3、M7-D6）。它是 GPL-3.0 程序，只作为外部进程运行，rurge 不链接、不复制它的任何代码。互操作测试固定 **0.8.5**（2022-07-30 发布），只在 Linux 上用这一个资产：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (x86_64，静态链接的 musl 版) | `tuic-server-0.8.5-x86_64-linux-musl`（可执行文件本身，不是压缩包） | `e4b5d5dcc15209f28de1c8a9ea12edba8a71c35442b4c53177309f728406726c` |

**SHA-256 取自发布自带的校验文件**：GitHub 发布 API 给这个版本的每个资产的 `digest` 都是 `null`（2022 年上传的资产没有），发布页也不显示校验和，而本仓库的规矩是不在本机下载二进制来自己算。发布里每个二进制旁边都有一个 `sha256sum` 格式的同名校验文件，上表的值就是 `tuic-server-0.8.5-x86_64-linux-musl.sha256sum` 里写的那一个（读的只是这份 102 字节的文本，没有下载二进制）；`.github/workflows/ci.yml` 里这一步的 `sha=` 与它相同（阶段 2 / M7c 收尾时钉住，此前是占位值、Linux 的 CI 任务在这一步失败）。校验文件与二进制出自同一个发布：钉住它保证的是以后下载到的仍是这一个文件，不是另一条独立的来源。CI 的这一步仍先核对 `sha=` 是 64 位十六进制，再下载、比对。

`tests/tuic.rs` 里 `v4_…_against_tuic_server` 的三个用例覆盖：TCP 的单块与跨多块（100 000 字节）往返（每条隧道都在服务端应答 `04 ff 00` 之后才交出）；UDP 往返一个回环 UDP 回显两次（v4 不分片，整包走 QUIC 数据报；服务端经它那一个双栈 socket 把 IPv4 来源写成 IPv4 映射的 IPv6 地址，rurge 按 IPv4 读，用例核对来源就是回显的地址）；token 错误时 TCP 请求以 `tuic: authentication failed` 失败（服务端以 0xfffffff1 关闭连接），UDP 发送同样。0.8.5 同样总是接受 QUIC 数据报，**`quic` 模式只由回环假服务端覆盖**。

CI 只在 Linux 上下载、校验并设置 `RURGE_TEST_TUIC_SERVER`；Windows 与 macOS 上这三个用例照常编译，运行时打印 `skipping …` 后返回（`RURGE_INTEROP_REQUIRED=1` 对 tuic-server 只在 Linux 上生效）。本机不安装它：只认 `RURGE_TEST_TUIC_SERVER`，不在 `PATH` 上找（`PATH` 上的 `tuic-server` 多半是 1.x 版，只说 v5，配置也不同），没设就跳过；这个 crate 不会下载或安装它。

渲染出的配置（`rurge_interop::tuic_server::render`）是一个 JSON 对象，只有 0.8.5 认识的七个键（它拒绝不认识的键）：`port`、`token`（一个 token）、`certificate` 与 `private_key`（夹具的证书与私钥，PEM 文件的路径）、`ip`（`127.0.0.1`）、`alpn`（`["h3"]`：它的缺省是不选 ALPN，rurge 的 QUIC 客户端于是握手失败）与 `log_level`（`info`）；其余用它的缺省（Cubic、空闲 15 秒、认证时限 1 秒）。启动命令是 `tuic-server -c <配置文件>`；它没有 TCP 端口可探，就绪与否看日志里出现 `Server started`（`info` 级，绑定端口之后才打印）。夹具的单元用例 `the_configuration_stays_on_the_loopback` 与 `the_configuration_is_written_as_tuic_server_reads_it` 断言只监听回环、只写这七个键。它转发 UDP 用的 socket 绑在所有地址上的一个空闲端口（`[::]:0`；0.8.5 总是这样，没有配置项）。以上命令行、七个键与就绪日志是按计划 P14 从 0.8.5 的源码读出的，还没有对真实的二进制跑过：第一次在 Linux CI 上运行时核对（M7b 计划「延后事项」#6）。

## masque-server

[masque-server](https://github.com/Vincent-bin/masque-server)（MIT）是 `masque` 的参照（阶段 2 / M7 设计 M7-D6）：经典 CONNECT 承载 TCP，RFC 9298 的 CONNECT-UDP 加 HTTP/3 数据报（RFC 9297）承载 UDP；每个 CONNECT 都要带 `proxy-authorization: Basic`（按 Argon2id 散列核对，不对就回 407）。它建在 quiche 0.29.3（BoringSSL）上，SETTINGS 通告 extended CONNECT 与 `H3_DATAGRAM`，并照 quiche 的缺省发 GREASE（保留的设置、保留类型的单向流、应答头之前的保留帧）。互操作测试固定 **v0.13.0**（2026-09-02 发布）。服务端只有 Linux 版（macOS 只发诊断工具 `masque-probe`，没有 Windows 版），CI 只在 Linux 上下载并校验这一个资产（SHA-256 取自 GitHub 发布 API 的 `digest`）：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (x86_64，静态链接的 musl 版) | `masque-v0.13.0-linux-x86_64.tar.gz` | `0ae2e3359db5149670df0d084e86104bf0332d0fd43501c9072ec39a48a1f298` |

发布包里是一个顶层目录 `masque-v0.13.0-linux-x86_64/`，服务端是其中的 `bin/masque-server`（另有 `masque-probe`、示例配置、systemd 与监控文件，不用）。

`tests/masque.rs` 的三个用例覆盖：TCP 的单块、跨多块（100 000 字节）与每个方向 9 MiB 同时往返（超过 rurge 每条流 8 MiB 的接收窗口：回来的数据要靠 rurge 读 DATA 帧时放出的流控额度）；UDP：同一个载体轮流发往两个回环 UDP 回显、各三轮——每个目标随它的第一个包开一条 CONNECT-UDP 流（对称型），包走 HTTP/3 数据报（masque-server 在 HTTP/3 上不读 capsule，rurge 也不发），每个回包都来自它所发往的目标；口令错误：服务端回 407，TCP 隧道与发往一个目标的第一个 UDP 包（它的 CONNECT-UDP 流同样被拒）都以 `masque: proxy authentication required` 失败。不覆盖端口跳跃（见上面「覆盖范围」末尾）、服务端不支持 CONNECT-UDP 的情形与流上收到的 DATAGRAM capsule：它们由回环假服务端（`rurge_proto_quic::testing::FakeH3Proxy`）覆盖。

CI 只在 Linux 上下载、校验并设置 `RURGE_TEST_MASQUE_SERVER`；Windows 与 macOS 上这三个用例照常编译，运行时打印 `skipping …` 后返回（`RURGE_INTEROP_REQUIRED=1` 对 masque-server 只在 Linux 上生效）。本机不安装它：`RURGE_TEST_MASQUE_SERVER`（优先于 `PATH` 查找）没有指向可执行文件、`PATH` 上也找不到 `masque-server` 时同样跳过；这个 crate 不会下载或安装它。

渲染出的配置（`rurge_interop::masque_server::render`）是一个 TOML 文件，它拒绝不认识的键，所以只写这些：`[tls]`（夹具的证书与 PKCS#8 私钥，PEM 文件的绝对路径——它经 BoringSSL 的 `PEM_read_bio_PrivateKey` 读私钥，PKCS#8 照收，不必像它的证书脚本那样转成 SEC1）；一个 `[[listeners]]`（`listen_addr = "127.0.0.1:<端口>"`、`transport = "http3"`、`shards = 1`）与它的 `[listeners.auth]`（`mode = "basic"`，一个用户：masque-server 自己的端到端测试用的 `test`，存口令 `test-password` 的 Argon2id 散列）；`[tcp_proxy]` 与 `[udp_proxy]` 都是 `allow_targets = ["0.0.0.0/0", "::/0"]`、`deny_targets = []`（它的缺省拒绝回环与私网目标，回环上的 echo 要放开）；`[ip_proxy] enabled = false`（不开 CONNECT-IP，免得它去建 TUN 网卡）；没有 `[observability]`（不开监控端口）。夹具的单元用例 `the_configuration_stays_on_the_loopback` 与 `the_configuration_is_written_as_masque_server_reads_it` 断言只有这一个监听、只听回环、各键逐字如上。启动命令是 `masque-server --config <配置文件>`，环境里的 `RUST_LOG` 去掉（用它自己的缺省日志级别）；它只开 UDP，没有 TCP 端口可探，就绪与否看日志（标准错误）里出现 `server ready`（全部监听绑定之后才打印）。策略里的服务器写成 `127.0.0.1`，rurge 不发 SNI（它的 TLS 也不看 SNI）；夹具的证书签给 `127.0.0.1`，rurge 信任夹具的 CA。它转发 CONNECT-UDP 用的 socket 绑在 `0.0.0.0:0`（所有地址上的一个空闲端口；目标是 IPv6 时是 `[::]:0`）上，再 `connect` 到目标（v0.13.0 总是这样，没有配置项）；用例的目标都是回环上的回显，这个 socket 于是只收它连到的那个回环地址发来的包。以上命令行、配置的各个键与就绪日志是按计划 P20 从 v0.13.0 的源码读出的，还没有对真实的二进制跑过：第一次在 Linux CI 上运行时核对（M7c 计划「延后事项」#1）。另有一处只有真实运行才看得出：服务端若在 CONNECT 的 2xx 之后再发 HEADERS（trailers），rurge 照 RFC 9114 4.4 判为连接错误（H3_FRAME_UNEXPECTED，`h3: the server sent HEADERS on a CONNECT tunnel`），整条连接连同其上的隧道一起关闭；masque-server（与 TrustTunnel endpoint 的 QUIC 监听）会不会这样做，同样等第一次 CI 运行见分晓。

## dnsproxy 与 CoreDNS

DoQ（`quic://`）与 DoH3（`h3://`）上游有两个参照服务端（阶段 2 / M7 设计第 7.4 节），都是 Apache-2.0、三个平台都有发布包、都建在 quic-go 上：AdGuard 的 [dnsproxy](https://github.com/AdguardTeam/dnsproxy) 与 [CoreDNS](https://github.com/coredns/coredns)。dnsproxy 读到流的 FIN 才作答（rurge 写完查询即发 FIN），同一来源的第一条连接要先过地址校验（QUIC Retry，quinn 自己处理）；CoreDNS 是严格的那个——DoQ 上报文 ID 不是 0 或带 edns-tcp-keepalive 选项时它不作答、以 DOQ_PROTOCOL_ERROR 关掉整条连接（RFC 9250 4.3.3），DoH3 只在 `/dns-query` 上作答，`h3://` 不写路径时 rurge 用的正是它。互操作测试固定 dnsproxy **v0.86.0**（2026-10-01 发布）与 CoreDNS **v1.14.7**（2026-08-19 发布），CI 下载并校验以下六个发布包（SHA-256 取自 GitHub 发布 API 每个资产的 `digest`）：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (amd64) | `dnsproxy-linux-amd64-v0.86.0.tar.gz` | `17ac43f76ebfcedf547c663f6f31aa70e68b8eb6fc41d2fc37d3bfe21a2ced66` |
| Windows (amd64) | `dnsproxy-windows-amd64-v0.86.0.zip` | `dedc186ddd4b96bf92474f91d972f67da0824e0141d09e3102fd96db9f8a0dbc` |
| macOS (arm64) | `dnsproxy-darwin-arm64-v0.86.0.tar.gz` | `5964db3a45d39df4e41f74907f50657a80907c514355f9ad75b9666e3431d277` |
| Linux (amd64) | `coredns_1.14.7_linux_amd64.tgz` | `9a09356438c6d9591cb80011a2a7a52f87fc3a113f6690e41f243f98b98faac9` |
| Windows (amd64) | `coredns_1.14.7_windows_amd64.tgz` | `19756e969e4ddd16ee0c189018e5659f6695113223e590709222433f7338504d` |
| macOS (arm64) | `coredns_1.14.7_darwin_arm64.tgz` | `0005d154d49c73a88b2c56f429f35106e46c4ca37ee6de4e4d482d7de5c4b8ba` |

dnsproxy 的包里是一个 `<系统>-<架构>/` 目录，内有 `dnsproxy`（Windows 是 `dnsproxy.exe`）与 `LICENSE`、`README.md`、`config.dist.yaml`；CoreDNS 的包里只有一个 `coredns`（Windows 是 `coredns.exe`），在包的顶层（Windows 另有内容相同的 `.zip`，这里用 `.tgz`，Git Bash 的 `tar` 能解）。

`tests/dns.rs` 的四个用例——两个服务端各一个 DoQ、一个 DoH3——都问同一个名字（`rurge_interop::DNS_NAME`，A 与 AAAA 各一个文档地址，服务端从自己的配置作答、不往外转发），经同一份配置的两个解析器：一个直连（与 `rurge run` 相同）；另一个的连接器不给 QUIC 直连的 socket、只经 `connect_udp` 给载体并数它们（与 `encrypted-dns-follow-outbound-mode` 下 QUIC 走的路径相同），问两次（第二次越过缓存），这四个问题必须都在一条连接上（载体只开了一个）。每次都核对两个地址与答案的来源（`quic://127.0.0.1:<端口>`；`h3://127.0.0.1:<端口>` 不写路径，来源是 `…/dns-query`）。解析器只用共用的根证书库校验服务器的证书，没有加入测试 CA 的入口，所以配置写 `encrypted-dns-skip-cert-verification = true`（证书本身仍是夹具的 CA 签给 `127.0.0.1` 的）。

CI 在三个平台上都下载、校验并设置 `RURGE_TEST_DNSPROXY` 与 `RURGE_TEST_COREDNS`（`RURGE_INTEROP_REQUIRED=1` 对两者都生效）。本机不安装它们：`RURGE_TEST_DNSPROXY` / `RURGE_TEST_COREDNS`（优先于 `PATH` 查找）没有指向可执行文件、`PATH` 上也找不到 `dnsproxy` / `coredns`（Windows 上带 `.exe`）时打印 `skipping …` 后返回；这个 crate 不会下载或安装它们。下面两小节里的命令行、Corefile 的写法（含引号）与就绪日志是按计划 P20 从 dnsproxy v0.86.0 与 CoreDNS v1.14.7 的源码读出的，还没有对真实的二进制跑过：第一次 CI 运行时核对（三个平台都装，M7c 计划「延后事项」#1）。

### dnsproxy

命令行（`rurge_interop::dnsproxy::args`）：`--listen 127.0.0.1`；`--port <端口>`（普通 DNS 的 UDP 与 TCP：不写时它要占 53；用例不用它）；`--quic-port <端口>`（DoQ）；`--https-port <端口>` 与 `--http3`（TCP 上的 DoH 与同一端口号 UDP 上的 DoH3，所以挑一个 TCP 与 UDP 都空闲的端口，普通 DNS 的端口同理）；`--tls-crt` / `--tls-key`（夹具的证书与私钥）；`--hosts-files <文件>`（`rurge_interop::dnsproxy::hosts`：`DNS_NAME` 的两个地址，名字小写）；`--upstream 127.0.0.1:9`（它必须有一个上游：回环的 9 号端口，没有用例的问题会到那里——hosts 文件答不了的问题没有用例去问。开着 discard 服务的机器上（开发机就是）那里是"接了不答"，CI 上它关着、连接被拒绝；两种都与用例无关）。就绪与否看日志（标准输出）里先后出现 `listening quic` 与 `listening to h3`。夹具的单元用例 `the_command_line_stays_on_the_loopback` 与 `the_command_line_is_written_as_dnsproxy_reads_it` 断言只听 `127.0.0.1`、唯一的上游是 9 号端口、没有 `--pprof` 之类的开关。

### CoreDNS

Corefile（`rurge_interop::coredns::render`）是两个服务块：`quic://.:<端口>`（DoQ）与 `https3://.:<端口>`（DoH3，1.13.2 起），各自 `bind 127.0.0.1`、`tls <证书> <私钥>`（路径加引号：Corefile 的引号里只转义引号，Windows 路径的反斜杠照写）与两个 `template`（`IN A` 与 `IN AAAA`，区域是 `DNS_NAME`，`answer "{{ .Name }} 60 IN A …"`）；没有 `forward`——CoreDNS 只运行 Corefile 写到的插件。启动命令是 `coredns -conf <Corefile>`；就绪与否看它启动完成后在标准输出列出的 `quic://.:<端口> on 127.0.0.1` 与 `https3://.:<端口> on 127.0.0.1` 两行。夹具的单元用例 `the_corefile_stays_on_the_loopback`、`the_corefile_is_written_as_coredns_reads_it` 与 `paths_are_quoted` 断言只有这两个服务块、各自只绑 `127.0.0.1`、没有 `forward`。

## mihomo

[mihomo](https://github.com/MetaCubeX/mihomo)（原 Clash.Meta）是 `snell` v1 ～ v3 的参照（阶段 2 / M8 设计 M8-D9 与第 4.3 节）：sing-box 的 `snell` 入站与 Surge 官方的 snell-server v5 都只说 v4 / v5 的线上格式，mihomo 的 `snell` 监听端按配置的 `version` 说 1 ～ 5 当中的一种。它同时是 `ss` 的 `chacha20`（8 字节 nonce 的原始 ChaCha20）唯一的参照（M8 设计第 4.1 节），也是第一个会说 simple-obfs 的参照服务端：在它之前，`obfs=http` / `obfs=tls` 只有 rurge 自己的回环假服务端。它是 GPL-3.0 程序，只作为外部进程运行，rurge 不链接、不复制它的任何代码。本节的事实都读自 v1.19.32 的源码（括号里是文件，`https://raw.githubusercontent.com/MetaCubeX/mihomo/v1.19.32/<文件>`），**还没有对真实的二进制跑过**：本机不装它，第一次 CI 运行时核对，要核对的项目列在本节末尾。

互操作测试固定 **v1.19.32**。CI 在三个平台上下载并校验以下发布包（SHA-256 取自 GitHub 发布 API 每个资产的 `digest`，2026-10-08）：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (amd64) | `mihomo-linux-amd64-v1-v1.19.32.gz` | `306f81e723e60ce6b828899a6fe83e1d00e9ecefb2dc8d4d849312a5bc00efdc` |
| Windows (amd64) | `mihomo-windows-amd64-v1-v1.19.32.zip` | `00549b347d8e124600e46cac67d59293358454f465e9d86d2511567da56b7029` |
| macOS (arm64) | `mihomo-darwin-arm64-v1.19.32.gz` | `3312a6780652c622890fd4357c6a853bbf865464fd047ac7b7f52dab8de18652` |

amd64 的两个故意选 `-v1` 构建（x86-64-v1，任何 x86-64 的 runner 都能跑）。`.gz` 是可执行文件本身的 gzip（不是 tar），CI 把它解成 `mihomo`；`.zip` 里按 `*.exe` 找（里面的文件叫什么没有核对；CI 不改名，用 `RURGE_TEST_MIHOMO` 指过去）。

### 覆盖

`tests/mihomo.rs` 的七个互操作用例，目标都是回环上的回显：

- `snell_v1_to_v3_against_mihomo`：`version` 为 1、2、3 的监听端各一个，另有一个 v1 的给不写 `version` 的策略行（缺省是 1）；各做单块与跨多块（100 000 字节）的 TCP 往返。v1 的块用 ChaCha20-Poly1305 密封，v2 / v3 用 AES-128-GCM，盐都是 16 字节（`transport/snell/snell.go` 的 `StreamConn`、`transport/snell/cipher.go`）；监听端对每个版本读的都是 `01 ‖ 命令 ‖ client id 长度 ‖ …`，TCP 的应答是它写出的第一段负载前面的一个 `00`（`listener/snell/server.go`）。
- `snell_v2_requests_share_a_connection_against_mihomo`：v2 总是复用连接——一连三个请求，每个请求两个方向都以零长度块结束（监听端收到 `ConnectV2` 才这样收尾并等下一个请求：`listener/snell/server.go` 的 `tcpRequestConn.Close` 与 `HandleConn` 的循环）。复用是否成立要问 mihomo：池里取出的连接失效时 rurge 会不声不响地换一条新连接重发，只看客户端这一侧，"复用了"与"每次都重拨"分不出来。`direct` 模式下 mihomo 为它转发出去的每个 TCP 请求记一行 `[TCP] <客户端地址> --> <目标> using DIRECT`（`tunnel/tunnel.go` 的 `logMetadata`，客户端地址是那个请求所在连接的对端），用例断言发往回显的恰好三行、三行的客户端地址相同。回显在拉起 mihomo 之前就绑好：就绪探测自己也在日志里留下一行这样的记录，它的目标是探测时临时开的一个端口，探测一结束就放掉了——回显要是事后才绑，可能拿到同一个号，那一行就会被算成第四个请求。
- `snell_v3_udp_against_mihomo`：UDP 从 v3 起才有——另开一条连接发 `01 06 00`，监听端立刻回一个只有 `00` 的块，此后每块一个数据报，回来的数据报以 `04` + IPv4（或 `06` + IPv6）+ 端口开头（`listener/snell/server.go` 的 `handleUDP`、`transport/snell/snell.go` 的 `ParseUDPRequest` / `WritePacketResponse`）；经同一个载体往返一个回环 UDP 回显两次，回包的来源必须就是那个回显。
- `a_snell_listener_of_another_psk_or_version_against_mihomo`：三行连不通的策略——PSK 不对的 v1 行连 v1 监听端、PSK 不对的 v4 行连 v4 监听端、PSK 对但写了 `version=1` 的行连 v4 监听端。mihomo 打不开第一条记录就立刻关掉连接，自己一个字节也不写（`listener/snell/server.go`：`handleRequest` 出错即返回，`HandleConn` 里延迟执行的 `Close`）；这时客户端发来的东西它只读了开头，所以这个关闭在有的系统上是"结束"、在有的系统上是"重置"。rurge 对两种各有一句话——`snell: the server closed the connection without answering (wrong psk or version?)` 与 `snell: the server reset the connection without answering (wrong psk or version?)`——用例只收这两句，逐字比对：会话必须在 10 秒内失败，文本必须是两句之一（里面自然没有任何一个 PSK）。带同样提示的还有第三句 `snell: the server's data failed to decrypt (wrong psk or version?)`，它不收：那意味着 mihomo 回了字节，与上面读到的行为相反。别的任何结果都是这一行的失败：三行都试，失败的各带着实际发生了什么记下，用例在最后带着 mihomo 的日志失败一次（判据是 `common::none_is_answered`，对 sing-box 与官方 snell-server 的同类用例共用它，见上面「覆盖范围」里 Snell 的一条与「snell-server」一节）。真实的 mihomo 在各平台上给出的是两句里的哪一句，通过时不显示。这套判据由 `only_the_two_sentences_of_an_unanswered_request_are_taken` 在没有参照服务端的地方核对：同一个 `none_is_answered` 对着回环上的四种服务端——PSK 不同的假服务端（`rurge_proto::testing::FakeSnell`）不回一个字节就关闭时是第一句，重置时是第二句，这两种通过；回一段哪把密钥都解不开的字节的服务端得到的是第三句，留着连接既不读也不关的服务端让请求挂住（这一种把等待缩到 300 毫秒），这两种记为失败，报告里各写明实际发生了什么。
- `ss_chacha20_against_mihomo`：`shadowsocks` 监听端 `cipher: chacha20`，单块与跨多块的 TCP 往返，再经 `udp-relay=true` 往返一个回环 UDP 回显两次（监听端的 UDP 端口与 TCP 端口同号）。mihomo 的 `shadowsocks` 监听端有两套实现：`cipher` 是 AEAD 或 2022 方法时用 sing-shadowsocks 的服务，其余的交给它自己较老的那一套（`listener/sing_shadowsocks/server.go` 的 `default` 分支 → `listener/shadowsocks`），后者的方法表在 `transport/shadowsocks/core/cipher.go`（名字先转成大写再比对）：`RC4-MD5`、`AES-128/192/256-CTR`、`AES-128/192/256-CFB`、`CHACHA20`、`CHACHA20-IETF`、`XCHACHA20`；`CHACHA20` 的 nonce 是 8 字节（`transport/shadowsocks/shadowstream/chacha20.go` 取 `chacha.NonceSize`，metacubex/chacha v0.1.5 的 `chacha.go` 说明它是 64 位 nonce 的那种）。
- `ss_behind_simple_obfs_against_mihomo`：`shadowsocks` 监听端加 `simple-obfs`——`aes-128-gcm` 配 `http`、`aes-128-gcm` 配 `tls`（sing-shadowsocks 那一套实现），以及 `aes-256-cfb` 配 `tls`（较老的那一套；两套都在读协议之前先套上同一层 obfs）——各做单块与跨多块的 TCP 往返。
- `snell_behind_simple_obfs_against_mihomo`：`snell` 监听端加 `obfs-opts`——v1 配 `tls`（rurge 只在 v1 ～ v3 上接受 `obfs=tls`）、v4 配 `http`——各做单块与跨多块的 TCP 往返。

simple-obfs 在 mihomo 服务端一侧的样子（`transport/simple-obfs/http_server.go`、`tls_server.go`）：`http` 模式用 Go 的 `http.ReadRequest` 读第一个包，要求方法是 `GET`、`Connection` 头逐字是 `Upgrade`，按 `Content-Length` 取出正文作为第一段负载，应答是 `HTTP/1.1 101 Switching Protocols` 一段头之后直接接数据；`tls` 模式按固定偏移读 ClientHello——跳过 140 字节后是 2 字节的 session ticket 长度与 ticket（第一段负载），再跳过 7 字节是 2 字节的服务器名长度与服务器名，再跳过固定的 66 字节，此后每条记录跳过 3 字节读 2 字节长度——应答是 ServerHello（96 字节）、ChangeCipherSpec（6 字节）与一条携带第一段数据的 `16 03 03` 记录，之后写出的数据切成至多 16 KiB 一条的 `17 03 03` 记录。rurge 的两个模板与这些偏移逐一对得上（`crates/rurge-proto/src/transport/obfs/`）。

做往返的步骤——各条策略的 TCP 往返、v3 与 `chacha20` 的 UDP 往返、v2 的三个请求——都经 `common::attempt` 执行：每一步在自己的任务里跑，失败了不打断用例，而是连同策略名与那一步自己的 panic（真实的错误文本）记进这个用例的 `common::Failures`，下一条策略照样试；用例末尾的 `finish` 只在有失败时让用例失败一次——先是"几条里失败了几条"，再是每条失败的策略一行，最后是 mihomo 的全部日志，只一遍（CI 给失败用例的输出做摘要时只取开头的 60 行、3000 个字符：该被截掉的是日志）。CI 上事后问不了 mihomo，每跑一次又要推送一次，所以一次运行要把能说的都说出来——阶段 2 / M8b 终审之前，`attempt` 在第一条失败的策略上就让用例失败，表里排在后面的策略那一次就看不到了。PSK 或版本不对的那个用例走 `common::none_is_answered`，同样记进 `Failures`、在最后带着日志失败一次；v2 复用对日志行的比对是单独的断言，自己把日志带进失败信息。

`common::Failures` 有一个 `Drop` 守卫：记过失败、却没有 `finish` 就被丢掉的 `Failures` 当场让用例失败，报出每一条失败（以 `failures were recorded and never finished, so the test would have passed:` 开头），所以用例不能靠漏写 `finish` 蒙混过关。用例本身正在 panic 时守卫不出声（在展开途中再 panic 会让进程中止）；`finish` 在 panic 之前先清空自己的失败表，免得守卫在展开时又说一遍。守卫取代了原先打算加的 `#[must_use]`：`#[must_use]` 只拦得住"建了就丢"，拦不住真正的写法——绑了变量、记了失败、不调 `finish`，编译器没有 lint 管这个——，而 `attempt` 的十五处用法全是那样写的。守卫当场找出过一个一直在"通过"的用例：`tests/mihomo.rs` 的 `only_the_two_sentences_of_an_unanswered_request_are_taken` 记了两条失败，却只调 `report`、没有 `finish`；现在它用 `finish`，并核对 `finish` 对着两条失败真的 panic。守卫自己有三个不需要参照服务端的用例，在 `tests/shadowsocks.rs`：`failures_that_are_never_finished_fail_the_test`、`nothing_that_failed_need_not_be_finished`，以及 `failures_dropped_by_a_test_that_fails_otherwise_say_nothing`——最后一个把本测试二进制当子进程再跑一遍自己（环境变量 `RURGE_INTEROP_UNWINDING_CHILD`），因为守卫若在展开途中 panic，整个测试二进制会跟着中止，在同一个进程里测不了；所以 `tests/interop/Cargo.toml` 里的 `tokio` 要开 `process` 特性。

`the_policies_of_every_case_load_and_build` 不需要 mihomo：`tests/mihomo.rs` 的每个用例都是一张"策略行 + 它要连的监听端"的表，这个用例把所有表里的策略行（端口换成固定的数）在 rurge 这一侧加载一遍——除了"用的是流式方法"（`W0031`）不许有别的诊断，拼错的参数名会以 `W0001` 现形——并各构建出出站。本机与 CI 都跑。

### 不写的用例

- **`salsa20` 与单独的 `rc4`**：mihomo 的流式方法表里没有这两种。`rc4` 由 `ssserver` 覆盖；`salsa20` 没有参照（见上面「shadowsocks-rust」一节）。
- **流式方法的口令错误**：与 `ssserver` 同理。mihomo 较老的那套实现从解出的乱码里读一个 SOCKS 地址（`listener/shadowsocks/tcp.go`），读不出就关连接，读得出就去连它。
- **目标一个字节都不回就结束**：这时 mihomo 的 `snell` 监听端回的是错误应答 `02`、码 `0x65`、文本 `Remote EOF`（`listener/snell/server.go` 的 `tcpRequestConn.Close`；不是 `ConnectV2` 时随后关连接），rurge 把它报成 `snell: the server refused: Remote EOF`。所以经 mihomo 没有"空的成功应答"这回事，别写期待它的用例。
- **v4 / v5 对 mihomo**：除了上面 obfs 那一个 v4 的行之外不写，它们由 sing-box 与官方 snell-server 覆盖。那个 v4 + `obfs=http` 的行若失败，原因可能在 obfs，也可能在 mihomo 的 v4。

### 配置、命令行与就绪

渲染出的配置（`rurge_interop::mihomo::render`）是 YAML，顶层只有四个键：`mode: direct`（缺省是 `rule`，没有规则时去向同样是 DIRECT；写明是为了让日志里的 `using DIRECT` 有定准）、`ipv6: false`、`log-level: info` 与 `listeners`。没有 `rules`、`dns`、`proxies`、`proxy-providers`、`rule-providers`、`external-controller`、`tun`。`listeners` 的每一项（`listener/parse.go`、`listener/inbound/base.go`）：

- `name`：必填，每个监听端一个（mihomo 按名字存放监听端，重名是配置错误）；`type`；`listen: 127.0.0.1`（不写时是 `0.0.0.0`）；`port`。
- `type: snell`（`listener/inbound/snell.go`）：`psk`、`version`（1 ～ 5；0 表示 4，别的值是错误）、`udp: true`（写在 `listeners` 里的条目缺省就是 true——`listener/parse.go` 的 `&IN.SnellOption{UDP: true}`——照样写明），需要时加 `obfs-opts`，它的 `mode`（`http` / `tls`）与 `host` **两个键都要写**：这个结构的字段标签写的是 `obfs:"…"` 而不是解码器认的 `inbound:"…"`，解码器于是按字段名不分大小写地找键，并且把两个字段都当成必填（`common/structure/structure.go` 的 `decodeStructFromMap`：缺了就是 `'obfs-opts' has unset fields: Host`）。`host` 的值服务端并不看；夹具写 `obfs.rurge.test`（`rurge_interop::mihomo::OBFS_HOST`），策略行的 `obfs-host` 写同一个名字。
- `type: shadowsocks`（`listener/inbound/shadowsocks.go`）：`password`、`cipher`、`udp: true`（缺省同样是 true），需要时加 `simple-obfs: { enable: true, mode: http | tls }`。
- 末尾另有一个 `name: ready`、`type: socks`、`udp: false` 的监听端，只给夹具判断就绪用（见下）。

测试自己编的字符串（PSK、口令、方法名、`host`）一律写成双引号标量（JSON 字符串的写法就是 YAML 的双引号写法），数字与布尔值不加引号（`port` 在 mihomo 里是字符串，解码器按弱类型把整数转过去）。夹具的单元用例 `the_configuration_stays_on_the_loopback`、`listeners_are_rendered_as_mihomo_reads_them`、`no_listener_lacks_a_key_mihomo_requires` 与 `made_up_strings_are_quoted` 断言：顶层只有这四个键、每个监听端都只听 `127.0.0.1`、两种监听端在不带 obfs、带 `http`、带 `tls` 三种情形下逐行如上、`obfs-opts` 总有 `host`、名字不重复。

一次拉起用到的端口——各监听端的，加上 `ready` 的——由 `rurge_interop::free_ports` 一次取齐：取到的每个端口先占着，等最后一个也取到才一起放开，所以没有两个监听端会拿到同一个号（一个一个地取时，前一个放开后系统可以把同一个号再给出来；那样 mihomo 只开其中一个、对另一个只记一行 `Listener … listen err`，而夹具去探的又是同一个端口）。`shadowsocks` 监听端的端口对 TCP 与同号的 UDP 都空闲（单元用例 `no_port_is_released_before_all_are_chosen`、`the_ports_of_one_call_differ_and_are_free_for_what_was_asked` 与夹具的 `a_shadowsocks_listener_asks_for_the_udp_port_of_its_number_too`）。

这不是 mihomo 独有的做法：凡一次拉起多个端口的夹具都这样一次取齐——sing-box（含它的 WireGuard、Hysteria 2、TUIC 形式，那几种里 UDP 入站的端口与就绪探测用的 `mixed` 入站的端口一起取）、xray、dnsproxy、CoreDNS、mihomo、`ssserver`——；只取一个端口的夹具（snell-server、`sshd`、TrustTunnel endpoint、tuic-server、masque-server）用 `free_port` / `free_udp_port` / `free_tcp_and_udp_port`，不需要它。夹具改用 `free_ports` 之后的取端口行为（M8c 任务 11）没有对真实的参照服务端跑过：第一次 CI 运行时核对。

启动命令是 `mihomo -d <临时目录> -f <临时目录>/mihomo.yaml`（`main.go`：`-d` 是它的主目录，`-f` 是配置文件）。`config.Init`（`config/initial.go`）只在主目录不存在时建目录、只在配置文件不存在时写一个缺省的 `config.yaml`；`-f` 指向夹具写好的文件，所以它什么都不建。**它在主目录里只写一个文件**：`cache.db`（`component/profile/cachefile/cache.go`；`profile.store-selected` 缺省为真，`hub/executor/executor.go` 的 `updateProfile` 启动时就打开它），在测试的临时目录里，随目录一起删除。**启动时不联网**：geo 数据只在规则或 DNS 的回落过滤用到时才加载，`geo-auto-update` 缺省关（`config/config.go` 的 `DefaultRawConfig`），界面只在配置了 `external-ui` 时才下载（`component/updater/update_ui.go`），`dns` 与 `ntp` 缺省不启用。

`main.go` 的十六个命令行参数里有十三个以一个环境变量为缺省值（`-m`、`-v`、`-t` 三个开关没有）——`CLASH_CONFIG_STRING` 会顶替整份配置，`CLASH_OVERRIDE_EXTERNAL_CONTROLLER*` 会打开控制端口，`CLASH_OVERRIDE_EXTERNAL_UI_DIR` 会让它去下载界面，`CLASH_POST_UP` / `CLASH_POST_DOWN` 会运行脚本——夹具把这十三个变量都从子进程的环境里去掉。另设 `DISABLE_LOOPBACK_DETECTOR=true`（`component/loopback/detector.go`）：mihomo 拒绝为"来源地址恰好是它自己某条出站连接的本地地址"的客户端转发，这是它防止自己连自己的保护；这里所有东西都在 `127.0.0.1` 上，客户端与 mihomo 的出站连接有可能拿到同一个端口号，这个保护在这里只可能误判。夹具的单元用例 `the_command_line_is_written_as_mihomo_reads_it` 断言参数、工作目录与这十四个环境变量。

**就绪的判据不是日志行，也不只是端口可连。**每个监听端起来时 mihomo 记一行 `Snell[<名字>] inbound listening at: <地址>` 或 `ShadowSocks[<名字>] proxy listening at: <地址>`（`listener/inbound/*.go`），但那时它还不转发：`hub/executor/executor.go` 的 `ApplyConfig` 先开监听端，过后（打开 `cache.db`、做一次 GC 之后）才调用 `tunnel.OnRunning()`，而在那之前交给 tunnel 的连接会被直接关掉（`tunnel/tunnel.go`：`handleTCPConn` 开头的 `isHandle`；UDP 包则被丢弃），`OnRunning()` 之后它也不再记任何日志。照别的夹具那样一连上端口就开始测，第一条连接就可能撞在这段空当里。所以夹具分两步：先等每个监听端的 TCP 端口都能连上（某个监听端没起来时 mihomo 只记一行 `Listener … listen err` 并继续运行，这一步会带着日志超时）；再经那个 `socks` 监听端发一个 SOCKS5 的 CONNECT，目标是夹具自己临时开的一个回环监听——连接真的到了才算就绪，mihomo 把探测连接关掉了就再问，总共限时 15 秒（`rurge_interop::mihomo` 的 `relays`，单元用例 `the_probe_tells_a_tunnel_that_relays_from_a_listener_that_accepts` 用一个头几次只关连接的假 SOCKS5 服务端验证它）。`socks` 监听端没配用户时照全局的 `lan-allowed-ips`（缺省放行全部地址）放行（`listener/socks/tcp.go`、`adapter/inbound/ipfilter.go`）。

CI 在三个平台上都下载、校验并设置 `RURGE_TEST_MIHOMO`（`RURGE_INTEROP_REQUIRED=1` 对它生效）。本机不安装它：`RURGE_TEST_MIHOMO`（优先于 `PATH` 查找）没有指向可执行文件、`PATH` 上也找不到 `mihomo` / `mihomo.exe` 时，用例打印一行 `skipping …` 后直接返回；这个 crate 不会下载或安装它。

### 第一次 CI 运行要核对的

- 三个资产能下载、SHA-256 相符，`.gz` 确实是单个可执行文件、`.zip` 里确实有一个 `.exe`；`-v1` 的 Linux / Windows 构建与 arm64 的 macOS 构建在各自的 runner 上能启动。
- 上面这份配置它收：`name` / `type` / `listen` / `port`，`snell` 的 `psk` / `version` / `udp` / `obfs-opts`（`mode` 与 `host`），`shadowsocks` 的 `password` / `cipher` / `udp` / `simple-obfs`，`socks` 的 `udp: false`；`-d` / `-f` 两个参数；`DISABLE_LOOPBACK_DETECTOR` 不妨碍启动。
- 启动时不联网、主目录里只多出 `cache.db`。
- 就绪：端口可连之后经 `socks` 监听端的探测在 15 秒内成功。
- `direct` 模式下的日志行 `[TCP] <客户端地址> --> <目标> using DIRECT`（v2 复用的判据靠它）。
- PSK 或版本不对时 mihomo 立刻关连接，以及各平台上 rurge 给出的是"结束"还是"重置"那一句。
- `shadowsocks` 监听端收 `cipher: chacha20`（TCP 与 UDP），以及 simple-obfs 的两种模式在两种监听端上都通。

## sshd

`tests/sshd.rs` 用 OpenSSH 的 `sshd` 验证 `ssh` 出站（阶段 2 M4 设计第 10 节）：OpenSSH 的默认算法，以及只开 Surge 手册要求的 `curve25519-sha256` 与 `aes128-gcm@openssh.com` 两种。不以 root 运行的 `sshd` 只能让运行它的用户登录，所以用例用密钥登录（用户名取环境变量 `USER`），并且只在 Unix 上编译。

查找顺序：`RURGE_TEST_SSHD`，然后 `/usr/sbin/sshd`，然后 `PATH` 上的 `sshd`；都没有就打印 `skipping …` 后返回（`RURGE_INTEROP_REQUIRED=1` 时改为失败）。CI 在 Linux 与 macOS 上设置 `RURGE_TEST_SSHD=/usr/sbin/sshd`，Linux 上缺它时先装 `openssh-server`；Windows 上不编译这两个用例。

渲染出的 `sshd_config`（`rurge_interop::sshd::render`）只监听 `127.0.0.1`，只认用例现场生成的那一把公钥，关掉口令、PAM 与终端，只允许本地端口转发（夹具的单元用例 `the_configuration_stays_on_the_loopback_with_one_key` 断言这一点）；主机密钥同样现场生成，写进临时目录。

## 安全约束

- 夹具渲染出的 sing-box 配置只有 `log` / `inbounds` / `outbounds` 三个顶层键（WireGuard 的配置另有 `endpoints`）；每个入站只监听 `127.0.0.1`（`hysteria2` 与 `tuic` 入站的 UDP 端口也是，同一份配置里另有一个只用来判断就绪的 `mixed` 入站；`rurge_interop::render_hysteria2` 与 `rurge_interop::render_tuic` 的单元测试 `the_hysteria2_configuration_never_touches_the_machine` 与 `the_tuic_configuration_never_touches_the_machine` 断言这些）；唯一的出站是 `direct`。WireGuard 端点在用户态运行（`system: false`），它的 UDP 端口开在所有地址上（端点没有监听地址这一项；`rurge_interop::render_wireguard` 的单元测试 `the_wireguard_configuration_never_touches_the_machine` 断言其余各项）。xray 配置同样只有这三个顶层键，唯一的出站是 `freedom`。`ssserver` 的配置只有 `servers`，每个服务端只听 `127.0.0.1`（TCP 与同号的 UDP）。`snell-server` 的配置只有 `[snell-server]` 一节，只听 `127.0.0.1`。TrustTunnel endpoint 只听 `127.0.0.1`、只开一种协议（HTTP/2，或 QUIC），转发只有 `direct`；`allow_private_network_connections = true` 只是为了让它连回环上的 echo。tuic-server 的配置只有它认识的七个键，只听 `127.0.0.1`（它转发 UDP 的 socket 绑在所有地址上的一个空闲端口，0.8.5 没有配置项）。masque-server 只有一个 HTTP/3 监听、只听 `127.0.0.1`，不开监控端口、不开 CONNECT-IP（不建 TUN 网卡）；它的目标策略放开到任何地址，只是为了让它连回环上的 echo（它转发 CONNECT-UDP 的 socket 绑在所有地址上的一个空闲端口、再连到那个回环目标，v0.13.0 没有配置项）。dnsproxy 的全部监听只在 `127.0.0.1`，唯一的上游是回环的 9 号端口（没有用例的问题会到那里）；CoreDNS 的两个服务块只绑 `127.0.0.1`，没有 `forward`。mihomo 的配置顶层只有 `mode`、`ipv6`、`log-level` 与 `listeners`，每个监听端只听 `127.0.0.1`（`shadowsocks` 监听端同号的 UDP 端口也是），进来的流量一律直连出去，不开控制端口、不建 TUN、不配 DNS；环境里能顶替或追加配置的 `CLASH_*` 变量都从子进程去掉；它只在测试的临时目录里留一个 `cache.db`（它转发 UDP 用的 socket 在会话期间绑在所有地址上的一个空闲端口，v1.19.32 的直连出站总是这样）。任何地方都不出现 `set_system_proxy`、`tun`、`auto_route` 这些键（`rurge_interop::render` 与 `rurge_interop::xray::render` 的单元测试 `the_configuration_never_touches_the_machine` 各自断言这一点）；`shadowtls` 入站的 `handshake.server` 恒为 `127.0.0.1`（夹具的单元用例断言）。
- 每个用例的连接目标都是回环 IP 字面量（`127.0.0.1` 上的 echo / 测试服务器；WireGuard 用例在隧道里连的是 sing-box 自己的隧道地址 `10.9.0.1`，由 sing-box 映射到它的 `127.0.0.1`），sing-box、xray、`ssserver`、`snell-server`、TrustTunnel endpoint、`tuic-server`、masque-server 与 mihomo 因此既不解析域名也不会访问公网；dnsproxy 与 CoreDNS 被问的只有它们自己配置里的那个名字，从配置作答，不往外转发。`ss` 的流式方法没有校验，口令不对时服务端解出的"目标"是任意地址，所以对 `ssserver` 与 mihomo 都不写流式方法的口令错误用例。
- 这个 crate 本身不下载、不安装任何东西；本机是否装有 sing-box、xray、shadowsocks-rust、snell-server、TrustTunnel endpoint、tuic-server、masque-server、dnsproxy、CoreDNS 或 mihomo 由项目所有者决定，没装就跳过。
