<div align="center">

---

中文

### 简介

rurge（**Ru**st + Su**rge**）是一个用 Rust 编写的跨平台网络代理工具，目标是逐步复刻 [Surge](https://nssurge.com/)（macOS / iOS）的全部功能，并做到：

- **原生兼容 Surge 配置格式**：直接使用 `.conf` 配置、`.sgmodule` 模块、`RULE-SET` / `DOMAIN-SET` 规则集、`policy-path` 策略订阅和托管配置；
- **兼容 Surge 脚本 API**：现有 `http-request` / `http-response` / `cron` / `event` / `dns` / `rule` / `generic` 脚本无需改动即可运行；
- **兼容 Surge HTTP API**：现有 Dashboard 与自动化工具可以直接对接；
- **跨平台核心优先**：Windows / Linux / macOS 共用同一个命令行守护进程，桌面 GUI 放在后期阶段。

### 当前状态

> **阶段 1 进行中：M1、M2a 完成**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、`rurge rule match`）；M2b（DNS）、M3、M4 未开始。`rurge check` 可以校验任意 Surge 配置并给出带行号的诊断；`rurge rule match` 可以离线测试一次会话会命中哪条规则；代理功能尚未实现。

完整的需求、模块划分、平台差异和分阶段路线图见 [docs/requirements.md](docs/requirements.md)；
Surge 配置项 / 规则 / 参数 / API 的逐项兼容清单见 [docs/surge-compatibility-matrix.md](docs/surge-compatibility-matrix.md)。

### 计划特性

| 模块           | 内容                                                                                                                                                          | 阶段  |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----- |
| 配置与 Profile | Surge`.conf` 解析、`[General]` 全部选项、托管配置自动更新、`.sgmodule` 模块、Requirement 表达式、Keystore                                               | 1 / 5 |
| 入站           | HTTP / HTTPS 代理、SOCKS5、局域网共享与认证、系统代理设置                                                                                                     | 1     |
| 规则系统       | 域名 / IP / GEOIP / IP-ASN / HTTP / 进程 / 源与端口 / 协议与网络 / 逻辑 / 脚本 / 规则集 / FINAL（规则引擎 / 规则集 / GeoIP 已实现，M2a）                                                               | 1     |
| DNS            | 普通 DNS、DoH / DoT / DoQ / DoH3、本地映射、劫持、fake-ip、always-real-ip                                                                                     | 1 / 3 |
| 出站协议       | DIRECT / REJECT 系列 / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / 外部程序 | 2     |
| 策略组         | select / url-test / fallback / load-balance / smart / subnet、策略引入与订阅、延迟测试                                                                        | 2     |
| 增强模式       | 虚拟网卡（Wintun / tun / utun）、UDP、路由包含与排除、进程识别、子网设置                                                                                      | 3     |
| HTTP 处理      | MITM（HTTPS 解密）、URL / Header / Body 重写、Map Local、请求查看与抓包                                                                                       | 4     |
| 脚本与模块     | JavaScript 引擎、完整 Surge 脚本 API、模块系统、信息面板                                                                                                      | 5     |
| API 与工具     | Surge 兼容 HTTP API、Web Dashboard、Logbook、延迟 / 基准测试、CLI                                                                                             | 6     |
| 高级网络       | 网关模式、DHCP 服务器、端口转发、内置 Snell / MTProto 服务器                                                                                                  | 7     |
| 桌面 GUI       | 跨平台桌面客户端、URL Scheme                                                                                                                                  | 8     |

### 路线图

1. **阶段 0** 需求与文档（当前）
2. **阶段 1** 核心骨架：配置解析、HTTP / SOCKS5 入站、DIRECT / REJECT、规则引擎、DNS、CLI、HTTP API 骨架
3. **阶段 2** 出站协议全集、策略组、策略订阅
4. **阶段 3** 增强模式（TUN）、fake-ip、进程识别
5. **阶段 4** HTTP 引擎：MITM、重写、Map Local、抓包
6. **阶段 5** 脚本引擎与模块系统
7. **阶段 6** HTTP API 全量兼容、Web Dashboard、测试与诊断工具
8. **阶段 7** 网关 / DHCP / 端口转发 / 内置服务器
9. **阶段 8** 桌面 GUI

近期非目标：iOS / tvOS 版本、Surge Ponte（依赖 iCloud）、Apple 平台专属界面；Tailscale 集成列为远期评估项。

### 快速开始（计划中的形态）

> `rurge check` 与 `rurge rule match`（离线规则测试）已可用；`rurge run` 将在 M3 提供。

```bash
# 构建
cargo build --release

# 校验配置
rurge check -c config.conf

# 离线测试某个请求会命中哪条规则、落到哪个策略（无需启动守护进程）
rurge rule match -c surge.conf www.example.com --explain

# 运行
rurge run -c config.conf
```

配置文件直接采用 Surge 格式：

```ini
[General]
loglevel = notify
dns-server = 223.5.5.5, 119.29.29.29
encrypted-dns-server = https://dns.alidns.com/dns-query
http-listen = 127.0.0.1:6152
socks5-listen = 127.0.0.1:6153
skip-proxy = 127.0.0.1, 192.168.0.0/16, 10.0.0.0/8, localhost, *.local
exclude-simple-hostnames = true
http-api = password@127.0.0.1:6171
http-api-web-dashboard = true

[Proxy]
Proxy-A = ss, example.com, 8388, encrypt-method=chacha20-ietf-poly1305, password=secret
Proxy-B = trojan, example.org, 443, password=secret, sni=example.org

[Proxy Group]
Auto = url-test, Proxy-A, Proxy-B, interval=600, tolerance=100
Select = select, Auto, Proxy-A, Proxy-B, DIRECT

[Rule]
DOMAIN-SUFFIX,example.com,Select
RULE-SET,SYSTEM,DIRECT
RULE-SET,LAN,DIRECT
GEOIP,CN,DIRECT
FINAL,Select,dns-failed
```

### 文档

- [需求文档（PRD）](docs/requirements.md)
- [Surge 兼容性清单](docs/surge-compatibility-matrix.md)
- 各阶段的设计文档与实施计划将放在 `docs/superpowers/specs/` 与 `docs/superpowers/plans/`

### 开发

- 工具链：Rust stable（通过 `rustup` 安装），`cargo fmt`、`cargo clippy`、`cargo test`
- 工作流：每个阶段先写设计文档，再写实施计划，再按计划实现并附测试
- 贡献前请先阅读需求文档，并在 Issue 中讨论较大的改动

### 免责声明

- rurge 是独立的开源项目，与 Surge 的开发者 NSSurge 没有任何关系；「Surge」是其所有者的商标，本项目仅用于描述兼容性。
- 本项目不提供任何代理服务器或网络服务，请遵守所在地区的法律法规使用。
- MITM（HTTPS 解密）等功能只应用于自己拥有或获得授权的设备与流量。
- GEOIP / IP-ASN 规则默认使用 MaxMind 的 GeoLite2 数据库：This product includes GeoLite2 data created by MaxMind, available from <https://www.maxmind.com>。

### 许可证

[MIT](LICENSE)

---

## English

### Introduction

rurge (**Ru**st + Su**rge**) is a cross-platform network proxy written in Rust. Its goal is to re-implement, phase by phase, the full feature set of [Surge](https://nssurge.com/) (macOS / iOS) while being:

- **Natively compatible with Surge's configuration format**: `.conf` profiles, `.sgmodule` modules, `RULE-SET` / `DOMAIN-SET` rule sets, `policy-path` subscriptions and managed profiles work as-is;
- **Compatible with Surge's scripting API**: existing `http-request` / `http-response` / `cron` / `event` / `dns` / `rule` / `generic` scripts run unchanged;
- **Compatible with Surge's HTTP API**: existing dashboards and automation tools can connect directly;
- **Core first, cross-platform**: one command-line daemon shared by Windows / Linux / macOS, with a desktop GUI planned for a later phase.

### Status

> **Phase 1 in progress: M1 and M2a are done** (profile parsing, rule engine, rule sets, GeoIP, external resource management, `rurge rule match`); M2b (DNS), M3 and M4 have not started. `rurge check` validates any Surge profile with line-numbered diagnostics; `rurge rule match` tests offline which rule a session would hit; proxying is not implemented yet.

See [docs/requirements.md](docs/requirements.md) (Chinese) for the full requirements, module breakdown, platform matrix and phased roadmap, and [docs/surge-compatibility-matrix.md](docs/surge-compatibility-matrix.md) for the item-by-item Surge compatibility checklist.

### Planned features

| Module              | Scope                                                                                                                                                                   | Phase |
| ------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----- |
| Profile             | Surge`.conf` parser, every `[General]` option, managed profiles, `.sgmodule` modules, requirement expressions, keystore                                           | 1 / 5 |
| Inbound             | HTTP / HTTPS proxy, SOCKS5, LAN sharing with authentication, system proxy                                                                                               | 1     |
| Rules               | Domain / IP / GEOIP / IP-ASN / HTTP / process / source & port / protocol & network / logical / script / rule sets / FINAL (rule engine / rule sets / GeoIP implemented, M2a)                                               | 1     |
| DNS                 | Plain DNS, DoH / DoT / DoQ / DoH3, local mapping, hijacking, fake IP, always-real-ip                                                                                    | 1 / 3 |
| Outbound            | DIRECT / REJECT family / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / external program | 2     |
| Policy groups       | select / url-test / fallback / load-balance / smart / subnet, policy including and subscriptions, latency tests                                                         | 2     |
| Enhanced mode       | Virtual interface (Wintun / tun / utun), UDP, included and excluded routes, process identification, subnet settings                                                     | 3     |
| HTTP processing     | MITM (HTTPS decryption), URL / header / body rewrite, Map Local, request viewer and capture                                                                             | 4     |
| Scripting & modules | JavaScript engine, full Surge scripting API, module system, information panels                                                                                          | 5     |
| API & tools         | Surge-compatible HTTP API, web dashboard, logbook, latency / benchmark tests, CLI                                                                                       | 6     |
| Advanced networking | Gateway mode, DHCP server, port forwarding, built-in Snell / MTProto servers                                                                                            | 7     |
| Desktop GUI         | Cross-platform desktop client, URL scheme                                                                                                                               | 8     |

### Roadmap

1. **Phase 0** Requirements and documentation (current)
2. **Phase 1** Core skeleton: config parser, HTTP / SOCKS5 inbound, DIRECT / REJECT, rule engine, DNS, CLI, HTTP API skeleton
3. **Phase 2** All outbound protocols, policy groups, subscriptions
4. **Phase 3** Enhanced mode (TUN), fake IP, process identification
5. **Phase 4** HTTP engine: MITM, rewrites, Map Local, capture
6. **Phase 5** Scripting engine and module system
7. **Phase 6** Full HTTP API compatibility, web dashboard, testing and diagnostic tools
8. **Phase 7** Gateway / DHCP / port forwarding / built-in servers
9. **Phase 8** Desktop GUI

Near-term non-goals: iOS / tvOS builds, Surge Ponte (depends on iCloud), Apple-only UI. Tailscale integration is a long-term item pending evaluation.

### Quick start (planned)

> `rurge check` and `rurge rule match` (offline rule testing) work today; `rurge run` arrives with milestone M3.

```bash
# Build
cargo build --release

# Validate a profile
rurge check -c config.conf

# Test offline which rule a request would hit and which policy it resolves to (no daemon needed)
rurge rule match -c surge.conf www.example.com --explain

# Run
rurge run -c config.conf
```

Profiles use the Surge format directly; see the example in the Chinese section above.

### Documentation

- [Requirements (PRD, Chinese)](docs/requirements.md)
- [Surge compatibility matrix (Chinese)](docs/surge-compatibility-matrix.md)
- Per-phase design specs and implementation plans will live in `docs/superpowers/specs/` and `docs/superpowers/plans/`

### Development

- Toolchain: Rust stable via `rustup`; `cargo fmt`, `cargo clippy`, `cargo test`
- Workflow: each phase starts with a design spec, then an implementation plan, then implementation with tests
- Please read the requirements document and open an issue before large changes

### Disclaimer

- rurge is an independent open-source project and is not affiliated with NSSurge, the developer of Surge. "Surge" is a trademark of its owner and is used here only to describe compatibility.
- This project provides no proxy servers or network services. Use it in accordance with the laws of your jurisdiction.
- MITM (HTTPS decryption) and similar features must only be used on devices and traffic you own or are authorised to inspect.
- GEOIP / IP-ASN rules default to MaxMind's GeoLite2 database: This product includes GeoLite2 data created by MaxMind, available from <https://www.maxmind.com>.

### License

[MIT](LICENSE)
