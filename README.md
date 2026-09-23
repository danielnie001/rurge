<div align="center">

# rurge

**中文** | [English](README_en.md)

</div>

---

### 简介

rurge（**Ru**st + Su**rge**）是一个用 Rust 编写的跨平台网络代理工具，目标是逐步复刻 [Surge](https://nssurge.com/)（macOS / iOS）的全部功能，并做到：

- **原生兼容 Surge 配置格式**：直接使用 `.conf` 配置、`.sgmodule` 模块、`RULE-SET` / `DOMAIN-SET` 规则集、`policy-path` 策略订阅和托管配置；
- **兼容 Surge 脚本 API**：现有 `http-request` / `http-response` / `cron` / `event` / `dns` / `rule` / `generic` 脚本无需改动即可运行；
- **兼容 Surge HTTP API**：现有 Dashboard 与自动化工具可以直接对接；
- **跨平台核心优先**：Windows / Linux / macOS 共用同一个命令行守护进程，桌面 GUI 放在后期阶段。

### 当前状态

> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；其余出站协议与 `url-test` / `fallback` / `load-balance` / `smart` / `subnet` 等策略组算法、策略订阅仍在阶段 2 后续里程碑。

完整的需求、模块划分、平台差异和分阶段路线图见 [docs/requirements.md](docs/requirements.md)；
Surge 配置项 / 规则 / 参数 / API 的逐项兼容清单见 [docs/surge-compatibility-matrix.md](docs/surge-compatibility-matrix.md)。

### 计划特性

| 模块           | 内容                                                                                                                                                          | 阶段  |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----- |
| 配置与 Profile | Surge`.conf` 解析、`[General]` 全部选项、托管配置自动更新、`.sgmodule` 模块、Requirement 表达式、Keystore                                               | 1 / 5 |
| 入站           | HTTP / HTTPS 代理、SOCKS5、局域网共享与认证、系统代理设置（HTTP / SOCKS5 监听、Basic 认证、`proxy-restricted-to-lan` 已实现，M3a；三平台系统代理开关、`skip-proxy` 转换、退出 / 崩溃恢复已实现，M4b）                                                                                                     | 1     |
| 规则系统       | 域名 / IP / GEOIP / IP-ASN / HTTP / 进程 / 源与端口 / 协议与网络 / 逻辑 / 脚本 / 规则集 / FINAL（规则引擎 / 规则集 / GeoIP 已实现，M2a）                                                               | 1     |
| DNS            | 普通 DNS、DoH / DoT / DoQ / DoH3、本地映射、劫持、fake-ip、always-real-ip（普通 DNS / DoH / DoT / `tcp://` / `[Host]` / 系统 hosts 已实现，M2b）                | 1 / 3 |
| 出站协议       | DIRECT / REJECT 系列 / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / 外部程序（DIRECT 与 REJECT 系四种已实现，M3a；HTTP / SOCKS5（含 `socks5-tls`）TCP 上游与 `underlying-proxy` 链已实现，阶段 2 / M1；Trojan（TCP；WebSocket 传输）已实现，阶段 2 / M2a；VMess（AEAD 握手，TCP；可叠加 TLS / WebSocket）与 AnyTLS（TCP，会话复用）已实现，阶段 2 / M2b） | 2     |
| 策略组         | select / url-test / fallback / load-balance / smart / subnet、策略引入与订阅、延迟测试（`select` 组的选择经 API 读取与切换已实现，阶段 2 / M1）                                                                        | 2     |
| 增强模式       | 虚拟网卡（Wintun / tun / utun）、UDP、路由包含与排除、进程识别、子网设置                                                                                      | 3     |
| HTTP 处理      | MITM（HTTPS 解密）、URL / Header / Body 重写、Map Local、请求查看与抓包                                                                                       | 4     |
| 脚本与模块     | JavaScript 引擎、完整 Surge 脚本 API、模块系统、信息面板                                                                                                      | 5     |
| API 与工具     | Surge 兼容 HTTP API、Web Dashboard、Logbook、延迟 / 基准测试、CLI（阶段 1 端点与 `rurge reload/stop/status` 已实现，M4a；`rurge service install/uninstall`（基础版）已实现，M4b）                                    | 6     |
| 高级网络       | 网关模式、DHCP 服务器、端口转发、内置 Snell / MTProto 服务器                                                                                                  | 7     |
| 桌面 GUI       | 跨平台桌面客户端、URL Scheme                                                                                                                                  | 8     |

### 路线图

1. **阶段 0** 需求与文档（当前）
2. **阶段 1** 核心骨架：配置解析、HTTP / SOCKS5 入站、DIRECT / REJECT、规则引擎、DNS、CLI、HTTP API 骨架、系统代理、服务安装（HTTP API 阶段 1 端点与 `rurge reload/stop/status` 已完成，M4a；系统代理与 `rurge service install/uninstall`（基础版）已完成，M4b）
3. **阶段 2** 出站协议全集、策略组、策略订阅（进行中：M1 出站地基与 HTTP / SOCKS5 上游已完成；M2a（Trojan，TCP + WebSocket）已完成；M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成）
4. **阶段 3** 增强模式（TUN）、fake-ip、进程识别
5. **阶段 4** HTTP 引擎：MITM、重写、Map Local、抓包
6. **阶段 5** 脚本引擎与模块系统
7. **阶段 6** HTTP API 全量兼容、Web Dashboard、测试与诊断工具
8. **阶段 7** 网关 / DHCP / 端口转发 / 内置服务器
9. **阶段 8** 桌面 GUI

近期非目标：iOS / tvOS 版本、Surge Ponte（依赖 iCloud）、Apple 平台专属界面；Tailscale 集成列为远期评估项。

### 快速开始（计划中的形态）

> `rurge check`、`rurge rule match`、`rurge dns lookup` 与 `rurge run`（HTTP / SOCKS5 代理，DIRECT / REJECT，以及 `http` / `https` / `socks5` / `socks5-tls` / `trojan` / `vmess` / `anytls` 上游——均可叠加 Shadow TLS，含 `underlying-proxy` 链）已可用；策略组算法在阶段 2 后续里程碑。HTTP API 与 `rurge reload` / `stop` / `status` 已可用（见 [docs/api/phase1.md](docs/api/phase1.md)，阶段 2 新增端点见 [docs/api/phase2.md](docs/api/phase2.md)）。`rurge run --system-proxy` 可以把系统代理指向 rurge，退出时恢复、崩溃后在下次启动时恢复；`rurge service install | uninstall [--user] [--dry-run]` 可以注册 / 移除开机自启（systemd / launchd / Windows 计划任务）。macOS 经 `networksetup` 设置，通常需要管理员账户；是否需要 `sudo` 尚未在真机验证（见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)），失败时 rurge 原样报出工具的错误。`rurge run` 另支持 `--idle-timeout`、`--request-log-size`、`--watch`（配置热重载）、`--log-file`（按天滚动）等 rurge 专有运行时选项，只经命令行参数 / 环境变量提供，不写入 Surge 配置文件。

```bash
# 构建
cargo build --release

# 校验配置
rurge check -c config.conf

# 离线测试某个请求会命中哪条规则、落到哪个策略（无需启动守护进程）
rurge rule match -c surge.conf www.example.com --explain

# 按配置的 DNS 设置离线解析一个域名
rurge dns lookup -c surge.conf www.example.com --trace

# 运行
rurge run -c config.conf

# 运行，并把系统代理指向 rurge（退出时恢复；崩溃后在下次启动时恢复）
rurge run -c config.conf --system-proxy

# 先看清注册开机自启会做什么（去掉 --dry-run 才真正安装）
rurge service install -c config.conf --user --dry-run
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
