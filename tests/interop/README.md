# rurge-interop

`rurge-interop` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它把 [sing-box](https://sing-box.sagernet.org/) 与 [xray](https://github.com/XTLS/Xray-core) 作为参照实现，以回环子进程的方式拉起来，驱动 rurge 的 `http` / `https` / `socks5` / `trojan` / `vmess` / `anytls` 出站，以及包在 Shadow TLS 里的 `trojan`，去连它们，验证 rurge 与真实的第三方实现互通。xray 只用来跑 `vmess`：VMess 协议由 xray 所在的这一脉实现定义，sing-box 的实现是重写，手写的编解码需要两个独立参照互相印证（M2 设计 M2-D5）。

## 固定版本

互操作测试固定 sing-box **1.14.1**（2026-09-15 发布的稳定版）。CI 下载并校验以下三个发布包：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (amd64) | `sing-box-1.14.1-linux-amd64.tar.gz` | `12cb2816b52febb356f6a885b740cc8758c3f30b8ae0ca8edba80f0d2d35343f` |
| Windows (amd64) | `sing-box-1.14.1-windows-amd64.zip` | `5197f16d492d93202dc623622149a6ed040f8eca263128f91d603f2b901baa89` |
| macOS (arm64) | `sing-box-1.14.1-darwin-arm64.tar.gz` | `b9024642ef7b4848252df5469b7f60ef3c18bb5e217a16a0934f0174f8ad11b4` |

## 本地运行

本地默认不安装 sing-box 与 xray：`cargo test -p rurge-interop` 会正常通过，sing-box 的十一个互操作用例与 xray 的一个互操作用例各打印一行 `skipping …` 后直接返回（两个夹具自身的单元测试——配置渲染、"配置绝不碰本机"的安全守卫、取空闲端口——照常运行并断言）。要在本机真正跑 sing-box 的用例，二选一：

- 自行安装 sing-box 1.14.1，让 `sing-box` / `sing-box.exe` 出现在 `PATH` 上；或
- 不安装到 `PATH`，改用 `RURGE_TEST_SING_BOX=<sing-box 可执行文件路径> cargo test -p rurge-interop`。

xray 的本机运行方式同理，见下面「xray」一节。

## 环境变量

- `RURGE_TEST_SING_BOX`：sing-box 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_TEST_XRAY`：xray 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_INTEROP_REQUIRED=1`：找不到二进制时让用例直接失败，而不是打印 `skipping …` 后跳过；对 sing-box 与 xray 两个夹具都生效。CI 会设置它，本机一般不需要。

## 覆盖范围

`tests/sing_box.rs` 驱动的四个用例（另一个是夹具自检）覆盖：

- `http`：CONNECT 隧道，含无认证 / 正确凭据 / 错误凭据三种；以及明文 HTTP 请求走绝对 URI 转发（不经 CONNECT）。
- `https`：私有 CA 校验、`sni=` 覆盖、`server-cert-fingerprint-sha256` 指纹钉定（含钉错的情形）、`client-cert=`（p12 客户端证书）双向 TLS，以及服务端要求客户端证书但客户端未提供的情形。
- `socks5`：无认证 / 正确凭据 / 错误凭据，以及 sing-box 的 `mixed` 入站同时按 SOCKS5 和 HTTP 两种协议接受连接。

`tests/sing_box_tls_family.rs` 驱动的五个用例覆盖：

- `trojan`：TLS 传输，以及可选的 V2Ray WebSocket 传输（`ws=true`, `ws-path=`）。trojan 协议对请求头没有任何应答，连接本身在密码错误时也会建立成功，所以"密码错误"用例断言的是负载不被回显，而不是连接失败。
- `vmess`：AEAD 握手（`vmess-aead=true`），两种 `encrypt-method`（默认的 `aes-128-gcm` 与 `chacha20-ietf-poly1305`），可选的 TLS 与 V2Ray WebSocket 传输及其组合，单块与跨多块（100 000 字节）的往返，以及 UUID 错误时的行为——sing-box 只应答它接受的请求，所以连接本身能建立，但读不到回显。
- `anytls`：同一个出站发起多条流默认复用同一个会话（`reuse` 的协议默认值），以及 `reuse=false` 时每条流各开一个新会话。

- Shadow TLS（`tests/sing_box_shadow_tls.rs`）：sing-box 的 `shadowtls` 入站（v2 与 v3，v3 开 `strict_mode`）把握手转发给夹具自己在回环上起的 TLS 服务端（伪装站点，证书由夹具的 CA 签发），解出来的流量经 `detour` 交给同一个 sing-box 里的 `trojan` 入站；覆盖小负载与跨多帧的往返，以及口令错误（v3 的会话文本、伪装站点确实收到了那个 HTTP 请求）。v2 的用例让伪装站点不发 session ticket（sing-box 对"首帧之前又转发了字节"只多容忍一次写），v3 的发两张（覆盖数据阶段开头的残留记录）。

**不覆盖 `socks5-tls`**：sing-box 的 `socks` 与 `mixed` 入站没有 `tls` 字段（参见 sing-box 文档 `configuration/inbound/{socks,mixed}`），无法用它搭建一个会说 TLS 的 SOCKS5 服务端。`socks5-tls` 的覆盖仍由 M1a 的回环假上游（`rurge_proto::testing::FakeSocks5`）承担。

## xray

xray 只用来验证 `vmess`：VMess 协议由 xray 所在的这一脉实现（v2ray / xray）定义，sing-box 的实现是重写，手写的编解码需要两个独立参照互相印证（M2 设计 M2-D5）。

互操作测试固定 xray **v26.3.27**。CI 下载并校验以下三个发布包：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (amd64) | `Xray-linux-64.zip` | `23cd9af937744d97776ee35ecad4972cf4b2109d1e0fe6be9930467608f7c8ae` |
| Windows (amd64) | `Xray-windows-64.zip` | `d004c39288ce9ada487c6f398c7c545f7d749e44bdfdd59dbc9f865afba4e1ad` |
| macOS (arm64) | `Xray-macos-arm64-v8a.zip` | `2e93a67e8aa1936ecefb307e120830fcbd4c643ab9b1c46a2d0838d5f8409eaf` |

`tests/xray.rs` 驱动的一个用例覆盖 `vmess`：两种 `encrypt-method`（默认的 `aes-128-gcm` 与 `chacha20-ietf-poly1305`）、可选的 V2Ray WebSocket 传输，以及单块与跨多块（100 000 字节）的往返。

本机不安装 xray：`RURGE_TEST_XRAY`（优先于 `PATH` 查找）没有指向可执行文件、`PATH` 上也找不到 `xray` / `xray.exe` 时，用例打印一行 `skipping …` 后直接返回；这个 crate 不会下载或安装 xray。互操作由首次推送后的 CI 证明（CI 安装 xray v26.3.27 并设置 `RURGE_TEST_XRAY` 与 `RURGE_INTEROP_REQUIRED=1`）。

渲染出的配置（`rurge_interop::xray::render`）只有 `log` / `inbounds` / `outbounds` 三个顶层键：每个入站是一个只监听 `127.0.0.1` 的 `vmess`（可选 `ws` 传输），唯一的出站是 `freedom`。

## 安全约束

- 夹具渲染出的 sing-box 配置只有 `log` / `inbounds` / `outbounds` 三个顶层键；每个入站只监听 `127.0.0.1`；唯一的出站是 `direct`。xray 配置同样只有这三个顶层键，唯一的出站是 `freedom`。任何地方都不出现 `set_system_proxy`、`tun`、`auto_route` 这些键（`rurge_interop::render` 与 `rurge_interop::xray::render` 的单元测试 `the_configuration_never_touches_the_machine` 各自断言这一点）；`shadowtls` 入站的 `handshake.server` 恒为 `127.0.0.1`（夹具的单元用例断言）。
- 每个用例的连接目标都是回环 IP 字面量（`127.0.0.1` 上的 echo / 测试服务器），sing-box 与 xray 因此既不解析域名也不会访问公网。
- 这个 crate 本身不下载、不安装任何东西；本机是否装有 sing-box 或 xray 由项目所有者决定，没装就跳过。
