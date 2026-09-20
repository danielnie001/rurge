# rurge-interop

`rurge-interop` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它把 [sing-box](https://sing-box.sagernet.org/) 作为参照实现，以回环子进程的方式拉起来，驱动 rurge 的 `http` / `https` / `socks5` / `trojan` 出站去连它，验证 rurge 与一个真实的第三方实现互通。

## 固定版本

互操作测试固定 sing-box **1.14.1**（2026-09-15 发布的稳定版）。CI 下载并校验以下三个发布包：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (amd64) | `sing-box-1.14.1-linux-amd64.tar.gz` | `12cb2816b52febb356f6a885b740cc8758c3f30b8ae0ca8edba80f0d2d35343f` |
| Windows (amd64) | `sing-box-1.14.1-windows-amd64.zip` | `5197f16d492d93202dc623622149a6ed040f8eca263128f91d603f2b901baa89` |
| macOS (arm64) | `sing-box-1.14.1-darwin-arm64.tar.gz` | `b9024642ef7b4848252df5469b7f60ef3c18bb5e217a16a0934f0174f8ad11b4` |

## 本地运行

本地默认不安装 sing-box：`cargo test -p rurge-interop` 会正常通过，六个互操作用例各打印一行 `skipping …` 后直接返回（夹具自身的单元测试——配置渲染、"配置绝不碰本机"的安全守卫、取空闲端口——照常运行并断言）。要在本机真正跑互操作用例，二选一：

- 自行安装 sing-box 1.14.1，让 `sing-box` / `sing-box.exe` 出现在 `PATH` 上；或
- 不安装到 `PATH`，改用 `RURGE_TEST_SING_BOX=<sing-box 可执行文件路径> cargo test -p rurge-interop`。

## 环境变量

- `RURGE_TEST_SING_BOX`：sing-box 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_INTEROP_REQUIRED=1`：找不到二进制时让用例直接失败，而不是打印 `skipping …` 后跳过。CI 会设置它，本机一般不需要。

## 覆盖范围

`tests/sing_box.rs` 驱动的六个用例覆盖：

- `http`：CONNECT 隧道，含无认证 / 正确凭据 / 错误凭据三种；以及明文 HTTP 请求走绝对 URI 转发（不经 CONNECT）。
- `https`：私有 CA 校验、`sni=` 覆盖、`server-cert-fingerprint-sha256` 指纹钉定（含钉错的情形）、`client-cert=`（p12 客户端证书）双向 TLS，以及服务端要求客户端证书但客户端未提供的情形。
- `socks5`：无认证 / 正确凭据 / 错误凭据，以及 sing-box 的 `mixed` 入站同时按 SOCKS5 和 HTTP 两种协议接受连接。
- `trojan`：TLS 传输，以及可选的 V2Ray WebSocket 传输（`ws=true`, `ws-path=`）。trojan 协议对请求头没有任何应答，连接本身在密码错误时也会建立成功，所以"密码错误"用例断言的是负载不被回显，而不是连接失败。

**不覆盖 `socks5-tls`**：sing-box 的 `socks` 与 `mixed` 入站没有 `tls` 字段（参见 sing-box 文档 `configuration/inbound/{socks,mixed}`），无法用它搭建一个会说 TLS 的 SOCKS5 服务端。`socks5-tls` 的覆盖仍由 M1a 的回环假上游（`rurge_proto::testing::FakeSocks5`）承担。

## 安全约束

- 夹具渲染出的 sing-box 配置只有 `log` / `inbounds` / `outbounds` 三个顶层键；每个入站只监听 `127.0.0.1`；唯一的出站是 `direct`。任何地方都不出现 `set_system_proxy`、`tun`、`auto_route` 这些键（`rurge_interop::render` 的单元测试 `the_configuration_never_touches_the_machine` 断言这一点)。
- 每个用例的连接目标都是回环 IP 字面量（`127.0.0.1` 上的 echo / 测试服务器），sing-box 因此既不解析域名也不会访问公网。
- 这个 crate 本身不下载、不安装任何东西；本机是否装有 sing-box 由项目所有者决定，没装就跳过。
