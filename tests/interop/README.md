# rurge-interop

`rurge-interop` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它把 [sing-box](https://sing-box.sagernet.org/)、[xray](https://github.com/XTLS/Xray-core)、[shadowsocks-rust](https://github.com/shadowsocks/shadowsocks-rust) 的 `ssserver` 与 Surge 官方的 `snell-server` 作为参照实现，以回环子进程的方式拉起来，驱动 rurge 的 `http` / `https` / `socks5` / `trojan` / `vmess` / `anytls` / `wireguard` / `ss` / `snell` 出站，以及包在 Shadow TLS 里的 `trojan`，去连它们，验证 rurge 与真实的第三方实现互通。xray 只用来跑 `vmess`：VMess 协议由 xray 所在的这一脉实现定义，sing-box 的实现是重写，手写的编解码需要两个独立参照互相印证（M2 设计 M2-D5）。

## 固定版本

互操作测试固定 sing-box **1.14.2**（2026-09-24 发布的稳定版；阶段 2 / M6b 从 1.14.1 升上来：`snell` 入站 1.14.0 起才有，1.14.2 是 1.14.x 的最新版）。CI 下载并校验以下三个发布包（SHA-256 取自 GitHub 发布 API 每个资产的 `digest`）：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (amd64) | `sing-box-1.14.2-linux-amd64.tar.gz` | `a684484d7477d1437282ee411f4d131d0340aaad60a7868841ebd5d87dd8a0c6` |
| Windows (amd64) | `sing-box-1.14.2-windows-amd64.zip` | `c2d8bfff918755808781dfdeeb8581b6c91eb3a243d9a7b55483cfc0c0684d32` |
| macOS (arm64) | `sing-box-1.14.2-darwin-arm64.tar.gz` | `925c5382eca8492b0150f868a6db20b18290a38700e621724b3703fd453e032d` |

## 本地运行

本地默认不安装 sing-box、xray、shadowsocks-rust 与 snell-server：`cargo test -p rurge-interop` 会正常通过，sing-box 的十九个互操作用例、xray 的两个、shadowsocks-rust 的两个与 snell-server 的四个互操作用例各打印一行 `skipping …` 后直接返回（各夹具自身的单元测试——配置渲染、"配置绝不碰本机"的安全守卫、取空闲端口——照常运行并断言）。要在本机真正跑 sing-box 的用例，二选一：

- 自行安装 sing-box 1.14.2，让 `sing-box` / `sing-box.exe` 出现在 `PATH` 上；或
- 不安装到 `PATH`，改用 `RURGE_TEST_SING_BOX=<sing-box 可执行文件路径> cargo test -p rurge-interop`。

xray、shadowsocks-rust 与 snell-server 的本机运行方式同理，见下面「xray」「shadowsocks-rust」「snell-server」三节。

## 环境变量

- `RURGE_TEST_SING_BOX`：sing-box 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_TEST_XRAY`：xray 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_TEST_SSSERVER`：shadowsocks-rust 的 `ssserver` 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_TEST_SNELL_SERVER`：Surge 官方 `snell-server` 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_INTEROP_REQUIRED=1`：找不到二进制时让用例直接失败，而不是打印 `skipping …` 后跳过；对 sing-box、xray、shadowsocks-rust 与 sshd 各夹具都生效，对 snell-server 只在 Linux 上生效（它只有 Linux 版）。CI 会设置它，本机一般不需要。

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

## sshd

`tests/sshd.rs` 用 OpenSSH 的 `sshd` 验证 `ssh` 出站（阶段 2 M4 设计第 10 节）：OpenSSH 的默认算法，以及只开 Surge 手册要求的 `curve25519-sha256` 与 `aes128-gcm@openssh.com` 两种。不以 root 运行的 `sshd` 只能让运行它的用户登录，所以用例用密钥登录（用户名取环境变量 `USER`），并且只在 Unix 上编译。

查找顺序：`RURGE_TEST_SSHD`，然后 `/usr/sbin/sshd`，然后 `PATH` 上的 `sshd`；都没有就打印 `skipping …` 后返回（`RURGE_INTEROP_REQUIRED=1` 时改为失败）。CI 在 Linux 与 macOS 上设置 `RURGE_TEST_SSHD=/usr/sbin/sshd`，Linux 上缺它时先装 `openssh-server`；Windows 上不编译这两个用例。

渲染出的 `sshd_config`（`rurge_interop::sshd::render`）只监听 `127.0.0.1`，只认用例现场生成的那一把公钥，关掉口令、PAM 与终端，只允许本地端口转发（夹具的单元用例 `the_configuration_stays_on_the_loopback_with_one_key` 断言这一点）；主机密钥同样现场生成，写进临时目录。

## 安全约束

- 夹具渲染出的 sing-box 配置只有 `log` / `inbounds` / `outbounds` 三个顶层键（WireGuard 的配置另有 `endpoints`）；每个入站只监听 `127.0.0.1`；唯一的出站是 `direct`。WireGuard 端点在用户态运行（`system: false`），它的 UDP 端口开在所有地址上（端点没有监听地址这一项；`rurge_interop::render_wireguard` 的单元测试 `the_wireguard_configuration_never_touches_the_machine` 断言其余各项）。xray 配置同样只有这三个顶层键，唯一的出站是 `freedom`。`ssserver` 的配置只有 `servers`，每个服务端只听 `127.0.0.1`（TCP 与同号的 UDP）。`snell-server` 的配置只有 `[snell-server]` 一节，只听 `127.0.0.1`。任何地方都不出现 `set_system_proxy`、`tun`、`auto_route` 这些键（`rurge_interop::render` 与 `rurge_interop::xray::render` 的单元测试 `the_configuration_never_touches_the_machine` 各自断言这一点）；`shadowtls` 入站的 `handshake.server` 恒为 `127.0.0.1`（夹具的单元用例断言）。
- 每个用例的连接目标都是回环 IP 字面量（`127.0.0.1` 上的 echo / 测试服务器；WireGuard 用例在隧道里连的是 sing-box 自己的隧道地址 `10.9.0.1`，由 sing-box 映射到它的 `127.0.0.1`），sing-box、xray、`ssserver` 与 `snell-server` 因此既不解析域名也不会访问公网。
- 这个 crate 本身不下载、不安装任何东西；本机是否装有 sing-box、xray、shadowsocks-rust 或 snell-server 由项目所有者决定，没装就跳过。
